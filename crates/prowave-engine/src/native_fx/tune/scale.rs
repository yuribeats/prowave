//! The notes Pro Wave Tune corrects to: a key and scale as a set of pitch classes, the nearest
//! allowed note to a sung pitch (with a little hysteresis, so a pitch between two notes does
//! not flicker between them), and note names for the meters.

/// The scale choices, in the Scale parameter's order (0 = the project's own).
pub const SCALES: [&str; 9] = [
    "Project",
    "Major",
    "Minor",
    "Harmonic Minor",
    "Major Pentatonic",
    "Minor Pentatonic",
    "Blues",
    "Chromatic",
    "Custom",
];
pub const SCALE_PROJECT: i32 = 0;
pub const SCALE_MAJOR: i32 = 1;
pub const SCALE_MINOR: i32 = 2;
pub const SCALE_CHROMATIC: i32 = 7;
pub const SCALE_CUSTOM: i32 = 8;

/// The key choices (0 = the project's key, then C to B).
pub const KEYS: [&str; 13] = [
    "Project", "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

/// Pitch-class names, sharps for black keys except Eb, Ab and Bb (as the key list).
pub const NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

/// Every pitch class on.
pub const ALL_NOTES: u16 = 0x0fff;

/// The steps of each scale from its root, as a bit set (bit 0 the root).
fn steps(scale: i32, minor_project: bool) -> u16 {
    let bits = |s: &[u8]| s.iter().fold(0u16, |m, k| m | 1 << k);
    match scale {
        SCALE_PROJECT => {
            if minor_project {
                bits(&[0, 2, 3, 5, 7, 8, 10])
            } else {
                bits(&[0, 2, 4, 5, 7, 9, 11])
            }
        }
        SCALE_MAJOR => bits(&[0, 2, 4, 5, 7, 9, 11]),
        SCALE_MINOR => bits(&[0, 2, 3, 5, 7, 8, 10]),
        3 => bits(&[0, 2, 3, 5, 7, 8, 11]),
        4 => bits(&[0, 2, 4, 7, 9]),
        5 => bits(&[0, 3, 5, 7, 10]),
        6 => bits(&[0, 3, 5, 6, 7, 10]),
        _ => ALL_NOTES,
    }
}

/// The allowed pitch classes (bit `k`: pitch class `k`, C = 0) for the Key and Scale
/// parameters, the project's key (`project_root` 0-11 and whether it is minor), and the
/// Custom notes (absolute pitch classes). Never empty: an empty custom set allows every note.
pub fn mask(key: i32, scale: i32, custom: u16, project_root: u8, project_minor: bool) -> u16 {
    if scale == SCALE_CUSTOM {
        let m = custom & ALL_NOTES;
        return if m == 0 { ALL_NOTES } else { m };
    }
    if scale == SCALE_CHROMATIC {
        return ALL_NOTES;
    }
    let root = if key <= 0 {
        u32::from(project_root % 12)
    } else {
        ((key - 1) % 12) as u32
    };
    let s = steps(scale, project_minor);
    // Rotate the steps up to the root.
    (((u32::from(s) << root) | (u32::from(s) >> (12 - root))) & 0x0fff) as u16
}

/// The allowed note (a MIDI note number) nearest to `pitch` (a MIDI note with a fraction).
/// With `current` the note corrected to until now: it is kept unless another allowed note is
/// nearer by more than `hysteresis` semitones.
pub fn snap(pitch: f32, mask: u16, current: Option<i32>, hysteresis: f32) -> i32 {
    let m = if mask & ALL_NOTES == 0 {
        ALL_NOTES
    } else {
        mask
    };
    let base = pitch.round() as i32;
    let mut best = base;
    let mut best_d = f32::INFINITY;
    // Scales leave gaps of at most four semitones (pentatonic, blues).
    for k in -6..=6 {
        let n = base + k;
        if m & (1 << n.rem_euclid(12)) == 0 {
            continue;
        }
        let d = (pitch - n as f32).abs();
        if d < best_d {
            best_d = d;
            best = n;
        }
    }
    if let Some(c) = current {
        let allowed = m & (1 << c.rem_euclid(12)) != 0;
        if allowed && c != best && (pitch - c as f32).abs() < best_d + hysteresis {
            return c;
        }
    }
    best
}

/// "C3", "F#1"... (MIDI 60 is C3, as Logic and the Piano Roll name it).
pub fn note_name(note: i32) -> String {
    format!(
        "{}{}",
        NAMES[note.rem_euclid(12) as usize],
        note.div_euclid(12) - 2
    )
}

/// The custom notes as text: "C D E F G A B", or "none".
pub fn mask_text(mask: u16) -> String {
    let names: Vec<&str> = (0..12)
        .filter(|k| mask & (1 << k) != 0)
        .map(|k| NAMES[k])
        .collect();
    if names.is_empty() {
        "none".into()
    } else {
        names.join(" ")
    }
}

/// A frequency as a MIDI note with a fraction, for a tuning of `a4` Hz.
#[inline]
pub fn hz_to_midi(hz: f32, a4: f32) -> f32 {
    69.0 + 12.0 * (hz / a4).log2()
}

/// A MIDI note with a fraction as a frequency, for a tuning of `a4` Hz.
#[inline]
pub fn midi_to_hz(midi: f32, a4: f32) -> f32 {
    a4 * ((midi - 69.0) / 12.0).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_rotate_to_their_key() {
        // C major: C D E F G A B.
        assert_eq!(mask(1, SCALE_MAJOR, 0, 0, false), 0b1010_1011_0101);
        // A minor has the same notes as C major.
        assert_eq!(
            mask(10, SCALE_MINOR, 0, 0, false),
            mask(1, SCALE_MAJOR, 0, 0, false)
        );
        // The project's key and mode: E minor = G major.
        assert_eq!(
            mask(0, SCALE_PROJECT, 0, 4, true),
            mask(8, SCALE_MAJOR, 0, 0, false)
        );
        assert_eq!(mask(3, SCALE_CHROMATIC, 0, 0, false), ALL_NOTES);
        assert_eq!(mask(0, SCALE_CUSTOM, 0b101, 0, false), 0b101);
        assert_eq!(mask(0, SCALE_CUSTOM, 0, 0, false), ALL_NOTES);
        assert_eq!(
            mask_text(mask(1, SCALE_MAJOR, 0, 0, false)),
            "C D E F G A B"
        );
    }

    #[test]
    fn snapping_picks_the_nearest_allowed_note() {
        let c_major = mask(1, SCALE_MAJOR, 0, 0, false);
        // A slightly flat E is E; C#4 (61) is not in C major: C or D, whichever is nearer.
        assert_eq!(snap(63.8, c_major, None, 0.0), 64);
        assert_eq!(snap(61.2, c_major, None, 0.0), 62);
        assert_eq!(snap(60.8, c_major, None, 0.0), 60);
        // Chromatic: every note.
        assert_eq!(snap(61.2, ALL_NOTES, None, 0.0), 61);
        // Hysteresis keeps the current note just past the half-way point...
        assert_eq!(snap(60.55, ALL_NOTES, Some(60), 0.15), 60);
        // ...but not far past it.
        assert_eq!(snap(60.8, ALL_NOTES, Some(60), 0.15), 61);
        // A note that is no longer allowed is left at once.
        assert_eq!(snap(61.1, c_major, Some(61), 0.15), 62);
        // Pentatonic gaps: A minor pentatonic (A C D E G), F#4 (66) goes to G (67) or E (64).
        let pent = mask(10, 5, 0, 0, false);
        assert_eq!(snap(66.0, pent, None, 0.0), 67);
        assert_eq!(snap(65.0, pent, None, 0.0), 64);
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(70), "Bb3");
        assert_eq!(note_name(0), "C-2");
        assert!((hz_to_midi(440.0, 440.0) - 69.0).abs() < 1e-6);
        assert!((midi_to_hz(81.0, 440.0) - 880.0).abs() < 1e-3);
    }
}
