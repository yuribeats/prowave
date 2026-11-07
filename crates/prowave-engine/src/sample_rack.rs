//! The Channel Rack instrument: one sample per channel, played whole on its note (channel 1
//! on C1, channel 2 on C#1, ...), as the rows of a step sequencer trigger them.
//!
//! Modelled on the Channel Rack of Lemon Studio (GPL-3.0-or-later,
//! https://github.com/ItsAlphaNeon/Lemon-Studio), itself a recreation of FL Studio's.
use crate::plugin_track::BlockProcessor;
use std::path::Path;
use std::sync::Arc;

/// The note of the first channel (C1); channel `k` plays `RACK_BASE_NOTE + k`.
pub const RACK_BASE_NOTE: u8 = 36;
/// Most channels a rack has.
pub const MAX_RACK_CHANNELS: usize = 64;
/// Samples sounding at once; the oldest is cut when another starts.
const VOICES: usize = 64;
/// Triggers one block can hold.
const MAX_PENDING: usize = 256;

/// One channel as the UI describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct RackChannelSpec {
    pub path: String,
    pub gain_db: f32,
    pub pan: f32,
    pub mute: bool,
}

struct Channel {
    /// Stereo, interleaved, at the engine's rate.
    sample: Option<Arc<Vec<f32>>>,
    gain_l: f32,
    gain_r: f32,
    mute: bool,
    gain_db: f32,
    pan: f32,
}

impl Channel {
    /// The left and right gains from the level and pan.
    fn aim(&mut self) {
        let g = 10f32.powf(self.gain_db.clamp(-96.0, 12.0) / 20.0);
        // Equal-power pan, unity in the centre.
        let a = (self.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        self.gain_l = g * a.cos() * std::f32::consts::SQRT_2;
        self.gain_r = g * a.sin() * std::f32::consts::SQRT_2;
    }
}

/// Automation parameters of the rack: channel `k`'s level (dB, -96 to 12) is
/// `k * RACK_PARAMS_PER_CHANNEL`, its pan (-1 to 1) the next one, its mute the one after.
pub const RACK_PARAMS_PER_CHANNEL: u32 = 3;

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    channel: usize,
    pos: usize,
    gain: f32,
    age: u64,
}

#[derive(Clone, Copy)]
struct Trigger {
    offset: u32,
    channel: usize,
    vel: u8,
}

/// The rack, ready for the audio thread.
pub struct SampleRack {
    channels: Vec<Channel>,
    voices: [Voice; VOICES],
    pending: Vec<Trigger>,
    started: u64,
}

/// A file read whole and brought to `rate` (linear interpolation), stereo interleaved.
pub(crate) fn load(path: &Path, rate: u32) -> Result<Vec<f32>, String> {
    let file = crate::disk::AudioFile::open(path).map_err(|e| e.to_string())?;
    let src = file.read_stereo().map_err(|e| e.to_string())?;
    if file.sample_rate == rate || file.sample_rate == 0 {
        return Ok(src);
    }
    let frames = src.len() / 2;
    let ratio = f64::from(file.sample_rate) / f64::from(rate.max(1));
    let out_frames = (frames as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let pos = i as f64 * ratio;
        let i0 = pos as usize;
        let frac = (pos - i0 as f64) as f32;
        let i1 = (i0 + 1).min(frames.saturating_sub(1));
        for c in 0..2 {
            let a = src[i0 * 2 + c];
            let b = src[i1 * 2 + c];
            out.push(a + (b - a) * frac);
        }
    }
    Ok(out)
}

impl SampleRack {
    /// Reads every channel's sample (UI thread: this reads files). Channels whose file
    /// cannot be read stay silent; their errors are returned.
    pub fn new(specs: &[RackChannelSpec], sample_rate: u32) -> (Self, Vec<String>) {
        let mut errors = Vec::new();
        let channels = specs
            .iter()
            .take(MAX_RACK_CHANNELS)
            .map(|c| {
                let sample = if c.path.is_empty() {
                    None
                } else {
                    match load(Path::new(&c.path), sample_rate) {
                        Ok(s) => Some(Arc::new(s)),
                        Err(e) => {
                            errors.push(format!("{}: {e}", c.path));
                            None
                        }
                    }
                };
                let mut ch = Channel {
                    sample,
                    gain_l: 1.0,
                    gain_r: 1.0,
                    mute: c.mute,
                    gain_db: c.gain_db,
                    pan: c.pan,
                };
                ch.aim();
                ch
            })
            .collect();
        (
            Self {
                channels,
                voices: [Voice::default(); VOICES],
                pending: Vec::with_capacity(MAX_PENDING),
                started: 0,
            },
            errors,
        )
    }

    fn start(&mut self, channel: usize, vel: u8) {
        let Some(c) = self.channels.get(channel) else {
            return;
        };
        if c.mute || c.sample.is_none() {
            return;
        }
        self.started += 1;
        let slot = self
            .voices
            .iter()
            .position(|v| !v.active)
            .unwrap_or_else(|| {
                // All busy: the oldest makes way.
                let mut oldest = 0;
                for (i, v) in self.voices.iter().enumerate() {
                    if v.age < self.voices[oldest].age {
                        oldest = i;
                    }
                }
                oldest
            });
        self.voices[slot] = Voice {
            active: true,
            channel,
            pos: 0,
            gain: f32::from(vel.min(127)) / 127.0,
            age: self.started,
        };
    }
}

