//! `prowave --dsp-load [seconds] [buffer]`: what the CPU meter reads. Starts the real engine,
//! reads the peak DSP load every 20 ms (as the meter does each frame) and prints how often the
//! peak passes 50%, 90% and 100% (an overload: the block took longer than its buffer), with
//! the overloads Core Audio and the engine counted. Nothing is played aloud: the master is
//! muted.
//!
//! - `--vst3 <bundle> [count]`: that many instances of the bundle's first class as Audio FX on
//!   a track, processing silence.
//! - `--au <type:subtype:manufacturer> <N>` (macOS): N instances of that Audio Unit, one per
//!   playback track, plus one more on a live track; every track plays a long note of the
//!   built-in synth into its effect, so the units process real sound.
//! - `--process <frames>`: Record > Process Buffer for the playback tracks (default 512; 0 runs
//!   every plug-in in the I/O callback, as before the Process Buffer Range).
//! - `--sweep <type:subtype:manufacturer>` (macOS): one engine start, then every combination
//!   of (I/O buffer, process buffer) in 64/off, 64/Small, 64/Medium, 1024/Large (MIX) with
//!   1, 2 and 4 instances of that Audio Unit on playback tracks plus one on a live track,
//!   `seconds` each. The I/O buffer changes while running, as the RECORD / MIX switch does.
pub fn run(args: &[String]) -> i32 {
    use std::time::Duration;

    // `--live-empty`: the live track plays its synth without the Audio Unit (only the
    // playback tracks carry it).
    let (args, live_empty) = take_flag(args, "--live-empty", 0);
    let live_unit = live_empty.is_none();
    let (args, sweep) = take_flag(&args, "--sweep", 1);
    if let Some(id) = sweep.and_then(|v| v.first().cloned()) {
        let secs: f64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(10.0);
        return run_sweep(&id, secs, live_unit);
    }
    let (args, vst3) = take_flag(&args, "--vst3", 2);
    let (args, au) = take_flag(&args, "--au", 2);
    let (args, process) = take_flag(&args, "--process", 1);
    let process: u32 = process
        .and_then(|v| v.first().and_then(|a| a.parse().ok()))
        .unwrap_or(prowave_engine::PROCESS_BUFFER_MEDIUM);
    let secs: f64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(10.0);
    let buffer: Option<u32> = args.get(1).and_then(|a| a.parse().ok());
    #[allow(unused_mut)]
    let mut engine = match prowave_engine::Engine::start_with(buffer) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("audio: {e}");
            return 1;
        }
    };
    // Never aloud.
    engine.send(prowave_engine::Command::SetMasterGain { linear: 0.0 });
    engine.send(prowave_engine::Command::SetProcessBuffer { frames: process });
    #[cfg(target_os = "macos")]
    if let Some(v) = vst3 {
        use prowave_engine::vst3::{module, Vst3Instance};
        let path = v.first().cloned().unwrap_or_default();
        let n: usize = v.get(1).and_then(|a| a.parse().ok()).unwrap_or(1);
        let p = std::path::Path::new(&path);
        let Some(class) = module(p).ok().and_then(|m| m.classes().into_iter().next()) else {
            eprintln!("{path}: no VST3 class");
            return 1;
        };
        let mut fx: Vec<Box<dyn prowave_engine::BlockProcessor>> = Vec::new();
        for _ in 0..n {
            match Vst3Instance::new(p, &class.cid, engine.sample_rate(), 8192) {
                Ok(i) => fx.push(Box::new(i)),
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            }
        }
        engine.send(prowave_engine::Command::SetPlugins {
            track: 0,
            plugins: Some(Box::new(prowave_engine::PluginTrack::new(None, fx, 8192))),
        });
        println!("{} x {}: {}", n, class.vendor, class.name);
    }
    #[cfg(target_os = "macos")]
    if let Some(v) = au {
        let id = v.first().cloned().unwrap_or_default();
        let n: usize = v.get(1).and_then(|a| a.parse().ok()).unwrap_or(1);
        if let Err(e) = load_au(&mut engine, &id, n, live_unit) {
            eprintln!("{e}");
            return 1;
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (vst3, au);
    std::thread::sleep(Duration::from_millis(500));
    let shared = engine.shared();
    shared.take_dsp_load();
    shared.take_worker_load();
    shared.take_heaviest_plugin();
    let start = (
        shared
            .hal_overloads
            .load(std::sync::atomic::Ordering::Relaxed),
        shared
            .late_callbacks
            .load(std::sync::atomic::Ordering::Relaxed),
        shared.overloads(),
        shared
            .prerender_underruns
            .load(std::sync::atomic::Ordering::Relaxed),
    );
    let n = (secs * 50.0) as usize;
    let mut live = Vec::with_capacity(n);
    let mut playback = Vec::with_capacity(n);
    let mut heaviest: Option<(u16, u8, f32)> = None;
    for _ in 0..n {
        std::thread::sleep(Duration::from_millis(20));
        live.push(shared.take_dsp_load());
        playback.push(shared.take_worker_load());
        if let Some(h) = shared.take_heaviest_plugin() {
            if heaviest.is_none_or(|b| h.2 > b.2) {
                heaviest = Some(h);
            }
        }
    }
    let ord = std::sync::atomic::Ordering::Relaxed;
    let end = (
        shared.hal_overloads.load(ord),
        shared.late_callbacks.load(ord),
        shared.overloads(),
        shared.prerender_underruns.load(ord),
    );
    println!(
        "I/O buffer {} frames, process buffer {} frames, {} readings of 20 ms",
        engine.buffer_frames(),
        process,
        live.len()
    );
    println!("live:     {}", summary(&live));
    println!("playback: {}", summary(&playback));
    println!(
        "overloads: Core Audio {}, engine (late or long callbacks) {}, shown {}; playback underruns {}",
        end.0 - start.0,
        end.1 - start.1,
        end.2 - start.2,
        end.3 - start.3,
    );
    if let Some((track, slot, load)) = heaviest {
        println!(
            "heaviest plug-in: track {track} slot {slot}, {:.1}% of its block",
            load * 100.0
        );
    }
    0
}

/// The overload counters: Core Audio's, the engine's own, what the meter shows, and the
/// playback thread's underruns.
fn counters(shared: &prowave_engine::Shared) -> [u32; 4] {
    let ord = std::sync::atomic::Ordering::Relaxed;
    [
        shared.hal_overloads.load(ord),
        shared.late_callbacks.load(ord),
        shared.overloads(),
        shared.prerender_underruns.load(ord),
    ]
}

/// `--sweep`: every configuration in one engine (see the module documentation).
#[cfg(target_os = "macos")]
fn run_sweep(id: &str, secs: f64, live_unit: bool) -> i32 {
    use std::time::Duration;
    let mut engine = match prowave_engine::Engine::start_with(Some(64)) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("audio: {e}");
            return 1;
        }
    };
    send(
        &mut engine,
        prowave_engine::Command::SetMasterGain { linear: 0.0 },
    );
    println!("{} at {} Hz", engine.device_name(), engine.sample_rate());
    let configs: [(&str, u32, u32); 4] = [
        ("before (all in the I/O callback)", 64, 0),
        (
            "RECORD (process Small)",
            64,
            prowave_engine::PROCESS_BUFFER_SMALL,
        ),
        ("process Medium", 64, prowave_engine::PROCESS_BUFFER_MEDIUM),
        (
            "MIX (process Large)",
            1024,
            prowave_engine::PROCESS_BUFFER_LARGE,
        ),
    ];
    let mut loaded = 0;
    for (name, io, process) in configs {
        let got = engine.set_buffer_frames(io).unwrap_or(0);
        send(
            &mut engine,
            prowave_engine::Command::SetProcessBuffer { frames: process },
        );
        for n in [1usize, 2, 4] {
            for t in n + 1..loaded {
                send(
                    &mut engine,
                    prowave_engine::Command::SetPlugins {
                        track: t as u16,
                        plugins: None,
                    },
                );
            }
            if let Err(e) = load_au(&mut engine, id, n, live_unit) {
                eprintln!("{e}");
                return 1;
            }
            loaded = n + 1;
            // The moves to the playback thread and the new buffer settle.
            std::thread::sleep(Duration::from_millis(1500));
            let shared = engine.shared();
            shared.take_dsp_load();
            shared.take_worker_load();
            shared.take_heaviest_plugin();
            let start = counters(shared);
            let readings = (secs * 50.0) as usize;
            let (mut live, mut playback) = (Vec::new(), Vec::new());
            let mut heaviest = 0f32;
            for _ in 0..readings {
                std::thread::sleep(Duration::from_millis(20));
                live.push(shared.take_dsp_load());
                playback.push(shared.take_worker_load());
                if let Some(h) = shared.take_heaviest_plugin() {
                    heaviest = heaviest.max(h.2);
                }
            }
            let end = counters(shared);
            println!("{name}: I/O {got}, process {process}, {n} on playback + 1 live, {secs} s");
            println!("  live:     {}", summary(&live));
            println!("  playback: {}", summary(&playback));
            println!(
                "  overloads: Core Audio {}, engine {}, shown {}; playback underruns {}; heaviest plug-in {:.1}% of its block",
                end[0] - start[0],
                end[1] - start[1],
                end[2] - start[2],
                end[3] - start[3],
                heaviest * 100.0
            );
        }
    }
    0
}

