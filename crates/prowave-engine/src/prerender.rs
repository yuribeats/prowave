//! Logic Pro's Process Buffer Range: two buffer sizes at once.
//!
//! Live tracks (record-enabled, monitoring, the selected instrument being played, and the Aux
//! strips they reach) run their plug-ins in the I/O callback at the I/O buffer size, as
//! before. Every other track's plug-ins run on the playback thread in larger blocks (the
//! process buffer: Small 256, Medium 512, Large 1024 frames), so a heavy plug-in has the time
//! of a whole large block instead of the I/O callback's few hundred microseconds.
//!
//! The engine mixes one frame at a time in the I/O callback, on the transport as it plays
//! there: notes, regions, automation, tempo and MIDI clock are all read for the frame being
//! made. A pre-rendered plug-in gets its frames (and the notes, automation and transport that
//! belong to them, at their offsets) through a lock-free lane, the playback thread processes
//! them a process buffer at a time, and its sound comes back [`prerender_delay`] frames after
//! its input. Delay compensation makes up that delay like any plug-in's latency: every other
//! strip waits for it, live tracks hear their regions that much later and their input at
//! once, fader automation is read that much late and MIDI out is stamped that much later.
//!
//! Nothing waits: if the playback thread falls behind, the I/O callback plays silence for the
//! frames it does not have, counts an underrun and drops the late frames when they come, so
//! the track stays in time.

/// Record > Process Buffer sizes, in frames (Logic: Small, Medium, Large).
pub const PROCESS_BUFFER_SMALL: u32 = 256;
pub const PROCESS_BUFFER_MEDIUM: u32 = 512;
pub const PROCESS_BUFFER_LARGE: u32 = 1024;
/// The largest process buffer.
pub const MAX_PROCESS_BUFFER: usize = PROCESS_BUFFER_LARGE as usize;
/// Frames of input (and of output) a lane holds between the I/O callback and the playback
/// thread.
pub(crate) const LANE_FRAMES: usize = 8192;

/// Frames a pre-rendered plug-in is heard after its input, with a process buffer of
/// `process` frames and an I/O buffer of `io`: one process buffer to gather a block, and the
/// time of one more (or of one I/O buffer, when that is longer) for the playback thread to
/// render it before the I/O callback needs it. 0 when pre-rendering is off (`process` 0) or
/// the I/O buffer is too large for the lane.
pub fn prerender_delay(process: u32, io: u32) -> u32 {
    if process == 0 {
        return 0;
    }
    let p = process.min(PROCESS_BUFFER_LARGE);
    let d = p + p.max(io);
    if (d + 2 * p) as usize > LANE_FRAMES {
        0
    } else {
        d
    }
}

/// The delay a track's plug-ins add to its sound by being processed in blocks, in frames
/// (their own latency comes on top): the instrument and the effects are each one hop, of an
/// I/O buffer (`io`) when the track is live or pre-rendering is off, else of
/// [`prerender_delay`].
pub fn plugin_host_delay(
    has_instrument: bool,
    has_effects: bool,
    live: bool,
    process: u32,
    io: u32,
) -> usize {
    let d = prerender_delay(process, io);
    let hop = if live || d == 0 { io } else { d } as usize;
    hop * (usize::from(has_instrument) + usize::from(has_effects))
}

use crate::core::Shared;
use crate::plugin_track::LaneWork;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// Messages from the I/O callback to the playback thread.
pub(crate) enum ToWorker {
    /// A track's plug-ins, to be pre-rendered from now on.
    Attach(Box<LaneWork>),
    /// Give the plug-ins of this lane back to the I/O callback.
    Detach(u32),
    /// The track of this lane is gone: free its plug-ins (off the real-time threads).
    Retire(u32),
    /// Plug-ins nobody wants any more, to be freed.
    Dispose(Box<LaneWork>),
    /// The process buffer and the sample rate.
    Chunk(u32, f32),
}

/// Messages the playback thread queue holds; past that the I/O callback tries again.
const PORT_CAPACITY: usize = 256;
/// Plug-ins on their way back to the I/O callback, or to be freed by the UI.
const BACK_CAPACITY: usize = 256;

/// The I/O callback's side of the playback thread.
pub(crate) struct WorkerPort {
    tx: Producer<ToWorker>,
    back: Consumer<Box<LaneWork>>,
    thread: Option<std::thread::Thread>,
    /// Lanes on the playback thread (or on their way there).
    away: usize,
    /// The process buffer this thread was last told (0: not yet).
    pub(crate) chunk_sent: u32,
}

impl WorkerPort {
    pub(crate) fn attach(&mut self, work: Box<LaneWork>) -> Result<(), Box<LaneWork>> {
        match self.tx.push(ToWorker::Attach(work)) {
            Ok(()) => {
                self.away += 1;
                Ok(())
            }
            Err(rtrb::PushError::Full(ToWorker::Attach(w))) => Err(w),
            Err(rtrb::PushError::Full(_)) => Ok(()),
        }
    }

