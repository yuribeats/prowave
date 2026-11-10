//! Fit to Grid (Logic: Smart Tempo's "Adapt Project Tempo" for the first audio of a project
//! and "Keep Project Tempo" with Flex Time for the rest): an audio file dropped into the
//! project lands tempo-matched and on the grid.
//!
//! - The first audio file of a project sets the project's tempo to its own (rounded to a whole
//!   number when it is one), its first downbeat lands on a bar line, and Flex markers on its
//!   beats pull any drift inside it onto the grid.
//! - A later file keeps the project's tempo: Flex Time stretches it to the project's tempo
//!   (Slicing, Rhythmic or Polyphonic, chosen by how the sound behaves), its first downbeat
//!   lands on the nearest bar line from the playhead, and its hits near a 1/16 line are moved
//!   onto it.
//! - A one-shot (no steady tempo, or shorter than a beat) is placed on the grid at the
//!   playhead, not stretched; an unsure tempo is never given to the project.
//! - Apple Loops carry their tempo in the file: it is trusted over the analysis.
//!
//! The file is read on a thread; the region plays its recording until the Flex sound is made
//! (as every Flex edit), and the recording itself is never changed. File > Import > Fit to
//! Grid on Drop turns it off; Option held while dropping drops the file as it is. The region
//! menu's Flex Time > Fit to Grid does it again to a region.
use crate::flex_edit::{markers_from, recording_of};
use crate::i18n::{tr, trf};
use crate::model::Project;
use crate::UiState;
use prowave_engine::sample_tools::transients;
use prowave_engine::{AudioFile, FlexMode, SampleBuf, TempoMap};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

/// How sure the analysis has to be before its tempo is used.
pub const MIN_CONFIDENCE: f64 = 0.3;
/// The tempo range a first file's tempo is folded into (twice or half what was heard).
const ADOPT_RANGE: (f64, f64) = (70.0, 180.0);
/// A tempo this near a whole number is that number.
const WHOLE_BPM: f64 = 0.15;
/// Files up to this long are read whole, to place every hit to the sample.
const MAX_PRECISE_SECS: f64 = 180.0;
/// The shortest file with a tempo, in seconds (about four beats at 120).
const MIN_LOOP_SECS: f64 = 2.0;
/// Flex markers go this long before the hit they move, in seconds.
const PRE_ATTACK: f64 = 0.002;
/// A hit this near (in beats) to a 1/16 line is moved onto it.
const SNAP_REACH: f64 = 0.06;

/// What the analysis of a file found, ready to place it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The tempo found (already doubled or halved for the project), unrounded.
    pub tempo: Option<f64>,
    pub confidence: f64,
    /// The tempo comes from the file's own tags (an Apple Loop).
    pub tagged: bool,
    /// The beats, seconds into the file, at `tempo`.
    pub beats: Vec<f64>,
    /// Which of `beats` is the first downbeat (those before it are a pickup).
    pub downbeat: usize,
    /// Where the hits start, seconds into the file (to the sample when the file is short).
    pub onsets: Vec<f64>,
    /// The Flex mode that suits the sound.
    pub mode: FlexMode,
    pub duration: f64,
}

/// What the analysis needs to know of the project.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// The file gives the project its tempo (it is the project's first audio).
    pub adopt: bool,
    /// The project's tempo where the file goes.
    pub project_bpm: f64,
    /// Beats in a bar there.
    pub beats_per_bar: usize,
}

struct Job {
    track: usize,
    region: usize,
    /// The recording it was started for, to check it is still the same region.
    path: PathBuf,
    /// Where it goes: the downbeat goes to the bar line nearest this (quarter notes).
    origin: f64,
    /// The grid a one-shot snaps to, in quarter notes.
    grid: f64,
    adopt: bool,
    result: Receiver<Result<Found, String>>,
}

/// Fit to Grid in the background, and whether it happens on a drop.
pub struct FitState {
    jobs: Vec<Job>,
    /// File > Import > Fit to Grid on Drop.
    pub on_drop: bool,
}

impl Default for FitState {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            on_drop: true,
        }
    }
}

impl FitState {
    /// Whether a file is still being read (the edit it makes is not finished yet).
    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }
}

/// The saved Fit to Grid on Drop setting (on unless turned off).
pub fn on_drop_setting() -> bool {
    off_file().is_none_or(|f| !f.exists())
}

/// Saves File > Import > Fit to Grid on Drop.
pub fn save_on_drop(on: bool) {
    // The tests of the menus must not change the settings of whoever runs them.
    if cfg!(test) {
        return;
    }
    let Some(file) = off_file() else { return };
    if on {
        let _ = std::fs::remove_file(file);
    } else if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(file, b""));
    }
}

fn off_file() -> Option<PathBuf> {
    crate::recent_files::config_dir().map(|d| d.join("fit-to-grid-off"))
}

/// Whether Option (Alt) is held now: a file dropped then goes in as it is. Asked of the system
/// on macOS, since a drag from the Finder leaves the window without the key's events.
pub fn raw_drop_held(ctx: &egui::Context) -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventSourceFlagsState(state_id: i32) -> u64;
        }
        // kCGEventSourceStateCombinedSessionState, kCGEventFlagMaskAlternate.
        const COMBINED_SESSION: i32 = 0;
        const ALTERNATE: u64 = 0x0008_0000;
        // SAFETY: a plain query of the keyboard flags; no pointers.
        if unsafe { CGEventSourceFlagsState(COMBINED_SESSION) } & ALTERNATE != 0 {
            return true;
        }
    }
    ctx.input(|i| i.modifiers.alt)
}

/// Whether the project has any audio region (not counting region `skip`).
fn has_audio_besides(p: &Project, skip: Option<(usize, usize)>) -> bool {
    p.tracks.iter().enumerate().any(|(t, track)| {
        track
            .regions
            .iter()
            .enumerate()
            .any(|(r, region)| region.audio.is_some() && Some((t, r)) != skip)
    })
}

/// Whether a file dropped now would be the project's first audio.
pub fn would_be_first(p: &Project) -> bool {
    !has_audio_besides(p, None)
}

/// The grid a one-shot snaps to: the Tracks area's snap where it goes (a 1/16 at the least).
pub fn grid_of(p: &Project, s: &UiState, at: f64) -> f64 {
    let bpb = p.bar_len(0);
    let beat_px = s.tracks.bar_w / bpb as f32;
    let step = crate::tracks_area::snap_step(
        s.tracks.snap,
        beat_px,
        p.bar_len_at(at),
        p.sigs().beat_unit_at(at),
    );
    if step.is_finite() && step > 0.0 {
        step.max(0.25)
    } else {
        0.25
    }
}

/// Imports an audio file dropped on the window (or added from the browser) as a new track at
/// the playhead, and fits it to the grid unless Fit to Grid on Drop is off or `raw` (Option
/// held).
pub fn import_dropped(
    p: &mut Project,
    s: &mut UiState,
    path: &std::path::Path,
    ctx: &egui::Context,
    raw: bool,
) -> Result<(), prowave_engine::AudioError> {
    let first = would_be_first(p);
    crate::import::import_audio(p, path, ctx)?;
    if s.fit.on_drop && !raw {
        let t = p.tracks.len() - 1;
        start_drop(p, s, t, 0, first, p.position_beats);
    }
    Ok(())
}

/// Starts fitting region `r` of track `t`, just dropped: `first` when it was the project's
/// first audio, `origin` where it was dropped (quarter notes).
pub fn start_drop(p: &Project, s: &mut UiState, t: usize, r: usize, first: bool, origin: f64) {
    let grid = grid_of(p, s, origin);
    start_with(p, s, t, r, first, origin, grid);
}

/// Region menu > Flex Time > Fit to Grid: fits a region where it is now. It sets the project's
/// tempo when it is the project's only audio region.
pub fn start_region(p: &Project, s: &mut UiState, t: usize, r: usize) {
    let Some(region) = p.tracks.get(t).and_then(|x| x.regions.get(r)) else {
        return;
    };
    let origin = p.region_start_beats(region);
    let first = !has_audio_besides(p, Some((t, r)));
    let grid = grid_of(p, s, origin);
    start_with(p, s, t, r, first, origin, grid);
}

