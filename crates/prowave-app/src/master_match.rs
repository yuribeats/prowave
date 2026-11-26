//! Mix > Mastering > Match to Reference…: a song (an audio file, the selected audio region,
//! the whole project or its cycle range, bounced first) mastered so its loudness, frequency
//! balance, peak level and stereo width follow a reference track. The work is the engine's
//! port of Matchering (`prowave_engine::reference_match`), run on a background thread with
//! progress and Cancel.
//!
//! The window shows the matching EQ the match made (mid and side, on the LCD), the loudness of
//! the target, the reference and the result, lets the original and the matched preview (the
//! loudest 30 seconds, as Matchering's preview) be compared at equal loudness, and writes the
//! result to a file or onto a new track.
use crate::hw::{self, Cap};
use crate::i18n::{tr, trf};
use crate::model::{Project, Region, RegionParams, Track, TrackKind};
use crate::spectrogram_view::hw_vocal;
use crate::theme::{self, Face};
use egui::{pos2, vec2, Align2, Id, Rect, Sense, Stroke};
use prowave_engine::reference_match::{
    self as rm, loudness, Ceiling, Config, Depth, Finish, Input, MatchError, Matched, Stereo,
};
use prowave_engine::{AudioFile, AuditionMsg, AuditionRemote, SampleBuf, AUDITION_LAYERS};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

/// The ceilings offered: Matchering's own (sample peak at -0.016 dBFS), then true-peak ones.
const CEILINGS: [Option<f64>; 5] = [None, Some(-0.1), Some(-0.3), Some(-0.5), Some(-1.0)];
const DEPTHS: [Depth; 3] = [Depth::Pcm16, Depth::Pcm24, Depth::Float32];
const WIDTH: f32 = 560.0;

/// What is mastered.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    File(PathBuf),
    /// An audio region's file, and where the region sits (the matched track goes there too).
    Region {
        name: String,
        file: PathBuf,
        start_bar: f32,
        len_bars: f32,
        offset_secs: f64,
    },
    /// The project bounced first (`cycle`: the cycle range), as File > Bounce does.
    Project {
        cycle: bool,
    },
}

impl Target {
    fn shown(&self) -> String {
        match self {
            Self::File(p) => file_name(p),
            Self::Region { name, .. } => name.clone(),
            Self::Project { cycle: false } => tr("Project").to_string(),
            Self::Project { cycle: true } => tr("Cycle Range").to_string(),
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The finish and the file, as the window is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub limiter: bool,
    pub normalize: bool,
    /// Index into [`CEILINGS`].
    pub ceiling: usize,
    /// Index into [`DEPTHS`].
    pub depth: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            limiter: true,
            normalize: true,
            ceiling: 0,
            depth: 1,
        }
    }
}

impl Settings {
    /// Set to the finish `f`.
    fn take_finish(&mut self, f: Finish) {
        self.limiter = f.limiter;
        self.normalize = f.normalize;
        self.ceiling = match f.ceiling {
            Ceiling::Matchering => 0,
            Ceiling::TruePeak(db) => CEILINGS.iter().position(|c| *c == Some(db)).unwrap_or(0),
        };
    }

    fn finish(&self) -> Finish {
        Finish {
            limiter: self.limiter,
            normalize: self.normalize,
            ceiling: match CEILINGS[self.ceiling.min(CEILINGS.len() - 1)] {
                None => Ceiling::Matchering,
                Some(db) => Ceiling::TruePeak(db),
            },
        }
    }
}

/// Where a running job is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Stage {
    Reading = 0,
    Matching = 1,
    Finishing = 2,
}

/// A match (or only its finish) running on its thread.
struct Job {
    progress: Arc<AtomicU32>,
    stage: Arc<AtomicU8>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Result<Outcome, String>>,
}

impl Job {
    fn fraction(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    fn stage(&self) -> Stage {
        match self.stage.load(Ordering::Relaxed) {
            0 => Stage::Reading,
            1 => Stage::Matching,
            _ => Stage::Finishing,
        }
    }
}

/// What a job hands back.
enum Outcome {
    /// A whole match.
    Matched(Box<Done>),
    /// A new finish of the match there is.
    Finished(Finished),
}

/// The finished result and what is measured on it.
struct Finished {
    result: Arc<Stereo>,
    finish: Finish,
    lufs: Option<f64>,
    true_peak_db: f64,
    preview: Arc<SampleBuf>,
}

/// A match that is done.
struct Done {
    matched: Arc<Matched>,
    /// The target as it was matched (two channels at 44.1 kHz), for the previews.
    target: Arc<Stereo>,
    target_lufs: Option<f64>,
    reference_lufs: Option<f64>,
    /// The original's preview and its loudness, the matched preview's loudness.
    original: Arc<SampleBuf>,
    original_preview_lufs: Option<f64>,
    finished: Finished,
    /// What was matched, for the file and the track.
    target_src: Target,
}

/// Which preview is heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Side {
    Original,
    #[default]
    Matched,
}

/// A file panel to open on the next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    Target,
    Reference,
    Save,
}

/// A file being written: where, its outcome, and whether it then goes on a new track.
struct Saving {
    path: PathBuf,
    rx: Receiver<Result<(), String>>,
    add: bool,
}

/// The window's state.
#[derive(Default)]
pub struct MatchUi {
    pub open: bool,
    target: Option<Target>,
    reference: Option<PathBuf>,
    settings: Settings,
    pick: Option<Pick>,
    job: Option<Job>,
    done: Option<Done>,
    /// The last failure, on the LCD until the next match.
    error: Option<String>,
    side: Side,
    playing: bool,
    level_match: bool,
    /// What the preview voice was last told: (playing, side, level match, the result's serial).
    sent: Option<(bool, Side, bool, u64)>,
    /// Counts finishes, so a new one is sent to the preview.
    serial: u64,
    /// Where the preview voice is (0 to 1) while it plays.
    phase: Option<f32>,
    /// Writing a file.
    saving: Option<Saving>,
    /// The project bounce a Project target waits for, and its file.
    bounce: Option<(crate::bounce::Job, PathBuf)>,
    /// MATCH was pressed with a Project target: the bounce starts on the next frame.
    bounce_wanted: Option<bool>,
}

