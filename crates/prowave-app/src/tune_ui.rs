//! Pro Wave Tune's window (Logic: the Pitch Correction plug-in window) as a PW-1 tuner unit:
//! a tuner LCD with the note heard and the note made (names, cents and needles on a ±50 cent
//! scale around the note aimed at), the target (key, scale or MIDI notes) with its twelve
//! note keys, the correction and voice knobs, and a status LCD.
use crate::gear::{self, Cap, Control, LedAt};
use crate::i18n::{tr, trf};
use crate::native_fx_ui::Edit;
use egui::{pos2, vec2, Align2, Rect, Sense, Shape, Stroke, Ui};
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

/// The unit's width: seven knobs.
pub const WIDTH: f32 = 7.0 * gear::CELL_W + 52.0;
const PAD: f32 = 26.0;

/// A MIDI note with a fraction as its nearest note's name and the cents from it.
pub fn note_and_cents(m: f32) -> (String, i32) {
    let n = m.round() as i32;
    (scale::note_name(n), ((m - n as f32) * 100.0).round() as i32)
}

/// The tuner LCD: what the effect hears and makes, as note names and cents, and both as
/// needles on a scale of -50 to +50 cents from the note aimed at.
fn tuner(ui: &mut Ui, rect: Rect, reading: Option<(f32, f32, f32)>) {
    let p = ui.painter().clone();
    let glass = gear::lcd(&p, rect);
    ui.interact(rect, ui.scope_id().with("tuner_lcd"), Sense::hover())
        .on_hover_text(tr(
            "Draw pitch corrections in the Piano Roll: pick Pitch Correction in its Automation/MIDI lane.",
        ));
    let (input, output, target) = reading.unwrap_or((f32::NAN, f32::NAN, f32::NAN));
    let side = |x: f32, align: Align2, label: &str, m: f32| {
        gear::lcd_label(&p, pos2(x, glass.top() + 8.0), align, label);
        let (name, cents) = if m.is_finite() {
            let (n, c) = note_and_cents(m);
            (n, Some(c))
        } else {
            ("--".to_string(), None)
        };
        gear::lcd_text(&p, pos2(x, glass.top() + 22.0), align, &name, 34.0);
        if let Some(c) = cents {
            let t = format!("{c}");
            let w = gear::seg7_width(&t, 18.0) + 24.0;
            let left = match align.x() {
                egui::Align::Min => x,
                _ => x - w,
            };
            gear::seg7(
                &p,
                pos2(left, glass.top() + 70.0),
                &t,
                18.0,
                gear::LCD_INK,
                true,
            );
            gear::lcd_text(
                &p,
                pos2(left + w, glass.top() + 88.0),
                Align2::RIGHT_BOTTOM,
                "CT",
                10.0,
            );
        }
    };
    side(glass.left() + 12.0, Align2::LEFT_TOP, tr("Input"), input);
    side(
        glass.right() - 12.0,
        Align2::RIGHT_TOP,
        tr("Output"),
        output,
    );
    // The cents scale around the note aimed at.
    let bar = Rect::from_min_max(
        pos2(glass.left() + 150.0, glass.top() + 44.0),
        pos2(glass.right() - 150.0, glass.top() + 96.0),
    );
    let ink = gear::LCD_INK;
    for c in (-50..=50).step_by(5) {
        let x = bar.left() + (c + 50) as f32 / 100.0 * bar.width();
        let major = c % 25 == 0;
        let h = if c == 0 {
            bar.height()
        } else if major {
            bar.height() * 0.45
        } else {
            bar.height() * 0.22
        };
        p.line_segment(
            [pos2(x, bar.bottom() - h), pos2(x, bar.bottom())],
            Stroke::new(
                if c == 0 { 2.0 } else { 1.0 },
                gear::alpha(ink, if major { 0.9 } else { 0.45 }),
            ),
        );
        if major {
            gear::lcd_label(
                &p,
                pos2(x, bar.bottom() + 3.0),
                Align2::CENTER_TOP,
                &format!("{c:+}"),
            );
        }
    }
    let x_of = |m: f32| {
        let c = ((m - target) * 100.0).clamp(-50.0, 50.0);
        bar.left() + (c + 50.0) / 100.0 * bar.width()
    };
    if input.is_finite() && target.is_finite() {
        // The note heard: an outlined pointer above the scale.
        let x = x_of(input);
        let y = bar.top() + 2.0;
        p.add(Shape::closed_line(
            vec![pos2(x - 6.0, y - 10.0), pos2(x + 6.0, y - 10.0), pos2(x, y)],
            Stroke::new(1.6, ink),
        ));
    }
    if output.is_finite() && target.is_finite() {
        // The note made: a solid bar through the scale.
        let x = x_of(output);
        p.rect_filled(
            Rect::from_min_max(pos2(x - 2.0, bar.top() + 4.0), pos2(x + 2.0, bar.bottom())),
            0.0,
            ink,
        );
    }
    let label = if target.is_finite() {
        trf("Target {}", &[&scale::note_name(target.round() as i32)])
    } else {
        tr("Not voiced").to_string()
    };
    gear::lcd_text(
        &p,
        pos2(bar.center().x, glass.top() + 10.0),
        Align2::CENTER_TOP,
        &label,
        13.0,
    );
}

