//! MIDI FX (Logic Pro: the MIDI FX slot of a channel strip): a chain of effects on a track
//! that changes its notes before they reach the instrument. They work on the notes the track
//! plays (after region and track transpose, quantize and limits), so the notes in the regions
//! stay as they were recorded; export writes the changed notes.
//!
//! - **Transposer** moves every note by semitones.
//! - **Velocity Processor** adds to the velocity and scales it.
//! - **Scale Quantizer** moves each note to the nearest note of a scale.
//! - **Chord Trigger** plays a chord on every note.
//! - **Note Repeater** repeats each note, at a rate, for as long as it is held.
//! - **Arpeggiator** plays the notes held together one after the other.
//! - **Randomizer** shifts pitch, velocity and time by a random amount (the same every time).
//! - **Modulator** is an LFO that moves each note's velocity or pitch with its place in the
//!   cycle, or, for an External MIDI track, sends a controller (a filter, a volume) that follows
//!   the cycle all along the track.
use crate::pattern::StepRate;
use prowave_engine::NoteEvent;

/// Most effects in a chain.
pub const MAX_FX: usize = 8;
/// Most notes a track's chain hands on: a repeater or arpeggiator over very many notes stops
/// here instead of using all the memory.
pub const MAX_FX_NOTES: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArpPattern {
    Up,
    Down,
    UpDown,
    Random,
    /// The notes in the order they were played (by pitch when they start together).
    AsPlayed,
}

impl ArpPattern {
    pub const ALL: [ArpPattern; 5] = [
        Self::Up,
        Self::Down,
        Self::UpDown,
        Self::Random,
        Self::AsPlayed,
    ];

    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0) as u8
    }

    pub fn from_index(i: u8) -> Self {
        Self::ALL.get(i as usize).copied().unwrap_or(Self::Up)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordKind {
    Major,
    Minor,
    Seventh,
    MajorSeventh,
    MinorSeventh,
    Sus4,
    Fifth,
    Octave,
}

impl ChordKind {
    pub const ALL: [ChordKind; 8] = [
        Self::Major,
        Self::Minor,
        Self::Seventh,
        Self::MajorSeventh,
        Self::MinorSeventh,
        Self::Sus4,
        Self::Fifth,
        Self::Octave,
    ];

    /// Semitones above the note, besides the note itself.
    pub fn intervals(self) -> &'static [i32] {
        match self {
            Self::Major => &[4, 7],
            Self::Minor => &[3, 7],
            Self::Seventh => &[4, 7, 10],
            Self::MajorSeventh => &[4, 7, 11],
            Self::MinorSeventh => &[3, 7, 10],
            Self::Sus4 => &[5, 7],
            Self::Fifth => &[7],
            Self::Octave => &[12],
        }
    }

    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0) as u8
    }

    pub fn from_index(i: u8) -> Self {
        Self::ALL.get(i as usize).copied().unwrap_or(Self::Major)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfoShape {
    Sine,
    Triangle,
    Saw,
    Square,
    /// A new random value each cycle.
    Random,
}

impl LfoShape {
    pub const ALL: [LfoShape; 5] = [
        Self::Sine,
        Self::Triangle,
        Self::Saw,
        Self::Square,
        Self::Random,
    ];

    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as u8
    }

    pub fn from_index(i: u8) -> Self {
        Self::ALL.get(i as usize).copied().unwrap_or(Self::Sine)
    }

    /// The LFO at `phase` (0..1) of cycle number `cycle`, from -1 to 1.
    pub fn at(self, phase: f64, cycle: i64, seed: u32) -> f64 {
        let p = phase.rem_euclid(1.0);
        match self {
            Self::Sine => (p * std::f64::consts::TAU).sin(),
            Self::Triangle => {
                if p < 0.5 {
                    4.0 * p - 1.0
                } else {
                    3.0 - 4.0 * p
                }
            }
            Self::Saw => 2.0 * p - 1.0,
            Self::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::Random => f64::from(noise(seed, cycle as u64, 99)),
        }
    }
}

/// What a Modulator moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModTarget {
    /// Each note's velocity, up to 63 either way.
    Velocity,
    /// Each note's pitch, up to 12 semitones either way.
    Pitch,
    /// A MIDI controller sent to an External MIDI track, around the middle (64).
    Controller { number: u8 },
}

/// The lengths of a Modulator's cycle, in quarter notes.
pub const MOD_PERIODS: [f64; 8] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0];

/// A controller change a Modulator makes: beats from the start of the song.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModControl {
    pub beat: f64,
    pub number: u8,
    pub value: u8,
}

