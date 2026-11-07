// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// Originally part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// Ported to Pro Wave from Lemon Studio's crates/os-engine/src/effects/maximus/mod.rs (the
// sound) with the half-band stage from crates/os-engine/src/effects/eq/hq.rs. Changes for Pro
// Wave: every buffer is allocated when the effect is made (fixed biquad cascades and smoother
// stages; delay lines, detector windows, the saturation table and the linear-phase crossover
// sized for their longest setting at the oversampling the effect is opened with), host blocks
// are processed in pieces of at most 512 frames, the editor's meters are replaced by a gain
// reduction reading, and the saved state is replaced by parameters (the maps are FL's factory
// map; solo and the shown band, editor settings, are left out).

//! Press: Lemon Studio's multiband maximiser (a recreation of FL's Maximus): a low cut, an
//! optional upsampling, a split into LOW, MID and HIGH (biquads or linear-phase FIR filters),
//! each band and then their sum (MASTER) through the same chain (separation, level detector,
//! the band's map as a gain table, attack and release stages, look-ahead, gain, saturation,
//! mix).
//!
//! Logic Pro equivalent: Multipressor followed by the Adaptive Limiter.
//!
// spec: plugins/maximus.md, audio/conventions.md ("Glide"), plugins/fruity-waveshaper.md ("Oversampling"),
// plugins/fruity-parametric-eq-2.md ("A section")

// The loops and comparisons keep the original's shape (NaN-aware `!(a > b)` tests, indexed
// loops) so the port can be diffed against Lemon Studio line by line.
#![allow(
    clippy::needless_range_loop,
    clippy::neg_cmp_op_on_partial_ord,
    clippy::needless_late_init
)]

pub mod map;
pub mod params;

use std::collections::VecDeque;

use self::map::{Map, Table};
use self::params::*;
pub use self::params::{value_text, PARAMS};
use super::{block_effect, stored_values, NativeFx};

pub const ID: &str = "prowave:press";
pub const NAME: &str = "Press";

const TINY: f64 = 1.0 / 16_777_216.0;
const PI: f64 = std::f64::consts::PI;
/// The longest piece of a host block processed at once.
const CHUNK: usize = 512;
/// Most biquads in one cascade (the low cut's four).
const MAX_BQ: usize = 4;
/// Most attack or release stages.
const MAX_STAGES: usize = 8;
/// Most oversampling stages (16 times).
const MAX_OVERSAMPLING: u32 = 4;
/// The saturation table's entries.
const SAT_ENTRIES: usize = 65538;

fn s(x: f64) -> f32 {
    x as f32
}

fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// The gain knobs' law (pre, post, ceiling): 0 at 0, 1 at 1000, 8 at 2000.
pub fn gain(v: i32) -> f32 {
    if v == 1000 {
        return 1.0;
    }
    let a = v as f32 * s(0.001);
    let b = a * s(7f64.ln());
    s(((f64::from(b)).exp() - 1.0) * f64::from(s(1.0 / 6.0)))
}

/// The time knobs' law: 0 .. 1 s in samples at `rate` (a double).
pub fn samples(v: i32, rate: f64) -> f64 {
    let x = v as f32 * s(0.0001);
    let m = s((f64::from(x) * 501f64.ln()).exp() - 1.0);
    f64::from(m) * 2.0 * 0.001 * rate
}

/// A crossover knob in Hz: 30 Hz .. 16 kHz.
pub fn crossover_hz(v: i32) -> f32 {
    s(
        (f64::from(v) * (533.333_333_333_333_4f64.ln() / (2f64.ln() * 65536.0)) * 2f64.ln()).exp()
            * 30.0,
    )
}

/// The low cut knob in Hz: 5 .. 80 Hz.
pub fn low_cut_hz(v: i32) -> f32 {
    s((f64::from(v) * (16f64.ln() / (2f64.ln() * 65536.0)) * 2f64.ln()).exp() * 5.0)
}

/// A smoother's coefficient for a time of `t` samples.
fn coef(t: f64) -> f64 {
    if t <= 0.0 {
        0.0
    } else {
        (0.01f64.ln() / t.max(1.0)).exp()
    }
}

/// Biquad coefficients `(b0, b1, b2, a1, a2)`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Coefs([f64; 5]);

impl Coefs {
    fn lowpass(alpha: f64, w: f64) -> Coefs {
        let (n, cs) = (1.0 / (alpha + 1.0), w.cos());
        let b1 = n * (1.0 - cs);
        Coefs([b1 * 0.5, b1, b1 * 0.5, n * cs * -2.0, n * (1.0 - alpha)])
    }

    fn highpass(alpha: f64, w: f64) -> Coefs {
        let (n, cs) = (1.0 / (alpha + 1.0), w.cos());
        let b1 = n * -(cs + 1.0);
        Coefs([-b1 * 0.5, b1, -b1 * 0.5, n * cs * -2.0, n * (1.0 - alpha)])
    }

    /// The all-pass that keeps LOW in phase behind a 24 dB upper edge: `(b0, b1, b2) = (a2, a1, 1)`.
    fn allpass(alpha: f64, w: f64) -> Coefs {
        let (n, cs) = (1.0 / (alpha + 1.0), w.cos());
        let (a1, a2) = (n * cs * -2.0, n * (1.0 - alpha));
        Coefs([a2, a1, 1.0, a1, a2])
    }
}

/// One biquad's state on one channel.
#[derive(Debug, Clone, Copy, Default)]
struct Bq {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Bq {
    fn run(&mut self, c: &Coefs, x: f32) -> f32 {
        let [b0, b1, b2, a1, a2] = c.0;
        let x = f64::from(x);
        let y = x * b0 + (((self.x1 * b1 - self.y2 * a2) + self.x2 * b2) - self.y1 * a1);
        (self.x2, self.x1, self.y2, self.y1) = (self.x1, x, self.y1, y);
        y as f32
    }

    fn tidy(&mut self) {
        for v in [&mut self.x1, &mut self.x2, &mut self.y1, &mut self.y2] {
            if v.abs() < TINY {
                *v = 0.0;
            }
        }
    }
}

/// A cascade of identical biquads on two channels (at most [`MAX_BQ`]).
#[derive(Debug, Clone, Default)]
struct Chain {
    c: [Coefs; MAX_BQ],
    n: usize,
    st: [[Bq; MAX_BQ]; 2],
}

impl Chain {
    /// New coefficients; the states of the biquads kept stay, added ones start empty.
    fn set(&mut self, c: &[Coefs]) {
        let n = c.len().min(MAX_BQ);
        for st in &mut self.st {
            st[n.min(self.n)..n.max(self.n)].fill(Bq::default());
        }
        self.c[..n].copy_from_slice(&c[..n]);
        self.n = n;
    }

    fn is_empty(&self) -> bool {
        self.n == 0
    }

    fn clear(&mut self) {
        for st in &mut self.st {
            st.fill(Bq::default());
        }
    }

    fn tidy(&mut self) {
        for st in &mut self.st {
            for b in &mut st[..self.n] {
                b.tidy();
            }
        }
    }

    fn run_all(&mut self, ch: usize, x: &mut [f32]) {
        self.run(0..self.n, ch, x);
    }

