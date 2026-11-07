//! The Signature track (Logic Pro: Global Tracks > Signature), shown above the tracks with Track >
//! Show Signature Track. Each block is a time signature that holds until the next one.
//!
//! - Double-click an empty spot: a new signature change at the nearest bar, with the signature
//!   that was in force there.
//! - Double-click a block, or right-click > Edit Signature…: change its numerator and
//!   denominator.
//! - Drag a block to move it to another bar.
//! - Right-click > Delete Signature Change. The first block (bar 1) is the song's own signature
//!   and cannot be deleted.
use crate::i18n::tr;
use crate::model::{Project, SIGNATURE_DENOMINATORS};
use crate::widgets::context_menu;
use egui::{pos2, Align2, Color32, FontId, Rect, Sense, Stroke, Ui};

/// Height of the Signature track, in points.
pub const HEIGHT: f32 = 24.0;

const BG: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
const HEADER_BG: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
const BLOCK: Color32 = Color32::from_rgb(0x5a, 0x6a, 0x8c);
const BLOCK_EDGE: Color32 = Color32::from_rgb(0x9a, 0xaa, 0xcc);

/// One block: the 1-based bar it starts at and its signature. Bar 1 is the song's own.
#[derive(Clone, Copy)]
struct Block {
    bar: u32,
    num: u8,
    den: u8,
}

fn blocks(p: &Project) -> Vec<Block> {
    std::iter::once(Block {
        bar: 1,
        num: p.sig.0,
        den: p.sig.1,
    })
    .chain(p.sig_changes.iter().map(|c| Block {
        bar: c.bar,
        num: c.num,
        den: c.den,
    }))
    .collect()
}

/// What a right-click or double-click asked for, run after drawing.
enum Action {
    Edit(u32),
    Delete(u32),
}

