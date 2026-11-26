//! Bounce: the project, or a section of it, played into an audio file (Logic: File > Bounce >
//! Project or Section), and every track into a file of its own (File > Export > All Tracks as
//! Audio Files).
//!
//! What is written is what the app plays. A second engine is made that has no sound card
//! (`prowave_engine::Engine::offline`) and it is handed a copy of the project by the same
//! code that hands the running engine the project: `EngineSync` for the song, the strips, the
//! routes, the automation and the delay compensation, `PluginHost` for the instruments and
//! the effects (new instances with the settings the running ones have). Its blocks are then
//! asked for on a thread of the bounce, faster than they would be played, and written to the
//! file. Nothing here is a second renderer: the audio comes out of `EngineCore::process`.
//!
//! What differs from playing, so that every run gives the same file:
//! - an audio region waits for the disk thread instead of dropping out;
//! - every plug-in runs in the block it is asked for (no playback threads);
//! - Audio Units and VST3s are told they render offline;
//! - the built-in instruments that are handed their sounds by the app's frame (the Singer,
//!   Pro Wave Tune and the like) are handed them here before the first block, and the bounce
//!   waits until they have them;
//! - the transport stops on the frame the range ends at, and what is then still sounding
//!   (releases, reverb, delay compensation) is kept as the tail.
//!
//! The running engine is not touched: the app goes on playing while a bounce runs.
use crate::engine_sync::EngineSync;
use crate::model::{Project, TrackKind};
use crate::plugins::{OfflineLinks, PluginHost, PluginSlot};
use prowave_engine::{Engine, OfflineOutput};
use std::collections::VecDeque;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Frames a block of the bounce's engine: every plug-in is opened for it and delays its
/// track by it, as an I/O buffer of this size would.
pub const BLOCK: usize = 512;
/// The largest block a plug-in is opened for (as the running engine's).
const MAX_PLUGIN_FRAMES: usize = 8192;
/// Below this (-80 dB) the tail counts as over.
const TAIL_FLOOR: f32 = 1.0e-4;
/// The tail ends this long after its last sample above [`TAIL_FLOOR`].
const TAIL_PAD_S: f64 = 0.25;
/// The longest bounce, in seconds of sound.
pub const MAX_SECONDS: f64 = 4.0 * 3600.0;
/// How long the app's own background work (Flex, Fit to Grid) is waited for.
const WAIT_FOR_APP: Duration = Duration::from_secs(300);
/// How long an engine may take to have everything (plug-ins opened, Sampler sounds made,
/// Space Designer responses) before the bounce goes on without what is missing.
const WAIT_FOR_ENGINE: Duration = Duration::from_secs(120);
/// Blocks played with the transport stopped once everything is in place, so what was sent
/// last is taken in before the first frame.
const PRE_ROLL_BLOCKS: usize = 24;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Wav,
    Aiff,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Aiff => "aiff",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Wav => "WAV",
            Format::Aiff => "AIFF",
        }
    }

    /// The format a file name asks for by its ending.
    pub fn of_path(path: &Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("wav" | "wave") => Some(Format::Wav),
            Some("aif" | "aiff") => Some(Format::Aiff),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth {
    Pcm16,
    Pcm24,
    Float32,
}

impl Depth {
    pub fn bits(self) -> u16 {
        match self {
            Depth::Pcm16 => 16,
            Depth::Pcm24 => 24,
            Depth::Float32 => 32,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Depth::Pcm16 => "16-bit",
            Depth::Pcm24 => "24-bit",
            Depth::Float32 => "32-bit float",
        }
    }
}

/// What a bounce is asked for.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The file of the mix. Each track's file goes beside it, named after it and the track.
    pub path: PathBuf,
    /// Start and end in quarter notes; `None`: the cycle range while Cycle is on, else from
    /// the start of the song to the end of its last region.
    pub range: Option<(f64, f64)>,
    pub format: Format,
    pub depth: Depth,
    /// Raises (or lowers) each file so its loudest sample is at full scale.
    pub normalize: bool,
    /// The least that is kept after the end, in seconds.
    pub tail_seconds: f64,
    /// The most: the tail goes on while the output is above -80 dB, up to here.
    pub max_tail_seconds: f64,
    /// Write the mix.
    pub mix: bool,
    /// Write every track that is heard into a file of its own.
    pub stems: bool,
    /// Only this track, into `path` (the other tracks play nothing; `mix` and `stems` are
    /// not looked at).
    pub only: Option<usize>,
    pub overwrite: bool,
    /// Bounce in Place and Join: the project is cut down to one track's own strip first (see
    /// [`Strip`]). Used with `only` set to the same track.
    pub strip: Option<Strip>,
}

/// What a Bounce in Place (or a Join that makes a new file) hears: track `track` with only
/// its regions `regions`, out of its own strip and nothing after it. Its sends, its output
/// to a bus and the Stereo Out's fader, effects and automation are left out (the new region
/// plays through them again), and so are, when asked, its effects, its fader and pan.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    pub track: usize,
    /// Indexes into the track's regions.
    pub regions: Vec<usize>,
    /// The track's effects (built-in and plug-ins) and their automation.
    pub effects: bool,
    /// The track's fader and pan, their automation and its VCA.
    pub fader: bool,
    /// The track's Mute automation.
    pub mute_automation: bool,
    /// Muted regions are heard (Convert Regions to New Audio Files: a muted region's file
    /// holds its sound, and the new region is muted as the old one was).
    pub unmute_regions: bool,
}

