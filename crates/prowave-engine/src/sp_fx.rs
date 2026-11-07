//! The effects of the Sampler, after the SP-303's 26 (SP-303 Owner's Manual pp. 21–27; see
//! `docs/SP303-NOTES.md` §7–9): five on their own keys (Filter+Drive, Pitch, Delay, Vinyl Sim,
//! Isolator) and the MFX list. One effect is on at a time, for the pads that are set to it
//! (manual p. 22); its three controls are the CTRL 1–3 knobs, each 0 to 1 here.
//!
//! The manual names each effect and what its knobs do, but gives no numbers and nothing about
//! how they work inside. The ranges and the algorithms here are ours, chosen to behave as the
//! manual describes; where an effect is famous for a sound (Vinyl Sim's pumping compression,
//! the noise, the wow) it is modelled on what producers describe, not on a measurement.
//! Two MFX are not made: 17 SLICER (its 16 timing patterns are a figure missing from the
//! manual's PDF) and 20 VOICE TRANS. (a formant shifter); they cannot be picked.
//!
//! Every buffer is made in [`SpFx::new`]; switching effects only clears state, so nothing is
//! allocated on the audio thread.
use std::f32::consts::{PI, TAU};

/// The effects, in the SP-303's order: the five keys, then MFX 1–21.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpFxKind {
    #[default]
    FilterDrive,
    Pitch,
    Delay,
    VinylSim,
    Isolator,
    Reverb,
    TapeEcho,
    Chorus,
    Flanger,
    Phaser,
    TremoloPan,
    Distortion,
    Overdrive,
    Fuzz,
    Wah,
    Octave,
    Comp,
    Equalizer,
    LoFi,
    NoiseGen,
    RadioTuning,
    Slicer,
    RingMod,
    ChromaticPs,
    VoiceTrans,
    CCanceler,
}

impl SpFxKind {
    pub const ALL: [SpFxKind; 26] = [
        SpFxKind::FilterDrive,
        SpFxKind::Pitch,
        SpFxKind::Delay,
        SpFxKind::VinylSim,
        SpFxKind::Isolator,
        SpFxKind::Reverb,
        SpFxKind::TapeEcho,
        SpFxKind::Chorus,
        SpFxKind::Flanger,
        SpFxKind::Phaser,
        SpFxKind::TremoloPan,
        SpFxKind::Distortion,
        SpFxKind::Overdrive,
        SpFxKind::Fuzz,
        SpFxKind::Wah,
        SpFxKind::Octave,
        SpFxKind::Comp,
        SpFxKind::Equalizer,
        SpFxKind::LoFi,
        SpFxKind::NoiseGen,
        SpFxKind::RadioTuning,
        SpFxKind::Slicer,
        SpFxKind::RingMod,
        SpFxKind::ChromaticPs,
        SpFxKind::VoiceTrans,
        SpFxKind::CCanceler,
    ];

