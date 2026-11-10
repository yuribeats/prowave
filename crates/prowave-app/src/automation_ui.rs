//! The automation lanes of the Tracks area, as in Ableton Live's Arrangement View (Live 12
//! manual, chapter 25: <https://www.ableton.com/en/live-manual/12/automation-and-editing-envelopes/>).
//!
//! - A toggles Automation Mode: every track shows the envelope of the parameter its choosers
//!   pick, over its regions, and the device and parameter choosers in its header; "+" shows
//!   another parameter in a lane of its own (§25, "Automation and Editing Envelopes").
//! - On a lane: click on the line adds a breakpoint, drag moves one (Shift: finely),
//!   double-click or Delete removes it, Option-drag on a segment bends it (§25.5.2, "Editing
//!   Breakpoints").
//! - B toggles Draw Mode: dragging draws steps one grid unit wide, or a free line when the grid
//!   is at Ticks or Command is held (§25.5.1, "Drawing Envelopes").
//! - Dragging over an empty stretch selects a time range: its breakpoints move with the pointer,
//!   its right handle stretches them in time and its top handle scales their values
//!   (§25.5.3, "Stretching and Skewing Envelopes"); Command-C, Command-V and Command-D copy,
//!   paste at the playhead and duplicate it, Delete removes it (§25.5.7, "Edit Menu Commands").
//! - The toolbar has Automation Arm and Re-Enable Automation (§25.1 and §25.4).
use crate::automation::{self, AutoView, Automation, ParamDesc, Strip};
use crate::hw::{self, Cap, Legend};
use crate::i18n::tr;
use crate::model::{Project, Track, TrackKind};
use crate::theme::{self, Face};
use crate::widgets::popup_button;
use egui::{pos2, vec2, Color32, Key, Modifiers, Painter, Pos2, Rect, Sense, Stroke, Ui};
use prowave_engine::{curve_shape, lane_value, AutoPoint, AutoTarget};
use std::collections::HashMap;

/// The strip under a track's header with its choosers, in Automation Mode.
pub const CHOOSER_H: f32 = 28.0;
/// A lane under a track (Show Automation in New Lane).
pub const LANE_H: f32 = 40.0;
/// An envelope line (Live draws them red) and an overridden one (grey, dimmed).
pub const ENVELOPE: Color32 = theme::ORANGE;
pub const OVERRIDDEN: Color32 = theme::SEAM_DK;
/// The LED of an automated control.
pub const LED_ON: Color32 = theme::ORANGE;
/// How far from a breakpoint a click still takes it, and from a line, in points.
const GRAB: f32 = 6.0;
const LINE_GRAB: f32 = 5.0;
/// Space above and below the envelope inside its lane.
const PAD: f32 = 4.0;

/// A time range selected on one lane.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sel {
    pub strip: Strip,
    pub target: AutoTarget,
    pub from: f64,
    pub to: f64,
}

/// A piece of envelope on the clipboard: breakpoints from its start, and its length.
#[derive(Clone, PartialEq, Debug)]
pub struct Piece {
    pub target: AutoTarget,
    pub len: f64,
    pub points: Vec<AutoPoint>,
}

/// What a drag on a lane is doing.
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// A breakpoint, with where it started and the pointer's height then.
    Point {
        index: usize,
        beat: f64,
        value: f32,
        y: f32,
    },
    /// A segment moved up or down: its two ends' values at the start.
    Segment {
        a: usize,
        b: usize,
        va: f32,
        vb: f32,
        y: f32,
    },
    /// A segment bent (Option-drag): its start breakpoint's curve then.
    Curve {
        index: usize,
        curve: f32,
        y: f32,
        rising: bool,
    },
    /// Selecting a range from `anchor`.
    Marquee { anchor: f64 },
    /// The selected breakpoints moved: the lane as it was and the pointer then.
    Move {
        orig: Vec<AutoPoint>,
        beat: f64,
        y: f32,
        sel: Sel,
    },
    /// The selection's right handle: stretches it in time from its start.
    Stretch { orig: Vec<AutoPoint>, sel: Sel },
    /// The selection's top handle: scales its values towards the bottom.
    Scale {
        orig: Vec<AutoPoint>,
        y: f32,
        sel: Sel,
    },
    /// Draw Mode: the last place drawn.
    Draw { last: Option<(f64, f32)> },
}

/// The editing state of the lanes.
#[derive(Default)]
pub struct LaneState {
    /// The last value seen of Audio Unit parameters moved by hand: (track, slot, param).
    pub au_now: HashMap<(usize, u8, u32), f32>,
    drag: Option<(Strip, AutoTarget, Drag)>,
    pub sel: Option<Sel>,
    pub clip: Option<Piece>,
    /// Automation copied with regions (Edit > Copy), per region: track, start beat, length,
    /// and the pieces of its lanes.
    pub with_regions: Vec<(usize, f64, f64, Vec<Piece>)>,
    /// The breakpoint a click just made (a double-click keeps it instead of removing it).
    made: Option<(Strip, AutoTarget, f64)>,
}

// ------------------------------------------------------------------ envelope edits

/// The value just before `beat` (what a jump at `beat` leaves), or `None` without breakpoints.
fn value_before(pts: &[AutoPoint], beat: f64) -> Option<f32> {
    let first = pts.first()?;
    if beat <= first.beat {
        return Some(first.value);
    }
    let i = pts.partition_point(|p| p.beat < beat);
    let (a, b) = (pts[i - 1], pts.get(i).copied().unwrap_or(pts[i - 1]));
    if b.beat <= a.beat {
        return Some(a.value);
    }
    let t = curve_shape(((beat - a.beat) / (b.beat - a.beat)) as f32, a.curve);
    Some(a.value + (b.value - a.value) * t)
}

/// Adds a breakpoint at `beat` on the line (it keeps the envelope's shape); returns its index.
pub fn add_on_line(pts: &mut Vec<AutoPoint>, beat: f64, own: f32) -> usize {
    let v = lane_value(pts, beat).unwrap_or(own);
    add_point(pts, beat, v)
}

/// Adds a breakpoint at `beat` with `value`; returns its index.
pub fn add_point(pts: &mut Vec<AutoPoint>, beat: f64, value: f32) -> usize {
    let beat = beat.max(0.0);
    let i = pts.partition_point(|p| p.beat <= beat);
    // A breakpoint added inside a bent segment starts with the same bend.
    let curve = if i > 0 && i < pts.len() {
        pts[i - 1].curve
    } else {
        0.0
    };
    if pts.len() < Automation::MAX_POINTS {
        pts.insert(i, AutoPoint { beat, value, curve });
    }
    i.min(pts.len().saturating_sub(1))
}

/// Moves breakpoint `i` to `beat` (kept between its neighbours) and `value`.
pub fn move_point(pts: &mut [AutoPoint], i: usize, beat: f64, value: f32) {
    let lo = if i > 0 { pts[i - 1].beat } else { 0.0 };
    let hi = pts.get(i + 1).map_or(f64::MAX, |p| p.beat);
    if let Some(p) = pts.get_mut(i) {
        p.beat = beat.clamp(lo, hi.max(lo));
        p.value = value;
    }
}

/// Removes breakpoint `i`.
pub fn remove_point(pts: &mut Vec<AutoPoint>, i: usize) {
    if i < pts.len() {
        pts.remove(i);
    }
}

/// Bends the segment that starts at breakpoint `i` (Option-drag): `curve` -1..=1.
pub fn bend(pts: &mut [AutoPoint], i: usize, curve: f32) {
    if i + 1 < pts.len() {
        pts[i].curve = curve.clamp(-1.0, 1.0);
    }
}

/// Draw Mode with the grid: the envelope holds `value` from `from` to `to` (one grid unit),
/// with jumps at both ends; what lay between is replaced.
pub fn draw_step(pts: &mut Vec<AutoPoint>, from: f64, to: f64, value: f32) {
    if to <= from {
        return;
    }
    let left = (from > 0.0)
        .then(|| value_before(pts, from))
        .flatten()
        .filter(|_| pts.iter().any(|p| p.beat < from));
    let right = pts
        .iter()
        .any(|p| p.beat > to)
        .then(|| lane_value(pts, to + 1e-9))
        .flatten();
    pts.retain(|p| p.beat < from || p.beat > to);
    let i = pts.partition_point(|p| p.beat < from);
    let mut add = Vec::with_capacity(4);
    if let Some(l) = left {
        add.push(AutoPoint::new(from, l));
    }
    add.push(AutoPoint::new(from, value));
    add.push(AutoPoint::new(to, value));
    if let Some(r) = right {
        add.push(AutoPoint::new(to, r));
    }
    pts.splice(i..i, add);
    dedupe(pts);
}

