//! The Pitch Editor's sound: the notes whose pitch, drift, modulation or formants were changed
//! are made again (resynthesized), the rest of the recording is left exactly as it was.
//!
//! A change is a [`NoteChange`] on a stretch of the recording. [`targets`] turns the changes
//! into a curve, per pitch frame, of how much the pitch moves (a ratio) and how far the
//! formants move; [`render_notes`] finds the stretches where anything moves, makes each of them
//! again with a [`Resynth`] engine, and fades them in and out of the untouched recording where
//! it is quietest nearby. Each stretch is kept in a [`SegmentCache`] by what it is made from, so
//! after an edit only the stretches that changed are made again.
//!
//! The engines (all offline):
//! - [`Resynth::World`]: the WORLD vocoder ([`crate::vocoder`]); pitch, spectral envelope and
//!   aperiodicity are separate, so the pitch moves without the formants and the formants move
//!   without the pitch.
//! - [`Resynth::Psola`]: time-domain PSOLA: one grain per pitch period, laid out again at the
//!   new period (formants stay; a formant shift resamples the grains).
//! - [`Resynth::Signalsmith`]: Signalsmith Stretch's pitch shift with its formant
//!   compensation (a spectral method).
//!
//! Which one the Pitch Editor uses was decided by measurement (see `examples/note_render_eval.rs`
//! and [`DEFAULT_RESYNTH`]).
use crate::melody::{decompose, pitch_centre};
use crate::pitch_tracker::{PitchTrack, PITCH_HOP};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A change to one note, on the stretch `start..end` (seconds into the recording).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteChange {
    pub start: f64,
    pub end: f64,
    /// Semitones added to the pitch centre.
    pub shift: f32,
    /// How much of the drift and of the modulation is kept (1 = as recorded, 0 = none, 2 =
    /// twice as much).
    pub drift: f32,
    pub modulation: f32,
    /// Semitones the formants move, independent of the pitch.
    pub formant: f32,
}

impl NoteChange {
    /// A change that changes nothing.
    pub fn none(start: f64, end: f64) -> Self {
        Self {
            start,
            end,
            shift: 0.0,
            drift: 1.0,
            modulation: 1.0,
            formant: 0.0,
        }
    }

    pub fn is_neutral(&self) -> bool {
        self.shift.abs() < 1e-4
            && (self.drift - 1.0).abs() < 1e-4
            && (self.modulation - 1.0).abs() < 1e-4
            && self.formant.abs() < 1e-4
    }
}

/// The resynthesis engines compared for the Pitch Editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resynth {
    World,
    Psola,
    Signalsmith,
}

/// The engine the Pitch Editor renders Melodic notes with, chosen by the measurements in
/// `examples/note_render_eval.rs` (the table is there): on real vocals moved +3 semitones it
/// keeps the spectral envelope within 2.1–3.0 dB of the original (Signalsmith 5.4–6.4, PSOLA
/// 2.9–3.5, the old Flex shift 9.4–11.2), makes a +2 semitone formant shift as 2.0–2.1
/// (Signalsmith 0.2), and keeps the pitch within 2 cents (median).
pub const DEFAULT_RESYNTH: Resynth = Resynth::World;
/// The engine for Percussive and Universal notes, transposed as a whole (drums, mixes:
/// not a single voice WORLD could model).
pub const WHOLE_RESYNTH: Resynth = Resynth::Signalsmith;

/// Per pitch frame: the pitch ratio (new / old, 1 where unvoiced or untouched) and the formant
/// ratio, and whether anything moves there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets {
    pub ratio: Vec<f32>,
    pub formant: Vec<f32>,
}

impl Targets {
    pub fn active(&self, f: usize) -> bool {
        self.ratio.get(f).is_some_and(|r| (r - 1.0).abs() > 1e-4)
            || self.formant.get(f).is_some_and(|r| (r - 1.0).abs() > 1e-4)
    }

