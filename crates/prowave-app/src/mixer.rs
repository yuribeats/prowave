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
use crate::i18n::{tr, trf};
use crate::model::{AutoTarget, Insert, Project, SendSlot, TrackKind, MAX_SENDS};
use crate::widgets::{menu_title, popup_button, stroke, BLUE, TEXT, TEXT_DIM};
use crate::UiState;
use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Rect, Sense, Stroke, StrokeKind, Ui};
use std::collections::HashMap;

const TOOLBAR_H: f32 = 30.0;
const LABEL_W: f32 = 88.0;
const GAP: f32 = 2.0;

const TOOLBAR_BG: Color32 = Color32::from_rgb(0x3c, 0x3c, 0x3c);
const MIXER_BG: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
const STRIP_BG: Color32 = Color32::from_rgb(0x3e, 0x3e, 0x3e);
const STRIP_SEL: Color32 = Color32::from_rgb(0x4d, 0x4d, 0x4d);
const SLOT_EMPTY: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x2c);
const SLOT_GRAY: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
const PLUGIN_BLUE: Color32 = Color32::from_rgb(0x36, 0x6f, 0xd2);
/// A strip that makes no sound (muted, or another track is soloed): instruments, plug-ins
/// and sends turn grey, and so do the meters.
const PLUGIN_OFF: Color32 = Color32::from_rgb(0x4a, 0x4a, 0x4a);
const METER_OFF: Color32 = Color32::from_rgb(0x70, 0x70, 0x70);
const READ_GREEN: Color32 = Color32::from_rgb(0x6c, 0xd0, 0x5a);
const MUTE_ON: Color32 = Color32::from_rgb(0x3b, 0x8c, 0xe6);
const SOLO_ON: Color32 = Color32::from_rgb(0xe6, 0xc2, 0x2e);
const OUT_PURPLE: Color32 = Color32::from_rgb(0x8a, 0x3f, 0xc8);

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

/// Shortens `text` with a trailing "…" so it fits in `max_w` (Logic truncates strip names).
pub fn elide(g: &Painter, text: &str, font: &FontId, max_w: f32) -> String {
    let width = |t: &str| {
        g.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
    if width(text) <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if width(&candidate) <= max_w {
            return candidate;
        }
    }
    String::new()
}

fn slot(g: &Painter, r: Rect, text: &str, fill: Color32, color: Color32) {
    g.rect_filled(r, 3.0, fill);
    if !text.is_empty() {
        let font = FontId::proportional(10.5);
        let shown = elide(g, text, &font, r.width() - 6.0);
        g.text(r.center(), Align2::CENTER_CENTER, shown, font, color);
    }
}

fn small_button(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    id: egui::Id,
    label: &str,
    on: bool,
    on_col: Color32,
) -> bool {
    let resp = ui.interact(r, id, Sense::click());
    g.rect_filled(r, 3.0, if on { on_col } else { SLOT_GRAY });
    let txt = if on && on_col == SOLO_ON {
        Color32::BLACK
    } else {
        Color32::from_gray(0xe8)
    };
    g.text(
        r.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(10.5),
        txt,
    );
    resp.clicked()
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
    c: egui::Pos2,
    id: egui::Id,
    value: f32,
    editable: bool,
) -> Moved {
    let r = Rect::from_center_size(c, vec2(26.0, 26.0));
    let mut out = Moved::default();
    if editable {
        let resp = ui.interact(r, id, Sense::click_and_drag());
        if resp.dragged() {
            out.value = Some((value - resp.drag_delta().y * 0.01).clamp(-1.0, 1.0));
        }
        if resp.double_clicked() {
            out.value = Some(0.0);
            out.started = true;
            out.stopped = true;
        }
        out.started |= resp.drag_started();
        out.stopped |= resp.drag_stopped();
    }
    crate::widgets::draw_knob(g, c, 12.0, value);
    out
}

/// Logic-style meter colour for a level in dB: green, yellow from -6 dB, red above 0 dB.
pub(crate) fn meter_colour(db: f32) -> Color32 {
    if db > 0.0 {
        Color32::from_rgb(0xe8, 0x3a, 0x2c)
    } else if db > -6.0 {
        Color32::from_rgb(0xe6, 0xc8, 0x2a)
    } else {
        Color32::from_rgb(0x34, 0xc2, 0x3e)
    }
}

