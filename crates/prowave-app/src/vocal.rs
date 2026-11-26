//! Vocal Top Line (Track > New Vocal Top Line Track): an instrument track whose notes and
//! lyrics (see `lyrics.rs`) are sung by a DiffSinger voice, CHIMERA or Novox The Robot, and the
//! audio track
//! under it that holds what was sung, one audio region per MIDI region, starting exactly where
//! it starts.
//!
//! The singing is made offline from a score in seconds read off the tempo map (tempo changes
//! and curves included). CHIMERA, a physical model of the voice (`chimera_singer.rs`), and
//! Novox The Robot, a formant synthesizer (`novox_singer.rs`), are sung in this program on the
//! render thread.
//! A DiffSinger voice is sung by `tools/vocal/engine` (OpenUtau's DiffSinger runtime with no
//! window, installed by `tools/vocal/setup.sh` with a voice the user downloads) on this Mac.
//! The notes are cut into phrases at rests; each phrase
//! is sung once and kept in a cache under its settings and notes, so an edit sings again only
//! the phrases it touched. Whenever the notes, lyrics, tempo or settings change, the track is
//! sung again in the background after a short pause (the status line shows the progress).
//!
//! Logic Pro has no equivalent; the nearest are Synthesizer V and ACE Studio in a DAW.
use crate::hw;
use crate::i18n::{tr, trf};
use crate::lyrics;
use crate::model::{Project, Region, RegionParams, Track, TrackKind};
use crate::spectrogram_view::hw_vocal;
use crate::theme::{self, Face};
use prowave_engine::TempoMap;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Bumped when the singer or the score changes in a way that makes cached phrases stale.
pub const ENGINE_REV: u64 = 1;
/// The singer's output rate.
pub const SAMPLE_RATE: u32 = 44_100;
/// Time sung before a phrase's first note (its first consonant comes before the beat).
pub const PREROLL: f64 = 0.5;
/// A rest at least this long (seconds) starts a new phrase.
pub const GAP: f64 = 0.3;
/// Notes shorter than this (seconds) are left out.
const MIN_NOTE: f64 = 0.01;
/// How long the notes must stay unchanged before they are sung again.
pub(crate) const SETTLE: Duration = Duration::from_millis(450);
/// The phrase cache is trimmed (oldest first) to this size.
const CACHE_BYTES: u64 = 256 << 20;
/// The singer program, under [`VOCAL_DIR`].
const ENGINE_BIN: &str = "engine/OpenUtau.Test";

/// Who sings a track.
// `DiffSinger` is the engine's own name, not the enum's suffix.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Singer {
    /// An installed DiffSinger voice (tools/vocal/setup.sh), sung by the external engine.
    #[default]
    DiffSinger,
    /// The CHIMERA physical model, in this program.
    Chimera,
    /// CHIMERA as it was before the engine's update (upstream commit 09fb776): the same
    /// settings, the earlier sound.
    Chimera09fb776,
    /// Novox The Robot, a formant synthesizer, in this program.
    Novox,
}

impl Singer {
    pub const ALL: [Singer; 4] = [
        Singer::DiffSinger,
        Singer::Chimera,
        Singer::Novox,
        Singer::Chimera09fb776,
    ];
    /// What a saved name nobody knows is, and a new track's singer: CHIMERA, which needs
    /// nothing installed (DiffSinger needs tools/vocal/setup.sh and a voice).
    const UNKNOWN: Singer = Singer::Chimera;

    pub fn name(self) -> &'static str {
        match self {
            Singer::DiffSinger => "diffsinger",
            Singer::Chimera => "chimera",
            Singer::Chimera09fb776 => "chimera-09fb776",
            Singer::Novox => "novox",
        }
    }

    /// Either copy of the CHIMERA engine (they share their settings).
    pub fn is_chimera(self) -> bool {
        matches!(self, Singer::Chimera | Singer::Chimera09fb776)
    }

    /// Sung in this program, on the render thread (not by DiffSinger's external engine).
    pub fn in_process(self) -> bool {
        self != Singer::DiffSinger
    }

    /// The saved name; an empty or unknown one (files from before CHIMERA) is DiffSinger.
    pub fn from_name(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|x| x.name() == s)
            .unwrap_or(Self::UNKNOWN)
    }

    /// The engines' own names (not translated).
    pub fn label(self) -> &'static str {
        match self {
            Singer::DiffSinger => "DiffSinger",
            Singer::Chimera => "CHIMERA",
            Singer::Chimera09fb776 => "CHIMERA (09fb776)",
            Singer::Novox => "Novox The Robot",
        }
    }
}

/// Where a track is sung.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Place {
    /// Whichever has been measured faster.
    #[default]
    Auto,
    ThisMac,
}

impl Place {
    pub const ALL: [Place; 2] = [Place::Auto, Place::ThisMac];

    pub fn name(self) -> &'static str {
        match self {
            Place::Auto => "auto",
            Place::ThisMac => "mac",
        }
    }

    pub fn from_name(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Place::Auto => tr("Fastest"),
            Place::ThisMac => tr("This Mac"),
        }
    }
}

/// How a Vocal Top Line track sings.
#[derive(Clone, PartialEq, Debug)]
pub struct VocalSettings {
    /// Ties the track to the audio track holding its singing.
    pub link: u64,
    /// The voice (its folder name among the installed DiffSinger voices).
    pub voice: String,
    /// The voice's colour (a DiffSinger subbank, such as "Power" or "Soft"); empty for its first.
    pub color: String,
    /// The lyrics' language: en, ja, es, de, fr, ko or zh.
    pub language: String,
    /// -100..100 each; 0 leaves the voice as it is. Voices support some of them only.
    pub gender: f32,
    pub breathiness: f32,
    pub tension: f32,
    /// 0..100 (100 = fully voiced).
    pub voicing: f32,
    /// Consonant speed, 0..200 (100 = as recorded).
    pub velocity: f32,
    /// Vibrato on held notes: depth in cents (0 = none) and rate in Hz.
    pub vibrato_depth: f32,
    pub vibrato_rate: f32,
    /// How long the pitch glides from one note to the next, in milliseconds.
    pub transition_ms: f32,
    pub place: Place,
    /// Who sings. The settings above it are DiffSinger's, except vibrato depth and rate, which
    /// CHIMERA takes too; the ones below are CHIMERA's, then Novox's.
    pub singer: Singer,
    /// CHIMERA's body: 0..100 toward the female anatomy and toward the child's (0 and 0 is the
    /// adult male's).
    pub body_female: f32,
    pub body_child: f32,
    /// 0..100 each.
    pub chimera_breathiness: f32,
    pub brightness: f32,
    pub ring: f32,
    /// Novox The Robot's voice: its preset and the knobs over it.
    pub novox: crate::novox_singer::Voice,
}

impl VocalSettings {
    pub fn new(link: u64) -> Self {
        Self {
            link,
            voice: "Hanami".into(),
            color: String::new(),
            language: "en".into(),
            gender: 0.0,
            breathiness: 0.0,
            tension: 0.0,
            voicing: 100.0,
            velocity: 100.0,
            vibrato_depth: 25.0,
            vibrato_rate: 5.5,
            transition_ms: 80.0,
            place: Place::Auto,
            singer: Singer::UNKNOWN,
            body_female: 0.0,
            body_child: 0.0,
            // CHIMERA's own defaults (its source's breathiness 0.15 and brightness 0.25, its
            // neutral posture's epilarynx 0).
            chimera_breathiness: 15.0,
            brightness: 25.0,
            ring: 0.0,
            novox: crate::novox_singer::Voice::new(),
        }
    }

    /// How CHIMERA sings with these settings.
    pub fn chimera_voice(&self) -> crate::chimera_singer::Voice {
        crate::chimera_singer::Voice {
            engine: match self.singer {
                Singer::Chimera09fb776 => crate::chimera_singer::Engine::V09fb776,
                _ => crate::chimera_singer::Engine::Current,
            },
            female: self.body_female,
            child: self.body_child,
            vibrato_depth: self.vibrato_depth,
            vibrato_rate: self.vibrato_rate,
            breathiness: self.chimera_breathiness,
            brightness: self.brightness,
            ring: self.ring,
        }
    }

    /// Every value held in its range (for settings read from a file).
    pub fn clamped(mut self) -> Self {
        let c = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.gender = c(self.gender, -100.0, 100.0, 0.0);
        self.breathiness = c(self.breathiness, -100.0, 100.0, 0.0);
        self.tension = c(self.tension, -100.0, 100.0, 0.0);
        self.voicing = c(self.voicing, 0.0, 100.0, 100.0);
        self.velocity = c(self.velocity, 0.0, 200.0, 100.0);
        self.vibrato_depth = c(self.vibrato_depth, 0.0, 200.0, 25.0);
        self.vibrato_rate = c(self.vibrato_rate, 3.0, 9.0, 5.5);
        self.transition_ms = c(self.transition_ms, 5.0, 300.0, 80.0);
        self.body_female = c(self.body_female, 0.0, 100.0, 0.0);
        self.body_child = c(self.body_child, 0.0, 100.0, 0.0);
        self.chimera_breathiness = c(self.chimera_breathiness, 0.0, 100.0, 15.0);
        self.brightness = c(self.brightness, 0.0, 100.0, 25.0);
        self.ring = c(self.ring, 0.0, 100.0, 0.0);
        self.novox = self.novox.clamped();
        if !LANGUAGES.iter().any(|(code, _)| *code == self.language) {
            self.language = "en".into();
        }
        self.voice = self.voice.chars().take(128).collect();
        self.color = self.color.chars().take(128).collect();
        self
    }

    /// What the sound depends on (not where it is made): the singer and its settings.
    fn sound_key(&self, h: &mut Fnv) {
        if self.singer == Singer::Novox {
            h.str(self.singer.name());
            h.u64(crate::novox_singer::REV);
            h.str(&self.novox.preset);
            for x in self.novox.key_values() {
                h.u64(x.to_bits() as u64);
            }
            return;
        }
        if self.singer.is_chimera() {
            h.str(self.singer.name());
            h.u64(crate::chimera_singer::REV);
            let v = self.chimera_voice();
            for x in [
                v.female,
                v.child,
                v.vibrato_depth,
                v.vibrato_rate,
                v.breathiness,
                v.brightness,
                v.ring,
            ] {
                h.u64(x.to_bits() as u64);
            }
            return;
        }
        h.str(&self.voice);
        h.str(&self.color);
        h.str(&self.language);
        for v in [
            self.gender,
            self.breathiness,
            self.tension,
            self.voicing,
            self.velocity,
            self.vibrato_depth,
            self.vibrato_rate,
            self.transition_ms,
        ] {
            h.u64(v.to_bits() as u64);
        }
    }
}

/// The languages the singer has phonemizers for.
pub const LANGUAGES: [(&str, &str); 7] = [
    ("en", "English"),
    ("ja", "日本語"),
    ("es", "Español"),
    ("de", "Deutsch"),
    ("fr", "Français"),
    ("ko", "한국어"),
    ("zh", "中文"),
];

/// A track's part in a Vocal Top Line.
#[derive(Clone, PartialEq, Debug)]
pub enum VocalRole {
    /// The notes and lyrics, and how they are sung.
    Score(VocalSettings),
    /// The audio track holding what the score track of this link sang.
    Output { link: u64 },
}

impl VocalRole {
    pub fn link(&self) -> u64 {
        match self {
            VocalRole::Score(s) => s.link,
            VocalRole::Output { link } => *link,
        }
    }
}

/// A track's Vocal Top Line part as saved.
pub fn to_doc(role: &VocalRole) -> prowave_project::VocalDoc {
    match role {
        VocalRole::Output { link } => prowave_project::VocalDoc {
            role: "output".into(),
            link: *link,
            ..Default::default()
        },
        VocalRole::Score(v) => prowave_project::VocalDoc {
            role: "score".into(),
            link: v.link,
            voice: v.voice.clone(),
            color: v.color.clone(),
            language: v.language.clone(),
            gender: v.gender,
            breathiness: v.breathiness,
            tension: v.tension,
            voicing: v.voicing,
            velocity: v.velocity,
            vibrato_depth: v.vibrato_depth,
            vibrato_rate: v.vibrato_rate,
            transition_ms: v.transition_ms,
            place: v.place.name().into(),
            singer: v.singer.name().into(),
            body_female: v.body_female,
            body_child: v.body_child,
            chimera_breathiness: v.chimera_breathiness,
            brightness: v.brightness,
            ring: v.ring,
            novox_preset: v.novox.preset.clone(),
            novox_formant: v.novox.formant,
            novox_breathiness: v.novox.breathiness,
            novox_grit: v.novox.grit,
            novox_brightness: v.novox.brightness,
            novox_voices: v.novox.voices,
            novox_vibrato_depth: v.novox.vibrato_depth,
            novox_vibrato_rate: v.novox.vibrato_rate,
            novox_scoop: v.novox.scoop,
            novox_portamento: v.novox.portamento,
        },
    }
}

