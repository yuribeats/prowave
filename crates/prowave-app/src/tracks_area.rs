//! Tracks area (Logic Pro: main window "Tracks" area): local toolbar, ruler with cycle area and
//! playhead, track headers and the region lanes.
//!
//! Works today: select/move regions with Snap (Shift-click adds to the selection, a drag over
//! the background is a rubber band, a drag of a selected region moves the whole selection),
//! Delete and Repeat regions, Set Locators by Regions, drag in the ruler to set the cycle,
//! click to move the playhead, scroll, zoom
//! (sliders and Ctrl+wheel), Catch playhead, add/duplicate tracks, M/S/R/I, volume, pan.
//! Menu entries that are not implemented yet are shown disabled.
use crate::hw::{self, Cap};
use crate::i18n::{tr, trf};
use crate::model::HeaderPart;
use crate::model::{AutoTarget, Project, Region, Track, TrackKind};
use crate::theme::{self, Face};
use crate::track_icons::{draw_icon, track_icon};
use crate::widgets::{context_click, context_menu, menu_title, popup_button};
use crate::{Bottom, UiState};
use egui::{
    pos2, vec2, Align2, Color32, Key, KeyboardShortcut, Modifiers, Painter, Pos2, Rect, Sense,
    Shape, Stroke, StrokeKind, Ui, Vec2,
};
use hw_tracks::{Glyph, Material};

/// The Hardware parts of the Tracks area (header keys, slider, knob, regions, ruler).
#[path = "hw_tracks.rs"]
pub(crate) mod hw_tracks;

/// The menu row over the tracks (Edit, Functions, View, the tool keys, Snap, Drag, zoom).
pub(crate) const TOOLBAR_H: f32 = 48.0;
/// The ruler: the cycle band on top, bar numbers and ticks under it.
pub(crate) const RULER_H: f32 = 38.0;
/// A track row's height before the track zoom changes it.
pub(crate) const DEFAULT_ROW_H: f32 = 70.0;
/// The track list.
pub(crate) const HEADER_W: f32 = 440.0;
/// Left margin of a header's contents (the number key starts here).
pub(crate) const NUM_W: f32 = 16.0;
/// How far each folder nesting level moves a track header's contents to the right.
const INDENT_W: f32 = 14.0;
/// Height of the ruler's cycle zone (the band and the strip a drag sets it in).
const CYCLE_ZONE_H: f32 = 16.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    Smart,
    Bar,
    Beat,
    Division,
    Ticks,
}

impl Snap {
    pub(crate) const ALL: [Snap; 5] = [
        Snap::Smart,
        Snap::Bar,
        Snap::Beat,
        Snap::Division,
        Snap::Ticks,
    ];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Snap::Smart => tr("Smart"),
            Snap::Bar => tr("Bar"),
            Snap::Beat => tr("Beat"),
            Snap::Division => tr("Division"),
            Snap::Ticks => tr("Ticks"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DragMode {
    Overlap,
    /// The earlier of two overlapping regions is shortened (`drag_modes.rs`).
    NoOverlap,
    XFade,
    /// Regions close up to the right (`drag_modes.rs`).
    ShuffleR,
    /// Regions close up to the left (`drag_modes.rs`).
    ShuffleL,
}

impl DragMode {
    /// In the order of Logic's Drag menu.
    const ALL: [DragMode; 5] = [
        DragMode::Overlap,
        DragMode::NoOverlap,
        DragMode::XFade,
        DragMode::ShuffleR,
        DragMode::ShuffleL,
    ];
    fn label(self) -> &'static str {
        match self {
            DragMode::Overlap => tr("Overlap"),
            DragMode::NoOverlap => tr("No Overlap"),
            DragMode::XFade => tr("X-Fade"),
            DragMode::ShuffleR => tr("Shuffle R"),
            DragMode::ShuffleL => tr("Shuffle L"),
        }
    }
}

/// What a click with one of the editing tools does, once the regions are drawn.
#[derive(Clone)]
enum ToolAct {
    Erase(usize, usize),
    /// The Scissors tool: (track, region, where, Option held: equal pieces).
    Split(usize, usize, f64, bool),
    /// The Mute tool on a region.
    Mute(usize, usize),
    /// A region's start dragged to a new place (quarter notes).
    Start(usize, usize, f64),
    Join(usize, usize),
    /// A region's end dragged to a new place (quarter notes); `true`: stretch to fit.
    End(usize, usize, f64, bool),
    /// Regions let go after a drag: (track, region, start before), how far later and how many
    /// tracks down they land, and whether Option was held (copies land there instead).
    Drop(Vec<(usize, usize, f64)>, f64, isize, bool),
}

/// A Normalize Region Gain scan running on its own thread: reading a long recording to find
/// its loudest sample takes seconds, which would freeze the window if done on the UI thread.
pub struct NormalizeJob {
    track: usize,
    region: usize,
    path: std::path::PathBuf,
    /// The file's peak, or why it could not be read.
    result: std::sync::mpsc::Receiver<Result<f32, String>>,
}

/// A tool of the Tracks area (Logic Pro: Tracks area tools). Only the tools that work here
/// are offered; Logic's Flex and automation tools are not among them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrackTool {
    Pointer,
    /// Click on an empty stretch of an instrument track: an empty MIDI region of one bar there
    /// (at the snap). On a region it works as the Pointer does.
    Pencil,
    /// Click a region to delete it.
    Eraser,
    /// Click a region to rename it.
    Text,
    /// Click a region to cut it there (at the snap).
    Scissors,
    /// Click a region to join it with the next one on its track.
    Join,
    /// Click a region to mute or unmute it.
    Mute,
    /// Click to zoom in, Option-click to zoom out.
    Zoom,
    /// Drag on an audio region to set its fade in (left half) or fade out (right half).
    Fade,
    /// Drag an audio region (or the selected ones) up or down to change its gain.
    Gain,
    /// Drag to select a stretch of time on one or more tracks (`marquee.rs`).
    Marquee,
    /// Hold the button down on a region: it plays from there, alone (`region_solo.rs`).
    Solo,
}

impl TrackTool {
    /// Logic's Tool menu for the Tracks area, as far as it is built, with Logic's keys (T in
    /// the open menu is the Pointer, so T twice gives the Pointer; Shift-T the Text tool).
    pub(crate) const ALL: [(TrackTool, Key, bool, &'static str); 12] = [
        (TrackTool::Pointer, Key::T, false, "T"),
        (TrackTool::Pencil, Key::P, false, "P"),
        (TrackTool::Eraser, Key::E, false, "E"),
        (TrackTool::Marquee, Key::R, false, "R"),
        (TrackTool::Text, Key::T, true, "⇧T"),
        (TrackTool::Scissors, Key::I, false, "I"),
        (TrackTool::Join, Key::J, false, "J"),
        (TrackTool::Mute, Key::M, false, "M"),
        (TrackTool::Solo, Key::S, false, "S"),
        (TrackTool::Zoom, Key::Y, false, "Y"),
        (TrackTool::Fade, Key::A, false, "A"),
        (TrackTool::Gain, Key::G, false, "G"),
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            TrackTool::Pointer => tr("Pointer Tool"),
            TrackTool::Pencil => tr("Pencil Tool"),
            TrackTool::Eraser => tr("Eraser Tool"),
            TrackTool::Text => tr("Text Tool"),
            TrackTool::Scissors => tr("Scissors Tool"),
            TrackTool::Join => tr("Join Tool"),
            TrackTool::Mute => tr("Mute Tool"),
            TrackTool::Zoom => tr("Zoom Tool"),
            TrackTool::Fade => tr("Fade Tool"),
            TrackTool::Gain => tr("Gain Tool"),
            TrackTool::Marquee => tr("Marquee Tool"),
            TrackTool::Solo => tr("Solo Tool"),
        }
    }

    fn glyph(self) -> Glyph {
        match self {
            TrackTool::Pointer => Glyph::Pointer,
            TrackTool::Pencil => Glyph::Pencil,
            TrackTool::Eraser => Glyph::Eraser,
            TrackTool::Text => Glyph::Text,
            TrackTool::Scissors => Glyph::Scissors,
            TrackTool::Join => Glyph::Join,
            TrackTool::Mute => Glyph::Mute,
            TrackTool::Zoom => Glyph::Zoom,
            TrackTool::Fade => Glyph::Fade,
            TrackTool::Gain => Glyph::Gain,
            TrackTool::Marquee => Glyph::Marquee,
            TrackTool::Solo => Glyph::Solo,
        }
    }

    /// Whether a press on a region selects and drags it as the Pointer does.
    fn moves_regions(self) -> bool {
        matches!(self, TrackTool::Pointer | TrackTool::Pencil)
    }
}

impl crate::piano_tools::MenuTool for TrackTool {
    fn entries() -> &'static [(Self, Key, bool, &'static str)] {
        &Self::ALL
    }
    fn label(self) -> &'static str {
        self.name()
    }
    fn paint(self, g: &Painter, c: Pos2, col: Color32) {
        hw_tracks::glyph(g, c, 16.0, self.glyph(), col);
    }
}

pub struct TracksView {
    /// Track > Show Hidden Tracks (H, Logic's Hide view): hidden tracks are shown, dimmed,
    /// and every track header has an H key. Off, a hidden track takes no room and nothing
    /// that selects or edits by position reaches it (`track_commands::takes_no_room`).
    pub show_hidden: bool,
    /// Width of one bar in points (horizontal zoom).
    pub bar_w: f32,
    /// Track height in points (vertical zoom).
    pub row_h: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub snap: Snap,
    pub drag_mode: DragMode,
    /// The left-click tool.
    pub tool: TrackTool,
    /// The Command-click tool: the one the pointer has while Command is held (Logic).
    pub cmd_tool: TrackTool,
    /// The Tool menu (T) open at this place on screen.
    tool_menu: Option<Pos2>,
    /// The tool a press in the lanes began with, kept until the button is let go (so that
    /// pressing or letting go of Command during a drag does not change what the drag does).
    held_tool: Option<TrackTool>,
    /// A Gain tool drag: each region's (track, region, gain in dB) when it began, and the
    /// pointer's height then.
    gain_drag: Option<GainDrag>,
    /// A Fade tool drag in progress (`fade_tool.rs`).
    fade_drag: Option<crate::fade_tool::Drag>,
    /// A region's end being dragged: (track, region, new end in quarter notes, Option held:
    /// stretch the audio to fit instead of shortening or lengthening the region).
    edge_drag: Option<(usize, usize, f64, bool)>,
    /// A region's start being dragged: (track, region, new start in quarter notes).
    start_drag: Option<(usize, usize, f64)>,
    /// The Marquee tool's selection of a stretch of time (`marquee.rs`).
    pub range: crate::marquee::State,
    /// The Solo tool held down on a region: whether the transport was playing before.
    solo_hold: Option<bool>,
    /// Where the pointer was when the drag of regions in progress began (Shift then keeps the
    /// drag to the direction it has gone furthest in).
    drag_origin: Option<Pos2>,
    pub catch: bool,
    /// Global Tempo track above the tracks (Track > Show Tempo Track).
    pub show_tempo: bool,
    /// View > Colour Spectrogram: audio regions show their sound's colour spectrogram
    /// instead of the waveform (`colour_spectrogram`); kept for the next start.
    pub colour_spectra: bool,
    /// Global Marker track above the tracks (Track > Show Marker Track).
    pub show_markers: bool,
    /// Global Signature track above the tracks (Track > Show Signature Track).
    pub show_signatures: bool,
    /// The selected region (track, region): the one the Inspector shows, and the one a drag moves
    /// first. Every other selected region is in `extra`.
    pub selected: Option<(usize, usize)>,
    /// Selected regions besides `selected` (Select All, Shift-click). Never holds `selected`.
    pub extra: Vec<(usize, usize)>,
    /// The marker selected on the Marker track (a click selects it; Delete removes it).
    /// Selecting regions, or nothing, deselects it.
    pub selected_marker: Option<usize>,
    /// Width of the lanes at the last frame (for Navigate > Zoom to Fit).
    pub lanes_w: f32,
    grab_beats: f64,
    /// A drag of selected regions in progress: each one's (track, region, start in beats) when it
    /// began, so they all move by the same distance.
    group_drag: Vec<(usize, usize, f64)>,
    /// That drag so far: quarter notes later (snapped) and tracks down.
    drag_by: (f64, isize),
    /// A rubber band being dragged over the lanes' background: the corner it began at (quarter
    /// notes, points down from the top of the first track) and what was selected then.
    marquee: Option<Marquee>,
    /// A swipe on a take row in progress: the comp as it was and where the swipe began.
    swipe: Option<(Vec<crate::model::CompSegment>, f32)>,
    /// Normalize Region Gain scans still running.
    pub normalize_jobs: Vec<NormalizeJob>,
    /// For the layout pictures (`--shots`) only: the right-click menu shown open, of a track
    /// (`(t, None)`) or of a region (`(t, Some(r))`).
    pub shot_menu: Option<(usize, Option<usize>)>,
    /// Smart Tempo analyses still running.
    pub tempo_jobs: Vec<crate::smart_tempo::Job>,
    /// The multitrack Smart Tempo analysis, while it runs.
    pub tempo_all: Option<crate::smart_tempo::AllJob>,
    /// The Smart Tempo Editor window.
    pub tempo_editor: crate::smart_tempo::Editor,
    /// What Nudge Left and Nudge Right move by (Edit > Nudge Value).
    pub nudge: crate::region_commands::NudgeValue,
    /// Zoom keys, Z's way back, Scroll in Play and the waveform zoom (`arrange_zoom`).
    pub zoom: crate::arrange_zoom::ZoomState,
    /// Snap to Grid, the division and Relative or Absolute (`snap_grid`).
    pub grid: crate::snap_grid::Grid,
    /// The selected tracks besides `Project::selected_track` (`track_selection.rs`).
    pub track_sel: crate::track_selection::TrackSel,
}

/// Each region's (track, region, gain in dB) when a Gain tool drag began, and the pointer's
/// height then.
type GainDrag = (Vec<(usize, usize, f32)>, f32);

/// A rubber-band selection in progress (Logic: drag over the Tracks area's background).
struct Marquee {
    /// Where it began: quarter notes, and points down from the top of the first track.
    anchor: (f64, f32),
    /// The regions selected when it began (Shift held: the band toggles these).
    base: Vec<(usize, usize)>,
}

/// The selection a rubber band gives: the regions it touches, and with `base` (Shift held when
/// it began) those regions switched in `base`: added when they were not selected, taken out when
/// they were, as Logic does.
fn band_selection(base: &[(usize, usize)], touched: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = base
        .iter()
        .copied()
        .filter(|x| !touched.contains(x))
        .collect();
    out.extend(touched.iter().copied().filter(|x| !base.contains(x)));
    out
}

impl TracksView {
    /// Whether a region is held by the Solo tool: the keys that edit regions wait until it is
    /// let go (they would change the region numbers the hold names).
    pub(crate) fn solo_holding(&self) -> bool {
        self.solo_hold.is_some()
    }

    /// Whether the Tool menu (T) is open: its keys (P, I, A, …) are its own then.
    pub(crate) fn tool_menu_open(&self) -> bool {
        self.tool_menu.is_some()
    }

    /// The tool the pointer has now: the Command-click tool while Command is held.
    pub(crate) fn tool_for(&self, command: bool) -> TrackTool {
        if command {
            self.cmd_tool
        } else {
            self.tool
        }
    }

    /// A press or the start of a drag on region `r` of track `t` with the Pointer tool. `add`
    /// (Shift or Command held): the region joins the selection. Without it a region that is
    /// not selected becomes the only one, and one that is selected keeps the whole selection,
    /// so that a drag moves all of it.
    pub fn grab(&mut self, t: usize, r: usize, add: bool) {
        if self.is_selected(t, r) {
            return;
        }
        if add {
            self.toggle(t, r);
        } else {
            self.select_only(t, r);
        }
    }

    /// Whether region `r` of track `t` is selected.
    pub fn is_selected(&self, t: usize, r: usize) -> bool {
        self.selected == Some((t, r)) || self.extra.contains(&(t, r))
    }

    /// Every selected region, in track and region order.
    pub fn selection(&self) -> Vec<(usize, usize)> {
        let mut all: Vec<(usize, usize)> = self
            .selected
            .into_iter()
            .chain(self.extra.iter().copied())
            .collect();
        all.sort_unstable();
        all.dedup();
        all
    }

    /// Selects region `r` of track `t` alone.
    pub fn select_only(&mut self, t: usize, r: usize) {
        self.selected = Some((t, r));
        self.extra.clear();
        self.selected_marker = None;
        // A region selection replaces the marquee (Delete, Cut and Split then act on it).
        self.range.clear();
    }

    /// Selects exactly these regions; the first one is the primary one. Nothing when empty.
    pub fn select_many(&mut self, regions: &[(usize, usize)]) {
        self.clear_selection();
        if !regions.is_empty() {
            self.range.clear();
        }
        for &x in regions {
            if self.selected.is_none() {
                self.selected = Some(x);
            } else if self.selected != Some(x) && !self.extra.contains(&x) {
                self.extra.push(x);
            }
        }
    }

    /// Sets the selection to one region, or to nothing.
    pub fn set_primary(&mut self, one: Option<(usize, usize)>) {
        match one {
            Some((t, r)) => self.select_only(t, r),
            None => self.clear_selection(),
        }
    }

    /// Selects nothing.
    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.extra.clear();
        self.group_drag.clear();
        self.selected_marker = None;
    }

    /// Shift-click: adds region `r` of track `t` to the selection, or takes it out again.
    pub fn toggle(&mut self, t: usize, r: usize) {
        self.selected_marker = None;
        self.range.clear();
        if self.selected == Some((t, r)) {
            // The primary one goes: the most recently added region takes its place.
            self.selected = self.extra.pop();
        } else if let Some(i) = self.extra.iter().position(|x| *x == (t, r)) {
            self.extra.remove(i);
        } else if self.selected.is_none() {
            self.selected = Some((t, r));
        } else {
            self.extra.push((t, r));
        }
    }

    /// A click on a region that is already selected: it becomes the primary one and the rest of
    /// the selection stays.
    pub fn make_primary(&mut self, t: usize, r: usize) {
        if let Some(i) = self.extra.iter().position(|x| *x == (t, r)) {
            if let Some(old) = self.selected {
                self.extra[i] = old;
            } else {
                self.extra.remove(i);
            }
            self.selected = Some((t, r));
        }
    }

    /// Edit > Select All: every region of every track.
    pub fn select_all(&mut self, p: &crate::model::Project) {
        let all: Vec<(usize, usize)> = p
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(t, tr)| (0..tr.regions.len()).map(move |r| (t, r)))
            .collect();
        self.select_many(&all);
    }

    /// Forgets selected regions the project no longer has (after a delete or an undo).
    pub fn prune(&mut self, p: &crate::model::Project) {
        let valid = |(t, r): (usize, usize)| p.tracks.get(t).is_some_and(|tr| r < tr.regions.len());
        self.extra.retain(|x| valid(*x));
        if self.selected.is_some_and(|x| !valid(x)) {
            self.selected = None;
        }
        if self.selected.is_none() {
            self.selected = self.extra.pop();
        }
    }
}

impl Default for TracksView {
    fn default() -> Self {
        Self {
            bar_w: 96.0,
            row_h: DEFAULT_ROW_H,
            scroll_x: 0.0,
            scroll_y: 0.0,
            lanes_w: 1000.0,
            snap: Snap::Smart,
            drag_mode: DragMode::Overlap,
            show_hidden: false,
            tool: TrackTool::Pointer,
            cmd_tool: TrackTool::Marquee,
            nudge: crate::region_commands::NudgeValue::load(),
            tool_menu: None,
            held_tool: None,
            gain_drag: None,
            fade_drag: None,
            edge_drag: None,
            start_drag: None,
            drag_origin: None,
            range: Default::default(),
            solo_hold: None,
            catch: true,
            show_tempo: false,
            colour_spectra: crate::colour_spectrogram::load_on(),
            show_markers: false,
            show_signatures: false,
            selected: None,
            extra: Vec::new(),
            selected_marker: None,
            grab_beats: 0.0,
            group_drag: Vec::new(),
            drag_by: (0.0, 0),
            marquee: None,
            swipe: None,
            normalize_jobs: Vec::new(),
            shot_menu: None,
            tempo_jobs: Vec::new(),
            tempo_all: None,
            tempo_editor: Default::default(),
            zoom: Default::default(),
            grid: Default::default(),
            track_sel: Default::default(),
        }
    }
}

/// True when another track is soloed and `track` is not: it is silent until solo is released
/// (Logic plays only soloed tracks and greys the others' regions).
pub(crate) fn silenced_by_solo(p: &Project, track: usize) -> bool {
    p.tracks.get(track).is_some() && !p.is_soloed(track) && p.any_solo()
}

/// The ruler over the selected region: a band of ink across the bar-number zone.
const RULER_SELECTION: Color32 = Color32::from_rgba_premultiplied(4, 4, 4, 30);

/// The playhead is the signal orange with an ink edge; while recording it is the deeper
/// record red.
#[allow(non_snake_case)]
pub(crate) fn PLAYHEAD_LINE() -> Color32 {
    theme::ORANGE()
}
#[allow(non_snake_case)]
pub(crate) fn PLAYHEAD_HEAD() -> Color32 {
    theme::ORANGE()
}
#[allow(non_snake_case)]
pub(crate) fn PLAYHEAD_EDGE() -> Color32 {
    theme::ON_ACCENT()
}

/// The playhead's line, head and outline colours.
pub(crate) fn playhead_colors(recording: bool) -> (Color32, Color32, Color32) {
    if recording {
        (theme::DANGER(), theme::DANGER(), theme::INK())
    } else {
        (PLAYHEAD_LINE(), PLAYHEAD_HEAD(), PLAYHEAD_EDGE())
    }
}

/// Half the width of the playhead head's grab zone in the ruler.
const PLAYHEAD_GRAB: f32 = 8.0;

/// Beat under screen x `x` while dragging the playhead (never before the start of the song).
fn playhead_beat(x: f32, lanes_left: f32, scroll_x: f32, beat_px: f32) -> f64 {
    ((x - lanes_left + scroll_x) / beat_px.max(1e-3)).max(0.0) as f64
}

/// Half the height of the grab zone on a track's lower edge.
const RESIZE_GRIP: f32 = 3.0;

/// `(top, height)` of every track row in points, from the top of the first track (unscrolled):
/// the global track zoom `row_h` times each track's own height factor.
#[cfg(test)]
fn row_layout(p: &Project, row_h: f32) -> Vec<(f32, f32)> {
    row_layout_with(p, row_h, false, |_| 0.0)
}

/// [`row_layout`] with `extra(track)` added to each row (Automation Mode's choosers and lanes),
/// and hidden tracks given room only when `show_hidden` (Track > Show Hidden Tracks).
fn row_layout_with(
    p: &Project,
    row_h: f32,
    show_hidden: bool,
    extra: impl Fn(&Track) -> f32,
) -> Vec<(f32, f32)> {
    let mut top = 0.0;
    p.tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            // The tracks of a folded folder take no room (every loop below skips them), nor do
            // hidden tracks while hidden tracks are not shown.
            let h = if crate::track_commands::takes_no_room(p, show_hidden, i) {
                0.0
            } else {
                (row_h * t.height).round().max(1.0) + take_rows_height(t) + extra(t)
            };
            let row = (top, h);
            top += h;
            row
        })
        .collect()
}

/// Every track's `(top, height)` as the lanes draw it now (Automation Mode's rows included, and
/// hidden tracks unless `show_hidden`), at track height `row_h`.
pub(crate) fn rows_as_drawn(
    p: &Project,
    auto: &crate::automation::AutoView,
    show_hidden: bool,
    row_h: f32,
) -> Vec<(f32, f32)> {
    row_layout_with(p, row_h, show_hidden, |t| {
        crate::automation_ui::chooser_height(auto, t) + crate::automation_ui::lanes_height(auto, t)
    })
}

/// Index of the track row containing `y` (same origin as [`row_layout`]).
fn row_at(rows: &[(f32, f32)], y: f32) -> Option<usize> {
    rows.iter().position(|&(top, h)| y >= top && y < top + h)
}

fn id(key: impl std::hash::Hash + std::fmt::Debug) -> egui::Id {
    egui::Id::unique(("tracks_area", key))
}

/// Fade tool steps per quarter note (1/16 notes, as the Inspector's fades).
const FADE_STEPS_PER_BEAT: f64 = 4.0;

/// The fade (in quarter notes) the Fade tool sets when dragged to `at`, for a region that
/// starts at `start` and is `len` long: from its start for a fade in, from its end for a fade
/// out; in 1/16 notes and never longer than the region.
pub(crate) fn fade_for_drag(at: f64, start: f64, len: f64, fade_in: bool) -> f32 {
    let raw = if fade_in {
        at - start
    } else {
        start + len - at
    };
    ((raw * FADE_STEPS_PER_BEAT).round().max(0.0) / FADE_STEPS_PER_BEAT).min(len) as f32
}

/// The direction a drag of regions is kept to while Shift is held.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DragAxis {
    /// Along the track: earlier or later.
    Time,
    /// Across the tracks: up or down.
    Tracks,
}

/// The snap a drag of regions uses while Control is held (Logic): divisions (sixteenth
/// notes), or ticks with Shift as well. `None`: the Snap menu's setting.
fn drag_snap(m: Modifiers) -> Option<f64> {
    match (m.ctrl, m.shift) {
        (true, false) => Some(0.25),
        (true, true) => Some(1.0 / 960.0),
        _ => None,
    }
}

/// The direction a drag from `origin` to `now` is kept to: with Shift held (and not Control,
/// which is the finer snap), the one it has gone furthest in.
fn drag_lock(m: Modifiers, origin: Option<Pos2>, now: Pos2) -> Option<DragAxis> {
    let by = now - origin?;
    if !m.shift || m.ctrl {
        return None;
    }
    Some(if by.x.abs() >= by.y.abs() {
        DragAxis::Time
    } else {
        DragAxis::Tracks
    })
}

/// Whether region `ri` of track `ti` overlaps another region of the track.
pub(crate) fn overlaps_another(p: &Project, ti: usize, ri: usize) -> bool {
    let track = &p.tracks[ti];
    let (s, e) = (
        p.region_start_beats(&track.regions[ri]),
        p.region_end_beats(&track.regions[ri]),
    );
    track.regions.iter().enumerate().any(|(j, o)| {
        j != ri && p.region_start_beats(o) < e - 1e-9 && p.region_end_beats(o) > s + 1e-9
    })
}

/// Snap step in beats (quarter notes) for the current setting and zoom. `beat_px` is pixels
/// per quarter note; `beat_unit` is the signature's beat in quarter notes (0.5 in 6/8), so
/// Beat snaps to eighths in x/8 as in Logic. Divisions stay 16th notes.
pub(crate) fn snap_step(snap: Snap, beat_px: f32, beats_per_bar: f64, beat_unit: f64) -> f64 {
    match snap {
        Snap::Smart if beat_px >= 48.0 => 0.25,
        Snap::Smart if beat_px * beat_unit as f32 >= 12.0 => beat_unit,
        Snap::Smart => beats_per_bar,
        Snap::Bar => beats_per_bar,
        Snap::Beat => beat_unit,
        Snap::Division => 0.25,
        Snap::Ticks => 1.0 / 960.0,
    }
}

