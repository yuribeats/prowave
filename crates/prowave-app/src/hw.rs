//! Painters for the Hardware look's controls (see `theme.rs` for the colours and type): key caps
//! with LED dots, LCD panels with seven-segment digits, knobs, faders with tick scales,
//! segmented meters, insert slots, dark value fields, silkscreen labels, seams and screws.
//!
//! The `paint_*` functions only draw; the ones without the prefix also take the click
//! (`ui.interact`) and return its `Response`, so a panel can swap a button for a key in one line.
// A kit: each window uses the pieces it needs, and the public build has fewer windows.
#![allow(dead_code)]
use crate::theme::{self, Face};
use egui::{
    epaint::{tessellator::path, Mesh, RoundedRect},
    pos2, vec2, Align2, Color32, CornerRadius, Id, Painter, Pos2, Rect, Response, Sense, Shape,
    Stroke, Ui, Vec2,
};

// ---------------------------------------------------------------------------------------------
// Fills.

/// `rect` rounded by `radius`, filled with a vertical gradient from `top` to `bottom`.
pub fn gradient(g: &Painter, rect: Rect, radius: f32, top: Color32, bottom: Color32) {
    gradient3(g, rect, radius, top, lerp(top, bottom, 0.5), bottom);
}

/// [`gradient`] through a middle colour at half height.
pub fn gradient3(
    g: &Painter,
    rect: Rect,
    radius: f32,
    top: Color32,
    mid: Color32,
    bottom: Color32,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let mut pts = Vec::new();
    path::rounded_rectangle(&mut pts, RoundedRect::new(rect, radius));
    let color_at = |y: f32| {
        let t = ((y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        if t < 0.5 {
            lerp(top, mid, t * 2.0)
        } else {
            lerp(mid, bottom, (t - 0.5) * 2.0)
        }
    };
    let mut mesh = Mesh::default();
    let c = rect.center();
    mesh.colored_vertex(c, color_at(c.y));
    for p in &pts {
        mesh.colored_vertex(*p, color_at(p.y));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    g.add(Shape::mesh(mesh));
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let [ar, ag, ab, aa] = a.to_array();
    let [br, bg, bb, ba] = b.to_array();
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_premultiplied(m(ar, br), m(ag, bg), m(ab, bb), m(aa, ba))
}

/// The chassis: bone, a touch lighter at the top.
pub fn chassis(g: &Painter, rect: Rect) {
    gradient(
        g,
        rect,
        0.0,
        Color32::from_rgb(0xef, 0xec, 0xe4),
        Color32::from_rgb(0xe7, 0xe3, 0xd9),
    );
}

/// A recessed bone panel (empty slots, wells): darker bone with an inner shadow at the top.
pub fn recess(g: &Painter, rect: Rect, radius: f32) {
    g.rect_filled(rect, radius, theme::BONE2);
    inner_shadow(g, rect, radius, 0.20);
    g.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, Color32::from_black_alpha(20)),
        egui::StrokeKind::Inside,
    );
}

/// A shadow cast inside `rect` from its top edge (a well).
pub fn inner_shadow(g: &Painter, rect: Rect, radius: f32, strength: f32) {
    let top = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 3.0));
    let cr = CornerRadius {
        nw: radius as u8,
        ne: radius as u8,
        sw: 0,
        se: 0,
    };
    g.rect_filled(
        top,
        cr,
        Color32::from_black_alpha((strength * 0.5 * 255.0) as u8),
    );
    g.line_segment(
        [
            pos2(rect.left() + radius, rect.top() + 0.5),
            pos2(rect.right() - radius, rect.top() + 0.5),
        ],
        Stroke::new(1.0, Color32::from_black_alpha((strength * 255.0) as u8)),
    );
}

/// A horizontal panel seam at `y` (a dark line over a light one).
pub fn seam_h(g: &Painter, x: impl Into<egui::Rangef>, y: f32) {
    let x = x.into();
    g.hline(x, y + 0.5, Stroke::new(1.0, theme::SEAM));
    g.hline(x, y + 1.5, Stroke::new(1.0, theme::BONE_HI));
}

/// A vertical panel seam at `x`.
pub fn seam_v(g: &Painter, x: f32, y: impl Into<egui::Rangef>) {
    let y = y.into();
    g.vline(x + 0.5, y, Stroke::new(1.0, theme::SEAM));
    g.vline(x + 1.5, y, Stroke::new(1.0, theme::BONE_HI));
}

/// A slotted screw head, its slot at `angle` degrees.
pub fn screw(g: &Painter, c: Pos2, r: f32, angle: f32) {
    g.circle_filled(c + vec2(0.0, 1.0), r, Color32::WHITE);
    g.circle_filled(c, r, Color32::from_rgb(0xb9, 0xb2, 0xa4));
    g.circle_filled(
        c + vec2(-r * 0.2, -r * 0.25),
        r * 0.7,
        Color32::from_rgb(0xdd, 0xd8, 0xcd),
    );
    g.circle_stroke(c, r, Stroke::new(1.0, Color32::from_black_alpha(40)));
    let a = angle.to_radians();
    let d = vec2(a.cos(), a.sin()) * (r * 0.75);
    g.line_segment(
        [c - d, c + d],
        Stroke::new(1.6, Color32::from_rgb(0x7b, 0x74, 0x67)),
    );
}

