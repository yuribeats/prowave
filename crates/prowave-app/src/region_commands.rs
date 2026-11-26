//! Logic Pro's region commands of the Tracks area that act on the whole selection: Set Region
//! Start / End to Playhead (Command-[ and Command-]), Split at Playhead without a selection,
//! resizing every selected region at once, Repeat (Command-R, with Logic's Repeat window),
//! Rename Regions (Shift-N, numbered as Logic numbers them), Reverse Region, the nudge value
//! (Edit > Nudge Value, Control-Option-T / D / B / M), the Region Inspector's settings on every
//! selected region, and the Inspector's Loop setting.
//!
//! Where this differs from Logic, and why:
//! - Repeat makes copies only (Logic also offers aliases of MIDI regions and clones of audio
//!   regions, which this app does not have). Its Adjustment offers Auto, Bar, Beat and Division;
//!   a grid value rounds the distance between the copies up to whole bars, beats or divisions
//!   (a sixteenth note), so each copy keeps its place in the bar.
//! - Rename Regions numbers the regions in the order they play (then by track) when the name
//!   ends with a number ("Bass 02" gives "Bass 02", "Bass 03", …); without a number they all
//!   take the name, as in Logic.
//! - The nudge value offers Logic's musical values (Tick, Division, Beat, Bar); its SMPTE frame,
//!   sample and millisecond values are not offered. A division is a sixteenth note.
//! - Inspector values that are dragged (Transpose, Velocity, Gain, Fade In and Fade Out) move
//!   every selected region by the same amount, each kept within its range; the others (Mute,
//!   Quantize, Curve, Reverse, X-Fade, Loop) are set to the same value. MIDI settings go to the
//!   selected MIDI regions and audio settings to the selected audio regions.

use crate::automation::AutoView;
use crate::hw::{self, Cap};
use crate::i18n::tr;
use crate::model::{Project, Region, RegionParams, TrackKind, MAX_REGION_GAIN, MIN_REGION_GAIN};
use crate::theme::{self, Face};
use crate::tracks_area::TracksView;
use egui::{pos2, vec2, Align2, Sense};

/// The shortest a region becomes, in quarter notes.
const MIN_LEN: f64 = 1.0 / 64.0;

/// The selected regions that are there.
fn live_selection(p: &Project, v: &TracksView) -> Vec<(usize, usize)> {
    v.selection()
        .into_iter()
        .filter(|&(t, r)| p.tracks.get(t).is_some_and(|tr| r < tr.regions.len()))
        .collect()
}

/// The regions a command on `clicked` acts on: the whole selection when `clicked` is part of it
/// (or there is no `clicked`: a key or a menu), else `clicked` alone, as in Logic.
fn targets(p: &Project, v: &TracksView, clicked: Option<(usize, usize)>) -> Vec<(usize, usize)> {
    match clicked {
        Some((t, r)) if !v.is_selected(t, r) => {
            if p.tracks.get(t).is_some_and(|tr| r < tr.regions.len()) {
                vec![(t, r)]
            } else {
                Vec::new()
            }
        }
        _ => live_selection(p, v),
    }
}

fn region(p: &Project, t: usize, r: usize) -> Option<&Region> {
    p.tracks.get(t)?.regions.get(r)
}

/// Whether `at` (quarter notes) is inside region `r` of track `t`, away from its edges.
fn inside(p: &Project, t: usize, r: usize, at: f64) -> bool {
    region(p, t, r).is_some_and(|reg| {
        at > p.region_start_beats(reg) + 1e-6 && at < p.region_end_beats(reg) - 1e-6
    })
}

/// Sets region `r` of track `t` to end at `end` (quarter notes), no shorter than [`MIN_LEN`].
/// Returns whether it changed.
fn set_end(p: &mut Project, t: usize, r: usize, end: f64) -> bool {
    let Some(reg) = region(p, t, r) else {
        return false;
    };
    let end = end.max(p.region_start_beats(reg) + MIN_LEN);
    let len_bars = ((p.beats_to_bar(end) + 1.0) as f32 - reg.start_bar).max(1.0 / 64.0);
    if (len_bars - reg.len_bars).abs() < 1e-7 {
        return false;
    }
    p.tracks[t].regions[r].len_bars = len_bars;
    p.song_rev += 1;
    true
}

// ------------------------------------------------------------ Set Region Start / End to Playhead

/// The selected regions whose start Set Region Start to Playhead moves: the playhead is inside
/// them, and they are not take folders, Pattern or Drummer regions or reversed audio (see
/// `Project::resize_region_start`).
fn start_targets(p: &Project, v: &TracksView) -> Vec<(usize, usize)> {
    let at = p.position_beats;
    live_selection(p, v)
        .into_iter()
        .filter(|&(t, r)| {
            let reg = &p.tracks[t].regions[r];
            inside(p, t, r, at)
                && reg.folder.is_none()
                && reg.generated.is_none()
                && !reg.params.reverse
        })
        .collect()
}

/// Whether Set Region Start to Playhead does anything.
pub(crate) fn can_set_start(p: &Project, v: &TracksView) -> bool {
    !start_targets(p, v).is_empty()
}

/// Set Region Start to Playhead (Command-[): every selected region the playhead is inside now
/// starts there and still ends where it did. What it plays stays where it was in the song, as
/// when its lower-left corner is dragged. Returns whether a region changed.
pub(crate) fn set_start_to_playhead(p: &mut Project, v: &TracksView) -> bool {
    let at = p.position_beats;
    let mut changed = false;
    for (t, r) in start_targets(p, v) {
        changed |= p.resize_region_start(t, r, at);
    }
    changed
}

/// Whether Set Region End to Playhead does anything: the playhead is inside a selected region.
pub(crate) fn can_set_end(p: &Project, v: &TracksView) -> bool {
    let at = p.position_beats;
    live_selection(p, v)
        .into_iter()
        .any(|(t, r)| inside(p, t, r, at))
}

/// Set Region End to Playhead (Command-]): every selected region the playhead is inside now
/// ends there, as when its lower-right corner is dragged. Returns whether a region changed.
pub(crate) fn set_end_to_playhead(p: &mut Project, v: &TracksView) -> bool {
    let at = p.position_beats;
    let mut changed = false;
    for (t, r) in live_selection(p, v) {
        if inside(p, t, r, at) {
            changed |= set_end(p, t, r, at);
        }
    }
    changed
}

// ------------------------------------------------------------ Split at Playhead

/// What Split at Playhead cuts: the selected regions under the playhead, or with no region
/// selected the regions under the playhead on the selected track (Logic).
fn split_targets(p: &Project, v: &TracksView) -> Vec<(usize, usize)> {
    let at = p.position_beats;
    let sel = live_selection(p, v);
    let pool = if sel.is_empty() {
        let t = p.selected_track;
        p.tracks.get(t).map_or(Vec::new(), |tr| {
            (0..tr.regions.len()).map(|r| (t, r)).collect()
        })
    } else {
        sel
    };
    pool.into_iter()
        .filter(|&(t, r)| p.can_split_region(t, r, at))
        .collect()
}

/// Whether Split at Playhead does anything.
pub(crate) fn can_split(p: &Project, v: &TracksView) -> bool {
    !split_targets(p, v).is_empty()
}

