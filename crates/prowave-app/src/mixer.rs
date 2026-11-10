//! Mixer (Logic Pro: Mixer in the main window): local toolbar, row labels and channel strips
//! with Setting, Gain Reduction, EQ, Input, Audio FX, Sends, Output, Group, Automation, Pan,
//! dB readouts, fader with scale, stereo meter, M/S and the coloured name label.
//!
//! Works today: fader, pan, mute, solo (all reach the engine), real post-fader meters with
//! peak hold, strip selection, Single/Tracks, the strip-type filters, scrolling; the Audio
//! FX slots (open, bypass, reorder or remove the effects an older project has; no built-in
//! effects are offered, see `empty_fx_slot`), the Gain Reduction meter and
//! the EQ curve, Sends to buses (with Sends on Faders) and the Aux strips that play them, and
//! the Automation modes (Read, Touch, Latch, Write) with the Automation editor. Groups are
//! drawn but not functional yet.
//!
//! [`channel_strip`] is also used by the Inspector's two strips.
use crate::hw::{self, Cap, SlotKind};
use crate::i18n::{tr, trf};
use crate::model::{AutoTarget, Insert, Project, SendSlot, TrackKind, MAX_SENDS};
use crate::theme::{self, Face};
use crate::widgets::{menu_title, popup_button};
use crate::UiState;
use egui::{
    epaint::{Mesh, TextShape},
    pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use std::collections::HashMap;

/// Hardware parts the Mixer group's screens share (dialog window, fitted names, icons).
#[path = "hw_mixer.rs"]
pub(crate) mod hw_mixer;

/// The Mixer's menu row.
const TOOLBAR_H: f32 = 42.0;
/// Key height in the menu row.
const BAR_KEY_H: f32 = 28.0;

/// Fader/meter scale marks in dB, top to bottom (Logic's channel strip scale).
const SCALE: [f32; 11] = [
    6.0, 3.0, 0.0, -3.0, -6.0, -10.0, -15.0, -20.0, -30.0, -40.0, -60.0,
];
pub(crate) const FILTERS: [&str; 6] = ["Audio", "Inst", "Aux", "Output", "VCA", "MIDI"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MixerMode {
    Single,
    Tracks,
}

pub struct MixerView {
    pub mode: MixerMode,
    /// Visibility per strip type, in `FILTERS` order.
    pub filters: [bool; 6],
    pub wide: bool,
    pub scroll_x: f32,
    /// How far the strips are scrolled down from their bottom, when the panel is too short
    /// for them (0 shows the faders, keys and names).
    pub scroll_y: f32,
    /// Peak hold per strip (linear), shared with the Inspector strips.
    pub peak_hold: HashMap<StripKey, f32>,
    /// The effect whose window is open: (track, slot).
    pub fx_window: Option<(usize, usize)>,
    /// Sends on Faders: the bus whose send levels the faders set.
    pub sends_on_faders: Option<u8>,
    /// Edit > Copy Channel Strip.
    pub copied: Option<CopiedStrip>,
    /// The audio input device that the Input slots choose from.
    pub inputs: crate::audio_rec::InputChoice,
    /// The Audio Units the Instrument and Audio FX slots offer.
    pub au_list: Vec<crate::plugins::AuChoice>,
    /// A plug-in window to open: (slot uid, title).
    pub open_plugin: Option<(u64, String)>,
    /// The delay of each running Stereo Out plug-in in seconds, in chain order (shown on its
    /// slot).
    pub master_latency: Vec<f64>,
}

/// A channel strip's effects, sends, level and pan, as Copy Channel Strip keeps them.
#[derive(Clone)]
pub struct CopiedStrip {
    inserts: Vec<Insert>,
    au_effects: Vec<crate::plugins::PluginSlot>,
    sends: Vec<SendSlot>,
    volume_db: f32,
    pan: f32,
}

impl CopiedStrip {
    fn of(t: &crate::model::Track) -> Self {
        Self {
            inserts: t.inserts.clone(),
            au_effects: t.au_effects.clone(),
            sends: t.sends.clone(),
            volume_db: t.volume_db,
            pan: t.pan,
        }
    }

    /// Onto `t`: the pasted plug-ins are new instances with the copied settings.
    fn paste(self, t: &mut crate::model::Track) {
        t.inserts = self.inserts;
        t.au_effects = self
            .au_effects
            .into_iter()
            .map(|mut a| {
                a.uid = crate::plugins::new_uid();
                a
            })
            .collect();
        t.sends = self.sends;
        t.volume_db = self.volume_db;
        t.pan = self.pan;
    }
}

impl Default for MixerView {
    fn default() -> Self {
        Self {
            mode: MixerMode::Tracks,
            filters: [true; 6],
            wide: false,
            scroll_x: 0.0,
            scroll_y: 0.0,
            peak_hold: HashMap::new(),
            fx_window: None,
            sends_on_faders: None,
            copied: None,
            inputs: crate::audio_rec::InputChoice::default(),
            au_list: Vec::new(),
            open_plugin: None,
            master_latency: Vec::new(),
        }
    }
}

/// Live meter values handed in by the app each frame.
#[derive(Default)]
pub struct Meters {
    pub tracks: Vec<(f32, f32)>,
    /// Gain reduction in dB of each track's compressors.
    pub reduction: Vec<f32>,
    /// Level going into each track's Space Designer (linear).
    pub fx_input: Vec<f32>,
    /// Gain reduction in dB and Space Designer input level of the Stereo Out strip's effects.
    pub master_reduction: f32,
    pub master_fx_input: f32,
    pub master: (f32, f32),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StripKey {
    Track(usize),
    StereoOut,
}

fn id(ns: &'static str, key: impl std::hash::Hash + std::fmt::Debug) -> egui::Id {
    egui::Id::unique((ns, key))
}

/// 0 (bottom, -60 dB and below) .. 1 (top, +6 dB).
pub(crate) fn db_to_frac(db: f32) -> f32 {
    if db >= SCALE[0] {
        return 1.0;
    }
    let n = SCALE.len() - 1;
    for i in 0..n {
        let (hi, lo) = (SCALE[i], SCALE[i + 1]);
        if db <= hi && db >= lo {
            let idx = i as f32 + (hi - db) / (hi - lo);
            return 1.0 - idx / n as f32;
        }
    }
    0.0
}

pub(crate) fn frac_to_db(f: f32) -> f32 {
    let n = SCALE.len() - 1;
    let idx = ((1.0 - f.clamp(0.0, 1.0)) * n as f32).min(n as f32 - 1e-4);
    let i = idx.floor() as usize;
    SCALE[i] + (SCALE[i + 1] - SCALE[i]) * (idx - i as f32)
}

pub(crate) fn lin_to_db(x: f32) -> f32 {
    if x <= 1e-6 {
        -120.0
    } else {
        20.0 * x.log10()
    }
}

// ---------------------------------------------------------------------------------------------
// Strip metrics. A strip is drawn at one of three densities: the full mixer view (tall panel),
// the main window's mixer, and a compact one for short panels and the Inspector. Every value
// comes from the Hardware spec (DIRECTIONS.md, A.3 and A.4); the compact set keeps every hit
// target at 20 points or more.

/// The sizes of one strip density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Density {
    /// Slot height and the pitch of the slot rows.
    sh: f32,
    pitch: f32,
    /// Space above the Setting row.
    top: f32,
    /// Extra space after the Audio FX rows.
    fx_gap: f32,
    /// Pan zone height, pan knob and send knob diameters.
    pan_h: f32,
    knob_d: f32,
    send_d: f32,
    /// dB readouts: height, gap under them, digit height, label size.
    db_h: f32,
    db_gap: f32,
    digit: f32,
    chip_label: f32,
    /// Mute/Solo keys and the name plate.
    ms_h: f32,
    ms_text: f32,
    name_h: f32,
    name_text: f32,
    /// Gap between the keys, the plate and the strip's bottom.
    end_gap: f32,
    /// Gap between the fader zone and the keys.
    fader_gap: f32,
    /// Fader cap, meter width, meter segment and gap.
    cap: Vec2,
    meter_w: f32,
    seg: f32,
    seg_gap: f32,
    /// Strip padding, scale tick length, scale numeral size, slot text size.
    pad: f32,
    tick_w: f32,
    num_size: f32,
    slot_text: f32,
    /// Row label column, strip widths (narrow, wide), gap between strips.
    label_w: f32,
    strip_w: [f32; 2],
    gap: f32,
}

/// The mixer view (a tall panel).
pub const BIG: Density = Density {
    sh: 32.0,
    pitch: 38.0,
    top: 16.0,
    fx_gap: 8.0,
    pan_h: 84.0,
    knob_d: 52.0,
    send_d: 28.0,
    db_h: 42.0,
    db_gap: 12.0,
    digit: 22.0,
    chip_label: 9.5,
    ms_h: 36.0,
    ms_text: 15.0,
    name_h: 34.0,
    name_text: 16.0,
    end_gap: 12.0,
    fader_gap: 14.0,
    cap: vec2(72.0, 44.0),
    meter_w: 64.0,
    seg: 6.0,
    seg_gap: 2.0,
    pad: 14.0,
    tick_w: 12.0,
    num_size: 11.0,
    slot_text: 12.5,
    label_w: 148.0,
    strip_w: [220.0, 286.0],
    gap: 10.0,
};

/// The main window's mixer.
pub const MAIN: Density = Density {
    sh: 26.0,
    pitch: 31.0,
    top: 12.0,
    fx_gap: 4.0,
    pan_h: 62.0,
    knob_d: 38.0,
    send_d: 22.0,
    db_h: 32.0,
    db_gap: 8.0,
    digit: 15.0,
    chip_label: 8.0,
    ms_h: 28.0,
    ms_text: 12.0,
    name_h: 26.0,
    name_text: 14.0,
    end_gap: 8.0,
    fader_gap: 10.0,
    cap: vec2(44.0, 30.0),
    meter_w: 30.0,
    seg: 4.0,
    seg_gap: 2.0,
    pad: 10.0,
    tick_w: 7.0,
    num_size: 10.0,
    slot_text: 11.0,
    label_w: 120.0,
    strip_w: [176.0, 252.0],
    gap: 8.0,
};

/// Short panels and the Inspector.
pub const COMPACT: Density = Density {
    sh: 20.0,
    pitch: 22.0,
    top: 6.0,
    fx_gap: 2.0,
    pan_h: 42.0,
    knob_d: 26.0,
    send_d: 18.0,
    db_h: 26.0,
    db_gap: 4.0,
    digit: 12.0,
    chip_label: 7.0,
    ms_h: 22.0,
    ms_text: 11.0,
    name_h: 22.0,
    name_text: 12.0,
    end_gap: 4.0,
    fader_gap: 4.0,
    cap: vec2(30.0, 22.0),
    meter_w: 22.0,
    seg: 4.0,
    seg_gap: 2.0,
    pad: 8.0,
    tick_w: 5.0,
    num_size: 8.5,
    slot_text: 9.5,
    label_w: 92.0,
    strip_w: [124.0, 176.0],
    gap: 6.0,
};

impl Density {
    /// The largest density whose fader still has a useful travel in a strip `h` tall and `w`
    /// wide with these rows.
    pub fn fit(h: f32, w: f32, fx_rows: usize, send_rows: usize) -> Density {
        for (d, min_fader, min_w) in [(BIG, 340.0, 200.0), (MAIN, 190.0, 140.0)] {
            let r = rows(0.0, fx_rows, send_rows, d);
            if bottom_parts(d) + r.fader_top + min_fader <= h && w >= min_w {
                return d;
            }
        }
        COMPACT
    }
}

/// The shortest fader travel a strip is drawn with; a shorter panel scrolls the strips.
const MIN_FADER: f32 = 150.0;

/// Height of the keys, the plate and their gaps under the fader zone.
fn bottom_parts(d: Density) -> f32 {
    d.fader_gap + d.ms_h + d.end_gap + d.name_h + d.end_gap
}

/// Vertical positions of the strip rows (shared by every strip so rows line up).
pub struct Rows {
    pub d: Density,
    setting: f32,
    input: f32,
    fx: f32,
    fx_rows: usize,
    sends: f32,
    send_rows: usize,
    output: f32,
    group: f32,
    pan: f32,
    db: f32,
    fader_top: f32,
}

impl Rows {
    /// The tops of the slot rows from Setting to Group, then the pan zone's: the places a
    /// short panel can hide the strips' top down to.
    fn row_tops(&self) -> Vec<f32> {
        let d = self.d;
        let mut tops = vec![self.setting, self.input];
        tops.extend((0..self.fx_rows).map(|k| self.fx + k as f32 * d.pitch));
        tops.extend((0..self.send_rows).map(|k| self.sends + k as f32 * d.pitch));
        tops.extend([self.output, self.group, self.pan]);
        tops
    }
}

pub fn rows(top: f32, fx_rows: usize, send_rows: usize, d: Density) -> Rows {
    let fx_rows = fx_rows.max(2);
    let send_rows = send_rows.clamp(1, MAX_SENDS);
    let setting = top + d.top;
    let input = setting + d.pitch;
    let fx = input + d.pitch;
    let sends = fx + fx_rows as f32 * d.pitch + d.fx_gap;
    let output = sends + send_rows as f32 * d.pitch;
    let group = output + d.pitch;
    let pan = group + d.pitch;
    let db = pan + d.pan_h;
    Rows {
        d,
        setting,
        input,
        fx,
        fx_rows,
        sends,
        send_rows,
        output,
        group,
        pan,
        db,
        fader_top: db + d.db_h + d.db_gap,
    }
}

/// Where a strip's parts go: the inner column, the fader, the keys and the plate.
struct StripGeo {
    /// Left edge and width of the inner column.
    x: f32,
    iw: f32,
    /// The fader's centre line, and its scale: the 0..1 travel runs from `top` (+6 dB) down
    /// `len` points (-60 dB).
    fcx: f32,
    top: f32,
    len: f32,
    ms: Rect,
    name: Rect,
}

fn strip_geo(strip: Rect, r: &Rows) -> StripGeo {
    let d = r.d;
    let x = strip.left() + d.pad;
    let iw = strip.width() - 2.0 * d.pad;
    let name_y = strip.bottom() - d.end_gap - d.name_h;
    let ms_y = name_y - d.end_gap - d.ms_h;
    let zone_bottom = ms_y - d.fader_gap;
    let top = r.fader_top + d.cap.y / 2.0;
    StripGeo {
        x,
        iw,
        fcx: x + d.cap.x / 2.0 + 2.0,
        top,
        len: (zone_bottom - d.cap.y / 2.0 - top).max(1.0),
        ms: Rect::from_min_size(pos2(x, ms_y), vec2(iw, d.ms_h)),
        name: Rect::from_min_size(
            pos2(x - d.pad * 0.3, name_y),
            vec2(iw + d.pad * 0.6, d.name_h),
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Strip parts, painted as the Hardware spec draws them (A.4).

/// `rect` rounded by `radius`, filled with a vertical gradient through `stops` (0..1, colour).
/// Unlike a centre fan, the rows follow the stops exactly, so a fader cap's bands stay flat.
fn vgrad(g: &Painter, rect: Rect, radius: f32, stops: &[(f32, Color32)]) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 || stops.is_empty() {
        return;
    }
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let color_at = |t: f32| {
        let t = t.clamp(0.0, 1.0);
        let mut prev = stops[0];
        for s in stops {
            if t <= s.0 {
                let span = (s.0 - prev.0).max(1e-4);
                return hw::lerp(prev.1, s.1, (t - prev.0) / span);
            }
            prev = *s;
        }
        prev.1
    };
    // Rows at every stop and through the rounded corners.
    let mut ys: Vec<f32> = stops
        .iter()
        .map(|s| rect.top() + s.0 * rect.height())
        .collect();
    for k in 0..=4 {
        let dy = r * k as f32 / 4.0;
        ys.push(rect.top() + dy);
        ys.push(rect.bottom() - dy);
    }
    ys.retain(|y| *y >= rect.top() && *y <= rect.bottom());
    ys.sort_by(f32::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    let inset = |y: f32| {
        let dy = if y < rect.top() + r {
            r - (y - rect.top())
        } else if y > rect.bottom() - r {
            r - (rect.bottom() - y)
        } else {
            0.0
        };
        r - (r * r - dy * dy).max(0.0).sqrt()
    };
    let mut mesh = Mesh::default();
    for (i, y) in ys.iter().enumerate() {
        let c = color_at((y - rect.top()) / rect.height());
        let ins = inset(*y);
        mesh.colored_vertex(pos2(rect.left() + ins, *y), c);
        mesh.colored_vertex(pos2(rect.right() - ins, *y), c);
        if i > 0 {
            let b = 2 * i as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    g.add(Shape::mesh(mesh));
}

/// A disc of radius `r` at `c` filled with a radial gradient centred at `focus` through
/// `stops` (0 at the focus, 1 at the rim).
fn radial(g: &Painter, c: Pos2, r: f32, focus: Pos2, stops: &[(f32, Color32)]) {
    let color_at = |t: f32| {
        let mut prev = stops[0];
        for s in stops {
            if t <= s.0 {
                let span = (s.0 - prev.0).max(1e-4);
                return hw::lerp(prev.1, s.1, (t - prev.0) / span);
            }
            prev = *s;
        }
        prev.1
    };
    let n = 40;
    let rings = 6;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(focus, color_at(0.0));
    for k in 1..=rings {
        let t = k as f32 / rings as f32;
        let col = color_at(t);
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let rim = c + vec2(a.cos(), a.sin()) * r;
            mesh.colored_vertex(focus + (rim - focus) * t, col);
        }
    }
    let n32 = n as u32;
    for i in 0..n32 {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n32);
    }
    for k in 1..rings as u32 {
        let (a0, b0) = (1 + (k - 1) * n32, 1 + k * n32);
        for i in 0..n32 {
            let j = (i + 1) % n32;
            mesh.add_triangle(a0 + i, a0 + j, b0 + i);
            mesh.add_triangle(a0 + j, b0 + j, b0 + i);
        }
    }
    g.add(Shape::mesh(mesh));
}

/// A point `dist` from `c` at `deg` degrees clockwise from straight up.
fn polar(c: Pos2, deg: f32, dist: f32) -> Pos2 {
    let a = deg.to_radians();
    c + vec2(a.sin(), -a.cos()) * dist
}

/// A soft round shadow: a few translucent discs.
fn disc_shadow(g: &Painter, c: Pos2, r: f32, dy: f32, blur: f32, alpha: f32) {
    for (grow, a) in [(blur, 0.25), (blur * 0.55, 0.35), (0.0, 0.5)] {
        g.circle_filled(
            c + vec2(0.0, dy),
            r + grow * 0.5,
            Color32::from_black_alpha((alpha * a * 255.0) as u8),
        );
    }
}

/// The knurled black knob (A.4.4): body, a ring of 36 knurl wedges, the cap and an orange
/// pointer. `value` runs -1..1 over -135°..+135°. `scale` adds the printed tick ring and L / R.
fn knurl_knob(g: &Painter, c: Pos2, d: f32, value: f32, scale: bool, hot: bool) {
    let r = d / 2.0;
    if scale {
        for i in 0..=10 {
            let deg = -135.0 + 27.0 * i as f32;
            let major = i % 5 == 0;
            g.line_segment(
                [
                    polar(c, deg, r + 4.0),
                    polar(c, deg, r + if major { 9.5 } else { 7.0 }),
                ],
                Stroke::new(if major { 1.8 } else { 1.1 }, theme::INK2),
            );
        }
        let off = r + if d >= 40.0 { 22.0 } else { 15.0 };
        for (dx, t) in [(-off, "L"), (off, "R")] {
            hw::silk(
                g,
                c + vec2(dx, r - 3.0),
                Align2::CENTER_CENTER,
                t,
                11.0,
                theme::INK2,
            );
        }
    }
    let (dy, blur) = if d > 40.0 { (4.0, 7.0) } else { (2.0, 4.0) };
    disc_shadow(g, c, r, dy, blur, 0.4);
    g.circle_filled(c + vec2(0.0, 1.0), r, Color32::from_black_alpha(150));
    radial(
        g,
        c,
        r,
        c + vec2(0.0, -0.2 * r),
        &[
            (0.0, Color32::from_rgb(0x3a, 0x38, 0x35)),
            (0.62, Color32::from_rgb(0x1c, 0x1b, 0x19)),
            (1.0, Color32::from_rgb(0x0e, 0x0e, 0x0d)),
        ],
    );
    // Knurl: alternating 5° wedges.
    let rk = r - (0.1 * d).max(2.5);
    let mut mesh = Mesh::default();
    for k in 0..72 {
        let col = if k % 2 == 0 {
            Color32::from_rgb(0x2e, 0x2c, 0x29)
        } else {
            Color32::from_rgb(0x12, 0x11, 0x10)
        };
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(c, col);
        mesh.colored_vertex(polar(c, k as f32 * 5.0, rk), col);
        mesh.colored_vertex(polar(c, (k + 1) as f32 * 5.0, rk), col);
        mesh.add_triangle(base, base + 1, base + 2);
    }
    g.add(Shape::mesh(mesh));
    g.circle_stroke(
        c,
        rk - 0.5,
        Stroke::new(1.0, Color32::from_black_alpha(110)),
    );
    // Cap.
    let rc = r - (0.22 * d).max(5.0);
    g.circle_filled(c + vec2(0.0, 1.0), rc + 0.5, Color32::from_black_alpha(140));
    radial(
        g,
        c,
        rc,
        c + vec2(-0.1 * rc, -0.3 * rc),
        &[
            (0.0, Color32::from_rgb(0x4d, 0x4a, 0x45)),
            (0.72, Color32::from_rgb(0x24, 0x22, 0x1f)),
            (1.0, Color32::from_rgb(0x20, 0x1e, 0x1b)),
        ],
    );
    g.circle_stroke(c, r - 0.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
    // Pointer.
    let deg = value.clamp(-1.0, 1.0) * 135.0;
    let ind = (0.07 * d).max(2.0);
    let a = polar(c, deg, r - ind);
    let b = polar(c, deg, r - ind - 0.38 * d);
    let glow = if hot { 0.9 } else { 0.6 };
    g.line_segment(
        [a, b],
        Stroke::new(5.0, theme::ORANGE.gamma_multiply(glow * 0.35)),
    );
    g.line_segment([a, b], Stroke::new(3.0, theme::ORANGE));
}

/// The fader groove (A.4.5): 6 wide, overshooting the travel by 6 at each end.
fn fader_slot(g: &Painter, cx: f32, top: f32, len: f32) {
    let r = Rect::from_min_max(pos2(cx - 3.0, top - 6.0), pos2(cx + 3.0, top + len + 6.0));
    g.rect_filled(r.translate(vec2(0.0, 1.0)), 3.0, theme::BONE_HI);
    g.rect_filled(r, 3.0, Color32::from_rgb(0x0f, 0x0f, 0x0e));
    g.hline(
        r.left() + 1.0..=r.right() - 1.0,
        r.top() + 1.5,
        Stroke::new(1.0, Color32::from_black_alpha(230)),
    );
}

/// The black fader cap (A.4.5): banded gradient, two grip lines and the bright index line.
fn fader_cap(g: &Painter, r: Rect) {
    for (dy, grow, a) in [(6.0, 4.0, 30u8), (5.0, 2.0, 40), (3.0, 0.5, 50)] {
        g.rect_filled(
            r.translate(vec2(0.0, dy)).expand(grow),
            3.0 + grow,
            Color32::from_black_alpha(a),
        );
    }
    g.rect_filled(
        r.translate(vec2(0.0, 1.0)),
        3.0,
        Color32::from_black_alpha(204),
    );
    vgrad(
        g,
        r,
        3.0,
        &[
            (0.0, Color32::from_rgb(0x4a, 0x47, 0x43)),
            (0.16, Color32::from_rgb(0x2a, 0x28, 0x25)),
            (0.5, Color32::from_rgb(0x1a, 0x19, 0x17)),
            (0.84, Color32::from_rgb(0x23, 0x21, 0x20)),
            (1.0, Color32::from_rgb(0x0c, 0x0b, 0x0a)),
        ],
    );
    g.hline(
        r.left() + 3.0..=r.right() - 3.0,
        r.top() + 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(61)),
    );
    for f in [0.22, 0.78] {
        g.hline(
            r.left() + 5.0..=r.right() - 5.0,
            r.top() + r.height() * f,
            Stroke::new(1.0, Color32::from_white_alpha(33)),
        );
    }
    let cy = r.center().y;
    g.hline(
        r.left() + 3.0..=r.right() - 3.0,
        cy,
        Stroke::new(5.0, Color32::from_white_alpha(30)),
    );
    g.hline(
        r.left() + 3.0..=r.right() - 3.0,
        cy,
        Stroke::new(3.0, theme::BONE_HI),
    );
}

/// The printed dB labels, top to bottom, evenly spaced (the fader and meter read off them).
const SCALE_LABELS: [&str; 11] = [
    "+6", "+3", "0", "3", "6", "10", "15", "20", "30", "40", "60",
];

/// A column of 21 scale ticks from `top` down `len`: majors `w` long, minors 55% of that.
/// Under 8 points a tick the minors go, so the column stays a scale and not a smear.
fn tick_col(g: &Painter, x: f32, top: f32, len: f32, w: f32, align_right: bool) {
    let minors = len / 20.0 >= 8.0;
    for i in 0..=20 {
        let y = (top + i as f32 * len / 20.0).round() + 0.5;
        let major = i % 2 == 0;
        if !major && !minors {
            continue;
        }
        let tw = if major { w } else { w * 0.55 };
        let x0 = if align_right { x + w - tw } else { x };
        g.hline(
            x0..=x0 + tw,
            y,
            Stroke::new(
                1.0,
                theme::INK2.gamma_multiply(if major { 0.7 } else { 0.38 }),
            ),
        );
    }
}

/// The LED-ladder meter (A.4.6): two columns of segments in a dark window, lit against the
/// shared dB scale (`top`/`len` are the fader's scale), the peak hold one lit segment.
#[allow(clippy::too_many_arguments)]
fn ladder(
    g: &Painter,
    rect: Rect,
    levels: (f32, f32),
    peak_db: f32,
    top: f32,
    len: f32,
    seg: f32,
    gap: f32,
    inactive: bool,
) {
    g.rect_filled(rect.translate(vec2(0.0, 1.0)), 3.0, theme::BONE_HI);
    g.rect_filled(rect, 3.0, theme::METER_WELL);
    hw::inner_shadow(g, rect, 3.0, 0.6);
    let y_of = |db: f32| top + (1.0 - db_to_frac(db)) * len;
    let (y0, y6) = (y_of(0.0), y_of(-6.0));
    let col_w = (rect.width() - 10.0) / 2.0;
    let n = ((rect.height() - 6.0) / (seg + gap)).floor().max(1.0) as usize;
    let bottom = rect.bottom().round();
    let off = Color32::from_rgb(0x2b, 0x29, 0x26);
    let dead = Color32::from_rgb(0x8b, 0x85, 0x78);
    for (c, lvl) in [levels.0, levels.1].into_iter().enumerate() {
        let x0 = rect.left() + 3.0 + c as f32 * (col_w + 4.0);
        let thr = if lvl > -60.0 { y_of(lvl) } else { f32::MAX };
        let pk_y = if peak_db > -60.0 {
            y_of(peak_db)
        } else {
            f32::MAX
        };
        let mut pk_done = false;
        for i in 0..n {
            let yy = bottom - 3.0 - (i + 1) as f32 * (seg + gap) + gap;
            let mid = yy + seg / 2.0;
            let hot = mid < y0;
            let warm = mid < y6;
            let lit_col = if inactive {
                dead
            } else if hot {
                theme::ORANGE
            } else if warm {
                Color32::from_rgb(0xff, 0xe6, 0xcf)
            } else {
                Color32::from_rgb(0xee, 0xea, 0xdf)
            };
            let mut lit = mid >= thr;
            if !lit && !pk_done && mid < pk_y + seg && mid >= pk_y - gap - seg {
                lit = true;
                pk_done = true;
            }
            let r = Rect::from_min_size(pos2(x0.round(), yy), vec2(col_w.round(), seg));
            if lit {
                if !inactive {
                    let glow = if hot {
                        Color32::from_rgba_unmultiplied(255, 90, 20, 70)
                    } else {
                        Color32::from_rgba_unmultiplied(255, 250, 235, 30)
                    };
                    g.rect_filled(r.expand(1.0), 1.0, glow);
                }
                g.rect_filled(r, 0.0, lit_col);
            } else {
                g.rect_filled(r, 0.0, off);
            }
        }
    }
}

/// A small LCD readout (A.4.3): glass, a label top left, and the value in segments at the
/// right. No value shows the unlit "88.8" ghost.
fn lcd_chip(g: &Painter, rect: Rect, label: &str, value: Option<&str>, d: Density) {
    hw::lcd_glass(g, rect);
    hw::text(
        g,
        rect.left_top() + vec2(5.0, 4.0),
        Align2::LEFT_TOP,
        label,
        Face::Lcd,
        d.chip_label,
        theme::LCD_INK.gamma_multiply(0.72),
    );
    let h = d.digit.min(rect.height() - d.chip_label - 6.0);
    let pad = if d.digit >= 20.0 { 8.0 } else { 6.0 };
    let (text, ink) = match value {
        Some(v) => (v, theme::LCD_INK),
        None => ("88.8", theme::LCD_GHOST),
    };
    let w = hw::seg_width(text, h);
    hw::seg(
        g,
        pos2(rect.right() - pad - w, rect.bottom() - 4.0 - h),
        text,
        h,
        ink,
        value.is_some(),
    );
}

/// What a slot shows before its label.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    None,
    /// A keyboard (an instrument).
    Keys,
    /// One ring (a mono input) or two (a stereo pair).
    Ring,
    Rings,
}

/// The keyboard pictogram, `s` wide, centred at `c`.
fn keys_icon(g: &Painter, c: Pos2, s: f32, col: Color32) {
    let k = s / 24.0;
    let at = |x: f32, y: f32| c + vec2((x - 12.0) * k, (y - 12.0) * k);
    let st = Stroke::new((2.0 * k).max(1.2), col);
    g.rect_stroke(
        Rect::from_min_max(at(3.0, 5.0), at(21.0, 19.0)),
        0.0,
        st,
        StrokeKind::Middle,
    );
    for x in [7.5, 12.0, 16.5] {
        g.line_segment([at(x, 5.0), at(x, 19.0)], st);
    }
    for x in [5.6, 14.8] {
        g.rect_filled(Rect::from_min_max(at(x, 5.0), at(x + 3.6, 13.0)), 0.0, col);
    }
}

/// A slot of the strip (A.4.10): the material, an LED (`led`: Some(lit)), an icon and the
/// label (centred on bone slots, after the LED on black ones). `dim` greys the print.
#[allow(clippy::too_many_arguments)]
fn paint_strip_slot(
    g: &Painter,
    r: Rect,
    kind: SlotKind,
    label: &str,
    led: Option<bool>,
    icon: Icon,
    dim: bool,
    size: f32,
    hovered: bool,
) {
    hw::paint_slot(g, r, kind, "", None, false, hovered);
    let ink = match kind {
        SlotKind::Filled if dim => theme::ON_DARK_DIM,
        SlotKind::Filled => theme::ON_DARK,
        SlotKind::Plain if dim => theme::PRINT,
        _ => theme::INK,
    };
    let mut x = r.left() + 8.0;
    if let Some(on) = led {
        let c = pos2(x + 2.5, r.center().y);
        if on {
            g.circle_filled(c, 5.0, Color32::from_rgba_unmultiplied(255, 90, 20, 50));
            g.circle_filled(c, 3.6, Color32::from_rgba_unmultiplied(255, 90, 20, 90));
            g.circle_filled(c, 2.5, Color32::from_rgb(0xff, 0x6a, 0x22));
        } else {
            g.circle_filled(c, 2.5, Color32::from_rgb(0x4a, 0x47, 0x41));
        }
        x += 13.0;
    }
    if label.is_empty() {
        return;
    }
    let isz = (r.height() * 0.5).clamp(9.0, 15.0);
    let ring = isz * 0.36;
    let icon_w = match icon {
        Icon::None => 0.0,
        Icon::Keys => isz + 7.0,
        Icon::Ring => ring * 2.0 + 7.0,
        Icon::Rings => ring * 2.0 + ring * 1.2 + 7.0,
    };
    let upper = label.to_uppercase();
    let room_with = r.right() - 8.0 - x - icon_w;
    let room_without = r.right() - 8.0 - x;
    let tw_at = |sz: f32| hw_mixer::width_t(g, &upper, Face::Bold, sz, 0.05);
    let (size, icon, icon_w, shown) = if tw_at(size) <= room_with {
        (size, icon, icon_w, upper.clone())
    } else if tw_at(size - 1.0) <= room_with {
        (size - 1.0, icon, icon_w, upper.clone())
    } else if tw_at(size - 1.0) <= room_without {
        (size - 1.0, Icon::None, 0.0, upper.clone())
    } else {
        let room = if icon == Icon::None {
            room_with
        } else {
            room_without
        };
        (
            size - 1.0,
            Icon::None,
            0.0,
            hw::fit(g, &upper, Face::Bold, size - 1.0, room),
        )
    };
    let tw =
        hw::galley(g, &shown, Face::Bold, size, ink).size().x - theme::tracking(Face::Bold) * size;
    let start = if kind == SlotKind::Plain {
        (r.center().x - (icon_w + tw) / 2.0).max(x)
    } else {
        x
    };
    let cy = r.center().y;
    match icon {
        Icon::None => {}
        Icon::Keys => keys_icon(g, pos2(start + isz / 2.0, cy), isz, ink),
        Icon::Ring | Icon::Rings => {
            let st = Stroke::new(2.0, ink);
            g.circle_stroke(pos2(start + ring, cy), ring, st);
            if icon == Icon::Rings {
                g.circle_stroke(pos2(start + ring * 2.2, cy), ring, st);
            }
        }
    }
    hw::text(
        g,
        pos2(start + icon_w, cy),
        Align2::LEFT_CENTER,
        &shown,
        Face::Bold,
        size,
        ink,
    );
}

/// [`paint_strip_slot`] that takes the click.
#[allow(clippy::too_many_arguments)]
fn strip_slot(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    id: egui::Id,
    kind: SlotKind,
    label: &str,
    led: Option<bool>,
    icon: Icon,
    dim: bool,
    size: f32,
) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click());
    paint_strip_slot(g, r, kind, label, led, icon, dim, size, resp.hovered());
    resp
}

/// The slot text size that goes with a slot `h` tall.
fn slot_text(h: f32) -> f32 {
    if h >= 30.0 {
        BIG.slot_text
    } else if h >= 24.0 {
        MAIN.slot_text
    } else {
        COMPACT.slot_text
    }
}

/// A strip's Mute or Solo key: a bone key with its LED at the left, latched while `on`. Solo's
/// legend turns orange when on.
#[allow(clippy::too_many_arguments)]
fn strip_key(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    id: egui::Id,
    label: &str,
    on: bool,
    orange_legend: bool,
    size: f32,
) -> bool {
    let resp = ui.interact(r, id, Sense::click());
    let down = on || resp.is_pointer_button_down_on();
    let face = hw::paint_key(g, r, Cap::Bone, down, resp.hovered());
    hw::led_sized(g, pos2(face.left() + 9.0 + 2.5, face.center().y), 3.0, on);
    let ink = if on && orange_legend {
        theme::ORANGE_DK
    } else {
        theme::INK
    };
    hw::text(
        g,
        face.center() + vec2(4.0, 0.0),
        Align2::CENTER_CENTER,
        label,
        Face::Bold,
        size,
        ink,
    );
    resp.clicked()
}

/// The material of a strip's name plate (A.5): told apart by material, never by hue.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Plate {
    /// Black (software instruments, external MIDI).
    Black,
    /// Bone with a diagonal hatch (Drummer).
    Hatch,
    /// Warm grey (audio, aux and buses).
    Grey,
    /// Bone (VCA).
    Bone,
    /// Black with an orange chip and underline (Stereo Out).
    Master,
}

fn plate_of(kind: Option<TrackKind>) -> Plate {
    match kind {
        None => Plate::Master,
        Some(TrackKind::Instrument | TrackKind::ExternalMidi) => Plate::Black,
        Some(TrackKind::Drummer) => Plate::Hatch,
        Some(TrackKind::Vca | TrackKind::Folder) => Plate::Bone,
        Some(_) => Plate::Grey,
    }
}

/// Diagonal hatch lines over `r` (`/`, every `pitch` points).
fn hatch(g: &Painter, r: Rect, pitch: f32, width: f32, col: Color32) {
    let clip = g.with_clip_rect(r.intersect(g.clip_rect()));
    let mut x = r.left() - r.height();
    while x < r.right() {
        clip.line_segment(
            [pos2(x, r.bottom()), pos2(x + r.height(), r.top())],
            Stroke::new(width, col),
        );
        x += pitch;
    }
}

/// The name plate at the foot of a strip: its material, a number chip and the name.
fn name_plate(
    g: &Painter,
    r: Rect,
    plate: Plate,
    selected: bool,
    num: &str,
    name: &str,
    size: f32,
) {
    let rad = 3.0;
    g.rect_filled(
        r.translate(vec2(0.0, 2.0)),
        rad,
        Color32::from_black_alpha(40),
    );
    let (fg, chip_bg, chip_fg) = plate_face(g, r, plate, selected);
    if plate == Plate::Master {
        let under = Rect::from_min_max(pos2(r.left(), r.bottom() - 3.0), r.max);
        g.rect_filled(
            under,
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: rad as u8,
                se: rad as u8,
            },
            theme::ORANGE,
        );
    }
    g.hline(
        r.left() + rad..=r.right() - rad,
        r.top() + 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(46)),
    );
    g.rect_stroke(
        r,
        rad,
        Stroke::new(1.0, Color32::from_black_alpha(140)),
        StrokeKind::Outside,
    );
    // Number chip.
    let chip_size = (size * 0.78).clamp(8.5, 12.0);
    let gal = hw::galley(g, num, Face::Lcd, chip_size, chip_fg);
    let chip = Rect::from_min_size(
        pos2(r.left() + 5.0, r.top() + 5.0),
        vec2(gal.size().x + 9.0, r.height() - 10.0),
    );
    g.rect_filled(chip, 2.0, chip_bg);
    g.galley(
        pos2(chip.left() + 4.5, chip.center().y - gal.size().y / 2.0),
        gal,
        chip_fg,
    );
    // The name: 0.08 em at the strip's size, then a point smaller at 0.04 em, then shortened
    // word by word, and only then cut (A.2: name plate).
    let x = chip.right() + 7.0;
    let fitted = hw_mixer::fit_steps(
        g,
        &name.to_uppercase(),
        Face::Silk,
        &[(size, 0.08), (size - 1.0, 0.04), (size - 2.0, 0.02)],
        r.right() - 6.0 - x,
    );
    hw_mixer::text_t(
        g,
        pos2(x, r.center().y),
        Align2::LEFT_CENTER,
        &fitted.text,
        Face::Silk,
        fitted.size,
        fitted.track,
        fg,
    );
}

