//! Top menu bar with the same menus and entries as Logic Pro. Each entry is drawn like a macOS
//! menu item: check-mark column, title, key command on the right; entries that are not built
//! are not shown (see [`implemented`]) and entries that do not apply now are greyed out (see
//! [`enabled`]). Drawn in the Hardware look: a chassis strip with the PW-1 wordmark and the
//! menu titles in silkscreen caps (the open one pressed in like a black key), and drop-down
//! menus in the black field material or a bone faceplate (the entry under the pointer pressed
//! into the panel with an orange edge, an LED on every entry that can be checked). Where the
//! bar is in the window, the MENUS key in the control bar switches the drop-downs between the
//! two (`UiState::light_menus`).
use crate::file_commands::FileCmd;
use crate::hw;
use crate::i18n::{self, tr, trf, Lang};
use crate::model::{Project, TrackKind};
use crate::project_settings::SettingsTab;
use crate::theme::{self, Face};
use crate::track_dialogs;
use crate::transport::hw_transport;
use crate::undo::UndoCmd;
use crate::Bottom;
use egui::{pos2, vec2, Align2, Color32, Key, KeyboardShortcut, Modifiers, Sense, Stroke};

const FILE: &[&str] = &[
    "New…",
    "New from Template…",
    "Open…",
    "Open Recent",
    "-",
    "Close",
    "Close Project",
    "Save",
    "Save As…",
    "Save A Copy As…",
    "Save as Template…",
    "Revert to",
    "-",
    "Alternatives",
    "Project Management",
    "Project Settings",
    "-",
    "Page Setup…",
    "Print",
    "-",
    "Movie",
    "-",
    "Import",
    "Export",
    "Bounce",
    "Share",
    "-",
    "Show in File Manager",
    "Open in Sound Library Manager",
    "-",
    "Project Alternatives…",
];

/// Entries that open a submenu (▸), as in Logic, and what is in each. "Open Recent" is filled
/// with the remembered projects instead.
const SUBMENUS: &[(&str, &[&str])] = &[
    ("Revert to", &["Revert to Saved"]),
    (
        "Alternatives",
        &[
            "New Alternative…",
            "Rename Alternative…",
            "Delete Alternative",
        ],
    ),
    (
        "Project Management",
        &["Clean Up…", "Consolidate…", "Rename Project…"],
    ),
    (
        "Project Settings",
        &[
            "General…",
            "Synchronization…",
            "Metronome…",
            "Recording…",
            "Tuning…",
            "Audio…",
            "MIDI…",
        ],
    ),
    ("Movie", &["Open Movie…", "Remove Movie"]),
    (
        "Import",
        &[
            "Audio File…",
            "MIDI File…",
            "Paint Image to Audio…",
            "Other Projects…",
            "-",
            "Fit to Grid on Drop",
        ],
    ),
    (
        "Export",
        &[
            "Selection as MIDI File…",
            "Track as Audio File…",
            "All Tracks as Audio Files…",
            "Score as MusicXML…",
        ],
    ),
    (
        "Bounce",
        &[
            "Project or Section…",
            "Regions in Place…",
            "Tracks in Place…",
        ],
    ),
    ("Share", &["Song to File…", "Song to Mobile Device…"]),
    ("Language", LANGUAGE),
    ("I/O Buffer Size", BUFFER_SIZES),
    ("Process Buffer", &["Small", "Medium", "Large"]),
    (
        "Space Designer",
        &[
            "Show",
            "Mastering Glue - Small Room",
            "Mastering Plate - Vocal Sheen",
            "Mastering Wide Hall",
        ],
    ),
];

/// Record > I/O Buffer Size (Logic: Settings > Audio > I/O Buffer Size), in frames.
const BUFFER_SIZES: &[&str] = &[
    "32 Samples",
    "64 Samples",
    "128 Samples",
    "256 Samples",
    "512 Samples",
    "1024 Samples",
];

/// The frames of an I/O Buffer Size entry.
fn buffer_frames(label: &str) -> Option<u32> {
    if !BUFFER_SIZES.contains(&label) {
        return None;
    }
    label.split(' ').next()?.parse().ok()
}

/// What is in the submenu opened by `label`, if it opens one.
pub(crate) fn submenu(label: &str) -> Option<&'static [&'static str]> {
    SUBMENUS
        .iter()
        .find(|(title, _)| *title == label)
        .map(|(_, items)| *items)
}

/// Every entry that runs something, submenus included (not separators or submenu titles).
#[cfg(test)]
pub(crate) fn all_entries() -> Vec<&'static str> {
    let mut out = Vec::new();
    for (_, items) in MENUS {
        for &it in items {
            match submenu(it) {
                Some(children) => out.extend_from_slice(children),
                None => out.push(it),
            }
        }
    }
    out.retain(|it| *it != "-" && *it != "Open Recent");
    out
}
const EDIT: &[&str] = &[
    "Undo",
    "Redo",
    "Undo History…",
    "Delete Undo History",
    "-",
    "Cut",
    "Copy",
    "Paste",
    "Paste Replace",
    "Delete",
    "-",
    "Select All",
    "Deselect All",
    "-",
    "Split at Playhead",
    "Join",
    "Bounce and Join",
];
const TRACK: &[&str] = &[
    "New",
    "New Software Instrument Track",
    "New Audio Track",
    "New Aux Track",
    "New VCA Track",
    "New External MIDI Track",
    "New Pattern Region",
    "New Drummer Region",
    "MIDI FX…",
    "Rename Track",
    "Delete Track",
    "-",
    "Create Track Stack…",
    "Release Track Stack",
    "Show Master Track",
    "Show Tempo Track",
    "Show Chord Track",
    "Show Marker Track",
    "Show Signature Track",
    "Show Arrangement Track",
    "-",
    "Assign Track Color…",
    "Assign Track Icon…",
    "Configure Track Header…",
];
const NAVIGATE: &[&str] = &[
    "Go to Beginning",
    "Go to End",
    "Go to Position…",
    "-",
    "Go to Left Locator",
    "Go to Right Locator",
    "-",
    "Go to Next Marker",
    "Go to Previous Marker",
    "Create Marker",
    "Delete Marker",
    "-",
    "Go to Next Region",
    "Go to Previous Region",
    "-",
    "Zoom to Fit",
    "Zoom In",
    "Zoom Out",
];
const RECORD: &[&str] = &[
    "Record",
    "Record / Record Repeat",
    "Record Toggle",
    "Record Enable",
    "-",
    "Capture Recording",
    "-",
    "Software Monitoring",
    "Low Latency Mode",
    "I/O Buffer Size",
    "Process Buffer",
    "Record / Mix Mode",
    "Switch to Record Mode When Arming",
    "Send All Notes Off",
    "-",
    "New Take",
    "Delete Take",
    "Bounce In Place",
    "-",
    "Record Project Settings",
    "Project Settings",
];
const MIX: &[&str] = &[
    "Show Automation",
    "Create Automation",
    "Convert Automation",
    "Delete Automation",
    "-",
    "Group",
    "Edit Groups…",
    "-",
    "Pre-Fader Metering",
    "Mastering",
    "Create Mastering Track",
    "Stem Splitter…",
    "-",
    "Space Designer",
    "-",
    "Bounce",
    "Export",
    "Import",
];
/// Logic Pro 11's View menu.
const VIEW: &[&str] = &[
    "Hide All Automation",
    "Show Automation",
    "Create Track Automation",
    "Convert Automation",
    "-",
    "Show Region Automation",
    "Show Track Automation",
    "-",
    "Show Advanced Tools",
    "Show Smart Controls",
    "-",
    "Show Piano Roll",
    "Show Score Editor",
    "Show Step Sequencer",
    "Show Session Player",
    "Show Flex Editor",
    "-",
    "Show Mixer",
    "Show Library",
    "Show List Editors",
    "Show Tips",
    "Show Inspector",
    "Show Performers",
    "Show Project Notes",
    "Show Movie",
    "Show Media Browser",
    "-",
    "Show Toolbar",
    "Customize Toolbar…",
    "-",
    "Show Track Header",
    "Show Ruler",
    "Show Global Tracks",
    "Show Colors",
    "Show Markers",
    "Show Grid",
    "Show Smart Tempo",
    "-",
    "Bigger Interface",
    "Smaller Interface",
    "Actual Interface Size",
    "-",
    "Enter Full Screen",
];
const WINDOW: &[&str] = &[
    "Minimize",
    "Zoom",
    "-",
    "Channel Rack",
    "Pitch Editor",
    "MIDI Monitor",
    "Score Editor",
    "Step Sequencer",
    "Smart Tempo",
    "Open Piano Roll",
    "Mixer",
    "Plugin Manager",
    "Get Free Plug-ins",
    "Video",
    "-",
    "Bring All to Front",
];
const HELP: &[&str] = &[
    "Tips",
    "Logic-style Key Commands…",
    "-",
    "Language",
    "-",
    "Check for Updates…",
    "Check for Updates Automatically",
    "-",
    "About Pro Wave (Yuri's Version)",
];

