//! Hardware (PW-1) parts of the Tracks area that the shared kit in `hw.rs` does not have: the
//! track header's background, small M/S/R/I keys, the header volume slider and pan knob, the
//! 24-grid toolbar glyphs, region materials (instrument, drummer, audio), the cycle band and the
//! playhead flag. Everything here only draws; the Tracks area handles the input.
//!
//! Values follow DIRECTIONS.md section A (keys 4.1, track pads 4.2, knobs 4.4, sliders 4.9,
//! regions 5).
// A kit: the Tracks area and the global tracks each use the pieces they need.
#![allow(dead_code)]
use crate::hw::{self, Cap};
use crate::theme::{self, Face, RegionLook};
use egui::{
    epaint::Mesh, pos2, vec2, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke,
    StrokeKind,
};

// ---------------------------------------------------------------------------------------------
// Materials of the arrange window (A.1).

/// The arrange floor.
pub const FLOOR: Color32 = theme::BONE3;
/// The lane behind the selected track: lit a step above the floor so it reads at a glance.
pub const FLOOR_SEL: Color32 = theme::GRID_SEL;
/// Inspector and track-list header panel.
pub const PANEL_INSPECTOR: Color32 = Color32::from_rgb(0xe4, 0xdf, 0xd4);
/// Menu rows (arrange, mixer).
pub const PANEL_MENU: Color32 = Color32::from_rgb(0xe7, 0xe3, 0xd9);
/// The slot of sliders and faders.
pub const GROOVE: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x0e);
/// Lines on the arrange floor.
pub const BAR_LINE: Color32 = Color32::from_rgba_premultiplied(4, 4, 4, 56);
pub const BEAT_LINE: Color32 = Color32::from_rgba_premultiplied(1, 1, 1, 18);
pub const LANE_LINE: Color32 = Color32::from_rgba_premultiplied(3, 3, 3, 41);
/// The cycle's tint over the lanes.
pub const CYCLE_TINT: Color32 = Color32::from_rgba_premultiplied(10, 4, 1, 10);
/// Meter segments (A.4.6).
pub const METER_LO: Color32 = Color32::from_rgb(0xee, 0xea, 0xdf);
pub const METER_MID: Color32 = Color32::from_rgb(0xff, 0xe6, 0xcf);
pub const METER_OFF: Color32 = Color32::from_rgb(0x2b, 0x29, 0x26);

// ---------------------------------------------------------------------------------------------
// Type.

/// `text` in `face` with `em` letter spacing, placed by `anchor` at `pos`; returns its rect.
#[allow(clippy::too_many_arguments)]
pub fn label(
    g: &Painter,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    face: Face,
    size: f32,
    em: f32,
    color: Color32,
) -> Rect {
    let gal = galley(g, text, face, size, em, color);
    let mut sz = gal.size();
    sz.x -= em * size;
    let rect = anchor.anchor_size(pos, sz);
    g.galley(rect.min, gal, color);
    rect
}

/// The laid-out `text` (for measuring).
pub fn galley(
    g: &Painter,
    text: &str,
    face: Face,
    size: f32,
    em: f32,
    color: Color32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: theme::font(face, size),
            color,
            extra_letter_spacing: em * size,
            ..Default::default()
        },
    );
    g.layout_job(job)
}

/// `text` cut with "…" to fit `width` when set like [`label`].
pub fn fit(g: &Painter, text: &str, face: Face, size: f32, em: f32, width: f32) -> String {
    let w = |s: &str| galley(g, s, face, size, em, theme::INK).size().x - em * size;
    if w(text) <= width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s: String = chars.iter().collect::<String>() + "…";
        if w(&s) <= width {
            return s;
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------------------------
// Fills.

/// `rect` filled left to right from `left` to `right` (square corners).
pub fn gradient_h(g: &Painter, rect: Rect, left: Color32, right: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(rect.left_top(), left);
    m.colored_vertex(rect.right_top(), right);
    m.colored_vertex(rect.right_bottom(), right);
    m.colored_vertex(rect.left_bottom(), left);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    g.add(Shape::mesh(m));
}

/// Diagonal hatch over `rect` ("/" lines `width` thick, `period` apart along the stripes'
/// normal), as CSS `repeating-linear-gradient(135deg, …)` draws it.
pub fn hatch(g: &Painter, rect: Rect, width: f32, period: f32, color: Color32) {
    let clip = g.with_clip_rect(rect.intersect(g.clip_rect()));
    let step = period * std::f32::consts::SQRT_2;
    let h = rect.height();
    let mut x = rect.left() - h - (rect.left() % step);
    let st = Stroke::new(width, color);
    while x < rect.right() + step {
        clip.line_segment([pos2(x, rect.bottom()), pos2(x + h, rect.top())], st);
        x += step;
    }
}

/// A soft drop shadow under `rect` (blur approximated by stacked translucent rects).
pub fn drop_shadow(g: &Painter, rect: Rect, radius: f32, dy: f32, alpha: u8) {
    for (grow, k) in [(1.5, 0.35), (0.5, 0.65)] {
        g.rect_filled(
            rect.translate(vec2(0.0, dy)).expand(grow),
            radius + grow,
            Color32::from_black_alpha((f32::from(alpha) * k) as u8),
        );
    }
}

/// A recessed well (track-type well, empty lanes): bone2 with a shadow cast from its top edge
/// and a faint ring, and the light line under it.
pub fn well(g: &Painter, rect: Rect, radius: f32) {
    g.rect_filled(rect.translate(vec2(0.0, 1.0)), radius, theme::BONE_HI);
    g.rect_filled(rect, radius, theme::BONE2);
    hw::inner_shadow(g, rect, radius, 0.18);
    g.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, Color32::from_black_alpha(20)),
        StrokeKind::Inside,
    );
}

