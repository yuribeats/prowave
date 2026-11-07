//! Pro Wave Tune's pitch tracker: YIN (de Cheveigné and Kawahara, 2002) on a low-passed copy
//! of the input taken down to about 12 kHz, then refined at the full rate around the lag it
//! found. Monophonic; real-time safe once made (all memory is allocated in [`Tracker::new`]).
//!
//! Each estimate looks back over one integration window plus one lag: the slowest period of
//! the voice range twice (about 36 ms for Low Male, 13 ms for Soprano). It does not delay the
//! audio: the shifter works on the delayed signal with the newest estimate.
//!
//! Cost per estimate, decimated YIN plus the refinement: about `W * lags + 9 * W * D`
//! multiply-adds (W the window, D the decimation); at 48 kHz in the widest range, ~56 000.

/// The voice ranges (Voice parameter), lowest and highest pitch looked for, in Hz.
pub const VOICES: [(&str, f32, f32); 4] = [
    ("Soprano", 150.0, 1100.0),
    ("Alto/Tenor", 80.0, 800.0),
    ("Low Male", 55.0, 450.0),
    ("Instrument", 50.0, 1200.0),
];

/// The lowest pitch any range looks for (it sizes the buffers).
const LOWEST_HZ: f32 = 50.0;
/// The highest.
const HIGHEST_HZ: f32 = 1200.0;
/// YIN's absolute threshold on the cumulative mean normalised difference.
const YIN_THRESHOLD: f32 = 0.15;

/// A two-pole low-pass section (direct form II transposed).
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn lowpass(rate: f32, hz: f32, q: f32) -> Self {
        let w = std::f32::consts::TAU * (hz / rate).min(0.49);
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - c) / a0;
        Self {
            b0: b1 * 0.5,
            b1,
            b2: b1 * 0.5,
            a1: -2.0 * c / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// One estimate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    /// The period in samples at the full rate (0: nothing found).
    pub period: f32,
    /// YIN's aperiodicity at that period: 0 perfectly periodic, 1 noise.
    pub aperiodicity: f32,
    /// The level of the window looked at (RMS).
    pub rms: f32,
}

impl Estimate {
    /// The pitch in Hz at `rate` (0 when nothing was found).
    pub fn hz(&self, rate: f32) -> f32 {
        if self.period > 0.0 {
            rate / self.period
        } else {
            0.0
        }
    }
}

pub struct Tracker {
    rate: f32,
    /// Decimation factor, and the rate YIN runs at.
    dec: usize,
    dec_rate: f32,
    phase: usize,
    aa: [Biquad; 2],
    /// The decimated signal (ring), and how many samples went in.
    low: Vec<f32>,
    low_n: usize,
    /// The full-rate signal (ring), and how many samples went in.
    full: Vec<f32>,
    full_n: usize,
    /// YIN's difference function, scratch.
    diff: Vec<f32>,
    /// The longest lag and window the buffers allow (decimated samples).
    max_lag: usize,
    /// The range looked in now.
    min_hz: f32,
    max_hz: f32,
}

fn ring_len(n: usize) -> usize {
    n.next_power_of_two().max(64)
}

impl Tracker {
    /// A tracker for audio at `rate` Hz. Allocates.
    pub fn new(rate: f32) -> Self {
        let rate = if rate.is_finite() && rate >= 8000.0 {
            rate
        } else {
            48000.0
        };
        let dec = ((rate / 11000.0).floor() as usize).max(1);
        let dec_rate = rate / dec as f32;
        let max_lag = (dec_rate / LOWEST_HZ).ceil() as usize + 2;
        let mut aa = [
            Biquad::lowpass(rate, dec_rate * 0.42, 0.541_196_1),
            Biquad::lowpass(rate, dec_rate * 0.42, 1.306_563),
        ];
        if dec == 1 {
            // Nothing is thrown away: leave the sound as it is (a pass-through filter).
            aa = [Biquad {
                b0: 1.0,
                ..Biquad::default()
            }; 2];
        }
        Self {
            rate,
            dec,
            dec_rate,
            phase: 0,
            aa,
            low: vec![0.0; ring_len(2 * max_lag + 4)],
            low_n: 0,
            full: vec![0.0; ring_len((2 * max_lag + 4) * dec + 16)],
            full_n: 0,
            diff: vec![0.0; max_lag + 2],
            max_lag,
            min_hz: VOICES[1].1,
            max_hz: VOICES[1].2,
        }
    }

