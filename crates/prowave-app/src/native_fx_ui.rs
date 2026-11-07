//! The native effects (ports of Lemon Studio's recreations of FL Studio's Fruity Limiter,
//! Maximus, Transient Processor, Soft Clipper and Fast Dist) in the Audio FX slots: their
//! settings, kept in the slot, and their window of controls.
use crate::i18n::tr;
use crate::model::Project;
use crate::plugins::PluginSlot;
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
    // LEDs: red while an envelope plays, grey when overridden by hand.
    let beat = p.position_beats;
    let auto = p.strip_auto(ti).cloned().unwrap_or_default();
    let target = |k: usize| prowave_engine::AutoTarget::Plugin {
        slot: j as u8 + 1,
        param: k as u32,
    };
    let mut open = true;
    let mut edits: Vec<Edit> = Vec::new();
    let title = format!("{} — {}", info.name, p.strip_name(ti));
    let tune_cx = (info.id == prowave_engine::tune::ID).then(|| tune_context(p, ti, uid, &vals));
    let mut window = egui::Window::new(title)
        .id(egui::Id::unique("native_fx").with(uid))
        .open(&mut open)
        .default_width(380.0)
        .vscroll(true);
    if tune_cx.is_some() {
        window = window.default_width(540.0).default_height(900.0);
    }
    window.show(ctx, |ui| {
        if let Some(cx) = &tune_cx {
            let played = |k: usize| {
                let param = info.params.get(k)?;
                auto.played(target(k), beat)
                    .map(|n| (param.min + (param.max - param.min) * n.clamp(0.0, 1.0)).round())
            };
            let led = |ui: &mut egui::Ui, k: usize| {
                if !auto.lane(target(k)).is_empty() {
                    let (dot, tip) = if auto.is_overridden(target(k)) {
                        (crate::automation_ui::LED_OFF, tr("Overridden"))
                    } else {
                        (crate::automation_ui::LED_ON, tr("Automated"))
                    };
                    ui.label(egui::RichText::new("•").color(dot))
                        .on_hover_text(tip);
                }
            };
            crate::tune_ui::body(ui, info, cx, &mut vals, &mut edits, &played, &led);
            return;
        }
        egui::Grid::new(("native_grid", uid))
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                for (k, param) in info.params.iter().enumerate() {
                    ui.label(param.name);
                    let played = auto.played(target(k), beat).map(|n| {
                        let x = param.min + (param.max - param.min) * n.clamp(0.0, 1.0);
                        if param.integer {
                            x.round()
                        } else {
                            x
                        }
                    });
                    let mut v = played.unwrap_or(vals[k]);
                    let changed = if param.choices.is_empty() {
                        let slider = egui::Slider::new(&mut v, param.min..=param.max)
                            .show_value(false)
                            .step_by(if param.integer { 1.0 } else { 0.0 });
                        ui.add_enabled(param.live, slider).changed()
                    } else {
                        let mut c = v.round().clamp(0.0, (param.choices.len() - 1) as f32) as usize;
                        let before = c;
                        egui::ComboBox::from_id_salt(("native_choice", uid, k))
                            .selected_text(param.choices[c])
                            .show_ui(ui, |ui| {
                                for (n, name) in param.choices.iter().enumerate() {
                                    ui.selectable_value(&mut c, n, *name);
                                }
                            });
                        v = c as f32;
                        c != before
                    };
                    ui.horizontal(|ui| {
                        ui.label(info.value_text(k, v));
                        if !auto.lane(target(k)).is_empty() {
                            let (dot, tip) = if auto.is_overridden(target(k)) {
                                (crate::automation_ui::LED_OFF, tr("Overridden"))
                            } else {
                                (crate::automation_ui::LED_ON, tr("Automated"))
                            };
                            ui.label(egui::RichText::new("•").color(dot))
                                .on_hover_text(tip);
                        }
                    });
                    ui.end_row();
                    if changed {
                        vals[k] = v;
                        edits.push((ti, uid, k, v));
                    }
                }
            });
        ui.separator();
        if ui.button(tr("Reset")).clicked() {
            for (k, d) in info.defaults().into_iter().enumerate() {
                vals[k] = d;
                edits.push((ti, uid, k, d));
            }
        }
    });
    if !edits.is_empty() {
        if let Some(e) = p.strip_plugins_mut(ti).and_then(|(fx, _)| fx.get_mut(j)) {
            e.state = Some(encode(&vals));
        }
        s.native_edits.extend(edits);
    }
    if !open {
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
