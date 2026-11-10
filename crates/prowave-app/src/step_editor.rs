//! Editors of Pattern and Drummer regions (Logic Pro: the Step Sequencer and the Drummer
//! Editor). They take the place of the Piano Roll in the bottom panel when the selected region
//! is one of these; the notes are made again from the settings after every change (see
//! `pattern.rs`), and "Convert to MIDI Region" turns the region into an ordinary one.
//!
//! - **Pattern:** a grid with a row for each pitch and a column for each step. Click an empty
//!   step to switch it on (the height of the click is its velocity) and drag to paint more;
//!   click a step that is on to switch it off, or drag up and down in it for its velocity;
//!   right-click or right-drag to clear. Drag a row's name to change its pitch (it sounds
//!   while it is held); right-click it to delete the row.
//! - **Drummer:** the style, the busyness and loudness on a pad (drag the dot), the swing and
//!   how often it fills, the pieces of the kit that play, and a new variation. The lanes show
//!   what it plays.
use crate::hw::{self, Cap};
use crate::i18n::{tr, trf};
use crate::model::{Generator, Project, Track, TrackKind};
use crate::pattern::{
    drums, parts, Direction, DrumStyle, DrummerParams, PatternParams, PatternRow, StepRate,
    DEFAULT_VELOCITY, MAX_ROWS,
};
use crate::piano_roll::{caption, ink, menu_row, Audition};
use crate::theme::{self, Face};
use crate::widgets::context_menu;
use crate::UiState;
use egui::{pos2, vec2, Align2, Color32, Rect, Sense, Stroke, Ui};

use crate::piano_roll::{BRACKET_Y, KEY_H, KEY_Y, TOOLBAR_H};

/// The pattern grid's rows: at least this tall, and up to `ROW_MAX` to fill the editor.
const ROW_H: f32 = 26.0;
const ROW_MAX: f32 = 56.0;
const LABEL_W: f32 = 128.0;
/// The step ruler over the grid (step numbers and the playing step's LED).
const RULER_H: f32 = 26.0;
/// Width of the Drummer's controls.
const PANEL_W: f32 = 360.0;

/// What a click or drag in the pattern grid sets on a step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EditMode {
    /// Whether the step is on, and how hard.
    #[default]
    Velocity,
    /// The chance (percent) it plays.
    Chance,
    /// How many times it repeats within its time (click to count up, right-click to reset).
    Repeat,
    /// Whether it carries on the note before it.
    Tie,
}

impl EditMode {
    const ALL: [EditMode; 4] = [Self::Velocity, Self::Chance, Self::Repeat, Self::Tie];

    fn name(self) -> &'static str {
        match self {
            Self::Velocity => tr("Velocity"),
            Self::Chance => tr("Chance"),
            Self::Repeat => tr("Repeat"),
            Self::Tie => tr("Tie"),
        }
    }
}

/// The chance (percent) the height of the pointer in a step stands for.
fn chance_at(y: f32, cell: Rect) -> u8 {
    (((cell.bottom() - y) / cell.height()).clamp(0.0, 1.0) * 100.0).round() as u8
}

fn direction_name(d: Direction) -> &'static str {
    match d {
        Direction::Forward => tr("Forward"),
        Direction::Backward => tr("Backward"),
        Direction::PingPong => tr("Ping-Pong"),
        Direction::Random => tr("Random"),
    }
}

/// What the editors remember between frames.
#[derive(Default)]
pub struct StepView {
    mode: EditMode,
    gesture: Option<Gesture>,
    /// The pitch sounding because a row's name is held.
    held: Option<u8>,
    /// How far a row's name has been dragged towards its next pitch.
    pitch_drag: f32,
    scroll_y: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Gesture {
    /// Switching steps on as the pointer crosses them.
    Paint,
    /// Switching steps off.
    Erase,
    /// Pressed on a step that is on: a drag sets its velocity, a click switches it off.
    Velocity {
        row: usize,
        step: usize,
        from_y: f32,
        moved: bool,
    },
}

/// Whether a track plays drums (so its pattern rows are named after the drums): a Drummer
/// track.
pub fn is_drum_track(t: &Track) -> bool {
    t.kind == TrackKind::Drummer
}

/// Draws the editor for the Pattern or Drummer region `ri` of track `ti` in `body`.
pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState, ti: usize, ri: usize, body: Rect) {
    let g = ui.painter_at(body);
    hw::chassis(&g, body);
    let drummer = matches!(
        p.tracks[ti].regions[ri]
            .generated
            .as_ref()
            .map(|g| &g.source),
        Some(Generator::Drummer(_))
    );
    if drummer {
        show_drummer(ui, p, s, ti, ri, body);
    } else {
        show_pattern(ui, p, s, ti, ri, body);
    }
}

/// The editor's menu row across the top of `body`, with the editor's key at its left (lit:
/// `active` 2 is the Step Sequencer, 3 the Drummer Editor). Returns the row and where the
/// controls after the key start.
fn strip(ui: &Ui, body: Rect, active: usize) -> (Rect, f32) {
    let strip = Rect::from_min_size(body.min, vec2(body.width(), TOOLBAR_H.min(body.height())));
    let g = ui.painter_at(strip);
    menu_row(&g, strip);
    let (_, right) =
        crate::piano_roll::editor_keys(ui, &g, strip.left() + 10.0, strip.top(), active, false);
    (strip, right + 18.0)
}

/// A value field with up/down arrows in a menu row: a click on the upper or lower half of the
/// arrows, the scroll wheel over it, or a drag up or down steps the value. Returns how many
/// steps (up is positive).
fn step_field(ui: &Ui, g: &egui::Painter, rect: Rect, id: egui::Id, value: &str) -> i32 {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    hw::paint_field(g, rect, value, true, resp.hovered() || resp.dragged());
    let mut d = 0;
    if resp.clicked() {
        if let Some(pt) = resp.interact_pointer_pos() {
            if pt.x >= rect.right() - 22.0 {
                d += if pt.y < rect.center().y { 1 } else { -1 };
            }
        }
    }
    d + drag_steps(ui, &resp, id)
}

/// Steps from a vertical drag (one per 6 points) or the scroll wheel over `resp`.
fn drag_steps(ui: &Ui, resp: &egui::Response, id: egui::Id) -> i32 {
    let mut moved = 0.0;
    if resp.dragged() {
        moved -= resp.drag_delta().y;
    }
    if resp.hovered() {
        moved += ui.input(|i| i.smooth_scroll_delta.y) * 0.25;
    }
    if moved == 0.0 {
        return 0;
    }
    let acc_id = id.with("acc");
    let acc: f32 = ui.data(|m| m.get_temp(acc_id)).unwrap_or(0.0) + moved;
    let n = (acc / 6.0).trunc();
    ui.data_mut(|m| m.insert_temp(acc_id, acc - n * 6.0));
    n as i32
}

