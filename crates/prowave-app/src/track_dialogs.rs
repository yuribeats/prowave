//! The Track menu's windows (Logic Pro: Track > Rename Track, Assign Track Color, Assign Track
//! Icon, and Configure Track Header in the Tracks area's View menu).
//!
//! Each one changes the track or the header display at once and closes; Cancel and the window's
//! close button change nothing.

use crate::hw::{self, Cap};
use crate::i18n::tr;
use crate::mixer::hw_mixer as hwm;
use crate::model::{HeaderPart, Project};
use crate::theme::{self, Face};
use crate::track_icons::{draw_icon, IconKind};
use egui::{pos2, vec2, Color32, Rect, Sense, Stroke};

/// The colour swatches of Assign Track Color.
pub const TRACK_COLORS: [Color32; 16] = crate::theme::TRACK_PALETTE;

/// The longest name Rename Track accepts, in characters.
pub const MAX_TRACK_NAME: usize = 64;

/// A window of the Track menu, and the track it is about (by index).
#[derive(Clone, Debug, PartialEq)]
pub enum TrackDialog {
    Rename { track: usize, name: String },
    Color { track: usize },
    Icon { track: usize },
    Header,
}

/// The track a dialog is about, if it is about one.
impl TrackDialog {
    pub fn track(&self) -> Option<usize> {
        match self {
            TrackDialog::Rename { track, .. }
            | TrackDialog::Color { track }
            | TrackDialog::Icon { track } => Some(*track),
            TrackDialog::Header => None,
        }
    }
}

/// The name of a header part in Configure Track Header. The texts are literal `tr` calls so the
/// translation test finds them.
fn part_label(part: HeaderPart) -> &'static str {
    match part {
        HeaderPart::Number => tr("Track Number"),
        HeaderPart::Icon => tr("Track Icon"),
        HeaderPart::Name => tr("Track Name"),
        HeaderPart::Buttons => tr("Mute, Solo, Record and Monitor"),
        HeaderPart::Volume => tr("Volume Slider and Meter"),
        HeaderPart::Pan => tr("Pan Knob"),
    }
}

/// Track > Rename Track: the name without its surrounding spaces, at most [`MAX_TRACK_NAME`]
/// characters. An empty name, or a track that is not there, changes nothing (`false`).
pub fn rename_track(p: &mut Project, track: usize, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || track >= p.tracks.len() {
        return false;
    }
    p.tracks[track].name = name.chars().take(MAX_TRACK_NAME).collect();
    true
}

/// Configure Track Header: shows or hides one part of every track header.
pub fn set_header_part(p: &mut Project, part: HeaderPart, shown: bool) {
    p.header_hidden.retain(|h| *h != part);
    if !shown {
        p.header_hidden.push(part);
    }
}

/// A key laid out in `ui`, `w` wide, with `label`; latched and its LED lit while `on` (when
/// `led`).
fn dialog_key(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    w: f32,
    cap: Cap,
    on: bool,
    led: bool,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
    let g = ui.painter().clone();
    hw::key(
        ui,
        &g,
        rect,
        egui::Id::unique(("track_dialog_key", id)),
        cap,
        on,
        led.then_some(on),
        hw::Legend::Text(label),
    )
    .0
    .clicked()
}

/// A key's width for `label` (8 a letter, and room for the LED).
fn key_w(label: &str) -> f32 {
    (label.chars().count() as f32 * 8.0 + 36.0).max(72.0)
}

/// The windows' width.
const DIALOG_W: f32 = 440.0;

/// Draws the open window, if any, and applies what the user chose.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    let Some(mut dialog) = s.track_dialog.take() else {
        return;
    };
    // A dialog about a track that is gone (deleted while the window was open) just closes.
    if dialog.track().is_some_and(|t| t >= p.tracks.len()) {
        return;
    }
    let mut done = false;
    let title = match &dialog {
        TrackDialog::Rename { .. } => tr("Rename Track"),
        TrackDialog::Color { .. } => tr("Assign Track Color…"),
        TrackDialog::Icon { .. } => tr("Assign Track Icon…"),
        TrackDialog::Header => tr("Configure Track Header…"),
    };
    let (_, closed) = hwm::dialog(
        ctx,
        egui::Id::unique("track_dialog"),
        title,
        DIALOG_W,
        |ui| match &mut dialog {
            TrackDialog::Rename { track, name } => rename_body(ui, p, *track, name, &mut done),
            TrackDialog::Color { track } => color_body(ui, p, *track, &mut done),
            TrackDialog::Icon { track } => icon_body(ui, p, *track, &mut done),
            TrackDialog::Header => header_body(ui, p),
        },
    );
    if !closed && !done {
        s.track_dialog = Some(dialog);
    }
}

