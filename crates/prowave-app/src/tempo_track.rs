//! The Tempo track (Logic Pro: Global Tracks > Tempo), shown above the tracks with Track > Show
//! Tempo Track. Each point is a tempo change that holds until the next one, or, with a tempo
//! curve, moves in a straight line to it; the first point is the tempo at the start of the song.
//!
//! - Double-click an empty spot: new tempo change there (snapped to the grid).
//! - Drag a point: up/down changes its tempo, left/right moves it (not past its neighbours).
//! - Right-click a point > Ramp to Next Tempo Change / Hold Tempo Until Next Change: makes the
//!   tempo curve to the next point, or hold.
//! - Double-click a point, or right-click > Delete Tempo Change: removes it.
use crate::i18n::tr;
use crate::model::{Project, MAX_BPM, MIN_BPM};
use crate::widgets::{context_click, context_menu};
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, Shape, Stroke, Ui};

/// Height of the Tempo track, in points.
/// The closest two tempo points may be, in quarter notes.
const MIN_GAP: f64 = 0.25;

pub const HEIGHT: f32 = 46.0;

const LINE: Color32 = Color32::from_rgb(0x5f, 0xc8, 0x6a);
const FILL: Color32 = Color32::from_rgba_premultiplied(0x18, 0x34, 0x1b, 0x60);
const BG: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
const HEADER_BG: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
/// Margin above and below the slowest and fastest tempo, in BPM.
const MARGIN: f32 = 8.0;
/// Padding inside the lane, in points.
const PAD: f32 = 7.0;

/// BPM range the lane shows: every tempo of the song with a margin, rounded to fives.
pub fn range(p: &Project) -> (f32, f32) {
    let (lo, hi) = p
        .tempo_changes
        .iter()
        .map(|t| t.bpm)
        .fold((p.tempo, p.tempo), |(lo, hi), b| (lo.min(b), hi.max(b)));
    let lo = ((lo - MARGIN) / 5.0).floor() * 5.0;
    let hi = ((hi + MARGIN) / 5.0).ceil() * 5.0;
    (lo.max(MIN_BPM), hi.min(MAX_BPM))
}

fn y_of(bpm: f32, lane: Rect, (lo, hi): (f32, f32)) -> f32 {
    let t = ((bpm - lo) / (hi - lo).max(1.0)).clamp(0.0, 1.0);
    lane.bottom() - PAD - t * (lane.height() - 2.0 * PAD)
}

fn bpm_of(y: f32, lane: Rect, (lo, hi): (f32, f32)) -> f32 {
    let t = (lane.bottom() - PAD - y) / (lane.height() - 2.0 * PAD).max(1.0);
    (lo + t * (hi - lo)).round().clamp(MIN_BPM, MAX_BPM)
}