// ---------------------------------------------------------------------------------------------
// Type.

/// `text` laid out in `face` at `size`, with the face's letter spacing.
pub fn galley(
    g: &Painter,
    text: &str,
    face: Face,
    size: f32,
    color: Color32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: theme::font(face, size),
            color,
            extra_letter_spacing: theme::tracking(face) * size,
            ..Default::default()
        },
    );
    g.layout_job(job)
}

/// Text in `face`, placed by `anchor` at `pos`; returns where it went.
pub fn text(
    g: &Painter,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    face: Face,
    size: f32,
    color: Color32,
) -> Rect {
    let gal = galley(g, text, face, size, color);
    let mut size_v = gal.size();
    // The last letter's spacing is not part of the word.
    size_v.x -= theme::tracking(face) * size;
    let rect = anchor.anchor_size(pos, size_v);
    g.galley(rect.min, gal, color);
    rect
}

/// Silkscreened caps: `label` upper-cased in the condensed face.
pub fn silk(
    g: &Painter,
    pos: Pos2,
    anchor: Align2,
    label: &str,
    size: f32,
    color: Color32,
) -> Rect {
    text(
        g,
        pos,
        anchor,
        &label.to_uppercase(),
        Face::Silk,
        size,
        color,
    )
}

/// Width of `label` as [`silk`] sets it.
pub fn silk_width(g: &Painter, label: &str, size: f32) -> f32 {
    galley(g, &label.to_uppercase(), Face::Silk, size, theme::INK)
        .size()
        .x
        - theme::tracking(Face::Silk) * size
}

/// `text` cut to fit `width` in `face` (with "…").
pub fn fit(g: &Painter, text: &str, face: Face, size: f32, width: f32) -> String {
    let w = |s: &str| galley(g, s, face, size, theme::INK).size().x;
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

/// A section bracket over a group of controls: a short tick, the label, a line to the right
/// end, and small drops at both ends (`|— VIEW ———————|`).
pub fn bracket(g: &Painter, left: f32, right: f32, y: f32, label: &str) {
    let ink = theme::INK2.gamma_multiply(0.6);
    let st = Stroke::new(1.0, ink);
    g.hline(left..=left + 10.0, y, st);
    let r = silk(
        g,
        pos2(left + 16.0, y),
        Align2::LEFT_CENTER,
        label,
        11.0,
        theme::INK2,
    );
    if r.right() + 6.0 < right {
        g.hline(r.right() + 6.0..=right, y, st);
    }
    g.vline(left, y..=y + 7.0, st);
    g.vline(right, y..=y + 7.0, st);
}

// ---------------------------------------------------------------------------------------------
// Keys and LEDs.

/// The three key caps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    /// Bone face, black print.
    Bone,
    /// Black face, bone print.
    Dark,
    /// Signal orange, black print.
    Orange,
}

impl Cap {
    /// The print colour on this cap.
    pub fn ink(self) -> Color32 {
        match self {
            Cap::Bone | Cap::Orange => theme::INK,
            Cap::Dark => theme::ON_DARK,
        }
    }
}

/// A key cap in `rect` (its lip and shadow drawn below it). `down` presses it in by a point.
pub fn paint_key(g: &Painter, rect: Rect, cap: Cap, down: bool, hovered: bool) -> Rect {
    let radius = (rect.height() * 0.12).clamp(3.0, 5.0);
    let (top, mid, bottom, edge, lip) = match cap {
        Cap::Bone => (
            theme::BONE_TOP,
            Color32::from_rgb(0xee, 0xea, 0xe1),
            Color32::from_rgb(0xe3, 0xde, 0xd3),
            theme::KEY_EDGE,
            theme::KEY_LIP,
        ),
        Cap::Dark => (
            theme::CAP_TOP,
            theme::CAP,
            Color32::from_rgb(0x15, 0x14, 0x13),
            theme::CAP_EDGE,
            Color32::from_rgb(0x05, 0x05, 0x05),
        ),
        Cap::Orange => (
            theme::ORANGE_HI,
            theme::ORANGE,
            Color32::from_rgb(0xee, 0x4e, 0x0c),
            theme::ORANGE_DK,
            Color32::from_rgb(0xa8, 0x36, 0x05),
        ),
    };
    let lift = if down { 1.0 } else { 2.0 };
    let face = rect.translate(vec2(0.0, if down { 1.0 } else { 0.0 }));
    // Soft shadow, then the lip under the face.
    g.rect_filled(
        rect.translate(vec2(0.0, lift + 1.0)).expand(0.5),
        radius + 0.5,
        Color32::from_black_alpha(if down { 22 } else { 40 }),
    );
    g.rect_filled(rect.translate(vec2(0.0, lift)), radius, lip);
    let (top, mid) = if hovered && !down {
        (
            lerp(top, Color32::WHITE, 0.25),
            lerp(mid, Color32::WHITE, 0.15),
        )
    } else {
        (top, mid)
    };
    gradient3(g, face, radius, top, mid, bottom);
    if down {
        inner_shadow(g, face, radius, 0.10);
    } else {
        let hi = match cap {
            Cap::Bone => Color32::WHITE,
            Cap::Dark => Color32::from_white_alpha(36),
            Cap::Orange => Color32::from_white_alpha(110),
        };
        g.hline(
            face.left() + radius..=face.right() - radius,
            face.top() + 1.5,
            Stroke::new(1.0, hi),
        );
    }
    g.rect_stroke(
        face,
        radius,
        Stroke::new(1.0, edge),
        egui::StrokeKind::Inside,
    );
    face
}