/// A track header's background (4.2): bone gradient, a light top line and a seam at the
/// bottom; the selected track's is lighter, with a 4-point orange bar on its left edge.
pub fn header_bg(g: &Painter, rect: Rect, selected: bool) {
    let (top, bottom) = if selected {
        (
            Color32::from_rgb(0xf6, 0xf3, 0xec),
            Color32::from_rgb(0xed, 0xe9, 0xe0),
        )
    } else {
        (
            Color32::from_rgb(0xe8, 0xe4, 0xda),
            Color32::from_rgb(0xe0, 0xdb, 0xcf),
        )
    };
    hw::gradient(g, rect, 0.0, top, bottom);
    g.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(150)),
    );
    g.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::SEAM),
    );
    if selected {
        g.rect_filled(
            Rect::from_min_size(rect.min, vec2(4.0, rect.height())),
            0.0,
            theme::ORANGE,
        );
    }
}

/// A global track's header (Marker, Signature, Tempo): the track header's bone face with the
/// track's name in silkscreen caps.
pub fn global_header(g: &Painter, rect: Rect, name: &str) {
    header_bg(g, rect, false);
    hw::silk(
        g,
        pos2(rect.left() + 16.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        11.0,
        theme::INK2,
    );
    seam_wide_v(g, rect.right(), rect.y_range());
}

/// A global track's lane: a recessed well on the panel, under the ruler.
pub fn global_lane(g: &Painter, rect: Rect) {
    g.rect_filled(rect, 0.0, theme::BONE);
    hw::recess(g, rect.shrink2(vec2(0.0, 2.0)), 2.0);
    g.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::SEAM),
    );
}

/// The fill of a marker plate: its colour when `lit`, else the colour sunk towards the panel.
pub fn plate_fill(color: Color32, lit: bool) -> Color32 {
    if lit {
        color
    } else {
        hw::lerp(color, theme::BONE2, 0.45)
    }
}

/// A coloured plate (a marker): raised a little, with an ink ring when `lit`.
pub fn plate(g: &Painter, rect: Rect, color: Color32, lit: bool) {
    if rect.width() <= 0.0 {
        return;
    }
    drop_shadow(g, rect, 3.0, 1.5, 36);
    let fill = plate_fill(color, lit);
    hw::gradient(
        g,
        rect,
        3.0,
        hw::lerp(fill, Color32::WHITE, 0.08),
        hw::lerp(fill, Color32::BLACK, 0.06),
    );
    g.rect_stroke(
        rect,
        3.0,
        Stroke::new(
            1.0,
            if lit {
                theme::INK
            } else {
                Color32::from_rgba_unmultiplied(20, 20, 19, 120)
            },
        ),
        StrokeKind::Inside,
    );
}

/// A dark value plate (a signature block): the dark field material with bone print.
pub fn dark_plate(g: &Painter, rect: Rect) {
    if rect.width() <= 0.0 {
        return;
    }
    g.rect_filled(rect.translate(vec2(0.0, 1.0)), 3.0, theme::BONE_HI);
    hw::gradient(
        g,
        rect,
        3.0,
        Color32::from_rgb(0x25, 0x23, 0x21),
        Color32::from_rgb(0x15, 0x14, 0x13),
    );
    g.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, theme::CAP_EDGE),
        StrokeKind::Inside,
    );
}

