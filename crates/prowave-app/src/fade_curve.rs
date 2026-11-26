//! The shape of a fade, changed with the Fade tool (Logic Pro: Control-Shift-drag on a fade
//! with the Fade tool changes its curve; Region Inspector > Curve), and the crossfade as it is
//! drawn.
//!
//! Logic's curve is a continuous value. A region here has one of three shapes
//! (`prowave_engine::FadeCurve`), the ones the Region Inspector's Curve offers, and they are
//! what the engine plays. A drag steps through them in order of how much level the fade
//! keeps: up, the fade bulges more (S-Curve, then Linear, then Equal Power); down, less.
//! One region has one shape for its fade in and its fade out, as the Inspector shows.
//!
//! A crossfade has two curves: the later region's fade in and the earlier one's fade out,
//! which plays under it. The later region is drawn over the earlier one, so its picture also
//! carries the earlier one's fade-out line, which would otherwise be hidden (`under`).

use crate::model::{Project, TrackKind};
use prowave_engine::FadeCurve;

/// The shapes in order of how much level the fade keeps along the way.
pub(crate) const ORDER: [FadeCurve; 3] =
    [FadeCurve::SCurve, FadeCurve::Linear, FadeCurve::EqualPower];

/// How far the pointer goes (points) for one step to the next shape.
pub(crate) const STEP: f32 = 16.0;

/// A curve drag in progress.
#[derive(Clone)]
pub struct CurveDrag {
    /// The regions whose curve it sets.
    targets: Vec<(usize, usize)>,
    /// The shape the dragged fade had (an index in [`ORDER`]).
    from: usize,
}

/// Which of region `r`'s fades (in, out) on track `t` is at `at` (quarter notes), with its
/// crossfades: `Some(true)` for the fade in.
fn fade_at(p: &Project, (t, r): (usize, usize), at: f64) -> Option<bool> {
    let reg = p.tracks.get(t)?.regions.get(r)?;
    let (start, end) = (p.region_start_beats(reg), p.region_end_beats(reg));
    let (fade_in, fade_out) = p.effective_fades(t, r);
    if fade_in > 0.0 && at >= start - 1e-9 && at <= start + f64::from(fade_in) + 1e-9 {
        Some(true)
    } else if fade_out > 0.0 && at <= end + 1e-9 && at >= end - f64::from(fade_out) - 1e-9 {
        Some(false)
    } else {
        None
    }
}

/// The audio region of track `t` that region `r` crossfades with at its start (`fade_in`) or
/// at its end.
fn partner(p: &Project, (t, r): (usize, usize), fade_in: bool) -> Option<usize> {
    let track = p.tracks.get(t)?;
    let reg = track.regions.get(r)?;
    let (s, e) = (p.region_start_beats(reg), p.region_end_beats(reg));
    track.regions.iter().enumerate().position(|(j, o)| {
        if j == r || o.audio.is_none() || o.params.mute {
            return false;
        }
        let (os, oe) = (p.region_start_beats(o), p.region_end_beats(o));
        if fade_in {
            reg.params.crossfade && os < s && s < oe
        } else {
            o.params.crossfade && s < os && os < e && oe >= e
        }
    })
}

/// A Control-Shift drag with the Fade tool begins at `at` on region `r` of track `t`.
/// `None` unless it is on one of the region's fades. Every selected audio region takes the
/// shape when the region is selected (as a Fade tool drag sets every selected one's fade), and
/// so does the region it crossfades with there.
pub fn begin(
    p: &Project,
    selected: &[(usize, usize)],
    (t, r): (usize, usize),
    at: f64,
) -> Option<CurveDrag> {
    let track = p.tracks.get(t)?;
    if track.kind != TrackKind::Audio || !track.regions.get(r)?.is_audio() {
        return None;
    }
    let fade_in = fade_at(p, (t, r), at)?;
    let mut targets: Vec<(usize, usize)> = if selected.contains(&(t, r)) {
        selected
            .iter()
            .copied()
            .filter(|&(st, sr)| {
                p.tracks.get(st).is_some_and(|x| {
                    x.kind == TrackKind::Audio && x.regions.get(sr).is_some_and(|g| g.is_audio())
                })
            })
            .collect()
    } else {
        vec![(t, r)]
    };
    if let Some(o) = partner(p, (t, r), fade_in) {
        if !targets.contains(&(t, o)) {
            targets.push((t, o));
        }
    }
    let shape = p.tracks[t].regions[r].params.fade_curve;
    let from = ORDER.iter().position(|&c| c == shape).unwrap_or(2);
    Some(CurveDrag { targets, from })
}

