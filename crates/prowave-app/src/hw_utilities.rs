//! Hardware pieces the utility windows share (the MIDI Monitor, Musical Typing, the device
//! editor and the Loop / All Files browser), built on `hw.rs`: foldable section brackets, keys
//! that can be switched off, a dark dropdown whose list scrolls, a stepped value field, small
//! pictograms and LCD rows. The floating faceplate itself is the vocal windows' one
//! (`hw_vocal::window`), so every small window wears the same title strip.
//!
//! Declared from `midi_monitor.rs` so the module list in `main.rs` stays as it is.
// A kit: the public build has no device editor, which uses some of these.
#![allow(dead_code)]
use crate::hw::{self, Cap};
use crate::hw_sampler;
use crate::theme::{self, Face};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, Id, Margin, Painter, Pos2, Rect, Response, Sense,
    Shape, Stroke, Ui,
};

pub use crate::spectrogram_view::hw_vocal::{lcd_divider, lcd_label, section, window, PAD};

/// Height of a key in a control row.
pub const KEY_H: f32 = 28.0;

/// A section bracket that folds what is under it: a small triangle, then the bracket with
/// `label`. Returns whether the section is open (kept between frames under `id`).
pub fn fold(ui: &mut Ui, id: Id, label: &str, default_open: bool) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let g = ui.painter();
    let c = pos2(r.left() + 5.0, r.center().y);
    let ink = if resp.hovered() {
        theme::INK
    } else {
        theme::INK2
    };
    let tri = if open {
        vec![
            c + vec2(-4.0, -2.5),
            c + vec2(4.0, -2.5),
            c + vec2(0.0, 3.0),
        ]
    } else {
        vec![
            c + vec2(-2.5, -4.0),
            c + vec2(3.0, 0.0),
            c + vec2(-2.5, 4.0),
        ]
    };
    g.add(Shape::convex_polygon(tri, ink, Stroke::NONE));
    hw_sampler::bracket(g, r.left() + 14.0, r.right() - 1.0, r.center().y, label);
    open
}

/// A key in `rect` with `label` (two lines at most, split at `\n`), an LED at its left when
/// `led` is Some, pressed in while `latched`. Off (`enabled` false) it is drawn faded and takes
/// no click.
#[allow(clippy::too_many_arguments)]
pub fn key(
    ui: &Ui,
    rect: Rect,
    id: Id,
    cap: Cap,
    latched: bool,
    led: Option<bool>,
    label: &str,
    enabled: bool,
) -> Response {
    let g = ui.painter().clone();
    if enabled {
        return hw_sampler::key(ui, &g, rect, id, cap, latched, led, label);
    }
    let mut faded = g;
    faded.set_opacity(0.42);
    hw_sampler::paint_key_sized(
        &faded,
        rect,
        cap,
        latched,
        false,
        led,
        label,
        (rect.height() * 0.4).clamp(8.5, 11.0),
    );
    ui.interact(rect, id, Sense::hover())
}

/// Width a key needs for `label` at `size` (with an LED when `led`).
pub fn key_width(g: &Painter, label: &str, size: f32, led: bool) -> f32 {
    let w = hw::galley(g, &label.to_uppercase(), Face::Bold, size, theme::INK)
        .size()
        .x;
    w + 22.0 + if led { 12.0 } else { 0.0 }
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

/// A dark value field with up/down arrows in `field` showing `shown`; a click opens a dark list
/// of `items` that scrolls when long (the `selected` one orange with a lit LED). Returns the
/// item picked. Off (`enabled` false) it is faded and does not open.
pub fn dropdown(
    ui: &mut Ui,
    id: Id,
    field: Rect,
    shown: &str,
    items: &[String],
    selected: Option<usize>,
    enabled: bool,
) -> Option<usize> {
    let g = ui.painter().clone();
    if !enabled {
        // Nothing to pick: the field shows what it holds, without arrows.
        hw::paint_field(&g, field, shown, false, false);
        return None;
    }
    let resp = hw::field(ui, &g, field, id, shown, true);
    let mut picked = None;
    let width = field.width().max(180.0);
    egui::Popup::menu(&resp)
        .id(id.with("list"))
        .frame(list_frame())
        .gap(3.0)
        .width(width)
        .show(|ui| {
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                    for (i, it) in items.iter().enumerate() {
                        let (r, rr) =
                            ui.allocate_exact_size(vec2(width - 8.0, 24.0), Sense::click());
                        let p = ui.painter();
                        if rr.hovered() {
                            p.rect_filled(r, 2.0, Color32::from_white_alpha(20));
                        }
                        let on = selected == Some(i);
                        hw::led_sized(p, pos2(r.left() + 9.0, r.center().y), 2.5, on);
                        let text =
                            hw::fit(p, &it.to_uppercase(), Face::Bold, 11.0, r.width() - 26.0);
                        hw::text(
                            p,
                            pos2(r.left() + 20.0, r.center().y),
                            Align2::LEFT_CENTER,
                            &text,
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
        });
    picked
}

/// A dark field showing a number from `lo` to `hi` with its up/down arrows: a click on the
/// upper or lower half steps it, as does the scroll wheel over it. Returns whether it changed.
pub fn stepper(ui: &Ui, id: Id, field: Rect, value: &mut u8, lo: u8, hi: u8, shown: &str) -> bool {
    let g = ui.painter().clone();
    let resp = hw::field(ui, &g, field, id, shown, true);
    let before = *value;
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if p.y < field.center().y {
                *value = value.saturating_add(1).min(hi);
            } else {
                *value = value.saturating_sub(1).max(lo);
            }
        }
    }
    if resp.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy > 0.5 {
            *value = value.saturating_add(1).min(hi);
        } else if dy < -0.5 {
            *value = value.saturating_sub(1).max(lo);
        }
    }
    *value != before
}

