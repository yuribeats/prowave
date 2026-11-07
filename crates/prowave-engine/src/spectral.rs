//! Spectral pictures, worked out offline (never on the audio thread).
//!
//! Two directions of one idea. [`spectrogram`] measures how loud every frequency is over time
//! (the short-time Fourier transform, STFT), which the Spectrogram window draws: time runs
//! left to right, frequency bottom to top, loudness is brightness. [`paint`] runs it
//! backwards (MetaSynth's Image Synth, in reverse): it makes audio whose spectrogram *is* a
//! given picture, so a face or a bird shows up when the sound is looked at.
//!
//! How a picture becomes sound: every column of the picture is a moment, every row a
//! frequency (bottom = low), and a pixel's brightness is how loud that frequency is then. That
//! gives the loudness of every frequency in every STFT frame, but not the phases a real sound
//! would have, and random phases leave the frames disagreeing where they overlap (the picture
//! smears into noise). Griffin–Lim fixes that: turn the frames into audio, measure that audio
//! again, keep the phases it has and put the picture's loudness back, and repeat; each round
//! the frames agree better and the picture gets sharper. The fast variant (Perraudin, Balazs
//! and Søndergaard, 2013) adds momentum and needs far fewer rounds.
//!
//! Everything here allocates freely and may take seconds: run it on a worker thread.
use std::sync::atomic::{AtomicU32, Ordering};

/// Samples in one analysis frame (about 43 ms at 48 kHz: 23 Hz between frequency bins).
pub const PAINT_FFT: usize = 2048;
/// Samples between frames: a quarter of a frame, so every sample is in four frames.
pub const PAINT_HOP: usize = PAINT_FFT / 4;
/// The longest sound [`paint`] makes, in seconds (its working memory grows with the length:
/// about 2 MB per second at 48 kHz).
pub const MAX_PAINT_SECONDS: f32 = 60.0;
/// Momentum of the fast Griffin–Lim rounds (0 is plain Griffin–Lim).
const MOMENTUM: f32 = 0.99;
/// The quietest level a spectrogram reports, in dB.
const FLOOR_DB: f32 = -160.0;

/// A radix-2 fast Fourier transform of one size, with its tables worked out once.
pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
}

impl Fft {
    /// A transform of `n` points; `n` is rounded up to a power of two (at least 2).
    pub fn new(n: usize) -> Self {
        let n = n.max(2).next_power_of_two();
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let angle = |k: usize| std::f64::consts::TAU * k as f64 / n as f64;
        Self {
            n,
            cos: (0..n / 2).map(|k| angle(k).cos() as f32).collect(),
            sin: (0..n / 2).map(|k| angle(k).sin() as f32).collect(),
            rev,
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Transforms `re` + i·`im` in place (both exactly [`Fft::len`] long). The inverse
    /// (`inverse = true`) is not divided by the length.
    pub fn process(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        let n = self.n;
        if re.len() != n || im.len() != n {
            return;
        }
        for i in 0..n {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sign = if inverse { 1.0 } else { -1.0 };
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], sign * self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let xr = re[b] * wr - im[b] * wi;
                    let xi = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - xr;
                    im[b] = im[a] - xi;
                    re[a] += xr;
                    im[a] += xi;
                }
            }
            len *= 2;
        }
    }
}

