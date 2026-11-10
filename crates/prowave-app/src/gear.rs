//! PW-1 hardware parts for the effect and plug-in windows: a bone chassis, black and one
//! signal orange. Keycaps with LEDs, knurled knobs with printed tick scales, LCD glass with
//! drawn seven-segment digits, black meter wells and LED ladders, all painted here so every
//! effect window reads as a piece of gear. Values follow DIRECTIONS.md (direction A).
use crate::hw;
use crate::theme::{self, Face};
use egui::epaint::{Mesh, Shadow};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Painter, Pos2, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use std::sync::Arc;

// ---------------------------------------------------------------- palette (theme tokens)

pub use crate::theme::{BONE, BONE2, BONE_HI, INK, INK2, LCD, LCD_INK, ORANGE, PRINT, SEAM_DK};

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn lerp_c(a: Color32, b: Color32, t: f32) -> Color32 {
    hw::lerp(a, b, t)
}

// ---------------------------------------------------------------- type (theme faces)

/// Silkscreen.
pub fn silk_font(size: f32) -> FontId {
    theme::font(Face::Silk, size)
}
/// Big silkscreen: model names.
pub fn silk_heavy(size: f32) -> FontId {
    theme::font(Face::SilkX, size)
}
/// Legends, values.
pub fn mono_font(size: f32) -> FontId {
    theme::font(Face::Bold, size)
}
/// LCD text.
pub fn mono_heavy(size: f32) -> FontId {
    theme::font(Face::Lcd, size)
}

/// `text` laid out with `spacing` em of letter spacing.
pub fn galley(
    p: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    spacing: f32,
) -> Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    let size = font.size;
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font,
            color,
            extra_letter_spacing: spacing * size,
            ..Default::default()
        },
    );
    p.layout_job(job)
}

/// Draws `text` anchored at `pos`; returns where it went.
pub fn text_at(
    p: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
    spacing: f32,
) -> Rect {
    let g = galley(p, text, font, color, spacing);
    // Letter spacing trails the last glyph; leave it out of the anchoring.
    let size = vec2(
        (g.size().x
            - spacing
                * g.job
                    .sections
                    .first()
                    .map_or(0.0, |s| s.format.font_id.size))
        .max(0.0),
        g.size().y,
    );
    let r = align.anchor_size(pos, size);
    p.galley(r.min, g, color);
    r
}

/// Silkscreen print: condensed caps with wide tracking.
pub fn silk(p: &Painter, pos: Pos2, align: Align2, text: &str, size: f32, color: Color32) -> Rect {
    text_at(
        p,
        pos,
        align,
        &text.to_uppercase(),
        silk_font(size),
        color,
        0.15,
    )
}

/// A legend as a key prints it: caps, except note names ("Eb", "C#") which keep their case.
fn legend_case(text: &str) -> String {
    let mut c = text.chars();
    let note = text.chars().count() <= 2
        && c.next().is_some_and(|f| ('A'..='G').contains(&f))
        && c.next().is_none_or(|s| s == 'b' || s == '#');
    if note {
        text.to_string()
    } else {
        text.to_uppercase()
    }
}

/// Mono legend in caps.
pub fn legend(
    p: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    size: f32,
    color: Color32,
) -> Rect {
    text_at(
        p,
        pos,
        align,
        &legend_case(text),
        mono_font(size),
        color,
        0.04,
    )
}

/// Width `text` takes as a legend.
pub fn legend_width(p: &Painter, text: &str, size: f32) -> f32 {
    galley(p, &legend_case(text), mono_font(size), INK, 0.04)
        .size()
        .x
}

/// `text` cut to `width` with a trailing dot run when it does not fit.
pub fn fit(p: &Painter, text: &str, size: f32, width: f32) -> String {
    if legend_width(p, text, size) <= width {
        return text.to_string();
    }
    let mut s: Vec<char> = text.chars().collect();
    while !s.is_empty() {
        s.pop();
        let t: String = s.iter().collect::<String>() + "…";
        if legend_width(p, &t, size) <= width {
            return t;
        }
    }
    String::new()
}

// ---------------------------------------------------------------- fills

/// The outline of `rect` with corners of radius `r`.
pub fn rrect_points(rect: Rect, r: f32) -> Vec<Pos2> {
    let r = r.min(rect.width() / 2.0).min(rect.height() / 2.0).max(0.0);
    if r < 0.5 {
        return vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
        ];
    }
    let steps = ((r * 0.8).ceil() as usize).clamp(3, 10);
    let mut out = Vec::with_capacity(steps * 4 + 4);
    let corners = [
        (pos2(rect.right() - r, rect.top() + r), -90.0f32),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), 90.0),
        (pos2(rect.left() + r, rect.top() + r), 180.0),
    ];
    for (c, start) in corners {
        for i in 0..=steps {
            let a = (start + 90.0 * i as f32 / steps as f32).to_radians();
            out.push(c + vec2(a.cos(), a.sin()) * r);
        }
    }
    out
}

/// Clips the convex polygon `poly` to `y0 <= y <= y1`.
fn clip_band(poly: &[Pos2], y0: f32, y1: f32) -> Vec<Pos2> {
    let clip = |pts: &[Pos2], keep: &dyn Fn(Pos2) -> bool, edge: f32| -> Vec<Pos2> {
        let mut out = Vec::with_capacity(pts.len() + 2);
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            let (ka, kb) = (keep(a), keep(b));
            if ka {
                out.push(a);
            }
            if ka != kb {
                let t = (edge - a.y) / (b.y - a.y);
                out.push(pos2(a.x + (b.x - a.x) * t, edge));
            }
        }
        out
    };
    let p = clip(poly, &|q: Pos2| q.y >= y0, y0);
    if p.len() < 3 {
        return p;
    }
    clip(&p, &|q: Pos2| q.y <= y1, y1)
}

