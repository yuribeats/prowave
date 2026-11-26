//! `prowave --control-headless`: the app without a window, for trying the control interface.
//!
//! The same `App` runs its frames against an egui context that nothing shows: requests come in
//! over the socket and are served as in the windowed app. There is no menu bar, no plug-in
//! window and, unless `--audio-output` names a device, no audio engine (the playhead then
//! runs on the frame clock, as the app does when it has no sound card). Pictures
//! (`ui.screenshot`) are drawn by a small software renderer instead of the GPU, so text and
//! shapes are there but not as sharp as on screen.
//!
//!     prowave --control-headless [--socket PATH] [--size WIDTHxHEIGHT] [--audio-output UID]
//!
//! It ends on `app.quit`, on SIGTERM / SIGINT, or when its standard input closes with
//! `--exit-on-stdin-close`. It leaves the user's settings alone: no recovery copies, no
//! recent-projects list.
use super::m_ui;
use super::protocol::RpcError;
use crate::App;
use egui::epaint::{ImageData, Primitive};
use egui::{Pos2, RawInput, Rect, Vec2};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Release);
}

/// The app as the window makes it, without the things only a window has.
pub(crate) fn app_without_window(sync: Option<crate::engine_sync::EngineSync>) -> App {
    let project = crate::model::Project::empty();
    App {
        saved: crate::file_commands::content(&project),
        history: crate::undo::History::new(&project),
        project,
        ui: crate::UiState::new(),
        sync,
        plugins: crate::plugins::PluginHost::default(),
        path: None,
        native_menu: None,
        bench: None,
        shots: None,
        frames_since_record: 0,
        midi: None,
        midi_out: None,
        mtc_in: None,
        clock_in: crate::clock_in::ClockIn::default(),
        clock_follow: crate::clock_follow::ClockFollower::default(),
        chase: crate::mtc_in::Chase::default(),
        autosave: crate::autosave::Autosave::off(),
        control: None,
        updater: crate::updater::Updater::new(),
    }
}

fn value_of(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Runs the app without a window until it is told to quit. Returns the exit code.
pub fn run(args: &[String]) -> i32 {
    crate::i18n::init();
    let size = value_of(args, "--size")
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some(Vec2::new(w.parse().ok()?, h.parse().ok()?))
        })
        .filter(|v| v.x >= 400.0 && v.y >= 300.0 && v.x <= 8000.0 && v.y <= 8000.0)
        .unwrap_or(Vec2::new(1600.0, 980.0));
    let socket = value_of(args, "--socket")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os(super::SOCKET_VAR).map(std::path::PathBuf::from));
    let Some(socket) = socket else {
        eprintln!(
            "--control-headless needs --socket PATH (or {}): it never takes the app's own socket",
            super::SOCKET_VAR
        );
        return 2;
    };
    let project = crate::model::Project::empty();
    let sync = match value_of(args, "--audio-output") {
        Some(uid) => match prowave_engine::Engine::start_with_devices(None, Some(uid), None) {
            Ok(engine) => Some(crate::engine_sync::EngineSync::new(engine, &project)),
            Err(e) => {
                eprintln!("audio: {e}");
                return 1;
            }
        },
        None => None,
    };
    let mut app = app_without_window(sync);
    app.autosave.quit_confirmed = true;
    app.ui.mixer.au_list = crate::plugins::choices();
    let mut control = match super::Control::start_at(&socket, false) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("control socket: {e}");
            return 1;
        }
    };
    control.core.headless = true;
    app.control = Some(control);

    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler only stores to an atomic, which is safe in a signal handler.
    unsafe {
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
    if args.iter().any(|a| a == "--exit-on-stdin-close") {
        std::thread::spawn(|| {
            let mut sink = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut sink);
            STOP.store(true, Ordering::Release);
        });
    }

    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    // A request arriving wakes the loop, as it wakes the window.
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
    ctx.set_request_repaint_callback(move |_| {
        let _ = wake_tx.send(());
    });
    let mut frame = eframe::Frame::_new_kittest();
    let mut textures = Textures::default();
    let started = Instant::now();
    println!("prowave headless: control socket at {}", socket.display());
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    loop {
        if STOP.load(Ordering::Acquire) {
            break;
        }
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(started.elapsed().as_secs_f64()),
            ..RawInput::default()
        };
        eframe::App::logic(&mut app, &ctx, &mut frame);
        let mut out = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        textures.take(&out.textures_delta);
        out.textures_delta.clear();
        let mut quit = false;
        if let Some(control) = app.control.as_mut() {
            for shot in std::mem::take(&mut control.core.shots) {
                let result = draw(
                    &ctx,
                    &textures,
                    out.shapes.clone(),
                    out.pixels_per_point,
                    size,
                )
                .and_then(|img| m_ui::save_rgba(img, &shot.path, shot.scale))
                .map(|dims| m_ui::shot_result(&shot.path, dims, "main"))
                .map_err(RpcError::failed);
                shot.done.store(true, Ordering::Release);
                if let Some(id) = &shot.id {
                    control.core.shared.respond(shot.conn, id, &result);
                }
            }
            quit = control.core.quit;
        }
        let root = out.viewport_output.get(&egui::ViewportId::ROOT);
        if root.is_some_and(|v| {
            v.commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Close))
        }) {
            quit = true;
        }
        if quit {
            break;
        }
        // Until the next frame is due, or something asks for one.
        let wait = root
            .map_or(Duration::from_millis(100), |v| v.repaint_delay)
            .clamp(Duration::from_millis(4), Duration::from_millis(200));
        let _ = wake_rx.recv_timeout(wait);
        while wake_rx.try_recv().is_ok() {}
    }
    // The app goes with its engine and its socket (which removes the socket file).
    drop(app);
    0
}

