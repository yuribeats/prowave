//! Piano Roll (Logic Pro: Piano Roll editor): editor tabs, local toolbar, the left inspector
//! (Time Quantize, Scale Quantize, Velocity), piano keys, ruler with the region strip, and
//! the note grid.
//! <https://support.apple.com/guide/logicpro/piano-roll-editor-overview-lgcp5b93e0e2/mac>
//!
//! Tools (`piano_tools`): Pointer, Pencil, Eraser, Scissors, Glue, Velocity, Quantize, Brush,
//! Finger and Zoom, picked for the left button and for Command-click in the toolbar, or with
//! the Tool menu (T, then the tool's key).
//!
//! Editing: select (click, Shift-click, marquee, a key of the keyboard for all notes of a
//! pitch), move (time and pitch, with Snap), Option-drag to copy, resize from either edge,
//! create (click), delete (Del or right-click), Cut/Copy/Paste (at the playhead), Duplicate,
//! arrow keys (grid; Option: one tick), Time Quantize with Strength and Swing, Scale
//! Quantize, Transpose, Force Legato, Remove Overlaps, Set Length to the grid, Fixed Velocity,
//! Humanize, velocity editing, zoom and scrolling.
//!
//! View: note names on the notes when zoomed in, Collapse Mode (only the pitches in use), the
//! scale highlighted or folded, the other regions of the track dimmed (click a note to edit
//! its region), the chord the selection makes, Catch (follow the playhead) and Link (follow
//! the selected region). Edits go to the project and reach the engine when the mouse is
//! released. The Score tab switches an ordinary MIDI region to the Score editor; Pattern and
//! Drummer regions open in the Step Sequencer and the Drummer Editor (`step_editor`).
use crate::hw::{self, Cap, Legend};
use crate::i18n::{tr, trf};
use crate::midi_draw::{self, Lane, LaneDrag};
use crate::model::{Note, Project, TrackKind};
use crate::note_edit::{self, Part, GRIDS, MODES, ROOTS};
use crate::piano_tools::{draw_icon, Tool};
use crate::theme::{self, Face};
use crate::tracks_area::{snap_step, snap_to, Snap};
use crate::widgets::{menu_title, popup_button, slider_h};
use crate::UiState;
use egui::{pos2, vec2, Align2, Color32, Key, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui};

/// The editors' menu row: section brackets along its top, keys 28 tall under them.
pub(crate) const TOOLBAR_H: f32 = 58.0;
/// Where a menu row's brackets and keys sit, from its top.
pub(crate) const BRACKET_Y: f32 = 11.0;
pub(crate) const KEY_Y: f32 = 22.0;
pub(crate) const KEY_H: f32 = 28.0;
const INSPECTOR_W: f32 = 248.0;
const KEYS_W: f32 = 60.0;
const RULER_H: f32 = 24.0;
const REGION_H: f32 = 18.0;
const SB_H: f32 = 12.0;
const VEL_H: f32 = 76.0;

// ---------------------------------------------------------------------------------------------
// Hardware pieces the MIDI editors share (Piano Roll, Score, Step Sequencer, Drummer, Event
// List). Colours are the theme's tokens; mixes of two tokens stand in for the in-between
// shades of the mockup (key rows, grid lines).

/// The ink of the grid lines at `alpha` (bar lines .22, beats .07, as on the arrange floor).
pub(crate) fn ink(alpha: f32) -> Color32 {
    theme::INK.gamma_multiply(alpha)
}

/// The note grid's rows: white keys on the recess, black keys a shade deeper.
const ROW_WHITE: Color32 = theme::BONE2;
fn row_black() -> Color32 {
    hw::lerp(theme::BONE2, theme::BONE3, 0.62)
}
/// Rows of pitches outside the scale, when the scale is highlighted: the deeper lane tone.
fn row_out() -> Color32 {
    hw::lerp(theme::BONE3, theme::SEAM, 0.3)
}

/// A note's velocity as the depth of its black: soft notes print grey, hard ones black (the
/// Hardware look tells values apart by material, not by hue).
pub(crate) fn velocity_color(vel: u8) -> Color32 {
    let t = (vel as f32 / 127.0).clamp(0.0, 1.0).powf(0.8);
    hw::lerp(theme::SEAM_DK, theme::CAP, t)
}

/// A strip of the chassis (the tab row, a menu row) with a seam along its bottom.
pub(crate) fn menu_row(g: &Painter, r: Rect) {
    hw::chassis(g, r);
    hw::seam_h(g, r.x_range(), r.bottom() - 2.0);
}

/// The editor mode keys in a menu row (`top` is the row's top), under a MODE bracket: a
/// latched pair of bone keys with LEDs, the active one down and lit. Returns the key clicked
/// (its number in `tabs`) and the right edge of the group.
pub(crate) fn mode_keys(
    ui: &Ui,
    g: &Painter,
    left: f32,
    top: f32,
    tabs: &[(usize, &str)],
    active: usize,
    clickable: bool,
) -> (Option<usize>, f32) {
    let size = 11.5;
    let widths: Vec<f32> = tabs
        .iter()
        .map(|(_, t)| {
            hw::galley(g, &t.to_uppercase(), Face::Bold, size, theme::INK)
                .size()
                .x
                + 34.0
        })
        .collect();
    let gap = 6.0;
    let total: f32 = widths.iter().sum::<f32>() + gap * (tabs.len().saturating_sub(1)) as f32;
    hw::bracket(g, left, left + total, top + BRACKET_Y, tr("Mode"));
    let mut x = left;
    let mut clicked = None;
    for ((i, t), w) in tabs.iter().copied().zip(&widths) {
        let r = Rect::from_min_size(pos2(x, top + KEY_Y), vec2(*w, KEY_H));
        let on = i == active;
        let (resp, _) = hw::key(
            ui,
            g,
            r,
            egui::Id::unique(("editor_tab", i)),
            Cap::Bone,
            on,
            Some(on),
            Legend::Text(t),
        );
        if clickable && resp.clicked() {
            clicked = Some(i);
        }
        x += w + gap;
    }
    (clicked, left + total)
}

/// The editor mode keys for the region on show (see [`mode_keys`]): Piano Roll and Score for
/// an ordinary MIDI region (clickable when `switch` allows), else only the editor that region
/// opens in. Returns the key clicked (0 Piano Roll, 1 Score) and the group's right edge.
pub(crate) fn editor_keys(
    ui: &Ui,
    g: &Painter,
    left: f32,
    top: f32,
    active: usize,
    switch: bool,
) -> (Option<usize>, f32) {
    let tabs: Vec<(usize, &str)> = match active {
        2 => vec![(2, tr("Step Sequencer"))],
        3 => vec![(3, tr("Drummer Editor"))],
        _ if switch => vec![(0, tr("Piano Roll")), (1, tr("Score"))],
        _ => vec![(0, tr("Piano Roll"))],
    };
    let (clicked, right) = mode_keys(ui, g, left, top, &tabs, active, switch);
    (clicked.filter(|i| *i < 2), right)
}

/// A bone key carrying a drawn icon, an LED at its left lit while `on` (latched then).
/// `orange` gives it the signal cap while on (MIDI In, like Record).
pub(crate) fn icon_key(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    id: egui::Id,
    on: bool,
    orange: bool,
    draw: impl FnOnce(&Painter, Pos2, Color32),
) -> egui::Response {
    let cap = if on && orange { Cap::Orange } else { Cap::Bone };
    let (resp, body) = hw::key(ui, g, r, id, cap, on, Some(on), Legend::Custom);
    draw(g, body.center() - vec2(1.0, 0.0), cap.ink());
    resp
}

/// A scroll bar: a recessed track and a black thumb with a grip line.
pub(crate) fn paint_scrollbar(g: &Painter, track: Rect, thumb: Rect, hot: bool) {
    g.rect_filled(track, 0.0, theme::BONE);
    let slot = track.shrink(2.0);
    hw::recess(g, slot, 3.0);
    let top = if hot {
        hw::lerp(theme::CAP_TOP, theme::BONE, 0.18)
    } else {
        theme::CAP_TOP
    };
    g.rect_filled(thumb.translate(vec2(0.0, 1.0)), 3.0, ink(0.35));
    hw::gradient(g, thumb, 3.0, top, theme::CAP);
    g.rect_stroke(
        thumb,
        3.0,
        Stroke::new(1.0, theme::CAP_EDGE),
        egui::StrokeKind::Inside,
    );
    let c = thumb.center();
    let grip = Stroke::new(1.0, theme::BONE_HI.gamma_multiply(0.45));
    if thumb.width() >= thumb.height() {
        for dx in [-3.0, 0.0, 3.0] {
            g.vline(c.x + dx, c.y - 2.5..=c.y + 2.5, grip);
        }
    } else {
        for dy in [-3.0, 0.0, 3.0] {
            g.hline(c.x - 2.5..=c.x + 2.5, c.y + dy, grip);
        }
    }
}

/// A small LCD chip with `value` in segments at its right (the inspector's values).
pub(crate) fn lcd_value(g: &Painter, r: Rect, value: &str) {
    hw::lcd_glass(g, r);
    let h = (r.height() - 8.0).clamp(8.0, 15.0);
    let w = hw::seg_width(value, h);
    hw::seg(
        g,
        pos2(r.right() - 6.0 - w, r.center().y - h / 2.0),
        value,
        h,
        theme::LCD_INK,
        true,
    );
}

/// A region's header bar in the editor's region strip: the type chip and the name, in the
/// region material (`look`); the selected one orange with an ink outline.
pub(crate) fn region_header(g: &Painter, r: Rect, look: &theme::RegionLook, tag: &str, name: &str) {
    let radius = 2.0;
    g.rect_filled(r.translate(vec2(0.0, 1.0)), radius, ink(0.18));
    g.rect_filled(r, radius, look.header);
    let clip = g.with_clip_rect(r.intersect(g.clip_rect()));
    let mut x = r.left() + 4.0;
    if !tag.is_empty() && r.width() > 40.0 {
        let gal = hw::galley(&clip, tag, Face::Lcd, 9.0, look.header);
        let chip = Rect::from_min_size(pos2(x, r.center().y - 5.5), vec2(gal.size().x + 6.0, 11.0));
        clip.rect_filled(chip, 1.5, look.header_ink);
        clip.galley(
            pos2(chip.left() + 3.0, chip.center().y - gal.size().y / 2.0),
            gal,
            look.header,
        );
        x = chip.right() + 6.0;
    }
    let shown = hw::fit(
        &clip,
        &name.to_uppercase(),
        Face::Silk,
        12.0,
        (r.right() - x - 4.0).max(0.0),
    );
    hw::text(
        &clip,
        pos2(x, r.center().y),
        Align2::LEFT_CENTER,
        &shown,
        Face::Silk,
        12.0,
        look.header_ink,
    );
    let outline = if look.outline == theme::ORANGE {
        Stroke::new(1.5, theme::INK)
    } else {
        Stroke::new(1.0, ink(0.75))
    };
    g.rect_stroke(r, radius, outline, egui::StrokeKind::Inside);
}

/// The playhead over `area` (a 2-point orange line with a thin ink outline) and its flag, an
/// orange pentagon with an ink edge, hanging from `flag_top`.
pub(crate) fn playhead(g: &Painter, flag: &Painter, x: f32, area: Rect, flag_top: f32) {
    g.vline(x, area.y_range(), Stroke::new(3.0, ink(0.55)));
    g.vline(x, area.y_range(), Stroke::new(2.0, theme::ORANGE));
    let pts = vec![
        pos2(x - 6.0, flag_top),
        pos2(x + 6.0, flag_top),
        pos2(x + 6.0, flag_top + 9.0),
        pos2(x, flag_top + 15.0),
        pos2(x - 6.0, flag_top + 9.0),
    ];
    flag.add(Shape::convex_polygon(
        pts,
        theme::ORANGE,
        Stroke::new(1.0, theme::INK),
    ));
}

/// Diagonal 135° hatch lines `pitch` apart over `r` (the Drummer material, ranges outside
/// a region).
pub(crate) fn hatch(g: &Painter, r: Rect, pitch: f32, stroke: Stroke) {
    if r.width() <= 0.0 || r.height() <= 0.0 {
        return;
    }
    let c = g.with_clip_rect(r.intersect(g.clip_rect()));
    let h = r.height();
    let mut x = r.left() - h;
    // Start on a multiple of `pitch` so the hatch stays put while the view scrolls a little.
    x -= x.rem_euclid(pitch);
    while x < r.right() {
        c.line_segment([pos2(x, r.bottom()), pos2(x + h, r.top())], stroke);
        x += pitch;
    }
}

/// A left-to-right gradient over `r` (the keyboard's keys, lit from the grid side).
pub(crate) fn gradient_h(g: &Painter, r: Rect, left: Color32, right: Color32) {
    let mut mesh = egui::epaint::Mesh::default();
    mesh.colored_vertex(r.left_top(), left);
    mesh.colored_vertex(r.right_top(), right);
    mesh.colored_vertex(r.right_bottom(), right);
    mesh.colored_vertex(r.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    g.add(Shape::mesh(mesh));
}

/// One key of the Piano Roll's keyboard in its row `r`. White keys are bone, shaded towards
/// their back, with a seam where two white keys meet (`seam_below`: the row under it is a
/// white key too). A black key's row has a black cap with a lip over its back part and the
/// seam between the two white keys either side of it at its middle. `held` (sounding) lights
/// the key orange.
pub(crate) fn paint_piano_key(g: &Painter, r: Rect, black: bool, held: bool, seam_below: bool) {
    let lit = |c: Color32| {
        if held {
            hw::lerp(c, theme::ORANGE, 0.6)
        } else {
            c
        }
    };
    gradient_h(
        g,
        r,
        lit(Color32::from_rgb(0xdc, 0xd7, 0xcb)),
        lit(theme::BONE_TOP),
    );
    // The white key's front edge, at the grid.
    g.vline(
        r.right() - 1.5,
        r.y_range(),
        Stroke::new(1.0, Color32::from_black_alpha(22)),
    );
    let seam = Stroke::new(1.0, theme::SEAM);
    if black {
        let b = Rect::from_min_size(r.min, vec2(r.width() * 0.6, r.height()));
        let cr = egui::CornerRadius {
            nw: 0,
            sw: 0,
            ne: 2,
            se: 2,
        };
        g.hline(b.right()..=r.right(), r.center().y, seam);
        // The lip: the cap's hard edge below and at its end.
        g.rect_filled(b.translate(vec2(1.0, 2.0)), cr, ink(0.25));
        g.rect_filled(b.translate(vec2(0.0, 1.5)), cr, Color32::from_rgb(5, 5, 5));
        let face = Rect::from_min_max(b.min, pos2(b.right(), b.bottom() - 1.0));
        let (top, bottom) = if held {
            (
                hw::lerp(theme::CAP_TOP, theme::ORANGE, 0.6),
                theme::ORANGE_DK,
            )
        } else {
            (theme::CAP_TOP, Color32::from_rgb(0x15, 0x14, 0x13))
        };
        hw::gradient(g, face, 2.0, top, bottom);
        g.hline(
            face.left()..=face.right() - 2.0,
            face.top() + 1.0,
            Stroke::new(1.0, Color32::from_white_alpha(30)),
        );
        g.vline(
            face.right() - 3.0,
            face.top() + 2.0..=face.bottom() - 1.0,
            Stroke::new(1.0, Color32::from_white_alpha(22)),
        );
    } else if seam_below {
        g.hline(r.x_range(), r.bottom() - 0.5, seam);
        g.hline(
            r.left()..=r.right() - 1.0,
            r.bottom() + 0.5,
            Stroke::new(1.0, theme::BONE_HI),
        );
    }
}

/// A note in `r` as a small key cap: a black cap whose depth gives its velocity, with a hard
/// lip under it, a lit top edge and a velocity line along its foot; selected notes take the
/// orange cap, muted ones are left as an outline. `hovered` lifts the face a little. Returns
/// the colour for text on it.
pub(crate) fn paint_note(
    g: &Painter,
    r: Rect,
    vel: u8,
    selected: bool,
    muted: bool,
    hovered: bool,
) -> Color32 {
    let radius = 2.0;
    if muted {
        g.rect_filled(r, radius, theme::BONE_HI.gamma_multiply(0.55));
        let edge = if selected {
            Stroke::new(1.5, theme::ORANGE)
        } else {
            Stroke::new(1.0, theme::PRINT)
        };
        g.rect_stroke(r, radius, edge, egui::StrokeKind::Inside);
        return theme::PRINT;
    }
    let (top, bottom, edge, lip, hi, line, print) = if selected {
        (
            theme::ORANGE_HI,
            Color32::from_rgb(0xee, 0x4e, 0x0c),
            Color32::from_rgb(0xb2, 0x3a, 0x06),
            Color32::from_rgb(0xa8, 0x36, 0x05),
            Color32::from_white_alpha(110),
            theme::INK.gamma_multiply(0.6),
            theme::INK,
        )
    } else {
        let c = velocity_color(vel);
        (
            hw::lerp(c, theme::BONE, 0.16),
            c,
            theme::CAP_EDGE,
            Color32::from_rgb(5, 5, 5),
            Color32::from_white_alpha(36),
            theme::ON_DARK.gamma_multiply(0.5),
            theme::ON_DARK,
        )
    };
    let lift = if r.height() >= 8.0 { 1.5 } else { 1.0 };
    let face = Rect::from_min_max(r.min, pos2(r.right(), r.bottom() - lift));
    g.rect_filled(r.translate(vec2(0.0, 1.0)), radius, ink(0.18));
    g.rect_filled(r, radius, lip);
    let (top, bottom) = if hovered {
        (
            hw::lerp(top, Color32::WHITE, 0.08),
            hw::lerp(bottom, Color32::WHITE, 0.06),
        )
    } else {
        (top, bottom)
    };
    hw::gradient(g, face, radius, top, bottom);
    g.rect_stroke(
        face,
        radius,
        Stroke::new(1.0, edge),
        egui::StrokeKind::Inside,
    );
    if face.width() >= 6.0 {
        g.hline(
            face.left() + 1.5..=face.right() - 1.5,
            face.top() + 1.5,
            Stroke::new(1.0, hi),
        );
    }
    if face.height() >= 13.0 && face.width() >= 8.0 {
        let w = (face.width() - 5.0) * f32::from(vel.min(127)) / 127.0;
        g.hline(
            face.left() + 2.5..=face.left() + 2.5 + w,
            face.bottom() - 2.0,
            Stroke::new(1.0, line),
        );
    }
    print
}

/// The grid outside the region edited: a deeper floor with a fine hatch.
fn outside(g: &Painter, r: Rect) {
    g.rect_filled(r, 0.0, ink(0.07));
    hatch(g, r, 7.0, Stroke::new(1.0, ink(0.06)));
}

/// Sets `ui`'s own look so egui's value widgets in it (DragValue, ComboBox, text fields) are
/// drawn as the Hardware dark fields: black face, bone print, a dark edge. Typing and
/// dragging stay as they are.
pub(crate) fn dark_fields(ui: &mut Ui) {
    let v = ui.visuals_mut();
    let r = egui::CornerRadius::same(3);
    for (w, face) in [
        (&mut v.widgets.inactive, theme::CAP),
        (&mut v.widgets.hovered, theme::CAP_TOP),
        (&mut v.widgets.active, theme::CAP_TOP),
        (&mut v.widgets.open, theme::CAP_TOP),
    ] {
        w.bg_fill = face;
        w.weak_bg_fill = face;
        w.bg_stroke = Stroke::new(1.0, theme::CAP_EDGE);
        w.fg_stroke = Stroke::new(1.0, theme::ON_DARK);
        w.corner_radius = r;
        w.expansion = 0.0;
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, theme::INK2);
    v.extreme_bg_color = theme::METER_WELL;
    v.override_text_color = Some(theme::ON_DARK);
    v.selection.bg_fill = theme::ORANGE;
    v.selection.stroke = Stroke::new(1.0, theme::INK);
    v.text_cursor.stroke = Stroke::new(2.0, theme::ORANGE);
    ui.spacing_mut().interact_size.y = 24.0;
    ui.spacing_mut().button_padding = vec2(8.0, 4.0);
}

/// A label as silkscreen prints it: without the colon the plain text puts after it.
pub(crate) fn caption(text: &str) -> &str {
    text.trim_end().trim_end_matches([':', '：']).trim_end()
}

/// A silkscreened label laid out in `ui` (caps, condensed, print colour).
pub(crate) fn silk_label(ui: &mut Ui, text: &str) {
    let text = caption(text);
    let g = ui.painter().clone();
    let w = hw::silk_width(&g, text, 11.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    hw::silk(
        &g,
        r.left_center(),
        Align2::LEFT_CENTER,
        text,
        11.0,
        theme::INK2,
    );
}

/// A bone key with a legend, laid out in `ui` (26 tall); greyed and inert when not `enabled`.
pub(crate) fn key_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    let g = ui.painter().clone();
    let text = label.to_uppercase();
    let w = hw::galley(&g, &text, Face::Bold, 11.0, theme::INK).size().x + 24.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    if !enabled {
        let face = hw::paint_key(&g, r, Cap::Bone, false, false);
        hw::text(
            &g,
            face.center(),
            Align2::CENTER_CENTER,
            &text,
            Face::Bold,
            11.0,
            theme::SEAM_DK,
        );
        return resp;
    }
    hw::key(
        ui,
        &g,
        r,
        resp.id.with("key"),
        Cap::Bone,
        false,
        None,
        Legend::Text(label),
    )
    .0
}

/// An editor with nothing to show: a recessed well with `msg` in print.
pub(crate) fn empty_well(g: &Painter, body: Rect, msg: &str) {
    let well = body.shrink(10.0);
    hw::recess(g, well, 3.0);
    hw::text(
        g,
        well.center(),
        Align2::CENTER_CENTER,
        msg,
        Face::Body,
        13.0,
        theme::TEXT_DIM,
    );
}

/// A bar number on a ruler, zero-padded to three digits as on the main ruler ("005").
pub(crate) fn bar_label(n: u32) -> String {
    format!("{n:03}")
}

/// Live notes the Piano Roll wants to play; `main` forwards them to the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audition {
    On { pitch: u8, vel: u8, track: u16 },
    Off { pitch: u8 },
}

