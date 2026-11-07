//! The Sampler instrument, modelled on the Boss Dr. Sample SP-303 (see `docs/SP303-NOTES.md`):
//! pads that each play a sample (a whole file, or a slice of one), with the pad's Gate, Loop
//! and level settings, through one effect; plus a preview voice for the file browser.
//!
//! It lives in a track's Instrument slot. Unlike the other built-in instruments it is not made
//! again when its settings change: the window sends [`SamplerMsg`]s through a lock-free ring
//! ([`SamplerRemote`]), and the audio thread hands the buffers it lets go of back through
//! another, so nothing is allocated or freed in [`Sampler::process`].
//!
//! Each pad's sound is prepared off the audio thread (cut, reversed, faded, normalised, and
//! time-stretched when the pad is in Stretch mode); here it is only played, repitched with a
//! windowed-sinc interpolator whose cut-off follows the playback speed, so playing a sample
//! higher does not fold its top end back down as aliasing.
use crate::plugin_track::BlockProcessor;
use crate::sp_fx::{FxSettings, SpFx};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Pads a sampler holds (banks of 8, as on the SP-303, and more banks for chops).
pub const SAMPLER_PADS: usize = 128;
/// Voices sounding at once; past that the oldest (a releasing one first) is taken.
pub const SAMPLER_VOICES: usize = 32;
/// No pad (in a [`KeyMap`]).
pub const NO_PAD: u8 = 255;
/// Messages the ring holds; more wait in the window until there is room.
const RING: usize = 1024;
/// Buffers waiting to be freed on the UI thread.
const TRASH: usize = 4096;
/// MIDI events one block holds.
const MAX_EVENTS: usize = 512;
/// The time a cut voice (choke, retrigger, steal) takes to fade, in seconds.
const CUT_SECONDS: f32 = 0.004;

/// A sound ready to play: interleaved stereo at its own rate.
#[derive(Debug, Default, PartialEq)]
pub struct SampleBuf {
    pub rate: u32,
    pub data: Vec<f32>,
}

impl SampleBuf {
    pub fn new(rate: u32, data: Vec<f32>) -> Self {
        Self { rate, data }
    }

    pub fn frames(&self) -> usize {
        self.data.len() / 2
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(self.rate.max(1))
    }
}

/// How one pad plays (the SP-303's per-pad Gate and Loop keys, and the rest an extension).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadParams {
    /// Linear gain.
    pub level: f32,
    /// -1 (left) to 1 (right).
    pub pan: f32,
    /// Semitones (with cents as the fraction) the pad's sound is played up or down, by speed.
    pub pitch: f32,
    /// Gate on: plays while held. Off: plays to the end however short the hit.
    pub gate: bool,
    /// Loops the sound. With Gate off, a second hit stops the loop (SP-303 manual p. 30).
    pub looping: bool,
    /// Pads in the same group (1 and up) cut each other off, as an open and a closed hi-hat.
    pub choke: u8,
    /// Envelope in seconds; `sustain` 0 to 1.
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    /// Low-pass cut-off in Hz (20 000 and up: open) and resonance 0 to 1.
    pub cutoff: f32,
    pub resonance: f32,
    /// 0: every hit at full level; 1: level follows velocity.
    pub vel_sens: f32,
    /// Goes through the effect (the SP-303's EFFECTS key works on the pads it is set for).
    pub fx: bool,
}

impl Default for PadParams {
    fn default() -> Self {
        Self {
            level: 1.0,
            pan: 0.0,
            pitch: 0.0,
            gate: false,
            looping: false,
            choke: 0,
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.01,
            cutoff: 20_000.0,
            resonance: 0.0,
            vel_sens: 1.0,
            fx: true,
        }
    }
}

/// What each MIDI note plays: a pad, and semitones on top of the pad's own pitch (the
/// Chromatic mode maps every note to one pad, each a semitone apart).
pub type KeyMap = [(u8, i8); 128];

/// The usual map: note `base + k` plays pad `k`.
pub fn pad_keys(base: u8, pads: usize) -> KeyMap {
    let mut map = [(NO_PAD, 0i8); 128];
    for (k, slot) in map.iter_mut().enumerate().skip(usize::from(base)) {
        let pad = k - usize::from(base);
        if pad < pads.min(SAMPLER_PADS) {
            *slot = (pad as u8, 0);
        }
    }
    map
}

/// Chromatic: every note plays `pad`, `root` at its own pitch.
pub fn chromatic_keys(pad: u8, root: u8) -> KeyMap {
    let mut map = [(NO_PAD, 0i8); 128];
    for (k, slot) in map.iter_mut().enumerate() {
        let semis = k as i32 - i32::from(root);
        if (-48..=48).contains(&semis) {
            *slot = (pad, semis as i8);
        }
    }
    map
}