/// The textures egui has handed out so far (the font atlas and the app's images).
#[derive(Default)]
struct Textures(HashMap<egui::TextureId, (usize, usize, Vec<egui::Color32>)>);

impl Textures {
    fn take(&mut self, delta: &egui::TexturesDelta) {
        for (id, deltas) in &delta.set {
            for d in deltas {
                let ImageData::Color(img) = &d.image;
                let [w, h] = img.size;
                match d.pos {
                    None => {
                        self.0.insert(*id, (w, h, img.pixels.clone()));
                    }
                    Some([x0, y0]) => {
                        if let Some((tw, th, px)) = self.0.get_mut(id) {
                            for y in 0..h {
                                for x in 0..w {
                                    if x0 + x < *tw && y0 + y < *th {
                                        px[(y0 + y) * *tw + x0 + x] = img.pixels[y * w + x];
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.0.remove(id);
        }
    }
}

/// Draws a frame's shapes into a picture: the tessellated triangles are filled with their
/// vertex colours times the texture (the same small renderer as the tests' `ui_test.rs`).
fn draw(
    ctx: &egui::Context,
    textures: &Textures,
    shapes: Vec<egui::epaint::ClippedShape>,
    ppp: f32,
    size: Vec2,
) -> Result<image::RgbaImage, String> {
    let prims = ctx.tessellate(shapes, ppp);
    let (pw, ph) = ((size.x * ppp) as usize, (size.y * ppp) as usize);
    if pw == 0 || ph == 0 {
        return Err("the picture would have no pixels".into());
    }
    let bg = crate::theme::BG().to_array().map(|c| f32::from(c) / 255.0);
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
        let tex = textures.0.get(&mesh.texture_id);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (
                mesh.vertices.get(tri[0] as usize),
                mesh.vertices.get(tri[1] as usize),
                mesh.vertices.get(tri[2] as usize),
            ) else {
                continue;
            };
            let v = [a, b, c];
            let p = v.map(|v| (v.pos.x * ppp, v.pos.y * ppp));
            let area =
                (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
            if area.abs() < 1e-6 {
                continue;
            }
            let fold = |f: fn(f32, f32) -> f32, init: f32, pick: fn(&(f32, f32)) -> f32| {
                p.iter().map(pick).fold(init, f)
            };
            let minx = fold(f32::min, f32::MAX, |q| q.0).max(cx0).floor() as i64;
            let maxx = fold(f32::max, f32::MIN, |q| q.0).min(cx1).ceil() as i64;
            let miny = fold(f32::min, f32::MAX, |q| q.1).max(cy0).floor() as i64;
            let maxy = fold(f32::max, f32::MIN, |q| q.1).min(cy1).ceil() as i64;
            let colors = v.map(|v| v.color.to_array().map(f32::from));
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
                        *s = (colors[0][k] * w0 + colors[1][k] * w1 + colors[2][k] * w2) / 255.0;
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
                    let alpha = src[3];
                    for k in 0..4 {
                        d[k] = src[k] + d[k] * (1.0 - alpha);
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
    Ok(out)
}

/// Bilinear texel lookup (premultiplied RGBA, 0..1).
fn sample(w: usize, h: usize, px: &[egui::Color32], u: f32, v: f32) -> [f32; 4] {
    if w == 0 || h == 0 || px.len() < w * h {
        return [1.0; 4];
    }
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