/// The material of a plate of `kind` (None: the Stereo Out) in `r`, as the Track Color window
/// shows it.
pub(crate) fn paint_plate_material(g: &Painter, r: Rect, kind: Option<TrackKind>) {
    plate_face(g, r, plate_of(kind), false);
}

/// Paints a plate's face; returns the name's ink, the chip's fill and the chip's ink.
fn plate_face(g: &Painter, r: Rect, plate: Plate, selected: bool) -> (Color32, Color32, Color32) {
    let rad = 3.0;
    let black = [
        (0.0, Color32::from_rgb(0x2d, 0x2b, 0x28)),
        (1.0, Color32::from_rgb(0x16, 0x15, 0x14)),
    ];
    if selected {
        vgrad(
            g,
            r,
            rad,
            &[
                (0.0, Color32::from_rgb(0xff, 0x6e, 0x2e)),
                (1.0, Color32::from_rgb(0xf2, 0x52, 0x0f)),
            ],
        );
        (theme::INK, theme::INK, theme::ORANGE)
    } else {
        match plate {
            Plate::Black => {
                vgrad(g, r, rad, &black);
                (theme::ON_DARK, theme::ON_DARK, theme::INK)
            }
            Plate::Master => {
                vgrad(g, r, rad, &black);
                (theme::ON_DARK, theme::ORANGE, theme::INK)
            }
            Plate::Hatch => {
                vgrad(
                    g,
                    r,
                    rad,
                    &[
                        (0.0, Color32::from_rgb(0xe4, 0xdf, 0xd3)),
                        (1.0, Color32::from_rgb(0xd3, 0xcd, 0xbf)),
                    ],
                );
                hatch(g, r, 6.0, 2.0, theme::INK.gamma_multiply(0.16));
                (theme::INK, theme::INK, theme::ON_DARK)
            }
            Plate::Grey => {
                vgrad(
                    g,
                    r,
                    rad,
                    &[
                        (0.0, Color32::from_rgb(0xab, 0xa5, 0x97)),
                        (1.0, Color32::from_rgb(0x95, 0x8f, 0x81)),
                    ],
                );
                (theme::INK, theme::INK, theme::ON_DARK)
            }
            Plate::Bone => {
                vgrad(
                    g,
                    r,
                    rad,
                    &[
                        (0.0, theme::BONE_TOP),
                        (1.0, Color32::from_rgb(0xe3, 0xde, 0xd3)),
                    ],
                );
                (theme::INK, theme::INK, theme::ON_DARK)
            }
        }
    }
}