/// An LED dot: dark when off, glowing orange when on.
pub fn led(g: &Painter, c: Pos2, on: bool) {
    led_sized(g, c, 3.0, on);
}

/// [`led`] with radius `r`.
pub fn led_sized(g: &Painter, c: Pos2, r: f32, on: bool) {
    if on {
        for (grow, a) in [(4.0, 30u8), (2.5, 55), (1.2, 90)] {
            g.circle_filled(c, r + grow, Color32::from_rgba_unmultiplied(255, 90, 20, a));
        }
        g.circle_filled(c, r, Color32::from_rgb(0xe5, 0x4a, 0x08));
        g.circle_filled(
            c + vec2(-0.3, -0.4),
            r * 0.75,
            Color32::from_rgb(0xff, 0x6a, 0x22),
        );
        g.circle_filled(
            c + vec2(-0.6, -0.8),
            r * 0.35,
            Color32::from_rgb(0xff, 0xd2, 0xb8),
        );
    } else {
        g.circle_filled(c + vec2(0.0, 1.0), r, Color32::from_white_alpha(130));
        g.circle_filled(c, r, theme::LED_OFF);
        g.circle_filled(
            c + vec2(0.0, 0.6),
            r * 0.7,
            Color32::from_rgb(0x6a, 0x65, 0x5b),
        );
    }
}

/// What a key shows on its face.
#[derive(Clone, Copy)]
pub enum Legend<'a> {
    None,
    Text(&'a str),
    /// Drawn by the caller into the face rect (an icon).
    Custom,
}

/// A clickable key: the cap, an optional LED (Some(lit)) near its top, and a text legend.
/// Returns the response and the face rect (for a custom legend).
#[allow(clippy::too_many_arguments)]
pub fn key(
    ui: &Ui,
    g: &Painter,
    rect: Rect,
    id: Id,
    cap: Cap,
    down: bool,
    led_on: Option<bool>,
    legend: Legend<'_>,
) -> (Response, Rect) {
    let resp = ui.interact(rect, id, Sense::click());
    let pressed = down || resp.is_pointer_button_down_on();
    let face = paint_key(g, rect, cap, pressed, resp.hovered());
    let mut body = face;
    if let Some(on) = led_on {
        if face.height() >= 30.0 {
            led(g, pos2(face.center().x, face.top() + 8.0), on);
            body.min.y += 9.0;
        } else {
            led_sized(g, pos2(face.left() + 8.0, face.center().y), 2.5, on);
            body.min.x += 10.0;
        }
    }
    if let Legend::Text(t) = legend {
        let size = (face.height() * 0.5).clamp(9.0, 12.0);
        let shown = fit(g, &t.to_uppercase(), Face::Bold, size, body.width() - 6.0);
        text(
            g,
            body.center(),
            Align2::CENTER_CENTER,
            &shown,
            Face::Bold,
            size,
            cap.ink(),
        );
    }
    (resp, body)
}

/// A small toggle key with an LED at its left and a legend (mute/solo/record, mode switches).
pub fn toggle(ui: &Ui, g: &Painter, rect: Rect, id: Id, on: bool, label: &str) -> Response {
    key(
        ui,
        g,
        rect,
        id,
        Cap::Bone,
        on,
        Some(on),
        Legend::Text(label),
    )
    .0
}

// ---------------------------------------------------------------------------------------------
// LCD.

/// The LCD's black bezel and green glass; returns the glass rect.
pub fn lcd(g: &Painter, rect: Rect) -> Rect {
    g.rect_filled(rect.translate(vec2(0.0, 1.0)), 7.0, theme::BONE_HI);
    gradient(
        g,
        rect,
        7.0,
        Color32::from_rgb(0x2a, 0x28, 0x26),
        Color32::from_rgb(0x12, 0x11, 0x10),
    );
    let glass = Rect::from_min_max(rect.min + vec2(8.0, 5.0), rect.max - vec2(8.0, 5.0));
    lcd_glass(g, glass);
    glass
}

/// LCD glass alone (for small readouts set in a bone panel).
pub fn lcd_glass(g: &Painter, glass: Rect) {
    gradient3(
        g,
        glass,
        3.0,
        Color32::from_rgb(0xa7, 0xb2, 0x93),
        Color32::from_rgb(0xb5, 0xbf, 0xa2),
        Color32::from_rgb(0xae, 0xb8, 0x9c),
    );
    // Faint scan lines.
    let mut y = glass.top() + 2.0;
    while y < glass.bottom() {
        g.hline(
            glass.x_range(),
            y,
            Stroke::new(1.0, Color32::from_black_alpha(6)),
        );
        y += 3.0;
    }
    inner_shadow(g, glass, 3.0, 0.30);
    g.rect_stroke(
        glass,
        3.0,
        Stroke::new(1.0, Color32::from_black_alpha(90)),
        egui::StrokeKind::Inside,
    );
}