#[cfg(not(target_os = "macos"))]
fn run_sweep(_id: &str, _secs: f64, _live_unit: bool) -> i32 {
    eprintln!("the sweep loads Audio Units: macOS only");
    1
}

/// Sends `cmd`; says so when the engine refused it (its queue was full).
#[cfg(target_os = "macos")]
fn send(engine: &mut prowave_engine::Engine, cmd: prowave_engine::Command) {
    if !engine.send(cmd) {
        eprintln!("the engine's queue was full: a command was dropped");
    }
}

/// `args` without `flag` and the `values` after it, and those values.
fn take_flag(args: &[String], flag: &str, values: usize) -> (Vec<String>, Option<Vec<String>>) {
    match args.iter().position(|a| a == flag) {
        Some(i) => {
            let end = (i + 1 + values).min(args.len());
            let mut rest = args[..i].to_vec();
            rest.extend_from_slice(&args[end..]);
            (rest, Some(args[i + 1..end].to_vec()))
        }
        None => (args.to_vec(), None),
    }
}

/// Median, 99th percentile and highest of `peaks`, and how many pass 50%, 90% and 100%.
fn summary(peaks: &[f32]) -> String {
    if peaks.is_empty() {
        return "no readings".into();
    }
    let mut sorted = peaks.to_vec();
    sorted.sort_by(f32::total_cmp);
    let pct = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize] * 100.0;
    let over = |x: f32| peaks.iter().filter(|p| **p > x).count();
    format!(
        "median peak {:.1}%, 99th {:.1}%, max {:.1}%; readings over 50% {}, over 90% {}, over 100% {}",
        pct(0.5),
        pct(0.99),
        pct(1.0),
        over(0.5),
        over(0.9),
        over(1.0)
    )
}