    /// Runs the biquads `range` in place.
    fn run(&mut self, range: std::ops::Range<usize>, ch: usize, x: &mut [f32]) {
        let n = range.len();
        for (c, st) in self.c[..self.n]
            .iter()
            .zip(self.st[ch].iter_mut())
            .skip(range.start)
            .take(n)
        {
            for v in x.iter_mut() {
                *v = st.run(c, *v);
            }
        }
    }
}

/// Peak or RMS detector over a window of `w` values (at most `cap`).
#[derive(Debug, Clone)]
struct Detector {
    w: usize,
    cap: usize,
    rms: bool,
    /// Peak: the values that can still be the window's largest, with their sample numbers (wrapping).
    peaks: VecDeque<(u32, f32)>,
    t: u32,
    /// RMS: the last `w` squares and their running sum.
    ring: Vec<f32>,
    at: usize,
    sum: f64,
    scale: f64,
}

impl Detector {
    fn with_room(cap: usize) -> Detector {
        let cap = cap.max(1);
        Detector {
            w: 0,
            cap,
            rms: false,
            peaks: VecDeque::with_capacity(cap + 1),
            t: 0,
            ring: Vec::with_capacity(cap),
            at: 0,
            sum: 0.0,
            scale: 0.0,
        }
    }

    fn start(&mut self, w: usize, rms: bool) {
        self.w = w.clamp(1, self.cap);
        self.rms = rms;
        self.peaks.clear();
        self.t = 0;
        self.ring.clear();
        if rms {
            self.ring.resize(self.w, 0.0);
        }
        self.at = 0;
        self.sum = 0.0;
        self.scale = f64::from(s(1.0 / (2.0 * self.w as f64)));
    }

    fn level(&mut self, l: f32, r: f32) -> f32 {
        if self.rms {
            let q = l * l + r * r;
            let old = std::mem::replace(&mut self.ring[self.at], q);
            self.at = if self.at + 1 == self.w {
                0
            } else {
                self.at + 1
            };
            self.sum = (self.sum - f64::from(old)) + f64::from(q);
            let mean = s(self.sum * self.scale).max(0.0);
            mean.sqrt()
        } else {
            let v = l.abs().max(r.abs());
            while self.peaks.back().is_some_and(|&(_, p)| p <= v) {
                self.peaks.pop_back();
            }
            // A NaN never leaves the back; keep the queue within its room regardless.
            if self.peaks.len() == self.peaks.capacity() {
                self.peaks.pop_front();
            }
            self.peaks.push_back((self.t, v));
            let now = self.t;
            while self
                .peaks
                .front()
                .is_some_and(|&(t, _)| now.wrapping_sub(t) as usize >= self.w)
            {
                self.peaks.pop_front();
            }
            self.t = self.t.wrapping_add(1);
            self.peaks.front().map_or(0.0, |&(_, p)| p)
        }
    }
}

/// The saturation of a band.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SatKind {
    Off,
    Table { scale: f64, ceil: f64 },
    Knee { knee: f32, width: f32, r: f32 },
}

#[derive(Debug, Clone)]
struct Sat {
    kind: SatKind,
    /// Mode A's table ([`SAT_ENTRIES`], made with room for it).
    t: Vec<f32>,
}

impl Sat {
    fn with_room() -> Sat {
        Sat {
            kind: SatKind::Off,
            t: Vec::with_capacity(SAT_ENTRIES),
        }
    }

    /// Works the saturation out again in place (mode A fills its table: costly, no allocation).
    fn set(&mut self, threshold: i32, ceil_knob: i32, reach_used: f64, post: f32) {
        let ceil = gain(ceil_knob);
        if threshold == 0 {
            self.kind = SatKind::Off;
            return;
        }
        if threshold > 0 {
            let knee = s(f64::from(1.0 - threshold as f32 * s(0.001)) * f64::from(ceil));
            let mut width = ceil - knee;
            if width <= 0.0 {
                width = 1.0 / 65536.0;
            }
            self.kind = SatKind::Knee {
                knee,
                width,
                r: 1.0 / width,
            };
            return;
        }
        let loud = (reach_used * f64::from(post)).max(f64::from(ceil));
        let depth = f64::from(s(f64::from(threshold) * -0.1) + 1.0);
        let q = s(depth.ln() / 101f64.ln());
        let hardness = f64::from(20.0 - q * 19.0);
        let f = |x: f64| x.powf(hardness).atan().powf(1.0 / hardness);
        let step = if ceil == 0.0 {
            1.0
        } else {
            loud / (f64::from(ceil) * 65536.0)
        };
        let top = f(step * 65536.0);
        self.t.clear();
        for i in 0..=65536 {
            let v = s(f(i as f64 * step) / top);
            self.t.push(if f64::from(v) < TINY { 0.0 } else { v });
        }
        self.t.push(self.t[65536]);
        self.kind = SatKind::Table {
            scale: f64::from(s(1.0 / loud)) * 65536.0,
            ceil: f64::from(ceil),
        };
    }

    fn run(&self, x: f32) -> f32 {
        match self.kind {
            SatKind::Off => x,
            SatKind::Table { scale, ceil } => {
                let pos = f64::from(x) * scale;
                if !(pos > -2_147_483_648.0 && pos < 2_147_483_648.0) {
                    return 0.0;
                }
                let w = pos.trunc();
                let fr = pos - w;
                let i = (w.abs() as usize).min(65536);
                let (a, b) = (f64::from(self.t[i]), f64::from(self.t[i + 1]));
                let y = if w >= 0.0 {
                    a + (b - a) * fr
                } else {
                    -(a - (b - a) * fr)
                };
                to_single_towards_zero(y * ceil)
            }
            SatKind::Knee { knee, width, r } => {
                let a = x.abs();
                if a <= knee {
                    return x;
                }
                let (k, wd, r, a) = (
                    f64::from(knee),
                    f64::from(width),
                    f64::from(r),
                    f64::from(a),
                );
                s((1.0 - ((k - a) * r).exp()) * wd + k).copysign(x)
            }
        }
    }
}

/// A double rounded to single towards zero.
fn to_single_towards_zero(y: f64) -> f32 {
    let v = y as f32;
    if f64::from(v).abs() > y.abs() {
        f32::from_bits(v.to_bits() - 1)
    } else {
        v
    }
}

/// A stereo delay line (at most its capacity long).
#[derive(Debug, Clone)]
struct Line {
    buf: [Vec<f32>; 2],
    at: usize,
}

impl Line {
    fn with_room(cap: usize) -> Line {
        Line {
            buf: [Vec::with_capacity(cap), Vec::with_capacity(cap)],
            at: 0,
        }
    }

    fn cap(&self) -> usize {
        self.buf[0].capacity().min(self.buf[1].capacity())
    }

    fn len(&self) -> usize {
        self.buf[0].len()
    }

    fn set(&mut self, len: usize) {
        let len = len.min(self.cap());
        for b in &mut self.buf {
            b.clear();
            b.resize(len, 0.0);
        }
        self.at = 0;
    }

    fn clear(&mut self) {
        for b in &mut self.buf {
            b.fill(0.0);
        }
    }