/// Help > Language: every language, each named in its own language.
const LANGUAGE: &[&str] = &[
    "English (Default)",
    "简体中文",
    "日本語",
    "Français",
    "Deutsch",
    "Español",
    "Português (Portugal)",
    "Português (Brasil)",
    "Filipino",
    "Bahasa Indonesia",
    "हिन्दी",
    "Українська",
    "Norsk",
    "Svenska",
    "Suomi",
    "Polski",
    "Lietuvių",
];

/// The menus in bar order, used by the in-window bar and the macOS menu bar.
pub(crate) const MENUS: [(&str, &[&str]); 9] = [
    ("File", FILE),
    ("Edit", EDIT),
    ("Track", TRACK),
    ("Navigate", NAVIGATE),
    ("Record", RECORD),
    ("Mix", MIX),
    ("View", VIEW),
    ("Window", WINDOW),
    ("Help", HELP),
];

/// Entries that can show a check mark (see [`checked`]).
pub(crate) const CHECKABLE: &[&str] = &[
    "Show Automation",
    "Show Tempo Track",
    "Show Marker Track",
    "Show Signature Track",
    "Show Master Track",
    "Show Piano Roll",
    "Show Mixer",
    "Show Inspector",
    "Show List Editors",
    "Show Tips",
    "Show Project Notes",
    "Show Media Browser",
    "Check for Updates Automatically",
    "Software Monitoring",
    "Low Latency Mode",
    "Fit to Grid on Drop",
    "Channel Rack",
    "Pitch Editor",
    "MIDI Monitor",
    "Get Free Plug-ins",
    "32 Samples",
    "64 Samples",
    "128 Samples",
    "256 Samples",
    "512 Samples",
    "1024 Samples",
    "Small",
    "Medium",
    "Large",
    "Switch to Record Mode When Arming",
    "Show Toolbar",
    "Show Track Header",
    "Show Ruler",
    "Show Colors",
    "Show Grid",
];

/// Whether `label` shows a check mark when it is on: [`CHECKABLE`] and the Language entries.
pub(crate) fn is_checkable(label: &str) -> bool {
    CHECKABLE.contains(&label) || Lang::from_native_name(label).is_some()
}

/// Whether the menu bar is drawn inside the window. On macOS the menus go in the system menu
/// bar at the top of the screen instead (`native_menu`).
pub const IN_WINDOW: bool = !cfg!(target_os = "macos");

/// Colours of one menu look.
struct Look {
    bar: Color32,
    bar_text: Color32,
    menu: Color32,
    text: Color32,
    /// Entries that do not apply now.
    dim: Color32,
    /// Key commands.
    key: Color32,
    /// The entry under the pointer: a recessed bar and how deep its shadow is.
    hover: Color32,
    hover_shade: f32,
    hover_text: Color32,
    /// A seam between groups: its dark line and the light line under it.
    separator: Color32,
    separator_hi: Color32,
}

/// Drop-downs in the black field material of the value fields.
const DARK: Look = Look {
    bar: theme::BONE,
    bar_text: theme::INK,
    menu: theme::CAP,
    text: theme::ON_DARK,
    dim: theme::ON_DARK_DIM,
    key: theme::ON_DARK_DIM,
    hover: Color32::from_rgb(0x12, 0x11, 0x10),
    hover_shade: 0.5,
    hover_text: theme::ON_DARK,
    separator: Color32::from_rgb(0x0b, 0x0b, 0x0a),
    separator_hi: Color32::from_rgb(0x3a, 0x38, 0x35),
};

/// Drop-downs in a bone faceplate, printed in ink.
const LIGHT: Look = Look {
    bar: theme::BONE,
    bar_text: theme::INK,
    menu: theme::PANEL,
    text: theme::INK,
    dim: theme::PRINT,
    key: theme::INK2,
    hover: theme::BONE2,
    hover_shade: 0.22,
    hover_text: theme::INK,
    separator: theme::SEAM,
    separator_hi: theme::BONE_HI,
};

fn look(light: bool) -> &'static Look {
    if light {
        &LIGHT
    } else {
        &DARK
    }
}

/// Fill for the panel behind the menu bar.
pub fn bar_fill(light: bool) -> Color32 {
    look(light).bar
}

/// Logic's default key commands for the entries this app implements.
fn shortcut(label: &str) -> Option<KeyboardShortcut> {
    if let Some(cmd) = FileCmd::from_menu(label) {
        return cmd.shortcut();
    }
    match label {
        "Undo" => return Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Z)),
        "Redo" => {
            return Some(KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::Z,
            ))
        }
        "Delete" => return Some(KeyboardShortcut::new(Modifiers::NONE, Key::Delete)),
        _ => {}
    }
    let interface_key = match label {
        "Bigger Interface" => Some(Key::Equals),
        "Smaller Interface" => Some(Key::Minus),
        "Actual Interface Size" => Some(Key::Num0),
        _ => None,
    };
    if let Some(key) = interface_key {
        return Some(KeyboardShortcut::new(Modifiers::COMMAND, key));
    }
    if label == "Open Piano Roll" {
        return Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Num4));
    }
    // Logic has no key command for this; Shift-Command-L is free here (L for latency).
    if label == "Record / Mix Mode" {
        return Some(KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::L,
        ));
    }
    let key = match label {
        "Show Inspector" => Key::I,
        "Show List Editors" => Key::D,
        "Show Mixer" => Key::X,
        "Show Piano Roll" => Key::P,
        _ => return None,
    };
    Some(KeyboardShortcut::new(Modifiers::NONE, key))
}

/// Whether track `t` has a Space Designer or has room for one.
fn space_designer_room(t: &crate::model::Track) -> bool {
    t.kind != TrackKind::Vca
        && (t.inserts.len() < prowave_engine::MAX_INSERTS
            || t.inserts
                .iter()
                .any(|i| i.kind == prowave_engine::FxKind::Reverb))
}

/// Whether the master bus has a Space Designer or room for one.
fn master_has_room(p: &Project) -> bool {
    p.master_inserts.len() < prowave_engine::MAX_INSERTS
        || p.master_inserts
            .iter()
            .any(|i| i.kind == prowave_engine::FxKind::Reverb)
}

/// Opens the Space Designer in `inserts` (adding one when there is none) as the effect window of
/// strip `strip`, set to mastering preset `preset` if given.
fn show_space_designer(
    inserts: &mut Vec<crate::model::Insert>,
    s: &mut crate::UiState,
    strip: usize,
    preset: Option<usize>,
) {
    use prowave_engine::FxKind;
    let slot = inserts
        .iter()
        .position(|i| i.kind == FxKind::Reverb)
        .or_else(|| {
            (inserts.len() < prowave_engine::MAX_INSERTS).then(|| {
                inserts.push(crate::model::Insert::new(FxKind::Reverb));
                inserts.len() - 1
            })
        });
    if let Some(slot) = slot {
        if let Some(i) = preset {
            inserts[slot].params = crate::space_designer_ui::mastering_preset(i);
        }
        s.mixer.fx_window = Some((strip, slot));
    }
}

