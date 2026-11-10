//! The Score editor (Logic Pro: Score): the notes of a MIDI region as notation on a grand
//! staff, treble from middle C up and bass below it, with the clefs, the key signature (the
//! project's key, picked at the top of the editor) and the time signature, accidentals that
//! hold to the end of the bar, rests, and notes that cross a bar line or have a length that is
//! not one note value tied together.
//!
//! The notation is a reading of the notes, not a second copy of them: starts and lengths are
//! rounded to sixteenths for the drawing only. The bars are set as systems that fill the
//! width, spaced by duration with room for every glyph. There are no beams, voices or chord
//! symbols yet. Click on a staff to add a note (a quarter note at the nearest eighth, spelled in the
//! key), right-click a note to delete it.
use crate::hw;
use crate::i18n::tr;
use crate::model::{Note, Project};
use crate::piano_roll::caption;
use crate::theme::{self, Face};
use crate::UiState;
use egui::{
    epaint::CubicBezierShape, pos2, vec2, Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Distance between two staff lines, in points.
const SPACE: f32 = 9.0;
const HALF: f32 = SPACE / 2.0;
/// Music is printed in black on a bone sheet set into the chassis.
const INK: Color32 = theme::INK;
const STAFF_INK: Color32 = theme::INK2;
const PAPER: Color32 = theme::BONE_HI;
/// Width reserved at the left for the clefs.
const CLEF_W: f32 = 50.0;
/// The margin of chassis around the sheet.
const SHEET_PAD: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Staff {
    Treble,
    Bass,
}

// ------------------------------------------------------------------------- keys

/// The keys the editor offers, with the number of sharps (positive) or flats (negative) each
/// has. A minor key has the signature of its relative major.
pub const KEYS: [(&str, i8); 30] = [
    ("Cmaj", 0),
    ("Gmaj", 1),
    ("Dmaj", 2),
    ("Amaj", 3),
    ("Emaj", 4),
    ("Bmaj", 5),
    ("F#maj", 6),
    ("C#maj", 7),
    ("Fmaj", -1),
    ("Bbmaj", -2),
    ("Ebmaj", -3),
    ("Abmaj", -4),
    ("Dbmaj", -5),
    ("Gbmaj", -6),
    ("Cbmaj", -7),
    ("Amin", 0),
    ("Emin", 1),
    ("Bmin", 2),
    ("F#min", 3),
    ("C#min", 4),
    ("G#min", 5),
    ("D#min", 6),
    ("A#min", 7),
    ("Dmin", -1),
    ("Gmin", -2),
    ("Cmin", -3),
    ("Fmin", -4),
    ("Bbmin", -5),
    ("Ebmin", -6),
    ("Abmin", -7),
];

/// Sharps (positive) or flats (negative) of the key named `key` ("Cmaj", "Ebmin"...).
pub fn fifths(key: &str) -> i8 {
    KEYS.iter().find(|(k, _)| *k == key).map_or(0, |(_, f)| *f)
}

/// How a key reads: "C major", "Eb minor"...
pub fn key_label(key: &str) -> String {
    let (root, mode) = key.split_at(key.len().saturating_sub(3));
    let mode = if mode == "min" {
        tr("minor")
    } else {
        tr("major")
    };
    format!("{root} {mode}")
}

/// The white keys (C = 0 ... B = 6) the sharps and flats of a key signature fall on.
const SHARP_ORDER: [usize; 7] = [3, 0, 4, 1, 5, 2, 6]; // F C G D A E B
const FLAT_ORDER: [usize; 7] = [6, 2, 5, 1, 4, 0, 3]; // B E A D G C F
/// The pitch classes of the white keys.
const NATURAL: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// How the key signature alters each letter C-B: +1, -1 or 0.
pub fn alterations(fifths: i8) -> [i8; 7] {
    let mut alt = [0i8; 7];
    let n = fifths.unsigned_abs().min(7) as usize;
    if fifths > 0 {
        SHARP_ORDER[..n].iter().for_each(|l| alt[*l] = 1);
    } else {
        FLAT_ORDER[..n].iter().for_each(|l| alt[*l] = -1);
    }
    alt
}

/// A note as it is written: the diatonic step (C of octave 0 is 0, so middle C is 35) and how
/// it is altered (-1 flat, 0 natural, +1 sharp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spelling {
    pub step: i32,
    pub alter: i8,
}

/// How `pitch` is written in a key with `fifths` sharps or flats: the notes of the key as
/// themselves, the other black keys as sharps in a sharp key and flats in a flat key.
pub fn spell(pitch: u8, fifths: i8) -> Spelling {
    let alt = alterations(fifths);
    let pc = i32::from(pitch % 12);
    let pick = |letter: usize, alter: i8| {
        // The octave comes from the natural pitch of the letter (B# and Cb cross it).
        let natural = i32::from(pitch) - i32::from(alter);
        Spelling {
            step: natural.div_euclid(12) * 7 + letter as i32,
            alter,
        }
    };
    // 1. A note of the key.
    for letter in 0..7 {
        if (NATURAL[letter] + i32::from(alt[letter])).rem_euclid(12) == pc {
            return pick(letter, alt[letter]);
        }
    }
    // 2. A white key the signature alters.
    if let Some(letter) = NATURAL.iter().position(|n| *n == pc) {
        return pick(letter, 0);
    }
    // 3. Any other black key.
    let shift = if fifths >= 0 { 1 } else { -1 };
    let letter = NATURAL
        .iter()
        .position(|n| (*n + shift).rem_euclid(12) == pc)
        .unwrap_or(0);
    pick(letter, shift as i8)
}

/// The pitch of the line or space `step`, in a key with `fifths` sharps or flats.
pub fn pitch_in_key(step: i32, fifths: i8) -> u8 {
    let (octave, letter) = (step.div_euclid(7), step.rem_euclid(7) as usize);
    let alt = alterations(fifths)[letter];
    (octave * 12 + NATURAL[letter] + i32::from(alt)).clamp(0, 127) as u8
}

/// The white key on diatonic step `step`.
#[cfg(test)]
pub fn pitch_of(step: i32) -> u8 {
    pitch_in_key(step, 0)
}

/// The diatonic step of a pitch in C major and whether it is a black key written as a sharp.
#[cfg(test)]
pub fn diatonic(pitch: u8) -> (i32, bool) {
    let s = spell(pitch, 0);
    (s.step, s.alter != 0)
}

pub fn staff_of(pitch: u8) -> Staff {
    if pitch >= 60 {
        Staff::Treble
    } else {
        Staff::Bass
    }
}

// ------------------------------------------------------------------------- layout

/// The note values drawn, in quarter notes, longest first.
const VALUES: [f32; 8] = [4.0, 3.0, 2.0, 1.5, 1.0, 0.75, 0.5, 0.25];
const EPS: f32 = 1e-4;

/// The note value nearest to `len` quarter notes.
#[cfg(test)]
pub fn display_duration(len: f32) -> f32 {
    VALUES
        .into_iter()
        .min_by(|a, b| (a - len).abs().total_cmp(&(b - len).abs()))
        .unwrap_or(1.0)
}

/// An accidental written before a note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accidental {
    Sharp,
    Flat,
    Natural,
}

/// One written note: a note of the region, or a piece of it when it is tied across a bar line
/// or because its length is not one note value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteItem {
    /// Beats from the region's start.
    pub at: f32,
    pub staff: Staff,
    pub step: i32,
    pub accidental: Option<Accidental>,
    /// The note value, in quarter notes.
    pub value: f32,
    /// Tied to the piece that starts here.
    pub tie_to: Option<f32>,
    /// Index of the note in the region.
    pub note: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestItem {
    pub at: f32,
    pub staff: Staff,
    /// The value drawn: 4 for a whole rest (also a whole bar's).
    pub value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    Note(NoteItem),
    Rest(RestItem),
}

impl Item {
    pub fn at(&self) -> f32 {
        match self {
            Item::Note(n) => n.at,
            Item::Rest(r) => r.at,
        }
    }
}

/// Splits the stretch `[from, to)` at the bar lines in `bars` (sorted) and returns the pieces.
fn cut_at_bars(from: f32, to: f32, bars: &[f32]) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut at = from;
    let first = bars.partition_point(|b| *b <= from + EPS);
    for &b in &bars[first..] {
        if b >= to - EPS {
            break;
        }
        out.push((at, b));
        at = b;
    }
    out.push((at, to));
    out
}

/// A length as note values, largest first: tied pieces.
fn note_values(mut len: f32) -> Vec<f32> {
    let mut out = Vec::new();
    while len > EPS && out.len() < 64 {
        let v = VALUES.into_iter().find(|v| *v <= len + EPS).unwrap_or(0.25);
        out.push(v);
        len -= v;
    }
    out
}

