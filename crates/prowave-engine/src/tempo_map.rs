//! Tempo map: tempo and time-signature changes (Logic Pro: the Tempo and Signature global
//! tracks).
//!
//! Positions are beats = quarter notes from the project start (0-based), like the rest of the
//! engine; as in Logic, the tempo is always quarter notes per minute whatever the signature.
//! Tempo changes can hold or ramp linearly in beat space until the next event. Segments are
//! computed when the map is built (on the UI thread), so every lookup on the audio thread is a
//! binary search and arithmetic only. Replace the engine's map with `Command::SetTempoMap`.

/// Logic Pro's tempo range.
pub const MIN_BPM: f32 = 5.0;
pub const MAX_BPM: f32 = 990.0;

/// Tempo change at `beat` (quarter notes from the start).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoEvent {
    pub beat: f64,
    pub bpm: f32,
    /// How tempo moves from this event to the next (Logic Pro: Tempo curve).
    pub curve: TempoCurve,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TempoCurve {
    #[default]
    Step,
    Linear,
}

/// SMPTE frame-rate conventions for Logic Pro's time display and synchronization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmpteRate {
    Fps24,
    Fps25,
    Fps29_97NonDrop,
    Fps29_97Drop,
    Fps30,
}

impl SmpteRate {
    /// Every rate, in the order a menu lists them.
    pub const ALL: [SmpteRate; 5] = [
        Self::Fps24,
        Self::Fps25,
        Self::Fps29_97NonDrop,
        Self::Fps29_97Drop,
        Self::Fps30,
    ];

    /// Real frames per second.
    pub fn frames_per_second(self) -> f64 {
        match self {
            Self::Fps24 => 24.0,
            Self::Fps25 => 25.0,
            Self::Fps29_97Drop | Self::Fps29_97NonDrop => 30_000.0 / 1_001.0,
            Self::Fps30 => 30.0,
        }
    }

    /// Frames per second as counted in the labels (30 for both 29.97 rates).
    pub fn nominal_fps(self) -> u64 {
        match self {
            Self::Fps24 => 24,
            Self::Fps25 => 25,
            Self::Fps29_97Drop | Self::Fps29_97NonDrop | Self::Fps30 => 30,
        }
    }

    /// Whether the labels skip `;00` and `;01` at the start of each minute but every tenth.
    pub fn is_drop_frame(self) -> bool {
        self == Self::Fps29_97Drop
    }

    /// The frame number of the label `t` (ignoring `subframe`): how many real frames after
    /// `00:00:00:00` it is. The inverse of [`SmpteRate::smpte_of_frames`] for whole frames.
    pub fn frames_of(self, t: Smpte) -> i64 {
        let nominal = self.nominal_fps() as i64;
        let seconds = t.h as i64 * 3600 + t.m as i64 * 60 + t.s as i64;
        let labels = seconds * nominal + t.f as i64;
        if self.is_drop_frame() {
            let minutes = t.h as i64 * 60 + t.m as i64;
            labels - 2 * (minutes - minutes / 10)
        } else {
            labels
        }
    }

    /// The label of the frame `frames` real frames after `00:00:00:00` (fractions of a frame
    /// are the `subframe`). Wraps at 24 hours.
    pub fn smpte_of_frames(self, frames: f64) -> Smpte {
        let nominal = self.nominal_fps();
        if !frames.is_finite() {
            return Smpte::default();
        }
        // A position that is exactly on a frame (a timecode converted to seconds and back)
        // can land a hair below it: `frame / fps * fps` is 2.9999999999999996, not 3, and
        // flooring that would label it one frame early. A millionth of a frame is 40 ns.
        let frames = if frames.abs() < 1e12 {
            (frames * 1e6).round() / 1e6
        } else {
            frames
        };

        // A 24-hour drop-frame day contains 2,589,408 actual frames. Other rates
        // wrap at their nominal 24-hour label count.
        let day_frames = if self.is_drop_frame() {
            2_589_408_i64
        } else {
            (nominal * 24 * 60 * 60) as i64
        };
        let mut frame_index = frames.floor() as i64 % day_frames;
        if frame_index < 0 {
            frame_index += day_frames;
        }
        let subframe = ((frames - frames.floor()) * 100.0).floor() as u8;

        let label_index = if self.is_drop_frame() {
            let frames_per_ten_minutes = 17_982_i64;
            let frames_per_minute = 1_798_i64;
            let ten_minute_blocks = frame_index / frames_per_ten_minutes;
            let remainder = frame_index % frames_per_ten_minutes;
            let dropped = 18 * ten_minute_blocks
                + if remainder >= 2 {
                    2 * ((remainder - 2) / frames_per_minute)
                } else {
                    0
                };
            frame_index + dropped
        } else {
            frame_index
        } as u64;
        let frames_per_day_label = nominal * 24 * 60 * 60;
        let label_index = label_index % frames_per_day_label;
        let f = label_index % nominal;
        let total_seconds = label_index / nominal;
        let s = total_seconds % 60;
        let total_minutes = total_seconds / 60;
        Smpte {
            h: (total_minutes / 60) as u8,
            m: (total_minutes % 60) as u8,
            s: s as u8,
            f: f as u8,
            subframe,
        }
    }
}

