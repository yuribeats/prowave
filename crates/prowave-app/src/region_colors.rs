//! Region colours (Logic Pro: View > Show Colors, Option-C, and Color Regions by Track Color,
//! Option-Shift-C).
//!
//! The Colors window holds the Hardware palette (`theme::TRACK_PALETTE`). A click on a colour
//! gives it to the selected regions, or, with no region selected, to the selected track (as in
//! Logic). A region with its own colour keeps it when its track's colour changes; Color Regions
//! by Track Color takes it away again. The window stays open while you work, and each click
//! changes the project in one go, so each is one undo step.

use crate::i18n::tr;
use crate::mixer::hw_mixer as hwm;
use crate::model::Project;
use crate::theme;
use crate::UiState;
use egui::{pos2, vec2, Color32, Rect, Sense, Stroke};

/// The colours of the window.
pub const PALETTE: [Color32; 16] = theme::TRACK_PALETTE;

/// The regions the window colours: the selected regions that are there.
fn targets(p: &Project, s: &UiState) -> Vec<(usize, usize)> {
    s.tracks
        .selection()
        .into_iter()
        .filter(|&(t, r)| p.tracks.get(t).is_some_and(|tr| r < tr.regions.len()))
        .collect()
}

/// The colour the window marks as current: that of the first selected region (its own, or its
/// track's), or the selected track's.
pub(crate) fn current(p: &Project, s: &UiState) -> Option<Color32> {
    match targets(p, s).first() {
        Some(&(t, r)) => {
            let track = &p.tracks[t];
            Some(track.regions[r].params.color.unwrap_or(track.color))
        }
        None => p.tracks.get(p.selected_track).map(|t| t.color),
    }
}

/// A colour picked in the window: the selected regions take it, or, with none selected, the
/// selected track. Returns whether anything changed.
pub(crate) fn give_color(p: &mut Project, s: &UiState, color: Color32) -> bool {
    let regions = targets(p, s);
    if regions.is_empty() {
        let Some(t) = p.tracks.get_mut(p.selected_track) else {
            return false;
        };
        let changed = t.color != color;
        t.color = color;
        return changed;
    }
    let mut changed = false;
    for (t, r) in regions {
        let params = &mut p.tracks[t].regions[r].params;
        changed |= params.color != Some(color);
        params.color = Some(color);
    }
    changed
}

/// Whether Color Regions by Track Color has something to do: a selected region with its own
/// colour.
pub(crate) fn can_reset(p: &Project, s: &UiState) -> bool {
    targets(p, s)
        .into_iter()
        .any(|(t, r)| p.tracks[t].regions[r].params.color.is_some())
}

/// Color Regions by Track Color: the selected regions take their track's colour again.
pub(crate) fn color_by_track(p: &mut Project, s: &UiState) {
    for (t, r) in targets(p, s) {
        p.tracks[t].regions[r].params.color = None;
    }
}

/// The id of colour `k`'s swatch.
pub(crate) fn swatch_id(k: usize) -> egui::Id {
    egui::Id::unique(("region_color", k))
}

