//! macOS: hosting the Audio Units installed on this Mac (Logic: AU instruments and Audio FX).
//!
//! Version 2 Audio Units (`.component` bundles) are opened in this process with the
//! AudioToolbox C API. An instance is made and set up on the UI thread ([`AuInstance::new`]
//! can take seconds: some instruments load sample libraries or ask for a license), then moved
//! to the audio thread, where [`AuInstance::process`] renders one block. The audio is 32-bit
//! float, two non-interleaved channels, at the engine's sample rate.
use objc2_audio_toolbox::{
    kAudioUnitProperty_HostCallbacks, kAudioUnitProperty_Latency,
    kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitProperty_SetRenderCallback,
    kAudioUnitProperty_StreamFormat, kAudioUnitScope_Global, kAudioUnitScope_Input,
    kAudioUnitScope_Output, kAudioUnitType_Effect, kAudioUnitType_MusicDevice,
    kAudioUnitType_MusicEffect, AURenderCallbackStruct, AudioComponent, AudioComponentCopyName,
    AudioComponentDescription, AudioComponentFindNext, AudioComponentGetDescription,
    AudioComponentInstanceDispose, AudioComponentInstanceNew, AudioUnit, AudioUnitGetProperty,
    AudioUnitInitialize, AudioUnitRender, AudioUnitRenderActionFlags, AudioUnitReset,
    AudioUnitSetProperty, AudioUnitUninitialize, MusicDeviceMIDIEvent,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, kAudioFormatFlagIsPacked,
    kAudioFormatLinearPCM, AudioBuffer, AudioBufferList, AudioStreamBasicDescription,
    AudioTimeStamp, AudioTimeStampFlags,
};
use objc2_core_foundation::{CFRetained, CFString};
use std::ffi::c_void;
use std::ptr::NonNull;

use crate::au_host_time::HostTime;
use crate::plugin_track::BlockTransport;
use crate::plugin_window_layout::WindowLayout;

/// What an Audio Unit is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuKind {
    /// Plays notes (`aumu`).
    Instrument,
    /// Processes audio (`aufx`).
    Effect,
    /// Processes audio and also takes MIDI (`aumf`).
    MusicEffect,
}

/// An Audio Unit on this Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuInfo {
    /// `type:subtype:manufacturer`, e.g. `aufx:DEC :SToy`: what a project saves.
    pub id: String,
    pub name: String,
    pub manufacturer: String,
    pub kind: AuKind,
    desc: [u32; 3],
}

impl AuInfo {
    /// The name as a menu shows it.
    pub fn label(&self) -> String {
        format!("{}: {}", self.manufacturer, self.name)
    }
}

fn fourcc(n: u32) -> String {
    n.to_be_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '?'
            }
        })
        .collect()
}

