//! Show Spectrogram (an audio region's right-click menu): a window with the region's
//! spectrogram, time left to right, frequency bottom to top and loudness as colour, so a
//! picture painted into audio (Paint Image to Audio) can be seen, and any recording looked
//! into. The spectrum is measured on a worker thread (`prowave_engine::spectrogram`, 2048-point
//! Hann frames); the frequency axis (logarithmic or linear), the colours and the range shown
//! are changed at once, from the measurement already made.
use crate::i18n::{tr, trf};
use crate::model::Project;
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke};
use prowave_engine::Spectrogram;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

/// Rows of the drawn picture (it is stretched to the window).
const ROWS: usize = 512;
/// At most this many frames across: a long region is looked at in steps further apart.
const MAX_FRAMES: usize = 3000;
/// The lowest frequency of the logarithmic axis, in Hz.
const LOG_LOW_HZ: f32 = 20.0;

/// How levels are coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colors {
    Magma,
    Viridis,
    Grey,
}

impl Colors {
    const ALL: [Colors; 3] = [Colors::Magma, Colors::Viridis, Colors::Grey];

    fn name(self) -> &'static str {
        match self {
            Colors::Magma => tr("Magma"),
            Colors::Viridis => tr("Viridis"),
            Colors::Grey => tr("Grey"),
        }
    }

    /// The colour of `t`, from 0 (quietest shown) to 1 (loudest).
    pub fn at(self, t: f32) -> Color32 {
        let stops: &[[u8; 3]] = match self {
            Colors::Magma => &[
                [0, 0, 4],
                [59, 15, 112],
                [140, 41, 129],
                [222, 73, 104],
                [254, 159, 109],
                [252, 253, 191],
            ],
            Colors::Viridis => &[
                [68, 1, 84],
                [65, 68, 135],
                [42, 120, 142],
                [34, 168, 132],
                [122, 209, 81],
                [253, 231, 37],
            ],
            Colors::Grey => &[[0, 0, 0], [255, 255, 255]],
        };
        let t = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
        let i = (t.floor() as usize).min(stops.len() - 2);
        let f = t - i as f32;
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * f).round() as u8;
        let (a, b) = (stops[i], stops[i + 1]);
        Color32::from_rgb(mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]))
    }
}

/// What the picture is drawn with: when one of these changes it is drawn again.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    log: bool,
    colors: Colors,
    range_db: f32,
}

/// The open Spectrogram window.
struct Window {
    title: String,
    /// The measurement while it is being made.
    pending: Option<Receiver<Result<Spectrogram, String>>>,
    spec: Option<Spectrogram>,
    error: Option<String>,
    look: Look,
    texture: Option<(Look, egui::TextureHandle)>,
}

/// The Spectrogram window, while one is open.
#[derive(Default)]
pub struct SpectrogramView {
    window: Option<Window>,
}

/// The frequencies shown from the bottom to the top of the axis.
fn axis(spec: &Spectrogram, log: bool) -> (f32, f32) {
    let top = spec.sample_rate as f32 / 2.0;
    if log {
        (LOG_LOW_HZ.min(top / 2.0), top)
    } else {
        (0.0, top)
    }
}

/// The frequency at height `v` of the axis (0 bottom, 1 top).
fn hz_at(low: f32, high: f32, log: bool, v: f32) -> f32 {
    if log {
        low * (high / low).powf(v)
    } else {
        low + (high - low) * v
    }
}

/// Where frequency `hz` is up the axis (0 bottom, 1 top).
fn height_of(low: f32, high: f32, log: bool, hz: f32) -> f32 {
    if log {
        (hz.max(1e-3) / low).ln() / (high / low).ln()
    } else {
        (hz - low) / (high - low)
    }
}

/// The level of one row (`bins` lo..hi, fractional) of frame `m`: the loudest bin of the
/// row's span, or, where a row is thinner than a bin, the level between the two nearest.
fn row_level(spec: &Spectrogram, m: usize, lo: f32, hi: f32) -> f32 {
    let last = spec.bins - 1;
    if hi - lo < 1.0 {
        let c = ((lo + hi) / 2.0).clamp(0.0, last as f32);
        let (k, f) = (c.floor() as usize, c.fract());
        let (a, b) = (spec.at(m, k), spec.at(m, (k + 1).min(last)));
        return a + (b - a) * f;
    }
    let (k0, k1) = (
        (lo.ceil() as usize).min(last),
        (hi.floor() as usize).min(last),
    );
    (k0..=k1.max(k0))
        .map(|k| spec.at(m, k))
        .fold(f32::MIN, f32::max)
}

