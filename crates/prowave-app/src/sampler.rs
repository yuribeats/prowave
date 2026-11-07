//! The Sampler: an instrument after the Boss Dr. Sample SP-303 (see `docs/SP303-NOTES.md`),
//! with a file browser, a sample editor that chops, and pitch and time per pad.
//!
//! The settings live in the track's Instrument slot (`prowave:sampler`) as JSON: the source
//! files, the pads (what part of which file, and how each plays), the effect and its three
//! knobs. The engine's instrument is not rebuilt when they change: [`tick`] compares what the
//! engine was last sent with what the settings say and sends the difference, preparing pads'
//! sounds on background threads first (decoding, cutting, stretching).
//!
//! Banks and pads follow the SP-303 (manual p. 18): 8 pads a bank, banks A–D. Banks E–H are an
//! extension, for chopping a loop into more than 32 slices. Pad `k` (counting through the banks)
//! plays on MIDI note `base_note + k`, C1 (36) for the first by default.
use crate::model::{Note, Project, Region, RegionParams, Track, TrackKind};
use crate::plugins::PluginSlot;
use prowave_engine::sample_tools::{self, Grade, SliceEdit};
use prowave_engine::{FxSettings, PadParams, SampleBuf, SamplerMsg, SamplerRemote, SpFxKind};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// The Sampler in an Instrument slot.
pub const SAMPLER_ID: &str = "prowave:sampler";
pub const PADS_PER_BANK: usize = 8;
/// A–D as on the SP-303, E–H the extension.
pub const BANKS: usize = 8;
pub const HARDWARE_BANKS: usize = 4;
pub const PADS: usize = PADS_PER_BANK * BANKS;
pub const BASE_NOTE: u8 = 36;
/// Where a pattern's notes are kept: regions named like this on the sampler's track.
pub const PATTERN_PREFIX: &str = "SP Pattern ";

/// How a pad changes pitch and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PitchMode {
    /// Speed and pitch together, as a classic sampler (and the SP-303's pads).
    #[default]
    Repitch,
    /// Pitch and length apart (Signalsmith Stretch), prepared off the audio thread.
    Stretch,
}

/// The pad's length change (the SP-303's TIME/BPM Time Modify, manual p. 42, and more).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TimeMode {
    /// The sound as it is ("oFF").
    #[default]
    Off,
    /// Played at this fraction of its own tempo (the SP-303 allows ½ to about 1.3).
    Ratio(f64),
    /// Follows the project tempo, from the pad's BPM ("Ptn").
    Follow,
    /// Fit to this many bars at the project tempo.
    Bars(f64),
}

/// The SP-303's pattern quantize values (manual p. 45).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Quantize {
    Off,
    Q4,
    Q8,
    Q8T,
    #[default]
    Q16,
}

impl Quantize {
    pub const ALL: [Quantize; 5] = [
        Quantize::Off,
        Quantize::Q4,
        Quantize::Q8,
        Quantize::Q8T,
        Quantize::Q16,
    ];
    /// The display's text.
    pub fn code(self) -> &'static str {
        match self {
            Quantize::Off => "oFF",
            Quantize::Q4 => "4",
            Quantize::Q8 => "8",
            Quantize::Q8T => "8-3",
            Quantize::Q16 => "16",
        }
    }
    /// The grid in quarter notes.
    pub fn beats(self) -> Option<f64> {
        match self {
            Quantize::Off => None,
            Quantize::Q4 => Some(1.0),
            Quantize::Q8 => Some(0.5),
            Quantize::Q8T => Some(1.0 / 3.0),
            Quantize::Q16 => Some(0.25),
        }
    }
}

/// One pad.
#[derive(Clone, Debug, PartialEq)]
pub struct Pad {
    /// Index into [`SamplerState::sources`]; `None` is an empty pad.
    pub source: Option<usize>,
    pub name: String,
    /// Frames of the source (at its own rate); `end` 0 means its end.
    pub start: u64,
    pub end: u64,
    pub gain_db: f32,
    pub reverse: bool,
    pub fade_in: f32,
    pub fade_out: f32,
    pub normalize: bool,
    pub grade: Grade,
    pub stereo: bool,
    /// The SP-303's LEVEL, 0–127 (manual p. 38).
    pub level: u8,
    pub pan: f32,
    pub semis: i32,
    pub cents: i32,
    pub gate: bool,
    pub looping: bool,
    pub choke: u8,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub vel_sens: f32,
    /// Set to the effect (the SP-303's REMAIN + pad, manual p. 22).
    pub fx: bool,
    pub mode: PitchMode,
    pub time: TimeMode,
    /// The pad's tempo (from its length, as the SP-303 works it out, or typed).
    pub bpm: Option<f64>,
}

impl Default for Pad {
    fn default() -> Self {
        Self {
            source: None,
            name: String::new(),
            start: 0,
            end: 0,
            gain_db: 0.0,
            reverse: false,
            fade_in: 0.0,
            fade_out: 0.0,
            normalize: false,
            grade: Grade::Standard,
            stereo: true,
            level: 127,
            pan: 0.0,
            semis: 0,
            cents: 0,
            gate: false,
            looping: false,
            choke: 0,
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.02,
            cutoff: 20_000.0,
            resonance: 0.0,
            vel_sens: 1.0,
            fx: false,
            mode: PitchMode::Repitch,
            time: TimeMode::Off,
            bpm: None,
        }
    }
}

impl Pad {
    pub fn is_empty(&self) -> bool {
        self.source.is_none()
    }
}

/// A file the pads play from.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Source {
    pub path: String,
    /// Detected or typed.
    pub bpm: Option<f64>,
    pub key: Option<String>,
    /// Chop points, frames of the source, in order.
    pub chops: Vec<u64>,
}

/// Everything the slot saves.
#[derive(Clone, Debug, PartialEq)]
pub struct SamplerState {
    pub sources: Vec<Source>,
    pub pads: Vec<Pad>,
    pub fx: FxSettings,
    /// The MFX type picked (1–21), kept while a key effect is on.
    pub mfx: usize,
    /// The VOLUME knob, 0 to 1.
    pub volume: f32,
    pub base_note: u8,
    /// Chromatic: every key plays this pad, `root` at its own pitch.
    pub chromatic: Option<usize>,
    pub root: u8,
    pub quantize: Quantize,
    /// Pattern length in bars (1–99, manual p. 45).
    pub pattern_len: u32,
    /// Copy the files the pads use into the project when it is saved.
    pub consolidate: bool,
}

impl Default for SamplerState {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            pads: vec![Pad::default(); PADS],
            fx: FxSettings::default(),
            mfx: 1,
            volume: 0.8,
            base_note: BASE_NOTE,
            chromatic: None,
            root: 60,
            quantize: Quantize::Q16,
            pattern_len: 1,
            consolidate: true,
        }
    }
}

pub fn is_sampler(slot: &PluginSlot) -> bool {
    slot.id == SAMPLER_ID
}

/// An Instrument slot with an empty Sampler.
pub fn new_slot() -> PluginSlot {
    let mut slot = PluginSlot::new(SAMPLER_ID, "Sampler");
    slot.state = Some(encode(&SamplerState::default()));
    slot
}

// ------------------------------------------------------------------ saving

fn grade_name(g: Grade) -> &'static str {
    match g {
        Grade::Standard => "standard",
        Grade::Long => "long",
        Grade::LoFi => "lofi",
    }
}

fn grade_of(s: &str) -> Grade {
    match s {
        "long" => Grade::Long,
        "lofi" => Grade::LoFi,
        _ => Grade::Standard,
    }
}