impl BlockProcessor for SampleRack {
    fn set_parameter(&mut self, index: usize, value: f32) {
        let per = RACK_PARAMS_PER_CHANNEL as usize;
        let Some(c) = self.channels.get_mut(index / per) else {
            return;
        };
        if !value.is_finite() {
            return;
        }
        match index % per {
            0 => c.gain_db = value.clamp(-96.0, 12.0),
            1 => c.pan = value.clamp(-1.0, 1.0),
            _ => c.mute = value >= 0.5,
        }
        c.aim();
    }

    fn midi(&mut self, status: u8, data1: u8, data2: u8, offset: u32) {
        // Note On starts the channel's sample; it plays to its end (Note Off is ignored).
        if status & 0xf0 != 0x90 || data2 == 0 || data1 < RACK_BASE_NOTE {
            return;
        }
        let channel = usize::from(data1 - RACK_BASE_NOTE);
        if channel < self.channels.len() && self.pending.len() < self.pending.capacity() {
            self.pending.push(Trigger {
                offset,
                channel,
                vel: data2,
            });
        }
    }

    fn process(&mut self, _input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        let frames = out.0.len().min(out.1.len());
        out.0[..frames].fill(0.0);
        out.1[..frames].fill(0.0);
        self.pending.sort_unstable_by_key(|t| t.offset);
        let mut next = 0;
        for i in 0..frames {
            while next < self.pending.len() && self.pending[next].offset as usize <= i {
                let t = self.pending[next];
                self.start(t.channel, t.vel);
                next += 1;
            }
            let (mut l, mut r) = (0f32, 0f32);
            for v in self.voices.iter_mut().filter(|v| v.active) {
                let c = &self.channels[v.channel];
                let Some(s) = c.sample.as_ref() else {
                    v.active = false;
                    continue;
                };
                if v.pos * 2 + 1 >= s.len() {
                    v.active = false;
                    continue;
                }
                l += s[v.pos * 2] * c.gain_l * v.gain;
                r += s[v.pos * 2 + 1] * c.gain_r * v.gain;
                v.pos += 1;
            }
            out.0[i] = l;
            out.1[i] = r;
        }
        // Triggers past the block (offsets beyond its length) start at its end.
        while next < self.pending.len() {
            let t = self.pending[next];
            self.start(t.channel, t.vel);
            next += 1;
        }
        self.pending.clear();
    }

    fn latency_seconds(&self) -> f64 {
        0.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn rack_with(samples: &[&[f32]]) -> SampleRack {
        let (mut rack, errors) = SampleRack::new(&[], 48_000);
        assert!(errors.is_empty());
        for s in samples {
            rack.channels.push(Channel {
                sample: Some(Arc::new(s.to_vec())),
                gain_l: 1.0,
                gain_r: 1.0,
                mute: false,
                gain_db: 0.0,
                pan: 0.0,
            });
        }
        rack
    }

    #[test]
    fn a_step_plays_its_channel_from_its_frame() {
        // Channel 2's sample: two stereo frames.
        let mut rack = rack_with(&[&[0.1, 0.1], &[0.5, 0.5, 0.25, 0.25]]);
        rack.midi(0x90, RACK_BASE_NOTE + 1, 127, 3);
        let (mut l, mut r) = (vec![9.0f32; 8], vec![9.0f32; 8]);
        assert_no_alloc::assert_no_alloc(|| rack.process((&[], &[]), (&mut l, &mut r)));
        assert_eq!(&l[..6], &[0.0, 0.0, 0.0, 0.5, 0.25, 0.0]);
        assert_eq!(l, r);
    }

    #[test]
    fn automation_sets_a_channel_level_pan_and_mute() {
        let mut rack = rack_with(&[&[0.5, 0.5, 0.5, 0.5]]);
        rack.set_parameter(0, -6.0206);
        rack.midi(0x90, RACK_BASE_NOTE, 127, 0);
        let (mut l, mut r) = (vec![0f32; 2], vec![0f32; 2]);
        rack.process((&[], &[]), (&mut l, &mut r));
        assert!((l[0] - 0.25).abs() < 1e-3, "{}", l[0]);
        rack.set_parameter(1, 1.0);
        rack.midi(0x90, RACK_BASE_NOTE, 127, 0);
        rack.process((&[], &[]), (&mut l, &mut r));
        assert!(l[0].abs() < 1e-6 && r[0] > 0.3, "{l:?} {r:?}");
        rack.set_parameter(2, 1.0);
        rack.midi(0x90, RACK_BASE_NOTE, 127, 0);
        rack.process((&[], &[]), (&mut l, &mut r));
        assert_eq!(r[1], 0.0);
        rack.set_parameter(99, 1.0);
        rack.set_parameter(0, f32::NAN);
    }

    #[test]
    fn muted_channels_and_other_notes_are_silent() {
        let mut rack = rack_with(&[&[0.5, 0.5]]);
        rack.channels[0].mute = true;
        rack.midi(0x90, RACK_BASE_NOTE, 100, 0);
        rack.midi(0x90, RACK_BASE_NOTE + 5, 100, 0);
        rack.midi(0x90, 10, 100, 0);
        let (mut l, mut r) = (vec![0f32; 4], vec![0f32; 4]);
        rack.process((&[], &[]), (&mut l, &mut r));
        assert!(l.iter().chain(&r).all(|x| *x == 0.0));
    }
}