fn from_fourcc(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    (b.len() == 4).then(|| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn parse_id(id: &str) -> Option<[u32; 3]> {
    let mut parts = id.split(':');
    let t = from_fourcc(parts.next()?)?;
    let s = from_fourcc(parts.next()?)?;
    let m = from_fourcc(parts.next()?)?;
    parts.next().is_none().then_some([t, s, m])
}

/// Every instrument and effect Audio Unit the system knows, by kind, then manufacturer and
/// name.
pub fn list() -> Vec<AuInfo> {
    let mut out = Vec::new();
    for (ty, kind) in [
        (kAudioUnitType_MusicDevice, AuKind::Instrument),
        (kAudioUnitType_Effect, AuKind::Effect),
        (kAudioUnitType_MusicEffect, AuKind::MusicEffect),
    ] {
        let mut want = AudioComponentDescription {
            componentType: ty,
            componentSubType: 0,
            componentManufacturer: 0,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        let mut comp: AudioComponent = std::ptr::null_mut();
        loop {
            // SAFETY: the description lives across the call; a null component starts the
            // search and the result is the next match or null.
            comp = unsafe { AudioComponentFindNext(comp, NonNull::from(&mut want)) };
            if comp.is_null() {
                break;
            }
            if let Some(info) = unsafe { describe(comp, kind) } {
                out.push(info);
            }
        }
    }
    out.sort_by(|a, b| {
        (
            a.kind as u8,
            a.manufacturer.to_lowercase(),
            a.name.to_lowercase(),
        )
            .cmp(&(
                b.kind as u8,
                b.manufacturer.to_lowercase(),
                b.name.to_lowercase(),
            ))
    });
    out
}

unsafe fn describe(comp: AudioComponent, kind: AuKind) -> Option<AuInfo> {
    let mut d = std::mem::zeroed::<AudioComponentDescription>();
    if AudioComponentGetDescription(comp, NonNull::from(&mut d)) != 0 {
        return None;
    }
    let mut name: *const CFString = std::ptr::null();
    let full = if AudioComponentCopyName(comp, NonNull::from(&mut name)) == 0 {
        NonNull::new(name.cast_mut())
            .map(|n| CFRetained::from_raw(n).to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let (manufacturer, name) = match full.split_once(": ") {
        Some((m, n)) => (m.to_string(), n.to_string()),
        None => (fourcc(d.componentManufacturer), full),
    };
    let desc = [d.componentType, d.componentSubType, d.componentManufacturer];
    Some(AuInfo {
        id: format!(
            "{}:{}:{}",
            fourcc(desc[0]),
            fourcc(desc[1]),
            fourcc(desc[2])
        ),
        name,
        manufacturer,
        kind,
        desc,
    })
}

/// The Audio Unit with this [`AuInfo::id`], if it is installed.
pub fn find(id: &str) -> Option<AuInfo> {
    let want = parse_id(id)?;
    list().into_iter().find(|i| i.desc == want)
}

/// An `AudioBufferList` with room for two buffers.
#[repr(C)]
struct StereoList {
    n: u32,
    buffers: [AudioBuffer; 2],
}

/// What the input callback reads: the block an effect processes.
struct Feed {
    l: Vec<f32>,
    r: Vec<f32>,
    frames: usize,
}

/// One running Audio Unit. Make it on the UI thread, process it on the audio thread, drop
/// it on the UI thread (disposing can take a while).
pub struct AuInstance {
    unit: AudioUnit,
    kind: AuKind,
    feed: Box<Feed>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    list: StereoList,
    sample_time: f64,
    max_frames: usize,
    latency_s: f64,
    errors: u64,
    /// The unit renders one channel; it is copied to the other.
    mono_out: bool,
    /// The tempo and position its host callbacks answer with (the unit holds its address).
    host: Box<HostTime>,
    /// The transport of the current block, and the frames rendered since it came.
    transport: BlockTransport,
    rendered: i64,
    sample_rate: f64,
    /// Whether the unit took the host callbacks.
    host_callbacks: bool,
}

// SAFETY: an Audio Unit may be used from another thread than the one that made it, as long
// as one thread at a time does; the engine hands it over through a queue.
unsafe impl Send for AuInstance {}

impl Drop for AuInstance {
    fn drop(&mut self) {
        // SAFETY: the unit was made by AudioComponentInstanceNew and is used no more.
        unsafe {
            AudioUnitUninitialize(self.unit);
            AudioComponentInstanceDispose(self.unit);
        }
    }
}

fn stereo_format(sample_rate: f64) -> AudioStreamBasicDescription {
    format_of(sample_rate, 2)
}

/// Float, non-interleaved, `channels` channels.
fn format_of(sample_rate: f64, channels: u32) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: sample_rate,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat
            | kAudioFormatFlagIsPacked
            | kAudioFormatFlagIsNonInterleaved,
        mBytesPerPacket: 4,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4,
        mChannelsPerFrame: channels,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

unsafe fn set_prop<T>(unit: AudioUnit, id: u32, scope: u32, element: u32, value: &T) -> i32 {
    AudioUnitSetProperty(
        unit,
        id,
        scope,
        element,
        (value as *const T).cast(),
        std::mem::size_of::<T>() as u32,
    )
}

unsafe extern "C-unwind" fn input_proc(
    ref_con: NonNull<c_void>,
    _flags: NonNull<AudioUnitRenderActionFlags>,
    _time: NonNull<AudioTimeStamp>,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> i32 {
    let feed = &mut *ref_con.as_ptr().cast::<Feed>();
    let Some(list) = data.as_mut() else {
        return 0;
    };
    let n = (list.mNumberBuffers as usize).min(2);
    let bufs = std::slice::from_raw_parts_mut(
        std::ptr::addr_of_mut!(list.mBuffers).cast::<AudioBuffer>(),
        n,
    );
    let frames = (frames as usize).min(feed.frames).min(feed.l.len());
    for (i, b) in bufs.iter_mut().enumerate() {
        let src = if i == 0 { &mut feed.l } else { &mut feed.r };
        if b.mData.is_null() {
            // The unit lets the host supply the memory.
            b.mData = src.as_mut_ptr().cast();
        } else {
            let dst = std::slice::from_raw_parts_mut(b.mData.cast::<f32>(), frames);
            dst.copy_from_slice(&src[..frames]);
        }
        b.mDataByteSize = (frames * 4) as u32;
        b.mNumberChannels = 1;
    }
    0
}

/// Plug-in windows float above the main window, as in Logic: clicking the transport or the
/// tracks does not cover them. They hide while another app is in front.
pub fn float_above_main(window: &objc2_app_kit::NSWindow) {
    // NSFloatingWindowLevel.
    window.setLevel(3);
    window.setHidesOnDeactivate(true);
}

/// The system preset folder u-he units require (see [`AuInstance::new`]).
pub const UHE_SYSTEM_PRESETS: &str = "/Library/Audio/Presets/u-he";

impl AuInstance {
    /// Opens `info` at `sample_rate` for blocks of up to `max_frames`. Can take seconds.
    pub fn new(info: &AuInfo, sample_rate: u32, max_frames: usize) -> Result<Self, String> {
        let mut want = AudioComponentDescription {
            componentType: info.desc[0],
            componentSubType: info.desc[1],
            componentManufacturer: info.desc[2],
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        // u-he units try to create /Library/Audio/Presets/u-he when the program has an
        // NSApplication; without permission to, they free a bad pointer and abort the whole
        // process. Their installer makes that folder; a user-folder install does not.
        if info.desc[2] == u32::from_be_bytes(*b"UHfX")
            && !std::path::Path::new(UHE_SYSTEM_PRESETS).is_dir()
        {
            return Err(format!(
                "{} needs the folder {UHE_SYSTEM_PRESETS}, which is missing (it would crash Pro Wave)",
                info.label()
            ));
        }
        // SAFETY: plain AudioToolbox calls on a unit made here; every pointer passed lives
        // across its call, and the feed is boxed so its address stays put.
        unsafe {
            let comp = AudioComponentFindNext(std::ptr::null_mut(), NonNull::from(&mut want));
            if comp.is_null() {
                return Err(format!("{} is not installed", info.label()));
            }
            let mut unit: AudioUnit = std::ptr::null_mut();
            let status = AudioComponentInstanceNew(comp, NonNull::from(&mut unit));
            if status != 0 || unit.is_null() {
                return Err(format!("{} did not open ({status})", info.label()));
            }
            let max_frames = max_frames.max(64);
            let mut au = Self {
                unit,
                kind: info.kind,
                feed: Box::new(Feed {
                    l: vec![0.0; max_frames],
                    r: vec![0.0; max_frames],
                    frames: 0,
                }),
                out_l: vec![0.0; max_frames],
                out_r: vec![0.0; max_frames],
                list: StereoList {
                    n: 2,
                    buffers: [std::mem::zeroed(), std::mem::zeroed()],
                },
                sample_time: 0.0,
                max_frames,
                latency_s: 0.0,
                errors: 0,
                mono_out: false,
                host: Box::new(HostTime::new(f64::from(sample_rate))),
                transport: BlockTransport::default(),
                rendered: 0,
                sample_rate: f64::from(sample_rate),
                host_callbacks: false,
            };
            // Tempo, position and transport for tempo-synced arpeggiators, LFOs and delays.
            // A unit that does not ask for them refuses the property; that is fine.
            let cb = au.host.callbacks();
            au.host_callbacks = set_prop(
                unit,
                kAudioUnitProperty_HostCallbacks,
                kAudioUnitScope_Global,
                0,
                &cb,
            ) == 0;
            let fmt = stereo_format(f64::from(sample_rate));
            let frames = max_frames as u32;
            set_prop(
                unit,
                kAudioUnitProperty_MaximumFramesPerSlice,
                kAudioUnitScope_Global,
                0,
                &frames,
            );
            let s = set_prop(
                unit,
                kAudioUnitProperty_StreamFormat,
                kAudioUnitScope_Output,
                0,
                &fmt,
            );
            if s != 0 {
                // A mono instrument (a chip emulation): it is heard in both channels.
                let mono = format_of(f64::from(sample_rate), 1);
                let m = set_prop(
                    unit,
                    kAudioUnitProperty_StreamFormat,
                    kAudioUnitScope_Output,
                    0,
                    &mono,
                );
                if m != 0 {
                    return Err(format!(
                        "{} refused stereo and mono output at {sample_rate} Hz ({s})",
                        info.label()
                    ));
                }
                au.mono_out = true;
            }
            if info.kind != AuKind::Instrument {
                let s = set_prop(
                    unit,
                    kAudioUnitProperty_StreamFormat,
                    kAudioUnitScope_Input,
                    0,
                    &fmt,
                );
                if s != 0 {
                    return Err(format!("{} refused stereo input ({s})", info.label()));
                }
                let cb = AURenderCallbackStruct {
                    inputProc: Some(input_proc),
                    inputProcRefCon: (&mut *au.feed as *mut Feed).cast(),
                };
                let s = set_prop(
                    unit,
                    kAudioUnitProperty_SetRenderCallback,
                    kAudioUnitScope_Input,
                    0,
                    &cb,
                );
                if s != 0 {
                    return Err(format!("{} took no input callback ({s})", info.label()));
                }
            }
            let s = AudioUnitInitialize(unit);
            if s != 0 {
                return Err(format!("{} did not start ({s})", info.label()));
            }
            let mut lat = 0f64;
            let mut size = std::mem::size_of::<f64>() as u32;
            if AudioUnitGetProperty(
                unit,
                kAudioUnitProperty_Latency,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut lat).cast(),
                NonNull::from(&mut size),
            ) == 0
                && lat.is_finite()
            {
                au.latency_s = lat.max(0.0);
            }
            Ok(au)
        }
    }

    pub fn kind(&self) -> AuKind {
        self.kind
    }

    /// A handle for the plug-in's window and settings, used from the UI thread while the
    /// instance renders on the audio thread. Valid as long as this instance lives.
    pub fn handle(&self) -> AuHandle {
        AuHandle(self.unit)
    }

    /// The delay the unit puts on the sound, in seconds (it can change with its settings).
    pub fn latency_seconds(&self) -> f64 {
        self.latency_s
    }

    /// Blocks the unit failed to render (they were silent).
    pub fn errors(&self) -> u64 {
        self.errors
    }

    /// A MIDI message for instruments and music effects, `offset` frames into the next
    /// block. Audio thread.
    pub fn midi(&mut self, status: u8, data1: u8, data2: u8, offset: u32) {
        if self.kind == AuKind::Effect {
            return;
        }
        // SAFETY: the unit is open; MusicDeviceMIDIEvent is meant for the render thread.
        unsafe {
            MusicDeviceMIDIEvent(
                self.unit,
                u32::from(status),
                u32::from(data1),
                u32::from(data2),
                offset,
            );
        }
    }

    /// Clears the unit's state (reverb tails, held notes). Not on the audio thread.
    pub fn reset(&mut self) {
        // SAFETY: the unit is open.
        unsafe {
            AudioUnitReset(self.unit, kAudioUnitScope_Global, 0);
        }
    }

    /// Whether the unit took the host callbacks (tempo, position, transport).
    pub fn has_host_callbacks(&self) -> bool {
        self.host_callbacks
    }

    /// Host callback calls the unit has made so far (diagnostics: does it follow the tempo).
    pub fn host_callback_calls(&self) -> u32 {
        self.host.calls()
    }

    /// Where the transport stands at the start of the next block: what the unit's host
    /// callbacks answer while it renders (the snapshot a VST 3 plug-in gets as its process
    /// context). Audio thread, no allocation.
    pub fn set_transport(&mut self, t: &BlockTransport) {
        if crate::au_host_time::discontinuous(&self.transport, t, self.rendered) {
            self.host.mark_changed();
        }
        self.transport = *t;
        self.rendered = 0;
    }

    /// Renders one block: `input` (an effect's; ignored by instruments) into `out`. Blocks
    /// longer than the unit was opened for are done in pieces. Audio thread, no allocation.
    pub fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        let frames = out.0.len().min(out.1.len());
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.max_frames);
            // Each piece answers with its own start (the block's, `rendered` frames on).
            let t = crate::plugin_track::advance(
                self.transport,
                self.rendered as u64,
                self.sample_rate as f32,
            );
            self.host.store(&t);
            if self.kind != AuKind::Instrument {
                let (il, ir) = input;
                for i in 0..n {
                    self.feed.l[i] = il.get(done + i).copied().unwrap_or(0.0);
                    self.feed.r[i] = ir.get(done + i).copied().unwrap_or(0.0);
                }
                self.feed.frames = n;
            }
            let ok = self.render(n);
            self.rendered += n as i64;
            for i in 0..n {
                out.0[done + i] = if ok { self.out_l[i] } else { 0.0 };
                out.1[done + i] = if ok { self.out_r[i] } else { 0.0 };
            }
            done += n;
        }
    }

    fn render(&mut self, n: usize) -> bool {
        let bytes = (n * 4) as u32;
        self.list.n = if self.mono_out { 1 } else { 2 };
        self.list.buffers[0] = AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: bytes,
            mData: self.out_l.as_mut_ptr().cast(),
        };
        self.list.buffers[1] = AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: bytes,
            mData: self.out_r.as_mut_ptr().cast(),
        };
        // SAFETY: an all-zero time stamp is valid; the fields used are set below.
        let mut ts: AudioTimeStamp = unsafe { std::mem::zeroed() };
        ts.mSampleTime = self.sample_time;
        ts.mFlags = AudioTimeStampFlags::SampleTimeValid;
        let mut flags = AudioUnitRenderActionFlags(0);
        // SAFETY: the list points at buffers of `n` frames that live across the call.
        let status = unsafe {
            AudioUnitRender(
                self.unit,
                &mut flags,
                NonNull::from(&ts),
                0,
                n as u32,
                NonNull::from(&mut self.list).cast(),
            )
        };
        self.sample_time += n as f64;
        if status != 0 {
            self.errors += 1;
            return false;
        }
        // A unit may answer with its own buffers instead of filling ours.
        let used = self.list.n as usize;
        for (i, mine) in [self.out_l.as_mut_ptr(), self.out_r.as_mut_ptr()]
            .into_iter()
            .enumerate()
            .take(used)
        {
            let b = self.list.buffers[i];
            if !b.mData.is_null() && b.mData.cast::<f32>() != mine {
                // SAFETY: the unit's buffer holds `n` frames until the next render.
                unsafe { std::ptr::copy_nonoverlapping(b.mData.cast::<f32>(), mine, n) };
            }
        }
        if self.mono_out {
            let (l, r) = (&self.out_l[..n], &mut self.out_r[..n]);
            r.copy_from_slice(l);
        }
        true
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        assert_eq!(
            parse_id("aufx:DEC :SToy").map(|d| fourcc(d[1])),
            Some("DEC ".into())
        );
        assert_eq!(parse_id("aufx:DEC"), None);
        assert_eq!(parse_id("aufx:DEC :SToy:x"), None);
    }

    /// Apple's own units are on every Mac: one of each kind opens and renders.
    #[test]
    fn apple_units_open_and_render() {
        let all = list();
        let synth = all
            .iter()
            .find(|i| i.id == "aumu:dls :appl")
            .expect("DLSMusicDevice is on every Mac");
        let mut au = AuInstance::new(synth, 48_000, 512).expect("opens");
        au.midi(0x90, 60, 100, 0);
        let (mut l, mut r) = (vec![0f32; 512], vec![0f32; 512]);
        let mut peak = 0f32;
        for _ in 0..20 {
            au.process((&[], &[]), (&mut l, &mut r));
            peak = l.iter().chain(&r).fold(peak, |m, x| m.max(x.abs()));
        }
        assert!(peak > 0.001, "the note sounds: {peak}");
        let delay = all
            .iter()
            .find(|i| i.id == "aufx:dely:appl")
            .expect("AUDelay is on every Mac");
        let mut fx = AuInstance::new(delay, 48_000, 256).expect("opens");
        let input = vec![0.5f32; 256];
        fx.process((&input, &input), (&mut l[..256], &mut r[..256]));
        assert_eq!(fx.errors(), 0);
    }

    /// Apple's units take the host callbacks; the ones registered answer with the transport
    /// of the block being rendered. Also lists which of Apple's units ask for it.
    #[test]
    fn apple_units_take_the_host_callbacks() {
        use objc2_audio_toolbox::HostCallbackInfo;
        let delay = find("aufx:dely:appl").expect("AUDelay is on every Mac");
        let mut fx = AuInstance::new(&delay, 48_000, 256).expect("opens");
        assert!(fx.has_host_callbacks());
        let t = BlockTransport {
            playing: true,
            recording: false,
            sample_pos: 141_000,
            beat: 5.5,
            bar_start_beat: 4.0,
            tempo: 117.02,
            sig: (4, 4),
            cycle: None,
        };
        fx.set_transport(&t);
        let input = vec![0.25f32; 256];
        let (mut l, mut r) = (vec![0f32; 256], vec![0f32; 256]);
        assert_no_alloc::assert_no_alloc(|| fx.process((&input, &input), (&mut l, &mut r)));
        // What the unit holds is what was registered.
        // SAFETY: a zeroed HostCallbackInfo is valid (null pointers); the unit writes it.
        let mut cb: HostCallbackInfo = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of::<HostCallbackInfo>() as u32;
        // SAFETY: `cb` and `size` live across the call.
        let s = unsafe {
            AudioUnitGetProperty(
                fx.unit,
                kAudioUnitProperty_HostCallbacks,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut cb).cast(),
                NonNull::from(&mut size),
            )
        };
        assert_eq!(s, 0);
        assert_eq!(cb.hostUserData, fx.host.callbacks().hostUserData);
        let (mut beat, mut tempo) = (0.0, 0.0);
        // SAFETY: the registered callback with its own user data and local outputs.
        let s =
            unsafe { cb.beatAndTempoProc.expect("set")(cb.hostUserData, &mut beat, &mut tempo) };
        assert_eq!(s, 0);
        // The last piece rendered started at the block's start: beat 5.5 at 117.02.
        assert_eq!((beat, tempo), (5.5, 117.02));
        // A block twice as long as the unit renders at once: the second piece is 256 frames on.
        let long = vec![0.25f32; 512];
        let (mut l2, mut r2) = (vec![0f32; 512], vec![0f32; 512]);
        fx.set_transport(&BlockTransport {
            sample_pos: t.sample_pos + 256,
            beat: t.beat + 256.0 / 48_000.0 * 117.02 / 60.0,
            ..t
        });
        fx.process((&long, &long), (&mut l2, &mut r2));
        // SAFETY: as above.
        unsafe {
            cb.beatAndTempoProc.expect("set")(cb.hostUserData, &mut beat, std::ptr::null_mut())
        };
        let want = t.beat + 512.0 / 48_000.0 * 117.02 / 60.0;
        assert!((beat - want).abs() < 1e-9, "{beat} vs {want}");

        // Which of Apple's own units ask (none has to: AUDelay has no tempo sync).
        let mut asked = Vec::new();
        for info in list().iter().filter(|i| i.id.ends_with(":appl")) {
            let Ok(mut u) = AuInstance::new(info, 48_000, 512) else {
                continue;
            };
            u.set_transport(&t);
            u.midi(0x90, 60, 100, 0);
            let (mut l, mut r) = (vec![0f32; 512], vec![0f32; 512]);
            let input = vec![0.1f32; 512];
            for _ in 0..4 {
                u.process((&input, &input), (&mut l, &mut r));
            }
            if u.host_callback_calls() > 0 {
                asked.push(format!("{} ({})", info.label(), u.host_callback_calls()));
            }
        }
        eprintln!("Apple units that asked for the host's tempo: {asked:?}");
    }

    #[test]
    fn audio_unit_parameters_are_listed_set_and_read_back() {
        let delay = find("aufx:dely:appl").expect("AUDelay is on every Mac");
        let mut fx = AuInstance::new(&delay, 48_000, 256).expect("opens");
        let h = fx.handle();
        let params = h.parameters();
        assert!(params.len() >= 3, "{params:?}");
        assert!(params.iter().all(|p| p.max > p.min && !p.name.is_empty()));
        // Its dry/wet mix: all dry passes the sound, all wet (an empty delay line) is silent.
        let mix = params
            .iter()
            .find(|p| p.name.to_lowercase().contains("mix"))
            .expect("a mix parameter");
        let input = vec![0.5f32; 256];
        let (mut l, mut r) = (vec![0f32; 256], vec![0f32; 256]);
        fx.set_parameter(mix.id, mix.min, 0);
        fx.process((&input, &input), (&mut l, &mut r));
        assert!((l[200] - 0.5).abs() < 0.05, "dry: {}", l[200]);
        assert_eq!(h.parameter(mix.id), Some(mix.min));
        fx.set_parameter(mix.id, mix.max, 0);
        // Not a number is not sent.
        fx.set_parameter(mix.id, f32::NAN, 0);
        fx.process((&input, &input), (&mut l, &mut r));
        assert!(l[200].abs() < 0.05, "wet: {}", l[200]);
        assert_eq!(h.parameter(mix.id), Some(mix.max));
        // Its window's moves can be listened to.
        let ids: Vec<u32> = params.iter().map(|p| p.id).collect();
        let watch = AuParamWatch::new(h, &ids).expect("a listener");
        assert!(watch.take().is_empty());
        drop(watch);
    }
}