/// A small LCD readout: glass with a label in its corner and a value in segments at its right.
pub fn lcd_readout(g: &Painter, rect: Rect, label: &str, value: &str) {
    lcd_glass(g, rect);
    text(
        g,
        rect.left_top() + vec2(4.0, 3.0),
        Align2::LEFT_TOP,
        &label.to_uppercase(),
        Face::Lcd,
        7.5,
        theme::LCD_INK.gamma_multiply(0.7),
    );
    let h = (rect.height() * 0.55).min(18.0);
    let w = seg_width(value, h);
    seg(
        g,
        pos2(rect.right() - 5.0 - w, rect.bottom() - 4.0 - h),
        value,
        h,
        theme::LCD_INK,
        true,
    );
}

// Seven segments: a top, b upper right, c lower right, d bottom, e lower left, f upper left,
// g middle.
fn segments(c: char) -> Option<u8> {
    Some(match c {
        '0' | 'O' | 'o' => 0b0111111,
        '1' => 0b0000110,
        '2' => 0b1011011,
        '3' => 0b1001111,
        '4' => 0b1100110,
        '5' | 'S' | 's' => 0b1101101,
        '6' => 0b1111101,
        '7' => 0b0000111,
        '8' => 0b1111111,
        '9' => 0b1101111,
        '-' => 0b1000000,
        '_' => 0b0001000,
        ' ' => 0,
        'A' | 'a' => 0b1110111,
        'b' | 'B' => 0b1111100,
        'C' | 'c' => 0b0111001,
        'd' | 'D' => 0b1011110,
        'E' | 'e' => 0b1111001,
        'F' | 'f' => 0b1110001,
        'H' | 'h' => 0b1110110,
        'L' | 'l' => 0b0111000,
        'n' | 'N' => 0b1010100,
        'P' | 'p' => 0b1110011,
        'r' | 'R' => 0b1010000,
        't' | 'T' => 0b1111000,
        'U' | 'u' => 0b0111110,
        _ => return None,
    })
}

/// Advance of one character set in segments at height `h`.
fn seg_advance(c: char, h: f32) -> f32 {
    match c {
        '.' | ':' | ',' => h * 0.24,
        ' ' => h * 0.3,
        '/' => h * 0.42,
        _ => h * 0.67,
    }
}

/// Width of `s` set in segments at height `h`.
pub fn seg_width(s: &str, h: f32) -> f32 {
    let w: f32 = s.chars().map(|c| seg_advance(c, h)).sum();
    // The last digit has no gap after it.
    w - if s.chars().last().is_some_and(|c| segments(c).is_some()) {
        h * 0.17
    } else {
        0.0
    }
}

/// `s` in seven-segment digits `h` tall, its top left at `pos`, slanted like an LCD's. Unlit
/// segments show faintly when `ghost`. Characters without segments (letters other than the
/// few a seven-segment display can show) are set in the LCD face.
pub fn seg(g: &Painter, pos: Pos2, s: &str, h: f32, on: Color32, ghost: bool) -> f32 {
    let mut x = pos.x;
    let w = h * 0.5;
    let t = (h * 0.12).max(1.6);
    let gap = (h * 0.035).max(0.6);
    let slant = 0.08;
    let ghost_c = theme::LCD_GHOST;
    let skew = |p: Pos2| pos2(p.x + (pos.y + h - p.y) * slant, p.y);
    for ch in s.chars() {
        if ch == ' ' {
            x += seg_advance(ch, h);
            continue;
        }
        if let Some(bits) = segments(ch) {
            let (x0, x1) = (x, x + w);
            let (y0, ym, y1) = (pos.y, pos.y + h / 2.0, pos.y + h);
            let hseg = |y: f32| {
                let (a, b) = (x0 + gap, x1 - gap);
                vec![
                    pos2(a, y),
                    pos2(a + t / 2.0, y - t / 2.0),
                    pos2(b - t / 2.0, y - t / 2.0),
                    pos2(b, y),
                    pos2(b - t / 2.0, y + t / 2.0),
                    pos2(a + t / 2.0, y + t / 2.0),
                ]
            };
            let vseg = |xc: f32, ya: f32, yb: f32| {
                let (a, b) = (ya + gap, yb - gap);
                vec![
                    pos2(xc, a),
                    pos2(xc + t / 2.0, a + t / 2.0),
                    pos2(xc + t / 2.0, b - t / 2.0),
                    pos2(xc, b),
                    pos2(xc - t / 2.0, b - t / 2.0),
                    pos2(xc - t / 2.0, a + t / 2.0),
                ]
            };
            let ht = t / 2.0;
            let polys = [
                hseg(y0 + ht),
                vseg(x1 - ht, y0 + ht, ym),
                vseg(x1 - ht, ym, y1 - ht),
                hseg(y1 - ht),
                vseg(x0 + ht, ym, y1 - ht),
                vseg(x0 + ht, y0 + ht, ym),
                hseg(ym),
            ];
            for (i, poly) in polys.into_iter().enumerate() {
                let lit = bits & (1 << i) != 0;
                if !lit && !ghost {
                    continue;
                }
                let pts: Vec<Pos2> = poly.into_iter().map(skew).collect();
                g.add(Shape::convex_polygon(
                    pts,
                    if lit { on } else { ghost_c },
                    Stroke::NONE,
                ));
            }
        } else {
            match ch {
                '.' | ',' => {
                    g.circle_filled(skew(pos2(x + h * 0.08, pos.y + h - t / 2.0)), t * 0.55, on);
                }
                ':' => {
                    for fy in [0.3, 0.72] {
                        g.circle_filled(skew(pos2(x + h * 0.08, pos.y + h * fy)), t * 0.5, on);
                    }
                }
                '/' => {
                    g.line_segment(
                        [
                            skew(pos2(x + h * 0.05, pos.y + h)),
                            skew(pos2(x + h * 0.3, pos.y)),
                        ],
                        Stroke::new(t * 0.7, on),
                    );
                }
                _ => {
                    text(
                        g,
                        pos2(x, pos.y + h / 2.0),
                        Align2::LEFT_CENTER,
                        &ch.to_string(),
                        Face::Lcd,
                        h * 0.8,
                        on,
                    );
                }
            }
        }
        x += seg_advance(ch, h);
    }
    x - pos.x
}

