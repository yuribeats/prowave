// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// Originally part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// Ported to Pro Wave from Lemon Studio's crates/os-engine/src/effects/transient_processor.rs
// (the sound) and plugins/lemon-small-fx/src/transient_processor.rs (names, value texts). The
// saved-state reading is left out; the sound, including its glide, is unchanged.

//! Lemon Transient Processor: Lemon Studio's transient processor (a recreation of FL's
//! Transient Processor).
//!
//! Logic Pro equivalent: Enveloper (attack and release gain), with FL's split and drive.
//!
//! The signal is split by a first-order crossover; the high band's attacks and tails are made louder or softer by one
//! gain per sample from five level followers, then soft-saturated with a drive, mixed back with the low band by the
//! split balance, and an output gain follows. The settings glide every 8th sample; the effect stands still in long
//! silence. All arithmetic is single precision with denormals flushed to zero.
//!
// spec: plugins/transient-processor.md, plugins/lemon-transient-processor.md

// The loops and comparisons keep the original's shape (NaN-aware `!(a > b)` tests, indexed
// loops) so the port can be diffed against Lemon Studio line by line.
#![allow(
    clippy::needless_range_loop,
    clippy::neg_cmp_op_on_partial_ord,
    clippy::needless_late_init
)]

use super::{block_effect, stored_values, NativeFx, NativeParam};

pub const ID: &str = "prowave:lemon-transient-processor";
pub const NAME: &str = "Lemon Transient Processor";

/// The settings, in the saved state's order (after the version field).
pub const COUNT: usize = 9;
pub const ATTACK: usize = 0;
pub const DRIVE: usize = 1;
pub const RELEASE: usize = 2;
pub const GAIN: usize = 3;
pub const ATTACK_SHAPE: usize = 4;
pub const RELEASE_SHAPE: usize = 5;
pub const SPLIT: usize = 6;
pub const BALANCE: usize = 7;
pub const ON: usize = 8;

/// The settings' ranges (the editor's).
pub const RANGES: [(i32, i32); COUNT] = [
    (0, 160),
    (0, 160),
    (0, 160),
    (0, 160),
    (0, 2),
    (0, 2),
    (0, 160),
    (0, 160),
    (0, 1),
];

/// A fresh instance's settings, also FL's "Default" preset.
pub const DEFAULTS: [i32; COUNT] = [80, 0, 80, 80, 1, 1, 80, 80, 1];

const SHAPES: &[&str] = &["Sharp", "Mid", "Slow"];

pub static PARAMS: [NativeParam; COUNT] = [
    NativeParam::knob("Attack", 0, 160, DEFAULTS[ATTACK], "%"),
    NativeParam::knob("Drive", 0, 160, DEFAULTS[DRIVE], "dB"),
    NativeParam::knob("Release", 0, 160, DEFAULTS[RELEASE], "%"),
    NativeParam::knob("Gain", 0, 160, DEFAULTS[GAIN], "dB"),
    NativeParam::choice("Attack shape", 0, DEFAULTS[ATTACK_SHAPE], SHAPES),
    NativeParam::choice("Release shape", 0, DEFAULTS[RELEASE_SHAPE], SHAPES),
    NativeParam::knob("Split frequency", 0, 160, DEFAULTS[SPLIT], "Hz"),
    NativeParam::knob("Split balance", 0, 160, DEFAULTS[BALANCE], "%"),
    NativeParam::choice("Processing", 0, DEFAULTS[ON], &["Off", "On"]),
];

/// The settings that glide (spec "Glide of the settings"), in index order.
const GLIDING: [usize; 6] = [ATTACK, DRIVE, RELEASE, GAIN, SPLIT, BALANCE];

// ---------------------------------------------------------------------------------------------------------------
// Single arithmetic with denormals flushed to zero (spec "Behaviour")

/// A denormal counts as zero (keeping its sign).
#[inline]
fn z(x: f32) -> f32 {
    if x != 0.0 && x.abs() < f32::MIN_POSITIVE {
        0.0f32.copysign(x)
    } else {
        x
    }
}