/// Draws the Tempo track's `header` and `lane` and handles its editing. `x_of` / `beat_of`
/// convert between beats and screen x as in the Tracks area; `snap` snaps a position.
pub fn show(
    ui: &Ui,
    p: &mut Project,
    header: Rect,
    lane: Rect,
    x_of: impl Fn(f64) -> f32,
    beat_of: impl Fn(f32) -> f64,
    snap: &dyn Fn(f64) -> f64,
) {
    // Keep the range still while a point is dragged, so the point stays under the pointer.
    let range_id = egui::Id::unique("tempo_track_range");
    let dragging = ui.ctx().dragged_id().is_some_and(|d| {
        ui.ctx()
            .data(|m| m.get_temp::<egui::Id>(range_id.with("drag")))
            .is_some_and(|ours| ours == d)
    });
    let r = match ui.ctx().data(|m| m.get_temp::<(f32, f32)>(range_id)) {
        Some(r) if dragging => r,
        _ => range(p),
    };
    ui.ctx().data_mut(|m| m.insert_temp(range_id, r));

    // ---- header
    let gh = ui.painter_at(header);
    gh.rect_filled(header, 0.0, HEADER_BG);
    gh.line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, Color32::from_gray(0x22)),
    );
    gh.text(
        pos2(header.left() + 12.0, header.center().y - 7.0),
        Align2::LEFT_CENTER,
        tr("Tempo"),
        FontId::proportional(13.0),
        Color32::from_gray(0xdc),
    );
    gh.text(
        pos2(header.left() + 12.0, header.center().y + 9.0),
        Align2::LEFT_CENTER,
        format!("{:.1} bpm", p.tempo_at(p.position_beats)),
        FontId::proportional(11.0),
        LINE,
    );
    gh.text(
        pos2(header.right() - 10.0, header.center().y),
        Align2::RIGHT_CENTER,
        format!("{:.0}–{:.0}", r.0, r.1),
        FontId::proportional(10.0),
        Color32::from_gray(0x8a),
    );

    // ---- lane: double-click an empty spot to add a tempo change
    let g = ui.painter_at(lane);
    g.rect_filled(lane, 0.0, BG);
    let lane_resp = ui.interact(lane, egui::Id::unique("tempo_track_lane"), Sense::click());
    if lane_resp.double_clicked() {
        if let Some(pt) = lane_resp.interact_pointer_pos() {
            p.add_tempo_change(snap(beat_of(pt.x)), bpm_of(pt.y, lane, r));
        }
    }

    // ---- the tempo line: each tempo holds until the next change, or on a tempo curve moves
    // in a straight line to it
    let all = p.tempo_points();
    let points: Vec<(f64, f32)> = all.iter().map(|t| (t.beat, t.bpm)).collect();
    let mut line = Vec::with_capacity(points.len() * 2 + 1);
    // Each segment's fill is one convex piece: a rectangle for a step, a trapezoid for a curve.
    for (i, &(beat, bpm)) in points.iter().enumerate() {
        let y = y_of(bpm, lane, r);
        let x0 = x_of(beat);
        let a = pos2(x0, y);
        line.push(a);
        let (end, end_y) = match points.get(i + 1) {
            Some(&(b, next_bpm)) if all[i].ramp => (x_of(b), y_of(next_bpm, lane, r)),
            Some(&(b, _)) => (x_of(b), y),
            None => (lane.right() + 1.0, y),
        };
        let b = pos2(end, end_y);
        // A step is two points at the same height; a curve goes straight to the next point,
        // which pushes its own start.
        if end_y == y {
            line.push(b);
        }
        // Only the stretches in view are filled.
        if b.x < lane.left() || a.x > lane.right() {
            continue;
        }
        g.add(Shape::convex_polygon(
            vec![a, b, pos2(b.x, lane.bottom()), pos2(a.x, lane.bottom())],
            FILL,
            Stroke::NONE,
        ));
    }
    g.add(Shape::line(line, Stroke::new(1.5, LINE)));

    // ---- points
    let mut delete = None;
    let mut ramp_change = None;
    for (i, &(beat, bpm)) in points.iter().enumerate() {
        let at = pos2(x_of(beat), y_of(bpm, lane, r));
        if at.x < lane.left() - 6.0 || at.x > lane.right() + 6.0 {
            continue;
        }
        let hit = Rect::from_center_size(at, vec2(12.0, 12.0));
        let resp = ui.interact(
            hit,
            egui::Id::unique(("tempo_point", i)),
            Sense::click_and_drag(),
        );
        if resp.drag_started() {
            ui.ctx()
                .data_mut(|m| m.insert_temp(range_id.with("drag"), resp.id));
        }
        if resp.dragged() {
            if let Some(pt) = resp.interact_pointer_pos() {
                let bpm = bpm_of(pt.y, lane, r);
                if i == 0 {
                    p.tempo = bpm;
                } else {
                    // Between its neighbours, so the order of the changes never flips.
                    let prev = points[i - 1].0;
                    let next = points.get(i + 1).map_or(f64::INFINITY, |n| n.0);
                    let lo = prev + MIN_GAP;
                    let hi = next - MIN_GAP;
                    let beat = if lo <= hi {
                        snap(beat_of(pt.x)).clamp(lo, hi)
                    } else {
                        beat
                    };
                    p.tempo_changes[i - 1].beat = beat;
                    p.tempo_changes[i - 1].bpm = bpm;
                }
            }
        }
        if resp.drag_stopped() {
            p.tidy_tempo();
        }
        if i > 0 && resp.double_clicked() {
            delete = Some(i - 1);
        }
        let has_next = i + 1 < points.len();
        let ramps = all[i].ramp;
        context_menu(&resp, |ui| {
            if has_next {
                let text = if ramps {
                    tr("Hold Tempo Until Next Change")
                } else {
                    tr("Ramp to Next Tempo Change")
                };
                if ui.button(text).clicked() {
                    ramp_change = Some((i, !ramps));
                    ui.close();
                }
            }
            if i > 0 && ui.button(tr("Delete Tempo Change")).clicked() {
                delete = Some(i - 1);
                ui.close();
            }
        });
        let hot = resp.hovered() || resp.dragged() || context_click(&resp);
        g.circle_filled(at, if hot { 4.5 } else { 3.5 }, LINE);
        if hot || resp.dragged() {
            g.text(
                at + vec2(6.0, -4.0),
                Align2::LEFT_BOTTOM,
                format!("{bpm:.0}"),
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
    }
    if let Some((i, ramp)) = ramp_change {
        p.set_tempo_ramp(i, ramp);
    }
    if let Some(i) = delete {
        if i < p.tempo_changes.len() {
            p.tempo_changes.remove(i);
            p.tidy_tempo();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_range_holds_every_tempo_with_a_margin() {
        let mut p = Project::empty();
        p.tempo = 120.0;
        assert_eq!(range(&p), (110.0, 130.0));
        p.add_tempo_change(16.0, 90.0);
        p.add_tempo_change(32.0, 171.0);
        assert_eq!(range(&p), (80.0, 180.0));
        p.tempo = 6.0;
        assert_eq!(range(&p).0, MIN_BPM);
    }

    #[test]
    fn y_and_bpm_are_inverse_within_the_lane() {
        let lane = Rect::from_min_size(pos2(0.0, 100.0), vec2(500.0, HEIGHT));
        let r = (100.0, 140.0);
        for bpm in [100.0, 117.0, 140.0] {
            assert_eq!(bpm_of(y_of(bpm, lane, r), lane, r), bpm);
        }
        assert!(
            y_of(140.0, lane, r) < y_of(100.0, lane, r),
            "faster is higher"
        );
    }
}