/// From the window to the audio thread.
// The key map travels by value: boxed, the old one would be freed on the audio thread.
#[allow(clippy::large_enum_variant)]
pub enum SamplerMsg {
    Pad {
        pad: u8,
        params: PadParams,
    },
    /// A pad's sound (`None`: empty).
    Buffer {
        pad: u8,
        buf: Option<Arc<SampleBuf>>,
    },
    Keys(KeyMap),
    /// The browser's or the editor's preview: frames `from..to` of `buf`, at `rate` times its
    /// speed.
    Preview {
        buf: Arc<SampleBuf>,
        from: u64,
        to: u64,
        looping: bool,
        gain: f32,
    },
    StopPreview,
    Fx(FxSettings),
    /// A pad hit on screen (or its key), and let go.
    Hit {
        pad: u8,
        vel: u8,
    },
    Release {
        pad: u8,
    },
    /// Everything stops at once.
    AllOff,
}

/// What the window can see of the audio thread.
#[derive(Default)]
pub struct SamplerStatus {
    /// Where the preview is, in frames of its buffer (`u64::MAX`: not playing).
    pub preview_frame: AtomicU64,
    /// Pads sounding now, one bit each.
    pub playing: [AtomicU64; 2],
    pub voices: AtomicU32,
    /// Messages taken from the ring so far.
    pub received: AtomicU64,
}

impl SamplerStatus {
    pub fn is_playing(&self, pad: usize) -> bool {
        pad < SAMPLER_PADS
            && self.playing[pad / 64].load(Ordering::Relaxed) & (1 << (pad % 64)) != 0
    }

    pub fn preview_at(&self) -> Option<u64> {
        let f = self.preview_frame.load(Ordering::Relaxed);
        (f != u64::MAX).then_some(f)
    }
}

/// The window's end of a sampler: sends it messages and frees what it let go of.
pub struct SamplerRemote {
    tx: rtrb::Producer<SamplerMsg>,
    trash: rtrb::Consumer<Arc<SampleBuf>>,
    pub status: Arc<SamplerStatus>,
}

impl SamplerRemote {
    /// Sends `m`; when the ring is full it comes back.
    #[allow(clippy::result_large_err)]
    pub fn send(&mut self, m: SamplerMsg) -> Result<(), SamplerMsg> {
        self.collect();
        self.tx.push(m).map_err(|rtrb::PushError::Full(m)| m)
    }

    /// Room for this many more messages.
    pub fn room(&self) -> usize {
        self.tx.slots()
    }

    /// Frees the buffers the audio thread is done with.
    pub fn collect(&mut self) {
        while self.trash.pop().is_ok() {}
    }

    /// Whether the sampler at the other end is gone (its track was rebuilt).
    pub fn is_abandoned(&self) -> bool {
        self.tx.is_abandoned()
    }
}

// ------------------------------------------------------------------ interpolation

/// Phases of the sinc table between two samples.
const PHASES: usize = 256;
/// Speed bands: band 0 for speeds up to 1, then quarter octaves up to 4 (two octaves up).
const BANDS: usize = 9;

/// A windowed-sinc low-pass interpolator for one range of speeds.
struct Kernel {
    taps: usize,
    /// `(PHASES + 1) * taps` coefficients, phase by phase.
    table: Vec<f32>,
}

impl Kernel {
    /// `cutoff` as a fraction of the source's Nyquist frequency.
    fn new(cutoff: f64, taps: usize) -> Self {
        let half = (taps / 2) as f64;
        let mut table = Vec::with_capacity((PHASES + 1) * taps);
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            let start = table.len();
            for k in 0..taps {
                let x = (k as f64 - (half - 1.0)) - frac;
                let sinc = if x.abs() < 1e-9 {
                    1.0
                } else {
                    let a = std::f64::consts::PI * cutoff * x;
                    a.sin() / a
                };
                // Blackman-Harris window over the kernel's width.
                let t = (x / half).clamp(-1.0, 1.0);
                let w = 0.358_75
                    + 0.488_29 * (std::f64::consts::PI * t).cos()
                    + 0.141_28 * (2.0 * std::f64::consts::PI * t).cos()
                    + 0.011_68 * (3.0 * std::f64::consts::PI * t).cos();
                table.push((cutoff * sinc * w) as f32);
            }
            // Unity gain at DC for every phase.
            let sum: f32 = table[start..].iter().sum();
            if sum.abs() > 1e-6 {
                for c in &mut table[start..] {
                    *c /= sum;
                }
            }
        }
        Self { taps, table }
    }
}

/// The kernels of every speed band.
struct Interp {
    bands: Vec<Kernel>,
}

impl Interp {
    fn new() -> Self {
        let mut bands = Vec::with_capacity(BANDS);
        bands.push(Kernel::new(0.90, 16));
        for b in 1..BANDS {
            let top = 2f64.powf(b as f64 / 4.0);
            let taps = ((16.0 * top).ceil() as usize).div_ceil(4) * 4;
            bands.push(Kernel::new(0.90 / top, taps.min(64)));
        }
        Self { bands }
    }