/// Split at Playhead (Command-T): the selected regions under the playhead are cut there; with
/// no region selected, the ones under the playhead on the selected track. The right pieces are
/// selected.
pub(crate) fn split_at_playhead(p: &mut Project, v: &mut TracksView) {
    if live_selection(p, v).is_empty() {
        let under = split_targets(p, v);
        if under.is_empty() {
            return;
        }
        v.select_many(&under);
    }
    crate::region_edit::split_at_playhead(p, v);
}

// ------------------------------------------------------------ resizing with the Pointer

/// The lower-left corner of region `clicked` let go at `at` (quarter notes): the region starts
/// there, and when it is selected every other selected region's start moves by the same
/// distance (Logic), each held within what it can show and none shorter than a 64th of a bar. Returns whether a region changed.
pub(crate) fn resize_starts(
    p: &mut Project,
    v: &TracksView,
    clicked: (usize, usize),
    at: f64,
) -> bool {
    let Some(start) = region(p, clicked.0, clicked.1).map(|reg| p.region_start_beats(reg)) else {
        return false;
    };
    let delta = at - start;
    let mut changed = false;
    for (t, r) in targets(p, v, Some(clicked)) {
        let reg = &p.tracks[t].regions[r];
        let (own, own_end) = (p.region_start_beats(reg), p.region_end_beats(reg));
        // No shorter than a 64th of a bar (the shortest region), and still ending where it did.
        let to = (own + delta).min(own_end - p.bar_len_at(own) / 64.0);
        changed |= p.resize_region_start(t, r, to);
    }
    changed
}

/// The lower-right corner of region `clicked` let go at `end` (quarter notes): the region ends
/// there, and when it is selected every other selected region's end moves by the same distance
/// (Logic), none shorter than a 64th of a quarter note. With `stretch` (Option held) the audio
/// of each is stretched to its new length instead. Returns false when a region could not be
/// stretched.
pub(crate) fn resize_ends(
    p: &mut Project,
    v: &TracksView,
    clicked: (usize, usize),
    end: f64,
    stretch: bool,
) -> bool {
    let Some(old_end) = region(p, clicked.0, clicked.1).map(|reg| p.region_end_beats(reg)) else {
        return true;
    };
    let delta = end - old_end;
    let mut ok = true;
    for (t, r) in targets(p, v, Some(clicked)) {
        let reg = &p.tracks[t].regions[r];
        let (start, own_end) = (p.region_start_beats(reg), p.region_end_beats(reg));
        let to = (own_end + delta).max(start + MIN_LEN);
        if stretch {
            ok &= crate::flex_edit::stretch_to(p, t, r, start, to);
        } else {
            set_end(p, t, r, to);
        }
    }
    ok
}

// ------------------------------------------------------------ Repeat

/// Repeat's Adjustment (Logic: the Adjustment pop-up of the Repeat Regions window): where each
/// copy starts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepeatAdjust {
    /// Right where the selection (with its loops) ends.
    Auto,
    /// The distance rounded up to whole bars.
    Bar,
    /// To whole beats.
    Beat,
    /// To whole divisions (sixteenth notes).
    Division,
}

impl RepeatAdjust {
    pub(crate) const ALL: [RepeatAdjust; 4] = [
        RepeatAdjust::Auto,
        RepeatAdjust::Bar,
        RepeatAdjust::Beat,
        RepeatAdjust::Division,
    ];

    fn label(self) -> &'static str {
        match self {
            RepeatAdjust::Auto => tr("Auto"),
            RepeatAdjust::Bar => tr("Bar"),
            RepeatAdjust::Beat => tr("Beat"),
            RepeatAdjust::Division => tr("Division"),
        }
    }

    /// Where a copy starts that would start at `next` with Auto, the previous one (or the
    /// selection) having started at `from` (quarter notes).
    fn place(self, p: &Project, from: f64, next: f64) -> f64 {
        let up = |dist: f64, unit: f64| (((dist - 1e-6) / unit).ceil().max(1.0)) * unit;
        match self {
            RepeatAdjust::Auto => next,
            RepeatAdjust::Bar => {
                let (a, b) = (p.beats_to_bar(from), p.beats_to_bar(next));
                p.bar_to_beats(a + up(b - a, 1.0))
            }
            RepeatAdjust::Beat => from + up(next - from, p.beat_unit_at(from)),
            RepeatAdjust::Division => from + up(next - from, 0.25),
        }
    }
}

/// The most copies one Repeat makes.
pub const MAX_COPIES: usize = 999;

/// Repeat (Command-R, Logic: Repeat Regions): `copies` copies of the selected regions, one after
/// the other, each set as far after the last as the selection is long, from the start of its
/// earliest region to the end of its latest one's loop (so a copy of a looped region lands after
/// its loop, not inside it), adjusted by `adjust`. The regions keep their distances and their
/// tracks; the automation under them is copied along (Automation: Move with Regions). The copies
/// are selected. Returns whether any were made.
pub(crate) fn repeat(
    p: &mut Project,
    v: &mut TracksView,
    auto: &AutoView,
    copies: usize,
    adjust: RepeatAdjust,
) -> bool {
    let sel = live_selection(p, v);
    if sel.is_empty() || copies == 0 {
        return false;
    }
    let spans: Vec<(usize, usize, f64, f64)> = sel
        .iter()
        .map(|&(t, r)| {
            let reg = &p.tracks[t].regions[r];
            (t, r, p.region_start_beats(reg), p.region_len_beats(reg))
        })
        .collect();
    let first = spans.iter().map(|s| s.2).fold(f64::INFINITY, f64::min);
    let last = sel
        .iter()
        .map(|&(t, r)| p.region_loop_end_beats(&p.tracks[t].regions[r]))
        .fold(f64::NEG_INFINITY, f64::max);
    let span = (last - first).max(MIN_LEN);
    let mut made = Vec::new();
    let mut drops = Vec::new();
    let mut at = first;
    for _ in 0..copies.min(MAX_COPIES) {
        at = adjust.place(p, at, at + span);
        for &(t, r, start, len) in &spans {
            let now = at + (start - first);
            let copy = Region {
                start_bar: p.beats_to_bar(now) as f32 + 1.0,
                ..p.tracks[t].regions[r].clone()
            };
            p.tracks[t].regions.push(copy);
            made.push((t, p.tracks[t].regions.len() - 1));
            drops.push((t, start, len, t, now));
        }
    }
    crate::automation_ui::follow_drops(p, auto, &drops, false);
    v.select_many(&made);
    p.song_rev += 1;
    true
}

/// A label printed in the windows' silkscreen, right-aligned at `x`.
fn row_label(ui: &egui::Ui, x: f32, y: f32, text: &str) {
    hw::silk(
        ui.painter(),
        pos2(x, y),
        Align2::RIGHT_CENTER,
        text.trim_end_matches(':'),
        11.0,
        theme::INK2(),
    );
}

/// A single-line dark value field over `r`.
fn text_field(ui: &mut egui::Ui, r: egui::Rect, text: &mut String, limit: usize) -> egui::Response {
    hw::paint_field(ui.painter(), r, "", false, false);
    ui.put(
        r.shrink2(vec2(4.0, 0.0)),
        egui::TextEdit::singleline(text)
            .char_limit(limit)
            .font(theme::font(Face::Bold, 13.0))
            .text_color(theme::ON_DARK())
            .frame(egui::Frame::NONE)
            .margin(vec2(6.0, 6.0))
            .desired_width(r.width() - 8.0)
            .min_size(vec2(r.width() - 8.0, r.height()))
            .align(Align2::LEFT_CENTER),
    )
}

