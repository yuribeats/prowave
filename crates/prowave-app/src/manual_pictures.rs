//! Pictures of windows for the manual (docs/manual). Ignored; they check nothing:
//! `MANUAL_PNG_DIR=docs/manual/assets cargo test -p prowave-app manual_pictures -- --ignored`
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
        crate::ui_test::render_png(
            560.0,
            1300.0,
            1.0,
            BG,
            |ui| {
                crate::theme::apply(ui.ctx());
                crate::native_fx_ui::show(ui.ctx(), &mut p, &mut s)
            },
            &out(&file),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "pictures for the manual"]
fn manual_pictures_space_designer() {
    use crate::model::Insert;
    use prowave_engine::FxKind;
    let p = Project::demo();
    let insert = Insert::new(FxKind::Reverb);
    let track = p.tracks[0].clone();
    crate::ui_test::render_png(
        1000.0,
        700.0,
        1.0,
        BG,
        |ui| {
            crate::theme::apply(ui.ctx());
            let _ = crate::space_designer_ui::window(
                ui.ctx(),
                &insert,
                &track,
                0,
                0.0,
                egui::Id::unique("manual_space_designer"),
                0.0,
            );
        },
        &out("space-designer.png"),
    )
    .unwrap();
}

#[test]
#[ignore = "pictures for the manual"]
fn manual_pictures_small_windows() {
    let mut t = crate::musical_typing::Typing::default();
    t.open = true;
    crate::ui_test::render_png(
        420.0,
        260.0,
        1.0,
        BG,
        |ui| {
            crate::theme::apply(ui.ctx());
            crate::musical_typing::show(ui.ctx(), &mut t)
        },
        &out("musical-typing.png"),
    )
    .unwrap();
    let p = Project::demo();
    let mut s = crate::UiState::new();
    s.show_midi_monitor = true;
    let mut test_note = None;
    crate::ui_test::render_png(
        900.0,
        560.0,
        1.0,
        BG,
        |ui| {
            crate::theme::apply(ui.ctx());
            crate::midi_monitor::show(ui.ctx(), &p, &mut s, &mut test_note)
        },
        &out("midi-monitor.png"),
    )
    .unwrap();
}
