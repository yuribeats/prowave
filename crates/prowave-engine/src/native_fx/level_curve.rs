// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// Originally part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// Ported to Pro Wave from Lemon Studio's crates/os-dsp/src/level.rs (only `curve`); unchanged
// apart from the module.

//! FL's shared level curve, as the soft clipper's and the fast distortion's gain knobs use it.
//!
// spec: audio/conventions.md ("Level curve")

/// The level curve `(11^x - 1) / 10` in the host's precision: `x * ln 11` rounded to single, the exponential and
/// the subtraction in doubles, times the single nearest 0.1, rounded once. `curve(1)` is exactly 1; values above 1
/// are allowed (`curve(1.25) = 1.9033...`).
pub fn curve(x: f32) -> f32 {
    if x == 1.0 {
        return 1.0;
    }
    let p = x * (11f64.ln() as f32);
    ((f64::from(p).exp() - 1.0) * f64::from(0.1f32)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_at_one_and_zero_at_zero() {
        assert_eq!(curve(1.0), 1.0);
        assert_eq!(curve(0.0), 0.0);
        assert!((curve(1.25) - 1.9033).abs() < 1e-4);
    }
}
