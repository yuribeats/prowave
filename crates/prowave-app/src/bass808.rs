//! The 808 (Instrument slot > 808): a tuned, monophonic 808 bass, synthesized or played from a
//! one-shot whose fundamental is measured when it is chosen (see the engine's `bass808`).
//!
//! The settings live in the slot (`prowave:808`) as JSON; the engine's instrument is made again
//! when they change, as the Channel Rack's is. A knob being turned changes only the window's
//! copy, and the slot (so the sound) when it is let go.
//!
//! The window is a Hardware unit (`gear::unit_window`): source keys, the root on an LCD, a row
//! of knobs per four settings; the last row holds the two waves, the high-pass filter and the
//! Legato and Sustain keys (Sustain only with a one-shot: the synthesized voice has no end;
//! greyed out when no loop fits in the one-shot, too short or too fast to fall).
use crate::gear::{self, Cap, Control, LedAt};
use crate::i18n::tr;
use crate::model::Project;
use crate::plugins::PluginSlot;
use egui::{pos2, vec2, Align2, Rect, Sense};
use prowave_engine::{
    Bass808Param, Bass808Params, Bass808Spec, BASS808_HIGH_PASS, BASS808_PARAMS, BASS808_SINE,
    BASS808_SQUARE,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The 808 in an Instrument slot.
pub const BASS808_ID: &str = "prowave:808";
/// No folder of one-shots is offered in the Sample menu: Choose… picks a file.
const LIVE_808S: &str = "";

/// What the slot keeps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bass808State {
    /// The one-shot, or empty for the synthesized voice.
    pub sample: String,
    /// Its fundamental, in Hz (measured when chosen; can be set by hand).
    pub root_hz: f32,
    pub params: Bass808Params,
}

pub fn is_808(slot: &PluginSlot) -> bool {
    slot.id == BASS808_ID
}

/// An Instrument slot with the synthesized 808.
pub fn new_slot() -> PluginSlot {
    let mut slot = PluginSlot::new(BASS808_ID, "808");
    slot.state = Some(encode(&Bass808State::default()));
    slot
}

pub fn encode(st: &Bass808State) -> Vec<u8> {
    let p = &st.params;
    json!({
        "sample": st.sample,
        "root_hz": st.root_hz,
        "decay_s": p.decay_s,
        "release_s": p.release_s,
        "glide_ms": p.glide_ms,
        "drive": p.drive,
        "punch_st": p.punch_st,
        "punch_ms": p.punch_ms,
        "tune_st": p.tune_st,
        "level_db": p.level_db,
        "legato": p.legato,
        "sine": p.sine,
        "square": p.square,
        "high_pass_hz": p.high_pass_hz,
        "sustain": p.sustain,
    })
    .to_string()
    .into_bytes()
}

/// The slot's settings; anything missing or unreadable is the default. A slot saved before the
/// waves, the filter and Sustain has none of them, and their defaults are all "off".
pub fn decode(state: &Option<Vec<u8>>) -> Bass808State {
    let v: Value = state
        .as_deref()
        .and_then(|b| serde_json::from_slice(b).ok())
        .unwrap_or(Value::Null);
    let d = Bass808Params::default();
    let num = |k: &str, or: f32| v[k].as_f64().map_or(or, |x| x as f32);
    Bass808State {
        sample: v["sample"].as_str().unwrap_or_default().to_string(),
        root_hz: num("root_hz", 0.0).max(0.0),
        params: Bass808Params {
            decay_s: num("decay_s", d.decay_s),
            release_s: num("release_s", d.release_s),
            glide_ms: num("glide_ms", d.glide_ms),
            drive: num("drive", d.drive),
            punch_st: num("punch_st", d.punch_st),
            punch_ms: num("punch_ms", d.punch_ms),
            tune_st: num("tune_st", d.tune_st),
            level_db: num("level_db", d.level_db),
            legato: v["legato"].as_bool().unwrap_or(d.legato),
            sine: num("sine", d.sine),
            square: num("square", d.square),
            high_pass_hz: num("high_pass_hz", d.high_pass_hz),
            sustain: v["sustain"].as_bool().unwrap_or(d.sustain),
        }
        .clamped(),
    }
}

