// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Pro Wave contributors. The effects, their parameter tables and value texts
// are ported from Lemon Studio (Copyright (C) 2026 ItsAlphaNeon, GPL-3.0-or-later,
// https://github.com/ItsAlphaNeon/Lemon-Studio); see each effect's file.

//! Native effects: FL Studio-style dynamics and drum processing ported from Lemon Studio, run
//! inside the engine instead of as Audio Units.
//!
//! Logic Pro equivalent: none directly — these are the FL Studio Fruity Limiter (limiter,
//! compressor, noise gate), Transient Processor, Fruity Soft Clipper, Fruity Fast Dist and
//! Maximus recreations from Lemon Studio, offered next to Logic's own Compressor and Limiter.
//!
//! Each effect is listed in [`native_effects`] with its parameters, and made with
//! [`open_native`] on a non-real-time thread (all of its memory is allocated there). It is then
//! a [`BlockProcessor`] that can sit in a [`crate::PluginTrack`]'s effect chain; `process` and
//! [`NativeFx::set_param`] never allocate, lock or panic.
//!
//! Parameter values are FL's own stored knob values (the numbers FL and Lemon Studio keep in
//! a project), with FL's ranges and defaults, so the knob laws are exactly Lemon's. They are
//! not in dB or ms: [`NativeFxInfo::value_text`] gives the text FL shows for a value ("-8.7dB
//! 0.37", "2.00ms", "4.0:1", ...); [`NativeParam::unit`] names the unit of that text.

pub mod lemon_fast_distortion;
pub mod lemon_limiter;
pub mod lemon_soft_clipper;
pub mod lemon_transient_processor;
mod level_curve;
pub mod press;
pub mod tune;

use crate::plugin_track::BlockProcessor;

/// One parameter of a native effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeParam {
    /// The name FL's hint shows.
    pub name: &'static str,
    /// The stored value's range and FL's default (a fresh instance).
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// The unit of the parameter's value text (`value_text`), not of the stored value.
    pub unit: &'static str,
    /// For a choice: the names of the values `min`, `min + 1`, ... (empty for knobs).
    pub choices: &'static [&'static str],
    /// The values are whole numbers (all of Lemon's parameters are).
    pub integer: bool,
    /// The parameter can change while the effect runs. `false`: it sizes the effect's
    /// memory, so `set_param` can only lower it below the value the effect was opened with;
    /// open the effect again to raise it.
    pub live: bool,
}

impl NativeParam {
    /// A whole-number knob.
    pub(crate) const fn knob(
        name: &'static str,
        min: i32,
        max: i32,
        default: i32,
        unit: &'static str,
    ) -> Self {
        Self {
            name,
            min: min as f32,
            max: max as f32,
            default: default as f32,
            unit,
            choices: &[],
            integer: true,
            live: true,
        }
    }

    /// A choice of named values from `min`.
    pub(crate) const fn choice(
        name: &'static str,
        min: i32,
        default: i32,
        choices: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            min: min as f32,
            max: (min + choices.len() as i32 - 1) as f32,
            default: default as f32,
            unit: "",
            choices,
            integer: true,
            live: true,
        }
    }

    /// `value` as a stored value: rounded and kept in the range; not a number gives the default.
    pub fn stored(&self, value: f32) -> i32 {
        let v = if value.is_finite() {
            value
        } else {
            self.default
        };
        v.clamp(self.min, self.max).round() as i32
    }
}

/// A preset of a native effect: the parameters it sets (the others are left as they are).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativePreset {
    pub name: &'static str,
    /// (parameter index, stored value).
    pub values: &'static [(usize, f32)],
}

impl NativePreset {
    /// `values` with this preset applied.
    pub fn apply(&self, values: &mut [f32]) {
        for (k, v) in self.values {
            if let Some(x) = values.get_mut(*k) {
                *x = *v;
            }
        }
    }
}

/// A native effect's entry in the menu.
#[derive(Clone, Copy, Debug)]
pub struct NativeFxInfo {
    /// Stable id saved in projects, e.g. `"prowave:lemon-limiter"`.
    pub id: &'static str,
    /// The name Lemon Studio gives it.
    pub name: &'static str,
    pub params: &'static [NativeParam],
    /// Its presets (none for the Lemon ports, whose presets are FL's).
    pub presets: &'static [NativePreset],
    text: fn(usize, i32) -> String,
}

