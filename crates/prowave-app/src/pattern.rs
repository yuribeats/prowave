//! Pattern regions and Drummer regions (Logic Pro: Step Sequencer pattern regions and Drummer
//! regions). Neither holds notes of its own: each is a few settings that make the notes, so a
//! region is as long as it is dragged and the notes follow. The notes are kept in the region as
//! for any MIDI region (the engine plays them like the rest), and made again whenever the
//! settings or the length of the region change.
//!
//! - A **pattern** is a grid of steps, one row to a pitch, that loops over the region.
//! - A **drummer** plays a drum kit in a style, more or less busy and loud, and fills the end
//!   of every fourth bar now and then. The same settings always give the same notes.
use crate::model::Note;

/// Most notes a pattern makes for one region (a very long region of fast steps stops here).
pub const MAX_PATTERN_NOTES: usize = 250_000;
/// Most steps a pattern can have, and most rows.
pub const MAX_STEPS: usize = 64;
pub const MAX_ROWS: usize = 16;
/// Velocity a new step gets.
pub const DEFAULT_VELOCITY: u8 = 100;

/// General MIDI drum pitches the engine's drum kit plays (see `synth::kit_voice`).
pub mod drums {
    pub const KICK: u8 = 36;
    pub const SNARE: u8 = 38;
    pub const CLAP: u8 = 39;
    pub const ELECTRIC_SNARE: u8 = 40;
    pub const TOM_FLOOR_LOW: u8 = 41;
    pub const CLOSED_HAT: u8 = 42;
    pub const TOM_FLOOR: u8 = 43;
    pub const PEDAL_HAT: u8 = 44;
    pub const TOM_LOW: u8 = 45;
    pub const OPEN_HAT: u8 = 46;
    pub const TOM_MID: u8 = 47;
    pub const TOM_MID_HIGH: u8 = 48;
    pub const CRASH: u8 = 49;
    pub const TOM_HIGH: u8 = 50;
    pub const RIDE: u8 = 51;
    pub const TAMBOURINE: u8 = 54;
    pub const COWBELL: u8 = 56;
}

// ------------------------------------------------------------------------- pattern

/// How long one step of a pattern is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepRate {
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    EighthTriplet,
    SixteenthTriplet,
}

impl StepRate {
    pub const ALL: [StepRate; 6] = [
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
        Self::EighthTriplet,
        Self::SixteenthTriplet,
    ];

    /// The step in quarter notes.
    pub fn beats(self) -> f64 {
        match self {
            Self::Quarter => 1.0,
            Self::Eighth => 0.5,
            Self::Sixteenth => 0.25,
            Self::ThirtySecond => 0.125,
            Self::EighthTriplet => 1.0 / 3.0,
            Self::SixteenthTriplet => 1.0 / 6.0,
        }
    }

    /// How it is written in the project file and shown.
    pub fn name(self) -> &'static str {
        match self {
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
            Self::EighthTriplet => "1/8T",
            Self::SixteenthTriplet => "1/16T",
        }
    }

    pub fn from_name(name: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|r| r.name() == name)
            .unwrap_or(Self::Sixteenth)
    }

    /// Steps in a quarter note, for drawing beat lines (3 and 6 for the triplets).
    pub fn per_beat(self) -> usize {
        (1.0 / self.beats()).round() as usize
    }
}

/// The order the steps of a pattern play in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
    /// Forward, then back (the end steps play once).
    PingPong,
    /// Any step, any time (the same ones every time for the same seed).
    Random,
}

impl Direction {
    pub const ALL: [Direction; 4] = [Self::Forward, Self::Backward, Self::PingPong, Self::Random];

    pub fn name(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Backward => "backward",
            Self::PingPong => "ping_pong",
            Self::Random => "random",
        }
    }

    pub fn from_name(name: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|d| d.name() == name)
            .unwrap_or(Self::Forward)
    }
}

/// Most times a step can repeat within itself.
pub const MAX_REPEAT: u8 = 8;

/// One row of a pattern: a pitch and, for each step, its velocity (0 = the step is off), the
/// chance it plays (percent), whether it is tied to the step before and how many times it
/// repeats. The lists always hold [`MAX_STEPS`] values; only the first `steps` of the pattern
/// play.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternRow {
    pub pitch: u8,
    pub steps: Vec<u8>,
    pub chance: Vec<u8>,
    /// A step that is on and tied carries on the note of the step before it (when that is
    /// on) instead of starting its own.
    pub tie: Vec<bool>,
    pub repeat: Vec<u8>,
}

impl PatternRow {
    pub fn new(pitch: u8) -> Self {
        Self {
            pitch,
            steps: vec![0; MAX_STEPS],
            chance: vec![100; MAX_STEPS],
            tie: vec![false; MAX_STEPS],
            repeat: vec![1; MAX_STEPS],
        }
    }

    /// Puts every list at full length and every value in range.
    fn tidy(&mut self) {
        self.pitch = self.pitch.min(127);
        self.steps.resize(MAX_STEPS, 0);
        self.chance.resize(MAX_STEPS, 100);
        self.tie.resize(MAX_STEPS, false);
        self.repeat.resize(MAX_STEPS, 1);
        for v in &mut self.steps {
            *v = (*v).min(127);
        }
        for v in &mut self.chance {
            *v = (*v).min(100);
        }
        for v in &mut self.repeat {
            *v = (*v).clamp(1, MAX_REPEAT);
        }
    }

    fn with_hits(pitch: u8, hits: impl IntoIterator<Item = usize>, vel: u8) -> Self {
        let mut row = Self::new(pitch);
        for k in hits {
            row.steps[k] = vel;
        }
        row
    }
}

/// The settings of a Pattern region.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternParams {
    /// Steps in one run of the pattern (1-64); it loops over the region.
    pub steps: usize,
    pub rate: StepRate,
    /// How far every second step is pushed back, as a fraction of a step (0 - 0.5).
    pub swing: f32,
    /// How long a note lasts, as a fraction of a step (0.05 - 1).
    pub gate: f32,
    /// From the top of the editor down.
    pub rows: Vec<PatternRow>,
    pub direction: Direction,
    /// Makes the chances and the random order come out the same every time.
    pub seed: u32,
}