    /// The panel's name.
    pub fn name(self) -> &'static str {
        match self {
            SpFxKind::FilterDrive => "FILTER+DRIVE",
            SpFxKind::Pitch => "PITCH",
            SpFxKind::Delay => "DELAY",
            SpFxKind::VinylSim => "VINYL SIM",
            SpFxKind::Isolator => "ISOLATOR",
            SpFxKind::Reverb => "REVERB",
            SpFxKind::TapeEcho => "TAPE ECHO",
            SpFxKind::Chorus => "CHORUS",
            SpFxKind::Flanger => "FLANGER",
            SpFxKind::Phaser => "PHASER",
            SpFxKind::TremoloPan => "TREMOLO/PAN",
            SpFxKind::Distortion => "DISTORTION",
            SpFxKind::Overdrive => "OVERDRIVE",
            SpFxKind::Fuzz => "FUZZ",
            SpFxKind::Wah => "WAH",
            SpFxKind::Octave => "OCTAVE",
            SpFxKind::Comp => "COMP",
            SpFxKind::Equalizer => "EQUALIZER",
            SpFxKind::LoFi => "LO-FI",
            SpFxKind::NoiseGen => "NOISE GEN",
            SpFxKind::RadioTuning => "RADIO TUNING",
            SpFxKind::Slicer => "SLICER",
            SpFxKind::RingMod => "RING MOD",
            SpFxKind::ChromaticPs => "CHROMATIC PS",
            SpFxKind::VoiceTrans => "VOICE TRANS.",
            SpFxKind::CCanceler => "C.CANCELER",
        }
    }

    /// The MFX number (1–21), or `None` for the five on their own keys.
    pub fn mfx_number(self) -> Option<usize> {
        let i = Self::ALL.iter().position(|k| *k == self)?;
        (i >= 5).then(|| i - 4)
    }

    pub fn from_mfx(n: usize) -> Option<Self> {
        (1..=21).contains(&n).then(|| Self::ALL[n + 4])
    }

    /// Whether Pro Wave makes this effect (all but SLICER and VOICE TRANS.).
    pub fn available(self) -> bool {
        !matches!(self, SpFxKind::Slicer | SpFxKind::VoiceTrans)
    }

    /// The display codes of CTRL 1–3 (manual pp. 24–27).
    pub fn codes(self) -> [&'static str; 3] {
        match self {
            SpFxKind::FilterDrive => ["CoF", "rES", "drV"],
            SpFxKind::Pitch => ["Pit", "Fdb", "dAL"],
            SpFxKind::Delay => ["t--", "Fdb", "LEV"],
            SpFxKind::VinylSim => ["CMP", "noS", "FLu"],
            SpFxKind::Isolator => ["Lo", "Mid", "Hi"],
            SpFxKind::Reverb => ["tiM", "ton", "LEV"],
            SpFxKind::TapeEcho => ["rAt", "int", "LEV"],
            SpFxKind::Chorus => ["dPt", "rAt", "LEV"],
            SpFxKind::Flanger | SpFxKind::Phaser => ["dPt", "rAt", "rES"],
            SpFxKind::TremoloPan => ["dPt", "rAt", "trM"],
            SpFxKind::Distortion | SpFxKind::Overdrive | SpFxKind::Fuzz => ["drV", "ton", "LEV"],
            SpFxKind::Wah => ["SnS", "Frq", "rES"],
            SpFxKind::Octave => ["oC2", "oC1", "dir"],
            SpFxKind::Comp => ["SuS", "Att", "LEV"],
            SpFxKind::Equalizer => ["Lo", "Mid", "Hi"],
            SpFxKind::LoFi => ["rAt", "bit", "FLt"],
            SpFxKind::NoiseGen => ["CoL", "qLt", "LEV"],
            SpFxKind::RadioTuning => ["tun", "noS", "Frq"],
            SpFxKind::Slicer => ["P01", "rAt", "Fdb"],
            SpFxKind::RingMod => ["Frq", "EFF", "dir"],
            SpFxKind::ChromaticPs => ["-C", "-C", "bAL"],
            SpFxKind::VoiceTrans => ["For", "EFF", "dir"],
            SpFxKind::CCanceler => ["bAL", "Lo", "Hi"],
        }
    }

    /// What CTRL 1–3 do, in the manual's words.
    pub fn knobs(self) -> [&'static str; 3] {
        match self {
            SpFxKind::FilterDrive => ["CUTOFF", "RESONANCE", "DRIVE"],
            SpFxKind::Pitch => ["PITCH", "FEEDBACK", "BALANCE"],
            SpFxKind::Delay => ["TIME", "FEEDBACK", "E.LEVEL"],
            SpFxKind::VinylSim => ["COMPRESSOR", "NOISE LEVEL", "WOW FLUTTER"],
            SpFxKind::Isolator => ["LOW", "MID", "HIGH"],
            SpFxKind::Reverb => ["REVERB TIME", "TONE", "E.LEVEL"],
            SpFxKind::TapeEcho => ["REPEAT RATE", "INTENSITY", "E.LEVEL"],
            SpFxKind::Chorus => ["DEPTH", "RATE", "E.LEVEL"],
            SpFxKind::Flanger | SpFxKind::Phaser => ["DEPTH/MANUAL", "RATE", "RESONANCE"],
            SpFxKind::TremoloPan => ["DEPTH", "RATE", "WAVEFORM"],
            SpFxKind::Distortion | SpFxKind::Overdrive | SpFxKind::Fuzz => {
                ["DRIVE", "TONE", "LEVEL"]
            }
            SpFxKind::Wah => ["SENS", "FREQUENCY", "RESONANCE"],
            SpFxKind::Octave => ["-2 OCT LEVEL", "-1 OCT LEVEL", "DIRECT LEVEL"],
            SpFxKind::Comp => ["SUSTAIN", "ATTACK", "LEVEL"],
            SpFxKind::Equalizer => ["LOW", "MID", "HIGH"],
            SpFxKind::LoFi => ["SAMPLE RATE", "BIT", "FILTER"],
            SpFxKind::NoiseGen => ["COLOR", "QUALITY", "LEVEL"],
            SpFxKind::RadioTuning => ["TUNING", "NOISE LEVEL", "FREQ RANGE"],
            SpFxKind::Slicer => ["TIMING PTN", "RATE", "FEEDBACK"],
            SpFxKind::RingMod => ["FREQUENCY", "EFFECT LEVEL", "DIRECT LEVEL"],
            SpFxKind::ChromaticPs => ["PITCH 1", "PITCH 2", "BALANCE"],
            SpFxKind::VoiceTrans => ["FORMANT", "EFFECT LEVEL", "DIRECT LEVEL"],
            SpFxKind::CCanceler => ["L-R BALANCE", "LOW BOOST", "HIGH BOOST"],
        }
    }

    /// The knob positions a newly picked effect starts at.
    pub fn defaults(self) -> [f32; 3] {
        match self {
            SpFxKind::FilterDrive => [0.6, 0.3, 0.2],
            SpFxKind::Pitch => [0.75, 0.0, 1.0],
            SpFxKind::Delay => [0.5, 0.35, 0.4],
            SpFxKind::VinylSim => [0.6, 0.3, 0.2],
            SpFxKind::Isolator | SpFxKind::Equalizer => [0.5, 0.5, 0.5],
            SpFxKind::CCanceler => [0.5, 0.3, 0.3],
            SpFxKind::Reverb => [0.5, 0.5, 0.35],
            SpFxKind::TremoloPan => [0.6, 0.4, 0.2],
            SpFxKind::Octave => [0.0, 0.6, 0.8],
            SpFxKind::Comp => [0.5, 0.3, 0.6],
            SpFxKind::LoFi => [0.4, 0.5, 0.7],
            SpFxKind::RadioTuning => [0.5, 0.3, 0.4],
            SpFxKind::RingMod => [0.4, 0.8, 0.0],
            SpFxKind::ChromaticPs => [0.5 + 7.0 / 24.0, 0.5 + 4.0 / 24.0, 0.6],
            _ => [0.5, 0.4, 0.5],
        }
    }
}

/// The DELAY key's TIME values (manual p. 24): note lengths in quarter notes.
pub const DELAY_NOTES: [(&str, f32); 13] = [
    ("t32", 0.125),
    ("t16", 0.25),
    ("t16.", 0.375),
    ("t8.3", 1.0 / 3.0),
    ("t8", 0.5),
    ("t8.", 0.75),
    ("t4.3", 2.0 / 3.0),
    ("t4", 1.0),
    ("t4.", 1.5),
    ("t2.3", 4.0 / 3.0),
    ("t2", 2.0),
    ("t2.", 3.0),
    ("t1", 4.0),
];
/// Longest delay the DELAY key gives (manual p. 24: "about 1450 ms").
pub const MAX_DELAY_SECONDS: f32 = 1.45;