    fn push(&mut self, l: f32, r: f32) -> (f32, f32) {
        let n = self.buf[0].len();
        if n == 0 {
            return (l, r);
        }
        let out = (
            std::mem::replace(&mut self.buf[0][self.at], l),
            std::mem::replace(&mut self.buf[1][self.at], r),
        );
        self.at = if self.at + 1 == n { 0 } else { self.at + 1 };
        out
    }
}

/// The separation's matrix `(LL, RR, RL, LR)` for a knob value.
fn separation(knob: i32) -> [f32; 4] {
    let x = knob as f32 * s(0.001);
    if x == 0.0 {
        return [1.0, 1.0, 0.0, 0.0];
    }
    let q = PI / 4.0;
    let u = s(1.0 / q.cos());
    let l = s(q.cos() * f64::from(u));
    let r = s(q.sin() * f64::from(u));
    let m = if x < 0.0 {
        x
    } else {
        s((f64::from(x) * 5f64.ln()).exp() - 1.0) * 0.25
    };
    let k = m.min(1.0) * 0.5;
    [l * (1.0 - k), r * (1.0 - k), l * k, r * k]
}

/// One band's chain and its settings.
#[derive(Debug, Clone)]
struct Band {
    /// The mode in effect.
    mode: i32,
    look: usize,
    att_n: usize,
    rel_n: usize,
    /// Attack stages: rise (rel2) and fall (attack) coefficients; release stages: the fall coefficient.
    att_rise: f64,
    att_fall: f64,
    rel_fall: f64,
    att: [f64; MAX_STAGES],
    rel: [f64; MAX_STAGES],
    pre: f32,
    post: f32,
    rest: f32,
    sep: [f32; 4],
    det: Detector,
    line: Line,
    sat: Sat,
    sat_dirty: bool,
    reach_used: f64,
    sign: f32,
    /// The smallest gain of the last block (f32::MAX: the compressor did not run).
    min_g: f32,
}

impl Band {
    fn with_room(cap: usize) -> Band {
        Band {
            mode: 0,
            look: 0,
            att_n: 0,
            rel_n: 0,
            att_rise: 0.0,
            att_fall: 0.0,
            rel_fall: 0.0,
            att: [1.0; MAX_STAGES],
            rel: [0.0; MAX_STAGES],
            pre: 1.0,
            post: 1.0,
            rest: 1.0,
            sep: [1.0, 1.0, 0.0, 0.0],
            det: Detector::with_room(cap),
            line: Line::with_room(cap),
            sat: Sat::with_room(),
            sat_dirty: true,
            reach_used: 4.0,
            sign: 1.0,
            min_g: f32::MAX,
        }
    }

    fn reset(&mut self) {
        self.att.fill(f64::from(self.rest));
        self.rel.fill(0.0);
        self.line.clear();
    }

    fn tidy(&mut self) {
        for v in self.att[..self.att_n]
            .iter_mut()
            .chain(self.rel[..self.rel_n].iter_mut())
        {
            if v.abs() <= TINY {
                *v = 0.0;
            }
        }
    }

    /// The chain on `n` samples of `d` (the band, separated in place, feeding the detector). `audio` is the signal the
    /// gain acts on when it is not the band itself (mid trigger: the unsplit input); the result is left there, else
    /// in `d`. `mix` is the LMH mix (`None` for MASTER).
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        dl: &mut [f32],
        dr: &mut [f32],
        mut audio: Option<(&mut [f32], &mut [f32])>,
        n: usize,
        target: [f32; 4],
        limit: f32,
        jump: bool,
        mix: Option<f32>,
        table: &Table,
    ) {
        self.min_g = f32::MAX;
        if self.mode == MODE_OFF {
            // Only MASTER runs in mode 3: its input times post.
            let (l, r) = match audio.as_mut() {
                Some((l, r)) => (&mut **l, &mut **r),
                None => (&mut *dl, &mut *dr),
            };
            for v in l[..n].iter_mut().chain(r[..n].iter_mut()) {
                *v *= self.post;
            }
            return;
        }
        if self.mode == MODE_MUTED {
            return;
        }
        // The separation's glide for this block.
        if jump {
            self.sep = target;
        }
        let mut step = [0.0f32; 4];
        let mut now = self.sep;
        for k in 0..4 {
            let cur = self.sep[k];
            let mut st = (target[k] - cur) / n as f32;
            let end;
            if st.abs() <= limit {
                end = target[k];
            } else {
                st = limit.copysign(target[k] - cur);
                let mut e = cur + st * n as f32;
                if f64::from(e).abs() < TINY {
                    e = 0.0;
                    st = -cur / n as f32;
                }
                end = e;
            }
            step[k] = st;
            self.sep[k] = end;
        }
        let comp = self.mode == MODE_ON;
        let (pre, post) = (self.pre, self.post);
        let pp = pre * post;
        let pk = pre * 8192.25;
        let topg = s(table.top * f64::from(post));
        let (mixed, a, b) = match mix {
            Some(m) if m != 1.0 => (true, (1.0 - m) * self.sign, m * self.sign),
            _ => (false, 0.0, self.sign),
        };
        for i in 0..n {
            let [ll, rr, rl, lr] = now;
            let (x, y) = (dl[i], dr[i]);
            let (sl, sr) = (x * ll + y * rl, y * rr + x * lr);
            dl[i] = sl;
            dr[i] = sr;
            for k in 0..4 {
                now[k] += step[k];
            }
            let mut g = post;
            if comp {
                let mut v = self.det.level(sl, sr);
                for st in self.rel[..self.rel_n].iter_mut() {
                    let c = if f64::from(v) >= *st {
                        0.0
                    } else {
                        self.rel_fall
                    };
                    *st = f64::from(v) + (*st - f64::from(v)) * c;
                    v = s(*st);
                }
                let p = v * pk;
                let j = (p.trunc() as i64).max(0) as usize;
                let mut gv = if j > table.limit {
                    topg / v
                } else {
                    let (c0, c1) = (table.c[j], table.c[j + 1]);
                    (c0 + (c1 - c0) * (p - j as f32)) * pp
                };
                for st in self.att[..self.att_n].iter_mut() {
                    let c = if f64::from(gv) >= *st {
                        self.att_rise
                    } else {
                        self.att_fall
                    };
                    *st = f64::from(gv) + (*st - f64::from(gv)) * c;
                    gv = s(*st);
                }
                g = gv;
                self.min_g = self.min_g.min(gv);
            }
            let (al, ar) = match audio.as_ref() {
                Some((l, r)) => (l[i], r[i]),
                None => (sl, sr),
            };
            let (ul, ur) = self.line.push(al, ar);
            let mut ol = self.sat.run(ul * g);
            let mut or = self.sat.run(ur * g);
            if mix.is_some() {
                if mixed {
                    ol = ul.mul_add(a, ol * b);
                    or = ur.mul_add(a, or * b);
                } else {
                    ol *= b;
                    or *= b;
                }
            }
            match audio.as_mut() {
                Some((l, r)) => {
                    l[i] = ol;
                    r[i] = or;
                }
                None => {
                    dl[i] = ol;
                    dr[i] = or;
                }
            }
        }
    }
}