impl PatternParams {
    /// A drum pattern: a kick, snare and hi-hat beat on a kit's rows.
    pub fn drums() -> Self {
        use drums::*;
        let rows = vec![
            PatternRow::new(CRASH),
            PatternRow::new(OPEN_HAT),
            PatternRow::with_hits(CLOSED_HAT, (0..16).step_by(2), 80),
            PatternRow::new(TOM_LOW),
            PatternRow::new(CLAP),
            PatternRow::with_hits(SNARE, [4, 12], DEFAULT_VELOCITY),
            PatternRow::with_hits(KICK, [0, 8, 10], 110),
        ];
        Self {
            steps: 16,
            rate: StepRate::Sixteenth,
            swing: 0.0,
            gate: 0.9,
            rows,
            direction: Direction::Forward,
            seed: 1,
        }
    }

    /// An empty pattern for a melodic instrument: eight rows of a C minor pentatonic scale.
    pub fn melodic() -> Self {
        let rows = [72, 70, 67, 65, 63, 60, 58, 55]
            .into_iter()
            .map(PatternRow::new)
            .collect();
        Self {
            steps: 16,
            rate: StepRate::Sixteenth,
            swing: 0.0,
            gate: 0.9,
            rows,
            direction: Direction::Forward,
            seed: 1,
        }
    }

    /// Puts every setting in range and every row at full length.
    pub fn tidy(&mut self) {
        self.steps = self.steps.clamp(1, MAX_STEPS);
        self.swing = if self.swing.is_finite() {
            self.swing.clamp(0.0, 0.5)
        } else {
            0.0
        };
        self.gate = if self.gate.is_finite() {
            self.gate.clamp(0.05, 1.0)
        } else {
            0.9
        };
        self.rows.truncate(MAX_ROWS);
        for row in &mut self.rows {
            row.tidy();
        }
    }

    /// The step that plays as the `k`-th one (counting from the start of the region).
    pub fn index_at(&self, k: usize) -> usize {
        let steps = self.steps.clamp(1, MAX_STEPS);
        match self.direction {
            Direction::Forward => k % steps,
            Direction::Backward => steps - 1 - k % steps,
            Direction::PingPong => {
                let period = (2 * steps).saturating_sub(2).max(1);
                let m = k % period;
                if m < steps {
                    m
                } else {
                    period - m
                }
            }
            Direction::Random => {
                (Rng::new(self.seed ^ (k as u32).wrapping_mul(0x9e37_79b1)).next() as usize) % steps
            }
        }
    }

    /// The notes of the pattern looped over `len_beats` quarter notes.
    pub fn notes(&self, len_beats: f64) -> Vec<Note> {
        let mut out = Vec::new();
        let step_len = self.rate.beats();
        let swing = self.swing.clamp(0.0, 0.5) as f64;
        let gate = (self.gate.clamp(0.05, 1.0) as f64 * step_len).max(1.0 / 64.0);
        let mut k = 0usize;
        loop {
            let delay = if k % 2 == 1 { swing * step_len } else { 0.0 };
            let start = k as f64 * step_len + delay;
            if start >= len_beats - 1e-9 || out.len() >= MAX_PATTERN_NOTES {
                // Later steps start later still, except that the swing of an odd step never
                // reaches the next even one.
                break;
            }
            let at = self.index_at(k);
            let before = (k > 0).then(|| self.index_at(k - 1));
            for (ri, row) in self.rows.iter().enumerate() {
                let on = |i: usize| row.steps.get(i).copied().unwrap_or(0) > 0;
                let vel = row.steps.get(at).copied().unwrap_or(0);
                if vel == 0 {
                    continue;
                }
                let tied = |i: usize| row.tie.get(i).copied().unwrap_or(false);
                // A tied step carries on the note before it.
                if tied(at) && before.is_some_and(on) {
                    continue;
                }
                let seed = self.seed
                    ^ (k as u32).wrapping_mul(0x85eb_ca6b)
                    ^ (ri as u32 + 1).wrapping_mul(0xc2b2_ae35);
                let chance = row.chance.get(at).copied().unwrap_or(100).min(100) as f32 / 100.0;
                if chance < 1.0 && Rng::new(seed).unit() >= chance {
                    continue;
                }
                // The steps after it that are tied on make the note longer.
                let mut held = 1usize;
                while held < 2 * MAX_STEPS {
                    let next = self.index_at(k + held);
                    if on(next) && tied(next) && start + (held as f64) * step_len < len_beats {
                        held += 1;
                    } else {
                        break;
                    }
                }
                let repeats = if held > 1 {
                    1
                } else {
                    row.repeat
                        .get(at)
                        .copied()
                        .unwrap_or(1)
                        .clamp(1, MAX_REPEAT) as usize
                };
                let each = step_len / repeats as f64;
                for r in 0..repeats {
                    let at_beat = start + r as f64 * each;
                    if at_beat >= len_beats - 1e-9 {
                        break;
                    }
                    let len = if held > 1 {
                        (held as f64 - 1.0) * step_len + gate
                    } else {
                        (gate / repeats as f64).max(1.0 / 64.0)
                    };
                    let fade = 1.0 - 0.08 * r as f32;
                    out.push(Note {
                        lyric: crate::lyrics::Lyric::NONE,
                        mute: false,
                        start: at_beat as f32,
                        len: len.min(len_beats - at_beat) as f32,
                        pitch: row.pitch,
                        vel: ((vel as f32 * fade).round() as u8).clamp(1, 127),
                    });
                }
            }
            k += 1;
        }
        out.sort_by(|a, b| a.start.total_cmp(&b.start));
        out
    }

    /// The step playing at `beat` (quarter notes from the region's start), if the pattern has
    /// any steps.
    pub fn step_at(&self, beat: f64) -> usize {
        self.index_at((beat.max(0.0) / self.rate.beats()) as usize)
    }
}

// ------------------------------------------------------------------------- drummer

/// The styles a Drummer region plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrumStyle {
    Rock,
    Pop,
    Funk,
    Jazz,
    HipHop,
    Electronic,
}

impl DrumStyle {
    pub const ALL: [DrumStyle; 6] = [
        Self::Rock,
        Self::Pop,
        Self::Funk,
        Self::Jazz,
        Self::HipHop,
        Self::Electronic,
    ];

