//! The Channel Rack's feel: for each drum class (kick, snare, closed hat…) a swing and sixteen
//! micro-timing offsets, one for each sixteenth of the bar. A channel plays its class's feel:
//! step `k` sounds at the straight sixteenth `k`, moved by the class's swing, plus the class's
//! offset for sixteenth `k % 16` of the bar ([`Feel::at`]). The notes of the rack's regions are
//! where the feel puts them, so the Piano Roll, playback, the bounce and an exported MIDI file
//! all have the timing the rack shows ([`retime`]).
//!
//! Swing is the place of the off-beat inside its pair: 50 straight, 66.7 a triplet feel. At 8th
//! swing the pair is a quarter note (the "&" moves); at 16th swing it is an eighth (the "e" and
//! "a" move). An offset is in quarter notes, on top of the swing: a groove measured from a
//! recording keeps, for each class and sixteenth, what is left of the hits' place once the
//! class's swing is taken out, so with the swing it was measured at the steps land where the
//! recording's hits were, and turning the swing keeps the rest of the feel.
//!
//! A step can be split (a ratchet, a roll, a triplet): a hit is at a [`Cell`], step `k` and a
//! fraction `sub / div` of the way to step `k + 1` with `div` 1 to 4. The fractions are those of
//! recorded hat rolls and triplets (0, 1/4, 1/3, 1/2, 2/3, 3/4 of a sixteenth: 64ths, 32nd
//! triplets, 32nds; an 8th triplet is 1/3 of the second sixteenth of its beat). A split hit plays
//! that fraction of the way from where its step plays to where the next straight step swings
//! to ([`Feel::place`]), so it stays inside its step at any swing.
//!
//! The groove's amounts scale it: the swing amount takes each class's swing toward straight (0:
//! straight, 1: as loaded), the offset amount its offsets ([`Groove::play`]).
use crate::kit_classify::DrumClass;
use crate::model::Note;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Sixteenths in a bar: the offsets a class has.
pub const SLOTS: usize = 16;
/// One step, in quarter notes.
pub const STEP: f64 = 0.25;
/// The swing a class can have, in percent.
pub const SWING_MIN: f32 = 35.0;
pub const SWING_MAX: f32 = 85.0;
pub const STRAIGHT: f32 = 50.0;
/// The largest offset, in quarter notes (a little under half a sixteenth: further would be the
/// next step).
pub const OFFSET_MAX: f32 = 0.12;
/// How near a note has to be to where a step plays to be that step, in quarter notes.
const NEAR: f32 = 0.01;
/// The most a step can be split into.
pub const MAX_DIV: u8 = 4;

/// Where a hit is on the rack's grid: step `step` (a sixteenth, from the region's start) and
/// `sub / div` of the way to the next step. Kept in lowest terms (`div` 1 when `sub` is 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cell {
    pub step: usize,
    pub sub: u8,
    pub div: u8,
}

fn gcd(a: u8, b: u8) -> u8 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// The least common multiple of two divisions: what a step holding hits of both is drawn as.
pub fn lcm(a: u8, b: u8) -> u8 {
    let (a, b) = (a.max(1), b.max(1));
    a / gcd(a, b) * b
}

impl Cell {
    /// Step `step`, `sub / div` of the way to the next (`div` 1 to [`MAX_DIV`], `sub` below
    /// it), in lowest terms.
    pub fn new(step: usize, sub: u8, div: u8) -> Cell {
        let div = div.clamp(1, MAX_DIV);
        let sub = sub.min(div - 1);
        if sub == 0 {
            return Cell {
                step,
                sub: 0,
                div: 1,
            };
        }
        let g = gcd(sub, div);
        Cell {
            step,
            sub: sub / g,
            div: div / g,
        }
    }

    /// The whole step.
    pub fn on(step: usize) -> Cell {
        Cell {
            step,
            sub: 0,
            div: 1,
        }
    }

    /// How far into its step, from 0 to under 1.
    pub fn frac(self) -> f64 {
        f64::from(self.sub) / f64::from(self.div.max(1))
    }

