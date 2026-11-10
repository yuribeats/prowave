//! Pro Wave Tune in the app: what each running effect is told (the project's key and tuning,
//! the notes of its MIDI target), what it reports (the meters), and the Piano Roll's Pitch
//! Correction lane.
//!
//! The lane shows the pitch the track's audio has (an analysis of its audio regions, made in
//! the background with the effect's own tracker; on an instrument track, the pitch the effect
//! heard while playing) against the notes or the scale, and lets the pitch be drawn:
//! - on the note grid, the Pencil draws a Pitch Override (the note the effect tunes to there,
//!   whatever the scale says; Shift draws between the semitones) and the Eraser removes it;
//! - in the lane under the grid, the Pencil draws the Correction Amount and the Eraser
//!   removes it.
//!
//! Both are the effect's automation (its Pitch Override and Amount parameters), so they are
//! saved, undone and played back like any other automation.
//!
//! The lane appears for a MIDI region whose track has the effect, or whose notes an effect
//! follows (the MIDI track above the audio track that sings it, or a MIDI track chosen as an
//! effect's MIDI target); and, with no MIDI region to show, for an audio track with the
//! effect, on its own grid.
use crate::i18n::tr;
use crate::model::{Project, Track, TrackKind};
use crate::piano_tools::Tool;
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Response, Sense, Stroke, Ui};
use prowave_engine::tune::{self, TargetNote, TuneRemote, TuneTargets};
use prowave_engine::{AutoPoint, AutoTarget};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

/// The detected pitch's colour, the drawn override's, the Amount curve's. The grids are bone
/// (as the Piano Roll's): the analysed pitch is printed in ink with a bone edge, what the
/// effect heard live in print grey, what it made in deep LCD green, and what the user drew
/// (the override) in the signal orange.
pub const CURVE: Color32 = crate::theme::INK;
pub const LIVE: Color32 = crate::theme::PRINT;
/// What the effect made while playing.
pub const MADE: Color32 = crate::theme::LCD_INK;
pub const OVERRIDE: Color32 = crate::theme::ORANGE;
pub const AMOUNT: Color32 = crate::theme::ORANGE;
/// The width at a note's left kept for its name.
const LABEL_W: f32 = 26.0;
/// The Amount lane's scale, at the well's left.
const SCALE_W: f32 = 30.0;
/// The analysis hop, seconds.
const ANALYSIS_HOP: f64 = 0.005;
/// Live history kept per effect.
const HISTORY: usize = 6000;
/// The step between an override and "off" (beats).
const EPS: f64 = 1e-4;

/// Where an effect is: the track (or `mixer::MASTER_STRIP`) and its Audio FX slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub track: usize,
    pub slot: usize,
}

impl Link {
    fn target(self, param: usize) -> AutoTarget {
        AutoTarget::Plugin {
            slot: self.slot as u8 + 1,
            param: param as u32,
        }
    }
}

// ------------------------------------------------------------------ the running effects

struct Running {
    remote: TuneRemote,
    sent: Option<Arc<TuneTargets>>,
}

enum Analysis {
    Running(Receiver<Option<Vec<(f32, f32)>>>),
    /// (seconds in the file, MIDI note with a fraction; NaN unvoiced), the file's rate.
    Ready(Arc<Vec<(f32, f32)>>),
    Failed,
}

#[derive(Default)]
struct Runtime {
    running: HashMap<u64, Running>,
    targets: HashMap<u64, Arc<TuneTargets>>,
    analysis: HashMap<PathBuf, Analysis>,
    /// What each effect heard and made while playing: (beat, in, out).
    history: HashMap<u64, VecDeque<(f64, f32, f32)>>,
    /// The detected curve of a track in beats, and what it was made for.
    curves: HashMap<usize, (u64, Curve)>,
}

/// A pitch curve: (beat, MIDI note with a fraction; NaN where not voiced).
pub type Curve = Arc<Vec<(f64, f32)>>;

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Runtime) -> R) -> R {
    let mut guard = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Runtime::default))
}

/// A new engine instance for slot `uid` (its track was built again): told everything afresh.
pub fn attach(uid: u64, remote: TuneRemote) {
    with(|rt| {
        rt.running.insert(uid, Running { remote, sent: None });
    });
}

/// What the effect in slot `uid` hears and makes now: (input, output, target) as MIDI notes
/// with a fraction, NaN while not voiced. `None` when it is not running.
pub fn meter(uid: u64) -> Option<(f32, f32, f32)> {
    with(|rt| rt.running.get(&uid).map(|r| r.remote.meter.read()))
}

pub fn is_tune(slot: &crate::plugins::PluginSlot) -> bool {
    slot.id == tune::ID
}

/// The effect's parameter values in `slot`.
pub fn values(slot: &crate::plugins::PluginSlot) -> Vec<f32> {
    match prowave_engine::native_info(tune::ID) {
        Some(info) => crate::native_fx_ui::values(slot, info),
        None => Vec::new(),
    }
}

/// The first Pro Wave Tune that is on in `t`'s Audio FX, by slot.
pub fn tune_slot(t: &Track) -> Option<usize> {
    t.au_effects.iter().position(|s| is_tune(s) && !s.bypass)
}

/// Every Pro Wave Tune in the project: (track or `MASTER_STRIP`, slot, uid).
fn all_slots(p: &Project) -> Vec<(usize, usize, u64)> {
    let mut out = Vec::new();
    for (i, t) in p.tracks.iter().enumerate() {
        for (k, s) in t.au_effects.iter().enumerate() {
            if is_tune(s) {
                out.push((i, k, s.uid));
            }
        }
    }
    for (k, s) in p.master_au_effects.iter().enumerate() {
        if is_tune(s) {
            out.push((crate::mixer::MASTER_STRIP, k, s.uid));
        }
    }
    out
}

/// The root (C = 0) and mode of a key name such as "Ebmin" or "F#maj".
pub fn key_root(key: &str) -> (u8, bool) {
    let minor = key.ends_with("min");
    let mut chars = key.chars();
    let base: i32 = match chars.next() {
        Some('C') => 0,
        Some('D') => 2,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 7,
        Some('A') => 9,
        Some('B') => 11,
        _ => 0,
    };
    let alt = match chars.next() {
        Some('#') => 1,
        Some('b') => -1,
        _ => 0,
    };
    ((base + alt).rem_euclid(12) as u8, minor)
}

pub fn has_notes(t: &Track) -> bool {
    !matches!(
        t.kind,
        TrackKind::Audio
            | TrackKind::Bus
            | TrackKind::Output
            | TrackKind::Aux
            | TrackKind::Folder
            | TrackKind::Vca
    ) && t.regions.iter().any(|r| !r.is_audio())
}

/// The track whose notes the effect on track `ti` with `values` follows in MIDI mode: the
/// MIDI Track parameter's (1 = the first track), or, on Auto, the track whose singing this
/// track holds, the track itself if it has notes, else the nearest track above that has them.
pub fn midi_source(p: &Project, ti: usize, values: &[f32]) -> Option<usize> {
    let chosen = values.get(tune::MIDI_TRACK).copied().unwrap_or(0.0).round() as usize;
    if chosen > 0 {
        return p
            .tracks
            .get(chosen - 1)
            .filter(|t| has_notes(t))
            .map(|_| chosen - 1);
    }
    let t = p.tracks.get(ti)?;
    if has_notes(t) {
        return Some(ti);
    }
    (0..ti).rev().find(|i| has_notes(&p.tracks[*i]))
}

