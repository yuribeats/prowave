//! The Pitch Editor's pitch tracker: probabilistic YIN (pYIN, Mauch and Dixon, 2014). Every
//! 5 ms the YIN difference function gives a few candidate periods (its dips); a spread of
//! thresholds (a Beta(2, 18) distribution) turns them into probabilities, and a hidden Markov
//! model with a voiced state per tenth of a semitone and one unvoiced state picks the most
//! likely path through them (Viterbi). The result is a pitch curve that does not jump an
//! octave on a weak frame, with a voicing probability for every frame.
//!
//! Offline only; nothing here runs on the audio thread.
use crate::spectral::Fft;

/// Seconds between frames (WORLD's usual frame period, so its analysis lines up).
pub const PITCH_HOP: f64 = 0.005;
/// The lowest and highest pitch looked for, in Hz.
pub const LOWEST_HZ: f64 = 55.0;
pub const HIGHEST_HZ: f64 = 1400.0;
/// The size of a pitch state, in cents.
const BIN_CENTS: f64 = 10.0;
/// How far the pitch may move in one frame without going through the unvoiced state, in bins.
const MAX_STEP: usize = 25;
/// Frames quieter than this (dBFS) are not looked at: silence.
const SILENCE_DB: f32 = -60.0;
/// The longest run of frames decoded at once (longer runs are cut at their weakest frame).
const MAX_CHUNK: usize = 2000;

/// The pitch of a recording, a frame every [`PITCH_HOP`] seconds, frame `i` centred on
/// `i * PITCH_HOP`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PitchTrack {
    /// Hz, 0 where there is no pitch.
    pub hz: Vec<f32>,
    /// How likely the frame has a pitch at all, 0 to 1.
    pub voicing: Vec<f32>,
    /// The loudness of the frame, in dBFS.
    pub level_db: Vec<f32>,
}

impl PitchTrack {
    pub fn len(&self) -> usize {
        self.hz.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hz.is_empty()
    }

    /// The pitch of frame `i` as a MIDI note with its fraction, if it has one.
    pub fn midi(&self, i: usize) -> Option<f32> {
        self.hz
            .get(i)
            .filter(|h| **h > 0.0)
            .map(|h| 69.0 + 12.0 * (h / 440.0).log2())
    }

    /// The frame nearest `secs`.
    pub fn frame_at(secs: f64) -> usize {
        (secs.max(0.0) / PITCH_HOP).round() as usize
    }

    pub fn time_of(frame: usize) -> f64 {
        frame as f64 * PITCH_HOP
    }
}

/// A dip of the difference function: its period in samples (with a fraction) and depth.
#[derive(Clone, Copy)]
struct Dip {
    period: f64,
    value: f32,
}

/// How readily the tracker calls a frame voiced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackerSettings {
    /// The mean of the Beta(2, b) distribution of YIN thresholds (pYIN: 0.1, 0.15 or 0.2).
    /// Higher hears breathier sound as pitched.
    pub threshold_mean: f64,
    /// The weight the deepest dip gets when no dip is under a threshold (pYIN: 0.01).
    pub no_dip_weight: f32,
    /// A dip gives way to one at twice its period that is under `octave_ratio` times as deep
    /// and deeper by more than `octave_margin` (so two near-perfect dips of a clean tone do
    /// not make an octave-down error). A ratio of 0 turns the check off.
    pub octave_ratio: f32,
    pub octave_margin: f32,
}

impl Default for TrackerSettings {
    fn default() -> Self {
        DEFAULT_SETTINGS
    }
}

/// The settings [`track_pitch`] uses (chosen on re-sung real vocals with known notes, see
/// `examples/note_render_eval.rs`).
pub const DEFAULT_SETTINGS: TrackerSettings = TrackerSettings {
    threshold_mean: 0.2,
    no_dip_weight: 0.3,
    octave_ratio: 0.5,
    octave_margin: 0.005,
};

