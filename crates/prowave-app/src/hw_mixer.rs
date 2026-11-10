//! Hardware parts shared by the Mixer group's screens (the Mixer, the Inspector, the Channel
//! Rack, the Track menu's windows and Project Settings) that `hw.rs` does not have: text with
//! its own letter spacing, names that step down before they are cut, a dark field with dim
//! print, painted icons, and the faceplate window the dialogs use.
use crate::hw::{self, Cap};
use crate::theme::{self, Face};
use egui::{epaint::Shadow, pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Sense, Stroke, Ui};
use std::sync::Arc;

/// Field text for a value that does nothing here (A.1: field text dim).
pub const FIELD_DIM: Color32 = Color32::from_rgb(0x6c, 0x67, 0x5d);

/// `text` in `face` at `size`, letter-spaced by `track` em.
pub fn galley_t(
    g: &Painter,
    text: &str,
    face: Face,
    size: f32,
    track: f32,
    color: Color32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: theme::font(face, size),
            color,
            extra_letter_spacing: track * size,
            ..Default::default()
        },
    );
    g.layout_job(job)
}

/// Width of `text` as [`text_t`] sets it.
pub fn width_t(g: &Painter, text: &str, face: Face, size: f32, track: f32) -> f32 {
    galley_t(g, text, face, size, track, theme::INK).size().x - track * size
}

/// [`galley_t`] placed by `anchor` at `pos`; returns where it went.
#[allow(clippy::too_many_arguments)]
pub fn text_t(
    g: &Painter,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    face: Face,
    size: f32,
    track: f32,
    color: Color32,
) -> Rect {
    let gal = galley_t(g, text, face, size, track, color);
    let mut sz = gal.size();
    sz.x -= track * size;
    let rect = anchor.anchor_size(pos, sz);
    g.galley(rect.min, gal, color);
    rect
}

/// Shorter forms of a name, longest first: the middle words, then the last, cut to their
/// initial ("CLASSIC ELECTRIC PIANO" -> "CLASSIC E. PIANO" -> "CLASSIC E. P.").
pub fn abbreviations(name: &str) -> Vec<String> {
    let words: Vec<&str> = name.split_whitespace().collect();
    let mut out = Vec::new();
    if words.len() < 2 {
        return out;
    }
    let mut cut: Vec<String> = words.iter().map(|w| (*w).to_string()).collect();
    let initial = |w: &str| {
        w.chars()
            .next()
            .map(|c| format!("{c}."))
            .unwrap_or_default()
    };
    for i in (1..words.len().saturating_sub(1)).rev() {
        if words[i].chars().count() > 2 {
            cut[i] = initial(words[i]);
            out.push(cut.join(" "));
        }
    }
    let last = words.len() - 1;
    if words[last].chars().count() > 2 {
        cut[last] = initial(words[last]);
        out.push(cut.join(" "));
    }
    out
}

/// How a name fits a width: the text, its size and its letter spacing.
pub struct Fitted {
    pub text: String,
    pub size: f32,
    pub track: f32,
}

/// `name` fitted into `width`: tried at each (size, tracking) step in turn, then shortened
/// word by word at the last step, and only then cut with "…".
pub fn fit_steps(g: &Painter, name: &str, face: Face, steps: &[(f32, f32)], width: f32) -> Fitted {
    for &(size, track) in steps {
        if width_t(g, name, face, size, track) <= width {
            return Fitted {
                text: name.to_string(),
                size,
                track,
            };
        }
    }
    let (size, track) = *steps.last().unwrap_or(&(12.0, 0.04));
    for short in abbreviations(name) {
        if width_t(g, &short, face, size, track) <= width {
            return Fitted {
                text: short,
                size,
                track,
            };
        }
    }
    let mut chars: Vec<char> = name.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
        if width_t(g, &s, face, size, track) <= width {
            return Fitted {
                text: s,
                size,
                track,
            };
        }
    }
    Fitted {
        text: String::new(),
        size,
        track,
    }
}