impl std::fmt::Display for Smpte {
    /// `hh:mm:ss:ff`, without the subframe.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}:{:02}:{:02}", self.h, self.m, self.s, self.f)
    }
}

/// SMPTE timecode. `subframe` is hundredths of a frame (0..99).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Smpte {
    pub h: u8,
    pub m: u8,
    pub s: u8,
    pub f: u8,
    pub subframe: u8,
}

/// Time signature `num/den` from the start of `bar` (0-based) on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignatureEvent {
    pub bar: u32,
    pub num: u8,
    pub den: u8,
}

/// A position in bars and beats (all 0-based; `beat` counts `den` notes, e.g. eighths in 6/8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicalPosition {
    pub bar: u32,
    pub beat: u32,
    /// How far into `beat` (0..1).
    pub fraction: f64,
    pub num: u8,
    pub den: u8,
}

#[derive(Clone, Copy, Debug)]
struct TempoSegment {
    beat: f64,
    seconds: f64,
    bpm: f64,
    slope: f64,
}

impl TempoSegment {
    fn bpm_at(&self, beat: f64) -> f64 {
        self.bpm + self.slope * (beat - self.beat)
    }

    fn seconds_after(&self, beats: f64) -> f64 {
        if self.slope == 0.0 {
            beats * 60.0 / self.bpm
        } else {
            (60.0 / self.slope) * (self.slope * beats / self.bpm).ln_1p()
        }
    }

