//! The Pitch Editor: a note editor for audio in the manner of Celemony Melodyne, in the editor
//! pane under the tracks (where the Piano Roll and the Channel Rack open). Double-click an
//! audio region, or choose Window > Pitch Editor.
//!
//! What follows the Melodyne 5 manual (sections named as there):
//! - "Audio characteristics and algorithms": the notes are found with the Melodic algorithm
//!   (one note at a time), the Percussive one (hits only, drawn on one line) or the Universal
//!   one (slices, drawn on one line, moved as a whole). The Polyphonic algorithm is not here:
//!   see below.
//! - "Main Tool": drag a note's middle up or down (pitch, on the pitch grid; Option for free)
//!   or sideways (time; the first movement decides which, as in Melodyne), drag its front or
//!   back to stretch it; a double-click puts it exactly on the nearest grid note.
//! - "Pitch Tool", "Pitch modulation and drift", "Formant Tool", "Amplitude Tool", "Timing
//!   Tool", "The Note Separation Tool": each its own tool. Modulation and drift run from 0 to
//!   200 % of the recorded amount (a double-click toggles between none and as recorded).
//! - "Correct Pitch Macro" (pitch centre % and drift %) and "Quantize Time Macro" (towards the
//!   grid, with a strength), on the selected notes or, with none selected, on all.
//! - "The Pitch Grid": chromatic, the project key's scale, or off.
//!
//! The edit is kept on the region ([`MelodyEdit`], inside its Flex edit) and saved with the
//! project. The sound is made from it in the background, like every Flex edit (see
//! `flex_edit.rs`): only the notes whose pitch, drift, modulation or formants changed are
//! made again (`prowave_engine::note_render`, WORLD for Melodic notes, chosen by measurement),
//! each changed stretch kept in memory so the next edit only makes what changed; levels and
//! times are applied to the whole after. Playback plays the made file: nothing new runs on the
//! audio thread.
//!
//! The notes are found once per recording and algorithm and kept on disk by the recording's
//! content hash (`pitch-analysis/` in the Flex cache), so a replaced file is found again.
//!
//! Not here: the Polyphonic algorithm. Editing the notes of a chord needs each note's sound
//! separated from the others and made again on its own; without that, moving one note of a
//! chord moves the whole chord's sound, which would only pretend to be a polyphonic editor.
use crate::i18n::{tr, trf};
use crate::model::{Project, TrackKind};
use crate::{Bottom, UiState};
use egui::{pos2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui};
use prowave_engine::melody::{find_notes, pitch_centre, Algorithm, Blob, NoteAnalysis};
use prowave_engine::note_render::{self, NoteChange, SegmentCache};
use prowave_engine::{AudioFile, FlexMode, PitchTrack, WarpPoint};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// The most notes a region keeps from a file.
const MAX_NOTES: usize = 20_000;
/// The shortest a note can be made (s).
const MIN_LEN: f64 = 0.02;
/// Two notes this close (s) are connected: moving the edge of one moves the other's (the
/// Timing Tool's "connected" notes).
const CONNECTED: f64 = 0.015;

// ------------------------------------------------------------------ the edit

/// One note of the Pitch Editor: where it is in the recording (seconds into the file) and
/// what was changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteEdit {
    pub start: f64,
    pub end: f64,
    /// The pitch centre found (MIDI, with its fraction); 0 for an unpitched note.
    pub pitch: f32,
    /// Where its pitch begins: its musical start, for Quantize Time.
    pub onset: f64,
    /// Where it now plays, in the recording's time.
    pub new_start: f64,
    pub new_end: f64,
    /// Semitones added to the pitch centre.
    pub shift: f32,
    /// How much of the recorded drift and modulation is kept (1 = as recorded), 0 to 2.
    pub drift: f32,
    pub modulation: f32,
    /// Formants moved, in semitones, independent of the pitch.
    pub formant: f32,
    pub gain_db: f32,
}

impl NoteEdit {
    pub fn from_blob(b: &Blob) -> Self {
        Self {
            start: b.start,
            end: b.end,
            pitch: b.pitch,
            onset: b.onset,
            new_start: b.start,
            new_end: b.end,
            shift: 0.0,
            drift: 1.0,
            modulation: 1.0,
            formant: 0.0,
            gain_db: 0.0,
        }
    }

    fn change(&self) -> NoteChange {
        NoteChange {
            start: self.start,
            end: self.end,
            shift: self.shift,
            drift: self.drift,
            modulation: self.modulation,
            formant: self.formant,
        }
    }

    fn moved(&self) -> bool {
        (self.new_start - self.start).abs() > 1e-6 || (self.new_end - self.end).abs() > 1e-6
    }

    pub fn is_neutral(&self) -> bool {
        self.change().is_neutral() && self.gain_db.abs() < 1e-3 && !self.moved()
    }

    /// The pitch centre it is heard at now.
    pub fn heard(&self) -> f32 {
        self.pitch + self.shift
    }

    /// Where moment `t` of the recording (inside the note) now plays.
    pub fn place(&self, t: f64) -> f64 {
        let len = self.end - self.start;
        if len <= 0.0 {
            return self.new_start;
        }
        self.new_start + (t - self.start) * (self.new_end - self.new_start) / len
    }
}

/// The Pitch Editor's edit of a region: its notes, found by `algorithm`.
#[derive(Clone, Debug, PartialEq)]
pub struct MelodyEdit {
    pub algorithm: Algorithm,
    pub notes: Vec<NoteEdit>,
}

impl MelodyEdit {
    pub fn is_neutral(&self) -> bool {
        self.notes.iter().all(NoteEdit::is_neutral)
    }

    /// Mixes everything that changes the sound into a fingerprint.
    pub fn fingerprint(&self, mix: &mut impl FnMut(u64)) {
        for b in self.algorithm.name().bytes() {
            mix(u64::from(b));
        }
        for n in self.notes.iter().filter(|n| !n.is_neutral()) {
            for v in [n.start, n.end, n.new_start, n.new_end] {
                mix(v.to_bits());
            }
            for v in [n.shift, n.drift, n.modulation, n.formant, n.gain_db] {
                mix(u64::from(v.to_bits()));
            }
        }
    }

    /// The time map of the moved notes: each note's start and end where they now play. Empty
    /// when nothing moved.
    pub fn warp_points(&self, duration: f64) -> Vec<WarpPoint> {
        if !self.notes.iter().any(NoteEdit::moved) {
            return Vec::new();
        }
        let mut pts: Vec<WarpPoint> = Vec::new();
        for n in &self.notes {
            for (src, dst) in [(n.start, n.new_start), (n.end, n.new_end)] {
                if pts
                    .last()
                    .is_none_or(|l| src > l.src + 1e-4 && dst > l.dst + 1e-4)
                {
                    pts.push(WarpPoint { src, dst });
                }
            }
        }
        // The recording keeps its length: the end stays where it is.
        if pts
            .last()
            .is_none_or(|l| duration > l.src + 0.002 && duration > l.dst + 0.002)
        {
            pts.push(WarpPoint {
                src: duration,
                dst: duration,
            });
        }
        pts.retain(|p| p.src > 0.0 && p.dst > 0.0);
        pts
    }
}

pub fn to_doc(m: &MelodyEdit) -> prowave_project::MelodyDoc {
    prowave_project::MelodyDoc {
        algorithm: m.algorithm.name().into(),
        notes: m
            .notes
            .iter()
            .map(|n| prowave_project::NoteEditDoc {
                start: n.start,
                end: n.end,
                pitch: n.pitch,
                onset: n.onset,
                new_start: n.new_start,
                new_end: n.new_end,
                shift: n.shift,
                drift: n.drift,
                modulation: n.modulation,
                formant: n.formant,
                gain_db: n.gain_db,
            })
            .collect(),
    }
}

/// A Pitch Editor edit from a file: only notes with finite times in order and values in their
/// ranges are kept.
pub fn from_doc(d: &prowave_project::MelodyDoc) -> Option<MelodyEdit> {
    let algorithm = Algorithm::from_name(&d.algorithm).unwrap_or_default();
    let t = |x: f64| x.is_finite() && (0.0..=2400.0).contains(&x);
    let f = |x: f32, lo: f32, hi: f32, or: f32| if x.is_finite() { x.clamp(lo, hi) } else { or };
    let mut notes: Vec<NoteEdit> = Vec::new();
    for n in d.notes.iter().take(MAX_NOTES) {
        let ok = t(n.start) && t(n.end) && t(n.new_start) && t(n.new_end);
        if !ok || n.end <= n.start || n.new_end <= n.new_start {
            continue;
        }
        if notes
            .last()
            .is_some_and(|l| n.start < l.end - 1e-6 || n.new_start < l.new_end - 1e-6)
        {
            continue;
        }
        notes.push(NoteEdit {
            start: n.start,
            end: n.end,
            pitch: f(n.pitch, 0.0, 127.0, 0.0),
            onset: if t(n.onset) {
                n.onset.clamp(n.start, n.end)
            } else {
                n.start
            },
            new_start: n.new_start,
            new_end: n.new_end,
            shift: f(n.shift, -36.0, 36.0, 0.0),
            drift: f(n.drift, 0.0, 2.0, 1.0),
            modulation: f(n.modulation, 0.0, 2.0, 1.0),
            formant: f(n.formant, -12.0, 12.0, 0.0),
            gain_db: f(n.gain_db, -60.0, 12.0, 0.0),
        });
    }
    Some(MelodyEdit { algorithm, notes })
}

// ------------------------------------------------------------------ analysis, cached

/// Bumped when the analysis changes, so old cached files are not used.
const ANALYSIS_VERSION: u32 = 1;

