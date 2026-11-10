//! Hardware panel parts for the vocal windows (the Spectrogram and the lyric windows): a floating faceplate with a silkscreened title strip and screws,
//! bracketed sections, right-aligned row labels, dark value fields that open a dark list, knobs
//! with an LCD readout (click the readout to type a value), and a green LCD status window.
//! Declared from `spectrogram_view.rs` (the one vocal-group file in both builds).
// A kit: the public build has only the Spectrogram window.
#![allow(dead_code)]
use crate::hw::{self, Cap, Legend};
use crate::theme::{self, Face};
use egui::{
    pos2, vec2, Align2, Color32, Context, CornerRadius, Id, Margin, Painter, Pos2, Rect, Response,
    Sense, Stroke, Ui, Vec2,
};
use std::ops::RangeInclusive;

/// Height of a panel's title strip.
pub const HEAD_H: f32 = 34.0;
/// The panel's side margin.
pub const PAD: f32 = 14.0;
/// Width of the right-aligned label column of a row.
pub const LABEL_W: f32 = 104.0;
/// Pitch of the rows (as the inspector's).
pub const ROW_H: f32 = 31.0;
/// A knob cell.
pub const CELL: Vec2 = vec2(84.0, 118.0);

/// A floating Hardware panel: a bone faceplate with a title strip (screw, wordmark, a small
/// orange tab, `subtitle` in silkscreen, a close key) over a seam, and `add` below it. `size`
/// makes it resizable (default and smallest size); otherwise it is `width` wide. Dragging any
/// bare part of the panel moves it. Returns whether it stays open, and what `add` gave.
pub fn window<R>(
    ctx: &Context,
    id: Id,
    title: &str,
    subtitle: &str,
    width: f32,
    size: Option<(Vec2, Vec2)>,
    add: impl FnOnce(&mut Ui) -> R,
) -> (bool, Option<R>) {
    let frame = egui::Frame {
        inner_margin: Margin::ZERO,
        fill: theme::BONE,
        stroke: Stroke::new(1.0, theme::SEAM_DK),
        corner_radius: CornerRadius::same(6),
        outer_margin: Margin::ZERO,
        shadow: ctx.global_style().visuals.window_shadow,
    };
    let mut w = egui::Window::new(title)
        .id(id)
        .title_bar(false)
        .collapsible(false)
        .frame(frame);
    w = match size {
        Some((default, min)) => w.resizable(true).default_size(default).min_size(min),
        None => w.resizable(false).default_width(width),
    };
    let mut close = false;
    let shown = w.show(ctx, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        if size.is_none() {
            ui.set_width(width);
        }
        let full = ui.available_width();
        let (head, _) = ui.allocate_exact_size(vec2(full, HEAD_H), Sense::hover());
        let g = ui.painter().clone();
        title_strip(ui, &g, head, id, title, subtitle, &mut close);
        let inner = egui::Frame::NONE
            .inner_margin(Margin {
                left: PAD as i8,
                right: PAD as i8,
                top: 8,
                bottom: PAD as i8,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                add(ui)
            })
            .inner;
        // Screws in the bottom corners (a resizable panel's right one sits clear of the grip).
        let all = ui.min_rect();
        let right = all.right() - if size.is_some() { 26.0 } else { 8.0 };
        for (x, a) in [(all.left() + 8.0, 70.0), (right, -15.0)] {
            hw::screw(&g, pos2(x, all.bottom() - 8.0), 3.5, a);
        }
        inner
    });
    let inner = shown.and_then(|s| s.inner);
    (!close, inner)
}

