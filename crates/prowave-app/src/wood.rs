//! Walnut end cheeks down both sides of the main window, like the wooden side panels of a
//! vintage synth (View > Wood Cheeks). The grain is made here, once, by code: a base of walnut
//! browns, rings bent along the length of the board, darker streaks and fine pores, in a strip
//! that repeats seamlessly from top to bottom. Each cheek has a dark seam on its inner edge, a
//! shade toward its outer edge and a slotted screw near the top. Narrow windows go without them.
use egui::{pos2, vec2, Color32, ColorImage, Painter, Pos2, Rect, Stroke, TextureHandle};

/// Width of one cheek, in points.
pub const CHEEK_W: f32 = 24.0;
/// Below this window width the cheeks are left out, so they never take room the work needs.
pub const MIN_WINDOW_W: f32 = 1000.0;
/// The grain is made at this many pixels per point (sharp on Retina screens).
const DENSITY: usize = 2;
/// Height of one tile of grain, in points (it repeats without a seam).
const TILE_H: f32 = 512.0;

/// Whether the cheeks are drawn in a window `width` points wide.
pub fn shown(width: f32) -> bool {
    crate::theme::wood() && width >= MIN_WINDOW_W
}

/// Adds the left and right cheeks as side panels of `ui` (before every other panel, so they
/// run the full height of the window).
pub fn cheeks(ui: &mut egui::Ui) {
    if !shown(ui.ctx().content_rect().width()) {
        return;
    }
    for (id, left) in [("wood_cheek_left", true), ("wood_cheek_right", false)] {
        let panel = if left {
            egui::Panel::left(id)
        } else {
            egui::Panel::right(id)
        };
        panel
            .exact_size(CHEEK_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                let rect = ui.max_rect();
                paint_cheek(ui.ctx(), &ui.painter_at(rect), rect, left);
            });
    }
}

/// One cheek in `rect`; `left` puts its inner edge on the right.
pub fn paint_cheek(ctx: &egui::Context, g: &Painter, rect: Rect, left: bool) {
    let tex = texture(ctx);
    let mut y = rect.top();
    // Each cheek starts the grain at a different point, so the two sides do not mirror.
    let mut v0 = if left { 0.0 } else { 0.43 };
    while y < rect.bottom() {
        let h = ((1.0 - v0) * TILE_H).min(rect.bottom() - y);
        let v1 = v0 + h / TILE_H;
        g.image(
            tex.id(),
            Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), h)),
            Rect::from_min_max(pos2(0.0, v0), pos2(1.0, v1)),
            Color32::WHITE,
        );
        y += h;
        v0 = 0.0;
    }
    // Shade toward the outer edge, a soft shadow on the inner one, and the dark seam where the
    // wood meets the chassis.
    let (outer, inner) = if left {
        (rect.left(), rect.right())
    } else {
        (rect.right(), rect.left())
    };
    let dir = if left { 1.0 } else { -1.0 };
    for i in 0..5 {
        let a = [115u8, 80, 50, 28, 12][i];
        let x = outer + dir * (i as f32 + 0.5);
        g.vline(
            x,
            rect.y_range(),
            Stroke::new(1.0, Color32::from_black_alpha(a)),
        );
    }
    for i in 0..3 {
        let a = [90u8, 45, 18][i];
        let x = inner - dir * (i as f32 + 1.5);
        g.vline(
            x,
            rect.y_range(),
            Stroke::new(1.0, Color32::from_black_alpha(a)),
        );
    }
    g.vline(
        inner - dir * 0.5,
        rect.y_range(),
        Stroke::new(1.0, Color32::from_rgb(0x1a, 0x0e, 0x07)),
    );
    // A highlight along the top of the board's face.
    g.vline(
        outer + dir * 6.5,
        rect.y_range(),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 220, 170, 10)),
    );
    screw(
        g,
        pos2(rect.center().x, rect.top() + 14.0),
        3.6,
        if left { 35.0 } else { -20.0 },
    );
}

/// A small nickel screw head set into the wood.
fn screw(g: &Painter, c: Pos2, r: f32, angle: f32) {
    g.circle_filled(c + vec2(0.0, 0.8), r + 0.6, Color32::from_black_alpha(110));
    g.circle_filled(c, r, Color32::from_rgb(0x6e, 0x6a, 0x62));
    g.circle_filled(
        c + vec2(-r * 0.22, -r * 0.25),
        r * 0.68,
        Color32::from_rgb(0xb9, 0xb4, 0xa9),
    );
    g.circle_stroke(c, r, Stroke::new(0.8, Color32::from_black_alpha(140)));
    let a = angle.to_radians();
    let d = vec2(a.cos(), a.sin()) * (r * 0.78);
    g.line_segment(
        [c - d, c + d],
        Stroke::new(1.2, Color32::from_rgb(0x2a, 0x27, 0x22)),
    );
}