/// The UI thread's way to an [`AuInstance`] the audio thread owns: its window and its
/// settings. Using it after the instance is dropped is undefined: close the window first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuHandle(AudioUnit);

// SAFETY: a plain pointer; the Audio Unit API is meant to be called from the UI thread for
// properties and views while another thread renders.
unsafe impl Send for AuHandle {}

impl AuHandle {
    /// The plug-in's settings (its class info property list, as binary plist bytes), for
    /// saving in a project.
    pub fn state(&self) -> Option<Vec<u8>> {
        use objc2_audio_toolbox::kAudioUnitProperty_ClassInfo;
        use objc2_core_foundation::{
            CFData, CFPropertyList, CFPropertyListCreateData, CFPropertyListFormat,
        };
        // SAFETY: the property is a retained CFPropertyListRef that we take over.
        unsafe {
            let mut list: *const c_void = std::ptr::null();
            let mut size = std::mem::size_of::<*const c_void>() as u32;
            let s = AudioUnitGetProperty(
                self.0,
                kAudioUnitProperty_ClassInfo,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut list).cast(),
                NonNull::from(&mut size),
            );
            if s != 0 || list.is_null() {
                return None;
            }
            let list: CFRetained<CFPropertyList> =
                CFRetained::from_raw(NonNull::new(list.cast_mut().cast())?);
            let data: CFRetained<CFData> = CFPropertyListCreateData(
                None,
                Some(&list),
                CFPropertyListFormat::BinaryFormat_v1_0,
                0,
                std::ptr::null_mut(),
            )?;
            Some(data.to_vec())
        }
    }

    /// Restores settings from [`AuHandle::state`].
    pub fn set_state(&self, bytes: &[u8]) -> bool {
        use objc2_audio_toolbox::kAudioUnitProperty_ClassInfo;
        use objc2_core_foundation::{CFData, CFPropertyListCreateWithData};
        // SAFETY: the list lives across the call; the unit copies what it needs.
        unsafe {
            let data = CFData::from_bytes(bytes);
            let Some(list) = CFPropertyListCreateWithData(
                None,
                Some(&data),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) else {
                return false;
            };
            let ptr: *const c_void = CFRetained::as_ptr(&list).as_ptr().cast();
            set_prop(
                self.0,
                kAudioUnitProperty_ClassInfo,
                kAudioUnitScope_Global,
                0,
                &ptr,
            ) == 0
        }
    }

    /// The plug-in's factory presets (Logic: the factory part of the plug-in's Settings
    /// menu), as (number, name), from `kAudioUnitProperty_FactoryPresets`.
    pub fn factory_presets(&self) -> Vec<(i32, String)> {
        use objc2_audio_toolbox::{kAudioUnitProperty_FactoryPresets, AUPreset};
        use objc2_core_foundation::CFArray;
        // SAFETY: the property is a retained CFArrayRef of AUPreset pointers that we take
        // over (released when `array` drops); each name lives as long as the array.
        unsafe {
            let mut list: *const c_void = std::ptr::null();
            let mut size = std::mem::size_of::<*const c_void>() as u32;
            let s = AudioUnitGetProperty(
                self.0,
                kAudioUnitProperty_FactoryPresets,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut list).cast(),
                NonNull::from(&mut size),
            );
            let Some(ptr) = NonNull::new(list.cast_mut()) else {
                return Vec::new();
            };
            if s != 0 {
                return Vec::new();
            }
            let array: CFRetained<CFArray> = CFRetained::from_raw(ptr.cast());
            (0..array.count())
                .filter_map(|k| {
                    let p = array.value_at_index(k).cast::<AUPreset>();
                    let preset = p.as_ref()?;
                    let name = preset
                        .presetName
                        .as_ref()
                        .map(|n| n.to_string())
                        .unwrap_or_default();
                    Some((preset.presetNumber, name))
                })
                .collect()
        }
    }

    /// Loads factory preset `number` (from [`AuHandle::factory_presets`]); its settings are
    /// then what [`AuHandle::state`] returns.
    pub fn set_factory_preset(&self, number: i32) -> bool {
        use objc2_audio_toolbox::{kAudioUnitProperty_PresentPreset, AUPreset};
        let preset = AUPreset {
            presetNumber: number,
            presetName: std::ptr::null(),
        };
        // SAFETY: the struct lives across the call; the unit reads the number.
        unsafe {
            set_prop(
                self.0,
                kAudioUnitProperty_PresentPreset,
                kAudioUnitScope_Global,
                0,
                &preset,
            ) == 0
        }
    }

    /// The name of the preset the plug-in says is loaded (`kAudioUnitProperty_PresentPreset`),
    /// if it gives one.
    pub fn present_preset(&self) -> Option<(i32, String)> {
        use objc2_audio_toolbox::{kAudioUnitProperty_PresentPreset, AUPreset};
        // SAFETY: the unit fills the struct; the name is retained for us (released here).
        unsafe {
            let mut preset = AUPreset {
                presetNumber: 0,
                presetName: std::ptr::null(),
            };
            let mut size = std::mem::size_of::<AUPreset>() as u32;
            let s = AudioUnitGetProperty(
                self.0,
                kAudioUnitProperty_PresentPreset,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut preset).cast(),
                NonNull::from(&mut size),
            );
            if s != 0 {
                return None;
            }
            let name = NonNull::new(preset.presetName.cast_mut())
                .map(|n| CFRetained::<CFString>::from_raw(n).to_string())
                .unwrap_or_default();
            Some((preset.presetNumber, name))
        }
    }

    /// Opens the plug-in's own window (its Cocoa view). Main thread only.
    pub fn open_window(&self, title: &str) -> Result<AuWindow, String> {
        use objc2::rc::Retained;
        use objc2::runtime::AnyObject;
        use objc2::{msg_send, MainThreadMarker, MainThreadOnly};
        use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
        use objc2_audio_toolbox::{kAudioUnitProperty_CocoaUI, AudioUnitGetPropertyInfo};
        use objc2_foundation::{NSBundle, NSPoint, NSRect, NSSize, NSString, NSURL};

        let mtm = MainThreadMarker::new().ok_or("plug-in windows open on the main thread")?;
        // SAFETY: the property is a variable-length AudioUnitCocoaViewInfo with retained CF
        // objects; the URL and class name are toll-free bridged to NSURL and NSString.
        unsafe {
            let mut size = 0u32;
            let mut writable = 0u8;
            let s = AudioUnitGetPropertyInfo(
                self.0,
                kAudioUnitProperty_CocoaUI,
                kAudioUnitScope_Global,
                0,
                &mut size,
                &mut writable,
            );
            if s != 0 || (size as usize) < 2 * std::mem::size_of::<*const c_void>() {
                return Err("this plug-in has no window".into());
            }
            let mut raw = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
            let s = AudioUnitGetProperty(
                self.0,
                kAudioUnitProperty_CocoaUI,
                kAudioUnitScope_Global,
                0,
                NonNull::new_unchecked(raw.as_mut_ptr().cast()),
                NonNull::from(&mut size),
            );
            if s != 0 {
                return Err(format!("the plug-in's window is not available ({s})"));
            }
            let url = raw[0] as *mut NSURL;
            let class = raw[1] as *mut NSString;
            let (Some(url), Some(class)) = (url.as_ref(), class.as_ref()) else {
                return Err("the plug-in's window is not available".into());
            };
            let bundle =
                NSBundle::bundleWithURL(url).ok_or("the plug-in's view bundle did not load")?;
            let cls = bundle
                .classNamed(class)
                .ok_or_else(|| format!("no view class {class}"))?;
            let factory: Option<Retained<AnyObject>> = msg_send![cls, new];
            let factory = factory.ok_or("the plug-in's view did not start")?;
            let view: Option<Retained<NSView>> = msg_send![
                &*factory,
                uiViewForAudioUnit: self.0.cast::<c_void>(),
                withSize: NSSize::new(0.0, 0.0)
            ];
            let view = view.ok_or("the plug-in made no view")?;
            // The CF objects in the property were ours to release.
            let _ = CFRetained::<objc2_core_foundation::CFType>::from_raw(NonNull::new_unchecked(
                raw[0] as *mut _,
            ));
            let _ = CFRetained::<objc2_core_foundation::CFType>::from_raw(NonNull::new_unchecked(
                raw[1] as *mut _,
            ));
            // Some views (JUCE's) give their real size only after they are in a window: they
            // sit in a container of their own size, and the window follows them (`fit`).
            let mut size = view.frame().size;
            if size.width < 50.0 || size.height < 50.0 {
                size = NSSize::new(600.0, 400.0);
            }
            let rect = NSRect::new(NSPoint::new(200.0, 200.0), size);
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            let container = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), size),
            );
            view.setFrameOrigin(NSPoint::new(0.0, 0.0));
            container.addSubview(&view);
            window.setContentView(Some(&container));
            window.setTitle(&NSString::from_str(title));
            window.center();
            float_above_main(&window);
            // Audio Unit views are scaled to the window, never given its size: an Audio Unit
            // has no way to say it can resize, and a view that claims to resize with its
            // superview (Native Instruments') takes the size but keeps drawing at its own.
            let layout = WindowLayout::new(&window, &container, size, false);
            window.makeKeyAndOrderFront(None);
            let w = AuWindow {
                window,
                view,
                layout,
            };
            w.fit();
            Ok(w)
        }
    }
}

