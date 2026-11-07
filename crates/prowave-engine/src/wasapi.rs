//! Windows low-latency output: WASAPI shared mode through `IAudioClient3`.
//!
//! cpal opens shared streams with the audio engine's default period (10 ms, ~32 ms reported
//! output latency). `IAudioClient3` lets a shared-mode client ask for the smallest period the
//! audio engine supports (often 128-480 frames, 2.7-10 ms) without taking the device away from
//! other programs. If anything here is not supported the caller falls back to cpal.
use crate::core::Readers;
use crate::output::Renderer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient3, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    WaitForSingleObject,
};

/// What the stream ended up with.
pub(crate) struct StreamInfo {
    pub device_name: String,
    pub sample_rate: u32,
    pub period_frames: u32,
}

/// The running stream. Dropping it stops the thread and the sound.
pub(crate) struct LowLatencyStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for LowLatencyStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What starting a stream gives back: the stream, what it ended up with, and the engine's
/// readers.
type Started = (LowLatencyStream, StreamInfo, Readers);

type Made = (Renderer, Readers);

impl LowLatencyStream {
    /// Opens the default output with a period of about `buffer_frames` (the smallest the
    /// audio engine allows when `None`) and calls `make(sample_rate, channels)` on the audio
    /// thread's behalf to build what renders into it.
    pub(crate) fn start<F>(buffer_frames: Option<u32>, make: F) -> Result<Started, String>
    where
        F: FnOnce(u32, usize) -> Made + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let stop_thread = stop.clone();
        let thread = std::thread::Builder::new()
            .name("prowave-audio".into())
            .spawn(move || {
                // SAFETY: COM is initialised for this thread and released when it ends.
                let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let inited = com.is_ok();
                run(buffer_frames, make, &stop_thread, &ready_tx);
                if inited {
                    unsafe { CoUninitialize() };
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok((info, readers))) => Ok((
                Self {
                    stop,
                    thread: Some(thread),
                },
                info,
                readers,
            )),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                Err("the audio device did not start in time".into())
            }
        }
    }
}

type Ready = Result<(StreamInfo, Readers), String>;

fn run<F>(buffer_frames: Option<u32>, make: F, stop: &AtomicBool, ready: &mpsc::Sender<Ready>)
where
    F: FnOnce(u32, usize) -> Made,
{
    match open(buffer_frames) {
        Err(e) => {
            let _ = ready.send(Err(e));
        }
        Ok(opened) => {
            let (renderer, readers) = make(opened.sample_rate, opened.channels);
            let info = StreamInfo {
                device_name: "Default output (WASAPI low latency)".into(),
                sample_rate: opened.sample_rate,
                period_frames: opened.period_frames,
            };
            let _ = ready.send(Ok((info, readers)));
            render_loop(opened, renderer, stop);
        }
    }
}

struct Opened {
    client: IAudioClient3,
    render: IAudioRenderClient,
    event: HANDLE,
    buffer_frames: u32,
    period_frames: u32,
    channels: usize,
    sample_rate: u32,
}

fn is_float32(format: &WAVEFORMATEX) -> bool {
    // The struct is packed: copy the fields out before using them.
    let (tag, bits) = ({ format.wFormatTag } as u32, { format.wBitsPerSample });
    if bits != 32 {
        return false;
    }
    if tag == WAVE_FORMAT_IEEE_FLOAT {
        return true;
    }
    if tag == WAVE_FORMAT_EXTENSIBLE {
        // SAFETY: an extensible format is a WAVEFORMATEXTENSIBLE.
        let ext = unsafe {
            std::ptr::read_unaligned((format as *const WAVEFORMATEX).cast::<WAVEFORMATEXTENSIBLE>())
        };
        let sub = { ext.SubFormat };
        return sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    }
    false
}

/// Rounds `wanted` frames to what the audio engine accepts: a multiple of `step`, within
/// `min..=max`.
fn pick_period(wanted: Option<u32>, step: u32, min: u32, max: u32) -> u32 {
    let step = step.max(1);
    let Some(wanted) = wanted else {
        return min;
    };
    let n = wanted.div_ceil(step) * step;
    n.clamp(min, max.max(min))
}