// ---------------------------------------------------------------------------------------------
// Knobs, faders, meters.

/// A black knob with an orange pointer and a ring of tick marks. `value` runs from -1 (full
/// left, 7 o'clock) to 1 (full right, 5 o'clock).
pub fn paint_knob(g: &Painter, c: Pos2, r: f32, value: f32) {
    // Ticks around the top arc.
    for k in 0..=10 {
        let deg = -135.0 + 27.0 * k as f32;
        let a = (deg - 90.0).to_radians();
        let d = vec2(a.cos(), a.sin());
        let long = k == 0 || k == 5 || k == 10;
        g.line_segment(
            [
                c + d * (r + 2.5),
                c + d * (r + if long { 6.0 } else { 4.5 }),
            ],
            Stroke::new(1.0, theme::INK2.gamma_multiply(0.75)),
        );
    }
    g.circle_filled(c + vec2(0.0, 1.5), r + 0.5, Color32::from_black_alpha(60));
    g.circle_filled(c, r, theme::CAP_EDGE);
    g.circle_filled(
        c + vec2(-r * 0.12, -r * 0.16),
        r * 0.86,
        Color32::from_rgb(0x2c, 0x2a, 0x27),
    );
    g.circle_filled(
        c + vec2(-r * 0.22, -r * 0.3),
        r * 0.45,
        Color32::from_rgb(0x3a, 0x38, 0x34),
    );
    g.circle_stroke(c, r, Stroke::new(1.0, Color32::from_rgb(0x0b, 0x0b, 0x0a)));
    let a = (-90.0 + value.clamp(-1.0, 1.0) * 135.0).to_radians();
    let d = vec2(a.cos(), a.sin());
    g.line_segment(
        [c + d * (r * 0.25), c + d * (r * 0.92)],
        Stroke::new((r * 0.16).clamp(1.6, 2.6), theme::ORANGE),
    );
}

/// A vertical fader: a slot, an optional tick scale beside it, and a black cap with white
/// lines at `value` (0 bottom, 1 top). Returns the cap's rect.
pub fn paint_fader_v(g: &Painter, track: Rect, value: f32, cap: Vec2) -> Rect {
    let cx = track.center().x;
    let slot = Rect::from_center_size(pos2(cx, track.center().y), vec2(4.0, track.height()));
    g.rect_filled(slot.expand(1.0), 2.0, Color32::from_white_alpha(120));
    g.rect_filled(slot, 2.0, theme::INK);
    let span = track.height() - cap.y;
    let y = track.bottom() - cap.y / 2.0 - span * value.clamp(0.0, 1.0);
    let r = Rect::from_center_size(pos2(cx, y), cap);
    fader_cap(g, r);
    r
}

/// A horizontal fader (track headers, small panels).
pub fn paint_fader_h(g: &Painter, track: Rect, value: f32, cap: Vec2) -> Rect {
    let cy = track.center().y;
    let slot = Rect::from_center_size(pos2(track.center().x, cy), vec2(track.width(), 3.0));
    g.rect_filled(slot, 1.5, theme::INK);
    let span = track.width() - cap.x;
    let x = track.left() + cap.x / 2.0 + span * value.clamp(0.0, 1.0);
    let r = Rect::from_center_size(pos2(x, cy), cap);
    // A vertical cap: a bone body with a black line down the middle.
    g.rect_filled(
        r.translate(vec2(0.0, 1.5)),
        2.0,
        Color32::from_black_alpha(50),
    );
    gradient(
        g,
        r,
        2.0,
        theme::BONE_TOP,
        Color32::from_rgb(0xd9, 0xd4, 0xc8),
    );
    g.rect_stroke(
        r,
        2.0,
        Stroke::new(1.0, theme::INK),
        egui::StrokeKind::Inside,
    );
    g.vline(
        r.center().x,
        r.top() + 2.0..=r.bottom() - 2.0,
        Stroke::new(2.0, theme::INK),
    );
    r
}