/// The strip module (A.7): a raised bone face with a ring, its top edge lit orange when
/// selected.
fn strip_face(g: &Painter, r: Rect, master: bool, selected: bool, lit: bool) {
    for (grow, a) in [(2.0, 14u8), (0.5, 22)] {
        g.rect_filled(
            r.translate(vec2(0.0, 2.0)).expand(grow),
            4.0 + grow,
            Color32::from_black_alpha(a),
        );
    }
    let (top, bottom) = if master {
        (
            Color32::from_rgb(0xe3, 0xde, 0xd2),
            Color32::from_rgb(0xd8, 0xd2, 0xc5),
        )
    } else {
        (
            Color32::from_rgb(0xed, 0xe9, 0xe0),
            Color32::from_rgb(0xe4, 0xdf, 0xd4),
        )
    };
    vgrad(g, r, 4.0, &[(0.0, top), (1.0, bottom)]);
    g.hline(
        r.left() + 4.0..=r.right() - 4.0,
        r.top() + 1.0,
        Stroke::new(1.0, Color32::WHITE),
    );
    g.vline(
        r.left() + 1.0,
        r.top() + 4.0..=r.bottom() - 4.0,
        Stroke::new(1.0, Color32::from_white_alpha(180)),
    );
    g.rect_stroke(
        r,
        4.0,
        Stroke::new(
            1.0,
            if selected {
                theme::INK
            } else {
                Color32::from_rgb(0xac, 0xa5, 0x94)
            },
        ),
        StrokeKind::Outside,
    );
    if lit {
        g.rect_filled(
            Rect::from_min_size(r.min, vec2(r.width(), 4.0)),
            egui::CornerRadius {
                nw: 4,
                ne: 4,
                sw: 0,
                se: 0,
            },
            theme::ORANGE,
        );
    }
}

/// What a fader or knob did this frame.
#[derive(Default, Clone, Copy)]
struct Moved {
    /// The new value, when it was dragged or double-clicked.
    value: Option<f32>,
    /// The gesture began / ended (a double click is both).
    started: bool,
    stopped: bool,
}

/// The pan knob at `c` showing `value` (-1..1); drag it, or double-click for the centre. Not
/// moved when `editable` is false (the strip plays its automation, or has no pan).
fn pan_knob(
    ui: &Ui,
    g: &Painter,
    c: Pos2,
    d: f32,
    id: egui::Id,
    value: f32,
    editable: bool,
) -> Moved {
    let r = Rect::from_center_size(c, Vec2::splat(d + 6.0));
    let mut out = Moved::default();
    let mut hot = false;
    if editable {
        let resp = ui.interact(r, id, Sense::click_and_drag());
        hot = resp.hovered() || resp.dragged();
        if resp.dragged() {
            let fine = if ui.input(|i| i.modifiers.shift) {
                0.002
            } else {
                0.01
            };
            out.value = Some((value - resp.drag_delta().y * fine).clamp(-1.0, 1.0));
        }
        if resp.double_clicked() {
            out.value = Some(0.0);
            out.started = true;
            out.stopped = true;
        }
        out.started |= resp.drag_started();
        out.stopped |= resp.drag_stopped();
    }
    knurl_knob(g, c, d, value, true, hot);
    out
}

/// The fader (slot, cap), its printed scale (ticks, numerals, ticks), the LED-ladder meter and
/// the ticks right of it, in that order left to right (A.4.5). `peaks` are the meter's levels
/// (linear). Returns what the fader did.
#[allow(clippy::too_many_arguments)]
fn fader_and_meter(
    ui: &Ui,
    g: &Painter,
    geo: &StripGeo,
    d: Density,
    id: egui::Id,
    db: f32,
    editable: bool,
    peaks: (f32, f32),
    inactive: bool,
) -> Moved {
    let (top, len) = (geo.top, geo.len);
    let fcx = geo.fcx;
    // A wide compact strip gets a wider meter.
    let mw = if d == COMPACT {
        (geo.iw * 0.18).clamp(d.meter_w, d.meter_w * 1.5)
    } else {
        d.meter_w
    };
    let big = d.cap.x >= 60.0;
    let meter_x = geo.x + geo.iw - mw - if big { 20.0 } else { d.tick_w + 6.0 };
    let tl = fcx + d.cap.x / 2.0 + if big { 6.0 } else { 4.0 };
    let tr = meter_x - if big { 6.0 } else { 4.0 } - d.tick_w;
    // Narrow strips give up the tick columns beside the numerals before the numerals touch
    // them: first the fader's, then the meter's.
    let num_w = d.num_size * 1.3 + 4.0;
    let num_x = if tr - (tl + d.tick_w) >= num_w {
        tick_col(g, tl, top, len, d.tick_w, false);
        tick_col(g, tr, top, len, d.tick_w, true);
        (tl + d.tick_w + tr) / 2.0
    } else if tr - tl >= num_w {
        tick_col(g, tr, top, len, d.tick_w, true);
        (tl + tr) / 2.0
    } else {
        (tl + meter_x) / 2.0
    };
    // On a short fader, every other numeral (+6 0 6 15 30 60), so they never crowd.
    let every = if len / 10.0 < d.num_size * 2.0 { 2 } else { 1 };
    for (i, label) in SCALE_LABELS.iter().enumerate().step_by(every) {
        hw::text(
            g,
            pos2(num_x, top + i as f32 * len / 10.0),
            Align2::CENTER_CENTER,
            label,
            Face::Bold,
            d.num_size,
            theme::INK2,
        );
    }
    tick_col(
        g,
        meter_x + mw + if big { 5.0 } else { 3.0 },
        top,
        len,
        (d.tick_w * 0.8).max(4.0),
        false,
    );

    // Meter ballistics: instant attack, release at 20 dB/s; the peak holds 1.5 s, then falls
    // at 20 dB/s.
    let state_id = id.with("ballistics");
    let (now, dt) = ui.input(|i| (i.time, i.stable_dt.min(0.1)));
    let mut st: (f32, f32, f32, f64) = ui.ctx().data(|m| {
        m.get_temp(state_id)
            .unwrap_or((-120.0, -120.0, -120.0, 0.0))
    });
    let raw = (lin_to_db(peaks.0), lin_to_db(peaks.1));
    st.0 = raw.0.max(st.0 - 20.0 * dt);
    st.1 = raw.1.max(st.1 - 20.0 * dt);
    let top_now = raw.0.max(raw.1);
    if top_now >= st.2 {
        st.2 = top_now;
        st.3 = now;
    } else if now - st.3 > 1.5 {
        st.2 -= 20.0 * dt;
    }
    ui.ctx().data_mut(|m| m.insert_temp(state_id, st));
    if st.0 > -60.0 || st.1 > -60.0 || st.2 > -60.0 {
        ui.ctx().request_repaint();
    }
    let well = Rect::from_min_max(
        pos2(meter_x, top - 8.0),
        pos2(meter_x + mw, top + len + 8.0),
    );
    ladder(
        g,
        well,
        (st.0, st.1),
        st.2,
        top,
        len,
        d.seg,
        d.seg_gap,
        inactive,
    );

    fader_slot(g, fcx, top, len);
    let cap_y = top + (1.0 - db_to_frac(db)) * len;
    let cap = Rect::from_center_size(pos2(fcx, cap_y), d.cap);
    let mut out = Moved::default();
    if editable {
        let hit = Rect::from_min_max(
            pos2(fcx - d.cap.x / 2.0 - 2.0, top - d.cap.y / 2.0),
            pos2(fcx + d.cap.x / 2.0 + 2.0, top + len + d.cap.y / 2.0),
        );
        let resp = ui.interact(hit, id, Sense::click_and_drag());
        if resp.dragged() {
            let fine = if ui.input(|i| i.modifiers.shift) {
                0.25
            } else {
                1.0
            };
            let f = db_to_frac(db) - resp.drag_delta().y * fine / len;
            out.value = Some(frac_to_db(f).clamp(-60.0, 6.0));
        }
        if resp.double_clicked() {
            out.value = Some(0.0);
            out.started = true;
            out.stopped = true;
        }
        out.started |= resp.drag_started();
        out.stopped |= resp.drag_stopped();
    }
    fader_cap(g, cap);
    out
}