/// Fills the convex polygon `poly` with a vertical gradient through `stops` (position 0..1
/// down `poly`'s height, colour).
pub fn fill_grad(p: &Painter, poly: &[Pos2], stops: &[(f32, Color32)]) {
    if poly.len() < 3 || stops.is_empty() {
        return;
    }
    let (top, bot) = poly
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.y), b.max(q.y)));
    let h = (bot - top).max(1e-3);
    let mut mesh = Mesh::default();
    let mut bands: Vec<(f32, Color32)> = stops.to_vec();
    if bands[0].0 > 0.0 {
        bands.insert(0, (0.0, bands[0].1));
    }
    if bands[bands.len() - 1].0 < 1.0 {
        bands.push((1.0, bands[bands.len() - 1].1));
    }
    for w in bands.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t1 <= t0 {
            continue;
        }
        let (y0, y1) = (top + t0 * h, top + t1 * h);
        let part = clip_band(poly, y0, y1);
        if part.len() < 3 {
            continue;
        }
        let base = mesh.vertices.len() as u32;
        for q in &part {
            mesh.colored_vertex(*q, lerp_c(c0, c1, (q.y - y0) / (y1 - y0)));
        }
        for i in 1..part.len() as u32 - 1 {
            mesh.add_triangle(base, base + i, base + i + 1);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// A rounded rect with a vertical gradient.
pub fn grad_rect(p: &Painter, rect: Rect, r: f32, stops: &[(f32, Color32)]) {
    fill_grad(p, &rrect_points(rect, r), stops);
}

/// A disc shaded from `inner` at `focus` (offset as a fraction of `r`) through `mid` at
/// `mid_at` to `outer` at the rim.
pub fn radial(p: &Painter, c: Pos2, r: f32, focus: Vec2, stops: &[(f32, Color32)]) {
    let n = ((r * 1.4) as usize).clamp(24, 72);
    let f = c + focus * r;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(f, stops[0].1);
    let rings: Vec<(f32, Color32)> = stops.iter().copied().filter(|s| s.0 > 0.0).collect();
    for (ri, (t, col)) in rings.iter().enumerate() {
        for i in 0..n {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            let rim = c + vec2(a.cos(), a.sin()) * r;
            mesh.colored_vertex(f + (rim - f) * *t, *col);
        }
        let base = 1 + (ri * n) as u32;
        for i in 0..n as u32 {
            let j = (i + 1) % n as u32;
            if ri == 0 {
                mesh.add_triangle(0, base + i, base + j);
            } else {
                let prev = base - n as u32;
                mesh.add_triangle(prev + i, base + i, base + j);
                mesh.add_triangle(prev + i, base + j, prev + j);
            }
        }
    }
    p.add(Shape::mesh(mesh));
}

/// A soft drop shadow under `rect`.
pub fn drop_shadow(p: &Painter, rect: Rect, r: f32, dy: i8, blur: u8, a: u8) {
    let s = Shadow {
        offset: [0, dy],
        blur,
        spread: 0,
        color: Color32::from_black_alpha(a),
    };
    p.add(s.as_shape(rect, CornerRadius::same(r.round() as u8)));
}

/// An inset shadow along the top inside `rect` (wells, glass, recesses).
pub fn inner_shadow(p: &Painter, rect: Rect, r: f32, strength: f32) {
    for i in 0..4 {
        let k = i as f32;
        p.rect_stroke(
            Rect::from_min_max(rect.min + vec2(0.0, k * 0.8), rect.max),
            r,
            Stroke::new(
                1.0,
                Color32::from_black_alpha((strength * (60.0 - k * 14.0)) as u8),
            ),
            StrokeKind::Inside,
        );
    }
}

// ---------------------------------------------------------------- chassis

/// A slotted screw head.
pub fn screw(p: &Painter, c: Pos2, angle_deg: f32) {
    hw::screw(p, c, 5.5, angle_deg);
}

/// A horizontal panel seam.
pub fn seam_h(p: &Painter, x0: f32, x1: f32, y: f32) {
    hw::seam_h(p, x0..=x1, y);
}

/// A vertical panel seam.
pub fn seam_v(p: &Painter, x: f32, y0: f32, y1: f32) {
    hw::seam_v(p, x, y0..=y1);
}

/// A section bracket over `width` from `left`.
pub fn bracket(p: &Painter, left: Pos2, width: f32, label: &str) {
    hw::bracket(p, left.x, left.x + width, left.y, label);
}

/// A black meter / display well.
pub fn well(p: &Painter, rect: Rect, r: f32) {
    p.line_segment(
        [
            pos2(rect.left() + r, rect.bottom() + 0.5),
            pos2(rect.right() - r, rect.bottom() + 0.5),
        ],
        Stroke::new(1.0, BONE_HI),
    );
    grad_rect(
        p,
        rect,
        r,
        &[(0.0, rgb(0x15, 0x14, 0x12)), (1.0, rgb(0x22, 0x21, 0x1E))],
    );
    inner_shadow(p, rect, r, 1.0);
    p.rect_stroke(
        rect,
        r,
        Stroke::new(1.0, rgb(0x0B, 0x0B, 0x0A)),
        StrokeKind::Inside,
    );
}

/// A black name plate with a badge (e.g. the track number) and a name.
pub fn name_plate(p: &Painter, rect: Rect, badge: &str, name: &str, orange: bool) {
    grad_rect(
        p,
        rect,
        3.0,
        &[(0.0, rgb(0x2D, 0x2B, 0x28)), (1.0, rgb(0x19, 0x18, 0x17))],
    );
    p.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, rgb(0x0B, 0x0B, 0x0A)),
        StrokeKind::Outside,
    );
    let mut x = rect.left() + 6.0;
    if !badge.is_empty() {
        let bw = legend_width(p, badge, 10.0) + 8.0;
        let b = Rect::from_min_size(pos2(x, rect.center().y - 7.0), vec2(bw, 14.0));
        p.rect_filled(b, 2.0, if orange { ORANGE } else { BONE });
        legend(p, b.center(), Align2::CENTER_CENTER, badge, 10.0, INK);
        x = b.right() + 7.0;
    }
    let shown = fit(p, name, 11.0, rect.right() - x - 6.0);
    legend(
        p,
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        &shown,
        11.0,
        BONE,
    );
    if orange {
        p.line_segment(
            [
                pos2(rect.left(), rect.bottom() + 1.5),
                pos2(rect.right(), rect.bottom() + 1.5),
            ],
            Stroke::new(2.0, ORANGE),
        );
    }
}