pub(crate) fn snap_to(b: f64, step: f64) -> f64 {
    (b / step).round() * step
}

// ------------------------------------------------------------------ small widgets

/// A zoom slider: the black groove `rect` with the 12×22 thumb; returns the new 0..1 value.
fn zoom_slider(ui: &Ui, g: &Painter, rect: Rect, key: &str, value: f32) -> f32 {
    let resp = ui.interact(
        rect.expand2(vec2(6.0, 11.0)),
        id(("slider", key)),
        Sense::click_and_drag(),
    );
    let mut v = value;
    if let Some(p) = resp.interact_pointer_pos() {
        v = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    }
    g.rect_filled(rect.translate(vec2(0.0, 1.0)), 2.0, theme::BONE_HI());
    g.rect_filled(rect, 2.0, hw_tracks::GROOVE);
    let thumb = Rect::from_center_size(
        pos2(rect.left() + rect.width() * v, rect.center().y),
        vec2(12.0, 22.0),
    );
    hw_tracks::thumb_h(g, thumb);
    v
}

/// A track stack's H key (Show Hidden Tracks): hides the stack with everything in it.
fn hide_button(ui: &Ui, g: &Painter, p: &mut Project, i: usize, rect: Rect) {
    let on = p.tracks[i].hidden;
    if header_button(ui, g, rect, (i, "h"), "H", on) {
        p.tracks[i].hidden = !on;
    }
}

/// An M/S/R/I key (4.7): a bone key latched with its LED lit while on. All four speak the
/// same language: the LED in its own column at the left, the letter centred beside it.
/// Returns whether it was clicked.
fn header_button(
    ui: &Ui,
    g: &Painter,
    rect: Rect,
    key: (usize, &str),
    label: &str,
    on: bool,
) -> bool {
    let resp = ui.interact(rect, id(("hdrbtn", key)), Sense::click());
    hw_tracks::small_key(
        g,
        rect,
        Cap::Bone,
        on,
        resp.is_pointer_button_down_on(),
        resp.hovered(),
        true,
        label,
        None,
    );
    resp.clicked()
}

/// A tool key in the menu row: a bone key with `glyph`, latched with its LED lit when `on`
/// (the LED only when `led`).
fn glyph_key(
    ui: &Ui,
    g: &Painter,
    rect: Rect,
    key: &str,
    on: bool,
    led: bool,
    glyph: Glyph,
) -> egui::Response {
    let resp = ui.interact(rect, id(("icon", key)), Sense::click());
    let face = hw::paint_key(
        g,
        rect,
        Cap::Bone,
        on || resp.is_pointer_button_down_on(),
        resp.hovered(),
    );
    let mut c = face.center();
    if led {
        hw::led_sized(g, pos2(face.left() + 9.0, face.center().y), 3.0, on);
        c.x += 7.0;
    }
    hw_tracks::glyph(g, c, 18.0, glyph, theme::INK());
    resp
}

fn shortcut(ui: &Ui, mods: Modifiers, key: Key) -> String {
    ui.ctx().format_shortcut(&KeyboardShortcut::new(mods, key))
}

// ------------------------------------------------------------------ toolbar

fn toolbar(ui: &Ui, p: &mut Project, s: &mut UiState, bar: Rect, lanes_w: f32) {
    let g = ui.painter_at(bar);
    g.rect_filled(bar, 0.0, hw_tracks::PANEL_MENU());
    g.hline(
        bar.x_range(),
        bar.top() + 0.5,
        Stroke::new(1.0, theme::emboss(255)),
    );
    hw_tracks::seam_wide_h(&g, bar.x_range(), bar.bottom());
    // The keys sit on the row's centre, above the seam.
    let cy = bar.center().y - 1.0;
    let (can_undo, can_redo) = (s.can_undo, s.can_redo);
    let mut undo_pick = None;
    // An entry of the View or Snap menu run through `menus::apply` once the toolbar is drawn.
    let mut nav_pick: Option<&'static str> = None;
    let v = &mut s.tracks;

    let mut x = bar.left() + 12.0;
    let sel = v.selected;
    // Cut, Copy and Delete also act on the marquee.
    let cut_ok = sel.is_some() || crate::marquee::has_material(p, v);
    let (edit, w) = menu_title(ui, &g, x, cy, tr("Edit"), id(("menu", "Edit")));
    x += w + 12.0;
    egui::Popup::menu(&edit).show(|ui| {
        ui.set_min_width(260.0);
        crate::widgets::undo_items(ui, can_undo, can_redo, &mut undo_pick);
        ui.separator();
        if ui
            .add_enabled(cut_ok, egui::Button::new(tr("Cut")))
            .clicked()
        {
            edit_regions(p, v, &mut s.region_clip, &mut s.auto, RegionEdit::Cut);
            ui.close();
        }
        if ui
            .add_enabled(cut_ok, egui::Button::new(tr("Copy")))
            .clicked()
        {
            edit_regions(p, v, &mut s.region_clip, &mut s.auto, RegionEdit::Copy);
            ui.close();
        }
        if ui
            .add_enabled(
                crate::region_edit::can_paste(p, &s.region_clip),
                egui::Button::new(tr("Paste")),
            )
            .clicked()
        {
            edit_regions(p, v, &mut s.region_clip, &mut s.auto, RegionEdit::Paste);
            ui.close();
        }
        if ui
            .add_enabled(
                cut_ok,
                egui::Button::new(tr("Delete")).shortcut_text(tr("Del")),
            )
            .clicked()
        {
            delete_selected(p, v);
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                p.tracks.iter().any(|t| !t.regions.is_empty()),
                egui::Button::new(tr("Select All")),
            )
            .clicked()
        {
            v.select_all(p);
            ui.close();
        }
        if ui
            .add_enabled(
                crate::marquee::can_split_any(p, v),
                egui::Button::new(tr("Split Regions at Playhead")),
            )
            .clicked()
        {
            crate::marquee::split_any(p, v);
            ui.close();
        }
        if ui
            .add_enabled(
                crate::region_edit::can_join(p, v),
                egui::Button::new(tr("Join Regions")),
            )
            .clicked()
        {
            crate::region_edit::join_selected(p, v);
            ui.close();
        }
        let rep = shortcut(ui, Modifiers::COMMAND, Key::R);
        if ui
            .add_enabled(
                sel.is_some(),
                egui::Button::new(tr("Repeat Regions")).shortcut_text(rep),
            )
            .clicked()
        {
            s.track_dialog = Some(crate::track_dialogs::TrackDialog::Repeat {
                copies: "1".into(),
                adjust: crate::region_commands::RepeatAdjust::Auto,
            });
            ui.close();
        }
        ui.separator();
        // Live's Lock Envelopes, the other way round: on, envelopes move with the regions.
        if ui
            .checkbox(&mut s.auto.with_regions, tr("Move Automation with Regions"))
            .clicked()
        {
            ui.close();
        }
    });
    if undo_pick.is_some() {
        s.undo_cmd = undo_pick;
    }
    let (func, w) = menu_title(ui, &g, x, cy, tr("Functions"), id(("menu", "Functions")));
    x += w + 12.0;
    egui::Popup::menu(&func).show(|ui| {
        ui.set_min_width(260.0);
        if ui
            .add_enabled(
                sel.is_some(),
                egui::Button::new(tr("Set Locators by Regions")),
            )
            .clicked()
        {
            set_locators_by_region(p, v);
            ui.close();
        }
        ui.separator();
        // Folder: the selected tracks go into a track stack, as Track > Create Track Stack.
        if ui
            .add_enabled(
                crate::track_selection::can_create_stack(p, v),
                egui::Button::new(tr("Folder")),
            )
            .clicked()
        {
            crate::track_selection::create_stack_in(p, v);
            // Track numbers shift, so the remembered selections may point at other tracks.
            s.piano.selected.clear();
            s.pitch_ed.region = None;
            ui.close();
        }
        if ui
            .add_enabled(
                crate::region_edit::can_quantize(p, v),
                egui::Button::new(tr("Quantize")),
            )
            .clicked()
        {
            crate::region_edit::quantize_selected(p, v);
            ui.close();
        }
        if ui
            .add_enabled(sel.is_some(), egui::Button::new(tr("Move to Playhead")))
            .clicked()
        {
            let was = region_starts(p, &v.selection());
            crate::region_edit::move_to_playhead(p, v);
            crate::automation_ui::follow_moves(p, &s.auto, &was);
            ui.close();
        }
    });
    let (view, w) = menu_title(ui, &g, x, cy, tr("View"), id(("menu", "View")));
    x += w + 24.0;
    egui::Popup::menu(&view).show(|ui| {
        ui.set_min_width(260.0);
        if ui.button(tr("Zoom to Fit")).clicked() {
            zoom_to_fit(p, v, lanes_w);
            ui.close();
        }
        for label in [
            "Toggle Zoom to Fit Selection or All Contents",
            "Zoom Horizontal In",
            "Zoom Horizontal Out",
            "Zoom Vertical In",
            "Zoom Vertical Out",
        ] {
            nav_pick = nav_pick.or(crate::arrange_nav::item(ui, p, v, label));
        }
        ui.separator();
        if ui.checkbox(&mut v.catch, tr("Catch Playhead")).clicked() {
            ui.close();
        }
        nav_pick = nav_pick.or(crate::arrange_nav::item(ui, p, v, "Scroll in Play"));
        ui.separator();
        for label in [
            "Waveform Zoom In",
            "Waveform Zoom Out",
            "Colour Spectrogram",
        ] {
            nav_pick = nav_pick.or(crate::arrange_nav::item(ui, p, v, label));
        }
        ui.separator();
        if ui.button(tr("Configure Track Header…")).clicked() {
            s.track_dialog = Some(crate::track_dialogs::TrackDialog::Header);
            ui.close();
        }
    });

    // Catch: follow the playhead (a latching key with its LED).
    let r = Rect::from_min_size(pos2(x, cy - 15.0), vec2(52.0, 30.0));
    if glyph_key(ui, &g, r, "catch", v.catch, true, Glyph::Catch).clicked() {
        v.catch = !v.catch;
    }
    // Automation Mode (A), Draw Mode (B), Automation Arm, Re-Enable Automation.
    let w = crate::automation_ui::toolbar(ui, &g, p, &mut s.auto, r.right() + 10.0, cy);
    let left_end = r.right() + 10.0 + w;

    // Right-hand side, laid out from the right edge: the zoom sliders, each with its glyph.
    // Horizontal zoom (log scale, 6..600 points per bar).
    let slider_w = if bar.width() > 1500.0 { 110.0 } else { 80.0 };
    let hz = Rect::from_min_size(
        pos2(bar.right() - 18.0 - slider_w, cy - 2.0),
        vec2(slider_w, 4.0),
    );
    let (lo, hi) = (6f32.ln(), 600f32.ln());
    let t = ((v.bar_w.ln() - lo) / (hi - lo)).clamp(0.0, 1.0);
    let t2 = zoom_slider(ui, &g, hz, "hzoom", t);
    if t2 != t {
        v.bar_w = (lo + t2 * (hi - lo)).exp();
    }
    hw_tracks::glyph(
        &g,
        pos2(hz.left() - 19.0, cy),
        18.0,
        Glyph::HZoom,
        theme::INK2(),
    );
    // Vertical zoom (track height 36..140).
    let vz = Rect::from_min_size(
        pos2(hz.left() - 50.0 - slider_w, cy - 2.0),
        vec2(slider_w, 4.0),
    );
    let t = ((v.row_h - 36.0) / (140.0 - 36.0)).clamp(0.0, 1.0);
    let t2 = zoom_slider(ui, &g, vz, "vzoom", t);
    if t2 != t {
        v.row_h = 36.0 + t2 * (140.0 - 36.0);
    }
    hw_tracks::glyph(
        &g,
        pos2(vz.left() - 19.0, cy),
        18.0,
        Glyph::VZoom,
        theme::INK2(),
    );
    // What fits between the two sides; the rest is left out rather than drawn on top.
    let mut rx = vz.left() - 44.0;
    // SNAP_W and DRAG_W allow for the English "SNAP" and "DRAG"; longer translations take
    // the difference from the room left.
    let label_w = |t: &str| hw::silk_width(&g, no_colon(t), 11.0);
    let longer = (label_w(tr("Snap:")) - label_w("Snap:")).max(0.0)
        + (label_w(tr("Drag:")) - label_w("Drag:")).max(0.0);
    let show = fit_toolbar(rx - left_end - longer);
    if show.drag {
        rx = drag_menu(ui, &g, v, rx, cy);
    }
    if show.snap {
        rx = snap_menu(ui, &g, p, v, (rx, cy), &mut nav_pick);
    }
    if show.tools {
        tool_menus(ui, &g, v, rx, cy);
    }
    if let Some(label) = nav_pick {
        crate::menus::apply(label, p, s);
    }
}

/// Which optional toolbar items are shown, given the free width between the left-hand group
/// and the zoom sliders. Items that do not fit are hidden; kept first: Snap, then the tool
/// menu (it is the narrowest and otherwise vanished at laptop widths), then Drag.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ToolbarFit {
    snap: bool,
    drag: bool,
    tools: bool,
}

/// Widths (with their labels and gaps) of the optional toolbar items.
const SNAP_W: f32 = 214.0;
const DRAG_W: f32 = 214.0;
const TOOLS_W: f32 = 136.0;

/// A label such as "Snap:" as the silkscreen prints it, without its colon.
fn no_colon(t: &str) -> &str {
    t.trim_end_matches([':', '：', ' '])
}

fn fit_toolbar(free: f32) -> ToolbarFit {
    let mut left = free;
    let mut take = |w: f32| {
        let fits = left >= w;
        if fits {
            left -= w;
        }
        fits
    };
    let snap = take(SNAP_W);
    let tools = take(TOOLS_W);
    let drag = take(DRAG_W);
    ToolbarFit { snap, drag, tools }
}

/// "Drag:" mode menu ending at `rx`; returns where the next item to its left ends.
fn drag_menu(ui: &Ui, g: &Painter, v: &mut TracksView, rx: f32, cy: f32) -> f32 {
    let drag_r = Rect::from_min_size(pos2(rx - 150.0, cy - 13.0), vec2(150.0, 26.0));
    let resp = popup_button(ui, g, drag_r, id(("popup", "drag")), v.drag_mode.label());
    egui::Popup::menu(&resp).show(|ui| {
        for m in DragMode::ALL {
            if ui.selectable_label(v.drag_mode == m, m.label()).clicked() {
                v.drag_mode = m;
                ui.close();
            }
        }
    });
    let label = hw::silk(
        g,
        pos2(drag_r.left() - 10.0, cy),
        Align2::RIGHT_CENTER,
        no_colon(tr("Drag:")),
        11.0,
        theme::INK2(),
    );
    label.left() - 24.0
}

/// "Snap:" menu ending at `rx`; returns where the next item to its left ends. It reads "Off"
/// while Snap to Grid is off (Command-G). An entry of `arrange_nav` chosen in it is left in
/// `pick`.
fn snap_menu(
    ui: &Ui,
    g: &Painter,
    p: &Project,
    v: &mut TracksView,
    (rx, cy): (f32, f32),
    pick: &mut Option<&'static str>,
) -> f32 {
    let snap_r = Rect::from_min_size(pos2(rx - 150.0, cy - 13.0), vec2(150.0, 26.0));
    let shown = if v.grid.on { v.snap.label() } else { tr("Off") };
    let resp = popup_button(ui, g, snap_r, id(("popup", "snap")), shown);
    egui::Popup::menu(&resp).show(|ui| {
        for m in Snap::ALL {
            if ui.selectable_label(v.snap == m, m.label()).clicked() {
                v.snap = m;
                ui.close();
            }
        }
        ui.separator();
        // Logic's Division (the LCD's): what Division and Smart snap to.
        ui.menu_button(tr("Division Value"), |ui| {
            for d in crate::snap_grid::DIVISIONS {
                let text = crate::snap_grid::division_label(d);
                if ui.selectable_label(v.grid.division == d, text).clicked() {
                    v.grid.division = d;
                    ui.close();
                }
            }
        });
        ui.separator();
        for label in [
            "Snap Regions to Relative Value",
            "Snap Regions to Absolute Value",
            "-",
            "Snap to Grid",
        ] {
            if label == "-" {
                ui.separator();
            } else {
                *pick = pick.or(crate::arrange_nav::item(ui, p, v, label));
            }
        }
    });
    let label = hw::silk(
        g,
        pos2(snap_r.left() - 10.0, cy),
        Align2::RIGHT_CENTER,
        no_colon(tr("Snap:")),
        11.0,
        theme::INK2(),
    );
    label.left() - 24.0
}

/// The two tool menus ending at `rx`, as in Logic: the left-click tool, then the
/// Command-click tool. Each is a key with the tool's glyph and a menu triangle.
fn tool_menus(ui: &Ui, g: &Painter, v: &mut TracksView, rx: f32, cy: f32) {
    let cmd_r = Rect::from_min_size(pos2(rx - 56.0, cy - 15.0), vec2(56.0, 30.0));
    let left_r = cmd_r.translate(vec2(-64.0, 0.0));
    tool_key(ui, g, left_r, "tool_l", &mut v.tool, false);
    tool_key(ui, g, cmd_r, "tool_cmd", &mut v.cmd_tool, true);
}

/// One tool menu key of the toolbar; `cmd`: the Command-click tool's.
fn tool_key(ui: &Ui, g: &Painter, r: Rect, key: &str, tool: &mut TrackTool, cmd: bool) {
    let resp = ui.interact(r, id(key), Sense::click());
    let face = hw::paint_key(
        g,
        r,
        Cap::Bone,
        resp.is_pointer_button_down_on(),
        resp.hovered(),
    );
    hw_tracks::glyph(
        g,
        pos2(face.left() + 20.0, face.center().y),
        16.0,
        tool.glyph(),
        theme::INK(),
    );
    hw::tri_down(
        g,
        pos2(face.right() - 14.0, face.center().y + 0.5),
        theme::INK2(),
    );
    let tip = if cmd {
        tr("Command-click Tool")
    } else {
        tr("Left-click Tool")
    };
    let resp = resp.on_hover_text(tip);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(220.0);
        for (t, _, _, letter) in TrackTool::ALL {
            let label = if cmd {
                t.name().to_string()
            } else {
                format!("{}    {}", t.name(), letter)
            };
            if ui.selectable_label(*tool == t, label).clicked() {
                *tool = t;
                ui.close();
            }
        }
    });
}

// ------------------------------------------------------------------ right-click menus

/// An entry picked in one of the Tracks area's right-click menus; run once the menus are drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CtxAction {
    NewTrack(TrackKind),
    ZoomToFit,
    OpenPianoRoll,
    /// A Pattern or Drummer region becomes an ordinary MIDI region.
    ConvertToMidi(usize, usize),
    ToggleRegionMute(usize, usize),
    ToggleReverse(usize, usize),
    ToggleTakeFolder(usize, usize),
    FlattenTakeFolder(usize, usize),
    Normalize(usize, usize),
    /// Smart Tempo: say the region's tempo (false) or make it the project's (true).
    DetectTempo(usize, usize, bool),
    /// Smart Tempo of every audio region together (multitrack).
    AdaptTempoToAll,
    SmartTempoEditor(usize, usize),
    /// The audio region's notes in the Pitch Editor.
    PitchEditor(usize, usize),
    FlexMode(usize, usize, prowave_engine::FlexMode),
    Flex(usize, usize, crate::flex_edit::Command),
    FlexRemove(usize, usize),
    /// Fit to Grid: the region's tempo and beats onto the project's grid.
    FitToGrid(usize, usize),
    /// Time Stretch the audio to the nearest bar line (false) or to the locators (true).
    TimeStretch(usize, usize, bool),
    /// The Spectrogram window on an audio region.
    ShowSpectrogram(usize, usize),
    /// Paint Image to Audio: a picture becomes a new audio track.
    PaintImage,
    RepeatRegion,
    LocatorsByRegion,
    DeleteRegion,
    CreateStack(usize),
    ReleaseStack(usize),
    ToggleMute(usize),
    ToggleSolo(usize),
    ResetHeight(usize),
    DeleteTrack(usize),
    /// Makes the track the Groove Track, or (if it is) no track.
    ToggleGrooveTrack(usize),
    ToggleFollowGroove(usize),
}

/// What the track menu needs to know about the track that was right-clicked.
struct TrackInfo {
    i: usize,
    folder: bool,
    mute: bool,
    solo: bool,
    custom_height: bool,
    last: bool,
    /// Create Track Stack can gather the selected tracks (`track_selection::can_create_stack`).
    stack: bool,
    /// The Groove Track, and whether this track follows it (`None`: there is none).
    groove: bool,
    follow_groove: Option<bool>,
}

fn pick(ui: &mut Ui, label: &str, key: &str, action: CtxAction, picked: &mut Option<CtxAction>) {
    if ui
        .add(egui::Button::new(label).shortcut_text(key))
        .clicked()
    {
        *picked = Some(action);
    }
}

fn new_track_items(ui: &mut Ui, picked: &mut Option<CtxAction>) {
    for (label, kind) in [
        (tr("New Software Instrument Track"), TrackKind::Instrument),
        (tr("New Audio Track"), TrackKind::Audio),
        (tr("New Aux Track"), TrackKind::Aux),
        (tr("New VCA Track"), TrackKind::Vca),
        (tr("New External MIDI Track"), TrackKind::ExternalMidi),
    ] {
        pick(ui, label, "", CtxAction::NewTrack(kind), picked);
    }
    pick(
        ui,
        tr("Paint Image to Audio…"),
        "",
        CtxAction::PaintImage,
        picked,
    );
}

#[allow(clippy::too_many_arguments)]
fn region_items(
    ui: &mut Ui,
    is_audio: bool,
    muted: bool,
    folder: Option<bool>,
    generated: bool,
    ti: usize,
    ri: usize,
    picked: &mut Option<CtxAction>,
) {
    if !is_audio {
        let open = if generated {
            tr("Open Editor")
        } else {
            tr("Open in Piano Roll")
        };
        pick(ui, open, "", CtxAction::OpenPianoRoll, picked);
        if generated {
            pick(
                ui,
                tr("Convert to MIDI Region"),
                "",
                CtxAction::ConvertToMidi(ti, ri),
                picked,
            );
        }
        ui.separator();
    }
    if let Some(open) = folder {
        let toggle = if open {
            tr("Close Take Folder")
        } else {
            tr("Open Take Folder")
        };
        pick(ui, toggle, "", CtxAction::ToggleTakeFolder(ti, ri), picked);
        pick(
            ui,
            tr("Flatten Take Folder"),
            "",
            CtxAction::FlattenTakeFolder(ti, ri),
            picked,
        );
        ui.separator();
    }
    let mute = if muted {
        tr("Unmute Region")
    } else {
        tr("Mute Region")
    };
    pick(ui, mute, "", CtxAction::ToggleRegionMute(ti, ri), picked);
    if is_audio {
        pick(
            ui,
            tr("Normalize Region Gain"),
            "",
            CtxAction::Normalize(ti, ri),
            picked,
        );
        pick(
            ui,
            tr("Show Spectrogram"),
            "",
            CtxAction::ShowSpectrogram(ti, ri),
            picked,
        );
        ui.menu_button(tr("Time Stretch"), |ui| {
            pick(
                ui,
                tr("Time Stretch Region to Nearest Bar"),
                "",
                CtxAction::TimeStretch(ti, ri, false),
                picked,
            );
            pick(
                ui,
                tr("Time Stretch Region to Locators"),
                "",
                CtxAction::TimeStretch(ti, ri, true),
                picked,
            );
        });
        pick(
            ui,
            tr("Open in Pitch Editor"),
            "",
            CtxAction::PitchEditor(ti, ri),
            picked,
        );
        ui.menu_button(tr("Smart Tempo"), |ui| {
            pick(
                ui,
                tr("Smart Tempo Editor…"),
                "",
                CtxAction::SmartTempoEditor(ti, ri),
                picked,
            );
            pick(
                ui,
                tr("Detect Tempo"),
                "",
                CtxAction::DetectTempo(ti, ri, false),
                picked,
            );
            pick(
                ui,
                tr("Adapt Project Tempo to Region"),
                "",
                CtxAction::DetectTempo(ti, ri, true),
                picked,
            );
            pick(
                ui,
                tr("Adapt Project Tempo to All Audio Regions"),
                "",
                CtxAction::AdaptTempoToAll,
                picked,
            );
            pick(
                ui,
                tr("Region Follows Project Tempo"),
                "",
                CtxAction::Flex(ti, ri, crate::flex_edit::Command::FollowProjectTempo),
                picked,
            );
        });
        ui.menu_button(tr("Flex Time"), |ui| {
            pick(
                ui,
                tr("Fit to Grid"),
                "",
                CtxAction::FitToGrid(ti, ri),
                picked,
            );
            ui.menu_button(tr("Flex Mode"), |ui| {
                for mode in prowave_engine::FlexMode::ALL {
                    pick(
                        ui,
                        crate::flex_edit::mode_label(mode),
                        "",
                        CtxAction::FlexMode(ti, ri, mode),
                        picked,
                    );
                }
            });
            pick(
                ui,
                tr("Quantize Audio to 1/16"),
                "",
                CtxAction::Flex(ti, ri, crate::flex_edit::Command::Quantize { grid: 0.25 }),
                picked,
            );
            pick(
                ui,
                tr("Quantize Audio to 1/8"),
                "",
                CtxAction::Flex(ti, ri, crate::flex_edit::Command::Quantize { grid: 0.5 }),
                picked,
            );
            pick(
                ui,
                tr("Remove Flex Edits"),
                "",
                CtxAction::FlexRemove(ti, ri),
                picked,
            );
        });
        ui.menu_button(tr("Flex Pitch"), |ui| {
            pick(
                ui,
                tr("Correct Pitch to Key"),
                "",
                CtxAction::Flex(ti, ri, crate::flex_edit::Command::CorrectPitch),
                picked,
            );
            pick(
                ui,
                tr("Create MIDI Track from Flex Pitch"),
                "",
                CtxAction::Flex(ti, ri, crate::flex_edit::Command::MidiFromPitch),
                picked,
            );
        });
        pick(
            ui,
            tr("Reverse Region"),
            "",
            CtxAction::ToggleReverse(ti, ri),
            picked,
        );
    }
    // One copy at once; Command-R opens the Repeat window instead.
    pick(ui, tr("Repeat Region"), "", CtxAction::RepeatRegion, picked);
    pick(
        ui,
        tr("Set Locators by Region"),
        "",
        CtxAction::LocatorsByRegion,
        picked,
    );
    ui.separator();
    pick(ui, tr("Delete"), "⌫", CtxAction::DeleteRegion, picked);
}