type MemoKey = (PathBuf, u64, u64, Algorithm);
type Memo = Mutex<HashMap<MemoKey, (u64, Arc<NoteAnalysis>)>>;

/// Analyses found this run: by file (path, size, modified time) and algorithm, with the
/// file's content hash.
fn memo() -> &'static Memo {
    static M: OnceLock<Memo> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn running() -> &'static Mutex<HashSet<MemoKey>> {
    static R: OnceLock<Mutex<HashSet<MemoKey>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// The made stretches of every recording, so the next edit only makes the changed notes.
fn segments() -> &'static SegmentCache {
    static C: OnceLock<SegmentCache> = OnceLock::new();
    C.get_or_init(SegmentCache::default)
}

fn memo_key(file: &AudioFile, algorithm: Algorithm) -> MemoKey {
    let meta = std::fs::metadata(&file.path).ok();
    let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
    let modified = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    (file.path.clone(), size, modified, algorithm)
}

/// A hash of the file's bytes (FNV-1a over 64-bit words).
pub fn content_hash(path: &Path) -> std::io::Result<u64> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for c in buf[..n].chunks(8) {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            h = (h ^ u64::from_le_bytes(w)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Ok(h)
}

fn analysis_dir() -> PathBuf {
    crate::flex_edit::cache_dir().join("pitch-analysis")
}

fn analysis_file(hash: u64, algorithm: Algorithm) -> PathBuf {
    analysis_dir().join(format!(
        "{hash:016x}-{}-v{ANALYSIS_VERSION}.bin",
        algorithm.name()
    ))
}

fn encode(a: &NoteAnalysis) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"PWNA");
    b.extend_from_slice(&ANALYSIS_VERSION.to_le_bytes());
    b.extend_from_slice(&(a.track.len() as u64).to_le_bytes());
    for v in [&a.track.hz, &a.track.voicing, &a.track.level_db] {
        for x in v.iter() {
            b.extend_from_slice(&x.to_le_bytes());
        }
    }
    b.extend_from_slice(&(a.blobs.len() as u64).to_le_bytes());
    for n in &a.blobs {
        b.extend_from_slice(&n.start.to_le_bytes());
        b.extend_from_slice(&n.end.to_le_bytes());
        b.extend_from_slice(&n.pitch.to_le_bytes());
        b.extend_from_slice(&n.onset.to_le_bytes());
    }
    b
}

fn decode(b: &[u8]) -> Option<NoteAnalysis> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = b.get(at..at + n)?;
        at += n;
        Some(s)
    };
    if take(4)? != b"PWNA" || u32::from_le_bytes(take(4)?.try_into().ok()?) != ANALYSIS_VERSION {
        return None;
    }
    let frames = usize::try_from(u64::from_le_bytes(take(8)?.try_into().ok()?)).ok()?;
    if frames > 10_000_000 {
        return None;
    }
    let mut read_f32s = |n: usize| -> Option<Vec<f32>> {
        let raw = take(n * 4)?;
        Some(
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect(),
        )
    };
    let hz = read_f32s(frames)?;
    let voicing = read_f32s(frames)?;
    let level_db = read_f32s(frames)?;
    let count = usize::try_from(u64::from_le_bytes(take(8)?.try_into().ok()?)).ok()?;
    if count > MAX_NOTES * 4 {
        return None;
    }
    let mut blobs = Vec::with_capacity(count);
    for _ in 0..count {
        let start = f64::from_le_bytes(take(8)?.try_into().ok()?);
        let end = f64::from_le_bytes(take(8)?.try_into().ok()?);
        let pitch = f32::from_le_bytes(take(4)?.try_into().ok()?);
        let onset = f64::from_le_bytes(take(8)?.try_into().ok()?);
        blobs.push(Blob {
            start,
            end,
            pitch,
            onset,
        });
    }
    Some(NoteAnalysis {
        track: PitchTrack {
            hz,
            voicing,
            level_db,
        },
        blobs,
    })
}

/// Removes cached analyses not used in a month (once per run).
fn clean_analyses() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Ok(entries) = std::fs::read_dir(analysis_dir()) else {
            return;
        };
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > 30 * 86_400);
            if old && e.path().extension().is_some_and(|x| x == "bin") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    });
}

/// The notes of `file` found by `algorithm`, with the file's content hash: from this run's
/// memory, the disk cache, or found now (slow: run it off the UI thread).
pub fn analysis_for(
    file: &AudioFile,
    algorithm: Algorithm,
) -> Result<(u64, Arc<NoteAnalysis>), String> {
    let key = memo_key(file, algorithm);
    if let Some(v) = memo().lock().ok().and_then(|m| m.get(&key).cloned()) {
        return Ok(v);
    }
    let hash = content_hash(&file.path).map_err(|e| e.to_string())?;
    let cached = std::fs::read(analysis_file(hash, algorithm))
        .ok()
        .and_then(|b| decode(&b));
    let analysis = match cached {
        Some(a) => a,
        None => {
            clean_analyses();
            let stereo = file.read_stereo().map_err(|e| e.to_string())?;
            let mono: Vec<f32> = stereo.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
            let onsets = if algorithm.pitched() {
                Vec::new()
            } else {
                file.analyze().map_err(|e| e.to_string())?.onsets
            };
            let a = find_notes(&mono, file.sample_rate, algorithm, &onsets);
            let path = analysis_file(hash, algorithm);
            if std::fs::create_dir_all(analysis_dir()).is_ok() {
                let tmp = path.with_extension("part");
                if std::fs::write(&tmp, encode(&a)).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
            a
        }
    };
    let v = (hash, Arc::new(analysis));
    if let Ok(mut m) = memo().lock() {
        m.insert(key, v.clone());
    }
    Ok(v)
}

/// The analysis if it is ready; otherwise starts it in the background (once) and returns
/// `None`. A failed analysis is reported through `error`.
fn analysis_ready(
    file: &AudioFile,
    algorithm: Algorithm,
    ctx: &egui::Context,
    error: &Arc<Mutex<Option<String>>>,
) -> Option<Arc<NoteAnalysis>> {
    let key = memo_key(file, algorithm);
    if let Some(v) = memo().lock().ok().and_then(|m| m.get(&key).cloned()) {
        return Some(v.1);
    }
    let started = running()
        .lock()
        .map(|mut r| r.insert(key.clone()))
        .unwrap_or(false);
    if started {
        let (file, ctx, error) = (file.clone(), ctx.clone(), error.clone());
        let spawned = std::thread::Builder::new()
            .name("prowave-pitch-analysis".into())
            .spawn(move || {
                let result = analysis_for(&file, algorithm);
                if let (Err(e), Ok(mut slot)) = (&result, error.lock()) {
                    *slot = Some(e.clone());
                }
                if let Ok(mut r) = running().lock() {
                    r.remove(&key);
                }
                ctx.request_repaint();
            });
        if spawned.is_err() {
            if let Ok(mut r) = running().lock() {
                r.clear();
            }
        }
    }
    None
}

/// Whether an analysis is running.
#[cfg(test)]
pub fn analysing() -> bool {
    running().lock().is_ok_and(|r| !r.is_empty())
}

// ------------------------------------------------------------------ the sound

/// The recording (interleaved stereo, `input`) with the Pitch Editor's edit made: the changed
/// notes made again, their levels, then their times. Runs on a Flex render thread.
pub fn render(original: &AudioFile, input: Vec<f32>, m: &MelodyEdit) -> Result<Vec<f32>, String> {
    let sr = original.sample_rate;
    let changes: Vec<NoteChange> = m
        .notes
        .iter()
        .map(NoteEdit::change)
        .filter(|c| !c.is_neutral())
        .collect();
    let mut out = if changes.is_empty() {
        input
    } else {
        let (hash, a) = analysis_for(original, m.algorithm)?;
        let (engine, voiced_only) = if m.algorithm.pitched() {
            (note_render::DEFAULT_RESYNTH, true)
        } else {
            (note_render::WHOLE_RESYNTH, false)
        };
        note_render::render_notes_with(
            &input,
            sr,
            &a.track,
            &changes,
            engine,
            voiced_only,
            segments(),
            hash,
        )
        .0
    };
    let gains: Vec<(f64, f64, f32)> = m
        .notes
        .iter()
        .filter(|n| n.gain_db.abs() >= 1e-3)
        .map(|n| (n.start, n.end, n.gain_db))
        .collect();
    note_render::apply_gains(&mut out, sr, &gains);
    let duration = (out.len() / 2) as f64 / f64::from(sr.max(1));
    let points = m.warp_points(duration);
    if !points.is_empty() {
        let mode = match m.algorithm {
            Algorithm::Melodic => FlexMode::Monophonic,
            Algorithm::Percussive => FlexMode::Slicing,
            Algorithm::Universal => FlexMode::Polyphonic,
        };
        let frames = out.len() / 2;
        out = prowave_engine::warp(&out, sr, &points, mode);
        // The same length as the recording, so the region's place in it holds.
        out.resize(frames * 2, 0.0);
    }
    Ok(out)
}

// ------------------------------------------------------------------ editing

/// How a note moved up or down lands (Melodyne's Pitch Grid).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Snap {
    #[default]
    Chromatic,
    Scale,
    Off,
}

impl Snap {
    pub const ALL: [Snap; 3] = [Snap::Chromatic, Snap::Scale, Snap::Off];

    pub fn label(self) -> &'static str {
        match self {
            Snap::Chromatic => tr("Chromatic"),
            Snap::Scale => tr("Scale"),
            Snap::Off => tr("Off"),
        }
    }
}

/// The nearest pitch the grid allows (`fifths`: the project key).
pub fn snap_pitch(m: f32, snap: Snap, fifths: i8) -> f32 {
    match snap {
        Snap::Chromatic => m.round(),
        Snap::Scale => crate::flex_edit::nearest_in_key(m, fifths),
        Snap::Off => m,
    }
}

