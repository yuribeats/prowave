//! Loop (Logic Pro: the Loop tool on the upper half of a region's right edge). Dragging there
//! to the right repeats the region after its end, up to where the pointer is let go (snapped);
//! dragging back to the left shortens the loop, and back to the region's end takes it off. The
//! repeats are stored as one length on the region (`RegionParams::loop_beats`) and played,
//! bounced and exported from `Project::loop_repeats`. The lower half of the edge keeps
//! shortening and lengthening the region itself.

use crate::model::Project;
use crate::theme;
use egui::{pos2, Id, Painter, Pos2, Rect, Sense, Stroke, Ui};

/// Width of the loop handle at the end of a region or of its loop, in points.
const HANDLE_W: f32 = 5.0;

/// The loop handle of region `at` (track, region) drawn in `full`: the upper half of the right
/// edge of the region, or of its loop when it has one. Returns whether a loop drag ended this
/// frame (the song changed).
#[allow(clippy::too_many_arguments)]
pub fn loop_handle(
    ui: &Ui,
    p: &mut Project,
    at: (usize, usize),
    full: Rect,
    lanes: Rect,
    id: Id,
    x_of: &impl Fn(f64) -> f32,
    beat_of: &impl Fn(f32) -> f64,
    snap_pos: &impl Fn(f64) -> f64,
) -> bool {
    let Some(region) = p.tracks.get(at.0).and_then(|t| t.regions.get(at.1)) else {
        return false;
    };
    let end = p.region_end_beats(region);
    let x = x_of(p.region_loop_end_beats(region));
    let grip = Rect::from_min_max(pos2(x - HANDLE_W, full.top()), pos2(x, full.center().y))
        .intersect(lanes);
    if grip.width() <= 0.0 || grip.height() <= 0.0 {
        return false;
    }
    let resp = ui.interact(grip, id, Sense::drag());
    if resp.hovered() || resp.dragged() {
        // Logic's Loop pointer: a circular arrow, drawn in place of the system arrow.
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        if let Some(pt) = ui.ctx().pointer_latest_pos() {
            let layer = egui::LayerId::new(egui::Order::Tooltip, id.with("cursor"));
            loop_glyph(&ui.ctx().layer_painter(layer), pt);
        }
    }
    if let (true, Some(pt)) = (resp.dragged(), resp.interact_pointer_pos()) {
        let beats = (snap_pos(beat_of(pt.x)) - end).max(0.0) as f32;
        p.tracks[at.0].regions[at.1].params.loop_beats = beats;
    }
    resp.drag_stopped()
}

/// The Loop pointer: a ring with an arrowhead, in ink on a bone halo so it reads on any region.
pub fn loop_glyph(g: &Painter, at: Pos2) {
    let c = at + egui::vec2(7.0, 7.0);
    let ring = |r: f32| -> Vec<Pos2> {
        (0..=20)
            .map(|i| {
                // From the top round to the left, leaving a gap at the upper right.
                let a = -1.2 + i as f32 / 20.0 * 5.0;
                c + r * egui::vec2(a.cos(), a.sin())
            })
            .collect()
    };
    let tip = c + 5.5 * egui::vec2((-1.2f32).cos(), (-1.2f32).sin());
    let head = [
        tip + egui::vec2(-3.5, -2.5),
        tip + egui::vec2(1.5, 1.0),
        tip + egui::vec2(-3.0, 3.0),
    ];
    for (width, colour) in [(4.0, theme::BONE), (1.8, theme::INK)] {
        g.add(egui::Shape::line(ring(5.5), Stroke::new(width, colour)));
        g.add(egui::Shape::convex_polygon(
            head.to_vec(),
            colour,
            Stroke::new(width - 1.8, theme::BONE),
        ));
    }
}

