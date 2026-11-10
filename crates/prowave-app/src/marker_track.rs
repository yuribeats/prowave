//! The Marker track (Logic Pro: Global Tracks > Marker), shown above the tracks with Track > Show
//! Marker Track. Each marker is a named, coloured section from its start to the next marker.
//!
//! - Double-click an empty spot (or Navigate > Create Marker): new marker there.
//! - Click a marker: the playhead goes to its start. Drag it to move it (snapped to the grid).
//! - Double-click a marker, or right-click > Rename Marker: edit its name.
//! - Right-click: colour, or Delete Marker.
use crate::i18n::tr;
use crate::model::{Project, MARKER_COLORS};
use crate::theme::{self, Face};
use crate::tracks_area::hw_tracks;
use crate::widgets::context_menu;
use egui::{pos2, vec2, Align2, Color32, Rect, Sense, Stroke, Ui};

/// Height of the Marker track, in points.
pub const HEIGHT: f32 = 28.0;

/// What a right-click or double-click asked for, run after drawing.
enum Action {
    Rename(usize),
    Color(usize, Color32),
    Delete(usize),
}

/// Draws the Marker track's `header` and `lane` and handles its editing. `x_of` / `beat_of`
/// convert between beats and screen x as in the Tracks area; `snap` snaps a position.
pub fn show(
    ui: &mut Ui,
    p: &mut Project,
    header: Rect,
    lane: Rect,
    x_of: impl Fn(f64) -> f32,
    beat_of: impl Fn(f32) -> f64,
    snap: &dyn Fn(f64) -> f64,
) {
    let rename_id = egui::Id::unique("marker_track_rename");
    let grab_id = egui::Id::unique("marker_track_grab");

    // ---- header
    let gh = ui.painter_at(header);
    hw_tracks::global_header(&gh, header, tr("Marker"));
    if let Some(i) = p.marker_at(p.position_beats) {
        let name = hw_tracks::fit(
            &gh,
            &p.markers[i].name.to_uppercase(),
            Face::Bold,
            11.0,
            0.05,
            header.width() * 0.45,
        );
        hw_tracks::label(
            &gh,
            pos2(header.right() - 20.0, header.center().y),
            Align2::RIGHT_CENTER,
            &name,
            Face::Bold,
            11.0,
            0.05,
            theme::INK2,
        );
    }

    // ---- lane: double-click an empty spot to add a marker
    let g = ui.painter_at(lane);
    hw_tracks::global_lane(&g, lane);
    let lane_resp = ui.interact(lane, egui::Id::unique("marker_track_lane"), Sense::click());
    if lane_resp.double_clicked() {
        if let Some(pt) = lane_resp.interact_pointer_pos() {
            let i = p.add_marker(snap(beat_of(pt.x)));
            ui.ctx()
                .data_mut(|m| m.insert_temp(rename_id, (i, p.markers[i].name.clone())));
        }
    }

    // ---- markers: each one runs to the next
    let mut action = None;
    let mut moved = false;
    for i in 0..p.markers.len() {
        let start = p.markers[i].beat;
        let x0 = x_of(start);
        let x1 = p
            .markers
            .get(i + 1)
            .map_or(lane.right() + 1.0, |n| x_of(n.beat));
        if x1 < lane.left() || x0 > lane.right() {
            continue;
        }
        let block = Rect::from_min_max(
            pos2(x0 + 1.0, lane.top() + 4.0),
            pos2(x1 - 1.0, lane.bottom() - 4.0),
        );
        let resp = ui.interact(
            block.intersect(lane),
            egui::Id::unique(("marker", i)),
            Sense::click_and_drag(),
        );
        if resp.clicked() {
            p.position_beats = start;
        }
        if resp.double_clicked() {
            action = Some(Action::Rename(i));
        }
        if resp.drag_started() {
            if let Some(pt) = resp.interact_pointer_pos() {
                ui.ctx()
                    .data_mut(|m| m.insert_temp(grab_id, beat_of(pt.x) - start));
            }
        }
        if resp.dragged() {
            if let Some(pt) = resp.interact_pointer_pos() {
                let grab = ui.ctx().data(|m| m.get_temp::<f64>(grab_id)).unwrap_or(0.0);
                p.markers[i].beat = snap((beat_of(pt.x) - grab).max(0.0));
            }
        }
        if resp.drag_stopped() {
            moved = true;
        }
        context_menu(&resp, |ui| {
            if ui.button(tr("Rename Marker")).clicked() {
                action = Some(Action::Rename(i));
            }
            ui.horizontal(|ui| {
                for c in MARKER_COLORS {
                    let (r, sw) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                    ui.painter().rect_filled(r.shrink(1.0), 3.0, c);
                    if sw.clicked() {
                        action = Some(Action::Color(i, c));
                    }
                }
            });
            ui.separator();
            if ui.button(tr("Delete Marker")).clicked() {
                action = Some(Action::Delete(i));
            }
        });

        let m = &p.markers[i];
        // The marker the playhead is in is an orange cap; the others are black caps. The
        // marker's own colour is a stripe at its left.
        let current = p.marker_at(p.position_beats) == Some(i);
        let cap = block.intersect(lane.expand2(vec2(4.0, 0.0)));
        if current {
            hw_tracks::plate(&g, cap, theme::ORANGE, true);
        } else {
            hw_tracks::dark_plate(&g, cap);
        }
        if cap.width() > 8.0 {
            g.rect_filled(
                Rect::from_min_size(cap.min + vec2(3.0, 3.0), vec2(3.0, cap.height() - 6.0)),
                1.0,
                m.color,
            );
        }
        g.vline(x0, lane.top()..=lane.bottom(), Stroke::new(2.0, theme::INK));
        let text_x = x0.max(lane.left()) + 11.0;
        if text_x + 10.0 < x1 {
            let clip = g.with_clip_rect(block.shrink(1.0).intersect(lane));
            hw_tracks::label(
                &clip,
                pos2(text_x, block.center().y),
                Align2::LEFT_CENTER,
                &m.name.to_uppercase(),
                Face::Silk,
                12.0,
                0.1,
                if current { theme::INK } else { theme::BONE_HI },
            );
        }
    }
    if moved {
        p.tidy_markers();
    }

    match action {
        Some(Action::Rename(i)) => {
            let name = p.markers[i].name.clone();
            ui.ctx().data_mut(|m| m.insert_temp(rename_id, (i, name)));
        }
        Some(Action::Color(i, c)) => p.markers[i].color = c,
        Some(Action::Delete(i)) => {
            p.markers.remove(i);
        }
        None => {}
    }

    // ---- renaming: a text field over the marker; Enter or clicking away keeps it
    if let Some((i, mut text)) = ui.ctx().data(|m| m.get_temp::<(usize, String)>(rename_id)) {
        let Some(m) = p.markers.get(i) else {
            ui.ctx()
                .data_mut(|m| m.remove::<(usize, String)>(rename_id));
            return;
        };
        let x0 = x_of(m.beat).max(lane.left());
        let r = Rect::from_min_size(pos2(x0, lane.top() + 1.0), vec2(160.0, HEIGHT - 2.0));
        let edit = ui.put(
            r,
            egui::TextEdit::singleline(&mut text)
                .id(rename_id.with("field"))
                .char_limit(64)
                .font(theme::font(Face::Silk, 13.0)),
        );
        if !edit.has_focus() && !edit.lost_focus() {
            edit.request_focus();
        }
        let (enter, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            ui.ctx()
                .data_mut(|m| m.remove::<(usize, String)>(rename_id));
        } else if enter || edit.lost_focus() {
            let name = text.trim();
            if !name.is_empty() {
                p.markers[i].name = name.to_string();
            }
            ui.ctx()
                .data_mut(|m| m.remove::<(usize, String)>(rename_id));
        } else {
            ui.ctx().data_mut(|m| m.insert_temp(rename_id, (i, text)));
        }
    }

    // The playhead runs through this lane from the Tracks area's ruler.
}