/// A plug-in's open window. Dropping it closes it. Its green button fills the screen, and
/// its corner drags: the plug-in's view is scaled to the window, keeping its proportions. See
/// `plugin_window_layout`.
pub struct AuWindow {
    window: objc2::rc::Retained<objc2_app_kit::NSWindow>,
    view: objc2::rc::Retained<objc2_app_kit::NSView>,
    layout: WindowLayout,
}

impl AuWindow {
    /// Whether the user has not closed it.
    pub fn is_open(&self) -> bool {
        self.window.isVisible()
    }

    /// Sizes the window to the plug-in's view, which may have grown or shrunk (its own size
    /// buttons, or a late layout), at 1:1 (outside full screen). Call it now and then (once a
    /// UI frame).
    pub fn fit(&self) {
        use objc2_foundation::NSPoint;
        self.layout.tend();
        let have = self.view.frame();
        if have.size.width < 1.0 || have.size.height < 1.0 {
            return;
        }
        if have.origin != NSPoint::new(0.0, 0.0) {
            self.view.setFrameOrigin(NSPoint::new(0.0, 0.0));
        }
        self.layout.view_resized(have.size);
    }

    /// Whether it fills the screen (the green button).
    pub fn is_full_screen(&self) -> bool {
        self.layout.is_full()
    }