/// The 3-point vertical seam on a panel's right edge (dark, light, dark).
pub fn seam_wide_v(g: &Painter, x: f32, y: impl Into<egui::Rangef>) {
    let y = y.into();
    g.vline(x - 2.5, y, Stroke::new(1.0, theme::SEAM));
    g.vline(x - 1.5, y, Stroke::new(1.0, theme::BONE_HI));
    g.vline(x - 0.5, y, Stroke::new(1.0, theme::SEAM));
}

/// The 3-point horizontal seam along a row's bottom edge.
pub fn seam_wide_h(g: &Painter, x: impl Into<egui::Rangef>, y: f32) {
    let x = x.into();
    g.hline(x, y - 2.5, Stroke::new(1.0, theme::SEAM));
    g.hline(x, y - 1.5, Stroke::new(1.0, theme::BONE_HI));
    g.hline(x, y - 0.5, Stroke::new(1.0, theme::SEAM_DK));
}

// ---------------------------------------------------------------------------------------------
// Keys.

/// A track's number key (4.3): a bone key (orange on the selected track) with its lip and
/// shadow at every size, the number in Mono Bold, never under 12 points.
pub fn number_pad(g: &Painter, rect: Rect, number: usize, selected: bool, hovered: bool) {
    let cap = if selected { Cap::Orange } else { Cap::Bone };
    let face = hw::paint_key(g, rect, cap, false, hovered);
    let size = (rect.height() * 0.32).clamp(12.0, 15.0);
    label(
        g,
        face.center(),
        Align2::CENTER_CENTER,
        &format!("{number:02}"),
        Face::Bold,
        size,
        0.0,
        cap.ink(),
    );
}

/// A small key (M/S/R/I, 27×19 in the header, 20×17 on a short row): the cap, latched when
/// `on`; with `led`, an LED in its own column at the left (lit when `on`) and `legend` centred
/// in the width that is left, in Mono Bold. Returns the face rect.
#[allow(clippy::too_many_arguments)]
pub fn small_key(
    g: &Painter,
    rect: Rect,
    cap: Cap,
    on: bool,
    pressed: bool,
    hovered: bool,
    led: bool,
    legend: &str,
    legend_color: Option<Color32>,
) -> Rect {
    let face = hw::paint_key(g, rect, cap, on || pressed, hovered);
    let mut c = face.center();
    if led {
        let col = (face.width() * 0.3).clamp(6.0, 9.0);
        let r = if face.width() >= 24.0 { 2.5 } else { 2.0 };
        hw::led_sized(
            g,
            pos2(face.left() + col / 2.0 + 1.5, face.center().y),
            r,
            on,
        );
        c.x = (face.left() + col + 1.5 + face.right()) / 2.0;
    }
    let size = (face.height() * 0.55).clamp(8.5, 10.5);
    label(
        g,
        c,
        Align2::CENTER_CENTER,
        legend,
        Face::Bold,
        size,
        0.04,
        legend_color.unwrap_or(cap.ink()),
    );
    face
}

// ---------------------------------------------------------------------------------------------
// Header volume slider and pan knob (4.9, 4.4).

/// The colour a meter segment at `db` lights in.
pub fn meter_colour(db: f32) -> Color32 {
    if db > 0.0 {
        theme::ORANGE
    } else if db > -6.0 {
        METER_MID
    } else {
        METER_LO
    }
}

/// A slider's meter: each channel's level (0..1 along the slider) and the dB at a point of it.
pub type SliderLevel<'a> = (&'a [f32], &'a dyn Fn(f32) -> f32);

/// A horizontal slider: a black groove `rect` (4 points tall), five ticks under it, and the
/// 12×22 black thumb at `value` (0..1). `level` lights the groove as a meter up to that point
/// (0..1, with the dB of each point from `db_at`); `dim` draws it grey (muted tracks).
pub fn slider(
    g: &Painter,
    rect: Rect,
    value: f32,
    level: Option<SliderLevel<'_>>,
    dim: bool,
) -> Rect {
    // Groove: inner shadow and the light line under it.
    g.rect_filled(rect.translate(vec2(0.0, 1.0)), 2.0, theme::BONE_HI);
    g.rect_filled(rect, 2.0, GROOVE);
    // Ticks.
    for i in 0..=4 {
        let x = rect.left() + rect.width() * i as f32 / 4.0;
        g.vline(
            x.round() + 0.5,
            rect.bottom() + 7.0..=rect.bottom() + 11.0,
            Stroke::new(1.0, theme::INK2.gamma_multiply(0.5)),
        );
    }
    // The level inside the groove: 2-point segments, 1 apart, one row per channel.
    if let Some((levels, db_at)) = level {
        let inner = rect.shrink2(vec2(2.0, 1.0));
        let n = levels.len().max(1);
        let row_h = inner.height() / n as f32;
        for (k, lv) in levels.iter().enumerate() {
            let upto = inner.left() + inner.width() * lv.clamp(0.0, 1.0);
            let y0 = inner.top() + row_h * k as f32;
            let mut x = inner.left();
            while x + 2.0 <= upto {
                let db = db_at((x - inner.left()) / inner.width());
                let c = if dim { theme::PRINT } else { meter_colour(db) };
                g.rect_filled(
                    Rect::from_min_size(pos2(x, y0), vec2(2.0, row_h.max(1.0))),
                    0.0,
                    c,
                );
                x += 3.0;
            }
        }
    }
    let span = rect.width();
    let cx = rect.left() + span * value.clamp(0.0, 1.0);
    let thumb = Rect::from_center_size(pos2(cx, rect.center().y), vec2(12.0, 22.0));
    thumb_h(g, thumb);
    thumb
}