/// A saved Vocal Top Line part (`None` for one this version does not know).
pub fn from_doc(d: &prowave_project::VocalDoc) -> Option<VocalRole> {
    // A file from before CHIMERA has no CHIMERA settings: its defaults.
    let fresh = VocalSettings::new(d.link);
    let (body_female, body_child, chimera_breathiness, brightness, ring) = if d.singer.is_empty() {
        (
            fresh.body_female,
            fresh.body_child,
            fresh.chimera_breathiness,
            fresh.brightness,
            fresh.ring,
        )
    } else {
        (
            d.body_female,
            d.body_child,
            d.chimera_breathiness,
            d.brightness,
            d.ring,
        )
    };
    // A file from before Novox has no Novox settings: its defaults.
    let novox = if d.novox_preset.is_empty() {
        fresh.novox.clone()
    } else {
        crate::novox_singer::Voice {
            preset: d.novox_preset.clone(),
            formant: d.novox_formant,
            breathiness: d.novox_breathiness,
            grit: d.novox_grit,
            brightness: d.novox_brightness,
            voices: d.novox_voices,
            vibrato_depth: d.novox_vibrato_depth,
            vibrato_rate: d.novox_vibrato_rate,
            scoop: d.novox_scoop,
            portamento: d.novox_portamento,
        }
    };
    match d.role.as_str() {
        "output" => Some(VocalRole::Output { link: d.link }),
        "score" => Some(VocalRole::Score(
            VocalSettings {
                link: d.link,
                voice: d.voice.clone(),
                color: d.color.clone(),
                language: d.language.clone(),
                gender: d.gender,
                breathiness: d.breathiness,
                tension: d.tension,
                voicing: d.voicing,
                velocity: d.velocity,
                vibrato_depth: d.vibrato_depth,
                vibrato_rate: d.vibrato_rate,
                transition_ms: d.transition_ms,
                place: Place::from_name(&d.place),
                singer: Singer::from_name(&d.singer),
                body_female,
                body_child,
                chimera_breathiness,
                brightness,
                ring,
                novox,
            }
            .clamped(),
        )),
        _ => None,
    }
}

/// FNV-1a: a hash that stays the same between runs and builds (the cache's file names).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 ^= *x as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }
    /// Seconds, to a tenth of a millisecond.
    fn secs(&mut self, s: f64) {
        self.u64((s * 10_000.0).round() as i64 as u64);
    }
}

/// A note as the singer gets it: seconds from the phrase's start.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreNote {
    pub start: f64,
    pub dur: f64,
    pub pitch: u8,
    pub lyric: String,
}

/// Notes sung in one breath. Sample 0 of its sound is at `origin` seconds of the song.
#[derive(Clone, Debug, PartialEq)]
pub struct Phrase {
    pub origin: f64,
    pub notes: Vec<ScoreNote>,
    /// Names its sound in the cache: the settings and the notes, not where it is in the song.
    pub key: u64,
}

/// The phrases of region `r` of track `t`, timed with `map`.
pub fn region_phrases(
    p: &Project,
    map: &TempoMap,
    t: &Track,
    r: &Region,
    set: &VocalSettings,
) -> Vec<Phrase> {
    let origin = p.region_start_beats(r);
    let len = p.region_len_beats(r) as f32;
    let transpose = r.params.transpose as i32 + t.params.transpose as i32;
    // (start, end, pitch, lyric) in song seconds.
    let mut sung: Vec<(f64, f64, u8, lyrics::Lyric)> = lyrics::sung_order(&r.notes)
        .into_iter()
        .map(|i| r.notes[i])
        .filter(|n| n.start >= 0.0 && n.start < len && n.len > 0.0)
        .map(|n| {
            let s = map.seconds_at(origin + n.start as f64);
            let e = map.seconds_at(origin + (n.start + n.len) as f64);
            let pitch = (n.pitch as i32 + transpose).clamp(0, 127) as u8;
            (s, e, pitch, n.lyric)
        })
        .collect();
    // One voice: a note ends where the next one starts.
    for k in 1..sung.len() {
        if sung[k - 1].1 > sung[k].0 {
            sung[k - 1].1 = sung[k].0;
        }
    }
    sung.retain(|n| n.1 - n.0 >= MIN_NOTE);
    let mut phrases: Vec<Vec<(f64, f64, u8, lyrics::Lyric)>> = Vec::new();
    for n in sung {
        let joins = n.3.is_next_syllable() || n.3.is_hold();
        match phrases.last_mut() {
            Some(ph) if joins || n.0 - ph.last().map_or(f64::MIN, |l| l.1) < GAP => ph.push(n),
            _ => phrases.push(vec![n]),
        }
    }
    phrases
        .into_iter()
        .map(|ph| {
            let origin = ph[0].0 - PREROLL;
            let notes: Vec<ScoreNote> = ph
                .iter()
                .map(|(s, e, pitch, lyric)| ScoreNote {
                    start: s - origin,
                    dur: e - s,
                    pitch: *pitch,
                    lyric: match set.singer {
                        Singer::DiffSinger => lyrics::engine_lyric(lyric),
                        Singer::Chimera | Singer::Chimera09fb776 | Singer::Novox => {
                            crate::chimera_singer::lyric(lyric)
                        }
                    },
                })
                .collect();
            let mut h = Fnv::new();
            h.u64(ENGINE_REV);
            set.sound_key(&mut h);
            for n in &notes {
                h.secs(n.start);
                h.secs(n.dur);
                h.u64(n.pitch as u64);
                h.str(&n.lyric);
            }
            Phrase {
                origin,
                notes,
                key: h.0,
            }
        })
        .collect()
}

/// A MIDI region to sing: where it starts and its phrases.
#[derive(Clone, Debug)]
struct Part {
    start_bar: f32,
    /// Song seconds of its start (sample 0 of its audio region).
    start_secs: f64,
    phrases: Vec<Phrase>,
    key: u64,
}

fn parts(p: &Project, t: &Track, set: &VocalSettings) -> Vec<Part> {
    let map = p.tempo_map();
    t.regions
        .iter()
        .filter(|r| !r.is_audio() && r.folder.is_none() && !r.params.mute)
        .map(|r| {
            let start_secs = map.seconds_at(p.region_start_beats(r));
            let phrases = region_phrases(p, &map, t, r, set);
            let mut h = Fnv::new();
            h.secs(start_secs);
            h.u64(r.start_bar.to_bits() as u64);
            for ph in &phrases {
                h.u64(ph.key);
                h.secs(ph.origin - start_secs);
            }
            Part {
                start_bar: r.start_bar,
                start_secs,
                phrases,
                key: h.0,
            }
        })
        .collect()
}

/// Every phrase of track `t`'s MIDI regions (the Singer instrument plays them where they are).
pub(crate) fn track_phrases(p: &Project, t: &Track, set: &VocalSettings) -> Vec<Phrase> {
    parts(p, t, set)
        .into_iter()
        .flat_map(|part| part.phrases)
        .collect()
}

/// What the singing of track `t` depends on: its parts, and so its notes, lyrics, settings and
/// the tempo where they are.
fn track_key(parts: &[Part]) -> u64 {
    let mut h = Fnv::new();
    for part in parts {
        h.u64(part.key);
    }
    h.0
}

/// The singer's folder: `vocal` in the settings folder (PROWAVE_VOCAL_HOME overrides it, for
/// tests); the phrases sung are kept there.
pub fn vocal_home() -> PathBuf {
    std::env::var_os("PROWAVE_VOCAL_HOME").map_or_else(
        || {
            crate::recent_files::config_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("vocal")
        },
        PathBuf::from,
    )
}

fn local_engine() -> PathBuf {
    vocal_home().join(ENGINE_BIN)
}

/// Whether this Mac has the DiffSinger engine.
pub fn local_installed() -> bool {
    local_engine().is_file()
}

fn cache_dir() -> PathBuf {
    vocal_home().join("cache")
}

pub(crate) fn cached(key: u64) -> PathBuf {
    cache_dir().join(format!("{key:016x}.wav"))
}

