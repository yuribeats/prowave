// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Pro Wave contributors.

//! Pro Wave Tune: live pitch correction for a monophonic voice.
//!
//! Logic Pro equivalent: Pitch Correction (Key and Scale, Response, Detune, Bypass notes) with
//! the parts of Antares Auto-Tune people reach for: Retune Speed down to 0 ms (the hard-tuned
//! sound of trap and pop), Humanize, Flex-Tune, a vibrato control, formant and throat shift,
//! and a MIDI mode that tunes to the notes of a MIDI track instead of a scale.
//!
//! Signal path: a YIN pitch tracker ([`tracker`]) looks at the newest input every
//! [`ProWaveTune::hop`] samples; when the sound is voiced the correction logic picks the note
//! to aim at (the key and scale, the MIDI notes, or the Pitch Override automation) and how far
//! to move toward it (Retune Speed, Humanize, Flex-Tune, Vibrato, Amount); the PSOLA shifter
//! ([`shifter`]) moves the pitch and keeps the formants. Unvoiced sound passes delayed and
//! untouched. The whole effect delays the sound by [`LATENCY_SECONDS`], reported to the host
//! for plug-in delay compensation.
//!
//! The project's key and tuning and the MIDI notes come from the app through a
//! [`TuneRemote`] ([`open_tune`]); the effect reports the pitch it hears and makes on the
//! remote's [`TuneMeter`]. Neither side waits for the other.

pub mod scale;
pub mod shifter;
pub mod tracker;

use super::{stored_values, NativeFx, NativeParam, NativePreset};
use crate::plugin_track::{BlockProcessor, BlockTransport};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub const ID: &str = "prowave:tune";
pub const NAME: &str = "Pro Wave Tune";

/// The delay the effect adds, in seconds: the shifter's look-ahead.
pub const LATENCY_SECONDS: f64 = 0.008;
/// The pitch logic runs this often (seconds); 64 samples at 48 kHz.
const HOP_SECONDS: f32 = 64.0 / 48000.0;
/// The longest extra retune time Humanize gives a held note, in ms.
const HUMANIZE_MS: f32 = 400.0;
/// Below this level (RMS) the sound counts as silence (about -50 dBFS).
const GATE_RMS: f32 = 0.003;
/// A note starts being corrected at this aperiodicity or lower, and stops above the second.
const VOICED_ON: f32 = 0.2;
const VOICED_OFF: f32 = 0.35;
/// The snapping hysteresis, in semitones.
const HYSTERESIS: f32 = 0.2;

// The parameters, in saved order.
pub const KEY: usize = 0;
pub const SCALE: usize = 1;
pub const NOTES: usize = 2;
pub const TARGET: usize = 3;
pub const MIDI_TRACK: usize = 4;
pub const RETUNE: usize = 5;
pub const HUMANIZE: usize = 6;
pub const FLEX: usize = 7;
pub const VIBRATO: usize = 8;
pub const ADD_VIBRATO: usize = 9;
pub const VIBRATO_RATE: usize = 10;
pub const FORMANT: usize = 11;
pub const THROAT: usize = 12;
pub const TRANSPOSE: usize = 13;
pub const VOICE: usize = 14;
pub const AMOUNT: usize = 15;
pub const MIX: usize = 16;
pub const OVERRIDE: usize = 17;
pub const COUNT: usize = 18;

/// Target choices.
pub const TARGET_SCALE: i32 = 0;
pub const TARGET_MIDI: i32 = 1;

/// The C major scale, the Custom notes' default.
pub const C_MAJOR: i32 = 0b1010_1011_0101;

const VOICE_NAMES: [&str; 4] = [
    tracker::VOICES[0].0,
    tracker::VOICES[1].0,
    tracker::VOICES[2].0,
    tracker::VOICES[3].0,
];

const fn knob(
    name: &'static str,
    min: i32,
    max: i32,
    default: i32,
    unit: &'static str,
) -> NativeParam {
    NativeParam::knob(name, min, max, default, unit)
}