/// Draws the Colors window while it is open, and gives a clicked colour to the selection.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    if !s.arrange.colors {
        return;
    }
    let now = current(p, s);
    let mut picked = None;
    let (_, closed) = hwm::dialog(
        ctx,
        egui::Id::unique("region_colors"),
        tr("Colours"),
        8.0 * 34.0 + 7.0 * 6.0,
        |ui| {
            let (w, h, gap) = (34.0, 22.0, 6.0);
            let rows = PALETTE.len().div_ceil(8);
            let (area, _) = ui.allocate_exact_size(
                vec2(ui.available_width(), rows as f32 * (h + gap) + 6.0),
                Sense::hover(),
            );
            let g = ui.painter().clone();
            for (k, color) in PALETTE.into_iter().enumerate() {
                let (c, row) = ((k % 8) as f32, (k / 8) as f32);
                let r = Rect::from_min_size(
                    pos2(
                        area.left() + c * (w + gap),
                        area.top() + 6.0 + row * (h + gap),
                    ),
                    vec2(w, h),
                );
                let resp = ui.interact(r, swatch_id(k), Sense::click());
                g.rect_filled(r, 3.0, color);
                g.rect_stroke(
                    r,
                    3.0,
                    Stroke::new(1.0, Color32::from_black_alpha(140)),
                    egui::StrokeKind::Inside,
                );
                if resp.hovered() {
                    g.rect_filled(r, 3.0, Color32::from_white_alpha(28));
                }
                if now == Some(color) {
                    g.rect_stroke(
                        r.expand(2.0),
                        4.0,
                        Stroke::new(2.0, theme::INK()),
                        egui::StrokeKind::Outside,
                    );
                }
                if resp.clicked() {
                    picked = Some(color);
                }
            }
        },
    );
    if let Some(color) = picked {
        give_color(p, s, color);
    }
    if closed {
        s.arrange.colors = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track_commands::tests::{app_frame, one_undo_step, rig, tap};
    use egui::{Key, Modifiers};

    const RUST: Color32 = PALETTE[6];

    #[test]
    fn option_c_opens_the_colors_window_and_a_click_colours_the_selected_regions() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        s.tracks.select_many(&[(0, 0), (1, 0)]);
        tap(h, p, s, Key::C, Modifiers::ALT);
        assert!(s.arrange.colors, "Option-C opens the window");
        assert!(crate::menus::checked("Show Colors", p, s));
        assert!(!p.cycle, "Option-C is not Cycle Mode (C)");
        // A click on a swatch.
        h.frame(vec![], |ui| show(ui.ctx(), p, s));
        let at = h
            .ctx
            .read_response(swatch_id(6))
            .expect("the swatch is on screen")
            .rect
            .center();
        let track_colors = (p.tracks[0].color, p.tracks[1].color);
        h.click(at, |ui| show(ui.ctx(), p, s));
        assert_eq!(p.tracks[0].regions[0].params.color, Some(RUST));
        assert_eq!(p.tracks[1].regions[0].params.color, Some(RUST));
        assert_eq!(
            (p.tracks[0].color, p.tracks[1].color),
            track_colors,
            "the tracks keep theirs"
        );
        assert!(s.arrange.colors, "the window stays open");
        // Option-C again closes it.
        tap(h, p, s, Key::C, Modifiers::ALT);
        assert!(!s.arrange.colors);
        assert_eq!(
            crate::key_commands::combo_of("Show Colors").map(|c| c.shortcut()),
            Some(egui::KeyboardShortcut::new(Modifiers::ALT, Key::C))
        );
    }

    #[test]
    fn a_colour_is_one_undo_step_and_without_regions_colours_the_track() {
        let (mut p, mut s, _) = rig();
        s.tracks.select_only(0, 0);
        one_undo_step(&mut p, |p| {
            assert!(give_color(p, &s, RUST));
        });
        s.tracks.clear_selection();
        p.selected_track = 2;
        one_undo_step(&mut p, |p| {
            assert!(give_color(p, &s, RUST));
            assert_eq!(p.tracks[2].color, RUST);
            assert_eq!(current(p, &s), Some(RUST));
        });
        assert_ne!(
            p.tracks[2].color, RUST,
            "Undo gave the track its colour back"
        );
    }

    #[test]
    fn option_shift_c_gives_regions_their_track_colour_again() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        p.tracks[0].regions[0].params.color = Some(RUST);
        p.tracks[1].regions[0].params.color = Some(RUST);
        s.tracks.select_only(0, 0);
        assert!(crate::menus::enabled("Color Regions by Track Color", p, s));
        tap(h, p, s, Key::C, Modifiers::ALT | Modifiers::SHIFT);
        assert_eq!(p.tracks[0].regions[0].params.color, None);
        assert_eq!(
            p.tracks[1].regions[0].params.color,
            Some(RUST),
            "only the selected region"
        );
        assert_eq!(
            current(p, s),
            Some(p.tracks[0].color),
            "it shows the track's colour"
        );
        assert!(
            !crate::menus::enabled("Color Regions by Track Color", p, s),
            "greyed: the selected region has the track's colour already"
        );
        s.tracks.clear_selection();
        assert!(!crate::menus::enabled("Color Regions by Track Color", p, s));
        // One undo step through the menu.
        s.tracks.select_only(1, 0);
        let s2 = &mut *s;
        one_undo_step(p, |p| {
            crate::menus::apply("Color Regions by Track Color", p, s2)
        });
    }

    #[test]
    fn a_region_colour_is_saved_and_loaded() {
        let (mut p, _, _) = rig();
        p.tracks[0].regions[0].params.color = Some(RUST);
        let doc = crate::file_commands::content(&p);
        assert_eq!(doc.tracks[0].regions[0].color, Some([0xa2, 0x4b, 0x2a]));
        assert_eq!(doc.tracks[1].regions[0].color, None);
        let ctx = egui::Context::default();
        let (back, _) =
            crate::project_io::from_doc(&doc, std::path::Path::new("/nonexistent"), &ctx);
        assert_eq!(back.tracks[0].regions[0].params.color, Some(RUST));
        assert_eq!(back.tracks[1].regions[0].params.color, None);
        // An older file, without the field, loads with the track's colour.
        let json = serde_json::to_string(&doc.tracks[1].regions[0]).expect("json");
        assert!(!json.contains("color"), "left out when unset: {json}");
        let old: prowave_project::RegionDoc = serde_json::from_str(&json).expect("loads");
        assert_eq!(old.color, None);
    }

    #[test]
    fn the_tracks_area_draws_a_region_in_its_own_colour() {
        let (p, _, _) = rig();
        let mut r = p.tracks[0].regions[0].clone();
        r.params.color = Some(RUST);
        let look = crate::tracks_area::region_look_for_test(&p.tracks[0], &r);
        assert_eq!(look.header, RUST);
        r.params.color = None;
        let look = crate::tracks_area::region_look_for_test(&p.tracks[0], &r);
        assert_eq!(look.header, p.tracks[0].color);
    }
}