/// The grain texture, made on first use and kept in the context.
fn texture(ctx: &egui::Context) -> TextureHandle {
    let id = egui::Id::unique("prowave_walnut_grain");
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return t;
    }
    let (w, h) = (CHEEK_W as usize * DENSITY, TILE_H as usize * DENSITY);
    let img = ColorImage::new([w, h], grain(w, h));
    let t = ctx.load_texture("walnut-grain", img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// A small integer hash to 0..1.
fn hash(x: i64, y: i64, seed: u32) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ u64::from(seed).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h & 0xFF_FFFF) as f32 / 16_777_215.0
}

/// Smooth value noise along one axis at `t`, repeating every `period` units.
fn noise1(t: f32, period: i64, seed: u32) -> f32 {
    let i = t.floor() as i64;
    let f = t - t.floor();
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash(i.rem_euclid(period), 0, seed);
    let b = hash((i + 1).rem_euclid(period), 0, seed);
    a + (b - a) * s
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// The walnut pixels, `w` by `h`, seamless from the last row to the first.
fn grain(w: usize, h: usize) -> Vec<Color32> {
    use std::f32::consts::TAU;
    let dark = [0x4f as f32, 0x29 as f32, 0x15 as f32];
    let light = [0x7a as f32, 0x45 as f32, 0x24 as f32];
    let streak = [0x2e as f32, 0x17 as f32, 0x0b as f32];
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            // The board's rings, bent slowly along its length (every term repeats over v).
            let bend = 0.55 * (TAU * (v * 2.0 + 0.15)).sin()
                + 0.25 * (TAU * (v * 5.0) + 1.3).sin()
                + 0.35 * (noise1(v * 12.0, 12, 7) - 0.5);
            let t = u * 3.2 + bend + 0.6 * noise1(u * 6.0 + v * 3.0, 3, 3);
            let ring = (TAU * t).sin() * 0.5 + 0.5;
            // Broad light and dark bands, then the narrow dark streaks between rings.
            let band = noise1(v * 6.0 + u * 1.5, 6, 11) * 0.6 + ring * 0.4;
            let mut c = mix(dark, light, band);
            let line = ((TAU * t * 2.0).sin() * 0.5 + 0.5).powf(14.0);
            c = mix(c, streak, line * 0.55);
            // Pores: short dark dashes running with the grain.
            let pore = hash(x as i64, ((y / 5) as i64).rem_euclid((h / 5) as i64), 5);
            let fine = hash(x as i64, y as i64, 9);
            let k = 1.0 - 0.10 * pore.powf(3.0) - 0.05 * fine + 0.03 * (1.0 - pore);
            px.push(Color32::from_rgb(
                (c[0] * k).clamp(0.0, 255.0) as u8,
                (c[1] * k).clamp(0.0, 255.0) as u8,
                (c[2] * k).clamp(0.0, 255.0) as u8,
            ));
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grain_repeats_without_a_seam_and_stays_walnut() {
        let (w, h) = (48, 1024);
        let px = grain(w, h);
        // The last row runs into the first as smoothly as any two rows next to each other.
        let row_diff = |a: usize, b: usize| -> f32 {
            (0..w)
                .map(|x| {
                    let (p, q) = (px[a * w + x].to_array(), px[b * w + x].to_array());
                    (0..3)
                        .map(|k| (f32::from(p[k]) - f32::from(q[k])).abs())
                        .sum::<f32>()
                })
                .sum::<f32>()
                / w as f32
        };
        let typical: f32 = (1..h).map(|y| row_diff(y - 1, y)).sum::<f32>() / (h - 1) as f32;
        assert!(row_diff(h - 1, 0) < typical * 3.0 + 6.0);
        // Every pixel is a warm brown: red over green over blue, and dark.
        for p in &px {
            let [r, g, b, _] = p.to_array();
            assert!(r >= g && g >= b && r < 0x90, "{p:?}");
        }
    }

    #[test]
    fn narrow_windows_go_without_cheeks() {
        crate::theme::set_wood(true);
        assert!(!shown(MIN_WINDOW_W - 1.0));
        assert!(shown(MIN_WINDOW_W));
    }
}
