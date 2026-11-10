//! The Project Settings window (Logic Pro: File > Project Settings): one tab each for General,
//! Synchronization, Metronome, Recording, Tuning, Audio and MIDI.
//!
//! Every control changes something the project or the engine really uses:
//! - General: the project's name.
//! - Synchronization: the SMPTE frame rate and start time, the MIDI Time Code ports, the MIDI
//!   Clock input to follow and the ports MIDI Clock goes to (the same entries as the
//!   right-click menu of the SMPTE time).
//! - Metronome: the click's volume and whether the first beat of a bar clicks higher.
//! - Recording: the bars of count-in before recording.
//! - Tuning: the reference pitch of SoundFont instruments, A4 = 427 to 453 Hz (Audio Units
//!   keep their own tuning).
//! - Audio: which output the engine plays on and its sample rate (read only: the output and
//!   inputs are chosen in an audio track's Input slot, the I/O buffer size in the Record
//!   menu).
//! - MIDI: which MIDI input plays live notes.

use crate::hw::{self, Cap};
use crate::i18n::{tr, trf};
use crate::mixer::hw_mixer::{self as hwm, dialog, row_label};
use crate::model::{
    Project, DEFAULT_CLICK_DB, MAX_COUNT_IN_BARS, MAX_TUNING_HZ, MIN_CLICK_DB, MIN_TUNING_HZ,
};
use crate::project_io::valid_smpte_start;
use crate::sync_menu::{start_label, STARTS};
use crate::theme::{self, Face};
use egui::{pos2, vec2, Align2, Sense};
use prowave_engine::SmpteRate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Synchronization,
    Metronome,
    Recording,
    Tuning,
    Audio,
    Midi,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 7] = [
        Self::General,
        Self::Synchronization,
        Self::Metronome,
        Self::Recording,
        Self::Tuning,
        Self::Audio,
        Self::Midi,
    ];

    /// The tab a menu entry opens ("Metronome…" opens the Metronome tab).
    pub fn from_menu(label: &str) -> Option<Self> {
        Some(match label {
            "General…" => Self::General,
            "Synchronization…" => Self::Synchronization,
            "Metronome…" => Self::Metronome,
            "Recording…" => Self::Recording,
            "Tuning…" => Self::Tuning,
            "Audio…" => Self::Audio,
            "MIDI…" => Self::Midi,
            _ => return None,
        })
    }

    fn title(self) -> String {
        match self {
            Self::General => tr("General"),
            Self::Synchronization => tr("Synchronization"),
            Self::Metronome => tr("Metronome"),
            Self::Recording => tr("Recording"),
            Self::Tuning => tr("Tuning"),
            Self::Audio => tr("Audio"),
            Self::Midi => tr("MIDI"),
        }
        .to_string()
    }
}

/// The longest project name the window accepts.
const MAX_NAME_CHARS: usize = 128;

/// The window's width, the label column's, a row's height and a control's.
const WINDOW_W: f32 = 880.0;
const LABEL_W: f32 = 220.0;
const ROW_H: f32 = 34.0;
const CTRL_H: f32 = 28.0;
/// The tab keys' height (LED on top, legend under it).
const TAB_H: f32 = 46.0;

/// A settings row: the label in silkscreen, right-aligned in its column, then `add`.
fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    row_ink(ui, label, theme::INK2, add)
}

/// [`row`] with the label printed in `ink`.
fn row_ink<R>(
    ui: &mut egui::Ui,
    label: &str,
    ink: egui::Color32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.set_min_height(ROW_H);
        row_label(ui, LABEL_W, CTRL_H, label, ink);
        ui.add_space(6.0);
        add(ui)
    })
    .inner
}

/// A seam across the window between groups of rows.
fn seam(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    hw::seam_h(ui.painter(), r.x_range(), r.center().y - 1.0);
}

/// A bone key `w` wide laid out in `ui`; latched with its LED lit while `on` when `led`.
fn key(ui: &mut egui::Ui, id: &str, label: &str, w: f32, on: bool, led: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(w, CTRL_H), Sense::hover());
    let g = ui.painter().clone();
    hw::key(
        ui,
        &g,
        rect,
        egui::Id::unique(("project_settings_key", id)),
        Cap::Bone,
        on,
        led.then_some(on),
        hw::Legend::Text(label),
    )
    .0
    .clicked()
}

