//! The small panes behind control bar buttons: the Toolbar, the Note Pad and Tips.
use crate::i18n::tr;
use crate::model::Project;

/// Width of the Note Pad.
pub const NOTES_WIDTH: f32 = 260.0;
/// Height of the Toolbar row.
pub const TOOLBAR_HEIGHT: f32 = 30.0;

/// The longest note a project keeps, in characters.
pub const MAX_NOTES: usize = 100_000;

/// What the Toolbar offers: menu entries, so each is greyed out when the menu's is.
const TOOLBAR: &[&str] = &[
    "Undo",
    "Redo",
    "-",
    "New",
    "Create Marker",
    "Capture Recording",
    "-",
    "Zoom In",
    "Zoom Out",
    "Zoom to Fit",
];

/// The Toolbar under the control bar (Logic: View > Show Toolbar): a menu row of bone keys,
/// in groups between seams; a key whose menu entry is greyed out is printed faint and does
/// nothing.
pub fn toolbar(ui: &mut egui::Ui, p: &mut Project, s: &mut crate::UiState) {
    use crate::hw::{self, Cap, Legend};
    use crate::theme::{self, Face};
    let row = ui.available_rect_before_wrap();
    let g = ui.painter_at(row);
    g.rect_filled(row, 0.0, crate::tracks_area::hw_tracks::PANEL_MENU);
    crate::tracks_area::hw_tracks::seam_wide_h(&g, row.x_range(), row.bottom());
    let key_h = (row.height() - 10.0).clamp(18.0, 24.0);
    let cy = row.center().y - 1.0;
    let mut x = row.left() + 12.0;
    for label in TOOLBAR {
        if *label == "-" {
            hw::seam_v(&g, x + 4.0, cy - key_h / 2.0..=cy + key_h / 2.0);
            x += 14.0;
            continue;
        }
        let text = tr(label).to_uppercase();
        let w = hw::galley(&g, &text, Face::Bold, 10.5, theme::INK).size().x + 22.0;
        let rect = egui::Rect::from_min_size(egui::pos2(x, cy - key_h / 2.0), egui::vec2(w, key_h));
        x += w + 6.0;
        if rect.right() > row.right() {
            break;
        }
        let on = crate::menus::enabled(label, p, s);
        let id = egui::Id::unique(("side_toolbar", *label));
        if on {
            let (resp, _) = hw::key(
                ui,
                &g,
                rect,
                id,
                Cap::Bone,
                false,
                None,
                Legend::Text(&text),
            );
            if resp.clicked() {
                crate::menus::apply(label, p, s);
            }
        } else {
            let mut faint = g.clone();
            faint.multiply_opacity(0.45);
            hw::paint_key(&faint, rect, Cap::Bone, false, false);
            hw::text(
                &faint,
                rect.center() - egui::vec2(0.0, 0.5),
                egui::Align2::CENTER_CENTER,
                &text,
                Face::Bold,
                10.5,
                theme::INK,
            );
        }
    }
    ui.allocate_rect(row, egui::Sense::hover());
}

/// The Note Pad: free text saved with the project (Logic: Project Notes).
pub fn note_pad(ui: &mut egui::Ui, p: &mut Project) {
    ui.add_space(8.0);
    let (r, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 16.0), egui::Sense::hover());
    crate::hw::bracket(
        ui.painter(),
        r.left() + 8.0,
        r.right() - 8.0,
        r.center().y,
        tr("Notes"),
    );
    ui.add_space(6.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_sized(
            ui.available_size(),
            egui::TextEdit::multiline(&mut p.notes).char_limit(MAX_NOTES),
        );
    });
}

/// Tips: fixed hints on how to do the main things, in a window of its own.
pub fn tips(ctx: &egui::Context, s: &mut crate::UiState) {
    egui::Window::new(tr("Tips"))
        .open(&mut s.show_tips)
        .resizable(false)
        .default_width(320.0)
        .show(ctx, |ui| {
            for line in [
                tr("Space plays and stops. Drag a file onto the window to add it."),
                tr("To record audio, pick the input in the track's Input slot, press R, then Record and Play."),
                tr("To record MIDI, press R on an instrument track, then Record and Play."),
                tr("The I button lets you hear an audio track's input. Use headphones to avoid feedback."),
                tr("With Count-in on, recording starts after one bar of clicks."),
                tr("Capture Recording turns what you just played on a MIDI keyboard into a region."),
            ] {
                ui.label(line);
                ui.add_space(4.0);
            }
        });
}