    pub(crate) fn detach(&mut self, id: u32) -> bool {
        self.tx.push(ToWorker::Detach(id)).is_ok()
    }

    pub(crate) fn retire(&mut self, id: u32) -> bool {
        let ok = self.tx.push(ToWorker::Retire(id)).is_ok();
        if ok {
            self.away = self.away.saturating_sub(1);
        }
        ok
    }

    pub(crate) fn dispose(&mut self, work: Box<LaneWork>) -> Result<(), Box<LaneWork>> {
        match self.tx.push(ToWorker::Dispose(work)) {
            Ok(()) => Ok(()),
            Err(rtrb::PushError::Full(ToWorker::Dispose(w))) => Err(w),
            Err(rtrb::PushError::Full(_)) => Ok(()),
        }
    }

    pub(crate) fn set_chunk(&mut self, frames: u32, sample_rate: f32) -> bool {
        self.tx.push(ToWorker::Chunk(frames, sample_rate)).is_ok()
    }

    /// Plug-ins the playback thread gave back.
    pub(crate) fn returned(&mut self) -> Option<Box<LaneWork>> {
        let w = self.back.pop().ok()?;
        self.away = self.away.saturating_sub(1);
        Some(w)
    }

    /// Lanes on this playback thread.
    pub(crate) fn lanes(&self) -> usize {
        self.away
    }

    /// Whether any lane is on the playback thread.
    pub(crate) fn busy(&self) -> bool {
        self.away > 0
    }

    /// Wakes the playback thread: new frames are in the lanes. Never blocks.
    pub(crate) fn wake(&self) {
        if let Some(t) = &self.thread {
            t.unpark();
        }
    }
}

/// The playback thread's state: the pre-rendered lanes. [`Prerender::step`] does one round;
/// [`spawn`] runs it on a real-time thread, tests call it themselves.
// The lanes stay boxed: they move between threads through rings as one pointer.
#[allow(clippy::vec_box)]
pub struct Prerender {
    rx: Consumer<ToWorker>,
    back: Producer<Box<LaneWork>>,
    /// Plug-ins to be freed by the UI thread (the engine collects them).
    trash: Producer<Box<LaneWork>>,
    lanes: Vec<Box<LaneWork>>,
    /// Plug-ins that could not be handed on yet (a full queue).
    held_back: Vec<Box<LaneWork>>,
    held_trash: Vec<Box<LaneWork>>,
    chunk: usize,
    sr: f32,
    shared: Arc<Shared>,
}

/// How many playback threads to run on a machine with `cores` cores: one per core that the
/// I/O thread and the UI leave over (Apple Silicon's performance cores are about half), at
/// least one and at most four.
pub fn playback_threads(cores: usize) -> usize {
    (cores / 2).saturating_sub(1).clamp(1, 4)
}

/// The UI's end of what the playback thread frees.
pub(crate) type WorkerTrash = Consumer<Box<LaneWork>>;

/// A playback thread (not running yet) and the I/O callback's port to it.
pub(crate) fn channel(shared: Arc<Shared>) -> (WorkerPort, Prerender, WorkerTrash) {
    let (tx, rx) = RingBuffer::new(PORT_CAPACITY);
    let (back_tx, back) = RingBuffer::new(BACK_CAPACITY);
    let (trash, trash_rx) = RingBuffer::new(BACK_CAPACITY);
    let cap = crate::core::MAX_TRACKS;
    (
        WorkerPort {
            tx,
            back,
            thread: None,
            away: 0,
            chunk_sent: 0,
        },
        Prerender {
            rx,
            back: back_tx,
            trash,
            lanes: Vec::with_capacity(cap),
            held_back: Vec::with_capacity(cap),
            held_trash: Vec::with_capacity(cap),
            chunk: PROCESS_BUFFER_MEDIUM as usize,
            sr: 48_000.0,
            shared,
        },
        trash_rx,
    )
}