// ---------------------------------------------------------------- LEDs and keys

/// An LED: orange with a glow when on, warm grey when off.
pub fn led(p: &Painter, c: Pos2, r: f32, on: bool) {
    hw::led_sized(p, c, r, on);
}

pub use crate::hw::Cap;

/// Paints a keycap; `down` latches it 1 px lower. Returns the face.
pub fn paint_key(p: &Painter, rect: Rect, cap: Cap, down: bool) -> Rect {
    hw::paint_key(p, rect, cap, down, false)
}

/// Where a key's LED sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LedAt {
    None,
    Top,
    Left,
}

/// A clickable keycap with a legend and an optional LED (`lit`). `down` draws it latched.
#[allow(clippy::too_many_arguments)]
pub fn key(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    legend_text: &str,
    cap: Cap,
    led_at: LedAt,
    lit: bool,
    down: bool,
) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let pressed = resp.is_pointer_button_down_on();
    let p = ui.painter();
    let face = paint_key(p, rect, cap, down || pressed);
    let size = if rect.height() < 22.0 { 10.0 } else { 11.0 };
    match led_at {
        LedAt::None => {
            let t = fit(p, legend_text, size, face.width() - 8.0);
            legend(p, face.center(), Align2::CENTER_CENTER, &t, size, cap.ink());
        }
        LedAt::Left => {
            led(p, pos2(face.left() + 10.0, face.center().y), 3.0, lit);
            let t = fit(p, legend_text, size, face.width() - 26.0);
            legend(
                p,
                pos2(face.center().x + 7.0, face.center().y),
                Align2::CENTER_CENTER,
                &t,
                size,
                cap.ink(),
            );
        }
        LedAt::Top => {
            led(p, pos2(face.center().x, face.top() + 8.0), 3.0, lit);
            let t = fit(p, legend_text, size, face.width() - 8.0);
            legend(
                p,
                pos2(face.center().x, face.center().y + 6.0),
                Align2::CENTER_CENTER,
                &t,
                size,
                cap.ink(),
            );
        }
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// A key with a drawn glyph instead of a legend.
pub fn glyph_key(ui: &mut Ui, rect: Rect, id: Id, cap: Cap, down: bool, glyph: Glyph) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let pressed = resp.is_pointer_button_down_on();
    let p = ui.painter();
    let face = paint_key(p, rect, cap, down || pressed);
    draw_glyph(
        p,
        face.center(),
        face.height().min(face.width()) * 0.42,
        glyph,
        cap.ink(),
    );
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Icons on the 24 grid, 2 px square strokes (the set every unit draws from).
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Glyph {
    Close,
    Power,
    Left,
    Right,
    Window,
    Knobs,
    Copy,
    Paste,
    Undo,
    Redo,
}

pub fn draw_glyph(p: &Painter, c: Pos2, s: f32, g: Glyph, col: Color32) {
    let k = s / 12.0;
    let st = Stroke::new(2.0 * k.clamp(0.7, 1.0), col);
    let at = |x: f32, y: f32| c + vec2((x - 12.0) * k, (y - 12.0) * k);
    match g {
        Glyph::Close => {
            p.line_segment([at(6.5, 6.5), at(17.5, 17.5)], st);
            p.line_segment([at(17.5, 6.5), at(6.5, 17.5)], st);
        }
        Glyph::Power => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = (40.0 + 280.0 * i as f32 / 20.0f32).to_radians();
                    at(12.0 + 7.5 * a.sin(), 13.0 - 7.5 * a.cos())
                })
                .collect();
            p.add(Shape::line(pts, st));
            p.line_segment([at(12.0, 3.0), at(12.0, 12.0)], st);
        }
        Glyph::Left => {
            p.add(Shape::convex_polygon(
                vec![at(15.0, 6.0), at(15.0, 18.0), at(7.0, 12.0)],
                col,
                Stroke::NONE,
            ));
        }
        Glyph::Right => {
            p.add(Shape::convex_polygon(
                vec![at(9.0, 6.0), at(17.0, 12.0), at(9.0, 18.0)],
                col,
                Stroke::NONE,
            ));
        }
        Glyph::Window => {
            p.rect_stroke(
                Rect::from_two_pos(at(3.5, 5.0), at(20.5, 19.0)),
                0.0,
                st,
                StrokeKind::Middle,
            );
            p.line_segment([at(3.5, 9.0), at(20.5, 9.0)], st);
        }
        Glyph::Knobs => {
            for (x, y) in [(7.0, 8.0), (17.0, 8.0), (7.0, 17.0), (17.0, 17.0)] {
                p.circle_stroke(at(x, y), 3.3 * k, st);
            }
        }
        Glyph::Copy => {
            p.rect_stroke(
                Rect::from_two_pos(at(4.0, 8.0), at(15.0, 19.0)),
                0.0,
                st,
                StrokeKind::Middle,
            );
            p.add(Shape::line(
                vec![
                    at(9.0, 8.0),
                    at(9.0, 5.0),
                    at(20.0, 5.0),
                    at(20.0, 16.0),
                    at(15.0, 16.0),
                ],
                st,
            ));
        }
        Glyph::Paste => {
            p.rect_stroke(
                Rect::from_two_pos(at(5.0, 5.0), at(19.0, 20.0)),
                0.0,
                st,
                StrokeKind::Middle,
            );
            p.rect_filled(Rect::from_two_pos(at(9.0, 3.5), at(15.0, 7.0)), 0.0, col);
        }
        Glyph::Undo | Glyph::Redo => {
            // A hook: an arc over the top with an arrow head at its start.
            let flip = if g == Glyph::Redo { -1.0 } else { 1.0 };
            let ax = |x: f32| 12.0 + (x - 12.0) * flip;
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = (-90.0 + 200.0 * i as f32 / 12.0f32).to_radians();
                    at(ax(13.0 + 6.0 * a.cos()), 13.0 + 6.0 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, st));
            p.add(Shape::convex_polygon(
                vec![at(ax(13.0), 3.5), at(ax(13.0), 10.5), at(ax(8.0), 7.0)],
                col,
                Stroke::NONE,
            ));
        }
    }
}

