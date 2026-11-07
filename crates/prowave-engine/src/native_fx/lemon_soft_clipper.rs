// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// Originally part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// Ported to Pro Wave from Lemon Studio's crates/os-engine/src/effects/soft_clipper.rs (the
// sound) and plugins/lemon-small-fx/src/soft_clipper.rs (names, value texts). The saved-state
// reading is left out; the sound is unchanged.

//! Lemon Soft Clipper: Lemon Studio's soft clipper (a recreation of FL's Fruity Soft Clipper).
//!
//! Logic Pro equivalent: none (FL Studio's Fruity Soft Clipper); compare the Limiter's soft
//! knee or Clip Distortion.
//!
//! No memory: each sample above the threshold is bent onto an exponential curve that leaves the threshold with slope
//! 1 and approaches full scale; samples below it pass unchanged. A post gain follows. Nothing glides: a change acts
//! from the next block on, as in FL.
//!
// spec: plugins/fruity-soft-clipper.md, plugins/lemon-soft-clipper.md

use super::level_curve::curve;
use super::{block_effect, stored_values, NativeFx, NativeParam};

pub const ID: &str = "prowave:lemon-soft-clipper";
pub const NAME: &str = "Lemon Soft Clipper";

/// The settings, in the saved state's order.
pub const COUNT: usize = 2;
pub const THRESHOLD: usize = 0;
pub const POST: usize = 1;

/// The editor's ranges.
pub const RANGES: [(i32, i32); COUNT] = [(1, 127), (0, 160)];

/// FL's `Default` preset, also a fresh instance's settings.
pub const DEFAULTS: [i32; COUNT] = [100, 128];

pub static PARAMS: [NativeParam; COUNT] = [
    NativeParam::knob("Threshold", RANGES[0].0, RANGES[0].1, DEFAULTS[0], "dB"),
    NativeParam::knob("Post gain", RANGES[1].0, RANGES[1].1, DEFAULTS[1], "dB"),
];

/// The threshold as a level, `single(threshold) * 0.0078125` (spec "From the settings to the constants").
pub fn threshold_level(threshold: i32) -> f32 {
    threshold as f32 * 0.0078125
}

/// The post gain: the level curve in the host's form of `single(post gain) * 0.0078125`; 128 is unity.
pub fn post_gain(post: i32) -> f32 {
    curve(post as f32 * 0.0078125)
}

/// A gain as FL's hint shows it.
pub(crate) fn db_text(gain: f32) -> String {
    if gain <= 0.0 {
        "-INFdB".into()
    } else {
        format!("{:.1}dB", 20.0 * f64::from(gain).log10())
    }
}

pub fn value_text(k: usize, v: i32) -> String {
    match k {
        THRESHOLD => db_text(threshold_level(v)),
        POST => db_text(post_gain(v)),
        _ => String::new(),
    }
}

/// The curve's constants for a threshold: `T`, `K = 1 - T` (2^-16 when not above 0) and `R = 1 / K`, singles.
pub fn constants(threshold: i32) -> (f32, f32, f32) {
    let t = threshold_level(threshold);
    let mut k = 1.0 - t;
    if k <= 0.0 {
        k = 1.0 / 65536.0;
    }
    (t, k, 1.0 / k)
}

/// 2^64: the spacing of the x87 unit's extended values just below 1 is its reciprocal.
const TWO64: f64 = 18_446_744_073_709_551_616.0;

/// The clipper's arithmetic on one sample (spec "Behaviour"): above the threshold, `(1 - exp((T - a) * R)) * K + T` in
/// doubles, rounded once, with the input's sign bit; below it the sample itself. `a > T` compares the singles as
/// non-negative numbers, so a NaN counts as above.
///
/// The exponential is FL's extended one: just below 1 its values lie 2^-64 apart, so `1 - exp` is a multiple of 2^-64
/// there (taken here from `exp_m1`, rounded to that grid).
#[inline]
pub fn clip(s: f32, t: f32, k: f32, r: f32) -> f32 {
    let a = s.abs();
    if a > t || a.is_nan() {
        let m = -((f64::from(t) - f64::from(a)) * f64::from(r)).exp_m1();
        let m = if m <= 0.5 {
            (m * TWO64).round_ties_even() / TWO64
        } else {
            m
        };
        let y = (m * f64::from(k) + f64::from(t)) as f32;
        y.copysign(s)
    } else {
        s
    }
}

/// The soft clipper.
pub struct LemonSoftClipper {
    rate: u32,
    applied: [i32; COUNT],
    t: f32,
    k: f32,
    r: f32,
    gain: f32,
}

impl LemonSoftClipper {
    pub fn new(rate: u32, values: &[f32]) -> Self {
        let mut c = LemonSoftClipper {
            rate,
            applied: DEFAULTS,
            t: 0.0,
            k: 0.0,
            r: 0.0,
            gain: 1.0,
        };
        c.derive(stored_values(&PARAMS, values));
        c
    }

    fn derive(&mut self, v: [i32; COUNT]) {
        self.applied = v;
        (self.t, self.k, self.r) = constants(v[THRESHOLD]);
        self.gain = post_gain(v[POST]);
    }

    /// One sample of either channel: the curve, then the post gain unless it is exactly 1.
    #[inline]
    pub fn sample(&self, s: f32) -> f32 {
        let y = clip(s, self.t, self.k, self.r);
        if self.gain != 1.0 {
            y * self.gain
        } else {
            y
        }
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

block_effect!(LemonSoftClipper);

impl NativeFx for LemonSoftClipper {
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(p) = PARAMS.get(index) else { return };
        let mut v = self.applied;
        v[index] = p.stored(value);
        if v != self.applied {
            self.derive(v);
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

    #[test]
    fn constants_texts_and_the_curve_match_the_original() {
        assert_eq!(constants(100), (0.78125, 0.21875, 1.0 / 0.21875f32));
        assert_eq!(post_gain(128), 1.0);
        assert_eq!(value_text(THRESHOLD, 100), "-2.1dB");
        assert_eq!(value_text(POST, 0), "-INFdB");
        assert_eq!(value_text(POST, 160), "5.6dB");
        let c = LemonSoftClipper::new(44100, &[]);
        for s in [0.0f32, 0.5, -0.78125, 0.78125] {
            assert_eq!(c.sample(s).to_bits(), s.to_bits());
        }
        assert!((c.sample(1.0) - 0.9195).abs() < 1e-4);
        let z = LemonSoftClipper::new(44100, &[0.0, 128.0]);
        // The range starts at 1: threshold 1/128.
        assert_eq!(z.param(THRESHOLD), 1.0);
    }

    /// Whatever goes in, the output stays within full scale times the post gain.
    #[test]
    fn output_is_bounded() {
        for (thr, post) in [(1, 128), (64, 128), (127, 128), (100, 160)] {
            let mut c = LemonSoftClipper::new(48000, &[thr as f32, post as f32]);
            let bound = post_gain(post);
            let loud: Vec<f32> = noise(48000, 50.0, 3)
                .into_iter()
                .chain([f32::MAX, -1e30, 4.0])
                .collect();
            let (l, r) = run_blocks(&mut c, &loud, &loud, 256);
            for (x, y) in loud.iter().zip(&l) {
                assert!(y.abs() <= bound * 1.000_001, "{thr} {post}: {x} -> {y}");
                assert_eq!(y.signum(), x.signum());
            }
            assert_eq!(l, r);
        }
    }
}
