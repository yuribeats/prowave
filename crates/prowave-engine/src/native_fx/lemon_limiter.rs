// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// Originally part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// Ported to Pro Wave from Lemon Studio's crates/os-engine/src/effects/dynamics.rs (the sound)
// and plugins/lemon-limiter/src/params.rs (names, value texts). Changes for Pro Wave: every
// buffer is allocated when the effect is made (fixed smoother stages, windows and delay line
// sized for their longest setting), the editor's display metering is replaced by a gain
// reduction reading, and the flags byte's "0.2 dB limiter safety" is a parameter.

//! Lemon Limiter: Lemon Studio's limiter, compressor and noise gate (a recreation of FL's
//! Fruity Limiter): gain, an RMS compressor with knee, a look-ahead peak limiter, a noise gate
//! and a ceiling with optional soft saturation, all acting on one smoothed gain applied to the
//! delayed audio. Configured from the 18 parameters of FL's limiter state.
//!
//! Logic Pro equivalent: none (FL Studio's Fruity Limiter); compare Logic's Compressor,
//! Limiter and Noise Gate in one plug-in.
//!
//! Arithmetic follows the spec's singles and doubles (`f32` / `f64`) so the output matches the original's.
//!
// spec: plugins/fruity-limiter-dsp.md, audio/conventions.md (single, double, Round)

// The loops and comparisons keep the original's shape (NaN-aware `!(a > b)` tests, indexed
// loops) so the port can be diffed against Lemon Studio line by line.
#![allow(
    clippy::needless_range_loop,
    clippy::neg_cmp_op_on_partial_ord,
    clippy::needless_late_init
)]

use std::collections::VecDeque;

use super::{block_effect, stored_values, NativeFx, NativeParam};

pub const ID: &str = "prowave:lemon-limiter";
pub const NAME: &str = "Lemon Limiter";

/// FL's parameters, in the original's order (state field k + 1).
pub const FL_PARAMS: usize = 18;
pub const GAIN: usize = 0;
pub const SAT: usize = 1;
pub const CEILING: usize = 2;
pub const LIM_ATT: usize = 3;
pub const LIM_ATT_CURVE: usize = 4;
pub const LIM_REL: usize = 5;
pub const LIM_REL_CURVE: usize = 6;
pub const PEAK_WINDOW: usize = 7;
pub const COMP_THRES: usize = 8;
pub const COMP_RATIO: usize = 9;
pub const COMP_KNEE: usize = 10;
pub const COMP_ATT: usize = 11;
pub const COMP_REL: usize = 12;
pub const COMP_CURVE: usize = 13;
pub const RMS_WINDOW: usize = 14;
pub const GATE_GAIN: usize = 15;
pub const GATE_THRES: usize = 16;
pub const GATE_REL: usize = 17;
/// Pro Wave's extra parameter: the state's flags bit 0 (FL's "0.2 dB limiter safety" menu
/// tick; 0 = ticked, the limiter aims at 0.975 of the ceiling).
pub const SAFETY: usize = 18;

/// Stored ranges (min, max) of the parameters (spec "Parameters").
pub const RANGES: [(i32, i32); FL_PARAMS] = [
    (0, 2000),
    (0, 1000),
    (0, 2000),
    (0, 10000),
    (1, 8),
    (0, 10000),
    (1, 8),
    (0, 10000),
    (0, 1000),
    (-1000, 1000),
    (-1000, 1000),
    (0, 10000),
    (0, 10000),
    (1, 8),
    (0, 10000),
    (0, 1000),
    (0, 1000),
    (0, 10000),
];

/// FL's factory preset "Default" (= a fresh instance).
pub const DEFAULTS: [i32; FL_PARAMS] = [
    1000, 1000, 1000, 1115, 3, 5000, 3, 2882, 1000, 0, 0, 0, 6666, 1, 650, 1000, 0, 4187,
];

/// How a parameter's value reads (Lemon Limiter spec §4.1).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Text {
    /// A level knob with base B: `<dB>dB  <amplitude>`.
    Level(f64),
    Ms,
    /// A release: shown as twice its time.
    MsDoubled,
    Curve,
    Ratio,
    Knee,
}