// ---------------------------------------------------------------- knobs

/// What a knob did this frame.
pub struct KnobOut {
    pub resp: Response,
    /// The new position (0..1) while it is turned or after a double-click.
    pub value: Option<f32>,
    /// The gesture ended this frame.
    pub ended: bool,
}

/// Paints a knob at `c` of diameter `d`, turned to `norm` (0..1 over 270°), with the
/// printed 11-tick scale when `scale`.
pub fn paint_knob(p: &Painter, c: Pos2, d: f32, norm: f32, scale: bool, dim: bool) {
    let r = d / 2.0;
    if scale {
        for i in 0..=10 {
            let a = (-135.0 + i as f32 * 27.0f32).to_radians();
            let dir = vec2(a.sin(), -a.cos());
            let major = i % 5 == 0;
            p.line_segment(
                [
                    c + dir * (r + 4.0),
                    c + dir * (r + if major { 9.5 } else { 7.0 }),
                ],
                Stroke::new(if major { 1.8 } else { 1.1 }, INK2),
            );
        }
    }
    drop_shadow(
        p,
        Rect::from_center_size(c, vec2(d, d)),
        r,
        if d > 40.0 { 4 } else { 2 },
        if d > 40.0 { 7 } else { 4 },
        102,
    );
    radial(
        p,
        c,
        r,
        vec2(0.0, -0.2),
        &[
            (0.0, rgb(0x3A, 0x38, 0x35)),
            (0.62, rgb(0x1C, 0x1B, 0x19)),
            (1.0, rgb(0x0E, 0x0E, 0x0D)),
        ],
    );
    // The knurled ring: 72 alternating wedges.
    let kr = r - (d * 0.1).max(2.5);
    let n = 72;
    let mut mesh = Mesh::default();
    for i in 0..n {
        let a0 = std::f32::consts::TAU * i as f32 / n as f32;
        let a1 = std::f32::consts::TAU * (i + 1) as f32 / n as f32;
        let col = if i % 2 == 0 {
            rgb(0x2E, 0x2C, 0x29)
        } else {
            rgb(0x12, 0x11, 0x10)
        };
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(c, col);
        mesh.colored_vertex(c + vec2(a0.cos(), a0.sin()) * kr, col);
        mesh.colored_vertex(c + vec2(a1.cos(), a1.sin()) * kr, col);
        mesh.add_triangle(base, base + 1, base + 2);
    }
    p.add(Shape::mesh(mesh));
    p.circle_stroke(c, kr, Stroke::new(1.0, Color32::from_black_alpha(120)));
    // The domed cap.
    let cr = r - (d * 0.22).max(5.0);
    p.circle_filled(c + vec2(0.0, 1.0), cr, Color32::from_black_alpha(150));
    radial(
        p,
        c,
        cr,
        vec2(-0.1, -0.3),
        &[(0.0, rgb(0x4D, 0x4A, 0x45)), (1.0, rgb(0x24, 0x22, 0x1F))],
    );
    p.circle_stroke(c, cr - 0.5, Stroke::new(0.8, Color32::from_white_alpha(18)));
    // The pointer.
    let a = (-135.0 + norm.clamp(0.0, 1.0) * 270.0f32).to_radians();
    let dir = vec2(a.sin(), -a.cos());
    let ind = (d * 0.07).max(2.0);
    let tip = c + dir * (r - ind);
    let tail = c + dir * (r - ind - d * 0.38);
    let col = if dim { alpha(ORANGE, 0.45) } else { ORANGE };
    p.line_segment([tail, tip], Stroke::new(5.0, alpha(ORANGE, 0.18)));
    p.line_segment([tail, tip], Stroke::new(3.0, col));
}

