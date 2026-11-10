//! The audition voice: a loop (up to [`AUDITION_LAYERS`] layers, such as a drum loop's stems)
//! played round and round on the output, apart from the tracks and the transport, for a
//! browser to preview with.
//!
//! Every layer plays at the same place in the loop, each with its own gain, so a layer can be
//! muted or soloed while it plays without a gap: only its gain moves (over a few
//! milliseconds, so there is no click). A new loop can start where the last one was, so
//! browsing keeps time.
//!
//! The loop's place is a phase, 0 to 1. Free, it moves at the loop's own speed. Synced, the
//! loop lasts `beats` quarter notes of the project: while the transport plays the phase is the
//! playhead's place in that many beats (so bar 1 of the loop is on bar 1, 9, 17... of the
//! project), and while it is stopped the phase moves at the project's tempo. A loop not yet
//! stretched to that tempo is played faster or slower to fit (repitched); the browser swaps in
//! the stretched layers when they are ready, at the same phase.
//!
//! The window talks to it through a lock-free ring ([`AuditionRemote`]) and
//! the buffers it lets go of go back to be freed there: nothing is allocated or freed on the
//! audio thread.
use crate::sample_buf::SampleBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Layers a loop can have.
pub const AUDITION_LAYERS: usize = 8;
/// Messages the ring holds.
const RING: usize = 256;
/// Buffers waiting to be freed on the UI thread.
const TRASH: usize = 1024;
/// How long a gain change or the start and stop take, in seconds.
const FADE_SECONDS: f64 = 0.006;

/// The buffers of a loop's layers; `None` for an unused layer.
pub type AuditionLayers = [Option<Arc<SampleBuf>>; AUDITION_LAYERS];

/// From the window to the audio thread.
// The layers travel by value: boxed, the box would be freed on the audio thread.
#[allow(clippy::large_enum_variant)]
pub enum AuditionMsg {
    /// Plays `layers` with `gains` (linear), a loop of `beats` quarter notes when synced.
    /// `from_start`: from its beginning; otherwise at the phase the last loop had reached.
    Play {
        layers: AuditionLayers,
        gains: [f32; AUDITION_LAYERS],
        beats: f64,
        from_start: bool,
    },
    /// The same loop, other buffers (stretched to the tempo): the phase carries on.
    Swap { layers: AuditionLayers },
    /// New layer gains (linear), faded to.
    Gains([f32; AUDITION_LAYERS]),
    /// The level of the whole preview (linear, 0 to 2).
    Volume(f32),
    /// Synced to the project, or at the loop's own speed.
    Sync(bool),
    /// Fades out and lets go of the loop.
    Stop,
}

/// What the window can see of the audio thread.
#[derive(Default)]
pub struct AuditionStatus {
    /// The phase of the loop, 0 to 1 (`f32` bits).
    phase: AtomicU32,
    playing: AtomicBool,
    /// Messages taken from the ring so far.
    pub received: AtomicU64,
}

impl AuditionStatus {
    /// Where the loop is, 0 to 1, while one plays.
    pub fn phase(&self) -> Option<f32> {
        self.playing
            .load(Ordering::Relaxed)
            .then(|| f32::from_bits(self.phase.load(Ordering::Relaxed)))
    }
}

/// The window's end of the audition voice.
pub struct AuditionRemote {
    tx: rtrb::Producer<AuditionMsg>,
    trash: rtrb::Consumer<Arc<SampleBuf>>,
    pub status: Arc<AuditionStatus>,
}

impl AuditionRemote {
    /// Sends `m`; when the ring is full it comes back.
    #[allow(clippy::result_large_err)]
    pub fn send(&mut self, m: AuditionMsg) -> Result<(), AuditionMsg> {
        self.collect();
        self.tx.push(m).map_err(|rtrb::PushError::Full(m)| m)
    }

    /// Frees the buffers the audio thread is done with.
    pub fn collect(&mut self) {
        while self.trash.pop().is_ok() {}
    }
}