impl Prerender {
    /// One round: takes the messages, then renders every lane's process buffers that have
    /// arrived. Returns the frames the busiest lane advanced. No allocation, no waiting.
    pub fn step(&mut self) -> usize {
        while let Ok(m) = self.rx.pop() {
            match m {
                ToWorker::Attach(w) => {
                    if self.lanes.len() < self.lanes.capacity() {
                        self.lanes.push(w);
                    } else {
                        self.give_back(w);
                    }
                }
                ToWorker::Detach(id) => {
                    if let Some(i) = self.lanes.iter().position(|w| w.id == id) {
                        let w = self.lanes.swap_remove(i);
                        self.give_back(w);
                    }
                }
                ToWorker::Retire(id) => {
                    if let Some(i) = self.lanes.iter().position(|w| w.id == id) {
                        let w = self.lanes.swap_remove(i);
                        self.throw_away(w);
                    }
                }
                ToWorker::Dispose(w) => self.throw_away(w),
                ToWorker::Chunk(frames, sr) => {
                    self.chunk = (frames as usize).clamp(1, MAX_PROCESS_BUFFER);
                    self.sr = sr.max(1.0);
                }
            }
        }
        while let Some(w) = self.held_back.pop() {
            if let Err(rtrb::PushError::Full(w)) = self.back.push(w) {
                self.held_back.push(w);
                break;
            }
        }
        while let Some(w) = self.held_trash.pop() {
            if let Err(rtrb::PushError::Full(w)) = self.trash.push(w) {
                self.held_trash.push(w);
                break;
            }
        }
        let mut most = 0;
        for w in self.lanes.iter_mut() {
            let mut done = 0;
            while w.process_chunk(self.chunk, self.sr, Some(&self.shared)) {
                done += self.chunk;
            }
            most = most.max(done);
        }
        most
    }

    fn give_back(&mut self, w: Box<LaneWork>) {
        if let Err(rtrb::PushError::Full(w)) = self.back.push(w) {
            if self.held_back.len() < self.held_back.capacity() {
                self.held_back.push(w);
            } else {
                // Cannot happen: there are never more lanes than tracks.
                std::mem::forget(w);
            }
        }
    }

    fn throw_away(&mut self, w: Box<LaneWork>) {
        if let Err(rtrb::PushError::Full(w)) = self.trash.push(w) {
            if self.held_trash.len() < self.held_trash.capacity() {
                self.held_trash.push(w);
            } else {
                std::mem::forget(w);
            }
        }
    }

    /// Lanes being pre-rendered.
    pub fn lanes(&self) -> usize {
        self.lanes.len()
    }
}

/// The running playback thread; dropping it stops the thread. The plug-ins still on it come
/// back with it and are freed on the thread that dropped it: disposing of an Audio Unit can
/// wait for the main thread, which is waiting for the playback thread to end.
pub(crate) struct WorkerThread {
    quit: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<Prerender>>,
}

impl Drop for WorkerThread {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            h.thread().unpark();
            if let Ok(prerender) = h.join() {
                drop(prerender);
            }
        }
    }
}

/// Starts the playback thread for `prerender`, wakes it through `port`. `workgroup_device`
/// is the Core Audio device whose I/O workgroup it joins (0: none yet); it follows changes.
pub(crate) fn spawn(
    mut prerender: Prerender,
    port: &mut WorkerPort,
    workgroup_device: Arc<AtomicU32>,
) -> std::io::Result<WorkerThread> {
    let quit = Arc::new(AtomicBool::new(false));
    let stop = quit.clone();
    let handle = std::thread::Builder::new()
        .name("prowave-playback".into())
        .spawn(move || {
            #[cfg(target_os = "macos")]
            let mut rt = crate::rt_thread::RealTime::new();
            while !stop.load(Ordering::Relaxed) {
                #[cfg(target_os = "macos")]
                rt.follow(
                    workgroup_device.load(Ordering::Relaxed),
                    prerender.chunk as u32,
                    prerender.sr,
                );
                #[cfg(not(target_os = "macos"))]
                let _ = &workgroup_device;
                let started = std::time::Instant::now();
                let frames = assert_no_alloc::assert_no_alloc(|| prerender.step());
                if frames > 0 {
                    let audio = frames as f64 / f64::from(prerender.sr);
                    let load = started.elapsed().as_secs_f64() / audio;
                    prerender.shared.record_worker_load(load as f32);
                } else {
                    // Woken by the I/O callback after each of its blocks; the timeout is
                    // only a safety net.
                    std::thread::park_timeout(std::time::Duration::from_millis(5));
                }
            }
            prerender
        })?;
    port.thread = Some(handle.thread().clone());
    Ok(WorkerThread {
        quit,
        handle: Some(handle),
    })
}

#[cfg(test)]
mod delay_tests {
    use super::*;

    #[test]
    fn the_delay_is_a_process_buffer_and_the_longer_of_it_and_the_io_buffer() {
        assert_eq!(prerender_delay(0, 64), 0);
        assert_eq!(prerender_delay(256, 64), 512);
        assert_eq!(prerender_delay(512, 64), 1024);
        assert_eq!(prerender_delay(1024, 1024), 2048);
        assert_eq!(prerender_delay(256, 1024), 1280);
        // An I/O buffer larger than the lane can carry: everything runs in the callback.
        assert_eq!(prerender_delay(1024, 8192), 0);
        assert_eq!(plugin_host_delay(true, true, false, 512, 64), 2048);
        assert_eq!(plugin_host_delay(false, true, true, 512, 64), 64);
        assert_eq!(plugin_host_delay(false, true, false, 0, 64), 64);
        assert_eq!(plugin_host_delay(false, false, false, 512, 64), 0);
    }
}