    /// The band for a speed (source frames per output frame).
    fn band(step: f64) -> usize {
        if step <= 1.0 + 1e-9 {
            0
        } else {
            ((step.log2() * 4.0).ceil() as usize).clamp(1, BANDS - 1)
        }
    }
}

/// Frame `i` of `buf`, wrapped when `looping` (else silence outside it).
#[inline]
fn frame(data: &[f32], i: isize, looping: bool) -> (f32, f32) {
    let n = (data.len() / 2) as isize;
    if n == 0 {
        return (0.0, 0.0);
    }
    let j = if (0..n).contains(&i) {
        i
    } else if looping {
        i.rem_euclid(n)
    } else {
        return (0.0, 0.0);
    };
    let j = j as usize * 2;
    (data[j], data[j + 1])
}

/// The sound of `data` at fractional frame `pos`.
#[inline]
fn read(k: &Kernel, exact: bool, data: &[f32], pos: f64, looping: bool) -> (f32, f32) {
    let base = pos.floor();
    let frac = pos - base;
    let base = base as isize;
    // On a sample, at a speed of 1 or less, the sample itself is the answer; faster, the
    // low-pass is still needed.
    if exact && frac < 1e-7 {
        return frame(data, base, looping);
    }
    let idx = frac * PHASES as f64;
    let p0 = (idx as usize).min(PHASES - 1);
    let t = (idx - p0 as f64) as f32;
    let a = &k.table[p0 * k.taps..(p0 + 1) * k.taps];
    let b = &k.table[(p0 + 1) * k.taps..(p0 + 2) * k.taps];
    let first = base - (k.taps as isize / 2 - 1);
    let (mut l, mut r) = (0f32, 0f32);
    let n = (data.len() / 2) as isize;
    if first >= 0 && first + k.taps as isize <= n {
        // All inside: no wrapping checks.
        let s = &data[first as usize * 2..(first as usize + k.taps) * 2];
        for (j, (ca, cb)) in a.iter().zip(b).enumerate() {
            let c = ca + t * (cb - ca);
            l += s[j * 2] * c;
            r += s[j * 2 + 1] * c;
        }
    } else {
        for (j, (ca, cb)) in a.iter().zip(b).enumerate() {
            let c = ca + t * (cb - ca);
            let (fl, fr) = frame(data, first + j as isize, looping);
            l += fl * c;
            r += fr * c;
        }
    }
    (l, r)
}

// ------------------------------------------------------------------ voices

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Stage {
    #[default]
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Default)]
struct Env {
    stage: Stage,
    level: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
}

impl Env {
    fn start(&mut self, p: &PadParams, sr: f32) {
        let per = |secs: f32| 1.0 / (secs.max(0.0) * sr).max(1.0);
        self.attack = per(p.attack);
        self.decay = per(p.decay);
        self.sustain = p.sustain.clamp(0.0, 1.0);
        self.release = per(p.release.max(0.001));
        if p.attack <= 0.0 {
            self.level = 1.0;
            self.stage = Stage::Decay;
        } else {
            self.level = 0.0;
            self.stage = Stage::Attack;
        }
    }

    /// Lets go: falls to silence over `secs` from wherever it is.
    fn release_over(&mut self, secs: f32, sr: f32) {
        if self.stage != Stage::Off {
            self.release = self.level.max(1e-3) / (secs * sr).max(1.0);
            self.stage = Stage::Release;
        }
    }

    #[inline]
    fn next(&mut self) -> f32 {
        match self.stage {
            Stage::Off => {}
            Stage::Attack => {
                self.level += self.attack;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level -= self.decay * (1.0 - self.sustain);
                if self.level <= self.sustain {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {}
            Stage::Release => {
                self.level -= self.release;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Off;
                }
            }
        }
        self.level
    }
}

/// A state-variable low-pass filter (Simper's TPT form), stereo.
#[derive(Clone, Copy, Default)]
struct Svf {
    on: bool,
    g: f32,
    k: f32,
    s: [[f32; 2]; 2],
}

impl Svf {
    fn set(&mut self, cutoff: f32, resonance: f32, sr: f32) {
        self.on = cutoff < 19_999.0 && cutoff < sr * 0.49;
        let fc = cutoff.clamp(20.0, sr * 0.49);
        self.g = (std::f32::consts::PI * fc / sr).tan();
        self.k = 2.0 - 1.95 * resonance.clamp(0.0, 1.0);
        self.s = [[0.0; 2]; 2];
    }