/// A fader's black cap with two white lines across.
pub fn fader_cap(g: &Painter, r: Rect) {
    g.rect_filled(
        r.translate(vec2(0.0, 2.0)),
        3.0,
        Color32::from_black_alpha(70),
    );
    gradient3(
        g,
        r,
        3.0,
        Color32::from_rgb(0x3e, 0x3c, 0x38),
        theme::CAP,
        Color32::from_rgb(0x10, 0x10, 0x0f),
    );
    g.rect_stroke(
        r,
        3.0,
        Stroke::new(1.0, theme::CAP_EDGE),
        egui::StrokeKind::Inside,
    );
    let w = r.width() * 0.36;
    for dy in [-2.0, 2.0] {
        g.hline(
            r.center().x - w..=r.center().x + w,
            r.center().y + dy,
            Stroke::new(1.2, theme::BONE_HI),
        );
    }
}

/// A tick scale beside a vertical fader: `marks` are (0..1 position, label).
pub fn scale_v(g: &Painter, rect: Rect, marks: &[(f32, &str)], label_right: bool) {
    let st = Stroke::new(1.0, theme::PRINT);
    for (v, label) in marks {
        let y = rect.bottom() - rect.height() * v.clamp(0.0, 1.0);
        if label_right {
            g.hline(rect.left()..=rect.left() + 5.0, y, st);
            text(
                g,
                pos2(rect.left() + 8.0, y),
                Align2::LEFT_CENTER,
                label,
                Face::Bold,
                8.5,
                theme::INK2,
            );
        } else {
            g.hline(rect.right() - 5.0..=rect.right(), y, st);
            text(
                g,
                pos2(rect.right() - 8.0, y),
                Align2::RIGHT_CENTER,
                label,
                Face::Bold,
                8.5,
                theme::INK2,
            );
        }
    }
}

/// A segmented level meter in a dark window, vertical: `level` and `peak` from 0 to 1 (the
/// peak hold drawn as a bright bar). Segments near the top light peach.
pub fn paint_meter_v(g: &Painter, rect: Rect, level: f32, peak: f32) {
    g.rect_filled(rect.expand(1.0), 2.0, Color32::from_white_alpha(140));
    g.rect_filled(rect, 2.0, theme::METER_WELL);
    let inner = rect.shrink(2.0);
    let pitch = 3.0;
    let n = (inner.height() / pitch).floor().max(1.0) as usize;
    let lit = (level.clamp(0.0, 1.0) * n as f32).round() as usize;
    let peak_seg = (peak.clamp(0.0, 1.0) * n as f32).round() as usize;
    for i in 0..n {
        let y1 = inner.bottom() - i as f32 * pitch;
        let r = Rect::from_min_max(
            pos2(inner.left(), y1 - pitch + 1.0),
            pos2(inner.right(), y1),
        );
        let frac = i as f32 / n as f32;
        let c = if i < lit {
            if frac > 0.86 {
                theme::ORANGE
            } else if frac > 0.7 {
                Color32::from_rgb(0xff, 0xd2, 0xb8)
            } else {
                Color32::from_rgb(0xf2, 0xec, 0xe2)
            }
        } else if peak_seg > 0 && i + 1 == peak_seg {
            Color32::from_rgb(0xff, 0xc8, 0xa8)
        } else {
            Color32::from_rgb(0x2c, 0x2a, 0x27)
        };
        g.rect_filled(r, 0.0, c);
    }
}

/// [`paint_meter_v`] laid on its side (levels in headers and the control bar).
pub fn paint_meter_h(g: &Painter, rect: Rect, level: f32, peak: f32) {
    g.rect_filled(rect, 2.0, theme::METER_WELL);
    let inner = rect.shrink(2.0);
    let pitch = 3.0;
    let n = (inner.width() / pitch).floor().max(1.0) as usize;
    let lit = (level.clamp(0.0, 1.0) * n as f32).round() as usize;
    let peak_seg = (peak.clamp(0.0, 1.0) * n as f32).round() as usize;
    for i in 0..n {
        let x0 = inner.left() + i as f32 * pitch;
        let r = Rect::from_min_max(
            pos2(x0, inner.top()),
            pos2(x0 + pitch - 1.0, inner.bottom()),
        );
        let frac = i as f32 / n as f32;
        let c = if i < lit {
            if frac > 0.86 {
                theme::ORANGE
            } else {
                Color32::from_rgb(0xf2, 0xec, 0xe2)
            }
        } else if peak_seg > 0 && i + 1 == peak_seg {
            Color32::from_rgb(0xff, 0xc8, 0xa8)
        } else {
            Color32::from_rgb(0x2c, 0x2a, 0x27)
        };
        g.rect_filled(r, 0.0, c);
    }
}

// ---------------------------------------------------------------------------------------------
// Slots and fields.

/// How an insert/send/output slot looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    /// Nothing in it: a recess.
    Empty,
    /// Something loaded: a black cap (an LED shows whether it is on).
    Filled,
    /// A routing or setting: a bone cap.
    Plain,
}