/// The engine with a playback thread that the test runs by hand after each block, so every
/// run is the same down to the sample.
#[cfg(test)]
mod engine_tests {
    use super::*;
    use crate::core::{Command, EngineCore, NoteEvent, Song, TrackMix};
    use crate::plugin_track::tests::{Fake, Gain};
    use crate::plugin_track::PluginTrack;
    use crate::routing::{AutoLane, AutoPoint, AutoTarget};
    use crate::tempo_map::{TempoCurve, TempoEvent, TempoMap};

    const IO: usize = 64;

    struct Rig {
        core: EngineCore,
        tx: Producer<Command>,
        /// Two playback threads, as on a machine with a few cores.
        workers: [Prerender; 2],
        shared: Arc<Shared>,
        /// Run the playback thread after each block.
        run_worker: bool,
        _garbage: Consumer<crate::core::Garbage>,
    }

    impl Rig {
        fn new() -> Self {
            let (tx, rx) = RingBuffer::new(256);
            let (gtx, grx) = RingBuffer::new(64);
            let shared = Arc::new(Shared::new());
            let mut core = EngineCore::new(48_000, rx, gtx, shared.clone());
            let (port, a, _trash) = channel(shared.clone());
            core.set_worker(port);
            let (port, b, _trash) = channel(shared.clone());
            core.set_worker(port);
            let mut tx = tx;
            // Unity at the Stereo Out, so levels read as the plug-ins make them.
            assert!(tx.push(Command::SetMasterGain { linear: 1.0 }).is_ok());
            Self {
                core,
                tx,
                workers: [a, b],
                shared,
                run_worker: true,
                _garbage: grx,
            }
        }

        fn send(&mut self, c: Command) {
            assert!(self.tx.push(c).is_ok());
        }

        /// `blocks` I/O blocks of [`IO`] frames, interleaved stereo.
        fn render(&mut self, blocks: usize) -> Vec<f32> {
            let mut out = Vec::with_capacity(blocks * IO * 2);
            let mut buf = vec![0f32; IO * 2];
            for _ in 0..blocks {
                self.core.set_device_time(IO as u32, 0, 0);
                let (core, workers, run) = (&mut self.core, &mut self.workers, self.run_worker);
                assert_no_alloc::assert_no_alloc(|| {
                    core.process(&mut buf);
                    if run {
                        for w in workers.iter_mut() {
                            w.step();
                        }
                    }
                });
                out.extend_from_slice(&buf);
            }
            out
        }

        /// Lanes on the playback threads.
        fn lanes(&self) -> usize {
            self.workers.iter().map(|w| w.lanes()).sum()
        }
    }

    fn note(track: u16, start: f64, end: f64) -> NoteEvent {
        NoteEvent {
            start_beat: start,
            end_beat: end,
            pitch: 60,
            vel: 100,
            track,
        }
    }

    fn left(buf: &[f32]) -> Vec<f32> {
        buf.as_chunks::<2>().0.iter().map(|f| f[0]).collect()
    }

    fn right(buf: &[f32]) -> Vec<f32> {
        buf.as_chunks::<2>().0.iter().map(|f| f[1]).collect()
    }

    fn onset(x: &[f32]) -> Option<usize> {
        x.iter().position(|v| v.abs() > 0.05)
    }

    fn instrument_track() -> Box<PluginTrack> {
        Box::new(PluginTrack::new(
            Some(Box::new(Fake::instrument())),
            Vec::new(),
            512,
        ))
    }

