//! macOS low-latency duplex I/O: one Core Audio IOProc that receives the input and fills the
//! output in the same callback, on the same clock (Logic: I/O Buffer Size + software
//! monitoring).
//!
//! cpal opens input and output as two streams with two callbacks; the input then has to wait
//! in a ring between them, and runs on another clock that must be resampled. Here the input of
//! a callback is heard in the output of that same callback: what a monitored track adds is
//! nothing but the device's own buffer and converter delay.
//!
//! When the input and output are different devices (a Mac's built-in microphone and
//! speakers), a private aggregate device joins them, clocked by the output, with drift
//! compensation on the input. It exists only while this program runs.
use objc2_core_audio::{
    kAudioDeviceProcessorOverload, kAudioDevicePropertyBufferFrameSize,
    kAudioDevicePropertyBufferFrameSizeRange, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyIOThreadOSWorkgroup, kAudioDevicePropertyLatency,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
    kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain, kAudioObjectPropertyName,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject, kAudioStreamPropertyLatency,
    AudioConvertHostTimeToNanos, AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID,
    AudioDeviceIOProc, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareDestroyAggregateDevice,
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
    AudioObjectID, AudioObjectPropertyAddress, AudioObjectPropertyScope,
    AudioObjectPropertySelector, AudioObjectRemovePropertyListener, AudioObjectSetPropertyData,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp, AudioValueRange};
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The buffer asked for when the caller names none: 64 frames, 1.3 ms at 48 kHz.
pub const DEFAULT_BUFFER_FRAMES: u32 = 64;
/// Input channels passed on (the rest of a large interface is not read).
const MAX_IN_CHANNELS: usize = crate::capture::MAX_INPUT_CHANNELS;
/// Output channels written (the engine renders a stereo pair into the first two).
const MAX_OUT_CHANNELS: usize = 64;
/// Frames one callback can carry; larger requests are cut to this.
const MAX_FRAMES: usize = 8192;
/// The start of the uid of our own aggregate devices (left out of [`devices`]).
const AGGREGATE_UID: &str = "prowave.duplex.";

/// Which devices to open, by Core Audio uid; `None` is the system's default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Devices {
    pub output: Option<String>,
    pub input: Option<String>,
    /// Open an input at all.
    pub with_input: bool,
}

/// An audio device the system has (Logic: Settings > Audio > Devices).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub uid: String,
    pub name: String,
    pub inputs: usize,
    pub outputs: usize,
    pub default_input: bool,
    pub default_output: bool,
}

/// Every audio device, in the system's order.
pub fn devices() -> Vec<DeviceInfo> {
    // SAFETY: property reads of the system object and of device ids the HAL has just listed;
    // an id that goes away meanwhile makes the read fail, which every helper reports as
    // `None` / 0.
    unsafe {
        let (def_in, def_out) = (
            system_device(kAudioHardwarePropertyDefaultInputDevice),
            system_device(kAudioHardwarePropertyDefaultOutputDevice),
        );
        all_devices()
            .into_iter()
            .filter_map(|d| {
                let uid = string_property(d, kAudioDevicePropertyDeviceUID)?;
                (!is_own_aggregate(&uid)).then(|| DeviceInfo {
                    uid,
                    name: name(d),
                    inputs: channels(d, kAudioObjectPropertyScopeInput),
                    outputs: channels(d, kAudioObjectPropertyScopeOutput),
                    default_input: def_in == Some(d),
                    default_output: def_out == Some(d),
                })
            })
            .collect()
    }
}

/// Whether `uid` is one of this program's own aggregate devices.
fn is_own_aggregate(uid: &str) -> bool {
    uid.starts_with(AGGREGATE_UID)
}

/// What the stream ended up with.
#[derive(Clone, Debug)]
pub struct DuplexInfo {
    /// The output device (or the input + output pair).
    pub device_name: String,
    /// The input device, when there is one.
    pub input_name: Option<String>,
    pub output_uid: String,
    pub input_uid: Option<String>,
    pub sample_rate: u32,
    pub buffer_frames: u32,
    pub in_channels: usize,
    pub out_channels: usize,
    /// Leading input channels of the device that belong to the output device.
    pub in_skip: usize,
    /// What the device reports beyond the buffer: converter, driver and safety offset.
    pub input_latency_frames: u32,
    pub output_latency_frames: u32,
    /// Whether a private aggregate device joins two devices.
    pub aggregate: bool,
}

/// One callback's audio, interleaved.
pub struct Block<'a> {
    /// `frames * in_channels` samples; empty when there is no input.
    pub input: &'a [f32],
    pub in_channels: usize,
    /// `frames * out_channels` samples, zeroed before the callback.
    pub output: &'a mut [f32],
    pub out_channels: usize,
    /// From now until the first output frame is heard.
    pub output_latency_us: u32,
    /// From when the first input frame reached the converter until now.
    pub input_latency_us: u32,
    /// The host clock at this callback, in nanoseconds.
    pub now_ns: u64,
    /// The device's sample time of the first output frame (frames since the device started;
    /// a jump forward means callbacks were missed). Negative when the device gives none.
    pub sample_time: f64,
}

/// The running stream. Dropping it stops the sound and removes the aggregate device.
pub struct DuplexStream {
    opened: Opened,
    proc_id: AudioDeviceIOProcID,
    running: bool,
    ctx: *mut c_void,
    io: AudioDeviceIOProc,
    free_ctx: unsafe fn(*mut c_void),
    configure: unsafe fn(*mut c_void, &DuplexInfo),
    devices: Devices,
    /// Counts `kAudioDeviceProcessorOverload` notifications of the running device.
    overloads: Arc<AtomicU32>,
    /// The device the overload listener is on (0: none).
    listening: AudioObjectID,
}

// SAFETY: the stream holds HAL ids (plain numbers) and the context pointer. The context is
// only touched by the HAL's I/O thread while the proc runs, and by the owning thread while it
// is stopped (`configure`, `free_ctx`); `F: Send` is required by `start`, so the closure in it
// may be dropped on whichever thread drops the stream. No HAL call here is tied to the
// creating thread.
unsafe impl Send for DuplexStream {}