/// The score file the singer reads (see tools/vocal/engine/Program.cs).
fn score_json(set: &VocalSettings, phrases: &[(&Phrase, String)]) -> String {
    let list: Vec<serde_json::Value> = phrases
        .iter()
        .map(|(ph, out)| {
            serde_json::json!({
                "id": format!("{:016x}", ph.key),
                "out_file": out,
                "notes": ph.notes.iter().map(|n| serde_json::json!({
                    "start": n.start,
                    "dur": n.dur,
                    "pitch": n.pitch,
                    "lyric": n.lyric,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({
        "singer": set.voice,
        "language": set.language,
        "color": set.color,
        "gender": set.gender,
        "breathiness": set.breathiness,
        "tension": set.tension,
        "voicing": set.voicing,
        "velocity": set.velocity,
        "vibrato_depth": set.vibrato_depth,
        "vibrato_rate": set.vibrato_rate,
        "transition_ms": set.transition_ms,
        "phrases": list,
    })
    .to_string()
}

/// Runs `cmd`, passing its "STEP" lines on as progress; returns its last other line.
fn run(mut cmd: Command, progress: &Arc<Mutex<String>>, prefix: &str) -> Result<String, String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let err_thread = child.stderr.take().map(|e| {
        std::thread::spawn(move || {
            BufReader::new(e)
                .lines()
                .map_while(Result::ok)
                .filter(|l| !l.trim().is_empty())
                .last()
                .unwrap_or_default()
        })
    });
    let mut last = String::new();
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(step) = line.strip_prefix("STEP ") {
                if let Ok(mut p) = progress.lock() {
                    *p = format!("{prefix}{step}");
                }
            } else if !line.trim().is_empty() {
                last = line;
            }
        }
    }
    let err = err_thread.and_then(|t| t.join().ok()).unwrap_or_default();
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(last)
    } else if let Some(msg) = serde_json::from_str::<serde_json::Value>(&last)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
    {
        Err(msg)
    } else {
        Err(if err.is_empty() {
            status.to_string()
        } else {
            err
        })
    }
}

/// Sings `todo` into the cache with DiffSinger.
// One arm is left in the public edition, which sings on this Mac only.
#[allow(clippy::match_single_binding)]
pub(crate) fn sing(
    set: &VocalSettings,
    todo: &[&Phrase],
    place: Place,
    progress: &Arc<Mutex<String>>,
) -> Result<(), String> {
    std::fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let job = format!("job-{stamp:x}-{}", std::process::id());
    match place {
        _ => {
            if !local_installed() {
                return Err(tr("this Mac has no singer: run tools/vocal/setup.sh").into());
            }
            let list: Vec<(&Phrase, String)> = todo
                .iter()
                .map(|ph| (*ph, cached(ph.key).to_string_lossy().into_owned()))
                .collect();
            let score = cache_dir().join(format!("{job}.json"));
            std::fs::write(&score, score_json(set, &list)).map_err(|e| e.to_string())?;
            let mut cmd = Command::new(local_engine());
            cmd.arg(&score).env("HOME", vocal_home());
            let r = run(cmd, progress, &format!("{}: ", tr("This Mac")));
            let _ = std::fs::remove_file(&score);
            r.map(|_| ())
        }
    }?;
    match todo.iter().find(|ph| !cached(ph.key).is_file()) {
        Some(ph) => Err(format!("phrase {:016x} was not sung", ph.key)),
        None => Ok(()),
    }
}

/// How the last render of an in-process singer went: phrases sung, in how long, and the seconds
/// of singing they hold.
pub(crate) fn sung_message(singer: Singer, phrases: usize, secs: f64, sung_secs: f64) -> String {
    let args: [&dyn std::fmt::Display; 3] =
        [&phrases, &format!("{secs:.1}"), &format!("{sung_secs:.1}")];
    match singer {
        Singer::Novox => trf("{} phrases sung by Novox in {} s ({} s of singing)", &args),
        _ => trf(
            "{} phrases sung by CHIMERA in {} s ({} s of singing)",
            &args,
        ),
    }
}

/// The voice the status LCD names under the singer: DiffSinger's voice, Novox's preset, none
/// for CHIMERA.
pub(crate) fn voice_name(set: &VocalSettings, voice: Option<&Voice>) -> String {
    match set.singer {
        Singer::DiffSinger => voice.map_or(set.voice.clone(), |v| v.name.clone()),
        Singer::Novox => set.novox.preset.clone(),
        Singer::Chimera | Singer::Chimera09fb776 => String::new(),
    }
}

/// The credits printed at the foot of the window for the singer (the voice's licence).
pub(crate) fn credits(set: &VocalSettings, voice: Option<&Voice>) -> Option<String> {
    match set.singer {
        Singer::Chimera | Singer::Chimera09fb776 => {
            Some(tr("CHIMERA voice model by Jameson Hodge (MIT licence)").into())
        }
        Singer::Novox => {
            Some(tr("Novox The Robot voice model by Jonathan Mann (MIT licence)").into())
        }
        Singer::DiffSinger => voice.map(|v| v.licence.clone()).filter(|l| !l.is_empty()),
    }
}

/// One phrase sung by an in-process singer, at [`SAMPLE_RATE`].
pub(crate) fn sing_phrase(set: &VocalSettings, ph: &Phrase) -> Vec<f32> {
    if set.singer == Singer::Novox {
        return crate::novox_singer::sing(&ph.notes, &set.novox, SAMPLE_RATE);
    }
    let x = crate::chimera_singer::sing(&ph.notes, &set.chimera_voice());
    crate::chimera_singer::resample(&x, crate::chimera_singer::RATE, SAMPLE_RATE)
}

/// Sings `todo` into the cache with an in-process singer (CHIMERA or Novox), on this thread
/// and helpers (one phrase each at a time, leaving two cores to the rest of the program).
pub(crate) fn sing_in_process(
    set: &VocalSettings,
    todo: &[&Phrase],
    progress: &Arc<Mutex<String>>,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    std::fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get().saturating_sub(2).max(1))
        .min(todo.len().max(1));
    let (next, sung) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let errors = Mutex::new(Vec::new());
    let total = todo.len();
    let work = || loop {
        let i = next.fetch_add(1, Ordering::Relaxed);
        let Some(ph) = todo.get(i) else { break };
        let x = sing_phrase(set, ph);
        if x.is_empty() && !ph.notes.is_empty() {
            if let Ok(mut errs) = errors.lock() {
                errs.push(format!(
                    "{}: phrase {:016x} was not sung",
                    set.singer.label(),
                    ph.key
                ));
            }
            continue;
        }
        if let Err(e) = write_wav(&cached(ph.key), &x, SAMPLE_RATE) {
            if let Ok(mut errs) = errors.lock() {
                errs.push(e.to_string());
            }
        }
        let n = sung.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut p) = progress.lock() {
            *p = match set.singer {
                Singer::Novox => trf("Novox: {} of {} phrases", &[&n, &total]),
                _ => trf("CHIMERA: {} of {} phrases", &[&n, &total]),
            };
        }
    };
    std::thread::scope(|sc| {
        for _ in 1..workers {
            sc.spawn(work);
        }
        work();
    });
    if let Some(e) = errors.into_inner().ok().and_then(|e| e.into_iter().next()) {
        return Err(e);
    }
    match todo.iter().find(|ph| !cached(ph.key).is_file()) {
        Some(ph) => Err(format!("phrase {:016x} was not sung", ph.key)),
        None => Ok(()),
    }
}

/// Reads a mono or multi-channel WAV (16-bit, 24-bit or 32-bit float) as mono samples.
pub fn read_wav(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(format!("{}: not a WAV file", path.display()));
    }
    let (mut fmt, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let len = u32::from_le_bytes([b[at + 4], b[at + 5], b[at + 6], b[at + 7]]) as usize;
        let body = at + 8..(at + 8 + len).min(b.len());
        if id == b"fmt " {
            fmt = Some(body.clone());
        } else if id == b"data" {
            data = Some(body.clone());
        }
        at = body.end + (len & 1);
    }
    let (Some(f), Some(d)) = (fmt, data) else {
        return Err(format!("{}: no audio in the file", path.display()));
    };
    let f = &b[f];
    if f.len() < 16 {
        return Err(format!("{}: bad WAV header", path.display()));
    }
    let tag = u16::from_le_bytes([f[0], f[1]]);
    let channels = u16::from_le_bytes([f[2], f[3]]).max(1) as usize;
    let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
    let bits = u16::from_le_bytes([f[14], f[15]]);
    let d = &b[d];
    let float = tag == 3 || (tag == 0xFFFE && bits == 32);
    let sample = |x: &[u8]| -> f32 {
        match bits {
            32 if float => f32::from_le_bytes([x[0], x[1], x[2], x[3]]),
            32 => i32::from_le_bytes([x[0], x[1], x[2], x[3]]) as f32 / 2_147_483_648.0,
            24 => (i32::from_le_bytes([0, x[0], x[1], x[2]]) >> 8) as f32 / 8_388_608.0,
            16 => i16::from_le_bytes([x[0], x[1]]) as f32 / 32768.0,
            _ => 0.0,
        }
    };
    let width = (bits as usize / 8).max(1);
    let frame = width * channels;
    let out = d
        .chunks_exact(frame)
        .map(|fr| fr.chunks_exact(width).map(sample).sum::<f32>() / channels as f32)
        .collect();
    Ok((out, rate))
}

/// Writes mono 32-bit float samples as a WAV file.
pub fn write_wav(path: &Path, samples: &[f32], rate: u32) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let data = (samples.len() * 4) as u32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    let tmp = path.with_extension("wav.part");
    std::fs::write(&tmp, b)?;
    std::fs::rename(tmp, path)
}

/// The sample of a phrase starting at `origin` seconds, in a sound starting at `start`.
pub fn sample_offset(origin: f64, start: f64) -> i64 {
    ((origin - start) * SAMPLE_RATE as f64).round() as i64
}

/// Mixes sounds that start at the given song seconds into one that starts at `start`; what
/// is before `start` is left out.
pub fn mix_at(start: f64, sounds: &[(f64, Vec<f32>)]) -> Vec<f32> {
    let end = sounds
        .iter()
        .map(|(o, s)| sample_offset(*o, start) + s.len() as i64)
        .max()
        .unwrap_or(0)
        .max(0) as usize;
    let mut out = vec![0f32; end];
    for (o, s) in sounds {
        let at = sample_offset(*o, start);
        for (k, x) in s.iter().enumerate() {
            let i = at + k as i64;
            if i >= 0 && (i as usize) < end {
                out[i as usize] += x;
            }
        }
    }
    out
}

/// Where the singing of a part goes (the session's media, saved into the project).
fn part_file(link: u64, part: &Part) -> PathBuf {
    crate::project_io::recordings_dir()
        .join("vocal")
        .join(format!("{link:016x}-{:016x}.wav", part.key))
}

/// A sung part, ready to become an audio region.
#[derive(Debug)]
struct Sung {
    start_bar: f32,
    start_secs: f64,
    file: PathBuf,
    secs: f64,
}

struct Done {
    link: u64,
    singer: Singer,
    parts: Vec<Sung>,
    /// Where it was sung, how long that took and how many seconds of phrases it made.
    place: Option<Place>,
    secs: f64,
    sung_secs: f64,
    phrases: usize,
}

fn render(
    set: VocalSettings,
    parts: Vec<Part>,
    order: Vec<Place>,
    progress: Arc<Mutex<String>>,
) -> Result<Done, String> {
    let started = Instant::now();
    let mut todo: Vec<&Phrase> = Vec::new();
    for ph in parts.iter().flat_map(|p| &p.phrases) {
        if !cached(ph.key).is_file() && !todo.iter().any(|t| t.key == ph.key) {
            todo.push(ph);
        }
    }
    let sung_secs: f64 = todo
        .iter()
        .map(|ph| ph.notes.last().map_or(0.0, |n| n.start + n.dur))
        .sum();
    let mut place = None;
    if !todo.is_empty() && set.singer.in_process() {
        sing_in_process(&set, &todo, &progress)?;
        place = Some(Place::ThisMac);
    } else if !todo.is_empty() {
        let mut errors = Vec::new();
        for at in order {
            match sing(&set, &todo, at, &progress) {
                Ok(()) => {
                    place = Some(at);
                    break;
                }
                Err(e) => errors.push(format!("{}: {e}", at.label())),
            }
        }
        if place.is_none() {
            return Err(errors.join("; "));
        }
    }
    let secs = started.elapsed().as_secs_f64();
    let mut out = Vec::new();
    for part in &parts {
        if part.phrases.is_empty() {
            continue;
        }
        let file = part_file(set.link, part);
        if !file.is_file() {
            let mut sounds = Vec::new();
            for ph in &part.phrases {
                let (s, _) = read_wav(&cached(ph.key))?;
                sounds.push((ph.origin, s));
            }
            let mixed = mix_at(part.start_secs, &sounds);
            write_wav(&file, &mixed, SAMPLE_RATE).map_err(|e| e.to_string())?;
        }
        let secs = prowave_engine::AudioFile::open(&file)
            .map_err(|e| format!("{}: {e}", file.display()))?
            .duration_secs();
        out.push(Sung {
            start_bar: part.start_bar,
            start_secs: part.start_secs,
            file,
            secs,
        });
    }
    trim_cache();
    Ok(Done {
        link: set.link,
        singer: set.singer,
        parts: out,
        place,
        secs,
        sung_secs,
        phrases: todo.len(),
    })
}