/// The linear-phase crossover: one FIR filter per band.
#[derive(Debug, Clone)]
struct Fir {
    len: usize,
    cap: usize,
    /// LOW, MID, HIGH taps (tap `k` meets the input of `k` samples ago).
    taps: [Vec<f32>; 3],
    /// Per band and channel the input history, written twice over so a window is one slice.
    hist: [[Vec<f32>; 2]; 3],
    at: [usize; 3],
    /// The edges' taps while they are worked out.
    lo: Vec<f64>,
    hi: Vec<f64>,
}

impl Fir {
    fn with_room(cap: usize) -> Fir {
        let v = || Vec::with_capacity(cap);
        let h = || [Vec::with_capacity(2 * cap), Vec::with_capacity(2 * cap)];
        Fir {
            len: 0,
            cap,
            taps: [v(), v(), v()],
            hist: [h(), h(), h()],
            at: [0; 3],
            lo: Vec::with_capacity(cap),
            hi: Vec::with_capacity(cap),
        }
    }

    fn set_len(&mut self, len: usize) {
        let len = len.min(self.cap);
        if self.len != len {
            self.len = len;
            for h in self.hist.iter_mut().flatten() {
                h.clear();
                h.resize(2 * len, 0.0);
            }
            self.at = [0; 3];
        }
    }

    fn clear(&mut self) {
        for h in self.hist.iter_mut().flatten() {
            h.fill(0.0);
        }
    }

    /// The taps of an edge at `f` Hz (`f_lim` limited to half the band rate) with slope `slope`, low- or high-pass,
    /// into `h`.
    #[allow(clippy::too_many_arguments)]
    fn edge(
        h: &mut Vec<f64>,
        len: usize,
        f: f64,
        f_lim: f64,
        slope: i32,
        ratio: f64,
        high: bool,
        rate_b: f64,
    ) {
        let n = ((round(ratio * 27500.0 * f64::from(slope + 1) / f) | 1).max(5) as usize).min(len);
        let fr = f64::from(s(f_lim / rate_b.trunc()));
        let q = 2.0 * PI / (n as f64 - 1.0);
        let mid = (n - 1) / 2;
        h.clear();
        h.resize(len, 0.0);
        let off = (len - n) / 2;
        for i in 0..n {
            let mut v = if i == mid {
                2.0 * fr
            } else {
                let t = f64::from(s(i as f64 - (n as f64 - 1.0) * 0.5));
                (2.0 * PI * fr * t).sin() / (PI * t)
            };
            if high {
                v = -v + if i == mid { 1.0 } else { 0.0 };
            }
            let w = 0.35875 - 0.48829 * (q * i as f64).cos() + 0.14128 * (2.0 * q * i as f64).cos()
                - 0.01168 * (3.0 * q * i as f64).cos();
            h[off + i] = v * w;
        }
    }