/// Gives the field the keyboard the first time the window is drawn.
fn focus_once(ui: &egui::Ui, field: &egui::Response, key: &str, done: bool) {
    let opened = egui::Id::unique(("region_dialog_opened", key));
    if done {
        ui.data_mut(|d| d.remove::<bool>(opened));
    } else if !ui.data(|d| d.get_temp::<bool>(opened).unwrap_or(false)) {
        field.request_focus();
        ui.data_mut(|d| d.insert_temp(opened, true));
    }
}

/// OK (the black key) and Cancel at the right of a window. Returns (ok, cancel).
fn ok_cancel(ui: &mut egui::Ui, ok_enabled: bool) -> (bool, bool) {
    let mut out = (false, false);
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 30.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            let cancel = tr("Cancel");
            out.1 = crate::track_dialogs::dialog_key(
                ui,
                "cancel",
                cancel,
                crate::track_dialogs::key_w(cancel),
                Cap::Bone,
                false,
                false,
            );
            let ok = tr("OK");
            out.0 = ui
                .add_enabled_ui(ok_enabled, |ui| {
                    crate::track_dialogs::dialog_key(
                        ui,
                        "ok",
                        ok,
                        crate::track_dialogs::key_w(ok),
                        Cap::Dark,
                        false,
                        false,
                    )
                })
                .inner;
        },
    );
    out
}

/// The number typed under Number of Copies, when it is one (1 to [`MAX_COPIES`]).
pub(crate) fn parse_copies(text: &str) -> Option<usize> {
    text.trim()
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=MAX_COPIES).contains(n))
}

/// The Repeat window (Logic: Repeat Regions): Number of Copies in a dark field, Adjustment as
/// four latching keys, then OK and Cancel. Return is OK, Escape is Cancel.
pub(crate) fn repeat_body(
    ui: &mut egui::Ui,
    p: &mut Project,
    s: &mut crate::UiState,
    copies: &mut String,
    adjust: &mut RepeatAdjust,
    done: &mut bool,
) {
    const LABEL_W: f32 = 130.0;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    row_label(
        ui,
        row.left() + LABEL_W - 8.0,
        row.center().y,
        tr("Number of Copies:"),
    );
    let fr = egui::Rect::from_min_size(
        pos2(row.left() + LABEL_W, row.top() + 1.0),
        vec2(90.0, 28.0),
    );
    let field = text_field(ui, fr, copies, 3);
    ui.add_space(8.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    row_label(
        ui,
        row.left() + LABEL_W - 8.0,
        row.center().y,
        tr("Adjustment:"),
    );
    let mut x = row.left() + LABEL_W;
    let g = ui.painter().clone();
    for a in RepeatAdjust::ALL {
        let label = a.label();
        let w = crate::track_dialogs::key_w(label);
        let r = egui::Rect::from_min_size(pos2(x, row.top() + 1.0), vec2(w, 28.0));
        let (resp, _) = hw::key(
            ui,
            &g,
            r,
            egui::Id::unique(("repeat_adjust", label)),
            Cap::Bone,
            *adjust == a,
            Some(*adjust == a),
            hw::Legend::Text(label),
        );
        if resp.clicked() {
            *adjust = a;
        }
        x += w + 6.0;
    }
    ui.add_space(10.0);
    let n = parse_copies(copies);
    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let (ok, cancel) = ok_cancel(ui, n.is_some());
    if cancel || escaped {
        *done = true;
    } else if let (true, Some(n)) = (ok || entered, n) {
        repeat(p, &mut s.tracks, &s.auto, n, *adjust);
        *done = true;
    }
    focus_once(ui, &field, "repeat", *done);
}

// ------------------------------------------------------------ Rename Regions

/// The longest region name, in characters.
pub const MAX_REGION_NAME: usize = 64;

/// The names Rename Regions gives `n` regions: `name` for each, or, when it ends with a number,
/// that number counting up from the first one, as wide as it was typed ("Bass 02", "Bass 03").
pub(crate) fn numbered_names(name: &str, n: usize) -> Vec<String> {
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (stem, num) = name.split_at(name.len() - digits);
    match num.parse::<u64>() {
        Ok(first) if n > 1 => (0..n as u64)
            .map(|k| format!("{stem}{:0w$}", first + k, w = digits))
            .collect(),
        _ => vec![name.to_string(); n],
    }
}

/// Rename Regions: `regions` take `name` (trimmed, at most [`MAX_REGION_NAME`] characters),
/// numbered in the order they play (then by track) when it ends with a number. An empty name
/// changes nothing. Returns whether a name changed.
pub(crate) fn rename(p: &mut Project, regions: &[(usize, usize)], name: &str) -> bool {
    let name: String = name.trim().chars().take(MAX_REGION_NAME).collect();
    let mut order: Vec<(f64, usize, usize)> = regions
        .iter()
        .filter_map(|&(t, r)| Some((p.region_start_beats(region(p, t, r)?), t, r)))
        .collect();
    if name.is_empty() || order.is_empty() {
        return false;
    }
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    order.dedup_by(|a, b| a.1 == b.1 && a.2 == b.2);
    let names = numbered_names(&name, order.len());
    let mut changed = false;
    for ((_, t, r), new) in order.into_iter().zip(names) {
        let reg = &mut p.tracks[t].regions[r];
        if reg.name != new {
            reg.name = new;
            changed = true;
        }
    }
    changed
}

/// The regions a rename of `clicked` (the Text tool) applies to: the selection when it is
/// selected, else it alone.
pub(crate) fn rename_targets(
    p: &Project,
    v: &TracksView,
    clicked: (usize, usize),
) -> Vec<(usize, usize)> {
    targets(p, v, Some(clicked))
}

/// The name Rename Regions starts with: the primary selected region's.
pub(crate) fn rename_start(p: &Project, v: &TracksView) -> Option<String> {
    let (t, r) = v.selected?;
    region(p, t, r).map(|reg| reg.name.clone())
}

/// The Rename Regions window: the name in a dark field, then OK and Cancel. Return is OK,
/// Escape is Cancel.
pub(crate) fn rename_body(
    ui: &mut egui::Ui,
    p: &mut Project,
    s: &mut crate::UiState,
    name: &mut String,
    done: &mut bool,
) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let field = text_field(ui, r, name, MAX_REGION_NAME);
    ui.add_space(4.0);
    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let ok_enabled = !name.trim().is_empty();
    let (ok, cancel) = ok_cancel(ui, ok_enabled);
    if cancel || escaped {
        *done = true;
    } else if (ok || entered) && ok_enabled {
        rename(p, &live_selection(p, &s.tracks), name);
        *done = true;
    }
    focus_once(ui, &field, "rename", *done);
}

// ------------------------------------------------------------ Reverse

/// Whether region `r` of track `t` plays audio (the Inspector's test).
fn audio_like(p: &Project, t: usize, r: usize) -> bool {
    p.tracks.get(t).is_some_and(|tr| {
        tr.kind == TrackKind::Audio || tr.regions.get(r).is_some_and(Region::is_audio)
    })
}

/// Whether Reverse Region does anything: an audio region is selected.
pub(crate) fn can_reverse(p: &Project, v: &TracksView) -> bool {
    live_selection(p, v)
        .into_iter()
        .any(|(t, r)| audio_like(p, t, r))
}