    /// Two tracks play the same note through the same instrument, panned apart: track 0 is
    /// pre-rendered (heard 512 frames after its input), track 1 live (64 frames). With delay
    /// compensation, or with the content delay a live track gets instead, both land on the
    /// same output sample.
    #[test]
    fn a_pre_rendered_and_a_live_track_land_on_the_same_sample() {
        for content in [false, true] {
            let mut r = Rig::new();
            let mixes = vec![
                TrackMix {
                    pan: -1.0,
                    ..TrackMix::default()
                },
                TrackMix {
                    pan: 1.0,
                    ..TrackMix::default()
                },
            ];
            let song = Song::new(vec![note(0, 1.0, 3.0), note(1, 1.0, 3.0)], mixes);
            r.send(Command::SetSong(Box::new(song)));
            r.send(Command::SetProcessBuffer { frames: 256 });
            r.send(Command::SetTrackLive {
                track: 1,
                live: true,
            });
            for t in 0..2 {
                r.send(Command::SetPlugins {
                    track: t,
                    plugins: Some(instrument_track()),
                });
            }
            let d = prerender_delay(256, IO as u32) as usize;
            assert_eq!(d, 512);
            // Track 0 waits 512 frames for its instrument; track 1 only 64.
            let wait = (d - IO) as u32;
            if content {
                r.send(Command::SetContentDelay {
                    track: 1,
                    frames: wait,
                });
            } else {
                r.send(Command::SetPdc {
                    track: 1,
                    pdc: Box::new(crate::fx::Pdc::new(wait as usize, 0)),
                });
            }
            r.send(Command::Play);
            let out = r.render(48_000 / IO);
            let (l, rr) = (left(&out), right(&out));
            let (Some(a), Some(b)) = (onset(&l), onset(&rr)) else {
                panic!("both tracks sound ({content})");
            };
            assert_eq!(a, b, "content delay {content}: pre-rendered {a}, live {b}");
            // The note is at beat 1 (24000 frames at 120 bpm), heard 512 frames later.
            assert_eq!(a, 24_000 + d, "content delay {content}");
        }
    }

    /// One track through an effect whose gain is automated, played twice: with every plug-in
    /// in the I/O callback, and pre-rendered. Through a tempo change, a cycle jump and a
    /// locate, the pre-rendered output is the other one 448 frames later, sample for sample
    /// (apart from the locate, where what was rendered ahead fades out and is not heard).
    #[test]
    fn pre_rendered_output_is_the_live_output_later_through_tempo_cycle_and_locate() {
        let render = |process: u32| -> Vec<f32> {
            let mut r = Rig::new();
            let notes = (0..24)
                .map(|k| note(0, 2.0 + k as f64 * 0.75, 2.5 + k as f64 * 0.75))
                .collect();
            r.send(Command::SetSong(Box::new(Song::new(
                notes,
                vec![TrackMix::default()],
            ))));
            r.send(Command::SetProcessBuffer { frames: process });
            // Without offsets the engine cuts the block at each change, so the result does
            // not depend on the block size.
            let fx = Gain::new(false, false);
            r.send(Command::SetPlugins {
                track: 0,
                plugins: Some(Box::new(PluginTrack::new(None, vec![Box::new(fx)], 512))),
            });
            let lane = AutoLane::new(
                0,
                AutoTarget::Plugin { slot: 1, param: 0 },
                vec![AutoPoint::new(2.0, 1.0), AutoPoint::new(12.0, 0.2)],
            )
            .sanitized();
            r.send(Command::SetAutomation(vec![lane]));
            r.send(Command::SetCycle {
                on: true,
                start_beat: 4.0,
                end_beat: 9.5,
            });
            r.send(Command::Play);
            let mut out = r.render(200);
            let map = TempoMap::new(
                vec![
                    TempoEvent {
                        beat: 0.0,
                        bpm: 120.0,
                        curve: TempoCurve::Step,
                    },
                    TempoEvent {
                        beat: 5.0,
                        bpm: 97.0,
                        curve: TempoCurve::Linear,
                    },
                    TempoEvent {
                        beat: 8.0,
                        bpm: 140.0,
                        curve: TempoCurve::Step,
                    },
                ],
                Vec::new(),
            );
            r.send(Command::SetTempoMap(Box::new(map)));
            out.extend(r.render(1300));
            r.send(Command::SetPosition { beat: 2.0 });
            out.extend(r.render(700));
            left(&out)
        };
        let live = render(0);
        let pre = render(256);
        let shift = 512 - IO;
        let locate = (200 + 1300) * IO;
        // Before the locate: everything after the move to the playback thread settled. The
        // last 448 frames before the locate were rendered ahead but are heard only after it,
        // so they are dropped.
        let from = 4096;
        let mut checked = 0;
        for n in from..locate - shift {
            assert_eq!(pre[n + shift], live[n], "frame {n}");
            checked += 1;
        }
        assert!(live[from..locate].iter().any(|x| x.abs() > 0.05));
        // At the locate the old position fades out within the fade, then nothing is heard
        // from it: the new position comes in after the pre-render delay.
        let fade = crate::plugin_track::FADE_FRAMES + IO;
        for (n, x) in pre[locate + fade..locate + shift].iter().enumerate() {
            assert_eq!(*x, 0.0, "stale frame {} after the locate", n + fade);
        }
        // The new position fades in over the fade, then it is in step again.
        let back = locate + IO;
        for n in back..back + crate::plugin_track::FADE_FRAMES {
            assert!(
                pre[n + shift].abs() <= live[n].abs() + 1e-6,
                "fading in at {n}"
            );
        }
        for n in back + crate::plugin_track::FADE_FRAMES..live.len() - shift {
            assert_eq!(pre[n + shift], live[n], "frame {n} after the locate");
            checked += 1;
        }
        assert!(checked > 100_000);
    }