/// The voice on the audio thread.
pub struct Auditioner {
    sr: f64,
    rx: rtrb::Consumer<AuditionMsg>,
    trash: rtrb::Producer<Arc<SampleBuf>>,
    status: Arc<AuditionStatus>,
    layers: AuditionLayers,
    gain: [f32; AUDITION_LAYERS],
    gain_to: [f32; AUDITION_LAYERS],
    volume: f32,
    /// The fade of the whole voice (0 to 1) and where it is going.
    level: f32,
    level_to: f32,
    /// Per-frame smoothing coefficient of the fades.
    smooth: f32,
    phase: f64,
    beats: f64,
    sync: bool,
    received: u64,
}

impl Auditioner {
    /// A silent voice and the window's end of it. UI thread.
    pub fn new(sample_rate: u32) -> (Self, AuditionRemote) {
        let (tx, rx) = rtrb::RingBuffer::new(RING);
        let (trash_tx, trash_rx) = rtrb::RingBuffer::new(TRASH);
        let status = Arc::new(AuditionStatus::default());
        let sr = f64::from(sample_rate.max(1));
        (
            Self {
                sr,
                rx,
                trash: trash_tx,
                status: status.clone(),
                layers: Default::default(),
                gain: [0.0; AUDITION_LAYERS],
                gain_to: [0.0; AUDITION_LAYERS],
                volume: 1.0,
                level: 0.0,
                level_to: 0.0,
                smooth: (1.0 - (-1.0 / (FADE_SECONDS * sr)).exp()) as f32,
                phase: 0.0,
                beats: 32.0,
                sync: true,
                received: 0,
            },
            AuditionRemote {
                tx,
                trash: trash_rx,
                status,
            },
        )
    }

    /// Hands a buffer back to the UI thread to free (never freed here).
    fn throw(&mut self, buf: Arc<SampleBuf>) {
        if let Err(rtrb::PushError::Full(b)) = self.trash.push(buf) {
            // The window has not collected for a long time: keep the memory rather than free
            // it here.
            std::mem::forget(b);
        }
    }

    fn replace_layers(&mut self, layers: AuditionLayers) {
        let old = std::mem::replace(&mut self.layers, layers);
        for b in old.into_iter().flatten() {
            self.throw(b);
        }
    }

    fn release(&mut self) {
        self.replace_layers(Default::default());
    }

    /// Whether there is anything to play or fade out.
    fn active(&self) -> bool {
        self.level > 0.0 || self.level_to > 0.0
    }

    /// Takes the window's messages. Once per block, before its frames.
    pub fn begin_block(&mut self) {
        for _ in 0..64 {
            let Ok(m) = self.rx.pop() else { break };
            self.received += 1;
            match m {
                AuditionMsg::Play {
                    layers,
                    gains,
                    beats,
                    from_start,
                } => {
                    let silent = !self.active();
                    self.replace_layers(layers);
                    for (k, g) in gains.iter().enumerate() {
                        self.gain_to[k] = sane_gain(*g);
                        if silent {
                            self.gain[k] = self.gain_to[k];
                        }
                    }
                    if beats.is_finite() && beats > 0.0 {
                        self.beats = beats;
                    }
                    if from_start || silent {
                        self.phase = 0.0;
                    }
                    self.level_to = 1.0;
                }
                AuditionMsg::Swap { layers } => {
                    if self.active() {
                        self.replace_layers(layers);
                    } else {
                        for b in layers.into_iter().flatten() {
                            self.throw(b);
                        }
                    }
                }
                AuditionMsg::Gains(gains) => {
                    for (k, g) in gains.iter().enumerate() {
                        self.gain_to[k] = sane_gain(*g);
                    }
                }
                AuditionMsg::Volume(v) => self.volume = sane_gain(v),
                AuditionMsg::Sync(on) => self.sync = on,
                AuditionMsg::Stop => self.level_to = 0.0,
            }
        }
        self.status.received.store(self.received, Ordering::Relaxed);
    }

