//! Device-independent engine core. `EngineCore::process` is the audio-thread entry point:
//! no allocation, no locks, no syscalls. Unit-testable without an audio device.
use crate::clock::song_position;
use crate::disk::{Block, ClipShared};
use crate::fx::{Chain, Pdc, MAX_FX_PARAMS};
use crate::gain_smoothing::GainRamp;
use crate::monitor::{InputFeed, MonitorTrack, MAX_MONITORS};
use crate::mtc::{MtcGenerator, MtcMessage};
use crate::plugin_track::PluginTrack;
#[cfg(test)]
use crate::routing::AutoPoint;
#[cfg(test)]
use crate::routing::SendSpec;
use crate::routing::{send_gain, AutoLane, AutoTarget, Route, MAX_BUSES, MAX_SENDS};
use crate::soundfont::SfPlayer;
use crate::synth::{kit_voice, osc, Kit, Svf, SynthPatch};
use crate::tempo_map::{SmpteRate, TempoMap};
use crate::triple::{triple_buffer, Reader, Writer};
use rtrb::{Consumer, Producer};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Tracks the engine mixes (Logic: 1000 tracks per project).
pub const MAX_TRACKS: usize = 1024;
/// The `track` number that addresses the master bus (the Stereo Out strip) in
/// [`Command::SetFx`] and [`Command::SetConvolver`].
pub const MASTER_TRACK: u16 = u16::MAX;
const MAX_VOICES: usize = 48;
/// Samples between two readings of the automation (0.7 ms at 48 kHz). Faders, pans, sends
/// and mutes ramp in a straight line from one reading to the next, so a lane is followed
/// without zipper noise and reaches each reading's value on its sample; plug-in parameters
/// are set at the frame of each reading.
pub const AUTO_BLOCK: u32 = 32;
/// Most lanes [`Command::SetAutomation`] plays; more are ignored.
pub const MAX_AUTO_LANES: usize = 8192;

/// A note scheduled on the timeline (beats are 0-based quarter notes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteEvent {
    pub start_beat: f64,
    pub end_beat: f64,
    pub pitch: u8,
    pub vel: u8,
    pub track: u16,
}

/// A MIDI note the engine sends out for a track that plays an external instrument (Logic:
/// External MIDI track). The app turns it into a MIDI message on the track's port and channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MidiOutEvent {
    pub track: u16,
    /// When the message is to be heard, in host-clock nanoseconds (the audio clock the frame
    /// it belongs to plays on). 0: as soon as possible (the device gave no time).
    pub at_ns: u64,
    /// A system real-time or common message instead of a track's note: 0xF8 clock, 0xFA
    /// start, 0xFB continue, 0xFC stop, 0xF2 song position (in `spp`). 0: not one.
    pub system: u8,
    /// Song Position Pointer, in sixteenth notes (with `system` 0xF2).
    pub spp: u16,
    /// Note On (`true`) or Note Off. Not used by a controller.
    pub on: bool,
    /// The note, or for a controller its number.
    pub pitch: u8,
    /// The velocity, or for a controller its value.
    pub vel: u8,
    /// A controller change (such as the sustain pedal) instead of a note.
    pub controller: bool,
    /// For a controller: its value in full (0-16383 for pitch bend, see `Control`); 0 for notes.
    pub value: u16,
}

/// A controller change on a track's timeline (beats are 0-based quarter notes): `number` is a
/// controller 0-127, or 128 pitch bend, 129 channel pressure, 130 one byte of a System
/// Exclusive message. External instruments get them as [`MidiOutEvent`]s, plug-in
/// instruments as MIDI at their frame, and the engine's own synth and SoundFont players
/// follow them (see [`MidiCtl`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlEvent {
    pub beat: f64,
    pub track: u16,
    pub number: u8,
    /// 0-127 for a controller, 0-16383 for pitch bend.
    pub value: u16,
}

/// How the engine's own instruments on a track stand with the region's controllers (Logic:
/// MIDI Draw), changed at the sample a [`ControlEvent`] falls on:
/// pitch bend (±2 semitones), Modulation (CC 1) and channel pressure (vibrato, up to 50 cents
/// at the patch's rate or 5.5 Hz), Volume (CC 7) and Expression (CC 11) as gain on the
/// GM curve (40·log10(v/127) dB each), Pan (CC 10, equal power, unity in the middle) and the
/// Sustain pedal (CC 64: notes that end while it is down sound until it goes up). With no
/// events a track is as before: no bend, no vibrato, unity gain, centred.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiCtl {
    /// Semitones.
    pub bend: f32,
    /// 0..1 each.
    pub modulation: f32,
    pub pressure: f32,
    /// Linear gains of Volume and Expression.
    pub volume: f32,
    pub expression: f32,
    /// Pan gains (both 1 in the middle).
    pub pan_l: f32,
    pub pan_r: f32,
    pub sustain: bool,
}

impl Default for MidiCtl {
    fn default() -> Self {
        Self {
            bend: 0.0,
            modulation: 0.0,
            pressure: 0.0,
            volume: 1.0,
            expression: 1.0,
            pan_l: 1.0,
            pan_r: 1.0,
            sustain: false,
        }
    }
}

/// Logic's default pitch bend range, in semitones each way.
pub const BEND_RANGE: f32 = 2.0;

impl MidiCtl {
    /// The state after event `number` = `value`. Returns whether the pitch changed (voices
    /// must recompute their frequency at once).
    pub fn apply(&mut self, number: u8, value: u16) -> bool {
        let cc = f32::from(value.min(127)) / 127.0;
        match number {
            128 => {
                let bend = (f32::from(value.min(16383)) - 8192.0) / 8192.0 * BEND_RANGE;
                let moved = bend != self.bend;
                self.bend = bend;
                return moved;
            }
            129 => self.pressure = cc,
            1 => self.modulation = cc,
            7 => self.volume = cc * cc,
            11 => self.expression = cc * cc,
            10 => {
                let a = cc * std::f32::consts::FRAC_PI_2;
                self.pan_l = a.cos() * std::f32::consts::SQRT_2;
                self.pan_r = a.sin() * std::f32::consts::SQRT_2;
            }
            64 => self.sustain = value >= 64,
            _ => {}
        }
        false
    }

    /// Extra vibrato depth from the wheel and pressure, in cents.
    pub fn vibrato_cents(&self) -> f32 {
        self.modulation.max(self.pressure) * 50.0
    }
}

/// A note sent to an external instrument that has not been released yet.
#[derive(Clone, Copy)]
struct ExtNote {
    track: u16,
    pitch: u8,
    end_beat: f64,
    /// Played from the keyboard: it ends on Note Off, not at `end_beat`.
    live: bool,
}

/// External notes that can sound at once.
const MAX_EXT_NOTES: usize = 512;

#[derive(Clone, Copy, Debug)]
pub struct TrackMix {
    pub gain_db: f32,
    pub mute: bool,
    pub solo: bool,
    /// -1.0 (left) .. 1.0 (right), equal-power, unity at centre.
    pub pan: f32,
}

impl Default for TrackMix {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            mute: false,
            solo: false,
            pan: 0.0,
        }
    }
}

impl TrackMix {
    fn sanitized(self) -> Self {
        Self {
            gain_db: if self.gain_db.is_finite() {
                self.gain_db.clamp(-96.0, 6.0)
            } else {
                0.0
            },
            pan: if self.pan.is_finite() {
                self.pan.clamp(-1.0, 1.0)
            } else {
                0.0
            },
            ..self
        }
    }
}

/// Immutable-by-the-audio-thread snapshot of the arrangement.
#[derive(Default)]
pub struct Song {
    /// Must be sorted by `start_beat`; use [`Song::new`].
    pub notes: Vec<NoteEvent>,
    pub tracks: Vec<TrackMix>,
    /// Streamed audio regions (create with `Engine::stream_clip`).
    pub clips: Vec<AudioClip>,
    /// Controller changes for external instruments, sorted by beat (see [`Song::with_controls`]).
    pub controls: Vec<ControlEvent>,
}

impl Song {
    pub fn new(mut notes: Vec<NoteEvent>, tracks: Vec<TrackMix>) -> Self {
        notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        Self {
            notes,
            tracks,
            clips: Vec::new(),
            controls: Vec::new(),
        }
    }

    /// The song with these controller changes (sorted here).
    pub fn with_controls(mut self, mut controls: Vec<ControlEvent>) -> Self {
        controls.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        self.controls = controls;
        self
    }

    pub fn with_clips(mut self, clips: Vec<AudioClip>) -> Self {
        self.clips = clips;
        self
    }
}

/// Audio-thread side of a streamed audio region (Logic Pro: audio region on an audio track).
/// Reads [`Block`]s produced by the disk thread; never allocates or blocks.
pub struct AudioClip {
    pub track: u16,
    pub start_beat: f64,
    end_beat: f64,
    /// Length in device-rate frames.
    pub len_frames: u64,
    edit: ClipEdit,
    gain_linear: f32,
    rx: Consumer<Block>,
    shared: Arc<ClipShared>,
    gen: u16,
    cur: Option<Block>,
    /// Next clip-relative frame to play (negative = before the region starts).
    pos: i64,
    underruns: u32,
}

/// Non-destructive region edits (Logic Pro: Region Inspector Gain, Fade In/Out, Reverse).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipEdit {
    /// Region gain in dB, clamped to -96..=+24.
    pub gain_db: f32,
    /// Fade durations in quarter-note beats, following the project tempo map.
    pub fade_in_beats: f64,
    pub fade_out_beats: f64,
    pub fade_curve: FadeCurve,
    pub reverse: bool,
    /// Where in the file the region starts, in seconds (a region cut or trimmed at its
    /// start plays from later in the file).
    pub offset_secs: f64,
    /// How much of the file the region plays, in seconds; 0 plays to the end of the file.
    pub length_secs: f64,
}

impl Default for ClipEdit {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            fade_in_beats: 0.0,
            fade_out_beats: 0.0,
            fade_curve: FadeCurve::EqualPower,
            reverse: false,
            offset_secs: 0.0,
            length_secs: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FadeCurve {
    Linear,
    #[default]
    EqualPower,
    SCurve,
}

impl ClipEdit {
    pub(crate) fn clean(self) -> Self {
        Self {
            gain_db: if self.gain_db.is_finite() {
                self.gain_db.clamp(-96.0, 24.0)
            } else {
                0.0
            },
            fade_in_beats: if self.fade_in_beats.is_finite() {
                self.fade_in_beats.max(0.0)
            } else {
                0.0
            },
            fade_out_beats: if self.fade_out_beats.is_finite() {
                self.fade_out_beats.max(0.0)
            } else {
                0.0
            },
            offset_secs: if self.offset_secs.is_finite() {
                self.offset_secs.max(0.0)
            } else {
                0.0
            },
            length_secs: if self.length_secs.is_finite() {
                self.length_secs.max(0.0)
            } else {
                0.0
            },
            ..self
        }
    }
}

impl AudioClip {
    pub(crate) fn new(
        track: u16,
        start_beat: f64,
        len_frames: u64,
        edit: ClipEdit,
        rx: Consumer<Block>,
        shared: Arc<ClipShared>,
    ) -> Self {
        let edit = edit.clean();
        let gain_linear = 10.0f32.powf(edit.gain_db / 20.0);
        Self {
            track,
            start_beat,
            end_beat: start_beat,
            len_frames,
            edit,
            gain_linear,
            rx,
            shared,
            gen: 0,
            cur: None,
            pos: 0,
            underruns: 0,
        }
    }

    /// Continue playback `offset` frames into the clip (negative: before it starts).
    fn reposition(&mut self, offset: i64) {
        self.pos = offset;
        self.gen = self.gen.wrapping_add(1);
        self.cur = None;
        let want = self.pos.clamp(0, self.len_frames as i64) as u64;
        self.shared.request(self.gen, want);
    }

    /// Once per callback: drop stale blocks so the disk thread gets ring space back.
    fn service(&mut self) {
        while self.cur.as_ref().is_none_or(|b| b.gen != self.gen) {
            match self.rx.pop() {
                Ok(b) if b.gen == self.gen => self.cur = Some(b),
                Ok(_) => self.cur = None,
                Err(_) => break,
            }
        }
    }

    fn tick(&mut self) -> Option<(f32, f32)> {
        let p = self.pos;
        self.pos += 1;
        if p < 0 || p as u64 >= self.len_frames {
            return None;
        }
        let p = p as u64;
        loop {
            if let Some(b) = &self.cur {
                if b.gen == self.gen {
                    if p < b.start {
                        return None; // gap: data for later frames already queued
                    }
                    if p < b.start + b.frames as u64 {
                        let i = (p - b.start) as usize * 2;
                        return Some((b.data[i], b.data[i + 1]));
                    }
                }
                self.cur = None;
            }
            match self.rx.pop() {
                Ok(b) if b.gen == self.gen => self.cur = Some(b),
                Ok(_) => {}
                Err(_) => {
                    self.underruns += 1;
                    return None;
                }
            }
        }
    }

    fn envelope(&self, beat: f64, end_beat: f64) -> f32 {
        let curve = |x: f64| -> f32 {
            let x = x.clamp(0.0, 1.0);
            // Outside the fade, return its exact endpoint without evaluating sin.
            if x == 0.0 || x == 1.0 {
                return x as f32;
            }
            match self.edit.fade_curve {
                FadeCurve::Linear => x as f32,
                FadeCurve::EqualPower => (x * std::f64::consts::FRAC_PI_2).sin() as f32,
                FadeCurve::SCurve => (x * x * (3.0 - 2.0 * x)) as f32,
            }
        };
        let mut gain = self.gain_linear;
        if self.edit.fade_in_beats > 0.0 {
            gain *= curve((beat - self.start_beat) / self.edit.fade_in_beats);
        }
        if self.edit.fade_out_beats > 0.0 {
            gain *= curve((end_beat - beat) / self.edit.fade_out_beats);
        }
        gain
    }
}

/// UI -> engine messages. Heap payloads are boxed so the queue slot stays small; the old
/// payload travels back through the garbage queue so it is freed off the audio thread.
pub enum Command {
    Play,
    /// Clicks `clicks` times, `beats_per_click` quarter notes apart, then plays from where
    /// the playhead is (Logic: Count-in). Pause or Stop cancels it.
    CountIn {
        clicks: u8,
        beats_per_click: f64,
    },
    /// The live input, for monitoring. Replaces the one before.
    SetInput(Option<Box<InputFeed>>),
    /// A track's plug-ins (instrument and Audio FX); `None` takes them away. The ones
    /// before come back as [`Garbage::Plugins`].
    SetPlugins {
        track: u16,
        plugins: Option<Box<PluginTrack>>,
    },
    /// The plug-in effects of the master bus (Stereo Out), after its built-in ones and before
    /// the master fader; `None` takes them away. They process each whole block of the mix
    /// before it is played, so they add only their own delay. The ones before come back as
    /// [`Garbage::Plugins`].
    SetMasterPlugins(Option<Box<PluginTrack>>),
    /// Master plug-in effects passed over for now (bit `k`: effect `k`), for Low Latency Mode.
    SetMasterPluginsOff(u64),
    /// MIDI clock out on or off (Logic: Transmit MIDI Clock).
    SetMidiClock(bool),
    /// Parameter `index` of the `effect`-th plug-in effect of `track` (native effects);
    /// [`MASTER_TRACK`] for the master bus.
    SetPluginParam {
        track: u16,
        effect: u8,
        index: u16,
        value: f32,
    },
    /// The tracks that hear their input now (the first `count` of `tracks`).
    SetMonitors {
        tracks: [MonitorTrack; MAX_MONITORS],
        count: u8,
    },
    Pause,
    /// Stop and return to the position the transport started from.
    Stop,
    SetPosition {
        beat: f64,
    },
    /// Tempo and time-signature changes (Logic: Tempo / Signature tracks). Build the map on
    /// the UI thread; the old one is sent back through the garbage queue.
    SetTempoMap(Box<TempoMap>),
    SetMetronome {
        on: bool,
    },
    /// Metronome settings (Logic: Project Settings > Metronome): the click's level in dB
    /// (clamped to -60..0) and whether the first beat of each bar has the higher accent click.
    /// Non-finite levels are ignored.
    SetClick {
        gain_db: f32,
        accent: bool,
    },
    /// Reference pitch of software instruments: the frequency of A4 in Hz (Logic: Project
    /// Settings > Tuning), clamped to 427..453 like Logic's. Non-finite values are ignored.
    SetTuning {
        a4_hz: f32,
    },
    /// Master fader (Logic: Stereo Out level), clamped to 0..2 linear. Changes ramp over
    /// 5 ms during playback/audition; silent stopped setup is immediate. Non-finite values
    /// are ignored so malformed commands cannot poison the output.
    SetMasterGain {
        linear: f32,
    },
    /// Logic's cycle locators, in quarter-note beats. The end is exclusive; wrapping
    /// preserves fractional-sample elapsed time. Invalid or non-finite bounds disable cycle.
    SetCycle {
        on: bool,
        start_beat: f64,
        end_beat: f64,
    },
    /// Track fader/pan (Logic: channel strip). Gain and pan ramp over 5 ms while sounding;
    /// mute and solo retain immediate switching. New song snapshots initialize immediately.
    SetTrackMix {
        track: u16,
        mix: TrackMix,
    },
    SetSong(Box<Song>),
    /// Replaces only the song's notes (sorted by start; build with [`Command::set_notes`]).
    /// Streamed audio clips keep playing and sounding voices are not cut, so MIDI edits while
    /// playing cause no audio dropout. Use `SetSong` when clips or the track count change.
    SetNotes(Vec<NoteEvent>),
    /// Replaces the song's controller changes for external instruments (sorted by beat; build
    /// with [`Command::set_controls`]).
    SetControls(Vec<ControlEvent>),
    /// Logic Pro "Test Oscillator": a sine on the output, independent of the transport.
    /// `hz <= 0` turns it off.
    SetTestTone {
        hz: f32,
        gain: f32,
    },
    /// Live note (clicking the Piano Roll keys, auditioning while editing). Works while the
    /// transport is stopped and is released by `NoteOff` (or automatically after 4 s).
    NoteOn {
        pitch: u8,
        vel: u8,
        track: u16,
    },
    /// Releases this live pitch on every track (legacy/global audition release).
    NoteOff {
        pitch: u8,
    },
    /// Releases a live pitch only on its instrument track, like Logic's per-track MIDI
    /// routing. Other tracks and timeline notes are unaffected; unknown tracks are ignored.
    NoteOffTrack {
        pitch: u8,
        track: u16,
    },
    /// Makes `track` play an external instrument: its notes go out as [`MidiOutEvent`]s
    /// instead of sounding in the engine.
    SetTrackExternal {
        track: u16,
        external: bool,
    },
    /// Sets the synth patch used by instrument notes on `track` (Library patch).
    SetTrackPatch {
        track: u16,
        patch: SynthPatch,
    },
    /// Makes `track` play its notes with a SoundFont instrument (`None`: back to its synth
    /// patch). The player is made off the audio thread; the one it replaces, and the notes
    /// sounding on it, go away with it.
    SetTrackSoundFont {
        track: u16,
        player: Option<Box<SfPlayer>>,
    },
    /// The effects of a channel strip (Logic: Audio FX slots), made off the audio thread. The
    /// strip's old effects come back through the garbage queue; their tails stop.
    SetInserts {
        track: u16,
        chain: Box<Chain>,
    },
    /// The effects of the master bus (Stereo Out): they process the whole mix before the master
    /// fader. The old chain goes back through the garbage queue.
    SetMasterInserts(Box<Chain>),
    /// Space Designer: a new impulse response for the effect in `slot` of `track` (made with
    /// [`Convolver::for_params`]); the old one goes back through the garbage queue.
    SetConvolver {
        track: u16,
        slot: u8,
        conv: Box<crate::convolver::Convolver>,
    },
    /// New parameters and bypass for effect `slot` of a strip's chain (no allocation, so
    /// knobs can move while it plays). Ignored when the slot does not exist.
    SetFx {
        track: u16,
        slot: u8,
        params: [f32; MAX_FX_PARAMS],
        bypass: bool,
    },
    /// Delay compensation of a strip (see [`Pdc`]): every strip is delayed so that effects with
    /// a look-ahead do not put it out of time with the others. The old delay comes back
    /// through the garbage queue.
    SetPdc {
        track: u16,
        pdc: Box<Pdc>,
    },
    /// Where a strip's signal goes: its sends, and for an Aux strip the bus it listens to.
    SetRoute {
        track: u16,
        route: Route,
    },
    /// Replaces all automation lanes (build each with [`AutoLane::sanitized`]).
    SetAutomation(Vec<AutoLane>),
    /// Turns MIDI Time Code output on or off (Logic: Synchronization > MTC). While on, the
    /// engine sends the SMPTE time of the playhead as [`MtcMessage`]s: quarter frames while
    /// playing, a full frame when the position jumps. `offset_frames` is the SMPTE time of the
    /// project start, in frames.
    SetMtc {
        on: bool,
        rate: SmpteRate,
        offset_frames: i64,
    },
    /// Record > Process Buffer Range (Logic): the plug-ins of tracks that are not live run on
    /// the playback thread in blocks of `frames` (see [`crate::prerender`]), heard
    /// [`crate::prerender::prerender_delay`] frames after their input. 0 runs every plug-in
    /// in the I/O callback, as before.
    SetProcessBuffer {
        frames: u32,
    },
    /// Whether `track` is live (record-enabled, monitoring its input, the selected instrument
    /// being played, or an Aux strip they reach): its plug-ins run in the I/O callback at
    /// the I/O buffer size. Moving between the two places fades the track out and in.
    SetTrackLive {
        track: u16,
        live: bool,
    },
    /// How long after the transport renders a frame it is heard, in frames: `strips`, the
    /// delay compensation every strip waits for before its fader; `master`, the whole delay
    /// up to the Stereo Out. Fader automation is read `strips` frames late, MIDI out is
    /// stamped `master` frames later and the click (count-in included) sounds `master` frames
    /// later, so all stay in time with the compensated sound.
    SetOutputDelay {
        strips: u32,
        master: u32,
    },
    /// Delays the regions of a live track (its audio regions and the notes its plug-in
    /// instrument plays from them), not its input or the notes played live, by `frames`, so
    /// they stay in time with the pre-rendered tracks while the input is heard at once.
    SetContentDelay {
        track: u16,
        frames: u32,
    },
}

impl Command {
    pub(crate) fn retires_state(&self) -> bool {
        matches!(
            self,
            Self::SetSong(_)
                | Self::SetNotes(_)
                | Self::SetControls(_)
                | Self::SetTempoMap(_)
                | Self::SetInserts { .. }
                | Self::SetConvolver { .. }
                | Self::SetMasterInserts(_)
                | Self::SetPdc { .. }
                | Self::SetAutomation(_)
                | Self::SetInput(_)
                | Self::SetPlugins { .. }
                | Self::SetMasterPlugins(_)
        )
    }

    /// [`Command::SetControls`] with `controls` sorted as the engine requires.
    pub fn set_controls(mut controls: Vec<ControlEvent>) -> Self {
        controls.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        Command::SetControls(controls)
    }

    /// [`Command::SetNotes`] with `notes` sorted as the engine requires.
    pub fn set_notes(mut notes: Vec<NoteEvent>) -> Self {
        notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        Command::SetNotes(notes)
    }
}

pub enum Garbage {
    Song(Box<Song>),
    Notes(Vec<NoteEvent>),
    Controls(Vec<ControlEvent>),
    TempoMap(Box<TempoMap>),
    Chain(Box<Chain>),
    Convolver(Box<crate::convolver::Convolver>),
    Pdc(Box<Pdc>),
    Automation(Vec<AutoLane>),
    Input(Box<InputFeed>),
    SoundFont(Box<SfPlayer>),
    Plugins(Box<PluginTrack>),
}

/// What the engine shows the UI after each audio block: the playhead, the transport and the
/// meters, all from the same block. Read with [`StateReader::read`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineState {
    pub position_beats: f64,
    pub playing: bool,
    /// Master output peak (left, right), linear, falling back slowly like a meter.
    pub peaks: (f32, f32),
    /// Post-fader stereo peak per track, linear, for the mixer meters.
    pub track_peaks: [(f32, f32); MAX_TRACKS],
    /// Gain reduction in dB (0 or more) of the compressors of each strip, for the mixer's
    /// Gain Reduction meter.
    pub gain_reduction: [f32; MAX_TRACKS],
    /// Level (linear) going into the Space Designer of each strip, for its Input meter.
    pub fx_input: [f32; MAX_TRACKS],
    /// The same two for the master bus's effects: gain reduction in dB and Space Designer input.
    pub master_reduction: f32,
    pub master_fx_input: f32,
    /// Frames in the most recent device callback.
    pub block_frames: u32,
    /// Device-reported delay from callback to DAC, in microseconds (0 if unknown).
    pub output_latency_us: u32,
    /// The delay of the plug-ins on the master bus that are on, in microseconds: everything
    /// is heard this much later on top of `output_latency_us`.
    pub master_latency_us: u32,
    /// Frames a pre-rendered plug-in is heard after its input (one hop; see
    /// [`crate::prerender::prerender_delay`]); 0 while every plug-in runs in the I/O callback.
    pub prerender_delay: u32,
    /// The process buffer the engine runs with (0: off).
    pub process_buffer: u32,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            position_beats: 0.0,
            playing: false,
            peaks: (0.0, 0.0),
            track_peaks: [(0.0, 0.0); MAX_TRACKS],
            gain_reduction: [0.0; MAX_TRACKS],
            fx_input: [0.0; MAX_TRACKS],
            master_reduction: 0.0,
            master_fx_input: 0.0,
            block_frames: 0,
            output_latency_us: 0,
            master_latency_us: 0,
            prerender_delay: 0,
            process_buffer: 0,
        }
    }
}

impl EngineState {
    /// Gain reduction in dB of the compressors of `track` (0 when there is none).
    pub fn gain_reduction(&self, track: usize) -> f32 {
        self.gain_reduction.get(track).copied().unwrap_or(0.0)
    }