    /// How it is written in the project file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rock => "rock",
            Self::Pop => "pop",
            Self::Funk => "funk",
            Self::Jazz => "jazz",
            Self::HipHop => "hip_hop",
            Self::Electronic => "electronic",
        }
    }

    pub fn from_name(name: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|s| s.name() == name)
            .unwrap_or(Self::Rock)
    }

    /// The swing a new region of this style starts with (percent).
    pub fn default_swing(self) -> u8 {
        match self {
            Self::Jazz => 70,
            Self::HipHop => 25,
            _ => 0,
        }
    }
}

/// The kit pieces a Drummer plays; a region plays only the ones that are on.
pub mod parts {
    pub const KICK: u8 = 1;
    pub const SNARE: u8 = 2;
    pub const HIHAT: u8 = 4;
    pub const TOMS: u8 = 8;
    pub const CYMBALS: u8 = 16;
    pub const PERCUSSION: u8 = 32;
    pub const ALL: u8 = 63;
    /// Every piece, in the order the editor lists them.
    pub const LIST: [u8; 6] = [KICK, SNARE, HIHAT, TOMS, CYMBALS, PERCUSSION];
}

/// The kit piece a drum pitch belongs to.
pub fn part_of(pitch: u8) -> u8 {
    use drums::*;
    match pitch {
        0..=37 => parts::KICK,
        SNARE | ELECTRIC_SNARE => parts::SNARE,
        CLAP | TAMBOURINE | COWBELL => parts::PERCUSSION,
        CLOSED_HAT | PEDAL_HAT | OPEN_HAT => parts::HIHAT,
        CRASH | RIDE | 52..=53 | 55 | 57..=59 => parts::CYMBALS,
        _ => parts::TOMS,
    }
}

/// The settings of a Drummer region.
#[derive(Clone, Debug, PartialEq)]
pub struct DrummerParams {
    pub style: DrumStyle,
    /// 0 (simple) - 100 (busy): how many hits the hi-hat, kick and snare add.
    pub complexity: u8,
    /// 0 (soft) - 100 (loud): how hard the drummer plays.
    pub loudness: u8,
    /// 0 - 100: how far the off-beats are pushed back.
    pub swing: u8,
    /// 0 - 100: how likely the last bar of every four ends in a fill.
    pub fills: u8,
    /// The kit pieces that play ([`parts`]).
    pub parts: u8,
    /// Another number plays another variation of the same settings.
    pub seed: u32,
}

impl DrummerParams {
    pub fn new(style: DrumStyle) -> Self {
        Self {
            style,
            complexity: 50,
            loudness: 60,
            swing: style.default_swing(),
            fills: 30,
            parts: parts::ALL,
            seed: 1,
        }
    }

    pub fn tidy(&mut self) {
        self.complexity = self.complexity.min(100);
        self.loudness = self.loudness.min(100);
        self.swing = self.swing.min(100);
        self.fills = self.fills.min(100);
        self.parts &= parts::ALL;
    }
}

/// One bar of the region: its length in quarter notes and its signature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarSpec {
    pub beats: f64,
    pub num: u8,
    pub den: u8,
}

/// A small random number generator that gives the same numbers for the same seed everywhere.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        // Close seeds must give unrelated numbers: mix the bits first.
        let mut s = seed;
        s ^= s >> 16;
        s = s.wrapping_mul(0x85eb_ca6b);
        s ^= s >> 13;
        s = s.wrapping_mul(0xc2b2_ae35);
        s ^= s >> 16;
        let mut r = Self(if s == 0 { 0x9e37_79b9 } else { s });
        r.next();
        r
    }

    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// A number from 0 up to (not including) 1.
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / 16_777_216.0
    }

    fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
}

/// A hit before it is turned into a note: sixteenth steps into its bar.
struct Hit {
    step: usize,
    pitch: u8,
    vel: f32,
    len: f32,
}

/// The notes of a Drummer region over `bars`, in quarter notes from its start.
pub fn drummer_notes(p: &DrummerParams, bars: &[BarSpec]) -> Vec<Note> {
    let mut out = Vec::new();
    let mut start = 0.0f64;
    for (index, bar) in bars.iter().enumerate() {
        let n16 = (bar.beats * 4.0).round() as usize;
        if n16 > 0 {
            let mut gen = BarGen::new(p, index, bar, n16);
            let hits = gen.play();
            let swing = if gen.compound {
                0.0
            } else {
                p.swing.min(100) as f64 / 100.0
            };
            for h in hits {
                let delay = match h.step % 4 {
                    2 => swing / 6.0,
                    1 | 3 => swing / 12.0,
                    _ => 0.0,
                };
                let at = start + h.step as f64 * 0.25 + delay;
                let end = (start + bar.beats).max(at);
                out.push(Note {
                    lyric: crate::lyrics::Lyric::NONE,
                    mute: false,
                    start: at as f32,
                    len: (h.len as f64).min(end - at).max(0.02) as f32,
                    pitch: h.pitch,
                    vel: h.vel.round().clamp(1.0, 127.0) as u8,
                });
            }
        }
        start += bar.beats;
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    out
}

/// The hits of one bar.
struct BarGen<'a> {
    p: &'a DrummerParams,
    index: usize,
    rng: Rng,
    n16: usize,
    /// Groups of three eighths (6/8, 9/8, 12/8) rather than beats of four sixteenths.
    compound: bool,
    complexity: f32,
    loudness: f32,
    /// The velocity of an ordinary hit.
    vel: f32,
    hits: Vec<Hit>,
}

impl<'a> BarGen<'a> {
    fn new(p: &'a DrummerParams, index: usize, bar: &BarSpec, n16: usize) -> Self {
        let seed = p.seed
            ^ (index as u32 + 1).wrapping_mul(0x9e37_79b1)
            ^ (p.style as u32 + 1).wrapping_mul(0x85eb_ca6b);
        let loudness = p.loudness.min(100) as f32 / 100.0;
        Self {
            p,
            index,
            rng: Rng::new(seed),
            n16,
            compound: bar.den == 8 && bar.num >= 6 && bar.num.is_multiple_of(3),
            complexity: p.complexity.min(100) as f32 / 100.0,
            loudness,
            vel: 30.0 + loudness * 90.0,
            hits: Vec::new(),
        }
    }