    /// The cell `frac` of a sixteenth after the start of step `step` that a division of 1 to
    /// [`MAX_DIV`] can hold, the nearest (a fraction past the last one rounds to the next step).
    pub fn nearest(step: usize, frac: f64) -> Cell {
        let mut best = (Cell::on(step), frac.abs());
        if (1.0 - frac).abs() < best.1 {
            best = (Cell::on(step + 1), (1.0 - frac).abs());
        }
        for div in 2..=MAX_DIV {
            for sub in 1..div {
                let d = (f64::from(sub) / f64::from(div) - frac).abs();
                if d < best.1 - 1e-9 {
                    best = (Cell::new(step, sub, div), d);
                }
            }
        }
        best.0
    }

    /// The cell of a place `beats` quarter notes from the region's start on the straight grid.
    pub fn of_straight(beats: f64) -> Cell {
        let x = (beats / STEP).max(0.0);
        let k = x.floor();
        Cell::nearest(k as usize, x - k)
    }

    /// Where the cell is on the straight grid, in quarter notes.
    pub fn straight(self) -> f64 {
        (self.step as f64 + self.frac()) * STEP
    }
}

/// Which off-beats swing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Sub {
    #[default]
    Sixteenth,
    Eighth,
}

impl Sub {
    pub fn parse(s: &str) -> Option<Sub> {
        match s.trim().to_lowercase().as_str() {
            "8th" | "8" | "eighth" => Some(Sub::Eighth),
            "16th" | "16" | "sixteenth" => Some(Sub::Sixteenth),
            _ => None,
        }
    }

    /// "16th" or "8th".
    pub fn id(self) -> &'static str {
        match self {
            Sub::Sixteenth => "16th",
            Sub::Eighth => "8th",
        }
    }

    /// Quarter notes in a swing pair.
    pub fn pair(self) -> f64 {
        match self {
            Sub::Eighth => 1.0,
            Sub::Sixteenth => 0.5,
        }
    }
}

/// Where straight position `beat` (quarter notes) goes with swing `pct` at `sub`: in each
/// pair the first half is stretched to `pct` percent of it and the second squeezed into the
/// rest, so the beats stay put and the off-beat lands at `pct`.
pub fn swing_beat(beat: f64, pct: f64, sub: Sub) -> f64 {
    let pair = sub.pair();
    let s = (pct / 100.0).clamp(0.01, 0.99);
    let x = beat / pair;
    let k = x.floor();
    let f = x - k;
    let g = if f <= 0.5 {
        f * 2.0 * s
    } else {
        s + (f - 0.5) * 2.0 * (1.0 - s)
    };
    (k + g) * pair
}

/// One class's feel: its swing and its offset on each sixteenth of the bar.
#[derive(Clone, Debug, PartialEq)]
pub struct Feel {
    pub swing: f32,
    pub sub: Sub,
    /// Quarter notes, on top of the swing, for sixteenths 1 to 16 of the bar.
    pub offsets: [f32; SLOTS],
    /// How much of these offsets this class plays, from 0 (none) to 1 (as loaded): the drum's
    /// own Offset, under the rack's (`Groove::offset_amount`), which scales every class.
    pub offset_amount: f32,
}

impl Default for Feel {
    fn default() -> Self {
        Self {
            swing: STRAIGHT,
            sub: Sub::Sixteenth,
            offsets: [0.0; SLOTS],
            offset_amount: 1.0,
        }
    }
}

impl Feel {
    /// Where straight step `k` swings to, without its offset.
    fn swung(&self, k: usize) -> f64 {
        swing_beat(k as f64 * STEP, f64::from(self.swing), self.sub)
    }

    /// Where step `k` (from the region's start, a sixteenth each) plays, in quarter notes.
    pub fn at(&self, k: usize) -> f32 {
        (self.swung(k) + f64::from(self.offsets[k % SLOTS])).max(0.0) as f32
    }

    /// Where a hit at `cell` plays: its step's place, and its fraction of the way from there to
    /// where the next step swings to.
    pub fn place(&self, cell: Cell) -> f32 {
        let at = f64::from(self.at(cell.step));
        if cell.sub == 0 {
            return at as f32;
        }
        let span = (self.swung(cell.step + 1) - self.swung(cell.step)).max(0.0);
        (at + cell.frac() * span) as f32
    }