/// A gap as rests, from `at` to `to` in the bar `bar_start..bar_end`: each rest starts on a
/// multiple of its own length, so the beats stay visible, and a whole bar is one rest.
fn rest_values(at: f32, to: f32, bar_start: f32, bar_end: f32) -> Vec<(f32, f32)> {
    if at <= bar_start + EPS && to >= bar_end - EPS {
        return vec![(at, 4.0)];
    }
    let mut out = Vec::new();
    let mut t = at;
    while t < to - EPS && out.len() < 64 {
        let into = t - bar_start;
        let v = [4.0f32, 2.0, 1.0, 0.5, 0.25]
            .into_iter()
            .find(|v| *v <= to - t + EPS && ((into / v) - (into / v).round()).abs() < 1e-3)
            .unwrap_or(0.25);
        out.push((t, v));
        t += v;
    }
    out
}

/// What the score of `notes` is: the written notes with their ties and accidentals, and the
/// rests, sorted by position. `bars` holds the start of every bar (beats from the region's
/// start, the first at 0) and, last, the end of the region.
pub fn layout(notes: &[Note], bars: &[f32], fifths: i8) -> Vec<Item> {
    let end = bars.last().copied().unwrap_or(0.0);
    let q = |x: f32| (x * 4.0).round() / 4.0;
    // The note as the score reads it: start and length in sixteenths, a spelling and a staff.
    struct Read {
        note: usize,
        start: f32,
        len: f32,
        spelling: Spelling,
        staff: Staff,
    }
    let mut reads: Vec<Read> = notes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.start.is_finite() && n.len.is_finite())
        .map(|(note, n)| Read {
            note,
            start: q(n.start.max(0.0)),
            len: q(n.len).max(0.25),
            spelling: spell(n.pitch, fifths),
            staff: staff_of(n.pitch),
        })
        .filter(|r| r.start < end - EPS)
        .collect();
    reads.sort_by(|a, b| {
        a.start
            .total_cmp(&b.start)
            .then(a.spelling.step.cmp(&b.spelling.step))
    });

    let alt = alterations(fifths);
    let mut items: Vec<Item> = Vec::new();
    // The accidentals in force in the bar being written, by step.
    let mut bar_index = usize::MAX;
    let mut held: HashMap<i32, i8> = HashMap::new();
    for r in &reads {
        let bar = bars
            .partition_point(|b| *b <= r.start + EPS)
            .saturating_sub(1);
        if bar != bar_index {
            bar_index = bar;
            held.clear();
        }
        let letter = r.spelling.step.rem_euclid(7) as usize;
        let state = held.get(&r.spelling.step).copied().unwrap_or(alt[letter]);
        let accidental = (state != r.spelling.alter).then_some(match r.spelling.alter {
            1 => Accidental::Sharp,
            -1 => Accidental::Flat,
            _ => Accidental::Natural,
        });
        held.insert(r.spelling.step, r.spelling.alter);
        // Pieces: cut at the bar lines, each cut into note values; tied from one to the next.
        let stop = (r.start + r.len).min(end);
        let mut pieces: Vec<(f32, f32)> = Vec::new();
        for (a, b) in cut_at_bars(r.start, stop, bars) {
            let mut t = a;
            for v in note_values(b - a) {
                pieces.push((t, v));
                t += v;
            }
        }
        for (k, &(at, value)) in pieces.iter().enumerate() {
            items.push(Item::Note(NoteItem {
                at,
                staff: r.staff,
                step: r.spelling.step,
                // Only the first piece carries the accidental.
                accidental: if k == 0 { accidental } else { None },
                value,
                tie_to: pieces.get(k + 1).map(|next| next.0),
                note: r.note,
            }));
        }
    }

    // Rests: what each staff does not play, bar by bar.
    for staff in [Staff::Treble, Staff::Bass] {
        let mut busy: Vec<(f32, f32)> = reads
            .iter()
            .filter(|r| r.staff == staff)
            .map(|r| (r.start, (r.start + r.len).min(end)))
            .collect();
        busy.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f32, f32)> = Vec::new();
        for (a, b) in busy {
            match merged.last_mut() {
                Some(last) if a <= last.1 + EPS => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        let mut j = 0;
        for w in 0..bars.len().saturating_sub(1) {
            let (bar_start, bar_end) = (bars[w], bars[w + 1]);
            while j < merged.len() && merged[j].1 <= bar_start + EPS {
                j += 1;
            }
            let mut t = bar_start;
            let mut k = j;
            while k < merged.len() && merged[k].0 < bar_end - EPS {
                let (a, b) = merged[k];
                if a > t + EPS {
                    for (at, value) in rest_values(t, a.min(bar_end), bar_start, bar_end) {
                        items.push(Item::Rest(RestItem { at, staff, value }));
                    }
                }
                t = t.max(b);
                k += 1;
            }
            if t < bar_end - EPS {
                for (at, value) in rest_values(t, bar_end, bar_start, bar_end) {
                    items.push(Item::Rest(RestItem { at, staff, value }));
                }
            }
        }
    }
    items.sort_by(|a, b| a.at().total_cmp(&b.at()));
    items
}

/// A number that changes with the notes, the key and the bars, so a layout is made again only
/// when one of them changed.
fn fingerprint(notes: &[Note], bars: &[f32], fifths: i8) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |w: u64| h = (h ^ w).wrapping_mul(0x0000_0100_0000_01b3);
    mix(fifths as u64);
    for b in bars {
        mix(u64::from(b.to_bits()));
    }
    for n in notes {
        mix(u64::from(n.start.to_bits()) << 32 | u64::from(n.len.to_bits()));
        mix(u64::from(n.pitch));
    }
    h
}

// ------------------------------------------------------------------------- the staves

/// The diatonic step of the bottom line of a staff.
fn bottom_step(staff: Staff) -> i32 {
    match staff {
        Staff::Treble => 37, // E4
        Staff::Bass => 25,   // G2
    }
}

/// The steps the sharps and flats of a key signature are written on, first to last.
pub fn signature_steps(fifths: i8, staff: Staff) -> Vec<i32> {
    // Treble sharps: F5 C5 G5 D5 A4 E5 B4. Flats: B4 E5 A4 D5 G4 C5 F4. The bass is two
    // octaves... a double step of seven lower.
    const SHARPS: [i32; 7] = [45, 42, 46, 43, 40, 44, 41];
    const FLATS: [i32; 7] = [41, 44, 40, 43, 39, 42, 38];
    let n = fifths.unsigned_abs().min(7) as usize;
    let base = if fifths >= 0 { &SHARPS } else { &FLATS };
    let drop = if staff == Staff::Bass { 14 } else { 0 };
    base[..n].iter().map(|s| s - drop).collect()
}

/// Where the staves are: the y of the bottom line of each.
#[derive(Clone, Copy, Debug)]
struct Staves {
    treble_bottom: f32,
    bass_bottom: f32,
}

impl Staves {
    fn y(&self, staff: Staff, step: i32) -> f32 {
        let bottom = match staff {
            Staff::Treble => self.treble_bottom,
            Staff::Bass => self.bass_bottom,
        };
        bottom - (step - bottom_step(staff)) as f32 * HALF
    }

    /// The staff and step a point is nearest to.
    fn step_at(&self, y: f32) -> (Staff, i32) {
        // Between the staves the lower one of the two owns the middle.
        let middle = (self.treble_bottom + self.bass_bottom - 4.0 * SPACE) / 2.0;
        let staff = if y < middle {
            Staff::Treble
        } else {
            Staff::Bass
        };
        let bottom = match staff {
            Staff::Treble => self.treble_bottom,
            Staff::Bass => self.bass_bottom,
        };
        (
            staff,
            bottom_step(staff) + ((bottom - y) / HALF).round() as i32,
        )
    }
}

/// What the editor remembers between frames.
pub struct ScoreView {
    pub beat_px: f32,
    scroll_y: f32,
    /// The layout of the notes on show, and what it was made from.
    cache: Option<(u64, Arc<Vec<Item>>)>,
    /// Where the systems were drawn last frame (clicks, tests).
    systems: Vec<System>,
}

impl Default for ScoreView {
    fn default() -> Self {
        Self {
            beat_px: 56.0,
            scroll_y: 0.0,
            cache: None,
            systems: Vec::new(),
        }
    }
}

/// One line of music (a system): a grand staff holding some whole bars.
#[derive(Clone, Debug)]
struct System {
    /// The y of the bottom lines of its staves.
    lay: Staves,
    /// Its bars (first, one past the last) in the bar list.
    bars: (usize, usize),
    /// Beats and where they are drawn: the bar lines and the start of every column of notes
    /// and rests. Positions in between are interpolated.
    anchors: Vec<(f32, f32)>,
    /// Its left and right ends (the staff lines).
    left: f32,
    right: f32,
}

impl System {
    fn top(&self) -> f32 {
        self.lay.treble_bottom - 4.0 * SPACE - SYS_TOP
    }

    fn bottom(&self) -> f32 {
        self.lay.bass_bottom + SYS_BOTTOM
    }

    fn beats(&self) -> (f32, f32) {
        (
            self.anchors.first().map_or(0.0, |a| a.0),
            self.anchors.last().map_or(0.0, |a| a.0),
        )
    }

    /// Where `beat` is drawn (clamped to the system's bars).
    fn x_of(&self, beat: f32) -> f32 {
        interpolate(&self.anchors, beat, |a| a.0, |a| a.1)
    }