    /// Adds a hit at `step` sixteenths into the bar, if the kit piece is on.
    fn hit(&mut self, step: usize, pitch: u8, vel: f32, len: f32) {
        // The draw is made whether the piece is on or not, so turning one off leaves the
        // rest of the performance as it was.
        let spread = 2.0 + 6.0 * self.complexity;
        let jitter = (self.rng.unit() * 2.0 - 1.0) * spread;
        if step >= self.n16 || self.p.parts & part_of(pitch) == 0 {
            return;
        }
        self.hits.push(Hit {
            step,
            pitch,
            vel: (vel + jitter).clamp(1.0, 127.0),
            len,
        });
    }

    /// The steps the kick and the snare play on in this bar.
    fn backbeat(&self) -> (Vec<usize>, Vec<usize>) {
        let n = self.n16;
        if self.compound {
            // A kick, then a snare, a group of three eighths each.
            let groups = (n / 6).max(1);
            let kicks = (0..groups).filter(|g| g % 2 == 0).map(|g| g * 6).collect();
            let snares = (0..groups).filter(|g| g % 2 == 1).map(|g| g * 6).collect();
            return (kicks, snares);
        }
        match n {
            0..=7 => (vec![0], vec![]),
            8 => (vec![0], vec![4]),
            12 => (vec![0], vec![4, 8]),
            16 => (vec![0, 8], vec![4, 12]),
            _ if n.is_multiple_of(4) => {
                // Kick on the odd-numbered beats, snare on the even ones.
                let beats = n / 4;
                (
                    (0..beats).filter(|b| b % 2 == 0).map(|b| b * 4).collect(),
                    (0..beats).filter(|b| b % 2 == 1).map(|b| b * 4).collect(),
                )
            }
            // 5/8, 7/8: a kick on the first eighth, a snare about halfway.
            _ => (vec![0], vec![(n / 2) & !1]),
        }
    }

    fn play(&mut self) -> Vec<Hit> {
        use DrumStyle::*;
        match self.p.style {
            Rock => self.rock(false),
            Pop => self.rock(true),
            Funk => self.funk(),
            Jazz => self.jazz(),
            HipHop => self.hip_hop(),
            Electronic => self.electronic(),
        }
        self.cymbal_and_fill();
        std::mem::take(&mut self.hits)
    }

    /// The chance of an added hit at `step`: off-beats easily, the other sixteenths less.
    fn extra_chance(&self, step: usize) -> f32 {
        if step % 4 == 2 {
            ((self.complexity - 0.15).max(0.0) * 0.9).min(0.8)
        } else {
            ((self.complexity - 0.5).max(0.0) * 0.6).min(0.3)
        }
    }

    fn rock(&mut self, pop: bool) {
        use drums::*;
        let (c, l, v, n) = (self.complexity, self.loudness, self.vel, self.n16);
        let (kicks, snares) = self.backbeat();
        // Hi-hat: quarters when simple, eighths, then sixteenths when busy; the ride at full
        // volume; the last eighth of the bar open when it is busy enough.
        let unit = if self.compound {
            2
        } else if c < 0.2 {
            4
        } else if c < 0.62 {
            2
        } else {
            1
        };
        let beat = if self.compound { 6 } else { 4 };
        for step in (0..n).step_by(unit) {
            let strong = step % beat == 0;
            let accent = if strong { 1.0 } else { 0.72 };
            let sixteenth = step % 2 == 1;
            let (pitch, len) = if l >= 0.8 && !self.compound {
                (RIDE, 0.5)
            } else if c >= 0.4 && step + 2 == n && !self.compound {
                (OPEN_HAT, 0.4)
            } else {
                (CLOSED_HAT, 0.15)
            };
            let vel = v * 0.72 * accent * if sixteenth { 0.7 } else { 1.0 };
            self.hit(step, pitch, vel, len);
        }
        let kick_scale = if pop { 0.5 } else { 1.0 };
        for &k in &kicks {
            self.hit(k, KICK, v + 4.0, 0.25);
        }
        if !self.compound {
            for step in [6, 10, 14, 3, 7, 11, 13] {
                let free = !kicks.contains(&step) && !snares.contains(&step);
                let chance = self.extra_chance(step) * kick_scale;
                if free && self.rng.chance(chance) {
                    self.hit(step, KICK, v * 0.92, 0.25);
                }
            }
        }
        let snare = if pop && l < 0.3 {
            ELECTRIC_SNARE
        } else {
            SNARE
        };
        for &s in &snares {
            self.hit(s, snare, v * 1.08 + 4.0, 0.25);
            if pop && l >= 0.6 {
                self.hit(s, CLAP, v * 0.6, 0.2);
            }
        }
        if c >= 0.5 && !self.compound {
            for step in [3, 7, 11, 15] {
                if self.rng.chance((c - 0.4) * 0.6) {
                    self.hit(step, SNARE, v * 0.28, 0.12);
                }
            }
        }
    }

    fn funk(&mut self) {
        use drums::*;
        let (c, v, n) = (self.complexity, self.vel, self.n16);
        let (kicks, snares) = self.backbeat();
        let unit = if c < 0.35 || self.compound { 2 } else { 1 };
        let accent = |step: usize| match step % 4 {
            0 => 1.0,
            2 => 0.75,
            _ => 0.45,
        };
        for step in (0..n).step_by(unit) {
            let open = step + 2 == n && self.rng.chance(0.5);
            let (pitch, len) = if open {
                (OPEN_HAT, 0.4)
            } else {
                (CLOSED_HAT, 0.12)
            };
            self.hit(step, pitch, v * 0.7 * accent(step), len);
        }
        for &k in &kicks {
            self.hit(k, KICK, v + 4.0, 0.25);
        }
        if !self.compound {
            for step in [3, 6, 10, 13, 8] {
                let free = !kicks.contains(&step) && !snares.contains(&step);
                if free && self.rng.chance(0.2 + c * 0.4) {
                    self.hit(step, KICK, v * 0.9, 0.25);
                }
            }
        }
        for &s in &snares {
            self.hit(s, SNARE, v * 1.1 + 4.0, 0.25);
        }
        for step in [1, 7, 9, 11, 15] {
            if !snares.contains(&step) && self.rng.chance(0.3 + c * 0.45) {
                self.hit(step, SNARE, v * 0.27, 0.1);
            }
        }
    }