fn track_items(ui: &mut Ui, t: &TrackInfo, picked: &mut Option<CtxAction>) {
    new_track_items(ui, picked);
    ui.separator();
    // On a selected track: every selected track goes in (`track_selection.rs`).
    let stack = ui.add_enabled(t.stack, egui::Button::new(tr("Create Track Stack")));
    if stack.clicked() {
        *picked = Some(CtxAction::CreateStack(t.i));
    }
    if t.folder {
        pick(
            ui,
            tr("Release Track Stack"),
            "",
            CtxAction::ReleaseStack(t.i),
            picked,
        );
    }
    ui.separator();
    let mute = if t.mute {
        tr("Unmute Track")
    } else {
        tr("Mute Track")
    };
    pick(ui, mute, "", CtxAction::ToggleMute(t.i), picked);
    let solo = if t.solo {
        tr("Unsolo Track")
    } else {
        tr("Solo Track")
    };
    pick(ui, solo, "", CtxAction::ToggleSolo(t.i), picked);
    if t.custom_height {
        pick(
            ui,
            tr("Reset Track Height"),
            "",
            CtxAction::ResetHeight(t.i),
            picked,
        );
    }
    ui.separator();
    let groove = if t.groove {
        tr("Clear Groove Track")
    } else {
        tr("Set as Groove Track")
    };
    pick(ui, groove, "", CtxAction::ToggleGrooveTrack(t.i), picked);
    if let Some(follows) = t.follow_groove {
        let label = if follows {
            tr("Stop Following Groove Track")
        } else {
            tr("Follow Groove Track")
        };
        pick(ui, label, "", CtxAction::ToggleFollowGroove(t.i), picked);
    }
    ui.separator();
    let delete = ui.add_enabled(!t.last, egui::Button::new(tr("Delete Track")));
    if delete.clicked() {
        *picked = Some(CtxAction::DeleteTrack(t.i));
    }
}

/// Runs a right-click menu entry.
fn apply_ctx(action: CtxAction, p: &mut Project, s: &mut UiState, lanes_w: f32) {
    let v = &mut s.tracks;
    match action {
        CtxAction::NewTrack(kind) => {
            p.add_track(kind);
        }
        CtxAction::ZoomToFit => zoom_to_fit(p, v, lanes_w),
        CtxAction::OpenPianoRoll => s.bottom = Bottom::PianoRoll,
        CtxAction::ConvertToMidi(t, r) => p.convert_to_midi_region(t, r),
        CtxAction::ToggleRegionMute(t, r) => {
            crate::region_edit::toggle_mute(p, &s.tracks, Some((t, r)));
        }
        CtxAction::ToggleTakeFolder(t, r) => {
            if let Some(f) = p
                .tracks
                .get_mut(t)
                .and_then(|t| t.regions.get_mut(r))
                .and_then(|r| r.folder.as_mut())
            {
                f.open = !f.open;
            }
        }
        // The comp as it is becomes an ordinary region: the other takes are let go.
        CtxAction::FlattenTakeFolder(t, r) => {
            if let Some(region) = p.tracks.get_mut(t).and_then(|t| t.regions.get_mut(r)) {
                region.folder = None;
            }
        }
        CtxAction::ToggleReverse(t, r) => {
            crate::region_commands::reverse(p, v, Some((t, r)));
        }
        CtxAction::Normalize(t, r) => start_normalize(p, s, t, r),
        CtxAction::ShowSpectrogram(t, r) => crate::spectrogram_view::open(p, s, t, r),
        CtxAction::PaintImage => s.paint.pick = true,
        CtxAction::TimeStretch(t, r, to_locators) => {
            let Some(region) = p.tracks.get(t).and_then(|x| x.regions.get(r)) else {
                return;
            };
            let start = p.region_start_beats(region);
            let (from, to) = if to_locators {
                (p.cycle_start, p.cycle_end)
            } else {
                let end = p.region_end_beats(region);
                let mut bar = p.sigs().nearest_bar_line(end);
                if bar <= start {
                    bar = p.bar_to_beats(p.beats_to_bar(start).floor() + 1.0);
                }
                (start, bar)
            };
            if !crate::flex_edit::stretch_to(p, t, r, from, to) {
                s.status = Some(tr("This region cannot be stretched").into());
            }
        }
        CtxAction::DetectTempo(t, r, adapt) => crate::smart_tempo::start(p, s, t, r, adapt),
        CtxAction::AdaptTempoToAll => crate::smart_tempo::start_all(p, s),
        CtxAction::ToggleGrooveTrack(i) => {
            let was = p.tracks.get(i).is_some_and(|t| t.params.groove);
            crate::groove::set_groove_track(p, (!was).then_some(i));
        }
        CtxAction::ToggleFollowGroove(i) => {
            if let Some(t) = p.tracks.get_mut(i) {
                t.params.follow_groove = !t.params.follow_groove;
                p.song_rev += 1;
            }
        }
        CtxAction::SmartTempoEditor(t, r) => crate::smart_tempo::open_editor(p, s, t, r),
        CtxAction::PitchEditor(t, r) => crate::pitch_editor::open(s, t, r),
        CtxAction::FlexMode(t, r, mode) => {
            crate::flex_edit::set_mode(p, t, r, mode);
            p.song_rev += 1;
        }
        CtxAction::Flex(t, r, command) => crate::flex_edit::start(p, s, t, r, command),
        CtxAction::FlexRemove(t, r) => s.flex.remove_later(t, r),
        CtxAction::FitToGrid(t, r) => crate::fit_grid::start_region(p, s, t, r),
        CtxAction::RepeatRegion => repeat_with_automation(p, v, &s.auto),
        CtxAction::LocatorsByRegion => set_locators_by_region(p, v),
        CtxAction::DeleteRegion => delete_selected(p, v),
        // On a selected track the right-click entries act on every selected track, as the
        // Track menu's do (a right-click on a header makes that track the focused one).
        CtxAction::CreateStack(i) if crate::track_selection::is_selected(p, v, i) => {
            crate::track_selection::create_stack(p, s);
        }
        CtxAction::CreateStack(i) => {
            p.create_track_stack(i);
            forget_track_indexes(s);
        }
        CtxAction::ReleaseStack(i) => {
            if p.release_track_stack(i) {
                forget_track_indexes(s);
            }
        }
        CtxAction::ToggleMute(i) if i == p.selected_track => {
            crate::track_selection::toggle_mute(p, v);
        }
        CtxAction::ToggleSolo(i) if i == p.selected_track => {
            crate::track_selection::toggle_solo(p, v);
        }
        CtxAction::ToggleMute(i) => {
            let mute = !p.tracks[i].mute;
            p.set_folder_flags(i, Some(mute), None);
        }
        CtxAction::ToggleSolo(i) => {
            let solo = !p.tracks[i].solo;
            p.set_folder_flags(i, None, Some(solo));
        }
        CtxAction::ResetHeight(i) => p.tracks[i].height = 1.0,
        CtxAction::DeleteTrack(i) if i == p.selected_track => {
            crate::track_selection::delete_tracks(p, s);
        }
        CtxAction::DeleteTrack(i) => {
            if p.delete_track(i) {
                forget_track_indexes(s);
            }
        }
    }
}

/// Logic's Normalize Region Gain: finds the region's loudest sample on a background thread,
/// then [`poll_normalize`] sets the gain that brings it to 0 dBFS. The file is not changed.
fn start_normalize(p: &Project, s: &mut UiState, track: usize, region: usize) {
    let Some(audio) = p
        .tracks
        .get(track)
        .and_then(|t| t.regions.get(region))
        .and_then(|r| r.audio.as_ref())
    else {
        s.status = Some(tr("Normalize: this region has no recorded audio").into());
        return;
    };
    let jobs = &mut s.tracks.normalize_jobs;
    if jobs.iter().any(|j| (j.track, j.region) == (track, region)) {
        return;
    }
    let file = audio.file.clone();
    let path = file.path.clone();
    let (tx, result) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("prowave-normalize".into())
        .spawn(move || {
            let peak = prowave_engine::peak_of(&file).map_err(|e| e.to_string());
            let _ = tx.send(peak);
        });
    match spawned {
        Ok(_) => {
            jobs.push(NormalizeJob {
                track,
                region,
                path,
                result,
            });
            s.status = Some(tr("Normalizing…").into());
        }
        Err(e) => s.status = Some(format!("cannot start the normalize scan: {e}")),
    }
}

/// Applies the Normalize Region Gain scans that have finished. Call once per frame.
pub fn poll_normalize(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    use std::sync::mpsc::TryRecvError;
    if s.tracks.normalize_jobs.is_empty() {
        return;
    }
    let mut finished = Vec::new();
    let mut message = None;
    s.tracks
        .normalize_jobs
        .retain(|job| match job.result.try_recv() {
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
            Ok(peak) => {
                finished.push((job.track, job.region, job.path.clone(), peak));
                false
            }
        });
    for (track, region, path, peak) in finished {
        // The project may have changed while the file was being read.
        let Some(r) = p
            .tracks
            .get_mut(track)
            .and_then(|t| t.regions.get_mut(region))
            .filter(|r| r.audio.as_ref().is_some_and(|a| a.file.path == path))
        else {
            continue;
        };
        match peak {
            Ok(peak) => match crate::model::RegionParams::normalize_gain(peak) {
                Some(gain) => r.params.gain_db = gain,
                None => message = Some(tr("Normalize: the region is silent").to_string()),
            },
            Err(e) => {
                message = Some(trf("Normalize: cannot read {}: {}", &[&path.display(), &e]));
            }
        }
    }
    if s.tracks.normalize_jobs.is_empty() {
        // Clears "Normalizing…" (or shows what went wrong).
        s.status = message;
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Track numbers shifted, so a remembered region or note selection may point at another track.
pub(crate) fn forget_track_indexes(s: &mut UiState) {
    s.tracks.clear_selection();
    s.tracks.range.clear();
    s.piano.selected.clear();
    s.pitch_ed.region = None;
    // A selected range on an automation lane names its track by number too.
    s.auto.forget();
}

// ------------------------------------------------------------------ actions

/// Delete: the material inside the marquee when there is one (`marquee.rs`), else the
/// selected marker, else the selected regions, or with none selected, the selected track when
/// it holds no regions (a track with regions goes only by Delete Track, Command-Delete).
pub(crate) fn delete_selected(p: &mut Project, v: &mut TracksView) {
    if v.range.active().is_some() {
        crate::marquee::delete(p, v);
        return;
    }
    if v.selected_marker.is_some_and(|i| i < p.markers.len()) {
        crate::marker_edit::delete(p, v);
        return;
    }
    let mut sel = v.selection();
    if sel.is_empty() {
        // Only an empty track that holds regions itself: a stack, an Aux or a VCA track has
        // none and is not "empty" (Delete Track, Command-Delete, removes those).
        if p.tracks
            .get(p.selected_track)
            .is_some_and(|t| t.kind.has_regions() && t.regions.is_empty())
        {
            p.delete_track(p.selected_track);
        }
        return;
    }
    // Shuffle L and R: the others close up (`drag_modes.rs`).
    crate::drag_modes::close_deleted(p, &sel, v.drag_mode);
    // Highest index first on each track, so the ones still to go keep their numbers.
    sel.sort_unstable();
    sel.reverse();
    for (t, r) in sel {
        if let Some(track) = p.tracks.get_mut(t) {
            if r < track.regions.len() {
                track.regions.remove(r);
            }
        }
    }
    v.clear_selection();
    p.song_rev += 1;
}

/// Edit > Cut, Copy and Paste of regions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RegionEdit {
    Cut,
    Copy,
    Paste,
}

/// Cut, Copy or Paste of the selected regions, with the automation under them (Live: with
/// Lock Envelopes off, envelopes are cut, copied and pasted with their clips).
pub(crate) fn edit_regions(
    p: &mut Project,
    v: &mut TracksView,
    clip: &mut crate::region_edit::RegionClip,
    auto: &mut crate::automation::AutoView,
    what: RegionEdit,
) {
    // With a marquee, Cut and Copy take the material inside it (`marquee.rs`).
    if v.range.active().is_some() {
        match what {
            RegionEdit::Cut => return crate::marquee::cut(p, v, clip),
            RegionEdit::Copy => {
                crate::marquee::copy(p, v, clip);
                return;
            }
            RegionEdit::Paste => {}
        }
    }
    match what {
        RegionEdit::Cut => {
            crate::automation_ui::copy_with_regions(p, auto, &v.selection());
            crate::automation_ui::cut_with_regions(p, auto);
            crate::region_edit::cut_selected(p, v, clip);
        }
        RegionEdit::Copy => {
            crate::automation_ui::copy_with_regions(p, auto, &v.selection());
            crate::region_edit::copy_selected(p, v, clip);
        }
        RegionEdit::Paste => {
            let at = p.position_beats;
            let fits = crate::region_edit::can_paste(p, clip);
            crate::region_edit::paste_at_playhead(p, v, clip);
            if fits {
                crate::automation_ui::paste_with_regions(p, auto, at);
            }
        }
    }
}

/// Where each of `sel` starts now, in beats: (track, region, beat).
fn region_starts(p: &Project, sel: &[(usize, usize)]) -> Vec<(usize, usize, f64)> {
    sel.iter()
        .filter_map(|&(t, r)| {
            let reg = p.tracks.get(t)?.regions.get(r)?;
            Some((t, r, p.region_start_beats(reg)))
        })
        .collect()
}

/// Repeat Region (the right-click menu): one copy of the selected regions, right after them
/// (after their loops), with the automation under them.
fn repeat_with_automation(p: &mut Project, v: &mut TracksView, auto: &crate::automation::AutoView) {
    crate::region_commands::repeat(p, v, auto, 1, crate::region_commands::RepeatAdjust::Auto);
}

pub(crate) fn set_locators_by_region(p: &mut Project, v: &TracksView) {
    // The span from the start of the first selected region to the end of the last one.
    let span = v
        .selection()
        .into_iter()
        .filter_map(|(t, r)| p.tracks.get(t)?.regions.get(r))
        .map(|reg| (p.region_start_beats(reg), p.region_end_beats(reg)))
        .fold(None, |acc: Option<(f64, f64)>, (s, e)| match acc {
            None => Some((s, e)),
            Some((a, b)) => Some((a.min(s), b.max(e))),
        });
    if let Some((start, end)) = span {
        p.cycle_start = start;
        p.cycle_end = end;
        p.cycle = true;
    }
}

/// Move to Playhead (Semicolon): the selected regions, with the automation under them.
pub(crate) fn move_to_playhead(p: &mut Project, s: &mut UiState) {
    let was = region_starts(p, &s.tracks.selection());
    crate::region_edit::move_to_playhead(p, &s.tracks);
    crate::automation_ui::follow_moves(p, &s.auto, &was);
}

/// Navigate > Zoom to Fit: the whole song from its start to where the last region (or its
/// loop) ends, across the lanes. Bars are counted in the first bar's length, as `bar_w` is.
pub(crate) fn zoom_to_fit(p: &Project, v: &mut TracksView, lanes_w: f32) {
    let end = crate::arrange_zoom::contents_end(p);
    if end > 0.0 {
        let bars = end / p.bar_len(0).max(1e-6);
        v.bar_w = ((lanes_w - 20.0) / bars as f32).clamp(6.0, 600.0);
        v.scroll_x = 0.0;
    }
}

// ------------------------------------------------------------------ drawing

/// The material a region is drawn in (A.5): audio regions warm grey with a waveform, Drummer
/// regions hatched bone with drum hits, every other MIDI region black with note dashes.
fn material_of(track: &Track, region: &Region) -> Material {
    let drummer = region
        .generated
        .as_ref()
        .is_some_and(|g| matches!(g.source, crate::model::Generator::Drummer(_)));
    if region.is_audio() || track.kind == TrackKind::Audio {
        Material::Audio
    } else if drummer || track.kind == TrackKind::Drummer {
        Material::Drum
    } else {
        Material::Inst
    }
}

/// The colours of `region` on `track`: the region's own colour (View > Show Colors) or else
/// the track's, in the header, and a hint of it in the body; the muted look is drawn by the
/// region painter itself.
fn look_of(
    track: &Track,
    region: &Region,
    material: Material,
    selected: bool,
) -> theme::RegionLook {
    let color = region.params.color.unwrap_or(track.color);
    theme::region_look(color, material == Material::Audio, selected, false)
}

/// How the Tracks area draws `region` on `track`, unselected (for the tests).
#[cfg(test)]
pub(crate) fn region_look_for_test(track: &Track, region: &Region) -> theme::RegionLook {
    look_of(track, region, material_of(track, region), false)
}

// ------------------------------------------------------------------ take folders

/// Height of one take's row in an opened take folder.
const TAKE_ROW_H: f32 = 22.0;
/// The header of a take folder (the same bar as any region's).
const FOLDER_HEAD_H: f32 = 15.0;

/// Extra height a track's row needs for its opened take folders (one row per take).
fn take_rows_height(t: &Track) -> f32 {
    t.regions
        .iter()
        .filter_map(|r| r.folder.as_ref())
        .filter(|f| f.open)
        .map(|f| f.takes.len() as f32 * TAKE_ROW_H)
        .fold(0.0, f32::max)
}

/// The triangle that opens and closes a take folder, at the left of its header.
fn folder_toggle_rect(full: Rect) -> Rect {
    Rect::from_min_size(full.min, vec2(16.0, FOLDER_HEAD_H))
}

/// The row of take number `take` (0 = first take) in an opened folder: the latest take is on
/// top, as in Logic. `full` is the folder's own rectangle.
fn take_row_rect(full: Rect, takes: usize, take: usize) -> Rect {
    let k = takes - 1 - take;
    Rect::from_min_size(
        pos2(full.left(), full.bottom() + 1.0 + k as f32 * TAKE_ROW_H),
        vec2(full.width(), TAKE_ROW_H - 2.0),
    )
}

/// Note dashes (pitch up, time right) of `notes` in `inner`: `h` points tall, a fixed step per
/// semitone with the notes centred in the region.
#[allow(clippy::too_many_arguments)]
fn draw_note_lines<'a>(
    g: &Painter,
    inner: Rect,
    left: f32,
    beat_px: f32,
    notes: impl Iterator<Item = &'a crate::model::Note>,
    lo: f32,
    hi: f32,
    ink: Color32,
) {
    let h = if inner.height() >= 24.0 { 3.0 } else { 2.0 };
    let step = (4.0f32).min((inner.height() - h - 2.0).max(1.0) / (hi - lo).max(1.0));
    let mid = (lo + hi) / 2.0;
    for n in notes {
        let nx = left + n.start * beat_px;
        if nx > inner.right() || nx + n.len * beat_px < inner.left() {
            continue;
        }
        // On whole points, so every note stays crisp; a 1-point gap before the next note.
        let ny = (inner.center().y - (n.pitch as f32 - mid) * step - h / 2.0).round();
        let x0 = nx.round();
        let x1 = (nx + n.len * beat_px).round() - 1.0;
        let nr = Rect::from_min_max(
            pos2(x0.max(inner.left()), ny),
            pos2(x1.max(x0 + 2.0).min(inner.right()), ny + h),
        );
        g.rect_filled(nr, 0.0, ink);
    }
}

/// Which of a Drummer region's three lanes a drum note plays in: cymbals and hi-hats on top,
/// snares, claps and toms in the middle, kicks at the bottom.
fn drum_lane(pitch: u8) -> usize {
    match pitch {
        35 | 36 => 2,
        42 | 44 | 46 | 49 | 51..=53 | 55 | 57 | 59 => 0,
        _ => 1,
    }
}

/// Drum hits: a bar of ink per note in its lane, at most 4 points wide.
fn draw_drum_hits(
    g: &Painter,
    inner: Rect,
    left: f32,
    beat_px: f32,
    notes: &[crate::model::Note],
    ink: Color32,
) {
    let lane_h = inner.height() / 3.0;
    for n in notes {
        let nx = left + n.start * beat_px;
        if nx > inner.right() || nx + 4.0 < inner.left() {
            continue;
        }
        let w = (beat_px / 6.0).clamp(1.5, 4.0);
        let top = inner.top() + drum_lane(n.pitch) as f32 * lane_h + 1.0;
        let r = Rect::from_min_size(pos2(nx, top), vec2(w, (lane_h - 2.0).max(1.0)));
        g.rect_filled(r.intersect(inner), 0.0, ink);
    }
}

/// A take folder: a region whose header carries the open/close triangle, the comp (the
/// stretches of the takes that play) side by side with ink lines between them, and, opened,
/// a recessed row for each take with the stretches the comp uses in the region material.
#[allow(clippy::too_many_arguments)]
fn draw_take_folder(
    g: &Painter,
    full: Rect,
    track: &Track,
    number: usize,
    region: &Region,
    folder: &crate::model::TakeFolder,
    beat_px: f32,
    selected: bool,
    silenced: bool,
) {
    let material = material_of(track, region);
    let look = look_of(track, region, material, selected);
    let body = hw_tracks::region(
        g,
        full,
        &hw_tracks::RegionPaint {
            material,
            look,
            selected,
            muted: region.params.mute,
            dim: silenced,
            recording: false,
            number,
            name: &region.name,
            lead: 14.0,
        },
    );
    let faded = region.params.mute || silenced;
    let head_ink = if selected {
        theme::ON_ACCENT()
    } else if faded {
        theme::INK2()
    } else {
        theme::pick(theme::BONE(), theme::ON_DARK())
    };
    let head = Rect::from_min_max(full.min, pos2(full.right(), body.top()));
    // The triangle: pointing right when closed, down when opened.
    hw_tracks::disclosure(
        g,
        pos2(full.left() + 8.0, head.center().y),
        folder.open,
        head_ink,
    );
    // Where one stretch gives way to the next: a notch in the header and a line below it.
    let ppp = g.pixels_per_point();
    let seg_x = |at: f32| hw_tracks::pixel_edge(full.left() + at * beat_px, ppp);
    for seg in folder.comp.iter().skip(1) {
        let x = seg_x(seg.start);
        if x <= full.left() || x >= full.right() {
            continue;
        }
        hw_tracks::time_line(g, x, head.bottom() - 4.0..=head.bottom(), 1.0, head_ink);
        hw_tracks::time_line(
            g,
            x,
            body.top()..=body.bottom(),
            1.0,
            theme::INK().gamma_multiply(0.7),
        );
    }
    // What the comp plays.
    let mut ink = material.ink();
    if faded {
        ink = ink.gamma_multiply(0.45);
    }
    let (lo, hi) = pitch_range(&region.notes);
    let inner = body.shrink2(vec2(0.0, 3.0));
    let gb = g.with_clip_rect(body.intersect(g.clip_rect()));
    draw_note_lines(
        &gb,
        inner,
        full.left(),
        beat_px,
        region.notes.iter(),
        lo,
        hi,
        ink,
    );

    // The takes, newest on top.
    if folder.open {
        for (take_idx, take) in folder.takes.iter().enumerate() {
            let row = take_row_rect(full, folder.takes.len(), take_idx);
            hw_tracks::well(g, row, 3.0);
            let row_inner = row.shrink2(vec2(0.0, 2.0));
            let (lo, hi) = pitch_range(&take.notes);
            // The stretches of this take that the comp plays are in the region material.
            for seg in folder.comp.iter().filter(|s| s.take == take_idx) {
                let x0 = seg_x(seg.start).max(full.left());
                let x1 = seg_x(seg.end).min(full.right());
                if x1 > x0 {
                    let r = Rect::from_min_max(pos2(x0, row.top()), pos2(x1, row.bottom()));
                    g.rect_filled(r, 0.0, look.body);
                    g.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(1.0, theme::INK().gamma_multiply(0.75)),
                        StrokeKind::Inside,
                    );
                }
            }
            let used = |start: f32| {
                folder
                    .comp
                    .iter()
                    .any(|s| s.take == take_idx && start >= s.start && start < s.end)
            };
            let gr = g.with_clip_rect(row.intersect(g.clip_rect()));
            draw_note_lines(
                &gr,
                row_inner,
                full.left(),
                beat_px,
                take.notes.iter().filter(|n| used(n.start)),
                lo,
                hi,
                material.ink(),
            );
            draw_note_lines(
                &gr,
                row_inner,
                full.left(),
                beat_px,
                take.notes.iter().filter(|n| !used(n.start)),
                lo,
                hi,
                theme::PRINT(),
            );
            // The take's number on a small bone plate, readable on either material.
            let label = trf("Take {}", &[&(take_idx + 1)]).to_uppercase();
            let gal = hw_tracks::galley(&gr, &label, Face::Silk, 10.0, 0.12, theme::INK());
            let plate = Rect::from_min_size(
                row.left_top() + vec2(4.0, 3.0),
                vec2(gal.size().x + 6.0, row.height() - 6.0),
            );
            gr.rect_filled(plate, 1.5, theme::BONE());
            gr.galley(
                pos2(plate.left() + 3.0, plate.center().y - gal.size().y / 2.0),
                gal,
                theme::INK(),
            );
        }
    }
}

/// Lowest and highest pitch of `notes` (a default span when there are none).
fn pitch_range<'a>(notes: impl IntoIterator<Item = &'a crate::model::Note>) -> (f32, f32) {
    let (lo, hi) = notes
        .into_iter()
        .fold((u8::MAX, 0u8), |(l, h), n| (l.min(n.pitch), h.max(n.pitch)));
    if lo > hi {
        (48.0, 72.0)
    } else {
        (lo as f32, hi as f32)
    }
}

/// The notes of a region that show inside it: those that sound before its end, which moves
/// when the region is shortened.
fn visible_notes(region: &Region, len_beats: f32) -> impl Iterator<Item = &crate::model::Note> {
    region
        .notes
        .iter()
        .filter(move |n| n.start < len_beats && n.start + n.len > 0.0)
}

/// The part of its file an audio region plays and how that runs across the region on screen:
/// what its waveform and Flex markers are drawn from. A cut or a trim changes the offset and
/// the length, so the picture follows the edit instead of squeezing the whole file in.
pub(crate) struct AudioSpan<'a> {
    map: &'a prowave_engine::TempoMap,
    /// Where the region starts, in quarter notes and in seconds.
    start_beat: f64,
    start_secs: f64,
    /// Where in the file the region starts, and how many seconds of the file it plays.
    offset: f64,
    len: f64,
    /// The whole file, in seconds.
    file: f64,
    reverse: bool,
}

impl<'a> AudioSpan<'a> {
    /// The span of region `r` (`None` when it has no audio). `map` is `p.tempo_map()`.
    pub(crate) fn of(p: &Project, map: &'a prowave_engine::TempoMap, r: &Region) -> Option<Self> {
        let a = r.audio.as_ref()?;
        let start_beat = p.region_start_beats(r);
        let start_secs = map.seconds_at(start_beat);
        let region_secs = (map.seconds_at(p.region_end_beats(r)) - start_secs).max(0.0);
        let file = a.file.duration_secs().max(0.0);
        let offset = r.params.offset_secs.max(0.0).min(file);
        Some(Self {
            map,
            start_beat,
            start_secs,
            offset,
            len: region_secs.min(file - offset),
            file,
            reverse: r.params.reverse,
        })
    }

    /// Seconds into the region at `beats` quarter notes from its start.
    fn secs_at(&self, beats: f64) -> f64 {
        self.map.seconds_at(self.start_beat + beats) - self.start_secs
    }

    /// Quarter notes from the region's start at `secs` seconds into it.
    fn beats_at(&self, secs: f64) -> f64 {
        self.map.beat_at(self.start_secs + secs) - self.start_beat
    }

    /// The second of the file heard `u` seconds into the region (`None` outside it).
    pub(crate) fn file_secs(&self, u: f64) -> Option<f64> {
        let f = if self.reverse {
            self.offset + self.len - u
        } else {
            self.offset + u
        };
        (0.0..=self.len).contains(&u).then_some(f)
    }

    /// Seconds into the region at which the file's second `f` is heard (`None` when the region
    /// does not play it).
    fn region_secs(&self, f: f64) -> Option<f64> {
        let u = if self.reverse {
            self.offset + self.len - f
        } else {
            f - self.offset
        };
        (0.0..=self.len).contains(&u).then_some(u)
    }