    #[inline]
    fn run(&mut self, c: usize, x: f32) -> f32 {
        let [ic1, ic2] = &mut self.s[c];
        let a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        let v3 = x - *ic2;
        let v1 = a1 * *ic1 + self.g * a1 * v3;
        let v2 = *ic2 + self.g * v1;
        *ic1 = 2.0 * v1 - *ic1;
        *ic2 = 2.0 * v2 - *ic2;
        v2
    }
}

#[derive(Default)]
struct Voice {
    buf: Option<Arc<SampleBuf>>,
    pad: u8,
    pos: f64,
    step: f64,
    band: usize,
    end: f64,
    looping: bool,
    /// Gate: a Note Off lets it go. Otherwise it plays on.
    gated: bool,
    held: bool,
    gain_l: f32,
    gain_r: f32,
    env: Env,
    filter: Svf,
    choke: u8,
    fx: bool,
    age: u64,
}

impl Voice {
    fn active(&self) -> bool {
        self.buf.is_some()
    }
}

#[derive(Clone, Copy)]
struct Event {
    offset: u32,
    status: u8,
    data1: u8,
    data2: u8,
}

/// The instrument on the audio thread.
pub struct Sampler {
    sr: f32,
    rx: rtrb::Consumer<SamplerMsg>,
    trash: rtrb::Producer<Arc<SampleBuf>>,
    status: Arc<SamplerStatus>,
    pads: Vec<PadParams>,
    bufs: Vec<Option<Arc<SampleBuf>>>,
    keys: KeyMap,
    voices: Vec<Voice>,
    preview: Voice,
    preview_gain: f32,
    interp: Interp,
    events: Vec<Event>,
    started: u64,
    fx: SpFx,
    received: u64,
}

impl Sampler {
    /// A sampler with empty pads, and the window's end of it. UI thread.
    pub fn new(sample_rate: u32) -> (Self, SamplerRemote) {
        let (tx, rx) = rtrb::RingBuffer::new(RING);
        let (trash_tx, trash_rx) = rtrb::RingBuffer::new(TRASH);
        let status = Arc::new(SamplerStatus::default());
        status.preview_frame.store(u64::MAX, Ordering::Relaxed);
        let sr = sample_rate.max(1) as f32;
        (
            Self {
                sr,
                rx,
                trash: trash_tx,
                status: status.clone(),
                pads: vec![PadParams::default(); SAMPLER_PADS],
                bufs: vec![None; SAMPLER_PADS],
                keys: pad_keys(36, SAMPLER_PADS),
                voices: (0..SAMPLER_VOICES).map(|_| Voice::default()).collect(),
                preview: Voice::default(),
                preview_gain: 1.0,
                interp: Interp::new(),
                events: Vec::with_capacity(MAX_EVENTS),
                started: 0,
                fx: SpFx::new(sample_rate),
                received: 0,
            },
            SamplerRemote {
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
            // it on the audio thread.
            std::mem::forget(b);
        }
    }

    fn stop_voice(&mut self, v: usize) {
        if let Some(b) = self.voices[v].buf.take() {
            self.throw(b);
        }
    }

    fn take_messages(&mut self) {
        // A bounded number per block, so a flood cannot take the whole callback.
        for _ in 0..256 {
            let Ok(m) = self.rx.pop() else { break };
            self.received += 1;
            match m {
                SamplerMsg::Pad { pad, params } => {
                    if let Some(p) = self.pads.get_mut(usize::from(pad)) {
                        *p = params;
                    }
                }
                SamplerMsg::Buffer { pad, buf } => {
                    let pad = usize::from(pad);
                    if pad < SAMPLER_PADS {
                        let old = std::mem::replace(&mut self.bufs[pad], buf);
                        if let Some(old) = old {
                            self.throw(old);
                        }
                    }
                }
                SamplerMsg::Keys(map) => self.keys = map,
                SamplerMsg::Preview {
                    buf,
                    from,
                    to,
                    looping,
                    gain,
                } => {
                    self.preview_gain = gain;
                    let step = f64::from(buf.rate) / f64::from(self.sr);
                    let end = (to.min(buf.frames() as u64)) as f64;
                    let mut env = Env::default();
                    env.start(
                        &PadParams {
                            attack: 0.002,
                            ..PadParams::default()
                        },
                        self.sr,
                    );
                    let old = std::mem::replace(
                        &mut self.preview,
                        Voice {
                            pos: from as f64,
                            step,
                            band: Interp::band(step),
                            end,
                            looping,
                            gated: false,
                            held: true,
                            gain_l: 1.0,
                            gain_r: 1.0,
                            env,
                            buf: Some(buf),
                            ..Voice::default()
                        },
                    );
                    if let Some(b) = old.buf {
                        self.throw(b);
                    }
                }
                SamplerMsg::StopPreview => {
                    if let Some(b) = self.preview.buf.take() {
                        self.throw(b);
                    }
                }
                SamplerMsg::Fx(s) => self.fx.set(s),
                SamplerMsg::Hit { pad, vel } => self.note_on_pad(pad, 0, vel),
                SamplerMsg::Release { pad } => self.release_pad(pad),
                SamplerMsg::AllOff => {
                    for v in 0..self.voices.len() {
                        self.stop_voice(v);
                    }
                    if let Some(b) = self.preview.buf.take() {
                        self.throw(b);
                    }
                    self.fx.reset();
                }
            }
        }
        self.status.received.store(self.received, Ordering::Relaxed);
    }

    fn note_on(&mut self, note: u8, vel: u8) {
        let (pad, semis) = self.keys[usize::from(note & 0x7f)];
        if pad != NO_PAD {
            self.note_on_pad(pad, semis, vel);
        }
    }

    fn note_on_pad(&mut self, pad: u8, semis: i8, vel: u8) {
        let pi = usize::from(pad);
        let Some(p) = self.pads.get(pi).copied() else {
            return;
        };
        let Some(buf) = self.bufs[pi].clone() else {
            return;
        };
        // A loop without Gate is stopped by its next hit.
        if p.looping && !p.gate {
            let mut stopped = false;
            for v in self.voices.iter_mut() {
                if v.active() && v.pad == pad && v.env.stage != Stage::Release {
                    v.env.release_over(p.release.max(CUT_SECONDS), self.sr);
                    stopped = true;
                }
            }
            if stopped {
                drop_clone(buf, &mut self.trash);
                return;
            }
        }
        let sr = self.sr;
        // The pad's earlier sound (unless it is played chromatically), and its choke group.
        for v in self.voices.iter_mut().filter(|v| v.active()) {
            if (v.pad == pad && semis == 0) || (p.choke > 0 && v.choke == p.choke) {
                v.env.release_over(CUT_SECONDS, sr);
            }
        }
        self.started += 1;
        let slot = self.free_voice();
        if self.voices[slot].active() {
            self.stop_voice(slot);
        }
        let ratio = 2f64.powf((f64::from(p.pitch) + f64::from(semis)) / 12.0);
        let step = ratio * f64::from(buf.rate) / f64::from(sr);
        let v = f32::from(vel.min(127)) / 127.0;
        let gain = p.level * (1.0 - p.vel_sens.clamp(0.0, 1.0) + p.vel_sens.clamp(0.0, 1.0) * v);
        let a = (p.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        let mut env = Env::default();
        env.start(&p, sr);
        let mut filter = Svf::default();
        filter.set(p.cutoff, p.resonance, sr);
        let end = buf.frames() as f64;
        self.voices[slot] = Voice {
            buf: Some(buf),
            pad,
            pos: 0.0,
            step,
            band: Interp::band(step),
            end,
            looping: p.looping,
            gated: p.gate,
            held: true,
            gain_l: gain * a.cos() * std::f32::consts::SQRT_2,
            gain_r: gain * a.sin() * std::f32::consts::SQRT_2,
            env,
            filter,
            choke: p.choke,
            fx: p.fx,
            age: self.started,
        };
    }

    fn release_pad(&mut self, pad: u8) {
        let sr = self.sr;
        for v in self
            .voices
            .iter_mut()
            .filter(|v| v.active() && v.pad == pad)
        {
            v.held = false;
            if v.gated {
                let r = self.pads[usize::from(pad)].release;
                v.env.release_over(r.max(0.001), sr);
            }
        }
    }

    fn note_off(&mut self, note: u8) {
        let (pad, _) = self.keys[usize::from(note & 0x7f)];
        if pad != NO_PAD {
            self.release_pad(pad);
        }
    }

    /// A free voice, or the one to take: a releasing one first, then the oldest.
    fn free_voice(&self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.active()) {
            return i;
        }
        let mut best = 0;
        let mut best_key = (true, u64::MAX);
        for (i, v) in self.voices.iter().enumerate() {
            let key = (v.env.stage != Stage::Release, v.age);
            if key < best_key {
                best_key = key;
                best = i;
            }
        }
        best
    }

    #[inline]
    fn render_voice(interp: &Interp, v: &mut Voice) -> Option<(f32, f32)> {
        let buf = v.buf.as_ref()?;
        if !v.looping && v.pos >= v.end {
            return None;
        }
        let e = v.env.next();
        if v.env.stage == Stage::Off {
            return None;
        }
        let (l, r) = read(
            &interp.bands[v.band],
            v.band == 0,
            &buf.data,
            v.pos,
            v.looping,
        );
        v.pos += v.step;
        if v.looping && v.end > 0.0 && v.pos >= v.end {
            v.pos -= v.end;
        }
        let (mut l, mut r) = (l * e * v.gain_l, r * e * v.gain_r);
        if v.filter.on {
            l = v.filter.run(0, l);
            r = v.filter.run(1, r);
        }
        Some((l, r))
    }
}

/// Lets go of a clone without freeing on this thread: it goes back with the trash.
fn drop_clone(buf: Arc<SampleBuf>, trash: &mut rtrb::Producer<Arc<SampleBuf>>) {
    if let Err(rtrb::PushError::Full(b)) = trash.push(buf) {
        std::mem::forget(b);
    }
}

impl BlockProcessor for Sampler {
    fn midi(&mut self, status: u8, data1: u8, data2: u8, offset: u32) {
        let kind = status & 0xf0;
        if (kind == 0x90 || kind == 0x80 || (kind == 0xb0 && data1 >= 120))
            && self.events.len() < self.events.capacity()
        {
            self.events.push(Event {
                offset,
                status,
                data1,
                data2,
            });
        }
    }

