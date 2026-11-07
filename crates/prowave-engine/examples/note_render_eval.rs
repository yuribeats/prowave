//! Measures the Pitch Editor: how well notes are found, and how well each resynthesis engine
//! (WORLD, PSOLA, Signalsmith Stretch, and the old Flex Pitch shift for comparison) makes the
//! same edits. Renders to files only; nothing is played.
//!
//! ```text
//! cargo run --release -p prowave-engine --example note_render_eval -- OUT_DIR [VOCAL.flac ...]
//! ```
//!
//! Edits, on every note: E1 pitch +3 semitones, E2 vibrato (modulation) ×0, E3 formants +2
//! semitones. Measured:
//! - f0 error after the edit, in cents (median and 95th percentile, voiced frames inside the
//!   notes, 30 ms in from their edges), against the asked-for curve;
//! - for the pitch-only edits, the spectral-envelope distance to the original (mel-free
//!   cepstral distance in dB over coefficients 1–24 of CheapTrick envelopes, each signal
//!   analysed at its own pitch): lower keeps the voice's formants better;
//! - for E3, the formant shift actually made (the warp that best matches the envelopes) and
//!   the pitch change (should be 0);
//! - the noise floor: harmonic-to-noise ratio (energy at the harmonics against between them),
//!   the output's minus the input's;
//! - transient smearing on plucked notes: pre-echo (energy in the 20 ms before an onset against
//!   the 20 ms after) and the 10–90 % rise time.
//!
//! Note finding: the pitch tracker against WORLD's Harvest on the real vocals (raw pitch
//! accuracy within 50 cents, voicing agreement), and notes against a known answer: each real
//! vocal resynthesized by WORLD with its pitch replaced by known notes (its own timbre,
//! consonants and breaths kept), scored like mir_eval's note transcription (onset within
//! 50 ms, pitch within 50 cents).
//!
//! Results (2026-11-06; a synthetic vowel with vibrato and drift, and 30 s of
//! three separated lead vocals from soul records; ranges are over the three vocals):
//!
//! | engine      | f0 err med / p95 (c) | envelope dist. (dB) | formant +2 made | ΔHNR (dB)   | lost voicing | plucks pre-echo / rise |
//! |-------------|----------------------|---------------------|-----------------|-------------|--------------|------------------------|
//! | WORLD       | 1.3–1.9 / 5.8–10.8   | 2.1–3.0 (syn 2.7)   | 2.0–2.1         | -3.2 … -0.3 | 3–17 %       | -42.3 dB / 4.5 ms      |
//! | PSOLA       | 1.2–2.2 / 5.4–11.1   | 2.9–3.5 (syn 6.0)   | 1.9–2.3         | -7.8 … -5.7 | 23–51 %      | -42.5 dB / 0.8 ms      |
//! | Signalsmith | 0.9–1.9 / 3.7–1201   | 5.4–6.4 (syn 9.3)   | 0.1–0.2 (syn 1.3)| -2.6 … -0.1 | 5–27 %      | -28.8 dB / 4.2 ms      |
//! | old Flex    | 1.4–2.1 / 7.5–24.8   | 9.4–11.2 (syn 14.7) | —               | -1.3 … -0.2 | 0–5 %        | -27.2 dB / 4.5 ms      |
//!
//! (pitch +3 rows; the original plucks: -42.2 dB, 0 ms.) WORLD keeps the voice's formants
//! best after a pitch move, is the only one besides PSOLA that really moves formants on their
//! own, adds the least noise of the two, and with the unvoiced sound kept from the recording
//! keeps attacks clean. PSOLA turns a third to a half of the sung frames rough enough to lose
//! their pitch; Signalsmith Stretch barely moves formants (0.2 of 2 semitones) and its
//! formant compensation leaves twice WORLD's envelope distance. So the Pitch Editor renders
//! Melodic notes with WORLD; Percussive and Universal notes (moved as a whole, often not a
//! voice) with Signalsmith Stretch.
//!
//! Note finding: synthetic melody F 1.00; re-sung real vocals F 0.86 / 0.77 / 0.85 (onset
//! error median 5–20 ms, pitch centre 1.3–1.6 cents); the tracker within 50 cents of Harvest
//! on 97.7 / 81.3 / 99.7 % of the frames both call voiced. The defaults of
//! `pitch_tracker::DEFAULT_SETTINGS` came from `SWEEP=1` (pYIN's own defaults gave F 0.53 /
//! 0.46 / 0.69 here, mostly octave-up errors and breathy notes missed).
// A measurement harness: index loops over parallel frame arrays read most plainly here.
#![allow(clippy::needless_range_loop, clippy::type_complexity)]
use prowave_engine::melody::{find_notes, Algorithm};
use prowave_engine::note_render::{render_notes, NoteChange, Resynth, SegmentCache};
use prowave_engine::pitch_tracker::{track_pitch, PitchTrack, PITCH_HOP};
use prowave_engine::{vocoder, AudioFile, Fft};
use std::path::{Path, PathBuf};
use std::time::Instant;