/// A knob to turn: vertical drag (Shift for fine), a double-click goes to `default`.
#[allow(clippy::too_many_arguments)]
pub fn knob(
    ui: &mut Ui,
    id: Id,
    c: Pos2,
    d: f32,
    norm: f32,
    default: f32,
    scale: bool,
    enabled: bool,
) -> KnobOut {
    let hit = Rect::from_center_size(c, vec2(d + 12.0, d + 12.0));
    let sense = if enabled {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(hit, id, sense);
    let mut value = None;
    let mut ended = false;
    if enabled {
        if resp.dragged() {
            let fine = ui.input(|i| i.modifiers.shift);
            let dy = resp.drag_delta().y - resp.drag_delta().x * 0.25;
            // The position while dragging lives in memory so tiny moves accumulate.
            let start: f32 = ui.data(|m| m.get_temp(id.with("knob_pos"))).unwrap_or(norm);
            let start = if resp.drag_started() { norm } else { start };
            let n = (start - dy / if fine { 900.0 } else { 220.0 }).clamp(0.0, 1.0);
            ui.data_mut(|m| m.insert_temp(id.with("knob_pos"), n));
            value = Some(n);
        }
        if resp.drag_stopped() {
            ended = true;
            value = value.or(Some(norm));
        }
        if resp.double_clicked() {
            value = Some(default.clamp(0.0, 1.0));
            ended = true;
        }
    }
    let shown = value.unwrap_or(norm);
    paint_knob(ui.painter(), c, d, shown, scale, !enabled);
    let resp = if enabled {
        resp.on_hover_cursor(CursorIcon::ResizeVertical)
    } else {
        resp
    };
    KnobOut { resp, value, ended }
}

// ---------------------------------------------------------------- LCD and digits

/// The width `text` takes as seven-segment digits `h` high.
pub fn seg7_width(text: &str, h: f32) -> f32 {
    hw::seg_width(text, h)
}

/// `text` as seven-segment digits `h` high from `pos` (top left), lit in `on`, with the
/// unlit "8" ghosts when `ghost`. Returns the width.
pub fn seg7(p: &Painter, pos: Pos2, text: &str, h: f32, on: Color32, ghost: bool) -> f32 {
    hw::seg(p, pos, text, h, on, ghost)
}

/// The black bezel around LCD glass (with the glass).
pub fn bezel(p: &Painter, rect: Rect) {
    hw::lcd(p, rect);
}

/// Green-grey LCD glass with faint scanlines.
pub fn glass(p: &Painter, rect: Rect) {
    hw::lcd_glass(p, rect);
}

/// A bezelled LCD; returns the glass.
pub fn lcd(p: &Painter, rect: Rect) -> Rect {
    hw::lcd(p, rect)
}

/// A small label printed on LCD glass.
pub fn lcd_label(p: &Painter, pos: Pos2, align: Align2, text: &str) -> Rect {
    text_at(
        p,
        pos,
        align,
        &text.to_uppercase(),
        mono_font(9.0),
        alpha(LCD_INK, 0.72),
        0.12,
    )
}

/// Heavy LCD text.
pub fn lcd_text(p: &Painter, pos: Pos2, align: Align2, text: &str, size: f32) -> Rect {
    text_at(
        p,
        pos,
        align,
        &text.to_uppercase(),
        mono_heavy(size),
        LCD_INK,
        0.02,
    )
}

/// Splits a value text like "-3.5 dB", "350ms" or "0.0dB 1.00" into the digits and the unit
/// (the first word after them); a text without digits up front is all unit.
pub fn split_value(text: &str) -> (String, String) {
    let t = text.trim();
    let end = t
        .char_indices()
        .find(|(_, c)| {
            !(c.is_ascii_digit() || matches!(c, '-' | '.' | '+' | ' ' | ':' | '/' | ','))
        })
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    let num = t[..end].trim().replace(',', ".");
    let unit = t[end..].split_whitespace().next().unwrap_or("").to_string();
    if num.chars().any(|c| c.is_ascii_digit()) {
        (num, unit)
    } else {
        (
            String::new(),
            t.split_whitespace().next().unwrap_or("").to_string(),
        )
    }
}

/// A small LCD chip (VOL / PEAK style): `label` top left, the value in segments (or LCD
/// text when it has letters) on the right, its unit after it.
pub fn lcd_chip(p: &Painter, rect: Rect, label: &str, value: &str) {
    p.line_segment(
        [
            pos2(rect.left() + 3.0, rect.bottom() + 0.5),
            pos2(rect.right() - 3.0, rect.bottom() + 0.5),
        ],
        Stroke::new(1.0, BONE_HI),
    );
    glass(p, rect);
    p.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, Color32::from_black_alpha(110)),
        StrokeKind::Outside,
    );
    if !label.is_empty() {
        lcd_label(p, rect.left_top() + vec2(5.0, 4.0), Align2::LEFT_TOP, label);
    }
    let (num, unit) = split_value(value);
    let h = (rect.height() * 0.5).clamp(9.0, 22.0);
    let right = rect.right() - 6.0;
    let base = rect.bottom() - 4.0;
    let mut x = right;
    if !unit.is_empty() {
        let u = fit(p, &unit, 8.5, rect.width() - 12.0);
        let r = lcd_text(
            p,
            pos2(x, base),
            Align2::RIGHT_BOTTOM,
            &u,
            if num.is_empty() { 11.0 } else { 8.5 },
        );
        x = r.left() - 3.0;
    }
    if !num.is_empty() {
        let w = seg7_width(&num, h);
        let left = (x - w).max(rect.left() + 3.0);
        seg7(p, pos2(left, base - h), &num, h, LCD_INK, true);
    }
}

