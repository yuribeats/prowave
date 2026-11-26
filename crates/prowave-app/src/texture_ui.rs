//! Pro Wave Texture's window as a PW-1 unit: a buffer LCD (the recording, now on the left,
//! oldest on the right, one tick a second; the grain at TIME drawn with the SHAPE envelope
//! over the SIZE it lasts, and the band TIME RANDOM can reach), the grain knobs with their
//! random amounts under them, DENSITY, the FREEZE key, the mix knobs, QUALITY and presets.
use crate::gear::{self, Cap, Change, Control, LedAt};
use crate::i18n::tr;
use crate::native_fx_ui::Edit;
use egui::{pos2, vec2, Align2, Rect, Sense, Shape, Stroke, Ui};
use prowave_engine::texture as tx;
use prowave_engine::NativeFxInfo;

const PAD: f32 = 26.0;
/// Columns of knob cells (and the quarter-cell gaps between sections).
const COLUMNS: f32 = 6.5;
/// The unit's width.
pub const WIDTH: f32 = COLUMNS * gear::CELL_W + 2.0 * PAD;
const DISPLAY_H: f32 = 118.0;
const ROW_H: f32 = 22.0 + gear::CELL_H + 6.0;

/// The panel's label for parameter `k` (translated).
fn label(k: usize) -> String {
    match k {
        tx::TIME | tx::TIME_RANDOM => tr("Time"),
        tx::SIZE | tx::SIZE_RANDOM => tr("Size"),
        tx::SHAPE | tx::SHAPE_RANDOM => tr("Shape"),
        tx::PITCH | tx::PITCH_RANDOM => tr("Pitch"),
        tx::DENSITY => tr("Density"),
        tx::FEEDBACK => tr("Feedback"),
        tx::DRY_WET => tr("Dry/Wet"),
        tx::REVERB => tr("Reverb"),
        tx::FREEZE => tr("Freeze"),
        _ => tr("Quality"),
    }
    .to_string()
}