/// Reverse Region: the clicked region, or the primary selected one (or the first selected audio
/// region), plays backwards or forwards again, and every other audio region the command acts on
/// takes the same state. Returns whether a region changed.
pub(crate) fn reverse(p: &mut Project, v: &TracksView, clicked: Option<(usize, usize)>) -> bool {
    let regions: Vec<(usize, usize)> = targets(p, v, clicked)
        .into_iter()
        .filter(|&(t, r)| audio_like(p, t, r))
        .collect();
    let lead = clicked
        .or(v.selected)
        .filter(|x| regions.contains(x))
        .or(regions.first().copied());
    let Some((lt, lr)) = lead else {
        return false;
    };
    let now = !p.tracks[lt].regions[lr].params.reverse;
    for (t, r) in regions {
        p.tracks[t].regions[r].params.reverse = now;
    }
    p.song_rev += 1;
    true
}

// ------------------------------------------------------------ Loop

/// The Inspector's Loop: every selected region loops (each up to the next region on its track,
/// as Loop On/Off does) or none does. Returns whether a region changed.
pub(crate) fn set_loop(p: &mut Project, v: &TracksView, on: bool) -> bool {
    let sel = live_selection(p, v);
    if sel.is_empty() {
        return false;
    }
    for &(t, r) in &sel {
        p.tracks[t].regions[r].params.loop_beats = 0.0;
    }
    if on {
        crate::region_edit::toggle_loop(p, v);
    }
    p.song_rev += 1;
    true
}

// ------------------------------------------------------------ the Inspector on every region

/// The Region Inspector changed the primary region `primary` from `before` to `after`: it takes
/// `after`, and every other selected region takes the same change (see the module notes). MIDI
/// settings go only to MIDI regions and audio settings only to audio regions. Returns whether a
/// region changed.
pub(crate) fn spread_params(
    p: &mut Project,
    v: &TracksView,
    primary: (usize, usize),
    before: RegionParams,
    after: RegionParams,
) -> bool {
    if before == after || region(p, primary.0, primary.1).is_none() {
        return false;
    }
    p.tracks[primary.0].regions[primary.1].params = after;
    let others: Vec<(usize, usize)> = targets(p, v, Some(primary))
        .into_iter()
        .filter(|x| *x != primary)
        .collect();
    for (t, r) in others {
        let audio = audio_like(p, t, r);
        let len = p.region_len_beats(&p.tracks[t].regions[r]) as f32;
        let q = &mut p.tracks[t].regions[r].params;
        if before.mute != after.mute {
            q.mute = after.mute;
        }
        // Groove Q (MIDI) and Groove Flex (audio) are one setting.
        if before.groove_q != after.groove_q {
            q.groove_q = after.groove_q;
        }
        if !audio {
            if before.quantize != after.quantize {
                q.quantize = after.quantize;
            }
            if before.transpose != after.transpose {
                let d = i32::from(after.transpose) - i32::from(before.transpose);
                q.transpose = (i32::from(q.transpose) + d).clamp(-96, 96) as i8;
            }
            if before.velocity != after.velocity {
                let d = i32::from(after.velocity) - i32::from(before.velocity);
                q.velocity = (i32::from(q.velocity) + d).clamp(-99, 99) as i8;
            }
        } else {
            if before.gain_db != after.gain_db {
                q.gain_db = (q.gain_db + after.gain_db - before.gain_db)
                    .clamp(MIN_REGION_GAIN, MAX_REGION_GAIN);
            }
            if before.fade_in != after.fade_in {
                q.fade_in = (q.fade_in + after.fade_in - before.fade_in).clamp(0.0, len);
            }
            if before.fade_out != after.fade_out {
                q.fade_out = (q.fade_out + after.fade_out - before.fade_out).clamp(0.0, len);
            }
            if before.fade_curve != after.fade_curve {
                q.fade_curve = after.fade_curve;
            }
            if before.reverse != after.reverse {
                q.reverse = after.reverse;
            }
            if before.crossfade != after.crossfade {
                q.crossfade = after.crossfade;
            }
        }
    }
    p.song_rev += 1;
    true
}

// ------------------------------------------------------------ the nudge value

/// What Nudge Left and Nudge Right move by (Logic: Set Nudge Value to …).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NudgeValue {
    /// A 960th of a quarter note.
    Tick,
    /// A sixteenth note.
    #[default]
    Division,
    /// A beat of the time signature.
    Beat,
    /// A bar.
    Bar,
}

impl NudgeValue {
    /// The menu entries (Edit > Nudge Value), as Logic names the key commands.
    pub(crate) const ENTRIES: [(NudgeValue, &'static str); 4] = [
        (NudgeValue::Tick, "Set Nudge Value to Tick"),
        (NudgeValue::Division, "Set Nudge Value to Division"),
        (NudgeValue::Beat, "Set Nudge Value to Beat"),
        (NudgeValue::Bar, "Set Nudge Value to Bar"),
    ];

    /// The value a menu entry sets.
    pub(crate) fn from_menu(label: &str) -> Option<NudgeValue> {
        Self::ENTRIES
            .iter()
            .find(|(_, l)| *l == label)
            .map(|(n, _)| *n)
    }

    fn key(self) -> &'static str {
        match self {
            NudgeValue::Tick => "tick",
            NudgeValue::Division => "division",
            NudgeValue::Beat => "beat",
            NudgeValue::Bar => "bar",
        }
    }

    /// How far a nudge from `at` (quarter notes) moves, in quarter notes: later with `right`,
    /// else earlier (a beat or a bar is the one before `at` then).
    pub(crate) fn beats(self, p: &Project, at: f64, right: bool) -> f64 {
        let at = if right { at } else { (at - 1e-6).max(0.0) };
        match self {
            NudgeValue::Tick => 1.0 / 960.0,
            NudgeValue::Division => 0.25,
            NudgeValue::Beat => p.beat_unit_at(at),
            NudgeValue::Bar => p.bar_len_at(at),
        }
    }

    /// The nudge value chosen last time (Division when none was).
    pub fn load() -> NudgeValue {
        if cfg!(test) {
            return NudgeValue::default();
        }
        let text = file().and_then(|f| std::fs::read_to_string(f).ok());
        Self::ENTRIES
            .iter()
            .map(|(n, _)| *n)
            .find(|n| text.as_deref().map(str::trim) == Some(n.key()))
            .unwrap_or_default()
    }

    /// Remembers the nudge value for the next start.
    pub fn save(self) {
        // The tests of the menus must not change the settings of whoever runs them.
        if cfg!(test) {
            return;
        }
        if let Some(f) = file() {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(f, self.key()));
            }
        }
    }
}

fn file() -> Option<std::path::PathBuf> {
    crate::recent_files::config_dir().map(|d| d.join("nudge-value"))
}