/// An on/off key: its LED at the left, "ON" on the cap, latched while `on`.
fn on_key(ui: &mut egui::Ui, id: &str, on: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(64.0, CTRL_H), Sense::hover());
    let g = ui.painter().clone();
    hw::toggle(
        ui,
        &g,
        rect,
        egui::Id::unique(("project_settings_on", id)),
        on,
        tr("On"),
    )
    .clicked()
}

/// A key's width for `label`.
fn key_w(label: &str) -> f32 {
    (label.chars().count() as f32 * 8.0 + 30.0).max(64.0)
}

/// A dark value field `w` wide (a dropdown when `stepper`).
fn field(ui: &mut egui::Ui, id: &str, value: &str, w: f32, stepper: bool) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let g = ui.painter().clone();
    hw::field(
        ui,
        &g,
        rect,
        egui::Id::unique(("project_settings_field", id)),
        value,
        stepper,
    )
}

/// An LCD readout `w` wide with the value in segments (for the numbers the settings set).
fn readout(ui: &mut egui::Ui, w: f32, label: &str, value: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click_and_drag());
    hw::lcd_readout(ui.painter(), rect, label, value);
    resp
}

/// The tabs: one row of equal keys across the window, the open one a black cap with its LED
/// lit.
fn tabs(ui: &mut egui::Ui, tab: &mut SettingsTab) {
    let n = SettingsTab::ALL.len() as f32;
    let gap = 6.0;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), TAB_H + 4.0), Sense::hover());
    let w = (row.width() - gap * (n - 1.0)) / n;
    let g = ui.painter().clone();
    for (k, t) in SettingsTab::ALL.into_iter().enumerate() {
        let r = egui::Rect::from_min_size(
            pos2(row.left() + k as f32 * (w + gap), row.top()),
            vec2(w, TAB_H),
        );
        let on = *tab == t;
        let cap = if on { Cap::Dark } else { Cap::Bone };
        let (resp, body) = hw::key(
            ui,
            &g,
            r,
            egui::Id::unique(("project_settings_tab", k)),
            cap,
            on,
            Some(on),
            hw::Legend::Custom,
        );
        let title = t.title().to_uppercase();
        let fitted = hwm::fit_steps(
            &g,
            &title,
            Face::Bold,
            &[(11.0, 0.04), (10.0, 0.02), (9.0, 0.0)],
            body.width() - 8.0,
        );
        hwm::text_t(
            &g,
            body.center() + vec2(0.0, -1.0),
            Align2::CENTER_CENTER,
            &fitted.text,
            Face::Bold,
            fitted.size,
            fitted.track,
            cap.ink(),
        );
        if resp.clicked() {
            *tab = t;
        }
    }
}

/// Draws the window when a tab is chosen. A change bumps nothing itself: the settings are read
/// by the engine sync on the next frame.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    let Some(mut tab) = s.settings_tab else {
        return;
    };
    let (_, closed) = dialog(
        ctx,
        egui::Id::unique("project_settings_window"),
        tr("Project Settings"),
        WINDOW_W,
        |ui| {
            tabs(ui, &mut tab);
            seam(ui);
            match tab {
                SettingsTab::General => general(ui, p),
                SettingsTab::Synchronization => sync(ui, p),
                SettingsTab::Metronome => metronome(ui, p),
                SettingsTab::Recording => recording(ui, p),
                SettingsTab::Tuning => tuning(ui, p),
                SettingsTab::Audio => audio(ui, s.audio_info.as_ref()),
                SettingsTab::Midi => {
                    midi_input(ui, p);
                    midi_panic(ui, &mut s.midi_panic);
                }
            }
        },
    );
    s.settings_tab = (!closed).then_some(tab);
}

