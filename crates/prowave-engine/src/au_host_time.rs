//! macOS: the host's tempo and position as an Audio Unit asks for them (Logic: what tempo-synced
//! arpeggiators, LFOs and delays follow).
//!
//! A version 2 Audio Unit asks through the callbacks the host sets with
//! `kAudioUnitProperty_HostCallbacks`: beat and tempo, the musical time location (time
//! signature, downbeat, samples to the next beat) and the transport state (playing,
//! recording, cycling, sample time). Each answers from the [`BlockTransport`] of the block
//! being rendered: the same per-block snapshot a VST 3 plug-in reads as its `ProcessContext`
//! ([`crate::vst3::fill_context`]), including where the lane put it for delay compensation.
//!
//! The snapshot sits in atomics behind a sequence counter, so the callbacks never lock or
//! allocate, and a unit that asks from another thread (its window drawing a tempo) reads a
//! consistent copy instead of racing the audio thread.

use objc2_audio_toolbox::{kAudioUnitErr_CannotDoInCurrentContext, HostCallbackInfo};
use std::ffi::c_void;
use std::sync::atomic::{fence, AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

use crate::plugin_track::BlockTransport;

/// The transport an Audio Unit reads, written by the thread that renders it. Boxed by its
/// owner: the unit holds its address.
pub(crate) struct HostTime {
    sample_rate: f64,
    /// Odd while a new snapshot is being written.
    seq: AtomicU32,
    playing: AtomicBool,
    recording: AtomicBool,
    sample_pos: AtomicI64,
    beat: AtomicU64,
    bar_start: AtomicU64,
    tempo: AtomicU64,
    /// Numerator in the high byte, denominator in the low byte.
    sig: AtomicU32,
    cycling: AtomicBool,
    cycle_start: AtomicU64,
    cycle_end: AtomicU64,
    /// A start, stop, locate or cycle change since the unit last asked for the transport.
    changed: AtomicBool,
    /// Callback calls answered (to tell whether a unit asks at all).
    calls: AtomicU32,
}

/// What the musical-time-location callback answers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MusicalTime {
    /// Samples from the block's first frame to the next whole beat (0 on a beat).
    pub samples_to_next_beat: u32,
    pub numerator: f32,
    pub denominator: u32,
    /// The beat (quarter notes) at which the bar being rendered began.
    pub downbeat: f64,
}

/// What the transport-state callbacks answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TransportState {
    pub playing: bool,
    pub recording: bool,
    pub sample: f64,
    pub cycling: bool,
    pub cycle_start: f64,
    pub cycle_end: f64,
}

/// Beat (quarter notes from the project start) and tempo, as VST 3 gets them in
/// `projectTimeMusic` and `tempo`.
pub(crate) fn beat_and_tempo(t: &BlockTransport) -> (f64, f64) {
    (t.beat, t.tempo)
}

/// Time signature, downbeat and the samples to the next whole beat at `sample_rate`.
pub(crate) fn musical_time(t: &BlockTransport, sample_rate: f64) -> MusicalTime {
    let next = t.beat.ceil();
    let samples = if t.tempo > 0.0 && t.beat.is_finite() {
        ((next - t.beat) * 60.0 / t.tempo * sample_rate).round()
    } else {
        0.0
    };
    MusicalTime {
        samples_to_next_beat: samples.clamp(0.0, f64::from(u32::MAX)) as u32,
        numerator: f32::from(t.sig.0.max(1)),
        denominator: u32::from(t.sig.1.max(1)),
        downbeat: t.bar_start_beat,
    }
}

/// Playing, recording, the sample in the timeline and the cycle locators (zero when not
/// cycling, as VST 3's `cycleStartMusic`/`cycleEndMusic`).
pub(crate) fn transport_state(t: &BlockTransport) -> TransportState {
    let (cs, ce) = t.cycle.unwrap_or((0.0, 0.0));
    TransportState {
        playing: t.playing,
        recording: t.recording,
        sample: t.sample_pos as f64,
        cycling: t.cycle.is_some(),
        cycle_start: cs,
        cycle_end: ce,
    }
}

fn pack_sig(sig: (u8, u8)) -> u32 {
    (u32::from(sig.0) << 8) | u32::from(sig.1)
}

