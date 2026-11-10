//! Show Spectrogram (an audio region's right-click menu): a window with the region's
//! spectrogram, time left to right, frequency bottom to top and loudness as colour, so a
//! picture painted into audio (Paint Image to Audio) can be seen, and any recording looked
//! into. The spectrum is measured on a worker thread (`prowave_engine::spectrogram`, 2048-point
//! Hann frames); the frequency axis (logarithmic or linear), the colours and the range shown
//! are changed at once, from the measurement already made.
use crate::hw::{self, Cap, Legend};
use crate::i18n::{tr, trf};
use crate::model::Project;
use crate::theme::{self, Face};
use egui::{pos2, vec2, Align2, Color32, Pos2, Rect, Sense, Stroke};
use prowave_engine::Spectrogram;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

/// The Hardware panel parts the vocal windows share (declared here: this is the one of them in
/// both builds).
#[path = "hw_vocal.rs"]
pub(crate) mod hw_vocal;

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
    /// The region's length and its file's rate, known before it is measured: the scales are
    /// printed from them meanwhile.
    secs: f64,
    rate: u32,
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
    let rate = file.sample_rate;
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
                secs: len_secs.max(0.0),
                rate,
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

/// The scope the spectrogram is shown on: a black bezel and a dark screen; returns the screen.
fn scope(g: &egui::Painter, rect: Rect) -> Rect {
    g.rect_filled(rect.translate(vec2(0.0, 1.0)), 7.0, theme::BONE_HI);
    hw::gradient(
        g,
        rect,
        7.0,
        Color32::from_rgb(0x2a, 0x28, 0x26),
        Color32::from_rgb(0x12, 0x11, 0x10),
    );
    let screen = rect.shrink(7.0);
    g.rect_filled(screen, 2.0, theme::METER_WELL);
    screen
}

/// The scope's place in the plot area: the frequency scale left of it, the time scale under it.
fn scope_rect(rect: Rect) -> Rect {
    Rect::from_min_max(rect.min + vec2(46.0, 0.0), rect.max - vec2(0.0, 22.0))
}

/// A message on a small piece of LCD glass in the middle of the screen (with a chaser running
/// under it while `busy`).
fn screen_message(g: &egui::Painter, screen: Rect, text: &str, busy: bool, time: f64) {
    let glass = Rect::from_center_size(
        screen.center(),
        vec2(
            260.0_f32.min(screen.width() - 20.0),
            if busy { 58.0 } else { 48.0 },
        ),
    );
    hw::lcd_glass(g, glass);
    let inner = glass.shrink2(vec2(12.0, 9.0));
    if busy {
        let (text_rect, run) = inner.split_top_bottom_at_y(inner.bottom() - 10.0);
        hw_vocal::lcd_text(g, text_rect, text, 11.0, 1, theme::LCD_INK);
        hw_vocal::lcd_chaser(g, run.shrink2(vec2(0.0, 1.0)), time);
    } else {
        hw_vocal::lcd_text(g, inner, text, 11.0, 2, theme::LCD_INK);
    }
}

/// A number in the scales' mono face, set tight (no letter spacing).
fn scale_text(g: &egui::Painter, pos: Pos2, anchor: Align2, text: &str) -> Rect {
    g.text(pos, anchor, text, theme::font(Face::Bold, 9.5), theme::INK2)
}