    fn beats_after(&self, seconds: f64) -> f64 {
        if self.slope == 0.0 {
            seconds * self.bpm / 60.0
        } else {
            (self.bpm / self.slope) * (seconds * self.slope / 60.0).exp_m1()
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SigSegment {
    bar: u32,
    beat: f64,
    num: u8,
    den: u8,
}

impl SigSegment {
    /// Quarter notes per signature beat (a `den` note).
    fn unit(&self) -> f64 {
        4.0 / self.den as f64
    }
    fn bar_len(&self) -> f64 {
        self.num as f64 * self.unit()
    }
}

#[derive(Clone, Debug)]
pub struct TempoMap {
    /// Sorted by beat; the first starts at beat 0.
    tempos: Vec<TempoSegment>,
    /// Sorted by bar; the first starts at bar 0.
    sigs: Vec<SigSegment>,
}

impl Default for TempoMap {
    fn default() -> Self {
        Self::constant(120.0, (4, 4))
    }
}

impl TempoMap {
    /// One tempo and one signature for the whole project.
    pub fn constant(bpm: f32, sig: (u8, u8)) -> Self {
        Self::new(
            vec![TempoEvent {
                beat: 0.0,
                bpm,
                curve: TempoCurve::Step,
            }],
            vec![SignatureEvent {
                bar: 0,
                num: sig.0,
                den: sig.1,
            }],
        )
    }

    /// Builds a map from events in any order. The first tempo and signature apply from the
    /// start (defaults 120 BPM, 4/4); of several events at the same place the last one wins.
    /// Tempos are clamped to Logic's 5-990 BPM; an invalid denominator becomes 4.
    pub fn new(mut tempos: Vec<TempoEvent>, mut sigs: Vec<SignatureEvent>) -> Self {
        tempos.retain(|e| e.beat.is_finite() && e.bpm.is_finite());
        for e in tempos.iter_mut() {
            e.beat = e.beat.max(0.0);
        }
        tempos.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        tempos.dedup_by(|later, kept| {
            let same = later.beat == kept.beat;
            if same {
                kept.bpm = later.bpm;
                kept.curve = later.curve;
            }
            same
        });
        if tempos.is_empty() {
            tempos.push(TempoEvent {
                beat: 0.0,
                bpm: 120.0,
                curve: TempoCurve::Step,
            });
        }
        let mut segs: Vec<TempoSegment> = Vec::with_capacity(tempos.len());
        for (i, e) in tempos.iter().enumerate() {
            // The first event holds from the project start, even if its source beat is later.
            let beat = if i == 0 { 0.0 } else { e.beat };
            let bpm = e.bpm.clamp(MIN_BPM, MAX_BPM) as f64;
            let seconds = segs.last().map_or(0.0, |prev: &TempoSegment| {
                prev.seconds + prev.seconds_after(beat - prev.beat)
            });
            let slope = tempos.get(i + 1).map_or(0.0, |next| {
                if e.curve == TempoCurve::Linear {
                    let end_beat = next.beat;
                    let end_bpm = next.bpm.clamp(MIN_BPM, MAX_BPM) as f64;
                    let slope = (end_bpm - bpm) / (end_beat - beat);
                    if slope.is_finite() {
                        slope
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            });
            segs.push(TempoSegment {
                beat,
                seconds,
                bpm,
                slope,
            });
        }

        sigs.sort_by_key(|s| s.bar);
        sigs.dedup_by(|later, kept| {
            let same = later.bar == kept.bar;
            if same {
                *kept = *later;
            }
            same
        });
        let mut sig_segs: Vec<SigSegment> = Vec::with_capacity(sigs.len().max(1));
        for (i, s) in sigs.iter().enumerate() {
            let bar = if i == 0 { 0 } else { s.bar };
            let den = if matches!(s.den, 1 | 2 | 4 | 8 | 16 | 32 | 64) {
                s.den
            } else {
                4
            };
            let beat = match sig_segs.last() {
                Some(p) => p.beat + (bar - p.bar) as f64 * p.bar_len(),
                None => 0.0,
            };
            sig_segs.push(SigSegment {
                bar,
                beat,
                num: s.num.clamp(1, 99),
                den,
            });
        }
        if sig_segs.is_empty() {
            sig_segs.push(SigSegment {
                bar: 0,
                beat: 0.0,
                num: 4,
                den: 4,
            });
        }
        Self {
            tempos: segs,
            sigs: sig_segs,
        }
    }

    fn tempo_seg(&self, beat: f64) -> &TempoSegment {
        let i = self.tempos.partition_point(|s| s.beat <= beat);
        &self.tempos[i.saturating_sub(1)]
    }

    fn sig_seg(&self, beat: f64) -> &SigSegment {
        let i = self.sigs.partition_point(|s| s.beat <= beat);
        &self.sigs[i.saturating_sub(1)]
    }

    /// Tempo in effect at `beat`.
    pub fn tempo_at(&self, beat: f64) -> f32 {
        if beat.is_nan() {
            return self.tempos[0].bpm as f32;
        }
        if beat < 0.0 {
            return self.tempos[0].bpm as f32;
        }
        let s = self.tempo_seg(beat);
        s.bpm_at(beat).clamp(MIN_BPM as f64, MAX_BPM as f64) as f32
    }

    /// Time signature in effect at `beat`.
    pub fn signature_at(&self, beat: f64) -> (u8, u8) {
        let s = self.sig_seg(beat);
        (s.num, s.den)
    }

    /// Seconds from the project start to `beat`.
    pub fn seconds_at(&self, beat: f64) -> f64 {
        if beat.is_nan() {
            return f64::NAN;
        }
        if beat < 0.0 {
            return beat * 60.0 / self.tempos[0].bpm;
        }
        let s = self.tempo_seg(beat);
        s.seconds + s.seconds_after(beat - s.beat)
    }

    /// Beat reached `seconds` after the project start (inverse of [`TempoMap::seconds_at`]).
    pub fn beat_at(&self, seconds: f64) -> f64 {
        if seconds.is_nan() {
            return f64::NAN;
        }
        if seconds < 0.0 {
            return seconds * self.tempos[0].bpm / 60.0;
        }
        let i = self.tempos.partition_point(|s| s.seconds <= seconds);
        let s = &self.tempos[i.saturating_sub(1)];
        s.beat + s.beats_after(seconds - s.seconds)
    }

    /// The beat `seconds` after `beat`, following every tempo change in between
    /// (the transport clock: called once per sample with `1 / sample_rate`).
    pub fn advance(&self, beat: f64, seconds: f64) -> f64 {
        if beat.is_nan() || seconds.is_nan() {
            return f64::NAN;
        }
        if seconds < 0.0 {
            return self.beat_at(self.seconds_at(beat) + seconds);
        }
        if seconds == 0.0 {
            return beat;
        }

        let mut position = beat;
        let mut remaining = seconds;
        if position < 0.0 {
            let to_zero = -position * 60.0 / self.tempos[0].bpm;
            if remaining < to_zero {
                return position + remaining * self.tempos[0].bpm / 60.0;
            }
            remaining -= to_zero;
            position = 0.0;
        }
        let mut index = self
            .tempos
            .partition_point(|segment| segment.beat <= position)
            .saturating_sub(1);
        loop {
            let segment = &self.tempos[index];
            // Rebase the ramp at the current position. `seconds_after` and `beats_after`
            // use the segment's starting BPM, while this advance may begin mid-ramp.
            let local = TempoSegment {
                beat: position,
                seconds: 0.0,
                bpm: segment.bpm_at(position),
                slope: segment.slope,
            };
            if let Some(next) = self.tempos.get(index + 1) {
                let distance = next.beat - position;
                let advance = local.beats_after(remaining);
                if advance < distance {
                    return position + advance;
                }
                // Integrate only when this advance actually crosses a boundary.
                // For a ramp, seconds_after() takes a logarithm; the usual per-sample
                // path above needs only the inverse calculation.
                let to_next = local.seconds_after(distance);
                remaining -= to_next;
                position = next.beat;
                index += 1;
            } else {
                return position + local.beats_after(remaining);
            }
        }
    }

    /// SMPTE timecode at `beat`, with the project starting at `00:00:00:00`.
    pub fn smpte_at(&self, beat: f64, rate: SmpteRate) -> Smpte {
        self.smpte_at_with_offset(beat, rate, 0)
    }

    /// SMPTE timecode at `beat`, adding a signed project start offset in frames.
    /// Timecode wraps at 24 hours. Drop-frame mode skips labels `;00` and `;01`
    /// at the start of each minute except every tenth minute.
    pub fn smpte_at_with_offset(&self, beat: f64, rate: SmpteRate, offset_frames: i64) -> Smpte {
        rate.smpte_of_frames(self.smpte_frames_at(beat, rate, offset_frames))
    }

    /// Real frames (with a fraction) from `00:00:00:00` to `beat`, when the project starts
    /// `offset_frames` frames after it.
    pub fn smpte_frames_at(&self, beat: f64, rate: SmpteRate, offset_frames: i64) -> f64 {
        self.seconds_at(beat) * rate.frames_per_second() + offset_frames as f64
    }

    /// The beat at timecode `t` (the inverse of [`TempoMap::smpte_at_with_offset`]); before
    /// the project start it is negative.
    pub fn beat_at_smpte(&self, t: Smpte, rate: SmpteRate, offset_frames: i64) -> f64 {
        let frames = rate.frames_of(t) as f64 + t.subframe as f64 / 100.0 - offset_frames as f64;
        self.beat_at(frames / rate.frames_per_second())
    }

    /// Beat at which `bar` (0-based) starts.
    pub fn beat_of_bar(&self, bar: u32) -> f64 {
        let i = self.sigs.partition_point(|s| s.bar <= bar);
        let s = &self.sigs[i.saturating_sub(1)];
        s.beat + (bar - s.bar) as f64 * s.bar_len()
    }

    /// Bars and beats of `beat` (negative positions count as the start).
    pub fn position(&self, beat: f64) -> MusicalPosition {
        let beat = beat.max(0.0);
        let s = self.sig_seg(beat);
        let into = beat - s.beat;
        let bars = (into / s.bar_len()).floor();
        let in_bar = (into - bars * s.bar_len()) / s.unit();
        let b = in_bar.floor();
        MusicalPosition {
            bar: s.bar + bars as u32,
            beat: (b as u32).min(s.num as u32 - 1),
            fraction: in_bar - b,
            num: s.num,
            den: s.den,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn tempo(beat: f64, bpm: f32) -> TempoEvent {
        TempoEvent {
            beat,
            bpm,
            curve: TempoCurve::Step,
        }
    }

    fn sig(bar: u32, num: u8, den: u8) -> SignatureEvent {
        SignatureEvent { bar, num, den }
    }

    #[test]
    fn constant_tempo_converts_both_ways() {
        let m = TempoMap::constant(120.0, (4, 4));
        assert!(close(m.seconds_at(4.0), 2.0));
        assert!(close(m.beat_at(2.0), 4.0));
        assert_eq!(m.tempo_at(1000.0), 120.0);
    }

    #[test]
    fn tempo_change_splits_time_at_the_event() {
        // 120 BPM for 4 beats (2 s), then 60 BPM.
        let m = TempoMap::new(vec![tempo(4.0, 60.0), tempo(0.0, 120.0)], vec![]);
        assert_eq!(m.tempo_at(3.999), 120.0);
        assert_eq!(m.tempo_at(4.0), 60.0);
        assert!(close(m.seconds_at(8.0), 6.0));
        assert!(close(m.beat_at(6.0), 8.0));
        // 0.5 s reaches beat 4, the other 0.5 s at 60 BPM adds half a beat.
        assert!(close(m.advance(3.0, 1.0), 4.5));
    }

    #[test]
    fn linear_tempo_ramp_integrates_and_inverts_exactly() {
        let map = TempoMap::new(
            vec![
                TempoEvent {
                    beat: 0.0,
                    bpm: 120.0,
                    curve: TempoCurve::Linear,
                },
                tempo(4.0, 60.0),
            ],
            vec![],
        );
        assert_eq!(map.tempo_at(0.0), 120.0);
        assert_eq!(map.tempo_at(2.0), 90.0);
        assert_eq!(map.tempo_at(4.0), 60.0);
        let expected_seconds = 4.0 * 2.0f64.ln();
        assert!(close(map.seconds_at(4.0), expected_seconds));
        for beat in [0.0, 0.25, 1.0, 2.0, 3.75, 4.0, 8.0] {
            assert!(close(map.beat_at(map.seconds_at(beat)), beat));
        }
        assert!(close(map.advance(0.0, expected_seconds), 4.0));
    }

    #[test]
    fn direct_advance_matches_time_conversion_across_segments() {
        let map = TempoMap::new(
            vec![
                TempoEvent {
                    beat: 0.0,
                    bpm: 110.0,
                    curve: TempoCurve::Linear,
                },
                tempo(3.0, 170.0),
                TempoEvent {
                    beat: 7.0,
                    bpm: 80.0,
                    curve: TempoCurve::Linear,
                },
                tempo(12.0, 50.0),
            ],
            vec![],
        );
        for (beat, seconds) in [
            (0.1, 0.03),
            (2.99, 0.1),
            (4.0, 2.0),
            (6.99, 0.1),
            (10.0, 4.0),
            (-1.0, 1.0),
        ] {
            let expected = map.beat_at(map.seconds_at(beat) + seconds);
            assert!(close(map.advance(beat, seconds), expected));
        }
        // Samples immediately before, on, and after each boundary must preserve
        // the same timing for both rising and falling ramps.
        for start in [-1.0, 0.0, 0.1, 3.0, 6.99, 7.0, 10.0] {
            for boundary in [3.0, 7.0, 12.0] {
                let until_boundary = map.seconds_at(boundary) - map.seconds_at(start);
                for delta in [-1e-10, 0.0, 1e-10] {
                    let seconds = until_boundary + delta;
                    let expected = map.beat_at(map.seconds_at(start) + seconds);
                    assert!(close(map.advance(start, seconds), expected));
                }
            }
        }
    }

    #[test]
    fn smpte_rates_offsets_and_drop_frame_labels() {
        let map = TempoMap::constant(120.0, (4, 4));
        let beat_at_frames = |frames: f64, fps: f64| frames / fps * 2.0;

        let drop_minute = map.smpte_at(
            beat_at_frames(1_800.0, 30_000.0 / 1_001.0),
            SmpteRate::Fps29_97Drop,
        );
        assert_eq!(
            drop_minute,
            Smpte {
                h: 0,
                m: 1,
                s: 0,
                f: 2,
                subframe: 0
            }
        );

        let tenth_minute = map.smpte_at(
            beat_at_frames(17_982.0, 30_000.0 / 1_001.0),
            SmpteRate::Fps29_97Drop,
        );
        assert_eq!(
            tenth_minute,
            Smpte {
                h: 0,
                m: 10,
                s: 0,
                f: 0,
                subframe: 0
            }
        );

        let offset = map.smpte_at_with_offset(0.0, SmpteRate::Fps25, 25 * 3_661);
        assert_eq!(
            offset,
            Smpte {
                h: 1,
                m: 1,
                s: 1,
                f: 0,
                subframe: 0
            }
        );

        let subframe = map.smpte_at(0.001, SmpteRate::Fps24);
        assert_eq!(
            subframe,
            Smpte {
                h: 0,
                m: 0,
                s: 0,
                f: 0,
                subframe: 1
            }
        );
    }

    #[test]
    fn advancing_sample_by_sample_lands_on_the_exact_beat() {
        let m = TempoMap::new(vec![tempo(0.0, 120.0), tempo(2.0, 90.0)], vec![]);
        let mut beat = 0.0;
        for _ in 0..48_000 * 2 {
            beat = m.advance(beat, 1.0 / 48_000.0);
        }
        // 1 s at 120 (2 beats) + 1 s at 90 (1.5 beats).
        assert!((beat - 3.5).abs() < 1e-6, "{beat}");
    }

    #[test]
    fn messy_input_is_normalised() {
        let m = TempoMap::new(
            vec![tempo(8.0, 100.0), tempo(2.0, 3000.0), tempo(8.0, 80.0)],
            vec![sig(0, 3, 5)],
        );
        // The first event holds from the start; out-of-range tempo is clamped.
        assert_eq!(m.tempo_at(0.0), MAX_BPM);
        // Last of two events at the same beat wins.
        assert_eq!(m.tempo_at(8.0), 80.0);
        assert_eq!(m.signature_at(0.0), (3, 4));

        let empty = TempoMap::new(vec![], vec![]);
        assert_eq!(
            (empty.tempo_at(0.0), empty.signature_at(0.0)),
            (120.0, (4, 4))
        );
    }

    #[test]
    fn last_duplicate_tempo_event_keeps_its_curve() {
        let map = TempoMap::new(
            vec![
                tempo(0.0, 120.0),
                TempoEvent {
                    beat: 0.0,
                    bpm: 120.0,
                    curve: TempoCurve::Linear,
                },
                tempo(4.0, 60.0),
            ],
            vec![],
        );
        assert!(close(map.seconds_at(4.0), 4.0 * 2.0f64.ln()));
    }

    #[test]
    fn signature_changes_move_bar_lines() {
        // Bars 0-1 in 4/4 (8 beats), then 6/8 (3 quarter notes per bar).
        let m = TempoMap::new(vec![], vec![sig(0, 4, 4), sig(2, 6, 8)]);
        assert!(close(m.beat_of_bar(2), 8.0));
        assert!(close(m.beat_of_bar(3), 11.0));
        assert_eq!(m.signature_at(7.9), (4, 4));
        assert_eq!(m.signature_at(8.0), (6, 8));

        let p = m.position(11.75);
        assert_eq!((p.bar, p.beat, p.num, p.den), (3, 1, 6, 8));
        assert!(close(p.fraction, 0.5));

        let p = m.position(5.0);
        assert_eq!((p.bar, p.beat), (1, 1));
        assert_eq!(m.position(-3.0).bar, 0);
    }

    #[test]
    fn labels_and_frame_numbers_are_inverses_for_every_rate() {
        for rate in SmpteRate::ALL {
            // Every frame of the first hour and a bit, which holds six drop-frame minutes.
            for n in (0..140_000i64).chain(2_000_000..2_001_000) {
                let t = rate.smpte_of_frames(n as f64);
                assert_eq!(rate.frames_of(t), n, "{rate:?} frame {n} is {t}");
            }
        }
        let t = SmpteRate::Fps29_97Drop.smpte_of_frames(1_800.0);
        assert_eq!(t.to_string(), "00:01:00:02");
        let t = SmpteRate::Fps25.smpte_of_frames(90_000.0);
        assert_eq!(t.to_string(), "01:00:00:00");
    }

    #[test]
    fn a_timecode_finds_its_beat_through_tempo_changes_and_the_start_offset() {
        let map = TempoMap::new(
            vec![
                TempoEvent {
                    beat: 0.0,
                    bpm: 100.0,
                    curve: TempoCurve::Linear,
                },
                tempo(16.0, 140.0),
                tempo(40.0, 90.0),
            ],
            vec![],
        );
        // The project starts at 01:00:00:00 (25 fps).
        let offset = 25 * 3600;
        for beat in [0.0, 0.5, 7.3, 16.0, 25.0, 40.0, 100.0] {
            let t = map.smpte_at_with_offset(beat, SmpteRate::Fps25, offset);
            let back = map.beat_at_smpte(t, SmpteRate::Fps25, offset);
            // The label is whole frames and hundredths of a frame: a frame at 25 fps is 0.04 s,
            // under 0.1 beat at these tempos.
            assert!((back - beat).abs() < 0.1, "{beat}: {t} gives {back}");
        }
        let at_start = map.smpte_at_with_offset(0.0, SmpteRate::Fps25, offset);
        assert_eq!(at_start.to_string(), "01:00:00:00");
        assert!(map.beat_at_smpte(at_start, SmpteRate::Fps25, offset).abs() < 1e-9);
        // Before the start the beat is negative (a pre-roll).
        let before = Smpte {
            h: 0,
            m: 59,
            s: 59,
            f: 0,
            subframe: 0,
        };
        assert!(map.beat_at_smpte(before, SmpteRate::Fps25, offset) < 0.0);
    }

    #[test]
    fn frame_exact_positions_get_their_own_frame_label() {
        // 60 BPM: one beat per second, so the beat is the time in seconds.
        let map = TempoMap::constant(60.0, (4, 4));
        for (rate, fps, nominal) in [
            (SmpteRate::Fps24, 24.0, 24u64),
            (SmpteRate::Fps25, 25.0, 25),
            (SmpteRate::Fps30, 30.0, 30),
            (SmpteRate::Fps29_97NonDrop, 30_000.0 / 1_001.0, 30),
        ] {
            for frame in 0..200_000u64 {
                let t = map.smpte_at(frame as f64 / fps, rate);
                let n = frame;
                let want = (
                    (n / (nominal * 3600)) % 24,
                    (n / (nominal * 60)) % 60,
                    (n / nominal) % 60,
                    n % nominal,
                );
                let got = (t.h as u64, t.m as u64, t.s as u64, t.f as u64);
                assert_eq!(got, want, "{rate:?} frame {frame}");
                assert_eq!(t.subframe, 0, "{rate:?} frame {frame}");
            }
        }
        // Drop frame: the label skips ;00 and ;01 each minute except every tenth.
        let fps = 30_000.0 / 1_001.0;
        let at = |frame: u64| map.smpte_at(frame as f64 / fps, SmpteRate::Fps29_97Drop);
        let label = |t: Smpte| (t.h, t.m, t.s, t.f);
        assert_eq!(label(at(1799)), (0, 0, 59, 29));
        assert_eq!(label(at(1800)), (0, 1, 0, 2));
        assert_eq!(label(at(17_981)), (0, 9, 59, 29));
        assert_eq!(label(at(17_982)), (0, 10, 0, 0));
        // And just before a frame is still the previous one.
        let before = map.smpte_at(100.0 / fps - 1e-4, SmpteRate::Fps29_97Drop);
        assert_eq!(label(before), (0, 0, 3, 9));
    }
}