/// Short-time Fourier transform with a Hann window: frame `m` is centred on sample
/// `m · hop`, so the first frame is centred on the first sample (zeros before it).
struct Stft {
    fft: Fft,
    hop: usize,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Stft {
    fn new(size: usize, hop: usize) -> Self {
        let fft = Fft::new(size);
        let n = fft.len();
        // Periodic Hann: overlapping copies a quarter apart add up evenly.
        let window = (0..n)
            .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()) as f32)
            .collect();
        Self {
            fft,
            hop: hop.max(1),
            window,
            re: vec![0.0; n],
            im: vec![0.0; n],
        }
    }

    fn size(&self) -> usize {
        self.fft.len()
    }

    fn bins(&self) -> usize {
        self.size() / 2 + 1
    }

    fn frames(&self, len: usize) -> usize {
        len / self.hop + 1
    }

    /// Where frame `m` starts (it may start before the first sample).
    fn start(&self, m: usize) -> isize {
        (m * self.hop) as isize - (self.size() / 2) as isize
    }

    /// The spectrum of frame `m` of `x` into `out` (`bins` values: real, imaginary).
    fn analyze_frame(&mut self, x: &[f32], m: usize, out: &mut [(f32, f32)]) {
        let start = self.start(m);
        for i in 0..self.size() {
            let at = start + i as isize;
            self.re[i] = if at >= 0 && (at as usize) < x.len() {
                x[at as usize] * self.window[i]
            } else {
                0.0
            };
            self.im[i] = 0.0;
        }
        self.fft.process(&mut self.re, &mut self.im, false);
        for (k, o) in out.iter_mut().enumerate() {
            *o = (self.re[k], self.im[k]);
        }
    }

    /// Every frame of `x` into `out` (frame after frame, `bins` values each).
    fn analyze(&mut self, x: &[f32], out: &mut [(f32, f32)]) {
        let bins = self.bins();
        for (m, frame) in out.chunks_exact_mut(bins).enumerate() {
            self.analyze_frame(x, m, frame);
        }
    }

    /// Audio of `len` samples whose frames are as close as can be to `spec` (windowed
    /// overlap-add, divided by the sum of the squared windows: the least-squares inverse).
    fn synthesize(&mut self, spec: &[(f32, f32)], len: usize) -> Vec<f32> {
        let (n, bins) = (self.size(), self.bins());
        let mut out = vec![0f32; len];
        let mut weight = vec![0f32; len];
        for (m, frame) in spec.chunks_exact(bins).enumerate() {
            for (k, &(r, i)) in frame.iter().enumerate() {
                self.re[k] = r;
                self.im[k] = i;
                if k > 0 && k < n - k {
                    self.re[n - k] = r;
                    self.im[n - k] = -i;
                }
            }
            // The DC and the highest bin of a real signal have no imaginary part.
            self.im[0] = 0.0;
            self.im[n / 2] = 0.0;
            self.fft.process(&mut self.re, &mut self.im, true);
            let start = self.start(m);
            for i in 0..n {
                let at = start + i as isize;
                if at < 0 || at as usize >= len {
                    continue;
                }
                let w = self.window[i];
                out[at as usize] += self.re[i] / n as f32 * w;
                weight[at as usize] += w * w;
            }
        }
        for (o, w) in out.iter_mut().zip(&weight) {
            *o = if *w > 1e-6 { *o / w } else { 0.0 };
        }
        out
    }
}

/// How loud every frequency is over time: `frames` columns of `bins` values in dB, where a
/// full-scale sine reads 0 dB (Logic's spectrum views use the same reference).
#[derive(Clone, Debug)]
pub struct Spectrogram {
    pub sample_rate: u32,
    /// Samples per frame (the FFT size) and between frames.
    pub size: usize,
    pub hop: usize,
    pub frames: usize,
    /// Frequency bins per frame: `size / 2 + 1`, from 0 Hz to half the sample rate.
    pub bins: usize,
    /// Frame after frame, `bins` values each.
    pub db: Vec<f32>,
}

impl Spectrogram {
    /// The level of bin `bin` in frame `frame`, in dB.
    pub fn at(&self, frame: usize, bin: usize) -> f32 {
        self.db
            .get(frame * self.bins + bin)
            .copied()
            .unwrap_or(FLOOR_DB)
    }

    /// The frequency of bin `bin`, in Hz.
    pub fn bin_hz(&self, bin: f32) -> f32 {
        bin * self.sample_rate as f32 / self.size as f32
    }

    /// The bin at frequency `hz` (fractional).
    pub fn hz_bin(&self, hz: f32) -> f32 {
        hz * self.size as f32 / self.sample_rate.max(1) as f32
    }