/// A slider thumb (12×22): black, a horizontal gradient, a bone index line down its middle.
pub fn thumb_h(g: &Painter, r: Rect) {
    g.rect_filled(
        r.translate(vec2(0.0, 2.0)).expand(0.5),
        2.5,
        Color32::from_black_alpha(60),
    );
    g.rect_filled(r, 2.0, Color32::from_rgb(0x26, 0x25, 0x23));
    gradient_h(
        g,
        r.shrink(0.5),
        Color32::from_rgb(0x3a, 0x38, 0x35),
        Color32::from_rgb(0x14, 0x13, 0x12),
    );
    g.rect_stroke(
        r,
        2.0,
        Stroke::new(1.0, theme::CAP_EDGE),
        StrokeKind::Inside,
    );
    g.rect_filled(
        Rect::from_center_size(r.center(), vec2(3.0, r.height() - 6.0)),
        0.0,
        theme::BONE_HI,
    );
}

/// A black knob (4.4) of diameter `d` at `c`: drop shadow, body, knurled ring, cap and the
/// orange pointer at `value` (-1 full left … 1 full right, ±135°). `scale` prints the 11 ticks
/// and L/R around it (channel-strip pan).
pub fn knob(g: &Painter, c: Pos2, d: f32, value: f32, scale: bool) {
    let r = d / 2.0;
    if scale {
        for k in 0..=10 {
            let deg = -135.0 + 27.0 * k as f32;
            let a = (deg - 90.0f32).to_radians();
            let dir = vec2(a.cos(), a.sin());
            let major = k % 5 == 0;
            g.line_segment(
                [
                    c + dir * (r + 4.0),
                    c + dir * (r + if major { 9.5 } else { 7.0 }),
                ],
                Stroke::new(if major { 1.8 } else { 1.1 }, theme::INK2),
            );
        }
    }
    // Shadow.
    let big = d >= 40.0;
    g.circle_filled(
        c + vec2(0.0, if big { 4.0 } else { 2.0 }),
        r + 1.0,
        Color32::from_black_alpha(45),
    );
    g.circle_filled(c + vec2(0.0, 1.0), r, Color32::from_black_alpha(150));
    // Body: dark radial, lighter towards 40 % height.
    g.circle_filled(c, r, Color32::from_rgb(0x0e, 0x0e, 0x0d));
    g.circle_filled(
        c + vec2(0.0, -r * 0.08),
        r * 0.9,
        Color32::from_rgb(0x1c, 0x1b, 0x19),
    );
    g.circle_filled(
        c + vec2(0.0, -r * 0.16),
        r * 0.62,
        Color32::from_rgb(0x2c, 0x2a, 0x27),
    );
    // Knurled ring: 36 wedges on a dark band.
    let ring_out = r - (0.1 * d).max(2.5);
    let ring_in = r - (0.22 * d).max(5.0);
    if ring_out > ring_in + 0.5 {
        g.circle_filled(c, ring_out, Color32::from_rgb(0x12, 0x11, 0x10));
        for k in 0..36 {
            let a0 = (k as f32 * 10.0).to_radians();
            let a1 = (k as f32 * 10.0 + 5.0).to_radians();
            let p = |a: f32, rr: f32| c + vec2(a.cos(), a.sin()) * rr;
            g.add(Shape::convex_polygon(
                vec![
                    p(a0, ring_in),
                    p(a0, ring_out),
                    p(a1, ring_out),
                    p(a1, ring_in),
                ],
                Color32::from_rgb(0x2e, 0x2c, 0x29),
                Stroke::NONE,
            ));
        }
    }
    // Cap.
    let cap_r = ring_in.max(r * 0.45);
    g.circle_filled(c + vec2(0.0, 1.0), cap_r, Color32::from_black_alpha(140));
    g.circle_filled(c, cap_r, Color32::from_rgb(0x24, 0x22, 0x1f));
    g.circle_filled(
        c + vec2(-cap_r * 0.12, -cap_r * 0.18),
        cap_r * 0.72,
        Color32::from_rgb(0x38, 0x36, 0x32),
    );
    g.circle_filled(
        c + vec2(-cap_r * 0.2, -cap_r * 0.3),
        cap_r * 0.38,
        Color32::from_rgb(0x4d, 0x4a, 0x45),
    );
    g.circle_stroke(c, r - 0.5, Stroke::new(1.0, Color32::from_white_alpha(18)));
    // Pointer, with its glow.
    let a = (-90.0 + value.clamp(-1.0, 1.0) * 135.0f32).to_radians();
    let dir = vec2(a.cos(), a.sin());
    let start = r - (0.07 * d).max(2.0);
    let end = start - 0.38 * d;
    let (p0, p1) = (c + dir * end, c + dir * start);
    g.line_segment(
        [p0, p1],
        Stroke::new(5.0, Color32::from_rgba_unmultiplied(255, 90, 20, 70)),
    );
    g.line_segment([p0, p1], Stroke::new(3.0, theme::ORANGE));
}