pub static PARAMS: [NativeParam; COUNT] = [
    NativeParam::choice("Key", 0, 0, &scale::KEYS),
    NativeParam::choice("Scale", 0, 0, &scale::SCALES),
    knob("Custom Notes", 0, 4095, C_MAJOR, "notes"),
    NativeParam::choice("Target", 0, 0, &["Scale", "MIDI"]),
    knob("MIDI Track", 0, 128, 0, "track"),
    knob("Retune Speed", 0, 400, 20, "ms"),
    knob("Humanize", 0, 100, 0, "%"),
    knob("Flex-Tune", 0, 200, 0, "cents"),
    knob("Vibrato", 0, 200, 100, "%"),
    knob("Add Vibrato", 0, 100, 0, "cents"),
    knob("Vibrato Rate", 30, 80, 55, "Hz"),
    knob("Formant", -1200, 1200, 0, "st"),
    knob("Throat", 50, 200, 100, "%"),
    knob("Transpose", -24, 24, 0, "st"),
    NativeParam::choice("Voice", 0, 1, &VOICE_NAMES),
    knob("Amount", 0, 100, 100, "%"),
    knob("Mix", 0, 100, 100, "%"),
    knob("Pitch Override", 0, 12700, 0, "note"),
];

/// The presets: the parameters each sets (the key, scale, target and override are kept).
pub static PRESETS: [NativePreset; 5] = [
    NativePreset {
        name: "Hard Tune (trap)",
        values: &[
            (RETUNE, 0.0),
            (HUMANIZE, 0.0),
            (FLEX, 0.0),
            (VIBRATO, 0.0),
            (ADD_VIBRATO, 0.0),
            (FORMANT, 0.0),
            (THROAT, 100.0),
            (AMOUNT, 100.0),
            (MIX, 100.0),
        ],
    },
    NativePreset {
        name: "Natural",
        values: &[
            (RETUNE, 45.0),
            (HUMANIZE, 40.0),
            (FLEX, 0.0),
            (VIBRATO, 100.0),
            (ADD_VIBRATO, 0.0),
            (FORMANT, 0.0),
            (THROAT, 100.0),
            (AMOUNT, 100.0),
            (MIX, 100.0),
        ],
    },
    NativePreset {
        name: "Subtle",
        values: &[
            (RETUNE, 120.0),
            (HUMANIZE, 70.0),
            (FLEX, 60.0),
            (VIBRATO, 100.0),
            (ADD_VIBRATO, 0.0),
            (FORMANT, 0.0),
            (THROAT, 100.0),
            (AMOUNT, 80.0),
            (MIX, 100.0),
        ],
    },
    NativePreset {
        name: "Formant Up",
        values: &[
            (RETUNE, 25.0),
            (HUMANIZE, 20.0),
            (FLEX, 0.0),
            (VIBRATO, 100.0),
            (ADD_VIBRATO, 0.0),
            (FORMANT, 400.0),
            (THROAT, 100.0),
            (AMOUNT, 100.0),
            (MIX, 100.0),
        ],
    },
    NativePreset {
        name: "Robot",
        values: &[
            (RETUNE, 0.0),
            (HUMANIZE, 0.0),
            (FLEX, 0.0),
            (VIBRATO, 0.0),
            (ADD_VIBRATO, 0.0),
            (FORMANT, -300.0),
            (THROAT, 120.0),
            (AMOUNT, 100.0),
            (MIX, 100.0),
        ],
    },
];