    /// This feel with its swing `swing` of the way from straight and its offsets `offset` of
    /// theirs (amounts from 0 to 1), times its own offset amount; the result plays its offsets
    /// whole.
    pub fn scaled(&self, swing: f32, offset: f32) -> Feel {
        let (a, o) = (swing.clamp(0.0, 1.0), offset.clamp(0.0, 1.0));
        let o = o * self.offset_amount.clamp(0.0, 1.0);
        Feel {
            swing: STRAIGHT + (self.swing - STRAIGHT) * a,
            sub: self.sub,
            offsets: self.offsets.map(|x| x * o),
            offset_amount: 1.0,
        }
    }

    fn to_json(&self) -> Value {
        let mut v = json!({ "swing": self.swing, "sub": self.sub.id() });
        if self.offsets.iter().any(|o| *o != 0.0) {
            v["offsets"] = json!(self.offsets.to_vec());
        }
        if self.offset_amount != 1.0 {
            v["offset_amount"] = json!(self.offset_amount);
        }
        v
    }

    fn from_json(v: &Value) -> Feel {
        let mut f = Feel {
            swing: v["swing"]
                .as_f64()
                .map_or(STRAIGHT, |x| (x as f32).clamp(SWING_MIN, SWING_MAX)),
            sub: v["sub"].as_str().and_then(Sub::parse).unwrap_or_default(),
            offsets: [0.0; SLOTS],
            offset_amount: v["offset_amount"]
                .as_f64()
                .filter(|x| x.is_finite())
                .map_or(1.0, |x| (x as f32).clamp(0.0, 1.0)),
        };
        for (k, o) in v["offsets"].as_array().into_iter().flatten().enumerate() {
            if k < SLOTS {
                f.offsets[k] = o
                    .as_f64()
                    .filter(|x| x.is_finite())
                    .map_or(0.0, |x| (x as f32).clamp(-OFFSET_MAX, OFFSET_MAX));
            }
        }
        f
    }
}

/// The rack's feel: a [`Feel`] for each class that has one of its own, and the one every other
/// class plays.
#[derive(Clone, Debug, PartialEq)]
pub struct Groove {
    /// What it was loaded from, as shown (empty: none, the swing set by hand).
    pub name: String,
    /// Where it came from (a library's short name; empty: by hand).
    pub source: String,
    pub classes: BTreeMap<DrumClass, Feel>,
    pub default: Feel,
    /// How much of the swing plays, from 0 (straight) to 1 (as loaded or set).
    pub swing_amount: f32,
    /// How much of the offsets plays, from 0 (none) to 1.
    pub offset_amount: f32,
    /// The velocity of each class's hits on each sixteenth of the bar, as measured (a source
    /// that has none: empty). The rack gives them to its steps when the groove is loaded.
    pub velocities: BTreeMap<DrumClass, [Option<u8>; SLOTS]>,
}

impl Default for Groove {
    fn default() -> Self {
        Self {
            name: String::new(),
            source: String::new(),
            classes: BTreeMap::new(),
            default: Feel::default(),
            swing_amount: 1.0,
            offset_amount: 1.0,
            velocities: BTreeMap::new(),
        }
    }
}

impl Groove {
    /// The feel of class `c` as loaded or set (before the amounts).
    pub fn feel(&self, c: DrumClass) -> &Feel {
        self.classes.get(&c).unwrap_or(&self.default)
    }

    /// The velocity the groove gives class `c`'s hits on step `k` (its sixteenth of the bar).
    pub fn velocity(&self, c: DrumClass, k: usize) -> Option<u8> {
        self.velocities.get(&c)?[k % SLOTS]
    }

    /// The feel class `c` plays: its feel with the amounts applied.
    pub fn play(&self, c: DrumClass) -> Feel {
        self.feel(c).scaled(self.swing_amount, self.offset_amount)
    }

