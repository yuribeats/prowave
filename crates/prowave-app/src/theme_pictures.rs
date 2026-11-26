//! Pictures of the main window in each look, with the walnut cheeks (`PROWAVE_UI_PNG_DIR`, else
//! the temp folder): `PROWAVE_UI_PNG_DIR=… cargo test -p prowave-app theme_pictures -- --ignored`.
use crate::theme::{self, Theme};
use std::path::PathBuf;

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("PROWAVE_UI_PNG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("prowave-theme"));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

/// The main window as the app lays it out: the cheeks, the control bar, the Tracks area and the
/// Mixer under it.
fn main_window(look: Theme, name: &str, width: f32) {
    theme::set_for_thread(Some(look));
    theme::set_wood(true);
    let mut p = crate::model::Project::demo();
    let mut s = crate::UiState::new();
    s.show_mixer = true;
    s.bottom = crate::Bottom::Mixer;
    s.tracks.select_only(0, 0);
    let meters = crate::mixer::Meters {
        tracks: (0..p.tracks.len())
            .map(|i| (0.55 + 0.05 * (i % 5) as f32, 0.5 + 0.06 * (i % 6) as f32))
            .collect(),
        master: (0.82, 0.9),
        ..Default::default()
    };
    crate::ui_test::render_png(
        width,
        900.0,
        2.0,
        theme::BG(),
        |ui| {
            theme::apply(ui.ctx());
            crate::wood::cheeks(ui);
            egui::Panel::top("control")
                .exact_size(crate::transport::HEIGHT)
                .frame(egui::Frame::new().fill(theme::TOOLBAR()))
                .show(ui, |ui| {
                    crate::transport::control_bar(ui, &mut p, &mut s, Some((0.7, 0.8)))
                });
            // The Mixer opens as the app opens it: whole strips, the Tracks area the rest.
            crate::bottom_pane::show(ui, &mut p, &mut s, &meters);
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ui, |ui| {
                    crate::tracks_area::show(ui, &mut p, &mut s, &meters)
                });
        },
        &out(name),
    )
    .unwrap();
    theme::set_for_thread(None);
}

#[test]
#[ignore = "pictures"]
fn theme_pictures_main_window() {
    main_window(Theme::Graphite, "main-graphite-amber.png", 1470.0);
    main_window(Theme::Bone, "main-bone.png", 1470.0);
    // A window narrower than the cheeks' threshold goes without them.
    main_window(Theme::Graphite, "main-graphite-amber-narrow.png", 960.0);
}