/// One effect and its settings.
#[derive(Clone, Debug, PartialEq)]
pub enum MidiFx {
    /// A program that changes the notes (see `scripter.rs`).
    Scripter {
        source: String,
    },
    Modulator {
        shape: LfoShape,
        /// Index into [`MOD_PERIODS`].
        period: u8,
        /// 0 - 100 percent of the full swing.
        depth: u8,
        target: ModTarget,
        seed: u32,
    },
    Transposer {
        semitones: i8,
    },
    Velocity {
        add: i8,
        /// 10 - 200 percent.
        scale: u8,
    },
    Scale {
        /// 0 = C.
        root: u8,
        minor: bool,
    },
    Chord {
        kind: ChordKind,
    },
    Repeater {
        rate: StepRate,
    },
    Arpeggiator {
        rate: StepRate,
        pattern: ArpPattern,
        octaves: u8,
        /// 5 - 100 percent of a step.
        gate: u8,
    },
    Randomizer {
        /// Up to this many semitones either way.
        pitch: u8,
        /// Up to this much velocity either way.
        velocity: u8,
        /// 0 - 100: up to a sixteenth note early or late.
        timing: u8,
        seed: u32,
    },
}

impl MidiFx {
    /// Every kind with its settings at their start values, in the order a menu lists them.
    pub fn all_defaults() -> [MidiFx; 9] {
        [
            Self::Scripter {
                source: crate::scripter::DEFAULT_SCRIPT.to_string(),
            },
            Self::Modulator {
                shape: LfoShape::Sine,
                period: 3,
                depth: 50,
                target: ModTarget::Velocity,
                seed: 1,
            },
            Self::Transposer { semitones: 0 },
            Self::Velocity { add: 0, scale: 100 },
            Self::Scale {
                root: 0,
                minor: false,
            },
            Self::Chord {
                kind: ChordKind::Major,
            },
            Self::Repeater {
                rate: StepRate::Sixteenth,
            },
            Self::Arpeggiator {
                rate: StepRate::Sixteenth,
                pattern: ArpPattern::Up,
                octaves: 1,
                gate: 80,
            },
            Self::Randomizer {
                pitch: 0,
                velocity: 10,
                timing: 0,
                seed: 1,
            },
        ]
    }

    /// How it is written in the project file.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Scripter { .. } => "scripter",
            Self::Modulator { .. } => "modulator",
            Self::Transposer { .. } => "transposer",
            Self::Velocity { .. } => "velocity",
            Self::Scale { .. } => "scale",
            Self::Chord { .. } => "chord",
            Self::Repeater { .. } => "repeater",
            Self::Arpeggiator { .. } => "arpeggiator",
            Self::Randomizer { .. } => "randomizer",
        }
    }

    /// The settings as numbers, for the project file.
    pub fn numbers(&self) -> Vec<f32> {
        match *self {
            Self::Scripter { .. } => Vec::new(),
            Self::Modulator {
                shape,
                period,
                depth,
                target,
                seed,
            } => {
                let (kind, number) = match target {
                    ModTarget::Velocity => (0u8, 0u8),
                    ModTarget::Pitch => (1u8, 0u8),
                    ModTarget::Controller { number } => (2u8, number),
                };
                vec![
                    f32::from(shape.index()),
                    f32::from(period),
                    f32::from(depth),
                    f32::from(kind),
                    f32::from(number),
                    seed as f32,
                ]
            }
            Self::Transposer { semitones } => vec![f32::from(semitones)],
            Self::Velocity { add, scale } => vec![f32::from(add), f32::from(scale)],
            Self::Scale { root, minor } => vec![f32::from(root), f32::from(u8::from(minor))],
            Self::Chord { kind } => vec![f32::from(kind.index())],
            Self::Repeater { rate } => vec![rate_index(rate) as f32],
            Self::Arpeggiator {
                rate,
                pattern,
                octaves,
                gate,
            } => vec![
                rate_index(rate) as f32,
                f32::from(pattern.index()),
                f32::from(octaves),
                f32::from(gate),
            ],
            Self::Randomizer {
                pitch,
                velocity,
                timing,
                seed,
            } => vec![
                f32::from(pitch),
                f32::from(velocity),
                f32::from(timing),
                seed as f32,
            ],
        }
    }

    /// The effect called `name` with settings `n` (missing ones start values, wild ones
    /// repaired), if there is one.
    pub fn from_numbers(name: &str, n: &[f32]) -> Option<MidiFx> {
        Self::from_parts(name, n, "")
    }

    /// [`MidiFx::from_numbers`] for a file's effect: the scripter's script comes along.
    pub fn from_parts(name: &str, n: &[f32], script: &str) -> Option<MidiFx> {
        let get = |i: usize, default: f32| {
            n.get(i)
                .copied()
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        let int = |i: usize, default: f32, lo: f32, hi: f32| get(i, default).round().clamp(lo, hi);
        let fx = match name {
            "scripter" => Self::Scripter {
                source: script
                    .chars()
                    .take(crate::scripter::MAX_SCRIPT_CHARS)
                    .collect(),
            },
            "modulator" => Self::Modulator {
                shape: LfoShape::from_index(int(0, 0.0, 0.0, 255.0) as u8),
                period: int(1, 3.0, 0.0, (MOD_PERIODS.len() - 1) as f32) as u8,
                depth: int(2, 50.0, 0.0, 100.0) as u8,
                target: match int(3, 0.0, 0.0, 2.0) as u8 {
                    0 => ModTarget::Velocity,
                    1 => ModTarget::Pitch,
                    _ => ModTarget::Controller {
                        number: int(4, 1.0, 0.0, 127.0) as u8,
                    },
                },
                seed: get(5, 1.0).max(0.0) as u32,
            },
            "transposer" => Self::Transposer {
                semitones: int(0, 0.0, -48.0, 48.0) as i8,
            },
            "velocity" => Self::Velocity {
                add: int(0, 0.0, -127.0, 127.0) as i8,
                scale: int(1, 100.0, 10.0, 200.0) as u8,
            },
            "scale" => Self::Scale {
                root: int(0, 0.0, 0.0, 11.0) as u8,
                minor: get(1, 0.0) >= 0.5,
            },
            "chord" => Self::Chord {
                kind: ChordKind::from_index(int(0, 0.0, 0.0, 255.0) as u8),
            },
            "repeater" => Self::Repeater {
                rate: rate_of_index(int(0, 2.0, 0.0, 255.0) as usize),
            },
            "arpeggiator" => Self::Arpeggiator {
                rate: rate_of_index(int(0, 2.0, 0.0, 255.0) as usize),
                pattern: ArpPattern::from_index(int(1, 0.0, 0.0, 255.0) as u8),
                octaves: int(2, 1.0, 1.0, 4.0) as u8,
                gate: int(3, 80.0, 5.0, 100.0) as u8,
            },
            "randomizer" => Self::Randomizer {
                pitch: int(0, 0.0, 0.0, 24.0) as u8,
                velocity: int(1, 10.0, 0.0, 127.0) as u8,
                timing: int(2, 0.0, 0.0, 100.0) as u8,
                seed: get(3, 1.0).max(0.0) as u32,
            },
            _ => return None,
        };
        Some(fx)
    }
}