impl MatchUi {
    pub fn new() -> Self {
        Self {
            level_match: true,
            ..Self::default()
        }
    }

    fn busy(&self) -> bool {
        if self.bounce.is_some() {
            return true;
        }
        self.job.is_some()
    }
}

/// Opens or closes the window (the menu entry).
pub fn toggle(s: &mut crate::UiState) {
    s.master_match.open = !s.master_match.open;
}

/// The words for what stopped a match.
fn error_words(e: &MatchError) -> String {
    match e {
        MatchError::TargetTooLong => tr("The target is longer than 15 minutes").into(),
        MatchError::TargetTooShort => tr("The target is too short").into(),
        MatchError::ReferenceTooLong => tr("The reference is longer than 15 minutes").into(),
        MatchError::ReferenceTooShort => tr("The reference is too short").into(),
        MatchError::TargetChannels => tr("The target has more than two channels").into(),
        MatchError::ReferenceChannels => tr("The reference has more than two channels").into(),
        MatchError::TargetEqualsReference => tr("The target and the reference are the same").into(),
        MatchError::Cancelled => "cancelled".into(),
    }
}

/// Reads `path` as Matchering takes it: its channels (one or two; more are refused), at its
/// rate, at most a little over 15 minutes (a longer file is refused by the checks).
fn read(path: &Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<Input, String> {
    let file =
        AudioFile::open(path).map_err(|e| trf("Cannot read {}: {}", &[&file_name(path), &e]))?;
    if file.channels > 2 {
        return Err(String::new());
    }
    let max = Config::default().max_length + 1.0;
    let st = file
        .read_stereo_up_to(max, |p, _| progress(p))
        .map_err(|e| trf("Cannot read {}: {}", &[&file_name(path), &e]))?
        .ok_or_else(|| "cancelled".to_string())?;
    let n = st.len() / 2;
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for [a, b] in st.as_chunks::<2>().0 {
        l.push(f64::from(*a));
        r.push(f64::from(*b));
    }
    drop(st);
    let channels = if file.channels == 1 {
        vec![l]
    } else {
        vec![l, r]
    };
    Ok(Input {
        channels,
        rate: file.sample_rate,
    })
}

fn to_buf(s: &Stereo, rate: u32) -> Arc<SampleBuf> {
    Arc::new(SampleBuf::new(rate, s.interleaved()))
}

fn lufs(s: &Stereo, rate: u32) -> Option<f64> {
    loudness::integrated_lufs(&[&s.l, &s.r], rate)
}

/// Finishes `m` and measures the result; its preview is cut where Matchering cuts it.
fn finish_and_measure(
    m: &Matched,
    target: &Stereo,
    how: Finish,
    cfg: &Config,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<(Finished, Stereo), String> {
    let result =
        rm::finish(m, &how, cfg, &mut |x| progress(x * 0.7)).map_err(|e| error_words(&e))?;
    if !progress(0.75) {
        return Err("cancelled".into());
    }
    let rate = cfg.internal_sample_rate;
    let lufs = lufs(&result, rate);
    let true_peak_db = loudness::db(loudness::true_peak(&[&result.l, &result.r]));
    if !progress(0.9) {
        return Err("cancelled".into());
    }
    let (orig, piece, _) = rm::preview(target, &result, cfg);
    Ok((
        Finished {
            result: Arc::new(result),
            finish: how,
            lufs,
            true_peak_db,
            preview: to_buf(&piece, rate),
        },
        orig,
    ))
}

/// Starts matching `target` to `reference`.
fn start_match(m: &mut MatchUi, target: Target, file: PathBuf, reference: PathBuf) {
    let progress = Arc::new(AtomicU32::new(0));
    let stage = Arc::new(AtomicU8::new(Stage::Reading as u8));
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    let how = m.settings.finish();
    let (p, st, c) = (progress.clone(), stage.clone(), cancel.clone());
    let run = move || -> Result<Outcome, String> {
        let set = |x: f32| {
            p.store(x.to_bits(), Ordering::Relaxed);
            !c.load(Ordering::Relaxed)
        };
        let cfg = Config::default();
        let t_in = read(&file, &mut |x| set(0.1 * x));
        // The project's bounce is a temporary file: once read it is not needed.
        if matches!(target, Target::Project { .. }) {
            let _ = std::fs::remove_file(&file);
        }
        let t_in = t_in.map_err(|e| {
            if e.is_empty() {
                error_words(&MatchError::TargetChannels)
            } else {
                e
            }
        })?;
        let r_in = read(&reference, &mut |x| set(0.1 + 0.1 * x)).map_err(|e| {
            if e.is_empty() {
                error_words(&MatchError::ReferenceChannels)
            } else {
                e
            }
        })?;
        st.store(Stage::Matching as u8, Ordering::Relaxed);
        let (t, r, _) = rm::prepare(t_in, r_in, &cfg).map_err(|e| error_words(&e))?;
        if !set(0.25) {
            return Err("cancelled".into());
        }
        let rate = cfg.internal_sample_rate;
        let target_lufs = lufs(&t, rate);
        let reference_lufs = lufs(&r, rate);
        let matched = rm::match_to(&t, &r, &cfg, &mut |x| set(0.25 + 0.55 * x))
            .map_err(|e| error_words(&e))?;
        drop(r);
        st.store(Stage::Finishing as u8, Ordering::Relaxed);
        let (finished, orig) =
            finish_and_measure(&matched, &t, how, &cfg, &mut |x| set(0.8 + 0.2 * x))?;
        let original_preview_lufs = lufs(&orig, rate);
        Ok(Outcome::Matched(Box::new(Done {
            matched: Arc::new(matched),
            target: Arc::new(t),
            target_lufs,
            reference_lufs,
            original: to_buf(&orig, rate),
            original_preview_lufs,
            finished,
            target_src: target,
        })))
    };
    let spawned = std::thread::Builder::new()
        .name("prowave-match".into())
        .spawn(move || {
            let _ = tx.send(run());
        });
    match spawned {
        Ok(_) => {
            m.job = Some(Job {
                progress,
                stage,
                cancel,
                rx,
            });
            m.error = None;
        }
        Err(e) => m.error = Some(e.to_string()),
    }
}

/// Finishes the match there is again with the window's settings.
fn start_finish(m: &mut MatchUi) {
    let Some(done) = m.done.as_ref() else {
        return;
    };
    let progress = Arc::new(AtomicU32::new(0));
    let stage = Arc::new(AtomicU8::new(Stage::Finishing as u8));
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    let how = m.settings.finish();
    let (matched, target) = (done.matched.clone(), done.target.clone());
    let (p, c) = (progress.clone(), cancel.clone());
    let spawned = std::thread::Builder::new()
        .name("prowave-match-finish".into())
        .spawn(move || {
            let cfg = Config::default();
            let res = finish_and_measure(&matched, &target, how, &cfg, &mut |x| {
                p.store(x.to_bits(), Ordering::Relaxed);
                !c.load(Ordering::Relaxed)
            })
            .map(|(f, _)| Outcome::Finished(f));
            let _ = tx.send(res);
        });
    if spawned.is_ok() {
        m.job = Some(Job {
            progress,
            stage,
            cancel,
            rx,
        });
    }
}

/// A name for the matched file: the target's, with "Matched".
fn result_stem(t: &Target) -> String {
    let base = match t {
        Target::File(p) => p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        Target::Region { name, .. } => name.clone(),
        Target::Project { .. } => String::new(),
    };
    let base = base.trim();
    if base.is_empty() {
        tr("Matched Master").to_string()
    } else {
        format!("{base} {}", tr("Matched Master"))
    }
}

/// Writes the result to `path` on a thread; `add`: then put it on a new track.
fn start_save(m: &mut MatchUi, path: PathBuf, add: bool) {
    let Some(done) = m.done.as_ref() else {
        return;
    };
    let result = done.finished.result.clone();
    let depth = DEPTHS[m.settings.depth.min(DEPTHS.len() - 1)];
    let rate = Config::default().internal_sample_rate;
    let (tx, rx) = channel();
    let to = path.clone();
    let spawned = std::thread::Builder::new()
        .name("prowave-match-save".into())
        .spawn(move || {
            let _ = tx.send(rm::write_wav(&to, &result, rate, depth).map_err(|e| e.to_string()));
        });
    if spawned.is_ok() {
        m.saving = Some(Saving { path, rx, add });
    }
}

/// A new audio track holding `path`, at `start_bar` (`len_bars`, `offset_secs`) or, without
/// them, at the playhead.
fn add_track(
    p: &mut Project,
    path: &Path,
    name: &str,
    at: Option<(f32, f32, f64)>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let file = AudioFile::open(path).map_err(|e| e.to_string())?;
    let (start_bar, len_bars, offset_secs) = match at {
        Some(at) => at,
        None => {
            let start_bar = p.beats_to_bar(p.position_beats).floor() + 1.0;
            let start = p.bar_to_beats(start_bar - 1.0);
            let end = p.tempo_map().advance(start, file.duration_secs());
            let len = p.beats_to_bar(end) - (start_bar - 1.0);
            (start_bar as f32, len as f32, 0.0)
        }
    };
    let mut track = Track::new(name, TrackKind::Audio);
    track.regions.push(Region {
        missing: None,
        name: name.to_string(),
        start_bar,
        len_bars,
        notes: Vec::new(),
        controls: Vec::new(),
        folder: None,
        generated: None,
        audio: Some(crate::import::audio_region(file, ctx)),
        params: RegionParams {
            offset_secs,
            ..RegionParams::default()
        },
    });
    p.tracks.push(track);
    p.song_rev += 1;
    Ok(())
}

/// Once a frame: the file panels, the jobs, the window and the preview voice.
pub fn frame(app: &mut crate::App, ctx: &egui::Context) {
    pick_files(app);
    if let Some(cycle) = app.ui.master_match.bounce_wanted.take() {
        if let Err(e) = start_bounce(app, cycle) {
            app.ui.master_match.error = Some(trf("Bounce failed: {}", &[&e]));
        }
    }
    poll(app, ctx);
    if app.ui.master_match.open {
        let name = app.project.name.clone();
        let selected = selected_region(&app.project, &app.ui);
        show(ctx, &name, &mut app.ui.master_match, selected);
    } else if app.ui.master_match.playing {
        app.ui.master_match.playing = false;
    }
    let m = &mut app.ui.master_match;
    if let Some(a) = app.sync.as_mut().and_then(|s| s.engine.audition()) {
        m.phase = a.status.phase();
        pump(m, a);
    }
    if m.playing {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// The audio region selected in the Tracks area, as a target.
fn selected_region(p: &Project, s: &crate::UiState) -> Option<Target> {
    let (t, r) = s.tracks.selected?;
    let region = p.tracks.get(t)?.regions.get(r)?;
    let audio = region.audio.as_ref()?;
    // The recording itself: a Flex edit's original file.
    let file = audio
        .flex
        .as_ref()
        .map_or_else(|| audio.file.path.clone(), |f| f.original.path.clone());
    Some(Target::Region {
        name: region.name.clone(),
        file,
        start_bar: region.start_bar,
        len_bars: region.len_bars,
        offset_secs: region.params.offset_secs,
    })
}

fn audio_panel() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter(
        tr("Audio File"),
        &[
            "wav", "wave", "aif", "aiff", "aifc", "flac", "mp3", "caf", "ogg", "oga", "m4a", "mp4",
        ],
    )
}

fn pick_files(app: &mut crate::App) {
    let Some(pick) = app.ui.master_match.pick.take() else {
        return;
    };
    let m = &mut app.ui.master_match;
    match pick {
        Pick::Target => {
            if let Some(path) = audio_panel().pick_file() {
                m.target = Some(Target::File(path));
            }
        }
        Pick::Reference => {
            if let Some(path) = audio_panel().pick_file() {
                m.reference = Some(path);
            }
        }
        Pick::Save => {
            let Some(done) = m.done.as_ref() else {
                return;
            };
            let mut panel = rfd::FileDialog::new()
                .set_file_name(format!("{}.wav", result_stem(&done.target_src)))
                .add_filter("WAV", &["wav"]);
            if let Some(dir) = app.path.as_ref().and_then(|f| f.parent()) {
                panel = panel.set_directory(dir);
            }
            if let Some(mut path) = panel.save_file() {
                path.set_extension("wav");
                start_save(m, path, false);
            }
        }
    }
}

/// Moves the jobs on: a finished match or finish is taken in, a written file reported (and
/// put on a track when asked).
fn poll(app: &mut crate::App, ctx: &egui::Context) {
    poll_bounce(app, ctx);
    let m = &mut app.ui.master_match;
    if let Some(job) = m.job.as_ref() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        match job.rx.try_recv() {
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                m.job = None;
                m.error = Some(tr("Match to Reference failed").to_string());
            }
            Ok(res) => {
                m.job = None;
                match res {
                    Ok(Outcome::Matched(done)) => {
                        m.done = Some(*done);
                        m.serial += 1;
                        m.error = None;
                    }
                    Ok(Outcome::Finished(f)) => {
                        if let Some(d) = m.done.as_mut() {
                            d.finished = f;
                            m.serial += 1;
                        }
                    }
                    Err(e) => {
                        // A finish that did not happen: the settings go back to the finish
                        // there is (or it would be started again at once).
                        if let Some(d) = m.done.as_ref() {
                            m.settings.take_finish(d.finished.finish);
                        }
                        if e != "cancelled" {
                            m.error = Some(e);
                        }
                    }
                }
            }
        }
    }
    // The settings were changed after a match: finish it again.
    if m.job.is_none() {
        if let Some(d) = m.done.as_ref() {
            if d.finished.finish != m.settings.finish() {
                start_finish(m);
            }
        }
    }
    let Some(Saving { path, rx, add }) = m.saving.as_ref() else {
        return;
    };
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
    let res = match rx.try_recv() {
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(tr("Match to Reference failed").to_string()),
        Ok(r) => r,
    };
    let (path, add) = (path.clone(), *add);
    m.saving = None;
    let at = m.done.as_ref().and_then(|d| match &d.target_src {
        Target::Region {
            start_bar,
            len_bars,
            offset_secs,
            ..
        } => Some((*start_bar, *len_bars, *offset_secs)),
        _ => None,
    });
    let name = m
        .done
        .as_ref()
        .map(|d| result_stem(&d.target_src))
        .unwrap_or_default();
    app.ui.status = Some(match res {
        Ok(()) if add => match add_track(&mut app.project, &path, &name, at, ctx) {
            Ok(()) => trf("Added {}", &[&name]),
            Err(e) => trf("Cannot read {}: {}", &[&file_name(&path), &e]),
        },
        Ok(()) => trf("Saved {}", &[&path.display()]),
        Err(e) => trf("Cannot write {}: {}", &[&path.display(), &e]),
    });
}

/// A Project target: its bounce runs first (the engine of File > Bounce), then the match.
fn start_bounce(app: &mut crate::App, cycle: bool) -> Result<(), String> {
    if app.ui.bounce.job.is_some() || app.ui.bounce.in_place.is_some() {
        return Err(tr("A bounce is already running").to_string());
    }
    let p = &app.project;
    let range = if cycle {
        Some((p.cycle_start, p.cycle_end))
    } else {
        crate::bounce::whole_range(p)
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = std::env::temp_dir().join(format!("prowave-match-{stamp}.wav"));
    let settings = crate::bounce::Settings {
        path: path.clone(),
        range,
        format: crate::bounce::Format::Wav,
        depth: crate::bounce::Depth::Float32,
        normalize: false,
        tail_seconds: 1.0,
        max_tail_seconds: 20.0,
        mix: true,
        stems: false,
        only: None,
        overwrite: true,
        strip: None,
    };
    let rate = app.sync.as_ref().map_or(48_000, |s| s.engine.sample_rate());
    let job = crate::bounce::Job::start(&app.project, rate, settings)?;
    app.ui.master_match.bounce = Some((job, path));
    app.ui.master_match.error = None;
    Ok(())
}

fn poll_bounce(app: &mut crate::App, ctx: &egui::Context) {
    let Some((mut job, path)) = app.ui.master_match.bounce.take() else {
        return;
    };
    let busy = app.ui.flex.busy() || app.ui.fit.busy();
    match job.poll(&mut app.project, &mut app.plugins, busy) {
        None => {
            app.ui.master_match.bounce = Some((job, path));
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        Some(Ok(report)) => {
            let m = &mut app.ui.master_match;
            let file = report.files.first().map_or(path, |f| f.path.clone());
            let (Some(target), Some(reference)) = (m.target.clone(), m.reference.clone()) else {
                return;
            };
            start_match(m, target, file, reference);
        }
        Some(Err(e)) if e == "cancelled" => {}
        Some(Err(e)) => app.ui.master_match.error = Some(trf("Bounce failed: {}", &[&e])),
    }
}

/// Tells the preview voice what to play when that has changed.
fn pump(m: &mut MatchUi, a: &mut AuditionRemote) {
    let want = (
        m.playing && m.done.is_some(),
        m.side,
        m.level_match,
        m.serial,
    );
    if m.sent == Some(want) {
        return;
    }
    let Some(done) = m.done.as_ref() else {
        if m.sent.is_some_and(|s| s.0) {
            let _ = a.send(AuditionMsg::Stop);
        }
        m.sent = Some(want);
        return;
    };
    if !want.0 {
        if m.sent.is_some_and(|s| s.0) {
            let _ = a.send(AuditionMsg::Stop);
        }
        m.sent = Some(want);
        return;
    }
    let (go, gm) = ab_gains(done, m.level_match);
    let mut gains = [0.0f32; AUDITION_LAYERS];
    match m.side {
        Side::Original => gains[0] = go,
        Side::Matched => gains[1] = gm,
    }
    // Starting, or a new result: the layers go (again); otherwise only the gains move, so the
    // switch between original and matched keeps its place.
    let restart = m.sent.is_none_or(|s| !s.0 || s.3 != m.serial);
    let msg = if restart {
        let mut layers: prowave_engine::AuditionLayers = Default::default();
        layers[0] = Some(done.original.clone());
        layers[1] = Some(done.finished.preview.clone());
        let _ = a.send(AuditionMsg::Sync(false));
        AuditionMsg::Play {
            layers,
            gains,
            beats: 4.0,
            from_start: m.sent.is_none_or(|s| !s.0),
        }
    } else {
        AuditionMsg::Gains(gains)
    };
    if a.send(msg).is_ok() {
        m.sent = Some(want);
    }
}

/// The preview gains of the original and the matched: with level matching, the louder of the
/// two is turned down to the other's integrated loudness.
fn ab_gains(d: &Done, level_match: bool) -> (f32, f32) {
    if !level_match {
        return (1.0, 1.0);
    }
    let preview_lufs = |b: &SampleBuf| {
        let l: Vec<f64> = b.data.iter().step_by(2).map(|v| f64::from(*v)).collect();
        let r: Vec<f64> = b
            .data
            .iter()
            .skip(1)
            .step_by(2)
            .map(|v| f64::from(*v))
            .collect();
        loudness::integrated_lufs(&[&l, &r], b.rate)
    };
    match (d.original_preview_lufs, preview_lufs(&d.finished.preview)) {
        (Some(o), Some(r)) => {
            let down = |by: f64| 10f64.powf(-by.max(0.0) / 20.0) as f32;
            (down(o - r), down(r - o))
        }
        _ => (1.0, 1.0),
    }
}

/// The window.
fn show(ctx: &egui::Context, project_name: &str, m: &mut MatchUi, selected: Option<Target>) {
    let id = Id::unique("prowave_match_master");
    let mut start = false;
    let (open, _) = hw_vocal::window(
        ctx,
        id,
        tr("Match to Reference"),
        project_name,
        WIDTH,
        None,
        |ui| {
            start = body(ui, id, m, selected);
        },
    );
    if !open {
        m.open = false;
        m.playing = false;
    }
    if start {
        go(m);
    }
}

/// MATCH was pressed.
fn go(m: &mut MatchUi) {
    let (Some(target), Some(reference)) = (m.target.clone(), m.reference.clone()) else {
        return;
    };
    m.playing = false;
    match &target {
        Target::File(f) => {
            let f = f.clone();
            start_match(m, target, f, reference);
        }
        Target::Region { file, .. } => {
            let f = file.clone();
            start_match(m, target, f, reference);
        }
        // The bounce is started from the frame, which has the project.
        Target::Project { cycle } => m.bounce_wanted = Some(*cycle),
    }
}

/// The panel's contents. Returns whether MATCH was pressed.
fn body(ui: &mut egui::Ui, id: Id, m: &mut MatchUi, selected: Option<Target>) -> bool {
    let busy = m.busy();
    // TARGET
    let field = hw_vocal::row(ui, tr("Target"));
    let items: Vec<&str> = [
        tr("Audio File…"),
        tr("Selected Region"),
        tr("Project"),
        tr("Cycle Range"),
    ]
    .into();
    let shown = m
        .target
        .as_ref()
        .map_or_else(|| "—".to_string(), Target::shown);
    let names = items;
    let current = m.target.as_ref().map(|t| match t {
        Target::File(_) => 0,
        Target::Region { .. } => 1,
        Target::Project { cycle } => 2 + usize::from(*cycle),
    });
    if let Some(i) = hw_vocal::dropdown(ui, id.with("target"), field, &shown, &names, current) {
        if !busy {
            match i {
                0 => m.pick = Some(Pick::Target),
                1 => {
                    if let Some(t) = selected.clone() {
                        m.target = Some(t);
                    } else {
                        m.error = Some(tr("Select an audio region first").to_string());
                    }
                }
                i => m.target = Some(Target::Project { cycle: i == 3 }),
            }
        }
    }
    // REFERENCE
    let field = hw_vocal::row(ui, tr("Reference"));
    let shown = m
        .reference
        .as_deref()
        .map_or_else(|| "—".to_string(), file_name);
    let g = ui.painter().clone();
    let shown = hw::fit(
        &g,
        &hw_vocal::printable(&g, &shown, Face::Bold, 12.0),
        Face::Bold,
        12.0,
        field.width() - 30.0,
    );
    if hw::field(ui, &g, field, id.with("reference"), &shown, false).clicked() && !busy {
        m.pick = Some(Pick::Reference);
    }
    hw_vocal::gap(ui, 6.0);
    lcd_panel(ui, m);
    hw_vocal::gap(ui, 6.0);
    // The finish.
    let on_off = [tr("Off"), tr("On")];
    let field = hw_vocal::row(ui, tr("Limiter"));
    let s = &mut m.settings;
    if let Some(i) = hw_vocal::dropdown(
        ui,
        id.with("limiter"),
        field,
        on_off[usize::from(s.limiter)],
        &on_off,
        Some(usize::from(s.limiter)),
    ) {
        s.limiter = i == 1;
    }
    if !s.limiter {
        let field = hw_vocal::row(ui, tr("Normalize"));
        if let Some(i) = hw_vocal::dropdown(
            ui,
            id.with("normalize"),
            field,
            on_off[usize::from(s.normalize)],
            &on_off,
            Some(usize::from(s.normalize)),
        ) {
            s.normalize = i == 1;
        }
    }
    if s.limiter || s.normalize {
        let names: Vec<String> = CEILINGS
            .iter()
            .map(|c| match c {
                None => "Matchering".to_string(),
                Some(db) => format!("{db:.1} dBTP"),
            })
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let field = hw_vocal::row(ui, tr("Ceiling"));
        let at = s.ceiling.min(CEILINGS.len() - 1);
        if let Some(i) =
            hw_vocal::dropdown(ui, id.with("ceiling"), field, refs[at], &refs, Some(at))
        {
            s.ceiling = i;
        }
    }
    let depth_names = ["16-bit", "24-bit", "32-bit float"];
    let field = hw_vocal::row(ui, tr("Bit Depth"));
    let at = s.depth.min(DEPTHS.len() - 1);
    if let Some(i) = hw_vocal::dropdown(
        ui,
        id.with("depth"),
        field,
        depth_names[at],
        &depth_names,
        Some(at),
    ) {
        s.depth = i;
    }
    hw_vocal::gap(ui, 10.0);
    // MATCH / CANCEL
    let ready = m.target.is_some() && m.reference.is_some();
    let mut start = false;
    if busy {
        if hw_vocal::key_row(
            ui,
            id.with("cancel"),
            &[(tr("Cancel"), Cap::Bone, None, true)],
        )[0]
        {
            if let Some(j) = m.job.as_ref() {
                j.cancel.store(true, Ordering::Relaxed);
            }
            if let Some((b, _)) = m.bounce.as_ref() {
                b.cancel();
            }
        }
    } else {
        start = hw_vocal::key_row(
            ui,
            id.with("match"),
            &[(tr("Match"), Cap::Orange, None, ready)],
        )[0];
    }
    hw_vocal::gap(ui, 8.0);
    // A/B
    let have = m.done.is_some();
    let clicked = hw_vocal::key_row(
        ui,
        id.with("ab"),
        &[
            (
                tr("Original"),
                Cap::Bone,
                Some(have && m.side == Side::Original),
                have,
            ),
            (
                tr("Matched Master"),
                Cap::Bone,
                Some(have && m.side == Side::Matched),
                have,
            ),
            (
                if m.playing { tr("Stop") } else { tr("Play") },
                Cap::Dark,
                Some(m.playing),
                have,
            ),
            (tr("Level Match"), Cap::Bone, Some(m.level_match), have),
        ],
    );
    if clicked[0] {
        m.side = Side::Original;
    }
    if clicked[1] {
        m.side = Side::Matched;
    }
    if clicked[2] {
        m.playing = !m.playing;
    }
    if clicked[3] {
        m.level_match = !m.level_match;
    }
    hw_vocal::gap(ui, 8.0);
    let can_write = have && m.job.is_none() && m.saving.is_none();
    let clicked = hw_vocal::key_row(
        ui,
        id.with("write"),
        &[
            (tr("Save As…"), Cap::Bone, None, can_write),
            (tr("Add to Project"), Cap::Bone, None, can_write),
        ],
    );
    if clicked[0] {
        m.pick = Some(Pick::Save);
    }
    if clicked[1] {
        if let Some(d) = m.done.as_ref() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let dir = crate::project_io::recordings_dir();
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(format!("{}-{stamp}.wav", result_stem(&d.target_src)));
            start_save(m, path, true);
        }
    }
    start
}

/// The LCD: the matching EQ (mid and side) over 20 Hz to 20 kHz, the loudness readings, and
/// the progress of a running job or the place of the preview.
fn lcd_panel(ui: &mut egui::Ui, m: &MatchUi) {
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 176.0), Sense::hover());
    let glass = hw::lcd(ui.painter(), outer).shrink2(vec2(8.0, 6.0));
    let g = ui.painter().clone();
    let readouts_w = 150.0;
    let plot = Rect::from_min_max(
        pos2(glass.left() + 26.0, glass.top() + 14.0),
        pos2(glass.right() - readouts_w - 12.0, glass.bottom() - 30.0),
    );
    eq_plot(&g, plot, m.done.as_ref().map(|d| &*d.matched));
    hw_vocal::lcd_divider(
        &g,
        glass.right() - readouts_w - 4.0,
        glass.top()..=glass.bottom() - 22.0,
    );
    // Readouts.
    let x = glass.right() - readouts_w + 2.0;
    let rows = [
        (tr("Target"), m.done.as_ref().and_then(|d| d.target_lufs)),
        (
            tr("Reference"),
            m.done.as_ref().and_then(|d| d.reference_lufs),
        ),
        (tr("Result"), m.done.as_ref().and_then(|d| d.finished.lufs)),
    ];
    let row_h = (plot.height() + 14.0) / 4.0;
    for (k, (label, v)) in rows.iter().enumerate() {
        let top = glass.top() + k as f32 * row_h;
        hw_vocal::lcd_label(&g, pos2(x, top + 2.0), &format!("{label} LUFS"));
        let value = v.map_or_else(|| "--.-".to_string(), |v| format!("{v:.1}"));
        let h = 15.0;
        let w = hw::seg_width(&value, h);
        hw::seg(
            &g,
            pos2(glass.right() - 6.0 - w, top + row_h - h - 3.0),
            &value,
            h,
            theme::LCD_INK(),
            true,
        );
    }
    let top = glass.top() + 3.0 * row_h;
    hw_vocal::lcd_label(&g, pos2(x, top + 2.0), "dBTP");
    let value = m.done.as_ref().map_or_else(
        || "--.-".to_string(),
        |d| format!("{:.1}", d.finished.true_peak_db),
    );
    let h = 15.0;
    let w = hw::seg_width(&value, h);
    hw::seg(
        &g,
        pos2(glass.right() - 6.0 - w, top + row_h - h - 3.0),
        &value,
        h,
        theme::LCD_INK(),
        true,
    );
    // The bottom line.
    let line = Rect::from_min_max(
        pos2(glass.left() + 4.0, glass.bottom() - 20.0),
        pos2(glass.right() - 4.0, glass.bottom() - 2.0),
    );
    let text_r = Rect::from_min_max(line.min, pos2(line.left() + 190.0, line.max.y));
    let bar = Rect::from_min_max(
        pos2(line.left() + 196.0, line.center().y - 3.5),
        pos2(line.right(), line.center().y + 3.5),
    );
    if let Some((b, _)) = m.bounce.as_ref() {
        let pr = b.progress();
        hw_vocal::lcd_text(
            &g,
            text_r,
            &format!("{} {}%", tr("Bouncing"), (pr.fraction * 100.0).floor()),
            11.0,
            1,
            theme::LCD_INK(),
        );
        hw_vocal::lcd_ladder(&g, bar, pr.fraction);
        return;
    }
    if let Some(j) = m.job.as_ref() {
        let what = match j.stage() {
            Stage::Reading => tr("Reading"),
            Stage::Matching => tr("Matching"),
            Stage::Finishing => tr("Finishing"),
        };
        let f = j.fraction();
        hw_vocal::lcd_text(
            &g,
            text_r,
            &format!("{what} {}%", (f * 100.0).floor()),
            11.0,
            1,
            theme::LCD_INK(),
        );
        hw_vocal::lcd_ladder(&g, bar, f);
        return;
    }
    if let Some(e) = m.error.as_ref() {
        hw_vocal::lcd_text(&g, line, e, 11.0, 1, theme::LCD_INK());
        return;
    }
    if m.done.is_some() {
        let side = match m.side {
            Side::Original => tr("Original"),
            Side::Matched => tr("Matched Master"),
        };
        hw_vocal::lcd_text(&g, text_r, side, 11.0, 1, theme::LCD_INK());
        hw_vocal::lcd_ladder(&g, bar, m.phase.filter(|_| m.playing).unwrap_or(0.0));
    }
}