const TEXTS: [Text; FL_PARAMS] = [
    Text::Level(7.0),
    Text::Level(3.0),
    Text::Level(3.0),
    Text::Ms,
    Text::Curve,
    Text::MsDoubled,
    Text::Curve,
    Text::Ms,
    Text::Level(7.0),
    Text::Ratio,
    Text::Knee,
    Text::Ms,
    Text::MsDoubled,
    Text::Curve,
    Text::Ms,
    Text::Level(7.0),
    Text::Level(7.0),
    Text::MsDoubled,
];

const fn p(k: usize, name: &'static str, unit: &'static str) -> NativeParam {
    NativeParam::knob(name, RANGES[k].0, RANGES[k].1, DEFAULTS[k], unit)
}

/// The parameters: FL's 18 in FL's order, then the limiter safety.
pub static PARAMS: [NativeParam; FL_PARAMS + 1] = [
    p(GAIN, "Gain", "dB"),
    p(SAT, "Soft saturation threshold", "dB"),
    p(CEILING, "Limiter ceiling", "dB"),
    p(LIM_ATT, "Limiter attack time", "ms"),
    p(LIM_ATT_CURVE, "Limiter attack curve", ""),
    p(LIM_REL, "Limiter release time", "ms"),
    p(LIM_REL_CURVE, "Limiter release curve", ""),
    p(PEAK_WINDOW, "Limiter peak window", "ms"),
    p(COMP_THRES, "Comp threshold", "dB"),
    p(COMP_RATIO, "Comp ratio", ":1"),
    p(COMP_KNEE, "Comp knee", "%"),
    p(COMP_ATT, "Comp attack time", "ms"),
    p(COMP_REL, "Comp release time", "ms"),
    p(COMP_CURVE, "Comp curve", ""),
    p(RMS_WINDOW, "Comp RMS window", "ms"),
    p(GATE_GAIN, "Noise gain", "dB"),
    p(GATE_THRES, "Noise threshold", "dB"),
    p(GATE_REL, "Noise release time", "ms"),
    NativeParam::choice("0.2 dB limiter safety", 0, 0, &["On", "Off"]),
];

/// Flags bit 0: the limiter aims at the ceiling itself instead of 0.975 of it.
pub const FLAG_EXACT_CEILING: u8 = 1;

/// Silence: every sample's magnitude at or below this (bit pattern 0x358637BD).
const SILENT: f32 = 1e-6;
/// Scratch length: longer host blocks are processed in pieces of this many samples.
const CHUNK: usize = 1024;
/// Most stages a smoother chain has (the curve knobs' range).
const MAX_STAGES: usize = 8;

/// A level knob: amplitude for stored value `v` with base `b` (spec "Knob laws").
pub fn level(v: i32, b: f64) -> f32 {
    let t = v as f32 * 0.001f32;
    if t == 1.0 {
        return 1.0;
    }
    let x = t * (b.ln() as f32);
    ((f64::from(x).exp() - 1.0) * f64::from((1.0 / (b - 1.0)) as f32)) as f32
}

/// A time knob's milliseconds (0..1000).
pub fn time_ms(v: i32) -> f32 {
    let x = (f64::from(v) * 0.0001) as f32;
    ((f64::from(x) * 501f64.ln()).exp() - 1.0) as f32 * 2.0
}

/// A time knob's length in samples.
pub fn time_samples(v: i32, rate: f64) -> f64 {
    f64::from(time_ms(v)) * 0.001 * rate
}

fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// The coefficient of a smoother direction taking `t` samples (0 = no smoothing).
fn coef(t: f64) -> f64 {
    if t > 0.0 {
        (0.01f64.ln() / t.max(1.0)).exp()
    } else {
        0.0
    }
}

/// The number a value text shows (dB for levels, −∞ as `NEG_INFINITY`; ms; the ratio's X; %).
fn number(t: Text, v: i32) -> f64 {
    match t {
        Text::Level(b) => 20.0 * f64::from(level(v, b)).log10(),
        Text::Ms => f64::from(time_ms(v)),
        Text::MsDoubled => 2.0 * f64::from(time_ms(v)),
        Text::Curve => f64::from(v),
        Text::Ratio => {
            let c = 51f64.powf(f64::from(v.unsigned_abs()) / 1000.0) - 1.0;
            if v > 0 {
                0.38 * c + 1.0
            } else {
                -(0.06 * c + 1.0)
            }
        }
        Text::Knee => (f64::from(v) / 10.0).round(),
    }
}