    /// The green button: into or out of full screen.
    pub fn toggle_full_screen(&self) {
        self.window.toggleFullScreen(None);
    }

    /// The plug-in view's size (its own, unscaled) and the window's content size, in points.
    pub fn sizes(&self) -> ((f64, f64), (f64, f64)) {
        crate::plugin_window_layout::sizes(&self.window, self.layout.natural())
    }

    /// Where a point of the plug-in's view (its own coordinates, origin bottom left) shows in
    /// the window, and the plug-in view a click there reaches, with the point it gets in that
    /// view's coordinates. Checks clicks land on the control drawn under them when scaled.
    pub fn probe_click(&self, x: f64, y: f64) -> Option<crate::plugin_window_layout::ClickProbe> {
        crate::plugin_window_layout::probe_click(&self.window, &self.view, x, y)
    }

    /// The window's number (what `screencapture -l` takes).
    pub fn window_number(&self) -> isize {
        self.window.windowNumber()
    }

    /// Brings it to the front.
    pub fn show(&self) {
        self.window.makeKeyAndOrderFront(None);
    }
}

impl Drop for AuWindow {
    fn drop(&mut self) {
        self.window.close();
    }
}

/// One parameter of an Audio Unit, as its parameter info gives it (Logic and Ableton list
/// these in the automation parameter menu).
#[derive(Clone, Debug, PartialEq)]
pub struct AuParam {
    pub id: u32,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// A switch or a choice of named values: automation steps between whole numbers.
    pub stepped: bool,
}