    /// Every step on its straight sixteenth.
    pub fn is_straight(&self) -> bool {
        let straight = Feel::default();
        std::iter::once(&self.default)
            .chain(self.classes.values())
            .all(|f| f.scaled(self.swing_amount, self.offset_amount) == straight)
    }

    /// Sets the swing of class `c` (None: of every class), keeping the offsets.
    pub fn set_swing(&mut self, c: Option<DrumClass>, pct: f32, sub: Option<Sub>) {
        let pct = pct.clamp(SWING_MIN, SWING_MAX);
        let set = |f: &mut Feel| {
            f.swing = pct;
            if let Some(s) = sub {
                f.sub = s;
            }
        };
        match c {
            Some(c) => {
                let default = self.default.clone();
                set(self.classes.entry(c).or_insert(default));
            }
            None => {
                set(&mut self.default);
                self.classes.values_mut().for_each(set);
            }
        }
    }

    /// Sets how much of its offsets class `c` plays (None: every class), from 0 to 1.
    pub fn set_offset(&mut self, c: Option<DrumClass>, amount: f32) {
        let amount = amount.clamp(0.0, 1.0);
        match c {
            Some(c) => {
                let default = self.default.clone();
                self.classes.entry(c).or_insert(default).offset_amount = amount;
            }
            None => {
                self.default.offset_amount = amount;
                self.classes
                    .values_mut()
                    .for_each(|f| f.offset_amount = amount);
            }
        }
    }

    pub fn to_json(&self) -> Value {
        let classes: serde_json::Map<String, Value> = self
            .classes
            .iter()
            .map(|(c, f)| (c.id().to_string(), f.to_json()))
            .collect();
        let mut v = json!({
            "name": self.name,
            "source": self.source,
            "default": self.default.to_json(),
            "classes": classes,
        });
        if !self.velocities.is_empty() {
            let vel: serde_json::Map<String, Value> = self
                .velocities
                .iter()
                .map(|(c, x)| (c.id().to_string(), json!(x.to_vec())))
                .collect();
            v["velocities"] = Value::Object(vel);
        }
        if self.swing_amount != 1.0 || self.offset_amount != 1.0 {
            v["amounts"] = json!({ "swing": self.swing_amount, "offset": self.offset_amount });
        }
        v
    }

    pub fn from_json(v: &Value) -> Groove {
        let amount = |k: &str| {
            v["amounts"][k]
                .as_f64()
                .filter(|x| x.is_finite())
                .map_or(1.0, |x| (x as f32).clamp(0.0, 1.0))
        };
        let velocities = v["velocities"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, a)| {
                let c = DrumClass::from_id(k)?;
                let a = a.as_array()?;
                let mut out = [None; SLOTS];
                for (i, x) in a.iter().take(SLOTS).enumerate() {
                    out[i] = x.as_u64().map(|x| x.clamp(1, 127) as u8);
                }
                Some((c, out))
            })
            .collect();
        Groove {
            velocities,
            swing_amount: amount("swing"),
            offset_amount: amount("offset"),
            name: v["name"].as_str().unwrap_or_default().to_string(),
            source: v["source"].as_str().unwrap_or_default().to_string(),
            default: Feel::from_json(&v["default"]),
            classes: v["classes"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(k, f)| Some((DrumClass::from_id(k)?, Feel::from_json(f))))
                .collect(),
        }
    }
}

/// The cell `start` is with feel `feel`: where the feel plays some step or split hit (the
/// nearest), else a straight sixteenth or a split of one; None when it is neither (a note moved
/// by hand).
pub fn cell_of(start: f32, feel: &Feel) -> Option<Cell> {
    let near = (f64::from(start) / STEP).round() as i64;
    let around = || ((near - 4).max(0)..=(near + 4).max(0)).map(|k| k as usize);
    let cells = |k: usize| {
        std::iter::once(Cell::on(k))
            .chain((2..=MAX_DIV).flat_map(move |d| (1..d).map(move |j| Cell::new(k, j, d))))
    };
    let best = |at: &dyn Fn(Cell) -> f32| {
        around()
            .flat_map(cells)
            .map(|c| (c, (at(c) - start).abs()))
            .filter(|(_, d)| *d < NEAR)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(c, _)| c)
    };
    best(&|c| feel.place(c)).or_else(|| best(&|c| c.straight() as f32))
}

