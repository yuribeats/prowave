//! Pure note-editing operations for the Piano Roll (no UI): quantize, scale quantize,
//! marquee selection and hit testing. Kept separate so they can be unit tested.
use crate::model::Note;

/// Scale modes for Scale Quantize: name and semitone offsets from the root.
pub const MODES: [(&str, &[u8]); 7] = [
    ("Major", &[0, 2, 4, 5, 7, 9, 11]),
    ("Minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("Dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("Mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
    ("Major Pentatonic", &[0, 2, 4, 7, 9]),
    ("Minor Pentatonic", &[0, 3, 5, 7, 10]),
    ("Chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
];

pub const ROOTS: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Time Quantize grid choices: label and length in beats.
pub const GRIDS: [(&str, f32); 8] = [
    ("1/4 Note", 1.0),
    ("1/8 Note", 0.5),
    ("1/8 Triplet", 1.0 / 3.0),
    ("1/16 Note", 0.25),
    ("1/16 Triplet", 1.0 / 6.0),
    ("1/32 Note", 0.125),
    ("1/32 Triplet", 1.0 / 12.0),
    ("1/64 Note", 0.0625),
];

/// Moves the start of every selected note towards the grid.
/// `strength` 0..=1 (1 = exactly on the grid). `swing` 0..=1 delays every second grid step by
/// up to a third of the step (ProWave's mapping of Logic's Swing percentage).
pub fn quantize(notes: &mut [Note], sel: &[usize], grid: f32, strength: f32, swing: f32) {
    if grid <= 0.0 {
        return;
    }
    let strength = strength.clamp(0.0, 1.0);
    let delay = swing.clamp(0.0, 1.0) * grid / 3.0;
    for &i in sel {
        let Some(n) = notes.get_mut(i) else { continue };
        let step = (n.start / grid).round();
        let mut target = step * grid;
        if (step as i64).rem_euclid(2) == 1 {
            target += delay;
        }
        n.start = (n.start + (target - n.start) * strength).max(0.0);
    }
}

/// Nearest pitch inside the scale (ties resolve downwards).
pub fn scale_snap(pitch: u8, root: u8, offsets: &[u8]) -> u8 {
    if offsets.is_empty() {
        return pitch;
    }
    let in_scale = |p: i32| {
        let pc = (p - root as i32).rem_euclid(12) as u8;
        offsets.contains(&pc)
    };
    for d in 0..12i32 {
        for p in [pitch as i32 - d, pitch as i32 + d] {
            if (0..=127).contains(&p) && in_scale(p) {
                return p as u8;
            }
        }
    }
    pitch
}

pub fn scale_quantize(notes: &mut [Note], sel: &[usize], root: u8, offsets: &[u8]) {
    for &i in sel {
        if let Some(n) = notes.get_mut(i) {
            n.pitch = scale_snap(n.pitch, root, offsets);
        }
    }
}

/// Indices of notes that intersect the beat/pitch rectangle.
pub fn marquee(notes: &[Note], beat0: f32, beat1: f32, pitch0: u8, pitch1: u8) -> Vec<usize> {
    let (b0, b1) = (beat0.min(beat1), beat0.max(beat1));
    let (p0, p1) = (pitch0.min(pitch1), pitch0.max(pitch1));
    notes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.pitch >= p0 && n.pitch <= p1 && n.start < b1 && n.start + n.len > b0)
        .map(|(i, _)| i)
        .collect()
}

/// Topmost note under (beat, pitch). The bool is true when the point is on the note's right
/// edge (within `edge` beats), which starts a resize instead of a move.
pub fn hit_note(notes: &[Note], beat: f32, pitch: u8, edge: f32) -> Option<(usize, bool)> {
    notes
        .iter()
        .enumerate()
        .rev()
        .find(|(_, n)| n.pitch == pitch && beat >= n.start && beat <= n.start + n.len)
        .map(|(i, n)| (i, beat >= n.start + n.len - edge.min(n.len * 0.5)))
}

/// Shortest note an edit leaves (a 1/64 note).
pub const MIN_LEN: f32 = 0.0625;
/// One tick at Logic's 960 ticks per quarter note.
pub const TICK: f32 = 1.0 / 960.0;

/// Whether `pitch` belongs to the scale on `root` (semitone `offsets` from the root).
pub fn in_scale(pitch: u8, root: u8, offsets: &[u8]) -> bool {
    offsets.contains(&((pitch as i32 - root as i32).rem_euclid(12) as u8))
}

/// Indices of every note with this pitch (Logic: click a key to select all its notes).
pub fn notes_of_pitch(notes: &[Note], pitch: u8) -> Vec<usize> {
    (0..notes.len())
        .filter(|&i| notes[i].pitch == pitch)
        .collect()
}

/// Indices of the notes from pitch `a` to pitch `b` (either order): Shift-click or drag over
/// keys of the keyboard.
pub fn notes_in_pitches(notes: &[Note], a: u8, b: u8) -> Vec<usize> {
    let (lo, hi) = (a.min(b), a.max(b));
    (0..notes.len())
        .filter(|&i| (lo..=hi).contains(&notes[i].pitch))
        .collect()
}

/// Moves the selected notes by `beats` and `semitones`, keeping them at or after the region
/// start and inside the MIDI range.
pub fn shift(notes: &mut [Note], sel: &[usize], beats: f32, semitones: i32) {
    for &i in sel {
        if let Some(n) = notes.get_mut(i) {
            n.start = (n.start + beats).max(0.0);
            n.pitch = (n.pitch as i32 + semitones).clamp(0, 127) as u8;
        }
    }
}

/// Copies of the selected notes, ordered by start, for the clipboard.
pub fn copy(notes: &[Note], sel: &[usize]) -> Vec<Note> {
    let mut out: Vec<Note> = sel.iter().filter_map(|&i| notes.get(i).copied()).collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// Pastes `clip` so its first note starts at `at` (beats from the region start). Returns the
/// indices of the new notes.
pub fn paste(notes: &mut Vec<Note>, clip: &[Note], at: f32) -> Vec<usize> {
    let Some(first) = clip.iter().map(|n| n.start).reduce(f32::min) else {
        return Vec::new();
    };
    let base = notes.len();
    notes.extend(clip.iter().map(|n| Note {
        start: (n.start - first + at).max(0.0),
        ..*n
    }));
    (base..notes.len()).collect()
}

/// Logic's Repeat / Duplicate: a copy of the selection right after it, the copy rounded up to
/// whole `step`s (the grid) so the pattern stays on the grid. Returns the new notes.
pub fn duplicate(notes: &mut Vec<Note>, sel: &[usize], step: f32) -> Vec<usize> {
    let clip = copy(notes, sel);
    let (Some(lo), Some(hi)) = (
        clip.iter().map(|n| n.start).reduce(f32::min),
        clip.iter().map(|n| n.start + n.len).reduce(f32::max),
    ) else {
        return Vec::new();
    };
    let span = hi - lo;
    let span = if step > 0.0 {
        ((span / step) - 1e-4).ceil().max(1.0) * step
    } else {
        span
    };
    paste(notes, &clip, lo + span)
}

/// Removes the notes `sel` (any order). Returns how many went.
pub fn delete(notes: &mut Vec<Note>, sel: &[usize]) -> usize {
    let mut sel: Vec<usize> = sel.iter().copied().filter(|i| *i < notes.len()).collect();
    sel.sort_unstable();
    sel.dedup();
    for &i in sel.iter().rev() {
        notes.remove(i);
    }
    sel.len()
}

/// Scissors: cuts note `i` at `at` (beats from the region start) into two. Returns the index
/// of the second part, or `None` when `at` is not inside the note (with room for both parts).
pub fn split(notes: &mut Vec<Note>, i: usize, at: f32) -> Option<usize> {
    let n = *notes.get(i)?;
    let (a, b) = (at - n.start, n.start + n.len - at);
    if a < TICK || b < TICK {
        return None;
    }
    notes[i].len = a;
    notes.push(Note {
        start: at,
        len: b,
        ..n
    });
    Some(notes.len() - 1)
}

/// Glue: joins the selected notes of each pitch into one note per pitch, from the first start
/// to the last end, with the first note's velocity. Returns the indices of the notes left from
/// the selection (indices shift because the glued notes are removed).
pub fn glue(notes: &mut Vec<Note>, sel: &[usize]) -> Vec<usize> {
    let mut keep: Vec<usize> = Vec::new();
    let mut gone: Vec<usize> = Vec::new();
    let mut by_pitch: Vec<(u8, Vec<usize>)> = Vec::new();
    for &i in sel.iter().filter(|i| **i < notes.len()) {
        let p = notes[i].pitch;
        match by_pitch.iter_mut().find(|(q, _)| *q == p) {
            Some((_, v)) => v.push(i),
            None => by_pitch.push((p, vec![i])),
        }
    }
    for (_, mut group) in by_pitch {
        group.sort_by(|a, b| notes[*a].start.total_cmp(&notes[*b].start));
        group.dedup();
        let first = group[0];
        let end = group
            .iter()
            .map(|&k| notes[k].start + notes[k].len)
            .fold(f32::MIN, f32::max);
        notes[first].len = (end - notes[first].start).max(TICK);
        keep.push(first);
        gone.extend_from_slice(&group[1..]);
    }
    gone.sort_unstable();
    gone.dedup();
    for &g in gone.iter().rev() {
        notes.remove(g);
    }
    keep.iter()
        .map(|&k| k - gone.iter().filter(|g| **g < k).count())
        .collect()
}

/// Which notes an Edit > Trim command trims the selection's ends against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Against {
    /// The other selected notes.
    Selected,
    /// Any note that follows (selected or not).
    Following,
    /// The next note of the same pitch.
    Repeated,
}

/// Logic's Edit > Trim > Note End commands
/// (<https://support.apple.com/guide/logicpro/resize-notes-lgcpa90a4474/mac>): the end of each
/// selected note goes to `gap` beats before the start of the next note it is trimmed
/// `against` (a negative `gap` overlaps it: "4 Ticks Overlap"). With `overlaps_only`
/// ("Remove Overlaps") only notes that sound past that next start are shortened; otherwise
/// ("legato") every note with a next note is lengthened or shortened to it. Notes sounding
/// together (a chord) are not each other's next note.
pub fn trim_end(
    notes: &mut [Note],
    sel: &[usize],
    against: Against,
    gap: f32,
    overlaps_only: bool,
) {
    let snapshot: Vec<Note> = notes.to_vec();
    for &i in sel {
        let Some(n) = snapshot.get(i).copied() else {
            continue;
        };
        let next = snapshot
            .iter()
            .enumerate()
            .filter(|(k, m)| {
                *k != i
                    && m.start > n.start + TICK / 2.0
                    && match against {
                        Against::Selected => sel.contains(k),
                        Against::Following => true,
                        Against::Repeated => m.pitch == n.pitch,
                    }
            })
            .map(|(_, m)| m.start)
            .reduce(f32::min);
        let Some(next) = next else { continue };
        if overlaps_only && n.start + n.len <= next {
            continue;
        }
        notes[i].len = (next - gap - n.start).max(TICK);
    }
}

/// Logic's 8-tick gap and 4-tick overlap of the Trim commands, in beats.
pub const TRIM_GAP: f32 = 8.0 * TICK;
pub const TRIM_OVERLAP: f32 = -4.0 * TICK;

/// The ends of the selected notes moved to `at` (Note End to Playhead), for the notes that
/// start before it.
pub fn end_to(notes: &mut [Note], sel: &[usize], at: f32) {
    for &i in sel {
        if let Some(n) = notes.get_mut(i).filter(|n| n.start < at) {
            n.len = at - n.start;
        }
    }
}

/// The starts of the selected notes moved to `at` with their ends kept (Note Start to
/// Playhead), for the notes that end after it.
pub fn start_to(notes: &mut [Note], sel: &[usize], at: f32) {
    for &i in sel {
        if let Some(n) = notes
            .get_mut(i)
            .filter(|n| n.start + n.len > at && at >= 0.0)
        {
            n.len = n.start + n.len - at;
            n.start = at;
        }
    }
}

/// Mute on/off (Logic: Mute tool, Control-M): the selected notes all take the opposite of
/// the first one's state. Returns the new state.
pub fn toggle_mute(notes: &mut [Note], sel: &[usize]) -> bool {
    let on = !sel
        .first()
        .and_then(|&i| notes.get(i))
        .is_some_and(|n| n.mute);
    for &i in sel {
        if let Some(n) = notes.get_mut(i) {
            n.mute = on;
        }
    }
    on
}

/// Sets the length of the selected notes.
pub fn set_length(notes: &mut [Note], sel: &[usize], len: f32) {
    for &i in sel {
        if let Some(n) = notes.get_mut(i) {
            n.len = len.max(TICK);
        }
    }
}

/// Sets the velocity of the selected notes.
pub fn fixed_velocity(notes: &mut [Note], sel: &[usize], vel: u8) {
    for &i in sel {
        if let Some(n) = notes.get_mut(i) {
            n.vel = vel.clamp(1, 127);
        }
    }
}

/// A small deterministic random generator (xorshift), so Humanize can be tested.
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed | 1)
    }
    /// A number in -1.0..=1.0.
    pub fn signed(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Humanize (Logic: MIDI Transform > Humanize): moves each selected note by up to `time`
/// beats either way and changes its velocity by up to `vel` either way, at random.
pub fn humanize(notes: &mut [Note], sel: &[usize], time: f32, vel: u8, rng: &mut Rng) {
    for &i in sel {
        if let Some(n) = notes.get_mut(i) {
            n.start = (n.start + rng.signed() * time.max(0.0)).max(0.0);
            let dv = (rng.signed() * vel as f32).round() as i32;
            n.vel = (n.vel as i32 + dv).clamp(1, 127) as u8;
        }
    }
}

/// Brush: the notes a brush stroke from `from` to `to` (beats) paints on `pitch`, one per
/// `step` with that length, on the grid. Steps already holding a note of that pitch are left.
pub fn brush(notes: &[Note], pitch: u8, from: f32, to: f32, step: f32, vel: u8) -> Vec<Note> {
    if step <= 0.0 {
        return Vec::new();
    }
    let (a, b) = (from.min(to).max(0.0), from.max(to).max(0.0));
    let (k0, k1) = ((a / step).floor() as i64, (b / step).floor() as i64);
    (k0..=k1)
        .map(|k| k as f32 * step)
        .filter(|s| {
            !notes
                .iter()
                .any(|n| n.pitch == pitch && (n.start - s).abs() < step / 2.0)
        })
        .map(|start| Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start,
            len: step,
            pitch,
            vel,
        })
        .collect()
}