/// The thresholds and their weights: Beta(2, b) over 0.01 … 1.00, its mean 2 / (2 + b).
fn beta_weights(mean: f64) -> Vec<(f32, f32)> {
    let b = (2.0 / mean.clamp(0.02, 0.5) - 2.0).max(1.0);
    let raw: Vec<(f32, f64)> = (1..=100)
        .map(|k| {
            let s = f64::from(k) / 100.0;
            (s as f32, s * (1.0 - s).powf(b - 1.0))
        })
        .collect();
    let total: f64 = raw.iter().map(|r| r.1).sum();
    raw.into_iter()
        .map(|(s, w)| (s, (w / total) as f32))
        .collect()
}

/// A low-passed copy of `x` at a `step`-th of the rate (a windowed-sinc filter).
fn decimate(x: &[f32], step: usize) -> Vec<f32> {
    if step <= 1 {
        return x
            .iter()
            .map(|v| if v.is_finite() { *v } else { 0.0 })
            .collect();
    }
    let half = 4 * step;
    let cutoff = 0.45 / step as f64;
    let taps: Vec<f32> = (0..=2 * half)
        .map(|i| {
            let n = i as f64 - half as f64;
            let sinc = if n == 0.0 {
                2.0 * cutoff
            } else {
                (std::f64::consts::TAU * cutoff * n).sin() / (std::f64::consts::PI * n)
            };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * n / (half as f64 + 1.0)).cos();
            (sinc * w) as f32
        })
        .collect();
    let n_out = x.len() / step;
    (0..n_out)
        .map(|o| {
            let c = (o * step) as isize;
            let mut s = 0.0f32;
            for (k, t) in taps.iter().enumerate() {
                let i = c + k as isize - half as isize;
                if i >= 0 && (i as usize) < x.len() {
                    let v = x[i as usize];
                    if v.is_finite() {
                        s += v * t;
                    }
                }
            }
            s
        })
        .collect()
}

/// The pitch of a mono recording (pYIN).
pub fn track_pitch(mono: &[f32], sample_rate: u32) -> PitchTrack {
    track_pitch_with(mono, sample_rate, DEFAULT_SETTINGS)
}