/// The printed scales and the faint graticule of a screen showing `low`..`high` Hz over
/// `secs` seconds: the frequencies left of the bezel (unit "HZ" under them), the seconds under
/// it (unit "S" at its end), with a closing tick at the end of the clip.
#[allow(clippy::too_many_arguments)]
fn scales(g: &egui::Painter, bezel: Rect, plot: Rect, low: f32, high: f32, log: bool, secs: f64) {
    let tick = Stroke::new(1.0, theme::INK2);
    let graticule = Stroke::new(1.0, Color32::from_white_alpha(16));
    let marks: Vec<f32> = if log {
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
        let y = plot.bottom() - height_of(low, high, log, hz) * plot.height();
        if y > plot.top() + 1.0 && y < plot.bottom() - 1.0 {
            g.hline(plot.x_range(), y, graticule);
        }
        g.hline(bezel.left() - 6.0..=bezel.left() - 1.0, y, tick);
        scale_text(
            g,
            pos2(bezel.left() - 9.0, y),
            Align2::RIGHT_CENTER,
            &hz_label(hz).to_uppercase(),
        );
    }
    hw::silk(
        g,
        pos2(bezel.left() - 9.0, bezel.bottom() + 8.0),
        Align2::RIGHT_TOP,
        "Hz",
        10.0,
        theme::PRINT,
    );
    let secs = secs.max(1e-3);
    let step = tick_step(
        secs,
        8.0,
        &[
            0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0,
        ],
    );
    let decimals = if (step * 10.0).fract().abs() > 1e-6 {
        2
    } else if step < 1.0 {
        1
    } else {
        0
    };
    let unit = hw::silk(
        g,
        pos2(bezel.right(), bezel.bottom() + 8.0),
        Align2::RIGHT_TOP,
        "s",
        10.0,
        theme::PRINT,
    );
    let x_at = |t: f64| plot.left() + (t / secs) as f32 * plot.width();
    for i in 0.. {
        let t = i as f64 * step;
        if t > secs + 1e-9 {
            break;
        }
        let x = x_at(t);
        if x > plot.left() + 1.0 && x < plot.right() - 1.0 {
            g.vline(x, plot.y_range(), graticule);
        }
        g.vline(x, bezel.bottom() + 1.0..=bezel.bottom() + 6.0, tick);
        let label = format!("{t:.decimals$}");
        let w = g
            .layout_no_wrap(label.clone(), theme::font(Face::Bold, 9.5), theme::INK2)
            .size()
            .x;
        // A label that would run into the unit is left off (its tick stays).
        if x + w / 2.0 < unit.left() - 6.0 {
            scale_text(g, pos2(x, bezel.bottom() + 8.0), Align2::CENTER_TOP, &label);
        }
    }
    // The end of the clip.
    g.vline(
        plot.right(),
        bezel.bottom() + 1.0..=bezel.bottom() + 4.0,
        tick,
    );
}

/// Draws the spectrogram on the scope in `rect`, its frequency scale printed on the panel at
/// the left and its time scale under it.
fn plot(ui: &mut egui::Ui, rect: Rect, spec: &Spectrogram, look: Look, tex: &egui::TextureHandle) {
    let resp = ui.interact(rect, ui.scope_id().with("spectrogram-plot"), Sense::hover());
    let g = ui.painter_at(rect.expand(2.0));
    let bezel = scope_rect(rect);
    let screen = scope(&g, bezel);
    let plot = screen.shrink(1.0);
    g.image(
        tex.id(),
        plot,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    hw::inner_shadow(&g, plot, 2.0, 0.5);
    let (low, high) = axis(spec, look.log);
    let secs = spec.frame_secs(spec.frames.saturating_sub(1));
    scales(&g, bezel, plot, low, high, look.log, secs);
    // What is under the pointer, on a strip of LCD glass in the screen's corner.
    if let Some(at) = resp.hover_pos().filter(|p| plot.contains(*p)) {
        let fx = (at.x - plot.left()) / plot.width();
        let fy = (plot.bottom() - at.y) / plot.height();
        let m = ((fx * spec.frames as f32) as usize).min(spec.frames.saturating_sub(1));
        let hz = hz_at(low, high, look.log, fy);
        let k = (spec.hz_bin(hz).round() as usize).min(spec.bins - 1);
        let cross = Stroke::new(1.0, Color32::from_white_alpha(70));
        g.vline(at.x, plot.y_range(), cross);
        g.hline(plot.x_range(), at.y, cross);
        let label = format!(
            "{:.2} S   {} HZ   {:.0} DB",
            spec.frame_secs(m),
            hz.round(),
            spec.at(m, k)
        );
        let w = hw::galley(&g, &label, Face::Lcd, 10.0, theme::LCD_INK)
            .size()
            .x
            + 16.0;
        let glass = Rect::from_min_size(plot.right_top() + vec2(-8.0 - w, 8.0), vec2(w, 22.0));
        hw::lcd_glass(&g, glass);
        hw::text(
            &g,
            glass.left_center() + vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            &label,
            Face::Lcd,
            10.0,
            theme::LCD_INK,
        );
    }
}

/// The scope with its scales but no picture yet (measuring, or failed), and `text` on the
/// screen: the frame stays as it will be once the picture is there.
fn empty_plot(ui: &egui::Ui, rect: Rect, look: Look, rate: u32, secs: f64, text: &str, busy: bool) {
    let g = ui.painter_at(rect.expand(2.0));
    let bezel = scope_rect(rect);
    let screen = scope(&g, bezel);
    let plot = screen.shrink(1.0);
    let top = rate.max(2) as f32 / 2.0;
    let (low, high) = if look.log {
        (LOG_LOW_HZ.min(top / 2.0), top)
    } else {
        (0.0, top)
    };
    scales(&g, bezel, plot, low, high, look.log, secs);
    screen_message(&g, screen, text, busy, ui.input(|i| i.time));
    if busy {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(80));
    }
}