fn general(ui: &mut egui::Ui, p: &mut Project) {
    row(ui, tr("Project Name"), |ui| {
        let mut name = p.name.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut name)
                    .char_limit(MAX_NAME_CHARS)
                    .font(theme::font(Face::Bold, 12.0))
                    .text_color(theme::ON_DARK)
                    .background_color(theme::CAP)
                    .margin(vec2(10.0, 6.0))
                    .desired_width(360.0),
            )
            .changed()
        {
            p.name = name;
        }
    });
}

/// A port setting: an ON key (lit while a port is set; pressed on, it takes the first port
/// `ports` finds) and a dropdown of Off and the ports. Returns the new setting when changed.
fn port_row(
    ui: &mut egui::Ui,
    label: &str,
    id: &str,
    current: Option<&str>,
    ports: fn() -> Vec<String>,
) -> Option<Option<String>> {
    row(ui, label, |ui| {
        let mut out = None;
        if on_key(ui, id, current.is_some()) {
            out = match current {
                Some(_) => Some(None),
                None => ports().into_iter().next().map(Some),
            };
        }
        let shown = current.map_or_else(|| tr("Off").to_string(), str::to_string);
        let resp = field(ui, id, &shown, 300.0, true);
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(280.0);
            if ui.selectable_label(current.is_none(), tr("Off")).clicked() {
                out = Some(None);
                ui.close();
            }
            let list = ports();
            if list.is_empty() {
                ui.add_enabled(false, egui::Button::new(tr("No MIDI ports found")));
            }
            for name in list {
                if ui
                    .selectable_label(current == Some(name.as_str()), &name)
                    .clicked()
                {
                    out = Some(Some(name));
                    ui.close();
                }
            }
        });
        out
    })
}

/// Synchronization: the SMPTE frame rate and start time, then MIDI Clock and MIDI Time Code in
/// and out (the same settings as the SMPTE time's right-click menu).
fn sync(ui: &mut egui::Ui, p: &mut Project) {
    row(ui, tr("Frame Rate"), |ui| {
        let resp = field(
            ui,
            "frame_rate",
            &crate::sync_menu::rate_label(p.smpte_rate),
            240.0,
            true,
        );
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(220.0);
            for rate in SmpteRate::ALL {
                if ui
                    .selectable_label(p.smpte_rate == rate, crate::sync_menu::rate_label(rate))
                    .clicked()
                {
                    p.smpte_rate = rate;
                    p.smpte_start = valid_smpte_start(p.smpte_start, rate);
                    ui.close();
                }
            }
        });
    });
    row(ui, tr("Start Time"), |ui| {
        let resp = field(
            ui,
            "start_time",
            &start_label(p.smpte_start, p.smpte_rate),
            240.0,
            true,
        );
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(220.0);
            for start in STARTS {
                let start = valid_smpte_start(start, p.smpte_rate);
                let text = start_label(start, p.smpte_rate);
                if ui.selectable_label(p.smpte_start == start, text).clicked() {
                    p.smpte_start = start;
                    ui.close();
                }
            }
        });
    });
    seam(ui);
    let clock_in = p.midi_clock_in.clone();
    if let Some(v) = port_row(
        ui,
        tr("Follow MIDI Clock"),
        "clock_in",
        clock_in.as_deref(),
        crate::clock_in::ports,
    ) {
        p.midi_clock_in = v;
    }
    let mtc_out = p.mtc_out.clone();
    if let Some(v) = port_row(
        ui,
        tr("Send MIDI Time Code"),
        "mtc_out",
        mtc_out.as_deref(),
        crate::midi_out::ports,
    ) {
        // A port that is both followed and fed would chase its own time code.
        if v.is_some() {
            p.mtc_in = None;
        }
        p.mtc_out = v;
    }
    // MIDI Clock goes to any number of ports (kept for this computer).
    row(ui, tr("Send MIDI Clock"), |ui| {
        let on = crate::midi_out::clock_ports();
        if on_key(ui, "clock_out", !on.is_empty()) {
            if on.is_empty() {
                if let Some(first) = crate::midi_out::ports().first() {
                    crate::midi_out::set_clock_port(first, true);
                }
            } else {
                for port in &on {
                    crate::midi_out::set_clock_port(port, false);
                }
            }
        }
        let shown = if on.is_empty() {
            tr("Off").to_string()
        } else {
            on.join(", ")
        };
        let resp = field(ui, "clock_out", &shown, 300.0, true);
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(280.0);
            let ports = crate::midi_out::ports();
            if ports.is_empty() {
                ui.add_enabled(false, egui::Button::new(tr("No MIDI ports found")));
            }
            for name in ports {
                let mut checked = on.contains(&name);
                if ui.checkbox(&mut checked, &name).changed() {
                    crate::midi_out::set_clock_port(&name, checked);
                }
            }
        });
    });
    let mtc_in = p.mtc_in.clone();
    if let Some(v) = port_row(
        ui,
        tr("Follow MIDI Time Code"),
        "mtc_in",
        mtc_in.as_deref(),
        crate::mtc_in::ports,
    ) {
        if v.is_some() {
            p.mtc_out = None;
        }
        p.mtc_in = v;
    }
}