impl Strip {
    /// Cuts `q` down to what is heard. Fails when the regions are not there any more.
    fn apply(&self, q: &mut Project) -> Result<(), String> {
        let gone = || "the regions to bounce are not there any more".to_string();
        // Follow Rack Groove on Auto: the class Auto picks from all the track's regions, not
        // from the ones bounced.
        if let Some((_, c)) = crate::rack_follow::target(q, self.track) {
            q.tracks[self.track].params.follow_rack.class = Some(c);
        }
        let t = q.tracks.get_mut(self.track).ok_or_else(gone)?;
        if self.regions.is_empty() || self.regions.iter().any(|&r| r >= t.regions.len()) {
            return Err(gone());
        }
        let mut i = 0;
        t.regions.retain(|_| {
            let keep = self.regions.contains(&i);
            i += 1;
            keep
        });
        if self.unmute_regions {
            for r in &mut t.regions {
                r.params.mute = false;
            }
        }
        t.mute = false;
        t.output = 0;
        t.sends.clear();
        t.auto.sends.clear();
        if !self.effects {
            t.inserts.clear();
            t.au_effects.clear();
            t.auto.fx.clear();
            // Slot 0 is the instrument, which stays.
            t.auto.plugins.retain(|l| l.slot == 0);
        }
        if !self.fader {
            t.volume_db = 0.0;
            t.pan = 0.0;
            t.vca = None;
            t.auto.volume.clear();
            t.auto.pan.clear();
        }
        if !self.mute_automation {
            t.auto.mute.clear();
        }
        q.master_vol = 1.0;
        q.master_inserts.clear();
        q.master_au_effects.clear();
        q.master_auto = Default::default();
        q.song_rev += 1;
        Ok(())
    }
}

impl Settings {
    /// The mix into `path`, in the format its ending names (WAV otherwise), 24-bit, with a
    /// tail of one to twenty seconds.
    pub fn mix_to(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            format: Format::of_path(&path).unwrap_or(Format::Wav),
            path,
            range: None,
            depth: Depth::Pcm24,
            normalize: false,
            tail_seconds: 1.0,
            max_tail_seconds: 20.0,
            mix: true,
            stems: false,
            only: None,
            overwrite: false,
            strip: None,
        }
    }
}

/// One file a bounce wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct FileReport {
    pub path: PathBuf,
    /// The track it holds (`None`: the mix).
    pub track: Option<String>,
    /// The track's place in the project, when it holds one.
    pub track_index: Option<usize>,
    pub seconds: f64,
    pub frames: u64,
    /// Of what was written (after Normalize), linear. 0 for a silent file.
    pub peak: f32,
    pub rms: f32,
    /// The tail kept after the end of the range, in seconds.
    pub tail_seconds: f64,
    /// The gain Normalize applied, linear (1 without it).
    pub gain: f32,
    /// Samples at full scale before Normalize: the engine's output is cut off there, as the
    /// app's is, so more than a few of these are heard as distortion.
    pub clipped: u64,
    /// How fast it was rendered: seconds of sound for each second of work.
    pub speed: f64,
}

impl FileReport {
    pub fn peak_db(&self) -> Option<f64> {
        db(f64::from(self.peak))
    }
    pub fn rms_db(&self) -> Option<f64> {
        db(f64::from(self.rms))
    }
    pub fn silent(&self) -> bool {
        self.peak <= 1.0e-6
    }
}

/// `x` (linear) in dB; `None` for silence.
pub fn db(x: f64) -> Option<f64> {
    (x > 1.0e-9).then(|| 20.0 * x.log10())
}

/// What a bounce did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub files: Vec<FileReport>,
    /// The range, in quarter notes.
    pub from: f64,
    pub to: f64,
    /// The range's length in seconds (without the tail).
    pub range_seconds: f64,
    pub sample_rate: u32,
    pub format: String,
    /// The tracks that are in the mix: (place in the project, name, what plays it).
    pub rendered: Vec<(usize, String, String)>,
    /// The tracks that are not heard in the app either (Mute, or another track's Solo), and
    /// so are not in the mix: not a fault.
    pub muted: Vec<String>,
    /// Everything that should be in the bounce and is not, each with its reason. Empty when
    /// the bounce holds all that the app plays.
    pub not_rendered: Vec<String>,
    /// Seconds from the start of the bounce to its end, everything included.
    pub took_seconds: f64,
}

impl Report {
    /// Seconds of sound rendered for each second the rendering took, over all files.
    pub fn speed(&self) -> f64 {
        let sound: f64 = self.files.iter().map(|f| f.seconds).sum();
        let work: f64 = self
            .files
            .iter()
            .map(|f| f.seconds / f.speed.max(1.0e-9))
            .sum();
        if work > 0.0 {
            sound / work
        } else {
            0.0
        }
    }
}

/// How far a bounce is.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// 0 to 1 over the whole bounce.
    pub fraction: f32,
    /// The file being made, from 1, and how many there are.
    pub file: usize,
    pub files: usize,
    /// The track being rendered (`None`: the mix).
    pub track: Option<String>,
    /// Still getting ready (the app's own work, plug-ins opening, sounds being made).
    pub preparing: bool,
}

/// One file to make.
#[derive(Clone, Debug)]
struct Pass {
    /// The one track heard (`None`: the mix).
    track: Option<usize>,
    path: PathBuf,
}

/// The offline engine of one pass with everything that feeds it.
struct Rig {
    q: Project,
    sync: EngineSync,
    out: Option<OfflineOutput>,
    host: PluginHost,
    singers: Vec<(u64, prowave_engine::SingerRemote, bool)>,
    tunes: Vec<(u64, prowave_engine::tune::TuneRemote, bool)>,
    scratch: Vec<f32>,
    began: Instant,
    /// Steps in a row in which nothing was left to do.
    steady: u32,
    /// What could not be opened or handed over, for the report.
    faults: Vec<String>,
    started: bool,
}

/// A pass being rendered on its thread.
struct Running {
    rig: Rig,
    handle: Option<std::thread::JoinHandle<(OfflineOutput, Result<Rendered, String>)>>,
    done: Arc<AtomicU64>,
    total: u64,
}