/// Audio FX rows needed so every strip in `keys` fits (inserts + one empty slot).
pub fn fx_rows_for(p: &Project, keys: &[StripKey]) -> usize {
    keys.iter()
        .map(|k| match k {
            StripKey::Track(i) => p
                .tracks
                .get(*i)
                .map_or(0, |t| t.inserts.len() + t.au_effects.len()),
            StripKey::StereoOut => p.master_inserts.len() + p.master_au_effects.len(),
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// Send rows needed so every strip in `keys` fits: its sends, or the one empty slot of a strip
/// with none (a strip that has sends adds another from a send's menu).
pub fn send_rows_for(p: &Project, keys: &[StripKey]) -> usize {
    keys.iter()
        .map(|k| match k {
            StripKey::Track(i) => p.tracks.get(*i).map_or(0, |t| t.sends.len()),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
        .max(1)
}

/// The strip's send slots, one per row: the sends it has, or one empty slot when it has none.
/// `x`, `w` and `top` place the rows (see [`Rows`]). Picking a bus no Aux strip plays makes
/// that Aux strip, as Logic does, so the send is heard.
#[allow(clippy::too_many_arguments)]
fn send_slots(
    ui: &Ui,
    g: &Painter,
    area: Rect,
    d: Density,
    rows: usize,
    p: &mut Project,
    i: usize,
    uid: egui::Id,
    inactive: bool,
) {
    let labels = bus_labels(p);
    let sends = &mut p.tracks[i].sends;
    let (mut remove, mut picked, mut added) = (None, None, None);
    let top = area.top();
    for k in 0..rows {
        let r = Rect::from_min_size(
            pos2(area.left(), top + k as f32 * d.pitch),
            vec2(area.width(), d.sh),
        );
        if k < sends.len() {
            let can_add = sends.len() < MAX_SENDS;
            match send_slot(
                ui,
                g,
                r,
                d,
                &mut sends[k],
                &labels,
                uid.with(k),
                inactive,
                can_add,
            ) {
                SendPick::Remove => remove = Some(k),
                SendPick::Bus(b) => picked = Some(b),
                SendPick::Add(b) => added = Some(b),
                SendPick::Keep => {}
            }
        } else if k == 0 && sends.is_empty() {
            let resp = strip_slot(
                ui,
                g,
                r,
                uid.with(("empty", k)),
                SlotKind::Empty,
                "",
                None,
                Icon::None,
                false,
                d.slot_text,
            );
            let mut new = None;
            egui::Popup::menu(&resp).show(|ui| {
                ui.set_min_width(110.0);
                for (b, label) in (1u8..).zip(&labels) {
                    if ui.button(label).clicked() {
                        new = Some(b);
                        ui.close();
                    }
                }
            });
            added = new;
        }
    }
    if let Some(bus) = added {
        sends.push(SendSlot {
            bus: Some(bus),
            level: 0.5,
            pre: false,
        });
        picked = Some(bus);
    }
    // An automated send's LED (grey when overridden by hand), at the end of its slot.
    let t = &p.tracks[i];
    for k in 0..t.sends.len().min(rows) {
        let y = top + k as f32 * d.pitch + d.sh / 2.0;
        crate::automation_ui::led(
            g,
            pos2(area.right() - d.send_d - 8.0 - 7.0, y),
            &t.auto,
            AutoTarget::Send { slot: k as u8 },
        );
    }
    if let Some(k) = remove {
        p.tracks[i].sends.remove(k);
        p.tracks[i].auto.send_removed(k);
    }
    if let Some(bus) = picked {
        p.ensure_aux_for_bus(bus);
    }
}

/// The bus menu's entries, Bus 1 to Bus 8: a bus an Aux strip plays shows that strip's name
/// ("Bus 1 (Reverb)"); one no strip plays yet just its number.
fn bus_labels(p: &Project) -> Vec<String> {
    (1..=prowave_engine::MAX_BUSES as u8)
        .map(|b| match p.aux_for_bus(b) {
            Some(a) => trf("Bus {} ({})", &[&b, &p.tracks[a].name]),
            None => trf("Bus {}", &[&b]),
        })
        .collect()
}

/// What the user did with a send slot's menu.
enum SendPick {
    Keep,
    /// Picked this bus.
    Bus(u8),
    /// Chose "No Send".
    Remove,
    /// Added another send, to this bus.
    Add(u8),
}

/// A send slot: a black "Bus N" slot (its LED lit while the strip sounds) with the send's level
/// knob beside it. Click the slot to pick a bus; the eye marks a pre-fader send. The engine
/// copies the strip's signal to the bus, where the Aux strip that listens to it plays it.
/// `labels` names Bus 1 to Bus 8 in the menu (see [`bus_labels`]).
#[allow(clippy::too_many_arguments)]
fn send_slot(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    d: Density,
    send: &mut crate::model::SendSlot,
    labels: &[String],
    uid: egui::Id,
    inactive: bool,
    can_add: bool,
) -> SendPick {
    let Some(bus) = send.bus else {
        return SendPick::Remove;
    };
    let mut pick = SendPick::Keep;
    let kd = d.send_d;
    let knob_c = pos2(r.right() - kd / 2.0, r.center().y);
    let slot_r = Rect::from_min_max(r.min, pos2(r.right() - kd - 8.0, r.bottom()));
    let resp = strip_slot(
        ui,
        g,
        slot_r,
        uid,
        SlotKind::Filled,
        &trf("Bus {}", &[&bus]),
        Some(!inactive),
        Icon::None,
        inactive,
        d.slot_text,
    );
    if send.pre {
        // The eye a pre-fader send shows.
        let c = pos2(slot_r.right() - 10.0, slot_r.center().y);
        let ink = theme::ON_DARK;
        g.circle_stroke(c, 4.0, Stroke::new(1.2, ink));
        g.circle_filled(c, 1.6, ink);
    }
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(130.0);
        for (b, label) in (1u8..).zip(labels) {
            if ui.selectable_label(send.bus == Some(b), label).clicked() {
                send.bus = Some(b);
                pick = SendPick::Bus(b);
                ui.close();
            }
        }
        ui.separator();
        if ui.selectable_label(send.pre, tr("Pre Fader")).clicked() {
            send.pre = !send.pre;
            ui.close();
        }
        if ui.button(tr("No Send")).clicked() {
            pick = SendPick::Remove;
            ui.close();
        }
        // Another send: the next row of the strip.
        if can_add {
            ui.separator();
            ui.menu_button(tr("Sends"), |ui| {
                ui.set_min_width(110.0);
                for (b, label) in (1u8..).zip(labels) {
                    if ui.button(label).clicked() {
                        pick = SendPick::Add(b);
                        ui.close();
                    }
                }
            });
        }
    });
    // Level knob: drag vertically; double-click turns the send off.
    let kresp = ui.interact(
        Rect::from_center_size(knob_c, Vec2::splat(kd + 6.0)),
        uid.with("level"),
        Sense::click_and_drag(),
    );
    if kresp.dragged() {
        send.level = (send.level - kresp.drag_delta().y * 0.01).clamp(0.0, 1.0);
    }
    if kresp.double_clicked() {
        send.level = 0.0;
    }
    knurl_knob(
        g,
        knob_c,
        kd,
        send.level * 2.0 - 1.0,
        false,
        kresp.hovered() || kresp.dragged(),
    );
    pick
}

/// What the user chose in an Audio FX slot's menu.
enum InsertAct {
    Add(prowave_engine::FxKind),
    Remove,
    Bypass,
    Open,
    Move(isize),
}

/// The number the effect window and the Audio FX menus use for the Stereo Out strip (no track
/// has it): its effects are `Project::master_inserts`.
pub const MASTER_STRIP: usize = usize::MAX;

/// The effects of strip `i` (a track, or [`MASTER_STRIP`]).
fn strip_inserts(p: &Project, i: usize) -> &[Insert] {
    if i == MASTER_STRIP {
        &p.master_inserts
    } else {
        p.tracks.get(i).map_or(&[], |t| &t.inserts)
    }
}

/// One Audio FX slot of track `i` (or of the Stereo Out strip, [`MASTER_STRIP`]): the effect
/// in it (blue, grey when bypassed or silenced), or the empty slot after the last. Click a
/// filled slot for its menu, double-click for the effect's window.
///
/// The only built-in effect Pro Wave offers to add is Space Designer; other built-in effects
/// shown are those an older project brought along, which keep working. A strip's slots go on
/// with its native, Audio Unit and VST3 effects, then the empty slot ([`empty_fx_slot`]); the
/// Stereo Out strip has the same slots and menus as a track.
#[allow(clippy::too_many_arguments)]
fn insert_slot(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    p: &mut Project,
    v: &mut MixerView,
    i: usize,
    k: usize,
    uid: egui::Id,
    inactive: bool,
) {
    let count = strip_inserts(p, i).len();
    let Some(fx) = strip_inserts(p, i).get(k).copied() else {
        // After the built-in effects: the native and plug-in effects, then the empty slot
        // that adds one.
        let j = k - count;
        let au = p.strip_plugins(i).map_or(0, Vec::len);
        if j < au {
            au_effect_slot(ui, g, r, p, v, i, j, uid, inactive);
        } else if j == au {
            empty_fx_slot(ui, g, r, p, v, i, uid);
        }
        return;
    };
    let resp = ui.interact(r, uid, Sense::click());
    paint_strip_slot(
        g,
        r,
        SlotKind::Filled,
        fx.name(),
        Some(!(inactive || fx.bypass)),
        Icon::None,
        inactive || fx.bypass,
        slot_text(r.height()),
        resp.hovered(),
    );
    if resp.double_clicked() {
        v.fx_window = Some((i, k));
    }
    let mut act = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(150.0);
        if ui.button(tr("Open")).clicked() {
            act = Some(InsertAct::Open);
            ui.close();
        }
        if ui.selectable_label(fx.bypass, tr("Bypass")).clicked() {
            act = Some(InsertAct::Bypass);
            ui.close();
        }
        if k > 0 && ui.button(tr("Move Up")).clicked() {
            act = Some(InsertAct::Move(-1));
            ui.close();
        }
        if k + 1 < count && ui.button(tr("Move Down")).clicked() {
            act = Some(InsertAct::Move(1));
            ui.close();
        }
        ui.separator();
        if ui.button(tr("Remove")).clicked() {
            act = Some(InsertAct::Remove);
            ui.close();
        }
    });
    if let Some(act) = act {
        apply_insert(p, v, i, k, act);
    }
}

/// A menu of the Audio Units in `list` (instruments or effects), one submenu per maker.
/// Returns the one picked.
fn au_menu(
    ui: &mut Ui,
    list: &[crate::plugins::AuChoice],
    instruments: bool,
) -> Option<crate::plugins::AuChoice> {
    let mut picked = None;
    let prefer_vst3 = crate::plugins::prefer_vst3();
    let mut makers: Vec<&str> = list
        .iter()
        .filter(|c| c.instrument == instruments)
        .map(|c| c.manufacturer.as_str())
        .collect();
    // A maker with both plain and MIDI-taking effects (Soundtoys) is one submenu.
    makers.sort_by_key(|m| m.to_lowercase());
    makers.dedup();
    if makers.is_empty() {
        ui.label(tr("No Audio Units found"));
    }
    for m in makers {
        ui.menu_button(m, |ui| {
            let mut items: Vec<&crate::plugins::AuChoice> = list
                .iter()
                .filter(|c| c.instrument == instruments && c.manufacturer == m)
                .collect();
            items.sort_by_key(|c| c.name.to_lowercase());
            let off = crate::plugins::quarantined();
            for c in items {
                if let Some(id) = plugin_menu_item(ui, c, &off, prefer_vst3) {
                    crate::plugins::release(&id);
                    picked = Some(crate::plugins::AuChoice {
                        id,
                        alt_id: None,
                        ..c.clone()
                    });
                    ui.close();
                }
            }
        });
    }
    // A plug-in installed as both AU and VST3 is one entry; this picks which one it loads.
    if list.iter().any(|c| c.alt_id.is_some()) {
        ui.separator();
        let mut on = prefer_vst3;
        if ui.checkbox(&mut on, tr("Prefer VST3")).changed() {
            crate::plugins::set_prefer_vst3(on);
        }
    }
    picked
}

/// One plug-in in [`au_menu`]: its name (loads the preferred format), and when it is
/// installed as both AU and VST3, a small button for each. Returns the id picked.
fn plugin_menu_item(
    ui: &mut Ui,
    c: &crate::plugins::AuChoice,
    off: &[String],
    prefer_vst3: bool,
) -> Option<String> {
    let warn = off.contains(&c.id) || c.alt_id.as_ref().is_some_and(|a| off.contains(a));
    let only_vst3 = crate::plugins::format_label(&c.id) == "VST3";
    let mut label = c.name.clone();
    if only_vst3 {
        label.push_str("  (VST3)");
    }
    if warn {
        label.push_str(" ⚠");
    }
    let Some(alt) = &c.alt_id else {
        return ui.button(label).clicked().then(|| c.id.clone());
    };
    let mut picked = None;
    ui.horizontal(|ui| {
        if ui.button(label).clicked() {
            picked = Some(c.preferred_id(prefer_vst3).to_string());
        }
        for (name, id, default) in [("AU", &c.id, !prefer_vst3), ("VST3", alt, prefer_vst3)] {
            let b = egui::Button::new(egui::RichText::new(name).small()).selected(default);
            if ui
                .add(b)
                .on_hover_text(format!("{} as {name}", c.name))
                .clicked()
            {
                picked = Some(id.clone());
            }
        }
    });
    picked
}

/// The slot's tooltip: the plug-in's name and its format.
fn plugin_tooltip(s: &crate::plugins::PluginSlot) -> String {
    match crate::plugins::format_label(&s.id) {
        "" => s.name.clone(),
        "AU" => format!("{}\nAudio Unit", s.name),
        f => format!("{}\n{f}", s.name),
    }
}

/// The Instrument slot of a software instrument or Drummer strip: the Audio Unit instrument,
/// or the SoundFont the track plays, or a dimmed "Instrument" when nothing is loaded (the
/// track is silent then). A click opens the loaded instrument's window (or, when empty, the
/// instrument menu); a right-click opens the menu to change or remove it.
#[allow(clippy::too_many_arguments)]
fn instrument_slot(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    p: &mut Project,
    v: &mut MixerView,
    i: usize,
    uid: egui::Id,
) {
    let t = &p.tracks[i];
    let loaded = t.au_instrument.clone();
    // The Input slot of an instrument strip: a bone slot with the keyboard and what plays the
    // track ("Instrument" when nothing does: the track is silent then).
    let label = match (&loaded, &t.soundfont) {
        (Some(au), _) => au.name.clone(),
        (None, Some(sf)) => crate::inspector::soundfont_label(sf),
        (None, None) => tr("Instrument").to_string(),
    };
    let mut resp = ui.interact(r, uid, Sense::click());
    paint_strip_slot(
        g,
        r,
        SlotKind::Plain,
        &label,
        None,
        Icon::Keys,
        false,
        slot_text(r.height()),
        resp.hovered(),
    );
    if let Some(au) = &loaded {
        resp = resp.on_hover_text(plugin_tooltip(au));
    }
    // As in Logic: a click opens the loaded instrument; the menu (change, remove) is on a
    // right-click. An empty slot opens the menu on a click.
    if loaded.is_some() && resp.clicked() {
        if let Some(au) = &loaded {
            v.open_plugin = Some((au.uid, au.name.clone()));
        }
    }
    let (mut pick, mut remove, mut open) = (None, false, false);
    // A built-in instrument picked: what makes its slot.
    let mut built_in: Option<fn() -> crate::plugins::PluginSlot> = None;
    let list = &v.au_list;
    let popup = if loaded.is_some() {
        egui::Popup::context_menu(&resp)
    } else {
        egui::Popup::menu(&resp)
    };
    popup.show(|ui| {
        ui.set_min_width(170.0);
        if loaded.is_some() {
            if ui.button(tr("Open")).clicked() {
                open = true;
                ui.close();
            }
            ui.separator();
        }
        if ui.button(tr("Channel Rack")).clicked() {
            built_in = Some(crate::channel_rack::new_slot);
            ui.close();
        }
        ui.separator();
        if let Some(c) = au_menu(ui, list, true) {
            pick = Some(c.clone());
        }
        if loaded.is_some() {
            ui.separator();
            if ui.button(tr("Remove")).clicked() {
                remove = true;
                ui.close();
            }
        }
    });
    let t = &mut p.tracks[i];
    if let Some(new_slot) = built_in {
        let slot = new_slot();
        v.open_plugin = Some((slot.uid, slot.name.clone()));
        t.au_instrument = Some(slot);
        t.soundfont = None;
    } else if let Some(c) = pick {
        t.au_instrument = Some(crate::plugins::PluginSlot::new(&c.id, &c.label()));
        // One instrument plays the track.
        t.soundfont = None;
        if let Some(au) = &t.au_instrument {
            v.open_plugin = Some((au.uid, au.name.clone()));
        }
    } else if remove {
        t.au_instrument = None;
    } else if open {
        if let Some(au) = &loaded {
            v.open_plugin = Some((au.uid, au.name.clone()));
        }
    }
}

/// The empty Audio FX slot after the last effect of track `i` (or of the Stereo Out strip,
/// [`MASTER_STRIP`]): click for the native effects, Space Designer and the Audio Unit and VST3
/// effects, one submenu per maker.
fn empty_fx_slot(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    p: &mut Project,
    v: &mut MixerView,
    i: usize,
    uid: egui::Id,
) {
    let resp = strip_slot(
        ui,
        g,
        r,
        uid,
        SlotKind::Empty,
        "",
        None,
        Icon::None,
        false,
        slot_text(r.height()),
    );
    let mut pick = None;
    let list = &v.au_list;
    let mut native = None;
    let room = strip_inserts(p, i).len() < prowave_engine::MAX_INSERTS;
    let mut space = false;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(170.0);
        // The native effects (FL Studio's dynamics, as Lemon Studio recreated them).
        for info in prowave_engine::native_effects() {
            if ui.button(info.name).clicked() {
                native = Some(info);
                ui.close();
            }
        }
        if room && ui.button(tr("Space Designer")).clicked() {
            space = true;
            ui.close();
        }
        ui.separator();
        if let Some(c) = au_menu(ui, list, false) {
            pick = Some(c.clone());
        }
    });
    if space {
        apply_insert(p, v, i, 0, InsertAct::Add(prowave_engine::FxKind::Reverb));
        return;
    }
    let slot = match (native, pick) {
        (Some(info), _) => Some(crate::native_fx_ui::new_slot(info)),
        (None, Some(c)) => Some(crate::plugins::PluginSlot::new(&c.id, &c.label())),
        _ => None,
    };
    if let (Some(slot), Some((fx, _))) = (slot, p.strip_plugins_mut(i)) {
        v.open_plugin = Some((slot.uid, slot.name.clone()));
        fx.push(slot);
    }
}

/// Plug-in effect `j` of track `i` (or of the Stereo Out strip, [`MASTER_STRIP`]): click for
/// its menu, double-click for its window.
#[allow(clippy::too_many_arguments)]
fn au_effect_slot(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    p: &mut Project,
    v: &mut MixerView,
    i: usize,
    j: usize,
    uid: egui::Id,
    inactive: bool,
) {
    let Some(fx) = p.strip_plugins(i) else {
        return;
    };
    let (Some(mut au), count) = (fx.get(j).cloned(), fx.len()) else {
        return;
    };
    // A plug-in on the Stereo Out delays everything; its slot says how much.
    let delay = (i == MASTER_STRIP && !au.bypass)
        .then(|| {
            let running = fx[..j].iter().filter(|e| !e.bypass).count();
            v.master_latency.get(running).copied()
        })
        .flatten()
        .filter(|s| *s > 0.0);
    // A native effect is shown by its name now (projects keep the name they were saved with).
    if let Some(info) = prowave_engine::native_info(&au.id) {
        au.name = info.name.to_string();
    }
    let off = inactive || au.bypass;
    let mut tip = plugin_tooltip(&au);
    if let Some(s) = delay {
        tip.push('\n');
        tip.push_str(&trf("Latency: {} ms", &[&format!("{:.1}", s * 1000.0)]));
    }
    let resp = ui.interact(r, uid, Sense::click()).on_hover_text(tip);
    paint_strip_slot(
        g,
        r,
        SlotKind::Filled,
        &au.name,
        Some(!off),
        Icon::None,
        off,
        slot_text(r.height()),
        resp.hovered(),
    );
    if resp.double_clicked() {
        v.open_plugin = Some((au.uid, au.name.clone()));
    }
    let mut act = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(150.0);
        if ui.button(tr("Open")).clicked() {
            act = Some(InsertAct::Open);
            ui.close();
        }
        if ui.selectable_label(au.bypass, tr("Bypass")).clicked() {
            act = Some(InsertAct::Bypass);
            ui.close();
        }
        if j > 0 && ui.button(tr("Move Up")).clicked() {
            act = Some(InsertAct::Move(-1));
            ui.close();
        }
        if j + 1 < count && ui.button(tr("Move Down")).clicked() {
            act = Some(InsertAct::Move(1));
            ui.close();
        }
        ui.separator();
        if ui.button(tr("Remove")).clicked() {
            act = Some(InsertAct::Remove);
            ui.close();
        }
    });
    if let Some(act) = act {
        apply_plugin(p, v, i, j, act);
    }
}

