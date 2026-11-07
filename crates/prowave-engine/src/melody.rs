//! The notes of a recording as the Pitch Editor shows them ("blobs", as Melodyne calls them;
//! Melodyne 5 manual, "Audio characteristics and algorithms"):
//!
//! - **Melodic** (one note at a time: a voice, a bass, a horn): the voiced stretches of the
//!   pitch curve ([`crate::track_pitch`]), split where the pitch settles on another note or the
//!   level dips for a new syllable; the unvoiced sound just before a note (a consonant, a
//!   breath) belongs to it, as Melodyne's sibilants do.
//! - **Percussive** (drums, noise): only the hits, each a blob from its onset to the next one,
//!   all at one height.
//! - **Universal** (anything, moved as a whole): slices between the onsets like Percussive,
//!   with the pitch curve still drawn and used where there is one.
//!
//! Each Melodic note's pitch curve is taken apart as Melodyne does ("Pitch modulation and
//! drift"): its **pitch centre** (the note it is heard as), its **drift** (the slow wandering
//! around the centre) and its **modulation** (the fast part: vibrato, trills).
use crate::pitch_tracker::{PitchTrack, PITCH_HOP};

/// How the notes are found (Melodyne's detection algorithms).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Algorithm {
    #[default]
    Melodic,
    Percussive,
    Universal,
}

impl Algorithm {
    pub const ALL: [Algorithm; 3] = [
        Algorithm::Melodic,
        Algorithm::Percussive,
        Algorithm::Universal,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Algorithm::Melodic => "Melodic",
            Algorithm::Percussive => "Percussive",
            Algorithm::Universal => "Universal",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == s)
    }

    /// Whether its notes have pitches of their own (else they are drawn on one line and moved
    /// up or down relative to where they are).
    pub fn pitched(self) -> bool {
        self == Algorithm::Melodic
    }
}

/// A note found in the recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blob {
    /// Seconds from the start of the file.
    pub start: f64,
    pub end: f64,
    /// The pitch centre, as a MIDI note with its fraction; 0 for a note without a pitch.
    pub pitch: f32,
    /// Where the note's pitch begins (after a consonant): its musical start, for Quantize Time.
    pub onset: f64,
}

/// The notes of a recording with the pitch curve they were found from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteAnalysis {
    pub track: PitchTrack,
    pub blobs: Vec<Blob>,
}

/// Bridged: unvoiced gaps inside a note at most this long (s).
const GAP: f64 = 0.03;
/// The shortest note kept (s).
const SHORTEST: f64 = 0.05;
/// How far a consonant before a note may reach back (s).
const CONSONANT: f64 = 0.15;
/// Two neighbouring notes closer than this in pitch centre (semitones) are one note.
const SAME_NOTE: f32 = 0.5;
/// A dip in level of at least this much (dB) inside a sung stretch starts a new syllable.
const SYLLABLE_DIP: f32 = 6.0;

/// Finds the notes of a mono recording. `onsets` are its hits (seconds, from
/// [`crate::Analyzer`]); Melodic finds its own boundaries and only uses them for nothing.
pub fn find_notes(
    mono: &[f32],
    sample_rate: u32,
    algorithm: Algorithm,
    onsets: &[f64],
) -> NoteAnalysis {
    let track = crate::pitch_tracker::track_pitch(mono, sample_rate);
    let len = mono.len() as f64 / f64::from(sample_rate.max(1));
    let blobs = match algorithm {
        Algorithm::Melodic => notes_from_track(&track),
        Algorithm::Percussive | Algorithm::Universal => slices(&track, onsets, len),
    };
    NoteAnalysis { track, blobs }
}

/// A `width`-frame moving average (shrinking at the ends), skipping `None`s.
fn smooth(v: &[f32], width: usize) -> Vec<f32> {
    let half = width / 2;
    let mut prefix = vec![0.0f64; v.len() + 1];
    for (i, x) in v.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(*x);
    }
    (0..v.len())
        .map(|i| {
            let a = i.saturating_sub(half);
            let b = (i + half + 1).min(v.len());
            ((prefix[b] - prefix[a]) / (b - a).max(1) as f64) as f32
        })
        .collect()
}