const SR: u32 = 44_100;

#[derive(Clone, Copy, PartialEq)]
enum Engine {
    R(Resynth),
    /// The old Flex Pitch shift (WSOLA, then resampled): moves the formants with the pitch.
    Flex,
}

impl Engine {
    fn name(self) -> &'static str {
        match self {
            Engine::R(Resynth::World) => "WORLD",
            Engine::R(Resynth::Psola) => "PSOLA",
            Engine::R(Resynth::Signalsmith) => "Signalsmith",
            Engine::Flex => "Flex (old)",
        }
    }
}

const ENGINES: [Engine; 4] = [
    Engine::R(Resynth::World),
    Engine::R(Resynth::Psola),
    Engine::R(Resynth::Signalsmith),
    Engine::Flex,
];

#[derive(Clone, Copy, PartialEq)]
enum Edit {
    Up3,
    NoVibrato,
    Formant2,
}

impl Edit {
    fn name(self) -> &'static str {
        match self {
            Edit::Up3 => "pitch +3",
            Edit::NoVibrato => "vibrato x0",
            Edit::Formant2 => "formant +2",
        }
    }

    fn change(self, start: f64, end: f64) -> NoteChange {
        let mut c = NoteChange::none(start, end);
        match self {
            Edit::Up3 => c.shift = 3.0,
            Edit::NoVibrato => c.modulation = 0.0,
            Edit::Formant2 => c.formant = 2.0,
        }
        c
    }
}

fn stereo(m: &[f32]) -> Vec<f32> {
    m.iter().flat_map(|v| [*v, *v]).collect()
}

fn mono(s: &[f32]) -> Vec<f32> {
    s.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect()
}

fn normalized(mut v: Vec<f32>, peak: f32) -> Vec<f32> {
    let p = v.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-9);
    for x in &mut v {
        *x *= peak / p;
    }
    v
}

/// A voice-like test signal: a band-limited glottal pulse train through three formants, with
/// a 5.5 Hz vibrato and a slow drift, as notes (start, end, MIDI); also returns its true f0.
fn synthetic_vocal(notes: &[(f64, f64, f64)], secs: f64) -> (Vec<f32>, Vec<f32>) {
    let fs = f64::from(SR);
    let n = (secs * fs) as usize;
    let mut phase = 0.0f64;
    let mut ys = [[0.0f64; 2]; 3];
    let mut f0 = vec![0.0f32; (secs / PITCH_HOP) as usize + 1];
    let mut out = Vec::with_capacity(n);
    let mut lp = 0.0;
    for i in 0..n {
        let t = i as f64 / fs;
        let note = notes.iter().find(|(a, b, _)| t >= *a && t < *b);
        let mut e = 0.0;
        if let Some((a, b, m)) = note {
            let m = m
                + 0.4 * (std::f64::consts::TAU * 5.5 * (t - a)).sin()
                + 0.3 * ((t - a) / (b - a) - 0.5);
            let hz = 440.0 * 2f64.powf((m - 69.0) / 12.0);
            let env = ((t - a) / 0.03).min(1.0).min((b - t) / 0.03).max(0.0);
            // Band-limited sawtooth (sum of harmonics under 8 kHz) as the glottal source.
            phase += hz / fs;
            phase -= phase.floor();
            let mut s = 0.0;
            let mut h = 1.0;
            while h * hz < 8000.0 {
                s += (std::f64::consts::TAU * h * phase).sin() / h;
                h += 1.0;
            }
            e = s * env;
        }
        // A little breath noise.
        let noise = ((i as u64).wrapping_mul(2_862_933_555_777_941_757) >> 40) as f64
            / (1u64 << 24) as f64
            - 0.5;
        lp = lp * 0.9 + noise * 0.1;
        let mut s = e + lp * 0.02;
        for (k, (f, bw)) in [(700.0, 90.0), (1220.0, 110.0), (2600.0, 160.0)]
            .iter()
            .enumerate()
        {
            let r = (-std::f64::consts::PI * bw / fs).exp();
            let c = 2.0 * r * (std::f64::consts::TAU * f / fs).cos();
            let o = s * (1.0 - r) + c * ys[k][0] - r * r * ys[k][1];
            ys[k][1] = ys[k][0];
            ys[k][0] = o;
            s = o;
        }
        out.push(s as f32);
    }
    for (k, v) in f0.iter_mut().enumerate() {
        let t = k as f64 * PITCH_HOP;
        if let Some((a, b, m)) = notes.iter().find(|(a, b, _)| t >= *a && t < *b) {
            let m = m
                + 0.4 * (std::f64::consts::TAU * 5.5 * (t - a)).sin()
                + 0.3 * ((t - a) / (b - a) - 0.5);
            *v = (440.0 * 2f64.powf((m - 69.0) / 12.0)) as f32;
        }
    }
    (normalized(out, 0.5), f0)
}