fn rate_index(rate: StepRate) -> usize {
    StepRate::ALL.iter().position(|r| *r == rate).unwrap_or(2)
}

fn rate_of_index(i: usize) -> StepRate {
    StepRate::ALL.get(i).copied().unwrap_or(StepRate::Sixteenth)
}

/// An effect in a chain, which can be switched off without losing its settings.
#[derive(Clone, Debug, PartialEq)]
pub struct FxSlot {
    pub fx: MidiFx,
    pub on: bool,
}

/// A small generator for the same numbers every time.
fn noise(seed: u32, a: u64, b: u64) -> f32 {
    let mut x = u64::from(seed)
        ^ a.wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ b.wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^= x >> 33;
    (x >> 40) as f32 / 16_777_216.0 * 2.0 - 1.0
}

const MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];

/// `pitch` moved to the nearest note of the scale (down first when two are as near).
pub fn in_scale(pitch: u8, root: u8, minor: bool) -> u8 {
    let scale = if minor { &MINOR } else { &MAJOR };
    let mut best = pitch;
    let mut best_d = i32::MAX;
    for delta in 0..=6i32 {
        for sign in [-1, 1] {
            let p = i32::from(pitch) + sign * delta;
            if !(0..=127).contains(&p) {
                continue;
            }
            let degree = ((p - i32::from(root)).rem_euclid(12)) as u8;
            if scale.contains(&degree) && delta < best_d {
                best_d = delta;
                best = p as u8;
            }
        }
        if best_d != i32::MAX {
            break;
        }
    }
    best
}

fn clamp_pitch(p: i32) -> Option<u8> {
    (0..=127).contains(&p).then_some(p as u8)
}

/// Runs `notes` (absolute beats, any order) through the chain; the result is sorted by start.
pub fn apply(chain: &[FxSlot], mut notes: Vec<NoteEvent>) -> Vec<NoteEvent> {
    for slot in chain.iter().filter(|s| s.on) {
        notes = run(&slot.fx, notes);
        notes.truncate(MAX_FX_NOTES);
    }
    notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
    notes
}

/// The LFO of a Modulator at `beat`, from -1 to 1.
fn lfo(shape: LfoShape, period: u8, seed: u32, beat: f64) -> f64 {
    let len = MOD_PERIODS[(period as usize).min(MOD_PERIODS.len() - 1)];
    let cycles = beat / len;
    shape.at(cycles, cycles.floor() as i64, seed)
}

/// The controller changes the Modulators of `chain` make between the beats `from` and `to`: a
/// change each time the controller's value moves, at the smallest every sixteenth of a beat.
pub fn controls(chain: &[FxSlot], from: f64, to: f64) -> Vec<ModControl> {
    let mut out = Vec::new();
    if !(from.is_finite() && to.is_finite()) || to <= from {
        return out;
    }
    for slot in chain.iter().filter(|s| s.on) {
        let MidiFx::Modulator {
            shape,
            period,
            depth,
            target: ModTarget::Controller { number },
            seed,
        } = slot.fx
        else {
            continue;
        };
        let len = MOD_PERIODS[(period as usize).min(MOD_PERIODS.len() - 1)];
        let step = (len / 16.0).max(1.0 / 16.0);
        let mut last: Option<u8> = None;
        let mut beat = from;
        let mut count = 0usize;
        while beat <= to && count < MAX_FX_NOTES {
            let swing = lfo(shape, period, seed, beat) * f64::from(depth) / 100.0 * 63.0;
            let value = (64.0 + swing).round().clamp(0.0, 127.0) as u8;
            if last != Some(value) {
                out.push(ModControl {
                    beat,
                    number,
                    value,
                });
                last = Some(value);
                count += 1;
            }
            beat += step;
        }
    }
    out.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    out
}