/// How the scale chosen under Scale Quantize shows on the grid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScaleView {
    Off,
    /// The rows of the scale's notes are lit, its root most.
    Highlight,
    /// Only the scale's rows are shown (and those of notes outside it).
    Fold,
}

enum Drag {
    None,
    /// Pencil: a note was just created; dragging sets its length.
    Create {
        index: usize,
    },
    Move {
        anchor_beat: f32,
        anchor_pitch: i32,
        orig: Vec<(usize, f32, u8)>,
        /// Option-drag: the notes moved are copies made when the drag began.
        copies: bool,
    },
    Resize {
        anchor_beat: f32,
        orig: Vec<(usize, f32)>,
    },
    /// The left edge: the start moves, the end stays.
    ResizeLeft {
        anchor_beat: f32,
        orig: Vec<(usize, f32, f32)>,
    },
    Marquee {
        from: Pos2,
    },
    /// Velocity tool: the pointer's height when the drag began, and the velocities then.
    Velocity {
        anchor_y: f32,
        orig: Vec<(usize, u8)>,
        /// Option-Shift: every note gets the velocity of the one grabbed.
        equal: bool,
    },
    /// Quantize tool held down: the notes it has quantized.
    Quantize {
        done: Vec<usize>,
    },
    /// Eraser held down: notes the pointer passes are deleted.
    Erase,
    /// Brush held down: where the pointer was last frame (beats).
    Brush {
        last: f32,
        pitch: u8,
    },
}

pub struct PianoView {
    /// The left-click tool.
    pub tool: Tool,
    /// The Command-click tool.
    pub cmd_tool: Tool,
    /// The Tool menu, open at this point (T).
    tool_menu: Option<Pos2>,
    /// Pitch currently sounding because of the mouse (key press or note edit).
    held: Option<u8>,
    /// Fit the region to the width of the editor on the next frame.
    fit: bool,
    /// Note whose velocity stem is being dragged in the velocity lane.
    vel_drag: Option<usize>,
    /// Keep `held` sounding until this time even if the mouse is up (click-created notes).
    release_at: Option<f64>,
    pub beat_px: f32,
    pub key_h: f32,
    scroll_x: f32,
    scroll_y: f32,
    pub selected: Vec<usize>,
    grid: usize,
    strength: f32,
    swing: f32,
    scale_root: Option<u8>,
    scale_mode: usize,
    new_velocity: u8,
    drag: Drag,
    centered_for: Option<(usize, usize)>,
    /// Edits waiting for the mouse to be released before the engine song is rebuilt.
    dirty: bool,
    /// Catch: the view follows the playhead while playing.
    pub catch: bool,
    /// Link: the editor shows the region selected in the Tracks area. Off: it stays on
    /// `pinned`.
    pub link: bool,
    pinned: Option<(usize, usize)>,
    /// Collapse Mode: only the pitches that have notes get a row.
    pub collapse: bool,
    pub scale_view: ScaleView,
    /// Note names on the notes (when they are tall and wide enough).
    pub note_labels: bool,
    /// The other regions of the track, dimmed, around the one edited.
    pub show_others: bool,
    /// Snap to the grid's absolute positions instead of moving by whole grid steps.
    pub snap_absolute: bool,
    /// Notes cut or copied, for Paste.
    clipboard: Vec<Note>,
    /// MIDI In: notes played on a keyboard (or Musical Typing) are entered at the playhead.
    pub midi_in: bool,
    step: note_edit::StepInput,
    /// The grid step of the last frame (the length of step-input notes), in beats.
    last_step: f32,
    /// The chord the selected notes make, for the toolbar.
    chord: Option<String>,
    /// What the lane under the notes shows (Automation/MIDI), and whether it is shown (A).
    pub lane: Lane,
    pub show_lane: bool,
    /// The lane shows Pro Wave Tune's Pitch Correction instead (see `tune_lane`), and whether
    /// the region on show has one to show.
    pub tune_lane: bool,
    tune_here: bool,
    lane_drag: LaneDrag,
    /// The point picked in a controller lane.
    lane_sel: Option<usize>,
    /// Where the note grid was drawn last (for tests and the separate window).
    grid_rect: Rect,
    /// The key last clicked on the keyboard (the other end of a Shift-click range).
    key_anchor: Option<u8>,
    /// Functions > Fixed Velocity, Humanize amounts (ticks, velocity).
    fixed_vel: u8,
    humanize_ticks: u32,
    humanize_vel: u8,
    humanize_seed: u32,
}

impl PianoView {
    /// Whether the Tool menu (T) is open: its tool keys (P, I, …) are its own then.
    pub(crate) fn tool_menu_open(&self) -> bool {
        self.tool_menu.is_some()
    }

    #[cfg(test)]
    pub(crate) fn open_tool_menu(&mut self) {
        self.tool_menu = Some(Pos2::ZERO);
    }
}

impl Default for PianoView {
    fn default() -> Self {
        Self {
            tool: Tool::Pointer,
            cmd_tool: Tool::Pencil,
            tool_menu: None,
            held: None,
            fit: true,
            vel_drag: None,
            release_at: None,
            beat_px: 64.0,
            key_h: 12.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            selected: Vec::new(),
            grid: 3,
            strength: 1.0,
            swing: 0.0,
            scale_root: None,
            scale_mode: 0,
            new_velocity: 100,
            drag: Drag::None,
            centered_for: None,
            dirty: false,
            catch: true,
            link: true,
            pinned: None,
            collapse: false,
            scale_view: ScaleView::Off,
            note_labels: true,
            show_others: true,
            snap_absolute: true,
            clipboard: Vec::new(),
            midi_in: false,
            step: note_edit::StepInput::default(),
            last_step: 0.25,
            chord: None,
            lane: Lane::Velocity,
            show_lane: true,
            tune_lane: false,
            tune_here: false,
            lane_drag: LaneDrag::None,
            lane_sel: None,
            key_anchor: None,
            grid_rect: Rect::NOTHING,
            fixed_vel: 100,
            humanize_ticks: 20,
            humanize_vel: 10,
            humanize_seed: 0x5eed,
        }
    }
}

/// Starts, switches or stops the audition note; `pitch = None` releases.
fn audition(
    out: &mut Vec<Audition>,
    held: &mut Option<u8>,
    pitch: Option<u8>,
    vel: u8,
    track: u16,
) {
    if *held == pitch {
        return;
    }
    if let Some(old) = *held {
        out.push(Audition::Off { pitch: old });
    }
    if let Some(pitch) = pitch {
        out.push(Audition::On { pitch, vel, track });
    }
    *held = pitch;
}

/// The bars that start between `from` and `to` (beats from the region's start `origin`), as
/// (beats from `origin`, 0-based bar number).
fn bar_starts(p: &Project, origin: f64, from: f32, to: f32) -> Vec<(f64, u32)> {
    p.bars_in((origin + from as f64).max(0.0), origin + to as f64)
        .into_iter()
        .map(|b| (b.start - origin, b.bar0))
        .filter(|(rel, _)| *rel >= from as f64 - 1e-9)
        .collect()
}

/// Snap step in beats for the Piano Roll. Smart follows the zoom, like Logic's; when zoomed
/// out it falls back to the signature's beat (an eighth in 6/8).
fn piano_step(snap: Snap, beat_px: f32, beats_per_bar: f64, beat_unit: f64) -> f32 {
    match snap {
        Snap::Smart => match beat_px {
            x if x >= 160.0 => 0.0625,
            x if x >= 80.0 => 0.125,
            x if x >= 24.0 => 0.25,
            _ => beat_unit as f32,
        },
        other => snap_step(other, beat_px, beats_per_bar, beat_unit) as f32,
    }
}

/// Where a moved note starts: the original start moved by whole grid steps (relative snap,
/// Logic's default), or put on the grid (Snap to Absolute Value).
fn snapped_start(orig: f32, delta: f32, step: f32, absolute: bool) -> f32 {
    let s = if absolute {
        snap_to((orig + delta) as f64, step as f64) as f32
    } else {
        orig + snap_to(delta as f64, step as f64) as f32
    };
    s.max(0.0)
}

fn id(key: impl std::hash::Hash + std::fmt::Debug) -> egui::Id {
    egui::Id::unique(("piano_roll", key))
}

fn is_black(pitch: i32) -> bool {
    matches!(pitch.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

/// Logic octave naming: pitch 60 is C3.
fn c_label(pitch: i32) -> String {
    format!("C{}", pitch / 12 - 2)
}

/// The pitch rows of the grid, top to bottom: all 128 pitches, or a list (Collapse Mode, a
/// folded scale), highest first.
struct Rows(Option<Vec<u8>>);

impl Rows {
    fn len(&self) -> usize {
        self.0.as_ref().map_or(128, |l| l.len())
    }

    /// The row of `pitch`; a pitch without a row gets the row of the next lower one shown.
    fn row(&self, pitch: i32) -> usize {
        match &self.0 {
            None => (127 - pitch.clamp(0, 127)) as usize,
            Some(l) => l
                .iter()
                .position(|&p| p as i32 <= pitch)
                .unwrap_or(l.len().saturating_sub(1)),
        }
    }

    fn pitch(&self, row: i32) -> i32 {
        match &self.0 {
            None => (127 - row).clamp(0, 127),
            Some(l) => l[row.clamp(0, l.len() as i32 - 1) as usize] as i32,
        }
    }

    fn shows(&self, pitch: i32) -> bool {
        self.0
            .as_ref()
            .is_none_or(|l| l.iter().any(|p| *p as i32 == pitch))
    }
}

/// The region the editor shows: with Link, the selected MIDI region, else the first one of
/// the selected track; without Link, the region it was left on.
pub(crate) fn target_region(p: &Project, s: &UiState) -> Option<(usize, usize)> {
    let valid = |(t, r): (usize, usize)| {
        p.tracks
            .get(t)
            .is_some_and(|tr| tr.kind != TrackKind::Audio && r < tr.regions.len())
    };
    if !s.piano.link {
        if let Some(pinned) = s.piano.pinned.filter(|x| valid(*x)) {
            return Some(pinned);
        }
    }
    if let Some(sel) = s.tracks.selected.filter(|x| valid(*x)) {
        return Some(sel);
    }
    let t = p.selected_track.min(p.tracks.len().checked_sub(1)?);
    let tr = &p.tracks[t];
    (tr.kind != TrackKind::Audio && !tr.regions.is_empty()).then_some((t, 0))
}

/// A note edit picked in a menu or with a key, run once the grid's input is handled.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Cut,
    Copy,
    Paste,
    Duplicate,
    SelectPitch,
    Transpose(i32),
    /// Edit > Trim > Note End to … (against, gap in beats, remove overlaps only).
    Trim(note_edit::Against, f32, bool),
    EndToPlayhead,
    StartToPlayhead,
    Join,
    /// Mute on/off for the selection (Control-M).
    Mute,
    LengthToGrid,
    FixedVelocity,
    Humanize,
}

struct Actions {
    delete: bool,
    select_all: bool,
    quantize: bool,
    scale: bool,
    ops: Vec<Op>,
    /// Open the MIDI Transform window, with this preset (or as it was left).
    transform: Option<Option<crate::transform::Preset>>,
    /// Whether there is something to undo / redo, and the one picked in the Edit menu.
    can_undo: bool,
    can_redo: bool,
    undo: Option<crate::undo::UndoCmd>,
}

fn shortcut_text(ui: &Ui, m: egui::Modifiers, k: Key) -> String {
    ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(m, k))
}

/// A menu entry; returns whether it was picked (and closes the menu then).
fn item(ui: &mut Ui, enabled: bool, label: &str, keys: &str) -> bool {
    let picked = ui
        .add_enabled(enabled, egui::Button::new(label).shortcut_text(keys))
        .clicked();
    if picked {
        ui.close();
    }
    picked
}

/// A toggle key of the toolbar: latched with its LED lit while on.
fn toggle_button(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    key: &str,
    on: bool,
    draw: impl FnOnce(&Painter, Pos2, Color32),
) -> egui::Response {
    icon_key(ui, g, r, id(("toggle", key)), on, false, draw)
}

/// A tool popup of the toolbar (left-click or Command-click tool): a bone key with the
/// tool's icon and a down triangle.
fn tool_popup(ui: &Ui, g: &Painter, r: Rect, key: &str, tool: &mut Tool, cmd: bool) {
    let (resp, body) = hw::key(
        ui,
        g,
        r,
        id(("tool", key)),
        Cap::Bone,
        false,
        None,
        Legend::Custom,
    );
    draw_icon(
        g,
        pos2(body.left() + 17.0, body.center().y),
        *tool,
        theme::INK,
    );
    hw::tri_down(
        g,
        pos2(body.right() - 11.0, body.center().y + 0.5),
        theme::INK2,
    );
    let tip = if cmd {
        tr("Command-click Tool")
    } else {
        tr("Left-click Tool")
    };
    let resp = resp.on_hover_text(tip);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(200.0);
        for (t, _, letter) in Tool::ALL {
            let label = if cmd {
                t.name().to_string()
            } else {
                format!("{}    {}", t.name(), letter)
            };
            if ui.selectable_label(*tool == t, label).clicked() {
                *tool = t;
                ui.close();
            }
        }
    });
}

pub(crate) fn draw_catch(g: &Painter, c: Pos2, col: Color32) {
    // A playhead line with an arrow chasing it.
    g.line_segment(
        [c + vec2(4.0, -7.0), c + vec2(4.0, 7.0)],
        Stroke::new(1.6, col),
    );
    g.line_segment(
        [c + vec2(-6.0, 0.0), c + vec2(1.0, 0.0)],
        Stroke::new(1.4, col),
    );
    g.add(Shape::convex_polygon(
        vec![
            c + vec2(2.0, 0.0),
            c + vec2(-2.0, -3.5),
            c + vec2(-2.0, 3.5),
        ],
        col,
        Stroke::NONE,
    ));
}

fn draw_link(g: &Painter, c: Pos2, col: Color32) {
    let s = Stroke::new(1.4, col);
    g.rect_stroke(
        Rect::from_center_size(c + vec2(-3.0, 0.0), vec2(9.0, 6.0)),
        3.0,
        s,
        egui::StrokeKind::Inside,
    );
    g.rect_stroke(
        Rect::from_center_size(c + vec2(3.0, 0.0), vec2(9.0, 6.0)),
        3.0,
        s,
        egui::StrokeKind::Inside,
    );
}

/// A MIDI plug (Logic's MIDI In button).
fn draw_midi_in(g: &Painter, c: Pos2, col: Color32) {
    g.circle_stroke(c, 6.0, Stroke::new(1.4, col));
    for (dx, dy) in [
        (-3.0, 0.5),
        (3.0, 0.5),
        (0.0, -3.0),
        (-2.2, -2.2),
        (2.2, -2.2),
    ] {
        g.circle_filled(c + vec2(dx, dy), 0.9, col);
    }
}

fn draw_collapse(g: &Painter, c: Pos2, col: Color32) {
    let s = Stroke::new(1.4, col);
    for dy in [-5.0, -1.5, 2.0] {
        g.line_segment([c + vec2(-7.0, dy), c + vec2(7.0, dy)], s);
    }
    g.add(Shape::convex_polygon(
        vec![c + vec2(-3.0, 4.5), c + vec2(3.0, 4.5), c + vec2(0.0, 7.5)],
        col,
        Stroke::NONE,
    ));
}

