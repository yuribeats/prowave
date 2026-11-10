//! Flex Time and Flex Pitch on audio regions (Logic: the Flex mode of a track, Quantize of
//! audio, Flex Pitch's Pitch Correction and "Create MIDI Track from Flex Pitch Data").
//!
//! An edit is kept as data on the region ([`FlexEdit`]): the mode, the Flex markers (a moment
//! of the recording and where it now plays) and the pitch moves. The sound the region plays
//! is made from it in the background (`prowave_engine::warp` and `shift_pitch`) and written to
//! a file in the temporary folder; until it is ready the region plays what it played before.
//! The recording itself is never changed, and the edit is saved with the project and made
//! again when the project opens.
//!
//! The commands that need to know the recording (where the hits are, which notes are sung)
//! read it on a thread too and apply their result when it comes back, if the region is still
//! there.
use crate::i18n::{tr, trf};
use crate::model::{AudioRegion, Note, Project, Region, Track, TrackKind};
use crate::UiState;
use prowave_engine::{AudioFile, FlexMode, PitchShift, TempoMap, WarpPoint};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The Flex edit of an audio region.
#[derive(Clone, Debug)]
pub struct FlexEdit {
    pub mode: FlexMode,
    /// Flex markers: `src` seconds into the recording plays at `dst` seconds from the start of
    /// the region.
    pub markers: Vec<WarpPoint>,
    /// Flex Pitch: stretches of the result (seconds from the region's start) moved in pitch.
    pub pitch: Vec<PitchShift>,
    /// The recording the edit starts from (the region's `file` is the result).
    pub original: AudioFile,
    /// The [`FlexEdit::fingerprint`] the region's `file` was made for (0: not made yet).
    pub rendered: u64,
    /// The Pitch Editor's note edits, made before the Flex markers and pitch moves.
    pub melody: Option<crate::pitch_editor::MelodyEdit>,
    /// Follow Tempo: the recording's own beats are known, and its markers are made again from
    /// them whenever the project's tempo (or the region's place) changes, so every beat stays
    /// on the project's beats.
    pub follow: Option<TempoFollow>,
    /// While the sound for new `markers` is being made: the markers of the sound the region
    /// still plays (None: it plays the sound of `markers`, or the recording when they are
    /// empty). The region's offset is in the time of the sound it plays, so it never plays one
    /// sound from a place in another; [`FlexEdit::target_offset`] takes it to the new one.
    pub played: Option<Vec<WarpPoint>>,
}

/// The beats of a recording that follows the project's tempo (Logic: Flex and Follow): where
/// its beat 1 is and where each beat after (or before) it falls, seconds into the file.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoFollow {
    /// Seconds into the recording of its beat 1 (the One); before the file's start when the
    /// recording begins later in the song.
    pub one_secs: f64,
    /// Seconds into the recording of whole beats in a row, rising: `beats[k]` is quarter note
    /// `first + k` counted from the One (the One is 0).
    pub beats: Vec<f64>,
    pub first: i64,
    /// The recording's tempo before its first beat and after its last one.
    pub bpm: f64,
}

impl TempoFollow {
    /// Quarter notes from the One to moment `src` of the recording, through its beats.
    /// Before the first beat and after the last, the beat next to it goes on at its own length
    /// (`bpm` when there is only one).
    pub fn beat_of(&self, src: f64) -> f64 {
        let b = &self.beats;
        let n = b.len();
        let fallback = 60.0 / self.bpm.max(1.0);
        let len = |i: usize| match (b.get(i), b.get(i + 1)) {
            (Some(x), Some(y)) if y > x => y - x,
            _ => fallback,
        };
        let first = self.first as f64;
        let k = b.partition_point(|x| *x <= src);
        match k.checked_sub(1) {
            Some(i) if i + 1 < n => first + i as f64 + (src - b[i]) / len(i),
            Some(i) => first + i as f64 + (src - b[i]) / len(i.saturating_sub(1)),
            None => first - (b.first().copied().unwrap_or(0.0) - src) / len(0),
        }
    }

    /// Whether it has enough beats to follow a tempo.
    pub fn is_usable(&self) -> bool {
        self.beats.len() >= 2
            && self.bpm.is_finite()
            && self.bpm > 0.0
            && self.one_secs.is_finite()
            && self
                .beats
                .windows(2)
                .all(|w| w[1] > w[0] && w[0].is_finite())
    }

    /// The Flex markers that put each beat on the project's beats when the One is on quarter
    /// note `one` and the recording starts `s0` seconds into the project (Fit to Grid's
    /// markers, from beats that are known instead of heard). Empty when nothing would move.
    pub fn markers(
        &self,
        map: &TempoMap,
        duration: f64,
        mode: FlexMode,
        one: f64,
        s0: f64,
    ) -> Vec<WarpPoint> {
        let found = crate::fit_grid::Found {
            tempo: Some(self.bpm),
            confidence: 1.0,
            tagged: true,
            beats: self.beats.clone(),
            downbeat: 0,
            onsets: Vec::new(),
            mode,
            duration,
        };
        let mut markers =
            crate::fit_grid::grid_markers(map, &found, self.bpm, one + self.first as f64, s0);
        // The tail after the last beat goes on at that beat's length (Fit to Grid's tail goes
        // on at one tempo, which a song with a tempo change does not keep).
        if let Some(m) = markers
            .last_mut()
            .filter(|m| (m.src - duration).abs() < 1e-9)
        {
            m.dst = map.seconds_at(one + self.beat_of(duration)) - s0;
        }
        if markers
            .iter()
            .all(|m| (m.dst - m.src).abs() < FOLLOW_NEUTRAL)
        {
            Vec::new()
        } else {
            markers
        }
    }
}

/// A Follow Tempo region whose beats all land within this of where they were recorded (seconds)
/// plays its recording as it is.
const FOLLOW_NEUTRAL: f64 = 0.0004;

impl FlexEdit {
    pub fn new(original: AudioFile, mode: FlexMode) -> Self {
        Self {
            mode,
            markers: Vec::new(),
            pitch: Vec::new(),
            original,
            rendered: 0,
            melody: None,
            follow: None,
            played: None,
        }
    }

    /// Which moment of the recording plays `dst` seconds into the result (the inverse of
    /// [`FlexEdit::place_of`]).
    pub fn source_of(&self, dst: f64) -> f64 {
        source_through(&self.markers, dst)
    }

    /// The markers of the sound the region plays now (see [`FlexEdit::played`]).
    pub fn played_markers(&self) -> &[WarpPoint] {
        self.played.as_deref().unwrap_or(&self.markers)
    }

    /// Where a region that starts `offset` seconds into the sound it plays now starts in the
    /// sound of `markers` (the same moment of the recording).
    pub fn target_offset(&self, offset: f64) -> f64 {
        match &self.played {
            None => offset,
            Some(old) => {
                let src = source_through(old, offset.max(0.0));
                if src <= 0.0 {
                    0.0
                } else {
                    place_through(&self.markers, src)
                }
            }
        }
    }

    /// Puts in new markers, whose sound is made in the background: until it is ready the
    /// region plays the sound it has (see [`FlexEdit::played`]).
    pub fn set_markers(&mut self, markers: Vec<WarpPoint>) {
        if self.played.is_none() {
            self.played = Some(std::mem::take(&mut self.markers));
        }
        self.markers = markers;
        if self.played.as_deref() == Some(&self.markers[..]) {
            self.played = None;
        }
    }

    /// A number that changes with anything that changes the sound.
    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |w: u64| h = (h ^ w).wrapping_mul(0x0000_0100_0000_01b3);
        for b in self.original.path.to_string_lossy().bytes() {
            mix(u64::from(b));
        }
        // The recording itself: a file that was replaced under the same name is another sound.
        mix(self.original.frames);
        mix(u64::from(self.original.sample_rate));
        mix(self.original.channels as u64);
        mix(std::fs::metadata(&self.original.path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs()));
        for b in self.mode.name().bytes() {
            mix(u64::from(b));
        }
        for m in &self.markers {
            mix(m.src.to_bits());
            mix(m.dst.to_bits());
        }
        mix(0xfeed);
        for p in &self.pitch {
            mix(p.start.to_bits());
            mix(p.end.to_bits());
            mix(u64::from(p.semitones.to_bits()));
        }
        if let Some(m) = &self.melody {
            mix(0x6d65_6c6f);
            m.fingerprint(&mut mix);
        }
        h.max(1)
    }

    /// Nothing is moved: the region plays the recording itself.
    pub fn is_neutral(&self) -> bool {
        self.markers.is_empty()
            && self.pitch.is_empty()
            && self.melody.as_ref().is_none_or(|m| m.is_neutral())
    }

    /// Where moment `src` of the recording plays, in seconds from the region's start.
    pub fn place_of(&self, src: f64) -> f64 {
        place_through(&self.markers, src)
    }
}