/// Carries out what the user chose in the menu of plug-in effect `j` of strip `i` (a track or
/// [`MASTER_STRIP`]). Choices that do not fit do nothing.
fn apply_plugin(p: &mut Project, v: &mut MixerView, i: usize, j: usize, act: InsertAct) {
    let Some((fx, auto)) = p.strip_plugins_mut(i) else {
        return;
    };
    if j >= fx.len() {
        return;
    }
    match act {
        InsertAct::Open => v.open_plugin = Some((fx[j].uid, fx[j].name.clone())),
        InsertAct::Bypass => fx[j].bypass = !fx[j].bypass,
        InsertAct::Move(d) => {
            let to = j as isize + d;
            if to >= 0 && (to as usize) < fx.len() {
                fx.swap(j, to as usize);
                // Its envelopes go with the plug-in.
                auto.plugin_swapped(j, to as usize);
            }
        }
        InsertAct::Remove => {
            fx.remove(j);
            auto.plugin_removed(j);
        }
        InsertAct::Add(_) => {}
    }
}

/// Carries out what the user chose in the menu of Audio FX slot `k` of track `i`. Choices that
/// do not fit (a slot that is not there) do nothing.
fn apply_insert(p: &mut Project, v: &mut MixerView, i: usize, k: usize, act: InsertAct) {
    // The Stereo Out strip's envelopes are the project's master automation.
    let (inserts, mut auto) = if i == MASTER_STRIP {
        (&mut p.master_inserts, Some(&mut p.master_auto))
    } else {
        let Some(t) = p.tracks.get_mut(i) else {
            return;
        };
        (&mut t.inserts, Some(&mut t.auto))
    };
    let filled = k < inserts.len();
    match act {
        InsertAct::Add(kind) => {
            if inserts.len() < prowave_engine::MAX_INSERTS {
                inserts.push(Insert::new(kind));
                v.fx_window = Some((i, inserts.len() - 1));
            }
        }
        InsertAct::Remove if filled => {
            inserts.remove(k);
            if let Some(a) = auto.as_mut() {
                a.insert_removed(k);
            }
            v.fx_window = match v.fx_window {
                Some((t, s)) if t == i && s == k => None,
                Some((t, s)) if t == i && s > k => Some((t, s - 1)),
                other => other,
            };
        }
        InsertAct::Bypass if filled => inserts[k].bypass = !inserts[k].bypass,
        InsertAct::Open if filled => v.fx_window = Some((i, k)),
        InsertAct::Move(d) if filled => {
            let to = k as isize + d;
            if to < 0 || to as usize >= inserts.len() {
                return;
            }
            let to = to as usize;
            inserts.swap(k, to);
            if let Some(a) = auto.as_mut() {
                a.insert_swapped(k, to);
            }
            if v.fx_window == Some((i, k)) {
                v.fx_window = Some((i, to));
            } else if v.fx_window == Some((i, to)) {
                v.fx_window = Some((i, k));
            }
        }
        _ => {}
    }
}

/// Where the strip is shown, which changes a few labels and the selection mark.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NameStyle {
    /// In the Mixer: the selected strip gets the orange top edge and an ink ring.
    Coloured,
    /// In the Inspector: no selection mark, and the Stereo Out without its empty wells.
    Plain,
}

/// The number on a strip's name plate: the track's ("01"), or "ST" for the Stereo Out.
fn plate_number(key: StripKey) -> String {
    match key {
        StripKey::Track(i) => format!("{:02}", i + 1),
        StripKey::StereoOut => "ST".into(),
    }
}

/// Draws one channel strip in `strip` and applies its edits to the project.
/// `ns` keeps widget ids unique when the same strip is shown in two places.
#[allow(clippy::too_many_arguments)]
pub fn channel_strip(
    ui: &Ui,
    g: &Painter,
    strip: Rect,
    key: StripKey,
    p: &mut Project,
    v: &mut MixerView,
    meters: &Meters,
    r: &Rows,
    ns: &'static str,
    name_style: NameStyle,
) {
    let d = r.d;
    let selected = matches!(key, StripKey::Track(i) if i == p.selected_track);
    let muted = matches!(key, StripKey::Track(i) if p.is_muted(i));
    let inactive = matches!(key, StripKey::Track(i)
        if p.is_muted(i) || crate::tracks_area::silenced_by_solo(p, i));
    // Whole-strip click selects (registered first so the controls on top win).
    if let StripKey::Track(i) = key {
        if ui
            .interact(strip, id(ns, (key, "strip")), Sense::click())
            .clicked()
        {
            p.selected_track = i;
        }
    }
    let master = key == StripKey::StereoOut;
    let mixer_sel = selected && name_style == NameStyle::Coloured;
    strip_face(g, strip, master, mixer_sel, mixer_sel);
    let geo = strip_geo(strip, r);
    let (sx, inner_w) = (geo.x, geo.iw);
    let row = |y: f32| Rect::from_min_size(pos2(sx, y), vec2(inner_w, d.sh));
    let text = d.slot_text;

    let (name, kind) = match key {
        StripKey::Track(i) => {
            let t = &p.tracks[i];
            (t.name.clone(), Some(t.kind))
        }
        StripKey::StereoOut => (tr("Stereo Out").to_string(), None),
    };
    let beat = p.position_beats;

    // A VCA strip is a fader with Mute and Solo: none of the channel strip's slots.
    let vca_strip = kind == Some(TrackKind::Vca);
    if vca_strip {
        hw::silk(
            g,
            pos2(strip.center().x, r.setting + d.sh / 2.0),
            Align2::CENTER_CENTER,
            tr("VCA"),
            d.name_text,
            theme::INK2,
        );
    }
    if !vca_strip {
        match key {
            StripKey::Track(i) => {
                let resp = strip_slot(
                    ui,
                    g,
                    row(r.setting),
                    id(ns, (key, "setting")),
                    SlotKind::Plain,
                    tr("Setting"),
                    None,
                    Icon::None,
                    false,
                    text,
                );
                setting_menu(&resp, p, v, i);
            }
            // The Stereo Out has no setting and no input: the rows stay as empty wells.
            StripKey::StereoOut if name_style == NameStyle::Coloured => {
                hw::recess(g, row(r.setting), 3.0);
                hw::recess(g, row(r.input), 3.0);
            }
            StripKey::StereoOut => {}
        }
        let input_r = row(r.input);
        match kind {
            Some(TrackKind::Instrument | TrackKind::Drummer) => {
                if let StripKey::Track(i) = key {
                    instrument_slot(ui, g, input_r, p, v, i, id(ns, (key, "instrument")));
                }
            }
            // An aux strip's input is the bus it listens to: Aux 1 takes Bus 1, and so on.
            Some(TrackKind::Aux) => {
                let n = match key {
                    StripKey::Track(i) => p.aux_bus_of(i),
                    _ => 1,
                };
                if n == 0 {
                    hw::recess(g, input_r, 3.0);
                } else {
                    paint_strip_slot(
                        g,
                        input_r,
                        SlotKind::Plain,
                        &trf("Bus {}", &[&n]),
                        None,
                        Icon::None,
                        false,
                        text,
                        false,
                    );
                }
            }
            // An external MIDI strip shows where its notes go: the port and the channel.
            Some(TrackKind::ExternalMidi) => {
                let label = match key {
                    StripKey::Track(i) => {
                        let t = &p.tracks[i];
                        let port = t
                            .midi_port
                            .clone()
                            .unwrap_or_else(|| tr("No Port").to_string());
                        format!("{port} · {}", t.midi_channel)
                    }
                    _ => String::new(),
                };
                paint_strip_slot(
                    g,
                    input_r,
                    SlotKind::Plain,
                    &label,
                    None,
                    Icon::None,
                    false,
                    text,
                    false,
                );
            }
            Some(TrackKind::Audio) => {
                if let StripKey::Track(i) = key {
                    let slot_id = id(ns, (key, "input"));
                    input_slot(ui, g, input_r, p, &mut v.inputs, i, slot_id);
                }
            }
            _ => {}
        }
        // The Stereo Out strip's effects are the master bus chain (`Project::master_inserts`).
        let strip_no = match key {
            StripKey::Track(i) => i,
            StripKey::StereoOut => MASTER_STRIP,
        };
        for k in 0..r.fx_rows {
            let rr = row(r.fx + k as f32 * d.pitch);
            insert_slot(
                ui,
                g,
                rr,
                p,
                v,
                strip_no,
                k,
                id(ns, (key, "fx", k)),
                inactive,
            );
        }
        match key {
            // An Aux strip takes its signal from a bus: it has no sends of its own.
            StripKey::Track(i) if p.tracks[i].kind != TrackKind::Aux => {
                send_slots(
                    ui,
                    g,
                    row(r.sends),
                    d,
                    r.send_rows,
                    p,
                    i,
                    id(ns, (key, "send")),
                    inactive,
                );
            }
            _ => {}
        }
        match key {
            StripKey::Track(i) => {
                let out = tr("St Out");
                output_slot(ui, g, row(r.output), p, i, out, id(ns, (key, "output")));
                group_slot(ui, g, row(r.group), p, i, id(ns, (key, "vca")));
            }
            // The main bus: two channels and no pan, marked in a dashed panel where the sends,
            // output, group and pan of a track strip go.
            StripKey::StereoOut => {
                let panel = Rect::from_min_max(
                    pos2(sx, r.sends),
                    pos2(sx + inner_w, r.pan + d.pan_h - 6.0),
                );
                let st = Stroke::new(1.0, theme::INK2.gamma_multiply(0.32));
                let c = [
                    panel.left_top(),
                    panel.right_top(),
                    panel.right_bottom(),
                    panel.left_bottom(),
                    panel.left_top(),
                ];
                for w in c.windows(2) {
                    g.extend(Shape::dashed_line(&[w[0], w[1]], st, 4.0, 3.0));
                }
                let mid = panel.center();
                hw::silk(
                    g,
                    mid - vec2(0.0, 8.0),
                    Align2::CENTER_CENTER,
                    "MAIN BUS",
                    11.0,
                    theme::PRINT,
                );
                hw::silk(
                    g,
                    mid + vec2(0.0, 8.0),
                    Align2::CENTER_CENTER,
                    "2 CH · NO PAN",
                    11.0,
                    theme::PRINT,
                );
            }
        }
    }
    let pan_c = pos2(strip.center().x, r.pan + d.pan_h / 2.0 + 1.0);
    match key {
        _ if vca_strip => {}
        StripKey::Track(i) => {
            let t = &p.tracks[i];
            let shown = crate::automation::shown_value(t, AutoTarget::Pan, beat);
            let editable = crate::automation::editable(t, AutoTarget::Pan);
            let m = pan_knob(
                ui,
                g,
                pan_c,
                d.knob_d,
                id(ns, (key, "pan")),
                shown,
                editable,
            );
            crate::automation_ui::led(
                g,
                pan_c + vec2(d.knob_d / 2.0 + 6.0, -d.knob_d / 2.0),
                &t.auto,
                AutoTarget::Pan,
            );
            if let Some(value) = m.value {
                let delta = value - shown;
                crate::automation::moved(&mut p.tracks[i], AutoTarget::Pan, value);
                for j in p.group_mates(i, |g| g.pan) {
                    let to = p.tracks[j].pan + delta;
                    crate::automation::moved(&mut p.tracks[j], AutoTarget::Pan, to);
                }
            }
            if m.stopped {
                let playing = p.playing;
                for j in std::iter::once(i).chain(p.group_mates(i, |g| g.pan)) {
                    crate::automation::released(&mut p.tracks[j], AutoTarget::Pan, beat, playing);
                }
            }
        }
        // Stereo Out has no pan in the engine: no knob.
        StripKey::StereoOut => {}
    }

    // Volume (dB) and peak-hold readouts.
    // Sends on Faders: the fader sets the strip's send to that bus instead of its level.
    let send_bus = match key {
        StripKey::Track(i) if !matches!(p.tracks[i].kind, TrackKind::Aux | TrackKind::Vca) => {
            v.sends_on_faders
        }
        _ => None,
    };
    let send_db = |sends: &[SendSlot], bus: u8| {
        let level = sends
            .iter()
            .find(|s| s.bus == Some(bus))
            .map_or(0.0, |s| s.level);
        lin_to_db(prowave_engine::send_gain(level)).max(-60.0)
    };
    let vol_db = match key {
        StripKey::Track(i) => match send_bus {
            Some(bus) => send_db(&p.tracks[i].sends, bus),
            None => crate::automation::shown_value(&p.tracks[i], AutoTarget::Volume, beat),
        },
        StripKey::StereoOut => p
            .master_auto
            .played(AutoTarget::Volume, beat)
            .unwrap_or_else(|| lin_to_db(p.master_vol).max(-60.0)),
    };
    let peaks = match key {
        StripKey::Track(i) => meters.tracks.get(i).copied().unwrap_or((0.0, 0.0)),
        _ => meters.master,
    };
    let hold = v.peak_hold.entry(key).or_insert(0.0);
    *hold = hold.max(peaks.0.max(peaks.1));
    let half = (inner_w - 6.0) / 2.0;
    let vol_r = Rect::from_min_size(pos2(sx, r.db), vec2(half, d.db_h));
    let pk_r = Rect::from_min_size(pos2(sx + half + 6.0, r.db), vec2(half, d.db_h));
    lcd_chip(g, vol_r, "VOL", Some(&format!("{vol_db:.1}")), d);
    // The fader's LED: lit while an envelope moves it, grey when it is overridden.
    let fader_auto = match key {
        StripKey::Track(i) => Some(&p.tracks[i].auto),
        StripKey::StereoOut => Some(&p.master_auto),
    };
    if let (Some(a), None) = (fader_auto, send_bus) {
        crate::automation_ui::led(
            g,
            pos2(vol_r.right() - 5.0, vol_r.top() + 5.0),
            a,
            AutoTarget::Volume,
        );
    }
    let hold_db = lin_to_db(*hold);
    let pk_text = (*hold > 1e-5).then(|| format!("{hold_db:.1}"));
    lcd_chip(g, pk_r, "PEAK", pk_text.as_deref(), d);
    if hold_db > 0.0 {
        // Over 0 dB: the chip's label lights in signal orange.
        hw::text(
            g,
            pk_r.left_top() + vec2(5.0, 4.0),
            Align2::LEFT_TOP,
            "PEAK",
            Face::Lcd,
            d.chip_label,
            theme::ORANGE_DK,
        );
    }
    if ui
        .interact(pk_r, id(ns, (key, "peak")), Sense::click())
        .clicked()
    {
        *hold = 0.0;
    }

    // Fader + scale + meter.
    let fader_free = match key {
        StripKey::Track(i) => {
            send_bus.is_some() || crate::automation::editable(&p.tracks[i], AutoTarget::Volume)
        }
        StripKey::StereoOut => true,
    };
    let moved = fader_and_meter(
        ui,
        g,
        &geo,
        d,
        id(ns, (key, "fader")),
        vol_db,
        fader_free,
        peaks,
        inactive,
    );
    if let Some(db) = moved.value {
        match key {
            StripKey::Track(i) => match send_bus {
                Some(bus) => set_send_level(&mut p.tracks[i].sends, bus, db),
                None => {
                    let delta = db - vol_db;
                    crate::automation::moved(&mut p.tracks[i], AutoTarget::Volume, db);
                    for j in p.group_mates(i, |g| g.volume) {
                        let to = p.tracks[j].volume_db + delta;
                        crate::automation::moved(&mut p.tracks[j], AutoTarget::Volume, to);
                    }
                }
            },
            StripKey::StereoOut => p.master_vol = 10f32.powf(db / 20.0).min(2.0),
        }
    }
    if let (true, StripKey::Track(i), None) = (moved.stopped, key, send_bus) {
        let playing = p.playing;
        for j in std::iter::once(i).chain(p.group_mates(i, |g| g.volume)) {
            crate::automation::released(&mut p.tracks[j], AutoTarget::Volume, beat, playing);
        }
    }

    // M / S
    let bw = (inner_w - 8.0) / 2.0;
    let m_r = Rect::from_min_size(geo.ms.min, vec2(bw, d.ms_h));
    let s_r = Rect::from_min_size(geo.ms.min + vec2(bw + 8.0, 0.0), vec2(bw, d.ms_h));
    match key {
        StripKey::Track(i) => {
            // Mute shows its envelope while that plays; a click overrides it.
            let muted = crate::automation::shown_value(&p.tracks[i], AutoTarget::Mute, beat) >= 0.5;
            let t = &mut p.tracks[i];
            if strip_key(ui, g, m_r, id(ns, (key, "m")), "M", muted, false, d.ms_text) {
                t.mute = !muted;
            }
            crate::automation_ui::led(
                g,
                pos2(m_r.right() - 7.0, m_r.top() + 6.0),
                &t.auto,
                AutoTarget::Mute,
            );
            if strip_key(ui, g, s_r, id(ns, (key, "s")), "S", t.solo, true, d.ms_text) {
                t.solo = !t.solo;
            }
        }
        // Stereo Out has no mute and no bounce button (Bounce is not built).
        StripKey::StereoOut => {}
    }

    // Name plate: click selects the track, double-click renames it.
    let name_r = geo.name;
    let font = theme::font(Face::Silk, d.name_text);
    if let StripKey::Track(i) = key {
        let resp = ui.interact(name_r, id(ns, (key, "name")), Sense::click());
        if resp.clicked() {
            p.selected_track = i;
        }
        let rename_key = id(ns, (key, "rename"));
        if let Some(new) = crate::widgets::rename_field(ui, &resp, name_r, rename_key, &name, font)
        {
            p.tracks[i].name = new;
        }
    }
    let plate_g = if muted {
        let mut f = g.clone();
        f.set_opacity(0.4);
        f
    } else {
        g.clone()
    };
    name_plate(
        &plate_g,
        name_r,
        plate_of(kind),
        selected,
        &plate_number(key),
        &name,
        if strip.width() < 170.0 {
            d.name_text - 1.0
        } else {
            d.name_text
        }
        .max(12.0),
    );
}