/// Rename Track: the name in a dark field, then OK (the black key) and Cancel at the right.
fn rename_body(
    ui: &mut egui::Ui,
    p: &mut Project,
    track: usize,
    name: &mut String,
    done: &mut bool,
) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    hw::paint_field(ui.painter(), r, "", false, false);
    ui.put(
        r.shrink2(vec2(4.0, 0.0)),
        egui::TextEdit::singleline(name)
            .char_limit(MAX_TRACK_NAME)
            .font(theme::font(Face::Bold, 13.0))
            .text_color(theme::ON_DARK)
            .frame(egui::Frame::NONE)
            .margin(vec2(6.0, 6.0))
            .desired_width(r.width() - 8.0)
            .min_size(vec2(r.width() - 8.0, r.height()))
            .align(egui::Align2::LEFT_CENTER),
    );
    ui.add_space(4.0);
    let ok = !name.trim().is_empty();
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 30.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            let cancel = tr("Cancel");
            if dialog_key(ui, "cancel", cancel, key_w(cancel), Cap::Bone, false, false) {
                *done = true;
            }
            let ok_label = tr("OK");
            let ok_clicked = ui
                .add_enabled_ui(ok, |ui| {
                    dialog_key(ui, "ok", ok_label, key_w(ok_label), Cap::Dark, false, false)
                })
                .inner;
            if ok_clicked && rename_track(p, track, name.as_str()) {
                *done = true;
            }
        },
    );
}

/// Assign Track Color: the track's plate material sixteen times, eight to a row, each with
/// its colour as a stripe along the foot (regions are told apart by material; the colour only
/// marks the header). The track's own is pressed in and ringed in ink.
fn color_body(ui: &mut egui::Ui, p: &mut Project, track: usize, done: &mut bool) {
    let current = p.tracks[track].color;
    let kind = Some(p.tracks[track].kind);
    let cols = 8.0;
    let gap = 10.0;
    let full = ui.available_width();
    let w = ((full - gap * (cols - 1.0)) / cols).floor();
    let h = 34.0;
    let used = w * cols + gap * (cols - 1.0);
    let rows = TRACK_COLORS.len().div_ceil(8);
    let (area, _) = ui.allocate_exact_size(
        vec2(full, rows as f32 * (h + gap) - gap + 4.0),
        Sense::hover(),
    );
    let left = area.left() + ((full - used) / 2.0).floor();
    let g = ui.painter().clone();
    for (k, color) in TRACK_COLORS.into_iter().enumerate() {
        let (c, rw) = ((k % 8) as f32, (k / 8) as f32);
        let r = Rect::from_min_size(
            pos2(left + c * (w + gap), area.top() + rw * (h + gap)),
            vec2(w, h),
        );
        let resp = ui.interact(r, egui::Id::unique(("track_color", k)), Sense::click());
        let down = color == current;
        // The lip under the plate, then the plate pressed a point when chosen.
        g.rect_filled(
            r.translate(vec2(0.0, 2.0)),
            3.0,
            Color32::from_black_alpha(if down { 30 } else { 60 }),
        );
        let face = r.translate(vec2(0.0, if down { 1.0 } else { 0.0 }));
        crate::mixer::paint_plate_material(&g, face, kind);
        let stripe = Rect::from_min_max(pos2(face.left(), face.bottom() - 4.0), face.max);
        g.rect_filled(
            stripe,
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: 3,
                se: 3,
            },
            color,
        );
        g.rect_stroke(
            face,
            3.0,
            Stroke::new(1.0, Color32::from_black_alpha(140)),
            egui::StrokeKind::Outside,
        );
        if resp.hovered() && !down {
            g.rect_filled(face, 3.0, Color32::from_white_alpha(28));
        }
        if down {
            g.rect_stroke(
                face.expand(3.0),
                5.0,
                Stroke::new(2.0, theme::INK),
                egui::StrokeKind::Middle,
            );
        }
        if resp.clicked() {
            p.tracks[track].color = color;
            *done = true;
        }
    }
}

