//! Hardware parts used by the control bar, the menus and the Project Chooser that the shared
//! kit (`hw.rs`) does not have: heavier seven-segment digits for the main LCD, a section
//! bracket whose label follows the key size, the PW-1 wordmark, and the key-command glyphs
//! drawn as shapes so they match the mono print whatever font is installed.
use crate::hw;
use crate::theme::{self, Face};
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Shape, Stroke};

/// Seven segments of `c` (a top, b upper right, c lower right, d bottom, e lower left, f upper
/// left, g middle), for the characters the LCD shows.
fn segments(c: char) -> Option<u8> {
    Some(match c {
        '0' => 0b0111111,
        '1' => 0b0000110,
        '2' => 0b1011011,
        '3' => 0b1001111,
        '4' => 0b1100110,
        '5' => 0b1101101,
        '6' => 0b1111101,
        '7' => 0b0000111,
        '8' => 0b1111111,
        '9' => 0b1101111,
        '-' => 0b1000000,
        _ => return None,
    })
}

/// `s` in seven-segment digits `h` tall with segments `thick`·h thick (the mockup's main LCD
/// reads heavier than the kit's 0.12), its top left at `pos`. Same advances as `hw::seg`, so
/// `hw::seg_width` measures it. Unlit segments show as ghosts when `ghost`. Returns the width.
pub fn seg(g: &Painter, pos: Pos2, s: &str, h: f32, on: Color32, ghost: bool, thick: f32) -> f32 {
    let mut x = pos.x;
    let w = h * 0.5;
    let t = (h * thick).max(1.7);
    let gap = (t * 0.22).max(0.5);
    let slant = 0.105;
    let skew = |p: Pos2| pos2(p.x + (pos.y + h - p.y) * slant, p.y);
    for ch in s.chars() {
        if let Some(bits) = segments(ch) {
            let (x0, x1) = (x, x + w);
            let (y0, ym, y1) = (pos.y, pos.y + h / 2.0, pos.y + h);
            let ht = t / 2.0;
            let hseg = |y: f32| {
                let (a, b) = (x0 + gap, x1 - gap);
                vec![
                    pos2(a, y),
                    pos2(a + ht, y - ht),
                    pos2(b - ht, y - ht),
                    pos2(b, y),
                    pos2(b - ht, y + ht),
                    pos2(a + ht, y + ht),
                ]
            };
            let vseg = |xc: f32, ya: f32, yb: f32| {
                let (a, b) = (ya + gap, yb - gap);
                vec![
                    pos2(xc, a),
                    pos2(xc + ht, a + ht),
                    pos2(xc + ht, b - ht),
                    pos2(xc, b),
                    pos2(xc - ht, b - ht),
                    pos2(xc - ht, a + ht),
                ]
            };
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
                let c = if lit { on } else { theme::LCD_GHOST };
                g.add(Shape::convex_polygon(
                    poly.into_iter().map(skew).collect(),
                    c,
                    Stroke::NONE,
                ));
            }
        } else {
            let sq = |cx: f32, cy: f32| {
                let r = Rect::from_center_size(pos2(cx, cy), vec2(t, t));
                g.add(Shape::convex_polygon(
                    vec![
                        skew(r.left_top()),
                        skew(r.right_top()),
                        skew(r.right_bottom()),
                        skew(r.left_bottom()),
                    ],
                    on,
                    Stroke::NONE,
                ));
            };
            match ch {
                '.' | ',' => sq(x + h * 0.09, pos.y + h - t / 2.0),
                ':' => {
                    sq(x + h * 0.09, pos.y + h * 0.3);
                    sq(x + h * 0.09, pos.y + h * 0.7);
                }
                '/' => {
                    let (a, b) = (x + h * 0.04, x + h * 0.34);
                    g.add(Shape::convex_polygon(
                        vec![
                            skew(pos2(a, pos.y + h)),
                            skew(pos2(b - t * 0.8, pos.y)),
                            skew(pos2(b, pos.y)),
                            skew(pos2(a + t * 0.8, pos.y + h)),
                        ],
                        on,
                        Stroke::NONE,
                    ));
                }
                ' ' => {}
                _ => {
                    hw::text(
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
        x += advance(ch, h);
    }
    x - pos.x
}

/// Advance of one character (as `hw::seg`).
fn advance(c: char, h: f32) -> f32 {
    match c {
        '.' | ':' | ',' => h * 0.24,
        ' ' => h * 0.3,
        '/' => h * 0.42,
        _ => h * 0.67,
    }
}

/// A section bracket over a group (`|— VIEW ——————|`) with its label set at `size`, so the
/// silkscreen grows with the keys under it.
pub fn bracket(g: &Painter, left: f32, right: f32, y: f32, label: &str, size: f32) {
    let ink = theme::INK2.gamma_multiply(0.6);
    let st = Stroke::new(1.0, ink);
    let tick = (size * 0.9).round();
    g.hline(left..=left + tick, y, st);
    let r = hw::silk(
        g,
        pos2(left + tick + 6.0, y),
        Align2::LEFT_CENTER,
        label,
        size,
        theme::INK2,
    );
    if r.right() + 6.0 < right {
        g.hline(r.right() + 6.0..=right, y, st);
    }
    g.vline(left, y..=y + 7.0, st);
    g.vline(right, y..=y + 7.0, st);
}

/// The PW-1 wordmark: PRO WAVE in the heavy silkscreen, the orange square and the model line,
/// its left edge at `left`, centred on `cy`. Returns its right edge.
pub fn wordmark(g: &Painter, left: f32, cy: f32, name: &str, model: &str) -> f32 {
    let r = hw::text(
        g,
        pos2(left, cy),
        Align2::LEFT_CENTER,
        &name.to_uppercase(),
        Face::SilkX,
        15.0,
        theme::INK,
    );
    let sq = Rect::from_center_size(pos2(r.right() + 18.0, cy), vec2(10.0, 10.0));
    g.rect_filled(sq, 1.0, theme::ORANGE);
    let m = hw::silk(
        g,
        pos2(sq.right() + 10.0, cy + 0.5),
        Align2::LEFT_CENTER,
        model,
        10.0,
        theme::INK2,
    );
    m.right()
}

/// The Command key's sign (⌘) `size` points square around `c`: four loops round a square.
pub fn command_glyph(g: &Painter, c: Pos2, size: f32, color: Color32) {
    let s = size / 12.0;
    let st = Stroke::new((1.15 * s).max(1.0), color);
    let k = 2.2 * s;
    g.rect_stroke(
        Rect::from_center_size(c, vec2(2.0 * k, 2.0 * k)),
        0.0,
        st,
        egui::StrokeKind::Middle,
    );
    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        g.circle_stroke(
            c + vec2(dx * (k + 1.6 * s), dy * (k + 1.6 * s)),
            1.6 * s,
            st,
        );
    }
}

/// The Shift key's sign (⇧) `size` points square around `c`.
pub fn shift_glyph(g: &Painter, c: Pos2, size: f32, color: Color32) {
    let s = size / 12.0;
    let p = |x: f32, y: f32| c + vec2(x * s, y * s);
    g.add(Shape::closed_line(
        vec![
            p(0.0, -5.0),
            p(5.0, 0.5),
            p(2.2, 0.5),
            p(2.2, 4.5),
            p(-2.2, 4.5),
            p(-2.2, 0.5),
            p(-5.0, 0.5),
        ],
        Stroke::new((1.1 * s).max(1.0), color),
    ));
}

/// A key command as text away from a Mac: "CTRL+SHIFT+Z".
fn shortcut_text(k: &egui::KeyboardShortcut) -> String {
    let mut s = String::new();
    if k.modifiers.command || k.modifiers.ctrl {
        s.push_str("Ctrl+");
    }
    if k.modifiers.alt {
        s.push_str("Alt+");
    }
    if k.modifiers.shift {
        s.push_str("Shift+");
    }
    s.push_str(k.logical_key.symbol_or_name());
    s.to_uppercase()
}

/// Width of a key command as [`shortcut`] prints it.
pub fn shortcut_width(g: &Painter, k: &egui::KeyboardShortcut, mac: bool, size: f32) -> f32 {
    let w = |s: &str| hw::galley(g, s, Face::Bold, size, theme::INK).size().x;
    if !mac {
        return w(&shortcut_text(k));
    }
    let signs =
        usize::from(k.modifiers.command || k.modifiers.mac_cmd) + usize::from(k.modifiers.shift);
    w(&k.logical_key.symbol_or_name().to_uppercase()) + 2.0 + signs as f32 * (size * 0.95 + 2.0)
}

/// A key command printed right-aligned at `right` (its vertical centre): on a Mac the ⇧ and ⌘
/// signs as shapes, then the key's name in the mono face; elsewhere "Ctrl+Shift+Z". All in
/// one face and on one baseline. Returns its width.
pub fn shortcut(
    g: &Painter,
    right: Pos2,
    k: &egui::KeyboardShortcut,
    mac: bool,
    size: f32,
    color: Color32,
) -> f32 {
    let key = k.logical_key.symbol_or_name();
    if !mac {
        let r = hw::text(
            g,
            right,
            Align2::RIGHT_CENTER,
            &shortcut_text(k),
            Face::Bold,
            size,
            color,
        );
        return r.width();
    }
    let r = hw::text(
        g,
        right,
        Align2::RIGHT_CENTER,
        &key.to_uppercase(),
        Face::Bold,
        size,
        color,
    );
    let mut x = r.left() - 2.0;
    let gs = size * 0.95;
    if k.modifiers.command || k.modifiers.mac_cmd {
        command_glyph(g, pos2(x - gs / 2.0, right.y), gs, color);
        x -= gs + 2.0;
    }
    if k.modifiers.shift {
        shift_glyph(g, pos2(x - gs / 2.0, right.y), gs, color);
        x -= gs + 2.0;
    }
    right.x - x
}
