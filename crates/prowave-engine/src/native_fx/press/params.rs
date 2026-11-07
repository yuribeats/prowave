// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// Originally part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// Ported to Pro Wave from Lemon Studio's crates/os-engine/src/effects/maximus/state.rs (the 59
// parameters, their ranges and the factory default) and plugins/press/src/ctl.rs (names and
// value texts). Changes for Pro Wave: the saved state is not read or written; the options
// Lemon keeps in it (oversampling, linear-phase crossover, mid trigger, the compressor safety)
// are four more parameters after FL's 59, and the band names are put before the band
// parameters' names.

//! Press's parameters: FL Maximus's 59 (7 global, then 13 for each of LOW, MID, HIGH and
//! MASTER) and four options.
//!
// spec: plugins/maximus.md ("Parameters", "Knob laws"), plugins/lemon-press.md §3

use super::super::NativeParam;
use super::{crossover_hz, gain, low_cut_hz, samples};

pub const FL_PARAMS: usize = 59;
pub const BANDS: usize = 4;
pub const LOW: usize = 0;
pub const MID: usize = 1;
pub const HIGH: usize = 2;
pub const MASTER: usize = 3;

// The global parameters.
pub const P_LOW_FREQ: usize = 0;
pub const P_LOW_SLOPE: usize = 1;
pub const P_HIGH_FREQ: usize = 2;
pub const P_HIGH_SLOPE: usize = 3;
pub const P_LMH_DELAY: usize = 4;
pub const P_LMH_MIX: usize = 5;
pub const P_LOW_CUT: usize = 6;

// A band's parameters, at `band(b, k)`.
pub const B_MODE: usize = 0;
pub const B_PRE: usize = 1;
pub const B_POST: usize = 2;
pub const B_SEP: usize = 3;
pub const B_THRES: usize = 4;
pub const B_CEIL: usize = 5;
pub const B_ATT: usize = 6;
pub const B_REL2: usize = 7;
pub const B_ATT_STAGES: usize = 8;
pub const B_REL: usize = 9;
pub const B_REL_STAGES: usize = 10;
pub const B_WINDOW: usize = 11;
pub const B_RMS: usize = 12;

/// The options after FL's parameters (in Lemon Studio they live in the saved state).
pub const OVERSAMPLING: usize = 59;
pub const FIR: usize = 60;
pub const MID_TRIGGER: usize = 61;
pub const SAFETY: usize = 62;
pub const COUNT: usize = 63;

/// Band modes: compressor on, compressor off, muted, not split off.
pub const MODE_ON: i32 = 0;
pub const MODE_COMP_OFF: i32 = 1;
pub const MODE_MUTED: i32 = 2;
pub const MODE_OFF: i32 = 3;

/// Parameter `k` of band `b`.
pub const fn band(b: usize, k: usize) -> usize {
    7 + 13 * b + k
}

/// Each FL parameter's range.
pub const fn range(k: usize) -> (i32, i32) {
    match k {
        P_LOW_FREQ | P_HIGH_FREQ => (0, 65536),
        P_LOW_SLOPE | P_HIGH_SLOPE => (0, 1),
        P_LMH_DELAY => (0, 10000),
        P_LMH_MIX => (0, 1000),
        P_LOW_CUT => (-1, 65536),
        _ => match (k - 7) % 13 {
            B_MODE => (0, 3),
            B_PRE | B_POST | B_CEIL => (0, 2000),
            B_SEP | B_THRES => (-1000, 1000),
            B_ATT_STAGES | B_REL_STAGES => (1, 8),
            B_RMS => (0, 1),
            _ => (0, 10000),
        },
    }
}

/// A band's factory settings: on, 0 dB gains, no separation or saturation, 2 ms attack, release 2 as the attack, two
/// attack stages, about 86 ms release with three stages, a 10 ms peak window.
const BAND_DEFAULT: [i32; 13] = [0, 1000, 1000, 0, 0, 1000, 1115, 0, 2, 5000, 3, 2882, 0];

/// The factory default: 200 Hz and 3 kHz at 24 dB, 2 ms band look-ahead, full mix, low cut at 20 Hz, every band
/// on with the 0 dB limiting map.
pub const fn fl_default(k: usize) -> i32 {
    match k {
        P_LOW_FREQ => 19800,
        P_LOW_SLOPE => 1,
        P_HIGH_FREQ => 48065,
        P_HIGH_SLOPE => 1,
        P_LMH_DELAY => 1115,
        P_LMH_MIX => 1000,
        P_LOW_CUT => 32768,
        _ => BAND_DEFAULT[(k - 7) % 13],
    }
}