/// The title strip: a lighter band, a screw, the title in big silkscreen, an orange tab, the
/// subtitle and a close key, over a seam.
fn title_strip(
    ui: &Ui,
    g: &Painter,
    head: Rect,
    id: Id,
    title: &str,
    subtitle: &str,
    close: &mut bool,
) {
    hw::gradient(g, head, 6.0, theme::BONE_TOP, theme::BONE);
    hw::screw(g, pos2(head.left() + 13.0, head.center().y), 4.0, 25.0);
    let title = hw::fit(
        g,
        &printable(g, &title.to_uppercase(), Face::SilkX, 14.0),
        Face::SilkX,
        14.0,
        head.width() - 26.0 - 70.0,
    );
    let t = hw::text(
        g,
        pos2(head.left() + 26.0, head.center().y),
        Align2::LEFT_CENTER,
        &title,
        Face::SilkX,
        14.0,
        theme::INK,
    );
    let tab = Rect::from_center_size(pos2(t.right() + 20.0, head.center().y), vec2(9.0, 9.0));
    g.rect_filled(tab, 1.0, theme::ORANGE);
    let key = Rect::from_center_size(pos2(head.right() - 22.0, head.center().y), vec2(26.0, 20.0));
    // A subtitle that only repeats the title is left off.
    if !subtitle.trim().is_empty() && !subtitle.trim().eq_ignore_ascii_case(title.trim()) {
        let room = key.left() - 12.0 - (tab.right() + 10.0);
        let shown = hw::fit(
            g,
            &printable(g, &subtitle.to_uppercase(), Face::Silk, 11.0),
            Face::Silk,
            11.0,
            room,
        );
        hw::text(
            g,
            pos2(tab.right() + 10.0, head.center().y),
            Align2::LEFT_CENTER,
            &shown,
            Face::Silk,
            11.0,
            theme::INK2,
        );
    }
    let (resp, face) = hw::key(
        ui,
        g,
        key,
        id.with("close"),
        Cap::Bone,
        false,
        None,
        Legend::Custom,
    );
    let c = face.center();
    let st = Stroke::new(1.6, theme::INK);
    g.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], st);
    g.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], st);
    if resp.clicked() {
        *close = true;
    }
    hw::seam_h(g, head.x_range(), head.bottom() - 2.0);
}

/// A bracketed section label across the panel (`|— VOICE ————|`).
pub fn section(ui: &mut Ui, label: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    hw::bracket(ui.painter(), r.left(), r.right() - 1.0, r.center().y, label);
}

/// Empty space.
pub fn gap(ui: &mut Ui, h: f32) {
    ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
}

/// A row: `label` right-aligned in silkscreen in the label column; returns the rect for its
/// field (24 tall).
pub fn row(ui: &mut Ui, label: &str) -> Rect {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let g = ui.painter();
    let shown = hw::fit(g, &label.to_uppercase(), Face::Silk, 11.0, LABEL_W);
    hw::text(
        g,
        pos2(r.left() + LABEL_W, r.center().y),
        Align2::RIGHT_CENTER,
        &shown,
        Face::Silk,
        11.0,
        theme::INK2,
    );
    Rect::from_min_max(
        pos2(r.left() + LABEL_W + 12.0, r.center().y - 12.0),
        pos2(r.right(), r.center().y + 12.0),
    )
}

/// The dark list a field opens.
fn list_frame() -> egui::Frame {
    egui::Frame {
        inner_margin: Margin::same(3),
        fill: Color32::from_rgb(0x1d, 0x1c, 0x1a),
        stroke: Stroke::new(1.0, theme::CAP_EDGE),
        corner_radius: CornerRadius::same(3),
        outer_margin: Margin::ZERO,
        shadow: egui::Shadow {
            offset: [0, 3],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha(70),
        },
    }
}

/// A dark value field with up/down arrows in `field` showing `shown`; a click opens the dark
/// list of `items` (the `selected` one in orange with a lit LED). Returns the item picked.
pub fn dropdown(
    ui: &mut Ui,
    id: Id,
    field: Rect,
    shown: &str,
    items: &[&str],
    selected: Option<usize>,
) -> Option<usize> {
    let g = ui.painter().clone();
    let shown = printable(&g, shown, Face::Bold, 12.0);
    let resp = hw::field(ui, &g, field, id, &shown, true);
    let mut picked = None;
    egui::Popup::menu(&resp)
        .id(id.with("list"))
        .frame(list_frame())
        .gap(3.0)
        .width(field.width())
        .show(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
            for (i, it) in items.iter().enumerate() {
                let (r, rr) =
                    ui.allocate_exact_size(vec2(field.width() - 6.0, 24.0), Sense::click());
                let p = ui.painter();
                if rr.hovered() {
                    p.rect_filled(r, 2.0, Color32::from_white_alpha(20));
                }
                let on = selected == Some(i);
                hw::led_sized(p, pos2(r.left() + 9.0, r.center().y), 2.5, on);
                let shown = hw::fit(
                    p,
                    &printable(p, &it.to_uppercase(), Face::Bold, 11.0),
                    Face::Bold,
                    11.0,
                    r.width() - 26.0,
                );
                hw::text(
                    p,
                    pos2(r.left() + 20.0, r.center().y),
                    Align2::LEFT_CENTER,
                    &shown,
                    Face::Bold,
                    11.0,
                    if on { theme::ORANGE } else { theme::ON_DARK },
                );
                if rr.clicked() {
                    picked = Some(i);
                    ui.close();
                }
            }
        });
    picked
}