/// The DELAY note a CTRL 1 position picks: the knob runs through [`DELAY_NOTES`] in the
/// manual's order.
pub fn delay_note(ctrl1: f32) -> usize {
    ((ctrl1.clamp(0.0, 1.0) * DELAY_NOTES.len() as f32) as usize).min(DELAY_NOTES.len() - 1)
}

/// The delay in seconds for a note at `bpm`, shortened by octaves past 1450 ms.
pub fn delay_seconds(ctrl1: f32, bpm: f32) -> f32 {
    let beats = DELAY_NOTES[delay_note(ctrl1)].1;
    let mut s = beats * 60.0 / bpm.clamp(40.0, 200.0);
    while s > MAX_DELAY_SECONDS {
        s *= 0.5;
    }
    s
}

/// The chromatic pitch of a CHROMATIC PS knob: −12 to +12 semitones in steps.
pub fn chromatic_semis(ctrl: f32) -> f32 {
    ((ctrl.clamp(0.0, 1.0) - 0.5) * 24.0).round()
}

/// The effect as the window sets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxSettings {
    pub kind: SpFxKind,
    pub on: bool,
    pub ctrl: [f32; 3],
    /// For DELAY's note values: the tempo it follows (the project's, or the sample's).
    pub bpm: f32,
}

impl Default for FxSettings {
    fn default() -> Self {
        Self {
            kind: SpFxKind::FilterDrive,
            on: false,
            ctrl: SpFxKind::FilterDrive.defaults(),
            bpm: 120.0,
        }
    }
}

// ------------------------------------------------------------------ building blocks

/// A delay line with fractional reads.
struct Line {
    buf: Vec<f32>,
    w: usize,
}

impl Line {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(4)],
            w: 0,
        }
    }
    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
    #[inline]
    fn push(&mut self, x: f32) {
        self.w = (self.w + 1) % self.buf.len();
        self.buf[self.w] = x;
    }
    /// The sample `d` frames ago (fractional, linear).
    #[inline]
    fn tap(&self, d: f32) -> f32 {
        let n = self.buf.len();
        let d = d.clamp(0.0, (n - 2) as f32);
        let i = d as usize;
        let f = d - i as f32;
        let a = self.buf[(self.w + n - i) % n];
        let b = self.buf[(self.w + n - i - 1) % n];
        a + (b - a) * f
    }
}

/// Simper's state-variable filter, one channel: low, band and high outputs.
#[derive(Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    #[inline]
    fn run(&mut self, x: f32, fc: f32, q_k: f32, sr: f32) -> (f32, f32, f32) {
        let g = (PI * fc.clamp(10.0, sr * 0.45) / sr).tan();
        let a1 = 1.0 / (1.0 + g * (g + q_k));
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + g * a1 * v3;
        let v2 = self.ic2 + g * v1;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        let hp = x - q_k * v1 - v2;
        (v2, v1, hp)
    }
}

/// A one-pole low-pass.
#[derive(Clone, Copy, Default)]
struct OnePole {
    y: f32,
}

impl OnePole {
    #[inline]
    fn lp(&mut self, x: f32, fc: f32, sr: f32) -> f32 {
        let a = 1.0 - (-TAU * fc / sr).exp();
        self.y += a * (x - self.y);
        self.y
    }
}

/// An RBJ biquad (shelves and peak for the EQUALIZER).
#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn set(&mut self, kind: u8, fc: f32, db: f32, q: f32, sr: f32) {
        let a = 10f32.powf(db / 40.0);
        let w = TAU * fc / sr;
        let (s, c) = (w.sin(), w.cos());
        let alpha = s / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            // Low shelf.
            0 => {
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * c + sq),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                    a * ((a + 1.0) - (a - 1.0) * c - sq),
                    (a + 1.0) + (a - 1.0) * c + sq,
                    -2.0 * ((a - 1.0) + (a + 1.0) * c),
                    (a + 1.0) + (a - 1.0) * c - sq,
                )
            }
            // High shelf.
            2 => {
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * c + sq),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                    a * ((a + 1.0) + (a - 1.0) * c - sq),
                    (a + 1.0) - (a - 1.0) * c + sq,
                    2.0 * ((a - 1.0) - (a + 1.0) * c),
                    (a + 1.0) - (a - 1.0) * c - sq,
                )
            }
            // Peak.
            _ => (
                1.0 + alpha * a,
                -2.0 * c,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * c,
                1.0 - alpha / a,
            ),
        };
        self.b = [b0 / a0, b1 / a0, b2 / a0];
        self.a = [a1 / a0, a2 / a0];
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// A delay-line pitch shifter: two read heads sweep through a 60 ms window at the speed
/// difference, crossfaded with a raised cosine so each restart is hidden.
struct Shifter {
    line: Line,
    phase: f32,
    window: f32,
}

impl Shifter {
    fn new(sr: f32) -> Self {
        let window = (0.06 * sr).max(64.0);
        Self {
            line: Line::new(window as usize + 8),
            phase: 0.0,
            window,
        }
    }
    fn clear(&mut self) {
        self.line.clear();
        self.phase = 0.0;
    }
    #[inline]
    fn run(&mut self, x: f32, ratio: f32) -> f32 {
        self.line.push(x);
        // The read head moves at `ratio` while the write head moves at 1: the delay changes
        // by (1 - ratio) a frame.
        self.phase = (self.phase + (1.0 - ratio) / self.window).rem_euclid(1.0);
        let p2 = (self.phase + 0.5).rem_euclid(1.0);
        let d1 = self.phase * self.window;
        let d2 = p2 * self.window;
        let g1 = (PI * self.phase).sin();
        let g2 = (PI * p2).sin();
        // sin² + cos² crossfade (the two gains are sin(πp) and |cos(πp)|).
        self.line.tap(d1) * g1 * g1 + self.line.tap(d2) * g2 * g2
    }
}

