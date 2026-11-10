//! Pictures of every effect and plug-in window as PW-1 hardware. Ignored; they check nothing:
//! `FX_PNG_DIR=/some/dir cargo test -p prowave-app fx_pictures -- --ignored`
use crate::model::{Insert, Project, TrackKind};
use egui::Color32;
use prowave_engine::FxKind;
use std::path::PathBuf;

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("FX_PNG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("prowave-fx"));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

/// The floor the windows float over (the arrange floor of the A design).
const FLOOR: Color32 = Color32::from_rgb(0xCF, 0xC8, 0xB8);

/// Whether picture `name` is wanted (`FX_ONLY=a,b` limits them to names holding a or b).
fn wanted(name: &str) -> bool {
    match std::env::var("FX_ONLY") {
        Ok(only) => only.split(',').any(|o| name.contains(o)),
        Err(_) => true,
    }
}

fn file_name(kind: FxKind) -> String {
    kind.name().to_lowercase().replace([' ', '-'], "")
}

#[test]
#[ignore = "pictures of the effect windows"]
fn fx_pictures_built_in_effects() {
    for kind in [
        FxKind::ChannelEq,
        FxKind::MultiEq,
        FxKind::Compressor,
        FxKind::Gate,
        FxKind::Limiter,
        FxKind::Delay,
        FxKind::Chorus,
        FxKind::Phaser,
        FxKind::Overdrive,
        FxKind::Filter,
        FxKind::Amp,
        FxKind::Reverb,
    ] {
        if !wanted(&file_name(kind)) {
            continue;
        }
        let mut p = Project::empty();
        let i = p.add_track(TrackKind::Audio);
        p.tracks[i].name = "Lead Vocal".into();
        let mut ins = Insert::new(kind);
        // Settings that show the display at work.
        match kind {
            FxKind::ChannelEq => {
                ins.params[0] = 4.5;
                ins.params[1] = -6.0;
                ins.params[4] = 3.0;
            }
            FxKind::MultiEq => {
                ins.params[1] = -4.0;
                ins.params[3] = 3.5;
                ins.params[6] = -7.0;
                ins.params[12] = 5.0;
                ins.params[15] = 2.0;
            }
            FxKind::Delay => ins.params[3] = 1.0,
            _ => {}
        }
        p.tracks[i].inserts = vec![ins];
        let mut s = crate::UiState::new();
        s.mixer.fx_window = Some((i, 0));
        let mut reduction = vec![0.0; i + 1];
        reduction[i] = 6.5;
        let meters = crate::mixer::Meters {
            reduction,
            fx_input: vec![0.4; i + 1],
            ..Default::default()
        };
        let (w, h) = match kind {
            FxKind::Reverb => (1000.0, 860.0),
            FxKind::MultiEq => (760.0, 940.0),
            _ => (720.0, 600.0),
        };
        crate::ui_test::render_png(
            w,
            h,
            2.0,
            FLOOR,
            |ui| {
                crate::theme::apply(ui.ctx());
                crate::fx_ui::show(ui.ctx(), &mut p, &mut s, &meters)
            },
            &out(&format!("fx-{}.png", file_name(kind))),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "pictures of the effect windows"]
fn fx_pictures_native_effects() {
    for info in prowave_engine::native_effects() {
        let name = info.id.trim_start_matches("prowave:").to_string();
        if !wanted(&name) {
            continue;
        }
        let mut p = Project::empty();
        let i = p.add_track(TrackKind::Audio);
        p.tracks[i].name = "Drum Bus".into();
        let slot = crate::native_fx_ui::new_slot(info);
        let uid = slot.uid;
        p.tracks[i].au_effects.push(slot);
        let mut s = crate::UiState::new();
        s.native_window = Some(uid);
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
            &out(&format!("native-{name}.png")),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "pictures of the effect windows"]
fn fx_pictures_midi_fx() {
    use crate::midi_fx::{FxSlot, MidiFx};
    let mut p = Project::empty();
    let i = p.add_track(TrackKind::Instrument);
    p.tracks[i].name = "Classic Electric Piano".into();
    p.selected_track = i;
    p.tracks[i].midi_fx = MidiFx::all_defaults()
        .into_iter()
        .enumerate()
        .map(|(k, fx)| FxSlot { fx, on: k != 3 })
        .collect();
    let mut s = crate::UiState::new();
    s.show_midi_fx = true;
    crate::ui_test::render_png(
        840.0,
        2140.0,
        2.0,
        FLOOR,
        |ui| {
            crate::theme::apply(ui.ctx());
            crate::midi_fx_ui::show(ui.ctx(), &mut p, &mut s)
        },
        &out("midi-fx.png"),
    )
    .unwrap();
}