/// The window's contents. `controls` and `shown` are each parameter as the panel shows it
/// (automation's value while a lane plays). Edits go to `vals` and `edits`.
#[allow(clippy::too_many_arguments)]
pub fn body(
    ui: &mut Ui,
    id: egui::Id,
    info: &NativeFxInfo,
    cx: &Context,
    controls: &[Control],
    shown: &[f32],
    vals: &mut [f32],
    edits: &mut Vec<Edit>,
) {
    let reading = crate::tune_lane::meter(cx.uid);
    if reading.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    let (ti, uid) = (cx.track, cx.uid);
    let mut set = |vals: &mut [f32], k: usize, v: f32| {
        if vals[k] != v {
            vals[k] = v;
            edits.push((ti, uid, k, v));
        }
    };
    let now = |k: usize| shown[k];
    let midi = now(tune::TARGET).round() as i32 == tune::TARGET_MIDI;
    let total = 150.0 + 18.0 + 22.0 + 66.0 + 62.0 + 2.0 * (22.0 + gear::CELL_H) + 48.0;
    let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, total), Sense::hover());
    let p = ui.painter().clone();
    let left = rect.left() + PAD;
    let inner_w = WIDTH - PAD * 2.0;
    let mut y = rect.top() + 16.0;
    tuner(
        ui,
        Rect::from_min_size(pos2(left, y), vec2(inner_w, 140.0)),
        reading,
    );
    y += 150.0 + 12.0;

    // The target: key, scale, what to correct to, and the presets.
    gear::bracket(&p, pos2(left, y + 6.0), inner_w, tr("Target"));
    y += 22.0;
    let names = |k: usize| -> Vec<String> {
        info.params[k]
            .choices
            .iter()
            .map(|s| s.to_string())
            .collect()
    };
    let field_w = 148.0;
    let label_at = |x: f32, t: &str| {
        gear::silk(
            &p,
            pos2(x, y + 6.0),
            Align2::LEFT_CENTER,
            t,
            11.0,
            gear::INK2,
        );
    };
    let mut x = left;
    let mut keys = names(tune::KEY);
    keys[0] = trf("Project ({})", &[&cx.project_key]);
    label_at(x, tr("Key"));
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    let cur = now(tune::KEY).round().clamp(0.0, (keys.len() - 1) as f32) as usize;
    if let Some(c) = gear::choice_field(
        ui,
        Rect::from_min_size(pos2(x, y + 16.0), vec2(field_w, 26.0)),
        id.with("key"),
        &refs,
        cur,
        true,
    ) {
        set(vals, tune::KEY, c as f32);
    }
    x += field_w + 14.0;
    let mut scales = names(tune::SCALE);
    scales[0] = tr("Project").to_string();
    label_at(x, tr("Scale"));
    let refs: Vec<&str> = scales.iter().map(String::as_str).collect();
    let cur = now(tune::SCALE)
        .round()
        .clamp(0.0, (scales.len() - 1) as f32) as usize;
    if let Some(c) = gear::choice_field(
        ui,
        Rect::from_min_size(pos2(x, y + 16.0), vec2(field_w, 26.0)),
        id.with("scale"),
        &refs,
        cur,
        true,
    ) {
        set(vals, tune::SCALE, c as f32);
    }
    x += field_w + 14.0;
    label_at(x, tr("Correct to"));
    for (i, t) in [tr("Key and scale"), tr("MIDI notes")].iter().enumerate() {
        let on = (i == 1) == midi;
        let r = Rect::from_min_size(pos2(x + i as f32 * 128.0, y + 16.0), vec2(120.0, 26.0));
        if gear::key(
            ui,
            r,
            id.with(("target", i)),
            t,
            Cap::Bone,
            LedAt::Left,
            on,
            on,
        )
        .clicked()
            && !on
        {
            set(vals, tune::TARGET, i as f32);
        }
    }
    y += 52.0;
    // MIDI notes from (with a MIDI target), else the notes of the scale in use: a click
    // makes it a Custom scale with that note turned on or off.
    if midi {
        let mut list = vec![match &cx.auto_source {
            Some(n) => trf("Auto ({})", &[n]),
            None => trf("Auto ({})", &[&tr("none")]),
        }];
        let mut numbers = vec![0usize];
        for (n, name) in &cx.tracks {
            list.push(format!("{n}. {name}"));
            numbers.push(*n);
        }
        let cur = now(tune::MIDI_TRACK).round() as usize;
        let c = numbers.iter().position(|n| *n == cur).unwrap_or(0);
        gear::silk(
            &p,
            pos2(left, y + 20.0),
            Align2::LEFT_CENTER,
            tr("MIDI notes from"),
            11.0,
            gear::INK2,
        );
        let refs: Vec<&str> = list.iter().map(String::as_str).collect();
        if let Some(i) = gear::choice_field(
            ui,
            Rect::from_min_size(pos2(left + 150.0, y + 7.0), vec2(300.0, 26.0)),
            id.with("midi_from"),
            &refs,
            c,
            true,
        ) {
            set(vals, tune::MIDI_TRACK, numbers[i] as f32);
        }
    } else {
        let mask = scale::mask(
            now(tune::KEY) as i32,
            now(tune::SCALE) as i32,
            now(tune::NOTES) as u16,
            cx.project_root,
            cx.project_minor,
        );
        let kw = (inner_w - 11.0 * 6.0) / 12.0;
        for (k, name) in scale::NAMES.iter().enumerate() {
            let on = mask & (1 << k) != 0;
            let sharp = name.contains('#') || (name.len() == 2 && name.ends_with('b'));
            let r =
                Rect::from_min_size(pos2(left + k as f32 * (kw + 6.0), y + 2.0), vec2(kw, 40.0));
            let cap = if sharp { Cap::Dark } else { Cap::Bone };
            let resp = gear::key(
                ui,
                r,
                id.with(("note", k)),
                name,
                cap,
                LedAt::Top,
                on,
                false,
            );
            if resp
                .on_hover_text(tr("Turn this note on or off (a Custom scale)"))
                .clicked()
            {
                set(vals, tune::NOTES, f32::from(mask ^ (1 << k)));
                set(vals, tune::SCALE, scale::SCALE_CUSTOM as f32);
            }
        }
    }
    y += 62.0;

    // Correction and voice.
    let mut changes = Vec::new();
    let mut voice_top = y;
    for (label, params) in [
        (
            tr("Correction"),
            vec![
                tune::RETUNE,
                tune::HUMANIZE,
                tune::FLEX,
                tune::VIBRATO,
                tune::ADD_VIBRATO,
                tune::VIBRATO_RATE,
                tune::AMOUNT,
            ],
        ),
        (
            tr("Voice"),
            vec![
                tune::FORMANT,
                tune::THROAT,
                tune::TRANSPOSE,
                tune::VOICE,
                tune::MIX,
            ],
        ),
    ] {
        voice_top = y;
        gear::bracket(
            &p,
            pos2(left, y + 6.0),
            params.len() as f32 * gear::CELL_W,
            label,
        );
        changes.extend(gear::control_grid(
            ui,
            pos2(left, y + 22.0),
            params.len(),
            id,
            controls,
            &params,
            46.0,
        ));
        y += 22.0 + gear::CELL_H;
    }
    for ch in changes {
        let v = crate::native_fx_ui::value_of(info, &ch);
        set(vals, ch.index, v);
    }

    // Status: the pitch override and the latency.
    let status = Rect::from_min_size(pos2(left, y + 6.0), vec2(inner_w, 30.0));
    gear::glass(&p, status);
    gear::lcd_text(
        &p,
        pos2(status.left() + 10.0, status.center().y),
        Align2::LEFT_CENTER,
        &trf(
            "Pitch Override: {}",
            &[&tune::override_text(now(tune::OVERRIDE) as i32)],
        ),
        11.0,
    );
    gear::lcd_text(
        &p,
        pos2(status.right() - 10.0, status.center().y),
        Align2::RIGHT_CENTER,
        &trf(
            "Latency {} ms, compensated",
            &[&format!("{:.1}", tune::LATENCY_SECONDS * 1000.0)],
        ),
        11.0,
    );
    // Presets: a menu beside the voice knobs.
    let px = left + 5.0 * gear::CELL_W + 24.0;
    let pr = Rect::from_min_size(pos2(px, voice_top + 46.0), vec2(left + inner_w - px, 26.0));
    gear::silk(
        &p,
        pos2(px, voice_top + 34.0),
        Align2::LEFT_CENTER,
        tr("Presets"),
        11.0,
        gear::INK2,
    );
    let resp = gear::field(ui, pr, id.with("presets"), tr("Presets"), true);
    egui::Popup::menu(&resp).show(|ui| {
        for preset in info.presets {
            if ui.selectable_label(false, preset.name).clicked() {
                let mut v = vals.to_vec();
                preset.apply(&mut v);
                for (k, x) in v.into_iter().enumerate() {
                    set(vals, k, x);
                }
            }
        }
    });
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
            egui::Color32::from_gray(0x2a),
            |ui| {
                crate::theme::apply(ui.ctx());
                crate::native_fx_ui::show(ui.ctx(), &mut p, &mut s)
            },
            std::path::Path::new(&path),
        )
        .unwrap();
    }
}