/// Parameter `k`'s value text (Lemon Limiter spec §4).
pub fn value_text(k: usize, v: i32) -> String {
    if k == SAFETY {
        return if v == 0 { "On" } else { "Off" }.into();
    }
    let Some(&t) = TEXTS.get(k) else {
        return String::new();
    };
    match t {
        Text::Level(b) => {
            let a = level(v, b);
            if a <= 0.0 {
                "-INFdB  0.00".into()
            } else {
                format!("{:.1}dB  {:.2}", 20.0 * f64::from(a).log10(), a)
            }
        }
        Text::Ms | Text::MsDoubled => format!("{:.2}ms", number(t, v)),
        Text::Curve => v.to_string(),
        Text::Ratio => {
            let x = number(t, v);
            if v > 0 {
                format!("{x:.1}:1")
            } else {
                format!("1:{:.1}", -x)
            }
        }
        // A whole number: −4..−1 read 0 %, not "-0%" (§4.1).
        Text::Knee => format!("{}%", number(t, v) as i32),
    }
}

/// Stages of one-pole smoothing in series (spec "Smoothers").
#[derive(Clone, Debug)]
struct Chain {
    rise: f64,
    fall: f64,
    start: f64,
    s: [f64; MAX_STAGES],
    n: usize,
}

impl Chain {
    fn new(start: f64) -> Self {
        Chain {
            rise: 0.0,
            fall: 0.0,
            start,
            s: [start; MAX_STAGES],
            n: 0,
        }
    }

    /// Sets the stage count (a changed count starts again at the starting value) and the times.
    fn set(&mut self, stages: usize, rise: f64, fall: f64) {
        let stages = stages.min(MAX_STAGES);
        if stages != self.n {
            self.n = stages;
            self.s = [self.start; MAX_STAGES];
        }
        self.rise = coef(rise);
        self.fall = coef(fall);
    }

    fn reset_to(&mut self, v: f64) {
        self.s.iter_mut().for_each(|s| *s = v);
    }

    /// Runs every stage over `buf`, one stage after the other.
    fn run(&mut self, buf: &mut [f32]) {
        for s in &mut self.s[..self.n] {
            for x in buf.iter_mut() {
                let v = f64::from(*x);
                let c = if v >= *s { self.rise } else { self.fall };
                *s = v + (*s - v) * c;
                *x = *s as f32;
            }
        }
    }

    /// End of block: tiny states and states that are not a number become 0.
    fn flush(&mut self) {
        for s in &mut self.s[..self.n] {
            if !(s.abs() > 1.0 / 16_777_216.0) {
                *s = 0.0;
            }
        }
    }
}

/// The values worked out from the parameters (spec "Working values").
#[derive(Clone, Debug, Default)]
struct Work {
    gain: f32,
    ceiling: f32,
    limit: f32,
    gate_gain: f32,
    gate_below: f32,
    gate_coef: f32,
    knee: f32,
    room: f32,
    saturate: bool,
    comp: bool,
    threshold: f32,
    thr_db: f32,
    top: f32,
    ratio: f32,
    slope: f32,
    knee_kind: i8,
    ka: f32,
    kb: f32,
    kc: f32,
    look: usize,
    peak_n: usize,
    rms_m: usize,
    tail: i64,
}

/// The limiter's whole processing.
pub struct Dynamics {
    rate: f64,
    p: [i32; FL_PARAMS],
    flags: u8,
    w: Work,
    release: Chain,
    compressor: Chain,
    attack: Chain,
    gate_state: f32,
    /// Peak window: (position, peak) in decreasing order of peak; the front is the largest of the last N.
    peaks: VecDeque<(u64, f32)>,
    pos: u64,
    /// RMS window: the last M squared sums and their running total.
    rms_ring: Vec<f32>,
    rms_at: usize,
    rms_sum: f64,
    /// Look-ahead delay (stereo).
    delay: Vec<[f32; 2]>,
    delay_at: usize,
    counter: i64,
    asleep: bool,
    peak: Vec<f32>,
    rms: Vec<f32>,
    g: Vec<f32>,
    /// The smallest gain of the last block over the gain knob's (1 = no reduction).
    reduction: f32,
}