/// Plucked notes: sudden attacks, exponential decay, a few harmonics.
fn plucks(onsets: &[f64], secs: f64) -> Vec<f32> {
    let fs = f64::from(SR);
    let n = (secs * fs) as usize;
    let mut v = vec![0.0f32; n];
    for (k, t0) in onsets.iter().enumerate() {
        let hz = 220.0 * 2f64.powf(k as f64 * 2.0 / 12.0);
        let a = (t0 * fs) as usize;
        for i in a..n {
            let t = (i - a) as f64 / fs;
            let env = (-t / 0.15).exp();
            if env < 1e-4 {
                break;
            }
            let p = std::f64::consts::TAU * hz * t;
            v[i] += (env * (p.sin() + 0.5 * (2.0 * p).sin() + 0.3 * (3.0 * p).sin()) * 0.25) as f32;
        }
    }
    v
}

/// The output of `engine` for `edit` on every note.
fn render(
    engine: Engine,
    edit: Edit,
    x: &[f32],
    blobs: &[(f64, f64)],
    track: &PitchTrack,
) -> Vec<f32> {
    let st = stereo(x);
    match engine {
        Engine::R(r) => {
            let changes: Vec<NoteChange> = blobs.iter().map(|(a, b)| edit.change(*a, *b)).collect();
            let cache = SegmentCache::default();
            mono(&render_notes(&st, SR, track, &changes, r, &cache, 1).0)
        }
        Engine::Flex => {
            if edit != Edit::Up3 {
                return Vec::new();
            }
            let shifts: Vec<prowave_engine::PitchShift> = blobs
                .iter()
                .map(|(a, b)| prowave_engine::PitchShift {
                    start: *a,
                    end: *b,
                    semitones: 3.0,
                })
                .collect();
            mono(&prowave_engine::shift_pitch(
                &st,
                SR,
                &shifts,
                prowave_engine::FlexMode::Monophonic,
            ))
        }
    }
}

/// Frames inside the notes, 30 ms in from their edges.
fn inner_frames(blobs: &[(f64, f64)], n: usize) -> Vec<usize> {
    let mut v = Vec::new();
    for (a, b) in blobs {
        let fa = PitchTrack::frame_at(a + 0.03);
        let fb = PitchTrack::frame_at(b - 0.03).min(n);
        v.extend(fa..fb);
    }
    v
}