fn open(buffer_frames: Option<u32>) -> Result<Opened, String> {
    // SAFETY: plain COM calls on interfaces created here; every pointer comes from WASAPI.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| e.to_string())?;
        let client: IAudioClient3 = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("no IAudioClient3: {e}"))?;
        let format_ptr = client.GetMixFormat().map_err(|e| e.to_string())?;
        let result = open_with(&client, format_ptr, buffer_frames);
        windows::Win32::System::Com::CoTaskMemFree(Some(format_ptr.cast()));
        result
    }
}

unsafe fn open_with(
    client: &IAudioClient3,
    format_ptr: *mut WAVEFORMATEX,
    buffer_frames: Option<u32>,
) -> Result<Opened, String> {
    let format = &*format_ptr;
    if !is_float32(format) {
        return Err("the mix format is not 32-bit float".into());
    }
    let channels = { format.nChannels } as usize;
    let sample_rate = { format.nSamplesPerSec };
    let (mut default, mut step, mut min, mut max) = (0u32, 0u32, 0u32, 0u32);
    client
        .GetSharedModeEnginePeriod(format_ptr, &mut default, &mut step, &mut min, &mut max)
        .map_err(|e| format!("no shared-mode period: {e}"))?;
    let period = pick_period(buffer_frames, step, min, max);
    tracing::info!("WASAPI shared periods: default {default}, step {step}, min {min}, max {max}; using {period}");
    client
        .InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, period, format_ptr, None)
        .map_err(|e| format!("InitializeSharedAudioStream({period}): {e}"))?;
    let event = CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| e.to_string())?;
    client.SetEventHandle(event).map_err(|e| e.to_string())?;
    let buffer_frames = client.GetBufferSize().map_err(|e| e.to_string())?;
    let render: IAudioRenderClient = client.GetService().map_err(|e| e.to_string())?;
    Ok(Opened {
        client: client.clone(),
        render,
        event,
        buffer_frames,
        period_frames: period,
        channels,
        sample_rate,
    })
}

fn render_loop(o: Opened, mut renderer: Renderer, stop: &AtomicBool) {
    // SAFETY: the client, render client and event belong to this thread until it returns.
    unsafe {
        // "Pro Audio" asks the multimedia scheduler for a real-time-ish priority.
        let mut task_index = 0u32;
        let mmcss =
            AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut task_index).ok();
        // Fill the whole buffer with silence before starting so the first period is never late.
        if let Ok(p) = o.render.GetBuffer(o.buffer_frames) {
            std::ptr::write_bytes(p, 0, o.buffer_frames as usize * o.channels * 4);
            let _ = o.render.ReleaseBuffer(o.buffer_frames, 0);
        }
        if o.client.Start().is_ok() {
            let stream_latency_us = o.client.GetStreamLatency().unwrap_or(0).max(0) as u64 / 10;
            while !stop.load(Ordering::Acquire) {
                if WaitForSingleObject(o.event, 100) != WAIT_OBJECT_0 {
                    continue;
                }
                let Ok(padding) = o.client.GetCurrentPadding() else {
                    break;
                };
                let frames = o.buffer_frames.saturating_sub(padding);
                if frames == 0 {
                    continue;
                }
                let Ok(p) = o.render.GetBuffer(frames) else {
                    break;
                };
                let data =
                    std::slice::from_raw_parts_mut(p.cast::<f32>(), frames as usize * o.channels);
                // What is already queued plays first, then this block.
                let queued_us = padding as u64 * 1_000_000 / o.sample_rate.max(1) as u64;
                let lat = (stream_latency_us + queued_us).min(u32::MAX as u64) as u32;
                renderer.fill(data, lat);
                if o.render.ReleaseBuffer(frames, 0).is_err() {
                    break;
                }
            }
            let _ = o.client.Stop();
        }
        if let Some(h) = mmcss {
            let _ = AvRevertMmThreadCharacteristics(h);
        }
        let _ = CloseHandle(o.event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_smallest_period_is_used_unless_a_size_is_asked_for() {
        assert_eq!(pick_period(None, 16, 48, 480), 48);
        assert_eq!(pick_period(Some(128), 16, 48, 480), 128);
        assert_eq!(
            pick_period(Some(130), 16, 48, 480),
            144,
            "rounded up to the step"
        );
        assert_eq!(
            pick_period(Some(16), 16, 48, 480),
            48,
            "not below the minimum"
        );
        assert_eq!(
            pick_period(Some(4096), 16, 48, 480),
            480,
            "not above the maximum"
        );
        assert_eq!(
            pick_period(Some(100), 0, 48, 480),
            100,
            "a zero step is harmless"
        );
    }
}