/// Whether `label` can be chosen now. Entries that are not implemented yet are greyed out,
/// so a click never silently does nothing; others are greyed while there is nothing to act on.
pub(crate) fn enabled(label: &str, p: &Project, s: &crate::UiState) -> bool {
    if FileCmd::from_menu(label).is_some() || is_checkable(label) && !CHECKABLE.contains(&label) {
        return true;
    }
    if SettingsTab::from_menu(label).is_some() {
        return true;
    }
    match label {
        "Undo" => s.can_undo,
        "Redo" => s.can_redo,
        "Delete Undo History" => s.can_undo || s.can_redo,
        "Delete" => s.tracks.selected.is_some() || p.tracks.len() > 1,
        "Deselect All" | "Copy" | "Cut" => s.tracks.selected.is_some(),
        "Rename Track" | "Assign Track Color…" | "Assign Track Icon…" => {
            p.tracks.get(p.selected_track).is_some()
        }
        "Configure Track Header…" | "Show Master Track" | "Show Automation" => true,
        // Live's Delete Automation: the envelope the selected track shows.
        "Delete Automation" => p
            .tracks
            .get(p.selected_track)
            .is_some_and(|t| !t.auto.lane(t.auto.shown).is_empty()),
        // Opens the Space Designer on the selected track (adding one when it has none). The
        // mastering presets go on the Stereo Out strip (the master bus).
        "Show" => p
            .tracks
            .get(p.selected_track)
            .is_some_and(space_designer_room),
        "Mastering Glue - Small Room" | "Mastering Plate - Vocal Sheen" | "Mastering Wide Hall" => {
            master_has_room(p)
        }
        "Paste" => crate::region_edit::can_paste(p, &s.region_clip),
        "Select All" => p.tracks.iter().any(|t| !t.regions.is_empty()),
        "Join" => crate::region_edit::can_join(p, &s.tracks),
        "Split at Playhead" => crate::region_edit::can_split(p, &s.tracks),
        "Delete Track" => p.tracks.len() > 1,
        "Release Track Stack" => p
            .tracks
            .get(p.selected_track)
            .is_some_and(|t| t.kind == TrackKind::Folder),
        "Go to Next Marker" => p.next_marker(p.position_beats).is_some(),
        "Go to Previous Marker" => p.previous_marker(p.position_beats).is_some(),
        "Delete Marker" => p.marker_at(p.position_beats).is_some(),
        // Pattern and Drummer regions go on a track that plays MIDI.
        "New Pattern Region" | "New Drummer Region" => p
            .tracks
            .get(p.selected_track)
            .is_some_and(|t| t.kind.has_regions() && t.kind != TrackKind::Audio),
        "Go to Next Region" | "Go to Previous Region" => {
            p.tracks.iter().any(|t| !t.regions.is_empty())
        }
        _ => {
            ALWAYS.contains(&label)
                || buffer_frames(label).is_some()
                || crate::engine_mode::process_frames(label).is_some()
        }
    }
}

/// Entries that always work.
const ALWAYS: &[&str] = &[
    "MIDI Monitor",
    "Get Free Plug-ins",
    "Send All Notes Off",
    "Channel Rack",
    "Pitch Editor",
    "New Software Instrument Track",
    "New Audio Track",
    "New from Template…",
    "New",
    "New Aux Track",
    "New VCA Track",
    "New External MIDI Track",
    "Create Track Stack…",
    "Go to Beginning",
    "Go to End",
    "Go to Left Locator",
    "Go to Right Locator",
    "Zoom to Fit",
    "Zoom In",
    "Zoom Out",
    "Record",
    "Capture Recording",
    "Tips",
    "Show Tips",
    "Show Toolbar",
    "Show Project Notes",
    "Show Media Browser",
    "Show Piano Roll",
    "Show Mixer",
    "Show Inspector",
    "Show List Editors",
    "Check for Updates…",
    "Check for Updates Automatically",
    "Software Monitoring",
    "Low Latency Mode",
    "Fit to Grid on Drop",
    "Record / Mix Mode",
    "Switch to Record Mode When Arming",
    "Show Tempo Track",
    "Show Marker Track",
    "Show Signature Track",
    "MIDI File…",
    "Paint Image to Audio…",
    "MIDI FX…",
    "Selection as MIDI File…",
    "Create Marker",
    "Mixer",
    "Open Piano Roll",
    "Bigger Interface",
    "Smaller Interface",
    "Actual Interface Size",
];

/// Entries that work only in some states (see [`enabled`]).
const CONTEXTUAL: &[&str] = &[
    "Show Automation",
    "Delete Automation",
    "Undo",
    "Redo",
    "Delete Undo History",
    "Delete",
    "Deselect All",
    "Cut",
    "Copy",
    "Paste",
    "Select All",
    "Split at Playhead",
    "Join",
    "Rename Track",
    "Assign Track Color…",
    "Assign Track Icon…",
    "Configure Track Header…",
    "Show Master Track",
    "Show",
    "Mastering Glue - Small Room",
    "Mastering Plate - Vocal Sheen",
    "Mastering Wide Hall",
    "Delete Track",
    "Release Track Stack",
    "Go to Next Marker",
    "Go to Previous Marker",
    "Delete Marker",
    "New Pattern Region",
    "New Drummer Region",
    "Go to Next Region",
    "Go to Previous Region",
];

/// Whether `label` does anything at all. Entries that are not built yet are not shown, so
/// the menus promise only what the program does.
pub(crate) fn implemented(label: &str) -> bool {
    if label == "-" {
        return false;
    }
    if let Some(children) = submenu(label) {
        return children.iter().any(|c| implemented(c));
    }
    label == "Open Recent"
        || FileCmd::from_menu(label).is_some()
        || SettingsTab::from_menu(label).is_some()
        || Lang::from_native_name(label).is_some()
        || ALWAYS.contains(&label)
        || CONTEXTUAL.contains(&label)
        || buffer_frames(label).is_some()
        || crate::engine_mode::process_frames(label).is_some()
}

/// `items` without what does nothing, and without separators left at an end or in a row.
pub(crate) fn shown(items: &[&'static str]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for &it in items {
        if it == "-" {
            if out.last().is_some_and(|l| *l != "-") {
                out.push(it);
            }
        } else if implemented(it) {
            out.push(it);
        }
    }
    if out.last() == Some(&"-") {
        out.pop();
    }
    out
}

/// Start of every region, in beats, sorted (for Go to Next / Previous Region).
fn region_starts(p: &Project) -> Vec<f64> {
    let mut starts: Vec<f64> = p
        .tracks
        .iter()
        .flat_map(|t| t.regions.iter())
        .map(|r| p.region_start_beats(r))
        .collect();
    starts.sort_by(f64::total_cmp);
    starts
}

/// Whether `label` shows a check mark (what it shows is visible).
pub(crate) fn checked(label: &str, p: &Project, s: &crate::UiState) -> bool {
    match label {
        "Show Piano Roll" => s.bottom == Bottom::PianoRoll,
        "Open Piano Roll" => s.bottom == Bottom::PianoRoll,
        "Show Mixer" => s.show_mixer && s.bottom == Bottom::Mixer,
        "Show Inspector" => s.show_inspector,
        "Show List Editors" => s.show_lists,
        "Show Tips" => s.show_tips,
        "Show Toolbar" => s.show_toolbar,
        "Show Project Notes" => s.show_notes,
        "Show Media Browser" => s.browser.open == Some(crate::file_browser::Tab::Files),
        "Check for Updates Automatically" => s.update_auto,
        "Software Monitoring" => s.software_monitoring,
        "Low Latency Mode" => s.low_latency,
        "Fit to Grid on Drop" => s.fit.on_drop,
        "Channel Rack" => s.bottom == Bottom::ChannelRack,
        "Pitch Editor" => s.bottom == Bottom::PitchEditor,
        "MIDI Monitor" => s.show_midi_monitor,
        "Get Free Plug-ins" => s.free_plugins.open,
        _ if buffer_frames(label).is_some() => buffer_frames(label) == Some(s.io_buffer),
        _ if crate::engine_mode::process_frames(label).is_some() => {
            crate::engine_mode::process_frames(label) == Some(s.process_buffer)
        }
        "Switch to Record Mode When Arming" => s.modes.auto_record,
        "Show Tempo Track" => s.tracks.show_tempo,
        "Show Marker Track" => s.tracks.show_markers,
        "Show Signature Track" => s.tracks.show_signatures,
        "Show Master Track" => p.show_master_track,
        "Show Automation" => s.auto.mode,
        _ if Lang::from_native_name(label).is_some() => {
            Lang::from_native_name(label) == Some(i18n::current())
        }
        // Always visible in this app.
        "Show Track Header" | "Show Ruler" | "Show Colors" | "Show Grid" => true,
        _ => false,
    }
}

/// Track numbers shifted, so a remembered region or note selection may point at another track.
fn forget_region_selection(s: &mut crate::UiState) {
    s.tracks.clear_selection();
    s.piano.selected.clear();
}