/// The pitch of frames `a..b` as MIDI, gaps filled in straight lines (ends held).
fn filled_midi(track: &PitchTrack, a: usize, b: usize) -> Vec<f32> {
    let raw: Vec<Option<f32>> = (a..b).map(|i| track.midi(i)).collect();
    let mut out = vec![0.0f32; raw.len()];
    let first = raw.iter().position(Option::is_some);
    let Some(first) = first else { return out };
    let mut last = (first, raw[first].unwrap_or(0.0));
    for (i, v) in raw.iter().enumerate() {
        match v {
            Some(m) => {
                if i > last.0 + 1 {
                    for (k, o) in out.iter_mut().enumerate().take(i).skip(last.0 + 1) {
                        let f = (k - last.0) as f32 / (i - last.0) as f32;
                        *o = last.1 + (m - last.1) * f;
                    }
                }
                out[i] = *m;
                last = (i, *m);
            }
            None if i < first => {}
            None => out[i] = last.1,
        }
    }
    for o in out.iter_mut().take(first) {
        *o = raw[first].unwrap_or(0.0);
    }
    out
}

/// The slow part of a pitch curve: two 200 ms moving averages (they null 5 Hz and its
/// multiples, so a vibrato is left out).
pub fn slow_part(midi: &[f32]) -> Vec<f32> {
    let w = (0.2 / PITCH_HOP).round() as usize | 1;
    smooth(&smooth(midi, w), w)
}

/// The pitch centre of frames `a..b`: the mean of the middle of the note, weighted by level,
/// as the ear hears it (the ends are where the voice slides in and out).
pub fn pitch_centre(track: &PitchTrack, a: usize, b: usize) -> Option<f32> {
    let voiced: Vec<usize> = (a..b.min(track.len()))
        .filter(|i| track.midi(*i).is_some())
        .collect();
    if voiced.is_empty() {
        return None;
    }
    let trim = voiced.len() * 15 / 100;
    let mid = &voiced[trim..voiced.len() - trim];
    let mid = if mid.is_empty() { &voiced[..] } else { mid };
    let (mut s, mut w) = (0.0f64, 0.0f64);
    for i in mid {
        let lv = 10f64.powf(f64::from(track.level_db[*i]) / 20.0);
        s += f64::from(track.midi(*i).unwrap_or(0.0)) * lv;
        w += lv;
    }
    (w > 0.0).then(|| (s / w) as f32)
}

/// The pitch curve of a note taken apart: for each frame of `a..b`, its drift and its
/// modulation in semitones (`curve = centre + drift + modulation`), `None` where unvoiced.
pub fn decompose(track: &PitchTrack, a: usize, b: usize, centre: f32) -> Vec<Option<(f32, f32)>> {
    let b = b.min(track.len());
    if b <= a {
        return Vec::new();
    }
    let m = filled_midi(track, a, b);
    let slow = slow_part(&m);
    (a..b)
        .map(|i| {
            track
                .midi(i)
                .map(|v| (slow[i - a] - centre, v - slow[i - a]))
        })
        .collect()
}

/// The Melodic notes of a pitch curve.
pub fn notes_from_track(track: &PitchTrack) -> Vec<Blob> {
    let n = track.len();
    let gap = (GAP / PITCH_HOP).round() as usize;
    let shortest = (SHORTEST / PITCH_HOP).round() as usize;
    // Voiced runs, short gaps bridged.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < n {
        if track.hz[i] <= 0.0 {
            i += 1;
            continue;
        }
        let a = i;
        let mut b = i;
        let mut j = i;
        while j < n {
            if track.hz[j] > 0.0 {
                b = j + 1;
                j += 1;
            } else if j - b < gap {
                j += 1;
            } else {
                break;
            }
        }
        if b - a >= shortest {
            runs.push((a, b));
        }
        i = j.max(i + 1);
    }
    let mut notes: Vec<(usize, usize)> = Vec::new();
    for (a, b) in runs {
        for (x, y) in split_run(track, a, b, shortest) {
            notes.push((x, y));
        }
    }
    // Each note's consonant: the loud unvoiced frames just before it, back to the last note.
    let mut blobs = Vec::new();
    let mut prev_end = 0usize;
    let reach = (CONSONANT / PITCH_HOP).round() as usize;
    for (a, b) in notes {
        let mut s = a;
        if a > prev_end {
            while s > prev_end
                && a - s < reach
                && track.hz[s - 1] <= 0.0
                && track.level_db[s - 1] > -45.0
            {
                s -= 1;
            }
        }
        let Some(pitch) = pitch_centre(track, a, b) else {
            continue;
        };
        blobs.push(Blob {
            start: PitchTrack::time_of(s),
            end: PitchTrack::time_of(b),
            pitch,
            onset: PitchTrack::time_of(a),
        });
        prev_end = b;
    }
    blobs
}

