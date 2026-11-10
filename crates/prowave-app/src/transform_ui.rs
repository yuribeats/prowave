//! The MIDI Transform window (Piano Roll > Functions > MIDI Transform…): pick which notes of
//! the region (a range of pitches, velocities and lengths) and what to do to each of the four
//! things a note has, then Apply. It works on the selected notes, or on all of them when none
//! is selected, and one Undo takes it back.
use crate::hw::{self, Cap, Legend};
use crate::i18n::{tr, trf};
use crate::model::Project;
use crate::piano_roll::{caption, dark_fields, key_button, silk_label};
use crate::theme::{self, Face};
use crate::transform::{apply, LengthOp, PitchOp, PositionOp, Preset, Transform, VelocityOp};
use crate::UiState;
use egui::{pos2, vec2, Align2, DragValue, Rect, Sense, Ui};

/// Width of the silkscreened label column of the window's rows.
const LABEL_W: f32 = 78.0;

/// A row's label, silkscreened and right-aligned in the label column.
fn row_label(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(LABEL_W, 24.0), Sense::hover());
    hw::silk(
        ui.painter(),
        pos2(r.right() - 6.0, r.center().y),
        Align2::RIGHT_CENTER,
        caption(text),
        11.0,
        theme::INK2,
    );
}

/// A section of the window: a bracket with its label across the full width.
fn section(ui: &mut Ui, label: &str) {
    ui.add_space(6.0);
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    hw::bracket(ui.painter(), r.left(), r.right() - 1.0, r.center().y, label);
}

/// What the window remembers.
pub struct TransformView {
    pub open: bool,
    pub t: Transform,
    /// Changes with every Apply, so a randomizing transform gives another take each time.
    seed: u32,
    /// Where the window was last frame: its faceplate is drawn there, under the controls.
    last_rect: Option<Rect>,
}

impl Default for TransformView {
    fn default() -> Self {
        Self {
            open: false,
            t: Preset::Transpose.transform(),
            seed: 1,
            last_rect: None,
        }
    }
}

impl TransformView {
    /// Opens the window with a ready-made transform, or as it was left (`None`).
    pub fn open_with(&mut self, preset: Option<Preset>) {
        if let Some(p) = preset {
            self.t = p.transform();
        }
        self.open = true;
    }
}

pub fn preset_name(p: Preset) -> &'static str {
    match p {
        Preset::Transpose => tr("Transpose"),
        Preset::FixedVelocity => tr("Fixed Velocity"),
        Preset::Legato => tr("Legato"),
        Preset::Humanize => tr("Humanize"),
        Preset::Quantize => tr("Quantize"),
    }
}

/// A box of `names` that picks one; returns the index when it changed.
fn pick(ui: &mut Ui, id: &str, names: &[&str], current: usize) -> Option<usize> {
    // A dark field with up/down arrows, its menu in a popup.
    let mut picked = None;
    let (r, _) = ui.allocate_exact_size(vec2(104.0, 24.0), Sense::hover());
    let g = ui.painter().clone();
    let resp = crate::widgets::popup_button(
        ui,
        &g,
        r,
        egui::Id::unique(("transform_pick", id)),
        names[current.min(names.len() - 1)],
    );
    egui::Popup::menu(&resp).show(|ui| {
        for (i, name) in names.iter().enumerate() {
            if ui.selectable_label(i == current, *name).clicked() {
                if i != current {
                    picked = Some(i);
                }
                ui.close();
            }
        }
    });
    picked
}