/// Which moment of the recording plays `dst` seconds into a sound made with `markers`.
fn source_through(markers: &[WarpPoint], dst: f64) -> f64 {
    let mut prev = WarpPoint { src: 0.0, dst: 0.0 };
    for m in markers {
        if dst < m.dst {
            let span = m.dst - prev.dst;
            return if span > 0.0 {
                prev.src + (dst - prev.dst) * (m.src - prev.src) / span
            } else {
                m.src
            };
        }
        prev = *m;
    }
    prev.src + (dst - prev.dst)
}

/// Where moment `src` of the recording plays in a sound made with `markers`.
fn place_through(markers: &[WarpPoint], src: f64) -> f64 {
    let mut prev = WarpPoint { src: 0.0, dst: 0.0 };
    for m in markers {
        if src < m.src {
            let span = m.src - prev.src;
            return if span > 0.0 {
                prev.dst + (src - prev.src) * (m.dst - prev.dst) / span
            } else {
                m.dst
            };
        }
        prev = *m;
    }
    prev.dst + (src - prev.src)
}

/// What a command does once the recording has been read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Flex markers on the hits, moved to the nearest step of `grid` beats.
    Quantize { grid: f64 },
    /// Flex markers on the beats, moved to the project's beats: the region follows the
    /// project's tempo (Smart Tempo: Adapt Region Tempo).
    FollowProjectTempo,
    /// Flex Pitch: every note moved to the nearest note of the project's key.
    CorrectPitch,
    /// A new software instrument track with the notes Flex Pitch hears.
    MidiFromPitch,
}

/// What the background work found.
enum Outcome {
    Markers {
        markers: Vec<WarpPoint>,
        /// The length of the result, in seconds.
        length: f64,
    },
    Pitch(Vec<PitchShift>),
    Notes(Vec<prowave_engine::DetectedNote>),
}

struct Task {
    track: usize,
    region: usize,
    /// The recording it was started for, to check it is still the same region.
    path: PathBuf,
    command: Command,
    result: Receiver<Result<Outcome, String>>,
}

struct Render {
    fingerprint: u64,
    result: Receiver<Result<AudioFile, String>>,
    /// Set when no region wants the sound any more: the thread stops at its next step and
    /// writes nothing. It keeps its place among [`MAX_RENDERS`] until it has stopped.
    cancel: Arc<AtomicBool>,
}

impl Render {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// The Flex work running in the background.
#[derive(Default)]
pub struct FlexState {
    tasks: Vec<Task>,
    renders: Vec<Render>,
    /// Regions whose edits are to be removed (it needs the context, which the menus lack).
    removals: Vec<(usize, usize)>,
    /// What the Follow Tempo regions were last made for (see [`follow_key`]).
    followed: u64,
    /// A [`follow_key`] not followed yet and when it was first seen: the markers are made
    /// again once it has rested for [`FOLLOW_REST`] (a dragged tempo makes one render, not one
    /// per frame).
    waiting: Option<(u64, Instant)>,
}

impl FlexState {
    /// Whether anything is still being worked on.
    pub fn busy(&self) -> bool {
        !self.tasks.is_empty()
            || !self.renders.is_empty()
            || !self.removals.is_empty()
            || self.waiting.is_some()
    }

    /// Removes the Flex edits of region `r` of track `t` on the next [`poll`].
    pub fn remove_later(&mut self, t: usize, r: usize) {
        self.removals.push((t, r));
    }
}

fn audio_of(p: &Project, t: usize, r: usize) -> Option<&AudioRegion> {
    p.tracks.get(t)?.regions.get(r)?.audio.as_ref()
}

/// The recording under region `r`: the original of its Flex edit, or its file.
pub(crate) fn recording_of(a: &AudioRegion) -> AudioFile {
    a.flex
        .as_ref()
        .map_or_else(|| a.file.clone(), |f| f.original.clone())
}

/// The name of a Flex mode, translated.
pub fn mode_label(mode: FlexMode) -> &'static str {
    match mode {
        FlexMode::Slicing => tr("Slicing"),
        FlexMode::Rhythmic => tr("Rhythmic"),
        FlexMode::Monophonic => tr("Monophonic"),
        FlexMode::Polyphonic => tr("Polyphonic"),
        FlexMode::Speed => tr("Speed"),
        FlexMode::Tempophone => tr("Tempophone"),
    }
}

/// Sets the Flex mode of an audio region (turning Flex on for it).
pub fn set_mode(p: &mut Project, t: usize, r: usize, mode: FlexMode) {
    let Some(a) = p
        .tracks
        .get_mut(t)
        .and_then(|t| t.regions.get_mut(r))
        .and_then(|r| r.audio.as_mut())
    else {
        return;
    };
    match &mut a.flex {
        Some(f) => f.mode = mode,
        None => a.flex = Some(FlexEdit::new(a.file.clone(), mode)),
    }
}

/// Removes the Flex edits of a region: it plays its recording again.
pub fn remove(p: &mut Project, t: usize, r: usize, ctx: &egui::Context) {
    let Some(a) = p
        .tracks
        .get_mut(t)
        .and_then(|t| t.regions.get_mut(r))
        .and_then(|r| r.audio.as_mut())
    else {
        return;
    };
    if let Some(f) = a.flex.take() {
        *a = crate::import::audio_region(f.original, ctx);
        p.song_rev += 1;
    }
}

/// Starts a command on region `r` of track `t`.
pub fn start(p: &Project, s: &mut UiState, t: usize, r: usize, command: Command) {
    let Some(a) = audio_of(p, t, r) else {
        s.status = Some(tr("Flex: this region has no recorded audio").into());
        return;
    };
    if s.flex
        .tasks
        .iter()
        .any(|j| (j.track, j.region, j.command) == (t, r, command))
    {
        return;
    }
    let file = recording_of(a);
    let edit = a.flex.clone();
    let path = file.path.clone();
    let map = p.tempo_map();
    let origin = p.region_start_beats(&p.tracks[t].regions[r]);
    let fifths = crate::score::fifths(p.key);
    let (tx, result) = channel();
    let spawned = std::thread::Builder::new()
        .name("prowave-flex".into())
        .spawn(move || {
            let _ = tx.send(work(&file, edit.as_ref(), &map, origin, fifths, command));
        });
    match spawned {
        Ok(_) => {
            s.flex.tasks.push(Task {
                track: t,
                region: r,
                path,
                command,
                result,
            });
            s.status = Some(tr("Flex: analysing…").into());
        }
        Err(e) => s.status = Some(format!("cannot start the Flex analysis: {e}")),
    }
}

/// Flex markers that put the moments `src` of the recording at `dst`, kept in order (a
/// marker that would go back in time, or land on another, is left out).
pub fn markers_from(pairs: impl IntoIterator<Item = (f64, f64)>) -> Vec<WarpPoint> {
    let mut out: Vec<WarpPoint> = Vec::new();
    for (src, dst) in pairs {
        if !(src.is_finite() && dst.is_finite() && src > 0.0 && dst > 0.0) {
            continue;
        }
        if out
            .last()
            .is_some_and(|m| src <= m.src + 0.002 || dst <= m.dst + 0.002)
        {
            continue;
        }
        out.push(WarpPoint { src, dst });
    }
    out
}

/// The markers that make the beats `beats` (seconds into the recording) land on the project's
/// beats, from the beat nearest the first one on. Also the result's length.
pub fn follow_tempo(
    beats: &[f64],
    duration: f64,
    map: &TempoMap,
    origin: f64,
) -> (Vec<WarpPoint>, f64) {
    let Some(first) = beats.first() else {
        return (Vec::new(), duration);
    };
    let s0 = map.seconds_at(origin);
    let k0 = (map.beat_at(s0 + first) - origin).round().max(0.0);
    let markers = markers_from(
        beats
            .iter()
            .enumerate()
            .map(|(k, b)| (*b, map.seconds_at(origin + k0 + k as f64) - s0)),
    );
    let length = match markers.last() {
        Some(m) => m.dst + (duration - m.src).max(0.0),
        None => duration,
    };
    (markers, length)
}