    /// The next frame (left, right). `transport_beat` is the playhead while the transport
    /// plays (NaN while it is stopped); `beats_per_frame` the project's tempo there.
    #[inline]
    pub fn frame(&mut self, transport_beat: f64, beats_per_frame: f64) -> (f32, f32) {
        if !self.active() {
            return (0.0, 0.0);
        }
        let k = self.smooth;
        self.level += (self.level_to - self.level) * k;
        if self.level_to == 0.0 && self.level < 1e-4 {
            self.level = 0.0;
            self.release();
            return (0.0, 0.0);
        }
        // Where the loop is this frame.
        if self.sync && transport_beat.is_finite() {
            self.phase = (transport_beat / self.beats).rem_euclid(1.0);
        }
        let (mut l, mut r) = (0f32, 0f32);
        let mut native_step = 0.0;
        for i in 0..AUDITION_LAYERS {
            self.gain[i] += (self.gain_to[i] - self.gain[i]) * k;
            let Some(buf) = self.layers[i].as_ref() else {
                continue;
            };
            let frames = buf.frames();
            if frames < 2 {
                continue;
            }
            if native_step == 0.0 {
                native_step = f64::from(buf.rate.max(1)) / self.sr / frames as f64;
            }
            let g = self.gain[i];
            if g < 1e-5 {
                continue;
            }
            let pos = self.phase * frames as f64;
            let a = (pos as usize).min(frames - 1);
            let b = if a + 1 >= frames { 0 } else { a + 1 };
            let t = (pos - a as f64) as f32;
            let d = &buf.data;
            l += (d[a * 2] + (d[b * 2] - d[a * 2]) * t) * g;
            r += (d[a * 2 + 1] + (d[b * 2 + 1] - d[a * 2 + 1]) * t) * g;
        }
        // On to the next frame, unless the playhead sets it.
        if !(self.sync && transport_beat.is_finite()) {
            let step = if self.sync && beats_per_frame.is_finite() && beats_per_frame > 0.0 {
                beats_per_frame / self.beats
            } else {
                native_step
            };
            self.phase += step;
            if self.phase >= 1.0 {
                self.phase -= self.phase.floor();
            }
        }
        let out = self.level * self.volume;
        (finite(l * out), finite(r * out))
    }

    /// Shows the phase to the window. Once per block, after its frames.
    pub fn end_block(&mut self) {
        let playing = self.active();
        self.status
            .phase
            .store((self.phase as f32).to_bits(), Ordering::Relaxed);
        self.status.playing.store(playing, Ordering::Relaxed);
    }
}

fn sane_gain(g: f32) -> f32 {
    if g.is_finite() {
        g.clamp(0.0, 2.0)
    } else {
        0.0
    }
}