// ---------------------------------------------------------------------------------------------
// Glyphs: the 24-grid toolbar icons, 2-point square-ended strokes.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Plus,
    Dup,
    Catch,
    Pointer,
    VZoom,
    HZoom,
    Eraser,
    Text,
    Scissors,
    Join,
    Mute,
    Zoom,
    Fade,
}

/// Draws `glyph` `size` points square, centred at `c`, in `color`.
pub fn glyph(g: &Painter, c: Pos2, size: f32, glyph: Glyph, color: Color32) {
    let s = size / 24.0;
    let p = |x: f32, y: f32| c + vec2((x - 12.0) * s, (y - 12.0) * s);
    let w = (2.0 * s).max(1.3);
    let st = Stroke::new(w, color);
    let line = |pts: &[(f32, f32)]| {
        g.add(Shape::line(pts.iter().map(|&(x, y)| p(x, y)).collect(), st));
    };
    let fill = |pts: &[(f32, f32)]| {
        g.add(Shape::convex_polygon(
            pts.iter().map(|&(x, y)| p(x, y)).collect(),
            color,
            Stroke::NONE,
        ));
    };
    let rect = |x: f32, y: f32, ww: f32, hh: f32| {
        g.rect_stroke(
            Rect::from_min_max(p(x, y), p(x + ww, y + hh)),
            0.0,
            st,
            StrokeKind::Middle,
        );
    };
    match glyph {
        Glyph::Plus => {
            line(&[(12.0, 5.0), (12.0, 19.0)]);
            line(&[(5.0, 12.0), (19.0, 12.0)]);
        }
        Glyph::Dup => {
            rect(4.0, 8.0, 11.0, 11.0);
            line(&[
                (9.0, 8.0),
                (9.0, 5.0),
                (20.0, 5.0),
                (20.0, 16.0),
                (15.0, 16.0),
            ]);
        }
        Glyph::Catch => {
            line(&[(4.5, 5.0), (4.5, 19.0)]);
            line(&[(8.5, 12.0), (19.5, 12.0)]);
            line(&[(15.5, 8.0), (19.5, 12.0), (15.5, 16.0)]);
        }
        Glyph::Pointer => {
            fill(&[(6.0, 3.5), (18.5, 11.7), (12.9, 13.0), (9.7, 18.6)]);
        }
        Glyph::VZoom => {
            line(&[(12.0, 3.5), (12.0, 20.5)]);
            line(&[(8.5, 7.0), (12.0, 3.5), (15.5, 7.0)]);
            line(&[(8.5, 17.0), (12.0, 20.5), (15.5, 17.0)]);
        }
        Glyph::HZoom => {
            line(&[(3.5, 12.0), (20.5, 12.0)]);
            line(&[(7.0, 8.5), (3.5, 12.0), (7.0, 15.5)]);
            line(&[(17.0, 8.5), (20.5, 12.0), (17.0, 15.5)]);
        }
        Glyph::Eraser => {
            line(&[
                (4.0, 15.0),
                (13.0, 6.0),
                (20.0, 13.0),
                (13.0, 20.0),
                (9.0, 20.0),
                (4.0, 15.0),
            ]);
            line(&[(8.5, 10.5), (15.5, 17.5)]);
            line(&[(13.0, 20.0), (20.0, 20.0)]);
        }
        Glyph::Text => {
            line(&[(5.0, 5.0), (19.0, 5.0)]);
            line(&[(12.0, 5.0), (12.0, 19.0)]);
            line(&[(9.0, 19.0), (15.0, 19.0)]);
        }
        Glyph::Scissors => {
            g.circle_stroke(p(7.0, 17.0), 3.0 * s, st);
            g.circle_stroke(p(17.0, 17.0), 3.0 * s, st);
            line(&[(9.0, 14.6), (17.5, 3.5)]);
            line(&[(15.0, 14.6), (6.5, 3.5)]);
        }
        Glyph::Join => {
            rect(3.5, 7.0, 7.0, 10.0);
            rect(13.5, 7.0, 7.0, 10.0);
            line(&[(10.5, 12.0), (13.5, 12.0)]);
        }
        Glyph::Mute => {
            rect(4.0, 6.0, 16.0, 12.0);
            line(&[(4.0, 18.0), (20.0, 6.0)]);
        }
        Glyph::Zoom => {
            g.circle_stroke(p(10.5, 10.5), 6.0 * s, st);
            line(&[(15.0, 15.0), (20.5, 20.5)]);
        }
        Glyph::Fade => {
            line(&[(4.0, 19.0), (20.0, 19.0)]);
            line(&[(4.0, 5.0), (20.0, 19.0)]);
            line(&[(4.0, 5.0), (4.0, 19.0)]);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Regions (A.5).

/// The material of a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    /// Software instrument / MIDI: black body, bone note dashes.
    Inst,
    /// Drummer: hatched bone body, ink hits.
    Drum,
    /// Audio: warm grey body, ink waveform.
    Audio,
}

impl Material {
    /// The type code on the header chip.
    pub fn code(self) -> &'static str {
        match self {
            Material::Inst => "IN",
            Material::Drum => "DR",
            Material::Audio => "AU",
        }
    }

    /// The print on the body: notes, hits, waveform.
    pub fn ink(self) -> Color32 {
        match self {
            Material::Inst => theme::BONE_HI,
            Material::Drum | Material::Audio => theme::INK,
        }
    }

    /// Bar lines inside the body.
    pub fn grid(self) -> Color32 {
        match self {
            Material::Inst => Color32::from_rgba_unmultiplied(233, 229, 219, 23),
            _ => Color32::from_rgba_unmultiplied(20, 20, 19, 33),
        }
    }

    fn header(self) -> Color32 {
        match self {
            Material::Inst => Color32::from_rgb(0x3d, 0x3b, 0x37),
            Material::Drum => theme::INK,
            Material::Audio => Color32::from_rgb(0x5c, 0x57, 0x4e),
        }
    }
}

/// How one region is drawn.
pub struct RegionPaint<'a> {
    pub material: Material,
    /// From `theme::region_look`: the body takes a hint of the track colour.
    pub look: RegionLook,
    pub selected: bool,
    /// Region muted: faded body, grey header text, a hatch across the header.
    pub muted: bool,
    /// Silent because another track is soloed: faded, no hatch.
    pub dim: bool,
    /// Being recorded.
    pub recording: bool,
    /// The track's number (1-based), printed before the name.
    pub number: usize,
    pub name: &'a str,
    /// Room kept free at the header's left (a take folder's triangle).
    pub lead: f32,
}

