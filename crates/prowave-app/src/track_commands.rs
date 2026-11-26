//! Logic Pro's track commands for the Tracks area: Delete Track (Command-Delete), the New
//! Tracks dialog (Option-Command-N), New Track with Duplicate Settings (Command-D), Show/Hide
//! Global Tracks (G), hidden tracks (H, Control-H), region colours (Option-C, Option-Shift-C),
//! Create Track Stack (Shift-Command-D), Name Track (Shift-Return), Delete Unused Tracks and
//! Show Editor (E). Each one is a menu entry (`menus::apply`) with its key in
//! `key_commands::COMMANDS`, and changes the project in one go, so it is one undo step.

use crate::i18n::tr;
use crate::model::{Project, TrackKind};
use crate::UiState;

/// What these commands remember while the program runs (not saved).
pub struct ArrangeView {
    /// Which of the Marker, Signature and Tempo tracks Show Global Tracks brings back.
    pub global: [bool; 3],
    /// View > Show Colors (Option-C): the Colors window is open (`region_colors`).
    pub colors: bool,
}

impl Default for ArrangeView {
    fn default() -> Self {
        Self {
            global: [true; 3],
            colors: false,
        }
    }
}

// ------------------------------------------------------------------ hidden tracks

/// Whether track `i` is hidden: its own H key is on, or that of a track stack around it.
pub(crate) fn hidden(p: &Project, i: usize) -> bool {
    let Some(t) = p.tracks.get(i) else {
        return false;
    };
    if t.hidden {
        return true;
    }
    let mut level = t.indent;
    for j in (0..i).rev() {
        let t = &p.tracks[j];
        if t.indent < level {
            if t.kind == TrackKind::Folder && t.hidden {
                return true;
            }
            level = t.indent;
        }
    }
    false
}

/// Whether track `i` has no row in the Tracks area: inside a folded stack, or hidden while
/// hidden tracks are not shown. It plays all the same.
pub(crate) fn takes_no_room(p: &Project, show_hidden: bool, i: usize) -> bool {
    p.hidden_by_fold(i) || !show_hidden && hidden(p, i)
}

// Track > Hide Track (Control-H) acts on every selected track: `track_selection::hide_tracks`.

/// Track > Unhide All Tracks: every track's H key off.
pub(crate) fn unhide_all(p: &mut Project) {
    for t in &mut p.tracks {
        t.hidden = false;
    }
}

/// Whether any track is hidden.
pub(crate) fn any_hidden(p: &Project) -> bool {
    p.tracks.iter().any(|t| t.hidden)
}

// ------------------------------------------------------------------ global tracks

/// The Marker, Signature and Tempo tracks: whether each is shown.
fn global_shown(s: &UiState) -> [bool; 3] {
    let v = &s.tracks;
    [v.show_markers, v.show_signatures, v.show_tempo]
}

/// View > Show Global Tracks: whether any global track is shown.
pub(crate) fn global_tracks_shown(s: &UiState) -> bool {
    global_shown(s).contains(&true)
}

/// View > Show Global Tracks (G): hides the global tracks that are shown and remembers which
/// they were, or brings those back (Logic shows and hides them together).
pub(crate) fn toggle_global_tracks(s: &mut UiState) {
    let shown = global_shown(s);
    let next = if shown.contains(&true) {
        s.arrange.global = shown;
        [false; 3]
    } else {
        s.arrange.global
    };
    let v = &mut s.tracks;
    [v.show_markers, v.show_signatures, v.show_tempo] = next;
}

// ------------------------------------------------------------------ New Tracks

/// The kinds of track the New Tracks dialog offers, as the "+" menu over the headers does.
pub(crate) const NEW_KINDS: [TrackKind; 5] = [
    TrackKind::Instrument,
    TrackKind::Audio,
    TrackKind::ExternalMidi,
    TrackKind::Aux,
    TrackKind::Vca,
];

/// The most tracks the New Tracks dialog makes at once.
pub(crate) const MAX_NEW_TRACKS: u32 = 64;

/// A kind's name on its key in the New Tracks dialog.
fn kind_label(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Audio => tr("Audio"),
        TrackKind::ExternalMidi => tr("External MIDI"),
        TrackKind::Aux => tr("Aux"),
        TrackKind::Vca => tr("VCA"),
        _ => tr("Software Instrument"),
    }
}

/// Inserts `count` new tracks of `kind` right below the selected track (after everything in
/// it, when it is a track stack), at its nesting level, as Logic does, and selects the first.
/// Returns its index.
pub(crate) fn add_tracks_below(p: &mut Project, kind: TrackKind, count: u32) -> usize {
    let (mut at, indent) = match p.tracks.get(p.selected_track) {
        Some(t) => (p.folder_end(p.selected_track), t.indent),
        None => (p.tracks.len(), 0),
    };
    let first = at;
    for _ in 0..count.max(1) {
        // Named and counted as the quick-add entries name them, then moved into place.
        p.add_track(kind);
        let Some(mut t) = p.tracks.pop() else {
            break;
        };
        t.indent = indent;
        p.tracks.insert(at, t);
        p.vca_inserted(at);
        at += 1;
    }
    p.selected_track = first;
    p.song_rev += 1;
    first
}