fn metronome(ui: &mut egui::Ui, p: &mut Project) {
    row(ui, tr("Click Volume"), |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(240.0, CTRL_H), Sense::hover());
        let g = ui.painter().clone();
        let span = -MIN_CLICK_DB;
        let frac = (p.click_volume_db - MIN_CLICK_DB) / span;
        if let Some(f) = crate::widgets::slider_h(
            ui,
            &g,
            r.shrink2(vec2(0.0, 8.0)),
            egui::Id::unique("click_volume"),
            frac,
            true,
        ) {
            p.click_volume_db = MIN_CLICK_DB + f * span;
        }
        ui.add_space(4.0);
        readout(ui, 96.0, "dB", &format!("{:.1}", p.click_volume_db));
        let reset = tr("Reset");
        if key(ui, "click_reset", reset, key_w(reset), false, false) {
            p.click_volume_db = DEFAULT_CLICK_DB;
        }
    });
    row(ui, tr("Accent First Beat of Each Bar"), |ui| {
        if on_key(ui, "accent", p.click_accent) {
            p.click_accent = !p.click_accent;
        }
    });
}

fn recording(ui: &mut egui::Ui, p: &mut Project) {
    // The count-in itself is the "1234" key of the control bar.
    row(ui, tr("Count-in"), |ui| {
        if on_key(ui, "count_in", p.count_in) {
            p.count_in = !p.count_in;
        }
    });
    let enabled = p.count_in;
    let ink = if enabled { theme::INK2 } else { theme::PRINT };
    row_ink(ui, tr("Count-in Bars"), ink, |ui| {
        let id = egui::Id::unique("count_in_bars");
        let (r, _) = ui.allocate_exact_size(vec2(36.0, CTRL_H), Sense::hover());
        let g = ui.painter().clone();
        if hwm::step_key(ui, &g, r, id.with("down"), false, enabled) {
            p.count_in_bars = p.count_in_bars.saturating_sub(1).max(1);
        }
        readout(ui, 70.0, "BARS", &p.count_in_bars.to_string());
        let (r, _) = ui.allocate_exact_size(vec2(36.0, CTRL_H), Sense::hover());
        if hwm::step_key(ui, &g, r, id.with("up"), true, enabled) {
            p.count_in_bars = (p.count_in_bars + 1).min(MAX_COUNT_IN_BARS);
        }
    });
}

fn tuning(ui: &mut egui::Ui, p: &mut Project) {
    row(ui, tr("Reference Pitch (A4)"), |ui| {
        // Drag the readout up and down (Shift for tenths).
        let resp = readout(ui, 120.0, "HZ", &format!("{:.1}", p.tuning_hz));
        if resp.dragged() {
            let fine = if ui.input(|i| i.modifiers.shift) {
                0.02
            } else {
                0.1
            };
            p.tuning_hz =
                (p.tuning_hz - resp.drag_delta().y * fine).clamp(MIN_TUNING_HZ, MAX_TUNING_HZ);
        }
        let reset = tr("Reset");
        if key(ui, "tuning_reset", reset, key_w(reset), false, false) {
            p.tuning_hz = 440.0;
        }
    });
}

