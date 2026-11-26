//! Navigate > Go to Marker Number… (Logic Pro: Go to Marker Number…, Option-Slash): a box that
//! takes a marker's number and moves the playhead to that marker.
//!
//! Markers are numbered from 1 in the order they stand in the song, as the Marker List counts
//! them. Logic also has Go to Marker Number 1 to 19 on the keypad's digits; egui gives the
//! keypad's digits as the top row's (see `key_commands`), so those are not offered.
//!
//! Moving the playhead does not change the project, so it is not an undo step.
use crate::hw::Cap;
use crate::i18n::tr;
use crate::model::Project;
use crate::theme::{self, Face};
use crate::track_dialogs::{dialog_key, key_w, TrackDialog};
use crate::UiState;
use egui::{vec2, Sense};

pub(crate) const LABEL: &str = "Go to Marker Number…";

/// Whether `label` is the entry run here.
pub(crate) fn handles(label: &str) -> bool {
    label == LABEL
}

/// Whether the entry can be chosen now (there is a marker); `None` when it is not this one.
pub(crate) fn enabled(label: &str, p: &Project) -> Option<bool> {
    handles(label).then_some(!p.markers.is_empty())
}

/// Opens the box; returns whether `label` was this entry.
pub(crate) fn apply(label: &str, p: &Project, s: &mut UiState) -> bool {
    if !handles(label) {
        return false;
    }
    if !p.markers.is_empty() {
        s.track_dialog = Some(TrackDialog::GoToMarker {
            text: String::new(),
        });
    }
    true
}

/// The beat of the marker whose number is typed in `text` (1 is the first), or `None` when
/// it is not the number of a marker.
pub(crate) fn marker_beat(p: &Project, text: &str) -> Option<f64> {
    let n: usize = text.trim().parse().ok()?;
    p.markers.get(n.checked_sub(1)?).map(|m| m.beat)
}