/// Draw Mode without the grid: a line from where the pointer was to where it is; what lay
/// between is replaced.
pub fn draw_free(pts: &mut Vec<AutoPoint>, last: Option<(f64, f32)>, beat: f64, value: f32) {
    let (b0, v0) = last.unwrap_or((beat, value));
    let (a, b) = if b0 <= beat { (b0, beat) } else { (beat, b0) };
    pts.retain(|p| p.beat < a || p.beat > b);
    for (x, y) in [(b0, v0), (beat, value)] {
        let i = pts.partition_point(|p| p.beat <= x);
        if pts.len() < Automation::MAX_POINTS {
            pts.insert(i, AutoPoint::new(x, y));
        }
    }
    pts.sort_by(|p, q| p.beat.total_cmp(&q.beat));
    dedupe(pts);
}

/// Removes breakpoints that repeat the one before them (same beat and value).
fn dedupe(pts: &mut Vec<AutoPoint>) {
    pts.dedup_by(|b, a| (a.beat - b.beat).abs() < 1e-9 && a.value == b.value);
}

/// The indexes of the breakpoints in `from..=to`.
pub fn in_range(pts: &[AutoPoint], from: f64, to: f64) -> std::ops::Range<usize> {
    let a = pts.partition_point(|p| p.beat < from - 1e-9);
    let b = pts.partition_point(|p| p.beat <= to + 1e-9);
    a..b.max(a)
}

/// The selected breakpoints moved by `beats` and `dv` (in the lane's units, kept in range);
/// they replace what they land on. `orig` is the lane before the move. Returns the lane and
/// the moved range.
pub fn move_range(
    orig: &[AutoPoint],
    from: f64,
    to: f64,
    beats: f64,
    dv: f32,
    target: AutoTarget,
) -> Vec<AutoPoint> {
    let (lo, hi) = Automation::range(target);
    let r = in_range(orig, from, to);
    let beats = beats.max(-from);
    let moved: Vec<AutoPoint> = orig[r.clone()]
        .iter()
        .map(|p| AutoPoint {
            beat: p.beat + beats,
            value: (p.value + dv).clamp(lo, hi),
            curve: p.curve,
        })
        .collect();
    let (na, nb) = (from + beats, to + beats);
    let mut out: Vec<AutoPoint> = orig
        .iter()
        .enumerate()
        .filter(|(i, p)| !r.contains(i) && (p.beat < na - 1e-9 || p.beat > nb + 1e-9))
        .map(|(_, p)| *p)
        .collect();
    let i = out.partition_point(|p| p.beat < na);
    out.splice(i..i, moved);
    out
}

/// The selected breakpoints stretched in time: `from..to` becomes `from..new_to`.
pub fn stretch_range(orig: &[AutoPoint], from: f64, to: f64, new_to: f64) -> Vec<AutoPoint> {
    let new_to = new_to.max(from + 1.0 / 64.0);
    let r = in_range(orig, from, to);
    let k = if to > from {
        (new_to - from) / (to - from)
    } else {
        1.0
    };
    let moved: Vec<AutoPoint> = orig[r.clone()]
        .iter()
        .map(|p| AutoPoint {
            beat: from + (p.beat - from) * k,
            ..*p
        })
        .collect();
    let mut out: Vec<AutoPoint> = orig
        .iter()
        .enumerate()
        .filter(|(i, p)| !r.contains(i) && (p.beat < from || p.beat > new_to))
        .map(|(_, p)| *p)
        .collect();
    let i = out.partition_point(|p| p.beat < from);
    out.splice(i..i, moved);
    out
}

/// The selected breakpoints' heights scaled by `k` towards the bottom of the lane.
pub fn scale_range(
    orig: &[AutoPoint],
    from: f64,
    to: f64,
    k: f32,
    target: AutoTarget,
) -> Vec<AutoPoint> {
    let mut out = orig.to_vec();
    for p in &mut out[in_range(orig, from, to)] {
        let f = automation::frac_of(target, p.value) * k.max(0.0);
        p.value = automation::value_of(target, f.min(1.0));
    }
    out
}

/// The envelope from `from` to `to` as a piece starting at 0, with breakpoints at both ends so
/// it keeps its values when pasted.
pub fn copy_piece(pts: &[AutoPoint], target: AutoTarget, from: f64, to: f64) -> Option<Piece> {
    if pts.is_empty() || to <= from {
        return None;
    }
    let mut out = Vec::new();
    let start = lane_value(pts, from)?;
    out.push(AutoPoint::new(0.0, start));
    for p in &pts[in_range(pts, from, to)] {
        if p.beat > from + 1e-9 && p.beat < to - 1e-9 {
            out.push(AutoPoint {
                beat: p.beat - from,
                ..*p
            });
        }
    }
    let end = value_before(pts, to)?;
    out.push(AutoPoint::new(to - from, end));
    Some(Piece {
        target,
        len: to - from,
        points: out,
    })
}

/// Puts `piece` at `at`, replacing the envelope there; the envelope after it keeps its values.
pub fn paste_piece(pts: &mut Vec<AutoPoint>, piece: &Piece, at: f64) {
    let at = at.max(0.0);
    let end = at + piece.len;
    let before = pts
        .iter()
        .any(|p| p.beat < at)
        .then(|| value_before(pts, at))
        .flatten();
    let after = pts
        .iter()
        .any(|p| p.beat > end)
        .then(|| lane_value(pts, end + 1e-9))
        .flatten();
    pts.retain(|p| p.beat < at - 1e-9 || p.beat > end + 1e-9);
    let i = pts.partition_point(|p| p.beat < at);
    let mut add = Vec::with_capacity(piece.points.len() + 2);
    if let Some(b) = before {
        add.push(AutoPoint::new(at, b));
    }
    add.extend(piece.points.iter().map(|p| AutoPoint {
        beat: p.beat + at,
        ..*p
    }));
    if let Some(a) = after {
        add.push(AutoPoint::new(end, a));
    }
    pts.splice(i..i, add);
    dedupe(pts);
    pts.truncate(Automation::MAX_POINTS);
}

/// Removes the breakpoints from `from` to `to`; the envelope keeps its values outside.
pub fn clear_range(pts: &mut Vec<AutoPoint>, from: f64, to: f64) {
    let before = pts
        .iter()
        .any(|p| p.beat < from)
        .then(|| value_before(pts, from))
        .flatten();
    let after = pts
        .iter()
        .any(|p| p.beat > to)
        .then(|| lane_value(pts, to + 1e-9))
        .flatten();
    pts.retain(|p| p.beat < from - 1e-9 || p.beat > to + 1e-9);
    let i = pts.partition_point(|p| p.beat < from);
    let mut add = Vec::new();
    if let Some(b) = before {
        add.push(AutoPoint::new(from, b));
    }
    if let Some(a) = after {
        add.push(AutoPoint::new(to, a));
    }
    pts.splice(i..i, add);
    dedupe(pts);
}

// ------------------------------------------------------------------ automation with regions

/// The pieces of every lane of `a` from `from` to `to` (a region's span).
fn pieces_of(a: &Automation, from: f64, to: f64) -> Vec<Piece> {
    a.lanes()
        .filter_map(|(t, pts)| {
            // Only the breakpoints under the region count: a lane without any is left alone.
            let r = in_range(pts, from, to);
            (!r.is_empty())
                .then(|| copy_piece(pts, t, from, to))
                .flatten()
        })
        .collect()
}

/// Edit > Copy (and Cut) of regions: the automation under them goes with them (Live: with
/// Lock Envelopes off, envelopes move and copy with the clips).
pub fn copy_with_regions(p: &Project, v: &mut AutoView, sel: &[(usize, usize)]) {
    v.lanes.with_regions.clear();
    if !v.with_regions {
        return;
    }
    for &(t, r) in sel {
        let Some(reg) = p.tracks.get(t).and_then(|tr| tr.regions.get(r)) else {
            continue;
        };
        let (start, len) = (p.region_start_beats(reg), p.region_len_beats(reg));
        let pieces = pieces_of(&p.tracks[t].auto, start, start + len);
        v.lanes.with_regions.push((t, start, len, pieces));
    }
}

/// Edit > Cut: the automation copied with the regions is taken out of their tracks.
pub fn cut_with_regions(p: &mut Project, v: &AutoView) {
    for (t, start, len, pieces) in &v.lanes.with_regions {
        if let Some(track) = p.tracks.get_mut(*t) {
            for piece in pieces {
                clear_range(track.auto.lane_mut(piece.target), *start, start + len);
            }
            track.auto.prune();
        }
    }
}