/// Deletes the oldest cached phrases past [`CACHE_BYTES`].
pub(crate) fn trim_cache() {
    let Ok(dir) = std::fs::read_dir(cache_dir()) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = dir
        .filter_map(Result::ok)
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= CACHE_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

struct Job {
    progress: Arc<Mutex<String>>,
    result: Receiver<Result<Done, String>>,
    key: u64,
}

/// One Vocal Top Line track's rendering.
#[derive(Default)]
struct TrackState {
    /// The key last sung (or that failed: it is not tried again until something changes).
    done: Option<u64>,
    /// The key seen and since when (it is sung once it has stayed for [`SETTLE`]).
    seen: Option<(u64, Instant)>,
    job: Option<Job>,
    /// How the last render went, for the window.
    last: String,
}

/// The Vocal Top Line window and the renders running.
#[derive(Default)]
pub struct VocalState {
    pub open: bool,
    tracks: HashMap<u64, TrackState>,
    /// Seconds taken per second of singing made, on this Mac and on another (measured).
    speed: [Option<f64>; 2],
    checked: Option<Instant>,
    /// Render now, whatever the pause.
    force: bool,
    /// The installed voices, read again every few seconds while the window is open.
    voices: Option<(Instant, Vec<Voice>)>,
}

impl VocalState {
    /// What the renders are doing, for the status line.
    pub fn progress(&self) -> Option<String> {
        self.tracks.values().find_map(|t| {
            t.job.as_ref().map(|j| {
                let step = j.progress.lock().map(|s| s.clone()).unwrap_or_default();
                trf("Singing: {}", &[&step])
            })
        })
    }

    /// The order to try the places in for `place`.
    fn order(&self, _: Place) -> Vec<Place> {
        vec![Place::ThisMac]
    }
}

/// A new link: random enough that two tracks never share one.
fn new_link() -> u64 {
    let mut h = Fnv::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.bytes(&now.to_le_bytes());
    h.u64(std::process::id() as u64);
    h.0.max(1)
}

/// Track > New Vocal Top Line Track: the score track (with an empty 4-bar region at the
/// playhead) and the audio track for its singing, after the other tracks.
pub fn new_track(p: &mut Project, s: &mut crate::UiState) {
    let link = new_link();
    let n = p
        .tracks
        .iter()
        .filter(|t| matches!(t.vocal, Some(VocalRole::Score(_))))
        .count()
        + 1;
    let name = trf("Top Line {}", &[&n]);
    let mut score = Track::new(name.clone(), TrackKind::Instrument);
    score.color = egui::Color32::from_rgb(0xd8, 0x5a, 0x9a);
    let mut set = VocalSettings::new(link);
    // With no DiffSinger voice installed, CHIMERA,
    // which needs nothing installed.
    if voices().is_empty() {
        set.singer = Singer::Chimera;
    }
    score.vocal = Some(VocalRole::Score(set));
    let bar = (p.beats_to_bar(p.position_beats).floor() + 1.0).max(1.0) as f32;
    score.regions.push(Region {
        missing: None,
        name: name.clone(),
        start_bar: bar,
        len_bars: 4.0,
        notes: Vec::new(),
        controls: Vec::new(),
        folder: None,
        audio: None,
        params: RegionParams::default(),
        generated: None,
    });
    let mut out = Track::new(trf("{} Vocal", &[&name]), TrackKind::Audio);
    out.color = egui::Color32::from_rgb(0xb0, 0x48, 0x80);
    out.vocal = Some(VocalRole::Output { link });
    p.tracks.push(score);
    p.tracks.push(out);
    p.selected_track = p.tracks.len() - 2;
    s.tracks.selected = Some((p.selected_track, 0));
    p.song_rev += 1;
    s.vocal.open = true;
}

fn output_track(p: &Project, link: u64) -> Option<usize> {
    p.tracks
        .iter()
        .position(|t| t.vocal == Some(VocalRole::Output { link }))
}

/// Puts the sung parts on the link's audio track (made again if it was deleted).
fn place_parts(ctx: &egui::Context, p: &mut Project, done: &Done) {
    let Some(src) = p
        .tracks
        .iter()
        .position(|t| matches!(&t.vocal, Some(VocalRole::Score(v)) if v.link == done.link))
    else {
        return;
    };
    let ti = match output_track(p, done.link) {
        Some(i) => i,
        None => {
            let mut out = Track::new(trf("{} Vocal", &[&p.tracks[src].name]), TrackKind::Audio);
            out.vocal = Some(VocalRole::Output { link: done.link });
            p.tracks.push(out);
            p.tracks.len() - 1
        }
    };
    let map = p.tempo_map();
    // Pitch Editor edits of phrases sung again unchanged (the same cached file) stay.
    let kept: HashMap<PathBuf, crate::flex_edit::FlexEdit> = p.tracks[ti]
        .regions
        .iter()
        .filter_map(|r| r.audio.as_ref()?.flex.clone())
        .map(|f| (f.original.path.clone(), f))
        .collect();
    let mut regions = Vec::new();
    for part in &done.parts {
        let Ok(file) = prowave_engine::AudioFile::open(&part.file) else {
            continue;
        };
        let mut audio = crate::import::audio_region(file, ctx);
        if let Some(mut f) = kept.get(&audio.file.path).cloned() {
            f.original = audio.file.clone();
            f.rendered = 0;
            audio.flex = Some(f);
        }
        // The region covers the sound: its end in beats, then in bars.
        let end_beats = map.beat_at(part.start_secs + part.secs);
        let end_bar = p.beats_to_bar(end_beats) + 1.0;
        let len_bars = ((end_bar - part.start_bar as f64) as f32).max(0.05);
        regions.push(Region {
            missing: None,
            name: p.tracks[src].name.clone(),
            start_bar: part.start_bar,
            len_bars,
            notes: Vec::new(),
            controls: Vec::new(),
            folder: None,
            audio: Some(audio),
            params: RegionParams::default(),
            generated: None,
        });
    }
    p.tracks[ti].regions = regions;
    p.song_rev += 1;
}

/// Once per frame: notices changes, starts renders after a pause, and places what was sung.
pub fn frame(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    let st = &mut s.vocal;
    // Finished renders.
    let mut finished = Vec::new();
    for (link, t) in st.tracks.iter_mut() {
        let Some(job) = &t.job else { continue };
        match job.result.try_recv() {
            Ok(r) => finished.push((*link, job.key, r)),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                finished.push((*link, job.key, Err("the render stopped".into())))
            }
        }
    }
    for (link, key, r) in finished {
        let t = st.tracks.entry(link).or_default();
        t.job = None;
        t.done = Some(key);
        match r {
            Ok(done) => {
                // The measured speeds choose where DiffSinger sings; CHIMERA's do not count.
                let measured = done.singer == Singer::DiffSinger && done.sung_secs > 0.5;
                if let (Some(place), true) = (done.place, measured) {
                    let i = usize::from(place != Place::ThisMac);
                    let rate = done.secs / done.sung_secs;
                    st.speed[i] = Some(st.speed[i].map_or(rate, |old| 0.5 * old + 0.5 * rate));
                }
                t.last = match done.place {
                    Some(_) if done.singer.in_process() => {
                        sung_message(done.singer, done.phrases, done.secs, done.sung_secs)
                    }
                    Some(place) => trf(
                        "{} phrases sung on {} in {} s",
                        &[&done.phrases, &place.label(), &format!("{:.1}", done.secs)],
                    ),
                    None => tr("Up to date (from the cache)").to_string(),
                };
                place_parts(ctx, p, &done);
            }
            Err(e) => {
                t.last = trf("Singing failed: {}", &[&e]);
                s.status = Some(t.last.clone());
            }
        }
    }
    // Changes, at most every 200 ms.
    let now = Instant::now();
    let due = st
        .checked
        .is_none_or(|c| now.duration_since(c) >= Duration::from_millis(200));
    if due || st.force {
        st.checked = Some(now);
        let force = std::mem::take(&mut st.force);
        for ti in 0..p.tracks.len() {
            let Some(VocalRole::Score(set)) = &p.tracks[ti].vocal else {
                continue;
            };
            let set = set.clone();
            let parts = parts(p, &p.tracks[ti], &set);
            let key = track_key(&parts);
            let t = st.tracks.entry(set.link).or_default();
            if t.job.is_some() || (t.done == Some(key) && !force) {
                continue;
            }
            let settled = match t.seen {
                Some((k, since)) if k == key => now.duration_since(since) >= SETTLE,
                _ => {
                    t.seen = Some((key, now));
                    false
                }
            };
            if !settled && !force {
                continue;
            }
            let order = st.order(set.place);
            let t = st.tracks.entry(set.link).or_default();
            let progress = Arc::new(Mutex::new(tr("starting").to_string()));
            let (tx, rx) = channel();
            let job_progress = progress.clone();
            let spawned = std::thread::Builder::new()
                .name("prowave-vocal".into())
                .spawn(move || {
                    let _ = tx.send(render(set, parts, order, job_progress));
                });
            match spawned {
                Ok(_) => {
                    t.job = Some(Job {
                        progress,
                        result: rx,
                        key,
                    })
                }
                Err(e) => t.last = e.to_string(),
            }
        }
    }
    if let Some(progress) = st.progress() {
        s.status = Some(progress);
        ctx.request_repaint_after(Duration::from_millis(250));
    } else if st
        .tracks
        .values()
        .any(|t| t.seen.is_some_and(|(k, _)| Some(k) != t.done))
    {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    show_window(ctx, p, s);
}

/// A DiffSinger voice installed for singing, from `voices.json`.
#[derive(Clone, Debug, Default)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub colors: Vec<String>,
    /// The settings the voice responds to (gender, breathiness, tension, voicing, velocity).
    pub controls: Vec<String>,
    pub licence: String,
}