/// A small chip: ink with bone print, or orange with ink print when `lit`
/// (`hw_sampler::chip`).
pub fn chip(g: &Painter, left_center: Pos2, label: &str, size: f32, lit: bool) -> Rect {
    hw_sampler::chip(g, left_center, label, size, lit)
}

/// A light-bone chip with print (a passive tag: a tempo, a key, a category).
pub fn tag(g: &Painter, left_center: Pos2, label: &str, size: f32) -> Rect {
    let gal = hw::galley(g, &label.to_uppercase(), Face::Bold, size, theme::INK2);
    let r = Rect::from_min_size(
        pos2(left_center.x, left_center.y - (size + 5.0) / 2.0),
        vec2(gal.size().x + 8.0, size + 5.0),
    );
    g.rect_filled(r, 2.0, theme::BONE2);
    g.rect_stroke(
        r,
        2.0,
        Stroke::new(1.0, Color32::from_black_alpha(26)),
        egui::StrokeKind::Inside,
    );
    g.galley(
        pos2(r.left() + 4.0, r.center().y - gal.size().y / 2.0),
        gal,
        theme::INK2,
    );
    r
}

/// A right- or left-pointing triangle centred at `c` (message direction, folder rows).
pub fn arrow(g: &Painter, c: Pos2, size: f32, right: bool, color: Color32) {
    let s = size / 2.0;
    let pts = if right {
        vec![
            c + vec2(-s * 0.8, -s),
            c + vec2(s, 0.0),
            c + vec2(-s * 0.8, s),
        ]
    } else {
        vec![
            c + vec2(s * 0.8, -s),
            c + vec2(-s, 0.0),
            c + vec2(s * 0.8, s),
        ]
    };
    g.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// An arrow pointing up (the parent folder key), centred at `c`.
pub fn icon_up(g: &Painter, c: Pos2, color: Color32) {
    let st = Stroke::new(2.0, color);
    g.line_segment([c + vec2(0.0, 6.0), c + vec2(0.0, -5.0)], st);
    g.line_segment([c + vec2(-5.0, 0.0), c + vec2(0.0, -5.5)], st);
    g.line_segment([c + vec2(5.0, 0.0), c + vec2(0.0, -5.5)], st);
}

/// A folder pictogram in `r`.
pub fn icon_folder(g: &Painter, r: Rect, color: Color32) {
    let st = Stroke::new(1.6, color);
    let body = Rect::from_min_max(pos2(r.left(), r.top() + r.height() * 0.28), r.max);
    let tab = Rect::from_min_max(
        pos2(r.left(), r.top() + r.height() * 0.1),
        pos2(r.left() + r.width() * 0.45, r.top() + r.height() * 0.32),
    );
    g.rect_filled(tab, 1.0, color);
    g.rect_stroke(body, 1.5, st, egui::StrokeKind::Inside);
    g.hline(
        body.left()..=body.right(),
        body.top() + 0.8,
        Stroke::new(1.6, color),
    );
}

/// An audio file pictogram in `r`: a few waveform columns.
pub fn icon_wave(g: &Painter, r: Rect, color: Color32) {
    let heights = [0.3, 0.75, 1.0, 0.5, 0.8, 0.35];
    let pitch = r.width() / heights.len() as f32;
    for (i, h) in heights.iter().enumerate() {
        let x = r.left() + pitch * (i as f32 + 0.5);
        let half = r.height() * 0.5 * h;
        g.line_segment(
            [pos2(x, r.center().y - half), pos2(x, r.center().y + half)],
            Stroke::new(1.3, color),
        );
    }
}

/// A MIDI file pictogram in `r`: a five-pin socket.
pub fn icon_midi(g: &Painter, r: Rect, color: Color32) {
    let c = r.center();
    let rad = r.width().min(r.height()) / 2.0;
    g.circle_stroke(c, rad - 0.8, Stroke::new(1.6, color));
    for deg in [180.0f32, 225.0, 270.0, 315.0, 0.0] {
        let a = deg.to_radians();
        g.circle_filled(c + vec2(a.cos(), a.sin()) * rad * 0.55, 1.3, color);
    }
}

/// A recessed pictogram well (the track-type well of the track headers) with `paint` inside.
pub fn well(g: &Painter, r: Rect) {
    hw::recess(g, r, 3.0);
}

/// Where the pictures of these windows go (`$MANUAL_PNG_DIR`, else a temporary folder).
#[cfg(test)]
pub fn picture_path(name: &str) -> std::path::PathBuf {
    crate::spectrogram_view::hw_vocal::picture_path(name)
}

/// `text` wrapped to the width left after `inset`, in `face`; returns the rect it took.
pub fn para(ui: &mut Ui, inset: f32, text: &str, face: Face, size: f32, color: Color32) -> Rect {
    let width = (ui.available_width() - inset).max(40.0);
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat {
            font_id: theme::font(face, size),
            color,
            extra_letter_spacing: theme::tracking(face) * size,
            line_height: Some(size * 1.45),
            ..Default::default()
        },
    );
    job.wrap.max_width = width;
    let gal = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), gal.size().y), Sense::hover());
    ui.painter()
        .galley(pos2(r.left() + inset, r.top()), gal, color);
    r
}

