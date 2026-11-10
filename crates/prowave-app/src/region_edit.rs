//! Edit > Cut, Copy, Paste, Split at Playhead and Join on the selected regions (Logic Pro: Edit
//! menu of the Tracks area). Select All is in `tracks_area.rs`, with the selection itself.
//!
//! Logic behaviour followed here:
//! - Copy and Cut take every selected region; Paste puts them on the selected track at the
//!   playhead, keeping the distances between them.
//! - Split at Playhead cuts every selected region that the playhead is inside of, as the
//!   Scissors tool does (`Project::split_region`): a note that crosses the cut ends there, the
//!   right half of an audio region plays on from the same place in its file, and the right
//!   pieces are what gets selected.
//! - Join merges the selected regions of each track into one region that spans them all, as the
//!   Join tool does (`Project::join_group`): MIDI regions keep each note where it was; audio
//!   regions join when they are consecutive parts of one file. The joined region takes the
//!   earliest region's name and settings.
//!
//! Pattern, Drummer and take-folder regions, and reversed audio, are not cut or joined, because
//! their notes or sound are made from settings or takes.

use crate::model::{Project, Region, TrackKind};
use crate::tracks_area::TracksView;

/// The regions the clipboard holds (Edit > Copy / Cut), earliest first.
pub(crate) type RegionClip = Vec<Region>;

/// A MIDI region with its own notes: the only kind that can be split or joined.
fn plain_midi(r: &Region) -> bool {
    r.audio.is_none() && r.folder.is_none() && r.generated.is_none()
}

/// Copies every selected region to the clipboard. Returns whether there was one.
pub(crate) fn copy_selected(p: &Project, v: &TracksView, clip: &mut RegionClip) -> bool {
    let mut picked: Vec<Region> = v
        .selection()
        .into_iter()
        .filter_map(|(t, r)| p.tracks.get(t)?.regions.get(r).cloned())
        .collect();
    if picked.is_empty() {
        return false;
    }
    picked.sort_by(|a, b| a.start_bar.total_cmp(&b.start_bar));
    *clip = picked;
    true
}

/// Copies the selected regions and removes them from their tracks.
pub(crate) fn cut_selected(p: &mut Project, v: &mut TracksView, clip: &mut RegionClip) {
    if copy_selected(p, v, clip) {
        crate::tracks_area::delete_selected(p, v);
    }
}

/// Whether the clipboard regions fit the selected track: audio regions go on audio tracks, the
/// others on tracks that play MIDI.
pub(crate) fn can_paste(p: &Project, clip: &RegionClip) -> bool {
    let Some(track) = p.tracks.get(p.selected_track) else {
        return false;
    };
    !clip.is_empty()
        && track.kind.has_regions()
        && clip
            .iter()
            .all(|r| r.audio.is_some() == (track.kind == TrackKind::Audio))
}

/// Puts copies of the clipboard regions on the selected track, the earliest at the playhead,
/// and selects them.
pub(crate) fn paste_at_playhead(p: &mut Project, v: &mut TracksView, clip: &RegionClip) {
    if !can_paste(p, clip) {
        return;
    }
    let base = clip.iter().map(|r| r.start_bar).fold(f32::MAX, f32::min);
    let playhead_bar = p.beats_to_bar(p.position_beats) as f32 + 1.0;
    let t = p.selected_track;
    let Some(track) = p.tracks.get_mut(t) else {
        return;
    };
    let mut made = Vec::new();
    for r in clip {
        track.regions.push(Region {
            start_bar: playhead_bar + (r.start_bar - base),
            ..r.clone()
        });
        made.push((t, track.regions.len() - 1));
    }
    v.select_many(&made);
    p.song_rev += 1;
}

/// Whether every region of a drag (track, region, start) would find a track of its kind
/// `rows` tracks further down (up when negative): audio regions go on audio tracks, the others
/// on tracks that play MIDI.
pub(crate) fn fits_rows(p: &Project, group: &[(usize, usize, f64)], rows: isize) -> bool {
    group.iter().all(|&(t, r, _)| {
        let Some(region) = p.tracks.get(t).and_then(|tr| tr.regions.get(r)) else {
            return false;
        };
        let Some(to) = t.checked_add_signed(rows).and_then(|to| p.tracks.get(to)) else {
            return false;
        };
        to.kind.has_regions() && region.audio.is_some() == (to.kind == TrackKind::Audio)
    })
}