/// Audio: the engine's output device and sample rate, read only (the inputs are chosen in an
/// audio track's Input slot, the I/O buffer size in the Record menu).
fn audio(ui: &mut egui::Ui, info: Option<&(String, u32)>) {
    let (device, rate) = match info {
        Some((d, r)) => (d.clone(), Some(*r)),
        None => ("—".to_string(), None),
    };
    row(ui, tr("Output Device"), |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(300.0, 26.0), Sense::hover());
        hw::paint_field(ui.painter(), r, &device, false, false);
    });
    row(ui, hwm::head(&trf("Sample Rate: {} Hz", &[&""])), |ui| {
        let value = rate.map_or_else(
            || "--.-".to_string(),
            |r| format!("{:.1}", r as f32 / 1000.0),
        );
        readout(ui, 110.0, "KHZ", &value);
    });
}

fn midi_input(ui: &mut egui::Ui, p: &mut Project) {
    row(ui, tr("MIDI Input"), |ui| {
        let shown = p
            .midi_in_port
            .clone()
            .unwrap_or_else(|| tr("Automatic").to_string());
        let resp = field(ui, "midi_input_port", &shown, 300.0, true);
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(280.0);
            if ui
                .selectable_label(p.midi_in_port.is_none(), tr("Automatic"))
                .clicked()
            {
                p.midi_in_port = None;
                ui.close();
            }
            for name in crate::mtc_in::ports() {
                let on = p.midi_in_port.as_deref() == Some(&name);
                if ui.selectable_label(on, &name).clicked() {
                    p.midi_in_port = Some(name);
                    ui.close();
                }
            }
        });
    });
}

fn midi_panic(ui: &mut egui::Ui, panic: &mut bool) {
    seam(ui);
    // Every note of every MIDI output the tracks use, on every channel (see midi_out.rs).
    row(ui, "", |ui| {
        let label = tr("Send MIDI Panic").to_uppercase();
        if key(ui, "panic", &label, key_w(&label), false, false) {
            *panic = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_settings_entry_of_the_menus_opens_its_tab() {
        let mut seen = Vec::new();
        for label in crate::menus::all_entries() {
            if let Some(tab) = SettingsTab::from_menu(label) {
                seen.push(tab);
            }
        }
        for tab in SettingsTab::ALL {
            assert!(seen.contains(&tab), "{tab:?} has a menu entry");
        }
    }

    /// Runs the window for a few frames over `p` and `s`.
    fn run(p: &mut Project, s: &mut crate::UiState) {
        let mut h = crate::ui_test::Harness::new(900.0, 600.0);
        for _ in 0..3 {
            h.frame(vec![], |ui| show(ui.ctx(), p, s));
        }
    }

    #[test]
    fn every_tab_draws_and_the_window_stays_open() {
        for tab in SettingsTab::ALL {
            let (mut p, mut s) = (Project::demo(), crate::UiState::new());
            s.settings_tab = Some(tab);
            s.audio_info = Some(("Speakers".into(), 48_000));
            run(&mut p, &mut s);
            assert_eq!(s.settings_tab, Some(tab));
        }
    }

    #[test]
    fn nothing_is_drawn_while_no_tab_is_chosen() {
        let (mut p, mut s) = (Project::demo(), crate::UiState::new());
        let before = p.click_volume_db;
        run(&mut p, &mut s);
        assert!(s.settings_tab.is_none() && p.click_volume_db == before);
    }

    /// Where the pictures go: `PROWAVE_SHOT_DIR`, else a folder in the temp directory.
    fn shot_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::var_os("PROWAVE_SHOT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-mixer"));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    /// Every tab of the window.
    #[test]
    #[ignore = "pictures"]
    fn project_settings_pictures() {
        for tab in SettingsTab::ALL {
            let (mut p, mut s) = (Project::demo(), crate::UiState::new());
            s.settings_tab = Some(tab);
            s.audio_info = Some(("Speakers".into(), 48_000));
            let name = format!("settings-{}.png", tab.title().to_lowercase());
            crate::ui_test::render_png(
                960.0,
                500.0,
                2.0,
                crate::theme::BONE3,
                |ui| {
                    crate::theme::apply(ui.ctx());
                    show(ui.ctx(), &mut p, &mut s)
                },
                &shot_path(&name),
            )
            .unwrap();
        }
    }
}