/// A value on a small LCD (its label in the corner, the number in segments) that a drag up
/// or down or the scroll wheel changes. Returns how many steps.
fn lcd_drag(ui: &Ui, g: &egui::Painter, rect: Rect, id: egui::Id, label: &str, value: &str) -> i32 {
    let resp = ui
        .interact(rect, id, Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    hw::lcd_readout(g, rect, label, value);
    drag_steps(ui, &resp, id)
}

/// A popup field (dark, with arrows) under a bracket at `x` in the menu row `strip`, `w`
/// wide; returns its response (open a menu on it) and its right edge.
#[allow(clippy::too_many_arguments)]
fn bracket_field(
    ui: &Ui,
    g: &egui::Painter,
    strip: Rect,
    x: f32,
    w: f32,
    id: egui::Id,
    label: &str,
    value: &str,
) -> (egui::Response, f32) {
    let r = Rect::from_min_size(pos2(x, strip.top() + KEY_Y + 1.0), vec2(w, 26.0));
    hw::bracket(
        g,
        r.left(),
        r.right(),
        strip.top() + BRACKET_Y,
        caption(label),
    );
    (crate::widgets::popup_button(ui, g, r, id, value), r.right())
}

/// A bone key with a legend in a menu row, right edge at `right`; greyed when not `enabled`.
/// Returns whether it was clicked and its left edge.
fn row_key(
    ui: &Ui,
    g: &egui::Painter,
    strip: Rect,
    right: f32,
    id: egui::Id,
    label: &str,
    enabled: bool,
) -> (bool, f32) {
    let text = label.to_uppercase();
    let w = hw::galley(g, &text, Face::Bold, 12.0, theme::INK).size().x + 26.0;
    let r = Rect::from_min_size(pos2(right - w, strip.top() + KEY_Y), vec2(w, KEY_H));
    if !enabled {
        let face = hw::paint_key(g, r, Cap::Bone, false, false);
        hw::text(
            g,
            face.center(),
            Align2::CENTER_CENTER,
            &text,
            Face::Bold,
            12.0,
            theme::SEAM_DK,
        );
        return (false, r.left());
    }
    let (resp, _) = hw::key(
        ui,
        g,
        r,
        id,
        Cap::Bone,
        false,
        None,
        hw::Legend::Text(label),
    );
    (resp.clicked(), r.left())
}

/// Where the pattern grid is in `body` for `rows` rows of `steps` steps: the grid (steps
/// only, under the step ruler), the top of its first row (before scrolling), the step width
/// and the row height. The steps fill the width and the rows the height, within limits.
fn pattern_geom(body: Rect, steps: usize, rows: usize) -> (Rect, f32, f32, f32) {
    let area = Rect::from_min_max(pos2(body.left(), body.top() + TOOLBAR_H), body.max);
    let grid = Rect::from_min_max(
        pos2(area.left() + LABEL_W, area.top() + RULER_H),
        pos2(area.right() - 10.0, area.bottom()),
    );
    let cw = (grid.width() / steps.max(1) as f32).max(10.0);
    let row_h = ((grid.height() - 10.0) / rows.max(1) as f32).clamp(ROW_H, ROW_MAX);
    (grid, grid.top() + 4.0, cw, row_h)
}

// ------------------------------------------------------------------------- names

pub(crate) const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Logic's name for a pitch: 60 is C3.
pub fn note_name(pitch: u8) -> String {
    format!(
        "{}{}",
        NOTE_NAMES[pitch as usize % 12],
        pitch as i32 / 12 - 2
    )
}

/// The name of a General MIDI drum pitch the kit plays.
fn drum_name(pitch: u8) -> Option<String> {
    use drums::*;
    let tom = |n: u32| trf("Tom {}", &[&n]);
    Some(match pitch {
        KICK => tr("Kick").into(),
        SNARE => tr("Snare").into(),
        ELECTRIC_SNARE => tr("Electric Snare").into(),
        CLAP => tr("Clap").into(),
        CLOSED_HAT => tr("Closed Hi-Hat").into(),
        PEDAL_HAT => tr("Pedal Hi-Hat").into(),
        OPEN_HAT => tr("Open Hi-Hat").into(),
        CRASH => tr("Crash").into(),
        RIDE => tr("Ride").into(),
        TAMBOURINE => tr("Tambourine").into(),
        COWBELL => tr("Cowbell").into(),
        TOM_HIGH => tom(1),
        TOM_MID_HIGH => tom(2),
        TOM_MID => tom(3),
        TOM_LOW => tom(4),
        TOM_FLOOR => tom(5),
        TOM_FLOOR_LOW => tom(6),
        _ => return None,
    })
}

fn row_name(pitch: u8, drums: bool) -> String {
    drums
        .then(|| drum_name(pitch))
        .flatten()
        .unwrap_or_else(|| note_name(pitch))
}

fn part_name(part: u8) -> &'static str {
    match part {
        parts::KICK => tr("Kick"),
        parts::SNARE => tr("Snare"),
        parts::HIHAT => tr("Hi-Hat"),
        parts::TOMS => tr("Toms"),
        parts::CYMBALS => tr("Cymbals"),
        _ => tr("Percussion"),
    }
}

fn style_name(style: DrumStyle) -> &'static str {
    match style {
        DrumStyle::Rock => tr("Rock"),
        DrumStyle::Pop => tr("Pop"),
        DrumStyle::Funk => tr("Funk"),
        DrumStyle::Jazz => tr("Jazz"),
        DrumStyle::HipHop => tr("Hip Hop"),
        DrumStyle::Electronic => tr("Electronic"),
    }
}

// ------------------------------------------------------------------------- pattern

/// The pitch a new row gets: the first drum a drum pattern does not have yet, or two
/// semitones below the lowest row.
fn next_row_pitch(pat: &PatternParams, drums: bool) -> u8 {
    use drums::*;
    let used = |p: u8| pat.rows.iter().any(|r| r.pitch == p);
    if drums {
        let order = [
            KICK,
            SNARE,
            CLAP,
            CLOSED_HAT,
            OPEN_HAT,
            TOM_LOW,
            CRASH,
            RIDE,
            ELECTRIC_SNARE,
            TOM_FLOOR_LOW,
            TOM_FLOOR,
            TOM_MID,
            TOM_MID_HIGH,
            TOM_HIGH,
            TAMBOURINE,
            COWBELL,
        ];
        if let Some(p) = order.into_iter().find(|p| !used(*p)) {
            return p;
        }
    }
    let lowest = pat.rows.iter().map(|r| r.pitch).min().unwrap_or(62);
    lowest.saturating_sub(2)
}

/// The velocity the height of the pointer in a step stands for.
fn velocity_at(y: f32, cell: Rect) -> u8 {
    let t = ((cell.bottom() - y) / cell.height()).clamp(0.1, 1.0);
    (t * 127.0).round().max(16.0) as u8
}