/// A knob in a cell: its label above (two lines at most; a unit in brackets goes to the
/// readout), the knob, and an LCD readout under it.
pub struct Dial<'a> {
    pub label: &'a str,
    pub value: &'a mut f32,
    pub range: RangeInclusive<f32>,
    /// Double-clicking the knob goes back to it.
    pub default: f32,
    pub decimals: usize,
    /// The readout's unit when the label has none in brackets ("%").
    pub unit: &'a str,
    /// Off: the knob stays black with its pointer dark, the readout unlit, and nothing moves.
    pub enabled: bool,
}

/// `label` split into its name and the unit in brackets at its end ("Rate (Hz)" → "Rate",
/// "Hz").
pub fn split_unit(label: &str) -> (&str, &str) {
    match (label.rfind(" ("), label.ends_with(')')) {
        (Some(at), true) => (&label[..at], &label[at + 2..label.len() - 1]),
        _ => (label, ""),
    }
}

/// `text` in silkscreen broken into at most two lines of `width`.
fn silk_lines(g: &Painter, text: &str, size: f32, width: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let fits = lines
            .last()
            .is_some_and(|last| hw::silk_width(g, &format!("{last} {word}"), size) <= width);
        let full = lines.len() >= 2;
        match lines.last_mut() {
            Some(last) if fits || full => {
                last.push(' ');
                last.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
        .into_iter()
        .map(|l| hw::fit(g, &l.to_uppercase(), Face::Silk, size, width))
        .collect()
}

/// A row of knob cells across the panel, wrapping. Returns whether any value changed.
pub fn dials(ui: &mut Ui, id: Id, dials: Vec<Dial<'_>>) -> bool {
    let per_row = ((ui.available_width() / CELL.x).floor() as usize).max(1);
    let mut changed = false;
    for (n, chunk) in dials
        .into_iter()
        .collect::<Vec<_>>()
        .chunks_mut(per_row)
        .enumerate()
    {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), CELL.y), Sense::hover());
        for (i, d) in chunk.iter_mut().enumerate() {
            let cell = Rect::from_min_size(pos2(r.left() + i as f32 * CELL.x, r.top()), CELL);
            changed |= dial(ui, cell, id.with((n, i)), d);
        }
    }
    changed
}

/// Where a cell's label ends: every label in a row sits on this one baseline, 10 points above
/// the knob's top tick; a second line grows upward.
const LABEL_BASE: f32 = 26.0;
/// The knob's centre, down from the top of its cell.
const KNOB_Y: f32 = 58.0;
const KNOB_R: f32 = 16.0;

/// One knob cell.
fn dial(ui: &mut Ui, cell: Rect, id: Id, d: &mut Dial<'_>) -> bool {
    let g = ui.painter().clone();
    let (name, unit) = split_unit(d.label);
    let unit = if unit.is_empty() { d.unit } else { unit };
    let lines = silk_lines(&g, name, 10.5, cell.width() - 6.0);
    let ink = if d.enabled { theme::INK2 } else { theme::PRINT };
    for (k, l) in lines.iter().rev().enumerate() {
        hw::text(
            &g,
            pos2(cell.center().x, cell.top() + LABEL_BASE - k as f32 * 12.0),
            Align2::CENTER_BOTTOM,
            l,
            Face::Silk,
            10.5,
            ink,
        );
    }
    let c = pos2(cell.center().x, cell.top() + KNOB_Y);
    let (lo, hi) = (*d.range.start(), *d.range.end());
    let span = (hi - lo).max(1e-6);
    let to_knob = |v: f32| ((v - lo) / span * 2.0 - 1.0).clamp(-1.0, 1.0);
    let mut changed = false;
    if d.enabled {
        let (resp, new) = hw::knob(ui, &g, c, KNOB_R, id, to_knob(*d.value), to_knob(d.default));
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if let Some(k) = new {
            let v = (lo + (k + 1.0) / 2.0 * span).clamp(lo, hi);
            if v != *d.value {
                *d.value = v;
                changed = true;
            }
        }
    } else {
        paint_knob_off(&g, c, KNOB_R, to_knob(*d.value));
    }
    let readout = Rect::from_center_size(
        pos2(cell.center().x, cell.bottom() - 15.0),
        vec2(66.0, 24.0),
    );
    changed |= readout_field(ui, &g, readout, id.with("readout"), unit, d);
    changed
}

