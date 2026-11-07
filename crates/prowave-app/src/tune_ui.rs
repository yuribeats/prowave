//! Pro Wave Tune's window (Logic: the Pitch Correction plug-in window): presets, the key and
//! scale (or the MIDI target), the correction and voice controls, and a live meter of the
//! pitch heard and made, with its note name and cents.
use crate::i18n::{tr, trf};
use crate::native_fx_ui::Edit;
use crate::widgets::{BLUE, BTN_BG, TEXT, TEXT_DIM};
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui};
use prowave_engine::tune::{self, scale};
use prowave_engine::NativeFxInfo;

/// What the window needs of the project, read before it opens.
pub struct Context {
    pub track: usize,
    pub uid: u64,
    /// The project's key, as the Key and Scale "Project" choices read it ("E minor"), and
    /// its root (C = 0) and mode.
    pub project_key: String,
    pub project_root: u8,
    pub project_minor: bool,
    /// The tracks a MIDI target can be: (number from 1, name).
    pub tracks: Vec<(usize, String)>,
    /// The track Auto follows, by name.
    pub auto_source: Option<String>,
}

const PANEL: Color32 = Color32::from_rgb(0x24, 0x24, 0x24);
const NEEDLE_IN: Color32 = Color32::from_rgb(0xff, 0xb0, 0x40);
const NEEDLE_OUT: Color32 = Color32::from_rgb(0x50, 0xd8, 0xff);

/// A MIDI note with a fraction as its nearest note's name and the cents from it.
pub fn note_and_cents(m: f32) -> (String, i32) {
    let n = m.round() as i32;
    (scale::note_name(n), ((m - n as f32) * 100.0).round() as i32)
}

/// The meter: what the effect hears (orange) and makes (blue), as note names and cents, and
/// both as needles on a scale of -50 to +50 cents from the note aimed at.
fn meter(ui: &mut Ui, reading: Option<(f32, f32, f32)>) {
    let w = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 84.0), Sense::hover());
    let g = ui.painter_at(rect);
    g.rect_filled(rect, 4.0, PANEL);
    let (input, output, target) = reading.unwrap_or((f32::NAN, f32::NAN, f32::NAN));
    let big = FontId::proportional(22.0);
    let small = FontId::proportional(11.0);
    let side = |x: f32, align: Align2, label: &str, m: f32, colour: Color32| {
        g.text(
            pos2(x, rect.top() + 8.0),
            align,
            label,
            small.clone(),
            TEXT_DIM,
        );
        let (name, cents) = if m.is_finite() {
            let (n, c) = note_and_cents(m);
            (n, format!("{c:+} ct"))
        } else {
            ("—".to_string(), String::new())
        };
        g.text(pos2(x, rect.top() + 24.0), align, name, big.clone(), colour);
        g.text(
            pos2(x, rect.top() + 52.0),
            align,
            cents,
            small.clone(),
            TEXT,
        );
    };
    side(
        rect.left() + 12.0,
        Align2::LEFT_TOP,
        tr("Input"),
        input,
        NEEDLE_IN,
    );
    side(
        rect.right() - 12.0,
        Align2::RIGHT_TOP,
        tr("Output"),
        output,
        NEEDLE_OUT,
    );
    // The cents scale, around the note aimed at.
    let bar = Rect::from_min_max(
        pos2(rect.left() + 90.0, rect.top() + 30.0),
        pos2(rect.right() - 90.0, rect.top() + 58.0),
    );
    g.rect_filled(bar, 2.0, Color32::from_gray(0x18));
    for c in [-50, -25, 0, 25, 50] {
        let x = bar.left() + (c + 50) as f32 / 100.0 * bar.width();
        let h = if c == 0 {
            bar.height()
        } else {
            bar.height() * 0.4
        };
        g.line_segment(
            [pos2(x, bar.bottom() - h), pos2(x, bar.bottom())],
            Stroke::new(1.0, Color32::from_gray(if c == 0 { 0xc0 } else { 0x60 })),
        );
        g.text(
            pos2(x, bar.bottom() + 2.0),
            Align2::CENTER_TOP,
            format!("{c:+}"),
            FontId::proportional(9.5),
            TEXT_DIM,
        );
    }
    let needle = |m: f32, colour: Color32, width: f32| {
        if m.is_finite() && target.is_finite() {
            let c = ((m - target) * 100.0).clamp(-50.0, 50.0);
            let x = bar.left() + (c + 50.0) / 100.0 * bar.width();
            g.line_segment(
                [pos2(x, bar.top() + 2.0), pos2(x, bar.bottom() - 2.0)],
                Stroke::new(width, colour),
            );
        }
    };
    needle(input, NEEDLE_IN, 3.0);
    needle(output, NEEDLE_OUT, 2.0);
    let label = if target.is_finite() {
        trf("Target {}", &[&scale::note_name(target.round() as i32)])
    } else {
        tr("Not voiced").to_string()
    };
    g.text(
        pos2(bar.center().x, bar.top() - 4.0),
        Align2::CENTER_BOTTOM,
        label,
        FontId::proportional(11.0),
        TEXT,
    );
}