/// [`track_pitch`] with other settings.
pub fn track_pitch_with(mono: &[f32], sample_rate: u32, settings: TrackerSettings) -> PitchTrack {
    let sr = f64::from(sample_rate.max(1));
    let step = ((sr / 14_000.0).floor() as usize).max(1);
    let rate = sr / step as f64;
    let x = decimate(mono, step);
    let frames = (mono.len() as f64 / sr / PITCH_HOP).floor() as usize + 1;
    if mono.is_empty() || rate < 2.0 * HIGHEST_HZ {
        return PitchTrack::default();
    }
    let max_tau = (rate / LOWEST_HZ).ceil() as usize;
    let min_tau = ((rate / HIGHEST_HZ).floor() as usize).max(2);
    let w = ((0.025 * rate) as usize).max(max_tau / 2 + 8);
    let span = w + max_tau + 2;
    let fft = Fft::new(span + w);
    let n = fft.len();
    let weights = beta_weights(settings.threshold_mean);
    // Squares, summed, for the energy terms of the difference function.
    let mut prefix = vec![0.0f64; x.len() + 1];
    for (i, v) in x.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(*v) * f64::from(*v);
    }
    let energy = |a: isize, b: isize| -> f64 {
        let a = a.clamp(0, x.len() as isize) as usize;
        let b = b.clamp(0, x.len() as isize) as usize;
        prefix[b] - prefix[a]
    };
    let nb = bins();
    let mut cands: Vec<Vec<(usize, f32, f32)>> = Vec::with_capacity(frames);
    let mut level_db = Vec::with_capacity(frames);
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    let (mut re2, mut im2) = (vec![0.0f32; n], vec![0.0f32; n]);
    let mut d = vec![0.0f32; max_tau + 2];
    let mut cm = vec![1.0f32; max_tau + 2];
    for f in 0..frames {
        let centre = (f as f64 * PITCH_HOP * rate) as isize;
        let start = centre - (span / 2) as isize;
        let at = |i: usize| -> f32 {
            let j = start + i as isize;
            if j >= 0 && (j as usize) < x.len() {
                x[j as usize]
            } else {
                0.0
            }
        };
        let e0 = energy(start, start + w as isize);
        let rms = (e0 / w as f64).sqrt() as f32;
        let db = 20.0 * rms.max(1e-9).log10();
        level_db.push(db);
        if db < SILENCE_DB {
            cands.push(Vec::new());
            continue;
        }
        // r(τ) = Σ x[j] x[j + τ] over the window, by FFT.
        for i in 0..n {
            re[i] = if i < span { at(i) } else { 0.0 };
            im[i] = 0.0;
            re2[i] = if i < w { at(i) } else { 0.0 };
            im2[i] = 0.0;
        }
        fft.process(&mut re, &mut im, false);
        fft.process(&mut re2, &mut im2, false);
        for i in 0..n {
            // X · conj(Y)
            let (a, b, c, e) = (re[i], im[i], re2[i], -im2[i]);
            re[i] = a * c - b * e;
            im[i] = a * e + b * c;
        }
        fft.process(&mut re, &mut im, true);
        let scale = 1.0 / n as f32;
        let mut running = 0.0f32;
        cm[0] = 1.0;
        for tau in 1..=max_tau.min(d.len() - 2) {
            let et = energy(start + tau as isize, start + (tau + w) as isize);
            let v = (e0 + et - 2.0 * f64::from(re[tau] * scale)).max(0.0) as f32;
            d[tau] = v;
            running += v;
            cm[tau] = if running > 0.0 {
                v * tau as f32 / running
            } else {
                1.0
            };
        }
        // The dips, in order of period.
        let mut dips: Vec<Dip> = Vec::new();
        for tau in min_tau.max(1)..max_tau.min(cm.len() - 2) {
            if cm[tau] < cm[tau - 1] && cm[tau] <= cm[tau + 1] {
                let (a, b, c) = (cm[tau - 1], cm[tau], cm[tau + 1]);
                let den = a - 2.0 * b + c;
                let shift = if den.abs() > 1e-9 {
                    (0.5 * (a - c) / den).clamp(-0.5, 0.5)
                } else {
                    0.0
                };
                dips.push(Dip {
                    period: tau as f64 + f64::from(shift),
                    value: b - 0.25 * (a - c) * shift,
                });
            }
        }
        // A weak fundamental makes a dip at half the period that passes a threshold before
        // the true period's (YIN's octave-up error): a dip at twice the period that is clearly
        // deeper is the pitch instead.
        let resolve = |i: usize| -> usize {
            let d = dips[i];
            dips.iter()
                .enumerate()
                .skip(i + 1)
                .find(|(_, e)| {
                    (e.period / d.period - 2.0).abs() < 0.06
                        && e.value < settings.octave_ratio * d.value
                        && d.value - e.value > settings.octave_margin
                })
                .map_or(i, |(j, _)| j)
        };
        let mut probs = vec![0.0f32; dips.len()];
        let lowest = dips
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.value.total_cmp(&b.1.value))
            .map(|(i, _)| i);
        for (s, wgt) in &weights {
            match dips.iter().position(|dp| dp.value < *s) {
                Some(i) => probs[resolve(i)] += wgt,
                // No dip under the threshold: the deepest one, as pYIN does, with a small
                // weight.
                None => {
                    if let Some(i) = lowest {
                        probs[i] += settings.no_dip_weight * wgt;
                    }
                }
            }
        }
        let mut c: Vec<(usize, f32, f32)> = Vec::new();
        for (dp, p) in dips.iter().zip(&probs) {
            if *p <= 1e-4 {
                continue;
            }
            let hz = rate / dp.period;
            if !(LOWEST_HZ..=HIGHEST_HZ).contains(&hz) {
                continue;
            }
            let b = bin_of(hz).min(nb - 1);
            c.push((b, *p, hz as f32));
        }
        cands.push(c);
    }
    let path = viterbi(&cands, nb);
    let mut raw = vec![0.0f32; frames];
    let mut voicing = vec![0.0f32; frames];
    for f in 0..frames {
        voicing[f] = cands[f].iter().map(|c| c.1).sum::<f32>().min(1.0);
        if let Some(b) = path[f] {
            // The candidate nearest the chosen state gives the exact pitch.
            raw[f] = cands[f]
                .iter()
                .filter(|c| c.0.abs_diff(b) <= 3)
                .min_by_key(|c| c.0.abs_diff(b))
                .map_or_else(|| hz_of_bin(b) as f32, |c| c.2);
        }
    }
    // A frame compares x[j] with x[j + τ] for j over the window: what it measures is centred
    // at the window's start plus (W + τ) / 2, not at the frame's time. Each pitch is put back
    // at its own time and the frames read off between them (fast vibrato would lag otherwise).
    let measured = |f: usize| -> f64 {
        let centre = f as f64 * PITCH_HOP;
        let tau = rate / f64::from(raw[f]);
        centre + ((w as f64 + tau) / 2.0 - (span / 2) as f64) / rate
    };
    let mut hz = vec![0.0f32; frames];
    for f in 0..frames {
        if raw[f] <= 0.0 {
            continue;
        }
        let t = f as f64 * PITCH_HOP;
        // The voiced neighbours whose measured times are either side of t.
        let mut k = f;
        while k + 1 < frames && raw[k + 1] > 0.0 && measured(k) < t {
            k += 1;
        }
        while k > 0 && raw[k - 1] > 0.0 && measured(k) > t {
            k -= 1;
        }
        let (a, b) = if measured(k) <= t {
            (
                k,
                if k + 1 < frames && raw[k + 1] > 0.0 {
                    k + 1
                } else {
                    k
                },
            )
        } else {
            (if k > 0 && raw[k - 1] > 0.0 { k - 1 } else { k }, k)
        };
        hz[f] = if a == b {
            raw[a]
        } else {
            let (ta, tb) = (measured(a), measured(b));
            let x = ((t - ta) / (tb - ta)).clamp(0.0, 1.0);
            // In log frequency, as pitch is heard.
            (f64::from(raw[a]).ln() * (1.0 - x) + f64::from(raw[b]).ln() * x).exp() as f32
        };
    }
    PitchTrack {
        hz,
        voicing,
        level_db,
    }
}