    fn jazz(&mut self) {
        use drums::*;
        let (c, v, n) = (self.complexity, self.vel, self.n16);
        let beat = if self.compound { 6 } else { 4 };
        let beats = (n / beat).max(1);
        for b in 0..beats {
            let step = b * beat;
            // The ride keeps time on every beat and skips on the off-beat of the back beats.
            self.hit(step, RIDE, v * if b % 2 == 0 { 0.74 } else { 0.66 }, 0.5);
            if b % 2 == 1 {
                self.hit(step + beat / 2 + beat % 2, RIDE, v * 0.5, 0.4);
                self.hit(step, PEDAL_HAT, v * 0.55, 0.2);
            }
            if c >= 0.7 && b % 2 == 0 {
                self.hit(step + beat / 2 + beat % 2, RIDE, v * 0.42, 0.4);
            }
        }
        // The bass drum feathers the beat softly, then only now and then as it gets busier.
        if c < 0.5 {
            for b in 0..beats {
                self.hit(b * beat, KICK, v * 0.28, 0.2);
            }
        } else {
            self.hit(0, KICK, v * 0.45, 0.2);
            if n > 8 {
                self.hit(8, KICK, v * 0.4, 0.2);
            }
            for step in [6, 10, 14] {
                if self.rng.chance(c * 0.3) {
                    self.hit(step, KICK, v * 0.62, 0.2);
                }
            }
        }
        // Comping on the snare, in places the ride does not play.
        for step in [2, 6, 7, 10, 11, 14] {
            if self.rng.chance(c * 0.3) {
                self.hit(step, SNARE, v * 0.5, 0.15);
            }
        }
    }

    fn hip_hop(&mut self) {
        use drums::*;
        let (c, v, n) = (self.complexity, self.vel, self.n16);
        let (mut kicks, snares) = self.backbeat();
        if !self.compound && n >= 12 {
            kicks.push(10);
        }
        if !self.compound && c >= 0.4 && self.rng.chance(0.6) {
            kicks.push(7);
        }
        if !self.compound && c >= 0.7 && self.rng.chance(0.5) {
            kicks.push(13);
        }
        for &k in &kicks {
            self.hit(k, KICK, v + 6.0, 0.3);
        }
        for &s in &snares {
            self.hit(s, SNARE, v * 1.1 + 6.0, 0.25);
            self.hit(s, CLAP, v * 0.5, 0.2);
        }
        let unit = if c < 0.6 { 2 } else { 1 };
        for step in (0..n).step_by(unit) {
            if unit == 1 && step % 2 == 1 && self.rng.chance(0.4) {
                continue;
            }
            let vel = v * 0.62 * if step % 4 == 0 { 1.0 } else { 0.72 };
            self.hit(step, CLOSED_HAT, vel, 0.15);
        }
        if n >= 16 && self.rng.chance(0.5) {
            self.hit(14, OPEN_HAT, v * 0.6, 0.4);
        }
    }

    fn electronic(&mut self) {
        use drums::*;
        let (c, v, n) = (self.complexity, self.vel, self.n16);
        let (_, snares) = self.backbeat();
        let beat = if self.compound { 6 } else { 4 };
        // Four on the floor.
        for step in (0..n).step_by(beat) {
            self.hit(step, KICK, v + 6.0, 0.3);
        }
        for &s in &snares {
            self.hit(s, CLAP, v * 1.05, 0.2);
            self.hit(s, SNARE, v * 0.45, 0.15);
        }
        // Closed hats on the off-beat, the last one open; sixteenths in between when busy.
        for step in (0..n).filter(|s| s % beat == beat / 2 + beat % 2) {
            let open = c >= 0.3 && step + 2 == n;
            let (pitch, len) = if open {
                (OPEN_HAT, 0.4)
            } else {
                (CLOSED_HAT, 0.12)
            };
            self.hit(step, pitch, v * 0.68, len);
        }
        if c >= 0.6 {
            for step in (1..n).step_by(2) {
                if self.rng.chance(0.7) {
                    self.hit(step, CLOSED_HAT, v * 0.3, 0.08);
                }
            }
        }
        if c >= 0.5 {
            for step in (3..n).step_by(4) {
                if self.rng.chance((c - 0.4) * 0.8) {
                    self.hit(step, TAMBOURINE, v * 0.35, 0.1);
                }
            }
            for step in [5, 11] {
                if self.rng.chance(0.12) {
                    self.hit(step, COWBELL, v * 0.4, 0.15);
                }
            }
        }
    }

