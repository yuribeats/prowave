// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// Originally part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// Ported to Pro Wave from Lemon Studio's crates/os-engine/src/effects/fast_dist.rs (the sound)
// and plugins/lemon-small-fx/src/fast_distortion.rs (names, value texts). Change for Pro Wave:
// the 20 curve tables are worked out once for the whole program (when the first instance is
// made), and an entry's product with the wet gain is taken per sample, so a setting change
// never allocates; the products are the same doubles the original stores.

//! Lemon Fast Distortion: Lemon Studio's fast distortion (a recreation of FL's Fruity Fast
//! Dist).
//!
//! Logic Pro equivalent: none (FL Studio's Fruity Fast Dist); compare Distortion II.
//!
//! A waveshaper without memory: each sample is replaced by a value looked up, with linear interpolation, from a table
//! of 8192 whole 16-bit levels over its own magnitude, and mixed with the unprocessed sample. Two curve types: A
//! compresses what is above -25 dB by a ratio the threshold sets and adds a make-up gain; B raises the attenuation in
//! dB to a power below one. A sample is worked out in more than single precision and rounded toward zero at the end.
//!
// spec: plugins/fruity-fast-dist.md, plugins/lemon-fast-distortion.md

use std::sync::OnceLock;

use super::lemon_soft_clipper::db_text;
use super::level_curve::curve as level_curve;
use super::{block_effect, stored_values, NativeFx, NativeParam};

pub const ID: &str = "prowave:lemon-fast-distortion";
pub const NAME: &str = "Lemon Fast Distortion";

/// The settings, in the saved state's order.
pub const COUNT: usize = 5;
pub const PRE: usize = 0;
pub const THRESHOLD: usize = 1;
pub const TYPE: usize = 2;
pub const MIX: usize = 3;
pub const POST: usize = 4;

/// The settings' ranges.
pub const RANGES: [(i32, i32); COUNT] = [(64, 192), (1, 10), (0, 1), (0, 128), (0, 128)];

/// FL's `Default` preset: unity pre amp, threshold 10, type A, full mix and post gain.
pub const DEFAULTS: [i32; COUNT] = [128, 10, 0, 128, 128];

/// Curve types.
pub const TYPE_A: i32 = 0;
pub const TYPE_B: i32 = 1;

pub static PARAMS: [NativeParam; COUNT] = [
    NativeParam::knob("Pre amp", RANGES[PRE].0, RANGES[PRE].1, DEFAULTS[PRE], "dB"),
    NativeParam::knob(
        "Threshold",
        RANGES[THRESHOLD].0,
        RANGES[THRESHOLD].1,
        DEFAULTS[THRESHOLD],
        "",
    ),
    NativeParam::choice("Type", 0, DEFAULTS[TYPE], &["A", "B"]),
    NativeParam::knob("Mix", RANGES[MIX].0, RANGES[MIX].1, DEFAULTS[MIX], "%"),
    NativeParam::knob(
        "Post gain",
        RANGES[POST].0,
        RANGES[POST].1,
        DEFAULTS[POST],
        "dB",
    ),
];

/// The table's size (one more entry follows for the interpolation).
pub const TABLE: usize = 8192;
/// Curves: A at thresholds 1..10, then B at 1..10.
const CURVES: usize = 20;

/// A level knob: the level curve in the plugins' form of `value / 128` (exactly 1 at 128).
pub fn knob(value: i32) -> f32 {
    level_curve(value as f32 * 0.0078125)
}

/// Value texts: pre amp and post gain as dB, the threshold as its number, the type as A or B,
/// the mix as the share of the distorted signal.
pub fn value_text(k: usize, v: i32) -> String {
    match k {
        THRESHOLD => v.to_string(),
        TYPE => if v == TYPE_A { "A" } else { "B" }.into(),
        MIX => format!("{:.1}%", f64::from(knob(v)) * 100.0),
        PRE | POST => db_text(knob(v)),
        _ => String::new(),
    }
}

/// The gains of the mix: `wet = mix * post`, `dry = (post - wet) * pre * post` (singles).
pub fn gains(pre: f32, mix: f32, post: f32) -> (f32, f32) {
    let wet = mix * post;
    let dry = (post - wet) * pre * post;
    (wet, dry)
}

/// The curve's whole 16-bit level (`Round(10^(shaped / 20) * 32767)`, doubles) at `i` of 8191 for a step
/// `10 - threshold`; 0 at `i = 0`.
pub fn level(i: usize, step: i32, kind: i32) -> f64 {
    if i == 0 {
        return 0.0;
    }
    let u = i as f64 / 8191.0;
    let db = 20.0 * u.log10();
    let step = f64::from(step);
    let shaped = if kind == TYPE_A {
        let k = if db >= -25.0 {
            (db + 25.0) * (step + 1.0) * 0.1 - 25.0
        } else {
            db
        };
        k + (9.0 - step) * 1.6
    } else {
        -(-db).powf(step * 0.02 + 0.8)
    };
    (10f64.powf(shaped / 20.0) * 32767.0).round_ties_even()
}

/// The curve a stored threshold and type pick, as (threshold 1..10, type): number `type * 10 + threshold - 1` of the
/// 20, the nearest end beyond them.
pub fn curve(threshold: i32, kind: i32) -> (i32, i32) {
    let n = (i64::from(kind) * 10 + i64::from(threshold) - 1).clamp(0, 19) as i32;
    (n % 10 + 1, n / 10)
}