impl AuInstance {
    /// Sets parameter `id` (global scope) from frame `offset` of the next render. Audio
    /// thread: `AudioUnitSetParameter` is the render-thread call, and it does not notify
    /// parameter listeners, so automation is never taken for a hand on a knob.
    pub fn set_parameter(&mut self, id: u32, value: f32, offset: u32) {
        if !value.is_finite() {
            return;
        }
        // SAFETY: the unit is open; a parameter it does not have is refused with an error.
        unsafe {
            objc2_audio_toolbox::AudioUnitSetParameter(
                self.unit,
                id,
                kAudioUnitScope_Global,
                0,
                value,
                offset,
            );
        }
    }
}

impl AuHandle {
    /// The writable parameters of the unit's global scope. UI thread.
    pub fn parameters(&self) -> Vec<AuParam> {
        use objc2_audio_toolbox::{
            kAudioUnitProperty_ParameterInfo, kAudioUnitProperty_ParameterList,
            AudioUnitGetPropertyInfo, AudioUnitParameterInfo, AudioUnitParameterOptions,
            AudioUnitParameterUnit,
        };
        let mut out = Vec::new();
        // SAFETY: the sizes passed are those of the buffers; the unit fills at most that.
        unsafe {
            let mut size = 0u32;
            if AudioUnitGetPropertyInfo(
                self.0,
                kAudioUnitProperty_ParameterList,
                kAudioUnitScope_Global,
                0,
                &mut size,
                std::ptr::null_mut(),
            ) != 0
                || size == 0
            {
                return out;
            }
            let n = size as usize / std::mem::size_of::<u32>();
            let mut ids = vec![0u32; n];
            if AudioUnitGetProperty(
                self.0,
                kAudioUnitProperty_ParameterList,
                kAudioUnitScope_Global,
                0,
                NonNull::new_unchecked(ids.as_mut_ptr()).cast(),
                NonNull::from(&mut size),
            ) != 0
            {
                return out;
            }
            ids.truncate(size as usize / std::mem::size_of::<u32>());
            for id in ids {
                let mut info: AudioUnitParameterInfo = std::mem::zeroed();
                let mut info_size = std::mem::size_of::<AudioUnitParameterInfo>() as u32;
                if AudioUnitGetProperty(
                    self.0,
                    kAudioUnitProperty_ParameterInfo,
                    kAudioUnitScope_Global,
                    id,
                    NonNull::from(&mut info).cast(),
                    NonNull::from(&mut info_size),
                ) != 0
                {
                    continue;
                }
                let flags = info.flags;
                let mut name = String::new();
                if flags.contains(AudioUnitParameterOptions::Flag_HasCFNameString)
                    && !info.cfNameString.is_null()
                {
                    name = (*info.cfNameString).to_string();
                    if flags.contains(AudioUnitParameterOptions::Flag_CFNameRelease) {
                        drop(CFRetained::from_raw(NonNull::new_unchecked(
                            info.cfNameString.cast_mut(),
                        )));
                    }
                }
                if name.is_empty() {
                    let bytes: Vec<u8> = info
                        .name
                        .iter()
                        .take_while(|c| **c != 0)
                        .map(|c| *c as u8)
                        .collect();
                    name = String::from_utf8_lossy(&bytes).into_owned();
                }
                let writable = flags.contains(AudioUnitParameterOptions::Flag_IsWritable);
                let meter = flags.contains(AudioUnitParameterOptions::Flag_MeterReadOnly);
                let (min, max) = (info.minValue, info.maxValue);
                if !writable || meter || !min.is_finite() || !max.is_finite() || max <= min {
                    continue;
                }
                let stepped = matches!(
                    info.unit,
                    AudioUnitParameterUnit::Boolean | AudioUnitParameterUnit::Indexed
                );
                out.push(AuParam {
                    id,
                    name: if name.is_empty() {
                        format!("#{id}")
                    } else {
                        name
                    },
                    min,
                    max,
                    default: info.defaultValue.clamp(min, max),
                    stepped,
                });
            }
        }
        out
    }