impl HostTime {
    pub(crate) fn new(sample_rate: f64) -> Self {
        let t = BlockTransport::default();
        Self {
            sample_rate,
            seq: AtomicU32::new(0),
            playing: AtomicBool::new(t.playing),
            recording: AtomicBool::new(t.recording),
            sample_pos: AtomicI64::new(t.sample_pos),
            beat: AtomicU64::new(t.beat.to_bits()),
            bar_start: AtomicU64::new(t.bar_start_beat.to_bits()),
            tempo: AtomicU64::new(t.tempo.to_bits()),
            sig: AtomicU32::new(pack_sig(t.sig)),
            cycling: AtomicBool::new(false),
            cycle_start: AtomicU64::new(0),
            cycle_end: AtomicU64::new(0),
            changed: AtomicBool::new(false),
            calls: AtomicU32::new(0),
        }
    }

    /// Publishes the transport of the block about to render. One writer (the rendering
    /// thread). No allocation, no lock.
    pub(crate) fn store(&self, t: &BlockTransport) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        let r = Ordering::Relaxed;
        self.playing.store(t.playing, r);
        self.recording.store(t.recording, r);
        self.sample_pos.store(t.sample_pos, r);
        self.beat.store(t.beat.to_bits(), r);
        self.bar_start.store(t.bar_start_beat.to_bits(), r);
        self.tempo.store(t.tempo.to_bits(), r);
        self.sig.store(pack_sig(t.sig), r);
        let (cs, ce) = t.cycle.unwrap_or((0.0, 0.0));
        self.cycling.store(t.cycle.is_some(), r);
        self.cycle_start.store(cs.to_bits(), r);
        self.cycle_end.store(ce.to_bits(), r);
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    /// Marks a start, stop, locate or cycle change for the next transport-state answer.
    pub(crate) fn mark_changed(&self) {
        self.changed.store(true, Ordering::Relaxed);
    }

    fn read(&self) -> BlockTransport {
        let r = Ordering::Relaxed;
        let sig = self.sig.load(r);
        BlockTransport {
            playing: self.playing.load(r),
            recording: self.recording.load(r),
            sample_pos: self.sample_pos.load(r),
            beat: f64::from_bits(self.beat.load(r)),
            bar_start_beat: f64::from_bits(self.bar_start.load(r)),
            tempo: f64::from_bits(self.tempo.load(r)),
            sig: ((sig >> 8) as u8, sig as u8),
            cycle: self.cycling.load(r).then(|| {
                (
                    f64::from_bits(self.cycle_start.load(r)),
                    f64::from_bits(self.cycle_end.load(r)),
                )
            }),
        }
    }

    /// The newest whole snapshot. On the rendering thread the first try always succeeds;
    /// another thread retries a bounded number of times while writes are under way and then
    /// gives up (`None`: the callbacks answer "cannot do in this context", as Apple allows
    /// outside a render call). Never blocks.
    pub(crate) fn load(&self) -> Option<BlockTransport> {
        for _ in 0..64 {
            let a = self.seq.load(Ordering::Acquire);
            if a & 1 == 0 {
                let t = self.read();
                fence(Ordering::Acquire);
                if self.seq.load(Ordering::Relaxed) == a {
                    return Some(t);
                }
            }
            std::hint::spin_loop();
        }
        None
    }

    /// Callback calls answered so far.
    pub(crate) fn calls(&self) -> u32 {
        self.calls.load(Ordering::Relaxed)
    }

    /// The callbacks to register for a unit, answering from `self` (which must outlive the
    /// unit's use of them).
    pub(crate) fn callbacks(&self) -> HostCallbackInfo {
        HostCallbackInfo {
            hostUserData: (self as *const Self).cast_mut().cast(),
            beatAndTempoProc: Some(beat_and_tempo_proc),
            musicalTimeLocationProc: Some(musical_time_location_proc),
            transportStateProc: Some(transport_state_proc),
            transportStateProc2: Some(transport_state_proc2),
        }
    }
}

/// Writes `v` through `p` unless the unit passed null (it does not want that value).
///
/// # Safety
/// `p` is null or valid for a write of `T`.
#[inline]
unsafe fn put<T>(p: *mut T, v: T) {
    if let Some(p) = p.as_mut() {
        *p = v;
    }
}

/// # Safety
/// `user` is null or the `HostTime` the callbacks were registered with.
#[inline]
unsafe fn host<'a>(user: *mut c_void) -> Option<&'a HostTime> {
    let h = user.cast::<HostTime>().cast_const().as_ref()?;
    h.calls.fetch_add(1, Ordering::Relaxed);
    Some(h)
}

pub(crate) unsafe extern "C-unwind" fn beat_and_tempo_proc(
    user: *mut c_void,
    beat: *mut f64,
    tempo: *mut f64,
) -> i32 {
    let Some(h) = host(user) else {
        return kAudioUnitErr_CannotDoInCurrentContext;
    };
    let Some(now) = h.load() else {
        return kAudioUnitErr_CannotDoInCurrentContext;
    };
    let (b, t) = beat_and_tempo(&now);
    put(beat, b);
    put(tempo, t);
    0
}