/// A knob whose control is not there: the same black knob, its pointer unlit.
pub fn paint_knob_off(g: &Painter, c: Pos2, r: f32, value: f32) {
    hw::paint_knob(g, c, r, value);
    let a = (-90.0 + value.clamp(-1.0, 1.0) * 135.0).to_radians();
    let d = vec2(a.cos(), a.sin());
    let w = (r * 0.16).clamp(1.6, 2.6);
    // Over the orange pointer, a little wider so none of it shows, then the unlit pointer.
    g.line_segment(
        [c + d * (r * 0.22), c + d * (r * 0.95)],
        Stroke::new(w + 1.4, theme::CAP),
    );
    g.line_segment(
        [c + d * (r * 0.25), c + d * (r * 0.92)],
        Stroke::new(w, theme::LED_OFF),
    );
}

/// A knob at `c` (radius `r`) with its readout in `readout`, laid out by the caller (a
/// toolbar). The label is printed by the caller too; its unit in brackets labels the readout.
pub fn inline_dial(ui: &mut Ui, c: Pos2, r: f32, readout: Rect, id: Id, d: &mut Dial<'_>) -> bool {
    let g = ui.painter().clone();
    let (lo, hi) = (*d.range.start(), *d.range.end());
    let span = (hi - lo).max(1e-6);
    let to_knob = |v: f32| ((v - lo) / span * 2.0 - 1.0).clamp(-1.0, 1.0);
    let (resp, new) = hw::knob(ui, &g, c, r, id, to_knob(*d.value), to_knob(d.default));
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let mut changed = false;
    if let Some(k) = new {
        let v = (lo + (k + 1.0) / 2.0 * span).clamp(lo, hi);
        changed = v != *d.value;
        *d.value = v;
    }
    let (_, unit) = split_unit(d.label);
    let unit = if unit.is_empty() { d.unit } else { unit };
    changed | readout_field(ui, &g, readout, id.with("readout"), unit, d)
}

fn round_to(v: f32, decimals: usize) -> f32 {
    let k = 10f32.powi(decimals as i32);
    (v * k).round() / k
}

/// An unlit LCD readout: the glass at full strength, the label faint and the segments at the
/// ghost level, the value only just there.
pub fn lcd_readout_off(g: &Painter, rect: Rect, label: &str, value: &str) {
    hw::lcd_glass(g, rect);
    hw::text(
        g,
        rect.left_top() + vec2(4.0, 3.0),
        Align2::LEFT_TOP,
        &label.to_uppercase(),
        Face::Lcd,
        7.5,
        theme::LCD_INK.gamma_multiply(0.4),
    );
    let h = (rect.height() * 0.55).min(18.0);
    let w = hw::seg_width(value, h);
    hw::seg(
        g,
        pos2(rect.right() - 5.0 - w, rect.bottom() - 4.0 - h),
        value,
        h,
        theme::LCD_INK.gamma_multiply(0.16),
        true,
    );
}