    /// The time frame `frame` is centred on, in seconds.
    pub fn frame_secs(&self, frame: usize) -> f64 {
        (frame * self.hop) as f64 / f64::from(self.sample_rate.max(1))
    }

    /// The loudest level anywhere, in dB.
    pub fn max_db(&self) -> f32 {
        self.db.iter().copied().fold(FLOOR_DB, f32::max)
    }
}

/// The spectrogram of `mono` with `size`-sample Hann frames `hop` samples apart. A `hop`
/// longer than `size` skips audio between frames (an overview of a long recording).
pub fn spectrogram(mono: &[f32], sample_rate: u32, size: usize, hop: usize) -> Spectrogram {
    let mut stft = Stft::new(size, hop);
    let (size, bins, frames) = (stft.size(), stft.bins(), stft.frames(mono.len()));
    let mut frame = vec![(0f32, 0f32); bins];
    let mut db = Vec::with_capacity(frames * bins);
    // A sine of amplitude 1 peaks at size / 4 through a Hann window.
    let full = size as f32 / 4.0;
    for m in 0..frames {
        stft.analyze_frame(mono, m, &mut frame);
        db.extend(frame.iter().map(|&(r, i)| {
            let mag = (r * r + i * i).sqrt() / full;
            if mag > 0.0 {
                (20.0 * mag.log10()).max(FLOOR_DB)
            } else {
                FLOOR_DB
            }
        }));
    }
    Spectrogram {
        sample_rate,
        size,
        hop: stft.hop,
        frames,
        bins,
        db,
    }
}

/// A picture to paint, as brightness from 0 (black) to 1 (white), row after row from the top.
#[derive(Clone, Debug)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub luma: Vec<f32>,
}

/// How the rows of the picture are spread over the frequencies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreqScale {
    /// Equal steps in Hz: looks right on a linear-frequency spectrogram.
    Linear,
    /// Equal steps in octaves: looks right on a logarithmic one (Logic's analyzers, and how
    /// pitch is heard), so this is the default.
    Log,
}

/// The choices of Paint Image to Audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintSettings {
    pub sample_rate: u32,
    /// Length of the sound: the picture's width.
    pub seconds: f32,
    /// The frequencies of the bottom and the top of the picture, in Hz.
    pub low_hz: f32,
    pub high_hz: f32,
    pub scale: FreqScale,
    /// Brightness curve: brightness is raised to this power first (above 1 darkens the
    /// middle greys, below 1 lightens them).
    pub gamma: f32,
    /// How many dB below white black-but-not-quite is: brightness maps to level in dB, so the
    /// picture's greys show as greys on a dB spectrogram. Pure black is silent.
    pub range_db: f32,
    /// Paint the negative (dark lines on a white picture become loud).
    pub invert: bool,
    /// The loudest sample, in dBFS.
    pub level_db: f32,
    /// Griffin–Lim rounds (0: random phases only).
    pub iterations: u32,
}

impl Default for PaintSettings {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            seconds: 8.0,
            low_hz: 100.0,
            high_hz: 16_000.0,
            scale: FreqScale::Log,
            gamma: 1.0,
            range_db: 60.0,
            invert: false,
            level_db: -1.0,
            iterations: 32,
        }
    }
}

impl PaintSettings {
    /// The settings made safe: lengths, frequencies and curves inside what works.
    pub fn clamped(mut self) -> Self {
        self.sample_rate = self.sample_rate.clamp(8_000, 192_000);
        let nyquist = self.sample_rate as f32 / 2.0;
        self.seconds = self.seconds.clamp(0.25, MAX_PAINT_SECONDS);
        self.high_hz = self.high_hz.clamp(20.0, nyquist * 0.995);
        self.low_hz = self.low_hz.clamp(1.0, self.high_hz * 0.9);
        self.gamma = self.gamma.clamp(0.1, 5.0);
        self.range_db = self.range_db.clamp(6.0, 120.0);
        self.level_db = self.level_db.clamp(-60.0, 0.0);
        self.iterations = self.iterations.min(500);
        self
    }

