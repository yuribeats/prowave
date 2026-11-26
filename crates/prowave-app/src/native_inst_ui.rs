//! The native instruments (the engine's `native_inst` table) in the Instrument slot: their
//! settings, kept in the slot as the native effects' are, and their window, a PW-1 hardware
//! unit laid out from the instrument's parameter table.
use crate::gear::{self, Cap, Control, LedAt};
use crate::i18n::tr;
use crate::model::Project;
use crate::native_fx_ui::{control, draw_rows, encode, rows_height, unit_width, value_of, values};
use crate::native_fx_ui::{Edit, Row, Section, PAD};
use crate::plugins::PluginSlot;
use egui::{pos2, vec2};
use prowave_engine::{native_instrument_info, NativeFxInfo, NativeInstInfo};

/// The entry of the native instrument in `slot`, if it holds one.
pub fn info_of(slot: &PluginSlot) -> Option<&'static NativeInstInfo> {
    native_instrument_info(&slot.id)
}

/// The entry of a native effect or a native instrument: both keep their settings the same
/// way, so automation and the control interface read them through this.
pub fn any_info(id: &str) -> Option<&'static NativeFxInfo> {
    prowave_engine::native_info(id).or_else(|| native_instrument_info(id))
}

/// An Instrument slot with native instrument `info` at its defaults.
pub fn new_slot(info: &NativeInstInfo) -> PluginSlot {
    let mut slot = PluginSlot::new(info.id, info.name);
    slot.state = Some(encode(&info.defaults()));
    slot
}

/// The track whose Instrument slot holds the native instrument `uid`.
pub fn track_of(p: &Project, uid: u64) -> Option<usize> {
    p.tracks.iter().position(|t| {
        t.au_instrument
            .as_ref()
            .is_some_and(|s| s.uid == uid && info_of(s).is_some())
    })
}

/// The model code on an instrument's faceplate.
fn model_code(id: &str) -> &'static str {
    use prowave_engine::native_inst::macro_osc;
    match id {
        macro_osc::ID => "MO-24",
        prowave_engine::native_inst::kick::ID => "KK-1",
        prowave_engine::native_inst::bass_line::ID => "BL-302",
        prowave_engine::native_inst::triple_osc::ID => "TO-3",
        prowave_engine::native_inst::pluck::ID => "PL-1",
        prowave_engine::native_inst::wave_draw::ID => "WD-200",
        prowave_engine::native_inst::engine_sound::ID => "EN-8",
        prowave_engine::native_inst::drum_synth::ID => "DS-8",
        prowave_engine::native_inst::monstro::ID => "MN-3",
        prowave_engine::native_inst::resonator::ID => "RS-6",
        prowave_engine::native_inst::digital_osc::ID => "DO-47",
        prowave_engine::native_inst::fm6::ID => "FM-6",
        prowave_engine::native_inst::chip_opl::ID => "CH-OPL",
        prowave_engine::native_inst::chip_opn::ID => "CH-OPN",
        prowave_engine::native_inst::chip_opll::ID => "CH-OPLL",
        prowave_engine::native_inst::chip_sid::ID => "CH-6581",
        prowave_engine::native_inst::chip_psg::ID => "CH-PSG",
        _ => "IN-1",
    }
}

/// The choices, then the knobs six to a row, of the parameters in `params`.
fn rows_of(info: &NativeInstInfo, params: &[usize]) -> Vec<Row> {
    let (choices, knobs): (Vec<usize>, Vec<usize>) = params
        .iter()
        .filter(|k| **k < info.params.len())
        .partition(|k| !info.params[**k].choices.is_empty());
    choices
        .chunks(6)
        .chain(knobs.chunks(6))
        .map(|c| {
            vec![Section {
                label: "",
                params: c.to_vec(),
            }]
        })
        .collect()
}

/// A page of an instrument with more parameters than one panel holds: the key that shows it,
/// its parameters, and the words its key already says (left off the knobs' labels).
struct Page {
    key: String,
    params: Vec<usize>,
    prefix: String,
}