/// An LCD readout of a dial's value; a click opens it for typing (Return keeps the value,
/// Escape leaves it).
fn readout_field(
    ui: &mut Ui,
    g: &Painter,
    rect: Rect,
    id: Id,
    unit: &str,
    d: &mut Dial<'_>,
) -> bool {
    let edit_id = id.with("edit");
    let mut changed = false;
    let editing = ui.data(|m| m.get_temp::<String>(id));
    if editing.is_none() {
        let value = format!("{:.*}", d.decimals, d.value);
        if !d.enabled {
            lcd_readout_off(g, rect, unit, &value);
            return false;
        }
        hw::lcd_readout(g, rect, unit, &value);
        let resp = ui.interact(rect, id.with("click"), Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if resp.clicked() {
            ui.data_mut(|m| m.insert_temp(id, value));
            ui.memory_mut(|m| m.request_focus(edit_id));
        }
        return false;
    }
    let mut text = editing.unwrap_or_default();
    hw::lcd_glass(g, rect);
    focus_ring(g, rect);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(5.0, 3.0))));
    let edit = child.add(
        egui::TextEdit::singleline(&mut text)
            .id(edit_id)
            .frame(egui::Frame::NONE)
            .margin(Margin::ZERO)
            .font(theme::font(Face::Lcd, 13.0))
            .text_color(theme::LCD_INK)
            .align(Align2::RIGHT_CENTER)
            .desired_width(rect.width() - 10.0)
            .char_limit(12),
    );
    let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if edit.lost_focus() || cancel {
        if !cancel {
            if let Ok(v) = text.trim().replace(',', ".").parse::<f32>() {
                let v = round_to(v, d.decimals).clamp(*d.range.start(), *d.range.end());
                if v.is_finite() && v != *d.value {
                    *d.value = v;
                    changed = true;
                }
            }
        }
        ui.data_mut(|m| m.remove::<String>(id));
    } else {
        if !edit.has_focus() {
            edit.request_focus();
        }
        ui.data_mut(|m| m.insert_temp(id, text));
    }
    changed
}

/// The orange ring of a field that has the keyboard: 2 points, just inside its edge.
pub fn focus_ring(g: &Painter, rect: Rect) {
    g.rect_stroke(
        rect.shrink(1.0),
        3.0,
        Stroke::new(2.0, theme::ORANGE),
        egui::StrokeKind::Inside,
    );
}

/// A key cap that can't be pressed: the same cap seated in the panel (no lip or drop shadow),
/// its LED unlit and its legend in the dim print.
pub fn paint_key_off(g: &Painter, rect: Rect, cap: Cap, label: &str, led: bool) {
    let radius = (rect.height() * 0.12).clamp(3.0, 5.0);
    let dark = cap == Cap::Dark;
    let (top, mid, bottom, edge) = if dark {
        (
            theme::CAP_TOP,
            theme::CAP,
            Color32::from_rgb(0x15, 0x14, 0x13),
            theme::CAP_EDGE,
        )
    } else {
        (
            theme::BONE_TOP,
            Color32::from_rgb(0xee, 0xea, 0xe1),
            Color32::from_rgb(0xe3, 0xde, 0xd3),
            theme::KEY_EDGE,
        )
    };
    // Seated: only a contact line under it.
    g.rect_filled(
        rect.translate(vec2(0.0, 1.0)),
        radius,
        Color32::from_black_alpha(30),
    );
    hw::gradient3(g, rect, radius, top, mid, bottom);
    g.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, edge),
        egui::StrokeKind::Inside,
    );
    if led {
        hw::led_sized(g, pos2(rect.left() + 11.0, rect.center().y), 2.5, false);
    }
    let ink = if dark {
        theme::PRINT
    } else {
        theme::PRINT.gamma_multiply(0.8)
    };
    legend(g, rect, label, ink, led);
}

/// A key's legend centred on its face (with or without an LED, so a row's legends share one
/// baseline; clear of the LED when there is one). Too long for one line at 11 or 10 points, it
/// goes on two at 10.
fn legend(g: &Painter, face: Rect, label: &str, ink: Color32, led: bool) {
    let room = face.width() - if led { 44.0 } else { 16.0 };
    let label = label.to_uppercase();
    let width = |s: &str, size: f32| hw::galley(g, s, Face::Bold, size, ink).size().x;
    if let Some(size) = [11.0, 10.0].into_iter().find(|s| width(&label, *s) <= room) {
        hw::text(
            g,
            face.center(),
            Align2::CENTER_CENTER,
            &label,
            Face::Bold,
            size,
            ink,
        );
        return;
    }
    // Two lines, broken at the space that best evens them.
    let size = 10.0;
    let split = label.match_indices(' ').map(|(i, _)| i).min_by(|a, b| {
        let d = |i: usize| (width(&label[..i], size) - width(&label[i + 1..], size)).abs();
        d(*a).total_cmp(&d(*b))
    });
    let Some(at) = split else {
        let shown = hw::fit(g, &label, Face::Bold, size, room);
        hw::text(
            g,
            face.center(),
            Align2::CENTER_CENTER,
            &shown,
            Face::Bold,
            size,
            ink,
        );
        return;
    };
    for (k, line) in [&label[..at], &label[at + 1..]].into_iter().enumerate() {
        let shown = hw::fit(g, line, Face::Bold, size, room);
        hw::text(
            g,
            pos2(face.center().x, face.center().y + (k as f32 - 0.5) * 12.0),
            Align2::CENTER_CENTER,
            &shown,
            Face::Bold,
            size,
            ink,
        );
    }
}

