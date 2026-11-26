//! Follow Rack Groove: a MIDI track plays its notes with the feel of one drum class of a Channel
//! Rack (Track Inspector: Follow Rack), so a synth part (an AU or VST3 instrument such as reFX
//! Nexus, a built-in instrument, an external synth) locks to the rack's drums.
//!
//! Non-destructive: the track's notes stay where they are; the notes the engine gets (playback,
//! the bounce) and, unless switched off, an exported MIDI file are moved when the song is made
//! ([`apply`]). Turning the option off, or changing the rack's groove or amounts, plays the notes
//! from where they are stored again.
//!
//! The timing is the rack's own hits ([`hits`]): every note of the class's channels where the
//! rack plays it, in every region and loop repeat, with the step it is on (a loaded pattern's
//! played hit: the step it was played on, at the time it was played). A note on a hit's step
//! plays as far from it as that one hit was played, so each bar follows its own hits, never a
//! mean of them; a note between two hits at most a sixteenth apart plays its fraction of the
//! way between where they play; a note where the class plays no hit stays where it is. A note's
//! end moves with its start (the length is kept), and is cut where the next note of the same
//! pitch starts. With Velocity on, the velocity of the hit that carries a note over the mean of
//! the class's hits scales the note's velocity.
//!
//! Early notes: a note on a hit played early plays early, before its region's start or its
//! loop repeat's (the end of the repeat before: pre-roll), except that it never moves before the
//! song's start (beat 0), and, while Cycle is on, a note in the cycle never moves before the
//! cycle's start (the engine starts every pass at the cycle start and would skip it): there it is
//! clamped.
//!
//! Groove Q (Region and Track Inspector, next to Quantize): a quantize toward the rack's hits
//! instead of the straight grid, with the same targets as Follow Rack ([`Lock::target`]). Each
//! note is pulled from where it was played toward the target of the nearest step (the step of
//! the region's Quantize grid, a sixteenth when it is off): where the class's own hit on that
//! step plays, or between two hits its share of the way; where the class plays no hit the note
//! stays. By the strength (0-100 %): the region's own, or else its track's, so a take recorded
//! on the track is grooved too. Non-destructive like Quantize: the stored notes do not move, the
//! strength can be changed at any time. It uses the track's Rack and Follow choices whether
//! Follow Rack is on or not; with Follow Rack on as well, Groove Q pulls toward the straight step
//! and Follow Rack then moves it, so a note at 100 % still lands on the hit once, not twice.
//!
//! Groove Live (opt-in): a key played live while the song plays can not sound before it is
//! pressed, so it is only held back, toward a hit the rack plays late, by the same pull (the
//! engine's [`prowave_engine::LiveGroove`]); a key before a hit played early, or where the class
//! plays none, sounds at once. The take is recorded at the moment the key was pressed, and Groove
//! Q places it on playback.
//!
//! Auto picks the class from the track ([`auto_class`]): 808 and bass parts follow the kick,
//! dense sixteenth parts (arps, plucks, hat-like rolls) the closed hat, everything else (chords,
//! pads, sparse leads) the snare, or the clap when the rack plays claps and no snares.
use crate::channel_rack::{self, RackState};
use crate::kit_classify::{self, DrumClass};
use crate::model::{Project, TrackKind};
use crate::rack_groove::STEP;
use prowave_engine::{NoteEvent, RACK_BASE_NOTE};

/// A track's Follow Rack setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FollowRack {
    pub on: bool,
    /// Which rack: 0 is the first track in the list playing a Channel Rack, 1 the second… (a
    /// number past the last: the first).
    pub rack: u8,
    /// The class whose feel the notes take; None: Auto.
    pub class: Option<DrumClass>,
    /// The groove's velocities accent the notes.
    pub velocity: bool,
    /// Groove Q (MIDI tracks) or Groove Flex (audio tracks): how far, 0-100 %, the notes or
    /// hits of the track's regions are pulled to the rack's hits (a region may have its own).
    pub groove_q: u8,
    /// Groove Live: keys played live wait for a hit the rack plays late.
    pub live: bool,
}

/// The classes the Follow menu leads with (Hat is the closed hat), after Auto.
pub const MAIN_CLASSES: [DrumClass; 3] = [DrumClass::ClosedHat, DrumClass::Snare, DrumClass::Kick];

/// The other classes the menu lists, below the main ones.
pub fn other_classes() -> impl Iterator<Item = DrumClass> {
    channel_rack::CHANNEL_CLASSES
        .into_iter()
        .filter(|c| !MAIN_CLASSES.contains(c))
}

/// The id the project file and the control interface use for a class choice ("auto" or a
/// class id).
pub fn class_id(c: Option<DrumClass>) -> &'static str {
    c.map_or("auto", DrumClass::id)
}

/// The class choice an id names ("auto", "hat" for the closed hat, or a rack class id).
pub fn parse_class(id: &str) -> Option<Option<DrumClass>> {
    match id.trim().to_lowercase().as_str() {
        "auto" | "" => Some(None),
        "hat" => Some(Some(DrumClass::ClosedHat)),
        s => DrumClass::from_id(s)
            .filter(|c| channel_rack::CHANNEL_CLASSES.contains(c))
            .map(Some),
    }
}

// ---------------------------------------------------------------- Auto

// The thresholds were measured on 3,740 producer MIDI files labelled by name (808/bass, hats,
// arps/plucks, chords, melodies) from sample packs, and the 808s of 46 recorded songs: with the name
// rule they sort 100 % of bass files to the kick, 78 % of hat and arp files to the hat and 88 %
// of chord files to the snare (melodies: 73 % snare, 27 % hat). Played 808s sit low (recorded songs: 97 %
// below 48); sample-pack 808 MIDI is often written around C3, which only the name finds.

/// Auto: a part whose median pitch is below this (Logic naming: pitch 48 is C2) and that plays
/// one note at a time is a bass part.
pub const BASS_BELOW: u8 = 48;
/// Notes per onset at most for a bass part (an 808 or a bass line plays one note at a time).
pub const BASS_POLY: f64 = 1.05;
/// Onsets per quarter note at least for a dense part (2: steady eighths; 4: every sixteenth).
pub const DENSE_ONSETS: f64 = 2.0;
/// The share of onsets on the off sixteenths (the "e" and "a") at least for a dense part.
pub const DENSE_OFF16: f64 = 0.10;

/// What Auto measures of a part.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartStats {
    pub notes: usize,
    /// Onsets: note starts, those within a 64th of each other as one.
    pub onsets: usize,
    pub median_pitch: u8,
    /// Notes per onset.
    pub poly: f64,
    /// Onsets per quarter note, over whole bars (four quarter notes) from the first onset to
    /// the end of the last note.
    pub density: f64,
    /// The share of onsets on the "e" or "a" of a beat (within a 32nd).
    pub off16: f64,
}