/// An LCD panel across the window with `label` in its corner and `lines` under it, wrapped
/// (numbered on inverted chips when `numbered`).
pub fn lcd_block(ui: &mut Ui, label: &str, lines: &[String], numbered: bool) {
    let w = ui.available_width();
    let size = 10.5;
    let num_w = if numbered { 24.0 } else { 0.0 };
    let text_w = w - 16.0 - 24.0 - num_w;
    let g = ui.painter().clone();
    let gals: Vec<_> = lines
        .iter()
        .map(|l| {
            let mut job = egui::text::LayoutJob::single_section(
                l.to_uppercase(),
                egui::TextFormat {
                    font_id: theme::font(Face::Lcd, size),
                    color: theme::LCD_INK,
                    extra_letter_spacing: theme::tracking(Face::Lcd) * size,
                    line_height: Some(size * 1.4),
                    ..Default::default()
                },
            );
            job.wrap.max_width = text_w;
            g.layout_job(job)
        })
        .collect();
    let body_h: f32 = gals.iter().map(|x| x.size().y + 6.0).sum();
    let h = 5.0 + 24.0 + body_h + 8.0 + 5.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let glass = hw::lcd(&g, r);
    lcd_label(&g, glass.left_top() + vec2(10.0, 7.0), label);
    let mut y = glass.top() + 24.0;
    for (i, gal) in gals.into_iter().enumerate() {
        let x = glass.left() + 12.0;
        if numbered {
            let chip = Rect::from_min_size(pos2(x, y + 1.0), vec2(17.0, 13.0));
            g.rect_filled(chip, 1.5, theme::LCD_INK);
            hw::text(
                &g,
                chip.center(),
                Align2::CENTER_CENTER,
                &(i + 1).to_string(),
                Face::Lcd,
                9.0,
                theme::LCD,
            );
        }
        let gh = gal.size().y;
        g.galley(pos2(x + num_w, y), gal, theme::LCD_INK);
        y += gh + 6.0;
    }
}

/// A row with `label` right-aligned in silkscreen in a column `label_w` wide; returns the rect
/// for its controls (`h` tall).
pub fn row(ui: &mut Ui, label: &str, label_w: f32, h: f32) -> Rect {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h + 7.0), Sense::hover());
    let g = ui.painter();
    let label = label.trim_end_matches(':').to_uppercase();
    let size = [11.0, 10.0, 9.0]
        .into_iter()
        .find(|s| hw::silk_width(g, &label, *s) <= label_w)
        .unwrap_or(9.0);
    let shown = hw::fit(g, &label, Face::Silk, size, label_w);
    hw::text(
        g,
        pos2(r.left() + label_w, r.center().y),
        Align2::RIGHT_CENTER,
        &shown,
        Face::Silk,
        size,
        theme::INK2,
    );
    Rect::from_min_max(
        pos2(r.left() + label_w + 12.0, r.center().y - h / 2.0),
        pos2(r.right(), r.center().y + h / 2.0),
    )
}