/// A key in `rect` with `label` printed on it (an LED at its left when `led` is Some, 11 points
/// in and centred on its height); `enabled` off seats it unlit and takes no click.
pub fn key(
    ui: &Ui,
    rect: Rect,
    id: Id,
    cap: Cap,
    label: &str,
    led: Option<bool>,
    enabled: bool,
) -> Response {
    let g = ui.painter().clone();
    if !enabled {
        paint_key_off(&g, rect, cap, label, led.is_some());
        return ui.interact(rect, id, Sense::hover());
    }
    let (resp, face) = hw::key(ui, &g, rect, id, cap, false, None, Legend::Custom);
    if let Some(on) = led {
        hw::led_sized(&g, pos2(face.left() + 11.0, face.center().y), 2.5, on);
    }
    legend(&g, face, label, cap.ink(), led.is_some());
    resp
}

/// The action row across the panel: equal keys 34 tall on an 8-point gutter, in the order
/// given: (legend, cap, LED, enabled). Returns which were clicked.
pub fn key_row(ui: &mut Ui, id: Id, keys: &[(&str, Cap, Option<bool>, bool)]) -> Vec<bool> {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    let gap = 8.0;
    let n = keys.len().max(1) as f32;
    let w = (r.width() - gap * (n - 1.0)) / n;
    keys.iter()
        .enumerate()
        .map(|(i, (label, cap, led, enabled))| {
            let rect = Rect::from_min_size(
                pos2(r.left() + i as f32 * (w + gap), r.top()),
                vec2(w, 34.0),
            );
            key(ui, rect, id.with(i), *cap, label, *led, *enabled).clicked()
        })
        .collect()
}

/// Hearts the fonts may not have, and what stands in for them.
fn is_heart(c: char) -> bool {
    matches!(
        c,
        '\u{2661}' | '\u{2665}' | '\u{2764}' | '\u{2763}' | '\u{1F495}'..='\u{1F49F}' | '\u{1F9E1}'
    )
}

/// `text` with every character the fonts of `face` can't draw replaced (a heart by one they
/// have, or by "<3"; anything else by a space), so no empty box shows in its place.
pub fn printable(g: &Painter, text: &str, face: Face, size: f32) -> String {
    let font = theme::font(face, size);
    g.fonts_mut(|f| {
        if f.has_glyphs(&font, text) {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            if c.is_whitespace() || f.has_glyph(&font, c) {
                out.push(c);
            } else if is_heart(c) {
                match ['\u{2665}', '\u{2764}']
                    .into_iter()
                    .find(|h| f.has_glyph(&font, *h))
                {
                    Some(h) => out.push(h),
                    None => out.push_str("<3"),
                }
            } else {
                out.push(' ');
            }
        }
        out
    })
}

/// A 35% bone wash over everything behind the panel `id` (a dialog), which is kept above it.
/// Clicks still reach what is behind (notes can be selected while it is open).
pub fn scrim(ctx: &Context, id: Id) {
    let screen = ctx.content_rect();
    egui::Area::new(id.with("scrim"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .interactable(false)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, theme::BONE3.gamma_multiply(0.35));
        });
    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, id));
}

/// The credits of a voice in small silkscreen print on a recessed strip: two lines at most, the
/// whole text on hover.
pub fn credits(ui: &mut Ui, text: &str) {
    let g = ui.painter().clone();
    let width = ui.available_width();
    let size = 10.0;
    let clean = printable(&g, &text.to_uppercase(), Face::Silk, size);
    let mut job = egui::text::LayoutJob::single_section(
        clean,
        egui::TextFormat {
            font_id: theme::font(Face::Silk, size),
            color: theme::INK2,
            extra_letter_spacing: theme::tracking(Face::Silk) * size,
            line_height: Some(13.0),
            ..Default::default()
        },
    );
    job.wrap.max_width = width - 20.0;
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let gal = g.layout_job(job);
    let (r, resp) = ui.allocate_exact_size(vec2(width, gal.size().y + 14.0), Sense::hover());
    hw::recess(&g, r, 3.0);
    g.galley(pos2(r.left() + 10.0, r.top() + 7.0), gal, theme::INK2);
    resp.on_hover_text(text);
}