/// What Auto measures of `notes`.
pub fn stats(notes: &[NoteEvent]) -> PartStats {
    if notes.is_empty() {
        return PartStats::default();
    }
    let mut pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
    pitches.sort_unstable();
    let mut starts: Vec<f64> = notes.iter().map(|n| n.start_beat).collect();
    starts.sort_by(f64::total_cmp);
    let mut onsets: Vec<f64> = Vec::new();
    for s in starts {
        if onsets.last().is_none_or(|o| s - o > 1.0 / 64.0) {
            onsets.push(s);
        }
    }
    let first = onsets[0];
    let last_end = notes.iter().map(|n| n.end_beat).fold(first, f64::max);
    let span = ((last_end - first) / 4.0).ceil().max(1.0) * 4.0;
    let off = onsets
        .iter()
        .filter(|o| {
            let x = *o * 4.0;
            let k = x.round();
            (x - k).abs() <= 0.5 && (k as i64).rem_euclid(2) == 1
        })
        .count();
    PartStats {
        notes: notes.len(),
        onsets: onsets.len(),
        median_pitch: pitches[pitches.len() / 2],
        poly: notes.len() as f64 / onsets.len() as f64,
        density: onsets.len() as f64 / span,
        off16: off as f64 / onsets.len() as f64,
    }
}

/// Whether a track's or instrument's name says it plays an 808 or a bass.
pub fn bass_name(name: &str) -> bool {
    kit_classify::words(name).iter().any(|w| {
        w == "808"
            || (w.starts_with("bass") && w != "bassoon")
            || matches!(w.as_str(), "sub" | "subs" | "subbass")
    })
}

/// The class Auto picks for a part with `stats`, its track and instrument named `names`:
/// Kick for an 808 or bass (by name, or low and one note at a time), the closed hat for a dense
/// part, else the snare.
pub fn auto_class(st: &PartStats, names: &[&str]) -> DrumClass {
    if names.iter().any(|n| bass_name(n)) {
        return DrumClass::Kick;
    }
    if st.notes == 0 {
        return DrumClass::Snare;
    }
    if st.median_pitch < BASS_BELOW && st.poly <= BASS_POLY {
        return DrumClass::Kick;
    }
    if st.density >= DENSE_ONSETS || st.off16 >= DENSE_OFF16 {
        return DrumClass::ClosedHat;
    }
    DrumClass::Snare
}

/// The class a choice plays: Auto's pick for the part, with the snare taken by the clap when
/// the rack plays claps and no snares (`has`: whether the rack plays hits of a class).
pub fn resolve(
    choice: Option<DrumClass>,
    auto: DrumClass,
    has: impl Fn(DrumClass) -> bool,
) -> DrumClass {
    match choice {
        Some(c) => c,
        None if auto == DrumClass::Snare && !has(DrumClass::Snare) && has(DrumClass::Clap) => {
            DrumClass::Clap
        }
        None => auto,
    }
}

// ---------------------------------------------------------------- the racks

/// The tracks playing a Channel Rack, in list order.
pub fn racks(p: &Project) -> Vec<usize> {
    (0..p.tracks.len())
        .filter(|i| channel_rack::rack_state(p, *i).is_some())
        .collect()
}

/// The rack track a setting follows (its number past the last: the first).
pub fn rack_of(p: &Project, f: &FollowRack) -> Option<usize> {
    let r = racks(p);
    r.get(usize::from(f.rack)).or(r.first()).copied()
}

/// What a track plays as it would follow: every unmuted region and loop repeat, through its
/// MIDI FX (what Auto reads; it does not depend on what is soloed).
pub fn played(p: &Project, ti: usize) -> Vec<NoteEvent> {
    let Some(t) = p.tracks.get(ti) else {
        return Vec::new();
    };
    let mut end = 0.0f64;
    let mut notes = Vec::new();
    for r in t.regions.iter().filter(|r| !r.params.mute && !r.is_audio()) {
        for (origin, span) in crate::engine_sync::plays_of(p, r) {
            end = end.max(p.region_end_beats(r).max(origin + span.unwrap_or(0.0)));
            notes.extend(crate::engine_sync::played_notes(
                t, r, origin, span, ti as u16,
            ));
        }
    }
    if t.midi_fx.iter().any(|s| s.on) {
        crate::midi_fx::apply_until(&t.midi_fx, notes, end)
    } else {
        notes
    }
}

/// The names Auto reads for track `ti`: the track's and its instrument's.
fn names(p: &Project, ti: usize) -> Vec<&str> {
    let t = &p.tracks[ti];
    let mut v = vec![t.name.as_str()];
    if let Some(s) = &t.au_instrument {
        v.push(s.name.as_str());
    }
    v
}

/// What track `ti` follows: the rack track and the class (Auto resolved). None when the
/// option is off, the track holds no MIDI, there is no rack, or the track is that rack.
pub fn target(p: &Project, ti: usize) -> Option<(usize, DrumClass)> {
    let t = p.tracks.get(ti)?;
    let f = t.params.follow_rack;
    if !f.on || !t.kind.has_regions() || t.kind == TrackKind::Audio {
        return None;
    }
    let rack = rack_of(p, &f)?;
    if rack == ti {
        return None;
    }
    let st = channel_rack::rack_state(p, rack)?;
    let auto = match f.class {
        Some(_) => DrumClass::Snare,
        None => auto_class(&stats(&played(p, ti)), &names(p, ti)),
    };
    Some((
        rack,
        resolve(f.class, auto, |c| !hits(p, rack, &st, c).is_empty()),
    ))
}

/// What Auto reads of track `ti`: the notes it plays, or for an audio track the hits Groove
/// Flex found (as notes of one pitch: only the name can make it a bass part).
fn part_of(p: &Project, ti: usize) -> Vec<NoteEvent> {
    if p.tracks[ti].kind == TrackKind::Audio {
        crate::flex_edit::track_hits(p, ti)
    } else {
        played(p, ti)
    }
}

/// What Groove Q and Groove Flex of track `ti` pull toward: the rack track and the class (Auto
/// resolved), whether Follow Rack is on or not. None when the track has no regions, there is no
/// rack, or the track is that rack.
pub fn groove_target(p: &Project, ti: usize) -> Option<(usize, DrumClass)> {
    let t = p.tracks.get(ti)?;
    if !t.kind.has_regions() {
        return None;
    }
    let f = t.params.follow_rack;
    let rack = rack_of(p, &f)?;
    if rack == ti {
        return None;
    }
    let st = channel_rack::rack_state(p, rack)?;
    let auto = match f.class {
        Some(_) => DrumClass::Snare,
        None => auto_class(&stats(&part_of(p, ti)), &names(p, ti)),
    };
    Some((
        rack,
        resolve(f.class, auto, |c| !hits(p, rack, &st, c).is_empty()),
    ))
}

/// What Auto picks for track `ti` (for the Inspector), whatever its setting.
pub fn auto_of(p: &Project, ti: usize) -> DrumClass {
    let auto = auto_class(&stats(&part_of(p, ti)), &names(p, ti));
    let Some(rack) = rack_of(p, &p.tracks[ti].params.follow_rack) else {
        return auto;
    };
    match channel_rack::rack_state(p, rack) {
        Some(st) => resolve(None, auto, |c| !hits(p, rack, &st, c).is_empty()),
        None => auto,
    }
}

// ---------------------------------------------------------------- the timing