/// A horizontal LED ladder in a black well: `level` 0..1 lit, segments past `hot` (0..1)
/// orange, `peak` a held segment.
pub fn ladder_h(p: &Painter, rect: Rect, level: f32, hot: f32, peak: Option<f32>, reverse: bool) {
    well(p, rect, 3.0);
    let inner = rect.shrink2(vec2(4.0, 4.0));
    let seg_w = 4.0;
    let gap = 2.0;
    let n = ((inner.width() + gap) / (seg_w + gap)).floor().max(1.0) as usize;
    let lit = (level.clamp(0.0, 1.0) * n as f32).round() as usize;
    let pk = peak.map(|v| ((v.clamp(0.0, 1.0) * n as f32).round() as usize).saturating_sub(1));
    for i in 0..n {
        let x = if reverse {
            inner.right() - (i as f32 + 1.0) * (seg_w + gap) + gap
        } else {
            inner.left() + i as f32 * (seg_w + gap)
        };
        let r = Rect::from_min_size(pos2(x, inner.top()), vec2(seg_w, inner.height()));
        let frac = i as f32 / n as f32;
        let on_col = if frac >= hot {
            ORANGE
        } else {
            rgb(0xF3, 0xE9, 0xDA)
        };
        let on = i < lit || pk == Some(i);
        let col = if on { on_col } else { rgb(0x2B, 0x29, 0x26) };
        p.rect_filled(r, 0.5, col);
        if on && frac >= hot {
            p.rect_filled(r.expand(1.0), 1.0, alpha(ORANGE, 0.15));
        }
    }
}

/// A black value field with a stepper glyph (a menu).
pub fn field(ui: &mut Ui, rect: Rect, id: Id, text: &str, enabled: bool) -> Response {
    let resp = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    hw::paint_field(ui.painter(), rect, text, true, enabled && resp.hovered());
    if !enabled {
        ui.painter().rect_filled(rect, 2.0, alpha(BONE, 0.35));
    }
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// A field that opens a menu of `choices`; returns the one picked.
pub fn choice_field(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    choices: &[&str],
    current: usize,
    enabled: bool,
) -> Option<usize> {
    let text = choices.get(current).copied().unwrap_or("");
    let resp = field(ui, rect, id, text, enabled);
    let mut picked = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(rect.width());
        for (i, c) in choices.iter().enumerate() {
            if ui.selectable_label(i == current, *c).clicked() {
                picked = Some(i);
            }
        }
    });
    picked.filter(|i| *i != current)
}

/// A fader cap: dark body with bone lines; `horizontal` turns it for a slide fader.
pub fn fader_cap(p: &Painter, c: Pos2, w: f32, h: f32, horizontal: bool) {
    let rect = if horizontal {
        Rect::from_center_size(c, vec2(h, w))
    } else {
        Rect::from_center_size(c, vec2(w, h))
    };
    hw::fader_cap(p, rect);
}

/// A horizontal slide fader in a slot: returns the new position while dragged and whether
/// the gesture ended.
pub fn slide_h(ui: &mut Ui, rect: Rect, id: Id, norm: f32, default: f32, enabled: bool) -> KnobOut {
    let resp = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let mut value = None;
    let mut ended = false;
    let x0 = rect.left() + 10.0;
    let x1 = rect.right() - 10.0;
    if enabled {
        if let Some(pt) = resp.interact_pointer_pos() {
            if resp.dragged() || resp.drag_started() {
                value = Some(((pt.x - x0) / (x1 - x0)).clamp(0.0, 1.0));
            }
        }
        if resp.drag_stopped() {
            ended = true;
            value = value.or(Some(norm));
        }
        if resp.double_clicked() {
            value = Some(default);
            ended = true;
        }
    }
    let n = value.unwrap_or(norm).clamp(0.0, 1.0);
    let p = ui.painter();
    let cy = rect.center().y;
    // The printed scale.
    for i in 0..=10 {
        let x = x0 + (x1 - x0) * i as f32 / 10.0;
        let major = i % 5 == 0;
        p.line_segment(
            [
                pos2(x, rect.top() + 1.0),
                pos2(x, rect.top() + if major { 6.0 } else { 4.0 }),
            ],
            Stroke::new(if major { 1.4 } else { 1.0 }, INK2),
        );
    }
    let slot = Rect::from_min_max(pos2(x0 - 3.0, cy - 3.0), pos2(x1 + 3.0, cy + 3.0));
    p.rect_filled(slot, 3.0, INK);
    inner_shadow(p, slot, 3.0, 1.0);
    fader_cap(p, pos2(x0 + (x1 - x0) * n, cy + 2.0), 14.0, 26.0, true);
    KnobOut {
        resp: if enabled {
            resp.on_hover_cursor(CursorIcon::ResizeHorizontal)
        } else {
            resp
        },
        value,
        ended,
    }
}

// ---------------------------------------------------------------- the unit

/// A unit's identity on its faceplate.
pub struct Plate<'a> {
    /// Model code, e.g. "EQ-3".
    pub model: &'a str,
    /// What it is, printed beside the model.
    pub name: &'a str,
    /// The strip it sits on: (badge, name), on the name plate at the foot.
    pub strip: Option<(&'a str, &'a str)>,
    /// A RESET key at the foot.
    pub reset: bool,
}

/// What the faceplate's keys did.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderOut {
    pub close: bool,
    pub power: bool,
    pub reset: bool,
}

/// Header height.
pub const HEADER_H: f32 = 60.0;
/// Foot height.
pub const FOOT_H: f32 = 56.0;