/// Edit > Paste: the regions' automation lands on the selected track at the playhead, with
/// the regions (the earliest at the playhead, the others at their distances from it).
pub fn paste_with_regions(p: &mut Project, v: &AutoView, playhead: f64) {
    let base = v
        .lanes
        .with_regions
        .iter()
        .map(|x| x.1)
        .fold(f64::MAX, f64::min);
    let to = p.selected_track;
    let Some(track) = p.tracks.get_mut(to) else {
        return;
    };
    for (from_track, start, _, pieces) in &v.lanes.with_regions {
        for piece in pieces {
            // A plug-in's lane only makes sense on the track the plug-in is on.
            if matches!(piece.target, AutoTarget::Plugin { .. }) && *from_track != to {
                continue;
            }
            paste_piece(
                track.auto.lane_mut(piece.target),
                piece,
                playhead + (start - base),
            );
        }
    }
    track.auto.prune();
}

/// Regions moved (dragged, Move to Playhead): (track, region, start beat before). The
/// automation under each moves with it.
pub fn follow_moves(p: &mut Project, v: &AutoView, moved: &[(usize, usize, f64)]) {
    if !v.with_regions {
        return;
    }
    // Lift every piece first, so regions moved over each other do not carry each other's.
    let mut lifted = Vec::new();
    for &(t, r, was) in moved {
        let Some(reg) = p.tracks.get(t).and_then(|tr| tr.regions.get(r)) else {
            continue;
        };
        let (now, len) = (p.region_start_beats(reg), p.region_len_beats(reg));
        if (now - was).abs() < 1e-9 {
            continue;
        }
        let pieces = pieces_of(&p.tracks[t].auto, was, was + len);
        let a = &mut p.tracks[t].auto;
        for piece in &pieces {
            let pts = a.lane_mut(piece.target);
            // The envelope closes over the hole the region left.
            pts.retain(|x| x.beat < was - 1e-9 || x.beat > was + len + 1e-9);
        }
        lifted.push((t, now, pieces));
    }
    for (t, now, pieces) in lifted {
        let a = &mut p.tracks[t].auto;
        for piece in &pieces {
            paste_piece(a.lane_mut(piece.target), piece, now);
        }
        a.prune();
    }
}

/// Regions dropped on other tracks or copied by an Option-drag: (track before, start before,
/// length, track now, start now) in quarter notes. The automation under each is copied to where
/// it landed, or with `lift` moved there (the envelope closes over the hole it left).
pub fn follow_drops(
    p: &mut Project,
    v: &AutoView,
    drops: &[(usize, f64, f64, usize, f64)],
    lift: bool,
) {
    if !v.with_regions {
        return;
    }
    // Lift every piece first, so regions dropped over each other do not carry each other's.
    let mut taken = Vec::new();
    for &(from, was, len, to, now) in drops {
        let Some(track) = p.tracks.get_mut(from) else {
            continue;
        };
        let pieces = pieces_of(&track.auto, was, was + len);
        if lift {
            for piece in &pieces {
                let pts = track.auto.lane_mut(piece.target);
                pts.retain(|x| x.beat < was - 1e-9 || x.beat > was + len + 1e-9);
            }
            track.auto.prune();
        }
        taken.push((from, to, now, pieces));
    }
    for (from, to, now, pieces) in taken {
        let Some(track) = p.tracks.get_mut(to) else {
            continue;
        };
        for piece in &pieces {
            // A plug-in's lane only makes sense on the track the plug-in is on.
            if matches!(piece.target, AutoTarget::Plugin { .. }) && from != to {
                continue;
            }
            paste_piece(track.auto.lane_mut(piece.target), piece, now);
        }
        track.auto.prune();
    }
}

/// Repeat Regions: each repeated region's automation is repeated after it.
pub fn follow_repeats(p: &mut Project, v: &AutoView, repeated: &[(usize, f64, f64)]) {
    if !v.with_regions {
        return;
    }
    for &(t, start, len) in repeated {
        let Some(track) = p.tracks.get_mut(t) else {
            continue;
        };
        for piece in pieces_of(&track.auto, start, start + len) {
            paste_piece(track.auto.lane_mut(piece.target), &piece, start + len);
        }
    }
}

// ------------------------------------------------------------------ layout

/// Whether `t` gets lanes in Automation Mode (a folder has no sound of its own).
pub fn has_lanes(t: &Track) -> bool {
    t.kind != TrackKind::Folder
}

/// The height Automation Mode adds to the track's own row (its choosers).
pub fn chooser_height(v: &AutoView, t: &Track) -> f32 {
    if v.mode && has_lanes(t) {
        CHOOSER_H
    } else {
        0.0
    }
}

/// The height of the lanes under the track.
pub fn lanes_height(v: &AutoView, t: &Track) -> f32 {
    if v.mode && has_lanes(t) {
        t.auto.extra.len() as f32 * LANE_H
    } else {
        0.0
    }
}

/// Entering Automation Mode, a track shows its automated parameters: the first on its own
/// lane, the others in lanes under it (Live shows only automated parameters by default).
pub fn lay_out(p: &mut Project, v: &mut AutoView) {
    for (i, t) in p.tracks.iter_mut().enumerate() {
        if v.laid_out.contains(&i) {
            continue;
        }
        v.laid_out.push(i);
        let automated = t.auto.automated();
        if automated.is_empty() {
            continue;
        }
        if !automated.contains(&t.auto.shown) {
            t.auto.shown = automated[0];
        }
        for a in automated {
            if a != t.auto.shown && !t.auto.extra.contains(&a) {
                t.auto.extra.push(a);
            }
        }
    }
}

// ------------------------------------------------------------------ drawing

/// Where value `v` of `target` is on `r` (top: the highest value).
fn y_of(r: Rect, target: AutoTarget, v: f32) -> f32 {
    r.bottom() - PAD - automation::frac_of(target, v) * (r.height() - 2.0 * PAD)
}

fn value_at_y(r: Rect, target: AutoTarget, y: f32) -> f32 {
    let f = (r.bottom() - PAD - y) / (r.height() - 2.0 * PAD).max(1.0);
    automation::value_of(target, f)
}

/// The envelope's line on `r`, as screen points (`beat_of`/`x_of` map time).
fn line_points(
    r: Rect,
    target: AutoTarget,
    pts: &[AutoPoint],
    own: f32,
    x_of: &dyn Fn(f64) -> f32,
) -> Vec<Pos2> {
    if pts.is_empty() {
        let y = y_of(r, target, own);
        return vec![pos2(r.left(), y), pos2(r.right(), y)];
    }
    let mut out = Vec::with_capacity(pts.len() * 2 + 2);
    out.push(pos2(r.left(), y_of(r, target, pts[0].value)));
    for (i, p) in pts.iter().enumerate() {
        let x = x_of(p.beat);
        out.push(pos2(x, y_of(r, target, p.value)));
        if let Some(n) = pts.get(i + 1) {
            let (x0, x1) = (x.max(r.left() - 4.0), x_of(n.beat).min(r.right() + 4.0));
            if p.curve != 0.0 && x1 > x0 {
                let steps = (((x1 - x0) / 3.0) as usize).clamp(2, 400);
                for k in 1..steps {
                    let xs = x0 + (x1 - x0) * k as f32 / steps as f32;
                    let t = ((xs - x) / (x_of(n.beat) - x).max(1e-3)).clamp(0.0, 1.0);
                    let v = p.value + (n.value - p.value) * curve_shape(t, p.curve);
                    out.push(pos2(xs, y_of(r, target, v)));
                }
            }
        }
    }
    let last = pts[pts.len() - 1];
    out.push(pos2(r.right(), y_of(r, target, last.value)));
    out.retain(|q| q.x.is_finite() && q.y.is_finite());
    out
}

/// Distance from `pt` to the envelope's line, vertically, at the pointer's x.
fn line_distance(line: &[Pos2], pt: Pos2) -> f32 {
    let mut best = f32::MAX;
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        if pt.x >= a.x.min(b.x) - 0.5 && pt.x <= a.x.max(b.x) + 0.5 {
            let y = if (b.x - a.x).abs() < 1e-3 {
                pt.y.clamp(a.y.min(b.y), a.y.max(b.y))
            } else {
                a.y + (b.y - a.y) * (pt.x - a.x) / (b.x - a.x)
            };
            best = best.min((y - pt.y).abs());
        }
    }
    best
}