/// x of `hz` in `r` (log, 20 Hz to 20 kHz).
fn x_of(r: Rect, hz: f64) -> f32 {
    let t = ((hz.max(20.0) / 20.0).log10() / 3.0) as f32;
    r.left() + t.clamp(0.0, 1.0) * r.width()
}

const RANGE_DB: f32 = 12.0;

/// The matching EQ: what the FIRs do to the mid (solid) and the side (faint), in dB.
fn eq_plot(g: &egui::Painter, r: Rect, m: Option<&Matched>) {
    let grid = Stroke::new(1.0, theme::LCD_INK().gamma_multiply(0.18));
    for hz in [100.0, 1000.0, 10_000.0] {
        let x = x_of(r, hz);
        g.vline(x, r.y_range(), grid);
    }
    let y_of = |db: f32| r.center().y - (db / RANGE_DB).clamp(-1.0, 1.0) * r.height() / 2.0;
    for db in [-RANGE_DB, -6.0, 0.0, 6.0, RANGE_DB] {
        let s = if db == 0.0 {
            Stroke::new(1.0, theme::LCD_INK().gamma_multiply(0.4))
        } else {
            grid
        };
        g.hline(r.x_range(), y_of(db), s);
    }
    for (db, label) in [(RANGE_DB, "+12"), (0.0, "0"), (-RANGE_DB, "-12")] {
        hw::text(
            g,
            pos2(r.left() - 4.0, y_of(db)),
            Align2::RIGHT_CENTER,
            label,
            Face::Lcd,
            8.0,
            theme::LCD_INK().gamma_multiply(0.7),
        );
    }
    for (hz, label) in [(100.0, "100"), (1000.0, "1K"), (10_000.0, "10K")] {
        hw::text(
            g,
            pos2(x_of(r, hz), r.bottom() + 2.0),
            Align2::CENTER_TOP,
            label,
            Face::Lcd,
            8.0,
            theme::LCD_INK().gamma_multiply(0.7),
        );
    }
    // The legend: a solid line for the mid, a dashed one for the side.
    let ink = theme::LCD_INK();
    let mid = hw_vocal::lcd_label(g, pos2(r.left() + 3.0, r.top() - 12.0), tr("Mid Channel"));
    let y = mid.center().y;
    g.line_segment(
        [pos2(mid.right() + 5.0, y), pos2(mid.right() + 21.0, y)],
        Stroke::new(1.8, ink),
    );
    let side = hw_vocal::lcd_label(
        g,
        pos2(mid.right() + 30.0, r.top() - 12.0),
        tr("Side Channel"),
    );
    for k in 0..3 {
        let x = side.right() + 5.0 + k as f32 * 6.0;
        g.line_segment(
            [pos2(x, y), pos2(x + 3.5, y)],
            Stroke::new(1.4, ink.gamma_multiply(0.45)),
        );
    }
    let Some(m) = m else {
        return;
    };
    let rate = f64::from(Config::default().internal_sample_rate);
    for (eq, color, dashed) in [
        (&m.side, theme::LCD_INK().gamma_multiply(0.45), true),
        (&m.mid, theme::LCD_INK(), false),
    ] {
        let n = eq.smoothed.len();
        if n < 3 {
            continue;
        }
        let bin_hz = rate / 2.0 / (n - 1) as f64;
        let pts: Vec<egui::Pos2> = (1..n)
            .map(|k| (k as f64 * bin_hz, eq.smoothed[k]))
            .filter(|(hz, _)| *hz >= 20.0 && *hz <= 20_000.0)
            .map(|(hz, v)| pos2(x_of(r, hz), y_of(20.0 * v.max(1e-6).log10() as f32)))
            .collect();
        if dashed {
            dashes(g, &pts, Stroke::new(1.4, color), 5.0, 4.0);
        } else {
            g.add(egui::Shape::line(pts, Stroke::new(1.8, color)));
        }
    }
}