/// Paints the unit's header across `rect`: the model, its name, the ON key (when `on` is
/// given) and the close key. Returns the keys' clicks and the x left of the keys.
pub fn header(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    plate: &Plate,
    on: Option<bool>,
) -> (HeaderOut, f32) {
    let mut out = HeaderOut::default();
    let p = ui.painter().clone();
    let cy = rect.top() + 32.0;
    silk(
        &p,
        pos2(rect.left() + 26.0, rect.top() + 14.0),
        Align2::LEFT_CENTER,
        "Pro Wave",
        9.5,
        PRINT,
    );
    let m = text_at(
        &p,
        pos2(rect.left() + 25.0, cy + 6.0),
        Align2::LEFT_CENTER,
        &plate.model.to_uppercase(),
        silk_heavy(28.0),
        INK,
        0.04,
    );
    let sq = Rect::from_center_size(pos2(m.right() + 13.0, cy + 5.0), vec2(9.0, 9.0));
    p.rect_filled(sq, 1.0, ORANGE);
    let name_x = sq.right() + 9.0;
    let k = 28.0;
    let close = Rect::from_min_size(
        pos2(rect.right() - 22.0 - k, cy - k / 2.0 + 1.0),
        vec2(k, k),
    );
    if glyph_key(
        ui,
        close,
        id.with("gear_close"),
        Cap::Dark,
        false,
        Glyph::Close,
    )
    .clicked()
    {
        out.close = true;
    }
    let mut x = close.left() - 10.0;
    if let Some(on) = on {
        let r = Rect::from_min_size(pos2(x - 66.0, cy - k / 2.0 + 1.0), vec2(66.0, k));
        let resp = key(
            ui,
            r,
            id.with("gear_power"),
            "ON",
            Cap::Bone,
            LedAt::Left,
            on,
            on,
        );
        if resp.on_hover_text(crate::i18n::tr("Bypass")).clicked() {
            out.power = true;
        }
        x = r.left() - 12.0;
    }
    let nm = fit(&p, &plate.name.to_uppercase(), 14.0, (x - name_x).max(20.0));
    text_at(
        &p,
        pos2(name_x, cy + 6.5),
        Align2::LEFT_CENTER,
        &nm,
        silk_font(14.5),
        INK2,
        0.15,
    );
    seam_h(
        &p,
        rect.left() + 1.0,
        rect.right() - 1.0,
        rect.top() + HEADER_H - 2.0,
    );
    (out, x)
}

/// Paints the unit's foot across `rect`: the strip's name plate and the RESET key. Returns
/// whether RESET was clicked and the x right of the plate, where callers may add keys.
pub fn foot(ui: &mut Ui, rect: Rect, id: Id, plate: &Plate) -> (bool, f32) {
    let p = ui.painter().clone();
    seam_h(&p, rect.left() + 1.0, rect.right() - 1.0, rect.top());
    let cy = rect.top() + rect.height() / 2.0 - 2.0;
    let mut x = rect.left() + 26.0;
    if let Some((badge, name)) = plate.strip {
        let r = Rect::from_min_size(pos2(x, cy - 11.0), vec2(230.0, 22.0));
        name_plate(&p, r, badge, name, false);
        x = r.right() + 14.0;
    }
    let mut reset = false;
    if plate.reset {
        let r = Rect::from_min_size(
            pos2(rect.right() - 26.0 - 84.0, cy - 13.0),
            vec2(84.0, 26.0),
        );
        reset = key(
            ui,
            r,
            id.with("gear_reset"),
            crate::i18n::tr("Reset"),
            Cap::Bone,
            LedAt::None,
            false,
            false,
        )
        .clicked();
    }
    (reset, x)
}

/// The chassis frame of a unit window.
pub fn frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(BONE)
        .stroke(Stroke::new(1.0, SEAM_DK))
        .corner_radius(CornerRadius::same(9))
        .shadow(Shadow {
            offset: [0, 10],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(95),
        })
}

/// Screws in the four corners of `rect`.
pub fn corner_screws(p: &Painter, rect: Rect) {
    let i = 11.0;
    screw(p, rect.left_top() + vec2(i, i), 30.0);
    screw(p, pos2(rect.right() - i, rect.top() + i), -20.0);
    screw(p, pos2(rect.left() + i, rect.bottom() - i), 75.0);
    screw(p, rect.right_bottom() - vec2(i, i), 10.0);
}

/// A unit window: a chassis without a title bar that moves by its faceplate. `body` lays out
/// the panel between the header and the foot. Returns the keys' clicks and `body`'s result.
pub fn unit_window<R>(
    ctx: &egui::Context,
    id: Id,
    width: f32,
    plate: &Plate,
    on: Option<bool>,
    body: impl FnOnce(&mut Ui) -> R,
) -> (HeaderOut, Option<R>) {
    theme::install_fonts(ctx);
    let mut out = HeaderOut::default();
    let resp = egui::Window::new(plate.model)
        .id(id)
        .title_bar(false)
        .resizable(false)
        .frame(frame())
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.set_width(width);
            let (hr, _) = ui.allocate_exact_size(vec2(width, HEADER_H), Sense::hover());
            let (h, _) = header(ui, hr, id, plate, on);
            out = h;
            let r = body(ui);
            if plate.strip.is_some() || plate.reset {
                let (fr, _) = ui.allocate_exact_size(vec2(width, FOOT_H), Sense::hover());
                out.reset = foot(ui, fr, id, plate).0;
            } else {
                ui.add_space(18.0);
            }
            r
        });
    if let Some(r) = &resp {
        corner_screws(&ctx.layer_painter(r.response.layer_id), r.response.rect);
    }
    (out, resp.and_then(|r| r.inner))
}

/// One control of a parameter panel.
#[derive(Clone, Debug)]
pub struct Control {
    pub label: String,
    /// Position 0..1.
    pub norm: f32,
    /// Where a double-click goes.
    pub default: f32,
    /// The value as text, for the LCD under it.
    pub text: String,
    /// Named values: shown as keys (few) or a menu (many) instead of a knob.
    pub choices: Vec<String>,
    /// The value's index among `choices`.
    pub choice: usize,
    /// Automation: `Some(true)` an envelope plays it, `Some(false)` overridden by hand.
    pub automated: Option<bool>,
    pub enabled: bool,
}

/// A change made on a panel: which control, its new position (0..1) or choice index, and
/// whether the gesture ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    pub index: usize,
    pub norm: f32,
    pub choice: Option<usize>,
    pub ended: bool,
}