    /// The beat drawn at `x`.
    fn beat_at(&self, x: f32) -> f32 {
        interpolate(&self.anchors, x, |a| a.1, |a| a.0)
    }
}

/// Piecewise-linear lookup in `pts` (sorted by `key`).
fn interpolate(
    pts: &[(f32, f32)],
    at: f32,
    key: impl Fn(&(f32, f32)) -> f32,
    val: impl Fn(&(f32, f32)) -> f32,
) -> f32 {
    let Some(first) = pts.first() else {
        return 0.0;
    };
    if at <= key(first) {
        return val(first);
    }
    for w in pts.windows(2) {
        let (k0, k1) = (key(&w[0]), key(&w[1]));
        if at <= k1 {
            if k1 - k0 <= EPS {
                return val(&w[1]);
            }
            let t = (at - k0) / (k1 - k0);
            return val(&w[0]) + (val(&w[1]) - val(&w[0])) * t;
        }
    }
    pts.last().map_or(0.0, val)
}

/// The start of every bar of the region and, last, its end, in beats from its start.
fn bar_lines(p: &Project, origin: f64, len: f64) -> Vec<f32> {
    let mut v: Vec<f32> = vec![0.0];
    v.extend(
        p.bars_in(origin, origin + len)
            .into_iter()
            .map(|b| (b.start - origin) as f32)
            .filter(|s| *s > EPS && f64::from(*s) < len - 1e-6),
    );
    v.push(len as f32);
    v
}

/// The room a system keeps above its treble staff (bar numbers, ledger lines) and below its
/// bass staff, and between its two staves.
const SYS_TOP: f32 = 40.0;
const SYS_BOTTOM: f32 = 34.0;
const STAFF_GAP: f32 = 66.0;
/// Room after a bar line before the first note, and the narrowest a column may be.
const BAR_PAD: f32 = 12.0;
const MIN_COL: f32 = 20.0;
/// Room an accidental takes in front of its note.
const ACC_W: f32 = 12.0;

/// The columns of a bar: each time something starts in it, with its offset from the bar line,
/// and the bar's natural width (time spaced, with room for every glyph).
fn bar_columns(items: &[Item], b0: f32, b1: f32, beat_px: f32) -> (Vec<(f32, f32)>, f32) {
    let lo = items.partition_point(|it| it.at() < b0 - EPS);
    let hi = items.partition_point(|it| it.at() < b1 - EPS);
    let here = &items[lo..hi.max(lo)];
    let mut times: Vec<f32> = Vec::new();
    for it in here {
        if times.last().is_none_or(|t| it.at() > t + EPS) {
            times.push(it.at());
        }
    }
    if times.first().is_none_or(|t| *t > b0 + EPS) {
        times.insert(0, b0);
    }
    let mut cols = Vec::new();
    let mut off = BAR_PAD;
    for (k, &t) in times.iter().enumerate() {
        let next = times.get(k + 1).copied().unwrap_or(b1);
        let at_t = here.iter().filter(|it| (it.at() - t).abs() < EPS);
        let mut accidentals = 0;
        let mut extra = 0.0f32;
        for it in at_t {
            if let Item::Note(n) = it {
                if n.accidental.is_some() {
                    accidentals += 1;
                }
                if dotted(n.value) {
                    extra = extra.max(6.0);
                }
                if n.value < 1.0 - EPS {
                    extra = extra.max(8.0);
                }
            }
        }
        off += ACC_W * f32::from(u8::from(accidentals > 0))
            + if accidentals > 1 { ACC_W * 0.7 } else { 0.0 };
        cols.push((t, off));
        off += ((next - t) * beat_px).max(MIN_COL + extra);
    }
    (cols, off + 4.0)
}