/// The New Tracks dialog (Track > New Tracks…, Option-Command-N): the kind of track, how many,
/// then Create (or Return) and Cancel (or Esc).
pub(crate) fn new_tracks_body(
    ui: &mut egui::Ui,
    p: &mut Project,
    s: &mut UiState,
    kind: &mut TrackKind,
    count: &mut u32,
    done: &mut bool,
) {
    use crate::hw::{self, Cap, Legend};
    use crate::theme;
    use egui::{pos2, vec2, Align2, Rect, Sense};
    let width = ui.available_width();
    let label_row = |ui: &mut egui::Ui, text: &str| {
        let (r, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
        hw::silk(
            ui.painter(),
            pos2(r.left() + 2.0, r.center().y),
            Align2::LEFT_CENTER,
            text,
            11.0,
            theme::PRINT(),
        );
    };
    label_row(ui, tr("Track Type"));
    // The kinds: one row of bone keys, the chosen one latched with its LED lit.
    let gap = 6.0;
    let key_w = ((width - gap * (NEW_KINDS.len() as f32 - 1.0)) / NEW_KINDS.len() as f32).floor();
    let (row, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
    let g = ui.painter().clone();
    for (n, k) in NEW_KINDS.into_iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(row.left() + n as f32 * (key_w + gap), row.top()),
            vec2(key_w, 30.0),
        );
        let on = *kind == k;
        let (resp, _) = hw::key(
            ui,
            &g,
            r,
            egui::Id::unique(("new_tracks_kind", n)),
            Cap::Bone,
            on,
            Some(on),
            Legend::Text(kind_label(k)),
        );
        if resp.clicked() {
            *kind = k;
        }
    }
    ui.add_space(8.0);
    label_row(ui, tr("Number of Tracks"));
    // The count: minus, the number on LCD glass in segments, plus.
    let (row, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
    let minus = Rect::from_min_size(row.min, vec2(30.0, 30.0));
    let glass = Rect::from_min_size(pos2(minus.right() + 6.0, row.top()), vec2(64.0, 30.0));
    let plus = Rect::from_min_size(pos2(glass.right() + 6.0, row.top()), vec2(30.0, 30.0));
    let step = |r: Rect, id: &str, legend: &str| {
        hw::key(
            ui,
            &g,
            r,
            egui::Id::unique(("new_tracks_step", id)),
            Cap::Bone,
            false,
            None,
            Legend::Text(legend),
        )
        .0
        .clicked()
    };
    let (less, more) = (step(minus, "minus", "-"), step(plus, "plus", "+"));
    // Taken out of the input, so the arrows do not also move notes in the Piano Roll.
    let (up, down, enter, escape) = ui.input_mut(|i| {
        let none = egui::Modifiers::NONE;
        (
            i.consume_key(none, egui::Key::ArrowUp),
            i.consume_key(none, egui::Key::ArrowDown),
            i.consume_key(none, egui::Key::Enter),
            i.consume_key(none, egui::Key::Escape),
        )
    });
    if less || down {
        *count = count.saturating_sub(1).max(1);
    }
    if more || up {
        *count = (*count + 1).min(MAX_NEW_TRACKS);
    }
    hw::lcd_glass(&g, glass);
    let digits = count.to_string();
    let h = 18.0;
    hw::seg(
        &g,
        pos2(
            glass.right() - 8.0 - hw::seg_width(&digits, h),
            glass.center().y - h / 2.0,
        ),
        &digits,
        h,
        theme::LCD_INK(),
        false,
    );
    ui.add_space(10.0);
    ui.allocate_ui_with_layout(
        vec2(width, 30.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            let cancel = tr("Cancel");
            let w = crate::track_dialogs::key_w(cancel);
            if crate::track_dialogs::dialog_key(ui, "cancel", cancel, w, Cap::Bone, false, false)
                || escape
            {
                *done = true;
                return;
            }
            let create = tr("Create");
            let w = crate::track_dialogs::key_w(create);
            if crate::track_dialogs::dialog_key(ui, "create", create, w, Cap::Dark, false, false)
                || enter
            {
                add_tracks_below(p, *kind, *count);
                forget_track_indexes(s);
                *done = true;
            }
        },
    );
}

// ------------------------------------------------------------------ duplicate settings

/// Track > New Track with Duplicate Settings (Command-D, and the duplicate key over the
/// headers), for track `i`: a new track right below it with its kind, instrument, plug-ins and
/// channel settings, but no regions and no automation, and selects it. Not for a track stack.
/// Returns the new track's index. `track_selection::duplicate_tracks` runs it for every
/// selected track.
pub(crate) fn duplicate_settings_of(p: &mut Project, i: usize) -> Option<usize> {
    let t = p.tracks.get(i).filter(|t| t.kind != TrackKind::Folder)?;
    let mut copy = t.clone();
    copy.regions.clear();
    copy.auto = crate::automation::Automation::default();
    copy.rec = false;
    // A new track is shown, even when made from a hidden one (Show Hidden Tracks on).
    copy.hidden = false;
    // There is one Groove Track; the copy follows it as the original does.
    copy.params.groove = false;
    {
        copy.vocal = None;
    }
    // Its own plug-in instances, opened with the same settings.
    for slot in copy
        .au_instrument
        .iter_mut()
        .chain(copy.au_effects.iter_mut())
    {
        slot.uid = crate::plugins::new_uid();
    }
    p.tracks.insert(i + 1, copy);
    p.vca_inserted(i + 1);
    // The shift above moved the copy's own VCA too; the original's is the right one.
    p.tracks[i + 1].vca = p.tracks[i].vca;
    p.selected_track = i + 1;
    p.song_rev += 1;
    Some(i + 1)
}

// ------------------------------------------------------------------ Delete Unused Tracks

/// Whether track `i` is unused for Delete Unused Tracks (Logic: tracks without regions): a
/// track that can hold regions and holds none, a VCA that no track is assigned to, or a track
/// stack with nothing inside. Aux, bus and output tracks have no regions by nature and stay.
fn unused(p: &Project, i: usize) -> bool {
    let t = &p.tracks[i];
    match t.kind {
        TrackKind::Folder => p.folder_end(i) == i + 1,
        TrackKind::Vca => !p.tracks.iter().any(|o| o.vca == Some(i)),
        k => k.has_regions() && t.regions.is_empty(),
    }
}

/// Whether Delete Unused Tracks has something to delete (and is not in the middle of a
/// recording). One track always stays.
pub(crate) fn can_delete_unused(p: &Project) -> bool {
    !p.recording && p.tracks.len() > 1 && (0..p.tracks.len()).any(|i| unused(p, i))
}

/// Track > Delete Unused Tracks: deletes every unused track (see [`unused`]), from the last up,
/// so a stack emptied by it goes too. The first track is kept when every one is unused.
/// Returns how many went.
pub(crate) fn delete_unused(p: &mut Project, s: &mut UiState) -> usize {
    if p.recording {
        return 0;
    }
    let mut gone = 0;
    for i in (0..p.tracks.len()).rev() {
        if i < p.tracks.len() && unused(p, i) && p.delete_track(i) {
            gone += 1;
        }
    }
    if gone > 0 {
        forget_track_indexes(s);
        s.pitch_ed.region = None;
    }
    gone
}

// ------------------------------------------------------------------ Show Editor

/// View > Show Editor (E): shows the editor for the selected region under the tracks, the
/// Piano Roll for a MIDI region and the Pitch Editor for an audio region, or hides it when it
/// is showing. With no region selected, the selected track's kind decides.
pub(crate) fn toggle_editor(p: &Project, s: &mut UiState) {
    use crate::Bottom;
    if matches!(s.bottom, Bottom::PianoRoll | Bottom::PitchEditor) {
        s.bottom = Bottom::Mixer;
        return;
    }
    let first = s
        .tracks
        .selection()
        .into_iter()
        .find(|&(t, r)| p.tracks.get(t).is_some_and(|tr| r < tr.regions.len()));
    let audio = match first {
        Some((t, r)) => p.tracks[t].regions[r].audio.is_some(),
        None => p
            .tracks
            .get(p.selected_track)
            .is_some_and(|t| t.kind == TrackKind::Audio),
    };
    match first {
        Some((t, r)) if audio => crate::pitch_editor::open_in(&mut s.pitch_ed, &mut s.bottom, t, r),
        _ if audio => {
            s.pitch_ed.region = None;
            s.bottom = Bottom::PitchEditor;
        }
        _ => {
            s.piano_window = false;
            s.bottom = Bottom::PianoRoll;
        }
    }
}

/// Track numbers shifted, so a remembered region or note selection may point at another track.
fn forget_track_indexes(s: &mut UiState) {
    crate::tracks_area::forget_track_indexes(s);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{Note, Region, RegionParams};
    use crate::ui_test::Harness;
    use egui::{Event, Key, Modifiers};

    pub(crate) fn region(name: &str, start_bar: f32) -> Region {
        Region {
            missing: None,
            name: name.into(),
            start_bar,
            len_bars: 1.0,
            notes: vec![Note {
                lyric: crate::lyrics::Lyric::NONE,
                mute: false,
                start: 0.0,
                len: 1.0,
                pitch: 60,
                vel: 90,
            }],
            controls: Vec::new(),
            folder: None,
            audio: None,
            params: RegionParams::default(),
            generated: None,
        }
    }

    /// Three instrument tracks: "Inst 1" with region "A", "Inst 2" with "B", "Inst 3" empty.
    pub(crate) fn rig() -> (Project, UiState, Harness) {
        let mut p = Project::empty();
        p.add_track(TrackKind::Instrument);
        p.add_track(TrackKind::Instrument);
        p.tracks[0].regions = vec![region("A", 2.0)];
        p.tracks[1].regions = vec![region("B", 3.0)];
        p.selected_track = 0;
        (p, UiState::new(), Harness::new(1400.0, 600.0))
    }

    /// One frame as the app runs it: the view keys, the Tracks area, then the key commands.
    pub(crate) fn app_frame(h: &mut Harness, p: &mut Project, s: &mut UiState, ev: Vec<Event>) {
        let meters = crate::mixer::Meters::default();
        h.frame(ev, |ui| {
            let ctx = ui.ctx().clone();
            crate::key_commands::begin_frame(&ctx);
            crate::menus::handle_keys(&ctx, p, s);
            crate::tracks_area::show(ui, p, s, &meters);
            crate::key_commands::handle(&ctx, p, s);
        });
    }

    /// `key` pressed and let go with `mods` held.
    pub(crate) fn tap(
        h: &mut Harness,
        p: &mut Project,
        s: &mut UiState,
        key: Key,
        mods: Modifiers,
    ) {
        h.modifiers = mods;
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: mods,
        };
        app_frame(h, p, s, vec![ev(true), ev(false)]);
        h.modifiers = Modifiers::NONE;
        app_frame(h, p, s, vec![]);
    }

    fn names(p: &Project) -> Vec<&str> {
        p.tracks.iter().map(|t| t.name.as_str()).collect()
    }

    /// Runs `edit` as one recorded step and checks that one Undo brings `p` back.
    pub(crate) fn one_undo_step(p: &mut Project, edit: impl FnOnce(&mut Project)) {
        let mut history = crate::undo::History::new(p);
        let before = crate::file_commands::content(p);
        edit(p);
        assert!(history.record(p), "the command changed the project");
        assert!(
            !history.record(p),
            "and nothing is left over for a second step"
        );
        assert!(history.undo(p));
        assert!(
            crate::file_commands::content(p) == before,
            "one Undo restores the project"
        );
        assert!(!history.can_undo(), "it was one step");
    }

    #[test]
    fn command_delete_deletes_the_track_whatever_regions_are_selected() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        // Region "B" on the second track is selected; the first track is the selected one.
        s.tracks.select_only(1, 0);
        p.selected_track = 0;
        tap(h, p, s, Key::Backspace, Modifiers::COMMAND);
        assert_eq!(
            names(p),
            vec!["Inst 2", "Inst 3"],
            "the selected track went"
        );
        assert_eq!(p.tracks[0].regions.len(), 1, "region B was not deleted");
        assert!(
            s.tracks.selection().is_empty(),
            "the old region indexes are forgotten"
        );
        // The forward delete key does the same.
        p.selected_track = 1;
        tap(h, p, s, Key::Delete, Modifiers::COMMAND);
        assert_eq!(names(p), vec!["Inst 2"]);
        // The menu shows the key.
        assert_eq!(
            crate::key_commands::combo_of("Delete Track").map(|c| c.shortcut()),
            Some(egui::KeyboardShortcut::new(
                Modifiers::COMMAND,
                Key::Backspace
            ))
        );
    }

    #[test]
    fn plain_delete_deletes_regions_and_only_an_empty_track() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        // A region selected: only the region goes.
        s.tracks.select_only(0, 0);
        tap(h, p, s, Key::Backspace, Modifiers::NONE);
        assert_eq!(p.tracks.len(), 3);
        assert!(p.tracks[0].regions.is_empty());
        // Nothing selected, and the selected track holds a region: nothing happens.
        p.selected_track = 1;
        tap(h, p, s, Key::Backspace, Modifiers::NONE);
        assert_eq!(names(p), vec!["Inst 1", "Inst 2", "Inst 3"]);
        assert!(
            !crate::menus::enabled("Delete", p, s),
            "Edit > Delete is greyed"
        );
        // An empty selected track goes.
        p.selected_track = 2;
        assert!(crate::menus::enabled("Delete", p, s));
        tap(h, p, s, Key::Delete, Modifiers::NONE);
        assert_eq!(names(p), vec!["Inst 1", "Inst 2"]);
    }

    /// Draws the open Track menu window for a frame with `ev`.
    fn dialog_frame(h: &mut Harness, p: &mut Project, s: &mut UiState, ev: Vec<Event>) {
        h.frame(ev, |ui| crate::track_dialogs::show(ui.ctx(), p, s));
    }

    fn click_id(h: &mut Harness, p: &mut Project, s: &mut UiState, id: egui::Id) {
        let at = h.ctx.read_response(id).expect("on screen").rect.center();
        h.click(at, |ui| crate::track_dialogs::show(ui.ctx(), p, s));
    }

    #[test]
    fn option_command_n_opens_new_tracks_and_creates_them_below_the_selected_track() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        h.modifiers = Modifiers::COMMAND | Modifiers::ALT;
        let ev = |pressed| Event::Key {
            key: Key::N,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::COMMAND | Modifiers::ALT,
        };
        // As the app runs a frame: the File keys first, then the rest.
        h.frame(vec![ev(true), ev(false)], |ui| {
            let ctx = ui.ctx().clone();
            crate::key_commands::begin_frame(&ctx);
            if let Some(cmd) = crate::file_commands::pressed(&ctx) {
                s.file_cmd = Some(cmd);
            }
            crate::key_commands::handle(&ctx, p, s);
        });
        h.modifiers = Modifiers::NONE;
        assert_eq!(s.file_cmd, None, "not File > New");
        assert_eq!(
            s.track_dialog,
            Some(crate::track_dialogs::TrackDialog::NewTracks {
                kind: TrackKind::Instrument,
                count: 1
            })
        );
        // Audio, three of them, then Create.
        dialog_frame(h, p, s, vec![]);
        click_id(h, p, s, egui::Id::unique(("new_tracks_kind", 1)));
        for _ in 0..2 {
            click_id(h, p, s, egui::Id::unique(("new_tracks_step", "plus")));
        }
        assert_eq!(
            s.track_dialog,
            Some(crate::track_dialogs::TrackDialog::NewTracks {
                kind: TrackKind::Audio,
                count: 3
            })
        );
        let mut history = crate::undo::History::new(p);
        click_id(h, p, s, egui::Id::unique(("track_dialog_key", "create")));
        assert!(s.track_dialog.is_none(), "closed");
        assert_eq!(
            names(p),
            vec!["Inst 1", "Audio 1", "Audio 2", "Audio 3", "Inst 2", "Inst 3"],
            "below the selected track"
        );
        assert!(p.tracks[1..4].iter().all(|t| t.kind == TrackKind::Audio));
        assert_eq!(p.selected_track, 1, "the first new track is selected");
        assert!(history.record(p));
        assert!(history.undo(p));
        assert_eq!(p.tracks.len(), 3, "one Undo takes all three away");
    }

    #[test]
    fn new_tracks_return_creates_escape_cancels_and_the_count_stays_in_range() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        crate::menus::apply("New Tracks…", p, s);
        assert!(crate::menus::enabled("New Tracks…", p, s));
        dialog_frame(h, p, s, vec![]);
        // Down never goes under one.
        h.key(Key::ArrowDown, |ui| {
            crate::track_dialogs::show(ui.ctx(), p, s)
        });
        h.key(Key::Escape, |ui| crate::track_dialogs::show(ui.ctx(), p, s));
        assert!(s.track_dialog.is_none());
        assert_eq!(p.tracks.len(), 3, "Cancel makes nothing");
        crate::menus::apply("New Tracks…", p, s);
        if let Some(crate::track_dialogs::TrackDialog::NewTracks { count, .. }) =
            s.track_dialog.as_mut()
        {
            *count = MAX_NEW_TRACKS;
        }
        dialog_frame(h, p, s, vec![]);
        h.key(Key::ArrowUp, |ui| {
            crate::track_dialogs::show(ui.ctx(), p, s)
        });
        p.selected_track = 2;
        h.key(Key::Enter, |ui| crate::track_dialogs::show(ui.ctx(), p, s));
        assert!(s.track_dialog.is_none());
        assert_eq!(p.tracks.len(), 3 + MAX_NEW_TRACKS as usize);
        assert_eq!(p.tracks[3].name, "Inst 4", "after the last track");
    }

    #[test]
    fn new_tracks_go_after_a_stack_and_into_the_stack_a_track_is_in() {
        let (mut p, _, _) = rig();
        // A stack holding the first track, then the second and third.
        p.create_track_stack(0);
        assert_eq!(p.tracks[1].indent, 1);
        p.selected_track = 0;
        let at = add_tracks_below(&mut p, TrackKind::Instrument, 1);
        assert_eq!(
            (at, p.tracks[at].indent),
            (2, 0),
            "after the stack, beside it"
        );
        p.selected_track = 1;
        let at = add_tracks_below(&mut p, TrackKind::Aux, 1);
        assert_eq!((at, p.tracks[at].indent), (2, 1), "inside the stack");
        // A VCA assignment still points at its VCA after tracks go in above it.
        let vca = p.add_track(TrackKind::Vca);
        assert!(p.assign_vca(0, Some(vca)));
        p.selected_track = 0;
        add_tracks_below(&mut p, TrackKind::Audio, 2);
        assert_eq!(p.tracks[p.tracks[0].vca.unwrap()].kind, TrackKind::Vca);
    }

    #[test]
    fn command_d_makes_a_track_with_the_same_settings_and_no_regions() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        {
            let t = &mut p.tracks[0];
            t.volume_db = -6.0;
            t.pan = 0.25;
            t.mute = true;
            t.rec = true;
            t.hidden = true;
            t.params.groove = true;
            t.inserts = crate::model::inserts_from_names(&["Channel EQ"]);
            t.au_instrument = Some(crate::plugins::PluginSlot::new(
                "aumu:abcd:Mnfr",
                "M: Synth",
            ));
            t.au_effects = vec![crate::plugins::PluginSlot::new("aufx:efgh:Mnfr", "M: Verb")];
            t.auto
                .volume
                .push(prowave_engine::AutoPoint::new(4.0, -3.0));
        }
        app_frame(h, p, s, vec![]);
        s.tracks.select_only(1, 0);
        let mut history = crate::undo::History::new(p);
        tap(h, p, s, Key::D, Modifiers::COMMAND);
        assert_eq!(p.tracks.len(), 4);
        assert_eq!(
            p.selected_track, 1,
            "the new track, right below, is selected"
        );
        let (a, b) = (&p.tracks[0], &p.tracks[1]);
        assert_eq!((b.name.as_str(), b.kind), (a.name.as_str(), a.kind));
        assert_eq!((b.volume_db, b.pan, b.mute), (a.volume_db, a.pan, a.mute));
        assert_eq!(b.inserts.len(), 1);
        assert!(b.regions.is_empty(), "no regions");
        assert!(b.auto.volume.is_empty(), "no automation");
        assert!(!b.rec && !b.params.groove);
        assert!(!b.hidden, "the new track is shown");
        let (ai, bi) = (
            a.au_instrument.as_ref().unwrap(),
            b.au_instrument.as_ref().unwrap(),
        );
        assert_eq!(ai.id, bi.id);
        assert_ne!(ai.uid, bi.uid, "its own instance");
        assert_ne!(a.au_effects[0].uid, b.au_effects[0].uid);
        assert_eq!(
            p.tracks[2].regions[0].name, "B",
            "the tracks after moved down"
        );
        assert!(
            s.tracks.selection().is_empty(),
            "old region indexes forgotten"
        );
        assert!(history.record(p));
        assert!(history.undo(p));
        assert_eq!(p.tracks.len(), 3, "one undo step");
        // Not for a track stack.
        p.create_track_stack(0);
        p.selected_track = 0;
        assert!(!crate::menus::enabled(
            "New Track with Duplicate Settings",
            p,
            s
        ));
        assert_eq!(duplicate_settings_of(p, 0), None);
    }

    #[test]
    fn the_duplicate_key_over_the_headers_duplicates_settings_only() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        let at = h
            .ctx
            .read_response(egui::Id::unique(("tracks_area", ("icon", "dup"))))
            .expect("the key is on screen")
            .rect
            .center();
        let meters = crate::mixer::Meters::default();
        h.click(at, |ui| crate::tracks_area::show(ui, p, s, &meters));
        assert_eq!(p.tracks.len(), 4);
        assert!(p.tracks[1].regions.is_empty());
    }

    #[test]
    fn g_hides_the_global_tracks_together_and_brings_back_the_ones_that_were_shown() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        let item = "Show Global Tracks";
        assert!(crate::menus::implemented(item) && crate::menus::enabled(item, p, s));
        assert!(!crate::menus::checked(item, p, s));
        // None shown: G shows all three.
        tap(h, p, s, Key::G, Modifiers::NONE);
        let v = &s.tracks;
        assert!(v.show_markers && v.show_signatures && v.show_tempo);
        assert!(crate::menus::checked(item, p, s));
        // Only Markers and Tempo shown: G hides them, G again brings back just those.
        s.tracks.show_signatures = false;
        tap(h, p, s, Key::G, Modifiers::NONE);
        let v = &s.tracks;
        assert!(!v.show_markers && !v.show_signatures && !v.show_tempo);
        tap(h, p, s, Key::G, Modifiers::NONE);
        let v = &s.tracks;
        assert!(v.show_markers && !v.show_signatures && v.show_tempo);
        // Shift-G is not G.
        tap(h, p, s, Key::G, Modifiers::SHIFT);
        assert!(s.tracks.show_markers);
    }

    /// Two plain frames, so that what egui remembers of the frame before is gone too.
    fn settle(h: &mut Harness, p: &mut Project, s: &mut UiState) {
        app_frame(h, p, s, vec![]);
        app_frame(h, p, s, vec![]);
    }

    /// Whether track `i`'s header was drawn in the last two frames (see [`settle`]).
    fn header_shown(h: &Harness, i: usize) -> bool {
        h.ctx
            .read_response(egui::Id::unique(("tracks_area", ("hdr", i))))
            .is_some()
    }

    #[test]
    fn control_h_hides_the_track_selects_the_next_and_h_shows_hidden_tracks() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        assert!((0..3).all(|i| header_shown(h, i)));
        s.tracks.select_only(0, 0);
        assert!(crate::menus::implemented("Hide Track"));
        assert!(
            !crate::menus::enabled("Unhide All Tracks", p, s),
            "nothing hidden yet"
        );
        let mut history = crate::undo::History::new(p);
        tap(h, p, s, Key::H, Modifiers::CTRL);
        assert!(p.tracks[0].hidden);
        settle(h, p, s);
        assert!(!header_shown(h, 0), "the hidden track has no row");
        assert!(header_shown(h, 1) && header_shown(h, 2));
        assert_eq!(p.selected_track, 1, "the next track is selected");
        assert!(
            s.tracks.selection().is_empty(),
            "its regions left the selection"
        );
        assert!(history.record(p), "one undo step");
        // It still plays: its notes are in the song.
        let notes = crate::engine_sync::song_notes_for_test(p);
        assert!(notes.iter().any(|n| n.track == 0), "the hidden track plays");
        // H: hidden tracks shown again, with an H key on every header, lit on the hidden one.
        tap(h, p, s, Key::H, Modifiers::NONE);
        assert!(s.tracks.show_hidden && crate::menus::checked("Show Hidden Tracks", p, s));
        settle(h, p, s);
        assert!(header_shown(h, 0));
        let key = |i: usize| egui::Id::unique(("tracks_area", ("hdrbtn", (i, "h"))));
        assert!(h.ctx.read_response(key(0)).is_some());
        // Its H key shows it again; another track's hides that one.
        let meters = crate::mixer::Meters::default();
        for i in [0, 2] {
            let at = h.ctx.read_response(key(i)).expect("H key").rect.center();
            h.click(at, |ui| crate::tracks_area::show(ui, p, s, &meters));
        }
        assert!(!p.tracks[0].hidden && p.tracks[2].hidden);
        // H again: the hidden one goes, and the H keys with it.
        tap(h, p, s, Key::H, Modifiers::NONE);
        assert!(!s.tracks.show_hidden);
        settle(h, p, s);
        assert!(!header_shown(h, 2) && h.ctx.read_response(key(0)).is_none());
        // Track > Unhide All Tracks.
        assert!(crate::menus::enabled("Unhide All Tracks", p, s));
        crate::menus::apply("Unhide All Tracks", p, s);
        settle(h, p, s);
        assert!((0..3).all(|i| header_shown(h, i)));
        // The menu shows the keys.
        let combo = |l| crate::key_commands::combo_of(l).map(|c| c.shortcut());
        assert_eq!(
            combo("Hide Track"),
            Some(egui::KeyboardShortcut::new(Modifiers::CTRL, Key::H))
        );
        assert_eq!(
            combo("Show Hidden Tracks"),
            Some(egui::KeyboardShortcut::new(Modifiers::NONE, Key::H))
        );
    }

    #[test]
    fn a_hidden_track_stack_hides_its_tracks_and_hidden_survives_a_save() {
        let (mut p, mut s, _) = rig();
        p.create_track_stack(1);
        // Stack at 1 holding "Inst 2" at 2.
        p.selected_track = 1;
        crate::track_selection::hide_tracks(&mut p, &mut s);
        assert!(hidden(&p, 1) && hidden(&p, 2) && !hidden(&p, 3));
        assert!(takes_no_room(&p, false, 2) && !takes_no_room(&p, true, 2));
        assert_eq!(p.selected_track, 3, "past the stack's contents");
        // Hide Track on a hidden track shows it again.
        p.selected_track = 1;
        crate::track_selection::hide_tracks(&mut p, &mut s);
        assert!(!hidden(&p, 2));
        p.tracks[2].hidden = true;
        let (doc, _) = crate::project_io::to_doc(&p);
        let (back, _) =
            crate::project_io::from_doc(&doc, std::path::Path::new("."), &egui::Context::default());
        let flags: Vec<bool> = back.tracks.iter().map(|t| t.hidden).collect();
        assert_eq!(flags, vec![false, false, true, false]);
        // A file from before hidden tracks loads with every track shown.
        let old: prowave_project::TrackDoc = serde_json::from_str(r#"{"name":"Old"}"#).unwrap();
        assert!(!old.hidden);
    }

    #[test]
    fn hide_track_is_one_undo_step() {
        let (mut p, mut s, _) = rig();
        one_undo_step(&mut p, |p| crate::menus::apply("Hide Track", p, &mut s));
    }

    #[test]
    fn delete_track_is_one_undo_step() {
        let (mut p, mut s, _) = rig();
        one_undo_step(&mut p, |p| crate::menus::apply("Delete Track", p, &mut s));
        assert_eq!(p.tracks.len(), 3);
    }

    #[test]
    fn shift_command_d_creates_a_track_stack_around_the_selected_track() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        p.selected_track = 1;
        tap(h, p, s, Key::D, Modifiers::COMMAND | Modifiers::SHIFT);
        assert_eq!(p.tracks.len(), 4, "one folder, no duplicate track");
        assert_eq!(p.tracks[1].kind, TrackKind::Folder);
        assert_eq!(
            (p.tracks[2].name.as_str(), p.tracks[2].indent),
            ("Inst 2", 1)
        );
        assert_eq!(p.selected_track, 1, "the stack is selected");
        assert_eq!(
            crate::key_commands::combo_of("Create Track Stack…").map(|c| c.shortcut()),
            Some(egui::KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::D
            ))
        );
        let (mut p, mut s, _) = rig();
        one_undo_step(&mut p, |p| {
            crate::menus::apply("Create Track Stack…", p, &mut s)
        });
    }

    #[test]
    fn shift_command_d_is_not_taken_by_an_automation_range() {
        use crate::automation::Strip;
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        crate::automation_ui::set_mode(p, &mut s.auto, true);
        app_frame(h, p, s, vec![]);
        s.auto.lanes.sel = Some(crate::automation_ui::Sel {
            strip: Strip::Track(0),
            target: prowave_engine::AutoTarget::Volume,
            from: 0.0,
            to: 4.0,
        });
        p.selected_track = 1;
        tap(h, p, s, Key::D, Modifiers::COMMAND | Modifiers::SHIFT);
        assert_eq!(p.tracks[1].kind, TrackKind::Folder, "a stack was made");
        assert_eq!(p.tracks.len(), 4, "and nothing else was duplicated");
    }

    #[test]
    fn shift_return_names_the_selected_track_and_return_takes_the_name() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        p.selected_track = 1;
        let before = p.position_beats;
        p.position_beats = 8.0;
        tap(h, p, s, Key::Enter, Modifiers::SHIFT);
        assert_eq!(p.position_beats, 8.0, "not Go to Beginning (Return)");
        p.position_beats = before;
        assert!(matches!(
            s.track_dialog,
            Some(crate::track_dialogs::TrackDialog::Rename { track: 1, .. })
        ));
        // The field has the keyboard at once: typing goes into the name, Return takes it.
        dialog_frame(h, p, s, vec![]);
        dialog_frame(h, p, s, vec![Event::Text(" Lead".into())]);
        let enter = |pressed| Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        dialog_frame(h, p, s, vec![enter(true), enter(false)]);
        assert_eq!(p.tracks[1].name, "Inst 2 Lead");
        assert_eq!(s.track_dialog, None, "the window closed");
        // The next time it opens, it has the keyboard again.
        tap(h, p, s, Key::Enter, Modifiers::SHIFT);
        dialog_frame(h, p, s, vec![]);
        dialog_frame(h, p, s, vec![Event::Text("!".into())]);
        dialog_frame(h, p, s, vec![enter(true), enter(false)]);
        assert_eq!(p.tracks[1].name, "Inst 2 Lead!");
    }

    #[test]
    fn delete_unused_tracks_deletes_tracks_without_regions_and_keeps_aux_and_used_vcas() {
        let (mut p, mut s, _) = rig();
        // 0 Inst 1 (A), 1 Inst 2 (B), 2 Inst 3 (empty), then an Aux, an assigned VCA, an empty
        // audio track, an unassigned VCA, and an empty stack.
        p.add_track(TrackKind::Aux);
        let vca = p.add_track(TrackKind::Vca);
        p.tracks[0].vca = Some(vca);
        p.add_track(TrackKind::Audio);
        p.add_track(TrackKind::Vca);
        p.add_track(TrackKind::Folder);
        assert!(crate::menus::enabled("Delete Unused Tracks", &p, &s));
        assert!(crate::menus::all_entries().contains(&"Delete Unused Tracks"));
        let kinds = |p: &Project| p.tracks.iter().map(|t| t.kind).collect::<Vec<_>>();
        one_undo_step(&mut p, |p| {
            assert_eq!(delete_unused(p, &mut s), 4);
            assert_eq!(
                kinds(p),
                vec![
                    TrackKind::Instrument,
                    TrackKind::Instrument,
                    TrackKind::Aux,
                    TrackKind::Vca
                ]
            );
            assert_eq!(
                p.tracks[0].vca,
                Some(3),
                "the VCA assignment follows its track"
            );
        });
        // A stack whose tracks are all unused goes with them; the last track always stays.
        let mut p = Project::empty();
        p.tracks[0].regions.clear();
        p.add_track(TrackKind::Instrument);
        p.create_track_stack(1);
        assert_eq!(p.tracks.len(), 3);
        crate::menus::apply("Delete Unused Tracks", &mut p, &mut s);
        assert_eq!(p.tracks.len(), 1);
        assert!(!crate::menus::enabled("Delete Unused Tracks", &p, &s));
        // Not while recording.
        let (mut p, s, _) = rig();
        p.recording = true;
        assert!(!crate::menus::enabled("Delete Unused Tracks", &p, &s));
    }

    #[test]
    fn e_shows_the_editor_of_the_selected_region_and_hides_it() {
        use crate::Bottom;
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        app_frame(h, p, s, vec![]);
        s.bottom = Bottom::Mixer;
        s.tracks.select_only(0, 0);
        tap(h, p, s, Key::E, Modifiers::NONE);
        assert!(
            s.bottom == Bottom::PianoRoll,
            "a MIDI region: the Piano Roll"
        );
        assert!(crate::menus::checked("Show Editor", p, s));
        tap(h, p, s, Key::E, Modifiers::NONE);
        assert!(s.bottom == Bottom::Mixer, "E again hides it");
        // An audio region opens in the Pitch Editor, on that region.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../prowave-engine/tests/fixtures");
        let at = p.add_track(TrackKind::Audio);
        let mut r = region("Vox", 2.0);
        r.notes.clear();
        r.audio = Some(crate::model::AudioRegion {
            file: prowave_engine::AudioFile::open(dir.join("tone.caf")).expect("fixture"),
            peaks: Default::default(),
            flex: None,
        });
        p.tracks[at].regions.push(r);
        app_frame(h, p, s, vec![]);
        s.tracks.select_only(at, 0);
        tap(h, p, s, Key::E, Modifiers::NONE);
        assert!(s.bottom == Bottom::PitchEditor);
        assert_eq!(s.pitch_ed.region, Some((at, 0)));
        // With no region selected, the selected track's kind decides.
        tap(h, p, s, Key::E, Modifiers::NONE);
        s.tracks.clear_selection();
        p.selected_track = 1;
        tap(h, p, s, Key::E, Modifiers::NONE);
        assert!(s.bottom == Bottom::PianoRoll);
        assert_eq!(
            crate::key_commands::combo_of("Show Editor").map(|c| c.shortcut()),
            Some(egui::KeyboardShortcut::new(Modifiers::NONE, Key::E))
        );
    }
}