/// Width of one knob cell.
pub const CELL_W: f32 = 92.0;
/// Height of one knob row.
pub const CELL_H: f32 = 132.0;

/// Draws one control in the cell at `rect` (label, knob or keys, LCD readout).
pub fn control_cell(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    k: usize,
    c: &Control,
    d: f32,
) -> Option<Change> {
    let p = ui.painter().clone();
    let label = fit(&p, &c.label.to_uppercase(), 11.0, rect.width() - 14.0);
    let lr = text_at(
        &p,
        pos2(rect.center().x, rect.top() + 9.0),
        Align2::CENTER_CENTER,
        &label,
        silk_font(11.5),
        INK2,
        0.12,
    );
    if let Some(on) = c.automated {
        led(&p, pos2(lr.right() + 7.0, lr.center().y), 2.5, on);
    }
    let mut change = None;
    if c.choices.is_empty() {
        let kc = pos2(rect.center().x, rect.top() + 24.0 + d / 2.0 + 6.0);
        let out = knob(
            ui,
            id.with(("knob", k)),
            kc,
            d,
            c.norm,
            c.default,
            true,
            c.enabled,
        );
        if let Some(v) = out.value {
            change = Some(Change {
                index: k,
                norm: v,
                choice: None,
                ended: out.ended,
            });
        } else if out.ended {
            change = Some(Change {
                index: k,
                norm: c.norm,
                choice: None,
                ended: true,
            });
        }
        if c.automated.is_some() {
            let tip = if c.automated == Some(true) {
                crate::i18n::tr("Automated")
            } else {
                crate::i18n::tr("Overridden")
            };
            out.resp.on_hover_text(tip);
        }
        let chip = Rect::from_min_size(
            pos2(rect.left() + 8.0, rect.bottom() - 30.0),
            vec2(rect.width() - 16.0, 24.0),
        );
        lcd_chip(&p, chip, "", &c.text);
    } else if c.choices.len() <= 3 {
        // Keys, one per value, stacked.
        let n = c.choices.len();
        let kh = 22.0;
        let top = rect.top() + 22.0;
        for (i, name) in c.choices.iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(rect.left() + 10.0, top + i as f32 * (kh + 8.0)),
                vec2(rect.width() - 20.0, kh),
            );
            let sel = i == c.choice;
            let resp = if c.enabled {
                key(
                    ui,
                    r,
                    id.with(("key", k, i)),
                    name,
                    Cap::Bone,
                    LedAt::Left,
                    sel,
                    sel,
                )
            } else {
                paint_key(&p, r, Cap::Bone, sel);
                led(
                    &p,
                    pos2(r.left() + 10.0, r.center().y + if sel { 1.0 } else { 0.0 }),
                    3.0,
                    sel,
                );
                ui.interact(r, id.with(("key", k, i)), Sense::hover())
            };
            if resp.clicked() && !sel {
                change = Some(Change {
                    index: k,
                    norm: if n > 1 {
                        i as f32 / (n - 1) as f32
                    } else {
                        0.0
                    },
                    choice: Some(i),
                    ended: true,
                });
            }
        }
    } else {
        let names: Vec<&str> = c.choices.iter().map(String::as_str).collect();
        let r = Rect::from_min_size(
            pos2(rect.left() + 6.0, rect.top() + 24.0 + d / 2.0 - 7.0),
            vec2(rect.width() - 12.0, 28.0),
        );
        if let Some(i) = choice_field(ui, r, id.with(("choice", k)), &names, c.choice, c.enabled) {
            let n = names.len();
            change = Some(Change {
                index: k,
                norm: if n > 1 {
                    i as f32 / (n - 1) as f32
                } else {
                    0.0
                },
                choice: Some(i),
                ended: true,
            });
        }
    }
    change
}

/// Height `controls` take laid out `per_row` to a row.
pub fn grid_height(n: usize, per_row: usize) -> f32 {
    let rows = n.div_ceil(per_row.max(1));
    rows as f32 * CELL_H
}

/// Lays `indices` of `controls` out in rows of knob cells from `top_left`, `per_row` to a row.
pub fn control_grid(
    ui: &mut Ui,
    top_left: Pos2,
    per_row: usize,
    id: Id,
    controls: &[Control],
    indices: &[usize],
    d: f32,
) -> Vec<Change> {
    let mut out = Vec::new();
    for (n, k) in indices.iter().enumerate() {
        let Some(c) = controls.get(*k) else { continue };
        let (row, col) = (n / per_row.max(1), n % per_row.max(1));
        let r = Rect::from_min_size(
            top_left + vec2(col as f32 * CELL_W, row as f32 * CELL_H),
            vec2(CELL_W, CELL_H),
        );
        if let Some(ch) = control_cell(ui, r, id, *k, c, d) {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_split_into_digits_and_unit() {
        assert_eq!(split_value("-3.5 dB"), ("-3.5".into(), "dB".into()));
        assert_eq!(split_value("350ms"), ("350".into(), "ms".into()));
        assert_eq!(split_value("Off"), (String::new(), "Off".into()));
        assert_eq!(split_value("1:1"), ("1:1".into(), String::new()));
    }

    #[test]
    fn segment_widths_grow_with_digits() {
        assert!(seg7_width("88", 20.0) > seg7_width("8", 20.0));
        assert!(seg7_width("8.8", 20.0) < seg7_width("888", 20.0));
    }

    #[test]
    fn a_gradient_band_clip_keeps_the_polygon_inside() {
        let poly = rrect_points(Rect::from_min_size(Pos2::ZERO, vec2(40.0, 20.0)), 5.0);
        let band = clip_band(&poly, 5.0, 10.0);
        assert!(band.iter().all(|q| q.y >= 4.999 && q.y <= 10.001));
        assert!(band.len() >= 4);
    }
}