/// The DiffSinger voices installed.
pub fn voices() -> Vec<Voice> {
    let Ok(text) = std::fs::read_to_string(vocal_home().join("voices.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let strings = |x: &serde_json::Value| -> Vec<String> {
        x.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    v.as_array()
        .map(|list| {
            list.iter()
                .map(|x| Voice {
                    id: x["id"].as_str().unwrap_or_default().to_string(),
                    name: x["name"].as_str().unwrap_or_default().to_string(),
                    colors: strings(&x["colors"]),
                    controls: strings(&x["controls"]),
                    licence: x["licence"].as_str().unwrap_or_default().to_string(),
                })
                .filter(|v| !v.id.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Bounce: the whole isolated vocal of link `link`, from the start of the song, into `path`.
pub fn bounce(p: &Project, link: u64, path: &Path) -> Result<f64, String> {
    let ti = output_track(p, link).ok_or_else(|| tr("nothing has been sung yet").to_string())?;
    let map = p.tempo_map();
    let mut sounds = Vec::new();
    for r in p.tracks[ti].regions.iter().filter(|r| !r.params.mute) {
        let Some(a) = &r.audio else { continue };
        let (mut s, rate) = read_wav(&a.file.path)?;
        if rate != SAMPLE_RATE {
            return Err(format!("{}: {rate} Hz", a.file.path.display()));
        }
        let skip = (r.params.offset_secs * SAMPLE_RATE as f64).round().max(0.0) as usize;
        s.drain(..skip.min(s.len()));
        let start = map.seconds_at(p.region_start_beats(r));
        let len = map.seconds_at(p.region_end_beats(r)) - start;
        s.truncate((len.max(0.0) * SAMPLE_RATE as f64).round() as usize);
        sounds.push((start, s));
    }
    if sounds.is_empty() {
        return Err(tr("nothing has been sung yet").into());
    }
    let mixed = mix_at(0.0, &sounds);
    write_wav(path, &mixed, SAMPLE_RATE).map_err(|e| e.to_string())?;
    Ok(mixed.len() as f64 / SAMPLE_RATE as f64)
}

/// The settings' knobs at their defaults (for a double-click).
fn defaults() -> VocalSettings {
    VocalSettings::new(0)
}

/// CHIMERA's settings: the body (and the shares it makes), the voice's breathiness, brightness
/// and ring, and the vibrato.
fn chimera_panel(ui: &mut egui::Ui, set: &mut VocalSettings, changed: &mut bool) {
    let d = defaults();
    let id = egui::Id::unique("vocal_chimera_dials");
    hw_vocal::section(ui, tr("Sound"));
    *changed |= hw_vocal::dials(
        ui,
        id,
        vec![
            dial(
                tr("Body: Female"),
                &mut set.body_female,
                0.0..=100.0,
                d.body_female,
                0,
            ),
            dial(
                tr("Body: Child"),
                &mut set.body_child,
                0.0..=100.0,
                d.body_child,
                0,
            ),
            dial(
                tr("Breathiness"),
                &mut set.chimera_breathiness,
                0.0..=100.0,
                d.chimera_breathiness,
                0,
            ),
            dial(
                tr("Brightness"),
                &mut set.brightness,
                0.0..=100.0,
                d.brightness,
                0,
            ),
            dial(tr("Singer's Ring"), &mut set.ring, 0.0..=100.0, d.ring, 0),
        ],
    );
    hw_vocal::gap(ui, 6.0);
    hw_vocal::section(ui, tr("Pitch"));
    *changed |= hw_vocal::dials(
        ui,
        id.with("pitch"),
        vec![
            dial(
                tr("Vibrato Depth (cents)"),
                &mut set.vibrato_depth,
                0.0..=200.0,
                d.vibrato_depth,
                0,
            ),
            dial(
                tr("Vibrato Rate (Hz)"),
                &mut set.vibrato_rate,
                3.0..=9.0,
                d.vibrato_rate,
                1,
            ),
        ],
    );
}

/// A knob with its own unit.
fn dial_in<'a>(
    label: &'a str,
    value: &'a mut f32,
    (lo, hi): (f32, f32),
    default: f32,
    decimals: usize,
    unit: &'a str,
) -> hw_vocal::Dial<'a> {
    hw_vocal::Dial {
        unit,
        ..dial(label, value, lo..=hi, default, decimals)
    }
}

/// Novox The Robot's settings: its voice (one of the engine's presets, which sets every knob),
/// and the knobs over it: the sound (vocal tract size, breathiness, grit, brightness, voices)
/// and the pitch (vibrato, the scoop into each note, the glide between joined notes). A knob
/// goes back to the voice's own value on a double-click.
fn novox_panel(ui: &mut egui::Ui, set: &mut VocalSettings, changed: &mut bool) {
    use crate::novox_singer as nv;
    let id = egui::Id::unique("vocal_novox");
    let field = hw_vocal::row(ui, tr("Voice"));
    let names = nv::presets();
    let now = names.iter().position(|n| *n == set.novox.preset);
    if let Some(i) = hw_vocal::dropdown(ui, id.with("preset"), field, &set.novox.preset, names, now)
    {
        if let Some(v) = nv::Voice::preset(names[i]) {
            *changed |= set.novox != v;
            set.novox = v;
        }
    }
    let d = nv::Voice::preset(&set.novox.preset).unwrap_or_default();
    let v = &mut set.novox;
    hw_vocal::gap(ui, 8.0);
    hw_vocal::section(ui, tr("Sound"));
    *changed |= hw_vocal::dials(
        ui,
        id.with("sound"),
        vec![
            dial_in(
                tr("Formant"),
                &mut v.formant,
                nv::FORMANT,
                d.formant,
                0,
                "%",
            ),
            dial_in(
                tr("Breathiness"),
                &mut v.breathiness,
                nv::BREATHINESS,
                d.breathiness,
                0,
                "%",
            ),
            dial_in(tr("Grit"), &mut v.grit, nv::GRIT, d.grit, 0, "%"),
            dial_in(
                tr("Brightness"),
                &mut v.brightness,
                nv::BRIGHTNESS,
                d.brightness,
                0,
                "%",
            ),
            dial_in(tr("Voices"), &mut v.voices, nv::VOICES, d.voices, 0, ""),
        ],
    );
    v.voices = v.voices.round();
    hw_vocal::gap(ui, 6.0);
    hw_vocal::section(ui, tr("Pitch"));
    *changed |= hw_vocal::dials(
        ui,
        id.with("pitch"),
        vec![
            dial_in(
                tr("Vibrato Depth (cents)"),
                &mut v.vibrato_depth,
                nv::VIBRATO_DEPTH,
                d.vibrato_depth,
                0,
                "",
            ),
            dial_in(
                tr("Vibrato Rate (Hz)"),
                &mut v.vibrato_rate,
                nv::VIBRATO_RATE,
                d.vibrato_rate,
                1,
                "",
            ),
            dial_in(tr("Scoop (cents)"), &mut v.scoop, nv::SCOOP, d.scoop, 0, ""),
            dial_in(
                tr("Portamento"),
                &mut v.portamento,
                nv::PORTAMENTO,
                d.portamento,
                0,
                "ms",
            ),
        ],
    );
}

/// `x`, off unless `on`.
fn on(mut x: hw_vocal::Dial<'_>, on: bool) -> hw_vocal::Dial<'_> {
    x.enabled = on;
    x
}

/// A knob for one setting.
fn dial<'a>(
    label: &'a str,
    value: &'a mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    decimals: usize,
) -> hw_vocal::Dial<'a> {
    hw_vocal::Dial {
        label,
        value,
        range,
        default,
        decimals,
        unit: "%",
        enabled: true,
    }
}

/// The singer and its settings, as a Hardware panel: the singer, voice, colour, language and
/// where it is sung as dark fields; the sound and the pitch as knobs (a knob for a control the
/// DiffSinger `voice` does not have is off). For CHIMERA and Novox, their own knobs.
pub(crate) fn settings_panel(
    ui: &mut egui::Ui,
    set: &mut VocalSettings,
    voices: &[Voice],
    changed: &mut bool,
) {
    let voice = voices.iter().find(|v| v.id == set.voice).cloned();
    let has = |c: &str| {
        voice
            .as_ref()
            .is_none_or(|v| v.controls.iter().any(|x| x == c))
    };
    let id = egui::Id::unique("vocal_settings");
    hw_vocal::section(ui, tr("Voice"));
    let field = hw_vocal::row(ui, tr("Singer"));
    let singers: Vec<&str> = Singer::ALL.iter().map(|s| s.label()).collect();
    let now = Singer::ALL.iter().position(|s| *s == set.singer);
    if let Some(i) = hw_vocal::dropdown(
        ui,
        id.with("singer"),
        field,
        set.singer.label(),
        &singers,
        now,
    ) {
        *changed |= set.singer != Singer::ALL[i];
        set.singer = Singer::ALL[i];
    }
    if set.singer.is_chimera() {
        hw_vocal::gap(ui, 8.0);
        chimera_panel(ui, set, changed);
        return;
    }
    if set.singer == Singer::Novox {
        novox_panel(ui, set, changed);
        return;
    }
    let field = hw_vocal::row(ui, tr("Voice"));
    let names: Vec<&str> = voices.iter().map(|v| v.name.as_str()).collect();
    let shown = voice.as_ref().map_or(set.voice.clone(), |v| v.name.clone());
    let now = voices.iter().position(|v| v.id == set.voice);
    if let Some(i) = hw_vocal::dropdown(ui, id.with("voice"), field, &shown, &names, now) {
        *changed |= set.voice != voices[i].id;
        set.voice = voices[i].id.clone();
    }
    if let Some(v) = voice.as_ref().filter(|v| !v.colors.is_empty()) {
        let field = hw_vocal::row(ui, tr("Colour"));
        let shown = if set.color.is_empty() {
            v.colors[0].clone()
        } else {
            set.color.clone()
        };
        let names: Vec<&str> = v.colors.iter().map(String::as_str).collect();
        let now = v.colors.iter().position(|c| *c == set.color);
        if let Some(i) = hw_vocal::dropdown(ui, id.with("colour"), field, &shown, &names, now) {
            *changed |= set.color != v.colors[i];
            set.color = v.colors[i].clone();
        }
    }
    let field = hw_vocal::row(ui, tr("Language"));
    let lang = LANGUAGES
        .iter()
        .find(|(c, _)| *c == set.language)
        .map_or("English", |(_, n)| n);
    let names: Vec<&str> = LANGUAGES.iter().map(|(_, n)| *n).collect();
    let now = LANGUAGES.iter().position(|(c, _)| *c == set.language);
    if let Some(i) = hw_vocal::dropdown(ui, id.with("language"), field, lang, &names, now) {
        *changed |= set.language != LANGUAGES[i].0;
        set.language = LANGUAGES[i].0.to_string();
    }
    let field = hw_vocal::row(ui, tr("Render on"));
    let places: Vec<&str> = Place::ALL.iter().map(|p| p.label()).collect();
    let now = Place::ALL.iter().position(|p| *p == set.place);
    if let Some(i) =
        hw_vocal::dropdown(ui, id.with("place"), field, set.place.label(), &places, now)
    {
        set.place = Place::ALL[i];
    }
    let d = defaults();
    hw_vocal::gap(ui, 8.0);
    hw_vocal::section(ui, tr("Sound"));
    *changed |= hw_vocal::dials(
        ui,
        id.with("sound"),
        vec![
            on(
                dial(tr("Gender"), &mut set.gender, -100.0..=100.0, d.gender, 0),
                has("gender"),
            ),
            on(
                dial(
                    tr("Breathiness"),
                    &mut set.breathiness,
                    -100.0..=100.0,
                    d.breathiness,
                    0,
                ),
                has("breathiness"),
            ),
            on(
                dial(
                    tr("Tension"),
                    &mut set.tension,
                    -100.0..=100.0,
                    d.tension,
                    0,
                ),
                has("tension"),
            ),
            on(
                dial(tr("Voicing"), &mut set.voicing, 0.0..=100.0, d.voicing, 0),
                has("voicing"),
            ),
            on(
                dial(
                    tr("Consonant Speed"),
                    &mut set.velocity,
                    0.0..=200.0,
                    d.velocity,
                    0,
                ),
                has("velocity"),
            ),
        ],
    );
    hw_vocal::gap(ui, 6.0);
    hw_vocal::section(ui, tr("Pitch"));
    *changed |= hw_vocal::dials(
        ui,
        id.with("pitch"),
        vec![
            dial(
                tr("Vibrato Depth (cents)"),
                &mut set.vibrato_depth,
                0.0..=200.0,
                d.vibrato_depth,
                0,
            ),
            dial(
                tr("Vibrato Rate (Hz)"),
                &mut set.vibrato_rate,
                3.0..=9.0,
                d.vibrato_rate,
                1,
            ),
            dial(
                tr("Pitch Transition (ms)"),
                &mut set.transition_ms,
                5.0..=300.0,
                d.transition_ms,
                0,
            ),
        ],
    );
}

/// What the status LCD of the Vocal Top Line and Singer windows shows.
pub(crate) struct Status<'a> {
    pub singer: Singer,
    /// The DiffSinger voice's name (empty for CHIMERA).
    pub voice: &'a str,
    /// What is going on (the work's progress, or how the last render went).
    pub line: String,
    /// A second line in smaller type.
    pub note: String,
    /// Work going on: the chaser runs.
    pub busy: bool,
    /// Phrases sung so far, of how many.
    pub ready: Option<(usize, usize)>,
    /// The measured speeds of the places DiffSinger sings on, with what they mean.
    pub speeds: Option<([Option<f64>; 2], String)>,
    /// CHIMERA's body: its male, female and child shares (in %).
    pub shares: Option<[f32; 3]>,
}

/// The status LCD across the panel, in three zones at fixed places for every singer: the
/// singer and voice; the status (what is going on, a chaser while it works, and a ladder of
/// the phrases ready); and the render: the measured speeds (DiffSinger) or the body's shares
/// (CHIMERA).
pub(crate) fn status_lcd(ui: &mut egui::Ui, st: &Status<'_>) {
    use egui::{pos2, vec2, Align2, Rect};
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 92.0), egui::Sense::hover());
    let g = ui.painter().clone();
    let glass = hw::lcd(&g, rect);
    let (top, bottom) = (glass.top() + 8.0, glass.bottom() - 8.0);
    let label_ink = theme::LCD_INK().gamma_multiply(0.72);
    // Zone edges.
    let z1 = Rect::from_min_max(
        pos2(glass.left() + 12.0, top),
        pos2(glass.left() + 142.0, bottom),
    );
    let z3 = Rect::from_min_max(
        pos2(glass.right() - 112.0, top),
        pos2(glass.right() - 12.0, bottom),
    );
    let z2 = Rect::from_min_max(pos2(z1.right() + 16.0, top), pos2(z3.left() - 16.0, bottom));
    hw_vocal::lcd_divider(&g, z1.right() + 8.0, top..=bottom);
    hw_vocal::lcd_divider(&g, z3.left() - 8.0, top..=bottom);

    // The singer (smaller for a long name, then cut), and its voice under it (smaller, then on
    // two lines, before it is cut).
    hw_vocal::lcd_label(&g, z1.min, tr("Singer"));
    let label = st.singer.label().to_uppercase();
    let size = [17.0, 15.0, 13.0]
        .into_iter()
        .find(|size| {
            hw::galley(&g, &label, Face::Lcd, *size, theme::LCD_INK())
                .size()
                .x
                <= z1.width()
        })
        .unwrap_or(13.0);
    let name = hw::fit(&g, &label, Face::Lcd, size, z1.width());
    hw::text(
        &g,
        pos2(z1.left(), z1.top() + 28.0),
        Align2::LEFT_CENTER,
        &name,
        Face::Lcd,
        size,
        theme::LCD_INK(),
    );
    if !st.voice.is_empty() {
        let voice = hw_vocal::printable(&g, &st.voice.to_uppercase(), Face::Lcd, 9.0);
        let one_line = [9.0, 8.0].into_iter().find(|size| {
            hw::galley(&g, &voice, Face::Lcd, *size, label_ink).size().x <= z1.width()
        });
        match one_line {
            Some(size) => {
                hw::text(
                    &g,
                    pos2(z1.left(), z1.bottom()),
                    Align2::LEFT_BOTTOM,
                    &voice,
                    Face::Lcd,
                    size,
                    label_ink,
                );
            }
            None => hw_vocal::lcd_text(
                &g,
                Rect::from_min_max(pos2(z1.left(), z1.bottom() - 22.0), z1.max),
                &voice,
                8.0,
                2,
                label_ink,
            ),
        }
    }

    // The status: a chaser by its label while working; the line and note; the phrases ready.
    let head = hw_vocal::lcd_label(&g, z2.min, tr("Status"));
    if st.busy {
        hw_vocal::lcd_chaser(
            &g,
            Rect::from_min_max(
                pos2(head.right() + 8.0, head.top() + 1.0),
                pos2(z2.right(), head.bottom() - 1.0),
            ),
            ui.input(|i| i.time),
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(80));
    }
    let text_bottom = if st.ready.is_some() {
        z2.bottom() - 16.0
    } else {
        z2.bottom()
    };
    let body = Rect::from_min_max(
        pos2(z2.left(), head.bottom() + 3.0),
        pos2(z2.right(), text_bottom),
    );
    match (st.line.is_empty(), st.note.is_empty()) {
        (false, true) => hw_vocal::lcd_text(&g, body, &st.line, 9.5, 3, theme::LCD_INK()),
        (true, false) => hw_vocal::lcd_text(&g, body, &st.note, 9.0, 3, label_ink),
        (false, false) => {
            let (a, b) = body.split_top_bottom_at_fraction(0.5);
            hw_vocal::lcd_text(&g, a, &st.line, 9.5, 2, theme::LCD_INK());
            hw_vocal::lcd_text(&g, b, &st.note, 8.5, 2, label_ink);
        }
        (true, true) => {}
    }
    if let Some((ready, total)) = st.ready {
        let count = format!("{ready}/{total}");
        let h = 11.0;
        let w = hw::seg_width(&count, h);
        let y = z2.bottom() - h;
        hw::seg(
            &g,
            pos2(z2.right() - w, y),
            &count,
            h,
            theme::LCD_INK(),
            true,
        );
        let frac = if total == 0 {
            0.0
        } else {
            ready as f32 / total as f32
        };
        hw_vocal::lcd_ladder(
            &g,
            Rect::from_min_max(
                pos2(z2.left(), y + 2.0),
                pos2(z2.right() - w - 8.0, z2.bottom() - 1.0),
            ),
            frac,
        );
        ui.interact(z2, ui.scope_id().with("vocal_ready"), egui::Sense::hover())
            .on_hover_text(trf("{} of {} phrases ready", &[&ready, &total]));
    }

    // The render: the speeds measured, each with its place and the x real time, or the body's
    // shares.
    if let Some((speeds, meaning)) = &st.speeds {
        hw_vocal::lcd_label(&g, z3.min, tr("Speed"));
        let rows = [(Place::ThisMac.label(), speeds[0])];
        for (k, (label, v)) in rows.into_iter().enumerate() {
            // Each row: its label and its digits on one baseline.
            let base = z3.top() + 36.0 + k as f32 * 24.0;
            let h = 13.0;
            hw::text(
                &g,
                pos2(z3.left(), base),
                Align2::LEFT_BOTTOM,
                &label.to_uppercase(),
                Face::Lcd,
                8.0,
                label_ink,
            );
            let unit = hw::text(
                &g,
                pos2(z3.right(), base),
                Align2::RIGHT_BOTTOM,
                "X",
                Face::Lcd,
                8.0,
                label_ink,
            );
            let value = v.map_or_else(|| "-.--".to_string(), |r| format!("{r:.2}"));
            let w = hw::seg_width(&value, h);
            hw::seg(
                &g,
                pos2(unit.left() - 3.0 - w, base - h),
                &value,
                h,
                theme::LCD_INK(),
                true,
            );
        }
        ui.interact(z3, ui.scope_id().with("vocal_speeds"), egui::Sense::hover())
            .on_hover_text(meaning);
    } else if let Some(shares) = st.shares {
        // M / F / C columns; the letters are the first of each share's word as translated.
        let words = trf(
            "Male {}%, female {}%, child {}%",
            &[
                &format!("{:.0}", shares[0]),
                &format!("{:.0}", shares[1]),
                &format!("{:.0}", shares[2]),
            ],
        );
        let letters: Vec<String> = words
            .split(',')
            .map(|w| {
                w.trim()
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default()
            })
            .collect();
        let col = z3.width() / 3.0;
        for (k, v) in shares.iter().enumerate() {
            let x0 = z3.left() + k as f32 * col;
            hw::text(
                &g,
                pos2(x0 + col - 4.0, z3.top()),
                Align2::RIGHT_TOP,
                letters.get(k).map_or("", String::as_str),
                Face::Lcd,
                8.5,
                label_ink,
            );
            let value = format!("{v:.0}");
            let h = 13.0;
            let w = hw::seg_width(&value, h);
            hw::seg(
                &g,
                pos2(x0 + col - 4.0 - w, z3.top() + 22.0),
                &value,
                h,
                theme::LCD_INK(),
                true,
            );
            hw::text(
                &g,
                pos2(x0 + col - 4.0, z3.bottom()),
                Align2::RIGHT_BOTTOM,
                "%",
                Face::Lcd,
                8.0,
                label_ink,
            );
        }
        ui.interact(z3, ui.scope_id().with("vocal_shares"), egui::Sense::hover())
            .on_hover_text(words);
    }
}