/// Draws the Signature track's `header` and `lane` and handles its editing. `x_of` / `beat_of`
/// convert between beats and screen x as in the Tracks area.
pub fn show(
    ui: &mut Ui,
    p: &mut Project,
    header: Rect,
    lane: Rect,
    x_of: impl Fn(f64) -> f32,
    beat_of: impl Fn(f32) -> f64,
) {
    let edit_id = egui::Id::unique("signature_track_edit");
    let grab_id = egui::Id::unique("signature_track_grab");

    // ---- header
    let gh = ui.painter_at(header);
    gh.rect_filled(header, 0.0, HEADER_BG);
    gh.line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, Color32::from_gray(0x22)),
    );
    gh.text(
        pos2(header.left() + 12.0, header.center().y),
        Align2::LEFT_CENTER,
        tr("Signature"),
        FontId::proportional(13.0),
        Color32::from_gray(0xdc),
    );
    let (num, den) = p.signature_of_bar(p.beats_to_bar(p.position_beats) as u32);
    gh.text(
        pos2(header.right() - 10.0, header.center().y),
        Align2::RIGHT_CENTER,
        format!("{num}/{den}"),
        FontId::proportional(11.0),
        Color32::from_gray(0xa8),
    );

    // ---- lane: double-click an empty spot to add a change
    let g = ui.painter_at(lane);
    g.rect_filled(lane, 0.0, BG);
    let lane_resp = ui.interact(
        lane,
        egui::Id::unique("signature_track_lane"),
        Sense::click(),
    );
    if lane_resp.double_clicked() {
        if let Some(pt) = lane_resp.interact_pointer_pos() {
            let bar = p.sigs().nearest_bar_line(beat_of(pt.x));
            let bar = p.beats_to_bar(bar).round() as u32 + 1;
            if bar >= 2 && !p.sig_changes.iter().any(|c| c.bar == bar) {
                let sig = p.signature_of_bar(bar - 1);
                p.set_signature_from(bar, sig);
                ui.ctx().data_mut(|m| m.insert_temp(edit_id, bar));
            }
        }
    }

    // ---- blocks: each one runs to the next
    let list = blocks(p);
    let mut action = None;
    let mut moved = false;
    for (i, b) in list.iter().enumerate() {
        let x0 = x_of(p.bar_to_beats((b.bar - 1) as f64));
        let x1 = list.get(i + 1).map_or(lane.right() + 1.0, |n| {
            x_of(p.bar_to_beats((n.bar - 1) as f64))
        });
        if x1 < lane.left() || x0 > lane.right() {
            continue;
        }
        let block = Rect::from_min_max(pos2(x0, lane.top() + 2.0), pos2(x1, lane.bottom() - 2.0));
        let resp = ui.interact(
            block.intersect(lane),
            egui::Id::unique(("signature", i)),
            Sense::click_and_drag(),
        );
        if resp.double_clicked() {
            action = Some(Action::Edit(b.bar));
        }
        if b.bar > 1 {
            if resp.drag_started() {
                if let Some(pt) = resp.interact_pointer_pos() {
                    let grab = beat_of(pt.x) - p.bar_to_beats((b.bar - 1) as f64);
                    ui.ctx().data_mut(|m| m.insert_temp(grab_id, grab));
                }
            }
            if resp.dragged() {
                if let Some(pt) = resp.interact_pointer_pos() {
                    let grab = ui.ctx().data(|m| m.get_temp::<f64>(grab_id)).unwrap_or(0.0);
                    let to = p.beats_to_bar(beat_of(pt.x) - grab).round() as u32 + 1;
                    let taken = p.sig_changes.iter().any(|c| c.bar == to);
                    if to >= 2 && to != b.bar && !taken {
                        if let Some(c) = p.sig_changes.iter_mut().find(|c| c.bar == b.bar) {
                            c.bar = to;
                        }
                        p.sig_changes.sort_by_key(|c| c.bar);
                        moved = true;
                    }
                }
            }
        }
        context_menu(&resp, |ui| {
            if ui.button(tr("Edit Signature…")).clicked() {
                action = Some(Action::Edit(b.bar));
            }
            if b.bar > 1 {
                ui.separator();
                if ui.button(tr("Delete Signature Change")).clicked() {
                    action = Some(Action::Delete(b.bar));
                }
            }
        });

        g.rect_filled(block, 3.0, BLOCK);
        g.line_segment(
            [pos2(x0, lane.top()), pos2(x0, lane.bottom())],
            Stroke::new(1.5, BLOCK_EDGE),
        );
        let label = g.layout_no_wrap(
            format!("{}/{}", b.num, b.den),
            FontId::proportional(12.0),
            Color32::WHITE,
        );
        let text_x = x0.max(lane.left()) + 5.0;
        if text_x + 10.0 < x1 {
            g.with_clip_rect(block.shrink(1.0).intersect(lane)).galley(
                pos2(text_x, block.center().y - label.size().y / 2.0),
                label,
                Color32::WHITE,
            );
        }
    }
    if moved {
        p.song_rev += 1;
    }

    match action {
        Some(Action::Edit(bar)) => {
            ui.ctx().data_mut(|m| m.insert_temp(edit_id, bar));
        }
        Some(Action::Delete(bar)) => {
            p.remove_signature_at(bar);
        }
        None => {}
    }

    // ---- editing: a small box under the block with the numerator and the denominator
    if let Some(bar) = ui.ctx().data(|m| m.get_temp::<u32>(edit_id)) {
        let (num, den) = p.signature_of_bar(bar - 1);
        let (mut new_num, mut new_den) = (num, den);
        let x0 = x_of(p.bar_to_beats((bar - 1) as f64)).max(lane.left());
        let mut close = false;
        let area = egui::Area::new(egui::Id::unique("signature_editor"))
            .fixed_pos(pos2(x0, lane.bottom() + 2.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut new_num).range(1..=99).speed(0.1));
                        ui.label("/");
                        for d in SIGNATURE_DENOMINATORS {
                            if ui.selectable_label(new_den == d, d.to_string()).clicked() {
                                new_den = d;
                            }
                        }
                        if ui.button("OK").clicked() {
                            close = true;
                        }
                    });
                });
            });
        let (escape, enter) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Enter),
            )
        });
        let clicked_away = ui.input(|i| i.pointer.any_pressed())
            && !ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|pt| area.response.rect.contains(pt));
        if (new_num, new_den) != (num, den) {
            p.set_signature_from(bar, (new_num, new_den));
        }
        if close || escape || enter || clicked_away {
            ui.ctx().data_mut(|m| m.remove::<u32>(edit_id));
        }
    }

    // ---- playhead
    let x = x_of(p.position_beats);
    if x >= lane.left() && x <= lane.right() {
        g.line_segment(
            [pos2(x, lane.top()), pos2(x, lane.bottom())],
            Stroke::new(1.0, Color32::WHITE),
        );
    }
    g.line_segment(
        [lane.left_bottom(), lane.right_bottom()],
        Stroke::new(1.0, Color32::from_gray(0x1a)),
    );
}