/// Sets the send to `bus` from a fader position in dB (a new send is made when needed, and
/// one turned right down is taken away).
fn set_send_level(sends: &mut Vec<SendSlot>, bus: u8, db: f32) {
    let level = if db <= -59.5 {
        0.0
    } else {
        (10f32.powf(db / 20.0) / 2.0).sqrt().clamp(0.0, 1.0)
    };
    match sends.iter().position(|s| s.bus == Some(bus)) {
        Some(k) => sends[k].level = level,
        None if level > 0.0 && sends.len() < MAX_SENDS => sends.push(SendSlot {
            bus: Some(bus),
            level,
            pre: false,
        }),
        None => {}
    }
}

/// The strip's Group slot: the mix group of the track, else the VCA track that moves its level
/// (empty for none). Click to pick a mix group, make a new one, change what the group moves
/// together, or pick one of the project's VCA tracks.
/// A strip's Setting slot: copy, paste or clear what is on the strip.
fn setting_menu(resp: &egui::Response, p: &mut Project, v: &mut MixerView, i: usize) {
    egui::Popup::menu(resp).show(|ui| {
        ui.set_min_width(170.0);
        let t = &mut p.tracks[i];
        if ui.button(tr("Copy Channel Strip")).clicked() {
            v.copied = Some(CopiedStrip::of(t));
            ui.close();
        }
        let paste = egui::Button::new(tr("Paste Channel Strip"));
        if ui.add_enabled(v.copied.is_some(), paste).clicked() {
            if let Some(c) = v.copied.clone() {
                c.paste(t);
            }
            ui.close();
        }
        ui.separator();
        if ui.button(tr("Reset Channel Strip")).clicked() {
            t.inserts.clear();
            t.au_effects.clear();
            t.sends.clear();
            t.volume_db = 0.0;
            t.pan = 0.0;
            ui.close();
        }
    });
}

/// A strip's Output slot: Stereo Out or a bus (an Aux strip plays that bus). Picking a bus no
/// Aux strip plays makes that Aux strip, as Logic does, so the track is still heard. An Aux
/// strip always goes to Stereo Out.
fn output_slot(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    p: &mut Project,
    i: usize,
    stereo_out: &str,
    id: egui::Id,
) {
    let current = p.tracks[i].output;
    let text = if current == 0 {
        stereo_out.to_string()
    } else {
        trf("Bus {}", &[&current])
    };
    let size = slot_text(r.height());
    if p.tracks[i].kind == TrackKind::Aux {
        paint_strip_slot(
            g,
            r,
            SlotKind::Plain,
            &text,
            None,
            Icon::None,
            false,
            size,
            false,
        );
        return;
    }
    let resp = strip_slot(
        ui,
        g,
        r,
        id,
        SlotKind::Plain,
        &text,
        None,
        Icon::None,
        false,
        size,
    );
    let mut picked = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(120.0);
        if ui
            .selectable_label(current == 0, tr("Stereo Out"))
            .clicked()
        {
            picked = Some(0);
            ui.close();
        }
        for (bus, label) in (1u8..).zip(bus_labels(p)) {
            if ui.selectable_label(current == bus, label).clicked() {
                picked = Some(bus);
                ui.close();
            }
        }
    });
    if let Some(bus) = picked {
        set_output(p, i, bus);
    }
}

/// Sends track `i` to `bus` (0 for Stereo Out), making the Aux strip that plays the bus when
/// there is none.
fn set_output(p: &mut Project, i: usize, bus: u8) {
    p.tracks[i].output = bus;
    p.ensure_aux_for_bus(bus);
}

/// An audio strip's Input slot: which inputs the track records, and from which device.
fn input_slot(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    p: &mut Project,
    inputs: &mut crate::audio_rec::InputChoice,
    i: usize,
    id: egui::Id,
) {
    use crate::audio_rec::input_label;
    use prowave_engine::InputSource;
    let source = p.tracks[i].input;
    // One ring for a mono input, two for a stereo pair.
    let icon = if matches!(source, InputSource::Stereo(_)) {
        Icon::Rings
    } else {
        Icon::Ring
    };
    let resp = strip_slot(
        ui,
        g,
        r,
        id,
        SlotKind::Plain,
        &input_label(source),
        None,
        icon,
        false,
        slot_text(r.height()),
    );
    if resp.clicked() {
        inputs.refresh();
    }
    let channels = inputs.channels();
    let (mut picked, mut device, mut output) = (None, None, None);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(150.0);
        let mut item = |ui: &mut Ui, s: InputSource| {
            if ui.selectable_label(s == source, input_label(s)).clicked() {
                picked = Some(s);
                ui.close();
            }
        };
        for c in 0..channels {
            item(ui, InputSource::Mono(c));
        }
        ui.separator();
        for c in (0..channels.saturating_sub(1)).step_by(2) {
            item(ui, InputSource::Stereo(c));
        }
        ui.separator();
        ui.menu_button(tr("Input Device"), |ui| {
            if ui
                .selectable_label(inputs.device.is_none(), tr("System Default"))
                .clicked()
            {
                device = Some(None);
                ui.close();
            }
            for d in &inputs.devices {
                let on = inputs.device.as_deref() == Some(d.id.as_str());
                if ui.selectable_label(on, &d.name).clicked() {
                    device = Some(Some(d.id.clone()));
                    ui.close();
                }
            }
            if inputs.devices.is_empty() {
                ui.label(tr("No audio input found"));
            }
        });
        // macOS: the engine plays through its own Core Audio device list.
        if cfg!(target_os = "macos") {
            ui.menu_button(tr("Output Device"), |ui| {
                if ui
                    .selectable_label(inputs.output.is_none(), tr("System Default"))
                    .clicked()
                {
                    output = Some(None);
                    ui.close();
                }
                for d in &inputs.outputs {
                    let on = inputs.output.as_deref() == Some(d.id.as_str());
                    if ui.selectable_label(on, &d.name).clicked() {
                        output = Some(Some(d.id.clone()));
                        ui.close();
                    }
                }
            });
        }
    });
    if let Some(s) = picked {
        p.tracks[i].input = s;
    }
    if let Some(d) = device {
        inputs.pick(d);
    }
    if let Some(d) = output {
        inputs.pick_output(d);
    }
}

fn group_slot(ui: &Ui, g: &Painter, r: Rect, p: &mut Project, i: usize, id: egui::Id) {
    let group = p.tracks[i].group.filter(|g| *g < p.groups.len());
    let name = match group {
        Some(g) => p.groups[g].name.clone(),
        None => p.vca_of(i).map(|v| v.name.clone()).unwrap_or_default(),
    };
    let kind = if name.is_empty() {
        SlotKind::Empty
    } else {
        SlotKind::Plain
    };
    let vcas: Vec<(usize, String)> = p
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TrackKind::Vca)
        .map(|(j, t)| (j, t.name.clone()))
        .collect();
    let resp = strip_slot(
        ui,
        g,
        r,
        id,
        kind,
        &name,
        None,
        Icon::None,
        false,
        slot_text(r.height()),
    );
    let current = p.tracks[i].vca;
    let mut picked = None;
    let mut group_pick: Option<Option<usize>> = None;
    let mut new_group = false;
    let mut delete = None;
    let names: Vec<String> = p.groups.iter().map(|g| g.name.clone()).collect();
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(150.0);
        if ui
            .selectable_label(group.is_none(), tr("No Group"))
            .clicked()
        {
            group_pick = Some(None);
            ui.close();
        }
        for (k, name) in names.iter().enumerate() {
            if ui.selectable_label(group == Some(k), name).clicked() {
                group_pick = Some(Some(k));
                ui.close();
            }
        }
        if ui.button(tr("New Group")).clicked() {
            new_group = true;
            ui.close();
        }
        if let Some(k) = group {
            ui.menu_button(tr("Group Settings"), |ui| {
                let grp = &mut p.groups[k];
                ui.checkbox(&mut grp.volume, tr("Volume"));
                ui.checkbox(&mut grp.pan, tr("Pan"));
                ui.checkbox(&mut grp.mute, tr("Mute"));
                ui.checkbox(&mut grp.solo, tr("Solo"));
            });
            if ui.button(tr("Delete Group")).clicked() {
                delete = Some(k);
                ui.close();
            }
        }
        if !vcas.is_empty() {
            ui.separator();
            if ui
                .selectable_label(current.is_none(), tr("No VCA"))
                .clicked()
            {
                picked = Some(None);
                ui.close();
            }
            for (j, name) in &vcas {
                if ui.selectable_label(current == Some(*j), name).clicked() {
                    picked = Some(Some(*j));
                    ui.close();
                }
            }
        }
    });
    if let Some(vca) = picked {
        p.assign_vca(i, vca);
    }
    if let Some(pick) = group_pick {
        p.assign_group(i, pick);
    }
    if new_group {
        let n = p.groups.len() + 1;
        if let Some(k) = p.add_group(&format!("Group {n}")) {
            p.assign_group(i, Some(k));
        }
    }
    if let Some(k) = delete {
        p.delete_group(k);
    }
}

/// A bone key of the menu row with an LED at its left and a legend; latched while `on`.
fn bar_key(ui: &Ui, g: &Painter, r: Rect, id: egui::Id, label: &str, on: bool) -> bool {
    hw::key(
        ui,
        g,
        r,
        id,
        Cap::Bone,
        on,
        Some(on),
        hw::Legend::Text(label),
    )
    .0
    .clicked()
}

/// Width of a filter key for `label` (A.4.1: 8.6 a letter and 34 for the LED and margins).
fn filter_key_w(label: &str) -> f32 {
    label.chars().count() as f32 * 8.6 + 34.0
}

/// The power pictogram of the Sends on Faders key.
fn power_icon(g: &Painter, c: Pos2, s: f32) {
    let st = Stroke::new(2.0, theme::INK);
    let r = s * 0.36;
    let pts: Vec<Pos2> = (0..=24)
        .map(|k| polar(c, 35.0 + k as f32 * (290.0 / 24.0), r))
        .collect();
    g.add(Shape::line(pts, st));
    g.line_segment([c - vec2(0.0, s * 0.42), c - vec2(0.0, s * 0.04)], st);
}

/// The strip-width pictograms: a panel with its divider left (narrow) or right (wide).
fn layout_icon(g: &Painter, c: Pos2, wide: bool) {
    let st = Stroke::new(1.6, theme::INK);
    let r = Rect::from_center_size(c, vec2(15.0, 13.0));
    g.rect_stroke(r, 0.0, st, StrokeKind::Middle);
    let x = if wide { r.left() + 9.5 } else { r.left() + 5.5 };
    g.vline(x, r.y_range(), st);
}

fn toolbar(ui: &Ui, p: &mut Project, s: &mut UiState, bar: Rect) {
    const NS: &str = "mixer_toolbar";
    let g = ui.painter_at(bar);
    g.rect_filled(bar, 0.0, Color32::from_rgb(0xe7, 0xe3, 0xd9));
    g.hline(
        bar.x_range(),
        bar.top() + 0.5,
        Stroke::new(1.0, Color32::WHITE),
    );
    // A 3-point seam under the row.
    let seam_y = bar.bottom() - 3.0;
    g.rect_filled(
        Rect::from_min_max(pos2(bar.left(), seam_y), bar.max),
        0.0,
        Color32::from_rgb(0xd6, 0xd0, 0xc3),
    );
    hw::seam_h(&g, bar.x_range(), seam_y);
    let cy = bar.top() + (bar.height() - 3.0) / 2.0;
    let (can_undo, can_redo) = (s.can_undo, s.can_redo);
    let mut undo_pick = None;
    let v = &mut s.mixer;
    let key_r =
        |x: f32, w: f32| Rect::from_min_size(pos2(x, cy - BAR_KEY_H / 2.0), vec2(w, BAR_KEY_H));

    let mut x = bar.left() + 12.0;
    let (edit, w) = menu_title(ui, &g, x, cy, tr("Edit"), id(NS, "Edit"));
    x += w + 8.0;
    egui::Popup::menu(&edit).show(|ui| {
        ui.set_min_width(220.0);
        crate::widgets::undo_items(ui, can_undo, can_redo, &mut undo_pick);
        if ui.button(tr("Copy Channel Strip")).clicked() {
            if let Some(t) = p.tracks.get(p.selected_track) {
                v.copied = Some(CopiedStrip::of(t));
            }
            ui.close();
        }
        let can_paste = v.copied.is_some() && !p.tracks.is_empty();
        if ui
            .add_enabled(can_paste, egui::Button::new(tr("Paste Channel Strip")))
            .clicked()
        {
            if let (Some(c), Some(t)) = (v.copied.clone(), p.tracks.get_mut(p.selected_track)) {
                c.paste(t);
            }
            ui.close();
        }
    });
    if undo_pick.is_some() {
        s.undo_cmd = undo_pick;
    }
    let (options, w) = menu_title(ui, &g, x, cy, tr("Options"), id(NS, "Options"));
    x += w + 8.0;
    egui::Popup::menu(&options).show(|ui| {
        ui.set_min_width(220.0);
        if ui
            .button(tr("Create New Auxiliary Channel Strip"))
            .clicked()
        {
            p.add_track(TrackKind::Aux);
            ui.close();
        }
        if ui.button(tr("Create New VCA")).clicked() {
            p.add_track(TrackKind::Vca);
            ui.close();
        }
        if ui.button(tr("Create Group…")).clicked() {
            let n = p.groups.len() + 1;
            if let Some(k) = p.add_group(&format!("Group {n}")) {
                let sel = p.selected_track;
                p.assign_group(sel, Some(k));
            }
            ui.close();
        }
    });
    let (view, w) = menu_title(ui, &g, x, cy, tr("View"), id(NS, "View"));
    x += w + 24.0;
    egui::Popup::menu(&view).show(|ui| {
        ui.set_min_width(220.0);
        if ui
            .selectable_label(v.wide, tr("Wide Channel Strips"))
            .clicked()
        {
            v.wide = !v.wide;
            ui.close();
        }
    });

    let menus_end = x;
    // Right-hand side first (strip width keys, then filters), so the rest can be fitted into
    // what is left.
    let mut rx = bar.right() - 12.0;
    for (wide, key) in [(true, "wide"), (false, "narrow")] {
        let r = key_r(rx - 40.0, 40.0);
        let (resp, face) = hw::key(
            ui,
            &g,
            r,
            id(NS, key),
            Cap::Bone,
            v.wide == wide,
            None,
            hw::Legend::Custom,
        );
        layout_icon(&g, face.center(), wide);
        if resp.clicked() {
            v.wide = wide;
        }
        rx = r.left() - 8.0;
    }
    rx -= 14.0;
    let labels: Vec<String> = FILTERS.iter().map(|l| tr(l).to_uppercase()).collect();
    let filter_widths: Vec<f32> = labels.iter().map(|l| filter_key_w(l)).collect();
    let show_w = hw::silk_width(&g, "Show", 11.0) + 14.0;
    let filters_w = filter_widths.iter().map(|w| w + 6.0).sum::<f32>() + show_w;
    let fit = fit_toolbar(menus_end, bar.center().x, rx, filters_w);

    if fit.filters_inline {
        for (i, label) in labels.iter().enumerate().rev() {
            let w = filter_widths[i];
            let r = key_r(rx - w, w);
            if bar_key(ui, &g, r, id(NS, ("filter", i)), label, v.filters[i]) {
                v.filters[i] = !v.filters[i];
            }
            rx = r.left() - 6.0;
        }
        hw::silk(
            &g,
            pos2(rx - 8.0, cy),
            Align2::RIGHT_CENTER,
            "Show",
            11.0,
            theme::PRINT,
        );
    } else {
        // Not enough room for a key per strip type: one menu with a check for each.
        let r = Rect::from_min_size(
            pos2(rx - FILTER_MENU_W, cy - 13.0),
            vec2(FILTER_MENU_W, 26.0),
        );
        let resp = popup_button(ui, &g, r, id(NS, "filters_menu"), tr("Show"));
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(160.0);
            for (i, label) in FILTERS.iter().enumerate() {
                if ui.checkbox(&mut v.filters[i], tr(label)).clicked() {
                    ui.close();
                }
            }
        });
    }

    if fit.show_sends {
        draw_sends_on_faders(ui, &g, menus_end, cy, &mut v.sends_on_faders);
    }
    let left = fit.modes_left;

    // Single | Tracks
    let modes = [
        (MixerMode::Single, tr("Single")),
        (MixerMode::Tracks, tr("Tracks")),
    ];
    let key_w = (MODE_W - 6.0) / 2.0;
    for (i, (m, label)) in modes.iter().enumerate() {
        let r = key_r(left + i as f32 * (key_w + 6.0), key_w);
        if bar_key(ui, &g, r, id(NS, ("mode", *label)), label, v.mode == *m) {
            v.mode = *m;
        }
    }
}

/// Width of the Single | Tracks keys, of the "Sends on Faders" group and of the "Show" menu
/// that replaces the filter keys when they do not fit.
const MODE_W: f32 = 174.0;
const SENDS_W: f32 = 290.0;
const FILTER_MENU_W: f32 = 96.0;
const TOOLBAR_GAP: f32 = 16.0;

/// Where the Mixer toolbar's groups go so that none overlaps another: the mode switch at the
/// centre when there is room, else just left of the filters; the filters as keys when they
/// fit, else as one menu; "Sends on Faders" only in the room left over.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ToolbarFit {
    modes_left: f32,
    filters_inline: bool,
    show_sends: bool,
}

fn fit_toolbar(menus_end: f32, center: f32, right_end: f32, filters_w: f32) -> ToolbarFit {
    let place = |filters_left: f32| {
        let centred = center - MODE_W / 2.0 + 60.0;
        centred.min(filters_left - TOOLBAR_GAP - MODE_W)
    };
    let mut filters_inline = true;
    let mut modes_left = place(right_end - filters_w);
    if modes_left < menus_end + TOOLBAR_GAP {
        filters_inline = false;
        modes_left = place(right_end - FILTER_MENU_W);
    }
    // When even that is too tight, the modes stay just after the menus (the panel has a
    // minimum width, so this only happens in very narrow windows).
    modes_left = modes_left.max(menus_end + TOOLBAR_GAP);
    let show_sends = menus_end + SENDS_W + TOOLBAR_GAP <= modes_left;
    ToolbarFit {
        modes_left,
        filters_inline,
        show_sends,
    }
}