/// Runs a menu entry (File entries are queued in `UiState::file_cmd` for `App`).
pub(crate) fn apply(label: &str, p: &mut Project, s: &mut crate::UiState) {
    if let Some(lang) = Lang::from_native_name(label) {
        i18n::choose(lang);
        return;
    }
    match label {
        _ if SettingsTab::from_menu(label).is_some() => {
            s.settings_tab = SettingsTab::from_menu(label)
        }
        "Rename Track"
        | "Assign Track Color…"
        | "Assign Track Icon…"
        | "Configure Track Header…" => {
            s.track_dialog = track_dialogs::open_for(label, p);
        }
        "Show Master Track" => p.show_master_track = !p.show_master_track,
        // Automation Mode (A in the Tracks area), as Live's Arrangement View has it.
        "Show Automation" => {
            let on = !s.auto.mode;
            crate::automation_ui::set_mode(p, &mut s.auto, on);
        }
        "Delete Automation" => {
            if let Some(t) = p.tracks.get_mut(p.selected_track) {
                let shown = t.auto.shown;
                t.auto.clear_lane(shown);
            }
        }
        "Select All" => s.tracks.select_all(p),
        "Join" => crate::region_edit::join_selected(p, &mut s.tracks),
        "Show Mixer" | "Mixer" => {
            s.show_mixer = !(s.show_mixer && s.bottom == Bottom::Mixer);
            s.bottom = Bottom::Mixer;
        }
        "Show Inspector" => s.show_inspector = !s.show_inspector,
        "Show List Editors" => s.show_lists = !s.show_lists,
        "Show Tips" | "Tips" => s.show_tips = !s.show_tips,
        "Show Toolbar" => s.show_toolbar = !s.show_toolbar,
        "Show Project Notes" => s.show_notes = !s.show_notes,
        "Show Media Browser" => s.browser.toggle(crate::file_browser::Tab::Files),
        "Capture Recording" => s.capture_cmd = true,
        "Check for Updates…" => s.update_check = true,
        "Check for Updates Automatically" => {
            s.update_auto = !s.update_auto;
            crate::updater::set_auto_enabled(s.update_auto);
        }
        "Fit to Grid on Drop" => {
            s.fit.on_drop = !s.fit.on_drop;
            crate::fit_grid::save_on_drop(s.fit.on_drop);
        }
        "Software Monitoring" => {
            s.software_monitoring = !s.software_monitoring;
            crate::audio_rec::set_software_monitoring(s.software_monitoring);
        }
        "Channel Rack" => {
            s.bottom = if s.bottom == Bottom::ChannelRack {
                Bottom::Mixer
            } else {
                Bottom::ChannelRack
            }
        }
        "Pitch Editor" => {
            if s.bottom == Bottom::PitchEditor {
                s.bottom = Bottom::Mixer;
            } else {
                s.pitch_ed.region = None;
                s.bottom = Bottom::PitchEditor;
            }
        }
        "MIDI Monitor" => s.show_midi_monitor = !s.show_midi_monitor,
        "Get Free Plug-ins" => s.free_plugins.toggle(),
        "Send All Notes Off" => s.midi_panic = true,
        "Low Latency Mode" => {
            s.low_latency = !s.low_latency;
            crate::audio_rec::set_low_latency(s.low_latency);
        }
        _ if buffer_frames(label).is_some() => {
            if let Some(n) = buffer_frames(label) {
                crate::engine_mode::set_io_buffer(s, n);
            }
        }
        _ if crate::engine_mode::process_frames(label).is_some() => {
            if let Some(n) = crate::engine_mode::process_frames(label) {
                crate::engine_mode::set_process_buffer(s, n);
            }
        }
        "Record / Mix Mode" => crate::engine_mode::toggle(s),
        "Switch to Record Mode When Arming" => crate::engine_mode::toggle_auto_record(s),
        "Show Tempo Track" => s.tracks.show_tempo = !s.tracks.show_tempo,
        "Show Marker Track" => s.tracks.show_markers = !s.tracks.show_markers,
        "Show Signature Track" => s.tracks.show_signatures = !s.tracks.show_signatures,
        // Logic: Create Marker puts it at the playhead, rounded to the nearest bar.
        "Create Marker" => {
            let bar = p.beats_to_bar(p.position_beats).round();
            p.add_marker(p.bar_to_beats(bar));
            s.tracks.show_markers = true;
        }
        "Delete Marker" => {
            if let Some(i) = p.marker_at(p.position_beats) {
                p.markers.remove(i);
            }
        }
        "Go to Next Marker" => {
            if let Some(b) = p.next_marker(p.position_beats) {
                p.position_beats = b;
            }
        }
        "Go to Previous Marker" => {
            if let Some(b) = p.previous_marker(p.position_beats) {
                p.position_beats = b;
            }
        }
        // P shows or hides the Piano Roll in the main window, under the tracks. Window > Open
        // Piano Roll (Command-4) opens it there too: the user wants it in the main window, as
        // Logic's editor pane, never in a window of its own.
        "Show Piano Roll" => {
            s.bottom = if s.bottom == Bottom::PianoRoll {
                Bottom::Mixer
            } else {
                s.piano_window = false;
                Bottom::PianoRoll
            }
        }
        "Open Piano Roll" => {
            s.piano_window = false;
            s.bottom = Bottom::PianoRoll;
        }
        "New from Template…" => s.chooser = Some(crate::start_screen::Chooser::templates()),
        // Track > New: a software instrument track, as the + button above the tracks adds.
        "New" | "New Software Instrument Track" => {
            p.selected_track = p.add_track(TrackKind::Instrument);
        }
        "New Audio Track" => {
            p.selected_track = p.add_track(TrackKind::Audio);
        }
        "New VCA Track" => {
            p.add_track(TrackKind::Vca);
        }
        "New External MIDI Track" => {
            p.add_track(TrackKind::ExternalMidi);
        }
        "MIDI FX…" => s.show_midi_fx = !s.show_midi_fx,
        // Mix > Space Designer > Show: the window of the selected track's Space Designer.
        "Show" => {
            let ti = p.selected_track;
            if let Some(t) = p.tracks.get_mut(ti) {
                show_space_designer(&mut t.inserts, s, ti, None);
            }
        }
        // The mastering presets: a Space Designer on Stereo Out, set to the preset.
        "Mastering Glue - Small Room" | "Mastering Plate - Vocal Sheen" | "Mastering Wide Hall" => {
            let preset = crate::space_designer_ui::MASTERING_PRESETS
                .iter()
                .position(|n| *n == label);
            show_space_designer(&mut p.master_inserts, s, crate::mixer::MASTER_STRIP, preset);
        }
        "MIDI File…" => s.midi_cmd = Some(crate::midi_file::MidiCmd::Import),
        "Paint Image to Audio…" => s.paint.pick = true,
        "Selection as MIDI File…" => s.midi_cmd = Some(crate::midi_file::MidiCmd::Export),
        "New Pattern Region" | "New Drummer Region" => {
            // On the selected track, at the bar the playhead is in.
            let bar = p.beats_to_bar(p.position_beats).floor() as f32 + 1.0;
            let track = p.selected_track;
            if let Some(ri) = p.add_generated_region(track, label == "New Drummer Region", bar) {
                s.tracks.select_only(track, ri);
            }
        }
        "New Aux Track" => {
            p.add_track(TrackKind::Aux);
        }
        "Undo" => s.undo_cmd = Some(UndoCmd::Undo),
        "Redo" => s.undo_cmd = Some(UndoCmd::Redo),
        "Delete Undo History" => s.undo_cmd = Some(UndoCmd::Clear),
        "Delete" => crate::tracks_area::delete_selected(p, &mut s.tracks),
        "Copy" | "Cut" | "Paste" => {
            use crate::tracks_area::RegionEdit;
            let what = match label {
                "Copy" => RegionEdit::Copy,
                "Cut" => RegionEdit::Cut,
                _ => RegionEdit::Paste,
            };
            crate::tracks_area::edit_regions(
                p,
                &mut s.tracks,
                &mut s.region_clip,
                &mut s.auto,
                what,
            );
        }
        "Split at Playhead" => crate::region_edit::split_at_playhead(p, &mut s.tracks),
        "Deselect All" => s.tracks.clear_selection(),
        "Go to End" => {
            p.position_beats = p
                .tracks
                .iter()
                .flat_map(|t| t.regions.iter())
                .map(|r| p.region_loop_end_beats(r))
                .fold(0.0, f64::max);
        }
        "Go to Left Locator" => p.position_beats = p.cycle_start,
        "Go to Right Locator" => p.position_beats = p.cycle_end,
        "Go to Next Region" => {
            let at = p.position_beats + 1e-6;
            if let Some(b) = region_starts(p).into_iter().find(|b| *b > at) {
                p.position_beats = b;
            }
        }
        "Go to Previous Region" => {
            let at = p.position_beats - 1e-6;
            if let Some(b) = region_starts(p).into_iter().rev().find(|b| *b < at) {
                p.position_beats = b;
            }
        }
        "Zoom In" => s.tracks.bar_w = (s.tracks.bar_w * 1.25).min(600.0),
        "Zoom Out" => s.tracks.bar_w = (s.tracks.bar_w / 1.25).max(6.0),
        "Zoom to Fit" => {
            let w = s.tracks.lanes_w;
            crate::tracks_area::zoom_to_fit(p, &mut s.tracks, w);
        }
        "Create Track Stack…" => {
            p.create_track_stack(p.selected_track);
            forget_region_selection(s);
        }
        "Bigger Interface" | "Smaller Interface" | "Actual Interface Size" => {
            let dir = match label {
                "Bigger Interface" => 1,
                "Smaller Interface" => -1,
                _ => 0,
            };
            s.ui_scale = crate::ui_scale::step(s.ui_scale, dir);
            crate::ui_scale::save(s.ui_scale);
        }
        "Delete Track" => {
            if p.delete_track(p.selected_track) {
                forget_region_selection(s);
            }
        }
        "Release Track Stack" => {
            if p.release_track_stack(p.selected_track) {
                forget_region_selection(s);
            }
        }
        "Go to Beginning" => p.position_beats = 0.0,
        "Record" => p.recording = !p.recording,
        _ => {
            if let Some(cmd) = FileCmd::from_menu(label) {
                s.file_cmd = Some(cmd);
            }
        }
    }
}

