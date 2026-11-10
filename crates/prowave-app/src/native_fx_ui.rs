//! The native effects (ports of Lemon Studio's recreations of FL Studio's Fruity Limiter,
//! Maximus, Transient Processor, Soft Clipper and Fast Dist, and Pro Wave Tune) in the Audio
//! FX slots: their settings, kept in the slot, and their windows, built as PW-1 hardware
//! units: a faceplate with the model, sections of knobs and keys with LCD readouts, and a
//! display where the unit has a curve to show.
use crate::gear::{self, Change, Control};
use crate::model::Project;
use crate::plugins::PluginSlot;
use egui::{pos2, vec2, Align2, Pos2, Rect, Shape, Stroke, Ui};
use prowave_engine::{native_info, NativeFxInfo};

/// Whether `slot` holds a native effect.
pub fn is_native(slot: &PluginSlot) -> bool {
    native_info(&slot.id).is_some()
}

/// A slot with native effect `info` at its defaults.
pub fn new_slot(info: &NativeFxInfo) -> PluginSlot {
    let mut slot = PluginSlot::new(info.id, info.name);
    slot.state = Some(encode(&info.defaults()));
    slot
}

/// The parameter values a slot keeps (the defaults where it has none).
pub fn values(slot: &PluginSlot, info: &NativeFxInfo) -> Vec<f32> {
    let saved: Vec<f32> = slot
        .state
        .as_deref()
        .and_then(|b| serde_json::from_slice::<Vec<f32>>(b).ok())
        .unwrap_or_default();
    let defaults = info.defaults();
    defaults
        .iter()
        .enumerate()
        .map(|(k, d)| {
            saved
                .get(k)
                .copied()
                .filter(|v| v.is_finite())
                .unwrap_or(*d)
        })
        .collect()
}

pub fn encode(values: &[f32]) -> Vec<u8> {
    serde_json::to_vec(values).unwrap_or_default()
}

/// A parameter moved in a window: (track, slot uid, parameter, value).
pub type Edit = (usize, u64, usize, f32);

/// Margin inside the faceplate.
const PAD: f32 = 26.0;

/// The model code on a native unit's faceplate.
fn model_code(id: &str) -> &'static str {
    use prowave_engine::native_fx::{
        lemon_fast_distortion, lemon_limiter, lemon_soft_clipper, lemon_transient_processor, press,
    };
    match id {
        lemon_limiter::ID => "LX-18",
        press::ID => "MX-4",
        lemon_transient_processor::ID => "TP-2",
        lemon_soft_clipper::ID => "SC-1",
        lemon_fast_distortion::ID => "FD-1",
        prowave_engine::tune::ID => "PT-1",
        _ => "FX-1",
    }
}

/// A section of a unit: its silkscreen label and its parameters, in order.
struct Section {
    label: &'static str,
    params: Vec<usize>,
}

/// A row of sections side by side.
type Row = Vec<Section>;

fn sec(label: &'static str, params: &[usize]) -> Section {
    Section {
        label,
        params: params.to_vec(),
    }
}

