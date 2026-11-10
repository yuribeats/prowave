//! The Channel Rack: a row of steps per channel, each channel playing one sample (Window >
//! Channel Rack). Modelled on the Channel Rack of Lemon Studio (GPL-3.0-or-later,
//! https://github.com/ItsAlphaNeon/Lemon-Studio), itself a recreation of FL Studio's.
//!
//! The rack is the instrument of an instrument track (its Instrument slot holds
//! [`RACK_ID`]); channel `k` plays on note `RACK_BASE_NOTE + k`. The steps are that track's
//! notes in the region being edited, one sixteenth note each, so the timeline, the piano roll
//! and playback all see what the grid shows.
use crate::i18n::tr;
use crate::model::{Note, Project, Region, RegionParams, TrackKind};
use crate::plugins::PluginSlot;
use crate::theme;
use egui::{vec2, Color32, Sense};
use prowave_engine::{RackChannelSpec, MAX_RACK_CHANNELS, RACK_BASE_NOTE};

/// The id of the Channel Rack in an Instrument slot.
pub const RACK_ID: &str = "prowave:rack";
/// One step, in quarter notes (a sixteenth note).
const STEP: f32 = 0.25;
/// Steps shown, at least and at most.
const MIN_STEPS: usize = 16;
const MAX_STEPS: usize = 64;
const VELOCITY: u8 = 100;

/// One channel: its sample and how it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct RackChannel {
    pub path: String,
    pub name: String,
    pub gain_db: f32,
    pub pan: f32,
    pub mute: bool,
}

/// Whether `slot` is the Channel Rack.
pub fn is_rack(slot: &PluginSlot) -> bool {
    slot.id == RACK_ID
}

/// An Instrument slot with an empty Channel Rack.
pub fn new_slot() -> PluginSlot {
    let mut slot = PluginSlot::new(RACK_ID, tr("Channel Rack"));
    slot.state = Some(encode(&[]));
    slot
}

