//! A way to try the editors without a window: egui is fed pointer events frame by frame, as
//! it would get them from the operating system, and the editors run in it. Nothing outside the
//! process is touched.
use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui, Vec2};

pub struct Harness {
    pub ctx: Context,
    time: f64,
    pub size: Vec2,
    /// The keys held down for the next frames (Shift, Option, Command…).
    pub modifiers: Modifiers,
}

impl Harness {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            ctx: Context::default(),
            time: 0.0,
            size: Vec2::new(width, height),
            modifiers: Modifiers::NONE,
        }
    }

    /// Runs one frame with `events`.
    pub fn frame(&mut self, events: Vec<Event>, mut ui: impl FnMut(&mut Ui)) {
        self.time += 1.0 / 60.0;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events: std::iter::once(Event::ModifiersChanged(self.modifiers))
                .chain(events)
                .collect(),
            ..RawInput::default()
        };
        let mut out = self.ctx.run_ui(input, |u| ui(u));
        // Nothing paints these frames, so the texture changes are not needed.
        out.textures_delta.clear();
    }

    /// Lets `secs` go by before the next frame (so two clicks are not a double-click).
    pub fn time_passes(&mut self, secs: f64) {
        self.time += secs;
    }

    /// A frame without input (widgets register where they are).
    pub fn settle(&mut self, mut ui: impl FnMut(&mut Ui)) {
        for _ in 0..2 {
            self.frame(vec![], &mut ui);
        }
    }

    fn button(&self, pos: Pos2, button: PointerButton, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: self.modifiers,
        }
    }

    /// A key pressed and let go, with the modifiers held.
    pub fn key(&mut self, key: egui::Key, ui: impl FnMut(&mut Ui)) {
        let m = self.modifiers;
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: m,
        };
        self.frame(vec![ev(true), ev(false)], ui);
    }

    pub fn move_to(&mut self, pos: Pos2, ui: impl FnMut(&mut Ui)) {
        self.frame(vec![Event::PointerMoved(pos)], ui);
    }

    /// Presses `button` at `pos` (after moving there), without letting go.
    pub fn press(&mut self, pos: Pos2, button: PointerButton, mut ui: impl FnMut(&mut Ui)) {
        self.move_to(pos, &mut ui);
        self.frame(vec![self.button(pos, button, true)], &mut ui);
    }

    pub fn release(&mut self, pos: Pos2, button: PointerButton, ui: impl FnMut(&mut Ui)) {
        self.frame(vec![self.button(pos, button, false)], ui);
    }

    /// A left click.
    pub fn click(&mut self, pos: Pos2, mut ui: impl FnMut(&mut Ui)) {
        self.press(pos, PointerButton::Primary, &mut ui);
        self.release(pos, PointerButton::Primary, &mut ui);
        self.frame(vec![], &mut ui);
    }

    pub fn right_click(&mut self, pos: Pos2, mut ui: impl FnMut(&mut Ui)) {
        self.press(pos, PointerButton::Secondary, &mut ui);
        self.release(pos, PointerButton::Secondary, &mut ui);
        self.frame(vec![], &mut ui);
    }

    /// Presses at `path[0]`, moves through the rest in steps, and lets go at the end.
    pub fn drag(&mut self, path: &[Pos2], button: PointerButton, mut ui: impl FnMut(&mut Ui)) {
        let (first, last) = (path[0], path[path.len() - 1]);
        self.press(first, button, &mut ui);
        for pt in &path[1..] {
            self.frame(vec![Event::PointerMoved(*pt)], &mut ui);
        }
        self.release(last, button, &mut ui);
        self.frame(vec![], &mut ui);
    }
}