impl Dynamics {
    /// A limiter at `rate` with stored parameters `p` and flags byte `flags`. Every window and
    /// the delay line get room for their longest setting here.
    pub fn new(rate: f64, p: [i32; FL_PARAMS], flags: u8) -> Self {
        let longest = round(time_samples(RANGES[PEAK_WINDOW].1, rate)).max(0) as usize + 2;
        let mut d = Dynamics {
            rate,
            p,
            flags,
            w: Work::default(),
            release: Chain::new(0.0),
            compressor: Chain::new(0.0),
            attack: Chain::new(1.0),
            gate_state: 1.0,
            peaks: VecDeque::with_capacity(longest),
            pos: 0,
            rms_ring: Vec::with_capacity(longest),
            rms_at: 0,
            rms_sum: 0.0,
            delay: Vec::with_capacity(longest),
            delay_at: 0,
            counter: 0,
            asleep: true,
            peak: vec![0.0; CHUNK],
            rms: vec![0.0; CHUNK],
            g: vec![0.0; CHUNK],
            reduction: 1.0,
        };
        d.work(true);
        d
    }

    /// The delay the effect reports: the look-ahead, in samples.
    pub fn latency(&self) -> usize {
        self.w.look
    }

    pub fn params(&self) -> &[i32; FL_PARAMS] {
        &self.p
    }

    pub fn flags(&self) -> u8 {
        self.flags
    }

    /// True while asleep (silence passes unchanged, spec "Silence").
    pub fn asleep(&self) -> bool {
        self.asleep
    }

    /// Sets parameter `k` to stored value `v` (automation: worked in at once, spec "A parameter
    /// under automation"). As Lemon Limiter does, only a changed value acts.
    pub fn set_param(&mut self, k: usize, v: i32) {
        if k >= FL_PARAMS || self.p[k] == v {
            return;
        }
        // Setting a time or curve knob opens the gate's glide again.
        if matches!(k, 3..=7 | 11..=14 | 17) {
            self.gate_state = 1.0;
        }
        self.p[k] = v;
        self.work(false);
    }

    pub fn set_flags(&mut self, flags: u8) {
        if flags != self.flags {
            self.flags = flags;
            self.work(false);
        }
    }