    /// The value of parameter `id` now. UI thread.
    pub fn parameter(&self, id: u32) -> Option<f32> {
        let mut v = 0f32;
        // SAFETY: the unit is open; it writes one float.
        let s = unsafe {
            objc2_audio_toolbox::AudioUnitGetParameter(
                self.0,
                id,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut v),
            )
        };
        (s == 0 && v.is_finite()).then_some(v)
    }

    /// The names of a stepped parameter's values (`kAudioUnitProperty_ParameterValueStrings`),
    /// from its minimum up; empty when the unit gives none. UI thread.
    pub fn parameter_value_names(&self, id: u32) -> Vec<String> {
        use objc2_audio_toolbox::kAudioUnitProperty_ParameterValueStrings;
        use objc2_core_foundation::CFArray;
        // SAFETY: the property is a retained CFArrayRef of CFStrings that we take over.
        unsafe {
            let mut list: *const c_void = std::ptr::null();
            let mut size = std::mem::size_of::<*const c_void>() as u32;
            let s = AudioUnitGetProperty(
                self.0,
                kAudioUnitProperty_ParameterValueStrings,
                kAudioUnitScope_Global,
                id,
                NonNull::from(&mut list).cast(),
                NonNull::from(&mut size),
            );
            let Some(ptr) = NonNull::new(list.cast_mut()) else {
                return Vec::new();
            };
            if s != 0 {
                return Vec::new();
            }
            let array: CFRetained<CFArray> = CFRetained::from_raw(ptr.cast());
            (0..array.count())
                .filter_map(|k| {
                    array
                        .value_at_index(k)
                        .cast::<CFString>()
                        .as_ref()
                        .map(|s| s.to_string())
                })
                .collect()
        }
    }

    /// The text the unit shows for parameter `id` at `value`
    /// (`kAudioUnitProperty_ParameterStringFromValue`), e.g. "4.0" or "350 ms". UI thread.
    pub fn parameter_text(&self, id: u32, value: f32) -> Option<String> {
        use objc2_audio_toolbox::{
            kAudioUnitProperty_ParameterStringFromValue, AudioUnitParameterStringFromValue,
        };
        let mut q = AudioUnitParameterStringFromValue {
            inParamID: id,
            inValue: NonNull::from(&value),
            outString: std::ptr::null(),
        };
        let mut size = std::mem::size_of::<AudioUnitParameterStringFromValue>() as u32;
        // SAFETY: the unit fills `outString` with a retained string (released here); `value`
        // lives across the call.
        unsafe {
            let s = AudioUnitGetProperty(
                self.0,
                kAudioUnitProperty_ParameterStringFromValue,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut q).cast(),
                NonNull::from(&mut size),
            );
            if s != 0 {
                return None;
            }
            NonNull::new(q.outString.cast_mut())
                .map(|n| CFRetained::<CFString>::from_raw(n).to_string())
        }
    }

    /// Moves parameter `id` to `value` as a hand on its knob does (`AUParameterSet`): the unit
    /// takes it, and its window and the parameter listeners (the automation watch) hear it.
    /// UI thread.
    pub fn edit_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        let param = objc2_audio_toolbox::AudioUnitParameter {
            mAudioUnit: self.0,
            mParameterID: id,
            mScope: kAudioUnitScope_Global,
            mElement: 0,
        };
        // SAFETY: the parameter lives across the call; null sender and object notify every
        // listener.
        unsafe {
            objc2_audio_toolbox::AUParameterSet(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                NonNull::from(&param),
                value,
                0,
            );
        }
    }

    /// Tells the plug-in's own window that parameter `id` changed (automation moved it with
    /// `AudioUnitSetParameter`, which views do not hear), so its knob follows. `sender` is
    /// left out of the notification. Main thread.
    pub fn notify_view(&self, id: u32, sender: Option<&AuParamWatch>) {
        let param = objc2_audio_toolbox::AudioUnitParameter {
            mAudioUnit: self.0,
            mParameterID: id,
            mScope: kAudioUnitScope_Global,
            mElement: 0,
        };
        // SAFETY: the parameter lives across the call; a null object is allowed.
        unsafe {
            objc2_audio_toolbox::AUParameterListenerNotify(
                sender.map_or(std::ptr::null_mut(), |w| w.listener),
                std::ptr::null_mut(),
                NonNull::from(&param),
            );
        }
    }
}