/// Height of a menu entry and size of its text, in points.
const ROW_H: f32 = 24.0;
const FONT: f32 = 12.0;
/// Size of the key commands' text.
const KEY_FONT: f32 = 11.0;
/// Left column (LED) and right padding of an entry.
const CHECK_W: f32 = 24.0;
const RIGHT_PAD: f32 = 10.0;
/// Gap between an entry's title and its key command or arrow.
const KEY_GAP: f32 = 28.0;
/// Height of the in-window menu bar, its left inset and the menu titles' size.
pub const BAR_H: f32 = 30.0;
const BAR_INSET: f32 = 16.0;
const TITLE_SIZE: f32 = 13.5;

/// Whether key commands are printed with the Mac's signs.
fn mac(ctx: &egui::Context) -> bool {
    ctx.os() == egui::os::OperatingSystem::Mac
}

/// Width of a menu holding `labels` (and their key commands), so it is as wide as its longest
/// entry, like a macOS menu, instead of stretching across the window.
fn menu_width(ui: &egui::Ui, labels: &[String]) -> f32 {
    let g = ui.painter();
    let width = |t: &str, face: Face, size: f32| hw::galley(g, t, face, size, theme::INK).size().x;
    let mac = mac(ui.ctx());
    let (mut title, mut key) = (0.0f32, 0.0f32);
    for it in labels.iter().filter(|l| *l != "-") {
        title = title.max(width(&tr(it).to_uppercase(), Face::Bold, FONT));
        if let Some(k) = shortcut(it) {
            key = key.max(hw_transport::shortcut_width(g, &k, mac, KEY_FONT));
        }
        if submenu(it).is_some() || it == "Open Recent" {
            key = key.max(8.0);
        }
    }
    let gap = if key > 0.0 { KEY_GAP } else { 12.0 };
    (CHECK_W + title + gap + key + RIGHT_PAD).max(140.0)
}

/// One entry: the LED column (an LED on every entry that can be checked, lit when it is),
/// the title, then the key command or a submenu arrow on the right. The entry under the
/// pointer is pressed into the panel: a recessed bar with an orange edge.
fn row(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    check: Option<bool>,
    key: Option<KeyboardShortcut>,
    arrow: bool,
    look: &Look,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
    let hot = enabled && (resp.hovered() || resp.has_focus());
    let g = ui.painter();
    if hot {
        let bar = rect.shrink2(vec2(0.0, 1.0));
        g.rect_filled(bar, 2.0, look.hover);
        hw::inner_shadow(g, bar, 2.0, look.hover_shade);
        g.rect_filled(
            egui::Rect::from_min_size(bar.min, vec2(3.0, bar.height())),
            egui::CornerRadius {
                nw: 2,
                sw: 2,
                ne: 0,
                se: 0,
            },
            theme::ORANGE,
        );
    }
    let color = match (enabled, hot) {
        (false, _) => look.dim,
        (true, true) => look.hover_text,
        (true, false) => look.text,
    };
    if let Some(on) = check {
        hw::led_sized(g, pos2(rect.left() + 12.0, rect.center().y), 2.6, on);
    }
    hw::text(
        g,
        pos2(rect.left() + CHECK_W, rect.center().y),
        Align2::LEFT_CENTER,
        &label.to_uppercase(),
        Face::Bold,
        FONT,
        color,
    );
    let right = pos2(rect.right() - RIGHT_PAD, rect.center().y);
    if arrow {
        let c = right + vec2(-3.0, 0.0);
        g.add(egui::Shape::convex_polygon(
            vec![
                c + vec2(-3.0, -4.0),
                c + vec2(3.0, 0.0),
                c + vec2(-3.0, 4.0),
            ],
            color,
            Stroke::NONE,
        ));
    } else if let Some(key) = key {
        let key_color = if enabled { look.key } else { look.dim };
        hw_transport::shortcut(g, right, &key, mac(ui.ctx()), KEY_FONT, key_color);
    }
    resp
}

fn separator(ui: &mut egui::Ui, look: &Look) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    let y = rect.center().y.round();
    let x = rect.left() + 10.0..=rect.right() - 10.0;
    ui.painter()
        .hline(x.clone(), y + 0.5, Stroke::new(1.0, look.separator));
    ui.painter()
        .hline(x, y + 1.5, Stroke::new(1.0, look.separator_hi));
}

/// How a remembered project is listed: "Night Drive  (Music)", so equal names in different
/// folders can be told apart.
fn recent_label(path: &std::path::Path) -> String {
    let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("?");
    match path
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|d| d.to_str())
    {
        Some(folder) => format!("{name}  ({folder})"),
        None => name.to_string(),
    }
}

/// The Open Recent submenu: the remembered projects, then Clear Menu.
fn recent_items(ui: &mut egui::Ui, s: &mut crate::UiState, look: &Look) {
    let labels: Vec<String> = s.recent.paths().iter().map(|p| recent_label(p)).collect();
    let mut sized = labels.clone();
    sized.push(tr("Clear Menu").into());
    ui.set_width(menu_width(ui, &sized));
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut picked = None;
    for (label, path) in labels.iter().zip(s.recent.paths()) {
        if row(ui, label, true, None, None, false, look).clicked() {
            picked = Some(path.clone());
        }
    }
    if !labels.is_empty() {
        separator(ui, look);
    }
    let clear = row(
        ui,
        tr("Clear Menu"),
        !labels.is_empty(),
        None,
        None,
        false,
        look,
    );
    if let Some(path) = picked {
        s.recent_pick = Some(path);
        s.file_cmd = Some(FileCmd::OpenRecent);
        ui.close();
    } else if clear.clicked() {
        s.recent.clear();
        ui.close();
    }
}