    /// Works the values out from the parameters; windows and the delay line follow their lengths.
    fn work(&mut self, fresh: bool) {
        let p = &self.p;
        let rate = self.rate;
        let mut w = Work {
            gain: level(p[GAIN], 7.0),
            ceiling: level(p[CEILING], 3.0),
            ..Work::default()
        };
        w.limit = if self.flags & FLAG_EXACT_CEILING != 0 {
            w.ceiling
        } else {
            w.ceiling * 0.975
        };
        w.gate_gain = level(p[GATE_GAIN], 7.0);
        w.gate_below = level(p[GATE_THRES], 7.0).min(w.limit);
        w.gate_coef =
            (0.01f64.ln() / (time_samples(p[GATE_REL], rate).max(1.0) * 2.0)).exp() as f32;
        w.knee = level(p[SAT], 3.0);
        w.saturate = w.knee < 1.0;
        if w.saturate {
            let room = w.ceiling.min(1.0) - w.knee;
            w.room = if room > 0.0 { room } else { 1.525_878_9e-5 };
        }

        w.comp = p[COMP_THRES] < 1000;
        let db_unit = (10f64.ln() * 0.05) as f32;
        w.threshold = level(p[COMP_THRES], 7.0).max((f64::from(db_unit) * -60.0).exp() as f32);
        w.thr_db = (f64::from(w.threshold).log10() * 20.0) as f32;
        w.top = w.threshold * 100.0;
        let c = ((f64::from((p[COMP_RATIO].unsigned_abs() as f64 * 0.001) as f32) * 51f64.ln())
            .exp()
            - 1.0) as f32;
        w.ratio = if p[COMP_RATIO] > 0 {
            (1.0 / (f64::from(c) * 0.38 + 1.0)) as f32
        } else {
            (f64::from(c) * 0.06 + 1.0) as f32
        };
        w.slope = w.ratio - 1.0;
        let kv = p[COMP_KNEE];
        let k = ((f64::from((kv as f32 * 0.001).abs()) * 201f64.ln()).exp() - 1.0) as f32;
        if kv > 0 {
            w.knee_kind = 1;
            w.ka = (f64::from(k) * 0.005) as f32;
            w.kb = (1.0 / f64::from(w.ka + 1.0).ln()) as f32;
        } else if kv < 0 {
            w.knee_kind = -1;
            w.ka = (f64::from(k) * 0.001) as f32;
            w.kb = f64::from(w.ka + 1.0).ln() as f32;
            w.kc = 1.0 / w.ka;
        }

        // The windows and the line never outgrow the room made for them.
        let room = self.delay.capacity();
        let look = (round(time_samples(p[LIM_ATT], rate)).max(0) as usize).min(room);
        w.look = look;
        w.peak_n = (round(time_samples(p[PEAK_WINDOW], rate)).max(0) as usize)
            .max(look)
            .max(1)
            .min(room);
        w.rms_m = (round(time_samples(p[RMS_WINDOW], rate)).max(0) as usize).clamp(1, room.max(1));
        w.tail = look as i64 + round(rate * 1.5);

        // Smoother chains.
        let rel = time_samples(p[LIM_REL], rate);
        let rel_stages = if rel == 0.0 {
            0
        } else {
            p[LIM_REL_CURVE].clamp(0, MAX_STAGES as i32) as usize
        };
        self.release.set(
            rel_stages,
            0.0,
            rel * 2.0 / (rel_stages.max(1) as f64).sqrt(),
        );
        let cs = p[COMP_CURVE].clamp(0, MAX_STAGES as i32) as usize;
        let sq = (cs.max(1) as f64).sqrt();
        self.compressor.set(
            cs,
            time_samples(p[COMP_ATT], rate) / sq,
            time_samples(p[COMP_REL], rate) * 2.0 / sq,
        );
        let att_stages = if look < 1 {
            0
        } else {
            p[LIM_ATT_CURVE].clamp(0, MAX_STAGES as i32) as usize
        };
        let at = f64::from((look as f64 * 0.8) as f32) / (att_stages.max(1) as f64).sqrt();
        self.attack.set(att_stages, at, at);

        let old = std::mem::replace(&mut self.w, w);
        // A changed look-ahead clears the delay line; a changed window clears that window. Both
        // stay within the capacity made in `new`, so nothing is allocated.
        if fresh || old.look != look {
            self.delay.clear();
            self.delay.resize(look, [0.0; 2]);
            self.delay_at = 0;
        }
        if fresh || old.peak_n != self.w.peak_n {
            self.peaks.clear();
        }
        if fresh || old.rms_m != self.w.rms_m {
            self.rms_ring.clear();
            self.rms_ring.resize(self.w.rms_m, 0.0);
            self.rms_at = 0;
            self.rms_sum = 0.0;
        }
    }

    /// Falling asleep, or the slot switched off or on (spec "State and timing").
    pub fn reset(&mut self) {
        self.attack.reset_to(1.0);
        self.release.reset_to(0.0);
        self.compressor.reset_to(0.0);
        self.gate_state = 1.0;
        self.delay.iter_mut().for_each(|s| *s = [0.0; 2]);
        self.delay_at = 0;
    }