/// Fader (scale + slot + cap) and stereo meter with peak-hold lines, laid out like Logic's
/// channel strip: fader scale on the left, the cap, the meter scale numbers, then L/R bars.
/// Returns the new fader value in dB when it was dragged or double-clicked.
#[allow(clippy::too_many_arguments)]
fn fader_and_meter(
    ui: &Ui,
    g: &Painter,
    area: Rect,
    id: egui::Id,
    db: f32,
    editable: bool,
    peaks: (f32, f32),
    inactive: bool,
) -> Moved {
    let top = area.top() + 8.0;
    let bottom = area.bottom() - 6.0;
    let h = (bottom - top).max(1.0);
    let y_of = |f: f32| bottom - f * h;
    let w = area.width();
    let scale_col = Color32::from_gray(0x8e);
    let tiny = FontId::proportional(6.5);

    // Columns, proportional to the strip width (Logic: fader left of centre, meter right).
    let fader_x = area.left() + (w * 0.30).max(22.0);
    let num_x = area.left() + w * 0.56;
    let bar_w = 5.0;
    let meter_left = (area.left() + w * 0.66).min(area.right() - 2.0 * bar_w - 6.0);

    // Fader scale: tick marks with small numbers, left of the slot.
    for mark in SCALE {
        let y = y_of(db_to_frac(mark));
        let tick_len = if mark == 0.0 { 6.0 } else { 4.0 };
        let tx = fader_x - 13.0;
        g.line_segment(
            [pos2(tx - tick_len, y), pos2(tx, y)],
            Stroke::new(1.0, scale_col),
        );
        if w >= 70.0 {
            g.text(
                pos2(tx - tick_len - 2.0, y),
                Align2::RIGHT_CENTER,
                format!("{}", mark.abs() as i32),
                tiny.clone(),
                scale_col,
            );
        }
        // Meter scale numbers between the fader and the meter.
        g.text(
            pos2(num_x, y),
            Align2::CENTER_CENTER,
            format!("{}", mark.abs() as i32),
            tiny.clone(),
            scale_col,
        );
    }

    // Fader slot.
    let slot_r = Rect::from_min_max(pos2(fader_x - 2.0, top), pos2(fader_x + 2.0, bottom));
    g.rect_filled(slot_r, 2.0, Color32::from_gray(0x14));
    g.line_segment(
        [slot_r.right_top(), slot_r.right_bottom()],
        Stroke::new(1.0, Color32::from_gray(0x50)),
    );

    // Meter bars with peak-hold lines (hold falls back at 12 dB/s).
    let hold_id = id.with("hold");
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let mut hold: (f32, f32) = ui
        .ctx()
        .data(|d| d.get_temp(hold_id).unwrap_or((-120.0, -120.0)));
    let levels = (lin_to_db(peaks.0), lin_to_db(peaks.1));
    hold.0 = levels.0.max(hold.0 - 12.0 * dt);
    hold.1 = levels.1.max(hold.1 - 12.0 * dt);
    ui.ctx().data_mut(|d| d.insert_temp(hold_id, hold));
    // One dark housing holds both bars (Logic draws L and R side by side in one well).
    let housing = Rect::from_min_max(
        pos2(meter_left - 2.0, top - 1.0),
        pos2(meter_left + 2.0 * bar_w + 3.0, bottom + 1.0),
    );
    g.rect_filled(housing, 2.5, Color32::from_gray(0x10));
    g.rect_stroke(
        housing,
        2.5,
        Stroke::new(1.0, Color32::from_gray(0x2e)),
        StrokeKind::Inside,
    );
    for (i, (lvl, hd)) in [(levels.0, hold.0), (levels.1, hold.1)]
        .into_iter()
        .enumerate()
    {
        let x0 = meter_left + i as f32 * (bar_w + 1.0);
        let bar = Rect::from_min_max(pos2(x0, top), pos2(x0 + bar_w, bottom));
        g.rect_filled(bar, 0.5, Color32::from_gray(0x08));
        let f = db_to_frac(lvl);
        if f > 0.0 {
            let bands = [(-120.0f32, -6.0f32), (-6.0, 0.0), (0.0, 12.0)];
            for (lo, hi) in bands {
                let (fa, fb) = (db_to_frac(lo), db_to_frac(hi).min(f));
                if fb > fa {
                    let seg =
                        Rect::from_min_max(pos2(bar.left(), y_of(fb)), pos2(bar.right(), y_of(fa)));
                    let col = if inactive {
                        METER_OFF
                    } else {
                        meter_colour((lo + hi) * 0.5)
                    };
                    g.rect_filled(seg, 0.0, col);
                    // Lit left edge, like the LED column in Logic.
                    g.rect_filled(
                        Rect::from_min_max(seg.min, pos2(seg.left() + bar_w * 0.4, seg.bottom())),
                        0.0,
                        col.lerp_to_gamma(Color32::WHITE, 0.28),
                    );
                }
            }
            // Thin dark gaps every few pixels make the bar look segmented.
            let mut y = bottom - 2.0;
            while y > y_of(f) {
                g.line_segment(
                    [pos2(bar.left(), y), pos2(bar.right(), y)],
                    Stroke::new(0.8, Color32::from_black_alpha(110)),
                );
                y -= 2.5;
            }
        }
        if hd > -60.0 {
            let y = y_of(db_to_frac(hd));
            g.line_segment(
                [pos2(bar.left(), y), pos2(bar.right(), y)],
                Stroke::new(
                    1.5,
                    if inactive {
                        METER_OFF
                    } else {
                        meter_colour(hd)
                    }
                    .lerp_to_gamma(Color32::WHITE, 0.4),
                ),
            );
        }
    }
    if levels.0 > -60.0 || levels.1 > -60.0 || hold.0 > -60.0 || hold.1 > -60.0 {
        ui.ctx().request_repaint();
    }

    // Fader cap: light grey block with a dark centre groove and fine ridges.
    let cap_y = y_of(db_to_frac(db));
    let cap = Rect::from_center_size(pos2(fader_x, cap_y), vec2(16.0, 27.0));
    let mut out = Moved::default();
    if editable {
        let hit = Rect::from_min_max(
            pos2(fader_x - 14.0, top - 17.0),
            pos2(fader_x + 14.0, bottom + 17.0),
        );
        let resp = ui.interact(hit, id, Sense::click_and_drag());
        if resp.dragged() {
            let f = db_to_frac(db) - resp.drag_delta().y / h;
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
    // Logic's cap: a narrow metal block. Soft shadow, silver body that is a little darker at the
    // edges, a dark groove across the middle with a bright line under it, and engraved ridges
    // (a dark line with a light line under it) above and below.
    g.rect_filled(
        cap.translate(vec2(0.5, 1.5)),
        2.0,
        Color32::from_black_alpha(110),
    );
    g.rect_filled(cap, 2.0, Color32::from_gray(0x9c));
    g.rect_filled(
        Rect::from_min_max(
            pos2(cap.left() + 1.5, cap.top() + 1.0),
            pos2(cap.right() - 1.5, cap.center().y),
        ),
        egui::CornerRadius {
            nw: 1,
            ne: 1,
            sw: 0,
            se: 0,
        },
        Color32::from_gray(0xd4),
    );
    g.rect_filled(
        Rect::from_min_max(
            pos2(cap.left() + 1.5, cap.center().y),
            pos2(cap.right() - 1.5, cap.bottom() - 1.0),
        ),
        egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: 1,
            se: 1,
        },
        Color32::from_gray(0xb2),
    );
    g.rect_stroke(
        cap,
        2.0,
        Stroke::new(1.0, Color32::from_gray(0x44)),
        StrokeKind::Inside,
    );
    for dy in [-9.0f32, -6.0, -3.0, 4.5, 7.5, 10.5] {
        g.line_segment(
            [
                pos2(cap.left() + 2.5, cap_y + dy),
                pos2(cap.right() - 2.5, cap_y + dy),
            ],
            Stroke::new(0.8, Color32::from_gray(0x80)),
        );
        g.line_segment(
            [
                pos2(cap.left() + 2.5, cap_y + dy + 1.0),
                pos2(cap.right() - 2.5, cap_y + dy + 1.0),
            ],
            Stroke::new(0.6, Color32::from_gray(0xe4)),
        );
    }
    g.line_segment(
        [
            pos2(cap.left() + 1.0, cap_y),
            pos2(cap.right() - 1.0, cap_y),
        ],
        Stroke::new(1.8, Color32::from_gray(0x24)),
    );
    g.line_segment(
        [
            pos2(cap.left() + 1.0, cap_y + 1.4),
            pos2(cap.right() - 1.0, cap_y + 1.4),
        ],
        Stroke::new(0.8, Color32::from_gray(0xee)),
    );
    out
}

/// Vertical positions of the strip rows (shared by every strip so rows line up).
pub struct Rows {
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

pub fn rows(top: f32, fx_rows: usize, send_rows: usize) -> Rows {
    let fx_rows = fx_rows.max(2);
    let send_rows = send_rows.clamp(1, MAX_SENDS);
    let setting = top + 6.0;
    let input = setting + 22.0;
    let fx = input + 22.0;
    let sends = fx + fx_rows as f32 * 17.0 + 4.0;
    let output = sends + send_rows as f32 * 17.0 + 3.0;
    let group = output + 20.0;
    let pan = group + 22.0;
    let db = pan + 30.0;
    Rows {
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
        fader_top: db + 18.0,
    }
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

/// Send slots needed so every strip in `keys` fits (its sends + one empty slot).
pub fn send_rows_for(p: &Project, keys: &[StripKey]) -> usize {
    keys.iter()
        .map(|k| match k {
            StripKey::Track(i) => p.tracks.get(*i).map_or(0, |t| t.sends.len()),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// The strip's send slots, one per row: the sends it has, then one empty slot to add another.
/// `x`, `w` and `top` place the rows (see [`Rows`]). Picking a bus no Aux strip plays makes
/// that Aux strip, as Logic does, so the send is heard.
#[allow(clippy::too_many_arguments)]
fn send_slots(
    ui: &Ui,
    g: &Painter,
    area: Rect,
    top: f32,
    rows: usize,
    p: &mut Project,
    i: usize,
    uid: egui::Id,
    inactive: bool,
) {
    let labels = bus_labels(p);
    let sends = &mut p.tracks[i].sends;
    let (mut remove, mut picked) = (None, None);
    for k in 0..rows {
        let r = Rect::from_min_size(
            pos2(area.left(), top + k as f32 * 17.0),
            vec2(area.width(), 16.0),
        );
        if k < sends.len() {
            match send_slot(ui, g, r, &mut sends[k], &labels, uid.with(k), inactive) {
                SendPick::Remove => remove = Some(k),
                SendPick::Bus(b) => picked = Some(b),
                SendPick::Keep => {}
            }
        } else if k == sends.len() && sends.len() < MAX_SENDS {
            let resp = ui.interact(r, uid.with(("empty", k)), Sense::click());
            slot(g, r, "", SLOT_EMPTY, TEXT);
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
            if let Some(bus) = new {
                sends.push(SendSlot {
                    bus: Some(bus),
                    level: 0.5,
                    pre: false,
                });
                picked = Some(bus);
            }
        }
    }
    // An automated send's LED (red; grey when overridden by hand).
    let t = &p.tracks[i];
    for k in 0..t.sends.len().min(rows) {
        let y = top + k as f32 * 17.0 + 8.0;
        crate::automation_ui::led(
            g,
            pos2(area.right() - 4.0, y),
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
}

/// Logic's send slot: a blue "Bus N" button with a round level knob beside it. Click the slot
/// to pick a bus; the eye marks a pre-fader send. The engine copies the strip's signal to the
/// bus, where the Aux strip that listens to it plays it. `labels` names Bus 1 to Bus 8 in the
/// menu (see [`bus_labels`]).
fn send_slot(
    ui: &Ui,
    g: &Painter,
    r: Rect,
    send: &mut crate::model::SendSlot,
    labels: &[String],
    uid: egui::Id,
    inactive: bool,
) -> SendPick {
    let Some(bus) = send.bus else {
        return SendPick::Remove;
    };
    let mut pick = SendPick::Keep;
    let knob_c = pos2(r.right() - 8.0, r.center().y);
    let slot_r = Rect::from_min_max(r.min, pos2(r.right() - 19.0, r.bottom()));
    let resp = ui.interact(slot_r, uid, Sense::click());
    slot(
        g,
        slot_r,
        &trf("Bus {}", &[&bus]),
        if inactive { PLUGIN_OFF } else { PLUGIN_BLUE },
        if inactive { TEXT_DIM } else { Color32::WHITE },
    );
    if send.pre {
        // The eye icon Logic shows on pre-fader sends.
        let c = pos2(slot_r.left() + 9.0, slot_r.center().y);
        g.circle_stroke(c, 4.5, Stroke::new(1.0, Color32::WHITE));
        g.circle_filled(c, 1.6, Color32::WHITE);
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
    });
    // Level knob: drag vertically; the indicator sweeps like a pan knob.
    let kresp = ui.interact(
        Rect::from_center_size(knob_c, vec2(16.0, 16.0)),
        uid.with("level"),
        Sense::click_and_drag(),
    );
    if kresp.dragged() {
        send.level = (send.level - kresp.drag_delta().y * 0.01).clamp(0.0, 1.0);
    }
    if kresp.double_clicked() {
        send.level = 0.0;
    }
    g.circle_filled(knob_c, 6.5, Color32::from_gray(0x5e));
    g.circle_stroke(knob_c, 6.5, Stroke::new(1.0, Color32::from_gray(0x26)));
    let a = -std::f32::consts::FRAC_PI_2 - 2.3 + send.level * 4.6;
    g.line_segment(
        [
            knob_c + vec2(a.cos(), a.sin()) * 2.0,
            knob_c + vec2(a.cos(), a.sin()) * 5.5,
        ],
        Stroke::new(1.4, Color32::WHITE),
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
    let off = inactive || fx.bypass;
    slot(
        g,
        r,
        fx.name(),
        if off { PLUGIN_OFF } else { PLUGIN_BLUE },
        if off { TEXT_DIM } else { Color32::WHITE },
    );
    let resp = ui.interact(r, uid, Sense::click());
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
    font: &FontId,
    on: (Color32, Color32),
    uid: egui::Id,
) {
    let t = &p.tracks[i];
    let loaded = t.au_instrument.clone();
    match (&loaded, &t.soundfont) {
        (Some(au), _) => {
            let fitted = elide(g, &au.name, font, r.width() - 8.0);
            slot(g, r, &fitted, PLUGIN_BLUE, Color32::WHITE);
        }
        (None, Some(sf)) => {
            let label = crate::inspector::soundfont_label(sf);
            let fitted = elide(g, &label, font, r.width() - 8.0);
            slot(g, r, &fitted, on.0, on.1);
        }
        (None, None) => slot(g, r, tr("Instrument"), SLOT_EMPTY, TEXT_DIM),
    }
    let mut resp = ui.interact(r, uid, Sense::click());
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
        if ui.button(tr("Sampler")).clicked() {
            built_in = Some(crate::sampler::new_slot);
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
    slot(g, r, "", SLOT_EMPTY, TEXT);
    let resp = ui.interact(r, uid, Sense::click());
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
    let font = FontId::proportional(10.5);
    let fitted = elide(g, &au.name, &font, r.width() - 8.0);
    slot(
        g,
        r,
        &fitted,
        if off { PLUGIN_OFF } else { PLUGIN_BLUE },
        if off { TEXT_DIM } else { Color32::WHITE },
    );
    let mut tip = plugin_tooltip(&au);
    if let Some(s) = delay {
        tip.push('\n');
        tip.push_str(&trf("Latency: {} ms", &[&format!("{:.1}", s * 1000.0)]));
    }
    let resp = ui.interact(r, uid, Sense::click()).on_hover_text(tip);
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

/// How the strip's name is shown at the bottom.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NameStyle {
    /// Coloured label (Mixer).
    Coloured,
    /// Plain text (Inspector).
    Plain,
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
    let selected = matches!(key, StripKey::Track(i) if i == p.selected_track);
    let inactive = matches!(key, StripKey::Track(i)
        if p.is_muted(i) || crate::tracks_area::silenced_by_solo(p, i));
    let (plug, plug_text) = if inactive {
        (PLUGIN_OFF, TEXT_DIM)
    } else {
        (PLUGIN_BLUE, Color32::WHITE)
    };
    // Whole-strip click selects (registered first so the controls on top win).
    if let StripKey::Track(i) = key {
        if ui
            .interact(strip, id(ns, (key, "strip")), Sense::click())
            .clicked()
        {
            p.selected_track = i;
        }
    }
    let bg = if selected && name_style == NameStyle::Coloured {
        STRIP_SEL
    } else {
        STRIP_BG
    };
    g.rect_filled(strip, 2.0, bg);
    let inner_w = strip.width() - 8.0;
    let sx = strip.left() + 4.0;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(sx, y), vec2(inner_w, h));

    let (name, color, kind) = match key {
        StripKey::Track(i) => {
            let t = &p.tracks[i];
            (t.name.clone(), t.color, Some(t.kind))
        }
        StripKey::StereoOut => (tr("Stereo Out").to_string(), OUT_PURPLE, None),
    };
    let beat = p.position_beats;

    // A VCA strip is a fader with Mute and Solo: none of the channel strip's slots.
    let vca_strip = kind == Some(TrackKind::Vca);
    if vca_strip {
        g.text(
            pos2(strip.center().x, r.setting + 8.0),
            Align2::CENTER_CENTER,
            tr("VCA"),
            FontId::proportional(12.0),
            TEXT_DIM,
        );
    }
    if !vca_strip {
        if let StripKey::Track(i) = key {
            slot(g, row(r.setting, 16.0), tr("Setting"), SLOT_GRAY, TEXT);
            setting_menu(ui, row(r.setting, 16.0), p, v, i, id(ns, (key, "setting")));
        }
        let input_r = row(r.input, 16.0);
        match kind {
            Some(TrackKind::Instrument | TrackKind::Drummer) => {
                if let StripKey::Track(i) = key {
                    let font = FontId::proportional(10.5);
                    instrument_slot(
                        ui,
                        g,
                        input_r,
                        p,
                        v,
                        i,
                        &font,
                        (plug, plug_text),
                        id(ns, (key, "instrument")),
                    );
                }
            }
            // An aux strip's input is the bus it listens to: Aux 1 takes Bus 1, and so on.
            Some(TrackKind::Aux) => {
                let n = match key {
                    StripKey::Track(i) => p.aux_bus_of(i),
                    _ => 1,
                };
                let text = if n == 0 {
                    String::new()
                } else {
                    trf("Bus {}", &[&n])
                };
                slot(g, input_r, &text, plug, plug_text);
            }
            // An external MIDI strip shows where its notes go: the port and the channel.
            Some(TrackKind::ExternalMidi) => {
                let text = match key {
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
                let fitted = elide(g, &text, &FontId::proportional(10.5), input_r.width() - 8.0);
                slot(g, input_r, &fitted, plug, plug_text);
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
        let strip = match key {
            StripKey::Track(i) => i,
            StripKey::StereoOut => MASTER_STRIP,
        };
        for k in 0..r.fx_rows {
            let rr = row(r.fx + k as f32 * 17.0, 15.0);
            insert_slot(ui, g, rr, p, v, strip, k, id(ns, (key, "fx", k)), inactive);
        }
        match key {
            // An Aux strip takes its signal from a bus: it has no sends of its own.
            StripKey::Track(i) if p.tracks[i].kind != TrackKind::Aux => {
                let area = row(r.sends, 16.0);
                send_slots(
                    ui,
                    g,
                    area,
                    r.sends,
                    r.send_rows,
                    p,
                    i,
                    id(ns, (key, "send")),
                    inactive,
                );
            }
            _ => {}
        }
        if let StripKey::Track(_) = key {
            let out = if name_style == NameStyle::Plain {
                tr("Stereo Out")
            } else {
                tr("St Out")
            };
            if let StripKey::Track(i) = key {
                output_slot(
                    ui,
                    g,
                    row(r.output, 16.0),
                    p,
                    i,
                    out,
                    id(ns, (key, "output")),
                );
                group_slot(ui, g, row(r.group, 16.0), p, i, id(ns, (key, "vca")));
            }
        }
    }
    let pan_c = pos2(strip.center().x, r.pan + 12.0);
    match key {
        _ if vca_strip => {}
        StripKey::Track(i) => {
            let t = &p.tracks[i];
            let shown = crate::automation::shown_value(t, AutoTarget::Pan, beat);
            let editable = crate::automation::editable(t, AutoTarget::Pan);
            let m = pan_knob(ui, g, pan_c, id(ns, (key, "pan")), shown, editable);
            crate::automation_ui::led(g, pan_c + vec2(13.0, -9.0), &t.auto, AutoTarget::Pan);
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
    let half = (inner_w - 4.0) / 2.0;
    let vol_r = Rect::from_min_size(pos2(sx, r.db), vec2(half, 14.0));
    let pk_r = Rect::from_min_size(pos2(sx + half + 4.0, r.db), vec2(half, 14.0));
    slot(
        g,
        vol_r,
        &format!("{vol_db:.1}"),
        Color32::from_gray(0x24),
        TEXT,
    );
    // The fader's LED: red while an envelope moves it, grey when it is overridden.
    let fader_auto = match key {
        StripKey::Track(i) => Some(&p.tracks[i].auto),
        StripKey::StereoOut => Some(&p.master_auto),
    };
    if let (Some(a), None) = (fader_auto, send_bus) {
        crate::automation_ui::led(
            g,
            pos2(vol_r.left() + 4.0, vol_r.center().y),
            a,
            AutoTarget::Volume,
        );
    }
    let hold_db = lin_to_db(*hold);
    let pk_text = if *hold <= 1e-5 {
        String::new()
    } else {
        format!("{hold_db:.1}")
    };
    let pk_col = if hold_db > 0.0 {
        Color32::from_rgb(0xff, 0x5a, 0x4a)
    } else {
        READ_GREEN
    };
    slot(g, pk_r, &pk_text, Color32::from_gray(0x24), pk_col);
    if ui
        .interact(pk_r, id(ns, (key, "peak")), Sense::click())
        .clicked()
    {
        *hold = 0.0;
    }

    // Fader + scale + meter.
    let name_h = 22.0;
    let fader_area = Rect::from_min_max(
        pos2(sx, r.fader_top),
        pos2(strip.right() - 4.0, strip.bottom() - 26.0 - name_h),
    );
    let fader_free = match key {
        StripKey::Track(i) => {
            send_bus.is_some() || crate::automation::editable(&p.tracks[i], AutoTarget::Volume)
        }
        StripKey::StereoOut => true,
    };
    let moved = fader_and_meter(
        ui,
        g,
        fader_area,
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
    let by = strip.bottom() - 22.0 - name_h;
    let bw = (inner_w - 6.0) / 2.0;
    let m_r = Rect::from_min_size(pos2(sx, by), vec2(bw, 16.0));
    let s_r = Rect::from_min_size(pos2(sx + bw + 6.0, by), vec2(bw, 16.0));
    match key {
        StripKey::Track(i) => {
            // Mute shows its envelope while that plays; a click overrides it.
            let muted = crate::automation::shown_value(&p.tracks[i], AutoTarget::Mute, beat) >= 0.5;
            let t = &mut p.tracks[i];
            if small_button(ui, g, m_r, id(ns, (key, "m")), "M", muted, MUTE_ON) {
                t.mute = !muted;
            }
            crate::automation_ui::led(
                g,
                pos2(m_r.left() + 4.0, m_r.top() + 4.0),
                &t.auto,
                AutoTarget::Mute,
            );
            if small_button(ui, g, s_r, id(ns, (key, "s")), "S", t.solo, SOLO_ON) {
                t.solo = !t.solo;
            }
        }
        // Stereo Out has no mute and no bounce button (Bounce is not built).
        StripKey::StereoOut => {}
    }

    // Name: click selects the track, double-click renames it.
    let name_r = Rect::from_min_max(pos2(strip.left(), strip.bottom() - name_h), strip.max);
    let font = FontId::proportional(10.5);
    if let StripKey::Track(i) = key {
        let resp = ui.interact(name_r, id(ns, (key, "name")), Sense::click());
        if resp.clicked() {
            p.selected_track = i;
        }
        let rename_key = id(ns, (key, "rename"));
        if let Some(new) =
            crate::widgets::rename_field(ui, &resp, name_r, rename_key, &name, font.clone())
        {
            p.tracks[i].name = new;
        }
    }
    let shown = elide(g, &name, &font, name_r.width() - 6.0);
    match name_style {
        NameStyle::Coloured => {
            g.rect_filled(name_r, 2.0, color);
            g.text(
                name_r.center(),
                Align2::CENTER_CENTER,
                shown,
                font,
                Color32::WHITE,
            );
        }
        NameStyle::Plain => {
            g.text(name_r.center(), Align2::CENTER_CENTER, shown, font, TEXT);
        }
    }
    if key == StripKey::StereoOut && name_style == NameStyle::Coloured {
        g.rect_stroke(
            strip,
            2.0,
            Stroke::new(1.5, Color32::from_rgb(0xb8, 0x3c, 0xc8)),
            StrokeKind::Inside,
        );
    }
    if selected && name_style == NameStyle::Coloured {
        g.rect_stroke(
            strip,
            2.0,
            Stroke::new(1.0, Color32::from_gray(0x8a)),
            StrokeKind::Inside,
        );
    }
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
fn setting_menu(ui: &Ui, r: Rect, p: &mut Project, v: &mut MixerView, i: usize, id: egui::Id) {
    let resp = ui.interact(r, id, Sense::click());
    egui::Popup::menu(&resp).show(|ui| {
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
    slot(g, r, &text, SLOT_GRAY, TEXT);
    if p.tracks[i].kind == TrackKind::Aux {
        return;
    }
    let resp = ui.interact(r, id, Sense::click());
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
    slot(g, r, &input_label(source), SLOT_GRAY, TEXT);
    // One ring for a mono input, two for a stereo pair.
    let ring = pos2(r.left() + 8.0, r.center().y);
    g.circle_stroke(ring, 3.5, stroke(TEXT));
    if matches!(source, InputSource::Stereo(_)) {
        g.circle_stroke(ring + vec2(3.0, 0.0), 3.5, stroke(TEXT));
    }
    let resp = ui.interact(r, id, Sense::click());
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
    let fill = if name.is_empty() {
        SLOT_EMPTY
    } else {
        SLOT_GRAY
    };
    slot(
        g,
        r,
        &elide(g, &name, &FontId::proportional(10.5), r.width() - 8.0),
        fill,
        TEXT,
    );
    let vcas: Vec<(usize, String)> = p
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TrackKind::Vca)
        .map(|(j, t)| (j, t.name.clone()))
        .collect();
    let resp = ui.interact(r, id, Sense::click());
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

fn toolbar(ui: &Ui, p: &mut Project, s: &mut UiState, bar: Rect) {
    const NS: &str = "mixer_toolbar";
    let g = ui.painter_at(bar);
    g.rect_filled(bar, 0.0, TOOLBAR_BG);
    g.line_segment(
        [bar.left_bottom(), bar.right_bottom()],
        Stroke::new(1.0, Color32::from_gray(0x24)),
    );
    let cy = bar.center().y;
    let (can_undo, can_redo) = (s.can_undo, s.can_redo);
    let mut undo_pick = None;
    let v = &mut s.mixer;

    let mut x = bar.left() + 8.0;
    let (edit, w) = menu_title(ui, &g, x, cy, tr("Edit"), id(NS, "Edit"));
    x += w + 2.0;
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
    x += w + 2.0;
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
    x += w + 12.0;
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
    // Right-hand side first (strip width buttons, then filters), so the rest can be fitted
    // into what is left.
    let mut rx = bar.right() - 8.0;
    for (wide, key) in [(true, "wide"), (false, "narrow")] {
        let r = Rect::from_min_size(pos2(rx - 26.0, cy - 10.0), vec2(26.0, 20.0));
        let resp = ui.interact(r, id(NS, key), Sense::click());
        if v.wide == wide {
            g.rect_filled(r, 4.0, Color32::from_gray(0x5a));
        }
        let c = r.center();
        let iw = if wide { 13.0 } else { 9.0 };
        g.rect_stroke(
            Rect::from_center_size(c, vec2(iw, 11.0)),
            1.5,
            stroke(TEXT),
            StrokeKind::Middle,
        );
        g.line_segment([c + vec2(0.0, -5.5), c + vec2(0.0, 5.5)], stroke(TEXT));
        if resp.clicked() {
            v.wide = wide;
        }
        rx = r.left() - 4.0;
    }
    rx -= 10.0;
    let filter_widths: Vec<f32> = FILTERS
        .iter()
        .map(|label| {
            g.layout_no_wrap(tr(label).into(), FontId::proportional(12.5), TEXT)
                .size()
                .x
                + 16.0
        })
        .collect();
    let filters_w = filter_widths.iter().map(|w| w + 3.0).sum::<f32>();
    let fit = fit_toolbar(menus_end, bar.center().x, rx, filters_w);

    if fit.filters_inline {
        for (i, label) in FILTERS.iter().enumerate().rev() {
            let w = filter_widths[i];
            let r = Rect::from_min_size(pos2(rx - w, cy - 10.0), vec2(w, 20.0));
            let resp = ui.interact(r, id(NS, ("filter", i)), Sense::click());
            let on = v.filters[i];
            g.rect_filled(r, 4.0, if on { BLUE } else { Color32::from_gray(0x50) });
            g.text(
                r.center(),
                Align2::CENTER_CENTER,
                tr(label),
                FontId::proportional(12.5),
                if on { Color32::WHITE } else { TEXT_DIM },
            );
            if resp.clicked() {
                v.filters[i] = !v.filters[i];
            }
            rx = r.left() - 3.0;
        }
    } else {
        // Not enough room for a button per strip type: one menu with a check for each.
        let r = Rect::from_min_size(
            pos2(rx - FILTER_MENU_W, cy - 10.0),
            vec2(FILTER_MENU_W, 20.0),
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
    let seg_w = MODE_W / modes.len() as f32;
    for (i, (m, label)) in modes.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(left + i as f32 * seg_w, cy - 10.0),
            vec2(seg_w - 2.0, 20.0),
        );
        let resp = ui.interact(r, id(NS, ("mode", *label)), Sense::click());
        let on = v.mode == *m;
        if on {
            g.rect_filled(r, 4.0, BLUE);
        } else if resp.hovered() {
            g.rect_filled(r, 4.0, Color32::from_white_alpha(14));
        }
        g.text(
            r.center(),
            Align2::CENTER_CENTER,
            *label,
            FontId::proportional(12.5),
            if on { Color32::WHITE } else { TEXT },
        );
        if resp.clicked() {
            v.mode = *m;
        }
    }
}

/// Width of the Single | Tracks switch, of the "Sends on Faders" group and of the
/// "Show" menu that replaces the filter buttons when they do not fit.
const MODE_W: f32 = 116.0;
const SENDS_W: f32 = 236.0;
const FILTER_MENU_W: f32 = 70.0;
const TOOLBAR_GAP: f32 = 10.0;

/// Where the Mixer toolbar's groups go so that none overlaps another: the mode switch at the
/// centre when there is room, else just left of the filters; the filters as buttons when they
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
    // Sends on Faders: the faders set the sends to a bus.
    g.text(
        pos2(x, cy),
        Align2::LEFT_CENTER,
        tr("Sends on Faders:"),
        FontId::proportional(12.5),
        TEXT_DIM,
    );
    let pw = pos2(x + 112.0, cy + 1.0);
    g.circle_stroke(pw, 5.5, stroke(TEXT_DIM));
    g.line_segment(
        [pw + vec2(0.0, -7.5), pw + vec2(0.0, -1.5)],
        Stroke::new(3.0, TOOLBAR_BG),
    );
    g.line_segment(
        [pw + vec2(0.0, -7.0), pw + vec2(0.0, -2.0)],
        stroke(TEXT_DIM),
    );
    let sof = Rect::from_min_size(pos2(x + 126.0, cy - 10.0), vec2(110.0, 20.0));
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

pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState, meters: &Meters) {
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, Sense::hover());
    let bar = Rect::from_min_size(area.min, vec2(area.width(), TOOLBAR_H));
    toolbar(ui, p, s, bar);
    let v = &mut s.mixer;

    let body = Rect::from_min_max(pos2(area.left(), bar.bottom()), area.max);
    let labels = Rect::from_min_max(body.min, pos2(body.left() + LABEL_W, body.bottom()));
    let strips_area = Rect::from_min_max(pos2(labels.right(), body.top()), body.max);
    let g = ui.painter_at(body);
    g.rect_filled(body, 0.0, MIXER_BG);

    // Which strips, in order.
    let filters = v.filters;
    let mut keys: Vec<StripKey> = if p.tracks.is_empty() {
        Vec::new()
    } else {
        match v.mode {
            MixerMode::Single => vec![StripKey::Track(p.selected_track.min(p.tracks.len() - 1))],
            MixerMode::Tracks => (0..p.tracks.len())
                .filter(|i| strip_visible(p.tracks[*i].kind, &filters))
                .map(StripKey::Track)
                .collect(),
        }
    };
    if filters[3] {
        keys.push(StripKey::StereoOut);
    }

    let strip_w = if v.wide { 96.0 } else { 80.0 };
    let total_w = keys.len() as f32 * (strip_w + GAP);
    if ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pt| strips_area.contains(pt))
    {
        let d = ui.input(|i| i.smooth_scroll_delta);
        v.scroll_x -= d.x + d.y;
    }
    v.scroll_x = v
        .scroll_x
        .clamp(0.0, (total_w - strips_area.width() + 8.0).max(0.0));
    let r = rows(body.top(), fx_rows_for(p, &keys), send_rows_for(p, &keys));

    // Row labels.
    let gl = ui.painter_at(labels);
    let lx = labels.right() - 6.0;
    let label_at = |y: f32, h: f32, t: &str| {
        gl.text(
            pos2(lx, y + h / 2.0),
            Align2::RIGHT_CENTER,
            t,
            FontId::proportional(10.5),
            TEXT_DIM,
        );
    };
    label_at(r.setting, 16.0, tr("Setting"));
    label_at(r.input, 16.0, tr("Input"));
    label_at(r.fx, 15.0, tr("Audio FX"));
    label_at(r.sends, 16.0, tr("Sends"));
    label_at(r.output, 16.0, tr("Output"));
    label_at(r.group, 16.0, tr("Group"));
    label_at(r.pan, 24.0, tr("Pan"));
    label_at(r.db, 14.0, "dB");

    let gs = ui.painter_at(strips_area);
    for (n, key) in keys.iter().enumerate() {
        let x = strips_area.left() + 4.0 + n as f32 * (strip_w + GAP) - v.scroll_x;
        let strip = Rect::from_min_max(
            pos2(x, body.top() + 2.0),
            pos2(x + strip_w, body.bottom() - 2.0),
        );
        if strip.right() < strips_area.left() || strip.left() > strips_area.right() {
            continue;
        }
        channel_strip(
            ui,
            &gs,
            strip,
            *key,
            p,
            v,
            meters,
            &r,
            "mixer",
            NameStyle::Coloured,
        );
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
        let keys: Vec<StripKey> = (0..p.tracks.len())
            .map(StripKey::Track)
            .chain([StripKey::StereoOut])
            .collect();
        let r = rows(TOOLBAR_H, fx_rows_for(p, &keys), send_rows_for(p, &keys));
        let strip_bottom = 700.0 - 2.0;
        let top = r.fader_top + 8.0;
        let bottom = strip_bottom - 26.0 - 22.0 - 6.0;
        let sx = LABEL_W + 4.0 + 4.0;
        pos2(sx + 22.0, bottom - db_to_frac(db) * (bottom - top))
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
        assert_eq!(send_rows_for(&p, &keys), 3);
        assert_eq!(send_rows_for(&Project::empty(), &[StripKey::Track(0)]), 1);
        let r = rows(0.0, 3, 3);
        assert!(r.output - r.sends >= 3.0 * 17.0);
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
}