fn finite(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// A stereo buffer whose left channel holds its frame number and right channel `-1`.
    fn ramp(frames: usize) -> Arc<SampleBuf> {
        let mut data = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            data.push(i as f32);
            data.push(-1.0);
        }
        Arc::new(SampleBuf::new(SR, data))
    }

    fn layers(bufs: &[Arc<SampleBuf>]) -> AuditionLayers {
        let mut l: AuditionLayers = Default::default();
        for (k, b) in bufs.iter().enumerate() {
            l[k] = Some(b.clone());
        }
        l
    }

    fn gains(g: &[f32]) -> [f32; AUDITION_LAYERS] {
        let mut out = [0.0; AUDITION_LAYERS];
        out[..g.len()].copy_from_slice(g);
        out
    }

    fn run(a: &mut Auditioner, frames: usize, beat: f64, bpf: f64) -> Vec<(f32, f32)> {
        a.begin_block();
        let out = (0..frames).map(|_| a.frame(beat, bpf)).collect();
        a.end_block();
        out
    }

    #[test]
    fn silent_until_told_and_cheap_while_silent() {
        let (mut a, remote) = Auditioner::new(SR);
        let out = run(&mut a, 64, f64::NAN, 0.0);
        assert!(out.iter().all(|&(l, r)| l == 0.0 && r == 0.0));
        assert_eq!(remote.status.phase(), None);
    }

    #[test]
    fn a_loop_plays_round_and_round_at_its_own_speed() {
        let (mut a, mut remote) = Auditioner::new(SR);
        remote.send(AuditionMsg::Sync(false)).ok().unwrap();
        let buf = ramp(100);
        remote
            .send(AuditionMsg::Play {
                layers: layers(&[buf]),
                gains: gains(&[1.0]),
                beats: 32.0,
                from_start: true,
            })
            .ok()
            .unwrap();
        // Long enough for the fade in to finish, then frame numbers count up and wrap.
        let out = run(&mut a, 2000, f64::NAN, 0.0);
        let left: Vec<f32> = out[1900..1905].iter().map(|x| x.0).collect();
        let want: Vec<f32> = (0..5).map(|i| ((1900 + i) % 100) as f32).collect();
        for (x, w) in left.iter().zip(&want) {
            assert!((x - w).abs() < 0.02, "{left:?} vs {want:?}");
        }
        assert!(remote.status.phase().is_some());
    }

    #[test]
    fn synced_while_playing_the_playhead_sets_the_place() {
        let (mut a, mut remote) = Auditioner::new(SR);
        let buf = ramp(3200);
        remote
            .send(AuditionMsg::Play {
                layers: layers(&[buf]),
                gains: gains(&[1.0]),
                beats: 32.0,
                from_start: true,
            })
            .ok()
            .unwrap();
        // Long enough for the fade in to finish.
        run(&mut a, 10_000, 0.0, 0.0);
        // Beat 8 of a 32-beat loop is a quarter of the way in; beat 40 the same.
        for beat in [8.0, 40.0] {
            let out = run(&mut a, 1, beat, 0.0);
            assert!((out[0].0 - 800.0).abs() < 0.5, "{}", out[0].0);
        }
    }

    #[test]
    fn muting_a_layer_fades_only_that_layer() {
        let (mut a, mut remote) = Auditioner::new(SR);
        remote.send(AuditionMsg::Sync(false)).ok().unwrap();
        let ones = Arc::new(SampleBuf::new(SR, vec![1.0; 2000]));
        let twos = Arc::new(SampleBuf::new(SR, vec![2.0; 2000]));
        remote
            .send(AuditionMsg::Play {
                layers: layers(&[ones, twos]),
                gains: gains(&[1.0, 1.0]),
                beats: 32.0,
                from_start: true,
            })
            .ok()
            .unwrap();
        let out = run(&mut a, 4000, f64::NAN, 0.0);
        assert!((out[3999].0 - 3.0).abs() < 1e-3);
        remote
            .send(AuditionMsg::Gains(gains(&[1.0, 0.0])))
            .ok()
            .unwrap();
        let out = run(&mut a, 4000, f64::NAN, 0.0);
        // No jump: the first frame after the change is still near 3.
        assert!(out[0].0 > 2.9, "{}", out[0].0);
        assert!((out[3999].0 - 1.0).abs() < 1e-3);
    }

    #[test]
    fn stop_fades_out_and_hands_the_buffers_back() {
        let (mut a, mut remote) = Auditioner::new(SR);
        let buf = Arc::new(SampleBuf::new(SR, vec![0.5; 2000]));
        remote
            .send(AuditionMsg::Play {
                layers: layers(std::slice::from_ref(&buf)),
                gains: gains(&[1.0]),
                beats: 32.0,
                from_start: true,
            })
            .ok()
            .unwrap();
        run(&mut a, 2000, f64::NAN, 0.0);
        remote.send(AuditionMsg::Stop).ok().unwrap();
        let out = run(&mut a, 48_000, f64::NAN, 0.0);
        assert_eq!(out[47_999], (0.0, 0.0));
        remote.collect();
        assert_eq!(Arc::strong_count(&buf), 1, "the voice let go of it");
        assert_eq!(remote.status.phase(), None);
    }

    #[test]
    fn the_audio_thread_does_not_allocate() {
        let (mut a, mut remote) = Auditioner::new(SR);
        let buf = ramp(4800);
        remote
            .send(AuditionMsg::Play {
                layers: layers(&[buf.clone(), buf.clone()]),
                gains: gains(&[1.0, 0.5]),
                beats: 32.0,
                from_start: true,
            })
            .ok()
            .unwrap();
        remote
            .send(AuditionMsg::Swap {
                layers: layers(&[buf]),
            })
            .ok()
            .unwrap();
        remote.send(AuditionMsg::Stop).ok().unwrap();
        assert_no_alloc::assert_no_alloc(|| {
            a.begin_block();
            for _ in 0..512 {
                a.frame(f64::NAN, 1e-4);
            }
            a.end_block();
        });
    }
}