pub(crate) unsafe extern "C-unwind" fn musical_time_location_proc(
    user: *mut c_void,
    to_next_beat: *mut u32,
    numerator: *mut f32,
    denominator: *mut u32,
    downbeat: *mut f64,
) -> i32 {
    let Some(h) = host(user) else {
        return kAudioUnitErr_CannotDoInCurrentContext;
    };
    let Some(now) = h.load() else {
        return kAudioUnitErr_CannotDoInCurrentContext;
    };
    let m = musical_time(&now, h.sample_rate);
    put(to_next_beat, m.samples_to_next_beat);
    put(numerator, m.numerator);
    put(denominator, m.denominator);
    put(downbeat, m.downbeat);
    0
}

pub(crate) unsafe extern "C-unwind" fn transport_state_proc(
    user: *mut c_void,
    playing: *mut u8,
    changed: *mut u8,
    sample: *mut f64,
    cycling: *mut u8,
    cycle_start: *mut f64,
    cycle_end: *mut f64,
) -> i32 {
    transport_state_proc2(
        user,
        playing,
        std::ptr::null_mut(),
        changed,
        sample,
        cycling,
        cycle_start,
        cycle_end,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C-unwind" fn transport_state_proc2(
    user: *mut c_void,
    playing: *mut u8,
    recording: *mut u8,
    changed: *mut u8,
    sample: *mut f64,
    cycling: *mut u8,
    cycle_start: *mut f64,
    cycle_end: *mut f64,
) -> i32 {
    let Some(h) = host(user) else {
        return kAudioUnitErr_CannotDoInCurrentContext;
    };
    let Some(now) = h.load() else {
        return kAudioUnitErr_CannotDoInCurrentContext;
    };
    let s = transport_state(&now);
    put(playing, u8::from(s.playing));
    put(recording, u8::from(s.recording));
    if !changed.is_null() {
        put(changed, u8::from(h.changed.swap(false, Ordering::Relaxed)));
    }
    put(sample, s.sample);
    put(cycling, u8::from(s.cycling));
    put(cycle_start, s.cycle_start);
    put(cycle_end, s.cycle_end);
    0
}

/// Whether the transport jumped or changed state between two block snapshots, given the
/// frames rendered since `prev` was taken.
pub(crate) fn discontinuous(prev: &BlockTransport, next: &BlockTransport, frames: i64) -> bool {
    let expected = prev.sample_pos + if prev.playing { frames } else { 0 };
    next.playing != prev.playing
        || next.cycle != prev.cycle
        // One sample of slack: the engine rounds each block's start to a sample.
        || (next.sample_pos - expected).abs() > 1
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::tempo_map::{SignatureEvent, TempoCurve, TempoEvent, TempoMap};

    const SR: f64 = 48_000.0;

    /// The transport at `beat`, computed the way the engine's per-block snapshot is
    /// (`Engine::block_transport` in core.rs).
    fn snapshot(
        map: &TempoMap,
        beat: f64,
        playing: bool,
        cycle: Option<(f64, f64)>,
    ) -> BlockTransport {
        let pos = map.position(beat);
        BlockTransport {
            playing,
            recording: false,
            sample_pos: (map.seconds_at(beat) * SR).round() as i64,
            beat,
            bar_start_beat: map.beat_of_bar(pos.bar),
            tempo: f64::from(map.tempo_at(beat)),
            sig: (pos.num, pos.den),
            cycle,
        }
    }

    /// 117.02 BPM in 4/4, 140 from bar 3 (beat 8), 7/8 from bar 4.
    fn map() -> TempoMap {
        let step = |beat, bpm| TempoEvent {
            beat,
            bpm,
            curve: TempoCurve::Step,
        };
        TempoMap::new(
            vec![step(0.0, 117.02), step(8.0, 140.0)],
            vec![
                SignatureEvent {
                    bar: 0,
                    num: 4,
                    den: 4,
                },
                SignatureEvent {
                    bar: 3,
                    num: 7,
                    den: 8,
                },
            ],
        )
    }

    struct Answers {
        beat: f64,
        tempo: f64,
        to_next: u32,
        num: f32,
        den: u32,
        downbeat: f64,
        playing: u8,
        recording: u8,
        changed: u8,
        sample: f64,
        cycling: u8,
        cs: f64,
        ce: f64,
    }

    /// Calls each callback in `cb` as a unit would.
    fn ask(cb: &HostCallbackInfo) -> Answers {
        let mut a = Answers {
            beat: -1.0,
            tempo: -1.0,
            to_next: 9,
            num: 0.0,
            den: 0,
            downbeat: -1.0,
            playing: 9,
            recording: 9,
            changed: 9,
            sample: -1.0,
            cycling: 9,
            cs: -1.0,
            ce: -1.0,
        };
        let u = cb.hostUserData;
        // SAFETY: every pointer is to a local that lives across the call; `u` is the
        // HostTime the callbacks were made from.
        unsafe {
            let f = cb.beatAndTempoProc.expect("beat and tempo");
            assert_eq!(f(u, &mut a.beat, &mut a.tempo), 0);
            let f = cb.musicalTimeLocationProc.expect("musical time");
            assert_eq!(
                f(u, &mut a.to_next, &mut a.num, &mut a.den, &mut a.downbeat),
                0
            );
            let f = cb.transportStateProc2.expect("transport 2");
            let mut changed = 0u8;
            assert_eq!(
                f(
                    u,
                    &mut a.playing,
                    &mut a.recording,
                    &mut changed,
                    &mut a.sample,
                    &mut a.cycling,
                    &mut a.cs,
                    &mut a.ce
                ),
                0
            );
            a.changed = changed;
            // The first form answers the same (and the change was taken by the call above).
            let f = cb.transportStateProc.expect("transport");
            let (mut p, mut c, mut s, mut cy, mut x, mut y) = (9u8, 9u8, -1.0, 9u8, -1.0, -1.0);
            assert_eq!(f(u, &mut p, &mut c, &mut s, &mut cy, &mut x, &mut y), 0);
            assert_eq!(
                (p, c, s, cy, x, y),
                (a.playing, 0, a.sample, a.cycling, a.cs, a.ce)
            );
            // A unit may pass null for what it does not want.
            use std::ptr::null_mut as n;
            assert_eq!(cb.beatAndTempoProc.expect("b")(u, n(), n()), 0);
            assert_eq!(
                cb.musicalTimeLocationProc.expect("m")(u, n(), n(), n(), n()),
                0
            );
            assert_eq!(
                cb.transportStateProc2.expect("t")(u, n(), n(), n(), n(), n(), n(), n()),
                0
            );
        }
        a
    }

    #[test]
    fn callbacks_answer_beat_tempo_and_bar_at_117_02_and_after_a_tempo_change() {
        let m = map();
        let h = Box::new(HostTime::new(SR));
        let cb = h.callbacks();

        // Bar 2, beat 2 and a half at 117.02 (beat 5.5), playing in a cycle.
        let t = snapshot(&m, 5.5, true, Some((4.0, 12.0)));
        h.store(&t);
        h.mark_changed();
        let a = assert_no_alloc::assert_no_alloc(|| ask(&cb));
        assert_eq!(a.beat, 5.5);
        assert!((a.tempo - 117.02).abs() < 1e-4, "{}", a.tempo);
        assert_eq!((a.num, a.den, a.downbeat), (4.0, 4, 4.0));
        // Half a beat at 117.02 BPM: 0.5 * 60 / 117.02 s.
        let want = (0.5 * 60.0 / a.tempo * SR).round() as u32;
        assert_eq!(a.to_next, want);
        assert_eq!(a.to_next, 12_306);
        assert_eq!((a.playing, a.recording, a.changed), (1, 0, 1));
        assert_eq!(a.sample, t.sample_pos as f64);
        // 5.5 beats at 117.02: 2.81994... s.
        assert_eq!(t.sample_pos, (5.5 * 60.0 / a.tempo * SR).round() as i64);
        assert_eq!((a.cycling, a.cs, a.ce), (1, 4.0, 12.0));
        // The change is told once.
        assert_eq!(ask(&cb).changed, 0);

        // Bar 3 at 140 (beat 9.25): the downbeat is beat 8; samples count both tempos.
        let t = snapshot(&m, 9.25, true, None);
        h.store(&t);
        let a = ask(&cb);
        assert_eq!((a.beat, a.tempo), (9.25, 140.0));
        assert_eq!((a.num, a.den, a.downbeat), (4.0, 4, 8.0));
        assert_eq!(a.to_next, (0.75 * 60.0 / 140.0 * SR).round() as u32);
        let secs = 8.0 * 60.0 / 117.02f32 as f64 + 1.25 * 60.0 / 140.0;
        assert_eq!(a.sample, (secs * SR).round());
        assert_eq!((a.cycling, a.cs, a.ce), (0, 0.0, 0.0));

        // Bar 4 in 7/8 at 140: it begins at beat 12; on a beat the next one is 0 samples away.
        let a = {
            h.store(&snapshot(&m, 13.0, false, None));
            ask(&cb)
        };
        assert_eq!((a.num, a.den, a.downbeat), (7.0, 8, 12.0));
        assert_eq!((a.to_next, a.playing), (0, 0));
        assert!(h.calls() > 0);
    }

    /// For the same block, an Audio Unit hears what a VST 3 plug-in reads in its process
    /// context.
    #[test]
    fn callbacks_match_the_vst3_process_context() {
        use vst3::Steinberg::Vst::ProcessContext;
        use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::*;
        let m = map();
        let h = Box::new(HostTime::new(SR));
        let cb = h.callbacks();
        for (beat, playing, cycle) in [
            (0.0, false, None),
            (5.5, true, Some((4.0, 12.0))),
            (7.999, true, None),
            (9.25, true, Some((8.0, 16.0))),
            (14.1, false, None),
        ] {
            let t = BlockTransport {
                recording: playing && beat > 9.0,
                ..snapshot(&m, beat, playing, cycle)
            };
            h.store(&t);
            let a = ask(&cb);
            // SAFETY: zeroed plain data.
            let mut c: ProcessContext = unsafe { std::mem::zeroed() };
            crate::vst3::fill_context(&t, SR, 0, &mut c);
            assert_eq!(a.beat, c.projectTimeMusic, "beat at {beat}");
            assert_eq!(a.tempo, c.tempo, "tempo at {beat}");
            assert_eq!(a.downbeat, c.barPositionMusic, "bar at {beat}");
            assert_eq!(a.num as i32, c.timeSigNumerator);
            assert_eq!(a.den as i32, c.timeSigDenominator);
            assert_eq!(a.sample as i64, c.projectTimeSamples);
            assert_eq!(a.playing != 0, c.state & kPlaying != 0);
            assert_eq!(a.recording != 0, c.state & kRecording != 0);
            assert_eq!(a.cycling != 0, c.state & kCycleActive != 0);
            assert_eq!((a.cs, a.ce), (c.cycleStartMusic, c.cycleEndMusic));
        }
    }

    #[test]
    fn starts_stops_and_locates_are_changes_but_playing_on_is_not() {
        let m = map();
        let a = snapshot(&m, 1.0, true, None);
        let later = snapshot(&m, 1.0 + 512.0 / SR * 117.02 / 60.0, true, None);
        let frames = later.sample_pos - a.sample_pos;
        assert!(!discontinuous(&a, &later, frames));
        assert!(discontinuous(&a, &later, frames + 64), "a locate");
        assert!(discontinuous(
            &a,
            &BlockTransport {
                playing: false,
                ..a
            },
            0
        ));
        assert!(discontinuous(
            &a,
            &BlockTransport {
                cycle: Some((0.0, 4.0)),
                ..a
            },
            0
        ));
        let stopped = BlockTransport {
            playing: false,
            ..a
        };
        assert!(!discontinuous(&stopped, &stopped, 512), "stopped stays put");
    }

    #[test]
    fn a_reader_on_another_thread_sees_whole_snapshots() {
        let at = |i: i32| {
            let x = f64::from(i);
            BlockTransport {
                beat: x,
                bar_start_beat: x,
                tempo: x + 60.0,
                sample_pos: i64::from(i),
                ..BlockTransport::default()
            }
        };
        let h = std::sync::Arc::new(HostTime::new(SR));
        h.store(&at(0));
        let w = h.clone();
        let writer = std::thread::spawn(move || {
            for i in 1..20_000 {
                w.store(&at(i));
            }
        });
        let mut whole = 0;
        for _ in 0..20_000 {
            // While the writer is busy a read may give up; what it does return is never torn.
            let Some(t) = h.load() else { continue };
            whole += 1;
            assert_eq!(t.beat, t.bar_start_beat);
            assert_eq!(t.tempo, t.beat + 60.0);
            assert_eq!(t.sample_pos as f64, t.beat);
        }
        assert!(writer.join().is_ok());
        assert!(whole > 0);
        assert_eq!(h.load().map(|t| t.beat), Some(19_999.0));
    }

    #[test]
    fn a_null_host_is_refused() {
        let (mut b, mut t) = (0.0, 0.0);
        // SAFETY: null user data is what is being tested; the outputs are locals.
        let s = unsafe { beat_and_tempo_proc(std::ptr::null_mut(), &mut b, &mut t) };
        assert_eq!(s, kAudioUnitErr_CannotDoInCurrentContext);
    }
}