/// A small LED: red when the control is automated, grey when its envelope is overridden.
pub fn led(g: &Painter, at: Pos2, a: &Automation, target: AutoTarget) {
    if a.lane(target).is_empty() {
        return;
    }
    hw::led_sized(g, at, 2.4, !a.is_overridden(target));
}

/// What the lanes need to know about the Tracks area.
pub struct Grid<'a> {
    pub x_of: &'a dyn Fn(f64) -> f32,
    pub beat_of: &'a dyn Fn(f32) -> f64,
    /// The grid step in beats, and whether Draw Mode draws freehand (no grid).
    pub step: f64,
    pub free: bool,
}

/// One lane: the envelope of `target` of `strip` on `r` (the lanes area's clip `clip`), and
/// its editing. `own` is the control's value without automation.
#[allow(clippy::too_many_arguments)]
pub fn lane(
    ui: &Ui,
    g: &Painter,
    p: &mut Project,
    v: &mut AutoView,
    strip: Strip,
    target: AutoTarget,
    r: Rect,
    grid: &Grid,
    over_regions: bool,
) {
    let Some(a) = automation::auto_of(p, strip) else {
        return;
    };
    let own = match strip {
        Strip::Track(i) => automation::own_value(&p.tracks[i], target).or_else(|| match target {
            AutoTarget::Plugin { slot, param } => v.lanes.au_now.get(&(i, slot, param)).copied(),
            _ => None,
        }),
        Strip::Master => automation::master_value(p, target).or_else(|| match target {
            AutoTarget::Plugin { slot, param } => v
                .lanes
                .au_now
                .get(&(crate::mixer::MASTER_STRIP, slot, param))
                .copied(),
            _ => None,
        }),
    }
    .unwrap_or_else(|| {
        let (lo, hi) = Automation::range(target);
        (lo + hi) / 2.0
    });
    let pts = a.lane(target).to_vec();
    let overridden = a.is_overridden(target);
    let x_of = grid.x_of;
    let beat_of = grid.beat_of;

    if over_regions {
        // The regions dim under the envelope, as in Live.
        g.rect_filled(r, 0.0, theme::INK.gamma_multiply(0.28));
    } else {
        // A lane of its own: a well of deeper bone set into the arrange floor.
        g.rect_filled(r, 0.0, theme::BONE3);
        let well = r.shrink2(vec2(0.0, 2.0));
        g.rect_filled(well, 2.0, hw::lerp(theme::BONE2, theme::BONE3, 0.55));
        hw::inner_shadow(g, well, 2.0, 0.28);
        g.hline(
            well.x_range(),
            well.bottom() + 0.5,
            Stroke::new(1.0, theme::BONE_HI),
        );
    }
    // The selected range.
    let sel = v
        .lanes
        .sel
        .filter(|s| s.strip == strip && s.target == target);
    if let Some(s) = sel {
        let sr = Rect::from_x_y_ranges(x_of(s.from)..=x_of(s.to), r.y_range());
        g.rect_filled(sr, 0.0, theme::ORANGE.gamma_multiply(0.1));
        g.rect_stroke(
            sr,
            0.0,
            Stroke::new(1.0, theme::ORANGE.gamma_multiply(0.7)),
            egui::StrokeKind::Inside,
        );
    }
    let line = line_points(r, target, &pts, own, x_of);
    let colour = if pts.is_empty() {
        ENVELOPE.gamma_multiply(0.45)
    } else if overridden {
        OVERRIDDEN.gamma_multiply(0.7)
    } else {
        ENVELOPE
    };
    // The envelope: an orange line with a thin ink edge, so it reads on the bone floor and on
    // the black regions alike.
    g.add(egui::Shape::line(
        line.clone(),
        Stroke::new(3.0, theme::INK.gamma_multiply(0.35)),
    ));
    g.add(egui::Shape::line(line.clone(), Stroke::new(1.8, colour)));
    let selected = |b: f64| sel.is_some_and(|s| b >= s.from - 1e-9 && b <= s.to + 1e-9);
    for pt in &pts {
        let c = pos2(x_of(pt.beat), y_of(r, target, pt.value));
        if r.expand(4.0).contains(c) {
            let fill = if selected(pt.beat) {
                theme::BONE_HI
            } else {
                colour
            };
            g.circle_filled(c, 3.4, fill);
            g.circle_stroke(c, 3.4, Stroke::new(1.0, theme::INK));
        }
    }
    // The selection's handles: right edge stretches, top edge scales.
    let handles = sel.map(|s| {
        let right = Rect::from_center_size(pos2(x_of(s.to), r.center().y), vec2(7.0, 12.0));
        let top = Rect::from_center_size(
            pos2((x_of(s.from) + x_of(s.to)) / 2.0, r.top() + 4.0),
            vec2(12.0, 7.0),
        );
        for h in [right, top] {
            g.rect_filled(h, 1.5, theme::BONE_HI);
            g.rect_stroke(
                h,
                1.5,
                Stroke::new(1.0, theme::INK),
                egui::StrokeKind::Inside,
            );
        }
        (right, top)
    });

    let id = egui::Id::unique(("auto_lane", strip, target, over_regions));
    let resp = ui.interact(r, id, Sense::click_and_drag());
    let hover = resp.hover_pos().or(resp.interact_pointer_pos());
    let mods = ui.input(|i| i.modifiers);
    let near_point = |pt: Pos2| {
        pts.iter()
            .enumerate()
            .map(|(i, q)| (i, pos2(x_of(q.beat), y_of(r, target, q.value)).distance(pt)))
            .filter(|(_, d)| *d <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    if let Some(pt) = hover.filter(|_| resp.hovered()) {
        let desc = automation::describe(p, strip, target, &v.au);
        if let Some(i) = near_point(pt) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            if let Some(d) = &desc {
                let q = pts[i];
                let c = pos2(x_of(q.beat), y_of(r, target, q.value));
                tag(g, r, c, &automation::value_text(d, q.value));
            }
        } else if v.draw {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if line_distance(&line, pt) <= LINE_GRAB {
            ui.ctx().set_cursor_icon(if mods.alt {
                egui::CursorIcon::ResizeVertical
            } else {
                egui::CursorIcon::PointingHand
            });
        }
    }

    let key = (strip, target);
    let snap = |b: f64| (b / grid.step).floor() * grid.step;
    if resp.drag_started() {
        // Where the button went down, not where the pointer is once it counts as a drag.
        let origin = ui.input(|i| i.pointer.press_origin());
        if let Some(pt) = origin.or(resp.interact_pointer_pos()) {
            let beat = beat_of(pt.x);
            let in_sel = sel.is_some_and(|s| beat >= s.from && beat <= s.to);
            let drag = if v.draw {
                Some(Drag::Draw { last: None })
            } else if let Some((right, top)) =
                handles.filter(|(a, b)| a.contains(pt) || b.contains(pt))
            {
                let s = sel.unwrap_or(Sel {
                    strip,
                    target,
                    from: 0.0,
                    to: 0.0,
                });
                if right.contains(pt) {
                    Some(Drag::Stretch {
                        orig: pts.clone(),
                        sel: s,
                    })
                } else {
                    let _ = top;
                    Some(Drag::Scale {
                        orig: pts.clone(),
                        y: pt.y,
                        sel: s,
                    })
                }
            } else if let Some(i) = near_point(pt) {
                if in_sel && sel.is_some_and(|s| in_range(&pts, s.from, s.to).len() > 1) {
                    Some(Drag::Move {
                        orig: pts.clone(),
                        beat,
                        y: pt.y,
                        sel: sel.unwrap_or(Sel {
                            strip,
                            target,
                            from: 0.0,
                            to: 0.0,
                        }),
                    })
                } else {
                    v.lanes.sel = None;
                    Some(Drag::Point {
                        index: i,
                        beat: pts[i].beat,
                        value: pts[i].value,
                        y: pt.y,
                    })
                }
            } else if !pts.is_empty() && line_distance(&line, pt) <= LINE_GRAB {
                let i = pts.partition_point(|q| q.beat <= beat);
                if mods.alt && i > 0 && i < pts.len() {
                    let rising = pts[i].value >= pts[i - 1].value;
                    Some(Drag::Curve {
                        index: i - 1,
                        curve: pts[i - 1].curve,
                        y: pt.y,
                        rising,
                    })
                } else {
                    let (a, b) = if i == 0 {
                        (0, 0)
                    } else if i >= pts.len() {
                        (pts.len() - 1, pts.len() - 1)
                    } else {
                        (i - 1, i)
                    };
                    Some(Drag::Segment {
                        a,
                        b,
                        va: pts[a].value,
                        vb: pts[b].value,
                        y: pt.y,
                    })
                }
            } else if in_sel {
                Some(Drag::Move {
                    orig: pts.clone(),
                    beat,
                    y: pt.y,
                    sel: sel.unwrap_or(Sel {
                        strip,
                        target,
                        from: 0.0,
                        to: 0.0,
                    }),
                })
            } else {
                Some(Drag::Marquee { anchor: beat })
            };
            v.lanes.drag = drag.map(|d| (strip, target, d));
        }
    }
    let ours = v
        .lanes
        .drag
        .as_ref()
        .is_some_and(|(s, t, _)| (*s, *t) == key);
    if resp.dragged() && ours {
        if let (Some(pt), Some((_, _, drag))) = (resp.interact_pointer_pos(), v.lanes.drag.clone())
        {
            let beat = beat_of(pt.x);
            // Shift: a tenth of the pointer's travel, for fine values.
            let fine = |y0: f32| {
                if mods.shift {
                    y0 + (pt.y - y0) * 0.1
                } else {
                    pt.y
                }
            };
            let plugin = plugin_ready(p, v, strip, target);
            let Some(a) = automation::auto_of_mut(p, strip) else {
                return;
            };
            if let Some((d, id)) = plugin {
                a.plugin_lane(&d, &id);
            }
            match drag {
                Drag::Point {
                    index,
                    beat: b0,
                    value,
                    y,
                } => {
                    let y_now = fine(y);
                    let v_new = value_at_y(r, target, y_of(r, target, value) + (y_now - y));
                    let b = if mods.shift { b0 } else { beat.max(0.0) };
                    move_point(a.lane_mut(target), index, b, v_new);
                }
                Drag::Segment {
                    a: ia,
                    b: ib,
                    va,
                    vb,
                    y,
                } => {
                    let dy = fine(y) - y;
                    let pts = a.lane_mut(target);
                    for (i, v0) in [(ia, va), (ib, vb)] {
                        if let Some(q) = pts.get_mut(i) {
                            q.value = value_at_y(r, target, y_of(r, target, v0) + dy);
                        }
                    }
                }
                Drag::Curve {
                    index,
                    curve,
                    y,
                    rising,
                } => {
                    // Up bends a rising segment towards a fast start, as if pulling it up.
                    let d = (fine(y) - y) / 60.0;
                    let c = if rising { curve + d } else { curve - d };
                    bend(a.lane_mut(target), index, c);
                }
                Drag::Marquee { anchor } => {
                    let (from, to) = if beat < anchor {
                        (beat, anchor)
                    } else {
                        (anchor, beat)
                    };
                    v.lanes.sel = Some(Sel {
                        strip,
                        target,
                        from: from.max(0.0),
                        to,
                    });
                }
                Drag::Move {
                    orig,
                    beat: b0,
                    y,
                    sel,
                } => {
                    // Shift moves only the values; values move by the lane's height (a dB
                    // fader lane is not even in dB).
                    let db = if mods.shift { 0.0 } else { beat - b0 };
                    let f = (y - fine(y)) / (r.height() - 2.0 * PAD).max(1.0);
                    let lifted: Vec<AutoPoint> = orig
                        .iter()
                        .map(|q| {
                            if q.beat >= sel.from - 1e-9 && q.beat <= sel.to + 1e-9 {
                                let h = automation::frac_of(target, q.value) + f;
                                AutoPoint {
                                    value: automation::value_of(target, h),
                                    ..*q
                                }
                            } else {
                                *q
                            }
                        })
                        .collect();
                    *a.lane_mut(target) = move_range(&lifted, sel.from, sel.to, db, 0.0, target);
                    let db = db.max(-sel.from);
                    v.lanes.sel = Some(Sel {
                        from: sel.from + db,
                        to: sel.to + db,
                        ..sel
                    });
                }
                Drag::Stretch { orig, sel } => {
                    let to = beat.max(sel.from + 1.0 / 64.0);
                    *a.lane_mut(target) = stretch_range(&orig, sel.from, sel.to, to);
                    v.lanes.sel = Some(Sel { to, ..sel });
                }
                Drag::Scale { orig, y, sel } => {
                    // The handle's height over the lane's bottom, against where it started.
                    let k = (r.bottom() - fine(y)).max(0.0) / (r.bottom() - y).max(1.0);
                    *a.lane_mut(target) = scale_range(&orig, sel.from, sel.to, k, target);
                }
                Drag::Draw { last } => {
                    let val = value_at_y(r, target, pt.y);
                    let pts = a.lane_mut(target);
                    if grid.free || mods.command {
                        draw_free(pts, last, beat.max(0.0), val);
                        v.lanes.drag = Some((
                            strip,
                            target,
                            Drag::Draw {
                                last: Some((beat.max(0.0), val)),
                            },
                        ));
                    } else {
                        let from = snap(beat.max(0.0));
                        draw_step(pts, from, from + grid.step, val);
                    }
                }
            }
        }
    }
    if resp.drag_stopped() && ours {
        if let Some((_, _, Drag::Marquee { .. })) = &v.lanes.drag {
            if v.lanes.sel.is_some_and(|s| x_of(s.to) - x_of(s.from) < 3.0) {
                v.lanes.sel = None;
            }
        }
        v.lanes.drag = None;
        if let Some(a) = automation::auto_of_mut(p, strip) {
            a.prune();
        }
    }
    if resp.clicked() {
        if let Some(pt) = resp.interact_pointer_pos() {
            let beat = beat_of(pt.x).max(0.0);
            if v.draw {
                // A click in Draw Mode draws one grid unit (or a point).
                let val = value_at_y(r, target, pt.y);
                let plugin = plugin_ready(p, v, strip, target);
                if let Some(a) = automation::auto_of_mut(p, strip) {
                    if let Some((d, id)) = plugin {
                        a.plugin_lane(&d, &id);
                    }
                    let from = snap(beat);
                    if grid.free {
                        add_point(a.lane_mut(target), beat, val);
                    } else {
                        draw_step(a.lane_mut(target), from, from + grid.step, val);
                    }
                }
            } else if near_point(pt).is_some() {
                // A click on a breakpoint takes it; nothing changes.
            } else if line_distance(&line, pt) <= LINE_GRAB {
                let plugin = plugin_ready(p, v, strip, target);
                if let Some(a) = automation::auto_of_mut(p, strip) {
                    if let Some((d, id)) = plugin {
                        a.plugin_lane(&d, &id);
                    }
                    add_on_line(a.lane_mut(target), beat, own);
                    v.lanes.made = Some((strip, target, ui.input(|i| i.time)));
                }
            } else {
                v.lanes.sel = None;
            }
        }
    }
    if resp.double_clicked() && !v.draw {
        if let Some(pt) = resp.interact_pointer_pos() {
            let beat = beat_of(pt.x).max(0.0);
            let made = v
                .lanes
                .made
                .take()
                .filter(|m| m.0 == strip && m.1 == target && ui.input(|i| i.time) - m.2 < 0.6);
            let plugin = plugin_ready(p, v, strip, target);
            if let Some(a) = automation::auto_of_mut(p, strip) {
                let pts_now = a.lane(target).to_vec();
                let hit = pts_now
                    .iter()
                    .enumerate()
                    .map(|(i, q)| (i, pos2(x_of(q.beat), y_of(r, target, q.value)).distance(pt)))
                    .filter(|(_, d)| *d <= GRAB)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i);
                match (hit, made) {
                    // The first click of this double-click made the breakpoint: keep it.
                    (_, Some(_)) => {}
                    (Some(i), None) => remove_point(a.lane_mut(target), i),
                    (None, None) => {
                        if let Some((d, id)) = plugin {
                            a.plugin_lane(&d, &id);
                        }
                        add_point(a.lane_mut(target), beat, value_at_y(r, target, pt.y));
                    }
                }
                a.prune();
            }
        }
    }
    let overridden_now = overridden;
    crate::widgets::context_menu(&resp, |ui| {
        if ui
            .add_enabled(!pts.is_empty(), egui::Button::new(tr("Delete Automation")))
            .clicked()
        {
            if let Some(a) = automation::auto_of_mut(p, strip) {
                a.clear_lane(target);
            }
            ui.close();
        }
        if ui
            .add_enabled(
                overridden_now,
                egui::Button::new(tr("Re-Enable Automation")),
            )
            .clicked()
        {
            if let Some(a) = automation::auto_of_mut(p, strip) {
                a.overridden.retain(|t| *t != target);
            }
            ui.close();
        }
    });
}

/// For a plug-in lane about to get breakpoints: its description and the plug-in's id.
fn plugin_ready(
    p: &Project,
    v: &AutoView,
    strip: Strip,
    target: AutoTarget,
) -> Option<(ParamDesc, String)> {
    let AutoTarget::Plugin { slot, .. } = target else {
        return None;
    };
    let id = automation::strip_plugin_slot(p, strip, slot)?.id.clone();
    Some((automation::describe(p, strip, target, &v.au)?, id))
}

/// A value tag next to a breakpoint.
fn tag(g: &Painter, lane: Rect, at: Pos2, text: &str) {
    // A small dark field, like the inspector's values.
    let galley = hw::galley(g, text, Face::Bold, 10.5, theme::ON_DARK);
    let size = galley.size() + vec2(10.0, 6.0);
    let mut r = Rect::from_min_size(at + vec2(6.0, -size.y - 4.0), size);
    if r.top() < lane.top() {
        r = r.translate(vec2(0.0, size.y + 8.0));
    }
    hw::paint_field(g, r, "", false, false);
    g.galley(r.min + vec2(5.0, 3.0), galley, theme::ON_DARK);
}

// ------------------------------------------------------------------ headers

/// The device and parameter choosers of a lane (Live's track header in Automation Mode), with
/// "+" (Show Automation in New Lane) or "−" (remove this lane) after them. `extra`: the lane's
/// index under the track, or `None` for the track's own lane.
#[allow(clippy::too_many_arguments)]
pub fn choosers(
    ui: &Ui,
    g: &Painter,
    p: &mut Project,
    v: &mut AutoView,
    strip: Strip,
    r: Rect,
    extra: Option<usize>,
) {
    let Some(a) = automation::auto_of(p, strip) else {
        return;
    };
    let target = match extra {
        Some(k) => match a.extra.get(k) {
            Some(t) => *t,
            None => return,
        },
        None => a.shown,
    };
    let devices = automation::devices(p, strip, &v.au);
    let (dev_i, desc) = devices
        .iter()
        .enumerate()
        .find_map(|(i, d)| {
            d.params
                .iter()
                .find(|x| x.target == target)
                .map(|x| (i, x.clone()))
        })
        .unwrap_or((0, devices[0].params[0].clone()));
    let automated = a.automated();
    let base = egui::Id::unique(("auto_choosers", strip, extra));
    // Device and parameter fields, an LED for the lane's state (lit: the automation plays;
    // dark: overridden), the value at the playhead on an LCD chip, then the + or − key.
    let fh = (r.height() - 6.0).min(22.0);
    let fy = r.center().y - fh / 2.0;
    let key_w = 27.0;
    let chip_w = if r.width() > 300.0 { 66.0 } else { 0.0 };
    let w = r.width() - 4.0 - key_w - 6.0 - chip_w - if chip_w > 0.0 { 6.0 } else { 0.0 } - 18.0;
    let dev_r = Rect::from_min_size(pos2(r.left() + 4.0, fy), vec2(w * 0.42, fh));
    let par_r = Rect::from_min_size(pos2(dev_r.right() + 18.0, fy), vec2(w * 0.58, fh));
    if automated.contains(&target) {
        led(g, pos2(par_r.left() - 9.0, par_r.center().y), a, target);
    }
    if chip_w > 0.0 {
        let chip = Rect::from_min_size(pos2(par_r.right() + 6.0, fy), vec2(chip_w, fh));
        let own = match strip {
            Strip::Track(i) => p
                .tracks
                .get(i)
                .and_then(|t| automation::own_value(t, target)),
            Strip::Master => automation::master_value(p, target),
        };
        let now = lane_value(a.lane(target), p.position_beats).or(own);
        hw::lcd_glass(g, chip);
        if let Some(value) = now {
            let text = automation::value_text(&desc, value).to_uppercase();
            let shown = hw::fit(g, &text, Face::Lcd, 10.0, chip.width() - 8.0);
            hw::text(
                g,
                pos2(chip.right() - 5.0, chip.center().y),
                egui::Align2::RIGHT_CENTER,
                &shown,
                Face::Lcd,
                10.0,
                theme::LCD_INK,
            );
        }
    }
    let mut pick: Option<AutoTarget> = None;
    let resp = popup_button(ui, g, dev_r, base.with("dev"), &devices[dev_i].name);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(180.0);
        for d in &devices {
            let lit = d.params.iter().any(|x| automated.contains(&x.target));
            if ui
                .add(egui::Button::selectable(
                    d.name == devices[dev_i].name,
                    chooser_text(&d.name, lit),
                ))
                .clicked()
            {
                // Its first automated parameter, or its first.
                pick = d
                    .params
                    .iter()
                    .find(|x| automated.contains(&x.target))
                    .or(d.params.first())
                    .map(|x| x.target);
                ui.close();
            }
        }
    });
    let resp = popup_button(ui, g, par_r, base.with("par"), &desc.name);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(200.0);
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for x in &devices[dev_i].params {
                    let lit = automated.contains(&x.target);
                    if ui
                        .add(egui::Button::selectable(
                            x.target == target,
                            chooser_text(&x.name, lit),
                        ))
                        .clicked()
                    {
                        pick = Some(x.target);
                        ui.close();
                    }
                }
            });
    });
    // "+" shows another parameter in a lane of its own; "−" takes this lane away.
    let btn = Rect::from_min_size(
        pos2(r.right() - key_w - 2.0, r.center().y - 11.0),
        vec2(key_w, 22.0_f32.min(r.height() - 2.0)),
    );
    let (b, face) = hw::key(
        ui,
        g,
        btn,
        base.with("btn"),
        Cap::Bone,
        false,
        None,
        Legend::Custom,
    );
    // A plus or a minus printed as bars (the font's minus sits off centre).
    let c = face.center();
    let st = Stroke::new(1.6, theme::INK);
    g.hline(c.x - 4.0..=c.x + 4.0, c.y, st);
    if extra.is_none() {
        g.vline(c.x, c.y - 4.0..=c.y + 4.0, st);
    }
    let b = b.on_hover_text(if extra.is_some() {
        tr("Remove Lane")
    } else {
        tr("Show Automation in New Lane")
    });
    let Some(a) = automation::auto_of_mut(p, strip) else {
        return;
    };
    if let Some(t) = pick {
        match extra {
            Some(k) => {
                if !a.extra.contains(&t) && t != a.shown {
                    a.extra[k] = t;
                }
            }
            None => a.shown = t,
        }
        v.lanes.sel = None;
    }
    if b.clicked() {
        match extra {
            Some(k) => {
                a.extra.remove(k);
            }
            None => {
                // The next parameter not shown yet: an automated one first, as Live does.
                let all: Vec<AutoTarget> = devices
                    .iter()
                    .flat_map(|d| d.params.iter().map(|x| x.target))
                    .collect();
                let shown: Vec<AutoTarget> = std::iter::once(a.shown)
                    .chain(a.extra.iter().copied())
                    .collect();
                let next = automated
                    .iter()
                    .chain(all.iter())
                    .find(|t| !shown.contains(t))
                    .copied();
                if let Some(t) = next {
                    a.extra.push(t);
                }
            }
        }
    }
}