/// An insert or routing slot with `label` (upper-cased). `led_on` draws the LED of a filled
/// slot (Some(on)); `centered` centres the label (routing slots).
pub fn paint_slot(
    g: &Painter,
    rect: Rect,
    kind: SlotKind,
    label: &str,
    led_on: Option<bool>,
    centered: bool,
    hovered: bool,
) {
    let radius = 3.0;
    let ink = match kind {
        SlotKind::Empty => {
            recess(g, rect, radius);
            if hovered {
                g.rect_stroke(
                    rect,
                    radius,
                    Stroke::new(1.0, theme::SEAM_DK),
                    egui::StrokeKind::Inside,
                );
            }
            theme::PRINT
        }
        SlotKind::Filled => {
            g.rect_filled(
                rect.translate(vec2(0.0, 1.0)),
                radius,
                Color32::from_white_alpha(150),
            );
            let top = if hovered {
                Color32::from_rgb(0x3c, 0x3a, 0x36)
            } else {
                Color32::from_rgb(0x2d, 0x2b, 0x28)
            };
            gradient(g, rect, radius, top, Color32::from_rgb(0x19, 0x18, 0x17));
            g.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, theme::CAP_EDGE),
                egui::StrokeKind::Inside,
            );
            g.hline(
                rect.left() + radius..=rect.right() - radius,
                rect.top() + 1.5,
                Stroke::new(1.0, Color32::from_white_alpha(22)),
            );
            theme::ON_DARK
        }
        SlotKind::Plain => {
            g.rect_filled(rect.translate(vec2(0.0, 1.0)), radius, theme::KEY_LIP);
            let top = if hovered {
                Color32::WHITE
            } else {
                theme::BONE_TOP
            };
            gradient(g, rect, radius, top, Color32::from_rgb(0xe8, 0xe4, 0xda));
            g.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, theme::KEY_EDGE),
                egui::StrokeKind::Inside,
            );
            theme::INK
        }
    };
    let mut x = rect.left() + 7.0;
    if let Some(on) = led_on {
        let c = pos2(x + 2.0, rect.center().y);
        if on {
            g.circle_filled(c, 4.0, Color32::from_rgba_unmultiplied(255, 90, 20, 60));
            g.circle_filled(c, 2.5, Color32::from_rgb(0xff, 0x6a, 0x22));
        } else {
            g.circle_filled(c, 2.5, Color32::from_rgb(0x4a, 0x47, 0x41));
        }
        x += 10.0;
    }
    if label.is_empty() {
        return;
    }
    let size = (rect.height() * 0.5).clamp(8.5, 11.0);
    let room = rect.right() - 6.0 - x;
    let shown = fit(g, &label.to_uppercase(), Face::Bold, size, room);
    if centered {
        text(
            g,
            pos2(
                rect.center().x + (x - rect.left() - 7.0) / 2.0,
                rect.center().y,
            ),
            Align2::CENTER_CENTER,
            &shown,
            Face::Bold,
            size,
            ink,
        );
    } else {
        text(
            g,
            pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            &shown,
            Face::Bold,
            size,
            ink,
        );
    }
}

/// A clickable [`paint_slot`].
#[allow(clippy::too_many_arguments)]
pub fn slot(
    ui: &Ui,
    g: &Painter,
    rect: Rect,
    id: Id,
    kind: SlotKind,
    label: &str,
    led_on: Option<bool>,
    centered: bool,
) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    paint_slot(g, rect, kind, label, led_on, centered, resp.hovered());
    resp
}

/// A dark value field (the inspector's values): black face, bone text, and up/down arrows at
/// the right when `stepper`.
pub fn paint_field(g: &Painter, rect: Rect, value: &str, stepper: bool, hovered: bool) {
    g.rect_filled(
        rect.translate(vec2(0.0, 1.0)),
        2.0,
        Color32::from_white_alpha(140),
    );
    let top = if hovered {
        Color32::from_rgb(0x30, 0x2e, 0x2b)
    } else {
        Color32::from_rgb(0x24, 0x22, 0x20)
    };
    gradient(g, rect, 2.0, top, Color32::from_rgb(0x16, 0x15, 0x14));
    g.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, theme::CAP_EDGE),
        egui::StrokeKind::Inside,
    );
    let room = rect.width() - if stepper { 26.0 } else { 12.0 };
    let shown = fit(g, &value.to_uppercase(), Face::Bold, 10.5, room);
    text(
        g,
        pos2(rect.left() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        &shown,
        Face::Bold,
        10.5,
        theme::ON_DARK,
    );
    if stepper {
        let c = pos2(rect.right() - 10.0, rect.center().y);
        let col = Color32::from_rgb(0x8b, 0x85, 0x78);
        g.add(Shape::convex_polygon(
            vec![
                c + vec2(-3.5, -1.5),
                c + vec2(0.0, -5.5),
                c + vec2(3.5, -1.5),
            ],
            col,
            Stroke::NONE,
        ));
        g.add(Shape::convex_polygon(
            vec![c + vec2(-3.5, 1.5), c + vec2(3.5, 1.5), c + vec2(0.0, 5.5)],
            col,
            Stroke::NONE,
        ));
    }
}

/// A small down triangle (menu titles).
pub fn tri_down(g: &Painter, c: Pos2, color: Color32) {
    g.add(Shape::convex_polygon(
        vec![
            c + vec2(-4.0, -2.0),
            c + vec2(4.0, -2.0),
            c + vec2(0.0, 3.0),
        ],
        color,
        Stroke::NONE,
    ));
}