impl Drop for DuplexStream {
    fn drop(&mut self) {
        // SAFETY: `ctx` came from `Box::into_raw` in `start_counted` with the same `F` as
        // `free_ctx`, and is freed only here, once. This relies on the HAL not calling the
        // proc after `AudioDeviceStop` + `AudioDeviceDestroyIOProcID` have returned (nothing
        // here waits for a callback that is still in flight; no test covers that window).
        unsafe {
            self.stop();
            (self.free_ctx)(self.ctx);
        }
    }
}

struct Ctx<F> {
    f: F,
    input: Vec<f32>,
    output: Vec<f32>,
    /// Input channels of the device that are not the chosen input's (an aggregate lists the
    /// output device's own inputs first).
    in_skip: usize,
    in_channels: usize,
    out_channels: usize,
    in_extra_ns: u64,
    out_extra_ns: u64,
}

impl<F> Ctx<F> {
    /// A context for no device yet (see [`Ctx::configure`]), with buffers for the largest
    /// callback any device can ask for, so a new device never needs new ones.
    fn new(f: F) -> Self {
        Self {
            f,
            input: vec![0.0; MAX_FRAMES * MAX_IN_CHANNELS],
            output: vec![0.0; MAX_FRAMES * MAX_OUT_CHANNELS],
            in_skip: 0,
            in_channels: 0,
            out_channels: 0,
            in_extra_ns: 0,
            out_extra_ns: 0,
        }
    }

    /// Points the context at a newly opened device. The channel counts are cut to what the
    /// buffers hold, which is what lets the callback index them without checks of its own.
    fn configure(&mut self, info: &DuplexInfo) {
        self.in_skip = info.in_skip;
        self.in_channels = info.in_channels.min(MAX_IN_CHANNELS);
        self.out_channels = info.out_channels.min(MAX_OUT_CHANNELS);
        self.in_extra_ns = frames_to_ns(info.input_latency_frames, info.sample_rate);
        self.out_extra_ns = frames_to_ns(info.output_latency_frames, info.sample_rate);
    }
}

/// `frames` at `sample_rate`, in nanoseconds.
fn frames_to_ns(frames: u32, sample_rate: u32) -> u64 {
    (f64::from(frames) * 1e9 / f64::from(sample_rate.max(1))) as u64
}

/// A latency for [`Block`]: the host-clock distance plus the device's own, in microseconds.
fn latency_us(host_ns: u64, extra_ns: u64) -> u32 {
    ((host_ns + extra_ns) / 1000).min(u32::MAX as u64) as u32
}

// SAFETY: `p` must be the pointer `Box::into_raw` gave for a `Ctx<F>` of this same `F`, not
// freed before, and nothing may use it afterwards (the proc must be stopped and destroyed).
unsafe fn free_ctx<F>(p: *mut c_void) {
    drop(Box::from_raw(p.cast::<Ctx<F>>()));
}

/// Points the context at a newly opened device (the proc is stopped meanwhile).
// SAFETY: `p` must point to a live `Ctx<F>` of this same `F`, and the proc must be stopped:
// this takes `&mut` to what the I/O thread otherwise owns.
unsafe fn configure_ctx<F>(p: *mut c_void, info: &DuplexInfo) {
    (*p.cast::<Ctx<F>>()).configure(info);
}

/// A device that is set up but not running yet.
struct Opened {
    device: AudioObjectID,
    aggregate: Option<AudioObjectID>,
    info: DuplexInfo,
}

impl Opened {
    /// Finds `devices`, joins them when they differ, sets the buffer and, when `rate` is
    /// given, the sample rate (a running engine cannot change its own).
    // SAFETY: no caller contract beyond the HAL's own: every id used is looked up here, and
    // the aggregate made here is destroyed again if the rest fails. `unsafe` because it is
    // HAL calls throughout.
    unsafe fn open(
        devices: &Devices,
        buffer_frames: Option<u32>,
        rate: Option<u32>,
    ) -> Result<Self, String> {
        let out_dev = match &devices.output {
            Some(uid) => device_by_uid(uid).ok_or_else(|| format!("no output device {uid}"))?,
            None => system_device(kAudioHardwarePropertyDefaultOutputDevice)
                .ok_or("no default output device")?,
        };
        if channels(out_dev, kAudioObjectPropertyScopeOutput) == 0 {
            return Err(format!("{} has no outputs", name(out_dev)));
        }
        let in_dev = if !devices.with_input {
            None
        } else {
            match &devices.input {
                Some(uid) => {
                    Some(device_by_uid(uid).ok_or_else(|| format!("no input device {uid}"))?)
                }
                None => system_device(kAudioHardwarePropertyDefaultInputDevice),
            }
            .filter(|d| channels(*d, kAudioObjectPropertyScopeInput) > 0)
        };
        if let Some(r) = rate {
            for d in [Some(out_dev), in_dev].into_iter().flatten() {
                set_rate(d, r);
            }
        }
        let mut aggregate = None;
        let device = match in_dev {
            Some(i) if i != out_dev => match make_aggregate(out_dev, i) {
                Ok(agg) => {
                    aggregate = Some(agg);
                    agg
                }
                Err(e) => {
                    tracing::warn!("cannot join input and output ({e}); output only");
                    out_dev
                }
            },
            _ => out_dev,
        };
        let opened = Self::describe(
            device,
            out_dev,
            in_dev.filter(|i| aggregate.is_some() || *i == out_dev),
            aggregate,
            buffer_frames,
            rate,
        );
        if opened.is_err() {
            if let Some(agg) = aggregate {
                AudioHardwareDestroyAggregateDevice(agg);
            }
        }
        opened
    }

