//! Offline work on samples, run on background threads: decoding a file, finding where to
//! chop it, preparing a sound (cut, reversed, faded, normalised, at a sampling grade,
//! stretched), and guessing tempo and key.
use crate::sample_buf::SampleBuf;
use std::path::Path;

/// Reads a whole file as stereo at its own rate. `progress` hears 0 to 1 and can stop it.
pub fn decode(path: &Path, progress: impl FnMut(f32) -> bool) -> Result<SampleBuf, String> {
    decode_with_head(path, 0.0, |_| {}, progress)
}

/// [`decode`], also handing over the first `head` seconds as soon as they are decoded (so a
/// preview can start long before a whole song is read).
pub fn decode_with_head(
    path: &Path,
    head: f64,
    mut on_head: impl FnMut(SampleBuf),
    mut progress: impl FnMut(f32) -> bool,
) -> Result<SampleBuf, String> {
    let file = crate::disk::AudioFile::open(path).map_err(|e| e.to_string())?;
    let rate = file.sample_rate;
    let want = (head * f64::from(rate)) as usize * 2;
    let mut sent = head <= 0.0;
    let data = file
        .read_stereo_with(|p, so_far| {
            if !sent && (so_far.len() >= want || p >= 1.0) {
                sent = true;
                on_head(SampleBuf::new(rate, so_far.to_vec()));
            }
            progress(p)
        })
        .map_err(|e| e.to_string())?
        .ok_or("stopped")?;
    Ok(SampleBuf::new(rate, data))
}

fn mono(buf: &SampleBuf, i: usize) -> f32 {
    (buf.data[i * 2] + buf.data[i * 2 + 1]) * 0.5
}

/// Where hits start in frames `start..end` (transient chop). `sensitivity` 0 to 1: higher
/// finds softer hits. Each is placed on the attack itself, to the sample.
pub fn transients(buf: &SampleBuf, start: usize, end: usize, sensitivity: f32) -> Vec<usize> {
    let end = end.min(buf.frames());
    if end <= start + 2 {
        return Vec::new();
    }
    let rate = buf.rate.max(1) as usize;
    let hop = (rate / 1000).max(1); // 1 ms
    let hops = (end - start) / hop;
    // Log energy of the first difference (the attacks, not the hum) per hop.
    let mut level = Vec::with_capacity(hops);
    let mut prev = mono(buf, start);
    for h in 0..hops {
        let mut e = 0.0f64;
        for i in start + h * hop..start + (h + 1) * hop {
            let x = mono(buf, i);
            let d = f64::from(x - prev);
            prev = x;
            e += d * d;
        }
        level.push(10.0 * (e / hop as f64 + 1e-12).log10());
    }
    let loudest = level.iter().copied().fold(f64::MIN, f64::max);
    let rise_db = 3.0 + (1.0 - f64::from(sensitivity.clamp(0.0, 1.0))) * 15.0;
    let floor = loudest - 20.0 - f64::from(sensitivity.clamp(0.0, 1.0)) * 30.0;
    let min_gap = 40; // hops (ms)
    let mut out: Vec<usize> = Vec::new();
    let mut last: Option<usize> = None;
    for h in 1..hops {
        let lo = h.saturating_sub(10);
        let before = level[lo..h].iter().copied().fold(f64::MIN, f64::max);
        let rise = level[h] - before;
        if rise > rise_db && level[h] > floor && last.is_none_or(|l| h >= l + min_gap) {
            last = Some(h);
            // To the sample: the first one past a fifth of the hit's peak.
            let a = start + (h - 1) * hop;
            let b = (start + (h + 4) * hop).min(end);
            let peak = (a..b).map(|i| mono(buf, i).abs()).fold(0.0, f32::max);
            let at = (a..b)
                .find(|i| mono(buf, *i).abs() >= peak * 0.2)
                .unwrap_or(a);
            out.push(at);
        }
    }
    out
}

/// `n` slices of equal length over `start..end`: their starts (the first is `start`).
pub fn equal_divisions(start: usize, end: usize, n: usize) -> Vec<usize> {
    let n = n.max(1);
    let len = end.saturating_sub(start) as u128;
    (0..n)
        .map(|k| start + (len * k as u128 / n as u128) as usize)
        .collect()
}

/// Slices every `beats` beats at `bpm` from `start`.
pub fn beat_divisions(start: usize, end: usize, rate: u32, bpm: f64, beats: f64) -> Vec<usize> {
    if !(bpm > 0.0 && beats > 0.0) {
        return vec![start];
    }
    let step = beats * 60.0 / bpm * f64::from(rate);
    let mut out = Vec::new();
    let mut k = 0usize;
    loop {
        let at = start + (k as f64 * step).round() as usize;
        if at >= end || out.len() >= 4096 {
            break;
        }
        out.push(at);
        k += 1;
    }
    out
}