/// A peak-following compressor (gain computer in dB), stereo-linked.
#[derive(Default)]
struct Compressor {
    env: f32,
}

impl Compressor {
    /// Gain for a frame whose peak is `level`.
    #[inline]
    fn gain(&mut self, level: f32, thresh_db: f32, ratio: f32, att: f32, rel: f32) -> f32 {
        let db = 20.0 * level.max(1e-6).log10();
        let over = (db - thresh_db).max(0.0);
        let target = over * (1.0 - 1.0 / ratio);
        let coef = if target > self.env { att } else { rel };
        self.env += coef * (target - self.env);
        10f32.powf(-self.env / 20.0)
    }
}

fn coef(secs: f32, sr: f32) -> f32 {
    1.0 - (-1.0 / (secs.max(1e-5) * sr)).exp()
}

/// A small white-noise source (xorshift), no allocation.
#[derive(Clone, Copy)]
struct Noise(u32);

impl Noise {
    #[inline]
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Freeverb's comb and all-pass, for the REVERB.
struct Comb {
    line: Vec<f32>,
    i: usize,
    store: f32,
}

impl Comb {
    #[inline]
    fn run(&mut self, x: f32, fb: f32, damp: f32) -> f32 {
        let y = self.line[self.i];
        self.store = y * (1.0 - damp) + self.store * damp;
        self.line[self.i] = x + self.store * fb;
        self.i = (self.i + 1) % self.line.len();
        y
    }
}

struct Allpass {
    line: Vec<f32>,
    i: usize,
}

impl Allpass {
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let b = self.line[self.i];
        let y = -x + b;
        self.line[self.i] = x + b * 0.5;
        self.i = (self.i + 1) % self.line.len();
        y
    }
}

// ------------------------------------------------------------------ the unit

/// One effect, on the pads set to it.
pub struct SpFx {
    sr: f32,
    s: FxSettings,
    /// The knobs, smoothed (so turning one does not crackle).
    k: [f32; 3],
    lines: [Line; 2],
    shift: [Shifter; 4],
    svf: [Svf; 16],
    lp: [OnePole; 6],
    eq: [Biquad; 6],
    comp: Compressor,
    noise: Noise,
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
    lfo: f32,
    lfo2: f32,
    env: f32,
    hold: [f32; 2],
    hold_n: f32,
    crackle: f32,
    fb: [f32; 2],
    ap: [[f32; 6]; 2],
    wow_drift: f32,
    /// Vinyl Sim: how much record surface is heard, following the music (0 in silence).
    surface: f32,
}