/// A run of LCD segments across `rect`, the first `frac` of them lit (progress).
pub fn lcd_ladder(g: &Painter, rect: Rect, frac: f32) {
    let pitch = 5.0;
    let n = ((rect.width() + 1.0) / pitch).floor().max(1.0) as usize;
    let lit = (frac.clamp(0.0, 1.0) * n as f32).round() as usize;
    for i in 0..n {
        let x = rect.left() + i as f32 * pitch;
        let r = Rect::from_min_max(pos2(x, rect.top()), pos2(x + pitch - 1.5, rect.bottom()));
        let c = if i < lit {
            theme::LCD_INK
        } else {
            theme::LCD_GHOST
        };
        g.rect_filled(r, 0.0, c);
    }
}

/// Work going on, with no measure of how far: a block of three lit segments running along a
/// row of LCD segments in `rect`.
pub fn lcd_chaser(g: &Painter, rect: Rect, time: f64) {
    let pitch = 5.0;
    let n = ((rect.width() + 1.0) / pitch).floor().max(4.0) as usize;
    let at = ((time * 12.0) as usize) % (n + 3);
    for i in 0..n {
        let x = rect.left() + i as f32 * pitch;
        let r = Rect::from_min_max(pos2(x, rect.top()), pos2(x + pitch - 1.5, rect.bottom()));
        let c = if i < at && i + 3 >= at {
            theme::LCD_INK
        } else {
            theme::LCD_GHOST
        };
        g.rect_filled(r, 0.0, c);
    }
}

/// LCD text in `rect` (centred on its height), wrapped to its width, at most `rows` lines.
pub fn lcd_text(g: &Painter, rect: Rect, text: &str, size: f32, rows: usize, color: Color32) {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_uppercase(),
        egui::TextFormat {
            font_id: theme::font(Face::Lcd, size),
            color,
            extra_letter_spacing: theme::tracking(Face::Lcd) * size,
            ..Default::default()
        },
    );
    job.wrap.max_width = rect.width();
    job.wrap.max_rows = rows;
    job.wrap.break_anywhere = false;
    let gal = g.layout_job(job);
    let y = rect.center().y - gal.size().y / 2.0;
    g.galley(pos2(rect.left(), y.max(rect.top())), gal, color);
}

/// A divider line on LCD glass.
pub fn lcd_divider(g: &Painter, x: f32, y: impl Into<egui::Rangef>) {
    g.vline(x, y, Stroke::new(1.0, theme::LCD_INK.gamma_multiply(0.22)));
}

/// A small label on LCD glass (zone names).
pub fn lcd_label(g: &Painter, pos: Pos2, text: &str) -> Rect {
    hw::text(
        g,
        pos,
        Align2::LEFT_TOP,
        &text.to_uppercase(),
        Face::Lcd,
        8.5,
        theme::LCD_INK.gamma_multiply(0.72),
    )
}

/// A recessed dark field (no arrows) behind a text being typed, with a 2-point orange ring
/// just outside it: the field has the keyboard.
pub fn edit_well(g: &Painter, rect: Rect) {
    hw::paint_field(g, rect, "", false, false);
    g.rect_stroke(
        rect,
        3.0,
        Stroke::new(2.0, theme::ORANGE),
        egui::StrokeKind::Outside,
    );
}

/// Where the window pictures go: `$MANUAL_PNG_DIR`, else a temporary folder.
#[cfg(test)]
pub fn picture_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::var_os("MANUAL_PNG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("prowave-manual"));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_in_brackets_go_to_the_readout() {
        assert_eq!(split_unit("Vibrato Rate (Hz)"), ("Vibrato Rate", "Hz"));
        assert_eq!(split_unit("Breathiness"), ("Breathiness", ""));
        assert_eq!(round_to(5.456, 1), 5.5);
    }
}