/// Flex markers that put `beats` (seconds into the recording, as the Smart Tempo Editor has
/// them) on the project's beats, set on region `r` of track `t` at once.
pub fn follow_beats(p: &mut Project, t: usize, r: usize, beats: &[f64]) {
    let Some(a) = audio_of(p, t, r) else { return };
    let duration = recording_of(a).duration_secs();
    let origin = p.region_start_beats(&p.tracks[t].regions[r]);
    let map = p.tempo_map();
    // The editor's beats are of the file the region plays; with an edit already there they are
    // taken back to the recording.
    let edit = a.flex.clone();
    let to_src = |x: f64| match &edit {
        Some(e) if !e.markers.is_empty() => {
            let mut prev = WarpPoint { src: 0.0, dst: 0.0 };
            for m in &e.markers {
                if x < m.dst {
                    let span = m.dst - prev.dst;
                    return if span > 0.0 {
                        prev.src + (x - prev.dst) * (m.src - prev.src) / span
                    } else {
                        m.src
                    };
                }
                prev = *m;
            }
            prev.src + (x - prev.dst)
        }
        _ => x,
    };
    let src: Vec<f64> = beats.iter().map(|b| to_src(*b)).collect();
    let (markers, length) = follow_tempo(&src, duration, &map, origin);
    let mode = flex_mode_of(p, t, r);
    set_mode(p, t, r, mode);
    let end = map.beat_at(map.seconds_at(origin) + length);
    let bar_end = p.beats_to_bar(end);
    let region = &mut p.tracks[t].regions[r];
    if let Some(f) = region.audio.as_mut().and_then(|a| a.flex.as_mut()) {
        f.set_markers(markers);
        f.follow = None;
    }
    region.len_bars = (bar_end as f32 - (region.start_bar - 1.0)).max(1.0 / 64.0);
    p.song_rev += 1;
}

/// Makes the markers of a Follow Tempo region (region `r` of track `t`) again for the project's
/// tempo and the region's place: its beats land on the project's beats, the One where it was.
/// Returns whether anything changed.
pub fn follow(p: &mut Project, t: usize, r: usize) -> bool {
    let Some(region) = p.tracks.get(t).and_then(|x| x.regions.get(r)) else {
        return false;
    };
    let Some(f) = region.audio.as_ref().and_then(|a| a.flex.as_ref()) else {
        return false;
    };
    let Some(fol) = f.follow.as_ref().filter(|x| x.is_usable()) else {
        return false;
    };
    let duration = f.original.duration_secs();
    if duration <= 0.0 {
        return false;
    }
    // The moment of the recording at the region's start is the same whatever the tempo: the
    // One's quarter note comes from it, so a moved or trimmed region keeps its beats. The
    // offset is in the time of the sound the region plays now.
    let start = p.region_start_beats(region);
    let src_at_start = source_through(f.played_markers(), region.params.offset_secs.max(0.0));
    let one = start - fol.beat_of(src_at_start);
    let file_start = one + fol.beat_of(0.0);
    let map = p.tempo_map();
    let s0 = map.seconds_at(file_start);
    let markers = fol.markers(&map, duration, f.mode, one, s0);
    let probe = FlexEdit {
        markers: markers.clone(),
        ..FlexEdit::new(f.original.clone(), f.mode)
    };
    let offset = if src_at_start <= 0.0 {
        0.0
    } else {
        probe.place_of(src_at_start)
    };
    let same = f.markers.len() == markers.len()
        && f.markers
            .iter()
            .zip(&markers)
            .all(|(a, b)| (a.src - b.src).abs() < 1e-7 && (a.dst - b.dst).abs() < 1e-7)
        && (f.target_offset(region.params.offset_secs) - offset).abs() < 1e-7;
    if same {
        return false;
    }
    let region = &mut p.tracks[t].regions[r];
    if let Some(f) = region.audio.as_mut().and_then(|a| a.flex.as_mut()) {
        // Until the sound for the new markers is made, the region plays the sound it has
        // from the place it has; the new place comes with the new sound (`poll_renders`).
        f.set_markers(markers);
        if f.played.is_none() {
            region.params.offset_secs = offset;
        }
    }
    p.song_rev += 1;
    true
}

/// A number that changes with anything a Follow Tempo region's markers depend on: the tempo
/// and signature tracks and each such region's place. 0 when there are none.
fn follow_key(p: &Project) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |w: u64| h = (h ^ w).wrapping_mul(0x0000_0100_0000_01b3);
    let mut any = false;
    for (t, track) in p.tracks.iter().enumerate() {
        for (r, region) in track.regions.iter().enumerate() {
            let follows = region
                .audio
                .as_ref()
                .and_then(|a| a.flex.as_ref())
                .is_some_and(|f| f.follow.is_some());
            if follows {
                any = true;
                mix(t as u64);
                mix(r as u64);
                mix(u64::from(region.start_bar.to_bits()));
                mix(region.params.offset_secs.to_bits());
            }
        }
    }
    if !any {
        return 0;
    }
    for pt in p.tempo_points() {
        mix(pt.beat.to_bits());
        mix(u64::from(pt.bpm.to_bits()));
        mix(u64::from(pt.ramp));
    }
    mix(u64::from(p.sig.0) << 8 | u64::from(p.sig.1));
    for c in &p.sig_changes {
        mix(u64::from(c.bar) << 16 | u64::from(c.num) << 8 | u64::from(c.den));
    }
    h.max(1)
}

/// How long the tempo and the Follow Tempo regions' places stay the same before their markers
/// are made again (a tempo being dragged, or followed from a MIDI clock, changes every frame).
const FOLLOW_REST: Duration = Duration::from_millis(300);

/// Makes the markers of every Follow Tempo region again when the tempo or a place changed and
/// has rested for [`FOLLOW_REST`].
fn follow_all(p: &mut Project, st: &mut FlexState) {
    let key = follow_key(p);
    if key == st.followed {
        st.waiting = None;
        return;
    }
    match st.waiting {
        Some((k, since)) if k == key => {
            if since.elapsed() < FOLLOW_REST {
                return;
            }
        }
        _ => {
            st.waiting = Some((key, Instant::now()));
            return;
        }
    }
    follow_now(p, st);
}

/// Makes the markers of every Follow Tempo region again now if the tempo or a place changed
/// since they were made (no waiting: an Undo step is about to be kept, so the tempo and its
/// markers go in it together). Returns whether any changed.
pub fn follow_now(p: &mut Project, st: &mut FlexState) -> bool {
    st.waiting = None;
    if follow_key(p) == st.followed {
        return false;
    }
    let mut changed = false;
    for t in 0..p.tracks.len() {
        for r in 0..p.tracks[t].regions.len() {
            changed |= follow(p, t, r);
        }
    }
    // The markers' own changes do not move the key (the offset may: take it as it is now).
    st.followed = follow_key(p);
    changed
}

/// Time Stretch: region `r` of track `t` plays its audio `factor` times as long (2 is half
/// speed), without a change of pitch. Existing Flex markers are stretched with it, and where
/// the region starts in its file moves with the sound. Made in the background, like any
/// Flex edit. Returns whether it is an audio region that could be stretched.
pub fn stretch_by(p: &mut Project, t: usize, r: usize, factor: f64) -> bool {
    if !(factor.is_finite() && (0.05..=20.0).contains(&factor)) {
        return false;
    }
    let Some(a) = audio_of(p, t, r) else {
        return false;
    };
    let duration = recording_of(a).duration_secs();
    if duration <= 0.0 {
        return false;
    }
    // A region without Flex yet stretches as music (Polyphonic); a chosen mode is kept.
    let mode = a.flex.as_ref().map_or(FlexMode::Polyphonic, |f| f.mode);
    set_mode(p, t, r, mode);
    let region = &mut p.tracks[t].regions[r];
    if let Some(f) = region.audio.as_mut().and_then(|a| a.flex.as_mut()) {
        // Stretched by hand: it no longer follows the tempo by its beats.
        f.follow = None;
        // A marker at the end, so the tail after the last one stretches too.
        let mut markers = f.markers.clone();
        let end = f.place_of(duration);
        if markers.last().is_none_or(|m| m.src < duration) {
            markers.push(WarpPoint {
                src: duration,
                dst: end,
            });
        }
        for m in &mut markers {
            m.dst *= factor;
        }
        // Where the region starts in its sound moves with the sound, when it is made.
        f.set_markers(markers);
    }
    p.song_rev += 1;
    true
}