/// The pages of an instrument; one page with an empty key for most.
fn pages(info: &NativeInstInfo) -> Vec<Page> {
    if info.id == prowave_engine::native_inst::fm6::ID {
        use prowave_engine::native_inst::fm6;
        let mut out: Vec<Page> = (1..=6)
            .map(|n| Page {
                key: format!("OP {n}"),
                params: ((6 - n) * fm6::OP_PARAMS..(7 - n) * fm6::OP_PARAMS).collect(),
                prefix: format!("Op {n} "),
            })
            .collect();
        out.push(Page {
            key: "PITCH".into(),
            params: (fm6::PITCH_ENV..fm6::PITCH_ENV + 8).collect(),
            prefix: "Pitch ".into(),
        });
        out.push(Page {
            key: "LFO".into(),
            params: (137..fm6::TRANSPOSE).collect(),
            prefix: "LFO ".into(),
        });
        out.push(Page {
            key: "VOICE".into(),
            params: vec![
                fm6::ALGORITHM,
                fm6::FEEDBACK,
                136,
                fm6::TRANSPOSE,
                fm6::VOICES,
                fm6::LEVEL,
            ],
            prefix: String::new(),
        });
        return out;
    }
    if info.id == prowave_engine::native_inst::wave_draw::ID {
        // The points are drawn on the display, not turned.
        return vec![Page {
            key: String::new(),
            params: (0..prowave_engine::native_inst::wave_draw::WAVE).collect(),
            prefix: String::new(),
        }];
    }
    if info.id == prowave_engine::native_inst::triple_osc::ID {
        use prowave_engine::native_inst::triple_osc as to;
        let mut out: Vec<Page> = (1..=3)
            .map(|n| Page {
                key: format!("OSC {n}"),
                params: ((n - 1) * to::OSC_PARAMS..n * to::OSC_PARAMS).collect(),
                prefix: format!("Osc {n} "),
            })
            .collect();
        out.push(Page {
            key: "VOICE".into(),
            params: (to::MOD_1..to::COUNT).collect(),
            prefix: String::new(),
        });
        return out;
    }
    if info.id == prowave_engine::native_inst::monstro::ID {
        use prowave_engine::native_inst::monstro as mo;
        let page = |key: &str, params: Vec<usize>, prefix: &str| Page {
            key: key.into(),
            params,
            prefix: prefix.into(),
        };
        let m = |t: usize, n: usize| mo::MATRIX + t * 4..mo::MATRIX + (t + n) * 4;
        let mut osc3: Vec<usize> = (mo::O3_VOL..mo::LFO).collect();
        osc3.push(mo::O23_MOD);
        let mut vol: Vec<usize> = m(mo::T_VOL, 3).collect();
        vol.push(mo::LEVEL);
        return vec![
            page("OSC 1", (mo::O1_VOL..mo::O2_VOL).collect(), "Osc 1 "),
            page("OSC 2", (mo::O2_VOL..mo::O3_VOL).collect(), "Osc 2 "),
            page("OSC 3", osc3, "Osc 3 "),
            page("LFO", (mo::LFO..mo::ENV).collect(), ""),
            page("ENV", (mo::ENV..mo::O23_MOD).collect(), ""),
            page("VOL", vol, "Vol "),
            page("PHASE", m(mo::T_PHS, 3).collect(), "Phase "),
            page("PITCH", m(mo::T_PIT, 3).collect(), "Pitch "),
            page("PW SUB", m(mo::T_PW, 2).collect(), ""),
        ];
    }
    if info.id == prowave_engine::native_inst::drum_synth::ID {
        use prowave_engine::native_inst::drum_synth as ds;
        let mut out: Vec<Page> = (1..=ds::PADS)
            .map(|n| Page {
                key: format!("PAD {n}"),
                params: (ds::PAD + (n - 1) * ds::PAD_PARAMS..ds::PAD + n * ds::PAD_PARAMS)
                    .collect(),
                prefix: format!("Pad {n} "),
            })
            .collect();
        out.push(Page {
            key: "KIT".into(),
            params: vec![ds::VOLUME, ds::OVERSAMPLING],
            prefix: String::new(),
        });
        return out;
    }
    if info.id == prowave_engine::native_inst::chip_opn::ID {
        use prowave_engine::native_inst::chip_opn as opn;
        let mut out: Vec<Page> = (1..=4)
            .map(|n| Page {
                key: format!("OP {n}"),
                params: (opn::OPS + (n - 1) * opn::OP_PARAMS..opn::OPS + n * opn::OP_PARAMS)
                    .collect(),
                prefix: format!("Op {n} "),
            })
            .collect();
        out.push(Page {
            key: "VOICE".into(),
            params: vec![
                opn::ALGORITHM,
                opn::FEEDBACK,
                opn::TUNE,
                opn::VOICES,
                opn::LEVEL,
            ],
            prefix: String::new(),
        });
        return out;
    }
    // More parameters than a panel holds, and no pages of its own: pages of 18, numbered.
    if info.params.len() > PAGE_PARAMS * 2 {
        let all: Vec<usize> = (0..info.params.len()).collect();
        return all
            .chunks(PAGE_PARAMS)
            .enumerate()
            .map(|(n, c)| Page {
                key: (n + 1).to_string(),
                params: c.to_vec(),
                prefix: String::new(),
            })
            .collect();
    }
    vec![Page {
        key: String::new(),
        params: (0..info.params.len()).collect(),
        prefix: String::new(),
    }]
}