/// Height of a region's header bar for a region `h` tall.
pub fn region_header_h(h: f32) -> f32 {
    if h >= 50.0 {
        15.0
    } else if h >= 30.0 {
        11.0
    } else {
        (h * 0.35).clamp(6.0, 10.0)
    }
}

/// Paints a region in `rect` (body, header with type chip and "NN NAME", outline, drop
/// shadow). Returns the body under the header, for the notes, hits or waveform.
pub fn region(g: &Painter, rect: Rect, rp: &RegionPaint<'_>) -> Rect {
    let radius = 3.0;
    let faded = rp.muted || rp.dim;
    drop_shadow(g, rect, radius, 2.0, 46);
    // Body.
    let body_c = rp.look.body;
    let (top, bottom) = match rp.material {
        Material::Inst => (
            hw::lerp(body_c, Color32::WHITE, 0.03),
            hw::lerp(body_c, Color32::BLACK, 0.35),
        ),
        Material::Drum => (
            Color32::from_rgb(0xec, 0xe7, 0xdd),
            Color32::from_rgb(0xdd, 0xd7, 0xca),
        ),
        Material::Audio => (
            hw::lerp(body_c, Color32::WHITE, 0.06),
            hw::lerp(body_c, Color32::BLACK, 0.07),
        ),
    };
    let (top, bottom) = if rp.recording {
        (
            Color32::from_rgb(0x2a, 0x1a, 0x14),
            Color32::from_rgb(0x16, 0x0e, 0x0b),
        )
    } else {
        (top, bottom)
    };
    if faded {
        g.rect_filled(rect, radius, FLOOR);
        hw::gradient(
            g,
            rect,
            radius,
            top.gamma_multiply(0.35),
            bottom.gamma_multiply(0.35),
        );
    } else {
        hw::gradient(g, rect, radius, top, bottom);
    }
    // The Drummer hatch, on regions tall enough for it to read as a material.
    if rp.material == Material::Drum && rect.height() >= 44.0 {
        let a = if faded { 0.05 } else { 0.12 };
        hatch(
            g,
            rect,
            2.0,
            7.0,
            Color32::from_rgba_unmultiplied(20, 20, 19, (a * 255.0) as u8),
        );
    }
    // Header bar.
    let hh = region_header_h(rect.height());
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), hh));
    // A selected region that does not sound keeps its faded header; only its outline says it
    // is selected.
    let head_c = if rp.recording {
        theme::DANGER
    } else if rp.selected && !faded {
        theme::ORANGE
    } else {
        rp.material.header()
    };
    // A faded region's header is a flat bone bar: its print stays readable.
    let head_c = if faded && !rp.recording {
        theme::BONE3
    } else {
        head_c
    };
    let cr = CornerRadius {
        nw: radius as u8,
        ne: radius as u8,
        sw: 0,
        se: 0,
    };
    g.rect_filled(head, cr, head_c);
    if rp.muted {
        hatch(g, head, 2.0, 6.0, theme::INK.gamma_multiply(0.22));
    }
    let lit = (rp.selected && !faded) || rp.recording;
    let text_c = if lit {
        theme::INK
    } else if faded {
        theme::INK2
    } else {
        theme::BONE
    };
    let clip = g.with_clip_rect(head.shrink2(vec2(1.0, 0.0)).intersect(g.clip_rect()));
    let mut x = head.left() + 5.0 + rp.lead;
    if hh >= 11.0 && rect.width() > 34.0 + rp.lead {
        // Type chip: bone with ink print; inverted on a selected region.
        let (chip_bg, chip_fg) = if lit {
            (theme::INK, theme::ORANGE)
        } else if faded {
            (theme::PRINT, theme::BONE2)
        } else {
            (theme::BONE, theme::INK)
        };
        let code = if rp.recording {
            "REC"
        } else {
            rp.material.code()
        };
        let gal = galley(&clip, code, Face::Lcd, 9.5, 0.1, chip_fg);
        let chip = Rect::from_min_size(
            pos2(x, head.center().y - 5.5),
            vec2(gal.size().x - 0.95 + 8.0, 11.0),
        );
        clip.rect_filled(chip, 1.5, chip_bg);
        clip.galley(
            pos2(chip.left() + 4.0, chip.center().y - gal.size().y / 2.0),
            gal,
            chip_fg,
        );
        x = chip.right() + 8.0;
    }
    // A muted region says so at the right of its header.
    let mut name_end = head.right() - 4.0;
    if rp.muted && hh >= 11.0 && rect.width() > 110.0 {
        let gal = galley(&clip, "MUTED", Face::Lcd, 9.0, 0.1, theme::BONE);
        let chip = Rect::from_min_size(
            pos2(head.right() - 8.0 - gal.size().x, head.center().y - 5.5),
            vec2(gal.size().x + 4.0, 11.0),
        );
        clip.rect_filled(chip, 1.5, theme::INK2);
        clip.galley(
            pos2(chip.left() + 2.5, chip.center().y - gal.size().y / 2.0),
            gal,
            theme::BONE,
        );
        name_end = chip.left() - 6.0;
    }
    if hh >= 8.0 {
        let size = if hh >= 15.0 {
            12.0
        } else {
            (hh * 0.78).max(6.5)
        };
        let name = format!("{:02}  {}", rp.number, rp.name.to_uppercase());
        let name = fit(
            &clip,
            &name,
            Face::Silk,
            size,
            0.12,
            (name_end - x).max(0.0),
        );
        label(
            &clip,
            pos2(x, head.center().y),
            Align2::LEFT_CENTER,
            &name,
            Face::Silk,
            size,
            0.12,
            text_c,
        );
    }
    // Outline: 1 point of ink at 75 %; 2 points of ink outside a selected region, orange
    // when the selected region is silent (muted, or another track soloed).
    if rp.selected {
        g.rect_stroke(
            rect,
            radius,
            Stroke::new(2.0, if faded { theme::ORANGE } else { theme::INK }),
            StrokeKind::Outside,
        );
    } else {
        g.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(20, 20, 19, 191)),
            StrokeKind::Inside,
        );
    }
    Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max)
}