/// What the render thread made.
struct Rendered {
    file: FileReport,
    underruns: u32,
    non_finite: u64,
}

enum State {
    /// The app is still making sounds the project plays (Flex, Fit to Grid).
    WaitForApp(Instant),
    /// The next pass's engine is being set up.
    Open(Box<Rig>),
    Render(Box<Running>),
    Idle,
}

/// A bounce in progress. Made by [`Job::start`]; [`Job::poll`] is called every frame on the
/// interface thread until it gives the result.
pub struct Job {
    settings: Settings,
    rate: u32,
    /// The project as it is bounced (set once the app's own work is done).
    base: Option<Project>,
    from: f64,
    to: f64,
    frames: u64,
    passes: VecDeque<Pass>,
    pass_count: usize,
    current: Option<Pass>,
    state: State,
    report: Report,
    cancel: Arc<AtomicBool>,
    began: Instant,
}

/// The range a bounce of `p` covers when none is given: the cycle range while Cycle is on,
/// else from the start of the song to the end of its last region (loops included).
pub fn default_range(p: &Project) -> Option<(f64, f64)> {
    if p.cycle && p.cycle_end > p.cycle_start {
        return Some((p.cycle_start, p.cycle_end));
    }
    whole_range(p)
}

/// The whole song: from its start to the end of its last region (`None` without regions).
pub fn whole_range(p: &Project) -> Option<(f64, f64)> {
    let end = song_end(p);
    (end > 0.0).then_some((0.0, end))
}

/// Where the last region of `p` ends, in quarter notes (0 without regions).
pub fn song_end(p: &Project) -> f64 {
    p.tracks
        .iter()
        .flat_map(|t| t.regions.iter())
        .map(|r| p.region_loop_end_beats(r))
        .fold(0.0, f64::max)
}

/// Whether track `i` of `p` makes sound of its own (regions or an instrument), as opposed to
/// an Aux, a bus, a VCA or a folder.
fn is_source(p: &Project, i: usize) -> bool {
    p.tracks.get(i).is_some_and(|t| {
        matches!(
            t.kind,
            TrackKind::Instrument | TrackKind::Audio | TrackKind::Drummer | TrackKind::ExternalMidi
        )
    })
}

/// Whether track `i` of `p` has anything to play: regions, or an instrument (which may play
/// by itself). An empty track is left out of the lists and gets no file of its own.
fn has_content(p: &Project, i: usize) -> bool {
    p.tracks
        .get(i)
        .is_some_and(|t| !t.regions.is_empty() || t.au_instrument.is_some())
}

/// Whether track `i` is heard in the app as the project stands: not muted (by itself or its
/// VCA), and soloed when any track is.
fn heard(p: &Project, i: usize) -> bool {
    if i >= p.tracks.len() {
        return false;
    }
    let any_solo = (0..p.tracks.len()).any(|j| p.track_mix_of(j).2);
    let (_, mute, solo) = p.track_mix_of(i);
    !mute && (!any_solo || solo)
}

/// A file name made from a track's name.
pub(crate) fn file_safe(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | ':' | '\\' | '\0') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() {
        "Track".into()
    } else {
        s
    }
}

/// The file of track `i` beside the mix's `path`: "Mix - 03 Bass.wav".
pub fn stem_path(path: &Path, i: usize, name: &str, format: Format) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Bounce");
    path.with_file_name(format!(
        "{stem} - {:02} {}.{}",
        i + 1,
        file_safe(name),
        format.extension()
    ))
}

impl Job {
    /// Checks what is asked for against `p` and makes the job. Nothing is rendered yet.
    /// `rate` is the sample rate of the files (the running engine's).
    pub fn start(p: &Project, rate: u32, settings: Settings) -> Result<Self, String> {
        if settings.format == Format::Aiff && settings.depth == Depth::Float32 {
            return Err("an AIFF file is 16-bit or 24-bit: choose WAV for 32-bit float".into());
        }
        if !settings.tail_seconds.is_finite()
            || settings.tail_seconds < 0.0
            || !settings.max_tail_seconds.is_finite()
        {
            return Err("the tail must be zero or more seconds".into());
        }
        if settings.only.is_none() && !settings.mix && !settings.stems {
            return Err("nothing to write: neither the mix nor the tracks were asked for".into());
        }
        if let Some(i) = settings.only {
            if i >= p.tracks.len() {
                return Err(format!("there is no track {}", i + 1));
            }
        }
        let mut job = Self {
            settings,
            rate: rate.max(1),
            base: None,
            from: 0.0,
            to: 0.0,
            frames: 0,
            passes: VecDeque::new(),
            pass_count: 0,
            current: None,
            state: State::WaitForApp(Instant::now()),
            report: Report::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            began: Instant::now(),
        };
        // The range and the files are checked now, against the project as it is, so that
        // a mistake is answered at once; they are worked out again when the bounce begins.
        job.plan(p)?;
        Ok(job)
    }