/// A voiced run split into notes: an optimal piecewise-constant fit of its slow pitch curve
/// (each extra note costs a penalty), then neighbours on the same note joined again, then
/// splits where the level dips for a new syllable.
fn split_run(track: &PitchTrack, a: usize, b: usize, shortest: usize) -> Vec<(usize, usize)> {
    // The raw curve: a vibrato adds the same cost to any fit (it averages out within a
    // note), and a step between legato notes stays a step.
    let s = filled_midi(track, a, b);
    let n = s.len();
    let mut prefix = vec![0.0f64; n + 1];
    let mut prefix2 = vec![0.0f64; n + 1];
    for (i, v) in s.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(*v);
        prefix2[i + 1] = prefix2[i] + f64::from(*v) * f64::from(*v);
    }
    let cost = |i: usize, j: usize| -> f64 {
        let len = (j - i) as f64;
        let sum = prefix[j] - prefix[i];
        (prefix2[j] - prefix2[i]) - sum * sum / len
    };
    const PENALTY: f64 = 4.0;
    // best[j]: the cheapest fit of the first j frames; from[j]: where its last note begins.
    let mut best = vec![f64::INFINITY; n + 1];
    let mut from = vec![0usize; n + 1];
    best[0] = 0.0;
    for j in 1..=n {
        let lo = j.saturating_sub(2000);
        for i in lo..j {
            if best[i].is_infinite() {
                continue;
            }
            // Notes are at least `shortest` long, except a whole run that is shorter.
            if j - i < shortest && !(i == 0 && j == n) {
                continue;
            }
            let c = best[i] + cost(i, j) + PENALTY;
            if c < best[j] {
                best[j] = c;
                from[j] = i;
            }
        }
    }
    let mut cuts = Vec::new();
    let mut j = n;
    while j > 0 {
        if best[j].is_infinite() {
            cuts.clear();
            break;
        }
        cuts.push(j);
        j = from[j];
    }
    cuts.reverse();
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for c in cuts {
        pieces.push((start, c));
        start = c;
    }
    if pieces.is_empty() {
        pieces.push((0, n));
    }
    // Neighbours heard as the same note are one note (a drift, not a new note).
    let centre = |p: (usize, usize)| pitch_centre(track, a + p.0, a + p.1).unwrap_or(0.0);
    let mut joined: Vec<(usize, usize)> = Vec::new();
    for p in pieces {
        match joined.last_mut() {
            Some(last) if (centre(*last) - centre(p)).abs() < SAME_NOTE => last.1 = p.1,
            _ => joined.push(p),
        }
    }
    // Syllables on one pitch: a dip in level between two louder stretches.
    let mut out = Vec::new();
    for (x, y) in joined {
        let mut start = x;
        let lv: Vec<f32> = (a + x..a + y).map(|i| track.level_db[i]).collect();
        let around = (0.08 / PITCH_HOP) as usize;
        let mut k = shortest;
        while k + shortest < lv.len() {
            let left = lv[k.saturating_sub(around)..k]
                .iter()
                .copied()
                .fold(f32::MIN, f32::max);
            let right = lv[k + 1..(k + 1 + around).min(lv.len())]
                .iter()
                .copied()
                .fold(f32::MIN, f32::max);
            let is_min = lv[k] <= lv[k - 1] && lv[k] <= lv[k + 1];
            if is_min
                && left - lv[k] >= SYLLABLE_DIP
                && right - lv[k] >= SYLLABLE_DIP
                && x + k - start >= shortest
            {
                out.push((a + start, a + x + k));
                start = x + k;
                k += shortest;
            } else {
                k += 1;
            }
        }
        out.push((a + start, a + y));
    }
    out
}