#[inline]
fn add(a: f32, b: f32) -> f32 {
    z(a + b)
}

#[inline]
fn sub(a: f32, b: f32) -> f32 {
    z(a - b)
}

#[inline]
fn mul(a: f32, b: f32) -> f32 {
    z(a * b)
}

#[inline]
fn div(a: f32, b: f32) -> f32 {
    z(a / b)
}

/// A single power, correctly rounded.
fn powf(b: f32, e: f32) -> f32 {
    f64::from(b).powf(f64::from(e)) as f32
}

/// A single tangent, correctly rounded.
fn tanf(x: f32) -> f32 {
    f64::from(x).tan() as f32
}

const C_04: f32 = f32::from_bits(0x3ECC_CCCD);
const C_06: f32 = f32::from_bits(0x3F19_999A);
const C_01: f32 = f32::from_bits(0x3DCC_CCCD);
const C_DRIVE_EXP: f32 = f32::from_bits(0x3F8E_38E3);
const C_GAIN_EXP: f32 = f32::from_bits(0x3F66_6667);
const C_SPLIT_SQ: f32 = f32::from_bits(0x4543_5E28);
const C_SPLIT_LIN: f32 = f32::from_bits(0x417B_53D2);
const C_1E4: f32 = f32::from_bits(0x38D1_B717);
const C_1E6: f32 = f32::from_bits(0x3586_37BD);
const C_A1: f32 = f32::from_bits(0x3F75_C290);
const C_A2: f32 = f32::from_bits(0x3B44_9BA6);
const C_A_MAX: f32 = f32::from_bits(0x40D3_3333);
const C_B1: f32 = f32::from_bits(0x3F57_0A3D);
const C_B2: f32 = f32::from_bits(0x3A1D_4952);
/// The drive's copy when the plugin is created.
const DRIVE_AT_CREATION: f32 = 0.01;
/// The silence count while standing still.
const STILL: i64 = -1_073_741_823;

// ---------------------------------------------------------------------------------------------------------------
// From a setting to its value (spec "From a setting to its value")

/// Setting `k`'s position in 0..1: `single(clamp(value * (1 / range), 0, 1))` in doubles (range 160, or 2 for the
/// shapes).
pub fn position(k: usize, value: i32) -> f32 {
    let range = if k == ATTACK_SHAPE || k == RELEASE_SHAPE {
        2.0
    } else {
        160.0
    };
    (f64::from(value) * (1.0 / range)).clamp(0.0, 1.0) as f32
}

/// `s = t * |t|` with `t = ((x - 0.5) + x) - 0.5`, -1..1.
fn signed_square(x: f32) -> f32 {
    let t = sub(add(sub(x, 0.5), x), 0.5);
    let s = mul(t, t);
    if t < 0.0 {
        -s
    } else {
        s
    }
}

/// The attack's weight, -0.6..0.4.
pub fn attack(x: f32) -> f32 {
    let s = signed_square(x);
    if s >= 0.0 {
        mul(C_04, s)
    } else {
        mul(C_06, s)
    }
}

/// The release's weight, -1..0.6.
pub fn release(x: f32) -> f32 {
    let s = signed_square(x);
    if s >= 0.0 {
        mul(s, C_06)
    } else {
        s
    }
}

/// The drive `D`, 0.1..15 (1 at position 0.1).
pub fn drive(x: f32) -> f32 {
    if x < C_01 {
        add(mul(x, 9.0), C_01)
    } else {
        powf(15.0, mul(sub(x, C_01), C_DRIVE_EXP))
    }
}

/// The output gain `G`, -9..+9 dB.
pub fn gain(x: f32) -> f32 {
    powf(10.0, mul(sub(x, 0.5), C_GAIN_EXP))
}

/// A shape's factor: Sharp 0.5, Mid 1, Slow 2 (the position compared as a double).
pub fn shape(x: f32) -> f64 {
    let x = f64::from(x);
    if x < 0.333 {
        0.5
    } else if x < 0.666 {
        1.0
    } else {
        2.0
    }
}