    /// Stops the bounce: the files being written are removed and [`Job::poll`] gives an error.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn progress(&self) -> Progress {
        let files = self.pass_count.max(1);
        let finished =
            files.saturating_sub(self.passes.len() + usize::from(self.current.is_some()));
        let (within, preparing) = match &self.state {
            State::Render(r) => (
                r.done.load(Ordering::Relaxed) as f32 / r.total.max(1) as f32,
                false,
            ),
            State::Idle => (0.0, false),
            _ => (0.0, true),
        };
        let track = self
            .current
            .as_ref()
            .and_then(|pass| pass.track)
            .and_then(|i| self.base.as_ref()?.tracks.get(i).map(|t| t.name.clone()));
        Progress {
            fraction: ((finished as f32 + within.clamp(0.0, 1.0)) / files as f32).clamp(0.0, 1.0),
            file: (finished + 1).min(files),
            files,
            track,
            preparing,
        }
    }

    /// The range, the frames and the files, from `p` as it is now.
    fn plan(&mut self, p: &Project) -> Result<(), String> {
        let s = &self.settings;
        let (from, to) = match s.range {
            Some(r) => r,
            None => default_range(p).ok_or_else(|| {
                "nothing to bounce: the project has no regions (give a range to bounce one anyway)"
                    .to_string()
            })?,
        };
        if !(from.is_finite() && to.is_finite()) || to <= from || from < 0.0 {
            return Err("the range must end after it starts".into());
        }
        let map = p.tempo_map();
        let seconds = map.seconds_at(to) - map.seconds_at(from);
        if seconds + s.max_tail_seconds.max(s.tail_seconds) > MAX_SECONDS {
            return Err(format!(
                "the range is {seconds:.0} seconds long; a bounce is {MAX_SECONDS:.0} seconds at most"
            ));
        }
        let mut passes = VecDeque::new();
        if let Some(i) = s.only {
            passes.push_back(Pass {
                track: Some(i),
                path: s.path.clone(),
            });
        } else {
            if s.mix {
                passes.push_back(Pass {
                    track: None,
                    path: s.path.clone(),
                });
            }
            if s.stems {
                for (i, t) in p.tracks.iter().enumerate() {
                    if is_source(p, i)
                        && t.kind != TrackKind::ExternalMidi
                        && has_content(p, i)
                        && heard(p, i)
                    {
                        passes.push_back(Pass {
                            track: Some(i),
                            path: stem_path(&s.path, i, &t.name, s.format),
                        });
                    }
                }
            }
        }
        if passes.is_empty() {
            return Err("nothing to write: no track of the project is heard".into());
        }
        if !s.overwrite {
            if let Some(there) = passes.iter().find(|pass| pass.path.exists()) {
                return Err(format!(
                    "{} is already there (it is not replaced unless that is asked for)",
                    there.path.display()
                ));
            }
        }
        self.from = from;
        self.to = to;
        self.frames = (seconds * f64::from(self.rate)).round().max(1.0) as u64;
        self.pass_count = passes.len();
        self.passes = passes;
        self.report.from = from;
        self.report.to = to;
        self.report.range_seconds = seconds;
        self.report.sample_rate = self.rate;
        self.report.format = format!("{}, {}", s.format.name(), s.depth.name());
        Ok(())
    }

    /// The project as the bounce plays it: `live` with the settings the running plug-ins
    /// have now, and without what belongs to playing by hand.
    fn take_project(&mut self, live: &mut Project, plugins: &mut PluginHost) -> Result<(), String> {
        plugins.save_states(live);
        let mut q = live.clone();
        q.refresh_generated();
        if let Some(strip) = &self.settings.strip {
            strip.apply(&mut q)?;
        }
        self.plan(&q)?;
        // A slot's number names its running instance; the bounce's instances are others.
        for slot in crate::plugins::slots_mut(&mut q) {
            slot.uid = crate::plugins::new_uid();
        }
        q.playing = false;
        q.recording = false;
        q.counting_in = false;
        q.count_in = false;
        q.metronome = false;
        // The range is played once, from its start to its end.
        q.cycle = false;
        q.position_beats = self.from;
        q.midi_clock_in = None;
        q.mtc_out = None;
        for t in &mut q.tracks {
            t.rec = false;
            t.monitor = false;
        }
        q.song_rev += 1;
        self.describe(&q);
        self.base = Some(q);
        Ok(())
    }

    /// What is in the mix and what is not, for the report.
    fn describe(&mut self, q: &Project) {
        let r = &mut self.report;
        r.rendered.clear();
        r.muted.clear();
        r.not_rendered.clear();
        let only = self.settings.only;
        for (i, t) in q.tracks.iter().enumerate() {
            if !is_source(q, i) || !has_content(q, i) || only.is_some_and(|o| o != i) {
                continue;
            }
            if only.is_none() && !heard(q, i) {
                r.muted.push(t.name.clone());
                continue;
            }
            if t.kind == TrackKind::ExternalMidi {
                if !t.regions.is_empty() {
                    r.not_rendered.push(format!(
                        "\"{}\": an External MIDI track plays an instrument outside the app",
                        t.name
                    ));
                }
                continue;
            }
            let missing = t.regions.iter().filter(|x| x.missing.is_some()).count();
            if missing > 0 {
                r.not_rendered.push(format!(
                    "\"{}\": {missing} audio regions whose files are missing",
                    t.name
                ));
            }
            let what = match (&t.au_instrument, &t.soundfont, t.kind) {
                (Some(slot), _, _) => slot.name.clone(),
                (None, Some(sf), _) => sf.path.clone(),
                (None, None, TrackKind::Audio) => "audio regions".to_string(),
                _ => "no instrument".to_string(),
            };
            r.rendered.push((i, t.name.clone(), what));
        }
    }

    /// One step, on the interface thread, every frame: `busy` says the app is still making
    /// sounds the project plays (Flex, Fit to Grid). Gives the result once, when the bounce
    /// is over; the job is then done with.
    pub fn poll(
        &mut self,
        live: &mut Project,
        plugins: &mut PluginHost,
        busy: bool,
    ) -> Option<Result<Report, String>> {
        match self.step(live, plugins, busy) {
            Ok(false) => None,
            Ok(true) => {
                self.report.took_seconds = self.began.elapsed().as_secs_f64();
                Some(Ok(self.report.clone()))
            }
            Err(e) => {
                self.abandon();
                Some(Err(e))
            }
        }
    }

    /// Lets go of what is being made (after an error or Cancel).
    fn abandon(&mut self) {
        self.cancel.store(true, Ordering::Release);
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Render(mut r) => {
                if let Some(h) = r.handle.take() {
                    if let Ok((out, _)) = h.join() {
                        r.rig.out = Some(out);
                    }
                }
                r.rig.dismantle();
            }
            State::Open(rig) => rig.dismantle(),
            _ => {}
        }
        if let Some(pass) = self.current.take() {
            remove_parts(&pass.path);
        }
    }

    fn cancelled(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Acquire) {
            Err("cancelled".into())
        } else {
            Ok(())
        }
    }

    /// Returns whether the bounce is finished.
    fn step(
        &mut self,
        live: &mut Project,
        plugins: &mut PluginHost,
        busy: bool,
    ) -> Result<bool, String> {
        self.cancelled()?;
        match std::mem::replace(&mut self.state, State::Idle) {
            State::WaitForApp(since) => {
                if busy {
                    if since.elapsed() > WAIT_FOR_APP {
                        return Err(
                            "the app is still making the project's sounds (Flex, Fit to Grid): try again when it is done"
                                .into(),
                        );
                    }
                    self.state = State::WaitForApp(since);
                    return Ok(false);
                }
                self.take_project(live, plugins)?;
                self.next_pass()?;
                Ok(false)
            }
            State::Open(mut rig) => {
                // Several turns a frame, for as long as each still changes something.
                let turn = Instant::now();
                let ready = loop {
                    if rig.step() {
                        break true;
                    }
                    if rig.waiting_on_others() || turn.elapsed() > Duration::from_millis(12) {
                        break false;
                    }
                };
                if !ready {
                    self.state = State::Open(rig);
                    return Ok(false);
                }
                let pass = self.current.clone().ok_or("no file to write")?;
                let running = self.launch(*rig, &pass)?;
                self.state = State::Render(Box::new(running));
                Ok(false)
            }
            State::Render(mut r) => {
                if !r.handle.as_ref().is_some_and(|h| h.is_finished()) {
                    self.state = State::Render(r);
                    return Ok(false);
                }
                let joined = r
                    .handle
                    .take()
                    .ok_or("the render thread is gone")?
                    .join()
                    .map_err(|_| "the render thread stopped".to_string());
                let (out, result) = match joined {
                    Ok(x) => x,
                    Err(e) => {
                        r.rig.dismantle();
                        return Err(e);
                    }
                };
                r.rig.out = Some(out);
                let faults = std::mem::take(&mut r.rig.faults);
                r.rig.dismantle();
                let made = result?;
                let pass = self.current.take().ok_or("no file to write")?;
                let name = pass
                    .track
                    .and_then(|i| self.base.as_ref()?.tracks.get(i).map(|t| t.name.clone()));
                for f in faults {
                    if !self.report.not_rendered.contains(&f) {
                        self.report.not_rendered.push(f);
                    }
                }
                if made.underruns > 0 {
                    self.report.not_rendered.push(format!(
                        "{}: audio regions dropped out {} times (a file could not be read)",
                        name.as_deref().unwrap_or("the mix"),
                        made.underruns
                    ));
                }
                if made.non_finite > 0 {
                    self.report.not_rendered.push(format!(
                        "{}: {} samples were not numbers and were made silent",
                        name.as_deref().unwrap_or("the mix"),
                        made.non_finite
                    ));
                }
                self.report.files.push(FileReport {
                    track: name,
                    track_index: pass.track,
                    ..made.file
                });
                if self.passes.is_empty() {
                    return Ok(true);
                }
                self.next_pass()?;
                Ok(false)
            }
            State::Idle => Ok(true),
        }
    }

    /// Sets up the engine of the next file.
    fn next_pass(&mut self) -> Result<(), String> {
        let pass = self.passes.pop_front().ok_or("no file to write")?;
        let base = self.base.as_ref().ok_or("no project")?;
        let rig = Rig::new(pass_project(base, pass.track), self.rate)?;
        self.current = Some(pass);
        self.state = State::Open(Box::new(rig));
        Ok(())
    }

    /// Starts the transport of `rig` and the thread that asks for its blocks.
    fn launch(&mut self, mut rig: Rig, pass: &Pass) -> Result<Running, String> {
        let mut out = rig.out.take().ok_or("the engine has no output")?;
        let rate = f64::from(self.rate);
        // Up to the Stereo Out the engine says how late the mix is; its own effects come
        // after that.
        let skip = u64::from(rig.sync.output_delay_frames()) + rig.master_latency(self.rate);
        let min_tail = (self.settings.tail_seconds * rate).round() as u64;
        let max_tail = ((self.settings.max_tail_seconds * rate).round() as u64).max(min_tail);
        let plan = RenderPlan {
            skip,
            frames: self.frames,
            min_tail,
            max_tail,
            rate: self.rate,
            path: pass.path.clone(),
            format: self.settings.format,
            depth: self.settings.depth,
            normalize: self.settings.normalize,
        };
        if let Some(dir) = pass.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        out.play_for(self.frames);
        let done = Arc::new(AtomicU64::new(0));
        let total = skip + self.frames + min_tail;
        let (progress, cancel) = (done.clone(), self.cancel.clone());
        let handle = std::thread::Builder::new()
            .name("prowave-bounce".into())
            .spawn(move || {
                let result = render(&mut out, &plan, &progress, &cancel);
                if result.is_err() {
                    remove_parts(&plan.path);
                }
                (out, result)
            })
            .map_err(|e| format!("cannot start the render thread: {e}"))?;
        Ok(Running {
            rig,
            handle: Some(handle),
            done,
            total,
        })
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        if !matches!(self.state, State::Idle) {
            self.abandon();
        }
    }
}