    /// The lowest and highest level the overview `peaks` (of the whole file) holds for the
    /// part heard from `u0` to `u1` seconds into the region; `None` where nothing plays.
    pub(crate) fn peak_range(&self, peaks: &[(f32, f32)], u0: f64, u1: f64) -> Option<(f32, f32)> {
        let (u0, u1) = (u0.max(0.0), u1.min(self.len));
        if peaks.is_empty() || self.file <= 0.0 || u1 <= u0 {
            return None;
        }
        let (f0, f1) = if self.reverse {
            (self.offset + self.len - u1, self.offset + self.len - u0)
        } else {
            (self.offset + u0, self.offset + u1)
        };
        let n = peaks.len();
        let bin = |f: f64| f / self.file * n as f64;
        let a = (bin(f0).floor().max(0.0) as usize).min(n - 1);
        let b = (bin(f1).ceil() as usize).clamp(a + 1, n);
        Some(
            peaks[a..b]
                .iter()
                .fold((0f32, 0f32), |(l, h), p| (l.min(p.0), h.max(p.1))),
        )
    }
}

/// A region in its material, with its notes, hits or waveform, bar lines inside it at
/// `bar_xs`, fades, Flex markers and the Drummer/Pattern mark.
#[allow(clippy::too_many_arguments)]
fn draw_region(
    g: &Painter,
    rr: Rect,
    full: Rect,
    track: &Track,
    number: usize,
    region: &Region,
    fades: (f32, f32),
    under: Option<crate::fade_curve::Under>,
    beat_px: f32,
    selected: bool,
    recording: bool,
    silenced: bool,
    bar_xs: &[f32],
    span: Option<&AudioSpan>,
) {
    let material = material_of(track, region);
    let muted = region.params.mute;
    let faded = muted || silenced;
    let body = hw_tracks::region(
        g,
        rr,
        &hw_tracks::RegionPaint {
            material,
            look: look_of(track, region, material, selected),
            selected,
            muted,
            dim: silenced,
            recording,
            number,
            name: &region.name,
            lead: 0.0,
        },
    );
    let head = Rect::from_min_max(rr.min, pos2(rr.right(), body.top()));
    let head_ink = if selected || recording {
        theme::ON_ACCENT()
    } else if faded {
        theme::INK2()
    } else {
        theme::pick(theme::BONE(), theme::ON_DARK())
    };
    let gb = g.with_clip_rect(body.shrink2(vec2(1.0, 0.0)).intersect(g.clip_rect()));
    // Missing audio: a bare bone body with a hatch across it where the waveform would be.
    if region.missing.is_some() {
        let bare = body.shrink(1.0);
        gb.rect_filled(bare, 0.0, theme::BONE2());
        hw_tracks::hatch(&gb, bare, 1.5, 7.0, theme::INK().gamma_multiply(0.22));
    }
    // View > Colour Spectrogram: the sound's colour spectrogram fills the body inside the
    // outline instead of the waveform, once it is made (the waveform until then). Each
    // moment of the file is drawn where the region plays it, as the waveform is.
    let colour = region.missing.is_none()
        && !recording
        && crate::colour_spectrogram::on(g.ctx())
        && match (region.audio.as_ref(), span) {
            (Some(a), Some(sp)) if beat_px > 0.0 => {
                let edge = if selected { 2.0 } else { 1.0 };
                let screen = Rect::from_min_max(
                    pos2(rr.left() + edge, body.top()),
                    pos2(rr.right() - edge, body.bottom() - edge),
                );
                let tint = if faded {
                    Color32::from_gray(105)
                } else {
                    Color32::WHITE
                };
                let left = full.left();
                crate::colour_spectrogram::paint(
                    &gb,
                    screen,
                    &a.file,
                    |x| sp.file_secs(sp.secs_at(f64::from((x - left) / beat_px))),
                    tint,
                )
            }
            _ => false,
        };
    // Bar lines inside the region (faint light ones over a colour spectrogram).
    let bar_line = if colour {
        Color32::from_white_alpha(22)
    } else {
        material.grid()
    };
    for &x in bar_xs {
        if x > body.left() + 1.0 && x < body.right() - 1.0 {
            hw_tracks::time_line(&gb, x, body.y_range(), 1.0, bar_line);
        }
    }
    if let Some(generated) = &region.generated {
        if head.width() > 120.0 {
            draw_generated_mark(
                g,
                head,
                matches!(generated.source, crate::model::Generator::Drummer(_)),
                head_ink,
            );
        }
    }
    let inner = body.shrink2(vec2(0.0, 3.0));
    // A Flex edit: its mode at the right of the header and its markers over the waveform.
    if let Some(f) = region.audio.as_ref().and_then(|a| a.flex.as_ref()) {
        if head.width() > 140.0 {
            let mark = if f.pitch.is_empty() {
                crate::flex_edit::mode_label(f.mode).to_uppercase()
            } else {
                format!("{} ♪", crate::flex_edit::mode_label(f.mode).to_uppercase())
            };
            hw_tracks::label(
                g,
                head.right_center() - vec2(6.0, 0.0),
                Align2::RIGHT_CENTER,
                &mark,
                Face::Bold,
                9.5,
                0.06,
                head_ink,
            );
        }
        // A marker sits where the region plays its place in the file.
        for m in &f.markers {
            let Some(u) = span.and_then(|sp| sp.region_secs(m.dst)) else {
                continue;
            };
            let x = full.left() + span.map_or(0.0, |sp| sp.beats_at(u)) as f32 * beat_px;
            if x > inner.left() && x < inner.right() {
                gb.vline(
                    x,
                    inner.y_range(),
                    Stroke::new(1.0, theme::ORANGE_DK().gamma_multiply(0.85)),
                );
            }
        }
    }
    let ink = if recording {
        theme::ORANGE_HI()
    } else if colour {
        theme::ON_DARK().gamma_multiply(if faded { 0.5 } else { 0.9 })
    } else if faded {
        material.ink().gamma_multiply(0.45)
    } else {
        material.ink()
    };
    let edit = region.audio.is_some() || track.kind == TrackKind::Audio;
    if colour {
        // The picture is drawn.
    } else if let (Some(peaks), Some(span)) =
        (region.audio.as_ref().and_then(|a| a.peaks.get()), span)
    {
        let gain =
            10f32.powf(region.params.gain_db / 20.0) * crate::arrange_zoom::wave_zoom(g.ctx());
        draw_peaks(&gb, inner, (full.left(), beat_px), span, peaks, gain, ink);
    } else if region.audio.is_some() {
        gb.hline(
            inner.x_range(),
            inner.center().y,
            Stroke::new(1.0, ink.gamma_multiply(0.5)),
        );
    } else if track.kind == TrackKind::Audio {
        // An audio region without a file has nothing to show.
    } else if !region.notes.is_empty() {
        if material == Material::Drum {
            draw_drum_hits(&gb, inner, full.left(), beat_px, &region.notes, ink);
        } else {
            // Scaled to the notes the region still shows, so a shortened region fills its height.
            let (lo, hi) = pitch_range(visible_notes(region, full.width() / beat_px.max(1e-3)));
            draw_note_lines(
                &gb,
                inner,
                full.left(),
                beat_px,
                region.notes.iter(),
                lo,
                hi,
                ink,
            );
        }
    }
    if edit {
        draw_fades(g, rr, inner, full, region, (fades, under), beat_px, ink);
    }
}

/// A small mark at the right of a region's header that tells a Drummer region (a drum with
/// its sticks) from a Pattern region (a grid of steps).
fn draw_generated_mark(g: &Painter, head: Rect, drummer: bool, color: Color32) {
    let c = pos2(head.right() - 12.0, head.center().y);
    let stroke = Stroke::new(1.2, color);
    if drummer {
        g.rect_stroke(
            Rect::from_center_size(c + vec2(0.0, 1.5), vec2(8.0, 4.5)),
            1.0,
            stroke,
            StrokeKind::Middle,
        );
        g.line_segment([c + vec2(-4.0, -2.0), c + vec2(-1.0, -4.5)], stroke);
        g.line_segment([c + vec2(4.0, -2.0), c + vec2(1.0, -4.5)], stroke);
    } else {
        for row in 0..2 {
            for col in 0..3 {
                let at = c + vec2(col as f32 * 3.6 - 3.6, row as f32 * 3.6 - 1.8);
                g.rect_filled(Rect::from_center_size(at, vec2(2.6, 2.6)), 0.0, color);
            }
        }
    }
}

/// Shades what the region's fades take away: a triangle from the region's start up to where
/// the fade in ends, and one down to the region's end for the fade out, with the curve's line
/// in the region's print colour.
#[allow(clippy::too_many_arguments)]
fn draw_fades(
    g: &Painter,
    rr: Rect,
    inner: Rect,
    full: Rect,
    region: &Region,
    ((fin, fout), under): ((f32, f32), Option<crate::fade_curve::Under>),
    beat_px: f32,
    ink: Color32,
) {
    if fin <= 0.0 && fout <= 0.0 && under.is_none() {
        return;
    }
    let g = g.with_clip_rect(g.clip_rect().intersect(rr));
    let shade = Color32::from_rgba_unmultiplied(20, 20, 19, 90);
    let line = Stroke::new(1.5, ink);
    let curve = |x: f32| crate::fade_curve::level(region.params.fade_curve, x);
    // `x0` where the fade is silent, `x1` where it reaches full level.
    let fade = |x0: f32, x1: f32| {
        const STEPS: usize = 16;
        let pts: Vec<Pos2> = (0..=STEPS)
            .map(|k| {
                let t = k as f32 / STEPS as f32;
                let y = inner.bottom() - curve(t) * inner.height();
                pos2(x0 + (x1 - x0) * t, y)
            })
            .collect();
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            g.add(Shape::convex_polygon(
                vec![pos2(a.x, inner.top()), pos2(b.x, inner.top()), b, a],
                shade,
                Stroke::NONE,
            ));
        }
        g.add(Shape::line(pts, line));
    };
    if fin > 0.0 {
        fade(full.left(), full.left() + fin * beat_px);
    }
    if fout > 0.0 {
        fade(full.right(), full.right() - fout * beat_px);
    }
    // A crossfade: the earlier region's fade out plays under this one's fade in, from this
    // region's start or before it (`fade_curve::under`); its line crosses this one's, as both
    // are heard.
    if let Some((from, len, shape)) = under.filter(|u| u.1 > 0.0) {
        const STEPS: usize = 16;
        let x0 = full.left() + from * beat_px;
        let x1 = x0 + len * beat_px;
        let pts: Vec<Pos2> = (0..=STEPS)
            .map(|k| {
                let t = k as f32 / STEPS as f32;
                let y = inner.bottom() - crate::fade_curve::level(shape, 1.0 - t) * inner.height();
                pos2(x0 + (x1 - x0) * t, y)
            })
            .collect();
        g.add(Shape::line(pts, line));
    }
}

/// Draws a recorded waveform as the Hardware look prints it: ink columns 1.7 points wide
/// every 3 points and a faint centre line. `x0` is the region's left edge on screen and
/// `beat_px` points per quarter note; `span` says which part of the file's overview `peaks`
/// each column shows, so a cut, trimmed or reversed region shows what it plays. `gain` is
/// the region gain (linear).
fn draw_peaks(
    g: &Painter,
    body: Rect,
    (x0, beat_px): (f32, f32),
    span: &AudioSpan,
    peaks: &[(f32, f32)],
    gain: f32,
    col: Color32,
) {
    if peaks.is_empty() || beat_px <= 0.0 {
        return;
    }
    let mid = body.center().y;
    let half = body.height() * 0.48;
    g.hline(
        body.x_range(),
        mid,
        Stroke::new(0.6, col.gamma_multiply(0.4)),
    );
    const PITCH: f32 = 3.0;
    let secs_at_x = |x: f32| span.secs_at(f64::from((x - x0) / beat_px));
    // Columns on a grid that moves with the region, so they do not shimmer while scrolling.
    let mut x = x0 + ((body.left() - x0) / PITCH).floor() * PITCH;
    while x < body.right() {
        if x + PITCH < body.left() {
            x += PITCH;
            continue;
        }
        let u0 = secs_at_x(x);
        if u0 >= span.len {
            break;
        }
        let Some((lo, hi)) = span.peak_range(peaks, u0, secs_at_x(x + PITCH)) else {
            x += PITCH;
            continue;
        };
        let (lo, hi) = (lo * gain, hi * gain);
        let (top, bottom) = (mid - hi.min(1.0) * half, mid - lo.max(-1.0) * half);
        let cx = x + PITCH / 2.0;
        g.line_segment(
            [
                pos2(cx, top.min(mid - 0.6)),
                pos2(cx, bottom.max(mid + 0.6)),
            ],
            Stroke::new(1.7, col),
        );
        x += PITCH;
    }
}

/// A folder's M and S keys: they set the same state on every track inside the folder.
fn folder_buttons(
    ui: &Ui,
    g: &Painter,
    p: &mut Project,
    i: usize,
    (x, y, size, pitch): (f32, f32, Vec2, f32),
) {
    let (mute, solo) = (p.tracks[i].mute, p.tracks[i].solo);
    let m = Rect::from_min_size(pos2(x, y), size);
    if header_button(ui, g, m, (i, "m"), "M", mute) {
        p.set_folder_flags(i, Some(!mute), None);
    }
    let s = Rect::from_min_size(pos2(x + pitch, y), size);
    if header_button(ui, g, s, (i, "s"), "S", solo) {
        p.set_folder_flags(i, None, Some(!solo));
    }
}

/// The header's M/S/R/I keys (4.7): 27×19 on a 31-point pitch.
const KEY_W: f32 = 27.0;
const KEY_H: f32 = 19.0;
const KEY_PITCH: f32 = 31.0;
/// The header's volume slider (4.9): an 80-point groove.
const SLIDER_W: f32 = 80.0;

/// Where 0 dB sits on a track header's volume slider (Logic puts unity about two thirds up).
const SLIDER_UNITY: f32 = 0.65;

/// Slider position (0..1) for a volume in dB (-60..+6): most of the travel is below unity,
/// the last third is the boost to +6 dB.
fn db_to_slider(db: f32) -> f32 {
    let db = db.clamp(-60.0, 6.0);
    if db >= 0.0 {
        SLIDER_UNITY + (1.0 - SLIDER_UNITY) * db / 6.0
    } else {
        SLIDER_UNITY * ((db + 60.0) / 60.0).sqrt()
    }
}

fn slider_to_db(f: f32) -> f32 {
    let f = f.clamp(0.0, 1.0);
    if f >= SLIDER_UNITY {
        (f - SLIDER_UNITY) / (1.0 - SLIDER_UNITY) * 6.0
    } else {
        -60.0 + 60.0 * (f / SLIDER_UNITY).powi(2)
    }
}

/// Where a track header's parts sit (4.2): the number key and the type well, both square and
/// as tall as the row allows (46 points at the default height, never under 24), the name to
/// their right with the M/S/R/I keys under it, and the volume slider and pan knob on the
/// right. On a short row the keys move into a line before the slider, at 20×17.
#[derive(Debug)]
struct HeaderLayout {
    pad: Rect,
    well: Rect,
    name_x: f32,
    /// Centre line of the name.
    name_y: f32,
    /// Top of the M/S/R/I keys under the name; `None` on a short row.
    keys_y: Option<f32>,
    /// Top of a short row's keys, in a line ending before the slider.
    inline_keys_y: Option<f32>,
    slider: Rect,
    pan: Pos2,
}

/// A short row's M/S/R/I keys, and their pitch.
const SMALL_KEY: Vec2 = vec2(20.0, 17.0);
const SMALL_KEY_PITCH: f32 = 22.0;

fn header_layout(hdr: Rect, indent: f32) -> HeaderLayout {
    let h = hdr.height();
    let side = (h - 24.0).clamp(24.0, 46.0).min(h - 4.0);
    let top = hdr.center().y - side / 2.0;
    let pad = Rect::from_min_size(pos2(hdr.left() + NUM_W, top), vec2(side, side));
    let well = Rect::from_min_size(pos2(pad.right() + 10.0 + indent, top), vec2(side, side));
    let compact = h < 52.0;
    HeaderLayout {
        pad,
        well,
        name_x: well.right() + 14.0,
        name_y: if compact { hdr.center().y } else { top + 9.0 },
        keys_y: (!compact).then_some(pad.bottom() - 15.0),
        inline_keys_y: (compact && h >= SMALL_KEY.y + 6.0)
            .then_some(hdr.center().y - SMALL_KEY.y / 2.0 - 1.0),
        slider: Rect::from_min_size(
            pos2(hdr.right() - 148.0, hdr.center().y - 2.0),
            vec2(SLIDER_W, 4.0),
        ),
        pan: pos2(hdr.right() - 34.0, hdr.center().y),
    }
}

/// The type code printed after a track's keys (4.7).
fn type_code(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Instrument => "IN",
        TrackKind::Audio => "AU",
        TrackKind::Drummer => "DR",
        TrackKind::Bus => "BU",
        TrackKind::Output => "ST",
        TrackKind::Aux => "AX",
        TrackKind::Folder => "FD",
        TrackKind::Vca => "VC",
        TrackKind::ExternalMidi => "EX",
    }
}

/// Draws track `i`'s header (`selected`: drawn as a selected track) and returns the response
/// of its background: the caller selects the track on a click (`track_selection.rs`), and
/// opens the right-click menu; the keys on it handle their own clicks.
#[allow(clippy::too_many_arguments)]
fn header(
    ui: &Ui,
    g: &Painter,
    p: &mut Project,
    i: usize,
    hdr: Rect,
    peaks: (f32, f32),
    silenced: bool,
    hide_view: bool,
    selected: bool,
) -> egui::Response {
    // Show Hidden Tracks: a hidden track's name is printed dim, as a silenced one's is.
    let dim_name = silenced || hide_view && crate::track_commands::hidden(p, i);
    // Track > Configure Track Header: the parts the user hid are not drawn (their space stays).
    let (show_number, show_icon, show_name, show_buttons, show_volume, show_pan) = (
        !p.header_hidden.contains(&HeaderPart::Number),
        !p.header_hidden.contains(&HeaderPart::Icon),
        !p.header_hidden.contains(&HeaderPart::Name),
        !p.header_hidden.contains(&HeaderPart::Buttons),
        !p.header_hidden.contains(&HeaderPart::Volume),
        !p.header_hidden.contains(&HeaderPart::Pan),
    );
    hw_tracks::header_bg(g, hdr, selected);
    let resp = ui.interact(hdr, id(("hdr", i)), Sense::click());

    let indent = f32::from(p.tracks[i].indent) * INDENT_W;
    let folder = p.tracks[i].kind == TrackKind::Folder;
    let lay = header_layout(hdr, indent);
    let pointer = ui.input(|inp| inp.pointer.hover_pos());
    if show_number {
        // The number key: pressing it selects the track (it is part of the header's click).
        let over = pointer.is_some_and(|pt| lay.pad.contains(pt));
        hw_tracks::number_pad(g, lay.pad, i + 1, selected, over && resp.hovered());
    }
    if show_icon {
        hw_tracks::well(g, lay.well, 4.0);
        let s = (lay.well.width() * 0.55).clamp(14.0, 26.0);
        draw_icon(
            g,
            Rect::from_center_size(lay.well.center(), vec2(s, s)),
            track_icon(&p.tracks[i]),
        );
    }

    let mut name_x = lay.name_x;
    if folder {
        // Fold arrow: click to hide or show the tracks inside.
        let arrow = Rect::from_center_size(pos2(name_x + 5.0, lay.name_y), vec2(16.0, 22.0));
        if ui
            .interact(arrow, id(("fold", i)), Sense::click())
            .clicked()
        {
            p.tracks[i].folded = !p.tracks[i].folded;
        }
        hw_tracks::disclosure(g, arrow.center(), !p.tracks[i].folded, theme::INK2());
        name_x += 18.0;
    }
    // A short row's keys sit in a line before the slider (and its automation LED).
    let inline_n = {
        let t = &p.tracks[i];
        let hide_key = usize::from(hide_view);
        if folder {
            2 + hide_key
        } else {
            2 + usize::from(t.kind.has_regions())
                + usize::from(t.kind == TrackKind::Audio)
                + hide_key
        }
    };
    let inline_x = lay.slider.left() - 18.0 - inline_n as f32 * SMALL_KEY_PITCH;
    // Where the keys go: under the name, or in a line before the slider.
    let key_row = match (lay.keys_y, lay.inline_keys_y, show_buttons) {
        (Some(y), _, true) => Some((name_x, y, vec2(KEY_W, KEY_H), KEY_PITCH)),
        (None, Some(y), true) => Some((inline_x, y, SMALL_KEY, SMALL_KEY_PITCH)),
        _ => None,
    };
    let name_end = if lay.keys_y.is_none() && key_row.is_some() {
        inline_x
    } else {
        lay.slider.left()
    };
    let name_w = (name_end - 8.0 - name_x).max(20.0);
    if show_name {
        // Double-click the name to rename the track: Return or clicking away keeps it, Escape
        // leaves it as it was.
        let name_rect = Rect::from_min_size(
            pos2(name_x - 3.0, lay.name_y - 11.0),
            vec2(name_w + 3.0, 22.0),
        );
        let key = id(("rename", i));
        let name_now = p.tracks[i].name.clone();
        if let Some(name) = crate::widgets::rename_field(
            ui,
            &resp,
            name_rect,
            key,
            &name_now,
            theme::font(Face::Silk, 16.0),
        ) {
            p.tracks[i].name = name;
        }
        if !crate::widgets::renaming(ui, key) {
            // 16-point caps; a long name steps down to 14 before it is cut.
            let upper = name_now.to_uppercase();
            let width = |size: f32| {
                hw_tracks::galley(g, &upper, Face::Silk, size, 0.09, theme::INK())
                    .size()
                    .x
                    - 0.09 * size
            };
            let size = if width(16.0) <= name_w { 16.0 } else { 14.0 };
            let shown = hw_tracks::fit(g, &upper, Face::Silk, size, 0.09, name_w);
            hw_tracks::label(
                g,
                pos2(name_x, lay.name_y),
                Align2::LEFT_CENTER,
                &shown,
                Face::Silk,
                size,
                0.09,
                if dim_name {
                    theme::PRINT()
                } else {
                    theme::INK()
                },
            );
        }
    }

    // A folder only has M and S, which act on everything inside it, and no volume or pan.
    if folder {
        if let Some(row) = key_row {
            folder_buttons(ui, g, p, i, row);
            let (x, y, size, pitch) = row;
            if hide_view {
                hide_button(
                    ui,
                    g,
                    p,
                    i,
                    Rect::from_min_size(pos2(x + 2.0 * pitch, y), size),
                );
            }
        }
        return resp;
    }

    // M S R (I) keys under the name (short tracks: hidden, like Logic at small heights).
    // Mute, the slider and the pan show what their envelopes play (Live), with an LED; one
    // moved by hand overrides its envelope (see `automation::tick`).
    let audio = p.tracks[i].kind == TrackKind::Audio;
    let beat = p.position_beats;
    let shown = |t: &Track, target| crate::automation::shown_value(t, target, beat);
    let t = &mut p.tracks[i];
    let (muted, vol_shown, pan_shown) = (
        shown(t, AutoTarget::Mute) >= 0.5,
        shown(t, AutoTarget::Volume),
        shown(t, AutoTarget::Pan),
    );
    if let Some((x0, by, size, pitch)) = key_row {
        let mut bx = x0;
        let at = |x: f32| Rect::from_min_size(pos2(x, by), size);
        if header_button(ui, g, at(bx), (i, "m"), "M", muted) {
            t.mute = !muted;
        }
        crate::automation_ui::led(
            g,
            pos2(bx + size.x - 3.0, by + 3.0),
            &t.auto,
            AutoTarget::Mute,
        );
        bx += pitch;
        if header_button(ui, g, at(bx), (i, "s"), "S", t.solo) {
            t.solo = !t.solo;
        }
        bx += pitch;
        // Aux and Folder tracks hold no regions, so there is nothing to record into.
        if t.kind.has_regions() {
            if header_button(ui, g, at(bx), (i, "r"), "R", t.rec) {
                t.rec = !t.rec;
            }
            bx += pitch;
        }
        if audio {
            if header_button(ui, g, at(bx), (i, "i"), "I", t.monitor) {
                t.monitor = !t.monitor;
            }
            bx += pitch;
        }
        // Track > Show Hidden Tracks: the H key hides the track or shows it again.
        if hide_view {
            if header_button(ui, g, at(bx), (i, "h"), "H", t.hidden) {
                t.hidden = !t.hidden;
            }
            bx += pitch;
        }
        let code_room = lay.slider.left() - 12.0 - bx;
        if lay.keys_y.is_some() && code_room > 16.0 {
            hw_tracks::label(
                g,
                pos2(bx + 3.0, by + size.y / 2.0 + 1.0),
                Align2::LEFT_CENTER,
                type_code(t.kind),
                Face::Lcd,
                10.0,
                0.1,
                theme::PRINT(),
            );
        }
    }

    if show_volume {
        // Volume slider (dB, -60..+6; double-click resets to 0 dB). Its groove lights up to the
        // track's level, a meter in the slot.
        let sl = lay.slider;
        let resp = ui.interact(
            sl.expand2(vec2(8.0, 11.0)),
            id(("vol", i)),
            Sense::click_and_drag(),
        );
        if let Some(pt) = resp.interact_pointer_pos() {
            let f = (pt.x - sl.left()) / sl.width();
            t.volume_db = slider_to_db(f.clamp(0.0, 1.0));
        }
        if resp.double_clicked() {
            t.volume_db = 0.0;
        }
        let silent = t.mute || silenced;
        let levels = [peaks.0, peaks.1].map(|pk| {
            let db = crate::mixer::lin_to_db(pk);
            if db <= -60.0 {
                0.0
            } else {
                db_to_slider(db)
            }
        });
        hw_tracks::slider(
            g,
            sl,
            db_to_slider(vol_shown),
            Some((&levels, &slider_to_db)),
            silent,
        );
        // Its automation LED sits over the slider's left end, clear of the cap (as the pan's
        // sits at the knob's corner), not beside the slider.
        crate::automation_ui::led(
            g,
            pos2(sl.left() + 3.0, sl.center().y - 15.0),
            &t.auto,
            AutoTarget::Volume,
        );
    }

    if show_pan {
        let kc = lay.pan;
        let d = (hdr.height() * 0.4).clamp(20.0, 28.0);
        let kresp = ui.interact(
            Rect::from_center_size(kc, vec2(d + 6.0, d + 6.0)),
            id(("pan", i)),
            Sense::click_and_drag(),
        );
        if kresp.dragged() {
            t.pan = (pan_shown - kresp.drag_delta().y * 0.01).clamp(-1.0, 1.0);
        }
        if kresp.double_clicked() {
            t.pan = 0.0;
        }
        hw_tracks::knob(g, kc, d, pan_shown, false);
        crate::automation_ui::led(
            g,
            kc + vec2(d / 2.0 + 2.0, -d / 2.0),
            &t.auto,
            AutoTarget::Pan,
        );
    }
    resp
}

// ------------------------------------------------------------------ main view

/// Height of the Master strip (Track > Show Master Track).
const MASTER_STRIP_H: f32 = 56.0;
/// The name strip at the top of a region: a click there takes the region in Automation Mode.
const REGION_TITLE_H: f32 = 15.0;