    /// Where frequency `hz` is up the picture: 0 at the bottom (`low_hz`), 1 at the top.
    fn height_of(&self, hz: f32) -> f32 {
        match self.scale {
            FreqScale::Linear => (hz - self.low_hz) / (self.high_hz - self.low_hz),
            FreqScale::Log => (hz.max(1e-3) / self.low_hz).ln() / (self.high_hz / self.low_hz).ln(),
        }
    }

    /// The frequency at height `v` (0 bottom, 1 top) of the picture.
    pub fn hz_at(&self, v: f32) -> f32 {
        match self.scale {
            FreqScale::Linear => self.low_hz + v * (self.high_hz - self.low_hz),
            FreqScale::Log => self.low_hz * (self.high_hz / self.low_hz).powf(v),
        }
    }
}

/// Sums of the picture's brightness over rectangles, any size, in constant time: the
/// integral of the brightness from the top-left corner (a summed-area table), bilinear inside
/// each pixel, so fractions of pixels count by their area.
struct AreaSums {
    w: usize,
    h: usize,
    sum: Vec<f64>,
}

impl AreaSums {
    fn new(width: usize, height: usize, value: impl Fn(usize, usize) -> f32) -> Self {
        let (w, h) = (width, height);
        let mut sum = vec![0f64; (w + 1) * (h + 1)];
        for y in 0..h {
            let mut row = 0f64;
            for x in 0..w {
                row += f64::from(value(x, y));
                sum[(y + 1) * (w + 1) + x + 1] = sum[y * (w + 1) + x + 1] + row;
            }
        }
        Self { w, h, sum }
    }

    /// The integral up to (`x`, `y`), in pixels.
    fn integral(&self, x: f64, y: f64) -> f64 {
        let x = x.clamp(0.0, self.w as f64);
        let y = y.clamp(0.0, self.h as f64);
        let (x0, y0) = (
            (x.floor() as usize).min(self.w),
            (y.floor() as usize).min(self.h),
        );
        let (x1, y1) = ((x0 + 1).min(self.w), (y0 + 1).min(self.h));
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let s = |x: usize, y: usize| self.sum[y * (self.w + 1) + x];
        let top = s(x0, y0) + (s(x1, y0) - s(x0, y0)) * fx;
        let bottom = s(x0, y1) + (s(x1, y1) - s(x0, y1)) * fx;
        top + (bottom - top) * fy
    }

    /// The mean brightness over [`x0`, `x1`] × [`y0`, `y1`] (pixels; a span thinner than a
    /// hundredth of a pixel is widened to that).
    fn mean(&self, x0: f64, x1: f64, y0: f64, y1: f64) -> f32 {
        let (x0, x1) = (x0.min(x1), x0.max(x1).max(x0.min(x1) + 0.01));
        let (y0, y1) = (y0.min(y1), y0.max(y1).max(y0.min(y1) + 0.01));
        let total = self.integral(x1, y1) - self.integral(x0, y1) - self.integral(x1, y0)
            + self.integral(x0, y0);
        (total / ((x1 - x0) * (y1 - y0))) as f32
    }
}

