//! The MIDI Transform window (Piano Roll > Functions > MIDI Transform…): pick which notes of
//! the region (a range of pitches, velocities and lengths) and what to do to each of the four
//! things a note has, then Apply. It works on the selected notes, or on all of them when none
//! is selected, and one Undo takes it back.
use crate::i18n::{tr, trf};
use crate::model::Project;
use crate::transform::{apply, LengthOp, PitchOp, PositionOp, Preset, Transform, VelocityOp};
use crate::UiState;
use egui::{ComboBox, DragValue, Ui};

/// What the window remembers.
pub struct TransformView {
    pub open: bool,
    pub t: Transform,
    /// Changes with every Apply, so a randomizing transform gives another take each time.
    seed: u32,
}

impl Default for TransformView {
    fn default() -> Self {
        Self {
            open: false,
            t: Preset::Transpose.transform(),
            seed: 1,
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
    let mut picked = None;
    ComboBox::from_id_salt(id)
        .selected_text(names[current.min(names.len() - 1)])
        .width(96.0)
        .show_ui(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                if ui.selectable_label(i == current, *name).clicked() && i != current {
                    picked = Some(i);
                }
            }
        });
    picked
}

fn pitch_row(ui: &mut Ui, op: &mut PitchOp) {
    ui.horizontal(|ui| {
        ui.label(tr("Pitch:"));
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
                        .suffix(format!(" {}", tr("semitones"))),
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
        ui.label(tr("Velocity:"));
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
                ui.add(DragValue::new(d).range(-127..=127));
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
        ui.label(tr("Length:"));
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
                        .suffix(format!(" {}", tr("beats"))),
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
        ui.label(tr("Position:"));
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
                        .suffix(format!(" {}", tr("beats"))),
                );
            }
            PositionOp::Quantize { grid, strength } => {
                ui.add(
                    DragValue::new(grid)
                        .range(0.01..=4.0)
                        .speed(0.01)
                        .suffix(format!(" {}", tr("beats"))),
                );
                ui.label(tr("Strength:"));
                ui.add(DragValue::new(strength).range(0..=100).suffix(" %"));
            }
            PositionOp::Randomize(a) => {
                ui.add(
                    DragValue::new(a)
                        .range(0.0..=1.0)
                        .speed(0.005)
                        .suffix(format!(" {}", tr("beats"))),
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
        ui.label(label);
        ui.add(DragValue::new(lo).range(min..=max));
        ui.label(tr("to"));
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
    egui::Window::new(tr("MIDI Transform"))
        .id(egui::Id::unique("transform_window"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            let Some((ti, ri)) = target else {
                ui.label(tr("Select an ordinary MIDI region to transform its notes"));
                return;
            };
            let selected = s.piano.selected.len();
            ui.horizontal(|ui| {
                ui.label(tr("Presets:"));
                for preset in Preset::ALL {
                    if ui.button(preset_name(preset)).clicked() {
                        s.transform.t = preset.transform();
                    }
                }
            });
            ui.separator();
            ui.strong(tr("Notes that match"));
            let t = &mut s.transform.t;
            range_row(ui, tr("Pitch:"), &mut t.pitch, 0, 127);
            range_row(ui, tr("Velocity:"), &mut t.velocity, 1, 127);
            let mut len = (t.length.0, t.length.1.min(64.0));
            range_row(ui, tr("Length:"), &mut len, 0.0, 64.0);
            t.length = (len.0, if len.1 >= 64.0 { f32::MAX } else { len.1 });
            ui.separator();
            ui.strong(tr("What is done to them"));
            pitch_row(ui, &mut t.pitch_op);
            velocity_row(ui, &mut t.velocity_op);
            length_row(ui, &mut t.length_op);
            position_row(ui, &mut t.position_op);
            ui.separator();
            let which = if selected > 0 {
                trf("{} selected notes", &[&selected])
            } else {
                tr("All notes of the region").to_string()
            };
            ui.label(which);
            if ui.button(tr("Apply")).clicked() {
                run = true;
            }
            let _ = (ti, ri);
        });
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