/// The gap between a track row's edges and its regions (4 points at the default height).
fn region_inset(row_h: f32) -> f32 {
    (row_h * 0.06).clamp(1.0, 4.0).round()
}

/// The Master strip, a track row of its own: in the track list its plate (black, an orange
/// "ST" chip and an orange underline), the master level (the transport's: a meter over a
/// fader) and VOL / PEAK readouts; the arrange side is floor.
fn master_strip(ui: &mut Ui, p: &mut Project, rect: Rect, meters: &crate::mixer::Meters) {
    let g = ui.painter_at(rect);
    let head = Rect::from_min_max(rect.min, pos2(rect.left() + HEADER_W, rect.bottom()));
    let lane = Rect::from_min_max(pos2(head.right(), rect.top()), rect.max);
    g.rect_filled(lane, 0.0, hw_tracks::FLOOR());
    hw_tracks::header_bg(&g, head, false);
    hw_tracks::seam_wide_v(&g, head.right(), head.y_range());
    hw::seam_h(&g, rect.x_range(), rect.top());
    let plate = Rect::from_min_size(
        pos2(head.left() + NUM_W, rect.center().y - 13.0),
        vec2(118.0, 26.0),
    );
    hw_tracks::drop_shadow(&g, plate, 3.0, 2.0, 40);
    hw::gradient(
        &g,
        plate,
        3.0,
        Color32::from_rgb(0x2d, 0x2b, 0x28),
        Color32::from_rgb(0x16, 0x15, 0x14),
    );
    g.rect_filled(
        Rect::from_min_max(
            pos2(plate.left(), plate.bottom() - 3.0),
            plate.right_bottom(),
        ),
        egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: 3,
            se: 3,
        },
        theme::ORANGE(),
    );
    let chip = Rect::from_min_size(
        pos2(plate.left() + 6.0, plate.center().y - 7.5),
        vec2(20.0, 13.0),
    );
    g.rect_filled(chip, 2.0, theme::ORANGE());
    hw_tracks::label(
        &g,
        chip.center(),
        Align2::CENTER_CENTER,
        "ST",
        Face::Lcd,
        9.5,
        0.06,
        theme::ON_ACCENT(),
    );
    hw_tracks::label(
        &g,
        pos2(chip.right() + 8.0, chip.center().y),
        Align2::LEFT_CENTER,
        &tr("Master").to_uppercase(),
        Face::Silk,
        14.0,
        0.08,
        theme::pick(theme::BONE(), theme::ON_DARK()),
    );
    // VOL and PEAK at the right of the track list, the level control between.
    let ro = Rect::from_min_size(
        pos2(head.right() - NUM_W - 112.0, rect.center().y - 15.0),
        vec2(54.0, 30.0),
    );
    let pk = ro.translate(vec2(58.0, 0.0));
    let db = |a: f32| 20.0 * a.max(1e-6).log10();
    let num = |d: f32| {
        if d <= -60.0 {
            "-INF".to_string()
        } else {
            format!("{d:.1}")
        }
    };
    hw::lcd_readout(&g, ro, "VOL", &num(db(p.master_vol)));
    hw::lcd_readout(
        &g,
        pk,
        "PEAK",
        &num(db(meters.master.0.max(meters.master.1))),
    );
    let level = Rect::from_min_max(
        pos2(plate.right() + 14.0, rect.center().y - 20.0),
        pos2(ro.left() - 12.0, rect.center().y + 20.0),
    );
    crate::transport::master_volume(
        ui,
        &g,
        p,
        level,
        Some(meters.master),
        id(("tracks", "master")),
    );
}

pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState, meters: &crate::mixer::Meters) {
    // A window over the app (a dialog, the chooser, the settings, the bounce window) has the
    // keys: the Tracks area's own keys wait.
    let window_open = s.track_dialog.is_some()
        || s.chooser.is_some()
        || s.settings_tab.is_some()
        || crate::key_commands::bounce_dialog_open(s);
    // Regions removed or moved by an undo or a track change are no longer selected.
    s.tracks.prune(p);
    let rec_targets: Vec<(usize, usize)> = s
        .rec
        .target()
        .into_iter()
        .chain(s.audio_rec.regions())
        .collect();
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    // Track > Show Master Track: a strip along the bottom, outside the scrolling tracks.
    // In Automation Mode it has a lane too: the Stereo Out's fader or an effect's parameter.
    let master_lane_h = if s.auto.mode {
        crate::automation_ui::LANE_H
    } else {
        0.0
    };
    let master_h = if p.show_master_track {
        MASTER_STRIP_H + master_lane_h
    } else {
        0.0
    };
    let area = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - master_h));
    let mut master_lane = None;
    if p.show_master_track {
        let strip = Rect::from_min_max(
            pos2(full.left(), area.bottom()),
            pos2(full.right(), area.bottom() + MASTER_STRIP_H),
        );
        master_strip(ui, p, strip, meters);
        if master_lane_h > 0.0 {
            let top = strip.bottom();
            let head = Rect::from_min_max(
                pos2(full.left(), top),
                pos2(full.left() + HEADER_W, full.bottom()),
            );
            let lane = Rect::from_min_max(pos2(head.right(), top), full.max);
            let g = ui.painter_at(head);
            hw_tracks::header_bg(&g, head, false);
            let cr = Rect::from_min_size(
                pos2(head.left() + NUM_W, head.top() + 4.0),
                vec2(HEADER_W - 2.0 * NUM_W, crate::automation_ui::CHOOSER_H),
            );
            let strip_key = crate::automation::Strip::Master;
            crate::automation_ui::choosers(ui, &g, p, &mut s.auto, strip_key, cr, None);
            master_lane = Some(lane);
        }
    }
    let tb = Rect::from_min_size(area.min, vec2(area.width(), TOOLBAR_H));
    let ruler_y = tb.bottom();
    // Global tracks between the ruler and the tracks: Marker, then Tempo, as in Logic.
    let marker_h = if s.tracks.show_markers {
        crate::marker_track::HEIGHT
    } else {
        0.0
    };
    let tempo_h = if s.tracks.show_tempo {
        crate::tempo_track::HEIGHT
    } else {
        0.0
    };
    let signature_h = if s.tracks.show_signatures {
        crate::signature_track::HEIGHT
    } else {
        0.0
    };
    let tracks_y = ruler_y + RULER_H + marker_h + signature_h + tempo_h;
    let headers = Rect::from_min_max(
        pos2(area.left(), tracks_y),
        pos2(area.left() + HEADER_W, area.bottom()),
    );
    let lanes = Rect::from_min_max(pos2(area.left() + HEADER_W, tracks_y), area.max);
    let ruler = Rect::from_min_max(
        pos2(lanes.left(), ruler_y),
        pos2(area.right(), ruler_y + RULER_H),
    );
    let corner = Rect::from_min_max(
        pos2(area.left(), ruler_y),
        pos2(headers.right(), ruler.bottom()),
    );

    // The Tool menu (T) takes its keys before anything else here looks at them.
    if let Some(at) = s.tracks.tool_menu {
        let (picked, open) = crate::piano_tools::tool_menu(ui.ctx(), "tracks", at, s.tracks.tool);
        if let Some(t) = picked {
            s.tracks.tool = t;
        }
        if !open {
            s.tracks.tool_menu = None;
        }
    }
    toolbar(ui, p, s, tb, lanes.width());

    // Zoom is counted in pixels per bar of the first bar's length.
    let bpb = p.bar_len(0);
    let v = &mut s.tracks;
    v.lanes_w = lanes.width();
    v.zoom.lanes_h = lanes.height();
    crate::arrange_zoom::publish_wave_zoom(ui.ctx(), v);
    crate::colour_spectrogram::publish(ui.ctx(), v.colour_spectra);

    // ---- wheel: scroll; Shift = horizontal; the zooms (Command, Option, pinch) are in
    // `arrange_zoom::wheel_zoom`
    let hover = ui.input(|i| i.pointer.hover_pos());
    if let Some(pt) =
        hover.filter(|&pt| lanes.contains(pt) || headers.contains(pt) || ruler.contains(pt))
    {
        let (delta, mods, zoom) =
            ui.input(|i| (i.smooth_scroll_delta, i.modifiers, i.zoom_delta()));
        if crate::arrange_zoom::wheel_zoom(p, v, lanes, pt, (delta, zoom, mods)) {
        } else if mods.shift {
            v.scroll_x -= delta.y + delta.x;
        } else {
            v.scroll_x -= delta.x;
            v.scroll_y -= delta.y;
        }
    }
    let auto_v = &s.auto;
    let hide_view = v.show_hidden;
    let rows = row_layout_with(p, v.row_h, hide_view, |t| {
        crate::automation_ui::chooser_height(auto_v, t)
            + crate::automation_ui::lanes_height(auto_v, t)
    });
    let content_h = rows.last().map_or(0.0, |(top, h)| top + h);
    v.scroll_y = v
        .scroll_y
        .clamp(0.0, (content_h - lanes.height() + 40.0).max(0.0));
    // As far right as a project can reach (Logic: 9999 bars).
    v.scroll_x = v
        .scroll_x
        .clamp(0.0, crate::model::MAX_SIG_BAR as f32 * v.bar_w);

    // The tool the pointer has: the Command-click tool while Command is held, and while a
    // button is down the one it went down with.
    // Control-Option held: the Zoom tool, whatever tool is chosen (Logic).
    let (command, mods) = ui.input(|i| (i.modifiers.command, i.modifiers));
    let pointer_tool = crate::arrange_zoom::pressed_tool(mods).unwrap_or(v.tool_for(command));
    if v.held_tool.is_none() && ui.input(|i| i.pointer.any_pressed()) {
        v.held_tool = Some(pointer_tool);
    }
    let tool = v.held_tool.unwrap_or(pointer_tool);
    // The Zoom tool: a rectangle to zoom to, or a click to zoom in or out (`arrange_zoom`).
    let zoom_band = crate::arrange_zoom::zoom_tool(ui, p, s, lanes, tool);
    let v = &mut s.tracks;
    let beat_px = v.bar_w / bpb as f32;
    // Scroll in Play: the playhead stays in the middle and the song moves; else Catch: page
    // the view when the playhead leaves it.
    if let Some(x) = crate::arrange_zoom::scroll_in_play(p, v, beat_px) {
        v.scroll_x = x;
    } else if v.catch && p.playing {
        let px = p.position_beats as f32 * beat_px - v.scroll_x;
        if px > lanes.width() - 30.0 || px < 0.0 {
            v.scroll_x = (p.position_beats as f32 * beat_px - lanes.width() * 0.1).max(0.0);
        }
    }
    let (scroll_x, scroll_y) = (v.scroll_x, v.scroll_y);
    // The one way from a time to the screen: on a physical pixel boundary, for the grid lines,
    // the regions, the cycle, the markers and the selection alike (see `hw_tracks::pixel_edge`).
    let ppp = ui.ctx().pixels_per_point();
    let x_of =
        |beat: f64| hw_tracks::pixel_edge(lanes.left() + beat as f32 * beat_px - scroll_x, ppp);
    // A track row's top on screen, on a pixel boundary as well.
    let row_y = |top: f32| hw_tracks::pixel_edge(lanes.top() + top - scroll_y, ppp);
    let beat_of = |x: f32| ((x - lanes.left() + scroll_x) / beat_px).max(0.0) as f64;

    // ---- lanes background + grid
    let g = ui.painter_at(lanes);
    g.rect_filled(lanes, 0.0, hw_tracks::FLOOR());
    let bars = p.bars_in(beat_of(lanes.left()), beat_of(lanes.right()));
    for (i, &(top, row_h)) in rows.iter().enumerate() {
        if row_h <= 0.0 {
            continue;
        }
        let y = row_y(top);
        let row = Rect::from_min_size(pos2(lanes.left(), y), vec2(lanes.width(), row_h));
        if crate::track_selection::is_selected(p, v, i) {
            g.rect_filled(row, 0.0, hw_tracks::FLOOR_SEL());
        }
        if p.tracks[i].kind == TrackKind::Folder {
            g.rect_filled(row, 0.0, Color32::from_white_alpha(18));
        }
        g.hline(
            row.x_range(),
            row.bottom() - 0.5,
            Stroke::new(1.0, hw_tracks::LANE_LINE()),
        );
    }
    // Bar lines at 22 % ink, beats at 7 %: each fills the pixel column where its time begins.
    for bar in &bars {
        let x = x_of(bar.start);
        hw_tracks::time_line(&g, x, lanes.y_range(), 1.0, hw_tracks::BAR_LINE());
        if beat_px * bar.unit as f32 >= 14.0 {
            for k in 1..bar.num {
                let xb = x_of(bar.start + k as f64 * bar.unit);
                hw_tracks::time_line(&g, xb, lanes.y_range(), 1.0, hw_tracks::BEAT_LINE());
            }
        }
    }
    // The cycle tints the lanes it spans.
    if p.cycle {
        let band = Rect::from_min_max(
            pos2(x_of(p.cycle_start).max(lanes.left()), lanes.top()),
            pos2(x_of(p.cycle_end).min(lanes.right()), lanes.bottom()),
        );
        if band.width() > 0.0 {
            g.rect_filled(band, 0.0, hw_tracks::CYCLE_TINT());
        }
    }
    let bar_xs: Vec<f32> = bars.iter().map(|b| x_of(b.start)).collect();

    // Click on an empty lane: select that track, deselect regions.
    // Drag over the background (Pointer tool): a rubber band that selects what it touches.
    let lane_resp = ui.interact(lanes, id("lanes"), Sense::click_and_drag());
    let mut picked: Option<CtxAction> = None;
    let adding = ui.input(|i| i.modifiers.shift);
    if lane_resp.clicked() || context_click(&lane_resp) {
        if let Some(pt) = lane_resp.interact_pointer_pos() {
            if let Some(row) = row_at(&rows, pt.y - lanes.top() + scroll_y) {
                p.selected_track = row;
            }
        }
        // Shift-click on the background keeps the selection (a slip while adding to it); a
        // Zoom tool click only zooms.
        if !(adding && lane_resp.clicked()) && tool != TrackTool::Zoom {
            v.clear_selection();
        }
    }
    // The Pencil on an empty stretch of a track: a new MIDI region there (made below, once
    // the snap is known).
    let mut pencil_at: Option<(usize, f64)> = None;
    if lane_resp.clicked() && tool == TrackTool::Pencil {
        if let Some(pt) = lane_resp.interact_pointer_pos() {
            if let Some(row) = row_at(&rows, pt.y - lanes.top() + scroll_y) {
                pencil_at = Some((row, beat_of(pt.x)));
            }
        }
    }
    if lane_resp.drag_started_by(egui::PointerButton::Primary) && tool == TrackTool::Pointer {
        let from = ui
            .input(|i| i.pointer.press_origin())
            .or(lane_resp.interact_pointer_pos());
        if let Some(pt) = from {
            v.marquee = Some(Marquee {
                anchor: (beat_of(pt.x), pt.y - lanes.top() + scroll_y),
                base: if adding { v.selection() } else { Vec::new() },
            });
        }
    }
    // The band on screen (drawn over the regions, below).
    let mut band: Option<Rect> = None;
    let banding = lane_resp.dragged_by(egui::PointerButton::Primary);
    if let (true, Some(anchor), Some(pt)) = (
        banding,
        v.marquee.as_ref().map(|m| m.anchor),
        lane_resp.interact_pointer_pos(),
    ) {
        let (b0, b1) = (anchor.0.min(beat_of(pt.x)), anchor.0.max(beat_of(pt.x)));
        let y_now = pt.y - lanes.top() + scroll_y;
        let (y0, y1) = (anchor.1.min(y_now), anchor.1.max(y_now));
        let mut touched = Vec::new();
        for (ti, &(top, row_h)) in rows.iter().enumerate() {
            let track = &p.tracks[ti];
            if row_h <= 0.0 {
                continue;
            }
            let base_h = row_h
                - take_rows_height(track)
                - crate::automation_ui::lanes_height(&s.auto, track);
            let inset = region_inset(base_h);
            if top + base_h - inset < y0 || top + inset > y1 {
                continue;
            }
            for (ri, region) in track.regions.iter().enumerate() {
                if p.region_start_beats(region) < b1.max(b0 + 1e-9)
                    && p.region_end_beats(region) > b0
                {
                    touched.push((ti, ri));
                }
            }
        }
        let base = v.marquee.as_ref().map_or(&[][..], |m| &m.base);
        let picked_now = band_selection(base, &touched);
        v.select_many(&picked_now);
        band = Some(Rect::from_min_max(
            pos2(x_of(b0), lanes.top() + y0 - scroll_y),
            pos2(x_of(b1), lanes.top() + y1 - scroll_y),
        ));
    }
    if !banding {
        v.marquee = None;
    }
    band = band.or(zoom_band);
    // Double-click on an empty stretch of a Drummer track: a Drummer region there, as in Logic.
    if lane_resp.double_clicked() {
        if let Some(pt) = lane_resp.interact_pointer_pos() {
            let drummer_row = row_at(&rows, pt.y - lanes.top() + scroll_y)
                .filter(|row| p.tracks[*row].kind == TrackKind::Drummer);
            if let Some(row) = drummer_row {
                let bar = p.beats_to_bar(beat_of(pt.x)).floor() as f32 + 1.0;
                if let Some(ri) = p.add_generated_region(row, true, bar) {
                    p.selected_track = row;
                    v.select_only(row, ri);
                }
            }
        }
    }
    context_menu(&lane_resp, |ui| {
        new_track_items(ui, &mut picked);
        ui.separator();
        if ui.button(tr("Zoom to Fit")).clicked() {
            picked = Some(CtxAction::ZoomToFit);
        }
    });

    // ---- regions
    // The Snap mode, Snap to Grid and the division (`snap_grid`).
    let unit = p.beat_unit_at(beat_of(lanes.left()));
    let step = v.grid.step(v.snap, beat_px, bpb, unit);
    // Snapping to bars follows the signature, so it goes to the nearest bar line.
    let snap_bars = v.grid.snaps_to_bars(v.snap, beat_px, unit);
    let sig_owned = p.sigs().to_owned();
    let sigs_down = sig_owned.clone();
    let snap_pos = move |b: f64| -> f64 {
        if snap_bars {
            sig_owned.as_ref().nearest_bar_line(b)
        } else {
            snap_to(b, step)
        }
    };
    // The grid line at or before `b` (where the Pencil puts a region's start).
    let snap_down = move |b: f64| -> f64 {
        if snap_bars {
            let sigs = sigs_down.as_ref();
            sigs.bar_to_beats((sigs.beats_to_bar(b) + 1e-9).floor())
        } else {
            (b / step + 1e-9).floor() * step
        }
    };
    if let Some((row, at)) = pencil_at {
        if let Some(ri) = crate::region_edit::pencil_region(p, row, snap_down(at).max(0.0)) {
            p.selected_track = row;
            v.select_only(row, ri);
        }
    }
    // Where the Scissors would cut the region under the pointer (drawn over the regions).
    let mut cut_preview: Option<(f32, Rect)> = None;
    // Seconds against beats, for drawing what audio regions play.
    let tempo_map = p.tempo_map();
    // The repeats of looped regions, ghosted and under every region, each with a mark where it
    // begins (Logic: Loop).
    for (ti, &(top, row_h)) in rows.iter().enumerate() {
        let track = &p.tracks[ti];
        if row_h <= 0.0 || track.regions.iter().all(|r| r.params.loop_beats <= 0.0) {
            continue;
        }
        let y = row_y(top);
        let base_h =
            row_h - take_rows_height(track) - crate::automation_ui::lanes_height(&s.auto, track);
        let inset = region_inset(base_h);
        for (ri, region) in track.regions.iter().enumerate() {
            let len = p.region_len_beats(region);
            for (at, span) in p.loop_repeats(region) {
                let full = Rect::from_min_max(
                    pos2(x_of(at), y + inset),
                    pos2(x_of(at + len), y + base_h - inset),
                );
                let shown = Rect::from_min_max(full.min, pos2(x_of(at + span), full.bottom()))
                    .intersect(lanes);
                if shown.width() <= 0.0 || shown.height() <= 0.0 {
                    continue;
                }
                draw_region(
                    &g.with_clip_rect(shown),
                    full,
                    full,
                    track,
                    ti + 1,
                    region,
                    p.effective_fades(ti, ri),
                    None,
                    beat_px,
                    false,
                    false,
                    true,
                    &bar_xs,
                    AudioSpan::of(p, &tempo_map, region).as_ref(),
                );
                crate::region_loop::repeat_mark(
                    &g.with_clip_rect(lanes),
                    full.left(),
                    full.top(),
                    full.bottom(),
                );
            }
        }
    }
    let mut moved = false;
    let mut tool_act: Option<ToolAct> = None;
    // The Marquee tool's press, drag and release, on the background or on a region.
    let mut mq = MarqueeInput::default();
    if tool == TrackTool::Marquee {
        mq.read(ui, &lane_resp);
    } else {
        mq.other |= lane_resp.clicked() || lane_resp.drag_started_by(egui::PointerButton::Primary);
    }
    // The Solo tool pressed on a region: (track, region, where).
    let mut solo_press: Option<(usize, usize, f64)> = None;
    for (ti, &(top, row_h)) in rows.iter().enumerate() {
        if row_h <= 0.0 {
            continue;
        }
        let y = row_y(top);
        if y + row_h < lanes.top() || y > lanes.bottom() {
            continue;
        }
        let base_h = row_h
            - take_rows_height(&p.tracks[ti])
            - crate::automation_ui::lanes_height(&s.auto, &p.tracks[ti]);
        let inset = region_inset(base_h);
        for ri in 0..p.tracks[ti].regions.len() {
            let start_beat = p.region_start_beats(&p.tracks[ti].regions[ri]);
            let len_beats = p.region_len_beats(&p.tracks[ti].regions[ri]);
            let full = Rect::from_min_max(
                pos2(x_of(start_beat), y + inset),
                pos2(x_of(start_beat + len_beats), y + base_h - inset),
            );
            let rr = full.intersect(lanes);
            if rr.width() <= 0.0 || rr.height() <= 0.0 {
                continue;
            }
            let resp = ui.interact(rr, id(("region", ti, ri)), Sense::click_and_drag());
            // A region whose audio is missing says which file it waits for.
            let resp = match &p.tracks[ti].regions[ri].missing {
                Some(m) => resp.on_hover_text(m.shown()),
                None => resp,
            };
            // Shift or Command click adds to the selection (or takes the region out of it). A
            // plain click or drag on a region that is selected keeps the whole selection.
            // A drag that begins with Shift held on a region outside the selection adds the
            // region and moves the whole selection (before, it dropped the selection).
            // (Command is the Command-click tool, as in Logic.)
            let add = ui.input(|i| i.modifiers.shift);
            // A click of the Marquee tool is the marquee's (it makes, moves or clears one), not a
            // region selection (Logic); a right-click still takes the region for its menu.
            let selects = tool != TrackTool::Marquee;
            if selects && resp.clicked() && add {
                v.toggle(ti, ri);
                // Shift: the region's track joins the selected tracks.
                let before = crate::track_selection::selected(p, v);
                crate::track_selection::add_track(p, v, &before, ti);
            } else if (selects && resp.clicked())
                || (resp.drag_started() && !matches!(tool, TrackTool::Marquee | TrackTool::Solo))
                || context_click(&resp)
            {
                if !v.is_selected(ti, ri) {
                    v.grab(ti, ri, add && resp.drag_started());
                }
                if resp.clicked() || resp.drag_started() {
                    // The region under the pointer is the one the Inspector shows.
                    v.make_primary(ti, ri);
                }
                p.selected_track = ti;
            }
            if resp.double_clicked() && p.tracks[ti].kind != TrackKind::Audio {
                s.bottom = Bottom::PianoRoll;
            } else if resp.double_clicked()
                && tool.moves_regions()
                && p.tracks[ti].regions[ri].audio.is_some()
            {
                // An audio region opens in the Pitch Editor, under the tracks.
                crate::pitch_editor::open_in(&mut s.pitch_ed, &mut s.bottom, ti, ri);
            }
            let fade_tool = tool == TrackTool::Fade && p.tracks[ti].kind == TrackKind::Audio;
            if fade_tool && resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            // The lower half of the region's right end: drag to shorten or lengthen it,
            // Option-drag to stretch its audio to fit (Logic: Time Stretch by dragging). The
            // upper half loops it (below).
            if tool.moves_regions() && (full.right() - rr.right()).abs() < 0.5 {
                let grip = Rect::from_min_max(pos2(rr.right() - 5.0, full.center().y), rr.max);
                let er = ui.interact(grip, id(("region_end", ti, ri)), Sense::drag());
                if er.hovered() || er.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if let (true, Some(pt)) = (er.dragged(), er.interact_pointer_pos()) {
                    let end = snap_pos(beat_of(pt.x)).max(start_beat + 1.0 / 64.0);
                    let stretch = ui.input(|i| i.modifiers.alt);
                    v.edge_drag = Some((ti, ri, end, stretch));
                    let x = x_of(end);
                    ui.painter_at(lanes).line_segment(
                        [pos2(x, full.top()), pos2(x, full.bottom())],
                        Stroke::new(2.0, theme::ORANGE()),
                    );
                }
                if er.drag_stopped() {
                    if let Some((t, r, end, stretch)) = v.edge_drag.take() {
                        tool_act = Some(ToolAct::End(t, r, end, stretch));
                    }
                }
            }
            // The lower half of the region's left end: drag to move its start; what it plays
            // stays where it is in the song (Logic: resize from the lower-left corner).
            if tool.moves_regions() && (full.left() - rr.left()).abs() < 0.5 {
                let grip = Rect::from_min_max(
                    pos2(rr.left(), full.center().y),
                    pos2(rr.left() + 5.0, rr.bottom()),
                );
                let sr = ui.interact(grip, id(("region_start", ti, ri)), Sense::drag());
                if sr.hovered() || sr.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if let (true, Some(pt)) = (sr.dragged(), sr.interact_pointer_pos()) {
                    let at = snap_pos(beat_of(pt.x)).min(start_beat + len_beats - 1.0 / 64.0);
                    v.start_drag = Some((ti, ri, at));
                    let x = x_of(at);
                    ui.painter_at(lanes).line_segment(
                        [pos2(x, full.top()), pos2(x, full.bottom())],
                        Stroke::new(2.0, theme::ORANGE()),
                    );
                }
                if sr.drag_stopped() {
                    if let Some((t, r, at)) = v.start_drag.take() {
                        tool_act = Some(ToolAct::Start(t, r, at));
                    }
                }
            }
            if tool.moves_regions() {
                moved |= crate::region_loop::loop_handle(
                    ui,
                    p,
                    (ti, ri),
                    full,
                    lanes,
                    id(("region_loop", ti, ri)),
                    &x_of,
                    &beat_of,
                    &snap_pos,
                );
            }
            if tool == TrackTool::Solo
                && v.solo_hold.is_none()
                && resp.is_pointer_button_down_on()
                && ui.input(|i| i.pointer.primary_down())
            {
                let from = ui.input(|i| i.pointer.press_origin());
                if let Some(pt) = from.or(resp.interact_pointer_pos()) {
                    solo_press = Some((ti, ri, beat_of(pt.x)));
                }
            }
            if tool == TrackTool::Marquee {
                mq.read(ui, &resp);
            } else {
                mq.other |= resp.clicked() || resp.drag_started_by(egui::PointerButton::Primary);
            }
            let click_tool = !tool.moves_regions()
                && !matches!(
                    tool,
                    TrackTool::Fade | TrackTool::Gain | TrackTool::Marquee | TrackTool::Solo
                );
            // The Scissors show where they would cut, on the grid.
            if tool == TrackTool::Scissors {
                if let Some(pt) = resp.hover_pos() {
                    let at = snap_pos(beat_of(pt.x));
                    if at > start_beat + 1e-6 && at < start_beat + len_beats - 1e-6 {
                        cut_preview = Some((x_of(at), full));
                    }
                }
            }
            if tool == TrackTool::Text {
                let r = &p.tracks[ti].regions[ri];
                let name_now = r.name.clone();
                let field = Rect::from_min_size(rr.min, vec2(rr.width().max(80.0), 18.0));
                if let Some(name) = crate::widgets::rename_field_opened(
                    ui,
                    &resp,
                    field,
                    id(("region_name", ti, ri)),
                    &name_now,
                    theme::font(Face::Silk, 12.0),
                    resp.clicked(),
                ) {
                    // Every selected region when the clicked one is selected (Logic).
                    let regions = crate::region_commands::rename_targets(p, v, (ti, ri));
                    crate::region_commands::rename(p, &regions, &name);
                }
            }
            if click_tool {
                if resp.clicked() {
                    let at = resp.interact_pointer_pos().map(|pt| beat_of(pt.x));
                    let alt = ui.input(|i| i.modifiers.alt);
                    match tool {
                        TrackTool::Eraser => tool_act = Some(ToolAct::Erase(ti, ri)),
                        TrackTool::Scissors => {
                            if let Some(b) = at {
                                tool_act = Some(ToolAct::Split(ti, ri, snap_pos(b), alt));
                            }
                        }
                        TrackTool::Join => tool_act = Some(ToolAct::Join(ti, ri)),
                        TrackTool::Mute => tool_act = Some(ToolAct::Mute(ti, ri)),
                        _ => {}
                    }
                }
            } else if fade_tool {
                // The Fade tool: the left half of the region gets a fade in, the right half a
                // fade out, as long as the pointer is dragged (in 1/16 notes, as the Inspector),
                // on every selected audio region when this one is selected; a drag across the
                // place where it touches another audio region crossfades the two; Option-click
                // removes a fade (`fade_tool.rs`). Control-Shift-drag on a fade changes its
                // curve instead (`fade_curve.rs`).
                let shape_keys = ui.input(|i| i.modifiers.ctrl && i.modifiers.shift);
                if crate::fade_curve::on_region(ui, p, v, &resp, (ti, ri), shape_keys, &beat_of) {
                    moved = true;
                } else if resp.drag_started() && !shape_keys {
                    let from = ui.input(|i| i.pointer.press_origin());
                    if let Some(pt) = from.or(resp.interact_pointer_pos()) {
                        v.fade_drag =
                            crate::fade_tool::begin(p, &v.selection(), (ti, ri), beat_of(pt.x));
                    }
                }
                if let (true, Some(d), Some(pt)) = (
                    resp.dragged(),
                    v.fade_drag.as_ref(),
                    resp.interact_pointer_pos(),
                ) {
                    crate::fade_tool::drag_to(p, d, beat_of(pt.x));
                }
                if resp.drag_stopped() {
                    v.fade_drag = None;
                    moved = true;
                }
                if resp.clicked() && ui.input(|i| i.modifiers.alt) {
                    if let Some(pt) = resp.interact_pointer_pos() {
                        moved |= crate::fade_tool::remove_at(p, (ti, ri), beat_of(pt.x));
                    }
                }
            } else if tool == TrackTool::Gain {
                // The Gain tool: up is louder, in half decibels, on every selected audio
                // region together when the one dragged is selected.
                if resp.drag_started() {
                    // Measured from where the button went down.
                    let from = ui.input(|i| i.pointer.press_origin());
                    if let Some(pt) = from.or(resp.interact_pointer_pos()) {
                        let picked = if v.is_selected(ti, ri) {
                            v.selection()
                        } else {
                            vec![(ti, ri)]
                        };
                        let from: Vec<(usize, usize, f32)> = picked
                            .into_iter()
                            .filter_map(|(t, r)| {
                                let reg = p.tracks.get(t)?.regions.get(r)?;
                                reg.is_audio().then_some((t, r, reg.params.gain_db))
                            })
                            .collect();
                        v.gain_drag = (!from.is_empty()).then_some((from, pt.y));
                    }
                }
                if let (true, Some((from, y0)), Some(pt)) = (
                    resp.dragged(),
                    v.gain_drag.as_ref(),
                    resp.interact_pointer_pos(),
                ) {
                    let by = gain_for_drag(y0 - pt.y);
                    for &(t, r, g0) in from {
                        if let Some(reg) = p.tracks.get_mut(t).and_then(|x| x.regions.get_mut(r)) {
                            reg.params.gain_db = (g0 + by).clamp(
                                crate::model::MIN_REGION_GAIN,
                                crate::model::MAX_REGION_GAIN,
                            );
                        }
                    }
                    let now = p.tracks[ti].regions[ri].params.gain_db;
                    let layer = egui::LayerId::new(egui::Order::Tooltip, id("gain_readout"));
                    hw::lcd_readout(
                        &ui.ctx().layer_painter(layer),
                        Rect::from_min_size(pt + vec2(14.0, -34.0), vec2(64.0, 30.0)),
                        "GAIN",
                        &format!("{now:+.1}"),
                    );
                }
                if resp.drag_stopped() {
                    v.gain_drag = None;
                    moved = true;
                }
            } else if tool.moves_regions() {
                if resp.drag_started() {
                    if let Some(pt) = resp.interact_pointer_pos() {
                        v.grab_beats = beat_of(pt.x) - start_beat;
                        v.drag_origin = Some(pt);
                    }
                    // Every selected region moves with the one being dragged: each remembers
                    // where it started.
                    let mut group: Vec<(usize, usize, f64)> = v
                        .selection()
                        .into_iter()
                        .filter_map(|(gt, gr)| {
                            let reg = p.tracks.get(gt)?.regions.get(gr)?;
                            Some((gt, gr, p.region_start_beats(reg)))
                        })
                        .collect();
                    if !group.iter().any(|g| (g.0, g.1) == (ti, ri)) {
                        group.push((ti, ri, start_beat));
                    }
                    v.group_drag = group;
                    v.drag_by = (0.0, 0);
                }
                // Option held: a copy (Logic). Taken when the button is let go, so pressing or
                // letting go of Option on the way switches between moving and copying.
                let copy = ui.input(|i| i.modifiers.alt);
                if resp.dragged() {
                    if copy {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
                    }
                    if let Some(pt) = resp.interact_pointer_pos() {
                        // Held on the way (Logic): Control moves in divisions and
                        // Control-Shift in ticks, whatever Snap is set to; Shift alone keeps
                        // the drag to one direction, the one it has gone furthest in.
                        let mods = ui.input(|i| i.modifiers);
                        let raw = (beat_of(pt.x) - v.grab_beats).max(0.0);
                        let grabbed = v
                            .group_drag
                            .iter()
                            .find(|g| (g.0, g.1) == (ti, ri))
                            .map_or(start_beat, |g| g.2);
                        // Relative (Logic's default): an off-grid region keeps its offset.
                        let nb = match drag_snap(mods) {
                            Some(step) => snap_to(raw, step),
                            None => v.grid.region_start(raw, grabbed, &snap_pos),
                        };
                        let lock = drag_lock(mods, v.drag_origin, pt);
                        let delta = if lock == Some(DragAxis::Tracks) {
                            0.0
                        } else {
                            crate::region_edit::clamp_delta(&v.group_drag, nb - grabbed)
                        };
                        // The track under the pointer, if every region fits on the track
                        // as far from its own.
                        let down = row_at(&rows, pt.y - lanes.top() + scroll_y)
                            .map_or(v.drag_by.1, |row| row as isize - ti as isize);
                        let down = if lock == Some(DragAxis::Time) {
                            0
                        } else {
                            down
                        };
                        let down =
                            if crate::region_edit::fits_rows(p, &v.group_drag, down, v.show_hidden)
                            {
                                down
                            } else {
                                0
                            };
                        v.drag_by = (delta, down);
                        // A move on the same tracks takes the regions along; a copy or a move
                        // to other tracks leaves them in place, and what lands follows the
                        // pointer (drawn after the regions).
                        let shown = if copy || down != 0 { 0.0 } else { delta };
                        let moves: Vec<(usize, usize, f32)> = v
                            .group_drag
                            .iter()
                            .map(|&(gt, gr, s0)| {
                                (gt, gr, crate::region_edit::landing_bar(p, s0, shown))
                            })
                            .collect();
                        for (gt, gr, bar) in moves {
                            if let Some(reg) = p.tracks[gt].regions.get_mut(gr) {
                                reg.start_bar = bar;
                            }
                        }
                    }
                }
                if resp.drag_stopped() {
                    // Placed after this frame's regions (a move to another track takes the
                    // region off this one).
                    let group = std::mem::take(&mut v.group_drag);
                    let (delta, down) = std::mem::take(&mut v.drag_by);
                    v.drag_origin = None;
                    tool_act = Some(ToolAct::Drop(group, delta, down, copy));
                }
            }
            let (is_audio, muted) = (
                p.tracks[ti].kind == TrackKind::Audio,
                p.tracks[ti].regions[ri].params.mute,
            );
            let folder_open = p.tracks[ti].regions[ri].folder.as_ref().map(|f| f.open);
            let generated = p.tracks[ti].regions[ri].generated.is_some();
            let shown = v.shot_menu == Some((ti, Some(ri)));
            crate::widgets::context_menu_shown(&resp, shown, |ui| {
                region_items(
                    ui,
                    is_audio,
                    muted,
                    folder_open,
                    generated,
                    ti,
                    ri,
                    &mut picked,
                );
            });
            let selected = v.is_selected(ti, ri);
            if folder_open.is_some() {
                // The triangle opens and closes the folder; the rows of an opened one are
                // swiped to pick which take plays where.
                let tri = ui.interact(
                    folder_toggle_rect(full).intersect(lanes),
                    id(("take_toggle", ti, ri)),
                    Sense::click(),
                );
                if tri.clicked() {
                    picked = Some(CtxAction::ToggleTakeFolder(ti, ri));
                    v.select_only(ti, ri);
                }
                if folder_open == Some(true) {
                    let n = p.tracks[ti].regions[ri]
                        .folder
                        .as_ref()
                        .map_or(0, |f| f.takes.len());
                    for take in 0..n {
                        let row = take_row_rect(full, n, take).intersect(lanes);
                        if row.width() <= 0.0 || row.height() <= 0.0 {
                            continue;
                        }
                        let resp = ui.interact(
                            row,
                            id(("take_row", ti, ri, take)),
                            Sense::click_and_drag(),
                        );
                        let Some(region) = p.tracks[ti].regions.get_mut(ri) else {
                            continue;
                        };
                        let Some(folder) = region.folder.as_mut() else {
                            continue;
                        };
                        let grab =
                            |pt: egui::Pos2| snap_pos((beat_of(pt.x) - start_beat).max(0.0)) as f32;
                        if resp.drag_started() {
                            if let Some(pt) = resp.interact_pointer_pos() {
                                v.swipe = Some((folder.comp.clone(), grab(pt)));
                            }
                        }
                        if resp.dragged() {
                            if let (Some((orig, anchor)), Some(pt)) =
                                (v.swipe.as_ref(), resp.interact_pointer_pos())
                            {
                                folder.comp = orig.clone();
                                folder.swipe(take, *anchor, grab(pt));
                                folder.apply(&mut region.notes, &mut region.controls);
                            }
                        }
                        if resp.drag_stopped() {
                            v.swipe = None;
                            moved = true;
                        }
                        if resp.clicked() {
                            // A click picks the whole take.
                            let len = folder.len;
                            folder.swipe(take, 0.0, len);
                            folder.apply(&mut region.notes, &mut region.controls);
                            moved = true;
                        }
                    }
                }
                if let Some(folder) = p.tracks[ti].regions[ri].folder.as_ref() {
                    draw_take_folder(
                        &g,
                        full,
                        &p.tracks[ti],
                        ti + 1,
                        &p.tracks[ti].regions[ri],
                        folder,
                        beat_px,
                        selected,
                        // A region held by the Solo tool silences the others.
                        silenced_by_solo(p, ti) || p.region_solo.is_some_and(|x| x != (ti, ri)),
                    );
                    continue;
                }
            }
            let tr = &p.tracks[ti];
            // Edge to edge on the grid: a region that ends where the next begins touches it,
            // and the outlines drawn inside them tell them apart. Where the view cuts a
            // region, its edge stays out of sight.
            let paint = full.intersect(lanes.expand2(vec2(4.0, 0.0)));
            draw_region(
                &g,
                paint,
                full,
                tr,
                ti + 1,
                &tr.regions[ri],
                p.effective_fades(ti, ri),
                crate::fade_curve::under(p, ti, ri),
                beat_px,
                selected,
                rec_targets.contains(&(ti, ri)),
                silenced_by_solo(p, ti) || p.region_solo.is_some_and(|x| x != (ti, ri)),
                &bar_xs,
                AudioSpan::of(p, &tempo_map, &tr.regions[ri]).as_ref(),
            );
        }
    }
    // What a copy or a move to other tracks lands as, following the pointer.
    let copying = ui.input(|i| i.modifiers.alt);
    if !v.group_drag.is_empty() && (copying || v.drag_by.1 != 0) {
        let (delta, down) = v.drag_by;
        for &(gt, gr, s0) in &v.group_drag {
            let Some(region) = p.tracks.get(gt).and_then(|t| t.regions.get(gr)) else {
                continue;
            };
            let to = gt.saturating_add_signed(down);
            let Some(&(top, row_h)) = rows.get(to) else {
                continue;
            };
            let y = row_y(top);
            let base_h = row_h
                - take_rows_height(&p.tracks[to])
                - crate::automation_ui::lanes_height(&s.auto, &p.tracks[to]);
            let inset = region_inset(base_h);
            let start = (s0 + delta).max(0.0);
            let full = Rect::from_min_max(
                pos2(x_of(start), y + inset),
                pos2(x_of(start + p.region_len_beats(region)), y + base_h - inset),
            );
            let paint = full.intersect(lanes.expand2(vec2(4.0, 0.0)));
            if paint.width() > 0.0 && paint.height() > 0.0 {
                draw_region(
                    &g,
                    paint,
                    full,
                    &p.tracks[to],
                    to + 1,
                    region,
                    p.effective_fades(gt, gr),
                    None,
                    beat_px,
                    true,
                    false,
                    false,
                    &bar_xs,
                    AudioSpan::of(p, &tempo_map, region).as_ref(),
                );
            }
        }
    }
    if let Some((x, region)) = cut_preview {
        hw_tracks::time_line(&g, x, region.y_range(), 1.0, theme::ORANGE());
    }
    if let Some((t, r, at)) = solo_press {
        v.solo_hold = crate::region_solo::begin(p, (t, r), at);
    }
    // The Marquee tool: a drag selects a stretch of time on the tracks it crosses, on the
    // Snap grid; Shift moves the nearer edge of the one there is; a click clears it, and so
    // does a click or a drag with another tool.
    let cell = |pt: Pos2| {
        let row = row_at(&rows, pt.y - lanes.top() + scroll_y).unwrap_or_else(|| {
            if pt.y < lanes.top() {
                0
            } else {
                rows.len().saturating_sub(1)
            }
        });
        (snap_pos(beat_of(pt.x)), row)
    };
    if let Some(pt) = mq.started {
        v.range.begin(cell(pt), adding);
    }
    if let Some(pt) = mq.at {
        v.range.drag_to(cell(pt));
    }
    if mq.stopped {
        v.range.end();
    }
    if let Some(pt) = mq.clicked {
        if adding && v.range.active().is_some() {
            v.range.begin(cell(pt), true);
            v.range.end();
        } else {
            v.range.clear();
        }
    }
    if mq.other {
        v.range.clear();
    }
    if let Some(r) = v
        .range
        .range
        .filter(|r| !r.is_empty() || v.range.anchor.is_some())
    {
        draw_marquee(&g, &rows, r, &x_of, &row_y, |t| {
            let track = &p.tracks[t];
            row_h_of(&rows, t)
                - take_rows_height(track)
                - crate::automation_ui::lanes_height(&s.auto, track)
        });
    }
    if let Some(band) = band {
        g.rect_filled(band, 0.0, theme::ORANGE().gamma_multiply(0.12));
        g.rect_stroke(
            band,
            0.0,
            Stroke::new(1.0, theme::ORANGE()),
            StrokeKind::Inside,
        );
    }
    if moved {
        p.song_rev += 1;
    }
    match tool_act {
        Some(ToolAct::Drop(group, delta, down, copy)) => {
            let landed =
                crate::drag_modes::drop(p, &s.auto, &group, delta, down, copy, v.drag_mode);
            // The copies (or the regions on their new tracks) are what is selected.
            if copy || down != 0 {
                v.select_many(&landed);
                if let Some(&(t, _)) = landed.first() {
                    p.selected_track = t;
                }
            }
        }
        Some(ToolAct::Erase(ti, ri)) => crate::shuffle_edit::erase(p, v, (ti, ri)),
        Some(ToolAct::Split(ti, ri, at, equal)) => {
            if !crate::region_edit::scissors(p, v, (ti, ri), at, equal) {
                s.status = Some(tr("Cannot cut this region here").into());
            }
        }
        Some(ToolAct::Mute(ti, ri)) => {
            crate::region_edit::toggle_mute(p, v, Some((ti, ri)));
        }
        Some(ToolAct::Start(ti, ri, at)) => {
            crate::shuffle_edit::resize_starts(p, v, (ti, ri), at);
        }
        Some(ToolAct::Join(ti, ri)) => {
            if p.join_regions(ti, ri) {
                v.select_only(ti, ri);
            } else {
                s.status = Some(tr("These regions cannot be joined").into());
            }
        }
        // Every selected region's end moves with the clicked one's (Logic).
        Some(ToolAct::End(ti, ri, end, stretch))
            if !crate::shuffle_edit::resize_ends(p, v, (ti, ri), end, stretch) =>
        {
            s.status = Some(tr("This region cannot be stretched").into());
        }
        Some(ToolAct::End(..)) | None => {}
    }

    if marker_h > 0.0 {
        let top = ruler_y + RULER_H;
        let mut marker_sel = v.selected_marker;
        let picked = crate::marker_track::show(
            ui,
            p,
            Rect::from_min_max(
                pos2(area.left(), top),
                pos2(headers.right(), top + marker_h),
            ),
            Rect::from_min_max(pos2(lanes.left(), top), pos2(area.right(), top + marker_h)),
            x_of,
            beat_of,
            &snap_pos,
            &mut marker_sel,
        );
        if picked {
            v.clear_selection();
            v.range.clear();
        }
        v.selected_marker = marker_sel;
    }
    if signature_h > 0.0 {
        let top = ruler_y + RULER_H + marker_h;
        crate::signature_track::show(
            ui,
            p,
            Rect::from_min_max(
                pos2(area.left(), top),
                pos2(headers.right(), top + signature_h),
            ),
            Rect::from_min_max(
                pos2(lanes.left(), top),
                pos2(area.right(), top + signature_h),
            ),
            x_of,
            beat_of,
        );
    }
    if tempo_h > 0.0 {
        let top = ruler_y + RULER_H + marker_h + signature_h;
        crate::tempo_track::show(
            ui,
            p,
            Rect::from_min_max(pos2(area.left(), top), pos2(headers.right(), tracks_y)),
            Rect::from_min_max(pos2(lanes.left(), top), pos2(area.right(), tracks_y)),
            x_of,
            beat_of,
            &snap_pos,
        );
    }

    // ---- automation lanes (Automation Mode): over each track's regions, and under it
    if s.auto.mode {
        let free = v.grid.free(v.snap);
        let grid = crate::automation_ui::Grid {
            x_of: &x_of,
            beat_of: &beat_of,
            step: step.max(1.0 / 64.0),
            free,
        };
        if let Some(r) = master_lane {
            let gm = ui.painter_at(r);
            let shown = p.master_auto.shown;
            let strip = crate::automation::Strip::Master;
            crate::automation_ui::lane(ui, &gm, p, &mut s.auto, strip, shown, r, &grid, false);
        }
        let gl = ui.painter_at(lanes);
        for (ti, &(top, row_h)) in rows.iter().enumerate() {
            let Some(t) = p.tracks.get(ti) else { continue };
            if row_h <= 0.0 || !crate::automation_ui::has_lanes(t) {
                continue;
            }
            let y = row_y(top);
            if y + row_h < lanes.top() || y > lanes.bottom() {
                continue;
            }
            let below = crate::automation_ui::lanes_height(&s.auto, t);
            let base_h = row_h - take_rows_height(t) - below;
            // The track's own lane: over its regions, under their name strips (a click there
            // still takes the region).
            let inset = region_inset(base_h);
            let main = Rect::from_min_max(
                pos2(lanes.left(), y + inset + REGION_TITLE_H),
                pos2(lanes.right(), y + base_h - inset),
            );
            let shown = t.auto.shown;
            let extra = t.auto.extra.clone();
            let strip = crate::automation::Strip::Track(ti);
            if main.height() > 4.0 {
                crate::automation_ui::lane(
                    ui,
                    &gl,
                    p,
                    &mut s.auto,
                    strip,
                    shown,
                    main,
                    &grid,
                    true,
                );
            }
            for (k, target) in extra.into_iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(
                        lanes.left(),
                        y + row_h - below + k as f32 * crate::automation_ui::LANE_H,
                    ),
                    vec2(lanes.width(), crate::automation_ui::LANE_H),
                );
                crate::automation_ui::lane(ui, &gl, p, &mut s.auto, strip, target, r, &grid, false);
            }
        }
    }

    // ---- keyboard: Delete (Command-R, Repeat, is a key command: `key_commands`)
    // A and B (Automation Mode, Draw Mode) and the keys of a range selected on a lane come
    // first; the Piano Roll keeps its own A while the pointer is over it, and Musical Typing
    // plays notes on A while it is open.
    // Not while a window (a dialog, the chooser, the settings, the bounce window) is open, nor
    // while a region is held by the Solo tool (Delete would shift the region numbers).
    let area_keys = !window_open && v.solo_hold.is_none();
    if !ui.ctx().egui_wants_keyboard_input() && !s.piano_focus && !s.typing.open && area_keys {
        crate::automation_ui::keys(ui, p, &mut s.auto);
    }
    // T: the Tool menu at the pointer, while the pointer is over the Tracks area (Logic).
    let keys_free = !ui.ctx().egui_wants_keyboard_input()
        && !s.piano_focus
        && !s.typing.open
        && s.track_dialog.is_none()
        && s.chooser.is_none()
        && s.settings_tab.is_none();
    if keys_free && v.tool_menu.is_none() {
        if let Some(pt) = hover.filter(|pt| full.contains(*pt)) {
            if take_plain_key(ui, Key::T) {
                v.tool_menu = Some(pt);
            }
        }
    }
    if !ui.ctx().egui_wants_keyboard_input() && !s.piano_focus && area_keys {
        // Command-Delete is Delete Track (`key_commands`), not this.
        let del = ui.input(|i| {
            !i.modifiers.command
                && !i.modifiers.ctrl
                && (i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
        });
        if del {
            let tracks = p.tracks.len();
            delete_selected(p, v);
            // The rows above were laid out for the old track list; drawing them now would
            // read tracks that are gone. Draw the new list on the next frame instead.
            if p.tracks.len() != tracks {
                ui.ctx().request_repaint();
                return;
            }
        }
    }

    // ---- headers
    let gh = ui.painter_at(headers);
    gh.rect_filled(headers, 0.0, hw_tracks::PANEL_INSPECTOR());
    for (i, &(top, row_h)) in rows.iter().enumerate() {
        if row_h <= 0.0 {
            continue;
        }
        let y = headers.top() + top - scroll_y;
        if y + row_h < headers.top() || y > headers.bottom() {
            continue;
        }
        let chooser = crate::automation_ui::chooser_height(&s.auto, &p.tracks[i]);
        let below = crate::automation_ui::lanes_height(&s.auto, &p.tracks[i]);
        let hdr = Rect::from_min_size(
            pos2(headers.left(), y),
            vec2(HEADER_W, row_h - chooser - below),
        );
        let peaks = meters.tracks.get(i).copied().unwrap_or((0.0, 0.0));
        let silenced = silenced_by_solo(p, i);
        let selected = crate::track_selection::is_selected(p, &s.tracks, i);
        let resp = header(ui, &gh, p, i, hdr, peaks, silenced, hide_view, selected);
        // A click on a track header selects the track, Command-click adds it or takes it out,
        // Shift-click selects the tracks from the last one clicked; the regions of every
        // selected track are selected (Logic: Select Regions on Track Selection).
        if context_click(&resp) {
            crate::track_selection::context_click(p, &mut s.tracks, i);
        } else if resp.clicked() {
            let how = crate::track_selection::Click::from_mods(ui.input(|i| i.modifiers));
            crate::track_selection::click(p, &mut s.tracks, i, how, hide_view);
        }
        if chooser > 0.0 {
            let strip = crate::automation::Strip::Track(i);
            let cr = Rect::from_min_size(
                pos2(headers.left() + NUM_W, hdr.bottom()),
                vec2(HEADER_W - 2.0 * NUM_W, chooser),
            );
            hw_tracks::header_bg(
                &gh,
                Rect::from_min_max(
                    pos2(headers.left(), hdr.bottom()),
                    pos2(headers.left() + HEADER_W, y + row_h),
                ),
                selected,
            );
            crate::automation_ui::choosers(ui, &gh, p, &mut s.auto, strip, cr, None);
            let n = p.tracks[i].auto.extra.len();
            for k in 0..n {
                let lr = Rect::from_min_size(
                    pos2(
                        headers.left() + NUM_W,
                        y + row_h - below + k as f32 * crate::automation_ui::LANE_H,
                    ),
                    vec2(HEADER_W - 2.0 * NUM_W, crate::automation_ui::CHOOSER_H),
                );
                gh.hline(
                    headers.left() + NUM_W..=headers.left() + HEADER_W,
                    lr.top() + 0.5,
                    Stroke::new(1.0, theme::SEAM()),
                );
                crate::automation_ui::choosers(ui, &gh, p, &mut s.auto, strip, lr, Some(k));
            }
        }
        let t = &p.tracks[i];
        let info = TrackInfo {
            i,
            folder: t.kind == TrackKind::Folder,
            mute: t.mute,
            solo: t.solo,
            custom_height: t.height != 1.0,
            last: p.tracks.len() == 1,
            stack: if crate::track_selection::is_selected(p, &s.tracks, i) {
                crate::track_selection::can_create_stack(p, &s.tracks)
            } else {
                !p.recording
            },
            groove: t.params.groove,
            follow_groove: crate::groove::groove_track(p)
                .filter(|g| *g != i)
                .map(|_| t.params.follow_groove),
        };
        let shown = s.tracks.shot_menu == Some((i, None));
        crate::widgets::context_menu_shown(&resp, shown, |ui| track_items(ui, &info, &mut picked));
    }
    if let Some(action) = picked {
        apply_ctx(action, p, s, lanes.width());
    }
    let v = &mut s.tracks;

    // ---- individual track height: drag the lower edge of a track (header or lane), as in
    // Logic. Registered after headers and regions so the edge wins over them.
    let auto_extra: Vec<f32> = p
        .tracks
        .iter()
        .map(|t| {
            crate::automation_ui::chooser_height(&s.auto, t)
                + crate::automation_ui::lanes_height(&s.auto, t)
        })
        .collect();
    let edges = Rect::from_min_max(pos2(headers.left(), lanes.top()), lanes.right_bottom());
    for (i, &(top, row_h)) in rows.iter().enumerate() {
        if row_h <= 0.0 {
            continue;
        }
        let bottom = lanes.top() + top + row_h - scroll_y;
        if bottom < edges.top() || bottom > edges.bottom() {
            continue;
        }
        let edge = Rect::from_min_max(
            pos2(edges.left(), bottom - RESIZE_GRIP),
            pos2(edges.right(), bottom + RESIZE_GRIP),
        )
        .intersect(edges);
        let resp = ui
            .interact(edge, id(("track_edge", i)), Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeVertical);
        if resp.dragged() {
            if let (Some(pt), Some(t)) = (resp.interact_pointer_pos(), p.tracks.get_mut(i)) {
                let wanted = pt.y - (bottom - row_h) - take_rows_height(t) - auto_extra[i];
                t.height = (wanted / v.row_h).clamp(Track::MIN_HEIGHT, Track::MAX_HEIGHT);
            }
        }
        if resp.double_clicked() {
            if let Some(t) = p.tracks.get_mut(i) {
                t.height = 1.0;
            }
        }
    }
    hw_tracks::seam_wide_v(&gh, headers.right(), headers.y_range());

    // ---- corner above the headers: "+" (new track), duplicate track, the column labels
    let gc = ui.painter_at(corner);
    gc.rect_filled(corner, 0.0, hw_tracks::PANEL_INSPECTOR());
    gc.hline(
        corner.x_range(),
        corner.bottom() - 0.5,
        Stroke::new(1.0, theme::INK2()),
    );
    hw_tracks::seam_wide_v(&gc, corner.right(), corner.y_range());
    let key_h = (corner.height() - 13.0).max(18.0);
    let plus = Rect::from_min_size(
        pos2(corner.left() + 14.0, corner.top() + 6.0),
        vec2(32.0, key_h),
    );
    let pr = glyph_key(ui, &gc, plus, "plus", false, false, Glyph::Plus);
    // "+": the kinds of track to add, as the right-click menu offers them.
    let mut picked = None;
    egui::Popup::menu(&pr).show(|ui| {
        ui.set_min_width(230.0);
        new_track_items(ui, &mut picked);
    });
    if let Some(CtxAction::NewTrack(kind)) = picked {
        p.selected_track = p.add_track(kind);
    }
    let dup = plus.translate(vec2(40.0, 0.0));
    let dr = glyph_key(ui, &gc, dup, "dup", false, false, Glyph::Dup);
    // Logic's duplicate key: New Track with Duplicate Settings (no regions), of each selected
    // track.
    if dr.clicked() && crate::track_selection::can_duplicate(p, &s.tracks) {
        crate::track_selection::duplicate_tracks(p, s);
    }
    let v = &mut s.tracks;
    // Column labels over the headers: the track count, then volume and pan.
    let col = header_layout(
        Rect::from_min_size(corner.min, vec2(HEADER_W, v.row_h)),
        0.0,
    );
    let label_y = corner.top() + 6.0 + key_h / 2.0;
    hw::silk(
        &gc,
        pos2(col.name_x.max(dup.right() + 14.0), label_y),
        Align2::LEFT_CENTER,
        &format!("{:02} {}", p.tracks.len(), tr("Tracks")),
        11.0,
        theme::PRINT(),
    );
    hw::silk(
        &gc,
        pos2(col.slider.left(), label_y),
        Align2::LEFT_CENTER,
        "VOL",
        11.0,
        theme::PRINT(),
    );
    hw::silk(
        &gc,
        pos2(col.pan.x, label_y),
        Align2::CENTER_CENTER,
        tr("Pan"),
        11.0,
        theme::PRINT(),
    );

    // ---- ruler: the cycle band on top, bar lines with zero-padded numbers, beat ticks
    let gr = ui.painter_at(ruler);
    hw::gradient(
        &gr,
        ruler,
        0.0,
        theme::pick(
            Color32::from_rgb(0xee, 0xea, 0xe1),
            Color32::from_rgb(0x28, 0x28, 0x2c),
        ),
        theme::pick(
            Color32::from_rgb(0xe2, 0xdd, 0xd2),
            Color32::from_rgb(0x21, 0x21, 0x24),
        ),
    );
    let numbers_top = ruler.top() + CYCLE_ZONE_H;
    // The stretch of the ruler over the selected region is shaded.
    let selection = v
        .selected
        .and_then(|(t, r)| p.tracks.get(t)?.regions.get(r))
        .map(|r| (p.region_start_beats(r), p.region_end_beats(r)));
    if let Some((start, end)) = selection {
        let band = Rect::from_min_max(
            pos2(x_of(start).max(ruler.left()), numbers_top),
            pos2(x_of(end).min(ruler.right()), ruler.bottom()),
        );
        if band.width() > 0.0 {
            gr.rect_filled(band, 0.0, RULER_SELECTION);
        }
    }
    // The cycle: solid orange while on, a recessed band while off.
    {
        let band = Rect::from_min_max(
            pos2(x_of(p.cycle_start), ruler.top() + 2.0),
            pos2(x_of(p.cycle_end), ruler.top() + 15.0),
        );
        let text = format!(
            "{} → {}",
            hw_tracks::bbdt_text(p.bbdt(p.cycle_start)),
            hw_tracks::bbdt_text(p.bbdt(p.cycle_end))
        );
        hw_tracks::cycle_band(&gr, band, p.cycle, &text);
        if p.cycle {
            gr.rect_filled(
                Rect::from_min_size(pos2(band.left(), ruler.top()), vec2(2.0, RULER_H)),
                0.0,
                theme::INK(),
            );
        }
    }
    let label_every = [1i64, 2, 4, 8, 16, 32]
        .into_iter()
        .find(|n| *n as f32 * v.bar_w >= 40.0)
        .unwrap_or(64);
    for bar in &bars {
        let b = bar.bar0 as i64;
        let x = x_of(bar.start);
        if b % label_every == 0 {
            hw_tracks::time_line(&gr, x, numbers_top..=ruler.bottom(), 1.0, theme::INK());
            hw_tracks::label(
                &gr,
                pos2(x + 5.0, numbers_top + 4.0),
                Align2::LEFT_TOP,
                &format!("{:03}", b + 1),
                Face::Lcd,
                12.0,
                0.0,
                theme::INK(),
            );
        } else {
            hw_tracks::time_line(
                &gr,
                x,
                ruler.bottom() - 7.0..=ruler.bottom(),
                1.0,
                theme::INK(),
            );
        }
        if beat_px * bar.unit as f32 >= 10.0 {
            for k in 1..bar.num {
                let xb = x_of(bar.start + k as f64 * bar.unit);
                hw_tracks::time_line(
                    &gr,
                    xb,
                    ruler.bottom() - 7.0..=ruler.bottom(),
                    1.0,
                    theme::INK2().gamma_multiply(0.6),
                );
            }
        }
    }
    gr.hline(
        ruler.x_range(),
        ruler.bottom() - 0.5,
        Stroke::new(1.0, theme::INK2()),
    );
    // The cycle strip on top (`cycle_area`): click toggles the cycle, a drag moves the band,
    // moves one locator or draws a new cycle, snapped as the Snap menu says. Under it: locate.
    let upper = Rect::from_min_max(ruler.min, pos2(ruler.right(), numbers_top));
    let lower = Rect::from_min_max(pos2(ruler.left(), numbers_top), ruler.max);
    crate::cycle_area::interact(ui, p, upper, &x_of, &beat_of, &snap_pos);
    let low = ui.interact(lower, id("ruler_locate"), Sense::click_and_drag());
    if let Some(pt) = low.interact_pointer_pos() {
        if low.clicked() || low.dragged() {
            p.position_beats = beat_of(pt.x);
        }
    }

    // ---- playhead: an orange line with an ink edge and the flag in the ruler
    let px = x_of(p.position_beats);
    // Grab the playhead's flag in the ruler and drag it anywhere in the song (back to the
    // start, past the end of the view, ...). Registered after the ruler so the flag wins over
    // the cycle area; kept alive while dragging even when the flag leaves the view.
    let head_id = id("playhead_head");
    let visible = px >= lanes.left() && px <= lanes.right();
    if visible || ui.ctx().is_being_dragged(head_id) {
        let grab = Rect::from_min_max(
            pos2(px - PLAYHEAD_GRAB, ruler.top()),
            pos2(px + PLAYHEAD_GRAB, ruler.bottom()),
        );
        let resp = ui
            .interact(grab, head_id, Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if resp.dragged() {
            if let Some(pt) = resp.interact_pointer_pos() {
                p.position_beats = playhead_beat(pt.x, lanes.left(), scroll_x, beat_px);
            }
        }
    }
    if visible {
        let (line_c, head_c, _) = playhead_colors(p.recording);
        let gp = ui.painter_at(Rect::from_min_max(ruler.min, area.max));
        hw_tracks::playhead_line(&gp, px, numbers_top + 12.0, area.bottom(), line_c);
        hw_tracks::playhead_flag(&gp, px, numbers_top, head_c);
    }

    // ---- the pointer shows its tool over the lanes (Logic), where nothing else set it
    let v = &mut s.tracks;
    if let Some(pt) = ui.input(|i| i.pointer.hover_pos()) {
        let plain = ui.ctx().output(|o| o.cursor_icon) == egui::CursorIcon::Default;
        if lanes.contains(pt) && tool != TrackTool::Pointer && v.tool_menu.is_none() && plain {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            let layer = egui::LayerId::new(egui::Order::Tooltip, id("tool_cursor"));
            hw_tracks::tool_cursor(&ui.ctx().layer_painter(layer), pt, tool.glyph());
        }
    }
    if !ui.input(|i| i.pointer.any_down()) {
        v.held_tool = None;
        if let Some(was) = v.solo_hold.take() {
            crate::region_solo::end(p, was);
        }
    }
}

/// What the Marquee tool's pointer did this frame in the lanes.
#[derive(Default)]
struct MarqueeInput {
    /// A drag began where the button went down.
    started: Option<Pos2>,
    /// The pointer of the drag is here.
    at: Option<Pos2>,
    /// The drag ended.
    stopped: bool,
    /// A click (no drag) here.
    clicked: Option<Pos2>,
    /// Another tool clicked or began a drag in the lanes.
    other: bool,
}

impl MarqueeInput {
    fn read(&mut self, ui: &Ui, resp: &egui::Response) {
        use egui::PointerButton::Primary;
        if resp.drag_started_by(Primary) {
            self.started = ui
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos());
        }
        if resp.dragged_by(Primary) {
            self.at = resp.interact_pointer_pos().or(self.at);
        }
        if resp.drag_stopped_by(Primary) {
            self.stopped = true;
        }
        if resp.clicked() {
            self.clicked = resp.interact_pointer_pos();
        }
    }
}