/// The window's contents. `played(k)` is what automation plays for parameter `k` now (if it
/// does); `led(k)` draws its automation light. Edits go to `vals` and `edits`.
#[allow(clippy::too_many_arguments)]
pub fn body(
    ui: &mut Ui,
    info: &NativeFxInfo,
    cx: &Context,
    vals: &mut [f32],
    edits: &mut Vec<Edit>,
    played: &dyn Fn(usize) -> Option<f32>,
    led: &dyn Fn(&mut Ui, usize),
) {
    let reading = crate::tune_lane::meter(cx.uid);
    if reading.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    let (ti, uid) = (cx.track, cx.uid);
    let set = |vals: &mut [f32], edits: &mut Vec<Edit>, k: usize, v: f32| {
        if vals[k] != v {
            vals[k] = v;
            edits.push((ti, uid, k, v));
        }
    };
    let now = |vals: &[f32], k: usize| played(k).unwrap_or(vals[k]);
    // Presets and Reset.
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(("tune_preset", uid))
            .selected_text(tr("Presets"))
            .width(180.0)
            .show_ui(ui, |ui| {
                for preset in info.presets {
                    if ui.selectable_label(false, preset.name).clicked() {
                        let mut v = vals.to_vec();
                        preset.apply(&mut v);
                        for (k, x) in v.into_iter().enumerate() {
                            set(vals, edits, k, x);
                        }
                    }
                }
            });
        if ui.button(tr("Reset")).clicked() {
            for (k, d) in info.defaults().into_iter().enumerate() {
                set(vals, edits, k, d);
            }
        }
    });
    ui.add_space(4.0);
    meter(ui, reading);
    ui.add_space(6.0);
    let p = info.params;
    let choice =
        |ui: &mut Ui, vals: &mut [f32], edits: &mut Vec<Edit>, k: usize, names: &[String]| {
            let mut c = now(vals, k).round().clamp(0.0, (names.len() - 1) as f32) as usize;
            let before = c;
            egui::ComboBox::from_id_salt(("tune_choice", uid, k))
                .selected_text(names[c].clone())
                .width(170.0)
                .show_ui(ui, |ui| {
                    for (n, name) in names.iter().enumerate() {
                        ui.selectable_value(&mut c, n, name);
                    }
                });
            if c != before {
                set(vals, edits, k, c as f32);
            }
        };
    let names = |k: usize| -> Vec<String> { p[k].choices.iter().map(|s| s.to_string()).collect() };
    ui.label(RichText::new(tr("Target")).strong().color(TEXT));
    egui::Grid::new(("tune_target", uid))
        .num_columns(3)
        .min_col_width(96.0)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            let mut keys = names(tune::KEY);
            keys[0] = trf("Project ({})", &[&cx.project_key]);
            ui.label(tr("Key"));
            choice(ui, vals, edits, tune::KEY, &keys);
            led(ui, tune::KEY);
            ui.end_row();
            let mut scales = names(tune::SCALE);
            scales[0] = tr("Project").to_string();
            ui.label(tr("Scale"));
            choice(ui, vals, edits, tune::SCALE, &scales);
            led(ui, tune::SCALE);
            ui.end_row();
            ui.label(tr("Correct to"));
            choice(
                ui,
                vals,
                edits,
                tune::TARGET,
                &[
                    tr("Key and scale").to_string(),
                    tr("MIDI notes").to_string(),
                ],
            );
            led(ui, tune::TARGET);
            ui.end_row();
            if now(vals, tune::TARGET).round() as i32 == tune::TARGET_MIDI {
                let mut list = vec![match &cx.auto_source {
                    Some(n) => trf("Auto ({})", &[n]),
                    None => trf("Auto ({})", &[&tr("none")]),
                }];
                let mut numbers = vec![0usize];
                for (n, name) in &cx.tracks {
                    list.push(format!("{n}. {name}"));
                    numbers.push(*n);
                }
                let cur = now(vals, tune::MIDI_TRACK).round() as usize;
                let mut c = numbers.iter().position(|n| *n == cur).unwrap_or(0);
                let before = c;
                ui.label(tr("MIDI notes from"));
                egui::ComboBox::from_id_salt(("tune_midi", uid))
                    .selected_text(list[c].clone())
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for (i, name) in list.iter().enumerate() {
                            ui.selectable_value(&mut c, i, name);
                        }
                    });
                if c != before {
                    set(vals, edits, tune::MIDI_TRACK, numbers[c] as f32);
                }
                ui.end_row();
            }
        });
    // The notes of the scale in use; a click makes it a Custom scale with that note turned
    // on or off.
    ui.add_space(4.0);
    let mask = {
        let (root, minor) = (cx.project_root, cx.project_minor);
        scale::mask(
            now(vals, tune::KEY) as i32,
            now(vals, tune::SCALE) as i32,
            now(vals, tune::NOTES) as u16,
            root,
            minor,
        )
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (k, name) in scale::NAMES.iter().enumerate() {
            let on = mask & (1 << k) != 0;
            let (bg, fg) = if on {
                (BLUE, Color32::WHITE)
            } else {
                (BTN_BG, Color32::from_gray(0xd0))
            };
            let b = egui::Button::new(RichText::new(*name).color(fg).size(12.0))
                .fill(bg)
                .min_size(vec2(30.0, 22.0));
            if ui
                .add(b)
                .on_hover_text(tr("Turn this note on or off (a Custom scale)"))
                .clicked()
            {
                set(vals, edits, tune::NOTES, f32::from(mask ^ (1 << k)));
                set(vals, edits, tune::SCALE, scale::SCALE_CUSTOM as f32);
            }
        }
    });
    ui.add_space(8.0);
    let knob = |ui: &mut Ui, vals: &mut [f32], edits: &mut Vec<Edit>, k: usize| {
        let param = &p[k];
        ui.label(param.name);
        if param.choices.is_empty() {
            let mut v = now(vals, k);
            let slider = egui::Slider::new(&mut v, param.min..=param.max)
                .show_value(false)
                .step_by(1.0);
            if ui.add_sized([170.0, 18.0], slider).changed() {
                set(vals, edits, k, v.round());
            }
        } else {
            choice(ui, vals, edits, k, &names(k));
        }
        ui.horizontal(|ui| {
            if param.choices.is_empty() {
                ui.label(RichText::new(info.value_text(k, now(vals, k))).color(TEXT));
            }
            led(ui, k);
        });
        ui.end_row();
    };
    ui.label(RichText::new(tr("Correction")).strong().color(TEXT));
    egui::Grid::new(("tune_correction", uid))
        .num_columns(3)
        .min_col_width(96.0)
        .spacing([10.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for k in [
                tune::RETUNE,
                tune::HUMANIZE,
                tune::FLEX,
                tune::VIBRATO,
                tune::ADD_VIBRATO,
                tune::VIBRATO_RATE,
                tune::AMOUNT,
            ] {
                knob(ui, vals, edits, k);
            }
        });
    ui.add_space(6.0);
    ui.label(RichText::new(tr("Voice")).strong().color(TEXT));
    egui::Grid::new(("tune_voice", uid))
        .num_columns(3)
        .min_col_width(96.0)
        .spacing([10.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for k in [
                tune::FORMANT,
                tune::THROAT,
                tune::TRANSPOSE,
                tune::VOICE,
                tune::MIX,
            ] {
                knob(ui, vals, edits, k);
            }
        });
    ui.add_space(6.0);
    ui.separator();
    ui.label(
        RichText::new(trf(
            "Pitch Override: {}",
            &[&tune::override_text(now(vals, tune::OVERRIDE) as i32)],
        ))
        .color(TEXT),
    );
    ui.label(
        RichText::new(trf(
            "Latency {} ms, compensated",
            &[&format!("{:.1}", tune::LATENCY_SECONDS * 1000.0)],
        ))
        .color(TEXT_DIM),
    );
    ui.label(
        RichText::new(tr(
            "Draw pitch corrections in the Piano Roll: pick Pitch Correction in its Automation/MIDI lane.",
        ))
        .color(TEXT_DIM),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Project, TrackKind};

    fn tune_project() -> (Project, crate::UiState, u64, usize) {
        let mut p = Project::empty();
        let i = p.add_track(TrackKind::Audio);
        p.tracks[i].name = "Lead Vocal".into();
        let info = prowave_engine::native_info(tune::ID).unwrap();
        let mut slot = crate::native_fx_ui::new_slot(info);
        if slot.uid == 0 {
            slot.uid = 4242;
        }
        let uid = slot.uid;
        p.tracks[i].au_effects.push(slot);
        let mut s = crate::UiState::new();
        s.native_window = Some(uid);
        (p, s, uid, i)
    }

    #[test]
    fn notes_and_cents_read_from_a_pitch() {
        assert_eq!(note_and_cents(69.12), ("A3".to_string(), 12));
        assert_eq!(note_and_cents(60.6), ("C#3".to_string(), -40));
    }

    /// The window draws offscreen (nothing on the screen), and a preset picked in it reaches
    /// the slot and the engine edits.
    #[test]
    fn the_window_draws_and_presets_reach_the_slot() {
        let (mut p, mut s, uid, i) = tune_project();
        let mut h = crate::ui_test::Harness::new(560.0, 1100.0);
        h.settle(|ui| crate::native_fx_ui::show(ui.ctx(), &mut p, &mut s));
        assert_eq!(s.native_window, Some(uid), "the window stays open");
        // Load the Hard Tune preset the way the menu does.
        let info = prowave_engine::native_info(tune::ID).unwrap();
        let slot = &mut p.tracks[i].au_effects[0];
        let mut v = crate::native_fx_ui::values(slot, info);
        info.presets[0].apply(&mut v);
        slot.state = Some(crate::native_fx_ui::encode(&v));
        let back = crate::native_fx_ui::values(&p.tracks[i].au_effects[0], info);
        assert_eq!(back[tune::RETUNE], 0.0);
        assert_eq!(back[tune::VIBRATO], 0.0);
    }

    /// A picture of the window, to look at: `PROWAVE_TUNE_PNG=/path/tune.png cargo test -p
    /// prowave-app tune_window_picture -- --ignored`.
    #[test]
    #[ignore = "picture"]
    fn tune_window_picture() {
        let (mut p, mut s, uid, _) = tune_project();
        // A running effect that has heard an A3 sung 30 cents flat, for the meter.
        let (mut fx, remote) = prowave_engine::open_tune_block(48000, &[]);
        let x: Vec<f32> = (0..24000)
            .map(|i| {
                let ph = i as f32 * 216.3 / 48000.0;
                (1..20)
                    .map(|k| (std::f32::consts::TAU * k as f32 * ph).sin() / k as f32)
                    .sum::<f32>()
                    * 0.2
            })
            .collect();
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        for c in x.as_chunks::<64>().0 {
            fx.process((&c[..], &c[..]), (&mut l, &mut r));
        }
        crate::tune_lane::attach(uid, remote);
        let path = std::env::var("PROWAVE_TUNE_PNG").unwrap_or_else(|_| {
            std::env::temp_dir()
                .join("prowave-tune.png")
                .to_string_lossy()
                .into_owned()
        });
        crate::ui_test::render_png(
            560.0,
            1000.0,
            1.0,
            Color32::from_gray(0x2a),
            |ui| {
                crate::theme::apply(ui.ctx());
                crate::native_fx_ui::show(ui.ctx(), &mut p, &mut s)
            },
            std::path::Path::new(&path),
        )
        .unwrap();
    }
}