/// The bar (1-based) a region that started at `start` lands on, moved by `delta` quarter notes
/// (never before the start of the song).
pub(crate) fn landing_bar(p: &Project, start: f64, delta: f64) -> f32 {
    p.beats_to_bar((start + delta).max(0.0)) as f32 + 1.0
}

/// The end of a Pointer-tool drag of regions, as in Logic. Each region of `group` (track,
/// region, its start in quarter notes when the drag began) lands `delta` quarter notes later
/// and `rows` tracks further down. With `copy` (Option held when the button is let go) the
/// regions stay where they were and copies of them land there instead, the same in everything
/// but their place. The automation under them goes along (Automation: Move with Regions), and
/// with `xfade` (Drag: X-Fade) an audio region dropped over another crossfades with it.
/// Returns where the landed regions are now, to be selected.
pub(crate) fn drop_regions(
    p: &mut Project,
    auto: &crate::automation::AutoView,
    group: &[(usize, usize, f64)],
    delta: f64,
    rows: isize,
    copy: bool,
    xfade: bool,
) -> Vec<(usize, usize)> {
    let group: Vec<(usize, usize, f64)> = group
        .iter()
        .copied()
        .filter(|&(t, r, _)| p.tracks.get(t).is_some_and(|tr| r < tr.regions.len()))
        .collect();
    let rows = if fits_rows(p, &group, rows) { rows } else { 0 };
    let mut landed: Vec<(usize, usize)> = Vec::new();
    if !copy && rows == 0 {
        // A plain move: each region takes its new place on its own track.
        for &(t, r, s0) in &group {
            p.tracks[t].regions[r].start_bar = landing_bar(p, s0, delta);
        }
        crate::automation_ui::follow_moves(p, auto, &group);
        landed.extend(group.iter().map(|&(t, r, _)| (t, r)));
    } else {
        let mut made: Vec<(usize, Region)> = Vec::new();
        let mut spans = Vec::new();
        for &(t, r, s0) in &group {
            // The region itself stays where it began (the drag may have moved it along).
            let back = p.beats_to_bar(s0) as f32 + 1.0;
            p.tracks[t].regions[r].start_bar = back;
            let to = t.saturating_add_signed(rows);
            let mut region = p.tracks[t].regions[r].clone();
            region.start_bar = landing_bar(p, s0, delta);
            let len = p.region_len_beats(&p.tracks[t].regions[r]);
            spans.push((t, s0, len, to, p.region_start_beats(&region)));
            made.push((to, region));
        }
        if !copy {
            // Moved to other tracks: taken off their own ones, the highest index first so
            // the lower ones stay valid.
            let mut gone: Vec<(usize, usize)> = group.iter().map(|&(t, r, _)| (t, r)).collect();
            gone.sort_unstable();
            gone.dedup();
            for &(t, r) in gone.iter().rev() {
                p.tracks[t].regions.remove(r);
            }
        }
        for (to, region) in made {
            p.tracks[to].regions.push(region);
            landed.push((to, p.tracks[to].regions.len() - 1));
        }
        crate::automation_ui::follow_drops(p, auto, &spans, !copy);
    }
    for &(t, r) in &landed {
        if p.tracks[t].kind == TrackKind::Audio && crate::tracks_area::overlaps_another(p, t, r) {
            p.tracks[t].regions[r].params.crossfade = xfade;
        }
    }
    p.song_rev += 1;
    landed
}

/// Whether Split at Playhead does anything to the selection.
pub(crate) fn can_split(p: &Project, v: &TracksView) -> bool {
    v.selection()
        .into_iter()
        .any(|(t, r)| p.can_split_region(t, r, p.position_beats))
}

/// Split at Playhead: every selected region the playhead is inside of is cut. The right pieces
/// become the selection.
pub(crate) fn split_at_playhead(p: &mut Project, v: &mut TracksView) {
    let at = p.position_beats;
    let mut targets: Vec<(usize, usize)> = v
        .selection()
        .into_iter()
        .filter(|&(t, r)| p.can_split_region(t, r, at))
        .collect();
    // Highest index first on each track: a cut puts its right piece right after the left one,
    // so the lower indexes stay valid, and the pieces already made move up by one.
    targets.sort_unstable();
    targets.reverse();
    let mut made: Vec<(usize, usize)> = Vec::new();
    for (t, r) in targets {
        if p.split_region(t, r, at) {
            for m in made.iter_mut().filter(|m| m.0 == t && m.1 > r) {
                m.1 += 1;
            }
            made.push((t, r + 1));
        }
    }
    if !made.is_empty() {
        v.select_many(&made);
    }
}