#[allow(clippy::too_many_arguments)]
fn toolbar(
    ui: &Ui,
    bar: Rect,
    v: &mut PianoView,
    has_sel: bool,
    can_paste: bool,
    snap: &mut Snap,
    act: &mut Actions,
    editor: (usize, bool),
) -> Option<usize> {
    const NS: &str = "piano_toolbar";
    let g = ui.painter_at(bar);
    menu_row(&g, bar);
    let top = bar.top();
    let cy = top + KEY_Y + KEY_H / 2.0;
    let mut x = bar.left() + 10.0;
    let uid = |k: &str| egui::Id::unique((NS, k.to_string()));
    let cmd = egui::Modifiers::COMMAND;

    let (edit, w) = menu_title(ui, &g, x, cy, tr("Edit"), uid("Edit"));
    x += w + 2.0;
    egui::Popup::menu(&edit).show(|ui| {
        ui.set_min_width(250.0);
        crate::widgets::undo_items(ui, act.can_undo, act.can_redo, &mut act.undo);
        ui.separator();
        for (op, label, key) in [(Op::Cut, tr("Cut"), Key::X), (Op::Copy, tr("Copy"), Key::C)] {
            if item(ui, has_sel, label, &shortcut_text(ui, cmd, key)) {
                act.ops.push(op);
            }
        }
        if item(ui, can_paste, tr("Paste"), &shortcut_text(ui, cmd, Key::V)) {
            act.ops.push(Op::Paste);
        }
        // Logic's Repeat is Command-R; Command-D does the same here.
        if item(ui, has_sel, tr("Repeat"), &shortcut_text(ui, cmd, Key::R)) {
            act.ops.push(Op::Duplicate);
        }
        if item(ui, has_sel, tr("Delete"), tr("Del")) {
            act.delete = true;
        }
        ui.separator();
        if item(ui, true, tr("Select All"), &shortcut_text(ui, cmd, Key::A)) {
            act.select_all = true;
        }
        let sc = shortcut_text(ui, egui::Modifiers::SHIFT, Key::E);
        if item(ui, has_sel, tr("Select Equal Pitches"), &sc) {
            act.ops.push(Op::SelectPitch);
        }
        ui.separator();
        if item(ui, true, tr("Quantize"), "Q") {
            act.quantize = true;
        }
        if item(
            ui,
            has_sel,
            tr("Join Notes"),
            &shortcut_text(ui, cmd, Key::J),
        ) {
            act.ops.push(Op::Join);
        }
        let ctrl = shortcut_text(ui, egui::Modifiers::CTRL, Key::M);
        if item(ui, has_sel, tr("Mute Notes On/Off"), &ctrl) {
            act.ops.push(Op::Mute);
        }
        ui.menu_button(tr("Trim"), |ui| {
            ui.set_min_width(380.0);
            use note_edit::{Against, TRIM_GAP, TRIM_OVERLAP};
            for (label, op) in [
                (tr("Note End to Playhead"), Op::EndToPlayhead),
                (tr("Note Start to Playhead"), Op::StartToPlayhead),
                (
                    tr("Note End to Selected Notes (8 Ticks Gap)"),
                    Op::Trim(Against::Selected, TRIM_GAP, false),
                ),
                (
                    tr("Note End to Selected Notes (4 Ticks Overlap)"),
                    Op::Trim(Against::Selected, TRIM_OVERLAP, false),
                ),
                (
                    tr("Note End to Following Notes (8 Ticks Gap)"),
                    Op::Trim(Against::Following, TRIM_GAP, false),
                ),
                (
                    tr("Note End to Following Notes (4 Ticks Overlap)"),
                    Op::Trim(Against::Following, TRIM_OVERLAP, false),
                ),
                (
                    tr("Note End to Remove Overlaps to Selected Notes (8 Ticks Gap)"),
                    Op::Trim(Against::Selected, TRIM_GAP, true),
                ),
                (
                    tr("Note End to Remove Overlaps to Following Notes (8 Ticks Gap)"),
                    Op::Trim(Against::Following, TRIM_GAP, true),
                ),
                (
                    tr("Note End to Remove Overlaps to Repeated Notes (8 Ticks Gap)"),
                    Op::Trim(Against::Repeated, TRIM_GAP, true),
                ),
            ] {
                if item(ui, has_sel, label, "") {
                    act.ops.push(op);
                }
            }
        });
    });
    let (func, w) = menu_title(ui, &g, x, cy, tr("Functions"), uid("Functions"));
    x += w + 2.0;
    egui::Popup::menu(&func).show(|ui| {
        ui.set_min_width(260.0);
        if item(ui, v.scale_root.is_some(), tr("Scale Quantize"), "") {
            act.scale = true;
        }
        ui.separator();
        let alt = egui::Modifiers::ALT;
        for (label, d, m, k) in [
            (tr("Transpose +1 Semitone"), 1, alt, Key::ArrowUp),
            (tr("Transpose −1 Semitone"), -1, alt, Key::ArrowDown),
            (
                tr("Transpose +1 Octave"),
                12,
                alt | egui::Modifiers::SHIFT,
                Key::ArrowUp,
            ),
            (
                tr("Transpose −1 Octave"),
                -12,
                alt | egui::Modifiers::SHIFT,
                Key::ArrowDown,
            ),
        ] {
            if item(ui, has_sel, label, &shortcut_text(ui, m, k)) {
                act.ops.push(Op::Transpose(d));
            }
        }
        ui.separator();
        ui.label(egui::RichText::new(tr("Note Events")).color(theme::TEXT_DIM));
        if item(ui, true, tr("Set Note Length to Grid"), "") {
            act.ops.push(Op::LengthToGrid);
        }
        ui.horizontal(|ui| {
            if ui.button(tr("Fixed Velocity")).clicked() {
                act.ops.push(Op::FixedVelocity);
                ui.close();
            }
            ui.add(egui::DragValue::new(&mut v.fixed_vel).range(1..=127));
        });
        ui.horizontal(|ui| {
            if ui.button(tr("Humanize")).clicked() {
                act.ops.push(Op::Humanize);
                ui.close();
            }
            ui.label(tr("Time"));
            ui.add(
                egui::DragValue::new(&mut v.humanize_ticks)
                    .range(0..=480)
                    .suffix(format!(" {}", tr("ticks"))),
            );
            ui.label(tr("Velocity"));
            ui.add(egui::DragValue::new(&mut v.humanize_vel).range(0..=64));
        });
        ui.separator();
        use crate::transform::Preset;
        for (label, preset) in [
            (tr("MIDI Transform…"), None),
            (tr("Transpose…"), Some(Preset::Transpose)),
            (tr("Legato"), Some(Preset::Legato)),
            (tr("Humanize…"), Some(Preset::Humanize)),
            (tr("Fixed Velocity…"), Some(Preset::FixedVelocity)),
        ] {
            if ui.button(label).clicked() {
                act.transform = Some(preset);
                ui.close();
            }
        }
    });
    let (view, w) = menu_title(ui, &g, x, cy, tr("View"), uid("View"));
    x += w + 14.0;
    egui::Popup::menu(&view).show(|ui| {
        ui.set_min_width(250.0);
        if ui.button(tr("Zoom In")).clicked() {
            v.beat_px = (v.beat_px * 1.25).min(400.0);
            ui.close();
        }
        if ui.button(tr("Zoom Out")).clicked() {
            v.beat_px = (v.beat_px / 1.25).max(12.0);
            ui.close();
        }
        if ui.button(tr("Zoom to Fit")).clicked() {
            v.fit = true;
            ui.close();
        }
        ui.separator();
        ui.checkbox(&mut v.catch, tr("Catch Playhead"));
        ui.checkbox(&mut v.link, tr("Link to Selected Region"));
        ui.checkbox(&mut v.collapse, tr("Collapse Mode"));
        ui.checkbox(&mut v.note_labels, tr("Note Labels"));
        ui.checkbox(&mut v.show_others, tr("Other Regions of the Track"));
        ui.checkbox(&mut v.show_lane, tr("Automation/MIDI"));
        ui.separator();
        ui.label(egui::RichText::new(tr("Scale")).color(theme::TEXT_DIM));
        let has_scale = v.scale_root.is_some();
        for (mode, label) in [
            (ScaleView::Off, tr("Off")),
            (ScaleView::Highlight, tr("Highlight Scale")),
            (ScaleView::Fold, tr("Fold to Scale")),
        ] {
            let resp = ui.add_enabled(
                has_scale || mode == ScaleView::Off,
                egui::Button::selectable(v.scale_view == mode, label),
            );
            if resp.clicked() {
                v.scale_view = mode;
                ui.close();
            }
        }
        if !has_scale {
            ui.label(
                egui::RichText::new(tr("Pick a root under Scale Quantize")).color(theme::TEXT_DIM),
            );
        }
    });

    // The editor mode keys (Piano Roll / Score), then the tools, the view switches and MIDI
    // In, each group under its bracket.
    let (mode, right) = editor_keys(ui, &g, x, top, editor.0, editor.1);
    x = right + 18.0;
    let kh = KEY_H;
    let ky = top + KEY_Y;
    let lt = Rect::from_min_size(pos2(x, ky), vec2(54.0, kh));
    tool_popup(ui, &g, lt, "left", &mut v.tool, false);
    let ct = Rect::from_min_size(pos2(lt.right() + 6.0, ky), vec2(54.0, kh));
    tool_popup(ui, &g, ct, "cmd", &mut v.cmd_tool, true);
    hw::bracket(&g, lt.left(), ct.right(), top + BRACKET_Y, tr("Edit"));
    x = ct.right() + 18.0;
    let view_left = x;
    for (key, on, draw) in [
        ("catch", v.catch, draw_catch as fn(&Painter, Pos2, Color32)),
        ("link", v.link, draw_link),
        ("collapse", v.collapse, draw_collapse),
    ] {
        let r = Rect::from_min_size(pos2(x, ky), vec2(42.0, kh));
        let resp = toggle_button(ui, &g, r, key, on, draw);
        let tip = match key {
            "catch" => tr("Catch Playhead"),
            "link" => tr("Link to Selected Region"),
            _ => tr("Collapse Mode"),
        };
        if resp.on_hover_text(tip).clicked() {
            match key {
                "catch" => v.catch = !v.catch,
                "link" => v.link = !v.link,
                _ => v.collapse = !v.collapse,
            }
        }
        x += 48.0;
    }
    hw::bracket(&g, view_left, x - 6.0, top + BRACKET_Y, tr("View"));
    // MIDI In (step input): the signal cap while on, like Record.
    let r = Rect::from_min_size(pos2(x + 12.0, ky), vec2(54.0, kh));
    let resp = icon_key(ui, &g, r, uid("midi_in"), v.midi_in, true, draw_midi_in);
    hw::bracket(&g, r.left(), r.right(), top + BRACKET_Y, tr("Input"));
    if resp
        .on_hover_text(tr(
            "MIDI In: step input from a MIDI keyboard or Musical Typing",
        ))
        .clicked()
    {
        v.midi_in = !v.midi_in;
        v.step.reset();
    }
    x = r.right() + 18.0;
    // The chord the selected notes make (Logic shows it in the editor's header), on a small
    // LCD.
    if let Some(chord) = &v.chord {
        let gal = hw::galley(&g, chord, Face::Lcd, 13.0, theme::LCD_INK);
        let chip = Rect::from_min_size(pos2(x, ky), vec2(gal.size().x + 20.0, kh));
        hw::bracket(
            &g,
            chip.left(),
            chip.right(),
            top + BRACKET_Y,
            caption(tr("Chord:")),
        );
        hw::lcd_glass(&g, chip);
        hw::text(
            &g,
            chip.center(),
            Align2::CENTER_CENTER,
            chord,
            Face::Lcd,
            13.0,
            theme::LCD_INK,
        );
        x = chip.right() + 10.0;
    }

    // Right side: Snap, then the zoom sliders (each behind its arrow) under ZOOM.
    let (lo, hi) = (12f32.ln(), 400f32.ln());
    let hz = Rect::from_min_size(pos2(bar.right() - 92.0, cy - 7.0), vec2(80.0, 14.0));
    let t = ((v.beat_px.ln() - lo) / (hi - lo)).clamp(0.0, 1.0);
    if let Some(n) = slider_h(ui, &g, hz, uid("hz"), t, true) {
        v.beat_px = (lo + n * (hi - lo)).exp();
    }
    zoom_arrow(&g, pos2(hz.left() - 14.0, cy), true);
    let vz = Rect::from_min_size(hz.min - vec2(120.0, 0.0), vec2(70.0, 14.0));
    let t = (v.key_h - 6.0) / 18.0;
    if let Some(n) = slider_h(ui, &g, vz, uid("vz"), t, true) {
        v.key_h = 6.0 + n * 18.0;
    }
    zoom_arrow(&g, pos2(vz.left() - 14.0, cy), false);
    hw::bracket(
        &g,
        vz.left() - 24.0,
        hz.right(),
        top + BRACKET_Y,
        tr("Zoom"),
    );
    let snap_r = Rect::from_min_size(pos2(vz.left() - 194.0, cy - 13.0), vec2(150.0, 26.0));
    if snap_r.left() > x + 8.0 {
        let resp = popup_button(ui, &g, snap_r, uid("snap"), snap.label());
        egui::Popup::menu(&resp).show(|ui| {
            for m in Snap::ALL {
                if ui.selectable_label(*snap == m, m.label()).clicked() {
                    *snap = m;
                    ui.close();
                }
            }
            ui.separator();
            ui.checkbox(&mut v.snap_absolute, tr("Snap to Absolute Value"));
        });
        hw::bracket(
            &g,
            snap_r.left(),
            snap_r.right(),
            top + BRACKET_Y,
            caption(tr("Snap:")),
        );
    }
    mode
}

/// The arrow in front of a zoom slider: ↔ for time, ↕ for pitch.
fn zoom_arrow(g: &Painter, c: Pos2, horizontal: bool) {
    let st = Stroke::new(1.6, theme::INK2);
    let (a, b) = if horizontal {
        (c - vec2(7.0, 0.0), c + vec2(7.0, 0.0))
    } else {
        (c - vec2(0.0, 7.0), c + vec2(0.0, 7.0))
    };
    g.line_segment([a, b], st);
    let d = (b - a).normalized();
    let n = vec2(-d.y, d.x);
    for (tip, dir) in [(a, d), (b, -d)] {
        g.add(Shape::convex_polygon(
            vec![tip, tip + dir * 4.0 + n * 3.5, tip + dir * 4.0 - n * 3.5],
            theme::INK2,
            Stroke::NONE,
        ));
    }
}

/// The Q key beside a quantize setting (greyed print while it has nothing to do).
fn small_q(ui: &Ui, g: &Painter, r: Rect, key: &str, enabled: bool) -> bool {
    if !enabled {
        let face = hw::paint_key(g, r, Cap::Bone, false, false);
        hw::text(
            g,
            face.center(),
            Align2::CENTER_CENTER,
            "Q",
            Face::Bold,
            12.0,
            theme::SEAM_DK,
        );
        return false;
    }
    let (resp, _) = hw::key(
        ui,
        g,
        r,
        id(("q", key)),
        Cap::Bone,
        false,
        None,
        Legend::Text("Q"),
    );
    resp.clicked()
}

/// An inspector slider from `x` to `x + w` at `y` (24 tall) with its value on an LCD chip at
/// the right. Returns whether it was dragged (`value` is then the new 0..1 value).
#[allow(clippy::too_many_arguments)]
fn insp_slider(
    ui: &Ui,
    g: &Painter,
    x: f32,
    w: f32,
    y: f32,
    key: &str,
    value: &mut f32,
    shown: &str,
) -> bool {
    let chip = Rect::from_min_size(pos2(x + w - 52.0, y), vec2(52.0, 24.0));
    lcd_value(g, chip, shown);
    let r = Rect::from_min_size(pos2(x + 2.0, y + 5.0), vec2(w - 66.0, 14.0));
    match slider_h(ui, g, r, id(key), *value, true) {
        Some(n) => {
            *value = n;
            true
        }
        None => false,
    }
}

/// [`insp_slider`] behind a silkscreened label, for a 0..1 amount shown in percent.
#[allow(clippy::too_many_arguments)]
fn insp_slider_row(
    ui: &Ui,
    g: &Painter,
    x: f32,
    w: f32,
    y: f32,
    label: &str,
    key: &str,
    value: &mut f32,
) {
    let col = 64.0;
    hw::silk(
        g,
        pos2(x + col - 8.0, y + 12.0),
        Align2::RIGHT_CENTER,
        label,
        11.0,
        theme::INK2,
    );
    let shown = format!("{}", (*value * 100.0).round() as i32);
    insp_slider(ui, g, x + col, w - col, y, key, value, &shown);
}

/// The notes of the track's other regions, placed relative to the region edited: (region,
/// beats from the edited region's start to that region's start).
fn other_regions(p: &Project, ti: usize, ri: usize, origin: f64) -> Vec<(usize, f32)> {
    p.tracks[ti]
        .regions
        .iter()
        .enumerate()
        .filter(|(r, reg)| *r != ri && reg.audio.is_none() && reg.folder.is_none())
        .map(|(r, reg)| (r, (p.region_start_beats(reg) - origin) as f32))
        .collect()
}

/// Keyboard state for the editor, read once per frame.
struct Keys {
    del: bool,
    all: bool,
    q: bool,
    t: bool,
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    shift: bool,
    alt: bool,
    cut: bool,
    copy: bool,
    paste: bool,
    duplicate: bool,
    join: bool,
    mute: bool,
    select_pitch: bool,
    catch: bool,
    /// A: show or hide the Automation/MIDI lane.
    lane: bool,
}

fn read_keys(ui: &Ui) -> Keys {
    ui.input(|i| {
        let ev = |f: fn(&egui::Event) -> bool| i.events.iter().any(f);
        let cmd = i.modifiers.command;
        Keys {
            del: i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
            all: cmd && i.key_pressed(Key::A),
            q: !cmd && i.key_pressed(Key::Q),
            t: !cmd && !i.modifiers.alt && i.key_pressed(Key::T),
            up: i.key_pressed(Key::ArrowUp),
            down: i.key_pressed(Key::ArrowDown),
            left: i.key_pressed(Key::ArrowLeft),
            right: i.key_pressed(Key::ArrowRight),
            shift: i.modifiers.shift,
            alt: i.modifiers.alt,
            // egui turns Command-X/C/V into these events (and may also send the key).
            cut: ev(|e| matches!(e, egui::Event::Cut)) || (cmd && i.key_pressed(Key::X)),
            copy: ev(|e| matches!(e, egui::Event::Copy)) || (cmd && i.key_pressed(Key::C)),
            paste: ev(|e| matches!(e, egui::Event::Paste(_))) || (cmd && i.key_pressed(Key::V)),
            duplicate: cmd && (i.key_pressed(Key::D) || i.key_pressed(Key::R)),
            join: cmd && i.key_pressed(Key::J),
            mute: i.modifiers.ctrl && !cmd && i.key_pressed(Key::M),
            catch: !cmd && i.key_pressed(Key::Backtick),
            lane: !cmd && !i.modifiers.shift && i.key_pressed(Key::A),
            select_pitch: !cmd && i.modifiers.shift && i.key_pressed(Key::E),
        }
    })
}

pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState) {
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, Sense::hover());
    let g = ui.painter_at(area);
    hw::chassis(&g, area);
    let region = target_region(p, s);
    // Without Link the editor stays on the region it shows now.
    s.piano.pinned = region;
    // A Pattern or Drummer region has its own editor.
    let generated = region.and_then(|(t, r)| {
        p.tracks[t].regions[r]
            .generated
            .as_ref()
            .map(|g| matches!(g.source, crate::model::Generator::Drummer(_)))
    });
    // Ordinary MIDI regions can be shown as a score as well.
    let ordinary =
        region.is_some_and(|(t, r)| generated.is_none() && p.tracks[t].regions[r].folder.is_none());
    let active = match generated {
        None if ordinary && s.score_tab => 1,
        None => 0,
        Some(false) => 2,
        Some(true) => 3,
    };
    // The Score, Step Sequencer and Drummer Editor draw their own menu rows, with the editor
    // keys in them.
    if let (Some((ti, ri)), true) = (region, active == 1) {
        s.piano_focus = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|pt| area.contains(pt));
        crate::score::show(ui, p, s, ti, ri, area);
        return;
    }
    if let (Some((ti, ri)), Some(_)) = (region, generated) {
        s.piano_focus = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|pt| area.contains(pt));
        crate::step_editor::show(ui, p, s, ti, ri, area);
        return;
    }
    let bar = Rect::from_min_size(area.min, vec2(area.width(), TOOLBAR_H));
    let body = Rect::from_min_max(pos2(area.left(), bar.bottom()), area.max);
    let hovered = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pt| area.contains(pt));
    s.piano_focus = hovered;

    let has_sel = !s.piano.selected.is_empty();
    let can_paste = !s.piano.clipboard.is_empty();
    let mut act = Actions {
        delete: false,
        select_all: false,
        quantize: false,
        scale: false,
        ops: Vec::new(),
        transform: None,
        can_undo: s.can_undo,
        can_redo: s.can_redo,
        undo: None,
    };
    let mut snap = s.tracks.snap;
    let mode = toolbar(
        ui,
        bar,
        &mut s.piano,
        has_sel,
        can_paste,
        &mut snap,
        &mut act,
        (active, ordinary),
    );
    if let Some(i) = mode {
        s.score_tab = i == 1;
    }
    s.tracks.snap = snap;
    if let Some(preset) = act.transform {
        s.transform.open_with(preset);
    }
    if act.undo.is_some() {
        s.undo_cmd = act.undo;
    }

    let Some((ti, mut ri)) = region else {
        if crate::tune_lane::audio_editor(ui, p, s, body) {
            return;
        }
        empty_well(&g, body, tr("Select a MIDI region to edit its notes"));
        return;
    };
    // A take folder's notes are its comp: they are edited after it is flattened.
    if p.tracks[ti].regions[ri].folder.is_some() {
        empty_well(
            &g,
            body,
            tr("This is a take folder: flatten it to edit its notes"),
        );
        return;
    }
    // Catch with Link: while playing, the editor moves on to the region under the playhead.
    if s.piano.catch && s.piano.link && p.playing {
        let pos = p.position_beats;
        let here = p.tracks[ti].regions.iter().position(|r| {
            r.audio.is_none()
                && r.folder.is_none()
                && pos >= p.region_start_beats(r)
                && pos < p.region_end_beats(r)
        });
        if let Some(r) = here.filter(|r| *r != ri) {
            let shift_beats = (p.region_start_beats(&p.tracks[ti].regions[r])
                - p.region_start_beats(&p.tracks[ti].regions[ri]))
                as f32;
            s.tracks.selected = Some((ti, r));
            s.piano.scroll_x -= shift_beats * s.piano.beat_px;
            s.piano.centered_for = Some((ti, r));
            s.piano.selected.clear();
            s.piano.pinned = Some((ti, r));
            ri = r;
        }
    }
    let origin = p.region_start_beats(&p.tracks[ti].regions[ri]);
    // The bar and beat length where the region starts (the grid draws every bar on its own).
    let bpb = p.bar_len_at(origin);
    let unit = p.beat_unit_at(origin);
    let (region_name, start_bar, len_beats) = {
        let r = &p.tracks[ti].regions[ri];
        (r.name.clone(), r.start_bar, p.region_len_beats(r) as f32)
    };
    let track_name = p.tracks[ti].name.clone();
    let position = (p.position_beats - origin) as f32;
    let others: Vec<(usize, f32)> = if s.piano.show_others {
        other_regions(p, ti, ri, origin)
    } else {
        Vec::new()
    };
    // The span the editor scrolls over: the region, and the other regions shown.
    let (span_lo, span_hi) = others.iter().fold((0f32, len_beats), |(lo, hi), (r, off)| {
        let len = p.region_len_beats(&p.tracks[ti].regions[*r]) as f32;
        (lo.min(*off), hi.max(off + len))
    });

    // Layout.
    let insp = Rect::from_min_size(body.min, vec2(INSPECTOR_W, body.height()));
    let editor = Rect::from_min_max(pos2(insp.right(), body.top()), body.max);
    let lane_h = if s.piano.show_lane { VEL_H + 4.0 } else { 0.0 };
    let lower = editor.bottom() - SB_H - lane_h;
    let keys = Rect::from_min_max(
        pos2(editor.left(), editor.top() + RULER_H + REGION_H),
        pos2(editor.left() + KEYS_W, lower),
    );
    let grid = Rect::from_min_max(
        pos2(keys.right(), keys.top()),
        pos2(editor.right() - SB_H, lower),
    );
    let hbar = Rect::from_min_max(
        pos2(grid.left(), grid.bottom()),
        pos2(grid.right(), grid.bottom() + SB_H),
    );
    let vbar = Rect::from_min_max(
        pos2(grid.right(), grid.top()),
        pos2(editor.right(), grid.bottom()),
    );
    let vel_rect = Rect::from_min_max(
        pos2(grid.left(), hbar.bottom() + 4.0),
        pos2(grid.right(), editor.bottom()),
    );
    let vel_label = Rect::from_min_max(
        pos2(keys.left(), vel_rect.top()),
        pos2(keys.right(), vel_rect.bottom()),
    );
    let ruler = Rect::from_min_max(
        pos2(grid.left(), editor.top()),
        pos2(editor.right(), editor.top() + RULER_H),
    );
    let region_bar = Rect::from_min_max(
        pos2(grid.left(), ruler.bottom()),
        pos2(editor.right(), grid.top()),
    );

    // The pitch rows: all of them, or those in use (Collapse Mode), or the scale's (Fold).
    let rows = {
        let v = &s.piano;
        let scale = v
            .scale_root
            .filter(|_| v.scale_view == ScaleView::Fold)
            .map(|root| (root, MODES[v.scale_mode].1));
        if v.collapse || scale.is_some() {
            let mut used = [false; 128];
            let mut mark = |n: &Note| used[n.pitch.min(127) as usize] = true;
            p.tracks[ti].regions[ri].notes.iter().for_each(&mut mark);
            for (r, _) in &others {
                p.tracks[ti].regions[*r].notes.iter().for_each(&mut mark);
            }
            if let (Some((root, offs)), false) = (scale, v.collapse) {
                for (pitch, u) in used.iter_mut().enumerate() {
                    *u |= note_edit::in_scale(pitch as u8, root, offs);
                }
            }
            let list: Vec<u8> = (0..128u8).rev().filter(|q| used[*q as usize]).collect();
            Rows((!list.is_empty()).then_some(list))
        } else {
            Rows(None)
        }
    };

    let v = &mut s.piano;
    let (down, now) = ui.input(|i| (i.pointer.primary_down(), i.time));
    if v.held.is_some() && !down {
        match v.release_at {
            Some(t) if now < t => ui
                .ctx()
                .request_repaint_after(std::time::Duration::from_millis(30)),
            _ => {
                audition(&mut s.audition, &mut v.held, None, 0, 0);
                v.release_at = None;
            }
        }
    }
    let track_u8 = ti.min(u16::MAX as usize) as u16;
    let notes_len = p.tracks[ti].regions[ri].notes.len();
    v.selected.retain(|i| *i < notes_len);

    // Centre on the notes when a different region is opened.
    if v.centered_for != Some((ti, ri)) {
        v.centered_for = Some((ti, ri));
        v.fit = true;
        let notes = &p.tracks[ti].regions[ri].notes;
        let (lo, hi) = notes
            .iter()
            .fold((127u8, 0u8), |(a, b), n| (a.min(n.pitch), b.max(n.pitch)));
        let mid = if notes.is_empty() {
            60.0
        } else {
            (lo as f32 + hi as f32) / 2.0
        };
        v.scroll_y = rows.row(mid.round() as i32) as f32 * v.key_h - grid.height() / 2.0;
        v.scroll_x = 0.0;
        v.selected.clear();
    }

    if v.fit {
        v.fit = false;
        v.beat_px = ((grid.width() - 16.0) / len_beats.max(1.0)).clamp(12.0, 400.0);
        v.scroll_x = 0.0;
    }

    // Wheel: vertical scroll; Shift = horizontal; Ctrl/Cmd = horizontal zoom.
    if ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pt| editor.contains(pt))
    {
        let (d, m) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers));
        if m.command && d.y != 0.0 {
            v.beat_px = (v.beat_px * (1.0 + d.y * 0.003)).clamp(12.0, 400.0);
        } else if m.shift {
            v.scroll_x -= d.y + d.x;
        } else {
            v.scroll_x -= d.x;
            v.scroll_y -= d.y;
        }
    }
    // Catch: while playing, a playhead leaving the view turns the page to it.
    if v.catch && p.playing && !down {
        let px = position * v.beat_px - v.scroll_x;
        if px < 0.0 || px > grid.width() - 8.0 {
            v.scroll_x = position * v.beat_px - 8.0;
        }
    }
    let content_h = rows.len() as f32 * v.key_h;
    v.scroll_y = v.scroll_y.clamp(0.0, (content_h - grid.height()).max(0.0));
    let min_x = span_lo * v.beat_px;
    let content_w = ((span_hi - span_lo + 2.0) * v.beat_px).max(grid.width());
    {
        let gs = ui.painter_at(Rect::from_min_max(hbar.min, vbar.max));
        let tw = hbar.width() - 4.0;
        let thumb_w = (tw * grid.width() / content_w).clamp(24.0, tw);
        let max_x = (content_w - grid.width()).max(1.0);
        let thumb_x =
            hbar.left() + 2.0 + (tw - thumb_w) * ((v.scroll_x - min_x) / max_x).clamp(0.0, 1.0);
        let thumb = Rect::from_min_size(
            pos2(thumb_x, hbar.top() + 2.0),
            vec2(thumb_w, hbar.height() - 4.0),
        );
        let r = ui.interact(thumb, id("hthumb"), Sense::drag());
        if r.dragged() {
            v.scroll_x += r.drag_delta().x * max_x / (tw - thumb_w).max(1.0);
        }
        paint_scrollbar(&gs, hbar, thumb, r.hovered() || r.dragged());

        let th = vbar.height() - 4.0;
        let thumb_h = (th * grid.height() / content_h).clamp(24.0, th);
        let max_y = (content_h - grid.height()).max(1.0);
        let thumb_y = vbar.top() + 2.0 + (th - thumb_h) * (v.scroll_y / max_y).clamp(0.0, 1.0);
        let thumb = Rect::from_min_size(
            pos2(vbar.left() + 2.0, thumb_y),
            vec2(vbar.width() - 4.0, thumb_h),
        );
        let r = ui.interact(thumb, id("vthumb"), Sense::drag());
        if r.dragged() {
            v.scroll_y += r.drag_delta().y * max_y / (th - thumb_h).max(1.0);
        }
        paint_scrollbar(&gs, vbar, thumb, r.hovered() || r.dragged());
    }
    v.scroll_y = v.scroll_y.clamp(0.0, (content_h - grid.height()).max(0.0));
    v.scroll_x = v
        .scroll_x
        .clamp(min_x, (min_x + content_w - grid.width()).max(min_x));
    let (beat_px, key_h, scroll_x, scroll_y) = (v.beat_px, v.key_h, v.scroll_x, v.scroll_y);
    v.grid_rect = grid;
    let x_of = |beat: f32| grid.left() + beat * beat_px - scroll_x;
    let y_of = |pitch: i32| grid.top() + rows.row(pitch) as f32 * key_h - scroll_y;
    let beat_at = |x: f32| (x - grid.left() + scroll_x) / beat_px;
    let pitch_at = |y: f32| rows.pitch(((y - grid.top() + scroll_y) / key_h).floor() as i32);
    let step = piano_step(s.tracks.snap, beat_px, bpb, unit);
    v.last_step = step;
    let bars = bar_starts(
        p,
        origin,
        scroll_x / beat_px,
        (scroll_x + grid.width()) / beat_px,
    );
    let scale = v
        .scale_root
        .filter(|_| v.scale_view != ScaleView::Off)
        .map(|root| (root, MODES[v.scale_mode].1));

    // ---- Grid background: the key rows on the recess, ink lines for bars and beats.
    let gg = ui.painter_at(grid);
    gg.rect_filled(grid, 0.0, ROW_WHITE);
    let top_row = ((scroll_y / key_h).floor() as i32).max(0);
    let bottom_row =
        (((scroll_y + grid.height()) / key_h).ceil() as i32).min(rows.len() as i32 - 1);
    for row in top_row..=bottom_row {
        let pitch = rows.pitch(row);
        let y = grid.top() + row as f32 * key_h - scroll_y;
        let r = Rect::from_min_size(pos2(grid.left(), y), vec2(grid.width(), key_h));
        // With the scale shown, rows in it are bone and the others the deeper lane tone; the
        // root's rows carry an orange mark at the left edge. Otherwise the black keys' rows
        // are a shade deeper.
        let fill = match scale {
            Some((root, offs)) if !note_edit::in_scale(pitch as u8, root, offs) => row_out(),
            Some(_) => ROW_WHITE,
            None if is_black(pitch) => row_black(),
            None => ROW_WHITE,
        };
        gg.rect_filled(r, 0.0, fill);
        if scale.is_some_and(|(root, _)| pitch.rem_euclid(12) == root as i32) {
            gg.rect_filled(
                Rect::from_min_size(r.min, vec2(3.0, key_h)),
                0.0,
                theme::ORANGE.gamma_multiply(0.6),
            );
        }
        if pitch % 12 == 0 || rows.0.is_some() {
            gg.hline(
                grid.x_range(),
                r.bottom() - 0.5,
                Stroke::new(1.0, ink(if pitch % 12 == 0 { 0.2 } else { 0.08 })),
            );
        }
    }
    let first = (scroll_x / beat_px).floor() as i32;
    let last = ((scroll_x + grid.width()) / beat_px).ceil() as i32;
    let sub = (step < 1.0 && step * beat_px >= 8.0).then_some(step);
    for b in first..=last {
        let bx = x_of(b as f32);
        gg.vline(bx, grid.y_range(), Stroke::new(1.0, ink(0.09)));
        if let Some(st) = sub {
            let mut k = st;
            while k < 1.0 {
                gg.vline(
                    x_of(b as f32 + k),
                    grid.y_range(),
                    Stroke::new(1.0, ink(0.045)),
                );
                k += st;
            }
        }
    }
    for &(rel, _) in &bars {
        gg.vline(
            x_of(rel as f32),
            grid.y_range(),
            Stroke::new(1.0, ink(0.26)),
        );
    }
    // Outside the region: a darker floor with a fine hatch.
    let (rx0, rx1) = (x_of(0.0), x_of(len_beats));
    if rx0 > grid.left() {
        outside(
            &gg,
            Rect::from_min_max(grid.min, pos2(rx0.min(grid.right()), grid.bottom())),
        );
    }
    if rx1 < grid.right() {
        outside(
            &gg,
            Rect::from_min_max(pos2(rx1.max(grid.left()), grid.top()), grid.max),
        );
    }

    // ---- Piano keys: press to play (and select that pitch's notes), drag to glissando
    let kresp = ui.interact(keys, id("keys"), Sense::click_and_drag());
    if kresp.is_pointer_button_down_on() {
        if let Some(pt) = kresp.interact_pointer_pos() {
            audition(
                &mut s.audition,
                &mut v.held,
                Some(pitch_at(pt.y) as u8),
                100,
                track_u8,
            );
            v.release_at = None;
        }
    }
    // A key selects all the notes of its pitch; Shift-click or a drag over the keys selects a
    // range of pitches (Logic: Select notes).
    let notes = &p.tracks[ti].regions[ri].notes;
    if kresp.clicked() || kresp.drag_started() {
        if let Some(pt) = ui
            .input(|i| i.pointer.press_origin())
            .or(kresp.interact_pointer_pos())
        {
            let pitch = pitch_at(pt.y) as u8;
            v.selected = match v.key_anchor.filter(|_| ui.input(|i| i.modifiers.shift)) {
                Some(a) => note_edit::notes_in_pitches(notes, a, pitch),
                None => {
                    v.key_anchor = Some(pitch);
                    note_edit::notes_of_pitch(notes, pitch)
                }
            };
        }
    }
    if kresp.dragged() {
        if let (Some(a), Some(pt)) = (v.key_anchor, kresp.interact_pointer_pos()) {
            v.selected = note_edit::notes_in_pitches(notes, a, pitch_at(pt.y) as u8);
        }
    }

    // ---- Tool menu (T)
    let menu_open = v.tool_menu.is_some();
    if let Some(at) = v.tool_menu {
        let (picked, open) = crate::piano_tools::tool_menu(ui.ctx(), at, v.tool);
        if let Some(t) = picked {
            v.tool = t;
        }
        if !open {
            v.tool_menu = None;
        }
    }

    // ---- Interaction
    let resp = ui.interact(grid, id("grid"), Sense::click_and_drag());
    let (shift, alt, command, ctrl) = ui.input(|i| {
        let m = i.modifiers;
        (m.shift, m.alt, m.command, m.ctrl && !m.command)
    });
    let tool = if command { v.cmd_tool } else { v.tool };
    // Pro Wave Tune's Pitch Correction lane: the Pencil and Eraser draw on the correction,
    // not on the notes.
    let tune_link = crate::tune_lane::link_for(p, ti);
    v.tune_here = tune_link.is_some();
    let tune_on = v.tune_lane && v.show_lane && tune_link.is_some();
    let y_mid = |m: f32| {
        let k = m.floor() as i32;
        let a = y_of(k) + key_h * 0.5;
        let b = y_of(k + 1) + key_h * 0.5;
        a + (b - a) * (m - k as f32)
    };
    let pitch_mid = |y: f32| {
        let k = pitch_at(y);
        k as f32 + (y_of(k) + key_h * 0.5 - y) / key_h
    };
    let x_abs = |b: f64| x_of((b - origin) as f32);
    let beat_abs = |x: f32| origin + f64::from(beat_at(x));
    let tune_geom = crate::tune_lane::Geom {
        grid,
        x_of: &x_abs,
        beat_at: &beat_abs,
        y_of: &y_mid,
        pitch_at: &pitch_mid,
    };
    let resp = match tune_link {
        Some(link) if tune_on && matches!(tool, Tool::Pencil | Tool::Eraser) => {
            crate::tune_lane::edit_grid(
                ui,
                p,
                &mut s.tune,
                link,
                &resp,
                &tune_geom,
                tool == Tool::Eraser,
            );
            ui.interact(Rect::NOTHING, id("grid_tune"), Sense::hover())
        }
        _ => resp,
    };
    let v = &mut s.piano;
    // Control while editing snaps to the division, Control-Shift to ticks (Logic: Snap items
    // to the grid, <https://support.apple.com/guide/logicpro/snap-items-to-the-grid-lgcpa9051d7a/mac>).
    let step = match (ctrl, shift) {
        (true, true) => note_edit::TICK,
        (true, false) => 0.25,
        _ => step,
    };
    // The Brush paints at the Time Quantize value.
    let brush_step = GRIDS[v.grid].1;
    let edge_beats = 10.0 / beat_px;
    // Right-click deletes the note under the pointer.
    let (secondary, hover, down_now) = ui.input(|i| {
        (
            i.pointer.button_pressed(egui::PointerButton::Secondary),
            i.pointer.hover_pos(),
            i.pointer.primary_down(),
        )
    });
    // A click (or the start of a drag) at this point, for the tools that act on a click.
    let mut tap: Option<Pos2> = None;
    // A click on a note of another region shown (and on none of this one's): that region,
    // its offset and the note.
    let open_other = resp
        .clicked()
        .then(|| resp.interact_pointer_pos())
        .flatten()
        .filter(|_| matches!(tool, Tool::Pointer | Tool::Pencil))
        .and_then(|pt| {
            let (beat, pitch) = (beat_at(pt.x), pitch_at(pt.y) as u8);
            let notes = &p.tracks[ti].regions[ri].notes;
            if note_edit::hit_note(notes, beat, pitch, 0.0).is_some() {
                return None;
            }
            others.iter().find_map(|&(r, off)| {
                note_edit::hit_note(&p.tracks[ti].regions[r].notes, beat - off, pitch, 0.0)
                    .map(|(i, _)| (r, off, i))
            })
        });
    {
        let notes = &mut p.tracks[ti].regions[ri].notes;
        if let Some(pt) = hover.filter(|pt| grid.contains(*pt) && resp.hovered()) {
            let (beat, pitch) = (beat_at(pt.x), pitch_at(pt.y));
            let over = note_edit::hit_part(notes, beat, pitch as u8, edge_beats);
            let edge = over.is_some_and(|(_, part)| part != Part::Body)
                && matches!(tool, Tool::Pointer | Tool::Pencil);
            if edge
                || tool == Tool::Finger
                || matches!(v.drag, Drag::Resize { .. } | Drag::ResizeLeft { .. })
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            } else if !matches!(tool, Tool::Pointer) {
                ui.ctx().set_cursor_icon(tool.cursor());
            }
            if secondary {
                if let Some((i, _)) = over {
                    notes.remove(i);
                    v.selected.clear();
                    v.dirty = true;
                }
            }
        }
        if let Drag::Create { index } = v.drag {
            if down_now {
                if let (Some(n), Some(pt)) = (notes.get_mut(index), hover) {
                    let len = snap_to((beat_at(pt.x) - n.start) as f64, step as f64) as f32;
                    n.len = len.max(step.max(note_edit::MIN_LEN));
                }
            } else {
                v.drag = Drag::None;
                v.dirty = true;
            }
        }
        if resp.drag_started() && !matches!(v.drag, Drag::Create { .. }) {
            let origin_pt = ui.input(|i| i.pointer.press_origin());
            if let Some(pt) = origin_pt.or(resp.interact_pointer_pos()) {
                let (beat, pitch) = (beat_at(pt.x), pitch_at(pt.y));
                let hit = note_edit::hit_part(notes, beat, pitch as u8, edge_beats);
                match tool {
                    Tool::Pointer | Tool::Pencil | Tool::Finger => match hit {
                        Some((i, part)) => {
                            // A muted note is not heard.
                            audition(
                                &mut s.audition,
                                &mut v.held,
                                (!notes[i].mute).then_some(notes[i].pitch),
                                notes[i].vel,
                                track_u8,
                            );
                            v.release_at = None;
                            if !v.selected.contains(&i) {
                                if shift {
                                    v.selected.push(i);
                                } else {
                                    v.selected = vec![i];
                                }
                            }
                            let part = if tool == Tool::Finger {
                                Part::Right
                            } else {
                                part
                            };
                            v.drag = match part {
                                Part::Right => Drag::Resize {
                                    anchor_beat: beat,
                                    orig: v.selected.iter().map(|&k| (k, notes[k].len)).collect(),
                                },
                                Part::Left => Drag::ResizeLeft {
                                    anchor_beat: beat,
                                    orig: v
                                        .selected
                                        .iter()
                                        .map(|&k| (k, notes[k].start, notes[k].len))
                                        .collect(),
                                },
                                Part::Body => {
                                    // Option-drag moves copies and leaves the originals.
                                    if alt {
                                        v.selected = note_edit::paste(
                                            notes,
                                            &note_edit::copy(notes, &v.selected),
                                            v.selected
                                                .iter()
                                                .map(|&k| notes[k].start)
                                                .fold(f32::MAX, f32::min),
                                        );
                                    }
                                    Drag::Move {
                                        anchor_beat: beat,
                                        anchor_pitch: pitch,
                                        orig: v
                                            .selected
                                            .iter()
                                            .map(|&k| (k, notes[k].start, notes[k].pitch))
                                            .collect(),
                                        copies: alt,
                                    }
                                }
                            };
                        }
                        None if tool == Tool::Pencil && beat >= 0.0 => {
                            let start = (beat / step).floor() * step;
                            notes.push(Note {
                                lyric: crate::lyrics::Lyric::NONE,
                                mute: false,
                                start,
                                len: step.max(note_edit::MIN_LEN),
                                pitch: pitch as u8,
                                vel: v.new_velocity,
                            });
                            let index = notes.len() - 1;
                            v.selected = vec![index];
                            v.drag = Drag::Create { index };
                            audition(
                                &mut s.audition,
                                &mut v.held,
                                Some(pitch as u8),
                                v.new_velocity,
                                track_u8,
                            );
                            v.release_at = None;
                            v.dirty = true;
                        }
                        None => {
                            if !shift {
                                v.selected.clear();
                            }
                            v.drag = Drag::Marquee { from: pt };
                        }
                    },
                    Tool::Velocity => {
                        if let Some((i, _)) = hit {
                            if !v.selected.contains(&i) {
                                v.selected = vec![i];
                            }
                            // The note grabbed first: Option-Shift sets the others to it.
                            let mut orig = vec![(i, notes[i].vel)];
                            orig.extend(
                                v.selected
                                    .iter()
                                    .filter(|k| **k != i)
                                    .map(|&k| (k, notes[k].vel)),
                            );
                            v.drag = Drag::Velocity {
                                anchor_y: pt.y,
                                orig,
                                equal: alt && shift,
                            };
                        }
                    }
                    Tool::Eraser => v.drag = Drag::Erase,
                    Tool::Brush => {
                        let painted = note_edit::brush(
                            notes,
                            pitch as u8,
                            beat,
                            beat,
                            brush_step,
                            v.new_velocity,
                        );
                        notes.extend(painted);
                        v.drag = Drag::Brush {
                            last: beat,
                            pitch: pitch as u8,
                        };
                        audition(
                            &mut s.audition,
                            &mut v.held,
                            Some(pitch as u8),
                            v.new_velocity,
                            track_u8,
                        );
                        v.release_at = None;
                        v.dirty = true;
                    }
                    // Dragging the Quantize tool quantizes every note it crosses.
                    Tool::Quantize => {
                        v.drag = Drag::Quantize { done: Vec::new() };
                        tap = Some(pt);
                    }
                    Tool::Scissors | Tool::Glue | Tool::Mute | Tool::Zoom => tap = Some(pt),
                }
            }
        }
        if resp.dragged() {
            if let Some(pt) = resp.interact_pointer_pos() {
                let (beat, pitch) = (beat_at(pt.x), pitch_at(pt.y));
                match &mut v.drag {
                    Drag::Move {
                        anchor_beat,
                        anchor_pitch,
                        orig,
                        ..
                    } => {
                        // In Collapse Mode a move steps through the rows shown.
                        let dr = rows.row(pitch) as i32 - rows.row(*anchor_pitch) as i32;
                        for &(i, start, pc) in orig.iter() {
                            notes[i].start =
                                snapped_start(start, beat - *anchor_beat, step, v.snap_absolute);
                            notes[i].pitch = if rows.0.is_some() {
                                rows.pitch(rows.row(pc as i32) as i32 + dr) as u8
                            } else {
                                (pc as i32 + pitch - *anchor_pitch).clamp(0, 127) as u8
                            };
                        }
                        let now_pitch = orig.first().map(|o| notes[o.0].pitch);
                        audition(&mut s.audition, &mut v.held, now_pitch, 100, track_u8);
                    }
                    Drag::Resize { anchor_beat, orig } => {
                        let delta = snap_to((beat - *anchor_beat) as f64, step as f64) as f32;
                        for &(i, len) in orig.iter() {
                            notes[i].len = (len + delta).max(note_edit::MIN_LEN);
                        }
                    }
                    Drag::ResizeLeft { anchor_beat, orig } => {
                        let delta = snap_to((beat - *anchor_beat) as f64, step as f64) as f32;
                        note_edit::resize_left(notes, orig, delta);
                    }
                    Drag::Marquee { from } => {
                        let (b0, p0) = (beat_at(from.x), pitch_at(from.y));
                        v.selected = note_edit::marquee(notes, b0, beat, p0 as u8, pitch as u8);
                    }
                    Drag::Create { index } => {
                        if let Some(n) = notes.get_mut(*index) {
                            n.len = (snap_to((beat - n.start) as f64, step as f64) as f32)
                                .max(step.max(note_edit::MIN_LEN));
                        }
                    }
                    Drag::Velocity {
                        anchor_y,
                        orig,
                        equal,
                    } => {
                        let dv = ((*anchor_y - pt.y) * 0.6).round() as i32;
                        let first = orig.first().map_or(64, |o| o.1 as i32);
                        for &(i, vel) in orig.iter() {
                            if let Some(n) = notes.get_mut(i) {
                                let base = if *equal { first } else { vel as i32 };
                                n.vel = (base + dv).clamp(1, 127) as u8;
                            }
                        }
                        v.dirty = true;
                    }
                    Drag::Quantize { done } => {
                        if let Some((i, _)) = note_edit::hit_note(notes, beat, pitch as u8, 0.0) {
                            if !done.contains(&i) {
                                done.push(i);
                                note_edit::quantize(
                                    notes,
                                    &[i],
                                    GRIDS[v.grid].1,
                                    v.strength,
                                    v.swing,
                                );
                                v.dirty = true;
                            }
                        }
                    }
                    Drag::Erase => {
                        if let Some((i, _)) = note_edit::hit_part(notes, beat, pitch as u8, 0.0) {
                            notes.remove(i);
                            v.selected.clear();
                            v.dirty = true;
                        }
                    }
                    Drag::Brush { last, pitch: first } => {
                        // Shift keeps the Brush on the pitch it started on.
                        let pitch = if shift { *first as i32 } else { pitch };
                        let painted = note_edit::brush(
                            notes,
                            pitch as u8,
                            *last,
                            beat,
                            brush_step,
                            v.new_velocity,
                        );
                        if !painted.is_empty() {
                            notes.extend(painted);
                            v.dirty = true;
                        }
                        *last = beat;
                        audition(
                            &mut s.audition,
                            &mut v.held,
                            Some(pitch as u8),
                            v.new_velocity,
                            track_u8,
                        );
                    }
                    Drag::None => {}
                }
            }
        }
        if resp.drag_stopped() {
            match &v.drag {
                // An Option-drag that went nowhere leaves no copies on top of the notes.
                Drag::Move {
                    orig, copies: true, ..
                } if orig
                    .iter()
                    .all(|&(i, start, pc)| notes[i].start == start && notes[i].pitch == pc) =>
                {
                    let made: Vec<usize> = orig.iter().map(|o| o.0).collect();
                    note_edit::delete(notes, &made);
                    v.selected.clear();
                }
                Drag::Move { .. }
                | Drag::Resize { .. }
                | Drag::ResizeLeft { .. }
                | Drag::Create { .. }
                | Drag::Velocity { .. }
                | Drag::Quantize { .. }
                | Drag::Brush { .. }
                | Drag::Erase => v.dirty = true,
                Drag::Marquee { .. } | Drag::None => {}
            }
            v.drag = Drag::None;
        }
        if resp.clicked() {
            if let Some(pt) = resp.interact_pointer_pos() {
                let (beat, pitch) = (beat_at(pt.x), pitch_at(pt.y));
                let hit = note_edit::hit_note(notes, beat, pitch as u8, 0.0);
                match tool {
                    Tool::Pointer | Tool::Pencil | Tool::Finger | Tool::Velocity | Tool::Brush => {
                        match hit {
                            Some((i, _)) if shift => {
                                if let Some(k) = v.selected.iter().position(|x| *x == i) {
                                    v.selected.remove(k);
                                } else {
                                    v.selected.push(i);
                                }
                            }
                            Some((i, _)) => {
                                v.selected = vec![i];
                                audition(
                                    &mut s.audition,
                                    &mut v.held,
                                    (!notes[i].mute).then_some(notes[i].pitch),
                                    notes[i].vel,
                                    track_u8,
                                );
                                v.release_at = Some(now + 0.25);
                            }
                            // A click (press and release without dragging) on empty space
                            // creates a note.
                            None if !shift
                                && beat >= 0.0
                                && open_other.is_none()
                                && matches!(tool, Tool::Pointer | Tool::Pencil | Tool::Brush) =>
                            {
                                let step = if tool == Tool::Brush {
                                    brush_step
                                } else {
                                    step
                                };
                                let start = (beat / step).floor() * step;
                                notes.push(Note {
                                    lyric: crate::lyrics::Lyric::NONE,
                                    mute: false,
                                    start,
                                    len: step.max(note_edit::MIN_LEN),
                                    pitch: pitch as u8,
                                    vel: v.new_velocity,
                                });
                                v.selected = vec![notes.len() - 1];
                                audition(
                                    &mut s.audition,
                                    &mut v.held,
                                    Some(pitch as u8),
                                    v.new_velocity,
                                    track_u8,
                                );
                                v.release_at = Some(now + 0.25);
                                v.dirty = true;
                            }
                            None => {}
                        }
                    }
                    Tool::Eraser => {
                        if let Some((i, _)) = hit {
                            let sel = if v.selected.contains(&i) {
                                std::mem::take(&mut v.selected)
                            } else {
                                vec![i]
                            };
                            note_edit::delete(notes, &sel);
                            v.selected.clear();
                            v.dirty = true;
                        }
                    }
                    Tool::Scissors | Tool::Glue | Tool::Mute | Tool::Quantize | Tool::Zoom => {
                        tap = Some(pt)
                    }
                }
            }
        }
        // The click tools.
        if let Some(pt) = tap {
            let (beat, pitch) = (beat_at(pt.x), pitch_at(pt.y));
            let hit = note_edit::hit_note(notes, beat, pitch as u8, 0.0).map(|h| h.0);
            // The note clicked, or the whole selection when it is part of it.
            let targets = |sel: &[usize], i: usize| {
                if sel.contains(&i) {
                    sel.to_vec()
                } else {
                    vec![i]
                }
            };
            match (tool, hit) {
                (Tool::Scissors, Some(i)) => {
                    let at = snap_to(beat as f64, step as f64) as f32;
                    let mut sel = Vec::new();
                    for k in targets(&v.selected, i) {
                        let at = if notes[k].start < at && notes[k].start + notes[k].len > at {
                            at
                        } else {
                            beat
                        };
                        sel.push(k);
                        if let Some(new) = note_edit::split(notes, k, at) {
                            sel.push(new);
                        }
                    }
                    v.selected = sel;
                    v.dirty = true;
                }
                (Tool::Glue, Some(i)) if v.selected.contains(&i) && v.selected.len() > 1 => {
                    v.selected = note_edit::glue(notes, &v.selected);
                    v.dirty = true;
                }
                (Tool::Mute, Some(i)) => {
                    note_edit::toggle_mute(notes, &targets(&v.selected, i));
                    v.dirty = true;
                }
                (Tool::Quantize, Some(i)) => {
                    let sel = targets(&v.selected, i);
                    note_edit::quantize(notes, &sel, GRIDS[v.grid].1, v.strength, v.swing);
                    v.dirty = true;
                }
                (Tool::Zoom, _) => {
                    let f = if alt { 0.5 } else { 2.0 };
                    let new_px = (beat_px * f).clamp(12.0, 400.0);
                    // Keep the beat under the pointer where it is.
                    v.beat_px = new_px;
                    v.scroll_x = beat * new_px - (pt.x - grid.left());
                }
                _ => {}
            }
        }
    }
    // ---- Controller lanes (MIDI Draw): drawn and edited with the tools.
    if v.show_lane && v.lane != Lane::Velocity && !tune_on {
        let view = midi_draw::LaneView {
            rect: vel_rect,
            x_of: &x_of,
            beat_at: &beat_at,
            tool,
        };
        let controls = &mut p.tracks[ti].regions[ri].controls;
        if midi_draw::edit(
            ui,
            &view,
            v.lane,
            controls,
            &mut v.lane_drag,
            &mut v.lane_sel,
        ) {
            v.dirty = true;
        }
    }
    // ---- Velocity lane (FL Studio style): drag a stem up or down to set that note's velocity.
    let notes = &mut p.tracks[ti].regions[ri].notes;
    let vresp_on = v.show_lane && v.lane == Lane::Velocity && !tune_on;
    // Only the lane shown takes the pointer.
    let vresp = ui.interact(
        if vresp_on { vel_rect } else { Rect::NOTHING },
        id("vel_lane"),
        Sense::click_and_drag(),
    );
    if vresp_on && (vresp.drag_started() || vresp.clicked()) {
        let origin = ui.input(|i| i.pointer.press_origin());
        if let Some(pt) = origin.or(vresp.interact_pointer_pos()) {
            let near = |n: &Note| (x_of(n.start) - pt.x).abs();
            let best = notes
                .iter()
                .enumerate()
                .filter(|(_, n)| near(n) <= 7.0)
                .min_by(|a, b| near(a.1).total_cmp(&near(b.1)))
                .map(|(i, _)| i);
            v.vel_drag = best;
            if let Some(i) = best {
                if !v.selected.contains(&i) {
                    v.selected = vec![i];
                }
            }
        }
    }
    if let Some(i) = v.vel_drag {
        match (down_now, hover) {
            (true, Some(pt)) => {
                let nv = (((vel_rect.bottom() - pt.y) / vel_rect.height()) * 127.0)
                    .round()
                    .clamp(1.0, 127.0) as i32;
                if let Some(cur) = notes.get(i).map(|n| n.vel as i32) {
                    let delta = nv - cur;
                    let targets = if v.selected.contains(&i) {
                        v.selected.clone()
                    } else {
                        vec![i]
                    };
                    for k in targets {
                        if let Some(n) = notes.get_mut(k) {
                            n.vel = (n.vel as i32 + delta).clamp(1, 127) as u8;
                        }
                    }
                    v.dirty = true;
                }
            }
            (true, None) => {}
            (false, _) => v.vel_drag = None,
        }
    }

    // ---- Keyboard (only while the pointer is over the editor, and not while the Tool menu
    // takes the keys)
    if hovered && !menu_open && !ui.ctx().egui_wants_keyboard_input() {
        let k = read_keys(ui);
        if k.t {
            v.tool_menu = Some(hover.unwrap_or(grid.center()));
        }
        if k.catch {
            v.catch = !v.catch;
        }
        if k.lane {
            v.show_lane = !v.show_lane;
        }
        // Delete in a controller lane removes its point, not the notes.
        let in_lane = hover.is_some_and(|pt| vel_rect.contains(pt)) && v.lane != Lane::Velocity;
        act.delete |= k.del && !in_lane;
        act.select_all |= k.all;
        act.quantize |= k.q;
        for (on, op) in [
            (k.cut, Op::Cut),
            (k.copy, Op::Copy),
            (k.paste, Op::Paste),
            (k.duplicate, Op::Duplicate),
            (k.join, Op::Join),
            (k.mute, Op::Mute),
            (k.select_pitch, Op::SelectPitch),
        ] {
            if on {
                act.ops.push(op);
            }
        }
        // Arrows: Up/Down transpose (Shift: an octave), Left/Right move by the grid (Option:
        // by one tick).
        let dp = (k.up as i32 - k.down as i32) * if k.shift { 12 } else { 1 };
        let dt =
            (k.right as i32 - k.left as i32) as f32 * if k.alt { note_edit::TICK } else { step };
        if (dp != 0 || dt != 0.0) && !v.selected.is_empty() {
            note_edit::shift(notes, &v.selected, dt, dp);
            v.dirty = true;
        }
    }
    if act.select_all {
        v.selected = (0..notes.len()).collect();
    }

    // ---- Inspector (left): quantize, scale quantize, velocity, on a faceplate with a seam
    // between it and the editor.
    let gi = ui.painter_at(insp);
    gi.rect_filled(insp, 0.0, theme::BONE);
    let plate = Rect::from_min_max(insp.min + vec2(6.0, 6.0), insp.max - vec2(9.0, 6.0));
    hw::faceplate(&gi, plate, 4.0, false);
    for c in [
        plate.left_top() + vec2(8.0, 8.0),
        plate.right_top() + vec2(-8.0, 8.0),
        plate.left_bottom() + vec2(8.0, -8.0),
        plate.right_bottom() + vec2(-8.0, -8.0),
    ] {
        hw::screw(&gi, c, 3.5, 35.0);
    }
    hw::seam_v(&gi, insp.right() - 2.0, insp.y_range());
    let ix = plate.left() + 12.0;
    let iw = plate.width() - 24.0;
    let mut y = plate.top() + 26.0;
    let n_sel = v.selected.len();
    // Region header, two-tone as in the main inspector: "▼ REGION" then the name in print.
    hw::tri_down(&gi, pos2(ix + 4.0, y + 1.0), theme::INK);
    let head = hw::text(
        &gi,
        pos2(ix + 14.0, y),
        Align2::LEFT_CENTER,
        &caption(tr("Region:")).to_uppercase(),
        Face::Silk,
        14.0,
        theme::INK,
    );
    let room = ix + iw - head.right() - 14.0;
    let name = hw::fit(&gi, &region_name.to_uppercase(), Face::Silk, 14.0, room);
    hw::text(
        &gi,
        pos2(head.right() + 10.0, y),
        Align2::LEFT_CENTER,
        &name,
        Face::Silk,
        14.0,
        theme::PRINT,
    );
    // The editor's mouse and key commands, on hover over the header.
    let head_r = Rect::from_min_max(pos2(ix, y - 10.0), pos2(ix + iw, y + 10.0));
    ui.interact(head_r, id("insp_head"), Sense::hover())
        .on_hover_text(format!(
            "{}\n{}",
            trf("on Track {}", &[&track_name]),
            tr("T: tools · Click: add · Drag: select · Del: delete")
        ));
    y += 16.0;
    hw::seam_h(&gi, plate.left() + 1.0..=plate.right() - 1.0, y);
    y += 22.0;

    hw::bracket(&gi, ix, ix + iw, y, tr("Time Quantize (classic)"));
    y += 16.0;
    let grid_r = Rect::from_min_size(pos2(ix, y), vec2(iw - 44.0, 24.0));
    let gresp = popup_button(ui, &gi, grid_r, id("grid_popup"), tr(GRIDS[v.grid].0));
    egui::Popup::menu(&gresp).show(|ui| {
        for (i, (label, _)) in GRIDS.iter().enumerate() {
            if ui.selectable_label(v.grid == i, tr(label)).clicked() {
                v.grid = i;
                ui.close();
            }
        }
    });
    if small_q(
        ui,
        &gi,
        Rect::from_min_size(pos2(ix + iw - 36.0, y - 1.0), vec2(36.0, 25.0)),
        "time",
        true,
    ) {
        act.quantize = true;
    }
    y += 38.0;
    for (label, key, value) in [
        (tr("Strength"), "strength", &mut v.strength),
        (tr("Swing"), "swing", &mut v.swing),
    ] {
        insp_slider_row(ui, &gi, ix, iw, y, label, key, value);
        y += 34.0;
    }
    y += 10.0;

    hw::bracket(&gi, ix, ix + iw, y, tr("Scale Quantize"));
    y += 16.0;
    let root_r = Rect::from_min_size(pos2(ix, y), vec2(70.0, 24.0));
    let root_text = v.scale_root.map_or(tr("Off"), |r| ROOTS[r as usize]);
    let rresp = popup_button(ui, &gi, root_r, id("root_popup"), root_text);
    egui::Popup::menu(&rresp).show(|ui| {
        if ui
            .selectable_label(v.scale_root.is_none(), tr("Off"))
            .clicked()
        {
            v.scale_root = None;
            ui.close();
        }
        for (i, r) in ROOTS.iter().enumerate() {
            if ui
                .selectable_label(v.scale_root == Some(i as u8), *r)
                .clicked()
            {
                v.scale_root = Some(i as u8);
                ui.close();
            }
        }
    });
    let mode_r = Rect::from_min_size(pos2(ix + 76.0, y), vec2(iw - 76.0 - 44.0, 24.0));
    let mresp = popup_button(ui, &gi, mode_r, id("mode_popup"), tr(MODES[v.scale_mode].0));
    egui::Popup::menu(&mresp).show(|ui| {
        for (i, (label, _)) in MODES.iter().enumerate() {
            if ui.selectable_label(v.scale_mode == i, tr(label)).clicked() {
                v.scale_mode = i;
                ui.close();
            }
        }
    });
    if small_q(
        ui,
        &gi,
        Rect::from_min_size(pos2(ix + iw - 36.0, y - 1.0), vec2(36.0, 25.0)),
        "scale",
        v.scale_root.is_some(),
    ) {
        act.scale = true;
    }
    if v.scale_root.is_none() {
        v.scale_view = ScaleView::Off;
    }
    y += 46.0;

    // Velocity of the selection (average), or the velocity for new notes.
    let vel = if n_sel == 0 {
        v.new_velocity as f32
    } else {
        v.selected.iter().map(|&i| notes[i].vel as f32).sum::<f32>() / n_sel as f32
    };
    hw::bracket(&gi, ix, ix + iw, y, tr("Velocity"));
    y += 16.0;
    let mut t = vel / 127.0;
    let shown = format!("{}", vel.round() as i32);
    if insp_slider(ui, &gi, ix, iw, y, "velocity", &mut t, &shown) {
        let new = (t * 127.0).round().clamp(1.0, 127.0) as u8;
        if n_sel == 0 {
            v.new_velocity = new;
        } else {
            let d = new as i32 - vel.round() as i32;
            for &i in &v.selected {
                notes[i].vel = (notes[i].vel as i32 + d).clamp(1, 127) as u8;
            }
            v.dirty = true;
        }
    }
    y += 34.0;
    // The chord the selected notes make, shown in the toolbar next frame.
    v.chord = (n_sel >= 2)
        .then(|| {
            let pitches: Vec<u8> = v.selected.iter().map(|&i| notes[i].pitch).collect();
            note_edit::chord_name(&pitches)
        })
        .flatten();

    // Automation/MIDI: what the lane under the notes shows (the lane's only selector).
    y += 10.0;
    if y + 44.0 < plate.bottom() {
        hw::bracket(&gi, ix, ix + iw, y, tr("Automation/MIDI"));
        let r = Rect::from_min_size(pos2(ix, y + 16.0), vec2(iw, 24.0));
        let shown = if tune_on {
            tr("Pitch Correction (Pro Wave Tune)").to_string()
        } else {
            lane_label(v)
        };
        let resp = popup_button(ui, &gi, r, id("lane_popup"), &shown);
        egui::Popup::menu(&resp).show(|ui| lane_menu(ui, v));
    }

    // ---- Apply toolbar/inspector actions
    let targets: Vec<usize> = if v.selected.is_empty() {
        (0..notes.len()).collect()
    } else {
        v.selected.clone()
    };
    if act.quantize {
        note_edit::quantize(notes, &targets, GRIDS[v.grid].1, v.strength, v.swing);
        v.dirty = true;
    }
    if act.scale {
        if let Some(root) = v.scale_root {
            note_edit::scale_quantize(notes, &targets, root, MODES[v.scale_mode].1);
            v.dirty = true;
        }
    }
    for op in std::mem::take(&mut act.ops) {
        let changed = apply_op(op, notes, v, &targets, position, step);
        v.dirty |= changed;
    }
    if act.delete && !v.selected.is_empty() {
        note_edit::delete(notes, &std::mem::take(&mut v.selected));
        v.dirty = true;
    }

    // ---- Draw keys, notes, ruler, playhead (after edits so they show immediately)
    let notes = &p.tracks[ti].regions[ri].notes;
    let gk = ui.painter_at(keys);
    gk.rect_filled(keys, 0.0, theme::SEAM);
    for row in top_row..=bottom_row {
        let pitch = rows.pitch(row);
        let y = grid.top() + row as f32 * key_h - scroll_y;
        let r = Rect::from_min_size(pos2(keys.left(), y), vec2(KEYS_W, key_h));
        let held = v.held == Some(pitch as u8);
        // Two white keys meet (E over F, B over C), or any two rows when rows are left out.
        let below = rows.pitch(row + 1);
        let seam_below =
            !is_black(pitch) && (rows.0.is_some() || (below != pitch && !is_black(below)));
        paint_piano_key(&gk, r, is_black(pitch), held, seam_below);
        // Every row is named when rows are left out (Collapse, Fold), else the Cs.
        if (rows.0.is_some() && key_h >= 9.0) || pitch % 12 == 0 {
            let label = if rows.0.is_some() {
                note_edit::note_name(pitch as u8)
            } else {
                c_label(pitch)
            };
            let on_black = rows.0.is_some() && is_black(pitch);
            hw::text(
                &gk,
                pos2(keys.right() - 5.0, y + key_h / 2.0),
                Align2::RIGHT_CENTER,
                &label,
                Face::Bold,
                (key_h * 0.75).clamp(7.5, 9.5),
                if on_black && !held {
                    theme::ON_DARK
                } else {
                    theme::INK2
                },
            );
        }
    }
    hw::seam_v(&gk, keys.right() - 2.0, keys.y_range());
    let visible = |r: &Rect| {
        r.right() >= grid.left()
            && r.left() <= grid.right()
            && r.bottom() >= grid.top()
            && r.top() <= grid.bottom()
    };
    let note_rect = |n: &Note, off: f32| {
        Rect::from_min_size(
            pos2(x_of(off + n.start), y_of(n.pitch as i32) + 1.0),
            vec2((n.len * beat_px).max(3.0), key_h - 2.0),
        )
    };
    // The track's other regions: their notes printed faintly on the floor.
    for &(r, off) in &others {
        for n in &p.tracks[ti].regions[r].notes {
            if !rows.shows(n.pitch as i32) {
                continue;
            }
            let rect = note_rect(n, off);
            if visible(&rect) {
                gg.rect_filled(rect, 2.0, ink(0.16));
                gg.rect_stroke(
                    rect,
                    2.0,
                    Stroke::new(1.0, ink(0.28)),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
    let label_size = 9.0;
    let hover_pt = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|pt| grid.contains(*pt));
    for (i, n) in notes.iter().enumerate() {
        let r = note_rect(n, 0.0);
        if !visible(&r) {
            continue;
        }
        let selected = v.selected.contains(&i);
        let hot = hover_pt.is_some_and(|pt| r.contains(pt));
        let print = paint_note(&gg, r, n.vel, selected, n.mute, hot);
        // The name only where the whole of it fits on the note; never cut short.
        if v.note_labels && key_h >= 10.0 {
            let name = note_edit::note_name(n.pitch);
            let w = hw::galley(&gg, &name, Face::Bold, label_size, print)
                .size()
                .x;
            if r.width() >= w + 8.0 {
                hw::text(
                    &gg.with_clip_rect(r.intersect(grid)),
                    pos2(r.left() + 4.0, r.center().y - 1.0),
                    Align2::LEFT_CENTER,
                    &name,
                    Face::Bold,
                    label_size,
                    print,
                );
            }
        }
    }
    if let Drag::Marquee { from } = &v.drag {
        if let Some(now) = ui.input(|i| i.pointer.hover_pos()) {
            let r = Rect::from_two_pos(*from, now);
            gg.rect_filled(r, 0.0, theme::ORANGE.gamma_multiply(0.08));
            gg.rect_stroke(
                r,
                0.0,
                Stroke::new(1.0, theme::ORANGE),
                egui::StrokeKind::Inside,
            );
        }
    }

    // The Automation/MIDI lane: a printed 0 / 64 / 127 scale at its left (the lane is picked
    // in the inspector), then the lane, a dark window like a meter's with its name in it.
    let gv = ui.painter_at(Rect::from_min_max(vel_label.min, vel_rect.max));
    if v.show_lane {
        gv.rect_filled(
            Rect::from_min_max(vel_label.min - vec2(0.0, 4.0), vel_rect.max),
            0.0,
            theme::BONE,
        );
        let marks: &[(f32, &str)] = match v.lane {
            _ if tune_on => &[],
            Lane::PitchBend => &[(0.0, "-"), (0.5, "0"), (1.0, "+")],
            _ => &[(0.0, "0"), (64.0 / 127.0, "64"), (1.0, "127")],
        };
        let scale_r = Rect::from_min_max(
            pos2(vel_label.left(), vel_rect.top() + 7.0),
            pos2(vel_label.right() - 6.0, vel_rect.bottom() - 7.0),
        );
        hw::scale_v(&gv, scale_r, marks, false);
        hw::seam_v(&gv, vel_label.right() - 2.0, vel_label.y_range());
    }
    gv.rect_filled(vel_rect.translate(vec2(0.0, 1.0)), 3.0, theme::BONE_HI);
    gv.rect_filled(vel_rect, 3.0, theme::METER_WELL);
    let lane_line = hw::lerp(theme::METER_WELL, theme::ON_DARK, 0.09);
    for f in [0.25f32, 0.5, 0.75] {
        let y = vel_rect.bottom() - f * vel_rect.height();
        gv.hline(vel_rect.x_range(), y, Stroke::new(1.0, lane_line));
    }
    for &(rel, _) in &bars {
        let bx = x_of(rel as f32);
        if bx > vel_rect.left() + 1.0 && bx < vel_rect.right() - 1.0 {
            gv.vline(
                bx,
                vel_rect.y_range(),
                Stroke::new(1.0, hw::lerp(theme::METER_WELL, theme::ON_DARK, 0.2)),
            );
        }
    }
    hw::inner_shadow(&gv, vel_rect, 3.0, 0.6);
    if v.show_lane && !tune_on {
        let name = match v.lane {
            Lane::Velocity => tr("Velocity").to_string(),
            Lane::PitchBend => tr("Bend").to_string(),
            Lane::Pressure => tr("Pressure").to_string(),
            Lane::Cc(_) => lane_label(v),
        };
        hw::silk(
            &gv,
            vel_rect.left_top() + vec2(8.0, 9.0),
            Align2::LEFT_CENTER,
            &name,
            10.0,
            theme::ON_DARK_DIM,
        );
    }
    if v.lane != Lane::Velocity && v.show_lane && !tune_on {
        let view = midi_draw::LaneView {
            rect: vel_rect,
            x_of: &x_of,
            beat_at: &beat_at,
            tool,
        };
        let controls = &p.tracks[ti].regions[ri].controls;
        midi_draw::draw(&gv, &view, v.lane, controls, v.lane_sel);
    }
    for (i, n) in notes.iter().enumerate() {
        if v.lane != Lane::Velocity || !v.show_lane || tune_on {
            break;
        }
        let x = x_of(n.start);
        if x < vel_rect.left() - 4.0 || x > vel_rect.right() + 4.0 {
            continue;
        }
        let top = vel_rect.bottom() - n.vel as f32 / 127.0 * (vel_rect.height() - 6.0);
        let sel = v.selected.contains(&i);
        // Each note's velocity: a stem up from the floor with a head, and a line along the
        // note (lit like a meter's segments; orange for the selection).
        let col = if sel { theme::ORANGE } else { theme::BONE_HI };
        if sel {
            gv.vline(
                x,
                top..=vel_rect.bottom() - 2.0,
                Stroke::new(6.0, theme::ORANGE.gamma_multiply(0.25)),
            );
            gv.circle_filled(pos2(x, top), 6.0, theme::ORANGE.gamma_multiply(0.3));
        }
        gv.vline(
            x,
            top..=vel_rect.bottom() - 2.0,
            Stroke::new(3.0, col.gamma_multiply(if sel { 1.0 } else { 0.55 })),
        );
        gv.circle_filled(pos2(x, top), 2.5, col);
    }

    // Ruler (absolute bar numbers) and region strip.
    let gr = ui.painter_at(Rect::from_min_max(
        pos2(keys.left(), editor.top()),
        editor.max,
    ));
    let head = Rect::from_min_max(
        pos2(keys.left(), editor.top()),
        pos2(editor.right(), grid.top()),
    );
    hw::chassis(&gr, head);
    // The region's span on the ruler: a lit band with an ink edge under it.
    let span = Rect::from_min_max(
        pos2(rx0.max(grid.left()), ruler.top()),
        pos2(rx1.min(grid.right()), ruler.bottom()),
    );
    if span.width() > 0.0 {
        gr.rect_filled(span, 0.0, theme::BONE_HI);
        gr.hline(
            span.x_range(),
            span.bottom() - 1.0,
            Stroke::new(2.0, theme::INK2),
        );
    }
    let label_every = [1i32, 2, 4, 8, 16]
        .into_iter()
        .find(|n| *n as f32 * bpb as f32 * beat_px >= 40.0)
        .unwrap_or(32);
    // Quarter ticks along the bottom of the ruler.
    if beat_px >= 6.0 {
        for b in first..=last {
            let bx = x_of(b as f32);
            if bx >= grid.left() {
                gr.vline(
                    bx,
                    ruler.bottom() - 6.0..=ruler.bottom(),
                    Stroke::new(1.0, ink(0.4)),
                );
            }
        }
    }
    for &(rel, bar0) in &bars {
        let x = x_of(rel as f32);
        gr.vline(
            x,
            ruler.top() + 4.0..=ruler.bottom(),
            Stroke::new(1.0, theme::INK2),
        );
        if (bar0 as i32 - (start_bar as i32 - 1)).rem_euclid(label_every) == 0 {
            hw::text(
                &gr,
                pos2(x + 4.0, ruler.top() + 10.0),
                Align2::LEFT_CENTER,
                &bar_label(bar0 + 1),
                Face::Lcd,
                11.0,
                theme::INK,
            );
        }
    }
    // The other regions' headers, then the edited region's (selected: orange).
    let track_color = p.tracks[ti].color;
    let tag = if p.tracks[ti].kind == TrackKind::Drummer {
        "DR"
    } else {
        "IN"
    };
    hw::seam_h(&gr, head.x_range(), region_bar.top() - 1.0);
    for &(r, off) in &others {
        let len = p.region_len_beats(&p.tracks[ti].regions[r]) as f32;
        let strip = Rect::from_min_max(
            pos2(x_of(off).max(region_bar.left()), region_bar.top() + 2.0),
            pos2(
                x_of(off + len).min(region_bar.right()),
                region_bar.bottom() - 1.0,
            ),
        );
        if strip.width() > 0.0 {
            let look = theme::region_look(track_color, false, false, false);
            region_header(&gr, strip, &look, tag, &p.tracks[ti].regions[r].name);
        }
    }
    let strip = Rect::from_min_max(
        pos2(rx0.max(region_bar.left()), region_bar.top() + 2.0),
        pos2(rx1.min(region_bar.right()), region_bar.bottom() - 1.0),
    );
    if strip.width() > 0.0 {
        let look = theme::region_look(track_color, false, true, false);
        region_header(&gr, strip, &look, tag, &region_name);
    }
    hw::seam_h(&gr, head.x_range(), grid.top() - 2.0);
    if let (Some(link), true) = (tune_link, tune_on) {
        crate::tune_lane::draw_grid(&gg, p, link, &tune_geom);
        crate::tune_lane::amount_lane(ui, p, &mut s.tune, link, vel_rect, &x_abs, &beat_abs, tool);
    }
    let px = x_of(position);
    if px >= grid.left() && px <= grid.right() {
        playhead(&gg, &gr, px, grid, ruler.top() + 1.0);
    }

    // A click on a dimmed note of another region opens that region, the view kept in place.
    if let Some((r, off, i)) = open_other {
        s.tracks.selected = Some((ti, r));
        v.pinned = Some((ti, r));
        v.centered_for = Some((ti, r));
        v.scroll_x -= off * beat_px;
        v.selected = vec![i];
    }

    // Rebuild the engine song once per gesture, not on every frame of a drag.
    if v.dirty && !ui.input(|i| i.pointer.any_down()) {
        v.dirty = false;
        p.song_rev += 1;
    }
}

/// The lanes of the Automation/MIDI area, the one shown ticked.
fn lane_menu(ui: &mut Ui, v: &mut PianoView) {
    ui.set_min_width(220.0);
    let mut pick = None;
    for lane in Lane::ALL {
        if ui
            .selectable_label(v.lane == lane && !v.tune_lane, lane.name())
            .clicked()
        {
            pick = Some(lane);
        }
    }
    if v.tune_here {
        ui.separator();
        if ui
            .selectable_label(v.tune_lane, tr("Pitch Correction (Pro Wave Tune)"))
            .clicked()
        {
            v.tune_lane = true;
            v.lane = Lane::Velocity;
            v.show_lane = true;
            ui.close();
        }
    }
    if let Some(lane) = pick {
        v.lane = lane;
        v.lane_sel = None;
        v.show_lane = true;
        v.tune_lane = false;
        ui.close();
    }
}

/// The name of the lane shown.
fn lane_label(v: &PianoView) -> String {
    v.lane.name()
}

/// The Piano Roll's own OS window (Window > Open Piano Roll, Command-4; Logic:
/// <https://support.apple.com/guide/logicpro/piano-roll-editor-interface-lgcpc788b136/mac>).
pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("prowave_piano_roll_window")
}

/// Shows the Piano Roll in its own window while `s.piano_window` is set: an immediate
/// viewport, drawn with the main window so it follows the playhead. Its close button and
/// Command-W close it; Space plays and stops from it too. Where the system has no second
/// windows (and in tests) it is a window inside the main one.
pub fn show_window(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    if !s.piano_window {
        return;
    }
    let title = match target_region(p, s) {
        Some((t, r)) => format!("{} — {}", tr("Piano Roll"), p.tracks[t].regions[r].name),
        None => tr("Piano Roll").to_string(),
    };
    let builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([1100.0, 620.0])
        .with_min_inner_size([760.0, 380.0])
        .with_resizable(true);
    let mut close = false;
    let mut toggle_play = false;
    ctx.show_viewport_immediate(viewport_id(), builder, |ui, class| {
        let own = class != egui::ViewportClass::EmbeddedWindow;
        let vctx = ui.ctx().clone();
        if own {
            crate::musical_typing::poll(&vctx, &mut s.typing);
            close = vctx.input_mut(|i| {
                i.viewport().close_requested()
                    || i.consume_key(egui::Modifiers::COMMAND, egui::Key::W)
            });
            toggle_play = !vctx.egui_wants_keyboard_input()
                && vctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG))
            .show(ui, |ui| {
                ui.push_id("piano_roll_window", |ui| show(ui, p, s));
            });
    });
    if toggle_play {
        p.playing = !p.playing;
    }
    if close {
        s.piano_window = false;
    }
}