fn run(fx: &MidiFx, notes: Vec<NoteEvent>) -> Vec<NoteEvent> {
    match *fx {
        // A script that fails leaves the notes as they were (the window says what is wrong).
        MidiFx::Scripter { ref source } => crate::scripter::run(source, &notes).unwrap_or(notes),
        MidiFx::Modulator {
            shape,
            period,
            depth,
            target,
            seed,
        } => notes
            .into_iter()
            .filter_map(|n| {
                let x = lfo(shape, period, seed, n.start_beat) * f64::from(depth) / 100.0;
                match target {
                    ModTarget::Velocity => Some(NoteEvent {
                        vel: (f64::from(n.vel) + x * 63.0).round().clamp(1.0, 127.0) as u8,
                        ..n
                    }),
                    ModTarget::Pitch => Some(NoteEvent {
                        pitch: clamp_pitch(i32::from(n.pitch) + (x * 12.0).round() as i32)?,
                        ..n
                    }),
                    // A controller does not change the notes.
                    ModTarget::Controller { .. } => Some(n),
                }
            })
            .collect(),
        MidiFx::Transposer { semitones } => notes
            .into_iter()
            .filter_map(|n| {
                let pitch = clamp_pitch(i32::from(n.pitch) + i32::from(semitones))?;
                Some(NoteEvent { pitch, ..n })
            })
            .collect(),
        MidiFx::Velocity { add, scale } => notes
            .into_iter()
            .map(|n| {
                let v =
                    (f32::from(n.vel) * f32::from(scale) / 100.0).round() as i32 + i32::from(add);
                NoteEvent {
                    vel: v.clamp(1, 127) as u8,
                    ..n
                }
            })
            .collect(),
        MidiFx::Scale { root, minor } => notes
            .into_iter()
            .map(|n| NoteEvent {
                pitch: in_scale(n.pitch, root, minor),
                ..n
            })
            .collect(),
        MidiFx::Chord { kind } => {
            let mut out = Vec::with_capacity((notes.len() * 3).min(MAX_FX_NOTES));
            for n in notes {
                if out.len() >= MAX_FX_NOTES {
                    break;
                }
                out.push(n);
                for &i in kind.intervals() {
                    if let Some(pitch) = clamp_pitch(i32::from(n.pitch) + i) {
                        out.push(NoteEvent { pitch, ..n });
                    }
                }
            }
            out
        }
        MidiFx::Repeater { rate } => {
            let step = rate.beats();
            let mut out = Vec::new();
            for n in notes {
                if out.len() >= MAX_FX_NOTES {
                    break;
                }
                let mut at = n.start_beat;
                let mut count = 0;
                while at < n.end_beat - 1e-6 && count < 512 {
                    out.push(NoteEvent {
                        start_beat: at,
                        end_beat: (at + step * 0.9).min(n.end_beat),
                        ..n
                    });
                    at += step;
                    count += 1;
                }
            }
            out
        }
        MidiFx::Arpeggiator {
            rate,
            pattern,
            octaves,
            gate,
        } => arpeggiate(notes, rate.beats(), pattern, octaves.clamp(1, 4), gate),
        MidiFx::Randomizer {
            pitch,
            velocity,
            timing,
            seed,
        } => notes
            .into_iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let key = (n.start_beat * 960.0) as u64;
                let id = i as u64 ^ (u64::from(n.pitch) << 40);
                let dp = (noise(seed, key, id) * (f32::from(pitch) + 0.999)).trunc() as i32;
                let dv = (noise(seed ^ 1, key, id) * (f32::from(velocity) + 0.999)).trunc() as i32;
                let dt = f64::from(noise(seed ^ 2, key, id)) * f64::from(timing) / 100.0 * 0.25;
                let p = clamp_pitch(i32::from(n.pitch) + dp)?;
                let start = (n.start_beat + dt).max(0.0);
                Some(NoteEvent {
                    start_beat: start,
                    end_beat: (n.end_beat + (start - n.start_beat)).max(start + 1e-3),
                    pitch: p,
                    vel: (i32::from(n.vel) + dv).clamp(1, 127) as u8,
                    ..n
                })
            })
            .collect(),
    }
}