/// Changes the user made to an Audio Unit's parameters in its own window (Ableton: touching a
/// plug-in control selects it in the automation chooser, and records or overrides it).
///
/// Plug-in views set their parameters with `AUParameterSet`, which notifies listeners; the
/// engine's automation uses `AudioUnitSetParameter`, which does not, so what arrives here is
/// the user's hand (and a host notification sent by [`AuHandle::notify_view`] with this
/// watch as the sender is left out). Callbacks come on the main run loop.
pub struct AuParamWatch {
    listener: objc2_audio_toolbox::AUParameterListenerRef,
    unit: AudioUnit,
    ids: Vec<u32>,
    inbox: Box<std::sync::Mutex<Vec<(u32, f32)>>>,
}

/// Most changes kept between two reads of a watch.
const WATCH_ROOM: usize = 4096;

unsafe extern "C-unwind" fn param_changed(
    user: *mut c_void,
    _object: *mut c_void,
    param: NonNull<objc2_audio_toolbox::AudioUnitParameter>,
    value: f32,
) {
    // SAFETY: `user` is the watch's inbox, which outlives the listener (disposed first), and
    // `param` is valid for the call.
    let (inbox, id) = unsafe {
        (
            &*(user as *const std::sync::Mutex<Vec<(u32, f32)>>),
            param.as_ref().mParameterID,
        )
    };
    if let Ok(mut v) = inbox.lock() {
        if v.len() < WATCH_ROOM {
            v.push((id, value));
        }
    }
}

impl AuParamWatch {
    /// Listens to parameters `ids` of the unit behind `h`. Main thread.
    pub fn new(h: AuHandle, ids: &[u32]) -> Option<Self> {
        use objc2_audio_toolbox::{AUListenerAddParameter, AUListenerCreate, AudioUnitParameter};
        use objc2_core_foundation::{kCFRunLoopCommonModes, CFRunLoop};
        let inbox = Box::new(std::sync::Mutex::new(Vec::new()));
        let mut listener: objc2_audio_toolbox::AUParameterListenerRef = std::ptr::null_mut();
        let run_loop = CFRunLoop::main()?;
        // SAFETY: the inbox is boxed (it does not move) and outlives the listener.
        unsafe {
            let user = NonNull::from(&*inbox).cast::<c_void>();
            if AUListenerCreate(
                Some(param_changed),
                user,
                Some(&run_loop),
                kCFRunLoopCommonModes,
                0.02,
                NonNull::from(&mut listener),
            ) != 0
                || listener.is_null()
            {
                return None;
            }
            for &id in ids {
                let p = AudioUnitParameter {
                    mAudioUnit: h.0,
                    mParameterID: id,
                    mScope: kAudioUnitScope_Global,
                    mElement: 0,
                };
                AUListenerAddParameter(listener, std::ptr::null_mut(), NonNull::from(&p));
            }
        }
        Some(Self {
            listener,
            unit: h.0,
            ids: ids.to_vec(),
            inbox,
        })
    }

    /// The changes since the last call, in order: (parameter, value).
    pub fn take(&self) -> Vec<(u32, f32)> {
        self.inbox
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }
}

impl Drop for AuParamWatch {
    fn drop(&mut self) {
        use objc2_audio_toolbox::{
            AUListenerDispose, AUListenerRemoveParameter, AudioUnitParameter,
        };
        // SAFETY: the listener was made by AUListenerCreate; after disposing it no callback
        // reaches the inbox, which is freed after this.
        unsafe {
            for &id in &self.ids {
                let p = AudioUnitParameter {
                    mAudioUnit: self.unit,
                    mParameterID: id,
                    mScope: kAudioUnitScope_Global,
                    mElement: 0,
                };
                AUListenerRemoveParameter(self.listener, std::ptr::null_mut(), NonNull::from(&p));
            }
            AUListenerDispose(self.listener);
        }
    }
}