/// The project of one pass: `base` for the mix; for a track, `base` with every other track
/// that makes sound of its own silent (no regions, no instrument, muted), so the one track is
/// heard through its strip, its sends and their Aux strips, and the Stereo Out.
pub(crate) fn pass_project(base: &Project, only: Option<usize>) -> Project {
    let mut q = base.clone();
    let Some(only) = only else {
        return q;
    };
    let any_solo = (0..q.tracks.len()).any(|j| q.track_mix_of(j).2);
    // The Channel Rack the track follows (Follow Rack Groove) keeps its regions and its rack, so
    // the track's notes still take its feel; it stays silent.
    let followed = crate::rack_follow::target(&q, only).map(|(r, _)| r);
    for i in 0..q.tracks.len() {
        if !is_source(&q, i) {
            continue;
        }
        let t = &mut q.tracks[i];
        if Some(i) == followed && i != only {
            t.mute = true;
            t.solo = false;
            t.sends.clear();
            t.au_effects.clear();
            t.inserts.clear();
            continue;
        }
        if i == only {
            t.mute = false;
            // Heard whatever Solo says elsewhere; the strips that are not sources keep
            // their Solo, so an Aux that is not heard in the mix is not heard here either.
            t.solo = any_solo;
        } else {
            t.mute = true;
            t.solo = false;
            t.regions.clear();
            t.au_instrument = None;
            t.au_effects.clear();
            t.inserts.clear();
            t.soundfont = None;
        }
    }
    q.song_rev += 1;
    q
}