fn pitch_row(ui: &mut Ui, op: &mut PitchOp) {
    ui.horizontal(|ui| {
        row_label(ui, tr("Pitch:"));
        let names = [tr("Keep"), tr("Add"), tr("Set")];
        let current = match op {
            PitchOp::Keep => 0,
            PitchOp::Add(_) => 1,
            PitchOp::Set(_) => 2,
        };
        match pick(ui, "tf_pitch", &names, current) {
            Some(0) => *op = PitchOp::Keep,
            Some(1) => *op = PitchOp::Add(12),
            Some(_) => *op = PitchOp::Set(60),
            None => {}
        }
        match op {
            PitchOp::Add(d) => {
                ui.add(
                    DragValue::new(d)
                        .range(-48..=48)
                        .custom_formatter(|v, _| format!("{v:+}"))
                        .suffix(format!(" {}", tr("semitones").to_uppercase())),
                );
            }
            PitchOp::Set(p) => {
                ui.add(DragValue::new(p).range(0..=127));
            }
            PitchOp::Keep => {}
        }
    });
}

fn velocity_row(ui: &mut Ui, op: &mut VelocityOp) {
    ui.horizontal(|ui| {
        row_label(ui, tr("Velocity:"));
        let names = [
            tr("Keep"),
            tr("Add"),
            tr("Set"),
            tr("Scale"),
            tr("Randomize"),
        ];
        let current = match op {
            VelocityOp::Keep => 0,
            VelocityOp::Add(_) => 1,
            VelocityOp::Set(_) => 2,
            VelocityOp::Scale(_) => 3,
            VelocityOp::Randomize(_) => 4,
        };
        match pick(ui, "tf_velocity", &names, current) {
            Some(0) => *op = VelocityOp::Keep,
            Some(1) => *op = VelocityOp::Add(10),
            Some(2) => *op = VelocityOp::Set(100),
            Some(3) => *op = VelocityOp::Scale(120),
            Some(_) => *op = VelocityOp::Randomize(8),
            None => {}
        }
        match op {
            VelocityOp::Add(d) => {
                ui.add(
                    DragValue::new(d)
                        .range(-127..=127)
                        .custom_formatter(|v, _| format!("{v:+}")),
                );
            }
            VelocityOp::Set(v) => {
                ui.add(DragValue::new(v).range(1..=127));
            }
            VelocityOp::Scale(p) => {
                ui.add(DragValue::new(p).range(10..=400).suffix(" %"));
            }
            VelocityOp::Randomize(a) => {
                ui.add(DragValue::new(a).range(0..=127));
            }
            VelocityOp::Keep => {}
        }
    });
}

fn length_row(ui: &mut Ui, op: &mut LengthOp) {
    ui.horizontal(|ui| {
        row_label(ui, tr("Length:"));
        let names = [tr("Keep"), tr("Set"), tr("Scale"), tr("Legato")];
        let current = match op {
            LengthOp::Keep => 0,
            LengthOp::Set(_) => 1,
            LengthOp::Scale(_) => 2,
            LengthOp::Legato => 3,
        };
        match pick(ui, "tf_length", &names, current) {
            Some(0) => *op = LengthOp::Keep,
            Some(1) => *op = LengthOp::Set(0.5),
            Some(2) => *op = LengthOp::Scale(50),
            Some(_) => *op = LengthOp::Legato,
            None => {}
        }
        match op {
            LengthOp::Set(l) => {
                ui.add(
                    DragValue::new(l)
                        .range(0.01..=64.0)
                        .speed(0.05)
                        .suffix(format!(" {}", tr("beats").to_uppercase())),
                );
            }
            LengthOp::Scale(p) => {
                ui.add(DragValue::new(p).range(10..=400).suffix(" %"));
            }
            LengthOp::Keep | LengthOp::Legato => {}
        }
    });
}