/// How a unit lays its parameters out: rows of sections, and the display at the top.
fn layout(info: &NativeFxInfo, band: usize) -> (Vec<Row>, Display) {
    use prowave_engine::native_fx::press::params as pp;
    use prowave_engine::native_fx::{
        lemon_fast_distortion as fd, lemon_limiter, lemon_soft_clipper,
        lemon_transient_processor as tp, press,
    };
    match info.id {
        lemon_limiter::ID => (
            vec![
                vec![
                    sec("Gain", &[0, 1]),
                    sec("Noise gate", &[15, 16, 17]),
                    sec("Safety", &[18]),
                ],
                vec![sec("Limiter", &[2, 3, 4, 5, 6, 7])],
                vec![sec("Compressor", &[8, 9, 10, 11, 12, 13, 14])],
            ],
            Display::Limiter,
        ),
        press::ID => {
            let b = 7 + band.min(3) * 13;
            let band_params: Vec<usize> = (b..b + 13).collect();
            (
                vec![
                    vec![sec(
                        "Crossover",
                        &[
                            pp::P_LOW_FREQ,
                            pp::P_LOW_SLOPE,
                            pp::P_HIGH_FREQ,
                            pp::P_HIGH_SLOPE,
                            pp::P_LMH_DELAY,
                            pp::P_LMH_MIX,
                            pp::P_LOW_CUT,
                        ],
                    )],
                    vec![Section {
                        label: ["Low band", "Mid band", "High band", "Master"][band.min(3)],
                        params: band_params[..7].to_vec(),
                    }],
                    vec![Section {
                        label: "",
                        params: band_params[7..].to_vec(),
                    }],
                    vec![sec(
                        "Options",
                        &[pp::OVERSAMPLING, pp::FIR, pp::MID_TRIGGER, pp::SAFETY],
                    )],
                ],
                Display::Bands,
            )
        }
        tp::ID => (
            vec![
                vec![
                    sec("Attack", &[tp::ATTACK, tp::ATTACK_SHAPE]),
                    sec("Release", &[tp::RELEASE, tp::RELEASE_SHAPE]),
                    sec("Output", &[tp::DRIVE, tp::GAIN]),
                ],
                vec![
                    sec("Split", &[tp::SPLIT, tp::BALANCE]),
                    sec("Processing", &[tp::ON]),
                ],
            ],
            Display::None,
        ),
        lemon_soft_clipper::ID => (vec![vec![sec("", &[0, 1])]], Display::SoftClip),
        fd::ID => (
            vec![vec![sec(
                "",
                &[fd::PRE, fd::THRESHOLD, fd::MIX, fd::POST, fd::TYPE],
            )]],
            Display::None,
        ),
        _ => {
            // Any other: knobs six to a row, then the choices.
            let knobs: Vec<usize> = (0..info.params.len())
                .filter(|k| info.params[*k].choices.is_empty())
                .collect();
            let choices: Vec<usize> = (0..info.params.len())
                .filter(|k| !info.params[*k].choices.is_empty())
                .collect();
            let mut rows: Vec<Row> = knobs
                .chunks(6)
                .map(|c| {
                    vec![Section {
                        label: "",
                        params: c.to_vec(),
                    }]
                })
                .collect();
            for c in choices.chunks(6) {
                rows.push(vec![Section {
                    label: "",
                    params: c.to_vec(),
                }]);
            }
            (rows, Display::None)
        }
    }
}

/// What a unit shows on its LCD.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Display {
    None,
    /// Gain, compressor and ceiling as a transfer curve.
    Limiter,
    /// The crossover's three bands.
    Bands,
    /// The soft clipper's curve.
    SoftClip,
}

