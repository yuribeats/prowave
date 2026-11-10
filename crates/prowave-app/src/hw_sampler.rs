//! Hardware pieces the instrument windows and the Pitch Editor share, built on `hw.rs`: keys
//! with one- or two-line legends that size themselves in a row, dark value fields for egui's
//! drag values and text fields, dropdowns, sliders, silkscreen labels and small LCD strips.
use crate::hw::{self, Cap};
use crate::theme::{self, Face};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, Id, Painter, Rect, Response, Sense, Stroke, Ui,
};

/// Height of a key in a toolbar row (one height for every key in a row).
pub const ROW_KEY_H: f32 = 28.0;
/// The gutter between keys in a row.
pub const GUTTER: f32 = 8.0;

/// Sets `label` (lines split at `\n`) centred in `body`: JetBrains Mono Bold, or the condensed
/// silkscreen face when a line would not fit.
pub fn legend(g: &Painter, body: Rect, label: &str, size: f32, color: Color32) {
    let lines: Vec<String> = label.split('\n').map(str::to_uppercase).collect();
    let n = lines.len() as f32;
    let size = if n > 1.0 { (size - 1.5).max(7.5) } else { size };
    let room = body.width() - 3.0;
    let fits = |face: Face, size: f32| {
        lines.iter().all(|l| {
            hw::galley(g, l, face, size, color).size().x - theme::tracking(face) * size <= room
        })
    };
    // The mono legend, a size or two smaller if need be, then the condensed face.
    let mut pick = None;
    for (face, s) in [
        (Face::Bold, size),
        (Face::Bold, size - 1.0),
        (Face::Bold, size - 2.0),
        (Face::Bold, size - 3.0),
        (Face::Silk, size + 0.5),
        (Face::Silk, size - 0.5),
    ] {
        if s >= 7.5 && fits(face, s) {
            pick = Some((face, s));
            break;
        }
    }
    let (face, size) = pick.unwrap_or((Face::Silk, (size - 3.0).max(7.5)));
    let line_h = size + 1.5;
    for (i, l) in lines.iter().enumerate() {
        let dy = (i as f32 - (n - 1.0) / 2.0) * line_h;
        // `fit` counts the last letter's spacing, which `fits` leaves out.
        let shown = hw::fit(g, l, face, size, room + theme::tracking(face) * size);
        hw::text(
            g,
            body.center() + vec2(0.0, dy),
            Align2::CENTER_CENTER,
            &shown,
            face,
            size,
            color,
        );
    }
}

/// A key cap at `rect`: pressed in while `latched` (or held), with an LED when `led` is
/// `Some(lit)` (at the top of a tall key, at the left of a short one) and a legend that may run
/// over two lines. Takes clicks and drags.
#[allow(clippy::too_many_arguments)]
pub fn key(
    ui: &Ui,
    g: &Painter,
    rect: Rect,
    id: Id,
    cap: Cap,
    latched: bool,
    led: Option<bool>,
    label: &str,
) -> Response {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    // Panel keys share one legend size, so a row reads evenly.
    paint_key_sized(
        g,
        rect,
        cap,
        latched || resp.is_pointer_button_down_on(),
        resp.hovered(),
        led,
        label,
        (rect.height() * 0.4).clamp(8.5, 11.0),
    );
    resp
}

/// [`key`] without the input (the face rect under the LED is returned).
pub fn paint_key(
    g: &Painter,
    rect: Rect,
    cap: Cap,
    down: bool,
    hovered: bool,
    led: Option<bool>,
    label: &str,
) -> Rect {
    let size = (rect.height() * 0.46).clamp(8.5, 12.0);
    paint_key_sized(g, rect, cap, down, hovered, led, label, size)
}

/// [`paint_key`] with the legend at `size`.
#[allow(clippy::too_many_arguments)]
pub fn paint_key_sized(
    g: &Painter,
    rect: Rect,
    cap: Cap,
    down: bool,
    hovered: bool,
    led: Option<bool>,
    label: &str,
    size: f32,
) -> Rect {
    let face = hw::paint_key(g, rect, cap, down, hovered);
    let mut body = face;
    if let Some(on) = led {
        if face.height() >= 34.0 {
            hw::led(g, pos2(face.center().x, face.top() + 8.0), on);
            body.min.y += 10.0;
        } else {
            hw::led_sized(g, pos2(face.left() + 7.5, face.center().y), 2.5, on);
            body.min.x += 10.0;
        }
    }
    if !label.is_empty() {
        legend(g, body, label, size, cap.ink());
    }
    body
}

/// Width a toolbar key needs for `label`.
fn key_width(g: &Painter, label: &str, led: bool) -> f32 {
    let w = hw::galley(g, &label.to_uppercase(), Face::Bold, 11.0, theme::INK)
        .size()
        .x;
    (w + 20.0 + if led { 12.0 } else { 0.0 }).max(44.0)
}

/// A key placed in the current row, as wide as its legend needs.
pub fn row_key(ui: &mut Ui, label: &str, cap: Cap, latched: bool, led: Option<bool>) -> Response {
    let g = ui.painter().clone();
    let w = key_width(&g, label, led.is_some());
    // Room for the key's lip under it.
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_KEY_H + 3.0), Sense::click());
    let rect = Rect::from_min_size(rect.min, vec2(w, ROW_KEY_H));
    paint_key(
        &g,
        rect,
        cap,
        latched || resp.is_pointer_button_down_on(),
        resp.hovered(),
        led,
        label,
    );
    resp
}