/// What the engine's 808 is made from.
pub fn spec(slot: &PluginSlot) -> Bass808Spec {
    let st = decode(&slot.state);
    Bass808Spec {
        sample: st.sample,
        root_hz: st.root_hz,
        params: st.params,
    }
}

/// An 808 lane saved with a range its parameter no longer has (Decay was 0.05 to 8 s and
/// Release 0.005 to 4 s before they were made longer): its points are moved to where the same
/// values sit on today's range, so the lane plays, reads and edits as it did. Other lanes are
/// left as they are.
pub fn update_lane(lane: &mut crate::model::PluginLane) {
    if lane.plugin != BASS808_ID {
        return;
    }
    let Some(info) = BASS808_PARAMS.get(lane.param as usize) else {
        return;
    };
    if (lane.lo, lane.hi) == (info.lo, info.hi) || lane.hi <= lane.lo {
        return;
    }
    for p in &mut lane.points {
        let v = lane.lo + p.value * (lane.hi - lane.lo);
        p.value = ((v - info.lo) / (info.hi - info.lo)).clamp(0.0, 1.0);
    }
    (lane.lo, lane.hi, lane.stepped) = (info.lo, info.hi, info.stepped);
}

/// The automation value of parameter `k`.
pub fn value(slot: &PluginSlot, k: u32) -> Option<f32> {
    decode(&slot.state).params.get(k as usize)
}

/// The rate the window reads one-shots at to measure them.
const ANALYSIS_RATE: u32 = 44_100;

/// A one-shot's fundamental, in Hz (UI thread: reads the file).
fn measure(path: &Path) -> Result<f32, String> {
    let mono = prowave_engine::bass808_prepare_sample(path, ANALYSIS_RATE)?;
    prowave_engine::bass808_measure_root_hz(&mono, ANALYSIS_RATE)
        .ok_or_else(|| "no pitch found".into())
}

/// The note nearest `hz`, and how far off it is in cents.
fn nearest_note(hz: f32) -> Option<(u8, f32)> {
    if !(hz.is_finite() && hz > 0.0) {
        return None;
    }
    let m = 69.0 + 12.0 * (hz / 440.0).log2();
    let n = m.round();
    if (0.0..=127.0).contains(&n) {
        Some((n as u8, (m - n) * 100.0))
    } else {
        None
    }
}