    /// Processes one block in place. Blocks are the original's ticks: sleep and waking are
    /// decided per block.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let sound = left[..n]
            .iter()
            .chain(&right[..n])
            .any(|x| !(x.abs() <= SILENT));
        if sound {
            self.counter = self.w.tail;
        } else {
            self.counter -= n as i64;
            if self.counter <= -(n as i64) {
                if !self.asleep {
                    self.asleep = true;
                    self.reset();
                }
                self.reduction = 1.0;
                return;
            }
        }
        self.asleep = false;
        self.reduction = f32::MAX;
        let mut at = 0;
        while at < n {
            let end = (at + CHUNK).min(n);
            self.run(&mut left[at..end], &mut right[at..end]);
            at = end;
        }
        self.release.flush();
        self.compressor.flush();
        self.attack.flush();
    }

    /// The gain reduction of the last block in dB (positive; 0 when the gain knob is at −∞).
    pub fn reduction_db(&self) -> f32 {
        if !(self.w.gain > 0.0) || self.reduction == f32::MAX {
            return 0.0;
        }
        // A gate closed to silence reads 240 dB.
        let db = -20.0 * (self.reduction / self.w.gain).clamp(1e-12, 1.0).log10();
        if db.is_finite() {
            db
        } else {
            0.0
        }
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let w = &self.w;
        // 1. Peak of the undelayed input, held over the peak window.
        for i in 0..n {
            let pk = left[i].abs().max(right[i].abs());
            let at = self.pos;
            while self.peaks.back().is_some_and(|&(_, v)| v <= pk) {
                self.peaks.pop_back();
            }
            // A NaN never leaves the window's back; keep the queue within its room regardless.
            if self.peaks.len() == self.peaks.capacity() {
                self.peaks.pop_front();
            }
            self.peaks.push_back((at, pk));
            while self
                .peaks
                .front()
                .is_some_and(|&(q, _)| q + w.peak_n as u64 <= at)
            {
                self.peaks.pop_front();
            }
            self.peak[i] = self.peaks.front().map_or(0.0, |&(_, v)| v);
            self.pos += 1;
        }

        // 2. Compressor.
        let g = &mut self.g[..n];
        g.fill(w.gain);
        if w.comp {
            let m = w.rms_m;
            let inv = (1.0 / (2.0 * m as f64)) as f32;
            for i in 0..n {
                let (l, r) = (left[i], right[i]);
                let x = l * l + r * r;
                self.rms_sum =
                    (self.rms_sum - f64::from(self.rms_ring[self.rms_at])) + f64::from(x);
                self.rms_ring[self.rms_at] = x;
                self.rms_at = (self.rms_at + 1) % m;
                self.rms[i] = ((f64::from(inv) * self.rms_sum) as f32).max(0.0).sqrt();
            }
            self.compressor.run(&mut self.rms[..n]);
            let db_unit = (10f64.ln() * 0.05) as f32;
            for i in 0..n {
                let lv = self.rms[i];
                if lv > w.threshold {
                    let over = (f64::from(lv.min(w.top)).log10() as f32) * 20.0 + (-w.thr_db);
                    let db = match w.knee_kind {
                        0 => over * w.slope,
                        kind => {
                            let mut u = over + 1.0;
                            u = if kind > 0 {
                                (f64::from(u * w.ka + 1.0).ln() as f32) * w.kb
                            } else {
                                ((f64::from(u * w.kb).exp() as f32) + -1.0) * w.kc
                            };
                            (u + -1.0) * w.ratio - over
                        }
                    };
                    g[i] = (f64::from(db * db_unit).exp() as f32) * w.gain;
                }
            }
        }

        // 3. Delay the audio by the look-ahead.
        if w.look > 0 {
            for i in 0..n {
                let d = &mut self.delay[self.delay_at];
                let out = *d;
                *d = [left[i], right[i]];
                left[i] = out[0];
                right[i] = out[1];
                self.delay_at = (self.delay_at + 1) % w.look;
            }
        }

        // 4. Limiter and gate on the release-smoothed held peak.
        self.release.run(&mut self.peak[..n]);
        for i in 0..n {
            let lv = self.peak[i];
            if lv * g[i] > w.limit {
                g[i] = w.limit / lv;
            }
            if lv < w.gate_below {
                g[i] = w.gate_gain + (self.gate_state - w.gate_gain) * w.gate_coef;
            }
            self.gate_state = g[i];
            // Pro Wave guard: the compressor's gain can overflow to infinity at extreme knee and
            // ratio settings, and times a −∞ gain knob it is not a number; keep it finite.
            if g[i].is_nan() {
                g[i] = 0.0;
                self.gate_state = 0.0;
            } else if g[i] > f32::MAX {
                g[i] = f32::MAX;
                self.gate_state = f32::MAX;
            }
        }

        // 5. Attack smoothing of the gain; 6. output; 7. ceiling.
        self.attack.run(g);
        let ceiling = w.ceiling;
        let inv_room = 1.0 / w.room;
        for i in 0..n {
            self.reduction = self.reduction.min(g[i]);
            for s in [&mut left[i], &mut right[i]] {
                let v = *s * g[i];
                let a = v.abs();
                *s = if w.saturate {
                    if a > w.knee {
                        let y = ((1.0 - (f64::from(w.knee - a) * f64::from(inv_room)).exp())
                            * f64::from(w.room)
                            + f64::from(w.knee)) as f32;
                        y.copysign(v)
                    } else {
                        v
                    }
                } else if a > ceiling {
                    ceiling.copysign(v)
                } else {
                    v
                };
            }
        }
    }
}