/// Every curve's table before the wet gain: `value / 32768` (exact) per entry, and an 8193rd
/// entry equal to the last.
fn tables() -> &'static [[f64; TABLE + 1]] {
    static TABLES: OnceLock<Vec<[f64; TABLE + 1]>> = OnceLock::new();
    TABLES.get_or_init(|| {
        (0..CURVES)
            .map(|n| {
                let (threshold, kind) = ((n % 10) as i32 + 1, (n / 10) as i32);
                let mut t = [0.0; TABLE + 1];
                for (i, v) in t.iter_mut().enumerate().take(TABLE) {
                    *v = level(i, 10 - threshold, kind) / 32768.0;
                }
                t[TABLE] = t[TABLE - 1];
                t
            })
            .collect()
    })
}

/// From `p = 2^31` on (and for NaN) the table's part of a sample is +0: FL's whole-number index overflows there.
const OVERFLOW: f64 = 2_147_483_648.0;

/// Rounds toward zero to single, as FL stores a sample.
#[inline]
pub fn chop(y: f64) -> f32 {
    let s = y as f32;
    if f64::from(s).abs() > y.abs() {
        // One step towards zero: the magnitude's bits less one (infinity becomes the largest finite single).
        f32::from_bits(s.to_bits() - 1)
    } else {
        s
    }
}

/// The fast distortion.
pub struct LemonFastDistortion {
    rate: u32,
    applied: [i32; COUNT],
    tables: &'static [[f64; TABLE + 1]],
    /// The curve in use.
    table: usize,
    wet: f64,
    /// `pre * 8191`, not rounded to single.
    scale: f64,
    dry: f32,
}

impl LemonFastDistortion {
    /// Works the curve tables out the first time any instance is made.
    pub fn new(rate: u32, values: &[f32]) -> Self {
        let mut d = LemonFastDistortion {
            rate,
            applied: stored_values(&PARAMS, values),
            tables: tables(),
            table: 0,
            wet: 1.0,
            scale: 0.0,
            dry: 0.0,
        };
        d.derive();
        d
    }

    fn derive(&mut self) {
        let v = self.applied;
        let pre = knob(v[PRE]);
        let (wet, dry) = gains(pre, knob(v[MIX]), knob(v[POST]));
        let (threshold, kind) = curve(v[THRESHOLD], v[TYPE]);
        self.table = ((kind * 10 + threshold - 1) as usize).min(CURVES - 1);
        self.wet = f64::from(wet);
        self.scale = f64::from(pre) * 8191.0;
        self.dry = dry;
    }

    /// One sample of either channel: the interpolated table at `p = |x * pre * 8191|`, with the sign of `x`, over `x *
    /// dry`, all in doubles, rounded toward zero to single.
    #[inline]
    pub fn sample(&self, x: f32) -> f32 {
        let t = &self.tables[self.table];
        let x64 = f64::from(x);
        let p = (x64 * self.scale).abs();
        let shaped = if p < OVERFLOW {
            let i = (p.min(8191.0) as usize).min(TABLE - 1);
            let f = p - p.trunc();
            let (a, b) = (t[i] * self.wet, t[i + 1] * self.wet);
            (a + (b - a) * f).copysign(x64)
        } else {
            0.0
        };
        chop(x64 * f64::from(self.dry) + shaped)
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        for v in left.iter_mut().chain(right.iter_mut()) {
            *v = self.sample(*v);
        }
    }

    fn latency_samples(&self) -> usize {
        0
    }
}

block_effect!(LemonFastDistortion);

impl NativeFx for LemonFastDistortion {
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(p) = PARAMS.get(index) else { return };
        let v = p.stored(value);
        if self.applied[index] != v {
            self.applied[index] = v;
            self.derive();
        }
    }

    fn param(&self, index: usize) -> f32 {
        self.applied.get(index).map_or(0.0, |v| *v as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_signals::{noise, run_blocks};
    use super::*;

    fn fd(v: [i32; COUNT]) -> LemonFastDistortion {
        LemonFastDistortion::new(44100, &v.map(|x| x as f32))
    }

    /// Samples of FL's renders (Lemon Studio's "Checked against FL" cases).
    #[test]
    fn known_samples_from_fl() {
        let cases: [([i32; COUNT], f32, f32); 6] = [
            ([128, 1, 0, 128, 128], 0.021931838, 0.02192916),
            ([128, 1, 0, 128, 128], 0.0591505, 0.059143275),
            ([192, 1, 0, 104, 93], 0.087571196, 0.11551188),
            ([192, 10, 1, 128, 80], 0.06572277, 0.14467822),
            ([192, 10, 1, 128, 128], -0.25366217, -0.8983255),
            ([128, 6, 0, 128, 128], 0.0, 0.0),
        ];
        for (v, x, want) in cases {
            assert_eq!(fd(v).sample(x), want, "{v:?} {x}");
        }
        assert_eq!(chop(0.1), 0.099999994);
        assert_eq!(
            value_text(MIX, 64),
            format!("{:.1}%", f64::from(knob(64)) * 100.0)
        );
    }

    #[test]
    fn output_is_bounded_and_keeps_the_sign() {
        let mut d = fd([192, 1, 1, 128, 128]);
        let x = noise(44100, 4.0, 11);
        let (l, _) = run_blocks(&mut d, &x, &x, 128);
        for (a, b) in x.iter().zip(&l) {
            assert!(b.abs() <= 1.0, "{a} -> {b}");
            assert!(*a == 0.0 || b.signum() == a.signum());
        }
    }
}