fn show_pattern(ui: &mut Ui, p: &mut Project, s: &mut UiState, ti: usize, ri: usize, body: Rect) {
    let Some(Generator::Pattern(mut pat)) = p.tracks[ti].regions[ri]
        .generated
        .as_ref()
        .map(|g| g.source.clone())
    else {
        return;
    };
    let (strip, mut x) = strip(ui, body, 2);
    let (origin, len_beats) = {
        let r = &p.tracks[ti].regions[ri];
        (p.region_start_beats(r), p.region_len_beats(r))
    };
    let bar_len = p.bar_len_at(origin);
    let drums = is_drum_track(&p.tracks[ti]);
    let mut changed = false;
    let mut convert = false;
    let mut mode = s.steps.mode;

    // ---- settings, each under its bracket in the menu row
    {
        let g = ui.painter_at(strip);
        let uid = |k: &str| egui::Id::unique(("pattern_bar", k.to_string(), ti, ri));
        let fy = strip.top() + KEY_Y + 1.0;
        // Steps: a field with arrows.
        let r = Rect::from_min_size(pos2(x, fy), vec2(64.0, 26.0));
        hw::bracket(
            &g,
            r.left(),
            r.right(),
            strip.top() + BRACKET_Y,
            caption(tr("Steps:")),
        );
        let d = step_field(ui, &g, r, uid("steps"), &pat.steps.to_string());
        if d != 0 {
            pat.steps = (pat.steps as i32 + d).clamp(1, 64) as usize;
            changed = true;
        }
        x = r.right() + 14.0;
        let (resp, right) = bracket_field(
            ui,
            &g,
            strip,
            x,
            80.0,
            uid("rate"),
            tr("Rate:"),
            pat.rate.name(),
        );
        egui::Popup::menu(&resp).show(|ui| {
            for r in StepRate::ALL {
                if ui.selectable_label(pat.rate == r, r.name()).clicked() {
                    pat.rate = r;
                    changed = true;
                    ui.close();
                }
            }
        });
        x = right + 14.0;
        // Swing and gate: values on LCD chips, dragged up or down.
        for (key, label, lo, hi) in [
            ("swing", tr("Swing:"), 0, 50),
            ("gate", tr("Gate:"), 5, 100),
        ] {
            let value = if key == "swing" {
                &mut pat.swing
            } else {
                &mut pat.gate
            };
            let r = Rect::from_min_size(pos2(x, strip.top() + KEY_Y - 4.0), vec2(78.0, 34.0));
            let now = (*value * 100.0).round() as i32;
            let d = lcd_drag(ui, &g, r, uid(key), caption(label), &now.to_string());
            if d != 0 {
                *value = (now + d).clamp(lo, hi) as f32 / 100.0;
                changed = true;
            }
            x = r.right() + 10.0;
        }
        x += 4.0;
        let (resp, right) = bracket_field(
            ui,
            &g,
            strip,
            x,
            112.0,
            uid("direction"),
            tr("Direction:"),
            direction_name(pat.direction),
        );
        egui::Popup::menu(&resp).show(|ui| {
            for d in Direction::ALL {
                if ui
                    .selectable_label(pat.direction == d, direction_name(d))
                    .clicked()
                {
                    pat.direction = d;
                    changed = true;
                    ui.close();
                }
            }
        });
        x = right + 14.0;
        let (resp, right) = bracket_field(
            ui,
            &g,
            strip,
            x,
            112.0,
            uid("mode"),
            tr("Edit:"),
            mode.name(),
        );
        egui::Popup::menu(&resp).show(|ui| {
            for m in EditMode::ALL {
                if ui.selectable_label(mode == m, m.name()).clicked() {
                    mode = m;
                    ui.close();
                }
            }
        });
        x = right + 18.0;
        // The keys, from the right: Convert, Add Row, New Variation.
        let (clicked, left) = row_key(
            ui,
            &g,
            strip,
            strip.right() - 10.0,
            uid("convert"),
            tr("Convert to MIDI Region"),
            true,
        );
        convert |= clicked;
        let room = pat.rows.len() < MAX_ROWS;
        let (clicked, left) = row_key(
            ui,
            &g,
            strip,
            left - 8.0,
            uid("add_row"),
            tr("Add Row"),
            room,
        );
        if clicked && room {
            let pitch = next_row_pitch(&pat, drums);
            pat.rows.push(PatternRow::new(pitch));
            changed = true;
        }
        let random = pat.rows.iter().any(|r| r.chance.iter().any(|c| *c < 100))
            || pat.direction == Direction::Random;
        if random && left - 8.0 > x + 100.0 {
            let (clicked, _) = row_key(
                ui,
                &g,
                strip,
                left - 8.0,
                uid("variation"),
                tr("New Variation"),
                true,
            );
            if clicked {
                pat.seed = pat.seed.wrapping_add(1);
                changed = true;
            }
        }
    }

    s.steps.mode = mode;

    // ---- grid: the step ruler, then a row of step keys for each pitch
    let area = Rect::from_min_max(pos2(body.left(), strip.bottom()), body.max);
    let steps = pat.steps.clamp(1, crate::pattern::MAX_STEPS);
    let (grid, top0, cw, row_h) = pattern_geom(body, steps, pat.rows.len());
    let v = &mut s.steps;
    let rows_area = Rect::from_min_max(pos2(area.left(), grid.top()), area.max);
    let overflow = (pat.rows.len() as f32 * row_h - rows_area.height() + 8.0).max(0.0);
    if ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pt| rows_area.contains(pt))
    {
        v.scroll_y -= ui.input(|i| i.smooth_scroll_delta.y);
    }
    v.scroll_y = v.scroll_y.clamp(0.0, overflow);
    let top = top0 - v.scroll_y;
    let cell = |r: usize, k: usize| {
        Rect::from_min_size(
            pos2(grid.left() + k as f32 * cw, top + r as f32 * row_h),
            vec2(cw, row_h),
        )
    };
    let per_beat = pat.rate.per_beat().max(1);
    let step_len = pat.rate.beats();
    let now = p.position_beats - origin;
    let current = (now >= 0.0 && now < len_beats).then(|| pat.step_at(now));
    let is_bar = |k: usize| {
        let at = k as f64 * step_len / bar_len.max(1e-9);
        (at - at.round()).abs() < 1e-6
    };
    let gr = ui.painter_at(area);
    // The step ruler: step numbers (beats bold) and an LED over each step, lit on the one
    // playing.
    let ruler = Rect::from_min_max(
        pos2(grid.left(), area.top()),
        pos2(grid.right(), grid.top()),
    );
    for k in 0..steps {
        let x0 = grid.left() + k as f32 * cw;
        let beat = k % per_beat == 0;
        hw::text(
            &gr,
            pos2(x0 + 6.0, ruler.center().y),
            Align2::LEFT_CENTER,
            &(k + 1).to_string(),
            if beat { Face::Lcd } else { Face::Bold },
            if beat { 11.0 } else { 9.5 },
            if beat { theme::INK } else { theme::PRINT },
        );
        hw::led_sized(
            &gr,
            pos2(x0 + cw - 9.0, ruler.center().y),
            2.5,
            current == Some(k),
        );
    }
    hw::seam_h(&gr, area.x_range(), grid.top() - 2.0);
    // The rows' names' column, a seam, then the steps: beat groups in recessed wells.
    hw::seam_v(&gr, grid.left() - 6.0, area.y_range());
    let g = ui.painter_at(rows_area);
    let hover = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|pt| grid.contains(*pt));
    let rows_h = pat.rows.len() as f32 * row_h;
    let mut k0 = 0;
    while k0 < steps {
        let k1 = (k0 + per_beat).min(steps);
        let well = Rect::from_min_max(
            pos2(grid.left() + k0 as f32 * cw + 1.0, top - 3.0),
            pos2(grid.left() + k1 as f32 * cw - 1.0, top + rows_h + 3.0),
        );
        hw::recess(&g, well, 3.0);
        k0 = k1;
    }
    for k in 1..steps {
        if is_bar(k) {
            hw::seam_v(
                &g,
                grid.left() + k as f32 * cw - 1.0,
                (top - 3.0)..=(top + rows_h + 3.0),
            );
        }
    }
    for (r, row) in pat.rows.iter().enumerate() {
        let y = top + r as f32 * row_h;
        if y + row_h < rows_area.top() || y > rows_area.bottom() {
            continue;
        }
        // The row's name on a bone key (orange while it sounds).
        let label = Rect::from_min_size(
            pos2(area.left() + 8.0, y + 3.0),
            vec2(LABEL_W - 24.0, row_h - 8.0),
        );
        let held = v.held == Some(row.pitch);
        let face = hw::paint_key(
            &g,
            label,
            if held { Cap::Orange } else { Cap::Bone },
            false,
            false,
        );
        let name = hw::fit(
            &g,
            &row_name(row.pitch, drums).to_uppercase(),
            Face::Bold,
            11.0,
            face.width() - 14.0,
        );
        hw::text(
            &g,
            face.left_center() + vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            &name,
            Face::Bold,
            11.0,
            theme::INK,
        );
        for k in 0..steps {
            let c = cell(r, k).shrink2(vec2(3.0, 3.5));
            let vel = row.steps[k];
            let hot = hover.is_some_and(|pt| cell(r, k).contains(pt));
            // Every step is a key: bone with its LED dark when empty, black with its LED lit
            // when on, and its level (velocity, or chance in Chance mode) on a meter along
            // its foot.
            let cap = if vel == 0 { Cap::Bone } else { Cap::Dark };
            let face = hw::paint_key(&g, c, cap, false, hot);
            if face.width() >= 16.0 && face.height() >= 14.0 {
                hw::led_sized(&g, pos2(face.left() + 7.0, face.top() + 7.0), 2.5, vel > 0);
            }
            if vel == 0 {
                continue;
            }
            let (level, lit) = match mode {
                EditMode::Chance => (row.chance[k] as f32 / 100.0, theme::ORANGE),
                _ => (vel as f32 / 127.0, theme::ON_DARK),
            };
            let well = Rect::from_min_max(
                pos2(face.left() + 4.0, face.bottom() - 7.0),
                pos2(face.right() - 4.0, face.bottom() - 4.0),
            );
            g.rect_filled(well, 1.0, theme::METER_WELL);
            let bar =
                Rect::from_min_size(well.min, vec2(well.width() * level.clamp(0.05, 1.0), 3.0));
            g.rect_filled(bar, 1.0, lit);
            // What the step also does, in every mode: a bar joining it to the step before
            // for a tie, the repeat count in its corner, a stripe on top when it does not
            // always play.
            if row.tie[k] {
                let join = Rect::from_min_max(
                    pos2(c.left() - 6.0, c.center().y - 2.0),
                    pos2(c.left() + 1.0, c.center().y + 2.0),
                );
                g.rect_filled(join, 1.0, theme::INK);
            }
            if row.repeat[k] > 1 {
                hw::text(
                    &g,
                    face.right_bottom() + vec2(-5.0, -9.0),
                    Align2::RIGHT_BOTTOM,
                    &format!("×{}", row.repeat[k]),
                    Face::Lcd,
                    9.0,
                    theme::ORANGE,
                );
            }
            if row.chance[k] < 100 {
                let w = (face.width() - 4.0) * row.chance[k] as f32 / 100.0;
                g.rect_filled(
                    Rect::from_min_size(face.min + vec2(2.0, 2.0), vec2(w, 2.0)),
                    0.0,
                    theme::ON_DARK_DIM,
                );
            }
        }
    }
    if let Some(k) = current {
        // The step playing: a 2-point orange edge along the top of its column.
        let x = grid.left() + k as f32 * cw;
        g.rect_filled(
            Rect::from_min_size(pos2(x + 2.0, top - 3.0), vec2(cw - 4.0, 2.0)),
            0.0,
            theme::ORANGE,
        );
    }

    // ---- painting steps
    let resp = ui.interact(
        grid,
        egui::Id::unique(("pattern_grid", ti, ri)),
        Sense::click_and_drag(),
    );
    let (pressed, down, sec_pressed, sec_down) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.secondary_pressed(),
            i.pointer.secondary_down(),
        )
    });
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let row_count = pat.rows.len();
    let cell_at = |pt: egui::Pos2| -> Option<(usize, usize)> {
        let (kx, ry) = (
            ((pt.x - grid.left()) / cw).floor(),
            ((pt.y - top) / row_h).floor(),
        );
        (kx >= 0.0 && ry >= 0.0 && (kx as usize) < steps && (ry as usize) < row_count)
            .then_some((ry as usize, kx as usize))
    };
    if let Some(pt) = pointer {
        if let Some(Gesture::Velocity {
            row,
            step,
            from_y,
            moved,
        }) = v.gesture
        {
            if down {
                let moved = moved || (pt.y - from_y).abs() > 3.0;
                if moved {
                    let c = cell(row, step);
                    let r = &mut pat.rows[row];
                    if mode == EditMode::Chance {
                        let chance = chance_at(pt.y, c);
                        if r.chance[step] != chance {
                            r.chance[step] = chance;
                            changed = true;
                        }
                    } else {
                        let vel = velocity_at(pt.y, c);
                        if r.steps[step] != vel {
                            r.steps[step] = vel;
                            changed = true;
                        }
                    }
                }
                v.gesture = Some(Gesture::Velocity {
                    row,
                    step,
                    from_y,
                    moved,
                });
            }
        } else if let Some((r, k)) = cell_at(pt) {
            let now_vel = pat.rows[r].steps[k];
            if mode != EditMode::Velocity {
                // The other properties belong to steps that are on.
                if resp.hovered() && now_vel > 0 {
                    let row = &mut pat.rows[r];
                    match mode {
                        EditMode::Chance if pressed => {
                            v.gesture = Some(Gesture::Velocity {
                                row: r,
                                step: k,
                                from_y: pt.y,
                                moved: false,
                            });
                        }
                        EditMode::Repeat if pressed => {
                            row.repeat[k] = row.repeat[k] % 4 + 1;
                            changed = true;
                        }
                        EditMode::Repeat if sec_pressed && row.repeat[k] != 1 => {
                            row.repeat[k] = 1;
                            changed = true;
                        }
                        EditMode::Tie if pressed => {
                            row.tie[k] = !row.tie[k];
                            changed = true;
                        }
                        _ => {}
                    }
                }
            } else if resp.hovered() && pressed {
                if now_vel == 0 {
                    pat.rows[r].steps[k] = velocity_at(pt.y, cell(r, k));
                    v.gesture = Some(Gesture::Paint);
                    changed = true;
                } else {
                    v.gesture = Some(Gesture::Velocity {
                        row: r,
                        step: k,
                        from_y: pt.y,
                        moved: false,
                    });
                }
            } else if resp.hovered() && sec_pressed {
                v.gesture = Some(Gesture::Erase);
                if now_vel != 0 {
                    pat.rows[r].steps[k] = 0;
                    changed = true;
                }
            } else if v.gesture == Some(Gesture::Paint) && down && now_vel == 0 {
                pat.rows[r].steps[k] = velocity_at(pt.y, cell(r, k));
                changed = true;
            } else if v.gesture == Some(Gesture::Erase) && sec_down && now_vel != 0 {
                pat.rows[r].steps[k] = 0;
                changed = true;
            }
        }
    }
    if !down && !sec_down {
        // Let go: a click on a step that was on, without moving, switches it off.
        if let Some(Gesture::Velocity {
            row,
            step,
            moved: false,
            ..
        }) = v.gesture
        {
            if mode == EditMode::Velocity
                && row < pat.rows.len()
                && step < pat.rows[row].steps.len()
            {
                pat.rows[row].steps[step] = 0;
                changed = true;
            }
        }
        v.gesture = None;
    }

    // ---- the rows' names
    let mut sounding = None;
    let mut delete = None;
    for r in 0..pat.rows.len() {
        let y = top + r as f32 * row_h;
        if y + row_h < rows_area.top() || y > rows_area.bottom() {
            continue;
        }
        let label = Rect::from_min_size(pos2(area.left(), y), vec2(LABEL_W - 8.0, row_h - 1.0))
            .intersect(rows_area);
        let resp = ui
            .interact(
                label,
                egui::Id::unique(("pattern_row", ti, ri, r)),
                Sense::click_and_drag(),
            )
            .on_hover_text(tr(
                "Drag to change the pitch; right-click to delete the row",
            ));
        if resp.drag_started() {
            v.pitch_drag = 0.0;
        }
        if resp.dragged() {
            v.pitch_drag -= resp.drag_delta().y;
            let steps = (v.pitch_drag / 8.0).trunc();
            if steps != 0.0 {
                v.pitch_drag -= steps * 8.0;
                let pitch = (pat.rows[r].pitch as i32 + steps as i32).clamp(0, 127) as u8;
                pat.rows[r].pitch = pitch;
                changed = true;
            }
        }
        if resp.is_pointer_button_down_on() {
            sounding = Some(pat.rows[r].pitch);
        }
        context_menu(&resp, |ui| {
            if ui.button(tr("Delete Row")).clicked() {
                delete = Some(r);
                ui.close();
            }
        });
    }
    if sounding != v.held {
        if let Some(old) = v.held {
            s.audition.push(Audition::Off { pitch: old });
        }
        if let Some(pitch) = sounding {
            s.audition.push(Audition::On {
                pitch,
                vel: DEFAULT_VELOCITY,
                track: ti.min(usize::from(u16::MAX)) as u16,
            });
        }
        v.held = sounding;
    }
    if let Some(r) = delete {
        if r < pat.rows.len() {
            pat.rows.remove(r);
            changed = true;
        }
    }

    if changed {
        pat.tidy();
        if let Some(g) = p.tracks[ti].regions[ri].generated.as_mut() {
            g.source = Generator::Pattern(pat);
        }
        p.regenerate(ti, ri);
    }
    if convert {
        p.convert_to_midi_region(ti, ri);
    }
}