fn position_row(ui: &mut Ui, op: &mut PositionOp) {
    ui.horizontal(|ui| {
        row_label(ui, tr("Position:"));
        let names = [tr("Keep"), tr("Shift"), tr("Quantize"), tr("Randomize")];
        let current = match op {
            PositionOp::Keep => 0,
            PositionOp::Shift(_) => 1,
            PositionOp::Quantize { .. } => 2,
            PositionOp::Randomize(_) => 3,
        };
        match pick(ui, "tf_position", &names, current) {
            Some(0) => *op = PositionOp::Keep,
            Some(1) => *op = PositionOp::Shift(0.25),
            Some(2) => {
                *op = PositionOp::Quantize {
                    grid: 0.25,
                    strength: 100,
                }
            }
            Some(_) => *op = PositionOp::Randomize(0.03),
            None => {}
        }
        match op {
            PositionOp::Shift(d) => {
                ui.add(
                    DragValue::new(d)
                        .range(-64.0..=64.0)
                        .speed(0.01)
                        .suffix(format!(" {}", tr("beats").to_uppercase())),
                );
            }
            PositionOp::Quantize { grid, strength } => {
                ui.add(
                    DragValue::new(grid)
                        .range(0.01..=4.0)
                        .speed(0.01)
                        .suffix(format!(" {}", tr("beats").to_uppercase())),
                );
                silk_label(ui, tr("Strength:"));
                ui.add(DragValue::new(strength).range(0..=100).suffix(" %"));
            }
            PositionOp::Randomize(a) => {
                ui.add(
                    DragValue::new(a)
                        .range(0.0..=1.0)
                        .speed(0.005)
                        .suffix(format!(" {}", tr("beats").to_uppercase())),
                );
            }
            PositionOp::Keep => {}
        }
    });
}

/// The two ends of a range, kept in order.
fn range_row<T: egui::emath::Numeric>(
    ui: &mut Ui,
    label: &str,
    (lo, hi): &mut (T, T),
    min: T,
    max: T,
) {
    ui.horizontal(|ui| {
        row_label(ui, label);
        ui.add(DragValue::new(lo).range(min..=max));
        silk_label(ui, tr("to"));
        ui.add(DragValue::new(hi).range(min..=max));
    });
    if lo.to_f64() > hi.to_f64() {
        std::mem::swap(lo, hi);
    }
}

/// Applies the window's transform to the region: the selected notes, or all of them.
pub fn apply_to(p: &mut Project, s: &mut UiState, ti: usize, ri: usize) {
    let t = s.transform.t;
    let seed = s.transform.seed;
    s.transform.seed = seed.wrapping_add(1);
    let which: Option<Vec<usize>> =
        (!s.piano.selected.is_empty()).then(|| s.piano.selected.clone());
    let len = p.region_len_beats(&p.tracks[ti].regions[ri]) as f32;
    let notes = &mut p.tracks[ti].regions[ri].notes;
    let changed = apply(&t, notes, which.as_deref(), len, seed);
    if changed > 0 {
        p.song_rev += 1;
    }
    s.status = Some(trf("{} notes changed", &[&changed]));
}

