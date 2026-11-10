//! MIDI Draw in the Piano Roll (Logic Pro: the Automation/MIDI area, region automation,
//! <https://support.apple.com/guide/logicpro/automationmidi-area-in-the-piano-roll-editor-lgcpa90a61bf/mac>):
//! lanes for controllers (Modulation, Volume, Expression, the Sustain pedal), pitch bend and
//! channel pressure, kept in the region's `controls` and played by the engine.
//!
//! The Pencil draws: a drag draws freehand, Shift-drag a straight line, a click puts one
//! point. The Pointer edits points: a click on empty space adds one, a drag moves one (time
//! and value), Delete removes the selected one, a right-click removes the one under it. The
//! Eraser removes the points it passes.
use crate::i18n::tr;
use crate::model::{Control, CHANNEL_PRESSURE, PITCH_BEND};

/// The beats between two points a drawn line or freehand stroke makes (a 1/32 note).
pub const DRAW_STEP: f32 = 0.125;

/// What the lane under the notes shows and edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lane {
    /// The notes' velocities (the lane it always had).
    Velocity,
    /// A controller, 0-127.
    Cc(u8),
    /// Pitch bend, 0-16383 with the centre at 8192.
    PitchBend,
    /// Channel pressure (aftertouch), 0-127.
    Pressure,
}

impl Lane {
    /// The lanes offered, in menu order.
    pub const ALL: [Lane; 8] = [
        Lane::Velocity,
        Lane::Cc(1),
        Lane::Cc(7),
        Lane::Cc(10),
        Lane::Cc(11),
        Lane::Cc(64),
        Lane::PitchBend,
        Lane::Pressure,
    ];

    pub fn name(self) -> String {
        match self {
            Lane::Velocity => tr("Note Velocity").into(),
            Lane::Cc(1) => tr("Modulation (CC 1)").into(),
            Lane::Cc(7) => tr("Volume (CC 7)").into(),
            Lane::Cc(10) => tr("Pan (CC 10)").into(),
            Lane::Cc(11) => tr("Expression (CC 11)").into(),
            Lane::Cc(64) => tr("Sustain Pedal (CC 64)").into(),
            Lane::Cc(n) => format!("CC {n}"),
            Lane::PitchBend => tr("Pitch Bend").into(),
            Lane::Pressure => tr("Channel Pressure").into(),
        }
    }

    /// The `Control::num` of the lane's events (none for Velocity).
    pub fn num(self) -> Option<u8> {
        match self {
            Lane::Velocity => None,
            Lane::Cc(n) => Some(n.min(127)),
            Lane::PitchBend => Some(PITCH_BEND),
            Lane::Pressure => Some(CHANNEL_PRESSURE),
        }
    }

    pub fn max(self) -> u16 {
        if self == Lane::PitchBend {
            16383
        } else {
            127
        }
    }

    /// The value a lane rests at with no events (and the line drawn across it).
    pub fn rest(self) -> u16 {
        match self {
            Lane::PitchBend => 8192,
            Lane::Cc(7) => 127,
            Lane::Cc(10) => 64,
            Lane::Cc(11) => 127,
            _ => 0,
        }
    }

    /// An on/off controller (the Sustain pedal): values are 0 or 127.
    pub fn switch(self) -> bool {
        matches!(self, Lane::Cc(64..=69))
    }

    /// `v` as the lane stores it: within range, and 0 or 127 for a switch.
    pub fn clamp(self, v: f32) -> u16 {
        let v = v.round().clamp(0.0, self.max() as f32) as u16;
        if self.switch() {
            if v >= 64 {
                127
            } else {
                0
            }
        } else {
            v
        }
    }

    /// The value as Logic shows it: pitch bend from -8192 to 8191, the others as they are.
    pub fn shown(self, v: u16) -> i32 {
        if self == Lane::PitchBend {
            v as i32 - 8192
        } else {
            v as i32
        }
    }
}

fn val(c: &Control) -> u16 {
    c.val
}

fn make(start: f32, num: u8, v: u16) -> Control {
    Control { start, num, val: v }
}

/// Indices of the lane's events, by time.
pub fn points(controls: &[Control], num: u8) -> Vec<usize> {
    let mut out: Vec<usize> = (0..controls.len())
        .filter(|&i| controls[i].num == num)
        .collect();
    out.sort_by(|a, b| controls[*a].start.total_cmp(&controls[*b].start));
    out
}