/// The channels a rack slot's settings hold.
pub fn decode(state: &Option<Vec<u8>>) -> Vec<RackChannel> {
    let Some(v) = state
        .as_deref()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
    else {
        return Vec::new();
    };
    let text = |c: &serde_json::Value, k: &str| c[k].as_str().unwrap_or_default().to_string();
    let num = |c: &serde_json::Value, k: &str| c[k].as_f64().unwrap_or(0.0) as f32;
    v["channels"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(MAX_RACK_CHANNELS)
                .map(|c| RackChannel {
                    path: text(c, "path"),
                    name: text(c, "name"),
                    gain_db: num(c, "gain_db").clamp(-96.0, 12.0),
                    pan: num(c, "pan").clamp(-1.0, 1.0),
                    mute: c["mute"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn encode(channels: &[RackChannel]) -> Vec<u8> {
    let list: Vec<serde_json::Value> = channels
        .iter()
        .map(|c| {
            serde_json::json!({
                "path": c.path, "name": c.name, "gain_db": c.gain_db, "pan": c.pan, "mute": c.mute,
            })
        })
        .collect();
    serde_json::json!({ "channels": list })
        .to_string()
        .into_bytes()
}

/// What the engine's rack is made from.
pub fn specs(slot: &PluginSlot) -> Vec<RackChannelSpec> {
    decode(&slot.state)
        .into_iter()
        .map(|c| RackChannelSpec {
            path: c.path,
            gain_db: c.gain_db,
            pan: c.pan,
            mute: c.mute,
        })
        .collect()
}

fn channel_from(path: &std::path::Path) -> RackChannel {
    RackChannel {
        path: path.to_string_lossy().into_owned(),
        name: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        gain_db: 0.0,
        pan: 0.0,
        mute: false,
    }
}

fn pick_samples() -> Vec<std::path::PathBuf> {
    rfd::FileDialog::new()
        .add_filter(
            "Audio",
            &["wav", "aif", "aiff", "flac", "mp3", "caf", "ogg", "m4a"],
        )
        .pick_files()
        .unwrap_or_default()
}

/// The track whose rack the window shows: the selected one, when it plays the rack.
fn rack_track(p: &Project) -> Option<usize> {
    let i = p.selected_track;
    p.tracks
        .get(i)
        .filter(|t| t.au_instrument.as_ref().is_some_and(is_rack))
        .map(|_| i)
}

/// The region the steps go in: the selected region of the rack's track when it is a plain
/// MIDI region, else its first one, else a new one-bar region at the first bar.
fn step_region(p: &mut Project, ti: usize, selected: Option<(usize, usize)>) -> usize {
    let plain = |r: &Region| r.audio.is_none() && r.generated.is_none() && r.folder.is_none();
    let t = &p.tracks[ti];
    if let Some((st, sr)) = selected {
        if st == ti && t.regions.get(sr).is_some_and(plain) {
            return sr;
        }
    }
    if let Some(ri) = t.regions.iter().position(plain) {
        return ri;
    }
    let name = t.name.clone();
    p.tracks[ti].regions.push(Region {
        name,
        start_bar: 1.0,
        len_bars: 1.0,
        notes: Vec::new(),
        controls: Vec::new(),
        folder: None,
        generated: None,
        audio: None,
        params: RegionParams::default(),
    });
    p.song_rev += 1;
    p.tracks[ti].regions.len() - 1
}

/// A new instrument track playing an empty Channel Rack, selected.
fn new_rack_track(p: &mut Project) {
    let i = p.add_track(TrackKind::Instrument);
    p.tracks[i].name = tr("Channel Rack").to_string();
    p.tracks[i].au_instrument = Some(new_slot());
    p.selected_track = i;
}

/// Height of a rack row, the narrowest and widest step pad, and the gap between pads.
const ROW: f32 = 30.0;
const PAD_MIN: f32 = 22.0;
const PAD_MAX: f32 = 64.0;
const PAD_GAP: f32 = 3.0;
/// Extra space between groups of four steps.
const GROUP_GAP: f32 = 6.0;
/// The columns before the steps: Mute, the sample, the gain readout, and their gaps.
const MUTE_W: f32 = 38.0;
const SAMPLE_W: f32 = 150.0;
const GAIN_W: f32 = 84.0;
const STEPS_X: f32 = MUTE_W + 5.0 + SAMPLE_W + 5.0 + GAIN_W + 12.0;
/// Height of the step ruler (numbers and the playhead LEDs) over the steps.
const RULER_H: f32 = 30.0;

/// The width of one step pad when `steps` share `w` points.
fn pad_width(w: f32, steps: usize) -> f32 {
    let groups = steps.div_ceil(4) as f32;
    let gaps = (steps as f32 - 1.0) * PAD_GAP + (groups - 1.0) * GROUP_GAP;
    ((w - gaps) / steps as f32).clamp(PAD_MIN, PAD_MAX).floor()
}

/// The left edge of step `k` from the steps' left edge.
fn step_x(k: usize, pad_w: f32) -> f32 {
    k as f32 * (pad_w + PAD_GAP) + (k / 4) as f32 * GROUP_GAP
}

/// A bone key of the rack (Mute, Add Channel), latched while `on`, with a left LED when
/// `led` is Some. Returns its response.
fn rack_key(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    id: egui::Id,
    label: &str,
    on: bool,
    led: Option<bool>,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let g = ui.painter().clone();
    crate::hw::key(
        ui,
        &g,
        rect,
        id,
        crate::hw::Cap::Bone,
        on,
        led,
        crate::hw::Legend::Text(label),
    )
    .0
}

/// One step pad in `rect`: a bone key, a shade darker in every other group of four (as the
/// beats are told apart), signal orange while the step plays. A beat's first step carries an
/// ink line along the top of its cap.
fn step_pad(
    ui: &egui::Ui,
    g: &egui::Painter,
    rect: egui::Rect,
    k: usize,
    on: bool,
    id: egui::Id,
) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    let cap = if on {
        crate::hw::Cap::Orange
    } else {
        crate::hw::Cap::Bone
    };
    let face = crate::hw::paint_key(g, rect, cap, false, resp.hovered());
    if !on && !(k / 4).is_multiple_of(2) {
        g.rect_filled(face.shrink(1.0), 3.0, Color32::from_black_alpha(22));
    }
    if k.is_multiple_of(4) {
        g.hline(
            face.left() + 4.0..=face.right() - 4.0,
            face.top() + 2.5,
            egui::Stroke::new(1.0, theme::INK2),
        );
    }
    resp.clicked()
}

/// The step ruler over the steps: the number of each beat's first step in silkscreen, and an
/// LED per step, lit on the step the playhead is in.
fn step_ruler(
    g: &egui::Painter,
    left: f32,
    top: f32,
    steps: usize,
    pad_w: f32,
    playing: Option<usize>,
) {
    for k in 0..steps {
        let cx = left + step_x(k, pad_w) + pad_w / 2.0;
        if k.is_multiple_of(4) {
            crate::hw::silk(
                g,
                egui::pos2(left + step_x(k, pad_w) + 1.0, top + 7.0),
                egui::Align2::LEFT_CENTER,
                &(k + 1).to_string(),
                11.0,
                theme::INK2,
            );
        }
        crate::hw::led_sized(g, egui::pos2(cx, top + 21.0), 2.5, playing == Some(k));
    }
}

/// The Channel Rack, in the editor pane under the tracks (Window > Channel Rack), where the
/// Piano Roll also goes.
pub fn show(ui: &mut egui::Ui, p: &mut Project, s: &mut crate::UiState) {
    let full = ui.available_rect_before_wrap();
    {
        let g = ui.painter_at(full);
        g.rect_filled(full, 0.0, theme::BG);
        crate::hw::seam_h(&g, full.x_range(), full.top());
    }
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 10,
            bottom: 10,
        })
        .show(ui, |ui| {
            let head = ui
                .allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover())
                .0;
            crate::hw::bracket(
                ui.painter(),
                head.left(),
                head.right(),
                head.center().y,
                tr("Channel Rack"),
            );
            ui.add_space(8.0);
            let Some(ti) = rack_track(p) else {
                ui.label(
                    egui::RichText::new(tr("Select a track that plays the Channel Rack."))
                        .font(theme::font(theme::Face::Body, 12.0))
                        .color(theme::INK2),
                );
                ui.add_space(6.0);
                let label = tr("New Channel Rack Track").to_uppercase();
                let w = label.chars().count() as f32 * 8.0 + 28.0;
                if rack_key(
                    ui,
                    vec2(w, 28.0),
                    egui::Id::unique("rack_new"),
                    &label,
                    false,
                    None,
                )
                .clicked()
                {
                    new_rack_track(p);
                }
                return;
            };
            let ri = step_region(p, ti, s.tracks.selected);
            let len_beats = p.region_len_beats(&p.tracks[ti].regions[ri]);
            let steps =
                ((len_beats / f64::from(STEP)).round() as usize).clamp(MIN_STEPS, MAX_STEPS);
            let Some(slot) = p.tracks[ti].au_instrument.clone() else {
                return;
            };
            let mut channels = decode(&slot.state);
            let mut changed = false;
            let mut remove: Option<usize> = None;
            let mut toggles: Vec<(u8, usize)> = Vec::new();
            {
                let region = &p.tracks[ti].regions[ri];
                let notes = &region.notes;
                // The step the playhead is in, while it is inside the region.
                let from = p.position_beats - p.region_start_beats(region);
                let playing = (from >= 0.0)
                    .then(|| (from / f64::from(STEP)).floor() as usize)
                    .filter(|k| *k < steps);
                let avail = ui.available_width();
                let pad_w = pad_width(avail - STEPS_X, steps);
                let width = STEPS_X + step_x(steps - 1, pad_w) + pad_w;
                egui::ScrollArea::both().show(ui, |ui| {
                    let h = RULER_H + channels.len() as f32 * ROW;
                    let (area, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
                    let g = ui.painter().clone();
                    let sx = area.left() + STEPS_X;
                    step_ruler(&g, sx, area.top(), steps, pad_w, playing);
                    for (c, ch) in channels.iter_mut().enumerate() {
                        let pitch = RACK_BASE_NOTE + c as u8;
                        let y = area.top() + RULER_H + c as f32 * ROW;
                        let cell = |x: f32, w: f32| {
                            egui::Rect::from_min_size(egui::pos2(x, y), vec2(w, ROW - 6.0))
                        };
                        let (mute, _) = crate::hw::key(
                            ui,
                            &g,
                            cell(area.left(), MUTE_W),
                            egui::Id::unique(("rack_mute", c)),
                            crate::hw::Cap::Bone,
                            ch.mute,
                            Some(ch.mute),
                            crate::hw::Legend::Text("M"),
                        );
                        if mute.on_hover_text(tr("Mute")).clicked() {
                            ch.mute = !ch.mute;
                            changed = true;
                        }
                        let label = if ch.name.is_empty() {
                            tr("(no sample)").to_string()
                        } else {
                            ch.name.clone()
                        };
                        // The channel's sample: a black slot, its LED lit while it plays.
                        let r = cell(area.left() + MUTE_W + 5.0, SAMPLE_W);
                        let name =
                            ui.interact(r, egui::Id::unique(("rack_name", c)), Sense::click());
                        crate::hw::paint_slot(
                            &g,
                            r,
                            crate::hw::SlotKind::Filled,
                            &label,
                            Some(!ch.mute && !ch.name.is_empty()),
                            false,
                            name.hovered(),
                        );
                        if name.clicked() {
                            if let Some(path) = pick_samples().first() {
                                let fresh = channel_from(path);
                                ch.path = fresh.path;
                                ch.name = fresh.name;
                                changed = true;
                            }
                        }
                        name.context_menu(|ui| {
                            if ui.button(tr("Remove Channel")).clicked() {
                                remove = Some(c);
                                ui.close();
                            }
                        });
                        // Gain: an LCD readout, dragged; double-click for 0 dB.
                        let r = cell(area.left() + MUTE_W + 5.0 + SAMPLE_W + 5.0, GAIN_W);
                        let gain = ui.interact(
                            r,
                            egui::Id::unique(("rack_gain", c)),
                            Sense::click_and_drag(),
                        );
                        if gain.dragged() {
                            let to = (ch.gain_db - gain.drag_delta().y * 0.2).clamp(-48.0, 12.0);
                            if to != ch.gain_db {
                                ch.gain_db = to;
                                changed = true;
                            }
                        }
                        if gain.double_clicked() {
                            ch.gain_db = 0.0;
                            changed = true;
                        }
                        crate::hw::lcd_readout(&g, r, "dB", &format!("{:.1}", ch.gain_db));
                        for k in 0..steps {
                            let at = k as f32 * STEP;
                            let on = notes
                                .iter()
                                .any(|n| n.pitch == pitch && (n.start - at).abs() < 0.01);
                            let r = cell(sx + step_x(k, pad_w), pad_w);
                            let id = egui::Id::unique(("rack_step", c, k));
                            if step_pad(ui, &g, r, k, on, id) {
                                toggles.push((pitch, k));
                            }
                        }
                    }
                });
            }
            ui.add_space(6.0);
            if channels.len() < MAX_RACK_CHANNELS {
                let label = tr("Add Channel…").to_uppercase();
                let w = label.chars().count() as f32 * 8.0 + 28.0;
                if rack_key(
                    ui,
                    vec2(w, 28.0),
                    egui::Id::unique("rack_add"),
                    &label,
                    false,
                    None,
                )
                .clicked()
                {
                    for path in pick_samples() {
                        if channels.len() < MAX_RACK_CHANNELS {
                            channels.push(channel_from(&path));
                            changed = true;
                        }
                    }
                }
            }
            let region = &mut p.tracks[ti].regions[ri];
            for (pitch, k) in toggles {
                let at = k as f32 * STEP;
                let found = region
                    .notes
                    .iter()
                    .position(|n| n.pitch == pitch && (n.start - at).abs() < 0.01);
                match found {
                    Some(i) => {
                        region.notes.remove(i);
                    }
                    None => {
                        region.notes.push(Note {
                            lyric: crate::lyrics::Lyric::NONE,
                            mute: false,
                            start: at,
                            len: STEP,
                            pitch,
                            vel: VELOCITY,
                        });
                        region.notes.sort_by(|a, b| a.start.total_cmp(&b.start));
                    }
                }
                p.song_rev += 1;
            }
            if let Some(c) = remove {
                channels.remove(c);
                // The channels after it move up a note, and their steps with them.
                let gone = RACK_BASE_NOTE + c as u8;
                for r in p.tracks[ti].regions.iter_mut() {
                    r.notes.retain(|n| n.pitch != gone);
                    for n in r.notes.iter_mut().filter(|n| n.pitch > gone) {
                        n.pitch -= 1;
                    }
                }
                p.song_rev += 1;
                changed = true;
            }
            if changed {
                if let Some(slot) = p.tracks[ti].au_instrument.as_mut() {
                    slot.state = Some(encode(&channels));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_round_trip_through_the_slot() {
        let channels = vec![RackChannel {
            path: "/k/kick.wav".into(),
            name: "kick".into(),
            gain_db: -3.0,
            pan: 0.25,
            mute: true,
        }];
        assert_eq!(decode(&Some(encode(&channels))), channels);
        assert!(decode(&None).is_empty());
        assert!(decode(&Some(b"not json".to_vec())).is_empty());
    }

    #[test]
    fn a_rack_track_gets_a_one_bar_region_for_its_steps() {
        let mut p = Project::empty();
        new_rack_track(&mut p);
        let ti = p.selected_track;
        assert_eq!(rack_track(&p), Some(ti));
        let ri = step_region(&mut p, ti, None);
        assert_eq!(p.tracks[ti].regions[ri].len_bars, 1.0);
        assert_eq!(
            step_region(&mut p, ti, None),
            ri,
            "the same region next time"
        );
    }

    /// Where the pictures go: `PROWAVE_SHOT_DIR`, else a folder in the temp directory.
    fn shot_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::var_os("PROWAVE_SHOT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-mixer"));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    /// The rack with five channels and a pattern.
    #[test]
    #[ignore = "pictures"]
    fn channel_rack_pictures() {
        let mut p = Project::empty();
        new_rack_track(&mut p);
        let ti = p.selected_track;
        let names = ["Kick", "Snare", "Hat", "Open Hat", "Clap"];
        let channels: Vec<RackChannel> = names
            .iter()
            .map(|n| RackChannel {
                path: format!("/samples/{n}.wav"),
                name: (*n).into(),
                gain_db: 0.0,
                pan: 0.0,
                mute: *n == "Clap",
            })
            .collect();
        if let Some(slot) = p.tracks[ti].au_instrument.as_mut() {
            slot.state = Some(encode(&channels));
        }
        let ri = step_region(&mut p, ti, None);
        let steps: [&[usize]; 5] = [
            &[0, 7, 10],
            &[4, 12],
            &[0, 2, 4, 6, 8, 10, 12, 14],
            &[15],
            &[4, 12],
        ];
        for (c, ks) in steps.iter().enumerate() {
            for &k in *ks {
                p.tracks[ti].regions[ri].notes.push(Note {
                    lyric: crate::lyrics::Lyric::NONE,
                    start: k as f32 * STEP,
                    len: STEP,
                    pitch: RACK_BASE_NOTE + c as u8,
                    vel: VELOCITY,
                    mute: false,
                });
            }
        }
        let mut s = crate::UiState::new();
        crate::ui_test::render_png(
            1200.0,
            300.0,
            2.0,
            crate::theme::BONE,
            |ui| {
                crate::theme::apply(ui.ctx());
                show(ui, &mut p, &mut s)
            },
            &shot_path("channel-rack.png"),
        )
        .unwrap();
    }
}