/// The strip (a track's place, or `mixer::MASTER_STRIP`) and the slot with this number.
fn slot_by_uid(q: &Project, uid: u64) -> Option<(usize, &PluginSlot)> {
    for (i, t) in q.tracks.iter().enumerate() {
        if let Some(s) = t
            .au_instrument
            .iter()
            .chain(t.au_effects.iter())
            .find(|s| s.uid == uid)
        {
            return Some((i, s));
        }
    }
    q.master_au_effects
        .iter()
        .find(|s| s.uid == uid)
        .map(|s| (crate::mixer::MASTER_STRIP, s))
}

impl Rig {
    fn new(q: Project, rate: u32) -> Result<Self, String> {
        let (engine, out) = Engine::offline(rate, BLOCK).map_err(|e| e.to_string())?;
        let sync = EngineSync::new(engine, &q);
        let mut host = PluginHost::default();
        host.offline = Some(OfflineLinks::default());
        Ok(Self {
            q,
            sync,
            out: Some(out),
            host,
            singers: Vec::new(),
            tunes: Vec::new(),
            scratch: vec![0.0; BLOCK * 2],
            began: Instant::now(),
            steady: 0,
            faults: Vec::new(),
            started: false,
        })
    }

    fn fault(&mut self, text: String) {
        if !self.faults.contains(&text) {
            self.faults.push(text);
        }
    }

    /// Plays `blocks` blocks into nothing: the engine takes what it was sent.
    fn run(&mut self, blocks: usize) {
        if let Some(out) = self.out.as_mut() {
            for _ in 0..blocks {
                out.process(&mut self.scratch);
            }
        }
    }

    /// Whether what is still missing is being made by another thread (a Sampler's sounds, a
    /// Space Designer response): then there is no point in turning again at once.
    fn waiting_on_others(&self) -> bool {
        self.steady == 0 && !self.host.needs_retry()
    }

    /// The delay of the Stereo Out's own effects in frames: the engine's delay compensation
    /// ends before them.
    fn master_latency(&self, rate: u32) -> u64 {
        let sr = rate as f32;
        let built_in: usize = self
            .q
            .master_inserts
            .iter()
            .filter(|i| !i.bypass)
            .map(|i| i.kind.latency_samples(sr))
            .sum();
        let plugins: f64 = self.host.master_latencies().iter().sum();
        built_in as u64 + (plugins * f64::from(rate)).round() as u64
    }

    /// One turn of what the app's frame does for the running engine: the plug-ins, then the
    /// project, then the built-in instruments' sounds; the engine then runs a few blocks to
    /// take it all in. Returns whether the transport has been started (the engine has
    /// everything and plays from the next block on).
    fn step(&mut self) -> bool {
        if self.started {
            return true;
        }
        for message in self
            .host
            .sync(&mut self.q, &mut self.sync.engine, MAX_PLUGIN_FRAMES)
        {
            self.fault(format!("could not be opened: {message}"));
        }
        let sr = f64::from(self.sync.engine.sample_rate());
        self.sync.process_buffer = 0;
        self.sync.low_latency = false;
        self.sync.plugin_shape = (0..self.q.tracks.len())
            .map(|i| {
                let (inst, fx) = self.host.shape(i);
                let own = (self.host.latency_seconds(i) * sr).round() as usize;
                (inst, fx, own)
            })
            .collect();
        self.sync.sync(&mut self.q);
        let fed = self.feed();
        self.run(4);
        let timed_out = self.began.elapsed() > WAIT_FOR_ENGINE;
        let settled = !self.host.needs_retry() && self.sync.settled(&self.q) && fed;
        if !settled && !timed_out {
            self.steady = 0;
            return false;
        }
        self.steady += 1;
        if self.steady < 3 && !timed_out {
            return false;
        }
        if !settled {
            self.fault(
                "the engine did not have everything in time: a plug-in, an instrument's sounds or a Space Designer response may be missing"
                    .into(),
            );
        }
        self.run(PRE_ROLL_BLOCKS);
        // From its next block on the engine plays.
        self.q.playing = true;
        self.sync.sync(&mut self.q);
        if !self.sync.settled(&self.q) {
            // No room for Play yet: the blocks above made some.
            self.q.playing = false;
            self.steady = 0;
            self.run(4);
            return false;
        }
        self.started = true;
        true
    }