    /// A crash on the first bar of every four (when it is loud and busy enough), and now and
    /// then a fill in the last beat or two of the fourth bar.
    fn cymbal_and_fill(&mut self) {
        use drums::*;
        let (c, l, v, n) = (self.complexity, self.loudness, self.vel, self.n16);
        let jazz = self.p.style == DrumStyle::Jazz;
        if self.index.is_multiple_of(4) && l >= 0.45 && c >= 0.2 && !jazz {
            self.hit(0, CRASH, v * 1.05 + 6.0, 1.0);
        }
        let fills = self.p.fills.min(100) as f32 / 100.0;
        if self.index % 4 != 3 || n < 8 || !self.rng.chance(fills) {
            return;
        }
        let len = if c < 0.5 { 4 } else { 8 }.min(n / 2);
        let from = n - len;
        self.hits.retain(|h| h.step < from || h.pitch == CRASH);
        // A kick opens it, then sixteenths down the toms; without toms, all on the snare.
        self.hit(from, KICK, v, 0.25);
        let run = [SNARE, TOM_HIGH, TOM_MID_HIGH, TOM_MID, TOM_LOW, TOM_FLOOR];
        for k in 0..len {
            let pitch = run[k * run.len() / len];
            let pitch = if self.p.parts & parts::TOMS == 0 {
                SNARE
            } else {
                pitch
            };
            let swell = 0.7 + 0.35 * k as f32 / (len - 1).max(1) as f32;
            self.hit(from + k, pitch, v * swell, 0.2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn four_four(bars: usize) -> Vec<BarSpec> {
        vec![
            BarSpec {
                beats: 4.0,
                num: 4,
                den: 4
            };
            bars
        ]
    }

    #[test]
    fn a_pattern_loops_over_the_region_and_stops_at_its_end() {
        let mut p = PatternParams::drums();
        p.steps = 16;
        let notes = p.notes(8.0);
        // Kick on steps 0, 8 and 10 of each of the two bars.
        let kicks: Vec<f32> = notes
            .iter()
            .filter(|n| n.pitch == drums::KICK)
            .map(|n| n.start)
            .collect();
        assert_eq!(kicks, [0.0, 2.0, 2.5, 4.0, 6.0, 6.5]);
        assert!(notes.iter().all(|n| n.start >= 0.0 && n.start < 8.0));
        // A shorter region cuts the pattern off, and the last note is shortened to fit.
        let cut = p.notes(2.1);
        assert!(cut.iter().all(|n| n.start < 2.1));
        assert!(cut.iter().all(|n| n.start + n.len <= 2.1 + 1e-6));
        assert!(p.notes(0.0).is_empty());
    }

    #[test]
    fn steps_rate_gate_and_swing_shape_the_notes() {
        let mut p = PatternParams::melodic();
        p.rows[0].steps[0] = 90;
        p.rows[0].steps[1] = 70;
        p.steps = 3;
        p.rate = StepRate::Eighth;
        p.gate = 0.5;
        let notes = p.notes(3.0);
        // Three steps of an eighth, then the pattern starts again: steps 0 and 1 are on.
        let starts: Vec<f32> = notes.iter().map(|n| n.start).collect();
        assert_eq!(starts, [0.0, 0.5, 1.5, 2.0]);
        assert!(notes.iter().all(|n| (n.len - 0.25).abs() < 1e-6));
        assert_eq!(notes[1].vel, 70);
        // Swing pushes every second step back by a part of a step.
        p.swing = 0.5;
        let swung = p.notes(3.0);
        assert_eq!(swung[1].start, 0.5 + 0.25);
        assert_eq!(swung[0].start, 0.0);
        // Triplets: three steps to a quarter note.
        p.rate = StepRate::EighthTriplet;
        p.swing = 0.0;
        p.steps = 3;
        let t = p.notes(1.0);
        assert_eq!(t.len(), 2);
        assert!((t[1].start - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(StepRate::EighthTriplet.per_beat(), 3);
        assert_eq!(StepRate::Sixteenth.per_beat(), 4);
    }

    /// A pattern with one row of the given velocities (0 = off), 1/4 notes, gate 1.
    fn line(steps: &[u8]) -> PatternParams {
        let mut p = PatternParams::melodic();
        p.rows.truncate(1);
        p.rows[0].steps[..steps.len()].copy_from_slice(steps);
        p.steps = steps.len();
        p.rate = StepRate::Quarter;
        p.gate = 1.0;
        p
    }

    #[test]
    fn tied_steps_make_one_longer_note() {
        let mut p = line(&[100, 100, 100, 0]);
        p.rows[0].tie[1] = true;
        p.rows[0].tie[2] = true;
        let notes = p.notes(4.0);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!((notes[0].start, notes[0].len), (0.0, 3.0));
        // A tie after a step that is off starts a note of its own, and a tie on a step that
        // is off does nothing.
        let mut q = line(&[0, 100, 0, 100]);
        q.rows[0].tie[1] = true;
        q.rows[0].tie[2] = true;
        assert_eq!(q.notes(4.0).len(), 2);
        // The note is cut off at the end of the region, and ties carry over the loop.
        let mut r = line(&[100, 100]);
        r.rows[0].tie[0] = true;
        r.rows[0].tie[1] = true;
        let notes = r.notes(5.0);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].len, 5.0);
    }

    #[test]
    fn a_repeating_step_plays_several_quieter_notes_in_its_time() {
        let mut p = line(&[100, 0]);
        p.rows[0].repeat[0] = 4;
        let notes = p.notes(2.0);
        let starts: Vec<f32> = notes.iter().map(|n| n.start).collect();
        assert_eq!(starts, [0.0, 0.25, 0.5, 0.75]);
        assert!(notes.windows(2).all(|w| w[1].vel < w[0].vel));
        assert!(notes.iter().all(|n| n.len <= 0.25));
        // Tied, a step does not repeat.
        let mut q = line(&[100, 100]);
        q.rows[0].repeat[0] = 4;
        q.rows[0].tie[1] = true;
        assert_eq!(q.notes(2.0).len(), 1);
    }

    #[test]
    fn chance_plays_a_step_some_of_the_time_and_the_same_way_every_time() {
        let mut p = line(&[100]);
        p.steps = 1;
        p.rows[0].chance[0] = 50;
        let a = p.notes(400.0);
        assert!(a.len() > 150 && a.len() < 250, "{} of 400", a.len());
        assert_eq!(a, p.notes(400.0));
        p.seed = 7;
        assert_ne!(a, p.notes(400.0), "another seed, other steps");
        p.rows[0].chance[0] = 0;
        assert!(p.notes(40.0).is_empty());
        p.rows[0].chance[0] = 100;
        assert_eq!(p.notes(40.0).len(), 40);
    }

    #[test]
    fn the_direction_sets_the_order_of_the_steps() {
        let mut p = line(&[10, 20, 30, 40]);
        let order = |p: &PatternParams| (0..10).map(|k| p.index_at(k)).collect::<Vec<_>>();
        assert_eq!(order(&p), [0, 1, 2, 3, 0, 1, 2, 3, 0, 1]);
        p.direction = Direction::Backward;
        assert_eq!(order(&p), [3, 2, 1, 0, 3, 2, 1, 0, 3, 2]);
        p.direction = Direction::PingPong;
        assert_eq!(order(&p), [0, 1, 2, 3, 2, 1, 0, 1, 2, 3]);
        p.direction = Direction::Random;
        let r = order(&p);
        assert_eq!(r, order(&p));
        assert!(r.iter().all(|i| *i < 4) && r.iter().any(|i| *i != r[0]));
        // The notes follow it: backward starts on the last step.
        p.direction = Direction::Backward;
        assert_eq!(p.notes(1.0)[0].vel, 40);
        // One step, ping-pong: no division by nothing.
        let mut one = line(&[100]);
        one.direction = Direction::PingPong;
        assert_eq!(one.notes(3.0).len(), 3);
        for d in Direction::ALL {
            assert_eq!(Direction::from_name(d.name()), d);
        }
    }

    #[test]
    fn a_pattern_repairs_what_a_file_may_hold() {
        let mut p = PatternParams::drums();
        p.steps = 500;
        p.swing = f32::NAN;
        p.gate = 9.0;
        p.rows.extend((0..30).map(PatternRow::new));
        p.rows[0].steps = vec![200; 3];
        p.rows[0].chance = vec![250; 3];
        p.rows[0].repeat = vec![99, 0];
        p.rows[0].pitch = 250;
        p.tidy();
        assert_eq!((p.steps, p.swing, p.gate), (MAX_STEPS, 0.0, 1.0));
        assert_eq!(p.rows.len(), MAX_ROWS);
        assert!(p.rows.iter().all(|r| r.steps.len() == MAX_STEPS));
        assert_eq!(p.rows[0].pitch, 127);
        assert_eq!(p.rows[0].steps[0], 127);
        assert_eq!(p.step_at(0.0), 0);
        assert_eq!(p.step_at(1.0), 4 % MAX_STEPS);
        for rate in StepRate::ALL {
            assert_eq!(StepRate::from_name(rate.name()), rate);
        }
    }

    #[test]
    fn the_drummer_is_the_same_every_time_and_a_new_seed_is_another_take() {
        let bars = four_four(8);
        let p = DrummerParams::new(DrumStyle::Rock);
        let a = drummer_notes(&p, &bars);
        assert_eq!(a, drummer_notes(&p, &bars));
        let other = DrummerParams {
            seed: 2,
            ..p.clone()
        };
        assert_ne!(a, drummer_notes(&other, &bars));
        // The first bars do not change when the region grows.
        let longer = drummer_notes(&p, &four_four(12));
        let first: Vec<&Note> = a.iter().filter(|n| n.start < 12.0).collect();
        let again: Vec<&Note> = longer.iter().filter(|n| n.start < 12.0).collect();
        assert_eq!(first, again);
    }

    #[test]
    fn every_style_keeps_time_in_four_four() {
        for style in DrumStyle::ALL {
            let mut p = DrummerParams::new(style);
            p.fills = 0;
            let notes = drummer_notes(&p, &four_four(4));
            assert!(!notes.is_empty(), "{style:?}");
            assert!(notes.iter().all(|n| n.start >= 0.0 && n.start < 16.0));
            assert!(notes.iter().all(|n| n.len > 0.0 && n.vel >= 1));
            if style != DrumStyle::Jazz {
                // A kick on the first beat of every bar.
                for bar in 0..4 {
                    let at = bar as f32 * 4.0;
                    assert!(
                        notes
                            .iter()
                            .any(|n| n.pitch == drums::KICK && (n.start - at).abs() < 0.2),
                        "{style:?} bar {bar}"
                    );
                }
            }
            if matches!(style, DrumStyle::Rock | DrumStyle::Pop | DrumStyle::Funk) {
                // And the snare on beats 2 and 4.
                for bar in 0..4 {
                    for beat in [1.0f32, 3.0] {
                        let at = bar as f32 * 4.0 + beat;
                        assert!(
                            notes.iter().any(|n| matches!(n.pitch, 38 | 40)
                                && (n.start - at).abs() < 0.2
                                && n.vel > 50),
                            "{style:?} bar {bar} beat {beat}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn complexity_adds_hits_and_loudness_adds_velocity() {
        let bars = four_four(4);
        let make = |c: u8, l: u8| {
            let mut p = DrummerParams::new(DrumStyle::Rock);
            p.fills = 0;
            p.complexity = c;
            p.loudness = l;
            drummer_notes(&p, &bars)
        };
        let hats = |notes: &[Note]| {
            notes
                .iter()
                .filter(|n| part_of(n.pitch) == parts::HIHAT || n.pitch == drums::RIDE)
                .count()
        };
        assert!(hats(&make(100, 50)) > hats(&make(50, 50)));
        assert!(hats(&make(50, 50)) > hats(&make(0, 50)));
        assert!(make(100, 50).len() > make(0, 50).len());
        let mean =
            |notes: &[Note]| notes.iter().map(|n| n.vel as f32).sum::<f32>() / notes.len() as f32;
        assert!(mean(&make(50, 100)) > mean(&make(50, 50)) + 15.0);
        assert!(mean(&make(50, 50)) > mean(&make(50, 0)) + 15.0);
    }

    #[test]
    fn kit_pieces_can_be_turned_off() {
        let bars = four_four(4);
        let mut p = DrummerParams::new(DrumStyle::Funk);
        p.complexity = 100;
        p.fills = 100;
        let all = drummer_notes(&p, &bars);
        for piece in parts::LIST {
            p.parts = parts::ALL & !piece;
            let some = drummer_notes(&p, &bars);
            assert!(some.iter().all(|n| part_of(n.pitch) != piece), "{piece}");
            // Taking a piece out leaves the rest as it was (the fill's toms become snare hits).
            assert!(some.len() <= all.len(), "{piece}");
            let kept: Vec<&Note> = all.iter().filter(|n| part_of(n.pitch) != piece).collect();
            if piece != parts::TOMS {
                let rest: Vec<&Note> = some.iter().collect();
                assert_eq!(kept, rest, "{piece}");
            }
        }
        p.parts = 0;
        assert!(drummer_notes(&p, &bars).is_empty());
    }

    #[test]
    fn fills_close_every_fourth_bar_when_asked_for() {
        let bars = four_four(8);
        let mut p = DrummerParams::new(DrumStyle::Rock);
        let tom_notes = |p: &DrummerParams| {
            drummer_notes(p, &bars)
                .into_iter()
                .filter(|n| part_of(n.pitch) == parts::TOMS)
                .collect::<Vec<_>>()
        };
        p.fills = 0;
        assert!(tom_notes(&p).is_empty());
        p.fills = 100;
        let toms = tom_notes(&p);
        assert!(!toms.is_empty());
        // Only in the fourth and eighth bar, in their last two beats at most.
        for n in &toms {
            let bar = (n.start / 4.0) as usize;
            assert!(bar == 3 || bar == 7, "a fill in bar {bar}");
            assert!(n.start % 4.0 >= 2.0);
        }
        // Without toms the fill is on the snare.
        p.parts = parts::ALL & !parts::TOMS;
        assert!(tom_notes(&p).is_empty());
    }

    #[test]
    fn other_signatures_get_a_beat_that_fits() {
        for (num, den) in [
            (3u8, 4u8),
            (2, 4),
            (5, 4),
            (6, 8),
            (7, 8),
            (9, 8),
            (12, 8),
            (5, 8),
            (1, 4),
            (7, 4),
        ] {
            let beats = num as f64 * 4.0 / den as f64;
            let bars = vec![BarSpec { beats, num, den }; 8];
            for style in DrumStyle::ALL {
                let mut p = DrummerParams::new(style);
                p.complexity = 100;
                p.fills = 100;
                p.swing = 100;
                let notes = drummer_notes(&p, &bars);
                assert!(!notes.is_empty(), "{num}/{den} {style:?}");
                for n in &notes {
                    let bar = (n.start as f64 / beats).floor();
                    let end = (bar + 1.0) * beats;
                    assert!(
                        (n.start as f64) < end - 1e-6 && (n.start + n.len) as f64 <= end + 1e-4,
                        "{num}/{den} {style:?}: note at {} len {} crosses the bar line {end}",
                        n.start,
                        n.len
                    );
                }
                // A kick or a ride starts every bar.
                for b in 0..8 {
                    let from = b as f64 * beats;
                    assert!(
                        notes.iter().any(|n| (n.start as f64) >= from
                            && (n.start as f64) < from + beats
                            && matches!(n.pitch, 36 | 51)),
                        "{num}/{den} {style:?} bar {b}"
                    );
                }
            }
        }
        // A bar too short to hold a sixteenth, and none at all.
        let p = DrummerParams::new(DrumStyle::Rock);
        assert!(drummer_notes(
            &p,
            &[BarSpec {
                beats: 0.01,
                num: 1,
                den: 4
            }]
        )
        .is_empty());
        assert!(drummer_notes(&p, &[]).is_empty());
    }

    #[test]
    fn swing_pushes_the_off_beats_back() {
        let bars = four_four(2);
        let mut p = DrummerParams::new(DrumStyle::Jazz);
        p.fills = 0;
        p.swing = 0;
        let straight = drummer_notes(&p, &bars);
        p.swing = 100;
        let swung = drummer_notes(&p, &bars);
        assert_eq!(straight.len(), swung.len());
        let off_beat = |notes: &[Note]| {
            notes
                .iter()
                .filter(|n| n.pitch == drums::RIDE && (n.start % 1.0) > 0.4)
                .map(|n| n.start % 1.0)
                .fold(f32::MAX, f32::min)
        };
        assert!((off_beat(&straight) - 0.5).abs() < 1e-6);
        assert!((off_beat(&swung) - (0.5 + 1.0 / 6.0)).abs() < 1e-5);
    }

    /// `cargo test -p prowave-app show_what_each_style_plays -- --ignored --nocapture`: the
    /// first two bars (and the fill bar) of every style as a grid of sixteenth steps.
    #[test]
    #[ignore = "prints, checks nothing"]
    fn show_what_each_style_plays() {
        let names = [
            (36, "kick "),
            (38, "snare"),
            (40, "e.snr"),
            (39, "clap "),
            (42, "hat  "),
            (44, "pedal"),
            (46, "open "),
            (51, "ride "),
            (49, "crash"),
            (50, "tom hi"),
            (47, "tom mi"),
            (45, "tom lo"),
            (43, "floor"),
            (54, "tamb "),
            (56, "cowbl"),
        ];
        for style in DrumStyle::ALL {
            for (complexity, loudness) in [(30u8, 40u8), (80, 85)] {
                let mut p = DrummerParams::new(style);
                p.complexity = complexity;
                p.loudness = loudness;
                p.fills = 100;
                let notes = drummer_notes(&p, &four_four(4));
                println!("\n{style:?} complexity {complexity} loudness {loudness}");
                for bar in [0usize, 3] {
                    println!("  bar {}", bar + 1);
                    for (pitch, name) in names {
                        let mut row = String::new();
                        let mut any = false;
                        for step in 0..16 {
                            let at = bar as f32 * 4.0 + step as f32 * 0.25;
                            let hit = notes
                                .iter()
                                .find(|n| n.pitch == pitch && (n.start - at).abs() < 0.2);
                            any |= hit.is_some();
                            row.push(match hit {
                                None => '.',
                                Some(n) if n.vel >= 90 => 'X',
                                Some(n) if n.vel >= 55 => 'x',
                                Some(_) => 'o',
                            });
                            if step % 4 == 3 {
                                row.push(' ');
                            }
                        }
                        if any {
                            println!("    {name:6} {row}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn style_names_round_trip() {
        for style in DrumStyle::ALL {
            assert_eq!(DrumStyle::from_name(style.name()), style);
        }
        assert_eq!(DrumStyle::from_name("polka"), DrumStyle::Rock);
        let mut p = DrummerParams {
            style: DrumStyle::Pop,
            complexity: 200,
            loudness: 200,
            swing: 200,
            fills: 200,
            parts: 255,
            seed: 0,
        };
        p.tidy();
        assert_eq!(
            (p.complexity, p.loudness, p.swing, p.fills, p.parts),
            (100, 100, 100, 100, 63)
        );
        // Seed 0 is allowed and gives a drummer like any other.
        assert!(!drummer_notes(&p, &four_four(2)).is_empty());
    }

    #[test]
    fn a_huge_region_of_fast_steps_stops_at_the_limit() {
        let mut p = PatternParams::drums();
        p.rate = StepRate::ThirtySecond;
        for row in &mut p.rows {
            row.steps.fill(100);
        }
        let began = std::time::Instant::now();
        let notes = p.notes(1.0e9);
        assert!(
            notes.len() >= MAX_PATTERN_NOTES,
            "it went on until the limit"
        );
        assert!(notes.len() <= MAX_PATTERN_NOTES + p.rows.len() * 8);
        assert!(began.elapsed().as_secs() < 5);
    }
}