impl NativeFxInfo {
    /// The text FL shows for parameter `index` at `value` (not for the audio thread: it
    /// allocates).
    pub fn value_text(&self, index: usize, value: f32) -> String {
        match self.params.get(index) {
            Some(p) => (self.text)(index, p.stored(value)),
            None => String::new(),
        }
    }

    /// Every parameter's default.
    pub fn defaults(&self) -> Vec<f32> {
        self.params.iter().map(|p| p.default).collect()
    }
}

/// A native effect: a [`BlockProcessor`] whose parameters can be read and set.
pub trait NativeFx: BlockProcessor {
    /// Sets parameter `index` (a stored value, rounded and kept in range). Real-time safe: no
    /// allocation, no locks. Unknown indices are ignored.
    fn set_param(&mut self, index: usize, value: f32);
    /// Parameter `index`'s value in use (0 for an unknown index).
    fn param(&self, index: usize) -> f32;
    /// How far the effect is turning the signal down, in dB (positive), as of its last block.
    fn gain_reduction_db(&self) -> f32 {
        0.0
    }
}

static EFFECTS: [NativeFxInfo; 6] = [
    NativeFxInfo {
        id: lemon_limiter::ID,
        name: lemon_limiter::NAME,
        params: &lemon_limiter::PARAMS,
        presets: &[],
        text: lemon_limiter::value_text,
    },
    NativeFxInfo {
        id: press::ID,
        name: press::NAME,
        params: &press::PARAMS,
        presets: &[],
        text: press::value_text,
    },
    NativeFxInfo {
        id: lemon_transient_processor::ID,
        name: lemon_transient_processor::NAME,
        params: &lemon_transient_processor::PARAMS,
        presets: &[],
        text: lemon_transient_processor::value_text,
    },
    NativeFxInfo {
        id: lemon_soft_clipper::ID,
        name: lemon_soft_clipper::NAME,
        params: &lemon_soft_clipper::PARAMS,
        presets: &[],
        text: lemon_soft_clipper::value_text,
    },
    NativeFxInfo {
        id: lemon_fast_distortion::ID,
        name: lemon_fast_distortion::NAME,
        params: &lemon_fast_distortion::PARAMS,
        presets: &[],
        text: lemon_fast_distortion::value_text,
    },
    NativeFxInfo {
        id: tune::ID,
        name: tune::NAME,
        params: &tune::PARAMS,
        presets: &tune::PRESETS,
        text: tune::value_text,
    },
];

/// A native effect in a track's effect chain.
struct InChain(Box<dyn NativeFx>);

impl BlockProcessor for InChain {
    fn midi(&mut self, status: u8, data1: u8, data2: u8, offset: u32) {
        self.0.midi(status, data1, data2, offset);
    }
    fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        self.0.process(input, out);
    }
    fn latency_seconds(&self) -> f64 {
        self.0.latency_seconds()
    }
    fn set_parameter(&mut self, index: usize, value: f32) {
        self.0.set_param(index, value);
    }
    fn block_transport(&mut self, t: &crate::plugin_track::BlockTransport) {
        self.0.block_transport(t);
    }
}

/// Pro Wave Tune, ready for a [`crate::PluginTrack`]'s effect chain, and the app's end of it
/// (the project's key, tuning and MIDI notes go in there; the meters come out).
pub fn open_tune_block(
    sample_rate: u32,
    values: &[f32],
) -> (Box<dyn BlockProcessor>, tune::TuneRemote) {
    let (fx, remote) = tune::open_tune(sample_rate.max(1), values);
    (Box::new(InChain(Box::new(fx))), remote)
}

/// [`open_native`], ready for a [`crate::PluginTrack`]'s effect chain.
pub fn open_native_block(
    id: &str,
    sample_rate: u32,
    values: &[f32],
) -> Option<Box<dyn BlockProcessor>> {
    open_native(id, sample_rate, values).map(|fx| Box::new(InChain(fx)) as Box<dyn BlockProcessor>)
}