    /// Looks for pitches between `min_hz` and `max_hz` from now on (kept within 50-1200 Hz).
    pub fn set_range(&mut self, min_hz: f32, max_hz: f32) {
        self.min_hz = min_hz.clamp(LOWEST_HZ, HIGHEST_HZ * 0.5);
        self.max_hz = max_hz.clamp(self.min_hz * 2.0, HIGHEST_HZ);
    }

    /// The Voice parameter's range.
    pub fn set_voice(&mut self, voice: usize) {
        let (_, lo, hi) = VOICES[voice.min(VOICES.len() - 1)];
        self.set_range(lo, hi);
    }

    /// One input sample.
    #[inline]
    pub fn push(&mut self, x: f32) {
        let x = if x.is_finite() { x } else { 0.0 };
        let fm = self.full.len() - 1;
        self.full[self.full_n & fm] = x;
        self.full_n = self.full_n.wrapping_add(1);
        let a = self.aa[0].run(x);
        let y = self.aa[1].run(a);
        self.phase += 1;
        if self.phase >= self.dec {
            self.phase = 0;
            let lm = self.low.len() - 1;
            self.low[self.low_n & lm] = y;
            self.low_n = self.low_n.wrapping_add(1);
        }
    }

    /// Lags (decimated) and window for the range in use.
    fn lags(&self) -> (usize, usize) {
        let lo = ((self.dec_rate / self.max_hz).floor() as usize).max(2);
        let hi = ((self.dec_rate / self.min_hz).ceil() as usize + 1).min(self.max_lag);
        (lo, hi)
    }

    /// The delay of an estimate: how far back the middle of the sound it looks at is, in
    /// full-rate samples.
    pub fn look_back(&self) -> usize {
        let (_, hi) = self.lags();
        hi * self.dec
    }

    /// The pitch of the newest sound. No allocation.
    pub fn estimate(&mut self) -> Estimate {
        self.estimate_near(None)
    }

    /// [`Tracker::estimate`], keeping to `hint` (the period found last, full-rate samples)
    /// when a dip there is nearly as good as the one picked an octave or a harmonic away:
    /// a sung line does not jump by octaves from one moment to the next, a tracker does.
    pub fn estimate_near(&mut self, hint: Option<f32>) -> Estimate {
        let (lo, hi) = self.lags();
        let w = hi;
        if self.low_n < w + hi + 1 {
            return Estimate::default();
        }
        let lm = self.low.len() - 1;
        let start = self.low_n - (w + hi);
        let x = |j: usize| self.low[(start + j) & lm];
        // d(tau) for tau in 1..=hi.
        let mut energy = 0.0f32;
        for j in 0..w {
            energy += x(j) * x(j);
        }
        self.diff[0] = 0.0;
        for tau in 1..=hi {
            let mut d = 0.0f32;
            for j in 0..w {
                let e = x(j) - x(j + tau);
                d += e * e;
            }
            self.diff[tau] = d;
        }
        // Cumulative mean normalised difference, in place.
        let mut sum = 0.0f32;
        for tau in 1..=hi {
            sum += self.diff[tau];
            self.diff[tau] = if sum > 0.0 {
                self.diff[tau] * tau as f32 / sum
            } else {
                1.0
            };
        }
        let rms = (energy / w as f32).sqrt();
        // The first dip under the threshold (to its bottom), else the deepest.
        let mut pick = 0usize;
        let mut tau = lo;
        while tau <= hi {
            if self.diff[tau] < YIN_THRESHOLD {
                while tau < hi && self.diff[tau + 1] < self.diff[tau] {
                    tau += 1;
                }
                pick = tau;
                break;
            }
            tau += 1;
        }
        if pick == 0 {
            let mut best = f32::INFINITY;
            for t in lo..=hi {
                if self.diff[t] < best {
                    best = self.diff[t];
                    pick = t;
                }
            }
        }
        if let Some(h) = hint.filter(|h| h.is_finite() && *h > 0.0) {
            let h = h / self.dec as f32;
            let ratio = if pick > 0 { pick as f32 / h } else { 0.0 };
            if pick == 0 || !(0.7..=1.43).contains(&ratio) {
                let a = ((h * 0.85).floor() as usize).max(lo);
                let b = ((h * 1.18).ceil() as usize).min(hi - 1);
                let mut near = 0usize;
                for t in a..=b {
                    if t > 0
                        && self.diff[t] <= self.diff[t - 1]
                        && self.diff[t] <= self.diff[t + 1]
                        && (near == 0 || self.diff[t] < self.diff[near])
                    {
                        near = t;
                    }
                }
                let good = if pick > 0 { self.diff[pick] } else { 1.0 };
                if near > 0 && self.diff[near] <= (good * 1.5 + 0.06).max(YIN_THRESHOLD * 1.5) {
                    pick = near;
                }
            }
        }
        if pick == 0 || pick >= hi {
            return Estimate {
                period: 0.0,
                aperiodicity: 1.0,
                rms,
            };
        }
        let aperiodicity = self.diff[pick].clamp(0.0, 1.0);
        let coarse =
            pick as f32 + parabola(self.diff[pick - 1], self.diff[pick], self.diff[pick + 1]);
        let period = self.refine(coarse * self.dec as f32, w * self.dec);
        Estimate {
            period,
            aperiodicity,
            rms,
        }
    }