/// MIDI In: enters the keys played (`input`) into the region on show at the playhead, a
/// grid step long, and moves the playhead on (Logic: step input). Chords go in together.
/// Returns whether the notes were taken (step input on and a region to put them in).
pub fn step_input(p: &mut Project, s: &mut UiState, input: &[crate::recorder::LiveNote]) -> bool {
    use crate::recorder::LiveNote;
    if !s.piano.midi_in || p.recording {
        return false;
    }
    let Some((ti, ri)) = target_region(p, s) else {
        return false;
    };
    {
        let r = &p.tracks[ti].regions[ri];
        if r.generated.is_some() || r.folder.is_some() {
            return false;
        }
    }
    let origin = p.region_start_beats(&p.tracks[ti].regions[ri]);
    let len = s.piano.last_step.max(note_edit::TICK);
    let mut pos = ((p.position_beats - origin) as f32).max(0.0);
    let mut changed = false;
    for ev in input {
        match *ev {
            LiveNote::On { pitch, vel } => {
                let notes = &mut p.tracks[ti].regions[ri].notes;
                let (i, next) = s.piano.step.note_on(notes, pitch, vel, pos, len);
                if !std::mem::replace(&mut changed, true) {
                    s.piano.selected.clear();
                }
                s.piano.selected.push(i);
                pos = next;
            }
            LiveNote::Off { pitch } => s.piano.step.note_off(pitch),
            _ => {}
        }
    }
    if changed {
        // The region grows to hold the notes, by whole bars.
        let end = p.tracks[ti].regions[ri]
            .notes
            .iter()
            .map(|n| n.start + n.len)
            .fold(0.0, f32::max) as f64;
        let start_bar = p.tracks[ti].regions[ri].start_bar as f64;
        let end_bar = p.beats_to_bar(origin + end) + 1.0;
        let r = &mut p.tracks[ti].regions[ri];
        if end_bar > start_bar + r.len_bars as f64 {
            r.len_bars = (end_bar - start_bar).ceil() as f32;
        }
        p.position_beats = origin + pos as f64;
        p.song_rev += 1;
    }
    true
}