fn percentile(mut v: Vec<f64>, p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Median and 95th percentile of |cents| between the output's pitch and `want` (Hz per frame).
fn f0_error(out: &[f32], want: &[f32], frames: &[usize]) -> (f64, f64, f64) {
    let t = track_pitch(out, SR);
    let mut e = Vec::new();
    let mut missing = 0;
    for f in frames {
        let (Some(o), Some(w)) = (t.hz.get(*f), want.get(*f)) else {
            continue;
        };
        if *w <= 0.0 {
            continue;
        }
        if *o <= 0.0 {
            missing += 1;
            continue;
        }
        e.push((1200.0 * (f64::from(*o) / f64::from(*w)).log2()).abs());
    }
    let unvoiced = missing as f64 / (missing + e.len()).max(1) as f64 * 100.0;
    (percentile(e.clone(), 0.5), percentile(e, 0.95), unvoiced)
}

/// CheapTrick envelopes (log power) of a signal at its own pitch, one per 5 ms frame.
fn envelopes(x: &[f32]) -> (Vec<Vec<f64>>, PitchTrack) {
    let t = track_pitch(x, SR);
    let xd: Vec<f64> = x.iter().map(|v| f64::from(*v)).collect();
    let f0: Vec<f64> = t.hz.iter().map(|h| f64::from(*h)).collect();
    let p = vocoder::analyze(&xd, SR, PITCH_HOP * 1000.0, &f0);
    let env = p.map_or_else(Vec::new, |p| {
        p.spectrum
            .iter()
            .map(|r| r.iter().map(|v| v.max(1e-20).ln()).collect())
            .collect()
    });
    (env, t)
}

/// The first `n` real-cepstrum coefficients of a log power envelope (bins 0..=N/2).
fn cepstrum(env: &[f64], n: usize) -> Vec<f64> {
    let bins = env.len();
    (1..=n)
        .map(|k| {
            let mut s = 0.0;
            for (j, v) in env.iter().enumerate() {
                let w = if j == 0 || j == bins - 1 { 1.0 } else { 2.0 };
                s += w * v * (std::f64::consts::PI * k as f64 * j as f64 / (bins - 1) as f64).cos();
            }
            s / (2.0 * (bins - 1) as f64)
        })
        .collect()
}

/// Mean cepstral distance in dB between two signals' envelopes over `frames`.
fn envelope_distance(a: &[Vec<f64>], b: &[Vec<f64>], frames: &[usize]) -> f64 {
    let mut d = Vec::new();
    for f in frames {
        let (Some(x), Some(y)) = (a.get(*f), b.get(*f)) else {
            continue;
        };
        // Up to 8 kHz (where a voice's formants are; the test vowel has nothing above).
        let top = (8000.0 / (f64::from(SR) / 2.0) * (x.len() - 1) as f64) as usize;
        // Natural-log power → dB cepstra: 10/ln10.
        let cx = cepstrum(&x[..=top], 24);
        let cy = cepstrum(&y[..=top], 24);
        let s: f64 = cx.iter().zip(&cy).map(|(p, q)| (p - q).powi(2)).sum();
        d.push(10.0 / std::f64::consts::LN_10 * (2.0 * s).sqrt());
    }
    d.iter().sum::<f64>() / d.len().max(1) as f64
}

/// The formant shift (semitones) that best maps `a`'s envelope onto `b`'s, 200 Hz – 5 kHz.
fn formant_shift(a: &[Vec<f64>], b: &[Vec<f64>], frames: &[usize]) -> f64 {
    let mut best = Vec::new();
    for f in frames.iter().step_by(4) {
        let (Some(x), Some(y)) = (a.get(*f), b.get(*f)) else {
            continue;
        };
        let bins = x.len();
        let hz = |j: usize| j as f64 * f64::from(SR) / (2.0 * (bins - 1) as f64);
        let mut top = (f64::INFINITY, 0.0);
        for k10 in -40..=40 {
            let k = 2f64.powf(f64::from(k10) / 120.0);
            let mut err = 0.0;
            let mut count = 0;
            for (j, yv) in y.iter().enumerate() {
                if !(200.0..5000.0).contains(&hz(j)) {
                    continue;
                }
                let src = j as f64 / k;
                let lo = src.floor() as usize;
                if lo + 1 >= bins {
                    continue;
                }
                let fr = src - lo as f64;
                let xv = x[lo] * (1.0 - fr) + x[lo + 1] * fr;
                err += (xv - yv).powi(2);
                count += 1;
            }
            // Levels may differ: compare shapes (the mean difference taken out roughly by
            // the square error's minimum being shape-driven at fixed level).
            let e = err / count.max(1) as f64;
            if e < top.0 {
                top = (e, f64::from(k10) / 10.0);
            }
        }
        best.push(top.1);
    }
    percentile(best, 0.5)
}

/// Harmonic-to-noise ratio (dB) on `frames`, the harmonics of `f0` (Hz per frame).
fn hnr(x: &[f32], f0: &[f32], frames: &[usize]) -> f64 {
    const N: usize = 2048;
    let fft = Fft::new(N);
    let hann: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos())
        .collect();
    let mut vals = Vec::new();
    for f in frames.iter().step_by(4) {
        let hz = f64::from(f0.get(*f).copied().unwrap_or(0.0));
        if hz <= 0.0 {
            continue;
        }
        let c = (PitchTrack::time_of(*f) * f64::from(SR)) as isize;
        let (mut re, mut im) = (vec![0.0f32; N], vec![0.0f32; N]);
        for i in 0..N {
            let j = c - (N / 2) as isize + i as isize;
            re[i] = if j >= 0 && (j as usize) < x.len() {
                x[j as usize] * hann[i]
            } else {
                0.0
            };
        }
        fft.process(&mut re, &mut im, false);
        let bin_hz = f64::from(SR) / N as f64;
        let (mut h, mut nz) = (0.0f64, 0.0f64);
        for b in 1..N / 2 {
            let fq = b as f64 * bin_hz;
            if fq > 5000.0 {
                break;
            }
            let p = f64::from(re[b] * re[b] + im[b] * im[b]);
            let k = fq / hz;
            if (k - k.round()).abs() * hz < 2.0 * bin_hz {
                h += p;
            } else {
                nz += p;
            }
        }
        if nz > 0.0 && h > 0.0 {
            vals.push(10.0 * (h / nz).log10());
        }
    }
    vals.iter().sum::<f64>() / vals.len().max(1) as f64
}