/// The chord the pitches make, as Logic's Chord Track names it ("C", "Am7", "G7/B"): the
/// first root (lowest first) whose intervals match a known chord. `None` for fewer than two
/// pitch classes or an unknown shape.
pub fn chord_name(pitches: &[u8]) -> Option<String> {
    const SHAPES: [(&str, &[u8]); 20] = [
        ("", &[0, 4, 7]),
        ("m", &[0, 3, 7]),
        ("dim", &[0, 3, 6]),
        ("aug", &[0, 4, 8]),
        ("sus2", &[0, 2, 7]),
        ("sus4", &[0, 5, 7]),
        ("7", &[0, 4, 7, 10]),
        ("maj7", &[0, 4, 7, 11]),
        ("m7", &[0, 3, 7, 10]),
        ("mMaj7", &[0, 3, 7, 11]),
        ("m7b5", &[0, 3, 6, 10]),
        ("dim7", &[0, 3, 6, 9]),
        ("6", &[0, 4, 7, 9]),
        ("m6", &[0, 3, 7, 9]),
        ("add9", &[0, 2, 4, 7]),
        ("9", &[0, 2, 4, 7, 10]),
        ("maj9", &[0, 2, 4, 7, 11]),
        ("m9", &[0, 2, 3, 7, 10]),
        ("7sus4", &[0, 5, 7, 10]),
        ("5", &[0, 7]),
    ];
    let bass = *pitches.iter().min()?;
    let mut classes: Vec<u8> = pitches.iter().map(|p| p % 12).collect();
    classes.sort_unstable();
    classes.dedup();
    if classes.len() < 2 {
        return None;
    }
    // Try the bass first, then the other notes as the root (inversions).
    let mut roots = vec![bass % 12];
    roots.extend(classes.iter().copied().filter(|c| *c != bass % 12));
    for root in roots {
        let mut iv: Vec<u8> = classes.iter().map(|c| (c + 12 - root) % 12).collect();
        iv.sort_unstable();
        if let Some((name, _)) = SHAPES.iter().find(|(_, s)| *s == iv.as_slice()) {
            let mut out = format!("{}{}", ROOTS[root as usize], name);
            if root != bass % 12 {
                out.push('/');
                out.push_str(ROOTS[(bass % 12) as usize]);
            }
            return Some(out);
        }
    }
    None
}