/// How near a note has to be to the straight place of a rack hit's step to be on it, in quarter
/// notes (a 256th).
pub const ON_HIT: f64 = 1.0 / 64.0;

/// A hit of the followed class as the rack plays it, in song quarter notes: the straight place
/// of its step (its cell on the rack's grid), where it plays, and its velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub straight: f64,
    pub at: f64,
    pub vel: u8,
}

impl Hit {
    /// How far from its step it plays.
    fn dev(&self) -> f64 {
        self.at - self.straight
    }
}

/// The hits of class `class` that rack track `rack` (state `st`) plays: every note of the
/// class's channels (a muted channel's left out) in every unmuted region and loop repeat, at
/// the place it plays, with the cell the rack gives it (a loaded pattern's played hit: the
/// cell it was played on). A note moved by hand onto no cell gives none. By straight place;
/// of hits on one place, the earliest.
pub fn hits(p: &Project, rack: usize, st: &RackState, class: DrumClass) -> Vec<Hit> {
    let Some(t) = p.tracks.get(rack) else {
        return Vec::new();
    };
    let chans: Vec<usize> = (0..st.channels.len())
        .filter(|&c| st.class(c) == class && !st.channels[c].mute)
        .collect();
    if chans.is_empty() {
        return Vec::new();
    }
    // A hit played before the pattern's start sits at the pattern's end (a hat pushed ahead of
    // the one): its step is the next repeat's first.
    let span = st.pattern.steps as f64 * STEP;
    let mut out = Vec::new();
    for r in t
        .regions
        .iter()
        .filter(|r| !r.params.mute && !r.is_audio() && r.generated.is_none())
    {
        for (origin, len) in crate::engine_sync::plays_of(p, r) {
            for n in r.notes.iter().filter(|n| !n.mute) {
                let Some(c) = n.pitch.checked_sub(RACK_BASE_NOTE).map(usize::from) else {
                    continue;
                };
                if !chans.contains(&c) {
                    continue;
                }
                let start = match r.params.quantize {
                    Some(q) if q > 0.0 => (n.start / q).round() * q,
                    _ => n.start,
                };
                if len.is_some_and(|l| f64::from(start) >= l - 1e-9) {
                    continue;
                }
                let Some(cell) = st.cell_at(c, start) else {
                    continue;
                };
                let mut dev = f64::from(start) - cell.straight();
                if span > 0.0 && dev.abs() > span / 2.0 {
                    dev -= (dev / span).round() * span;
                }
                let at = origin + f64::from(start);
                out.push(Hit {
                    straight: at - dev,
                    at,
                    vel: n.vel,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.straight
            .total_cmp(&b.straight)
            .then(a.at.total_cmp(&b.at))
    });
    out.dedup_by(|b, a| (b.straight - a.straight).abs() < 1e-9);
    out
}

/// Everything a following track's notes need: the rack's hits of the class, the velocity they
/// accent with, and where notes may not move before.
#[derive(Clone, Debug, PartialEq)]
pub struct Lock {
    /// By straight place.
    pub hits: Vec<Hit>,
    /// The mean velocity of the hits, when their velocities accent the notes (Velocity on).
    pub mean_vel: Option<f64>,
    /// While Cycle is on: its start and end.
    pub cycle: Option<(f64, f64)>,
}

impl Lock {
    /// The lock of a track following class `class` of the rack on track `rack`.
    pub fn new(p: &Project, rack: usize, class: DrumClass, velocity: bool) -> Option<Lock> {
        let st = channel_rack::rack_state(p, rack)?;
        let hits = hits(p, rack, &st, class);
        let mean_vel = (velocity && !hits.is_empty())
            .then(|| hits.iter().map(|h| f64::from(h.vel)).sum::<f64>() / hits.len() as f64);
        let cycle = (p.cycle && p.cycle_end > p.cycle_start && p.cycle_start.is_finite())
            .then_some((p.cycle_start, p.cycle_end));
        Some(Lock {
            hits,
            mean_vel,
            cycle,
        })
    }

    /// Where a note at `beat` plays by the rack's hits, and the hit that carries it: on a hit's
    /// step, as far from it as that hit plays from it; between two hits at most a sixteenth
    /// apart, its fraction of the way between where they play; elsewhere None (it stays).
    fn carry(&self, beat: f64) -> Option<(f64, &Hit)> {
        let i = self.hits.partition_point(|h| h.straight <= beat + ON_HIT);
        let prev = &self.hits[i.checked_sub(1)?];
        if (beat - prev.straight).abs() <= ON_HIT {
            return Some((beat + prev.dev(), prev));
        }
        let next = self.hits.get(i)?;
        let gap = next.straight - prev.straight;
        if gap > STEP + ON_HIT {
            return None;
        }
        let frac = (beat - prev.straight) / gap;
        Some((prev.at + frac * (next.at - prev.at), prev))
    }

    /// Where a note starting at `beat` plays.
    pub fn place(&self, beat: f64) -> f64 {
        self.target(beat).unwrap_or(beat)
    }

    /// Where a note starting at `beat` plays when a hit carries it (see [`Lock::place`]); None
    /// where the class plays no hit. Follow Rack, Groove Q, Groove Flex and Groove Live all take
    /// their targets from here.
    pub fn target(&self, beat: f64) -> Option<f64> {
        let (to, _) = self.carry(beat)?;
        // Never before the song, and in a cycle never before its start.
        let floor = match self.cycle {
            Some((s, e)) if beat >= s - 1e-9 && beat < e => s,
            _ => 0.0,
        };
        Some(if to < floor && beat >= floor {
            floor
        } else {
            to.max(0.0)
        })
    }

    /// The target of the step of `grid` (quarter notes, from `bar`, the start of the beat's bar)
    /// nearest `beat`: where the rack's hit on that step plays ([`Lock::target`]); `beat` itself
    /// where the class plays no hit there. `straight`: the step itself where there is a hit
    /// (Follow Rack moves it next).
    pub fn step_target(&self, beat: f64, grid: f64, bar: f64, straight: bool) -> f64 {
        let grid = if grid.is_finite() && grid > 1e-6 {
            grid
        } else {
            STEP
        };
        let step = (bar + ((beat - bar) / grid).round() * grid).max(0.0);
        match self.target(step) {
            Some(_) if straight => step,
            Some(to) => to,
            None => beat,
        }
    }

    /// The velocity of a note of velocity `vel` at `beat`: scaled by the velocity of the hit
    /// that carries it over the hits' mean (Velocity on).
    pub fn velocity(&self, vel: u8, beat: f64) -> u8 {
        let Some(mean) = self.mean_vel.filter(|m| *m > 0.0) else {
            return vel;
        };
        match self.carry(beat) {
            Some((_, h)) => (f64::from(vel) * f64::from(h.vel) / mean)
                .round()
                .clamp(1.0, 127.0) as u8,
            None => vel,
        }
    }
}

/// The shortest a note is cut to, in quarter notes (a 256th).
const MIN_LEN: f64 = 1.0 / 64.0;

/// Moves `notes` (one track's, in song beats) to the rack's hits: each note's start where the
/// lock places it, its end by as much, its velocity accented; then each note ends at the latest
/// where the next of its pitch starts.
pub fn shift(notes: &mut [NoteEvent], lock: &Lock) {
    for n in notes.iter_mut() {
        let to = lock.place(n.start_beat);
        n.vel = lock.velocity(n.vel, n.start_beat);
        let d = to - n.start_beat;
        n.start_beat = to;
        n.end_beat = (n.end_beat + d).max(to + MIN_LEN);
    }
    notes.sort_by(|a, b| {
        a.pitch
            .cmp(&b.pitch)
            .then(a.start_beat.total_cmp(&b.start_beat))
    });
    for i in 1..notes.len() {
        let (a, b) = (notes[i - 1], notes[i]);
        if a.pitch == b.pitch && a.end_beat > b.start_beat {
            notes[i - 1].end_beat = b.start_beat.max(a.start_beat + MIN_LEN);
        }
    }
    notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
}

/// Moves the notes track `ti` plays (`notes`: its regions through its MIDI FX) to the hits of
/// the rack it follows. Returns the class followed; None (nothing moved) when it follows none.
pub fn apply(p: &Project, ti: usize, notes: &mut [NoteEvent]) -> Option<DrumClass> {
    let (rack, class) = target(p, ti)?;
    let lock = Lock::new(p, rack, class, p.tracks[ti].params.follow_rack.velocity)?;
    shift(notes, &lock);
    Some(class)
}

/// Whether any track follows a rack or uses Groove Q (the song then depends on the cycle too).
pub fn any(p: &Project) -> bool {
    p.tracks
        .iter()
        .any(|t| t.params.follow_rack.on || t.regions.iter().any(|r| groove_q_of(t, r) > 0))
}

// ---------------------------------------------------------------- Groove Q

/// The most Groove Q (and Groove Flex) pulls: 100 %.
pub const MAX_GROOVE_Q: u8 = 100;

/// Region `r`'s Groove Q strength on track `t`: its own, or else its track's (0: off).
pub fn groove_q_of(t: &crate::model::Track, r: &crate::model::Region) -> u8 {
    r.params
        .groove_q
        .unwrap_or(t.params.follow_rack.groove_q)
        .min(MAX_GROOVE_Q)
}

/// Where a note (or hit) at `from` goes when pulled `strength` % of the way to `to`.
pub fn pull(from: f64, to: f64, strength: u8) -> f64 {
    from + f64::from(strength.min(MAX_GROOVE_Q)) / 100.0 * (to - from)
}

/// The grid Groove Q pulls a region's notes to the steps of: its Quantize grid, or sixteenths.
pub fn grid_of(r: &crate::model::Region) -> f64 {
    match r.params.quantize {
        Some(q) if q.is_finite() && q > 0.0 => f64::from(q),
        _ => STEP,
    }
}

/// What a track's Groove Q pulls toward.
#[derive(Clone, Debug, PartialEq)]
pub struct GrooveQ {
    pub lock: Lock,
    /// Follow Rack is on too: pull to the straight step, which it then moves.
    pub straight: bool,
}

impl GrooveQ {
    /// Pulls `notes` (one region's, in song beats) `strength` % of the way to the target of the
    /// nearest step of `grid` ([`Lock::step_target`]); each note's end moves with its start. In order of
    /// start after.
    pub fn apply(
        &self,
        notes: &mut [NoteEvent],
        strength: u8,
        grid: f64,
        bar_of: impl Fn(f64) -> f64,
    ) {
        if strength == 0 {
            return;
        }
        for n in notes.iter_mut() {
            let to = self
                .lock
                .step_target(n.start_beat, grid, bar_of(n.start_beat), self.straight);
            let at = pull(n.start_beat, to, strength).max(0.0);
            let d = at - n.start_beat;
            n.start_beat = at;
            n.end_beat = (n.end_beat + d).max(at + MIN_LEN);
        }
        notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
    }
}

/// What Groove Q of track `ti` pulls toward, when any of its regions has a strength above 0.
pub fn groove_q(p: &Project, ti: usize) -> Option<GrooveQ> {
    let t = p.tracks.get(ti)?;
    if t.kind == TrackKind::Audio || !t.regions.iter().any(|r| groove_q_of(t, r) > 0) {
        return None;
    }
    let (rack, class) = groove_target(p, ti)?;
    Some(GrooveQ {
        lock: Lock::new(p, rack, class, false)?,
        straight: t.params.follow_rack.on,
    })
}

/// Most steps a Groove Live table holds (1,024 bars of sixteenths).
pub const MAX_LIVE_STEPS: usize = 16_384;

/// Groove Live of track `ti`: every sixteenth of the song (and some bars past its end) with
/// where the rack's hit on it plays (NaN: no hit), and the strength (0..1). None when Groove Live is off, the track's
/// Groove Q is 0, or it follows no rack.
pub fn live_groove(p: &Project, ti: usize) -> Option<prowave_engine::LiveGroove> {
    let t = p.tracks.get(ti)?;
    let f = t.params.follow_rack;
    if !f.live || f.groove_q == 0 || t.kind == TrackKind::Audio {
        return None;
    }
    let (rack, class) = groove_target(p, ti)?;
    let lock = Lock::new(p, rack, class, false)?;
    let end = p
        .tracks
        .iter()
        .flat_map(|t| &t.regions)
        .flat_map(|r| {
            crate::engine_sync::plays_of(p, r)
                .into_iter()
                .map(move |(o, span)| o + span.unwrap_or(p.region_len_beats(r)))
        })
        .fold(0.0f64, f64::max)
        + 32.0;
    let count = ((end / STEP).ceil() as usize).min(MAX_LIVE_STEPS);
    let mut steps: Vec<(f64, f64)> = Vec::with_capacity(count);
    for k in 0..count {
        let b = k as f64 * STEP;
        let bar = p.bar_start(b);
        let step = bar + ((b - bar) / STEP).round() * STEP;
        // No hit on the step: NaN, a key there never waits.
        let at = lock.target(step).unwrap_or(f64::NAN);
        if steps.last().is_none_or(|l| step > l.0 + 1e-9) {
            steps.push((step, at));
        }
    }
    Some(prowave_engine::LiveGroove::new(
        steps,
        f64::from(f.groove_q.min(MAX_GROOVE_Q)) / 100.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rack_groove::Groove;

    fn ev(start: f64, len: f64, pitch: u8) -> NoteEvent {
        NoteEvent {
            start_beat: start,
            end_beat: start + len,
            pitch,
            vel: 100,
            track: 0,
        }
    }

    fn lock(hits: &[(f64, f64, u8)]) -> Lock {
        Lock {
            hits: hits
                .iter()
                .map(|&(straight, at, vel)| Hit { straight, at, vel })
                .collect(),
            mean_vel: None,
            cycle: None,
        }
    }

    #[test]
    fn a_note_on_a_hit_plays_exactly_where_that_hit_was_played_every_bar_its_own() {
        // Hats on every sixteenth of two bars, each played its own way: no two bars alike.
        let dev = |k: usize| ((k * 7) % 11) as f64 * 0.007 - 0.03;
        let hits: Vec<(f64, f64, u8)> = (0..32)
            .map(|k| (k as f64 * 0.25, k as f64 * 0.25 + dev(k), 100))
            .collect();
        let l = lock(&hits);
        let mut n: Vec<NoteEvent> = (1..32).map(|k| ev(k as f64 * 0.25, 0.2, 60)).collect();
        shift(&mut n, &l);
        for (x, k) in n.iter().zip(1..32) {
            let want = k as f64 * 0.25 + dev(k);
            assert!((x.start_beat - want).abs() < 1e-9, "{k}: {}", x.start_beat);
            assert!(
                (x.end_beat - x.start_beat - 0.2).abs() < 1e-9,
                "length kept"
            );
        }
        // The same sixteenth of the two bars plays apart: each bar's own hit, not a mean.
        assert!((dev(5) - dev(21)).abs() > 1e-3);
        // A note a little off the step keeps its own distance from it.
        assert!((l.place(1.01) - (1.01 + dev(4))).abs() < 1e-9);
    }

    #[test]
    fn a_note_between_two_hits_plays_its_fraction_of_the_way_between_them() {
        let l = lock(&[(0.25, 0.31, 100), (0.5, 0.48, 100)]);
        assert!((l.place(0.375) - (0.31 + 0.5 * 0.17)).abs() < 1e-9);
        assert!((l.place(0.25 + 0.25 / 3.0) - (0.31 + 0.17 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn a_note_where_the_class_plays_no_hit_stays_where_it_is() {
        // Snares on two and four.
        let l = lock(&[(1.0, 1.02, 100), (3.0, 2.99, 100)]);
        assert_eq!(l.place(0.0), 0.0);
        assert_eq!(l.place(2.0), 2.0);
        assert_eq!(
            l.place(1.5),
            1.5,
            "the hits are more than a sixteenth apart"
        );
        assert_eq!(l.place(5.0), 5.0, "past the last hit");
        assert!((l.place(1.0) - 1.02).abs() < 1e-9);
        assert!((l.place(3.0) - 2.99).abs() < 1e-9);
        assert_eq!(lock(&[]).place(1.0), 1.0);
    }

    #[test]
    fn early_hits_play_early_but_never_before_the_song_or_the_cycle_start() {
        let mut l = lock(&[
            (0.0, -0.02, 100),
            (4.0, 3.97, 100),
            (8.0, 7.97, 100),
            (12.0, 11.96, 100),
        ]);
        assert_eq!(l.place(0.0), 0.0);
        assert!((l.place(4.0) - 3.97).abs() < 1e-9);
        l.cycle = Some((8.0, 16.0));
        assert_eq!(l.place(8.0), 8.0, "the cycle's start");
        assert!((l.place(12.0) - 11.96).abs() < 1e-9);
        assert!((l.place(4.0) - 3.97).abs() < 1e-9, "outside the cycle");
    }

    #[test]
    fn lengths_never_go_negative_or_overlap_the_next_note_of_the_pitch() {
        // An "a" played late, the next downbeat early: 0.15 apart.
        let l = lock(&[(0.75, 0.81, 100), (1.0, 0.96, 100)]);
        let mut n = vec![ev(0.75, 0.25, 60), ev(1.0, 0.25, 60), ev(1.0, 0.25, 64)];
        shift(&mut n, &l);
        let p60: Vec<&NoteEvent> = n.iter().filter(|x| x.pitch == 60).collect();
        assert!((p60[0].start_beat - 0.81).abs() < 1e-9);
        assert!((p60[1].start_beat - 0.96).abs() < 1e-9);
        assert!((p60[0].end_beat - 0.96).abs() < 1e-9, "cut at the next C");
        let e = n.iter().find(|x| x.pitch == 64).unwrap();
        assert!((e.end_beat - e.start_beat - 0.25).abs() < 1e-9);
        assert!(n.windows(2).all(|w| w[0].start_beat <= w[1].start_beat));
    }

    #[test]
    fn velocity_takes_the_carrying_hits_over_the_mean() {
        let mut l = lock(&[(0.0, 0.0, 120), (0.5, 0.5, 80)]);
        assert_eq!(l.velocity(100, 0.0), 100, "Velocity off");
        l.mean_vel = Some(100.0);
        assert_eq!(l.velocity(100, 0.0), 120);
        assert_eq!(l.velocity(100, 0.5), 80);
        assert_eq!(l.velocity(120, 0.0), 127, "kept in range");
        assert_eq!(l.velocity(100, 2.0), 100, "no hit there");
    }

    fn part(notes: &[(f64, f64, u8)]) -> PartStats {
        let v: Vec<NoteEvent> = notes.iter().map(|(s, l, p)| ev(*s, *l, *p)).collect();
        stats(&v)
    }

    #[test]
    fn auto_follows_the_kick_with_808s_and_bass() {
        // An 808 line around C1 (36), one note at a time.
        let b = part(&[
            (0.0, 0.75, 36),
            (0.75, 0.5, 36),
            (2.5, 1.0, 43),
            (4.0, 2.0, 31),
        ]);
        assert_eq!(auto_class(&b, &["Inst 1"]), DrumClass::Kick);
        // A name says it, whatever the notes.
        let high = part(&[(0.0, 1.0, 72)]);
        assert_eq!(auto_class(&high, &["808 Slide"]), DrumClass::Kick);
        assert_eq!(auto_class(&high, &["Nexus", "BassLine"]), DrumClass::Kick);
        assert_eq!(auto_class(&high, &["Sub Bass"]), DrumClass::Kick);
        assert!(!bass_name("Subtle Pad"));
        assert!(!bass_name("Bassoon"));
        // Low chords are not a bass part.
        let low_chords = part(&[(0.0, 4.0, 36), (0.0, 4.0, 40), (0.0, 4.0, 43)]);
        assert_ne!(auto_class(&low_chords, &[""]), DrumClass::Kick);
    }

    #[test]
    fn auto_follows_the_hat_with_dense_sixteenths_and_the_snare_with_chords() {
        // An arp: every sixteenth.
        let arp: Vec<(f64, f64, u8)> = (0..32)
            .map(|k| (k as f64 * 0.25, 0.2, 60 + (k % 4) as u8 * 4))
            .collect();
        let s = part(&arp);
        assert_eq!(s.onsets, 32);
        assert!((s.density - 4.0).abs() < 1e-6);
        assert!((s.off16 - 0.5).abs() < 1e-6);
        assert_eq!(auto_class(&s, &["Pluck"]), DrumClass::ClosedHat);
        // Syncopated plucks on the "e" and "a", not many of them.
        let sync = part(&[
            (0.25, 0.2, 72),
            (1.75, 0.2, 72),
            (2.5, 0.2, 74),
            (3.25, 0.2, 76),
        ]);
        assert_eq!(auto_class(&sync, &[""]), DrumClass::ClosedHat);
        // Chords on the bar, and a sparse lead on beats and eighths.
        let chords = part(&[
            (0.0, 4.0, 60),
            (0.0, 4.0, 64),
            (0.0, 4.0, 67),
            (4.0, 4.0, 62),
            (4.0, 4.0, 65),
            (4.0, 4.0, 69),
        ]);
        assert_eq!(auto_class(&chords, &["Pad"]), DrumClass::Snare);
        let lead = part(&[
            (0.0, 0.5, 72),
            (1.0, 0.5, 74),
            (1.5, 1.0, 76),
            (3.0, 1.0, 74),
        ]);
        assert_eq!(auto_class(&lead, &[""]), DrumClass::Snare);
        assert_eq!(auto_class(&PartStats::default(), &[""]), DrumClass::Snare);
    }

    #[test]
    fn auto_takes_the_clap_when_the_rack_plays_no_snare() {
        let none = |_: DrumClass| false;
        assert_eq!(resolve(None, DrumClass::Snare, none), DrumClass::Snare);
        let clap = |c: DrumClass| c == DrumClass::Clap;
        assert_eq!(resolve(None, DrumClass::Snare, clap), DrumClass::Clap);
        assert_eq!(resolve(None, DrumClass::Kick, clap), DrumClass::Kick);
        // A class picked by hand is kept.
        assert_eq!(
            resolve(Some(DrumClass::Snare), DrumClass::Kick, clap),
            DrumClass::Snare
        );
        let both = |c: DrumClass| matches!(c, DrumClass::Clap | DrumClass::Snare);
        assert_eq!(resolve(None, DrumClass::Snare, both), DrumClass::Snare);
    }

    /// A project with a rack (kick, snare, closed hat; hats on all sixteen steps of a one-bar
    /// region at bar 1, looped to bar 3) and a MIDI track of straight sixteenths over bars 1-2.
    fn rack_and_synth() -> (Project, usize, usize) {
        use crate::model::{Note, Region, RegionParams};
        let mut p = Project::empty();
        let pads = vec![
            Some(std::path::PathBuf::from("/lib/k/BD.wav")),
            Some(std::path::PathBuf::from("/lib/k/SD.wav")),
            Some(std::path::PathBuf::from("/lib/k/hat.wav")),
        ];
        let classes = [DrumClass::Kick, DrumClass::Snare, DrumClass::ClosedHat];
        let rack = channel_rack::load_kit(&mut p, None, "K", &pads, &classes);
        let ri = channel_rack::step_region(&mut p, rack, None);
        let st = channel_rack::rack_state(&p, rack).unwrap();
        for k in 0..16 {
            channel_rack::set_step(&mut p.tracks[rack].regions[ri], &st, 2, k, true);
        }
        p.tracks[rack].regions[ri].params.loop_beats = 8.0;
        let ti = p.add_track(TrackKind::Instrument);
        p.tracks[ti].name = "Keys".into();
        let note = |k: usize| Note {
            start: k as f32 * 0.25,
            len: 0.2,
            pitch: 72,
            vel: 100,
            mute: false,
            lyric: crate::lyrics::Lyric::NONE,
        };
        p.tracks[ti].regions.push(Region {
            missing: None,
            name: "Keys".into(),
            start_bar: 1.0,
            len_bars: 2.0,
            notes: (0..32).map(note).collect(),
            controls: Vec::new(),
            folder: None,
            generated: None,
            audio: None,
            params: RegionParams::default(),
        });
        (p, rack, ti)
    }

    fn starts_of(p: &Project, ti: usize) -> Vec<f64> {
        crate::engine_sync::song_notes_for_test(p)
            .into_iter()
            .filter(|n| usize::from(n.track) == ti)
            .map(|n| n.start_beat)
            .collect()
    }

    #[test]
    fn a_following_track_plays_and_exports_where_the_rack_plays_its_hats() {
        let (mut p, rack, ti) = rack_and_synth();
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 62.0, None);
        {
            let hat = g.classes.get_mut(&DrumClass::ClosedHat).unwrap();
            hat.offsets[4] = -0.03;
            hat.offsets[5] = 0.02;
        }
        g.set_swing(Some(DrumClass::Kick), 50.0, None);
        channel_rack::set_groove(&mut p, rack, g).unwrap();
        let straight: Vec<f64> = (0..32).map(|k| k as f64 * 0.25).collect();
        assert_eq!(starts_of(&p, ti), straight, "off: as stored");
        p.tracks[ti].params.follow_rack = FollowRack {
            on: true,
            ..FollowRack::default()
        };
        // Auto: straight sixteenths are a dense part, the closed hat.
        assert_eq!(target(&p, ti), Some((rack, DrumClass::ClosedHat)));
        // Every note plays where the rack plays its hat on that sixteenth (bar 2 is the rack
        // region's loop repeat).
        let hats: Vec<f64> = crate::engine_sync::song_notes_for_test(&p)
            .into_iter()
            .filter(|n| usize::from(n.track) == rack && n.start_beat < 8.0)
            .map(|n| n.start_beat)
            .collect();
        let got = starts_of(&p, ti);
        assert_eq!(hats.len(), 32);
        for (a, b) in got.iter().zip(&hats) {
            assert!((a - b).abs() < 1e-6, "{a} vs the rack's {b}");
        }
        assert!((got[1] - 0.31).abs() < 1e-6, "62 % swing: the e at 0.31");
        assert!((got[4] - 0.97).abs() < 1e-6, "the second beat 0.03 early");
        // The stored notes have not moved.
        assert!(p.tracks[ti].regions[0]
            .notes
            .iter()
            .enumerate()
            .all(|(k, n)| n.start == k as f32 * 0.25));
        // The MIDI file has them as they play, or as stored when asked.
        let ticks = |follow: bool| -> Vec<u64> {
            let f = crate::midi_file::export_with(&p, None, follow);
            let t = f.tracks.iter().find(|t| t.name == "Keys").unwrap();
            t.notes.iter().map(|n| n.start).collect()
        };
        let ppq = crate::midi_file::export_with(&p, None, true).ppq;
        let q = f64::from(ppq);
        assert_eq!(
            ticks(false),
            straight
                .iter()
                .map(|b| (b * q).round() as u64)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            ticks(true),
            got.iter()
                .map(|b| (b * q).round() as u64)
                .collect::<Vec<_>>()
        );
        // Following the kick (straight here) plays them straight; so does the rack's swing and
        // offset amounts at 0.
        p.tracks[ti].params.follow_rack.class = Some(DrumClass::Kick);
        assert_eq!(starts_of(&p, ti), straight);
        p.tracks[ti].params.follow_rack.class = None;
        let mut st = channel_rack::rack_state(&p, rack).unwrap();
        st.groove.swing_amount = 0.0;
        st.groove.offset_amount = 0.0;
        let old = channel_rack::rack_state(&p, rack).unwrap();
        channel_rack::commit(&mut p, rack, &old, st);
        for (a, b) in starts_of(&p, ti).iter().zip(&straight) {
            assert!((a - b).abs() < 1e-6);
        }
        // Off again: as stored.
        p.tracks[ti].params.follow_rack.on = false;
        assert_eq!(starts_of(&p, ti), straight);
    }

    #[test]
    fn a_bounce_of_the_following_track_alone_keeps_the_rack_silent_and_the_feel() {
        let (mut p, rack, ti) = rack_and_synth();
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 60.0, None);
        channel_rack::set_groove(&mut p, rack, g).unwrap();
        p.tracks[ti].params.follow_rack.on = true;
        let whole = starts_of(&p, ti);
        assert!((whole[1] - 0.30).abs() < 1e-6);
        // The track bounced alone (File > Export > All Tracks, export.bounce with a track):
        // the rack is muted but kept, so the notes are where they are in the mix.
        let q = crate::bounce::pass_project(&p, Some(ti));
        assert!(q.tracks[rack].mute);
        assert!(channel_rack::rack_state(&q, rack).is_some());
        assert_eq!(starts_of(&q, ti), whole);
        // A track that follows nothing: the rack goes as every other source does.
        p.tracks[ti].params.follow_rack.on = false;
        let q = crate::bounce::pass_project(&p, Some(ti));
        assert!(q.tracks[rack].regions.is_empty());
    }

    #[test]
    fn a_following_track_plays_on_a_loaded_patterns_played_hits_not_an_average() {
        use crate::model::{Note, Region, RegionParams};
        use crate::rack_groove::Cell;
        let mut p = Project::empty();
        let pads = vec![
            Some(std::path::PathBuf::from("/lib/k/BD.wav")),
            Some(std::path::PathBuf::from("/lib/k/SD.wav")),
            Some(std::path::PathBuf::from("/lib/k/hat.wav")),
        ];
        let classes = [DrumClass::Kick, DrumClass::Snare, DrumClass::ClosedHat];
        let rack = channel_rack::load_kit(&mut p, None, "K", &pads, &classes);
        // Two bars of eighth-note hats, each played its own way: bar 1 late, bar 2 early.
        let dev = |k: usize| -> f32 {
            if k < 16 {
                0.01 + 0.004 * k as f32
            } else {
                -0.02 - 0.003 * (k - 16) as f32
            }
        };
        let pat = channel_rack::RackPattern {
            name: "Played".into(),
            source: "test".into(),
            steps: 32,
            hits: (0..32)
                .step_by(2)
                .map(|k| (DrumClass::ClosedHat, Cell::on(k), 100, Some(dev(k))))
                .collect(),
            ..channel_rack::RackPattern::default()
        };
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 66.0, None);
        channel_rack::load_pattern(
            &mut p,
            Some(rack),
            None,
            &pat,
            g,
            None,
            channel_rack::PatternTempo::Session,
        );
        let hats: Vec<f64> = crate::engine_sync::song_notes_for_test(&p)
            .into_iter()
            .filter(|n| usize::from(n.track) == rack)
            .map(|n| n.start_beat)
            .collect();
        assert_eq!(hats.len(), 16);
        let origin = hats[0] - f64::from(dev(0));
        for (h, k) in hats.iter().zip((0..32).step_by(2)) {
            let want = origin + k as f64 * 0.25 + f64::from(dev(k));
            assert!((h - want).abs() < 1e-5, "the rack's hat {k}: {h} vs {want}");
        }
        let ti = p.add_track(TrackKind::Instrument);
        p.tracks[ti].name = "Keys".into();
        let start_bar = (p.beats_to_bar(origin) + 1.0) as f32;
        p.tracks[ti].regions.push(Region {
            missing: None,
            name: "Keys".into(),
            start_bar,
            len_bars: 2.0,
            notes: (0..16)
                .map(|k| Note {
                    start: k as f32 * 0.5,
                    len: 0.4,
                    pitch: 72,
                    vel: 100,
                    mute: false,
                    lyric: crate::lyrics::Lyric::NONE,
                })
                .collect(),
            controls: Vec::new(),
            folder: None,
            generated: None,
            audio: None,
            params: RegionParams::default(),
        });
        p.tracks[ti].params.follow_rack = FollowRack {
            on: true,
            class: Some(DrumClass::ClosedHat),
            ..FollowRack::default()
        };
        // Every eighth plays exactly where the hat of that eighth was played: the groove's
        // swing (for steps added by hand) plays no part, and the bars differ.
        let got = starts_of(&p, ti);
        assert_eq!(got.len(), 16);
        for (a, b) in got.iter().zip(&hats) {
            assert!((a - b).abs() < 1e-5, "{a} vs the rack's {b}");
        }
        assert!(((got[1] - got[0]) - (got[9] - got[8])).abs() > 1e-3);
    }

    #[test]
    fn the_setting_is_saved_with_the_project() {
        let (mut p, _, ti) = rack_and_synth();
        let f = FollowRack {
            on: true,
            rack: 0,
            class: Some(DrumClass::Snare),
            velocity: true,
            groove_q: 65,
            live: true,
        };
        p.tracks[ti].params.follow_rack = f;
        p.tracks[ti].regions[0].params.groove_q = Some(30);
        let (doc, _) = crate::project_io::to_doc(&p);
        let json = serde_json::to_string(&doc).unwrap();
        let back: prowave_project::ProjectDoc = serde_json::from_str(&json).unwrap();
        let d = back.tracks[ti].follow_rack.clone().unwrap();
        assert_eq!(
            (d.on, d.rack, d.class.as_str(), d.velocity),
            (true, 0, "snare", true)
        );
        assert_eq!((d.groove_q, d.live), (65, true));
        assert_eq!(back.tracks[ti].regions[0].groove_q, Some(30));
        // Read back into the project.
        let ctx = egui::Context::default();
        let (q, _) = crate::project_io::from_doc(&back, std::path::Path::new("/nonexistent"), &ctx);
        assert_eq!(q.tracks[ti].params.follow_rack, f);
        assert_eq!(q.tracks[ti].regions[0].params.groove_q, Some(30));
        // A track that does not follow writes nothing.
        assert!(doc.tracks[0].follow_rack.is_none());
    }

    // ------------------------------------------------ Groove Q

    /// Hats on every sixteenth of bar 1, played with 60 % swing (the "e" and "a" 0.05 late) and
    /// beat 2 0.03 early; bar 2 has no hats.
    fn swung() -> Lock {
        let hits: Vec<(f64, f64, u8)> = (0..16)
            .map(|k| {
                let b = k as f64 * STEP;
                let late = if k % 2 == 1 { 0.05 } else { 0.0 };
                let early = if k == 4 { -0.03 } else { 0.0 };
                (b, b + late + early, 100)
            })
            .collect();
        lock(&hits)
    }

    fn bar4(b: f64) -> f64 {
        (b / 4.0).floor() * 4.0
    }

    #[test]
    fn groove_q_targets_are_follow_racks_targets_of_the_nearest_step() {
        let l = swung();
        let t = |b: f64, grid: f64| l.step_target(b, grid, bar4(b), false);
        // On a step: where its own hit plays (Follow Rack's target, the same function).
        assert!((t(0.25, STEP) - 0.30).abs() < 1e-9, "the e, played late");
        assert!((t(0.5, STEP) - 0.5).abs() < 1e-9, "the & on time");
        assert!((t(1.0, STEP) - 0.97).abs() < 1e-9, "beat 2 early");
        for k in 0..16 {
            let b = k as f64 * STEP;
            assert_eq!(t(b, STEP), l.place(b));
        }
        // Played off the step: the nearest step's hit.
        assert!((t(0.22, STEP) - 0.30).abs() < 1e-9);
        assert!((t(0.36, STEP) - 0.30).abs() < 1e-9);
        assert!((t(0.40, STEP) - 0.5).abs() < 1e-9);
        assert!((t(1.04, STEP) - 0.97).abs() < 1e-9);
        // Where the class plays no hit (bar 2): the note stays as played.
        assert_eq!(t(4.27, STEP), 4.27);
        assert_eq!(t(5.02, STEP), 5.02);
        // An eighth grid: the e snaps to the beat or the &, never to its own late place.
        assert!((t(0.3, 0.5) - 0.5).abs() < 1e-9);
        assert!((t(0.2, 0.5) - 0.0).abs() < 1e-9);
        // Straight (Follow Rack moves it after): the step itself, where there is a hit.
        assert!((l.step_target(0.27, STEP, 0.0, true) - 0.25).abs() < 1e-9);
        assert_eq!(l.step_target(4.27, STEP, 4.0, true), 4.27);
    }

    #[test]
    fn groove_q_strength_0_50_and_100() {
        let l = GrooveQ {
            lock: swung(),
            straight: false,
        };
        // Played a little early on the e (0.22), and late on beat 2 (1.04).
        let played = vec![ev(0.22, 0.2, 60), ev(1.04, 0.5, 62)];
        let at = |strength: u8| {
            let mut n = played.clone();
            l.apply(&mut n, strength, STEP, bar4);
            n
        };
        assert_eq!(at(0), played, "0 %: untouched");
        let n50 = at(50);
        assert!((n50[0].start_beat - 0.26).abs() < 1e-9, "half way to 0.30");
        assert!((n50[1].start_beat - 1.005).abs() < 1e-9, "half way to 0.97");
        let n100 = at(100);
        assert!((n100[0].start_beat - 0.30).abs() < 1e-9);
        assert!((n100[1].start_beat - 0.97).abs() < 1e-9);
        for n in n50.iter().chain(&n100) {
            let len = if n.pitch == 60 { 0.2 } else { 0.5 };
            assert!(
                (n.end_beat - n.start_beat - len).abs() < 1e-9,
                "length kept"
            );
        }
        assert_eq!(pull(1.0, 2.0, 25), 1.25);
        assert_eq!(pull(1.0, 2.0, 200), 2.0, "never past the target");
    }

    #[test]
    fn groove_q_plays_recorded_notes_on_the_racks_hits_and_keeps_the_stored_ones() {
        let (mut p, rack, ti) = rack_and_synth();
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 60.0, None);
        channel_rack::set_groove(&mut p, rack, g).unwrap();
        // A take played loosely: every sixteenth 0.02 early.
        for n in &mut p.tracks[ti].regions[0].notes {
            n.start = (n.start - 0.02).max(0.0);
        }
        let stored: Vec<f32> = p.tracks[ti].regions[0]
            .notes
            .iter()
            .map(|n| n.start)
            .collect();
        let loose = starts_of(&p, ti);
        let hats: Vec<f64> = crate::engine_sync::song_notes_for_test(&p)
            .into_iter()
            .filter(|n| usize::from(n.track) == rack && n.start_beat < 8.0)
            .map(|n| n.start_beat)
            .collect();
        // The track's Groove Q (Follow Rack off) at 100 %: each note on the rack's hat.
        p.tracks[ti].params.follow_rack.groove_q = 100;
        assert_eq!(groove_target(&p, ti), Some((rack, DrumClass::ClosedHat)));
        let got = starts_of(&p, ti);
        assert_eq!(got.len(), hats.len());
        for (a, b) in got.iter().zip(&hats) {
            assert!((a - b).abs() < 1e-6, "{a} vs the rack's {b}");
        }
        assert!((got[1] - 0.30).abs() < 1e-6);
        // The region's own 50 % wins over the track's.
        p.tracks[ti].regions[0].params.groove_q = Some(50);
        for ((h, l), g) in starts_of(&p, ti).iter().zip(&loose).zip(&got) {
            assert!((h - (l + g) / 2.0).abs() < 1e-6);
        }
        // 0 % on the region: as played.
        p.tracks[ti].regions[0].params.groove_q = Some(0);
        assert_eq!(starts_of(&p, ti), loose);
        // With Follow Rack on as well, 100 % still lands on the hats (not moved twice).
        p.tracks[ti].regions[0].params.groove_q = None;
        p.tracks[ti].params.follow_rack.on = true;
        for (a, b) in starts_of(&p, ti).iter().zip(&got) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
        // The stored notes never moved.
        let now: Vec<f32> = p.tracks[ti].regions[0]
            .notes
            .iter()
            .map(|n| n.start)
            .collect();
        assert_eq!(now, stored);
        // The MIDI export has them as they play.
        let f = crate::midi_file::export_with(&p, None, true);
        let q = f64::from(f.ppq);
        let t = f.tracks.iter().find(|t| t.name == "Keys").unwrap();
        assert!((t.notes[1].start as f64 - 0.30 * q).abs() <= 1.0);
    }

    #[test]
    fn groove_live_waits_only_for_a_hit_played_late() {
        let (mut p, rack, ti) = rack_and_synth();
        let mut g = Groove::default();
        g.set_swing(Some(DrumClass::ClosedHat), 60.0, None);
        channel_rack::set_groove(&mut p, rack, g).unwrap();
        p.tracks[ti].params.follow_rack.groove_q = 100;
        assert!(live_groove(&p, ti).is_none(), "off until switched on");
        p.tracks[ti].params.follow_rack.live = true;
        let lg = live_groove(&p, ti).unwrap();
        // A key on the e of bar 2 waits for the late hat; one on the & sounds at once.
        // (The rack's notes are stored in f32: within a millionth of a beat.)
        assert!((lg.wait_at(4.25) - 0.05).abs() < 1e-6);
        assert_eq!(lg.wait_at(4.5), 0.0);
        assert_eq!(lg.wait_at(4.32), 0.0, "after the hat: never earlier");
        // Past the rack's regions there are no hats: no wait.
        assert_eq!(lg.wait_at(12.25), 0.0);
        p.tracks[ti].params.follow_rack.groove_q = 50;
        assert!((live_groove(&p, ti).unwrap().wait_at(4.25) - 0.025).abs() < 1e-6);
    }

    #[test]
    fn class_choices_read_and_write() {
        assert_eq!(parse_class("auto"), Some(None));
        assert_eq!(parse_class("hat"), Some(Some(DrumClass::ClosedHat)));
        assert_eq!(parse_class("Kick"), Some(Some(DrumClass::Kick)));
        assert_eq!(parse_class("loop"), None);
        assert_eq!(parse_class("nope"), None);
        for c in std::iter::once(None).chain(channel_rack::CHANNEL_CLASSES.map(Some)) {
            assert_eq!(parse_class(class_id(c)), Some(c));
        }
        assert_eq!(other_classes().count(), 13);
    }
}