    fn process(&mut self, _input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        let frames = out.0.len().min(out.1.len());
        self.take_messages();
        self.events.sort_unstable_by_key(|e| e.offset);
        let mut next = 0;
        for i in 0..frames {
            while next < self.events.len() && self.events[next].offset as usize <= i {
                let e = self.events[next];
                next += 1;
                match e.status & 0xf0 {
                    0x90 if e.data2 > 0 => self.note_on(e.data1, e.data2),
                    0x90 | 0x80 => self.note_off(e.data1),
                    // All Sound Off / All Notes Off.
                    _ => {
                        for v in self.voices.iter_mut().filter(|v| v.active()) {
                            v.env.release_over(CUT_SECONDS, self.sr);
                        }
                    }
                }
            }
            let (mut dl, mut dr, mut fl, mut fr) = (0f32, 0f32, 0f32, 0f32);
            for k in 0..self.voices.len() {
                match Self::render_voice(&self.interp, &mut self.voices[k]) {
                    Some((l, r)) => {
                        if self.voices[k].fx {
                            fl += l;
                            fr += r;
                        } else {
                            dl += l;
                            dr += r;
                        }
                    }
                    None => {
                        if self.voices[k].active() {
                            self.stop_voice(k);
                        }
                    }
                }
            }
            let (el, er) = self.fx.process(fl, fr);
            dl += el;
            dr += er;
            if self.preview.active() {
                match Self::render_voice(&self.interp, &mut self.preview) {
                    Some((l, r)) => {
                        dl += l * self.preview_gain;
                        dr += r * self.preview_gain;
                    }
                    None => {
                        if let Some(b) = self.preview.buf.take() {
                            self.throw(b);
                        }
                    }
                }
            }
            out.0[i] = dl;
            out.1[i] = dr;
        }
        // Events past the block's end happen at its end.
        while next < self.events.len() {
            let e = self.events[next];
            next += 1;
            if e.status & 0xf0 == 0x90 && e.data2 > 0 {
                self.note_on(e.data1, e.data2);
            } else if e.status & 0xf0 != 0xb0 {
                self.note_off(e.data1);
            }
        }
        self.events.clear();
        let mut bits = [0u64; 2];
        let mut n = 0;
        for v in self.voices.iter().filter(|v| v.active()) {
            bits[usize::from(v.pad) / 64] |= 1 << (v.pad % 64);
            n += 1;
        }
        self.status.playing[0].store(bits[0], Ordering::Relaxed);
        self.status.playing[1].store(bits[1], Ordering::Relaxed);
        self.status.voices.store(n, Ordering::Relaxed);
        self.status.preview_frame.store(
            if self.preview.active() {
                self.preview.pos as u64
            } else {
                u64::MAX
            },
            Ordering::Relaxed,
        );
    }