/// The split's filter coefficient `k = tan(pi * (995 x^2 + 5) / rate)` as the spec rounds it.
pub fn split_k(x: f32, sample_rate: u32) -> f32 {
    let inv = (1.0 / f64::from(sample_rate)) as f32;
    let arg = add(mul(mul(mul(x, x), inv), C_SPLIT_SQ), mul(inv, C_SPLIT_LIN));
    tanf(arg)
}

/// The split frequency in Hz, about `995 x^2 + 5` (the editor's text).
pub fn split_hz(x: f32) -> f64 {
    let x = f64::from(x);
    995.0 * x * x + 5.0
}

/// The split balance: the low band's and the processed high band's levels.
pub fn balance(x: f32) -> (f32, f32) {
    let r = sub(1.0, x);
    (add(r, r).min(1.0), add(x, x).min(1.0))
}

fn mapped(k: usize, v: i32) -> f32 {
    let x = position(k, v);
    match k {
        ATTACK => attack(x),
        RELEASE => release(x),
        DRIVE => drive(x),
        _ => gain(x),
    }
}

/// Parameter `k`'s value text (Lemon Transient Processor's).
pub fn value_text(k: usize, v: i32) -> String {
    match k {
        ATTACK | RELEASE => match mapped(k, v) {
            0.0 => "0%".into(),
            w => format!("{:+.2}%", f64::from(w) * 100.0),
        },
        DRIVE | GAIN => super::lemon_soft_clipper::db_text(mapped(k, v)),
        ATTACK_SHAPE | RELEASE_SHAPE => match shape(position(k, v)) {
            0.5 => "Sharp",
            1.0 => "Mid",
            _ => "Slow",
        }
        .into(),
        SPLIT => format!("{:.1}Hz", split_hz(position(SPLIT, v))),
        BALANCE => match (f64::from(v.clamp(0, 160)) - 80.0) / 80.0 * 100.0 {
            0.0 => "Centred".into(),
            p => format!("{:.1}% {}", p.abs(), if p < 0.0 { "low" } else { "high" }),
        },
        ON => if v == 0 { "Off" } else { "On" }.into(),
        _ => String::new(),
    }
}

/// `0.1^y` in doubles, rounded to a single.
fn p(y: f64) -> f32 {
    0.1f64.powf(y) as f32
}

/// The followers' and the glide's constants for a rate and the two shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Consts {
    kf: f32,
    ka: f32,
    kr: f32,
    kr2: f32,
    k4: f32,
    k3: f32,
    kp: f32,
}

impl Consts {
    fn new(sample_rate: u32, ash: f64, rsh: f64) -> Consts {
        let rate = f64::from(sample_rate);
        let kr = p(1.0 / (rsh * 0.02 * rate));
        Consts {
            kf: p(5.0 / rate),
            ka: 1.0 - p(1.0 / (ash * 0.01 * rate)),
            kr,
            kr2: 1.0 - kr,
            k4: p(1.0 / (rsh * 0.1 * rate)),
            k3: p(1.0 / f64::from(sample_rate as f32 * 4.0)),
            kp: 1.0 - p(200.0 / rate),
        }
    }
}

/// The split's coefficients from `k`: `c = 1 / (k + 1)`, `c * k`, `a = -((k - 1) * c)`.
#[derive(Clone, Copy, Debug, Default)]
struct Split {
    c: f32,
    ck: f32,
    a: f32,
}

impl Split {
    fn new(k: f32) -> Split {
        let c = div(1.0, add(k, 1.0));
        Split {
            c,
            ck: mul(c, k),
            a: -mul(sub(k, 1.0), c),
        }
    }
}

