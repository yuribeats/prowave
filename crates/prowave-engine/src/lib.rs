//! ProWave Studio real-time engine.
//!
//! Logic Pro equivalent: transport + software-instrument playback + metronome (Klopfgeist).
//! The UI talks to the engine only through [`Command`]s on a lock-free SPSC queue and reads
//! state back through a lock-free triple buffer ([`StateReader`]) and atomics ([`Shared`]); the audio callback never locks, allocates or does I/O.
#![deny(clippy::unwrap_used, clippy::expect_used)]

// Debug test builds abort if anything inside `assert_no_alloc(..)` allocates or frees.
#[cfg(all(test, debug_assertions))]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

mod analysis;
#[cfg(target_os = "macos")]
pub mod au;
#[cfg(target_os = "macos")]
mod au_host_time;
mod audition;
#[cfg(not(target_arch = "wasm32"))]
mod capture;
mod clock;
mod convolver;
mod core;
mod disk;
#[cfg(target_os = "macos")]
pub mod duplex;
mod flex;
mod fx;
mod gain_smoothing;
#[cfg(not(target_arch = "wasm32"))]
mod mapped;
pub mod melody;
mod monitor;
mod mtc;
pub mod native_fx;
pub mod note_render;
mod output;
pub mod pitch_tracker;
#[cfg(target_os = "macos")]
pub mod plugin_handle;
mod plugin_track;
#[cfg(target_os = "macos")]
mod plugin_window_layout;
pub mod prerender;
mod routing;
#[cfg(target_os = "macos")]
mod rt_thread;
mod sample_buf;
mod sample_rack;
pub mod sample_tools;
mod soundfont;
mod spectral;
mod synth;
mod tempo_map;
#[cfg(test)]
mod timing_tests;
mod triple;
#[cfg(not(target_arch = "wasm32"))]
pub mod vocoder;
#[cfg(target_os = "macos")]
pub mod vst3;
#[cfg(target_os = "macos")]
pub mod vst3_host;
#[cfg(target_os = "macos")]
mod vst3_view;
#[cfg(windows)]
mod wasapi;

pub use crate::analysis::{
    beats_at, tempo_curve, Analysis, Analyzer, TempoEstimate, STRENGTH_RATE,
};
pub use crate::audition::{
    AuditionLayers, AuditionMsg, AuditionRemote, AuditionStatus, Auditioner, AUDITION_LAYERS,
};
#[cfg(not(target_arch = "wasm32"))]
pub use crate::capture::{
    input_devices, Capture, CaptureError, InputInfo, InputSource, Recorded, DUPLEX_INPUT_ID,
    MAX_INPUT_CHANNELS,
};
pub use crate::clock::{song_position, tick_index, ClockMessage, TICKS_PER_BEAT};
pub use crate::convolver::{
    ir_preview, ir_samples, ir_seconds, sd, volume_env, Convolver, BLOCK as CONVOLVER_BLOCK,
    IR_NAMES, IR_PARAMS, MAX_IR_SECONDS,
};
pub use crate::core::{
    dsp_load, AudioClip, ClipEdit, Command, ControlEvent, EngineCore, EngineState, FadeCurve,
    Garbage, MidiCtl, MidiOutEvent, NoteEvent, Shared, Song, StateReader, TrackMix, AUTO_BLOCK,
    MASTER_TRACK, MAX_AUTO_LANES, MAX_TRACKS,
};
pub use crate::disk::{
    ring_blocks_for, AudioError, AudioFile, DiskThread, RING_BLOCKS, RING_BLOCKS_MAX,
};
pub use crate::flex::{
    detect_notes, hz_to_midi, pitch_track, shift_pitch, warp, write_wav, DetectedNote, FlexMode,
    PitchShift, WarpPoint, MAX_POINTS as MAX_FLEX_POINTS,
};
pub use crate::fx::{
    eq_response_db, response_db, Chain, Fx, FxKind, ParamInfo, Pdc, AMP_CABS, AMP_MODELS,
    MAX_FX_PARAMS, MAX_INSERTS,
};
pub use crate::melody::{Algorithm, Blob, NoteAnalysis};
pub use crate::monitor::{InputFeed, MonitorTrack, MAX_MONITORS};
pub use crate::mtc::{
    full_frame, quarter_frame_data, rate_code, rate_of_code, MtcDecoder, MtcEvent, MtcGenerator,
    MtcMessage,
};
pub use crate::native_fx::tune;
pub use crate::native_fx::{
    native_effects, native_info, open_native, open_native_block, open_tune_block, NativeFx,
    NativeFxInfo, NativeParam, NativePreset,
};
pub use crate::note_render::{NoteChange, Resynth, SegmentCache};
pub use crate::output::{peak_of, Engine, EngineError};
pub use crate::pitch_tracker::{track_pitch, PitchTrack, PITCH_HOP};
pub use crate::plugin_track::{BlockProcessor, BlockTransport, PluginTrack};
pub use crate::prerender::{
    plugin_host_delay, prerender_delay, PROCESS_BUFFER_LARGE, PROCESS_BUFFER_MEDIUM,
    PROCESS_BUFFER_SMALL,
};
pub use crate::routing::{
    curve_shape, lane_value, send_gain, AutoLane, AutoPoint, AutoTarget, Route, SendSpec,
    MAX_AUTO_POINTS, MAX_BUSES, MAX_SENDS,
};
pub use crate::sample_buf::SampleBuf;
pub use crate::sample_rack::{
    RackChannelSpec, SampleRack, MAX_RACK_CHANNELS, RACK_BASE_NOTE, RACK_PARAMS_PER_CHANNEL,
};
pub use crate::soundfont::{PresetInfo, SfPlayer, SoundFontBank, MAX_SOUNDFONT_BYTES};
pub use crate::spectral::{
    paint, spectrogram, Fft, FreqScale, PaintSettings, Picture, Spectrogram, MAX_PAINT_SECONDS,
    PAINT_FFT, PAINT_HOP,
};
pub use crate::synth::{Kit, SynthPatch, Wave};
pub use crate::tempo_map::{
    MusicalPosition, SignatureEvent, Smpte, SmpteRate, TempoCurve, TempoEvent, TempoMap, MAX_BPM,
    MIN_BPM,
};