/// Nudge Left / Right by the nudge value: the selected regions move together, with the
/// automation under them.
pub(crate) fn nudge(p: &mut Project, s: &crate::UiState, right: bool) {
    let Some(at) = crate::region_edit::selection_start(p, &s.tracks) else {
        return;
    };
    let by = s.tracks.nudge.beats(p, at, right);
    let by = if right { by } else { -by };
    // As a drop in the Drag mode chosen (`shuffle_edit.rs`).
    crate::shuffle_edit::nudge(p, &s.tracks, &s.auto, by);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Note;
    use crate::track_dialogs::TrackDialog;
    use crate::ui_test::Harness;
    use crate::undo::History;
    use crate::UiState;
    use egui::{vec2, Event, Key, Modifiers, PointerButton, Pos2, Rect};

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

    fn midi(name: &str, start_bar: f32, len_bars: f32) -> Region {
        Region {
            missing: None,
            name: name.into(),
            start_bar,
            len_bars,
            notes: vec![note(0.1, 1.0), note(2.6, 0.5)],
            controls: Vec::new(),
            folder: None,
            audio: None,
            params: RegionParams::default(),
            generated: None,
        }
    }

    /// Two instrument tracks in 4/4: track 0 has "A" (bars 2-3, beats 4-12) and "B" (bars 5-6,
    /// beats 16-24), track 1 has "C" (bars 3-4, beats 8-16) and "D" (bars 9-10).
    fn rig() -> (Project, UiState, Harness) {
        let mut p = Project::empty();
        p.add_track(TrackKind::Instrument);
        p.tracks[0].regions = vec![midi("A", 2.0, 2.0), midi("B", 5.0, 2.0)];
        p.tracks[1].regions = vec![midi("C", 3.0, 2.0), midi("D", 9.0, 2.0)];
        p.selected_track = 0;
        (p, UiState::new(), Harness::new(1400.0, 600.0))
    }

    /// One frame as the app runs the Tracks area: the View keys, the area, the dialogs and the
    /// key commands.
    fn app_frame(h: &mut Harness, p: &mut Project, s: &mut UiState, events: Vec<Event>) {
        let meters = crate::mixer::Meters::default();
        h.frame(events, |ui| {
            let ctx = ui.ctx().clone();
            crate::key_commands::begin_frame(&ctx);
            crate::menus::handle_keys(&ctx, p, s);
            crate::tracks_area::show(ui, p, s, &meters);
            if s.track_dialog.is_some() {
                crate::track_dialogs::show(&ctx, p, s);
            }
            crate::key_commands::handle(&ctx, p, s);
        });
    }

    fn key_event(key: Key, mods: Modifiers, pressed: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: mods,
        }
    }

    /// `key` pressed and let go with `mods` held.
    fn tap(h: &mut Harness, p: &mut Project, s: &mut UiState, key: Key, mods: Modifiers) {
        h.modifiers = mods;
        let evs = vec![key_event(key, mods, true), key_event(key, mods, false)];
        app_frame(h, p, s, evs);
        h.modifiers = Modifiers::NONE;
        app_frame(h, p, s, vec![]);
    }

    fn bars(p: &Project, t: usize, r: usize) -> (f32, f32) {
        let reg = &p.tracks[t].regions[r];
        (reg.start_bar, reg.len_bars)
    }

    fn names(p: &Project, t: usize) -> Vec<String> {
        p.tracks[t].regions.iter().map(|r| r.name.clone()).collect()
    }

    const CMD: Modifiers = Modifiers::COMMAND;
    const NONE: Modifiers = Modifiers::NONE;

    // ---- 1. Set Region Start / End to Playhead

    #[test]
    fn command_brackets_trim_every_selected_region_under_the_playhead_in_one_undo_step() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        // "A" (4-12), "C" (8-16) and "B" (16-24) selected; the playhead at beat 10 is inside A
        // and C only.
        s.tracks.select_many(&[(0, 0), (1, 0), (0, 1)]);
        p.position_beats = 10.0;
        assert!(crate::menus::enabled("Set Region Start to Playhead", p, s));
        assert!(crate::menus::enabled("Set Region End to Playhead", p, s));
        let mut history = History::new(p);
        tap(h, p, s, Key::OpenBracket, CMD);
        assert_eq!(
            bars(p, 0, 0),
            (3.5, 0.5),
            "A starts at the playhead, ends at 12"
        );
        assert_eq!(
            bars(p, 1, 0),
            (3.5, 1.5),
            "C starts at the playhead, ends at 16"
        );
        assert_eq!(bars(p, 0, 1), (5.0, 2.0), "B is not under the playhead");
        // What plays stays where it was: A's second note (song beat 6.6) was cut away, the one
        // at 4.1 too; C's notes (8.1, 10.6) - the one at 10.6 is now 0.6 into the region.
        assert_eq!(p.tracks[1].regions[0].notes.len(), 1);
        assert!((p.tracks[1].regions[0].notes[0].start - 0.6).abs() < 1e-4);
        assert!(history.record(p));
        assert!(history.undo(p));
        assert_eq!(bars(p, 0, 0), (2.0, 2.0));
        assert_eq!(bars(p, 1, 0), (3.0, 2.0));
        assert!(!history.can_undo(), "one step for both regions");

        // Command-]: both end at the playhead.
        tap(h, p, s, Key::CloseBracket, CMD);
        assert_eq!(bars(p, 0, 0), (2.0, 1.5));
        assert_eq!(bars(p, 1, 0), (3.0, 0.5));
        assert_eq!(bars(p, 0, 1), (5.0, 2.0));

        // With the playhead outside every selected region neither does anything.
        p.position_beats = 40.0;
        assert!(!crate::menus::enabled("Set Region Start to Playhead", p, s));
        assert!(!crate::menus::enabled("Set Region End to Playhead", p, s));
        let before = p.tracks.clone();
        tap(h, p, s, Key::OpenBracket, CMD);
        tap(h, p, s, Key::CloseBracket, CMD);
        assert_eq!(p.tracks.len(), before.len());
        assert_eq!(bars(p, 0, 0), (2.0, 1.5));
    }

    // ---- 2. Split at Playhead without a selection

    #[test]
    fn command_t_without_a_selection_cuts_the_selected_tracks_regions_under_the_playhead() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        // Beat 10 is inside "A" (track 0) and "C" (track 1); track 0 is selected.
        p.position_beats = 10.0;
        s.tracks.clear_selection();
        assert!(crate::menus::enabled("Split at Playhead", p, s));
        let mut history = History::new(p);
        tap(h, p, s, Key::T, CMD);
        assert_eq!(names(p, 0).len(), 3, "A is cut in two");
        assert_eq!(bars(p, 0, 0), (2.0, 1.5));
        assert_eq!(bars(p, 0, 1), (3.5, 0.5));
        assert_eq!(names(p, 1).len(), 2, "track 1 is not the selected track");
        assert_eq!(
            s.tracks.selection(),
            vec![(0, 1)],
            "the right piece is selected"
        );
        assert!(history.record(p));
        assert!(history.undo(p));
        assert_eq!(names(p, 0).len(), 2);
        assert!(!history.can_undo());

        // With a selection, only the selection is cut, as before.
        s.tracks.select_only(1, 0);
        tap(h, p, s, Key::T, CMD);
        assert_eq!(names(p, 0).len(), 2);
        assert_eq!(names(p, 1).len(), 3);

        // Nothing under the playhead on the selected track: nothing to do.
        s.tracks.clear_selection();
        p.position_beats = 14.0;
        assert!(!crate::menus::enabled("Split at Playhead", p, s));
    }

    // ---- 3. Resizing every selected region with the Pointer

    /// Where region `r` of track `t` is on screen (the last frame).
    fn rect_of(h: &Harness, t: usize, r: usize) -> Rect {
        h.ctx
            .read_response(egui::Id::unique(("tracks_area", ("region", t, r))))
            .expect("the region is on screen")
            .rect
    }

    fn drag(h: &mut Harness, p: &mut Project, s: &mut UiState, from: Pos2, by: f32) {
        let meters = crate::mixer::Meters::default();
        h.time_passes(1.0);
        h.drag(
            &[
                from,
                from + vec2(by.signum() * 20.0, 0.0),
                from + vec2(by, 0.0),
            ],
            PointerButton::Primary,
            |ui| crate::tracks_area::show(ui, p, s, &meters),
        );
    }

    /// The default zoom: a bar is 96 points.
    const BAR: f32 = 96.0;

    #[test]
    fn dragging_a_corner_resizes_every_selected_region_by_the_same_amount() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        s.tracks.select_many(&[(0, 0), (1, 0)]);
        app_frame(h, p, s, vec![]);
        app_frame(h, p, s, vec![]);
        // A's lower-left corner half a bar to the right: C's start moves half a bar too.
        let grip = rect_of(h, 0, 0).left_bottom() + vec2(2.0, -4.0);
        let mut history = History::new(p);
        drag(h, p, s, grip, BAR / 2.0);
        assert_eq!(bars(p, 0, 0), (2.5, 1.5));
        assert_eq!(bars(p, 1, 0), (3.5, 1.5));
        assert_eq!(bars(p, 0, 1), (5.0, 2.0), "B was not selected");
        assert!(history.record(p));
        assert!(history.undo(p));
        assert_eq!((bars(p, 0, 0), bars(p, 1, 0)), ((2.0, 2.0), (3.0, 2.0)));
        assert!(!history.can_undo(), "one step");

        // A's lower-right corner a bar to the left: C ends a bar earlier too.
        app_frame(h, p, s, vec![]);
        app_frame(h, p, s, vec![]);
        let grip = rect_of(h, 0, 0).right_bottom() + vec2(-2.0, -4.0);
        drag(h, p, s, grip, -BAR);
        assert_eq!(bars(p, 0, 0), (2.0, 1.0));
        assert_eq!(bars(p, 1, 0), (3.0, 1.0));

        // A region that is not selected is resized alone.
        s.tracks.select_only(1, 1);
        app_frame(h, p, s, vec![]);
        app_frame(h, p, s, vec![]);
        let grip = rect_of(h, 0, 1).right_bottom() + vec2(-2.0, -4.0);
        drag(h, p, s, grip, BAR);
        assert_eq!(bars(p, 0, 1), (5.0, 3.0));
        assert_eq!(bars(p, 1, 1), (9.0, 2.0));
    }

    #[test]
    fn a_shared_start_move_stops_at_each_regions_own_end() {
        let (mut p, _, _) = rig();
        let mut v = TracksView::default();
        // "A" is two bars, "B" one: moving both starts six beats on leaves B its minimum.
        p.tracks[0].regions[1].len_bars = 1.0;
        v.select_many(&[(0, 0), (0, 1)]);
        assert!(resize_starts(&mut p, &v, (0, 0), 10.0));
        assert_eq!(bars(&p, 0, 0), (3.5, 0.5));
        let b = &p.tracks[0].regions[1];
        assert!(p.region_end_beats(b) - p.region_start_beats(b) > 0.0);
        assert!(
            (p.region_end_beats(b) - 20.0).abs() < 1e-3,
            "B still ends at 20: {}",
            p.region_end_beats(b)
        );
        assert!(
            (b.len_bars - 1.0 / 64.0).abs() < 1e-6,
            "a 64th of a bar left"
        );
    }

    // ---- 4. Repeat

    #[test]
    fn command_r_opens_the_repeat_window_and_return_makes_the_copies() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        s.tracks.select_only(0, 0);
        tap(h, p, s, Key::R, CMD);
        assert_eq!(
            s.track_dialog,
            Some(TrackDialog::Repeat {
                copies: "1".into(),
                adjust: RepeatAdjust::Auto
            })
        );
        // Type 3 over the 1 and press Return.
        let mut history = History::new(p);
        app_frame(
            h,
            p,
            s,
            vec![
                key_event(Key::Backspace, NONE, true),
                key_event(Key::Backspace, NONE, false),
                Event::Text("3".into()),
            ],
        );
        app_frame(
            h,
            p,
            s,
            vec![
                key_event(Key::Enter, NONE, true),
                key_event(Key::Enter, NONE, false),
            ],
        );
        app_frame(h, p, s, vec![]);
        assert!(s.track_dialog.is_none(), "the window closes");
        let starts: Vec<f32> = p.tracks[0].regions.iter().map(|r| r.start_bar).collect();
        assert_eq!(
            starts,
            vec![2.0, 5.0, 4.0, 6.0, 8.0],
            "three copies, one after the other"
        );
        assert_eq!(p.tracks[0].regions[0].params.loop_beats, 0.0);
        assert_eq!(s.tracks.selection(), vec![(0, 2), (0, 3), (0, 4)]);
        assert!(history.record(p));
        assert!(history.undo(p));
        assert_eq!(p.tracks[0].regions.len(), 2);
        assert!(!history.can_undo(), "one step");
        // Escape closes the window and makes nothing.
        s.tracks.select_only(0, 0);
        tap(h, p, s, Key::R, CMD);
        assert!(s.track_dialog.is_some());
        tap(h, p, s, Key::Escape, NONE);
        assert!(s.track_dialog.is_none());
        assert_eq!(p.tracks[0].regions.len(), 2);
    }

    #[test]
    fn a_repeated_looped_region_lands_after_its_loop_and_the_grid_adjustments_round_up() {
        let (mut p, _, _) = rig();
        let auto = AutoView::default();
        let mut v = TracksView::default();
        // "A" (beats 4-12) loops for six beats, to 18: the copy starts at 18 (bar 5.5).
        p.tracks[0].regions[0].params.loop_beats = 6.0;
        v.select_only(0, 0);
        assert!(repeat(&mut p, &mut v, &auto, 1, RepeatAdjust::Auto));
        assert_eq!(p.tracks[0].regions[2].start_bar, 5.5);
        // Bar: rounded up to whole bars (14 beats -> 4 bars): bar 6.
        v.select_only(0, 0);
        repeat(&mut p, &mut v, &auto, 1, RepeatAdjust::Bar);
        assert_eq!(p.tracks[0].regions[3].start_bar, 6.0);
        // Beat and Division: 14 beats is already whole.
        p.tracks[0].regions[0].params.loop_beats = 5.9;
        v.select_only(0, 0);
        repeat(&mut p, &mut v, &auto, 1, RepeatAdjust::Beat);
        assert_eq!(p.tracks[0].regions[4].start_bar, 5.5);
        v.select_only(0, 0);
        repeat(&mut p, &mut v, &auto, 2, RepeatAdjust::Division);
        assert_eq!(p.tracks[0].regions[5].start_bar, 5.5);
        assert_eq!(p.tracks[0].regions[6].start_bar, 9.0);
        // Regions on two tracks keep their distance.
        let (mut p, _, _) = rig();
        v.select_many(&[(0, 0), (1, 0)]);
        repeat(&mut p, &mut v, &auto, 1, RepeatAdjust::Auto);
        assert_eq!(p.tracks[0].regions[2].start_bar, 5.0);
        assert_eq!(p.tracks[1].regions[2].start_bar, 6.0);
        assert_eq!(parse_copies("0"), None);
        assert_eq!(parse_copies("12"), Some(12));
        assert_eq!(parse_copies("x"), None);
    }

    #[test]
    fn repeat_copies_the_automation_under_the_regions_along() {
        use prowave_engine::{lane_value, AutoPoint};
        let (mut p, _, _) = rig();
        let auto = AutoView::default();
        let mut v = TracksView::default();
        // A volume ramp under "A" (beats 4-12); two copies land at 12 and 20.
        p.tracks[0].auto.volume = vec![AutoPoint::new(5.0, -12.0), AutoPoint::new(7.0, -3.0)];
        v.select_only(0, 0);
        repeat(&mut p, &mut v, &auto, 2, RepeatAdjust::Auto);
        let vol = &p.tracks[0].auto.volume;
        assert_eq!(lane_value(vol, 13.0), Some(-12.0));
        assert_eq!(lane_value(vol, 15.0), Some(-3.0));
        assert_eq!(lane_value(vol, 21.0), Some(-12.0));
    }

    // ---- 5. Rename Regions

    #[test]
    fn rename_regions_numbers_names_that_end_with_a_number() {
        assert_eq!(numbered_names("Bass", 3), vec!["Bass"; 3]);
        assert_eq!(
            numbered_names("Bass 02", 3),
            vec!["Bass 02", "Bass 03", "Bass 04"]
        );
        assert_eq!(numbered_names("Take 9", 2), vec!["Take 9", "Take 10"]);
        assert_eq!(numbered_names("7", 1), vec!["7"]);
    }

    #[test]
    fn shift_n_renames_every_selected_region_in_the_order_they_play() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        s.tracks.select_many(&[(0, 1), (1, 0), (0, 0)]);
        tap(h, p, s, Key::N, Modifiers::SHIFT);
        assert_eq!(
            s.track_dialog,
            Some(TrackDialog::RenameRegions { name: "B".into() }),
            "the primary region's name to start from"
        );
        let mut history = History::new(p);
        // Select all of "B" and type over it.
        app_frame(
            h,
            p,
            s,
            vec![
                key_event(Key::Backspace, NONE, true),
                key_event(Key::Backspace, NONE, false),
                Event::Text("Verse 1".into()),
            ],
        );
        app_frame(
            h,
            p,
            s,
            vec![
                key_event(Key::Enter, NONE, true),
                key_event(Key::Enter, NONE, false),
            ],
        );
        app_frame(h, p, s, vec![]);
        assert!(s.track_dialog.is_none());
        // A (beat 4), C (beat 8), B (beat 16).
        assert_eq!(names(p, 0), vec!["Verse 1", "Verse 3"]);
        assert_eq!(names(p, 1), vec!["Verse 2", "D"]);
        assert!(history.record(p));
        assert!(history.undo(p));
        assert_eq!(names(p, 0), vec!["A", "B"]);
        assert!(!history.can_undo());
        // Without a number they all take the name.
        assert!(rename(p, &[(0, 0), (1, 1)], "Hook"));
        assert_eq!(
            (names(p, 0)[0].as_str(), names(p, 1)[1].as_str()),
            ("Hook", "Hook")
        );
        assert!(!rename(p, &[(0, 0)], "  "), "an empty name changes nothing");
        // Nothing selected: the command is off.
        s.tracks.clear_selection();
        assert!(!crate::menus::enabled("Rename Regions", p, s));
    }

    #[test]
    fn the_text_tool_renames_the_selection_when_the_clicked_region_is_in_it() {
        let (mut p, _, _) = rig();
        let mut v = TracksView::default();
        v.select_many(&[(0, 0), (1, 0)]);
        assert_eq!(rename_targets(&p, &v, (1, 0)), vec![(0, 0), (1, 0)]);
        assert_eq!(rename_targets(&p, &v, (0, 1)), vec![(0, 1)], "B alone");
        let regions = rename_targets(&p, &v, (0, 0));
        assert!(rename(&mut p, &regions, "Pad 1"));
        assert_eq!(names(&p, 0), vec!["Pad 1", "B"]);
        assert_eq!(names(&p, 1), vec!["Pad 2", "D"]);
    }

    #[test]
    fn a_double_click_on_the_region_inspectors_name_opens_rename_regions() {
        let (mut p, mut s, _) = rig();
        let (p, s) = (&mut p, &mut s);
        let mut h = Harness::new(360.0, 1200.0);
        s.tracks.select_many(&[(0, 0), (1, 0)]);
        for _ in 0..2 {
            inspector_frames(&mut h, p, s, vec![]);
        }
        let open = s.inspector.region_open;
        let at = h
            .ctx
            .read_response(egui::Id::unique(("inspector", ("section", "region"))))
            .expect("the Region header is drawn")
            .rect
            .center();
        let meters = crate::mixer::Meters::default();
        h.click(at, |ui| crate::inspector::show(ui, p, s, &meters));
        h.click(at, |ui| crate::inspector::show(ui, p, s, &meters));
        assert_eq!(
            s.track_dialog,
            Some(TrackDialog::RenameRegions { name: "A".into() })
        );
        assert_eq!(s.inspector.region_open, open, "the section stays as it was");
    }

    // ---- 6. Q

    #[test]
    fn q_quantizes_the_selected_regions_notes_in_one_undo_step() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        p.tracks[1].regions[0].params.quantize = Some(1.0);
        s.tracks.select_many(&[(0, 0), (1, 0)]);
        let mut history = History::new(p);
        tap(h, p, s, Key::Q, NONE);
        let starts = |p: &Project, t: usize| -> Vec<f32> {
            p.tracks[t].regions[0]
                .notes
                .iter()
                .map(|n| n.start)
                .collect()
        };
        assert_eq!(
            starts(p, 0),
            vec![0.0, 2.5],
            "A: Quantize Off, so a sixteenth"
        );
        assert_eq!(starts(p, 1), vec![0.0, 3.0], "C: its own quarter notes");
        assert_eq!(
            p.tracks[0].regions[1].notes[0].start, 0.1,
            "B is not selected"
        );
        assert!(history.record(p));
        assert!(history.undo(p));
        assert_eq!(starts(p, 0), vec![0.1, 2.6]);
        assert!(!history.can_undo());
    }

    // ---- 7. The nudge value

    #[test]
    fn control_option_keys_set_the_nudge_value_that_option_arrows_move_by() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        s.tracks.select_only(0, 0);
        let start = |p: &Project| p.region_start_beats(&p.tracks[0].regions[0]);
        let ctrl_alt = Modifiers::CTRL | Modifiers::ALT;
        assert!(crate::menus::checked("Set Nudge Value to Division", p, s));
        tap(h, p, s, Key::ArrowRight, Modifiers::ALT);
        assert_eq!(start(p), 4.25, "a division");
        tap(h, p, s, Key::B, ctrl_alt);
        assert_eq!(s.tracks.nudge, NudgeValue::Beat);
        assert!(crate::menus::checked("Set Nudge Value to Beat", p, s));
        assert!(!crate::menus::checked("Set Nudge Value to Division", p, s));
        tap(h, p, s, Key::ArrowRight, Modifiers::ALT);
        assert_eq!(start(p), 5.25, "a beat");
        tap(h, p, s, Key::M, ctrl_alt);
        assert_eq!(s.tracks.nudge, NudgeValue::Bar);
        tap(h, p, s, Key::ArrowLeft, Modifiers::ALT);
        assert_eq!(start(p), 1.25, "a bar back");
        tap(h, p, s, Key::T, ctrl_alt);
        assert_eq!(s.tracks.nudge, NudgeValue::Tick);
        assert_eq!(s.tracks.tool, crate::tracks_area::TrackTool::Pointer);
        tap(h, p, s, Key::ArrowRight, Modifiers::ALT);
        assert!((start(p) - (1.25 + 1.0 / 960.0)).abs() < 1e-6, "a tick");
        tap(h, p, s, Key::D, ctrl_alt);
        assert_eq!(s.tracks.nudge, NudgeValue::Division);
        // The menu entries do the same.
        crate::menus::apply("Set Nudge Value to Bar", p, s);
        assert_eq!(s.tracks.nudge, NudgeValue::Bar);
        // In 3/4 a bar is three beats.
        p.sig = (3, 4);
        assert_eq!(NudgeValue::Bar.beats(p, 0.0, true), 3.0);
    }

    // ---- 8. The Region Inspector on every selected region

    fn inspector_frames(h: &mut Harness, p: &mut Project, s: &mut UiState, events: Vec<Event>) {
        let meters = crate::mixer::Meters::default();
        h.frame(events, |ui| crate::inspector::show(ui, p, s, &meters));
    }

    fn inspector_click(h: &mut Harness, p: &mut Project, s: &mut UiState, key: &str) {
        for _ in 0..2 {
            inspector_frames(h, p, s, vec![]);
        }
        let at = h
            .ctx
            .read_response(egui::Id::unique(("inspector", ("check", key))))
            .expect("the checkbox is drawn")
            .rect
            .center();
        h.time_passes(1.0);
        let meters = crate::mixer::Meters::default();
        h.click(at, |ui| crate::inspector::show(ui, p, s, &meters));
        for _ in 0..2 {
            inspector_frames(h, p, s, vec![]);
        }
    }

    #[test]
    fn the_inspectors_mute_and_loop_set_every_selected_region() {
        let (mut p, mut s, _) = rig();
        let (p, s) = (&mut p, &mut s);
        let mut h = Harness::new(360.0, 1200.0);
        s.tracks.select_many(&[(0, 0), (1, 0)]);
        let mut history = History::new(p);
        inspector_click(&mut h, p, s, "region_mute");
        assert!(p.tracks[0].regions[0].params.mute && p.tracks[1].regions[0].params.mute);
        assert!(!p.tracks[0].regions[1].params.mute, "B is not selected");
        assert!(history.record(p));
        assert!(history.undo(p));
        assert!(!p.tracks[1].regions[0].params.mute);
        assert!(!history.can_undo());
        // Loop: A loops up to B (4 beats), C up to D (16 beats); off again.
        inspector_click(&mut h, p, s, "region_loop");
        assert_eq!(p.tracks[0].regions[0].params.loop_beats, 4.0);
        assert_eq!(p.tracks[1].regions[0].params.loop_beats, 16.0);
        inspector_click(&mut h, p, s, "region_loop");
        assert_eq!(p.tracks[0].regions[0].params.loop_beats, 0.0);
        assert_eq!(p.tracks[1].regions[0].params.loop_beats, 0.0);
    }

    #[test]
    fn inspector_changes_spread_by_the_same_amount_and_stay_in_range() {
        let (mut p, _, _) = rig();
        // Track 0 becomes an audio track; its regions take the audio settings.
        p.tracks[0].kind = TrackKind::Audio;
        p.tracks[0].regions[1].params.gain_db = MAX_REGION_GAIN - 1.0;
        p.tracks[1].regions[1].params.transpose = 95;
        let mut v = TracksView::default();
        v.select_many(&[(1, 0), (1, 1), (0, 0), (0, 1)]);
        // On "C" (MIDI): transpose +3, quantize to quarter notes.
        let before = p.tracks[1].regions[0].params;
        let mut after = before;
        after.transpose = 3;
        after.quantize = Some(1.0);
        assert!(spread_params(&mut p, &v, (1, 0), before, after));
        assert_eq!(p.tracks[1].regions[0].params.transpose, 3);
        assert_eq!(p.tracks[1].regions[1].params.transpose, 96, "held at +96");
        assert_eq!(p.tracks[1].regions[1].params.quantize, Some(1.0));
        assert_eq!(
            p.tracks[0].regions[0].params.transpose, 0,
            "audio takes no transpose"
        );
        // On "A" (audio): gain +2 dB, a fade in of one beat, reverse.
        let before = p.tracks[0].regions[0].params;
        let mut after = before;
        after.gain_db = 2.0;
        after.fade_in = 1.0;
        after.fade_curve = prowave_engine::FadeCurve::SCurve;
        after.reverse = true;
        assert!(spread_params(&mut p, &v, (0, 0), before, after));
        let b = p.tracks[0].regions[1].params;
        assert_eq!(b.gain_db, MAX_REGION_GAIN, "held at the most");
        assert_eq!(
            (b.fade_in, b.fade_curve, b.reverse),
            (1.0, prowave_engine::FadeCurve::SCurve, true)
        );
        assert!(
            !p.tracks[1].regions[0].params.reverse,
            "MIDI takes no reverse"
        );
        // A region outside the selection changes alone.
        v.select_only(1, 1);
        let before = p.tracks[0].regions[0].params;
        let mut after = before;
        after.mute = true;
        spread_params(&mut p, &v, (0, 0), before, after);
        assert!(!p.tracks[1].regions[1].params.mute);
        assert!(!p.tracks[0].regions[1].params.mute);
    }

    // ---- 9. Reverse Region

    #[test]
    fn reverse_region_flips_every_selected_audio_region_together() {
        let (mut p, mut s, _) = rig();
        let (p, s) = (&mut p, &mut s);
        p.tracks[0].kind = TrackKind::Audio;
        s.tracks.clear_selection();
        assert!(!crate::menus::enabled("Reverse Region", p, s));
        s.tracks.select_only(1, 0);
        assert!(!crate::menus::enabled("Reverse Region", p, s), "MIDI only");
        s.tracks.select_many(&[(0, 0), (0, 1), (1, 0)]);
        p.tracks[0].regions[1].params.reverse = true;
        assert!(crate::menus::enabled("Reverse Region", p, s));
        let mut history = History::new(p);
        crate::menus::apply("Reverse Region", p, s);
        assert!(p.tracks[0].regions[0].params.reverse);
        assert!(
            p.tracks[0].regions[1].params.reverse,
            "takes the primary's new state"
        );
        assert!(!p.tracks[1].regions[0].params.reverse);
        assert!(history.record(p));
        crate::menus::apply("Reverse Region", p, s);
        assert!(!p.tracks[0].regions[0].params.reverse && !p.tracks[0].regions[1].params.reverse);
        assert!(history.record(p));
        assert!(history.undo(p));
        assert!(p.tracks[0].regions[0].params.reverse);
    }

    // ---- 10. Loop On/Off keeps L

    #[test]
    fn loop_on_off_is_l_as_in_logic_and_control_l_is_not_it() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        s.tracks.select_only(0, 0);
        tap(h, p, s, Key::L, Modifiers::CTRL);
        assert_eq!(p.tracks[0].regions[0].params.loop_beats, 0.0);
        tap(h, p, s, Key::L, NONE);
        assert_eq!(p.tracks[0].regions[0].params.loop_beats, 4.0);
    }
}