    fn run(&mut self, band: usize, ch: usize, x: &[f32], out: &mut [f32]) {
        let len = self.len;
        if len == 0 {
            out[..x.len()].copy_from_slice(x);
            return;
        }
        let taps = &self.taps[band];
        let hist = &mut self.hist[band][ch];
        let mut at = self.at[band];
        for (i, &v) in x.iter().enumerate() {
            hist[at] = v;
            hist[at + len] = v;
            // The newest sample is at `at`; tap k meets `at - k`.
            let base = at + len;
            let mut acc = 0.0f64;
            for (k, &t) in taps.iter().enumerate() {
                acc += f64::from(t) * f64::from(hist[base - k]);
            }
            out[i] = acc as f32;
            at = if at + 1 == len { 0 } else { at + 1 };
        }
        if ch == 1 {
            self.at[band] = at;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// HQ Legacy's half-band up- and downsampler (from Lemon Studio's eq/hq.rs)

const CA: [f64; 3] = [0.1473771136010466, 0.48246854276970014, 0.8830050257693731];
const CB: [f64; 3] = [0.039151597734460045, 0.3026468483284934, 0.6746159185469639];
/// Previous outputs below this are cleared at block ends: 2^-24.
const HB_TINY: f64 = 5.960464477539063e-8;

/// A first-order all-pass `y = x' + c * (x - y')` with its own previous input and output.
#[derive(Clone, Copy, Debug, Default)]
struct AllPass {
    xp: f64,
    yp: f64,
}

fn path(p: &mut [AllPass; 3], c: &[f64; 3], x: f64) -> f64 {
    let mut x = x;
    for (ap, &c) in p.iter_mut().zip(c) {
        let y = ap.xp + c * (x - ap.yp);
        ap.xp = x;
        ap.yp = y;
        x = y;
    }
    x
}

/// Two paths of three all-passes.
#[derive(Clone, Copy, Debug, Default)]
struct HalfBand {
    a: [AllPass; 3],
    b: [AllPass; 3],
}

impl HalfBand {
    /// Up: both paths on `x`, path B's output first.
    fn up(&mut self, x: f32) -> [f32; 2] {
        let x = f64::from(x);
        let b = path(&mut self.b, &CB, x);
        let a = path(&mut self.a, &CA, x);
        [b as f32, a as f32]
    }

    /// Down: the first of a pair through path A, the second through path B.
    fn down(&mut self, first: f32, second: f32) -> f32 {
        let a = path(&mut self.a, &CA, f64::from(first));
        let b = path(&mut self.b, &CB, f64::from(second));
        ((a + b) * 0.5) as f32
    }

    /// Block end: the previous outputs below 2^-24 are cleared (the previous inputs are not touched).
    fn tidy(&mut self) {
        for ap in self.a.iter_mut().chain(self.b.iter_mut()) {
            if ap.yp.abs() < HB_TINY {
                ap.yp = 0.0;
            }
        }
    }
}

/// The edge biquads of a crossover edge at `f` Hz.
fn edge(f: f64, slope: i32, high: bool, lim: f64, angle: f64) -> ([Coefs; 2], usize) {
    let k24 = 1.0 / (0.5f64.sqrt() * 2.0);
    let w = f.min(lim) * angle;
    let (n, alpha) = if slope == 0 {
        (1, w.sin())
    } else {
        (2, w.sin() * k24)
    };
    let c = if high {
        Coefs::highpass(alpha, w)
    } else {
        Coefs::lowpass(alpha, w)
    };
    ([c; 2], n)
}

/// What Lemon Studio keeps in the saved state and plays: FL's 59 parameters and the options.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    params: [i32; FL_PARAMS],
    oversampling: u32,
    fir: bool,
    mid_trigger: bool,
    /// The maps' 0 dB at 1.0 instead of 0.975.
    full_ceiling: bool,
}

/// The Press effect.
pub struct Press {
    rate: u32,
    st: Settings,
    fs: f64,
    /// The oversampling stages the memory was made for.
    max_stages: u32,
    stages: u32,
    bands: [Band; BANDS],
    // The crossover: LOW's low-pass and all-pass, MID's high- and low-pass, HIGH's high-pass.
    low_lp: Chain,
    low_ap: Chain,
    mid_hp: Chain,
    mid_lp: Chain,
    high_hp: Chain,
    low_edge: bool,
    high_edge: bool,
    fir: Fir,
    low_cut: Chain,
    up: [[HalfBand; 2]; MAX_OVERSAMPLING as usize],
    down: [[HalfBand; 2]; MAX_OVERSAMPLING as usize],
    counter: i64,
    asleep: bool,
    jump: bool,
    delay: usize,
    xi: [Vec<f32>; 2],
    sum: [Vec<f32>; 2],
    mid: [Vec<f32>; 2],
    high: [Vec<f32>; 2],
    un: [Vec<f32>; 2],
    tmp: Vec<f32>,
    /// The factory map's table with the 0.975 ceiling and with the full one.
    tables: [Table; 2],
    reduction: f32,
}

impl Press {
    /// Allocates every buffer for the oversampling in `values` (FL's factory default for the
    /// rest): call off the audio thread.
    pub fn new(rate: u32, values: &[f32]) -> Self {
        let v = stored_values(&PARAMS, values);
        let mut params = [0; FL_PARAMS];
        params.copy_from_slice(&v[..FL_PARAMS]);
        let st = Settings {
            params,
            oversampling: (v[OVERSAMPLING].max(0) as u32).min(MAX_OVERSAMPLING),
            fir: v[FIR] != 0,
            mid_trigger: v[MID_TRIGGER] != 0,
            full_ceiling: v[SAFETY] != 0,
        };
        let fs = f64::from(rate.max(1));
        let fmax = 1usize << st.oversampling;
        // The longest look-ahead or window: a time knob's top (1 s), at the band rate for LOW, MID and HIGH.
        let second = round(samples(10000, fs)).max(1) as usize;
        let room = |b: usize| second * if b == MASTER { 1 } else { fmax } + 1;
        let fir_room = ((round(0.004 * fs * fmax as f64) | 1).max(5)) as usize;
        let map = Map::default_map();
        let scratch = || [vec![0.0f32; CHUNK * fmax], vec![0.0f32; CHUNK * fmax]];
        let mut m = Press {
            rate,
            st,
            fs,
            max_stages: st.oversampling,
            stages: 0,
            bands: std::array::from_fn(|b| Band::with_room(room(b))),
            low_lp: Chain::default(),
            low_ap: Chain::default(),
            mid_hp: Chain::default(),
            mid_lp: Chain::default(),
            high_hp: Chain::default(),
            low_edge: true,
            high_edge: true,
            fir: Fir::with_room(fir_room),
            low_cut: Chain::default(),
            up: Default::default(),
            down: Default::default(),
            counter: 0,
            asleep: true,
            jump: true,
            delay: 0,
            xi: scratch(),
            sum: scratch(),
            mid: scratch(),
            high: scratch(),
            un: scratch(),
            tmp: Vec::with_capacity(CHUNK * fmax),
            tables: [Table::new(&map, false), Table::new(&map, true)],
            reduction: 0.0,
        };
        m.setup();
        m.reset();
        m
    }

    /// The delay the output is reported late by, in samples.
    pub fn delay(&self) -> usize {
        self.delay
    }

    fn f(&self) -> usize {
        1 << self.stages
    }

    fn table(&self) -> &Table {
        &self.tables[usize::from(self.st.full_ceiling)]
    }

    /// Everything worked out again from the settings.
    fn setup(&mut self) {
        self.stages = self.st.oversampling.min(self.max_stages);
        self.up = Default::default();
        self.down = Default::default();
        for b in &mut self.bands {
            b.sat_dirty = true;
        }
        self.modes();
        self.crossover();
        self.low_cut_coefs();
        for b in 0..BANDS {
            self.times(b, true);
            self.gains(b);
            self.bands[b].reach_used = self.reach_used(b);
        }
        self.report();
    }

    /// The modes in effect and the edges they leave; returns whether an edge appeared or disappeared.
    fn modes(&mut self) -> bool {
        let p = &self.st.params;
        let mut m: [i32; BANDS] = std::array::from_fn(|b| p[band(b, B_MODE)]);
        if self.st.mid_trigger {
            m[LOW] = MODE_MUTED;
            m[HIGH] = MODE_MUTED;
        }
        for (b, v) in m.into_iter().enumerate() {
            self.bands[b].mode = v;
        }
        let (lo, hi) = (m[LOW] != MODE_OFF, m[HIGH] != MODE_OFF);
        let changed = lo != self.low_edge || hi != self.high_edge;
        self.low_edge = lo;
        self.high_edge = hi;
        changed
    }

    /// The crossover's filters for the edges and frequencies in force (state kept).
    fn crossover(&mut self) {
        let p = self.st.params;
        let rate_b = self.fs * self.f() as f64;
        let lim = f64::from(s(0.4999 * rate_b));
        let (mut flo, mut fhi) = (
            f64::from(crossover_hz(p[P_LOW_FREQ])),
            f64::from(crossover_hz(p[P_HIGH_FREQ])),
        );
        if self.low_edge && self.high_edge && flo > fhi {
            std::mem::swap(&mut flo, &mut fhi);
        }
        let (slo, shi) = (p[P_LOW_SLOPE], p[P_HIGH_SLOPE]);
        let angle = (1.0 / rate_b) * 2.0 * PI;
        let none: &[Coefs] = &[];
        let (c, n) = edge(flo, slo, false, lim, angle);
        self.low_lp.set(if self.low_edge { &c[..n] } else { none });
        let (c, n) = edge(flo, slo, true, lim, angle);
        self.mid_hp.set(if self.low_edge { &c[..n] } else { none });
        let (c, n) = edge(fhi, shi, false, lim, angle);
        self.mid_lp.set(if self.high_edge { &c[..n] } else { none });
        let (c, n) = edge(fhi, shi, true, lim, angle);
        self.high_hp
            .set(if self.high_edge { &c[..n] } else { none });
        let ap = self.high_edge && shi != 0;
        let w = fhi.min(lim) * angle;
        let c = [Coefs::allpass(w.sin(), w)];
        self.low_ap.set(if ap { &c } else { none });
        let fir = self.st.fir;
        self.bands[LOW].sign = if !fir && slo == 0 { -1.0 } else { 1.0 };
        self.bands[HIGH].sign = if !fir && shi == 0 { -1.0 } else { 1.0 };
        self.bands[MID].sign = 1.0;
        self.bands[MASTER].sign = 1.0;
        if fir {
            self.fir
                .set_len(((round(0.004 * rate_b) | 1).max(5)) as usize);
            let len = self.fir.len;
            let ratio = self.f() as f64;
            let half = 0.5 * rate_b;
            let (lo, hi) = (self.low_edge, self.high_edge);
            let fr = &mut self.fir;
            if lo {
                Fir::edge(
                    &mut fr.lo,
                    len,
                    flo,
                    flo.min(half),
                    slo,
                    ratio,
                    false,
                    rate_b,
                );
            }
            if hi {
                Fir::edge(
                    &mut fr.hi,
                    len,
                    fhi,
                    fhi.min(half),
                    shi,
                    ratio,
                    true,
                    rate_b,
                );
            }
            for t in &mut fr.taps {
                t.clear();
                t.resize(len, 0.0);
            }
            for i in 0..len {
                let d = if i == (len - 1) / 2 { 1.0 } else { 0.0 };
                let l = if lo { fr.lo[i] } else { 0.0 };
                let h = if hi { fr.hi[i] } else { 0.0 };
                fr.taps[0][i] = l as f32;
                fr.taps[1][i] = (d - l - h) as f32;
                fr.taps[2][i] = h as f32;
            }
        }
    }

    fn low_cut_coefs(&mut self) {
        let v = self.st.params[P_LOW_CUT];
        if v < 0 {
            self.low_cut.set(&[]);
            return;
        }
        let w = f64::from(low_cut_hz(v)) * 0.8 * 2.0 * PI / self.fs;
        let ds = [0x3E4A_74F9u32, 0x3F1E_8C43, 0x3F2D_45AF, 0x3F8A_285E];
        let c = ds.map(|d| Coefs::highpass(f64::from(f32::from_bits(d)) * w.sin(), w));
        self.low_cut.set(&c);
    }

    /// `reachUsed` for band `b`: the table's reach for a peak detector with mode parameter 0, else 4.
    fn reach_used(&self, b: usize) -> f64 {
        let p = &self.st.params;
        if p[band(b, B_RMS)] == 0 && p[band(b, B_MODE)] == 0 {
            self.table().reach
        } else {
            4.0
        }
    }

    fn gains(&mut self, b: usize) {
        let p = self.st.params;
        let c0 = self.table().c[0];
        let bd = &mut self.bands[b];
        bd.pre = gain(p[band(b, B_PRE)]);
        bd.post = gain(p[band(b, B_POST)]);
        bd.rest = bd.pre * bd.post * c0;
        bd.sat_dirty = true;
    }

    /// Band `b`'s times; a look-ahead whose length changes starts empty, changed stage counts start again, a changed
    /// window starts the detector again (`fresh`: all of them).
    fn times(&mut self, b: usize, fresh: bool) {
        let p = self.st.params;
        let f = if b == MASTER { 1 } else { self.f() };
        let t = |v: i32| samples(v, self.fs) * f as f64;
        let k = |x: usize| p[band(b, x)];
        let modes: [i32; BANDS] = std::array::from_fn(|i| self.bands[i].mode);
        let mode = modes[b];
        let none = b != MASTER && modes[..3].iter().all(|&m| m > 0);
        let look = if mode >= 2 || (b == MASTER && mode >= 1) || none {
            0
        } else {
            let knob = if b == MASTER {
                k(B_ATT)
            } else {
                p[P_LMH_DELAY]
            };
            (round(samples(knob, self.fs)) as usize) * f
        };
        let att_n = k(B_ATT_STAGES).clamp(1, MAX_STAGES as i32) as usize;
        let rel_n = if k(B_REL) == 0 {
            0
        } else {
            k(B_REL_STAGES).clamp(1, MAX_STAGES as i32) as usize
        };
        let a = if k(B_ATT) == 0 {
            look as f64
        } else {
            t(k(B_ATT))
        };
        let attack = a * 0.800_000_011_920_929 / (att_n as f64).sqrt();
        let rel2 = if k(B_REL2) == 0 {
            attack
        } else {
            t(k(B_REL2)) * 2.0 / (att_n as f64).sqrt()
        };
        let w = look
            .max(1)
            .max((round(samples(k(B_WINDOW), self.fs)) as usize) * f);
        let rel = t(k(B_REL)) * 2.0 / (rel_n.max(1) as f64).sqrt();
        let rms = k(B_RMS) != 0;
        let bd = &mut self.bands[b];
        // Never past the room made for them (they fit at the oversampling the effect was made for).
        let look = look.min(bd.line.cap());
        let w = w.clamp(1, bd.det.cap);
        if fresh || look != bd.look || bd.line.len() != look {
            bd.line.set(look);
        }
        bd.look = look;
        if fresh || att_n != bd.att_n {
            bd.att = [1.0; MAX_STAGES];
        }
        if fresh || rel_n != bd.rel_n {
            bd.rel = [0.0; MAX_STAGES];
        }
        (bd.att_n, bd.rel_n) = (att_n, rel_n);
        if fresh || w != bd.det.w || rms != bd.det.rms {
            bd.det.start(w, rms);
        }
        bd.att_rise = coef(rel2);
        bd.att_fall = coef(attack);
        bd.rel_fall = coef(rel);
    }

    fn report(&mut self) {
        let f = self.f();
        let fir = if self.st.fir {
            (self.fir.len.saturating_sub(1)) / 2
        } else {
            0
        };
        let lmh = self.bands[..3].iter().map(|b| b.look).max().unwrap_or(0) + fir;
        self.delay = lmh / f + self.bands[MASTER].look;
    }

    /// The reset: attack stages to their resting gain, release stages to 0; delay lines, filters and half-band stages
    /// cleared; the separations jump to their knobs in the next block. The detectors keep their windows.
    pub fn reset(&mut self) {
        for b in &mut self.bands {
            b.reset();
        }
        for c in [
            &mut self.low_lp,
            &mut self.low_ap,
            &mut self.mid_hp,
            &mut self.mid_lp,
            &mut self.high_hp,
            &mut self.low_cut,
        ] {
            c.clear();
        }
        self.fir.clear();
        for h in self.up.iter_mut().chain(self.down.iter_mut()) {
            *h = [HalfBand::default(); 2];
        }
        self.jump = true;
    }

    /// An FL parameter arriving while the effect runs (automation or the editor).
    fn set_fl_param(&mut self, k: usize, v: i32) {
        let (lo, hi) = range(k);
        self.st.params[k] = v.clamp(lo, hi);
        match k {
            P_LOW_FREQ | P_HIGH_FREQ => self.crossover(),
            P_LOW_SLOPE | P_HIGH_SLOPE => {
                self.setup();
                self.reset();
            }
            P_LMH_DELAY => {
                for b in 0..3 {
                    self.times(b, false);
                }
            }
            P_LMH_MIX => {}
            P_LOW_CUT => self.low_cut_coefs(),
            _ => {
                let b = (k - 7) / 13;
                match (k - 7) % 13 {
                    B_MODE => self.mode_changed(),
                    B_PRE | B_POST => self.gains(b),
                    B_SEP => {}
                    B_THRES | B_CEIL => self.bands[b].sat_dirty = true,
                    B_RMS => {
                        self.bands[b].det.w = 0;
                        self.times(b, false);
                        self.bands[b].reach_used = self.reach_used(b);
                        self.bands[b].sat_dirty = true;
                    }
                    _ => self.times(b, false),
                }
            }
        }
        self.report();
    }

    /// A mode or mid trigger changed: the modes in effect, the times and the crossover again (the crossover's
    /// biquads cleared when an edge appears or disappears); `reachUsed` of the shown band (MASTER).
    fn mode_changed(&mut self) {
        if self.modes() && !self.st.fir {
            for c in [
                &mut self.low_lp,
                &mut self.low_ap,
                &mut self.mid_hp,
                &mut self.mid_lp,
                &mut self.high_hp,
            ] {
                c.clear();
            }
        }
        self.crossover();
        for b in 0..BANDS {
            self.times(b, false);
        }
        let r = self.reach_used(MASTER);
        if r != self.bands[MASTER].reach_used {
            self.bands[MASTER].reach_used = r;
            self.bands[MASTER].sat_dirty = true;
        }
    }

    fn process_planar(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len()).min(CHUNK);
        if n == 0 {
            return;
        }
        // Silence.
        let floor = s(1e-6);
        if left[..n]
            .iter()
            .chain(&right[..n])
            .all(|x| x.abs() <= floor)
        {
            self.counter -= n as i64;
            if self.counter <= -(n as i64) {
                if !self.asleep {
                    self.asleep = true;
                    self.reset();
                }
                self.reduction = 0.0;
                return;
            }
        } else {
            self.counter = self.delay as i64 + round(self.fs * 1.5);
        }
        self.asleep = false;
        // The saturations whose settings changed.
        for b in 0..BANDS {
            if self.bands[b].sat_dirty {
                let p = &self.st.params;
                let bd = &mut self.bands[b];
                bd.sat.set(
                    p[band(b, B_THRES)],
                    p[band(b, B_CEIL)],
                    bd.reach_used,
                    bd.post,
                );
                bd.sat_dirty = false;
            }
        }
        // Low cut, at the project's rate.
        if !self.low_cut.is_empty() {
            self.low_cut.run_all(0, &mut left[..n]);
            self.low_cut.run_all(1, &mut right[..n]);
            self.low_cut.tidy();
        }
        // Up.
        let f = self.f();
        let ni = n * f;
        for (ch, inp) in [&left[..n], &right[..n]].into_iter().enumerate() {
            self.xi[ch][..n].copy_from_slice(inp);
            let mut len = n;
            for st in 0..self.stages as usize {
                self.tmp.clear();
                self.tmp.extend_from_slice(&self.xi[ch][..len]);
                for (i, &x) in self.tmp.iter().enumerate() {
                    let [a, b] = self.up[st][ch].up(x);
                    self.xi[ch][2 * i] = a;
                    self.xi[ch][2 * i + 1] = b;
                }
                self.up[st][ch].tidy();
                len *= 2;
            }
        }
        self.split(ni);
        // Down.
        for ch in 0..2 {
            let mut len = ni;
            for st in (0..self.stages as usize).rev() {
                for j in 0..len / 2 {
                    self.sum[ch][j] =
                        self.down[st][ch].down(self.sum[ch][2 * j], self.sum[ch][2 * j + 1]);
                }
                self.down[st][ch].tidy();
                len /= 2;
            }
        }
        // MASTER.
        let (target, limit) = (
            separation(self.st.params[band(MASTER, B_SEP)]),
            s(44100.0 / self.fs * 0.001),
        );
        let jump = self.jump;
        let table = &self.tables[usize::from(self.st.full_ceiling)];
        let [sl, sr] = &mut self.sum;
        let m = &mut self.bands[MASTER];
        if m.mode == MODE_MUTED {
            left[..n].fill(0.0);
            right[..n].fill(0.0);
        } else {
            m.run(
                &mut sl[..n],
                &mut sr[..n],
                None,
                n,
                target,
                limit,
                jump,
                None,
                table,
            );
            left[..n].copy_from_slice(&sl[..n]);
            right[..n].copy_from_slice(&sr[..n]);
        }
        for c in [
            &mut self.low_lp,
            &mut self.low_ap,
            &mut self.mid_hp,
            &mut self.mid_lp,
            &mut self.high_hp,
        ] {
            c.tidy();
        }
        for b in &mut self.bands {
            b.tidy();
        }
        self.jump = false;
        // The deepest reduction of any band that compressed, against its resting gain.
        self.reduction = self
            .bands
            .iter()
            .filter(|b| b.mode == MODE_ON && b.min_g != f32::MAX && b.rest > 0.0)
            .map(|b| -20.0 * (b.min_g / b.rest).clamp(1e-12, 1.0).log10())
            .filter(|d| d.is_finite())
            .fold(0.0, f32::max);
    }

    /// The split and the three bands' chains at the band rate, leaving their sum in `sum`.
    fn split(&mut self, ni: usize) {
        let rate_b = self.fs * self.f() as f64;
        let limit = s(44100.0 / rate_b * 0.001);
        let jump = self.jump;
        let mix = s(f64::from(self.st.params[P_LMH_MIX] as f32 * s(0.001)));
        let trig = self.st.mid_trigger;
        let fir = self.st.fir;
        let (lo, hi) = (self.low_edge, self.high_edge);
        let modes: [i32; BANDS] = std::array::from_fn(|b| self.bands[b].mode);
        let target = |b: usize| separation(self.st.params[band(b, B_SEP)]);
        let tl = target(LOW);
        let tm = target(MID);
        let th = target(HIGH);
        let table = &self.tables[usize::from(self.st.full_ceiling)];
        let [xl, xr] = &mut self.xi;
        let [sl, sr] = &mut self.sum;
        let [ml, mr] = &mut self.mid;
        let [hl, hr] = &mut self.high;
        let [ul, ur] = &mut self.un;
        let (xl, xr, sl, sr, ml, mr, hl, hr, ul, ur) = (
            &mut xl[..ni],
            &mut xr[..ni],
            &mut sl[..ni],
            &mut sr[..ni],
            &mut ml[..ni],
            &mut mr[..ni],
            &mut hl[..ni],
            &mut hr[..ni],
            &mut ul[..ni],
            &mut ur[..ni],
        );
        if trig {
            ul.copy_from_slice(xl);
            ur.copy_from_slice(xr);
        }
        // The filters.
        if fir {
            let any = lo || hi;
            if lo || hi {
                self.fir.run(0, 0, xl, sl);
                self.fir.run(0, 1, xr, sr);
            }
            if any {
                self.fir.run(1, 0, xl, ml);
                self.fir.run(1, 1, xr, mr);
            } else {
                ml.copy_from_slice(xl);
                mr.copy_from_slice(xr);
            }
            if hi {
                self.fir.run(2, 0, xl, hl);
                self.fir.run(2, 1, xr, hr);
            }
        } else {
            if lo {
                for (ch, (x, o)) in [(&*xl, &mut *sl), (&*xr, &mut *sr)].into_iter().enumerate() {
                    o.copy_from_slice(x);
                    self.low_lp.run_all(ch, o);
                    self.low_ap.run_all(ch, o);
                }
            }
            for (ch, (x, o)) in [(&mut *xl, &mut *ml), (&mut *xr, &mut *mr)]
                .into_iter()
                .enumerate()
            {
                let hp = self.mid_hp.n;
                if trig {
                    o.copy_from_slice(x);
                    self.mid_hp.run_all(ch, o);
                    self.mid_lp.run_all(ch, o);
                } else if hi {
                    // MID's high-pass runs in place on the input, its low-pass into MID's buffer.
                    if hp > 0 {
                        self.mid_hp.run_all(ch, x);
                    }
                    o.copy_from_slice(x);
                    self.mid_lp.run_all(ch, o);
                } else if hp > 0 {
                    // Without a high edge the last high-pass writes into MID's buffer.
                    self.mid_hp.run(0..hp - 1, ch, x);
                    o.copy_from_slice(x);
                    self.mid_hp.run(hp - 1..hp, ch, o);
                } else {
                    o.copy_from_slice(x);
                }
            }
            if hi {
                for (ch, (x, o)) in [(&*xl, &mut *hl), (&*xr, &mut *hr)].into_iter().enumerate() {
                    o.copy_from_slice(x);
                    self.high_hp.run_all(ch, o);
                }
            }
        }
        // The chains and the sum.
        let first_low = lo;
        if first_low {
            if modes[LOW] < 2 {
                self.bands[LOW].run(sl, sr, None, ni, tl, limit, jump, Some(mix), table);
            } else {
                sl.fill(0.0);
                sr.fill(0.0);
            }
        }
        let add = |sl: &mut [f32], sr: &mut [f32], l: &[f32], r: &[f32]| {
            for (a, b) in sl.iter_mut().zip(l) {
                *a += *b;
            }
            for (a, b) in sr.iter_mut().zip(r) {
                *a += *b;
            }
        };
        // MID.
        let mid_runs = modes[MID] < 2;
        if mid_runs {
            if trig {
                self.bands[MID].run(
                    ml,
                    mr,
                    Some((&mut *ul, &mut *ur)),
                    ni,
                    tm,
                    limit,
                    jump,
                    Some(mix),
                    table,
                );
            } else {
                self.bands[MID].run(ml, mr, None, ni, tm, limit, jump, Some(mix), table);
            }
        }
        let (mo_l, mo_r): (&[f32], &[f32]) = if trig { (ul, ur) } else { (ml, mr) };
        if !first_low {
            if mid_runs {
                sl.copy_from_slice(mo_l);
                sr.copy_from_slice(mo_r);
            } else {
                sl.fill(0.0);
                sr.fill(0.0);
            }
        } else if mid_runs {
            add(sl, sr, mo_l, mo_r);
        }
        // HIGH.
        if hi && modes[HIGH] < 2 {
            self.bands[HIGH].run(hl, hr, None, ni, th, limit, jump, Some(mix), table);
            add(sl, sr, hl, hr);
            if !first_low && modes[MID] == MODE_OFF {
                add(sl, sr, hl, hr);
            }
        }
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let mut at = 0;
        while at < n {
            let end = (at + CHUNK).min(n);
            self.process_planar(&mut left[at..end], &mut right[at..end]);
            at = end;
        }
    }

    fn latency_samples(&self) -> usize {
        self.delay
    }
}

block_effect!(Press);

impl NativeFx for Press {
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(p) = PARAMS.get(index) else { return };
        let v = p.stored(value);
        if index < FL_PARAMS {
            if self.st.params[index] != v {
                self.set_fl_param(index, v);
            }
            return;
        }
        let on = v != 0;
        match index {
            OVERSAMPLING => {
                // Only within the memory made when the effect was opened.
                let stages = (v.max(0) as u32).min(self.max_stages);
                if stages != self.st.oversampling {
                    self.st.oversampling = stages;
                    self.setup();
                    self.reset();
                }
            }
            FIR if on != self.st.fir => {
                self.st.fir = on;
                self.setup();
                self.reset();
            }
            MID_TRIGGER if on != self.st.mid_trigger => {
                self.st.mid_trigger = on;
                self.mode_changed();
                self.report();
            }
            SAFETY if on != self.st.full_ceiling => {
                self.st.full_ceiling = on;
                for b in 0..BANDS {
                    self.gains(b);
                    self.bands[b].reach_used = self.reach_used(b);
                }
                self.report();
            }
            _ => {}
        }
    }