/// Draws `ui` offscreen (a few frames, so layout settles) and writes the last frame to `path`
/// as a PNG at `scale` pixels per point. A small software rasterizer stands in for the GPU:
/// the tessellated triangles are filled with their vertex colours times the texture.
pub fn render_png(
    width: f32,
    height: f32,
    scale: f32,
    background: egui::Color32,
    mut ui: impl FnMut(&mut Ui),
    path: &std::path::Path,
) -> Result<(), String> {
    use egui::epaint::{ImageData, Primitive};
    use std::collections::HashMap;
    let ctx = Context::default();
    let mut textures: HashMap<egui::TextureId, (usize, usize, Vec<egui::Color32>)> = HashMap::new();
    let mut last = None;
    for i in 0..40 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height))),
            time: Some(f64::from(i) / 60.0),
            ..RawInput::default()
        };
        ctx.set_pixels_per_point(scale);
        let mut out = ctx.run_ui(input, |u| ui(u));
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                let ImageData::Color(img) = &delta.image;
                let [w, h] = img.size;
                match delta.pos {
                    None => {
                        textures.insert(*id, (w, h, img.pixels.clone()));
                    }
                    Some([x0, y0]) => {
                        if let Some((tw, _, px)) = textures.get_mut(id) {
                            for y in 0..h {
                                for x in 0..w {
                                    px[(y0 + y) * *tw + x0 + x] = img.pixels[y * w + x];
                                }
                            }
                        }
                    }
                }
            }
        }
        out.textures_delta.clear();
        last = Some((out.shapes, out.pixels_per_point));
    }
    let (shapes, ppp) = last.ok_or("no frame")?;
    let prims = ctx.tessellate(shapes, ppp);
    let (pw, ph) = ((width * ppp) as usize, (height * ppp) as usize);
    let bg = background.to_array().map(|c| f32::from(c) / 255.0);
    let mut img = vec![bg; pw * ph];
    for cp in prims {
        let Primitive::Mesh(mesh) = cp.primitive else {
            continue;
        };
        let clip = cp.clip_rect;
        let (cx0, cy0) = ((clip.min.x * ppp).max(0.0), (clip.min.y * ppp).max(0.0));
        let (cx1, cy1) = (
            (clip.max.x * ppp).min(pw as f32),
            (clip.max.y * ppp).min(ph as f32),
        );
        let tex = textures.get(&mesh.texture_id);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [
                &mesh.vertices[tri[0] as usize],
                &mesh.vertices[tri[1] as usize],
                &mesh.vertices[tri[2] as usize],
            ];
            let p: Vec<(f32, f32)> = v.iter().map(|v| (v.pos.x * ppp, v.pos.y * ppp)).collect();
            let area =
                (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
            if area.abs() < 1e-6 {
                continue;
            }
            let minx = p
                .iter()
                .map(|q| q.0)
                .fold(f32::MAX, f32::min)
                .max(cx0)
                .floor() as i64;
            let maxx = p
                .iter()
                .map(|q| q.0)
                .fold(f32::MIN, f32::max)
                .min(cx1)
                .ceil() as i64;
            let miny = p
                .iter()
                .map(|q| q.1)
                .fold(f32::MAX, f32::min)
                .max(cy0)
                .floor() as i64;
            let maxy = p
                .iter()
                .map(|q| q.1)
                .fold(f32::MIN, f32::max)
                .min(cy1)
                .ceil() as i64;
            for y in miny.max(0)..maxy.min(ph as i64) {
                for x in minx.max(0)..maxx.min(pw as i64) {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    if fx < cx0 || fx >= cx1 || fy < cy0 || fy >= cy1 {
                        continue;
                    }
                    let w0 = ((p[1].0 - fx) * (p[2].1 - fy) - (p[2].0 - fx) * (p[1].1 - fy)) / area;
                    let w1 = ((p[2].0 - fx) * (p[0].1 - fy) - (p[0].0 - fx) * (p[2].1 - fy)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let mut src = [0.0f32; 4];
                    for (k, s) in src.iter_mut().enumerate() {
                        *s = (f32::from(v[0].color.to_array()[k]) * w0
                            + f32::from(v[1].color.to_array()[k]) * w1
                            + f32::from(v[2].color.to_array()[k]) * w2)
                            / 255.0;
                    }
                    if let Some((tw, th, px)) = tex {
                        let u = v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2;
                        let vv = v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2;
                        let t = sample(*tw, *th, px, u, vv);
                        for k in 0..4 {
                            src[k] *= t[k];
                        }
                    }
                    let d = &mut img[y as usize * pw + x as usize];
                    let a = src[3];
                    for k in 0..4 {
                        d[k] = src[k] + d[k] * (1.0 - a);
                    }
                }
            }
        }
    }
    let mut out = image::RgbaImage::new(pw as u32, ph as u32);
    for (i, px) in img.iter().enumerate() {
        let c = px.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        out.put_pixel(
            (i % pw) as u32,
            (i / pw) as u32,
            image::Rgba([c[0], c[1], c[2], 255]),
        );
    }
    out.save(path).map_err(|e| e.to_string())
}

/// Bilinear texel lookup (premultiplied RGBA, 0..1).
fn sample(w: usize, h: usize, px: &[egui::Color32], u: f32, v: f32) -> [f32; 4] {
    let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let get = |x: usize, y: usize| px[y * w + x].to_array().map(|c| f32::from(c) / 255.0);
    let (a, b, c, d) = (get(x0, y0), get(x1, y0), get(x0, y1), get(x1, y1));
    let mut o = [0.0; 4];
    for k in 0..4 {
        o[k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
    }
    o
}