impl SpFx {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as f32;
        let scale = sr / 44_100.0;
        let mut combs = Vec::new();
        let mut allpasses = Vec::new();
        for (c, spread) in [0usize, 23].into_iter().enumerate() {
            for len in [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617] {
                combs.push(Comb {
                    line: vec![0.0; ((len + spread) as f32 * scale) as usize + 1],
                    i: 0,
                    store: 0.0,
                });
            }
            for len in [556, 441, 341, 225] {
                allpasses.push(Allpass {
                    line: vec![0.0; ((len + spread) as f32 * scale) as usize + 1],
                    i: 0,
                });
            }
            let _ = c;
        }
        let line = || Line::new((sr * 1.6) as usize);
        Self {
            sr,
            s: FxSettings::default(),
            k: SpFxKind::FilterDrive.defaults(),
            lines: [line(), line()],
            shift: [
                Shifter::new(sr),
                Shifter::new(sr),
                Shifter::new(sr),
                Shifter::new(sr),
            ],
            svf: [Svf::default(); 16],
            lp: [OnePole::default(); 6],
            eq: [Biquad::default(); 6],
            comp: Compressor::default(),
            noise: Noise(0x1234_5678),
            combs,
            allpasses,
            lfo: 0.0,
            lfo2: 0.0,
            env: 0.0,
            hold: [0.0; 2],
            hold_n: 0.0,
            crackle: 0.0,
            fb: [0.0; 2],
            ap: [[0.0; 6]; 2],
            wow_drift: 0.0,
            surface: 0.0,
        }
    }

    pub fn settings(&self) -> FxSettings {
        self.s
    }

    /// New settings; a different effect starts from silence.
    pub fn set(&mut self, s: FxSettings) {
        if s.kind != self.s.kind || (s.on && !self.s.on) {
            self.reset();
            self.k = s.ctrl;
        }
        self.s = s;
        if s.kind == SpFxKind::Equalizer {
            self.set_eq();
        }
    }

    /// Clears every tail.
    pub fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        for s in &mut self.shift {
            s.clear();
        }
        self.svf = [Svf::default(); 16];
        self.lp = [OnePole::default(); 6];
        for b in &mut self.eq {
            b.z = [0.0; 2];
        }
        self.comp = Compressor::default();
        for c in &mut self.combs {
            c.line.fill(0.0);
            c.store = 0.0;
        }
        for a in &mut self.allpasses {
            a.line.fill(0.0);
        }
        self.fb = [0.0; 2];
        self.ap = [[0.0; 6]; 2];
        self.env = 0.0;
    }

    fn set_eq(&mut self) {
        let db = |c: f32| (c.clamp(0.0, 1.0) - 0.5) * 30.0;
        let [lo, mid, hi] = self.s.ctrl;
        for ch in 0..2 {
            self.eq[ch * 3].set(0, 120.0, db(lo), 0.7, self.sr);
            self.eq[ch * 3 + 1].set(1, 1_000.0, db(mid), 0.9, self.sr);
            self.eq[ch * 3 + 2].set(2, 6_000.0, db(hi), 0.7, self.sr);
        }
    }

    /// One frame of the pads set to the effect (when it is off, they pass as they are).
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if !self.s.on || !self.s.kind.available() {
            return (l, r);
        }
        let sm = 0.002;
        for i in 0..3 {
            self.k[i] += (self.s.ctrl[i] - self.k[i]) * sm;
        }
        let out = self.effect(l, r);
        let ok = out.0.is_finite() && out.1.is_finite();
        if ok {
            out
        } else {
            self.reset();
            (0.0, 0.0)
        }
    }

    fn effect(&mut self, l: f32, r: f32) -> (f32, f32) {
        let sr = self.sr;
        let [c1, c2, c3] = self.k;
        match self.s.kind {
            SpFxKind::FilterDrive => {
                // Low-pass 60 Hz – 16 kHz, then overdrive (manual p. 24: "LPF + overdrive").
                let fc = 60.0 * (16_000f32 / 60.0).powf(c1);
                let k = 2.0 - 1.9 * c2;
                let drive = 1.0 + c3 * c3 * 30.0;
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let (lp, _, _) = self.svf[ch].run(x, fc, k, sr);
                    o[ch] = (lp * drive).tanh();
                }
                let out = 1.0 - 0.4 * c3;
                (o[0] * out, o[1] * out)
            }
            SpFxKind::Pitch => {
                // ±2 octaves (manual p. 24), feedback, then direct/effect balance.
                let ratio = 2f32.powf((c1 - 0.5) * 4.0);
                let fb = c2 * 0.85;
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let y = self.shift[ch].run(x + self.fb[ch] * fb, ratio);
                    self.fb[ch] = y;
                    o[ch] = x * (1.0 - c3) + y * c3;
                }
                (o[0], o[1])
            }
            SpFxKind::Delay => {
                let d = delay_seconds(self.s.ctrl[0], self.s.bpm) * sr;
                let fb = c2 * 0.92;
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let y = self.lines[ch].tap(d);
                    self.lines[ch].push(x + y * fb);
                    o[ch] = x + y * c3;
                }
                (o[0], o[1])
            }
            SpFxKind::VinylSim => self.vinyl(l, r),
            SpFxKind::Isolator => {
                // Low < 250 Hz, high > 3 kHz, mid between; left kills, centre keeps, right
                // boosts by up to 6 dB (a DJ isolator; manual p. 24 says each band is cut or
                // kept).
                let g = |c: f32| {
                    if c < 0.5 {
                        (c * 2.0) * (c * 2.0)
                    } else {
                        1.0 + (c - 0.5) * 2.0
                    }
                };
                let (gl, gm, gh) = (g(c1), g(c2), g(c3));
                // Fourth-order (Linkwitz-Riley) bands: two Butterworth sections each.
                let q = std::f32::consts::SQRT_2;
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let f = &mut self.svf[ch * 8..ch * 8 + 8];
                    let a = f[0].run(x, 250.0, q, sr).0;
                    let low = f[1].run(a, 250.0, q, sr).0;
                    let a = f[2].run(x, 3_000.0, q, sr).2;
                    let high = f[3].run(a, 3_000.0, q, sr).2;
                    let a = f[4].run(x, 250.0, q, sr).2;
                    let m = f[5].run(a, 250.0, q, sr).2;
                    let a = f[6].run(m, 3_000.0, q, sr).0;
                    let mid = f[7].run(a, 3_000.0, q, sr).0;
                    o[ch] = low * gl + mid * gm + high * gh;
                }
                (o[0], o[1])
            }
            SpFxKind::Reverb => {
                let fb = 0.70 + c1 * 0.28;
                let damp = 0.05 + (1.0 - c2) * 0.6;
                let input = (l + r) * 0.015;
                let mut o = [0.0; 2];
                for (ch, slot) in o.iter_mut().enumerate() {
                    let mut acc = 0.0;
                    for c in &mut self.combs[ch * 8..ch * 8 + 8] {
                        acc += c.run(input, fb, damp);
                    }
                    for a in &mut self.allpasses[ch * 4..ch * 4 + 4] {
                        acc = a.run(acc);
                    }
                    *slot = acc;
                }
                (l + o[0] * c3 * 3.0, r + o[1] * c3 * 3.0)
            }
            SpFxKind::TapeEcho => {
                // Tape speed: faster repeats at the right. Wow on the head, tape-like tone and
                // saturation in the loop.
                let base = 0.6 - c1 * 0.52;
                self.lfo = (self.lfo + 0.7 / sr).fract();
                let wobble = 1.0 + 0.004 * (TAU * self.lfo).sin();
                let d = base * sr * wobble;
                let fb = c2 * 0.95;
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let y = self.lines[ch].tap(d);
                    let y = self.lp[ch].lp(y, 3_500.0, sr);
                    let y = y - self.lp[ch + 2].lp(y, 120.0, sr);
                    self.lines[ch].push((x + y * fb).tanh());
                    o[ch] = x + y * c3 * 1.2;
                }
                (o[0], o[1])
            }
            SpFxKind::Chorus => {
                let rate = 0.1 + c2 * c2 * 6.0;
                self.lfo = (self.lfo + rate / sr).fract();
                let depth = c1 * 0.008 * sr;
                let base = 0.012 * sr;
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let ph = self.lfo + ch as f32 * 0.25;
                    let d = base + depth * (0.5 + 0.5 * (TAU * ph).sin());
                    self.lines[ch].push(x);
                    o[ch] = x + self.lines[ch].tap(d) * c3;
                }
                (o[0], o[1])
            }
            SpFxKind::Flanger => {
                // CTRL 2 fully left stops the sweep and CTRL 1 is the manual delay (p. 25).
                let sweep = if c2 < 0.02 {
                    c1
                } else {
                    self.lfo = (self.lfo + (0.05 + c2 * c2 * 4.0) / sr).fract();
                    0.5 + 0.5 * c1 * (TAU * self.lfo).sin()
                };
                let d = (0.0005 + sweep * 0.007) * sr;
                let fb = c3 * 0.92;
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let y = self.lines[ch].tap(d);
                    self.lines[ch].push(x + y * fb);
                    o[ch] = (x + y) * 0.7;
                }
                (o[0], o[1])
            }
            SpFxKind::Phaser => {
                let sweep = if c2 < 0.02 {
                    c1
                } else {
                    self.lfo = (self.lfo + (0.05 + c2 * c2 * 4.0) / sr).fract();
                    0.5 + 0.5 * c1 * (TAU * self.lfo).sin()
                };
                let fc = 200.0 * 15f32.powf(sweep);
                let t = (PI * fc / sr).tan();
                let a = (t - 1.0) / (t + 1.0);
                let fb = c3 * 0.85;
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let mut y = x + self.fb[ch] * fb;
                    for st in &mut self.ap[ch] {
                        let v = a * y + *st;
                        *st = y - a * v;
                        y = v;
                    }
                    self.fb[ch] = y;
                    o[ch] = (x + y) * 0.6;
                }
                (o[0], o[1])
            }
            SpFxKind::TremoloPan => {
                self.lfo = (self.lfo + (0.5 + c2 * c2 * 12.0) / sr).fract();
                let s = (TAU * self.lfo).sin();
                if c3 < 0.5 {
                    let g = 1.0 - c1 * (0.5 + 0.5 * s);
                    (l * g, r * g)
                } else {
                    let p = c1 * s;
                    let a = (p + 1.0) * std::f32::consts::FRAC_PI_4;
                    let m = (l + r) * 0.5;
                    (m * a.cos() * 1.41, m * a.sin() * 1.41)
                }
            }
            SpFxKind::Distortion | SpFxKind::Overdrive | SpFxKind::Fuzz => {
                let kind = self.s.kind;
                let gain = match kind {
                    SpFxKind::Overdrive => 1.0 + c1 * 20.0,
                    SpFxKind::Distortion => 1.0 + c1 * 80.0,
                    _ => 5.0 + c1 * 300.0,
                };
                let tone = 800.0 * 15f32.powf(c2);
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let v = x * gain;
                    let y = match kind {
                        SpFxKind::Overdrive => (v + 0.2 * v * v.abs().min(1.0)).tanh(),
                        SpFxKind::Distortion => v.clamp(-1.0, 1.0) * 0.8 + 0.2 * v.tanh(),
                        _ => {
                            let a = v.abs().min(4.0);
                            (v.signum() * (1.0 - (-a).exp())).clamp(-1.0, 1.0)
                        }
                    };
                    o[ch] = self.lp[ch].lp(y, tone, sr) * c3 * 0.8;
                }
                (o[0], o[1])
            }
            SpFxKind::Wah => {
                let level = l.abs().max(r.abs());
                let c = if level > self.env { 0.01 } else { 0.0005 };
                self.env += c * (level - self.env);
                let fc = (300.0 * 8f32.powf(c2) * (1.0 + c1 * 6.0 * self.env)).min(6_000.0);
                let k = 2.0 - 1.9 * c3;
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let (_, bp, _) = self.svf[ch].run(x, fc, k, sr);
                    o[ch] = bp * 2.0;
                }
                (o[0], o[1])
            }
            SpFxKind::Octave => {
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let down2 = self.shift[ch].run(x, 0.25);
                    let down1 = self.shift[ch + 2].run(x, 0.5);
                    o[ch] = down2 * c1 + down1 * c2 + x * c3;
                }
                (o[0], o[1])
            }
            SpFxKind::Comp => {
                // SUSTAIN: how far down the threshold goes; ATTACK 0.1–50 ms; LEVEL out.
                let thresh = -c1 * 40.0;
                let g = self.comp.gain(
                    l.abs().max(r.abs()),
                    thresh,
                    4.0,
                    coef(0.0001 + c2 * 0.05, sr),
                    coef(0.15, sr),
                );
                let makeup = 10f32.powf(-thresh * 0.75 * 0.5 / 20.0);
                let out = g * makeup * c3 * 1.6;
                (l * out, r * out)
            }
            SpFxKind::Equalizer => {
                let mut o = [l, r];
                for (ch, x) in o.iter_mut().enumerate() {
                    for b in &mut self.eq[ch * 3..ch * 3 + 3] {
                        *x = b.run(*x);
                    }
                }
                (o[0], o[1])
            }
            SpFxKind::LoFi => {
                // SAMPLE RATE 1–44.1 kHz (sample and hold), BIT 4–16, FILTER low-pass.
                let rate = 1_000.0 * 44.1f32.powf(c1);
                self.hold_n += rate / sr;
                if self.hold_n >= 1.0 {
                    self.hold_n -= self.hold_n.floor();
                    let bits = 4.0 + c2 * 12.0;
                    let q = 2f32.powf(bits - 1.0);
                    self.hold = [(l * q).round() / q, (r * q).round() / q];
                }
                let fc = 500.0 * 36f32.powf(c3);
                (
                    self.lp[0].lp(self.hold[0], fc, sr),
                    self.lp[1].lp(self.hold[1], fc, sr),
                )
            }
            SpFxKind::NoiseGen => {
                let n = self.noise.next();
                let hiss = self.lp[0].lp(n, 500.0 * 30f32.powf(c1), sr);
                let crack = self.crackle_sample(c2 * c2 * 40.0);
                let add = (hiss * 0.5 + crack) * c3 * 0.3;
                (l + add, r + add)
            }
            SpFxKind::RadioTuning => {
                let off = (c1 - 0.5) * 6.0;
                let sig = (-off * off).exp();
                let band = 400.0 + c3 * 3_000.0;
                let n = self.noise.next() * (c2 * 0.3 + (1.0 - sig) * 0.15);
                let mut o = [0.0; 2];
                let m = (l + r) * 0.5 * sig;
                for (ch, slot) in o.iter_mut().enumerate() {
                    let (_, bp, _) = self.svf[ch].run(m + n, band, 1.2, sr);
                    *slot = (bp * 2.5).tanh();
                }
                (o[0], o[1])
            }
            SpFxKind::RingMod => {
                let hz = 20.0 * 100f32.powf(c1);
                self.lfo = (self.lfo + hz / sr).fract();
                let m = (TAU * self.lfo).sin();
                (l * m * c2 + l * c3, r * m * c2 + r * c3)
            }
            SpFxKind::ChromaticPs => {
                let r1 = 2f32.powf(chromatic_semis(self.s.ctrl[0]) / 12.0);
                let r2 = 2f32.powf(chromatic_semis(self.s.ctrl[1]) / 12.0);
                let mut o = [0.0; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let a = self.shift[ch].run(x, r1);
                    let b = self.shift[ch + 2].run(x, r2);
                    o[ch] = x * (1.0 - c3) + (a + b) * 0.5 * c3 * 1.4;
                }
                (o[0], o[1])
            }
            SpFxKind::CCanceler => {
                // The centre (what is equal in both sides) cancelled; L-R BALANCE weighs the
                // sides; LOW BOOST brings the bass back from the mix, HIGH BOOST lifts the top.
                let w = 0.5 + (c1 - 0.5);
                let side = l * w - r * (1.0 - w);
                let bass = self.lp[0].lp((l + r) * 0.5, 150.0, sr) * c2 * 2.0;
                let top = side - self.lp[1].lp(side, 4_000.0, sr);
                let y = side * 1.4 + top * c3 * 2.0;
                (y + bass, y + bass)
            }
            SpFxKind::Slicer | SpFxKind::VoiceTrans => (l, r),
        }
    }

    /// Vinyl clicks: a random pop now and then (`per_second` on average), with a short ring.
    fn crackle_sample(&mut self, per_second: f32) -> f32 {
        if (self.noise.next() * 0.5 + 0.5) < per_second / self.sr {
            self.crackle = self.noise.next().signum() * (0.3 + 0.7 * (self.noise.next().abs()));
        }
        let c = self.crackle;
        self.crackle *= 0.6;
        c
    }

    /// VINYL SIM (manual p. 24: "the sonic qualities of an analog record"). Modelled, not
    /// measured: a hard, fast compressor the loud hits pump (CTRL 1), a record's band-limits
    /// and grit, surface hiss and crackle (CTRL 2), and wow and flutter through a short
    /// modulated delay (CTRL 3). See `docs/SP303-NOTES.md` §9 for what this rests on.
    fn vinyl(&mut self, l: f32, r: f32) -> (f32, f32) {
        let sr = self.sr;
        let [c1, c2, c3] = self.k;
        // Wow (0.55 Hz) and flutter (7 Hz) with a slow random drift, up to ~2.5 ms of swing.
        self.lfo = (self.lfo + 0.55 / sr).fract();
        self.lfo2 = (self.lfo2 + 7.0 / sr).fract();
        self.wow_drift += (self.noise.next() * 0.5 - self.wow_drift) * (0.3 / sr);
        let swing =
            (TAU * self.lfo).sin() * 0.8 + (TAU * self.lfo2).sin() * 0.15 + self.wow_drift * 0.6;
        let d = (0.003 + c3 * c3 * 0.0025 * swing) * sr;
        let mut x = [0.0; 2];
        for (ch, v) in [l, r].into_iter().enumerate() {
            self.lines[ch].push(v);
            let y = if c3 > 0.001 { self.lines[ch].tap(d) } else { v };
            // A record's bandwidth: no deep sub, a softened top.
            let hp = y - self.lp[ch].lp(y, 35.0, sr);
            x[ch] = self.lp[ch + 2].lp(hp, 12_000.0 - c1 * 5_000.0, sr);
        }
        // Compression: up to 10:1 from a threshold down to -32 dB, 1 ms attack, 120 ms
        // release, with make-up gain, so a loud kick ducks the rest.
        let thresh = -c1 * 32.0;
        let ratio = 1.0 + c1 * 9.0;
        let g = self.comp.gain(
            x[0].abs().max(x[1].abs()),
            thresh,
            ratio,
            coef(0.001, sr),
            coef(0.12, sr),
        );
        let makeup = 10f32.powf((-thresh * (1.0 - 1.0 / ratio) * 0.6) / 20.0);
        // The surface noise rides on the music: it fades in with the pads (5 ms) and out over
        // about a second after they stop, so an idle sampler is silent rather than hissing.
        let level = l.abs().max(r.abs());
        let target = (level / 0.02).min(1.0);
        let t = if target > self.surface { 0.005 } else { 1.0 };
        self.surface += (target - self.surface) * (1.0 - coef(t, sr));
        let n = self.noise.next();
        let hiss = self.lp[4].lp(n, 6_000.0, sr) * c2 * 0.05 * self.surface;
        let crack = self.crackle_sample(1.0 + c2 * 25.0) * c2 * 0.25 * self.surface;
        let mut o = [0.0; 2];
        for ch in 0..2 {
            let v = x[ch] * g * makeup;
            // A little grit from the hot level.
            o[ch] = (v * 1.2).tanh() / 1.2_f32.tanh() + hiss + crack;
        }
        (o[0], o[1])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn run(fx: &mut SpFx, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len());
        assert_no_alloc::assert_no_alloc(|| {
            for x in input {
                out.push(fx.process(*x, *x).0);
            }
        });
        out
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    fn sine(hz: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (TAU * hz * i as f32 / SR as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn every_available_effect_is_finite_and_not_silent() {
        let input = sine(220.0, 24_000);
        for kind in SpFxKind::ALL.into_iter().filter(|k| k.available()) {
            let mut fx = SpFx::new(SR);
            let mut s = FxSettings {
                kind,
                on: true,
                ctrl: kind.defaults(),
                bpm: 120.0,
            };
            if kind == SpFxKind::CCanceler {
                // A mono sine is all centre: give it one side.
                s.ctrl = [1.0, 0.0, 0.0];
            }
            fx.set(s);
            let mut out = Vec::new();
            for x in &input {
                let (a, b) = fx.process(*x, if kind == SpFxKind::CCanceler { 0.0 } else { *x });
                assert!(a.is_finite() && b.is_finite(), "{kind:?}");
                out.push(a);
            }
            assert!(rms(&out[12_000..]) > 0.01, "{kind:?} is silent");
        }
    }

    #[test]
    fn vinyl_sim_surface_noise_rides_on_the_music() {
        let mut fx = SpFx::new(SR);
        fx.set(FxSettings {
            kind: SpFxKind::VinylSim,
            on: true,
            ctrl: [0.6, 0.3, 0.25],
            bpm: 120.0,
        });
        // An idle sampler: silence in, silence out (no static on an empty track).
        let idle = run(&mut fx, &vec![0.0; 48_000]);
        assert!(
            rms(&idle) < 1e-6,
            "hiss with nothing playing: {}",
            rms(&idle)
        );
        // Playing: the music comes through, and the surface noise is there under it.
        let played = run(&mut fx, &sine(220.0, 24_000));
        assert!(rms(&played[12_000..]) > 0.05);
        // After the pads stop, the noise fades away within about a second and a half.
        let tail = run(&mut fx, &vec![0.0; 96_000]);
        assert!(
            rms(&tail[..2_400]) > 1e-4,
            "no surface right after the music"
        );
        assert!(
            rms(&tail[72_000..]) < 1e-4,
            "surface still loud: {}",
            rms(&tail[72_000..])
        );
    }

    #[test]
    fn off_passes_through_and_no_effect_allocates() {
        let mut fx = SpFx::new(SR);
        let input = sine(440.0, 1000);
        assert_eq!(run(&mut fx, &input), input);
        for kind in SpFxKind::ALL {
            fx.set(FxSettings {
                kind,
                on: true,
                ctrl: kind.defaults(),
                bpm: 90.0,
            });
            run(&mut fx, &input);
        }
    }

    #[test]
    fn the_delay_repeats_at_its_note_value() {
        let mut fx = SpFx::new(SR);
        // CTRL 1 on "t4" (the 8th value): a quarter note at 120 BPM is 0.5 s.
        let t4 = 7.5 / 13.0;
        assert_eq!(DELAY_NOTES[delay_note(t4)].0, "t4");
        fx.set(FxSettings {
            kind: SpFxKind::Delay,
            on: true,
            ctrl: [t4, 0.0, 1.0],
            bpm: 120.0,
        });
        let mut input = vec![0.0; 30_000];
        input[0] = 1.0;
        let out = run(&mut fx, &input);
        let echo = (100..30_000)
            .max_by(|a, b| out[*a].abs().total_cmp(&out[*b].abs()))
            .unwrap();
        assert!((echo as i64 - 24_000).abs() <= 2, "echo at {echo}");
        // A whole note at 40 BPM (6 s) is brought under the 1450 ms limit.
        assert!(delay_seconds(1.0, 40.0) <= MAX_DELAY_SECONDS);
    }

    #[test]
    fn vinyl_sim_compression_ducks_after_a_loud_hit() {
        let mut fx = SpFx::new(SR);
        fx.set(FxSettings {
            kind: SpFxKind::VinylSim,
            on: true,
            ctrl: [1.0, 0.0, 0.0],
            bpm: 120.0,
        });
        // A quiet tone, then a loud burst, then the quiet tone again.
        let quiet = sine(300.0, 9_600)
            .into_iter()
            .map(|x| x * 0.1)
            .collect::<Vec<_>>();
        let mut input = quiet.clone();
        input.extend(sine(60.0, 4_800).into_iter().map(|x| x * 2.0));
        input.extend(quiet.iter().take(2_400));
        let out = run(&mut fx, &input);
        let before = rms(&out[4_800..9_600]);
        let after = rms(&out[14_400 + 200..14_400 + 2_400]);
        assert!(after < before * 0.7, "pumps: {before} then {after}");
    }

    #[test]
    fn the_pitch_effect_moves_the_pitch() {
        let mut fx = SpFx::new(SR);
        // CTRL 1 three quarters: up an octave; all effect.
        fx.set(FxSettings {
            kind: SpFxKind::Pitch,
            on: true,
            ctrl: [0.75, 0.0, 1.0],
            bpm: 120.0,
        });
        let input = sine(300.0, 32_768);
        let out = run(&mut fx, &input);
        let hz = crate::sampler::tests::peak_hz(&out[8_192..], SR);
        assert!((hz - 600.0).abs() < 8.0, "{hz}");
    }

    #[test]
    fn isolator_kills_the_low_band() {
        let mut fx = SpFx::new(SR);
        fx.set(FxSettings {
            kind: SpFxKind::Isolator,
            on: true,
            ctrl: [0.0, 0.5, 0.5],
            bpm: 120.0,
        });
        let out = run(&mut fx, &sine(60.0, 24_000));
        assert!(rms(&out[12_000..]) < 0.05, "{}", rms(&out[12_000..]));
        let mut fx = SpFx::new(SR);
        fx.set(FxSettings {
            kind: SpFxKind::Isolator,
            on: true,
            ctrl: [0.0, 0.5, 0.5],
            bpm: 120.0,
        });
        let out = run(&mut fx, &sine(1_000.0, 24_000));
        assert!(rms(&out[12_000..]) > 0.3);
    }

    #[test]
    fn mfx_numbers_follow_the_panel() {
        assert_eq!(SpFxKind::from_mfx(1), Some(SpFxKind::Reverb));
        assert_eq!(SpFxKind::from_mfx(14), Some(SpFxKind::LoFi));
        assert_eq!(SpFxKind::from_mfx(21), Some(SpFxKind::CCanceler));
        assert_eq!(SpFxKind::VinylSim.mfx_number(), None);
        assert_eq!(SpFxKind::Comp.mfx_number(), Some(12));
        assert_eq!(chromatic_semis(1.0), 12.0);
    }
}