/// The transient processor.
pub struct LemonTransientProcessor {
    rate: u32,
    applied: [i32; COUNT],
    /// The gliding settings' positions and targets (indexed by setting).
    pos: [f32; COUNT],
    target: [f32; COUNT],
    /// The mapped values of the positions.
    attack: f32,
    release: f32,
    drive: f32,
    out_gain: f32,
    split: Split,
    lo: f32,
    hi: f32,
    /// The copies the gain and the saturator use (a sample behind the values).
    attack_copy: f32,
    release_copy: f32,
    drive_copy: f32,
    consts: Consts,
    on: bool,
    e: [f32; 5],
    /// Per channel: the previous input, low and high band.
    x1: [f32; 2],
    low1: [f32; 2],
    high1: [f32; 2],
    /// Samples processed since creation (the glide's phase).
    count: u64,
    /// The silence count.
    quiet: i64,
}

impl LemonTransientProcessor {
    /// A new instance with these settings in force at once (nothing glides in); the followers,
    /// filters and counts at 0, the copies at attack 0, release 0, drive 0.01.
    pub fn new(sample_rate: u32, values: &[f32]) -> Self {
        let v = stored_values(&PARAMS, values);
        let mut t = LemonTransientProcessor {
            rate: sample_rate,
            applied: v,
            pos: [0.0; COUNT],
            target: [0.0; COUNT],
            attack: 0.0,
            release: 0.0,
            drive: 1.0,
            out_gain: 1.0,
            split: Split::default(),
            lo: 1.0,
            hi: 1.0,
            attack_copy: 0.0,
            release_copy: 0.0,
            drive_copy: DRIVE_AT_CREATION,
            consts: Consts::new(sample_rate, 1.0, 1.0),
            on: true,
            e: [0.0; 5],
            x1: [0.0; 2],
            low1: [0.0; 2],
            high1: [0.0; 2],
            count: 0,
            quiet: 0,
        };
        for k in 0..COUNT {
            t.target[k] = position(k, v[k]);
            t.pos[k] = t.target[k];
        }
        for k in GLIDING {
            t.map(k);
        }
        t.reshape();
        t.on = v[ON] != 0;
        t
    }

    /// A new setting: a gliding one gets a new target, the shapes and the switch act at once.
    fn set(&mut self, k: usize, v: i32) {
        if k >= COUNT || v == self.applied[k] {
            return;
        }
        self.applied[k] = v;
        self.target[k] = position(k, v);
        match k {
            ATTACK_SHAPE | RELEASE_SHAPE => {
                self.pos[k] = self.target[k];
                self.reshape();
            }
            ON => self.on = v != 0,
            _ => {}
        }
    }

    /// The followers' and the glide's constants for the rate and the shapes.
    fn reshape(&mut self) {
        let (a, r) = (
            shape(self.pos[ATTACK_SHAPE]),
            shape(self.pos[RELEASE_SHAPE]),
        );
        self.consts = Consts::new(self.rate, a, r);
    }

    /// Setting `k`'s position through its mapping.
    fn map(&mut self, k: usize) {
        let x = self.pos[k];
        match k {
            ATTACK => self.attack = attack(x),
            RELEASE => self.release = release(x),
            DRIVE => self.drive = drive(x),
            GAIN => self.out_gain = gain(x),
            SPLIT => self.split = Split::new(split_k(x, self.rate)),
            BALANCE => (self.lo, self.hi) = balance(x),
            _ => {}
        }
    }

    /// Every 8th processed sample: each gliding setting off its target moves by `kp` of the way, onto it within 1e-4.
    fn glide(&mut self) {
        let kp = self.consts.kp;
        for k in GLIDING {
            let (v, t) = (self.pos[k], self.target[k]);
            if v == t {
                continue;
            }
            let mut n = add(v, mul(sub(t, v), kp));
            if sub(n, t).abs() < C_1E4 {
                n = t;
            }
            self.pos[k] = n;
            self.map(k);
        }
    }