fn quantize_of(s: &str) -> Quantize {
    Quantize::ALL
        .into_iter()
        .find(|q| q.code() == s)
        .unwrap_or_default()
}

fn time_to_json(t: TimeMode) -> Value {
    match t {
        TimeMode::Off => json!({"kind": "off"}),
        TimeMode::Ratio(r) => json!({"kind": "ratio", "value": r}),
        TimeMode::Follow => json!({"kind": "follow"}),
        TimeMode::Bars(b) => json!({"kind": "bars", "value": b}),
    }
}

fn time_of(v: &Value) -> TimeMode {
    let x = v["value"].as_f64().unwrap_or(1.0);
    match v["kind"].as_str().unwrap_or("off") {
        "ratio" if x.is_finite() => TimeMode::Ratio(x.clamp(0.25, 4.0)),
        "follow" => TimeMode::Follow,
        "bars" if x.is_finite() => TimeMode::Bars(x.clamp(0.0625, 64.0)),
        _ => TimeMode::Off,
    }
}

pub fn encode(s: &SamplerState) -> Vec<u8> {
    let pads: Vec<Value> = s
        .pads
        .iter()
        .map(|p| {
            json!({
                "source": p.source, "name": p.name, "start": p.start, "end": p.end,
                "gain_db": p.gain_db, "reverse": p.reverse, "fade_in": p.fade_in,
                "fade_out": p.fade_out, "normalize": p.normalize, "grade": grade_name(p.grade),
                "stereo": p.stereo, "level": p.level, "pan": p.pan, "semis": p.semis,
                "cents": p.cents, "gate": p.gate, "loop": p.looping, "choke": p.choke,
                "attack": p.attack, "decay": p.decay, "sustain": p.sustain,
                "release": p.release, "cutoff": p.cutoff, "resonance": p.resonance,
                "vel_sens": p.vel_sens, "fx": p.fx,
                "mode": if p.mode == PitchMode::Stretch { "stretch" } else { "repitch" },
                "time": time_to_json(p.time), "bpm": p.bpm,
            })
        })
        .collect();
    let sources: Vec<Value> = s
        .sources
        .iter()
        .map(|c| json!({"path": c.path, "bpm": c.bpm, "key": c.key, "chops": c.chops}))
        .collect();
    let kind = SpFxKind::ALL
        .iter()
        .position(|k| *k == s.fx.kind)
        .unwrap_or(0);
    json!({
        "version": 1,
        "sources": sources,
        "pads": pads,
        "fx": {"kind": kind, "on": s.fx.on, "ctrl": s.fx.ctrl},
        "mfx": s.mfx,
        "volume": s.volume,
        "base_note": s.base_note,
        "chromatic": s.chromatic,
        "root": s.root,
        "quantize": s.quantize.code(),
        "pattern_len": s.pattern_len,
        "consolidate": s.consolidate,
    })
    .to_string()
    .into_bytes()
}

/// The settings a slot holds; anything missing or out of range takes its default.
pub fn decode(state: &Option<Vec<u8>>) -> SamplerState {
    let mut s = SamplerState::default();
    let Some(v) = state
        .as_deref()
        .and_then(|b| serde_json::from_slice::<Value>(b).ok())
    else {
        return s;
    };
    let f = |v: &Value, k: &str, d: f32, lo: f32, hi: f32| {
        v[k].as_f64()
            .map(|x| x as f32)
            .filter(|x| x.is_finite())
            .unwrap_or(d)
            .clamp(lo, hi)
    };
    let b = |v: &Value, k: &str, d: bool| v[k].as_bool().unwrap_or(d);
    if let Some(list) = v["sources"].as_array() {
        s.sources = list
            .iter()
            .take(256)
            .map(|c| Source {
                path: c["path"].as_str().unwrap_or_default().to_string(),
                bpm: c["bpm"].as_f64().filter(|x| x.is_finite() && *x > 0.0),
                key: c["key"].as_str().map(String::from),
                chops: c["chops"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_u64).take(4096).collect())
                    .unwrap_or_default(),
            })
            .collect();
    }
    let n = s.sources.len();
    if let Some(list) = v["pads"].as_array() {
        for (k, p) in list.iter().take(PADS).enumerate() {
            let d = Pad::default();
            s.pads[k] = Pad {
                source: p["source"].as_u64().map(|x| x as usize).filter(|x| *x < n),
                name: p["name"].as_str().unwrap_or_default().to_string(),
                start: p["start"].as_u64().unwrap_or(0),
                end: p["end"].as_u64().unwrap_or(0),
                gain_db: f(p, "gain_db", 0.0, -60.0, 24.0),
                reverse: b(p, "reverse", false),
                fade_in: f(p, "fade_in", 0.0, 0.0, 30.0),
                fade_out: f(p, "fade_out", 0.0, 0.0, 30.0),
                normalize: b(p, "normalize", false),
                grade: grade_of(p["grade"].as_str().unwrap_or("")),
                stereo: b(p, "stereo", true),
                level: p["level"].as_u64().map_or(127, |x| x.min(127) as u8),
                pan: f(p, "pan", 0.0, -1.0, 1.0),
                semis: p["semis"].as_i64().map_or(0, |x| x.clamp(-24, 24) as i32),
                cents: p["cents"].as_i64().map_or(0, |x| x.clamp(-100, 100) as i32),
                gate: b(p, "gate", false),
                looping: b(p, "loop", false),
                choke: p["choke"].as_u64().map_or(0, |x| x.min(16) as u8),
                attack: f(p, "attack", d.attack, 0.0, 10.0),
                decay: f(p, "decay", d.decay, 0.0, 10.0),
                sustain: f(p, "sustain", d.sustain, 0.0, 1.0),
                release: f(p, "release", d.release, 0.0, 10.0),
                cutoff: f(p, "cutoff", d.cutoff, 20.0, 20_000.0),
                resonance: f(p, "resonance", 0.0, 0.0, 1.0),
                vel_sens: f(p, "vel_sens", 1.0, 0.0, 1.0),
                fx: b(p, "fx", false),
                mode: if p["mode"].as_str() == Some("stretch") {
                    PitchMode::Stretch
                } else {
                    PitchMode::Repitch
                },
                time: time_of(&p["time"]),
                bpm: p["bpm"]
                    .as_f64()
                    .filter(|x| x.is_finite() && (20.0..=400.0).contains(x)),
            };
        }
    }
    let fx = &v["fx"];
    let kind = fx["kind"]
        .as_u64()
        .and_then(|k| SpFxKind::ALL.get(k as usize).copied())
        .unwrap_or_default();
    let mut ctrl = kind.defaults();
    if let Some(a) = fx["ctrl"].as_array() {
        for (c, x) in ctrl.iter_mut().zip(a) {
            *c = x.as_f64().map_or(*c, |x| x as f32).clamp(0.0, 1.0);
        }
    }
    s.fx = FxSettings {
        kind,
        on: fx["on"].as_bool().unwrap_or(false) && kind.available(),
        ctrl,
        bpm: 120.0,
    };
    s.mfx = v["mfx"].as_u64().map_or(1, |x| (x as usize).clamp(1, 21));
    s.volume = f(&v, "volume", 0.8, 0.0, 1.0);
    s.base_note = v["base_note"]
        .as_u64()
        .map_or(BASE_NOTE, |x| x.min(127) as u8);
    s.chromatic = v["chromatic"]
        .as_u64()
        .map(|x| x as usize)
        .filter(|x| *x < PADS);
    s.root = v["root"].as_u64().map_or(60, |x| x.min(127) as u8);
    s.quantize = quantize_of(v["quantize"].as_str().unwrap_or("16"));
    s.pattern_len = v["pattern_len"]
        .as_u64()
        .map_or(1, |x| x.clamp(1, 99) as u32);
    s.consolidate = b(&v, "consolidate", true);
    s
}