/// Time Stretch to a new end: region `r` of track `t` is made to last until `end` (quarter
/// notes), its audio stretched to fill it (Logic: Option-drag the region's end, Time Stretch
/// Region to Nearest Bar / to Locators).
pub fn stretch_to(p: &mut Project, t: usize, r: usize, start: f64, end: f64) -> bool {
    let Some(region) = p.tracks.get(t).and_then(|x| x.regions.get(r)) else {
        return false;
    };
    if end <= start || region.audio.is_none() {
        return false;
    }
    let map = p.tempo_map();
    let old_start = p.region_start_beats(region);
    let old = map.seconds_at(p.region_end_beats(region)) - map.seconds_at(old_start);
    let new = map.seconds_at(end) - map.seconds_at(start);
    if old <= 0.0 || !stretch_by(p, t, r, new / old) {
        return false;
    }
    let start_bar = p.beats_to_bar(start) as f32 + 1.0;
    let end_bar = p.beats_to_bar(end) as f32 + 1.0;
    let region = &mut p.tracks[t].regions[r];
    region.start_bar = start_bar;
    region.len_bars = end_bar - start_bar;
    true
}

/// The nearest MIDI note to `pitch` that is in the key with `fifths` sharps or flats.
pub fn nearest_in_key(pitch: f32, fifths: i8) -> f32 {
    const NATURAL: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    let alt = crate::score::alterations(fifths);
    let pcs: Vec<i32> = (0..7)
        .map(|l| (NATURAL[l] + i32::from(alt[l])).rem_euclid(12))
        .collect();
    let base = pitch.round() as i32;
    (-6..=6)
        .map(|d| base + d)
        .filter(|m| pcs.contains(&m.rem_euclid(12)))
        .min_by(|a, b| {
            (*a as f32 - pitch)
                .abs()
                .total_cmp(&(*b as f32 - pitch).abs())
        })
        .unwrap_or(base) as f32
}

fn work(
    file: &AudioFile,
    edit: Option<&FlexEdit>,
    map: &TempoMap,
    origin: f64,
    fifths: i8,
    command: Command,
) -> Result<Outcome, String> {
    let found = file.analyze().map_err(|e| e.to_string())?;
    let place = |src: f64| edit.map_or(src, |e| e.place_of(src));
    match command {
        Command::Quantize { grid } => {
            let grid = if grid.is_finite() && grid > 0.0 {
                grid
            } else {
                0.25
            };
            let s0 = map.seconds_at(origin);
            let markers = markers_from(found.onsets.iter().map(|t| {
                let beat = map.beat_at(s0 + t) - origin;
                let q = (beat / grid).round() * grid;
                (*t, map.seconds_at(origin + q.max(0.0)) - s0)
            }));
            let length = match markers.last() {
                Some(m) => m.dst + (file.duration_secs() - m.src).max(0.0),
                None => file.duration_secs(),
            };
            if markers.is_empty() {
                return Err(tr("no hits found to quantize").into());
            }
            Ok(Outcome::Markers { markers, length })
        }
        Command::FollowProjectTempo => {
            if found.beats.len() < 2 {
                return Err(tr("no steady beat found").into());
            }
            let (markers, length) = follow_tempo(&found.beats, file.duration_secs(), map, origin);
            Ok(Outcome::Markers { markers, length })
        }
        Command::CorrectPitch | Command::MidiFromPitch => {
            let stereo = file.read_stereo().map_err(|e| e.to_string())?;
            let mono: Vec<f32> = stereo.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
            let notes = prowave_engine::detect_notes(&mono, file.sample_rate, &found.onsets);
            if notes.is_empty() {
                return Err(tr("no notes with a clear pitch found").into());
            }
            // Into the time of the result (after any Flex Time markers).
            let notes: Vec<_> = notes
                .into_iter()
                .map(|n| prowave_engine::DetectedNote {
                    start: place(n.start),
                    end: place(n.end),
                    ..n
                })
                .collect();
            if command == Command::MidiFromPitch {
                return Ok(Outcome::Notes(notes));
            }
            Ok(Outcome::Pitch(
                notes
                    .iter()
                    .filter_map(|n| {
                        let semitones = nearest_in_key(n.pitch, fifths) - n.pitch;
                        (semitones.abs() > 0.05).then_some(PitchShift {
                            start: n.start,
                            end: n.end,
                            semitones,
                        })
                    })
                    .collect(),
            ))
        }
    }
}

/// Applies the commands that finished and the sounds that are ready, and starts making the
/// sound of edits that changed. Call once per frame.
pub fn poll(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    for (t, r) in std::mem::take(&mut s.flex.removals) {
        remove(p, t, r, ctx);
    }
    poll_tasks(p, s);
    follow_all(p, &mut s.flex);
    start_renders(ctx, p, s);
    poll_renders(ctx, p, s);
    if s.flex.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn poll_tasks(p: &mut Project, s: &mut UiState) {
    if s.flex.tasks.is_empty() {
        return;
    }
    let mut finished = Vec::new();
    s.flex.tasks.retain(|job| match job.result.try_recv() {
        Err(TryRecvError::Empty) => true,
        Err(TryRecvError::Disconnected) => false,
        Ok(result) => {
            finished.push((job.track, job.region, job.path.clone(), job.command, result));
            false
        }
    });
    for (t, r, path, _command, result) in finished {
        let same = audio_of(p, t, r).is_some_and(|a| recording_of(a).path == path);
        if !same {
            continue;
        }
        let outcome = match result {
            Ok(o) => o,
            Err(e) => {
                s.status = Some(trf("Flex: {}", &[&e]));
                continue;
            }
        };
        s.status = None;
        match outcome {
            Outcome::Markers { markers, length } => {
                let mode = flex_mode_of(p, t, r);
                set_mode(p, t, r, mode);
                let origin = p.region_start_beats(&p.tracks[t].regions[r]);
                let map = p.tempo_map();
                let end = map.beat_at(map.seconds_at(origin) + length);
                let bar_end = p.beats_to_bar(end);
                let region = &mut p.tracks[t].regions[r];
                if let Some(f) = region.audio.as_mut().and_then(|a| a.flex.as_mut()) {
                    f.set_markers(markers);
                    f.follow = None;
                }
                // The region covers the whole result.
                region.len_bars = (bar_end as f32 - (region.start_bar - 1.0)).max(1.0 / 64.0);
                s.status = Some(tr("Flex: markers set").into());
            }
            Outcome::Pitch(shifts) => {
                let mode = flex_mode_of(p, t, r);
                set_mode(p, t, r, mode);
                let n = shifts.len();
                if let Some(f) = p.tracks[t].regions[r]
                    .audio
                    .as_mut()
                    .and_then(|a| a.flex.as_mut())
                {
                    f.pitch = shifts;
                }
                s.status = Some(trf("Flex Pitch: {} notes corrected", &[&n]));
            }
            Outcome::Notes(notes) => {
                let made = midi_track_from(p, t, r, &notes);
                s.status = Some(trf("Flex Pitch: {} notes on a new track", &[&made]));
            }
        }
        p.song_rev += 1;
    }
}

/// The region's Flex mode, or the one that suits its track (Monophonic for most recordings).
fn flex_mode_of(p: &Project, t: usize, r: usize) -> FlexMode {
    audio_of(p, t, r)
        .and_then(|a| a.flex.as_ref())
        .map_or(FlexMode::Monophonic, |f| f.mode)
}

/// Adds a software instrument track under track `t` with a region at the place of region `r`
/// holding `notes`. Returns how many notes it has.
fn midi_track_from(
    p: &mut Project,
    t: usize,
    r: usize,
    notes: &[prowave_engine::DetectedNote],
) -> usize {
    let src = &p.tracks[t].regions[r];
    let origin = p.region_start_beats(src);
    let map = p.tempo_map();
    let s0 = map.seconds_at(origin);
    let beats = |sec: f64| (map.beat_at(s0 + sec) - origin) as f32;
    let midi: Vec<Note> = notes
        .iter()
        .map(|n| {
            let start = beats(n.start).max(0.0);
            Note {
                lyric: crate::lyrics::Lyric::NONE,
                mute: false,
                start,
                len: (beats(n.end) - start).max(1.0 / 32.0),
                pitch: n.pitch.round().clamp(0.0, 127.0) as u8,
                vel: (40.0 + 87.0 * n.level.clamp(0.0, 1.0)).round() as u8,
            }
        })
        .collect();
    let region = Region {
        name: format!("{} (Flex Pitch)", src.name),
        start_bar: src.start_bar,
        len_bars: src.len_bars,
        notes: midi,
        controls: Vec::new(),
        folder: None,
        audio: None,
        params: crate::model::RegionParams::default(),
        generated: None,
    };
    let mut track = Track::new(
        format!("{} (Flex Pitch)", p.tracks[t].name),
        TrackKind::Instrument,
    );
    let made = region.notes.len();
    track.regions.push(region);
    p.tracks.insert(t + 1, track);
    made
}

/// The folder the sounds of Flex edits are written to: the user's own config folder (a shared
/// temporary folder could be prepared by another user), the temporary one only when there is
/// none.
pub(crate) fn cache_dir() -> PathBuf {
    crate::recent_files::config_dir()
        .map(|d| d.join("flex-cache"))
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("prowave-flex-{}", std::process::id()))
        })
}