/// Silkscreen caps in the current row (labels beside controls).
pub fn silk(ui: &mut Ui, label: &str, size: f32, color: Color32) -> Response {
    let g = ui.painter().clone();
    let w = hw::silk_width(&g, label, size);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_KEY_H), Sense::hover());
    hw::silk(
        &g,
        rect.left_center(),
        Align2::LEFT_CENTER,
        label,
        size,
        color,
    );
    resp
}

/// A section bracket over `left..right` at `y` ([`hw::bracket`]); when the group is too narrow
/// for its label, the label is set smaller between two short drops.
pub fn bracket(g: &Painter, left: f32, right: f32, y: f32, label: &str) {
    if hw::silk_width(g, label, 11.0) + 26.0 <= right - left {
        hw::bracket(g, left, right, y, label);
        return;
    }
    let ink = theme::INK2.gamma_multiply(0.6);
    let st = Stroke::new(1.0, ink);
    g.vline(left, y..=y + 7.0, st);
    g.vline(right, y..=y + 7.0, st);
    let size = [10.0, 9.0, 8.0]
        .into_iter()
        .find(|s| hw::silk_width(g, label, *s) + 6.0 <= right - left)
        .unwrap_or(8.0);
    hw::silk(
        g,
        pos2((left + right) / 2.0, y),
        Align2::CENTER_CENTER,
        label,
        size,
        theme::INK2,
    );
}

/// Runs `add` with egui's own value widgets (drag values, text fields, combo boxes) drawn as
/// the Hardware's dark fields with bone text.
pub fn fields<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let r = CornerRadius::same(3);
        let v = ui.visuals_mut();
        v.override_text_color = Some(theme::ON_DARK);
        v.extreme_bg_color = theme::CAP;
        v.text_edit_bg_color = Some(theme::CAP);
        for (w, fill) in [
            (&mut v.widgets.inactive, theme::CAP),
            (&mut v.widgets.hovered, theme::CAP_TOP),
            (&mut v.widgets.active, theme::CAP_TOP),
            (&mut v.widgets.open, theme::CAP_TOP),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::new(1.0, theme::CAP_EDGE);
            w.fg_stroke = Stroke::new(1.0, theme::ON_DARK);
            w.corner_radius = r;
            w.expansion = 0.0;
        }
        v.selection.bg_fill = theme::ORANGE_DK;
        v.selection.stroke = Stroke::new(1.0, theme::ON_DARK);
        let s = ui.style_mut();
        s.text_styles
            .insert(egui::TextStyle::Button, theme::font(Face::Bold, 11.0));
        s.text_styles
            .insert(egui::TextStyle::Body, theme::font(Face::Bold, 11.0));
        s.spacing.interact_size.y = 22.0;
        s.spacing.button_padding = vec2(8.0, 3.0);
        add(ui)
    })
    .inner
}

/// A dark dropdown field `width` wide showing `value`; its menu holds `items`.
pub fn dropdown(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    width: f32,
    value: &str,
    items: impl FnOnce(&mut Ui),
) -> Response {
    let g = ui.painter().clone();
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_KEY_H), Sense::hover());
    let field = Rect::from_center_size(rect.center(), vec2(width, 22.0));
    let resp = hw::field(ui, &g, field, ui.make_persistent_id(id_salt), value, true);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(width);
        items(ui);
    });
    resp
}

/// A slider (black slot, bone cap) `width` long for `value` in `lo..=hi` (on a log scale when
/// `log`); returns whether it moved.
#[allow(clippy::too_many_arguments)]
pub fn slider(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    width: f32,
    value: &mut f32,
    lo: f32,
    hi: f32,
    log: bool,
) -> bool {
    let g = ui.painter().clone();
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_KEY_H), Sense::hover());
    let track = Rect::from_center_size(rect.center(), vec2(width - 4.0, 12.0));
    let to_norm = |x: f32| {
        if log {
            ((x.max(lo) / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0)
        } else {
            ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
        }
    };
    let from_norm = |n: f32| {
        if log {
            lo * (hi / lo).powf(n)
        } else {
            lo + (hi - lo) * n
        }
    };
    let id = ui.make_persistent_id(id_salt);
    match crate::widgets::slider_h(ui, &g, track, id, to_norm(*value), true) {
        Some(n) => {
            let nv = from_norm(n);
            let changed = nv != *value;
            *value = nv;
            changed
        }
        None => false,
    }
}

/// A read-only dark field showing `value` (a slider's or a knob's reading).
pub fn readout(ui: &mut Ui, value: &str, width: f32) {
    let g = ui.painter().clone();
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_KEY_H), Sense::hover());
    hw::paint_field(
        &g,
        Rect::from_center_size(rect.center(), vec2(width, 22.0)),
        value,
        false,
        false,
    );
}

/// A small type chip ("A1", "IN"): ink with bone print, or orange with ink print when `lit`.
pub fn chip(g: &Painter, left_center: egui::Pos2, label: &str, size: f32, lit: bool) -> Rect {
    let (bg, ink) = if lit {
        (theme::ORANGE, theme::INK)
    } else {
        (theme::INK, theme::ON_DARK)
    };
    let gal = hw::galley(g, label, Face::Lcd, size, ink);
    let r = Rect::from_min_size(
        pos2(left_center.x, left_center.y - (size + 5.0) / 2.0),
        vec2(gal.size().x + 8.0, size + 5.0),
    );
    g.rect_filled(r, 2.0, bg);
    g.galley(
        pos2(r.left() + 4.0, r.center().y - gal.size().y / 2.0),
        gal,
        ink,
    );
    r
}