/// Draws the window when it is open.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    if !s.transform.open {
        return;
    }
    let target = crate::piano_roll::target_region(p, s).filter(|&(t, r)| {
        let region = &p.tracks[t].regions[r];
        region.generated.is_none() && region.folder.is_none()
    });
    let (mut open, mut run) = (true, false);
    let last = s.transform.last_rect;
    let shown = egui::Window::new(tr("MIDI Transform"))
        .id(egui::Id::unique("transform_window"))
        .title_bar(false)
        .resizable(false)
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            left: 18,
            right: 18,
            top: 48,
            bottom: 20,
        }))
        .show(ctx, |ui| {
            // The window is a faceplate: a silkscreened title on a strip with a seam under it,
            // a screw in each corner and a key to close it. It is drawn where the window was
            // last frame, under this frame's controls.
            let g = ui.ctx().layer_painter(ui.layer_id());
            if let Some(r) = last {
                hw::faceplate(&g, r, 5.0, false);
                hw::seam_h(&g, r.left() + 1.0..=r.right() - 1.0, r.top() + 36.0);
                hw::text(
                    &g,
                    pos2(r.left() + 30.0, r.top() + 19.0),
                    Align2::LEFT_CENTER,
                    &tr("MIDI Transform").to_uppercase(),
                    Face::SilkX,
                    14.0,
                    theme::INK,
                );
                for c in [
                    r.left_top() + vec2(13.0, 19.0),
                    r.left_bottom() + vec2(10.0, -10.0),
                    r.right_bottom() + vec2(-10.0, -10.0),
                ] {
                    hw::screw(&g, c, 3.5, 40.0);
                }
                let close =
                    Rect::from_min_size(pos2(r.right() - 40.0, r.top() + 8.0), vec2(28.0, 22.0));
                let (resp, face) = hw::key(
                    ui,
                    &g,
                    close,
                    egui::Id::unique("transform_close"),
                    Cap::Bone,
                    false,
                    None,
                    Legend::Custom,
                );
                let c = face.center();
                let st = egui::Stroke::new(1.6, theme::INK);
                g.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], st);
                g.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], st);
                if resp.clicked() {
                    open = false;
                }
            }
            dark_fields(ui);
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            let Some((ti, ri)) = target else {
                silk_label(
                    ui,
                    tr("Select an ordinary MIDI region to transform its notes"),
                );
                return;
            };
            let selected = s.piano.selected.len();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                silk_label(ui, tr("Presets:"));
                ui.add_space(4.0);
                for preset in Preset::ALL {
                    if key_button(ui, preset_name(preset), true).clicked() {
                        s.transform.t = preset.transform();
                    }
                }
            });
            section(ui, tr("Notes that match"));
            let t = &mut s.transform.t;
            range_row(ui, tr("Pitch:"), &mut t.pitch, 0, 127);
            range_row(ui, tr("Velocity:"), &mut t.velocity, 1, 127);
            let mut len = (t.length.0, t.length.1.min(64.0));
            range_row(ui, tr("Length:"), &mut len, 0.0, 64.0);
            t.length = (len.0, if len.1 >= 64.0 { f32::MAX } else { len.1 });
            section(ui, tr("What is done to them"));
            pitch_row(ui, &mut t.pitch_op);
            velocity_row(ui, &mut t.velocity_op);
            length_row(ui, &mut t.length_op);
            position_row(ui, &mut t.position_op);
            ui.add_space(4.0);
            hw::seam_h(ui.painter(), ui.max_rect().x_range(), ui.cursor().top());
            ui.add_space(12.0);
            // What it works on, on a small LCD (the count in segments), and Apply: the
            // window's one orange key, at the right.
            let (label, count) = if selected > 0 {
                (trf("{} selected notes", &[&""]), selected)
            } else {
                (
                    tr("All notes of the region").to_string(),
                    p.tracks[ti].regions[ri].notes.len(),
                )
            };
            let w = ui.available_width();
            let (row, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
            let g = ui.painter().clone();
            let chip = Rect::from_min_size(row.min, vec2((w - 110.0).min(240.0), 32.0));
            hw::lcd_glass(&g, chip);
            let label = hw::fit(
                &g,
                label.trim().to_uppercase().as_str(),
                Face::Lcd,
                9.5,
                chip.width() - 60.0,
            );
            hw::text(
                &g,
                pos2(chip.left() + 8.0, chip.center().y),
                Align2::LEFT_CENTER,
                &label,
                Face::Lcd,
                9.5,
                theme::LCD_INK.gamma_multiply(0.75),
            );
            let digits = count.min(9999).to_string();
            let h = 17.0;
            hw::seg(
                &g,
                pos2(
                    chip.right() - 8.0 - hw::seg_width(&digits, h),
                    chip.center().y - h / 2.0,
                ),
                &digits,
                h,
                theme::LCD_INK,
                true,
            );
            let apply_r =
                Rect::from_min_size(pos2(row.right() - 96.0, row.top()), vec2(96.0, 30.0));
            let (resp, _) = hw::key(
                ui,
                &g,
                apply_r,
                egui::Id::unique("transform_apply"),
                Cap::Orange,
                false,
                None,
                Legend::Text(tr("Apply")),
            );
            if resp.clicked() {
                run = true;
            }
        });
    if let Some(shown) = shown {
        s.transform.last_rect = Some(shown.response.rect);
        if last.is_none() {
            ctx.request_repaint();
        }
    }
    if run {
        if let Some((ti, ri)) = target {
            apply_to(p, s, ti, ri);
        }
    }
    if !open {
        s.transform.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_test::Harness;

    #[test]
    fn the_window_draws_for_every_operation_and_applies() {
        let mut p = Project::demo();
        let mut s = UiState::new();
        s.tracks.selected = Some((0, 0));
        s.transform.open_with(None);
        let mut h = Harness::new(700.0, 900.0);
        let before: Vec<_> = p.tracks[0].regions[0]
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        for preset in Preset::ALL {
            s.transform.open_with(Some(preset));
            for _ in 0..3 {
                h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
            }
        }
        for t in [
            Transform {
                pitch_op: PitchOp::Set(60),
                velocity_op: VelocityOp::Scale(50),
                length_op: LengthOp::Set(1.0),
                position_op: PositionOp::Shift(0.5),
                ..Transform::default()
            },
            Transform {
                pitch_op: PitchOp::Add(-3),
                velocity_op: VelocityOp::Randomize(9),
                length_op: LengthOp::Scale(70),
                position_op: PositionOp::Randomize(0.1),
                ..Transform::default()
            },
            Transform {
                velocity_op: VelocityOp::Add(-5),
                length_op: LengthOp::Legato,
                position_op: PositionOp::Quantize {
                    grid: 0.5,
                    strength: 80,
                },
                ..Transform::default()
            },
        ] {
            s.transform.t = t;
            for _ in 0..3 {
                h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
            }
        }
        assert!(s.transform.open);
        assert_eq!(
            before,
            p.tracks[0].regions[0]
                .notes
                .iter()
                .map(|n| n.pitch)
                .collect::<Vec<_>>(),
            "drawing the window changes nothing"
        );
        // A generated region is not transformed.
        let t = p.add_track(crate::model::TrackKind::Drummer);
        let r = p.add_generated_region(t, true, 1.0).unwrap();
        s.tracks.selected = Some((t, r));
        h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
        s.transform.open = false;
        h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
    }

    #[test]
    fn apply_changes_the_selected_notes_or_all_and_tells_the_engine() {
        let mut p = Project::demo();
        let mut s = UiState::new();
        let before: Vec<u8> = p.tracks[0].regions[0]
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        let rev = p.song_rev;
        s.transform.open_with(Some(Preset::Transpose));
        // Two notes selected: only they move.
        s.piano.selected = vec![0, 2];
        apply_to(&mut p, &mut s, 0, 0);
        let after: Vec<u8> = p.tracks[0].regions[0]
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        for (i, (a, b)) in before.iter().zip(&after).enumerate() {
            let moved = matches!(i, 0 | 2);
            assert_eq!(*b, if moved { (*a + 12).min(127) } else { *a }, "note {i}");
        }
        assert!(p.song_rev > rev);
        assert!(
            s.status.as_deref().is_some_and(|t| t.starts_with('2')),
            "{:?}",
            s.status
        );
        // None selected: all of them.
        s.piano.selected.clear();
        apply_to(&mut p, &mut s, 0, 0);
        assert!(s.status.as_deref().is_some_and(|t| !t.starts_with('0')));
        // A transform that changes nothing leaves the song alone.
        s.transform.t = Transform::default();
        let rev = p.song_rev;
        apply_to(&mut p, &mut s, 0, 0);
        assert_eq!(p.song_rev, rev);
    }

    #[test]
    fn preset_names_exist() {
        for p in Preset::ALL {
            assert!(!preset_name(p).is_empty());
        }
    }
}