/// Draws the score of the (not generated) region `ri` of track `ti` in `area`: its menu row
/// (editor keys, key, zoom), then the music on a sheet set into a faceplate, as systems of
/// whole bars that fill the width, stacked down the page.
pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState, ti: usize, ri: usize, area: Rect) {
    use crate::piano_roll::{BRACKET_Y, KEY_H, KEY_Y, TOOLBAR_H};
    let g0 = ui.painter_at(area);
    hw::chassis(&g0, area);
    let (origin, len_beats) = {
        let r = &p.tracks[ti].regions[ri];
        (p.region_start_beats(r), p.region_len_beats(r))
    };
    // ---- the menu row: editor keys, the key, zoom
    let strip = Rect::from_min_size(area.min, vec2(area.width(), TOOLBAR_H.min(area.height())));
    crate::piano_roll::menu_row(&g0, strip);
    let top = strip.top();
    let (mode, right) = crate::piano_roll::editor_keys(ui, &g0, strip.left() + 10.0, top, 1, true);
    if let Some(i) = mode {
        s.score_tab = i == 1;
    }
    let mut key = p.key;
    let field = Rect::from_min_size(pos2(right + 18.0, top + KEY_Y + 1.0), vec2(150.0, 26.0));
    hw::bracket(
        &g0,
        field.left(),
        field.right(),
        top + BRACKET_Y,
        caption(tr("Key:")),
    );
    let resp = crate::widgets::popup_button(
        ui,
        &g0,
        field,
        egui::Id::unique("score_key"),
        &key_label(key),
    );
    egui::Popup::menu(&resp).show(|ui| {
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for (name, _) in KEYS {
                    if ui.selectable_label(key == name, key_label(name)).clicked() {
                        key = name;
                        ui.close();
                    }
                }
            });
    });
    if key != p.key {
        p.key = key;
        p.song_rev += 1;
    }
    // Zoom: how wide a beat is, which sets how many bars a system holds.
    {
        let (lo, hi) = (20f32.ln(), 200f32.ln());
        let cy = top + KEY_Y + KEY_H / 2.0;
        let hz = Rect::from_min_size(pos2(strip.right() - 132.0, cy - 7.0), vec2(120.0, 14.0));
        if hz.left() > field.right() + 60.0 {
            let t = ((s.score.beat_px.ln() - lo) / (hi - lo)).clamp(0.0, 1.0);
            if let Some(n) =
                crate::widgets::slider_h(ui, &g0, hz, egui::Id::unique("score_zoom"), t, true)
            {
                s.score.beat_px = (lo + n * (hi - lo)).exp();
            }
            let c = pos2(hz.left() - 14.0, cy);
            let st = Stroke::new(1.6, theme::INK2);
            g0.line_segment([c - vec2(7.0, 0.0), c + vec2(7.0, 0.0)], st);
            for (tip, d) in [(c - vec2(7.0, 0.0), 1.0), (c + vec2(7.0, 0.0), -1.0)] {
                g0.add(Shape::convex_polygon(
                    vec![tip, tip + vec2(4.0 * d, 3.5), tip + vec2(4.0 * d, -3.5)],
                    theme::INK2,
                    Stroke::NONE,
                ));
            }
            hw::bracket(
                &g0,
                hz.left() - 24.0,
                hz.right(),
                top + BRACKET_Y,
                tr("Zoom"),
            );
        }
    }
    let fifths = fifths(p.key);

    // ---- the faceplate, and the sheet in a recess on it
    let plate = Rect::from_min_max(
        pos2(area.left() + SHEET_PAD, strip.bottom() + SHEET_PAD),
        area.max - vec2(SHEET_PAD, SHEET_PAD),
    );
    if plate.width() < 40.0 || plate.height() < 40.0 {
        return;
    }
    hw::faceplate(&g0, plate, 4.0, false);
    for c in [
        plate.left_top() + vec2(9.0, 9.0),
        plate.right_top() + vec2(-9.0, 9.0),
        plate.left_bottom() + vec2(9.0, -9.0),
        plate.right_bottom() + vec2(-9.0, -9.0),
    ] {
        hw::screw(&g0, c, 3.5, 30.0);
    }
    let well = plate.shrink2(vec2(20.0, 12.0));
    hw::recess(&g0, well, 3.0);
    let sheet = well.shrink(4.0);
    g0.rect_filled(
        sheet.translate(vec2(0.0, 1.5)),
        2.0,
        theme::INK.gamma_multiply(0.12),
    );
    g0.rect_filled(sheet, 2.0, PAPER);
    let g = ui.painter_at(sheet);

    // A note just added sounds for a moment.
    if let Some((pitch, until)) = s.score_heard {
        if ui.input(|i| i.time) >= until {
            s.audition.push(crate::piano_roll::Audition::Off { pitch });
            s.score_heard = None;
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    // ---- notes and rests: laid out again only when something changed
    let bars = bar_lines(p, origin, len_beats);
    let print = fingerprint(&p.tracks[ti].regions[ri].notes, &bars, fifths);
    let items = match &s.score.cache {
        Some((seen, items)) if *seen == print => items.clone(),
        _ => {
            let items = Arc::new(layout(&p.tracks[ti].regions[ri].notes, &bars, fifths));
            s.score.cache = Some((print, items.clone()));
            items
        }
    };
    let v = &mut s.score;
    if ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pt| sheet.contains(pt))
    {
        let (d, m) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers));
        if m.command && d.y != 0.0 {
            v.beat_px = (v.beat_px * (1.0 + d.y * 0.003)).clamp(20.0, 200.0);
        } else {
            v.scroll_y -= d.y;
        }
    }
    let beat_px = v.beat_px;

    // ---- systems: whole bars packed to the width, then spread to fill it
    let sig_w = fifths.unsigned_abs() as f32 * 9.0;
    let clef_x = sheet.left() + 30.0;
    let music_left = clef_x + CLEF_W - 12.0;
    let lead_first = sig_w + 46.0;
    let lead_other = sig_w + 12.0;
    let nbars = bars.len().saturating_sub(1);
    let cols: Vec<(Vec<(f32, f32)>, f32)> = (0..nbars)
        .map(|w| bar_columns(&items, bars[w], bars[w + 1], beat_px))
        .collect();
    let sys_h = SYS_TOP + 4.0 * SPACE + STAFF_GAP + 4.0 * SPACE + SYS_BOTTOM;
    let mut systems: Vec<System> = Vec::new();
    let mut w = 0;
    while w < nbars {
        let lead = if systems.is_empty() {
            lead_first
        } else {
            lead_other
        };
        let room = (sheet.right() - 10.0 - music_left - lead).max(40.0);
        let mut end = w;
        let mut used = 0.0;
        while end < nbars && (end == w || used + cols[end].1 <= room) {
            used += cols[end].1;
            end += 1;
        }
        // Spread the bars over the width; the last system only when it is mostly full.
        let last = end == nbars;
        let scale = if last && used < room * 0.65 {
            1.0
        } else {
            room / used.max(1.0)
        };
        let top_y = sheet.top() + 8.0 + systems.len() as f32 * sys_h - v.scroll_y;
        let treble_bottom = top_y + SYS_TOP + 4.0 * SPACE;
        let lay = Staves {
            treble_bottom,
            bass_bottom: treble_bottom + STAFF_GAP + 4.0 * SPACE,
        };
        let mut anchors = Vec::new();
        let mut x = music_left + lead;
        for (b, (bar_cols, bw)) in cols.iter().enumerate().take(end).skip(w) {
            anchors.push((bars[b], x));
            for &(t, off) in bar_cols {
                if t > bars[b] + EPS {
                    anchors.push((t, x + off * scale));
                } else {
                    // The bar's first column: the bar line stays an anchor of its own.
                    anchors.push((t + EPS * 0.5, x + off * scale));
                }
            }
            x += bw * scale;
        }
        anchors.push((bars[end], x));
        systems.push(System {
            lay,
            bars: (w, end),
            anchors,
            left: clef_x - 8.0,
            right: if last && scale == 1.0 {
                x
            } else {
                sheet.right() - 10.0
            },
        });
        w = end;
    }
    let content_h = systems.len() as f32 * sys_h + 16.0;
    let max_scroll = (content_h - sheet.height()).max(0.0);
    v.scroll_y = v.scroll_y.clamp(0.0, max_scroll);

    // ---- drawing, system by system
    let (num, den) = p.signature_of_bar(p.beats_to_bar(origin).floor().max(0.0) as u32);
    let start_bar = p.beats_to_bar(origin).floor().max(0.0) as usize;
    let playhead = (p.position_beats - origin) as f32;
    let staff_st = Stroke::new(1.0, STAFF_INK);
    for (k, sys) in systems.iter().enumerate() {
        if sys.bottom() < sheet.top() || sys.top() > sheet.bottom() {
            continue;
        }
        let lay = sys.lay;
        let (t_top, b_bot) = (lay.treble_bottom - 4.0 * SPACE, lay.bass_bottom);
        for bottom in [lay.treble_bottom, lay.bass_bottom] {
            for line in 0..5 {
                let y = bottom - line as f32 * SPACE;
                g.hline(sys.left..=sys.right, y, staff_st);
            }
        }
        // The system's line and brace at its left.
        g.vline(sys.left, t_top..=b_bot, Stroke::new(2.0, INK));
        brace(&g, sys.left - 4.0, t_top, b_bot);
        treble_clef(&g, pos2(clef_x + 10.0, lay.y(Staff::Treble, 39)));
        bass_clef(&g, pos2(clef_x + 8.0, lay.y(Staff::Bass, 33)));
        for staff in [Staff::Treble, Staff::Bass] {
            for (i, step) in signature_steps(fifths, staff).into_iter().enumerate() {
                let at = pos2(music_left + 4.0 + i as f32 * 9.0, lay.y(staff, step));
                if fifths > 0 {
                    sharp(&g, at);
                } else {
                    flat(&g, at);
                }
            }
            if k == 0 {
                let tx = music_left + sig_w + 16.0;
                let bottom = match staff {
                    Staff::Treble => lay.treble_bottom,
                    Staff::Bass => lay.bass_bottom,
                };
                for (text, y) in [
                    (num.to_string(), bottom - 3.0 * SPACE),
                    (den.to_string(), bottom - SPACE),
                ] {
                    hw::text(
                        &g,
                        pos2(tx, y),
                        Align2::CENTER_CENTER,
                        &text,
                        Face::SilkX,
                        22.0,
                        INK,
                    );
                }
            }
        }
        // Bar lines through both staves, bar numbers over them; a double line at the end.
        for (b, &at) in bars
            .iter()
            .enumerate()
            .take(sys.bars.1 + 1)
            .skip(sys.bars.0)
        {
            let x = sys.x_of(at);
            let end = b == nbars;
            if b > sys.bars.0 || k > 0 {
                g.vline(x, t_top..=b_bot, Stroke::new(1.0, INK));
            }
            if end {
                g.vline(x - 4.0, t_top..=b_bot, Stroke::new(1.0, INK));
                g.vline(x + 0.5, t_top..=b_bot, Stroke::new(3.0, INK));
            }
            if b < sys.bars.1 {
                let bx = if b == sys.bars.0 { x - 4.0 } else { x };
                g.vline(
                    bx,
                    t_top - 18.0..=t_top - 12.0,
                    Stroke::new(1.0, theme::PRINT),
                );
                hw::text(
                    &g,
                    pos2(bx + 3.0, t_top - 15.0),
                    Align2::LEFT_CENTER,
                    &crate::piano_roll::bar_label((start_bar + b + 1) as u32),
                    Face::Lcd,
                    10.0,
                    theme::INK2,
                );
            }
        }
        // The notes and rests of its bars.
        let (b0, b1) = sys.beats();
        let lo = items.partition_point(|it| it.at() < b0 - EPS);
        let hi = items.partition_point(|it| it.at() < b1 - EPS);
        let here = &items[lo..hi.max(lo)];
        // Notes that start together on a staff make a chord with one stem.
        let mut chords: Vec<Vec<NoteItem>> = Vec::new();
        for it in here {
            match it {
                Item::Rest(r) => {
                    let x = if r.value >= 4.0 - EPS {
                        // A whole bar's rest sits in the middle of its bar.
                        let b = bars.partition_point(|b| *b <= r.at + EPS).saturating_sub(1);
                        (sys.x_of(bars[b]) + sys.x_of(bars[(b + 1).min(nbars)])) / 2.0
                    } else {
                        sys.x_of(r.at) + 4.0
                    };
                    draw_rest(&g, &lay, x, r);
                }
                Item::Note(n) => {
                    let same = chords
                        .iter_mut()
                        .rev()
                        .take_while(|c| (c[0].at - n.at).abs() < EPS)
                        .find(|c| c[0].staff == n.staff);
                    match same {
                        Some(c) => c.push(*n),
                        None => chords.push(vec![*n]),
                    }
                }
            }
        }
        for chord in &chords {
            let x = sys.x_of(chord[0].at) + 4.0;
            draw_chord(&g, &lay, x, chord);
            for c in chord {
                if let Some(next) = c.tie_to {
                    let x1 = if next < b1 - EPS {
                        sys.x_of(next) + 4.0
                    } else {
                        sys.right + 6.0
                    };
                    draw_tie(&g, &lay, x, x1, c);
                }
            }
        }
        if playhead >= b0 && playhead < b1 + EPS && f64::from(playhead) <= len_beats {
            let x = sys.x_of(playhead);
            let span = Rect::from_min_max(pos2(x, t_top - 10.0), pos2(x, b_bot + 12.0));
            crate::piano_roll::playhead(&g, &g, x, span, span.top() - 15.0);
        }
    }
    // A scroll bar when the systems run past the sheet.
    if max_scroll > 0.0 {
        let track = Rect::from_min_max(
            pos2(well.right() + 4.0, well.top()),
            pos2(well.right() + 14.0, well.bottom()),
        );
        let th = track.height() - 4.0;
        let thumb_h = (th * sheet.height() / content_h).clamp(24.0, th);
        let ty = track.top() + 2.0 + (th - thumb_h) * (v.scroll_y / max_scroll);
        let thumb = Rect::from_min_size(pos2(track.left() + 2.0, ty), vec2(6.0, thumb_h));
        let r = ui.interact(
            thumb,
            egui::Id::unique(("score_vbar", ti, ri)),
            Sense::drag(),
        );
        if r.dragged() {
            v.scroll_y += r.drag_delta().y * max_scroll / (th - thumb_h).max(1.0);
        }
        crate::piano_roll::paint_scrollbar(&g0, track, thumb, r.hovered() || r.dragged());
    }
    v.systems = systems;

    // ---- editing: click a staff to add a note, right-click a note to delete it
    let resp = ui
        .interact(sheet, egui::Id::unique(("score", ti, ri)), Sense::click())
        .on_hover_text(tr("Click to add a note, right-click a note to delete it"));
    let Some(pt) = resp.interact_pointer_pos() else {
        return;
    };
    let Some(sys) = s
        .score
        .systems
        .iter()
        .find(|sys| pt.y >= sys.top() && pt.y < sys.top() + sys_h)
        .cloned()
    else {
        return;
    };
    let lay = sys.lay;
    if resp.secondary_clicked() {
        let (b0, b1) = sys.beats();
        let hit = items.iter().find_map(|it| match it {
            Item::Note(n)
                if n.at >= b0 - EPS
                    && n.at < b1 - EPS
                    && (sys.x_of(n.at) + 4.0 - pt.x).abs() < 8.0
                    && (lay.y(n.staff, n.step) - pt.y).abs() < HALF + 1.0 =>
            {
                Some(n.note)
            }
            _ => None,
        });
        if let Some(note) = hit {
            p.tracks[ti].regions[ri].notes.remove(note);
            p.song_rev += 1;
        }
    } else if resp.clicked() && pt.x <= sys.right {
        let beat = sys.beat_at(pt.x - 4.0);
        if beat < 0.0 || f64::from(beat) >= len_beats {
            return;
        }
        let (staff, step) = lay.step_at(pt.y);
        let pitch = pitch_in_key(step, fifths);
        // On the staff it was clicked on, not past the other one's middle C.
        if staff_of(pitch) == staff || pitch == 60 {
            let start = (beat * 2.0).round() / 2.0;
            p.tracks[ti].regions[ri].notes.push(Note {
                lyric: crate::lyrics::Lyric::NONE,
                mute: false,
                start,
                len: 1.0,
                pitch,
                vel: 100,
            });
            p.song_rev += 1;
            s.audition.push(crate::piano_roll::Audition::On {
                pitch,
                vel: 100,
                track: ti.min(usize::from(u16::MAX)) as u16,
            });
            s.score_heard = Some((pitch, ui.input(|i| i.time) + 0.3));
        }
    }
}