pub fn value_text(k: usize, v: i32) -> String {
    match k {
        KEY => scale::KEYS[v.clamp(0, 12) as usize].into(),
        SCALE => scale::SCALES[v.clamp(0, 8) as usize].into(),
        NOTES => scale::mask_text(v.clamp(0, 4095) as u16),
        TARGET => if v == TARGET_MIDI { "MIDI" } else { "Scale" }.into(),
        MIDI_TRACK => {
            if v == 0 {
                "Auto".into()
            } else {
                format!("Track {v}")
            }
        }
        RETUNE => format!("{v} ms"),
        HUMANIZE | VIBRATO | AMOUNT | MIX | THROAT => format!("{v}%"),
        FLEX => {
            if v == 0 {
                "Off".into()
            } else {
                format!("{v} cents")
            }
        }
        ADD_VIBRATO => format!("{v} cents"),
        VIBRATO_RATE => format!("{:.1} Hz", v as f32 / 10.0),
        FORMANT => format!("{:+.1} st", v as f32 / 100.0),
        TRANSPOSE => format!("{v:+} st"),
        VOICE => VOICE_NAMES[v.clamp(0, 3) as usize].into(),
        OVERRIDE => override_text(v),
        _ => v.to_string(),
    }
}

/// The Pitch Override's text: "Off", or the note and cents ("A4 -12c").
pub fn override_text(v: i32) -> String {
    if v <= 0 {
        return "Off".into();
    }
    let note = (v + 49) / 100;
    let cents = v - note * 100;
    if cents == 0 {
        scale::note_name(note)
    } else {
        format!("{} {cents:+}c", scale::note_name(note))
    }
}

/// A note of the MIDI target, in beats from the project start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetNote {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
}

/// What the app tells the effect: the project's key and tuning, and the notes of the MIDI
/// target (sorted by start).
#[derive(Clone, Debug, PartialEq)]
pub struct TuneTargets {
    /// The project key's root (0 = C ... 11 = B) and whether it is minor.
    pub root: u8,
    pub minor: bool,
    /// The project's tuning (A4) in Hz.
    pub a4: f32,
    pub notes: Vec<TargetNote>,
}

impl Default for TuneTargets {
    fn default() -> Self {
        Self {
            root: 0,
            minor: false,
            a4: 440.0,
            notes: Vec::new(),
        }
    }
}

impl TuneTargets {
    /// The note sounding at `beat` (the latest started of those that do).
    pub fn note_at(&self, beat: f64) -> Option<u8> {
        let end = self.notes.partition_point(|n| n.start <= beat);
        self.notes[..end]
            .iter()
            .rev()
            .take(64)
            .find(|n| n.end > beat)
            .map(|n| n.pitch)
    }
}

/// What the effect hears and makes, for the window's meters: MIDI notes with a fraction, NaN
/// while the sound is not voiced.
#[derive(Debug)]
pub struct TuneMeter {
    input: AtomicU32,
    output: AtomicU32,
    target: AtomicU32,
}

impl Default for TuneMeter {
    fn default() -> Self {
        let nan = f32::NAN.to_bits();
        Self {
            input: AtomicU32::new(nan),
            output: AtomicU32::new(nan),
            target: AtomicU32::new(nan),
        }
    }
}

impl TuneMeter {
    fn store(&self, input: f32, output: f32, target: f32) {
        self.input.store(input.to_bits(), Ordering::Relaxed);
        self.output.store(output.to_bits(), Ordering::Relaxed);
        self.target.store(target.to_bits(), Ordering::Relaxed);
    }

    /// The pitch heard, the pitch made and the note aimed at (MIDI notes; NaN: unvoiced).
    pub fn read(&self) -> (f32, f32, f32) {
        (
            f32::from_bits(self.input.load(Ordering::Relaxed)),
            f32::from_bits(self.output.load(Ordering::Relaxed)),
            f32::from_bits(self.target.load(Ordering::Relaxed)),
        )
    }
}

/// The app's end of a running Pro Wave Tune.
pub struct TuneRemote {
    tx: rtrb::Producer<Arc<TuneTargets>>,
    trash: rtrb::Consumer<Arc<TuneTargets>>,
    pub meter: Arc<TuneMeter>,
}

impl TuneRemote {
    /// Sends new targets (the old ones are freed here, not on the audio thread). `false` when
    /// the queue is full (send them again later).
    pub fn send(&mut self, t: Arc<TuneTargets>) -> bool {
        while self.trash.pop().is_ok() {}
        self.tx.push(t).is_ok()
    }