/// A numbered track pad: a bone key with the number (orange key when `selected`).
pub fn track_pad(g: &Painter, rect: Rect, number: usize, selected: bool, hovered: bool) {
    let cap = if selected { Cap::Orange } else { Cap::Bone };
    let face = paint_key(g, rect, cap, false, hovered);
    let size = (rect.height() * 0.32).clamp(10.0, 15.0);
    text(
        g,
        face.center(),
        Align2::CENTER_CENTER,
        &format!("{number:02}"),
        Face::Bold,
        size,
        cap.ink(),
    );
}

/// A region: its body, a header bar in the track colour with a small kind tag ("IN", "AU",
/// "DR"; empty for none) and the name, and the outline. Returns the body under the header,
/// for the notes or the waveform.
pub fn paint_region(
    g: &Painter,
    rect: Rect,
    look: &theme::RegionLook,
    tag: &str,
    name: &str,
) -> Rect {
    let radius = 2.0;
    g.rect_filled(rect, radius, look.body);
    let header_h = if rect.height() >= 30.0 {
        13.0
    } else {
        (rect.height() * 0.4).min(11.0)
    };
    let header = Rect::from_min_size(rect.min, vec2(rect.width(), header_h));
    let cr = CornerRadius {
        nw: radius as u8,
        ne: radius as u8,
        sw: 0,
        se: 0,
    };
    g.rect_filled(header, cr, look.header);
    let clip = g.with_clip_rect(header.intersect(g.clip_rect()));
    let mut x = header.left() + 4.0;
    let size = (header_h * 0.72).clamp(7.0, 9.5);
    if !tag.is_empty() && header_h >= 10.0 && rect.width() > 40.0 {
        let gal = galley(&clip, tag, Face::Bold, size - 1.0, look.header);
        let tag_r = Rect::from_min_size(
            pos2(x, header.top() + 2.0),
            vec2(gal.size().x + 5.0, header_h - 4.0),
        );
        clip.rect_filled(tag_r, 1.0, look.header_ink);
        clip.galley(
            pos2(tag_r.left() + 2.5, tag_r.center().y - gal.size().y / 2.0),
            gal,
            look.header,
        );
        x = tag_r.right() + 5.0;
    }
    if header_h >= 8.0 {
        text(
            &clip,
            pos2(x, header.center().y),
            Align2::LEFT_CENTER,
            &name.to_uppercase(),
            Face::Bold,
            size,
            look.header_ink,
        );
    }
    g.rect_stroke(
        rect,
        radius,
        Stroke::new(
            if look.outline == theme::ORANGE {
                1.5
            } else {
                1.0
            },
            look.outline,
        ),
        egui::StrokeKind::Inside,
    );
    Rect::from_min_max(pos2(rect.left(), header.bottom()), rect.max)
}

/// A faceplate: a raised bone panel with a bevel (light top-left edge, seam at the bottom) and
/// a soft shadow under it; `lit_top` draws the orange strip a selected strip has along its top.
pub fn faceplate(g: &Painter, rect: Rect, radius: f32, lit_top: bool) {
    g.rect_filled(
        rect.translate(vec2(0.0, 2.0)).expand(0.5),
        radius + 0.5,
        Color32::from_black_alpha(28),
    );
    gradient(
        g,
        rect,
        radius,
        Color32::from_rgb(0xef, 0xec, 0xe4),
        Color32::from_rgb(0xe6, 0xe2, 0xd8),
    );
    g.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, theme::SEAM),
        egui::StrokeKind::Inside,
    );
    g.hline(
        rect.left() + radius..=rect.right() - radius,
        rect.top() + 1.5,
        Stroke::new(1.0, Color32::from_white_alpha(200)),
    );
    if lit_top {
        let strip = Rect::from_min_size(rect.min, vec2(rect.width(), 3.0));
        g.rect_filled(
            strip,
            CornerRadius {
                nw: radius as u8,
                ne: radius as u8,
                sw: 0,
                se: 0,
            },
            theme::ORANGE,
        );
    }
}

/// A clickable dark value field ([`paint_field`]); a dropdown when `stepper`.
pub fn field(ui: &Ui, g: &Painter, rect: Rect, id: Id, value: &str, stepper: bool) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    paint_field(g, rect, value, stepper, resp.hovered());
    resp
}

/// A clickable knob ([`paint_knob`]) dragged up/down; returns the response and the new value
/// (-1..1) while it is dragged. Double-click resets to `default`.
pub fn knob(
    ui: &Ui,
    g: &Painter,
    c: Pos2,
    r: f32,
    id: Id,
    value: f32,
    default: f32,
) -> (Response, Option<f32>) {
    let rect = Rect::from_center_size(c, Vec2::splat(r * 2.0 + 8.0));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let mut out = None;
    if resp.dragged() {
        let d = -resp.drag_delta().y + resp.drag_delta().x;
        let fine = if ui.input(|i| i.modifiers.shift) {
            0.002
        } else {
            0.01
        };
        out = Some((value + d * fine).clamp(-1.0, 1.0));
    }
    if resp.double_clicked() {
        out = Some(default);
    }
    paint_knob(g, c, r, out.unwrap_or(value));
    (resp, out)
}

/// A header's volume slider drawn without input (pictures).
pub fn slider_like(g: &Painter, rect: Rect) {
    paint_fader_h(g, rect, 0.7, vec2(9.0, 16.0));
}
