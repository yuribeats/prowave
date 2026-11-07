//! MIDI Transform (Logic Pro: Functions > Transform > MIDI Transform): changes the notes of a
//! region that meet some conditions in one go. The conditions say which notes (a range of
//! pitches, velocities and lengths); the operations say what happens to them (pitch, velocity,
//! length and position each get one). The presets of the Piano Roll's Functions menu
//! (Transpose, Fixed Velocity, Legato, Humanize) are sets of these.
use crate::model::Note;

/// Notes sorted into the same chord start within this many beats of each other.
const SAME_START: f32 = 1e-4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PitchOp {
    Keep,
    /// Semitones up (or down).
    Add(i8),
    Set(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VelocityOp {
    Keep,
    Add(i16),
    Set(u8),
    /// Percent of the velocity it has (10 - 400).
    Scale(u16),
    /// Up to this much louder or softer, a different amount for each note.
    Randomize(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LengthOp {
    Keep,
    /// Beats.
    Set(f32),
    /// Percent of the length it has (10 - 400).
    Scale(u16),
    /// Held until the next note starts (or, for the last, as it is).
    Legato,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PositionOp {
    Keep,
    /// Beats later (or earlier); notes do not go before the start of the region.
    Shift(f32),
    /// Pulled to the nearest line of a grid of this many beats, by this percent of the way.
    Quantize {
        grid: f32,
        strength: u8,
    },
    /// Up to this many beats early or late, a different amount for each note.
    Randomize(f32),
}

/// Which notes, and what is done to them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Lowest and highest pitch that match.
    pub pitch: (u8, u8),
    pub velocity: (u8, u8),
    /// Shortest and longest length that match, in beats.
    pub length: (f32, f32),
    pub pitch_op: PitchOp,
    pub velocity_op: VelocityOp,
    pub length_op: LengthOp,
    pub position_op: PositionOp,
}

impl Default for Transform {
    /// Every note, and nothing done to it.
    fn default() -> Self {
        Self {
            pitch: (0, 127),
            velocity: (1, 127),
            length: (0.0, f32::MAX),
            pitch_op: PitchOp::Keep,
            velocity_op: VelocityOp::Keep,
            length_op: LengthOp::Keep,
            position_op: PositionOp::Keep,
        }
    }
}

/// The ready-made transforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Transpose,
    FixedVelocity,
    Legato,
    Humanize,
    Quantize,
}

impl Preset {
    pub const ALL: [Preset; 5] = [
        Self::Transpose,
        Self::FixedVelocity,
        Self::Legato,
        Self::Humanize,
        Self::Quantize,
    ];

    pub fn transform(self) -> Transform {
        let base = Transform::default();
        match self {
            Self::Transpose => Transform {
                pitch_op: PitchOp::Add(12),
                ..base
            },
            Self::FixedVelocity => Transform {
                velocity_op: VelocityOp::Set(100),
                ..base
            },
            Self::Legato => Transform {
                length_op: LengthOp::Legato,
                ..base
            },
            Self::Humanize => Transform {
                velocity_op: VelocityOp::Randomize(8),
                position_op: PositionOp::Randomize(0.03),
                ..base
            },
            Self::Quantize => Transform {
                position_op: PositionOp::Quantize {
                    grid: 0.25,
                    strength: 100,
                },
                ..base
            },
        }
    }
}

impl Transform {
    /// Whether `n` meets the conditions.
    pub fn matches(&self, n: &Note) -> bool {
        (self.pitch.0..=self.pitch.1).contains(&n.pitch)
            && (self.velocity.0..=self.velocity.1).contains(&n.vel)
            && n.len >= self.length.0
            && n.len <= self.length.1
    }
}

/// A number from -1 to 1 for note `index`, the same every time for the same `seed`.
fn noise(seed: u32, index: usize, salt: u32) -> f32 {
    let mut x =
        u64::from(seed) ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ u64::from(salt);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^= x >> 33;
    (x >> 40) as f32 / 16_777_216.0 * 2.0 - 1.0
}

/// Applies `t` to the notes at `which` (all of them when `None`) that meet its conditions.
/// Returns how many were changed. `region_len` (beats) is where notes are kept inside.
pub fn apply(
    t: &Transform,
    notes: &mut [Note],
    which: Option<&[usize]>,
    region_len: f32,
    seed: u32,
) -> usize {
    let chosen: Vec<usize> = match which {
        Some(w) => w.iter().copied().filter(|i| *i < notes.len()).collect(),
        None => (0..notes.len()).collect(),
    };
    // For legato: every note start in the region, in order.
    let mut starts: Vec<f32> = notes.iter().map(|n| n.start).collect();
    starts.sort_by(f32::total_cmp);
    starts.dedup_by(|a, b| (*a - *b).abs() < SAME_START);
    let mut changed = 0;
    for i in chosen {
        let before = notes[i];
        if !t.matches(&before) {
            continue;
        }
        let mut n = before;
        n.pitch = match t.pitch_op {
            PitchOp::Keep => n.pitch,
            PitchOp::Add(d) => (i16::from(n.pitch) + i16::from(d)).clamp(0, 127) as u8,
            PitchOp::Set(p) => p.min(127),
        };
        n.vel = match t.velocity_op {
            VelocityOp::Keep => n.vel,
            VelocityOp::Add(d) => (i32::from(n.vel) + i32::from(d)).clamp(1, 127) as u8,
            VelocityOp::Set(v) => v.clamp(1, 127),
            VelocityOp::Scale(pct) => (f32::from(n.vel) * f32::from(pct.clamp(10, 400)) / 100.0)
                .round()
                .clamp(1.0, 127.0) as u8,
            VelocityOp::Randomize(amount) => (f32::from(n.vel)
                + noise(seed, i, 1) * f32::from(amount))
            .round()
            .clamp(1.0, 127.0) as u8,
        };
        // Position first, then length (legato looks at where the note now starts).
        n.start = match t.position_op {
            PositionOp::Keep => n.start,
            PositionOp::Shift(d) => n.start + if d.is_finite() { d } else { 0.0 },
            PositionOp::Quantize { grid, strength } => {
                if grid.is_finite() && grid > 1e-4 {
                    let target = (n.start / grid).round() * grid;
                    n.start + (target - n.start) * f32::from(strength.min(100)) / 100.0
                } else {
                    n.start
                }
            }
            PositionOp::Randomize(amount) => {
                n.start + noise(seed, i, 2) * if amount.is_finite() { amount } else { 0.0 }
            }
        }
        .clamp(0.0, region_len.max(0.0));
        n.len = match t.length_op {
            LengthOp::Keep => n.len,
            LengthOp::Set(l) => {
                if l.is_finite() {
                    l.max(1.0 / 960.0)
                } else {
                    n.len
                }
            }
            LengthOp::Scale(pct) => {
                (n.len * f32::from(pct.clamp(10, 400)) / 100.0).max(1.0 / 960.0)
            }
            LengthOp::Legato => {
                match starts
                    .iter()
                    .find(|s| **s > n.start + SAME_START)
                    .map(|s| s - n.start)
                {
                    Some(gap) => gap,
                    None => n.len,
                }
            }
        };
        if n != before {
            notes[i] = n;
            changed += 1;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(start: f32, len: f32, pitch: u8, vel: u8) -> Note {
        Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start,
            len,
            pitch,
            vel,
        }
    }

    fn song() -> Vec<Note> {
        vec![
            n(0.0, 0.5, 60, 100),
            n(1.0, 0.5, 64, 80),
            n(1.0, 0.5, 67, 80),
            n(3.0, 0.25, 72, 40),
        ]
    }

    #[test]
    fn conditions_pick_the_notes() {
        let t = Transform {
            pitch: (62, 70),
            ..Transform::default()
        };
        let hits: Vec<bool> = song().iter().map(|x| t.matches(x)).collect();
        assert_eq!(hits, [false, true, true, false]);
        let t = Transform {
            velocity: (50, 90),
            length: (0.4, 1.0),
            ..Transform::default()
        };
        let hits: Vec<bool> = song().iter().map(|x| t.matches(x)).collect();
        assert_eq!(hits, [false, true, true, false]);
        assert!(Transform::default().matches(&n(0.0, 99.0, 0, 1)));
    }

    #[test]
    fn pitch_velocity_and_length_operations() {
        let mut notes = song();
        let changed = apply(&Preset::Transpose.transform(), &mut notes, None, 8.0, 1);
        assert_eq!(changed, 4);
        assert_eq!(
            notes.iter().map(|x| x.pitch).collect::<Vec<_>>(),
            [72, 76, 79, 84]
        );
        // Only the notes that match, and only the ones asked for.
        let mut notes = song();
        let t = Transform {
            pitch: (0, 65),
            pitch_op: PitchOp::Set(10),
            ..Transform::default()
        };
        assert_eq!(apply(&t, &mut notes, Some(&[0, 1, 3, 99]), 8.0, 1), 2);
        assert_eq!(
            notes.iter().map(|x| x.pitch).collect::<Vec<_>>(),
            [10, 10, 67, 72]
        );
        // The top and bottom of the range stop.
        let mut notes = vec![n(0.0, 1.0, 125, 120), n(1.0, 1.0, 2, 3)];
        let t = Transform {
            pitch_op: PitchOp::Add(12),
            velocity_op: VelocityOp::Add(30),
            ..Transform::default()
        };
        apply(&t, &mut notes, None, 8.0, 1);
        assert_eq!(
            (notes[0].pitch, notes[0].vel, notes[1].pitch),
            (127, 127, 14)
        );
        let mut notes = song();
        apply(&Preset::FixedVelocity.transform(), &mut notes, None, 8.0, 1);
        assert!(notes.iter().all(|x| x.vel == 100));
        let mut notes = song();
        let t = Transform {
            velocity_op: VelocityOp::Scale(50),
            length_op: LengthOp::Scale(200),
            ..Transform::default()
        };
        apply(&t, &mut notes, None, 8.0, 1);
        assert_eq!((notes[0].vel, notes[0].len), (50, 1.0));
        let mut notes = song();
        let t = Transform {
            length_op: LengthOp::Set(2.0),
            ..Transform::default()
        };
        apply(&t, &mut notes, None, 8.0, 1);
        assert!(notes.iter().all(|x| x.len == 2.0));
        // Nothing to do: nothing is counted.
        assert_eq!(apply(&Transform::default(), &mut song(), None, 8.0, 1), 0);
    }

    #[test]
    fn legato_holds_each_note_until_the_next_one_starts() {
        let mut notes = song();
        apply(&Preset::Legato.transform(), &mut notes, None, 8.0, 1);
        // The note at 0 reaches the chord at 1; the chord reaches the note at 3; the last
        // note has nothing after it.
        assert_eq!(
            notes.iter().map(|x| x.len).collect::<Vec<_>>(),
            [1.0, 2.0, 2.0, 0.25]
        );
    }

    #[test]
    fn position_operations_keep_notes_inside_the_region() {
        let mut notes = vec![
            n(0.1, 0.5, 60, 90),
            n(1.37, 0.5, 60, 90),
            n(7.9, 0.5, 60, 90),
        ];
        let t = Transform {
            position_op: PositionOp::Quantize {
                grid: 0.25,
                strength: 100,
            },
            ..Transform::default()
        };
        apply(&t, &mut notes, None, 8.0, 1);
        assert_eq!(
            notes.iter().map(|x| x.start).collect::<Vec<_>>(),
            [0.0, 1.25, 8.0]
        );
        let mut notes = vec![n(1.0, 1.0, 60, 90)];
        let half = Transform {
            position_op: PositionOp::Quantize {
                grid: 1.0,
                strength: 50,
            },
            ..Transform::default()
        };
        notes[0].start = 1.4;
        apply(&half, &mut notes, None, 8.0, 1);
        assert!((notes[0].start - 1.2).abs() < 1e-6);
        let mut notes = vec![n(0.2, 1.0, 60, 90), n(7.9, 1.0, 60, 90)];
        let shift = Transform {
            position_op: PositionOp::Shift(-0.5),
            ..Transform::default()
        };
        apply(&shift, &mut notes, None, 8.0, 1);
        assert_eq!(notes[0].start, 0.0, "not before the start");
        let shift = Transform {
            position_op: PositionOp::Shift(5.0),
            ..Transform::default()
        };
        apply(&shift, &mut notes, None, 8.0, 1);
        assert_eq!(notes[1].start, 8.0, "not past the end");
        // Wild numbers do nothing wild.
        for op in [
            PositionOp::Shift(f32::NAN),
            PositionOp::Quantize {
                grid: 0.0,
                strength: 255,
            },
            PositionOp::Quantize {
                grid: f32::NAN,
                strength: 5,
            },
            PositionOp::Randomize(f32::INFINITY),
        ] {
            let mut notes = song();
            apply(
                &Transform {
                    position_op: op,
                    ..Transform::default()
                },
                &mut notes,
                None,
                8.0,
                1,
            );
            assert!(
                notes.iter().all(|x| x.start.is_finite() && x.start >= 0.0),
                "{op:?}"
            );
        }
    }

    #[test]
    fn humanize_is_random_but_the_same_every_time_and_within_its_limits() {
        let notes: Vec<Note> = (0..50).map(|i| n(i as f32 + 1.0, 0.5, 60, 90)).collect();
        let t = Preset::Humanize.transform();
        let mut a = notes.clone();
        apply(&t, &mut a, None, 100.0, 5);
        let mut b = notes.clone();
        apply(&t, &mut b, None, 100.0, 5);
        assert_eq!(a, b);
        let mut c = notes.clone();
        apply(&t, &mut c, None, 100.0, 6);
        assert_ne!(a, c, "another seed, another take");
        assert!(a.iter().all(|x| (82..=98).contains(&x.vel)));
        assert!(a
            .iter()
            .zip(&notes)
            .all(|(x, y)| (x.start - y.start).abs() <= 0.03 + 1e-6));
        assert!(
            a.iter().any(|x| x.vel != 90) && a.iter().zip(&notes).any(|(x, y)| x.start != y.start)
        );
    }

    #[test]
    fn the_presets_are_all_there() {
        for p in Preset::ALL {
            let t = p.transform();
            assert_ne!(t, Transform::default(), "{p:?} does something");
            let mut notes = song();
            apply(&t, &mut notes, None, 8.0, 1);
        }
    }
}
