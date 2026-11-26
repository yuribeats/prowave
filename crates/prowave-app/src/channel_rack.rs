//! The Channel Rack: a row of steps per channel, each channel playing one sample (Window >
//! Channel Rack). Modelled on the Channel Rack of Lemon Studio (GPL-3.0-or-later,
//! https://github.com/ItsAlphaNeon/Lemon-Studio), itself a recreation of FL Studio's.
//!
//! The rack is the instrument of an instrument track (its Instrument slot holds
//! [`RACK_ID`]); channel `k` plays on note `RACK_BASE_NOTE + k`. The steps are that track's
//! notes in the region being edited, one sixteenth note each, so the timeline, the piano roll
//! and playback all see what the grid shows. A step can be split into two to four hits (a
//! ratchet, a roll, a triplet: [`rack_groove::Cell`]), and every hit has its own velocity (the
//! note's).
//!
//! A pattern ([`RackPattern`]: hits by drum class, with their velocities) is loaded onto the
//! channels by class ([`load_pattern`]); its velocities are kept so the velocity amount can take
//! them toward the rack's velocity and back.
use crate::i18n::tr;
use crate::kit_classify::{self, DrumClass};
use crate::model::{Note, Project, Region, RegionParams, TrackKind};
use crate::plugins::PluginSlot;
use crate::rack_groove::{self, Cell, Groove};
use crate::theme;
use egui::{vec2, Color32, Sense};
use prowave_engine::{RackChannelSpec, MAX_RACK_CHANNELS, RACK_BASE_NOTE};

/// The id of the Channel Rack in an Instrument slot.
pub const RACK_ID: &str = "prowave:rack";
/// One step, in quarter notes (a sixteenth note).
const STEP: f32 = 0.25;
/// Steps shown, at least and at most.
const MIN_STEPS: usize = 16;
pub const MAX_STEPS: usize = 64;
/// Two hits of one channel in one cell closer than this (seconds) are one hit found twice.
const DOUBLE_S: f64 = 0.010;
/// The longest pattern a rack loads, in steps (1024 bars of 4/4), in regions of [`MAX_STEPS`].
pub const MAX_PATTERN_STEPS: usize = 1024 * 16;
/// The velocity a new hit gets in a new rack.
pub const VELOCITY: u8 = 100;

/// One channel: its sample, how it plays, and the drum class whose feel its steps take.
#[derive(Clone, Debug, PartialEq)]
pub struct RackChannel {
    pub path: String,
    pub name: String,
    pub gain_db: f32,
    pub pan: f32,
    pub mute: bool,
    /// Solo: while any channel of the rack is soloed, only the soloed ones play.
    pub solo: bool,
    pub class: DrumClass,
}

impl RackChannel {
    /// A channel with no sample.
    pub fn empty(class: DrumClass) -> Self {
        Self {
            path: String::new(),
            name: String::new(),
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            class,
        }
    }
}

/// The classes a channel can be given, in the order a menu lists them.
pub const CHANNEL_CLASSES: [DrumClass; 16] = [
    DrumClass::Kick,
    DrumClass::Snare,
    DrumClass::Clap,
    DrumClass::ClosedHat,
    DrumClass::OpenHat,
    DrumClass::Rim,
    DrumClass::TomLow,
    DrumClass::TomMid,
    DrumClass::TomHigh,
    DrumClass::Perc,
    DrumClass::Shaker,
    DrumClass::Crash,
    DrumClass::Ride,
    DrumClass::Bass808,
    DrumClass::Fx,
    DrumClass::Other,
];

/// One hit of a loaded pattern (or of the steps a loaded groove gave velocities), as the rack
/// keeps it: its channel (from 0), its cell (the step counted within the pattern), the velocity
/// the source gives it, and the velocity it plays at a velocity amount of 0 (a pattern's hit:
/// the rack's velocity when it was loaded; a step a groove reached: its velocity before).
/// `played`: where the source played it, in quarter notes from the cell's straight place
/// (None: the source has no time of its own for it, and it plays where the groove puts it).
/// A played hit is never moved by the groove: its note stays exactly where it was played.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatternHit {
    pub channel: usize,
    pub cell: Cell,
    pub vel: u8,
    pub base: u8,
    pub played: Option<f32>,
}

/// How near a note has to be to a played hit's place to be that hit, in quarter notes.
const PLAYED_NEAR: f32 = 1e-4;

/// The pattern loaded into a rack: its name, how many steps it is, and its hits' velocities.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedPattern {
    pub name: String,
    pub source: String,
    pub steps: usize,
    pub hits: Vec<PatternHit>,
    /// The song's folder it came from (empty: none), and the song's bar (from 1) its first
    /// step is (0: not known): the song's own kit and files line up with it.
    pub dir: String,
    pub bar: usize,
}

/// Everything a rack slot keeps: its channels, the kit they were loaded from and the feel.
#[derive(Clone, Debug, PartialEq)]
pub struct RackState {
    pub channels: Vec<RackChannel>,
    /// The name of the kit the channels were loaded from (empty: none).
    pub kit: String,
    /// Its folder, when it was loaded from one (empty: not known).
    pub kit_dir: String,
    pub groove: Groove,
    /// The velocity of a hit added by hand (and of a pattern's hits at a velocity amount of 0).
    pub velocity: u8,
    /// How much of the source's velocities (a loaded pattern's or groove's) plays, from 0 (each
    /// hit at its base) to 1.
    pub velocity_amount: f32,
    /// The pattern loaded last (empty name: none).
    pub pattern: LoadedPattern,
}

impl Default for RackState {
    fn default() -> Self {
        Self {
            channels: Vec::new(),
            kit: String::new(),
            kit_dir: String::new(),
            groove: Groove::default(),
            velocity: VELOCITY,
            velocity_amount: 1.0,
            pattern: LoadedPattern::default(),
        }
    }
}

/// The velocity a pattern hit loaded at `src` plays with amount `amount` toward `base`.
pub fn scaled_velocity(src: u8, base: u8, amount: f32) -> u8 {
    let v = f32::from(base) + (f32::from(src) - f32::from(base)) * amount.clamp(0.0, 1.0);
    v.round().clamp(1.0, 127.0) as u8
}

/// The velocity a pattern hit has to have been loaded at to play `vel` (amount above 0).
fn unscaled_velocity(vel: u8, base: u8, amount: f32) -> u8 {
    let v = f32::from(base) + (f32::from(vel) - f32::from(base)) / amount.max(0.01);
    v.round().clamp(1.0, 127.0) as u8
}

impl LoadedPattern {
    fn to_json(&self) -> serde_json::Value {
        let hits: Vec<serde_json::Value> = self
            .hits
            .iter()
            .map(|h| {
                serde_json::json!([
                    h.channel,
                    h.cell.step,
                    h.cell.sub,
                    h.cell.div,
                    h.vel,
                    h.base,
                    h.played
                ])
            })
            .collect();
        let mut v = serde_json::json!({
            "name": self.name, "source": self.source, "steps": self.steps, "hits": hits,
        });
        if !self.dir.is_empty() {
            v["dir"] = self.dir.clone().into();
            v["bar"] = self.bar.into();
        }
        v
    }

    fn from_json(v: &serde_json::Value) -> Self {
        let hits = v["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                let n = |i: usize| h[i].as_u64();
                Some(PatternHit {
                    channel: usize::try_from(n(0)?).ok()?.min(MAX_RACK_CHANNELS),
                    cell: Cell::new(
                        usize::try_from(n(1)?).ok()?.min(MAX_STEPS),
                        n(2)?.min(8) as u8,
                        n(3)?.clamp(1, 8) as u8,
                    ),
                    vel: n(4)?.clamp(1, 127) as u8,
                    base: n(5).map_or(VELOCITY, |b| b.clamp(1, 127) as u8),
                    played: h[6].as_f64().filter(|x| x.is_finite()).map(|x| x as f32),
                })
            })
            .collect();
        Self {
            name: v["name"].as_str().unwrap_or_default().to_string(),
            source: v["source"].as_str().unwrap_or_default().to_string(),
            steps: v["steps"].as_u64().unwrap_or(0).min(MAX_STEPS as u64) as usize,
            hits,
            dir: v["dir"].as_str().unwrap_or_default().to_string(),
            bar: v["bar"].as_u64().unwrap_or(0) as usize,
        }
    }

    /// The source's velocity of channel `c`'s hit at `cell` and its base (the pattern repeats
    /// over a longer region).
    fn velocity_at(&self, c: usize, cell: Cell) -> Option<(u8, u8)> {
        if self.steps == 0 {
            return None;
        }
        let at = Cell {
            step: cell.step % self.steps,
            ..cell
        };
        self.hits
            .iter()
            .find(|h| h.channel == c && h.cell == at)
            .map(|h| (h.vel, h.base))
    }

    fn velocity_at_mut(&mut self, c: usize, cell: Cell) -> Option<&mut PatternHit> {
        if self.steps == 0 {
            return None;
        }
        let at = Cell {
            step: cell.step % self.steps,
            ..cell
        };
        self.hits
            .iter_mut()
            .find(|h| h.channel == c && h.cell == at)
    }

    /// Where channel `c`'s hit at `cell` was played (the pattern repeats over a longer region),
    /// in quarter notes from the region's start; None when it is not a played hit.
    fn played_at(&self, c: usize, cell: Cell) -> Option<f32> {
        if self.steps == 0 {
            return None;
        }
        let at = Cell {
            step: cell.step % self.steps,
            ..cell
        };
        let h = self.hits.iter().find(|h| h.channel == c && h.cell == at)?;
        Some(self.wrapped(h.cell, h.played?) + (cell.straight() - at.straight()) as f32)
    }

    /// Where a hit at `cell` (within the pattern) played `dev` from it sits in the pattern: a
    /// hit played before the pattern's start (a hat pushed ahead of the one) sits that far
    /// before the pattern's end, so a looped pattern plays it just ahead of its next one.
    fn wrapped(&self, cell: Cell, dev: f32) -> f32 {
        let at = cell.straight() as f32 + dev;
        if at < 0.0 {
            at + self.steps as f32 * STEP
        } else {
            at
        }
    }

    /// The played hit of channel `c` whose note starts at `start` (quarter notes from the
    /// region's start), as its cell in the region.
    fn played_cell(&self, c: usize, start: f32) -> Option<Cell> {
        if self.steps == 0 {
            return None;
        }
        let span = self.steps as f32 * STEP;
        let reps = (MAX_STEPS / self.steps).max(1);
        self.hits
            .iter()
            .filter(|h| h.channel == c)
            .filter_map(|h| Some((h, h.played?)))
            .find_map(|(h, dev)| {
                let at = self.wrapped(h.cell, dev);
                let rep = ((start - at) / span).round();
                if rep < 0.0 || rep as usize >= reps {
                    return None;
                }
                let place = at + rep * span;
                ((place - start).abs() < PLAYED_NEAR).then(|| Cell {
                    step: h.cell.step + rep as usize * self.steps,
                    ..h.cell
                })
            })
    }

    /// Whether the pattern has hits with their own played times.
    pub fn is_played(&self) -> bool {
        self.hits.iter().any(|h| h.played.is_some())
    }

    /// Channel `c` is gone: its hits go and the channels after it move up one.
    fn remove_channel(&mut self, c: usize) {
        self.hits.retain(|h| h.channel != c);
        for h in self.hits.iter_mut().filter(|h| h.channel > c) {
            h.channel -= 1;
        }
    }
}