/// Assign Track Icon: Automatic (a key with its LED), then every icon in a recessed well, as
/// on a track header; the chosen one ringed in signal orange.
fn icon_body(ui: &mut egui::Ui, p: &mut Project, track: usize, done: &mut bool) {
    let chosen = p.tracks[track].icon;
    let auto = tr("Automatic");
    if dialog_key(
        ui,
        "automatic",
        auto,
        key_w(auto),
        Cap::Bone,
        chosen.is_none(),
        true,
    ) {
        p.tracks[track].icon = None;
        *done = true;
    }
    let cols = 8.0;
    let gap = 6.0;
    let full = ui.available_width();
    let w = ((full - gap * (cols - 1.0)) / cols).floor();
    let used = w * cols + gap * (cols - 1.0);
    let n = IconKind::ALL.len();
    let rows = n.div_ceil(8);
    let (area, _) = ui.allocate_exact_size(
        vec2(full, rows as f32 * (w + gap) - gap + 4.0),
        Sense::hover(),
    );
    let left = area.left() + ((full - used) / 2.0).floor();
    let g = ui.painter().clone();
    for (k, kind) in IconKind::ALL.into_iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(
                left + (k % 8) as f32 * (w + gap),
                area.top() + (k / 8) as f32 * (w + gap),
            ),
            vec2(w, w),
        );
        let resp = ui.interact(r, egui::Id::unique(("track_icon", k)), Sense::click());
        hw::recess(&g, r, 4.0);
        if resp.hovered() {
            g.rect_stroke(
                r,
                4.0,
                Stroke::new(1.0, theme::SEAM_DK),
                egui::StrokeKind::Inside,
            );
        }
        if chosen == Some(kind) {
            g.rect_stroke(
                r.expand(1.5),
                5.0,
                Stroke::new(2.0, theme::ORANGE),
                egui::StrokeKind::Middle,
            );
        }
        draw_icon(&g, r.shrink(8.0), kind);
        if resp.clicked() {
            p.tracks[track].icon = Some(kind);
            *done = true;
        }
    }
}

/// Configure Track Header: a row per part, its name in silkscreen right-aligned against an ON
/// key with its LED, lit while the part shows.
fn header_body(ui: &mut egui::Ui, p: &mut Project) {
    for part in HeaderPart::ALL {
        let shown = !p.header_hidden.contains(&part);
        let clicked = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                hwm::row_label(ui, 280.0, 28.0, part_label(part), theme::INK2);
                let (r, _) = ui.allocate_exact_size(vec2(64.0, 28.0), Sense::hover());
                let g = ui.painter().clone();
                hw::toggle(
                    ui,
                    &g,
                    r,
                    egui::Id::unique(("track_header_part", part.name())),
                    shown,
                    tr("On"),
                )
                .clicked()
            })
            .inner;
        if clicked {
            set_header_part(p, part, !shown);
        }
    }
}