/// How long a made sound is kept after it was last made, in days.
const CACHE_DAYS: u64 = 14;
/// Renders running at once: each reads a whole recording.
const MAX_RENDERS: usize = 4;

/// Removes sounds nobody asked for in a while (once per run), so the folder does not grow
/// without end.
fn clean_cache(dir: &std::path::Path, days: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let limit = std::time::Duration::from_secs(days * 86_400);
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > limit);
        let ours = e
            .path()
            .extension()
            .is_some_and(|x| x == "wav" || x == "part");
        if old && ours {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The sounds the regions wait for: the fingerprints of the edits not made yet that are not
/// neutral.
fn wanted(p: &Project) -> Vec<u64> {
    p.tracks
        .iter()
        .flat_map(|t| &t.regions)
        .filter_map(|r| r.audio.as_ref()?.flex.as_ref())
        .filter(|f| !f.is_neutral())
        .map(FlexEdit::fingerprint)
        .collect()
}

/// Stops the renders no region waits for any more (a tempo moved on, an edit changed again):
/// they write nothing and free their place when their thread has stopped.
fn cancel_stale(p: &Project, st: &mut FlexState) {
    if st.renders.iter().all(Render::cancelled) {
        return;
    }
    let wanted = wanted(p);
    for r in &st.renders {
        if !wanted.contains(&r.fingerprint) {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn start_renders(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    static CLEANED: std::sync::Once = std::sync::Once::new();
    cancel_stale(p, &mut s.flex);
    for track in &mut p.tracks {
        for region in &mut track.regions {
            let Some(a) = region.audio.as_mut() else {
                continue;
            };
            let Some(f) = a.flex.as_mut() else { continue };
            let fp = f.fingerprint();
            if f.rendered == fp
                || s.flex
                    .renders
                    .iter()
                    .any(|r| r.fingerprint == fp && !r.cancelled())
            {
                continue;
            }
            if f.is_neutral() {
                // Nothing moved: the recording itself plays, with its own waveform (the
                // overview of the sound made before would not match it), from the place in it
                // the region had in the sound before.
                region.params.offset_secs = f.target_offset(region.params.offset_secs);
                f.played = None;
                if a.file.path != f.original.path {
                    if let Some(flex) = a.flex.take() {
                        *a = crate::import::audio_region(flex.original.clone(), ctx);
                        a.flex = Some(flex);
                    }
                }
                if let Some(f) = a.flex.as_mut() {
                    f.rendered = fp;
                }
                p.song_rev += 1;
                continue;
            }
            if s.flex.renders.len() >= MAX_RENDERS {
                // The rest wait their turn (this runs every frame).
                return;
            }
            CLEANED.call_once(|| clean_cache(&cache_dir(), CACHE_DAYS));
            let edit = f.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let stop = cancel.clone();
            let (tx, result) = channel();
            let spawned = std::thread::Builder::new()
                .name("prowave-flex-render".into())
                .spawn(move || {
                    let _ = tx.send(render_unless(&edit, fp, &stop));
                });
            if spawned.is_ok() {
                s.flex.renders.push(Render {
                    fingerprint: fp,
                    result,
                    cancel,
                });
            }
        }
    }
}

/// Makes the sound of an edit (or finds it already made).
#[cfg(test)]
pub(crate) fn render(edit: &FlexEdit, fingerprint: u64) -> Result<AudioFile, String> {
    render_unless(edit, fingerprint, &AtomicBool::new(false))
}

/// What a render stopped by its `cancel` flag returns.
const CANCELLED: &str = "cancelled";

/// [`render`], stopped between its steps (and before anything is written) once `cancel` is
/// set. The edit's own markers are made, never the ones of the sound played before it.
fn render_unless(
    edit: &FlexEdit,
    fingerprint: u64,
    cancel: &AtomicBool,
) -> Result<AudioFile, String> {
    let stopped = || cancel.load(Ordering::Relaxed);
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{fingerprint:016x}.wav"));
    if let Ok(f) = AudioFile::open(&path) {
        return Ok(f);
    }
    if stopped() {
        return Err(CANCELLED.into());
    }
    let sr = edit.original.sample_rate;
    let input = edit.original.read_stereo().map_err(|e| e.to_string())?;
    let input = match &edit.melody {
        Some(m) if !m.is_neutral() => crate::pitch_editor::render(&edit.original, input, m)?,
        _ => input,
    };
    if stopped() {
        return Err(CANCELLED.into());
    }
    // AGENT: @engine-agent a cancel flag inside `warp` / `shift_pitch` would stop a stale
    // render mid-way; today it stops only between them.
    let timed = if edit.markers.is_empty() {
        input
    } else {
        prowave_engine::warp(&input, sr, &edit.markers, edit.mode)
    };
    if stopped() {
        return Err(CANCELLED.into());
    }
    let pitched = if edit.pitch.is_empty() {
        timed
    } else {
        // Pitch moves keep the time; Polyphonic material keeps its longer window.
        let mode = if edit.mode == FlexMode::Polyphonic {
            FlexMode::Polyphonic
        } else {
            FlexMode::Monophonic
        };
        prowave_engine::shift_pitch(&timed, sr, &edit.pitch, mode)
    };
    if stopped() {
        return Err(CANCELLED.into());
    }
    prowave_engine::write_wav(&path, &pitched, sr).map_err(|e| e.to_string())?;
    AudioFile::open(&path).map_err(|e| e.to_string())
}

fn poll_renders(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    let mut done = Vec::new();
    s.flex.renders.retain(|job| match job.result.try_recv() {
        Err(TryRecvError::Empty) => true,
        Err(TryRecvError::Disconnected) => false,
        Ok(result) => {
            done.push((job.fingerprint, job.cancelled(), result));
            false
        }
    });
    for (fp, cancelled, result) in done {
        let file = match result {
            Ok(f) => f,
            Err(_) if cancelled => continue,
            Err(e) => {
                s.status = Some(trf("Flex: cannot make the sound: {}", &[&e]));
                continue;
            }
        };
        // Only the regions whose edit this sound is made for; every other keeps its own.
        for track in &mut p.tracks {
            for region in &mut track.regions {
                let Some(a) = region.audio.as_mut() else {
                    continue;
                };
                if a.flex.as_ref().is_none_or(|f| f.fingerprint() != fp) {
                    continue;
                }
                let Some(mut f) = a.flex.take() else {
                    continue;
                };
                // The new sound comes with its own place for the region's start.
                region.params.offset_secs = f.target_offset(region.params.offset_secs);
                f.played = None;
                f.rendered = fp;
                *a = crate::import::audio_region(file.clone(), ctx);
                a.flex = Some(f);
                p.song_rev += 1;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {

    /// Time Stretch to a length: a one-bar audio region dragged out to two bars plays its
    /// recording at half speed, with an even stretch (a marker at the recording's end).
    #[test]
    fn a_region_stretched_to_twice_its_length_plays_its_recording_evenly() {
        use crate::model::{AudioRegion, Region, RegionParams, Track, TrackKind};
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../prowave-engine/tests/fixtures");
        let file = AudioFile::open(dir.join("tone.caf")).unwrap();
        let dur = file.duration_secs();
        let mut p = Project::empty();
        p.tracks.clear();
        let mut t = Track::new("Audio 1", TrackKind::Audio);
        t.regions.push(Region {
            name: "r".into(),
            start_bar: 1.0,
            len_bars: 1.0,
            notes: Vec::new(),
            controls: Vec::new(),
            folder: None,
            generated: None,
            audio: Some(AudioRegion {
                file,
                peaks: Default::default(),
                flex: None,
            }),
            params: RegionParams::default(),
        });
        p.tracks.push(t);
        assert!(stretch_to(&mut p, 0, 0, 0.0, 8.0));
        let r = &p.tracks[0].regions[0];
        assert_eq!((r.start_bar, r.len_bars), (1.0, 2.0));
        let f = r.audio.as_ref().unwrap().flex.as_ref().unwrap();
        assert_eq!(f.mode, FlexMode::Polyphonic);
        assert_eq!(f.markers.len(), 1);
        assert!((f.markers[0].src - dur).abs() < 1e-9);
        assert!(
            (f.markers[0].dst - 2.0 * dur).abs() < 1e-6,
            "{:?}",
            f.markers
        );
        assert!(
            !stretch_to(&mut p, 0, 0, 8.0, 4.0),
            "an end before the start is refused"
        );
    }

    use super::*;

    /// A stereo 16-bit WAV with a click at each of `times` (seconds).
    pub(crate) fn write_clicks(path: &std::path::Path, times: &[f64], seconds: f64) {
        const RATE: u32 = 44_100;
        let frames = (seconds * f64::from(RATE)) as usize;
        let mut v = vec![0i16; frames];
        for t in times {
            let a = (t * f64::from(RATE)) as usize;
            for k in 0..300 {
                if a + k < frames {
                    v[a + k] = if k % 2 == 0 { 20_000 } else { -20_000 };
                }
            }
        }
        let data_len = (frames * 4) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&RATE.to_le_bytes());
        b.extend_from_slice(&(RATE * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for s in v {
            b.extend_from_slice(&s.to_le_bytes());
            b.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    /// A stereo 16-bit WAV of sine notes: (start, end, MIDI pitch).
    pub(crate) fn write_notes(path: &std::path::Path, notes: &[(f64, f64, f32)], seconds: f64) {
        const RATE: u32 = 44_100;
        let frames = (seconds * f64::from(RATE)) as usize;
        let mut v = vec![0i16; frames];
        for (a, b, m) in notes {
            let hz = 440.0 * 2f64.powf((f64::from(*m) - 69.0) / 12.0);
            let (from, to) = (
                (a * f64::from(RATE)) as usize,
                ((b * f64::from(RATE)) as usize).min(frames),
            );
            for (i, s) in v.iter_mut().enumerate().take(to).skip(from) {
                let t = i as f64 / f64::from(RATE);
                *s = ((std::f64::consts::TAU * hz * t).sin() * 12_000.0) as i16;
            }
        }
        let data_len = (frames * 4) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&RATE.to_le_bytes());
        b.extend_from_slice(&(RATE * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for s in v {
            b.extend_from_slice(&s.to_le_bytes());
            b.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    fn project_with(path: &std::path::Path) -> (Project, usize) {
        let mut p = Project::empty();
        p.tempo = 120.0;
        crate::import::import_audio(&mut p, path, &egui::Context::default()).unwrap();
        // The region at bar 1, so seconds and beats are easy: half a second a beat.
        let t = p.tracks.len() - 1;
        p.tracks[t].regions[0].start_bar = 1.0;
        (p, t)
    }

    /// Polls until nothing is running.
    fn settle(p: &mut Project, s: &mut UiState) {
        let ctx = egui::Context::default();
        let began = std::time::Instant::now();
        loop {
            poll(&ctx, p, s);
            if !s.flex.busy() {
                break;
            }
            assert!(began.elapsed().as_secs() < 60, "Flex never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn old_sounds_are_cleaned_and_other_files_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.wav", "b.wav.part", "notes.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        clean_cache(dir.path(), 14);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3, "all new");
        // With a limit of zero days everything of ours is old.
        std::thread::sleep(std::time::Duration::from_millis(20));
        clean_cache(dir.path(), 0);
        let left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["notes.txt"]);
        assert!(clean_cache_missing());
    }

    fn clean_cache_missing() -> bool {
        clean_cache(std::path::Path::new("/no/such/folder"), 1);
        true
    }

    #[test]
    fn a_replaced_recording_is_another_sound() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("r.wav");
        write_clicks(&wav, &[0.5], 2.0);
        let a = FlexEdit::new(AudioFile::open(&wav).unwrap(), FlexMode::Rhythmic);
        write_clicks(&wav, &[0.5], 3.0);
        let b = FlexEdit::new(AudioFile::open(&wav).unwrap(), FlexMode::Rhythmic);
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn markers_stay_in_order() {
        let m = markers_from([
            (1.0, 1.0),
            (0.5, 2.0),
            (2.0, 0.5),
            (3.0, 3.0),
            (f64::NAN, 4.0),
        ]);
        assert_eq!(
            m.iter().map(|m| (m.src, m.dst)).collect::<Vec<_>>(),
            [(1.0, 1.0), (3.0, 3.0)]
        );
        let edit = FlexEdit {
            markers: m,
            ..FlexEdit::new(
                AudioFile {
                    path: "x.wav".into(),
                    sample_rate: 44_100,
                    frames: 1,
                    channels: 2,
                },
                FlexMode::Rhythmic,
            )
        };
        assert_eq!(edit.place_of(0.5), 0.5);
        assert_eq!(edit.place_of(2.0), 2.0);
        assert_eq!(edit.place_of(4.0), 4.0);
        let mut other = edit.clone();
        other.mode = FlexMode::Slicing;
        assert_ne!(edit.fingerprint(), other.fingerprint());
    }

    #[test]
    fn the_key_decides_where_a_note_goes() {
        assert_eq!(nearest_in_key(60.3, 0), 60.0);
        assert_eq!(nearest_in_key(61.2, 0), 62.0, "C# is not in C major");
        assert_eq!(nearest_in_key(66.0, 1), 66.0, "F# is in G major");
        assert_eq!(nearest_in_key(65.8, 1), 66.0);
        assert_eq!(nearest_in_key(70.0, -1), 70.0, "Bb is in F major");
    }

    #[test]
    fn quantize_moves_the_hits_to_the_grid_and_the_region_plays_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("drums.wav");
        // At 120 BPM the beats are half a second apart: two hits are late, one early.
        write_clicks(&wav, &[0.5, 1.04, 1.47, 2.0, 2.53], 3.5);
        let (mut p, t) = project_with(&wav);
        let original = p.tracks[t].regions[0]
            .audio
            .as_ref()
            .unwrap()
            .file
            .path
            .clone();
        let mut s = UiState::new();
        set_mode(&mut p, t, 0, FlexMode::Slicing);
        start(&p, &mut s, t, 0, Command::Quantize { grid: 1.0 });
        settle(&mut p, &mut s);
        let a = p.tracks[t].regions[0].audio.as_ref().unwrap();
        let f = a.flex.as_ref().unwrap();
        assert_eq!(f.mode, FlexMode::Slicing);
        assert!(f.markers.len() >= 4, "{:?}", f.markers);
        for m in &f.markers {
            assert!(
                ((m.dst * 2.0) - (m.dst * 2.0).round()).abs() < 0.01,
                "{m:?}"
            );
        }
        // The region now plays a new file; the recording is untouched.
        assert_ne!(a.file.path, original);
        assert_eq!(f.original.path, original);
        assert_eq!(f.rendered, f.fingerprint());
        let found = a.file.analyze().unwrap();
        for o in &found.onsets {
            let off = (o * 2.0 - (o * 2.0).round()).abs() / 2.0;
            assert!(off < 0.012, "a hit at {o}");
        }
        // Removing the edit plays the recording again.
        remove(&mut p, t, 0, &egui::Context::default());
        let a = p.tracks[t].regions[0].audio.as_ref().unwrap();
        assert!(a.flex.is_none());
        assert_eq!(a.file.path, original);
    }

    #[test]
    fn pitch_is_corrected_to_the_key_and_made_into_midi() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("vocal.wav");
        // A slightly sharp E (64.3), then a C# (61) that is not in C major.
        write_notes(&wav, &[(0.0, 0.8, 64.3), (1.0, 1.8, 61.0)], 2.0);
        let (mut p, t) = project_with(&wav);
        p.key = "Cmaj";
        let mut s = UiState::new();
        start(&p, &mut s, t, 0, Command::CorrectPitch);
        settle(&mut p, &mut s);
        let f = p.tracks[t].regions[0]
            .audio
            .as_ref()
            .unwrap()
            .flex
            .clone()
            .unwrap();
        assert_eq!(f.pitch.len(), 2, "{:?}", f.pitch);
        assert!((f.pitch[0].semitones + 0.3).abs() < 0.1, "{:?}", f.pitch);
        assert!(
            (f.pitch[1].semitones.abs() - 1.0).abs() < 0.1,
            "C# goes to C or D: {:?}",
            f.pitch
        );
        // The made sound has the new pitches.
        let a = p.tracks[t].regions[0].audio.as_ref().unwrap();
        let stereo = a.file.read_stereo().unwrap();
        let mono: Vec<f32> = stereo.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
        let notes = prowave_engine::detect_notes(&mono, 44_100, &[1.0]);
        assert!((notes[0].pitch - 64.0).abs() < 0.15, "{notes:?}");

        // Create MIDI: a new track under it with the two notes.
        let tracks = p.tracks.len();
        start(&p, &mut s, t, 0, Command::MidiFromPitch);
        settle(&mut p, &mut s);
        assert_eq!(p.tracks.len(), tracks + 1);
        let midi = &p.tracks[t + 1];
        assert_eq!(midi.kind, TrackKind::Instrument);
        let n = &midi.regions[0].notes;
        assert_eq!(n.len(), 2, "{n:?}");
        assert_eq!(n[0].pitch, 64);
        assert!(
            (n[1].start - 2.0).abs() < 0.1,
            "second note on beat 3: {n:?}"
        );
    }

    #[test]
    fn a_region_can_follow_the_project_tempo() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("loop.wav");
        // A loop at 100 BPM in a project at 120.
        let times: Vec<f64> = (0..16).map(|k| 0.3 + k as f64 * 0.6).collect();
        write_clicks(&wav, &times, 10.0);
        let (mut p, t) = project_with(&wav);
        let mut s = UiState::new();
        start(&p, &mut s, t, 0, Command::FollowProjectTempo);
        settle(&mut p, &mut s);
        let a = p.tracks[t].regions[0].audio.as_ref().unwrap();
        let found = a.file.analyze().unwrap();
        let tempo = found.tempo.unwrap();
        assert!((tempo.bpm - 120.0).abs() < 1.5, "{}", tempo.bpm);
        // The region got shorter with the faster tempo.
        assert!(
            p.tracks[t].regions[0].len_bars < 4.6,
            "{}",
            p.tracks[t].regions[0].len_bars
        );
    }

    /// Follow Tempo: a loop at 100 BPM whose beats are known, in a project at 120, then 90:
    /// its markers are made again when the tempo changes and every beat lands on the project's.
    #[test]
    fn a_follow_tempo_region_keeps_its_beats_on_the_project_beats() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("loop.wav");
        let beats: Vec<f64> = (0..16).map(|k| 0.25 + k as f64 * 0.6).collect();
        write_clicks(&wav, &beats, 10.0);
        let (mut p, t) = project_with(&wav);
        set_mode(&mut p, t, 0, FlexMode::Rhythmic);
        let follow = TempoFollow {
            one_secs: 0.25,
            beats: beats.clone(),
            first: 0,
            bpm: 100.0,
        };
        // The recording starts a quarter second before its beat 1: on bar 2 (beat 4).
        p.tracks[t].regions[0].start_bar = 2.0 - (0.25 / 0.6 / 4.0) as f32;
        if let Some(f) = p.tracks[t].regions[0]
            .audio
            .as_mut()
            .and_then(|a| a.flex.as_mut())
        {
            f.follow = Some(follow);
        }
        let mut s = UiState::new();
        for bpm in [120.0f32, 90.0] {
            p.tempo = bpm;
            settle(&mut p, &mut s);
            let region = &p.tracks[t].regions[0];
            let a = region.audio.as_ref().unwrap();
            assert!(!a.flex.as_ref().unwrap().markers.is_empty());
            let x = a.file.read_stereo().unwrap();
            let map = p.tempo_map();
            let s0 = map.seconds_at(p.region_start_beats(region));
            for k in [0usize, 5, 15] {
                let want = map.seconds_at(4.0 + k as f64) - s0;
                let i0 = ((want - 0.01) * 44_100.0) as usize;
                let i = (i0..i0 + 882).find(|&i| x[i * 2].abs() > 0.3).unwrap();
                let err = i as f64 / 44_100.0 - want;
                assert!(err.abs() < 1e-3, "{bpm}: beat {k} off {err}");
            }
        }
    }

    /// A loop at 100 BPM whose beats are known (Follow Tempo, Rhythmic), its beat 1 on bar 2,
    /// in a project at 120, its sound made.
    fn follow_project(dir: &std::path::Path, s: &mut UiState) -> (Project, usize) {
        let wav = dir.join("loop.wav");
        let beats: Vec<f64> = (0..16).map(|k| 0.25 + k as f64 * 0.6).collect();
        write_clicks(&wav, &beats, 10.0);
        let (mut p, t) = project_with(&wav);
        set_mode(&mut p, t, 0, FlexMode::Rhythmic);
        p.tracks[t].regions[0].start_bar = 2.0 - (0.25 / 0.6 / 4.0) as f32;
        if let Some(f) = p.tracks[t].regions[0]
            .audio
            .as_mut()
            .and_then(|a| a.flex.as_mut())
        {
            f.follow = Some(TempoFollow {
                one_secs: 0.25,
                beats,
                first: 0,
                bpm: 100.0,
            });
        }
        settle(&mut p, s);
        (p, t)
    }

    /// Whether every Follow Tempo region's markers are the ones for the project's tempo now.
    fn followed_now(p: &Project) -> bool {
        let mut q = p.clone();
        (0..q.tracks.len()).all(|t| (0..q.tracks[t].regions.len()).all(|r| !follow(&mut q, t, r)))
    }

    /// Where beat `k` of the loop (quarter note 4 + k) is heard in region `r`'s sound, against
    /// where the project has it, in seconds.
    fn beat_error(p: &Project, t: usize, r: usize, k: f64) -> f64 {
        let region = &p.tracks[t].regions[r];
        let a = region.audio.as_ref().unwrap();
        let x = a.file.read_stereo().unwrap();
        let map = p.tempo_map();
        let s0 = map.seconds_at(p.region_start_beats(region)) - region.params.offset_secs;
        let want = map.seconds_at(4.0 + k) - s0;
        let i0 = ((want - 0.01) * 44_100.0) as usize;
        let i = (i0..i0 + 882)
            .find(|&i| x[i * 2].abs() > 0.3)
            .unwrap_or_else(|| panic!("no beat {k} near {want} s"));
        i as f64 / 44_100.0 - want
    }

    /// Review id 10: the right half of a cut Follow Tempo region starts seconds into its sound.
    /// After a tempo change it plays the sound it has from the place it had until the sound for
    /// the new tempo is made (never the old sound from the new sound's place), then the new one
    /// from its new place, every beat on the project's.
    #[test]
    fn a_cut_follow_region_keeps_its_place_until_its_new_sound_is_made() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = UiState::new();
        let (mut p, t) = follow_project(dir.path(), &mut s);
        assert!(p.split_region(t, 0, 8.0));
        settle(&mut p, &mut s);
        let r = &p.tracks[t].regions[1];
        let (file, offset) = (
            r.audio.as_ref().unwrap().file.path.clone(),
            r.params.offset_secs,
        );
        assert!(offset > 1.0, "{offset}");
        assert!(beat_error(&p, t, 1, 6.0).abs() < 1e-3);
        p.tempo = 90.0;
        assert!(follow(&mut p, t, 1), "markers made for 90");
        let r = &p.tracks[t].regions[1];
        assert_eq!(r.audio.as_ref().unwrap().file.path, file, "not made yet");
        assert_eq!(
            r.params.offset_secs, offset,
            "the sound made for 120 plays from its own place"
        );
        settle(&mut p, &mut s);
        let r = &p.tracks[t].regions[1];
        assert_ne!(r.audio.as_ref().unwrap().file.path, file);
        assert!(r.params.offset_secs > offset, "{}", r.params.offset_secs);
        for k in [4.0, 6.0, 11.0, 15.0] {
            let e = beat_error(&p, t, 1, k);
            assert!(e.abs() < 1e-3, "beat {k} off {e}");
        }
        assert!(followed_now(&p));
    }

    /// Review id 11: a tempo dragged (or followed from a clock) changes every frame; no Flex
    /// render starts until it rests, and then one for the tempo it rests on.
    #[test]
    fn a_tempo_still_moving_starts_no_renders() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = UiState::new();
        let (mut p, t) = follow_project(dir.path(), &mut s);
        let ctx = egui::Context::default();
        for i in 0..12 {
            p.tempo = 100.0 + i as f32;
            poll(&ctx, &mut p, &mut s);
            assert!(
                s.flex.renders.is_empty(),
                "frame {i}: a render started for a tempo still moving"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        settle(&mut p, &mut s);
        let f = p.tracks[t].regions[0]
            .audio
            .as_ref()
            .unwrap()
            .flex
            .as_ref()
            .unwrap();
        assert_eq!(f.rendered, f.fingerprint());
        assert!(followed_now(&p));
        for k in [0.0, 7.0, 15.0] {
            assert!(beat_error(&p, t, 0, k).abs() < 1e-3);
        }
    }

    /// Review id 11: a render no region waits for any more is stopped and writes nothing; the
    /// one a region waits for goes on.
    #[test]
    fn a_render_nobody_waits_for_is_cancelled_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("r.wav");
        write_clicks(&wav, &[0.5, 1.0, 1.5], 2.0);
        let (mut p, t) = project_with(&wav);
        set_mode(&mut p, t, 0, FlexMode::Rhythmic);
        let f = p.tracks[t].regions[0]
            .audio
            .as_mut()
            .unwrap()
            .flex
            .as_mut()
            .unwrap();
        f.markers = markers_from([(0.5, 0.6), (1.0, 1.1)]);
        let edit = f.clone();
        let wanted = edit.fingerprint();
        let mut st = FlexState::default();
        let stale = wanted ^ 0x00ff_00ff_00ff_00ff;
        let mut keep = Vec::new();
        for fp in [wanted, stale] {
            let (tx, result) = channel();
            keep.push(tx);
            st.renders.push(Render {
                fingerprint: fp,
                result,
                cancel: Arc::new(AtomicBool::new(false)),
            });
        }
        cancel_stale(&p, &mut st);
        assert!(!st.renders[0].cancelled(), "a region waits for it");
        assert!(st.renders[1].cancelled(), "nobody waits for it");
        // A stopped render writes nothing (a fingerprint no sound was made for).
        let fp = stale ^ std::process::id() as u64;
        let path = cache_dir().join(format!("{fp:016x}.wav"));
        let _ = std::fs::remove_file(&path);
        let r = render_unless(&edit, fp, &AtomicBool::new(true));
        assert_eq!(r.err().as_deref(), Some(CANCELLED));
        assert!(!path.exists());
        // Not stopped, it is made.
        let made = render(&edit, fp).unwrap();
        assert_eq!(made.path, path);
        let _ = std::fs::remove_file(&path);
    }

    /// The end of a frame as the app has it (`App::track_history`): an edit is recorded for
    /// Undo.
    fn frame_end(h: &mut crate::undo::History, p: &mut Project, s: &mut UiState) {
        h.record_followed(p, &mut s.flex);
    }

    fn undo(h: &mut crate::undo::History, p: &mut Project, s: &mut UiState) -> bool {
        h.undo_followed(p, &mut s.flex)
    }

    fn redo(h: &mut crate::undo::History, p: &mut Project, s: &mut UiState) -> bool {
        h.redo_followed(p, &mut s.flex)
    }

    /// Review id 14: a tempo change (Session to Song Tempo, a typed tempo) and the Follow Tempo
    /// markers made for it are one Undo step; Undo and Redo never leave markers made for
    /// another tempo.
    #[test]
    fn undo_of_a_tempo_change_takes_its_follow_markers_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = UiState::new();
        let (mut p, t) = follow_project(dir.path(), &mut s);
        let mut h = crate::undo::History::new(&p);
        let at_120 = p.tracks[t].regions[0].clone();
        // The tempo changes in the window, and the frame ends.
        p.tempo = 90.0;
        frame_end(&mut h, &mut p, &mut s);
        // The frames after it: Flex catches up.
        settle(&mut p, &mut s);
        frame_end(&mut h, &mut p, &mut s);
        assert!(followed_now(&p));
        assert!(undo(&mut h, &mut p, &mut s));
        settle(&mut p, &mut s);
        assert!(
            followed_now(&p),
            "after Undo, at {} BPM, the markers are another tempo's",
            p.tempo
        );
        assert_eq!(p.tempo, 120.0, "one Undo takes the tempo change back");
        let f = |r: &Region| {
            r.audio
                .as_ref()
                .unwrap()
                .flex
                .as_ref()
                .unwrap()
                .markers
                .clone()
        };
        assert_eq!(f(&p.tracks[t].regions[0]), f(&at_120));
        assert!(redo(&mut h, &mut p, &mut s));
        settle(&mut p, &mut s);
        assert_eq!(p.tempo, 90.0);
        assert!(followed_now(&p));
    }

    #[test]
    fn a_region_without_audio_and_a_gone_region_change_nothing() {
        let mut p = Project::demo();
        let mut s = UiState::new();
        start(&p, &mut s, 0, 0, Command::CorrectPitch);
        assert!(!s.flex.busy());
        assert_eq!(
            s.status.as_deref(),
            Some("Flex: this region has no recorded audio")
        );
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("x.wav");
        write_clicks(&wav, &[0.5, 1.0], 2.0);
        let (mut q, t) = project_with(&wav);
        start(&q, &mut s, t, 0, Command::Quantize { grid: 1.0 });
        q.tracks[t].regions.clear();
        settle(&mut q, &mut s);
        assert!(q.tracks[t].regions.is_empty());
        settle(&mut p, &mut s);
    }

    /// Flex edits taken back until nothing moves: the region plays its recording again and its
    /// waveform is the recording's, not the overview of the sound made before.
    #[test]
    fn a_flex_edit_back_to_neutral_draws_the_recordings_waveform() {
        use crate::model::RegionParams;
        let original = AudioFile::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../prowave-engine/tests/fixtures/tone.caf"),
        )
        .unwrap();
        let made = AudioFile {
            path: "/prowave-flex-test/made.wav".into(),
            ..original.clone()
        };
        let stale = vec![(-0.9f32, 0.9f32); 7];
        let audio = AudioRegion {
            file: made,
            peaks: Default::default(),
            flex: Some(FlexEdit::new(original.clone(), FlexMode::Polyphonic)),
        };
        let _ = audio.peaks.set(stale.clone());
        let old = audio.peaks.clone();
        let mut p = Project::empty();
        p.tracks.clear();
        let mut t = Track::new("Audio 1", TrackKind::Audio);
        t.regions.push(Region {
            name: "r".into(),
            start_bar: 1.0,
            len_bars: 1.0,
            notes: Vec::new(),
            controls: Vec::new(),
            folder: None,
            generated: None,
            audio: Some(audio),
            params: RegionParams::default(),
        });
        p.tracks.push(t);
        let mut s = UiState::new();
        let ctx = egui::Context::default();
        start_renders(&ctx, &mut p, &mut s);
        let a = p.tracks[0].regions[0].audio.as_ref().unwrap();
        assert_eq!(a.file.path, original.path, "the recording plays again");
        assert!(a.flex.is_some(), "the Flex edit stays on the region");
        assert!(!std::sync::Arc::ptr_eq(&a.peaks, &old), "a new overview");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let fresh = loop {
            if let Some(v) = a.peaks.get() {
                break v.clone();
            }
            assert!(std::time::Instant::now() < deadline, "no overview made");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(fresh, original.peaks(fresh.len()).unwrap());
        assert_ne!(fresh, stale);
    }
}