/// Where a note at `beat` on `step` of `staff` was drawn last frame (tests).
#[cfg(test)]
fn point_of(v: &ScoreView, beat: f32, staff: Staff, step: i32) -> Option<Pos2> {
    let sys = v.systems.iter().find(|s| {
        let (a, b) = s.beats();
        beat >= a - EPS && beat < b
    })?;
    Some(pos2(sys.x_of(beat) + 4.0, sys.lay.y(staff, step)))
}

// ------------------------------------------------------------------------- drawing

/// A notehead at `c`: an oval tilted up to the right, filled, or hollow for half and whole
/// notes.
fn notehead(g: &egui::Painter, c: Pos2, value: f32) {
    let oval = |rx: f32, ry: f32, tilt: f32| -> Vec<Pos2> {
        let (sn, cs) = tilt.to_radians().sin_cos();
        (0..24)
            .map(|k| {
                let a = k as f32 / 24.0 * std::f32::consts::TAU;
                let (x, y) = (rx * a.cos(), ry * a.sin());
                c + vec2(x * cs - y * sn, x * sn + y * cs)
            })
            .collect()
    };
    if value >= 4.0 - EPS {
        g.add(Shape::convex_polygon(
            oval(6.6, 4.4, 0.0),
            INK,
            Stroke::NONE,
        ));
        g.add(Shape::convex_polygon(
            oval(2.6, 3.6, -35.0),
            PAPER,
            Stroke::NONE,
        ));
    } else if value >= 2.0 - EPS {
        g.add(Shape::convex_polygon(
            oval(5.8, 4.1, -22.0),
            INK,
            Stroke::NONE,
        ));
        g.add(Shape::convex_polygon(
            oval(4.6, 1.9, -22.0),
            PAPER,
            Stroke::NONE,
        ));
    } else {
        g.add(Shape::convex_polygon(
            oval(5.8, 4.1, -22.0),
            INK,
            Stroke::NONE,
        ));
    }
}

/// The notes of one staff that start together: their heads (a second's upper head on the
/// other side of the stem), accidentals staggered so they do not collide, dots, ledger lines,
/// and one stem with the flags of the shortest value.
fn draw_chord(g: &egui::Painter, lay: &Staves, x: f32, chord: &[NoteItem]) {
    let Some(first) = chord.first() else { return };
    let staff = first.staff;
    let bottom = bottom_step(staff);
    let mut steps: Vec<i32> = chord.iter().map(|n| n.step).collect();
    steps.sort_unstable();
    let (low, high) = (steps[0], steps[steps.len() - 1]);
    // The stem goes up when the notes sit mostly below the middle line.
    let up = (low + high) < 2 * (bottom + 4);
    let head_w = 5.6;
    // Ledger lines for notes above or below the staff.
    let ledger = |step: i32| {
        let ly = lay.y(staff, step);
        g.hline(x - 9.0..=x + 9.0, ly, Stroke::new(1.2, STAFF_INK));
    };
    let hi_line = bottom + 8;
    if low < bottom {
        for step in (low..bottom).filter(|st| (bottom - st) % 2 == 0) {
            ledger(step);
        }
    }
    if high > hi_line {
        for step in ((hi_line + 1)..=high).filter(|st| (st - hi_line) % 2 == 0) {
            ledger(step);
        }
    }
    // Heads: a note a step from the one before it goes to the other side of the stem.
    let mut ordered: Vec<NoteItem> = chord.to_vec();
    ordered.sort_by_key(|n| n.step);
    let mut flipped = vec![false; ordered.len()];
    for k in 1..ordered.len() {
        if ordered[k].step - ordered[k - 1].step == 1 && !flipped[k - 1] {
            flipped[k] = true;
        }
    }
    let shift = if up {
        2.0 * head_w - 0.6
    } else {
        -(2.0 * head_w - 0.6)
    };
    let mut acc_k = 0;
    for (k, n) in ordered.iter().enumerate().rev() {
        let y = lay.y(staff, n.step);
        let hx = if flipped[k] { x + shift } else { x };
        notehead(g, pos2(hx, y), n.value);
        if dotted(n.value) {
            // A dot sits in a space: a note on a line puts it in the space above.
            let on_line = (n.step - bottom) % 2 == 0;
            let dy = if on_line { -HALF } else { 0.0 };
            let dx = if flipped.iter().any(|f| *f) && up {
                shift
            } else {
                0.0
            };
            g.circle_filled(pos2(x + dx + 10.0, y + dy), 1.7, INK);
        }
        if let Some(a) = n.accidental {
            let ax = x - head_w - 7.0 - (acc_k % 2) as f32 * 8.0;
            acc_k += 1;
            match a {
                Accidental::Sharp => sharp(g, pos2(ax, y)),
                Accidental::Flat => flat(g, pos2(ax + 1.0, y)),
                Accidental::Natural => natural(g, pos2(ax + 1.0, y)),
            }
        }
    }
    let value = chord.iter().map(|n| n.value).fold(f32::INFINITY, f32::min);
    if value >= 4.0 - EPS {
        return;
    }
    let (sx, y0, y1) = if up {
        let y = lay.y(staff, low);
        (x + head_w - 0.6, y - 1.0, lay.y(staff, high) - 3.5 * SPACE)
    } else {
        let y = lay.y(staff, high);
        (x - head_w + 0.6, y + 1.0, lay.y(staff, low) + 3.5 * SPACE)
    };
    g.line_segment([pos2(sx, y0), pos2(sx, y1)], Stroke::new(1.3, INK));
    let flags = if value <= 0.375 {
        2
    } else if value <= 0.75 {
        1
    } else {
        0
    };
    for f in 0..flags {
        let d = f as f32 * 6.5;
        let dir = if up { 1.0 } else { -1.0 };
        let a = pos2(sx, y1 + d * dir);
        curve(
            g,
            [
                a,
                a + vec2(1.0, 5.0 * dir),
                a + vec2(8.5, 7.0 * dir),
                a + vec2(6.5, 16.0 * dir),
            ],
            Stroke::new(1.8, INK),
        );
    }
}

fn dotted(value: f32) -> bool {
    [0.75, 1.5, 3.0].iter().any(|d| (value - d).abs() < 1e-3)
}