/// The lane's value at `beat`: the last event at or before it (controllers hold their value),
/// or `rest` before the first.
pub fn value_at(controls: &[Control], num: u8, beat: f32, rest: u16) -> u16 {
    controls
        .iter()
        .filter(|c| c.num == num && c.start <= beat + 1e-6)
        .max_by(|a, b| a.start.total_cmp(&b.start))
        .map_or(rest, val)
}

/// Keeps the events in time order (a stable sort, so the bytes of a SysEx message at one
/// time keep their order).
fn tidy(controls: &mut [Control]) {
    controls.sort_by(|a, b| a.start.total_cmp(&b.start));
}

/// Puts one point at `beat` with value `v`, replacing a point of the lane at the same place.
/// Returns its index.
pub fn put(controls: &mut Vec<Control>, lane: Lane, beat: f32, v: f32) -> Option<usize> {
    let num = lane.num()?;
    let beat = beat.max(0.0);
    let v = lane.clamp(v);
    if let Some(c) = controls
        .iter_mut()
        .find(|c| c.num == num && (c.start - beat).abs() < 1e-4)
    {
        *c = make(beat, num, v);
    } else {
        controls.push(make(beat, num, v));
    }
    tidy(controls);
    controls
        .iter()
        .position(|c| c.num == num && (c.start - beat).abs() < 1e-4)
}

/// Draws a straight line from (`b0`, `v0`) to (`b1`, `v1`): the lane's events between the two
/// are replaced by points every `step` beats along the line (a switch lane gets only the
/// points where it changes). Freehand drawing is this from one frame's point to the next.
pub fn draw_line(
    controls: &mut Vec<Control>,
    lane: Lane,
    (b0, v0): (f32, f32),
    (b1, v1): (f32, f32),
    step: f32,
) {
    let Some(num) = lane.num() else { return };
    let ((b0, v0), (b1, v1)) = if b0 <= b1 {
        ((b0.max(0.0), v0), (b1.max(0.0), v1))
    } else {
        ((b1.max(0.0), v1), (b0.max(0.0), v0))
    };
    controls.retain(|c| !(c.num == num && c.start >= b0 - 1e-6 && c.start <= b1 + 1e-6));
    let step = step.max(1.0 / 960.0);
    let n = ((b1 - b0) / step).ceil().max(0.0) as usize;
    let mut last: Option<u16> = None;
    for k in 0..=n {
        let b = (b0 + k as f32 * step).min(b1);
        let t = if b1 > b0 { (b - b0) / (b1 - b0) } else { 1.0 };
        let v = lane.clamp(v0 + (v1 - v0) * t);
        if last == Some(v) && k != n {
            continue;
        }
        if last == Some(v) {
            break;
        }
        controls.push(make(b, num, v));
        last = Some(v);
        if b >= b1 {
            break;
        }
    }
    tidy(controls);
}

/// Removes the lane's events between two beats (Eraser).
pub fn erase(controls: &mut Vec<Control>, lane: Lane, b0: f32, b1: f32) -> usize {
    let Some(num) = lane.num() else { return 0 };
    let (lo, hi) = (b0.min(b1), b0.max(b1));
    let before = controls.len();
    controls.retain(|c| !(c.num == num && c.start >= lo && c.start <= hi));
    before - controls.len()
}

/// The lane's point nearest to `beat` within `tol` beats (and to the value, for ties).
pub fn hit(controls: &[Control], lane: Lane, beat: f32, tol: f32) -> Option<usize> {
    let num = lane.num()?;
    (0..controls.len())
        .filter(|&i| controls[i].num == num && (controls[i].start - beat).abs() <= tol)
        .min_by(|a, b| {
            (controls[*a].start - beat)
                .abs()
                .total_cmp(&(controls[*b].start - beat).abs())
        })
}

/// Moves point `i` to `beat` with value `v`, between its neighbours of the lane (it cannot
/// pass them). Returns its new index.
pub fn move_point(controls: &mut [Control], lane: Lane, i: usize, beat: f32, v: f32) -> usize {
    let Some(num) = lane.num() else { return i };
    let Some(cur) = controls.get(i).copied() else {
        return i;
    };
    let pts = points(controls, num);
    let at = pts.iter().position(|&k| k == i).unwrap_or(0);
    let lo = at
        .checked_sub(1)
        .map_or(0.0, |k| controls[pts[k]].start + 1.0 / 960.0);
    let hi = pts
        .get(at + 1)
        .map_or(f32::MAX, |&k| controls[k].start - 1.0 / 960.0);
    let beat = beat.clamp(lo, hi.max(lo)).max(0.0);
    controls[i] = make(beat, cur.num, lane.clamp(v));
    tidy(controls);
    controls
        .iter()
        .position(|c| c.num == num && c.start == beat)
        .unwrap_or(i)
}