/// Small, fast, repeatable random numbers (the phases a painting starts from).
struct Rng(u64);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The loudness every STFT bin should have: the picture's mean brightness over the patch of
/// it that the bin covers (its frequency span by the frame's time span).
fn target_magnitudes(pic: &Picture, s: &PaintSettings, frames: usize, bins: usize) -> Vec<f32> {
    let (w, h) = (pic.width, pic.height);
    let areas = AreaSums::new(w, h, |x, y| {
        let l = pic
            .luma
            .get(y * w + x)
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let l = if s.invert { 1.0 - l } else { l };
        l.powf(s.gamma)
    });
    let size = (bins - 1) * 2;
    let bin_hz = s.sample_rate as f32 / size as f32;
    // Each bin's span of picture rows (top, bottom), or none outside the frequency range.
    let rows: Vec<Option<(f64, f64)>> = (0..bins)
        .map(|k| {
            let (lo, hi) = ((k as f32 - 0.5) * bin_hz, (k as f32 + 0.5) * bin_hz);
            if hi <= s.low_hz || lo >= s.high_hz {
                return None;
            }
            let (lo, hi) = (lo.max(s.low_hz), hi.min(s.high_hz));
            let y = |hz: f32| f64::from(1.0 - s.height_of(hz)) * h as f64;
            Some((y(hi), y(lo)))
        })
        .collect();
    let hop_secs = PAINT_HOP as f64 / f64::from(s.sample_rate);
    let px_per_sec = w as f64 / f64::from(s.seconds);
    let mut out = vec![0f32; frames * bins];
    for m in 0..frames {
        let t = m as f64 * hop_secs;
        let (x0, x1) = (
            (t - hop_secs / 2.0) * px_per_sec,
            (t + hop_secs / 2.0) * px_per_sec,
        );
        if x1 <= 0.0 || x0 >= w as f64 {
            continue;
        }
        for (k, span) in rows.iter().enumerate() {
            let Some((y0, y1)) = *span else { continue };
            let b = areas.mean(x0, x1, y0, y1);
            if b > 1e-4 {
                out[m * bins + k] = 10f32.powf((b - 1.0) * s.range_db / 20.0);
            }
        }
    }
    out
}