/// Pre-echo (dB) and 10–90 % rise time (ms) at each onset, averaged.
fn transients(x: &[f32], onsets: &[f64]) -> (f64, f64) {
    let fs = f64::from(SR);
    let rms = |a: isize, b: isize| -> f64 {
        let (a, b) = (a.max(0) as usize, (b.max(0) as usize).min(x.len()));
        if b <= a {
            return 0.0;
        }
        (x[a..b].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / (b - a) as f64).sqrt()
    };
    let (mut pre, mut rise) = (Vec::new(), Vec::new());
    for t in onsets {
        let o = (t * fs) as isize;
        let ms = (fs / 1000.0) as isize;
        let before = rms(o - 20 * ms, o - 2 * ms);
        let after = rms(o, o + 20 * ms);
        pre.push(20.0 * (before.max(1e-5) / after.max(1e-9)).log10());
        let env: Vec<f64> = (0..40)
            .map(|k| rms(o + (k - 5) * ms, o + (k - 4) * ms))
            .collect();
        let peak = env.iter().copied().fold(0.0, f64::max);
        let t10 = env.iter().position(|v| *v >= 0.1 * peak).unwrap_or(0);
        let t90 = env.iter().position(|v| *v >= 0.9 * peak).unwrap_or(0);
        rise.push((t90 as f64 - t10 as f64).max(0.0));
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    (mean(&pre), mean(&rise))
}

/// The asked-for f0 after `edit`: the source's, moved as the targets say.
fn wanted_f0(source_f0: &[f32], track: &PitchTrack, blobs: &[(f64, f64)], edit: Edit) -> Vec<f32> {
    let changes: Vec<NoteChange> = blobs.iter().map(|(a, b)| edit.change(*a, *b)).collect();
    let t = prowave_engine::note_render::targets(track, &changes);
    source_f0
        .iter()
        .enumerate()
        .map(|(f, h)| h * t.ratio.get(f).copied().unwrap_or(1.0))
        .collect()
}

fn write(dir: &Path, name: &str, x: &[f32]) {
    let _ = prowave_engine::write_wav(&dir.join(format!("{name}.wav")), &stereo(x), SR);
}

struct Row {
    material: String,
    engine: &'static str,
    edit: &'static str,
    f0_med: f64,
    f0_p95: f64,
    lost: f64,
    env_db: f64,
    formant: f64,
    hnr_delta: f64,
    secs: f64,
}

/// Runs every engine and edit on `x`, whose true f0 (if known) is `truth`.
fn evaluate(name: &str, x: &[f32], truth: Option<&[f32]>, dir: &Path, rows: &mut Vec<Row>) {
    let found = find_notes(x, SR, Algorithm::Melodic, &[]);
    let blobs: Vec<(f64, f64)> = found.blobs.iter().map(|b| (b.start, b.end)).collect();
    let track = &found.track;
    let source_f0: Vec<f32> = truth.map_or_else(|| track.hz.clone(), <[f32]>::to_vec);
    let frames = inner_frames(&blobs, track.len());
    let (env_in, _) = envelopes(x);
    let hnr_in = hnr(x, &source_f0, &frames);
    write(dir, &format!("{name}-original"), x);
    for engine in ENGINES {
        for edit in [Edit::Up3, Edit::NoVibrato, Edit::Formant2] {
            let began = Instant::now();
            let out = render(engine, edit, x, &blobs, track);
            let secs = began.elapsed().as_secs_f64();
            if out.is_empty() {
                continue;
            }
            write(
                dir,
                &format!("{name}-{}-{}", engine.name(), edit.name()).replace([' ', '(', ')'], ""),
                &out,
            );
            let want = wanted_f0(&source_f0, track, &blobs, edit);
            let (f0_med, f0_p95, lost) = f0_error(&out, &want, &frames);
            let (env_out, _) = envelopes(&out);
            let env_db = if edit == Edit::Formant2 {
                f64::NAN
            } else {
                envelope_distance(&env_in, &env_out, &frames)
            };
            let formant = if edit == Edit::Formant2 {
                formant_shift(&env_in, &env_out, &frames)
            } else {
                f64::NAN
            };
            let hnr_delta = hnr(&out, &want, &frames) - hnr_in;
            rows.push(Row {
                material: name.into(),
                engine: engine.name(),
                edit: edit.name(),
                f0_med,
                f0_p95,
                lost,
                env_db,
                formant,
                hnr_delta,
                secs,
            });
        }
    }
}

/// The loudest `secs` of a file's middle channel (where most is sung).
fn loudest(path: &Path, secs: f64) -> Option<Vec<f32>> {
    let f = AudioFile::open(path).ok()?;
    if f.sample_rate != SR {
        eprintln!(
            "{}: {} Hz, skipped (the harness works at 44.1 kHz)",
            path.display(),
            f.sample_rate
        );
        return None;
    }
    let m = mono(&f.read_stereo().ok()?);
    let w = (secs * f64::from(SR)) as usize;
    if m.len() <= w {
        return Some(m);
    }
    let hop = SR as usize;
    let mut best = (0.0f64, 0usize);
    let mut s = 0;
    while s + w <= m.len() {
        let e: f64 = m[s..s + w]
            .iter()
            .step_by(16)
            .map(|v| f64::from(*v).powi(2))
            .sum();
        if e > best.0 {
            best = (e, s);
        }
        s += hop;
    }
    Some(m[best.1..best.1 + w].to_vec())
}

/// Known notes from a real vocal: Harvest's pitch rounded to semitones (hysteresis, at least
/// 80 ms a note), then sung back by WORLD at exactly those notes with a gentle vibrato.
fn resung(x: &[f32]) -> Option<(Vec<f32>, Vec<(f64, f64, f32)>)> {
    let xd: Vec<f64> = x.iter().map(|v| f64::from(*v)).collect();
    let fms = PITCH_HOP * 1000.0;
    let hv = vocoder::harvest(&xd, SR, fms, 70.0, 1000.0);
    let mut p = vocoder::analyze(&xd, SR, fms, &hv)?;
    let n = p.f0.len();
    // Only frames the source really sings: Harvest calls nearly everything voiced (reverb
    // tails, breath), so a frame counts only if D4C finds it periodic below 3 kHz too.
    let bins = p.bins();
    let bin_of = |hz: f64| (hz / f64::from(SR) * p.fft_size as f64) as usize;
    let (lo, hi) = (bin_of(100.0), bin_of(3000.0).min(bins - 1));
    for f in 0..n {
        let ap: f64 = p.aperiodicity[f][lo..=hi].iter().sum::<f64>() / (hi - lo + 1) as f64;
        if ap > 0.35 {
            p.f0[f] = 0.0;
        }
    }
    let min_len = (0.08 / PITCH_HOP) as usize;
    // Notes: runs of voiced frames, cut where the rounded pitch changes for 40 ms or more.
    let mut notes: Vec<(usize, usize, f32)> = Vec::new();
    let mut f = 0;
    while f < n {
        if p.f0[f] <= 0.0 {
            f += 1;
            continue;
        }
        let midi = |i: usize| 69.0 + 12.0 * (p.f0[i] / 440.0).log2();
        let mut start = f;
        let mut cur = midi(f).round();
        let mut g = f;
        while g < n && p.f0[g] > 0.0 {
            let m = midi(g);
            if (m - cur).abs() > 0.7 {
                // A new note if it stays there 40 ms.
                let stays =
                    (g..(g + 8).min(n)).all(|k| p.f0[k] > 0.0 && (midi(k) - m.round()).abs() < 0.7);
                if stays {
                    if g - start >= min_len {
                        notes.push((start, g, cur as f32));
                    }
                    start = g;
                    cur = m.round();
                }
            }
            g += 1;
        }
        if g - start >= min_len {
            notes.push((start, g, cur as f32));
        }
        f = g + 1;
    }
    // Sing them: flat at the note, a 5.5 Hz ±25 cent vibrato from 150 ms in, unvoiced where
    // no note is.
    let mut f0 = vec![0.0f64; n];
    for (a, b, m) in &notes {
        for (k, v) in f0.iter_mut().enumerate().take(*b).skip(*a) {
            let t = (k - a) as f64 * PITCH_HOP;
            let vib = if t > 0.15 {
                0.25 * (std::f64::consts::TAU * 5.5 * t).sin()
            } else {
                0.0
            };
            *v = 440.0 * 2f64.powf((f64::from(*m) + vib - 69.0) / 12.0);
        }
    }
    p.f0 = f0;
    let y = vocoder::synthesize(&p, x.len());
    let y: Vec<f32> = y.into_iter().map(|v| v as f32).collect();
    let notes = notes
        .into_iter()
        .map(|(a, b, m)| (PitchTrack::time_of(a), PitchTrack::time_of(b), m))
        .collect();
    Some((normalized(y, 0.5), notes))
}

/// Precision, recall, F (onset ±50 ms, pitch ±50 cents) and the median onset, offset and
/// pitch errors of the matched notes.
fn score_notes(found: &[(f64, f64, f32)], truth: &[(f64, f64, f32)]) -> [f64; 6] {
    score_notes_with(found, truth, false)
}

/// The same with the pitch compared octave-free (an octave error counts as right).
fn score_notes_with(found: &[(f64, f64, f32)], truth: &[(f64, f64, f32)], fold: bool) -> [f64; 6] {
    let dist = |a: f32, b: f32| {
        let d = (a - b).abs();
        if fold {
            let r = d.rem_euclid(12.0);
            r.min(12.0 - r)
        } else {
            d
        }
    };
    let mut used = vec![false; found.len()];
    let (mut on, mut off, mut pitch) = (Vec::new(), Vec::new(), Vec::new());
    let mut hits = 0;
    for (ta, tb, tm) in truth {
        let best = found
            .iter()
            .enumerate()
            .filter(|(i, (a, _, m))| !used[*i] && (a - ta).abs() <= 0.05 && dist(*m, *tm) <= 0.5)
            .min_by(|x, y| (x.1 .0 - ta).abs().total_cmp(&(y.1 .0 - ta).abs()));
        if let Some((i, (a, b, m))) = best {
            used[i] = true;
            hits += 1;
            on.push((a - ta).abs() * 1000.0);
            off.push((b - tb).abs() * 1000.0);
            pitch.push(f64::from(dist(*m, *tm)) * 100.0);
        }
    }
    let p = hits as f64 / found.len().max(1) as f64;
    let r = hits as f64 / truth.len().max(1) as f64;
    let f = if p + r > 0.0 {
        2.0 * p * r / (p + r)
    } else {
        0.0
    };
    [
        p,
        r,
        f,
        percentile(on, 0.5),
        percentile(off, 0.5),
        percentile(pitch, 0.5),
    ]
}

/// The tracker against Harvest: raw pitch accuracy (% within 50 cents where both are voiced),
/// voicing agreement (%), median |cents|.
fn against_harvest(x: &[f32]) -> (f64, f64, f64) {
    let t = track_pitch(x, SR);
    let xd: Vec<f64> = x.iter().map(|v| f64::from(*v)).collect();
    let hv = vocoder::harvest(&xd, SR, PITCH_HOP * 1000.0, 55.0, 1400.0);
    let n = t.len().min(hv.len());
    let (mut agree, mut within, mut both) = (0, 0, 0);
    let mut cents = Vec::new();
    for f in 0..n {
        let (a, b) = (f64::from(t.hz[f]), hv[f]);
        if (a > 0.0) == (b > 0.0) {
            agree += 1;
        }
        if a > 0.0 && b > 0.0 {
            both += 1;
            let c = (1200.0 * (a / b).log2()).abs();
            if c <= 50.0 {
                within += 1;
            }
            cents.push(c);
        }
    }
    let (mut up, mut down) = (0, 0);
    for f in 0..n {
        let (a, b) = (f64::from(t.hz[f]), hv[f]);
        if a > 0.0 && b > 0.0 {
            let c = 1200.0 * (a / b).log2();
            if c > 600.0 {
                up += 1;
            } else if c < -600.0 {
                down += 1;
            }
        }
    }
    println!("octave disagreements: ours higher {up} frames, ours lower {down} frames");
    let ours = t.hz.iter().take(n).filter(|h| **h > 0.0).count();
    let theirs = hv.iter().take(n).filter(|h| **h > 0.0).count();
    println!(
        "voiced frames: ours {:.1} %, Harvest {:.1} %",
        ours as f64 / n.max(1) as f64 * 100.0,
        theirs as f64 / n.max(1) as f64 * 100.0
    );
    (
        within as f64 / both.max(1) as f64 * 100.0,
        agree as f64 / n.max(1) as f64 * 100.0,
        percentile(cents, 0.5),
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first().map(PathBuf::from) else {
        eprintln!("usage: note_render_eval OUT_DIR [VOCAL ...]");
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let mut rows = Vec::new();

    // Synthetic: three notes with vibrato and drift, known f0.
    let notes = [(0.2, 1.2, 57.0), (1.3, 2.3, 60.0), (2.4, 3.6, 62.0)];
    let (syn, truth) = synthetic_vocal(&notes, 3.8);
    evaluate("synthetic", &syn, Some(&truth), &dir, &mut rows);

    // Synthetic detection: are the three notes found?
    let found = find_notes(&syn, SR, Algorithm::Melodic, &[]);
    let fnotes: Vec<(f64, f64, f32)> = found
        .blobs
        .iter()
        .map(|b| (b.onset, b.end, b.pitch))
        .collect();
    let tnotes: Vec<(f64, f64, f32)> = notes.iter().map(|(a, b, m)| (*a, *b, *m as f32)).collect();
    let s = score_notes(&fnotes, &tnotes);
    println!("\nDETECTION synthetic: P {:.2} R {:.2} F {:.2}, onset {:.1} ms, offset {:.1} ms, pitch {:.1} cents", s[0], s[1], s[2], s[3], s[4], s[5]);

    // Transients: plucks moved up 3 semitones.
    let onsets = [0.3, 0.8, 1.3, 1.8];
    let pl = plucks(&onsets, 2.4);
    let pf = find_notes(&pl, SR, Algorithm::Melodic, &[]);
    let pblobs: Vec<(f64, f64)> = pf.blobs.iter().map(|b| (b.start, b.end)).collect();
    let (pre0, rise0) = transients(&pl, &onsets);
    println!("\nTRANSIENTS (plucks, pitch +3; {} notes found) original: pre-echo {pre0:.1} dB, rise {rise0:.1} ms", pblobs.len());
    for engine in ENGINES {
        let out = render(engine, Edit::Up3, &pl, &pblobs, &pf.track);
        let (pre, rise) = transients(&out, &onsets);
        write(
            &dir,
            &format!("plucks-{}", engine.name()).replace([' ', '(', ')'], ""),
            &out,
        );
        println!(
            "  {:12} pre-echo {pre:6.1} dB  rise {rise:5.1} ms",
            engine.name()
        );
    }

    if std::env::var("SWEEP").is_ok() {
        sweep(&args[1..]);
        return;
    }

    // Real vocals.
    for (k, path) in args.iter().skip(1).enumerate() {
        let Some(x) = loudest(Path::new(path), 30.0) else {
            continue;
        };
        let name = format!("vocal{}", k + 1);
        println!("\n{name} = {path}");
        let (rpa, va, med) = against_harvest(&x);
        println!("TRACKER vs Harvest: raw pitch accuracy {rpa:.1} %, voicing agreement {va:.1} %, median {med:.1} cents");
        let found = find_notes(&x, SR, Algorithm::Melodic, &[]);
        println!("NOTES found in the real vocal: {}", found.blobs.len());
        if let Some((y, tn)) = resung(&x) {
            write(&dir, &format!("{name}-resung"), &y);
            let f = find_notes(&y, SR, Algorithm::Melodic, &[]);
            let fnotes: Vec<(f64, f64, f32)> =
                f.blobs.iter().map(|b| (b.onset, b.end, b.pitch)).collect();
            let s = score_notes(&fnotes, &tn);
            if std::env::var("DUMP").is_ok() {
                for (a, b, m) in tn.iter().take(25) {
                    println!("  true  {a:6.3} {b:6.3} {m:5.1}");
                }
                for (a, b, m) in fnotes.iter().take(25) {
                    println!("  found {a:6.3} {b:6.3} {m:5.1}");
                }
                let tv = f.track.hz.iter().filter(|h| **h > 0.0).count();
                let truth_v: usize = tn
                    .iter()
                    .map(|(a, b, _)| ((b - a) / PITCH_HOP) as usize)
                    .sum();
                println!("  voiced frames on the re-sung: ours {tv}, truth {truth_v}");
            }
            println!(
                "DETECTION re-sung ({} true notes, {} found): P {:.2} R {:.2} F {:.2}, onset {:.1} ms, offset {:.1} ms, pitch {:.1} cents",
                tn.len(), fnotes.len(), s[0], s[1], s[2], s[3], s[4], s[5]
            );
            let o = score_notes_with(&fnotes, &tn, true);
            println!("  octave-free: P {:.2} R {:.2} F {:.2}", o[0], o[1], o[2]);
        }
        let began = Instant::now();
        let _ = find_notes(&x, SR, Algorithm::Melodic, &[]);
        println!("analysis: {:.2} s for 30 s", began.elapsed().as_secs_f64());
        evaluate(&name, &x, None, &dir, &mut rows);
    }

    println!("\nRENDER (f0 error after the edit in cents: median / p95, % frames lost to unvoiced; envelope distance dB; formant shift made, semitones; HNR change dB; seconds)");
    println!(
        "{:10} {:12} {:11} {:>7} {:>7} {:>6} {:>7} {:>7} {:>7} {:>6}",
        "material", "engine", "edit", "f0med", "f0p95", "lost%", "envdB", "formant", "dHNR", "secs"
    );
    for r in &rows {
        println!(
            "{:10} {:12} {:11} {:7.1} {:7.1} {:6.1} {:7.2} {:7.2} {:7.1} {:6.2}",
            r.material,
            r.engine,
            r.edit,
            r.f0_med,
            r.f0_p95,
            r.lost,
            r.env_db,
            r.formant,
            r.hnr_delta,
            r.secs
        );
    }
}

/// Detection on the re-sung vocals (and false voicing on noise) for several tracker settings.
fn sweep(paths: &[String]) {
    use prowave_engine::pitch_tracker::{track_pitch_with, TrackerSettings};
    let sung: Vec<(Vec<f32>, Vec<(f64, f64, f32)>)> = paths
        .iter()
        .filter_map(|p| resung(&loudest(Path::new(p), 30.0)?))
        .collect();
    let mut seed = 7u32;
    let noise: Vec<f32> = (0..SR as usize * 5)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.5
        })
        .collect();
    let base = prowave_engine::pitch_tracker::DEFAULT_SETTINGS;
    let mut sets = vec![(0.1, 0.01f32, 0.0f32, 0.0f32)];
    for (r, fl) in [(0.5, 0.0), (0.5, 0.005), (0.5, 0.01)] {
        sets.push((base.threshold_mean, base.no_dip_weight, fl, r));
    }
    for (mean, nd, fl, r) in sets {
        {
            let st = TrackerSettings {
                threshold_mean: mean,
                no_dip_weight: nd,
                octave_ratio: r,
                octave_margin: fl,
            };
            let nv = track_pitch_with(&noise, SR, st)
                .hz
                .iter()
                .filter(|h| **h > 0.0)
                .count() as f64
                / (noise.len() as f64 / f64::from(SR) / PITCH_HOP)
                * 100.0;
            let mut fs = Vec::new();
            let mut vr = Vec::new();
            for (y, tn) in &sung {
                let t = track_pitch_with(y, SR, st);
                let truth_v: usize = tn
                    .iter()
                    .map(|(a, b, _)| ((b - a) / PITCH_HOP) as usize)
                    .sum();
                // Voiced recall: true voiced frames we call voiced.
                let mut hit = 0;
                for (a, b, _) in tn {
                    for f in PitchTrack::frame_at(*a)..PitchTrack::frame_at(*b) {
                        if t.hz.get(f).is_some_and(|h| *h > 0.0) {
                            hit += 1;
                        }
                    }
                }
                vr.push(hit as f64 / truth_v.max(1) as f64 * 100.0);
                let blobs = prowave_engine::melody::notes_from_track(&t);
                let fnotes: Vec<(f64, f64, f32)> =
                    blobs.iter().map(|b| (b.onset, b.end, b.pitch)).collect();
                fs.push(score_notes(&fnotes, tn)[2]);
                fs.push(score_notes_with(&fnotes, tn, true)[2]);
            }
            println!(
                "mean {mean:.2} no-dip {nd:.2} octave ratio {r:.2} margin {fl:.3}: note F (strict, octave-free) {:?}, voiced recall {:?} %, noise voiced {nv:.1} %",
                fs.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>(),
                vr.iter().map(|v| format!("{v:.0}")).collect::<Vec<_>>()
            );
        }
    }
}