    /// Whether the effect at the other end is gone (its track was rebuilt).
    pub fn is_abandoned(&self) -> bool {
        self.tx.is_abandoned()
    }
}

pub struct ProWaveTune {
    rate: u32,
    rate_f: f32,
    v: [i32; COUNT],
    tracker: tracker::Tracker,
    psola: shifter::Psola,
    hop: usize,
    hop_left: usize,
    hop_seconds: f32,
    // The correction's state.
    voiced: bool,
    on_count: u8,
    /// The period found last while voiced, and the last three pitches.
    period: f32,
    recent: [f32; 3],
    /// The Pitch Override in force (a MIDI note with a fraction), and its last value.
    override_note: Option<f32>,
    override_prev: i32,
    current: Option<i32>,
    held: f32,
    last_target: f32,
    correction: f32,
    vibrato_phase: f32,
    // The transport: the beat when the block began, beats per sample, playing, and how far
    // into the block the effect is.
    beat: f64,
    beats_per_sample: f64,
    playing: bool,
    block_pos: usize,
    targets: Arc<TuneTargets>,
    rx: Option<rtrb::Consumer<Arc<TuneTargets>>>,
    trash: Option<rtrb::Producer<Arc<TuneTargets>>>,
    /// Old targets waiting for room on the way back.
    pending: Option<Arc<TuneTargets>>,
    meter: Arc<TuneMeter>,
}

/// Makes Pro Wave Tune at `sample_rate` with parameter `values` (missing ones take their
/// defaults), and the app's end of it. Allocates: call it off the audio thread.
pub fn open_tune(sample_rate: u32, values: &[f32]) -> (ProWaveTune, TuneRemote) {
    let mut fx = ProWaveTune::new(sample_rate, values);
    let (tx, rx) = rtrb::RingBuffer::new(8);
    let (trash_tx, trash) = rtrb::RingBuffer::new(16);
    fx.rx = Some(rx);
    fx.trash = Some(trash_tx);
    let meter = fx.meter.clone();
    (fx, TuneRemote { tx, trash, meter })
}

impl ProWaveTune {
    /// A tune at `sample_rate` with `values` (missing ones are the defaults). Allocates.
    pub fn new(sample_rate: u32, values: &[f32]) -> Self {
        let rate = sample_rate.max(8000);
        let rate_f = rate as f32;
        let latency = (LATENCY_SECONDS * f64::from(rate)).round() as usize;
        let hop = ((HOP_SECONDS * rate_f).round() as usize).max(16);
        let mut fx = Self {
            rate,
            rate_f,
            v: stored_values(&PARAMS, values),
            tracker: tracker::Tracker::new(rate_f),
            psola: shifter::Psola::new(rate_f, latency),
            hop,
            hop_left: hop,
            hop_seconds: hop as f32 / rate_f,
            voiced: false,
            on_count: 0,
            period: 0.0,
            recent: [0.0; 3],
            override_note: None,
            override_prev: 0,
            current: None,
            held: 0.0,
            last_target: f32::NAN,
            correction: 0.0,
            vibrato_phase: 0.0,
            beat: 0.0,
            beats_per_sample: 0.0,
            playing: false,
            block_pos: 0,
            targets: Arc::new(TuneTargets::default()),
            rx: None,
            trash: None,
            pending: None,
            meter: Arc::new(TuneMeter::default()),
        };
        fx.tracker.set_voice(fx.v[VOICE] as usize);
        fx
    }

    /// The delay in samples.
    pub fn latency_samples(&self) -> usize {
        self.psola.latency()
    }

    /// How often the pitch logic runs, in samples.
    pub fn hop(&self) -> usize {
        self.hop
    }

    /// The meters' readings.
    pub fn meter(&self) -> &Arc<TuneMeter> {
        &self.meter
    }

    /// Uses `t` as the key, tuning and MIDI notes (for tests and offline use; the app sends
    /// them through the remote).
    pub fn set_targets(&mut self, t: Arc<TuneTargets>) {
        self.targets = t;
    }