/// QUALITY's choices, translated.
fn quality_names() -> Vec<String> {
    [
        tr("Digital"),
        tr("Vintage"),
        tr("Clean Tape"),
        tr("Dirty Tape"),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The FREEZE key: a latching key whose LED is lit while the buffer is frozen.
fn freeze_cell(ui: &mut Ui, rect: Rect, id: egui::Id, c: &Control) -> Option<Change> {
    let p = ui.painter().clone();
    let lr = gear::text_at(
        &p,
        pos2(rect.center().x, rect.top() + 9.0),
        Align2::CENTER_CENTER,
        &c.label.to_uppercase(),
        gear::silk_font(11.5),
        gear::INK2(),
        0.12,
    );
    if let Some(on) = c.automated {
        gear::led(&p, pos2(lr.right() + 7.0, lr.center().y), 2.5, on);
    }
    let on = c.choice == 1;
    let key = Rect::from_center_size(pos2(rect.center().x, rect.top() + 58.0), vec2(64.0, 52.0));
    let text = if on { tr("On") } else { tr("Off") };
    let resp = gear::key(
        ui,
        key,
        id.with(("freeze", tx::FREEZE)),
        text,
        Cap::Bone,
        LedAt::Top,
        on,
        on,
    );
    resp.clicked().then(|| Change {
        index: tx::FREEZE,
        norm: if on { 0.0 } else { 1.0 },
        choice: Some(usize::from(!on)),
        ended: true,
    })
}

/// The buffer LCD for the settings `v` (stored values).
fn buffer_display(p: &egui::Painter, rect: Rect, info: &NativeFxInfo, v: &[f32]) {
    let glass = gear::lcd(p, rect);
    let ink = gear::LCD_INK();
    let at = |k: usize| v[k].round() as i32;
    let seconds = tx::media_of(at(tx::QUALITY)).buffer_seconds();

    // Readouts: the buffer's length, and the grain's.
    gear::lcd_label(
        p,
        glass.left_top() + vec2(8.0, 6.0),
        Align2::LEFT_TOP,
        tr("Buffer"),
    );
    let t = format!("{seconds:.1}");
    let w = gear::seg7(p, glass.left_top() + vec2(8.0, 20.0), &t, 20.0, ink, true);
    gear::lcd_text(
        p,
        glass.left_top() + vec2(12.0 + w, 40.0),
        Align2::LEFT_BOTTOM,
        "s",
        9.0,
    );
    gear::lcd_label(
        p,
        pos2(glass.right() - 8.0, glass.top() + 6.0),
        Align2::RIGHT_TOP,
        tr("Size"),
    );
    let (num, unit) = gear::split_value(&info.value_text(tx::SIZE, v[tx::SIZE]));
    let mut x = glass.right() - 8.0;
    if !unit.is_empty() {
        let r = gear::lcd_text(
            p,
            pos2(x, glass.top() + 40.0),
            Align2::RIGHT_BOTTOM,
            &unit,
            if num.is_empty() { 14.0 } else { 9.0 },
        );
        x = r.left() - 4.0;
    }
    if !num.is_empty() {
        let w = gear::seg7_width(&num, 20.0);
        gear::seg7(p, pos2(x - w, glass.top() + 20.0), &num, 20.0, ink, true);
    }

    // The timeline: now on the left, the oldest recording on the right.
    let plot = Rect::from_min_max(
        pos2(glass.left() + 10.0, glass.top() + 48.0),
        pos2(glass.right() - 10.0, glass.bottom() - 8.0),
    );
    let faint = Stroke::new(1.0, gear::alpha(ink, 0.18));
    p.line_segment([plot.left_bottom(), plot.right_bottom()], faint);
    let whole = seconds.floor() as i32;
    for s in 0..=whole {
        let x = plot.left() + plot.width() * s as f32 / seconds;
        let h = if s % 4 == 0 { 7.0 } else { 4.0 };
        p.line_segment([pos2(x, plot.bottom()), pos2(x, plot.bottom() - h)], faint);
    }
    let x_of = |pos: f32| plot.left() + plot.width() * pos.clamp(0.0, 1.0);

    let Some(grain) = tx::grain_seconds(at(tx::SIZE)) else {
        // Delay mode: a line at each repeat (the delay, then FEEDBACK's repeats of it).
        let (repeat, _) = tx::delay_repeat(at(tx::QUALITY), at(tx::DENSITY), at(tx::TIME));
        let count = ((seconds / repeat.max(1e-3)) as usize).min(64);
        for k in 1..=count {
            let x = plot.left() + plot.width() * (k as f32 * repeat / seconds);
            let a = if k == 1 {
                1.0
            } else {
                0.55 * 0.85f32.powi(k as i32 - 2)
            };
            p.line_segment(
                [pos2(x, plot.top()), pos2(x, plot.bottom())],
                Stroke::new(if k == 1 { 2.0 } else { 1.5 }, gear::alpha(ink, a)),
            );
        }
        return;
    };
    let time = (v[tx::TIME] / 1000.0).clamp(0.0, 1.0);
    // How far TIME RANDOM reaches: uniform draws for a positive amount, rarer jumps up to
    // three times as far for a negative one.
    let amount = tx::attenuverter_law(v[tx::TIME_RANDOM] / 1000.0);
    let reach = if amount >= 0.0 {
        amount
    } else {
        (-3.0 * amount).min(1.0)
    };
    if reach > 0.0 {
        let band = Rect::from_min_max(
            pos2(x_of(time - time * reach), plot.top()),
            pos2(x_of(time + (1.0 - time) * reach), plot.bottom()),
        );
        p.rect_filled(band, 0.0, gear::alpha(ink, 0.10));
    }
    // The grain: its envelope over the time it lasts, from TIME.
    let width = (plot.width() * grain.abs() / seconds).max(6.0);
    let x0 = x_of(time);
    let env = tx::envelope_points(at(tx::SHAPE), 96);
    let n = env.len().max(2) - 1;
    let mut pts: Vec<egui::Pos2> = env
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let f = i as f32 / n as f32;
            // A reversed grain plays toward now.
            let f = if grain < 0.0 { 1.0 - f } else { f };
            pos2(x0 + f * width, plot.bottom() - e * plot.height())
        })
        .collect();
    if grain < 0.0 {
        pts.reverse();
    }
    let clip = plot.expand2(vec2(0.0, 2.0));
    let mut fill = pts.clone();
    fill.push(pos2(pts.last().map_or(x0, |q| q.x), plot.bottom()));
    fill.push(pos2(pts.first().map_or(x0, |q| q.x), plot.bottom()));
    p.with_clip_rect(clip).add(Shape::convex_polygon(
        fill,
        gear::alpha(ink, 0.22),
        Stroke::NONE,
    ));
    p.with_clip_rect(clip)
        .add(Shape::line(pts, Stroke::new(2.0, ink)));
    p.line_segment(
        [pos2(x0, plot.top()), pos2(x0, plot.bottom())],
        Stroke::new(1.0, gear::alpha(ink, 0.5)),
    );
}

