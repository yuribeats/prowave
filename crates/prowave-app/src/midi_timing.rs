//! `prowave --midi-timing [seconds] [buffer]`: the MIDI timing check (STUDIO-NETWORK-SPEC
//! item 9). The real engine and MIDI sender send clock at 120 bpm (one tick every 20.833 ms)
//! to a virtual MIDI port in this process, which notes when each tick arrives. Prints the
//! spread of the intervals against the ideal; the pass mark is a standard deviation of at most
//! 0.05 ms and a worst case of at most 0.25 ms. Nothing is played aloud (the song is empty).
#[cfg(target_os = "macos")]
pub fn run(args: &[String]) -> i32 {
    use prowave_engine::{Command, Engine};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let secs: f64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(10.0);
    let buffer: Option<u32> = args.get(1).and_then(|a| a.parse().ok());
    let name = format!("Pro Wave timing {}", std::process::id());
    let arrivals: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let Ok(client) = coremidi::Client::new("Pro Wave timing") else {
        eprintln!("no CoreMIDI client");
        return 1;
    };
    let seen = arrivals.clone();
    let Ok(_dest) = client.virtual_destination(&name, move |packets: &coremidi::PacketList| {
        let now = prowave_engine::duplex::host_now_ns();
        for p in packets.iter() {
            if p.data().first() == Some(&0xF8) {
                if let Ok(mut v) = seen.lock() {
                    v.push(now);
                }
            }
        }
    }) else {
        eprintln!("no virtual destination");
        return 1;
    };
    // The destination has to be seen by the sender's port list.
    std::thread::sleep(Duration::from_millis(300));
    let mut engine = match Engine::start_with(buffer) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("audio: {e}");
            return 1;
        }
    };
    let (Some(events), Some(mtc)) = (engine.take_midi_out(), engine.take_mtc_out()) else {
        return 1;
    };
    crate::midi_out::set_clock_port(&name, true);
    let _out = crate::midi_out::MidiOut::start(crate::midi_out::Queues { notes: events, mtc });
    let map = prowave_engine::TempoMap::constant(120.0, (4, 4));
    engine.send(Command::SetTempoMap(Box::new(map)));
    engine.send(Command::SetMidiClock(true));
    engine.send(Command::Play);
    std::thread::sleep(Duration::from_secs_f64(secs));
    engine.send(Command::Pause);
    std::thread::sleep(Duration::from_millis(200));
    crate::midi_out::set_clock_port(&name, false);

    let t = arrivals.lock().map(|v| v.clone()).unwrap_or_default();
    if t.len() < 10 {
        println!("only {} ticks arrived", t.len());
        return 1;
    }
    // Skip the first ticks (start-up), then compare every interval with the ideal one.
    let ideal = 60.0 / 120.0 / 24.0 * 1e3;
    let iv: Vec<f64> = t[4..]
        .windows(2)
        .map(|w| (w[1] as f64 - w[0] as f64) / 1e6 - ideal)
        .collect();
    let n = iv.len() as f64;
    let mean = iv.iter().sum::<f64>() / n;
    let sd = (iv.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n).sqrt();
    let max = iv.iter().fold(0f64, |m, d| m.max(d.abs()));
    println!(
        "{} ticks at 120 bpm, buffer {} frames: interval error sd {sd:.4} ms, max {max:.4} ms, mean {mean:+.4} ms -> {}",
        t.len(),
        engine.buffer_frames(),
        if sd <= 0.05 && max <= 0.25 { "PASS" } else { "FAIL" }
    );
    0
}

#[cfg(not(target_os = "macos"))]
pub fn run(_args: &[String]) -> i32 {
    eprintln!("the MIDI timing check runs on macOS");
    1
}