    /// Takes new targets from the app, if any came. No allocation and no freeing: the old
    /// ones go back to the app (kept here while the way back is full).
    fn take_targets(&mut self) {
        if let Some(old) = self.pending.take() {
            match self.trash.as_mut().map(|t| t.push(old)) {
                Some(Err(rtrb::PushError::Full(o))) => {
                    self.pending = Some(o);
                    return;
                }
                Some(Ok(())) => {}
                None => return,
            }
        }
        let Some(rx) = self.rx.as_mut() else { return };
        if let Ok(t) = rx.pop() {
            let old = std::mem::replace(&mut self.targets, t);
            if let Some(Err(rtrb::PushError::Full(o))) = self.trash.as_mut().map(|t| t.push(old)) {
                self.pending = Some(o);
            }
        }
    }

    /// The note aimed at for `pitch` (MIDI with a fraction) at the beat `beat` (`None`
    /// stopped).
    fn target_for(&mut self, pitch: f32, beat: Option<f64>) -> f32 {
        if let Some(t) = self.override_note {
            self.current = None;
            return t;
        }
        if self.v[TARGET] == TARGET_MIDI {
            if let Some(n) = beat.and_then(|b| self.targets.note_at(b)) {
                self.current = Some(i32::from(n));
                return f32::from(n);
            }
        }
        let mask = scale::mask(
            self.v[KEY],
            self.v[SCALE],
            self.v[NOTES] as u16,
            self.targets.root,
            self.targets.minor,
        );
        let n = scale::snap(pitch, mask, self.current, HYSTERESIS);
        self.current = Some(n);
        n as f32
    }

    /// The pitch logic, once a hop, `offset` samples into the block.
    fn control(&mut self, offset: usize) {
        // The Pitch Override counts once it holds still for two hops: an automation lane that
        // steps from off to a note passes through other values for a moment.
        let ov = self.v[OVERRIDE];
        if ov <= 0 {
            self.override_note = None;
        } else if self.override_prev > 0 && (ov - self.override_prev).abs() < 100 {
            self.override_note = Some(ov as f32 / 100.0);
        }
        self.override_prev = ov;
        let hint = (self.voiced && self.period > 0.0).then_some(self.period);
        let e = self.tracker.estimate_near(hint);
        let on = e.period > 0.0 && e.aperiodicity < VOICED_ON && e.rms > GATE_RMS;
        let keep = e.period > 0.0 && e.aperiodicity < VOICED_OFF && e.rms > GATE_RMS * 0.5;
        let was = self.voiced;
        self.voiced = if was {
            keep
        } else {
            self.on_count = if on { self.on_count + 1 } else { 0 };
            self.on_count >= 2
        };
        let gamma = (self.v[FORMANT] as f32 / 1200.0).exp2() * 100.0 / self.v[THROAT].max(1) as f32;
        if !self.voiced {
            self.on_count = self.on_count.min(1);
            self.current = None;
            self.correction = 0.0;
            self.held = 0.0;
            self.last_target = f32::NAN;
            self.period = 0.0;
            self.psola.set_control(false, 0.0, 0.0, gamma);
            self.meter.store(f32::NAN, f32::NAN, f32::NAN);
            return;
        }
        let a4 = if self.targets.a4.is_finite() && self.targets.a4 > 100.0 {
            self.targets.a4
        } else {
            440.0
        };
        self.period = e.period;
        let p = scale::hz_to_midi(self.rate_f / e.period, a4);
        // The note is chosen on the median of the last three pitches (one stray estimate does
        // not change it).
        if !was {
            self.recent = [p; 3];
        } else {
            self.recent = [self.recent[1], self.recent[2], p];
        }
        let [a, b, c] = self.recent;
        let p_note = a.max(b).min(a.min(b).max(c));
        let beat = self
            .playing
            .then(|| self.beat + (self.block_pos + offset) as f64 * self.beats_per_sample);
        let n = self.target_for(p_note, beat);
        if n != self.last_target {
            self.held = 0.0;
            self.last_target = n;
        } else {
            self.held += self.hop_seconds;
        }
        let dev = n - p;
        let flex = self.v[FLEX] as f32 / 100.0;
        let weight = if flex <= 0.0 || self.override_note.is_some() {
            1.0
        } else {
            1.0 - smoothstep(flex * 0.5, flex, dev.abs())
        };
        let wanted = weight * dev;
        let human = self.v[HUMANIZE] as f32 / 100.0
            * ((self.held - 0.15) / 0.35).clamp(0.0, 1.0)
            * HUMANIZE_MS;
        let retune_ms = self.v[RETUNE] as f32 + human;
        if !was {
            // A note begins as sung, unless the retune is instant.
            self.correction = if retune_ms <= 0.0 { wanted } else { 0.0 };
        }
        let k = if retune_ms <= 0.0 {
            1.0
        } else {
            // Retune Speed: 95% of the way in that time.
            1.0 - (-3.0 * self.hop_seconds * 1000.0 / retune_ms).exp()
        };
        self.correction += (wanted - self.correction) * k;
        let mut o = p + self.correction;
        // What is left of the singer's own movement around the note.
        o = n + self.v[VIBRATO] as f32 / 100.0 * (o - n);
        let depth = self.v[ADD_VIBRATO] as f32 / 100.0;
        if depth > 0.0 {
            self.vibrato_phase += self.v[VIBRATO_RATE] as f32 / 10.0 * self.hop_seconds;
            self.vibrato_phase -= self.vibrato_phase.floor();
            o += depth * (std::f32::consts::TAU * self.vibrato_phase).sin();
        }
        o = p + self.v[AMOUNT] as f32 / 100.0 * (o - p);
        o += self.v[TRANSPOSE] as f32;
        let ratio = ((o - p) / 12.0).exp2();
        self.psola
            .set_control(true, e.period, e.period / ratio, gamma);
        self.meter.store(p, o, n);
    }