/// The spectrogram as a picture: one column per frame, `rows` rows from the top frequency
/// down, coloured from `range_db` below the loudest level up to it.
fn picture(spec: &Spectrogram, look: Look, rows: usize) -> egui::ColorImage {
    let width = spec.frames.max(1);
    let (low, high) = axis(spec, look.log);
    let top_db = spec.max_db();
    let range = look.range_db.max(1.0);
    // Each row's span of bins, worked out once.
    let spans: Vec<(f32, f32)> = (0..rows)
        .map(|r| {
            let v_top = 1.0 - r as f32 / rows as f32;
            let v_bottom = 1.0 - (r + 1) as f32 / rows as f32;
            (
                spec.hz_bin(hz_at(low, high, look.log, v_bottom)),
                spec.hz_bin(hz_at(low, high, look.log, v_top)),
            )
        })
        .collect();
    let mut pixels = vec![Color32::BLACK; width * rows];
    for m in 0..spec.frames {
        for (r, &(lo, hi)) in spans.iter().enumerate() {
            let db = row_level(spec, m, lo, hi);
            pixels[r * width + m] = look.colors.at((db - (top_db - range)) / range);
        }
    }
    egui::ColorImage::new([width, rows], pixels)
}

/// Measures the spectrogram of `mono` (at `rate`), at most [`MAX_FRAMES`] frames across.
fn measure(mono: &[f32], rate: u32) -> Spectrogram {
    let size = if rate > 60_000 { 4096 } else { 2048 };
    let hop = (mono.len() / MAX_FRAMES).max(size / 4);
    prowave_engine::spectrogram(mono, rate, size, hop)
}

/// Opens the Spectrogram window on region `r` of track `t` (the part of its file the region
/// plays) and starts measuring it.
pub fn open(p: &Project, s: &mut crate::UiState, t: usize, r: usize) {
    let Some(region) = p.tracks.get(t).and_then(|x| x.regions.get(r)) else {
        return;
    };
    let Some(audio) = region.audio.as_ref() else {
        s.status = Some(tr("Show Spectrogram: this region has no audio").into());
        return;
    };
    let file = audio.file.clone();
    let map = p.tempo_map();
    let len_secs =
        map.seconds_at(p.region_end_beats(region)) - map.seconds_at(p.region_start_beats(region));
    let offset = region.params.offset_secs.max(0.0);
    let (tx, pending) = channel();
    let spawned = std::thread::Builder::new()
        .name("prowave-spectrogram".into())
        .spawn(move || {
            let done = file.read_stereo().map_err(|e| e.to_string()).map(|st| {
                let rate = f64::from(file.sample_rate.max(1));
                let frames = st.len() / 2;
                let from = ((offset * rate) as usize).min(frames);
                let to = (from + (len_secs.max(0.0) * rate) as usize).min(frames);
                let mono: Vec<f32> = st[from * 2..to * 2]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|f| (f[0] + f[1]) * 0.5)
                    .collect();
                measure(&mono, file.sample_rate)
            });
            let _ = tx.send(done);
        });
    let look = s
        .spectrogram
        .window
        .as_ref()
        .map_or(DEFAULT_LOOK, |w| w.look);
    match spawned {
        Ok(_) => {
            s.spectrogram.window = Some(Window {
                title: trf("Spectrogram: {}", &[&region.name]),
                pending: Some(pending),
                spec: None,
                error: None,
                look,
                texture: None,
            })
        }
        Err(e) => s.status = Some(trf("Spectrogram failed: {}", &[&e])),
    }
}

const DEFAULT_LOOK: Look = Look {
    log: true,
    colors: Colors::Magma,
    range_db: 80.0,
};

/// A round step for about `count` ticks over `span`, from `steps`.
fn tick_step(span: f64, count: f64, steps: &[f64]) -> f64 {
    steps
        .iter()
        .copied()
        .find(|s| span / s <= count)
        .unwrap_or_else(|| steps.last().copied().unwrap_or(1.0))
}

