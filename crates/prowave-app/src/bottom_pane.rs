//! How tall the pane under the Tracks area opens in the main window (the Mixer, Piano Roll,
//! Channel Rack or Pitch Editor), and the grab handle on its top edge that drags it taller or
//! shorter. A dragged height is kept per pane with the project (`Project::pane_heights`);
//! a double-click on the handle gives the pane back its own height.
use crate::{mixer, theme, tracks_area, Bottom, Project, UiState};
use egui::{pos2, vec2, CursorIcon, Rect, Sense, Stroke, Ui};

/// The least the Tracks area keeps over the Mixer: its menu row, the ruler and one track row.
pub const ARRANGE_MIN: f32 =
    tracks_area::TOOLBAR_H + tracks_area::RULER_H + tracks_area::DEFAULT_ROW_H;
/// The shortest the pane gets.
pub const PANE_MIN: f32 = 160.0;
/// The grab handle across the pane's top edge.
pub const HANDLE_H: f32 = 12.0;
/// The grip in the middle of the handle.
const GRIP_W: f32 = 96.0;

/// The name a pane's dragged height is kept under.
pub fn key(b: Bottom) -> &'static str {
    match b {
        Bottom::Mixer => "mixer",
        Bottom::PianoRoll => "piano_roll",
        Bottom::ChannelRack => "channel_rack",
        Bottom::PitchEditor => "pitch_editor",
    }
}

/// The pane's own height in `room` points under the control bar, `width` points wide, the handle included. The
/// Mixer is as tall as its channel strips shown whole (`mixer::full_height`) and the Tracks
/// area gets the rest; a window too short for that keeps the strips whole and shrinks the
/// Tracks area down to `ARRANGE_MIN` (the strips then drop to their compact density). The
/// Channel Rack opens with its head and every channel's row shown (`gap`: egui's item
/// spacing), within the same limits; the other editors at about half the room, 700 points at
/// most.
pub fn auto_height(room: f32, width: f32, p: &Project, s: &UiState, gap: f32) -> f32 {
    let own = match s.bottom {
        Bottom::Mixer => Some(mixer::full_height(p, &s.mixer)),
        Bottom::ChannelRack => crate::channel_rack::full_height(p, gap, width),
        _ => None,
    };
    match own {
        Some(h) => (h + HANDLE_H).min(room - ARRANGE_MIN).max(PANE_MIN),
        None => (room * 0.48).clamp(PANE_MIN, 700.0),
    }
}

/// The pane's height now: the one it was dragged to (held between `PANE_MIN` and what leaves
/// the Tracks area its least), else its own.
pub fn height(room: f32, width: f32, p: &Project, s: &UiState, gap: f32) -> f32 {
    match p.pane_heights.get(key(s.bottom)) {
        Some(&h) => h.min(room - ARRANGE_MIN).max(PANE_MIN),
        None => auto_height(room, width, p, s, gap),
    }
}

/// The pane at the bottom of `ui` (the Mixer or the editor `s.bottom` names) at [`height`],
/// with its grab handle on top. The Mixer has a panel of its own so it never takes an editor's
/// size.
///
/// The pane's contents are drawn in a child of the panel that cannot grow it: contents taller
/// than the pane are cut off at its bottom. (Grown, egui keeps the panel at its size but moves
/// its top down by the overflow, so the Tracks area, laid out after it, would cover the
/// handle and the pane's head rows and take their clicks.)
pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState, meters: &mixer::Meters) {
    let room = ui.available_height();
    let height = height(
        room,
        ui.available_width(),
        p,
        s,
        ui.spacing().item_spacing.y,
    );
    let id = if s.bottom == Bottom::Mixer {
        "bottom_mixer"
    } else {
        "bottom"
    };
    egui::Panel::bottom(id)
        .resizable(false)
        .show_separator_line(false)
        .exact_size(height)
        .frame(egui::Frame::new())
        .show(ui, |ui| {
            let all = ui.max_rect();
            let rest = handle(ui, p, s, room, height);
            let mut pane = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("pane")
                    .max_rect(rest)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            pane.shrink_clip_rect(rest);
            match s.bottom {
                Bottom::Mixer => mixer::show(&mut pane, p, s, meters),
                Bottom::PianoRoll => crate::piano_roll::show(&mut pane, p, s),
                Bottom::ChannelRack => crate::channel_rack::show(&mut pane, p, s),
                Bottom::PitchEditor => crate::pitch_editor::show(&mut pane, p, s),
            }
            // The panel is the pane's height whatever its contents asked for.
            ui.expand_to_include_rect(all);
        });
}