/// The settings with each source file given a new name by `rename` (for saving into the
/// project: `rename` gives the file's place among the project's media). Only when the slot
/// says to consolidate; otherwise the paths stay as they are.
pub fn state_for_save(
    state: &Option<Vec<u8>>,
    mut rename: impl FnMut(&Path) -> String,
) -> Option<Vec<u8>> {
    let mut s = decode(state);
    if !s.consolidate {
        return state.clone();
    }
    for src in s.sources.iter_mut() {
        if !src.path.is_empty() && Path::new(&src.path).is_file() {
            src.path = rename(Path::new(&src.path));
        }
    }
    Some(encode(&s))
}

/// The settings of a loaded project, with paths into its media made into files on disk.
pub fn state_after_load(state: &Option<Vec<u8>>, media_dir: &Path) -> Option<Vec<u8>> {
    let mut s = decode(state);
    let mut changed = false;
    for src in s.sources.iter_mut() {
        if prowave_project::is_media_path(&src.path) {
            src.path = media_dir.join(&src.path).to_string_lossy().into_owned();
            changed = true;
        }
    }
    if changed {
        Some(encode(&s))
    } else {
        state.clone()
    }
}

// ------------------------------------------------------------------ to the engine

/// The project's tempo at the start (the pads follow one tempo, as the SP-303 does).
pub fn project_bpm(p: &Project) -> f64 {
    f64::from(p.tempo).clamp(20.0, 400.0)
}

pub fn beats_per_bar(p: &Project) -> f64 {
    f64::from(p.sig.0.max(1)) * 4.0 / f64::from(p.sig.1.max(1))
}

/// Frames of a pad's part of its source (`frames`: the source's length).
pub fn pad_range(pad: &Pad, frames: u64) -> (u64, u64) {
    let end = if pad.end == 0 {
        frames
    } else {
        pad.end.min(frames)
    };
    (pad.start.min(end), end)
}

/// How much longer the pad's sound becomes (1: unchanged), at `bpm`.
pub fn length_factor(pad: &Pad, seconds: f64, bpm: f64, bar_beats: f64) -> f64 {
    let f = match pad.time {
        TimeMode::Off => 1.0,
        TimeMode::Ratio(r) => 1.0 / r.max(0.05),
        TimeMode::Follow => pad.bpm.map_or(1.0, |b| b / bpm.max(1.0)),
        TimeMode::Bars(bars) => {
            if seconds > 0.0 {
                bars * bar_beats * 60.0 / bpm.max(1.0) / seconds
            } else {
                1.0
            }
        }
    };
    if f.is_finite() {
        f.clamp(0.05, 20.0)
    } else {
        1.0
    }
}

/// What the engine plays for a pad.
pub fn pad_params(pad: &Pad, volume: f32) -> PadParams {
    let semis = pad.semis as f32 + pad.cents as f32 / 100.0;
    // A pad whose length is changed is stretched, with its pitch done there too.
    let stretched = pad.mode == PitchMode::Stretch || pad.time != TimeMode::Off;
    PadParams {
        level: f32::from(pad.level) / 127.0 * volume * 1.25,
        pan: pad.pan,
        pitch: if stretched { 0.0 } else { semis },
        gate: pad.gate,
        looping: pad.looping,
        choke: pad.choke,
        attack: pad.attack,
        decay: pad.decay,
        sustain: pad.sustain,
        release: pad.release,
        cutoff: pad.cutoff,
        resonance: pad.resonance,
        vel_sens: pad.vel_sens,
        fx: pad.fx,
    }
}

/// How a pad's sound is made: the source and everything done to it.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderSpec {
    pub path: String,
    pub edit: SliceEdit,
    /// Length factor and pitch for Stretch (`None`: played as cut).
    pub stretch: Option<(f64, f32)>,
}

impl RenderSpec {
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{self:?}").hash(&mut h);
        h.finish().max(1)
    }
}

/// The spec of pad `k` (`frames`, `rate`: its source's, once decoded).
pub fn render_spec(
    s: &SamplerState,
    k: usize,
    frames: u64,
    rate: u32,
    bpm: f64,
    bar_beats: f64,
) -> Option<RenderSpec> {
    let pad = s.pads.get(k)?;
    let src = s.sources.get(pad.source?)?;
    let (start, end) = pad_range(pad, frames);
    let seconds = (end - start) as f64 / f64::from(rate.max(1));
    let factor = length_factor(pad, seconds, bpm, bar_beats);
    let semis = pad.semis as f32 + pad.cents as f32 / 100.0;
    let stretched = pad.mode == PitchMode::Stretch || pad.time != TimeMode::Off;
    Some(RenderSpec {
        path: src.path.clone(),
        edit: SliceEdit {
            start: start as usize,
            end: end as usize,
            gain_db: pad.gain_db,
            reverse: pad.reverse,
            fade_in: pad.fade_in,
            fade_out: pad.fade_out,
            normalize: pad.normalize,
            grade: pad.grade,
            stereo: pad.stereo,
        },
        stretch: stretched.then_some(((factor * 1e6).round() / 1e6, semis)),
    })
}

/// A pad's sound, made from its decoded source.
pub fn render(src: &SampleBuf, spec: &RenderSpec) -> SampleBuf {
    let cut = sample_tools::render_slice(src, &spec.edit);
    match spec.stretch {
        Some((factor, semis)) if (factor - 1.0).abs() > 1e-6 || semis.abs() > 1e-4 => {
            sample_tools::stretch(&cut, factor, semis)
        }
        _ => cut,
    }
}

/// The note map the engine uses.
pub fn key_map(s: &SamplerState) -> prowave_engine::KeyMap {
    match s.chromatic {
        Some(pad) => prowave_engine::chromatic_keys(pad as u8, s.root),
        None => prowave_engine::pad_keys(s.base_note, PADS),
    }
}

/// The pad a MIDI note plays (`None`: no pad), and the semitones on top.
#[cfg(test)]
pub fn pad_for_note(s: &SamplerState, note: u8) -> Option<(usize, i8)> {
    let (pad, semis) = key_map(s)[usize::from(note & 0x7f)];
    (pad != prowave_engine::NO_PAD).then_some((usize::from(pad), semis))
}

/// The note that plays pad `k` (pad mode).
pub fn note_for_pad(s: &SamplerState, k: usize) -> Option<u8> {
    let n = usize::from(s.base_note) + k;
    (n < 128).then_some(n as u8)
}

// ------------------------------------------------------------------ the running side

/// A decoded file, or how far its decoding is.
pub enum Decoded {
    Loading(Arc<AtomicU32>),
    Ready(Arc<SampleBuf>),
    Failed(String),
}

/// What the engine was last sent for one sampler.
struct Link {
    remote: SamplerRemote,
    pads: Vec<Option<PadParams>>,
    bufs: Vec<u64>,
    keys: Option<prowave_engine::KeyMap>,
    fx: Option<FxSettings>,
}

enum Done {
    Decoded(String, Result<Arc<SampleBuf>, String>),
    /// The first seconds of a file, for a preview that should not wait for the rest.
    Head(String, Arc<SampleBuf>),
    Rendered(u64, Arc<SampleBuf>),
    Info(PathBuf, FileInfo),
}

/// What the browser shows of a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileInfo {
    pub seconds: Option<f64>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub failed: bool,
}