fn chooser_text(name: &str, automated: bool) -> egui::RichText {
    if automated {
        egui::RichText::new(format!("• {name}")).color(LED_ON)
    } else {
        egui::RichText::new(format!("   {name}"))
    }
}

// ------------------------------------------------------------------ toolbar and keys

/// A key of the automation toolbar: latched while on, with an LED at its left when it has one
/// (`led`); `orange` gives it the signal cap while on (Automation Arm). Greyed and inert when
/// not `enabled`. Returns whether it was clicked.
#[allow(clippy::too_many_arguments)]
fn toggle(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    id: &str,
    on: bool,
    orange: bool,
    led: bool,
    text: &str,
    tip: &str,
    enabled: bool,
) -> bool {
    if !enabled {
        let face = hw::paint_key(g, r, Cap::Bone, false, false);
        if led {
            hw::led_sized(g, pos2(face.left() + 8.0, face.center().y), 2.5, false);
        }
        let _ = ui
            .interact(r, egui::Id::unique(("auto_tool", id)), Sense::hover())
            .on_hover_text(tip);
        return false;
    }
    let cap = if on && orange { Cap::Orange } else { Cap::Bone };
    let (resp, _) = hw::key(
        ui,
        g,
        r,
        egui::Id::unique(("auto_tool", id)),
        cap,
        on,
        led.then_some(on),
        if text.is_empty() {
            Legend::Custom
        } else {
            Legend::Text(text)
        },
    );
    resp.on_hover_text(tip).clicked()
}

