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
    // SAFETY: plain HAL property calls; every id comes from the HAL.
    unsafe {
        let (def_in, def_out) = (
            system_device(kAudioHardwarePropertyDefaultInputDevice),
            system_device(kAudioHardwarePropertyDefaultOutputDevice),
        );
        all_devices()
            .into_iter()
            .filter_map(|d| {
                let uid = string_property(d, kAudioDevicePropertyDeviceUID)?;
                (!uid.starts_with(AGGREGATE_UID)).then(|| DeviceInfo {
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

// SAFETY: the stream only holds ids and the context pointer, which is used by the HAL's
// thread until the proc is stopped; nothing here is tied to the creating thread.
unsafe impl Send for DuplexStream {}

impl Drop for DuplexStream {
    fn drop(&mut self) {
        // SAFETY: once the proc is stopped and destroyed the HAL no longer calls it, so the
        // context can be freed.
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

unsafe fn free_ctx<F>(p: *mut c_void) {
    drop(Box::from_raw(p.cast::<Ctx<F>>()));
}

/// Points the context at a newly opened device (the proc is stopped meanwhile).
unsafe fn configure_ctx<F>(p: *mut c_void, info: &DuplexInfo) {
    let ctx = &mut *p.cast::<Ctx<F>>();
    let to_ns = |f: u32| (f64::from(f) * 1e9 / f64::from(info.sample_rate.max(1))) as u64;
    ctx.in_skip = info.in_skip;
    ctx.in_channels = info.in_channels;
    ctx.out_channels = info.out_channels;
    ctx.in_extra_ns = to_ns(info.input_latency_frames);
    ctx.out_extra_ns = to_ns(info.output_latency_frames);
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
        // An aggregate lists its sub-devices in order, the output first: only the input's
        // inputs and the output's outputs are ours.
        let (in_skip, in_channels, out_channels) = if aggregate.is_some() {
            let skip = channels(out_dev, kAudioObjectPropertyScopeInput);
            let ins = channels(device, kAudioObjectPropertyScopeInput).saturating_sub(skip);
            let outs = channels(device, kAudioObjectPropertyScopeOutput)
                .min(channels(out_dev, kAudioObjectPropertyScopeOutput));
            (skip, ins, outs)
        } else if in_dev.is_some() {
            let ins = channels(device, kAudioObjectPropertyScopeInput);
            (0, ins, channels(device, kAudioObjectPropertyScopeOutput))
        } else {
            (0, 0, channels(device, kAudioObjectPropertyScopeOutput))
        };
        let (in_channels, out_channels) = (
            in_channels.min(MAX_IN_CHANNELS),
            out_channels.min(MAX_OUT_CHANNELS),
        );
        if out_channels == 0 {
            return Err("the device has no output channels".into());
        }
        // The latencies of the real devices; an aggregate's own figures are not documented.
        let in_lat = in_dev.map_or(0, |d| extra_latency(d, kAudioObjectPropertyScopeInput));
        let out_lat = extra_latency(out_dev, kAudioObjectPropertyScopeOutput);
        let out_name = name(out_dev);
        let input_name = in_dev.map(|d| name(d));
        let info = DuplexInfo {
            device_name: match (&input_name, aggregate) {
                (Some(i), Some(_)) => format!("{i} + {out_name}"),
                _ => out_name,
            },
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
        // SAFETY: plain HAL calls; the context lives until `Drop` has stopped the proc.
        unsafe {
            let opened = Opened::open(devices, buffer_frames, None)?;
            let ctx = Box::new(Ctx {
                f: make(&opened.info),
                // The largest a device can ask for, so a new device never needs a new one.
                input: vec![0.0; MAX_FRAMES * MAX_IN_CHANNELS],
                output: vec![0.0; MAX_FRAMES * MAX_OUT_CHANNELS],
                in_skip: 0,
                in_channels: 0,
                out_channels: 0,
                in_extra_ns: 0,
                out_extra_ns: 0,
            });
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
    /// same sample rate. If they cannot be opened, the devices before are opened again.
    pub fn switch(&mut self, devices: &Devices) -> Result<DuplexInfo, String> {
        let (frames, rate) = (self.opened.info.buffer_frames, self.opened.info.sample_rate);
        // SAFETY: the proc is stopped before the context is changed.
        unsafe {
            self.stop();
            match Opened::open(devices, Some(frames), Some(rate)) {
                Ok(opened) => {
                    self.opened = opened;
                    self.devices = devices.clone();
                    (self.configure)(self.ctx, &self.opened.info);
                    self.run()?;
                    Ok(self.opened.info.clone())
                }
                Err(e) => {
                    let old = self.devices.clone();
                    if let Ok(opened) = Opened::open(&old, Some(frames), Some(rate)) {
                        self.opened = opened;
                        (self.configure)(self.ctx, &self.opened.info);
                        let _ = self.run();
                    }
                    Err(e)
                }
            }
        }
    }

    /// Asks for `frames` per callback while running (Logic: I/O Buffer Size). Returns what
    /// the device took.
    pub fn set_buffer_frames(&mut self, frames: u32) -> u32 {
        // SAFETY: a property of the open device; the callback copes with any size.
        let got = unsafe { set_frames(self.opened.device, frames) };
        self.opened.info.buffer_frames = got;
        got
    }

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
    // SAFETY: a plain HAL property read of a pointer-sized value.
    let wg = unsafe {
        get::<*mut c_void>(
            device,
            kAudioDevicePropertyIOThreadOSWorkgroup,
            kAudioObjectPropertyScopeGlobal,
        )
    }?;
    (!wg.is_null()).then_some(wg)
}

unsafe fn set_frames(device: AudioObjectID, wanted: u32) -> u32 {
    let wanted = wanted.max(1);
    let frames = match get::<AudioValueRange>(
        device,
        kAudioDevicePropertyBufferFrameSizeRange,
        kAudioObjectPropertyScopeGlobal,
    ) {
        Some(r) => (f64::from(wanted)).clamp(r.mMinimum, r.mMaximum) as u32,
        None => wanted,
    }
    .min(MAX_FRAMES as u32);
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

unsafe fn all_devices() -> Vec<AudioObjectID> {
    let Some((buf, size)) = get_bytes(
        kAudioObjectSystemObject as AudioObjectID,
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    ) else {
        return Vec::new();
    };
    let n = size / std::mem::size_of::<AudioObjectID>();
    std::slice::from_raw_parts(buf.as_ptr().cast::<AudioObjectID>(), n).to_vec()
}

unsafe fn device_by_uid(uid: &str) -> Option<AudioObjectID> {
    all_devices()
        .into_iter()
        .find(|d| string_property(*d, kAudioDevicePropertyDeviceUID).as_deref() == Some(uid))
}

/// The host clock (`mach_absolute_time` units, what CoreMIDI time stamps use) for `ns`
/// nanoseconds of it.
pub fn host_time_from_ns(ns: u64) -> u64 {
    // SAFETY: a pure conversion with the host's time base.
    unsafe { objc2_core_audio::AudioConvertNanosToHostTime(ns) }
}

/// The host clock now, in nanoseconds.
pub fn host_now_ns() -> u64 {
    // SAFETY: plain reads of the host clock.
    unsafe {
        objc2_core_audio::AudioConvertHostTimeToNanos(objc2_core_audio::AudioGetCurrentHostTime())
    }
}

/// The buffers of an `AudioBufferList`.
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
    // SAFETY: a pure conversion with the host's time base.
    unsafe { AudioConvertHostTimeToNanos(later - earlier) }
}

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
    let outs = buffers(output);
    let Some(frames) = outs
        .iter()
        .find(|b| b.mNumberChannels > 0)
        .map(|b| b.mDataByteSize as usize / 4 / b.mNumberChannels as usize)
    else {
        return 0;
    };
    let frames = frames.min(MAX_FRAMES);
    let (now, in_t, out_t) = (now.as_ref(), input_time.as_ref(), output_time.as_ref());

    // The input, gathered from every stream into one interleaved block.
    let ic = ctx.in_channels;
    let in_len = if ic > 0 { frames * ic } else { 0 };
    let gathered = &mut ctx.input[..in_len];
    gathered.fill(0.0);
    if ic > 0 {
        let (skip, mut base) = (ctx.in_skip, 0);
        for b in buffers(input) {
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
    let output_latency_us = ((host_ns(out_t.mHostTime, now.mHostTime) + ctx.out_extra_ns) / 1000)
        .min(u32::MAX as u64) as u32;
    let input_latency_us = ((host_ns(now.mHostTime, in_t.mHostTime) + ctx.in_extra_ns) / 1000)
        .min(u32::MAX as u64) as u32;
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
        let have = (b.mDataByteSize as usize / 4 / n).min(frames);
        let dst = std::slice::from_raw_parts_mut(b.mData.cast::<f32>(), have * n);
        for (f, frame) in dst.chunks_exact_mut(n).enumerate() {
            for (c, s) in frame.iter_mut().enumerate() {
                let ch = base + c;
                *s = if ch < oc {
                    ctx.output[f * oc + ch]
                } else {
                    0.0
                };
            }
        }
        base += n;
    }
    0
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

unsafe fn system_device(selector: AudioObjectPropertySelector) -> Option<AudioObjectID> {
    get::<AudioObjectID>(
        kAudioObjectSystemObject as AudioObjectID,
        selector,
        kAudioObjectPropertyScopeGlobal,
    )
    .filter(|d| *d != 0)
}

/// Channels of `device` in `scope`, over all its streams.
unsafe fn channels(device: AudioObjectID, scope: AudioObjectPropertyScope) -> usize {
    let Some((buf, _)) = get_bytes(device, kAudioDevicePropertyStreamConfiguration, scope) else {
        return 0;
    };
    let list = NonNull::new_unchecked(buf.as_ptr().cast_mut().cast::<AudioBufferList>());
    buffers(list)
        .iter()
        .map(|b| b.mNumberChannels as usize)
        .sum()
}

/// Device latency + safety offset + the first stream's latency, in frames.
unsafe fn extra_latency(device: AudioObjectID, scope: AudioObjectPropertyScope) -> u32 {
    let dev = get::<u32>(device, kAudioDevicePropertyLatency, scope).unwrap_or(0);
    let safety = get::<u32>(device, kAudioDevicePropertySafetyOffset, scope).unwrap_or(0);
    let stream = get_bytes(device, kAudioDevicePropertyStreams, scope)
        .and_then(|(ids, size)| {
            if size < 4 {
                return None;
            }
            let first = *ids.as_ptr().cast::<AudioObjectID>();
            (first != 0).then_some(first)
        })
        .and_then(|s| {
            get::<u32>(
                s,
                kAudioStreamPropertyLatency,
                kAudioObjectPropertyScopeGlobal,
            )
        })
        .unwrap_or(0);
    dev + safety + stream
}

unsafe fn string_property(
    device: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Option<String> {
    let raw = get::<*const CFString>(device, selector, kAudioObjectPropertyScopeGlobal)?;
    let s = CFRetained::from_raw(NonNull::new(raw.cast_mut())?);
    Some(s.to_string())
}

unsafe fn name(device: AudioObjectID) -> String {
    string_property(device, kAudioObjectPropertyName).unwrap_or_else(|| format!("device {device}"))
}

/// A private aggregate device of `output` (the clock) and `input` (drift-compensated).
unsafe fn make_aggregate(
    output: AudioObjectID,
    input: AudioObjectID,
) -> Result<AudioObjectID, String> {
    let out_uid = string_property(output, kAudioDevicePropertyDeviceUID).ok_or("no output uid")?;
    let in_uid = string_property(input, kAudioDevicePropertyDeviceUID).ok_or("no input uid")?;
    let key = |s: &str| CFString::from_str(s);
    let sub = |uid: &str, drift: bool| {
        let (k_uid, k_drift) = (key("uid"), key("drift"));
        let (v_uid, v_drift) = (CFString::from_str(uid), CFNumber::new_i32(i32::from(drift)));
        CFDictionary::<CFString, CFType>::from_slices(&[&*k_uid, &*k_drift], &[&*v_uid, &*v_drift])
    };
    let subs = [sub(&out_uid, false), sub(&in_uid, true)];
    let sub_list = CFArray::from_objects(&[&*subs[0], &*subs[1]]);
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // A new uid each time: removing the one before finishes asynchronously.
    let uid = format!("{AGGREGATE_UID}{}.{n}", std::process::id());
    let keys = [
        key("uid"),
        key("name"),
        key("private"),
        key("stacked"),
        key("master"),
        key("subdevices"),
    ];
    let (v_uid, v_name) = (
        CFString::from_str(&uid),
        CFString::from_str("ProWave Duplex"),
    );
    let (yes, no) = (CFNumber::new_i32(1), CFNumber::new_i32(0));
    let v_master = CFString::from_str(&out_uid);
    let values: [&CFType; 6] = [&v_uid, &v_name, &yes, &no, &v_master, &sub_list];
    let key_refs: Vec<&CFString> = keys.iter().map(|k| &**k).collect();
    let desc = CFDictionary::<CFString, CFType>::from_slices(&key_refs, &values);
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