/// `items` as drawn: a group left with a single entry joins the group after it, so a menu
/// does not end up with one-line groups between separators.
fn grouped(items: &[&'static str]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    let mut run = 0;
    for &it in items {
        if it == "-" {
            if run > 1 {
                out.push(it);
                run = 0;
            }
        } else {
            out.push(it);
            run += 1;
        }
    }
    out
}

/// The entries of one menu or submenu.
fn entries(ui: &mut egui::Ui, items: &[&'static str], p: &mut Project, s: &mut crate::UiState) {
    let items = &grouped(&shown(items))[..];
    let look = look(s.light_menus);
    let labels: Vec<String> = items.iter().map(|it| it.to_string()).collect();
    ui.set_width(menu_width(ui, &labels));
    ui.spacing_mut().item_spacing.y = 0.0;
    for &it in items {
        if it == "-" {
            separator(ui, look);
            continue;
        }
        if it == "Open Recent" {
            let resp = row(ui, tr(it), true, None, None, true, look);
            egui::containers::menu::SubMenu::new().show(ui, &resp, |ui| recent_items(ui, s, look));
            continue;
        }
        if let Some(children) = submenu(it) {
            let on = children.iter().any(|c| enabled(c, p, s));
            let resp = row(ui, tr(it), on, None, None, true, look);
            if on {
                egui::containers::menu::SubMenu::new()
                    .show(ui, &resp, |ui| entries(ui, children, p, s));
            }
            continue;
        }
        let on = enabled(it, p, s);
        let check = is_checkable(it).then(|| checked(it, p, s));
        if row(ui, tr(it), on, check, shortcut(it), false, look).clicked() {
            apply(it, p, s);
            ui.close();
        }
    }
}

/// Key under which a picture can ask for a menu to be drawn open.
fn force_open_id() -> egui::Id {
    egui::Id::unique("prowave_menu_bar_force_open")
}

/// One menu title on the bar: silkscreen caps that press in like a key while its menu is
/// open (a black key with bone print), its drop-down under its left edge. Hovering another
/// title while a menu is open moves to that menu, as in a system menu bar. Returns the
/// title's right edge.
#[allow(clippy::too_many_arguments)]
fn menu(
    ui: &mut egui::Ui,
    g: &egui::Painter,
    left: f32,
    bar: egui::Rect,
    title: &'static str,
    items: &[&'static str],
    p: &mut Project,
    s: &mut crate::UiState,
) -> f32 {
    use egui::containers::menu::MenuConfig;
    use egui::{Popup, SetOpenCommand, UiKind, UiStackInfo};
    let light = s.light_menus;
    let text = tr(title).to_uppercase();
    let tw = hw::silk_width(g, &text, TITLE_SIZE);
    let r = egui::Rect::from_min_size(pos2(left, bar.center().y - 11.0), vec2(tw + 20.0, 22.0));
    let resp = ui.interact(
        r,
        egui::Id::unique(("menu_bar_title", title)),
        Sense::click(),
    );
    let open_menu_id = egui::Id::unique("prowave_menu_bar_open");
    let mut menu = Popup::menu(&resp);
    let popup_id = menu.get_id();
    let forced = ui.data(|d| d.get_temp::<&'static str>(force_open_id())) == Some(title);
    let other_open = ui
        .data(|d| d.get_temp::<egui::Id>(open_menu_id))
        .is_some_and(|id| id != popup_id && Popup::is_id_open(ui.ctx(), id));
    if forced || (other_open && resp.hovered() && !resp.clicked()) {
        menu = menu.open_memory(Some(SetOpenCommand::Bool(true)));
    }
    let open = forced || Popup::is_id_open(ui.ctx(), popup_id) || resp.clicked();
    let ink = if open {
        let face = hw::paint_key(g, r, hw::Cap::Dark, true, false);
        hw::silk(
            g,
            face.center(),
            Align2::CENTER_CENTER,
            &text,
            TITLE_SIZE,
            theme::ON_DARK,
        );
        None
    } else {
        if resp.hovered() {
            hw::paint_key(g, r, hw::Cap::Bone, false, true);
        }
        Some(theme::INK)
    };
    if let Some(ink) = ink {
        hw::silk(
            g,
            r.center() + vec2(0.0, 0.5),
            Align2::CENTER_CENTER,
            &text,
            TITLE_SIZE,
            ink,
        );
    }
    let config = MenuConfig::new().style(menu_style(light));
    let shown = menu
        .close_behavior(config.close_behavior)
        .style(menu_style(light))
        .info(UiStackInfo::new(UiKind::Menu).with_tag_value(MenuConfig::MENU_CONFIG_TAG, config))
        .show(|ui| entries(ui, items, p, s));
    if shown.is_some() {
        ui.data_mut(|d| d.insert_temp(open_menu_id, popup_id));
    }
    r.right()
}

/// Style of the drop-down menus for the current look.
fn menu_style(light: bool) -> impl Fn(&mut egui::Style) + Send + Sync + 'static {
    move |style: &mut egui::Style| {
        egui::containers::menu::menu_style(style);
        let l = look(light);
        let v = &mut style.visuals;
        v.window_fill = l.menu;
        v.window_stroke = Stroke::new(
            1.0,
            if light {
                theme::SEAM_DK
            } else {
                theme::CAP_EDGE
            },
        );
        v.menu_corner_radius = egui::CornerRadius::same(3);
        v.override_text_color = None;
        v.widgets.noninteractive.fg_stroke.color = l.text;
        v.widgets.inactive.fg_stroke.color = l.text;
        v.widgets.hovered.fg_stroke.color = l.text;
        v.widgets.open.fg_stroke.color = l.text;
        v.widgets.active.fg_stroke.color = l.text;
    }
}

/// Key commands of the View entries above (I, X), unless a text field has focus.
pub fn handle_keys(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    // While the Piano Roll's Tool menu is open, a plain letter picks a tool (P Pencil,
    // I Scissors), so the View keys wait.
    let view_keys: &[&str] = if s.piano.tool_menu_open() {
        &[]
    } else {
        &[
            "Show Inspector",
            "Show Mixer",
            "Show Piano Roll",
            "Open Piano Roll",
            "Record / Mix Mode",
        ]
    };
    for &label in view_keys {
        if let Some(k) = shortcut(label) {
            if ctx.input_mut(|i| i.consume_shortcut(&k)) {
                apply(label, p, s);
            }
        }
    }
    // Redo first: matching Cmd+Z ignores an extra Shift. Ctrl+Y is Redo on Windows and Linux.
    let redo = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    let redo_y = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
    let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
    if ctx.input_mut(|i| i.consume_shortcut(&redo))
        || (!cfg!(target_os = "macos") && ctx.input_mut(|i| i.consume_shortcut(&redo_y)))
    {
        s.undo_cmd = Some(UndoCmd::Redo);
    } else if ctx.input_mut(|i| i.consume_shortcut(&undo)) {
        s.undo_cmd = Some(UndoCmd::Undo);
    }
    // Cmd/Ctrl + = also comes as Plus on keyboards where = needs no Shift.
    let bigger = KeyboardShortcut::new(Modifiers::COMMAND, Key::Plus);
    if ctx.input_mut(|i| i.consume_shortcut(&bigger)) {
        apply("Bigger Interface", p, s);
    }
    for label in [
        "Bigger Interface",
        "Smaller Interface",
        "Actual Interface Size",
    ] {
        if let Some(k) = shortcut(label) {
            if ctx.input_mut(|i| i.consume_shortcut(&k)) {
                apply(label, p, s);
            }
        }
    }
}

/// The app's name split into the wordmark and the edition ("Pro Wave", "Private").
fn name_parts() -> (&'static str, Option<&'static str>) {
    match crate::APP_NAME.split_once(" (") {
        Some((name, rest)) => (name, Some(rest.trim_end_matches(')'))),
        None => (crate::APP_NAME, None),
    }
}

/// The menu bar: a strip of the chassis with a seam under it, the PW-1 wordmark, the menu
/// titles, the last error or warning, and at the right the project's track count and the
/// edition in small silkscreen. Returns where its contents end, so the rest of the row can be
/// used (on Linux it holds the window buttons and moves the window, see `window_chrome`).
pub fn menu_bar(ui: &mut egui::Ui, p: &mut Project, s: &mut crate::UiState) -> f32 {
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
    // The whole panel, under its frame's margins too.
    let panel = egui::Rect::from_x_y_ranges(
        ui.clip_rect().x_range(),
        strip.top() - 2.0..=strip.bottom() + 2.0,
    );
    let bar = egui::Rect::from_x_y_ranges(panel.x_range(), strip.y_range());
    let g = ui.painter_at(panel);
    hw::chassis(&g, panel);
    hw::seam_h(&g, panel.x_range(), panel.bottom() - 2.0);
    let (name, edition) = name_parts();
    let mut x = hw_transport::wordmark(
        &g,
        bar.left() + BAR_INSET,
        bar.center().y,
        name,
        "PW-1 Digital Audio Workstation",
    ) + 22.0;
    for (title, items) in MENUS {
        let items = shown(items);
        if !items.is_empty() {
            x = menu(ui, &g, x, bar, title, &items, p, s) + 2.0;
        }
    }
    // The right end: tracks and edition, clear of the window buttons where the app draws them.
    let right = bar.right()
        - if crate::window_chrome::CUSTOM {
            3.0 * 40.0 + 14.0
        } else {
            BAR_INSET
        };
    let mut info = format!("{:02} TRK", p.tracks.len());
    if let Some(e) = edition {
        info.push_str(" / ");
        info.push_str(e);
    }
    let info_r = hw::text(
        &g,
        pos2(right, bar.center().y + 0.5),
        Align2::RIGHT_CENTER,
        &info.to_uppercase(),
        Face::Bold,
        9.5,
        theme::PRINT,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                pos2(x + 10.0, bar.top()),
                pos2((info_r.left() - 10.0).max(x + 10.0), bar.bottom()),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    status_line(&mut child, p, s);
    child.min_rect().right().max(x)
}

/// Colour of the menu bar's text, for things drawn on the bar.
pub fn bar_text(light: bool) -> Color32 {
    look(light).bar_text
}

/// Whether [`status_line`] has anything to say.
pub fn has_status(p: &Project, s: &crate::UiState) -> bool {
    s.status.is_some() || p.tracks.len() > prowave_engine::MAX_TRACKS
}

/// The last error or warning, after the menus (or on its own line on macOS, where the menus
/// are in the system menu bar).
pub fn status_line(ui: &mut egui::Ui, p: &Project, s: &crate::UiState) {
    // The bar is bone in both looks: a dark orange print reads on it.
    let warn = theme::ORANGE_DK;
    if let Some(msg) = &s.status {
        ui.colored_label(warn, msg);
    }
    // The engine plays only its first MAX_TRACKS tracks; say so instead of failing silently.
    let max = prowave_engine::MAX_TRACKS;
    if p.tracks.len() > max {
        let text = trf(
            "Tracks {}–{} are silent (limit {})",
            &[&(max + 1), &p.tracks.len(), &max],
        );
        ui.colored_label(warn, text).on_hover_text(tr(
            "The audio engine plays up to 1024 tracks. Tracks after that are kept in the project but not played.",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_commands::content;

    /// Help > Check for Updates… and Check for Updates Automatically.
    fn updates(s: &crate::UiState) -> (bool, bool) {
        (s.update_check, s.update_auto)
    }

    /// Everything a menu entry can change, to compare before and after.
    fn state(p: &Project, s: &crate::UiState) -> String {
        format!(
            "{:?}",
            (
                content(p) == content(&Project::demo()),
                p.tracks.len(),
                p.position_beats,
                p.recording,
                (s.show_mixer, s.show_inspector, s.show_lists),
                (
                    updates(s),
                    s.show_midi_fx,
                    (s.software_monitoring, s.fit.on_drop),
                    s.low_latency,
                    (s.io_buffer, s.process_buffer, s.modes.clone()),
                    s.bottom == Bottom::ChannelRack,
                    s.midi_panic,
                    (s.show_midi_monitor, s.free_plugins.open),
                    s.settings_tab
                ),
                (
                    s.capture_cmd,
                    s.show_tips,
                    s.show_toolbar,
                    s.show_notes,
                    s.browser.open
                ),
                (s.bottom == Bottom::Mixer, s.piano_window),
                (
                    s.file_cmd,
                    s.chooser.is_some(),
                    s.undo_cmd,
                    s.midi_cmd,
                    s.paint.pick
                ),
                (
                    s.ui_scale,
                    s.tracks.bar_w,
                    s.tracks.selected,
                    s.region_clip.len(),
                    s.tracks.extra.len(),
                    p.show_master_track,
                    p.header_hidden.len(),
                    s.track_dialog.is_some(),
                    p.master_inserts.len(),
                    s.mixer.fx_window
                ),
                (
                    s.tracks.show_tempo,
                    s.tracks.show_markers,
                    s.tracks.show_signatures,
                    s.auto.mode
                ),
                (i18n::chosen(), i18n::current(),),
            )
        )
    }

    #[test]
    fn view_menu_checks_follow_the_panels() {
        let mut s = crate::UiState::new();
        let mut p = Project::demo();
        assert!(checked("Show Inspector", &p, &s) && checked("Show Mixer", &p, &s));
        apply("Show Inspector", &mut p, &mut s);
        assert!(!s.show_inspector && !checked("Show Inspector", &p, &s));
        apply("Show Piano Roll", &mut p, &mut s);
        assert!(checked("Show Piano Roll", &p, &s) && !checked("Show Mixer", &p, &s));
        apply("Show Mixer", &mut p, &mut s);
        assert!(checked("Show Mixer", &p, &s));
        // Window > Channel Rack takes the same pane, in place of the Piano Roll; again gives
        // the mixer back.
        apply("Channel Rack", &mut p, &mut s);
        assert!(checked("Channel Rack", &p, &s) && !checked("Show Piano Roll", &p, &s));
        apply("Channel Rack", &mut p, &mut s);
        assert!(!checked("Channel Rack", &p, &s) && s.bottom == Bottom::Mixer);
        // Window > Open Piano Roll opens the pane in the main window, never a window of its
        // own, and leaves it open when it already is.
        apply("Open Piano Roll", &mut p, &mut s);
        assert!(!s.piano_window && checked("Show Piano Roll", &p, &s));
        apply("Open Piano Roll", &mut p, &mut s);
        assert!(!s.piano_window && checked("Show Piano Roll", &p, &s));
        assert_eq!(
            shortcut("Show Piano Roll"),
            Some(KeyboardShortcut::new(Modifiers::NONE, Key::P))
        );
        assert_eq!(
            shortcut("Open Piano Roll"),
            Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Num4))
        );
    }

    #[test]
    fn file_entries_queue_their_command() {
        let mut s = crate::UiState::new();
        apply("Save As…", &mut Project::demo(), &mut s);
        assert_eq!(s.file_cmd, Some(FileCmd::SaveAs));
        assert!(shortcut("Show Inspector").is_some() && shortcut("Bounce").is_none());

        for (label, cmd) in [
            ("Save A Copy As…", FileCmd::SaveCopy),
            ("Revert to Saved", FileCmd::Revert),
        ] {
            assert!(all_entries().contains(&label), "{label} is in the menus");
            apply(label, &mut Project::demo(), &mut s);
            assert_eq!(s.file_cmd.take(), Some(cmd));
        }
    }

    #[test]
    fn every_enabled_entry_does_something() {
        // An entry that is enabled must be handled by `apply` (or open a file command):
        // a click must never silently do nothing.
        let (p, mut s) = (Project::demo(), crate::UiState::new());
        s.can_undo = true;
        s.can_redo = true;
        s.tracks.select_only(0, 0);
        for label in all_entries() {
            if !enabled(label, &p, &s) {
                continue;
            }
            let (mut p2, mut s2) = (Project::demo(), crate::UiState::new());
            s2.tracks.select_only(0, 0);
            let before = state(&p2, &s2);
            apply(label, &mut p2, &mut s2);
            // These two may already be where they would take you.
            // These may already be where they would take you.
            let may_stay = [
                "Go to Next Region",
                "Actual Interface Size",
                "English (Default)",
                // The buffer sizes it starts with.
                "64 Samples",
                "Small",
            ]
            .contains(&label);
            assert!(
                before != state(&p2, &s2) || may_stay,
                "{label} is enabled but does nothing"
            );
        }
        // And unimplemented ones are greyed, like the ones from the GitHub report.
        for label in ["Close", "Export", "Stem Splitter…"] {
            assert!(!enabled(label, &p, &s), "{label}");
        }
    }

    #[test]
    fn track_new_adds_an_instrument_track() {
        let (mut p, mut s) = (Project::demo(), crate::UiState::new());
        let n = p.tracks.len();
        apply("New", &mut p, &mut s);
        assert_eq!(p.tracks.len(), n + 1);
        assert_eq!(p.tracks[n].kind, TrackKind::Instrument);
    }

    #[test]
    fn undo_entries_follow_the_history_and_queue_their_command() {
        let (p, mut s) = (Project::demo(), crate::UiState::new());
        assert!(!enabled("Undo", &p, &s) && !enabled("Redo", &p, &s));
        s.can_undo = true;
        assert!(enabled("Undo", &p, &s) && enabled("Delete Undo History", &p, &s));
        apply("Undo", &mut Project::demo(), &mut s);
        assert_eq!(s.undo_cmd, Some(UndoCmd::Undo));
    }

    #[test]
    fn region_navigation_jumps_between_region_starts() {
        let (mut p, mut s) = (Project::demo(), crate::UiState::new());
        p.position_beats = 0.0;
        apply("Go to End", &mut p, &mut s);
        assert_eq!(
            p.position_beats,
            12.0 * 4.0,
            "the demo's regions end at bar 13"
        );
        apply("Go to Previous Region", &mut p, &mut s);
        assert_eq!(p.position_beats, 0.0);
    }

    #[test]
    fn space_designer_entries_open_its_window_and_the_mastering_presets_set_it() {
        use prowave_engine::{sd, FxKind};
        let (mut p, mut s) = (Project::demo(), crate::UiState::new());
        p.selected_track = 1;
        assert!(enabled("Show", &p, &s));
        let before_inserts = p.tracks[1].inserts.len();
        apply("Show", &mut p, &mut s);
        let slot = p.tracks[1]
            .inserts
            .iter()
            .position(|i| i.kind == FxKind::Reverb);
        assert!(
            slot.is_some(),
            "Show adds a Space Designer when there is none"
        );
        assert_eq!(s.mixer.fx_window, slot.map(|k| (1, k)));
        // A mastering preset goes on Stereo Out and sets the whole effect: a quiet wet level,
        // the dry sound at 0 dB.
        for (i, label) in crate::space_designer_ui::MASTERING_PRESETS
            .iter()
            .enumerate()
        {
            assert!(enabled(label, &p, &s));
            apply(label, &mut p, &mut s);
            assert_eq!(p.master_inserts.len(), 1, "one Space Designer, reused");
            assert_eq!(s.mixer.fx_window, Some((crate::mixer::MASTER_STRIP, 0)));
            let params = p.master_inserts[0].params;
            assert_eq!(params, crate::space_designer_ui::mastering_preset(i));
            assert!(
                params[sd::WET] <= -18.0 && params[sd::DRY] == 0.0,
                "{label}"
            );
        }
        assert_eq!(
            p.tracks[1].inserts.len(),
            before_inserts + 1,
            "tracks are left alone"
        );
    }

    #[test]
    fn new_aux_track_adds_an_aux_track() {
        let (mut p, mut s) = (Project::demo(), crate::UiState::new());
        assert!(TRACK.contains(&"New Aux Track"));
        let n = p.tracks.len();
        apply("New Aux Track", &mut p, &mut s);
        assert_eq!(p.tracks.len(), n + 1);
        assert_eq!(p.tracks[n].kind, TrackKind::Aux);
        assert_eq!(p.selected_track, n);
    }

    #[test]
    fn track_stack_entries_group_and_release_the_selected_track() {
        let (mut p, mut s) = (Project::demo(), crate::UiState::new());
        assert!(TRACK.contains(&"Create Track Stack…") && TRACK.contains(&"Release Track Stack"));
        p.selected_track = 2;
        s.tracks.select_only(2, 0);
        let n = p.tracks.len();
        apply("Create Track Stack…", &mut p, &mut s);
        assert_eq!(p.tracks.len(), n + 1);
        assert_eq!(
            (p.tracks[2].kind, p.tracks[3].indent),
            (TrackKind::Folder, 1)
        );
        assert_eq!(
            s.tracks.selected, None,
            "indexes moved, so the selection is dropped"
        );
        apply("Release Track Stack", &mut p, &mut s);
        assert_eq!(p.tracks.len(), n);
        assert!(p
            .tracks
            .iter()
            .all(|t| t.indent == 0 && t.kind != TrackKind::Folder));
    }

    #[test]
    fn recent_projects_are_listed_with_their_folder() {
        let label = recent_label(std::path::Path::new("/home/me/Music/Night Drive.prowave"));
        assert_eq!(label, "Night Drive  (Music)");
        assert_eq!(recent_label(std::path::Path::new("Solo.prowave")), "Solo");
    }

    #[test]
    fn the_file_menu_follows_logic_with_submenus() {
        for title in ["Revert to", "Import", "Export", "Bounce", "Share"] {
            assert!(FILE.contains(&title), "{title}");
            assert!(submenu(title).is_some(), "{title} opens a submenu");
        }
        // Every submenu is reachable from a menu.
        for (title, _) in SUBMENUS {
            assert!(
                MENUS.iter().any(|(_, items)| items.contains(title)),
                "{title}"
            );
        }
        // Revert to > Revert to Saved is enabled, so its submenu opens.
        let (p, s) = (Project::demo(), crate::UiState::new());
        assert!(submenu("Revert to")
            .unwrap()
            .iter()
            .any(|c| enabled(c, &p, &s)));
    }

    #[test]
    fn markers_are_created_at_the_nearest_bar_and_navigated() {
        let (mut p, mut s) = (Project::empty(), crate::UiState::new());
        assert!(!enabled("Go to Next Marker", &p, &s));
        p.position_beats = 5.0;
        apply("Create Marker", &mut p, &mut s);
        assert_eq!(p.markers[0].beat, 4.0, "bar 2 in 4/4");
        assert!(s.tracks.show_markers, "the Marker track opens to show it");
        p.position_beats = 15.0;
        apply("Create Marker", &mut p, &mut s);
        p.position_beats = 0.0;
        apply("Go to Next Marker", &mut p, &mut s);
        apply("Go to Next Marker", &mut p, &mut s);
        assert_eq!(p.position_beats, 16.0);
        apply("Go to Previous Marker", &mut p, &mut s);
        assert_eq!(p.position_beats, 4.0);
        apply("Delete Marker", &mut p, &mut s);
        assert_eq!(p.markers.len(), 1);
        assert_eq!(p.markers[0].beat, 16.0);
    }

    #[test]
    fn help_language_lists_every_language_by_its_own_name() {
        let all: Vec<&str> = Lang::ALL.iter().map(|l| l.native_name()).collect();
        assert_eq!(LANGUAGE, all.as_slice());
        assert!(HELP.contains(&"Language") && submenu("Language").is_some());

        let (mut p, mut s) = (Project::demo(), crate::UiState::new());
        assert!(
            checked("English (Default)", &p, &s),
            "English until another is picked"
        );
        apply("Deutsch", &mut p, &mut s);
        assert_eq!(i18n::current(), Lang::De);
        assert_eq!(i18n::chosen(), Some(Lang::De), "saved for the next start");
        assert!(checked("Deutsch", &p, &s) && !checked("English (Default)", &p, &s));
        assert_eq!(tr("Language"), "Sprache", "the menus switch at once");
        apply("English (Default)", &mut p, &mut s);
        assert!(checked("English (Default)", &p, &s));
    }

    #[test]
    fn p_and_i_belong_to_the_piano_roll_tool_menu_while_it_is_open() {
        let press = |key| egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut p = Project::demo();
        let mut s = crate::UiState::new();
        s.bottom = Bottom::PianoRoll;
        let inspector = s.show_inspector;
        s.piano.open_tool_menu();
        for key in [Key::P, Key::I] {
            let mut out = ctx.run_ui(press(key), |ui| handle_keys(ui.ctx(), &mut p, &mut s));
            out.textures_delta.clear();
        }
        assert!(
            s.bottom == Bottom::PianoRoll,
            "P did not hide the Piano Roll"
        );
        assert_eq!(
            s.show_inspector, inspector,
            "I did not toggle the Inspector"
        );
        // With the menu closed, P is Show Piano Roll again.
        s.piano = Default::default();
        let mut out = ctx.run_ui(press(Key::P), |ui| handle_keys(ui.ctx(), &mut p, &mut s));
        out.textures_delta.clear();
        assert!(s.bottom == Bottom::Mixer);
    }

    /// Pictures of the menu bar and of the View menu in both looks (`PROWAVE_UI_PNG_DIR`, else
    /// the temp folder).
    #[test]
    #[ignore = "pictures"]
    fn pictures_menus() {
        let dir = std::env::var_os("PROWAVE_UI_PNG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-transport"));
        let _ = std::fs::create_dir_all(&dir);
        for light in [false, true] {
            let mut p = Project::demo();
            let mut s = crate::UiState::new();
            s.light_menus = light;
            s.show_inspector = true;
            s.show_mixer = true;
            let name = if light {
                "menu-view-light.png"
            } else {
                "menu-view.png"
            };
            crate::ui_test::render_png(
                1470.0,
                620.0,
                2.0,
                crate::theme::BONE,
                |ui| {
                    crate::theme::apply(ui.ctx());
                    // The View menu drawn open, under its title, as in the app's top panel.
                    ui.data_mut(|d| d.insert_temp(force_open_id(), "View"));
                    let frame =
                        egui::Frame::side_top_panel(ui.style()).fill(bar_fill(s.light_menus));
                    egui::Panel::top("menu")
                        .frame(frame)
                        .show(ui, |ui| menu_bar(ui, &mut p, &mut s));
                },
                &dir.join(name),
            )
            .unwrap();
        }
    }
}