/// The automation buttons of the Tracks area's toolbar, from `x`: Automation Mode (A), Draw
/// Mode (B), Automation Arm and Re-Enable Automation. Returns their width.
pub fn toolbar(ui: &Ui, g: &Painter, p: &mut Project, v: &mut AutoView, x: f32, cy: f32) -> f32 {
    let mut at = x;
    let mut next = |w: f32| {
        let r = Rect::from_min_size(pos2(at, cy - 14.0), vec2(w, 28.0));
        at += w + 6.0;
        r
    };
    let r = next(44.0);
    if toggle(
        ui,
        g,
        r,
        "mode",
        v.mode,
        false,
        true,
        "A",
        tr("Automation Mode (A)"),
        true,
    ) {
        set_mode(p, v, !v.mode);
    }
    let r = next(44.0);
    if toggle(
        ui,
        g,
        r,
        "draw",
        v.draw,
        false,
        true,
        "B",
        tr("Draw Mode (B)"),
        true,
    ) {
        v.draw = !v.draw;
    }
    let r = next(58.0);
    if toggle(
        ui,
        g,
        r,
        "arm",
        v.arm,
        true,
        true,
        "ARM",
        tr("Automation Arm"),
        true,
    ) {
        v.arm = !v.arm;
    }
    let any = automation::any_overridden(p);
    // A normal key whose LED is lit while some envelope is overridden; it does nothing
    // otherwise.
    let r = next(44.0);
    let (resp, _) = hw::key(
        ui,
        g,
        r,
        egui::Id::unique(("auto_tool", "reenable")),
        Cap::Bone,
        false,
        Some(any),
        Legend::Custom,
    );
    if resp.on_hover_text(tr("Re-Enable Automation")).clicked() && any {
        automation::reenable(p);
    }
    // Live's Re-Enable Automation icon: an arrow turning back.
    let c = theme::INK;
    let (centre, rad) = (r.center() + vec2(4.0, 0.5), 6.0);
    let arc: Vec<Pos2> = (0..=12)
        .map(|k| {
            let a = std::f32::consts::PI * (0.15 + 1.45 * k as f32 / 12.0);
            centre + vec2(a.cos(), -a.sin()) * rad
        })
        .collect();
    let tip = arc[arc.len() - 1];
    g.add(egui::Shape::line(arc, Stroke::new(1.4, c)));
    g.add(egui::Shape::convex_polygon(
        vec![
            tip + vec2(-3.0, -2.5),
            tip + vec2(3.0, -1.0),
            tip + vec2(-0.5, 3.0),
        ],
        c,
        Stroke::NONE,
    ));
    at - x
}