    /// Hands the built-in instruments opened so far what the app's frame hands the running
    /// ones. Returns whether each has all it plays.
    fn feed(&mut self) -> bool {
        let rate = self.sync.engine.sample_rate();
        if let Some(links) = self.host.offline.as_mut() {
            for (uid, remote) in links.singers.drain(..) {
                self.singers.push((uid, remote, false));
            }
            for (uid, remote) in links.tunes.drain(..) {
                self.tunes.push((uid, remote, false));
            }
        }
        let mut all = true;
        let mut faults = Vec::new();
        for (uid, remote, sent) in &mut self.singers {
            if *sent {
                continue;
            }
            let Some((ti, _)) = slot_by_uid(&self.q, *uid) else {
                continue;
            };
            let (score, phrases, have) = crate::singer::offline_score(&self.q, ti, rate);
            if have < phrases {
                faults.push(format!(
                    "\"{}\": {} of the Singer's {phrases} phrases have not been sung yet (open the project and let it sing, then bounce again)",
                    self.q.tracks[ti].name,
                    phrases - have
                ));
            }
            match score {
                Some(score) => {
                    if remote.send(score).is_ok() {
                        *sent = true;
                    } else {
                        all = false;
                    }
                }
                None => *sent = true,
            }
        }
        for (uid, remote, sent) in &mut self.tunes {
            if *sent {
                continue;
            }
            let Some((strip, slot)) = slot_by_uid(&self.q, *uid) else {
                continue;
            };
            if crate::tune_lane::tell_offline(&self.q, strip, slot, remote) {
                *sent = true;
            } else {
                all = false;
            }
        }
        for f in faults {
            self.fault(f);
        }
        all
    }

    /// Frees the engine: the plug-ins' handles first, then the engine's audio side with the
    /// plug-ins in it, then its queues and its disk thread.
    fn dismantle(self) {
        let Rig {
            host, out, sync, ..
        } = self;
        drop(host);
        drop(out);
        drop(sync);
    }
}

/// What the render thread is told.
struct RenderPlan {
    /// Frames before the range's first frame comes out (delay compensation).
    skip: u64,
    /// The range's length.
    frames: u64,
    min_tail: u64,
    max_tail: u64,
    rate: u32,
    path: PathBuf,
    format: Format,
    depth: Depth,
    normalize: bool,
}

/// The file the samples are kept in until the bounce's own file is written from them.
fn raw_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bounce-part");
    path.with_file_name(name)
}

/// The bounce's file while it is being written.
fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

fn remove_parts(path: &Path) {
    let _ = std::fs::remove_file(raw_path(path));
    let _ = std::fs::remove_file(part_path(path));
}

/// Asks `out` for its blocks from the frame the transport starts on: leaves out the delay
/// compensation's `skip` frames, keeps the range and its tail, and writes the file.
fn render(
    out: &mut OfflineOutput,
    plan: &RenderPlan,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<Rendered, String> {
    let began = Instant::now();
    let raw = raw_path(&plan.path);
    let io = |e: std::io::Error| format!("{}: {e}", raw.display());
    let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&raw).map_err(io)?);
    let block = out.block_frames();
    let mut buf = vec![0f32; block * 2];
    let main_end = plan.skip + plan.frames;
    let soft_end = main_end + plan.min_tail;
    let hard_end = main_end + plan.max_tail;
    let pad = (TAIL_PAD_S * f64::from(plan.rate)) as u64;
    // Quiet frames of the tail past its least length: kept only if something loud follows.
    let mut held: Vec<f32> = Vec::new();
    let (mut seen, mut written) = (0u64, 0u64);
    let (mut peak, mut sum, mut bad, mut clipped) = (0f32, 0f64, 0u64, 0u64);
    let mut last_loud = 0u64;
    let mut finished = false;
    while !finished {
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        out.process(&mut buf);
        for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let at = seen + i as u64;
            if at < plan.skip {
                continue;
            }
            for s in f.iter_mut() {
                if !s.is_finite() {
                    bad += 1;
                    *s = 0.0;
                }
            }
            let loud = f[0].abs().max(f[1].abs());
            if loud > TAIL_FLOOR {
                last_loud = at;
            }
            if at >= hard_end || (at >= soft_end && at > last_loud + pad) {
                finished = true;
                break;
            }
            if at >= soft_end && loud <= TAIL_FLOOR {
                held.extend_from_slice(f);
                continue;
            }
            // A loud frame in the tail: what was held before it belongs to the file.
            for s in held.drain(..).chain(f.iter().copied()) {
                peak = peak.max(s.abs());
                clipped += u64::from(s.abs() >= 1.0);
                sum += f64::from(s) * f64::from(s);
                w.write_all(&s.to_le_bytes()).map_err(io)?;
                written += 1;
            }
        }
        seen += block as u64;
        progress.store(seen, Ordering::Relaxed);
    }
    // The tail ends `pad` after its last loud frame: of the quiet frames held, those.
    let frames_now = written / 2;
    let wanted = (last_loud + 1 + pad)
        .saturating_sub(plan.skip)
        .clamp(plan.frames + plan.min_tail, plan.frames + plan.max_tail);
    let extra = (wanted.saturating_sub(frames_now) as usize * 2).min(held.len());
    for s in &held[..extra] {
        w.write_all(&s.to_le_bytes()).map_err(io)?;
        written += 1;
    }
    w.flush().map_err(io)?;
    drop(w);
    let frames = written / 2;
    let rendering = began.elapsed().as_secs_f64();
    let gain = if plan.normalize && peak > 1.0e-6 {
        1.0 / peak
    } else {
        1.0
    };
    let result = write_file(&raw, plan, frames, gain);
    let _ = std::fs::remove_file(&raw);
    result?;
    let seconds = frames as f64 / f64::from(plan.rate);
    let rms = if written == 0 {
        0.0
    } else {
        (sum / written as f64).sqrt() as f32
    };
    Ok(Rendered {
        file: FileReport {
            path: plan.path.clone(),
            track: None,
            track_index: None,
            seconds,
            frames,
            peak: peak * gain,
            rms: rms * gain,
            tail_seconds: frames.saturating_sub(plan.frames) as f64 / f64::from(plan.rate),
            gain,
            clipped,
            speed: seconds / rendering.max(1.0e-9),
        },
        underruns: out.underruns(),
        non_finite: bad,
    })
}