fn start_with(
    p: &Project,
    s: &mut UiState,
    t: usize,
    r: usize,
    first: bool,
    origin: f64,
    grid: f64,
) {
    let Some(a) = p
        .tracks
        .get(t)
        .and_then(|x| x.regions.get(r))
        .and_then(|x| x.audio.as_ref())
    else {
        s.status = Some(tr("Fit to Grid: this region has no recorded audio").into());
        return;
    };
    if s.fit.jobs.iter().any(|j| (j.track, j.region) == (t, r)) {
        return;
    }
    let file = recording_of(a);
    let path = file.path.clone();
    // A project with tempo changes keeps them: the file follows it instead.
    let adopt = first && p.tempo_changes.is_empty();
    let map = p.tempo_map();
    let cx = Context {
        adopt,
        project_bpm: f64::from(map.tempo_at(origin)),
        beats_per_bar: usize::from(map.signature_at(origin).0.max(1)),
    };
    let (tx, result) = channel();
    let spawned = std::thread::Builder::new()
        .name("prowave-fit-to-grid".into())
        .spawn(move || {
            let _ = tx.send(analyse(&file, &cx));
        });
    match spawned {
        Ok(_) => {
            s.fit.jobs.push(Job {
                track: t,
                region: r,
                path,
                origin,
                grid,
                adopt,
                result,
            });
            s.status = Some(tr("Fit to Grid: analysing…").into());
        }
        Err(e) => {
            s.status = Some(trf(
                "Fit to Grid: cannot read {}: {}",
                &[&path.display(), &e],
            ))
        }
    }
}