/// The zero crossing of the mono mix nearest `frame` (within `radius` frames), else `frame`.
pub fn zero_crossing(buf: &SampleBuf, frame: usize, radius: usize) -> usize {
    let n = buf.frames();
    if n < 2 {
        return frame.min(n);
    }
    let frame = frame.min(n - 1);
    for d in 0..=radius {
        for i in [frame.saturating_sub(d), frame + d] {
            if i >= 1 && i < n {
                let (a, b) = (mono(buf, i - 1), mono(buf, i));
                if a == 0.0 || b == 0.0 || (a < 0.0) != (b < 0.0) {
                    return i;
                }
            }
        }
    }
    frame
}

/// Sampling grades: STANDARD 44.1 kHz, LONG 22.05 kHz, LO-FI 11.025 kHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Grade {
    #[default]
    Standard,
    Long,
    LoFi,
}

impl Grade {
    pub fn rate(self) -> u32 {
        match self {
            Grade::Standard => 44_100,
            Grade::Long => 22_050,
            Grade::LoFi => 11_025,
        }
    }
}

/// How a pad's sound is prepared from its source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliceEdit {
    pub start: usize,
    pub end: usize,
    pub gain_db: f32,
    pub reverse: bool,
    /// Seconds.
    pub fade_in: f32,
    pub fade_out: f32,
    pub normalize: bool,
    pub grade: Grade,
    /// STEREO off: both sides the mono mix.
    pub stereo: bool,
}

impl Default for SliceEdit {
    fn default() -> Self {
        Self {
            start: 0,
            end: usize::MAX,
            gain_db: 0.0,
            reverse: false,
            fade_in: 0.0,
            fade_out: 0.0,
            normalize: false,
            grade: Grade::Standard,
            stereo: true,
        }
    }
}

/// Brings `data` (stereo) from `from` Hz to `to` Hz with a windowed-sinc low-pass, for the
/// LONG and LO-FI grades (downwards only).
fn downsample(data: &[f32], from: u32, to: u32) -> Vec<f32> {
    if to >= from {
        return data.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let frames = data.len() / 2;
    let out_frames = (frames as f64 / ratio) as usize;
    let half = (8.0 * ratio).ceil() as isize;
    let cutoff = 0.9 / ratio;
    let mut out = Vec::with_capacity(out_frames * 2);
    for o in 0..out_frames {
        let c = o as f64 * ratio;
        let ci = c.floor() as isize;
        let (mut l, mut r, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
        for k in ci - half..=ci + half {
            if k < 0 || k as usize >= frames {
                continue;
            }
            let x = k as f64 - c;
            let s = if x.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * cutoff * x)
            };
            let t = (x / half as f64).clamp(-1.0, 1.0);
            let w = 0.42
                + 0.5 * (std::f64::consts::PI * t).cos()
                + 0.08 * (2.0 * std::f64::consts::PI * t).cos();
            let h = s * w;
            l += f64::from(data[k as usize * 2]) * h;
            r += f64::from(data[k as usize * 2 + 1]) * h;
            wsum += h;
        }
        let wsum = if wsum.abs() < 1e-9 { 1.0 } else { wsum };
        out.push((l / wsum) as f32);
        out.push((r / wsum) as f32);
    }
    out
}

/// A pad's sound from its source.
pub fn render_slice(src: &SampleBuf, e: &SliceEdit) -> SampleBuf {
    let n = src.frames();
    let start = e.start.min(n);
    let end = e.end.min(n).max(start);
    let mut data = src.data[start * 2..end * 2].to_vec();
    if !e.stereo {
        for f in data.as_chunks_mut::<2>().0 {
            let m = (f[0] + f[1]) * 0.5;
            f[0] = m;
            f[1] = m;
        }
    }
    let mut rate = src.rate;
    let target = e.grade.rate();
    if e.grade != Grade::Standard && target < rate {
        data = downsample(&data, rate, target);
        rate = target;
    }
    if e.reverse {
        let frames = data.len() / 2;
        for i in 0..frames / 2 {
            let j = frames - 1 - i;
            data.swap(i * 2, j * 2);
            data.swap(i * 2 + 1, j * 2 + 1);
        }
    }
    let frames = data.len() / 2;
    let fi = ((e.fade_in.max(0.0) * rate as f32) as usize).min(frames);
    let fo = ((e.fade_out.max(0.0) * rate as f32) as usize).min(frames);
    for i in 0..fi {
        let g = i as f32 / fi as f32;
        data[i * 2] *= g;
        data[i * 2 + 1] *= g;
    }
    for i in 0..fo {
        let g = i as f32 / fo as f32;
        let j = frames - 1 - i;
        data[j * 2] *= g;
        data[j * 2 + 1] *= g;
    }
    let mut gain = 10f32.powf(e.gain_db.clamp(-60.0, 24.0) / 20.0);
    if e.normalize {
        let peak = data.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        if peak > 1e-6 {
            gain = 1.0 / peak * gain.min(1.0);
        }
    }
    if gain != 1.0 {
        for x in &mut data {
            *x *= gain;
        }
    }
    SampleBuf::new(rate, data)
}