impl RackState {
    /// What a rack slot's settings hold. A channel saved without a class takes the class of
    /// its sample's name.
    pub fn read(state: &Option<Vec<u8>>) -> Self {
        let Some(v) = state
            .as_deref()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
        else {
            return Self::default();
        };
        let text = |c: &serde_json::Value, k: &str| c[k].as_str().unwrap_or_default().to_string();
        let num = |c: &serde_json::Value, k: &str| c[k].as_f64().unwrap_or(0.0) as f32;
        let channels = v["channels"]
            .as_array()
            .map(|a| {
                a.iter()
                    .take(MAX_RACK_CHANNELS)
                    .map(|c| {
                        let path = text(c, "path");
                        let class = c["class"]
                            .as_str()
                            .and_then(DrumClass::from_id)
                            .unwrap_or_else(|| class_of_path(&path));
                        RackChannel {
                            name: text(c, "name"),
                            gain_db: num(c, "gain_db").clamp(-96.0, 12.0),
                            pan: num(c, "pan").clamp(-1.0, 1.0),
                            mute: c["mute"].as_bool().unwrap_or(false),
                            solo: c["solo"].as_bool().unwrap_or(false),
                            path,
                            class,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            channels,
            kit: v["kit"].as_str().unwrap_or_default().to_string(),
            kit_dir: v["kit_dir"].as_str().unwrap_or_default().to_string(),
            groove: Groove::from_json(&v["groove"]),
            velocity: v["velocity"]
                .as_u64()
                .map_or(VELOCITY, |x| x.clamp(1, 127) as u8),
            velocity_amount: v["velocity_amount"]
                .as_f64()
                .filter(|x| x.is_finite())
                .map_or(1.0, |x| (x as f32).clamp(0.0, 1.0)),
            pattern: LoadedPattern::from_json(&v["pattern"]),
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        let list: Vec<serde_json::Value> = self
            .channels
            .iter()
            .map(|c| {
                let mut v = serde_json::json!({
                    "path": c.path, "name": c.name, "gain_db": c.gain_db, "pan": c.pan,
                    "mute": c.mute, "class": c.class.id(),
                });
                if c.solo {
                    v["solo"] = true.into();
                }
                v
            })
            .collect();
        let mut v = serde_json::json!({ "channels": list });
        if !self.kit.is_empty() {
            v["kit"] = self.kit.clone().into();
        }
        if !self.kit_dir.is_empty() {
            v["kit_dir"] = self.kit_dir.clone().into();
        }
        if self.groove != Groove::default() {
            v["groove"] = self.groove.to_json();
        }
        if self.velocity != VELOCITY {
            v["velocity"] = self.velocity.into();
        }
        if self.velocity_amount != 1.0 {
            v["velocity_amount"] = self.velocity_amount.into();
        }
        if self.pattern != LoadedPattern::default() {
            v["pattern"] = self.pattern.to_json();
        }
        v.to_string().into_bytes()
    }

    /// Each channel's class, from the first.
    pub fn classes(&self) -> Vec<DrumClass> {
        self.channels.iter().map(|c| c.class).collect()
    }

    /// The class of channel `c` (Other past the last).
    pub fn class(&self, c: usize) -> DrumClass {
        self.channels.get(c).map_or(DrumClass::Other, |ch| ch.class)
    }

    /// The feel channel `c` plays (its class's, with the amounts applied).
    pub fn feel(&self, c: usize) -> rack_groove::Feel {
        self.groove.play(self.class(c))
    }

    /// Where channel `c`'s hit at `cell` plays: where the loaded pattern's source played it, else
    /// where the channel's feel puts it (a step added by hand).
    pub fn place(&self, c: usize, cell: Cell) -> f32 {
        self.pattern
            .played_at(c, cell)
            .unwrap_or_else(|| self.feel(c).place(cell))
    }

    /// The cell of channel `c`'s note starting at `start`: a played hit of the loaded pattern,
    /// else a step of the channel's feel (None: a note moved by hand).
    pub fn cell_at(&self, c: usize, start: f32) -> Option<Cell> {
        self.pattern
            .played_cell(c, start)
            .or_else(|| rack_groove::cell_of(start, &self.feel(c)))
    }

    /// Whether `n` is a played hit of the loaded pattern (the groove never moves it).
    fn is_played_note(&self, n: &Note) -> bool {
        n.pitch
            .checked_sub(RACK_BASE_NOTE)
            .is_some_and(|c| self.pattern.played_cell(usize::from(c), n.start).is_some())
    }

    /// The velocity a hit of channel `c` at `cell` plays with: the loaded pattern's, scaled by
    /// the velocity amount; None when the pattern has no hit there.
    pub fn pattern_velocity(&self, c: usize, cell: Cell) -> Option<u8> {
        self.pattern
            .velocity_at(c, cell)
            .map(|(v, base)| scaled_velocity(v, base, self.velocity_amount))
    }
}

/// The class a sample's name says (a channel with no sample: Other).
fn class_of_path(path: &str) -> DrumClass {
    if path.is_empty() {
        return DrumClass::Other;
    }
    match kit_classify::classify(path) {
        DrumClass::Loop | DrumClass::SoundFont => DrumClass::Other,
        c => c,
    }
}

/// Whether `slot` is the Channel Rack.
pub fn is_rack(slot: &PluginSlot) -> bool {
    slot.id == RACK_ID
}

/// An Instrument slot with a new Channel Rack: the kit the Kit Browser's settings name for new
/// racks (`default_kit` in `kit-library/settings.json`), or empty when there is none or its
/// folder is not there.
pub fn new_slot() -> PluginSlot {
    let mut slot = PluginSlot::new(RACK_ID, tr("Channel Rack"));
    slot.state = Some(default_state().bytes());
    slot
}

/// What a new rack holds (see [`new_slot`]). Tests never read the machine's library.
fn default_state() -> RackState {
    if cfg!(test) {
        return RackState::default();
    }
    let Some(data) = crate::kit_library::data_dir() else {
        return kit_state(std::path::Path::new(&crate::kit_library::default_kit()));
    };
    let settings = crate::kit_library::Settings::load(&data);
    let kit = std::path::PathBuf::from(&settings.default_kit);
    // A default kit on another computer is copied here on another thread (a share can take
    // seconds a file) and named here from then on; this rack plays it from the share.
    let copy =
        settings.copy_kits == crate::kit_copy::CopyMode::Always || crate::kit_copy::is_remote(&kit);
    if copy && kit.is_dir() && !COPYING_DEFAULT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let mode = settings.copy_kits;
        let from = kit.clone();
        let spawned = std::thread::Builder::new()
            .name("default-kit-copy".into())
            .spawn(move || {
                let local = crate::kit_library::local_dir();
                let here = crate::kit_copy::localize_kit_folder(local.as_deref(), mode, &from);
                if here != from {
                    // Read again: the Kit Browser may have saved the settings meanwhile.
                    let mut now = crate::kit_library::Settings::load(&data);
                    if now.default_kit == from.to_string_lossy() {
                        now.default_kit = here.to_string_lossy().into_owned();
                        if let Err(e) = now.save(&data) {
                            tracing::warn!("could not save the kit library settings: {e}");
                        }
                    }
                }
                COPYING_DEFAULT.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            tracing::warn!("could not start copying the default kit: {e}");
            COPYING_DEFAULT.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    kit_state(&kit)
}

/// The default kit is being copied here (one copy at a time).
static COPYING_DEFAULT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the default kit is being copied here.
pub fn copying_default() -> bool {
    COPYING_DEFAULT.load(std::sync::atomic::Ordering::SeqCst)
}

/// A rack holding the kit in folder `dir` as Load Kit puts it on the channels, each channel
/// with its sample's class; empty when `dir` is not a folder or has no drum samples.
pub fn kit_state(dir: &std::path::Path) -> RackState {
    if dir.as_os_str().is_empty() || !dir.is_dir() {
        return RackState::default();
    }
    let kit = crate::kit_files::read_kit(dir);
    let channels = kit
        .pads
        .iter()
        .take(MAX_RACK_CHANNELS)
        .enumerate()
        .map(|(k, pad)| match pad {
            Some(i) => {
                let mut c = channel_from(&kit.path_of(*i));
                c.class = kit.samples[*i].class;
                c
            }
            None => RackChannel::empty(layout_class(k)),
        })
        .collect();
    RackState {
        channels,
        kit: dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        kit_dir: dir.to_string_lossy().into_owned(),
        ..RackState::default()
    }
}

/// The class Load Kit gives channel `k` (from 0) when it has nothing of its own.
fn layout_class(k: usize) -> DrumClass {
    kit_classify::PAD_LAYOUT
        .get(k)
        .copied()
        .unwrap_or(DrumClass::Other)
}

/// The channels a rack slot's settings hold.
pub fn decode(state: &Option<Vec<u8>>) -> Vec<RackChannel> {
    RackState::read(state).channels
}

/// The name of the kit a rack slot's channels were loaded from (empty: none).
pub fn kit_of(state: &Option<Vec<u8>>) -> String {
    RackState::read(state).kit
}

#[cfg(test)]
pub fn encode(channels: &[RackChannel]) -> Vec<u8> {
    encode_kit(channels, "")
}

/// [`encode`], with the name of the kit the channels were loaded from.
#[cfg(test)]
pub fn encode_kit(channels: &[RackChannel], kit: &str) -> Vec<u8> {
    RackState {
        channels: channels.to_vec(),
        kit: kit.to_string(),
        ..RackState::default()
    }
    .bytes()
}

/// What the engine's rack is made from.
pub fn specs(slot: &PluginSlot) -> Vec<RackChannelSpec> {
    let channels = decode(&slot.state);
    let soloed = channels.iter().any(|c| c.solo);
    channels
        .into_iter()
        .map(|c| RackChannelSpec {
            mute: c.mute || (soloed && !c.solo),
            path: c.path,
            gain_db: c.gain_db,
            pan: c.pan,
        })
        .collect()
}

fn channel_from(path: &std::path::Path) -> RackChannel {
    let p = path.to_string_lossy().into_owned();
    RackChannel {
        class: class_of_path(&p),
        path: p,
        name: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        gain_db: 0.0,
        pan: 0.0,
        mute: false,
        solo: false,
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
pub fn rack_track(p: &Project) -> Option<usize> {
    let i = p.selected_track;
    p.tracks
        .get(i)
        .filter(|t| t.au_instrument.as_ref().is_some_and(is_rack))
        .map(|_| i)
}

/// The rack of track `ti`, when it plays one.
pub fn rack_state(p: &Project, ti: usize) -> Option<RackState> {
    p.tracks
        .get(ti)?
        .au_instrument
        .as_ref()
        .filter(|s| is_rack(s))
        .map(|s| RackState::read(&s.state))
}

/// Puts `new` in the rack of track `ti` and moves the steps of every plain MIDI region of the
/// track from where `old` (what the rack held) played them to where `new` plays them. Returns
/// how many notes moved; false-y when the track has no rack.
pub fn commit(p: &mut Project, ti: usize, old: &RackState, new: RackState) -> usize {
    let Some(slot) = p
        .tracks
        .get_mut(ti)
        .and_then(|t| t.au_instrument.as_mut())
        .filter(|s| is_rack(s))
    else {
        return 0;
    };
    slot.state = Some(new.bytes());
    let (oc, nc) = (old.classes(), new.classes());
    let mut moved = 0;
    if old.groove != new.groove || oc != nc {
        for r in p.tracks[ti].regions.iter_mut().filter(|r| is_plain(r)) {
            // Played hits stay where they were played; only the steps on the groove move.
            let (mut played, mut rest): (Vec<Note>, Vec<Note>) =
                r.notes.drain(..).partition(|n| old.is_played_note(n));
            moved += rack_groove::retime(
                &mut rest,
                RACK_BASE_NOTE,
                &old.groove,
                &oc,
                &new.groove,
                &nc,
            );
            rest.append(&mut played);
            r.notes = rest;
            r.notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        }
    }
    // The velocity amount (or the velocity it scales toward) changed: the pattern's hits take
    // their velocities from what they were loaded with.
    if old.velocity_amount != new.velocity_amount || old.pattern != new.pattern {
        for r in p.tracks[ti].regions.iter_mut().filter(|r| is_plain(r)) {
            revelocity(&mut r.notes, &new);
        }
    }
    p.song_rev += 1;
    moved
}

/// Gives every hit of `notes` that is a hit of the rack's loaded pattern the velocity the
/// pattern and the velocity amount say. Returns how many changed.
fn revelocity(notes: &mut [Note], st: &RackState) -> usize {
    if st.pattern.hits.is_empty() {
        return 0;
    }
    let mut changed = 0;
    for n in notes.iter_mut() {
        let Some(c) = n.pitch.checked_sub(RACK_BASE_NOTE).map(usize::from) else {
            continue;
        };
        if c >= st.channels.len() {
            continue;
        }
        let Some(cell) = st.cell_at(c, n.start) else {
            continue;
        };
        if let Some(v) = st.pattern_velocity(c, cell) {
            if n.vel != v {
                n.vel = v;
                changed += 1;
            }
        }
    }
    changed
}

/// Sets the class of channel `c` (from 0) of the rack on track `ti`; its steps move to the
/// class's feel. False when there is no such channel.
pub fn set_class(p: &mut Project, ti: usize, c: usize, class: DrumClass) -> bool {
    let Some(old) = rack_state(p, ti) else {
        return false;
    };
    if c >= old.channels.len() {
        return false;
    }
    let mut new = old.clone();
    new.channels[c].class = class;
    commit(p, ti, &old, new);
    true
}

/// Gives the rack on track `ti` the feel `groove` (a loaded groove, or the swing changed);
/// its steps move. Returns how many notes moved, None when the track has no rack.
pub fn set_groove(p: &mut Project, ti: usize, groove: Groove) -> Option<usize> {
    let old = rack_state(p, ti)?;
    let mut new = old.clone();
    let fresh = !groove.velocities.is_empty() && groove.velocities != old.groove.velocities;
    new.groove = groove;
    if fresh {
        groove_velocities(p, ti, &mut new);
    }
    Some(commit(p, ti, &old, new))
}

/// A groove with velocities was loaded: every step of the rack's region (the one the window
/// shows) whose class and sixteenth it has a velocity for keeps that as its source velocity
/// (its velocity now as its base, unless it had one), so the velocity amount takes it from its
/// velocity before (0 %) to the groove's (100 %).
fn groove_velocities(p: &Project, ti: usize, st: &mut RackState) {
    let Some(ri) = find_step_region(p, ti, None) else {
        return;
    };
    if st.pattern.steps == 0 {
        st.pattern.steps = MAX_STEPS;
    }
    let notes = &p.tracks[ti].regions[ri].notes;
    // Where the steps are now (the feel before this groove).
    let before = rack_state(p, ti).unwrap_or_default();
    for c in 0..st.channels.len() {
        let class = st.class(c);
        for (cell, vel, _) in hits_of(notes, &before, c, MAX_STEPS) {
            let Some(src) = st.groove.velocity(class, cell.step) else {
                continue;
            };
            let at = Cell {
                step: cell.step % st.pattern.steps,
                ..cell
            };
            match st
                .pattern
                .hits
                .iter_mut()
                .find(|h| h.channel == c && h.cell == at)
            {
                Some(h) => h.vel = src,
                None => st.pattern.hits.push(PatternHit {
                    channel: c,
                    cell: at,
                    vel: src,
                    base: vel,
                    played: None,
                }),
            }
        }
    }
}

/// A plain MIDI region: what the rack's steps can be in.
fn is_plain(r: &Region) -> bool {
    !r.is_audio() && r.generated.is_none() && r.folder.is_none()
}

/// The region the steps are in, when the track has one (see [`step_region`]).
pub fn find_step_region(p: &Project, ti: usize, selected: Option<(usize, usize)>) -> Option<usize> {
    let t = p.tracks.get(ti)?;
    if let Some((st, sr)) = selected {
        if st == ti && t.regions.get(sr).is_some_and(is_plain) {
            return Some(sr);
        }
    }
    t.regions.iter().position(is_plain)
}

/// The region the steps go in: the selected region of the rack's track when it is a plain
/// MIDI region, else its first one, else a new one-bar region at the first bar.
pub fn step_region(p: &mut Project, ti: usize, selected: Option<(usize, usize)>) -> usize {
    if let Some(ri) = find_step_region(p, ti, selected) {
        return ri;
    }
    let name = p.tracks[ti].name.clone();
    p.tracks[ti].regions.push(Region {
        missing: None,
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

/// The hits of channel `c` in `notes` on the first `steps` steps, in order: each one's cell,
/// velocity and note (its index in `notes`).
pub fn hits_of(
    notes: &[Note],
    state: &RackState,
    c: usize,
    steps: usize,
) -> Vec<(Cell, u8, usize)> {
    let pitch = RACK_BASE_NOTE + c as u8;
    let mut out: Vec<(Cell, u8, usize)> = notes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.pitch == pitch)
        .filter_map(|(i, n)| Some((state.cell_at(c, n.start)?, n.vel, i)))
        .filter(|(cell, _, _)| cell.step < steps)
        .collect();
    out.sort_by_key(|h| h.0);
    out.dedup_by_key(|h| h.0);
    out
}

/// The steps of channel `c` on in `notes` (any hit in them), as step numbers from 0 (`steps`
/// of them).
pub fn steps_on(notes: &[Note], state: &RackState, c: usize, steps: usize) -> Vec<usize> {
    let mut v: Vec<usize> = hits_of(notes, state, c, steps)
        .into_iter()
        .map(|h| h.0.step)
        .collect();
    v.dedup();
    v
}

/// The note of channel `c`'s hit at `cell`.
fn note_at(notes: &[Note], state: &RackState, c: usize, cell: Cell) -> Option<usize> {
    hits_of(notes, state, c, cell.step + 1)
        .into_iter()
        .find(|h| h.0 == cell)
        .map(|h| h.2)
}

fn rack_note(start: f32, len: f32, pitch: u8, vel: u8) -> Note {
    Note {
        lyric: crate::lyrics::Lyric::NONE,
        mute: false,
        start,
        len,
        pitch,
        vel: vel.clamp(1, 127),
    }
}

/// Puts channel `c`'s hit at `cell` in `region` (where the loaded pattern's source played it,
/// else where the channel's feel plays it; with velocity `vel`, else the pattern's there, else
/// the rack's), or takes it out.
pub fn set_hit(
    region: &mut Region,
    state: &RackState,
    c: usize,
    cell: Cell,
    on: bool,
    vel: Option<u8>,
) {
    let pitch = RACK_BASE_NOTE + c as u8;
    match (note_at(&region.notes, state, c, cell), on) {
        (Some(i), false) => {
            region.notes.remove(i);
        }
        (Some(i), true) => {
            if let Some(v) = vel {
                region.notes[i].vel = v.clamp(1, 127);
            }
        }
        (None, true) => {
            let v = vel
                .or_else(|| state.pattern_velocity(c, cell))
                .unwrap_or(state.velocity);
            let len = (STEP / f32::from(cell.div.max(1))).max(0.05);
            region
                .notes
                .push(rack_note(state.place(c, cell), len, pitch, v));
            region.notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        }
        (None, false) => {}
    }
}

/// Turns step `k` of channel `c` on or off in `region` (on: a hit where the channel's feel
/// plays the step, when the step has none; off: every hit of the step).
pub fn set_step(region: &mut Region, state: &RackState, c: usize, k: usize, on: bool) {
    let here: Vec<Cell> = hits_of(&region.notes, state, c, k + 1)
        .into_iter()
        .filter(|h| h.0.step == k)
        .map(|h| h.0)
        .collect();
    if on {
        if here.is_empty() {
            set_hit(region, state, c, Cell::on(k), true, None);
        }
    } else {
        for cell in here {
            set_hit(region, state, c, cell, false, None);
        }
    }
}

/// Splits step `k` of channel `c` into `div` hits (1 to 4: one hit, a 32nd ratchet, a triplet,
/// 64ths), each at the loudest velocity the step had (the rack's when it had none).
pub fn divide_step(region: &mut Region, state: &RackState, c: usize, k: usize, div: u8) {
    let div = div.clamp(1, rack_groove::MAX_DIV);
    let here: Vec<(Cell, u8, usize)> = hits_of(&region.notes, state, c, k + 1)
        .into_iter()
        .filter(|h| h.0.step == k)
        .collect();
    let vel = here.iter().map(|h| h.1).max().unwrap_or(state.velocity);
    for h in &here {
        set_hit(region, state, c, h.0, false, None);
    }
    for j in 0..div {
        set_hit(region, state, c, Cell::new(k, j, div), true, Some(vel));
    }
}

/// What a step shows: how many parts (the division its hits need) and, for each part, the
/// velocity of the hit there (None: off).
pub fn step_parts(hits: &[(Cell, u8, usize)], k: usize) -> Vec<Option<u8>> {
    let div = hits
        .iter()
        .filter(|h| h.0.step == k)
        .fold(1u8, |d, h| rack_groove::lcm(d, h.0.div));
    let mut parts = vec![None; usize::from(div)];
    for h in hits.iter().filter(|h| h.0.step == k) {
        let at = usize::from(h.0.sub) * usize::from(div) / usize::from(h.0.div);
        parts[at] = Some(h.1);
    }
    parts
}

/// The cell part `i` of a step shown in `parts` parts is, when a division of 1 to 4 can hold it.
pub fn part_cell(k: usize, i: usize, parts: usize) -> Option<Cell> {
    let parts = parts.max(1);
    let cell = Cell::nearest(k, i as f64 / parts as f64);
    (cell.step == k && (cell.frac() * parts as f64 - i as f64).abs() < 1e-9).then_some(cell)
}

/// Sets the velocity of channel `c`'s hit at `cell` in `region`; when it is a hit of the loaded
/// pattern, the pattern keeps the velocity it now stands for. Returns the rack with that change
/// (to commit) when the pattern's velocity changed.
pub fn set_hit_velocity(
    region: &mut Region,
    state: &RackState,
    c: usize,
    cell: Cell,
    vel: u8,
) -> Option<RackState> {
    let i = note_at(&region.notes, state, c, cell)?;
    let vel = vel.clamp(1, 127);
    region.notes[i].vel = vel;
    if state.velocity_amount <= 0.0 {
        return None;
    }
    let mut new = state.clone();
    let hit = new.pattern.velocity_at_mut(c, cell)?;
    let src = unscaled_velocity(vel, hit.base, state.velocity_amount);
    if hit.vel == src {
        return None;
    }
    hit.vel = src;
    Some(new)
}

/// A new instrument track playing a new Channel Rack, selected.
fn new_rack_track(p: &mut Project) {
    let i = p.add_track(TrackKind::Instrument);
    p.tracks[i].name = tr("Channel Rack").to_string();
    p.tracks[i].au_instrument = Some(new_slot());
    p.selected_track = i;
}

/// Loads a kit into a rack: channel `k` takes `pads[k]` (`None`: an empty channel), named for
/// its file, with class `classes[k]` (left out: the class of its file's name); each channel
/// keeps its level, pan and mute, and the steps stay where they are, so a pattern plays the
/// new kit (each step moved to its channel's new class's feel). `track`: the rack's track, or
/// `None` for a new rack track; with no pads, a new rack track keeps what a new rack holds.
/// Returns the track.
pub fn load_kit(
    p: &mut Project,
    track: Option<usize>,
    kit: &str,
    pads: &[Option<std::path::PathBuf>],
    classes: &[DrumClass],
) -> usize {
    let ti = match track.filter(|&i| rack_state(p, i).is_some()) {
        Some(i) => i,
        None => {
            new_rack_track(p);
            let i = p.selected_track;
            if pads.is_empty() {
                return i;
            }
            if !kit.is_empty() {
                p.tracks[i].name = kit.to_string();
            }
            i
        }
    };
    let Some(old) = rack_state(p, ti) else {
        return ti;
    };
    let channels: Vec<RackChannel> = pads
        .iter()
        .take(MAX_RACK_CHANNELS)
        .enumerate()
        .map(|(k, pad)| {
            let mut c = match pad {
                Some(path) => channel_from(path),
                None => RackChannel::empty(layout_class(k)),
            };
            if let Some(class) = classes.get(k) {
                c.class = *class;
            }
            if let Some(o) = old.channels.get(k) {
                c.gain_db = o.gain_db;
                c.pan = o.pan;
                c.mute = o.mute;
                c.solo = o.solo;
            }
            c
        })
        .collect();
    let new = RackState {
        channels,
        kit: kit.to_string(),
        kit_dir: String::new(),
        ..old.clone()
    };
    commit(p, ti, &old, new);
    ti
}

/// Names the folder the rack of track `ti` was loaded from (its Samples menus list that
/// folder's samples).
pub fn set_kit_dir(p: &mut Project, ti: usize, dir: &std::path::Path) {
    let Some(old) = rack_state(p, ti) else {
        return;
    };
    let new = RackState {
        kit_dir: dir.to_string_lossy().into_owned(),
        ..old.clone()
    };
    if new != old {
        commit(p, ti, &old, new);
    }
}

/// The folder of the kit on a rack: the one it was loaded from, else the folder all its
/// channels' samples are in (None: no sample, or nothing in common but the disk's root).
pub fn kit_folder(st: &RackState) -> Option<std::path::PathBuf> {
    if !st.kit_dir.is_empty() {
        return Some(std::path::PathBuf::from(&st.kit_dir));
    }
    let mut common: Option<std::path::PathBuf> = None;
    for c in st.channels.iter().filter(|c| !c.path.is_empty()) {
        let parent = std::path::Path::new(&c.path).parent()?.to_path_buf();
        common = Some(match common {
            None => parent,
            Some(prev) => {
                let mut a = prev.as_path();
                while !parent.starts_with(a) {
                    a = a.parent()?;
                }
                a.to_path_buf()
            }
        });
    }
    // A folder this near the disk's root is not one kit.
    common.filter(|d| d.components().count() > 3)
}

/// A sample of a kit as a channel's menu lists it: its name and file.
pub type SampleRow = (String, std::path::PathBuf);

/// The samples of the rack's kit folder for channel `c`'s menu: those of its class first,
/// then every other one (any sample can go on any channel), as (name, file).
pub fn channel_samples(
    kit: &crate::kit_files::Kit,
    class: DrumClass,
) -> (Vec<SampleRow>, Vec<SampleRow>) {
    let mut mine = Vec::new();
    let mut rest = Vec::new();
    for (i, s) in kit.samples.iter().enumerate() {
        if s.class == DrumClass::SoundFont {
            continue;
        }
        let row = (s.name.clone(), kit.path_of(i));
        if s.class == class {
            mine.push(row);
        } else {
            rest.push(row);
        }
    }
    (mine, rest)
}

/// The next (`step` 1) or previous (-1) sample of channel `c`'s class in the rack's kit
/// folder after the one it plays (the first when it plays none of them), wrapping round.
pub fn cycle_sample(
    kit: &crate::kit_files::Kit,
    ch: &RackChannel,
    step: i32,
) -> Option<std::path::PathBuf> {
    let (mine, _) = channel_samples(kit, ch.class);
    if mine.is_empty() {
        return None;
    }
    let n = mine.len() as i32;
    let at = mine
        .iter()
        .position(|(_, p)| p.to_string_lossy() == ch.path)
        .map_or(if step > 0 { -1 } else { 0 }, |i| i as i32);
    Some(mine[(at + step).rem_euclid(n) as usize].1.clone())
}

/// A hit of a pattern: its class, its cell, its velocity and where it was played, in quarter
/// notes from the cell's straight place (None: no time of its own; the groove places it).
pub type PatternHitOf = (DrumClass, Cell, u8, Option<f32>);

/// A pitched note of a pattern's bass line (an 808): its start and length in quarter notes from
/// the pattern's start (as played), its MIDI note and velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BassNote {
    pub start: f32,
    pub len: f32,
    pub pitch: u8,
    pub vel: u8,
}

/// A pattern to load into a rack: hits by drum class, each at a cell with a velocity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RackPattern {
    pub name: String,
    /// Where it came from (a library's short name).
    pub source: String,
    /// How many steps (sixteenths) long. Up to [`MAX_STEPS`] it is one region the step grid
    /// shows; a longer one is loaded as regions of [`MAX_STEPS`] one after the other (see
    /// [`load_pattern`]).
    pub steps: usize,
    pub hits: Vec<PatternHitOf>,
    /// The tempo it was played at, when known.
    pub bpm: Option<f64>,
    /// Its tempo changes as (quarter notes from its start, bpm), the first at 0 (empty: `bpm`
    /// throughout). A pattern of a song of several tempos has one at each change.
    pub tempo_map: Vec<(f64, f64)>,
    /// Its bass line (an 808), played on the 808 track beside the rack (empty: none).
    pub bass: Vec<BassNote>,
    /// The one-shot the 808 track plays the bass line with, and its fundamental in Hz (None:
    /// the 808's synthesized voice).
    pub bass_sample: Option<(std::path::PathBuf, f32)>,
    /// The 808 track keeps the one-shot it has (a kit's 808 picked before), `bass_sample`
    /// only when it has none: the pattern is loaded without its kit.
    pub keep_bass_sample: bool,
    /// The song's folder (empty: none) and the song's bar (from 1) the pattern starts at.
    pub dir: String,
    pub bar: usize,
}

/// The project's tempo when a pattern is loaded: the tempo the pattern was played at (the
/// song's), or the session's as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PatternTempo {
    #[default]
    Song,
    Session,
}

/// A kit to load with a pattern: its name, the sample of each channel and their classes (see
/// [`load_kit`]).
pub struct KitLoad<'a> {
    pub name: &'a str,
    pub pads: &'a [Option<std::path::PathBuf>],
    pub classes: &'a [DrumClass],
}

/// What [`load_pattern`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatternLoaded {
    pub track: usize,
    pub region: usize,
    /// Hits put in the region.
    pub hits: usize,
    /// Hits of a class no channel had, when the rack had no room for another channel.
    pub dropped: usize,
    /// Channels added for classes the rack had none of (empty: a sample is still to be put).
    pub added: usize,
    /// Hits of a class the pattern's kit has no sample of (left out, not given an empty channel).
    pub no_sample: usize,
    /// The project's tempo set to the pattern's (Song BPM), when it changed.
    pub tempo: Option<f32>,
    /// Tempo changes written inside the pattern (Song BPM, a song of several tempos).
    pub tempo_changes: usize,
    /// Regions the pattern was put in (more than one: longer than the step grid).
    pub regions: usize,
    /// The 808 track and the notes put on it, when the pattern has a bass line.
    pub bass_track: Option<usize>,
    pub bass_notes: usize,
}

/// The channel a pattern's hit of class `c` goes on: the first of that class, else of a class
/// that stands in for it, else a new empty channel of that class when there is room.
fn channel_for(st: &mut RackState, c: DrumClass, added: &mut usize) -> Option<usize> {
    if let Some(i) = st.channels.iter().position(|ch| ch.class == c) {
        return Some(i);
    }
    for alt in kit_classify::stand_ins(c) {
        if let Some(i) = st
            .channels
            .iter()
            .position(|ch| ch.class == *alt && !ch.path.is_empty())
        {
            return Some(i);
        }
    }
    if st.channels.len() >= MAX_RACK_CHANNELS {
        return None;
    }
    st.channels.push(RackChannel::empty(c));
    *added += 1;
    Some(st.channels.len() - 1)
}

/// Loads `pat` into the rack of `track` (`None`: a new rack track): with `kit`, the kit first
/// (as [`load_kit`]); every hit goes on the channel of its class ([`channel_for`]), in the
/// region the steps are in (`selected`, as [`step_region`]), which is made as long as the
/// pattern and loses the rack's notes it had. A hit with a played time is put exactly there
/// (the groove places only the hits without one, and steps added by hand later). With a kit,
/// a hit of a class the kit has no sample of is left out. The rack takes `groove` (its amounts
/// full), the pattern's velocities, and the velocity amount full. `tempo` Song sets the
/// project's tempo at the region's start to the pattern's. A bass line goes on the 808 track
/// ([`load_bass`]).
pub fn load_pattern(
    p: &mut Project,
    track: Option<usize>,
    selected: Option<(usize, usize)>,
    pat: &RackPattern,
    groove: Groove,
    kit: Option<KitLoad>,
    tempo: PatternTempo,
) -> PatternLoaded {
    let ti = match track.filter(|&i| rack_state(p, i).is_some()) {
        Some(i) => i,
        None => load_kit(p, None, "", &[], &[]),
    };
    let kit_classes: Option<Vec<DrumClass>> = kit.as_ref().map(|k| {
        k.classes
            .iter()
            .zip(k.pads.iter())
            .filter(|(_, pad)| pad.is_some())
            .map(|(c, _)| *c)
            .collect()
    });
    if let Some(k) = kit {
        load_kit(p, Some(ti), k.name, k.pads, k.classes);
    }
    let old = rack_state(p, ti).unwrap_or_default();
    let mut new = old.clone();
    let mut out = PatternLoaded {
        track: ti,
        ..PatternLoaded::default()
    };
    // A pattern longer than the step grid: its first MAX_STEPS steps are the loaded pattern
    // (the grid shows them), the rest go in regions of MAX_STEPS each after it.
    let total = pat.steps.clamp(1, MAX_PATTERN_STEPS);
    let steps = total.min(MAX_STEPS);
    let mut hits: Vec<PatternHit> = Vec::new();
    let mut later: Vec<PatternHit> = Vec::new();
    // DOUBLE_S in quarter notes at the pattern's tempo.
    let double_beats = DOUBLE_S * pat.bpm.filter(|b| *b > 0.0).unwrap_or(120.0) / 60.0;
    for (class, cell, vel, played) in &pat.hits {
        if cell.step >= total {
            continue;
        }
        // The pattern's own kit has no sample of the class: no empty channel is made for it.
        if let Some(have) = &kit_classes {
            if !have.contains(class) {
                out.no_sample += 1;
                continue;
            }
        }
        let Some(c) = channel_for(&mut new, *class, &mut out.added) else {
            out.dropped += 1;
            continue;
        };
        // Long: a hit played outside the first page (before the start, or past its end) is
        // placed by its time with the later pages'.
        let page0 = cell.step < steps
            && (total == steps
                || played.is_none_or(|d| {
                    let at = cell.straight() + f64::from(d);
                    (0.0..steps as f64 * rack_groove::STEP).contains(&at)
                }));
        // Two hits in one cell: one hit found twice (within DOUBLE_S of each other) plays
        // once, the louder at its own time; two hits played apart (a flam, a 32nd) both play,
        // the second as a note of its own beside the step grid's.
        let near = |h: &PatternHit| match (h.played, *played) {
            (Some(a), Some(b)) => f64::from((a - b).abs()) < double_beats,
            _ => true,
        };
        let here = |h: &PatternHit| h.channel == c && h.cell == *cell;
        let apart =
            hits.iter().any(|h| here(h) && !near(h)) && !hits.iter().any(|h| here(h) && near(h));
        let list = if page0 && !apart {
            &mut hits
        } else {
            &mut later
        };
        match list.iter_mut().find(|h| here(h) && near(h)) {
            Some(h) => {
                if *vel > h.vel {
                    h.vel = *vel;
                    h.played = *played;
                }
            }
            None => list.push(PatternHit {
                channel: c,
                cell: *cell,
                vel: (*vel).clamp(1, 127),
                base: new.velocity,
                played: *played,
            }),
        }
    }
    hits.sort_by_key(|h| (h.channel, h.cell));
    new.groove = Groove {
        swing_amount: 1.0,
        offset_amount: 1.0,
        ..groove
    };
    new.velocity_amount = 1.0;
    new.pattern = LoadedPattern {
        name: pat.name.clone(),
        source: pat.source.clone(),
        steps,
        hits: hits.clone(),
        dir: pat.dir.clone(),
        bar: pat.bar,
    };
    // The region: as long as the pattern's first page, without the rack's notes it had.
    let ri = step_region(p, ti, selected);
    let top = RACK_BASE_NOTE + MAX_RACK_CHANNELS as u8;
    let start = p.region_start_beats(&p.tracks[ti].regions[ri]);
    let page_beats = steps as f64 * rack_groove::STEP;
    {
        let bar0 = p.beats_to_bar(start);
        let end = p.beats_to_bar(start + page_beats);
        let r = &mut p.tracks[ti].regions[ri];
        r.len_bars = (end - bar0) as f32;
        r.notes
            .retain(|n| !(RACK_BASE_NOTE..top).contains(&n.pitch));
    }
    commit(p, ti, &old, new.clone());
    let region = &mut p.tracks[ti].regions[ri];
    for h in &hits {
        set_hit(region, &new, h.channel, h.cell, true, Some(h.vel));
    }
    // The later pages: a region each (one there already starting on the page's bar is used,
    // without the rack's notes it had), every hit at its played place from the pattern's start
    // in the region its time falls in. Short regions keep each note's place exact (a note's
    // place in a region is an f32).
    let pages = total.div_ceil(steps);
    let span = total as f64 * rack_groove::STEP;
    let mut page_region = vec![ri];
    for k in 1..pages {
        let at = start + k as f64 * page_beats;
        let bar = (p.beats_to_bar(at) + 1.0) as f32;
        let to = (at + page_beats).min(start + span);
        let len = (p.beats_to_bar(to) - p.beats_to_bar(at)) as f32;
        let found = p.tracks[ti]
            .regions
            .iter()
            .position(|r| is_plain(r) && (r.start_bar - bar).abs() < 1e-4);
        let i = match found {
            Some(i) => {
                let r = &mut p.tracks[ti].regions[i];
                r.len_bars = len;
                r.notes
                    .retain(|n| !(RACK_BASE_NOTE..top).contains(&n.pitch));
                i
            }
            None => {
                let name = p.tracks[ti].name.clone();
                p.tracks[ti].regions.push(Region {
                    missing: None,
                    name,
                    start_bar: bar,
                    len_bars: len,
                    notes: Vec::new(),
                    controls: Vec::new(),
                    folder: None,
                    generated: None,
                    audio: None,
                    params: RegionParams::default(),
                });
                p.tracks[ti].regions.len() - 1
            }
        };
        page_region.push(i);
    }
    for h in &later {
        let local = Cell {
            step: h.cell.step % steps,
            ..h.cell
        };
        let dev = match h.played {
            Some(d) => f64::from(d),
            None => f64::from(new.feel(h.channel).place(local)) - local.straight(),
        };
        let mut at = h.cell.straight() + dev;
        if at < 0.0 {
            // Played before the pattern's start: at its end, as a looped pattern plays it.
            at += span;
        }
        let k = ((at / page_beats).floor().max(0.0) as usize).min(pages - 1);
        let len = (STEP / f32::from(h.cell.div.max(1))).max(0.05);
        let vel = scaled_velocity(h.vel, h.base, new.velocity_amount);
        let pitch = RACK_BASE_NOTE + h.channel as u8;
        let r = &mut p.tracks[ti].regions[page_region[k]];
        r.notes.push(rack_note(
            (at - k as f64 * page_beats) as f32,
            len,
            pitch,
            vel,
        ));
    }
    for &i in &page_region[1..] {
        p.tracks[ti].regions[i]
            .notes
            .sort_by(|a, b| a.start.total_cmp(&b.start));
    }
    p.tracks[ti].regions[ri]
        .notes
        .sort_by(|a, b| a.start.total_cmp(&b.start));
    out.hits = hits.len() + later.len();
    out.region = ri;
    out.regions = page_region.len();
    if tempo == PatternTempo::Song {
        let map: Vec<(f64, f64)> = if pat.tempo_map.is_empty() {
            pat.bpm.map(|b| vec![(0.0, b)]).unwrap_or_default()
        } else {
            pat.tempo_map.clone()
        };
        let map: Vec<(f64, f32)> = map
            .into_iter()
            .filter(|(b, bpm)| b.is_finite() && bpm.is_finite() && *bpm > 0.0 && *b < span)
            .map(|(b, bpm)| {
                (
                    b,
                    (bpm as f32).clamp(crate::model::MIN_BPM, crate::model::MAX_BPM),
                )
            })
            .collect();
        if let Some(&(_, first)) = map.first() {
            // The pattern's tempo changes replace any inside its span.
            p.tempo_changes
                .retain(|t| !(t.beat > start + 1e-6 && t.beat < start + span - 1e-6));
            if (p.tempo_at(start) - first).abs() > 1e-4 {
                p.set_tempo_at(start, first);
            }
            for &(b, bpm) in &map[1..] {
                if b > 1e-6 {
                    p.add_tempo_change(start + b, bpm);
                }
            }
            out.tempo = Some(p.tempo_at(start));
            out.tempo_changes = map.len() - 1;
        }
    }
    let (start_bar, len_bars) = {
        let r = &p.tracks[ti].regions[ri];
        (
            r.start_bar,
            (p.beats_to_bar(start + span) - p.beats_to_bar(start)) as f32,
        )
    };
    let (bt, bn) = load_bass(p, start_bar, len_bars, pat);
    out.bass_track = bt;
    out.bass_notes = bn;
    p.song_rev += 1;
    out
}

/// The name of the track that plays a pattern's bass line beside the rack.
pub const BASS_TRACK: &str = "808";

/// Puts `pat`'s bass line on the 808 track (an instrument track
/// named [`BASS_TRACK`] playing the 808, made after the rack's track when there is none), in
/// its first plain region, made to start and end with the rack's region: the region loses the
/// notes it had. With no bass line, an 808 track there is already is emptied, so it does not
/// play the last pattern's line under this one. Returns the track and the notes put.
fn load_bass(
    p: &mut Project,
    start_bar: f32,
    len_bars: f32,
    pat: &RackPattern,
) -> (Option<usize>, usize) {
    let found = p.tracks.iter().position(|t| {
        t.name == BASS_TRACK && t.au_instrument.as_ref().is_some_and(crate::bass808::is_808)
    });
    let bi = match found {
        Some(i) => i,
        None if pat.bass.is_empty() => return (None, 0),
        None => {
            let keep = p.selected_track;
            let i = p.add_track(TrackKind::Instrument);
            p.tracks[i].name = BASS_TRACK.to_string();
            p.tracks[i].au_instrument = Some(crate::bass808::new_slot());
            // add_track appends and selects the new track: the rack's stays selected.
            p.selected_track = keep;
            i
        }
    };
    if let Some(slot) = p.tracks[bi].au_instrument.as_mut() {
        let mut st = crate::bass808::decode(&slot.state);
        let keep = pat.keep_bass_sample && !st.sample.is_empty();
        match &pat.bass_sample {
            _ if keep => {}
            Some((path, root)) => {
                st.sample = path.to_string_lossy().into_owned();
                st.root_hz = *root;
                // A one-shot plays as long as its note (an 808 held for its length).
                st.params.sustain = true;
            }
            None => {
                st.sample.clear();
            }
        }
        slot.state = Some(crate::bass808::encode(&st));
    }
    let ri = match p.tracks[bi].regions.iter().position(is_plain) {
        Some(r) => r,
        None => {
            let name = p.tracks[bi].name.clone();
            p.tracks[bi].regions.push(Region {
                missing: None,
                name,
                start_bar,
                len_bars,
                notes: Vec::new(),
                controls: Vec::new(),
                folder: None,
                generated: None,
                audio: None,
                params: RegionParams::default(),
            });
            p.tracks[bi].regions.len() - 1
        }
    };
    let r = &mut p.tracks[bi].regions[ri];
    r.start_bar = start_bar;
    r.len_bars = len_bars;
    r.notes.clear();
    let end = pat.steps.clamp(1, MAX_PATTERN_STEPS) as f32 * STEP;
    for n in &pat.bass {
        if n.start < 0.0 || n.start >= end {
            continue;
        }
        let len = n.len.min(end - n.start).max(0.01);
        r.notes
            .push(rack_note(n.start, len, n.pitch.min(127), n.vel));
    }
    r.notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    let put = r.notes.len();
    p.song_rev += 1;
    (Some(bi), put)
}

/// Puts one sample on channel `channel` (from 0) of the rack on track `ti`, adding empty
/// channels up to it; the channel keeps its level, pan, mute and class (a channel added takes
/// its sample's class). False when the track has no rack or the channel is past the last one a
/// rack can have.
pub fn assign_sample(p: &mut Project, ti: usize, channel: usize, path: &std::path::Path) -> bool {
    let Some(old) = rack_state(p, ti) else {
        return false;
    };
    if channel >= MAX_RACK_CHANNELS {
        return false;
    }
    let mut new = old.clone();
    while new.channels.len() <= channel {
        let k = new.channels.len();
        new.channels.push(RackChannel::empty(layout_class(k)));
    }
    let fresh = channel_from(path);
    if channel >= old.channels.len() || new.channels[channel].path.is_empty() {
        new.channels[channel].class = fresh.class;
    }
    new.channels[channel].path = fresh.path;
    new.channels[channel].name = fresh.name;
    commit(p, ti, &old, new);
    true
}

/// The kit in folder `dir`, read once and kept (the last few read) for the channels' menus.
fn kit_samples(dir: &std::path::Path) -> std::sync::Arc<crate::kit_files::Kit> {
    type Kept = Vec<(std::path::PathBuf, std::sync::Arc<crate::kit_files::Kit>)>;
    static KEPT: std::sync::Mutex<Kept> = std::sync::Mutex::new(Vec::new());
    let Ok(mut kept) = KEPT.lock() else {
        return std::sync::Arc::new(crate::kit_files::read_kit(dir));
    };
    if let Some((_, k)) = kept.iter().find(|(d, _)| d == dir) {
        return k.clone();
    }
    let k = std::sync::Arc::new(crate::kit_files::read_kit(dir));
    kept.push((dir.to_path_buf(), k.clone()));
    if kept.len() > 8 {
        kept.remove(0);
    }
    k
}

/// A sample dragged from the Kit Browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleDrag(pub std::path::PathBuf);

/// Height of a rack row, the narrowest and widest step pad, and the gap between pads.
const ROW: f32 = 30.0;
const PAD_MIN: f32 = 22.0;
const PAD_MAX: f32 = 64.0;
const PAD_GAP: f32 = 3.0;
/// Extra space between groups of four steps.
const GROUP_GAP: f32 = 6.0;
/// The columns before the steps: Mute and Solo, the sample, the volume knob and its readout, the pan
/// knob, the class, its swing, its offset amount, and their gaps.
const MUTE_W: f32 = 58.0;
const SAMPLE_W: f32 = 150.0;
const KNOB_W: f32 = 26.0;
const GAIN_W: f32 = 64.0;
const CLASS_W: f32 = 78.0;
const SWING_W: f32 = 70.0;
const OFFSET_W: f32 = 62.0;
const VOL_X: f32 = MUTE_W + 5.0 + SAMPLE_W + 5.0;
const GAIN_X: f32 = VOL_X + KNOB_W + 2.0;
const PAN_X: f32 = GAIN_X + GAIN_W + 5.0;
const CLASS_X: f32 = PAN_X + KNOB_W + 5.0;
const SWING_X: f32 = CLASS_X + CLASS_W + 5.0;
const OFFSET_X: f32 = SWING_X + SWING_W + 5.0;
const STEPS_X: f32 = OFFSET_X + OFFSET_W + 12.0;
/// The keys in the rack's head (Kits, Groove and, private build, Pattern), and the slot beside
/// them that names the loaded kit.
const KITS_W: f32 = 76.0;
const GROOVE_W: f32 = 88.0;
const KIT_NAME_W: f32 = 240.0;
/// Height of the step ruler (numbers and the playhead LEDs) over the steps.
const RULER_H: f32 = 30.0;
/// Height of the rack's head: the amounts on the left, the keys on the right, in one row.
const AMOUNTS_H: f32 = 30.0;
/// Widths in the head that give way when the row is short: the loaded pattern's or groove's
/// name on the left and the kit's name beside the Kits key.
const TITLE_W: f32 = 220.0;
/// The slot that says a kit is being copied here.
const COPYING_W: f32 = 110.0;
/// The Add Channel key under the rows, and its row with the gap over it.
const ADD_KEY_H: f32 = 28.0;
const ADD_ROW_H: f32 = ADD_KEY_H + 6.0;
/// The rack's margins: left and right, top and bottom.
const MARGIN_X: i8 = 14;
const MARGIN_Y: i8 = 10;
/// Height of the keys in the head, and the gap under the head when no track plays a rack.
const KEY_H: f32 = 26.0;
const HEAD_GAP: f32 = 8.0;
/// The gap between the amounts and the step ruler.
const AMOUNTS_GAP: f32 = 4.0;

/// How tall the rack is with every channel's row shown in a pane `width` points wide, with
/// `gap` points between the items laid one under another (egui's item spacing); `None` when
/// the selected track plays no rack.
pub fn full_height(p: &Project, gap: f32, width: f32) -> Option<f32> {
    let r = rack_state(p, rack_track(p)?)?;
    let n = r.channels.len();
    let head = if head_layout(width - 2.0 * f32::from(MARGIN_X), Some(&r)).two_rows {
        2.0 * AMOUNTS_H + gap
    } else {
        AMOUNTS_H
    };
    let add = if n < MAX_RACK_CHANNELS {
        ADD_ROW_H + gap
    } else {
        0.0
    };
    Some(2.0 * f32::from(MARGIN_Y) + head + AMOUNTS_GAP + RULER_H + n as f32 * ROW + add + gap)
}

/// A channel's level, as its knob turns.
const GAIN_MIN: f32 = -48.0;
const GAIN_MAX: f32 = 12.0;
/// The velocities the Velocity menu offers.
const VELOCITIES: [u8; 9] = [127, 120, 110, 100, 90, 80, 64, 48, 32];

/// A level in dB as a knob's value (-1 at -48 dB, 1 at +12 dB, linear in dB).
fn gain_to_knob(db: f32) -> f32 {
    ((db - GAIN_MIN) / (GAIN_MAX - GAIN_MIN) * 2.0 - 1.0).clamp(-1.0, 1.0)
}

/// The level a knob's value stands for (see [`gain_to_knob`]), to a tenth of a dB.
fn knob_to_gain(v: f32) -> f32 {
    let db = GAIN_MIN + (v.clamp(-1.0, 1.0) + 1.0) / 2.0 * (GAIN_MAX - GAIN_MIN);
    (db * 10.0).round() / 10.0
}

/// A pan as the readout shows it: C, or L or R and a percentage.
fn pan_text(pan: f32) -> String {
    let pct = (pan * 100.0).round() as i32;
    match pct {
        0 => "C".into(),
        p if p < 0 => format!("L{}", -p),
        p => format!("R{p}"),
    }
}

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

/// A hit's velocity as an ink bar along the foot of `r`, as long as the velocity.
fn velocity_bar(g: &egui::Painter, r: egui::Rect, vel: u8) {
    let w = (r.width() - 4.0).max(1.0) * f32::from(vel.min(127)) / 127.0;
    let bar = egui::Rect::from_min_size(egui::pos2(r.left() + 2.0, r.bottom() - 5.0), vec2(w, 3.0));
    g.rect_filled(bar, 1.0, theme::INK());
}

/// One step pad in `rect`: a bone key, a shade darker in every other group of four (as the
/// beats are told apart). `parts` are the step's hits (one part: the whole step), each with
/// its hit's velocity (None: off): a whole step that plays is a signal orange key; a split step
/// shows a cell per part, orange where a hit plays (a part no division of 1 to 4 can hold is
/// dark). A hit carries an ink bar as long as its velocity. A beat's first step has an ink
/// line along the top of its cap.
fn step_pad(
    ui: &egui::Ui,
    g: &egui::Painter,
    rect: egui::Rect,
    k: usize,
    parts: &[Option<u8>],
    id: egui::Id,
) -> egui::Response {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let whole = parts.len() <= 1;
    let lit = whole && parts.first().copied().flatten().is_some();
    let cap = if lit {
        crate::hw::Cap::Orange
    } else {
        crate::hw::Cap::Bone
    };
    let face = crate::hw::paint_key(g, rect, cap, false, resp.hovered());
    if !lit && !(k / 4).is_multiple_of(2) {
        g.rect_filled(face.shrink(1.0), 3.0, Color32::from_black_alpha(22));
    }
    if k.is_multiple_of(4) {
        g.hline(
            face.left() + 4.0..=face.right() - 4.0,
            face.top() + 2.5,
            egui::Stroke::new(1.0, theme::INK2()),
        );
    }
    if whole {
        if let Some(Some(v)) = parts.first() {
            velocity_bar(g, face, *v);
        }
        return resp;
    }
    let n = parts.len();
    let inner = face.shrink2(vec2(2.0, 5.0));
    let w = inner.width() / n as f32;
    for (i, part) in parts.iter().enumerate() {
        let r = egui::Rect::from_min_size(
            egui::pos2(inner.left() + i as f32 * w, inner.top()),
            vec2(w, inner.height()),
        )
        .shrink2(vec2(0.5, 0.0));
        if part_cell(k, i, n).is_none() {
            g.rect_filled(r, 1.5, Color32::from_black_alpha(60));
            continue;
        }
        match part {
            Some(v) => {
                g.rect_filled(r, 1.5, theme::ORANGE());
                velocity_bar(g, r, *v);
            }
            None => {
                g.rect_stroke(
                    r,
                    1.5,
                    egui::Stroke::new(1.0, theme::INK2().gamma_multiply(0.5)),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
    resp
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
                theme::INK2(),
            );
        }
        crate::hw::led_sized(g, egui::pos2(cx, top + 21.0), 2.5, playing == Some(k));
    }
}

/// The Groove menu: the groove loaded, Straight, and (private build) the grooves of the
/// libraries. Returns the groove picked.
fn groove_menu(ui: &mut egui::Ui, now: &Groove) -> Option<Groove> {
    let mut out = None;
    ui.set_min_width(220.0);
    if !now.name.is_empty() {
        ui.label(
            egui::RichText::new(&now.name)
                .font(theme::font(theme::Face::Bold, 12.0))
                .color(theme::INK()),
        );
        ui.separator();
    }
    if ui.button(tr("Straight")).clicked() {
        out = Some(Groove::default());
        ui.close();
    }
    out
}

/// A knob for an amount from 0 to 100 % in `rect`: its label in silkscreen, the knob, and the
/// percentage in an LCD readout. Returns the new amount while it is turned (double-click: 100 %).
fn amount_knob(
    ui: &egui::Ui,
    g: &egui::Painter,
    rect: egui::Rect,
    id: egui::Id,
    label: &str,
    amount: f32,
) -> Option<f32> {
    let lw = crate::hw::silk_width(g, label, 10.0);
    crate::hw::silk(
        g,
        egui::pos2(rect.left(), rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        10.0,
        theme::INK2(),
    );
    let c = egui::pos2(rect.left() + lw + 16.0, rect.center().y);
    let (_, v) = crate::hw::knob(ui, g, c, 9.0, id, amount * 2.0 - 1.0, 1.0);
    let now = v.map_or(amount, |v| (v + 1.0) / 2.0);
    let lcd = egui::Rect::from_min_max(
        egui::pos2(c.x + 16.0, rect.top() + 2.0),
        egui::pos2(rect.right(), rect.bottom() - 2.0),
    );
    crate::hw::lcd_readout(g, lcd, "%", &format!("{:.0}", now * 100.0));
    v.map(|_| (now * 100.0).round() / 100.0)
}

/// Width of each of the Groove and Offset amounts: wider for a pattern played as recorded,
/// whose labels say they place only the steps added by hand.
fn amount_w(added: bool) -> f32 {
    if added {
        230.0
    } else {
        140.0
    }
}

/// Width of the amounts in the head without the pattern's name: Groove, Offset, Velocity and
/// the VEL field.
fn amounts_w(added: bool) -> f32 {
    2.0 * (amount_w(added) + 10.0) + 170.0 + 86.0
}

/// Width of the Split to Tracks key.
fn split_w() -> f32 {
    tr("Split to Tracks").chars().count() as f32 * 8.0 + 28.0
}

/// Width of the head's keys without the kit's name: Kits and, with a rack, Groove, Pattern
/// (private build) and Split to Tracks, 6 points apart.
fn keys_w(rack: bool) -> f32 {
    if !rack {
        return KITS_W;
    }
    #[allow(unused_mut)]
    let mut w = KITS_W + 6.0 + GROOVE_W + 6.0 + split_w();
    w
}

/// How the rack's head is laid out in `width` points: the amounts and the keys in one row when
/// they fit side by side, else the keys over the amounts; and the widths of the pattern's (or
/// groove's) name and of the kit's name, which give way when the row is short.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HeadLayout {
    two_rows: bool,
    title_w: f32,
    name_w: f32,
}

fn head_layout(width: f32, rack: Option<&RackState>) -> HeadLayout {
    let Some(r) = rack else {
        return HeadLayout {
            two_rows: false,
            title_w: 0.0,
            name_w: 0.0,
        };
    };
    let keys = keys_w(true);
    let left = amounts_w(r.pattern.is_played());
    let titled = !r.pattern.name.is_empty() || !r.groove.name.is_empty();
    let named = !r.kit.is_empty();
    let gap = |on: bool, w: f32| if on { w } else { 0.0 };
    if width >= left + 12.0 + keys {
        let room = width - left - 12.0 - keys - gap(titled, 14.0) - gap(named, 6.0);
        let (title_w, name_w) = give_way(room, titled, named);
        HeadLayout {
            two_rows: false,
            title_w,
            name_w,
        }
    } else {
        HeadLayout {
            two_rows: true,
            title_w: give_way(width - left - 14.0, titled, false).0,
            name_w: give_way(width - keys - 6.0, false, named).1,
        }
    }
}

/// A name slot narrower than this is left out.
const MIN_SLOT_W: f32 = 48.0;

/// The widths of the pattern's name and the kit's name (each when shown) in `room` points:
/// each its whole width when both fit, else an even share; one narrower than `MIN_SLOT_W` is
/// left out (0).
fn give_way(room: f32, title: bool, name: bool) -> (f32, f32) {
    let want = [
        if title { TITLE_W } else { 0.0 },
        if name { KIT_NAME_W } else { 0.0 },
    ];
    let n = f32::from(u8::from(title) + u8::from(name));
    let share = if want[0] + want[1] <= room || n == 0.0 {
        f32::MAX
    } else {
        (room / n).max(0.0)
    };
    let w = |v: f32| {
        let v = v.min(share);
        if v >= MIN_SLOT_W {
            v
        } else {
            0.0
        }
    };
    (w(want[0]), w(want[1]))
}

/// A change to a channel's steps, made once the rack has been drawn.
enum StepAct {
    /// The whole step on (a hit) or off (every hit).
    Step(usize, usize, bool),
    /// One hit on or off.
    Hit(usize, Cell, bool),
    /// One hit's velocity.
    Vel(usize, Cell, u8),
    /// Every hit of a step to one velocity.
    StepVel(usize, usize, u8),
    /// A step split into hits.
    Divide(usize, usize, u8),
}

/// The Channel Rack, in the editor pane under the tracks (Window > Channel Rack), where the
/// Piano Roll also goes.
pub fn show(ui: &mut egui::Ui, p: &mut Project, s: &mut crate::UiState) {
    let full = ui.available_rect_before_wrap();
    {
        let g = ui.painter_at(full);
        g.rect_filled(full, 0.0, theme::BG());
        crate::hw::seam_h(&g, full.x_range(), full.top());
    }
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: MARGIN_X,
            right: MARGIN_X,
            top: MARGIN_Y,
            bottom: MARGIN_Y,
        })
        .show(ui, |ui| {
            // The head: on the right the keys (Split to Tracks, Pattern, Groove, the kit its
            // channels came from, the Kit Browser), on the left the amounts, drawn below once
            // the rack's state is known; the amounts go under the keys when the row is short.
            let head = ui
                .allocate_exact_size(vec2(ui.available_width(), AMOUNTS_H), Sense::hover())
                .0;
            if crate::kit_browser::finish_copy(&mut s.kits, p).0 {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
            let rack = rack_track(p).and_then(|i| rack_state(p, i));
            let copying = crate::kit_browser::copying(&s.kits) || copying_default();
            let split_label = tr("Split to Tracks").to_uppercase();
            let split_w = split_w();
            let layout = head_layout(head.width(), rack.as_ref());
            let name_w = layout.name_w;
            let kit = if name_w > 0.0 {
                rack.as_ref().map(|r| r.kit.clone()).unwrap_or_default()
            } else {
                String::new()
            };
            let kits_label = tr("Kits").to_uppercase();
            let inset = (AMOUNTS_H - KEY_H) / 2.0;
            let key = egui::Rect::from_min_max(
                egui::pos2(head.right() - KITS_W, head.top() + inset),
                egui::pos2(head.right(), head.bottom() - inset),
            );
            let name = egui::Rect::from_min_max(
                egui::pos2(key.left() - 6.0 - name_w, key.top() + 1.0),
                egui::pos2(key.left() - 6.0, key.bottom() - 1.0),
            );
            let kit_left = if kit.is_empty() {
                key.left()
            } else {
                name.left()
            };
            let groove_key = egui::Rect::from_min_max(
                egui::pos2(kit_left - 6.0 - GROOVE_W, key.top()),
                egui::pos2(kit_left - 6.0, key.bottom()),
            );
            let keys_left = groove_key.left();
            // The Split to Tracks key, left of the others.
            let split_key = egui::Rect::from_min_max(
                egui::pos2(keys_left - 6.0 - split_w, key.top()),
                egui::pos2(keys_left - 6.0, key.bottom()),
            );
            let g = ui.painter().clone();
            let mut toggle = false;
            if !kit.is_empty() {
                let resp = ui.interact(name, egui::Id::unique("rack_kit_name"), Sense::click());
                crate::hw::paint_slot(
                    &g,
                    name,
                    crate::hw::SlotKind::Filled,
                    &kit,
                    Some(true),
                    false,
                    resp.hovered(),
                );
                toggle |= resp.clicked();
            }
            let (resp, _) = crate::hw::key(
                ui,
                &g,
                key,
                egui::Id::unique("rack_kits"),
                crate::hw::Cap::Bone,
                s.kits.open,
                Some(s.kits.open),
                crate::hw::Legend::Text(&kits_label),
            );
            toggle |= resp.clicked();
            if toggle {
                s.kits.open = !s.kits.open;
            }
            // The Groove key: lit while the steps are off the straight grid; its menu loads a
            // groove or goes back to straight.
            let mut picked_groove: Option<Groove> = None;
            if let Some(r) = &rack {
                let lit = !r.groove.is_straight();
                let label = tr("Groove").to_uppercase();
                let (resp, _) = crate::hw::key(
                    ui,
                    &g,
                    groove_key,
                    egui::Id::unique("rack_groove"),
                    crate::hw::Cap::Bone,
                    false,
                    Some(lit),
                    crate::hw::Legend::Text(&label),
                );
                let resp = if r.groove.name.is_empty() {
                    resp
                } else {
                    resp.on_hover_text(&r.groove.name)
                };
                egui::Popup::menu(&resp).show(|ui| {
                    picked_groove = groove_menu(ui, &r.groove);
                });
            }
            let mut split = false;
            if rack.is_some() {
                let (resp, _) = crate::hw::key(
                    ui,
                    &g,
                    split_key,
                    egui::Id::unique("rack_split"),
                    crate::hw::Cap::Bone,
                    false,
                    None,
                    crate::hw::Legend::Text(&split_label),
                );
                split = resp.clicked();
            }
            let Some(ti) = rack_track(p) else {
                ui.add_space(HEAD_GAP);
                ui.label(
                    egui::RichText::new(tr("Select a track that plays the Channel Rack."))
                        .font(theme::font(theme::Face::Body, 12.0))
                        .color(theme::INK2()),
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
            if let Some(g) = picked_groove {
                set_groove(p, ti, g);
            }
            if split && !crate::rack_split::split(p, ti).is_empty() {
                // Tracks were put under the rack's: remembered track numbers are stale.
                crate::tracks_area::forget_track_indexes(s);
                return;
            }
            let ri = step_region(p, ti, s.tracks.selected);
            let len_beats = p.region_len_beats(&p.tracks[ti].regions[ri]);
            let steps =
                ((len_beats / f64::from(STEP)).round() as usize).clamp(MIN_STEPS, MAX_STEPS);
            let Some(before) = rack_state(p, ti) else {
                return;
            };
            let mut state = before.clone();
            // The amounts, in the head left of its keys: the loaded pattern (or groove), how
            // much of its swing, offsets and velocities play, the velocity of a hit added by
            // hand, and the copying of a kit.
            {
                let (row, right) = if layout.two_rows {
                    let row = ui
                        .allocate_exact_size(vec2(ui.available_width(), AMOUNTS_H), Sense::hover())
                        .0;
                    (row, row.right())
                } else {
                    (head, split_key.left() - 12.0)
                };
                let g = ui.painter().clone();
                let mut x = row.left();
                let title = if state.pattern.name.is_empty() {
                    state.groove.name.clone()
                } else {
                    state.pattern.name.clone()
                };
                // The room left of the keys (the kit's name took its share of it).
                let title_w = TITLE_W.min(right - 14.0 - x - amounts_w(state.pattern.is_played()));
                if !title.is_empty() && title_w >= MIN_SLOT_W {
                    let r = egui::Rect::from_min_size(
                        egui::pos2(x, row.top() + 2.0),
                        vec2(title_w, AMOUNTS_H - 4.0),
                    );
                    crate::hw::paint_slot(
                        &g,
                        r,
                        crate::hw::SlotKind::Filled,
                        &title,
                        Some(true),
                        false,
                        false,
                    );
                    x = r.right() + 14.0;
                }
                let knob = |x: f32, w: f32| {
                    egui::Rect::from_min_size(egui::pos2(x, row.top()), vec2(w, AMOUNTS_H))
                };
                // A pattern played as recorded: its hits keep their own times, and the groove's
                // swing and offsets (each class's averages) place only steps added by hand.
                let added = state.pattern.is_played();
                let kw = amount_w(added);
                let groove_label = if added {
                    tr("Groove (Added Steps)").to_uppercase()
                } else {
                    tr("Groove").to_uppercase()
                };
                if let Some(a) = amount_knob(
                    ui,
                    &g,
                    knob(x, kw),
                    egui::Id::unique("rack_amt_swing"),
                    &groove_label,
                    state.groove.swing_amount,
                ) {
                    state.groove.swing_amount = a;
                }
                x += kw + 10.0;
                let offset_label = if added {
                    tr("Offset (Added Steps)").to_uppercase()
                } else {
                    tr("Offset").to_uppercase()
                };
                if let Some(a) = amount_knob(
                    ui,
                    &g,
                    knob(x, kw),
                    egui::Id::unique("rack_amt_offset"),
                    &offset_label,
                    state.groove.offset_amount,
                ) {
                    state.groove.offset_amount = a;
                }
                x += kw + 10.0;
                let vel_label = tr("Velocity").to_uppercase();
                if let Some(a) = amount_knob(
                    ui,
                    &g,
                    knob(x, 160.0),
                    egui::Id::unique("rack_amt_vel"),
                    &vel_label,
                    state.velocity_amount,
                ) {
                    state.velocity_amount = a;
                }
                x += 170.0;
                // The velocity of a hit added by hand: a dark field; its menu picks one.
                let r = egui::Rect::from_min_size(
                    egui::pos2(x, row.top() + 3.0),
                    vec2(86.0, AMOUNTS_H - 6.0),
                );
                let field = crate::hw::field(
                    ui,
                    &g,
                    r,
                    egui::Id::unique("rack_velocity"),
                    &format!("VEL {}", state.velocity),
                    true,
                )
                .on_hover_text(tr("Velocity"));
                egui::Popup::menu(&field).show(|ui| {
                    for v in VELOCITIES {
                        if ui
                            .selectable_label(state.velocity == v, v.to_string())
                            .clicked()
                        {
                            state.velocity = v;
                            ui.close();
                        }
                    }
                    ui.add(egui::Slider::new(&mut state.velocity, 1..=127));
                });
                // A kit being copied here from a share: a lit LED and its word, where there is
                // room left of the keys.
                let r = egui::Rect::from_min_size(
                    egui::pos2(x + 86.0 + 10.0, row.top() + 2.0),
                    vec2(COPYING_W, AMOUNTS_H - 4.0),
                );
                if copying && r.right() <= right {
                    crate::hw::paint_slot(
                        &g,
                        r,
                        crate::hw::SlotKind::Filled,
                        &tr("Copying kit").to_uppercase(),
                        Some(true),
                        true,
                        false,
                    );
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(250));
                }
            }
            ui.add_space(AMOUNTS_GAP);
            let mut remove: Option<usize> = None;
            let mut acts: Vec<StepAct> = Vec::new();
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
                let kit_dir = kit_folder(&state);
                // The head and the amounts stay put; only the rows scroll, in what the pane
                // leaves them over the Add Channel key under them.
                let under = if state.channels.len() < MAX_RACK_CHANNELS {
                    ADD_ROW_H + ui.spacing().item_spacing.y
                } else {
                    0.0
                };
                let rows_h = (ui.available_height() - under).max(ROW);
                egui::ScrollArea::both().max_height(rows_h).show(ui, |ui| {
                    let h = RULER_H + state.channels.len() as f32 * ROW;
                    let (area, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
                    let g = ui.painter().clone();
                    let sx = area.left() + STEPS_X;
                    step_ruler(&g, sx, area.top(), steps, pad_w, playing);
                    for c in 0..state.channels.len() {
                        let hits = hits_of(notes, &before, c, steps);
                        let swing = state.groove.feel(state.class(c)).swing;
                        let offset = state.groove.feel(state.class(c)).offset_amount;
                        let ch = &mut state.channels[c];
                        let y = area.top() + RULER_H + c as f32 * ROW;
                        let cell = |x: f32, w: f32| {
                            egui::Rect::from_min_size(egui::pos2(x, y), vec2(w, ROW - 6.0))
                        };
                        // Mute and Solo, side by side in the Mute column.
                        let half = (MUTE_W - 2.0) / 2.0;
                        let (mute, _) = crate::hw::key(
                            ui,
                            &g,
                            cell(area.left(), half),
                            egui::Id::unique(("rack_mute", c)),
                            crate::hw::Cap::Bone,
                            ch.mute,
                            Some(ch.mute),
                            crate::hw::Legend::Text("M"),
                        );
                        if mute.on_hover_text(tr("Mute")).clicked() {
                            ch.mute = !ch.mute;
                        }
                        let (solo, _) = crate::hw::key(
                            ui,
                            &g,
                            cell(area.left() + half + 2.0, half),
                            egui::Id::unique(("rack_solo", c)),
                            crate::hw::Cap::Bone,
                            ch.solo,
                            Some(ch.solo),
                            crate::hw::Legend::Text("S"),
                        );
                        if solo.on_hover_text(tr("Solo")).clicked() {
                            ch.solo = !ch.solo;
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
                        // A sample dragged from the Kit Browser lands here.
                        if name.dnd_hover_payload::<SampleDrag>().is_some() {
                            g.rect_stroke(
                                r,
                                3.0,
                                egui::Stroke::new(2.0, theme::ORANGE()),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if let Some(drop) = name.dnd_release_payload::<SampleDrag>() {
                            let fresh = channel_from(&s.kits.localize(&drop.0));
                            ch.path = fresh.path;
                            ch.name = fresh.name;
                        }
                        if name.clicked() {
                            if let Some(path) = pick_samples().first() {
                                let fresh = channel_from(&s.kits.localize(path));
                                ch.path = fresh.path;
                                ch.name = fresh.name;
                            }
                        }
                        // Its menu: hear it, the kit's samples of its class (and every other
                        // one), the one before or after it, and Remove Channel.
                        name.context_menu(|ui| {
                            if !ch.path.is_empty() && ui.button(tr("Hear")).clicked() {
                                s.kits.play_file(std::path::Path::new(&ch.path));
                                ui.close();
                            }
                            if let Some(kit) = kit_dir.as_deref().map(kit_samples) {
                                let mut put: Option<std::path::PathBuf> = None;
                                ui.horizontal(|ui| {
                                    if ui.button("‹").clicked() {
                                        put = cycle_sample(&kit, ch, -1);
                                    }
                                    if ui.button("›").clicked() {
                                        put = cycle_sample(&kit, ch, 1);
                                    }
                                });
                                let (mine, rest) = channel_samples(&kit, ch.class);
                                ui.menu_button(
                                    format!("{}  ({})", tr("Samples"), mine.len()),
                                    |ui| {
                                        egui::ScrollArea::vertical().max_height(420.0).show(
                                            ui,
                                            |ui| {
                                                for (n, path) in &mine {
                                                    let on = path.to_string_lossy() == ch.path;
                                                    if ui.selectable_label(on, n).clicked() {
                                                        put = Some(path.clone());
                                                        ui.close();
                                                    }
                                                }
                                            },
                                        );
                                    },
                                );
                                ui.menu_button(
                                    format!("{}  ({})", tr("All Other Samples"), rest.len()),
                                    |ui| {
                                        let h = ui.spacing().interact_size.y
                                            + ui.spacing().item_spacing.y;
                                        egui::ScrollArea::vertical().max_height(420.0).show_rows(
                                            ui,
                                            h,
                                            rest.len(),
                                            |ui, range| {
                                                for (n, path) in &rest[range] {
                                                    if ui.button(n).clicked() {
                                                        put = Some(path.clone());
                                                        ui.close();
                                                    }
                                                }
                                            },
                                        );
                                    },
                                );
                                if let Some(path) = put {
                                    let fresh = channel_from(&path);
                                    ch.path = fresh.path;
                                    ch.name = fresh.name;
                                    s.kits.play_file(&path);
                                }
                                ui.separator();
                            }
                            if ui.button(tr("Remove Channel")).clicked() {
                                remove = Some(c);
                                ui.close();
                            }
                        });
                        // Volume: a knob, and its level in an LCD readout that can be dragged
                        // too; double-click either for 0 dB.
                        let kc =
                            egui::pos2(area.left() + VOL_X + KNOB_W / 2.0, y + (ROW - 6.0) / 2.0);
                        let (vol, v) = crate::hw::knob(
                            ui,
                            &g,
                            kc,
                            9.0,
                            egui::Id::unique(("rack_vol", c)),
                            gain_to_knob(ch.gain_db),
                            gain_to_knob(0.0),
                        );
                        if let Some(v) = v {
                            ch.gain_db = knob_to_gain(v);
                        }
                        let _ = vol.on_hover_text(tr("Volume"));
                        let r = cell(area.left() + GAIN_X, GAIN_W);
                        let gain = ui.interact(
                            r,
                            egui::Id::unique(("rack_gain", c)),
                            Sense::click_and_drag(),
                        );
                        if gain.dragged() {
                            ch.gain_db =
                                (ch.gain_db - gain.drag_delta().y * 0.2).clamp(GAIN_MIN, GAIN_MAX);
                        }
                        if gain.double_clicked() {
                            ch.gain_db = 0.0;
                        }
                        crate::hw::lcd_readout(&g, r, "dB", &format!("{:.1}", ch.gain_db));
                        let _ = gain.on_hover_text(tr("Volume"));
                        // Pan: a knob; double-click for the centre.
                        let kc =
                            egui::pos2(area.left() + PAN_X + KNOB_W / 2.0, y + (ROW - 6.0) / 2.0);
                        let (pan, v) = crate::hw::knob(
                            ui,
                            &g,
                            kc,
                            9.0,
                            egui::Id::unique(("rack_pan", c)),
                            ch.pan,
                            0.0,
                        );
                        if let Some(v) = v {
                            ch.pan = (v * 100.0).round() / 100.0;
                        }
                        let _ = pan.on_hover_text(format!("{} {}", tr("Pan"), pan_text(ch.pan)));
                        // The class: a dark field; its menu gives the channel another class.
                        let r = cell(area.left() + CLASS_X, CLASS_W);
                        let class = crate::hw::field(
                            ui,
                            &g,
                            r,
                            egui::Id::unique(("rack_class", c)),
                            ch.class.tag(),
                            true,
                        );
                        egui::Popup::menu(&class).show(|ui| {
                            for k in CHANNEL_CLASSES {
                                if ui.selectable_label(ch.class == k, k.tag()).clicked() {
                                    ch.class = k;
                                    ui.close();
                                }
                            }
                        });
                        let class_now = ch.class;
                        // Swing of the channel's class: an LCD readout, dragged (every channel
                        // of the class follows); double-click for straight.
                        let r = cell(area.left() + SWING_X, SWING_W);
                        let sw = ui.interact(
                            r,
                            egui::Id::unique(("rack_swing", c)),
                            Sense::click_and_drag(),
                        );
                        let mut to = None;
                        if sw.dragged() && sw.drag_delta().y != 0.0 {
                            to = Some(swing - sw.drag_delta().y * 0.1);
                        }
                        if sw.double_clicked() {
                            to = Some(rack_groove::STRAIGHT);
                        }
                        crate::hw::lcd_readout(&g, r, "SW", &format!("{swing:.1}"));
                        let _ = sw.on_hover_text(tr("Swing"));
                        if let Some(pct) = to {
                            state.groove.set_swing(Some(class_now), pct, None);
                        }
                        // How much of its class's offsets the channel plays (every channel of
                        // the class follows), under the rack's Offset: an LCD readout in
                        // percent, dragged; double-click for 100.
                        let r = cell(area.left() + OFFSET_X, OFFSET_W);
                        let of = ui.interact(
                            r,
                            egui::Id::unique(("rack_offset", c)),
                            Sense::click_and_drag(),
                        );
                        let mut to = None;
                        if of.dragged() && of.drag_delta().y != 0.0 {
                            to = Some(offset - of.drag_delta().y * 0.005);
                        }
                        if of.double_clicked() {
                            to = Some(1.0);
                        }
                        crate::hw::lcd_readout(&g, r, "OFS", &format!("{:.0}", offset * 100.0));
                        let _ = of.on_hover_text(tr("Offset"));
                        if let Some(a) = to {
                            state.groove.set_offset(Some(class_now), a);
                        }
                        // The steps: click a step (or a part of a split one) to turn it on or
                        // off, drag a hit up or down for its velocity; the menu splits a step
                        // and sets its velocity.
                        for k in 0..steps {
                            let parts = step_parts(&hits, k);
                            let n = parts.len();
                            let r = cell(sx + step_x(k, pad_w), pad_w);
                            let id = egui::Id::unique(("rack_step", c, k));
                            let resp = step_pad(ui, &g, r, k, &parts, id);
                            let part_at = |x: f32| {
                                (((x - r.left()) / r.width() * n as f32).floor().max(0.0) as usize)
                                    .min(n - 1)
                            };
                            // The hit a click or a drag at `x` is on.
                            let hit_at = |x: f32| -> Option<Cell> {
                                if n == 1 {
                                    hits.iter().find(|h| h.0.step == k).map(|h| h.0)
                                } else {
                                    part_cell(k, part_at(x), n)
                                }
                            };
                            if resp.clicked() {
                                if n == 1 {
                                    acts.push(StepAct::Step(c, k, parts[0].is_none()));
                                } else if let Some(x) = resp.interact_pointer_pos().map(|p| p.x) {
                                    if let Some(cell) = part_cell(k, part_at(x), n) {
                                        acts.push(StepAct::Hit(
                                            c,
                                            cell,
                                            parts[part_at(x)].is_none(),
                                        ));
                                    }
                                }
                            }
                            if resp.dragged() && resp.drag_delta().y != 0.0 {
                                let origin = ui.input(|i| i.pointer.press_origin());
                                if let Some(cell) = origin.and_then(|o| hit_at(o.x)) {
                                    if let Some(h) = hits.iter().find(|h| h.0 == cell) {
                                        // Fractions of a velocity kept between frames.
                                        let key = id.with("vel_drag");
                                        let mut acc: f32 =
                                            ui.ctx().data(|d| d.get_temp(key)).unwrap_or(0.0);
                                        acc -= resp.drag_delta().y * 0.7;
                                        let whole = acc.trunc();
                                        ui.ctx().data_mut(|d| d.insert_temp(key, acc - whole));
                                        if whole != 0.0 {
                                            let v =
                                                (f32::from(h.1) + whole).clamp(1.0, 127.0) as u8;
                                            acts.push(StepAct::Vel(c, cell, v));
                                        }
                                    }
                                }
                            }
                            let lit: Vec<u8> = parts.iter().flatten().copied().collect();
                            let resp = if lit.is_empty() {
                                resp
                            } else {
                                let vels: Vec<String> = lit.iter().map(u8::to_string).collect();
                                resp.on_hover_text(format!("{} {}", tr("Velocity"), vels.join(" ")))
                            };
                            resp.context_menu(|ui| {
                                ui.label(tr("Divide"));
                                ui.horizontal(|ui| {
                                    for d in 1..=rack_groove::MAX_DIV {
                                        if ui
                                            .selectable_label(n == usize::from(d), d.to_string())
                                            .clicked()
                                        {
                                            acts.push(StepAct::Divide(c, k, d));
                                            ui.close();
                                        }
                                    }
                                });
                                if let Some(top) = lit.iter().max() {
                                    ui.separator();
                                    ui.label(tr("Velocity"));
                                    let mut v = *top;
                                    if ui.add(egui::Slider::new(&mut v, 1..=127)).changed() {
                                        acts.push(StepAct::StepVel(c, k, v));
                                    }
                                }
                            });
                        }
                    }
                });
            }
            ui.add_space(ADD_ROW_H - ADD_KEY_H);
            if state.channels.len() < MAX_RACK_CHANNELS {
                let label = tr("Add Channel…").to_uppercase();
                let w = label.chars().count() as f32 * 8.0 + 28.0;
                let add = rack_key(
                    ui,
                    vec2(w, ADD_KEY_H),
                    egui::Id::unique("rack_add"),
                    &label,
                    false,
                    None,
                );
                // A sample dropped on the key becomes a new channel.
                if let Some(drop) = add.dnd_release_payload::<SampleDrag>() {
                    state.channels.push(channel_from(&s.kits.localize(&drop.0)));
                }
                if add.clicked() {
                    for path in pick_samples() {
                        if state.channels.len() < MAX_RACK_CHANNELS {
                            state.channels.push(channel_from(&s.kits.localize(&path)));
                        }
                    }
                }
            }
            let mut before = before;
            if let Some(c) = remove {
                state.channels.remove(c);
                state.pattern.remove_channel(c);
                if c < before.channels.len() {
                    before.channels.remove(c);
                    before.pattern.remove_channel(c);
                }
                // The channels after it move up a note, and their steps with them.
                let gone = RACK_BASE_NOTE + c as u8;
                for r in p.tracks[ti].regions.iter_mut() {
                    r.notes.retain(|n| n.pitch != gone);
                    for n in r.notes.iter_mut().filter(|n| n.pitch > gone) {
                        n.pitch -= 1;
                    }
                }
                p.song_rev += 1;
            }
            if state != before || remove.is_some() {
                commit(p, ti, &before, state.clone());
            }
            if !acts.is_empty() {
                apply_step_acts(p, ti, ri, acts);
            }
        });
}

/// Makes the step changes of one frame in region `ri` of the rack on track `ti`.
fn apply_step_acts(p: &mut Project, ti: usize, ri: usize, acts: Vec<StepAct>) {
    let Some(mut state) = rack_state(p, ti) else {
        return;
    };
    for act in acts {
        let region = &mut p.tracks[ti].regions[ri];
        let changed = match act {
            StepAct::Step(c, k, on) => {
                set_step(region, &state, c, k, on);
                None
            }
            StepAct::Hit(c, cell, on) => {
                set_hit(region, &state, c, cell, on, None);
                None
            }
            StepAct::Vel(c, cell, v) => set_hit_velocity(region, &state, c, cell, v),
            StepAct::StepVel(c, k, v) => {
                let cells: Vec<Cell> = hits_of(&region.notes, &state, c, k + 1)
                    .into_iter()
                    .filter(|h| h.0.step == k)
                    .map(|h| h.0)
                    .collect();
                let mut now = None;
                for cell in cells {
                    let st = now.as_ref().unwrap_or(&state);
                    if let Some(n) = set_hit_velocity(region, st, c, cell, v) {
                        now = Some(n);
                    }
                }
                now
            }
            StepAct::Divide(c, k, d) => {
                divide_step(region, &state, c, k, d);
                None
            }
        };
        if let Some(new) = changed {
            let old = std::mem::replace(&mut state, new.clone());
            commit(p, ti, &old, new);
        }
    }
    p.song_rev += 1;
}

/// A project with a rack of `n` channels (at most eight) and a one-bar pattern on the first
/// five, for tests and pictures.
#[cfg(test)]
pub(crate) fn picture_project(n: usize) -> Project {
    let mut p = Project::empty();
    new_rack_track(&mut p);
    let ti = p.selected_track;
    let names = [
        "Kick", "Snare", "Hat", "Open Hat", "Clap", "Perc", "Kick 2", "Snare 2",
    ];
    let channels: Vec<RackChannel> = names[..n]
        .iter()
        .map(|n| RackChannel {
            path: format!("/samples/{n}.wav"),
            name: (*n).into(),
            gain_db: 0.0,
            pan: 0.0,
            mute: *n == "Clap",
            solo: false,
            class: class_of_path(&format!("/samples/{n}.wav")),
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
    for (c, ks) in steps.iter().enumerate().take(n) {
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
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names in the head give way to the amounts and keys: both whole when there is room,
    /// an even share when not, none narrower than `MIN_SLOT_W`.
    #[test]
    fn head_names_give_way() {
        assert_eq!(give_way(1000.0, true, true), (TITLE_W, KIT_NAME_W));
        assert_eq!(give_way(200.0, true, true), (100.0, 100.0));
        assert_eq!(give_way(200.0, false, true), (0.0, 200.0));
        assert_eq!(give_way(60.0, true, true), (0.0, 0.0));
        assert_eq!(give_way(-50.0, true, false), (0.0, 0.0));
        assert_eq!(give_way(-50.0, false, false), (0.0, 0.0));
    }

    /// The head is one row on the laptop with the Inspector shown (a pane about 1226 points
    /// wide), even for a pattern played as recorded; the keys go over the amounts in a narrower
    /// pane, and the rack's opening height counts that row.
    #[test]
    fn the_head_is_one_row_on_the_laptop() {
        let mut p = picture_project(3);
        let ti = p.selected_track;
        let mut r = rack_state(&p, ti).unwrap();
        r.kit = "A Kit".into();
        r.groove.name = "A Groove".into();
        let one = head_layout(1226.0 - 2.0 * f32::from(MARGIN_X), Some(&r));
        assert!(!one.two_rows && one.title_w >= MIN_SLOT_W && one.name_w >= MIN_SLOT_W);
        assert!(!head_layout(1150.0, Some(&r)).two_rows);
        // 850: two rows in both builds (the public head has no Pattern key, so 900 fits one).
        let two = head_layout(850.0, Some(&r));
        assert!(two.two_rows && two.title_w == TITLE_W && two.name_w == KIT_NAME_W);
        let old = rack_state(&p, ti).unwrap();
        commit(&mut p, ti, &old, r);
        let wide = full_height(&p, 4.0, 1226.0).unwrap();
        assert_eq!(full_height(&p, 4.0, 850.0).unwrap(), wide + AMOUNTS_H + 4.0);
    }

    #[test]
    fn channels_round_trip_through_the_slot() {
        let channels = vec![RackChannel {
            path: "/k/kick.wav".into(),
            name: "kick".into(),
            gain_db: -3.0,
            pan: 0.25,
            mute: true,
            solo: false,
            class: DrumClass::Perc,
        }];
        assert_eq!(decode(&Some(encode(&channels))), channels);
        assert!(decode(&None).is_empty());
        assert!(decode(&Some(b"not json".to_vec())).is_empty());
        // A rack saved before channels had classes: the class of the sample's name.
        let old = br#"{"channels":[{"path":"/k/Snare 2.wav","name":"Snare 2"},{"path":""}]}"#;
        let ch = decode(&Some(old.to_vec()));
        assert_eq!(
            (ch[0].class, ch[1].class),
            (DrumClass::Snare, DrumClass::Other)
        );
        // The groove is kept with the channels.
        let mut st = RackState {
            channels,
            ..RackState::default()
        };
        st.groove.set_swing(Some(DrumClass::Perc), 58.0, None);
        st.groove.name = "G".into();
        assert_eq!(RackState::read(&Some(st.bytes())), st);
    }

    #[test]
    fn a_kit_load_fills_the_channels_and_keeps_the_mix_and_the_steps() {
        let mut p = Project::empty();
        let pads = vec![
            Some(std::path::PathBuf::from("/lib/k/Kick 1.wav")),
            None,
            Some(std::path::PathBuf::from("/lib/k/hats/CH.wav")),
        ];
        // No rack yet: a new track named for the kit.
        let ti = load_kit(&mut p, None, "Kit One", &pads, &[]);
        assert_eq!(p.tracks[ti].name, "Kit One");
        assert_eq!(rack_track(&p), Some(ti));
        let slot = p.tracks[ti].au_instrument.clone().unwrap();
        assert_eq!(kit_of(&slot.state), "Kit One");
        let ch = decode(&slot.state);
        assert_eq!(
            ch.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["Kick 1", "", "CH"]
        );
        assert_eq!(ch[2].path, "/lib/k/hats/CH.wav");
        // A level, a mute and a step set by hand stay through the next kit.
        let mut ch = ch;
        ch[0].gain_db = -6.0;
        ch[2].mute = true;
        p.tracks[ti].au_instrument.as_mut().unwrap().state = Some(encode_kit(&ch, "Kit One"));
        let ri = step_region(&mut p, ti, None);
        p.tracks[ti].regions[ri].notes.push(Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start: 0.0,
            len: STEP,
            pitch: RACK_BASE_NOTE,
            vel: VELOCITY,
        });
        let two = vec![
            Some(std::path::PathBuf::from("/lib/b/BD.wav")),
            Some(std::path::PathBuf::from("/lib/b/SD.wav")),
            Some(std::path::PathBuf::from("/lib/b/HH.wav")),
            Some(std::path::PathBuf::from("/lib/b/OH.wav")),
        ];
        assert_eq!(load_kit(&mut p, Some(ti), "Kit Two", &two, &[]), ti);
        assert_eq!(
            p.tracks[ti].name, "Kit One",
            "a track already there keeps its name"
        );
        let slot = p.tracks[ti].au_instrument.clone().unwrap();
        assert_eq!(kit_of(&slot.state), "Kit Two");
        let ch = decode(&slot.state);
        assert_eq!(ch.len(), 4);
        assert_eq!((ch[0].name.as_str(), ch[0].gain_db), ("BD", -6.0));
        assert!(ch[2].mute && !ch[3].mute);
        assert_eq!(p.tracks[ti].regions[ri].notes.len(), 1);
        // A track that plays something else is not touched: the kit gets a rack of its own.
        let other = p.add_track(TrackKind::Instrument);
        let made = load_kit(&mut p, Some(other), "Kit Three", &two, &[]);
        assert_ne!(made, other);
    }

    #[test]
    fn one_sample_goes_on_one_channel() {
        let mut p = Project::empty();
        new_rack_track(&mut p);
        let ti = p.selected_track;
        let path = std::path::Path::new("/lib/k/Clap.wav");
        assert!(assign_sample(&mut p, ti, 2, path));
        let ch = decode(&p.tracks[ti].au_instrument.as_ref().unwrap().state);
        assert_eq!(ch.len(), 3);
        assert_eq!((ch[0].path.as_str(), ch[2].name.as_str()), ("", "Clap"));
        assert!(!assign_sample(&mut p, ti, MAX_RACK_CHANNELS, path));
        assert!(!assign_sample(&mut p, ti + 9, 0, path));
        // The rack's window keeps the kit's name when a channel changes.
        let slot = p.tracks[ti].au_instrument.as_mut().unwrap();
        slot.state = Some(encode_kit(&ch, "K"));
        assert!(assign_sample(&mut p, ti, 0, path));
        assert_eq!(
            kit_of(&p.tracks[ti].au_instrument.as_ref().unwrap().state),
            "K"
        );
        // A rack saved before kits had names reads as it did.
        assert_eq!(kit_of(&Some(encode(&ch))), "");
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

    #[test]
    fn a_kit_load_gives_each_channel_a_class_and_steps_take_its_feel() {
        let mut p = Project::empty();
        let pads = vec![
            Some(std::path::PathBuf::from("/lib/k/BD.wav")),
            Some(std::path::PathBuf::from("/lib/k/thing.wav")),
            None,
            Some(std::path::PathBuf::from("/lib/k/hat.wav")),
        ];
        let classes = [DrumClass::Kick, DrumClass::Snare];
        let ti = load_kit(&mut p, None, "K", &pads, &classes);
        let st = rack_state(&p, ti).unwrap();
        assert_eq!(
            st.classes(),
            [
                DrumClass::Kick,
                DrumClass::Snare,
                DrumClass::Clap,
                DrumClass::ClosedHat
            ],
            "given, given, the layout's for an empty channel, the file's"
        );
        // Steps: kick on 1 and the "a" of beat 1, hats on every sixteenth.
        let ri = step_region(&mut p, ti, None);
        for (c, k) in [(0, 0), (0, 3), (3, 0), (3, 1), (3, 2), (3, 3)] {
            set_step(&mut p.tracks[ti].regions[ri], &st, c, k, true);
        }
        // A groove: the hats swing 60 at 16ths, the kick plays its 4th sixteenth 0.02 late.
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 60.0, None);
        let mut kick = rack_groove::Feel::default();
        kick.offsets[3] = 0.02;
        g.classes.insert(DrumClass::Kick, kick);
        assert_eq!(set_groove(&mut p, ti, g.clone()), Some(3));
        let st = rack_state(&p, ti).unwrap();
        assert_eq!(st.groove, g);
        let at = |p: &Project, pitch: u8| -> Vec<f32> {
            let mut v: Vec<f32> = p.tracks[ti].regions[ri]
                .notes
                .iter()
                .filter(|n| n.pitch == pitch)
                .map(|n| n.start)
                .collect();
            v.sort_by(f32::total_cmp);
            v
        };
        let kick = at(&p, RACK_BASE_NOTE);
        assert!((kick[1] - 0.77).abs() < 1e-6, "{kick:?}");
        let hats = at(&p, RACK_BASE_NOTE + 3);
        assert!((hats[1] - 0.30).abs() < 1e-6 && (hats[3] - 0.80).abs() < 1e-6);
        // The grid still shows the same steps.
        let notes = &p.tracks[ti].regions[ri].notes;
        assert_eq!(steps_on(notes, &st, 0, 16), [0, 3]);
        assert_eq!(steps_on(notes, &st, 3, 16), [0, 1, 2, 3]);
        // A step added now goes where the feel plays it; taken off, it goes.
        set_step(&mut p.tracks[ti].regions[ri], &st, 3, 5, true);
        assert!(at(&p, RACK_BASE_NOTE + 3)
            .iter()
            .any(|x| (x - 1.30).abs() < 1e-6));
        set_step(&mut p.tracks[ti].regions[ri], &st, 3, 5, false);
        assert_eq!(at(&p, RACK_BASE_NOTE + 3).len(), 4);
        // The hat channel made a kick: its steps take the kick's feel.
        assert!(set_class(&mut p, ti, 3, DrumClass::Kick));
        let hats = at(&p, RACK_BASE_NOTE + 3);
        assert!((hats[1] - 0.25).abs() < 1e-6 && (hats[3] - 0.77).abs() < 1e-6);
        assert!(!set_class(&mut p, ti, 9, DrumClass::Kick));
        // A new kit keeps the groove; back to straight puts every step on the grid.
        load_kit(&mut p, Some(ti), "K2", &pads, &classes);
        assert_eq!(rack_state(&p, ti).unwrap().groove, g);
        set_groove(&mut p, ti, Groove::default());
        assert_eq!(at(&p, RACK_BASE_NOTE), [0.0, 0.75]);
        assert_eq!(at(&p, RACK_BASE_NOTE + 3), [0.0, 0.25, 0.5, 0.75]);
    }

    #[test]
    fn a_new_rack_starts_with_the_default_kit_when_its_folder_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let kit = dir.path().join("Best Of OZ");
        for f in [
            "Kicks/Kick (A).wav",
            "Snares/Snare (B).wav",
            "Hi Hats/Hihat (C).wav",
        ] {
            let path = kit.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"RIFF").unwrap();
        }
        let st = kit_state(&kit);
        assert_eq!(st.kit, "Best Of OZ");
        let names: Vec<&str> = st.channels.iter().map(|c| c.name.as_str()).collect();
        // A kit of fewer than five samples leaves the clap's channel empty.
        assert_eq!(names, ["Kick (A)", "Snare (B)", "", "Hihat (C)"]);
        assert_eq!(
            st.classes(),
            [
                DrumClass::Kick,
                DrumClass::Snare,
                DrumClass::Clap,
                DrumClass::ClosedHat
            ]
        );
        // A folder that is not there (a drive not mounted), or none: an empty rack.
        assert_eq!(kit_state(&dir.path().join("gone")), RackState::default());
        assert_eq!(kit_state(std::path::Path::new("")), RackState::default());
        // The tests' own racks start empty, whatever the machine has.
        assert!(decode(&new_slot().state).is_empty());
    }

    /// A project with a rack of kick, snare and closed hat.
    fn three_channel_rack() -> (Project, usize) {
        let mut p = Project::empty();
        let pads = vec![
            Some(std::path::PathBuf::from("/lib/k/Kick.wav")),
            Some(std::path::PathBuf::from("/lib/k/Snare.wav")),
            Some(std::path::PathBuf::from("/lib/k/Hat.wav")),
        ];
        let classes = [DrumClass::Kick, DrumClass::Snare, DrumClass::ClosedHat];
        let ti = load_kit(&mut p, None, "K", &pads, &classes);
        (p, ti)
    }

    fn notes_of(p: &Project, ti: usize, pitch: u8) -> Vec<(f32, u8)> {
        let ri = find_step_region(p, ti, None).unwrap();
        let mut v: Vec<(f32, u8)> = p.tracks[ti].regions[ri]
            .notes
            .iter()
            .filter(|n| n.pitch == pitch)
            .map(|n| (n.start, n.vel))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    }

    #[test]
    fn a_pattern_goes_onto_the_channels_by_class_with_its_rolls_and_velocities() {
        let (mut p, ti) = three_channel_rack();
        // A two-bar pattern: kicks, a clap (no clap channel: a snare stands in), a hat roll in
        // 64ths on step 3 and a triplet pair in step 7, a crash (a new channel).
        let pat = RackPattern {
            name: "Song · A".into(),
            source: "test".into(),
            steps: 32,
            hits: vec![
                (DrumClass::Kick, Cell::on(0), 120, None),
                (DrumClass::Kick, Cell::on(0), 90, None), // the same hit twice: the louder
                (DrumClass::Kick, Cell::on(19), 100, None),
                (DrumClass::Clap, Cell::on(4), 110, None),
                (DrumClass::ClosedHat, Cell::on(2), 70, None),
                (DrumClass::ClosedHat, Cell::new(2, 1, 4), 60, None),
                (DrumClass::ClosedHat, Cell::new(2, 2, 4), 50, None),
                (DrumClass::ClosedHat, Cell::new(2, 3, 4), 40, None),
                (DrumClass::ClosedHat, Cell::new(6, 1, 3), 80, None),
                (DrumClass::ClosedHat, Cell::new(6, 2, 3), 81, None),
                (DrumClass::Crash, Cell::on(0), 127, None),
                (DrumClass::Kick, Cell::on(40), 100, None), // past the pattern's end: left out
            ],
            bpm: Some(80.0),
            tempo_map: Vec::new(),
            ..RackPattern::default()
        };
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 60.0, None);
        g.swing_amount = 0.3; // a groove's amounts start full
        let done = load_pattern(&mut p, Some(ti), None, &pat, g, None, PatternTempo::Session);
        assert_eq!((done.hits, done.added, done.dropped), (10, 1, 0));
        let st = rack_state(&p, ti).unwrap();
        assert_eq!(st.channels.len(), 4);
        assert_eq!(st.channels[3].class, DrumClass::Crash);
        assert_eq!(st.groove.swing_amount, 1.0);
        assert_eq!(st.pattern.steps, 32);
        assert_eq!(st.pattern.name, "Song · A");
        // The region is two bars.
        let ri = done.region;
        assert_eq!(p.region_len_beats(&p.tracks[ti].regions[ri]), 8.0);
        assert_eq!(
            notes_of(&p, ti, RACK_BASE_NOTE),
            [(0.0, 120), (4.75, 100)],
            "kicks"
        );
        assert_eq!(
            notes_of(&p, ti, RACK_BASE_NOTE + 1),
            [(1.0, 110)],
            "the clap on the snare"
        );
        // Hats at 60 % 16th swing: step 2 (the "&") stays at 0.5, its span to the swung "a"
        // (0.80) is 0.30; step 6 (the "&" of beat 2) at 1.5.
        let hats = notes_of(&p, ti, RACK_BASE_NOTE + 2);
        let want = [
            (0.5, 70),
            (0.575, 60),
            (0.65, 50),
            (0.725, 40),
            (1.6, 80),
            (1.7, 81),
        ];
        assert_eq!(hats.len(), want.len());
        for (h, w) in hats.iter().zip(want) {
            assert!((h.0 - w.0).abs() < 1e-5 && h.1 == w.1, "{hats:?}");
        }
        // The grid shows step 3 split in four and step 7 in three.
        let notes = &p.tracks[ti].regions[ri].notes;
        let hits = hits_of(notes, &st, 2, 32);
        assert_eq!(
            step_parts(&hits, 2),
            [Some(70), Some(60), Some(50), Some(40)]
        );
        assert_eq!(step_parts(&hits, 6), [None, Some(80), Some(81)]);
        assert_eq!(steps_on(notes, &st, 2, 32), [2, 6]);
    }

    #[test]
    fn a_long_pattern_goes_in_four_bar_regions_with_every_hit_where_played() {
        let (mut p, ti) = three_channel_rack();
        p.tempo = 120.0;
        // Nine bars: a kick on bar 1, one on bar 6 played 0.0371 late, a hat on bar 9 and one
        // played just before bar 1 (it sits at the end, as a looped pattern plays it); the
        // tempo changes at beat 20 (bar 6) and 21.
        let pat = RackPattern {
            steps: 9 * 16,
            hits: vec![
                (DrumClass::Kick, Cell::on(0), 120, Some(0.0)),
                (DrumClass::Kick, Cell::on(80), 100, Some(0.0371)),
                (DrumClass::ClosedHat, Cell::on(130), 90, Some(-0.0123)),
                (DrumClass::ClosedHat, Cell::on(0), 80, Some(-0.02)),
                // A second hat in the same cell played 0.1 beat later (a flam): both play. A
                // kick found twice 3 ms apart: once.
                (DrumClass::ClosedHat, Cell::on(130), 60, Some(0.1)),
                (DrumClass::Kick, Cell::on(0), 90, Some(0.005)),
            ],
            bpm: Some(90.0),
            tempo_map: vec![(0.0, 90.0), (20.0, 150.0), (21.0, 100.0)],
            bass: vec![BassNote {
                start: 30.0,
                len: 1.0,
                pitch: 33,
                vel: 100,
            }],
            ..RackPattern::default()
        };
        let done = load_pattern(
            &mut p,
            Some(ti),
            None,
            &pat,
            Groove::default(),
            None,
            PatternTempo::Song,
        );
        assert_eq!(done.hits, 5);
        assert_eq!(done.regions, 3);
        assert_eq!(done.tempo_changes, 2);
        let st = rack_state(&p, ti).unwrap();
        assert_eq!(st.pattern.steps, MAX_STEPS);
        assert_eq!(
            st.pattern.hits.len(),
            1,
            "the first page only: {:?}",
            st.pattern.hits
        );
        // Each note's place from the pattern's start, by the region it is in.
        let mut at: Vec<(f64, u8)> = p.tracks[ti]
            .regions
            .iter()
            .flat_map(|r| {
                let s = p.region_start_beats(r);
                r.notes
                    .iter()
                    .map(move |n| (s + f64::from(n.start), n.pitch))
                    .collect::<Vec<_>>()
            })
            .collect();
        at.sort_by(|a, b| a.0.total_cmp(&b.0));
        let want = [
            (0.0, RACK_BASE_NOTE),
            (20.0 + 0.0371, RACK_BASE_NOTE),
            (32.5 - 0.0123, RACK_BASE_NOTE + 2),
            (32.5 + 0.1, RACK_BASE_NOTE + 2),
            (36.0 - 0.02, RACK_BASE_NOTE + 2),
        ];
        assert_eq!(at.len(), 5, "{at:?}");
        for (a, w) in at.iter().zip(want) {
            assert!((a.0 - w.0).abs() < 1e-6 && a.1 == w.1, "{at:?}");
        }
        // Regions of four bars from bar 1; the last one as long as its page.
        let mut bars: Vec<(f32, f32)> = p.tracks[ti]
            .regions
            .iter()
            .map(|r| (r.start_bar, r.len_bars))
            .collect();
        bars.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(bars, [(1.0, 4.0), (5.0, 4.0), (9.0, 1.0)]);
        // The tempo map: 90, then 150 at beat 20 and 100 at 21.
        assert_eq!(p.tempo_at(0.0), 90.0);
        assert_eq!(p.tempo_at(20.5), 150.0);
        assert_eq!(p.tempo_at(30.0), 100.0);
        // The 808 note on beat 30 (past the first four bars) is there.
        assert_eq!(done.bass_notes, 1);
        // Loading a 4-bar pattern over it clears the tempo changes inside its span.
        let short = RackPattern {
            steps: 16,
            hits: vec![(DrumClass::Kick, Cell::on(0), 120, Some(0.0))],
            bpm: Some(95.0),
            ..RackPattern::default()
        };
        let d = load_pattern(
            &mut p,
            Some(ti),
            Some((ti, done.region)),
            &short,
            Groove::default(),
            None,
            PatternTempo::Song,
        );
        assert_eq!((d.regions, d.tempo_changes), (1, 0));
        assert_eq!(p.tempo_at(0.0), 95.0);
    }

    #[test]
    fn played_hits_stay_where_they_were_played_and_the_tempo_follows_the_song() {
        let (mut p, ti) = three_channel_rack();
        p.tempo = 120.0;
        let pads = [
            Some(std::path::PathBuf::from("/lib/k/Kick.wav")),
            Some(std::path::PathBuf::from("/lib/k/Snare.wav")),
            Some(std::path::PathBuf::from("/lib/k/Hat.wav")),
        ];
        let classes = [DrumClass::Kick, DrumClass::Snare, DrumClass::ClosedHat];
        let pat = RackPattern {
            steps: 16,
            hits: vec![
                (DrumClass::Kick, Cell::on(0), 120, Some(0.0123)),
                // Further from its step than any groove offset can be.
                (DrumClass::Kick, Cell::on(8), 100, Some(-0.117)),
                (DrumClass::ClosedHat, Cell::on(2), 90, Some(0.031)),
                // No time of its own: where the groove puts it.
                (DrumClass::ClosedHat, Cell::on(6), 80, None),
                // The pattern's kit has no crash: left out, no empty channel made.
                (DrumClass::Crash, Cell::on(0), 90, Some(0.0)),
            ],
            bpm: Some(80.0),
            tempo_map: Vec::new(),
            bass: vec![
                BassNote {
                    start: 0.02,
                    len: 1.5,
                    pitch: 33,
                    vel: 110,
                },
                BassNote {
                    start: 4.5, // past the pattern's end
                    len: 1.0,
                    pitch: 33,
                    vel: 110,
                },
            ],
            ..RackPattern::default()
        };
        let mut g = Groove::default();
        let mut hat = rack_groove::Feel::default();
        hat.offsets[6] = 0.04;
        hat.offsets[2] = -0.05;
        g.classes.insert(DrumClass::ClosedHat, hat);
        let kit = KitLoad {
            name: "K",
            pads: &pads,
            classes: &classes,
        };
        let done = load_pattern(
            &mut p,
            Some(ti),
            None,
            &pat,
            g,
            Some(kit),
            PatternTempo::Song,
        );
        assert_eq!((done.hits, done.no_sample, done.added), (4, 1, 0));
        assert_eq!(done.tempo, Some(80.0));
        assert_eq!(p.tempo, 80.0);
        let close = |a: &[(f32, u8)], b: &[(f32, u8)]| {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(x, y)| (x.0 - y.0).abs() < 1e-5 && x.1 == y.1)
        };
        let kicks = notes_of(&p, ti, RACK_BASE_NOTE);
        assert!(close(&kicks, &[(0.0123, 120), (1.883, 100)]), "{kicks:?}");
        let hats = notes_of(&p, ti, RACK_BASE_NOTE + 2);
        assert!(close(&hats, &[(0.531, 90), (1.54, 80)]), "{hats:?}");
        // The grid shows the played hits on their steps.
        let st = rack_state(&p, ti).unwrap();
        assert!(st.pattern.is_played());
        let ri = done.region;
        assert_eq!(
            steps_on(&p.tracks[ti].regions[ri].notes, &st, 0, 16),
            [0, 8]
        );
        assert_eq!(
            steps_on(&p.tracks[ti].regions[ri].notes, &st, 2, 16),
            [2, 6]
        );
        // Saved and read back with its played times.
        assert_eq!(RackState::read(&Some(st.bytes())), st);
        // A new groove moves the hat the groove placed; the played hits stay.
        set_groove(&mut p, ti, Groove::default());
        let hats = notes_of(&p, ti, RACK_BASE_NOTE + 2);
        assert!(close(&hats, &[(0.531, 90), (1.5, 80)]), "{hats:?}");
        assert!(close(
            &notes_of(&p, ti, RACK_BASE_NOTE),
            &[(0.0123, 120), (1.883, 100)]
        ));
        // A played step turned off and on again plays where it was played.
        let st = rack_state(&p, ti).unwrap();
        set_step(&mut p.tracks[ti].regions[ri], &st, 0, 8, false);
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE).len(), 1);
        set_step(&mut p.tracks[ti].regions[ri], &st, 0, 8, true);
        assert!(close(
            &notes_of(&p, ti, RACK_BASE_NOTE),
            &[(0.0123, 120), (1.883, 100)]
        ));
        // The bass line is on the 808 track, as long as the rack's region; the rack's track
        // stays selected.
        let bi = done.bass_track.unwrap();
        assert_eq!(done.bass_notes, 1);
        assert_eq!(p.tracks[bi].name, BASS_TRACK);
        assert!(p.tracks[bi]
            .au_instrument
            .as_ref()
            .is_some_and(crate::bass808::is_808));
        let br = &p.tracks[bi].regions[0];
        assert_eq!(
            (br.start_bar, br.len_bars),
            (
                p.tracks[ti].regions[ri].start_bar,
                p.tracks[ti].regions[ri].len_bars
            )
        );
        assert_eq!(br.notes.len(), 1);
        assert_eq!(
            (br.notes[0].start, br.notes[0].len, br.notes[0].pitch),
            (0.02, 1.5, 33)
        );
        assert_eq!(p.selected_track, ti);
        // Session BPM keeps the project's tempo; a pattern without a bass line empties the 808
        // track rather than leave the last line playing.
        p.set_tempo_at(0.0, 100.0);
        let plain = RackPattern {
            bass: Vec::new(),
            ..pat.clone()
        };
        let again = load_pattern(
            &mut p,
            Some(ti),
            None,
            &plain,
            Groove::default(),
            None,
            PatternTempo::Session,
        );
        assert_eq!((again.tempo, p.tempo), (None, 100.0));
        assert_eq!(again.bass_track, Some(bi));
        assert!(p.tracks[bi].regions[0].notes.is_empty());
    }

    #[test]
    fn the_velocity_amount_takes_the_patterns_velocities_to_the_racks_and_back() {
        let (mut p, ti) = three_channel_rack();
        let pat = RackPattern {
            steps: 16,
            hits: vec![
                (DrumClass::Kick, Cell::on(0), 120, None),
                (DrumClass::Kick, Cell::on(8), 40, None),
            ],
            ..RackPattern::default()
        };
        load_pattern(
            &mut p,
            Some(ti),
            None,
            &pat,
            Groove::default(),
            None,
            PatternTempo::Session,
        );
        let old = rack_state(&p, ti).unwrap();
        let mut half = old.clone();
        half.velocity_amount = 0.5;
        commit(&mut p, ti, &old, half.clone());
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE), [(0.0, 110), (2.0, 70)]);
        let mut none = half.clone();
        none.velocity_amount = 0.0;
        none.velocity = 90; // the velocity of hits added by hand: not the pattern's base
        commit(&mut p, ti, &half, none.clone());
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE), [(0.0, 100), (2.0, 100)]);
        let mut full = none.clone();
        full.velocity_amount = 1.0;
        commit(&mut p, ti, &none, full.clone());
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE), [(0.0, 120), (2.0, 40)]);
        // A hit added by hand is not the pattern's: the amount leaves it.
        let ri = find_step_region(&p, ti, None).unwrap();
        set_hit(
            &mut p.tracks[ti].regions[ri],
            &full,
            0,
            Cell::on(4),
            true,
            Some(33),
        );
        commit(&mut p, ti, &full, half.clone());
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE)[1], (1.0, 33));
        // A pattern hit's velocity set by hand at 50 %: the pattern keeps what it stands for.
        let st = rack_state(&p, ti).unwrap();
        let new =
            set_hit_velocity(&mut p.tracks[ti].regions[ri], &st, 0, Cell::on(0), 100).unwrap();
        assert_eq!(new.pattern.velocity_at(0, Cell::on(0)), Some((100, 100)));
        commit(&mut p, ti, &st, new.clone());
        let mut back = new.clone();
        back.velocity_amount = 1.0;
        commit(&mut p, ti, &new, back);
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE)[0], (0.0, 100));
        // The pattern repeats over a longer region.
        assert_eq!(scaled_velocity(120, 100, 0.25), 105);
        assert_eq!(scaled_velocity(1, 127, 1.0), 1);
        let lp = LoadedPattern {
            steps: 16,
            hits: vec![PatternHit {
                channel: 0,
                cell: Cell::on(3),
                vel: 77,
                base: 90,
                played: Some(-0.031),
            }],
            ..LoadedPattern::default()
        };
        assert_eq!(lp.velocity_at(0, Cell::on(19)), Some((77, 90)));
        // A played hit plays where it was played in every repeat of the pattern.
        let at = lp.played_at(0, Cell::on(19)).unwrap();
        assert!((at - (19.0 * 0.25 - 0.031)).abs() < 1e-5, "{at}");
        assert_eq!(lp.played_cell(0, at), Some(Cell::on(19)));
        assert_eq!(lp.played_cell(0, at + 0.01), None);
        // A hit played ahead of the pattern's start sits that far ahead of its end.
        let mut early = lp.clone();
        early.hits[0].cell = Cell::on(0);
        assert!((early.played_at(0, Cell::on(0)).unwrap() - (4.0 - 0.031)).abs() < 1e-5);
        assert_eq!(early.played_cell(0, 4.0 - 0.031), Some(Cell::on(0)));
        assert_eq!(LoadedPattern::from_json(&lp.to_json()), lp);
    }

    #[test]
    fn a_groove_with_velocities_gives_them_to_the_steps_there_and_the_amount_scales_them() {
        let (mut p, ti) = three_channel_rack();
        let st = rack_state(&p, ti).unwrap();
        let ri = step_region(&mut p, ti, None);
        // Kicks on 1 and 3 at 100 and 60; a hat on the "e" at 90; a snare at 80 (the groove
        // has no snare velocities).
        let r = &mut p.tracks[ti].regions[ri];
        set_hit(r, &st, 0, Cell::on(0), true, Some(100));
        set_hit(r, &st, 0, Cell::on(8), true, Some(60));
        set_hit(r, &st, 2, Cell::on(1), true, Some(90));
        set_hit(r, &st, 1, Cell::on(4), true, Some(80));
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 60.0, None);
        let mut kick = [None; rack_groove::SLOTS];
        kick[0] = Some(120);
        kick[8] = Some(40);
        let mut hat = [None; rack_groove::SLOTS];
        hat[1] = Some(50);
        g.velocities.insert(DrumClass::Kick, kick);
        g.velocities.insert(DrumClass::ClosedHat, hat);
        set_groove(&mut p, ti, g.clone());
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE), [(0.0, 120), (2.0, 40)]);
        let hats = notes_of(&p, ti, RACK_BASE_NOTE + 2);
        assert!(
            (hats[0].0 - 0.30).abs() < 1e-6 && hats[0].1 == 50,
            "{hats:?}"
        );
        assert_eq!(
            notes_of(&p, ti, RACK_BASE_NOTE + 1),
            [(1.0, 80)],
            "no velocity: as it was"
        );
        // The amount: 0 % leaves them as they were, 50 % half way, 100 % the groove's.
        let set = |p: &mut Project, a: f32| {
            let old = rack_state(p, ti).unwrap();
            let mut new = old.clone();
            new.velocity_amount = a;
            commit(p, ti, &old, new);
        };
        set(&mut p, 0.0);
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE), [(0.0, 100), (2.0, 60)]);
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE + 2)[0].1, 90);
        set(&mut p, 0.5);
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE), [(0.0, 110), (2.0, 50)]);
        set(&mut p, 1.0);
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE), [(0.0, 120), (2.0, 40)]);
        // The swing changed by hand later: the velocities are not given again (the bases stay).
        let mut g2 = rack_state(&p, ti).unwrap().groove;
        g2.set_swing(Some(DrumClass::ClosedHat), 55.0, None);
        set_groove(&mut p, ti, g2);
        set(&mut p, 0.0);
        assert_eq!(notes_of(&p, ti, RACK_BASE_NOTE), [(0.0, 100), (2.0, 60)]);
    }

    #[test]
    fn a_step_splits_into_a_ratchet_and_its_parts_turn_on_and_off() {
        let (mut p, ti) = three_channel_rack();
        let st = rack_state(&p, ti).unwrap();
        let ri = step_region(&mut p, ti, None);
        let r = &mut p.tracks[ti].regions[ri];
        set_hit(r, &st, 2, Cell::on(5), true, Some(90));
        divide_step(r, &st, 2, 5, 3);
        let hits = hits_of(&r.notes, &st, 2, 16);
        assert_eq!(step_parts(&hits, 5), [Some(90); 3]);
        assert_eq!(r.notes.len(), 3);
        // Each a third of a sixteenth apart, a third long.
        assert!((r.notes[1].start - (1.25 + 0.25 / 3.0)).abs() < 1e-6);
        assert!((r.notes[1].len - 0.25 / 3.0).abs() < 1e-6);
        // The middle one off; a 64th added: the step shows twelve parts.
        set_hit(r, &st, 2, Cell::new(5, 1, 3), false, None);
        set_hit(r, &st, 2, Cell::new(5, 1, 4), true, None);
        let hits = hits_of(&r.notes, &st, 2, 16);
        let parts = step_parts(&hits, 5);
        assert_eq!(parts.len(), 12);
        assert_eq!(parts[3], Some(st.velocity));
        assert_eq!(parts[8], Some(90));
        assert_eq!(part_cell(5, 3, 12), Some(Cell::new(5, 1, 4)));
        assert_eq!(part_cell(5, 1, 12), None, "1/12 is no division of 1 to 4");
        assert_eq!(part_cell(5, 2, 4), Some(Cell::new(5, 1, 2)));
        // Back to one: one hit at the loudest velocity; the step off takes every hit.
        divide_step(r, &st, 2, 5, 1);
        assert_eq!(
            hits_of(&r.notes, &st, 2, 16)
                .iter()
                .map(|h| (h.0, h.1))
                .collect::<Vec<_>>(),
            [(Cell::on(5), 100)]
        );
        set_step(r, &st, 2, 5, false);
        assert!(r.notes.is_empty());
    }

    #[test]
    fn solo_plays_only_the_soloed_channels_and_is_kept() {
        let (mut p, ti) = three_channel_rack();
        let old = rack_state(&p, ti).unwrap();
        let mut new = old.clone();
        new.channels[1].solo = true;
        new.channels[2].mute = true;
        commit(&mut p, ti, &old, new);
        let slot = p.tracks[ti].au_instrument.as_ref().unwrap();
        let mutes: Vec<bool> = specs(slot).iter().map(|c| c.mute).collect();
        assert_eq!(
            mutes,
            vec![true, false, true],
            "only the soloed channel plays"
        );
        let back = RackState::read(&slot.state);
        assert!(back.channels[1].solo && !back.channels[0].solo);
        // A kit loaded on top keeps each channel's solo, as it keeps its mute.
        let pads = vec![Some(std::path::PathBuf::from("/lib/x/Kick 2.wav")); 3];
        load_kit(&mut p, Some(ti), "X", &pads, &[]);
        assert!(rack_state(&p, ti).unwrap().channels[1].solo);
        // Nothing soloed: the mutes alone.
        let old = rack_state(&p, ti).unwrap();
        let mut new = old.clone();
        new.channels[1].solo = false;
        commit(&mut p, ti, &old, new);
        let slot = p.tracks[ti].au_instrument.as_ref().unwrap();
        let mutes: Vec<bool> = specs(slot).iter().map(|c| c.mute).collect();
        assert_eq!(mutes, vec![false, false, true]);
    }

    #[test]
    fn the_kit_folder_and_the_pattern_song_are_kept_with_the_rack() {
        let mut st = RackState {
            kit_dir: "/kits/OZ".into(),
            ..RackState::default()
        };
        st.pattern = LoadedPattern {
            name: "Song · A".into(),
            dir: "/music/Song".into(),
            bar: 9,
            ..LoadedPattern::default()
        };
        let back = RackState::read(&Some(st.bytes()));
        assert_eq!(back.kit_dir, "/kits/OZ");
        assert_eq!(
            (back.pattern.dir.as_str(), back.pattern.bar),
            ("/music/Song", 9)
        );
        // Without a folder named, the folder every channel's sample is in.
        let (p, ti) = three_channel_rack();
        let st = rack_state(&p, ti).unwrap();
        assert_eq!(st.kit_dir, "");
        assert_eq!(
            kit_folder(&st),
            None,
            "/lib/k is too near the root to be one kit"
        );
        let mut st = st;
        for (i, c) in st.channels.iter_mut().enumerate() {
            c.path = format!("/Users/me/kits/OZ/Drums/{i}/s.wav");
        }
        assert_eq!(
            kit_folder(&st),
            Some(std::path::PathBuf::from("/Users/me/kits/OZ/Drums"))
        );
    }

    /// Each drum's own Offset: the snare at 50 % plays half its offsets, the kick all of its,
    /// the rack's Offset scaling both; the notes move with it. Dragging a row's OFS readout up
    /// raises its class's amount, a double-click puts it back at 100 %.
    #[test]
    fn each_drum_has_its_own_offset_amount() {
        let mut p = picture_project(5);
        let ti = p.selected_track;
        let mut g = Groove::default();
        g.default.offsets[4] = 0.04;
        g.default.offsets[7] = 0.04;
        set_groove(&mut p, ti, g.clone());
        let start = |p: &Project, c: u8, k: f32| {
            p.tracks[ti].regions[0]
                .notes
                .iter()
                .find(|n| n.pitch == RACK_BASE_NOTE + c && (n.start - k * STEP).abs() < 0.05)
                .map(|n| n.start)
                .unwrap()
        };
        assert!((start(&p, 1, 4.0) - 1.04).abs() < 1e-5);
        g.set_offset(Some(DrumClass::Snare), 0.5);
        set_groove(&mut p, ti, g.clone());
        assert!((start(&p, 1, 4.0) - 1.02).abs() < 1e-5, "snare at half");
        assert!((start(&p, 0, 7.0) - 1.79).abs() < 1e-5, "kick whole");
        let mut st = rack_state(&p, ti).unwrap();
        let old = st.clone();
        st.groove.offset_amount = 0.5;
        commit(&mut p, ti, &old, st);
        assert!((start(&p, 1, 4.0) - 1.01).abs() < 1e-5);
        assert!((start(&p, 0, 7.0) - 1.77).abs() < 1e-5);
        // Saved with the rack.
        let st = rack_state(&p, ti).unwrap();
        assert_eq!(RackState::read(&Some(st.bytes())), st);
        // The snare row's OFS readout, dragged up 40 points: 20 % more.
        let mut s = crate::UiState::new();
        let mut h = crate::ui_test::Harness::new(1470.0, 600.0);
        let frame = |ui: &mut egui::Ui, p: &mut Project, s: &mut crate::UiState| {
            crate::theme::apply(ui.ctx());
            show(ui, p, s)
        };
        h.settle(|ui| frame(ui, &mut p, &mut s));
        let at = egui::pos2(f32::from(MARGIN_X) + OFFSET_X + OFFSET_W / 2.0, 120.0);
        let snare = |p: &Project| {
            rack_state(p, ti)
                .unwrap()
                .groove
                .feel(DrumClass::Snare)
                .offset_amount
        };
        h.drag(
            &[at, at - vec2(0.0, 20.0), at - vec2(0.0, 40.0)],
            egui::PointerButton::Primary,
            |ui| frame(ui, &mut p, &mut s),
        );
        assert!((snare(&p) - 0.7).abs() < 0.02, "{}", snare(&p));
        assert_eq!(
            rack_state(&p, ti)
                .unwrap()
                .groove
                .feel(DrumClass::Kick)
                .offset_amount,
            1.0
        );
        h.time_passes(1.0);
        h.click(at, |ui| frame(ui, &mut p, &mut s));
        h.click(at, |ui| frame(ui, &mut p, &mut s));
        assert_eq!(snare(&p), 1.0);
    }

    #[test]
    fn the_volume_knob_turns_through_the_readouts_range() {
        assert_eq!(gain_to_knob(GAIN_MIN), -1.0);
        assert_eq!(gain_to_knob(GAIN_MAX), 1.0);
        assert!((gain_to_knob(0.0) - 0.6).abs() < 1e-6);
        for db in [-48.0, -20.5, -6.0, 0.0, 3.3, 12.0] {
            assert!((knob_to_gain(gain_to_knob(db)) - db).abs() < 0.051, "{db}");
        }
        assert_eq!(pan_text(0.0), "C");
        assert_eq!(pan_text(-0.25), "L25");
        assert_eq!(pan_text(1.0), "R100");
        // A rack's velocity and amounts are kept with it.
        let mut st = RackState {
            velocity: 64,
            velocity_amount: 0.4,
            ..RackState::default()
        };
        st.groove.offset_amount = 0.2;
        assert_eq!(RackState::read(&Some(st.bytes())), st);
        assert_eq!(
            RackState::read(&Some(br#"{"channels":[]}"#.to_vec())).velocity,
            VELOCITY
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
        let mut p = picture_project(5);
        let mut s = crate::UiState::new();
        crate::ui_test::render_png(
            1200.0,
            300.0,
            2.0,
            crate::theme::BONE(),
            |ui| {
                crate::theme::apply(ui.ctx());
                show(ui, &mut p, &mut s)
            },
            &shot_path("channel-rack.png"),
        )
        .unwrap();
    }
}