    /// Moving a sounding track between the I/O callback and the playback thread fades it
    /// out and in: the output never jumps by more than one fade step.
    #[test]
    fn switching_a_track_between_live_and_pre_rendered_does_not_click() {
        let mut r = Rig::new();
        r.send(Command::SetSong(Box::new(Song::new(
            vec![note(0, 0.0, 100.0)],
            vec![TrackMix::default()],
        ))));
        r.send(Command::SetProcessBuffer { frames: 512 });
        r.send(Command::SetPlugins {
            track: 0,
            plugins: Some(instrument_track()),
        });
        r.send(Command::Play);
        let mut out = left(&r.render(100));
        for k in 0..8 {
            r.send(Command::SetTrackLive {
                track: 0,
                live: k % 2 == 0,
            });
            out.extend(left(&r.render(200)));
        }
        // The instrument holds 0.5 once the note started (after the first fade-in).
        let start = onset(&out).unwrap_or(0) + 4 * crate::plugin_track::FADE_FRAMES;
        let step = 0.5 / crate::plugin_track::FADE_FRAMES as f32;
        let worst = out[start..]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(
            worst <= step * 1.01,
            "largest step {worst}, fade step {step}"
        );
        // Each move comes back to full level.
        for k in 0..8 {
            let end = 100 * IO + (k + 1) * 200 * IO;
            assert!(
                (out[end - 1] - 0.5).abs() < 1e-6,
                "move {k}: {}",
                out[end - 1]
            );
        }
        // The last move (k = 7) made it pre-rendered.
        assert_eq!(r.lanes(), 1, "pre-rendered at the end");
    }

    /// The playback thread stops for a while: the I/O callback plays silence for the track,
    /// counts underruns and never waits; when it comes back the track is in time again.
    #[test]
    fn a_late_playback_thread_costs_silence_not_time() {
        let mut r = Rig::new();
        r.send(Command::SetSong(Box::new(Song::new(
            vec![note(0, 0.5, 100.0)],
            vec![TrackMix::default()],
        ))));
        r.send(Command::SetProcessBuffer { frames: 256 });
        r.send(Command::SetPlugins {
            track: 0,
            plugins: Some(instrument_track()),
        });
        r.send(Command::Play);
        let out = left(&r.render(800));
        assert!((out[out.len() - 1] - 0.5).abs() < 1e-6);
        assert_eq!(r.shared.prerender_underruns.load(Ordering::Relaxed), 0);
        r.run_worker = false;
        let stalled = left(&r.render(40));
        assert!(stalled[stalled.len() - IO..].iter().all(|x| *x == 0.0));
        assert!(r.shared.prerender_underruns.load(Ordering::Relaxed) > 0);
        r.run_worker = true;
        let back = left(&r.render(100));
        assert!((back[back.len() - 1] - 0.5).abs() < 1e-6);
    }

    /// Stalled long enough to fill its lane, the playback thread does not hand the track
    /// back to the I/O callback (that would overload the live tracks): the frames it missed
    /// are silent and the track stays in time. Two tracks share the two threads.
    #[test]
    fn a_full_lane_leaves_the_track_on_the_playback_thread_and_in_time() {
        let mut r = Rig::new();
        let mixes = vec![
            TrackMix {
                pan: -1.0,
                ..TrackMix::default()
            },
            TrackMix {
                pan: 1.0,
                ..TrackMix::default()
            },
        ];
        r.send(Command::SetSong(Box::new(Song::new(
            vec![note(0, 0.5, 100.0), note(1, 0.5, 100.0)],
            mixes,
        ))));
        r.send(Command::SetProcessBuffer { frames: 256 });
        for t in 0..2 {
            r.send(Command::SetPlugins {
                track: t,
                plugins: Some(instrument_track()),
            });
        }
        r.send(Command::Play);
        r.render(800);
        assert_eq!(r.lanes(), 2);
        assert!(r.workers.iter().all(|w| w.lanes() == 1), "one track each");
        let level = r.render(1)[0];
        r.run_worker = false;
        // 1500 blocks (2 s): more blocks than the lane has room for notes and transport, so
        // the frames that were not sent must go out as one gap.
        r.render(1500);
        r.run_worker = true;
        let back = r.render(400);
        assert_eq!(r.lanes(), 2, "still pre-rendered");
        let (l, rr) = (left(&back), right(&back));
        assert!(level > 0.5, "{level}");
        assert!((l[l.len() - 1] - level).abs() < 1e-6 && (rr[rr.len() - 1] - level).abs() < 1e-6);
        assert!(r.shared.prerender_underruns.load(Ordering::Relaxed) > 0);
        // Both tracks, on different threads, are back at full level for the whole last block.
        assert!(l[l.len() - 64..]
            .iter()
            .chain(&rr[rr.len() - 64..])
            .all(|x| (x - level).abs() < 1e-5));
    }