/// The groups Join merges: per track, the indexes of its selected regions, when they can be
/// joined (two or more; see `Project::can_join_group`).
fn join_groups(p: &Project, v: &TracksView) -> Vec<(usize, Vec<usize>)> {
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (t, r) in v.selection() {
        match groups.iter_mut().find(|(gt, _)| *gt == t) {
            Some((_, rs)) => rs.push(r),
            None => groups.push((t, vec![r])),
        }
    }
    groups.retain(|(t, rs)| p.can_join_group(*t, rs));
    groups
}

/// Whether Join does anything to the selection.
pub(crate) fn can_join(p: &Project, v: &TracksView) -> bool {
    !join_groups(p, v).is_empty()
}

/// Join: merges each group of selected regions into one region on its track. The joined region
/// is selected.
pub(crate) fn join_selected(p: &mut Project, v: &mut TracksView) {
    let mut made = Vec::new();
    for (t, rs) in join_groups(p, v) {
        if let Some(at) = p.join_group(t, &rs) {
            made.push((t, at));
        }
    }
    if !made.is_empty() {
        v.select_many(&made);
    }
}

/// Functions > Move to Playhead: the earliest selected region starts at the playhead, the others
/// keep their distances from it (each stays on its track).
pub(crate) fn move_to_playhead(p: &mut Project, v: &TracksView) {
    let sel = v.selection();
    let base = sel
        .iter()
        .filter_map(|&(t, r)| p.tracks.get(t)?.regions.get(r))
        .map(|reg| reg.start_bar)
        .fold(f32::MAX, f32::min);
    if sel.is_empty() || base == f32::MAX {
        return;
    }
    let playhead_bar = p.beats_to_bar(p.position_beats) as f32 + 1.0;
    for (t, r) in sel {
        if let Some(reg) = p.tracks.get_mut(t).and_then(|tr| tr.regions.get_mut(r)) {
            reg.start_bar = playhead_bar + (reg.start_bar - base);
        }
    }
    p.song_rev += 1;
}

/// Grid used by Functions > Quantize when the region's Quantize setting is Off: 1/16 note.
pub const DEFAULT_QUANTIZE_GRID: f32 = 0.25;

/// Whether Quantize does anything to the selection: a selected region with notes of its own.
pub(crate) fn can_quantize(p: &Project, v: &TracksView) -> bool {
    v.selection().into_iter().any(|(t, r)| {
        p.tracks
            .get(t)
            .and_then(|tr| tr.regions.get(r))
            .is_some_and(|reg| plain_midi(reg) && !reg.notes.is_empty())
    })
}