/// [`hw::paint_field`] with the value printed in `ink` (dim for a field that does nothing
/// here: the black material stays, only the print fades).
pub fn paint_field_ink(g: &Painter, rect: Rect, value: &str, stepper: bool, ink: Color32) {
    hw::paint_field(g, rect, "", stepper, false);
    let room = rect.width() - if stepper { 26.0 } else { 12.0 };
    let shown = hw::fit(g, &value.to_uppercase(), Face::Bold, 10.5, room);
    hw::text(
        g,
        pos2(rect.left() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        &shown,
        Face::Bold,
        10.5,
        ink,
    );
}

/// The label of a template such as "Sample Rate: {} Hz": the words before its colon (so a
/// row label reuses a translated text).
pub fn head(template: &str) -> &str {
    template
        .split([':', '：'])
        .next()
        .unwrap_or(template)
        .trim()
}

// ---------------------------------------------------------------------------------------------
// Icons: 2-point strokes, square ends (A.9).

/// A minus sign `s` wide at `c`.
pub fn icon_minus(g: &Painter, c: Pos2, s: f32, col: Color32) {
    g.hline(c.x - s / 2.0..=c.x + s / 2.0, c.y, Stroke::new(2.0, col));
}

/// A plus sign `s` wide at `c`.
pub fn icon_plus(g: &Painter, c: Pos2, s: f32, col: Color32) {
    icon_minus(g, c, s, col);
    g.vline(c.x, c.y - s / 2.0..=c.y + s / 2.0, Stroke::new(2.0, col));
}

/// A close cross `s` wide at `c`.
pub fn icon_x(g: &Painter, c: Pos2, s: f32, col: Color32) {
    let h = s / 2.0;
    let st = Stroke::new(2.0, col);
    g.line_segment([c + vec2(-h, -h), c + vec2(h, h)], st);
    g.line_segment([c + vec2(-h, h), c + vec2(h, -h)], st);
}

/// A bone key with a painted minus or plus (`plus`); returns whether it was clicked.
pub fn step_key(ui: &Ui, g: &Painter, rect: Rect, id: egui::Id, plus: bool, enabled: bool) -> bool {
    if !enabled {
        let face = hw::paint_key(g, rect, Cap::Bone, false, false);
        let c = face.center();
        if plus {
            icon_plus(g, c, 10.0, theme::PRINT);
        } else {
            icon_minus(g, c, 10.0, theme::PRINT);
        }
        return false;
    }
    let (resp, face) = hw::key(ui, g, rect, id, Cap::Bone, false, None, hw::Legend::Custom);
    if plus {
        icon_plus(g, face.center(), 10.0, theme::INK);
    } else {
        icon_minus(g, face.center(), 10.0, theme::INK);
    }
    resp.clicked()
}

/// A section bracket at `y` from `left` to `right` with `label` in silkscreen `size` and ink
/// (the window title's bracket; [`hw::bracket`] is the 11-point one).
pub fn bracket_title(g: &Painter, left: f32, right: f32, y: f32, label: &str, size: f32) {
    let st = Stroke::new(1.0, theme::INK2.gamma_multiply(0.6));
    g.hline(left..=left + 10.0, y, st);
    let r = text_t(
        g,
        pos2(left + 16.0, y),
        Align2::LEFT_CENTER,
        &label.to_uppercase(),
        Face::Silk,
        size,
        0.16,
        theme::INK,
    );
    if r.right() + 8.0 < right {
        g.hline(r.right() + 8.0..=right, y, st);
    }
    g.vline(left, y..=y + 7.0, st);
    g.vline(right, y..=y + 7.0, st);
}

// ---------------------------------------------------------------------------------------------
// The dialog window.

/// The header of a dialog window.
const HEADER_H: f32 = 48.0;

/// The window shadow of A.9: (0, 3), blur 5, no spread, black at 22%.
pub const WINDOW_SHADOW: Shadow = Shadow {
    offset: [0, 3],
    blur: 5,
    spread: 0,
    color: Color32::from_rgba_premultiplied(0, 0, 0, 0x38),
};

/// A dialog drawn as a faceplate: a bone panel with screws in its corners, the title on a
/// bracket in silkscreen, a close key and a seam under the header; the body is `add`'s,
/// `width` wide. Returns what `add` returned, and whether the close key was pressed.
pub fn dialog<R>(
    ctx: &egui::Context,
    id: egui::Id,
    title: &str,
    width: f32,
    add: impl FnOnce(&mut Ui) -> R,
) -> (Option<R>, bool) {
    let mut closed = false;
    let frame = egui::Frame::NONE
        .fill(Color32::from_rgb(0xe9, 0xe5, 0xdb))
        .stroke(Stroke::new(1.0, theme::SEAM_DK))
        .corner_radius(4)
        .shadow(WINDOW_SHADOW);
    let out = egui::Window::new(title)
        .id(id)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.set_width(width);
            let (hr, _) = ui.allocate_exact_size(vec2(width, HEADER_H), Sense::hover());
            let g = ui.painter().clone();
            hw::gradient(
                &g,
                hr.shrink(1.0),
                3.0,
                Color32::from_rgb(0xf1, 0xee, 0xe7),
                Color32::from_rgb(0xe6, 0xe2, 0xd8),
            );
            g.hline(
                hr.left() + 4.0..=hr.right() - 4.0,
                hr.top() + 1.5,
                Stroke::new(1.0, Color32::from_white_alpha(200)),
            );
            hw::screw(&g, hr.left_top() + vec2(12.0, 12.0), 4.5, 35.0);
            hw::screw(&g, hr.right_top() + vec2(-12.0, 12.0), 4.5, -20.0);
            let cy = hr.top() + 26.0;
            let close =
                Rect::from_center_size(pos2(hr.right() - 26.0 - 14.0, cy), vec2(28.0, 22.0));
            bracket_title(&g, hr.left() + 26.0, close.left() - 14.0, cy, title, 14.0);
            let (resp, face) = hw::key(
                ui,
                &g,
                close,
                id.with("close"),
                Cap::Bone,
                false,
                None,
                hw::Legend::Custom,
            );
            icon_x(&g, face.center(), 8.0, theme::INK);
            closed = resp.clicked();
            hw::seam_h(&g, hr.left() + 1.0..=hr.right() - 1.0, hr.bottom() - 2.0);
            let inner = egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 18,
                    right: 18,
                    top: 14,
                    bottom: 22,
                })
                .show(ui, |ui| {
                    ui.set_width(width - 36.0);
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    add(ui)
                })
                .inner;
            let all = ui.min_rect();
            hw::screw(&g, pos2(all.left() + 12.0, all.bottom() - 11.0), 4.5, 70.0);
            hw::screw(&g, pos2(all.right() - 12.0, all.bottom() - 11.0), 4.5, 10.0);
            inner
        });
    (out.and_then(|r| r.inner), closed)
}