/// A disclosure triangle at `c`: pointing down when `open`, right when closed.
pub fn disclosure(g: &Painter, c: Pos2, open: bool, color: Color32) {
    let pts = if open {
        vec![
            c + vec2(-4.0, -2.5),
            c + vec2(4.0, -2.5),
            c + vec2(0.0, 3.5),
        ]
    } else {
        vec![
            c + vec2(-2.5, -4.0),
            c + vec2(-2.5, 4.0),
            c + vec2(3.5, 0.0),
        ]
    };
    g.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

// ---------------------------------------------------------------------------------------------
// Ruler: cycle band and playhead.

/// The cycle band in the ruler (A.5): solid orange (no hatch) with a 2-point ink line at the
/// left locator, the range in Mono ExtraBold. Off, it is a plain recessed bone band with the
/// range in grey print.
pub fn cycle_band(g: &Painter, band: Rect, on: bool, text: &str) {
    if band.width() <= 0.0 {
        return;
    }
    if on {
        g.rect_filled(band, 0.0, theme::ORANGE);
        g.hline(
            band.x_range(),
            band.bottom() - 0.5,
            Stroke::new(1.0, Color32::from_black_alpha(64)),
        );
    } else {
        g.rect_filled(band, 0.0, theme::BONE2);
        hw::inner_shadow(g, band, 0.0, 0.15);
    }
    // "CYCLE" and the range: ink on the orange; off, ink on a lit bone chip so it still reads
    // over the recessed band.
    let text = format!("CYCLE  {text}");
    let gal = galley(g, &text, Face::Lcd, 10.0, 0.08, theme::INK);
    let w = gal.size().x + 12.0;
    if w + 4.0 < band.width() {
        let plate = Rect::from_min_size(
            pos2(band.left() + 6.0, band.center().y - 6.0),
            vec2(w, 12.0),
        );
        let fg = if on {
            g.rect_filled(plate, 0.0, theme::ORANGE);
            theme::INK
        } else {
            g.rect_filled(plate, 1.5, theme::BONE_HI);
            g.rect_stroke(
                plate,
                1.5,
                Stroke::new(1.0, theme::SEAM),
                StrokeKind::Inside,
            );
            theme::INK2
        };
        label(
            g,
            pos2(plate.left() + 6.0, plate.center().y),
            Align2::LEFT_CENTER,
            &text,
            Face::Lcd,
            10.0,
            0.08,
            fg,
        );
    }
}

/// The playhead's line from `top` to `bottom` at `x`: 2 points of `color` with a thin ink edge.
pub fn playhead_line(g: &Painter, x: f32, top: f32, bottom: f32, color: Color32) {
    g.vline(
        x,
        top..=bottom,
        Stroke::new(3.0, Color32::from_rgba_unmultiplied(20, 20, 19, 115)),
    );
    g.vline(x, top..=bottom, Stroke::new(2.0, color));
}

/// The playhead's flag in the ruler: a 16×21 pentagon pointing down, `color` with an ink
/// outline, its top at `top`.
pub fn playhead_flag(g: &Painter, x: f32, top: f32, color: Color32) {
    let pts = vec![
        pos2(x - 7.5, top),
        pos2(x + 7.5, top),
        pos2(x + 7.5, top + 12.0),
        pos2(x, top + 20.5),
        pos2(x - 7.5, top + 12.0),
    ];
    g.add(Shape::convex_polygon(
        pts.iter().map(|p| *p + vec2(0.0, 1.5)).collect(),
        Color32::from_black_alpha(50),
        Stroke::NONE,
    ));
    g.add(Shape::convex_polygon(
        pts,
        color,
        Stroke::new(1.0, theme::INK),
    ));
}

/// A "bar beat division tick" position as the Hardware LCD sets it: "003 1 1 001".
pub fn bbdt_text((bar, beat, div, tick): (u32, u32, u32, u32)) -> String {
    format!("{bar:03} {beat} {div} {tick:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_set_like_the_lcd() {
        assert_eq!(bbdt_text((3, 1, 1, 1)), "003 1 1 001");
        assert_eq!(bbdt_text((123, 4, 2, 240)), "123 4 2 240");
    }

    #[test]
    fn meter_colours_follow_the_db_thresholds() {
        assert_eq!(meter_colour(-20.0), METER_LO);
        assert_eq!(meter_colour(-3.0), METER_MID);
        assert_eq!(meter_colour(1.0), theme::ORANGE);
    }

    #[test]
    fn materials_have_their_codes() {
        assert_eq!(Material::Inst.code(), "IN");
        assert_eq!(Material::Drum.code(), "DR");
        assert_eq!(Material::Audio.code(), "AU");
    }
}
