//! Groove Live: keys played live on a track whose Groove Q follows a Channel Rack wait for a
//! step the rack's groove plays late, by the Groove Q strength. A key can not sound before it is
//! pressed, so a key before a step the groove plays early sounds at once: only delays are made.
//!
//! The app makes the table (every sixteenth of the song and where the groove plays it) off the
//! audio thread and sends it with [`crate::Command::SetTrackLiveGroove`]; the engine only looks
//! it up and keeps the keys waiting in a fixed queue ([`Held`]). Nothing here allocates on the
//! audio thread.

/// The longest a key waits, in quarter notes (a sixteenth): a groove never plays a step
/// further late than that, and anything longer is a jump of the playhead (a cycle pass, a locate).
pub const MAX_WAIT_BEATS: f64 = 0.25;

/// Keys (and key releases) that can wait at once, over all tracks.
pub const MAX_HELD: usize = 256;

/// Tracks that can have Groove Live at once.
pub const MAX_LIVE_GROOVES: usize = 64;

/// The groove of one track for keys played live.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveGroove {
    /// (a straight step, where the rack's hit on it plays), quarter notes, rising by step; NaN
    /// where there is no hit (a key there never waits).
    steps: Vec<(f64, f64)>,
    /// How much of the way to the groove a key goes, 0..=1.
    strength: f64,
    /// How long each pitch's last key waited (its release waits as long, so the note keeps
    /// its length).
    waited: [f64; 128],
}

impl LiveGroove {
    pub fn new(mut steps: Vec<(f64, f64)>, strength: f64) -> Self {
        steps.retain(|(a, b)| a.is_finite() && !b.is_infinite());
        steps.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self {
            steps,
            strength: if strength.is_finite() {
                strength.clamp(0.0, 1.0)
            } else {
                0.0
            },
            waited: [0.0; 128],
        }
    }

    pub fn strength(&self) -> f64 {
        self.strength
    }

    /// How many quarter notes a key pressed at `beat` waits: the pull toward where the groove
    /// plays the nearest step, when that is later; 0 when it is earlier or there is no step.
    pub fn wait_at(&self, beat: f64) -> f64 {
        let s = &self.steps;
        if s.is_empty() || !beat.is_finite() {
            return 0.0;
        }
        let i = s.partition_point(|x| x.0 <= beat);
        let near = match (i.checked_sub(1).map(|j| s[j]), s.get(i).copied()) {
            (Some(a), Some(b)) => {
                if beat - a.0 <= b.0 - beat {
                    a
                } else {
                    b
                }
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return 0.0,
        };
        // NaN (no hit on the step) compares false: no wait.
        let wait = self.strength * (near.1 - beat);
        if wait > 1e-6 {
            wait.min(MAX_WAIT_BEATS)
        } else {
            0.0
        }
    }

    pub(crate) fn set_waited(&mut self, pitch: u8, beats: f64) {
        self.waited[usize::from(pitch.min(127))] = beats;
    }

    pub(crate) fn waited(&self, pitch: u8) -> f64 {
        self.waited[usize::from(pitch.min(127))]
    }
}

/// A key press or release waiting for its beat.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeldKey {
    pub due: f64,
    pub track: u16,
    pub pitch: u8,
    pub vel: u8,
    pub on: bool,
}

/// The keys waiting, in a fixed array (no allocation).
#[derive(Clone, Debug)]
pub struct Held {
    keys: [HeldKey; MAX_HELD],
    len: usize,
}

impl Default for Held {
    fn default() -> Self {
        Self {
            keys: [HeldKey::default(); MAX_HELD],
            len: 0,
        }
    }
}

impl Held {
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Keeps `k` until its beat. False when the queue is full (the caller plays it at once).
    pub fn push(&mut self, k: HeldKey) -> bool {
        if self.len >= MAX_HELD {
            return false;
        }
        self.keys[self.len] = k;
        self.len += 1;
        true
    }

    /// Takes out, in the order they came, the keys due at `beat` (all of them with `all`, those
    /// of `track` only with `track`, and any waiting more than [`MAX_WAIT_BEATS`]: the playhead
    /// jumped) and gives each to `f`.
    pub fn release(
        &mut self,
        beat: f64,
        all: bool,
        track: Option<u16>,
        mut f: impl FnMut(HeldKey),
    ) {
        let mut kept = 0;
        for i in 0..self.len {
            let k = self.keys[i];
            let due = all
                || track == Some(k.track)
                || k.due <= beat
                || k.due - beat > MAX_WAIT_BEATS + 1e-9;
            if due {
                f(k);
            } else {
                self.keys[kept] = k;
                kept += 1;
            }
        }
        self.len = kept;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sixteenths with the "e" and "a" 0.05 late and beat 2 0.03 early (16th swing and an offset).
    fn groove(strength: f64) -> LiveGroove {
        let steps = (0..16)
            .map(|k| {
                let b = k as f64 * 0.25;
                let late = if k % 2 == 1 { 0.05 } else { 0.0 };
                let early = if k == 4 { -0.03 } else { 0.0 };
                (b, b + late + early)
            })
            .collect();
        LiveGroove::new(steps, strength)
    }

    #[test]
    fn a_key_waits_only_for_a_late_step_by_the_strength() {
        let g = groove(1.0);
        // On the "e": all the way to its late place.
        assert!((g.wait_at(0.25) - 0.05).abs() < 1e-9);
        // Just after it: what is left.
        assert!((g.wait_at(0.27) - 0.03).abs() < 1e-9);
        // Past the late place: no wait (never earlier).
        assert_eq!(g.wait_at(0.31), 0.0);
        // Before a step the groove plays early: at once.
        assert_eq!(g.wait_at(1.0), 0.0);
        assert_eq!(g.wait_at(0.99), 0.0);
        // Half the strength, half the wait.
        assert!((groove(0.5).wait_at(0.25) - 0.025).abs() < 1e-9);
        assert_eq!(groove(0.0).wait_at(0.25), 0.0);
        assert_eq!(LiveGroove::new(Vec::new(), 1.0).wait_at(0.25), 0.0);
        // A step with no hit: a key before it sounds at once.
        let none = LiveGroove::new(vec![(0.0, f64::NAN), (0.25, 0.30)], 1.0);
        assert_eq!(none.wait_at(-0.01), 0.0);
        assert!((none.wait_at(0.25) - 0.05).abs() < 1e-9);
    }

    #[test]
    fn held_keys_come_out_when_due_in_order_and_on_a_jump() {
        let mut h = Held::default();
        let key = |due: f64, on: bool| HeldKey {
            due,
            track: 1,
            pitch: 60,
            vel: 100,
            on,
        };
        assert!(h.push(key(1.05, true)));
        assert!(h.push(key(1.25, false)));
        let mut out = Vec::new();
        h.release(1.0, false, None, |k| out.push(k));
        assert!(out.is_empty());
        h.release(1.05, false, None, |k| out.push(k));
        assert_eq!(out, vec![key(1.05, true)]);
        // The playhead jumped back (a cycle pass): what waits goes out at once.
        h.release(0.0, false, None, |k| out.push(k));
        assert_eq!(out.len(), 2);
        assert!(h.is_empty());
        for _ in 0..MAX_HELD {
            assert!(h.push(key(2.0, true)));
        }
        assert!(!h.push(key(2.0, true)), "full: played at once instead");
        h.release(0.0, true, None, |_| {});
        assert!(h.is_empty());
    }
}