    /// The period near `guess` (full-rate samples) from the difference function of the newest
    /// `w` full-rate samples, to a fraction of a sample.
    fn refine(&self, guess: f32, w: usize) -> f32 {
        if self.dec == 1 {
            return guess;
        }
        let fm = self.full.len() - 1;
        let r = self.dec as isize + 1;
        let centre = guess.round() as isize;
        let lo = (centre - r).max(2) as usize;
        let hi = (centre + r) as usize;
        if self.full_n < w + hi + 2 || w + hi + 2 > self.full.len() {
            return guess;
        }
        let start = self.full_n - (w + hi + 1);
        let x = |j: usize| self.full[(start + j) & fm];
        let d = |tau: usize| {
            let mut s = 0.0f32;
            for j in 0..w {
                let e = x(j) - x(j + tau);
                s += e * e;
            }
            s
        };
        // The lowest of d over lo..=hi, with its neighbours.
        let (mut best, mut best_d) = (lo, f32::INFINITY);
        let mut prev = d(lo - 1);
        let mut cur = d(lo);
        let (mut left, mut right) = (prev, prev);
        for tau in lo..=hi {
            let next = d(tau + 1);
            if cur < best_d {
                best_d = cur;
                best = tau;
                left = prev;
                right = next;
            }
            prev = cur;
            cur = next;
        }
        best as f32 + parabola(left, best_d, right)
    }

    /// The rate it was made for.
    pub fn rate(&self) -> f32 {
        self.rate
    }
}

/// Where the bottom of a parabola through three equally spaced points lies, from the middle
/// one (-0.5..=0.5).
#[inline]
fn parabola(a: f32, b: f32, c: f32) -> f32 {
    let den = a - 2.0 * b + c;
    if den.abs() < 1e-12 || !den.is_finite() {
        0.0
    } else {
        (0.5 * (a - c) / den).clamp(-0.5, 0.5)
    }
}

/// One frame of an offline pitch analysis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PitchFrame {
    /// Seconds from the start of the sound, of the middle of what the estimate looked at.
    pub time: f64,
    /// Hz, 0 where the sound is not voiced.
    pub hz: f32,
    pub aperiodicity: f32,
    pub rms: f32,
}

/// How voiced a frame must be to count (aperiodicity at most this) and how loud (RMS).
pub const VOICED_APERIODICITY: f32 = 0.25;
pub const VOICED_RMS: f32 = 0.003;