    /// One sample pair (spec "The split", "The gain, per sample", "Saturation and output").
    fn frame(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.count.is_multiple_of(8) {
            self.glide();
        }
        self.count += 1;
        let Split { c, ck, a } = self.split;
        let x = [z(l), z(r)];
        let mut low = [0.0f32; 2];
        let mut high = [0.0f32; 2];
        for ch in 0..2 {
            low[ch] = add(
                add(mul(ck, x[ch]), mul(ck, self.x1[ch])),
                mul(a, self.low1[ch]),
            );
            high[ch] = add(
                add(mul(c, x[ch]), mul(-c, self.x1[ch])),
                mul(a, self.high1[ch]),
            );
        }
        self.x1 = x;
        self.low1 = low;
        self.high1 = high;

        let k = self.consts;
        let m = mul(add(high[1].abs(), high[0].abs()), 0.5);
        let [e1, e2, e3, e4, e5] = &mut self.e;
        *e1 = if m > *e1 { m } else { mul(*e1, k.kf) };
        *e2 = if m > *e2 {
            add(mul(sub(m, *e2), k.ka), *e2)
        } else {
            mul(*e2, k.kf)
        };
        *e3 = if m >= *e3 { m } else { mul(*e3, k.k3) };
        *e4 = if m >= *e4 { m } else { mul(*e4, k.k4) };
        let q = if self.release_copy >= 0.0 && *e4 > 0.0 {
            let d = add(*e4, C_1E4);
            div(sub(d, *e5), d)
        } else {
            1.0
        };
        let q = mul(q, 3.0).clamp(0.0, 1.0);
        *e5 = add(mul(add(mul(q, k.kr), k.kr2), sub(*e4, *e5)), *e5);
        let (e1, e2, e3, e5) = (*e1, *e2, *e3, *e5);

        let ra = if e2 > 0.0 { div(e1, e2) } else { 0.0 };
        let att = if e2 > 0.0 && ra >= 1.0 {
            if self.attack_copy < 0.0 {
                ra
            } else if ra <= 10.0 {
                mul(ra, sub(C_A1, mul(mul(ra, C_A2), ra)))
            } else {
                C_A_MAX
            }
        } else {
            1.0
        };
        let rb = if e5 > C_1E6 { div(e3, e5) } else { 0.0 };
        let rel = if e5 > C_1E6 && rb >= 1.0 {
            if self.release_copy < 0.0 {
                rb
            } else if rb <= 20.0 {
                sub(mul(rb, C_B1), mul(mul(rb, rb), mul(rb, C_B2)))
            } else {
                12.0
            }
        } else {
            1.0
        };
        let s = add(
            mul(sub(rel, 1.0), self.release_copy),
            mul(sub(att, 1.0), self.attack_copy),
        );
        let g = if s >= 0.0 {
            add(s, 1.0)
        } else {
            div(1.0, sub(1.0, s))
        };

        let d = self.drive_copy;
        let mut out = [0.0f32; 2];
        for ch in 0..2 {
            let u = mul(d, mul(high[ch], g));
            let sat = if u < -3.0 {
                -1.0
            } else if u > 3.0 {
                1.0
            } else {
                div(mul(add(mul(u, u), 27.0), u), add(mul(mul(u, 9.0), u), 27.0))
            };
            let y = div(sat, d);
            out[ch] = mul(add(mul(self.lo, low[ch]), mul(self.hi, y)), self.out_gain);
        }

        self.attack_copy = add(sub(self.attack, self.attack_copy), self.attack_copy);
        self.release_copy = add(sub(self.release, self.release_copy), self.release_copy);
        self.drive_copy = add(sub(self.drive, self.drive_copy), self.drive_copy);
        (out[0], out[1])
    }

    /// The silence count for a block of `n` frames: whether the block is processed (spec "State and timing").
    fn runs(&mut self, silent: bool, n: usize) -> bool {
        let n = n as i64;
        if silent {
            self.quiet -= n;
        } else {
            self.quiet = (f64::from(self.rate) * 1.5).round_ties_even() as i64 + 88;
        }
        if self.quiet > -n {
            true
        } else {
            self.quiet = STILL;
            false
        }
    }

