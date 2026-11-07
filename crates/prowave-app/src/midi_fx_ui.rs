//! The MIDI FX window (Track > MIDI FX…): the chain of effects of the selected track, with each
//! effect's settings. Changes bump the song revision so the engine gets the new notes.
use crate::i18n::{tr, trf};
use crate::midi_fx::{
    ArpPattern, ChordKind, FxSlot, LfoShape, MidiFx, ModTarget, MAX_FX, MOD_PERIODS,
};
use crate::model::Project;
use crate::pattern::StepRate;
use crate::step_editor::NOTE_NAMES;
use crate::UiState;
use egui::{ComboBox, DragValue, Ui};

fn title(fx: &MidiFx) -> &'static str {
    match fx {
        MidiFx::Scripter { .. } => tr("Scripter"),
        MidiFx::Modulator { .. } => tr("Modulator"),
        MidiFx::Transposer { .. } => tr("Transposer"),
        MidiFx::Velocity { .. } => tr("Velocity Processor"),
        MidiFx::Scale { .. } => tr("Scale Quantizer"),
        MidiFx::Chord { .. } => tr("Chord Trigger"),
        MidiFx::Repeater { .. } => tr("Note Repeater"),
        MidiFx::Arpeggiator { .. } => tr("Arpeggiator"),
        MidiFx::Randomizer { .. } => tr("Randomizer"),
    }
}

fn shape_name(s: LfoShape) -> &'static str {
    match s {
        LfoShape::Sine => tr("Sine"),
        LfoShape::Triangle => tr("Triangle"),
        LfoShape::Saw => tr("Saw"),
        LfoShape::Square => tr("Square"),
        LfoShape::Random => tr("Random"),
    }
}

/// A Modulator's cycle length as text: quarter notes, or bars of four when longer.
fn period_text(i: usize) -> String {
    let beats = MOD_PERIODS[i.min(MOD_PERIODS.len() - 1)];
    if beats >= 4.0 {
        trf("{} bars", &[&(beats / 4.0)])
    } else {
        trf("{} beats", &[&beats])
    }
}

fn chord_name(k: ChordKind) -> &'static str {
    match k {
        ChordKind::Major => tr("Major"),
        ChordKind::Minor => tr("Minor"),
        ChordKind::Seventh => tr("Dominant 7th"),
        ChordKind::MajorSeventh => tr("Major 7th"),
        ChordKind::MinorSeventh => tr("Minor 7th"),
        ChordKind::Sus4 => tr("Sus4"),
        ChordKind::Fifth => tr("Power Chord"),
        ChordKind::Octave => tr("Octave"),
    }
}

fn pattern_name(p: ArpPattern) -> &'static str {
    match p {
        ArpPattern::Up => tr("Up"),
        ArpPattern::Down => tr("Down"),
        ArpPattern::UpDown => tr("Up and Down"),
        ArpPattern::Random => tr("Random"),
        ArpPattern::AsPlayed => tr("As Played"),
    }
}

fn rate_combo(ui: &mut Ui, id: &str, rate: &mut StepRate) -> bool {
    let mut changed = false;
    ComboBox::from_id_salt(id)
        .selected_text(rate.name())
        .width(64.0)
        .show_ui(ui, |ui| {
            for r in StepRate::ALL {
                if ui.selectable_label(*rate == r, r.name()).clicked() {
                    *rate = r;
                    changed = true;
                }
            }
        });
    changed
}