    fn param(&self, index: usize) -> f32 {
        match index {
            k if k < FL_PARAMS => self.st.params[k] as f32,
            OVERSAMPLING => self.st.oversampling as f32,
            FIR => f32::from(u8::from(self.st.fir)),
            MID_TRIGGER => f32::from(u8::from(self.st.mid_trigger)),
            SAFETY => f32::from(u8::from(self.st.full_ceiling)),
            _ => 0.0,
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        self.reduction
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_signals::{peak, run_blocks, sine};
    use super::*;
    use crate::plugin_track::BlockProcessor;

    fn defaults() -> Vec<f32> {
        PARAMS.iter().map(|p| p.default).collect()
    }

    /// LOW, MID and HIGH all above mode 0 drop their own look-ahead, not MASTER's.
    #[test]
    fn masters_look_ahead_stays_when_the_bands_do_not_compress() {
        let both = Press::new(44100, &[]).delay();
        let mut off = defaults();
        for b in 0..MASTER {
            off[band(b, B_MODE)] = 1.0;
        }
        let master = Press::new(44100, &off).delay();
        assert!(master > 0 && master < both, "{master} of {both}");
        off[band(MASTER, B_MODE)] = 1.0;
        assert_eq!(Press::new(44100, &off).delay(), 0);
    }

    /// The factory default is a limiter at 0.975: a sine pushed 12 dB over comes out at it.
    #[test]
    fn default_limits_a_hot_signal_at_the_ceiling() {
        let rate = 44100;
        let mut v = defaults();
        v[band(MASTER, B_PRE)] = 1700.0; // about +12 dB
        let mut fx = Press::new(rate, &v);
        assert!(fx.latency_seconds() > 0.0);
        let input = sine(44100, 0.9, 440.0, 44100.0);
        let (l, r) = run_blocks(&mut fx, &input, &input, 512);
        let p = peak(&l[22050..]).max(peak(&r[22050..]));
        assert!(p <= 1.0 && p > 0.8, "{p}");
        assert!(fx.gain_reduction_db() > 6.0, "{}", fx.gain_reduction_db());
    }

    /// Quiet audio passes the default setting at the maps' 0.975 twice over (a band's map,
    /// then MASTER's: FL's "0.2 dB compressor safety"), the bands summing back flat; with the
    /// safety off it passes at unity.
    #[test]
    fn quiet_audio_passes_at_the_safety_gain() {
        let input = sine(48000, 0.1, 1000.0, 48000.0);
        for (safety, want) in [(0.0, 0.1 * 0.975 * 0.975), (1.0, 0.1)] {
            let mut v = defaults();
            v[SAFETY] = safety;
            let mut fx = Press::new(48000, &v);
            let (l, _) = run_blocks(&mut fx, &input, &input, 300);
            let out = peak(&l[24000..]);
            assert!(
                (out / want - 1.0).abs() < 0.01,
                "safety {safety}: {out}, want {want}"
            );
            assert!(fx.gain_reduction_db() < 0.1, "{}", fx.gain_reduction_db());
        }
    }

    /// Every option works inside the memory made for it, oversampling included.
    #[test]
    fn options_and_oversampling_run_without_allocating() {
        let mut v = defaults();
        v[OVERSAMPLING] = 2.0;
        let mut fx = Press::new(44100, &v);
        let input = sine(8192, 0.9, 3000.0, 44100.0);
        for (k, x) in [
            (FIR, 1.0),
            (MID_TRIGGER, 1.0),
            (SAFETY, 1.0),
            (OVERSAMPLING, 1.0),
            (OVERSAMPLING, 4.0),
        ] {
            assert_no_alloc::assert_no_alloc(|| fx.set_param(k, x));
            let (l, _) = run_blocks(&mut fx, &input, &input, 700);
            assert!(l.iter().all(|x| x.is_finite()), "{k}");
        }
        // Raising the oversampling past what it was opened with stays at the most it has room for.
        assert_eq!(fx.param(OVERSAMPLING), 2.0);
        for k in [band(MASTER, B_THRES), band(LOW, B_THRES)] {
            assert_no_alloc::assert_no_alloc(|| fx.set_param(k, -800.0));
        }
        let (l, _) = run_blocks(&mut fx, &input, &input, 512);
        assert!(l.iter().all(|x| x.is_finite()));
    }
}