/// Height of track row `t` (0 when it has none).
fn row_h_of(rows: &[(f32, f32)], t: usize) -> f32 {
    rows.get(t).map_or(0.0, |r| r.1)
}

/// The marquee: a bright band over the region part of each track it covers, its edges in the
/// signal colour, on the pixel grid of the time line.
fn draw_marquee(
    g: &Painter,
    rows: &[(f32, f32)],
    r: crate::marquee::Range,
    x_of: &dyn Fn(f64) -> f32,
    row_y: &dyn Fn(f32) -> f32,
    base_h: impl Fn(usize) -> f32,
) {
    let (x0, x1) = (x_of(r.start), x_of(r.end));
    let ppp = g.pixels_per_point().max(1e-3);
    for (t, &(top, h)) in rows.iter().enumerate().take(r.last + 1).skip(r.first) {
        if h <= 0.0 {
            continue;
        }
        let y = row_y(top);
        let band = Rect::from_min_max(pos2(x0, y), pos2(x1.max(x0 + 1.0 / ppp), y + base_h(t)));
        g.rect_filled(band, 0.0, theme::BONE_HI().gamma_multiply(0.38));
        hw_tracks::time_line(g, x0, band.y_range(), 1.0, theme::ORANGE());
        hw_tracks::time_line(
            g,
            band.right() - 1.0 / ppp,
            band.y_range(),
            1.0,
            theme::ORANGE(),
        );
    }
}

/// Takes a press of `key` with no modifier held out of this frame's input; returns whether
/// there was one.
fn take_plain_key(ui: &Ui, key: Key) -> bool {
    let mut found = false;
    ui.input_mut(|i| {
        i.events.retain(|e| match e {
            egui::Event::Key {
                key: k,
                pressed,
                modifiers,
                ..
            } if *k == key && modifiers.is_none() => {
                found |= *pressed;
                false
            }
            _ => true,
        });
    });
    found
}

/// The gain (dB, in half decibels) a Gain tool drag `up` points upward adds: one decibel for
/// every ten points.
fn gain_for_drag(up: f32) -> f32 {
    (up / 10.0 * 2.0).round() / 2.0
}

#[cfg(test)]
mod row_tests {
    use super::*;

    #[test]
    fn delete_removes_the_selected_track_without_reading_it_again() {
        use crate::mixer::Meters;
        use crate::model::TrackKind;
        use crate::ui_test::Harness;
        let mut p = Project::empty();
        p.add_track(TrackKind::Instrument);
        p.add_track(TrackKind::Instrument);
        let n = p.tracks.len();
        p.selected_track = n - 1;
        let mut s = crate::UiState::new();
        let meters = Meters::default();
        let mut h = Harness::new(1400.0, 600.0);
        h.settle(|ui| show(ui, &mut p, &mut s, &meters));
        // The last track goes; the rows laid out for it this frame must not be drawn.
        h.key(Key::Delete, |ui| show(ui, &mut p, &mut s, &meters));
        assert_eq!(p.tracks.len(), n - 1);
        h.settle(|ui| show(ui, &mut p, &mut s, &meters));
    }

    #[test]
    fn volume_slider_round_trips_and_puts_unity_two_thirds_up() {
        assert_eq!(db_to_slider(0.0), SLIDER_UNITY);
        assert_eq!(db_to_slider(-60.0), 0.0);
        assert_eq!(db_to_slider(6.0), 1.0);
        for db in [-60.0, -30.0, -12.5, -0.1, 0.0, 2.0, 6.0] {
            assert!((slider_to_db(db_to_slider(db)) - db).abs() < 1e-3, "{db}");
        }
    }

    #[test]
    fn the_toolbar_keeps_snap_and_the_tool_menu_when_narrow() {
        let all = SNAP_W + DRAG_W + TOOLS_W;
        let f = fit_toolbar(all);
        assert!(f.snap && f.drag && f.tools);
        let f = fit_toolbar(all - 1.0);
        assert!(f.snap && f.tools && !f.drag, "Drag goes first");
        let f = fit_toolbar(SNAP_W + TOOLS_W);
        assert!(f.snap && f.tools && !f.drag);
        let f = fit_toolbar(SNAP_W);
        assert!(f.snap && !f.tools && !f.drag);
        assert_eq!(
            fit_toolbar(-50.0),
            ToolbarFit {
                snap: false,
                drag: false,
                tools: false,
            }
        );
    }

    #[test]
    fn rows_follow_each_tracks_height() {
        let mut p = Project::demo();
        p.tracks.truncate(3);
        p.tracks[1].height = 2.0;
        p.tracks[2].height = 0.5;
        let rows = row_layout(&p, 56.0);
        assert_eq!(rows, vec![(0.0, 56.0), (56.0, 112.0), (168.0, 28.0)]);
        assert_eq!(row_at(&rows, 0.0), Some(0));
        assert_eq!(row_at(&rows, 167.9), Some(1));
        assert_eq!(row_at(&rows, 168.0), Some(2));
        assert_eq!(row_at(&rows, 196.0), None);
        assert_eq!(row_at(&rows, -1.0), None);
    }

    #[test]
    fn the_tracks_of_a_folded_folder_take_no_room() {
        let mut p = Project::demo();
        p.tracks.truncate(4);
        let folder = p.create_track_stack(1); // wraps track 1: [t0, F, t1, t2, t3]
        assert_eq!(folder, 1);
        let open = row_layout(&p, 56.0);
        assert_eq!(open.len(), 5);
        assert!(open.iter().all(|&(_, h)| h == 56.0));

        p.tracks[1].folded = true;
        let folded = row_layout(&p, 56.0);
        assert_eq!(folded[2].1, 0.0, "the wrapped track is hidden");
        assert_eq!(
            folded[3],
            (112.0, 56.0),
            "the next track moves up under the folder"
        );
        assert_eq!(row_at(&folded, 100.0), Some(1));
        assert_eq!(row_at(&folded, 112.0), Some(3), "hidden rows are never hit");
    }

    #[test]
    fn dragging_the_playhead_follows_the_pointer_and_stops_at_the_start() {
        // Lanes start at x = 100, scrolled by 50 px, 20 px per beat.
        assert_eq!(playhead_beat(150.0, 100.0, 50.0, 20.0), 5.0);
        assert_eq!(playhead_beat(100.0, 100.0, 0.0, 20.0), 0.0);
        // Dragged left of the lanes: pinned to the beginning of the song.
        assert_eq!(playhead_beat(20.0, 100.0, 0.0, 20.0), 0.0);
    }

    #[test]
    fn solo_silences_every_other_track_until_released() {
        let mut p = Project::demo();
        assert!((0..p.tracks.len()).all(|i| !silenced_by_solo(&p, i)));
        p.tracks[2].solo = true;
        assert!(!silenced_by_solo(&p, 2));
        assert!((0..p.tracks.len())
            .filter(|&i| i != 2)
            .all(|i| silenced_by_solo(&p, i)));
        // Soloing a second track adds it to what plays.
        p.tracks[4].solo = true;
        assert!(!silenced_by_solo(&p, 4) && silenced_by_solo(&p, 0));
        p.tracks[2].solo = false;
        p.tracks[4].solo = false;
        assert!((0..p.tracks.len()).all(|i| !silenced_by_solo(&p, i)));
    }

    #[test]
    fn beat_snap_follows_the_signature_beat() {
        // 4/4: a beat is a quarter note, as before.
        assert_eq!(snap_step(Snap::Beat, 20.0, 4.0, 1.0), 1.0);
        assert_eq!(snap_step(Snap::Smart, 20.0, 4.0, 1.0), 1.0);
        assert_eq!(snap_step(Snap::Bar, 20.0, 4.0, 1.0), 4.0);
        // 6/8: a beat is an eighth (0.5 quarter notes), a bar 3 quarter notes.
        assert_eq!(snap_step(Snap::Beat, 20.0, 3.0, 0.5), 0.5);
        assert_eq!(snap_step(Snap::Bar, 20.0, 3.0, 0.5), 3.0);
        // Smart snaps to eighths once an eighth is at least 12 px wide, else to bars.
        assert_eq!(snap_step(Snap::Smart, 24.0, 3.0, 0.5), 0.5);
        assert_eq!(snap_step(Snap::Smart, 20.0, 3.0, 0.5), 3.0);
        // Divisions are 16th notes in any signature.
        assert_eq!(snap_step(Snap::Division, 20.0, 3.0, 0.5), 0.25);
        assert_eq!(snap_step(Snap::Smart, 60.0, 3.0, 0.5), 0.25);
    }