/// The box: the number in a dark field (Return goes there), then OK and Cancel.
pub(crate) fn body(ui: &mut egui::Ui, p: &mut Project, text: &mut String, done: &mut bool) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    crate::hw::paint_field(ui.painter(), r, "", false, false);
    let field = ui.put(
        r.shrink2(vec2(4.0, 0.0)),
        egui::TextEdit::singleline(text)
            .char_limit(6)
            .font(theme::font(Face::Bold, 13.0))
            .text_color(theme::ON_DARK())
            .frame(egui::Frame::NONE)
            .margin(vec2(6.0, 6.0))
            .desired_width(r.width() - 8.0)
            .min_size(vec2(r.width() - 8.0, r.height()))
            .align(egui::Align2::LEFT_CENTER),
    );
    // The field has the keyboard as soon as the box opens, so the number can be typed.
    let opened = egui::Id::unique("go_to_marker_number_opened");
    if !ui.data(|d| d.get_temp::<bool>(opened).unwrap_or(false)) {
        field.request_focus();
        ui.data_mut(|d| d.insert_temp(opened, true));
    }
    let at = marker_beat(p, text);
    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
    ui.add_space(4.0);
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 30.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            let cancel = tr("Cancel");
            if dialog_key(ui, "cancel", cancel, key_w(cancel), Cap::Bone, false, false) || escaped {
                *done = true;
            }
            let ok_label = tr("OK");
            let ok_clicked = ui
                .add_enabled_ui(at.is_some(), |ui| {
                    dialog_key(ui, "ok", ok_label, key_w(ok_label), Cap::Dark, false, false)
                })
                .inner;
            if let (true, Some(at)) = (ok_clicked || entered, at) {
                p.position_beats = at;
                *done = true;
            }
        },
    );
    if *done {
        ui.data_mut(|d| d.remove::<bool>(opened));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_test::Harness;
    use egui::{Event, Key, Modifiers};

    /// Markers at bars 9, 3 and 5 (numbered 1, 2, 3 in song order: bar 3, 5, 9).
    fn rig() -> (Project, UiState) {
        let mut p = Project::empty();
        for beat in [32.0, 8.0, 16.0] {
            p.add_marker(beat);
        }
        assert_eq!(p.markers.len(), 3);
        (p, UiState::new())
    }

    /// One frame as the app runs it: the keys, then the Track menu's windows (this box among them).
    fn frame(h: &mut Harness, p: &mut Project, s: &mut UiState, events: Vec<Event>) {
        h.frame(events, |ui| {
            let ctx = ui.ctx().clone();
            crate::key_commands::begin_frame(&ctx);
            crate::menus::handle_keys(&ctx, p, s);
            crate::key_commands::handle(&ctx, p, s);
            crate::track_dialogs::show(&ctx, p, s);
        });
    }

    fn key(key: Key, mods: Modifiers) -> Vec<Event> {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: mods,
        };
        vec![ev(true), ev(false)]
    }

    fn press(h: &mut Harness, p: &mut Project, s: &mut UiState, k: Key, mods: Modifiers) {
        h.modifiers = mods;
        frame(h, p, s, key(k, mods));
        h.modifiers = Modifiers::NONE;
        frame(h, p, s, vec![]);
    }

    #[test]
    fn marker_numbers_count_in_song_order() {
        let (p, _) = rig();
        assert_eq!(marker_beat(&p, "1"), Some(8.0));
        assert_eq!(marker_beat(&p, " 2 "), Some(16.0));
        assert_eq!(marker_beat(&p, "3"), Some(32.0));
        for bad in ["0", "4", "", "x", "-1", "1.5"] {
            assert_eq!(marker_beat(&p, bad), None, "{bad:?}");
        }
    }

    #[test]
    fn option_slash_opens_the_box_and_return_goes_to_the_marker() {
        let (mut p, mut s) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut Harness::new(900.0, 600.0));
        assert!(crate::menus::implemented(LABEL));
        assert!(crate::menus::MENUS
            .iter()
            .any(|(m, items)| *m == "Navigate" && items.contains(&LABEL)));
        let c = crate::key_commands::combo_of(LABEL).unwrap();
        assert!(c.key == Key::Slash && c.alt && !c.shift && !c.cmd && !c.ctrl);
        press(h, p, s, Key::Slash, Modifiers::ALT);
        assert_eq!(
            s.track_dialog,
            Some(TrackDialog::GoToMarker {
                text: String::new()
            })
        );
        // Typed into the field, which has the keyboard; the keys wait (2 is not a command).
        frame(h, p, s, vec![Event::Text("2".into())]);
        assert_eq!(
            s.track_dialog,
            Some(TrackDialog::GoToMarker { text: "2".into() })
        );
        let mut history = crate::undo::History::new(p);
        press(h, p, s, Key::Enter, Modifiers::NONE);
        assert!(s.track_dialog.is_none(), "closed");
        assert_eq!(p.position_beats, 16.0, "marker 2 is at bar 5");
        assert!(
            !history.record(p),
            "moving the playhead is not an undo step"
        );
        // A number that is not a marker's: Return leaves the box open and the playhead still.
        crate::menus::apply(LABEL, p, s);
        frame(h, p, s, vec![]);
        frame(h, p, s, vec![Event::Text("7".into())]);
        press(h, p, s, Key::Enter, Modifiers::NONE);
        assert!(s.track_dialog.is_some());
        assert_eq!(p.position_beats, 16.0);
        // Escape closes it.
        press(h, p, s, Key::Escape, Modifiers::NONE);
        assert!(s.track_dialog.is_none());
        // Plain Slash is still Go to Position.
        press(h, p, s, Key::Slash, Modifiers::NONE);
        assert!(matches!(s.track_dialog, Some(TrackDialog::GoTo { .. })));
    }

    #[test]
    fn without_markers_it_is_greyed_and_the_key_does_nothing() {
        let mut p = Project::empty();
        p.markers.clear();
        let mut s = UiState::new();
        let h = &mut Harness::new(900.0, 600.0);
        assert!(!crate::menus::enabled(LABEL, &p, &s));
        press(h, &mut p, &mut s, Key::Slash, Modifiers::ALT);
        assert!(s.track_dialog.is_none());
        p.add_marker(4.0);
        assert!(crate::menus::enabled(LABEL, &p, &s));
    }
}