/// The settings of one effect; returns whether any changed.
fn settings(ui: &mut Ui, fx: &mut MidiFx, id: usize) -> bool {
    let mut changed = false;
    match fx {
        MidiFx::Scripter { source } => {
            let edit = egui::TextEdit::multiline(source)
                .code_editor()
                .desired_rows(9)
                .desired_width(430.0)
                .char_limit(crate::scripter::MAX_SCRIPT_CHARS);
            changed |= ui.add(edit).changed();
            match crate::scripter::check(source) {
                Ok(()) => {
                    ui.label(tr("The script works"));
                }
                Err(e) => {
                    ui.colored_label(egui::Color32::from_rgb(0xe8, 0x70, 0x60), e);
                }
            }
            if ui.button(tr("Start Over")).clicked() {
                *source = crate::scripter::DEFAULT_SCRIPT.to_string();
                changed = true;
            }
        }
        MidiFx::Modulator {
            shape,
            period,
            depth,
            target,
            seed,
        } => {
            ui.horizontal(|ui| {
                ui.label(tr("Shape:"));
                ComboBox::from_id_salt(("fx_shape", id))
                    .selected_text(shape_name(*shape))
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for s in LfoShape::ALL {
                            if ui.selectable_label(*shape == s, shape_name(s)).clicked() {
                                *shape = s;
                                changed = true;
                            }
                        }
                    });
                ui.label(tr("Cycle:"));
                ComboBox::from_id_salt(("fx_period", id))
                    .selected_text(period_text(*period as usize))
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for i in 0..MOD_PERIODS.len() {
                            if ui
                                .selectable_label(*period as usize == i, period_text(i))
                                .clicked()
                            {
                                *period = i as u8;
                                changed = true;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label(tr("Depth:"));
                changed |= ui
                    .add(DragValue::new(depth).range(0..=100).suffix(" %"))
                    .changed();
                ui.label(tr("Moves:"));
                let name = match target {
                    ModTarget::Velocity => tr("Velocity"),
                    ModTarget::Pitch => tr("Pitch"),
                    ModTarget::Controller { .. } => tr("Controller"),
                };
                ComboBox::from_id_salt(("fx_target", id))
                    .selected_text(name)
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for (t, label) in [
                            (ModTarget::Velocity, tr("Velocity")),
                            (ModTarget::Pitch, tr("Pitch")),
                            (ModTarget::Controller { number: 74 }, tr("Controller")),
                        ] {
                            let on = std::mem::discriminant(target) == std::mem::discriminant(&t);
                            if ui.selectable_label(on, label).clicked() && !on {
                                *target = t;
                                changed = true;
                            }
                        }
                    });
                if let ModTarget::Controller { number } = target {
                    ui.label(tr("Number:"));
                    changed |= ui.add(DragValue::new(number).range(0..=127)).changed();
                }
            });
            if *shape == LfoShape::Random && ui.button(tr("New Variation")).clicked() {
                *seed = seed.wrapping_add(1);
                changed = true;
            }
            if matches!(target, ModTarget::Controller { .. }) {
                ui.label(tr("Sent to External MIDI tracks"));
            }
        }
        MidiFx::Transposer { semitones } => {
            ui.horizontal(|ui| {
                ui.label(tr("Semitones:"));
                changed |= ui.add(DragValue::new(semitones).range(-48..=48)).changed();
            });
        }
        MidiFx::Velocity { add, scale } => {
            ui.horizontal(|ui| {
                ui.label(tr("Add:"));
                changed |= ui.add(DragValue::new(add).range(-127..=127)).changed();
                ui.label(tr("Scale:"));
                changed |= ui
                    .add(DragValue::new(scale).range(10..=200).suffix(" %"))
                    .changed();
            });
        }
        MidiFx::Scale { root, minor } => {
            ui.horizontal(|ui| {
                ui.label(tr("Root:"));
                ComboBox::from_id_salt(("fx_root", id))
                    .selected_text(NOTE_NAMES[*root as usize % 12])
                    .width(56.0)
                    .show_ui(ui, |ui| {
                        for (i, name) in NOTE_NAMES.iter().enumerate() {
                            if ui.selectable_label(*root as usize == i, *name).clicked() {
                                *root = i as u8;
                                changed = true;
                            }
                        }
                    });
                if ui.selectable_label(!*minor, tr("Major")).clicked() {
                    *minor = false;
                    changed = true;
                }
                if ui.selectable_label(*minor, tr("Minor")).clicked() {
                    *minor = true;
                    changed = true;
                }
            });
        }
        MidiFx::Chord { kind } => {
            ui.horizontal(|ui| {
                ui.label(tr("Chord:"));
                ComboBox::from_id_salt(("fx_chord", id))
                    .selected_text(chord_name(*kind))
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for k in ChordKind::ALL {
                            if ui.selectable_label(*kind == k, chord_name(k)).clicked() {
                                *kind = k;
                                changed = true;
                            }
                        }
                    });
            });
        }
        MidiFx::Repeater { rate } => {
            ui.horizontal(|ui| {
                ui.label(tr("Rate:"));
                changed |= rate_combo(ui, &format!("fx_rate{id}"), rate);
            });
        }
        MidiFx::Arpeggiator {
            rate,
            pattern,
            octaves,
            gate,
        } => {
            ui.horizontal(|ui| {
                ui.label(tr("Rate:"));
                changed |= rate_combo(ui, &format!("fx_rate{id}"), rate);
                ui.label(tr("Pattern:"));
                ComboBox::from_id_salt(("fx_pattern", id))
                    .selected_text(pattern_name(*pattern))
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for p in ArpPattern::ALL {
                            if ui
                                .selectable_label(*pattern == p, pattern_name(p))
                                .clicked()
                            {
                                *pattern = p;
                                changed = true;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label(tr("Octaves:"));
                changed |= ui.add(DragValue::new(octaves).range(1..=4)).changed();
                ui.label(tr("Gate:"));
                changed |= ui
                    .add(DragValue::new(gate).range(5..=100).suffix(" %"))
                    .changed();
            });
        }
        MidiFx::Randomizer {
            pitch,
            velocity,
            timing,
            seed,
        } => {
            ui.horizontal(|ui| {
                ui.label(tr("Pitch:"));
                changed |= ui.add(DragValue::new(pitch).range(0..=24)).changed();
                ui.label(tr("Velocity:"));
                changed |= ui.add(DragValue::new(velocity).range(0..=127)).changed();
                ui.label(tr("Timing:"));
                changed |= ui
                    .add(DragValue::new(timing).range(0..=100).suffix(" %"))
                    .changed();
            });
            if ui.button(tr("New Variation")).clicked() {
                *seed = seed.wrapping_add(1);
                changed = true;
            }
        }
    }
    changed
}