/// Every native effect, in menu order.
pub fn native_effects() -> &'static [NativeFxInfo] {
    &EFFECTS
}

/// The entry for `id`.
pub fn native_info(id: &str) -> Option<&'static NativeFxInfo> {
    EFFECTS.iter().find(|e| e.id == id)
}

/// The stored values for `params` from `values` (missing ones are the defaults).
pub(crate) fn stored_values<const N: usize>(params: &[NativeParam; N], values: &[f32]) -> [i32; N] {
    std::array::from_fn(|k| {
        let p = &params[k];
        p.stored(values.get(k).copied().unwrap_or(p.default))
    })
}

/// Makes the effect `id` at `sample_rate` with parameter `values` (missing ones take their
/// defaults). Allocates: call it off the audio thread. `None` for an unknown id.
pub fn open_native(id: &str, sample_rate: u32, values: &[f32]) -> Option<Box<dyn NativeFx>> {
    let rate = sample_rate.max(1);
    Some(match id {
        lemon_limiter::ID => Box::new(lemon_limiter::LemonLimiter::new(rate, values)),
        press::ID => Box::new(press::Press::new(rate, values)),
        lemon_transient_processor::ID => Box::new(
            lemon_transient_processor::LemonTransientProcessor::new(rate, values),
        ),
        lemon_soft_clipper::ID => Box::new(lemon_soft_clipper::LemonSoftClipper::new(rate, values)),
        lemon_fast_distortion::ID => Box::new(lemon_fast_distortion::LemonFastDistortion::new(
            rate, values,
        )),
        tune::ID => Box::new(tune::ProWaveTune::new(rate, values)),
        _ => return None,
    })
}

/// Copies `input` into `out` and returns the frames both sides of both hold: effects then work
/// in place on `out`.
pub(crate) fn copy_in(input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) -> usize {
    let n = input
        .0
        .len()
        .min(input.1.len())
        .min(out.0.len())
        .min(out.1.len());
    out.0[..n].copy_from_slice(&input.0[..n]);
    out.1[..n].copy_from_slice(&input.1[..n]);
    n
}

/// Implements [`BlockProcessor`] for an effect with `fn run(&mut self, l: &mut [f32], r: &mut
/// [f32])` (in place) and `fn latency_samples(&self) -> usize`.
macro_rules! block_effect {
    ($t:ty) => {
        impl $crate::plugin_track::BlockProcessor for $t {
            fn midi(&mut self, _status: u8, _data1: u8, _data2: u8, _offset: u32) {}
            fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
                let n = $crate::native_fx::copy_in(input, (&mut *out.0, &mut *out.1));
                self.run(&mut out.0[..n], &mut out.1[..n]);
            }
            fn latency_seconds(&self) -> f64 {
                self.latency_samples() as f64 / f64::from(self.rate)
            }
        }
    };
}
pub(crate) use block_effect;