/// A label without the section's own word ("Limiter attack time" in "Limiter": "Attack time";
/// "LOW Pre gain": "Pre gain").
fn short_label(name: &str) -> String {
    let mut words: Vec<&str> = name.split_whitespace().collect();
    if words.len() > 1
        && matches!(
            words[0],
            "Limiter" | "Comp" | "Noise" | "LOW" | "MID" | "HIGH" | "MASTER" | "LMH"
        )
    {
        words.remove(0);
    }
    if words.len() > 1 && words.last() == Some(&"time") {
        words.pop();
    }
    let s = words.join(" ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

/// Parameter `k` as a panel control.
fn control(info: &NativeFxInfo, k: usize, v: f32, automated: Option<bool>) -> Control {
    let param = &info.params[k];
    let span = (param.max - param.min).max(1e-6);
    Control {
        label: short_label(param.name),
        norm: ((v - param.min) / span).clamp(0.0, 1.0),
        default: ((param.default - param.min) / span).clamp(0.0, 1.0),
        text: info.value_text(k, v),
        choices: param.choices.iter().map(|c| c.to_string()).collect(),
        choice: (v - param.min).round().max(0.0) as usize,
        automated,
        enabled: param.live || !param.choices.is_empty(),
    }
}

/// The stored value a panel change sets.
pub(crate) fn value_of(info: &NativeFxInfo, ch: &Change) -> f32 {
    let param = &info.params[ch.index];
    match ch.choice {
        Some(i) => param.min + i as f32,
        None => {
            let v = param.min + (param.max - param.min) * ch.norm;
            if param.integer {
                v.round()
            } else {
                v
            }
        }
    }
}

/// Width of the widest row.
fn unit_width(rows: &[Row]) -> f32 {
    let cells = rows
        .iter()
        .map(|r| {
            r.iter().map(|s| s.params.len()).sum::<usize>() as f32 + (r.len() as f32 - 1.0) * 0.25
        })
        .fold(0.0, f32::max);
    (cells * gear::CELL_W + PAD * 2.0).max(560.0)
}

/// Height of the rows.
fn rows_height(rows: &[Row]) -> f32 {
    rows.iter()
        .map(|r| {
            let labelled = r.iter().any(|s| !s.label.is_empty());
            gear::CELL_H + 10.0 + if labelled { 22.0 } else { 0.0 }
        })
        .sum()
}

/// Lays `rows` out from `top_left`; returns the changes.
fn draw_rows(
    ui: &mut Ui,
    top_left: Pos2,
    width: f32,
    id: egui::Id,
    rows: &[Row],
    controls: &[Control],
) -> Vec<Change> {
    let p = ui.painter().clone();
    let mut out = Vec::new();
    let mut y = top_left.y;
    for row in rows {
        let labelled = row.iter().any(|s| !s.label.is_empty());
        let cells: f32 = row.iter().map(|s| s.params.len() as f32).sum::<f32>()
            + (row.len() as f32 - 1.0) * 0.25;
        let mut x = top_left.x + (width - cells * gear::CELL_W) / 2.0;
        for s in row {
            let w = s.params.len() as f32 * gear::CELL_W;
            if !s.label.is_empty() {
                gear::bracket(&p, pos2(x + 6.0, y + 8.0), w - 12.0, s.label);
            }
            let top = y + if labelled { 22.0 } else { 0.0 };
            out.extend(gear::control_grid(
                ui,
                pos2(x, top),
                s.params.len(),
                id,
                controls,
                &s.params,
                46.0,
            ));
            x += w + gear::CELL_W * 0.25;
        }
        y += gear::CELL_H + 10.0 + if labelled { 22.0 } else { 0.0 };
    }
    out
}

/// The number in a value text ("-3.2dB" gives -3.2).
fn number(text: &str) -> Option<f32> {
    gear::split_value(text).0.parse().ok()
}

/// The unit's LCD for `display`.
fn draw_display(p: &egui::Painter, rect: Rect, display: Display, info: &NativeFxInfo, v: &[f32]) {
    let glass = gear::lcd(p, rect);
    let ink = Stroke::new(2.2, gear::LCD_INK);
    let faint = Stroke::new(1.0, gear::alpha(gear::LCD_INK, 0.12));
    let text = |k: usize| info.value_text(k, v[k]);
    match display {
        Display::None => {}
        Display::Limiter => {
            // Output over input, -48 to +12 dB: gain, the compressor, then the ceiling.
            let plot = glass.shrink2(vec2(glass.width() / 2.0 - glass.height() * 0.7, 10.0));
            let x_of = |db: f32| plot.left() + (db + 48.0) / 60.0 * plot.width();
            let y_of = |db: f32| plot.bottom() - (db + 48.0) / 60.0 * plot.height();
            for d in [-36.0f32, -24.0, -12.0, 0.0] {
                p.line_segment(
                    [pos2(x_of(d), glass.top()), pos2(x_of(d), glass.bottom())],
                    faint,
                );
                p.line_segment(
                    [pos2(glass.left(), y_of(d)), pos2(glass.right(), y_of(d))],
                    faint,
                );
            }
            let gain = number(&text(0)).unwrap_or(0.0);
            let thr = number(&text(8)).unwrap_or(0.0);
            let ratio = number(&text(9)).unwrap_or(1.0).max(1.0);
            let ceil = number(&text(2)).unwrap_or(0.0);
            let pts: Vec<Pos2> = (0..=120)
                .map(|i| {
                    let x = -48.0 + 60.0 * i as f32 / 120.0;
                    let mut y = x + gain;
                    if y > thr {
                        y = thr + (y - thr) / ratio;
                    }
                    y = y.min(ceil);
                    pos2(x_of(x), y_of(y.clamp(-48.0, 12.0)))
                })
                .collect();
            p.with_clip_rect(glass).add(Shape::line(pts, ink));
            gear::lcd_label(
                p,
                glass.left_top() + vec2(8.0, 6.0),
                Align2::LEFT_TOP,
                "Ceiling",
            );
            let t = format!("{ceil:.1}");
            gear::seg7(
                p,
                glass.left_top() + vec2(8.0, 22.0),
                &t,
                26.0,
                gear::LCD_INK,
                true,
            );
            gear::lcd_label(
                p,
                pos2(glass.right() - 8.0, glass.top() + 6.0),
                Align2::RIGHT_TOP,
                "Gain",
            );
            let t = format!("{gain:.1}");
            let w = gear::seg7_width(&t, 26.0);
            gear::seg7(
                p,
                pos2(glass.right() - 8.0 - w, glass.top() + 22.0),
                &t,
                26.0,
                gear::LCD_INK,
                true,
            );
        }
        Display::Bands => {
            use prowave_engine::native_fx::press::params as pp;
            let plot = glass.shrink2(vec2(14.0, 12.0));
            let x_of = |hz: f32| plot.left() + (hz.max(20.0) / 20.0).log10() / 3.0 * plot.width();
            let lo = number(&text(pp::P_LOW_FREQ)).unwrap_or(200.0);
            let hi = number(&text(pp::P_HIGH_FREQ)).unwrap_or(2000.0);
            let k = |t: String, n: f32| {
                if t.to_lowercase().contains("khz") {
                    n * 1000.0
                } else {
                    n
                }
            };
            let (lo, hi) = (k(text(pp::P_LOW_FREQ), lo), k(text(pp::P_HIGH_FREQ), hi));
            let (xl, xh) = (x_of(lo), x_of(hi));
            for x in [xl, xh] {
                p.line_segment(
                    [pos2(x, glass.top() + 4.0), pos2(x, glass.bottom() - 4.0)],
                    ink,
                );
            }
            for (a, b, name) in [
                (plot.left(), xl, "LOW"),
                (xl, xh, "MID"),
                (xh, plot.right(), "HIGH"),
            ] {
                gear::lcd_text(
                    p,
                    pos2((a + b) / 2.0, glass.center().y),
                    Align2::CENTER_CENTER,
                    name,
                    16.0,
                );
            }
            gear::lcd_label(
                p,
                pos2(xl + 4.0, glass.bottom() - 4.0),
                Align2::LEFT_BOTTOM,
                &text(pp::P_LOW_FREQ),
            );
            gear::lcd_label(
                p,
                pos2(xh + 4.0, glass.bottom() - 4.0),
                Align2::LEFT_BOTTOM,
                &text(pp::P_HIGH_FREQ),
            );
        }
        Display::SoftClip => {
            use prowave_engine::native_fx::lemon_soft_clipper as sc;
            let plot = glass.shrink2(vec2(glass.width() / 2.0 - glass.height() * 0.6, 10.0));
            p.line_segment(
                [
                    pos2(plot.center().x, glass.top()),
                    pos2(plot.center().x, glass.bottom()),
                ],
                faint,
            );
            p.line_segment(
                [
                    pos2(glass.left(), plot.center().y),
                    pos2(glass.right(), plot.center().y),
                ],
                faint,
            );
            let (t, kk, r) = sc::constants(v[0].round() as i32);
            let pts: Vec<Pos2> = (0..=160)
                .map(|i| {
                    let x = -1.25 + 2.5 * i as f32 / 160.0;
                    let y = sc::clip(x, t, kk, r);
                    pos2(
                        plot.center().x + x / 1.25 * plot.width() / 2.0,
                        plot.center().y - y / 1.25 * plot.height() / 2.0,
                    )
                })
                .collect();
            p.with_clip_rect(glass).add(Shape::line(pts, ink));
            gear::lcd_label(
                p,
                glass.left_top() + vec2(8.0, 6.0),
                Align2::LEFT_TOP,
                "Threshold",
            );
            let (num, _) = gear::split_value(&text(0));
            gear::seg7(
                p,
                glass.left_top() + vec2(8.0, 22.0),
                &num,
                26.0,
                gear::LCD_INK,
                true,
            );
        }
    }
}

/// The window of the native effect in slot `uid`, if one is open (`s.native_window`).
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    let Some(uid) = s.native_window else { return };
    // A track's effect, or the Stereo Out's (`ti` is then `mixer::MASTER_STRIP`).
    let found = p.find_au_effect(uid);
    let Some((ti, j)) = found else {
        s.native_window = None;
        return;
    };
    let Some(slot) = p.strip_plugins(ti).and_then(|fx| fx.get(j)).cloned() else {
        s.native_window = None;
        return;
    };
    let Some(info) = native_info(&slot.id) else {
        s.native_window = None;
        return;
    };
    let mut vals = values(&slot, info);
    // What the envelopes play now (Live moves the knobs of automated parameters), and their
    // LEDs: lit while an envelope plays, dark when overridden by hand.
    let beat = p.position_beats;
    let auto = p.strip_auto(ti).cloned().unwrap_or_default();
    let target = |k: usize| prowave_engine::AutoTarget::Plugin {
        slot: j as u8 + 1,
        param: k as u32,
    };
    let played = |k: usize| {
        let param = info.params.get(k)?;
        auto.played(target(k), beat).map(|n| {
            let x = param.min + (param.max - param.min) * n.clamp(0.0, 1.0);
            if param.integer {
                x.round()
            } else {
                x
            }
        })
    };
    let automated =
        |k: usize| (!auto.lane(target(k)).is_empty()).then(|| !auto.is_overridden(target(k)));
    let shown: Vec<f32> = (0..vals.len())
        .map(|k| played(k).unwrap_or(vals[k]))
        .collect();
    let controls: Vec<Control> = (0..vals.len())
        .map(|k| control(info, k, shown[k], automated(k)))
        .collect();
    let (badge, strip_name) = crate::fx_ui::strip_plate(p, ti);
    let tune = info.id == prowave_engine::tune::ID;
    let plate = gear::Plate {
        model: model_code(info.id),
        name: info.name,
        strip: Some((&badge, &strip_name)),
        reset: true,
    };
    let id = egui::Id::unique("native_unit").with(uid);
    let band_id = id.with("band");
    let band: usize = ctx.data(|d| d.get_temp(band_id)).unwrap_or(0);
    let (rows, display) = layout(info, band);
    let tune_cx = tune.then(|| tune_context(p, ti, uid, &vals));
    let width = if tune {
        crate::tune_ui::WIDTH
    } else {
        unit_width(&rows)
    };
    let mut edits: Vec<Edit> = Vec::new();
    let (head, _) = gear::unit_window(ctx, id, width, &plate, Some(!slot.bypass), |ui| {
        if let Some(cx) = &tune_cx {
            crate::tune_ui::body(ui, id, info, cx, &controls, &shown, &mut vals, &mut edits);
            return;
        }
        let disp_h = if display == Display::None { 0.0 } else { 128.0 };
        let band_h = if display == Display::Bands { 44.0 } else { 0.0 };
        let total = 14.0 + disp_h + band_h + rows_height(&rows) + 8.0;
        let (rect, _) = ui.allocate_exact_size(vec2(width, total), egui::Sense::hover());
        let painter = ui.painter().clone();
        let mut y = rect.top() + 14.0;
        if display != Display::None {
            let r = Rect::from_min_size(
                pos2(rect.left() + PAD, y + 4.0),
                vec2(width - PAD * 2.0, 110.0),
            );
            draw_display(&painter, r, display, info, &shown);
            y += disp_h;
        }
        if display == Display::Bands {
            // The band keys: which band's settings the panel shows.
            let names = ["LOW", "MID", "HIGH", "MASTER"];
            let kw = 96.0;
            let x0 = rect.left() + (width - kw * 4.0 - 30.0) / 2.0;
            for (b, name) in names.iter().enumerate() {
                let r = Rect::from_min_size(pos2(x0 + b as f32 * (kw + 10.0), y), vec2(kw, 28.0));
                let on = b == band;
                if gear::key(
                    ui,
                    r,
                    id.with(("band", b)),
                    name,
                    gear::Cap::Bone,
                    gear::LedAt::Left,
                    on,
                    on,
                )
                .clicked()
                {
                    ui.data_mut(|d| d.insert_temp(band_id, b));
                }
            }
            y += band_h;
        }
        for ch in draw_rows(
            ui,
            pos2(rect.left() + PAD, y),
            width - PAD * 2.0,
            id,
            &rows,
            &controls,
        ) {
            let v = value_of(info, &ch);
            vals[ch.index] = v;
            edits.push((ti, uid, ch.index, v));
        }
    });
    if head.reset {
        for (k, d) in info.defaults().into_iter().enumerate() {
            vals[k] = d;
            edits.push((ti, uid, k, d));
        }
    }
    if head.power {
        if let Some(e) = p.strip_plugins_mut(ti).and_then(|(fx, _)| fx.get_mut(j)) {
            e.bypass = !e.bypass;
        }
    }
    if !edits.is_empty() {
        if let Some(e) = p.strip_plugins_mut(ti).and_then(|(fx, _)| fx.get_mut(j)) {
            e.state = Some(encode(&vals));
        }
        s.native_edits.extend(edits);
    }
    if head.close {
        s.native_window = None;
    }
}

/// What Pro Wave Tune's window needs of the project.
fn tune_context(p: &Project, ti: usize, uid: u64, vals: &[f32]) -> crate::tune_ui::Context {
    let (root, minor) = crate::tune_lane::key_root(p.key);
    let tracks = p
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| crate::tune_lane::has_notes(t))
        .map(|(i, t)| (i + 1, t.name.clone()))
        .collect();
    let mut auto_vals = vals.to_vec();
    if let Some(v) = auto_vals.get_mut(prowave_engine::tune::MIDI_TRACK) {
        *v = 0.0;
    }
    let auto_source = (ti != crate::mixer::MASTER_STRIP)
        .then(|| crate::tune_lane::midi_source(p, ti, &auto_vals))
        .flatten()
        .map(|i| p.tracks[i].name.clone());
    crate::tune_ui::Context {
        track: ti,
        uid,
        project_key: crate::score::key_label(p.key),
        project_root: root,
        project_minor: minor,
        tracks,
        auto_source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_missing_ones_take_defaults() {
        let info = &prowave_engine::native_effects()[0];
        let mut slot = new_slot(info);
        assert_eq!(values(&slot, info), info.defaults());
        slot.state = Some(encode(&[1.0]));
        let v = values(&slot, info);
        assert_eq!(v[0], 1.0);
        assert_eq!(&v[1..], &info.defaults()[1..]);
        slot.state = Some(b"junk".to_vec());
        assert_eq!(values(&slot, info), info.defaults());
    }
}