/// The dialog a Track menu entry opens for the selected track, if it opens one.
pub fn open_for(label: &str, p: &Project) -> Option<TrackDialog> {
    let t = p.selected_track;
    let name = || p.tracks.get(t).map(|tr| tr.name.clone());
    match label {
        "Rename Track" => Some(TrackDialog::Rename {
            track: t,
            name: name()?,
        }),
        "Assign Track Color…" => (t < p.tracks.len()).then_some(TrackDialog::Color { track: t }),
        "Assign Track Icon…" => (t < p.tracks.len()).then_some(TrackDialog::Icon { track: t }),
        "Configure Track Header…" => Some(TrackDialog::Header),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_test::Harness;

    #[test]
    fn every_colour_and_icon_has_a_distinct_choice() {
        for (i, a) in TRACK_COLORS.iter().enumerate() {
            assert!(
                TRACK_COLORS[i + 1..].iter().all(|b| b != a),
                "{a:?} repeats"
            );
        }
        for (i, a) in IconKind::ALL.iter().enumerate() {
            assert!(IconKind::ALL[i + 1..].iter().all(|b| b != a));
            assert_eq!(IconKind::from_name(a.name()), Some(*a));
        }
        assert_eq!(IconKind::from_name("nope"), None);
    }

    #[test]
    fn every_header_part_has_a_name_that_round_trips() {
        for part in HeaderPart::ALL {
            assert_eq!(HeaderPart::from_name(part.name()), Some(part));
        }
        assert_eq!(HeaderPart::from_name("nope"), None);
    }

    #[test]
    fn rename_trims_limits_and_refuses_an_empty_name() {
        let mut p = Project::demo();
        assert!(rename_track(&mut p, 0, "  Lead Vox  "));
        assert_eq!(p.tracks[0].name, "Lead Vox");
        assert!(!rename_track(&mut p, 0, "   "), "blank");
        assert_eq!(p.tracks[0].name, "Lead Vox");
        assert!(!rename_track(&mut p, 999, "x"), "no such track");
        let long = "é".repeat(MAX_TRACK_NAME + 10);
        assert!(rename_track(&mut p, 1, &long));
        assert_eq!(p.tracks[1].name.chars().count(), MAX_TRACK_NAME);
    }

    #[test]
    fn hiding_and_showing_a_header_part_is_undone_by_showing_it_again() {
        let mut p = Project::demo();
        set_header_part(&mut p, HeaderPart::Pan, false);
        set_header_part(&mut p, HeaderPart::Pan, false);
        assert_eq!(p.header_hidden, vec![HeaderPart::Pan], "no duplicates");
        set_header_part(&mut p, HeaderPart::Pan, true);
        assert!(p.header_hidden.is_empty());
    }

    #[test]
    fn the_menu_opens_the_right_window_for_the_selected_track() {
        let mut p = Project::demo();
        p.selected_track = 1;
        let name = p.tracks[1].name.clone();
        assert_eq!(
            open_for("Rename Track", &p),
            Some(TrackDialog::Rename { track: 1, name })
        );
        assert_eq!(
            open_for("Assign Track Color…", &p),
            Some(TrackDialog::Color { track: 1 })
        );
        assert_eq!(
            open_for("Assign Track Icon…", &p),
            Some(TrackDialog::Icon { track: 1 })
        );
        assert_eq!(
            open_for("Configure Track Header…", &p),
            Some(TrackDialog::Header)
        );
        assert_eq!(open_for("Cut", &p), None);
        // No track selected (past the end): the track dialogs do not open.
        p.selected_track = 999;
        assert_eq!(open_for("Assign Track Color…", &p), None);
        assert_eq!(open_for("Rename Track", &p), None);
    }

    /// Runs the window for a few frames; returns what it left open.
    fn run(p: &mut Project, s: &mut crate::UiState) {
        let mut h = Harness::new(900.0, 700.0);
        for _ in 0..3 {
            h.frame(vec![], |ui| show(ui.ctx(), p, s));
        }
    }

    #[test]
    fn every_dialog_draws_and_stays_open_until_chosen() {
        for dialog in [
            TrackDialog::Rename {
                track: 0,
                name: "Vox".into(),
            },
            TrackDialog::Color { track: 0 },
            TrackDialog::Icon { track: 0 },
            TrackDialog::Header,
        ] {
            let (mut p, mut s) = (Project::demo(), crate::UiState::new());
            s.track_dialog = Some(dialog.clone());
            run(&mut p, &mut s);
            assert_eq!(s.track_dialog, Some(dialog));
        }
    }

    #[test]
    fn a_dialog_about_a_removed_track_closes() {
        let (mut p, mut s) = (Project::demo(), crate::UiState::new());
        s.track_dialog = Some(TrackDialog::Color { track: 999 });
        run(&mut p, &mut s);
        assert!(s.track_dialog.is_none());
    }

    /// Where the pictures go: `PROWAVE_SHOT_DIR`, else a folder in the temp directory.
    fn shot_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::var_os("PROWAVE_SHOT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-mixer"));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    /// Each of the Track menu's windows.
    #[test]
    #[ignore = "pictures"]
    fn track_dialog_pictures() {
        for (name, dialog) in [
            (
                "dialog-rename.png",
                TrackDialog::Rename {
                    track: 0,
                    name: "Classic Electric Piano".into(),
                },
            ),
            ("dialog-color.png", TrackDialog::Color { track: 0 }),
            ("dialog-icon.png", TrackDialog::Icon { track: 0 }),
            ("dialog-header.png", TrackDialog::Header),
        ] {
            let (mut p, mut s) = (Project::demo(), crate::UiState::new());
            p.header_hidden = vec![HeaderPart::Pan];
            s.track_dialog = Some(dialog);
            crate::ui_test::render_png(
                560.0,
                440.0,
                2.0,
                crate::theme::BONE3,
                |ui| {
                    crate::theme::apply(ui.ctx());
                    show(ui.ctx(), &mut p, &mut s)
                },
                &shot_path(name),
            )
            .unwrap();
        }
    }
}