    fn run(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.take_targets();
        let mix = self.v[MIX] as f32 / 100.0;
        for i in 0..l.len().min(r.len()) {
            let (x, y) = (l[i], r[i]);
            self.tracker.push(0.5 * (x + y));
            self.hop_left -= 1;
            if self.hop_left == 0 {
                self.hop_left = self.hop;
                self.control(i);
            }
            let (wl, wr) = self.psola.process(x, y);
            let (dl, dr) = self.psola.dry();
            l[i] = mix * wl + (1.0 - mix) * dl;
            r[i] = mix * wr + (1.0 - mix) * dr;
        }
        self.block_pos += l.len();
    }
}

#[inline]
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if b <= a {
        return if x < a { 0.0 } else { 1.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl BlockProcessor for ProWaveTune {
    fn midi(&mut self, _status: u8, _data1: u8, _data2: u8, _offset: u32) {}

    fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        let n = super::copy_in(input, (&mut *out.0, &mut *out.1));
        self.run(&mut out.0[..n], &mut out.1[..n]);
    }

    fn latency_seconds(&self) -> f64 {
        self.latency_samples() as f64 / f64::from(self.rate)
    }

    fn set_parameter(&mut self, index: usize, value: f32) {
        self.set_param(index, value);
    }

    fn block_transport(&mut self, t: &BlockTransport) {
        self.playing = t.playing;
        self.beat = t.beat;
        self.beats_per_sample = if t.tempo.is_finite() && t.tempo > 0.0 {
            t.tempo / 60.0 / f64::from(self.rate)
        } else {
            0.0
        };
        self.block_pos = 0;
    }
}

impl NativeFx for ProWaveTune {
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(p) = PARAMS.get(index) {
            self.v[index] = p.stored(value);
            if index == VOICE {
                self.tracker.set_voice(self.v[VOICE] as usize);
            }
        }
    }

    fn param(&self, index: usize) -> f32 {
        self.v.get(index).map_or(0.0, |v| *v as f32)
    }
}

#[cfg(test)]
mod tests;