/// Turns Automation Mode on or off.
pub fn set_mode(p: &mut Project, v: &mut AutoView, on: bool) {
    v.mode = on;
    v.lanes.sel = None;
    if on {
        lay_out(p, v);
    }
}

/// Keys in the Tracks area: A (Automation Mode), B (Draw Mode), and with a range selected on a
/// lane Delete, Command-C, Command-V and Command-D. Returns whether a key was used (the Tracks
/// area then leaves it alone).
pub fn keys(ui: &Ui, p: &mut Project, v: &mut AutoView) -> bool {
    let (a, b) = ui.input_mut(|i| {
        let plain = |i: &egui::InputState| {
            !i.modifiers.command && !i.modifiers.alt && !i.modifiers.ctrl && !i.modifiers.shift
        };
        let a = plain(i) && i.key_pressed(Key::A);
        let b = plain(i) && i.key_pressed(Key::B);
        (a, b)
    });
    if a {
        set_mode(p, v, !v.mode);
    }
    if b {
        v.draw = !v.draw;
    }
    let used = a || b;
    let Some(sel) = v.lanes.sel.filter(|_| v.mode) else {
        return used;
    };
    let (del, copy, paste, dup) = ui.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::Delete)
                || i.consume_key(Modifiers::NONE, Key::Backspace),
            i.consume_key(Modifiers::COMMAND, Key::C),
            i.consume_key(Modifiers::COMMAND, Key::V),
            i.consume_key(Modifiers::COMMAND, Key::D),
        )
    });
    let playhead = p.position_beats;
    let Some(auto) = automation::auto_of_mut(p, sel.strip) else {
        return used;
    };
    let pts = auto.lane_mut(sel.target);
    if copy {
        v.lanes.clip = copy_piece(pts, sel.target, sel.from, sel.to);
    }
    if del {
        let r = in_range(pts, sel.from, sel.to);
        pts.drain(r);
    }
    if paste {
        if let Some(piece) = v
            .lanes
            .clip
            .clone()
            .filter(|c| c.target == sel.target || same_kind(c.target, sel.target))
        {
            paste_piece(pts, &piece, playhead);
            v.lanes.sel = Some(Sel {
                from: playhead,
                to: playhead + piece.len,
                ..sel
            });
        }
    }
    if dup {
        if let Some(piece) = copy_piece(pts, sel.target, sel.from, sel.to) {
            paste_piece(pts, &piece, sel.to);
            v.lanes.sel = Some(Sel {
                from: sel.to,
                to: sel.to + piece.len,
                ..sel
            });
        }
    }
    auto.prune();
    used || del || copy || paste || dup
}

