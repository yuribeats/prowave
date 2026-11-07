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
use egui::{vec2, Color32, RichText, Sense};
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

/// The Channel Rack, in the editor pane under the tracks (Window > Channel Rack), where the
/// Piano Roll also goes.
pub fn show(ui: &mut egui::Ui, p: &mut Project, s: &mut crate::UiState) {
    egui::Frame::new()
        .fill(ui.visuals().panel_fill)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.heading(tr("Channel Rack"));
            let Some(ti) = rack_track(p) else {
                ui.label(tr("Select a track that plays the Channel Rack."));
                if ui.button(tr("New Channel Rack Track")).clicked() {
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
                let notes = &p.tracks[ti].regions[ri].notes;
                egui::ScrollArea::both().show(ui, |ui| {
                    for (c, ch) in channels.iter_mut().enumerate() {
                        let pitch = RACK_BASE_NOTE + c as u8;
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(ch.mute, "M")
                                .on_hover_text(tr("Mute"))
                                .clicked()
                            {
                                ch.mute = !ch.mute;
                                changed = true;
                            }
                            let label = if ch.name.is_empty() {
                                tr("(no sample)").to_string()
                            } else {
                                ch.name.clone()
                            };
                            let name = ui.add_sized(
                                vec2(110.0, 22.0),
                                egui::Button::new(RichText::new(label).size(11.0)).truncate(),
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
                            if ui
                                .add(
                                    egui::DragValue::new(&mut ch.gain_db)
                                        .range(-48.0..=12.0)
                                        .speed(0.2)
                                        .suffix(" dB"),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                            ui.add_space(6.0);
                            for k in 0..steps {
                                let at = k as f32 * STEP;
                                let on = notes
                                    .iter()
                                    .any(|n| n.pitch == pitch && (n.start - at).abs() < 0.01);
                                // Groups of four alternate, as in FL Studio.
                                let off_fill = if (k / 4) % 2 == 0 {
                                    Color32::from_gray(0x4a)
                                } else {
                                    Color32::from_rgb(0x5a, 0x44, 0x44)
                                };
                                let fill = if on {
                                    Color32::from_rgb(0xf0, 0xc8, 0x40)
                                } else {
                                    off_fill
                                };
                                let (rect, resp) =
                                    ui.allocate_exact_size(vec2(16.0, 22.0), Sense::click());
                                ui.painter().rect_filled(rect, 2.0, fill);
                                if resp.clicked() {
                                    toggles.push((pitch, k));
                                }
                            }
                        });
                    }
                });
            }
            ui.separator();
            if channels.len() < MAX_RACK_CHANNELS && ui.button(tr("Add Channel…")).clicked() {
                for path in pick_samples() {
                    if channels.len() < MAX_RACK_CHANNELS {
                        channels.push(channel_from(&path));
                        changed = true;
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
}