/// Applies the analyses that have finished. Call once per frame, before the Flex poll.
pub fn poll(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    if s.fit.jobs.is_empty() {
        return;
    }
    let mut finished = Vec::new();
    s.fit.jobs.retain(|job| match job.result.try_recv() {
        Err(TryRecvError::Empty) => true,
        Err(TryRecvError::Disconnected) => false,
        Ok(result) => {
            finished.push((
                job.track,
                job.region,
                job.path.clone(),
                job.origin,
                job.grid,
                job.adopt,
                result,
            ));
            false
        }
    });
    for (t, r, path, origin, grid, adopt, result) in finished {
        // The project may have changed while the file was being read.
        let same = p
            .tracks
            .get(t)
            .and_then(|x| x.regions.get(r))
            .and_then(|x| x.audio.as_ref())
            .is_some_and(|a| recording_of(a).path == path);
        if !same {
            continue;
        }
        s.status = Some(match result {
            Err(e) => trf("Fit to Grid: cannot read {}: {}", &[&path.display(), &e]),
            Ok(found) => apply(p, t, r, &found, origin, grid, adopt),
        });
    }
    if s.fit.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

// ------------------------------------------------------------------ analysis

/// Reads a file for Fit to Grid: its tempo (from its tags, or heard), beats, first downbeat,
/// hits and the Flex mode that suits it. Run off the UI thread.
pub fn analyse(file: &AudioFile, cx: &Context) -> Result<Found, String> {
    analyse_with(file, cx, true)
}

/// [`analyse`], with the file's tempo tags read or not (`tags`).
pub fn analyse_with(file: &AudioFile, cx: &Context, tags: bool) -> Result<Found, String> {
    let duration = file.duration_secs();
    let sr = f64::from(file.sample_rate.max(1));
    let heard = file.analyze().map_err(|e| e.to_string())?;
    let mono: Option<Vec<f32>> = if duration <= MAX_PRECISE_SECS {
        let stereo = file.read_stereo().map_err(|e| e.to_string())?;
        Some(stereo.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect())
    } else {
        None
    };
    let onsets: Vec<f64> = match &mono {
        Some(m) => {
            let buf = SampleBuf::new(
                file.sample_rate,
                m.iter().flat_map(|x| [*x, *x]).collect::<Vec<f32>>(),
            );
            transients(&buf, 0, m.len(), 0.6)
                .into_iter()
                .map(|f| f as f64 / sr)
                .collect()
        }
        None => heard.onsets.clone(),
    };
    let mode = mono
        .as_deref()
        .map_or(FlexMode::Polyphonic, |m| mode_for(m, sr));
    let mut found = Found {
        tempo: None,
        confidence: 0.0,
        tagged: false,
        beats: Vec::new(),
        downbeat: 0,
        onsets,
        mode,
        duration,
    };
    // An Apple Loop's own tempo: its beats are even from its start, which is a downbeat.
    let tagged = tags
        .then(|| crate::apple_loops::read_file(&file.path))
        .flatten()
        .and_then(|t| t.tempo);
    if let Some(bpm) = tagged.map(f64::from) {
        found.tagged = true;
        found.confidence = 1.0;
        let gap = 60.0 / bpm;
        found.beats = (0..)
            .map(|k| f64::from(k) * gap)
            .take_while(|b| *b < duration - 0.01)
            .collect();
        found.tempo = Some(bpm);
        fold(&mut found, cx, mono.as_deref(), sr, &heard);
        return Ok(found);
    }
    if duration < MIN_LOOP_SECS {
        return Ok(found);
    }
    let Some((bpm, confidence)) = heard_tempo(&heard.strength, duration) else {
        return Ok(found);
    };
    found.tempo = Some(bpm);
    found.beats = snap_beats(
        &prowave_engine::beats_at(&heard.strength, bpm),
        &found.onsets,
        0.03,
    );
    // The beat tracking may run on into silence at either end: only beats with a hit count
    // there.
    let heard_at = |b: &f64| found.onsets.iter().any(|o| (o - b).abs() <= 0.03);
    if !found.onsets.is_empty() {
        while found.beats.last().is_some_and(|b| !heard_at(b)) {
            found.beats.pop();
        }
        let lead = found.beats.iter().take_while(|b| !heard_at(b)).count();
        found.beats.drain(..lead);
    }
    // The beats placed to the sample give a finer tempo, when they agree with it (a loop's
    // length, when that gave it, is finer still).
    let by_length = duration <= TEMPO_SECS
        && ((duration * bpm / 60.0) - (duration * bpm / 60.0).round()).abs() < 1e-6;
    if let Some(fine) = steady_bpm(&found.beats).filter(|b| (b - bpm).abs() <= 0.003 * bpm) {
        if !by_length {
            found.tempo = Some(fine);
        }
    }
    let accent = |b: f64| accent_at(mono.as_deref(), sr, &heard, b);
    // Sure when the beats fall on the strongest onsets and the hits fall on the beats' grid
    // (too few hits to tell say little either way).
    let share = if found.onsets.len() >= 8 {
        grid_share(&found.beats, &found.onsets, &accent)
    } else {
        0.75
    };
    found.confidence = confidence * share;
    fold(&mut found, cx, mono.as_deref(), sr, &heard);
    Ok(found)
}

/// The most of a file whose onset strength is searched for its tempo, in seconds.
const TEMPO_SECS: f64 = 60.0;

/// The tempo heard in an onset strength ([`prowave_engine::STRENGTH_RATE`] values a second)
/// and how sure it is (0 to 1): the tempo whose beats, at their best phase, fall on the
/// strongest onsets (a comb over 60–200 BPM, then refined to a hundredth of a BPM), with a
/// liking for usual speeds; a loop cut to whole bars gets the tempo of its length. How sure
/// it is: how far above the average the beats' onsets are, against the strongest onsets there
/// are.
pub fn heard_tempo(strength: &[f32], duration: f64) -> Option<(f64, f64)> {
    let rate = prowave_engine::STRENGTH_RATE;
    let s = &strength[..strength.len().min((TEMPO_SECS * rate) as usize)];
    let n = s.len();
    if (n as f64) < MIN_LOOP_SECS * rate {
        return None;
    }
    // Each value the largest within `w` blocks of it, so a beat a little early or late counts.
    let dilate = |w: usize| -> Vec<f32> {
        (0..n)
            .map(|i| {
                s[i.saturating_sub(w)..(i + w + 1).min(n)]
                    .iter()
                    .copied()
                    .fold(0.0f32, f32::max)
            })
            .collect()
    };
    let (wide, narrow) = (dilate(4), dilate(1));
    let comb = |v: &[f32], bpm: f64| -> f64 {
        let p = 60.0 * rate / bpm;
        let mut best = 0.0f64;
        for phase in 0..p.ceil() as usize {
            let (mut sum, mut k) = (0.0f64, 0usize);
            loop {
                let i = (phase as f64 + k as f64 * p).round() as usize;
                if i >= n {
                    break;
                }
                sum += f64::from(v[i]);
                k += 1;
            }
            if k >= 3 {
                best = best.max(sum / k as f64);
            }
        }
        best
    };
    // A mild liking for the usual speeds (a broad bump around 115 on a log scale).
    let pref = |bpm: f64| (-0.5 * (bpm / 115.0).ln().powi(2)).exp();
    // A loop cut to 1, 2, 4, 8... bars of four beats (or half a bar).
    let lengths: Vec<f64> = if duration <= TEMPO_SECS {
        [2u32, 4, 8, 16, 32, 64, 128]
            .iter()
            .map(|b| 60.0 * f64::from(*b) / duration)
            .filter(|b| (60.0..=200.0).contains(b))
            .collect()
    } else {
        Vec::new()
    };
    let coarse: Vec<(f64, f64)> = (0..=280)
        .map(|k| 60.0 + f64::from(k) * 0.5)
        .map(|b| (b, comb(&wide, b)))
        .collect();
    let mut picks: Vec<f64> = (1..coarse.len() - 1)
        .filter(|&i| coarse[i].1 >= coarse[i - 1].1 && coarse[i].1 >= coarse[i + 1].1)
        .map(|i| coarse[i].0)
        .collect();
    picks.sort_by(|a, b| (comb(&wide, *b) * pref(*b)).total_cmp(&(comb(&wide, *a) * pref(*a))));
    picks.truncate(6);
    picks.extend(lengths.iter().copied());
    let mut best: Option<(f64, f64)> = None;
    for b in picks {
        // Refined between 1 % slower and faster, on the narrow strength.
        let fine = (0..=200)
            .map(|k| b * (0.99 + f64::from(k) * 0.0001))
            .max_by(|x, y| comb(&narrow, *x).total_cmp(&comb(&narrow, *y)))
            .unwrap_or(b);
        let (mut tempo, mut bonus) = (fine, 1.0);
        if let Some(l) = lengths.iter().find(|l| (*l - fine).abs() <= 0.004 * fine) {
            tempo = *l;
            bonus = 1.5;
        }
        let score = comb(&wide, tempo) * pref(tempo) * bonus;
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((tempo, score));
        }
    }
    let (tempo, _) = best?;
    // How sure: the beats' onsets against the average and against the strongest ones.
    let mean = wide.iter().map(|v| f64::from(*v)).sum::<f64>() / n as f64;
    let teeth = ((n as f64) / (60.0 * rate / tempo)).floor().max(1.0) as usize;
    let mut sorted = wide.clone();
    sorted.sort_by(|a, b| b.total_cmp(a));
    // The strongest values, one per beat (the dilation repeats each peak over nine blocks).
    let top: f64 = sorted
        .iter()
        .step_by(9)
        .take(teeth)
        .map(|v| f64::from(*v))
        .sum::<f64>()
        / teeth as f64;
    let sure = if top > mean {
        ((comb(&wide, tempo) - mean) / (top - mean)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some((tempo, sure))
}

/// How sure a tempo is: how much of the sound's hits (weighted by how strong they are) fall on
/// a 1/16 or a triplet 1/8 between its beats, above the share that hits at random times would.
pub fn grid_share(beats: &[f64], onsets: &[f64], accent: &dyn Fn(f64) -> f64) -> f64 {
    const NEAR: f64 = 0.015;
    const PLACES: [f64; 7] = [0.0, 0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75, 1.0];
    if beats.len() < 2 {
        return 0.0;
    }
    let (mut on, mut all, mut gaps) = (0.0f64, 0.0f64, 0.0f64);
    let mut counted = 0usize;
    for o in onsets {
        let k = beats.partition_point(|b| b <= o);
        let (Some(i), Some(next)) = (k.checked_sub(1), beats.get(k)) else {
            continue;
        };
        let (a, gap) = (beats[i], next - beats[i]);
        if gap <= 0.0 {
            continue;
        }
        let w = accent(*o).max(1e-9);
        let near = PLACES.iter().any(|f| (o - (a + f * gap)).abs() <= NEAR);
        all += w;
        if near {
            on += w;
        }
        gaps += gap;
        counted += 1;
    }
    if counted == 0 || all <= 0.0 {
        return 0.0;
    }
    // Six places a beat, each NEAR either side.
    let chance = (6.0 * 2.0 * NEAR / (gaps / counted as f64)).min(0.95);
    ((on / all - chance) / (1.0 - chance)).clamp(0.0, 1.0)
}

/// Each beat moved to the hit within `reach` seconds of it, if there is one.
fn snap_beats(beats: &[f64], onsets: &[f64], reach: f64) -> Vec<f64> {
    beats
        .iter()
        .map(|b| {
            onsets
                .iter()
                .copied()
                .filter(|o| (o - b).abs() <= reach)
                .min_by(|x, y| (x - b).abs().total_cmp(&(y - b).abs()))
                .unwrap_or(*b)
        })
        .collect()
}

/// The tempo of beats that are meant to be even: the slope of the straight line through them
/// (least squares, a second time without the beats far off it).
pub fn steady_bpm(beats: &[f64]) -> Option<f64> {
    let line = |pts: &[(f64, f64)]| -> Option<(f64, f64)> {
        let n = pts.len() as f64;
        if pts.len() < 3 {
            return None;
        }
        let (mx, my) = (
            pts.iter().map(|p| p.0).sum::<f64>() / n,
            pts.iter().map(|p| p.1).sum::<f64>() / n,
        );
        let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        (sxx > 0.0).then(|| {
            let slope = sxy / sxx;
            (slope, my - slope * mx)
        })
    };
    let pts: Vec<(f64, f64)> = beats
        .iter()
        .enumerate()
        .map(|(k, b)| (k as f64, *b))
        .collect();
    let (slope, at) = line(&pts)?;
    let near: Vec<(f64, f64)> = pts
        .iter()
        .copied()
        .filter(|(k, b)| (b - (at + slope * k)).abs() <= 0.025)
        .collect();
    let slope = if near.len() * 2 >= pts.len() {
        line(&near).map_or(slope, |l| l.0)
    } else {
        slope
    };
    (slope > 0.0).then(|| 60.0 / slope)
}

/// The project tempo for a file's tempo: the whole number when it is that near one, else to a
/// hundredth.
pub fn project_tempo(bpm: f64) -> f32 {
    let whole = bpm.round();
    let t = if (bpm - whole).abs() <= WHOLE_BPM {
        whole
    } else {
        (bpm * 100.0).round() / 100.0
    };
    t as f32
}

/// Doubles or halves the tempo for the project (into 70–180 for a first file, nearest the
/// project's tempo for the rest), then finds the first downbeat.
fn fold(
    found: &mut Found,
    cx: &Context,
    mono: Option<&[f32]>,
    sr: f64,
    heard: &prowave_engine::Analysis,
) {
    let Some(mut bpm) = found.tempo else { return };
    let accent = |b: f64| accent_at(mono, sr, heard, b);
    let factor = if cx.adopt {
        let mut f = 1.0;
        while bpm * f < ADOPT_RANGE.0 && f < 8.0 {
            f *= 2.0;
        }
        while bpm * f > ADOPT_RANGE.1 && f > 0.125 {
            f /= 2.0;
        }
        f
    } else {
        [0.5, 1.0, 2.0]
            .into_iter()
            .min_by(|a, b| {
                let d = |f: f64| ((bpm * f) / cx.project_bpm.max(1.0)).ln().abs();
                d(*a).total_cmp(&d(*b))
            })
            .unwrap_or(1.0)
    };
    let mut beats = std::mem::take(&mut found.beats);
    let mut f = factor;
    while f >= 2.0 {
        let mut v = Vec::with_capacity(beats.len() * 2);
        for w in beats.windows(2) {
            v.push(w[0]);
            v.push((w[0] + w[1]) / 2.0);
        }
        v.extend(beats.last());
        beats = v;
        f /= 2.0;
    }
    while f <= 0.5 {
        // Every other beat: the ones that sound stronger.
        let strength = |parity: usize| -> f64 {
            beats
                .iter()
                .skip(parity)
                .step_by(2)
                .map(|b| accent(*b))
                .sum::<f64>()
                / beats.iter().skip(parity).step_by(2).count().max(1) as f64
        };
        let parity = usize::from(strength(1) > strength(0) * 1.1);
        beats = beats.iter().skip(parity).step_by(2).copied().collect();
        f *= 2.0;
    }
    bpm *= factor;
    found.tempo = Some(bpm);
    found.downbeat = if found.tagged {
        0
    } else {
        downbeat_of(&beats, cx.beats_per_bar, &accent)
    };
    found.beats = beats;
}

/// Which beat is the first downbeat: the bar's place whose beats sound strongest (the low end
/// counts most: kick drums and bass notes fall on downbeats). When no place stands out, the
/// first beat.
pub fn downbeat_of(beats: &[f64], per_bar: usize, accent: &dyn Fn(f64) -> f64) -> usize {
    let per_bar = per_bar.max(1);
    if beats.len() < per_bar * 2 {
        return 0;
    }
    let scores: Vec<f64> = (0..per_bar)
        .map(|phase| {
            let v: Vec<f64> = beats
                .iter()
                .skip(phase)
                .step_by(per_bar)
                .map(|b| accent(*b))
                .collect();
            v.iter().sum::<f64>() / v.len().max(1) as f64
        })
        .collect();
    let best = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i);
    if scores[best] > scores[0] * 1.15 {
        best
    } else {
        0
    }
}

/// How strong a beat at `at` seconds sounds: the low end's energy just after it, and its peak.
fn accent_at(mono: Option<&[f32]>, sr: f64, heard: &prowave_engine::Analysis, at: f64) -> f64 {
    let Some(m) = mono else {
        // The onset strength near it.
        let rate = prowave_engine::STRENGTH_RATE;
        let i = (at * rate).round() as usize;
        return heard
            .strength
            .get(i.saturating_sub(4)..(i + 8).min(heard.strength.len()))
            .map_or(0.0, |s| s.iter().copied().fold(0.0f32, f32::max).into());
    };
    let a = ((at - 0.005).max(0.0) * sr) as usize;
    let b = (((at + 0.07) * sr) as usize).min(m.len());
    if a >= b {
        return 0.0;
    }
    // A one-pole low-pass at about 150 Hz.
    let k = (1.0 - (-std::f64::consts::TAU * 150.0 / sr).exp()) as f32;
    let (mut low, mut energy, mut peak) = (0.0f32, 0.0f64, 0.0f32);
    for x in &m[a..b] {
        low += k * (x - low);
        energy += f64::from(low * low);
        peak = peak.max(x.abs());
    }
    let rms_low = (energy / (b - a) as f64).sqrt();
    4.0 * rms_low + 0.5 * f64::from(peak)
}

/// The Flex mode for a sound: Slicing when its hits stand out of near silence (drums), Rhythmic
/// when they stand out of a quieter bed, Polyphonic for sustained or full mixes.
pub fn mode_for(mono: &[f32], sr: f64) -> FlexMode {
    let block = ((0.01 * sr) as usize).max(1);
    let mut db: Vec<f64> = mono
        .chunks(block)
        .map(|c| {
            let e = c.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / c.len() as f64;
            10.0 * (e + 1e-12).log10()
        })
        .collect();
    // From the first block that sounds to the last.
    let loudest = db.iter().copied().fold(f64::MIN, f64::max);
    let sounds = |v: &f64| *v > loudest - 45.0;
    let (Some(first), Some(last)) = (db.iter().position(sounds), db.iter().rposition(sounds))
    else {
        return FlexMode::Polyphonic;
    };
    db = db[first..=last]
        .iter()
        .map(|v| v.max(loudest - 60.0))
        .collect();
    db.sort_by(f64::total_cmp);
    let at = |q: f64| db[((db.len() - 1) as f64 * q).round() as usize];
    let spread = at(0.9) - at(0.5);
    if spread >= 20.0 {
        FlexMode::Slicing
    } else if spread >= 9.0 {
        FlexMode::Rhythmic
    } else {
        FlexMode::Polyphonic
    }
}

// ------------------------------------------------------------------ placing

/// How sure the tempo is, said for the status line.
fn sureness(found: &Found) -> String {
    if found.tagged {
        tr("tagged in the file").to_string()
    } else {
        trf("sure: {}%", &[&((found.confidence * 100.0).round() as i64)])
    }
}

/// Places region `r` of track `t` as `found` says, dropped at `origin` (quarter notes).
/// Returns the status line.
pub fn apply(
    p: &mut Project,
    t: usize,
    r: usize,
    found: &Found,
    origin: f64,
    grid: f64,
    adopt: bool,
) -> String {
    let project_bpm = f64::from(p.tempo_map().tempo_at(origin));
    let Some(bpm) = found.tempo else {
        place_unstretched(p, t, r, found, origin, grid);
        return tr("Fit to Grid: one-shot, placed on the grid unstretched").into();
    };
    if found.beats.len() < 4 || found.duration < 60.0 / project_bpm {
        place_unstretched(p, t, r, found, origin, grid);
        return tr("Fit to Grid: one-shot, placed on the grid unstretched").into();
    }
    let sure = sureness(found);
    if !found.tagged && found.confidence < MIN_CONFIDENCE {
        place_unstretched(p, t, r, found, origin, grid);
        return trf(
            "Fit to Grid: no steady tempo ({}), placed on the grid unstretched; the project tempo is unchanged",
            &[&sure],
        );
    }
    let shown = format!("{:.2}", (bpm * 100.0).round() / 100.0);
    if adopt {
        p.tempo = project_tempo(bpm);
        p.tempo_ramp = false;
    }
    let flexed = place_on_grid(p, t, r, found, bpm, origin);
    // The project's tempo as its Tempo display has it: whole, or to a hundredth.
    let now_bpm = f64::from(p.tempo_map().tempo_at(origin));
    let now = if now_bpm.fract() == 0.0 {
        format!("{now_bpm:.0}")
    } else {
        format!("{now_bpm:.2}")
    };
    let mode = flexed.map(crate::flex_edit::mode_label);
    match (adopt, mode) {
        (true, None) => trf(
            "Fit to Grid: {} BPM ({}), the project tempo is now {} BPM",
            &[&shown, &sure, &now],
        ),
        (true, Some(m)) => trf(
            "Fit to Grid: {} BPM ({}), the project tempo is now {} BPM; beats on the grid with Flex {}",
            &[&shown, &sure, &now, &m],
        ),
        (false, Some(m)) => trf(
            "Fit to Grid: {} BPM ({}), stretched to {} BPM with Flex {}",
            &[&shown, &sure, &now, &m],
        ),
        (false, None) => trf(
            "Fit to Grid: {} BPM ({}), already at the project tempo",
            &[&shown, &sure],
        ),
    }
}

/// A one-shot: its first hit on the grid line nearest the drop, nothing stretched.
fn place_unstretched(p: &mut Project, t: usize, r: usize, found: &Found, origin: f64, grid: f64) {
    let map = p.tempo_map();
    let grid = if grid.is_finite() && grid > 0.0 {
        grid
    } else {
        0.25
    };
    let target = ((origin / grid).round() * grid).max(0.0);
    // The first hit, when the sound starts with a little silence.
    let lead = found
        .onsets
        .first()
        .copied()
        .filter(|o| *o < 0.25)
        .unwrap_or(0.0);
    let start_secs = map.seconds_at(target) - lead;
    let start = if start_secs >= 0.0 {
        map.beat_at(start_secs)
    } else {
        target
    };
    let Some(region) = p.tracks.get_mut(t).and_then(|x| x.regions.get_mut(r)) else {
        return;
    };
    // Any Flex markers from before go: the sound plays as recorded.
    if let Some(f) = region.audio.as_mut().and_then(|a| a.flex.as_mut()) {
        f.markers.clear();
        f.follow = None;
    }
    region.params.offset_secs = 0.0;
    let start_bar = p.sigs().beats_to_bar(start) + 1.0;
    let s0 = map.seconds_at(p.sigs().bar_to_beats(f64::from(start_bar as f32) - 1.0));
    let end = map.beat_at(s0 + found.duration);
    let end_bar = p.sigs().beats_to_bar(end) as f32 + 1.0;
    let region = &mut p.tracks[t].regions[r];
    region.start_bar = start_bar as f32;
    region.len_bars = (end_bar - region.start_bar).max(1.0 / 64.0);
    p.song_rev += 1;
}

/// Where each moment of the recording goes: the first downbeat on the bar line nearest the
/// drop, each beat on a beat, the hits near a 1/16 on it. Sets the Flex markers (and the mode)
/// when anything moves; returns the mode then.
fn place_on_grid(
    p: &mut Project,
    t: usize,
    r: usize,
    found: &Found,
    bpm: f64,
    origin: f64,
) -> Option<FlexMode> {
    let map = p.tempo_map();
    let beats = &found.beats;
    let d = found.downbeat.min(beats.len().saturating_sub(1));
    // Quarter notes of the file before its first downbeat.
    let pre = d as f64 + beats.first().copied().unwrap_or(0.0) * bpm / 60.0;
    let sigs = p.sigs();
    let mut target = sigs.nearest_bar_line(origin + pre);
    while target < pre - 1e-9 {
        target = sigs.next_bar_start(target);
    }
    let start = target - pre;
    let start_bar = (sigs.beats_to_bar(start) + 1.0) as f32;
    // The start as it is kept (a 32-bit bar number), so the markers fit it exactly.
    let s0 = map.seconds_at(sigs.bar_to_beats(f64::from(start_bar) - 1.0));
    let first_beat = target - d as f64;
    let markers = grid_markers(&map, found, bpm, first_beat, s0);
    let neutral = markers.iter().all(|m| (m.dst - m.src).abs() < 0.0015);
    let length = markers.last().map_or(found.duration, |m| m.dst);
    let end = map.beat_at(s0 + length);
    let mode = (!neutral).then(|| {
        p.tracks
            .get(t)
            .and_then(|x| x.regions.get(r))
            .and_then(|x| x.audio.as_ref())
            .and_then(|a| a.flex.as_ref())
            .map_or(found.mode, |f| f.mode)
    });
    if let Some(m) = mode {
        crate::flex_edit::set_mode(p, t, r, m);
    }
    let len_bars = p.sigs().beats_to_bar(end) as f32 + 1.0 - start_bar;
    let region = p.tracks.get_mut(t).and_then(|x| x.regions.get_mut(r))?;
    if let Some(f) = region.audio.as_mut().and_then(|a| a.flex.as_mut()) {
        f.markers = if neutral { Vec::new() } else { markers };
        f.follow = None;
    }
    region.params.offset_secs = 0.0;
    region.start_bar = start_bar;
    region.len_bars = len_bars.max(1.0 / 64.0);
    p.song_rev += 1;
    mode
}

/// Flex markers that put beat `k` of the file on quarter note `first_beat + k`, and the hits
/// near a 1/16 line on it; the last one at the end of the recording, so the tail stretches
/// with the rest. `s0` is the region's start in seconds.
pub fn grid_markers(
    map: &TempoMap,
    found: &Found,
    bpm: f64,
    first_beat: f64,
    s0: f64,
) -> Vec<prowave_engine::WarpPoint> {
    let beats = &found.beats;
    let per_sec = bpm / 60.0;
    // The recording's time as quarter notes of the project, through its beats.
    let place = |src: f64| -> f64 {
        let k = beats.partition_point(|b| *b <= src);
        match (k.checked_sub(1), beats.get(k)) {
            (Some(i), Some(next)) if next > &beats[i] => {
                first_beat + i as f64 + (src - beats[i]) / (next - beats[i])
            }
            (Some(i), _) => first_beat + i as f64 + (src - beats[i]) * per_sec,
            (None, _) => first_beat - (beats.first().copied().unwrap_or(0.0) - src) * per_sec,
        }
    };
    let dst = |q: f64| map.seconds_at(q) - s0;
    let mut pairs: Vec<(f64, f64)> = beats
        .iter()
        .enumerate()
        .map(|(k, b)| (*b, dst(first_beat + k as f64)))
        .collect();
    for o in &found.onsets {
        if beats.iter().any(|b| (b - o).abs() < 0.02) {
            continue;
        }
        let at = place(*o);
        let q = (at * 4.0).round() / 4.0;
        if (at - q).abs() <= SNAP_REACH {
            pairs.push((*o, dst(q)));
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Each marker a little before its hit, so the hit's whole attack moves with it (a slice
    // that ends at the hit would keep the very start of it behind).
    let mut markers = markers_from(
        pairs
            .into_iter()
            .map(|(src, dst)| (src - PRE_ATTACK, dst - PRE_ATTACK)),
    );
    // The tail after the last marker plays at the project's speed.
    let end = found.duration;
    let tail_to = dst(place(end));
    if markers
        .last()
        .is_none_or(|m| end > m.src + 0.002 && tail_to > m.dst + 0.002)
    {
        markers.push(prowave_engine::WarpPoint {
            src: end,
            dst: tail_to,
        });
    }
    markers
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A drum loop: a kick (a low thump) on each downbeat, a snare-like noise burst on the other
    /// beats and a quiet hat on the off-beats. `pickup` beats come before the first downbeat,
    /// which is at `start` seconds; `wobble` is how far the tempo sways (0.01 is 1 %) around
    /// `bpm`, returning to it.
    pub(crate) fn drum_loop(
        bpm: f64,
        beats: usize,
        start: f64,
        pickup: usize,
        wobble: f64,
        accent: bool,
    ) -> (Vec<f32>, Vec<f64>) {
        let mut times = Vec::new();
        let mut at = start;
        for k in 0..beats {
            times.push(at);
            let sway = 1.0 + wobble * (std::f64::consts::TAU * k as f64 / 8.0).sin();
            at += 60.0 / (bpm * sway);
        }
        let seconds = at + 0.6;
        let n = (seconds * f64::from(RATE)) as usize;
        let mut v = vec![0.0f32; n];
        let mut seed = 7u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        for (k, t) in times.iter().enumerate() {
            let a = (t * f64::from(RATE)) as usize;
            let down = (k + 4 - pickup % 4).is_multiple_of(4);
            for i in 0..(0.25 * f64::from(RATE)) as usize {
                let s = i as f32 / RATE as f32;
                let x = if down && accent {
                    // A kick: the beater's click, then a thump falling from 120 to 50 Hz.
                    let phase = std::f32::consts::TAU
                        * (50.0 * s + 70.0 / 30.0 * (1.0 - (-s * 30.0).exp()));
                    phase.sin() * (-s * 14.0).exp() * 0.8 + noise() * (-s * 400.0).exp() * 0.4
                } else {
                    noise() * (-s * 60.0).exp() * 0.5
                };
                if let Some(o) = v.get_mut(a + i) {
                    *o += x;
                }
            }
            // A hat half way to the next beat.
            let h = ((t + 30.0 / bpm) * f64::from(RATE)) as usize;
            for i in 0..(0.03 * f64::from(RATE)) as usize {
                let s = i as f32 / RATE as f32;
                if let Some(o) = v.get_mut(h + i) {
                    *o += noise() * (-s * 200.0).exp() * 0.12;
                }
            }
        }
        (v, times)
    }

    /// Notes with a slow attack and a long ring that overlap (a pad or chords), one a beat.
    pub(crate) fn tonal_loop(bpm: f64, beats: usize) -> Vec<f32> {
        let gap = 60.0 / bpm;
        let n = ((beats as f64 + 2.0) * gap * f64::from(RATE)) as usize;
        let mut v = vec![0.0f32; n];
        let notes = [220.0f32, 277.18, 329.63, 246.94];
        for k in 0..beats {
            let a = (k as f64 * gap * f64::from(RATE)) as usize;
            let hz = notes[k % notes.len()];
            for i in 0..(2.5 * gap * f64::from(RATE)) as usize {
                let s = i as f32 / RATE as f32;
                let env = (s / 0.02).min(1.0) * (-s * 0.8).exp();
                let x = ((std::f32::consts::TAU * hz * s).sin()
                    + 0.5 * (std::f32::consts::TAU * hz * 1.5 * s).sin())
                    * env
                    * 0.25;
                if let Some(o) = v.get_mut(a + i) {
                    *o += x;
                }
            }
        }
        v
    }

    pub(crate) fn write_wav(path: &std::path::Path, mono: &[f32]) {
        let stereo: Vec<f32> = mono.iter().flat_map(|x| [*x, *x]).collect();
        prowave_engine::write_wav(path, &stereo, RATE).unwrap();
    }

    fn cx(adopt: bool, project_bpm: f64) -> Context {
        Context {
            adopt,
            project_bpm,
            beats_per_bar: 4,
        }
    }

    #[test]
    fn whole_tempos_are_rounded_and_others_kept_to_a_hundredth() {
        assert_eq!(project_tempo(119.93), 120.0);
        assert_eq!(project_tempo(93.5), 93.5);
        assert_eq!(project_tempo(87.333), 87.33);
        assert_eq!(
            steady_bpm(&[0.0, 0.5, 1.0, 1.5]).map(|b| b.round()),
            Some(120.0)
        );
        assert_eq!(steady_bpm(&[0.0, 0.5]), None);
    }

    #[test]
    fn the_downbeat_is_the_strongest_place_in_the_bar_or_else_the_first_beat() {
        let beats: Vec<f64> = (0..12).map(|k| k as f64 * 0.5).collect();
        let even = |_: f64| 1.0;
        assert_eq!(downbeat_of(&beats, 4, &even), 0);
        let third = |b: f64| {
            if ((b / 0.5).round() as usize) % 4 == 2 {
                3.0
            } else {
                1.0
            }
        };
        assert_eq!(downbeat_of(&beats, 4, &third), 2);
        assert_eq!(
            downbeat_of(&beats[..5], 4, &third),
            0,
            "too few beats to tell"
        );
    }

    #[test]
    fn the_tempo_of_drum_loops_is_found() {
        let dir = tempfile::tempdir().unwrap();
        for (bpm, start, pickup) in [
            (87.0, 0.31, 0),
            (93.5, 0.12, 2),
            (120.0, 0.05, 1),
            (140.0, 0.4, 0),
        ] {
            let (v, times) = drum_loop(bpm, 24, start, pickup, 0.01, true);
            let wav = dir.path().join(format!("{bpm}.wav"));
            write_wav(&wav, &v);
            let file = AudioFile::open(&wav).unwrap();
            let f = analyse(&file, &cx(true, 120.0)).unwrap();
            let got = f.tempo.unwrap();
            eprintln!(
                "{bpm} BPM: found {got:.3} (sure {:.2}), mode {:?}, downbeat {} at {:.4} (true {:.4})",
                f.confidence,
                f.mode,
                f.downbeat,
                f.beats[f.downbeat],
                times[pickup]
            );
            assert!((got - bpm).abs() <= 0.5, "{bpm}: {got}");
            // A whole tempo exactly, another to a twentieth.
            let set = f64::from(project_tempo(got));
            if bpm.fract() == 0.0 {
                assert_eq!(set, bpm, "{bpm}: {got}");
            } else {
                assert!((set - bpm).abs() <= 0.05, "{bpm}: {got}");
            }
            assert!(
                (f.beats[f.downbeat] - times[pickup]).abs() < 0.005,
                "{bpm}: downbeat at {} not {}",
                f.beats[f.downbeat],
                times[pickup]
            );
            assert!(
                matches!(f.mode, FlexMode::Slicing | FlexMode::Rhythmic),
                "{:?}",
                f.mode
            );
        }
    }

    #[test]
    fn a_sustained_loop_is_polyphonic_and_a_one_shot_has_no_tempo() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("pad.wav");
        write_wav(&wav, &tonal_loop(100.0, 16));
        let f = analyse(&AudioFile::open(&wav).unwrap(), &cx(false, 120.0)).unwrap();
        assert_eq!(f.mode, FlexMode::Polyphonic);
        let shot = dir.path().join("shot.wav");
        let (v, _) = drum_loop(120.0, 1, 0.02, 0, 0.0, true);
        write_wav(&shot, &v);
        let f = analyse(&AudioFile::open(&shot).unwrap(), &cx(false, 120.0)).unwrap();
        assert_eq!(f.tempo, None);
        assert!((f.onsets[0] - 0.02).abs() < 0.003, "{:?}", f.onsets);
    }

    #[test]
    fn half_and_double_time_follow_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let (v, _) = drum_loop(90.0, 24, 0.1, 0, 0.0, true);
        let wav = dir.path().join("l.wav");
        write_wav(&wav, &v);
        let file = AudioFile::open(&wav).unwrap();
        let h = file.analyze().unwrap();
        eprintln!(
            "heard {:?} beats {:?}",
            h.tempo,
            &h.beats[..h.beats.len().min(20)]
        );
        let f = analyse(&file, &cx(false, 170.0)).unwrap();
        eprintln!(
            "fit {:?} beats {:?}",
            f.tempo,
            &f.beats[..f.beats.len().min(20)]
        );
        assert!((f.tempo.unwrap() - 180.0).abs() < 1.0, "{:?}", f.tempo);
        let f = analyse(&file, &cx(false, 100.0)).unwrap();
        assert!((f.tempo.unwrap() - 90.0).abs() < 1.0, "{:?}", f.tempo);
    }

    /// The tempo heard in real loops against the tempo in their tags (READ ONLY). Run with
    /// `cargo test -p prowave-app fit_grid::tests::real_loops -- --ignored --nocapture`;
    /// FIT_LOOPS_DIR picks the folder, FIT_LOOPS_N how many files.
    #[test]
    #[ignore]
    fn real_loops_heard_against_tagged() {
        let dir =
            std::env::var("FIT_LOOPS_DIR").unwrap_or_else(|_| "/Library/Audio/Apple Loops".into());
        let n: usize = std::env::var("FIT_LOOPS_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(80);
        let mut files = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "caf") {
                    files.push(path);
                }
            }
        }
        files.sort();
        let tagged: Vec<(std::path::PathBuf, f64)> = files
            .into_iter()
            .filter_map(|f| {
                let t = crate::apple_loops::read_file(&f)?.tempo?;
                Some((f, f64::from(t)))
            })
            .collect();
        let step = (tagged.len() / n.max(1)).max(1);
        let (mut exact, mut octave, mut wrong, mut none, mut count) = (0, 0, 0, 0, 0);
        let (mut sure_exact, mut sure_octave, mut sure_wrong) = (0, 0, 0);
        let mut errors = Vec::new();
        let tmp = tempfile::tempdir().unwrap();
        // FIT_LOOPS_SKIP starts the sample further in (another set of loops).
        let skip: usize = std::env::var("FIT_LOOPS_SKIP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        for (src, tag) in tagged.iter().skip(skip).step_by(step).take(n) {
            let path = src;
            // Apple's AAC loops do not decode here: a WAV copy is made with afconvert.
            let wav = tmp.path().join("loop.wav");
            let path = if AudioFile::open(path).is_err() {
                let made = std::process::Command::new("afconvert")
                    .args(["-f", "WAVE", "-d", "LEI16"])
                    .arg(path)
                    .arg(&wav)
                    .status()
                    .is_ok_and(|s| s.success());
                if !made {
                    continue;
                }
                &wav
            } else {
                path
            };
            let file = match AudioFile::open(path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("cannot open {}: {e}", path.display());
                    continue;
                }
            };
            let f = match analyse_with(&file, &cx(true, 120.0), false) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("cannot read {}: {e}", path.display());
                    continue;
                }
            };
            count += 1;
            let name = src.file_name().unwrap().to_string_lossy();
            match f.tempo {
                None => {
                    none += 1;
                    eprintln!("NONE   {tag:7.2} {name}");
                }
                Some(b) if (b - tag).abs() <= 0.5 => {
                    exact += 1;
                    sure_exact += usize::from(f.confidence >= MIN_CONFIDENCE);
                    errors.push((b - tag).abs());
                    eprintln!(
                        "ok     {tag:7.2} {b:7.2} sure {:.2} {:?} {name}",
                        f.confidence, f.mode
                    );
                }
                Some(b) if (b * 2.0 - tag).abs() <= 1.0 || (b / 2.0 - tag).abs() <= 0.5 => {
                    octave += 1;
                    sure_octave += usize::from(f.confidence >= MIN_CONFIDENCE);
                    eprintln!("OCTAVE {tag:7.2} {b:7.2} sure {:.2} {name}", f.confidence);
                }
                Some(b) => {
                    wrong += 1;
                    sure_wrong += usize::from(f.confidence >= MIN_CONFIDENCE);
                    eprintln!("WRONG  {tag:7.2} {b:7.2} sure {:.2} {name}", f.confidence);
                }
            }
        }
        eprintln!(
            "of those sure enough to set a tempo ({MIN_CONFIDENCE}): {sure_exact} within 0.5 BPM, {sure_octave} twice or half, {sure_wrong} wrong"
        );
        errors.sort_by(f64::total_cmp);
        let median = errors.get(errors.len() / 2).copied().unwrap_or(0.0);
        eprintln!(
            "{count} loops: {exact} within 0.5 BPM (median error {median:.3}), {octave} twice or half, {wrong} wrong, {none} no tempo"
        );
    }

    /// Runs Fit to Grid and Flex until both are done.
    pub(crate) fn settle(p: &mut Project, s: &mut UiState) {
        let ctx = egui::Context::default();
        let began = std::time::Instant::now();
        loop {
            poll(&ctx, p, s);
            crate::flex_edit::poll(&ctx, p, s);
            if !s.fit.busy() && !s.flex.busy() {
                break;
            }
            assert!(
                began.elapsed().as_secs() < 120,
                "Fit to Grid never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The hits the region plays, in quarter notes of the project (from the file it plays now).
    fn hits_in_song(p: &Project, t: usize, r: usize) -> Vec<f64> {
        let region = &p.tracks[t].regions[r];
        let a = region.audio.as_ref().unwrap();
        let map = p.tempo_map();
        let s0 = map.seconds_at(p.region_start_beats(region)) - region.params.offset_secs;
        let stereo = a.file.read_stereo().unwrap();
        let buf = SampleBuf::new(a.file.sample_rate, stereo);
        let sr = f64::from(a.file.sample_rate);
        transients(&buf, 0, buf.frames(), 0.6)
            .into_iter()
            .map(|f| map.beat_at(s0 + f as f64 / sr))
            .collect()
    }

    /// How far (in seconds) `beat` is from the nearest line of a grid of `step` quarter notes.
    fn off_grid(p: &Project, beat: f64, step: f64) -> f64 {
        let map = p.tempo_map();
        let q = (beat / step).round() * step;
        (map.seconds_at(beat) - map.seconds_at(q)).abs()
    }

    fn drop_file(p: &mut Project, s: &mut UiState, path: &std::path::Path, raw: bool) {
        import_dropped(p, s, path, &egui::Context::default(), raw).unwrap();
    }

    #[test]
    fn the_first_file_sets_the_tempo_and_its_downbeat_lands_on_a_bar() {
        let dir = tempfile::tempdir().unwrap();
        // 93.5 BPM, swaying 1.5 %, one beat of pickup before the first downbeat.
        let (v, times) = drum_loop(93.5, 20, 0.21, 1, 0.015, true);
        let wav = dir.path().join("first.wav");
        write_wav(&wav, &v);
        let mut p = Project::empty();
        let (tempo_before, tracks_before) = (p.tempo, p.tracks.len());
        let mut history = crate::undo::History::new(&p);
        let mut s = UiState::new();
        drop_file(&mut p, &mut s, &wav, false);
        assert!(s.fit.busy());
        assert_eq!(s.status.as_deref(), Some("Fit to Grid: analysing…"));
        settle(&mut p, &mut s);
        let said = s.status.clone().unwrap();
        eprintln!("{said}");
        assert!((p.tempo - 93.5).abs() <= 0.05, "{} ({said})", p.tempo);
        assert!(said.contains("the project tempo is now 93.5"), "{said}");
        let t = p.tracks.len() - 1;
        let region = &p.tracks[t].regions[0];
        let f = region
            .audio
            .as_ref()
            .unwrap()
            .flex
            .as_ref()
            .expect("the sway is taken out");
        assert!(
            matches!(f.mode, FlexMode::Slicing | FlexMode::Rhythmic),
            "{:?}",
            f.mode
        );
        // The first downbeat of the recording plays on a bar line.
        let map = p.tempo_map();
        let s0 = map.seconds_at(p.region_start_beats(region));
        let down = map.beat_at(s0 + f.place_of(times[1]));
        let bar = p.sigs().nearest_bar_line(down);
        let off = (map.seconds_at(down) - map.seconds_at(bar)).abs();
        assert!(
            off < 0.005,
            "downbeat {down} is {off} s from bar line {bar}"
        );
        // Every hit of the result is on an eighth note.
        let hits = hits_in_song(&p, t, 0);
        assert!(hits.len() >= 30, "{}", hits.len());
        let worst = hits
            .iter()
            .map(|h| off_grid(&p, *h, 0.5))
            .fold(0.0, f64::max);
        eprintln!(
            "first file: {} hits, the farthest {:.1} ms off the grid",
            hits.len(),
            worst * 1000.0
        );
        assert!(worst < 0.010, "{worst}");
        // The recording is untouched; dropping and fitting are one Undo step.
        assert_eq!(f.original.path, wav);
        assert!(history.record(&p));
        assert!(history.undo(&mut p));
        assert_eq!((p.tempo, p.tracks.len()), (tempo_before, tracks_before));
        assert!(!history.can_undo());
    }

    #[test]
    fn a_later_file_keeps_the_project_tempo_and_is_stretched_onto_the_grid() {
        let dir = tempfile::tempdir().unwrap();
        let (v, _) = drum_loop(120.0, 16, 0.0, 0, 0.0, true);
        let first = dir.path().join("first.wav");
        write_wav(&first, &v);
        let mut p = Project::empty();
        let mut s = UiState::new();
        drop_file(&mut p, &mut s, &first, false);
        settle(&mut p, &mut s);
        assert_eq!(p.tempo, 120.0, "{:?}", s.status);
        // A drifting 87 BPM loop, its first hit 0.3 s in, dropped at beat 6.5.
        let (v, times) = drum_loop(87.0, 24, 0.3, 0, 0.01, true);
        let later = dir.path().join("later.wav");
        write_wav(&later, &v);
        p.position_beats = 6.5;
        drop_file(&mut p, &mut s, &later, false);
        settle(&mut p, &mut s);
        let said = s.status.clone().unwrap();
        eprintln!("{said}");
        assert_eq!(p.tempo, 120.0);
        assert!(said.contains("stretched to 120 BPM with Flex"), "{said}");
        let t = p.tracks.len() - 1;
        let region = &p.tracks[t].regions[0];
        let f = region.audio.as_ref().unwrap().flex.as_ref().unwrap();
        // The downbeat at the bar line nearest the drop (bar 3, beat 8).
        let map = p.tempo_map();
        let s0 = map.seconds_at(p.region_start_beats(region));
        let down = map.beat_at(s0 + f.place_of(times[0]));
        assert!(
            (map.seconds_at(down) - map.seconds_at(8.0)).abs() < 0.005,
            "{down}"
        );
        // Played at 120: 24 beats of 87 last 12 seconds now.
        let hits = hits_in_song(&p, t, 0);
        let worst = hits
            .iter()
            .map(|h| off_grid(&p, *h, 0.5))
            .fold(0.0, f64::max);
        eprintln!(
            "later file: {} hits, the farthest {:.1} ms off the grid",
            hits.len(),
            worst * 1000.0
        );
        assert!(hits.len() >= 40, "{}", hits.len());
        assert!(worst < 0.010, "{worst}");
        let span = hits.last().unwrap() - hits.first().unwrap();
        assert!((span - 23.5).abs() < 0.1, "{span}");
        // The menu does it again to the region, as it is: nothing changes.
        let before = f.markers.clone();
        start_region(&p, &mut s, t, 0);
        settle(&mut p, &mut s);
        let f = p.tracks[t].regions[0]
            .audio
            .as_ref()
            .unwrap()
            .flex
            .as_ref()
            .unwrap();
        assert_eq!(f.markers.len(), before.len());
    }

    #[test]
    fn a_tonal_loop_is_stretched_polyphonic() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::empty();
        let mut s = UiState::new();
        let (v, _) = drum_loop(120.0, 16, 0.0, 0, 0.0, true);
        let first = dir.path().join("drums.wav");
        write_wav(&first, &v);
        drop_file(&mut p, &mut s, &first, false);
        settle(&mut p, &mut s);
        let pad = dir.path().join("pad.wav");
        write_wav(&pad, &tonal_loop(100.0, 16));
        drop_file(&mut p, &mut s, &pad, false);
        settle(&mut p, &mut s);
        let said = s.status.clone().unwrap();
        assert!(said.contains("with Flex Polyphonic"), "{said}");
    }

    #[test]
    fn a_one_shot_is_placed_on_the_grid_and_an_option_drop_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (v, _) = drum_loop(120.0, 1, 0.03, 0, 0.0, true);
        let shot = dir.path().join("kick.wav");
        write_wav(&shot, &v);
        let mut p = Project::empty();
        let mut s = UiState::new();
        p.position_beats = 1.3;
        drop_file(&mut p, &mut s, &shot, false);
        settle(&mut p, &mut s);
        assert_eq!(
            s.status.as_deref(),
            Some("Fit to Grid: one-shot, placed on the grid unstretched")
        );
        assert_eq!(p.tempo, 120.0);
        let t = p.tracks.len() - 1;
        // On the Tracks area's snap nearest the playhead.
        let g = grid_of(&p, &s, 1.3);
        let hit = hits_in_song(&p, t, 0)[0];
        assert!(off_grid(&p, hit, g) < 0.002, "{hit}");
        assert!(
            (hit - (1.3 / g).round() * g).abs() < 0.01,
            "{hit} on a grid of {g}"
        );
        assert!(p.tracks[t].regions[0]
            .audio
            .as_ref()
            .unwrap()
            .flex
            .is_none());

        // Option held: as before, at the playhead's bar, nothing analysed.
        let (v, _) = drum_loop(93.0, 16, 0.2, 0, 0.0, true);
        let raw = dir.path().join("raw.wav");
        write_wav(&raw, &v);
        let mut p = Project::empty();
        let mut s = UiState::new();
        p.position_beats = 5.0;
        drop_file(&mut p, &mut s, &raw, true);
        assert!(!s.fit.busy());
        let r = &p.tracks[p.tracks.len() - 1].regions[0];
        assert_eq!(r.start_bar, 2.0);
        assert!(r.audio.as_ref().unwrap().flex.is_none());
        assert_eq!(p.tempo, 120.0);
        // Fit to Grid on Drop off: the same.
        s.fit.on_drop = false;
        drop_file(&mut p, &mut s, &raw, false);
        assert!(!s.fit.busy());
    }

    #[test]
    fn an_unsure_tempo_is_not_given_to_the_project() {
        let dir = tempfile::tempdir().unwrap();
        // Hits at random times: no tempo anyone could keep.
        let mut seed = 99u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            f64::from(seed >> 8) / f64::from(1u32 << 24)
        };
        let mut v = vec![0.0f32; 8 * RATE as usize];
        let mut at = 0.1;
        while at < 7.5 {
            let a = (at * f64::from(RATE)) as usize;
            for i in 0..2000 {
                v[a + i] += (rnd() as f32 * 2.0 - 1.0) * (-(i as f32) / 300.0).exp() * 0.5;
            }
            at += 0.08 + rnd() * 0.5;
        }
        let wav = dir.path().join("random.wav");
        write_wav(&wav, &v);
        let mut p = Project::empty();
        let mut s = UiState::new();
        drop_file(&mut p, &mut s, &wav, false);
        settle(&mut p, &mut s);
        let said = s.status.clone().unwrap();
        eprintln!("{said}");
        assert!(
            said.starts_with("Fit to Grid: no steady tempo (sure: "),
            "{said}"
        );
        assert_eq!(p.tempo, 120.0);
    }

    #[test]
    fn a_region_gone_while_it_is_read_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (v, _) = drum_loop(100.0, 16, 0.1, 0, 0.0, true);
        let wav = dir.path().join("x.wav");
        write_wav(&wav, &v);
        let mut p = Project::empty();
        let mut s = UiState::new();
        drop_file(&mut p, &mut s, &wav, false);
        let t = p.tracks.len() - 1;
        p.tracks[t].regions.clear();
        settle(&mut p, &mut s);
        assert_eq!(p.tempo, 120.0);
        // A MIDI region has nothing to fit.
        let mut s = UiState::new();
        start_region(&Project::demo(), &mut s, 0, 0);
        assert!(!s.fit.busy());
        assert_eq!(
            s.status.as_deref(),
            Some("Fit to Grid: this region has no recorded audio")
        );
    }

    /// A before/after demo: a drifting 100 BPM loop against a 120 BPM click, dropped raw, then
    /// dropped with Fit to Grid (the project already at 120 from an earlier file). Each part is
    /// a direct mix of what the click and the region play at their places in the song. Run with
    /// FIT_DEMO_OUT=<file.wav> `cargo test -p prowave-app fit_grid::tests::demo -- --ignored`.
    #[test]
    #[ignore]
    fn demo_before_and_after() {
        let Ok(out) = std::env::var("FIT_DEMO_OUT") else {
            return;
        };
        assert!(
            !std::path::Path::new(&out).exists(),
            "never overwrite {out}"
        );
        let dir = tempfile::tempdir().unwrap();
        let (v, _) = drum_loop(100.0, 32, 0.37, 0, 0.02, true);
        let wav = dir.path().join("drifting loop.wav");
        write_wav(&wav, &v);
        // The project at 120 (its first audio set it: a short steady loop at 120).
        let (first, _) = drum_loop(120.0, 8, 0.0, 0, 0.0, false);
        let first_wav = dir.path().join("first.wav");
        write_wav(&first_wav, &first);
        let mix = |raw: bool| -> Vec<f32> {
            let mut p = Project::empty();
            let mut s = UiState::new();
            drop_file(&mut p, &mut s, &first_wav, false);
            settle(&mut p, &mut s);
            assert_eq!(p.tempo, 120.0);
            p.position_beats = 0.0;
            drop_file(&mut p, &mut s, &wav, raw);
            settle(&mut p, &mut s);
            eprintln!("{}: {:?}", if raw { "before" } else { "after" }, s.status);
            let t = p.tracks.len() - 1;
            let region = &p.tracks[t].regions[0];
            let a = region.audio.as_ref().unwrap();
            let map = p.tempo_map();
            let s0 = map.seconds_at(p.region_start_beats(region));
            let played = a.file.read_stereo().unwrap();
            let secs = 22.0;
            let n = (secs * f64::from(RATE)) as usize;
            let mut out = vec![0.0f32; n * 2];
            let start = (s0 * f64::from(RATE)) as usize;
            for (i, x) in played.iter().enumerate() {
                if let Some(o) = out.get_mut(start * 2 + i) {
                    *o += x * 0.8;
                }
            }
            // The click: a short tone on every beat, higher on the bar.
            let mut beat = 0.0;
            while map.seconds_at(beat) < secs {
                let at = (map.seconds_at(beat) * f64::from(RATE)) as usize;
                let hz = if beat % 4.0 == 0.0 { 2000.0 } else { 1000.0 };
                for i in 0..(0.02 * f64::from(RATE)) as usize {
                    let s = i as f32 / RATE as f32;
                    let x = (std::f32::consts::TAU * hz * s).sin() * (-s * 200.0).exp() * 0.3;
                    for c in 0..2 {
                        if let Some(o) = out.get_mut((at + i) * 2 + c) {
                            *o += x;
                        }
                    }
                }
                beat += 1.0;
            }
            out
        };
        let mut all = mix(true);
        all.extend(vec![0.0f32; 2 * RATE as usize]);
        all.extend(mix(false));
        let peak = all.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        if peak > 0.98 {
            for x in &mut all {
                *x *= 0.98 / peak;
            }
        }
        prowave_engine::write_wav(std::path::Path::new(&out), &all, RATE).unwrap();
    }
}