/// A tie from the note at `x0` to the next piece at `x1`, curved away from the stem: under the
/// noteheads when the stem goes up, over them when it goes down.
fn draw_tie(g: &egui::Painter, lay: &Staves, x0: f32, x1: f32, n: &NoteItem) {
    let y = lay.y(n.staff, n.step);
    let below = n.step < bottom_step(n.staff) + 4;
    let dir = if below { 1.0 } else { -1.0 };
    let (a, b) = (pos2(x0 + 4.0, y + 5.0 * dir), pos2(x1 - 4.0, y + 5.0 * dir));
    if b.x <= a.x + 2.0 {
        return;
    }
    let sag = dir * (4.0 + (b.x - a.x).min(200.0) * 0.02);
    // A crescent: two curves, thick in the middle, drawn as one filled band.
    let pts = |bulge: f32| -> Vec<Pos2> {
        (0..=16)
            .map(|k| {
                let t = k as f32 / 16.0;
                let s = 4.0 * t * (1.0 - t);
                pos2(
                    a.x + (b.x - a.x) * t,
                    a.y + (b.y - a.y) * t + sag * s * bulge,
                )
            })
            .collect()
    };
    let (outer, inner) = (pts(1.0), pts(0.72));
    for k in 0..16 {
        let mut mesh = egui::epaint::Mesh::default();
        mesh.colored_vertex(outer[k], INK);
        mesh.colored_vertex(outer[k + 1], INK);
        mesh.colored_vertex(inner[k + 1], INK);
        mesh.colored_vertex(inner[k], INK);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        g.add(Shape::mesh(mesh));
    }
    g.add(Shape::line(outer, Stroke::new(0.8, INK)));
}

fn draw_rest(g: &egui::Painter, lay: &Staves, x: f32, r: &RestItem) {
    let bottom = bottom_step(r.staff);
    let line = |k: i32| lay.y(r.staff, bottom + 2 * k); // line 0 (bottom) .. 4 (top)
    let v = r.value;
    let p = |cx: f32, cy: f32, dx: f32, dy: f32| pos2(cx + dx * SPACE, cy + dy * SPACE);
    if v >= 4.0 - 1e-3 {
        // A whole rest hangs from the fourth line.
        g.rect_filled(
            Rect::from_min_size(pos2(x - 6.0, line(3)), vec2(12.0, HALF + 0.5)),
            0.0,
            INK,
        );
    } else if v >= 2.0 - 1e-3 {
        // A half rest sits on the middle line.
        g.rect_filled(
            Rect::from_min_size(pos2(x - 6.0, line(2) - HALF - 0.5), vec2(12.0, HALF + 0.5)),
            0.0,
            INK,
        );
    } else if v >= 1.0 - 1e-3 {
        // A quarter rest: a thin stroke down, a thick one back, a thin one down and the hook.
        let c = line(2);
        let thin = Stroke::new(1.4, INK);
        g.line_segment([p(x, c, -0.3, -1.5), p(x, c, 0.35, -0.65)], thin);
        g.line_segment(
            [p(x, c, 0.35, -0.65), p(x, c, -0.25, 0.1)],
            Stroke::new(3.4, INK),
        );
        g.line_segment([p(x, c, -0.25, 0.1), p(x, c, 0.35, 0.8)], thin);
        curve(
            g,
            [
                p(x, c, 0.35, 0.8),
                p(x, c, -0.35, 0.45),
                p(x, c, -0.65, 1.05),
                p(x, c, -0.05, 1.55),
            ],
            Stroke::new(1.8, INK),
        );
    } else {
        // Eighth and sixteenth rests: a slanted stem with a flag and a ball for each.
        let flags = if v <= 0.25 + 1e-3 { 2 } else { 1 };
        let c = line(3) - HALF;
        let top = p(x, c, 0.45, -0.55);
        let foot = p(x, c, -0.15, 1.1 + (flags - 1) as f32);
        g.line_segment([top, foot], Stroke::new(1.4, INK));
        for f in 0..flags {
            let fy = f as f32;
            let tip = p(x, c, 0.45 - 0.22 * fy, -0.55 + fy);
            let ball = p(x, c, -0.35 - 0.22 * fy, -0.42 + fy);
            curve(
                g,
                [
                    ball + vec2(0.0, 1.5),
                    ball + vec2(2.0, 4.0),
                    tip + vec2(-3.0, 2.5),
                    tip,
                ],
                Stroke::new(1.4, INK),
            );
            g.circle_filled(ball, 2.1, INK);
        }
    }
}

/// A brace joining the two staves of a system, its point at the middle, left of `x`.
fn brace(g: &egui::Painter, x: f32, top: f32, bottom: f32) {
    let h = bottom - top;
    let n = 40;
    let at = |t: f32| -> Pos2 {
        // Out to the left in each half, back in at the ends and to a point at the middle.
        let u = if t < 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
        let bow = (u * std::f32::consts::PI).sin();
        let tip = (1.0 - u).powi(6);
        pos2(x - 5.0 - bow * 3.0 - tip * 6.0, top + h * t)
    };
    for k in 0..n {
        let (t0, t1) = (k as f32 / n as f32, (k + 1) as f32 / n as f32);
        let u = if t0 < 0.5 { t0 * 2.0 } else { (1.0 - t0) * 2.0 };
        let w = 1.2 + 4.8 * (u * std::f32::consts::PI).sin();
        g.line_segment([at(t0), at(t1)], Stroke::new(w, INK));
    }
}

/// A sharp sign centred at `c`.
fn sharp(g: &egui::Painter, c: Pos2) {
    let stroke = Stroke::new(1.1, INK);
    for dx in [-1.8, 1.8] {
        g.line_segment([c + vec2(dx, -6.0), c + vec2(dx, 6.0)], stroke);
    }
    for dy in [-1.8, 1.8] {
        g.line_segment(
            [c + vec2(-4.0, dy + 1.0), c + vec2(4.0, dy - 1.0)],
            Stroke::new(2.0, INK),
        );
    }
}

/// A flat sign centred at `c`: a stem and a belly.
fn flat(g: &egui::Painter, c: Pos2) {
    let stroke = Stroke::new(1.2, INK);
    g.line_segment([c + vec2(-2.0, -7.0), c + vec2(-2.0, 3.5)], stroke);
    g.add(Shape::CubicBezier(CubicBezierShape::from_points_stroke(
        [
            c + vec2(-2.0, 3.5),
            c + vec2(5.5, 0.5),
            c + vec2(4.5, -4.5),
            c + vec2(-2.0, -1.0),
        ],
        false,
        Color32::TRANSPARENT,
        stroke,
    )));
}

/// A natural sign centred at `c`.
fn natural(g: &egui::Painter, c: Pos2) {
    let stroke = Stroke::new(1.1, INK);
    g.line_segment([c + vec2(-2.5, -7.0), c + vec2(-2.5, 3.0)], stroke);
    g.line_segment([c + vec2(2.5, -3.0), c + vec2(2.5, 7.0)], stroke);
    g.line_segment(
        [c + vec2(-2.5, 3.0), c + vec2(2.5, 1.5)],
        Stroke::new(2.0, INK),
    );
    g.line_segment(
        [c + vec2(-2.5, -1.5), c + vec2(2.5, -3.0)],
        Stroke::new(2.0, INK),
    );
}

fn curve(g: &egui::Painter, pts: [Pos2; 4], stroke: Stroke) {
    g.add(Shape::CubicBezier(CubicBezierShape::from_points_stroke(
        pts,
        false,
        Color32::TRANSPARENT,
        stroke,
    )));
}

/// The treble clef, curled round the G line, which is where `at` is.
fn treble_clef(g: &egui::Painter, at: Pos2) {
    let stroke = Stroke::new(1.8, INK);
    let p = |x: f32, y: f32| at + vec2(x, y) * SPACE;
    // The tail below the staff, the long stroke up to the top, the hump, the big loop round
    // the G line and the small curl inside it.
    curve(
        g,
        [p(-0.9, 3.0), p(-0.5, 3.9), p(0.5, 3.8), p(0.4, 2.6)],
        stroke,
    );
    curve(
        g,
        [p(0.4, 2.6), p(0.2, 0.0), p(-0.4, -1.8), p(0.1, -3.4)],
        stroke,
    );
    curve(
        g,
        [p(0.1, -3.4), p(0.6, -5.0), p(1.4, -4.4), p(1.0, -3.0)],
        stroke,
    );
    curve(
        g,
        [p(1.0, -3.0), p(0.4, -1.2), p(-1.6, -0.2), p(-1.4, 1.3)],
        stroke,
    );
    curve(
        g,
        [p(-1.4, 1.3), p(-1.0, 2.6), p(1.6, 2.4), p(1.3, 0.6)],
        stroke,
    );
    curve(
        g,
        [p(1.3, 0.6), p(1.0, -0.6), p(-0.5, -0.7), p(-0.3, 0.3)],
        stroke,
    );
    g.circle_filled(p(-0.9, 3.0), 1.8, INK);
}