/// Audio whose spectrogram is `pic` (mono, at `settings.sample_rate`), peaking at
/// `settings.level_db`. `progress` goes from 0 to 1000 as the work gets done.
pub fn paint(pic: &Picture, settings: &PaintSettings, progress: &AtomicU32) -> Vec<f32> {
    let s = settings.clamped();
    progress.store(0, Ordering::Relaxed);
    let len = (f64::from(s.seconds) * f64::from(s.sample_rate)).round() as usize;
    if pic.width == 0 || pic.height == 0 || pic.luma.len() < pic.width * pic.height {
        progress.store(1000, Ordering::Relaxed);
        return vec![0.0; len];
    }
    let mut stft = Stft::new(PAINT_FFT, PAINT_HOP);
    let (bins, frames) = (stft.bins(), stft.frames(len));
    let target = target_magnitudes(pic, &s, frames, bins);
    let steps = s.iterations + 1;
    let tick = |done: u32| progress.store(done * 1000 / steps, Ordering::Relaxed);

    // Start from random phases.
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut spec: Vec<(f32, f32)> = target
        .iter()
        .map(|&a| {
            let phase = rng.next_f32() * std::f32::consts::TAU;
            (a * phase.cos(), a * phase.sin())
        })
        .collect();
    let mut last = spec.clone();
    tick(1);
    for round in 0..s.iterations {
        let audio = stft.synthesize(&spec, len);
        stft.analyze(&audio, &mut spec);
        // Keep the phases the audio has, put the picture's loudness back, and run on a
        // little past that (the momentum of the fast variant).
        for ((c, prev), &a) in spec.iter_mut().zip(last.iter_mut()).zip(&target) {
            let mag = (c.0 * c.0 + c.1 * c.1).sqrt();
            let t = if mag > 1e-12 {
                (a * c.0 / mag, a * c.1 / mag)
            } else {
                (a, 0.0)
            };
            *c = (
                t.0 + MOMENTUM * (t.0 - prev.0),
                t.1 + MOMENTUM * (t.1 - prev.1),
            );
            *prev = t;
        }
        tick(round + 2);
    }
    let mut audio = stft.synthesize(&last, len);

    // A 2 ms fade at both ends, so a bright edge of the picture does not click.
    let fade = (s.sample_rate as usize / 500).min(len / 2);
    for i in 0..fade {
        let g = i as f32 / fade as f32;
        audio[i] *= g;
        audio[len - 1 - i] *= g;
    }
    let peak = audio.iter().fold(0f32, |m, v| m.max(v.abs()));
    if peak > 1e-9 {
        let gain = 10f32.powf(s.level_db / 20.0) / peak;
        for v in &mut audio {
            *v *= gain;
        }
    }
    progress.store(1000, Ordering::Relaxed);
    audio
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn picture(w: usize, h: usize, lit: impl Fn(usize, usize) -> bool) -> Picture {
        let mut luma = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                if lit(x, y) {
                    luma[y * w + x] = 1.0;
                }
            }
        }
        Picture {
            width: w,
            height: h,
            luma,
        }
    }

    fn quick(seconds: f32) -> PaintSettings {
        PaintSettings {
            seconds,
            iterations: 8,
            ..PaintSettings::default()
        }
    }

    /// The loudest bin of frame `m`, as a frequency.
    fn loudest_hz(sp: &Spectrogram, m: usize) -> f32 {
        let k = (0..sp.bins)
            .max_by(|a, b| sp.at(m, *a).total_cmp(&sp.at(m, *b)))
            .unwrap();
        sp.bin_hz(k as f32)
    }

    #[test]
    fn fft_matches_the_plain_fourier_sum_and_inverts() {
        let n = 16;
        let fft = Fft::new(n);
        let x: Vec<f32> = (0..n).map(|i| ((i * 7 % 5) as f32) - 2.0).collect();
        let (mut re, mut im) = (x.clone(), vec![0f32; n]);
        fft.process(&mut re, &mut im, false);
        for k in 0..n {
            let (mut sr, mut si) = (0f32, 0f32);
            for (i, v) in x.iter().enumerate() {
                let a = -std::f32::consts::TAU * (k * i) as f32 / n as f32;
                sr += v * a.cos();
                si += v * a.sin();
            }
            assert!(
                (re[k] - sr).abs() < 1e-3 && (im[k] - si).abs() < 1e-3,
                "bin {k}"
            );
        }
        fft.process(&mut re, &mut im, true);
        for i in 0..n {
            assert!((re[i] / n as f32 - x[i]).abs() < 1e-4);
        }
    }

    #[test]
    fn a_pure_sine_peaks_at_its_bin_at_full_scale() {
        let rate = 48_000;
        // 1500 Hz is exactly bin 64 of a 2048-point frame at 48 kHz.
        let x: Vec<f32> = (0..rate)
            .map(|i| (std::f32::consts::TAU * 1500.0 * i as f32 / rate as f32).sin())
            .collect();
        let sp = spectrogram(&x, rate as u32, 2048, 512);
        assert_eq!(sp.bins, 1025);
        let m = sp.frames / 2;
        let k = (0..sp.bins)
            .max_by(|a, b| sp.at(m, *a).total_cmp(&sp.at(m, *b)))
            .unwrap();
        assert_eq!(k, 64);
        assert!(
            sp.at(m, 64).abs() < 0.5,
            "full scale reads 0 dB: {}",
            sp.at(m, 64)
        );
        assert!(sp.at(m, 200) < -60.0);
        assert!((sp.bin_hz(64.0) - 1500.0).abs() < 1e-3);
    }

    #[test]
    fn a_bright_horizontal_line_becomes_a_tone_at_its_height() {
        let (w, h) = (32, 128);
        let row = 64;
        let pic = picture(w, h, |_, y| y == row);
        let s = quick(1.0);
        let done = AtomicU32::new(0);
        let audio = paint(&pic, &s, &done);
        assert_eq!(done.load(Ordering::Relaxed), 1000);
        assert_eq!(audio.len(), 48_000);
        // The row covers heights (h - row - 1)/h to (h - row)/h; its middle is the tone.
        let v = (h - row) as f32 / h as f32 - 0.5 / h as f32;
        let want = s.hz_at(v);
        let sp = spectrogram(&audio, s.sample_rate, 4096, 1024);
        for m in [sp.frames / 4, sp.frames / 2, 3 * sp.frames / 4] {
            let got = loudest_hz(&sp, m);
            assert!(
                (got / want - 1.0).abs() < 0.04,
                "frame {m}: loudest {got} Hz, painted {want} Hz"
            );
        }
        // Next to nothing an octave away.
        let m = sp.frames / 2;
        let at = |hz: f32| sp.at(m, sp.hz_bin(hz).round() as usize);
        assert!(at(want) - at(want * 2.0) > 40.0 && at(want) - at(want / 2.0) > 40.0);
        let peak = audio.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((20.0 * peak.log10() + 1.0).abs() < 0.01, "peaks at -1 dBFS");
    }

    #[test]
    fn a_bright_vertical_line_becomes_a_click_at_its_time_across_the_band() {
        let (w, h) = (64, 64);
        let col = 40;
        let pic = picture(w, h, |x, _| x == col);
        let s = quick(2.0);
        let audio = paint(&pic, &s, &AtomicU32::new(0));
        // The column covers 40/64 to 41/64 of the length.
        let want = (col as f32 + 0.5) / w as f32 * s.seconds;
        let block = 480; // 10 ms
        let energy: Vec<f32> = audio
            .chunks(block)
            .map(|c| c.iter().map(|v| v * v).sum())
            .collect();
        let loudest = (0..energy.len())
            .max_by(|a, b| energy[*a].total_cmp(&energy[*b]))
            .unwrap();
        let got = (loudest as f32 + 0.5) * block as f32 / s.sample_rate as f32;
        assert!(
            (got - want).abs() < 0.04,
            "click at {got} s, painted at {want} s"
        );
        // Silent away from it.
        let far = energy[energy.len() / 10];
        assert!(far < energy[loudest] * 1e-4, "quiet elsewhere");
        // Broadband: low, middle and high frequencies all loud at the click.
        let sp = spectrogram(&audio, s.sample_rate, 1024, 256);
        let m = (want * s.sample_rate as f32 / 256.0).round() as usize;
        let top = sp.max_db();
        for hz in [300.0, 2_000.0, 10_000.0] {
            let k = sp.hz_bin(hz).round() as usize;
            let level = (m.saturating_sub(2)..m + 3)
                .map(|f| sp.at(f, k))
                .fold(FLOOR_DB, f32::max);
            assert!(level > top - 20.0, "{hz} Hz is {level} dB, top {top} dB");
        }
        // Nothing outside the painted range.
        let k = sp.hz_bin(19_000.0).round() as usize;
        assert!(sp.at(m, k) < top - 40.0);
    }

    #[test]
    fn invert_and_linear_scale_move_the_tone() {
        // A white picture with one dark row: inverted, only that row sounds.
        let (w, h) = (16, 100);
        let pic = picture(w, h, |_, y| y != 25);
        let s = PaintSettings {
            scale: FreqScale::Linear,
            invert: true,
            low_hz: 0.0,
            high_hz: 20_000.0,
            ..quick(0.5)
        }
        .clamped();
        let audio = paint(&pic, &s, &AtomicU32::new(0));
        let sp = spectrogram(&audio, s.sample_rate, 4096, 1024);
        let want = s.hz_at(1.0 - 25.5 / h as f32);
        let got = loudest_hz(&sp, sp.frames / 2);
        assert!(
            (got - want).abs() < 2.0 * s.high_hz / h as f32,
            "{got} vs {want}"
        );
    }

    #[test]
    fn a_black_or_empty_picture_is_silent_and_never_nan() {
        let s = quick(0.5);
        let black = paint(&picture(8, 8, |_, _| false), &s, &AtomicU32::new(0));
        assert!(black.iter().all(|v| *v == 0.0));
        let empty = Picture {
            width: 0,
            height: 0,
            luma: vec![],
        };
        let out = paint(&empty, &s, &AtomicU32::new(0));
        assert_eq!(out.len(), 24_000);
        let white = paint(&picture(8, 8, |_, _| true), &s, &AtomicU32::new(0));
        assert!(white.iter().all(|v| v.is_finite()));
    }
}