/// The pitch of a whole mono sound at `rate`, every `hop` samples, in the Voice range `voice`
/// (for the Piano Roll's correction lane and for measurements). Allocates; not real-time.
pub fn analyze(samples: &[f32], rate: u32, hop: usize, voice: usize) -> Vec<PitchFrame> {
    let rate_f = rate.max(1) as f32;
    let mut t = Tracker::new(rate_f);
    t.set_voice(voice);
    let hop = hop.max(1);
    let back = t.look_back() as f64;
    let mut out = Vec::with_capacity(samples.len() / hop + 1);
    for (i, x) in samples.iter().enumerate() {
        t.push(*x);
        if (i + 1) % hop == 0 {
            let e = t.estimate();
            let voiced =
                e.period > 0.0 && e.aperiodicity <= VOICED_APERIODICITY && e.rms >= VOICED_RMS;
            out.push(PitchFrame {
                time: ((i + 1) as f64 - back).max(0.0) / f64::from(rate_f),
                hz: if voiced { e.hz(rate_f) } else { 0.0 },
                aperiodicity: e.aperiodicity,
                rms: e.rms,
            });
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A sung-like tone: harmonics falling 6 dB an octave, with the pitch given per sample in
    /// Hz, through two formant resonances (700 Hz and 1200 Hz).
    pub fn voice(hz: &[f32], rate: f32) -> Vec<f32> {
        let mut phase = 0.0f64;
        let mut f1 = Resonator::new(700.0, 90.0, rate);
        let mut f2 = Resonator::new(1200.0, 110.0, rate);
        hz.iter()
            .map(|f| {
                phase += f64::from(*f) / f64::from(rate);
                phase -= phase.floor();
                let n = ((rate * 0.45) / f.max(1.0)).floor() as usize;
                let mut s = 0.0f64;
                for k in 1..=n.min(60) {
                    s += (std::f64::consts::TAU * k as f64 * phase).sin() / k as f64;
                }
                let s = s as f32 * 0.2;
                0.5 * f1.run(s) + 0.3 * f2.run(s) + 0.05 * s
            })
            .collect()
    }

    /// A two-pole resonance at `hz` with bandwidth `bw`, gain about 1 at its peak.
    pub struct Resonator {
        a1: f32,
        a2: f32,
        g: f32,
        y1: f32,
        y2: f32,
    }

    impl Resonator {
        pub fn new(hz: f32, bw: f32, rate: f32) -> Self {
            let r = (-std::f32::consts::PI * bw / rate).exp();
            let w = std::f32::consts::TAU * hz / rate;
            Self {
                a1: 2.0 * r * w.cos(),
                a2: -r * r,
                g: 1.0 - r,
                y1: 0.0,
                y2: 0.0,
            }
        }
        pub fn run(&mut self, x: f32) -> f32 {
            let y = self.g * x + self.a1 * self.y1 + self.a2 * self.y2;
            self.y2 = self.y1;
            self.y1 = y;
            y
        }
    }

    pub fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2()
    }

    #[test]
    fn steady_tones_are_found_within_a_cent() {
        for rate in [44100.0f32, 48000.0, 96000.0] {
            for (voice, hz) in [(1, 110.0f32), (1, 220.0), (1, 440.0), (0, 880.0), (2, 65.0)] {
                let x = voice_tone(hz, rate, (rate * 0.3) as usize);
                let mut t = Tracker::new(rate);
                t.set_voice(voice);
                x.iter().for_each(|s| t.push(*s));
                let e = t.estimate();
                let c = cents(e.hz(rate), hz);
                assert!(c.abs() < 1.0, "{rate} Hz rate, {hz} Hz: {c} cents");
                assert!(e.aperiodicity < 0.1, "{hz}: {}", e.aperiodicity);
            }
        }
    }

    fn voice_tone(hz: f32, rate: f32, n: usize) -> Vec<f32> {
        voice(&vec![hz; n], rate)
    }

    #[test]
    fn noise_and_silence_are_not_voiced() {
        let rate = 48000.0;
        let mut t = Tracker::new(rate);
        let noise = crate::native_fx::test_signals::noise(20000, 0.5, 3);
        noise.iter().for_each(|s| t.push(*s));
        let e = t.estimate();
        assert!(e.aperiodicity > VOICED_APERIODICITY, "{e:?}");
        let mut t = Tracker::new(rate);
        (0..20000).for_each(|_| t.push(0.0));
        let e = t.estimate();
        assert!(e.rms < VOICED_RMS);
    }

    #[test]
    fn a_slide_and_vibrato_are_followed() {
        let rate = 48000.0;
        let n = 48000;
        // A slide from 200 to 300 Hz over a second.
        let hz: Vec<f32> = (0..n)
            .map(|i| 200.0 + 100.0 * i as f32 / n as f32)
            .collect();
        let x = voice(&hz, rate);
        let frames = analyze(&x, 48000, 256, 1);
        let mut worst = 0.0f32;
        for f in frames.iter().filter(|f| f.time > 0.05 && f.time < 0.95) {
            let truth = 200.0 + 100.0 * f.time as f32;
            assert!(f.hz > 0.0, "unvoiced at {}", f.time);
            worst = worst.max(cents(f.hz, truth).abs());
        }
        assert!(worst < 8.0, "slide: {worst} cents");
    }

    #[test]
    fn estimating_does_not_allocate() {
        let mut t = Tracker::new(48000.0);
        let x = voice_tone(200.0, 48000.0, 9000);
        assert_no_alloc::assert_no_alloc(|| {
            for s in &x {
                t.push(*s);
            }
            t.estimate()
        });
    }
}