/// Whether a piece copied from one lane can be pasted on another (the same kind of values).
fn same_kind(a: AutoTarget, b: AutoTarget) -> bool {
    Automation::range(a) == Automation::range(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(v: &[(f64, f32)]) -> Vec<AutoPoint> {
        v.iter().map(|(b, x)| AutoPoint::new(*b, *x)).collect()
    }

    #[test]
    fn breakpoints_are_added_on_the_line_moved_bent_and_removed() {
        let mut l = pts(&[(0.0, 0.0), (4.0, 1.0)]);
        let i = add_on_line(&mut l, 2.0, 0.5);
        assert_eq!((i, l[1].value), (1, 0.5));
        // Moving keeps it between its neighbours.
        move_point(&mut l, 1, 9.0, 0.25);
        assert_eq!(l[1], AutoPoint::new(4.0, 0.25));
        move_point(&mut l, 1, 1.0, 0.25);
        bend(&mut l, 0, 0.8);
        assert_eq!(l[0].curve, 0.8);
        bend(&mut l, 2, 0.8);
        assert_eq!(l[2].curve, 0.0, "the last breakpoint starts no segment");
        // A breakpoint added in a bent segment keeps its bend.
        add_point(&mut l, 0.5, 0.1);
        assert_eq!(l[1].curve, 0.8);
        remove_point(&mut l, 1);
        remove_point(&mut l, 9);
        assert_eq!(l.len(), 3);
        // With no breakpoints a click on the line starts the envelope at the control's value.
        let mut empty = Vec::new();
        add_on_line(&mut empty, 3.0, -6.0);
        assert_eq!(empty, pts(&[(3.0, -6.0)]));
    }

    #[test]
    fn draw_mode_makes_steps_on_the_grid_and_lines_without_it() {
        let mut l = pts(&[(0.0, 0.5), (8.0, 0.5)]);
        draw_step(&mut l, 2.0, 3.0, 1.0);
        draw_step(&mut l, 3.0, 4.0, 0.0);
        // Each grid unit holds its value, with jumps between them.
        for (b, want) in [
            (1.9, 0.5),
            (2.0, 1.0),
            (2.5, 1.0),
            (2.99, 1.0),
            (3.0, 0.0),
            (3.5, 0.0),
            (4.01, 0.5),
        ] {
            let got = lane_value(&l, b).unwrap_or(f32::NAN);
            assert!((got - want).abs() < 1e-3, "{b}: {got} {want} {l:?}");
        }
        // Drawn again over the same unit, it is replaced.
        draw_step(&mut l, 2.0, 3.0, 0.25);
        assert_eq!(lane_value(&l, 2.5), Some(0.25));
        assert_eq!(lane_value(&l, 3.5), Some(0.0));
        // Freehand: a line from where the pointer was, replacing what lay between.
        let mut f = pts(&[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)]);
        draw_free(&mut f, None, 0.5, 0.2);
        draw_free(&mut f, Some((0.5, 0.2)), 1.5, 0.6);
        assert!((lane_value(&f, 1.0).unwrap_or(0.0) - 0.4).abs() < 1e-6);
        assert!(!f.iter().any(|q| q.beat == 1.0));
    }

    #[test]
    fn a_selected_range_moves_stretches_scales_and_copies() {
        let l = pts(&[(0.0, 0.0), (1.0, 0.5), (2.0, 1.0), (4.0, 0.0)]);
        let moved = move_range(
            &l,
            0.9,
            2.1,
            1.0,
            -0.25,
            AutoTarget::Fx { slot: 0, param: 0 },
        );
        assert_eq!(
            moved,
            pts(&[(0.0, 0.0), (2.0, 0.25), (3.0, 0.75), (4.0, 0.0)])
        );
        let st = stretch_range(&l, 0.0, 2.0, 4.0);
        assert_eq!(
            st,
            pts(&[(0.0, 0.0), (2.0, 0.5), (4.0, 1.0)]),
            "what it covers is replaced"
        );
        let sc = scale_range(&l, 0.5, 2.5, 0.5, AutoTarget::Fx { slot: 0, param: 0 });
        assert_eq!((sc[1].value, sc[2].value, sc[3].value), (0.25, 0.5, 0.0));

        let piece = copy_piece(&l, AutoTarget::Mute, 1.5, 3.0).unwrap_or_else(|| panic!("piece"));
        assert_eq!(piece.len, 1.5);
        assert_eq!(piece.points.first().map(|q| q.value), Some(0.75));
        let mut dst = pts(&[(0.0, 0.2), (10.0, 0.2)]);
        paste_piece(&mut dst, &piece, 4.0);
        assert_eq!(lane_value(&dst, 4.0), Some(0.75));
        assert_eq!(lane_value(&dst, 4.5), Some(1.0));
        assert!((lane_value(&dst, 5.5).unwrap_or(0.0) - 0.0).abs() < 1e-6 + 0.5 * 1.0);
        assert_eq!(
            lane_value(&dst, 3.99),
            Some(0.2),
            "the envelope before stays"
        );
        assert_eq!(lane_value(&dst, 5.6), Some(0.2), "and after");
        clear_range(&mut dst, 4.0, 5.5);
        assert_eq!(lane_value(&dst, 4.7), Some(0.2));
    }

    #[test]
    fn automation_moves_and_copies_with_regions() {
        use crate::model::{Region, TrackKind};
        let mut p = Project::empty();
        p.tracks[0].kind = TrackKind::Instrument;
        p.tracks[0].regions = vec![Region {
            missing: None,
            name: "A".into(),
            start_bar: 1.0,
            len_bars: 1.0,
            notes: Vec::new(),
            controls: Vec::new(),
            folder: None,
            generated: None,
            audio: None,
            params: crate::model::RegionParams::default(),
        }];
        let len = p.region_len_beats(&p.tracks[0].regions[0]);
        let a = &mut p.tracks[0].auto;
        a.volume = pts(&[(1.0, -12.0), (2.0, -3.0)]);
        let v = AutoView::default();
        // Dragged one bar later: its automation goes with it.
        p.tracks[0].regions[0].start_bar = 2.0;
        follow_moves(&mut p, &v, &[(0, 0, 0.0)]);
        let to = p.region_start_beats(&p.tracks[0].regions[0]);
        assert_eq!(lane_value(&p.tracks[0].auto.volume, to + 1.0), Some(-12.0));
        assert_eq!(lane_value(&p.tracks[0].auto.volume, to + 2.0), Some(-3.0));
        // Copy and paste at the playhead carry it too; with Lock Envelopes they would not.
        let mut v = AutoView::default();
        copy_with_regions(&p, &mut v, &[(0, 0)]);
        assert_eq!(v.lanes.with_regions.len(), 1);
        p.selected_track = 0;
        paste_with_regions(&mut p, &v, 16.0);
        assert_eq!(lane_value(&p.tracks[0].auto.volume, 17.0), Some(-12.0));
        follow_repeats(&mut p, &v, &[(0, to, len)]);
        assert_eq!(
            lane_value(&p.tracks[0].auto.volume, to + len + 1.0),
            Some(-12.0)
        );
        let mut locked = AutoView::default();
        locked.with_regions = false;
        let before = p.tracks[0].auto.volume.clone();
        p.tracks[0].regions[0].start_bar = 9.0;
        follow_moves(&mut p, &locked, &[(0, 0, to)]);
        assert_eq!(p.tracks[0].auto.volume, before);
        cut_with_regions(&mut p, &v);
    }

    /// The Tracks area in a 1400 x 600 window, with its first track's own lane: where it is,
    /// and the y of `value` on it.
    fn main_lane() -> Rect {
        // The menu row and the ruler, then a 70-point track and the 22-point chooser strip;
        // regions sit 4 points in from the row edges and the lane starts under their
        // 15-point name strips.
        use crate::tracks_area::{HEADER_W, RULER_H, TOOLBAR_H};
        let top = TOOLBAR_H + RULER_H;
        Rect::from_min_max(
            pos2(HEADER_W, top + 4.0 + 15.0),
            pos2(1400.0, top + 70.0 + 22.0 - 4.0),
        )
    }

    #[test]
    fn the_tracks_area_toggles_automation_mode_and_edits_an_envelope() {
        use crate::mixer::Meters;
        use crate::ui_test::Harness;
        use egui::PointerButton::Primary;
        let mut p = Project::empty();
        p.position_beats = 0.0;
        let mut s = crate::UiState::new();
        let meters = Meters::default();
        let mut h = Harness::new(1400.0, 600.0);
        macro_rules! show {
            () => {
                |ui: &mut Ui| crate::tracks_area::show(ui, &mut p, &mut s, &meters)
            };
        }
        h.settle(show!());
        // A: Automation Mode.
        h.key(Key::A, show!());
        assert!(s.auto.mode);
        h.settle(show!());
        let lane = main_lane();
        let y0 = y_of(lane, AutoTarget::Volume, 0.0);
        // A click on the line (the fader's value, 0 dB) makes a breakpoint there.
        let at = pos2(600.0, y0);
        h.click(at, show!());
        assert_eq!(
            p.tracks[0].auto.volume.len(),
            1,
            "{:?}",
            p.tracks[0].auto.volume
        );
        assert!(p.tracks[0].auto.volume[0].value.abs() < 0.2);
        // Dragging it down lowers it.
        let down = pos2(600.0, y0 + 25.0);
        h.time_passes(1.0);
        h.drag(&[at, pos2(600.0, y0 + 10.0), down], Primary, show!());
        let v = p.tracks[0].auto.volume[0].value;
        assert!(v < -3.0, "{v}");
        // Double-click removes it.
        h.time_passes(1.0);
        h.click(down, show!());
        h.click(down, show!());
        assert!(
            p.tracks[0].auto.volume.is_empty(),
            "{:?}",
            p.tracks[0].auto.volume
        );
        // B: Draw Mode draws steps on the grid.
        h.key(Key::B, show!());
        assert!(s.auto.draw);
        let path = [
            pos2(500.0, lane.top() + 10.0),
            pos2(560.0, lane.top() + 10.0),
            pos2(640.0, lane.bottom() - 10.0),
        ];
        h.drag(&path, Primary, show!());
        assert!(
            p.tracks[0].auto.volume.len() >= 4,
            "{:?}",
            p.tracks[0].auto.volume
        );
        // Re-Enable Automation lights up when an envelope is overridden, and clears it.
        p.tracks[0].auto.set_point(AutoTarget::Pan, 2.0, 0.5);
        p.tracks[0].auto.set_point(AutoTarget::Pan, 8.0, -0.5);
        p.tracks[0].auto.overridden.push(AutoTarget::Pan);
        p.tracks[0].auto.extra.push(AutoTarget::Pan);
        s.auto.draw = false;
        // The Stereo Out's lane under the tracks.
        p.show_master_track = true;
        p.master_auto.set_point(AutoTarget::Volume, 0.0, -6.0);
        p.master_auto.set_point(AutoTarget::Volume, 12.0, 0.0);
        // The mixer: the LEDs of the automated fader, pan and mute (grey: overridden).
        p.tracks[0].auto.set_point(AutoTarget::Mute, 1.0, 1.0);
        let mixer_png = std::env::temp_dir().join("prowave-automation-mixer.png");
        crate::ui_test::render_png(
            700.0,
            520.0,
            1.0,
            Color32::from_gray(0x2a),
            |ui| crate::mixer::show(ui, &mut p, &mut s, &meters),
            &mixer_png,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let out = std::env::temp_dir().join("prowave-automation-lanes.png");
        crate::ui_test::render_png(1400.0, 600.0, 1.0, Color32::from_gray(0x2a), show!(), &out)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(out.exists());
        // A again: back to the regions.
        h.key(Key::A, show!());
        assert!(!s.auto.mode);
    }
}