/// The licence of the voice, as credits printed on a strip at the foot of the panel.
pub(crate) fn licence(ui: &mut egui::Ui, text: &str) {
    hw_vocal::gap(ui, 12.0);
    hw_vocal::credits(ui, text);
}

/// The Vocal Top Line window (Track > Vocal Top Line…): the voice and how it sings, where it
/// is sung, Render and Bounce.
fn show_window(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    if !s.vocal.open {
        return;
    }
    let sel = p.selected_track;
    let ti = if matches!(
        p.tracks.get(sel).and_then(|t| t.vocal.as_ref()),
        Some(VocalRole::Score(_))
    ) {
        Some(sel)
    } else {
        // The audio track selected: its score track; else the first Vocal Top Line.
        let link = p
            .tracks
            .get(sel)
            .and_then(|t| t.vocal.as_ref())
            .map(|v| v.link());
        p.tracks.iter().position(|t| match &t.vocal {
            Some(VocalRole::Score(v)) => link.is_none_or(|l| l == v.link),
            _ => false,
        })
    };
    let mut changed = false;
    let mut bounce_link = None;
    let mut pitch_link = None;
    let st = &mut s.vocal;
    if st
        .voices
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(5))
    {
        st.voices = Some((Instant::now(), voices()));
    }
    let voices = st.voices.as_ref().map(|v| v.1.clone()).unwrap_or_default();
    let subtitle = ti.map(|t| p.tracks[t].name.clone()).unwrap_or_default();
    // How many of the track's phrases are sung (in the cache) so far.
    let ready = ti.and_then(|ti| match &p.tracks[ti].vocal {
        Some(VocalRole::Score(set)) => {
            let phrases = track_phrases(p, &p.tracks[ti], set);
            let sung = phrases.iter().filter(|ph| cached(ph.key).is_file()).count();
            (!phrases.is_empty()).then_some((sung, phrases.len()))
        }
        _ => None,
    });
    let (open, _) = hw_vocal::window(
        ctx,
        egui::Id::unique("prowave_vocal_window"),
        tr("Vocal Top Line"),
        &subtitle,
        448.0,
        None,
        |ui| {
            let Some(ti) = ti else {
                status_lcd(
                    ui,
                    &Status {
                        singer: Singer::default(),
                        voice: "",
                        line: tr("No Vocal Top Line track: Track > New Vocal Top Line Track")
                            .to_string(),
                        note: String::new(),
                        busy: false,
                        ready: None,
                        speeds: None,
                        shares: None,
                    },
                );
                return;
            };
            let Some(VocalRole::Score(set)) = p.tracks[ti].vocal.as_mut() else {
                return;
            };
            let voice = voices.iter().find(|v| v.id == set.voice).cloned();
            let chimera = set.singer.is_chimera();
            let link = set.link;
            let t = st.tracks.entry(link).or_default();
            let (line, busy) = match &t.job {
                Some(job) => (
                    job.progress.lock().map(|s| s.clone()).unwrap_or_default(),
                    true,
                ),
                None => (t.last.clone(), false),
            };
            let note = String::new();
            let voice_name = voice_name(set, voice.as_ref());
            status_lcd(
                ui,
                &Status {
                    singer: set.singer,
                    voice: &voice_name,
                    line,
                    note,
                    busy,
                    ready,
                    shares: chimera.then(|| set.chimera_voice().shares()),
                    speeds: None,
                },
            );
            hw_vocal::gap(ui, 6.0);
            settings_panel(ui, set, &voices, &mut changed);
            hw_vocal::gap(ui, 12.0);
            let t = st.tracks.entry(link).or_default();
            let busy = t.job.is_some();
            // Render Now is the primary key: orange while phrases wait to be sung.
            let stale = !busy && ready.is_some_and(|(r, n)| r < n);
            let render_cap = if stale {
                hw::Cap::Orange
            } else {
                hw::Cap::Bone
            };
            let clicked = hw_vocal::key_row(
                ui,
                egui::Id::unique("vocal_keys"),
                &[
                    (tr("Render Now"), render_cap, Some(busy), true),
                    (tr("Bounce Vocal…"), hw::Cap::Bone, None, true),
                    (tr("Edit in Pitch Editor"), hw::Cap::Bone, None, true),
                ],
            );
            if clicked[0] {
                t.done = None;
                st.force = true;
            }
            if clicked[1] {
                bounce_link = Some(link);
            }
            if clicked[2] {
                pitch_link = Some(link);
            }
            if let Some(text) = credits(set, voice.as_ref()) {
                licence(ui, &text);
            }
        },
    );
    if changed {
        p.song_rev += 1;
    }
    if !open {
        s.vocal.open = false;
    }
    // The sung vocal's first phrase in the Pitch Editor (the others are a double-click away).
    if let Some(link) = pitch_link {
        match output_track(p, link).filter(|o| !p.tracks[*o].regions.is_empty()) {
            Some(o) => crate::pitch_editor::open(s, o, 0),
            None => s.status = Some(tr("Nothing sung yet: render the vocal first").into()),
        }
    }
    if let (Some(link), Some(ti)) = (bounce_link, ti) {
        let name = format!("{} Vocal.wav", p.tracks[ti].name);
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter("WAV", &["wav"])
            .save_file()
        {
            s.status = Some(match bounce(p, link, &path) {
                Ok(secs) => trf(
                    "Bounced {} s of vocal to {}",
                    &[&format!("{secs:.1}"), &path.display()],
                ),
                Err(e) => trf("Bounce failed: {}", &[&e]),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::Lyric;
    use crate::model::Note;

    fn note(start: f32, len: f32, pitch: u8, lyric: &str) -> Note {
        Note {
            start,
            len,
            pitch,
            vel: 100,
            mute: false,
            lyric: Lyric::new(lyric),
        }
    }

    /// A project with one Vocal Top Line track holding `notes` in a region at `bar`.
    fn project(notes: Vec<Note>, bar: f32) -> Project {
        let mut p = Project::empty();
        p.tempo = 120.0;
        let mut t = Track::new("Top", TrackKind::Instrument);
        // DiffSinger, whose words are cut into syllables (a new track's singer differs by build).
        let mut set = VocalSettings::new(7);
        set.singer = Singer::DiffSinger;
        t.vocal = Some(VocalRole::Score(set));
        t.regions.push(Region {
            missing: None,
            name: "Top".into(),
            start_bar: bar,
            len_bars: 8.0,
            notes,
            controls: Vec::new(),
            folder: None,
            audio: None,
            params: RegionParams::default(),
            generated: None,
        });
        p.tracks.push(t);
        p
    }

    fn settings(p: &Project) -> VocalSettings {
        match &p.tracks.last().unwrap().vocal {
            Some(VocalRole::Score(s)) => s.clone(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn notes_are_timed_in_seconds_from_the_tempo_map_with_a_tempo_change() {
        // 120 BPM, then 60 BPM from beat 4: beats 0, 2, 4, 6 are at 0, 1, 2 and 4 seconds.
        let mut p = project(
            vec![
                note(0.0, 1.0, 60, "row"),
                note(2.0, 1.0, 62, "row"),
                note(4.0, 1.0, 64, "row"),
                note(6.0, 2.0, 65, "boat"),
            ],
            1.0,
        );
        p.add_tempo_change(4.0, 60.0);
        let map = p.tempo_map();
        let t = p.tracks.last().unwrap();
        let ph = region_phrases(&p, &map, t, &t.regions[0], &settings(&p));
        // Rests of half a second and more split the phrases.
        assert_eq!(ph.len(), 4);
        let abs: Vec<(f64, f64)> = ph
            .iter()
            .map(|x| (x.origin + x.notes[0].start, x.notes[0].dur))
            .collect();
        for (got, want) in abs
            .iter()
            .zip([(0.0, 0.5), (1.0, 0.5), (2.0, 1.0), (4.0, 2.0)])
        {
            assert!(
                (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
                "{got:?} {want:?}"
            );
        }
        // Each phrase starts its sound PREROLL before its first note.
        assert!(ph.iter().all(|x| (x.notes[0].start - PREROLL).abs() < 1e-9));
        assert_eq!(ph[3].notes[0].lyric, "boat");
        // The score file carries the seconds and the settings.
        let json: serde_json::Value =
            serde_json::from_str(&score_json(&settings(&p), &[(&ph[2], "x.wav".into())])).unwrap();
        assert_eq!(json["phrases"][0]["notes"][0]["dur"].as_f64(), Some(1.0));
        assert_eq!(json["phrases"][0]["out_file"], "x.wav");
        assert_eq!(json["language"], "en");
    }

    #[test]
    fn continuation_notes_stay_in_the_phrase_and_overlaps_are_cut() {
        let p = project(
            vec![
                note(0.0, 1.5, 70, "you"),
                note(1.0, 0.25, 68, "-"),
                note(4.0, 0.5, 67, "gently"),
                note(6.0, 0.5, 65, "+"),
            ],
            1.0,
        );
        let map = p.tempo_map();
        let t = p.tracks.last().unwrap();
        let ph = region_phrases(&p, &map, t, &t.regions[0], &settings(&p));
        assert_eq!(ph.len(), 2, "the + joins its word across the rest");
        assert!(
            (ph[0].notes[0].dur - 0.5).abs() < 1e-9,
            "cut where the next note starts"
        );
        let lyrics: Vec<&str> = ph
            .iter()
            .flat_map(|x| &x.notes)
            .map(|n| n.lyric.as_str())
            .collect();
        assert_eq!(lyrics, ["you", "+", "gently", "+"]);
    }

    #[test]
    fn the_cache_key_follows_the_sound_not_the_place_in_the_song() {
        let notes = vec![note(0.0, 1.0, 60, "row"), note(1.0, 1.0, 62, "your")];
        let p1 = project(notes.clone(), 1.0);
        let p2 = project(notes.clone(), 3.0);
        let key = |p: &Project| {
            let t = p.tracks.last().unwrap();
            parts(p, t, &settings(p))
        };
        let (a, b) = (key(&p1), key(&p2));
        assert_eq!(
            a[0].phrases[0].key, b[0].phrases[0].key,
            "moved: the phrase is reused"
        );
        assert_ne!(a[0].key, b[0].key, "but its region is placed again");
        // A lyric, a pitch, a setting or the tempo changes the phrase.
        let mut p3 = project(notes.clone(), 1.0);
        p3.tracks.last_mut().unwrap().regions[0].notes[1].lyric = Lyric::new("my");
        assert_ne!(key(&p3)[0].phrases[0].key, a[0].phrases[0].key);
        let mut p4 = project(notes.clone(), 1.0);
        p4.tracks.last_mut().unwrap().regions[0].notes[0].pitch = 61;
        assert_ne!(key(&p4)[0].phrases[0].key, a[0].phrases[0].key);
        let mut p5 = project(notes.clone(), 1.0);
        if let Some(VocalRole::Score(s)) = &mut p5.tracks.last_mut().unwrap().vocal {
            s.vibrato_depth = 50.0;
        }
        assert_ne!(key(&p5)[0].phrases[0].key, a[0].phrases[0].key);
        let mut p6 = project(notes.clone(), 1.0);
        p6.tempo = 92.0;
        assert_ne!(key(&p6)[0].phrases[0].key, a[0].phrases[0].key);
        // Where it is sung does not change the sound.
        let mut p7 = project(notes, 1.0);
        if let Some(VocalRole::Score(s)) = &mut p7.tracks.last_mut().unwrap().vocal {
            s.place = Place::ThisMac;
        }
        assert_eq!(track_key(&key(&p7)), track_key(&a));
    }

    #[test]
    fn the_cache_key_follows_the_singer_and_its_settings() {
        let notes = vec![note(0.0, 1.0, 60, "row"), note(1.0, 1.0, 62, "-")];
        let with = |f: &dyn Fn(&mut VocalSettings)| {
            let mut p = project(notes.clone(), 1.0);
            if let Some(VocalRole::Score(s)) = &mut p.tracks.last_mut().unwrap().vocal {
                f(s);
            }
            let t = p.tracks.last().unwrap();
            parts(&p, t, &settings(&p))[0].phrases[0].clone()
        };
        let diffsinger = with(&|_| {});
        let chimera = with(&|s| s.singer = Singer::Chimera);
        assert_ne!(diffsinger.key, chimera.key, "another singer sings it again");
        let before = with(&|s| s.singer = Singer::Chimera09fb776);
        assert_ne!(before.key, chimera.key, "so does the other CHIMERA engine");
        assert_eq!(before.notes, chimera.notes, "with the same lyrics");
        // Novox: its own key, its voice and every knob count, the others' settings do not.
        let novox = with(&|s| s.singer = Singer::Novox);
        assert_ne!(novox.key, chimera.key);
        assert_ne!(novox.key, before.key);
        assert_eq!(
            novox.notes, chimera.notes,
            "Novox is given the lyrics as typed too"
        );
        for f in [
            |s: &mut VocalSettings| s.novox.preset = "female".into(),
            |s: &mut VocalSettings| s.novox.formant = 120.0,
            |s: &mut VocalSettings| s.novox.breathiness = 30.0,
            |s: &mut VocalSettings| s.novox.grit = 30.0,
            |s: &mut VocalSettings| s.novox.brightness = -30.0,
            |s: &mut VocalSettings| s.novox.voices = 2.0,
            |s: &mut VocalSettings| s.novox.vibrato_depth = 30.0,
            |s: &mut VocalSettings| s.novox.vibrato_rate = 7.0,
            |s: &mut VocalSettings| s.novox.scoop = -50.0,
            |s: &mut VocalSettings| s.novox.portamento = 90.0,
        ] {
            let k = with(&|s| {
                s.singer = Singer::Novox;
                f(s);
            });
            assert_ne!(k.key, novox.key);
            assert_eq!(
                with(&|s| f(s)).key,
                diffsinger.key,
                "Novox's knobs leave DiffSinger alone"
            );
        }
        let k = with(&|s| {
            s.singer = Singer::Novox;
            s.ring = 50.0;
            s.vibrato_depth = 120.0;
        });
        assert_eq!(
            k.key, novox.key,
            "the other singers' settings leave Novox alone"
        );
        // Each singer's own settings count; the other's do not.
        assert_ne!(
            with(&|s| {
                s.singer = Singer::Chimera;
                s.body_female = 100.0;
            })
            .key,
            chimera.key
        );
        for f in [
            |s: &mut VocalSettings| s.body_child = 40.0,
            |s: &mut VocalSettings| s.chimera_breathiness = 60.0,
            |s: &mut VocalSettings| s.brightness = 70.0,
            |s: &mut VocalSettings| s.ring = 30.0,
            |s: &mut VocalSettings| s.vibrato_depth = 60.0,
            |s: &mut VocalSettings| s.vibrato_rate = 7.0,
        ] {
            let k = with(&|s| {
                s.singer = Singer::Chimera;
                f(s);
            });
            assert_ne!(k.key, chimera.key);
        }
        let k = with(&|s| {
            s.singer = Singer::Chimera;
            s.gender = 50.0;
            s.voice = "Other".into();
        });
        assert_eq!(
            k.key, chimera.key,
            "DiffSinger's settings leave CHIMERA alone"
        );
        assert_eq!(
            with(&|s| s.ring = 30.0).key,
            diffsinger.key,
            "and CHIMERA's leave DiffSinger alone"
        );
        // CHIMERA is given the lyrics as typed: a hold stays a hold.
        let lyrics: Vec<&str> = chimera.notes.iter().map(|n| n.lyric.as_str()).collect();
        assert_eq!(lyrics, ["row", "-"]);
        let lyrics: Vec<&str> = diffsinger.notes.iter().map(|n| n.lyric.as_str()).collect();
        assert_eq!(lyrics, ["row", "+"]);
    }

    /// CHIMERA through the pipeline: a phrase of "la la la la" at 92 BPM is sung into the
    /// cache at the Vocal Top Line rate (resampled from CHIMERA's 48 kHz), as long as its notes
    /// and tail, sounding, and its notes at their times.
    #[test]
    fn chimera_sings_a_phrase_into_the_cache_at_the_vocal_rate() {
        let notes: Vec<Note> = [57u8, 60, 62, 64]
            .iter()
            .enumerate()
            .map(|(k, p)| note(1.0 + k as f32, 1.0, *p, "la"))
            .collect();
        let mut p = project(notes, 1.0);
        p.tempo = 92.0;
        if let Some(VocalRole::Score(s)) = &mut p.tracks.last_mut().unwrap().vocal {
            s.singer = Singer::Chimera;
            s.vibrato_depth = 0.0;
        }
        let t = p.tracks.last().unwrap();
        let ph = &parts(&p, t, &settings(&p))[0].phrases[0];
        let beat = 60.0 / 92.0;
        assert_eq!(ph.notes.len(), 4);
        for (k, n) in ph.notes.iter().enumerate() {
            assert!((n.start - (PREROLL + k as f64 * beat)).abs() < 1e-6);
        }
        let x = crate::chimera_singer::sing(&ph.notes, &settings(&p).chimera_voice());
        let y = crate::chimera_singer::resample(&x, crate::chimera_singer::RATE, SAMPLE_RATE);
        let rate = SAMPLE_RATE as f64;
        let want = x.len() as f64 / crate::chimera_singer::RATE as f64;
        assert!((y.len() as f64 / rate - want).abs() < 1e-3);
        let end = PREROLL + 4.0 * beat;
        assert!(y.len() as f64 / rate > end, "{} s", y.len() as f64 / rate);
        let rms = |a: f64, b: f64| {
            let s = &y[(a * rate) as usize..(b * rate) as usize];
            (s.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
        };
        // Sound in each note (from its first quarter on), none well before the first.
        let quiet = rms(0.0, 0.2);
        for k in 0..4 {
            let a = PREROLL + (k as f64 + 0.25) * beat;
            let loud = rms(a, a + 0.5 * beat);
            assert!(
                loud > 0.01 && loud > 20.0 * quiet,
                "note {k}: {loud} vs {quiet}"
            );
        }
    }

    #[test]
    fn phrases_are_placed_in_their_region_to_the_sample() {
        // A region at bar 3 (4 s at 120 BPM); a note at beat 2.5 of it (5.25 s), so its
        // phrase's sound starts at 4.75 s: sample 0.75 * 44100 of the region's audio.
        let p = project(vec![note(2.5, 1.0, 60, "la")], 3.0);
        let t = p.tracks.last().unwrap();
        let part = &parts(&p, t, &settings(&p))[0];
        assert!((part.start_secs - 4.0).abs() < 1e-9);
        let ph = &part.phrases[0];
        let at = sample_offset(ph.origin, part.start_secs);
        assert_eq!(at, 33_075);
        // An impulse at sample 10 of the phrase lands at 33_085 of the region's sound.
        let mut sound = vec![0f32; 100];
        sound[10] = 1.0;
        let mixed = mix_at(part.start_secs, &[(ph.origin, sound)]);
        assert_eq!(mixed.iter().position(|x| *x == 1.0), Some(33_085));
        // Before the region start is left out.
        let early = mix_at(1.0, &[(0.5, vec![1.0; 44_100])]);
        assert_eq!(early.len(), 22_050);
    }

    /// The sung vocal opens in the Pitch Editor like any audio, and its edits survive the
    /// vocal being sung again when a phrase comes back the same (the same cached file).
    #[test]
    fn pitch_editor_edits_of_the_sung_vocal_survive_singing_again() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
        crate::flex_edit::tests::write_notes(&a, &[(0.1, 0.5, 60.0)], 0.6);
        crate::flex_edit::tests::write_notes(&b, &[(0.1, 0.5, 62.0)], 0.6);
        let mut p = project(vec![note(0.0, 1.0, 60, "la")], 1.0);
        let ctx = egui::Context::default();
        let done = |file: &Path| Done {
            link: 7,
            singer: Singer::DiffSinger,
            parts: vec![Sung {
                start_bar: 1.0,
                start_secs: 0.0,
                file: file.to_path_buf(),
                secs: 0.6,
            }],
            place: None,
            secs: 0.0,
            sung_secs: 0.0,
            phrases: 1,
        };
        place_parts(&ctx, &mut p, &done(&a));
        let out = output_track(&p, 7).unwrap();
        let mut s = crate::UiState::new();
        crate::pitch_editor::open(&mut s, out, 0);
        assert_eq!(crate::pitch_editor::target(&p, &s), Some((out, 0)));
        let edit = crate::pitch_editor::MelodyEdit {
            algorithm: prowave_engine::Algorithm::Melodic,
            notes: vec![crate::pitch_editor::NoteEdit {
                shift: 2.0,
                ..crate::pitch_editor::NoteEdit::from_blob(&prowave_engine::Blob {
                    start: 0.1,
                    end: 0.5,
                    pitch: 60.0,
                    onset: 0.1,
                })
            }],
        };
        crate::pitch_editor::commit(&mut p, out, 0, edit.algorithm, edit.notes.clone());
        let melody = |p: &Project| {
            p.tracks[out].regions[0]
                .audio
                .as_ref()
                .and_then(|a| a.flex.as_ref())
                .and_then(|f| f.melody.clone())
        };
        place_parts(&ctx, &mut p, &done(&a));
        assert_eq!(melody(&p), Some(edit), "the same phrase keeps its edit");
        place_parts(&ctx, &mut p, &done(&b));
        assert_eq!(melody(&p), None, "a phrase sung differently starts clean");
    }

    #[test]
    fn wav_files_are_written_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.wav");
        write_wav(&path, &[0.0, 0.5, -0.25], SAMPLE_RATE).unwrap();
        let (s, rate) = read_wav(&path).unwrap();
        assert_eq!((s, rate), (vec![0.0, 0.5, -0.25], SAMPLE_RATE));
        let f = prowave_engine::AudioFile::open(&path).unwrap();
        assert_eq!((f.frames, f.sample_rate), (3, SAMPLE_RATE));
    }

    #[test]
    fn lyrics_and_vocal_settings_are_saved_with_the_project() {
        let mut p = project(
            vec![note(0.0, 1.0, 60, "row"), note(1.0, 1.0, 62, "+")],
            1.0,
        );
        if let Some(VocalRole::Score(s)) = &mut p.tracks.last_mut().unwrap().vocal {
            s.voice = "Voice A".into();
            s.color = "02: Fragrance (Power)".into();
            s.gender = -20.0;
            s.vibrato_depth = 40.0;
            s.place = Place::ThisMac;
            s.singer = Singer::Chimera;
            s.body_female = 70.0;
            s.body_child = 20.0;
            s.chimera_breathiness = 30.0;
            s.brightness = 60.0;
            s.ring = 45.0;
            s.novox = crate::novox_singer::Voice::preset("crooner").unwrap();
            s.novox.formant = 104.0;
            s.novox.breathiness = 20.0;
            s.novox.grit = 12.0;
            s.novox.brightness = -40.0;
            s.novox.voices = 2.0;
            s.novox.vibrato_depth = 55.0;
            s.novox.vibrato_rate = 6.5;
            s.novox.scoop = -90.0;
            s.novox.portamento = 140.0;
        }
        let mut out = Track::new("Top Vocal", TrackKind::Audio);
        out.vocal = Some(VocalRole::Output { link: 7 });
        p.tracks.push(out);
        let (doc, _) = crate::project_io::to_doc(&p);
        let json = serde_json::to_string(&doc).unwrap();
        let doc: prowave_project::ProjectDoc = serde_json::from_str(&json).unwrap();
        let (q, _) = crate::project_io::from_doc(&doc, Path::new("."), &egui::Context::default());
        let n = q.tracks.len();
        assert_eq!(q.tracks[n - 2].vocal, p.tracks[n - 2].vocal);
        assert_eq!(q.tracks[n - 1].vocal, Some(VocalRole::Output { link: 7 }));
        let lyrics: Vec<&str> = q.tracks[n - 2].regions[0]
            .notes
            .iter()
            .map(|n| n.lyric.as_str())
            .collect();
        assert_eq!(lyrics, ["row", "+"]);
        // Every singer's name round-trips.
        for singer in Singer::ALL {
            let mut set = VocalSettings::new(5);
            set.singer = singer;
            let back = from_doc(&to_doc(&VocalRole::Score(set.clone())));
            assert_eq!(back, Some(VocalRole::Score(set)), "{singer:?}");
        }
        // A file from before CHIMERA: DiffSinger, and CHIMERA's defaults.
        let old = prowave_project::VocalDoc {
            role: "score".into(),
            link: 3,
            ..Default::default()
        };
        let Some(VocalRole::Score(s)) = from_doc(&old) else {
            panic!("a score")
        };
        let fresh = VocalSettings::new(3);
        assert_eq!(s.singer, Singer::UNKNOWN);
        assert_eq!(
            (s.chimera_breathiness, s.brightness, s.ring),
            (fresh.chimera_breathiness, fresh.brightness, fresh.ring)
        );
        assert_eq!(s.novox, fresh.novox, "and Novox's");
        // Novox settings out of range or an unknown voice in a file are held to what it takes.
        let mut odd = to_doc(&VocalRole::Score(VocalSettings::new(4)));
        odd.singer = "novox".into();
        odd.novox_preset = "nobody".into();
        odd.novox_voices = 40.0;
        odd.novox_formant = f32::NAN;
        let Some(VocalRole::Score(s)) = from_doc(&odd) else {
            panic!("a score")
        };
        assert_eq!(s.singer, Singer::Novox);
        assert_eq!(s.novox.preset, crate::novox_singer::DEFAULT_PRESET);
        assert_eq!(
            (s.novox.voices, s.novox.formant),
            (8.0, fresh.novox.formant)
        );
        // Notes without lyrics leave them out of the file.
        let plain = serde_json::to_string(&prowave_project::NoteDoc::default()).unwrap();
        assert!(!plain.contains("lyric"));
    }

    #[test]
    fn settings_from_a_file_are_held_in_range() {
        let mut s = VocalSettings::new(1);
        s.gender = 500.0;
        s.vibrato_rate = f32::NAN;
        s.language = "xx".into();
        let s = s.clamped();
        assert_eq!(
            (s.gender, s.vibrato_rate, s.language.as_str()),
            (100.0, 5.5, "en")
        );
        assert_eq!(Place::from_name("?"), Place::Auto);
    }

    /// End to end with the installed singer: the Phase 1 test lines as Pro Wave regions (92 BPM,
    /// a tempo change to 70 BPM in the second), sung, placed and written to
    /// PROWAVE_VOCAL_E2E_OUT with each note's song seconds, for checking pitch and timing.
    #[test]
    #[ignore]
    fn e2e_sings_regions_in_place() {
        let Some(out) = std::env::var_os("PROWAVE_VOCAL_E2E_OUT").map(PathBuf::from) else {
            return;
        };
        let row = [
            ("row", 65, 1.0),
            ("row", 65, 1.0),
            ("row", 65, 0.75),
            ("your", 67, 0.25),
            ("boat", 69, 1.0),
            ("gently", 69, 0.75),
            ("+", 67, 0.25),
            ("down", 69, 0.75),
            ("the", 70, 0.25),
            ("stream", 72, 2.0),
        ];
        let rnb = [
            ("I", 75, 0.5),
            ("need", 72, 0.5),
            ("you", 70, 0.5),
            ("-", 68, 0.25),
            ("-", 67, 0.25),
            ("-", 65, 0.25),
            ("-", 63, 0.25),
            ("-", 65, 0.5),
            ("tonight", 67, 0.5),
            ("+", 68, 1.5),
        ];
        let mk = |line: &[(&str, u8, f32)]| {
            let mut t = 1.0f32;
            line.iter()
                .map(|(l, p, b)| {
                    let n = note(t, *b, *p, l);
                    t += b;
                    n
                })
                .collect::<Vec<_>>()
        };
        let mut p = project(mk(&row), 2.0);
        p.tempo = 92.0;
        p.tracks.last_mut().unwrap().regions.push(Region {
            missing: None,
            name: "rnb".into(),
            start_bar: 6.0,
            len_bars: 4.0,
            notes: mk(&rnb),
            controls: Vec::new(),
            folder: None,
            audio: None,
            params: RegionParams::default(),
            generated: None,
        });
        // 70 BPM from bar 7 (beat 24), in the middle of the second line.
        p.add_tempo_change(24.0, 70.0);
        let t = p.tracks.last().unwrap();
        let set = settings(&p);
        let parts = parts(&p, t, &set);
        let progress = Arc::new(Mutex::new(String::new()));
        let done = render(set, parts.clone(), vec![Place::ThisMac], progress).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let map = p.tempo_map();
        let mut report = Vec::new();
        for (k, (sung, part)) in done.parts.iter().zip(&parts).enumerate() {
            let file = out.join(format!("region{k}.wav"));
            std::fs::copy(&sung.file, &file).unwrap();
            let r = &t.regions[k];
            let origin = p.region_start_beats(r);
            let notes: Vec<serde_json::Value> = r
                .notes
                .iter()
                .map(|n| {
                    let s = map.seconds_at(origin + n.start as f64) - part.start_secs;
                    let e = map.seconds_at(origin + (n.start + n.len) as f64) - part.start_secs;
                    serde_json::json!({"start": s, "dur": e - s, "pitch": n.pitch,
                        "lyric": n.lyric.as_str()})
                })
                .collect();
            report.push(
                serde_json::json!({"file": file, "region_start_secs": part.start_secs,
                "notes": notes}),
            );
        }
        std::fs::write(
            out.join("notes.json"),
            serde_json::to_string_pretty(&report).unwrap(),
        )
        .unwrap();
        eprintln!(
            "sang {} phrases in {:.2} s ({:.2} s of singing)",
            done.phrases, done.secs, done.sung_secs
        );
    }

    /// End to end with CHIMERA: "Twinkle Twinkle Little Star" as a Pro Wave region (92 BPM,
    /// eight bars from bar 2), sung through the pipeline (cache, region sound) and written to
    /// PROWAVE_CHIMERA_DEMO (never over an existing file), with the speed: the pipeline's, and
    /// one core's for the whole phrase.
    #[test]
    #[ignore]
    fn e2e_chimera_sings_a_region() {
        let Some(demo) = std::env::var_os("PROWAVE_CHIMERA_DEMO").map(PathBuf::from) else {
            return;
        };
        assert!(
            !demo.exists(),
            "{} exists: renders are never overwritten",
            demo.display()
        );
        // Keep the phrase cache out of the installed singer's folder.
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("PROWAVE_VOCAL_HOME", home.path());
        let line = [
            ("twinkle", 48, 2.0),
            ("+", 48, 2.0),
            ("twinkle", 55, 2.0),
            ("+", 55, 2.0),
            ("little", 57, 2.0),
            ("+", 57, 2.0),
            ("star", 55, 4.0),
            ("how", 53, 2.0),
            ("I", 53, 2.0),
            ("wonder", 52, 2.0),
            ("+", 52, 2.0),
            ("what", 50, 2.0),
            ("you", 50, 2.0),
            ("are", 48, 4.0),
        ];
        let mut beat = 1.0f32;
        let notes: Vec<Note> = line
            .iter()
            .map(|(l, p, b)| {
                let n = note(beat, *b, *p, l);
                beat += b;
                n
            })
            .collect();
        let mut p = project(notes, 2.0);
        p.tempo = 92.0;
        p.tracks.last_mut().unwrap().regions[0].len_bars = 9.0;
        if let Some(VocalRole::Score(s)) = &mut p.tracks.last_mut().unwrap().vocal {
            s.singer = Singer::Chimera;
        }
        let t = p.tracks.last().unwrap();
        let set = settings(&p);
        let parts = parts(&p, t, &set);
        let ph = &parts[0].phrases[0];
        assert_eq!(ph.notes.len(), line.len());
        let started = Instant::now();
        let x = crate::chimera_singer::sing(&ph.notes, &set.chimera_voice());
        let one_core = started.elapsed().as_secs_f64();
        let audio = x.len() as f64 / crate::chimera_singer::RATE as f64;
        let progress = Arc::new(Mutex::new(String::new()));
        let done = render(set, parts, Vec::new(), progress).unwrap();
        let (s, rate) = read_wav(&done.parts[0].file).unwrap();
        assert_eq!(rate, SAMPLE_RATE);
        let peak = s.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(
            peak > 0.02 && s.iter().all(|v| v.is_finite()),
            "peak {peak}"
        );
        write_wav(&demo, &s, SAMPLE_RATE).unwrap();
        eprintln!(
            "{}: {:.2} s at {} Hz, peak {:.3}. One core: {:.2} s for {:.2} s of singing ({:.2}x real time). Pipeline: {} phrase(s) in {:.2} s",
            demo.display(),
            s.len() as f64 / rate as f64,
            rate,
            peak,
            one_core,
            audio,
            one_core / audio,
            done.phrases,
            done.secs
        );
    }
}