/// Parameters on a numbered page.
const PAGE_PARAMS: usize = 18;

/// The voices of the .syx bank last opened in a window: (name, parameter values).
type Bank = std::sync::Arc<Vec<(String, Vec<f32>)>>;

/// Whether the instrument reads voices from .syx files (FM6).
fn loads_syx(info: &NativeInstInfo) -> bool {
    info.id == prowave_engine::native_inst::fm6::ID
}

/// The voices in the .syx file at `path` (none when it cannot be read or is not a bank).
fn read_bank(path: &std::path::Path) -> Bank {
    let voices = std::fs::read(path)
        .map(|bytes| prowave_engine::native_inst::fm6::bank_voices(&bytes))
        .unwrap_or_default();
    std::sync::Arc::new(voices)
}

/// Height of Wave Draw's display and of the row of shape keys under it.
const GRAPH_H: f32 = 150.0;
const SHAPE_ROW_H: f32 = 40.0;

/// Whether the instrument's wave is drawn on a display (Wave Draw).
fn draws_wave(info: &NativeInstInfo) -> bool {
    info.id == prowave_engine::native_inst::wave_draw::ID
}

/// The point and its value (-100 to 100) under `pos` on a display `glass` showing `length`
/// points.
fn point_at(glass: egui::Rect, pos: egui::Pos2, length: usize) -> (usize, f32) {
    let x = ((pos.x - glass.left()) / glass.width().max(1.0)).clamp(0.0, 0.999_999);
    let y = ((glass.center().y - pos.y) / (glass.height() / 2.0).max(1.0)).clamp(-1.0, 1.0);
    ((x * length as f32) as usize, (y * 100.0).round())
}

