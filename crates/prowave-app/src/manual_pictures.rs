//! Pictures of windows for the manual (docs/manual). Ignored; they check nothing:
//! `MANUAL_PNG_DIR=docs/manual/assets cargo test -p prowave-app manual_pictures -- --ignored`
//!
//! The manual also uses the pictures other modules' tests draw (run them all with
//! `MANUAL_PNG_DIR=… PROWAVE_UI_PNG_DIR=… cargo test -p prowave-app -- --include-ignored
//! picture the_editor_drags_a_note the_tracks_area_toggles_automation_mode`; the ones without
//! a folder variable land in the temp folder):
//! - `tracks_area::pictures` → `tracks-main.png`; `transport` → `control-bar-1470.png`
//! - `musical_typing`, `midi_monitor` → `musical-typing.png`, `midi-monitor.png`
//! - `file_browser` → `loop-browser.png`, `all-files.png`
//! - `free_plugins` → `free-plugins.png`, `free-plugins-running.png`
//! - `pitch_editor` → `pitch-editor.png`; `tune_lane` → `tune-lane-*.png`
//! - `automation_ui` → `prowave-automation-lanes.png`, `prowave-automation-mixer.png`
//! - `piano_roll::midi_editor_pictures` → `prowave-midi-shots/*.png` (Piano Roll, Score,
//!   Step Sequencer, Drummer, Event List, Transform)
//! - `fx_pictures` → `prowave-fx/*.png` (MIDI FX rack, plug-in units, Pro Wave Tune)
//! - `start_screen` → `chooser-templates.png`
use crate::model::{Note, Project, TrackKind};
use egui::Color32;
use std::path::PathBuf;

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("MANUAL_PNG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("prowave-manual"));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

const BG: Color32 = Color32::from_gray(0x2a);
/// The floor the effect units float over, as in `fx_pictures`.
const FLOOR: Color32 = Color32::from_rgb(0xCF, 0xC8, 0xB8);

#[test]
#[ignore = "pictures for the manual"]
fn manual_pictures_mixer_and_tracks() {
    let mut p = Project::demo();
    let mut s = crate::UiState::new();
    let meters = crate::mixer::Meters::default();
    crate::ui_test::render_png(
        1400.0,
        620.0,
        1.0,
        BG,
        |ui| {
            crate::theme::apply(ui.ctx());
            crate::mixer::show(ui, &mut p, &mut s, &meters)
        },
        &out("mixer.png"),
    )
    .unwrap();
    let mut p = Project::demo();
    let mut s = crate::UiState::new();
    crate::ui_test::render_png(
        1400.0,
        560.0,
        1.0,
        BG,
        |ui| {
            crate::theme::apply(ui.ctx());
            crate::tracks_area::show(ui, &mut p, &mut s, &meters)
        },
        &out("tracks-area.png"),
    )
    .unwrap();
}

#[test]
#[ignore = "pictures for the manual"]
fn manual_pictures_channel_rack() {
    use crate::channel_rack::{encode, new_slot, RackChannel};
    let mut p = Project::empty();
    let i = p.add_track(TrackKind::Instrument);
    p.tracks[i].name = "Channel Rack".into();
    let names = ["Kick", "Snare", "Hat", "Open Hat", "Clap"];
    let channels: Vec<RackChannel> = names
        .iter()
        .map(|n| RackChannel {
            path: format!("/samples/{n}.wav"),
            name: (*n).into(),
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
        })
        .collect();
    let mut slot = new_slot();
    slot.state = Some(encode(&channels));
    p.tracks[i].au_instrument = Some(slot);
    p.selected_track = i;
    let steps: [&[usize]; 5] = [
        &[0, 7, 10],
        &[4, 12],
        &[0, 2, 4, 6, 8, 10, 12, 14],
        &[15],
        &[4, 12],
    ];
    let mut s = crate::UiState::new();
    let mut frame = 0;
    crate::ui_test::render_png(
        1200.0,
        300.0,
        1.0,
        BG,
        |ui| {
            crate::theme::apply(ui.ctx());
            frame += 1;
            if frame == 2 {
                if let Some(r) = p.tracks[i].regions.first_mut() {
                    for (c, ks) in steps.iter().enumerate() {
                        for &k in *ks {
                            r.notes.push(Note {
                                lyric: crate::lyrics::Lyric::NONE,
                                start: k as f32 * 0.25,
                                len: 0.25,
                                pitch: prowave_engine::RACK_BASE_NOTE + c as u8,
                                vel: 100,
                                mute: false,
                            });
                        }
                    }
                }
            }
            crate::channel_rack::show(ui, &mut p, &mut s)
        },
        &out("channel-rack.png"),
    )
    .unwrap();
}

#[test]
#[ignore = "pictures for the manual"]
fn manual_pictures_native_effects() {
    for info in prowave_engine::native_effects() {
        let mut p = Project::empty();
        let i = p.add_track(TrackKind::Audio);
        p.tracks[i].name = "Drums".into();
        let slot = crate::native_fx_ui::new_slot(info);
        let uid = slot.uid;
        p.tracks[i].au_effects.push(slot);
        let mut s = crate::UiState::new();
        s.native_window = Some(uid);
        let file = format!(
            "fx-{}.png",
            info.id.trim_start_matches("prowave:").replace(':', "-")
        );
        // The whole unit, at the size the effect windows open (the tune unit is taller).
        crate::ui_test::render_png(
            820.0,
            if info.id == prowave_engine::tune::ID {
                900.0
            } else {
                820.0
            },
            2.0,
            FLOOR,
            |ui| {
                crate::theme::apply(ui.ctx());
                crate::native_fx_ui::show(ui.ctx(), &mut p, &mut s)
            },
            &out(&file),
        )
        .unwrap();
    }
}

/// Automation Mode in the tracks area (`automation.png`): a drawn volume envelope, an
/// overridden pan lane and the Stereo Out's lane; and the mixer with its automation LEDs
/// (`automation-mixer.png`).
#[test]
#[ignore = "pictures for the manual"]
fn manual_pictures_automation() {
    use prowave_engine::AutoTarget;
    let mut p = Project::demo();
    p.position_beats = 0.0;
    let mut s = crate::UiState::new();
    s.auto.mode = true;
    let auto = &mut p.tracks[0].auto;
    for (beat, db) in [
        (0.0, 0.0),
        (8.0, 0.0),
        (8.0, -12.0),
        (16.0, -12.0),
        (24.0, -3.0),
    ] {
        auto.set_point(AutoTarget::Volume, beat, db);
    }
    auto.set_point(AutoTarget::Pan, 2.0, 0.5);
    auto.set_point(AutoTarget::Pan, 8.0, -0.5);
    auto.overridden.push(AutoTarget::Pan);
    auto.extra.push(AutoTarget::Pan);
    auto.set_point(AutoTarget::Mute, 1.0, 1.0);
    p.show_master_track = true;
    p.master_auto.set_point(AutoTarget::Volume, 0.0, -6.0);
    p.master_auto.set_point(AutoTarget::Volume, 12.0, 0.0);
    let meters = crate::mixer::Meters::default();
    crate::ui_test::render_png(
        1400.0,
        640.0,
        1.0,
        BG,
        |ui| {
            crate::theme::apply(ui.ctx());
            crate::tracks_area::show(ui, &mut p, &mut s, &meters)
        },
        &out("automation.png"),
    )
    .unwrap();
    crate::ui_test::render_png(
        700.0,
        620.0,
        1.0,
        BG,
        |ui| {
            crate::theme::apply(ui.ctx());
            crate::mixer::show(ui, &mut p, &mut s, &meters)
        },
        &out("automation-mixer.png"),
    )
    .unwrap();
}

/// Space D as its effect window opens from a mixer slot (the whole SD-1 unit). Musical Typing
/// and the MIDI Monitor are drawn by their own modules' picture tests.
#[test]
#[ignore = "pictures for the manual"]
fn manual_pictures_space_designer() {
    use crate::model::Insert;
    use prowave_engine::FxKind;
    let mut p = Project::empty();
    let i = p.add_track(TrackKind::Audio);
    p.tracks[i].name = "Lead Vocal".into();
    p.tracks[i].inserts = vec![Insert::new(FxKind::Reverb)];
    let mut s = crate::UiState::new();
    s.mixer.fx_window = Some((i, 0));
    let meters = crate::mixer::Meters {
        fx_input: vec![0.4; i + 1],
        ..Default::default()
    };
    crate::ui_test::render_png(
        1000.0,
        860.0,
        2.0,
        FLOOR,
        |ui| {
            crate::theme::apply(ui.ctx());
            crate::fx_ui::show(ui.ctx(), &mut p, &mut s, &meters)
        },
        &out("space-designer.png"),
    )
    .unwrap();
}

#[test]
#[ignore = "pictures for the manual"]
fn manual_pictures_control_bar() {
    let mut p = Project::demo();
    let mut s = crate::UiState::new();
    for (playing, name) in [
        (false, "control-bar.png"),
        (true, "control-bar-playing.png"),
    ] {
        p.playing = playing;
        crate::ui_test::render_png(
            1470.0,
            crate::transport::HEIGHT,
            2.0,
            BG,
            |ui| {
                crate::theme::apply(ui.ctx());
                crate::transport::control_bar(ui, &mut p, &mut s, Some((0.55, 0.4)))
            },
            &out(name),
        )
        .unwrap();
    }
}

/// Every Hardware painter on one sheet (`hw-kit.png`): the reference for restyling a window.
#[test]
#[ignore = "pictures for the manual"]
fn manual_pictures_hardware_kit() {
    use crate::hw::{self, Cap, Legend, SlotKind};
    use crate::theme::{self, Face};
    use egui::{pos2, vec2, Align2, Rect};
    crate::ui_test::render_png(
        1200.0,
        700.0,
        2.0,
        theme::BONE,
        |ui| {
            theme::apply(ui.ctx());
            let g = ui.painter().clone();
            let full = ui.max_rect();
            hw::chassis(&g, full);
            hw::screw(&g, pos2(12.0, 12.0), 5.5, 20.0);
            hw::screw(&g, pos2(full.right() - 12.0, 12.0), 5.5, -35.0);
            hw::text(
                &g,
                pos2(30.0, 12.0),
                Align2::LEFT_CENTER,
                "PRO WAVE",
                Face::SilkX,
                15.0,
                theme::INK,
            );
            g.rect_filled(
                Rect::from_min_size(pos2(128.0, 5.0), vec2(14.0, 14.0)),
                1.0,
                theme::ORANGE,
            );
            hw::silk(
                &g,
                pos2(150.0, 12.0),
                Align2::LEFT_CENTER,
                "PW-1 digital audio workstation",
                11.0,
                theme::INK2,
            );
            hw::seam_h(&g, full.x_range(), 26.0);
            // Keys.
            hw::bracket(&g, 20.0, 330.0, 44.0, "View");
            let id = ui.scope_id().with("kit");
            for (i, (cap, down, led)) in [
                (Cap::Bone, true, Some(true)),
                (Cap::Bone, false, Some(false)),
                (Cap::Dark, false, None),
                (Cap::Orange, false, None),
                (Cap::Dark, true, None),
            ]
            .into_iter()
            .enumerate()
            {
                let r = Rect::from_min_size(pos2(20.0 + i as f32 * 62.0, 60.0), vec2(52.0, 52.0));
                hw::key(ui, &g, r, id.with(i), cap, down, led, Legend::Text("Rec"));
                hw::silk(
                    &g,
                    pos2(r.center().x, 126.0),
                    Align2::CENTER_CENTER,
                    "Caption",
                    9.5,
                    theme::PRINT,
                );
            }
            // LCD.
            let glass = hw::lcd(
                &g,
                Rect::from_min_size(pos2(350.0, 40.0), vec2(470.0, 96.0)),
            );
            hw::text(
                &g,
                glass.left_top() + vec2(12.0, 10.0),
                Align2::LEFT_TOP,
                "BAR",
                Face::Lcd,
                9.0,
                theme::LCD_INK,
            );
            hw::seg(
                &g,
                glass.left_top() + vec2(60.0, 14.0),
                "005 1 1 001",
                40.0,
                theme::LCD_INK,
                true,
            );
            hw::seg(
                &g,
                glass.left_top() + vec2(60.0, 64.0),
                "01:00:08:00.00",
                14.0,
                theme::LCD_INK,
                true,
            );
            hw::seg(
                &g,
                glass.left_top() + vec2(330.0, 20.0),
                "120.0000",
                20.0,
                theme::LCD_INK,
                true,
            );
            hw::lcd_readout(
                &g,
                Rect::from_min_size(pos2(840.0, 50.0), vec2(96.0, 34.0)),
                "Vol",
                "-3.2",
            );
            hw::lcd_readout(
                &g,
                Rect::from_min_size(pos2(944.0, 50.0), vec2(96.0, 34.0)),
                "Peak",
                "-0.9",
            );
            // Toggles, LEDs, track pads.
            hw::toggle(
                ui,
                &g,
                Rect::from_min_size(pos2(840.0, 100.0), vec2(60.0, 22.0)),
                id.with("m"),
                true,
                "M",
            );
            hw::toggle(
                ui,
                &g,
                Rect::from_min_size(pos2(906.0, 100.0), vec2(60.0, 22.0)),
                id.with("s"),
                false,
                "S",
            );
            hw::led(&g, pos2(990.0, 111.0), true);
            hw::led(&g, pos2(1006.0, 111.0), false);
            hw::seam_h(&g, full.x_range(), 150.0);
            hw::track_pad(
                &g,
                Rect::from_min_size(pos2(20.0, 170.0), vec2(46.0, 46.0)),
                1,
                true,
                false,
            );
            hw::track_pad(
                &g,
                Rect::from_min_size(pos2(76.0, 170.0), vec2(46.0, 46.0)),
                2,
                false,
                false,
            );
            hw::text(
                &g,
                pos2(134.0, 182.0),
                Align2::LEFT_CENTER,
                "CLASSIC ELECTRIC PIANO",
                Face::Bold,
                13.0,
                theme::INK,
            );
            hw::slider_like(
                &g,
                Rect::from_min_size(pos2(134.0, 200.0), vec2(90.0, 14.0)),
            );
            hw::paint_knob(&g, pos2(260.0, 196.0), 12.0, 0.4);
            // Fields and slots.
            hw::paint_field(
                &g,
                Rect::from_min_size(pos2(300.0, 170.0), vec2(150.0, 20.0)),
                "Equal Power",
                true,
                false,
            );
            hw::paint_field(
                &g,
                Rect::from_min_size(pos2(300.0, 196.0), vec2(150.0, 20.0)),
                "0 dB",
                false,
                false,
            );
            for (i, (kind, label, led)) in [
                (SlotKind::Plain, "Setting", None),
                (SlotKind::Filled, "Tape Delay", Some(true)),
                (SlotKind::Filled, "Channel EQ", Some(false)),
                (SlotKind::Empty, "", None),
            ]
            .into_iter()
            .enumerate()
            {
                hw::paint_slot(
                    &g,
                    Rect::from_min_size(pos2(470.0, 166.0 + i as f32 * 26.0), vec2(170.0, 22.0)),
                    kind,
                    label,
                    led,
                    kind == SlotKind::Plain,
                    false,
                );
            }
            // Regions.
            let tracks = [
                (
                    theme::TRACK_PALETTE[0],
                    false,
                    true,
                    "IN",
                    "01 Classic Electric Piano",
                ),
                (
                    theme::TRACK_PALETTE[1],
                    false,
                    false,
                    "IN",
                    "02 Steinway Grand Piano",
                ),
                (
                    theme::TRACK_PALETTE[2],
                    true,
                    false,
                    "AU",
                    "05 Electric Bass",
                ),
            ];
            for (i, (c, audio, sel, tag, name)) in tracks.into_iter().enumerate() {
                let r =
                    Rect::from_min_size(pos2(660.0, 166.0 + i as f32 * 52.0), vec2(520.0, 46.0));
                let look = theme::region_look(c, audio, sel, false);
                let body = hw::paint_region(&g, r, &look, tag, name);
                for k in 0..30 {
                    let x = body.left() + 6.0 + k as f32 * 17.0;
                    let y = body.top() + 4.0 + ((k * 7) % 20) as f32;
                    g.rect_filled(
                        Rect::from_min_size(pos2(x, y), vec2(12.0, 3.0)),
                        0.0,
                        look.print,
                    );
                }
            }
            // Faceplate with a fader, scale and meter.
            for (i, lit) in [true, false].into_iter().enumerate() {
                let plate =
                    Rect::from_min_size(pos2(20.0 + i as f32 * 220.0, 250.0), vec2(200.0, 430.0));
                hw::faceplate(&g, plate, 4.0, lit);
                hw::paint_slot(
                    &g,
                    Rect::from_min_size(plate.min + vec2(10.0, 14.0), vec2(180.0, 22.0)),
                    SlotKind::Plain,
                    "Setting",
                    None,
                    true,
                    false,
                );
                hw::paint_knob(
                    &g,
                    plate.center_top() + vec2(0.0, 70.0),
                    14.0,
                    -0.3 + i as f32 * 0.6,
                );
                let track = Rect::from_min_size(plate.min + vec2(24.0, 110.0), vec2(30.0, 260.0));
                hw::paint_fader_v(&g, track, 0.62, vec2(30.0, 18.0));
                let marks = [
                    (1.0, "+6"),
                    (0.9, "+3"),
                    (0.8, "0"),
                    (0.6, "6"),
                    (0.4, "15"),
                    (0.2, "30"),
                    (0.0, "60"),
                ];
                hw::scale_v(
                    &g,
                    Rect::from_min_size(plate.min + vec2(60.0, 110.0), vec2(40.0, 260.0)),
                    &marks,
                    true,
                );
                hw::paint_meter_v(
                    &g,
                    Rect::from_min_size(plate.min + vec2(120.0, 110.0), vec2(12.0, 260.0)),
                    0.74,
                    0.8,
                );
                hw::paint_meter_v(
                    &g,
                    Rect::from_min_size(plate.min + vec2(136.0, 110.0), vec2(12.0, 260.0)),
                    0.9,
                    0.93,
                );
                hw::paint_slot(
                    &g,
                    Rect::from_min_size(plate.min + vec2(10.0, 395.0), vec2(180.0, 24.0)),
                    if lit {
                        SlotKind::Filled
                    } else {
                        SlotKind::Plain
                    },
                    "01 Classic Electric Piano",
                    None,
                    false,
                    false,
                );
            }
            hw::paint_meter_h(
                &g,
                Rect::from_min_size(pos2(470.0, 280.0), vec2(170.0, 10.0)),
                0.8,
                0.85,
            );
            hw::recess(
                &g,
                Rect::from_min_size(pos2(470.0, 300.0), vec2(170.0, 60.0)),
                3.0,
            );
            hw::seam_v(&g, 455.0, 250.0..=680.0);
        },
        &out("hw-kit.png"),
    )
    .unwrap();
}