/// The shape for a drag `dy` points down from where it began.
pub fn shape_for(d: &CurveDrag, dy: f32) -> FadeCurve {
    let steps = (-dy / STEP).round() as isize;
    let i = (d.from as isize + steps).clamp(0, ORDER.len() as isize - 1) as usize;
    ORDER[i]
}

/// The drag is `dy` points below where it began: every target takes the shape for it.
/// Returns the shape.
pub fn drag_to(p: &mut Project, d: &CurveDrag, dy: f32) -> FadeCurve {
    let shape = shape_for(d, dy);
    for &(t, r) in &d.targets {
        if let Some(reg) = p.tracks.get_mut(t).and_then(|x| x.regions.get_mut(r)) {
            reg.params.fade_curve = shape;
        }
    }
    shape
}

/// The Fade tool on region `r` of track `t` (its response `resp`) with Control and Shift held
/// (`keys`): a drag that begins on a fade changes its curve, and the shape it has reached is
/// printed by the pointer. Returns whether the project changed.
pub(crate) fn on_region(
    ui: &egui::Ui,
    p: &mut Project,
    v: &crate::tracks_area::TracksView,
    resp: &egui::Response,
    (t, r): (usize, usize),
    keys: bool,
    beat_of: &dyn Fn(f32) -> f64,
) -> bool {
    let id = egui::Id::unique(("fade_curve_drag", t, r));
    let held: Option<CurveDrag> = ui.data(|d| d.get_temp(id));
    if keys && held.is_none() {
        let on_fade = resp
            .hover_pos()
            .is_some_and(|pt| fade_at(p, (t, r), beat_of(pt.x)).is_some());
        if on_fade {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
    }
    let from = ui.input(|i| i.pointer.press_origin());
    if keys && resp.drag_started() {
        if let Some(pt) = from.or(resp.interact_pointer_pos()) {
            if let Some(d) = begin(p, &v.selection(), (t, r), beat_of(pt.x)) {
                ui.data_mut(|m| m.insert_temp(id, d));
            }
        }
    }
    let Some(d) = ui.data(|m| m.get_temp::<CurveDrag>(id)) else {
        return false;
    };
    let mut changed = false;
    if let (Some(a), Some(pt)) = (from, resp.interact_pointer_pos()) {
        let before = p.tracks[t].regions.get(r).map(|x| x.params.fade_curve);
        let shape = drag_to(p, &d, pt.y - a.y);
        changed = before != Some(shape);
        let name = crate::inspector::FADE_CURVES
            .iter()
            .find(|(_, c)| *c == shape)
            .map_or("", |(n, _)| crate::i18n::tr(n));
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("fade_curve_label"),
        ));
        let text = format!("{} {}", crate::i18n::tr("Curve:"), name).to_uppercase();
        let galley = painter.layout_no_wrap(
            text,
            crate::theme::font(crate::theme::Face::Lcd, 12.0),
            crate::theme::LCD_INK(),
        );
        let rect = egui::Rect::from_min_size(
            pt + egui::vec2(14.0, -8.0 - galley.size().y),
            galley.size() + egui::vec2(10.0, 4.0),
        );
        painter.rect_filled(rect, 2.0, crate::theme::LCD());
        painter.galley(
            rect.min + egui::vec2(5.0, 2.0),
            galley,
            crate::theme::LCD_INK(),
        );
    }
    if resp.drag_stopped() || !resp.dragged() {
        ui.data_mut(|m| m.remove::<CurveDrag>(id));
        changed = true;
    }
    changed
}