fn draw_sends_on_faders(ui: &Ui, g: &egui::Painter, x: f32, cy: f32, bus: &mut Option<u8>) {
    const NS: &str = "mixer_toolbar";
    // Sends on Faders: the faders set the sends to a bus. The power key turns it on (Bus 1)
    // and off; the field picks the bus.
    let label = tr("Sends on Faders:").trim_end_matches(':').to_string();
    let lr = hw::silk(
        g,
        pos2(x, cy),
        Align2::LEFT_CENTER,
        &label,
        11.0,
        theme::INK2,
    );
    let kr = Rect::from_min_size(
        pos2(lr.right() + 14.0, cy - BAR_KEY_H / 2.0),
        vec2(BAR_KEY_H, BAR_KEY_H),
    );
    let (resp, face) = hw::key(
        ui,
        g,
        kr,
        id(NS, "sends_on_faders_power"),
        Cap::Bone,
        bus.is_some(),
        None,
        hw::Legend::Custom,
    );
    power_icon(g, face.center(), 16.0);
    if resp.clicked() {
        *bus = if bus.is_some() { None } else { Some(1) };
    }
    let sof = Rect::from_min_size(pos2(kr.right() + 10.0, cy - 13.0), vec2(120.0, 26.0));
    let label = match bus {
        Some(b) => trf("Bus {}", &[b]),
        None => tr("Off").to_string(),
    };
    let resp = popup_button(ui, g, sof, id(NS, "sends_on_faders"), &label);
    egui::Popup::menu(&resp).show(|ui| {
        if ui.selectable_label(bus.is_none(), tr("Off")).clicked() {
            *bus = None;
            ui.close();
        }
        for b in 1..=8u8 {
            if ui
                .selectable_label(*bus == Some(b), trf("Bus {}", &[&b]))
                .clicked()
            {
                *bus = Some(b);
                ui.close();
            }
        }
    });
}

/// Whether a strip of `kind` is shown with the toolbar's filter buttons set as `filters`
/// (Audio, Inst, Aux, Output, VCA, MIDI).
fn strip_visible(kind: TrackKind, filters: &[bool; 6]) -> bool {
    match kind {
        TrackKind::Audio => filters[0],
        TrackKind::Instrument | TrackKind::Drummer => filters[1],
        TrackKind::Aux => filters[2],
        TrackKind::Bus | TrackKind::Output => filters[3],
        TrackKind::Vca => filters[4],
        TrackKind::ExternalMidi => filters[5],
        // A folder is only a way to group tracks; it has no channel strip.
        TrackKind::Folder => false,
    }
}

/// Where everything in the Mixer goes for a panel `area`.
struct Layout {
    bar: Rect,
    body: Rect,
    labels: Rect,
    strips: Rect,
    /// The channel strips (the Stereo Out is apart, see `master`).
    keys: Vec<StripKey>,
    /// Whether the Stereo Out strip shows.
    master: bool,
    rows: Rows,
    strip_top: f32,
    strip_h: f32,
    strip_w: f32,
    master_w: f32,
    /// The top of what shows of the strips: rows above it are hidden whole.
    clip_top: f32,
    /// How many rows are hidden at the top when the strips do not fit (the wheel shows them).
    max_hidden: usize,
}

/// The end cap's width: a narrow panel with the wordmark, right of the Stereo Out.
const END_CAP_W: f32 = 40.0;
/// Room between the last channel strip and the Stereo Out (the seam sits in its middle), and
/// between the Stereo Out and the end cap.
const MASTER_GAP: f32 = 24.0;
const CAP_GAP: f32 = 14.0;

/// Where the Stereo Out, the seam before it and the end cap go, and the channel strips' area.
struct Racks {
    /// The channel strips' rects (scrolled), and the area they are drawn in.
    strips: Vec<Rect>,
    chan_area: Rect,
    /// How far the channel strips scroll sideways.
    max_scroll: f32,
    master: Option<Rect>,
    seam: Option<f32>,
    end_cap: Rect,
}

impl Layout {
    fn new(area: Rect, p: &Project, v: &MixerView) -> Layout {
        let bar = Rect::from_min_size(area.min, vec2(area.width(), TOOLBAR_H));
        let body = Rect::from_min_max(pos2(area.left(), bar.bottom()), area.max);
        // Which strips, in order.
        let filters = v.filters;
        let keys: Vec<StripKey> = if p.tracks.is_empty() {
            Vec::new()
        } else {
            match v.mode {
                MixerMode::Single => {
                    vec![StripKey::Track(p.selected_track.min(p.tracks.len() - 1))]
                }
                MixerMode::Tracks => (0..p.tracks.len())
                    .filter(|i| strip_visible(p.tracks[*i].kind, &filters))
                    .map(StripKey::Track)
                    .collect(),
            }
        };
        let master = filters[3];
        let mut all = keys.clone();
        if master {
            all.push(StripKey::StereoOut);
        }
        let (fx, sends) = (fx_rows_for(p, &all), send_rows_for(p, &all));
        let d = Density::fit(body.height() - 16.0, f32::INFINITY, fx, sends);
        let (top_m, bottom_m) = if d == BIG {
            (16.0, 14.0)
        } else if d == MAIN {
            (10.0, 8.0)
        } else {
            (6.0, 6.0)
        };
        // A panel too short for the strips hides whole rows from the top (the slots first),
        // never part of one; the wheel brings them back a row at a time.
        let fit_h = body.height() - top_m - bottom_m;
        let r0 = rows(0.0, fx, sends, d);
        let tops = r0.row_tops();
        let need = r0.fader_top + bottom_parts(d) + MIN_FADER;
        let cut = |k: usize| tops[k] - tops[0];
        let mut max_hidden = 0;
        while need - cut(max_hidden) > fit_h && max_hidden + 1 < tops.len() {
            max_hidden += 1;
        }
        let back = (v.scroll_y / d.pitch).round().max(0.0) as usize;
        let hidden = max_hidden - back.min(max_hidden);
        let strip_h = fit_h + cut(max_hidden);
        let clip_top = if max_hidden > 0 {
            body.top() + top_m + d.top - 1.0
        } else {
            body.top()
        };
        let strip_top = body.top() + top_m - cut(hidden);
        let labels = Rect::from_min_max(body.min, pos2(body.left() + d.label_w, body.bottom()));
        let strips = Rect::from_min_max(pos2(labels.right(), body.top()), body.max);
        let strip_w = d.strip_w[usize::from(v.wide)];
        Layout {
            bar,
            body,
            labels,
            strips,
            keys,
            master,
            rows: rows(strip_top, fx, sends, d),
            strip_top,
            strip_h,
            strip_w,
            master_w: strip_w + 2.0 * d.pad,
            clip_top,
            max_hidden,
        }
    }

    /// Where the strips go, the channel strips scrolled sideways by `scroll`. The Stereo Out
    /// and the end cap never scroll: they follow the last channel strip while everything fits,
    /// and stay pinned at the right when the channel strips scroll under them.
    fn racks(&self, scroll: f32) -> Racks {
        let d = self.rows.d;
        let n = self.keys.len() as f32;
        let chans_w = if n > 0.0 {
            d.pad + n * self.strip_w + (n - 1.0) * d.gap
        } else {
            0.0
        };
        let master_room = if self.master {
            MASTER_GAP + self.master_w
        } else {
            0.0
        };
        let fixed = master_room + CAP_GAP + END_CAP_W;
        let room = (self.strips.width() - fixed).max(0.0);
        let max_scroll = (chans_w - room).max(0.0);
        let scroll = scroll.clamp(0.0, max_scroll);
        let chans_end = self.strips.left() + chans_w.min(room);
        let chan_area = Rect::from_min_max(
            pos2(self.strips.left(), self.body.top()),
            pos2(chans_end + d.gap.min(4.0), self.body.bottom()),
        );
        let strips = self
            .keys
            .iter()
            .enumerate()
            .map(|(k, _)| {
                Rect::from_min_size(
                    pos2(
                        self.strips.left() + d.pad + k as f32 * (self.strip_w + d.gap) - scroll,
                        self.strip_top,
                    ),
                    vec2(self.strip_w, self.strip_h),
                )
            })
            .collect();
        let (master, seam) = if self.master {
            let left = if n > 0.0 {
                chans_end + MASTER_GAP
            } else {
                self.strips.left() + d.pad
            };
            (
                Some(Rect::from_min_size(
                    pos2(left, self.strip_top),
                    vec2(self.master_w, self.strip_h),
                )),
                (n > 0.0).then_some(chans_end + MASTER_GAP / 2.0),
            )
        } else {
            (None, None)
        };
        let cap_left = master.map_or(chans_end, |m| m.right()) + CAP_GAP;
        let end_cap = Rect::from_min_max(
            pos2(cap_left, self.body.top()),
            pos2(cap_left + END_CAP_W, self.body.bottom()),
        );
        Racks {
            strips,
            chan_area,
            max_scroll,
            master,
            seam,
            end_cap,
        }
    }

    /// Every strip's rect, the Stereo Out's last (for the tests).
    #[cfg(test)]
    fn strip_rects(&self, scroll: f32) -> Vec<Rect> {
        let r = self.racks(scroll);
        r.strips.into_iter().chain(r.master).collect()
    }
}

/// The end cap right of the Stereo Out: a narrow faceplate on a seam, with screws and the
/// wordmark set on its side.
fn end_cap(g: &Painter, r: Rect) {
    hw::faceplate(g, r, 0.0, false);
    hw::seam_v(g, r.left(), r.y_range());
    let cx = r.center().x + 1.0;
    hw::screw(g, pos2(cx, r.top() + 16.0), 5.0, 40.0);
    hw::screw(g, pos2(cx, r.bottom() - 16.0), 5.0, -10.0);
    if r.height() < 260.0 {
        return;
    }
    let size = 14.0;
    let word = hw::galley(g, "PRO WAVE", Face::SilkX, size, theme::INK2);
    let model = hw::galley(g, "PW-1", Face::SilkX, size, theme::INK2);
    let sq = size * 0.7;
    let gap = size * 0.9;
    let len = word.size().x + gap + sq + gap + model.size().x;
    let gh = word.size().y;
    // Read bottom to top: text rotated a quarter turn back, starting at the bottom.
    let mut y = r.center().y + len / 2.0;
    let angle = -std::f32::consts::FRAC_PI_2;
    let wx = word.size().x;
    g.add(TextShape::new(pos2(cx - gh / 2.0, y), word, theme::INK2).with_angle(angle));
    y -= wx + gap;
    g.rect_filled(
        Rect::from_min_max(pos2(cx - sq / 2.0, y - sq), pos2(cx + sq / 2.0, y)),
        0.0,
        theme::ORANGE,
    );
    y -= sq + gap;
    g.add(TextShape::new(pos2(cx - gh / 2.0, y), model, theme::INK2).with_angle(angle));
}

/// A child of `ui` that draws and takes clicks only inside `clip`.
fn clipped(ui: &mut Ui, clip: Rect) -> Ui {
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(clip));
    c.set_clip_rect(clip.intersect(ui.clip_rect()));
    c
}

pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState, meters: &Meters) {
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, Sense::hover());
    // The wheel brings hidden rows back when the strips do not fit the panel's height, else
    // scrolls the channel strips sideways.
    let over = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pt| area.contains(pt) && pt.y > area.top() + TOOLBAR_H);
    let scroll = if over {
        ui.input(|i| i.smooth_scroll_delta)
    } else {
        Vec2::ZERO
    };
    let probe = Layout::new(area, p, &s.mixer);
    let (mut dx, mut dy) = (scroll.x, scroll.y);
    if probe.max_hidden > 0 {
        let span = probe.max_hidden as f32 * probe.rows.d.pitch;
        s.mixer.scroll_y = (s.mixer.scroll_y + dy).clamp(0.0, span);
        dy = 0.0;
    } else {
        s.mixer.scroll_y = 0.0;
    }
    dx += dy;
    let lay = Layout::new(area, p, &s.mixer);
    toolbar(ui, p, s, lay.bar);
    let v = &mut s.mixer;
    let d = lay.rows.d;

    let g = ui.painter_at(lay.body);
    vgrad(
        &g,
        lay.body,
        0.0,
        &[
            (0.0, Color32::from_rgb(0xd6, 0xd0, 0xc3)),
            (1.0, Color32::from_rgb(0xcf, 0xc9, 0xbb)),
        ],
    );

    // Scrolling: the wheel moves the channel strips sideways.
    let max_scroll = lay.racks(0.0).max_scroll;
    v.scroll_x = (v.scroll_x - dx).clamp(0.0, max_scroll);
    let racks = lay.racks(v.scroll_x);
    let r = &lay.rows;

    // Row labels: silkscreen in the label column, each with a short rule to its row. Rows
    // hidden at the top lose their labels with them.
    let gl = ui.painter_at(lay.labels);
    gl.rect_filled(lay.labels, 0.0, Color32::from_rgb(0xdd, 0xd8, 0xcb));
    hw::seam_v(&gl, lay.labels.right() - 2.0, lay.labels.y_range());
    let lx = lay.labels.right() - 22.0;
    let label_at = |y: f32, h: f32, t: &str| {
        if y < lay.clip_top {
            return;
        }
        let cy = y + h / 2.0;
        hw::silk(
            &gl,
            pos2(lx, cy),
            Align2::RIGHT_CENTER,
            t,
            11.0,
            theme::INK2,
        );
        gl.hline(
            lay.labels.right() - 14.0..=lay.labels.right() - 4.0,
            cy,
            Stroke::new(1.0, theme::INK2.gamma_multiply(0.5)),
        );
    };
    label_at(r.setting, d.sh, tr("Setting"));
    label_at(r.input, d.sh, tr("Input"));
    label_at(r.fx, d.sh, tr("Audio FX"));
    label_at(r.sends, d.sh, tr("Sends"));
    label_at(r.output, d.sh, tr("Output"));
    label_at(r.group, d.sh, tr("Group"));
    label_at(r.pan, d.pan_h, tr("Pan"));
    label_at(r.db, d.db_h, "dB");
    if let Some(first) = racks.strips.first().or(racks.master.as_ref()) {
        let geo = strip_geo(*first, r);
        label_at(geo.top - d.cap.y / 2.0, geo.len + d.cap.y, "Level");
        label_at(geo.ms.top(), d.ms_h, "Mute / Solo");
        label_at(geo.name.top(), d.name_h, "Channel");
    }

    let shown = Rect::from_min_max(pos2(lay.strips.left(), lay.clip_top), lay.strips.max);
    let gs = ui.painter_at(lay.strips);
    if let Some(x) = racks.seam {
        let y = lay.clip_top..=lay.strip_top + lay.strip_h + 4.0;
        gs.vline(x, y.clone(), Stroke::new(2.0, theme::SEAM));
        gs.vline(x + 1.5, y, Stroke::new(1.0, theme::BONE_HI));
    }
    if racks.end_cap.left() < lay.strips.right() {
        end_cap(&gs, racks.end_cap);
    }
    let chans = clipped(ui, racks.chan_area.intersect(shown));
    let gc = chans.painter().clone();
    for (key, strip) in lay.keys.iter().zip(racks.strips) {
        if strip.right() < racks.chan_area.left() || strip.left() > racks.chan_area.right() {
            continue;
        }
        channel_strip(
            &chans,
            &gc,
            strip,
            *key,
            p,
            v,
            meters,
            r,
            "mixer",
            NameStyle::Coloured,
        );
    }
    if let Some(m) = racks.master {
        let mu = clipped(ui, m.expand2(vec2(4.0, 0.0)).intersect(shown));
        let gm = mu.painter().clone();
        channel_strip(
            &mu,
            &gm,
            m,
            StripKey::StereoOut,
            p,
            v,
            meters,
            r,
            "mixer",
            NameStyle::Coloured,
        );
    }
    // Channel strips scrolled under the Stereo Out: a shadow along the edge they pass under.
    if racks.max_scroll > 0.0 {
        let x = racks.chan_area.right();
        let edge = Rect::from_min_max(pos2(x - 8.0, shown.top()), pos2(x, shown.bottom()));
        let mut mesh = Mesh::default();
        mesh.colored_vertex(edge.left_top(), Color32::TRANSPARENT);
        mesh.colored_vertex(edge.right_top(), Color32::from_black_alpha(46));
        mesh.colored_vertex(edge.right_bottom(), Color32::from_black_alpha(46));
        mesh.colored_vertex(edge.left_bottom(), Color32::TRANSPARENT);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        gs.add(Shape::mesh(mesh));
    }
    // Rows hidden at the top: the strips run under a seam with a shadow, a recessed edge.
    if lay.max_hidden > 0 {
        let x = lay.body.left()..=lay.body.right();
        let gb = ui.painter_at(lay.body);
        let band = Rect::from_min_max(
            pos2(lay.body.left(), lay.clip_top),
            pos2(lay.body.right(), lay.clip_top + 6.0),
        );
        let mut mesh = Mesh::default();
        mesh.colored_vertex(band.left_top(), Color32::from_black_alpha(60));
        mesh.colored_vertex(band.right_top(), Color32::from_black_alpha(60));
        mesh.colored_vertex(band.right_bottom(), Color32::TRANSPARENT);
        mesh.colored_vertex(band.left_bottom(), Color32::TRANSPARENT);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        gb.add(Shape::mesh(mesh));
        gb.rect_filled(
            Rect::from_min_max(lay.body.min, pos2(lay.body.right(), lay.clip_top)),
            0.0,
            Color32::from_rgb(0xd6, 0xd0, 0xc3),
        );
        hw::seam_h(&gb, x, lay.clip_top - 1.5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::PointerButton;
    use prowave_engine::FxKind;

    /// A project with an audio-style track and an Aux track, and its mixer drawn in a window
    /// 1400 by 700 points.
    fn mixer_project() -> Project {
        let mut p = Project::empty();
        p.add_track(TrackKind::Aux);
        p.selected_track = 0;
        p
    }

    /// Where the fader cap of the first strip sits at `db`, and the rows of the mixer.
    fn fader_at(p: &Project, db: f32) -> egui::Pos2 {
        let area = Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 700.0));
        let lay = Layout::new(area, p, &MixerView::default());
        let rects = lay.strip_rects(0.0);
        let geo = strip_geo(rects[0], &lay.rows);
        pos2(geo.fcx, geo.top + (1.0 - db_to_frac(db)) * geo.len)
    }

    fn run(h: &mut crate::ui_test::Harness, p: &mut Project, s: &mut UiState, n: usize) {
        for _ in 0..n {
            h.frame(vec![], |ui| show(ui, p, s, &Meters::default()));
        }
    }

    #[test]
    fn the_fader_moves_the_level_unless_the_strip_plays_its_automation() {
        let mut h = crate::ui_test::Harness::new(1400.0, 700.0);
        let mut s = UiState::new();
        let mut p = mixer_project();
        run(&mut h, &mut p, &mut s, 2);
        let cap = fader_at(&p, 0.0);
        let up = cap - vec2(0.0, 40.0);
        let frame = |h: &mut crate::ui_test::Harness, p: &mut Project, s: &mut UiState| {
            h.drag(
                &[cap, cap - vec2(0.0, 20.0), up],
                PointerButton::Primary,
                |ui| show(ui, p, s, &Meters::default()),
            )
        };
        frame(&mut h, &mut p, &mut s);
        assert!(p.tracks[0].volume_db > 0.5, "{}", p.tracks[0].volume_db);
    }

    #[test]
    fn sends_on_faders_set_the_send_to_the_bus_and_a_fader_at_the_bottom_takes_it_away() {
        let mut h = crate::ui_test::Harness::new(1400.0, 700.0);
        let mut s = UiState::new();
        let mut p = mixer_project();
        s.mixer.sends_on_faders = Some(2);
        run(&mut h, &mut p, &mut s, 2);
        let bottom = fader_at(&p, -60.0);
        let top = fader_at(&p, 0.0);
        h.drag(
            &[bottom, bottom - vec2(0.0, 20.0), top],
            PointerButton::Primary,
            |ui| show(ui, &mut p, &mut s, &Meters::default()),
        );
        let sends = &p.tracks[0].sends;
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].bus, Some(2));
        assert!(sends[0].level > 0.3, "{}", sends[0].level);
        // The track's own level was not touched.
        assert_eq!(p.tracks[0].volume_db, 0.0);
        // The conversions agree both ways, and the bottom of the scale removes the send.
        let mut list = vec![];
        set_send_level(&mut list, 3, 0.0);
        assert!((prowave_engine::send_gain(list[0].level) - 1.0).abs() < 0.01);
        set_send_level(&mut list, 3, -60.0);
        assert_eq!(list[0].level, 0.0);
        set_send_level(&mut list, 4, -60.0);
        assert_eq!(list.len(), 1, "nothing to add for a silent send");
    }

    #[test]
    fn a_fader_moves_its_whole_group_by_the_same_amount() {
        let mut h = crate::ui_test::Harness::new(1400.0, 700.0);
        let mut s = UiState::new();
        let mut p = mixer_project();
        // Track 0 (the instrument) and a new second track are one group; the aux is not.
        let second = p.add_track(TrackKind::Instrument);
        let g = p.add_group("Keys").unwrap_or(0);
        p.assign_group(0, Some(g));
        p.assign_group(second, Some(g));
        p.tracks[second].volume_db = -10.0;
        run(&mut h, &mut p, &mut s, 2);
        let cap = fader_at(&p, 0.0);
        let up = cap - vec2(0.0, 30.0);
        h.drag(
            &[cap, cap - vec2(0.0, 15.0), up],
            PointerButton::Primary,
            |ui| show(ui, &mut p, &mut s, &Meters::default()),
        );
        let moved = p.tracks[0].volume_db;
        assert!(moved > 0.5, "{moved}");
        assert!(
            (p.tracks[second].volume_db - (-10.0 + moved)).abs() < 0.01,
            "{} vs {}",
            p.tracks[second].volume_db,
            moved
        );
        assert_eq!(
            p.tracks[1].volume_db, 0.0,
            "the Aux strip is not in the group"
        );
        // With Volume off in the group the others stay.
        p.groups[g].volume = false;
        let cap = fader_at(&p, moved);
        let before = p.tracks[second].volume_db;
        h.drag(
            &[cap, cap + vec2(0.0, 10.0), cap + vec2(0.0, 20.0)],
            PointerButton::Primary,
            |ui| show(ui, &mut p, &mut s, &Meters::default()),
        );
        assert!(p.tracks[0].volume_db < moved);
        assert_eq!(p.tracks[second].volume_db, before);
    }

    #[test]
    fn the_slot_menus_move_bypass_and_remove_effects_an_older_project_has() {
        let mut p = mixer_project();
        let mut v = MixerView::default();
        p.tracks[0].inserts = [FxKind::ChannelEq, FxKind::Compressor, FxKind::Reverb]
            .map(Insert::new)
            .to_vec();
        v.fx_window = Some((0, 2));
        let kinds = |p: &Project| {
            p.tracks[0]
                .inserts
                .iter()
                .map(|x| x.kind)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(&p),
            [FxKind::ChannelEq, FxKind::Compressor, FxKind::Reverb]
        );
        apply_insert(&mut p, &mut v, 0, 2, InsertAct::Move(-1));
        assert_eq!(
            kinds(&p),
            [FxKind::ChannelEq, FxKind::Reverb, FxKind::Compressor]
        );
        assert_eq!(v.fx_window, Some((0, 1)), "the window follows its effect");
        apply_insert(&mut p, &mut v, 0, 0, InsertAct::Move(-1));
        apply_insert(&mut p, &mut v, 0, 2, InsertAct::Move(1));
        assert_eq!(kinds(&p).len(), 3, "moving past an end does nothing");
        apply_insert(&mut p, &mut v, 0, 1, InsertAct::Bypass);
        assert!(p.tracks[0].inserts[1].bypass);
        apply_insert(&mut p, &mut v, 0, 0, InsertAct::Remove);
        assert_eq!(
            v.fx_window,
            Some((0, 0)),
            "the window moved up with the others"
        );
        apply_insert(&mut p, &mut v, 0, 0, InsertAct::Remove);
        assert_eq!(v.fx_window, None, "its effect is gone");
        apply_insert(&mut p, &mut v, 0, 5, InsertAct::Remove);
        apply_insert(&mut p, &mut v, 9, 0, InsertAct::Open);
        assert_eq!(p.tracks[0].inserts.len(), 1);
    }

    #[test]
    fn the_stereo_out_strip_takes_the_audio_fx_menu_like_a_track() {
        let mut p = Project::empty();
        let mut v = MixerView::default();
        apply_insert(
            &mut p,
            &mut v,
            MASTER_STRIP,
            0,
            InsertAct::Add(FxKind::Reverb),
        );
        apply_insert(
            &mut p,
            &mut v,
            MASTER_STRIP,
            1,
            InsertAct::Add(FxKind::Compressor),
        );
        assert_eq!(p.master_inserts.len(), 2);
        assert!(p.tracks[0].inserts.is_empty(), "no track was touched");
        assert_eq!(
            v.fx_window,
            Some((MASTER_STRIP, 1)),
            "the new effect's window opens"
        );
        apply_insert(&mut p, &mut v, MASTER_STRIP, 0, InsertAct::Bypass);
        assert!(p.master_inserts[0].bypass);
        apply_insert(&mut p, &mut v, MASTER_STRIP, 0, InsertAct::Move(1));
        assert_eq!(p.master_inserts[1].kind, FxKind::Reverb);
        assert_eq!(
            v.fx_window,
            Some((MASTER_STRIP, 0)),
            "the window follows its effect"
        );
        apply_insert(&mut p, &mut v, MASTER_STRIP, 0, InsertAct::Remove);
        assert_eq!(p.master_inserts.len(), 1);
        assert_eq!(
            fx_rows_for(&p, &[StripKey::StereoOut]),
            2,
            "one effect and an empty slot"
        );
    }

    #[test]
    fn rows_make_room_for_every_send_and_the_strip_draws_with_all_of_it() {
        let mut p = mixer_project();
        p.tracks[0].sends = vec![
            SendSlot {
                bus: Some(1),
                level: 0.5,
                pre: true,
            },
            SendSlot {
                bus: Some(2),
                level: 0.7,
                pre: false,
            },
        ];
        p.tracks[0].inserts = vec![
            Insert::new(FxKind::ChannelEq),
            Insert::new(FxKind::Compressor),
        ];
        let keys = [StripKey::Track(0), StripKey::Track(1)];
        assert_eq!(send_rows_for(&p, &keys), 2);
        assert_eq!(send_rows_for(&Project::empty(), &[StripKey::Track(0)]), 1);
        let r = rows(0.0, 3, 3, MAIN);
        assert!(r.output - r.sends >= 3.0 * MAIN.pitch);
        let mut h = crate::ui_test::Harness::new(1400.0, 700.0);
        let mut s = UiState::new();
        let meters = Meters {
            tracks: vec![(0.5, 0.4), (0.0, 0.0)],
            reduction: vec![6.0, 0.0],
            fx_input: vec![],
            master_reduction: 0.0,
            master_fx_input: 0.0,
            master: (0.3, 0.3),
        };
        for _ in 0..3 {
            h.frame(vec![], |ui| show(ui, &mut p, &mut s, &meters));
        }
    }

    #[test]
    fn the_stereo_out_takes_plugins_like_a_track_and_their_lanes_follow_them() {
        use crate::plugins::PluginSlot;
        let mut p = mixer_project();
        let mut v = MixerView::default();
        p.master_inserts = vec![Insert::new(FxKind::Reverb)];
        let info = &prowave_engine::native_effects()[0];
        p.master_au_effects = vec![
            PluginSlot::new("aufx:dely:appl", "Apple: AUDelay"),
            crate::native_fx_ui::new_slot(info),
        ];
        let lane = AutoTarget::Plugin { slot: 2, param: 0 };
        p.master_auto.set_point(lane, 0.0, 0.5);
        // One built-in, two plug-ins and the empty slot.
        assert_eq!(fx_rows_for(&p, &[StripKey::StereoOut]), 4);
        // Move the native effect up: its envelope goes with it.
        apply_plugin(&mut p, &mut v, MASTER_STRIP, 1, InsertAct::Move(-1));
        assert_eq!(p.master_au_effects[0].id, info.id);
        let moved = AutoTarget::Plugin { slot: 1, param: 0 };
        assert_eq!(p.master_auto.lane(moved).len(), 1);
        assert!(p.master_auto.lane(lane).is_empty());
        apply_plugin(&mut p, &mut v, MASTER_STRIP, 1, InsertAct::Bypass);
        assert!(p.master_au_effects[1].bypass);
        apply_plugin(&mut p, &mut v, MASTER_STRIP, 1, InsertAct::Open);
        assert_eq!(
            v.open_plugin.as_ref().map(|o| o.0),
            Some(p.master_au_effects[1].uid)
        );
        apply_plugin(&mut p, &mut v, MASTER_STRIP, 0, InsertAct::Remove);
        assert_eq!(p.master_au_effects.len(), 1);
        assert!(
            p.master_auto.lane(moved).is_empty(),
            "its lane goes with it"
        );
        assert!(p.tracks.iter().all(|t| t.au_effects.is_empty()));
        // Out of range: nothing.
        apply_plugin(&mut p, &mut v, MASTER_STRIP, 5, InsertAct::Remove);
        assert_eq!(p.master_au_effects.len(), 1);
        // The strip draws with them (and the delay of the running one on its slot).
        v.master_latency = vec![0.002];
        let mut h = crate::ui_test::Harness::new(1400.0, 700.0);
        let mut s = UiState::new();
        s.mixer = v;
        p.master_au_effects[0].bypass = false;
        for _ in 0..3 {
            h.frame(vec![], |ui| show(ui, &mut p, &mut s, &Meters::default()));
        }
    }

    #[test]
    fn toolbar_groups_never_overlap_and_give_way_in_order() {
        // Wide panel: modes centred, every filter a button, Sends on Faders shown.
        let f = fit_toolbar(200.0, 1200.0, 2300.0, 520.0);
        assert!(f.filters_inline && f.show_sends);
        assert_eq!(f.modes_left, 1200.0 - MODE_W / 2.0 + 60.0);

        // Narrower: the modes move left of the filters, Sends on Faders goes first.
        let f = fit_toolbar(200.0, 700.0, 1040.0, 520.0);
        assert!(f.filters_inline && !f.show_sends);
        assert!(f.modes_left + MODE_W + TOOLBAR_GAP <= 1040.0 - 520.0);
        assert!(f.modes_left >= 200.0 + TOOLBAR_GAP);

        // Narrower still: the filters fold into one menu.
        let f = fit_toolbar(200.0, 500.0, 700.0, 520.0);
        assert!(!f.filters_inline);
        assert!(f.modes_left + MODE_W + TOOLBAR_GAP <= 700.0 - FILTER_MENU_W);
    }

    #[test]
    fn output_to_a_bus_makes_its_aux_strip_and_the_mixer_shows_it() {
        let mut p = Project::empty();
        p.tracks.clear();
        p.add_track(TrackKind::Audio);
        p.selected_track = 0;
        assert_eq!(bus_labels(&p)[0], "Bus 1");
        set_output(&mut p, 0, 1);
        let aux = p.aux_for_bus(1).unwrap();
        assert_eq!(p.tracks[0].output, 1);
        assert_eq!(p.tracks[aux].name, "Aux 1");
        assert_eq!(p.selected_track, 0);
        assert_eq!(bus_labels(&p)[0], "Bus 1 (Aux 1)", "the menu names the Aux");
        assert_eq!(bus_labels(&p)[1], "Bus 2");
        // The Aux strip is shown with the mixer's filters as they start.
        assert!(strip_visible(TrackKind::Aux, &MixerView::default().filters));
        // Back to Stereo Out: nothing more is made.
        set_output(&mut p, 0, 0);
        assert_eq!((p.tracks[0].output, p.tracks.len()), (0, 2));
    }

    #[test]
    fn strips_follow_the_filter_buttons_and_folders_have_none() {
        let all = [true; 6];
        let mut only_aux = [false; 6];
        only_aux[2] = true;
        assert!(strip_visible(TrackKind::Aux, &all));
        assert!(strip_visible(TrackKind::Aux, &only_aux));
        assert!(!strip_visible(TrackKind::Audio, &only_aux));
        assert!(!strip_visible(TrackKind::Instrument, &only_aux));
        assert!(!strip_visible(TrackKind::Aux, &[false; 6]));
        assert!(
            !strip_visible(TrackKind::Folder, &all),
            "a folder has no channel strip"
        );
    }

    /// Where the pictures go: `PROWAVE_SHOT_DIR`, else a folder in the temp directory.
    fn shot_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::var_os("PROWAVE_SHOT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-mixer"));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    /// The demo song as the Hardware mockup shows it: sends on the instruments, a send on
    /// Bus 1 and live meter levels.
    fn picture_song() -> (Project, Meters) {
        let mut p = Project::demo();
        p.selected_track = 0;
        for (t, bus) in [(0usize, 1u8), (1, 3), (2, 3), (3, 3)] {
            if let Some(tr) = p.tracks.get_mut(t) {
                tr.sends = vec![SendSlot {
                    bus: Some(bus),
                    level: 0.55,
                    pre: false,
                }];
            }
        }
        if let Some(t) = p.tracks.get_mut(2) {
            t.pan = 0.35;
        }
        if let Some(t) = p.tracks.get_mut(1) {
            t.pan = -0.3;
        }
        let db = |x: f32| 10f32.powf(x / 20.0);
        let levels = [
            (-6.6, -7.9),
            (-8.4, -7.2),
            (-10.5, -11.8),
            (-2.8, -3.9),
            (-9.6, -9.9),
            (-8.1, -10.2),
            (-14.0, -15.0),
            (-12.0, -13.0),
        ];
        let meters = Meters {
            tracks: (0..p.tracks.len())
                .map(|i| {
                    let (l, r) = levels[i % levels.len()];
                    (db(l), db(r))
                })
                .collect(),
            reduction: vec![0.0; p.tracks.len()],
            fx_input: vec![0.0; p.tracks.len()],
            master_reduction: 0.0,
            master_fx_input: 0.0,
            master: (db(-2.4), db(-3.0)),
        };
        (p, meters)
    }

    fn picture(name: &str, w: f32, h: f32, wide: bool) {
        let (mut p, meters) = picture_song();
        let mut s = UiState::new();
        s.mixer.wide = wide;
        crate::ui_test::render_png(
            w,
            h,
            2.0,
            theme::BONE,
            |ui| {
                theme::apply(ui.ctx());
                show(ui, &mut p, &mut s, &meters)
            },
            &shot_path(name),
        )
        .unwrap();
    }

    /// Pictures of the Mixer at its three densities (`PROWAVE_SHOT_DIR=… cargo test -p
    /// prowave-app mixer_pictures -- --ignored`).
    #[test]
    #[ignore = "pictures"]
    fn mixer_pictures() {
        picture("mixer-big.png", 2000.0, 1060.0, true);
        picture("mixer-main.png", 1760.0, 700.0, true);
        picture("mixer-main-narrow.png", 1400.0, 700.0, false);
        picture("mixer-compact.png", 1400.0, 470.0, false);
        picture("mixer-compact-wide.png", 1400.0, 420.0, true);
    }
}