    /// One block in place. In long silence, or switched off, the input passes unchanged and the state stands still.
    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let silent = left[..n]
            .iter()
            .chain(&right[..n])
            .all(|x| x.abs() <= C_1E6);
        if self.runs(silent, n) && self.on {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                (*l, *r) = self.frame(*l, *r);
            }
        }
    }

    fn latency_samples(&self) -> usize {
        0
    }
}

block_effect!(LemonTransientProcessor);

impl NativeFx for LemonTransientProcessor {
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(p) = PARAMS.get(index) {
            self.set(index, p.stored(value));
        }
    }

    fn param(&self, index: usize) -> f32 {
        self.applied.get(index).map_or(0.0, |v| *v as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_signals::{peak, run_blocks};
    use super::*;

    #[test]
    fn settings_map_and_read_as_the_original() {
        assert_eq!(attack(position(ATTACK, 80)), 0.0);
        assert_eq!(drive(position(DRIVE, 16)), 1.0);
        assert_eq!(gain(0.5), 1.0);
        assert_eq!(value_text(ATTACK, 160), "+40.00%");
        assert_eq!(value_text(RELEASE, 0), "-100.00%");
        assert_eq!(value_text(DRIVE, 0), "-20.0dB");
        assert_eq!(value_text(DRIVE, 160), "23.5dB");
        assert_eq!(value_text(GAIN, 0), "-9.0dB");
        assert_eq!(value_text(SPLIT, 80), "253.8Hz");
        assert_eq!(value_text(BALANCE, 40), "50.0% low");
        assert_eq!(value_text(RELEASE_SHAPE, 2), "Slow");
    }

    /// A drum-like hit: a burst of 2 kHz tone with a 300 ms decay.
    fn hits(rate: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = (i % (rate as usize / 2)) as f64 / rate;
                (0.6 * (-t / 0.3).exp() * (std::f64::consts::TAU * 2000.0 * t).sin()) as f32
            })
            .collect()
    }

    fn energy(x: &[f32]) -> f64 {
        x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum()
    }

    /// Attack up makes the first milliseconds of each hit louder; attack down softer; the
    /// tail is left about as it was.
    #[test]
    fn attack_changes_the_energy_of_the_onsets() {
        let rate = 44100.0;
        let input = hits(rate, 44100);
        let onset = |x: &[f32]| energy(&x[22050..22050 + 220]);
        let tail = |x: &[f32]| energy(&x[22050 + 8000..22050 + 12000]);
        let run = |attack: f32| {
            let mut t = LemonTransientProcessor::new(rate as u32, &[attack]);
            run_blocks(&mut t, &input, &input, 256).0
        };
        let (neutral, up, down) = (run(80.0), run(160.0), run(0.0));
        assert!(
            onset(&up) > onset(&neutral) * 1.5,
            "{} {}",
            onset(&up),
            onset(&neutral)
        );
        assert!(
            onset(&down) < onset(&neutral) * 0.8,
            "{} {}",
            onset(&down),
            onset(&neutral)
        );
        let r = tail(&up) / tail(&neutral);
        assert!((0.8..1.25).contains(&r), "{r}");
        // Off: the input comes back unchanged.
        let mut off = LemonTransientProcessor::new(
            rate as u32,
            &[160.0, 0.0, 80.0, 80.0, 1.0, 1.0, 80.0, 80.0, 0.0],
        );
        let (l, _) = run_blocks(&mut off, &input, &input, 256);
        assert_eq!(l, input);
        assert!(peak(&up) > peak(&neutral));
    }

    #[test]
    fn settings_glide() {
        let mut t = LemonTransientProcessor::new(44100, &[]);
        t.set_param(GAIN, 160.0);
        assert_eq!(t.param(GAIN), 160.0);
        let x = vec![0.25f32; 9];
        run_blocks(&mut t, &x, &x, 9);
        assert!(t.pos[GAIN] > 0.5 && t.pos[GAIN] < 1.0);
        let x = vec![0.25f32; 8000];
        run_blocks(&mut t, &x, &x, 512);
        assert_eq!(t.pos[GAIN], 1.0);
    }
}