/// Replaces each group of notes that start together by the notes one after the other.
fn arpeggiate(
    mut notes: Vec<NoteEvent>,
    step: f64,
    pattern: ArpPattern,
    octaves: u8,
    gate: u8,
) -> Vec<NoteEvent> {
    notes.sort_by(|a, b| {
        a.start_beat
            .total_cmp(&b.start_beat)
            .then(a.pitch.cmp(&b.pitch))
    });
    let mut out = Vec::new();
    let mut i = 0;
    while i < notes.len() {
        // The notes that start within a hair of the first one are a chord, on the same track.
        let first = notes[i];
        let mut j = i;
        while j < notes.len()
            && notes[j].start_beat - first.start_beat < 0.02
            && notes[j].track == first.track
        {
            j += 1;
        }
        let group = &notes[i..j];
        let end = group
            .iter()
            .map(|n| n.end_beat)
            .fold(first.end_beat, f64::max);
        let vel = group.iter().map(|n| n.vel).max().unwrap_or(first.vel);
        let mut pitches: Vec<u8> = Vec::new();
        for o in 0..octaves {
            pitches.extend(
                group
                    .iter()
                    .filter_map(|n| clamp_pitch(i32::from(n.pitch) + 12 * i32::from(o))),
            );
        }
        let order = sequence(&pitches, pattern, first.start_beat);
        let mut at = first.start_beat;
        let mut k = 0usize;
        while at < end - 1e-6 && !order.is_empty() && k < 4096 && out.len() < MAX_FX_NOTES {
            out.push(NoteEvent {
                start_beat: at,
                end_beat: (at + step * f64::from(gate) / 100.0).min(end),
                pitch: order[k % order.len()],
                vel,
                track: first.track,
            });
            at += step;
            k += 1;
        }
        i = j;
    }
    out
}