/// Percussive and Universal: a blob from each hit to the next (or to where the sound has
/// died away), at the pitch centre where there is one.
fn slices(track: &PitchTrack, onsets: &[f64], len: f64) -> Vec<Blob> {
    let mut times: Vec<f64> = onsets
        .iter()
        .copied()
        .filter(|t| t.is_finite() && *t >= 0.0 && *t < len)
        .collect();
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < SHORTEST);
    let mut out = Vec::new();
    for (k, t) in times.iter().enumerate() {
        let next = times.get(k + 1).copied().unwrap_or(len);
        // Where it has died away: 40 dB under its loudest, or the next hit.
        let (fa, fb) = (
            PitchTrack::frame_at(*t),
            PitchTrack::frame_at(next).min(track.len()),
        );
        let peak = (fa..fb).map(|i| track.level_db[i]).fold(f32::MIN, f32::max);
        let end = (fa..fb)
            .skip(4)
            .find(|i| track.level_db[*i] < peak - 40.0)
            .map_or(next, PitchTrack::time_of);
        if end - t < 0.01 {
            continue;
        }
        out.push(Blob {
            start: *t,
            end,
            pitch: 0.0,
            onset: *t,
        });
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    /// Notes (start, end, MIDI) as a harmonic tone, with a 5.5 Hz vibrato of `vib` semitones
    /// and silence between the notes that do not touch.
    pub(crate) fn melody(notes: &[(f64, f64, f32)], secs: f64, vib: f64) -> Vec<f32> {
        let n = (secs * f64::from(SR)) as usize;
        let mut phase = 0.0f64;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(SR);
                let Some((a, b, m)) = notes.iter().find(|(a, b, _)| t >= *a && t < *b) else {
                    return 0.0;
                };
                let m = f64::from(*m) + vib * (std::f64::consts::TAU * 5.5 * (t - a)).sin();
                let hz = 440.0 * 2f64.powf((m - 69.0) / 12.0);
                phase += std::f64::consts::TAU * hz / f64::from(SR);
                // A short fade in and out, as a singer's note has.
                let env = ((t - a) / 0.01).min(1.0).min((b - t) / 0.01);
                let s = phase.sin() + 0.5 * (2.0 * phase).sin() + 0.3 * (3.0 * phase).sin();
                (s * 0.25 * env) as f32
            })
            .collect()
    }

    #[test]
    fn a_melody_is_cut_into_its_notes() {
        // Four notes: two apart, then a legato step (no gap), then a jump after a rest.
        let notes = [
            (0.10, 0.50, 60.0f32),
            (0.60, 1.00, 64.0),
            (1.00, 1.40, 62.0),
            (1.60, 2.20, 67.0),
        ];
        let x = melody(&notes, 2.4, 0.4);
        let found = find_notes(&x, SR, Algorithm::Melodic, &[]);
        let b = &found.blobs;
        assert_eq!(b.len(), 4, "{b:?}");
        for (blob, (s, e, m)) in b.iter().zip(notes) {
            assert!((blob.pitch - m).abs() < 0.1, "{blob:?} vs {m}");
            assert!((blob.onset - s).abs() < 0.03, "start {blob:?} vs {s}");
            assert!((blob.end - e).abs() < 0.05, "end {blob:?} vs {e}");
        }
    }

    #[test]
    fn the_curve_comes_apart_into_centre_drift_and_modulation() {
        // A4 drifting up by 0.4 semitones over the note, with a ±0.5 semitone vibrato.
        let secs = 1.2;
        let n = (secs * f64::from(SR)) as usize;
        let mut phase = 0.0f64;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(SR);
                let m = 69.0 + 0.4 * t / secs + 0.5 * (std::f64::consts::TAU * 5.5 * t).sin();
                phase +=
                    std::f64::consts::TAU * 440.0 * 2f64.powf((m - 69.0) / 12.0) / f64::from(SR);
                (phase.sin() * 0.3) as f32
            })
            .collect();
        let found = find_notes(&x, SR, Algorithm::Melodic, &[]);
        assert_eq!(found.blobs.len(), 1, "{:?}", found.blobs);
        let blob = found.blobs[0];
        assert!((blob.pitch - 69.2).abs() < 0.1, "{blob:?}");
        let (a, b) = (
            PitchTrack::frame_at(blob.start),
            PitchTrack::frame_at(blob.end),
        );
        let parts = decompose(&found.track, a, b, blob.pitch);
        // In the middle: the drift is slow and small, the modulation swings ±0.5.
        let mid = &parts[parts.len() / 4..parts.len() * 3 / 4];
        let md = mid
            .iter()
            .flatten()
            .map(|p| p.1.abs())
            .fold(0.0f32, f32::max);
        let dd = mid
            .iter()
            .flatten()
            .map(|p| p.0.abs())
            .fold(0.0f32, f32::max);
        assert!((md - 0.5).abs() < 0.12, "modulation peak {md}");
        assert!(dd < 0.3, "drift peak {dd}");
    }

    #[test]
    fn percussive_slices_follow_the_hits() {
        let mut x = vec![0.0f32; SR as usize];
        for hit in [0.1, 0.4, 0.7] {
            let a = (hit * f64::from(SR)) as usize;
            for k in 0..2000 {
                x[a + k] = 0.8 * (-(k as f32) / 300.0).exp() * if k % 2 == 0 { 1.0 } else { -1.0 };
            }
        }
        let found = find_notes(&x, SR, Algorithm::Percussive, &[0.1, 0.4, 0.7]);
        assert_eq!(found.blobs.len(), 3);
        assert!(found
            .blobs
            .iter()
            .all(|b| b.pitch == 0.0 && b.end > b.start));
        assert!((found.blobs[1].start - 0.4).abs() < 1e-9);
        assert!(found.blobs[0].end <= 0.4 + 1e-9);
        for a in Algorithm::ALL {
            assert_eq!(Algorithm::from_name(a.name()), Some(a));
        }
    }
}