    // SAFETY: `device`, `out_dev` and `in_dev` must be ids the HAL gave (a stale one only
    // makes the reads fail); `aggregate`, when set, must be `device`.
    unsafe fn describe(
        device: AudioObjectID,
        out_dev: AudioObjectID,
        in_dev: Option<AudioObjectID>,
        aggregate: Option<AudioObjectID>,
        buffer_frames: Option<u32>,
        want_rate: Option<u32>,
    ) -> Result<Self, String> {
        let frames = set_frames(device, buffer_frames.unwrap_or(DEFAULT_BUFFER_FRAMES));
        if let Some(r) = want_rate {
            set_rate(device, r);
        }
        let rate = get::<f64>(
            device,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
        )
        .unwrap_or(48_000.0);
        let sample_rate = rate.round() as u32;
        if let Some(r) = want_rate.filter(|r| *r != sample_rate) {
            return Err(format!(
                "{} runs at {sample_rate} Hz, not {r} Hz",
                name(device)
            ));
        }
        let dev_ins = if aggregate.is_some() || in_dev.is_some() {
            channels(device, kAudioObjectPropertyScopeInput)
        } else {
            0
        };
        let dev_outs = channels(device, kAudioObjectPropertyScopeOutput);
        let joined = aggregate.map(|_| {
            (
                channels(out_dev, kAudioObjectPropertyScopeInput),
                channels(out_dev, kAudioObjectPropertyScopeOutput),
            )
        });
        let (in_skip, in_channels, out_channels) =
            channel_layout(dev_ins, dev_outs, joined, in_dev.is_some());
        if out_channels == 0 {
            return Err("the device has no output channels".into());
        }
        // The latencies of the real devices; an aggregate's own figures are not documented.
        let in_lat = in_dev.map_or(0, |d| extra_latency(d, kAudioObjectPropertyScopeInput));
        let out_lat = extra_latency(out_dev, kAudioObjectPropertyScopeOutput);
        let out_name = name(out_dev);
        let input_name = in_dev.map(|d| name(d));
        let info = DuplexInfo {
            device_name: device_label(out_name, input_name.as_deref(), aggregate.is_some()),
            input_name,
            output_uid: string_property(out_dev, kAudioDevicePropertyDeviceUID).unwrap_or_default(),
            input_uid: in_dev.and_then(|d| string_property(d, kAudioDevicePropertyDeviceUID)),
            sample_rate,
            buffer_frames: frames,
            in_channels,
            out_channels,
            in_skip,
            input_latency_frames: in_lat,
            output_latency_frames: out_lat,
            aggregate: aggregate.is_some(),
        };
        Ok(Self {
            device,
            aggregate,
            info,
        })
    }
}

/// Which of the device's channels are the stream's: `(in_skip, in_channels, out_channels)`.
/// `dev_ins` / `dev_outs` are the channels of the device the callback runs on; `joined` is the
/// output device's own `(inputs, outputs)` when that device is an aggregate of the two.
fn channel_layout(
    dev_ins: usize,
    dev_outs: usize,
    joined: Option<(usize, usize)>,
    with_input: bool,
) -> (usize, usize, usize) {
    // An aggregate lists its sub-devices in order, the output first: only the input's
    // inputs and the output's outputs are ours.
    let (skip, ins, outs) = match joined {
        Some((out_dev_ins, out_dev_outs)) => (
            out_dev_ins,
            dev_ins.saturating_sub(out_dev_ins),
            dev_outs.min(out_dev_outs),
        ),
        None if with_input => (0, dev_ins, dev_outs),
        None => (0, 0, dev_outs),
    };
    (skip, ins.min(MAX_IN_CHANNELS), outs.min(MAX_OUT_CHANNELS))
}

/// The name shown for the stream: the output's, or "input + output" for a joined pair.
fn device_label(out_name: String, input_name: Option<&str>, aggregate: bool) -> String {
    match input_name {
        Some(i) if aggregate => format!("{i} + {out_name}"),
        _ => out_name,
    }
}

/// The steps of a device switch as [`switch_rig`] orders them, apart from the HAL, so the
/// order and the fallback can be tested without a device.
trait Rig {
    type Opened;
    /// Stops the callback and lets go of the device. Harmless when already halted.
    fn halt(&mut self);
    fn open(&mut self, devices: &Devices) -> Result<Self::Opened, String>;
    /// Makes `opened` the device to run. Only called while halted.
    fn adopt(&mut self, opened: Self::Opened);
    fn run(&mut self) -> Result<(), String>;
}

/// How a device switch ended.
#[derive(Debug, PartialEq, Eq)]
enum Switched {
    /// Running on the new devices.
    Moved,
    /// The new devices failed (why); running on the ones before again.
    Kept(String),
    /// Neither the new devices nor the ones before could be started (why, for both):
    /// nothing runs.
    Stopped(String),
}

/// Moves `rig` from the `old` devices to `new`. If `new` cannot be opened or started, back
/// to `old`; if those do not come back either, the rig is left halted.
fn switch_rig<R: Rig>(rig: &mut R, new: &Devices, old: &Devices) -> Switched {
    rig.halt();
    let why = match bring_up(rig, new) {
        Ok(()) => return Switched::Moved,
        Err(e) => e,
    };
    match bring_up(rig, old) {
        Ok(()) => Switched::Kept(why),
        Err(old_why) => Switched::Stopped(format!(
            "{why}; the devices before did not come back either ({old_why}): audio is stopped"
        )),
    }
}

/// Opens `devices` on a halted rig and starts them. On an error the rig is halted again
/// (a device that opened but did not start is let go of).
fn bring_up<R: Rig>(rig: &mut R, devices: &Devices) -> Result<(), String> {
    let opened = rig.open(devices)?;
    rig.adopt(opened);
    let started = rig.run();
    if started.is_err() {
        rig.halt();
    }
    started
}

/// A [`DuplexStream`] being switched, at the buffer size and sample rate it had before.
struct Switching<'a> {
    stream: &'a mut DuplexStream,
    frames: u32,
    rate: u32,
}