/// The decoded files, the prepared pads, and the samplers' links: shared by every sampler and
/// the windows, on the UI thread.
pub struct Runtime {
    links: HashMap<u64, Link>,
    pub decoded: HashMap<String, Decoded>,
    pub heads: HashMap<String, Arc<SampleBuf>>,
    renders: HashMap<u64, Arc<SampleBuf>>,
    rendering: HashSet<u64>,
    pub info: HashMap<PathBuf, FileInfo>,
    analysing: HashSet<PathBuf>,
    analysis_queue: Option<Sender<PathBuf>>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// Kept so a stopped app can tell background decodes to stop.
    pub cancel: Arc<AtomicBool>,
    /// Prepared sounds asked for outside the pads (a synced preview), newest last.
    keep: Vec<u64>,
    /// Files a window shows or previews this frame: not let go of.
    pinned: HashSet<String>,
}

impl Default for Runtime {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            links: HashMap::new(),
            decoded: HashMap::new(),
            heads: HashMap::new(),
            renders: HashMap::new(),
            rendering: HashSet::new(),
            info: HashMap::new(),
            analysing: HashSet::new(),
            analysis_queue: None,
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            keep: Vec::new(),
            pinned: HashSet::new(),
        }
    }
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

/// Runs `f` with the shared runtime.
pub fn with_runtime<R>(f: impl FnOnce(&mut Runtime) -> R) -> R {
    let mut guard = match RUNTIME.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(guard.get_or_insert_with(Runtime::default))
}

/// A new engine instance for the slot `uid` (its track was built again): everything is sent
/// to it afresh.
pub fn attach(uid: u64, remote: SamplerRemote) {
    with_runtime(|rt| {
        rt.links.insert(
            uid,
            Link {
                remote,
                pads: vec![None; PADS],
                bufs: vec![0; PADS],
                keys: None,
                fx: None,
            },
        );
    });
}

impl Runtime {
    /// The decoded file at `path`, starting its decode if it has not been.
    pub fn source(&mut self, path: &str) -> Option<Arc<SampleBuf>> {
        match self.decoded.get(path) {
            Some(Decoded::Ready(b)) => return Some(b.clone()),
            Some(_) => return None,
            None => {}
        }
        if path.is_empty() {
            return None;
        }
        let progress = Arc::new(AtomicU32::new(0));
        self.decoded
            .insert(path.to_string(), Decoded::Loading(progress.clone()));
        let (tx, p, cancel) = (self.tx.clone(), path.to_string(), self.cancel.clone());
        std::thread::spawn(move || {
            let head_tx = tx.clone();
            let head_path = p.clone();
            let r = sample_tools::decode_with_head(
                Path::new(&p),
                3.0,
                |head| {
                    let _ = head_tx.send(Done::Head(head_path.clone(), Arc::new(head)));
                },
                |f| {
                    progress.store(f.to_bits(), Ordering::Relaxed);
                    !cancel.load(Ordering::Relaxed)
                },
            );
            let _ = tx.send(Done::Decoded(p, r.map(Arc::new)));
        });
        None
    }

    /// How far the file's decoding is (0 to 1), while it is going on.
    pub fn progress(&self, path: &str) -> Option<f32> {
        match self.decoded.get(path) {
            Some(Decoded::Loading(p)) => Some(f32::from_bits(p.load(Ordering::Relaxed))),
            _ => None,
        }
    }

    pub fn error(&self, path: &str) -> Option<&str> {
        match self.decoded.get(path) {
            Some(Decoded::Failed(e)) => Some(e),
            _ => None,
        }
    }

    /// Asks for the file's length, tempo and key (one file at a time, in the background).
    pub fn analyse(&mut self, path: &Path) {
        if self.info.contains_key(path) || !self.analysing.insert(path.to_path_buf()) {
            return;
        }
        if self.analysis_queue.is_none() {
            let (qtx, qrx) = channel::<PathBuf>();
            let tx = self.tx.clone();
            std::thread::Builder::new()
                .name("sampler-analysis".into())
                .spawn(move || {
                    while let Ok(path) = qrx.recv() {
                        let info = analyse_file(&path);
                        if tx.send(Done::Info(path, info)).is_err() {
                            break;
                        }
                    }
                })
                .ok();
            self.analysis_queue = Some(qtx);
        }
        if let Some(q) = &self.analysis_queue {
            let _ = q.send(path.to_path_buf());
        }
    }

    /// Takes the finished background work. Returns whether anything arrived.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok(done) = self.rx.try_recv() {
            any = true;
            match done {
                Done::Decoded(p, Ok(b)) => {
                    self.heads.remove(&p);
                    self.decoded.insert(p, Decoded::Ready(b));
                }
                Done::Decoded(p, Err(e)) => {
                    self.heads.remove(&p);
                    self.decoded.insert(p, Decoded::Failed(e));
                }
                Done::Head(p, b) => {
                    if matches!(self.decoded.get(&p), Some(Decoded::Loading(_))) {
                        self.heads.insert(p, b);
                    }
                }
                Done::Rendered(k, b) => {
                    self.rendering.remove(&k);
                    self.renders.insert(k, b);
                }
                Done::Info(p, i) => {
                    self.analysing.remove(&p);
                    self.info.insert(p, i);
                }
            }
        }
        any
    }

    /// Whether background work is still going (the window keeps redrawing then).
    pub fn busy(&self) -> bool {
        !self.rendering.is_empty()
            || !self.analysing.is_empty()
            || self
                .decoded
                .values()
                .any(|d| matches!(d, Decoded::Loading(_)))
    }

    fn render_job(&mut self, key: u64, src: Arc<SampleBuf>, spec: RenderSpec) {
        if self.renders.contains_key(&key) || !self.rendering.insert(key) {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let b = render(&src, &spec);
            let _ = tx.send(Done::Rendered(key, Arc::new(b)));
        });
    }

    /// The sound `spec` makes from `src`: ready, or started in the background (`None`).
    pub fn prepared(&mut self, spec: &RenderSpec, src: Arc<SampleBuf>) -> Option<Arc<SampleBuf>> {
        let key = spec.key();
        if !self.keep.contains(&key) {
            self.keep.push(key);
            if self.keep.len() > 4 {
                self.keep.remove(0);
            }
        }
        if let Some(b) = self.renders.get(&key) {
            return Some(b.clone());
        }
        self.render_job(key, src, spec.clone());
        None
    }

    /// Whether the sound `spec` makes is ready.
    pub fn has_prepared(&self, spec: &RenderSpec) -> bool {
        self.renders.contains_key(&spec.key())
    }

    /// Keeps the decoded `path` while a window uses it (call every frame).
    pub fn pin(&mut self, path: &str) {
        self.pinned.insert(path.to_string());
    }

    /// The engine end of sampler `uid`, if it is running.
    pub fn remote(&mut self, uid: u64) -> Option<&mut SamplerRemote> {
        self.links.get_mut(&uid).map(|l| &mut l.remote)
    }

    /// Whether pad `k` of sampler `uid` is sounding.
    pub fn is_playing(&self, uid: u64, k: usize) -> bool {
        self.links
            .get(&uid)
            .is_some_and(|l| l.remote.status.is_playing(k))
    }

    pub fn preview_at(&self, uid: u64) -> Option<u64> {
        self.links.get(&uid)?.remote.status.preview_at()
    }
}

/// Length, tempo and key of a file (the browser's columns). Background thread.
pub fn analyse_file(path: &Path) -> FileInfo {
    match sample_tools::decode(path, |_| true) {
        Ok(b) => FileInfo {
            seconds: Some(b.seconds()),
            bpm: sample_tools::estimate_bpm(&b).map(|x| (x * 10.0).round() / 10.0),
            key: sample_tools::estimate_key(&b),
            failed: false,
        },
        Err(_) => FileInfo {
            failed: true,
            ..FileInfo::default()
        },
    }
}