    fn latency_seconds(&self) -> f64 {
        0.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;

    const SR: u32 = 48_000;

    pub(crate) fn sine(hz: f64, frames: usize, rate: u32) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin() as f32 * 0.5;
            v.push(s);
            v.push(s);
        }
        v
    }

    /// The strongest frequency of a mono signal (FFT peak with parabolic refinement).
    pub(crate) fn peak_hz(mono: &[f32], rate: u32) -> f64 {
        let n = mono.len().next_power_of_two() / 2;
        let fft = crate::spectral::Fft::new(n);
        let mut re: Vec<f32> = (0..n)
            .map(|i| {
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
                mono[i] * w
            })
            .collect();
        let mut im = vec![0.0; n];
        fft.process(&mut re, &mut im, false);
        let mag: Vec<f64> = (0..n / 2).map(|i| f64::from(re[i].hypot(im[i]))).collect();
        let k = (1..n / 2 - 1)
            .max_by(|a, b| mag[*a].total_cmp(&mag[*b]))
            .unwrap();
        let (a, b, c) = (mag[k - 1].ln(), mag[k].ln(), mag[k + 1].ln());
        let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
        (k as f64 + shift) * f64::from(rate) / n as f64
    }

    fn run(s: &mut Sampler, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0f32; frames], vec![0f32; frames]);
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            assert_no_alloc::assert_no_alloc(|| s.process((&[], &[]), (cl, cr)));
        }
        (l, r)
    }

    fn dry() -> PadParams {
        PadParams {
            fx: false,
            release: 0.001,
            ..PadParams::default()
        }
    }

    fn sampler_with(bufs: &[Vec<f32>]) -> (Sampler, SamplerRemote) {
        let (mut s, mut remote) = Sampler::new(SR);
        for (k, b) in bufs.iter().enumerate() {
            remote
                .send(SamplerMsg::Pad {
                    pad: k as u8,
                    params: dry(),
                })
                .ok()
                .unwrap();
            remote
                .send(SamplerMsg::Buffer {
                    pad: k as u8,
                    buf: Some(Arc::new(SampleBuf::new(SR, b.clone()))),
                })
                .ok()
                .unwrap();
        }
        run(&mut s, 256);
        (s, remote)
    }

    fn sounding_len(l: &[f32]) -> usize {
        l.iter().rposition(|x| x.abs() > 1e-4).map_or(0, |i| i + 1)
    }

    #[test]
    fn repitch_up_an_octave_halves_the_length_and_doubles_the_frequency() {
        let frames = 48_000;
        let (mut s, mut remote) = sampler_with(&[sine(440.0, frames, SR)]);
        let mut p = dry();
        p.pitch = 12.0;
        remote
            .send(SamplerMsg::Pad { pad: 0, params: p })
            .ok()
            .unwrap();
        s.midi(0x90, 36, 127, 0);
        let (l, _) = run(&mut s, frames);
        let len = sounding_len(&l);
        assert!((len as i64 - 24_000).abs() < 40, "length {len}");
        let hz = peak_hz(&l[1000..len - 1000], SR);
        assert!((hz - 880.0).abs() < 1.0, "{hz} Hz");
        // And the same sound unshifted for comparison.
        p.pitch = 0.0;
        remote
            .send(SamplerMsg::Pad { pad: 0, params: p })
            .ok()
            .unwrap();
        s.midi(0x90, 36, 127, 0);
        let (l, _) = run(&mut s, frames + 512);
        assert!((sounding_len(&l) as i64 - 48_000).abs() < 40);
        assert!((peak_hz(&l[1000..47_000], SR) - 440.0).abs() < 1.0);
    }

    #[test]
    fn playing_high_does_not_alias() {
        // 15 kHz played an octave up would fold to 18 kHz without the band-limiting: the
        // output must be far quieter than the input.
        let frames = 24_000;
        let (mut s, mut remote) = sampler_with(&[sine(15_000.0, frames, SR)]);
        let mut p = dry();
        p.pitch = 12.0;
        remote
            .send(SamplerMsg::Pad { pad: 0, params: p })
            .ok()
            .unwrap();
        s.midi(0x90, 36, 127, 0);
        let (l, _) = run(&mut s, frames / 2);
        let rms = (l[500..11_000].iter().map(|x| x * x).sum::<f32>() / 10_500.0).sqrt();
        // The input's RMS is 0.35; -40 dB is 0.0035.
        assert!(rms < 0.0035, "aliased energy {rms}");
    }

    #[test]
    fn notes_map_to_pads_and_chromatic_maps_one_pad_across_the_keys() {
        let map = pad_keys(36, 16);
        assert_eq!(map[36], (0, 0));
        assert_eq!(map[51], (15, 0));
        assert_eq!(map[52].0, NO_PAD);
        assert_eq!(map[35].0, NO_PAD);
        let map = chromatic_keys(3, 60);
        assert_eq!(map[60], (3, 0));
        assert_eq!(map[72], (3, 12));
        assert_eq!(map[48], (3, -12));
        // In use: note 37 plays pad 1 (a constant 0.25), not pad 0 (0.5).
        let (mut s, _remote) = sampler_with(&[vec![0.5; 2000], vec![0.25; 2000]]);
        s.midi(0x90, 37, 127, 0);
        let (l, _) = run(&mut s, 256);
        assert!((l[10] - 0.25).abs() < 1e-4, "{}", l[10]);
    }

    #[test]
    fn a_choke_group_cuts_the_previous_voice() {
        let open = vec![0.5; 96_000];
        let closed = vec![0.25; 96_000];
        let (mut s, mut remote) = sampler_with(&[open, closed]);
        for pad in 0..2 {
            let mut p = dry();
            p.choke = 1;
            remote
                .send(SamplerMsg::Pad { pad, params: p })
                .ok()
                .unwrap();
        }
        s.midi(0x90, 36, 127, 0);
        run(&mut s, 1024);
        assert_eq!(s.status.voices.load(Ordering::Relaxed), 1);
        s.midi(0x90, 37, 127, 0);
        let (l, _) = run(&mut s, 1024);
        // Within 4 ms (192 frames) only the closed hat sounds.
        assert!((l[400] - 0.25).abs() < 1e-4, "{}", l[400]);
        assert_eq!(s.status.voices.load(Ordering::Relaxed), 1);
        assert!(!s.status.is_playing(0) && s.status.is_playing(1));
    }

    #[test]
    fn gate_stops_on_release_and_one_shot_plays_on() {
        let (mut s, mut remote) = sampler_with(&[vec![0.5; 48_000], vec![0.5; 48_000]]);
        let mut gated = dry();
        gated.gate = true;
        remote
            .send(SamplerMsg::Pad {
                pad: 0,
                params: gated,
            })
            .ok()
            .unwrap();
        s.midi(0x90, 36, 127, 0);
        s.midi(0x90, 37, 127, 0);
        run(&mut s, 512);
        s.midi(0x80, 36, 0, 0);
        s.midi(0x80, 37, 0, 0);
        run(&mut s, 2048);
        assert!(!s.status.is_playing(0), "gated pad stopped");
        assert!(s.status.is_playing(1), "one-shot pad plays on");
    }

    #[test]
    fn a_loop_without_gate_runs_until_hit_again() {
        let (mut s, mut remote) = sampler_with(&[vec![0.5; 1000]]);
        let mut p = dry();
        p.looping = true;
        remote
            .send(SamplerMsg::Pad { pad: 0, params: p })
            .ok()
            .unwrap();
        s.midi(0x90, 36, 127, 0);
        s.midi(0x80, 36, 0, 10);
        let (l, _) = run(&mut s, 10_000);
        assert!(l[9_000].abs() > 0.4, "still looping");
        s.midi(0x90, 36, 127, 0);
        run(&mut s, 2048);
        assert!(!s.status.is_playing(0));
    }

    #[test]
    fn thirty_two_voices_then_the_oldest_is_taken() {
        let bufs: Vec<Vec<f32>> = (0..40).map(|_| vec![0.01; 96_000]).collect();
        let (mut s, _remote) = sampler_with(&bufs);
        for k in 0..40 {
            s.midi(0x90, 36 + k, 100, 0);
            run(&mut s, 256);
        }
        assert_eq!(s.status.voices.load(Ordering::Relaxed), 32);
        assert!(!s.status.is_playing(0), "the first was taken");
        assert!(s.status.is_playing(39));
    }

    #[test]
    fn velocity_sensitivity_and_level() {
        let (mut s, mut remote) = sampler_with(&[vec![1.0; 4000]]);
        let mut p = dry();
        p.vel_sens = 1.0;
        p.level = 0.5;
        remote
            .send(SamplerMsg::Pad { pad: 0, params: p })
            .ok()
            .unwrap();
        run(&mut s, 64);
        s.midi(0x90, 36, 64, 0);
        let (l, _) = run(&mut s, 256);
        assert!((l[10] - 0.5 * 64.0 / 127.0).abs() < 1e-3, "{}", l[10]);
    }

    #[test]
    fn the_preview_plays_from_where_it_is_asked_and_reports_its_place() {
        let mut data = vec![0.0f32; 20_000];
        for (i, x) in data.iter_mut().enumerate() {
            *x = (i / 2) as f32 / 10_000.0;
        }
        let buf = Arc::new(SampleBuf::new(SR, data));
        let (mut s, mut remote) = Sampler::new(SR);
        remote
            .send(SamplerMsg::Preview {
                buf,
                from: 5_000,
                to: 10_000,
                looping: false,
                gain: 1.0,
            })
            .ok()
            .unwrap();
        let (l, _) = run(&mut s, 256);
        assert!((l[200] - 0.52).abs() < 0.01, "{}", l[200]);
        let at = s.status.preview_at().unwrap();
        assert!((5_200..5_300).contains(&at), "{at}");
        run(&mut s, 6000);
        assert_eq!(s.status.preview_at(), None, "stopped at its end");
    }

    #[test]
    fn replaced_buffers_are_freed_by_the_window_not_the_audio_thread() {
        let (mut s, mut remote) = sampler_with(&[vec![0.5; 4000]]);
        let first = Arc::new(SampleBuf::new(SR, vec![0.1; 100]));
        let weak = Arc::downgrade(&first);
        remote
            .send(SamplerMsg::Buffer {
                pad: 0,
                buf: Some(first),
            })
            .ok()
            .unwrap();
        run(&mut s, 64);
        s.midi(0x90, 36, 127, 0);
        run(&mut s, 64);
        remote
            .send(SamplerMsg::Buffer { pad: 0, buf: None })
            .ok()
            .unwrap();
        run(&mut s, 512); // the voice ends (the buffer is 50 frames) and the pad is emptied
        assert!(weak.upgrade().is_some(), "still held, in the trash ring");
        remote.collect();
        assert!(weak.upgrade().is_none(), "freed by collect()");
    }

    #[test]
    fn silence_in_silence_out_and_no_nan() {
        let (mut s, _r) = Sampler::new(SR);
        s.midi(0x90, 36, 127, 0);
        let (l, r) = run(&mut s, 4096);
        assert!(l.iter().chain(&r).all(|x| *x == 0.0));
    }
}