/// Wave Draw's display: the cycle's points, drawn by dragging across it, and the keys that
/// draw a whole shape. Returns the points that changed, as (parameter, value).
fn wave_graph(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    vals: &[f32],
) -> Vec<(usize, f32)> {
    use prowave_engine::native_inst::wave_draw as wd;
    let mut changed = Vec::new();
    let length = (vals[wd::LENGTH] as usize).clamp(4, wd::POINTS);
    let screen = egui::Rect::from_min_size(rect.min, vec2(rect.width(), GRAPH_H - 10.0));
    let p = ui.painter().clone();
    let glass = gear::lcd(&p, screen);
    let resp = ui.interact(glass, id.with("wave"), egui::Sense::click_and_drag());
    if let Some(pos) = resp
        .interact_pointer_pos()
        .filter(|_| resp.is_pointer_button_down_on())
    {
        let (i, v) = point_at(glass, pos, length);
        if vals[wd::WAVE + i] != v {
            changed.push((wd::WAVE + i, v));
        }
    }
    let faint = egui::Stroke::new(1.0, gear::alpha(gear::LCD_INK(), 0.12));
    p.line_segment([glass.left_center(), glass.right_center()], faint);
    // Each point as the step it is: a level held until the next.
    let step = glass.width() / length as f32;
    let y_of = |v: f32| glass.center().y - v / 100.0 * (glass.height() / 2.0 - 4.0);
    let mut line = Vec::with_capacity(length * 2);
    for i in 0..length {
        let v = changed
            .iter()
            .find(|(k, _)| *k == wd::WAVE + i)
            .map_or(vals[wd::WAVE + i], |(_, v)| *v);
        line.push(pos2(glass.left() + i as f32 * step, y_of(v)));
        line.push(pos2(glass.left() + (i + 1) as f32 * step, y_of(v)));
    }
    p.with_clip_rect(glass).add(egui::Shape::line(
        line,
        egui::Stroke::new(1.8, gear::LCD_INK()),
    ));
    // The shape keys.
    let keys = wd::SHAPES.len() as f32;
    let x0 = rect.left() + (rect.width() - keys * (PAGE_KEY_W + PAGE_KEY_GAP)) / 2.0;
    for (n, name) in wd::SHAPES.iter().enumerate() {
        let r = egui::Rect::from_min_size(
            pos2(
                x0 + n as f32 * (PAGE_KEY_W + PAGE_KEY_GAP),
                rect.top() + GRAPH_H,
            ),
            vec2(PAGE_KEY_W, 28.0),
        );
        let legend = name.to_uppercase();
        let key = gear::key(
            ui,
            r,
            id.with(("shape", n)),
            &legend,
            Cap::Bone,
            LedAt::None,
            false,
            false,
        );
        if key.clicked() {
            changed.clear();
            for i in 0..wd::POINTS {
                let v = wd::shape(name, i) as f32;
                if vals[wd::WAVE + i] != v {
                    changed.push((wd::WAVE + i, v));
                }
            }
        }
    }
    changed
}

/// Width of a page key and the gap between two.
const PAGE_KEY_W: f32 = 62.0;
const PAGE_KEY_GAP: f32 = 6.0;
/// Height of the row of page keys, with the space under it.
const PAGE_ROW_H: f32 = 40.0;

/// Puts a voice's parameter values (the first of the instrument's parameters) into `vals`;
/// returns the ones that changed, as (parameter, value).
fn take_voice(vals: &mut [f32], voice: &[f32]) -> Vec<(usize, f32)> {
    let mut changed = Vec::new();
    for (k, (old, new)) in vals.iter_mut().zip(voice).enumerate() {
        if *old != *new {
            *old = *new;
            changed.push((k, *new));
        }
    }
    changed
}