/// Once a frame: every sampler in the project gets what its settings say, as far as its
/// sounds are ready. Returns whether to keep redrawing (work in progress).
pub fn tick(p: &Project) -> bool {
    let bpm = project_bpm(p);
    let bar = beats_per_bar(p);
    with_runtime(|rt| {
        rt.poll();
        let mut wanted: HashSet<u64> = HashSet::new();
        let mut live: HashSet<u64> = HashSet::new();
        for t in &p.tracks {
            let Some(slot) = t.au_instrument.as_ref().filter(|s| is_sampler(s)) else {
                continue;
            };
            live.insert(slot.uid);
            let s = decode(&slot.state);
            // The pads' sounds.
            let mut bufs: Vec<Option<(u64, Option<Arc<SampleBuf>>)>> = vec![None; PADS];
            for (k, pad) in s.pads.iter().enumerate() {
                let Some(src) = pad.source.and_then(|i| s.sources.get(i)) else {
                    bufs[k] = Some((0, None));
                    continue;
                };
                let Some(data) = rt.source(&src.path) else {
                    continue;
                };
                let Some(spec) = render_spec(&s, k, data.frames() as u64, data.rate, bpm, bar)
                else {
                    continue;
                };
                let key = spec.key();
                wanted.insert(key);
                match rt.renders.get(&key) {
                    Some(b) => bufs[k] = Some((key, Some(b.clone()))),
                    None => rt.render_job(key, data, spec),
                }
            }
            let Some(link) = rt.links.get_mut(&slot.uid) else {
                continue;
            };
            link.remote.collect();
            let mut fx = s.fx;
            fx.bpm = bpm as f32;
            if link.fx != Some(fx) && link.remote.send(SamplerMsg::Fx(fx)).is_ok() {
                link.fx = Some(fx);
            }
            let keys = key_map(&s);
            if link.keys != Some(keys) && link.remote.send(SamplerMsg::Keys(keys)).is_ok() {
                link.keys = Some(keys);
            }
            for (k, pad) in s.pads.iter().enumerate() {
                let params = pad_params(pad, s.volume);
                if link.pads[k] != Some(params)
                    && link
                        .remote
                        .send(SamplerMsg::Pad {
                            pad: k as u8,
                            params,
                        })
                        .is_ok()
                {
                    link.pads[k] = Some(params);
                }
                if let Some((key, buf)) = bufs[k].take() {
                    if link.bufs[k] != key
                        && link
                            .remote
                            .send(SamplerMsg::Buffer { pad: k as u8, buf })
                            .is_ok()
                    {
                        link.bufs[k] = key;
                    }
                }
            }
        }
        // Instruments that are gone, and sounds nothing uses any more.
        rt.links
            .retain(|uid, l| live.contains(uid) && !l.remote.is_abandoned());
        let keep = rt.keep.clone();
        rt.renders
            .retain(|k, _| wanted.contains(k) || keep.contains(k));
        // Decoded files nothing uses (a song is tens of megabytes): let go.
        let mut used: HashSet<String> = std::mem::take(&mut rt.pinned);
        for t in &p.tracks {
            if let Some(slot) = t.au_instrument.as_ref().filter(|s| is_sampler(s)) {
                used.extend(decode(&slot.state).sources.into_iter().map(|s| s.path));
            }
        }
        rt.decoded
            .retain(|path, d| !matches!(d, Decoded::Ready(_)) || used.contains(path));
        rt.busy()
    })
}

// ------------------------------------------------------------------ editing

/// The track whose Sampler has slot `uid`.
pub fn track_of(p: &Project, uid: u64) -> Option<usize> {
    p.tracks.iter().position(|t| {
        t.au_instrument
            .as_ref()
            .is_some_and(|s| s.uid == uid && is_sampler(s))
    })
}

/// The settings of sampler `uid`.
pub fn state_of(p: &Project, uid: u64) -> Option<SamplerState> {
    let t = track_of(p, uid)?;
    Some(decode(&p.tracks[t].au_instrument.as_ref()?.state))
}

/// Stores new settings for sampler `uid`.
pub fn store(p: &mut Project, uid: u64, s: &SamplerState) {
    if let Some(t) = track_of(p, uid) {
        if let Some(slot) = p.tracks[t].au_instrument.as_mut() {
            let bytes = encode(s);
            if slot.state.as_deref() != Some(&bytes[..]) {
                slot.state = Some(bytes);
            }
        }
    }
}

/// The source index of `path`, added if new.
pub fn add_source(s: &mut SamplerState, path: &str) -> usize {
    if let Some(i) = s.sources.iter().position(|x| x.path == path) {
        return i;
    }
    s.sources.push(Source {
        path: path.to_string(),
        ..Source::default()
    });
    s.sources.len() - 1
}