/// The sound made `time` times as long and `semis` higher, with its pitch and speed kept
/// apart (Signalsmith Stretch). Off the audio thread.
pub fn stretch(buf: &SampleBuf, time: f64, semis: f32) -> SampleBuf {
    let frames = buf.frames();
    let out_frames = ((frames as f64) * time.clamp(0.05, 20.0)).round() as usize;
    if frames == 0 || out_frames == 0 {
        return SampleBuf::new(buf.rate, Vec::new());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut st = signalsmith_stretch::Stretch::preset_default(2, buf.rate.max(1));
        if semis.abs() > 1e-4 {
            st.set_transpose_factor_semitones(semis, None);
        }
        let mut out = vec![0.0f32; out_frames * 2];
        if st.exact(&buf.data[..frames * 2], &mut out) && out.iter().all(|x| x.is_finite()) {
            return SampleBuf::new(buf.rate, out);
        }
    }
    // No stretcher (browser build) or it failed: plain speed change.
    let _ = semis;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let pos = i as f64 / time;
        let j = (pos as usize).min(frames - 1);
        out.push(buf.data[j * 2]);
        out.push(buf.data[j * 2 + 1]);
    }
    SampleBuf::new(buf.rate, out)
}

/// The tempo of the music in `buf` (onset autocorrelation, as Smart Tempo uses).
pub fn estimate_bpm(buf: &SampleBuf) -> Option<f64> {
    let mut a = crate::analysis::Analyzer::new(buf.rate);
    for i in 0..buf.frames() {
        if a.is_full() {
            break;
        }
        a.push(mono(buf, i));
    }
    a.finish().tempo.map(|t| t.bpm)
}