fn hz_label(hz: f32) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{}k", k.round())
        } else {
            format!("{k:.1}k")
        }
    } else {
        format!("{}", hz.round())
    }
}

/// Draws the spectrogram with its frequency axis on the left and time along the bottom.
fn plot(ui: &mut egui::Ui, spec: &Spectrogram, look: Look, tex: &egui::TextureHandle) {
    let size = ui.available_size().max(vec2(480.0, 260.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let g = ui.painter_at(rect);
    let plot = Rect::from_min_max(rect.min + vec2(44.0, 4.0), rect.max - vec2(6.0, 20.0));
    g.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    g.image(
        tex.id(),
        plot,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    let text = ui.visuals().text_color();
    let font = FontId::proportional(10.0);
    let tick = Stroke::new(1.0, text.gamma_multiply(0.6));
    let (low, high) = axis(spec, look.log);
    let marks: Vec<f32> = if look.log {
        [
            20.0, 50.0, 100.0, 200.0, 500.0, 1e3, 2e3, 5e3, 1e4, 2e4, 5e4,
        ]
        .into_iter()
        .filter(|f| *f >= low && *f <= high)
        .collect()
    } else {
        let step = tick_step(f64::from(high), 8.0, &[1e3, 2e3, 5e3, 1e4, 2e4]) as f32;
        (0..)
            .map(|i| i as f32 * step)
            .take_while(|f| *f <= high)
            .collect()
    };
    for hz in marks {
        let y = plot.bottom() - height_of(low, high, look.log, hz) * plot.height();
        g.line_segment([pos2(plot.left() - 4.0, y), pos2(plot.left(), y)], tick);
        g.text(
            pos2(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            hz_label(hz),
            font.clone(),
            text,
        );
    }
    let secs = spec.frame_secs(spec.frames.saturating_sub(1)).max(1e-3);
    let step = tick_step(
        secs,
        8.0,
        &[
            0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0,
        ],
    );
    for i in 0.. {
        let t = i as f64 * step;
        if t > secs + 1e-9 {
            break;
        }
        let x = plot.left() + (t / secs) as f32 * plot.width();
        g.line_segment([pos2(x, plot.bottom()), pos2(x, plot.bottom() + 4.0)], tick);
        g.text(
            pos2(x, plot.bottom() + 5.0),
            Align2::CENTER_TOP,
            format!("{} s", (t * 100.0).round() / 100.0),
            font.clone(),
            text,
        );
    }
    // What is under the pointer.
    if let Some(at) = resp.hover_pos().filter(|p| plot.contains(*p)) {
        let fx = (at.x - plot.left()) / plot.width();
        let fy = (plot.bottom() - at.y) / plot.height();
        let m = ((fx * spec.frames as f32) as usize).min(spec.frames.saturating_sub(1));
        let hz = hz_at(low, high, look.log, fy);
        let k = (spec.hz_bin(hz).round() as usize).min(spec.bins - 1);
        let label = format!(
            "{:.2} s   {} Hz   {:.0} dB",
            spec.frame_secs(m),
            hz.round(),
            spec.at(m, k)
        );
        g.text(
            plot.right_top() + vec2(-6.0, 6.0),
            Align2::RIGHT_TOP,
            label,
            FontId::proportional(11.0),
            Color32::WHITE,
        );
    }
}

/// Once per frame: the Spectrogram window, if one is open.
pub fn show(ctx: &egui::Context, s: &mut crate::UiState) {
    let Some(w) = s.spectrogram.window.as_mut() else {
        return;
    };
    if let Some(rx) = &w.pending {
        match rx.try_recv() {
            Ok(Ok(spec)) => {
                w.spec = Some(spec);
                w.pending = None;
            }
            Ok(Err(e)) => {
                w.error = Some(e);
                w.pending = None;
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(TryRecvError::Disconnected) => {
                w.error = Some("the measurement stopped".into());
                w.pending = None;
            }
        }
    }
    let mut open = true;
    egui::Window::new(w.title.clone())
        .id(egui::Id::unique("spectrogram-window"))
        .open(&mut open)
        .default_size([760.0, 420.0])
        .min_size([520.0, 320.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("Frequency axis"));
                ui.radio_value(&mut w.look.log, true, tr("Logarithmic"));
                ui.radio_value(&mut w.look.log, false, tr("Linear"));
                ui.separator();
                ui.label(tr("Colours"));
                egui::ComboBox::from_id_salt("spectrogram-colors")
                    .selected_text(w.look.colors.name())
                    .show_ui(ui, |ui| {
                        for c in Colors::ALL {
                            ui.selectable_value(&mut w.look.colors, c, c.name());
                        }
                    });
                ui.separator();
                ui.label(tr("Range"));
                ui.add(egui::Slider::new(&mut w.look.range_db, 30.0..=140.0).suffix(" dB"));
            });
            ui.separator();
            if let Some(e) = &w.error {
                ui.label(trf("Spectrogram failed: {}", &[e]));
                return;
            }
            let Some(spec) = &w.spec else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Analysing…"));
                });
                return;
            };
            let stale = w.texture.as_ref().is_none_or(|(l, _)| *l != w.look);
            if stale {
                let img = picture(spec, w.look, ROWS);
                let tex = ctx.load_texture("spectrogram", img, egui::TextureOptions::LINEAR);
                w.texture = Some((w.look, tex));
            }
            if let Some((_, tex)) = &w.texture {
                plot(ui, spec, w.look, tex);
            }
        });
    if !open {
        s.spectrogram.window = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    /// The brightest row of column `x` of a grey picture.
    fn brightest_row(img: &egui::ColorImage, x: usize) -> usize {
        let w = img.size[0];
        (0..img.size[1])
            .max_by_key(|r| img.pixels[r * w + x].r())
            .unwrap()
    }

    #[test]
    fn a_sine_is_one_bright_row_at_its_height_on_both_axes() {
        let rate = 48_000;
        let spec = measure(&sine(1000.0, rate, 1.0), rate);
        assert!(spec.frames <= MAX_FRAMES + 1);
        for log in [true, false] {
            let look = Look {
                log,
                colors: Colors::Grey,
                range_db: 80.0,
            };
            let img = picture(&spec, look, ROWS);
            assert_eq!(img.size, [spec.frames, ROWS]);
            let (low, high) = axis(&spec, log);
            let want = ((1.0 - height_of(low, high, log, 1000.0)) * ROWS as f32) as usize;
            let got = brightest_row(&img, spec.frames / 2);
            assert!(
                got.abs_diff(want) <= 2,
                "log {log}: row {got}, expected {want}"
            );
            // Dark far from it.
            let w = img.size[0];
            let far = if want > ROWS / 2 { 5 } else { ROWS - 5 };
            assert!(img.pixels[far * w + spec.frames / 2].r() < 60);
        }
    }

    #[test]
    fn long_audio_is_measured_in_at_most_the_frames_drawn() {
        let rate = 8_000;
        let spec = measure(&vec![0.0; rate as usize * 600], rate);
        assert!(spec.frames <= MAX_FRAMES + 1, "{}", spec.frames);
    }

    #[test]
    fn colour_maps_run_from_dark_to_light() {
        for c in Colors::ALL {
            let (a, b) = (c.at(0.0), c.at(1.0));
            let sum = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
            assert!(sum(a) < sum(b), "{c:?}");
            assert_eq!(c.at(-1.0), a);
            assert_eq!(c.at(2.0), b);
        }
        assert_eq!(Colors::Grey.at(0.5), Color32::from_rgb(128, 128, 128));
    }

    #[test]
    fn opening_a_region_without_audio_says_so() {
        let p = Project::demo();
        let mut s = crate::UiState::new();
        let (t, r) = p
            .tracks
            .iter()
            .enumerate()
            .find_map(|(t, tr)| (!tr.regions.is_empty()).then_some((t, 0)))
            .unwrap();
        assert!(p.tracks[t].regions[r].audio.is_none());
        open(&p, &mut s, t, r);
        assert!(s.spectrogram.window.is_none());
        assert!(s.status.is_some());
    }

    #[test]
    fn tick_labels_are_short() {
        assert_eq!(hz_label(1000.0), "1k");
        assert_eq!(hz_label(2500.0), "2.5k");
        assert_eq!(hz_label(200.0), "200");
        assert_eq!(tick_step(8.0, 8.0, &[0.5, 1.0, 2.0]), 1.0);
    }
}