fn stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Puts frames `start..end` of `path` on pad `k` (the SP-303 records onto a pad; here a file
/// or part of one is "sampled" onto it). Its tempo is worked out from its length, as the
/// SP-303 does (manual pp. 29–30), unless the file's tempo is known and fits.
pub fn assign(s: &mut SamplerState, k: usize, path: &str, start: u64, end: u64, rate: u32) {
    if k >= PADS {
        return;
    }
    let src = add_source(s, path);
    let seconds = end.saturating_sub(start) as f64 / f64::from(rate.max(1));
    let pad = &mut s.pads[k];
    let keep_fx = pad.fx;
    *pad = Pad {
        source: Some(src),
        name: stem(path),
        start,
        end,
        bpm: sample_tools::loop_bpm(seconds),
        fx: keep_fx,
        ..Pad::default()
    };
}

/// The slices between `points` (and the pad's own start and end) onto pads `first`, `first +
/// 1`, ... in order (an extension: the SP-303 has no chop). Returns how many were made.
pub fn slices_to_pads(
    s: &mut SamplerState,
    from_pad: usize,
    first: usize,
    points: &[u64],
    rate: u32,
    frames: u64,
) -> usize {
    let Some(pad) = s.pads.get(from_pad).cloned() else {
        return 0;
    };
    let Some(src) = pad.source else { return 0 };
    let (start, end) = pad_range(&pad, frames);
    let mut cuts: Vec<u64> = points
        .iter()
        .copied()
        .filter(|x| *x > start && *x < end)
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut bounds = vec![start];
    bounds.extend(cuts);
    bounds.push(end);
    let path = s.sources[src].path.clone();
    let name = stem(&path);
    let mut made = 0;
    for (n, w) in bounds.windows(2).enumerate() {
        let k = first + n;
        if k >= PADS {
            break;
        }
        assign(s, k, &path, w[0], w[1], rate);
        s.pads[k].name = format!("{name} {}", n + 1);
        // The slices keep the chopped pad's playing settings.
        let fresh = s.pads[k].clone();
        s.pads[k] = Pad {
            source: fresh.source,
            name: fresh.name,
            start: fresh.start,
            end: fresh.end,
            bpm: fresh.bpm,
            ..pad.clone()
        };
        made += 1;
    }
    made
}

/// A pad's name as a pad shows it: bank letter and number (A1–H8).
pub fn pad_label(k: usize) -> String {
    let bank = (b'A' + (k / PADS_PER_BANK) as u8) as char;
    format!("{bank}{}", k % PADS_PER_BANK + 1)
}

/// "Slices to MIDI pattern" (the MPC's Convert to pattern): a MIDI region on track `t` at the
/// playhead that plays the pads cut from pad `k`'s source in their original order at their
/// original places. The places are in beats at the source's tempo when it is known (the
/// groove lands on the project's grid), else at the project tempo (the same seconds). Returns
/// the region's index.
pub fn slices_to_pattern(p: &mut Project, t: usize, s: &SamplerState, k: usize) -> Option<usize> {
    let src = s.pads.get(k)?.source?;
    let mut slices: Vec<(usize, &Pad)> = s
        .pads
        .iter()
        .enumerate()
        .filter(|(_, pad)| pad.source == Some(src))
        .collect();
    slices.sort_by_key(|(_, pad)| pad.start);
    let origin = slices.first()?.1.start;
    let rate =
        with_runtime(|rt| rt.source(&s.sources[src].path).map(|b| b.rate)).or_else(|| {
            prowave_engine::AudioFile::open(&s.sources[src].path)
                .ok()
                .map(|f| f.sample_rate)
        })?;
    let bpm = s.sources[src].bpm.unwrap_or_else(|| project_bpm(p));
    let notes = pattern_notes(&slices, origin, rate, bpm, |k| note_for_pad(s, k));
    let bar = beats_per_bar(p);
    let total = notes
        .iter()
        .map(|n| f64::from(n.start + n.len))
        .fold(0.0, f64::max);
    let start_bar = p.beats_to_bar(p.position_beats) + 1.0;
    let len_bars = (total / bar).ceil().max(1.0);
    p.tracks[t].regions.push(Region {
        name: format!("{} slices", stem(&s.sources[src].path)),
        start_bar: start_bar as f32,
        len_bars: len_bars as f32,
        notes,
        controls: Vec::new(),
        folder: None,
        generated: None,
        audio: None,
        params: RegionParams::default(),
    });
    p.song_rev += 1;
    Some(p.tracks[t].regions.len() - 1)
}

/// The notes of [`slices_to_pattern`]: each slice where it was in the source (seconds from
/// `origin`, in beats at `bpm`), as long as it is.
pub fn pattern_notes(
    slices: &[(usize, &Pad)],
    origin: u64,
    rate: u32,
    bpm: f64,
    note: impl Fn(usize) -> Option<u8>,
) -> Vec<Note> {
    let beats = |frames: u64| frames as f64 / f64::from(rate.max(1)) * bpm / 60.0;
    slices
        .iter()
        .filter_map(|(k, pad)| {
            let len = pad.end.saturating_sub(pad.start);
            Some(Note {
                lyric: crate::lyrics::Lyric::NONE,
                start: beats(pad.start.saturating_sub(origin)) as f32,
                len: (beats(len) as f32).max(1.0 / 64.0),
                pitch: note(*k)?,
                vel: 100,
                mute: false,
            })
        })
        .collect()
}

/// "Slice to audio track": the pad's sound written as a WAV into the project's audio folder
/// and placed on a new audio track exactly at the playhead. Returns the new track.
pub fn slice_to_audio_track(
    p: &mut Project,
    buf: &SampleBuf,
    name: &str,
    dir: &Path,
    ctx: &egui::Context,
) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = dir.join(format!("{}-{stamp}.wav", safe.trim()));
    prowave_engine::write_wav(&path, &buf.data, buf.rate).map_err(|e| e.to_string())?;
    let file = prowave_engine::AudioFile::open(&path).map_err(|e| e.to_string())?;
    let start = p.position_beats;
    let end = p.tempo_map().advance(start, file.duration_secs());
    let start_bar = p.beats_to_bar(start) + 1.0;
    let len_bars = p.beats_to_bar(end) - p.beats_to_bar(start);
    let mut track = Track::new(name, TrackKind::Audio);
    track.regions = vec![Region {
        name: name.to_string(),
        start_bar: start_bar as f32,
        len_bars: len_bars as f32,
        notes: Vec::new(),
        controls: Vec::new(),
        folder: None,
        generated: None,
        audio: Some(crate::import::audio_region(file, ctx)),
        params: RegionParams::default(),
    }];
    p.tracks.push(track);
    p.song_rev += 1;
    Ok(p.tracks.len() - 1)
}

// ------------------------------------------------------------------ patterns

/// The region that holds pattern `n` (0–31) on track `t`.
pub fn pattern_region(p: &Project, t: usize, n: usize) -> Option<usize> {
    let name = format!("{PATTERN_PREFIX}{}", pad_label(n));
    p.tracks.get(t)?.regions.iter().position(|r| r.name == name)
}

/// Pattern `n`'s region, made (empty, `bars` long, after the track's last region) if missing.
pub fn ensure_pattern(p: &mut Project, t: usize, n: usize, bars: u32) -> usize {
    if let Some(r) = pattern_region(p, t, n) {
        return r;
    }
    let after = p.tracks[t]
        .regions
        .iter()
        .map(|r| r.start_bar + r.len_bars)
        .fold(1.0f32, f32::max);
    p.tracks[t].regions.push(Region {
        name: format!("{PATTERN_PREFIX}{}", pad_label(n)),
        start_bar: after.ceil(),
        len_bars: bars.clamp(1, 99) as f32,
        notes: Vec::new(),
        controls: Vec::new(),
        folder: None,
        generated: None,
        audio: None,
        params: RegionParams::default(),
    });
    p.song_rev += 1;
    p.tracks[t].regions.len() - 1
}

/// A pad hit while a pattern records (manual pp. 44–45: recording loops over the pattern and
/// overdubs, quantized): the note goes in at the song position, folded into the region and
/// moved to the grid.
pub fn record_hit(p: &mut Project, t: usize, r: usize, pitch: u8, q: Quantize) {
    let (start, len) = {
        let region = &p.tracks[t].regions[r];
        (p.region_start_beats(region), p.region_len_beats(region))
    };
    if len <= 0.0 {
        return;
    }
    let mut at = (p.position_beats - start).rem_euclid(len);
    if let Some(g) = q.beats() {
        at = ((at / g).round() * g).rem_euclid(len);
    }
    let note_len = q.beats().unwrap_or(0.25) as f32;
    let notes = &mut p.tracks[t].regions[r].notes;
    if notes
        .iter()
        .any(|n| n.pitch == pitch && (f64::from(n.start) - at).abs() < 1e-3)
    {
        return;
    }
    notes.push(Note {
        lyric: crate::lyrics::Lyric::NONE,
        start: at as f32,
        len: note_len,
        pitch,
        vel: 100,
        mute: false,
    });
    notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    p.song_rev += 1;
}

// ------------------------------------------------------------------ the browser's folder

/// The browser's folder until one is picked, under the home folder.
const FIRST_FOLDER: &str = "Music";

/// The browser's folder: the last one picked, else [`FIRST_FOLDER`] in the home folder.
pub fn default_folder() -> Option<PathBuf> {
    let remembered = crate::recent_files::config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("sampler-folder.txt")).ok())
        .map(|t| PathBuf::from(t.trim()))
        .filter(|p| p.is_dir());
    remembered.or_else(|| {
        let home = std::env::var_os("HOME")?;
        let p = Path::new(&home).join(FIRST_FOLDER);
        p.is_dir().then_some(p)
    })
}

pub fn remember_folder(dir: &Path) {
    if cfg!(test) {
        return;
    }
    if let Some(d) = crate::recent_files::config_dir() {
        let _ = std::fs::create_dir_all(&d).and_then(|_| {
            std::fs::write(
                d.join("sampler-folder.txt"),
                dir.to_string_lossy().as_bytes(),
            )
        });
    }
}

/// The audio files under `dir` (and its folders), sorted by name.
pub fn scan(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if stack.len() < 10_000 {
                    stack.push(path);
                }
            } else if is_sample_file(&path) {
                out.push(path);
            }
            if out.len() >= 50_000 {
                break;
            }
        }
    }
    out.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    out
}