fn connected(a: &NoteEdit, b: &NoteEdit) -> bool {
    (b.start - a.end).abs() < CONNECTED
}

/// Moves notes `idx` up or down: from `before` (the notes when the drag began) by `delta`
/// semitones, landing on the grid unless `free`. Unpitched notes move by whole semitones.
pub fn move_pitch(
    notes: &mut [NoteEdit],
    before: &[NoteEdit],
    idx: &[usize],
    delta: f32,
    snap: Snap,
    fifths: i8,
    free: bool,
) {
    for &i in idx {
        let (Some(n), Some(b)) = (notes.get_mut(i), before.get(i)) else {
            continue;
        };
        let wanted = b.shift + delta;
        n.shift = if free {
            wanted
        } else if b.pitch > 0.0 {
            snap_pitch(b.pitch + wanted, snap, fifths) - b.pitch
        } else {
            wanted.round()
        }
        .clamp(-36.0, 36.0);
    }
}

/// Moves notes `idx` in time by `delta` seconds from `before`, kept between their unmoved
/// neighbours; a connected neighbour's edge follows. `snap` (where the start may land) is
/// applied to the first selected note and the others keep their distance to it.
pub fn move_time(
    notes: &mut [NoteEdit],
    before: &[NoteEdit],
    idx: &[usize],
    delta: f64,
    snap: Option<&dyn Fn(f64) -> f64>,
    duration: f64,
) {
    let mut sel: Vec<usize> = idx.iter().copied().filter(|i| *i < notes.len()).collect();
    sel.sort_unstable();
    sel.dedup();
    let Some(&first) = sel.first() else { return };
    let mut d = delta;
    if let Some(snap) = snap {
        let anchor = before[first].place(before[first].onset);
        d = snap(anchor + delta) - anchor;
    }
    // The room the unselected neighbours leave.
    let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
    for &i in &sel {
        let b = before[i];
        lo = lo.max(-b.new_start);
        hi = hi.min(duration - b.new_end);
        if i > 0 && !sel.contains(&(i - 1)) {
            let p = before[i - 1];
            lo = lo.max(if connected(&p, &b) {
                p.new_start + MIN_LEN - b.new_start
            } else {
                p.new_end - b.new_start
            });
        }
        if i + 1 < notes.len() && !sel.contains(&(i + 1)) {
            let q = before[i + 1];
            hi = hi.min(if connected(&b, &q) {
                q.new_end - MIN_LEN - b.new_end
            } else {
                q.new_start - b.new_end
            });
        }
    }
    let d = if lo <= hi { d.clamp(lo, hi) } else { 0.0 };
    for &i in &sel {
        notes[i].new_start = before[i].new_start + d;
        notes[i].new_end = before[i].new_end + d;
        if i > 0 && !sel.contains(&(i - 1)) && connected(&before[i - 1], &before[i]) {
            notes[i - 1].new_end = notes[i].new_start;
        }
        if i + 1 < notes.len() && !sel.contains(&(i + 1)) && connected(&before[i], &before[i + 1]) {
            notes[i + 1].new_start = notes[i].new_end;
        }
    }
}

/// Which edge of a note is dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

/// Stretches note `i` so its `edge` plays at `to` (seconds), from `before`: the note at least
/// [`MIN_LEN`] long, not past its neighbour, a connected neighbour's edge following.
pub fn stretch(
    notes: &mut [NoteEdit],
    before: &[NoteEdit],
    i: usize,
    edge: Edge,
    to: f64,
    duration: f64,
) {
    let Some(b) = before.get(i).copied() else {
        return;
    };
    match edge {
        Edge::Start => {
            let mut lo = 0.0f64;
            if i > 0 {
                let p = before[i - 1];
                lo = if connected(&p, &b) {
                    p.new_start + MIN_LEN
                } else {
                    p.new_end
                };
            }
            let t = to.clamp(lo, b.new_end - MIN_LEN);
            notes[i].new_start = t;
            if i > 0 && connected(&before[i - 1], &b) {
                notes[i - 1].new_end = t;
            }
        }
        Edge::End => {
            let mut hi = duration;
            if let Some(q) = before.get(i + 1) {
                hi = if connected(&b, q) {
                    q.new_end - MIN_LEN
                } else {
                    q.new_start
                };
            }
            let t = to.clamp(b.new_start + MIN_LEN, hi);
            notes[i].new_end = t;
            if i + 1 < notes.len() && connected(&b, &before[i + 1]) {
                notes[i + 1].new_start = t;
            }
        }
    }
}

/// Splits note `i` at moment `at` of the recording (the Note Separation Tool). Both halves keep
/// its changes; each gets its own pitch centre. Returns whether it split.
pub fn split(notes: &mut Vec<NoteEdit>, i: usize, at: f64, track: Option<&PitchTrack>) -> bool {
    let Some(n) = notes.get(i).copied() else {
        return false;
    };
    if at <= n.start + MIN_LEN || at >= n.end - MIN_LEN {
        return false;
    }
    let mid = n.place(at);
    let centre = |a: f64, b: f64| {
        track
            .filter(|_| n.pitch > 0.0)
            .and_then(|t| pitch_centre(t, PitchTrack::frame_at(a), PitchTrack::frame_at(b)))
            .unwrap_or(n.pitch)
    };
    let left = NoteEdit {
        end: at,
        new_end: mid,
        pitch: centre(n.start, at),
        onset: n.onset.min(at),
        ..n
    };
    let right = NoteEdit {
        start: at,
        new_start: mid,
        pitch: centre(at, n.end),
        onset: at.max(n.onset),
        ..n
    };
    notes[i] = left;
    notes.insert(i + 1, right);
    true
}

/// Joins note `i` with the next one (a double-click on the separation between them). The
/// joined note takes the first one's changes. Returns whether it joined.
pub fn join(notes: &mut Vec<NoteEdit>, i: usize, track: Option<&PitchTrack>) -> bool {
    if i + 1 >= notes.len() {
        return false;
    }
    let (a, b) = (notes[i], notes[i + 1]);
    let pitch = if a.pitch > 0.0 {
        track
            .and_then(|t| {
                pitch_centre(
                    t,
                    PitchTrack::frame_at(a.start),
                    PitchTrack::frame_at(b.end),
                )
            })
            .unwrap_or((a.pitch + b.pitch) / 2.0)
    } else {
        0.0
    };
    // The first note's shift is kept for the note it was heard as.
    let shift = a.pitch + a.shift - pitch;
    notes[i] = NoteEdit {
        end: b.end,
        new_end: b.new_end,
        pitch,
        shift: if a.pitch > 0.0 { shift } else { a.shift },
        ..a
    };
    notes.remove(i + 1);
    true
}

/// The Correct Pitch macro: each note's centre moves `centre` (0–1) of the way to the nearest
/// grid note, and its drift is reduced by `drift` (0–1). Unpitched notes are left alone.
pub fn correct_pitch(
    notes: &mut [NoteEdit],
    idx: &[usize],
    centre: f32,
    drift: f32,
    snap: Snap,
    fifths: i8,
) {
    let snap = if snap == Snap::Off {
        Snap::Chromatic
    } else {
        snap
    };
    for &i in idx {
        let Some(n) = notes.get_mut(i) else { continue };
        if n.pitch <= 0.0 {
            continue;
        }
        let now = n.heard();
        let target = snap_pitch(now, snap, fifths);
        n.shift += (target - now) * centre.clamp(0.0, 1.0);
        n.drift = (n.drift * (1.0 - drift.clamp(0.0, 1.0))).clamp(0.0, 2.0);
    }
}

/// The Quantize Time macro: each note's musical start moves `strength` (0–1) of the way to
/// the nearest grid line (`grid` gives it for a moment of the recording), neighbours as in
/// [`move_time`].
pub fn quantize_time(
    notes: &mut [NoteEdit],
    idx: &[usize],
    grid: &dyn Fn(f64) -> f64,
    strength: f64,
    duration: f64,
) {
    let mut sel: Vec<usize> = idx.to_vec();
    sel.sort_unstable();
    for i in sel {
        let before = notes.to_vec();
        let Some(n) = before.get(i) else { continue };
        let at = n.place(n.onset);
        let d = (grid(at) - at) * strength.clamp(0.0, 1.0);
        move_time(notes, &before, &[i], d, None, duration);
    }
}