/// FL's hint names, with the band's name first.
const NAMES: [[&str; 13]; BANDS] = [
    [
        "LOW Mode",
        "LOW Pre gain",
        "LOW Post gain",
        "LOW Stereo separation",
        "LOW Soft saturation threshold",
        "LOW Soft saturation ceiling",
        "LOW Attack time",
        "LOW Release 2 time",
        "LOW Attack / release 2 curve",
        "LOW Release time",
        "LOW Release curve",
        "LOW Sustain window",
        "LOW Sustain mode",
    ],
    [
        "MID Mode",
        "MID Pre gain",
        "MID Post gain",
        "MID Stereo separation",
        "MID Soft saturation threshold",
        "MID Soft saturation ceiling",
        "MID Attack time",
        "MID Release 2 time",
        "MID Attack / release 2 curve",
        "MID Release time",
        "MID Release curve",
        "MID Sustain window",
        "MID Sustain mode",
    ],
    [
        "HIGH Mode",
        "HIGH Pre gain",
        "HIGH Post gain",
        "HIGH Stereo separation",
        "HIGH Soft saturation threshold",
        "HIGH Soft saturation ceiling",
        "HIGH Attack time",
        "HIGH Release 2 time",
        "HIGH Attack / release 2 curve",
        "HIGH Release time",
        "HIGH Release curve",
        "HIGH Sustain window",
        "HIGH Sustain mode",
    ],
    [
        "MASTER Mode",
        "MASTER Pre gain",
        "MASTER Post gain",
        "MASTER Stereo separation",
        "MASTER Soft saturation threshold",
        "MASTER Soft saturation ceiling",
        "MASTER Attack time",
        "MASTER Release 2 time",
        "MASTER Attack / release 2 curve",
        "MASTER Release time",
        "MASTER Release curve",
        "MASTER Sustain window",
        "MASTER Sustain mode",
    ],
];

pub const MODE_NAMES: [&str; 4] = ["on", "compression off", "muted", "off"];
const OFF_ON: &[&str] = &["Off", "On"];

const fn fl_param(k: usize) -> NativeParam {
    let (lo, hi) = range(k);
    let d = fl_default(k);
    match k {
        P_LOW_FREQ => NativeParam::knob("Low band frequency", lo, hi, d, "Hz"),
        P_LOW_SLOPE => NativeParam::choice("Low band slope", 0, d, &["12dB", "24dB"]),
        P_HIGH_FREQ => NativeParam::knob("High band frequency", lo, hi, d, "Hz"),
        P_HIGH_SLOPE => NativeParam::choice("High band slope", 0, d, &["12dB", "24dB"]),
        P_LMH_DELAY => NativeParam::knob("LMH lookahead delay", lo, hi, d, "ms"),
        P_LMH_MIX => NativeParam::knob("LMH mix", lo, hi, d, "%"),
        P_LOW_CUT => NativeParam::knob("Low cut", lo, hi, d, "Hz"),
        _ => {
            let (b, p) = ((k - 7) / 13, (k - 7) % 13);
            let name = NAMES[b][p];
            match p {
                B_MODE => NativeParam::choice(name, 0, d, &MODE_NAMES),
                B_RMS => NativeParam::choice(name, 0, d, &["PEAK", "RMS"]),
                B_PRE | B_POST | B_CEIL => NativeParam::knob(name, lo, hi, d, "dB"),
                B_SEP | B_THRES => NativeParam::knob(name, lo, hi, d, "%"),
                B_ATT_STAGES | B_REL_STAGES => NativeParam::knob(name, lo, hi, d, ""),
                _ => NativeParam::knob(name, lo, hi, d, "ms"),
            }
        }
    }
}

const fn build() -> [NativeParam; COUNT] {
    let mut out = [NativeParam::knob("", 0, 0, 0, ""); COUNT];
    let mut k = 0;
    while k < FL_PARAMS {
        out[k] = fl_param(k);
        k += 1;
    }
    let mut os = NativeParam::choice("Oversampling", 0, 0, &["1x", "2x", "4x", "8x", "16x"]);
    os.live = false;
    out[OVERSAMPLING] = os;
    out[FIR] = NativeParam::choice("Linear-phase crossover", 0, 0, OFF_ON);
    out[MID_TRIGGER] = NativeParam::choice("Mid trigger", 0, 0, OFF_ON);
    out[SAFETY] = NativeParam::choice("0.2 dB compressor safety", 0, 0, &["On", "Off"]);
    out
}

/// FL's 59 parameters, then oversampling (stages: 1x .. 16x; it sizes the effect's memory),
/// the linear-phase crossover, mid trigger and the compressor safety (On: the maps' 0 dB
/// comes out at 0.975).
pub static PARAMS: [NativeParam; COUNT] = build();