/// The window of the native instrument in slot `uid`, if one is open
/// (`s.native_inst_window`).
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    let Some(uid) = s.native_inst_window else {
        return;
    };
    let found = track_of(p, uid)
        .and_then(|ti| Some((ti, p.tracks[ti].au_instrument.clone()?)))
        .and_then(|(ti, slot)| Some((ti, info_of(&slot)?, slot)));
    let Some((ti, info, slot)) = found else {
        s.native_inst_window = None;
        return;
    };
    let mut vals = values(&slot, info);
    // What the envelopes play now, and the LEDs of the automated parameters, as on the native
    // effects' units.
    let beat = p.position_beats;
    let auto = p.strip_auto(ti).cloned().unwrap_or_default();
    let target = |k: usize| prowave_engine::AutoTarget::Plugin {
        slot: 0,
        param: k as u32,
    };
    let played = |k: usize| {
        let param = info.params.get(k)?;
        auto.played(target(k), beat)
            .map(|n| (param.min + (param.max - param.min) * n.clamp(0.0, 1.0)).round())
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
    let plate = gear::Plate {
        model: model_code(info.id),
        name: info.name,
        strip: Some((&badge, &strip_name)),
        reset: true,
    };
    let id = egui::Id::unique("native_instrument").with(uid);
    let all = pages(info);
    let page_id = id.with("page");
    let page: usize = ctx
        .data(|d| d.get_temp(page_id))
        .unwrap_or(0usize)
        .min(all.len() - 1);
    let bank_id = id.with("bank");
    let bank: Bank = ctx.data(|d| d.get_temp(bank_id)).unwrap_or_default();
    let syx = loads_syx(info);
    let keys = if all.len() > 1 { all.len() } else { 0 } + usize::from(syx);
    // Every page as wide and as tall as the largest, so the unit does not change size.
    let all_rows: Vec<Vec<Row>> = all.iter().map(|pg| rows_of(info, &pg.params)).collect();
    let keys_w = keys as f32 * (PAGE_KEY_W + PAGE_KEY_GAP) + PAD * 2.0;
    let width = all_rows
        .iter()
        .map(|r| unit_width(r))
        .fold(keys_w, f32::max);
    let rows_h = all_rows.iter().map(|r| rows_height(r)).fold(0.0, f32::max);
    let rows = &all_rows[page];
    // The page's own words are on its key, not on every knob.
    let mut controls = controls;
    for k in &all[page].params {
        if let Some(c) = controls.get_mut(*k) {
            if let Some(rest) = info.params[*k].name.strip_prefix(all[page].prefix.as_str()) {
                c.label = rest.to_string();
            }
        }
    }
    let mut edits: Vec<Edit> = Vec::new();
    let mut picked: Option<Vec<f32>> = None;
    let (head, _) = gear::unit_window(ctx, id, width, &plate, None, |ui| {
        let keys_h = if keys > 0 { PAGE_ROW_H } else { 0.0 };
        let graph_h = if draws_wave(info) {
            GRAPH_H + SHAPE_ROW_H
        } else {
            0.0
        };
        let total = 14.0 + graph_h + keys_h + rows_h + 8.0;
        let (rect, _) = ui.allocate_exact_size(vec2(width, total), egui::Sense::hover());
        let mut y = rect.top() + 14.0;
        if draws_wave(info) {
            let area = egui::Rect::from_min_size(
                pos2(rect.left() + PAD, y),
                vec2(width - PAD * 2.0, graph_h),
            );
            for (k, v) in wave_graph(ui, area, id, &shown) {
                vals[k] = v;
                edits.push((ti, uid, k, v));
            }
            y += graph_h;
        }
        if keys > 0 {
            let x0 = rect.left() + (width - keys as f32 * (PAGE_KEY_W + PAGE_KEY_GAP)) / 2.0;
            let key_rect = |n: usize| {
                egui::Rect::from_min_size(
                    pos2(x0 + n as f32 * (PAGE_KEY_W + PAGE_KEY_GAP), y),
                    vec2(PAGE_KEY_W, 28.0),
                )
            };
            let mut n = 0;
            if all.len() > 1 {
                for (b, pg) in all.iter().enumerate() {
                    let on = b == page;
                    let (r, kid) = (key_rect(n), id.with(("page", b)));
                    n += 1;
                    let key = gear::key(ui, r, kid, &pg.key, Cap::Bone, LedAt::Left, on, on);
                    if key.clicked() {
                        ui.data_mut(|d| d.insert_temp(page_id, b));
                    }
                }
            }
            if syx {
                // The .syx key: a file's voices, and the voices of the file opened last.
                let (r, kid) = (key_rect(n), id.with("syx"));
                let key = gear::key(ui, r, kid, "SYX", Cap::Dark, LedAt::None, false, false);
                egui::Popup::menu(&key).show(|ui| {
                    ui.set_min_width(150.0);
                    if ui.button(format!("{}…", tr("Choose"))).clicked() {
                        let file = rfd::FileDialog::new()
                            .add_filter("SysEx", &["syx"])
                            .pick_file();
                        if let Some(path) = file {
                            let read = read_bank(&path);
                            ui.data_mut(|d| d.insert_temp(bank_id, read));
                        }
                        ui.close();
                    }
                    if !bank.is_empty() {
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                for (name, voice) in bank.iter() {
                                    if ui.button(name.as_str()).clicked() {
                                        picked = Some(voice.clone());
                                        ui.close();
                                    }
                                }
                            });
                    }
                });
            }
            y += keys_h;
        }
        let top_left = pos2(rect.left() + PAD, y);
        for ch in draw_rows(ui, top_left, width - PAD * 2.0, id, rows, &controls) {
            let v = value_of(info, &ch);
            vals[ch.index] = v;
            edits.push((ti, uid, ch.index, v));
        }
    });
    if let Some(voice) = picked {
        let changed = take_voice(&mut vals, &voice);
        edits.extend(changed.into_iter().map(|(k, v)| (ti, uid, k, v)));
    }
    if head.reset {
        for (k, d) in info.defaults().into_iter().enumerate() {
            vals[k] = d;
            edits.push((ti, uid, k, d));
        }
    }
    if !edits.is_empty() {
        if let Some(slot) = p.tracks[ti].au_instrument.as_mut() {
            slot.state = Some(encode(&vals));
        }
        s.native_edits.extend(edits);
    }
    if head.close {
        s.native_inst_window = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_slot_holds_the_defaults_and_every_parameter_has_a_place_on_the_panel() {
        for info in prowave_engine::native_instruments() {
            let slot = new_slot(info);
            assert_eq!(slot.id, info.id);
            assert_eq!(values(&slot, info), info.defaults());
            assert!(info_of(&slot).is_some());
            assert!(any_info(info.id).is_some());
            // Every parameter is on one page, once.
            let mut placed: Vec<usize> = pages(info)
                .iter()
                .flat_map(|pg| rows_of(info, &pg.params))
                .flat_map(|row| row.into_iter().flat_map(|s| s.params))
                .collect();
            if draws_wave(info) {
                // The wave's points are on the display.
                placed.extend(prowave_engine::native_inst::wave_draw::WAVE..info.params.len());
            }
            placed.sort_unstable();
            assert_eq!(placed, (0..info.params.len()).collect::<Vec<_>>());
            assert_ne!(model_code(info.id), "");
        }
        assert!(any_info(prowave_engine::native_effects()[0].id).is_some());
        assert!(any_info("prowave:nothing").is_none());
    }

    /// A voice read from a bank replaces the voice's parameters and leaves the others.
    #[test]
    fn a_bank_voice_replaces_the_voice_and_keeps_voices_and_level() {
        use prowave_engine::native_inst::fm6;
        let info = prowave_engine::native_instrument_info(fm6::ID).unwrap();
        assert!(loads_syx(info) && pages(info).len() == 9);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bank.syx");
        let mut file = vec![0xf0, 0x43, 0x00, 0x09, 0x20, 0x00];
        file.extend((0..4096u32).map(|k| (k % 50) as u8));
        file.extend([0x00, 0xf7]);
        std::fs::write(&path, &file).unwrap();
        let bank = read_bank(&path);
        assert_eq!(bank.len(), 32);
        assert!(read_bank(&dir.path().join("none.syx")).is_empty());
        let mut vals = info.defaults();
        vals[fm6::LEVEL] = -20.0;
        let changed = take_voice(&mut vals, &bank[3].1);
        assert!(!changed.is_empty() && changed.iter().all(|(k, _)| *k < fm6::VOICE_PARAMS));
        assert_eq!(&vals[..fm6::VOICE_PARAMS], &bank[3].1[..]);
        assert_eq!(vals[fm6::LEVEL], -20.0);
        assert!(take_voice(&mut vals, &bank[3].1).is_empty());
    }

    /// A place on Wave Draw's display is a point and a value; the edges stay in range.
    #[test]
    fn the_display_maps_the_pointer_to_a_point_and_a_value() {
        let glass = egui::Rect::from_min_size(pos2(100.0, 50.0), vec2(400.0, 200.0));
        assert_eq!(point_at(glass, pos2(100.0, 150.0), 200), (0, 0.0));
        assert_eq!(point_at(glass, pos2(300.0, 50.0), 200), (100, 100.0));
        assert_eq!(point_at(glass, pos2(499.0, 250.0), 4), (3, -100.0));
        // Outside: the nearest point, the nearest value.
        assert_eq!(point_at(glass, pos2(900.0, -40.0), 32), (31, 100.0));
        assert_eq!(point_at(glass, pos2(0.0, 900.0), 32), (0, -100.0));
        let info =
            prowave_engine::native_instrument_info(prowave_engine::native_inst::wave_draw::ID)
                .unwrap();
        assert!(draws_wave(info) && pages(info)[0].params.len() == 9);
    }

    /// A project whose one track plays `info` for a bar: a low C held for two beats.
    fn one_note(info: &NativeInstInfo) -> Project {
        use crate::model::{Note, Region, RegionParams, Track, TrackKind};
        let mut p = Project::empty();
        p.tempo = 120.0;
        p.master_vol = 1.0;
        p.tracks.clear();
        let mut t = Track::new("Synth", TrackKind::Instrument);
        t.au_instrument = Some(new_slot(info));
        t.regions.push(Region {
            missing: None,
            name: "Note".into(),
            start_bar: 1.0,
            len_bars: 1.0,
            notes: vec![Note {
                lyric: crate::lyrics::Lyric::NONE,
                mute: false,
                start: 0.0,
                len: 2.0,
                // A low C: a note every instrument plays (a drum machine's bass drum).
                pitch: 36,
                vel: 110,
            }],
            controls: Vec::new(),
            folder: None,
            audio: None,
            params: RegionParams::default(),
            generated: None,
        });
        p.tracks.push(t);
        p
    }

    /// Every native instrument is heard in a bounce: its note sounds from the bar's start.
    #[cfg(target_os = "macos")]
    #[test]
    fn every_native_instrument_is_in_the_bounce() {
        use crate::bounce::{run_blocking, Depth, Settings};
        for info in prowave_engine::native_instruments() {
            let dir = tempfile::tempdir().unwrap();
            let mut p = one_note(info);
            let out = dir.path().join("mix.wav");
            let settings = Settings {
                depth: Depth::Float32,
                tail_seconds: 0.5,
                max_tail_seconds: 0.5,
                ..Settings::mix_to(&out)
            };
            let mut host = crate::plugins::PluginHost::default();
            let r = run_blocking(&mut p, &mut host, 48_000, settings).unwrap();
            assert!(
                r.not_rendered.is_empty(),
                "{}: {:?}",
                info.id,
                r.not_rendered
            );
            let got = prowave_engine::AudioFile::open(&out)
                .unwrap()
                .read_stereo()
                .unwrap();
            assert!(got.iter().all(|v| v.is_finite()), "{}", info.id);
            // The first second (the note is held for it).
            let peak = got[..96_000].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.01, "{}: peak {peak}", info.id);
        }
    }

    /// A native instrument's settings come back from a saved project, and the instrument
    /// opens with them.
    #[test]
    fn settings_are_saved_and_opened_again() {
        let ctx = egui::Context::default();
        for info in prowave_engine::native_instruments() {
            let dir = tempfile::tempdir().unwrap();
            let mut p = one_note(info);
            // Every parameter away from its default: the top of its range, or the bottom.
            let v: Vec<f32> = info
                .params
                .iter()
                .map(|q| if q.default == q.max { q.min } else { q.max })
                .collect();
            p.tracks[0].au_instrument.as_mut().unwrap().state = Some(encode(&v));
            let path = crate::project_io::save_project(&p, &dir.path().join("inst")).unwrap();
            let (q, warnings) = crate::project_io::open_project(&path, &ctx).unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            let slot = q.tracks[0].au_instrument.as_ref().unwrap();
            assert_eq!((slot.id.as_str(), slot.name.as_str()), (info.id, info.name));
            let back = values(slot, info);
            assert_eq!(back, v, "{}", info.id);
            let inst = prowave_engine::open_native_instrument(info.id, 44100, &back).unwrap();
            for (k, x) in back.iter().enumerate() {
                assert_eq!(inst.param(k), *x, "{} {}", info.id, info.params[k].name);
            }
        }
    }

    /// The instrument's parameters are automation targets of slot 0, with the slot's values.
    #[test]
    fn parameters_are_automation_targets() {
        for info in prowave_engine::native_instruments() {
            let slot = new_slot(info);
            let descs = crate::automation::plugin_params(&slot, 0, None);
            assert_eq!(descs.len(), info.params.len(), "{}", info.id);
            assert_eq!(descs[0].name, info.params[0].name);
            let vals = crate::automation::plugin_values(&slot, 0);
            assert_eq!(vals.len(), info.params.len(), "{}", info.id);
        }
    }
}