pub fn is_sample_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "mp3" | "wav" | "wave" | "aif" | "aiff" | "flac" | "m4a"
        )
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_state() -> SamplerState {
        let mut s = SamplerState::default();
        assign(&mut s, 0, "/records/loop.mp3", 1000, 89_200, 44_100);
        s.sources[0].bpm = Some(90.0);
        s.sources[0].chops = vec![23_050, 45_100];
        let p = &mut s.pads[0];
        p.semis = -3;
        p.cents = 25;
        p.gate = true;
        p.looping = true;
        p.reverse = true;
        p.choke = 2;
        p.mode = PitchMode::Stretch;
        p.time = TimeMode::Bars(2.0);
        p.grade = Grade::LoFi;
        p.stereo = false;
        p.level = 90;
        p.fx = true;
        p.cutoff = 3_000.0;
        s.fx = FxSettings {
            kind: SpFxKind::VinylSim,
            on: true,
            ctrl: [0.7, 0.2, 0.1],
            bpm: 120.0,
        };
        s.chromatic = Some(0);
        s.root = 48;
        s.quantize = Quantize::Q8T;
        s.pattern_len = 4;
        s.consolidate = false;
        s
    }

    #[test]
    fn settings_round_trip_through_the_slot() {
        let s = sample_state();
        let mut back = decode(&Some(encode(&s)));
        back.fx.bpm = s.fx.bpm;
        assert_eq!(back, s);
        assert_eq!(decode(&None), SamplerState::default());
        assert_eq!(decode(&Some(b"nonsense".to_vec())), SamplerState::default());
    }

    #[test]
    fn notes_map_to_pads_through_the_banks_and_chromatic_maps_one_pad() {
        let mut s = SamplerState::default();
        assert_eq!(pad_for_note(&s, 36), Some((0, 0)));
        assert_eq!(pad_for_note(&s, 43), Some((7, 0)), "A8");
        assert_eq!(pad_for_note(&s, 44), Some((8, 0)), "B1");
        assert_eq!(pad_label(8), "B1");
        assert_eq!(pad_for_note(&s, 35), None);
        assert_eq!(note_for_pad(&s, 17), Some(53));
        s.chromatic = Some(5);
        s.root = 60;
        assert_eq!(pad_for_note(&s, 67), Some((5, 7)));
        assert_eq!(pad_for_note(&s, 48), Some((5, -12)));
    }

    #[test]
    fn chopping_puts_the_slices_on_pads_in_order() {
        let mut s = SamplerState::default();
        assign(&mut s, 0, "/r/break.wav", 0, 88_200, 44_100);
        s.pads[0].choke = 1;
        let points: Vec<u64> = sample_tools::equal_divisions(0, 88_200, 4)
            .into_iter()
            .map(|x| x as u64)
            .collect();
        let made = slices_to_pads(&mut s, 0, 0, &points, 44_100, 88_200);
        assert_eq!(made, 4);
        for k in 0..4 {
            assert_eq!(s.pads[k].start, 22_050 * k as u64);
            assert_eq!(s.pads[k].end, 22_050 * (k as u64 + 1));
            assert_eq!(s.pads[k].choke, 1, "kept the pad's settings");
        }
        assert!(s.pads[4].is_empty());
    }

    #[test]
    fn slices_to_pattern_land_on_the_right_ticks_at_two_tempos() {
        // Four slices of a 2-second loop at 120 BPM: one beat each.
        let mut s = SamplerState::default();
        assign(&mut s, 0, "/r/loop.wav", 0, 88_200, 44_100);
        let points: Vec<u64> = sample_tools::equal_divisions(0, 88_200, 4)
            .into_iter()
            .map(|x| x as u64)
            .collect();
        slices_to_pads(&mut s, 0, 0, &points, 44_100, 88_200);
        let slices: Vec<(usize, &Pad)> = s.pads.iter().enumerate().take(4).collect();
        let ticks = |notes: &[Note]| -> Vec<i64> {
            notes
                .iter()
                .map(|n| (f64::from(n.start) * 960.0).round() as i64)
                .collect()
        };
        // At the loop's own tempo (known), the slices are a beat apart at any project tempo.
        let notes = pattern_notes(&slices, 0, 44_100, 120.0, |k| note_for_pad(&s, k));
        assert_eq!(ticks(&notes), vec![0, 960, 1920, 2880]);
        assert_eq!(
            notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            vec![36, 37, 38, 39]
        );
        // Tempo unknown, project at 90: the same half seconds are 0.75 beat apart.
        let notes = pattern_notes(&slices, 0, 44_100, 90.0, |k| note_for_pad(&s, k));
        assert_eq!(ticks(&notes), vec![0, 720, 1440, 2160]);
        assert!((notes[0].len - 0.75).abs() < 1e-6);
        // Through the project: a region at the playhead.
        let mut p = Project::empty();
        p.tempo = 90.0;
        let t = p.add_track(TrackKind::Instrument);
        let mut slot = new_slot();
        slot.state = Some(encode(&s));
        p.tracks[t].au_instrument = Some(slot);
        p.position_beats = 8.0;
        // The source is not a real file: give its rate through the decoded cache.
        with_runtime(|rt| {
            rt.decoded.insert(
                "/r/loop.wav".into(),
                Decoded::Ready(Arc::new(SampleBuf::new(44_100, vec![0.0; 88_200 * 2]))),
            )
        });
        let r = slices_to_pattern(&mut p, t, &s, 2).unwrap();
        let region = &p.tracks[t].regions[r];
        assert_eq!(region.start_bar, 3.0);
        assert_eq!(ticks(&region.notes), vec![0, 720, 1440, 2160]);
    }

    #[test]
    fn a_slice_lands_on_an_audio_track_at_the_playhead_sample() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::empty();
        p.tempo = 93.0;
        p.position_beats = 13.37;
        let buf = SampleBuf::new(48_000, vec![0.25; 4800 * 2]);
        let ctx = egui::Context::default();
        let t = slice_to_audio_track(&mut p, &buf, "chop A3", dir.path(), &ctx).unwrap();
        let region = &p.tracks[t].regions[0];
        let at = p.tempo_map().seconds_at(p.region_start_beats(region)) * 48_000.0;
        let want = p.tempo_map().seconds_at(13.37) * 48_000.0;
        assert!((at - want).abs() < 1.0, "{at} vs {want}");
        let audio = region.audio.as_ref().unwrap();
        assert_eq!(audio.file.frames, 4800);
        assert!(audio.file.path.starts_with(dir.path()));
    }

    #[test]
    fn patterns_record_quantized_and_fold_into_the_loop() {
        let mut p = Project::empty();
        p.tempo = 120.0;
        let t = p.add_track(TrackKind::Instrument);
        let r = ensure_pattern(&mut p, t, 9, 1);
        assert_eq!(pattern_region(&p, t, 9), Some(r));
        assert_eq!(p.tracks[t].regions[r].name, "SP Pattern B2");
        let start = p.region_start_beats(&p.tracks[t].regions[r]);
        // A hit a little after beat 2 of the second pass.
        p.position_beats = start + 4.0 + 2.04;
        record_hit(&mut p, t, r, 36, Quantize::Q16);
        p.position_beats = start + 0.36;
        record_hit(&mut p, t, r, 37, Quantize::Q8T);
        let notes = &p.tracks[t].regions[r].notes;
        assert_eq!(notes.len(), 2);
        assert!(
            (notes[0].start - 1.0 / 3.0).abs() < 1e-6,
            "{}",
            notes[0].start
        );
        assert_eq!(notes[1].start, 2.0);
    }

    #[test]
    fn length_follows_the_tempo_when_fitted() {
        let mut pad = Pad {
            time: TimeMode::Bars(1.0),
            ..Pad::default()
        };
        // A 2.5 s loop to one bar at 120 (2 s): 0.8 as long.
        assert!((length_factor(&pad, 2.5, 120.0, 4.0) - 0.8).abs() < 1e-9);
        pad.time = TimeMode::Follow;
        pad.bpm = Some(90.0);
        assert!((length_factor(&pad, 2.5, 120.0, 4.0) - 0.75).abs() < 1e-9);
        pad.time = TimeMode::Ratio(1.25);
        assert!((length_factor(&pad, 2.5, 120.0, 4.0) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn sources_are_copied_into_the_project_and_found_again() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("break.wav");
        crate::project_io::tests::write_wav(&wav);
        let mut s = SamplerState::default();
        assign(&mut s, 0, &wav.to_string_lossy(), 0, 100, 44_100);
        let state = Some(encode(&s));
        let saved = state_for_save(&state, |_| "media/3_break.wav".into());
        assert_eq!(decode(&saved).sources[0].path, "media/3_break.wav");
        let media = dir.path().join("x");
        let loaded = state_after_load(&saved, &media);
        assert_eq!(
            PathBuf::from(&decode(&loaded).sources[0].path),
            media.join("media/3_break.wav")
        );
        // Off: the paths stay.
        s.consolidate = false;
        let state = Some(encode(&s));
        assert_eq!(state_for_save(&state, |_| "x".into()), state);
    }

    /// The browser on a real folder of records (read only): every file is listed (pictures
    /// skipped), names come through whole, a preview has sound within 100 ms of the click,
    /// and the tempo of every file is found in the background while the UI thread's polls
    /// stay short. Run with `cargo test -p prowave-app --release sampler_folder -- --ignored
    /// --nocapture` (and `PROWAVE_SAMPLE_FOLDER` for another folder).
    #[test]
    #[ignore]
    fn sampler_folder_browses_previews_and_analyses() {
        use std::time::{Duration, Instant};
        let dir = std::env::var("PROWAVE_SAMPLE_FOLDER")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(FIRST_FOLDER)
            });
        if !dir.is_dir() {
            eprintln!("{} is not here", dir.display());
            return;
        }
        let all = std::fs::read_dir(&dir).unwrap().count();
        let files = scan(&dir);
        eprintln!("{} entries, {} audio files listed", all, files.len());
        assert!(files.iter().all(|f| is_sample_file(f)));
        let longest = files
            .iter()
            .map(|f| f.file_stem().unwrap().to_string_lossy().chars().count())
            .max()
            .unwrap_or(0);
        eprintln!("longest name: {longest} characters");
        let mut rt = Runtime::default();
        // Preview latency: from the click (asking for the file) to a buffer to play.
        let mut waits = Vec::new();
        for f in files.iter().take(10) {
            let key = f.to_string_lossy().into_owned();
            let t = Instant::now();
            rt.source(&key);
            loop {
                rt.poll();
                if rt.heads.contains_key(&key)
                    || matches!(rt.decoded.get(&key), Some(Decoded::Ready(_)))
                {
                    break;
                }
                assert!(t.elapsed() < Duration::from_secs(5), "{key}");
                std::thread::sleep(Duration::from_millis(1));
            }
            waits.push(t.elapsed().as_secs_f64() * 1000.0);
            rt.decoded.clear();
            rt.heads.clear();
        }
        waits.sort_by(f64::total_cmp);
        eprintln!(
            "preview ready after: median {:.1} ms, worst {:.1} ms",
            waits[waits.len() / 2],
            waits[waits.len() - 1]
        );
        assert!(waits[waits.len() - 1] < 100.0);
        // Tempo and key of every file, in the background.
        let t = Instant::now();
        for f in &files {
            rt.analyse(f);
        }
        let mut worst_poll = 0.0f64;
        while rt.info.len() < files.len() {
            let p = Instant::now();
            rt.poll();
            worst_poll = worst_poll.max(p.elapsed().as_secs_f64() * 1000.0);
            std::thread::sleep(Duration::from_millis(16));
            assert!(t.elapsed() < Duration::from_secs(900));
        }
        let failed: Vec<_> = rt.info.iter().filter(|(_, i)| i.failed).collect();
        let with_bpm = rt.info.values().filter(|i| i.bpm.is_some()).count();
        eprintln!(
            "analysed {} files in {:.1} s ({} with a tempo, {} failed); longest UI poll {:.2} ms",
            rt.info.len(),
            t.elapsed().as_secs_f64(),
            with_bpm,
            failed.len(),
            worst_poll
        );
        assert!(failed.is_empty(), "{failed:?}");
        assert!(worst_poll < 5.0);
    }

    /// From the slot's settings to sound: a WAV on a pad is decoded and prepared in the
    /// background, sent to the instrument, and a hit plays it (stretched pads too).
    #[test]
    fn a_pad_plays_its_file_end_to_end() {
        use prowave_engine::BlockProcessor;
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("tone.wav");
        let tone: Vec<f32> = (0..24_000)
            .flat_map(|i| {
                let x = (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin() * 0.5;
                [x, x]
            })
            .collect();
        prowave_engine::write_wav(&wav, &tone, 48_000).unwrap();
        let mut p = Project::empty();
        let t = p.add_track(TrackKind::Instrument);
        let slot = new_slot();
        let uid = slot.uid;
        p.tracks[t].au_instrument = Some(slot);
        let mut s = SamplerState::default();
        let path = wav.to_string_lossy().into_owned();
        assign(&mut s, 0, &path, 12_000, 24_000, 48_000);
        assign(&mut s, 1, &path, 0, 24_000, 48_000);
        s.pads[1].time = TimeMode::Bars(1.0); // 0.5 s to a bar at 120: 2 s
        store(&mut p, uid, &s);
        let (mut inst, remote) = prowave_engine::Sampler::new(48_000);
        attach(uid, remote);
        let (mut l, mut r) = (vec![0f32; 512], vec![0f32; 512]);
        let mut ready = false;
        for _ in 0..500 {
            tick(&p);
            inst.process((&[], &[]), (&mut l, &mut r));
            ready = with_runtime(|rt| {
                rt.links
                    .get(&uid)
                    .is_some_and(|link| link.bufs[0] != 0 && link.bufs[1] != 0)
            });
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ready, "the pads' sounds reached the instrument");
        inst.process((&[], &[]), (&mut l, &mut r));
        inst.midi(0x90, 36, 127, 0);
        let mut out = Vec::new();
        for _ in 0..30 {
            inst.process((&[], &[]), (&mut l, &mut r));
            out.extend_from_slice(&l);
        }
        let sounding = out.iter().rposition(|x| x.abs() > 1e-3).unwrap_or(0);
        assert!(
            (sounding as i64 - 12_000).abs() < 300,
            "half of the file: {sounding}"
        );
        // The stretched pad is four times as long.
        inst.midi(0x90, 37, 127, 0);
        let mut out = Vec::new();
        for _ in 0..200 {
            inst.process((&[], &[]), (&mut l, &mut r));
            out.extend_from_slice(&l);
        }
        let sounding = out.iter().rposition(|x| x.abs() > 1e-3).unwrap_or(0);
        assert!(
            (sounding as f64 / 96_000.0 - 1.0).abs() < 0.01,
            "{sounding}"
        );
    }

    #[test]
    fn the_folder_scan_finds_audio_and_skips_pictures() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        for name in [
            "Bobby's Song - The Kavettes - Part 1.mp3",
            "cover.jpg",
            "sub/b.WAV",
            ".hidden.wav",
            "c.flac",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let found: Vec<String> = scan(dir.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            found,
            vec![
                "Bobby's Song - The Kavettes - Part 1.mp3",
                "c.flac",
                "b.WAV"
            ]
        );
    }
}