/// The bounce's file from the samples kept in `raw` (interleaved stereo, 32-bit float, little
/// endian), each times `gain`. Written beside its name first, so nobody sees half a file.
fn write_file(raw: &Path, plan: &RenderPlan, frames: u64, gain: f32) -> Result<(), String> {
    let part = part_path(&plan.path);
    let io = |e: std::io::Error| format!("{}: {e}", plan.path.display());
    let bytes_per = u64::from(plan.depth.bits() / 8);
    let data_len = frames * 2 * bytes_per;
    let data32 = u32::try_from(data_len + 64)
        .map(|_| data_len as u32)
        .map_err(|_| "too long for one file (4 GB)".to_string())?;
    let mut r = BufReader::with_capacity(1 << 20, std::fs::File::open(raw).map_err(io)?);
    let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&part).map_err(io)?);
    let big_endian = match plan.format {
        Format::Wav => {
            let float = plan.depth == Depth::Float32;
            let block_align = 2 * bytes_per as u16;
            let mut h = Vec::with_capacity(44);
            h.extend_from_slice(b"RIFF");
            h.extend_from_slice(&(36 + data32).to_le_bytes());
            h.extend_from_slice(b"WAVEfmt ");
            h.extend_from_slice(&16u32.to_le_bytes());
            h.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
            h.extend_from_slice(&2u16.to_le_bytes());
            h.extend_from_slice(&plan.rate.to_le_bytes());
            h.extend_from_slice(&(plan.rate * u32::from(block_align)).to_le_bytes());
            h.extend_from_slice(&block_align.to_le_bytes());
            h.extend_from_slice(&plan.depth.bits().to_le_bytes());
            h.extend_from_slice(b"data");
            h.extend_from_slice(&data32.to_le_bytes());
            w.write_all(&h).map_err(io)?;
            false
        }
        Format::Aiff => {
            let frames32 = u32::try_from(frames).map_err(|_| "too long for an AIFF file")?;
            let mut h = Vec::with_capacity(54);
            h.extend_from_slice(b"FORM");
            h.extend_from_slice(&(4 + 26 + 16 + data32).to_be_bytes());
            h.extend_from_slice(b"AIFFCOMM");
            h.extend_from_slice(&18u32.to_be_bytes());
            h.extend_from_slice(&2u16.to_be_bytes());
            h.extend_from_slice(&frames32.to_be_bytes());
            h.extend_from_slice(&plan.depth.bits().to_be_bytes());
            h.extend_from_slice(&extended(plan.rate));
            h.extend_from_slice(b"SSND");
            h.extend_from_slice(&(8 + data32).to_be_bytes());
            h.extend_from_slice(&[0u8; 8]);
            w.write_all(&h).map_err(io)?;
            true
        }
    };
    let mut chunk = vec![0u8; 1 << 16];
    let mut left = frames * 2;
    let mut bytes = Vec::with_capacity((1 << 16) / 4 * bytes_per as usize);
    while left > 0 {
        let want = (left as usize * 4).min(chunk.len());
        r.read_exact(&mut chunk[..want]).map_err(io)?;
        bytes.clear();
        for c in chunk[..want].as_chunks::<4>().0 {
            let x = f32::from_le_bytes(*c) * gain;
            match plan.depth {
                Depth::Float32 => bytes.extend_from_slice(&x.to_le_bytes()),
                Depth::Pcm16 => {
                    let v = (f64::from(x) * 32_767.0).round().clamp(-32_768.0, 32_767.0) as i16;
                    if big_endian {
                        bytes.extend_from_slice(&v.to_be_bytes());
                    } else {
                        bytes.extend_from_slice(&v.to_le_bytes());
                    }
                }
                Depth::Pcm24 => {
                    let v = (f64::from(x) * 8_388_607.0)
                        .round()
                        .clamp(-8_388_608.0, 8_388_607.0) as i32;
                    let b = v.to_le_bytes();
                    if big_endian {
                        bytes.extend_from_slice(&[b[2], b[1], b[0]]);
                    } else {
                        bytes.extend_from_slice(&b[..3]);
                    }
                }
            }
        }
        w.write_all(&bytes).map_err(io)?;
        left -= (want / 4) as u64;
    }
    // An AIFF sound chunk has an even length; two channels of 2 or 3 bytes always give one.
    w.flush().map_err(io)?;
    let mut f = w.into_inner().map_err(|e| e.to_string())?;
    f.seek(SeekFrom::End(0)).map_err(io)?;
    f.sync_all().map_err(io)?;
    drop(f);
    std::fs::rename(&part, &plan.path).map_err(io)
}

/// `rate` as the 80-bit float an AIFF file keeps its sample rate in.
fn extended(rate: u32) -> [u8; 10] {
    let mut out = [0u8; 10];
    if rate == 0 {
        return out;
    }
    let shift = rate.leading_zeros();
    let exponent = 16_383 + 31 - shift as u16;
    out[..2].copy_from_slice(&exponent.to_be_bytes());
    out[2..6].copy_from_slice(&(rate << shift).to_be_bytes());
    out
}

/// Runs a bounce of `p` to its end on this thread, for the tests. `plugins` is the host of the
/// running plug-ins, whose settings are read.
#[cfg(test)]
pub fn run_blocking(
    p: &mut Project,
    plugins: &mut PluginHost,
    rate: u32,
    settings: Settings,
) -> Result<Report, String> {
    let mut job = Job::start(p, rate, settings)?;
    loop {
        if let Some(result) = job.poll(p, plugins, false) {
            return result;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
#[path = "bounce_tests.rs"]
mod tests;