fn bins() -> usize {
    (1200.0 * (HIGHEST_HZ / LOWEST_HZ).log2() / BIN_CENTS).ceil() as usize + 1
}

fn bin_of(hz: f64) -> usize {
    (1200.0 * (hz / LOWEST_HZ).log2() / BIN_CENTS)
        .round()
        .max(0.0) as usize
}

fn hz_of_bin(b: usize) -> f64 {
    LOWEST_HZ * 2f64.powf(b as f64 * BIN_CENTS / 1200.0)
}

/// The most likely pitch state of each frame (`None`: unvoiced). The frames are decoded in
/// runs cut at silence, so the back-pointers of a long recording stay small.
fn viterbi(cands: &[Vec<(usize, f32, f32)>], nb: usize) -> Vec<Option<usize>> {
    let mut out = vec![None; cands.len()];
    let mut start = 0;
    while start < cands.len() {
        // Skip frames with nothing in them.
        while start < cands.len() && cands[start].is_empty() {
            start += 1;
        }
        if start >= cands.len() {
            break;
        }
        // The run goes to the next 50 ms without candidates, or is cut near MAX_CHUNK.
        let mut end = start;
        let mut empty = 0;
        while end < cands.len() && empty < 10 && end - start < MAX_CHUNK {
            if cands[end].is_empty() {
                empty += 1;
            } else {
                empty = 0;
            }
            end += 1;
        }
        if end - start >= MAX_CHUNK {
            // Cut at the least voiced frame of the last fifth instead of mid-note.
            let from = start + MAX_CHUNK * 4 / 5;
            if let Some(cut) = (from..end).min_by(|a, b| {
                let v = |i: usize| cands[i].iter().map(|c| c.1).sum::<f32>();
                v(*a).total_cmp(&v(*b))
            }) {
                end = cut.max(start + 1);
            }
        }
        decode(&cands[start..end], nb, &mut out[start..end]);
        start = end;
    }
    out
}