/// `n` instances of the Audio Unit `id` on tracks 0..n (pre-rendered) and, with `live_unit`,
/// one more on track `n` (live), each track playing a long synth note into its unit.
#[cfg(target_os = "macos")]
fn load_au(
    engine: &mut prowave_engine::Engine,
    id: &str,
    n: usize,
    live_unit: bool,
) -> Result<(), String> {
    use prowave_engine::au::{find, AuInstance};
    use prowave_engine::{Command, NoteEvent, PluginTrack, Song, TrackMix};
    let info = find(id).ok_or_else(|| format!("{id}: not found"))?;
    let tracks = n + 1;
    let notes = (0..tracks)
        .map(|t| NoteEvent {
            start_beat: 0.0,
            end_beat: 1.0e6,
            pitch: 45 + (t % 24) as u8,
            vel: 100,
            track: t as u16,
        })
        .collect();
    engine.send(Command::SetSong(Box::new(Song::new(
        notes,
        vec![TrackMix::default(); tracks],
    ))));
    for t in 0..tracks {
        engine.send(Command::SetTrackLive {
            track: t as u16,
            live: t == n,
        });
        let plugins = if t < n || live_unit {
            let unit = AuInstance::new(&info, engine.sample_rate(), 8192)?;
            let fx: Vec<Box<dyn prowave_engine::BlockProcessor>> = vec![Box::new(unit)];
            Some(Box::new(PluginTrack::new(None, fx, 8192)))
        } else {
            None
        };
        engine.send(Command::SetPlugins {
            track: t as u16,
            plugins,
        });
    }
    engine.send(Command::Play);
    let live = if live_unit { "with" } else { "without" };
    println!("{n} x {id} on playback tracks + a live track {live} one");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_and_their_values_come_out_of_the_arguments() {
        let args: Vec<String> = ["10", "--au", "aufx:cl3G:Taps", "4", "--process", "0", "64"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (rest, au) = take_flag(&args, "--au", 2);
        assert_eq!(au.unwrap(), ["aufx:cl3G:Taps", "4"]);
        let (rest, process) = take_flag(&rest, "--process", 1);
        assert_eq!(process.unwrap(), ["0"]);
        assert_eq!(rest, ["10", "64"]);
        assert_eq!(take_flag(&rest, "--vst3", 2).1, None);
    }
}
