//! Pro Wave Tune's pitch shifter: pitch-synchronous overlap-add (TD-PSOLA), run causally.
//!
//! The input's pitch periods are marked: a note's first mark is the highest point of a
//! low-passed copy of its newest period; each next one is where the waveform around the last
//! mark comes again, a period on (the best correlation, to a fraction of a sample), so the
//! marks stay in step with each other. Each output period is one grain: two input periods
//! around the mark nearest in time, under a Hann window, laid down at the output's own
//! period. As the grains keep the input's waveform, the formants stay where they are while
//! the pitch moves. A formant shift reads each grain faster or slower (it gets shorter or
//! longer), which moves the formants and not the pitch.
//!
//! The grains are mixed in as the output reaches them, reading the input as it arrives, so a
//! grain only needs its mark, not the input after it. Marks a period or more ahead of the
//! newest found one are carried forward from it by the period (they are found for real a
//! moment later), so a low voice is not held back by its long period: the corrected sound
//! comes [`Psola::latency`] after the input at any pitch.
//!
//! Unvoiced sound (and the moments before the first mark of a note) is the input, delayed by
//! the same amount: breaths and consonants pass untouched. The change between the two is a
//! short crossfade.

/// The most grains sounding at once (a fourfold rise with formants an octave down needs 16).
const MAX_GRAINS: usize = 24;
/// Marks remembered.
const MARKS: usize = 32;
/// The crossfade between the delayed input and the grains, in seconds.
const FADE_SECONDS: f32 = 0.006;

#[derive(Clone, Copy, Debug, Default)]
struct Mark {
    pos: f64,
    period: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Grain {
    /// Where its middle goes in the output (input time).
    centre: f64,
    /// The mark it reads around.
    source: f64,
    /// Half its length in the output, and how fast it reads the input (formant factor).
    half: f32,
    speed: f32,
}

pub struct Psola {
    rate: f32,
    latency: usize,
    left: Vec<f32>,
    right: Vec<f32>,
    /// The low-passed copy the marks are found on.
    marker: Vec<f32>,
    mask: usize,
    lp: [f32; 2],
    lp_k: f32,
    /// Samples taken in.
    n: u64,
    marks: [Mark; MARKS],
    n_marks: usize,
    /// The newest mark, while the sound is voiced.
    last: Option<Mark>,
    grains: [Grain; MAX_GRAINS],
    n_grains: usize,
    /// Where the next output period's grain is centred.
    next: Option<f64>,
    /// What the pitch control said last: voiced, the input period, the output period, the
    /// formant factor (all in samples).
    voiced: bool,
    period: f32,
    out_period: f32,
    speed: f32,
    /// 0 the delayed input, 1 the grains; the step per sample.
    wet: f32,
    fade_step: f32,
    /// The delayed input of the last sample out.
    dry: (f32, f32),
    /// Scratch for the mark search's correlations.
    corr: Vec<f32>,
}

impl Psola {
    /// A shifter at `rate` Hz with a delay of `latency` samples. Allocates.
    pub fn new(rate: f32, latency: usize) -> Self {
        let rate = if rate.is_finite() && rate >= 8000.0 {
            rate
        } else {
            48000.0
        };
        // Room for the delay and four of the slowest periods (50 Hz) behind it.
        let len = (latency + (rate / 50.0 * 4.0) as usize + 64).next_power_of_two();
        Self {
            rate,
            latency: latency.max(1),
            left: vec![0.0; len],
            right: vec![0.0; len],
            marker: vec![0.0; len],
            mask: len - 1,
            lp: [0.0; 2],
            lp_k: 1.0 - (-std::f32::consts::TAU * 900.0 / rate).exp(),
            n: 0,
            marks: [Mark::default(); MARKS],
            n_marks: 0,
            last: None,
            grains: [Grain::default(); MAX_GRAINS],
            n_grains: 0,
            next: None,
            voiced: false,
            period: 0.0,
            out_period: 0.0,
            speed: 1.0,
            wet: 0.0,
            fade_step: 1.0 / (FADE_SECONDS * rate).max(1.0),
            dry: (0.0, 0.0),
            corr: vec![0.0; (rate / 50.0 * 0.3) as usize + 8],
        }
    }

    /// The delay in samples.
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// The pitch to work with from now on: whether the input is voiced, its period, the period
    /// the output should have, and the formant factor (above 1 moves the formants up). Periods
    /// in samples.
    pub fn set_control(&mut self, voiced: bool, period: f32, out_period: f32, formant: f32) {
        let ok = voiced
            && period.is_finite()
            && out_period.is_finite()
            && period >= 8.0
            && (period as usize) * 4 < self.mask
            && out_period >= 2.0;
        self.voiced = ok;
        if ok {
            self.period = period;
            self.out_period = out_period.clamp(period / 4.5, period * 4.5);
        } else {
            // A new note starts from its own marks.
            self.last = None;
            self.n_marks = 0;
        }
        self.speed = if formant.is_finite() {
            formant.clamp(0.25, 4.0)
        } else {
            1.0
        };
    }