/// The grab handle across the top of the pane in `ui` (which is `height` tall, in `room`
/// points under the control bar): drag it to make the pane taller or shorter, double-click it
/// for the pane's own height. Returns the rest of the pane, under the handle.
pub fn handle(ui: &mut Ui, p: &mut Project, s: &UiState, room: f32, height: f32) -> Rect {
    let all = ui.max_rect();
    let bar = Rect::from_min_size(all.min, vec2(all.width(), HANDLE_H));
    let id = ui.scope_id().with("pane_split").with(key(s.bottom));
    let resp = ui
        .interact(bar, id, Sense::click_and_drag())
        .on_hover_cursor(CursorIcon::ResizeVertical);
    if resp.double_clicked() {
        p.pane_heights.remove(key(s.bottom));
    } else if resp.dragged() {
        let dy = resp.drag_delta().y;
        if dy != 0.0 {
            let h = (height - dy).min(room - ARRANGE_MIN).max(PANE_MIN);
            p.pane_heights.insert(key(s.bottom).to_string(), h);
        }
    }
    if resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    paint_handle(ui, bar, resp.hovered() || resp.dragged());
    Rect::from_min_max(pos2(all.left(), bar.bottom()), all.max)
}

/// The handle: a raised bar in key-face bone across the pane (a highlight on top, a dark seam
/// under it) with a black grip cap in the middle ruled with three lines; the bar brightens
/// while the pointer is on it or drags it.
fn paint_handle(ui: &Ui, bar: Rect, hot: bool) {
    let g = ui.painter_at(bar);
    g.rect_filled(
        bar,
        0.0,
        if hot {
            theme::BONE_TOP()
        } else {
            theme::BONE_HI()
        },
    );
    g.hline(
        bar.x_range(),
        bar.top() + 0.5,
        Stroke::new(1.0, theme::emboss(140)),
    );
    g.hline(
        bar.x_range(),
        bar.bottom() - 0.5,
        Stroke::new(1.0, theme::SEAM_DK()),
    );
    let cap = Rect::from_center_size(bar.center(), vec2(GRIP_W, HANDLE_H - 4.0));
    g.rect_filled(cap, 2.0, theme::CAP());
    let ridge = theme::ON_DARK();
    for dy in [-2.0, 0.0, 2.0] {
        let y = cap.center().y + dy + 0.5;
        g.hline(
            (cap.left() + 14.0)..=(cap.right() - 14.0),
            y,
            Stroke::new(1.0, ridge),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The item spacing the app's theme sets (`theme::apply`).
    const GAP: f32 = 4.0;

    #[test]
    fn the_mixer_opens_as_tall_as_its_strips_and_the_tracks_keep_their_least() {
        let p = Project::demo();
        let mut s = UiState::new();
        s.bottom = Bottom::Mixer;
        let wd = 1200.0;
        let full = mixer::full_height(&p, &s.mixer) + HANDLE_H;
        // A tall window: the whole strip, the rest to the Tracks area.
        assert_eq!(height(2000.0, wd, &p, &s, GAP), full);
        assert_eq!(height(full + ARRANGE_MIN, wd, &p, &s, GAP), full);
        // A short one: the Tracks area shrinks to its least, the Mixer takes the rest.
        let room = full + ARRANGE_MIN - 100.0;
        assert_eq!(room - height(room, wd, &p, &s, GAP), ARRANGE_MIN);
        // Too short even for that: the pane keeps its own least.
        assert_eq!(height(250.0, wd, &p, &s, GAP), PANE_MIN);
        // The editors open at about half the room.
        s.bottom = Bottom::PianoRoll;
        assert_eq!(height(1000.0, wd, &p, &s, GAP), 480.0);
        assert_eq!(height(2000.0, wd, &p, &s, GAP), 700.0);
    }

    /// The handle drags the Mixer shorter (kept with the project under "mixer", within its
    /// range); a double-click gives it its own height back. Saved and opened with the project,
    /// and not counted as an unsaved change.
    #[test]
    fn the_handle_drags_the_pane_and_a_double_click_resets_it() {
        let mut p = Project::demo();
        let mut s = UiState::new();
        s.bottom = Bottom::Mixer;
        s.show_mixer = true;
        let meters = mixer::Meters::default();
        let mut h = crate::ui_test::Harness::new(1400.0, 1000.0);
        let wd = 1400.0;
        let auto = auto_height(1000.0, wd, &p, &s, GAP);
        let frame = |ui: &mut Ui, p: &mut Project, s: &mut UiState| {
            show(ui, p, s, &meters);
            egui::CentralPanel::default().show(ui, |_| {});
        };
        h.settle(|ui| frame(ui, &mut p, &mut s));
        let top = 1000.0 - auto + HANDLE_H / 2.0;
        h.drag(
            &[
                pos2(700.0, top),
                pos2(700.0, top + 60.0),
                pos2(700.0, top + 120.0),
            ],
            egui::PointerButton::Primary,
            |ui| frame(ui, &mut p, &mut s),
        );
        let dragged = p.pane_heights["mixer"];
        assert!(
            (dragged - (auto - 120.0)).abs() < 1.0,
            "{dragged} vs {auto}"
        );
        assert_eq!(height(1000.0, wd, &p, &s, GAP), dragged);
        // Never under the pane's least, nor over what leaves the Tracks area its least.
        p.pane_heights.insert("mixer".into(), 20.0);
        assert_eq!(height(1000.0, wd, &p, &s, GAP), PANE_MIN);
        p.pane_heights.insert("mixer".into(), 5000.0);
        assert_eq!(height(1000.0, wd, &p, &s, GAP), 1000.0 - ARRANGE_MIN);
        p.pane_heights.insert("mixer".into(), dragged);
        // The editors keep their own.
        s.bottom = Bottom::PianoRoll;
        assert_eq!(height(1000.0, wd, &p, &s, GAP), 480.0);
        s.bottom = Bottom::Mixer;
        // Saved with the project, read back, and no "save changes?" for it alone.
        let saved = crate::file_commands::content(&p);
        let (doc, _) = crate::project_io::to_doc(&p);
        assert_eq!(doc.pane_heights.get("mixer"), Some(&dragged));
        let mut q = p.clone();
        q.pane_heights.clear();
        assert_eq!(crate::file_commands::content(&q), saved);
        // A double-click on the handle: its own height again.
        let top = 1000.0 - dragged + HANDLE_H / 2.0;
        h.time_passes(1.0);
        h.settle(|ui| frame(ui, &mut p, &mut s));
        h.click(pos2(700.0, top), |ui| frame(ui, &mut p, &mut s));
        h.click(pos2(700.0, top), |ui| frame(ui, &mut p, &mut s));
        assert!(!p.pane_heights.contains_key("mixer"));
        assert_eq!(height(1000.0, wd, &p, &s, GAP), auto);
    }

    /// The main window with the Mixer dragged short: `main-window-mixer-dragged.png`, the
    /// strips compact with their top rows behind the wheel.
    #[test]
    #[ignore = "a picture; checks nothing"]
    fn main_window_mixer_dragged_picture() {
        main_window_picture(Some(300.0), "main-window-mixer-dragged.png");
    }

    /// The main window at a laptop's size with the Mixer open: control bar, Tracks area and
    /// the Mixer at its opening height. Writes `main-window-mixer.png` into
    /// `PROWAVE_UI_PNG_DIR` (or the temp folder).
    #[test]
    #[ignore = "a picture; checks nothing"]
    fn main_window_mixer_picture() {
        main_window_picture(None, "main-window-mixer.png");
    }

    fn main_window_picture(dragged: Option<f32>, name: &str) {
        let mut p = Project::demo();
        let mut s = UiState::new();
        s.bottom = Bottom::Mixer;
        s.show_mixer = true;
        if let Some(h) = dragged {
            p.pane_heights.insert("mixer".into(), h);
        }
        let meters = mixer::Meters::default();
        let dir = std::env::var_os("PROWAVE_UI_PNG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let _ = std::fs::create_dir_all(&dir);
        crate::ui_test::render_png(
            1470.0,
            920.0,
            1.0,
            crate::theme::BG(),
            |ui| {
                crate::theme::apply(ui.ctx());
                egui::Panel::top("control")
                    .exact_size(crate::transport::HEIGHT)
                    .frame(egui::Frame::new().fill(crate::theme::TOOLBAR()))
                    .show(ui, |ui| {
                        crate::transport::control_bar(ui, &mut p, &mut s, None)
                    });
                show(ui, &mut p, &mut s, &meters);
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ui, |ui| tracks_area::show(ui, &mut p, &mut s, &meters));
            },
            &dir.join(name),
        )
        .unwrap();
        eprintln!(
            "mixer pane {} of {} points (full strip {})",
            height(920.0 - crate::transport::HEIGHT, 1226.0, &p, &s, GAP),
            920.0 - crate::transport::HEIGHT,
            mixer::full_height(&p, &s.mixer)
        );
    }

    /// The main window as the app lays it out (control bar, Inspector, the pane, the Tracks
    /// area). Returns the Tracks area's rect.
    fn app_frame(ui: &mut Ui, p: &mut Project, s: &mut UiState, meters: &mixer::Meters) -> Rect {
        crate::theme::apply(ui.ctx());
        egui::Panel::top("control")
            .exact_size(crate::transport::HEIGHT)
            .frame(egui::Frame::new().fill(crate::theme::TOOLBAR()))
            .show(ui, |ui| crate::transport::control_bar(ui, p, s, None));
        if s.show_inspector {
            egui::Panel::left("inspector")
                .exact_size(244.0)
                .frame(egui::Frame::new())
                .show(ui, |ui| crate::inspector::show(ui, p, s, meters));
        }
        show(ui, p, s, meters);
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| tracks_area::show(ui, p, s, meters))
            .response
            .rect
    }

    const SIZES: [(f32, f32); 4] = [
        (1470.0, 920.0),
        (1512.0, 982.0),
        (1920.0, 1080.0),
        (2560.0, 1440.0),
    ];
    const PANES: [Bottom; 4] = [
        Bottom::ChannelRack,
        Bottom::PianoRoll,
        Bottom::PitchEditor,
        Bottom::Mixer,
    ];

    /// What would take a press at `pos`: the widget that gets a drag and the one that gets a
    /// click (their rects).
    fn hits_at(
        h: &mut crate::ui_test::Harness,
        pos: egui::Pos2,
        frame: impl FnMut(&mut Ui),
    ) -> (Option<Rect>, Option<Rect>) {
        h.move_to(pos, frame);
        h.ctx
            .viewport(|v| (v.hits.drag.map(|w| w.rect), v.hits.click.map(|w| w.rect)))
    }

    /// The bug of 2026-11-25: a Channel Rack with eight channels in a short pane was taller
    /// than the pane, egui moved the panel's top down by the difference, and the Tracks area
    /// covered the handle and the rack's head (Kits, Groove, Pattern): the handle could not be
    /// grabbed and the head's keys not clicked. Now the Tracks area ends where the pane
    /// begins, the handle takes the drag and the Kits key the click, at every window size and
    /// pane height.
    #[test]
    fn a_pane_taller_than_its_room_never_hides_its_handle_or_head() {
        for (w, wh) in SIZES {
            for dragged in [None, Some(PANE_MIN), Some(240.0)] {
                let mut p = crate::channel_rack::picture_project(8);
                let mut s = UiState::new();
                s.bottom = Bottom::ChannelRack;
                if let Some(d) = dragged {
                    p.pane_heights.insert(key(Bottom::ChannelRack).into(), d);
                }
                let meters = mixer::Meters::default();
                let mut h = crate::ui_test::Harness::new(w, wh);
                let mut tracks = Rect::NOTHING;
                h.settle(|ui| tracks = app_frame(ui, &mut p, &mut s, &meters));
                let room = wh - crate::transport::HEIGHT;
                let wd = w - 244.0;
                let top = wh - height(room, wd, &p, &s, GAP);
                assert_eq!(tracks.bottom(), top, "{w}x{wh} {dragged:?}");
                // The handle takes a drag anywhere on it, also on its top and bottom rows.
                for dy in [1.0, HANDLE_H / 2.0, HANDLE_H - 1.0] {
                    let at = pos2(w * 0.6, top + dy);
                    let (drag, _) =
                        hits_at(&mut h, at, |ui| _ = app_frame(ui, &mut p, &mut s, &meters));
                    let drag = drag.unwrap_or(Rect::NOTHING);
                    assert_eq!(
                        (drag.top(), drag.height()),
                        (top, HANDLE_H),
                        "{w}x{wh} {dragged:?} handle at +{dy}"
                    );
                }
                // The Kits key at the right end of the rack's head takes a click.
                let kits = pos2(w - 14.0 - 38.0, top + HANDLE_H + 10.0 + 13.0);
                let (_, click) = hits_at(&mut h, kits, |ui| {
                    _ = app_frame(ui, &mut p, &mut s, &meters)
                });
                let click = click.unwrap_or(Rect::NOTHING);
                assert!(
                    click.contains(kits) && click.top() >= top + HANDLE_H,
                    "{w}x{wh} {dragged:?}: Kits key {click:?}"
                );
            }
        }
    }

    /// Every pane (Channel Rack, Piano Roll, Pitch Editor, Mixer) at every window size: the
    /// handle drags it up until the Tracks area keeps its least (its menu row, ruler and one
    /// track) and down to the pane's least; at the top the Tracks area's EDIT menu and the
    /// control bar still take clicks, and nothing of the Tracks area reaches into the pane.
    #[test]
    fn the_handle_drags_every_pane_between_its_limits_and_the_menus_stay_clickable() {
        for (w, wh) in SIZES {
            for b in PANES {
                let mut p = crate::channel_rack::picture_project(8);
                let mut s = UiState::new();
                s.bottom = b;
                s.show_mixer = b == Bottom::Mixer;
                let meters = mixer::Meters::default();
                let mut h = crate::ui_test::Harness::new(w, wh);
                let room = wh - crate::transport::HEIGHT;
                let wd = w - 244.0;
                let x = 244.0 + (w - 244.0) * 0.6;
                let mut tracks = Rect::NOTHING;
                h.settle(|ui| tracks = app_frame(ui, &mut p, &mut s, &meters));
                let name = format!("{w}x{wh} {}", key(b));
                // Up as far as it goes.
                let top = wh - height(room, wd, &p, &s, GAP) + HANDLE_H / 2.0;
                h.drag(
                    &[pos2(x, top), pos2(x, top - 30.0), pos2(x, 0.0)],
                    egui::PointerButton::Primary,
                    |ui| tracks = app_frame(ui, &mut p, &mut s, &meters),
                );
                assert_eq!(
                    height(room, wd, &p, &s, GAP),
                    room - ARRANGE_MIN,
                    "{name} up"
                );
                h.settle(|ui| tracks = app_frame(ui, &mut p, &mut s, &meters));
                assert_eq!(tracks.height(), ARRANGE_MIN, "{name} up");
                // The Tracks area's EDIT menu (left end of its menu row) and the control
                // bar's Play key take clicks.
                let edit = pos2(
                    tracks.left() + 40.0,
                    tracks.top() + tracks_area::TOOLBAR_H / 2.0,
                );
                let (_, click) = hits_at(&mut h, edit, |ui| {
                    _ = app_frame(ui, &mut p, &mut s, &meters)
                });
                let click = click.unwrap_or(Rect::NOTHING);
                assert!(
                    click.contains(edit) && click.bottom() <= tracks.top() + tracks_area::TOOLBAR_H,
                    "{name}: EDIT menu {click:?}"
                );
                let control = pos2(w / 2.0, crate::transport::HEIGHT / 2.0);
                h.move_to(control, |ui| _ = app_frame(ui, &mut p, &mut s, &meters));
                let inside = h.ctx.viewport(|v| {
                    v.hits
                        .contains_pointer
                        .iter()
                        .all(|w| w.rect.top() < crate::transport::HEIGHT)
                });
                assert!(inside, "{name}: the control bar is covered");
                // Just under the handle every widget is the pane's own.
                let under = pos2(x, tracks.bottom() + HANDLE_H + 2.0);
                h.move_to(under, |ui| _ = app_frame(ui, &mut p, &mut s, &meters));
                let other: Vec<_> = h.ctx.viewport(|v| {
                    v.hits
                        .contains_pointer
                        .iter()
                        // (Not the window's own background, which spans it all.)
                        .filter(|w| w.rect.top() < tracks.bottom() && w.rect.top() > 0.0)
                        .map(|w| w.rect)
                        .collect()
                });
                assert!(
                    other.is_empty(),
                    "{name}: the Tracks area reaches into the pane: {other:?} (pane at {})",
                    tracks.bottom()
                );
                // Down as far as it goes.
                let top = tracks.bottom() + HANDLE_H / 2.0;
                h.drag(
                    &[pos2(x, top), pos2(x, top + 30.0), pos2(x, wh)],
                    egui::PointerButton::Primary,
                    |ui| tracks = app_frame(ui, &mut p, &mut s, &meters),
                );
                assert_eq!(height(room, wd, &p, &s, GAP), PANE_MIN, "{name} down");
                h.settle(|ui| tracks = app_frame(ui, &mut p, &mut s, &meters));
                assert_eq!(tracks.bottom(), wh - PANE_MIN, "{name} down");
            }
        }
    }

    /// The Channel Rack opens with its head and every channel's row: `channel_rack::full_height`
    /// is what the rack takes laid out with room to spare, so its rows need no scrolling.
    #[test]
    fn the_channel_rack_opens_with_every_row_shown() {
        // 900 points wide, the rack's keys go over its amounts.
        for (n, wd) in [(1, 1470.0), (5, 1470.0), (8, 1470.0), (8, 900.0)] {
            let mut p = crate::channel_rack::picture_project(n);
            let mut s = UiState::new();
            s.bottom = Bottom::ChannelRack;
            let mut h = crate::ui_test::Harness::new(wd, 3000.0);
            let mut used = 0.0;
            h.settle(|ui| {
                crate::theme::apply(ui.ctx());
                used = ui
                    .scope(|ui| crate::channel_rack::show(ui, &mut p, &mut s))
                    .response
                    .rect
                    .height();
            });
            let full = crate::channel_rack::full_height(&p, GAP, wd).unwrap();
            assert!(
                (full - used).abs() < 1.0,
                "{n} channels, {wd}: {full} vs {used}"
            );
            assert_eq!(auto_height(2000.0, wd, &p, &s, GAP), full + HANDLE_H);
        }
        // On the laptop the eight rows fit with the Tracks area well over its least.
        let wd = 1470.0 - 244.0;
        let p = crate::channel_rack::picture_project(8);
        let mut s = UiState::new();
        s.bottom = Bottom::ChannelRack;
        let room = 920.0 - crate::transport::HEIGHT;
        let h = auto_height(room, wd, &p, &s, GAP);
        assert_eq!(
            h,
            crate::channel_rack::full_height(&p, GAP, wd).unwrap() + HANDLE_H
        );
        assert!(room - h > ARRANGE_MIN, "{h} of {room}");
    }

    /// Pictures of the main window with the Channel Rack (eight channels) at its own height,
    /// dragged small and dragged as tall as it goes, and the other panes dragged tall, at the
    /// laptop's size: `pane-<pane>-<size>.png` in `PROWAVE_UI_PNG_DIR` (or the temp folder).
    #[test]
    #[ignore = "pictures; checks nothing"]
    fn pane_pictures() {
        let dir = std::env::var_os("PROWAVE_UI_PNG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let _ = std::fs::create_dir_all(&dir);
        for (w, wh) in [(1470.0, 920.0), (1920.0, 1080.0)] {
            for (b, size) in [
                (Bottom::ChannelRack, "own"),
                (Bottom::ChannelRack, "small"),
                (Bottom::ChannelRack, "large"),
                (Bottom::PianoRoll, "large"),
                (Bottom::PitchEditor, "large"),
            ] {
                let mut p = crate::channel_rack::picture_project(8);
                // The snare's own Offset at 50 %, the hats' at 25 %.
                let ti = p.selected_track;
                let old = crate::channel_rack::rack_state(&p, ti).unwrap();
                let mut new = old.clone();
                new.groove
                    .set_offset(Some(crate::kit_classify::DrumClass::Snare), 0.5);
                new.groove
                    .set_offset(Some(crate::kit_classify::DrumClass::ClosedHat), 0.25);
                crate::channel_rack::commit(&mut p, ti, &old, new);
                let mut s = UiState::new();
                s.bottom = b;
                match size {
                    "small" => _ = p.pane_heights.insert(key(b).into(), PANE_MIN),
                    "large" => _ = p.pane_heights.insert(key(b).into(), 5000.0),
                    _ => {}
                }
                let meters = mixer::Meters::default();
                crate::ui_test::render_png(
                    w,
                    wh,
                    1.0,
                    crate::theme::BG(),
                    |ui| _ = app_frame(ui, &mut p, &mut s, &meters),
                    &dir.join(format!("pane-{}-{size}-{w}.png", key(b))),
                )
                .unwrap();
            }
        }
    }
}