/// The toolbar: the frequency axis keys, the colour list and the range knob, wrapped onto a
/// second line when the window is narrow. Returns its height.
fn toolbar(ui: &mut egui::Ui, look: &mut Look) -> f32 {
    let top = ui.cursor().min;
    let width = ui.available_width();
    let g = ui.painter().clone();
    let id = ui.scope_id().with("spectrogram-toolbar");
    let silk = |at: Pos2, label: &str| {
        hw::text(
            &g,
            at,
            Align2::LEFT_CENTER,
            &label.to_uppercase(),
            Face::Silk,
            11.0,
            theme::INK2,
        )
        .width()
    };
    let key_w = |label: &str| {
        hw::galley(&g, &label.to_uppercase(), Face::Bold, 11.0, theme::INK)
            .size()
            .x
            + 30.0
    };
    let (log_w, lin_w) = (key_w(tr("Logarithmic")), key_w(tr("Linear")));
    let groups = [
        hw::silk_width(&g, tr("Frequency axis"), 11.0) + 10.0 + log_w + 4.0 + lin_w,
        hw::silk_width(&g, tr("Colours"), 11.0) + 10.0 + 120.0,
        hw::silk_width(&g, tr("Range"), 11.0) + 10.0 + 30.0 + 8.0 + 64.0,
    ];
    // The groups go on as few rows as fit, each kept whole; a row's groups are spread from
    // its left edge to its right one.
    let row_h = 38.0;
    let gap = 26.0;
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    let mut used = 0.0;
    for (i, w) in groups.iter().enumerate() {
        let full = rows
            .last()
            .is_some_and(|row| !row.is_empty() && used + gap + w > width);
        if full {
            rows.push(Vec::new());
            used = 0.0;
        }
        if let Some(row) = rows.last_mut() {
            if !row.is_empty() {
                used += gap;
            }
            used += w;
            row.push(i);
        }
    }
    let mut starts = [top; 3];
    for (r, row) in rows.iter().enumerate() {
        let y = top.y + row_h / 2.0 + r as f32 * row_h;
        let total: f32 = row.iter().map(|i| groups[*i]).sum();
        let between = if row.len() > 1 {
            ((width - total) / (row.len() - 1) as f32).max(gap)
        } else {
            0.0
        };
        // A group alone on a later row sits at the right, under the end of the row above.
        let mut x = if r > 0 && row.len() == 1 {
            top.x + width - total
        } else {
            top.x
        };
        for i in row {
            starts[*i] = pos2(x, y);
            x += groups[*i] + between;
        }
    }
    let place = |i: usize| starts[i];
    // Frequency axis.
    let p = place(0);
    let x = p.x + silk(p, tr("Frequency axis")) + 10.0;
    for (on, label, w, dx) in [
        (true, tr("Logarithmic"), log_w, 0.0),
        (false, tr("Linear"), lin_w, log_w + 4.0),
    ] {
        let r = Rect::from_min_size(pos2(x + dx, p.y - 13.0), vec2(w, 26.0));
        let lit = look.log == on;
        let (resp, _) = hw::key(
            ui,
            &g,
            r,
            id.with(label),
            Cap::Bone,
            lit,
            Some(lit),
            Legend::Text(label),
        );
        if resp.clicked() {
            look.log = on;
        }
    }
    // Colours.
    let p = place(1);
    let x = p.x + silk(p, tr("Colours")) + 10.0;
    let names: Vec<&str> = Colors::ALL.iter().map(|c| c.name()).collect();
    let field = Rect::from_min_size(pos2(x, p.y - 12.0), vec2(120.0, 24.0));
    let now = Colors::ALL.iter().position(|c| *c == look.colors);
    if let Some(i) = hw_vocal::dropdown(
        ui,
        id.with("colors"),
        field,
        look.colors.name(),
        &names,
        now,
    ) {
        look.colors = Colors::ALL[i];
    }
    // Range.
    let p = place(2);
    let x = p.x + silk(p, tr("Range")) + 10.0;
    let label = format!("{} (dB)", tr("Range"));
    hw_vocal::inline_dial(
        ui,
        pos2(x + 13.0, p.y + 1.0),
        11.0,
        Rect::from_min_size(pos2(x + 38.0, p.y - 12.0), vec2(64.0, 24.0)),
        id.with("range"),
        &mut hw_vocal::Dial {
            label: &label,
            value: &mut look.range_db,
            range: 30.0..=140.0,
            default: DEFAULT_LOOK.range_db,
            decimals: 0,
            unit: "",
            enabled: true,
        },
    );
    rows.len() as f32 * row_h
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
    let title = w.title.clone();
    let (open, _) = hw_vocal::window(
        ctx,
        egui::Id::unique("spectrogram-window"),
        &title,
        "",
        760.0,
        Some((vec2(760.0, 420.0), vec2(520.0, 320.0))),
        |ui| {
            let h = toolbar(ui, &mut w.look);
            hw_vocal::gap(ui, h + 8.0);
            let size = ui.available_size().max(vec2(480.0, 240.0));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            if let Some(e) = &w.error {
                let text = trf("Spectrogram failed: {}", &[e]);
                empty_plot(ui, rect, w.look, w.rate, w.secs, &text, false);
                return;
            }
            let Some(spec) = &w.spec else {
                empty_plot(ui, rect, w.look, w.rate, w.secs, tr("Analysing…"), true);
                return;
            };
            let stale = w.texture.as_ref().is_none_or(|(l, _)| *l != w.look);
            if stale {
                let img = picture(spec, w.look, ROWS);
                let tex = ctx.load_texture("spectrogram", img, egui::TextureOptions::LINEAR);
                w.texture = Some((w.look, tex));
            }
            if let Some((_, tex)) = &w.texture {
                plot(ui, rect, spec, w.look, tex);
            }
        },
    );
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

    /// A sung line: a gliding fundamental with vibrato and twelve harmonics, in syllables.
    fn voice(rate: u32, secs: f32) -> Vec<f32> {
        let mut phase = 0.0f32;
        (0..(rate as f32 * secs) as usize)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let f0 = 196.0 * (1.0 + 0.35 * (t * 0.9).sin()) + 4.0 * (t * 34.0).sin();
                phase += std::f32::consts::TAU * f0 / rate as f32;
                let gate = if (t * 2.0).fract() < 0.82 { 1.0 } else { 0.0 };
                let tone: f32 = (1..=12).map(|k| (phase * k as f32).sin() / k as f32).sum();
                tone * 0.2 * gate
            })
            .collect()
    }

    fn window_with(spec: Option<Spectrogram>) -> crate::UiState {
        let mut s = crate::UiState::new();
        s.spectrogram.window = Some(Window {
            title: trf("Spectrogram: {}", &[&"Lead Vocal"]),
            pending: None,
            spec,
            error: None,
            look: DEFAULT_LOOK,
            texture: None,
            secs: 4.0,
            rate: 44_100,
        });
        s
    }

    /// Pictures of the window, measured and while measuring:
    /// `MANUAL_PNG_DIR=… cargo test -p prowave-app spectrogram_pictures -- --ignored`.
    #[test]
    #[ignore = "pictures of the window"]
    fn spectrogram_pictures() {
        let rate = 44_100;
        let spec = measure(&voice(rate, 4.0), rate);
        for (name, spec, w, h) in [
            ("spectrogram.png", Some(spec.clone()), 800.0, 470.0),
            ("spectrogram-narrow.png", Some(spec), 560.0, 380.0),
            ("spectrogram-analysing.png", None, 800.0, 470.0),
        ] {
            let mut s = window_with(spec);
            crate::ui_test::render_png(
                w,
                h,
                2.0,
                theme::BONE3,
                |ui| {
                    theme::apply(ui.ctx());
                    show(ui.ctx(), &mut s)
                },
                &hw_vocal::picture_path(name),
            )
            .unwrap();
        }
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