/// The bass clef, with its dot on the F line, which is where `at` is.
fn bass_clef(g: &egui::Painter, at: Pos2) {
    let stroke = Stroke::new(1.9, INK);
    let p = |x: f32, y: f32| at + vec2(x, y) * SPACE;
    g.circle_filled(p(-0.7, 0.0), 0.62 * SPACE, INK);
    curve(
        g,
        [p(-0.7, 0.0), p(0.0, -1.9), p(2.0, -1.4), p(1.8, 0.5)],
        stroke,
    );
    curve(
        g,
        [p(1.8, 0.5), p(1.5, 2.3), p(0.4, 3.0), p(-1.0, 3.8)],
        stroke,
    );
    for dy in [-0.65, 0.65] {
        g.circle_filled(p(2.45, dy), 0.28 * SPACE, INK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(start: f32, len: f32, pitch: u8) -> Note {
        Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start,
            len,
            pitch,
            vel: 90,
        }
    }

    fn notes_of(items: &[Item]) -> Vec<NoteItem> {
        items
            .iter()
            .filter_map(|i| match i {
                Item::Note(n) => Some(*n),
                _ => None,
            })
            .collect()
    }

    fn rests_of(items: &[Item], staff: Staff) -> Vec<(f32, f32)> {
        items
            .iter()
            .filter_map(|i| match i {
                Item::Rest(r) if r.staff == staff => Some((r.at, r.value)),
                _ => None,
            })
            .collect()
    }

    fn marks(items: &[Item]) -> Vec<Option<Accidental>> {
        notes_of(items).iter().map(|i| i.accidental).collect()
    }

    const FOUR_BARS: [f32; 5] = [0.0, 4.0, 8.0, 12.0, 16.0];

    #[test]
    fn pitches_sit_on_the_staff_where_music_writes_them() {
        assert_eq!(diatonic(60), (35, false));
        assert_eq!(diatonic(61), (35, true), "C sharp is a C with a sharp");
        assert_eq!(diatonic(62), (36, false));
        assert_eq!(diatonic(64), (37, false), "E4: bottom line of the treble");
        assert_eq!(
            diatonic(67),
            (39, false),
            "G4: the line the clef curls round"
        );
        assert_eq!(
            diatonic(53),
            (31, false),
            "F3: the F line of the bass staff"
        );
        assert_eq!(diatonic(43), (25, false), "G2: bottom line of the bass");
        for p in 0..128u8 {
            let (step, sharp) = diatonic(p);
            assert_eq!(pitch_of(step), if sharp { p - 1 } else { p }, "{p}");
        }
        assert_eq!((staff_of(60), staff_of(59)), (Staff::Treble, Staff::Bass));
    }

    #[test]
    fn notes_are_spelled_in_their_key() {
        let letter_alter = |p, f| {
            let s = spell(p, f);
            (s.step.rem_euclid(7), s.alter)
        };
        // Letters: C 0, D 1, E 2, F 3, G 4, A 5, B 6.
        assert_eq!(letter_alter(66, 1), (3, 1), "F sharp in G major");
        assert_eq!(letter_alter(65, 1), (3, 0), "F natural in G major");
        assert_eq!(letter_alter(70, -1), (6, -1), "B flat in F major");
        assert_eq!(letter_alter(70, 1), (5, 1), "A sharp in G major");
        assert_eq!(letter_alter(71, -7), (0, -1), "C flat in C flat major");
        // Every pitch reads back as itself in every key.
        for (_, f) in KEYS {
            for pitch in 12..=115u8 {
                let s = spell(pitch, f);
                let alt = alterations(f)[s.step.rem_euclid(7) as usize];
                let back = i32::from(pitch_in_key(s.step, f)) + i32::from(s.alter - alt);
                assert_eq!(back, i32::from(pitch), "{pitch} in {f}: {s:?}");
            }
        }
        assert_eq!(fifths("Ebmaj"), -3);
        assert_eq!(fifths("Amin"), 0);
        assert_eq!(fifths("nonsense"), 0);
        assert_eq!(key_label("Cmaj"), "C major");
        assert_eq!(key_label("F#min"), "F# minor");
    }

    #[test]
    fn key_signatures_alter_the_right_letters_and_sit_on_the_right_lines() {
        assert_eq!(alterations(0), [0; 7]);
        // C D E F G A B: G major sharpens F; D major F and C; F major flattens B.
        assert_eq!(alterations(1), [0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(alterations(2), [1, 0, 0, 1, 0, 0, 0]);
        assert_eq!(alterations(-1), [0, 0, 0, 0, 0, 0, -1]);
        assert_eq!(alterations(7), [1; 7]);
        assert_eq!(alterations(-7), [-1; 7]);
        // The treble staff's sharps: F5 C5 G5; its flats: B4 E5 A4. The bass is 14 lower.
        assert_eq!(signature_steps(3, Staff::Treble), [45, 42, 46]);
        assert_eq!(signature_steps(-3, Staff::Treble), [41, 44, 40]);
        assert_eq!(signature_steps(3, Staff::Bass), [31, 28, 32]);
        assert!(signature_steps(0, Staff::Treble).is_empty());
        // None of them leaves the staff by more than a space.
        for f in -7..=7i8 {
            for staff in [Staff::Treble, Staff::Bass] {
                let b = bottom_step(staff);
                assert!(
                    signature_steps(f, staff)
                        .iter()
                        .all(|s| (b - 1..=b + 9).contains(s)),
                    "{f} {staff:?}"
                );
            }
        }
        // Every key the editor offers is one the project file keeps by name.
        for (k, _) in KEYS {
            assert!(
                crate::project_io::builtin_name(k) == Some(k),
                "{k} must round-trip through a saved project"
            );
        }
    }

    #[test]
    fn an_accidental_holds_to_the_end_of_the_bar_and_is_taken_back() {
        // C major: C sharp twice (the second needs no mark), then C natural, then in the next
        // bar C sharp again (written again).
        let notes = [
            n(0.0, 1.0, 61),
            n(1.0, 1.0, 61),
            n(2.0, 1.0, 60),
            n(4.0, 1.0, 61),
        ];
        assert_eq!(
            marks(&layout(&notes, &FOUR_BARS, 0)),
            [
                Some(Accidental::Sharp),
                None,
                Some(Accidental::Natural),
                Some(Accidental::Sharp)
            ]
        );
        // In G major the F sharp of the signature needs no mark, an F natural does.
        let notes = [n(0.0, 1.0, 66), n(1.0, 1.0, 65), n(2.0, 1.0, 66)];
        assert_eq!(
            marks(&layout(&notes, &FOUR_BARS, 1)),
            [None, Some(Accidental::Natural), Some(Accidental::Sharp)]
        );
        // In F major a B flat needs none, and a B natural does.
        let notes = [n(0.0, 1.0, 70), n(1.0, 1.0, 71)];
        assert_eq!(
            marks(&layout(&notes, &FOUR_BARS, -1)),
            [None, Some(Accidental::Natural)]
        );
    }

    #[test]
    fn a_note_across_a_bar_line_is_tied_and_odd_lengths_are_split() {
        // A half note starting on the last beat of bar 1: a quarter, tied to a quarter.
        let items = notes_of(&layout(&[n(3.0, 2.0, 64)], &FOUR_BARS, 0));
        assert_eq!(items.len(), 2);
        assert_eq!(
            (items[0].at, items[0].value, items[0].tie_to),
            (3.0, 1.0, Some(4.0))
        );
        assert_eq!(
            (items[1].at, items[1].value, items[1].tie_to),
            (4.0, 1.0, None)
        );
        assert!(items.iter().all(|i| i.note == 0));
        assert!(items[1].accidental.is_none());
        // Five beats from the start of a bar: a whole note tied to a quarter.
        let items = notes_of(&layout(&[n(4.0, 5.0, 64)], &FOUR_BARS, 0));
        assert_eq!(
            items.iter().map(|i| (i.at, i.value)).collect::<Vec<_>>(),
            [(4.0, 4.0), (8.0, 1.0)]
        );
        // A length that is not a note value: 1.25 beats = a quarter and a sixteenth.
        let items = notes_of(&layout(&[n(0.0, 1.25, 64)], &FOUR_BARS, 0));
        assert_eq!(
            items.iter().map(|i| i.value).collect::<Vec<_>>(),
            [1.0, 0.25]
        );
        // A dotted half stays one note.
        let items = notes_of(&layout(&[n(0.0, 3.0, 64)], &FOUR_BARS, 0));
        assert_eq!(items.len(), 1);
        assert!(dotted(items[0].value));
        // A note that runs past the end of the region is cut there.
        let items = notes_of(&layout(&[n(15.0, 9.0, 64)], &FOUR_BARS, 0));
        assert_eq!(items.iter().map(|i| i.value).sum::<f32>(), 1.0);
        // Start and length are read to sixteenths.
        let items = notes_of(&layout(&[n(0.51, 0.49, 64)], &FOUR_BARS, 0));
        assert_eq!((items[0].at, items[0].value), (0.5, 0.5));
    }

    #[test]
    fn what_a_staff_does_not_play_is_rests() {
        // One treble quarter note at the start of bar 1: the treble staff rests the other
        // three beats (on the beats) and then whole bars; the bass rests in every bar.
        let items = layout(&[n(0.0, 1.0, 72)], &FOUR_BARS, 0);
        assert_eq!(
            rests_of(&items, Staff::Treble),
            [(1.0, 1.0), (2.0, 2.0), (4.0, 4.0), (8.0, 4.0), (12.0, 4.0)]
        );
        assert_eq!(
            rests_of(&items, Staff::Bass),
            [(0.0, 4.0), (4.0, 4.0), (8.0, 4.0), (12.0, 4.0)]
        );
        // A rest starts where it can be counted: the gap from beat 0.5 to 2.
        let items = layout(&[n(0.0, 0.5, 72), n(2.0, 2.0, 72)], &FOUR_BARS, 0);
        assert_eq!(
            rests_of(&items, Staff::Treble)[..2],
            [(0.5, 0.5), (1.0, 1.0)]
        );
        // The notes and rests of a staff add up to its bar.
        for bar in 0..4 {
            let (a, b) = (FOUR_BARS[bar], FOUR_BARS[bar + 1]);
            let mut time = 0.0;
            for it in &items {
                match it {
                    Item::Note(n) if n.staff == Staff::Treble && n.at >= a && n.at < b => {
                        time += n.value
                    }
                    Item::Rest(r) if r.staff == Staff::Treble && r.at >= a && r.at < b => {
                        time += r.value.min(b - r.at)
                    }
                    _ => {}
                }
            }
            assert!((time - 4.0).abs() < 1e-4, "bar {bar}: {time}");
        }
        // Without notes, a whole rest in each bar, on both staves.
        let items = layout(&[], &FOUR_BARS, 0);
        assert_eq!(rests_of(&items, Staff::Treble).len(), 4);
        assert_eq!(rests_of(&items, Staff::Bass).len(), 4);
        // A bar of 3/4 gets one whole rest too.
        let items = layout(&[], &[0.0, 3.0, 6.0], 0);
        assert_eq!(rests_of(&items, Staff::Treble), [(0.0, 4.0), (3.0, 4.0)]);
    }

    #[test]
    fn odd_input_does_not_break_the_layout() {
        let wild = [
            n(f32::NAN, 1.0, 60),
            n(0.0, f32::INFINITY, 60),
            n(-5.0, 1.0, 60),
            n(1.0e9, 1.0, 60),
            n(0.0, 0.0, 255),
        ];
        assert!(!layout(&wild, &FOUR_BARS, 7).is_empty());
        assert!(layout(&wild, &[], 0).is_empty());
        assert!(layout(&[], &[0.0], 0).is_empty());
        // Many notes lay out quickly.
        let many: Vec<Note> = (0..50_000)
            .map(|i| n((i % 4000) as f32 * 0.25, 0.25, 40 + (i % 50) as u8))
            .collect();
        let bars: Vec<f32> = (0..=250).map(|b| b as f32 * 4.0).collect();
        let began = std::time::Instant::now();
        layout(&many, &bars, 2);
        assert!(began.elapsed().as_secs() < 10, "{:?}", began.elapsed());
    }

    #[test]
    fn durations_are_shown_as_the_nearest_note_value() {
        assert_eq!(display_duration(1.0), 1.0);
        assert_eq!(display_duration(0.9), 1.0);
        assert_eq!(display_duration(0.3), 0.25);
        assert_eq!(display_duration(1.4), 1.5);
        assert_eq!(display_duration(2.4), 2.0);
        assert_eq!(display_duration(9.0), 4.0);
        assert_eq!(display_duration(0.01), 0.25);
    }

    #[test]
    fn a_click_finds_the_line_or_space_and_the_staff() {
        let lay = Staves {
            treble_bottom: 100.0,
            bass_bottom: 200.0,
        };
        assert_eq!(lay.step_at(100.0), (Staff::Treble, 37));
        assert_eq!(lay.step_at(100.0 - HALF * 0.9), (Staff::Treble, 38));
        for staff in [Staff::Treble, Staff::Bass] {
            for step in (bottom_step(staff) - 3)..(bottom_step(staff) + 12) {
                assert_eq!(
                    lay.step_at(lay.y(staff, step)),
                    (staff, step),
                    "{staff:?} {step}"
                );
            }
        }
        assert_eq!(lay.y(Staff::Treble, 35), 100.0 + 2.0 * HALF);
        assert_eq!(lay.y(Staff::Bass, 35), 200.0 - 10.0 * HALF);
    }

    #[test]
    fn clicking_the_staff_adds_a_note_in_the_key_and_right_clicking_it_deletes_it() {
        use crate::ui_test::Harness;
        let mut p = Project::demo();
        let mut s = UiState::new();
        let (ti, ri) = (0, 0);
        p.key = "Gmaj"; // F sharp
        let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 420.0));
        let mut h = Harness::new(1000.0, 420.0);
        h.settle(|ui| show(ui, &mut p, &mut s, ti, ri, body));
        // Beat 2 on the top line of the treble staff: F5, an F sharp in G major.
        let at = point_of(&s.score, 2.0, Staff::Treble, 45).unwrap();
        let has = |p: &Project| {
            p.tracks[ti].regions[ri]
                .notes
                .iter()
                .any(|n| n.pitch == 78 && n.start == 2.0 && n.vel == 100 && n.len == 1.0)
        };
        let before = p.tracks[ti].regions[ri].notes.len();
        let rev = p.song_rev;
        assert!(!has(&p));
        h.click(at, |ui| show(ui, &mut p, &mut s, ti, ri, body));
        assert!(has(&p), "an F sharp (the key's), not an F natural");
        assert_eq!(p.tracks[ti].regions[ri].notes.len(), before + 1);
        assert!(p.song_rev > rev, "and the engine is told");
        h.right_click(at, |ui| show(ui, &mut p, &mut s, ti, ri, body));
        assert!(!has(&p), "right-click deleted it");
        assert_eq!(p.tracks[ti].regions[ri].notes.len(), before);
        // A click past the end of the region adds nothing.
        let far = pos2(990.0, at.y);
        h.click(far, |ui| show(ui, &mut p, &mut s, ti, ri, body));
        assert_eq!(p.tracks[ti].regions[ri].notes.len(), before);
        // The note sounded for a moment and is let go of.
        for _ in 0..30 {
            h.frame(vec![], |ui| show(ui, &mut p, &mut s, ti, ri, body));
            std::thread::sleep(std::time::Duration::from_millis(12));
        }
        assert!(s
            .audition
            .iter()
            .any(|a| matches!(a, crate::piano_roll::Audition::Off { pitch: 78 })));
    }

    #[test]
    fn the_score_draws_in_every_key_at_any_size_with_extreme_notes() {
        use crate::ui_test::Harness;
        let mut p = Project::demo();
        let mut s = UiState::new();
        let extremes = [0u8, 12, 21, 59, 60, 61, 108, 127];
        for (i, pitch) in extremes.into_iter().enumerate() {
            p.tracks[0].regions[0].notes.push(Note {
                lyric: crate::lyrics::Lyric::NONE,
                mute: false,
                start: i as f32 * 0.25,
                len: [0.01, 0.3, 1.0, 1.4, 2.4, 3.0, 9.0, 0.75][i],
                pitch,
                vel: 100,
            });
        }
        let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 420.0));
        let mut hn = Harness::new(1000.0, 420.0);
        for (key, _) in KEYS {
            p.key = key;
            for _ in 0..2 {
                hn.frame(vec![], |ui| show(ui, &mut p, &mut s, 0, 0, body));
            }
        }
        p.key = "Ebmaj";
        for (w, h) in [(1000.0, 420.0), (400.0, 150.0), (100.0, 60.0)] {
            let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
            let mut hn = Harness::new(w, h);
            for _ in 0..3 {
                hn.frame(vec![], |ui| show(ui, &mut p, &mut s, 0, 0, body));
            }
        }
        // A signature change inside the region, and a region of a fraction of a bar.
        p.set_signature_from(3, (3, 4));
        let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 400.0));
        let mut hn = Harness::new(800.0, 400.0);
        hn.frame(vec![], |ui| show(ui, &mut p, &mut s, 0, 0, body));
        p.tracks[0].regions[0].len_bars = 0.3;
        hn.frame(vec![], |ui| show(ui, &mut p, &mut s, 0, 0, body));
    }

    #[test]
    fn the_layout_is_kept_until_the_notes_the_key_or_the_bars_change() {
        let notes = [n(0.0, 1.0, 60), n(1.0, 1.0, 62)];
        let a = fingerprint(&notes, &FOUR_BARS, 0);
        assert_eq!(a, fingerprint(&notes, &FOUR_BARS, 0));
        assert_ne!(a, fingerprint(&notes, &FOUR_BARS, 1));
        assert_ne!(a, fingerprint(&notes, &[0.0, 3.0, 6.0], 0));
        assert_ne!(
            a,
            fingerprint(&[n(0.0, 1.0, 60), n(1.0, 1.0, 63)], &FOUR_BARS, 0)
        );
        assert_ne!(
            a,
            fingerprint(&[n(0.0, 1.0, 60), n(1.5, 1.0, 62)], &FOUR_BARS, 0)
        );
        assert_ne!(a, fingerprint(&notes[..1], &FOUR_BARS, 0));
    }
}