/// The order the arpeggio plays `pitches` in, over one run (it then starts again).
fn sequence(pitches: &[u8], pattern: ArpPattern, seed: f64) -> Vec<u8> {
    let mut up = pitches.to_vec();
    up.sort_unstable();
    up.dedup();
    match pattern {
        ArpPattern::Up => up,
        ArpPattern::Down => {
            up.reverse();
            up
        }
        ArpPattern::UpDown => {
            let mut v = up.clone();
            if up.len() > 2 {
                v.extend(up[1..up.len() - 1].iter().rev());
            } else if up.len() == 2 {
                v.push(up[0]);
                v.pop();
            }
            v
        }
        ArpPattern::Random => {
            let mut v = up;
            let key = (seed * 960.0) as u64;
            for k in (1..v.len()).rev() {
                let r = ((noise(7, key, k as u64) + 1.0) / 2.0 * (k as f32 + 1.0)) as usize;
                v.swap(k, r.min(k));
            }
            v
        }
        ArpPattern::AsPlayed => pitches.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: f64, end: f64, pitch: u8) -> NoteEvent {
        NoteEvent {
            start_beat: start,
            end_beat: end,
            pitch,
            vel: 100,
            track: 3,
        }
    }

    fn slot(fx: MidiFx) -> FxSlot {
        FxSlot { fx, on: true }
    }

    fn pitches(notes: &[NoteEvent]) -> Vec<u8> {
        notes.iter().map(|n| n.pitch).collect()
    }

    #[test]
    fn transposer_and_velocity_move_notes_and_drop_what_leaves_the_range() {
        let notes = vec![note(0.0, 1.0, 60), note(1.0, 2.0, 120)];
        let up = apply(&[slot(MidiFx::Transposer { semitones: 12 })], notes.clone());
        assert_eq!(pitches(&up), [72], "127 is the top");
        let down = apply(
            &[slot(MidiFx::Transposer { semitones: -12 })],
            notes.clone(),
        );
        assert_eq!(pitches(&down), [48, 108]);
        let v = apply(
            &[slot(MidiFx::Velocity { add: 20, scale: 50 })],
            notes.clone(),
        );
        assert_eq!(v[0].vel, 70);
        let hard = apply(
            &[slot(MidiFx::Velocity {
                add: 100,
                scale: 200,
            })],
            notes,
        );
        assert_eq!(hard[0].vel, 127);
    }

    #[test]
    fn a_switched_off_effect_does_nothing_and_a_chain_runs_in_order() {
        let notes = vec![note(0.0, 1.0, 60)];
        let off = FxSlot {
            fx: MidiFx::Transposer { semitones: 5 },
            on: false,
        };
        assert_eq!(apply(&[off], notes.clone()), notes);
        // Chord then transpose: the whole chord moves. Transpose then scale: a note off the
        // scale is pulled in afterwards.
        let chain = [
            slot(MidiFx::Chord {
                kind: ChordKind::Major,
            }),
            slot(MidiFx::Transposer { semitones: 2 }),
        ];
        assert_eq!(pitches(&apply(&chain, notes.clone())), [62, 66, 69]);
        let chain = [
            slot(MidiFx::Transposer { semitones: 1 }),
            slot(MidiFx::Scale {
                root: 0,
                minor: false,
            }),
        ];
        assert_eq!(
            pitches(&apply(&chain, notes)),
            [60],
            "61 is not in C major: down to 60"
        );
    }

    #[test]
    fn the_scale_quantizer_picks_the_nearest_note_of_the_scale() {
        // C major: black keys go down (the nearer white key is a tie, so the lower one).
        let major: Vec<u8> = (60..72).map(|p| in_scale(p, 0, false)).collect();
        assert_eq!(major, [60, 60, 62, 62, 64, 65, 65, 67, 67, 69, 69, 71]);
        // A minor from root 9 has the same notes as C major; C minor has Eb.
        assert_eq!(in_scale(63, 0, true), 63);
        assert_eq!(in_scale(64, 0, true), 63);
        assert_eq!(in_scale(61, 9, true), 60, "A minor, 61 -> 60");
        assert!((0..128).all(|p| in_scale(p, 5, false) <= 127));
    }

    #[test]
    fn a_chord_trigger_adds_the_chord_on_every_note() {
        let notes = vec![note(0.0, 1.0, 60), note(2.0, 3.0, 125)];
        for kind in ChordKind::ALL {
            let out = apply(&[slot(MidiFx::Chord { kind })], notes.clone());
            assert!(out.len() >= 2 + kind.intervals().len() - 1, "{kind:?}");
            assert!(out.iter().all(|n| n.pitch <= 127 && n.track == 3));
        }
        let m = apply(
            &[slot(MidiFx::Chord {
                kind: ChordKind::MinorSeventh,
            })],
            notes,
        );
        assert_eq!(pitches(&m[..4]), [60, 63, 67, 70]);
        assert!(
            m[4..].iter().all(|n| n.start_beat == 2.0) && m.len() == 4 + 1,
            "the top of 125 is cut"
        );
    }

    #[test]
    fn the_repeater_repeats_a_held_note_at_its_rate() {
        let out = apply(
            &[slot(MidiFx::Repeater {
                rate: StepRate::Eighth,
            })],
            vec![note(1.0, 3.0, 60), note(4.0, 4.2, 62)],
        );
        let starts: Vec<f64> = out.iter().map(|n| n.start_beat).collect();
        assert_eq!(starts, [1.0, 1.5, 2.0, 2.5, 4.0]);
        assert!(out.iter().all(|n| n.end_beat - n.start_beat <= 0.45 + 1e-9));
        assert!(out
            .iter()
            .all(|n| n.end_beat <= 4.2 + 1e-9 || n.start_beat < 4.0));
    }

    #[test]
    fn the_arpeggiator_plays_a_held_chord_one_note_at_a_time() {
        let chord = vec![note(0.0, 4.0, 67), note(0.0, 4.0, 60), note(0.0, 4.0, 64)];
        let arp = |pattern, octaves| {
            apply(
                &[slot(MidiFx::Arpeggiator {
                    rate: StepRate::Quarter,
                    pattern,
                    octaves,
                    gate: 50,
                })],
                chord.clone(),
            )
        };
        assert_eq!(pitches(&arp(ArpPattern::Up, 1)), [60, 64, 67, 60]);
        let eighths = apply(
            &[slot(MidiFx::Arpeggiator {
                rate: StepRate::Eighth,
                pattern: ArpPattern::Up,
                octaves: 1,
                gate: 100,
            })],
            chord.clone(),
        );
        assert_eq!(pitches(&eighths), [60, 64, 67, 60, 64, 67, 60, 64]);
        assert_eq!(eighths[1].start_beat, 0.5);
        assert!((eighths[0].end_beat - 0.5).abs() < 1e-9);
        let two = apply(
            &[slot(MidiFx::Arpeggiator {
                rate: StepRate::Eighth,
                pattern: ArpPattern::Down,
                octaves: 2,
                gate: 100,
            })],
            chord.clone(),
        );
        assert_eq!(pitches(&two[..6]), [79, 76, 72, 67, 64, 60]);
        let updown = apply(
            &[slot(MidiFx::Arpeggiator {
                rate: StepRate::Eighth,
                pattern: ArpPattern::UpDown,
                octaves: 1,
                gate: 100,
            })],
            chord.clone(),
        );
        assert_eq!(pitches(&updown[..6]), [60, 64, 67, 64, 60, 64]);
        // Random is a shuffle of the same notes, the same every time.
        let r1 = pitches(&arp(ArpPattern::Random, 1));
        assert_eq!(r1, pitches(&arp(ArpPattern::Random, 1)));
        // Two chords one after the other each get their own run, a single note is repeated.
        let two_chords = apply(
            &[slot(MidiFx::Arpeggiator {
                rate: StepRate::Quarter,
                pattern: ArpPattern::Up,
                octaves: 1,
                gate: 100,
            })],
            vec![note(0.0, 2.0, 60), note(0.0, 2.0, 64), note(2.0, 4.0, 62)],
        );
        assert_eq!(pitches(&two_chords), [60, 64, 62, 62]);
        assert!(apply(
            &[slot(MidiFx::Arpeggiator {
                rate: StepRate::Quarter,
                pattern: ArpPattern::Up,
                octaves: 1,
                gate: 50
            })],
            vec![]
        )
        .is_empty());
    }

    #[test]
    fn the_randomizer_is_random_but_the_same_every_time_and_within_its_limits() {
        let notes: Vec<NoteEvent> = (0..40)
            .map(|i| note(i as f64, i as f64 + 0.5, 60))
            .collect();
        let fx = MidiFx::Randomizer {
            pitch: 3,
            velocity: 20,
            timing: 50,
            seed: 5,
        };
        let a = apply(&[slot(fx.clone())], notes.clone());
        assert_eq!(a, apply(&[slot(fx)], notes.clone()));
        assert_eq!(a.len(), 40);
        assert!(a.iter().all(|n| (57..=63).contains(&n.pitch)));
        assert!(a.iter().all(|n| (80..=120).contains(&n.vel)));
        assert!(a.iter().any(|n| n.pitch != 60) && a.iter().any(|n| n.vel != 100));
        assert!(a
            .iter()
            .all(|n| n.end_beat > n.start_beat && n.start_beat >= 0.0));
        let moved = a
            .iter()
            .zip(&notes)
            .map(|(x, y)| (x.start_beat - y.start_beat).abs())
            .fold(0.0, f64::max);
        assert!(moved > 0.01 && moved <= 0.125 + 1e-9, "{moved}");
        // With nothing to randomise, nothing changes.
        let none = MidiFx::Randomizer {
            pitch: 0,
            velocity: 0,
            timing: 0,
            seed: 1,
        };
        assert_eq!(apply(&[slot(none)], notes.clone()), notes);
    }

    #[test]
    fn the_lfo_shapes_swing_between_minus_one_and_one() {
        for shape in LfoShape::ALL {
            let values: Vec<f64> = (0..64).map(|i| shape.at(i as f64 / 64.0, 0, 3)).collect();
            assert!(values.iter().all(|v| (-1.0..=1.0).contains(v)), "{shape:?}");
            assert_eq!(LfoShape::from_index(shape.index()), shape);
        }
        assert!((LfoShape::Sine.at(0.25, 0, 0) - 1.0).abs() < 1e-12);
        assert!(LfoShape::Sine.at(0.0, 0, 0).abs() < 1e-12);
        assert_eq!(LfoShape::Triangle.at(0.0, 0, 0), -1.0);
        assert_eq!(LfoShape::Triangle.at(0.5, 0, 0), 1.0);
        assert_eq!(LfoShape::Saw.at(0.0, 0, 0), -1.0);
        assert_eq!(LfoShape::Square.at(0.1, 0, 0), 1.0);
        assert_eq!(LfoShape::Square.at(0.6, 0, 0), -1.0);
        // Random holds a value for a cycle and differs from cycle to cycle.
        let a = LfoShape::Random.at(0.2, 5, 1);
        assert_eq!(a, LfoShape::Random.at(0.9, 5, 1));
        assert_ne!(a, LfoShape::Random.at(0.2, 6, 1));
        // Phases before the start work too.
        assert!(LfoShape::Sine.at(-0.25, -1, 0) < 0.0);
    }

    #[test]
    fn a_modulator_moves_velocity_or_pitch_with_the_cycle() {
        // A sine of 2 beats: 0 at beat 0, the top at 0.5, 0 at 1, the bottom at 1.5.
        let notes = vec![
            note(0.0, 0.2, 60),
            note(0.5, 0.7, 60),
            note(1.0, 1.2, 60),
            note(1.5, 1.7, 60),
        ];
        let fx = |target| {
            slot(MidiFx::Modulator {
                shape: LfoShape::Sine,
                period: 3,
                depth: 100,
                target,
                seed: 1,
            })
        };
        let v = apply(&[fx(ModTarget::Velocity)], notes.clone());
        assert_eq!(
            v.iter().map(|n| n.vel).collect::<Vec<_>>(),
            [100, 127, 100, 37]
        );
        let p = apply(&[fx(ModTarget::Pitch)], notes.clone());
        assert_eq!(pitches(&p), [60, 72, 60, 48]);
        // Half the depth, half the swing.
        let half = apply(
            &[slot(MidiFx::Modulator {
                shape: LfoShape::Sine,
                period: 3,
                depth: 50,
                target: ModTarget::Pitch,
                seed: 1,
            })],
            notes.clone(),
        );
        assert_eq!(pitches(&half), [60, 66, 60, 54]);
        // A controller does not touch the notes.
        let c = apply(&[fx(ModTarget::Controller { number: 74 })], notes.clone());
        assert_eq!(c, notes);
    }

    #[test]
    fn a_controller_modulator_sends_the_cycle_as_controller_changes() {
        let chain = [slot(MidiFx::Modulator {
            shape: LfoShape::Triangle,
            period: 4, // 4 beats
            depth: 100,
            target: ModTarget::Controller { number: 74 },
            seed: 1,
        })];
        let c = controls(&chain, 0.0, 8.0);
        assert!(c.iter().all(|x| x.number == 74 && x.value <= 127));
        assert!(c.windows(2).all(|w| w[0].beat <= w[1].beat));
        // A triangle starts at the bottom, reaches the top at the middle of the cycle.
        assert_eq!(
            c[0],
            ModControl {
                beat: 0.0,
                number: 74,
                value: 1
            }
        );
        let top = c.iter().find(|x| x.beat == 2.0).unwrap();
        assert_eq!(top.value, 127);
        let max_gap = c
            .windows(2)
            .map(|w| w[1].beat - w[0].beat)
            .fold(0.0, f64::max);
        assert!(
            max_gap <= 0.25 + 1e-9,
            "a change at least every quarter beat: {max_gap}"
        );
        // A value that does not move is not sent again.
        let flat = [slot(MidiFx::Modulator {
            shape: LfoShape::Sine,
            period: 3,
            depth: 0,
            target: ModTarget::Controller { number: 7 },
            seed: 1,
        })];
        assert_eq!(
            controls(&flat, 0.0, 100.0),
            [ModControl {
                beat: 0.0,
                number: 7,
                value: 64
            }]
        );
        // Modulators of other targets, switched off ones and empty spans send nothing.
        let off = FxSlot {
            fx: chain[0].fx.clone(),
            on: false,
        };
        assert!(controls(&[off], 0.0, 8.0).is_empty());
        assert!(controls(&chain, 5.0, 5.0).is_empty());
        assert!(controls(&chain, f64::NAN, 5.0).is_empty());
        let velocity = [slot(MidiFx::Modulator {
            shape: LfoShape::Sine,
            period: 3,
            depth: 50,
            target: ModTarget::Velocity,
            seed: 1,
        })];
        assert!(controls(&velocity, 0.0, 8.0).is_empty());
    }

    #[test]
    fn settings_round_trip_through_numbers_and_wild_ones_are_repaired() {
        for fx in MidiFx::all_defaults() {
            let script = match &fx {
                MidiFx::Scripter { source } => source.clone(),
                _ => String::new(),
            };
            assert_eq!(
                MidiFx::from_parts(fx.name(), &fx.numbers(), &script),
                Some(fx)
            );
        }
        let fx = MidiFx::Arpeggiator {
            rate: StepRate::EighthTriplet,
            pattern: ArpPattern::UpDown,
            octaves: 3,
            gate: 40,
        };
        assert_eq!(MidiFx::from_numbers("arpeggiator", &fx.numbers()), Some(fx));
        assert_eq!(MidiFx::from_numbers("polka", &[]), None);
        let m = MidiFx::Modulator {
            shape: LfoShape::Random,
            period: 5,
            depth: 70,
            target: ModTarget::Controller { number: 74 },
            seed: 9,
        };
        assert_eq!(MidiFx::from_numbers("modulator", &m.numbers()), Some(m));
        let wild_mod =
            MidiFx::from_numbers("modulator", &[99.0, 99.0, 999.0, 2.0, 999.0, -5.0]).unwrap();
        assert_eq!(
            wild_mod,
            MidiFx::Modulator {
                shape: LfoShape::Sine,
                period: 7,
                depth: 100,
                target: ModTarget::Controller { number: 127 },
                seed: 0
            }
        );
        let wild = MidiFx::from_numbers("arpeggiator", &[99.0, 99.0, 99.0, 999.0]).unwrap();
        assert_eq!(
            wild,
            MidiFx::Arpeggiator {
                rate: StepRate::Sixteenth,
                pattern: ArpPattern::Up,
                octaves: 4,
                gate: 100
            }
        );
        let nan = MidiFx::from_numbers("transposer", &[f32::NAN]).unwrap();
        assert_eq!(nan, MidiFx::Transposer { semitones: 0 });
        assert_eq!(
            MidiFx::from_numbers("velocity", &[]),
            Some(MidiFx::Velocity { add: 0, scale: 100 })
        );
    }

    #[test]
    fn effects_stop_at_the_limit_instead_of_using_all_the_memory() {
        let notes: Vec<NoteEvent> = (0..5_000)
            .map(|i| note(i as f64, i as f64 + 1.0e6, 60))
            .collect();
        let chain = [
            slot(MidiFx::Repeater {
                rate: StepRate::ThirtySecond,
            }),
            slot(MidiFx::Chord {
                kind: ChordKind::Seventh,
            }),
        ];
        let began = std::time::Instant::now();
        let out = apply(&chain, notes);
        assert!(out.len() <= MAX_FX_NOTES);
        assert!(began.elapsed().as_secs() < 20, "{:?}", began.elapsed());
        // Many chords one after another through the arpeggiator.
        let chords: Vec<NoteEvent> = (0..20_000)
            .flat_map(|i| {
                (0..4).map(move |k| note(i as f64 * 200.0, i as f64 * 200.0 + 199.0, 40 + k * 4))
            })
            .collect();
        let out = apply(
            &[slot(MidiFx::Arpeggiator {
                rate: StepRate::ThirtySecond,
                pattern: ArpPattern::Random,
                octaves: 4,
                gate: 100,
            })],
            chords,
        );
        assert!(out.len() <= MAX_FX_NOTES);
    }
}