#[cfg(test)]
pub(crate) mod test_signals {
    /// A sine of `n` samples at `hz`.
    pub fn sine(n: usize, amp: f32, hz: f64, rate: f64) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (i as f64 * std::f64::consts::TAU * hz / rate).sin() as f32)
            .collect()
    }

    /// Deterministic white noise in -amp..amp.
    pub fn noise(n: usize, amp: f32, seed: u32) -> Vec<f32> {
        let mut x = seed.max(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    /// Runs `fx` on `l`/`r` in blocks of `block`, each block inside `assert_no_alloc`.
    pub fn run_blocks(
        fx: &mut dyn super::NativeFx,
        l: &[f32],
        r: &[f32],
        block: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut ol = vec![0.0; l.len()];
        let mut or = vec![0.0; r.len()];
        for at in (0..l.len()).step_by(block) {
            let end = (at + block).min(l.len());
            let (il, ir) = (&l[at..end], &r[at..end]);
            let (bl, br) = (&mut ol[at..end], &mut or[at..end]);
            assert_no_alloc::assert_no_alloc(|| fx.process((il, ir), (bl, br)));
        }
        (ol, or)
    }

    pub fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    pub fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64)
            .sqrt() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::test_signals::{noise, run_blocks};
    use super::*;

    #[test]
    fn every_effect_opens_with_its_defaults_and_ids_are_unique() {
        for (i, e) in native_effects().iter().enumerate() {
            assert!(e.id.starts_with("prowave:"), "{}", e.id);
            assert!(native_effects()[i + 1..].iter().all(|o| o.id != e.id));
            let Some(fx) = open_native(e.id, 48000, &[]) else {
                panic!("{} does not open", e.id)
            };
            for (k, p) in e.params.iter().enumerate() {
                assert!(
                    p.min <= p.default && p.default <= p.max,
                    "{} {}",
                    e.id,
                    p.name
                );
                assert_eq!(fx.param(k), p.default, "{} {}", e.id, p.name);
                assert!(
                    !e.value_text(k, p.default).is_empty(),
                    "{} {}",
                    e.id,
                    p.name
                );
            }
        }
        assert!(open_native("prowave:nothing", 48000, &[]).is_none());
    }

    /// Silence and one second of loud noise give finite output from every effect, at every
    /// parameter's minimum, default and maximum, with no allocation while processing or
    /// setting parameters.
    #[test]
    fn no_nan_or_inf_on_silence_and_noise_and_no_allocation() {
        let rate = 44100;
        let n = rate as usize;
        let silence = vec![0.0f32; n];
        let (nl, nr) = (noise(n, 1.5, 7), noise(n, 1.5, 99));
        for e in native_effects() {
            for setting in 0..3 {
                let Some(mut fx) = open_native(e.id, rate, &[]) else {
                    panic!("{} does not open", e.id)
                };
                for (k, p) in e.params.iter().enumerate() {
                    let v = match setting {
                        0 => p.min,
                        1 => p.default,
                        _ => p.max,
                    };
                    // Memory-sizing parameters stay where the effect was opened.
                    if p.live {
                        assert_no_alloc::assert_no_alloc(|| fx.set_param(k, v));
                    }
                }
                for (l, r) in [(&silence, &silence), (&nl, &nr)] {
                    let (ol, or) = run_blocks(fx.as_mut(), l, r, 512);
                    assert!(
                        ol.iter().chain(&or).all(|x| x.is_finite()),
                        "{} setting {setting}",
                        e.id
                    );
                    assert!(fx.gain_reduction_db().is_finite());
                }
                let (ol, _) = run_blocks(fx.as_mut(), &silence, &silence, 333);
                assert!(ol.iter().all(|x| x.is_finite()));
            }
        }
    }

    #[test]
    fn values_are_stored_rounded_and_in_range() {
        let p = NativeParam::knob("x", 0, 10, 5, "");
        assert_eq!(p.stored(3.4), 3);
        assert_eq!(p.stored(-3.0), 0);
        assert_eq!(p.stored(99.0), 10);
        assert_eq!(p.stored(f32::NAN), 5);
        let c = NativeParam::choice("c", 0, 1, &["a", "b", "c"]);
        assert_eq!(c.max, 2.0);
    }

    /// A native effect sits in a track's effect chain like an Audio Unit: its look-ahead is
    /// part of the track's delay, and a loud input comes back limited a block later.
    #[test]
    fn native_effects_run_in_a_plugin_track() {
        let Some(limiter) = open_native(lemon_limiter::ID, 48000, &[]) else {
            panic!("the limiter does not open")
        };
        let look = limiter.latency_seconds();
        let mut track = crate::PluginTrack::new(None, vec![limiter as Box<dyn BlockProcessor>], 64);
        assert!(track.latency_seconds() >= look && look > 0.0);
        let mut most = 0.0f32;
        for block in 0..400 {
            for i in 0..64 {
                let x = if (block * 64 + i) % 40 < 20 {
                    4.0
                } else {
                    -4.0
                };
                let (l, r) = track.through_effects(i, x, x);
                if block > 200 {
                    most = most.max(l.abs()).max(r.abs());
                }
            }
            assert_no_alloc::assert_no_alloc(|| track.end_block(64));
        }
        assert!(most > 0.9 && most <= 1.0, "{most}");
    }
}