    /// Forgets everything heard.
    pub fn reset(&mut self) {
        self.left.iter_mut().for_each(|x| *x = 0.0);
        self.right.iter_mut().for_each(|x| *x = 0.0);
        self.marker.iter_mut().for_each(|x| *x = 0.0);
        self.lp = [0.0; 2];
        self.n = 0;
        self.n_marks = 0;
        self.last = None;
        self.n_grains = 0;
        self.next = None;
        self.wet = 0.0;
        self.voiced = false;
    }

    #[inline]
    fn at(buf: &[f32], mask: usize, i: i64) -> f32 {
        if i < 0 {
            0.0
        } else {
            buf[i as usize & mask]
        }
    }

    /// The input at a fractional position (cubic Hermite).
    #[inline]
    fn read(&self, buf: &[f32], pos: f64) -> f32 {
        let i = pos.floor();
        let t = (pos - i) as f32;
        let i = i as i64;
        let m = self.mask;
        let (y0, y1, y2, y3) = (
            Self::at(buf, m, i - 1),
            Self::at(buf, m, i),
            Self::at(buf, m, i + 1),
            Self::at(buf, m, i + 2),
        );
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * t + c2) * t + c1) * t + y1
    }

    /// The highest point of the marker signal in `from..to` (sample positions).
    fn peak(&self, from: u64, to: u64) -> u64 {
        let mut best = from;
        let mut v = f32::NEG_INFINITY;
        for i in from..to {
            let x = self.marker[i as usize & self.mask];
            if x > v {
                v = x;
                best = i;
            }
        }
        best
    }

    fn push_mark(&mut self, m: Mark) {
        if self.n_marks == MARKS {
            self.marks.copy_within(1.., 0);
            self.n_marks -= 1;
        }
        self.marks[self.n_marks] = m;
        self.n_marks += 1;
        self.last = Some(m);
    }

    /// Finds the next mark once the input is far enough past the last one.
    fn find_marks(&mut self) {
        if !self.voiced {
            return;
        }
        let p = self.period as f64;
        match self.last {
            None => {
                // A new note: the highest point of the newest period.
                let span = p.ceil() as u64;
                if self.n > span + 2 {
                    let at = self.peak(self.n - span, self.n);
                    self.push_mark(Mark {
                        pos: at as f64,
                        period: self.period,
                    });
                }
            }
            Some(last) => {
                if (self.n as f64) > last.pos + 3.0 * p {
                    // The pitch moved too far: start again from the newest period.
                    self.last = None;
                    return self.find_marks();
                }
                // The next mark is where the waveform around the last one comes again: the
                // best match of the period before it (and a little after), a period on.
                let m = last.pos.round() as i64;
                let lo = (0.85 * p).floor() as i64;
                let hi = (1.15 * p).ceil() as i64;
                let back = (0.6 * p).ceil() as i64;
                let fwd = (0.2 * p).ceil() as i64;
                if (self.n as i64) < m + hi + fwd + 3 {
                    return;
                }
                let x = |i: i64| {
                    0.5 * (Self::at(&self.left, self.mask, i) + Self::at(&self.right, self.mask, i))
                };
                let mut e1 = 0.0f32;
                for k in -back..fwd {
                    e1 += x(m + k) * x(m + k);
                }
                let lags = ((hi - lo + 3) as usize).min(self.corr.len());
                for (j, tau) in (lo - 1..lo - 1 + lags as i64).enumerate() {
                    let (mut c, mut e2) = (0.0f32, 0.0f32);
                    for k in -back..fwd {
                        let b = x(m + tau + k);
                        c += x(m + k) * b;
                        e2 += b * b;
                    }
                    self.corr[j] = if e1 > 0.0 && e2 > 0.0 {
                        c / (e1 * e2).sqrt()
                    } else {
                        0.0
                    };
                }
                let mut best = 1;
                for j in 1..lags - 1 {
                    if self.corr[j] > self.corr[best] {
                        best = j;
                    }
                }
                let pos = if self.corr[best] > 0.3 {
                    let (a, b, c) = (self.corr[best - 1], self.corr[best], self.corr[best + 1]);
                    let den = a - 2.0 * b + c;
                    let frac = if den < 0.0 {
                        (0.5 * (a - c) / den).clamp(-0.5, 0.5)
                    } else {
                        0.0
                    };
                    // The lag was found from the whole sample nearest the last mark; the next
                    // mark is that far from the mark itself (its fraction kept).
                    last.pos + (lo - 1 + best as i64) as f64 + f64::from(frac)
                } else {
                    // Nothing like it: the highest point near a period on.
                    let a = (last.pos + 0.75 * p).ceil() as u64;
                    let b = (last.pos + 1.25 * p).ceil() as u64 + 1;
                    self.peak(a, b.min(self.n)) as f64
                };
                self.push_mark(Mark {
                    pos,
                    period: self.period,
                });
            }
        }
    }

    /// The mark nearest to `pos`: a remembered one, or one carried forward from the newest
    /// by whole periods, as long as the input it is read from will have arrived (no later
    /// than `pos` plus the delay).
    fn nearest(&self, pos: f64) -> Option<Mark> {
        let mut best = self.marks[..self.n_marks]
            .iter()
            .min_by(|a, b| (a.pos - pos).abs().total_cmp(&(b.pos - pos).abs()))
            .copied();
        if let Some(last) = self.last {
            let limit = pos + self.latency as f64 - 2.0;
            let p = f64::from(self.period.max(2.0));
            for k in 1..=4 {
                let a = last.pos + k as f64 * p;
                if a > limit {
                    break;
                }
                if best.is_none_or(|b| (a - pos).abs() < (b.pos - pos).abs()) {
                    best = Some(Mark {
                        pos: a,
                        period: self.period,
                    });
                }
            }
        }
        best
    }

    /// Starts the grains due at output position `t`.
    fn start_grains(&mut self, t: f64) {
        if !self.voiced || self.n_marks == 0 {
            self.next = None;
            return;
        }
        let half_of = |s: &Self, m: &Mark| (m.period / s.speed).max(2.0);
        let next = match self.next {
            Some(x) => x,
            None => {
                // The first grain of a note sits on a mark (in step with the input), the
                // first one still to come.
                let m = self.marks[self.n_marks - 1];
                let half = half_of(self, &m) as f64;
                let mut c = m.pos;
                while c - half < t {
                    c += self.out_period as f64;
                }
                c
            }
        };
        let mut c = next;
        while let Some(m) = self.nearest(c) {
            let half = half_of(self, &m);
            if c - half as f64 > t {
                break;
            }
            if self.n_grains < MAX_GRAINS && c + (half as f64) > t {
                self.grains[self.n_grains] = Grain {
                    centre: c,
                    source: m.pos,
                    half,
                    speed: self.speed,
                };
                self.n_grains += 1;
            }
            c += self.out_period.max(2.0) as f64;
        }
        self.next = Some(c);
    }

    /// One stereo sample in, one out (delayed by [`Psola::latency`]). No allocation.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let i = self.n as usize & self.mask;
        let (l, r) = (
            if l.is_finite() { l } else { 0.0 },
            if r.is_finite() { r } else { 0.0 },
        );
        self.left[i] = l;
        self.right[i] = r;
        self.lp[0] += self.lp_k * (0.5 * (l + r) - self.lp[0]);
        self.lp[1] += self.lp_k * (self.lp[0] - self.lp[1]);
        self.marker[i] = self.lp[1];
        self.n += 1;
        self.find_marks();
        let t = self.n as f64 - 1.0 - self.latency as f64;
        if t < 0.0 {
            return (0.0, 0.0);
        }
        let dry = (
            Self::at(&self.left, self.mask, t as i64),
            Self::at(&self.right, self.mask, t as i64),
        );
        self.dry = dry;
        // The grains only start while voiced; the ones begun play out.
        let target = if self.voiced { 1.0 } else { 0.0 };
        if self.voiced {
            self.start_grains(t);
        } else {
            self.next = None;
        }
        let (mut al, mut ar, mut wsum) = (0.0f32, 0.0f32, 0.0f32);
        let mut k = 0;
        let newest = self.n as f64 - 3.0;
        while k < self.n_grains {
            let g = self.grains[k];
            let x = (t - g.centre) as f32;
            if x >= g.half {
                self.grains[k] = self.grains[self.n_grains - 1];
                self.n_grains -= 1;
                continue;
            }
            if x > -g.half {
                let w = 0.5 + 0.5 * (std::f32::consts::PI * x / g.half).cos();
                let src = (g.source + f64::from(x * g.speed)).min(newest);
                al += w * self.read(&self.left, src);
                ar += w * self.read(&self.right, src);
                wsum += w;
            }
            k += 1;
        }
        // Where the grains overlap more than once (a rise) they are brought back to level;
        // where they leave gaps the delayed input fills in.
        let norm = 1.0 / wsum.max(1.0);
        let fill = (1.0 - wsum * 4.0).max(0.0);
        let wet = (al * norm + fill * dry.0, ar * norm + fill * dry.1);
        if self.wet < target {
            self.wet = (self.wet + self.fade_step).min(1.0);
        } else if self.wet > target {
            self.wet = (self.wet - self.fade_step).max(0.0);
        }
        let g = self.wet;
        (g * wet.0 + (1.0 - g) * dry.0, g * wet.1 + (1.0 - g) * dry.1)
    }

    /// The rate it was made for.
    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// The delayed input of the last sample out (for a dry/wet mix in step with it).
    pub fn dry(&self) -> (f32, f32) {
        self.dry
    }

    /// Whether grains are sounding (the output is not just the delayed input).
    pub fn shifting(&self) -> bool {
        self.wet > 0.0
    }
}