/// Draws the window when it is open.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    if !s.show_midi_fx {
        return;
    }
    let ti = p.selected_track;
    let Some(name) = p.tracks.get(ti).map(|t| t.name.clone()) else {
        return;
    };
    let mut open = true;
    let mut changed = false;
    egui::Window::new(trf("MIDI FX: {}", &[&name]))
        .id(egui::Id::unique("midi_fx_window"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            let chain = &mut p.tracks[ti].midi_fx;
            if chain.is_empty() {
                ui.label(tr("No MIDI FX on this track"));
            }
            let (mut remove, mut up) = (None, None);
            let count = chain.len();
            for (i, slot) in chain.iter_mut().enumerate() {
                ui.separator();
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut slot.on, title(&slot.fx)).changed();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                        if ui
                            .add_enabled(i + 1 < count, egui::Button::new("↓").small())
                            .clicked()
                        {
                            up = Some(i + 1);
                        }
                        if ui
                            .add_enabled(i > 0, egui::Button::new("↑").small())
                            .clicked()
                        {
                            up = Some(i);
                        }
                    });
                });
                ui.add_enabled_ui(slot.on, |ui| {
                    changed |= settings(ui, &mut slot.fx, i);
                });
            }
            if let Some(i) = remove {
                chain.remove(i);
                changed = true;
            }
            if let Some(i) = up {
                if i > 0 && i < chain.len() {
                    chain.swap(i - 1, i);
                    changed = true;
                }
            }
            ui.separator();
            let room = chain.len() < MAX_FX;
            ui.add_enabled_ui(room, |ui| {
                ui.menu_button(tr("Add MIDI FX"), |ui| {
                    for fx in MidiFx::all_defaults() {
                        if ui.button(title(&fx)).clicked() {
                            chain.push(FxSlot { fx, on: true });
                            changed = true;
                            ui.close();
                        }
                    }
                });
            });
        });
    if changed {
        p.song_rev += 1;
    }
    if !open {
        s.show_midi_fx = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_test::Harness;

    #[test]
    fn the_window_draws_every_effect_and_closes() {
        let mut p = Project::demo();
        let mut s = UiState::new();
        s.show_midi_fx = true;
        p.selected_track = 0;
        p.tracks[0].midi_fx = MidiFx::all_defaults()
            .into_iter()
            .map(|fx| FxSlot { fx, on: true })
            .chain([FxSlot {
                fx: MidiFx::Transposer { semitones: 3 },
                on: false,
            }])
            .collect();
        let mut h = Harness::new(900.0, 1400.0);
        let rev = p.song_rev;
        for _ in 0..4 {
            h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
        }
        assert!(s.show_midi_fx, "still open");
        assert_eq!(p.song_rev, rev, "looking changes nothing");
        // A track that is not there, and a window that is shut, draw nothing.
        p.selected_track = 999;
        h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
        s.show_midi_fx = false;
        h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
    }

    #[test]
    fn every_title_and_name_exists_in_english() {
        for fx in MidiFx::all_defaults() {
            assert!(!title(&fx).is_empty());
        }
        for k in ChordKind::ALL {
            assert!(!chord_name(k).is_empty());
        }
        for a in ArpPattern::ALL {
            assert!(!pattern_name(a).is_empty());
        }
    }
}