/// Functions > Quantize: moves the notes of every selected MIDI region onto the region's
/// Quantize grid (1/16 when it is Off). Destructive, like Logic's Functions > Quantize.
pub(crate) fn quantize_selected(p: &mut Project, v: &TracksView) {
    let mut changed = false;
    for (t, r) in v.selection() {
        let Some(reg) = p.tracks.get_mut(t).and_then(|tr| tr.regions.get_mut(r)) else {
            continue;
        };
        if !plain_midi(reg) || reg.notes.is_empty() {
            continue;
        }
        let grid = reg
            .params
            .quantize
            .filter(|g| *g > 0.0)
            .unwrap_or(DEFAULT_QUANTIZE_GRID);
        let all: Vec<usize> = (0..reg.notes.len()).collect();
        crate::note_edit::quantize(&mut reg.notes, &all, grid, 1.0, 0.0);
        changed = true;
    }
    if changed {
        p.song_rev += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioRegion, Note};

    fn test_audio() -> AudioRegion {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../prowave-engine/tests/fixtures");
        AudioRegion {
            file: prowave_engine::AudioFile::open(dir.join("tone.caf")).unwrap(),
            peaks: Default::default(),
            flex: None,
        }
    }

    fn note(start: f32, len: f32) -> Note {
        Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start,
            len,
            pitch: 60,
            vel: 90,
        }
    }

    /// The demo project with one region on track 0: 4 bars with notes at beats 0, 3 and 8.
    fn rig() -> (Project, TracksView) {
        let mut p = Project::demo();
        let region = &mut p.tracks[0].regions[0];
        region.start_bar = 1.0;
        region.len_bars = 4.0;
        region.notes = vec![note(0.0, 1.0), note(3.0, 3.0), note(8.0, 1.0)];
        region.controls.clear();
        p.tracks[0].regions.truncate(1);
        let mut v = TracksView::default();
        v.select_only(0, 0);
        p.selected_track = 0;
        (p, v)
    }

    /// Adds a second region on track 0 at `bar` with the same length and one note.
    fn second_region(p: &mut Project, bar: f32) {
        let mut r = p.tracks[0].regions[0].clone();
        r.start_bar = bar;
        r.len_bars = 2.0;
        r.notes = vec![note(1.0, 1.0)];
        p.tracks[0].regions.push(r);
    }

    #[test]
    fn split_cuts_the_region_and_shortens_the_note_that_crosses() {
        let (mut p, mut v) = rig();
        p.position_beats = 4.0; // bar 2 in 4/4
        assert!(can_split(&p, &v));
        split_at_playhead(&mut p, &mut v);
        let regions = &p.tracks[0].regions;
        assert_eq!(regions.len(), 2);
        let (left, right) = (&regions[0], &regions[1]);
        assert_eq!((left.start_bar, left.len_bars), (1.0, 1.0));
        assert_eq!((right.start_bar, right.len_bars), (2.0, 3.0));
        // Notes at 0 and 3 stay left; the one at 3 (length 3) ends at the cut.
        assert_eq!(left.notes.len(), 2);
        assert!((left.notes[1].len - 1.0).abs() < 1e-6);
        // The note at 8 moves right and starts 4 beats into the right piece.
        assert_eq!(right.notes.len(), 1);
        assert!((right.notes[0].start - 4.0).abs() < 1e-6);
        assert_eq!(v.selected, Some((0, 1)));
    }

    #[test]
    fn split_needs_the_playhead_inside_a_selected_region() {
        let (mut p, v) = rig();
        for at in [0.0, 16.0, 40.0] {
            p.position_beats = at;
            assert!(!can_split(&p, &v), "at {at}");
        }
    }

    #[test]
    fn split_cuts_every_selected_region_under_the_playhead() {
        let (mut p, mut v) = rig();
        second_region(&mut p, 1.0); // also under bar 1
        v.select_many(&[(0, 0), (0, 1)]);
        p.position_beats = 2.0;
        split_at_playhead(&mut p, &mut v);
        assert_eq!(p.tracks[0].regions.len(), 4, "both regions were cut");
        assert_eq!(v.selection().len(), 2, "the two right pieces are selected");
    }

    #[test]
    fn audio_regions_are_split_but_reversed_audio_is_not() {
        let (mut p, mut v) = rig();
        p.position_beats = 4.0;
        p.tracks[0].regions[0].audio = Some(test_audio());
        assert!(can_split(&p, &v));
        p.tracks[0].regions[0].params.reverse = true;
        assert!(!can_split(&p, &v));
        p.tracks[0].regions[0].params.reverse = false;
        split_at_playhead(&mut p, &mut v);
        let right = &p.tracks[0].regions[1];
        assert!(
            right.params.offset_secs > 1.9,
            "the right half plays on from the cut (2 s at 120 bpm): {}",
            right.params.offset_secs
        );
    }

    #[test]
    fn copy_paste_puts_the_regions_at_the_playhead_keeping_their_spacing() {
        let (mut p, mut v) = rig();
        second_region(&mut p, 3.0); // two bars after the first
        v.select_many(&[(0, 0), (0, 1)]);
        let mut clip = Vec::new();
        assert!(!can_paste(&p, &clip));
        assert!(copy_selected(&p, &v, &mut clip));
        assert_eq!(clip.len(), 2);
        p.position_beats = 16.0; // bar 5
        paste_at_playhead(&mut p, &mut v, &clip);
        let regions = &p.tracks[0].regions;
        assert_eq!(regions.len(), 4);
        assert_eq!(regions[2].start_bar, 5.0);
        assert_eq!(regions[3].start_bar, 7.0, "the same two bars apart");
        assert_eq!(v.selection().len(), 2);
    }

    #[test]
    fn cut_removes_the_regions_and_paste_brings_them_back() {
        let (mut p, mut v) = rig();
        let mut clip = Vec::new();
        cut_selected(&mut p, &mut v, &mut clip);
        assert!(p.tracks[0].regions.is_empty() && v.selected.is_none());
        paste_at_playhead(&mut p, &mut v, &clip);
        assert_eq!(p.tracks[0].regions.len(), 1);
    }

    #[test]
    fn an_audio_region_does_not_paste_onto_a_midi_track() {
        let (p, _) = rig();
        let mut audio = p.tracks[0].regions[0].clone();
        audio.audio = Some(test_audio());
        assert!(!can_paste(&p, &vec![audio]));
    }

    #[test]
    fn join_merges_two_regions_and_keeps_each_notes_place() {
        let (mut p, mut v) = rig();
        second_region(&mut p, 5.0); // starts at bar 5, after the first (bars 1-4)
        v.select_many(&[(0, 0), (0, 1)]);
        assert!(can_join(&p, &v));
        join_selected(&mut p, &mut v);
        let regions = &p.tracks[0].regions;
        assert_eq!(regions.len(), 1, "one region is left");
        let joined = &regions[0];
        assert_eq!(
            (joined.start_bar, joined.len_bars),
            (1.0, 6.0),
            "bars 1 to 6"
        );
        // The first region's notes at 0, 3 and 8; the second's note at 1, moved on by the 16 beats
        // before bar 5, to 17.
        let mut starts: Vec<f32> = joined.notes.iter().map(|n| n.start).collect();
        starts.sort_by(f32::total_cmp);
        assert_eq!(starts, vec![0.0, 3.0, 8.0, 17.0]);
        assert_eq!(v.selected, Some((0, 0)));
    }

    #[test]
    fn join_needs_two_plain_midi_regions_on_one_track() {
        let (mut p, mut v) = rig();
        assert!(!can_join(&p, &v), "one region");
        second_region(&mut p, 5.0);
        p.tracks[0].regions[1].generated = None;
        p.tracks[0].regions[1].audio = Some(test_audio());
        v.select_many(&[(0, 0), (0, 1)]);
        assert!(!can_join(&p, &v), "an audio region is in it");
    }
    #[test]
    fn move_to_playhead_keeps_the_distances_between_the_regions() {
        let (mut p, mut v) = rig();
        second_region(&mut p, 3.0); // two bars after the first
        v.select_many(&[(0, 0), (0, 1)]);
        p.position_beats = 32.0; // bar 9
        move_to_playhead(&mut p, &v);
        let starts: Vec<f32> = p.tracks[0].regions.iter().map(|r| r.start_bar).collect();
        assert_eq!(starts, vec![9.0, 11.0]);
    }

    #[test]
    fn move_to_playhead_does_nothing_without_a_selection() {
        let (mut p, mut v) = rig();
        v.clear_selection();
        p.position_beats = 32.0;
        move_to_playhead(&mut p, &v);
        assert_eq!(p.tracks[0].regions[0].start_bar, 1.0);
    }

    #[test]
    fn quantize_snaps_the_notes_of_selected_regions_to_the_grid() {
        let (mut p, mut v) = rig();
        // Notes at 0, 3 and 8 are on the 1/16 grid already; move one off it.
        p.tracks[0].regions[0].notes[1].start = 3.1;
        v.select_only(0, 0);
        assert!(can_quantize(&p, &v));
        quantize_selected(&mut p, &v);
        assert!((p.tracks[0].regions[0].notes[1].start - 3.0).abs() < 1e-5);
    }

    #[test]
    fn quantize_uses_the_regions_grid_and_skips_generated_regions() {
        let (mut p, mut v) = rig();
        p.tracks[0].regions[0].notes = vec![note(0.3, 1.0)];
        p.tracks[0].regions[0].params.quantize = Some(1.0); // whole beats
        v.select_only(0, 0);
        quantize_selected(&mut p, &v);
        assert_eq!(p.tracks[0].regions[0].notes[0].start, 0.0);
        // A generated region's notes are made from its settings, so they are left alone.
        p.tracks[0].regions[0].generated = Some(crate::model::Generated::new(
            crate::model::Generator::Drummer(crate::pattern::DrummerParams::new(
                crate::pattern::DrumStyle::Rock,
            )),
        ));
        assert!(!can_quantize(&p, &v));
    }

    // ---- Option-drag copies (Logic: Option held when the region is let go)

    use crate::ui_test::Harness;
    use egui::{vec2, Modifiers, PointerButton, Pos2, Rect};

    /// Two instrument tracks; track 0 has a two-bar region at bar 2 with notes, a controller
    /// and its own settings, and track 1 a region far right (only there to find the row).
    fn drag_rig() -> (Project, crate::UiState) {
        let mut p = Project::empty();
        p.add_track(TrackKind::Instrument);
        let mut r = Region {
            name: "Riff".into(),
            start_bar: 2.0,
            len_bars: 2.0,
            notes: vec![note(0.0, 1.0), note(2.5, 0.5)],
            controls: vec![crate::model::Control {
                start: 1.0,
                num: 64,
                val: 127,
            }],
            folder: None,
            audio: None,
            params: crate::model::RegionParams {
                transpose: 3,
                velocity: -5,
                quantize: Some(0.25),
                ..Default::default()
            },
            generated: None,
        };
        p.tracks[0].regions.push(r.clone());
        r.start_bar = 9.0;
        r.name = "Far".into();
        p.tracks[1].regions.push(r);
        p.selected_track = 0;
        (p, crate::UiState::new())
    }

    fn show(h: &mut Harness, p: &mut Project, s: &mut crate::UiState) {
        let meters = crate::mixer::Meters::default();
        h.settle(|ui| crate::tracks_area::show(ui, p, s, &meters));
    }

    /// Where region `r` of track `t` is on screen (the last frame).
    fn rect_of(h: &Harness, t: usize, r: usize) -> Rect {
        h.ctx
            .read_response(egui::Id::unique(("tracks_area", ("region", t, r))))
            .expect("the region is on screen")
            .rect
    }

    /// Drags from `from` by `by`, with `mods` held at each step (the last ones when let go).
    /// The drag begins with a small step (a drag starts only once the pointer has moved a
    /// little, and the region is held where it was then); `by` is counted from there.
    fn drag_with(
        h: &mut Harness,
        p: &mut Project,
        s: &mut crate::UiState,
        from: Pos2,
        by: egui::Vec2,
        mods: &[Modifiers],
    ) {
        let meters = crate::mixer::Meters::default();
        let mut ui = |ui: &mut egui::Ui| crate::tracks_area::show(ui, p, s, &meters);
        h.modifiers = mods[0];
        h.press(from, PointerButton::Primary, &mut ui);
        let from = from + vec2(10.0, 0.0);
        h.move_to(from, &mut ui);
        for (i, m) in mods.iter().enumerate() {
            h.modifiers = *m;
            let k = (i + 1) as f32 / mods.len() as f32;
            h.move_to(from + by * k, &mut ui);
        }
        h.release(from + by, PointerButton::Primary, &mut ui);
        h.frame(vec![], &mut ui);
        h.modifiers = Modifiers::NONE;
    }

    /// Everything about two regions but where they are.
    fn same_content(a: &Region, b: &Region) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.len_bars, b.len_bars);
        assert_eq!(a.notes, b.notes);
        assert_eq!(a.controls, b.controls);
        assert!(a.params == b.params);
        assert_eq!(a.generated, b.generated);
        assert_eq!(
            a.audio.as_ref().map(|x| &x.file.path),
            b.audio.as_ref().map(|x| &x.file.path)
        );
    }

    /// Points of one bar in the tracks area (a bar is 96 points wide at the start).
    const BAR: f32 = 96.0;

    #[test]
    fn option_drag_leaves_the_region_and_drops_an_identical_copy_where_it_ends() {
        let (mut p, mut s) = drag_rig();
        let mut h = Harness::new(1400.0, 600.0);
        show(&mut h, &mut p, &mut s);
        let original = p.tracks[0].regions[0].clone();
        let grab = rect_of(&h, 0, 0).left_center() + vec2(20.0, 0.0);
        let meters = crate::mixer::Meters::default();
        h.modifiers = Modifiers::ALT;
        h.press(grab, PointerButton::Primary, |ui| {
            crate::tracks_area::show(ui, &mut p, &mut s, &meters)
        });
        let grab = grab + vec2(10.0, 0.0);
        h.move_to(grab, |ui| {
            crate::tracks_area::show(ui, &mut p, &mut s, &meters)
        });
        h.move_to(grab + vec2(BAR, 0.0), |ui| {
            crate::tracks_area::show(ui, &mut p, &mut s, &meters)
        });
        h.move_to(grab + vec2(2.0 * BAR, 0.0), |ui| {
            crate::tracks_area::show(ui, &mut p, &mut s, &meters)
        });
        // On the way the region stays where it was (its copy follows the pointer).
        assert_eq!(
            p.tracks[0].regions.len(),
            1,
            "nothing lands before the button is let go"
        );
        assert_eq!(
            p.tracks[0].regions[0].start_bar, 2.0,
            "the original stays in place"
        );
        h.release(grab + vec2(2.0 * BAR, 0.0), PointerButton::Primary, |ui| {
            crate::tracks_area::show(ui, &mut p, &mut s, &meters)
        });
        h.modifiers = Modifiers::NONE;
        let regions = &p.tracks[0].regions;
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].start_bar, 2.0, "the original is unchanged");
        same_content(&regions[0], &original);
        assert_eq!(regions[1].start_bar, 4.0, "the copy is two bars later");
        same_content(&regions[1], &original);
        assert_eq!(s.tracks.selection(), vec![(0, 1)], "the copy is selected");
    }

    #[test]
    fn a_plain_drag_still_moves_and_option_counts_only_when_the_button_is_let_go() {
        let (mut p, mut s) = drag_rig();
        let mut h = Harness::new(1400.0, 600.0);
        show(&mut h, &mut p, &mut s);
        let grab = rect_of(&h, 0, 0).left_center() + vec2(20.0, 0.0);
        let (none, alt) = (Modifiers::NONE, Modifiers::ALT);
        drag_with(
            &mut h,
            &mut p,
            &mut s,
            grab,
            vec2(BAR, 0.0),
            &[none, none, none],
        );
        assert_eq!(p.tracks[0].regions.len(), 1);
        assert_eq!(p.tracks[0].regions[0].start_bar, 3.0, "moved a bar");

        // Option pressed on the way and let go before the drop: a move.
        show(&mut h, &mut p, &mut s);
        let grab = rect_of(&h, 0, 0).left_center() + vec2(20.0, 0.0);
        drag_with(
            &mut h,
            &mut p,
            &mut s,
            grab,
            vec2(BAR, 0.0),
            &[none, alt, none],
        );
        assert_eq!(p.tracks[0].regions.len(), 1);
        assert_eq!(p.tracks[0].regions[0].start_bar, 4.0);

        // Option pressed only at the end: a copy.
        show(&mut h, &mut p, &mut s);
        let grab = rect_of(&h, 0, 0).left_center() + vec2(20.0, 0.0);
        drag_with(
            &mut h,
            &mut p,
            &mut s,
            grab,
            vec2(-BAR, 0.0),
            &[none, none, alt],
        );
        let starts: Vec<f32> = p.tracks[0].regions.iter().map(|r| r.start_bar).collect();
        assert_eq!(starts, vec![4.0, 3.0]);
    }

    #[test]
    fn option_drag_copies_a_whole_selection_keeping_its_spacing() {
        let (mut p, mut s) = drag_rig();
        let mut extra = p.tracks[0].regions[0].clone();
        extra.start_bar = 5.0;
        extra.name = "Second".into();
        p.tracks[0].regions.push(extra);
        let mut h = Harness::new(1400.0, 600.0);
        show(&mut h, &mut p, &mut s);
        s.tracks.select_many(&[(0, 0), (0, 1)]);
        let grab = rect_of(&h, 0, 0).left_center() + vec2(20.0, 0.0);
        let alt = Modifiers::ALT;
        drag_with(&mut h, &mut p, &mut s, grab, vec2(BAR, 0.0), &[alt, alt]);
        let regions = &p.tracks[0].regions;
        let at: Vec<(&str, f32)> = regions
            .iter()
            .map(|r| (r.name.as_str(), r.start_bar))
            .collect();
        assert_eq!(
            at,
            vec![
                ("Riff", 2.0),
                ("Second", 5.0),
                ("Riff", 3.0),
                ("Second", 6.0)
            ]
        );
        same_content(&regions[2], &regions[0]);
        same_content(&regions[3], &regions[1]);
        assert_eq!(s.tracks.selection(), vec![(0, 2), (0, 3)]);
    }

    #[test]
    fn option_drag_copies_to_another_track_and_a_plain_drag_moves_there() {
        let (mut p, mut s) = drag_rig();
        let mut h = Harness::new(1400.0, 600.0);
        show(&mut h, &mut p, &mut s);
        let grab = rect_of(&h, 0, 0).left_center() + vec2(20.0, 0.0);
        let down = rect_of(&h, 1, 0).center().y - rect_of(&h, 0, 0).center().y;
        assert!(down > 10.0);
        let alt = Modifiers::ALT;
        drag_with(&mut h, &mut p, &mut s, grab, vec2(BAR, down), &[alt, alt]);
        assert_eq!(
            p.tracks[0].regions.len(),
            1,
            "the original stays on its track"
        );
        assert_eq!(p.tracks[0].regions[0].start_bar, 2.0);
        let copy = &p.tracks[1].regions[1];
        assert_eq!(copy.start_bar, 3.0);
        same_content(copy, &p.tracks[0].regions[0]);
        assert_eq!(s.tracks.selection(), vec![(1, 1)]);
        assert_eq!(p.selected_track, 1);

        // Without Option the region goes to the other track.
        show(&mut h, &mut p, &mut s);
        let grab = rect_of(&h, 0, 0).left_center() + vec2(20.0, 0.0);
        let none = Modifiers::NONE;
        drag_with(&mut h, &mut p, &mut s, grab, vec2(0.0, down), &[none, none]);
        assert!(p.tracks[0].regions.is_empty());
        let names: Vec<&str> = p.tracks[1]
            .regions
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, vec!["Far", "Riff", "Riff"]);
    }

    #[test]
    fn an_audio_region_is_not_copied_onto_a_midi_track() {
        let (mut p, _) = drag_rig();
        p.tracks[0].kind = TrackKind::Audio;
        p.tracks[0].regions[0].audio = Some(test_audio());
        let auto = crate::automation::AutoView::default();
        let landed = drop_regions(&mut p, &auto, &[(0, 0, 4.0)], 4.0, 1, true, false);
        assert_eq!(landed, vec![(0, 1)], "copied on its own track instead");
        assert_eq!(p.tracks[1].regions.len(), 1);
    }

    #[test]
    fn a_copied_audio_region_keeps_its_file_offset_gain_and_fades() {
        let mut p = Project::empty();
        p.tracks[0].kind = TrackKind::Audio;
        p.add_track(TrackKind::Audio);
        p.tracks[0].regions.push(Region {
            name: "Take".into(),
            start_bar: 1.0,
            len_bars: 1.0,
            notes: Vec::new(),
            controls: Vec::new(),
            folder: None,
            audio: Some(test_audio()),
            params: crate::model::RegionParams {
                gain_db: -4.5,
                fade_in: 0.5,
                fade_out: 0.75,
                fade_curve: prowave_engine::FadeCurve::SCurve,
                offset_secs: 0.25,
                reverse: true,
                ..Default::default()
            },
            generated: None,
        });
        let auto = crate::automation::AutoView::default();
        let landed = drop_regions(&mut p, &auto, &[(0, 0, 0.0)], 8.0, 1, true, false);
        assert_eq!(landed, vec![(1, 0)]);
        let (a, b) = (&p.tracks[0].regions[0], &p.tracks[1].regions[0]);
        same_content(a, b);
        assert_eq!((a.start_bar, b.start_bar), (1.0, 3.0));
    }

    #[test]
    fn undo_takes_the_copy_away_in_one_step() {
        let (mut p, mut s) = drag_rig();
        let mut history = crate::undo::History::new(&p);
        let mut h = Harness::new(1400.0, 600.0);
        show(&mut h, &mut p, &mut s);
        let grab = rect_of(&h, 0, 0).left_center() + vec2(20.0, 0.0);
        let alt = Modifiers::ALT;
        drag_with(
            &mut h,
            &mut p,
            &mut s,
            grab,
            vec2(2.0 * BAR, 0.0),
            &[alt, alt, alt],
        );
        assert_eq!(p.tracks[0].regions.len(), 2);
        assert!(history.record(&p));
        assert!(history.undo(&mut p));
        assert_eq!(p.tracks[0].regions.len(), 1);
        assert_eq!(p.tracks[0].regions[0].start_bar, 2.0);
        assert!(!history.can_undo(), "one step");
    }
}