/// A gesture in a controller lane.
#[derive(Default)]
pub enum LaneDrag {
    #[default]
    None,
    /// Pencil: the last point of a freehand stroke; with Shift a straight line from `from`,
    /// drawn again on the events as they were (`before`) every frame.
    Draw {
        last: (f32, f32),
        line: Option<((f32, f32), Vec<Control>)>,
    },
    /// Pointer: the point being moved.
    Move { i: usize },
    /// Eraser: the last beat passed.
    Erase { last: f32 },
}

/// What the lane editor needs of the Piano Roll's view: where beats are, and the tools.
pub struct LaneView<'a> {
    pub rect: egui::Rect,
    pub x_of: &'a dyn Fn(f32) -> f32,
    pub beat_at: &'a dyn Fn(f32) -> f32,
    /// The tool the pointer has (left-click or Command-click).
    pub tool: crate::piano_tools::Tool,
}

impl LaneView<'_> {
    fn value_at_y(&self, lane: Lane, y: f32) -> f32 {
        let r = self.rect.shrink2(egui::vec2(0.0, 3.0));
        (r.bottom() - y) / r.height() * lane.max() as f32
    }

    fn y_of(&self, lane: Lane, v: u16) -> f32 {
        let r = self.rect.shrink2(egui::vec2(0.0, 3.0));
        r.bottom() - v as f32 / lane.max() as f32 * r.height()
    }
}

/// Edits the lane with the pointer. Returns whether the events changed.
pub fn edit(
    ui: &egui::Ui,
    view: &LaneView,
    lane: Lane,
    controls: &mut Vec<Control>,
    drag: &mut LaneDrag,
    selected: &mut Option<usize>,
) -> bool {
    use crate::piano_tools::Tool;
    let Some(num) = lane.num() else {
        return false;
    };
    let resp = ui.interact(
        view.rect,
        egui::Id::unique(("midi_draw", "lane")),
        egui::Sense::click_and_drag(),
    );
    let (shift, down, hover, right) = ui.input(|i| {
        (
            i.modifiers.shift,
            i.pointer.primary_down(),
            i.pointer.hover_pos(),
            i.pointer.button_pressed(egui::PointerButton::Secondary),
        )
    });
    let tol = 6.0 / ((view.x_of)(1.0) - (view.x_of)(0.0)).max(1e-3);
    let mut changed = false;
    let at = |pt: egui::Pos2| ((view.beat_at)(pt.x).max(0.0), view.value_at_y(lane, pt.y));
    if right && resp.hovered() {
        if let Some(i) = hover.and_then(|pt| hit(controls, lane, at(pt).0, tol)) {
            controls.remove(i);
            *selected = None;
            changed = true;
        }
    }
    let press = resp
        .drag_started()
        .then(|| ui.input(|i| i.pointer.press_origin()))
        .flatten();
    if let Some(pt) = press {
        let (b, v) = at(pt);
        *drag = match view.tool {
            Tool::Pencil | Tool::Brush => {
                let line = shift.then(|| ((b, v), controls.clone()));
                if line.is_none() {
                    draw_line(controls, lane, (b, v), (b, v), DRAW_STEP);
                    changed = true;
                }
                LaneDrag::Draw { last: (b, v), line }
            }
            Tool::Eraser => {
                changed |= erase(controls, lane, b - tol, b + tol) > 0;
                LaneDrag::Erase { last: b }
            }
            _ => match hit(controls, lane, b, tol) {
                Some(i) => {
                    *selected = Some(i);
                    LaneDrag::Move { i }
                }
                None => LaneDrag::None,
            },
        };
    }
    if resp.dragged() {
        if let Some(pt) = resp.interact_pointer_pos() {
            let (b, v) = at(pt);
            match drag {
                LaneDrag::Draw { last, line: None } => {
                    draw_line(controls, lane, *last, (b, v), DRAW_STEP);
                    *last = (b, v);
                    changed = true;
                }
                LaneDrag::Draw {
                    line: Some((from, before)),
                    ..
                } => {
                    *controls = before.clone();
                    draw_line(controls, lane, *from, (b, v), DRAW_STEP);
                    changed = true;
                }
                LaneDrag::Move { i } => {
                    *i = move_point(controls, lane, *i, b, v);
                    *selected = Some(*i);
                    changed = true;
                }
                LaneDrag::Erase { last } => {
                    changed |= erase(controls, lane, *last, b) > 0;
                    *last = b;
                }
                LaneDrag::None => {}
            }
        }
    }
    if !down {
        *drag = LaneDrag::None;
    }
    if resp.clicked() {
        if let Some(pt) = resp.interact_pointer_pos() {
            let (b, v) = at(pt);
            match view.tool {
                Tool::Eraser => {
                    changed |= erase(controls, lane, b - tol, b + tol) > 0;
                }
                _ => match hit(controls, lane, b, tol) {
                    Some(i) if view.tool != Tool::Pencil => *selected = Some(i),
                    _ => {
                        *selected = put(controls, lane, b, v);
                        changed = true;
                    }
                },
            }
        }
    }
    if resp.hovered()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        if let Some(i) = selected
            .take()
            .filter(|i| controls.get(*i).is_some_and(|c| c.num == num))
        {
            controls.remove(i);
            changed = true;
        }
    }
    if selected.is_some_and(|i| controls.get(i).is_none_or(|c| c.num != num)) {
        *selected = None;
    }
    changed
}