    /// A live track's synth notes from its regions wait for its content delay; the same note
    /// played live does not.
    #[test]
    fn a_live_tracks_regions_wait_for_its_content_delay_and_live_notes_do_not() {
        let mut r = Rig::new();
        r.send(Command::SetSong(Box::new(Song::new(
            vec![note(0, 0.5, 1.0)],
            vec![TrackMix::default()],
        ))));
        r.send(Command::SetContentDelay {
            track: 0,
            frames: 1000,
        });
        r.send(Command::Play);
        let out = left(&r.render(500));
        let Some(at) = onset(&out) else {
            panic!("it plays");
        };
        assert!((13_000..13_200).contains(&at), "{at}");
        let mut r = Rig::new();
        r.send(Command::SetSong(Box::new(Song::new(
            Vec::new(),
            vec![TrackMix::default()],
        ))));
        r.send(Command::SetContentDelay {
            track: 0,
            frames: 1000,
        });
        r.send(Command::NoteOn {
            pitch: 60,
            vel: 100,
            track: 0,
        });
        let out = left(&r.render(10));
        assert!(onset(&out).is_some_and(|n| n < 2 * IO), "{:?}", onset(&out));
    }

    /// Fader automation acts after the delay compensation, so it is read that many frames
    /// late; MIDI out from the timeline is stamped the whole output delay later.
    #[test]
    fn fader_automation_and_midi_out_follow_the_output_delay() {
        let quiet_from = |delay: u32| {
            let mut r = Rig::new();
            r.send(Command::SetSong(Box::new(Song::new(
                vec![note(0, 0.0, 100.0)],
                vec![TrackMix::default()],
            ))));
            let lane = AutoLane::new(
                0,
                AutoTarget::Volume,
                vec![AutoPoint::new(1.0, 0.0), AutoPoint::new(1.0001, -96.0)],
            )
            .sanitized();
            r.send(Command::SetAutomation(vec![lane]));
            r.send(Command::SetOutputDelay {
                strips: delay,
                master: delay,
            });
            r.send(Command::Play);
            let out = left(&r.render(600));
            (24_000..out.len() - 64)
                .find(|&n| out[n..n + 64].iter().all(|x| x.abs() < 0.01))
                .unwrap_or(0)
        };
        let (a, b) = (quiet_from(0), quiet_from(2048));
        assert!((b as i64 - a as i64 - 2048).abs() <= 64, "{a} -> {b}");

        let mut r = Rig::new();
        let Some(mut midi) = r.core.take_midi_out_reader() else {
            panic!("one reader");
        };
        r.send(Command::SetSong(Box::new(Song::new(
            vec![note(0, 0.0, 1.0)],
            vec![TrackMix::default()],
        ))));
        r.send(Command::SetTrackExternal {
            track: 0,
            external: true,
        });
        r.send(Command::SetOutputDelay {
            strips: 0,
            master: 4800,
        });
        r.send(Command::Play);
        r.core.set_device_time(IO as u32, 0, 1_000_000_000);
        let mut buf = vec![0f32; IO * 2];
        r.core.process(&mut buf);
        let Ok(on) = midi.pop() else {
            panic!("the note goes out");
        };
        assert!(on.on);
        // 4800 frames at 48 kHz: 100 ms later than its frame.
        assert_eq!(on.at_ns, 1_000_000_000 + 100_000_000);
    }

    /// The heaviest plug-in is reported with its track and slot, from either thread.
    #[test]
    fn the_heaviest_plug_in_is_reported() {
        let mut r = Rig::new();
        r.send(Command::SetProcessBuffer { frames: 256 });
        r.send(Command::SetPlugins {
            track: 7,
            plugins: Some(Box::new(PluginTrack::new(
                None,
                vec![Box::new(Fake::halver())],
                512,
            ))),
        });
        r.render(100);
        let Some((track, slot, load)) = r.shared.take_heaviest_plugin() else {
            panic!("reported");
        };
        assert_eq!((track, slot), (7, 1));
        assert!(load >= 0.0);
        assert_eq!(r.lanes(), 1, "pre-rendered");
    }

    /// Frames the master-bus test effect delays the mix by.
    const MASTER_DELAY: usize = 128;

    /// A master-bus effect that really delays the mix by [`MASTER_DELAY`] frames and says so.
    struct MasterDelay {
        line: [Vec<f32>; 2],
        pos: usize,
    }

    impl MasterDelay {
        fn new() -> Self {
            Self {
                line: [vec![0.0; MASTER_DELAY], vec![0.0; MASTER_DELAY]],
                pos: 0,
            }
        }
    }