/// Logic's note names (C3 is middle C, pitch 60).
pub fn note_name(pitch: u8) -> String {
    format!("{}{}", ROOTS[(pitch % 12) as usize], pitch as i32 / 12 - 2)
}

/// Resizes the notes `orig` (index, start, len) by dragging their left edge `delta` beats:
/// the start moves and the end stays.
pub fn resize_left(notes: &mut [Note], orig: &[(usize, f32, f32)], delta: f32) {
    for &(i, start, len) in orig {
        let end = start + len;
        if let Some(n) = notes.get_mut(i) {
            n.start = (start + delta).clamp(0.0, end - MIN_LEN.min(len));
            n.len = end - n.start;
        }
    }
}

/// Step input (Logic: the Piano Roll's MIDI In button,
/// <https://support.apple.com/guide/logicpro/use-step-input-recording-techniques-lgcpb19a8406/mac>):
/// each key played enters a note at the position, with the step's length, and the position
/// moves on by a step. Keys played while others are still held join them as a chord at the
/// same position (the position moves on once, with the first key).
#[derive(Default, Debug)]
pub struct StepInput {
    held: Vec<u8>,
    chord_at: f32,
}

impl StepInput {
    /// A key went down at position `pos` (beats from the region start). Returns the new
    /// note's index and the position after it.
    pub fn note_on(
        &mut self,
        notes: &mut Vec<Note>,
        pitch: u8,
        vel: u8,
        pos: f32,
        len: f32,
    ) -> (usize, f32) {
        if self.held.is_empty() {
            self.chord_at = pos.max(0.0);
        }
        if !self.held.contains(&pitch) {
            self.held.push(pitch);
        }
        let len = len.max(TICK);
        notes.push(Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start: self.chord_at,
            len,
            pitch: pitch.min(127),
            vel: vel.clamp(1, 127),
        });
        (notes.len() - 1, self.chord_at + len)
    }

    /// A key was let go.
    pub fn note_off(&mut self, pitch: u8) {
        self.held.retain(|p| *p != pitch);
    }

    /// Forgets the keys held (step input turned off, another region).
    pub fn reset(&mut self) {
        self.held.clear();
    }
}