/// Draws the lane: its rest line, the value held between events as steps, and the points.
pub fn draw(
    g: &egui::Painter,
    view: &LaneView,
    lane: Lane,
    controls: &[Control],
    selected: Option<usize>,
) {
    use crate::theme;
    use egui::{pos2, Stroke};
    let Some(num) = lane.num() else { return };
    let r = view.rect;
    // Drawn in the lane's dark window in the signal orange.
    let rest_y = view.y_of(lane, lane.rest());
    if lane == Lane::PitchBend {
        g.line_segment(
            [pos2(r.left(), rest_y), pos2(r.right(), rest_y)],
            Stroke::new(1.0, crate::hw::lerp(theme::METER_WELL, theme::ON_DARK, 0.3)),
        );
    }
    let pts = points(controls, num);
    let mut x = r.left();
    let mut y = view.y_of(
        lane,
        value_at(controls, num, (view.beat_at)(r.left()), lane.rest()),
    );
    let mut line = vec![pos2(x, y)];
    for &i in &pts {
        let px = (view.x_of)(controls[i].start);
        if px < r.left() {
            continue;
        }
        if px > r.right() {
            break;
        }
        let py = view.y_of(lane, val(&controls[i]));
        line.push(pos2(px, y));
        line.push(pos2(px, py));
        x = px;
        y = py;
    }
    line.push(pos2(r.right().max(x), y));
    // The area under the value as one mesh (no seams between the steps), orange fading to
    // the floor, then the line.
    let mut mesh = egui::epaint::Mesh::default();
    let fill_at = |y: f32| {
        let t = ((y - r.top()) / r.height().max(1.0)).clamp(0.0, 1.0);
        theme::ORANGE.gamma_multiply(0.25 + (0.05 - 0.25) * t)
    };
    for w in line.windows(2) {
        if w[1].x > w[0].x {
            let top = w[0].y.clamp(r.top(), r.bottom());
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(w[0].x, top), fill_at(top));
            mesh.colored_vertex(pos2(w[1].x, top), fill_at(top));
            mesh.colored_vertex(pos2(w[1].x, r.bottom()), fill_at(r.bottom()));
            mesh.colored_vertex(pos2(w[0].x, r.bottom()), fill_at(r.bottom()));
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
        }
    }
    g.add(egui::Shape::mesh(mesh));
    g.add(egui::Shape::line(line, Stroke::new(2.0, theme::ORANGE)));
    // Breakpoint handles (a bone ring round an orange core) where there is room for them;
    // a drawn ramp's dense points show only at its ends.
    let shown: Vec<usize> = pts
        .iter()
        .copied()
        .filter(|&i| {
            let px = (view.x_of)(controls[i].start);
            px >= r.left() - 4.0 && px <= r.right() + 4.0
        })
        .collect();
    let mut last_x = f32::NEG_INFINITY;
    for (k, &i) in shown.iter().enumerate() {
        let px = (view.x_of)(controls[i].start);
        let next_x = shown
            .get(k + 1)
            .map_or(f32::INFINITY, |&j| (view.x_of)(controls[j].start));
        let room = px - last_x >= 10.0 && next_x - px >= 10.0;
        let end = k == 0 || k + 1 == shown.len();
        if !(room || end || selected == Some(i)) {
            continue;
        }
        last_x = px;
        let p = pos2(px, view.y_of(lane, val(&controls[i])));
        if selected == Some(i) {
            g.circle_filled(p, 8.0, theme::ORANGE.gamma_multiply(0.3));
        }
        g.circle_filled(p, 4.5, theme::BONE_HI);
        g.circle_filled(
            p,
            3.0,
            if selected == Some(i) {
                theme::ORANGE_HI
            } else {
                theme::ORANGE
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane_points(c: &[Control], lane: Lane) -> Vec<(f32, u16)> {
        points(c, lane.num().unwrap())
            .into_iter()
            .map(|i| (c[i].start, val(&c[i])))
            .collect()
    }

    #[test]
    fn lanes_have_their_kinds_ranges_and_rest_values() {
        assert_eq!(Lane::Cc(1).num(), Some(1));
        assert_eq!(Lane::PitchBend.num(), Some(PITCH_BEND));
        assert_eq!(Lane::Pressure.num(), Some(CHANNEL_PRESSURE));
        assert_eq!(Lane::Velocity.num(), None);
        assert_eq!(
            (Lane::PitchBend.max(), Lane::PitchBend.rest()),
            (16383, 8192)
        );
        assert_eq!(Lane::Cc(64).clamp(80.0), 127);
        assert_eq!(Lane::Cc(64).clamp(20.0), 0);
        assert_eq!(Lane::Cc(1).clamp(300.0), 127);
        assert_eq!(Lane::PitchBend.shown(0), -8192);
        for l in Lane::ALL {
            assert!(!l.name().is_empty());
        }
    }

    #[test]
    fn a_line_replaces_the_lane_between_its_ends_and_keeps_other_events() {
        let mut c = vec![make(0.5, 1, 10), make(1.0, 64, 127), make(3.0, 1, 99)];
        draw_line(&mut c, Lane::Cc(1), (0.0, 0.0), (1.0, 127.0), 0.25);
        assert_eq!(
            lane_points(&c, Lane::Cc(1)),
            vec![
                (0.0, 0),
                (0.25, 32),
                (0.5, 64),
                (0.75, 95),
                (1.0, 127),
                (3.0, 99)
            ]
        );
        assert_eq!(
            lane_points(&c, Lane::Cc(64)),
            vec![(1.0, 127)],
            "other lanes stay"
        );
        // Drawn right to left it is the same line.
        let mut d = Vec::new();
        draw_line(&mut d, Lane::Cc(1), (1.0, 127.0), (0.0, 0.0), 0.25);
        assert_eq!(
            lane_points(&d, Lane::Cc(1)),
            lane_points(&c, Lane::Cc(1))[..5]
        );
    }

    #[test]
    fn a_switch_lane_gets_only_its_changes_and_bend_its_full_range() {
        let mut c = Vec::new();
        draw_line(&mut c, Lane::Cc(64), (0.0, 127.0), (2.0, 0.0), 0.125);
        // The line crosses the middle just after beat 1: the pedal goes up there.
        assert_eq!(lane_points(&c, Lane::Cc(64)), vec![(0.0, 127), (1.125, 0)]);
        draw_line(&mut c, Lane::PitchBend, (0.0, 8192.0), (1.0, 16383.0), 0.5);
        assert_eq!(
            lane_points(&c, Lane::PitchBend),
            vec![(0.0, 8192), (0.5, 12288), (1.0, 16383)]
        );
        assert_eq!(value_at(&c, PITCH_BEND, 0.7, 8192), 12288);
        assert_eq!(value_at(&c, 1, 0.7, 0), 0, "an empty lane is at rest");
    }

    #[test]
    fn points_are_put_moved_hit_and_erased() {
        let mut c = Vec::new();
        put(&mut c, Lane::Cc(11), 1.0, 50.0);
        put(&mut c, Lane::Cc(11), 2.0, 60.0);
        put(&mut c, Lane::Cc(11), 3.0, 70.0);
        assert_eq!(
            put(&mut c, Lane::Cc(11), 2.0, 65.0),
            Some(1),
            "same place: replaced"
        );
        assert_eq!(c.len(), 3);
        let i = hit(&c, Lane::Cc(11), 2.03, 0.1).unwrap();
        // A point cannot pass its neighbours.
        let i = move_point(&mut c, Lane::Cc(11), i, 5.0, 200.0);
        assert!(c[i].start < 3.0 && val(&c[i]) == 127);
        assert_eq!(erase(&mut c, Lane::Cc(11), 0.0, 1.5), 1);
        assert_eq!(c.len(), 2);
        assert!(put(&mut c, Lane::Velocity, 1.0, 1.0).is_none());
    }
}