/// One row: sections of controls side by side, each with its bracket label.
fn row(
    ui: &mut Ui,
    left: f32,
    y: f32,
    id: egui::Id,
    sections: &[(String, Vec<usize>)],
    controls: &[Control],
) -> Vec<Change> {
    let p = ui.painter().clone();
    let mut out = Vec::new();
    let mut x = left;
    for (name, params) in sections {
        let w = params.len() as f32 * gear::CELL_W;
        if !name.is_empty() {
            gear::bracket(&p, pos2(x + 6.0, y + 8.0), w - 12.0, name);
        }
        for (n, k) in params.iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(x + n as f32 * gear::CELL_W, y + 22.0),
                vec2(gear::CELL_W, gear::CELL_H),
            );
            let Some(c) = controls.get(*k) else { continue };
            let ch = if *k == tx::FREEZE {
                freeze_cell(ui, r, id, c)
            } else {
                gear::control_cell(ui, r, id, *k, c, 46.0)
            };
            out.extend(ch);
        }
        x += w + gear::CELL_W * 0.25;
    }
    out
}

/// The window's contents. `controls` and `shown` are each parameter as the panel shows it
/// (automation's value while a lane plays). Edits go to `vals` and `edits`.
#[allow(clippy::too_many_arguments)]
pub fn body(
    ui: &mut Ui,
    id: egui::Id,
    info: &NativeFxInfo,
    track: usize,
    uid: u64,
    controls: &[Control],
    shown: &[f32],
    vals: &mut [f32],
    edits: &mut Vec<Edit>,
) {
    let mut set = |vals: &mut [f32], k: usize, v: f32| {
        if vals[k] != v {
            vals[k] = v;
            edits.push((track, uid, k, v));
        }
    };
    let mut controls = controls.to_vec();
    for (k, c) in controls.iter_mut().enumerate() {
        c.label = label(k);
    }
    if let Some(c) = controls.get_mut(tx::QUALITY) {
        c.choices = quality_names();
    }
    // In the delay mode DENSITY sets the time between repeats and TIME multiplies it.
    let at = |k: usize| shown[k].round() as i32;
    if tx::grain_seconds(at(tx::SIZE)).is_none() {
        let (repeat, multiplier) = tx::delay_repeat(at(tx::QUALITY), at(tx::DENSITY), at(tx::TIME));
        controls[tx::DENSITY].text = if repeat < 1.0 {
            format!("{:.0} ms", repeat * 1000.0)
        } else {
            format!("{repeat:.2} s")
        };
        controls[tx::TIME].text = format!("x{multiplier}");
    }

    let total = 14.0 + DISPLAY_H + 10.0 + 3.0 * ROW_H + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, total), Sense::hover());
    let p = ui.painter().clone();
    let left = rect.left() + PAD;
    let inner_w = WIDTH - 2.0 * PAD;
    let mut y = rect.top() + 14.0;
    buffer_display(
        &p,
        Rect::from_min_size(pos2(left, y), vec2(inner_w, DISPLAY_H)),
        info,
        shown,
    );
    y += DISPLAY_H + 10.0;

    let mut changes = Vec::new();
    changes.extend(row(
        ui,
        left,
        y,
        id,
        &[
            (
                tr("Grain").to_string(),
                vec![tx::TIME, tx::SIZE, tx::SHAPE, tx::PITCH],
            ),
            (String::new(), vec![tx::DENSITY]),
            (String::new(), vec![tx::FREEZE]),
        ],
        &controls,
    ));
    y += ROW_H;
    changes.extend(row(
        ui,
        left,
        y,
        id,
        &[(
            tr("Random").to_string(),
            vec![
                tx::TIME_RANDOM,
                tx::SIZE_RANDOM,
                tx::SHAPE_RANDOM,
                tx::PITCH_RANDOM,
            ],
        )],
        &controls,
    ));
    // QUALITY and the presets beside the random amounts, one above the other.
    let side = left + 4.25 * gear::CELL_W + 6.0;
    let side_w = left + inner_w - side - 6.0;
    let side_label = |y: f32, text: &str| {
        gear::text_at(
            &p,
            pos2(side, y),
            Align2::LEFT_CENTER,
            &text.to_uppercase(),
            gear::silk_font(11.5),
            gear::INK2(),
            0.12,
        )
    };
    let q = &controls[tx::QUALITY];
    let lr = side_label(y + 22.0 + 9.0, &q.label);
    if let Some(on) = q.automated {
        gear::led(&p, pos2(lr.right() + 7.0, lr.center().y), 2.5, on);
    }
    let names: Vec<&str> = q.choices.iter().map(String::as_str).collect();
    let qr = Rect::from_min_size(pos2(side, y + 22.0 + 22.0), vec2(side_w, 28.0));
    if let Some(i) = gear::choice_field(ui, qr, id.with("quality"), &names, q.choice, q.enabled) {
        changes.push(Change {
            index: tx::QUALITY,
            norm: i as f32 / (names.len().max(2) - 1) as f32,
            choice: Some(i),
            ended: true,
        });
    }
    side_label(y + 22.0 + 76.0, tr("Presets"));
    let pr = Rect::from_min_size(pos2(side, y + 22.0 + 89.0), vec2(side_w, 28.0));
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
    y += ROW_H;
    changes.extend(row(
        ui,
        left,
        y,
        id,
        &[(
            tr("Mix").to_string(),
            vec![tx::FEEDBACK, tx::DRY_WET, tx::REVERB],
        )],
        &controls,
    ));
    for ch in changes {
        let v = crate::native_fx_ui::value_of(info, &ch);
        set(vals, ch.index, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Project, TrackKind};

    fn texture_project() -> (Project, crate::UiState, u64, usize) {
        let mut p = Project::empty();
        let i = p.add_track(TrackKind::Audio);
        p.tracks[i].name = "Guitar".into();
        let info = prowave_engine::native_info(tx::ID).unwrap();
        let mut slot = crate::native_fx_ui::new_slot(info);
        if slot.uid == 0 {
            slot.uid = 5151;
        }
        let uid = slot.uid;
        p.tracks[i].au_effects.push(slot);
        let mut s = crate::UiState::new();
        s.native_window = Some(uid);
        (p, s, uid, i)
    }

    /// The window draws offscreen, and a preset applied the way the menu does reaches the
    /// slot.
    #[test]
    fn the_window_draws_and_presets_reach_the_slot() {
        let (mut p, mut s, uid, i) = texture_project();
        let mut h = crate::ui_test::Harness::new(WIDTH + 40.0, 900.0);
        h.settle(|ui| crate::native_fx_ui::show(ui.ctx(), &mut p, &mut s));
        assert_eq!(s.native_window, Some(uid), "the window stays open");
        let info = prowave_engine::native_info(tx::ID).unwrap();
        let slot = &mut p.tracks[i].au_effects[0];
        let mut v = crate::native_fx_ui::values(slot, info);
        info.presets[4].apply(&mut v);
        slot.state = Some(crate::native_fx_ui::encode(&v));
        let back = crate::native_fx_ui::values(&p.tracks[i].au_effects[0], info);
        assert_eq!(back[tx::SIZE], 1000.0);
    }

    /// Pictures of the window, to look at: `PROWAVE_TEXTURE_PNG=/dir cargo test -p
    /// prowave-app texture_window_pictures -- --ignored` (defaults, a random cloud, a
    /// reversed grain with freeze, the delay mode).
    #[test]
    #[ignore = "picture"]
    fn texture_window_pictures() {
        let dir = std::env::var("PROWAVE_TEXTURE_PNG")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        std::fs::create_dir_all(&dir).unwrap();
        let info = prowave_engine::native_info(tx::ID).unwrap();
        let sets: [(&str, &[(usize, f32)]); 4] = [
            ("defaults", &[]),
            ("cloud", info.presets[0].values),
            (
                "reverse-frozen",
                &[
                    (tx::SIZE, 150.0),
                    (tx::SHAPE, 800.0),
                    (tx::FREEZE, 1.0),
                    (tx::TIME_RANDOM, -300.0),
                    (tx::QUALITY, 3.0),
                ],
            ),
            ("delay", info.presets[4].values),
        ];
        for (name, values) in sets {
            let (mut p, mut s, _, i) = texture_project();
            let slot = &mut p.tracks[i].au_effects[0];
            let mut v = crate::native_fx_ui::values(slot, info);
            for (k, x) in values {
                v[*k] = *x;
            }
            slot.state = Some(crate::native_fx_ui::encode(&v));
            crate::ui_test::render_png(
                WIDTH + 60.0,
                820.0,
                1.0,
                egui::Color32::from_gray(0x2a),
                |ui| {
                    crate::theme::apply(ui.ctx());
                    crate::native_fx_ui::show(ui.ctx(), &mut p, &mut s)
                },
                &dir.join(format!("texture-{name}.png")),
            )
            .unwrap();
        }
    }
}