/// Which part of a note the pointer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Body,
    /// The left edge: dragging moves the start, the end stays.
    Left,
    /// The right edge: dragging changes the length.
    Right,
}

/// Topmost note under (beat, pitch) and the part of it there; the edges are `edge` beats wide
/// (at most a third of the note each, so a short note can still be moved).
pub fn hit_part(notes: &[Note], beat: f32, pitch: u8, edge: f32) -> Option<(usize, Part)> {
    let (i, n) = notes
        .iter()
        .enumerate()
        .rev()
        .find(|(_, n)| n.pitch == pitch && beat >= n.start && beat <= n.start + n.len)?;
    let e = edge.min(n.len / 3.0);
    let part = if beat >= n.start + n.len - e {
        Part::Right
    } else if beat <= n.start + e {
        Part::Left
    } else {
        Part::Body
    };
    Some((i, part))
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
            vel: 80,
        }
    }

    #[test]
    fn step_input_enters_notes_one_after_another_and_chords_together() {
        let mut notes = Vec::new();
        let mut st = StepInput::default();
        let (i, pos) = st.note_on(&mut notes, 60, 90, 4.0, 0.25);
        assert_eq!((i, pos), (0, 4.25));
        st.note_off(60);
        // A chord: the second and third keys go down while the first is held.
        let (_, pos) = st.note_on(&mut notes, 64, 80, pos, 0.25);
        let (_, p2) = st.note_on(&mut notes, 67, 0, pos + 0.25, 0.25);
        st.note_on(&mut notes, 71, 80, p2, 0.25);
        for k in [64, 67, 71] {
            st.note_off(k);
        }
        let starts: Vec<(f32, u8)> = notes.iter().map(|n| (n.start, n.pitch)).collect();
        assert_eq!(starts, vec![(4.0, 60), (4.25, 64), (4.25, 67), (4.25, 71)]);
        assert_eq!(notes[2].vel, 1, "a velocity of 0 is still a note");
        assert_eq!(pos, 4.5, "the position moved on once for the chord");
        // Then a single note after the chord.
        let (_, pos) = st.note_on(&mut notes, 72, 100, pos, 0.5);
        assert_eq!((notes[4].start, pos), (4.5, 5.0));
    }

    #[test]
    fn hit_part_finds_both_edges_and_the_body() {
        let notes = vec![n(1.0, 2.0, 60)];
        assert_eq!(hit_part(&notes, 1.05, 60, 0.2), Some((0, Part::Left)));
        assert_eq!(hit_part(&notes, 2.0, 60, 0.2), Some((0, Part::Body)));
        assert_eq!(hit_part(&notes, 2.95, 60, 0.2), Some((0, Part::Right)));
        assert_eq!(hit_part(&notes, 2.0, 61, 0.2), None);
    }

    #[test]
    fn resize_left_moves_the_start_and_keeps_the_end() {
        let mut notes = vec![n(1.0, 2.0, 60)];
        resize_left(&mut notes, &[(0, 1.0, 2.0)], 0.5);
        assert_eq!((notes[0].start, notes[0].len), (1.5, 1.5));
        resize_left(&mut notes, &[(0, 1.0, 2.0)], -2.0);
        assert_eq!((notes[0].start, notes[0].len), (0.0, 3.0));
        // Never past the end.
        resize_left(&mut notes, &[(0, 1.0, 2.0)], 5.0);
        assert!((notes[0].start + notes[0].len - 3.0).abs() < 1e-6 && notes[0].len >= MIN_LEN);
    }

    #[test]
    fn shift_nudges_and_transposes_within_range() {
        let mut notes = vec![n(0.1, 1.0, 126), n(2.0, 1.0, 60)];
        shift(&mut notes, &[0], -0.5, 12);
        assert_eq!((notes[0].start, notes[0].pitch), (0.0, 127));
        shift(&mut notes, &[1], TICK, -1);
        assert!((notes[1].start - (2.0 + TICK)).abs() < 1e-6);
        assert_eq!(notes[1].pitch, 59);
    }

    #[test]
    fn copy_and_paste_keep_the_shape_at_the_new_place() {
        let mut notes = vec![n(2.0, 0.5, 60), n(1.0, 0.5, 64)];
        let clip = copy(&notes, &[0, 1]);
        assert_eq!(clip[0].start, 1.0);
        let new = paste(&mut notes, &clip, 8.0);
        assert_eq!(new, vec![2, 3]);
        assert_eq!((notes[2].start, notes[2].pitch), (8.0, 64));
        assert_eq!((notes[3].start, notes[3].pitch), (9.0, 60));
    }

    #[test]
    fn duplicate_places_the_copy_after_the_selection_on_the_grid() {
        let mut notes = vec![n(0.0, 0.5, 60), n(0.5, 0.75, 62)];
        let new = duplicate(&mut notes, &[0, 1], 1.0);
        // The selection spans 1.25 beats: the copy starts at the next whole beat.
        assert_eq!(new.len(), 2);
        assert_eq!(notes[2].start, 2.0);
        assert_eq!(notes[3].start, 2.5);
        assert!(duplicate(&mut notes, &[], 1.0).is_empty());
    }

    #[test]
    fn delete_removes_any_order_once() {
        let mut notes = vec![n(0.0, 1.0, 60), n(1.0, 1.0, 61), n(2.0, 1.0, 62)];
        assert_eq!(delete(&mut notes, &[2, 0, 2, 9]), 2);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch, 61);
    }

    #[test]
    fn split_cuts_a_note_in_two_at_the_point() {
        let mut notes = vec![n(1.0, 2.0, 60)];
        assert_eq!(split(&mut notes, 0, 1.5), Some(1));
        assert_eq!((notes[0].start, notes[0].len), (1.0, 0.5));
        assert_eq!(
            (notes[1].start, notes[1].len, notes[1].pitch),
            (1.5, 1.5, 60)
        );
        assert_eq!(split(&mut notes, 0, 3.0), None);
        assert_eq!(split(&mut notes, 0, 1.0), None);
    }

    #[test]
    fn glue_joins_each_pitch_into_one_note() {
        let mut notes = vec![
            n(0.0, 0.5, 60),
            n(1.0, 0.5, 62),
            n(2.0, 1.0, 60),
            n(4.0, 1.0, 60),
        ];
        let kept = glue(&mut notes, &[0, 1, 2]);
        assert_eq!(notes.len(), 3);
        assert_eq!((notes[0].start, notes[0].len), (0.0, 3.0));
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|&k| k < notes.len()));
        // The unselected note is untouched.
        assert!(notes.iter().any(|x| x.start == 4.0 && x.len == 1.0));
    }

    #[test]
    fn trim_to_selected_notes_is_legato_with_a_gap_and_chords_end_together() {
        let mut notes = vec![
            n(0.0, 0.25, 60),
            n(0.0, 0.5, 64),
            n(1.0, 3.0, 62),
            n(2.0, 0.25, 65),
        ];
        trim_end(
            &mut notes,
            &[0, 1, 2, 3],
            Against::Selected,
            TRIM_GAP,
            false,
        );
        let want = 1.0 - TRIM_GAP;
        assert!((notes[0].len - want).abs() < 1e-6 && (notes[1].len - want).abs() < 1e-6);
        assert!((notes[2].len - want).abs() < 1e-6);
        assert_eq!(notes[3].len, 0.25, "the last note keeps its length");
        let len = notes[0].len;
        trim_end(&mut notes, &[0], Against::Selected, TRIM_OVERLAP, false);
        assert_eq!(notes[0].len, len, "no other selected note: unchanged");
        trim_end(&mut notes, &[0], Against::Following, TRIM_OVERLAP, false);
        assert!((notes[0].len - (1.0 + 4.0 * TICK)).abs() < 1e-6);
    }

    #[test]
    fn remove_overlaps_only_shortens_and_repeated_looks_at_the_same_pitch() {
        let mut notes = vec![
            n(0.0, 2.0, 60),
            n(1.0, 0.25, 62),
            n(3.0, 1.0, 64),
            n(1.5, 1.0, 60),
        ];
        let all = [0, 1, 2, 3];
        let mut rep = notes.clone();
        trim_end(&mut notes, &all, Against::Following, TRIM_GAP, true);
        assert!((notes[0].len - (1.0 - TRIM_GAP)).abs() < 1e-6);
        assert_eq!(notes[1].len, 0.25, "a gap is not filled");
        trim_end(&mut rep, &all, Against::Repeated, TRIM_GAP, true);
        assert!(
            (rep[0].len - (1.5 - TRIM_GAP)).abs() < 1e-6,
            "trimmed to the next C3"
        );
    }

    #[test]
    fn ends_and_starts_go_to_the_playhead() {
        let mut notes = vec![n(0.0, 1.0, 60), n(3.0, 1.0, 62)];
        end_to(&mut notes, &[0, 1], 2.0);
        assert_eq!((notes[0].len, notes[1].len), (2.0, 1.0));
        start_to(&mut notes, &[0, 1], 1.5);
        assert_eq!((notes[0].start, notes[0].len), (1.5, 0.5));
        assert_eq!(
            (notes[1].start, notes[1].len),
            (1.5, 2.5),
            "a later note starts earlier, its end kept"
        );
    }

    #[test]
    fn mute_toggles_the_selection_together() {
        let mut notes = vec![n(0.0, 1.0, 60), n(1.0, 1.0, 62), n(2.0, 1.0, 64)];
        notes[1].mute = true;
        assert!(toggle_mute(&mut notes, &[0, 1]));
        assert!(notes[0].mute && notes[1].mute && !notes[2].mute);
        assert!(!toggle_mute(&mut notes, &[1, 2]));
        assert!(notes[0].mute && !notes[1].mute && !notes[2].mute);
        // Copies keep it.
        let clip = copy(&notes, &[0]);
        assert!(clip[0].mute);
    }

    #[test]
    fn set_length_and_fixed_velocity_change_only_the_selection() {
        let mut notes = vec![n(0.0, 2.0, 60), n(1.0, 0.25, 62)];
        set_length(&mut notes, &[1], 0.5);
        fixed_velocity(&mut notes, &[1], 200);
        assert_eq!((notes[0].len, notes[0].vel), (2.0, 80));
        assert_eq!((notes[1].len, notes[1].vel), (0.5, 127));
    }

    #[test]
    fn humanize_stays_within_its_amounts_and_is_repeatable() {
        let base: Vec<Note> = (0..50).map(|k| n(4.0 + k as f32, 0.5, 60)).collect();
        let all: Vec<usize> = (0..50).collect();
        let mut a = base.clone();
        humanize(&mut a, &all, 0.05, 10, &mut Rng::new(7));
        let mut b = base.clone();
        humanize(&mut b, &all, 0.05, 10, &mut Rng::new(7));
        assert_eq!(a, b);
        assert!(a.iter().zip(&base).any(|(x, y)| x.start != y.start));
        for (x, y) in a.iter().zip(&base) {
            assert!((x.start - y.start).abs() <= 0.05 + 1e-6);
            assert!((x.vel as i32 - y.vel as i32).abs() <= 10);
        }
    }

    #[test]
    fn brush_paints_one_note_per_grid_step_and_skips_taken_steps() {
        let notes = vec![n(0.5, 0.25, 60)];
        let painted = brush(&notes, 60, 0.1, 1.1, 0.25, 90);
        let starts: Vec<f32> = painted.iter().map(|x| x.start).collect();
        assert_eq!(starts, vec![0.0, 0.25, 0.75, 1.0]);
        assert!(painted.iter().all(|x| x.len == 0.25 && x.vel == 90));
    }

    #[test]
    fn chords_are_named_with_their_inversion() {
        assert_eq!(chord_name(&[60, 64, 67]).as_deref(), Some("C"));
        assert_eq!(chord_name(&[57, 60, 64]).as_deref(), Some("Am"));
        assert_eq!(chord_name(&[59, 62, 67, 65]).as_deref(), Some("G7/B"));
        assert_eq!(chord_name(&[64, 67, 72]).as_deref(), Some("C/E"));
        assert_eq!(chord_name(&[60, 63, 67, 70]).as_deref(), Some("Cm7"));
        assert_eq!(chord_name(&[60, 72]), None);
        assert_eq!(chord_name(&[60, 61, 62]), None);
    }

    #[test]
    fn pitch_helpers() {
        let notes = vec![
            n(0.0, 1.0, 60),
            n(1.0, 1.0, 64),
            n(2.0, 1.0, 60),
            n(3.0, 1.0, 60),
        ];
        assert_eq!(notes_of_pitch(&notes, 60), vec![0, 2, 3]);
        assert_eq!(notes_in_pitches(&notes, 64, 61), vec![1]);
        assert_eq!(notes_in_pitches(&notes, 59, 65), vec![0, 1, 2, 3]);
        assert!(in_scale(62, 0, MODES[0].1) && !in_scale(61, 0, MODES[0].1));
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(0), "C-2");
    }

    fn note(start: f32, pitch: u8) -> Note {
        Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start,
            len: 0.5,
            pitch,
            vel: 90,
        }
    }

    #[test]
    fn quantize_full_strength_snaps_to_grid() {
        let mut notes = vec![note(0.9, 60), note(1.3, 62)];
        quantize(&mut notes, &[0, 1], 0.5, 1.0, 0.0);
        assert!((notes[0].start - 1.0).abs() < 1e-6);
        assert!((notes[1].start - 1.5).abs() < 1e-6);
    }

    #[test]
    fn quantize_half_strength_moves_halfway_and_only_selected() {
        let mut notes = vec![note(0.9, 60), note(0.9, 62)];
        quantize(&mut notes, &[0], 1.0, 0.5, 0.0);
        assert!((notes[0].start - 0.95).abs() < 1e-6);
        assert!((notes[1].start - 0.9).abs() < 1e-6);
    }

    #[test]
    fn swing_delays_odd_steps_only() {
        let mut notes = vec![note(0.0, 60), note(0.5, 60), note(1.0, 60), note(1.5, 60)];
        quantize(&mut notes, &[0, 1, 2, 3], 0.5, 1.0, 1.0);
        let d = 0.5 / 3.0;
        let want = [0.0, 0.5 + d, 1.0, 1.5 + d];
        for (n, w) in notes.iter().zip(want) {
            assert!((n.start - w).abs() < 1e-5, "{} vs {w}", n.start);
        }
    }

    #[test]
    fn scale_snap_c_major() {
        let major = MODES[0].1;
        assert_eq!(scale_snap(61, 0, major), 60); // C# -> C (tie goes down)
        assert_eq!(scale_snap(63, 0, major), 62); // D# -> D
        assert_eq!(scale_snap(64, 0, major), 64); // E stays
        assert_eq!(scale_snap(70, 0, major), 69); // A# -> A
                                                  // Other roots: D major contains F# but not F.
        assert_eq!(scale_snap(65, 2, major), 64); // F -> E
        assert_eq!(scale_snap(66, 2, major), 66);
    }

    #[test]
    fn scale_quantize_changes_only_selected() {
        let mut notes = vec![note(0.0, 61), note(1.0, 61)];
        scale_quantize(&mut notes, &[1], 0, MODES[0].1);
        assert_eq!((notes[0].pitch, notes[1].pitch), (61, 60));
    }

    #[test]
    fn marquee_picks_intersecting_notes() {
        let notes = vec![note(0.0, 60), note(2.0, 64), note(2.0, 72)];
        assert_eq!(marquee(&notes, 1.9, 3.0, 62, 66), vec![1]);
        assert_eq!(marquee(&notes, 3.0, 1.9, 66, 62), vec![1]);
        assert!(marquee(&notes, 5.0, 6.0, 0, 127).is_empty());
    }

    #[test]
    fn hit_note_detects_body_and_right_edge() {
        let notes = vec![Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start: 1.0,
            len: 2.0,
            pitch: 60,
            vel: 100,
        }];
        assert_eq!(hit_note(&notes, 1.5, 60, 0.2), Some((0, false)));
        assert_eq!(hit_note(&notes, 2.9, 60, 0.2), Some((0, true)));
        assert_eq!(hit_note(&notes, 1.5, 61, 0.2), None);
        assert_eq!(hit_note(&notes, 3.5, 60, 0.2), None);
    }
}