    impl crate::plugin_track::BlockProcessor for MasterDelay {
        fn midi(&mut self, _status: u8, _d1: u8, _d2: u8, _offset: u32) {}
        fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
            for i in 0..out.0.len() {
                let p = self.pos;
                out.0[i] = self.line[0][p];
                out.1[i] = self.line[1][p];
                self.line[0][p] = input.0[i];
                self.line[1][p] = input.1[i];
                self.pos = (p + 1) % MASTER_DELAY;
            }
        }
        fn latency_seconds(&self) -> f64 {
            MASTER_DELAY as f64 / 48_000.0
        }
    }

    /// The Stereo Out's plug-ins together with Process Buffer Range: a pre-rendered track
    /// (left), a live track (right) and an external MIDI track play at beat 1, with a master
    /// plug-in that really delays the whole mix. The master chain runs in the I/O callback on
    /// the mix of both, so both clicks still land on the same output sample, later by the
    /// pre-render delay and by the master plug-in's delay; the external note's time stamp
    /// carries both delays, each once.
    #[test]
    fn a_master_plugin_and_a_pre_rendered_track_stay_in_time_and_midi_carries_both_delays() {
        let mut r = Rig::new();
        let (Some(mut midi), Some(mut state)) =
            (r.core.take_midi_out_reader(), r.core.take_state_reader())
        else {
            panic!("readers");
        };
        let mixes = vec![
            TrackMix {
                pan: -1.0,
                ..TrackMix::default()
            },
            TrackMix {
                pan: 1.0,
                ..TrackMix::default()
            },
            TrackMix::default(),
        ];
        let song = Song::new(
            vec![note(0, 1.0, 3.0), note(1, 1.0, 3.0), note(2, 1.0, 3.0)],
            mixes,
        );
        r.send(Command::SetSong(Box::new(song)));
        r.send(Command::SetTrackExternal {
            track: 2,
            external: true,
        });
        r.send(Command::SetProcessBuffer { frames: 256 });
        r.send(Command::SetTrackLive {
            track: 1,
            live: true,
        });
        for t in 0..2 {
            r.send(Command::SetPlugins {
                track: t,
                plugins: Some(instrument_track()),
            });
        }
        r.send(Command::SetMasterPlugins(Some(Box::new(PluginTrack::new(
            None,
            vec![Box::new(MasterDelay::new())],
            512,
        )))));
        let d = prerender_delay(256, IO as u32) as usize;
        assert_eq!(d, 512);
        // What the app sends for this mix: the live track's regions wait for the
        // pre-rendered one, and the whole mix is `d` frames late at the Stereo Out.
        r.send(Command::SetContentDelay {
            track: 1,
            frames: (d - IO) as u32,
        });
        r.send(Command::SetOutputDelay {
            strips: d as u32,
            master: d as u32,
        });
        r.send(Command::Play);
        // Host time runs with the frames: block `k` is heard at T0 + k * IO frames.
        const T0: u64 = 1_000_000_000;
        let ns = |frames: usize| (frames as f64 * 1e9 / 48_000.0) as u64;
        let mut out = Vec::new();
        let mut buf = vec![0f32; IO * 2];
        for k in 0..48_000 / IO {
            r.core.set_device_time(IO as u32, 0, T0 + ns(k * IO));
            let (core, workers) = (&mut r.core, &mut r.workers);
            assert_no_alloc::assert_no_alloc(|| {
                core.process(&mut buf);
                for w in workers.iter_mut() {
                    w.step();
                }
            });
            out.extend_from_slice(&buf);
        }
        let (l, rr) = (left(&out), right(&out));
        let (Some(a), Some(b)) = (onset(&l), onset(&rr)) else {
            panic!("both tracks sound");
        };
        assert_eq!(a, b, "pre-rendered {a}, live {b}");
        // Beat 1 is frame 24000 at 120 bpm.
        assert_eq!(a, 24_000 + d + MASTER_DELAY, "both delays, once each");
        let master_ns = (MASTER_DELAY as f64 / 48_000.0 * 1e9) as u64;
        assert_eq!(state.read().master_latency_us, (master_ns / 1000) as u32);
        let events: Vec<_> = std::iter::from_fn(|| midi.pop().ok()).collect();
        let Some(on) = events.iter().find(|e| e.track == 2 && e.on) else {
            panic!("the external note goes out");
        };
        // Its frame is heard at T0 + 24000 frames, after the mix delay and the master
        // plug-in's delay.
        let want = T0 + ns(24_000) + ns(d) + master_ns;
        assert!(
            on.at_ns.abs_diff(want) <= 2,
            "stamp {} want {want} (mix {} ns, master {master_ns} ns)",
            on.at_ns,
            ns(d)
        );
    }
}