/// Changes a value of notes `idx` from `before` by a drag of `dy` points up.
fn drag_value(
    notes: &mut [NoteEdit],
    before: &[NoteEdit],
    idx: &[usize],
    tool: Tool,
    dy: f32,
    row_h: f32,
) {
    for &i in idx {
        let (Some(n), Some(b)) = (notes.get_mut(i), before.get(i)) else {
            continue;
        };
        match tool {
            Tool::Modulation => n.modulation = (b.modulation + dy / 100.0).clamp(0.0, 2.0),
            Tool::Drift => n.drift = (b.drift + dy / 100.0).clamp(0.0, 2.0),
            Tool::Formant => n.formant = (b.formant + dy / row_h.max(1.0)).clamp(-12.0, 12.0),
            Tool::Amplitude => n.gain_db = (b.gain_db + dy / 4.0).clamp(-60.0, 12.0),
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ the view

/// The Pitch Editor's tools (Melodyne's toolbar).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Main,
    Pitch,
    Modulation,
    Drift,
    Formant,
    Amplitude,
    Timing,
    Separation,
}

impl Tool {
    pub const ALL: [Tool; 8] = [
        Tool::Main,
        Tool::Pitch,
        Tool::Modulation,
        Tool::Drift,
        Tool::Formant,
        Tool::Amplitude,
        Tool::Timing,
        Tool::Separation,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Main => tr("Main"),
            Tool::Pitch => tr("Pitch"),
            Tool::Modulation => tr("Modulation"),
            Tool::Drift => tr("Drift"),
            Tool::Formant => tr("Formant"),
            Tool::Amplitude => tr("Amplitude"),
            Tool::Timing => tr("Timing"),
            Tool::Separation => tr("Separation"),
        }
    }
}

fn algorithm_label(a: Algorithm) -> &'static str {
    match a {
        Algorithm::Melodic => tr("Melodic"),
        Algorithm::Percussive => tr("Percussive"),
        Algorithm::Universal => tr("Universal"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragKind {
    /// The Main Tool's middle: the axis is decided by the first movement.
    Main,
    Pitch,
    Time,
    Edge(Edge),
    Value(Tool),
}

#[derive(Clone, Debug)]
struct Drag {
    kind: DragKind,
    from: Pos2,
    before: Vec<NoteEdit>,
    note: usize,
}

/// What the Pitch Editor shows and how.
pub struct PitchView {
    /// The region it edits (track, region), set when it is opened on one.
    pub region: Option<(usize, usize)>,
    pub tool: Tool,
    pub snap: Snap,
    /// The algorithm for a region not edited yet.
    pub algorithm: Algorithm,
    pub selected: Vec<usize>,
    /// Correct Pitch: centre and drift, 0–1.
    pub centre: f32,
    pub drift: f32,
    /// Quantize Time: the grid (beats) and the strength, 0–1.
    pub grid: f64,
    pub strength: f32,
    /// Points per second, the recording's time at the left edge, points per semitone, the
    /// pitch at the top edge.
    pub zoom: f32,
    pub scroll: f64,
    pub row_h: f32,
    pub top: f32,
    /// The notes while a drag is under way (drawn instead of the region's).
    preview: Option<Vec<NoteEdit>>,
    drag: Option<Drag>,
    /// The region the view was last fitted to.
    fitted: Option<(usize, usize, PathBuf)>,
    error: Arc<Mutex<Option<String>>>,
    /// Where the notes were last drawn.
    pub plot: Rect,
}

impl Default for PitchView {
    fn default() -> Self {
        Self {
            region: None,
            tool: Tool::Main,
            snap: Snap::Chromatic,
            algorithm: Algorithm::Melodic,
            selected: Vec::new(),
            centre: 1.0,
            drift: 0.5,
            grid: 0.25,
            strength: 1.0,
            zoom: 120.0,
            scroll: 0.0,
            row_h: 12.0,
            top: 76.0,
            preview: None,
            drag: None,
            fitted: None,
            error: Arc::default(),
            plot: Rect::NOTHING,
        }
    }
}

/// Opens the Pitch Editor on region `r` of track `t` (a double-click on an audio region).
pub fn open(s: &mut UiState, t: usize, r: usize) {
    open_in(&mut s.pitch_ed, &mut s.bottom, t, r);
}

/// [`open`], for callers that hold other parts of the UI state.
pub fn open_in(v: &mut PitchView, bottom: &mut Bottom, t: usize, r: usize) {
    v.region = Some((t, r));
    v.selected.clear();
    v.drag = None;
    v.preview = None;
    *bottom = Bottom::PitchEditor;
}

/// The audio region the editor shows: the one it was opened on, else the selected one.
pub fn target(p: &Project, s: &UiState) -> Option<(usize, usize)> {
    let audio = |(t, r): (usize, usize)| {
        p.tracks.get(t).is_some_and(|tr| {
            tr.kind == TrackKind::Audio && tr.regions.get(r).is_some_and(|x| x.audio.is_some())
        })
    };
    s.pitch_ed
        .region
        .filter(|x| audio(*x))
        .or(s.tracks.selected.filter(|x| audio(*x)))
}

/// The notes of region (t, r) as the editor edits them: its edit's, or the ones found.
fn notes_of(
    p: &Project,
    t: usize,
    r: usize,
    algorithm: Algorithm,
    found: &NoteAnalysis,
) -> Vec<NoteEdit> {
    let edit = p.tracks[t].regions[r]
        .audio
        .as_ref()
        .and_then(|a| a.flex.as_ref())
        .and_then(|f| f.melody.as_ref())
        .filter(|m| m.algorithm == algorithm);
    match edit {
        Some(m) => m.notes.clone(),
        None => found.blobs.iter().map(NoteEdit::from_blob).collect(),
    }
}

/// Puts `notes` on region (t, r) as its Pitch Editor edit.
pub fn commit(p: &mut Project, t: usize, r: usize, algorithm: Algorithm, notes: Vec<NoteEdit>) {
    let has_flex = p.tracks[t].regions[r]
        .audio
        .as_ref()
        .is_some_and(|a| a.flex.is_some());
    if !has_flex {
        crate::flex_edit::set_mode(p, t, r, FlexMode::Monophonic);
    }
    if let Some(f) = p.tracks[t].regions[r]
        .audio
        .as_mut()
        .and_then(|a| a.flex.as_mut())
    {
        let m = MelodyEdit { algorithm, notes };
        if f.melody.as_ref() != Some(&m) {
            f.melody = Some(m);
            p.song_rev += 1;
        }
    }
}

/// Seconds of the recording ↔ the project's beats for a region.
struct Clock {
    map: prowave_engine::TempoMap,
    /// The project's seconds at the region's start, and where the region starts in its file.
    s0: f64,
    offset: f64,
}

impl Clock {
    fn beat(&self, t: f64) -> f64 {
        self.map.beat_at(self.s0 + t - self.offset)
    }

    fn secs(&self, beat: f64) -> f64 {
        self.map.seconds_at(beat) - self.s0 + self.offset
    }
}

const BG: Color32 = Color32::from_gray(0x24);
const ROW_IN: Color32 = Color32::from_gray(0x2e);
const ROW_OUT: Color32 = Color32::from_gray(0x27);
const TEXT: Color32 = Color32::from_gray(0xdd);
const DIM: Color32 = Color32::from_gray(0x9a);
const BLOB: Color32 = Color32::from_rgb(0xd8, 0x8f, 0x32);
const BLOB_SEL: Color32 = Color32::from_rgb(0xff, 0xc4, 0x5c);
const CURVE: Color32 = Color32::from_rgb(0x3a, 0x22, 0x08);
const RULER_W: f32 = 46.0;
const TIME_H: f32 = 18.0;
const STATUS_H: f32 = 20.0;

fn note_name(m: f32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let r = m.round() as i32;
    format!(
        "{}{}",
        NAMES[r.rem_euclid(12) as usize],
        r.div_euclid(12) - 2
    )
}

/// Draws the Pitch Editor in `ui` (the editor pane).
pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState) {
    let area = ui.available_rect_before_wrap();
    ui.painter().rect_filled(area, 0.0, BG);
    let Some((t, r)) = target(p, s) else {
        ui.allocate_rect(area, Sense::hover());
        ui.painter().text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            tr("Double-click an audio region to edit its notes"),
            FontId::proportional(14.0),
            TEXT,
        );
        return;
    };
    let Some(audio) = p.tracks[t].regions[r].audio.as_ref() else {
        return;
    };
    let file = crate::flex_edit::recording_of(audio);
    let algorithm = audio
        .flex
        .as_ref()
        .and_then(|f| f.melody.as_ref())
        .map_or(s.pitch_ed.algorithm, |m| m.algorithm);
    let fifths = crate::score::fifths(p.key);
    let duration = file.duration_secs();

    // Row 1: name, algorithm, tools, pitch grid.
    let mut new_algorithm = algorithm;
    let v = &mut s.pitch_ed;
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tr("Pitch Editor")).strong().color(TEXT));
        ui.label(egui::RichText::new(&p.tracks[t].regions[r].name).color(DIM));
        ui.separator();
        egui::ComboBox::from_id_salt("pitch-algorithm")
            .selected_text(algorithm_label(algorithm))
            .show_ui(ui, |ui| {
                for a in Algorithm::ALL {
                    ui.selectable_value(&mut new_algorithm, a, algorithm_label(a));
                }
                ui.add_enabled(false, egui::Button::new(tr("Polyphonic (not available)")));
            });
        ui.separator();
        for tool in Tool::ALL {
            if ui.selectable_label(v.tool == tool, tool.label()).clicked() {
                v.tool = tool;
            }
        }
        ui.separator();
        ui.label(egui::RichText::new(tr("Pitch Grid")).color(DIM));
        egui::ComboBox::from_id_salt("pitch-snap")
            .selected_text(v.snap.label())
            .show_ui(ui, |ui| {
                for sn in Snap::ALL {
                    ui.selectable_value(&mut v.snap, sn, sn.label());
                }
            });
    });
    // Row 2: the macros.
    let mut correct = false;
    let mut quantize = false;
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(tr("Correct Pitch"))
                .strong()
                .color(TEXT),
        );
        ui.label(egui::RichText::new(tr("Centre")).color(DIM));
        ui.add(
            egui::Slider::new(&mut v.centre, 0.0..=1.0)
                .custom_formatter(|x, _| format!("{:.0} %", x * 100.0)),
        );
        ui.label(egui::RichText::new(tr("Drift")).color(DIM));
        ui.add(
            egui::Slider::new(&mut v.drift, 0.0..=1.0)
                .custom_formatter(|x, _| format!("{:.0} %", x * 100.0)),
        );
        correct = ui.button(tr("Apply")).clicked();
        ui.separator();
        ui.label(
            egui::RichText::new(tr("Quantize Time"))
                .strong()
                .color(TEXT),
        );
        let grid_name = crate::note_edit::GRIDS
            .iter()
            .find(|(_, g)| (f64::from(*g) - v.grid).abs() < 1e-6)
            .map_or("1/16 Note", |(n, _)| n);
        egui::ComboBox::from_id_salt("pitch-grid")
            .selected_text(tr(grid_name))
            .show_ui(ui, |ui| {
                for (name, g) in crate::note_edit::GRIDS {
                    ui.selectable_value(&mut v.grid, f64::from(g), tr(name));
                }
            });
        ui.label(egui::RichText::new(tr("Strength")).color(DIM));
        ui.add(
            egui::Slider::new(&mut v.strength, 0.0..=1.0)
                .custom_formatter(|x, _| format!("{:.0} %", x * 100.0)),
        );
        quantize = ui.button(tr("Apply")).clicked();
    });

    if new_algorithm != algorithm {
        v.algorithm = new_algorithm;
        v.selected.clear();
        // Notes found another way are other notes: the edit starts again.
        if let Some(f) = p.tracks[t].regions[r]
            .audio
            .as_mut()
            .and_then(|a| a.flex.as_mut())
        {
            if f.melody.take().is_some() {
                p.song_rev += 1;
            }
        }
        return;
    }

    let canvas = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(canvas, Sense::click_and_drag());
    let g = ui.painter_at(canvas);
    let Some(found) = analysis_ready(&file, algorithm, ui.ctx(), &s.pitch_ed.error) else {
        let msg = s
            .pitch_ed
            .error
            .lock()
            .ok()
            .and_then(|e| e.clone())
            .map_or_else(
                || tr("Analysing…").to_string(),
                |e| trf("Pitch Editor: {}", &[&e]),
            );
        g.text(
            canvas.center(),
            egui::Align2::CENTER_CENTER,
            msg,
            FontId::proportional(14.0),
            TEXT,
        );
        return;
    };
    let notes = notes_of(p, t, r, algorithm, &found);
    let region = &p.tracks[t].regions[r];
    let clock = Clock {
        map: p.tempo_map(),
        s0: p.tempo_map().seconds_at(p.region_start_beats(region)),
        offset: region.params.offset_secs,
    };
    let region_secs = clock.map.seconds_at(p.region_end_beats(region)) - clock.s0;
    let v = &mut s.pitch_ed;
    let plot = Rect::from_min_max(
        pos2(canvas.left() + RULER_W, canvas.top() + TIME_H),
        pos2(canvas.right(), canvas.bottom() - STATUS_H),
    );
    v.plot = plot;
    // Fit the view to the region the first time it is shown.
    let key = (t, r, file.path.clone());
    if v.fitted.as_ref() != Some(&key) && plot.width() > 10.0 {
        v.fitted = Some(key);
        let len = region_secs.clamp(0.5, duration.max(0.5));
        v.scroll = clock.offset;
        v.zoom = (plot.width() / len as f32).max(1.0);
        // The notes the region shows set the pitch range.
        let (a, b) = (clock.offset, clock.offset + len);
        let pitched: Vec<f32> = notes
            .iter()
            .filter(|n| n.pitch > 0.0 && n.new_end > a && n.new_start < b)
            .map(NoteEdit::heard)
            .collect();
        let (lo, hi) = if pitched.is_empty() {
            (54.0, 66.0)
        } else {
            (
                pitched.iter().copied().fold(f32::MAX, f32::min),
                pitched.iter().copied().fold(f32::MIN, f32::max),
            )
        };
        let span = (hi - lo + 6.0).max(12.0);
        v.row_h = (plot.height() / span).clamp(4.0, 28.0);
        v.top = (hi + lo) / 2.0 + plot.height() / v.row_h / 2.0;
    }
    // Scrolling and zooming: the wheel moves, Command and the wheel zoom the time.
    if resp.hovered() {
        let (scroll, zoom_key) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.command));
        if zoom_key && scroll.y != 0.0 {
            v.zoom = (v.zoom * (1.0 + scroll.y * 0.003)).clamp(2.0, 4000.0);
        } else {
            v.scroll = (v.scroll - f64::from(scroll.x / v.zoom)).max(0.0);
            v.top = (v.top + scroll.y / v.row_h).clamp(12.0, 127.0);
        }
    }
    let (scroll, zoom) = (v.scroll, v.zoom);
    let x_of = |sec: f64| plot.left() + ((sec - scroll) as f32) * zoom;
    let sec_of = |x: f32| scroll + f64::from((x - plot.left()) / zoom);
    let (top, row_h) = (v.top, v.row_h);
    let y_of = |m: f32| plot.top() + (top - m) * row_h;
    let m_of = |y: f32| top - (y - plot.top()) / row_h;
    let pitched = algorithm.pitched();
    // Unpitched notes sit on one line, moved up and down from it.
    let line = (top - (plot.height() / row_h) / 2.0).round();
    let row_of = |n: &NoteEdit| if pitched { n.heard() } else { line + n.shift };

    // The pitch grid: a row per semitone, the key's notes lighter.
    let alt = crate::score::alterations(fifths);
    const NATURAL: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    let pcs: Vec<i32> = (0..7)
        .map(|l| (NATURAL[l] + i32::from(alt[l])).rem_euclid(12))
        .collect();
    let lo_m = m_of(plot.bottom()).floor() as i32;
    let hi_m = top.ceil() as i32;
    for m in lo_m..=hi_m {
        let y0 = y_of(m as f32 + 0.5);
        let row = Rect::from_min_max(pos2(plot.left(), y0), pos2(plot.right(), y0 + row_h));
        // Out of the key (Scale), or a black key (otherwise), a little darker.
        const WHITE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
        let in_key = !pitched
            || if v.snap == Snap::Scale {
                pcs.contains(&m.rem_euclid(12))
            } else {
                WHITE.contains(&m.rem_euclid(12))
            };
        g.rect_filled(
            row.intersect(plot),
            0.0,
            if in_key { ROW_IN } else { ROW_OUT },
        );
        if m.rem_euclid(12) == 0 && pitched {
            g.line_segment(
                [
                    pos2(plot.left(), y0 + row_h),
                    pos2(plot.right(), y0 + row_h),
                ],
                Stroke::new(1.0, Color32::from_gray(0x44)),
            );
        }
        let label = if pitched {
            note_name(m as f32)
        } else {
            format!("{:+}", m - line as i32)
        };
        if row_h >= 9.0
            && y_of(m as f32) > plot.top() + 4.0
            && (pitched || (m - line as i32) % 2 == 0)
        {
            g.text(
                pos2(canvas.left() + 4.0, y_of(m as f32)),
                egui::Align2::LEFT_CENTER,
                label,
                FontId::proportional((row_h * 0.75).min(11.0)),
                if m.rem_euclid(12) == 0 || !pitched {
                    TEXT
                } else {
                    DIM
                },
            );
        }
    }
    // The beats and bars of the project.
    let (b0, b1) = (
        clock.beat(sec_of(plot.left())).floor(),
        clock.beat(sec_of(plot.right())).ceil(),
    );
    if b1 - b0 < 2000.0 {
        let mut b = b0;
        while b <= b1 {
            let x = x_of(clock.secs(b));
            let bar_no = p.beats_to_bar(b);
            let bar = (bar_no - bar_no.round()).abs() < 1e-6;
            g.line_segment(
                [pos2(x, plot.top()), pos2(x, plot.bottom())],
                Stroke::new(1.0, Color32::from_gray(if bar { 0x50 } else { 0x36 })),
            );
            if bar {
                g.text(
                    pos2(x + 3.0, canvas.top() + TIME_H / 2.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{}", bar_no.round() as i64 + 1),
                    FontId::proportional(11.0),
                    TEXT,
                );
            }
            b += 1.0;
        }
    }
    // The region's edges.
    for sec in [clock.offset, clock.offset + region_secs] {
        let x = x_of(sec);
        g.line_segment(
            [pos2(x, plot.top()), pos2(x, plot.bottom())],
            Stroke::new(1.5, Color32::from_rgb(0x5a, 0x8f, 0xd8)),
        );
    }

    // The interaction.
    let shown: Vec<NoteEdit> = v.preview.clone().unwrap_or_else(|| notes.clone());
    let hit = |pt: Pos2, list: &[NoteEdit]| -> Option<usize> {
        list.iter().position(|n| {
            let (x0, x1) = (x_of(n.new_start), x_of(n.new_end));
            let y = y_of(row_of(n));
            pt.x >= x0 && pt.x <= x1 && (pt.y - y).abs() <= row_h.max(8.0) * 0.8
        })
    };
    let modifiers = ui.input(|i| i.modifiers);
    let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
    let mut commit_notes: Option<Vec<NoteEdit>> = None;
    let zone = |n: &NoteEdit, x: f32| -> Option<Edge> {
        let (x0, x1) = (x_of(n.new_start), x_of(n.new_end));
        let w = ((x1 - x0) * 0.2).clamp(3.0, 8.0);
        if x - x0 < w {
            Some(Edge::Start)
        } else if x1 - x < w {
            Some(Edge::End)
        } else {
            None
        }
    };
    if let (Some(pt), true) = (pointer, resp.hovered() && v.drag.is_none()) {
        if let Some(i) = hit(pt, &shown) {
            let edgeable = matches!(v.tool, Tool::Main | Tool::Timing);
            ui.ctx()
                .set_cursor_icon(match (edgeable, zone(&shown[i], pt.x)) {
                    (true, Some(_)) => egui::CursorIcon::ResizeHorizontal,
                    _ if v.tool == Tool::Separation => egui::CursorIcon::Crosshair,
                    _ => egui::CursorIcon::Grab,
                });
        }
    }
    if resp.double_clicked() {
        if let Some(pt) = pointer {
            let mut list = notes.clone();
            match (v.tool, hit(pt, &list)) {
                (Tool::Main | Tool::Pitch, Some(i)) => {
                    let n = &mut list[i];
                    if n.pitch > 0.0 {
                        let snap = if v.snap == Snap::Off {
                            Snap::Chromatic
                        } else {
                            v.snap
                        };
                        n.shift = snap_pitch(n.heard(), snap, fifths) - n.pitch;
                    } else {
                        n.shift = n.shift.round();
                    }
                    commit_notes = Some(list);
                }
                (Tool::Modulation, Some(i)) => {
                    list[i].modulation = if list[i].modulation == 0.0 { 1.0 } else { 0.0 };
                    commit_notes = Some(list);
                }
                (Tool::Drift, Some(i)) => {
                    list[i].drift = if list[i].drift == 0.0 { 1.0 } else { 0.0 };
                    commit_notes = Some(list);
                }
                (Tool::Formant, Some(i)) => {
                    list[i].formant = 0.0;
                    commit_notes = Some(list);
                }
                (Tool::Amplitude, Some(i)) => {
                    list[i].gain_db = 0.0;
                    commit_notes = Some(list);
                }
                (Tool::Separation, _) => {
                    // On the separation between two notes: they become one.
                    let near = (0..list.len().saturating_sub(1)).find(|k| {
                        (x_of(list[*k].new_end) - pt.x).abs() < 5.0
                            && (x_of(list[k + 1].new_start) - pt.x).abs() < 5.0
                    });
                    if let Some(k) = near {
                        if join(&mut list, k, Some(&found.track)) {
                            v.selected.clear();
                            commit_notes = Some(list);
                        }
                    }
                }
                _ => {}
            }
        }
    } else if resp.clicked() {
        if let Some(pt) = pointer {
            match hit(pt, &notes) {
                Some(i) if v.tool == Tool::Separation => {
                    let mut list = notes.clone();
                    let n = list[i];
                    // The moment of the recording under the pointer.
                    let at = n.start
                        + (sec_of(pt.x) - n.new_start) * (n.end - n.start)
                            / (n.new_end - n.new_start).max(1e-9);
                    if split(&mut list, i, at, Some(&found.track)) {
                        v.selected = vec![i + 1];
                        commit_notes = Some(list);
                    }
                }
                Some(i) => {
                    if modifiers.shift || modifiers.command {
                        if let Some(k) = v.selected.iter().position(|x| *x == i) {
                            v.selected.remove(k);
                        } else {
                            v.selected.push(i);
                        }
                    } else {
                        v.selected = vec![i];
                    }
                }
                None => v.selected.clear(),
            }
        }
    }
    if resp.drag_started() {
        if let Some(pt) = resp.interact_pointer_pos() {
            if let Some(i) = hit(pt, &notes) {
                if !v.selected.contains(&i) {
                    v.selected = vec![i];
                }
                let kind = match v.tool {
                    Tool::Main => zone(&notes[i], pt.x).map_or(DragKind::Main, DragKind::Edge),
                    Tool::Timing => zone(&notes[i], pt.x).map_or(DragKind::Time, DragKind::Edge),
                    Tool::Pitch => DragKind::Pitch,
                    Tool::Separation => DragKind::Value(Tool::Separation),
                    other => DragKind::Value(other),
                };
                v.drag = Some(Drag {
                    kind,
                    from: pt,
                    before: notes.clone(),
                    note: i,
                });
            }
        }
    }
    if let (Some(d), Some(pt)) = (v.drag.as_mut(), resp.interact_pointer_pos()) {
        let delta = pt - d.from;
        if d.kind == DragKind::Main && delta.length() > 4.0 {
            d.kind = if delta.y.abs() > delta.x.abs() {
                DragKind::Pitch
            } else {
                DragKind::Time
            };
        }
        let mut list = d.before.clone();
        let free = modifiers.alt;
        let grid = v.grid;
        let snap_time = |sec: f64| clock.secs((clock.beat(sec) / grid).round() * grid);
        match d.kind {
            DragKind::Main | DragKind::Value(Tool::Separation) => {}
            DragKind::Pitch => {
                move_pitch(
                    &mut list,
                    &d.before,
                    &v.selected,
                    -delta.y / row_h,
                    v.snap,
                    fifths,
                    free,
                );
            }
            DragKind::Time => {
                let snap: Option<&dyn Fn(f64) -> f64> = if free { None } else { Some(&snap_time) };
                move_time(
                    &mut list,
                    &d.before,
                    &v.selected,
                    f64::from(delta.x / zoom),
                    snap,
                    duration,
                );
            }
            DragKind::Edge(edge) => {
                let n = d.before[d.note];
                let at = match edge {
                    Edge::Start => n.new_start,
                    Edge::End => n.new_end,
                } + f64::from(delta.x / zoom);
                let at = if free { at } else { snap_time(at) };
                stretch(&mut list, &d.before, d.note, edge, at, duration);
            }
            DragKind::Value(tool) => {
                drag_value(&mut list, &d.before, &v.selected, tool, -delta.y, row_h);
            }
        }
        v.preview = Some(list);
    }
    if resp.drag_stopped() {
        if let Some(list) = v.preview.take() {
            if list != notes {
                commit_notes = Some(list);
            }
        }
        v.drag = None;
    }
    let target_notes: Vec<usize> = if v.selected.is_empty() {
        (0..notes.len()).collect()
    } else {
        v.selected.clone()
    };
    if correct && pitched {
        let mut list = notes.clone();
        correct_pitch(&mut list, &target_notes, v.centre, v.drift, v.snap, fifths);
        commit_notes = Some(list);
    }
    if quantize {
        let mut list = notes.clone();
        let grid = v.grid;
        let to_grid = |sec: f64| clock.secs((clock.beat(sec) / grid).round() * grid);
        quantize_time(
            &mut list,
            &target_notes,
            &to_grid,
            f64::from(v.strength),
            duration,
        );
        commit_notes = Some(list);
    }
    v.selected.retain(|i| *i < notes.len() + 1);

    // The notes.
    let shown: Vec<NoteEdit> = v.preview.clone().unwrap_or(notes);
    let track = &found.track;
    let changes: Vec<NoteChange> = shown.iter().map(NoteEdit::change).collect();
    let curve = note_render::targets_with(track, &changes, pitched);
    for (i, n) in shown.iter().enumerate() {
        let (x0, x1) = (x_of(n.new_start), x_of(n.new_end));
        if x1 < plot.left() || x0 > plot.right() {
            continue;
        }
        let sel = v.selected.contains(&i);
        let yc = y_of(row_of(n));
        let (fa, fb) = (
            PitchTrack::frame_at(n.start),
            PitchTrack::frame_at(n.end).min(track.len()),
        );
        // The blob: as thick as the note is loud (Melodyne's amplitude envelope).
        let gain = n.gain_db;
        let half = |f: usize| -> f32 {
            let db = track.level_db.get(f).copied().unwrap_or(-60.0) + gain;
            ((db + 50.0) / 50.0).clamp(0.08, 1.0) * row_h.max(6.0) * 0.55
        };
        let step = ((fb - fa) / 200).max(1);
        let pts: Vec<(f32, f32)> = (fa..fb)
            .step_by(step)
            .map(|f| (x_of(n.place(PitchTrack::time_of(f))), half(f)))
            .collect();
        let fill = if sel { BLOB_SEL } else { BLOB };
        if pts.len() >= 2 {
            let mut poly: Vec<Pos2> = pts.iter().map(|(x, h)| pos2(*x, yc - h)).collect();
            poly.extend(pts.iter().rev().map(|(x, h)| pos2(*x, yc + h)));
            // One mesh (a strip of triangles), so no seams show between its pieces.
            let mut mesh = egui::epaint::Mesh::default();
            for (x, h) in &pts {
                mesh.colored_vertex(pos2(*x, yc - h), fill);
                mesh.colored_vertex(pos2(*x, yc + h), fill);
            }
            for k in 0..pts.len() as u32 - 1 {
                let a = 2 * k;
                mesh.add_triangle(a, a + 1, a + 2);
                mesh.add_triangle(a + 1, a + 3, a + 2);
            }
            g.add(Shape::mesh(mesh));
            g.add(Shape::closed_line(
                poly,
                Stroke::new(
                    if sel { 1.5 } else { 1.0 },
                    Color32::from_rgb(0x7a, 0x4a, 0x12),
                ),
            ));
        } else {
            g.rect_filled(
                Rect::from_min_max(pos2(x0, yc - 3.0), pos2(x1.max(x0 + 2.0), yc + 3.0)),
                2.0,
                fill,
            );
        }
        // The pitch curve through it, as it will sound.
        if pitched {
            let line: Vec<Pos2> = (fa..fb)
                .filter_map(|f| {
                    let m = track.midi(f)?;
                    let ratio = curve.ratio.get(f).copied().unwrap_or(1.0);
                    let m = m + 12.0 * ratio.log2();
                    Some(pos2(x_of(n.place(PitchTrack::time_of(f))), y_of(m)))
                })
                .collect();
            for w in line.windows(2) {
                if (w[1].x - w[0].x).abs() < 12.0 {
                    g.line_segment([w[0], w[1]], Stroke::new(1.5, CURVE));
                }
            }
        }
        // The formant beam (Formant Tool) and the level (Amplitude Tool).
        if v.tool == Tool::Formant || n.formant.abs() > 0.01 {
            let yb = yc - row_h.max(6.0) * 0.9 - n.formant * row_h * 0.25;
            g.line_segment(
                [pos2(x0, yb), pos2(x1, yb)],
                Stroke::new(2.0, Color32::from_rgb(0x8f, 0xd0, 0xff)),
            );
        }
        if v.tool == Tool::Amplitude && sel {
            g.text(
                pos2(x0, yc - row_h - 4.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:+.1} dB", n.gain_db),
                FontId::proportional(11.0),
                TEXT,
            );
        }
    }
    // The playhead.
    let ph = clock.secs(p.position_beats);
    if ph >= scroll {
        let x = x_of(ph);
        g.line_segment(
            [pos2(x, plot.top()), pos2(x, plot.bottom())],
            Stroke::new(1.0, Color32::WHITE),
        );
    }
    // The status line: what is selected.
    let status = Rect::from_min_max(pos2(canvas.left(), plot.bottom()), canvas.max);
    g.rect_filled(status, 0.0, Color32::from_gray(0x1c));
    let text = match v.selected.as_slice() {
        [] => trf("{} notes", &[&shown.len()]),
        [i] => match shown.get(*i) {
            Some(n) => {
                let pitch = if n.pitch > 0.0 {
                    let cents = ((n.heard() - n.heard().round()) * 100.0).round();
                    format!("{} {cents:+.0} ct", note_name(n.heard()))
                } else {
                    format!("{:+.2} st", n.shift)
                };
                trf(
                    "{}  ·  drift {} %  ·  modulation {} %  ·  formant {} st  ·  {} dB",
                    &[
                        &pitch,
                        &format!("{:.0}", n.drift * 100.0),
                        &format!("{:.0}", n.modulation * 100.0),
                        &format!("{:+.1}", n.formant),
                        &format!("{:+.1}", n.gain_db),
                    ],
                )
            }
            None => String::new(),
        },
        many => trf("{} notes selected", &[&many.len()]),
    };
    let busy = if s.flex.busy() {
        format!("  ·  {}", tr("rendering…"))
    } else {
        String::new()
    };
    g.text(
        pos2(status.left() + 8.0, status.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{text}{busy}"),
        FontId::proportional(11.0),
        TEXT,
    );
    if let Some(list) = commit_notes {
        commit(p, t, r, algorithm, list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flex_edit::tests::write_notes;

    fn project_with(path: &Path) -> (Project, usize) {
        let mut p = Project::empty();
        p.tempo = 120.0;
        crate::import::import_audio(&mut p, path, &egui::Context::default()).unwrap();
        let t = p.tracks.len() - 1;
        p.tracks[t].regions[0].start_bar = 1.0;
        (p, t)
    }

    /// Polls the Flex renders until nothing is running.
    fn settle(p: &mut Project, s: &mut UiState) {
        let ctx = egui::Context::default();
        let began = std::time::Instant::now();
        loop {
            crate::flex_edit::poll(&ctx, p, s);
            if !s.flex.busy() && !analysing() {
                break;
            }
            assert!(began.elapsed().as_secs() < 120, "never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn mono_of(f: &AudioFile) -> Vec<f32> {
        f.read_stereo()
            .unwrap()
            .chunks(2)
            .map(|c| (c[0] + c[1]) * 0.5)
            .collect()
    }

    fn median_pitch(x: &[f32], a: f64, b: f64) -> f32 {
        let t = prowave_engine::track_pitch(x, 44_100);
        let mut v: Vec<f32> = (PitchTrack::frame_at(a)..PitchTrack::frame_at(b))
            .filter_map(|i| t.midi(i))
            .collect();
        assert!(!v.is_empty(), "no pitch in {a}..{b}");
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    }

    fn melody_of(p: &Project, t: usize) -> Option<MelodyEdit> {
        p.tracks[t].regions[0]
            .audio
            .as_ref()
            .and_then(|a| a.flex.as_ref())
            .and_then(|f| f.melody.clone())
    }

    #[test]
    fn edits_are_heard_undone_and_saved() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("sung.wav");
        write_notes(&wav, &[(0.1, 0.6, 57.0), (0.7, 1.2, 60.0)], 1.5);
        let (mut p, t) = project_with(&wav);
        let mut s = UiState::new();
        let mut history = crate::undo::History::new(&p);
        let file = p.tracks[t].regions[0].audio.as_ref().unwrap().file.clone();
        let (_, found) = analysis_for(&file, Algorithm::Melodic).unwrap();
        let notes: Vec<NoteEdit> = found.blobs.iter().map(NoteEdit::from_blob).collect();
        assert_eq!(notes.len(), 2, "{notes:?}");
        // The first note up 3 semitones, exactly on the grid; the second 100 ms later.
        let mut list = notes.clone();
        move_pitch(&mut list, &notes, &[0], 3.0, Snap::Chromatic, 0, false);
        let before = list.clone();
        move_time(&mut list, &before, &[1], 0.1, None, file.duration_secs());
        commit(&mut p, t, 0, Algorithm::Melodic, list.clone());
        history.record(&p);
        settle(&mut p, &mut s);
        let a = p.tracks[t].regions[0].audio.as_ref().unwrap();
        assert_ne!(a.file.path, file.path, "the region plays the made sound");
        let made = mono_of(&a.file);
        assert_eq!(made.len(), mono_of(&file).len(), "as long as the recording");
        let up = median_pitch(&made, 0.2, 0.5);
        assert!(((up - 60.0) * 100.0).abs() < 10.0, "moved note at {up}");
        let second = median_pitch(&made, 0.9, 1.2);
        assert!(
            ((second - 60.0) * 100.0).abs() < 10.0,
            "untouched note at {second}"
        );
        // The second note now starts at 0.8 s, to the sample within 5 ms.
        let start = made
            .iter()
            .enumerate()
            .skip((0.65 * 44_100.0) as usize)
            .find(|(_, v)| v.abs() > 0.05)
            .map(|(i, _)| i as f64 / 44_100.0)
            .unwrap();
        assert!((start - 0.8).abs() < 0.005, "second note starts at {start}");

        // Saved and opened again: the same edit.
        let path = crate::project_io::save_project(&p, &dir.path().join("pitch")).unwrap();
        let (q, warnings) =
            crate::project_io::open_project(&path, &egui::Context::default()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(melody_of(&q, t).map(|m| m.notes), Some(list.clone()));

        // Undo takes it away, Redo brings it back.
        assert!(history.undo(&mut p));
        assert!(melody_of(&p, t).is_none());
        settle(&mut p, &mut s);
        assert_eq!(
            p.tracks[t].regions[0].audio.as_ref().unwrap().file.path,
            file.path
        );
        assert!(history.redo(&mut p));
        assert_eq!(melody_of(&p, t).map(|m| m.notes), Some(list));
    }

    #[test]
    fn the_editor_drags_a_note_up_a_minor_third_and_draws_readably() {
        use crate::ui_test::Harness;
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("line.wav");
        write_notes(
            &wav,
            &[(0.1, 0.7, 57.0), (0.8, 1.4, 62.0), (1.5, 2.2, 64.0)],
            2.4,
        );
        let (mut p, t) = project_with(&wav);
        let mut s = UiState::new();
        open(&mut s, t, 0);
        let mut h = Harness::new(1100.0, 420.0);
        let began = std::time::Instant::now();
        loop {
            h.settle(|ui| show(ui, &mut p, &mut s));
            let file = p.tracks[t].regions[0].audio.as_ref().unwrap().file.clone();
            if memo()
                .lock()
                .unwrap()
                .contains_key(&memo_key(&file, Algorithm::Melodic))
            {
                break;
            }
            assert!(began.elapsed().as_secs() < 60, "no analysis");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        h.settle(|ui| show(ui, &mut p, &mut s));
        let v = &s.pitch_ed;
        let (plot, scroll, zoom, top, row_h) = (v.plot, v.scroll, v.zoom, v.top, v.row_h);
        assert!(plot.width() > 500.0 && row_h >= 4.0, "{plot:?} {row_h}");
        let at = |sec: f64, m: f32| {
            pos2(
                plot.left() + ((sec - scroll) as f32) * zoom,
                plot.top() + (top - m) * row_h,
            )
        };
        // Click the first note, then drag its middle up three rows.
        let from = at(0.4, 57.0);
        h.click(from, |ui| show(ui, &mut p, &mut s));
        assert_eq!(s.pitch_ed.selected, [0]);
        let path: Vec<Pos2> = (0..=6)
            .map(|k| from - egui::vec2(0.0, k as f32 * row_h * 0.5))
            .collect();
        h.time_passes(1.0);
        h.drag(&path, egui::PointerButton::Primary, |ui| {
            show(ui, &mut p, &mut s)
        });
        let m = melody_of(&p, t).expect("an edit was made");
        assert!((m.notes[0].heard() - 60.0).abs() < 1e-3, "{:?}", m.notes[0]);
        assert_eq!(m.notes[1].shift, 0.0);
        // The Separation Tool splits the second note where it is clicked.
        s.pitch_ed.tool = Tool::Separation;
        h.time_passes(1.0);
        h.click(at(1.1, 62.0), |ui| show(ui, &mut p, &mut s));
        assert_eq!(melody_of(&p, t).unwrap().notes.len(), 4);
        s.pitch_ed.tool = Tool::Main;
        s.pitch_ed.selected = vec![0];
        // Drawn offscreen for a look (never on screen).
        let out = std::env::var_os("PROWAVE_UI_PNG_DIR")
            .map_or_else(|| dir.path().to_path_buf(), PathBuf::from)
            .join("pitch-editor.png");
        crate::ui_test::render_png(1100.0, 420.0, 1.0, BG, |ui| show(ui, &mut p, &mut s), &out)
            .unwrap();
        assert!(std::fs::metadata(&out).unwrap().len() > 10_000);

        // A real vocal too, when one is given (PROWAVE_PITCH_EDITOR_SOURCE, seconds from
        // PROWAVE_PITCH_EDITOR_AT): six seconds of it, drawn for a look.
        if let Some(src) = std::env::var_os("PROWAVE_PITCH_EDITOR_SOURCE") {
            let (mut p, t) = project_with(Path::new(&src));
            let at: f64 = std::env::var("PROWAVE_PITCH_EDITOR_AT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30.0);
            p.tracks[t].regions[0].params.offset_secs = at;
            p.tracks[t].regions[0].len_bars = 3.0;
            let mut s = UiState::new();
            open(&mut s, t, 0);
            let file = p.tracks[t].regions[0].audio.as_ref().unwrap().file.clone();
            analysis_for(&file, Algorithm::Melodic).unwrap();
            let out = out.with_file_name("pitch-editor-real.png");
            crate::ui_test::render_png(1400.0, 520.0, 1.0, BG, |ui| show(ui, &mut p, &mut s), &out)
                .unwrap();
        }
    }

    fn note(start: f64, end: f64, pitch: f32) -> NoteEdit {
        NoteEdit::from_blob(&Blob {
            start,
            end,
            pitch,
            onset: start,
        })
    }

    #[test]
    fn a_note_moves_up_on_the_grid_or_freely() {
        let mut n = vec![note(0.0, 1.0, 64.3)];
        let before = n.clone();
        move_pitch(&mut n, &before, &[0], 2.4, Snap::Chromatic, 0, false);
        assert!((n[0].heard() - 67.0).abs() < 1e-4, "{:?}", n[0]);
        move_pitch(&mut n, &before, &[0], 2.4, Snap::Off, 0, true);
        assert!((n[0].heard() - 66.7).abs() < 1e-4);
        // On the C major scale, 66 (F#) is not allowed: 65 or 67.
        move_pitch(&mut n, &before, &[0], 1.6, Snap::Scale, 0, false);
        assert!([65.0, 67.0].contains(&n[0].heard().round()), "{:?}", n[0]);
        assert_eq!(n[0].heard().fract(), 0.0);
    }

    #[test]
    fn moving_a_note_in_time_moves_its_connected_neighbours_edges() {
        let mut n = vec![
            note(0.0, 1.0, 60.0),
            note(1.0, 2.0, 62.0),
            note(2.0, 3.0, 64.0),
        ];
        let before = n.clone();
        move_time(&mut n, &before, &[1], 0.25, None, 4.0);
        assert_eq!((n[1].new_start, n[1].new_end), (1.25, 2.25));
        assert_eq!(n[0].new_end, 1.25, "the note before stretches");
        assert_eq!(n[2].new_start, 2.25, "the note after shrinks");
        // It cannot be pushed through its neighbour.
        let mut m = before.clone();
        move_time(&mut m, &before, &[1], 5.0, None, 4.0);
        assert!(m[1].new_end <= m[2].new_end - MIN_LEN + 1e-9);
        // With a gap, the neighbours stay.
        let mut g = vec![note(0.0, 0.8, 60.0), note(1.0, 1.5, 62.0)];
        let b = g.clone();
        move_time(&mut g, &b, &[1], -0.5, None, 4.0);
        assert_eq!(g[1].new_start, 0.8, "stopped at the note before");
        assert_eq!(g[0].new_end, 0.8);
        // A grid snap puts the start on the grid.
        let mut q = before.clone();
        let snap = |t: f64| (t * 4.0).round() / 4.0;
        move_time(&mut q, &before, &[1], 0.3, Some(&snap), 4.0);
        assert_eq!(q[1].new_start, 1.25);
    }

    #[test]
    fn stretching_split_and_join() {
        let mut n = vec![note(0.0, 1.0, 60.0), note(1.0, 2.0, 62.0)];
        let before = n.clone();
        stretch(&mut n, &before, 1, Edge::Start, 0.6, 3.0);
        assert_eq!((n[1].new_start, n[0].new_end), (0.6, 0.6));
        stretch(&mut n, &before, 1, Edge::End, 2.7, 3.0);
        assert_eq!(n[1].new_end, 2.7);
        // Not shorter than MIN_LEN.
        let mut m = before.clone();
        stretch(&mut m, &before, 0, Edge::End, -4.0, 3.0);
        assert!((m[0].new_end - MIN_LEN).abs() < 1e-9);
        // Split in the middle, then join again: the same note.
        let mut s = vec![NoteEdit {
            shift: 2.0,
            ..note(0.0, 1.0, 60.0)
        }];
        assert!(split(&mut s, 0, 0.4, None));
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].end, s[1].start, s[1].new_start), (0.4, 0.4, 0.4));
        assert_eq!(s[1].shift, 2.0);
        assert!(!split(&mut s, 0, 0.0, None), "not at the very edge");
        assert!(join(&mut s, 0, None));
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].start, s[0].end), (0.0, 1.0));
        assert!((s[0].heard() - 62.0).abs() < 1e-4);
        assert!(!join(&mut s, 0, None));
    }

    #[test]
    fn the_macros_correct_pitch_and_time() {
        let mut n = vec![
            NoteEdit {
                drift: 1.0,
                ..note(0.03, 0.5, 64.3)
            },
            note(0.5, 1.02, 61.0),
            note(1.1, 1.4, 0.0),
        ];
        correct_pitch(&mut n, &[0, 1, 2], 1.0, 0.5, Snap::Scale, 0);
        assert!((n[0].heard() - 64.0).abs() < 1e-4);
        assert!([60.0, 62.0].contains(&n[1].heard()), "C# is not in C major");
        assert_eq!(n[0].drift, 0.5);
        assert_eq!(n[2].shift, 0.0, "unpitched notes are left alone");
        let mut h = vec![note(0.0, 0.5, 64.3)];
        correct_pitch(&mut h, &[0], 0.5, 0.0, Snap::Chromatic, 0);
        assert!((h[0].heard() - 64.15).abs() < 1e-4, "half way");
        // Quantize Time: to the quarter second, fully and half way.
        let grid = |t: f64| (t * 4.0).round() / 4.0;
        let mut q = vec![note(0.1, 0.4, 60.0), note(0.55, 0.9, 62.0)];
        quantize_time(&mut q, &[0, 1], &grid, 1.0, 2.0);
        assert!(
            (q[0].new_start - 0.0).abs() < 1e-9 && (q[1].new_start - 0.5).abs() < 1e-9,
            "{q:?}"
        );
        let mut q = vec![note(0.1, 0.4, 60.0)];
        quantize_time(&mut q, &[0], &grid, 0.5, 2.0);
        assert!((q[0].new_start - 0.05).abs() < 1e-9);
    }

    #[test]
    fn the_time_map_follows_the_moved_notes() {
        let mut m = MelodyEdit {
            algorithm: Algorithm::Melodic,
            notes: vec![note(0.5, 1.0, 60.0), note(1.0, 1.5, 62.0)],
        };
        assert!(m.warp_points(2.0).is_empty(), "nothing moved");
        assert!(m.is_neutral());
        let before = m.notes.clone();
        move_time(&mut m.notes, &before, &[1], 0.1, None, 2.0);
        let pts: Vec<(f64, f64)> = m.warp_points(2.0).iter().map(|p| (p.src, p.dst)).collect();
        assert_eq!(pts, [(0.5, 0.5), (1.0, 1.1), (1.5, 1.6), (2.0, 2.0)]);
        assert!(!m.is_neutral());
    }

    #[test]
    fn the_saved_form_comes_back_and_nonsense_is_left_out() {
        let m = MelodyEdit {
            algorithm: Algorithm::Percussive,
            notes: vec![
                NoteEdit {
                    shift: 1.5,
                    drift: 0.3,
                    modulation: 1.7,
                    formant: -2.0,
                    gain_db: -3.0,
                    new_start: 0.12,
                    ..note(0.1, 0.4, 0.0)
                },
                note(0.4, 0.9, 0.0),
            ],
        };
        assert_eq!(from_doc(&to_doc(&m)), Some(m.clone()));
        let mut d = to_doc(&m);
        d.notes[0].shift = f32::NAN;
        d.notes[0].modulation = 9.0;
        d.notes[1].start = 0.2; // overlaps the first
        d.algorithm = "Nonsense".into();
        let back = from_doc(&d).unwrap();
        assert_eq!(back.algorithm, Algorithm::Melodic);
        assert_eq!(back.notes.len(), 1);
        assert_eq!((back.notes[0].shift, back.notes[0].modulation), (0.0, 2.0));
    }

    #[test]
    fn the_analysis_is_cached_by_content_and_a_replaced_file_is_found_again() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("v.wav");
        crate::flex_edit::tests::write_notes(&wav, &[(0.1, 0.6, 60.0), (0.7, 1.2, 64.0)], 1.4);
        let f = AudioFile::open(&wav).unwrap();
        let (h1, a1) = analysis_for(&f, Algorithm::Melodic).unwrap();
        assert_eq!(a1.blobs.len(), 2, "{:?}", a1.blobs);
        // From memory the second time: the very same analysis.
        let (h2, a2) = analysis_for(&f, Algorithm::Melodic).unwrap();
        assert!(Arc::ptr_eq(&a1, &a2) && h1 == h2);
        // On disk, and read back the same.
        let saved = std::fs::read(analysis_file(h1, Algorithm::Melodic)).unwrap();
        assert_eq!(decode(&saved).as_ref(), Some(&*a1));
        assert!(decode(&saved[..saved.len() - 3]).is_none());
        // The file replaced by another under the same name: found again.
        std::thread::sleep(std::time::Duration::from_millis(20));
        crate::flex_edit::tests::write_notes(&wav, &[(0.1, 1.2, 67.0)], 1.4);
        let f = AudioFile::open(&wav).unwrap();
        let (h3, a3) = analysis_for(&f, Algorithm::Melodic).unwrap();
        assert_ne!(h1, h3);
        assert_eq!(a3.blobs.len(), 1);
        assert!((a3.blobs[0].pitch - 67.0).abs() < 0.2);
    }
}