    /// The level going into the Space Designer of `track` (linear; 0 when it has none).
    pub fn fx_input(&self, track: usize) -> f32 {
        self.fx_input.get(track).copied().unwrap_or(0.0)
    }

    /// Post-fader (left, right) peak of `track`, linear; silence for a track out of range.
    pub fn track_peaks(&self, track: usize) -> (f32, f32) {
        self.track_peaks.get(track).copied().unwrap_or((0.0, 0.0))
    }
}

/// UI side of the engine's state: the newest complete [`EngineState`], through a lock-free
/// triple buffer (the audio thread never waits for the UI, and the UI never sees a value made
/// of two different blocks).
pub struct StateReader(Reader<EngineState>);

impl StateReader {
    /// The state after the newest audio block. Wait-free.
    pub fn read(&mut self) -> &EngineState {
        self.0.read()
    }
}

/// Engine -> UI counters, all lock-free. The per-block state is in [`EngineState`].
pub struct Shared {
    pub xruns: AtomicU32,
    /// Callbacks in which a streamed clip had no data ready (disk too slow).
    pub underruns: AtomicU32,
    /// Highest DSP load (f32 bits, 1.0 = the whole callback budget) since the UI last read it.
    dsp_load_max: AtomicU32,
    /// The same for the playback (pre-render) thread: busy time over the audio it made.
    worker_load_max: AtomicU32,
    /// Overloads Core Audio reported (`kAudioDeviceProcessorOverload`), shared with the
    /// device listener.
    pub hal_overloads: Arc<AtomicU32>,
    /// Overloads the engine saw itself: a callback that took longer than its buffer, or a
    /// callback that came late (frames were skipped between two).
    pub late_callbacks: AtomicU32,
    /// I/O callbacks in which a pre-rendered track had no audio ready (the playback thread
    /// fell behind): it is silent for those frames; the callback never waits.
    pub prerender_underruns: AtomicU32,
    /// The heaviest plug-in since the UI last read it: its share of the real-time budget
    /// (f32 bits) in the high half, `track << 8 | slot` in the low half.
    heaviest: std::sync::atomic::AtomicU64,
    /// The Core Audio device whose I/O workgroup the playback thread joins (0: none).
    pub(crate) workgroup_device: Arc<AtomicU32>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            xruns: AtomicU32::new(0),
            underruns: AtomicU32::new(0),
            dsp_load_max: AtomicU32::new(0),
            worker_load_max: AtomicU32::new(0),
            hal_overloads: Arc::new(AtomicU32::new(0)),
            late_callbacks: AtomicU32::new(0),
            prerender_underruns: AtomicU32::new(0),
            heaviest: std::sync::atomic::AtomicU64::new(0),
            workgroup_device: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Audio thread: report one callback's DSP load (see [`dsp_load`]). Lock- and alloc-free.
    pub fn record_dsp_load(&self, load: f32) {
        // Non-negative f32s order like their bit patterns, so an integer max works.
        self.dsp_load_max
            .fetch_max(load.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// UI thread: the highest DSP load since the previous call (Logic's CPU meter), then reset.
    /// This is the I/O callback's load: the live tracks and the mix (the meter's "Live").
    pub fn take_dsp_load(&self) -> f32 {
        f32::from_bits(self.dsp_load_max.swap(0, Ordering::Relaxed))
    }

    /// Playback thread: report the load of one round of pre-rendering.
    pub fn record_worker_load(&self, load: f32) {
        self.worker_load_max
            .fetch_max(load.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// UI thread: the playback thread's highest load since the previous call (the meter's
    /// "Playback"; Logic shows the pre-rendered share as part of its Audio meter).
    pub fn take_worker_load(&self) -> f32 {
        f32::from_bits(self.worker_load_max.swap(0, Ordering::Relaxed))
    }

    /// Audio or playback thread: plug-in `slot` (0 the instrument, `k + 1` effect `k`) of
    /// `track` used `load` of the real-time budget of the block it processed.
    pub fn record_plugin_load(&self, track: u16, slot: u8, load: f32) {
        let bits = u64::from(load.max(0.0).to_bits()) << 32;
        let id = u64::from(track) << 8 | u64::from(slot);
        self.heaviest.fetch_max(bits | id, Ordering::Relaxed);
    }

    /// UI thread: the heaviest plug-in since the previous call, as (track, slot, load), then
    /// reset. `None` when no plug-in ran.
    pub fn take_heaviest_plugin(&self) -> Option<(u16, u8, f32)> {
        let v = self.heaviest.swap(0, Ordering::Relaxed);
        (v != 0).then(|| {
            let id = v as u32;
            ((id >> 8) as u16, id as u8, f32::from_bits((v >> 32) as u32))
        })
    }

    /// System overloads so far (Logic: "System Overload"): the larger of what Core Audio
    /// reported and what the engine saw itself, since both usually see the same event.
    pub fn overloads(&self) -> u32 {
        self.hal_overloads
            .load(Ordering::Relaxed)
            .max(self.late_callbacks.load(Ordering::Relaxed))
    }
}

/// Share of the real-time budget a callback used: time spent rendering `frames` divided by
/// the time those frames take to play. Above 1.0 the device underruns (Logic: "System
/// Overload").
pub fn dsp_load(busy: std::time::Duration, frames: usize, sample_rate: u32) -> f32 {
    if frames == 0 || sample_rate == 0 {
        return 0.0;
    }
    (busy.as_secs_f64() * sample_rate as f64 / frames as f64) as f32
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    releasing: bool,
    phase_a: f32,
    phase_b: f32,
    base_hz: f32,
    amp: f32,
    env: f32,
    /// 0 attack, 1 decay, 2 sustain, 3 release.
    stage: u8,
    rel_step: f32,
    end_beat: f64,
    track: u16,
    /// Live note: ignores the transport and the timeline, ends on `NoteOff`.
    live: bool,
    pitch: u8,
    /// Safety net so a lost `NoteOff` cannot hang a note forever.
    live_left: u32,
    patch: SynthPatch,
    filter: Svf,
    vibrato: f32,
    noise: u32,
    pitch_env: f32,
    /// Oscillator B / A frequency ratio (fixed per note).
    ratio_b: f32,
    /// `pitch_env` decay over one control block.
    pitch_decay_block: f32,
    /// Samples until the control-rate values below are recomputed.
    ctrl_left: u32,
    dt_a: f32,
    dt_b: f32,
    filter_f: f32,
    filter_damping: f32,
    /// Envelope increments per sample for the attack and decay stages (fixed per note).
    attack_step: f32,
    decay_step: f32,
}

/// Modulation (vibrato, pitch drop, filter envelope) is recomputed every `CTRL_BLOCK` samples
/// (0.33 ms at 48 kHz) instead of every sample: it removes the per-sample `powf`/`sin` calls,
/// the bulk of the synth's cost, without audible stepping.
const CTRL_BLOCK: u32 = 16;

/// The most SoundFont notes sounding at once across all tracks. A note that finds no free
/// slot is not played.
const MAX_SF_NOTES: usize = 128;

/// A note played by a track's SoundFont instrument (see [`SfPlayer`]).
#[derive(Clone, Copy, Debug, Default)]
struct SfNote {
    active: bool,
    track: u16,
    pitch: u8,
    /// Timeline notes end at this beat; live notes end on their `NoteOff`.
    end_beat: f64,
    live: bool,
    /// Safety net so a live note whose `NoteOff` never came still ends.
    live_left: u32,
    /// The note's `NoteOff` was sent (or it must end at the next frame).
    released: bool,
}

/// The app's halves of the queues a core publishes to.
pub(crate) struct Readers {
    pub state: Option<StateReader>,
    pub midi: Option<Consumer<MidiOutEvent>>,
    pub mtc: Option<Consumer<MtcMessage>>,
    pub audition: Option<crate::audition::AuditionRemote>,
}

pub struct EngineCore {
    sr: f32,
    rx: Consumer<Command>,
    trash: Producer<Garbage>,
    shared: Arc<Shared>,
    /// What the UI was last shown; the meters fall back from it.
    shown: EngineState,
    state: Writer<EngineState>,
    state_reader: Option<StateReader>,
    song: Box<Song>,
    next_note: usize,
    next_control: usize,
    /// External tracks whose sustain pedal was last sent down.
    ext_sustain: Vec<bool>,
    /// The region controllers the engine's own instruments follow, per track.
    midi_ctl: Vec<MidiCtl>,
    beat: f64,
    start_beat: f64,
    playing: bool,
    tempo_map: Box<TempoMap>,
    metronome: bool,
    cycle: Option<(f64, f64)>,
    master: GainRamp,
    voices: [Voice; MAX_VOICES],
    click_remaining: u32,
    click_phase: f32,
    click_freq: f32,
    click_gain: f32,
    click_accent: bool,
    /// Clicks waiting for the output delay (see [`Command::SetOutputDelay`]), as (frames
    /// left, frequency): the click is heard with the delay-compensated tracks.
    click_queue: [(u32, f32); CLICK_QUEUE],
    click_queued: usize,
    /// The transport just started (Play, the end of a count-in, a locate while playing): the
    /// beat it starts on gets its click.
    start_click: bool,
    /// Frequency of A4 in Hz.
    tuning_hz: f32,
    /// A count-in is running: the clicks left, the frames to the next one, the frames
    /// between clicks and how many clicks it has.
    counting: bool,
    count_left: u8,
    count_wait: u32,
    count_step: u32,
    input: Option<Box<InputFeed>>,
    monitors: [MonitorTrack; MAX_MONITORS],
    monitor_count: usize,
    track_gain_l: Vec<f32>,
    track_gain_r: Vec<f32>,
    track_ramp_l: Vec<GainRamp>,
    track_ramp_r: Vec<GainRamp>,
    mix_ramp_remaining: u32,
    track_pk_l: Vec<f32>,
    track_pk_r: Vec<f32>,
    track_audible: Vec<bool>,
    track_patch: Vec<SynthPatch>,
    track_external: Vec<bool>,
    /// SoundFont instruments per track (`None`: the track plays its synth patch).
    sf: Vec<Option<Box<SfPlayer>>>,
    /// The tracks that have a SoundFont instrument, rendered every frame.
    sf_tracks: Vec<u16>,
    /// Each track's plug-ins, and the tracks that have any.
    plugins: Vec<Option<Box<PluginTrack>>>,
    plugin_tracks: Vec<u16>,
    /// The frame of the block being made (where a plug-in's MIDI goes).
    frame_i: usize,
    /// When this block's first frame is heard, host-clock nanoseconds (0: unknown).
    heard_ns: u64,
    /// MIDI clock out is on, running, and the tick (in 1/24 quarter notes) it sends next.
    midi_clock: bool,
    clock_running: bool,
    next_tick: f64,
    /// The SoundFont notes sounding now (a fixed table: nothing is allocated per note).
    sf_notes: [SfNote; MAX_SF_NOTES],
    /// Effects per strip (an empty box for none), sends and bus inputs.
    chains: Vec<Option<Box<Chain>>>,
    /// The effects of the master bus.
    master_chain: Option<Box<Chain>>,
    /// The plug-in effects of the master bus, after `master_chain`.
    master_plugins: Option<Box<PluginTrack>>,
    /// Their delay (the ones that are on), in nanoseconds.
    master_latency_ns: u64,
    /// Delay compensation per strip.
    pdc: Vec<Option<Box<Pdc>>>,
    routes: Vec<Route>,
    /// Strips with effects or sends (processed in order), and Aux strips (processed after
    /// them, once the buses are full).
    complex: Vec<u16>,
    aux: Vec<u16>,
    /// 0 plain strip, 1 complex, 2 aux.
    strip_kind: Vec<u8>,
    /// The signal of each strip for the current sample, before effects and fader.
    /// The strips' signals and the buses are summed in 64 bits, so many strips (and the
    /// buses they feed) add up without losing the quiet ones to rounding.
    in_l: Vec<f64>,
    in_r: Vec<f64>,
    touched: Vec<bool>,
    dirty: Vec<u16>,
    bus_l: [f64; MAX_BUSES],
    bus_r: [f64; MAX_BUSES],
    automation: Vec<AutoLane>,
    /// The value the automation gives each strip right now (None: its fader or knob rules).
    auto_vol: Vec<Option<f32>>,
    auto_pan: Vec<Option<f32>>,
    auto_on: Vec<bool>,
    auto_tracks: Vec<u16>,
    /// Some lane is played.
    auto_any: bool,
    auto_left: u32,
    /// Mute automation: the strip is muted by its lane now.
    auto_mute: Vec<bool>,
    /// Send automation: whether each send follows a lane, and its gain ramping towards it.
    auto_send_on: Vec<[bool; MAX_SENDS]>,
    auto_send: Vec<[GainRamp; MAX_SENDS]>,
    /// The Stereo Out fader's lane is played (its value in dB is aimed at every 32 frames),
    /// and the master fader the user set.
    auto_master: bool,
    master_user: f32,
    /// The value each plug-in lane last sent (NaN: none yet), by lane; room for
    /// [`MAX_AUTO_LANES`] lanes is made up front so the audio thread never allocates.
    auto_last: Vec<f32>,
    midi_out: Producer<MidiOutEvent>,
    midi_out_reader: Option<Consumer<MidiOutEvent>>,
    mtc: MtcGenerator,
    mtc_rate: SmpteRate,
    mtc_offset_frames: i64,
    /// Seconds from the project start to the playhead, kept sample by sample while MTC is on.
    mtc_secs: f64,
    mtc_out: Producer<MtcMessage>,
    mtc_out_reader: Option<Consumer<MtcMessage>>,
    ext_notes: Vec<ExtNote>,
    test_hz: f32,
    test_gain: f32,
    test_phase: f32,
    /// The browser's preview loop (see [`crate::audition`]), and the window's end of it until
    /// the app takes it.
    audition: crate::audition::Auditioner,
    audition_remote: Option<crate::audition::AuditionRemote>,
    /// Record > Process Buffer in frames (0: off) and the delay it gives a pre-rendered hop.
    process_buffer: u32,
    /// Which tracks are live (their plug-ins stay in the I/O callback).
    track_live: Vec<bool>,
    /// (strips, master): see [`Command::SetOutputDelay`].
    output_delay: (u32, u32),
    /// Frames each live track's regions are delayed by (see [`Command::SetContentDelay`]).
    content_delay: Vec<u32>,
    /// The delay lines that carry the regions of live tracks (a few tracks are live at once),
    /// and the line of each track (`NO_LINE`: none).
    content_lines: Vec<ContentLine>,
    content_line: Vec<u8>,
    /// The playback thread, when there is one (pre-rendering is off without it).
    workers: Vec<crate::prerender::WorkerPort>,
    /// Lanes whose track went away while on a playback thread (thread, lane), still to be
    /// retired there, and plug-ins that came back for a track that no longer has them.
    retire: Vec<(usize, u32)>,
    #[allow(clippy::vec_box)] // moved through rings as one pointer
    orphans: Vec<Box<crate::plugin_track::LaneWork>>,
    /// The transport's beat at each of the last frames, for automation read late (see
    /// [`Command::SetOutputDelay`]).
    beat_hist: Vec<f64>,
    hist_pos: usize,
    /// The beat a transport-following instrument was given at each of the last frames (NaN:
    /// stopped), so a live track's instrument can follow it its content delay late, and the
    /// plug-ins of Aux strips and the Stereo Out the output delay late. As long as
    /// `beat_hist`, which covers the longest output delay.
    transport_hist: Vec<f64>,
    transport_pos: usize,
    /// The cycle just wrapped: the plug-ins are given this, the cycle start, for the next
    /// frame instead of the transport's beat (a fraction of a frame past it), so what sits
    /// on the cycle start plays on every pass. NaN otherwise.
    wrap_beat: f64,
}

/// Regions of live tracks that can be delayed at once (see [`Command::SetContentDelay`]).
const CONTENT_LINES: usize = 32;
/// The longest content delay, in frames.
const CONTENT_FRAMES: usize = 16_384;
const NO_LINE: u8 = u8::MAX;
/// Clicks that can wait for the output delay at once (one a beat; the delay is under a second).
const CLICK_QUEUE: usize = 16;
/// Frames of transport history kept for late automation.
const BEAT_HISTORY: usize = 32_768;

/// A stereo delay line for the regions of one live track.
struct ContentLine {
    track: u16,
    l: Vec<f32>,
    r: Vec<f32>,
    pos: usize,
    /// What the track's regions gave this frame.
    acc: (f64, f64),
}

impl ContentLine {
    fn new() -> Self {
        Self {
            track: u16::MAX,
            l: vec![0.0; CONTENT_FRAMES],
            r: vec![0.0; CONTENT_FRAMES],
            pos: 0,
            acc: (0.0, 0.0),
        }
    }

    /// This frame's regions in, the regions of `delay` frames ago out.
    #[inline]
    fn tick(&mut self, delay: u32) -> (f64, f64) {
        let n = self.l.len();
        let (l, r) = std::mem::take(&mut self.acc);
        self.l[self.pos] = l as f32;
        self.r[self.pos] = r as f32;
        let d = (delay as usize).min(n - 1);
        let at = (self.pos + n - d) % n;
        let out = (f64::from(self.l[at]), f64::from(self.r[at]));
        self.pos = (self.pos + 1) % n;
        out
    }

    fn clear(&mut self) {
        self.l.fill(0.0);
        self.r.fill(0.0);
        self.acc = (0.0, 0.0);
    }
}

/// Keeps an oscillator phase in 0..1. `fract` costs a `floorf` call per sample on baseline
/// x86-64; the phase only passes 1.0 once per period, so test first.
#[inline]
fn wrap_phase(p: f32) -> f32 {
    if p < 1.0 {
        p
    } else {
        p.fract()
    }
}

/// `x`, or 0 when it is NaN or infinite.
#[inline]
fn finite_or_zero(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// A meter's new peak: this callback's peak, or the old one decayed. Never NaN or infinite, so
/// a bad value cannot stick (`inf * 0.85` is still `inf`).
#[inline]
fn decayed_peak(new: f32, old: f32) -> f32 {
    finite_or_zero(new.max(old * 0.85)).min(16.0)
}

/// Logic's range for the reference pitch (A4) in Hz.
pub const MIN_TUNING_HZ: f32 = 427.0;
pub const MAX_TUNING_HZ: f32 = 453.0;
/// The click's level before Project Settings > Metronome changes it (linear, about -10.5 dB).
const DEFAULT_CLICK_GAIN: f32 = 0.3;

fn midi_hz(p: u8, a4_hz: f32) -> f32 {
    a4_hz * 2f32.powf((p as f32 - 69.0) / 12.0)
}

impl EngineCore {
    pub fn new(
        sample_rate: u32,
        rx: Consumer<Command>,
        trash: Producer<Garbage>,
        shared: Arc<Shared>,
    ) -> Self {
        let (state, reader) = triple_buffer(EngineState::default());
        let (midi_out, midi_out_reader) = rtrb::RingBuffer::new(1024);
        let (mtc_out, mtc_out_reader) = rtrb::RingBuffer::new(256);
        let (audition, audition_remote) = crate::audition::Auditioner::new(sample_rate);
        Self {
            sr: sample_rate.max(1) as f32,
            rx,
            trash,
            shared,
            shown: EngineState::default(),
            state,
            state_reader: Some(StateReader(reader)),
            song: Box::new(Song::default()),
            next_note: 0,
            next_control: 0,
            ext_sustain: vec![false; MAX_TRACKS],
            midi_ctl: vec![MidiCtl::default(); MAX_TRACKS],
            beat: 0.0,
            start_beat: 0.0,
            playing: false,
            tempo_map: Box::default(),
            metronome: false,
            cycle: None,
            master: GainRamp::new(0.8),
            voices: [Voice::default(); MAX_VOICES],
            click_remaining: 0,
            click_phase: 0.0,
            click_freq: 1000.0,
            click_gain: DEFAULT_CLICK_GAIN,
            click_accent: true,
            click_queue: [(0, 0.0); CLICK_QUEUE],
            click_queued: 0,
            start_click: false,
            tuning_hz: 440.0,
            counting: false,
            count_left: 0,
            count_wait: 0,
            count_step: 1,
            input: None,
            monitors: [MonitorTrack::default(); MAX_MONITORS],
            monitor_count: 0,
            track_gain_l: vec![1.0; MAX_TRACKS],
            track_gain_r: vec![1.0; MAX_TRACKS],
            track_ramp_l: vec![GainRamp::new(1.0); MAX_TRACKS],
            track_ramp_r: vec![GainRamp::new(1.0); MAX_TRACKS],
            mix_ramp_remaining: 0,
            track_pk_l: vec![0.0; MAX_TRACKS],
            track_pk_r: vec![0.0; MAX_TRACKS],
            track_audible: vec![true; MAX_TRACKS],
            track_patch: vec![SynthPatch::default(); MAX_TRACKS],
            track_external: vec![false; MAX_TRACKS],
            sf: (0..MAX_TRACKS).map(|_| None).collect(),
            sf_tracks: Vec::with_capacity(MAX_TRACKS),
            plugins: (0..MAX_TRACKS).map(|_| None).collect(),
            plugin_tracks: Vec::with_capacity(MAX_TRACKS),
            frame_i: 0,
            heard_ns: 0,
            midi_clock: false,
            clock_running: false,
            next_tick: 0.0,
            sf_notes: [SfNote::default(); MAX_SF_NOTES],
            chains: (0..MAX_TRACKS).map(|_| None).collect(),
            master_chain: None,
            master_plugins: None,
            master_latency_ns: 0,
            pdc: (0..MAX_TRACKS).map(|_| None).collect(),
            routes: vec![Route::default(); MAX_TRACKS],
            complex: Vec::with_capacity(MAX_TRACKS),
            aux: Vec::with_capacity(MAX_TRACKS),
            strip_kind: vec![0; MAX_TRACKS],
            in_l: vec![0.0; MAX_TRACKS],
            in_r: vec![0.0; MAX_TRACKS],
            touched: vec![false; MAX_TRACKS],
            dirty: Vec::with_capacity(MAX_TRACKS),
            bus_l: [0.0; MAX_BUSES],
            bus_r: [0.0; MAX_BUSES],
            auto_any: false,
            automation: Vec::new(),
            auto_vol: vec![None; MAX_TRACKS],
            auto_pan: vec![None; MAX_TRACKS],
            auto_on: vec![false; MAX_TRACKS],
            auto_tracks: Vec::with_capacity(MAX_TRACKS),
            auto_left: 0,
            auto_mute: vec![false; MAX_TRACKS],
            auto_send_on: vec![[false; MAX_SENDS]; MAX_TRACKS],
            auto_send: vec![[GainRamp::new(0.0); MAX_SENDS]; MAX_TRACKS],
            auto_master: false,
            master_user: 0.8,
            auto_last: Vec::with_capacity(MAX_AUTO_LANES),
            midi_out,
            midi_out_reader: Some(midi_out_reader),
            mtc: MtcGenerator::default(),
            mtc_rate: SmpteRate::Fps25,
            mtc_offset_frames: 0,
            mtc_secs: 0.0,
            mtc_out,
            mtc_out_reader: Some(mtc_out_reader),
            ext_notes: Vec::with_capacity(MAX_EXT_NOTES),
            test_hz: 0.0,
            test_gain: 0.0,
            test_phase: 0.0,
            audition,
            audition_remote: Some(audition_remote),
            process_buffer: 0,
            track_live: vec![false; MAX_TRACKS],
            output_delay: (0, 0),
            content_delay: vec![0; MAX_TRACKS],
            content_lines: (0..CONTENT_LINES).map(|_| ContentLine::new()).collect(),
            content_line: vec![NO_LINE; MAX_TRACKS],
            workers: Vec::with_capacity(8),
            retire: Vec::with_capacity(MAX_TRACKS),
            orphans: Vec::with_capacity(MAX_TRACKS),
            beat_hist: vec![0.0; BEAT_HISTORY],
            hist_pos: 0,
            transport_hist: vec![f64::NAN; BEAT_HISTORY],
            transport_pos: 0,
            wrap_beat: f64::NAN,
        }
    }

    /// Connects the playback thread (see [`crate::prerender`]): the plug-ins of tracks that
    /// are not live run there once [`Command::SetProcessBuffer`] turns pre-rendering on.
    pub(crate) fn set_worker(&mut self, port: crate::prerender::WorkerPort) {
        self.workers.push(port);
    }

    /// The beat `lag` frames ago (the current one while stopped).
    #[inline]
    fn beat_ago(&self, lag: usize) -> f64 {
        if lag == 0 || !self.playing {
            return self.beat;
        }
        let n = self.beat_hist.len();
        self.beat_hist[(self.hist_pos + n - lag.min(n - 1)) % n]
    }

    /// The beat a lane read `lag` frames late aims at: `AUTO_BLOCK` frames after it.
    fn beat_ahead(&self, lag: usize) -> f64 {
        if !self.playing {
            return self.beat;
        }
        let auto = AUTO_BLOCK as usize;
        if lag >= auto {
            self.beat_ago(lag - auto)
        } else {
            let from = self.beat_ago(lag);
            self.tempo_map
                .advance(from, (auto - lag) as f64 / f64::from(self.sr))
        }
    }

    /// The delay before the playback thread's sound is heard, for this block size.
    fn prerender_hop(&self, block: usize) -> u32 {
        if self.workers.is_empty() {
            return 0;
        }
        let io = if self.shown.block_frames > 0 {
            self.shown.block_frames
        } else {
            block as u32
        };
        crate::prerender::prerender_delay(self.process_buffer, io)
    }

    /// A locate by the user: what was rendered ahead for the old position is not heard.
    fn flush_ahead(&mut self) {
        for k in 0..self.plugin_tracks.len() {
            if let Some(p) = self.plugins[self.plugin_tracks[k] as usize].as_mut() {
                p.flush();
            }
        }
        for pdc in self.pdc.iter_mut().flatten() {
            pdc.clear();
        }
        for line in self
            .content_lines
            .iter_mut()
            .filter(|l| l.track != u16::MAX)
        {
            line.clear();
        }
        // Clicks and transport beats on their way out are not heard either.
        self.click_queued = 0;
        self.transport_hist.fill(f64::NAN);
        self.wrap_beat = f64::NAN;
    }

    /// Plug-ins the playback thread gave back go home to their track.
    fn take_returned(&mut self) {
        for k in 0..self.workers.len() {
            while let Some(w) = self.workers[k].returned() {
                let t = w.track as usize;
                match self.plugins.get_mut(t) {
                    Some(Some(p)) if p.lane_id() == w.id && p.is_away() => p.come_home(w),
                    _ => {
                        if self.orphans.len() < self.orphans.capacity() {
                            self.orphans.push(w);
                        } else {
                            std::mem::forget(w);
                        }
                    }
                }
            }
        }
        if let Some(port) = self.workers.first_mut() {
            while let Some(w) = self.orphans.pop() {
                if let Err(w) = port.dispose(w) {
                    self.orphans.push(w);
                    break;
                }
            }
        }
        while let Some(&(k, id)) = self.retire.last() {
            if self.workers.get_mut(k).is_some_and(|port| !port.retire(id)) {
                break;
            }
            self.retire.pop();
        }
        let (frames, sr) = (self.process_buffer, self.sr);
        for port in self.workers.iter_mut() {
            if port.chunk_sent != frames && frames > 0 && port.set_chunk(frames, sr) {
                port.chunk_sent = frames;
            }
        }
    }

    /// Record > Process Buffer: `frames` (0 off).
    fn set_process_buffer(&mut self, frames: u32) {
        self.process_buffer = frames.min(crate::prerender::PROCESS_BUFFER_LARGE);
    }

    /// The regions of `track` wait `frames` (0: none): its audio regions and its engine
    /// synth's notes through a content line, its plug-in instrument's notes in the plug-in.
    fn set_content_delay(&mut self, track: u16, frames: u32) {
        let t = track as usize;
        if t >= MAX_TRACKS {
            return;
        }
        let frames = frames.min(CONTENT_FRAMES as u32 - 1);
        self.content_delay[t] = frames;
        if let Some(p) = self.plugins[t].as_mut() {
            p.set_content_delay(frames);
        }
        let line = self.content_line[t];
        if frames == 0 {
            if let Some(l) = self.content_lines.get_mut(line as usize) {
                l.track = u16::MAX;
            }
            self.content_line[t] = NO_LINE;
        } else if line == NO_LINE {
            if let Some(i) = self.content_lines.iter().position(|l| l.track == u16::MAX) {
                let l = &mut self.content_lines[i];
                l.clear();
                l.track = track;
                self.content_line[t] = i as u8;
            }
        }
    }

    /// Adds `(l, r)` from the regions of track `t` to its strip, or to its content line
    /// when the track is live and its regions wait.
    #[inline]
    fn add_content(&mut self, t: usize, l: f64, r: f64) {
        match self.content_lines.get_mut(self.content_line[t] as usize) {
            Some(line) => {
                line.acc.0 += l;
                line.acc.1 += r;
            }
            None => {
                self.in_l[t] += l;
                self.in_r[t] += r;
                if !self.touched[t] {
                    self.touched[t] = true;
                    self.dirty.push(t as u16);
                }
            }
        }
    }

    /// The regions of live tracks come out of their content lines into their strips.
    fn content_out(&mut self) {
        for k in 0..self.content_lines.len() {
            let track = self.content_lines[k].track;
            if track == u16::MAX {
                continue;
            }
            let t = track as usize;
            let (l, r) = self.content_lines[k].tick(self.content_delay[t]);
            if self.track_audible[t] && (l != 0.0 || r != 0.0) {
                self.in_l[t] += l;
                self.in_r[t] += r;
                if !self.touched[t] {
                    self.touched[t] = true;
                    self.dirty.push(t as u16);
                }
            }
        }
    }

    /// The UI's half of the state this core publishes after every block. There is one;
    /// it is `None` once taken.
    pub fn take_state_reader(&mut self) -> Option<StateReader> {
        self.state_reader.take()
    }

    /// The app's half of the MIDI the engine sends to external instruments. There is one; it
    /// is `None` once taken.
    pub fn take_midi_out_reader(&mut self) -> Option<Consumer<MidiOutEvent>> {
        self.midi_out_reader.take()
    }

    /// The app's half of the MIDI Time Code the engine sends. There is one; it is `None` once
    /// taken.
    pub fn take_mtc_reader(&mut self) -> Option<Consumer<MtcMessage>> {
        self.mtc_out_reader.take()
    }

    /// Every reader the app takes from the core, together.
    pub(crate) fn take_readers(&mut self) -> Readers {
        Readers {
            state: self.take_state_reader(),
            midi: self.take_midi_out_reader(),
            mtc: self.take_mtc_reader(),
            audition: self.audition_remote.take(),
        }
    }

    /// SMPTE frames of the playhead as MTC counts them.
    fn mtc_frames(&self) -> f64 {
        self.mtc_secs * self.mtc_rate.frames_per_second() + self.mtc_offset_frames as f64
    }

    /// The playhead moved or the tempo map changed: tell MTC receivers where it is.
    fn mtc_locate(&mut self) {
        if !self.mtc.is_on() {
            return;
        }
        self.mtc_secs = self.tempo_map.seconds_at(self.beat);
        let frames = self.mtc_frames();
        let tx = &mut self.mtc_out;
        self.mtc.locate(frames, |m| {
            let _ = tx.push(m);
        });
    }

    /// Whether `track`'s notes go out of the engine's own synth: to an external instrument
    /// or to a plug-in instrument.
    fn plays_elsewhere(&self, track: u16) -> bool {
        let t = track as usize;
        self.track_external.get(t).copied().unwrap_or(false)
            || self
                .plugins
                .get(t)
                .is_some_and(|p| p.as_ref().is_some_and(|p| p.has_instrument()))
    }

    /// Sends a Note On or Off to the external instrument of `track`, or to its plug-in
    /// instrument at this frame. Lost when the queue is full (nobody is listening), never
    /// blocks.
    fn send_midi(&mut self, track: u16, on: bool, pitch: u8, vel: u8, timeline: bool) {
        if let Some(Some(p)) = self.plugins.get_mut(track as usize) {
            if p.has_instrument() {
                let status = if on { 0x90 } else { 0x80 };
                let at = self.frame_i as u32;
                if timeline {
                    p.push_midi_timeline(at, status, pitch & 0x7f, vel & 0x7f);
                } else {
                    p.push_midi(at, status, pitch & 0x7f, vel & 0x7f);
                }
                return;
            }
        }
        let at_ns = self.stamp_for(timeline);
        let _ = self.midi_out.push(MidiOutEvent {
            track,
            at_ns,
            on,
            pitch,
            vel,
            controller: false,
            value: 0,
            ..MidiOutEvent::default()
        });
    }

    /// A note for an external instrument starts.
    fn ext_note_on(&mut self, n: NoteEvent, live: bool) {
        // A note that is still sounding is released first, as a keyboard would.
        if let Some(i) = self
            .ext_notes
            .iter()
            .position(|e| e.track == n.track && e.pitch == n.pitch)
        {
            let old = self.ext_notes.swap_remove(i);
            self.send_midi(old.track, false, old.pitch, 0, !old.live);
        }
        if self.ext_notes.len() < MAX_EXT_NOTES {
            self.ext_notes.push(ExtNote {
                track: n.track,
                pitch: n.pitch,
                end_beat: n.end_beat,
                live,
            });
            self.send_midi(n.track, true, n.pitch, n.vel.max(1), !live);
        }
    }

    /// Releases external notes that are due: those of the timeline once their end is reached
    /// or the transport stopped, all of them when `all`.
    fn release_ext_notes(&mut self, all: bool) {
        let mut i = 0;
        while i < self.ext_notes.len() {
            let e = self.ext_notes[i];
            let due = all || (!e.live && (self.beat >= e.end_beat || !self.playing));
            if due {
                self.ext_notes.swap_remove(i);
                self.send_midi(e.track, false, e.pitch, 0, !e.live);
            } else {
                i += 1;
            }
        }
    }

    /// The device callback's size and output delay, published with the next block's state.
    pub fn set_device_info(&mut self, block_frames: u32, output_latency_us: u32) {
        self.set_device_time(block_frames, output_latency_us, 0);
    }

    /// [`EngineCore::set_device_info`], with `now_ns` the host clock (nanoseconds) at the
    /// callback: the block's first frame is heard `output_latency_us` later, which is the time
    /// MIDI for that frame is stamped with. 0: unknown (MIDI then goes out unstamped).
    pub fn set_device_time(&mut self, block_frames: u32, output_latency_us: u32, now_ns: u64) {
        self.shown.block_frames = block_frames;
        self.shown.output_latency_us = output_latency_us;
        self.heard_ns = if now_ns > 0 {
            now_ns + u64::from(output_latency_us) * 1000
        } else {
            0
        };
    }

    /// The host time frame `frame_i` of this block is heard at (0: unknown).
    fn stamp(&self) -> u64 {
        if self.heard_ns == 0 {
            return 0;
        }
        // The master plug-ins delay everything that is heard.
        self.heard_ns
            + self.master_latency_ns
            + (self.frame_i as f64 * 1e9 / f64::from(self.sr.max(1.0) as u32)) as u64
    }

    /// The master plug-ins changed: their delay is measured again. No allocation.
    fn master_plugins_changed(&mut self) {
        let secs = self
            .master_plugins
            .as_ref()
            .map_or(0.0, |p| p.active_effect_latency_seconds());
        self.master_latency_ns = if secs.is_finite() && secs > 0.0 {
            (secs.min(10.0) * 1e9) as u64
        } else {
            0
        };
        self.shown.master_latency_us = (self.master_latency_ns / 1000) as u32;
    }

    /// [`EngineCore::stamp`] for a message from the timeline: it is heard with the sound of its
    /// frame, after the output delay (see [`Command::SetOutputDelay`]). A message played live
    /// goes out at once.
    fn stamp_for(&self, timeline: bool) -> u64 {
        let at = self.stamp();
        if at == 0 || !timeline {
            return at;
        }
        at + (f64::from(self.output_delay.1) * 1e9 / f64::from(self.sr.max(1.0))) as u64
    }

    /// A system message (clock, start, ...) at this frame.
    fn send_system(&mut self, status: u8, spp: u16) {
        let at_ns = self.stamp_for(true);
        let _ = self.midi_out.push(MidiOutEvent {
            at_ns,
            system: status,
            spp,
            ..MidiOutEvent::default()
        });
    }

    /// MIDI clock, once per frame while it is on: 24 ticks a quarter note on the beat, Start
    /// or Continue (with the song position) when the transport starts, Stop when it stops, and
    /// Stop + position + Continue when the playhead jumps (a cycle, a locate).
    fn tick_midi_clock(&mut self) {
        if !self.midi_clock {
            if self.clock_running {
                self.clock_running = false;
                self.send_system(0xFC, 0);
            }
            return;
        }
        let beat = self.beat;
        match (self.playing, self.clock_running) {
            (true, false) => {
                let spp = song_position(beat);
                self.send_system(0xF2, spp);
                self.send_system(if spp == 0 { 0xFA } else { 0xFB }, 0);
                self.clock_running = true;
                // The tick on the playhead (or the next one) is the first.
                self.next_tick = (beat * 24.0).ceil();
            }
            (false, true) => {
                self.send_system(0xFC, 0);
                self.clock_running = false;
            }
            (true, true) => {
                let jumped =
                    beat * 24.0 < self.next_tick - 1.5 || beat * 24.0 > self.next_tick + 2.0;
                if jumped {
                    let spp = song_position(beat);
                    self.send_system(0xFC, 0);
                    self.send_system(0xF2, spp);
                    self.send_system(0xFB, 0);
                    self.next_tick = (beat * 24.0).ceil();
                }
                if beat * 24.0 >= self.next_tick - 1e-9 {
                    self.send_system(0xF8, 0);
                    self.next_tick += 1.0;
                }
            }
            (false, false) => {}
        }
    }

    /// Sends a controller change to the external instrument of its track (once; the sustain
    /// pedal is remembered so a stop can let it up).
    fn send_control(&mut self, c: ControlEvent) {
        if let Some(Some(p)) = self.plugins.get_mut(c.track as usize) {
            if p.has_instrument() {
                // Kinds above 127 (see `ControlEvent::number`): 128 pitch bend, 129 channel
                // pressure; System Exclusive bytes (130) do not go to a plug-in.
                let at = self.frame_i as u32;
                let v = c.value;
                match c.number {
                    128 => {
                        p.push_midi_timeline(at, 0xe0, (v & 0x7f) as u8, ((v >> 7) & 0x7f) as u8)
                    }
                    129 => p.push_midi_timeline(at, 0xd0, v.min(127) as u8, 0),
                    n if n < 128 => p.push_midi_timeline(at, 0xb0, n, v.min(127) as u8),
                    _ => {}
                }
                return;
            }
        }
        if !self
            .track_external
            .get(c.track as usize)
            .copied()
            .unwrap_or(false)
        {
            self.internal_control(c);
            return;
        }
        if c.number == 64 {
            if let Some(down) = self.ext_sustain.get_mut(c.track as usize) {
                *down = c.value >= 64;
            }
        }
        let at_ns = self.stamp_for(true);
        let _ = self.midi_out.push(MidiOutEvent {
            track: c.track,
            at_ns,
            on: false,
            pitch: c.number,
            vel: c.value.min(127) as u8,
            controller: true,
            value: c.value,
            ..MidiOutEvent::default()
        });
    }

    /// A controller change for the engine's own instruments on its track: the synth's state
    /// (its voices take a new pitch at this very sample) and the SoundFont player's.
    fn internal_control(&mut self, c: ControlEvent) {
        let t = c.track as usize;
        let Some(ctl) = self.midi_ctl.get_mut(t) else {
            return;
        };
        if ctl.apply(c.number, c.value) {
            for v in self
                .voices
                .iter_mut()
                .filter(|v| v.active && v.track == c.track)
            {
                v.ctrl_left = 0;
            }
        }
        if let Some(Some(player)) = self.sf.get_mut(t) {
            let v = c.value;
            match c.number {
                128 => player.midi(0xe0, (v & 0x7f) as u8, ((v >> 7) & 0x7f) as u8),
                129 => player.midi(0xd0, v.min(127) as u8, 0),
                n if n < 128 => player.midi(0xb0, n, v.min(127) as u8),
                _ => {}
            }
        }
    }

    /// The controllers of the engine's own instruments as the song has them at the playhead:
    /// every event before it, in order (after a jump; no allocation).
    fn chase_controls(&mut self) {
        for c in self.midi_ctl.iter_mut() {
            *c = MidiCtl::default();
        }
        for k in 0..self.next_control.min(self.song.controls.len()) {
            let c = self.song.controls[k];
            let external = self
                .track_external
                .get(c.track as usize)
                .copied()
                .unwrap_or(false);
            if !external {
                self.internal_control(c);
            }
        }
    }

    /// The controllers the engine's own synth has on `track` (for tests and meters).
    pub fn midi_ctl(&self, track: u16) -> MidiCtl {
        self.midi_ctl
            .get(track as usize)
            .copied()
            .unwrap_or_default()
    }

    /// Lets up the sustain pedal of every external instrument it is down on.
    fn release_ext_sustain(&mut self) {
        for track in 0..self.ext_sustain.len() {
            if std::mem::replace(&mut self.ext_sustain[track], false) {
                let _ = self.midi_out.push(MidiOutEvent {
                    track: track as u16,
                    on: false,
                    pitch: 64,
                    vel: 0,
                    controller: true,
                    value: 0,
                    ..MidiOutEvent::default()
                });
            }
        }
    }

    /// Silences timeline voices (seek, pause). Live notes keep playing until `NoteOff`.
    fn kill_voices(&mut self) {
        self.release_ext_sustain();
        for v in self.voices.iter_mut().filter(|v| !v.live) {
            v.active = false;
        }
        for n in self.sf_notes.iter_mut().filter(|n| n.active && !n.live) {
            n.released = true;
        }
        // The external instruments' timeline notes stop as well (live ones are kept).
        let mut i = 0;
        while i < self.ext_notes.len() {
            if self.ext_notes[i].live {
                i += 1;
            } else {
                let e = self.ext_notes.swap_remove(i);
                self.send_midi(e.track, false, e.pitch, 0, true);
            }
        }
    }

    fn seek(&mut self, beat: f64) {
        self.beat = if beat.is_finite() { beat.max(0.0) } else { 0.0 };
        self.wrap_beat = f64::NAN;
        self.kill_voices();
        self.next_note = self
            .song
            .notes
            .partition_point(|n| n.start_beat < self.beat);
        self.next_control = self.song.controls.partition_point(|c| c.beat < self.beat);
        self.chase_controls();
        self.reposition_clips();
        self.mtc_locate();
    }

    fn reposition_clips(&mut self) {
        // Clips play in real time, so their offset is the time since the region start.
        let map = &self.tempo_map;
        let now = map.seconds_at(self.beat);
        for c in self.song.clips.iter_mut() {
            let start = map.seconds_at(c.start_beat);
            // Refresh on song/map changes and seeks. The region's end is constant
            // between these events, including while transport is stopped.
            c.end_beat = map.beat_at(start + c.len_frames as f64 / self.sr as f64);
            let since = now - start;
            c.reposition((since * self.sr as f64).round() as i64);
        }
    }

    fn smoothing_samples(&self) -> u32 {
        let sounding = self.playing
            || self.voices.iter().any(|v| v.active)
            || self.click_remaining > 0
            || self.counting
            || (self.test_hz > 0.0 && self.test_gain > 0.0);
        if sounding {
            (self.sr * 0.005).round().max(1.0) as u32
        } else {
            0
        }
    }

    fn rebuild_mix(&mut self, smooth: bool, changed_track: Option<usize>) {
        let samples = if smooth { self.smoothing_samples() } else { 0 };
        self.mix_ramp_remaining = samples;
        let any_solo = self.song.tracks.iter().any(|t| t.solo);
        for i in 0..MAX_TRACKS {
            match self.song.tracks.get(i) {
                Some(t) => {
                    // Solo affects every channel, but another channel's gain/pan
                    // target is unchanged. Preserve full immediate resets when silent.
                    // An Aux strip stays audible under solo: it carries the sends of the
                    // soloed strips (the others have no signal to send).
                    let aux = self.routes[i].bus_in > 0;
                    self.track_audible[i] = !t.mute && (!any_solo || t.solo || aux);
                    if samples > 0 && changed_track.is_some_and(|track| track != i) {
                        continue;
                    }
                    let t = t.sanitized();
                    let db = if self.auto_mute[i] {
                        -400.0
                    } else {
                        self.auto_vol[i].unwrap_or(t.gain_db)
                    };
                    self.set_strip_gain(i, db, self.auto_pan[i].unwrap_or(t.pan), samples);
                }
                None => {
                    self.track_ramp_l[i].set(1.0, 0);
                    self.track_ramp_r[i].set(1.0, 0);
                    self.track_audible[i] = true;
                }
            }
            if samples == 0 {
                self.track_gain_l[i] = self.track_ramp_l[i].next();
                self.track_gain_r[i] = self.track_ramp_r[i].next();
            }
        }
    }

    /// Aims strip `i`'s left and right gains at a fader (dB) and pan, over `samples`.
    fn set_strip_gain(&mut self, i: usize, gain_db: f32, pan: f32, samples: u32) {
        let g = 10f32.powf(gain_db / 20.0);
        // Equal-power pan law, normalised so the centre is unity gain.
        let a = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        self.track_ramp_l[i].set(g * a.cos() * std::f32::consts::SQRT_2, samples);
        self.track_ramp_r[i].set(g * a.sin() * std::f32::consts::SQRT_2, samples);
    }

    /// Sorts the strips into plain, complex (effects or sends) and Aux after any of them changed.
    fn sort_strips(&mut self) {
        self.complex.clear();
        self.aux.clear();
        for i in 0..MAX_TRACKS {
            let has_chain = self.chains[i].as_ref().is_some_and(|c| !c.fx.is_empty())
                || self.plugins[i].is_some();
            let has_pdc = self.pdc[i].as_ref().is_some_and(|p| !p.is_empty());
            let to_bus = self.routes[i].output > 0;
            self.strip_kind[i] = if self.routes[i].bus_in > 0 {
                self.aux.push(i as u16);
                2
            } else if has_chain || has_pdc || to_bus || self.routes[i].has_sends() {
                self.complex.push(i as u16);
                1
            } else {
                0
            };
        }
    }

    /// Reads the automation lanes at the playhead and aims the strips they move.
    ///
    /// Faders, pans, sends and mutes are read where the playhead will be `AUTO_BLOCK` frames
    /// from now and ramp there, so they are on the lane at every reading; effect and plug-in
    /// parameters are read where it is and take effect at this frame (`frame`).
    fn update_automation(&mut self, frame: usize) {
        let beat = self.beat;
        // Faders, pans, sends and mutes act on the sound after the delay compensation, so
        // they are read that much late; the Stereo Out after the whole output delay.
        let ahead = self.beat_ahead(self.output_delay.0 as usize);
        let ahead_master = self.beat_ahead(self.output_delay.1 as usize);
        let master_beat = self.beat_ago(self.output_delay.1 as usize);
        let samples = AUTO_BLOCK;
        let mut master_db = None;
        for k in 0..self.automation.len().min(self.auto_last.len()) {
            let lane = &self.automation[k];
            if !lane.follow {
                continue;
            }
            let t = lane.track as usize;
            let ramped = matches!(
                lane.target,
                AutoTarget::Volume | AutoTarget::Pan | AutoTarget::Mute | AutoTarget::Send { .. }
            );
            // The Stereo Out's effects and plug-ins hear the mix after the whole output delay
            // too, so their parameters are read that much late as well.
            let at = match (ramped, lane.track == MASTER_TRACK) {
                (false, false) => beat,
                (false, true) => master_beat,
                (true, false) => ahead,
                (true, true) => ahead_master,
            };
            let Some(v) = lane.value_at(at) else {
                continue;
            };
            if lane.track == MASTER_TRACK {
                match lane.target {
                    AutoTarget::Volume => master_db = Some(v.clamp(-96.0, 6.0)),
                    AutoTarget::Fx { slot, param } => {
                        if let Some(chain) = self.master_chain.as_mut() {
                            chain.set_param_norm(slot as usize, param as usize, v);
                        }
                    }
                    // The master plug-ins run after this block's frames: the change lands at
                    // this frame of it all the same.
                    AutoTarget::Plugin { slot, param } => {
                        let value = lane.plugin_value(v);
                        if self.auto_last[k] != value {
                            if let Some(p) = self.master_plugins.as_mut() {
                                p.push_param(frame as u32, slot, param, value);
                                self.auto_last[k] = value;
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if t >= MAX_TRACKS {
                continue;
            }
            match lane.target {
                AutoTarget::Volume => {
                    self.auto_vol[t] = Some((v + lane.offset_db).clamp(-96.0, 6.0))
                }
                AutoTarget::Pan => self.auto_pan[t] = Some(v.clamp(-1.0, 1.0)),
                AutoTarget::Mute => self.auto_mute[t] = v >= 0.5,
                AutoTarget::Send { slot } => {
                    if let Some(r) = self.auto_send[t].get_mut(slot as usize) {
                        r.set(send_gain(v), samples);
                    }
                }
                AutoTarget::Fx { slot, param } => {
                    if let Some(chain) = self.chains[t].as_mut() {
                        chain.set_param_norm(slot as usize, param as usize, v);
                    }
                }
                AutoTarget::Plugin { slot, param } => {
                    let value = lane.plugin_value(v);
                    if self.auto_last[k] != value {
                        if let Some(p) = self.plugins[t].as_mut() {
                            p.push_param_timeline(frame as u32, slot, param, value);
                            self.auto_last[k] = value;
                        }
                    }
                }
            }
        }
        if let Some(db) = master_db {
            self.master
                .set(10f32.powf(db / 20.0).clamp(0.0, 2.0), samples);
        }
        for k in 0..self.auto_tracks.len() {
            let t = self.auto_tracks[k] as usize;
            let own = self.song.tracks.get(t).copied().unwrap_or_default();
            // A muted strip ramps to silence instead of switching off with a click.
            let db = if self.auto_mute[t] {
                -400.0
            } else {
                self.auto_vol[t].unwrap_or(own.gain_db)
            };
            self.set_strip_gain(t, db, self.auto_pan[t].unwrap_or(own.pan), samples);
        }
    }

    /// Runs one strip: effects, delay compensation, pre-fader sends, fader and pan,
    /// post-fader sends, and into the mix.
    #[inline]
    fn run_strip(&mut self, t: usize, out: &mut (f64, f64)) {
        let route = self.routes[t];
        let audible = self.track_audible[t];
        let (mut xl, mut xr) = (self.in_l[t], self.in_r[t]);
        self.in_l[t] = 0.0;
        self.in_r[t] = 0.0;
        if route.bus_in > 0 {
            let b = route.bus_in as usize - 1;
            xl += self.bus_l[b];
            xr += self.bus_r[b];
        }
        if !audible {
            xl = 0.0;
            xr = 0.0;
        }
        // The effects work in 32 bits.
        let (mut xl, mut xr) = (xl as f32, xr as f32);
        if let Some(chain) = self.chains[t].as_mut() {
            (xl, xr) = chain.process(xl, xr);
        }
        if let Some(p) = self.plugins[t].as_mut() {
            if p.has_effects() {
                (xl, xr) = p.through_effects(self.frame_i, xl, xr);
            }
        }
        if let Some(pdc) = self.pdc[t].as_mut() {
            (xl, xr) = pdc.delay_before(xl, xr);
        }
        let (xl, xr) = (xl as f64, xr as f64);
        let on = self.auto_send_on[t];
        let level = |k: usize, s: &crate::routing::SendSpec, ramps: &[GainRamp; MAX_SENDS]| {
            if on[k] {
                ramps[k].current() as f64
            } else {
                send_gain(s.level) as f64
            }
        };
        if audible && route.bus_in == 0 && !self.auto_mute[t] {
            for (k, s) in route.sends.iter().enumerate() {
                if s.bus > 0 && s.pre {
                    let g = level(k, s, &self.auto_send[t]);
                    self.bus_l[s.bus as usize - 1] += xl * g;
                    self.bus_r[s.bus as usize - 1] += xr * g;
                }
            }
        }
        let (yl, yr) = (
            xl * self.track_gain_l[t] as f64,
            xr * self.track_gain_r[t] as f64,
        );
        if audible {
            if route.bus_in == 0 {
                for (k, s) in route.sends.iter().enumerate() {
                    if s.bus > 0 && !s.pre {
                        let g = level(k, s, &self.auto_send[t]);
                        self.bus_l[s.bus as usize - 1] += yl * g;
                        self.bus_r[s.bus as usize - 1] += yr * g;
                    }
                }
            }
            self.track_pk_l[t] = self.track_pk_l[t].max(yl.abs() as f32);
            self.track_pk_r[t] = self.track_pk_r[t].max(yr.abs() as f32);
        }
        // The compensation after the fader keeps running while the strip is silent, so its
        // timing is right the moment it is heard.
        let (yl, yr) = match self.pdc[t].as_mut() {
            Some(pdc) => {
                let (a, b) = pdc.delay_after(yl as f32, yr as f32);
                (a as f64, b as f64)
            }
            None => (yl, yr),
        };
        if audible {
            if route.output > 0 {
                self.bus_l[route.output as usize - 1] += yl;
                self.bus_r[route.output as usize - 1] += yr;
            } else {
                out.0 += yl;
                out.1 += yr;
            }
        }
    }

    /// The click's pitch on beat `beat_in_bar` (0: the downbeat, accented when asked).
    fn click_freq_of(&self, beat_in_bar: u32) -> f32 {
        if beat_in_bar == 0 && self.click_accent {
            1500.0
        } else {
            1000.0
        }
    }

    /// The click of the transport starting at `beat`: one when `beat` is the first frame of a
    /// beat (as if the transport had run into it), with that beat's pitch.
    fn click_on(&self, beat: f64) -> Option<f32> {
        let b = self.tempo_map.position(beat);
        if beat <= 0.0 {
            return Some(self.click_freq_of(b.beat));
        }
        let before = self.tempo_map.advance(beat, -1.0 / f64::from(self.sr));
        let a = self.tempo_map.position(before);
        ((a.bar, a.beat) != (b.bar, b.beat)).then(|| self.click_freq_of(b.beat))
    }

    /// A click of `freq` that starts with this frame's sound: heard after the output delay
    /// (see [`Command::SetOutputDelay`]), like every track, so it lands on the same sample as
    /// a region's beat. No allocation.
    fn schedule_click(&mut self, freq: f32) {
        let wait = self.output_delay.1;
        if wait == 0 {
            self.start_click_sound(freq);
        } else if self.click_queued < CLICK_QUEUE {
            self.click_queue[self.click_queued] = (wait, freq);
            self.click_queued += 1;
        }
    }

    /// Clicks whose wait is over start sounding; the others wait one frame less.
    fn release_clicks(&mut self) {
        let mut k = 0;
        while k < self.click_queued {
            let (left, freq) = self.click_queue[k];
            if left == 0 {
                self.start_click_sound(freq);
                self.click_queued -= 1;
                self.click_queue[k] = self.click_queue[self.click_queued];
            } else {
                self.click_queue[k].0 = left - 1;
                k += 1;
            }
        }
    }

    fn start_click_sound(&mut self, freq: f32) {
        self.click_remaining = (0.03 * self.sr) as u32;
        self.click_phase = 0.0;
        self.click_freq = freq;
    }

    /// One frame of a count-in: a click on each beat, then the transport starts.
    fn tick_count_in(&mut self) {
        if self.count_wait == 0 {
            if self.count_left == 0 {
                self.counting = false;
                self.start_beat = self.beat;
                self.playing = true;
                self.start_click = true;
                self.mtc_locate();
                return;
            }
            self.schedule_click(1000.0);
            self.count_left -= 1;
            self.count_wait = self.count_step;
        }
        self.count_wait -= 1;
    }

    /// One frame of the live input into the strips of the tracks that monitor it.
    fn hear_input(&mut self) {
        let Some(feed) = self.input.as_mut() else {
            return;
        };
        feed.advance();
        for m in &self.monitors[..self.monitor_count] {
            let t = m.track as usize;
            if t >= MAX_TRACKS || !self.track_audible[t] {
                continue;
            }
            let l = feed.value(m.channel as usize);
            let r = if m.stereo {
                feed.value(m.channel as usize + 1)
            } else {
                l
            };
            self.in_l[t] += l as f64;
            self.in_r[t] += r as f64;
            if !self.touched[t] {
                self.touched[t] = true;
                self.dirty.push(t as u16);
            }
        }
    }

    /// The strips' signals of this sample, through their effects, faders and sends, summed.
    fn mix_strips(&mut self) -> (f64, f64) {
        let mut out = (0f64, 0f64);
        for k in 0..self.dirty.len() {
            let t = self.dirty[k] as usize;
            self.touched[t] = false;
            if self.strip_kind[t] != 0 {
                continue;
            }
            let (yl, yr) = (
                self.in_l[t] * self.track_gain_l[t] as f64,
                self.in_r[t] * self.track_gain_r[t] as f64,
            );
            self.in_l[t] = 0.0;
            self.in_r[t] = 0.0;
            out.0 += yl;
            out.1 += yr;
            self.track_pk_l[t] = self.track_pk_l[t].max(yl.abs() as f32);
            self.track_pk_r[t] = self.track_pk_r[t].max(yr.abs() as f32);
        }
        self.dirty.clear();
        for k in 0..self.complex.len() {
            self.run_strip(self.complex[k] as usize, &mut out);
        }
        for k in 0..self.aux.len() {
            self.run_strip(self.aux[k] as usize, &mut out);
        }
        if !self.aux.is_empty() || !self.complex.is_empty() {
            self.bus_l = [0.0; MAX_BUSES];
            self.bus_r = [0.0; MAX_BUSES];
        }
        out
    }

    fn drain_commands(&mut self) {
        while let Ok(next) = self.rx.peek() {
            // Leave owning commands in the queue until their old state can be returned.
            // This is the only garbage producer; the consumer can only free more slots,
            // so the single push below is guaranteed to fit after this check.
            if next.retires_state() && self.trash.is_full() {
                break;
            }
            let Ok(cmd) = self.rx.pop() else { break };
            match cmd {
                Command::CountIn {
                    clicks,
                    beats_per_click,
                } => {
                    if self.playing {
                        continue;
                    }
                    let span = if beats_per_click.is_finite() && beats_per_click > 0.0 {
                        self.tempo_map.seconds_at(self.beat + beats_per_click)
                            - self.tempo_map.seconds_at(self.beat)
                    } else {
                        0.0
                    };
                    self.count_step = ((span * self.sr as f64).round() as u32).max(1);
                    self.count_left = clicks;
                    self.count_wait = 0;
                    self.counting = true;
                }
                Command::SetInput(feed) => {
                    let mut feed = feed;
                    if let Some(f) = feed.as_mut() {
                        f.set_output_rate(self.sr);
                    }
                    if let Some(old) = std::mem::replace(&mut self.input, feed) {
                        let _ = self.trash.push(Garbage::Input(old));
                    }
                }
                Command::SetPlugins { track, plugins } => {
                    let t = track as usize;
                    if t < MAX_TRACKS {
                        // Notes still out to the instrument before are let go.
                        let mut i = 0;
                        while i < self.ext_notes.len() {
                            if self.ext_notes[i].track == track {
                                self.ext_notes.swap_remove(i);
                            } else {
                                i += 1;
                            }
                        }
                        if let Some(old) = std::mem::replace(&mut self.plugins[t], plugins) {
                            // Plug-ins on the playback thread are freed there.
                            if old.is_away()
                                && !old.lane_home()
                                && self.retire.len() < self.retire.capacity()
                            {
                                self.retire.push((old.worker(), old.lane_id()));
                            }
                            let _ = self.trash.push(Garbage::Plugins(old));
                        }
                        let delay = self.content_delay[t];
                        if let Some(p) = self.plugins[t].as_mut() {
                            p.set_track(track);
                            p.set_content_delay(delay);
                        }
                        self.plugin_tracks.retain(|x| *x != track);
                        if self.plugins[t].is_some() {
                            self.plugin_tracks.push(track);
                        }
                        // New instances learn their automated values at the next reading.
                        for (lane, last) in self.automation.iter().zip(self.auto_last.iter_mut()) {
                            if lane.track == track {
                                *last = f32::NAN;
                            }
                        }
                        self.sort_strips();
                    } else if let Some(p) = plugins {
                        let _ = self.trash.push(Garbage::Plugins(p));
                    }
                }
                Command::SetMasterPlugins(plugins) => {
                    if let Some(old) = std::mem::replace(&mut self.master_plugins, plugins) {
                        let _ = self.trash.push(Garbage::Plugins(old));
                    }
                    // Named in the heaviest plug-in report as the Stereo Out's.
                    if let Some(p) = self.master_plugins.as_mut() {
                        p.set_track(MASTER_TRACK);
                    }
                    // New instances learn their automated values at the next reading.
                    for (lane, last) in self.automation.iter().zip(self.auto_last.iter_mut()) {
                        if lane.track == MASTER_TRACK {
                            *last = f32::NAN;
                        }
                    }
                    self.master_plugins_changed();
                }
                Command::SetMasterPluginsOff(mask) => {
                    if let Some(p) = self.master_plugins.as_mut() {
                        p.set_effects_off(mask);
                    }
                    self.master_plugins_changed();
                }
                Command::SetMidiClock(on) => self.midi_clock = on,
                Command::SetPluginParam {
                    track,
                    effect,
                    index,
                    value,
                } => {
                    let p = if track == MASTER_TRACK {
                        self.master_plugins.as_mut()
                    } else {
                        self.plugins
                            .get_mut(track as usize)
                            .and_then(|p| p.as_mut())
                    };
                    if let Some(p) = p {
                        p.set_effect_param(usize::from(effect), usize::from(index), value);
                    }
                }
                Command::SetMonitors { tracks, count } => {
                    self.monitors = tracks;
                    self.monitor_count = (count as usize).min(MAX_MONITORS);
                }
                Command::Play => {
                    self.counting = false;
                    if !self.playing {
                        self.start_beat = self.beat;
                        self.start_click = true;
                    }
                    self.playing = true;
                    self.mtc_locate();
                }
                Command::Pause => {
                    self.counting = false;
                    self.playing = false;
                    self.kill_voices();
                    self.mtc_locate();
                    self.flush_ahead();
                }
                Command::Stop => {
                    self.counting = false;
                    self.playing = false;
                    let b = self.start_beat;
                    self.seek(b);
                    self.flush_ahead();
                }
                Command::SetPosition { beat } => {
                    self.seek(beat);
                    if self.playing {
                        self.flush_ahead();
                        self.start_click = true;
                    }
                }
                Command::SetTempoMap(map) => {
                    let old = std::mem::replace(&mut self.tempo_map, map);
                    let _ = self.trash.push(Garbage::TempoMap(old));
                    self.reposition_clips();
                    // A tempo edit after the playhead leaves the time where it was.
                    if (self.tempo_map.seconds_at(self.beat) - self.mtc_secs).abs() > 0.001 {
                        self.mtc_locate();
                    }
                }
                Command::SetMtc {
                    on,
                    rate,
                    offset_frames,
                } => {
                    self.mtc.configure(on, rate);
                    self.mtc_rate = rate;
                    self.mtc_offset_frames = offset_frames;
                    self.mtc_locate();
                }
                Command::SetMetronome { on } => self.metronome = on,
                Command::SetClick { gain_db, accent } => {
                    if gain_db.is_finite() {
                        self.click_gain = 10f32.powf(gain_db.clamp(-60.0, 0.0) / 20.0);
                    }
                    self.click_accent = accent;
                }
                Command::SetTuning { a4_hz } => {
                    if a4_hz.is_finite() {
                        self.tuning_hz = a4_hz.clamp(MIN_TUNING_HZ, MAX_TUNING_HZ);
                        let hz = self.tuning_hz;
                        for player in self.sf.iter_mut().flatten() {
                            player.set_tuning(hz);
                        }
                    }
                }
                Command::SetMasterGain { linear } => {
                    if linear.is_finite() {
                        self.master_user = linear.clamp(0.0, 2.0);
                        // While the Stereo Out lane plays, it rules the master fader.
                        if !self.auto_master {
                            let samples = self.smoothing_samples();
                            self.master.set(self.master_user, samples);
                        }
                    }
                }
                Command::SetCycle {
                    on,
                    start_beat,
                    end_beat,
                } => {
                    self.cycle = (on
                        && start_beat.is_finite()
                        && end_beat.is_finite()
                        && start_beat >= 0.0
                        && end_beat > start_beat)
                        .then_some((start_beat, end_beat));
                }
                Command::SetTrackMix { track, mix } => {
                    if let Some(t) = self.song.tracks.get_mut(track as usize) {
                        *t = mix.sanitized();
                        self.rebuild_mix(true, Some(track as usize));
                    }
                }
                Command::NoteOn { pitch, vel, track } => {
                    self.live_note_on(pitch.min(127), vel.clamp(1, 127), track)
                }
                Command::NoteOff { pitch } => self.live_note_off(pitch, None),
                Command::NoteOffTrack { pitch, track } => self.live_note_off(pitch, Some(track)),
                Command::SetTrackExternal { track, external } => {
                    if let Some(slot) = self.track_external.get_mut(track as usize) {
                        let was = std::mem::replace(slot, external);
                        // Notes already out to the instrument end when the track stops using it.
                        if was && !external {
                            let mut i = 0;
                            while i < self.ext_notes.len() {
                                if self.ext_notes[i].track == track {
                                    let e = self.ext_notes.swap_remove(i);
                                    self.send_midi(e.track, false, e.pitch, 0, !e.live);
                                } else {
                                    i += 1;
                                }
                            }
                        }
                    }
                }
                Command::SetTrackPatch { track, patch } => {
                    if let Some(slot) = self.track_patch.get_mut(track as usize) {
                        *slot = patch.sanitized();
                    }
                }
                Command::SetTrackSoundFont { track, player } => {
                    let t = track as usize;
                    if t < MAX_TRACKS {
                        self.sf_stop_track(track);
                        let mut player = player;
                        if let Some(p) = player.as_mut() {
                            p.set_tuning(self.tuning_hz);
                        }
                        let old = std::mem::replace(&mut self.sf[t], player);
                        if let Some(mut old) = old {
                            old.all_notes_off();
                            let _ = self.trash.push(Garbage::SoundFont(old));
                        }
                        self.sf_tracks.retain(|&x| x as usize != t);
                        if self.sf[t].is_some() {
                            self.sf_tracks.push(track);
                        }
                    } else if let Some(p) = player {
                        let _ = self.trash.push(Garbage::SoundFont(p));
                    }
                }
                Command::SetInserts { track, chain } => {
                    let t = track as usize;
                    if t < MAX_TRACKS {
                        let old = self.chains[t].replace(chain);
                        if let Some(old) = old {
                            let _ = self.trash.push(Garbage::Chain(old));
                        }
                        self.shown.gain_reduction[t] = 0.0;
                        self.sort_strips();
                    } else {
                        let _ = self.trash.push(Garbage::Chain(chain));
                    }
                }
                Command::SetPdc { track, pdc } => {
                    let t = track as usize;
                    if t < MAX_TRACKS {
                        if let Some(old) = self.pdc[t].replace(pdc) {
                            let _ = self.trash.push(Garbage::Pdc(old));
                        }
                        self.sort_strips();
                    } else {
                        let _ = self.trash.push(Garbage::Pdc(pdc));
                    }
                }
                Command::SetMasterInserts(chain) => {
                    if let Some(old) = self.master_chain.replace(chain) {
                        let _ = self.trash.push(Garbage::Chain(old));
                    }
                }
                Command::SetConvolver { track, slot, conv } => {
                    let chain = if track == MASTER_TRACK {
                        self.master_chain.as_mut()
                    } else {
                        self.chains.get_mut(track as usize).and_then(|c| c.as_mut())
                    };
                    let fx = chain.and_then(|c| c.fx.get_mut(slot as usize));
                    let old = match fx {
                        Some(fx) => fx.set_convolver(conv),
                        None => conv,
                    };
                    let _ = self.trash.push(Garbage::Convolver(old));
                }
                Command::SetFx {
                    track,
                    slot,
                    params,
                    bypass,
                } => {
                    let chain = if track == MASTER_TRACK {
                        self.master_chain.as_mut()
                    } else {
                        self.chains.get_mut(track as usize).and_then(|c| c.as_mut())
                    };
                    if let Some(fx) = chain.and_then(|c| c.fx.get_mut(slot as usize)) {
                        fx.set(params, bypass);
                    }
                }
                Command::SetRoute { track, route } => {
                    let t = track as usize;
                    if t < MAX_TRACKS {
                        self.routes[t] = route.sanitized();
                        self.sort_strips();
                        self.rebuild_mix(true, Some(t));
                    }
                }
                Command::SetAutomation(lanes) => {
                    let old = std::mem::replace(&mut self.automation, lanes);
                    let _ = self.trash.push(Garbage::Automation(old));
                    self.auto_vol.fill(None);
                    self.auto_pan.fill(None);
                    self.auto_on.fill(false);
                    self.auto_mute.fill(false);
                    self.auto_tracks.clear();
                    // Effect parameters go back to what the user set; the lanes that are
                    // played move them again from the next block.
                    for chain in self.chains.iter_mut().flatten() {
                        chain.restore();
                    }
                    if let Some(chain) = self.master_chain.as_mut() {
                        chain.restore();
                    }
                    // Within the room made up front: no allocation.
                    self.auto_last.clear();
                    let n = self.automation.len().min(self.auto_last.capacity());
                    self.auto_last.resize(n, f32::NAN);
                    self.auto_any = self.automation.iter().any(|l| l.follow);
                    let was_master = self.auto_master;
                    self.auto_master = false;
                    // Sends that keep a lane carry on from where they are.
                    for t in 0..MAX_TRACKS {
                        let was = std::mem::replace(&mut self.auto_send_on[t], [false; MAX_SENDS]);
                        let sends = self.routes[t].sends;
                        for ((on, ramp), spec) in was
                            .iter()
                            .zip(self.auto_send[t].iter_mut())
                            .zip(sends.iter())
                        {
                            if !on {
                                ramp.set(send_gain(spec.level), 0);
                            }
                        }
                    }
                    for k in 0..n {
                        let lane = &self.automation[k];
                        if !lane.follow {
                            continue;
                        }
                        if lane.track == MASTER_TRACK {
                            self.auto_master |= lane.target == AutoTarget::Volume;
                            continue;
                        }
                        let t = lane.track as usize;
                        if t >= MAX_TRACKS
                            || matches!(
                                lane.target,
                                AutoTarget::Fx { .. } | AutoTarget::Plugin { .. }
                            )
                        {
                            continue;
                        }
                        if let AutoTarget::Send { slot } = lane.target {
                            if let Some(on) = self.auto_send_on[t].get_mut(slot as usize) {
                                *on = true;
                            }
                        }
                        if !self.auto_on[t] {
                            self.auto_on[t] = true;
                            self.auto_tracks.push(lane.track);
                        }
                    }
                    if was_master && !self.auto_master {
                        let samples = self.smoothing_samples();
                        self.master.set(self.master_user, samples);
                    }
                    self.auto_left = 0;
                    self.rebuild_mix(true, None);
                }
                Command::SetTestTone { hz, gain } => {
                    self.test_hz = if hz.is_finite() {
                        hz.clamp(0.0, self.sr * 0.45)
                    } else {
                        0.0
                    };
                    self.test_gain = if gain.is_finite() {
                        gain.clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                }
                Command::SetSong(new) => {
                    let old = std::mem::replace(&mut self.song, new);
                    let _ = self.trash.push(Garbage::Song(old));
                    self.rebuild_mix(false, None);
                    let b = self.beat;
                    self.seek(b);
                }
                Command::SetControls(controls) => {
                    let old = std::mem::replace(&mut self.song.controls, controls);
                    let _ = self.trash.push(Garbage::Controls(old));
                    self.next_control = self.song.controls.partition_point(|c| c.beat < self.beat);
                    // Controllers taken out of the song do not stay applied.
                    self.chase_controls();
                }
                Command::SetProcessBuffer { frames } => self.set_process_buffer(frames),
                Command::SetTrackLive { track, live } => {
                    if let Some(slot) = self.track_live.get_mut(track as usize) {
                        *slot = live;
                    }
                }
                Command::SetOutputDelay { strips, master } => {
                    let most = (BEAT_HISTORY - AUTO_BLOCK as usize) as u32;
                    self.output_delay = (strips.min(most), master.min(most));
                }
                Command::SetContentDelay { track, frames } => self.set_content_delay(track, frames),
                Command::SetNotes(notes) => {
                    let old = std::mem::replace(&mut self.song.notes, notes);
                    let _ = self.trash.push(Garbage::Notes(old));
                    self.next_note = self
                        .song
                        .notes
                        .partition_point(|n| n.start_beat < self.beat);
                }
            }
        }
    }

    fn trigger(&mut self, n: NoteEvent) -> usize {
        if self.plays_elsewhere(n.track) {
            // An external instrument makes its own sound.
            self.ext_note_on(n, false);
            return 0;
        }
        let sf_track = n.track.min(MAX_TRACKS as u16 - 1);
        if self.sf.get(sf_track as usize).is_some_and(Option::is_some) {
            self.sf_note_on(sf_track, n.pitch, n.vel, n.end_beat, false);
            return 0;
        }
        // No free voice: steal the first one.
        let idx = self
            .voices
            .iter()
            .position(|v| !v.active)
            .unwrap_or_default();
        let track = n.track.min(MAX_TRACKS as u16 - 1);
        let mut patch = self.track_patch[track as usize];
        let mut base_hz = midi_hz(n.pitch, self.tuning_hz);
        if patch.kit != Kit::None {
            (patch, base_hz) = kit_voice(&patch, n.pitch);
        }
        self.voices[idx] = Voice {
            active: true,
            releasing: false,
            phase_a: 0.0,
            phase_b: 0.0,
            base_hz,
            amp: (n.vel as f32 / 127.0) * 0.18 * patch.gain,
            env: 0.0,
            stage: 0,
            rel_step: 0.0,
            end_beat: n.end_beat,
            track,
            live: false,
            pitch: n.pitch,
            live_left: 0,
            patch,
            filter: Svf::default(),
            vibrato: 0.0,
            noise: 0x9E37_79B9 ^ (n.pitch as u32).wrapping_mul(2_654_435_761) | 1,
            pitch_env: patch.pitch_drop,
            ratio_b: 2f32.powf((patch.semi_b + patch.detune_b / 100.0) / 12.0),
            pitch_decay_block: (-(CTRL_BLOCK as f32) / (patch.pitch_decay.max(0.001) * self.sr))
                .exp(),
            ctrl_left: 0,
            dt_a: 0.0,
            dt_b: 0.0,
            filter_f: 0.0,
            filter_damping: Svf::damping(patch.resonance),
            attack_step: 1.0 / (patch.attack.max(0.001) * self.sr),
            decay_step: (1.0 - patch.sustain).max(0.0) / (patch.decay.max(0.001) * self.sr),
        };
        idx
    }

    fn live_note_on(&mut self, pitch: u8, vel: u8, track: u16) {
        if self.plays_elsewhere(track) {
            self.ext_note_on(
                NoteEvent {
                    start_beat: 0.0,
                    end_beat: f64::MAX,
                    pitch,
                    vel,
                    track,
                },
                true,
            );
            return;
        }
        let sf_track = track.min(MAX_TRACKS as u16 - 1);
        if self.sf.get(sf_track as usize).is_some_and(Option::is_some) {
            self.sf_note_on(sf_track, pitch, vel, f64::MAX, true);
            return;
        }
        let idx = self.trigger(NoteEvent {
            start_beat: 0.0,
            end_beat: f64::MAX,
            pitch,
            vel,
            track,
        });
        // Use the exact slot allocated or stolen, including same-pitch timeline voices.
        let live_left = (4.0 * self.sr) as u32;
        let v = &mut self.voices[idx];
        v.live = true;
        v.live_left = live_left;
    }

    fn live_note_off(&mut self, pitch: u8, track: Option<u16>) {
        let mut i = 0;
        while i < self.ext_notes.len() {
            let e = self.ext_notes[i];
            if e.live && e.pitch == pitch && track.is_none_or(|t| e.track == t) {
                self.ext_notes.swap_remove(i);
                self.send_midi(e.track, false, e.pitch, 0, false);
            } else {
                i += 1;
            }
        }
        for v in self.voices.iter_mut().filter(|v| {
            v.active && v.live && v.pitch == pitch && track.is_none_or(|t| v.track == t)
        }) {
            v.releasing = true;
        }
        for n in self.sf_notes.iter_mut().filter(|n| {
            n.active && n.live && n.pitch == pitch && track.is_none_or(|t| n.track == t)
        }) {
            n.released = true;
        }
    }

    /// Starts a SoundFont note on `track`, which has a player. A note that finds no free slot
    /// is not played.
    fn sf_note_on(&mut self, track: u16, pitch: u8, vel: u8, end_beat: f64, live: bool) {
        let Some(slot) = self.sf_notes.iter().position(|n| !n.active) else {
            return;
        };
        if let Some(Some(player)) = self.sf.get_mut(track as usize) {
            player.note_on(pitch, vel);
        }
        self.sf_notes[slot] = SfNote {
            active: true,
            track,
            pitch,
            end_beat,
            live,
            live_left: (4.0 * self.sr) as u32,
            released: false,
        };
    }

    /// Ends the SoundFont notes that are due: a timeline note at its end beat (or when the
    /// transport stops), a live note on its `NoteOff` (or after the safety time).
    fn sf_release_due(&mut self) {
        for n in self.sf_notes.iter_mut().filter(|n| n.active) {
            let due = if n.live {
                n.live_left = n.live_left.saturating_sub(1);
                n.live_left == 0 || n.released
            } else {
                n.released || !self.playing || self.beat >= n.end_beat
            };
            if due {
                n.active = false;
                if let Some(Some(player)) = self.sf.get_mut(n.track as usize) {
                    player.note_off(n.pitch);
                }
            }
        }
    }

    /// Stops the SoundFont notes of `track` at once (its instrument is being replaced).
    fn sf_stop_track(&mut self, track: u16) {
        for n in self
            .sf_notes
            .iter_mut()
            .filter(|n| n.active && n.track == track)
        {
            n.active = false;
        }
    }

    /// Adds one frame of every SoundFont instrument to its track's strip.
    fn sf_render(&mut self) {
        for k in 0..self.sf_tracks.len() {
            let t = self.sf_tracks[k] as usize;
            let Some(Some(player)) = self.sf.get_mut(t) else {
                continue;
            };
            let (x, y) = player.render();
            if self.track_audible[t] {
                self.in_l[t] += finite_or_zero(x) as f64;
                self.in_r[t] += finite_or_zero(y) as f64;
                if !self.touched[t] {
                    self.touched[t] = true;
                    self.dirty.push(t as u16);
                }
            }
        }
    }

    /// The beat the plug-ins are given for the frame about to be rendered (NaN: stopped): the
    /// transport's, or the cycle start on the frame after a wrap (see `wrap_beat`).
    #[inline]
    fn plugin_beat(&self) -> f64 {
        if !self.playing {
            f64::NAN
        } else if self.wrap_beat.is_finite() {
            self.wrap_beat
        } else {
            self.beat
        }
    }

    /// Frames from the next one until the transport plays (0: it plays), and the beat it
    /// plays from; `None` while stopped and not counting in.
    fn start_ahead(&self) -> Option<(usize, f64)> {
        if self.playing {
            Some((0, self.plugin_beat()))
        } else if self.counting {
            // `tick_count_in`: a click whenever the wait reaches 0, the start after the last.
            let left =
                self.count_wait as usize + usize::from(self.count_left) * self.count_step as usize;
            Some((left, self.beat))
        } else {
            None
        }
    }

    /// The transport `lag` frames ago, for the process context (VST3) and host callbacks
    /// (Audio Units) of plug-ins that hear their input that late: a live track's regions
    /// (its content delay), an Aux strip's sends (the strips' delay compensation), the
    /// Stereo Out's mix (the whole output delay). For each output sample they then read the
    /// position the transport had when that sound was made, as a pre-rendered track's plug-ins
    /// do. When the transport (seen `lag` late) starts inside this block of `block` frames,
    /// the block plays from its first frame from a position before the start (pre-roll), so
    /// the plug-in reaches the start beat exactly on the frame it is played; never "stopped"
    /// for the whole block. Stopped: the playhead now, not playing. No allocation.
    fn block_transport_ago(&self, lag: usize, block: usize) -> crate::plugin_track::BlockTransport {
        let n = self.transport_hist.len();
        let lag = lag.min(n - 1);
        // Frame `j` of this block hears the transport's frame `j - lag`: kept in the history
        // for `j < lag` (the block's first frame is written next, at `transport_pos + 1`).
        let hist = |j: usize| self.transport_hist[(self.transport_pos + 1 + j + n - lag) % n];
        let mut start = None;
        if lag > 0 {
            let b = hist(0);
            if b.is_finite() {
                return self.transport_at(b, true);
            }
            start = (1..lag.min(block))
                .map(|j| (j, hist(j)))
                .find(|(_, b)| b.is_finite());
        }
        if start.is_none() {
            start = self
                .start_ahead()
                .map(|(k, b)| (lag + k, b))
                .filter(|&(j, _)| j < block.max(1));
        }
        match start {
            Some((0, b)) => self.transport_at(b, true),
            Some((j, b)) => {
                crate::plugin_track::pre_roll(self.transport_at(b, true), j as u64, self.sr)
            }
            None => self.transport_at(self.beat, false),
        }
    }

    fn transport_at(&self, beat: f64, playing: bool) -> crate::plugin_track::BlockTransport {
        let map = &self.tempo_map;
        let beat = if beat.is_finite() { beat } else { 0.0 };
        let pos = map.position(beat);
        let secs = map.seconds_at(beat);
        crate::plugin_track::BlockTransport {
            playing,
            recording: false,
            sample_pos: if secs.is_finite() {
                (secs * f64::from(self.sr)).round() as i64
            } else {
                0
            },
            beat,
            bar_start_beat: map.beat_of_bar(pos.bar),
            tempo: f64::from(map.tempo_at(beat)),
            sig: (pos.num, pos.den),
            cycle: self.cycle,
        }
    }

    /// The master plug-ins process the block of the mix fed to them, and it goes out through the
    /// master fader (whose gain each frame of `out` holds until then). Returns the block's
    /// peaks. No allocation.
    fn finish_master_block(&mut self, out: &mut [f32]) -> (f32, f32) {
        let (mut pk_l, mut pk_r) = (0f32, 0f32);
        let Some(p) = self.master_plugins.as_mut() else {
            return (pk_l, pk_r);
        };
        // Always in the I/O callback (no playback thread): they come after the mix of the
        // live and the pre-rendered tracks.
        p.end_block_with(out.len() / 2, self.sr, &mut [], Some(&self.shared));
        for (i, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let (l, r) = p.effect_output(i);
            let gain = frame[0];
            // `clamp` lets NaN through, so a non-finite sum becomes silence first.
            let l = finite_or_zero(l * gain).clamp(-1.0, 1.0);
            let r = finite_or_zero(r * gain).clamp(-1.0, 1.0);
            pk_l = pk_l.max(l.abs());
            pk_r = pk_r.max(r.abs());
            frame[0] = l;
            frame[1] = r;
        }
        (pk_l, pk_r)
    }

    /// Render interleaved stereo (`out.len() == frames * 2`).
    pub fn process(&mut self, out: &mut [f32]) {
        flush_denormals();
        self.drain_commands();
        self.take_returned();
        let frame_secs = 1.0 / self.sr as f64;
        let (mut pk_l, mut pk_r) = (0f32, 0f32);
        for c in self.song.clips.iter_mut() {
            c.service();
        }
        self.track_pk_l.fill(0.0);
        self.track_pk_r.fill(0.0);
        if let Some(feed) = self.input.as_mut() {
            feed.trim(self.monitor_count > 0);
        }

        let block_frames = out.len() / 2;
        self.audition.begin_block();
        // The preview loop's tempo when it follows a stopped transport: quarter notes a frame.
        let audition_bpf = f64::from(self.tempo_map.tempo_at(self.beat)) / 60.0 * frame_secs;
        // The master plug-ins take this block whole once its frames are mixed (see
        // `finish_master_block`); a block longer than they were made for passes them by.
        let master_block = self
            .master_plugins
            .as_ref()
            .is_some_and(|p| p.has_active_effects() && block_frames <= p.capacity());
        if master_block {
            // They hear the mix the whole output delay late.
            let bt = self.block_transport_ago(self.output_delay.1 as usize, block_frames);
            if let Some(p) = self.master_plugins.as_mut() {
                p.set_block_transport(bt);
                // Never sent to a playback thread: the master chain stays in the I/O callback.
                p.begin_block(block_frames, false, 0);
            }
        }
        if !self.plugin_tracks.is_empty() {
            let bt = self.block_transport_ago(0, block_frames);
            // Tracks that are not live render their plug-ins on the playback thread.
            let hop = self.prerender_hop(block_frames);
            for k in 0..self.plugin_tracks.len() {
                let t = self.plugin_tracks[k] as usize;
                let away = hop > 0 && !self.track_live[t];
                // A live track's plug-ins hear its regions its content delay late; an Aux
                // strip's hear the sends after the strips' delay compensation.
                let lag = if self.strip_kind[t] == 2 {
                    self.output_delay.0 as usize
                } else {
                    self.content_delay[t] as usize
                };
                let bt = if lag == 0 {
                    bt
                } else {
                    self.block_transport_ago(lag, block_frames)
                };
                if let Some(p) = self.plugins[t].as_mut() {
                    p.set_block_transport(bt);
                    p.begin_block(block_frames, away, hop);
                }
            }
        }
        for (frame_i, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            self.frame_i = frame_i;
            self.hist_pos = (self.hist_pos + 1) % self.beat_hist.len();
            self.beat_hist[self.hist_pos] = self.beat;
            self.tick_midi_clock();
            // A count-in ending now starts the transport on this frame, before the plug-ins
            // learn its beat: they play the start beat on the frame the notes and click do.
            if self.counting {
                self.tick_count_in();
            }
            // Plug-in instruments sound whether or not the transport runs (tails, live notes).
            // An instrument that follows the transport (a drum machine) learns this frame's beat.
            // A live track's instrument gets it as late as its regions are (its content delay),
            // so it plays in time with the other tracks whether the track is live or not.
            let transport_beat = self.plugin_beat();
            self.wrap_beat = f64::NAN;
            let hist_n = self.transport_hist.len();
            self.transport_pos = (self.transport_pos + 1) % hist_n;
            self.transport_hist[self.transport_pos] = transport_beat;
            for k in 0..self.plugin_tracks.len() {
                let t = self.plugin_tracks[k] as usize;
                if let Some(p) = self.plugins[t].as_mut() {
                    if p.follows_transport() {
                        let lag = self.content_delay[t] as usize;
                        let beat = if lag == 0 {
                            transport_beat
                        } else {
                            self.transport_hist[(self.transport_pos + hist_n - lag) % hist_n]
                        };
                        p.set_beat(frame_i, beat);
                    }
                    let (pl, pr) = p.instrument_frame(frame_i);
                    if self.track_audible[t] {
                        self.in_l[t] += pl as f64;
                        self.in_r[t] += pr as f64;
                    }
                }
            }
            let sample_beat = self.beat;
            // Advance once per track/sample, not once per voice or overlapping audio clip.
            if self.mix_ramp_remaining > 0 {
                for i in 0..MAX_TRACKS {
                    if !self.auto_on[i] {
                        self.track_gain_l[i] = self.track_ramp_l[i].next();
                        self.track_gain_r[i] = self.track_ramp_r[i].next();
                    }
                }
                self.mix_ramp_remaining -= 1;
            }
            if self.auto_any {
                if self.auto_left == 0 {
                    self.update_automation(frame_i);
                    self.auto_left = AUTO_BLOCK;
                }
                self.auto_left -= 1;
                for k in 0..self.auto_tracks.len() {
                    let t = self.auto_tracks[k] as usize;
                    self.track_gain_l[t] = self.track_ramp_l[t].next();
                    self.track_gain_r[t] = self.track_ramp_r[t].next();
                    for s in 0..MAX_SENDS {
                        if self.auto_send_on[t][s] {
                            self.auto_send[t][s].next();
                        }
                    }
                }
            }
            let (mut l, mut r) = (0f64, 0f64);
            if self.playing {
                while let Some(n) = self.song.notes.get(self.next_note).copied() {
                    if n.start_beat > self.beat {
                        break;
                    }
                    self.next_note += 1;
                    if n.end_beat > self.beat {
                        self.trigger(n);
                    }
                }
                while let Some(c) = self.song.controls.get(self.next_control).copied() {
                    if c.beat > self.beat {
                        break;
                    }
                    self.next_control += 1;
                    self.send_control(c);
                }
                for k in 0..self.song.clips.len() {
                    let c = &mut self.song.clips[k];
                    if let Some((cl, cr)) = c.tick() {
                        let t = (c.track as usize).min(MAX_TRACKS - 1);
                        if self.track_audible[t] {
                            let edit_gain = c.envelope(sample_beat, c.end_beat);
                            self.add_content(t, (cl * edit_gain) as f64, (cr * edit_gain) as f64);
                        }
                    }
                }
            }
            if !self.ext_notes.is_empty() {
                self.release_ext_notes(false);
            }
            if self.monitor_count > 0 {
                self.hear_input();
            }
            if !self.sf_tracks.is_empty() {
                self.sf_release_due();
            }
            for v in self.voices.iter_mut().filter(|v| v.active) {
                if v.live {
                    v.live_left = v.live_left.saturating_sub(1);
                    if v.live_left == 0 {
                        v.releasing = true;
                    }
                } else if !self.playing
                    || (self.beat >= v.end_beat && !self.midi_ctl[v.track as usize].sustain)
                {
                    // A note that ends while the Sustain pedal is down sounds until it goes up.
                    v.releasing = true;
                }
                let sr = self.sr;
                if v.releasing && v.stage != 3 {
                    v.stage = 3;
                    v.rel_step = v.env / (v.patch.release.max(0.002) * sr);
                }
                let pt = &v.patch;
                match v.stage {
                    0 => {
                        v.env += v.attack_step;
                        if v.env >= 1.0 {
                            v.env = 1.0;
                            v.stage = 1;
                        }
                    }
                    1 => {
                        v.env -= v.decay_step;
                        if v.env <= pt.sustain {
                            v.env = pt.sustain;
                            v.stage = 2;
                        }
                    }
                    2 => {}
                    _ => {
                        v.env -= v.rel_step;
                        if v.env <= 0.0 {
                            v.active = false;
                            continue;
                        }
                    }
                }
                let filtered = pt.cutoff < 19_000.0 || pt.highpass;
                if v.ctrl_left == 0 {
                    v.ctrl_left = CTRL_BLOCK;
                    let ctl = self.midi_ctl[v.track as usize];
                    let depth = pt.vibrato_cents + ctl.vibrato_cents();
                    let vib = if depth != 0.0 {
                        let rate = if pt.vibrato_hz > 0.0 {
                            pt.vibrato_hz
                        } else {
                            5.5
                        };
                        v.vibrato = (v.vibrato + rate * CTRL_BLOCK as f32 / sr).fract();
                        (v.vibrato * std::f32::consts::TAU).sin() * depth / 1200.0
                    } else {
                        0.0
                    };
                    let bend = v.pitch_env / 12.0 + vib + ctl.bend / 12.0;
                    let mul = if bend == 0.0 { 1.0 } else { 2f32.powf(bend) };
                    if pt.pitch_drop != 0.0 {
                        v.pitch_env *= v.pitch_decay_block;
                    }
                    v.dt_a = v.base_hz * mul / sr;
                    v.dt_b = v.dt_a * v.ratio_b;
                    if filtered {
                        let fc = if pt.filter_env == 0.0 {
                            pt.cutoff
                        } else {
                            pt.cutoff * 2f32.powf(pt.filter_env * v.env * 4.0)
                        };
                        v.filter_f =
                            Svf::coefficient(fc, sr).min(Svf::max_coefficient(v.filter_damping));
                    }
                }
                v.ctrl_left -= 1;
                let (dt_a, dt_b) = (v.dt_a, v.dt_b);
                v.phase_a = wrap_phase(v.phase_a + dt_a);
                v.phase_b = wrap_phase(v.phase_b + dt_b);
                let mut x = osc(pt.wave_a, v.phase_a, dt_a, pt.pulse_width) * pt.level_a
                    + osc(pt.wave_b, v.phase_b, dt_b, pt.pulse_width) * pt.level_b;
                if pt.noise > 0.0 {
                    v.noise ^= v.noise << 13;
                    v.noise ^= v.noise >> 17;
                    v.noise ^= v.noise << 5;
                    x += ((v.noise as f32 / u32::MAX as f32) * 2.0 - 1.0) * pt.noise;
                }
                if filtered {
                    let (low, high) = v.filter.tick(x, v.filter_f, v.filter_damping);
                    x = if pt.highpass { high } else { low };
                }
                // Last line of defence: a voice that ever goes NaN or infinite is silenced for
                // that sample and its filter reset, instead of poisoning the mix and meters.
                if !x.is_finite() {
                    v.filter = Svf::default();
                    x = 0.0;
                }
                let t = v.track as usize;
                if self.track_audible[t] {
                    let ctl = &self.midi_ctl[t];
                    let x = finite_or_zero(x * v.amp * v.env * ctl.volume * ctl.expression);
                    let (xl, xr) = ((x * ctl.pan_l) as f64, (x * ctl.pan_r) as f64);
                    // A live track's regions wait in its content line; live notes do not.
                    match self.content_lines.get_mut(self.content_line[t] as usize) {
                        Some(line) if !v.live => {
                            line.acc.0 += xl;
                            line.acc.1 += xr;
                        }
                        _ => {
                            self.in_l[t] += xl;
                            self.in_r[t] += xr;
                            if !self.touched[t] {
                                self.touched[t] = true;
                                self.dirty.push(t as u16);
                            }
                        }
                    }
                }
            }
            self.content_out();
            self.sf_render();
            let (ml, mr) = self.mix_strips();
            l += ml;
            r += mr;
            if self.test_hz > 0.0 {
                self.test_phase += self.test_hz / self.sr;
                if self.test_phase >= 1.0 {
                    self.test_phase -= 1.0;
                }
                let t = (self.test_phase * std::f32::consts::TAU).sin() * self.test_gain;
                l += t as f64;
                r += t as f64;
            }
            {
                let at = if self.playing { self.beat } else { f64::NAN };
                let (al, ar) = self.audition.frame(at, audition_bpf);
                l += al as f64;
                r += ar as f64;
            }
            if self.start_click {
                self.start_click = false;
                if self.playing && self.metronome {
                    if let Some(freq) = self.click_on(self.beat) {
                        self.schedule_click(freq);
                    }
                }
            }
            if self.click_queued > 0 {
                self.release_clicks();
            }
            if self.click_remaining > 0 {
                self.click_remaining -= 1;
                self.click_phase += self.click_freq / self.sr;
                let env = self.click_remaining as f32 / (0.03 * self.sr);
                let c = (self.click_phase * std::f32::consts::TAU).sin() * env * self.click_gain;
                l += c as f64;
                r += c as f64;
            }
            // The master effects: the whole mix goes through them, before the master fader.
            if let Some(chain) = self.master_chain.as_mut() {
                (l, r) = {
                    let (cl, cr) =
                        chain.process(finite_or_zero(l as f32), finite_or_zero(r as f32));
                    (f64::from(cl), f64::from(cr))
                };
            }
            let master = self.master.next() as f64;
            if master_block {
                // The mix waits in the master plug-ins and the fader's gain in the output
                // until the block is done.
                if let Some(p) = self.master_plugins.as_mut() {
                    p.through_effects(frame_i, finite_or_zero(l as f32), finite_or_zero(r as f32));
                }
                frame[0] = master as f32;
                frame[1] = 0.0;
            } else {
                // `clamp` lets NaN through, so a non-finite sum becomes silence first.
                let l = finite_or_zero((l * master) as f32).clamp(-1.0, 1.0);
                let r = finite_or_zero((r * master) as f32).clamp(-1.0, 1.0);
                pk_l = pk_l.max(l.abs());
                pk_r = pk_r.max(r.abs());
                frame[0] = l;
                frame[1] = r;
            }
            // Render events at the current sample position, then advance to the next one.
            if self.playing {
                let prev = self.beat;
                self.beat = self.tempo_map.advance(prev, frame_secs);
                if self.mtc.is_on() {
                    self.mtc_secs += frame_secs;
                    let frames = self.mtc_frames();
                    let tx = &mut self.mtc_out;
                    self.mtc.tick(frames, |m| {
                        let _ = tx.push(m);
                    });
                }
                if let Some((s, e)) = self.cycle {
                    if self.beat >= e {
                        let start = self.tempo_map.seconds_at(s);
                        let end = self.tempo_map.seconds_at(e);
                        let duration = end - start;
                        if duration > 0.0 {
                            let excess = self.tempo_map.seconds_at(self.beat) - end;
                            let wrapped =
                                self.tempo_map.beat_at(start + excess.rem_euclid(duration));
                            self.seek(wrapped);
                            // The plug-ins hear the cycle start itself on the next frame.
                            self.wrap_beat = s;
                            // A fractional-sample wrap must still trigger notes at cycle start.
                            self.next_note = self.song.notes.partition_point(|n| n.start_beat < s);
                            self.next_control = self.song.controls.partition_point(|c| c.beat < s);
                        }
                    }
                }
                if self.metronome {
                    let (a, b) = (
                        self.tempo_map.position(prev),
                        self.tempo_map.position(self.beat),
                    );
                    if (a.bar, a.beat) != (b.bar, b.beat) {
                        let freq = self.click_freq_of(b.beat);
                        self.schedule_click(freq);
                    }
                }
            }
        }
        self.audition.end_block();
        if master_block {
            (pk_l, pk_r) = self.finish_master_block(out);
        }
        // The plug-ins make the next block: the instruments play this block's notes, the
        // effects process what reached them.
        let mut late = 0;
        for k in 0..self.plugin_tracks.len() {
            let t = self.plugin_tracks[k] as usize;
            if let Some(p) = self.plugins[t].as_mut() {
                p.end_block_with(block_frames, self.sr, &mut self.workers, Some(&self.shared));
                late += std::mem::take(&mut p.underruns).min(1);
            }
        }
        if late > 0 {
            self.shared
                .prerender_underruns
                .fetch_add(1, Ordering::Relaxed);
        }
        for port in self.workers.iter().filter(|p| p.busy()) {
            port.wake();
        }
        let mut underruns = 0;
        for c in self.song.clips.iter_mut() {
            underruns += std::mem::take(&mut c.underruns).min(1);
        }
        if underruns > 0 {
            self.shared
                .underruns
                .fetch_add(underruns, Ordering::Relaxed);
        }
        let shown = &mut self.shown;
        shown.position_beats = self.beat;
        shown.playing = self.playing;
        shown.process_buffer = self.process_buffer;
        shown.prerender_delay =
            crate::prerender::prerender_delay(self.process_buffer, block_frames as u32);
        // Peak with simple decay so the UI meter does not flicker between callbacks.
        let (ol, or) = shown.peaks;
        shown.peaks = (decayed_peak(pk_l, ol), decayed_peak(pk_r, or));
        for (t, (l, r)) in shown.track_peaks.iter_mut().enumerate() {
            *l = decayed_peak(self.track_pk_l[t], *l);
            *r = decayed_peak(self.track_pk_r[t], *r);
        }
        for &t in self.complex.iter().chain(self.aux.iter()) {
            let t = t as usize;
            shown.gain_reduction[t] = self.chains[t].as_ref().map_or(0.0, |c| c.reduction_db());
            shown.fx_input[t] = self.chains[t].as_ref().map_or(0.0, |c| c.input_level());
        }
        shown.master_reduction = self.master_chain.as_ref().map_or(0.0, |c| c.reduction_db());
        shown.master_fx_input = self.master_chain.as_ref().map_or(0.0, |c| c.input_level());
        *self.state.input() = *shown;
        self.state.publish();
    }
}

/// Treats the smallest floating-point numbers as zero on this thread. A decaying reverb or filter
/// tail would otherwise reach them, where every operation can take many times as long, and nobody
/// can hear them. Other processors keep their usual behaviour.
#[inline]
fn flush_denormals() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: only sets the flush-to-zero and denormals-are-zero bits of this thread's control word.
    #[allow(deprecated)]
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | 0x8040);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn denormals_become_zero_on_the_audio_thread() {
        use std::hint::black_box;
        // Its own thread: the setting belongs to the thread and stays on it.
        let thread = std::thread::spawn(|| {
            let tiny = black_box(1e-40f32);
            assert!(tiny * black_box(1.0) != 0.0, "a denormal before");
            flush_denormals();
            assert_eq!(tiny * black_box(1.0), 0.0, "zero after");
        });
        assert!(thread.join().is_ok());
    }

    fn rig() -> (EngineCore, Producer<Command>, StateReader) {
        let (tx, rx) = rtrb::RingBuffer::new(64);
        let (gtx, _grx) = rtrb::RingBuffer::new(8);
        let mut core = EngineCore::new(48_000, rx, gtx, Arc::new(Shared::new()));
        let Some(state) = core.take_state_reader() else {
            panic!("one reader");
        };
        (core, tx, state)
    }

    fn note(start: f64, end: f64) -> NoteEvent {
        NoteEvent {
            start_beat: start,
            end_beat: end,
            pitch: 69,
            vel: 100,
            track: 0,
        }
    }

    #[test]
    fn clip_fade_timing_refreshes_after_tempo_and_song_changes_without_allocating() {
        let (mut core, mut tx, _) = rig();
        let (mut producer, consumer) = rtrb::RingBuffer::new(8);
        let clip = AudioClip::new(
            0,
            4.0,
            48_000,
            ClipEdit {
                fade_out_beats: 1.0,
                fade_curve: FadeCurve::Linear,
                ..ClipEdit::default()
            },
            consumer,
            Arc::new(ClipShared::default()),
        );
        let song = Song::new(vec![], vec![TrackMix::default()]).with_clips(vec![clip]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());

        for (bpm, expected_end, expected_sample) in
            [(120.0, 6.0, 0.8), (60.0, 5.0, 0.4), (180.0, 7.0, 0.8)]
        {
            assert!(tx
                .push(Command::SetTempoMap(Box::new(TempoMap::constant(
                    bpm,
                    (4, 4)
                ))))
                .is_ok());
            assert!(tx.push(Command::SetPosition { beat: 4.5 }).is_ok());
            assert!(tx.push(Command::Play).is_ok());
            assert_no_alloc::assert_no_alloc(|| core.process(&mut []));
            let clip = &core.song.clips[0];
            assert!((clip.end_beat - expected_end).abs() < 1e-12);
            assert!(producer
                .push(Block {
                    gen: clip.gen,
                    start: clip.pos as u64,
                    frames: 1024,
                    data: [1.0; 2048],
                })
                .is_ok());
            let mut out = [0.0; 2];
            assert_no_alloc::assert_no_alloc(|| core.process(&mut out));
            assert!(out.iter().all(|s| (*s - expected_sample).abs() < 1e-6));
        }

        let (_producer, consumer) = rtrb::RingBuffer::new(8);
        let replacement = AudioClip::new(
            0,
            8.0,
            96_000,
            ClipEdit::default(),
            consumer,
            Arc::new(ClipShared::default()),
        );
        assert!(tx
            .push(Command::SetSong(Box::new(
                Song::default().with_clips(vec![replacement]),
            )))
            .is_ok());
        assert_no_alloc::assert_no_alloc(|| core.process(&mut []));
        assert!((core.song.clips[0].end_beat - 14.0).abs() < 1e-12);
    }

    #[test]
    fn fade_curves_preserve_reference_gain_inside_outside_and_overlapping_fades() {
        let (_producer, consumer) = rtrb::RingBuffer::new(8);
        let mut clip = AudioClip::new(
            0,
            0.0,
            96_000,
            ClipEdit::default(),
            consumer,
            Arc::new(ClipShared::default()),
        );
        for curve in [FadeCurve::Linear, FadeCurve::EqualPower, FadeCurve::SCurve] {
            for duration in [1.0, 3.0] {
                clip.edit.fade_curve = curve;
                clip.edit.fade_in_beats = duration;
                clip.edit.fade_out_beats = duration;
                for beat in [-1.0_f64, 0.0, 0.5, 1.0, 2.0, 3.0, 3.5, 4.0, 5.0] {
                    let reference = |x: f64| {
                        let x = x.clamp(0.0, 1.0);
                        match curve {
                            FadeCurve::Linear => x as f32,
                            FadeCurve::EqualPower => (x * std::f64::consts::FRAC_PI_2).sin() as f32,
                            FadeCurve::SCurve => (x * x * (3.0 - 2.0 * x)) as f32,
                        }
                    };
                    let expected = reference(beat / duration) * reference((4.0 - beat) / duration);
                    assert_eq!(clip.envelope(beat, 4.0), expected);
                }
            }
        }
    }

    #[test]
    fn single_channel_mix_updates_match_full_rebuild_without_allocating() {
        let (mut changed, mut changed_tx, _) = rig();
        let (mut reference, mut reference_tx, _) = rig();
        for tx in [&mut changed_tx, &mut reference_tx] {
            assert!(tx
                .push(Command::SetSong(Box::new(Song::new(
                    vec![],
                    vec![TrackMix::default(); 2],
                ))))
                .is_ok());
            for (track, pitch) in [(0, 60), (1, 67)] {
                assert!(tx
                    .push(Command::NoteOn {
                        track,
                        pitch,
                        vel: 80
                    })
                    .is_ok());
            }
        }
        let mut actual = [0.0; 128];
        let mut expected = [0.0; 128];
        changed.process(&mut actual);
        reference.process(&mut expected);
        assert_eq!(actual, expected);

        for (track, gain_db, pan, mute, solo) in [
            (0, -6.0, -0.5, false, false),
            (1, -9.0, 0.75, false, true),
            (0, -3.0, 0.5, true, false),
            (1, 0.0, 0.0, false, false),
        ] {
            let mix = TrackMix {
                gain_db,
                pan,
                mute,
                solo,
            };
            assert!(changed_tx.push(Command::SetTrackMix { track, mix }).is_ok());
            reference.song.tracks[track as usize] = mix;
            reference.rebuild_mix(true, None);
            // Retarget while earlier 240-sample ramps are still running.
            for _ in 0..2 {
                assert_no_alloc::assert_no_alloc(|| changed.process(&mut actual));
                assert_no_alloc::assert_no_alloc(|| reference.process(&mut expected));
                assert_eq!(actual, expected);
                assert_eq!(changed.track_audible, reference.track_audible);
            }
        }

        // A silent edit must still complete every pending ramp immediately.
        for core in [&mut changed, &mut reference] {
            for voice in &mut core.voices {
                voice.active = false;
            }
        }
        let mix = TrackMix::default();
        assert!(changed_tx
            .push(Command::SetTrackMix { track: 0, mix })
            .is_ok());
        reference.song.tracks[0] = mix;
        reference.rebuild_mix(false, None);
        assert_no_alloc::assert_no_alloc(|| changed.process(&mut []));
        assert_eq!(changed.track_gain_l, reference.track_gain_l);
        assert_eq!(changed.track_gain_r, reference.track_gain_r);
    }

    #[test]
    fn full_garbage_queue_defers_state_in_order_and_does_not_allocate() {
        let (mut tx, rx) = rtrb::RingBuffer::new(8);
        let (gtx, mut grx) = rtrb::RingBuffer::new(1);
        let shared = Arc::new(Shared::new());
        let mut e = EngineCore::new(48_000, rx, gtx, shared);
        let commands = [
            Command::SetSong(Box::new(Song::new(vec![note(0.0, 1.0)], vec![]))),
            Command::SetNotes(vec![note(2.0, 3.0)]),
            Command::SetTempoMap(Box::new(TempoMap::constant(90.0, (6, 8)))),
            Command::SetSong(Box::new(Song::new(vec![note(4.0, 5.0)], vec![]))),
            Command::Play,
        ];
        for cmd in commands {
            assert!(tx.push(cmd).is_ok());
        }
        let mut buf = [0.0; 256];
        for remaining in [4, 3, 2, 0] {
            assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
            assert_eq!(e.rx.slots(), remaining);
            assert!(buf.iter().all(|s| s.is_finite()));
            if remaining > 0 {
                // Repeated callbacks must neither consume nor destroy the blocked command.
                assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
                assert_eq!(e.rx.slots(), remaining);
                assert!(!e.playing);
            }
            assert!(grx.pop().is_ok()); // reclamation outside the audio callback
        }
        assert!(e.playing);
        assert_eq!(e.song.notes[0].start_beat, 4.0);
    }

    #[test]
    fn one_second_of_silence_is_finite_and_zero() {
        let (mut e, _tx, _s) = rig();
        let mut buf = vec![1.0f32; 48_000 * 2];
        e.process(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite() && *x == 0.0));
    }

    #[test]
    fn invalid_public_parameters_cannot_poison_the_mix_or_transport() {
        let (mut e, mut tx, _) = rig();
        let tracks = vec![
            TrackMix {
                gain_db: f32::NAN,
                pan: f32::INFINITY,
                ..TrackMix::default()
            },
            TrackMix::default(),
        ];
        let song = Song::new(
            vec![
                note(0.0, 1.0),
                NoteEvent {
                    track: 1,
                    ..note(0.0, 1.0)
                },
            ],
            tracks,
        );
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx
            .push(Command::SetTrackPatch {
                track: 0,
                patch: SynthPatch {
                    gain: f32::INFINITY,
                    semi_b: f32::MAX,
                    ..SynthPatch::default()
                },
            })
            .is_ok());
        assert!(tx
            .push(Command::SetTestTone {
                hz: f32::NAN,
                gain: f32::NAN
            })
            .is_ok());
        assert!(tx
            .push(Command::SetPosition {
                beat: f64::INFINITY
            })
            .is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = [0.0; 1024];
        assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        assert!(buf.iter().all(|x| x.is_finite()));
        assert!(buf.iter().any(|x| x.abs() > 0.0));
        assert!(e.beat.is_finite());
    }

    #[test]
    fn transport_advances_one_beat_per_half_second_at_120() {
        let (mut e, mut tx, mut s) = rig();
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 24_000 * 2];
        e.process(&mut buf);
        assert!(
            (s.read().position_beats - 1.0).abs() < 1e-3,
            "{}",
            s.read().position_beats
        );
        assert!(s.read().playing);
    }

    #[test]
    fn stop_returns_to_start_position() {
        let (mut e, mut tx, mut s) = rig();
        assert!(tx.push(Command::SetPosition { beat: 8.0 }).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 4800 * 2];
        e.process(&mut buf);
        assert!(s.read().position_beats > 8.0);
        assert!(tx.push(Command::Stop).is_ok());
        e.process(&mut buf);
        assert_eq!(s.read().position_beats, 8.0);
    }

    #[test]
    fn note_makes_sound_and_never_nan() {
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(
            vec![note(0.0, 1.0), note(0.5, 2.0)],
            vec![TrackMix::default()],
        );
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 48_000 * 2];
        e.process(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite()));
        assert!(buf.iter().any(|x| x.abs() > 0.01));
    }

    /// A core with a garbage queue the test can read (to see the instruments it retires).
    fn rig_with_garbage() -> (EngineCore, Producer<Command>, rtrb::Consumer<Garbage>) {
        let (tx, rx) = rtrb::RingBuffer::new(64);
        let (gtx, grx) = rtrb::RingBuffer::new(8);
        let core = EngineCore::new(48_000, rx, gtx, Arc::new(Shared::new()));
        (core, tx, grx)
    }

    fn sine_player(sr: f32) -> Box<SfPlayer> {
        let Ok(bank) =
            crate::soundfont::SoundFontBank::load(&crate::soundfont::test_font::sine_font())
        else {
            panic!("the test SoundFont loads");
        };
        let Some(preset) = bank.presets().first().cloned() else {
            panic!("a preset");
        };
        let Ok(player) = SfPlayer::new(&bank, &preset, sr) else {
            panic!("a player");
        };
        Box::new(player)
    }

    #[test]
    fn a_soundfont_track_plays_its_notes_through_its_strip_and_ends_them() {
        let (mut e, mut tx, mut s) = rig();
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 0,
                player: Some(sine_player(48_000.0)),
            })
            .is_ok());
        assert!(tx
            .push(Command::SetSong(Box::new(Song::new(
                vec![note(0.0, 1.0)],
                vec![TrackMix::default()]
            ))))
            .is_ok());
        assert!(tx.push(Command::Play).is_ok());
        // 120 BPM: one beat is half a second. The note lasts one beat.
        let mut buf = vec![0.0f32; 48_000 * 2 * 2];
        e.process(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite()));
        let first_half_second: f32 = buf[..48_000 * 2 / 2].iter().map(|x| x.abs()).sum();
        assert!(
            first_half_second > 1.0,
            "the note sounds: {first_half_second}"
        );
        assert!(s.read().track_peaks(0).0 > 0.01, "the track's meter moves");
    }

    #[test]
    fn a_live_soundfont_note_sounds_while_stopped_and_stops_on_note_off() {
        let (mut e, mut tx, _s) = rig();
        assert!(tx
            .push(Command::SetSong(Box::new(Song::new(
                vec![],
                vec![TrackMix::default()]
            ))))
            .is_ok());
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 0,
                player: Some(sine_player(48_000.0)),
            })
            .is_ok());
        assert!(tx
            .push(Command::NoteOn {
                pitch: 69,
                vel: 100,
                track: 0
            })
            .is_ok());
        let mut buf = vec![0.0f32; 4_800 * 2];
        e.process(&mut buf);
        // The test font is quiet (its peak is about 0.04 here); real banks are louder.
        assert!(
            buf.iter().any(|x| x.abs() > 0.02),
            "live note sounds while stopped"
        );
        assert!(tx.push(Command::NoteOff { pitch: 69 }).is_ok());
        // The note fades out after its NoteOff (the test sample alone would last a second).
        let mut peaks = [0f32; 8];
        for peak in peaks.iter_mut() {
            let mut tail = vec![0.0f32; 4_800 * 2];
            e.process(&mut tail);
            *peak = tail.iter().fold(0f32, |m, x| m.max(x.abs()));
        }
        assert!(peaks[3] < peaks[0] * 0.5, "the note fades: {peaks:?}");
        assert!(peaks[7] < 0.001, "the note is released: {peaks:?}");
    }

    #[test]
    fn replacing_a_soundfont_returns_the_old_player_and_stops_its_notes() {
        let (mut e, mut tx, mut garbage) = rig_with_garbage();
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 1,
                player: Some(sine_player(48_000.0)),
            })
            .is_ok());
        assert!(tx
            .push(Command::NoteOn {
                pitch: 60,
                vel: 100,
                track: 1
            })
            .is_ok());
        let mut buf = vec![0.0f32; 480 * 2];
        e.process(&mut buf);
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 1,
                player: None,
            })
            .is_ok());
        e.process(&mut buf);
        match garbage.pop() {
            Ok(Garbage::SoundFont(_)) => {}
            _ => panic!("the old player goes back to the UI thread"),
        }
        assert!(
            e.sf_notes.iter().all(|n| !(n.active && n.track == 1)),
            "its notes stopped"
        );
    }

    /// The General MIDI SoundFont named by `PROWAVE_TEST_SF2`, if set (it is not in the repo):
    /// a real piano note reaches the strip through the engine's own command path.
    #[test]
    fn a_general_midi_piano_plays_through_the_engine_if_a_soundfont_is_provided() {
        let Ok(path) = std::env::var("PROWAVE_TEST_SF2") else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            panic!("cannot read {path}");
        };
        let Ok(bank) = crate::soundfont::SoundFontBank::load(&bytes) else {
            panic!("the SoundFont loads");
        };
        let Some(piano) = bank.presets().first().cloned() else {
            panic!("a preset");
        };
        let Ok(player) = SfPlayer::new(&bank, &piano, 48_000.0) else {
            panic!("a player");
        };
        let (mut e, mut tx, mut s) = rig();
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 0,
                player: Some(Box::new(player)),
            })
            .is_ok());
        assert!(tx
            .push(Command::SetSong(Box::new(Song::new(
                vec![note(0.0, 2.0)],
                vec![TrackMix::default()]
            ))))
            .is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 48_000 * 2];
        e.process(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite()));
        let peak = s.read().track_peaks(0).0;
        assert!(peak > 0.05, "the piano is heard on its track: {peak}");
    }

    #[test]
    fn soundfont_notes_do_not_allocate_on_the_audio_thread() {
        let (mut e, mut tx, _s) = rig();
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 0,
                player: Some(sine_player(48_000.0)),
            })
            .is_ok());
        assert!(tx
            .push(Command::SetSong(Box::new(Song::new(
                vec![note(0.0, 2.0), note(0.5, 1.0)],
                vec![TrackMix::default()]
            ))))
            .is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 4_800 * 2];
        assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        assert!(buf.iter().any(|x| x.abs() > 0.01));
    }

    #[test]
    fn muted_track_is_silent() {
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(
            vec![note(0.0, 1.0)],
            vec![TrackMix {
                gain_db: 0.0,
                mute: true,
                solo: false,
                pan: 0.0,
            }],
        );
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 24_000 * 2];
        e.process(&mut buf);
        assert!(buf.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn a_count_in_clicks_each_beat_then_plays_from_the_same_place() {
        let (mut e, mut tx, _s) = rig();
        assert!(tx.push(Command::SetPosition { beat: 8.0 }).is_ok());
        let (clicks, beats_per_click) = (2, 1.0);
        let cmd = Command::CountIn {
            clicks,
            beats_per_click,
        };
        assert!(tx.push(cmd).is_ok());
        // 120 BPM: 2 beats are 1 second at 48 kHz.
        let mut buf = vec![0.0f32; 47_000 * 2];
        assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        assert!(!e.playing && e.counting, "still counting");
        assert_eq!(e.beat, 8.0, "the playhead waits");
        assert!(buf[..960].iter().any(|x| x.abs() > 0.05), "first click");
        let second = 24_000 * 2;
        assert!(buf[second..second + 960].iter().any(|x| x.abs() > 0.05));
        let mut more = vec![0.0f32; 2_000 * 2];
        e.process(&mut more);
        assert!(e.playing && !e.counting);
        assert!(e.beat > 8.0 && e.beat < 8.1, "{}", e.beat);
    }

    #[test]
    fn stopping_cancels_a_count_in() {
        let (mut e, mut tx, _s) = rig();
        let cmd = Command::CountIn {
            clicks: 4,
            beats_per_click: 1.0,
        };
        assert!(tx.push(cmd).is_ok());
        let mut buf = vec![0.0f32; 256];
        e.process(&mut buf);
        assert!(tx.push(Command::Pause).is_ok());
        e.process(&mut buf);
        assert!(!e.counting && !e.playing);
    }

    #[test]
    fn a_monitored_track_hears_its_input_through_its_strip() {
        let (mut e, mut tx, mut s) = rig();
        let song = Song::new(Vec::new(), vec![TrackMix::default(), TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        let (mut input, rx) = rtrb::RingBuffer::new(8192);
        let feed = InputFeed::new(rx, 2, 48_000);
        assert!(tx.push(Command::SetInput(Some(Box::new(feed)))).is_ok());
        let mut tracks = [MonitorTrack::default(); MAX_MONITORS];
        tracks[0] = MonitorTrack {
            track: 1,
            channel: 1,
            stereo: false,
        };
        assert!(tx.push(Command::SetMonitors { tracks, count: 1 }).is_ok());
        let mut buf = vec![0.0f32; 512];
        e.process(&mut buf);
        for _ in 0..256 {
            assert!(input.push(0.0).is_ok());
            assert!(input.push(0.25).is_ok());
        }
        assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        let loudest = buf.iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(loudest > 0.1, "input 2 is heard: {loudest}");
        assert!(s.read().track_peaks(1).0 > 0.2 && s.read().track_peaks(0).0 == 0.0);
        // No more tracks listening: silence, and the input does not pile up.
        assert!(tx.push(Command::SetMonitors { tracks, count: 0 }).is_ok());
        for _ in 0..256 {
            assert!(input.push(0.5).is_ok());
        }
        e.process(&mut buf);
        assert!(buf.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn a_plugin_instrument_plays_the_notes_instead_of_the_synth() {
        use crate::plugin_track::tests::Fake;
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(vec![note(0.0, 4.0)], vec![TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        let p = PluginTrack::new(Some(Box::new(Fake::instrument())), Vec::new(), 256);
        assert!(tx
            .push(Command::SetPlugins {
                track: 0,
                plugins: Some(Box::new(p)),
            })
            .is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 128];
        e.process(&mut buf);
        assert!(
            buf.iter().all(|x| *x == 0.0),
            "the built-in synth does not play it"
        );
        assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        assert!(
            buf[0] > 0.1,
            "the plug-in plays it a block later: {}",
            buf[0]
        );
        // A live note on a plug-in track goes to the plug-in too.
        assert!(tx.push(Command::Pause).is_ok());
        assert!(tx
            .push(Command::NoteOn {
                pitch: 60,
                vel: 100,
                track: 0
            })
            .is_ok());
        e.process(&mut buf);
        e.process(&mut buf);
        assert!(buf.iter().any(|x| *x > 0.1), "live note");
        // Taking the plug-ins away hands them back to be freed off the audio thread.
        assert!(tx
            .push(Command::SetPlugins {
                track: 0,
                plugins: None,
            })
            .is_ok());
        e.process(&mut buf);
        assert!(e.plugin_tracks.is_empty());
    }

    /// Plug-ins learn where each block starts: playing, the tempo, the signature and the
    /// position in samples and beats (VST3: the process context).
    #[test]
    fn plugins_learn_the_transport_of_each_block() {
        use crate::plugin_track::{BlockProcessor, BlockTransport};
        use std::sync::{Arc, Mutex};
        struct Spy(Arc<Mutex<Vec<BlockTransport>>>);
        impl BlockProcessor for Spy {
            fn midi(&mut self, _: u8, _: u8, _: u8, _: u32) {}
            fn process(&mut self, _i: (&[f32], &[f32]), o: (&mut [f32], &mut [f32])) {
                o.0.fill(0.0);
                o.1.fill(0.0);
            }
            fn latency_seconds(&self) -> f64 {
                0.0
            }
            fn block_transport(&mut self, t: &BlockTransport) {
                if let Ok(mut v) = self.0.lock() {
                    if v.len() < v.capacity() {
                        v.push(*t);
                    }
                }
            }
        }
        let seen = Arc::new(Mutex::new(Vec::with_capacity(8)));
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(Vec::new(), vec![TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        let p = PluginTrack::new(None, vec![Box::new(Spy(seen.clone()))], 256);
        assert!(tx
            .push(Command::SetPlugins {
                track: 0,
                plugins: Some(Box::new(p)),
            })
            .is_ok());
        let mut buf = vec![0.0f32; 128];
        e.process(&mut buf);
        assert!(tx.push(Command::Play).is_ok());
        e.process(&mut buf);
        assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        let v = seen.lock().map(|v| v.clone()).unwrap_or_default();
        assert_eq!(v.len(), 3);
        assert!(!v[0].playing && v[1].playing && v[2].playing);
        assert_eq!(v[1].sample_pos, 0);
        assert_eq!(v[2].sample_pos, 64, "one block of 64 frames later");
        let sr = f64::from(e.sr);
        assert!((v[2].beat - 64.0 / sr * v[2].tempo / 60.0).abs() < 1e-9);
        assert_eq!(v[2].sig, (4, 4));
        assert_eq!(v[2].bar_start_beat, 0.0);
        assert!(v[2].tempo > 0.0);
    }

    /// Sixteen tracks monitor sixteen inputs at once, each hearing only its own.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn sixteen_tracks_monitor_their_own_inputs() {
        let (mut e, mut tx, mut s) = rig();
        let song = Song::new(Vec::new(), vec![TrackMix::default(); 16]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        let (mut capture, mut tap) = crate::capture::Capture::tapped(16, 48_000, "test");
        assert!(tx
            .push(Command::SetInput(capture.take_monitor_feed()))
            .is_ok());
        let mut tracks = [MonitorTrack::default(); MAX_MONITORS];
        for (c, t) in tracks.iter_mut().enumerate().take(16) {
            *t = MonitorTrack {
                track: c as u16,
                channel: c as u16,
                stereo: false,
            };
        }
        assert!(tx.push(Command::SetMonitors { tracks, count: 16 }).is_ok());
        let mut buf = vec![0.0f32; 128];
        let block: Vec<f32> = (0..64 * 16).map(|i| 0.01 * (1 + i % 16) as f32).collect();
        for _ in 0..4 {
            tap.feed(&block, 0);
            e.process(&mut buf);
        }
        let st = s.read();
        for t in 0..16 {
            let want = 0.01 * (1 + t) as f32;
            let got = st.track_peaks(t).0;
            assert!(
                (got - want).abs() < 0.002,
                "track {t} hears input {}: {got}",
                t + 1
            );
        }
    }

    /// The duplex path (macOS): input that arrives in a callback is heard in that same
    /// callback's output, one frame later at most.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_tapped_input_is_heard_in_the_same_block() {
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(Vec::new(), vec![TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        let (mut capture, mut tap) = crate::capture::Capture::tapped(1, 48_000, "test");
        let feed = capture.take_monitor_feed();
        assert!(feed.is_some());
        assert!(tx.push(Command::SetInput(feed)).is_ok());
        let mut tracks = [MonitorTrack::default(); MAX_MONITORS];
        tracks[0] = MonitorTrack {
            track: 0,
            channel: 0,
            stereo: false,
        };
        assert!(tx.push(Command::SetMonitors { tracks, count: 1 }).is_ok());
        let mut buf = vec![0.0f32; 128];
        tap.feed(&[0.0; 64], 0);
        e.process(&mut buf);
        for _ in 0..4 {
            let mut input = [0.0f32; 64];
            input[10] = 0.5;
            tap.feed(&input, 0);
            e.process(&mut buf);
            let first = buf
                .as_chunks::<2>()
                .0
                .iter()
                .position(|f| f[0].abs() > 1e-4);
            assert!(
                matches!(first, Some(10 | 11)),
                "the spike at frame 10 is heard at {first:?}"
            );
        }
    }

    #[test]
    fn a_strip_with_a_bus_output_is_heard_only_through_that_bus() {
        let play = |listen: bool| {
            let (mut e, mut tx, _s) = rig();
            let song = Song::new(
                vec![note(0.0, 4.0)],
                vec![TrackMix::default(), TrackMix::default()],
            );
            assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
            let route = Route {
                output: 2,
                ..Route::default()
            };
            assert!(tx.push(Command::SetRoute { track: 0, route }).is_ok());
            if listen {
                let route = Route {
                    bus_in: 2,
                    ..Route::default()
                };
                assert!(tx.push(Command::SetRoute { track: 1, route }).is_ok());
            }
            assert!(tx.push(Command::Play).is_ok());
            let mut buf = vec![0.0f32; 9_600];
            e.process(&mut buf);
            buf.iter().fold(0f32, |m, x| m.max(x.abs()))
        };
        assert!(play(false) < 1e-6, "nothing listens to the bus");
        assert!(play(true) > 0.01, "the Aux plays it");
    }

    #[test]
    fn metronome_clicks_on_beat() {
        let (mut e, mut tx, _s) = rig();
        assert!(tx.push(Command::SetMetronome { on: true }).is_ok());
        assert!(tx.push(Command::SetPosition { beat: 0.99 }).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 4_800 * 2];
        e.process(&mut buf);
        assert!(buf.iter().any(|x| x.abs() > 0.05));
    }

    /// The MIDI clock messages on the MIDI out queue so far (notes are skipped), as
    /// [`ClockMessage`]s, each with the host time it is stamped with.
    fn drain_clock(midi: &mut Consumer<MidiOutEvent>) -> Vec<crate::clock::ClockMessage> {
        use crate::clock::ClockMessage;
        let mut out = Vec::new();
        while let Ok(m) = midi.pop() {
            out.push(match m.system {
                0xF8 => ClockMessage::Tick,
                0xFA => ClockMessage::Start,
                0xFB => ClockMessage::Continue,
                0xFC => ClockMessage::Stop,
                0xF2 => ClockMessage::SongPosition(m.spp),
                _ => continue,
            });
        }
        out
    }

    /// A core with MIDI clock out on, and the queue the clock goes out on.
    fn clock_rig() -> (EngineCore, Producer<Command>, Consumer<MidiOutEvent>) {
        let (mut e, mut tx, _s) = rig();
        let Some(midi) = e.take_midi_out_reader() else {
            panic!("the MIDI out reader");
        };
        assert!(tx.push(Command::SetMidiClock(true)).is_ok());
        (e, tx, midi)
    }

    #[test]
    fn midi_clock_starts_at_the_beginning_ticks_24_per_beat_and_stops() {
        use crate::clock::ClockMessage;
        let (mut e, mut tx, mut clock) = clock_rig();
        assert!(tx.push(Command::Play).is_ok());
        // 120 BPM at 48 kHz: one second is two beats, so 48 ticks.
        let mut buf = vec![0.0f32; 48_000 * 2];
        e.process(&mut buf);
        let msgs = drain_clock(&mut clock);
        assert_eq!(
            &msgs[..2],
            &[ClockMessage::SongPosition(0), ClockMessage::Start]
        );
        let ticks = msgs.iter().filter(|m| **m == ClockMessage::Tick).count();
        assert!((47..=49).contains(&ticks), "{ticks} ticks in two beats");
        assert!(tx.push(Command::Pause).is_ok());
        e.process(&mut buf[..2]);
        assert_eq!(drain_clock(&mut clock), vec![ClockMessage::Stop]);
    }

    #[test]
    fn midi_clock_from_the_middle_of_the_song_sends_the_position_then_continue() {
        use crate::clock::ClockMessage;
        let (mut e, mut tx, mut clock) = clock_rig();
        assert!(tx.push(Command::SetPosition { beat: 4.0 }).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 512];
        e.process(&mut buf);
        let msgs = drain_clock(&mut clock);
        assert_eq!(
            &msgs[..2],
            &[ClockMessage::SongPosition(16), ClockMessage::Continue],
            "bar 2 is sixteenth 16"
        );
        assert!(msgs.iter().skip(2).all(|m| *m == ClockMessage::Tick));
    }

    #[test]
    fn midi_clock_resyncs_when_the_playhead_jumps_while_playing() {
        use crate::clock::ClockMessage;
        let (mut e, mut tx, mut clock) = clock_rig();
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 512];
        e.process(&mut buf);
        drain_clock(&mut clock);
        assert!(tx.push(Command::SetPosition { beat: 8.0 }).is_ok());
        e.process(&mut buf);
        let msgs = drain_clock(&mut clock);
        assert_eq!(
            &msgs[..3],
            &[
                ClockMessage::Stop,
                ClockMessage::SongPosition(32),
                ClockMessage::Continue
            ]
        );
        let ticks = msgs.iter().filter(|m| **m == ClockMessage::Tick).count();
        assert!(ticks <= 2, "no burst of ticks for the jump: {ticks}");
    }

    #[test]
    fn midi_clock_off_sends_nothing() {
        let (mut e, mut tx, _s) = rig();
        let Some(mut midi) = e.take_midi_out_reader() else {
            panic!("the MIDI out reader");
        };
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 4_800];
        e.process(&mut buf);
        assert!(drain_clock(&mut midi).is_empty());
    }

    /// The loudest sample of one second of the click on a bar's first beat.
    fn click_peak(commands: Vec<Command>) -> f32 {
        let (mut e, mut tx, _s) = rig();
        assert!(tx.push(Command::SetMetronome { on: true }).is_ok());
        for c in commands {
            assert!(tx.push(c).is_ok());
        }
        assert!(tx.push(Command::SetPosition { beat: 0.99 }).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 4_800 * 2];
        e.process(&mut buf);
        buf.iter().fold(0f32, |m, x| m.max(x.abs()))
    }

    #[test]
    fn click_volume_follows_the_setting() {
        let loud = click_peak(vec![]);
        let quiet = click_peak(vec![Command::SetClick {
            gain_db: -20.0,
            accent: true,
        }]);
        assert!(loud > 0.05, "{loud}");
        // The click starts at 0.3 (-10.5 dB), so -20 dB is a third of it.
        assert!((quiet / loud - 0.1 / 0.3).abs() < 0.03, "{quiet} / {loud}");
        // A level that is not a number leaves the last one.
        let kept = click_peak(vec![Command::SetClick {
            gain_db: f32::NAN,
            accent: true,
        }]);
        assert!((kept / loud - 1.0).abs() < 0.03, "{kept} / {loud}");
    }

    #[test]
    fn the_accent_can_be_switched_off() {
        for (accent, want) in [(true, 1500.0), (false, 1000.0)] {
            let (mut e, mut tx, _s) = rig();
            assert!(tx.push(Command::SetMetronome { on: true }).is_ok());
            assert!(tx
                .push(Command::SetClick {
                    gain_db: -10.0,
                    accent
                })
                .is_ok());
            // Just before the first beat of bar 2.
            assert!(tx.push(Command::SetPosition { beat: 3.999 }).is_ok());
            assert!(tx.push(Command::Play).is_ok());
            let mut buf = vec![0.0f32; 64 * 2];
            e.process(&mut buf);
            assert!(e.click_remaining > 0);
            assert_eq!(e.click_freq, want, "accent {accent}");
        }
    }

    /// Zero crossings of the left channel while an A4 note plays for half a second.
    fn crossings_of_a4(tuning: Option<f32>) -> usize {
        let (mut e, mut tx, _s) = rig();
        if let Some(a4_hz) = tuning {
            assert!(tx.push(Command::SetTuning { a4_hz }).is_ok());
        }
        let song = Song::new(vec![note(0.0, 4.0)], vec![TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 24_000 * 2];
        e.process(&mut buf);
        let left: Vec<f32> = buf.iter().step_by(2).copied().collect();
        left.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    #[test]
    fn tuning_moves_the_pitch_of_an_instrument() {
        let default = crossings_of_a4(None);
        assert!(default > 100, "{default}");
        assert_eq!(crossings_of_a4(Some(440.0)), default);
        let high = crossings_of_a4(Some(453.0)) as f32;
        let low = crossings_of_a4(Some(427.0)) as f32;
        // 453 Hz over 427 Hz is 6.1 % higher.
        let ratio = high / low;
        assert!((ratio - 453.0 / 427.0).abs() < 0.02, "ratio {ratio}");
        // Out of Logic's range it is held at the range's end; not a number is ignored.
        assert_eq!(crossings_of_a4(Some(1000.0)) as f32, high);
        assert_eq!(crossings_of_a4(Some(f32::NAN)), default);
    }

    #[test]
    fn hard_left_pan_silences_right_and_meters_track() {
        let (mut e, mut tx, mut s) = rig();
        let mix = TrackMix {
            pan: -1.0,
            ..TrackMix::default()
        };
        let song = Song::new(vec![note(0.0, 2.0)], vec![mix]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 4800 * 2];
        e.process(&mut buf);
        let right = buf
            .iter()
            .skip(1)
            .step_by(2)
            .fold(0f32, |m, x| m.max(x.abs()));
        let left = buf.iter().step_by(2).fold(0f32, |m, x| m.max(x.abs()));
        assert!(right < 1e-6, "right {right}");
        assert!(left > 0.05, "left {left}");
        let (tl, tr) = s.read().track_peaks(0);
        assert!(tl > 0.05 && tr < 1e-6, "meters {tl} {tr}");
    }

    #[test]
    fn live_note_sounds_while_stopped_and_stops_on_note_off() {
        let (mut e, mut tx, mut s) = rig();
        assert!(tx
            .push(Command::SetSong(Box::new(Song::new(
                vec![],
                vec![TrackMix::default()]
            ))))
            .is_ok());
        assert!(tx
            .push(Command::NoteOn {
                pitch: 69,
                vel: 100,
                track: 0
            })
            .is_ok());
        let mut buf = vec![0.0f32; 4800 * 2];
        e.process(&mut buf);
        assert!(buf.iter().any(|x| x.abs() > 0.05), "no sound while stopped");
        assert!(!s.read().playing);
        // A seek must not cut a live note.
        assert!(tx.push(Command::SetPosition { beat: 1.0 }).is_ok());
        e.process(&mut buf);
        assert!(
            buf.iter().any(|x| x.abs() > 0.05),
            "seek killed the live note"
        );
        assert!(tx.push(Command::NoteOff { pitch: 69 }).is_ok());
        e.process(&mut buf); // 100 ms: release is 90 ms
        e.process(&mut buf);
        assert!(buf.iter().all(|x| x.abs() < 1e-4), "note did not release");
    }

    #[test]
    fn live_note_releases_by_itself_after_four_seconds() {
        let (mut e, mut tx, _s) = rig();
        assert!(tx
            .push(Command::NoteOn {
                pitch: 60,
                vel: 100,
                track: 0
            })
            .is_ok());
        let mut buf = vec![0.0f32; 48_000 * 2];
        for _ in 0..3 {
            e.process(&mut buf);
        }
        assert!(
            buf.iter().any(|x| x.abs() > 0.05),
            "should still sound at 3 s"
        );
        // The safety release starts at 4.0 s and fades over 90 ms: silent from the 6th second.
        for _ in 0..3 {
            e.process(&mut buf);
        }
        assert!(
            buf.iter().all(|x| x.abs() < 1e-4),
            "should be silent after 6 s"
        );
    }

    #[test]
    fn live_note_marks_its_own_voice_and_preserves_timeline_voice() {
        let (mut e, _tx, _s) = rig();
        let timeline = e.trigger(note(0.0, 8.0));
        e.live_note_on(69, 100, 1);
        assert!(!e.voices[timeline].live);
        assert!(e.voices[1].live);
        assert_eq!(e.voices[1].track, 1);
        // Fill the remaining pool, then exercise the voice-stealing path.
        for _ in 2..MAX_VOICES {
            e.trigger(note(0.0, 8.0));
        }
        e.live_note_on(72, 100, 2);
        assert!(e.voices[0].live);
        assert_eq!((e.voices[0].pitch, e.voices[0].track), (72, 2));
        assert_eq!(e.voices.iter().filter(|v| v.active).count(), MAX_VOICES);
    }

    #[test]
    fn scoped_note_off_preserves_other_tracks_and_timeline_notes() {
        let (mut e, mut tx, _s) = rig();
        let timeline = e.trigger(note(0.0, 8.0));
        e.live_note_on(69, 100, 0);
        e.live_note_on(69, 100, 1);
        e.live_note_on(72, 100, 0);
        assert!(tx
            .push(Command::NoteOffTrack {
                pitch: 69,
                track: 0
            })
            .is_ok());
        assert_no_alloc::assert_no_alloc(|| e.process(&mut []));
        assert!(!e.voices[timeline].releasing);
        assert!(e.voices[1].releasing);
        assert!(!e.voices[2].releasing);
        assert!(!e.voices[3].releasing);
        assert!(tx
            .push(Command::NoteOffTrack {
                pitch: 69,
                track: 255
            })
            .is_ok());
        assert_no_alloc::assert_no_alloc(|| e.process(&mut []));
        assert!(!e.voices[2].releasing);
        // Existing callers retain the global-release behavior.
        assert!(tx.push(Command::NoteOff { pitch: 69 }).is_ok());
        assert_no_alloc::assert_no_alloc(|| e.process(&mut []));
        assert!(e.voices[2].releasing);
        assert!(!e.voices[timeline].releasing);
        assert!(!e.voices[3].releasing);
    }

    fn play_note_with(patch: SynthPatch, pitch: u8, frames: usize) -> Vec<f32> {
        let (mut e, mut tx, _s) = rig();
        assert!(tx
            .push(Command::SetSong(Box::new(Song::new(
                vec![],
                vec![TrackMix::default()]
            ))))
            .is_ok());
        assert!(tx.push(Command::SetTrackPatch { track: 0, patch }).is_ok());
        assert!(tx
            .push(Command::NoteOn {
                pitch,
                vel: 110,
                track: 0
            })
            .is_ok());
        let mut buf = vec![0.0f32; frames * 2];
        e.process(&mut buf);
        buf.iter().step_by(2).copied().collect()
    }

    #[test]
    fn slow_attack_patch_starts_quieter_than_the_default() {
        let slow = SynthPatch {
            attack: 0.4,
            ..SynthPatch::default()
        };
        let early = |x: &[f32]| x[..480].iter().map(|v| v * v).sum::<f32>();
        let fast = play_note_with(SynthPatch::default(), 69, 4800);
        let slow = play_note_with(slow, 69, 4800);
        assert!(
            early(&slow) < early(&fast) * 0.05,
            "{} vs {}",
            early(&slow),
            early(&fast)
        );
    }

    /// A GitHub report: piano patches (no resonance, a filter envelope that opens the cutoff
    /// to its limit) made the filter diverge, and the track's peak meter stuck at `inf`.
    #[test]
    fn a_fully_opened_filter_without_resonance_stays_finite() {
        use crate::synth::Wave;
        let patch = SynthPatch {
            wave_a: Wave::Saw,
            cutoff: 4000.0,
            resonance: 0.0,
            filter_env: 1.0,
            ..SynthPatch::default()
        };
        for pitch in [48, 72, 96] {
            let out = play_note_with(patch, pitch, 48_000);
            assert!(out.iter().all(|x| x.is_finite()), "pitch {pitch}");
            assert!(out.iter().any(|x| x.abs() > 0.01), "pitch {pitch} silent");
        }
    }

    #[test]
    fn stuck_or_bad_peaks_fall_back_to_finite_values() {
        assert_eq!(decayed_peak(f32::INFINITY, 0.5), 0.0);
        assert_eq!(decayed_peak(0.2, f32::INFINITY), 0.0);
        assert_eq!(
            decayed_peak(f32::NAN, 0.5),
            0.5 * 0.85,
            "NaN is ignored by max"
        );
        assert_eq!(decayed_peak(0.0, 1.0), 0.85, "decays");
        assert_eq!(finite_or_zero(f32::NAN), 0.0);
        assert_eq!(finite_or_zero(-0.25), -0.25);
    }

    #[test]
    fn the_stability_limit_holds_for_every_resonance() {
        for r in [0.0, 0.05, 0.3, 1.0] {
            let d = Svf::damping(r);
            let f = Svf::max_coefficient(d);
            assert!(f * f + 2.0 * f * d < 4.0, "resonance {r}");
            assert!(f > 0.9, "the limit leaves the filter bright: {f}");
        }
    }

    #[test]
    fn a_dark_filtered_saw_is_smoother_than_a_bright_one() {
        use crate::synth::Wave;
        let base = SynthPatch {
            wave_a: Wave::Saw,
            level_b: 0.0,
            ..SynthPatch::default()
        };
        let dark = SynthPatch {
            cutoff: 400.0,
            ..base
        };
        let rough = |x: &[f32]| {
            x.windows(3)
                .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
                .sum::<f32>()
        };
        let a = play_note_with(base, 57, 9600);
        let b = play_note_with(dark, 57, 9600);
        assert!(rough(&b[2400..]) < rough(&a[2400..]) * 0.6);
        assert!(b.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn drum_kit_notes_sound_different_and_decay() {
        let kit = SynthPatch {
            kit: Kit::Acoustic,
            ..SynthPatch::default()
        };
        let kick = play_note_with(kit, 36, 24_000);
        let hat = play_note_with(kit, 42, 24_000);
        assert!(kick.iter().any(|x| x.abs() > 0.05), "kick silent");
        assert!(hat.iter().any(|x| x.abs() > 0.02), "hat silent");
        // Both are short: the last quarter second is quiet.
        let tail = |x: &[f32]| x[18_000..].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(tail(&kick) < 0.02 && tail(&hat) < 0.02);
        assert!(kick != hat);
    }

    /// Roadmap §5 acceptance: zero allocations/frees on the audio thread.
    #[test]
    fn process_does_not_allocate() {
        let (mut e, mut tx, _s) = rig();
        let notes = (0..64)
            .map(|i| note(i as f64 * 0.25, i as f64 * 0.25 + 0.5))
            .collect();
        let song = Song::new(notes, vec![TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        let mut buf = vec![0.0f32; 512 * 2];
        let cmds = [
            Command::SetMetronome { on: true },
            Command::SetCycle {
                on: true,
                start_beat: 0.0,
                end_beat: 4.0,
            },
            Command::SetTestTone {
                hz: 440.0,
                gain: 0.1,
            },
            Command::SetTrackMix {
                track: 0,
                mix: TrackMix {
                    gain_db: -6.0,
                    mute: false,
                    solo: true,
                    pan: 0.0,
                },
            },
            Command::SetTempoMap(Box::new(TempoMap::constant(140.0, (3, 4)))),
            Command::NoteOn {
                pitch: 64,
                vel: 90,
                track: 0,
            },
            Command::SetTrackPatch {
                track: 0,
                patch: SynthPatch {
                    kit: Kit::Acoustic,
                    ..SynthPatch::default()
                },
            },
            Command::NoteOn {
                pitch: 42,
                vel: 90,
                track: 0,
            },
            Command::Play,
        ];
        for c in cmds {
            assert!(tx.push(c).is_ok());
        }
        for i in 0..400 {
            if i == 200 {
                assert!(tx.push(Command::SetPosition { beat: 2.0 }).is_ok());
            }
            assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        }
        assert!(tx.push(Command::Stop).is_ok());
        assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        assert!(buf.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn cycle_loops_back() {
        let (mut e, mut tx, mut s) = rig();
        assert!(tx
            .push(Command::SetCycle {
                on: true,
                start_beat: 0.0,
                end_beat: 1.0
            })
            .is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 48_000 * 2];
        e.process(&mut buf);
        assert!(s.read().position_beats < 1.0);
    }

    #[test]
    fn midi_note_does_not_start_one_sample_early() {
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(vec![note(1.0 / 24_000.0, 1.0)], vec![TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut frame = [0.0; 2];
        e.process(&mut frame);
        assert_eq!(frame, [0.0; 2]);
        assert_eq!(e.next_note, 0);
        e.process(&mut frame);
        assert_eq!(e.next_note, 1);
        assert!(frame[0].abs() > 0.0);
    }

    #[test]
    fn fractional_cycle_wrap_retriggers_note_at_cycle_start() {
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(vec![note(0.0, 1.0)], vec![TrackMix::default()]);
        for cmd in [
            Command::SetSong(Box::new(song)),
            Command::SetCycle {
                on: true,
                start_beat: 0.0,
                end_beat: 2.5 / 24_000.0,
            },
            Command::Play,
        ] {
            assert!(tx.push(cmd).is_ok());
        }
        e.process(&mut [0.0; 6]);
        assert!(e.beat > 0.0, "fractional remainder preserved");
        assert_eq!(e.next_note, 0, "cycle-start note must not be skipped");
        let mut frame = [0.0; 2];
        e.process(&mut frame);
        assert_eq!(e.next_note, 1);
        assert!(frame[0].abs() > 0.0);
    }

    #[test]
    fn invalid_cycle_bounds_are_ignored() {
        let (mut e, mut tx, _s) = rig();
        for (start, end) in [
            (0.0, f64::INFINITY),
            (f64::NAN, 1.0),
            (-1.0, 1.0),
            (1.0, 1.0),
        ] {
            assert!(tx
                .push(Command::SetCycle {
                    on: true,
                    start_beat: start,
                    end_beat: end
                })
                .is_ok());
            e.process(&mut []);
            assert!(e.cycle.is_none());
        }
    }

    #[test]
    fn dsp_load_is_busy_time_over_buffer_time() {
        use std::time::Duration;
        // 256 frames at 48 kHz play for 5.333 ms; 1.333 ms of work is a quarter of that.
        let load = dsp_load(Duration::from_micros(1_333), 256, 48_000);
        assert!((load - 0.25).abs() < 0.001, "{load}");
        assert_eq!(dsp_load(Duration::from_millis(1), 0, 48_000), 0.0);
    }

    #[test]
    fn dsp_load_meter_keeps_the_peak_until_read() {
        let s = Shared::new();
        for load in [0.2, 0.7, 0.3] {
            assert_no_alloc::assert_no_alloc(|| s.record_dsp_load(load));
        }
        assert_eq!(s.take_dsp_load(), 0.7);
        assert_eq!(s.take_dsp_load(), 0.0);
        s.record_dsp_load(1.5);
        assert_eq!(s.take_dsp_load(), 1.5, "overloads above 100% are reported");
    }

    #[test]
    fn transport_follows_tempo_changes() {
        use crate::tempo_map::TempoEvent;
        let (mut e, mut tx, mut s) = rig();
        // 120 BPM for the first beat (0.5 s), then 60 BPM.
        let map = TempoMap::new(
            vec![
                TempoEvent {
                    beat: 0.0,
                    bpm: 120.0,
                    curve: crate::tempo_map::TempoCurve::Step,
                },
                TempoEvent {
                    beat: 1.0,
                    bpm: 60.0,
                    curve: crate::tempo_map::TempoCurve::Step,
                },
            ],
            vec![],
        );
        assert!(tx.push(Command::SetTempoMap(Box::new(map))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 48_000 * 2];
        e.process(&mut buf);
        assert!(
            (s.read().position_beats - 1.5).abs() < 1e-6,
            "{}",
            s.read().position_beats
        );
    }

    #[test]
    fn metronome_accents_the_first_beat_of_each_bar_in_three_four() {
        let (mut e, mut tx, _s) = rig();
        let map = TempoMap::constant(120.0, (3, 4));
        assert!(tx.push(Command::SetTempoMap(Box::new(map))).is_ok());
        assert!(tx.push(Command::SetMetronome { on: true }).is_ok());
        let mut buf = vec![0.0f32; 64 * 2];
        let mut click_at = |e: &mut EngineCore, tx: &mut Producer<Command>, beat: f64| {
            assert!(tx.push(Command::SetPosition { beat: beat - 0.001 }).is_ok());
            assert!(tx.push(Command::Play).is_ok());
            e.process(&mut buf);
            assert!(e.click_remaining > 0, "no click at beat {beat}");
            e.click_freq
        };
        assert_eq!(click_at(&mut e, &mut tx, 2.0), 1000.0);
        assert_eq!(
            click_at(&mut e, &mut tx, 3.0),
            1500.0,
            "bar 2 starts at beat 3"
        );
        assert_eq!(click_at(&mut e, &mut tx, 4.0), 1000.0, "not a 4/4 downbeat");
    }

    #[test]
    fn set_notes_swaps_notes_without_cutting_voices() {
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(vec![note(0.0, 8.0)], vec![TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 4_800 * 2];
        e.process(&mut buf);
        assert_eq!(e.voices.iter().filter(|v| v.active).count(), 1);
        // Unsorted on purpose: `set_notes` sorts.
        let notes = vec![note(1.0, 2.0), note(0.05, 0.5), note(0.5, 1.0)];
        assert!(tx.push(Command::set_notes(notes)).is_ok());
        assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        assert_eq!(e.song.notes.len(), 3);
        assert!(e
            .song
            .notes
            .windows(2)
            .all(|w| w[0].start_beat <= w[1].start_beat));
        // The held note survives; the next note to play is the one at beat 0.5.
        assert!(e.voices.iter().any(|v| v.active && v.end_beat == 8.0));
        assert_eq!(e.song.notes[e.next_note].start_beat, 0.5);
    }

    #[test]
    fn solo_plays_only_soloed_tracks_until_released() {
        let (mut e, mut tx, mut s) = rig();
        let on = |track| NoteEvent {
            start_beat: 0.0,
            end_beat: 8.0,
            pitch: 60,
            vel: 100,
            track,
        };
        let soloed = TrackMix {
            solo: true,
            ..TrackMix::default()
        };
        let song = Song::new(vec![on(0), on(1)], vec![TrackMix::default(), soloed]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let mut buf = vec![0.0f32; 4_800 * 2];
        e.process(&mut buf);
        assert_eq!(s.read().track_peaks(0), (0.0, 0.0), "not soloed: silent");
        assert!(s.read().track_peaks(1).0 > 0.0, "soloed: audible");

        // Releasing solo brings the other track back.
        let mix = TrackMix::default();
        assert!(tx.push(Command::SetTrackMix { track: 1, mix }).is_ok());
        e.process(&mut buf);
        assert!(s.read().track_peaks(0).0 > 0.0 && s.read().track_peaks(1).0 > 0.0);
    }

    fn two_tracks_song(notes: Vec<NoteEvent>) -> Box<Song> {
        Box::new(Song::new(
            notes,
            vec![TrackMix::default(), TrackMix::default()],
        ))
    }

    fn strip_note(track: u16, end: f64) -> NoteEvent {
        NoteEvent {
            track,
            ..note(0.0, end)
        }
    }

    fn render(e: &mut EngineCore, secs: f32) -> Vec<f32> {
        let mut buf = vec![0.0f32; (secs * 48_000.0) as usize * 2];
        for chunk in buf.chunks_mut(512 * 2) {
            e.process(chunk);
        }
        buf
    }

    fn energy(buf: &[f32], from: f32, to: f32) -> f32 {
        let (a, b) = ((from * 48_000.0) as usize * 2, (to * 48_000.0) as usize * 2);
        buf[a..b.min(buf.len())].iter().map(|x| x * x).sum()
    }

    fn effect(kind: crate::fx::FxKind, set: &[(usize, f32)]) -> Box<Chain> {
        let mut p = kind.defaults();
        for (i, v) in set {
            p[*i] = *v;
        }
        Box::new(Chain::new(vec![crate::fx::Fx::new(
            kind, p, false, 48_000.0,
        )]))
    }

    #[test]
    fn an_effect_on_a_strip_changes_what_is_heard() {
        use crate::fx::FxKind;
        let play = |chain: Option<Box<Chain>>| {
            let (mut e, mut tx, _s) = rig();
            assert!(tx
                .push(Command::SetSong(two_tracks_song(vec![strip_note(0, 0.25)])))
                .is_ok());
            if let Some(chain) = chain {
                assert!(tx.push(Command::SetInserts { track: 0, chain }).is_ok());
            }
            assert!(tx.push(Command::Play).is_ok());
            render(&mut e, 0.8)
        };
        let dry = play(None);
        // A 150 ms delay, all wet: the note comes back after it ended (0.125 s at 120 BPM).
        let wet = play(Some(effect(
            FxKind::Delay,
            &[(0, 150.0), (1, 0.0), (4, 1.0)],
        )));
        assert!(
            energy(&dry, 0.45, 0.8) < 1e-6,
            "{}",
            energy(&dry, 0.45, 0.8)
        );
        assert!(energy(&wet, 0.2, 0.5) > 0.05, "{}", energy(&wet, 0.2, 0.5));
        assert!(wet.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn a_send_feeds_the_aux_strip_that_listens_to_the_bus() {
        use crate::fx::FxKind;
        let play = |send_pre: bool, aux_muted: bool, level: f32| {
            let (mut e, mut tx, mut s) = rig();
            let mut mixes = vec![TrackMix::default(), TrackMix::default()];
            // The note's own fader is all the way down: only a pre-fader send is heard.
            mixes[0].gain_db = -96.0;
            mixes[1].mute = aux_muted;
            let song = Song::new(vec![strip_note(0, 2.0)], mixes);
            assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
            let mut route = Route::default();
            route.sends[0] = SendSpec {
                bus: 1,
                level,
                pre: send_pre,
            };
            assert!(tx.push(Command::SetRoute { track: 0, route }).is_ok());
            let aux = Route {
                bus_in: 1,
                ..Route::default()
            };
            assert!(tx
                .push(Command::SetRoute {
                    track: 1,
                    route: aux
                })
                .is_ok());
            assert!(tx
                .push(Command::SetInserts {
                    track: 1,
                    chain: effect(FxKind::ChannelEq, &[]),
                })
                .is_ok());
            assert!(tx.push(Command::Play).is_ok());
            let buf = render(&mut e, 0.3);
            let (a0l, a0r) = s.read().track_peaks(0);
            let (a1l, a1r) = s.read().track_peaks(1);
            (energy(&buf, 0.0, 0.3), a0l.max(a0r), a1l.max(a1r))
        };
        let (pre, strip, aux) = play(true, false, 0.7);
        assert!(
            pre > 0.01 && strip < 1e-3 && aux > 0.05,
            "{pre} {strip} {aux}"
        );
        // After the fader the send carries nothing (the fader is down).
        let (post, _, aux_post) = play(false, false, 0.7);
        assert!(post < 1e-5 && aux_post < 1e-4, "{post} {aux_post}");
        // A muted Aux strip is silent; a send at zero sends nothing.
        assert!(play(true, true, 0.7).0 < 1e-5);
        assert!(play(true, false, 0.0).0 < 1e-5);
        // The send level counts: half the knob is about 6 dB under the top.
        let top = play(true, false, 1.0).0;
        let half = play(true, false, 0.5).0;
        assert!(half < top * 0.2 && half > top * 0.01, "{half} {top}");
    }

    #[test]
    fn automation_plays_the_fader_and_stops_when_it_is_not_followed() {
        let level = |lane: Option<(bool, f32, f32)>| {
            let (mut e, mut tx, _s) = rig();
            assert!(tx
                .push(Command::SetSong(two_tracks_song(vec![strip_note(0, 4.0)])))
                .is_ok());
            if let Some((follow, db, offset)) = lane {
                let lane = AutoLane {
                    track: 0,
                    target: AutoTarget::Volume,
                    follow,
                    offset_db: offset,
                    lo: 0.0,
                    hi: 1.0,
                    stepped: false,
                    points: vec![AutoPoint::new(0.0, db), AutoPoint::new(100.0, db)],
                };
                assert!(tx.push(Command::SetAutomation(vec![lane])).is_ok());
            }
            assert!(tx.push(Command::Play).is_ok());
            let buf = render(&mut e, 0.4);
            energy(&buf, 0.2, 0.4)
        };
        let plain = level(None);
        let quiet = level(Some((true, -20.0, 0.0)));
        let ignored = level(Some((false, -20.0, 0.0)));
        // -20 dB is a tenth of the level, a hundredth of the energy.
        assert!((quiet / plain - 0.01).abs() < 0.004, "{}", quiet / plain);
        assert!((ignored / plain - 1.0).abs() < 1e-3);
        // The VCA's level is added to the lane's value.
        let with_vca = level(Some((true, -14.0, -6.0)));
        assert!(
            (with_vca / quiet - 1.0).abs() < 0.03,
            "{}",
            with_vca / quiet
        );
    }

    /// Track 0 plays a constant 1.0 (a stub plug-in instrument) through a fader at 0 dB, with
    /// the master at unity: what comes out is the strip's gain.
    fn constant_strip(
        effect: Option<crate::plugin_track::tests::Gain>,
    ) -> (EngineCore, Producer<Command>) {
        use crate::plugin_track::tests::Gain;
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(vec![note(0.0, 400.0)], vec![TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        let effects: Vec<Box<dyn crate::BlockProcessor>> = match effect {
            Some(g) => vec![Box::new(g)],
            None => Vec::new(),
        };
        let p = PluginTrack::new(Some(Box::new(Gain::new(true, false))), effects, 512);
        assert!(tx
            .push(Command::SetPlugins {
                track: 0,
                plugins: Some(Box::new(p)),
            })
            .is_ok());
        assert!(tx.push(Command::SetMasterGain { linear: 1.0 }).is_ok());
        e.process(&mut [0.0; 2]);
        (e, tx)
    }

    #[test]
    fn volume_automation_lands_on_its_sample() {
        // Ableton: automation is read sample by sample; here every 32 frames, and the fader
        // ramps so it is on the lane at each reading (no lag, no zipper noise).
        let (mut e, mut tx) = constant_strip(None);
        // 120 bpm: a beat is 24 000 frames. A drop to silence just before beat 2 (48 000), and
        // a straight line in dB from 0 to -40 over the first beat.
        let lane = AutoLane::new(
            0,
            AutoTarget::Volume,
            vec![
                AutoPoint::new(0.0, 0.0),
                AutoPoint::new(1.0, -40.0),
                AutoPoint::new(1.5, 0.0),
                AutoPoint::new(1.9999, 0.0),
                AutoPoint::new(1.9999, -96.0),
            ],
        );
        assert!(tx.push(Command::SetAutomation(vec![lane])).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        // The first frame rendered already plays (the commands go in at the call's start).
        let buf = render(&mut e, 1.1);
        let at = |n: usize| buf[n * 2];
        // The plug-in is heard a block (512 frames) late, but its sound is constant.
        for (n, db) in [(6_000usize, -10.0f32), (12_000, -20.0), (18_000, -30.0)] {
            let want = 10f32.powf(db / 20.0);
            assert!((at(n) / want - 1.0).abs() < 0.01, "{n}: {} {want}", at(n));
        }
        assert!((at(40_000) - 1.0).abs() < 1e-4, "{}", at(40_000));
        assert!(
            (at(47_960) - 1.0).abs() < 1e-4,
            "before the drop: {}",
            at(47_960)
        );
        // The drop ramps over the 32 frames before the reading that finds it, so it is
        // complete on frame 48 000 itself.
        assert!(
            at(47_984) < 0.6 && at(47_984) > 0.4,
            "mid-ramp: {}",
            at(47_984)
        );
        assert!(at(48_000) < 1e-4, "on the beat: {}", at(48_000));
        assert!(at(52_000) < 1e-4);
    }

    #[test]
    fn mute_send_master_and_plugin_lanes_are_played() {
        use crate::plugin_track::tests::Gain;
        // Mute: a lane at 1 silences the strip, ramping (no click).
        let (mut e, mut tx) = constant_strip(None);
        let mute = AutoLane::new(
            0,
            AutoTarget::Mute,
            vec![
                AutoPoint::new(0.0, 0.0),
                AutoPoint::new(0.5, 0.0),
                AutoPoint::new(0.5, 1.0),
            ],
        );
        assert!(tx.push(Command::SetAutomation(vec![mute])).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let buf = render(&mut e, 0.5);
        assert!((buf[6_000 * 2] - 1.0).abs() < 1e-4);
        assert!(buf[13_000 * 2].abs() < 1e-6, "{}", buf[13_000 * 2]);

        // The Stereo Out fader: -20 dB from its lane rules over the master gain.
        let (mut e, mut tx) = constant_strip(None);
        let master = AutoLane::new(
            MASTER_TRACK,
            AutoTarget::Volume,
            vec![AutoPoint::new(0.0, -20.0)],
        );
        assert!(tx
            .push(Command::SetAutomation(vec![master.clone()]))
            .is_ok());
        assert!(tx.push(Command::SetMasterGain { linear: 2.0 }).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let buf = render(&mut e, 0.2);
        assert!((buf[6_000 * 2] - 0.1).abs() < 1e-3, "{}", buf[6_000 * 2]);
        // When the lane stops the user's master comes back.
        let mut off = master;
        off.follow = false;
        assert!(tx.push(Command::SetAutomation(vec![off])).is_ok());
        let buf = render(&mut e, 0.1);
        assert!(
            (buf[3_000 * 2] - 2.0f32.min(1.0)).abs() < 1e-3,
            "{}",
            buf[3_000 * 2]
        );

        // A plug-in effect's parameter: position 0.5 of 0..0.5 is a gain of 0.25.
        let fx = Gain::new(false, false);
        let log = fx.log.clone();
        let (mut e, mut tx) = constant_strip(Some(fx));
        let mut plug = AutoLane::new(
            0,
            AutoTarget::Plugin { slot: 1, param: 0 },
            vec![AutoPoint::new(0.0, 0.5)],
        );
        (plug.lo, plug.hi) = (0.0, 0.5);
        assert!(tx.push(Command::SetAutomation(vec![plug])).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let buf = render(&mut e, 0.2);
        assert!((buf[6_000 * 2] - 0.25).abs() < 1e-4, "{}", buf[6_000 * 2]);
        // Only changes are sent: one value, once.
        let told = log.lock().map(|l| l.told.clone()).unwrap_or_default();
        assert_eq!(told, [(0, 0.25, u32::MAX)]);

        // A send: the lane at 0 empties a bus the strip feeds at full level.
        let (mut e, mut tx) = constant_strip(None);
        let mut mixes = vec![TrackMix::default(), TrackMix::default()];
        mixes[0].gain_db = -96.0;
        let song = Song::new(vec![note(0.0, 400.0)], mixes);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        let mut route = Route::default();
        route.sends[0] = SendSpec {
            bus: 1,
            level: 1.0,
            pre: true,
        };
        assert!(tx.push(Command::SetRoute { track: 0, route }).is_ok());
        let aux = Route {
            bus_in: 1,
            ..Route::default()
        };
        assert!(tx
            .push(Command::SetRoute {
                track: 1,
                route: aux
            })
            .is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let loud = energy(&render(&mut e, 0.2), 0.1, 0.2);
        let send = AutoLane::new(
            0,
            AutoTarget::Send { slot: 0 },
            vec![AutoPoint::new(0.0, 0.0)],
        );
        assert!(tx.push(Command::SetAutomation(vec![send])).is_ok());
        let quiet = energy(&render(&mut e, 0.2), 0.1, 0.2);
        // (What is left is the strip's own fader at -96 dB.)
        assert!(loud > 1.0 && quiet < 1e-5, "{loud} {quiet}");
        // And none of it allocates.
        let mut buf = vec![0.0f32; 512 * 2];
        assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
    }

    #[test]
    fn pan_automation_moves_the_sound_and_a_compressor_reports_its_reduction() {
        use crate::fx::FxKind;
        let (mut e, mut tx, mut s) = rig();
        assert!(tx
            .push(Command::SetSong(two_tracks_song(vec![strip_note(0, 4.0)])))
            .is_ok());
        let lane = AutoLane {
            track: 0,
            target: AutoTarget::Pan,
            follow: true,
            offset_db: 0.0,
            lo: 0.0,
            hi: 1.0,
            stepped: false,
            points: vec![AutoPoint::new(0.0, 1.0)],
        };
        assert!(tx.push(Command::SetAutomation(vec![lane])).is_ok());
        assert!(tx
            .push(Command::SetInserts {
                track: 0,
                chain: effect(FxKind::Compressor, &[(0, -40.0), (1, 10.0), (2, 1.0)]),
            })
            .is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let buf = render(&mut e, 0.3);
        let left: f32 = buf.iter().step_by(2).skip(2400).map(|x| x * x).sum();
        let right: f32 = buf
            .iter()
            .skip(1)
            .step_by(2)
            .skip(2400)
            .map(|x| x * x)
            .sum();
        assert!(left < right * 1e-4 && right > 0.01, "{left} {right}");
        assert!(
            s.read().gain_reduction(0) > 3.0,
            "{}",
            s.read().gain_reduction(0)
        );
        assert_eq!(s.read().gain_reduction(1), 0.0);
        // Taking the effects away clears the meter.
        assert!(tx
            .push(Command::SetInserts {
                track: 0,
                chain: Box::default(),
            })
            .is_ok());
        render(&mut e, 0.05);
        assert_eq!(s.read().gain_reduction(0), 0.0);
    }

    #[test]
    fn effects_sends_and_automation_do_not_allocate_and_survive_bad_commands() {
        use crate::fx::FxKind;
        let (mut e, mut tx, _s) = rig();
        assert!(tx
            .push(Command::SetSong(two_tracks_song(vec![strip_note(0, 8.0)])))
            .is_ok());
        let aux = Route {
            bus_in: 2,
            ..Route::default()
        };
        let mut route = Route::default();
        route.sends[0] = SendSpec {
            bus: 2,
            level: 0.8,
            pre: false,
        };
        let lane = AutoLane {
            track: 0,
            target: AutoTarget::Volume,
            follow: true,
            offset_db: 0.0,
            lo: 0.0,
            hi: 1.0,
            stepped: false,
            points: vec![AutoPoint::new(0.0, 0.0), AutoPoint::new(4.0, -12.0)],
        };
        let cmds = vec![
            Command::SetInserts {
                track: 0,
                chain: effect(FxKind::Reverb, &[]),
            },
            Command::SetInserts {
                track: 1,
                chain: effect(FxKind::Phaser, &[]),
            },
            Command::SetRoute { track: 0, route },
            Command::SetRoute {
                track: 1,
                route: aux,
            },
            Command::SetAutomation(vec![lane.clone()]),
            Command::SetFx {
                track: 0,
                slot: 0,
                params: [f32::NAN; MAX_FX_PARAMS],
                bypass: false,
            },
            Command::SetFx {
                track: 700,
                slot: 9,
                params: [1.0; MAX_FX_PARAMS],
                bypass: true,
            },
            Command::SetRoute {
                track: 5000,
                route: Route::default(),
            },
            Command::SetAutomation(vec![AutoLane {
                track: 4000,
                ..lane
            }]),
            Command::Play,
        ];
        for c in cmds {
            assert!(tx.push(c).is_ok());
        }
        let mut buf = vec![0.0f32; 512 * 2];
        for _ in 0..300 {
            assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
            assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        }
    }

    #[test]
    fn an_effect_parameter_follows_its_lane_and_goes_back_when_the_lane_stops() {
        use crate::fx::FxKind;
        // A Channel EQ with its mid band at +12 dB: the lane (position 0 = -18 dB) pulls it
        // down while it is played.
        let level = |lane: Option<bool>| {
            let (mut e, mut tx, _s) = rig();
            assert!(tx
                .push(Command::SetSong(two_tracks_song(vec![strip_note(0, 4.0)])))
                .is_ok());
            let chain = effect(FxKind::MultiEq, &[(5, 440.0), (6, 12.0), (7, 0.5)]);
            assert!(tx.push(Command::SetInserts { track: 0, chain }).is_ok());
            if let Some(follow) = lane {
                let lane = AutoLane {
                    track: 0,
                    target: AutoTarget::Fx { slot: 0, param: 6 },
                    follow,
                    offset_db: 0.0,
                    lo: 0.0,
                    hi: 1.0,
                    stepped: false,
                    points: vec![AutoPoint::new(0.0, 0.0)],
                };
                assert!(tx.push(Command::SetAutomation(vec![lane])).is_ok());
            }
            assert!(tx.push(Command::Play).is_ok());
            energy(&render(&mut e, 0.4), 0.2, 0.4)
        };
        let plain = level(None);
        let played = level(Some(true));
        let ignored = level(Some(false));
        assert!(played < plain * 0.5, "{played} {plain}");
        assert!((ignored / plain - 1.0).abs() < 1e-3, "{ignored} {plain}");

        // Stopping the lane puts the user's value back.
        let (mut e, mut tx, _s) = rig();
        assert!(tx
            .push(Command::SetSong(two_tracks_song(vec![strip_note(0, 8.0)])))
            .is_ok());
        assert!(tx
            .push(Command::SetInserts {
                track: 0,
                chain: effect(FxKind::MultiEq, &[(6, 12.0)]),
            })
            .is_ok());
        let mut lane = AutoLane {
            track: 0,
            target: AutoTarget::Fx { slot: 0, param: 6 },
            follow: true,
            offset_db: 0.0,
            lo: 0.0,
            hi: 1.0,
            stepped: false,
            points: vec![AutoPoint::new(0.0, 0.0)],
        };
        assert!(tx.push(Command::SetAutomation(vec![lane.clone()])).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        render(&mut e, 0.1);
        let moved = e.chains[0].as_ref().map(|c| c.fx[0].current()[6]);
        assert_eq!(moved, Some(-18.0));
        lane.follow = false;
        assert!(tx.push(Command::SetAutomation(vec![lane])).is_ok());
        render(&mut e, 0.1);
        let back = e.chains[0].as_ref().map(|c| c.fx[0].current()[6]);
        assert_eq!(back, Some(12.0));
    }

    #[test]
    fn compensation_delays_a_strip_and_the_sum_keeps_quiet_signals() {
        // A strip delayed by 100 samples before its fader and 50 after.
        let (mut e, mut tx, _s) = rig();
        assert!(tx.push(Command::SetSong(two_tracks_song(vec![]))).is_ok());
        assert!(tx
            .push(Command::SetPdc {
                track: 0,
                pdc: Box::new(Pdc::new(100, 50)),
            })
            .is_ok());
        e.drain_commands();
        let mut first = None;
        for i in 0..400 {
            if i == 10 {
                e.in_l[0] = 0.5;
                e.in_r[0] = -0.5;
                e.touched[0] = true;
                e.dirty.push(0);
            }
            let (l, r) = e.mix_strips();
            if first.is_none() && l != 0.0 {
                assert!((l - 0.5).abs() < 1e-6 && (r + 0.5).abs() < 1e-6, "{l} {r}");
                first = Some(i);
            }
        }
        assert_eq!(first, Some(10 + 150));

        // 64-bit sums: 1000 and -1000 cancel and what was between them is still there (in
        // 32 bits the 0.00001 would be lost: the spacing of numbers near 1000 is 0.00006).
        let (mut e, _tx, _s) = rig();
        for (t, v) in [(0usize, 1000.0f64), (1, 0.00001), (2, -1000.0)] {
            e.in_l[t] = v;
            e.touched[t] = true;
            e.dirty.push(t as u16);
        }
        let (l, _) = e.mix_strips();
        assert!((l - 0.00001).abs() < 1e-12, "{l}");
        assert!(e.dirty.is_empty() && e.in_l[0] == 0.0);
        // Garbage: the old compensation comes back through the queue, and a track past the
        // end is ignored.
        let (mut e, mut tx, _s) = rig();
        for pdc in [Pdc::new(1, 1), Pdc::new(2, 2)] {
            assert!(tx
                .push(Command::SetPdc {
                    track: 3,
                    pdc: Box::new(pdc)
                })
                .is_ok());
        }
        assert!(tx
            .push(Command::SetPdc {
                track: 9000,
                pdc: Box::new(Pdc::new(5, 5))
            })
            .is_ok());
        render(&mut e, 0.01);
        assert_eq!(e.pdc[3].as_ref().map(|p| p.lengths()), Some((2, 2)));
    }

    fn ctl(beat: f64, number: u8, value: u16) -> ControlEvent {
        ControlEvent {
            beat,
            track: 0,
            number,
            value,
        }
    }

    /// The engine's synth on track 0 playing one long note, with these region controllers.
    fn controlled(controls: Vec<ControlEvent>) -> (EngineCore, Producer<Command>) {
        let (mut e, mut tx, _s) = rig();
        let song =
            Song::new(vec![note(0.0, 8.0)], vec![TrackMix::default()]).with_controls(controls);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        e.process(&mut [0.0; 2]);
        // Undo that first frame: start again from the top.
        assert!(tx.push(Command::SetPosition { beat: 0.0 }).is_ok());
        (e, tx)
    }

    /// Frames (stereo) to the first frame at or after `beat` at 120 BPM, as the transport
    /// counts them.
    fn frame_of(beat: f64) -> usize {
        (beat * 24_000.0).ceil() as usize
    }

    #[test]
    fn volume_from_a_region_cuts_the_synth_at_its_exact_sample() {
        let (mut e, _tx) = controlled(vec![ctl(1.0, 7, 0)]);
        let at = frame_of(1.0);
        let mut buf = vec![0.0f32; (at + 2000) * 2];
        for chunk in buf.chunks_mut(512 * 2) {
            e.process(chunk);
        }
        let last_heard = (0..buf.len() / 2).rev().find(|&k| buf[k * 2] != 0.0);
        assert_eq!(last_heard, Some(at - 1), "silent from frame {at} on");
    }

    #[test]
    fn pitch_bend_and_modulation_change_the_synth_at_the_sample_and_follow_a_jump() {
        let (mut e, mut tx) = controlled(vec![
            ctl(1.0, 128, 16383),
            ctl(1.0, 1, 127),
            ctl(2.0, 128, 8192),
        ]);
        let at = frame_of(1.0);
        // Up to the frame before the event: no bend; that frame: two semitones up.
        let mut buf = vec![0.0f32; (at - 1) * 2];
        e.process(&mut buf);
        e.process(&mut [0.0; 2]);
        assert_eq!(e.midi_ctl(0).bend, 0.0);
        e.process(&mut [0.0; 2]);
        let c = e.midi_ctl(0);
        // 16383 is the top: 8191 steps of 8192 above the centre.
        assert!(
            (c.bend - BEND_RANGE * 8191.0 / 8192.0).abs() < 1e-6,
            "{}",
            c.bend
        );
        assert_eq!((c.modulation, c.vibrato_cents()), (1.0, 50.0));
        // The voice took its new pitch at once: its next control block starts there.
        assert!(e
            .voices
            .iter()
            .any(|v| v.active && v.ctrl_left == CTRL_BLOCK - 1));
        // A jump past the release of the bend: back to the centre, the wheel still up.
        assert!(tx.push(Command::SetPosition { beat: 3.0 }).is_ok());
        e.process(&mut [0.0; 2]);
        let c = e.midi_ctl(0);
        assert_eq!((c.bend, c.modulation), (0.0, 1.0));
        // And into the middle again: bent.
        assert!(tx.push(Command::SetPosition { beat: 1.5 }).is_ok());
        e.process(&mut [0.0; 2]);
        assert!(e.midi_ctl(0).bend > 1.99);
    }

    /// Zero crossings per second of the left channel between two times.
    fn crossings(buf: &[f32], from: f32, to: f32) -> f32 {
        let (a, b) = ((from * 48_000.0) as usize, (to * 48_000.0) as usize);
        let n = (a..b - 1)
            .filter(|&k| (buf[k * 2] <= 0.0) != (buf[(k + 1) * 2] <= 0.0))
            .count();
        n as f32 / 2.0 / (to - from)
    }

    #[test]
    fn a_soundfont_instrument_bends_with_the_region() {
        let (mut e, mut tx, _s) = rig();
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 0,
                player: Some(sine_player(48_000.0)),
            })
            .is_ok());
        let song = Song::new(vec![note(0.0, 4.0)], vec![TrackMix::default()])
            .with_controls(vec![ctl(1.0, 128, 16383)]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let buf = render(&mut e, 1.0);
        // The test font's sine is A 440 at pitch 69: two semitones up is B (494 Hz).
        let (before, after) = (crossings(&buf, 0.1, 0.45), crossings(&buf, 0.6, 0.95));
        assert!((before - 440.0).abs() < 12.0, "{before}");
        let want = 440.0 * 2f32.powf(2.0 / 12.0);
        assert!((after - want).abs() < 12.0, "{after} vs {want}");
    }

    #[test]
    fn the_sustain_pedal_holds_synth_notes_until_it_goes_up() {
        let (mut e, mut tx, _s) = rig();
        // A one-beat note, the pedal down at 0.5 and up at 2.
        let song = Song::new(vec![note(0.0, 1.0)], vec![TrackMix::default()])
            .with_controls(vec![ctl(0.5, 64, 127), ctl(2.0, 64, 0)]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let buf = render(&mut e, 2.0);
        // 120 BPM: beat 1 is at 0.5 s, beat 2 at 1 s.
        assert!(
            energy(&buf, 0.6, 0.95) > 1.0,
            "held past its end by the pedal"
        );
        assert!(
            energy(&buf, 1.5, 2.0) < 1e-6,
            "released once the pedal is up"
        );
        // Without the pedal it stops at its end.
        let (mut e, mut tx, _s) = rig();
        let song = Song::new(vec![note(0.0, 1.0)], vec![TrackMix::default()]);
        assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
        assert!(tx.push(Command::Play).is_ok());
        let buf = render(&mut e, 1.0);
        assert!(energy(&buf, 0.7, 0.95) < 1e-6);
    }

    #[test]
    fn controllers_do_not_allocate_on_the_audio_thread() {
        let controls: Vec<ControlEvent> = (0..400)
            .map(|k| {
                ctl(
                    k as f64 * 0.01,
                    [1, 7, 10, 11, 64, 128, 129][k % 7],
                    (k * 37 % 128) as u16,
                )
            })
            .collect();
        let (mut e, mut tx) = controlled(controls.clone());
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 0,
                player: Some(sine_player(48_000.0)),
            })
            .is_ok());
        let mut buf = vec![0.0f32; 512 * 2];
        for i in 0..200 {
            if i == 100 {
                assert!(tx.push(Command::SetPosition { beat: 2.5 }).is_ok());
            }
            assert_no_alloc::assert_no_alloc(|| e.process(&mut buf));
        }
        assert!(buf.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn midi_ctl_follows_the_controller_numbers() {
        let mut c = MidiCtl::default();
        assert!(c.apply(128, 0));
        assert_eq!(c.bend, -BEND_RANGE);
        assert!(!c.apply(128, 0), "the same bend again is no change");
        c.apply(7, 127);
        c.apply(11, 0);
        assert_eq!((c.volume, c.expression), (1.0, 0.0));
        c.apply(10, 64);
        assert!((c.pan_l - c.pan_r).abs() < 0.03 && (c.pan_l - 1.0).abs() < 0.03);
        c.apply(10, 0);
        assert!((c.pan_r).abs() < 1e-6);
        c.apply(64, 100);
        assert!(c.sustain);
        c.apply(129, 127);
        assert_eq!(c.vibrato_cents(), 50.0);
    }
}