    /// A 0.5 s mono 16-bit 44.1 kHz WAV: a square wave at `amp` (0..1), or silence for 0.
    fn write_square_wav(path: &std::path::Path, amp: f32) {
        let n = 22_050u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + n * 2).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&44_100u32.to_le_bytes());
        b.extend_from_slice(&88_200u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(n * 2).to_le_bytes());
        for i in 0..n {
            let s = if (i / 50) % 2 == 0 { amp } else { -amp };
            b.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    /// Runs `poll_normalize` until the scan is over (it reads the file on another thread).
    fn wait_for_normalize(p: &mut Project, s: &mut UiState) {
        let ctx = egui::Context::default();
        let began = std::time::Instant::now();
        while !s.tracks.normalize_jobs.is_empty() {
            assert!(began.elapsed().as_secs() < 10, "the scan never finished");
            poll_normalize(&ctx, p, s);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn project_with_wav(amp: f32) -> (Project, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("take.wav");
        write_square_wav(&wav, amp);
        let mut p = Project::empty();
        crate::import::import_audio(&mut p, &wav, &egui::Context::default()).unwrap();
        (p, dir)
    }

    #[test]
    fn normalize_reads_the_file_in_the_background_and_sets_the_gain() {
        let (mut p, _dir) = project_with_wav(0.25);
        let (t, mut s) = (p.tracks.len() - 1, UiState::new());
        start_normalize(&p, &mut s, t, 0);
        assert_eq!(s.tracks.normalize_jobs.len(), 1);
        assert_eq!(s.status.as_deref(), Some("Normalizing…"));
        start_normalize(&p, &mut s, t, 0);
        assert_eq!(s.tracks.normalize_jobs.len(), 1, "one scan per region");
        wait_for_normalize(&mut p, &mut s);
        // A peak of 0.25 needs +12.04 dB to reach full scale.
        let gain = p.tracks[t].regions[0].params.gain_db;
        assert!((gain - 12.04).abs() < 0.05, "gain {gain}");
        assert_eq!(s.status, None, "the message goes away when it is done");
    }

    #[test]
    fn normalize_reports_silence_and_regions_without_audio() {
        let (mut p, _dir) = project_with_wav(0.0);
        let (t, mut s) = (p.tracks.len() - 1, UiState::new());
        start_normalize(&p, &mut s, t, 0);
        wait_for_normalize(&mut p, &mut s);
        assert_eq!(p.tracks[t].regions[0].params.gain_db, 0.0);
        assert_eq!(s.status.as_deref(), Some("Normalize: the region is silent"));
        // A MIDI region has nothing to scan.
        let mut s = UiState::new();
        start_normalize(&Project::demo(), &mut s, 0, 0);
        assert!(s.tracks.normalize_jobs.is_empty());
        assert_eq!(
            s.status.as_deref(),
            Some("Normalize: this region has no recorded audio")
        );
    }

    #[test]
    fn a_scan_for_a_region_that_has_gone_changes_nothing() {
        let (mut p, _dir) = project_with_wav(0.25);
        let (t, mut s) = (p.tracks.len() - 1, UiState::new());
        start_normalize(&p, &mut s, t, 0);
        // The user deletes the region while the file is being read.
        p.tracks[t].regions.clear();
        wait_for_normalize(&mut p, &mut s);
        assert!(p.tracks[t].regions.is_empty());
        assert_eq!(s.status, None);
    }

    #[test]
    fn the_fade_tool_measures_from_the_edge_in_sixteenths() {
        // A region from beat 8, 4 beats long.
        assert_eq!(fade_for_drag(9.0, 8.0, 4.0, true), 1.0);
        assert_eq!(
            fade_for_drag(9.1, 8.0, 4.0, true),
            1.0,
            "snaps to 1/16 notes"
        );
        assert_eq!(fade_for_drag(9.2, 8.0, 4.0, true), 1.25);
        assert_eq!(fade_for_drag(11.0, 8.0, 4.0, false), 1.0, "from the end");
        assert_eq!(
            fade_for_drag(7.0, 8.0, 4.0, true),
            0.0,
            "not before the start"
        );
        assert_eq!(
            fade_for_drag(13.0, 8.0, 4.0, false),
            0.0,
            "not past the end"
        );
        assert_eq!(
            fade_for_drag(30.0, 8.0, 4.0, true),
            4.0,
            "not longer than the region"
        );
        assert_eq!(fade_for_drag(-5.0, 8.0, 4.0, false), 4.0);
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn shift_click_adds_and_takes_out_and_the_primary_is_always_selected() {
        let mut v = TracksView::default();
        v.toggle(0, 0);
        assert_eq!(v.selected, Some((0, 0)), "the first one is primary");
        v.toggle(1, 2);
        v.toggle(0, 1);
        assert_eq!(v.selection(), vec![(0, 0), (0, 1), (1, 2)]);
        // Taking the primary out makes the most recent one primary.
        v.toggle(0, 0);
        assert_eq!(v.selected, Some((0, 1)));
        assert_eq!(v.selection(), vec![(0, 1), (1, 2)]);
        v.toggle(0, 1);
        v.toggle(1, 2);
        assert_eq!(v.selected, None);
        assert!(v.selection().is_empty());
    }

    #[test]
    fn a_click_inside_a_selection_makes_that_region_primary_and_keeps_the_rest() {
        let mut v = TracksView::default();
        v.select_many(&[(0, 0), (0, 1), (1, 0)]);
        v.make_primary(1, 0);
        assert_eq!(v.selected, Some((1, 0)));
        assert_eq!(v.selection(), vec![(0, 0), (0, 1), (1, 0)]);
        // A region that is not selected is ignored here (the caller selects it alone instead).
        v.make_primary(5, 5);
        assert_eq!(v.selected, Some((1, 0)));
    }

    #[test]
    fn select_many_keeps_the_first_as_primary_and_drops_repeats() {
        let mut v = TracksView::default();
        v.select_many(&[(2, 1), (0, 0), (2, 1)]);
        assert_eq!(v.selected, Some((2, 1)));
        assert_eq!(v.extra, vec![(0, 0)]);
        v.select_many(&[]);
        assert!(v.selected.is_none() && v.extra.is_empty());
    }

    #[test]
    fn select_all_selects_every_region_of_every_track() {
        let mut p = Project::demo();
        let count: usize = p.tracks.iter().map(|t| t.regions.len()).sum();
        let mut v = TracksView::default();
        v.select_all(&p);
        assert_eq!(v.selection().len(), count);
        assert!(count > 1, "the demo project has several regions");
        // An empty project selects nothing.
        p.tracks.iter_mut().for_each(|t| t.regions.clear());
        v.select_all(&p);
        assert!(v.selected.is_none());
    }

    #[test]
    fn prune_forgets_regions_that_are_gone() {
        let mut p = Project::demo();
        let mut v = TracksView::default();
        v.select_many(&[(0, 0), (1, 0), (99, 0)]);
        v.prune(&p);
        assert_eq!(
            v.selection(),
            vec![(0, 0), (1, 0)],
            "track 99 does not exist"
        );
        // The primary one goes: another selected region takes its place.
        p.tracks[0].regions.clear();
        v.prune(&p);
        assert_eq!(v.selected, Some((1, 0)));
        assert!(v.extra.is_empty());
    }

    #[test]
    fn a_selected_region_is_drawn_as_selected() {
        let mut v = TracksView::default();
        v.select_many(&[(0, 1), (2, 0)]);
        assert!(v.is_selected(0, 1) && v.is_selected(2, 0));
        assert!(!v.is_selected(0, 0));
    }
}

/// The picture inside a region follows its edits: a cut, a trim, a join, Undo and Redo change
/// which part of the file's waveform it shows, and a shortened MIDI region scales its notes
/// to the ones it still shows.
#[cfg(test)]
mod preview_tests {
    use super::*;
    use crate::model::{AudioRegion, Note, RegionParams};

    /// 8 seconds whose overview rises evenly: bin `i` of `BINS` holds level `i / BINS`, so a
    /// level tells which second of the file is drawn.
    const SECS: f64 = 8.0;
    const BINS: usize = 800;

    fn ramp_audio() -> AudioRegion {
        let a = AudioRegion {
            file: prowave_engine::AudioFile {
                path: "/prowave-preview-test/ramp.wav".into(),
                sample_rate: 48_000,
                frames: (SECS * 48_000.0) as u64,
                channels: 2,
            },
            peaks: Default::default(),
            flex: None,
        };
        let peaks = (0..BINS)
            .map(|i| {
                let v = i as f32 / BINS as f32;
                (-v, v)
            })
            .collect();
        let _ = a.peaks.set(peaks);
        a
    }

    /// One audio track with the 8-second file on bars 1-5 (120 bpm, 4/4: 2 seconds a bar).
    fn project() -> Project {
        let mut p = Project::empty();
        p.tracks.clear();
        let mut t = Track::new("Audio 1", TrackKind::Audio);
        t.regions.push(Region {
            missing: None,
            name: "ramp".into(),
            start_bar: 1.0,
            len_bars: 4.0,
            notes: Vec::new(),
            controls: Vec::new(),
            folder: None,
            generated: None,
            audio: Some(ramp_audio()),
            params: RegionParams::default(),
        });
        p.tracks.push(t);
        p
    }

    /// The file second drawn at `u` seconds into region `ri` (the top of a 10 ms column),
    /// `None` where the region shows nothing.
    fn drawn_at(p: &Project, ri: usize, u: f64) -> Option<f64> {
        let map = p.tempo_map();
        let r = &p.tracks[0].regions[ri];
        let span = AudioSpan::of(p, &map, r).unwrap();
        let peaks = r.audio.as_ref().unwrap().peaks.get().unwrap();
        span.peak_range(peaks, u, u + 0.01)
            .map(|(_, hi)| f64::from(hi) * SECS)
    }

    fn near(got: Option<f64>, want: f64) -> bool {
        got.is_some_and(|g| (g - want).abs() < 0.03)
    }

    #[test]
    fn a_cut_region_draws_the_part_of_the_file_it_plays() {
        let mut p = project();
        assert!(near(drawn_at(&p, 0, 0.0), 0.0));
        assert!(near(drawn_at(&p, 0, 5.0), 5.0));
        // Cut at bar 2 (2 seconds in).
        assert!(p.split_region(0, 0, 4.0));
        assert!(
            near(drawn_at(&p, 0, 1.5), 1.5),
            "the left part: the file's start"
        );
        assert_eq!(
            drawn_at(&p, 0, 2.5),
            None,
            "nothing after the left part's end"
        );
        assert!(
            near(drawn_at(&p, 1, 0.0), 2.0),
            "the right part starts where the cut was: {:?}",
            drawn_at(&p, 1, 0.0)
        );
        assert!(near(drawn_at(&p, 1, 5.9), 7.9), "and ends with the file");
    }

    #[test]
    fn a_trimmed_end_shows_less_of_the_file_and_never_squeezes_it() {
        let mut p = project();
        p.tracks[0].regions[0].len_bars = 2.0;
        assert!(near(drawn_at(&p, 0, 3.0), 3.0), "same place, same picture");
        assert_eq!(drawn_at(&p, 0, 4.5), None, "past the new end");
        // Lengthened past the file: the file plays once, then nothing.
        p.tracks[0].regions[0].len_bars = 6.0;
        assert!(near(drawn_at(&p, 0, 7.9), 7.9));
        assert_eq!(drawn_at(&p, 0, 9.0), None);
    }

    #[test]
    fn join_undo_and_redo_bring_the_picture_back() {
        let mut p = project();
        let mut h = crate::undo::History::new(&p);
        assert!(p.split_region(0, 0, 4.0));
        assert!(h.record(&p));
        assert!(near(drawn_at(&p, 1, 0.0), 2.0));
        // Join: one region again, the whole file from its start.
        assert!(p.join_regions(0, 0));
        assert_eq!(p.tracks[0].regions.len(), 1);
        assert!(near(drawn_at(&p, 0, 0.0), 0.0));
        assert!(near(drawn_at(&p, 0, 6.0), 6.0));
        // Undo the join: the cut parts and their pictures.
        assert!(h.undo(&mut p));
        assert_eq!(p.tracks[0].regions.len(), 2);
        assert!(near(drawn_at(&p, 1, 0.0), 2.0));
        // Undo the cut, then redo it.
        assert!(h.undo(&mut p));
        assert_eq!(p.tracks[0].regions.len(), 1);
        assert!(near(drawn_at(&p, 0, 3.0), 3.0));
        assert!(h.redo(&mut p));
        assert!(near(drawn_at(&p, 1, 1.0), 3.0));
    }

    #[test]
    fn a_reversed_region_draws_its_part_of_the_file_back_to_front() {
        let mut p = project();
        assert!(p.split_region(0, 0, 4.0));
        p.tracks[0].regions[1].params.reverse = true;
        // The right part plays seconds 2-8 backwards: 8 first, 2 last.
        assert!(near(drawn_at(&p, 1, 0.0), 8.0));
        assert!(near(drawn_at(&p, 1, 5.98), 2.0));
    }

    #[test]
    fn a_tempo_change_inside_a_region_keeps_its_picture_on_time() {
        let mut p = project();
        // 60 bpm from bar 2: bar 2 lasts 4 seconds, so bars 1-5 run past the file's end.
        p.add_tempo_change(4.0, 60.0);
        let map = p.tempo_map();
        let span = AudioSpan::of(&p, &map, &p.tracks[0].regions[0]).unwrap();
        // Beat 6 (halfway through bar 2) is 2 + 2 seconds in.
        assert!((span.secs_at(6.0) - 4.0).abs() < 1e-9);
        assert!((span.beats_at(4.0) - 6.0).abs() < 1e-9);
        assert!(near(drawn_at(&p, 0, span.secs_at(6.0)), 4.0));
    }

    #[test]
    fn a_shortened_midi_region_scales_to_the_notes_it_still_shows() {
        let note = |start: f32, pitch: u8| Note {
            start,
            len: 1.0,
            pitch,
            vel: 100,
            mute: false,
            lyric: Default::default(),
        };
        let mut p = Project::demo();
        let r = &mut p.tracks[0].regions[0];
        r.notes = vec![note(0.0, 60), note(2.0, 64), note(12.0, 96)];
        r.len_bars = 4.0;
        assert_eq!(pitch_range(visible_notes(r, 16.0)), (60.0, 96.0));
        r.len_bars = 1.0;
        assert_eq!(
            pitch_range(visible_notes(r, 4.0)),
            (60.0, 64.0),
            "the note cut off no longer sets the scale"
        );
    }
}

/// Pictures of the Tracks area in the Hardware look, to compare with the design mockup.
/// Ignored; they check nothing:
/// `MANUAL_PNG_DIR=… cargo test -p prowave-app tracks_area_pictures -- --ignored`
#[cfg(test)]
mod pictures {
    use super::*;
    use crate::model::{AudioRegion, Note};
    use std::path::PathBuf;

    fn out(name: &str) -> PathBuf {
        // PROWAVE_PNG_DIR, else MANUAL_PNG_DIR, else a folder in the temp directory.
        let dir = std::env::var_os("PROWAVE_PNG_DIR")
            .or_else(|| std::env::var_os("MANUAL_PNG_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-manual"));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    /// A made-up recording with an overview, for drawing (nothing reads the file).
    fn fake_audio(seconds: f64, seed: u32, punchy: bool) -> AudioRegion {
        let rate = 48_000u32;
        let file = prowave_engine::AudioFile {
            path: "picture.wav".into(),
            sample_rate: rate,
            frames: (seconds * f64::from(rate)) as u64,
            channels: 2,
        };
        let n = 2000;
        let mut s = seed;
        let peaks: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = (s >> 16) as f32 / 65_536.0;
                let t = i as f32 / n as f32 * seconds as f32;
                let env = if punchy {
                    let ph = (t * 2.0).fract();
                    0.3 + 0.65 * (-ph * 3.2).exp()
                } else {
                    let ph = (t / 2.0).fract();
                    if ph < 0.06 || (t as i32 % 4 == 1 && ph > 0.55) {
                        0.02
                    } else {
                        0.3 + 0.55 * (ph * 9.0 + t).sin().abs()
                    }
                };
                let a = env * (0.6 + 0.4 * noise);
                (-a, a)
            })
            .collect();
        let region = AudioRegion {
            file,
            peaks: Default::default(),
            flex: None,
        };
        let _ = region.peaks.set(peaks);
        region
    }

    fn note(start: f32, len: f32, pitch: u8) -> Note {
        Note {
            start,
            len,
            pitch,
            vel: 96,
            mute: false,
            lyric: Default::default(),
        }
    }

    /// C, A minor, F, G: one chord a bar.
    const CHORDS: [[u8; 3]; 4] = [[60, 64, 67], [57, 60, 64], [53, 57, 60], [55, 59, 62]];

    /// The electric piano's line: five notes a bar on the chord's tones.
    fn ep_notes() -> Vec<Note> {
        let mut out = Vec::new();
        for bar in 0..12 {
            let c = CHORDS[bar % 4];
            let t = bar as f32 * 4.0;
            for (at, len, pitch) in [
                (0.0, 0.75, c[0] + 12),
                (1.0, 0.45, c[1] + 12),
                (1.5, 0.9, c[2] + 12),
                (2.5, 0.45, c[1] + 12),
                (3.0, 0.9, c[2] + 7),
            ] {
                out.push(note(t + at, len, pitch));
            }
        }
        out
    }

    /// The grand piano: the chord stacked on every beat.
    fn piano_notes() -> Vec<Note> {
        let mut out = Vec::new();
        for bar in 0..12 {
            let c = CHORDS[bar % 4];
            for beat in 0..4 {
                let t = bar as f32 * 4.0 + beat as f32;
                for pitch in c {
                    out.push(note(t, 0.8, pitch));
                }
            }
        }
        out
    }

    /// The pad: each chord held for two bars, with its root an octave down.
    fn pad_notes() -> Vec<Note> {
        let mut out = Vec::new();
        for k in 0..6 {
            let c = CHORDS[k % 4];
            let t = k as f32 * 8.0;
            for pitch in [c[0] - 12, c[0], c[1], c[2]] {
                out.push(note(t, 7.75, pitch));
            }
        }
        out
    }

    /// Kick, snare and hi-hat for twelve bars.
    fn drum_notes() -> Vec<Note> {
        let mut out = Vec::new();
        let hit = |start: f32, pitch: u8| Note {
            start,
            len: 0.12,
            pitch,
            vel: 100,
            mute: false,
            lyric: Default::default(),
        };
        for bar in 0..12 {
            for step in 0..16 {
                let t = bar as f32 * 4.0 + step as f32 * 0.25;
                if step % 2 == 0 || step == 7 {
                    out.push(hit(t, 42));
                }
                if step == 4 || step == 12 {
                    out.push(hit(t, 38));
                }
                if step == 0 || step == 6 || step == 8 {
                    out.push(hit(t, 36));
                }
            }
        }
        out
    }

    /// The demo song as the mockup shows it: a Drummer pattern, two recordings, the cycle on
    /// from bar 3, the playhead in bar 4 and the first track selected.
    fn picture_project() -> (Project, crate::UiState) {
        let mut p = Project::demo();
        p.tracks[0].regions[0].notes = ep_notes();
        p.tracks[1].regions[0].notes = piano_notes();
        p.tracks[2].regions[0].notes = pad_notes();
        p.tracks[3].regions[0].notes = drum_notes();
        // Twelve bars of 4/4 at the demo's tempo.
        let secs = 48.0 * 60.0 / f64::from(p.tempo);
        p.tracks[4].regions[0].audio = Some(fake_audio(secs, 31, true));
        p.tracks[5].regions[0].audio = Some(fake_audio(secs, 97, false));
        p.cycle = true;
        p.cycle_start = 8.0;
        p.cycle_end = 88.0;
        p.position_beats = 15.8;
        p.selected_track = 0;
        let mut s = crate::UiState::new();
        s.tracks.select_only(0, 0);
        (p, s)
    }

    fn render(name: &str, w: f32, h: f32, p: &mut Project, s: &mut crate::UiState) {
        let meters = crate::mixer::Meters::default();
        crate::ui_test::render_png(
            w,
            h,
            1.0,
            theme::BONE(),
            |ui| {
                theme::apply(ui.ctx());
                show(ui, p, s, &meters)
            },
            &out(name),
        )
        .unwrap();
    }

    #[test]
    #[ignore = "pictures for the design review"]
    fn tracks_area_pictures() {
        // The main view, at the mockup's size.
        let (mut p, mut s) = picture_project();
        render("tracks-main.png", 1670.0, 560.0, &mut p, &mut s);

        // States: a muted region, a soloed track (the others go dim), R armed, I on, track 1
        // muted, the cycle off; the global tracks and the Master strip shown.
        let (mut p, mut s) = picture_project();
        p.tracks[1].regions[0].params.mute = true;
        p.tracks[2].solo = true;
        p.tracks[4].rec = true;
        p.tracks[5].monitor = true;
        p.tracks[0].mute = true;
        p.cycle = false;
        p.show_master_track = true;
        s.tracks.show_markers = true;
        s.tracks.show_signatures = true;
        s.tracks.show_tempo = true;
        p.add_marker(0.0);
        let verse = p.add_marker(16.0);
        p.markers[verse].name = "Verse".into();
        p.add_tempo_change(32.0, 132.0);
        p.set_signature_from(9, (3, 4));
        s.tracks.select_many(&[(4, 0), (5, 0)]);
        p.selected_track = 4;
        render("tracks-states.png", 1670.0, 760.0, &mut p, &mut s);

        // Short tracks on a 13-inch screen.
        let (mut p, mut s) = picture_project();
        s.tracks.row_h = 40.0;
        s.tracks.bar_w = 48.0;
        render("tracks-compact.png", 1140.0, 420.0, &mut p, &mut s);
    }

    /// The Tool menu (T) open over the regions, the left-click and Command-click tool keys in
    /// the toolbar, and every tool's pointer along the bottom.
    /// `PROWAVE_PNG_DIR=… cargo test -p prowave-app tracks_area_tool_pictures -- --ignored`
    #[test]
    #[ignore = "pictures for the design review"]
    fn tracks_area_tool_pictures() {
        let (mut p, mut s) = picture_project();
        s.tracks.tool = TrackTool::Scissors;
        s.tracks.cmd_tool = TrackTool::Pencil;
        s.tracks.tool_menu = Some(pos2(900.0, 160.0));
        let meters = crate::mixer::Meters::default();
        crate::ui_test::render_png(
            1670.0,
            560.0,
            1.0,
            theme::BONE(),
            |ui| {
                theme::apply(ui.ctx());
                show(ui, &mut p, &mut s, &meters);
                let layer = egui::LayerId::new(egui::Order::Tooltip, id("shot_cursors"));
                let g = ui.ctx().layer_painter(layer);
                for (k, (tool, ..)) in TrackTool::ALL.into_iter().enumerate() {
                    let at = pos2(480.0 + k as f32 * 60.0, 500.0);
                    hw_tracks::tool_cursor(&g, at, tool.glyph());
                }
            },
            &out("tracks-tools.png"),
        )
        .unwrap();
    }

    /// The two recordings edited: the first cut four bars in with its left part trimmed by a
    /// bar, the second trimmed to half its length. Each part shows what it plays.
    /// `PROWAVE_PNG_DIR=… cargo test -p prowave-app tracks_area_edit_pictures -- --ignored`
    #[test]
    #[ignore = "pictures for the design review"]
    fn tracks_area_edit_pictures() {
        let (mut p, mut s) = picture_project();
        let at = p.region_start_beats(&p.tracks[4].regions[0]) + 16.0;
        assert!(p.split_region(4, 0, at));
        p.tracks[4].regions[0].len_bars -= 1.0;
        p.tracks[5].regions[0].len_bars /= 2.0;
        s.tracks.select_only(4, 1);
        render("tracks-edited.png", 1670.0, 560.0, &mut p, &mut s);
    }
}

/// Regions, the cycle and markers fill the time grid exactly: drawn from the pixel edge of
/// their start to the pixel edge of their end, with nothing of the background between two
/// that touch, at any zoom, scroll and screen density. Rendered offscreen and read back pixel
/// by pixel. `PROWAVE_PNG_DIR=… cargo test -p prowave-app grid_fill` also writes the pictures.
#[cfg(test)]
#[path = "colour_spectrogram_pictures.rs"]
mod colour_spectrogram_pictures;

#[cfg(test)]
mod grid_fill_tests {
    use super::*;
    use crate::model::RegionParams;

    /// Regions on one track that each start where the one before ends: (start bar, bars).
    /// Bars 2, 3, 5 and 8 start on a bar line; the others on beats and sixteenths.
    const CHAIN: [(f32, f32); 8] = [
        (2.0, 1.0),
        (3.0, 0.5),
        (3.5, 0.25),
        (3.75, 1.25),
        (5.0, 2.0),
        (7.0, 0.0625),
        (7.0625, 0.9375),
        (8.0, 3.0),
    ];

    fn empty_region(start_bar: f32, len_bars: f32) -> Region {
        Region {
            missing: None,
            name: String::new(),
            start_bar,
            len_bars,
            notes: Vec::new(),
            controls: Vec::new(),
            folder: None,
            audio: None,
            params: RegionParams::default(),
            generated: None,
        }
    }

    fn rig() -> (Project, crate::UiState) {
        let mut p = Project::empty();
        assert_eq!(p.tracks[0].kind, TrackKind::Instrument);
        p.add_track(TrackKind::Instrument);
        p.tracks[0].regions = CHAIN.iter().map(|&(s, l)| empty_region(s, l)).collect();
        p.tracks[1].regions.clear();
        p.selected_track = 1;
        // The cycle from bar 3 to bar 5, two markers that touch (bar 2 and bar 4).
        (p.cycle, p.cycle_start, p.cycle_end) = (true, 8.0, 16.0);
        p.add_marker(4.0);
        p.add_marker(12.0);
        let mut s = crate::UiState::new();
        s.tracks.show_markers = true;
        (p, s)
    }

    /// Light against the region bodies, the plates and the ink lines.
    fn lum(img: &image::RgbaImage, x: i64, y: i64) -> f32 {
        let c = img.get_pixel(x as u32, y as u32).0;
        0.299 * f32::from(c[0]) + 0.587 * f32::from(c[1]) + 0.114 * f32::from(c[2])
    }

    fn is_orange(img: &image::RgbaImage, x: i64, y: i64) -> bool {
        let c = img.get_pixel(x as u32, y as u32).0;
        c[0] > 200 && c[1] > 60 && c[1] < 170 && c[2] < 90
    }

    fn rect_of(ctx: &egui::Context, key: impl std::hash::Hash + std::fmt::Debug) -> Rect {
        ctx.read_response(id(key)).expect("drawn").rect
    }

    #[test]
    fn regions_markers_and_the_cycle_fill_the_grid_with_no_gap_at_any_zoom() {
        // (points per bar, scroll in points, pixels per point)
        let views: [(f32, f32, f32); 7] = [
            (96.0, 0.0, 1.0),
            (61.7, 13.3, 1.0),
            (37.3, 7.77, 2.0),
            (150.0, 0.0, 2.0),
            (233.1, 101.37, 2.0),
            (77.7, 33.33, 1.5),
            (600.0, 1010.9, 1.0),
        ];
        // Measured on the Bone look's light floor.
        theme::set_for_thread(Some(theme::Theme::Bone));
        let dark = 140.0;
        let light = 150.0;
        let mut checked = (0, 0, 0);
        for (n, &(bar_w, scroll, ppp)) in views.iter().enumerate() {
            let (mut p, mut s) = rig();
            s.tracks.bar_w = bar_w;
            s.tracks.scroll_x = scroll;
            let meters = crate::mixer::Meters::default();
            let (img, ctx) =
                crate::ui_test::render_image(1400.0, 420.0, ppp, theme::BONE(), |ui| {
                    theme::apply(ui.ctx());
                    show(ui, &mut p, &mut s, &meters)
                })
                .unwrap();
            if let Some(dir) = std::env::var_os("PROWAVE_PNG_DIR") {
                let _ = std::fs::create_dir_all(&dir);
                img.save(std::path::Path::new(&dir).join(format!("grid-fill-{n}.png")))
                    .unwrap();
            }
            let lanes = rect_of(&ctx, "lanes");
            let beat_px = f64::from(bar_w) / 4.0;
            // The pixel column where `beat` begins: the same rounding the drawing uses.
            let edge = |beat: f64| {
                ((f64::from(lanes.left()) + beat * beat_px - f64::from(scroll)) * f64::from(ppp))
                    .round() as i64
            };
            let (lo, hi) = (
                (lanes.left() * ppp).ceil() as i64 + 1,
                (lanes.right() * ppp).floor() as i64 - 2,
            );
            let inside = |x: i64| x >= lo && x <= hi;
            let row = |y: f32| (y * ppp) as i64;
            let view = format!("view {n}: {bar_w} points a bar, scroll {scroll}, {ppp}x");

            // The regions: under each one's header every column from its start to its end is
            // region; the columns just before the first and after the last are background.
            let y = row(lanes.top() + 45.0);
            for (i, &(sb, len)) in CHAIN.iter().enumerate() {
                let (b0, b1) = (f64::from(sb - 1.0) * 4.0, f64::from(sb + len - 1.0) * 4.0);
                let (x0, x1) = (edge(b0), edge(b1));
                for x in x0.max(lo)..x1.min(hi + 1) {
                    let l = lum(&img, x, y);
                    assert!(
                        l < dark,
                        "{view}: region {i} column {x} ({x0}..{x1}) is {l}"
                    );
                    checked.0 += 1;
                }
                if i == 0 && inside(x0 - 1) {
                    let l = lum(&img, x0 - 1, y);
                    assert!(
                        l > light,
                        "{view}: before the first region, column {} is {l}",
                        x0 - 1
                    );
                }
                // The last one ends on bar 11: its line fills the next column(s), the floor
                // comes after it.
                let line_w = ppp.round().max(1.0) as i64;
                if i + 1 == CHAIN.len() && inside(x1 + line_w) {
                    let on = lum(&img, x1, y);
                    let l = lum(&img, x1 + line_w, y);
                    assert!(
                        on > dark - 20.0 && on + 15.0 < l,
                        "{view}: bar 11's line is {on}"
                    );
                    assert!(
                        l > light,
                        "{view}: after the last region, column {} is {l}",
                        x1 + line_w
                    );
                }
            }

            // The bar lines on the empty second track: each begins on its bar's edge, the
            // column a region starting on that bar starts on.
            let y2 = row(lanes.top() + 70.0 + 35.0);
            for bar in 0..40 {
                let x = edge(f64::from(bar) * 4.0);
                if !inside(x - 1) || !inside(x + 1) {
                    continue;
                }
                let (before, on) = (lum(&img, x - 1, y2), lum(&img, x, y2));
                assert!(
                    on + 15.0 < before,
                    "{view}: bar {} line: column {x} is {on}, {before} before",
                    bar + 1
                );
                checked.1 += 1;
            }

            // The markers (bar 2 to 4 and bar 4 on): dark plates edge to edge.
            let markers = Rect::from_min_max(
                pos2(lanes.left(), lanes.top() - crate::marker_track::HEIGHT),
                pos2(lanes.right(), lanes.top()),
            );
            let ym = row(markers.top() + 4.0 + 1.5);
            let (m0, m1) = (edge(4.0), edge(12.0));
            if inside(m0 - 1) {
                assert!(
                    lum(&img, m0 - 1, ym) > light,
                    "{view}: before the first marker"
                );
            }
            for x in m0.max(lo)..(m1 + 30).min(hi + 1) {
                let l = lum(&img, x, ym);
                assert!(l < dark, "{view}: marker column {x} ({m0}, {m1}) is {l}");
                checked.2 += 1;
            }

            // The cycle band in the ruler: orange from the left locator's column to the right
            // locator's, not one column more.
            let yc = row(markers.top() - RULER_H + 2.0 + 6.5);
            let (c0, c1) = (edge(8.0), edge(16.0));
            if inside(c0 - 1) {
                assert!(
                    !is_orange(&img, c0 - 1, yc),
                    "{view}: orange before the cycle"
                );
                assert!(
                    lum(&img, c0, yc) < dark,
                    "{view}: the left locator's ink line"
                );
            }
            if inside(c1) && inside(c1 - 1) {
                assert!(
                    is_orange(&img, c1 - 1, yc),
                    "{view}: the cycle's last column"
                );
                assert!(!is_orange(&img, c1, yc), "{view}: orange after the cycle");
            }
        }
        assert!(
            checked.0 > 3000 && checked.1 > 20 && checked.2 > 200,
            "{checked:?}"
        );
    }
}

#[cfg(test)]
mod track_set_ctx_tests {
    use super::*;
    use crate::track_selection::{click, selected, Click};

    /// Six instrument tracks "T0".."T5", tracks 1 and 3 selected (3 in focus).
    fn rig() -> (Project, UiState) {
        let mut p = Project::empty();
        while p.tracks.len() < 6 {
            p.add_track(TrackKind::Instrument);
        }
        for (i, t) in p.tracks.iter_mut().enumerate() {
            t.name = format!("T{i}");
        }
        let mut s = UiState::new();
        click(&mut p, &mut s.tracks, 1, Click::Plain, false);
        click(&mut p, &mut s.tracks, 3, Click::Toggle, false);
        (p, s)
    }

    #[test]
    fn right_click_entries_on_a_selected_track_act_on_every_selected_track() {
        // Mute and Solo: both selected tracks, one undo step each.
        let (mut p, mut s) = rig();
        crate::track_selection::context_click(&mut p, &mut s.tracks, 1);
        assert_eq!(selected(&p, &s.tracks), vec![1, 3], "the set stays");
        let mut h = crate::undo::History::new(&p);
        apply_ctx(CtxAction::ToggleMute(1), &mut p, &mut s, 800.0);
        let muted: Vec<bool> = p.tracks.iter().map(|t| t.mute).collect();
        assert_eq!(muted, vec![false, true, false, true, false, false]);
        assert!(h.record(&p) && !h.record(&p), "one undo step");
        apply_ctx(CtxAction::ToggleSolo(1), &mut p, &mut s, 800.0);
        assert!(p.tracks[1].solo && p.tracks[3].solo && !p.tracks[2].solo);
        assert!(h.record(&p) && !h.record(&p), "one undo step");
        // Delete Track: both go.
        apply_ctx(CtxAction::DeleteTrack(1), &mut p, &mut s, 800.0);
        let names: Vec<&str> = p.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["T0", "T2", "T4", "T5"]);
        assert!(h.record(&p) && !h.record(&p), "one undo step");
        // Create Track Stack: both go under one stack.
        let (mut p, mut s) = rig();
        crate::track_selection::context_click(&mut p, &mut s.tracks, 3);
        apply_ctx(CtxAction::CreateStack(3), &mut p, &mut s, 800.0);
        let names: Vec<&str> = p.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["T0", "Folder 1", "T1", "T3", "T2", "T4", "T5"]);
        // A right-click on a track outside the set: that track alone.
        let (mut p, mut s) = rig();
        crate::track_selection::context_click(&mut p, &mut s.tracks, 5);
        assert_eq!(selected(&p, &s.tracks), vec![5]);
        apply_ctx(CtxAction::ToggleMute(5), &mut p, &mut s, 800.0);
        let muted: Vec<bool> = p.tracks.iter().map(|t| t.mute).collect();
        assert_eq!(muted, vec![false, false, false, false, false, true]);
    }
}