/// Ableton Live's 808 one-shots on this Mac, by name.
fn live_808s() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(LIVE_808S)
        .map(|d| {
            d.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension().is_some_and(|e| e == "wav" || e == "aif")
                        && p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("808 "))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Where `v` sits on a knob for `info`, 0 to 1: seconds, milliseconds and Hz turn on a log
/// scale (from 0 ms, as one plus the value), the rest straight.
fn to_norm(info: &Bass808Param, v: f32) -> f32 {
    let v = v.clamp(info.lo, info.hi);
    let n = match info.unit {
        "s" | "ms" | "Hz" if info.lo > 0.0 => (v / info.lo).ln() / (info.hi / info.lo).ln(),
        "s" | "ms" => (1.0 + v - info.lo).ln() / (1.0 + info.hi - info.lo).ln(),
        _ => (v - info.lo) / (info.hi - info.lo),
    };
    if n.is_finite() {
        n.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The value at knob position `n` (the inverse of [`to_norm`]).
fn from_norm(info: &Bass808Param, n: f32) -> f32 {
    let n = n.clamp(0.0, 1.0);
    let v = match info.unit {
        "s" | "ms" | "Hz" if info.lo > 0.0 => info.lo * (info.hi / info.lo).powf(n),
        "s" | "ms" => info.lo + (1.0 + info.hi - info.lo).powf(n) - 1.0,
        _ => info.lo + (info.hi - info.lo) * n,
    };
    v.clamp(info.lo, info.hi)
}

/// `v` as the LCD under its knob shows it.
fn value_text(info: &Bass808Param, v: f32) -> String {
    match info.unit {
        "s" => format!("{v:.2} s"),
        "ms" => format!("{v:.0} ms"),
        "st" => format!("{v:.1} st"),
        // The filter is off at the bottom of its range.
        "Hz" if v <= info.lo => tr("Off").to_string(),
        "Hz" => format!("{v:.0} Hz"),
        "dB" => format!("{v:.1} dB"),
        // Drive and the waves' levels, 0 to 1, as a percentage.
        _ => format!("{:.0} %", v * 100.0),
    }
}

/// The 808 window and its working copy.
#[derive(Default)]
pub struct Bass808Ui {
    /// The slot whose window is open.
    pub open: Option<u64>,
    /// A knob being turned (or the root dragged): the slot and the settings shown meanwhile.
    drag: Option<(u64, Bass808State)>,
    /// Live's 808s, read when the window first opens.
    library: Option<Vec<PathBuf>>,
    /// Whether the slot's one-shot can be held (a sustain loop fits in it), by its path and
    /// root: worked out once for each.
    loopable: Option<(String, u32, bool)>,
}

/// Width of the unit: four knob cells and the side margins.
const WIDTH: f32 = 4.0 * gear::CELL_W + 2.0 * PAD;
const PAD: f32 = 26.0;
/// The knobs' diameter.
const KNOB_D: f32 = 46.0;
/// The settings on knobs, in the order they are laid out, four to a row.
const KNOBS: [usize; 11] = [
    0,
    1,
    2,
    3,
    4,
    5,
    6,
    7,
    BASS808_SINE,
    BASS808_SQUARE,
    BASS808_HIGH_PASS,
];

/// The 808 window: the sound (synthesized or a one-shot and its root) and how it plays.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    let Some(uid) = s.bass808.open else { return };
    let Some(ti) = p.tracks.iter().position(|t| {
        t.au_instrument
            .as_ref()
            .is_some_and(|x| x.uid == uid && is_808(x))
    }) else {
        s.bass808.open = None;
        return;
    };
    let name = p.tracks[ti].name.clone();
    let Some(slot) = p.tracks[ti].au_instrument.as_mut() else {
        return;
    };
    let ui_state = &mut s.bass808;
    let library = ui_state.library.get_or_insert_with(live_808s).clone();
    // From the saved settings, not the ones being dragged: the root moves while it is dragged.
    let saved = spec(slot);
    let loopable = match &ui_state.loopable {
        Some((path, root, ok)) if *path == saved.sample && *root == saved.root_hz.to_bits() => *ok,
        _ => {
            let ok = prowave_engine::bass808_can_sustain(&saved, ANALYSIS_RATE);
            ui_state.loopable = Some((saved.sample.clone(), saved.root_hz.to_bits(), ok));
            ok
        }
    };
    let mut st = match &ui_state.drag {
        Some((u, d)) if *u == uid => d.clone(),
        _ => decode(&slot.state),
    };
    let mut commit = false;
    let mut dragging = false;
    let mut chosen: Option<PathBuf> = None;
    let mut error: Option<String> = None;
    let labels = [
        tr("Decay"),
        tr("Release"),
        tr("Glide"),
        tr("Saturate"),
        tr("Punch"),
        tr("Punch Time"),
        tr("Tune"),
        tr("Level"),
        tr("Sine"),
        tr("Square"),
        tr("High Pass"),
    ];
    let defaults = Bass808Params::default();
    let controls: Vec<Control> = KNOBS
        .iter()
        .zip(&labels)
        .map(|(&k, label)| {
            let info = &BASS808_PARAMS[k];
            let v = st.params.get(k).unwrap_or(info.lo);
            Control {
                label: label.to_string(),
                norm: to_norm(info, v),
                default: to_norm(info, defaults.get(k).unwrap_or(info.lo)),
                text: value_text(info, v),
                choices: Vec::new(),
                choice: 0,
                automated: None,
                enabled: true,
            }
        })
        .collect();
    let plate = gear::Plate {
        model: "808",
        name: &name,
        strip: None,
        reset: true,
    };
    let id = egui::Id::unique(("prowave_808_window", uid));
    let (head, _) = gear::unit_window(ctx, id, WIDTH, &plate, None, |ui| {
        let src_h = 34.0;
        let root_h = if st.sample.is_empty() { 0.0 } else { 44.0 };
        let grid_h = gear::grid_height(controls.len(), 4);
        let total = 14.0 + src_h + root_h + 10.0 + grid_h;
        let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, total), Sense::hover());
        let painter = ui.painter().clone();
        let mut y = rect.top() + 14.0;
        // The source: the synthesized voice, or a one-shot from the menu.
        let synth = st.sample.is_empty();
        let kr = Rect::from_min_size(pos2(rect.left() + PAD, y), vec2(132.0, 28.0));
        if gear::key(
            ui,
            kr,
            id.with("synth"),
            tr("Synthesizer"),
            Cap::Bone,
            LedAt::Left,
            synth,
            synth,
        )
        .clicked()
            && !synth
        {
            st.sample.clear();
            st.root_hz = 0.0;
            commit = true;
        }
        let label = if synth {
            tr("Sample").to_string()
        } else {
            Path::new(&st.sample)
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let fr = Rect::from_min_max(
            pos2(kr.right() + 12.0, y),
            pos2(rect.right() - PAD, y + 28.0),
        );
        let resp = gear::field(ui, fr, id.with("sample"), &label, true);
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(fr.width());
            if ui.button(format!("{}…", tr("Choose"))).clicked() {
                let mut dialog = rfd::FileDialog::new()
                    .add_filter("Audio", &["wav", "aif", "aiff", "flac", "mp3", "caf"]);
                if Path::new(LIVE_808S).is_dir() {
                    dialog = dialog.set_directory(LIVE_808S);
                }
                chosen = dialog.pick_file();
                ui.close();
            }
            if !library.is_empty() {
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for path in &library {
                            let stem = path
                                .file_stem()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let on = Path::new(&st.sample) == path.as_path();
                            if ui.selectable_label(on, stem).clicked() {
                                chosen = Some(path.clone());
                                ui.close();
                            }
                        }
                    });
            }
        });
        y += src_h;
        // The one-shot's root on an LCD: drag up or down to set it (Shift for fine).
        if !synth {
            let glass = Rect::from_min_max(
                pos2(rect.left() + PAD, y + 4.0),
                pos2(rect.right() - PAD, y + root_h - 6.0),
            );
            gear::glass(&painter, glass);
            gear::lcd_label(
                &painter,
                glass.left_top() + vec2(6.0, 5.0),
                Align2::LEFT_TOP,
                &tr("Root").to_uppercase(),
            );
            let note = nearest_note(st.root_hz)
                // Rounded, so a note in tune reads +0, not -0.
                .map(|(n, cents)| {
                    let cents = cents.round() + 0.0;
                    format!("{} {cents:+.0}¢", crate::step_editor::note_name(n))
                })
                .unwrap_or_default();
            gear::lcd_text(
                &painter,
                pos2(glass.right() - 8.0, glass.center().y),
                Align2::RIGHT_CENTER,
                &format!("{:.2} HZ   {note}", st.root_hz),
                14.0,
            );
            let r = ui
                .interact(glass, id.with("root"), Sense::drag())
                .on_hover_cursor(egui::CursorIcon::ResizeVertical);
            if r.dragged() {
                let fine = ui.input(|i| i.modifiers.shift);
                let dy = r.drag_delta().y;
                st.root_hz = (st.root_hz - dy * if fine { 0.01 } else { 0.1 }).clamp(20.0, 400.0);
                dragging = true;
            }
            commit |= r.drag_stopped();
            y += root_h;
        }
        y += 10.0;
        let all: Vec<usize> = (0..controls.len()).collect();
        for ch in gear::control_grid(
            ui,
            pos2(rect.left() + PAD, y),
            4,
            id,
            &controls,
            &all,
            KNOB_D,
        ) {
            let k = KNOBS[ch.index];
            st.params.set(k, from_norm(&BASS808_PARAMS[k], ch.norm));
            if ch.ended {
                commit = true;
            } else {
                dragging = true;
            }
        }
        // The keys, in the last row's fourth cell: on the knobs' centre line, one above the
        // other when there are two.
        let last_row = y + grid_h - gear::CELL_H;
        let cell = Rect::from_min_size(
            pos2(rect.left() + PAD + 3.0 * gear::CELL_W, last_row),
            vec2(gear::CELL_W, gear::CELL_H),
        );
        // As wide as the readouts above them.
        let key_size = vec2(gear::CELL_W - 14.0, 26.0);
        let mut keys = vec![(tr("Legato"), "legato", st.params.legato, true)];
        if !synth {
            keys.push((tr("Sustain"), "sustain", st.params.sustain, loopable));
        }
        let gap = 10.0;
        let stack = keys.len() as f32 * key_size.y + (keys.len() - 1) as f32 * gap;
        // The knobs' centre: under the label, as `gear::control_cell` places it.
        let centre = cell.top() + 30.0 + KNOB_D / 2.0;
        for (n, (label, what, on, enabled)) in keys.into_iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(
                    cell.center().x - key_size.x / 2.0,
                    centre - stack / 2.0 + n as f32 * (key_size.y + gap),
                ),
                key_size,
            );
            // A key that can do nothing is greyed out and does not take clicks; its LED is off.
            let lit = on && enabled;
            let clicked = ui
                .add_enabled_ui(enabled, |ui| {
                    gear::key(
                        ui,
                        r,
                        id.with(what),
                        label,
                        Cap::Bone,
                        LedAt::Left,
                        lit,
                        lit,
                    )
                })
                .inner
                .clicked();
            if clicked {
                if what == "legato" {
                    st.params.legato = !on;
                } else {
                    st.params.sustain = !on;
                }
                commit = true;
            }
        }
    });
    if head.reset {
        st.params = Bass808Params {
            legato: st.params.legato,
            sustain: st.params.sustain,
            ..Bass808Params::default()
        };
        commit = true;
    }
    if let Some(path) = chosen {
        match measure(&path) {
            Ok(hz) => {
                st.sample = path.to_string_lossy().into_owned();
                st.root_hz = hz;
                commit = true;
            }
            Err(e) => error = Some(format!("{}: {e}", path.display())),
        }
    }
    ui_state.drag = (dragging && !commit).then(|| (uid, st.clone()));
    if commit {
        slot.state = Some(encode(&st));
    }
    if let Some(e) = error {
        s.status = Some(e);
    }
    if head.close {
        s.bass808.open = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_the_slot() {
        let st = Bass808State {
            sample: "/s/808 Heavy E.wav".into(),
            root_hz: 41.2,
            params: Bass808Params {
                glide_ms: 120.0,
                legato: false,
                drive: 0.6,
                sine: 0.4,
                square: 0.15,
                high_pass_hz: 85.0,
                sustain: true,
                decay_s: 45.0,
                release_s: 12.0,
                ..Bass808Params::default()
            },
        };
        assert_eq!(decode(&Some(encode(&st))), st);
        assert_eq!(decode(&None), Bass808State::default());
        assert_eq!(decode(&Some(b"not json".to_vec())), Bass808State::default());
        let slot = new_slot();
        assert!(is_808(&slot));
        assert_eq!(spec(&slot).params, Bass808Params::default());
        assert_eq!(value(&slot, 2), Some(Bass808Params::default().glide_ms));
    }

    /// A slot as the app wrote it before the waves, the filter and Sustain: every setting it
    /// has comes back as saved, and the new ones are off.
    #[test]
    fn a_slot_saved_before_the_waves_loads_as_it_was() {
        let old = br#"{"sample":"/s/808 Heavy E.wav","root_hz":41.2,"decay_s":8.0,
            "release_s":4.0,"glide_ms":120.0,"drive":0.6,"punch_st":7.0,"punch_ms":30.0,
            "tune_st":-2.0,"level_db":-3.0,"legato":false}"#;
        let st = decode(&Some(old.to_vec()));
        assert_eq!(st.sample, "/s/808 Heavy E.wav");
        assert_eq!(st.root_hz, 41.2);
        assert_eq!(
            st.params,
            Bass808Params {
                decay_s: 8.0,
                release_s: 4.0,
                glide_ms: 120.0,
                drive: 0.6,
                punch_st: 7.0,
                punch_ms: 30.0,
                tune_st: -2.0,
                level_db: -3.0,
                legato: false,
                sine: 0.0,
                square: 0.0,
                high_pass_hz: 20.0,
                sustain: false,
            }
        );
        assert_eq!(
            value_text(&BASS808_PARAMS[BASS808_HIGH_PASS], st.params.high_pass_hz),
            "Off"
        );
    }

    #[test]
    fn knob_positions_map_back_to_their_values() {
        for (k, info) in BASS808_PARAMS
            .iter()
            .enumerate()
            .filter(|(_, i)| !i.stepped)
        {
            for v in [info.lo, (info.lo + info.hi) / 2.0, info.hi] {
                let back = from_norm(info, to_norm(info, v));
                assert!(
                    (back - v).abs() <= (info.hi - info.lo) * 1e-4,
                    "{k}: {v} -> {back}"
                );
            }
        }
    }

    #[test]
    fn the_window_draws_and_a_turned_knob_reaches_the_slot_when_let_go() {
        let mut p = Project::empty();
        let i = p.add_track(crate::model::TrackKind::Instrument);
        p.tracks[i].name = "808".into();
        let slot = new_slot();
        let uid = slot.uid;
        p.tracks[i].au_instrument = Some(slot);
        let mut s = crate::UiState::new();
        s.bass808.open = Some(uid);
        let mut h = crate::ui_test::Harness::new(700.0, 700.0);
        h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
        assert_eq!(s.bass808.open, Some(uid));
    }

    /// Pictures of the window (for the eye, not a check): written to the temp folder as
    /// `bass808.png` (a one-shot) and `bass808-synth.png` by `cargo test -p prowave-app bass808_picture -- --ignored`.
    #[test]
    #[ignore = "a picture of the window"]
    fn bass808_picture() {
        let mut p = Project::empty();
        let i = p.add_track(crate::model::TrackKind::Instrument);
        p.tracks[i].name = "808 Heavy".into();
        let mut slot = new_slot();
        let dir = std::env::temp_dir().join(format!("prowave-808-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sample = test_one_shot(&dir.join("808 Heavy E.wav"), 41.2, 2.0);
        let st = Bass808State {
            sample: sample.clone(),
            root_hz: 41.2,
            params: Bass808Params {
                glide_ms: 120.0,
                drive: 0.6,
                sine: 0.4,
                high_pass_hz: 85.0,
                sustain: true,
                ..Bass808Params::default()
            },
        };
        slot.state = Some(encode(&st));
        let uid = slot.uid;
        p.tracks[i].au_instrument = Some(slot);
        let mut s = crate::UiState::new();
        s.bass808.open = Some(uid);
        let out = std::env::temp_dir().join("bass808.png");
        crate::ui_test::render_png(
            560.0,
            700.0,
            2.0,
            crate::theme::BONE3(),
            |ui| {
                crate::theme::apply(ui.ctx());
                show(ui.ctx(), &mut p, &mut s)
            },
            &out,
        )
        .unwrap();
        // The synthesized voice: no root, no Sustain key.
        let slot = p.tracks[i].au_instrument.as_mut().unwrap();
        slot.state = Some(encode(&Bass808State::default()));
        crate::ui_test::render_png(
            560.0,
            700.0,
            2.0,
            crate::theme::BONE3(),
            |ui| {
                crate::theme::apply(ui.ctx());
                show(ui.ctx(), &mut p, &mut s)
            },
            &std::env::temp_dir().join("bass808-synth.png"),
        )
        .unwrap();
        // A one-shot too short to hold: the Sustain key greyed out.
        let short = test_one_shot(&dir.join("808 Short E.wav"), 41.2, 0.1);
        let slot = p.tracks[i].au_instrument.as_mut().unwrap();
        slot.state = Some(encode(&Bass808State {
            sample: short,
            ..st.clone()
        }));
        crate::ui_test::render_png(
            560.0,
            700.0,
            2.0,
            crate::theme::BONE3(),
            |ui| {
                crate::theme::apply(ui.ctx());
                show(ui.ctx(), &mut p, &mut s)
            },
            &std::env::temp_dir().join("bass808-no-loop.png"),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A falling `hz` sine `seconds` long written to `path` (a stand-in 808 one-shot).
    fn test_one_shot(path: &Path, hz: f32, seconds: f32) -> String {
        let frames = (44_100.0 * seconds) as usize;
        let stereo: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let t = i as f32 / 44_100.0;
                let y = (std::f32::consts::TAU * hz * t).sin() * (-t * 1.5).exp();
                [y, y]
            })
            .collect();
        prowave_engine::write_wav(path, &stereo, 44_100).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn the_sustain_key_is_greyed_out_when_no_loop_fits() {
        let dir = std::env::temp_dir().join(format!("prowave-808-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let long = test_one_shot(&dir.join("long.wav"), 41.2, 2.0);
        let short = test_one_shot(&dir.join("short.wav"), 41.2, 0.1);
        for (sample, want) in [(long, true), (short, false), ("/no/such.wav".into(), false)] {
            let mut p = Project::empty();
            let i = p.add_track(crate::model::TrackKind::Instrument);
            let mut slot = new_slot();
            slot.state = Some(encode(&Bass808State {
                sample: sample.clone(),
                root_hz: 41.2,
                params: Bass808Params {
                    sustain: true,
                    ..Bass808Params::default()
                },
            }));
            let uid = slot.uid;
            p.tracks[i].au_instrument = Some(slot);
            let mut s = crate::UiState::new();
            s.bass808.open = Some(uid);
            let mut h = crate::ui_test::Harness::new(700.0, 800.0);
            h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
            assert_eq!(s.bass808.loopable, Some((sample, 41.2f32.to_bits(), want)));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_root_is_named_with_its_cents() {
        let (n, c) = nearest_note(41.2).unwrap_or_default();
        assert_eq!(crate::step_editor::note_name(n), "E0");
        assert!(c.abs() < 1.0, "{c}");
        assert!(nearest_note(0.0).is_none());
    }

    /// A project saved when Decay went to 8 s and Release to 4 s: its 808 lanes load on
    /// today's range with the same values, and lanes of other plug-ins are untouched.
    #[test]
    fn an_808_lane_saved_with_the_old_range_plays_the_same_values() {
        use prowave_engine::{BASS808_DECAY, BASS808_RELEASE};
        let old = |param: usize, hi: f32, plugin: &str| crate::model::PluginLane {
            slot: 0,
            param: param as u32,
            plugin: plugin.into(),
            lo: BASS808_PARAMS[param].lo,
            hi,
            stepped: false,
            points: vec![
                prowave_engine::AutoPoint::new(0.0, 0.0),
                prowave_engine::AutoPoint {
                    curve: 0.4,
                    ..prowave_engine::AutoPoint::new(4.0, 0.5)
                },
                prowave_engine::AutoPoint::new(8.0, 1.0),
            ],
        };
        let value =
            |l: &crate::model::PluginLane, k: usize| l.lo + l.points[k].value * (l.hi - l.lo);
        for (param, old_hi) in [(BASS808_DECAY, 8.0), (BASS808_RELEASE, 4.0)] {
            let before = old(param, old_hi, BASS808_ID);
            let mut after = before.clone();
            update_lane(&mut after);
            assert_eq!(
                (after.lo, after.hi),
                (BASS808_PARAMS[param].lo, BASS808_PARAMS[param].hi)
            );
            for k in 0..3 {
                let (was, now) = (value(&before, k), value(&after, k));
                assert!(
                    (was - now).abs() < 1e-5 * old_hi,
                    "{param} point {k}: {was} -> {now}"
                );
                assert_eq!(after.points[k].curve, before.points[k].curve);
            }
            // The middle point: 4.025 s (Decay) or 2.0025 s (Release), as before.
            println!(
                "{param}: 0.5 of the old range is {:.4}, now at {:.5}",
                value(&after, 1),
                after.points[1].value
            );
            // Loaded again, it is already on today's range: nothing moves.
            let again = after.clone();
            update_lane(&mut after);
            assert_eq!(after, again);
            // Another plug-in's lane with the same slot and parameter is left alone.
            let mut other = old(param, old_hi, "au:other");
            update_lane(&mut other);
            assert_eq!(other, old(param, old_hi, "au:other"));
        }
    }

    /// Through the file: a project whose 808 Decay lane was written with the old range opens
    /// with the lane on today's range and the same values.
    #[test]
    fn a_project_with_an_old_range_808_lane_opens_with_the_same_values() {
        let doc = prowave_project::AutoPluginDoc {
            slot: 0,
            param: prowave_engine::BASS808_DECAY as u32,
            plugin: BASS808_ID.into(),
            lo: 0.05,
            hi: 8.0,
            stepped: false,
            points: vec![[0.0, 0.5, 0.0], [4.0, 1.0, 0.0]],
        };
        let mut a = crate::model::Automation::default();
        crate::project_io::plugin_lanes_from_doc(&mut a, &[doc]);
        let l = &a.plugins[0];
        let v = |k: usize| l.lo + l.points[k].value * (l.hi - l.lo);
        assert_eq!((l.lo, l.hi), (0.05, 60.0));
        assert!(
            (v(0) - 4.025).abs() < 1e-4 && (v(1) - 8.0).abs() < 1e-4,
            "{} {}",
            v(0),
            v(1)
        );
    }
}