/// A row label of a dialog: silkscreen in ink2, right-aligned in a column `w` wide (`ink` for
/// a row that does nothing here).
pub fn row_label(ui: &mut Ui, w: f32, h: f32, label: &str, ink: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut size = 11.0;
    while size > 9.0 && hw::silk_width(ui.painter(), label, size) > w - 4.0 {
        size -= 0.5;
    }
    hw::silk(
        ui.painter(),
        pos2(r.right() - 2.0, r.center().y),
        Align2::RIGHT_CENTER,
        label,
        size,
        ink,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_shorten_word_by_word_from_the_middle() {
        assert_eq!(
            abbreviations("CLASSIC ELECTRIC PIANO"),
            vec!["CLASSIC E. PIANO", "CLASSIC E. P."]
        );
        assert_eq!(abbreviations("DRUMMER"), Vec::<String>::new());
        assert_eq!(abbreviations("LEAD VOCAL"), vec!["LEAD V."]);
    }

    #[test]
    fn a_template_gives_its_label() {
        assert_eq!(head("Sample Rate: {} Hz"), "Sample Rate");
        assert_eq!(head("采样率：{} Hz"), "采样率");
        assert_eq!(
            head("Fréquence d'échantillonnage : {} Hz"),
            "Fréquence d'échantillonnage"
        );
    }
}