/// Runs a menu or key edit on the region's notes; `targets` is the selection (all notes when
/// none is selected). Returns whether the notes changed.
fn apply_op(
    op: Op,
    notes: &mut Vec<Note>,
    v: &mut PianoView,
    targets: &[usize],
    playhead: f32,
    step: f32,
) -> bool {
    let sel = v.selected.clone();
    match op {
        Op::Copy | Op::Cut if sel.is_empty() => false,
        Op::Copy => {
            v.clipboard = note_edit::copy(notes, &sel);
            false
        }
        Op::Cut => {
            v.clipboard = note_edit::copy(notes, &sel);
            note_edit::delete(notes, &sel);
            v.selected.clear();
            true
        }
        // Paste goes to the playhead (or the region start when the playhead is before it).
        Op::Paste if !v.clipboard.is_empty() => {
            let clip = v.clipboard.clone();
            v.selected = note_edit::paste(notes, &clip, playhead.max(0.0));
            true
        }
        Op::Paste => false,
        Op::Duplicate => {
            let new = note_edit::duplicate(notes, &sel, step);
            let changed = !new.is_empty();
            if changed {
                v.selected = new;
            }
            changed
        }
        Op::SelectPitch => {
            let mut pitches: Vec<u8> = sel.iter().map(|&i| notes[i].pitch).collect();
            pitches.dedup();
            v.selected = (0..notes.len())
                .filter(|&i| pitches.contains(&notes[i].pitch))
                .collect();
            false
        }
        Op::Transpose(d) => {
            note_edit::shift(notes, &sel, 0.0, d);
            !sel.is_empty()
        }
        Op::Trim(against, gap, only) => {
            note_edit::trim_end(notes, &sel, against, gap, only);
            !sel.is_empty()
        }
        Op::EndToPlayhead => {
            note_edit::end_to(notes, &sel, playhead);
            !sel.is_empty()
        }
        Op::StartToPlayhead => {
            note_edit::start_to(notes, &sel, playhead);
            !sel.is_empty()
        }
        Op::Mute => {
            note_edit::toggle_mute(notes, &sel);
            !sel.is_empty()
        }
        Op::Join => {
            if sel.len() > 1 {
                v.selected = note_edit::glue(notes, &sel);
            }
            sel.len() > 1
        }
        Op::LengthToGrid => {
            note_edit::set_length(notes, targets, step);
            true
        }
        Op::FixedVelocity => {
            note_edit::fixed_velocity(notes, targets, v.fixed_vel);
            true
        }
        Op::Humanize => {
            v.humanize_seed = v.humanize_seed.wrapping_mul(747_796_405).wrapping_add(1);
            let mut rng = note_edit::Rng::new(v.humanize_seed);
            let time = v.humanize_ticks as f32 * note_edit::TICK;
            note_edit::humanize(notes, targets, time, v.humanize_vel, &mut rng);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoomed_out_smart_snap_uses_the_signature_beat() {
        assert_eq!(piano_step(Snap::Smart, 10.0, 4.0, 1.0), 1.0);
        assert_eq!(piano_step(Snap::Smart, 10.0, 3.0, 0.5), 0.5);
        assert_eq!(piano_step(Snap::Smart, 100.0, 3.0, 0.5), 0.125);
        assert_eq!(piano_step(Snap::Beat, 100.0, 3.0, 0.5), 0.5);
    }

    #[test]
    fn absolute_snap_lands_on_the_grid_and_relative_keeps_the_offset() {
        assert!((snapped_start(1.1, 0.9, 0.25, false) - 2.1).abs() < 1e-6);
        assert!((snapped_start(1.1, 0.9, 0.25, true) - 2.0).abs() < 1e-6);
        assert_eq!(snapped_start(0.1, -1.0, 0.25, false), 0.0);
    }

    use crate::model::{Region, RegionParams};
    use crate::ui_test::Harness;
    use egui::{Event, Modifiers, PointerButton};

    fn note(start: f32, len: f32, pitch: u8) -> Note {
        Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start,
            len,
            pitch,
            vel: 100,
        }
    }

    fn region(start_bar: f32, notes: Vec<Note>) -> Region {
        Region {
            name: "R".into(),
            start_bar,
            len_bars: 2.0,
            notes,
            controls: Vec::new(),
            folder: None,
            audio: None,
            params: RegionParams::default(),
            generated: None,
        }
    }

    /// A Piano Roll on one region with C3 (0-1), E3 (2-3) and C3 (1.5-2), at 100 points a
    /// beat, rows 12 points high with G4 (pitch 79) at the top, snapping to the beat.
    struct Rig {
        p: Project,
        s: UiState,
        h: Harness,
    }

    impl Rig {
        fn new() -> Self {
            let mut p = Project::empty();
            p.tracks[0].regions.push(region(
                1.0,
                vec![note(0.0, 1.0, 60), note(2.0, 1.0, 64), note(1.5, 0.5, 60)],
            ));
            let mut s = UiState::new();
            s.tracks.selected = Some((0, 0));
            s.tracks.snap = Snap::Beat;
            let mut r = Self {
                p,
                s,
                h: Harness::new(1200.0, 700.0),
            };
            r.settle();
            let v = &mut r.s.piano;
            (v.beat_px, v.key_h, v.scroll_x, v.scroll_y) = (100.0, 12.0, 0.0, 48.0 * 12.0);
            v.catch = false;
            r.settle();
            r
        }
        fn settle(&mut self) {
            let (p, s) = (&mut self.p, &mut self.s);
            self.h.settle(|ui| show(ui, p, s));
        }
        fn at(&self, beat: f32, pitch: i32) -> Pos2 {
            let v = &self.s.piano;
            pos2(
                v.grid_rect.left() + beat * v.beat_px - v.scroll_x,
                v.grid_rect.top() + (127 - pitch) as f32 * v.key_h - v.scroll_y + v.key_h / 2.0,
            )
        }
        fn click(&mut self, pt: Pos2) {
            let (p, s) = (&mut self.p, &mut self.s);
            self.h.click(pt, |ui| show(ui, p, s));
        }
        fn drag(&mut self, from: Pos2, to: Pos2) {
            let path: Vec<Pos2> = (0..=6).map(|k| from.lerp(to, k as f32 / 6.0)).collect();
            let (p, s) = (&mut self.p, &mut self.s);
            self.h
                .drag(&path, PointerButton::Primary, |ui| show(ui, p, s));
        }
        fn key(&mut self, key: Key) {
            let (p, s) = (&mut self.p, &mut self.s);
            self.h.key(key, |ui| show(ui, p, s));
        }
        fn events(&mut self, events: Vec<Event>) {
            let (p, s) = (&mut self.p, &mut self.s);
            self.h.frame(events, |ui| show(ui, p, s));
        }
        fn hover(&mut self, pt: Pos2) {
            let (p, s) = (&mut self.p, &mut self.s);
            self.h.move_to(pt, |ui| show(ui, p, s));
        }
        fn notes(&self) -> &Vec<Note> {
            &self.p.tracks[0].regions[0].notes
        }
    }

    #[test]
    fn the_pencil_draws_a_note_as_long_as_the_drag() {
        let mut r = Rig::new();
        r.s.piano.tool = Tool::Pencil;
        let rev = r.p.song_rev;
        r.drag(r.at(4.2, 67), r.at(6.4, 67));
        let n = *r.notes().last().unwrap();
        assert_eq!((n.start, n.len, n.pitch), (4.0, 2.0, 67));
        assert!(r.p.song_rev > rev, "the engine hears the new note");
    }

    #[test]
    fn option_drag_copies_and_the_left_edge_resizes() {
        let mut r = Rig::new();
        r.h.modifiers = Modifiers::ALT;
        r.drag(r.at(0.5, 60), r.at(4.5, 60));
        r.h.modifiers = Modifiers::NONE;
        assert_eq!(r.notes().len(), 4);
        assert_eq!(r.notes()[0].start, 0.0, "the original stays");
        assert_eq!((r.notes()[3].start, r.notes()[3].pitch), (4.0, 60));
        // E3 (2-3) grabbed at its left edge and pulled a beat earlier: it starts at 1 and
        // still ends at 3.
        r.drag(r.at(2.02, 64), r.at(1.02, 64));
        assert_eq!((r.notes()[1].start, r.notes()[1].len), (1.0, 2.0));
    }

    #[test]
    fn eraser_scissors_and_join_tools() {
        let mut r = Rig::new();
        r.s.tracks.snap = Snap::Division;
        r.s.piano.tool = Tool::Scissors;
        r.click(r.at(2.4, 64));
        let e: Vec<(f32, f32)> = r
            .notes()
            .iter()
            .filter(|n| n.pitch == 64)
            .map(|n| (n.start, n.len))
            .collect();
        assert_eq!(e, vec![(2.0, 0.5), (2.5, 0.5)], "cut at the snap");
        // Join: the two C3s selected and joined into one.
        r.s.piano.tool = Tool::Glue;
        r.s.piano.selected = vec![0, 2];
        r.click(r.at(0.5, 60));
        let c: Vec<(f32, f32)> = r
            .notes()
            .iter()
            .filter(|n| n.pitch == 60)
            .map(|n| (n.start, n.len))
            .collect();
        assert_eq!(c, vec![(0.0, 2.0)]);
        r.s.piano.tool = Tool::Eraser;
        r.click(r.at(0.5, 60));
        assert!(r.notes().iter().all(|n| n.pitch != 60));
    }

    #[test]
    fn the_mute_tool_and_control_m_mute_notes_and_the_engine_skips_them() {
        let mut r = Rig::new();
        let played = |p: &Project| -> Vec<u8> {
            crate::engine_sync::song_notes_for_test(p)
                .iter()
                .filter(|n| n.track == 0)
                .map(|n| n.pitch)
                .collect()
        };
        assert!(played(&r.p).contains(&64));
        r.s.piano.tool = Tool::Mute;
        let rev = r.p.song_rev;
        r.click(r.at(2.5, 64));
        assert!(r.notes()[1].mute);
        assert!(r.p.song_rev > rev, "the engine is told");
        assert!(!played(&r.p).contains(&64), "a muted note is not played");
        // A click on a muted note does not sound it.
        r.s.piano.tool = Tool::Pointer;
        r.s.audition.clear();
        r.click(r.at(2.5, 64));
        assert!(!r
            .s
            .audition
            .iter()
            .any(|a| matches!(a, Audition::On { pitch: 64, .. })));
        // Control-M on the selection turns it back on.
        r.hover(r.at(6.0, 70));
        r.h.modifiers = Modifiers::CTRL;
        r.key(Key::M);
        r.h.modifiers = Modifiers::NONE;
        assert!(!r.notes()[1].mute);
        assert!(played(&r.p).contains(&64));
    }

    #[test]
    fn velocity_brush_and_quantize_tools() {
        let mut r = Rig::new();
        r.s.piano.tool = Tool::Velocity;
        r.s.piano.selected.clear();
        r.drag(r.at(2.5, 64), r.at(2.5, 64) - vec2(0.0, 20.0));
        assert_eq!(r.notes()[1].vel, 112);
        assert_eq!(r.notes()[0].vel, 100, "only the note dragged");
        // The Brush paints one 1/16 note (the Time Quantize value) per step it passes.
        r.s.piano.tool = Tool::Brush;
        let before = r.notes().len();
        r.drag(r.at(4.0, 67), r.at(4.9, 67));
        let painted: Vec<f32> = r.notes()[before..].iter().map(|n| n.start).collect();
        assert_eq!(painted, vec![4.0, 4.25, 4.5, 4.75]);
        // The Quantize tool puts the note it clicks on the 1/16 grid.
        r.p.tracks[0].regions[0].notes[1].start = 2.1;
        r.s.piano.tool = Tool::Quantize;
        r.click(r.at(2.5, 64));
        assert!((r.notes()[1].start - 2.0).abs() < 1e-6);
    }

    #[test]
    fn t_opens_the_tool_menu_and_a_letter_picks_the_tool() {
        let mut r = Rig::new();
        r.hover(r.at(6.0, 70));
        r.key(Key::T);
        assert!(r.s.piano.tool_menu.is_some());
        r.key(Key::E);
        assert_eq!(r.s.piano.tool, Tool::Eraser);
        assert!(r.s.piano.tool_menu.is_none());
        // The letter is the Tool menu's: E did not do anything else, and Q did not quantize.
        r.key(Key::T);
        r.key(Key::T);
        assert_eq!(r.s.piano.tool, Tool::Pointer, "T twice: the Pointer");
        // Command-click uses the Command-click tool (the Pencil): a drag draws a note.
        r.h.modifiers = Modifiers::COMMAND;
        let n = r.notes().len();
        r.drag(r.at(5.1, 70), r.at(7.2, 70));
        r.h.modifiers = Modifiers::NONE;
        assert_eq!(r.notes().len(), n + 1);
        assert_eq!(r.notes()[n].len, 2.0);
    }

    #[test]
    fn copy_paste_at_the_playhead_duplicate_and_transpose() {
        let mut r = Rig::new();
        r.hover(r.at(6.0, 70));
        r.s.piano.selected = vec![0];
        r.events(vec![Event::Copy]);
        r.p.position_beats = 6.0;
        r.events(vec![Event::Paste(String::new())]);
        assert_eq!(r.notes().len(), 4);
        assert_eq!((r.notes()[3].start, r.notes()[3].pitch), (6.0, 60));
        assert_eq!(r.s.piano.selected, vec![3], "the pasted note is selected");
        r.h.modifiers = Modifiers::COMMAND;
        r.key(Key::D);
        assert_eq!(r.notes().len(), 5);
        assert_eq!(r.notes()[4].start, 7.0, "repeated right after");
        // Option-Up: a semitone; Shift-Up: an octave; Option-Right: one tick.
        r.h.modifiers = Modifiers::ALT;
        r.key(Key::ArrowUp);
        assert_eq!(r.notes()[4].pitch, 61);
        r.key(Key::ArrowRight);
        assert!((r.notes()[4].start - (7.0 + note_edit::TICK)).abs() < 1e-6);
        r.h.modifiers = Modifiers::SHIFT;
        r.key(Key::ArrowUp);
        assert_eq!(r.notes()[4].pitch, 73);
        // Cut takes the selection away.
        r.h.modifiers = Modifiers::NONE;
        r.events(vec![Event::Cut]);
        assert_eq!(r.notes().len(), 4);
    }

    #[test]
    fn a_key_selects_its_pitch_and_shift_click_a_range_of_pitches() {
        let mut r = Rig::new();
        let key = |r: &Rig, pitch| pos2(r.s.piano.grid_rect.left() - 10.0, r.at(0.0, pitch).y);
        r.click(key(&r, 60));
        let mut sel = r.s.piano.selected.clone();
        sel.sort_unstable();
        assert_eq!(sel, vec![0, 2]);
        r.h.modifiers = Modifiers::SHIFT;
        r.click(key(&r, 65));
        r.h.modifiers = Modifiers::NONE;
        assert_eq!(r.s.piano.selected.len(), 3);
        // Shift-E: the notes with the selection's pitches.
        r.s.piano.selected = vec![2];
        r.hover(r.at(6.0, 70));
        r.h.modifiers = Modifiers::SHIFT;
        r.key(Key::E);
        let mut sel = r.s.piano.selected.clone();
        sel.sort_unstable();
        assert_eq!(sel, vec![0, 2]);
    }

    #[test]
    fn collapse_mode_moves_a_note_to_the_next_row_shown() {
        let mut r = Rig::new();
        r.s.piano.collapse = true;
        r.s.piano.scroll_y = 0.0;
        r.settle();
        // Two rows: E3 on top, C3 under it.
        let v = &r.s.piano;
        let row = |k: f32| pos2(0.0, v.grid_rect.top() + k * v.key_h + v.key_h / 2.0);
        let (top, second) = (row(0.0).y, row(1.0).y);
        let x = r.at(2.5, 0).x;
        r.drag(pos2(x, top), pos2(x, second));
        assert_eq!(r.notes()[1].pitch, 60, "E3 moved one row down is a C3");
    }

    #[test]
    fn another_region_of_the_track_is_shown_dimmed_and_a_click_opens_it() {
        let mut r = Rig::new();
        r.p.tracks[0]
            .regions
            .push(region(3.0, vec![note(1.0, 1.0, 67)]));
        r.s.piano.scroll_x = 600.0;
        r.settle();
        // The second region starts at bar 3: 8 beats after the first.
        r.click(r.at(9.5, 67));
        assert_eq!(r.s.tracks.selected, Some((0, 1)));
        assert_eq!(r.s.piano.selected, vec![0]);
        assert_eq!(r.p.tracks[0].regions[0].notes.len(), 3, "no note was added");
        // The view stayed where it was: 8 beats earlier relative to the new region's start.
        r.settle();
        assert_eq!(r.s.piano.scroll_x, 600.0 - 800.0);
    }

    #[test]
    fn catch_turns_the_page_to_the_playhead_and_link_off_keeps_the_region() {
        let mut r = Rig::new();
        r.s.piano.catch = true;
        r.p.playing = true;
        r.p.position_beats = 20.0;
        r.p.tracks[0].regions[0].len_bars = 8.0;
        r.settle();
        let v = &r.s.piano;
        let px = 20.0 * v.beat_px - v.scroll_x;
        assert!(
            px >= 0.0 && px < v.grid_rect.width(),
            "the playhead is in view"
        );
        r.p.playing = false;
        // Link off: selecting another region leaves the editor where it is.
        r.p.tracks[0].regions.push(region(9.0, vec![]));
        r.s.piano.link = false;
        r.settle();
        r.s.tracks.selected = Some((0, 1));
        r.settle();
        assert_eq!(target_region(&r.p, &r.s), Some((0, 0)));
        r.s.piano.link = true;
        assert_eq!(target_region(&r.p, &r.s), Some((0, 1)));
    }

    #[test]
    fn midi_in_enters_played_notes_at_the_playhead_and_moves_it_on() {
        use crate::recorder::LiveNote;
        let mut r = Rig::new();
        r.p.position_beats = 6.0;
        let played = [
            LiveNote::On { pitch: 72, vel: 90 },
            LiveNote::On { pitch: 76, vel: 80 },
            LiveNote::Off { pitch: 72 },
            LiveNote::Off { pitch: 76 },
            LiveNote::On { pitch: 79, vel: 70 },
            LiveNote::Off { pitch: 79 },
        ];
        // Off: nothing is entered.
        assert!(!step_input(&mut r.p, &mut r.s, &played));
        assert_eq!(r.notes().len(), 3);
        r.s.piano.midi_in = true;
        assert!(step_input(&mut r.p, &mut r.s, &played));
        let step = r.s.piano.last_step;
        assert_eq!(step, 1.0, "the grid: Snap is on the beat");
        let new: Vec<(f32, u8, f32)> = r.notes()[3..]
            .iter()
            .map(|n| (n.start, n.pitch, n.len))
            .collect();
        assert_eq!(new, vec![(6.0, 72, 1.0), (6.0, 76, 1.0), (7.0, 79, 1.0)]);
        assert_eq!(r.p.position_beats, 8.0);
        assert_eq!(r.p.tracks[0].regions[0].len_bars, 2.0, "8 beats still fit");
        // Past the region's end: it grows by whole bars.
        let more = [LiveNote::On { pitch: 60, vel: 64 }];
        step_input(&mut r.p, &mut r.s, &more);
        assert_eq!(r.p.tracks[0].regions[0].len_bars, 3.0);
        // Not while recording.
        r.p.recording = true;
        assert!(!step_input(&mut r.p, &mut r.s, &more));
    }

    #[test]
    fn midi_draw_lanes_draw_with_the_pencil_and_edit_points_with_the_pointer() {
        use crate::model::PITCH_BEND;
        let mut r = Rig::new();
        let g = r.s.piano.grid_rect;
        let (top, bottom) = (g.bottom() + SB_H + 4.0 + 3.0, r.h.size.y - 3.0);
        let lane_y = |v: f32| bottom - v * (bottom - top);
        let x = |r: &Rig, beat: f32| r.at(beat, 60).x;
        // Modulation: a Shift-drag with the Pencil draws a straight line from 0 to full.
        r.s.piano.lane = Lane::Cc(1);
        r.s.piano.tool = Tool::Pencil;
        r.settle();
        r.h.modifiers = Modifiers::SHIFT;
        r.drag(pos2(x(&r, 1.0), lane_y(0.0)), pos2(x(&r, 2.0), lane_y(1.0)));
        r.h.modifiers = Modifiers::NONE;
        let cc1: Vec<(f32, u16)> = r.p.tracks[0].regions[0]
            .controls
            .iter()
            .filter(|c| c.num == 1)
            .map(|c| (c.start, c.val))
            .collect();
        assert_eq!(cc1.first(), Some(&(1.0, 0)));
        assert_eq!(cc1.last().map(|c| c.0), Some(2.0));
        assert!(cc1.last().unwrap().1 >= 125, "{cc1:?}");
        assert!(cc1.windows(2).all(|w| w[1].1 >= w[0].1), "rising: {cc1:?}");
        // Pitch bend: a Pointer click puts one point; dragging it moves it.
        r.s.piano.lane = Lane::PitchBend;
        r.s.piano.tool = Tool::Pointer;
        r.settle();
        r.click(pos2(x(&r, 3.0), lane_y(0.5)));
        let bends: Vec<u16> = r.p.tracks[0].regions[0]
            .controls
            .iter()
            .filter(|c| c.num == PITCH_BEND)
            .map(|c| c.val)
            .collect();
        assert_eq!(bends.len(), 1);
        assert!((bends[0] as i32 - 8192).abs() < 200, "{bends:?}");
        r.drag(pos2(x(&r, 3.0), lane_y(0.5)), pos2(x(&r, 3.5), lane_y(1.0)));
        let c = *r.p.tracks[0].regions[0]
            .controls
            .iter()
            .find(|c| c.num == PITCH_BEND)
            .unwrap();
        assert!((c.start - 3.5).abs() < 0.02 && c.val > 16000, "{c:?}");
        // The notes were not touched, and A hides the lane.
        assert_eq!(r.notes().len(), 3);
        r.hover(r.at(6.0, 70));
        r.key(Key::A);
        assert!(!r.s.piano.show_lane);
    }

    #[test]
    fn the_piano_roll_window_shows_the_editor_and_edits_in_it() {
        assert_ne!(viewport_id(), egui::ViewportId::ROOT);
        let mut p = Project::empty();
        p.tracks[0]
            .regions
            .push(region(1.0, vec![note(0.0, 1.0, 60)]));
        let mut s = UiState::new();
        s.tracks.selected = Some((0, 0));
        let mut h = Harness::new(1300.0, 800.0);
        // Closed: nothing is drawn.
        h.settle(|ui| show_window(ui.ctx(), &mut p, &mut s));
        assert_eq!(s.piano.grid_rect, Rect::NOTHING);
        s.piano_window = true;
        h.settle(|ui| show_window(ui.ctx(), &mut p, &mut s));
        assert!(s.piano.grid_rect.width() > 300.0, "{:?}", s.piano.grid_rect);
        // The editor works in it: Select All with Command-A.
        let inside = s.piano.grid_rect.center();
        h.move_to(inside, |ui| show_window(ui.ctx(), &mut p, &mut s));
        h.modifiers = Modifiers::COMMAND;
        h.key(Key::A, |ui| show_window(ui.ctx(), &mut p, &mut s));
        assert_eq!(s.piano.selected, vec![0]);
    }

    #[test]
    fn trims_and_humanize_from_the_menu_functions() {
        let mut v = PianoView::default();
        let mut notes = vec![note(0.0, 3.0, 60), note(1.0, 1.0, 62)];
        v.selected = vec![0, 1];
        let all = [0, 1];
        assert!(apply_op(
            Op::Trim(note_edit::Against::Selected, note_edit::TRIM_GAP, true),
            &mut notes,
            &mut v,
            &all,
            0.0,
            1.0
        ));
        assert!((notes[0].len - (1.0 - note_edit::TRIM_GAP)).abs() < 1e-6);
        let before = notes.clone();
        v.humanize_ticks = 30;
        v.humanize_vel = 20;
        apply_op(Op::Humanize, &mut notes, &mut v, &all, 0.0, 1.0);
        assert_ne!(notes, before);
        apply_op(Op::LengthToGrid, &mut notes, &mut v, &all, 0.0, 0.5);
        assert!(notes.iter().all(|n| n.len == 0.5));
        v.fixed_vel = 33;
        apply_op(Op::FixedVelocity, &mut notes, &mut v, &all, 0.0, 0.5);
        assert!(notes.iter().all(|n| n.vel == 33));
        apply_op(Op::EndToPlayhead, &mut notes, &mut v, &all, 4.0, 0.5);
        assert!(notes.iter().all(|n| (n.start + n.len - 4.0).abs() < 1e-6));
    }

    /// A picture of the Piano Roll for checking the look (written to the target folder).
    #[test]
    fn piano_roll_picture() {
        let mut p = Project::empty();
        p.tracks[0].regions.push(region(
            1.0,
            vec![
                note(0.0, 1.0, 60),
                note(0.0, 1.0, 64),
                note(0.0, 1.0, 67),
                note(1.0, 0.5, 62),
                note(1.5, 0.5, 65),
                note(2.0, 2.0, 72),
            ],
        ));
        p.tracks[0]
            .regions
            .push(region(3.0, vec![note(0.0, 2.0, 69)]));
        let mut s = UiState::new();
        s.tracks.selected = Some((0, 0));
        s.piano.scale_root = Some(0);
        s.piano.scale_view = ScaleView::Highlight;
        // A muted note and a drawn Modulation lane.
        p.tracks[0].regions[0].notes[4].mute = true;
        s.piano.lane = Lane::Cc(1);
        midi_draw::draw_line(
            &mut p.tracks[0].regions[0].controls,
            Lane::Cc(1),
            (0.0, 0.0),
            (3.0, 127.0),
            midi_draw::DRAW_STEP,
        );
        let mut frame = 0;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        let _ = std::fs::create_dir_all(&dir);
        crate::ui_test::render_png(
            1200.0,
            620.0,
            1.0,
            Color32::BLACK,
            |ui| {
                frame += 1;
                if frame == 3 {
                    s.piano.selected = vec![0, 1, 2];
                    (s.piano.beat_px, s.piano.key_h) = (60.0, 14.0);
                    s.piano.scroll_y = 50.0 * 14.0;
                    s.piano.catch = false;
                }
                show(ui, &mut p, &mut s)
            },
            &dir.join("piano_roll.png"),
        )
        .unwrap();
    }

    #[test]
    fn collapsed_rows_map_pitches_both_ways() {
        let rows = Rows(Some(vec![72, 64, 60]));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.row(64), 1);
        assert_eq!(
            rows.row(63),
            2,
            "a pitch without a row takes the next lower row"
        );
        assert_eq!(rows.pitch(0), 72);
        assert_eq!(rows.pitch(9), 60);
        assert!(rows.shows(60) && !rows.shows(61));
        let all = Rows(None);
        assert_eq!((all.len(), all.row(127), all.pitch(67)), (128, 0, 60));
    }

    /// Pictures of the MIDI editors in the Hardware look, to compare with the mockup:
    /// `PROWAVE_MIDI_SHOTS=/folder cargo test -p prowave-app midi_editor_pictures -- --ignored`.
    #[test]
    #[ignore = "pictures"]
    fn midi_editor_pictures() {
        let dir = std::env::var_os("PROWAVE_MIDI_SHOTS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-midi-shots"));
        let _ = std::fs::create_dir_all(&dir);
        type Draw<'a> = &'a mut dyn FnMut(&mut Ui, &mut Project, &mut UiState);
        let shot = |name: &str, w: f32, h: f32, p: &mut Project, s: &mut UiState, f: Draw| {
            crate::ui_test::render_png(
                w,
                h,
                1.0,
                crate::theme::BONE,
                |ui| {
                    crate::theme::apply(ui.ctx());
                    f(ui, p, s);
                },
                &dir.join(name),
            )
            .unwrap();
        };
        // The Piano Roll on the demo's first instrument region, a few notes selected.
        let mut p = Project::demo();
        let mut s = UiState::new();
        s.tracks.selected = Some((0, 0));
        s.piano.catch = false;
        p.position_beats = p.region_start_beats(&p.tracks[0].regions[0]) + 3.0;
        let mut frame = 0;
        shot(
            "piano-roll.png",
            1400.0,
            640.0,
            &mut p,
            &mut s,
            &mut |ui, p, s| {
                frame += 1;
                if frame == 2 {
                    s.piano.selected = vec![2, 3, 4];
                }
                show(ui, p, s)
            },
        );
        // The same with the scale highlighted and a controller lane.
        s.piano.scale_root = Some(0);
        s.piano.scale_view = ScaleView::Highlight;
        s.piano.lane = Lane::Cc(1);
        midi_draw::draw_line(
            &mut p.tracks[0].regions[0].controls,
            Lane::Cc(1),
            (0.0, 20.0),
            (8.0, 120.0),
            midi_draw::DRAW_STEP,
        );
        shot(
            "piano-roll-scale-cc.png",
            1400.0,
            640.0,
            &mut p,
            &mut s,
            &mut |ui, p, s| show(ui, p, s),
        );
        // Score.
        s.score_tab = true;
        shot(
            "score.png",
            1400.0,
            520.0,
            &mut p,
            &mut s,
            &mut |ui, p, s| show(ui, p, s),
        );
        s.score_tab = false;
        // MIDI Transform window over the Piano Roll.
        s.transform.open_with(None);
        shot(
            "transform.png",
            1400.0,
            760.0,
            &mut p,
            &mut s,
            &mut |ui, p, s| {
                show(ui, p, s);
                crate::transform_ui::show(ui.ctx(), p, s);
            },
        );
        s.transform.open = false;
        // The Event List.
        s.show_lists = true;
        shot(
            "event-list.png",
            420.0,
            640.0,
            &mut p,
            &mut s,
            &mut |ui, p, s| crate::event_list::show(ui, p, s),
        );
        // Automation lanes in the Tracks area: the volume over the regions, the pan in a lane
        // of its own.
        {
            use prowave_engine::AutoTarget;
            let mut p = Project::demo();
            let mut s = UiState::new();
            let meters = crate::mixer::Meters::default();
            crate::automation_ui::set_mode(&mut p, &mut s.auto, true);
            for (b, v) in [(0.0, -12.0), (6.0, -12.0), (10.0, -2.0), (20.0, -8.0)] {
                p.tracks[0].auto.set_point(AutoTarget::Volume, b, v);
            }
            p.tracks[0].auto.set_point(AutoTarget::Pan, 2.0, 0.5);
            p.tracks[0].auto.set_point(AutoTarget::Pan, 12.0, -0.5);
            p.tracks[0].auto.extra.push(AutoTarget::Pan);
            p.tracks[1].auto.set_point(AutoTarget::Volume, 4.0, -6.0);
            shot(
                "automation.png",
                1400.0,
                520.0,
                &mut p,
                &mut s,
                &mut |ui, p, s| crate::tracks_area::show(ui, p, s, &meters),
            );
        }
        // Step Sequencer (a Pattern region) and Drummer Editor.
        if let Some(ri) = p.add_generated_region(0, false, 40.0) {
            s.tracks.selected = Some((0, ri));
            // A few steps on, at different velocities, one tied, one repeated, one by chance.
            if let Some(crate::model::Generator::Pattern(pat)) = p.tracks[0].regions[ri]
                .generated
                .as_mut()
                .map(|g| &mut g.source)
            {
                for (r, k, v) in [
                    (0, 0, 120),
                    (0, 4, 70),
                    (1, 2, 100),
                    (2, 8, 50),
                    (3, 12, 127),
                ] {
                    if let Some(row) = pat.rows.get_mut(r) {
                        row.steps[k] = v;
                    }
                }
                if let Some(row) = pat.rows.get_mut(1) {
                    row.steps[3] = 90;
                    row.tie[3] = true;
                    row.repeat[2] = 3;
                    row.chance[2] = 60;
                }
            }
            p.regenerate(0, ri);
            shot(
                "step-sequencer.png",
                1400.0,
                560.0,
                &mut p,
                &mut s,
                &mut |ui, p, s| show(ui, p, s),
            );
        }
        let dt = p
            .tracks
            .iter()
            .position(crate::step_editor::is_drum_track)
            .unwrap_or(0);
        if let Some(ri) = p.add_generated_region(dt, true, 40.0) {
            s.tracks.selected = Some((dt, ri));
            shot(
                "drummer.png",
                1400.0,
                560.0,
                &mut p,
                &mut s,
                &mut |ui, p, s| show(ui, p, s),
            );
        }
    }
}