impl Rig for Switching<'_> {
    type Opened = Opened;

    fn halt(&mut self) {
        // SAFETY: the ids `stop` uses are the stream's own (the listener, proc and aggregate
        // it set up), each cleared as it is released, so a second call does nothing.
        unsafe { self.stream.stop() }
    }

    fn open(&mut self, devices: &Devices) -> Result<Opened, String> {
        // SAFETY: HAL calls on ids looked up inside; see `Opened::open`.
        unsafe { Opened::open(devices, Some(self.frames), Some(self.rate)) }
    }

    fn adopt(&mut self, opened: Opened) {
        self.stream.opened = opened;
        // SAFETY: `ctx` is the live context made in `start_counted` and `configure` is
        // `configure_ctx` of the same `F`. `switch_rig` calls this only after `halt`, so the
        // proc is stopped and the I/O thread is not using the context.
        unsafe { (self.stream.configure)(self.stream.ctx, &self.stream.opened.info) }
    }

    fn run(&mut self) -> Result<(), String> {
        // SAFETY: the stream is halted (no proc installed) and `ctx` is live; see `run`.
        unsafe { self.stream.run() }
    }
}

impl DuplexStream {
    /// Opens `devices` with about `buffer_frames` frames per callback
    /// ([`DEFAULT_BUFFER_FRAMES`] when `None`), then calls `make` with what was opened to
    /// build the callback.
    pub fn start<F, M>(
        buffer_frames: Option<u32>,
        devices: &Devices,
        make: M,
    ) -> Result<(Self, DuplexInfo), String>
    where
        F: FnMut(Block<'_>) + Send + 'static,
        M: FnOnce(&DuplexInfo) -> F,
    {
        Self::start_counted(buffer_frames, devices, Arc::new(AtomicU32::new(0)), make)
    }

    /// [`DuplexStream::start`], counting the device's processor overloads (Logic: "System
    /// Overload") into `overloads`.
    pub fn start_counted<F, M>(
        buffer_frames: Option<u32>,
        devices: &Devices,
        overloads: Arc<AtomicU32>,
        make: M,
    ) -> Result<(Self, DuplexInfo), String>
    where
        F: FnMut(Block<'_>) + Send + 'static,
        M: FnOnce(&DuplexInfo) -> F,
    {
        // SAFETY: HAL calls on ids `Opened::open` looked up. `ctx` is a leaked `Box<Ctx<F>>`
        // and `io_proc`, `free_ctx` and `configure_ctx` are all taken at this same `F`, so
        // every cast back is to the right type. The stream owns the pointer from here: it is
        // configured before the proc first runs, and freed only by `Drop`, after the proc is
        // stopped (also when `run` fails below: `stream` is dropped).
        unsafe {
            let opened = Opened::open(devices, buffer_frames, None)?;
            let ctx = Box::new(Ctx::new(make(&opened.info)));
            let ctx = Box::into_raw(ctx).cast::<c_void>();
            configure_ctx::<F>(ctx, &opened.info);
            let info = opened.info.clone();
            let mut stream = Self {
                opened,
                proc_id: None,
                running: false,
                ctx,
                io: Some(io_proc::<F>),
                free_ctx: free_ctx::<F>,
                configure: configure_ctx::<F>,
                devices: devices.clone(),
                overloads,
                listening: 0,
            };
            stream.run()?;
            Ok((stream, info))
        }
    }

    /// What is open now.
    pub fn info(&self) -> &DuplexInfo {
        &self.opened.info
    }

    /// The Core Audio device the callback runs on (an aggregate when two devices are
    /// joined): the playback thread joins its I/O workgroup.
    pub fn device_id(&self) -> u32 {
        self.opened.device
    }

    /// Moves to other devices without stopping the engine: the same callback goes on at the
    /// same sample rate. If they cannot be opened or started, the devices before are opened
    /// again and the error says why. If those do not come back either, nothing runs any
    /// more and the error says so; a later `switch` can still bring the stream back.
    pub fn switch(&mut self, devices: &Devices) -> Result<DuplexInfo, String> {
        let (frames, rate) = (self.opened.info.buffer_frames, self.opened.info.sample_rate);
        let old = self.devices.clone();
        let mut rig = Switching {
            stream: self,
            frames,
            rate,
        };
        match switch_rig(&mut rig, devices, &old) {
            Switched::Moved => {
                self.devices = devices.clone();
                Ok(self.opened.info.clone())
            }
            // `self.devices` stays what it was: the next switch falls back to it too.
            Switched::Kept(why) | Switched::Stopped(why) => Err(why),
        }
    }

    /// Asks for `frames` per callback while running (Logic: I/O Buffer Size). Returns what
    /// the device took.
    pub fn set_buffer_frames(&mut self, frames: u32) -> u32 {
        // SAFETY: a property write on the stream's own device id. It may land while the proc
        // runs: the callback takes its frame count from each call's buffers, never from here.
        let got = unsafe { set_frames(self.opened.device, frames) };
        self.opened.info.buffer_frames = got;
        got
    }

    // SAFETY: the stream must be halted (no proc installed: fresh, or after `stop`), and
    // `self.ctx` must be the live `Ctx<F>` that `self.io` was made for, already configured
    // for `self.opened`. The proc gets `ctx`, and the overload listener a pointer into
    // `self.overloads`; both stay valid until `stop` has removed them.
    unsafe fn run(&mut self) -> Result<(), String> {
        let device = self.opened.device;
        let mut proc_id: AudioDeviceIOProcID = None;
        let status =
            AudioDeviceCreateIOProcID(device, self.io, self.ctx, NonNull::from(&mut proc_id));
        if status != 0 || proc_id.is_none() {
            return Err(format!("AudioDeviceCreateIOProcID failed ({status})"));
        }
        let status = AudioDeviceStart(device, proc_id);
        if status != 0 {
            AudioDeviceDestroyIOProcID(device, proc_id);
            return Err(format!("AudioDeviceStart failed ({status})"));
        }
        self.proc_id = proc_id;
        self.running = true;
        let addr = address(
            kAudioDeviceProcessorOverload,
            kAudioObjectPropertyScopeGlobal,
        );
        let ctx = Arc::as_ptr(&self.overloads).cast_mut().cast::<c_void>();
        if AudioObjectAddPropertyListener(device, NonNull::from(&addr), Some(on_overload), ctx) == 0
        {
            self.listening = device;
        }
        Ok(())
    }

    /// Stops the proc and removes the aggregate device; the context is kept.
    // SAFETY: only uses ids this stream set up in `run` / `open`, and clears each as it is
    // released, so calling it again is harmless. The listener is removed with the same
    // function and client pointer it was added with. After it returns the caller may touch
    // the context, on the HAL's word that a stopped, destroyed proc is not called again.
    unsafe fn stop(&mut self) {
        if self.listening != 0 {
            let addr = address(
                kAudioDeviceProcessorOverload,
                kAudioObjectPropertyScopeGlobal,
            );
            let ctx = Arc::as_ptr(&self.overloads).cast_mut().cast::<c_void>();
            AudioObjectRemovePropertyListener(
                self.listening,
                NonNull::from(&addr),
                Some(on_overload),
                ctx,
            );
            self.listening = 0;
        }
        if self.running {
            AudioDeviceStop(self.opened.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.opened.device, self.proc_id);
            self.running = false;
        }
        if let Some(agg) = self.opened.aggregate.take() {
            AudioHardwareDestroyAggregateDevice(agg);
        }
    }
}

/// `kAudioDeviceProcessorOverload`: the HAL missed a deadline of this device. Called on the
/// I/O thread; only counts.
// SAFETY: `client` must be null or point to an `AtomicU32` that outlives the call (here: the
// inside of the stream's `Arc`, kept until after the listener is removed). The other
// arguments are not read.
unsafe extern "C-unwind" fn on_overload(
    _device: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    client: *mut c_void,
) -> i32 {
    if let Some(n) = client.cast::<AtomicU32>().as_ref() {
        n.fetch_add(1, Ordering::Relaxed);
    }
    0
}

/// The I/O workgroup of `device` (`kAudioDevicePropertyIOThreadOSWorkgroup`), retained: the
/// caller releases it with `os_release`.
pub(crate) fn io_workgroup(device: AudioObjectID) -> Option<*mut c_void> {
    // SAFETY: the property is one pointer, and `get` only accepts a reply of exactly that
    // size into storage of that size; a wrong `device` makes the read fail (`None`).
    let wg = unsafe {
        get::<*mut c_void>(
            device,
            kAudioDevicePropertyIOThreadOSWorkgroup,
            kAudioObjectPropertyScopeGlobal,
        )
    }?;
    (!wg.is_null()).then_some(wg)
}

/// The buffer to ask a device for: `wanted` frames, within the `(least, most)` the device
/// takes and what one callback here can carry.
fn pick_frames(wanted: u32, range: Option<(f64, f64)>) -> u32 {
    match range {
        // Not `f64::clamp`: it panics when a driver's range is inverted or not a number.
        // `max` then `min` take the other value for a NaN and let `most` win.
        Some((least, most)) => f64::from(wanted).max(least).min(most) as u32,
        None => wanted,
    }
    .clamp(1, MAX_FRAMES as u32)
}

// SAFETY: `device` must be a HAL object id (a stale one only makes the calls fail). The
// property types match the HAL's: an `AudioValueRange` for the range, a `u32` for the size.
unsafe fn set_frames(device: AudioObjectID, wanted: u32) -> u32 {
    let range = get::<AudioValueRange>(
        device,
        kAudioDevicePropertyBufferFrameSizeRange,
        kAudioObjectPropertyScopeGlobal,
    );
    let frames = pick_frames(wanted, range.map(|r| (r.mMinimum, r.mMaximum)));
    if !set(
        device,
        kAudioDevicePropertyBufferFrameSize,
        kAudioObjectPropertyScopeGlobal,
        &frames,
    ) {
        tracing::warn!("the device refused a buffer of {frames} frames");
    }
    get::<u32>(
        device,
        kAudioDevicePropertyBufferFrameSize,
        kAudioObjectPropertyScopeGlobal,
    )
    .unwrap_or(frames)
}

// SAFETY: `device` must be a HAL object id; the nominal sample rate is an `f64`. Blocks up
// to half a second: never on the I/O thread.
unsafe fn set_rate(device: AudioObjectID, rate: u32) {
    let now = get::<f64>(
        device,
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
    );
    if now.is_some_and(|r| r.round() as u32 == rate) {
        return;
    }
    let r = f64::from(rate);
    if set(
        device,
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
        &r,
    ) {
        // The change is asynchronous.
        for _ in 0..50 {
            let now = get::<f64>(
                device,
                kAudioDevicePropertyNominalSampleRate,
                kAudioObjectPropertyScopeGlobal,
            );
            if now.is_some_and(|x| x.round() as u32 == rate) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

// SAFETY: no caller contract: a read of the system object's device list.
unsafe fn all_devices() -> Vec<AudioObjectID> {
    let Some((buf, size)) = get_bytes(
        kAudioObjectSystemObject as AudioObjectID,
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    ) else {
        return Vec::new();
    };
    object_ids(&buf, size)
}

/// The object ids in the first `size` bytes of a property block (see [`get_bytes`]).
fn object_ids(words: &[u64], size: usize) -> Vec<AudioObjectID> {
    words
        .iter()
        .flat_map(|w| {
            let b = w.to_ne_bytes();
            [
                u32::from_ne_bytes([b[0], b[1], b[2], b[3]]),
                u32::from_ne_bytes([b[4], b[5], b[6], b[7]]),
            ]
        })
        .take(size / std::mem::size_of::<AudioObjectID>())
        .collect()
}

// SAFETY: no caller contract: reads of the device list and of each listed device's uid.
unsafe fn device_by_uid(uid: &str) -> Option<AudioObjectID> {
    all_devices()
        .into_iter()
        .find(|d| string_property(*d, kAudioDevicePropertyDeviceUID).as_deref() == Some(uid))
}

/// The host clock (`mach_absolute_time` units, what CoreMIDI time stamps use) for `ns`
/// nanoseconds of it.
pub fn host_time_from_ns(ns: u64) -> u64 {
    // SAFETY: takes and returns plain integers; no pointers, no state of ours.
    unsafe { objc2_core_audio::AudioConvertNanosToHostTime(ns) }
}

/// The host clock now, in nanoseconds.
pub fn host_now_ns() -> u64 {
    // SAFETY: both take and return plain integers; no pointers, no state of ours.
    unsafe {
        objc2_core_audio::AudioConvertHostTimeToNanos(objc2_core_audio::AudioGetCurrentHostTime())
    }
}

/// The buffers of an `AudioBufferList`.
// SAFETY: `list` must point to an `AudioBufferList` that really is followed by the
// `mNumberBuffers` buffers it claims (the struct only declares the first), aligned for it,
// and neither freed nor written for as long as the returned slice is used: `'a` is the
// caller's choice, nothing checks it. Each buffer's `mData` is a pointer of its own, not
// reached through the slice.
unsafe fn buffers<'a>(list: NonNull<AudioBufferList>) -> &'a [AudioBuffer] {
    let list = list.as_ptr();
    let n = (*list).mNumberBuffers as usize;
    std::slice::from_raw_parts(
        std::ptr::addr_of!((*list).mBuffers).cast::<AudioBuffer>(),
        n,
    )
}

fn host_ns(later: u64, earlier: u64) -> u64 {
    if later <= earlier {
        return 0;
    }
    // SAFETY: takes and returns plain integers; no pointers, no state of ours.
    unsafe { AudioConvertHostTimeToNanos(later - earlier) }
}

// SAFETY: called by the HAL on its I/O thread, which keeps every pointer valid for the call.
// `client` must be the `Ctx<F>` (same `F`) given to `AudioDeviceCreateIOProcID`, and nothing
// else may touch that context while the proc is installed and running (the stream only does
// so after `stop`); the HAL never runs one proc twice at once.
unsafe extern "C-unwind" fn io_proc<F: FnMut(Block<'_>)>(
    _device: AudioObjectID,
    now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    input_time: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let ctx = &mut *client.cast::<Ctx<F>>();
    // The input list is only read when there is an input to pass on.
    let ins = if ctx.in_channels > 0 {
        buffers(input)
    } else {
        &[]
    };
    run_block(
        ctx,
        ins,
        buffers(output),
        now.as_ref(),
        input_time.as_ref(),
        output_time.as_ref(),
        host_ns,
    );
    0
}

/// One callback: gathers `ins`, calls the closure, spreads what it rendered over `outs`.
/// `host_ns(later, earlier)` is the host-clock distance in nanoseconds ([`host_ns`]; a
/// parameter so tests can run this without the host clock). No allocation, lock or syscall.
// SAFETY: every buffer in `ins` and `outs` with a non-null `mData` must point to
// `mDataByteSize` bytes aligned for `f32`: readable in `ins`, writable in `outs` and not
// overlapping `ctx` or each other. `ctx` must have been set up by `Ctx::new` and
// `Ctx::configure`: with at most MAX_IN_CHANNELS / MAX_OUT_CHANNELS channels and never more
// than MAX_FRAMES frames taken, every index into `ctx.input` / `ctx.output` is in bounds.
// Buffer sizes are not trusted beyond that: each buffer is read or written only up to its
// own `mDataByteSize`.
unsafe fn run_block<F: FnMut(Block<'_>)>(
    ctx: &mut Ctx<F>,
    ins: &[AudioBuffer],
    outs: &[AudioBuffer],
    now: &AudioTimeStamp,
    in_t: &AudioTimeStamp,
    out_t: &AudioTimeStamp,
    host_ns: impl Fn(u64, u64) -> u64,
) {
    let Some(frames) = outs
        .iter()
        .find(|b| b.mNumberChannels > 0)
        .map(|b| b.mDataByteSize as usize / 4 / b.mNumberChannels as usize)
    else {
        return;
    };
    let frames = frames.min(MAX_FRAMES);

    // The input, gathered from every stream into one interleaved block.
    let ic = ctx.in_channels;
    let in_len = if ic > 0 { frames * ic } else { 0 };
    let gathered = &mut ctx.input[..in_len];
    gathered.fill(0.0);
    if ic > 0 {
        let (skip, mut base) = (ctx.in_skip, 0);
        for b in ins {
            let n = b.mNumberChannels as usize;
            // Device channels base..base + n; ours are skip..skip + ic.
            if n == 0 || b.mData.is_null() || base + n <= skip || base >= skip + ic {
                base += n;
                continue;
            }
            let have = (b.mDataByteSize as usize / 4 / n).min(frames);
            let src = std::slice::from_raw_parts(b.mData.cast::<f32>(), have * n);
            for (f, frame) in src.chunks_exact(n).enumerate() {
                for (c, s) in frame.iter().enumerate() {
                    let ch = base + c;
                    if ch >= skip && ch < skip + ic {
                        gathered[f * ic + ch - skip] = *s;
                    }
                }
            }
            base += n;
        }
    }

    let oc = ctx.out_channels;
    let rendered = &mut ctx.output[..frames * oc];
    rendered.fill(0.0);
    let output_latency_us = latency_us(host_ns(out_t.mHostTime, now.mHostTime), ctx.out_extra_ns);
    let input_latency_us = latency_us(host_ns(now.mHostTime, in_t.mHostTime), ctx.in_extra_ns);
    (ctx.f)(Block {
        input: &ctx.input[..in_len],
        in_channels: ic,
        output: rendered,
        out_channels: oc,
        output_latency_us,
        input_latency_us,
        now_ns: host_ns(now.mHostTime, 0),
        // kAudioTimeStampSampleTimeValid
        sample_time: if out_t.mFlags.0 & 1 != 0 {
            out_t.mSampleTime
        } else {
            -1.0
        },
    });

    // The output, spread back over the device's streams.
    let mut base = 0;
    for b in outs {
        let n = b.mNumberChannels as usize;
        if n == 0 || b.mData.is_null() {
            base += n;
            continue;
        }
        // Every sample the buffer holds is written, not only this callback's frames: a
        // stream that is longer than the first one, or than MAX_FRAMES, gets silence for
        // the rest instead of going out with whatever the buffer held before.
        let dst =
            std::slice::from_raw_parts_mut(b.mData.cast::<f32>(), b.mDataByteSize as usize / 4);
        let mut whole = dst.chunks_exact_mut(n);
        for (f, frame) in whole.by_ref().enumerate() {
            for (c, s) in frame.iter_mut().enumerate() {
                let ch = base + c;
                *s = if f < frames && ch < oc {
                    ctx.output[f * oc + ch]
                } else {
                    0.0
                };
            }
        }
        whole.into_remainder().fill(0.0);
        base += n;
    }
}

fn address(
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

// SAFETY: `T` must be the plain-data type the HAL documents for this property, valid for any
// bytes the HAL writes into it (integers, floats, raw pointers, structs of those). The HAL
// is given `size_of::<T>()` bytes of zeroed storage and the value is only taken as written
// when it reports having filled exactly that many. A raw pointer read this way is as good as
// the HAL's word; a retained object in it is the caller's to release.
unsafe fn get<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Option<T> {
    let addr = address(selector, scope);
    let mut value = std::mem::MaybeUninit::<T>::zeroed();
    let mut size = std::mem::size_of::<T>() as u32;
    let status = AudioObjectGetPropertyData(
        object,
        NonNull::from(&addr),
        0,
        std::ptr::null(),
        NonNull::from(&mut size),
        NonNull::new_unchecked(value.as_mut_ptr().cast()),
    );
    (status == 0 && size as usize == std::mem::size_of::<T>()).then(|| value.assume_init())
}

// SAFETY: `T` must be the plain-data type the HAL documents for this property; the HAL reads
// `size_of::<T>()` bytes from `value` during the call and does not keep the pointer.
unsafe fn set<T>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
    value: &T,
) -> bool {
    let addr = address(selector, scope);
    AudioObjectSetPropertyData(
        object,
        NonNull::from(&addr),
        0,
        std::ptr::null(),
        std::mem::size_of::<T>() as u32,
        NonNull::from(value).cast(),
    ) == 0
}

/// A property that is a variable-length block of bytes (8-byte aligned).
// SAFETY: no caller contract: the block is sized by the HAL's own answer, the HAL is told
// that size and cannot write past it, and the bytes come back as plain `u64` words (zeroed
// beyond what was written) with the count the HAL reports having written.
unsafe fn get_bytes(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Option<(Vec<u64>, usize)> {
    let addr = address(selector, scope);
    let mut size = 0u32;
    if AudioObjectGetPropertyDataSize(
        object,
        NonNull::from(&addr),
        0,
        std::ptr::null(),
        NonNull::from(&mut size),
    ) != 0
    {
        return None;
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
    let status = AudioObjectGetPropertyData(
        object,
        NonNull::from(&addr),
        0,
        std::ptr::null(),
        NonNull::from(&mut size),
        NonNull::new_unchecked(buf.as_mut_ptr().cast()),
    );
    (status == 0).then_some((buf, size as usize))
}

// SAFETY: `selector` must be a system-object property whose value is one `AudioObjectID`.
unsafe fn system_device(selector: AudioObjectPropertySelector) -> Option<AudioObjectID> {
    get::<AudioObjectID>(
        kAudioObjectSystemObject as AudioObjectID,
        selector,
        kAudioObjectPropertyScopeGlobal,
    )
    .filter(|d| *d != 0)
}

/// Channels of `device` in `scope`, over all its streams.
// SAFETY: no caller contract: a property read; a stale `device` gives 0.
unsafe fn channels(device: AudioObjectID, scope: AudioObjectPropertyScope) -> usize {
    let Some((buf, size)) = get_bytes(device, kAudioDevicePropertyStreamConfiguration, scope)
    else {
        return 0;
    };
    stream_config_channels(&buf, size)
}

/// The channels listed in a `kAudioDevicePropertyStreamConfiguration` block: an
/// `AudioBufferList` in the first `size` bytes of `words`. Buffers the list claims beyond
/// those bytes are not read.
fn stream_config_channels(words: &[u64], size: usize) -> usize {
    let size = size.min(std::mem::size_of_val(words));
    let first = std::mem::offset_of!(AudioBufferList, mBuffers);
    if size < first {
        return 0;
    }
    let fit = (size - first) / std::mem::size_of::<AudioBuffer>();
    let list = words.as_ptr().cast::<AudioBufferList>();
    // SAFETY: `words` is 8-byte aligned, which is `AudioBufferList`'s alignment, and holds
    // at least `first` bytes, so the count can be read; the slice then covers `n` buffers
    // that lie wholly inside `words` (`n <= fit`). Any bit pattern is a valid `AudioBuffer`
    // (two `u32`s and a raw pointer that is never followed here).
    let bufs = unsafe {
        let n = (std::ptr::addr_of!((*list).mNumberBuffers).read() as usize).min(fit);
        std::slice::from_raw_parts(
            std::ptr::addr_of!((*list).mBuffers).cast::<AudioBuffer>(),
            n,
        )
    };
    bufs.iter().map(|b| b.mNumberChannels as usize).sum()
}

/// Device latency + safety offset + the first stream's latency, in frames.
// SAFETY: no caller contract: `u32` property reads; a stale `device` gives 0.
unsafe fn extra_latency(device: AudioObjectID, scope: AudioObjectPropertyScope) -> u32 {
    let dev = get::<u32>(device, kAudioDevicePropertyLatency, scope).unwrap_or(0);
    let safety = get::<u32>(device, kAudioDevicePropertySafetyOffset, scope).unwrap_or(0);
    let stream = get_bytes(device, kAudioDevicePropertyStreams, scope)
        .and_then(|(ids, size)| object_ids(&ids, size).first().copied())
        .filter(|first| *first != 0)
        .and_then(|s| {
            get::<u32>(
                s,
                kAudioStreamPropertyLatency,
                kAudioObjectPropertyScopeGlobal,
            )
        })
        .unwrap_or(0);
    extra_latency_frames(dev, safety, stream)
}

/// What a device adds beyond its buffer, in frames.
fn extra_latency_frames(device: u32, safety: u32, stream: u32) -> u32 {
    device.saturating_add(safety).saturating_add(stream)
}

// SAFETY: `selector` must be a property whose value is one `CFStringRef` that the caller
// owns (the HAL's rule for its string properties): it is released when the copy is made.
unsafe fn string_property(
    device: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Option<String> {
    let raw = get::<*const CFString>(device, selector, kAudioObjectPropertyScopeGlobal)?;
    let s = CFRetained::from_raw(NonNull::new(raw.cast_mut())?);
    Some(s.to_string())
}

// SAFETY: no caller contract: a string property read; a stale `device` gives a made-up name.
unsafe fn name(device: AudioObjectID) -> String {
    string_property(device, kAudioObjectPropertyName).unwrap_or_else(|| format!("device {device}"))
}

/// What an aggregate device is made of (the dictionary the HAL is given).
#[derive(Clone, Debug, PartialEq, Eq)]
struct AggregateSpec {
    uid: String,
    name: &'static str,
    /// Not shown to other programs, and gone when this one exits.
    private: bool,
    /// Stacked: the same channels on every sub-device (not wanted here).
    stacked: bool,
    /// The uid of the sub-device that is the clock.
    master: String,
    /// The sub-devices in channel order, each with whether it is drift-compensated.
    subs: [(String, bool); 2],
}

/// The uid of this process's `n`th aggregate device.
fn aggregate_uid(pid: u32, n: u32) -> String {
    format!("{AGGREGATE_UID}{pid}.{n}")
}

/// The output first (its channels come first, and it is the clock), then the input, which
/// is resampled to follow that clock.
fn aggregate_spec(out_uid: &str, in_uid: &str, pid: u32, n: u32) -> AggregateSpec {
    AggregateSpec {
        uid: aggregate_uid(pid, n),
        name: "ProWave Duplex",
        private: true,
        stacked: false,
        master: out_uid.to_owned(),
        subs: [(out_uid.to_owned(), false), (in_uid.to_owned(), true)],
    }
}

/// One entry of the sub-device list.
fn sub_device_entry(uid: &str, drift: bool) -> CFRetained<CFDictionary<CFString, CFType>> {
    let (k_uid, k_drift) = (CFString::from_str("uid"), CFString::from_str("drift"));
    let (v_uid, v_drift) = (CFString::from_str(uid), CFNumber::new_i32(i32::from(drift)));
    CFDictionary::<CFString, CFType>::from_slices(&[&*k_uid, &*k_drift], &[&*v_uid, &*v_drift])
}

/// `spec` as the dictionary `AudioHardwareCreateAggregateDevice` takes.
fn aggregate_description(spec: &AggregateSpec) -> CFRetained<CFDictionary<CFString, CFType>> {
    let key = |s: &str| CFString::from_str(s);
    let subs = [
        sub_device_entry(&spec.subs[0].0, spec.subs[0].1),
        sub_device_entry(&spec.subs[1].0, spec.subs[1].1),
    ];
    let sub_list = CFArray::from_objects(&[&*subs[0], &*subs[1]]);
    let keys = [
        key("uid"),
        key("name"),
        key("private"),
        key("stacked"),
        key("master"),
        key("subdevices"),
    ];
    let (v_uid, v_name) = (CFString::from_str(&spec.uid), CFString::from_str(spec.name));
    let (v_private, v_stacked) = (
        CFNumber::new_i32(i32::from(spec.private)),
        CFNumber::new_i32(i32::from(spec.stacked)),
    );
    let v_master = CFString::from_str(&spec.master);
    let values: [&CFType; 6] = [
        &v_uid, &v_name, &v_private, &v_stacked, &v_master, &sub_list,
    ];
    let key_refs: Vec<&CFString> = keys.iter().map(|k| &**k).collect();
    CFDictionary::<CFString, CFType>::from_slices(&key_refs, &values)
}

/// A private aggregate device of `output` (the clock) and `input` (drift-compensated).
// SAFETY: `output` and `input` must be HAL device ids. The description is a live
// CFDictionary for the length of the create call, and the device made is either returned
// (the caller destroys it) or destroyed here.
unsafe fn make_aggregate(
    output: AudioObjectID,
    input: AudioObjectID,
) -> Result<AudioObjectID, String> {
    let out_uid = string_property(output, kAudioDevicePropertyDeviceUID).ok_or("no output uid")?;
    let in_uid = string_property(input, kAudioDevicePropertyDeviceUID).ok_or("no input uid")?;
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // A new uid each time: removing the one before finishes asynchronously.
    let desc = aggregate_description(&aggregate_spec(&out_uid, &in_uid, std::process::id(), n));
    let mut agg: AudioObjectID = 0;
    let status = AudioHardwareCreateAggregateDevice(desc.as_opaque(), NonNull::from(&mut agg));
    if status != 0 || agg == 0 {
        return Err(format!(
            "AudioHardwareCreateAggregateDevice failed ({status})"
        ));
    }
    // The HAL builds the device's streams asynchronously.
    for _ in 0..50 {
        if channels(agg, kAudioObjectPropertyScopeInput) > 0
            && channels(agg, kAudioObjectPropertyScopeOutput) > 0
        {
            return Ok(agg);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    AudioHardwareDestroyAggregateDevice(agg);
    Err("the aggregate device did not come up".into())
}

#[cfg(test)]
#[path = "duplex_tests.rs"]
mod tests;