    fn at(v: &[f32], secs: f64) -> f32 {
        if v.is_empty() {
            return 1.0;
        }
        let x = (secs / PITCH_HOP).max(0.0);
        let i = (x.floor() as usize).min(v.len() - 1);
        let j = (i + 1).min(v.len() - 1);
        let f = (x - i as f64).clamp(0.0, 1.0) as f32;
        v[i] + (v[j] - v[i]) * f
    }

    pub fn ratio_at(&self, secs: f64) -> f32 {
        Self::at(&self.ratio, secs)
    }

    pub fn formant_at(&self, secs: f64) -> f32 {
        Self::at(&self.formant, secs)
    }
}

/// How long a change of pitch or formant takes to glide in at a note's edge (s): Melodyne's
/// pitch transitions.
const TRANSITION: f64 = 0.03;

/// The pitch and formant curves `changes` ask for, on the frames of `track` (a pitch moves
/// only where there is one).
pub fn targets(track: &PitchTrack, changes: &[NoteChange]) -> Targets {
    targets_with(track, changes, true)
}

/// [`targets`]; with `voiced_only` false every frame of a changed note is moved (Percussive
/// and Universal notes, transposed as a whole).
pub fn targets_with(track: &PitchTrack, changes: &[NoteChange], voiced_only: bool) -> Targets {
    let n = track.len();
    // Semitones each frame moves, before and after the transitions are smoothed.
    let mut semis = vec![0.0f32; n];
    let mut shift = vec![0.0f32; n];
    let mut formant = vec![0.0f32; n];
    for c in changes {
        if !(c.start.is_finite() && c.end.is_finite() && c.end > c.start) || c.is_neutral() {
            continue;
        }
        let (a, b) = (
            PitchTrack::frame_at(c.start).min(n),
            PitchTrack::frame_at(c.end).min(n),
        );
        if b <= a {
            continue;
        }
        let drift = c.drift.clamp(-1.0, 2.0);
        let modulation = c.modulation.clamp(-1.0, 2.0);
        let reshape = (drift - 1.0).abs() > 1e-4 || (modulation - 1.0).abs() > 1e-4;
        let centre = pitch_centre(track, a, b);
        let parts = match (reshape, centre) {
            (true, Some(cn)) => decompose(track, a, b, cn),
            _ => Vec::new(),
        };
        for f in a..b {
            shift[f] = c.shift.clamp(-36.0, 36.0);
            formant[f] = c.formant.clamp(-24.0, 24.0);
            if let Some(Some((d, m))) = parts.get(f - a) {
                // curve = centre + d + m  →  centre + d·drift + m·modulation
                semis[f] = d * (drift - 1.0) + m * (modulation - 1.0);
            }
        }
    }
    // The steps at the notes' edges glide over TRANSITION.
    let w = ((TRANSITION / PITCH_HOP).round() as usize) | 1;
    let smooth = |v: &[f32]| -> Vec<f32> {
        let half = w / 2;
        (0..v.len())
            .map(|i| {
                let a = i.saturating_sub(half);
                let b = (i + half + 1).min(v.len());
                v[a..b].iter().sum::<f32>() / (b - a) as f32
            })
            .collect()
    };
    let shift = smooth(&shift);
    let formant = smooth(&formant);
    Targets {
        ratio: (0..n)
            .map(|f| {
                if track.hz[f] > 0.0 || !voiced_only {
                    2f32.powf((shift[f] + semis[f]) / 12.0)
                } else {
                    1.0
                }
            })
            .collect(),
        formant: formant.iter().map(|s| 2f32.powf(s / 12.0)).collect(),
    }
}

/// Made stretches kept by what they are made from, so only changed notes are made again.
#[derive(Default)]
pub struct SegmentCache {
    map: Mutex<HashMap<u64, Arc<Vec<f32>>>>,
}

/// Most samples a [`SegmentCache`] keeps (about 200 MB); then it starts again.
const CACHE_SAMPLES: usize = 50_000_000;

impl SegmentCache {
    pub fn get(&self, key: u64) -> Option<Arc<Vec<f32>>> {
        self.map.lock().ok()?.get(&key).cloned()
    }