fn decode(cands: &[Vec<(usize, f32, f32)>], nb: usize, out: &mut [Option<usize>]) {
    let n = cands.len();
    if n == 0 {
        return;
    }
    let unv = nb; // the unvoiced state
    let states = nb + 1;
    // Transition weights: a triangle over ±MAX_STEP bins, staying voiced with 0.99.
    let tri: Vec<f32> = {
        let raw: Vec<f32> = (0..=MAX_STEP).map(|d| (MAX_STEP + 1 - d) as f32).collect();
        let total: f32 = raw[0] + 2.0 * raw[1..].iter().sum::<f32>();
        raw.iter().map(|r| (0.99 * r / total).ln()).collect()
    };
    let to_unvoiced = 0.01f32.ln();
    let stay_unvoiced = 0.99f32.ln();
    let from_unvoiced = (0.01 / nb as f32).ln();
    let floor = 1e-12f32;
    let obs = |f: usize, out: &mut Vec<f32>| {
        out.clear();
        out.resize(states, floor.ln());
        let total: f32 = cands[f].iter().map(|c| c.1).sum::<f32>().min(1.0);
        for c in &cands[f] {
            out[c.0] = (out[c.0].exp() + c.1).ln();
        }
        out[unv] = (1.0 - total).max(floor).ln();
    };
    let mut ob = Vec::with_capacity(states);
    obs(0, &mut ob);
    let mut delta: Vec<f32> = (0..states)
        .map(|s| {
            let prior = if s == unv { 0.5f32 } else { 0.5 / nb as f32 };
            prior.ln() + ob[s]
        })
        .collect();
    let mut back: Vec<u16> = vec![0; n * states];
    let mut next = vec![0.0f32; states];
    for f in 1..n {
        obs(f, &mut ob);
        for b in 0..nb {
            let lo = b.saturating_sub(MAX_STEP);
            let hi = (b + MAX_STEP).min(nb - 1);
            let mut best = (delta[unv] + from_unvoiced, unv);
            for p in lo..=hi {
                let v = delta[p] + tri[p.abs_diff(b)];
                if v > best.0 {
                    best = (v, p);
                }
            }
            next[b] = best.0 + ob[b];
            back[f * states + b] = best.1 as u16;
        }
        let mut best = (delta[unv] + stay_unvoiced, unv);
        for (p, v) in delta.iter().enumerate().take(nb) {
            if v + to_unvoiced > best.0 {
                best = (v + to_unvoiced, p);
            }
        }
        next[unv] = best.0 + ob[unv];
        back[f * states + unv] = best.1 as u16;
        // Keep the numbers in range.
        let top = next.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for (d, v) in delta.iter_mut().zip(&next) {
            *d = v - top;
        }
    }
    let mut s = delta
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(unv, |(i, _)| i);
    for f in (0..n).rev() {
        out[f] = (s != unv).then_some(s);
        if f > 0 {
            s = usize::from(back[f * states + s]);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    /// A tone following `hz(t)`, with some harmonics (closer to a voice than a sine).
    pub(crate) fn tone(secs: f64, hz: impl Fn(f64) -> f64) -> Vec<f32> {
        let n = (secs * f64::from(SR)) as usize;
        let mut phase = 0.0f64;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(SR);
                phase += std::f64::consts::TAU * hz(t) / f64::from(SR);
                let s = phase.sin() + 0.5 * (2.0 * phase).sin() + 0.25 * (3.0 * phase).sin();
                (s * 0.3) as f32
            })
            .collect()
    }

    fn cents(a: f64, b: f64) -> f64 {
        1200.0 * (a / b).log2()
    }

    #[test]
    fn steady_pitches_are_found_to_a_few_cents() {
        for hz in [82.4, 110.0, 196.0, 440.0, 880.0] {
            let t = track_pitch(&tone(0.5, |_| hz), SR);
            let mid: Vec<f32> = t.hz[20..t.len() - 20].to_vec();
            assert!(mid.iter().all(|h| *h > 0.0), "{hz}: unvoiced frames");
            for h in &mid {
                assert!(cents(f64::from(*h), hz).abs() < 5.0, "{hz}: {h}");
            }
            assert!(t.voicing[50] > 0.5);
        }
    }

    #[test]
    fn a_glide_and_a_vibrato_are_followed() {
        // An octave glide up over a second: 220 → 440 Hz.
        let glide = |t: f64| 220.0 * 2f64.powf(t);
        let t = track_pitch(&tone(1.0, glide), SR);
        let mut worst = 0.0f64;
        for f in 10..t.len() - 10 {
            worst = worst.max(cents(f64::from(t.hz[f]), glide(PitchTrack::time_of(f))).abs());
        }
        assert!(worst < 15.0, "glide off by {worst} cents");
        // A 5.5 Hz vibrato of ±60 cents around A3.
        let vib = |t: f64| 220.0 * 2f64.powf(0.6 / 12.0 * (std::f64::consts::TAU * 5.5 * t).sin());
        let t = track_pitch(&tone(1.0, vib), SR);
        let mut worst = 0.0f64;
        for f in 10..t.len() - 10 {
            worst = worst.max(cents(f64::from(t.hz[f]), vib(PitchTrack::time_of(f))).abs());
        }
        assert!(worst < 15.0, "vibrato off by {worst} cents");
    }

    /// A weak fundamental under strong second and third harmonics (a dark voice, a stem with
    /// its low end taken out) is still heard at the fundamental, not an octave up.
    #[test]
    fn a_weak_fundamental_is_not_an_octave_error() {
        for hz in [110.0f64, 196.0, 330.0] {
            let n = SR as usize / 2;
            let x: Vec<f32> = (0..n)
                .map(|i| {
                    let p = std::f64::consts::TAU * hz * i as f64 / f64::from(SR);
                    (0.05 * p.sin()
                        + 0.6 * (2.0 * p).sin()
                        + 0.3 * (3.0 * p).sin()
                        + 0.4 * (4.0 * p).sin()) as f32
                        * 0.4
                })
                .collect();
            let t = track_pitch(&x, SR);
            let mut v: Vec<f32> = t.hz[20..t.len() - 20]
                .iter()
                .copied()
                .filter(|h| *h > 0.0)
                .collect();
            assert!(v.len() > 40, "{hz}: unvoiced");
            v.sort_by(f32::total_cmp);
            let m = f64::from(v[v.len() / 2]);
            assert!(cents(m, hz).abs() < 10.0, "{hz}: heard at {m}");
        }
    }

    #[test]
    fn silence_and_noise_have_no_pitch() {
        let t = track_pitch(&vec![0.0; 22_050], SR);
        assert!(t.hz.iter().all(|h| *h == 0.0));
        let mut seed = 1u32;
        let noise: Vec<f32> = (0..22_050)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect();
        let t = track_pitch(&noise, SR);
        let voiced = t.hz.iter().filter(|h| **h > 0.0).count();
        assert!(voiced < t.len() / 5, "{voiced} of {} voiced", t.len());
        assert!(track_pitch(&[], SR).is_empty());
        // Frames line up with time: frame i is at i · 5 ms.
        assert_eq!(PitchTrack::frame_at(0.1), 20);
        assert!((PitchTrack::time_of(20) - 0.1).abs() < 1e-12);
    }
}