/// The mark where a repeat of a looped region begins: a seam line down its height, with a
/// notch at the top.
pub fn repeat_mark(g: &Painter, x: f32, top: f32, bottom: f32) {
    g.line_segment(
        [pos2(x, top), pos2(x, bottom)],
        Stroke::new(1.5, theme::INK2),
    );
    g.add(egui::Shape::convex_polygon(
        vec![pos2(x, top), pos2(x + 5.0, top), pos2(x, top + 5.0)],
        theme::INK2,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use crate::model::{Note, Project, Region};
    use crate::ui_test::Harness;
    use egui::{vec2, PointerButton, Pos2, Rect};

    /// One instrument track with a region of `bars` bars at bar 1.
    fn rig(bars: f32) -> (Project, crate::UiState) {
        let mut p = Project::empty();
        p.tracks[0].regions.push(Region {
            missing: None,
            name: "Beat".into(),
            start_bar: 1.0,
            len_bars: bars,
            notes: vec![Note {
                start: 0.0,
                len: 0.5,
                pitch: 36,
                vel: 100,
                mute: false,
                lyric: crate::lyrics::Lyric::NONE,
            }],
            controls: Vec::new(),
            folder: None,
            audio: None,
            params: Default::default(),
            generated: None,
        });
        (p, crate::UiState::new())
    }

    fn show(h: &mut Harness, p: &mut Project, s: &mut crate::UiState) {
        let meters = crate::mixer::Meters::default();
        h.settle(|ui| crate::tracks_area::show(ui, p, s, &meters));
    }

    fn rect_of(h: &Harness) -> Rect {
        h.ctx
            .read_response(egui::Id::unique((
                "tracks_area",
                ("region", 0usize, 0usize),
            )))
            .expect("the region is on screen")
            .rect
    }

    fn drag(h: &mut Harness, p: &mut Project, s: &mut crate::UiState, from: Pos2, dx: f32) {
        let meters = crate::mixer::Meters::default();
        let path = [
            from,
            from + vec2(dx / 3.0, 0.0),
            from + vec2(dx * 2.0 / 3.0, 0.0),
            from + vec2(dx, 0.0),
        ];
        h.drag(&path, PointerButton::Primary, |ui| {
            crate::tracks_area::show(ui, p, s, &meters)
        });
    }

    /// Points of one bar in the tracks area (a bar is 96 points wide at the start).
    const BAR: f32 = 96.0;

    #[test]
    fn dragging_the_upper_right_corner_loops_the_region_and_back_shortens_it() {
        let (mut p, mut s) = rig(1.0);
        let mut h = Harness::new(1400.0, 600.0);
        show(&mut h, &mut p, &mut s);
        let r = rect_of(&h);
        drag(
            &mut h,
            &mut p,
            &mut s,
            pos(r.right() - 2.0, r.top() + 3.0),
            2.0 * BAR,
        );
        let region = &p.tracks[0].regions[0];
        assert_eq!(region.params.loop_beats, 8.0, "two bars of repeats");
        assert_eq!(region.len_bars, 1.0, "the region itself keeps its length");
        assert_eq!(p.loop_repeats(region), vec![(4.0, 4.0), (8.0, 4.0)]);
        assert_eq!(region.start_bar, 1.0, "not moved");

        // The handle is now at the end of the loop: back a bar and a half, snapped.
        show(&mut h, &mut p, &mut s);
        let end = r.right() + 2.0 * BAR;
        drag(
            &mut h,
            &mut p,
            &mut s,
            pos(end - 2.0, r.top() + 3.0),
            -1.5 * BAR,
        );
        let region = &p.tracks[0].regions[0];
        let left = region.params.loop_beats;
        assert!(left > 0.0 && left < 8.0, "shorter: {left}");
        assert_eq!(left.fract(), 0.0, "on the grid: {left}");
        // Back to the region's end: no loop.
        show(&mut h, &mut p, &mut s);
        let end = r.right() + left * BAR / 4.0;
        drag(
            &mut h,
            &mut p,
            &mut s,
            pos(end - 2.0, r.top() + 3.0),
            -3.0 * BAR,
        );
        assert_eq!(p.tracks[0].regions[0].params.loop_beats, 0.0);
        assert!(p.loop_repeats(&p.tracks[0].regions[0]).is_empty());
    }

    #[test]
    fn the_lower_half_of_the_right_edge_still_trims() {
        let (mut p, mut s) = rig(2.0);
        let mut h = Harness::new(1400.0, 600.0);
        show(&mut h, &mut p, &mut s);
        let r = rect_of(&h);
        drag(
            &mut h,
            &mut p,
            &mut s,
            pos(r.right() - 2.0, r.bottom() - 3.0),
            -BAR,
        );
        let region = &p.tracks[0].regions[0];
        assert_eq!(region.len_bars, 1.0, "a bar shorter");
        assert_eq!(region.params.loop_beats, 0.0, "not looped");
    }

    #[test]
    fn undo_takes_the_loop_off_in_one_step() {
        let (mut p, mut s) = rig(1.0);
        let mut history = crate::undo::History::new(&p);
        let mut h = Harness::new(1400.0, 600.0);
        show(&mut h, &mut p, &mut s);
        let r = rect_of(&h);
        drag(
            &mut h,
            &mut p,
            &mut s,
            pos(r.right() - 2.0, r.top() + 3.0),
            3.0 * BAR,
        );
        assert_eq!(p.tracks[0].regions[0].params.loop_beats, 12.0);
        assert!(history.record(&p));
        assert!(history.undo(&mut p));
        assert_eq!(p.tracks[0].regions[0].params.loop_beats, 0.0);
        assert!(!history.can_undo(), "one step");
    }

    #[test]
    fn a_loop_is_saved_and_a_file_from_before_loops_opens_without_one() {
        let (mut p, _) = rig(1.0);
        p.tracks[0].regions[0].params.loop_beats = 6.0;
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let (doc, _) = crate::project_io::to_doc(&p);
        let (back, _) = crate::project_io::from_doc(&doc, dir.path(), &ctx);
        assert_eq!(back.tracks[0].regions[0].params.loop_beats, 6.0);

        // A file written before loops existed has no "loop_beats" at all.
        let mut json = serde_json::to_value(&doc).unwrap();
        fn strip(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(m) => {
                    m.remove("loop_beats");
                    m.values_mut().for_each(strip);
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
                _ => {}
            }
        }
        strip(&mut json);
        assert!(!json.to_string().contains("loop_beats"));
        let old: prowave_project::ProjectDoc = serde_json::from_value(json).unwrap();
        let (back, warnings) = crate::project_io::from_doc(&old, dir.path(), &ctx);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.tracks[0].regions[0].params.loop_beats, 0.0);
        assert_eq!(back.tracks[0].regions[0].notes.len(), 1);
    }

    #[test]
    fn a_cut_leaves_the_loop_on_the_right_piece() {
        let (mut p, _) = rig(2.0);
        p.tracks[0].regions[0].params.loop_beats = 8.0;
        assert!(p.split_region(0, 0, 4.0));
        let (l, r) = (&p.tracks[0].regions[0], &p.tracks[0].regions[1]);
        assert_eq!((l.params.loop_beats, r.params.loop_beats), (0.0, 8.0));
    }

    fn pos(x: f32, y: f32) -> Pos2 {
        Pos2::new(x, y)
    }
}