/// Moves the rack's notes in `notes` (channel `c` on note `base + c`) from where `old` and the
/// channels' classes then (`old_classes`) put each hit to where `new` and `new_classes` do.
/// A note that is on no step (moved by hand) stays where it is. Returns how many moved.
pub fn retime(
    notes: &mut [Note],
    base: u8,
    old: &Groove,
    old_classes: &[DrumClass],
    new: &Groove,
    new_classes: &[DrumClass],
) -> usize {
    let mut moved = 0;
    for n in notes.iter_mut() {
        let Some(c) = n.pitch.checked_sub(base).map(usize::from) else {
            continue;
        };
        let (Some(oc), Some(nc)) = (old_classes.get(c), new_classes.get(c)) else {
            continue;
        };
        let Some(cell) = cell_of(n.start, &old.play(*oc)) else {
            continue;
        };
        let to = new.play(*nc).place(cell);
        if to != n.start {
            n.start = to;
            moved += 1;
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pitch: u8, start: f32) -> Note {
        Note {
            start,
            len: 0.25,
            pitch,
            vel: 100,
            mute: false,
            lyric: crate::lyrics::Lyric::NONE,
        }
    }

    #[test]
    fn swing_moves_the_off_beats_and_keeps_the_beats() {
        let f = |b: f64, p: f64, s: Sub| swing_beat(b, p, s);
        // Straight leaves everything.
        for k in 0..16 {
            let b = k as f64 * 0.25;
            assert!((f(b, 50.0, Sub::Sixteenth) - b).abs() < 1e-12);
            assert!((f(b, 50.0, Sub::Eighth) - b).abs() < 1e-12);
        }
        // 16th swing at 60: the "e" and "a" move 0.1 of an eighth later (0.05 beats), the
        // beat and the "&" stay.
        assert!((f(0.25, 60.0, Sub::Sixteenth) - 0.30).abs() < 1e-12);
        assert!((f(0.75, 60.0, Sub::Sixteenth) - 0.80).abs() < 1e-12);
        assert_eq!(f(0.5, 60.0, Sub::Sixteenth), 0.5);
        assert_eq!(f(1.0, 60.0, Sub::Sixteenth), 1.0);
        // 8th swing at 2/3: the "&" lands on the third triplet; the "e" halfway to it.
        assert!((f(0.5, 200.0 / 3.0, Sub::Eighth) - 2.0 / 3.0).abs() < 1e-12);
        assert!((f(0.25, 200.0 / 3.0, Sub::Eighth) - 1.0 / 3.0).abs() < 1e-12);
        assert_eq!(f(2.0, 66.0, Sub::Eighth), 2.0);
    }

    #[test]
    fn a_step_plays_at_its_swing_plus_its_sixteenths_offset() {
        let mut feel = Feel {
            swing: 60.0,
            sub: Sub::Sixteenth,
            offsets: [0.0; SLOTS],
            ..Feel::default()
        };
        feel.offsets[1] = 0.01;
        feel.offsets[4] = -0.02;
        assert_eq!(feel.at(0), 0.0);
        assert!((feel.at(1) - 0.31).abs() < 1e-6);
        assert!((feel.at(4) - 0.98).abs() < 1e-6);
        // The next bar's sixteenths take the same offsets.
        assert!((feel.at(17) - 4.31).abs() < 1e-6);
        // A step can not play before the region starts.
        feel.offsets[0] = -0.05;
        assert_eq!(feel.at(0), 0.0);
    }

    #[test]
    fn each_class_plays_its_own_feel_and_the_rest_the_default() {
        let mut g = Groove::default();
        assert!(g.is_straight());
        g.set_swing(Some(DrumClass::ClosedHat), 62.0, None);
        assert_eq!(g.feel(DrumClass::ClosedHat).swing, 62.0);
        assert_eq!(g.feel(DrumClass::Kick).swing, 50.0);
        assert!(!g.is_straight());
        g.set_swing(None, 55.0, Some(Sub::Eighth));
        assert_eq!(g.feel(DrumClass::ClosedHat).swing, 55.0);
        assert_eq!(g.feel(DrumClass::Snare).sub, Sub::Eighth);
        g.set_swing(Some(DrumClass::Kick), 99.0, None);
        assert_eq!(g.feel(DrumClass::Kick).swing, SWING_MAX);
        g.classes.get_mut(&DrumClass::Kick).unwrap().offsets[3] = 0.02;
        g.name = "A Song".into();
        g.source = "file".into();
        assert_eq!(Groove::from_json(&g.to_json()), g);
        assert_eq!(Groove::from_json(&Value::Null), Groove::default());
    }

    #[test]
    fn notes_move_from_one_groove_to_another_per_channel_class() {
        let base = 36;
        // Channel 0 kick, channel 1 closed hat; hats on every sixteenth, kick on 1 and 3.
        let mut notes: Vec<Note> = (0..16).map(|k| note(base + 1, k as f32 * 0.25)).collect();
        notes.push(note(base, 0.0));
        notes.push(note(base, 0.75));
        // A note put off the grid by hand.
        notes.push(note(base, 1.105));
        let straight = Groove::default();
        let classes = [DrumClass::Kick, DrumClass::ClosedHat];
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 60.0, None);
        let mut kick = Feel::default();
        kick.offsets[3] = 0.01;
        g.classes.insert(DrumClass::Kick, kick);
        let moved = retime(&mut notes, base, &straight, &classes, &g, &classes);
        // The hats' "e" and "a" (8 of them) and the kick on sixteenth 4.
        assert_eq!(moved, 9);
        assert!((notes[1].start - 0.30).abs() < 1e-6);
        assert_eq!(notes[2].start, 0.5);
        assert!((notes[17].start - 0.76).abs() < 1e-6);
        assert_eq!(notes[18].start, 1.105, "a note on no step stays");
        // Back to straight puts every note where it was.
        retime(&mut notes, base, &g, &classes, &straight, &classes);
        for (k, n) in notes.iter().take(16).enumerate() {
            assert!((n.start - k as f32 * 0.25).abs() < 1e-6);
        }
        assert!((notes[17].start - 0.75).abs() < 1e-6);
        // A channel given another class takes that class's feel.
        let hats = [DrumClass::ClosedHat, DrumClass::ClosedHat];
        retime(&mut notes, base, &straight, &classes, &g, &hats);
        assert!((notes[17].start - 0.80).abs() < 1e-6);
        // Notes of other pitches are not touched.
        let mut other = vec![note(10, 0.25)];
        assert_eq!(retime(&mut other, base, &straight, &classes, &g, &hats), 0);
    }

    #[test]
    fn a_step_is_found_where_its_feel_plays_it_or_on_the_straight_grid() {
        let mut f = Feel {
            swing: 80.0,
            sub: Sub::Eighth,
            offsets: [0.0; SLOTS],
            ..Feel::default()
        };
        f.offsets[2] = 0.05;
        // 8th swing at 80 puts the "&" 0.3 later and the offset 0.05 more.
        let at = f.at(2);
        assert!((at - 0.85).abs() < 1e-6);
        assert_eq!(cell_of(at, &f), Some(Cell::on(2)));
        assert_eq!(cell_of(0.0, &f), Some(Cell::on(0)));
        assert_eq!(cell_of(1.0, &f), Some(Cell::on(4)));
        assert_eq!(cell_of(1.75, &f), Some(Cell::on(7)), "on the straight grid");
        // Where the feel plays a split hit comes first: 0.5 is a quarter into the "e" here.
        assert_eq!(cell_of(0.5, &f), Some(Cell::new(1, 1, 4)));
        assert_eq!(cell_of(0.645, &f), None);
    }

    #[test]
    fn cells_hold_the_rolls_and_triplets_of_the_drake_songs() {
        // Kept in lowest terms.
        assert_eq!(Cell::new(3, 2, 4), Cell::new(3, 1, 2));
        assert_eq!(Cell::new(3, 0, 3), Cell::on(3));
        assert_eq!(
            Cell::new(0, 9, 3),
            Cell::new(0, 2, 3),
            "sub past the division: its last"
        );
        assert_eq!(lcm(3, 4), 12);
        assert_eq!(lcm(2, 4), 4);
        assert_eq!(lcm(0, 3), 3);
        // Roll and triplet places measured from records (fractions of a sixteenth).
        for (beats, cell) in [
            (231.4375, Cell::new(925, 3, 4)), // a 64th roll
            (231.5833, Cell::new(926, 1, 3)), // a 32nd triplet
            (231.6667, Cell::new(926, 2, 3)),
            (164.3333, Cell::new(657, 1, 3)), // an 8th triplet: 1/3 into the "e"
            (2.125, Cell::new(8, 1, 2)),      // a 32nd
            (2.0, Cell::on(8)),
            (2.2499, Cell::on(9)),
        ] {
            let c = Cell::of_straight(beats);
            assert_eq!(c, cell, "{beats}");
            assert!((c.straight() - beats).abs() < 0.001, "{beats}");
        }
    }

    #[test]
    fn a_split_hit_plays_inside_its_swung_step() {
        let mut f = Feel {
            swing: 60.0,
            sub: Sub::Sixteenth,
            offsets: [0.0; SLOTS],
            ..Feel::default()
        };
        // Straight: a fraction of a sixteenth.
        let straight = Feel::default();
        assert!((straight.place(Cell::new(1, 1, 2)) - 0.375).abs() < 1e-6);
        assert!((straight.place(Cell::new(2, 2, 3)) - (0.5 + 0.5 / 3.0)).abs() < 1e-6);
        // At 60 the "e" is at 0.30 and the "&" stays at 0.5: the "e"'s span is 0.2.
        assert!((f.place(Cell::new(1, 1, 2)) - 0.40).abs() < 1e-6);
        // The downbeat's span is 0.30.
        assert!((f.place(Cell::new(0, 1, 3)) - 0.10).abs() < 1e-6);
        // An offset moves the step and its split hits together.
        f.offsets[1] = 0.01;
        assert!((f.place(Cell::new(1, 3, 4)) - (0.31 + 0.15)).abs() < 1e-6);
        assert_eq!(f.place(Cell::on(1)), f.at(1));
        // Every cell of a step is found again where it plays, and on the straight grid.
        for k in 0..20 {
            for d in 1..=MAX_DIV {
                for j in 0..d {
                    let c = Cell::new(k, j, d);
                    assert_eq!(cell_of(f.place(c), &f), Some(c), "{c:?}");
                    if j == 0 {
                        assert_eq!(cell_of(c.straight() as f32, &f), Some(c), "{c:?} straight");
                    }
                }
            }
        }
    }

    /// Each drum class has its own offset amount; the rack's Offset scales it, so a class plays
    /// its offsets times both. Kept with the groove (left out at 100 %).
    #[test]
    fn each_class_plays_its_own_share_of_its_offsets_under_the_racks() {
        let mut g = Groove::default();
        for c in [DrumClass::Kick, DrumClass::Snare] {
            let mut f = Feel::default();
            f.offsets[4] = 0.04;
            g.classes.insert(c, f);
        }
        g.default.offsets[4] = 0.04;
        g.set_offset(Some(DrumClass::Snare), 0.5);
        assert_eq!(g.feel(DrumClass::Snare).offset_amount, 0.5);
        assert_eq!(g.feel(DrumClass::Kick).offset_amount, 1.0);
        let at = |g: &Groove, c| g.play(c).at(4);
        assert!((at(&g, DrumClass::Kick) - 1.04).abs() < 1e-6);
        assert!((at(&g, DrumClass::Snare) - 1.02).abs() < 1e-6);
        // The rack's Offset at 50 %: both halved.
        g.offset_amount = 0.5;
        assert!((at(&g, DrumClass::Kick) - 1.02).abs() < 1e-6);
        assert!((at(&g, DrumClass::Snare) - 1.01).abs() < 1e-6);
        // A class with no feel of its own gets one, from the default, with its own amount.
        g.offset_amount = 1.0;
        g.set_offset(Some(DrumClass::Clap), 0.0);
        assert!((at(&g, DrumClass::Clap) - 1.0).abs() < 1e-6);
        assert!((at(&g, DrumClass::Perc) - 1.04).abs() < 1e-6);
        assert!(!g.is_straight());
        // Kept, and out of range comes back in range.
        let back = Groove::from_json(&g.to_json());
        assert_eq!(back, g);
        assert!(!g.to_json()["classes"]["kick"]
            .as_object()
            .unwrap()
            .contains_key("offset_amount"));
        g.set_offset(None, 7.0);
        assert!(g.classes.values().all(|f| f.offset_amount == 1.0));
        let mut v = g.to_json();
        v["classes"]["snare"]["offset_amount"] = serde_json::json!(-3.0);
        assert_eq!(
            Groove::from_json(&v).feel(DrumClass::Snare).offset_amount,
            0.0
        );
        // Every class at 0: straight.
        g.set_offset(None, 0.0);
        assert!(g.is_straight());
    }

    #[test]
    fn the_amounts_take_the_swing_and_offsets_toward_straight() {
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 66.0, None);
        g.classes.get_mut(&DrumClass::ClosedHat).unwrap().offsets[1] = 0.02;
        let full = g.play(DrumClass::ClosedHat);
        assert_eq!(full.swing, 66.0);
        let mut vel = [None; SLOTS];
        vel[1] = Some(77);
        g.velocities.insert(DrumClass::ClosedHat, vel);
        assert_eq!(g.velocity(DrumClass::ClosedHat, 17), Some(77));
        assert_eq!(g.velocity(DrumClass::ClosedHat, 2), None);
        assert_eq!(g.velocity(DrumClass::Kick, 1), None);
        g.swing_amount = 0.5;
        g.offset_amount = 0.25;
        let half = g.play(DrumClass::ClosedHat);
        assert_eq!(half.swing, 58.0);
        assert!((half.offsets[1] - 0.005).abs() < 1e-7);
        assert_eq!(g.feel(DrumClass::ClosedHat).swing, 66.0, "as loaded, kept");
        assert!(!g.is_straight());
        assert_eq!(Groove::from_json(&g.to_json()), g);
        g.swing_amount = 0.0;
        g.offset_amount = 0.0;
        assert!(g.is_straight(), "both amounts at 0: straight");
        assert_eq!(Groove::from_json(&g.to_json()), g);
        // A groove saved before amounts: both full.
        let old = json!({"name": "x", "default": {"swing": 55.0}});
        let o = Groove::from_json(&old);
        assert_eq!((o.swing_amount, o.offset_amount), (1.0, 1.0));
    }

    #[test]
    fn split_hits_move_with_their_steps_when_the_feel_changes() {
        let base = 36;
        let straight = Groove::default();
        let classes = [DrumClass::ClosedHat];
        let cells = [
            Cell::on(1),
            Cell::new(1, 1, 3),
            Cell::new(1, 2, 3),
            Cell::new(2, 3, 4),
        ];
        let mut notes: Vec<Note> = cells
            .iter()
            .map(|c| note(base, c.straight() as f32))
            .collect();
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 60.0, None);
        assert_eq!(
            retime(&mut notes, base, &straight, &classes, &g, &classes),
            4
        );
        let f = g.play(DrumClass::ClosedHat);
        for (n, c) in notes.iter().zip(cells) {
            assert!((n.start - f.place(c)).abs() < 1e-6);
        }
        // Amounts to 0 and back: every hit where it was.
        let mut zero = g.clone();
        zero.swing_amount = 0.0;
        retime(&mut notes, base, &g, &classes, &zero, &classes);
        for (n, c) in notes.iter().zip(cells) {
            assert!((f64::from(n.start) - c.straight()).abs() < 1e-6);
        }
        retime(&mut notes, base, &zero, &classes, &g, &classes);
        for (n, c) in notes.iter().zip(cells) {
            assert!((n.start - f.place(c)).abs() < 1e-6);
        }
    }
}