/// `pts` as a dashed line: `on` points drawn, `off` points left, measured along the line.
fn dashes(g: &egui::Painter, pts: &[egui::Pos2], stroke: Stroke, on: f32, off: f32) {
    let period = on + off;
    let mut at = 0.0f32;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = a.distance(b);
        if len <= 0.0 {
            continue;
        }
        let mut t = 0.0f32;
        while t < len {
            let phase = (at + t) % period;
            let step = if phase < on {
                on - phase
            } else {
                period - phase
            }
            .min(len - t);
            if phase < on {
                let p0 = a + (b - a) * (t / len);
                let p1 = a + (b - a) * ((t + step) / len);
                g.line_segment([p0, p1], stroke);
            }
            t += step.max(1e-3);
        }
        at += len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stereo test song: noise shaped by a one-pole filter (`bright` > 0: brighter), with a
    /// loud/quiet shape, at `gain`.
    fn song(seed: u64, seconds: f64, bright: f64, width: f64, gain: f64) -> Stereo {
        let n = (seconds * 44_100.0) as usize;
        let mut x = seed;
        let mut noise = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        };
        let (mut lp, mut prev) = (0.0, 0.0);
        let mut l = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for i in 0..n {
            let w = noise();
            lp += 0.08 * (w - lp);
            let m = lp * 3.0 + bright * (w - prev);
            prev = w;
            let s = noise() * 0.3 * width;
            let shape = if (i / 66_150) % 2 == 0 { 0.5 } else { 1.0 };
            l.push((m + s) * gain * shape);
            r.push((m - s) * gain * shape);
        }
        Stereo { l, r }
    }

    /// A match of two songs written as files, run as the window runs it.
    fn matched(dir: &Path) -> MatchUi {
        let t = dir.join("Song demo.wav");
        let r = dir.join("reference.wav");
        rm::write_wav(&t, &song(1, 8.0, -0.2, 0.3, 0.06), 44_100, Depth::Float32).unwrap();
        rm::write_wav(&r, &song(2, 8.0, 0.8, 0.8, 0.2), 44_100, Depth::Float32).unwrap();
        let mut m = MatchUi::new();
        m.target = Some(Target::File(t.clone()));
        m.reference = Some(r.clone());
        start_match(&mut m, Target::File(t.clone()), t, r);
        let job = m.job.take().unwrap();
        match job.rx.recv().unwrap() {
            Ok(Outcome::Matched(d)) => m.done = Some(*d),
            Ok(Outcome::Finished(_)) => panic!("a whole match was asked for"),
            Err(e) => panic!("{e}"),
        }
        m.serial += 1;
        m
    }

    #[test]
    fn a_match_runs_on_its_thread_and_lands_on_the_references_loudness() {
        let dir = tempfile::tempdir().unwrap();
        let m = matched(dir.path());
        let d = m.done.as_ref().unwrap();
        let (r, out) = (d.reference_lufs.unwrap(), d.finished.lufs.unwrap());
        println!("reference {r:.2} LUFS, result {out:.2}");
        assert!((r - out).abs() < 0.5, "{out} vs {r} LUFS");
        assert!(d.finished.true_peak_db < 0.5);
        // The preview is the whole of a song shorter than 30 s, both sides the same length.
        assert_eq!(d.original.frames(), d.finished.preview.frames());
        assert_eq!(d.original.rate, 44_100);
    }

    #[test]
    fn changed_settings_finish_again_and_a_failed_finish_puts_them_back() {
        let mut s = Settings::default();
        for (limiter, normalize, ceiling) in [
            (true, true, 0),
            (true, true, 4),
            (false, false, 2),
            (false, true, 1),
        ] {
            s.limiter = limiter;
            s.normalize = normalize;
            s.ceiling = ceiling;
            let mut back = Settings::default();
            back.take_finish(s.finish());
            assert_eq!(
                (back.limiter, back.normalize, back.ceiling),
                (limiter, normalize, ceiling)
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let mut m = matched(dir.path());
        m.settings.ceiling = 4;
        start_finish(&mut m);
        let job = m.job.take().unwrap();
        let Ok(Outcome::Finished(f)) = job.rx.recv().unwrap() else {
            panic!("no finish");
        };
        assert_eq!(f.finish.ceiling, Ceiling::TruePeak(-1.0));
        assert!(f.true_peak_db <= -1.0 + 1e-9, "{}", f.true_peak_db);
    }

    #[test]
    fn level_matching_turns_the_louder_preview_down() {
        let dir = tempfile::tempdir().unwrap();
        let m = matched(dir.path());
        let d = m.done.as_ref().unwrap();
        let (o, r) = ab_gains(d, true);
        // The matched song is the louder: it is the one turned down, by the difference.
        assert_eq!(o, 1.0);
        assert!(r < 1.0);
        let lufs_of = |b: &SampleBuf, g: f32| {
            let l: Vec<f64> = b
                .data
                .iter()
                .step_by(2)
                .map(|v| f64::from(*v * g))
                .collect();
            let rr: Vec<f64> = b
                .data
                .iter()
                .skip(1)
                .step_by(2)
                .map(|v| f64::from(*v * g))
                .collect();
            loudness::integrated_lufs(&[&l, &rr], b.rate).unwrap()
        };
        let diff = lufs_of(&d.original, o) - lufs_of(&d.finished.preview, r);
        assert!(diff.abs() < 0.05, "{diff}");
        assert_eq!(ab_gains(d, false), (1.0, 1.0));
    }

    #[test]
    fn the_result_is_written_and_put_on_a_track_where_the_region_was() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = matched(dir.path());
        let path = dir.path().join("out.wav");
        start_save(&mut m, path.clone(), true);
        let saving = m.saving.take().unwrap();
        saving.rx.recv().unwrap().unwrap();
        let f = AudioFile::open(&path).unwrap();
        assert_eq!((f.sample_rate, f.channels), (44_100, 2));
        let mut p = Project::demo();
        let before = p.tracks.len();
        let ctx = egui::Context::default();
        add_track(&mut p, &path, "Song Matched", Some((5.0, 4.0, 1.5)), &ctx).unwrap();
        assert_eq!(p.tracks.len(), before + 1);
        let r = &p.tracks[before].regions[0];
        assert_eq!(
            (r.start_bar, r.len_bars, r.params.offset_secs),
            (5.0, 4.0, 1.5)
        );
        assert_eq!(r.name, "Song Matched");
        assert_eq!(
            result_stem(&Target::File(PathBuf::from("/a/Mix 3.wav"))),
            "Mix 3 Matched Master"
        );
    }

    /// Pictures of the window: before a match, while one runs, and after:
    /// `MANUAL_PNG_DIR=… cargo test -p prowave-app match_window_pictures -- --ignored`.
    #[test]
    #[ignore = "pictures of the window"]
    fn match_window_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let shot = |name: &str, m: &mut MatchUi| {
            crate::ui_test::render_png(
                WIDTH + 40.0,
                640.0,
                2.0,
                theme::BONE3(),
                |u| {
                    theme::apply(u.ctx());
                    show(u.ctx(), "Untitled", m, None);
                },
                &hw_vocal::picture_path(name),
            )
            .unwrap();
        };
        let mut empty = MatchUi::new();
        shot("match-empty.png", &mut empty);
        let mut m = matched(dir.path());
        shot("match-done.png", &mut m);
        m.settings.limiter = false;
        m.side = Side::Original;
        m.playing = true;
        m.phase = Some(0.4);
        shot("match-no-limiter.png", &mut m);
        let mut running = MatchUi::new();
        running.target = m.target.clone();
        running.reference = m.reference.clone();
        let (_tx, rx) = channel();
        running.job = Some(Job {
            progress: Arc::new(AtomicU32::new(0.42f32.to_bits())),
            stage: Arc::new(AtomicU8::new(Stage::Matching as u8)),
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
        });
        shot("match-running.png", &mut running);
        running.job = None;
        running.error = Some(error_words(&MatchError::ReferenceTooShort));
        shot("match-error.png", &mut running);
    }
}