    pub fn put(&self, key: u64, v: Arc<Vec<f32>>) {
        if let Ok(mut m) = self.map.lock() {
            let total: usize = m.values().map(|x| x.len()).sum();
            if total + v.len() > CACHE_SAMPLES {
                m.clear();
            }
            m.insert(key, v);
        }
    }

    pub fn len(&self) -> usize {
        self.map.lock().map_or(0, |m| m.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn mix(h: &mut u64, w: u64) {
    *h = (*h ^ w).wrapping_mul(0x0000_0100_0000_01b3);
}

/// What was made, for the tests and the status line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub segments: usize,
    /// Of them, made now (the others came from the cache).
    pub made: usize,
}

/// The recording (interleaved stereo) with the changed notes made again by `engine`.
/// `source_id` names the recording in the cache keys (its file hash).
pub fn render_notes(
    stereo: &[f32],
    sample_rate: u32,
    track: &PitchTrack,
    changes: &[NoteChange],
    engine: Resynth,
    cache: &SegmentCache,
    source_id: u64,
) -> (Vec<f32>, RenderStats) {
    render_notes_with(
        stereo,
        sample_rate,
        track,
        changes,
        engine,
        true,
        cache,
        source_id,
    )
}

/// [`render_notes`], choosing whether only voiced frames move (see [`targets_with`]).
#[allow(clippy::too_many_arguments)]
pub fn render_notes_with(
    stereo: &[f32],
    sample_rate: u32,
    track: &PitchTrack,
    changes: &[NoteChange],
    engine: Resynth,
    voiced_only: bool,
    cache: &SegmentCache,
    source_id: u64,
) -> (Vec<f32>, RenderStats) {
    let sr = f64::from(sample_rate.max(1));
    let frames = stereo.len() / 2;
    let mut out = stereo[..frames * 2].to_vec();
    let t = targets_with(track, changes, voiced_only);
    let mut stats = RenderStats::default();
    for (fa, fb) in active_spans(&t, track) {
        let (a, b) = (
            ((PitchTrack::time_of(fa) * sr) as usize).min(frames),
            ((PitchTrack::time_of(fb) * sr) as usize).min(frames),
        );
        if b <= a + 64 {
            continue;
        }
        stats.segments += 1;
        let mut key = 0xcbf2_9ce4_8422_2325u64;
        mix(&mut key, source_id);
        mix(&mut key, engine as u64);
        mix(&mut key, u64::from(voiced_only));
        mix(&mut key, u64::from(sample_rate));
        mix(&mut key, a as u64);
        mix(&mut key, b as u64);
        for f in fa..fb {
            mix(&mut key, u64::from(t.ratio[f].to_bits()));
            mix(&mut key, u64::from(t.formant[f].to_bits()));
        }
        let made = match cache.get(key) {
            Some(v) => v,
            None => {
                stats.made += 1;
                let v = Arc::new(make_segment(stereo, sample_rate, track, &t, a, b, engine));
                cache.put(key, v.clone());
                v
            }
        };
        // Fade the made stretch in and out over 10 ms at its ends (which are quiet: see
        // `active_spans`). Inside, the unvoiced sound (consonants, breaths, the silence
        // before an attack) stays the recording's own, as Melodyne leaves sibilants alone.
        let fade = ((0.01 * sr) as usize).min((b - a) / 4).max(1);
        let voiced = voiced_weights(track, fa, fb, voiced_only && keep_unvoiced(engine));
        for i in 0..b - a {
            let edge = if i < fade {
                i as f32 / fade as f32
            } else if b - a - i <= fade {
                (b - a - i) as f32 / fade as f32
            } else {
                1.0
            };
            let secs = (a + i) as f64 / sr;
            let x = secs / PITCH_HOP - fa as f64;
            let k = (x.floor().max(0.0) as usize).min(voiced.len().saturating_sub(1));
            let k2 = (k + 1).min(voiced.len().saturating_sub(1));
            let fr = (x - x.floor()).clamp(0.0, 1.0) as f32;
            let v = voiced.get(k).map_or(1.0, |w| w + (voiced[k2] - w) * fr);
            let g = edge * v;
            for c in 0..2 {
                let o = &mut out[2 * (a + i) + c];
                let m = made.get(2 * i + c).copied().unwrap_or(0.0);
                *o = *o * (1.0 - g) + m * g;
            }
        }
    }
    (out, stats)
}

/// Whether an engine's result is used only where the recording is voiced: WORLD's, whose
/// noise and smoothed attacks are worse than leaving the unvoiced sound as it is (measured on
/// plucks: pre-echo -23 dB and a 16 ms rise made again, -42 dB and 4.5 ms kept).
fn keep_unvoiced(engine: Resynth) -> bool {
    engine == Resynth::World
}

/// Per frame of `fa..fb`: 1 where the made sound is used, 0 where the recording is kept
/// (unvoiced, when `keep` is set), the steps eased over 15 ms.
fn voiced_weights(track: &PitchTrack, fa: usize, fb: usize, keep: bool) -> Vec<f32> {
    let raw: Vec<f32> = (fa..fb)
        .map(|f| {
            let v = track.hz.get(f).is_some_and(|h| *h > 0.0);
            if v || !keep {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    (0..raw.len())
        .map(|i| {
            let a = i.saturating_sub(1);
            let b = (i + 2).min(raw.len());
            raw[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect()
}

/// The stretches (frames) where anything moves, widened by 40 ms and then to the quietest frame
/// within another 60 ms, joined when they come close.
fn active_spans(t: &Targets, track: &PitchTrack) -> Vec<(usize, usize)> {
    let n = track.len().min(t.ratio.len());
    let margin = (0.04 / PITCH_HOP) as usize;
    let look = (0.06 / PITCH_HOP) as usize;
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut f = 0;
    while f < n {
        if !t.active(f) {
            f += 1;
            continue;
        }
        let a = f;
        while f < n && t.active(f) {
            f += 1;
        }
        let quietest = |lo: usize, hi: usize| {
            (lo..hi.min(n))
                .min_by(|x, y| track.level_db[*x].total_cmp(&track.level_db[*y]))
                .unwrap_or(lo)
        };
        let a0 = a.saturating_sub(margin);
        let s = quietest(a0.saturating_sub(look), a0 + 1);
        let b0 = (f + margin).min(n);
        let e = quietest(b0, b0 + look + 1).max(b0);
        match spans.last_mut() {
            Some(last) if s <= last.1 + margin => last.1 = e.max(last.1),
            _ => spans.push((s, e)),
        }
    }
    spans
}

/// Samples `a..b` of the stereo recording made again (interleaved stereo, `b - a` frames).
/// A recording whose two sides are the same is made once.
fn make_segment(
    stereo: &[f32],
    sample_rate: u32,
    track: &PitchTrack,
    t: &Targets,
    a: usize,
    b: usize,
    engine: Resynth,
) -> Vec<f32> {
    let left: Vec<f32> = (a..b).map(|i| stereo[2 * i]).collect();
    let right: Vec<f32> = (a..b).map(|i| stereo[2 * i + 1]).collect();
    let same = left.iter().zip(&right).all(|(l, r)| (l - r).abs() < 1e-5);
    let offset = a as f64 / f64::from(sample_rate.max(1));
    let l = resynth(&left, sample_rate, track, t, offset, engine);
    let r = if same {
        l.clone()
    } else {
        resynth(&right, sample_rate, track, t, offset, engine)
    };
    let mut out = Vec::with_capacity((b - a) * 2);
    for i in 0..b - a {
        out.push(l.get(i).copied().unwrap_or(0.0));
        out.push(r.get(i).copied().unwrap_or(0.0));
    }
    out
}

/// One channel, starting `offset` seconds into the recording, made again by `engine` with the
/// pitch and formant curves `t`. The result is as long as `x`.
pub fn resynth(
    x: &[f32],
    sample_rate: u32,
    track: &PitchTrack,
    t: &Targets,
    offset: f64,
    engine: Resynth,
) -> Vec<f32> {
    let out = match engine {
        #[cfg(not(target_arch = "wasm32"))]
        Resynth::World => world(x, sample_rate, track, t, offset),
        Resynth::Psola => psola(x, sample_rate, track, t, offset),
        #[cfg(not(target_arch = "wasm32"))]
        Resynth::Signalsmith => signalsmith(x, sample_rate, t, offset),
        #[cfg(target_arch = "wasm32")]
        _ => psola(x, sample_rate, track, t, offset),
    };
    out.into_iter()
        .map(|v| {
            if v.is_finite() {
                v.clamp(-4.0, 4.0)
            } else {
                0.0
            }
        })
        .collect()
}

/// The source pitch at `secs` (Hz, 0 unvoiced), from the nearest frame.
fn source_hz(track: &PitchTrack, secs: f64) -> f32 {
    track
        .hz
        .get(PitchTrack::frame_at(secs))
        .copied()
        .unwrap_or(0.0)
}

#[cfg(not(target_arch = "wasm32"))]
fn world(x: &[f32], sr: u32, track: &PitchTrack, t: &Targets, offset: f64) -> Vec<f32> {
    let frame_ms = PITCH_HOP * 1000.0;
    let xd: Vec<f64> = x.iter().map(|v| f64::from(*v)).collect();
    let frames = crate::vocoder::frames_for(x.len(), sr, frame_ms);
    let f0: Vec<f64> = (0..frames)
        .map(|i| f64::from(source_hz(track, offset + i as f64 * PITCH_HOP)))
        .collect();
    let Some(mut p) = crate::vocoder::analyze(&xd, sr, frame_ms, &f0) else {
        return x.to_vec();
    };
    let bins = p.bins();
    let mut row = vec![0.0f64; bins];
    for i in 0..frames {
        let secs = offset + i as f64 * PITCH_HOP;
        p.f0[i] *= f64::from(t.ratio_at(secs));
        let k = f64::from(t.formant_at(secs));
        if (k - 1.0).abs() > 1e-4 {
            // The envelope stretched along the frequency axis: what was at f is now at k·f.
            for (j, o) in row.iter_mut().enumerate() {
                let src = j as f64 / k;
                let lo = src.floor() as usize;
                let fr = src - lo as f64;
                let get = |m: usize| p.spectrum[i][m.min(bins - 1)];
                *o = get(lo) * (1.0 - fr) + get(lo + 1) * fr;
            }
            p.spectrum[i].copy_from_slice(&row);
            for (j, o) in row.iter_mut().enumerate() {
                let src = (j as f64 / k).min((bins - 1) as f64) as usize;
                *o = p.aperiodicity[i][src];
            }
            p.aperiodicity[i].copy_from_slice(&row);
        }
    }
    crate::vocoder::synthesize(&p, x.len())
        .into_iter()
        .map(|v| v as f32)
        .collect()
}

/// Time-domain PSOLA. Analysis marks one period apart on the waveform's peaks where it is
/// voiced (5 ms apart elsewhere); for each output mark the nearest analysis grain (two of its
/// periods, Hann windowed) is added at the new period's spacing.
fn psola(x: &[f32], sr: u32, track: &PitchTrack, t: &Targets, offset: f64) -> Vec<f32> {
    let srf = f64::from(sr.max(1));
    let n = x.len();
    let unvoiced_period = 0.005 * srf;
    let period_at = |i: f64| -> (f64, bool) {
        let hz = source_hz(track, offset + i / srf);
        if hz > 0.0 {
            (srf / f64::from(hz), true)
        } else {
            (unvoiced_period, false)
        }
    };
    // Analysis marks.
    let mut marks: Vec<(f64, f64)> = Vec::new(); // (position, period)
    let mut pos = 0.0f64;
    while pos < n as f64 {
        let (p, voiced) = period_at(pos);
        let mut m = pos;
        if voiced {
            // The highest sample within a quarter period, so the marks sit on the pulses.
            let lo = (pos - p * 0.25).max(0.0) as usize;
            let hi = ((pos + p * 0.25) as usize).min(n.saturating_sub(1));
            if let Some(best) = (lo..=hi).max_by(|a, b| x[*a].total_cmp(&x[*b])) {
                m = best as f64;
            }
        }
        if marks.last().is_none_or(|l| m > l.0 + 0.5 * p.min(l.1)) {
            marks.push((m, p));
        }
        pos = m.max(pos) + p;
    }
    if marks.is_empty() {
        return x.to_vec();
    }
    let mut out = vec![0.0f32; n];
    let mut ts = marks[0].0;
    while ts < n as f64 {
        let secs = offset + ts / srf;
        let (pa, voiced) = period_at(ts);
        let r = if voiced {
            f64::from(t.ratio_at(secs)).clamp(0.25, 4.0)
        } else {
            1.0
        };
        let k = f64::from(t.formant_at(secs)).clamp(0.25, 4.0);
        let ps = pa / r;
        // The analysis mark nearest the output time.
        let i = marks.partition_point(|m| m.0 < ts);
        let pick = match (i.checked_sub(1), marks.get(i)) {
            (Some(j), Some(b)) => {
                if ts - marks[j].0 <= b.0 - ts {
                    marks[j]
                } else {
                    *b
                }
            }
            (Some(j), None) => marks[j],
            (None, Some(b)) => *b,
            (None, None) => break,
        };
        let (m, p) = pick;
        // The grain: two periods around the mark, read `k` times as fast (formants × k), its
        // level kept per period (more grains a second at a higher pitch).
        let half = p / k;
        let gain = (1.0 / r) as f32;
        let lo = (ts - half).ceil() as isize;
        let hi = (ts + half).floor() as isize;
        for o in lo..=hi {
            if o < 0 || o as usize >= n {
                continue;
            }
            let d = o as f64 - ts; // -half..half
            let w = 0.5 + 0.5 * (std::f64::consts::PI * d / half).cos();
            let src = m + d * k;
            if src < 0.0 || src >= (n - 1) as f64 {
                continue;
            }
            let s0 = src.floor() as usize;
            let fr = (src - s0 as f64) as f32;
            let v = x[s0] + (x[s0 + 1] - x[s0]) * fr;
            out[o as usize] += v * w as f32 * gain;
        }
        ts += ps.max(8.0);
    }
    out
}

/// Signalsmith Stretch, transposing block by block with its formant compensation (and any
/// formant shift). Its formant base is left to its own detection: setting it through the
/// 0.1.3 bindings silences the output.
#[cfg(not(target_arch = "wasm32"))]
fn signalsmith(x: &[f32], sr: u32, t: &Targets, offset: f64) -> Vec<f32> {
    const BLOCK: usize = 128;
    let srf = f64::from(sr.max(1));
    let mut st = signalsmith_stretch::Stretch::preset_default(1, sr);
    let out_latency = st.output_latency();
    let latency = st.input_latency() + out_latency;
    let total = x.len() + latency;
    let mut out = vec![0.0f32; total + BLOCK];
    let mut inp = vec![0.0f32; BLOCK];
    let mut pos = 0;
    while pos < total {
        // The output of this block is the input `latency` samples earlier, but a transposition
        // set now is heard an output latency later (measured: scheduling it by the whole
        // latency left a vibrato 37 cents off, by the output latency 4 cents).
        let at = (pos + BLOCK / 2).saturating_sub(out_latency) as f64 / srf;
        let r = t.ratio_at(offset + at);
        let k = t.formant_at(offset + at);
        st.set_transpose_factor(r, None);
        st.set_formant_factor(k, true);
        for (i, v) in inp.iter_mut().enumerate() {
            *v = x.get(pos + i).copied().unwrap_or(0.0);
        }
        st.process(&inp, &mut out[pos..pos + BLOCK]);
        pos += BLOCK;
    }
    out.drain(..latency.min(out.len()));
    out.truncate(x.len());
    out
}

/// Each stretch `(start, end, dB)` (seconds) made louder or softer, with 10 ms ramps.
pub fn apply_gains(stereo: &mut [f32], sample_rate: u32, gains: &[(f64, f64, f32)]) {
    let sr = f64::from(sample_rate.max(1));
    let frames = stereo.len() / 2;
    let ramp = (0.01 * sr) as usize;
    for (s, e, db) in gains {
        if !(s.is_finite() && e.is_finite() && db.is_finite()) || e <= s || db.abs() < 0.01 {
            continue;
        }
        let g = 10f32.powf(db.clamp(-96.0, 24.0) / 20.0);
        let (a, b) = (
            ((s.max(0.0) * sr) as usize).min(frames),
            ((e * sr) as usize).min(frames),
        );
        let (pa, pb) = (a.saturating_sub(ramp), (b + ramp).min(frames));
        for i in pa..pb {
            let w = if i < a {
                1.0 - (a - i) as f32 / ramp.max(1) as f32
            } else if i >= b {
                1.0 - (i - b + 1) as f32 / ramp.max(1) as f32
            } else {
                1.0
            };
            let gi = 1.0 + (g - 1.0) * w.clamp(0.0, 1.0);
            stereo[2 * i] *= gi;
            stereo[2 * i + 1] *= gi;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::melody::{find_notes, Algorithm};
    use crate::pitch_tracker::track_pitch;

    const SR: u32 = 44_100;

    /// A sung-like vowel: a pulse train with a 5.5 Hz vibrato through two formant resonances.
    fn vowel(midi: f64, secs: f64, vib: f64) -> Vec<f32> {
        let n = (secs * f64::from(SR)) as usize;
        let fs = f64::from(SR);
        let mut phase = 0.0;
        let mut ys = [[0.0f64; 2]; 3];
        (0..n)
            .map(|i| {
                let t = i as f64 / fs;
                let m = midi + vib * (std::f64::consts::TAU * 5.5 * t).sin();
                phase += 440.0 * 2f64.powf((m - 69.0) / 12.0) / fs;
                let mut s = if phase >= 1.0 {
                    phase -= 1.0;
                    1.0
                } else {
                    0.0
                };
                for (k, (f, bw)) in [(700.0, 90.0), (1220.0, 100.0), (2600.0, 150.0)]
                    .iter()
                    .enumerate()
                {
                    let r = (-std::f64::consts::PI * bw / fs).exp();
                    let c = 2.0 * r * (std::f64::consts::TAU * f / fs).cos();
                    let o = s + c * ys[k][0] - r * r * ys[k][1];
                    ys[k][1] = ys[k][0];
                    ys[k][0] = o;
                    s = o;
                }
                let env = (t / 0.02).min(1.0).min((secs - t) / 0.02);
                (s * env) as f32
            })
            .collect::<Vec<f32>>()
            .normalized()
    }

    trait Normalized {
        fn normalized(self) -> Vec<f32>;
    }

    impl Normalized for Vec<f32> {
        /// Peak at 0.5.
        fn normalized(self) -> Vec<f32> {
            let peak = self.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
            self.into_iter().map(|v| v * 0.5 / peak).collect()
        }
    }

    fn stereo(m: &[f32]) -> Vec<f32> {
        m.iter().flat_map(|v| [*v, *v]).collect()
    }

    fn median_midi(x: &[f32], a: f64, b: f64) -> f32 {
        let t = track_pitch(x, SR);
        let mut v: Vec<f32> = (PitchTrack::frame_at(a)..PitchTrack::frame_at(b))
            .filter_map(|i| t.midi(i))
            .collect();
        assert!(!v.is_empty());
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    }

    #[test]
    fn a_note_moved_up_three_semitones_sings_there_within_ten_cents() {
        let x = vowel(57.0, 1.2, 0.0);
        let st = stereo(&x);
        let found = find_notes(&x, SR, Algorithm::Melodic, &[]);
        assert_eq!(found.blobs.len(), 1);
        let b = found.blobs[0];
        let change = NoteChange {
            shift: 3.0,
            ..NoteChange::none(b.start, b.end)
        };
        let cache = SegmentCache::default();
        for engine in [Resynth::World, Resynth::Psola, Resynth::Signalsmith] {
            let (out, stats) = render_notes(&st, SR, &found.track, &[change], engine, &cache, 1);
            assert_eq!(out.len(), st.len());
            assert_eq!(stats.segments, 1);
            let mono: Vec<f32> = out.chunks(2).map(|c| c[0]).collect();
            let t = track_pitch(&mono, SR);
            let voiced = t.hz.iter().filter(|h| **h > 0.0).count();
            assert!(voiced > 100, "{engine:?}: {voiced} voiced frames");
            let m = median_midi(&mono, 0.3, 0.9);
            let want = b.pitch + 3.0;
            assert!(
                ((m - want) * 100.0).abs() < 10.0,
                "{engine:?}: {m} vs {want}"
            );
        }
    }

    #[test]
    fn a_formant_shift_leaves_the_pitch_alone() {
        let x = vowel(57.0, 1.0, 0.0);
        let st = stereo(&x);
        let found = find_notes(&x, SR, Algorithm::Melodic, &[]);
        let b = found.blobs[0];
        let change = NoteChange {
            formant: 2.0,
            ..NoteChange::none(b.start, b.end)
        };
        let cache = SegmentCache::default();
        let (out, _) = render_notes(&st, SR, &found.track, &[change], DEFAULT_RESYNTH, &cache, 1);
        let mono: Vec<f32> = out.chunks(2).map(|c| c[0]).collect();
        let m = median_midi(&mono, 0.3, 0.7);
        assert!(((m - b.pitch) * 100.0).abs() < 10.0, "{m} vs {}", b.pitch);
        // But the sound did change.
        let diff: f32 = out.iter().zip(&st).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 1.0);
    }

    #[test]
    fn vibrato_scaled_to_zero_is_flat_and_untouched_notes_stay_exact() {
        let mut x = vowel(57.0, 1.2, 0.5);
        x.extend(vec![0.0; 4410]);
        x.extend(vowel(60.0, 0.8, 0.5));
        let st = stereo(&x);
        let found = find_notes(&x, SR, Algorithm::Melodic, &[]);
        assert_eq!(found.blobs.len(), 2, "{:?}", found.blobs);
        let b = found.blobs[0];
        let change = NoteChange {
            modulation: 0.0,
            ..NoteChange::none(b.start, b.end)
        };
        let cache = SegmentCache::default();
        let (out, _) = render_notes(&st, SR, &found.track, &[change], DEFAULT_RESYNTH, &cache, 7);
        let mono: Vec<f32> = out.chunks(2).map(|c| c[0]).collect();
        let t = track_pitch(&mono, SR);
        let swing: Vec<f32> = (PitchTrack::frame_at(0.3)..PitchTrack::frame_at(0.9))
            .filter_map(|i| t.midi(i))
            .collect();
        let lo = swing.iter().copied().fold(f32::MAX, f32::min);
        let hi = swing.iter().copied().fold(f32::MIN, f32::max);
        assert!(hi - lo < 0.25, "still swings {} semitones", hi - lo);
        // The second note was not touched: its samples are the recording's.
        let second = (1.4 * f64::from(SR)) as usize * 2;
        assert!(out[second..].iter().zip(&st[second..]).all(|(a, b)| a == b));
        // Made again with the same edit: from the cache.
        let (again, stats) =
            render_notes(&st, SR, &found.track, &[change], DEFAULT_RESYNTH, &cache, 7);
        assert_eq!((stats.segments, stats.made), (1, 0));
        assert_eq!(again, out);
    }

    #[test]
    fn gains_ramp_in_and_out() {
        let mut s = vec![0.5f32; 44_100 * 2];
        apply_gains(&mut s, SR, &[(0.25, 0.5, -6.0206)]);
        assert!((s[2 * 15_000] - 0.25).abs() < 1e-3);
        assert_eq!(s[2 * 30_000], 0.5);
        assert!(s.iter().all(|v| v.is_finite()));
        apply_gains(&mut s, SR, &[(f64::NAN, 1.0, 3.0), (0.5, 0.1, 3.0)]);
    }
}