/// A loop's tempo from its length, taking the number of beats (a power of two) that puts it
/// nearest the middle of 70–140 BPM. It may be half or double the felt tempo.
pub fn loop_bpm(seconds: f64) -> Option<f64> {
    if seconds <= 0.05 {
        return None;
    }
    let mut best: Option<f64> = None;
    for k in 0..8 {
        let beats = f64::from(1u32 << k);
        let bpm = beats * 60.0 / seconds;
        if (40.0..=200.0).contains(&bpm)
            && best.is_none_or(|b| (bpm.ln() - 100f64.ln()).abs() < (b.ln() - 100f64.ln()).abs())
        {
            best = Some(bpm);
        }
    }
    best
}

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The key of the music in `buf` (chroma against the Krumhansl–Kessler profiles), as "C",
/// "F#m", ... `None` for silence.
pub fn estimate_key(buf: &SampleBuf) -> Option<String> {
    let n = 8192;
    let fft = crate::spectral::Fft::new(n);
    let frames = buf.frames().min(buf.rate as usize * 90);
    let mut chroma = [0f64; 12];
    let mut re = vec![0f32; n];
    let mut im = vec![0f32; n];
    let rate = f64::from(buf.rate.max(1));
    let mut at = 0;
    while at + n <= frames {
        for (i, (r, m)) in re.iter_mut().zip(im.iter_mut()).enumerate() {
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
            *r = mono(buf, at + i) * w;
            *m = 0.0;
        }
        fft.process(&mut re, &mut im, false);
        for k in 1..n / 2 {
            let hz = k as f64 * rate / n as f64;
            if !(55.0..=2_000.0).contains(&hz) {
                continue;
            }
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            let pc = (midi.round() as i64).rem_euclid(12) as usize;
            chroma[pc] += f64::from(re[k].hypot(im[k]));
        }
        at += n / 2;
    }
    if chroma.iter().sum::<f64>() < 1e-6 {
        return None;
    }
    const MAJOR: [f64; 12] = [
        6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
    ];
    const MINOR: [f64; 12] = [
        6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
    ];
    let corr = |profile: &[f64; 12], root: usize| {
        let (mx, my) = (
            chroma.iter().sum::<f64>() / 12.0,
            profile.iter().sum::<f64>() / 12.0,
        );
        let (mut num, mut dx, mut dy) = (0.0, 0.0, 0.0);
        for i in 0..12 {
            let x = chroma[(i + root) % 12] - mx;
            let y = profile[i] - my;
            num += x * y;
            dx += x * x;
            dy += y * y;
        }
        num / (dx * dy).sqrt().max(1e-12)
    };
    let mut best = (f64::MIN, 0, false);
    for root in 0..12 {
        for (minor, p) in [(false, &MAJOR), (true, &MINOR)] {
            let c = corr(p, root);
            if c > best.0 {
                best = (c, root, minor);
            }
        }
    }
    Some(format!(
        "{}{}",
        NAMES[best.1],
        if best.2 { "m" } else { "" }
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    fn clicks(times: &[f64], seconds: f64) -> SampleBuf {
        let n = (seconds * f64::from(SR)) as usize;
        let mut data = vec![0.0f32; n * 2];
        // A little noise floor so it is not digital silence.
        let mut seed = 1u32;
        for x in data.iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *x = ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.002;
        }
        for t in times {
            let at = (t * f64::from(SR)) as usize;
            for i in 0..2000 {
                let env = (-(i as f32) / 300.0).exp();
                let s = (i as f32 * 0.3).sin() * env * 0.8;
                data[(at + i) * 2] += s;
                data[(at + i) * 2 + 1] += s;
            }
        }
        SampleBuf::new(SR, data)
    }

    #[test]
    fn transient_chop_finds_the_clicks_within_5_ms() {
        let times = [0.25, 0.61, 1.0, 1.333, 1.9, 2.4];
        let buf = clicks(&times, 3.0);
        let found = transients(&buf, 0, buf.frames(), 0.5);
        assert_eq!(found.len(), times.len(), "{found:?}");
        for (f, t) in found.iter().zip(times) {
            let err = (*f as f64 / f64::from(SR) - t).abs();
            assert!(err < 0.005, "{f} vs {t}: {} ms", err * 1000.0);
        }
    }

    #[test]
    fn an_mp3_and_a_wav_decode() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut heads = 0;
        let mp3 =
            decode_with_head(&fixtures.join("tone.mp3"), 0.1, |_| heads += 1, |_| true).unwrap();
        assert_eq!(heads, 1, "the head came first");
        assert_eq!(mp3.rate, 44_100);
        assert!((mp3.seconds() - 0.5).abs() < 0.08, "{}", mp3.seconds());
        assert!(mp3.data.iter().any(|x| x.abs() > 0.1));
        // A WAV written here: 0.25 s of 1 kHz.
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("t.wav");
        let tone = crate::sample_buf::tests::sine(1000.0, 12_000, 48_000);
        crate::flex::write_wav(&wav, &tone, 48_000).unwrap();
        let back = decode(&wav, |_| true).unwrap();
        assert_eq!((back.rate, back.frames()), (48_000, 12_000));
        assert!((back.data[200] - tone[200]).abs() < 1e-6);
    }

    #[test]
    fn equal_divisions_are_exact() {
        for (start, end, n) in [
            (0usize, 44_100usize, 16usize),
            (123, 98_765, 7),
            (0, 10, 64),
        ] {
            let starts = equal_divisions(start, end, n);
            assert_eq!(starts.len(), n);
            assert_eq!(starts[0], start);
            let mut bounds = starts.clone();
            bounds.push(end);
            let lens: Vec<usize> = bounds.windows(2).map(|w| w[1] - w[0]).collect();
            assert_eq!(lens.iter().sum::<usize>(), end - start);
            let (lo, hi) = (lens.iter().min().unwrap(), lens.iter().max().unwrap());
            assert!(hi - lo <= 1, "{lens:?}");
            for (k, s) in starts.iter().enumerate() {
                assert_eq!(*s, start + (end - start) * k / n);
            }
        }
        // 2 s at 44.1 kHz in 8: exactly 11 025 frames each.
        let s = equal_divisions(0, 88_200, 8);
        assert!(s.windows(2).all(|w| w[1] - w[0] == 11_025));
    }

    #[test]
    fn beat_divisions_follow_the_tempo() {
        // 90 BPM, one beat: every 2/3 s.
        let s = beat_divisions(0, 4 * SR as usize, SR, 90.0, 1.0);
        assert_eq!(s.len(), 6);
        assert_eq!(s[3], 88_200);
    }

    #[test]
    fn zero_crossings_are_found_nearby() {
        let mut data = Vec::new();
        for i in 0..1000 {
            let s = (i as f32 * 0.05).sin();
            data.push(s);
            data.push(s);
        }
        let buf = SampleBuf::new(SR, data);
        let z = zero_crossing(&buf, 70, 50);
        // sin(0.05 i) crosses zero at i = 62.8.
        assert_eq!(z, 63);
    }

    #[test]
    fn a_slice_is_cut_reversed_faded_and_normalised() {
        let data: Vec<f32> = (0..200).flat_map(|i| [i as f32 / 400.0; 2]).collect();
        let buf = SampleBuf::new(SR, data);
        let e = SliceEdit {
            start: 100,
            end: 150,
            reverse: true,
            normalize: true,
            ..SliceEdit::default()
        };
        let out = render_slice(&buf, &e);
        assert_eq!(out.frames(), 50);
        // Reversed: frame 0 is source frame 149, the loudest, normalised to 1.
        assert!((out.data[0] - 1.0).abs() < 1e-6);
        assert!((out.data[98] - 100.0 / 149.0).abs() < 1e-5);
        let faded = render_slice(
            &buf,
            &SliceEdit {
                start: 100,
                end: 150,
                fade_in: 10.0 / SR as f32,
                ..SliceEdit::default()
            },
        );
        assert_eq!(faded.data[0], 0.0);
        assert!(faded.data[20] > 0.0);
    }

    #[test]
    fn the_lo_fi_grade_is_11_khz() {
        let buf = SampleBuf::new(SR, crate::sample_buf::tests::sine(440.0, 44_100, SR));
        let out = render_slice(
            &buf,
            &SliceEdit {
                grade: Grade::LoFi,
                ..SliceEdit::default()
            },
        );
        assert_eq!(out.rate, 11_025);
        assert!((out.frames() as i64 - 11_025).abs() <= 1);
        let mono: Vec<f32> = out.data.iter().step_by(2).copied().collect();
        let hz = crate::sample_buf::tests::peak_hz(&mono, 11_025);
        assert!((hz - 440.0).abs() < 2.0, "{hz}");
    }

    #[test]
    fn stretch_keeps_the_pitch_and_changes_the_length_within_one_percent() {
        let buf = SampleBuf::new(SR, crate::sample_buf::tests::sine(440.0, 88_200, SR));
        for time in [0.75, 1.5] {
            let out = stretch(&buf, time, 0.0);
            let want = 88_200.0 * time;
            let err = (out.frames() as f64 - want).abs() / want;
            assert!(err < 0.01, "length off by {}", err * 100.0);
            let mono: Vec<f32> = out.data.iter().step_by(2).copied().collect();
            let n = mono.len();
            let hz = crate::sample_buf::tests::peak_hz(&mono[n / 8..n * 7 / 8], SR);
            assert!((hz - 440.0).abs() / 440.0 < 0.01, "{hz} Hz");
        }
        // And the pitch alone, up a fifth, length kept.
        let out = stretch(&buf, 1.0, 7.0);
        assert_eq!(out.frames(), 88_200);
        let mono: Vec<f32> = out.data.iter().step_by(2).copied().collect();
        let hz = crate::sample_buf::tests::peak_hz(&mono[10_000..80_000], SR);
        assert!((hz - 659.25).abs() < 7.0, "{hz}");
    }

    #[test]
    fn loop_tempo_from_length() {
        // A 2-second loop: 4 beats is 120 BPM.
        assert!((loop_bpm(2.0).unwrap() - 120.0).abs() < 1e-9);
        // 4 bars at 90: 10.667 s, 16 beats.
        assert!((loop_bpm(16.0 * 60.0 / 90.0).unwrap() - 90.0).abs() < 1e-9);
    }

    #[test]
    fn the_key_of_an_a_minor_chord_progression() {
        let mut data = Vec::new();
        // A minor triad then E (A C E, then E G# B, then A C E), 2 s each.
        for chord in [
            [220.0, 261.63, 329.63],
            [164.81, 207.65, 246.94],
            [220.0, 261.63, 329.63],
        ] {
            for i in 0..2 * SR as usize {
                let t = i as f32 / SR as f32;
                let s: f32 = chord
                    .iter()
                    .map(|f| (std::f32::consts::TAU * f * t).sin() * 0.2)
                    .sum();
                data.push(s);
                data.push(s);
            }
        }
        let key = estimate_key(&SampleBuf::new(SR, data)).unwrap();
        assert_eq!(key, "Am");
    }
}