/// The notes of track `src` in beats from the project start, in order.
pub fn notes_of(p: &Project, src: usize) -> Vec<TargetNote> {
    let mut out = Vec::new();
    let Some(t) = p.tracks.get(src) else {
        return out;
    };
    for r in t.regions.iter().filter(|r| !r.is_audio() && !r.params.mute) {
        let origin = p.region_start_beats(r);
        let end = p.region_end_beats(r);
        for n in r.notes.iter().filter(|n| !n.mute) {
            let s = origin + f64::from(n.start);
            let e = (s + f64::from(n.len)).min(end);
            if s < end && e > s {
                out.push(TargetNote {
                    start: s,
                    end: e,
                    pitch: (i32::from(n.pitch) + i32::from(r.params.transpose)).clamp(0, 127) as u8,
                });
            }
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// What the effect on track `ti` with `values` should be told.
pub fn targets_for(p: &Project, ti: usize, values: &[f32]) -> TuneTargets {
    let (root, minor) = key_root(p.key);
    let midi = values.get(tune::TARGET).copied().unwrap_or(0.0).round() as i32 == tune::TARGET_MIDI;
    let notes = if midi && ti != crate::mixer::MASTER_STRIP {
        midi_source(p, ti, values)
            .map(|s| notes_of(p, s))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    TuneTargets {
        root,
        minor,
        a4: p.tuning_hz,
        notes,
    }
}

/// Once a frame: tells every running effect what changed, keeps what they heard while
/// playing, and takes in finished analyses.
pub fn frame(p: &Project, ctx: &egui::Context) {
    let slots = all_slots(p);
    with(|rt| {
        for (ti, k, uid) in &slots {
            let slot = if *ti == crate::mixer::MASTER_STRIP {
                p.master_au_effects.get(*k)
            } else {
                p.tracks.get(*ti).and_then(|t| t.au_effects.get(*k))
            };
            let Some(slot) = slot else { continue };
            let want = targets_for(p, *ti, &values(slot));
            if rt.targets.get(uid).is_none_or(|t| **t != want) {
                rt.targets.insert(*uid, Arc::new(want));
            }
        }
        rt.running.retain(|_, r| !r.remote.is_abandoned());
        for (uid, r) in rt.running.iter_mut() {
            let Some(t) = rt.targets.get(uid) else {
                continue;
            };
            if !r.sent.as_ref().is_some_and(|s| Arc::ptr_eq(s, t)) && r.remote.send(t.clone()) {
                r.sent = Some(t.clone());
            }
            if p.playing {
                let (i, o, _) = r.remote.meter.read();
                let h = rt.history.entry(*uid).or_default();
                if h.back().is_some_and(|b| p.position_beats < b.0) {
                    h.clear();
                }
                h.push_back((p.position_beats, i, o));
                while h.len() > HISTORY {
                    h.pop_front();
                }
            }
        }
        let live: Vec<u64> = slots.iter().map(|s| s.2).collect();
        rt.targets.retain(|uid, _| live.contains(uid));
        rt.history.retain(|uid, _| live.contains(uid));
        let mut done = false;
        for a in rt.analysis.values_mut() {
            if let Analysis::Running(rx) = a {
                match rx.try_recv() {
                    Ok(Some(v)) => {
                        *a = Analysis::Ready(Arc::new(v));
                        done = true;
                    }
                    Ok(None) | Err(TryRecvError::Disconnected) => *a = Analysis::Failed,
                    Err(TryRecvError::Empty) => {}
                }
            }
        }
        if done {
            rt.curves.clear();
            ctx.request_repaint();
        }
    });
}

/// The Pro Wave Tune that a MIDI region on track `ti` shows the correction of: the track's
/// own, else one that follows its notes.
pub fn link_for(p: &Project, ti: usize) -> Option<Link> {
    let t = p.tracks.get(ti)?;
    if let Some(k) = tune_slot(t) {
        return Some(Link { track: ti, slot: k });
    }
    p.tracks.iter().enumerate().find_map(|(i, o)| {
        let k = tune_slot(o)?;
        (i != ti && midi_source(p, i, &values(&o.au_effects[k])) == Some(ti))
            .then_some(Link { track: i, slot: k })
    })
}

/// The analysis of a file: starts it the first time; `None` until it is done.
fn analysis_of(rt: &mut Runtime, file: &prowave_engine::AudioFile) -> Option<Arc<Vec<(f32, f32)>>> {
    match rt.analysis.get(&file.path) {
        Some(Analysis::Ready(v)) => return Some(v.clone()),
        Some(_) => return None,
        None => {}
    }
    let (tx, rx) = channel();
    let f = file.clone();
    let spawned = std::thread::Builder::new()
        .name("prowave-tune-analysis".into())
        .spawn(move || {
            let result = f.read_stereo().ok().map(|st| {
                let mono: Vec<f32> = st
                    .chunks(2)
                    .map(|c| 0.5 * (c[0] + c.get(1).copied().unwrap_or(c[0])))
                    .collect();
                let rate = f.sample_rate.max(1);
                let hop = ((f64::from(rate) * ANALYSIS_HOP) as usize).max(1);
                tune::tracker::analyze(&mono, rate, hop, 3)
                    .into_iter()
                    .map(|fr| {
                        let m = if fr.hz > 0.0 {
                            tune::scale::hz_to_midi(fr.hz, 440.0)
                        } else {
                            f32::NAN
                        };
                        (fr.time as f32, m)
                    })
                    .collect()
            });
            let _ = tx.send(result);
        });
    rt.analysis.insert(
        file.path.clone(),
        if spawned.is_ok() {
            Analysis::Running(rx)
        } else {
            Analysis::Failed
        },
    );
    None
}

/// The pitch of track `ti`'s audio regions in beats (NaN where it is not voiced), for the
/// project's tuning; empty until the analysis is done.
pub fn curve(p: &Project, ti: usize) -> Arc<Vec<(f64, f32)>> {
    let Some(t) = p.tracks.get(ti) else {
        return Arc::default();
    };
    // What the curve depends on: the regions, the tempo (the song revision) and the tuning.
    let mut key = p.song_rev.wrapping_mul(31) ^ u64::from(p.tuning_hz.to_bits());
    for r in &t.regions {
        if let Some(a) = &r.audio {
            key = key
                .wrapping_mul(1_000_003)
                .wrapping_add(
                    u64::from(r.start_bar.to_bits()) ^ u64::from(r.len_bars.to_bits()) << 7,
                )
                .wrapping_add(r.params.offset_secs.to_bits())
                .wrapping_add(a.file.frames);
        }
    }
    with(|rt| {
        if let Some((k, c)) = rt.curves.get(&ti) {
            if *k == key {
                return c.clone();
            }
        }
        let map = p.tempo_map();
        let tune_cents = 12.0 * (p.tuning_hz / 440.0).log2();
        let mut out = Vec::new();
        let mut pending = false;
        for r in t.regions.iter().filter(|r| !r.params.mute) {
            let Some(a) = &r.audio else { continue };
            let Some(frames) = analysis_of(rt, &a.file) else {
                pending = true;
                continue;
            };
            let start = map.seconds_at(p.region_start_beats(r));
            let end = map.seconds_at(p.region_end_beats(r));
            let off = r.params.offset_secs;
            for (time, m) in frames.iter() {
                let at = start + (f64::from(*time) - off);
                if at < start || at > end {
                    continue;
                }
                out.push((map.beat_at(at), m - tune_cents));
            }
            out.push((f64::NAN, f32::NAN));
        }
        let c = Arc::new(out);
        if !pending {
            rt.curves.insert(ti, (key, c.clone()));
        }
        c
    })
}

/// What the effect in slot `uid` heard (and made) while playing: (beat, in, out).
fn history(uid: u64) -> Vec<(f64, f32, f32)> {
    with(|rt| {
        rt.history
            .get(&uid)
            .map(|h| h.iter().copied().collect())
            .unwrap_or_default()
    })
}

// ------------------------------------------------------------------ the lanes' data

/// The automation lane of parameter `param` of the effect at `link`, readied for points.
fn lane_mut(p: &mut Project, link: Link, param: usize) -> Option<&mut Vec<AutoPoint>> {
    let t = p.tracks.get_mut(link.track)?;
    let slot = t.au_effects.get(link.slot)?.clone();
    let target = link.target(param);
    let descs = crate::automation::plugin_params(&slot, link.slot as u8 + 1, None);
    let desc = descs.get(param)?.clone();
    t.auto.plugin_lane(&desc, &slot.id);
    Some(t.auto.lane_mut(target))
}

fn lane(p: &Project, link: Link, param: usize) -> &[AutoPoint] {
    p.tracks
        .get(link.track)
        .map_or(&[], |t| t.auto.lane(link.target(param)))
}

/// The Pitch Override lane's position for a MIDI note (0 is off).
fn override_pos(note: f32) -> f32 {
    (note * 100.0 / tune::PARAMS[tune::OVERRIDE].max).clamp(0.0, 1.0)
}

fn override_note(pos: f32) -> Option<f32> {
    let v = pos * tune::PARAMS[tune::OVERRIDE].max;
    (v >= 1.0).then_some(v / 100.0)
}

/// The override drawn: stretches (beat from, beat to, the note at each point).
pub fn override_spans(points: &[AutoPoint]) -> Vec<Vec<(f64, f32)>> {
    let mut out: Vec<Vec<(f64, f32)>> = Vec::new();
    let mut cur: Vec<(f64, f32)> = Vec::new();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        match (override_note(a.value), override_note(b.value)) {
            (Some(x), Some(y)) if b.beat - a.beat > EPS * 1.5 || (x - y).abs() < 0.01 => {
                if cur.is_empty() {
                    cur.push((a.beat, x));
                }
                cur.push((b.beat, y));
            }
            _ => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `before` with an override (or, `erase`, none) from `lo` to `hi`: `samples` are lane
/// positions by beat. Outside the stretch the lane plays as before, stepping in and out.
pub fn with_override(
    before: &[AutoPoint],
    samples: &[(f64, f32)],
    lo: f64,
    hi: f64,
    erase: bool,
) -> Vec<AutoPoint> {
    let at = |b: f64| prowave_engine::lane_value(before, b).unwrap_or(0.0);
    let (left, right) = (at(lo - 2.0 * EPS), at(hi + 2.0 * EPS));
    let mut out: Vec<AutoPoint> = before
        .iter()
        .filter(|q| q.beat < lo - EPS * 1.5 || q.beat > hi + EPS * 1.5)
        .copied()
        .collect();
    out.push(AutoPoint::new((lo - EPS).max(0.0), left));
    if erase || samples.is_empty() {
        out.push(AutoPoint::new(lo, 0.0));
        out.push(AutoPoint::new(hi, 0.0));
    } else {
        out.extend(samples.iter().map(|(b, v)| AutoPoint::new(*b, *v)));
    }
    out.push(AutoPoint::new(hi + EPS, right));
    out.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    // Runs of the same value keep their ends only.
    let mut thin: Vec<AutoPoint> = Vec::with_capacity(out.len());
    for (i, q) in out.iter().enumerate() {
        let same_before = thin.last().is_some_and(|l| l.value == q.value);
        let same_after = out.get(i + 1).is_some_and(|n| n.value == q.value);
        if same_before && same_after {
            continue;
        }
        thin.push(*q);
    }
    // A lane that is all off says nothing.
    if thin.iter().all(|q| q.value == 0.0) {
        thin.clear();
    }
    thin
}

/// `before` with the Amount drawn from `lo` to `hi` (`samples`), or with the points there
/// taken away (`erase`).
pub fn with_amount(
    before: &[AutoPoint],
    samples: &[(f64, f32)],
    lo: f64,
    hi: f64,
    erase: bool,
) -> Vec<AutoPoint> {
    let mut out: Vec<AutoPoint> = before
        .iter()
        .filter(|q| q.beat < lo || q.beat > hi)
        .copied()
        .collect();
    if !erase {
        out.extend(samples.iter().map(|(b, v)| AutoPoint::new(*b, *v)));
    }
    out.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    out
}

// ------------------------------------------------------------------ editing

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Override,
    Amount,
}

/// A stroke being drawn.
struct PenStroke {
    kind: Kind,
    erase: bool,
    link: Link,
    before: Vec<AutoPoint>,
    /// Lane positions by beat (in 1/4096 beat steps).
    samples: BTreeMap<i64, f32>,
    last: Option<(f64, f32)>,
    lo: f64,
    hi: f64,
}

/// The lane's view state (part of the UI state).
#[derive(Default)]
pub struct TuneView {
    stroke: Option<PenStroke>,
    /// The audio-track editor's view: first beat shown, pixels per beat, the pitch at the
    /// middle of the grid, pixels per semitone.
    pub scroll_beat: f64,
    pub beat_px: f32,
    pub mid_pitch: f32,
    pub key_h: f32,
    pub for_track: Option<usize>,
}

/// Where things are on a pitch grid.
pub struct Geom<'a> {
    pub grid: Rect,
    /// Absolute beat to x, and back.
    pub x_of: &'a dyn Fn(f64) -> f32,
    pub beat_at: &'a dyn Fn(f32) -> f64,
    /// MIDI note (with a fraction) to y, and back.
    pub y_of: &'a dyn Fn(f32) -> f32,
    pub pitch_at: &'a dyn Fn(f32) -> f32,
}

fn key(b: f64) -> i64 {
    (b * 4096.0).round() as i64
}

impl PenStroke {
    /// Adds the point (beat, lane position), filling the way from the last one.
    fn add(&mut self, beat: f64, pos: f32) {
        let beat = beat.max(0.0);
        if let Some((b0, v0)) = self.last {
            let steps = (((beat - b0).abs() * 64.0).ceil() as usize).clamp(1, 4096);
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let b = b0 + (beat - b0) * t;
                self.samples.insert(key(b), v0 + (pos - v0) * t as f32);
            }
        } else {
            self.samples.insert(key(beat), pos);
        }
        self.last = Some((beat, pos));
        self.lo = self.lo.min(beat);
        self.hi = self.hi.max(beat);
    }

    fn points(&self) -> Vec<(f64, f32)> {
        let raw: Vec<AutoPoint> = self
            .samples
            .iter()
            .map(|(k, v)| AutoPoint::new(*k as f64 / 4096.0, *v))
            .collect();
        crate::automation::thin(&raw, 0.0004)
            .into_iter()
            .map(|q| (q.beat, q.value))
            .collect()
    }

    fn apply(&self, p: &mut Project) {
        let pts = self.points();
        let (lo, hi) = (self.lo, self.hi.max(self.lo + 1.0 / 64.0));
        let new = match self.kind {
            Kind::Override => with_override(&self.before, &pts, lo, hi, self.erase),
            Kind::Amount => with_amount(&self.before, &pts, lo, hi, self.erase),
        };
        let param = match self.kind {
            Kind::Override => tune::OVERRIDE,
            Kind::Amount => tune::AMOUNT,
        };
        if let Some(l) = lane_mut(p, self.link, param) {
            *l = new;
        }
    }
}

/// Handles a drag on the grid (`resp`) as an override stroke: drawn, or erased.
pub fn edit_grid(
    ui: &Ui,
    p: &mut Project,
    v: &mut TuneView,
    link: Link,
    resp: &Response,
    g: &Geom,
    erase: bool,
) {
    let shift = ui.input(|i| i.modifiers.shift);
    if resp.drag_started() {
        v.stroke = Some(PenStroke {
            kind: Kind::Override,
            erase,
            link,
            before: lane(p, link, tune::OVERRIDE).to_vec(),
            samples: BTreeMap::new(),
            last: None,
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
        });
    }
    let pt = resp
        .interact_pointer_pos()
        .or_else(|| ui.input(|i| i.pointer.press_origin()));
    if let (Some(st), Some(pt)) = (v.stroke.as_mut(), pt) {
        if st.kind == Kind::Override && (resp.dragged() || resp.drag_started()) {
            let mut note = (g.pitch_at)(pt.y.clamp(g.grid.top(), g.grid.bottom()));
            if !shift {
                note = note.round();
            }
            st.add((g.beat_at)(pt.x), override_pos(note.clamp(1.0, 126.0)));
            st.apply(p);
        }
    }
    if resp.drag_stopped() {
        if let Some(st) = v.stroke.take() {
            st.apply(p);
        }
    }
}

/// Where a note's name is printed on a note `r` (its left end), or None when the note is too
/// small to carry one. The pitch curve leaves this spot clear so the name stays readable.
fn label_spot(r: Rect) -> Option<Rect> {
    (r.height() >= 10.0 && r.width() >= 28.0)
        .then(|| Rect::from_min_max(r.min, pos2(r.left() + LABEL_W, r.bottom())))
}

/// The notes the effect follows (its MIDI target), if it follows any.
fn target_notes(p: &Project, link: Link) -> Vec<TargetNote> {
    let Some(slot) = p
        .tracks
        .get(link.track)
        .and_then(|t| t.au_effects.get(link.slot))
    else {
        return Vec::new();
    };
    let vals = values(slot);
    midi_source(p, link.track, &vals).map_or_else(Vec::new, |src| notes_of(p, src))
}

/// Draws the detected pitch, what the effect heard live, and the override, on the grid.
/// The pitch is printed ink with a bone edge, so it reads on the bone floor and on the black
/// note caps alike; it stays clear of the notes' names.
pub fn draw_grid(g: &Painter, p: &Project, link: Link, geom: &Geom) {
    let clip = geom.grid;
    let kh = ((geom.y_of)(0.0) - (geom.y_of)(1.0)).abs();
    let spots: Vec<Rect> = target_notes(p, link)
        .iter()
        .filter_map(|n| {
            let y = (geom.y_of)(f32::from(n.pitch));
            label_spot(Rect::from_min_max(
                pos2((geom.x_of)(n.start), y - kh / 2.0),
                pos2((geom.x_of)(n.end), y + kh / 2.0),
            ))
        })
        .collect();
    let line = |pts: &[(f64, f32)], strokes: &[Stroke], gap: f64, avoid: bool| {
        let mut runs: Vec<Vec<Pos2>> = Vec::new();
        let mut run: Vec<Pos2> = Vec::new();
        let mut last_b = f64::NAN;
        for (b, m) in pts {
            let x = if b.is_finite() {
                (geom.x_of)(*b)
            } else {
                f32::NAN
            };
            let inside = x >= clip.left() - 50.0 && x <= clip.right() + 50.0;
            let joined = (b - last_b).abs() <= gap;
            let pt = pos2(x, (geom.y_of)(*m));
            let hidden = avoid && m.is_finite() && spots.iter().any(|s| s.contains(pt));
            if !m.is_finite() || !inside || !joined || hidden {
                if run.len() > 1 {
                    runs.push(std::mem::take(&mut run));
                }
                run.clear();
            }
            last_b = if m.is_finite() { *b } else { f64::NAN };
            if m.is_finite() && inside && !hidden {
                run.push(pt);
            }
        }
        if run.len() > 1 {
            runs.push(run);
        }
        // Every run's edge first, then every run's core, so crossings stay clean.
        for st in strokes {
            for r in &runs {
                g.add(egui::Shape::line(r.clone(), *st));
            }
        }
    };
    let edge = |w: f32| Stroke::new(w, crate::theme::BONE_HI.gamma_multiply(0.85));
    // The track's audio, as analysed.
    let c = curve(p, link.track);
    if !c.is_empty() {
        line(&c, &[edge(3.6), Stroke::new(1.6, CURVE)], 0.25, true);
    }
    // What the effect heard and made while playing (instrument tracks, and fresh takes).
    if let Some(uid) = p
        .tracks
        .get(link.track)
        .and_then(|t| t.au_effects.get(link.slot))
        .map(|s| s.uid)
    {
        let h = history(uid);
        if !h.is_empty() {
            let heard: Vec<(f64, f32)> = h.iter().map(|(b, i, _)| (*b, *i)).collect();
            let made: Vec<(f64, f32)> = h.iter().map(|(b, _, o)| (*b, *o)).collect();
            if c.is_empty() {
                line(&heard, &[edge(3.0), Stroke::new(1.2, LIVE)], 0.25, true);
            }
            line(&made, &[edge(3.2), Stroke::new(1.4, MADE)], 0.25, true);
        }
    }
    // The override, thick, with an ink edge and a handle where it starts.
    for span in override_spans(lane(p, link, tune::OVERRIDE)) {
        line(
            &span,
            &[
                Stroke::new(4.6, crate::theme::INK.gamma_multiply(0.7)),
                Stroke::new(3.0, OVERRIDE),
            ],
            f64::INFINITY,
            false,
        );
        if let Some((b, m)) = span.first() {
            let c = pos2((geom.x_of)(*b), (geom.y_of)(*m));
            g.circle(c, 3.5, OVERRIDE, Stroke::new(1.0, crate::theme::INK));
        }
    }
}

/// The Correction Amount lane in `rect`: drawn with the Pencil, erased with the Eraser. A
/// recessed well with a 0/50/100 scale, the bars, and the amount as an orange line with its
/// points as handles. Its name goes in the lane's header column, outside the well.
#[allow(clippy::too_many_arguments)]
pub fn amount_lane(
    ui: &mut Ui,
    p: &mut Project,
    v: &mut TuneView,
    link: Link,
    rect: Rect,
    x_of: &dyn Fn(f64) -> f32,
    beat_at: &dyn Fn(f32) -> f64,
    tool: Tool,
) {
    let resp = ui
        .interact(
            rect,
            egui::Id::unique("tune_amount").with((link.track, link.slot)),
            Sense::click_and_drag(),
        )
        .on_hover_text(tr("Correction Amount"));
    let r = rect.shrink2(vec2(0.0, 8.0));
    let erase = tool == Tool::Eraser;
    if matches!(tool, Tool::Pencil | Tool::Eraser) {
        if resp.drag_started() {
            v.stroke = Some(PenStroke {
                kind: Kind::Amount,
                erase,
                link,
                before: lane(p, link, tune::AMOUNT).to_vec(),
                samples: BTreeMap::new(),
                last: None,
                lo: f64::INFINITY,
                hi: f64::NEG_INFINITY,
            });
        }
        if let (Some(st), Some(pt)) = (v.stroke.as_mut(), resp.interact_pointer_pos()) {
            if st.kind == Kind::Amount && (resp.dragged() || resp.drag_started()) {
                let pos = ((r.bottom() - pt.y) / r.height()).clamp(0.0, 1.0);
                st.add(beat_at(pt.x), pos);
                st.apply(p);
            }
        }
        if resp.drag_stopped() {
            if let Some(st) = v.stroke.take() {
                st.apply(p);
            }
        }
    }
    let g = ui.painter_at(rect.expand(1.0));
    let ink = |a: f32| crate::theme::INK.gamma_multiply(a);
    crate::hw::recess(&g, rect, 3.0);
    let y_of = |pos: f32| r.bottom() - pos * r.height();
    let left = rect.left() + SCALE_W;
    // The scale's lines, and the bars.
    for (f, a) in [(0.0f32, 0.14), (0.5, 0.08), (1.0, 0.14)] {
        g.hline(left..=rect.right(), y_of(f), Stroke::new(1.0, ink(a)));
    }
    let first = p.beats_to_bar(beat_at(left)).floor().max(0.0) as i64;
    let last = p.beats_to_bar(beat_at(rect.right())).ceil() as i64;
    for bar in first..=last {
        let x = x_of(p.bar_to_beats(bar as f64));
        if x > left && x < rect.right() - 1.0 {
            g.vline(
                x,
                rect.top() + 2.0..=rect.bottom() - 1.0,
                Stroke::new(1.0, ink(0.16)),
            );
        }
    }
    crate::hw::scale_v(
        &g,
        Rect::from_min_max(
            pos2(rect.left() + 3.0, r.top()),
            pos2(left - 2.0, r.bottom()),
        ),
        &[(0.0, "0"), (0.5, "50"), (1.0, "100")],
        true,
    );
    let pts = lane(p, link, tune::AMOUNT);
    let knob = p
        .tracks
        .get(link.track)
        .and_then(|t| t.au_effects.get(link.slot))
        .map_or(1.0, |s| {
            values(s).get(tune::AMOUNT).copied().unwrap_or(100.0) / 100.0
        });
    let gl = g.with_clip_rect(Rect::from_min_max(
        pos2(left, rect.top()),
        rect.max + vec2(1.0, 1.0),
    ));
    if pts.is_empty() {
        gl.hline(
            left..=rect.right(),
            y_of(knob),
            Stroke::new(1.5, AMOUNT.gamma_multiply(0.6)),
        );
    } else {
        let mut line: Vec<Pos2> = Vec::new();
        let mut handles: Vec<Pos2> = Vec::new();
        line.push(pos2(left, y_of(pts[0].value)));
        for q in pts {
            let x = x_of(q.beat);
            if x > left - 2.0 && x < rect.right() + 2.0 {
                line.push(pos2(x, y_of(q.value)));
                handles.push(pos2(x, y_of(q.value)));
            }
        }
        if let Some(l) = pts.last() {
            line.push(pos2(rect.right(), y_of(l.value)));
        }
        line.sort_by(|a, b| a.x.total_cmp(&b.x));
        gl.add(egui::Shape::line(line, Stroke::new(2.0, AMOUNT)));
        for h in handles {
            let hr = Rect::from_center_size(h, vec2(6.0, 6.0));
            gl.rect_filled(hr, 1.0, AMOUNT);
            gl.rect_stroke(
                hr,
                1.0,
                Stroke::new(1.0, crate::theme::INK),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// The name of a lane in its header column `r` (left of the lane): a silkscreened word and,
/// under it, what the lane holds.
pub fn lane_header(g: &Painter, r: Rect, name: &str, holds: &str) {
    use crate::hw;
    use crate::theme::{Face, INK2, PRINT};
    let n = hw::fit(g, &name.to_uppercase(), Face::Silk, 11.0, r.width() - 8.0);
    hw::text(
        g,
        r.center() - vec2(0.0, 7.0),
        Align2::CENTER_CENTER,
        &n,
        Face::Silk,
        11.0,
        INK2,
    );
    let h = hw::fit(g, &holds.to_uppercase(), Face::Silk, 9.0, r.width() - 6.0);
    hw::text(
        g,
        r.center() + vec2(0.0, 7.0),
        Align2::CENTER_CENTER,
        &h,
        Face::Silk,
        9.0,
        PRINT,
    );
}

// ------------------------------------------------------------------ an audio track's own grid

/// The scale lines the effect corrects to on the grid (true for a note of the scale).
fn in_scale(p: &Project, link: Link, note: i32) -> bool {
    let v = p
        .tracks
        .get(link.track)
        .and_then(|t| t.au_effects.get(link.slot))
        .map(values)
        .unwrap_or_default();
    let get = |k: usize| v.get(k).copied().unwrap_or(tune::PARAMS[k].default) as i32;
    let (root, minor) = key_root(p.key);
    let mask = tune::scale::mask(
        get(tune::KEY),
        get(tune::SCALE),
        get(tune::NOTES) as u16,
        root,
        minor,
    );
    mask & (1 << note.rem_euclid(12)) != 0
}

/// With no MIDI region to show: the Pitch Correction of the selected audio track, if it has
/// Pro Wave Tune, on a grid of its own. Built from the Piano Roll's parts (inspector, keys,
/// ruler, region strip, bone grid, black note caps, playhead) so the two editors match.
/// Returns whether it was shown.
pub fn audio_editor(ui: &mut Ui, p: &mut Project, s: &mut crate::UiState, body: Rect) -> bool {
    use crate::hw;
    use crate::piano_roll as pr;
    use crate::theme::{self, Face};
    let ti = p.selected_track;
    let Some(t) = p.tracks.get(ti) else {
        return false;
    };
    if t.kind != TrackKind::Audio {
        return false;
    }
    let Some(k) = tune_slot(t) else { return false };
    let link = Link { track: ti, slot: k };
    let tool = s.piano.tool;
    let v = &mut s.tune;
    if v.for_track != Some(ti) || v.beat_px <= 0.0 {
        v.for_track = Some(ti);
        v.beat_px = 60.0;
        v.key_h = 12.0;
        v.scroll_beat = p.tracks[ti]
            .regions
            .first()
            .map_or(0.0, |r| p.region_start_beats(r));
        // Centre on the sung pitch.
        let c = curve(p, ti);
        let mut ms: Vec<f32> = c.iter().map(|x| x.1).filter(|m| m.is_finite()).collect();
        ms.sort_by(|a, b| a.total_cmp(b));
        v.mid_pitch = ms.get(ms.len() / 2).copied().unwrap_or(60.0);
    }
    // The Piano Roll's measures.
    const INSP_W: f32 = 248.0;
    const KEYS: f32 = 60.0;
    const RULER: f32 = 24.0;
    const STRIP: f32 = 18.0;
    const LANE: f32 = 76.0;
    let g = ui.painter_at(body);
    hw::chassis(&g, body);
    let insp = Rect::from_min_size(body.min, vec2(INSP_W, body.height()));
    let editor = Rect::from_min_max(pos2(insp.right(), body.top()), body.max);
    let lane_r = Rect::from_min_max(
        pos2(editor.left() + KEYS, editor.bottom() - LANE - 6.0),
        pos2(editor.right() - 8.0, editor.bottom() - 6.0),
    );
    // Keys, grid and lane share one bottom edge with one seam between them.
    let lower = lane_r.top() - 6.0;
    let keys = Rect::from_min_max(
        pos2(editor.left(), editor.top() + RULER + STRIP),
        pos2(editor.left() + KEYS, lower),
    );
    let grid = Rect::from_min_max(pos2(keys.right(), keys.top()), pos2(lane_r.right(), lower));
    let ruler = Rect::from_min_max(
        pos2(grid.left(), editor.top()),
        pos2(grid.right(), editor.top() + RULER),
    );
    let strip = Rect::from_min_max(
        pos2(grid.left(), ruler.bottom()),
        pos2(grid.right(), grid.top()),
    );
    let lane_head = Rect::from_min_max(
        pos2(editor.left(), lane_r.top()),
        pos2(lane_r.left(), lane_r.bottom()),
    );
    // Wheel: scroll; Command: zoom in time; Shift: sideways.
    if ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pt| editor.contains(pt))
    {
        let (d, m) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers));
        if m.command && d.y != 0.0 {
            v.beat_px = (v.beat_px * (1.0 + d.y * 0.003)).clamp(8.0, 600.0);
        } else if m.shift {
            v.scroll_beat = (v.scroll_beat - f64::from((d.y + d.x) / v.beat_px)).max(0.0);
        } else {
            v.scroll_beat = (v.scroll_beat - f64::from(d.x / v.beat_px)).max(0.0);
            v.mid_pitch = (v.mid_pitch + d.y / v.key_h).clamp(12.0, 115.0);
        }
    }
    let (scroll, bpx, mid, kh) = (v.scroll_beat, v.beat_px, v.mid_pitch, v.key_h);
    let x_of = move |b: f64| grid.left() + ((b - scroll) as f32) * bpx;
    let beat_at = move |x: f32| scroll + f64::from((x - grid.left()) / bpx);
    let y_of = move |m: f32| grid.center().y - (m - mid) * kh;
    let pitch_at = move |y: f32| mid + (grid.center().y - y) / kh;
    let geom = Geom {
        grid,
        x_of: &x_of,
        beat_at: &beat_at,
        y_of: &y_of,
        pitch_at: &pitch_at,
    };
    let black = |n: i32| matches!(n.rem_euclid(12), 1 | 3 | 6 | 8 | 10);

    // ---- The grid: key rows on the recess (the scale's notes lit, the others a shade
    // deeper), octave lines, beat and bar lines.
    let gg = ui.painter_at(grid);
    gg.rect_filled(grid, 0.0, theme::BONE2);
    let lo = pitch_at(grid.bottom()).floor() as i32;
    let hi = pitch_at(grid.top()).ceil() as i32;
    for n in lo..=hi {
        let y = y_of(n as f32);
        let row = Rect::from_center_size(pos2(grid.center().x, y), vec2(grid.width(), kh));
        let fill = if !in_scale(p, link, n) {
            hw::lerp(theme::BONE3, theme::SEAM, 0.45)
        } else if black(n) {
            hw::lerp(theme::BONE2, theme::BONE3, 0.62)
        } else {
            theme::BONE2
        };
        gg.rect_filled(row, 0.0, fill);
        if n.rem_euclid(12) == 0 {
            gg.hline(
                grid.x_range(),
                row.bottom() - 0.5,
                Stroke::new(1.0, pr::ink(0.2)),
            );
        }
    }
    let b0 = beat_at(grid.left()).floor() as i64;
    let b1 = beat_at(grid.right()).ceil() as i64;
    for b in b0..=b1 {
        gg.vline(
            x_of(b as f64),
            grid.y_range(),
            Stroke::new(1.0, pr::ink(0.09)),
        );
    }
    let first = p.beats_to_bar(beat_at(grid.left())).floor().max(0.0) as i64;
    let last = p.beats_to_bar(beat_at(grid.right())).ceil() as i64;
    for bar in first..=last {
        let x = x_of(p.bar_to_beats(bar as f64));
        gg.vline(x, grid.y_range(), Stroke::new(1.0, pr::ink(0.26)));
    }

    // ---- The notes it follows in MIDI mode: black caps named at their left end.
    let vals = values(&p.tracks[ti].au_effects[k]);
    if vals.get(tune::TARGET).copied().unwrap_or(0.0).round() as i32 == tune::TARGET_MIDI {
        if let Some(src) = midi_source(p, ti, &vals) {
            for n in notes_of(p, src) {
                let (a, b) = (x_of(n.start), x_of(n.end));
                if b < grid.left() || a > grid.right() {
                    continue;
                }
                let y = y_of(f32::from(n.pitch));
                let r = Rect::from_min_max(
                    pos2(a.round() + 0.5, (y - kh / 2.0).round() + 1.0),
                    pos2(b.round() - 0.5, (y + kh / 2.0).round() - 1.0),
                );
                let print = pr::paint_note(&gg, r, 100, false, false, false);
                if label_spot(r).is_some() {
                    hw::text(
                        &gg.with_clip_rect(r.intersect(grid)),
                        pos2(r.left() + 4.0, r.center().y - 0.5),
                        Align2::LEFT_CENTER,
                        &tune::scale::note_name(i32::from(n.pitch)),
                        Face::Bold,
                        (kh * 0.75).clamp(7.5, 9.5),
                        print,
                    );
                }
            }
        }
    }

    // ---- Drawing on the grid: the Pencil draws the override, the Eraser (or Option) erases.
    let resp = ui
        .interact(
            grid,
            egui::Id::unique("tune_grid").with(ti),
            Sense::click_and_drag(),
        )
        .on_hover_text(tr(
            "Pitch Correction: draw a pitch with the Pencil; Option or the Eraser erases",
        ));
    let alt = ui.input(|i| i.modifiers.alt);
    if matches!(tool, Tool::Pencil | Tool::Eraser | Tool::Pointer) {
        edit_grid(
            ui,
            p,
            &mut s.tune,
            link,
            &resp,
            &geom,
            tool == Tool::Eraser || alt,
        );
    }
    draw_grid(&gg, p, link, &geom);
    if curve(p, ti).is_empty() {
        hw::text(
            &gg,
            grid.center(),
            Align2::CENTER_CENTER,
            tr("Analysing the pitch of this track…"),
            Face::Bold,
            12.0,
            theme::INK2,
        );
    }

    // ---- The keyboard: bone keys with seams, black caps; the Cs named.
    let gk = ui.painter_at(keys);
    gk.rect_filled(keys, 0.0, theme::SEAM);
    for n in lo..=hi {
        let r = Rect::from_center_size(pos2(keys.center().x, y_of(n as f32)), vec2(KEYS, kh));
        // Two white keys meet (E over F, B over C): the row under is a white key too.
        let seam_below = !black(n) && !black(n - 1);
        pr::paint_piano_key(&gk, r, black(n), false, seam_below);
        if n.rem_euclid(12) == 0 {
            hw::text(
                &gk,
                pos2(keys.right() - 5.0, r.center().y),
                Align2::RIGHT_CENTER,
                &tune::scale::note_name(n),
                Face::Bold,
                (kh * 0.75).clamp(7.5, 9.0),
                theme::INK2,
            );
        }
    }
    hw::seam_v(&gk, keys.right() - 2.0, keys.y_range());

    // ---- Ruler and region strip.
    let head = Rect::from_min_max(
        pos2(editor.left(), editor.top()),
        pos2(editor.right(), grid.top()),
    );
    let gr = ui.painter_at(head);
    hw::chassis(&gr, head);
    let gr = gr.with_clip_rect(Rect::from_min_max(ruler.min, strip.max));
    let regions: Vec<(f32, f32, String, bool)> = p.tracks[ti]
        .regions
        .iter()
        .map(|r| {
            (
                x_of(p.region_start_beats(r)),
                x_of(p.region_end_beats(r)),
                r.name.clone(),
                r.params.mute,
            )
        })
        .collect();
    for (a, b, _, _) in &regions {
        let span = Rect::from_min_max(
            pos2(a.max(ruler.left()), ruler.top()),
            pos2(b.min(ruler.right()), ruler.bottom()),
        );
        if span.width() > 0.0 {
            gr.rect_filled(span, 0.0, theme::BONE_HI);
            gr.hline(
                span.x_range(),
                span.bottom() - 1.0,
                Stroke::new(2.0, theme::INK2),
            );
        }
    }
    if bpx >= 6.0 {
        for b in b0..=b1 {
            let x = x_of(b as f64);
            gr.vline(
                x,
                ruler.bottom() - 6.0..=ruler.bottom(),
                Stroke::new(1.0, pr::ink(0.4)),
            );
        }
    }
    for bar in first..=last {
        let x = x_of(p.bar_to_beats(bar as f64));
        gr.vline(
            x,
            ruler.top() + 4.0..=ruler.bottom(),
            Stroke::new(1.0, theme::INK2),
        );
        hw::text(
            &gr,
            pos2(x + 4.0, ruler.top() + 10.0),
            Align2::LEFT_CENTER,
            &pr::bar_label(bar.max(0) as u32 + 1),
            Face::Lcd,
            11.0,
            theme::INK,
        );
    }
    hw::seam_h(&gr, head.x_range(), strip.top() - 1.0);
    let color = p.tracks[ti].color;
    for (a, b, name, muted) in &regions {
        let r = Rect::from_min_max(
            pos2(a.max(strip.left()), strip.top() + 2.0),
            pos2(b.min(strip.right()), strip.bottom() - 1.0),
        );
        if r.width() > 0.0 {
            let look = theme::region_look(color, true, true, *muted);
            pr::region_header(&gr, r, &look, "AU", name);
        }
    }
    hw::seam_h(&g, editor.x_range(), grid.top() - 2.0);

    // ---- Playhead, over the ruler and the grid, its flag on the ruler.
    let px = x_of(p.position_beats);
    if px >= grid.left() && px <= grid.right() {
        let gp = ui.painter_at(Rect::from_min_max(ruler.min, grid.max));
        pr::playhead(
            &gp,
            &gp,
            px,
            Rect::from_min_max(pos2(grid.left(), ruler.bottom()), grid.max),
            ruler.top() + 1.0,
        );
    }

    // ---- The Correction Amount lane, under one seam, its name in the header column.
    hw::seam_h(&g, editor.left()..=editor.right(), lower + 2.0);
    lane_header(&g, lane_head, tr("Tune"), tr("Correction"));
    hw::seam_v(&g, lane_head.right() - 2.0, lane_head.y_range());
    let tool_lane = if alt { Tool::Eraser } else { tool };
    amount_lane(
        ui,
        p,
        &mut s.tune,
        link,
        lane_r,
        &x_of,
        &beat_at,
        if tool_lane == Tool::Pointer {
            Tool::Pencil
        } else {
            tool_lane
        },
    );

    // ---- The inspector: the region and the effect's settings, as the Piano Roll's.
    inspector(ui, p, link, insp);
    true
}

/// The audio editor's inspector: the region (as the Piano Roll's) and what Pro Wave Tune
/// corrects to.
fn inspector(ui: &Ui, p: &Project, link: Link, insp: Rect) {
    use crate::hw;
    use crate::theme::{self, Face};
    let gi = ui.painter_at(insp);
    gi.rect_filled(insp, 0.0, theme::TOOLBAR);
    hw::seam_v(&gi, insp.right() - 3.0, insp.y_range());
    hw::seam_v(&gi, insp.right() - 2.0, insp.y_range());
    let t = &p.tracks[link.track];
    let ix = insp.left() + 14.0;
    let iw = insp.width() - 30.0;
    let mut y = insp.top() + 14.0;
    let pos = p.position_beats;
    let region = t
        .regions
        .iter()
        .find(|r| pos >= p.region_start_beats(r) && pos < p.region_end_beats(r))
        .or_else(|| t.regions.first());
    hw::tri_down(&gi, pos2(ix + 4.0, y + 1.0), theme::INK);
    let head = hw::text(
        &gi,
        pos2(ix + 14.0, y),
        Align2::LEFT_CENTER,
        &crate::piano_roll::caption(tr("Region:")).to_uppercase(),
        Face::Silk,
        14.0,
        theme::INK,
    );
    if let Some(r) = region {
        let room = ix + iw - head.right() - 10.0;
        let name = hw::fit(&gi, &r.name.to_uppercase(), Face::Silk, 14.0, room);
        hw::text(
            &gi,
            pos2(head.right() + 10.0, y),
            Align2::LEFT_CENTER,
            &name,
            Face::Silk,
            14.0,
            theme::PRINT,
        );
    }
    y += 20.0;
    let line = crate::i18n::trf("on Track {}", &[&t.name]).to_uppercase();
    let line = hw::fit(&gi, &line, Face::Silk, 11.0, iw - 14.0);
    hw::text(
        &gi,
        pos2(ix + 14.0, y),
        Align2::LEFT_CENTER,
        &line,
        Face::Silk,
        11.0,
        theme::PRINT,
    );
    y += 16.0;
    hw::seam_h(&gi, insp.left()..=insp.right() - 3.0, y);
    y += 20.0;

    let vals = values(&t.au_effects[link.slot]);
    let get = |k: usize| vals.get(k).copied().unwrap_or(tune::PARAMS[k].default);
    let choice = |k: usize, names: &[&str]| -> String {
        names
            .get(get(k).round().max(0.0) as usize)
            .map_or_else(String::new, |s| tr(s).to_uppercase())
    };
    hw::bracket(&gi, ix, ix + iw, y, tr("Correction"));
    y += 18.0;
    let row = |y: f32, label: &str| {
        hw::silk(
            &gi,
            pos2(ix + 84.0, y + 12.0),
            Align2::RIGHT_CENTER,
            label,
            11.0,
            theme::INK2,
        );
        Rect::from_min_size(pos2(ix + 92.0, y), vec2(iw - 92.0, 24.0))
    };
    let key = if get(tune::KEY).round() as i32 == 0 {
        let (root, minor) = key_root(p.key);
        format!(
            "{} {}",
            tune::scale::NAMES[root as usize % 12],
            if minor { tr("Minor") } else { tr("Major") }
        )
        .to_uppercase()
    } else {
        choice(tune::KEY, tune::PARAMS[tune::KEY].choices)
    };
    hw::paint_field(&gi, row(y, tr("Key")), &key, false, false);
    y += 32.0;
    hw::paint_field(
        &gi,
        row(y, tr("Scale")),
        &choice(tune::SCALE, tune::PARAMS[tune::SCALE].choices),
        false,
        false,
    );
    y += 32.0;
    hw::paint_field(
        &gi,
        row(y, tr("Target")),
        &choice(tune::TARGET, tune::PARAMS[tune::TARGET].choices),
        false,
        false,
    );
    y += 34.0;
    hw::seam_h(&gi, ix..=ix + iw, y);
    y += 12.0;
    for (label, k) in [
        (tr("Speed"), tune::RETUNE),
        (tr("Correction"), tune::AMOUNT),
        (tr("Mix"), tune::MIX),
    ] {
        let r = row(y, label);
        crate::piano_roll::lcd_value(
            &gi,
            Rect::from_min_size(pos2(r.right() - 64.0, y), vec2(64.0, 24.0)),
            &format!("{}", get(k).round() as i32),
        );
        hw::text(
            &gi,
            pos2(r.right() - 70.0, y + 12.0),
            Align2::RIGHT_CENTER,
            &tune::PARAMS[k].unit.to_uppercase(),
            Face::Silk,
            10.0,
            theme::PRINT,
        );
        y += 32.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(v: &[(f64, f32)]) -> Vec<AutoPoint> {
        v.iter().map(|(b, x)| AutoPoint::new(*b, *x)).collect()
    }

    #[test]
    fn keys_are_read_from_their_names() {
        assert_eq!(key_root("Cmaj"), (0, false));
        assert_eq!(key_root("Ebmin"), (3, true));
        assert_eq!(key_root("F#maj"), (6, false));
        assert_eq!(key_root("Bbmaj"), (10, false));
    }

    #[test]
    fn an_override_steps_in_and_out_and_erasing_takes_it_away() {
        let e4 = override_pos(64.0);
        let drawn = with_override(&[], &[(1.0, e4), (2.0, e4)], 1.0, 2.0, false);
        // Off before, E4 from beat 1 to 2, off after.
        assert_eq!(prowave_engine::lane_value(&drawn, 0.5), Some(0.0));
        assert_eq!(prowave_engine::lane_value(&drawn, 1.5), Some(e4));
        assert_eq!(prowave_engine::lane_value(&drawn, 2.5), Some(0.0));
        let spans = override_spans(&drawn);
        assert_eq!(spans.len(), 1);
        assert!((spans[0][0].1 - 64.0).abs() < 1e-3);
        // A second one after it keeps the first.
        let g4 = override_pos(67.0);
        let two = with_override(&drawn, &[(3.0, g4), (4.0, g4)], 3.0, 4.0, false);
        assert_eq!(override_spans(&two).len(), 2);
        assert_eq!(prowave_engine::lane_value(&two, 1.5), Some(e4));
        // Erasing the middle of the first leaves its two ends.
        let cut = with_override(&two, &[], 1.25, 1.75, true);
        assert_eq!(override_spans(&cut).len(), 3);
        assert_eq!(prowave_engine::lane_value(&cut, 1.5), Some(0.0));
        assert_eq!(prowave_engine::lane_value(&cut, 1.1), Some(e4));
        // Erasing everything leaves no lane.
        assert!(with_override(&cut, &[], 0.0, 10.0, true).is_empty());
    }

    #[test]
    fn the_amount_is_drawn_and_erased() {
        let a = with_amount(
            &pts(&[(0.0, 1.0), (8.0, 1.0)]),
            &[(2.0, 0.2), (3.0, 0.4)],
            2.0,
            3.0,
            false,
        );
        assert_eq!(a.len(), 4);
        let e = with_amount(&a, &[], 1.5, 3.5, true);
        assert_eq!(e.len(), 2);
    }

    /// A top line: a MIDI track with a 4-bar region at bar 1, and the audio track under it
    /// that sings its notes (what an effect there follows in MIDI mode).
    fn top_line(p: &mut Project) -> (usize, usize) {
        {
            use crate::model::{Region, RegionParams};
            let score = p.add_track(TrackKind::Instrument);
            p.tracks[score].regions.push(Region {
                missing: None,
                name: "Top Line 1".into(),
                start_bar: 1.0,
                len_bars: 4.0,
                notes: vec![],
                controls: vec![],
                folder: None,
                audio: None,
                params: RegionParams::default(),
                generated: None,
            });
            p.add_track(TrackKind::Audio);
        }
        (p.tracks.len() - 2, p.tracks.len() - 1)
    }

    #[test]
    fn the_midi_target_is_the_top_lines_notes_or_the_track_above() {
        use crate::model::{Note, Region, RegionParams};
        let mut p = Project::empty();
        let (score, out) = top_line(&mut p);
        p.tracks[score].regions[0].notes.push(Note {
            start: 1.0,
            len: 2.0,
            pitch: 64,
            vel: 100,
            mute: false,
            lyric: Default::default(),
        });
        let info = prowave_engine::native_info(tune::ID).unwrap();
        let mut slot = crate::native_fx_ui::new_slot(info);
        let mut vals = info.defaults();
        vals[tune::TARGET] = tune::TARGET_MIDI as f32;
        slot.state = Some(crate::native_fx_ui::encode(&vals));
        p.tracks[out].au_effects.push(slot);
        assert_eq!(midi_source(&p, out, &vals), Some(score));
        let t = targets_for(&p, out, &vals);
        let origin = p.region_start_beats(&p.tracks[score].regions[0]);
        assert_eq!(t.notes.len(), 1);
        assert_eq!(t.notes[0].pitch, 64);
        assert!((t.notes[0].start - (origin + 1.0)).abs() < 1e-9);
        // The score's region in the Piano Roll shows the output track's correction.
        assert_eq!(
            link_for(&p, score),
            Some(Link {
                track: out,
                slot: 0
            })
        );
        // A chosen track number wins; an audio track is not a MIDI source.
        vals[tune::MIDI_TRACK] = (out + 1) as f32;
        assert_eq!(midi_source(&p, out, &vals), None);
        // A plain audio track under a MIDI track follows it.
        let midi = p.add_track(TrackKind::Instrument);
        p.tracks[midi].regions.push(Region {
            missing: None,
            name: "guide".into(),
            start_bar: 1.0,
            len_bars: 1.0,
            notes: vec![],
            controls: vec![],
            folder: None,
            audio: None,
            params: RegionParams::default(),
            generated: None,
        });
        let audio = p.add_track(TrackKind::Audio);
        vals[tune::MIDI_TRACK] = 0.0;
        assert_eq!(midi_source(&p, audio, &vals), Some(midi));
        let t = targets_for(&p, audio, &info.defaults());
        assert!(t.notes.is_empty(), "scale mode sends no notes");
        p.key = "Ebmin";
        let t = targets_for(&p, audio, &info.defaults());
        assert_eq!((t.root, t.minor), (3, true));
    }

    #[test]
    fn drawing_on_the_grid_writes_the_override_lane() {
        let mut p = Project::empty();
        let ti = p.add_track(TrackKind::Audio);
        let info = prowave_engine::native_info(tune::ID).unwrap();
        p.tracks[ti]
            .au_effects
            .push(crate::native_fx_ui::new_slot(info));
        let link = Link { track: ti, slot: 0 };
        let mut st = PenStroke {
            kind: Kind::Override,
            erase: false,
            link,
            before: Vec::new(),
            samples: BTreeMap::new(),
            last: None,
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
        };
        st.add(4.0, override_pos(62.0));
        st.add(6.0, override_pos(62.0));
        st.apply(&mut p);
        let lane = p.tracks[ti].auto.lane(link.target(tune::OVERRIDE));
        assert!(!lane.is_empty());
        let pl = &p.tracks[ti].auto.plugins[0];
        assert_eq!((pl.lo, pl.hi), (0.0, 12700.0));
        assert_eq!(pl.plugin, tune::ID);
        // Played back, the lane is D4 between beats 4 and 6 (in the effect's own units).
        let at = |b: f64| prowave_engine::lane_value(lane, b).map(|x| pl.lo + x * (pl.hi - pl.lo));
        assert!((at(5.0).unwrap_or(0.0) - 6200.0).abs() < 1.0);
        assert_eq!(at(3.0), Some(0.0));
        assert_eq!(at(7.0), Some(0.0));
    }

    /// A top line whose sung audio has Pro Wave Tune, with an analysed take (made up here), an
    /// override and an Amount curve.
    fn picture_project() -> (Project, crate::UiState, usize, usize) {
        use crate::model::{Note, Region, RegionParams};
        let mut p = Project::empty();
        let mut s = crate::UiState::new();
        let (score, out) = top_line(&mut p);
        p.selected_track = score;
        s.tracks.selected = Some((score, 0));
        let melody = [
            60u8, 62, 64, 65, 67, 65, 64, 62, 60, 64, 67, 72, 71, 67, 64, 60,
        ];
        for (i, m) in melody.iter().enumerate() {
            p.tracks[score].regions[0].notes.push(Note {
                start: i as f32,
                len: 0.9,
                pitch: *m,
                vel: 100,
                mute: false,
                lyric: Default::default(),
            });
        }
        let bar = p.tracks[score].regions[0].start_bar;
        let path = PathBuf::from("/prowave-tune-picture/take.wav");
        p.tracks[out].regions.push(Region {
            missing: None,
            name: "take".into(),
            start_bar: bar,
            len_bars: 4.0,
            notes: vec![],
            controls: vec![],
            folder: None,
            audio: Some(crate::model::AudioRegion {
                file: prowave_engine::AudioFile {
                    path: path.clone(),
                    sample_rate: 48000,
                    frames: 48000 * 8,
                    channels: 1,
                },
                peaks: Arc::new(std::sync::OnceLock::new()),
                flex: None,
            }),
            params: RegionParams::default(),
            generated: None,
        });
        // The take: each note sung a little off, scooped into, with vibrato (120 bpm: a beat
        // is half a second).
        let mut frames = Vec::new();
        for k in 0..1600 {
            let t = k as f32 * 0.005;
            let beat = t * 2.0;
            let i = beat.floor() as usize;
            let within = beat - beat.floor();
            let m = if i < melody.len() && within < 0.85 {
                let off = [0.3, -0.25, 0.15, -0.35][i % 4];
                let scoop = (-within * 12.0).exp() * -0.8;
                f32::from(melody[i]) + off + scoop + 0.2 * (std::f32::consts::TAU * 5.5 * t).sin()
            } else {
                f32::NAN
            };
            frames.push((t, m));
        }
        with(|rt| {
            rt.analysis
                .insert(path.clone(), Analysis::Ready(Arc::new(frames)))
        });
        let info = prowave_engine::native_info(tune::ID).unwrap();
        let mut slot = crate::native_fx_ui::new_slot(info);
        let mut vals = info.defaults();
        vals[tune::TARGET] = tune::TARGET_MIDI as f32;
        slot.state = Some(crate::native_fx_ui::encode(&vals));
        p.tracks[out].au_effects.push(slot);
        let link = Link {
            track: out,
            slot: 0,
        };
        let origin = p.region_start_beats(&p.tracks[score].regions[0]);
        let ov = with_override(
            &[],
            &[
                (origin + 9.0, override_pos(63.0)),
                (origin + 10.0, override_pos(63.0)),
                (origin + 10.5, override_pos(66.5)),
            ],
            origin + 9.0,
            origin + 10.5,
            false,
        );
        *lane_mut(&mut p, link, tune::OVERRIDE).unwrap() = ov;
        *lane_mut(&mut p, link, tune::AMOUNT).unwrap() = vec![
            AutoPoint::new(origin, 1.0),
            AutoPoint::new(origin + 6.0, 1.0),
            AutoPoint::new(origin + 8.0, 0.4),
            AutoPoint::new(origin + 12.0, 0.4),
            AutoPoint::new(origin + 13.0, 1.0),
        ];
        (p, s, score, out)
    }

    /// The lane picks up the take, the notes and the drawn override; the audio track's own
    /// grid shows them too. Drawn offscreen, nothing on the screen.
    #[test]
    fn the_correction_lane_draws_in_the_piano_roll_and_on_its_own() {
        let (mut p, mut s, score, out) = picture_project();
        assert_eq!(
            link_for(&p, score),
            Some(Link {
                track: out,
                slot: 0
            })
        );
        let c = curve(&p, out);
        assert!(c.iter().filter(|x| x.1.is_finite()).count() > 1000);
        let origin = p.region_start_beats(&p.tracks[score].regions[0]);
        // The first sung frame lands on the score's first beat.
        let first = c
            .iter()
            .find(|x| x.1.is_finite())
            .map(|x| x.0)
            .unwrap_or(-1.0);
        assert!((first - origin).abs() < 0.02, "{first} vs {origin}");
        s.tracks.selected = Some((score, 0));
        s.piano.tune_lane = true;
        let mut h = crate::ui_test::Harness::new(1200.0, 640.0);
        h.settle(|ui| crate::piano_roll::show(ui, &mut p, &mut s));
        // With no MIDI region selected and the audio track selected, its own grid shows.
        s.tracks.selected = None;
        p.selected_track = out;
        h.settle(|ui| crate::piano_roll::show(ui, &mut p, &mut s));
        assert_eq!(s.tune.for_track, Some(out));
    }

    /// Pictures of both, to look at: `PROWAVE_TUNE_PNG_DIR=/path cargo test -p prowave-app
    /// tune_lane_pictures -- --ignored`.
    #[test]
    #[ignore = "picture"]
    fn tune_lane_pictures() {
        let dir = std::env::var("PROWAVE_TUNE_PNG_DIR")
            .or_else(|_| std::env::var("MANUAL_PNG_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let _ = std::fs::create_dir_all(&dir);
        let (mut p, mut s, score, out) = picture_project();
        s.tracks.selected = Some((score, 0));
        s.piano.tune_lane = true;
        let mut frame = 0;
        crate::ui_test::render_png(
            1200.0,
            640.0,
            1.0,
            Color32::BLACK,
            |ui| {
                frame += 1;
                if frame == 3 {
                    (s.piano.beat_px, s.piano.key_h) = (60.0, 12.0);
                    s.piano.catch = false;
                }
                crate::theme::apply(ui.ctx());
                crate::piano_roll::show(ui, &mut p, &mut s)
            },
            &dir.join("tune-lane-piano-roll.png"),
        )
        .unwrap();
        s.tracks.selected = None;
        p.selected_track = out;
        crate::ui_test::render_png(
            1200.0,
            640.0,
            1.0,
            Color32::BLACK,
            |ui| {
                crate::theme::apply(ui.ctx());
                crate::piano_roll::show(ui, &mut p, &mut s)
            },
            &dir.join("tune-lane-audio-track.png"),
        )
        .unwrap();
    }

    /// The effect's settings and the drawn correction are saved with the project and come
    /// back the same.
    #[test]
    fn settings_and_corrections_are_saved_and_loaded() {
        let (mut p, _, _, out) = picture_project();
        // The made-up take has no file to save.
        p.tracks[out].regions.clear();
        let info = prowave_engine::native_info(tune::ID).unwrap();
        let mut v = values(&p.tracks[out].au_effects[0]);
        info.presets[0].apply(&mut v);
        v[tune::KEY] = 5.0;
        v[tune::FORMANT] = 250.0;
        p.tracks[out].au_effects[0].state = Some(crate::native_fx_ui::encode(&v));
        let dir = tempfile::tempdir().unwrap();
        let path = crate::project_io::save_project(&p, &dir.path().join("tune")).unwrap();
        let ctx = egui::Context::default();
        let (q, _) = crate::project_io::open_project(&path, &ctx).unwrap();
        let back = &q.tracks[out].au_effects[0];
        assert_eq!(back.id, tune::ID);
        assert_eq!(values(back), v);
        let link = Link {
            track: out,
            slot: 0,
        };
        for param in [tune::OVERRIDE, tune::AMOUNT] {
            assert_eq!(
                lane(&q, link, param),
                lane(&p, link, param),
                "param {param}"
            );
            assert!(!lane(&q, link, param).is_empty());
        }
        let pl = q.tracks[out]
            .auto
            .plugins
            .iter()
            .find(|l| l.param == tune::OVERRIDE as u32)
            .unwrap();
        assert_eq!((pl.lo, pl.hi, pl.plugin.as_str()), (0.0, 12700.0, tune::ID));
    }
}