/// A gain as dB and as a factor: `0.0dB 1.00`, `-INFdB 0.00`.
fn gain_text(g: f64) -> String {
    let db = if g > 0.0 {
        format!("{:.1}", 20.0 * g.log10())
    } else {
        "-INF".into()
    };
    format!("{db}dB {g:.2}")
}

/// A time knob in ms: the knob law at 1 kHz, release times twice as long (as FL's hints show them).
fn ms(k: usize, v: i32) -> f64 {
    let x = samples(v, 1000.0);
    if k >= 7 && matches!((k - 7) % 13, B_REL | B_REL2) {
        2.0 * x
    } else {
        x
    }
}

fn pct(v: i32) -> String {
    format!("{}%", (f64::from(v.abs()) / 10.0).round() as i64)
}

/// The value text of parameter `k` at `v` (FL's hint values, as Lemon Studio measured them).
pub fn value_text(k: usize, v: i32) -> String {
    match k {
        P_LOW_FREQ | P_HIGH_FREQ => format!("{}Hz", crossover_hz(v).round() as i64),
        P_LOW_SLOPE | P_HIGH_SLOPE => if v == 0 { "12dB" } else { "24dB" }.into(),
        P_LMH_DELAY => format!("{:.2}ms", ms(k, v)),
        P_LMH_MIX => pct(v),
        P_LOW_CUT => {
            if v < 0 {
                "Off".into()
            } else {
                format!("{}Hz", low_cut_hz(v).round() as i64)
            }
        }
        OVERSAMPLING | FIR | MID_TRIGGER | SAFETY => PARAMS[k]
            .choices
            .get(v.max(0) as usize)
            .map_or_else(String::new, |s| (*s).into()),
        k if k >= COUNT => String::new(),
        _ => match (k - 7) % 13 {
            B_MODE => MODE_NAMES[v.clamp(0, 3) as usize].into(),
            B_PRE | B_POST | B_CEIL => gain_text(f64::from(gain(v))),
            B_SEP => match v {
                0 => "Original".into(),
                v if v > 0 => format!("{} merged", pct(v)),
                v => format!("{} separated", pct(v)),
            },
            B_THRES => match v {
                0 => "No saturation".into(),
                v if v < 0 => format!("{} saturation mode A", pct(v)),
                v => format!("{} saturation mode B", pct(v)),
            },
            B_ATT | B_REL2 if v == 0 => "Default".into(),
            B_ATT_STAGES | B_REL_STAGES => v.to_string(),
            B_RMS => if v == 0 { "PEAK" } else { "RMS" }.into(),
            _ => format!("{:.2}ms", ms(k, v)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texts FL's hints show for the factory default and for unusual values (Lemon Studio's
    /// plugins/lemon-press.md §3).
    #[test]
    fn texts_match_fls_hints() {
        let b = |k| band(MASTER, k);
        for (k, v, t) in [
            (P_LOW_FREQ, 19800, "200Hz"),
            (P_HIGH_FREQ, 48065, "3000Hz"),
            (P_LOW_FREQ, 0, "30Hz"),
            (P_HIGH_FREQ, 65536, "16000Hz"),
            (P_LOW_CUT, 32768, "20Hz"),
            (P_LOW_CUT, -1, "Off"),
            (P_LMH_DELAY, 1115, "2.00ms"),
            (P_LMH_MIX, 1000, "100%"),
            (b(B_PRE), 1000, "0.0dB 1.00"),
            (b(B_PRE), 0, "-INFdB 0.00"),
            (b(B_POST), 2000, "18.1dB 8.00"),
            (b(B_CEIL), 1500, "9.3dB 2.92"),
            (b(B_SEP), 500, "50% merged"),
            (b(B_SEP), -1000, "100% separated"),
            (b(B_THRES), -500, "50% saturation mode A"),
            (b(B_THRES), 1000, "100% saturation mode B"),
            (b(B_ATT), 0, "Default"),
            (b(B_REL), 5000, "85.53ms"),
            (b(B_REL2), 10000, "2000.00ms"),
            (b(B_WINDOW), 2882, "10.00ms"),
            (b(B_MODE), 1, "compression off"),
            (b(B_RMS), 1, "RMS"),
            (OVERSAMPLING, 2, "4x"),
        ] {
            assert_eq!(value_text(k, v), t, "{k} at {v}");
        }
        for (k, p) in PARAMS.iter().enumerate() {
            assert!(!p.name.is_empty(), "{k}");
        }
        assert_eq!(PARAMS[band(HIGH, B_WINDOW)].name, "HIGH Sustain window");
        assert_eq!(PARAMS[band(MID, B_PRE)].default, 1000.0);
        assert!(!PARAMS[OVERSAMPLING].live);
    }
}