/// The Lemon Limiter effect.
pub struct LemonLimiter {
    rate: u32,
    dsp: Dynamics,
}

impl LemonLimiter {
    /// Allocates: call off the audio thread.
    pub fn new(rate: u32, values: &[f32]) -> Self {
        let v = stored_values(&PARAMS, values);
        let mut p = DEFAULTS;
        p.copy_from_slice(&v[..FL_PARAMS]);
        let flags = if v[SAFETY] != 0 {
            FLAG_EXACT_CEILING
        } else {
            0
        };
        LemonLimiter {
            rate,
            dsp: Dynamics::new(f64::from(rate), p, flags),
        }
    }

    pub fn dynamics(&self) -> &Dynamics {
        &self.dsp
    }

    fn run(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.dsp.process(l, r);
    }

    fn latency_samples(&self) -> usize {
        self.dsp.latency()
    }
}

block_effect!(LemonLimiter);

impl NativeFx for LemonLimiter {
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(p) = PARAMS.get(index) else { return };
        let v = p.stored(value);
        if index == SAFETY {
            self.dsp
                .set_flags(if v != 0 { FLAG_EXACT_CEILING } else { 0 });
        } else {
            self.dsp.set_param(index, v);
        }
    }

    fn param(&self, index: usize) -> f32 {
        match index {
            SAFETY => f32::from(self.dsp.flags() & FLAG_EXACT_CEILING),
            k if k < FL_PARAMS => self.dsp.params()[k] as f32,
            _ => 0.0,
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        self.dsp.reduction_db()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_signals::{peak, run_blocks, sine};
    use super::*;
    use crate::plugin_track::BlockProcessor;

    fn values(set: &[(usize, i32)]) -> Vec<f32> {
        let mut v: Vec<f32> = PARAMS.iter().map(|p| p.default).collect();
        for &(k, x) in set {
            v[k] = x as f32;
        }
        v
    }

    #[test]
    fn knob_laws_and_texts_match_the_original() {
        let db = |a: f32| 20.0 * f64::from(a).log10();
        assert_eq!(level(0, 7.0), 0.0);
        assert!((db(level(500, 7.0)) + 11.2).abs() < 0.05);
        assert_eq!(level(1000, 7.0), 1.0);
        assert!((db(level(2000, 7.0)) - 18.06).abs() < 0.01);
        assert!((time_ms(1115) - 2.0).abs() < 0.005);
        assert!((time_ms(10000) - 1000.0).abs() < 0.01);
        assert_eq!(value_text(0, 1000), "0.0dB  1.00");
        assert_eq!(value_text(16, 0), "-INFdB  0.00");
        assert_eq!(value_text(8, 600), "-8.7dB  0.37");
        assert_eq!(value_text(5, 5000), "85.53ms");
        assert_eq!(value_text(9, 1000), "20.0:1");
        assert_eq!(value_text(9, -1000), "1:4.0");
        assert_eq!(value_text(10, -720), "-72%");
        assert_eq!(value_text(SAFETY, 0), "On");
    }

    /// The compressor at a −20 dB threshold and 4:1 brings a steady 0 dBFS-peak sine (RMS
    /// −3 dB, 17 dB over) down by 17 × (1 − 1/4) ≈ 12.8 dB.
    #[test]
    fn compressor_reduces_a_steady_sine_by_the_expected_amount() {
        let rate = 48000.0;
        // Threshold −20 dB: the level knob with base 7 at −20 dB.
        let thr = (0..=1000)
            .min_by_key(|&v| ((20.0 * f64::from(level(v, 7.0)).log10() + 20.0).abs() * 1e6) as i64);
        let thr = thr.unwrap_or(0);
        // Ratio 4:1: 0.38 (51^(v/1000) − 1) + 1 = 4.
        let ratio = ((3.0f64 / 0.38 + 1.0).ln() / 51f64.ln() * 1000.0).round() as i32;
        assert_eq!(value_text(COMP_RATIO, ratio), "4.0:1");
        let mut fx = LemonLimiter::new(
            rate as u32,
            &values(&[
                (COMP_THRES, thr),
                (COMP_RATIO, ratio),
                (LIM_ATT, 0),
                (CEILING, 2000),
            ]),
        );
        let input = sine(48000, 1.0, 1000.0, rate);
        let (l, _) = run_blocks(&mut fx, &input, &input, 256);
        let out_rms = super::super::test_signals::rms(&l[24000..]);
        let in_rms = super::super::test_signals::rms(&input[24000..]);
        let reduction = 20.0 * (in_rms / out_rms).log10();
        let thr_db = 20.0 * level(thr, 7.0).log10();
        let over = 20.0 * in_rms.log10() - thr_db;
        let want = over * (1.0 - 1.0 / 4.0);
        assert!(
            (reduction - want).abs() < 0.5,
            "reduction {reduction} dB, want {want} dB"
        );
        assert!(
            (fx.gain_reduction_db() - want).abs() < 1.0,
            "{}",
            fx.gain_reduction_db()
        );
    }

    #[test]
    fn limiter_ceiling_holds_and_the_look_ahead_is_reported() {
        let rate = 44100;
        let mut fx = LemonLimiter::new(rate, &[]);
        // 2 ms of look-ahead = 88 samples.
        assert_eq!(fx.latency_seconds(), 88.0 / 44100.0);
        // +12 dB of gain into a hot sine.
        fx.set_param(GAIN, 1800.0);
        let input = sine(44100, 0.9, 220.0, 44100.0);
        let (l, r) = run_blocks(&mut fx, &input, &input, 512);
        let settled = peak(&l[4410..]).max(peak(&r[4410..]));
        assert!(settled <= 1.0 && settled > 0.9, "{settled}");
        assert!(fx.gain_reduction_db() > 3.0);
        // Exact ceiling at −6 dB: the output stays at the ceiling.
        let ceiling = (0..=2000)
            .min_by_key(|&v| ((level(v, 3.0) - 0.5).abs() * 1e7) as i64)
            .unwrap_or(0);
        let mut fx = LemonLimiter::new(rate, &values(&[(CEILING, ceiling), (SAFETY, 1)]));
        let (l, _) = run_blocks(&mut fx, &input, &input, 512);
        assert!(peak(&l[4410..]) <= level(ceiling, 3.0) + 1e-6);
    }

    #[test]
    fn noise_gate_closes_below_its_threshold() {
        let mut fx = LemonLimiter::new(
            44100,
            &values(&[(LIM_ATT, 0), (GATE_THRES, 500), (GATE_GAIN, 0)]),
        );
        let quiet = sine(44100, 0.05, 220.0, 44100.0);
        let (l, _) = run_blocks(&mut fx, &quiet, &quiet, 512);
        assert!(peak(&l[30000..]) < 1e-3, "{}", peak(&l[30000..]));
        // Above the threshold it stays open.
        let loud = sine(44100, 0.5, 220.0, 44100.0);
        let mut fx = LemonLimiter::new(
            44100,
            &values(&[(LIM_ATT, 0), (GATE_THRES, 500), (GATE_GAIN, 0)]),
        );
        let (l, _) = run_blocks(&mut fx, &loud, &loud, 512);
        assert!(peak(&l[30000..]) > 0.45);
    }

    #[test]
    fn longest_settings_fit_the_room_made_for_them() {
        let mut fx = LemonLimiter::new(48000, &[]);
        let cap = fx.dsp.delay.capacity();
        for k in [LIM_ATT, PEAK_WINDOW, RMS_WINDOW, COMP_THRES] {
            assert_no_alloc::assert_no_alloc(|| fx.set_param(k, PARAMS[k].max));
        }
        fx.set_param(COMP_THRES, 0.0);
        let input = sine(48000, 0.8, 100.0, 48000.0);
        let (l, _) = run_blocks(&mut fx, &input, &input, 1000);
        assert!(l.iter().all(|x| x.is_finite()));
        assert_eq!(fx.dsp.delay.capacity(), cap);
        assert_eq!(fx.latency_seconds(), fx.dsp.latency() as f64 / 48000.0);
        assert!(fx.dsp.latency() > 47000);
    }
}