// ------------------------------------------------------------------------- drummer

fn show_drummer(ui: &mut Ui, p: &mut Project, s: &mut UiState, ti: usize, ri: usize, body: Rect) {
    let Some(Generator::Drummer(mut d)) = p.tracks[ti].regions[ri]
        .generated
        .as_ref()
        .map(|g| g.source.clone())
    else {
        return;
    };
    let _ = s;
    let (origin, len_beats) = {
        let r = &p.tracks[ti].regions[ri];
        (p.region_start_beats(r), p.region_len_beats(r))
    };
    let mut changed = false;
    let mut convert = false;
    let g = ui.painter_at(body);
    let uid = |k: &str| egui::Id::unique(("drummer_bar", k.to_string(), ti, ri));

    // ---- the menu row: the style, the variation, Convert
    let (strip, x) = strip(ui, body, 3);
    {
        let gs = ui.painter_at(strip);
        let (resp, right) = bracket_field(
            ui,
            &gs,
            strip,
            x,
            140.0,
            uid("style"),
            tr("Style:"),
            style_name(d.style),
        );
        egui::Popup::menu(&resp).show(|ui| {
            for style in DrumStyle::ALL {
                if ui
                    .selectable_label(d.style == style, style_name(style))
                    .clicked()
                {
                    d.style = style;
                    // A new style starts with its own swing.
                    d.swing = style.default_swing();
                    changed = true;
                    ui.close();
                }
            }
        });
        // The variation's number on an LCD, and the key for the next one.
        let var = Rect::from_min_size(
            pos2(right + 18.0, strip.top() + KEY_Y - 4.0),
            vec2(96.0, 34.0),
        );
        let label = trf("Variation {}", &[&""]);
        hw::lcd_readout(&gs, var, label.trim(), &format!("{:02}", d.seed % 100));
        let text = tr("New Variation").to_uppercase();
        let w = hw::galley(&gs, &text, Face::Bold, 12.0, theme::INK)
            .size()
            .x
            + 26.0;
        let key = Rect::from_min_size(
            pos2(var.right() + 10.0, strip.top() + KEY_Y),
            vec2(w, KEY_H),
        );
        if hw::key(
            ui,
            &gs,
            key,
            uid("variation"),
            Cap::Bone,
            false,
            None,
            hw::Legend::Text(tr("New Variation")),
        )
        .0
        .clicked()
        {
            d.seed = d.seed.wrapping_add(1);
            changed = true;
        }
        let (clicked, _) = row_key(
            ui,
            &gs,
            strip,
            strip.right() - 10.0,
            uid("convert"),
            tr("Convert to MIDI Region"),
            true,
        );
        convert |= clicked;
    }
    let body = Rect::from_min_max(pos2(body.left(), strip.bottom()), body.max);

    // ---- the controls on the left, on a faceplate
    let panel = Rect::from_min_size(body.min, vec2(PANEL_W.min(body.width()), body.height()));
    let plate = Rect::from_min_max(panel.min + vec2(6.0, 6.0), panel.max - vec2(8.0, 6.0));
    if plate.width() > 40.0 && plate.height() > 40.0 {
        hw::faceplate(&g, plate, 4.0, false);
        for c in [
            plate.left_top() + vec2(8.0, 8.0),
            plate.right_top() + vec2(-8.0, 8.0),
            plate.left_bottom() + vec2(8.0, -8.0),
            plate.right_bottom() + vec2(-8.0, -8.0),
        ] {
            hw::screw(&g, c, 3.5, 35.0);
        }
    }
    hw::seam_v(&g, panel.right() - 2.0, panel.y_range());
    let pad = drummer_pad(body);
    changed |= xy_pad(ui, &g, pad, &mut d, (ti, ri));

    // Swing and fills: sliders with their values on LCD chips.
    let mut y = pad.bottom() + 40.0;
    let (x0, x1) = (plate.left() + 16.0, plate.right() - 16.0);
    for (label, key, value) in [
        (tr("Swing"), "swing", &mut d.swing),
        (tr("Fills"), "fills", &mut d.fills),
    ] {
        hw::silk(
            &g,
            pos2(x0 + 44.0, y + 12.0),
            Align2::RIGHT_CENTER,
            label,
            11.0,
            theme::INK2,
        );
        let chip = Rect::from_min_size(pos2(x1 - 52.0, y), vec2(52.0, 24.0));
        crate::piano_roll::lcd_value(&g, chip, &format!("{}", (*value).min(100)));
        let r = Rect::from_min_max(pos2(x0 + 54.0, y + 5.0), pos2(chip.left() - 12.0, y + 19.0));
        let t = f32::from((*value).min(100)) / 100.0;
        if let Some(n) = crate::widgets::slider_h(
            ui,
            &g,
            r,
            egui::Id::unique(("drummer", key, ti, ri)),
            t,
            true,
        ) {
            let new = (n * 100.0).round() as u8;
            if new != *value {
                *value = new;
                changed = true;
            }
        }
        y += 34.0;
    }

    // The kit: the pieces that play, on a fixed grid of equal keys.
    y += 12.0;
    hw::bracket(&g, x0, x1, y, tr("Kit"));
    let cols = 3;
    let kw = (x1 - x0 - 8.0 * (cols - 1) as f32) / cols as f32;
    for (i, part) in parts::LIST.into_iter().enumerate() {
        let (c, r) = (i % cols, i / cols);
        let rect = Rect::from_min_size(
            pos2(x0 + c as f32 * (kw + 8.0), y + 14.0 + r as f32 * 36.0),
            vec2(kw, 28.0),
        );
        if rect.bottom() > plate.bottom() - 4.0 {
            break;
        }
        let on = d.parts & part != 0;
        if hw::toggle(ui, &g, rect, uid(&format!("kit{i}")), on, part_name(part)).clicked() {
            d.parts ^= part;
            changed = true;
        }
    }

    // ---- what it plays: the ruler, the region's header, then a lane for each piece of the
    // kit in the drum-region material (dark, bone hits as tall as their velocity, accents
    // orange), with beat and sixteenth lines and the playhead.
    let lanes = Rect::from_min_max(pos2(panel.right() + 4.0, body.top()), body.max);
    let label_w = 96.0;
    let grid = Rect::from_min_max(
        pos2(lanes.left() + label_w, lanes.top() + 24.0 + 18.0),
        pos2(lanes.right() - 10.0, lanes.bottom() - 6.0),
    );
    let lane_h = (grid.height() / parts::LIST.len() as f32).max(14.0);
    let beat_px = (grid.width() / len_beats.max(1.0) as f32).max(1.0);
    let x_of = |b: f32| grid.left() + b * beat_px;
    let lg = ui.painter_at(lanes);
    let ruler = Rect::from_min_max(
        pos2(grid.left(), lanes.top()),
        pos2(grid.right(), lanes.top() + 24.0),
    );
    let start_bar = p.beats_to_bar(origin).floor().max(0.0) as u32;
    let bars = p.bars_in(origin, origin + len_beats);
    for b in 0..=(len_beats.ceil() as i32) {
        let x = x_of(b as f32);
        if x <= ruler.right() + 0.5 {
            lg.vline(
                x,
                ruler.bottom() - 6.0..=ruler.bottom(),
                Stroke::new(1.0, ink(0.4)),
            );
        }
    }
    for (i, bar) in bars.iter().enumerate() {
        let x = x_of((bar.start - origin) as f32);
        lg.vline(
            x,
            ruler.top() + 4.0..=ruler.bottom(),
            Stroke::new(1.0, theme::INK2),
        );
        hw::text(
            &lg,
            pos2(x + 4.0, ruler.top() + 10.0),
            Align2::LEFT_CENTER,
            &crate::piano_roll::bar_label(start_bar + i as u32 + 1),
            Face::Lcd,
            11.0,
            theme::INK,
        );
    }
    hw::seam_h(&lg, lanes.x_range(), ruler.bottom());
    let head = Rect::from_min_max(
        pos2(grid.left(), ruler.bottom() + 3.0),
        pos2(x_of(len_beats as f32).min(grid.right()), grid.top() - 2.0),
    );
    let look = theme::region_look(p.tracks[ti].color, false, true, false);
    let name = p.tracks[ti].regions[ri].name.clone();
    crate::piano_roll::region_header(&lg, head, &look, "DR", &name);
    let notes = &p.tracks[ti].regions[ri].notes;
    for (i, part) in parts::LIST.into_iter().enumerate() {
        let lane = Rect::from_min_size(
            pos2(grid.left(), grid.top() + i as f32 * lane_h + 2.0),
            vec2(grid.width(), lane_h - 4.0),
        );
        let on = d.parts & part != 0;
        let name = hw::fit(&lg, part_name(part), Face::Silk, 11.0, label_w - 20.0);
        hw::silk(
            &lg,
            pos2(lanes.left() + 12.0, lane.center().y),
            Align2::LEFT_CENTER,
            &name,
            11.0,
            if on { theme::INK } else { theme::PRINT },
        );
        g.rect_filled(lane.translate(vec2(0.0, 1.0)), 3.0, theme::BONE_HI);
        hw::gradient(
            &lg,
            lane,
            3.0,
            Color32::from_rgb(0x26, 0x24, 0x21),
            Color32::from_rgb(0x14, 0x13, 0x12),
        );
        hw::inner_shadow(&lg, lane, 3.0, 0.5);
        let ll = lg.with_clip_rect(lane.shrink(1.0));
        // Sixteenths, beats and bars.
        let sixteenth = 0.25f32;
        let mut t = 0.0f32;
        while t <= len_beats as f32 + 1e-3 {
            let x = x_of(t);
            let beat = (t - t.round()).abs() < 1e-3;
            let a = if beat { 0.25 } else { 0.08 };
            if beat || beat_px * sixteenth >= 5.0 {
                ll.vline(
                    x,
                    lane.y_range(),
                    Stroke::new(1.0, theme::ON_DARK.gamma_multiply(a)),
                );
            }
            t += sixteenth;
        }
        for bar in &bars {
            let x = x_of((bar.start - origin) as f32);
            ll.vline(
                x,
                lane.y_range(),
                Stroke::new(1.0, theme::ON_DARK.gamma_multiply(0.45)),
            );
        }
        if !on {
            // A piece left out of the kit: its lane dims.
            ll.rect_filled(lane, 3.0, theme::BONE.gamma_multiply(0.35));
        }
        for n in notes
            .iter()
            .filter(|n| crate::pattern::part_of(n.pitch) == part)
        {
            let x = x_of(n.start);
            if x > lane.right() || x < lane.left() - 6.0 {
                continue;
            }
            let h = ((lane.height() - 8.0) * n.vel as f32 / 127.0).max(3.0);
            let r = Rect::from_min_max(
                pos2(x + 1.0, lane.bottom() - 4.0 - h),
                pos2(
                    x + 1.0 + (n.len * beat_px).clamp(3.0, 5.0),
                    lane.bottom() - 4.0,
                ),
            );
            let col = if n.vel >= 112 {
                theme::ORANGE
            } else {
                theme::BONE_HI.gamma_multiply(if on { 1.0 } else { 0.5 })
            };
            ll.rect_filled(r, 1.0, col);
        }
    }
    let now = p.position_beats - origin;
    if now >= 0.0 && now < len_beats {
        let x = x_of(now as f32);
        let span = Rect::from_min_max(pos2(x, grid.top()), pos2(x, grid.bottom()));
        crate::piano_roll::playhead(&lg, &lg, x, span, ruler.top() + 1.0);
    }

    if changed {
        d.tidy();
        if let Some(g) = p.tracks[ti].regions[ri].generated.as_mut() {
            g.source = Generator::Drummer(d);
        }
        p.regenerate(ti, ri);
    }
    if convert {
        p.convert_to_midi_region(ti, ri);
    }
}