/// See [`under`]: (start from this region's start, length, shape).
pub type Under = (f32, f32, FadeCurve);

/// What a region's picture carries of the region it crossfades with at its start: where that
/// region's fade out begins, from this one's start (quarter notes; below zero when it begins
/// earlier, as when its own fade out is longer than the overlap), how long it is, and its
/// shape.
pub fn under(p: &Project, t: usize, r: usize) -> Option<Under> {
    let o = partner(p, (t, r), true)?;
    let fade_out = p.effective_fades(t, o).1;
    if fade_out <= 0.0 {
        return None;
    }
    let tr = &p.tracks[t];
    let start = p.region_start_beats(&tr.regions[r]);
    let other_end = p.region_end_beats(&tr.regions[o]);
    let from = (other_end - f64::from(fade_out) - start) as f32;
    Some((from, fade_out, tr.regions[o].params.fade_curve))
}

/// The level (0..=1) a fade of shape `c` has reached a fraction `x` of the way in, as the
/// engine plays it.
pub fn level(c: FadeCurve, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    match c {
        FadeCurve::Linear => x,
        FadeCurve::EqualPower => (x * std::f32::consts::FRAC_PI_2).sin(),
        FadeCurve::SCurve => x * x * (3.0 - 2.0 * x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marquee::rig::Rig;
    use crate::tracks_area::TrackTool;
    use crate::undo::History;
    use egui::{vec2, Modifiers};

    const CTRL_SHIFT: Modifiers = Modifiers {
        alt: false,
        ctrl: true,
        shift: true,
        mac_cmd: false,
        command: false,
    };

    #[test]
    fn the_shapes_go_up_in_the_level_they_keep() {
        for w in ORDER.windows(2) {
            assert!(level(w[0], 0.25) < level(w[1], 0.25));
        }
        // What is drawn is what the engine plays: the same formulas.
        assert_eq!(level(FadeCurve::Linear, 0.3), 0.3);
        assert!((level(FadeCurve::EqualPower, 0.5) - 0.70710677).abs() < 1e-6);
        assert_eq!(level(FadeCurve::SCurve, 0.5), 0.5);
    }

    #[test]
    fn control_shift_drag_on_a_fade_steps_its_curve_in_one_undo_step() {
        let mut p = crate::fade_tool::tests::project();
        p.tracks[1].regions[0].params.fade_in = 2.0;
        let mut r = Rig::new(p);
        r.s.tracks.tool = TrackTool::Fade;
        let mut hist = History::new(&r.p);
        assert_eq!(
            r.p.tracks[1].regions[0].params.fade_curve,
            FadeCurve::EqualPower
        );
        // Down two steps: Linear, then S-Curve. Nothing else changes.
        let from = r.at(1.0, 1);
        r.drag(from, from + vec2(0.0, 2.0 * STEP + 2.0), CTRL_SHIFT);
        let reg = &r.p.tracks[1].regions[0];
        assert_eq!(reg.params.fade_curve, FadeCurve::SCurve);
        assert_eq!((reg.params.fade_in, reg.params.fade_out), (2.0, 0.0));
        assert_eq!(r.spans(1), vec![(1.0, 3.0)]);
        assert!(hist.record(&r.p));
        assert!(hist.undo(&mut r.p));
        assert!(!hist.can_undo(), "one step");
        assert_eq!(
            r.p.tracks[1].regions[0].params.fade_curve,
            FadeCurve::EqualPower
        );
        // One step down: Linear; up from there past the top: Equal Power.
        r.drag(from, from + vec2(0.0, STEP), CTRL_SHIFT);
        assert_eq!(
            r.p.tracks[1].regions[0].params.fade_curve,
            FadeCurve::Linear
        );
        r.drag(from, from - vec2(0.0, 4.0 * STEP), CTRL_SHIFT);
        assert_eq!(
            r.p.tracks[1].regions[0].params.fade_curve,
            FadeCurve::EqualPower
        );
        // Away from the fade it does nothing (no fade is made either).
        let away = r.at(5.0, 1);
        r.drag(away, away + vec2(0.0, 2.0 * STEP), CTRL_SHIFT);
        let reg = &r.p.tracks[1].regions[0];
        assert_eq!(reg.params.fade_curve, FadeCurve::EqualPower);
        assert_eq!((reg.params.fade_in, reg.params.fade_out), (2.0, 0.0));
    }

    #[test]
    fn a_curve_drag_in_a_crossfade_shapes_both_of_its_regions() {
        let mut p = crate::fade_tool::tests::project();
        let d = crate::fade_tool::begin(&p, &[], (0, 0), 7.0).expect("an audio region");
        crate::fade_tool::drag_to(&mut p, &d, 9.0);
        // In the crossfade (beats 7-8), on the later region.
        let c = begin(&p, &[], (0, 1), 7.5).expect("on a fade");
        assert_eq!(drag_to(&mut p, &c, STEP), FadeCurve::Linear);
        assert_eq!(p.tracks[0].regions[0].params.fade_curve, FadeCurve::Linear);
        assert_eq!(p.tracks[0].regions[1].params.fade_curve, FadeCurve::Linear);
        // The later one's picture carries the earlier one's fade out: a beat from its start,
        // Linear.
        assert_eq!(under(&p, 0, 1), Some((0.0, 1.0, FadeCurve::Linear)));
        // An earlier region whose own fade out is longer than the overlap: its line begins
        // before the later region does, where the engine begins that fade.
        p.tracks[0].regions[0].params.fade_out = 3.0;
        assert_eq!(under(&p, 0, 1), Some((-2.0, 3.0, FadeCurve::Linear)));
        assert_eq!(under(&p, 0, 0), None);
        // Selected regions take the shape together.
        let mut p = crate::fade_tool::tests::project();
        p.tracks[0].regions[0].params.fade_out = 1.0;
        p.tracks[1].regions[0].params.fade_out = 1.0;
        let sel = [(0, 0), (1, 0)];
        let c = begin(&p, &sel, (1, 0), 7.5).expect("on a fade");
        drag_to(&mut p, &c, 2.0 * STEP);
        assert_eq!(p.tracks[0].regions[0].params.fade_curve, FadeCurve::SCurve);
        assert_eq!(p.tracks[1].regions[0].params.fade_curve, FadeCurve::SCurve);
        assert_eq!(
            p.tracks[0].regions[1].params.fade_curve,
            FadeCurve::EqualPower
        );
    }

    /// The picture of a crossfade (the earlier region's fade out crossing the later one's fade
    /// in) and of a region with both fades, in the Tracks area.
    #[test]
    #[ignore = "picture test: writes prowave-crossfade.png to the temp folder"]
    fn crossfade_picture() {
        let mut p = crate::fade_tool::tests::project();
        let d = crate::fade_tool::begin(&p, &[], (0, 0), 6.0).expect("an audio region");
        crate::fade_tool::drag_to(&mut p, &d, 10.0);
        p.tracks[0].regions[0].params.fade_curve = FadeCurve::Linear;
        p.tracks[1].regions[0].params.fade_in = 2.0;
        p.tracks[1].regions[0].params.fade_out = 3.0;
        p.tracks[1].regions[0].params.fade_curve = FadeCurve::SCurve;
        let mut s = crate::UiState::new();
        let meters = crate::mixer::Meters::default();
        let out = std::env::temp_dir().join("prowave-crossfade.png");
        crate::ui_test::render_png(
            900.0,
            300.0,
            2.0,
            egui::Color32::from_gray(0x2a),
            |ui| {
                crate::theme::apply(ui.ctx());
                crate::tracks_area::show(ui, &mut p, &mut s, &meters)
            },
            &out,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(out.exists());
    }
}