/// Where the Drummer's pad is in `body` (the editor under its menu row).
fn drummer_pad(body: Rect) -> Rect {
    let w = (PANEL_W - 92.0).min(body.width() - 80.0).max(40.0);
    Rect::from_min_size(pos2(body.left() + 62.0, body.top() + 26.0), vec2(w, 140.0))
}

/// The Drummer's pad: busyness across, loudness up. Returns whether a drag changed them.
fn xy_pad(
    ui: &mut Ui,
    g: &egui::Painter,
    pad: Rect,
    d: &mut DrummerParams,
    (ti, ri): (usize, usize),
) -> bool {
    // The pad is LCD glass in a black bezel; the grid and the puck are printed in LCD ink.
    // Its four ends are named on the faceplate around it.
    let bezel = pad.expand(5.0);
    g.rect_filled(bezel.translate(vec2(0.0, 1.0)), 6.0, theme::BONE_HI);
    hw::gradient(g, bezel, 6.0, theme::CAP_TOP, theme::METER_WELL);
    hw::lcd_glass(g, pad);
    let line = Stroke::new(1.0, theme::LCD_INK.gamma_multiply(0.14));
    for k in 1..4 {
        let t = k as f32 / 4.0;
        g.vline(pad.left() + pad.width() * t, pad.y_range(), line);
        g.hline(pad.x_range(), pad.top() + pad.height() * t, line);
    }
    // Loud and soft beside it at the left, simple and complex under it.
    for (at, anchor, text) in [
        (
            pos2(bezel.left() - 6.0, pad.top() + 6.0),
            Align2::RIGHT_CENTER,
            tr("Loud"),
        ),
        (
            pos2(bezel.left() - 6.0, pad.bottom() - 6.0),
            Align2::RIGHT_CENTER,
            tr("Soft"),
        ),
        (
            bezel.left_bottom() + vec2(0.0, 5.0),
            Align2::LEFT_TOP,
            tr("Simple"),
        ),
        (
            bezel.right_bottom() + vec2(0.0, 5.0),
            Align2::RIGHT_TOP,
            tr("Complex"),
        ),
    ] {
        hw::silk(g, at, anchor, text, 10.0, theme::PRINT);
    }
    let resp = ui.interact(
        pad,
        egui::Id::unique(("drummer_pad", ti, ri)),
        Sense::click_and_drag(),
    );
    let mut changed = false;
    if let (true, Some(pt)) = (
        resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on(),
        resp.interact_pointer_pos(),
    ) {
        let x = ((pt.x - pad.left()) / pad.width()).clamp(0.0, 1.0);
        let y = 1.0 - ((pt.y - pad.top()) / pad.height()).clamp(0.0, 1.0);
        let (c, l) = ((x * 100.0).round() as u8, (y * 100.0).round() as u8);
        if (c, l) != (d.complexity, d.loudness) {
            (d.complexity, d.loudness) = (c, l);
            changed = true;
        }
    }
    let dot = pos2(
        pad.left() + pad.width() * d.complexity.min(100) as f32 / 100.0,
        pad.bottom() - pad.height() * d.loudness.min(100) as f32 / 100.0,
    );
    // The puck: crosshairs through it and a solid block of LCD ink.
    let hair = Stroke::new(1.0, theme::LCD_INK.gamma_multiply(0.4));
    g.vline(dot.x, pad.y_range(), hair);
    g.hline(pad.x_range(), dot.y, hair);
    g.rect_filled(
        Rect::from_center_size(dot, vec2(11.0, 11.0)),
        1.5,
        theme::LCD_INK,
    );
    g.rect_filled(Rect::from_center_size(dot, vec2(5.0, 5.0)), 0.5, theme::LCD);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_named_after_drums_or_notes() {
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(61), "C#3");
        assert_eq!(note_name(0), "C-2");
        assert_eq!(row_name(36, true), "Kick");
        assert_eq!(row_name(36, false), "C1");
        assert_eq!(row_name(37, true), "C#1", "no drum there: its note");
        // Every pitch the Drummer plays has a name.
        for pitch in [
            36, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 54, 56,
        ] {
            assert!(drum_name(pitch).is_some(), "{pitch}");
        }
    }

    #[test]
    fn a_new_row_takes_the_next_drum_or_a_lower_pitch() {
        let mut p = PatternParams::drums();
        // The drum pattern already has a kick, snare, clap, hats, tom and crash.
        assert_eq!(next_row_pitch(&p, true), drums::RIDE);
        assert_eq!(next_row_pitch(&p, false), 34);
        p.rows.clear();
        assert_eq!(next_row_pitch(&p, true), drums::KICK);
        assert_eq!(next_row_pitch(&p, false), 60);
        let m = PatternParams::melodic();
        assert_eq!(next_row_pitch(&m, false), 53);
    }

    #[test]
    fn the_height_of_a_click_is_its_velocity() {
        let cell = Rect::from_min_size(pos2(0.0, 100.0), vec2(30.0, 24.0));
        assert_eq!(velocity_at(100.0, cell), 127);
        assert_eq!(
            velocity_at(124.0, cell),
            16,
            "the bottom is soft, not silent"
        );
        assert!(velocity_at(112.0, cell) > 55 && velocity_at(112.0, cell) < 75);
        assert_eq!(velocity_at(0.0, cell), 127);
    }

    #[test]
    fn drum_tracks_are_the_drummer_tracks() {
        let drummer = Track::new("D", TrackKind::Drummer);
        assert!(is_drum_track(&drummer));
        let synth = Track::new("S", TrackKind::Instrument);
        assert!(!is_drum_track(&synth));
    }

    // ---- the editors with a pointer, in a headless egui (see ui_test.rs)

    use crate::ui_test::Harness;
    use egui::PointerButton;

    const W: f32 = 1000.0;
    const H: f32 = 420.0;

    fn body() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(W, H))
    }

    /// Where a step of the pattern editor is, in a `body()` scrolled to the top (the test
    /// pattern has 8 rows).
    fn pattern_cell(steps: usize, row: usize, step: usize) -> Rect {
        let (grid, top, cw, row_h) = pattern_geom(body(), steps, 8);
        Rect::from_min_size(
            pos2(grid.left() + step as f32 * cw, top + row as f32 * row_h),
            vec2(cw, row_h),
        )
    }

    struct Rig {
        p: Project,
        s: UiState,
        h: Harness,
        t: usize,
        r: usize,
    }

    impl Rig {
        fn pattern() -> Self {
            let mut p = Project::empty();
            let t = p.add_track(TrackKind::Instrument);
            let r = p.add_generated_region(t, false, 1.0).unwrap();
            Self::with(p, t, r)
        }

        fn drummer() -> Self {
            let mut p = Project::empty();
            let t = p.add_track(TrackKind::Drummer);
            let r = p.add_generated_region(t, true, 1.0).unwrap();
            Self::with(p, t, r)
        }

        fn with(p: Project, t: usize, r: usize) -> Self {
            let mut rig = Self {
                p,
                s: UiState::new(),
                h: Harness::new(W, H),
                t,
                r,
            };
            rig.settle();
            rig
        }

        fn settle(&mut self) {
            let (p, s, t, r) = (&mut self.p, &mut self.s, self.t, self.r);
            self.h.settle(|ui| show(ui, p, s, t, r, body()));
        }

        fn click(&mut self, pos: egui::Pos2) {
            let (p, s, t, r) = (&mut self.p, &mut self.s, self.t, self.r);
            self.h.click(pos, |ui| show(ui, p, s, t, r, body()));
        }

        fn right_click(&mut self, pos: egui::Pos2) {
            let (p, s, t, r) = (&mut self.p, &mut self.s, self.t, self.r);
            self.h.right_click(pos, |ui| show(ui, p, s, t, r, body()));
        }

        fn drag(&mut self, path: &[egui::Pos2], button: PointerButton) {
            let (p, s, t, r) = (&mut self.p, &mut self.s, self.t, self.r);
            self.h.drag(path, button, |ui| show(ui, p, s, t, r, body()));
        }

        fn pattern_params(&self) -> PatternParams {
            match &self.p.tracks[self.t].regions[self.r]
                .generated
                .as_ref()
                .unwrap()
                .source
            {
                Generator::Pattern(p) => p.clone(),
                other => panic!("{other:?}"),
            }
        }

        fn drummer_params(&self) -> DrummerParams {
            match &self.p.tracks[self.t].regions[self.r]
                .generated
                .as_ref()
                .unwrap()
                .source
            {
                Generator::Drummer(d) => d.clone(),
                other => panic!("{other:?}"),
            }
        }

        fn notes(&self) -> usize {
            self.p.tracks[self.t].regions[self.r].notes.len()
        }
    }

    #[test]
    fn clicking_an_empty_step_switches_it_on_and_the_notes_follow() {
        let mut rig = Rig::pattern();
        assert_eq!(rig.notes(), 0);
        // Near the top of the cell: a hard step.
        let c = pattern_cell(16, 2, 4);
        rig.click(pos2(c.center().x, c.top() + 2.0));
        let pat = rig.pattern_params();
        assert!(pat.rows[2].steps[4] >= 110, "{}", pat.rows[2].steps[4]);
        assert_eq!(pat.rows[2].steps.iter().filter(|v| **v > 0).count(), 1);
        // Four bars of 4/4 loop the one bar of 16 steps four times.
        let notes = &rig.p.tracks[rig.t].regions[rig.r].notes;
        assert_eq!(notes.len(), 4);
        assert_eq!(notes[0].start, 1.0);
        assert_eq!(notes[0].pitch, pat.rows[2].pitch);
        // Near the bottom: soft.
        let c = pattern_cell(16, 3, 9);
        rig.click(pos2(c.center().x, c.bottom() - 2.0));
        assert!(rig.pattern_params().rows[3].steps[9] < 40);
    }

    #[test]
    fn clicking_a_step_that_is_on_switches_it_off_and_dragging_in_it_sets_its_velocity() {
        let mut rig = Rig::pattern();
        let c = pattern_cell(16, 0, 1);
        rig.click(c.center());
        assert!(rig.pattern_params().rows[0].steps[1] > 0);
        rig.click(c.center());
        assert_eq!(
            rig.pattern_params().rows[0].steps[1],
            0,
            "a click switched it off"
        );
        assert_eq!(rig.notes(), 0);
        // On again; dragging down inside it makes it softer instead of switching it off.
        rig.click(pos2(c.center().x, c.top() + 2.0));
        let loud = rig.pattern_params().rows[0].steps[1];
        rig.drag(
            &[
                pos2(c.center().x, c.top() + 3.0),
                pos2(c.center().x, c.top() + 12.0),
                pos2(c.center().x, c.bottom() - 1.0),
            ],
            PointerButton::Primary,
        );
        let soft = rig.pattern_params().rows[0].steps[1];
        assert!(soft > 0 && soft < loud / 2, "{loud} -> {soft}");
    }

    #[test]
    fn dragging_across_steps_paints_them_and_a_right_drag_clears_them() {
        let mut rig = Rig::pattern();
        let y = pattern_cell(16, 1, 0).center().y;
        let x = |k: usize| pattern_cell(16, 1, k).center().x;
        rig.drag(
            &[pos2(x(3), y), pos2(x(4), y), pos2(x(5), y), pos2(x(6), y)],
            PointerButton::Primary,
        );
        let on: Vec<usize> = (0..16)
            .filter(|k| rig.pattern_params().rows[1].steps[*k] > 0)
            .collect();
        assert_eq!(on, [3, 4, 5, 6]);
        rig.drag(&[pos2(x(4), y), pos2(x(5), y)], PointerButton::Secondary);
        let on: Vec<usize> = (0..16)
            .filter(|k| rig.pattern_params().rows[1].steps[*k] > 0)
            .collect();
        assert_eq!(on, [3, 6]);
        rig.right_click(pos2(x(3), y));
        assert_eq!(rig.pattern_params().rows[1].steps[3], 0);
    }

    #[test]
    fn the_edit_modes_set_chance_repeat_and_tie_on_steps_that_are_on() {
        let mut rig = Rig::pattern();
        for k in [2, 3] {
            rig.click(pattern_cell(16, 0, k).center());
        }
        // Chance: a drag to the bottom of the step lowers it; an empty step is left alone.
        rig.s.steps.mode = EditMode::Chance;
        let c = pattern_cell(16, 0, 2);
        rig.drag(
            &[
                pos2(c.center().x, c.top() + 3.0),
                pos2(c.center().x, c.top() + 10.0),
                pos2(c.center().x, c.bottom() - 4.0),
            ],
            PointerButton::Primary,
        );
        let chance = rig.pattern_params().rows[0].chance[2];
        assert!(chance < 30, "{chance}");
        rig.click(pattern_cell(16, 0, 7).center());
        assert_eq!(
            rig.pattern_params().rows[0].steps[7],
            0,
            "no step made in this mode"
        );
        assert_eq!(rig.pattern_params().rows[0].chance[7], 100);
        // Repeat: each click counts up to 4 and round to 1; a right-click resets.
        rig.s.steps.mode = EditMode::Repeat;
        let c = pattern_cell(16, 0, 3).center();
        let seen: Vec<u8> = (0..5)
            .map(|_| {
                rig.click(c);
                rig.pattern_params().rows[0].repeat[3]
            })
            .collect();
        assert_eq!(seen, [2, 3, 4, 1, 2]);
        rig.right_click(c);
        assert_eq!(rig.pattern_params().rows[0].repeat[3], 1);
        // Tie: a click ties the step to the one before, another unties it.
        rig.s.steps.mode = EditMode::Tie;
        rig.click(c);
        assert!(rig.pattern_params().rows[0].tie[3]);
        // (The chance of step 2 is put back, so every bar plays.)
        if let Some(g) = rig.p.tracks[rig.t].regions[rig.r].generated.as_mut() {
            if let Generator::Pattern(pat) = &mut g.source {
                pat.rows[0].chance[2] = 100;
            }
        }
        rig.p.regenerate(rig.t, rig.r);
        // Steps 2 and 3 are tied: one note a bit longer than two steps' worth, repeated in
        // each of the four bars.
        let notes = &rig.p.tracks[rig.t].regions[rig.r].notes;
        assert_eq!(notes.len(), 4);
        assert!(
            notes[0].len > 0.25 && notes[0].len < 0.6,
            "{}",
            notes[0].len
        );
        rig.click(c);
        assert!(!rig.pattern_params().rows[0].tie[3]);
        assert_eq!(rig.notes(), 8);
    }

    #[test]
    fn dragging_a_rows_name_changes_its_pitch_and_it_sounds_while_held() {
        let mut rig = Rig::pattern();
        let before = rig.pattern_params().rows[1].pitch;
        let y = pattern_cell(16, 1, 0).center().y;
        let x = 40.0;
        let (p, s, t, r) = (&mut rig.p, &mut rig.s, rig.t, rig.r);
        rig.h.press(pos2(x, y), PointerButton::Primary, |ui| {
            show(ui, p, s, t, r, body())
        });
        assert!(
            rig.s
                .audition
                .iter()
                .any(|a| matches!(a, Audition::On { pitch, .. } if *pitch == before)),
            "it sounds"
        );
        let (p, s, t, r) = (&mut rig.p, &mut rig.s, rig.t, rig.r);
        for dy in [-6.0, -12.0, -20.0] {
            rig.h
                .frame(vec![egui::Event::PointerMoved(pos2(x, y + dy))], |ui| {
                    show(ui, p, s, t, r, body())
                });
        }
        rig.h
            .release(pos2(x, y - 20.0), PointerButton::Primary, |ui| {
                show(ui, p, s, t, r, body())
            });
        rig.h.frame(vec![], |ui| show(ui, p, s, t, r, body()));
        // egui starts a drag after a few points of movement, so 20 points up is one or two
        // semitones.
        let moved = rig.pattern_params().rows[1].pitch as i32 - before as i32;
        assert!((1..=2).contains(&moved), "{moved}");
        assert!(
            rig.s
                .audition
                .iter()
                .any(|a| matches!(a, Audition::Off { .. })),
            "and it stops"
        );
    }

    #[test]
    fn the_drummer_pad_sets_complexity_and_loudness_and_the_notes_follow() {
        let mut rig = Rig::drummer();
        let pad = drummer_pad(Rect::from_min_max(pos2(0.0, TOOLBAR_H), pos2(W, H)));
        rig.click(pad.center());
        let d = rig.drummer_params();
        assert!(
            (48..=52).contains(&d.complexity) && (48..=52).contains(&d.loudness),
            "{d:?}"
        );
        rig.drag(
            &[
                pad.center(),
                pad.right_top() + vec2(-10.0, 10.0),
                pad.right_top() + vec2(30.0, -30.0),
            ],
            PointerButton::Primary,
        );
        let d = rig.drummer_params();
        assert_eq!(
            (d.complexity, d.loudness),
            (100, 100),
            "dragged past the corner"
        );
        let busy = rig.notes();
        rig.drag(
            &[
                pad.center(),
                pad.left_bottom() + vec2(5.0, -5.0),
                pad.left_bottom() + vec2(-30.0, 30.0),
            ],
            PointerButton::Primary,
        );
        let d = rig.drummer_params();
        assert_eq!((d.complexity, d.loudness), (0, 0));
        assert!(
            rig.notes() < busy,
            "a simple, soft drummer plays less ({} vs {busy})",
            rig.notes()
        );
    }

    #[test]
    fn both_editors_draw_at_small_sizes_without_trouble() {
        for (w, h) in [(1000.0, 420.0), (640.0, 200.0), (300.0, 120.0)] {
            for drummer in [false, true] {
                let mut rig = if drummer {
                    Rig::drummer()
                } else {
                    Rig::pattern()
                };
                rig.h = Harness::new(w, h);
                let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
                let (p, s, t, r) = (&mut rig.p, &mut rig.s, rig.t, rig.r);
                for _ in 0..3 {
                    rig.h.frame(vec![], |ui| show(ui, p, s, t, r, body));
                }
            }
        }
    }
}
