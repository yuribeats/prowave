//! Opens the macOS duplex stream and reports what it got.
//!
//! cargo run --release -p prowave-engine --example duplex_probe -- [--frames N] [--click]
//!     [--out UID] [--in UID] [--then-out UID --then-in UID] [--then-frames N] [--list]
//!
//! Without `--click` it is silent: it prints the buffer, the reported latencies and the input
//! level for two seconds. With `--click` it plays a short click every half second and finds
//! it again in the input, which measures the real round trip (output → cable or air → input).
#[cfg(target_os = "macos")]
fn main() {
    use prowave_engine::duplex::{devices, Block, Devices, DuplexStream};
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    let args: Vec<String> = std::env::args().collect();
    let frames = args
        .iter()
        .position(|a| a == "--frames")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok());
    let click = args.iter().any(|a| a == "--click");
    let named = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if args.iter().any(|a| a == "--list") {
        for d in devices() {
            println!(
                "{:<40} in {:>2} out {:>2}{}{}  {}",
                d.name,
                d.inputs,
                d.outputs,
                if d.default_input { " [default in]" } else { "" },
                if d.default_output {
                    " [default out]"
                } else {
                    ""
                },
                d.uid
            );
        }
        return;
    }
    let wanted = Devices {
        output: named("--out"),
        input: named("--in"),
        with_input: true,
    };

    let callbacks = Arc::new(AtomicU64::new(0));
    let peak = Arc::new(AtomicU32::new(0));
    let trips = Arc::new(AtomicU64::new(0));
    let trip_sum = Arc::new(AtomicU64::new(0));
    let trip_min = Arc::new(AtomicU64::new(u64::MAX));
    let lat = Arc::new(AtomicU64::new(0));
    let (cb, pk, tr, ts, tm, lt) = (
        callbacks.clone(),
        peak.clone(),
        trips.clone(),
        trip_sum.clone(),
        trip_min.clone(),
        lat.clone(),
    );
    let started = DuplexStream::start(frames, &wanted, move |info| {
        let rate = u64::from(info.sample_rate);
        let mut t: u64 = 0;
        let mut sent: Option<u64> = None;
        let mut next = rate / 2;
        move |b: Block<'_>| {
            let n = b.output.len() / b.out_channels;
            cb.fetch_add(1, Ordering::Relaxed);
            lt.store(
                (u64::from(b.input_latency_us) << 32) | u64::from(b.output_latency_us),
                Ordering::Relaxed,
            );
            for f in 0..n {
                let now = t + f as u64;
                let s = if b.in_channels > 0 {
                    b.input[f * b.in_channels].abs()
                } else {
                    0.0
                };
                pk.fetch_max(s.to_bits(), Ordering::Relaxed);
                if let Some(at) = sent {
                    if s > 0.2 && now > at {
                        let d = now - at;
                        tr.fetch_add(1, Ordering::Relaxed);
                        ts.fetch_add(d, Ordering::Relaxed);
                        tm.fetch_min(d, Ordering::Relaxed);
                        sent = None;
                    } else if now > at + rate / 4 {
                        sent = None;
                    }
                }
                if click && now >= next && sent.is_none() {
                    for c in 0..b.out_channels.min(2) {
                        b.output[f * b.out_channels + c] = 0.9;
                    }
                    sent = Some(now);
                    next = now + rate / 2;
                }
            }
            t += n as u64;
        }
    });
    let (mut stream, info) = match started {
        Ok(s) => s,
        Err(e) => {
            eprintln!("duplex failed: {e}");
            std::process::exit(1);
        }
    };
    let rate = f64::from(info.sample_rate);
    let ms = |f: f64| f * 1000.0 / rate;
    println!("device       {}", info.device_name);
    println!("aggregate    {}", info.aggregate);
    println!("rate         {} Hz", info.sample_rate);
    println!(
        "buffer       {} frames ({:.2} ms)",
        info.buffer_frames,
        ms(f64::from(info.buffer_frames))
    );
    println!(
        "channels     in {} / out {}",
        info.in_channels, info.out_channels
    );
    println!(
        "device extra in {} frames ({:.2} ms) / out {} frames ({:.2} ms)",
        info.input_latency_frames,
        ms(f64::from(info.input_latency_frames)),
        info.output_latency_frames,
        ms(f64::from(info.output_latency_frames))
    );
    let secs = if click { 6 } else { 2 };
    let then = Devices {
        output: named("--then-out"),
        input: named("--then-in"),
        with_input: true,
    };
    let then_frames: Option<u32> = named("--then-frames").and_then(|v| v.parse().ok());
    if let Some(n) = then_frames {
        std::thread::sleep(Duration::from_secs(secs / 2));
        let before = callbacks.load(Ordering::Relaxed);
        let got = stream.set_buffer_frames(n);
        std::thread::sleep(Duration::from_secs(secs - secs / 2));
        let after = callbacks.load(Ordering::Relaxed) - before;
        println!(
            "buffer now   {got} frames: {after} callbacks in {} s (expected about {:.0})",
            secs - secs / 2,
            (secs - secs / 2) as f64 * rate / f64::from(got.max(1))
        );
    } else if then.output.is_some() || then.input.is_some() {
        // Half the time on the first devices, half on the others.
        std::thread::sleep(Duration::from_secs(secs / 2));
        let before = callbacks.load(Ordering::Relaxed);
        match stream.switch(&then) {
            Ok(i) => println!(
                "switched to  {} ({} frames, in {} / out {}) after {before} callbacks",
                i.device_name, i.buffer_frames, i.in_channels, i.out_channels
            ),
            Err(e) => println!("switch failed: {e}"),
        }
        std::thread::sleep(Duration::from_secs(secs - secs / 2));
    } else {
        std::thread::sleep(Duration::from_secs(secs));
    }
    let l = lat.load(Ordering::Relaxed);
    println!(
        "callback     in {:.2} ms / out {:.2} ms (from the HAL's time stamps + device extra)",
        (l >> 32) as f64 / 1000.0,
        (l & 0xffff_ffff) as f64 / 1000.0
    );
    let calls = callbacks.load(Ordering::Relaxed);
    let expected = secs as f64 * rate / f64::from(info.buffer_frames.max(1));
    println!("callbacks    {calls} in {secs} s (expected about {expected:.0})");
    println!(
        "input peak   {:.4}",
        f32::from_bits(peak.load(Ordering::Relaxed))
    );
    let n = trips.load(Ordering::Relaxed);
    if click {
        if n == 0 {
            println!("round trip   no click came back (input level too low or muted)");
        } else {
            let avg = trip_sum.load(Ordering::Relaxed) as f64 / n as f64;
            let min = trip_min.load(Ordering::Relaxed) as f64;
            println!(
                "round trip   {n} clicks, min {min} frames ({:.2} ms), mean {avg:.0} frames ({:.2} ms)",
                ms(min),
                ms(avg)
            );
        }
    }
    drop(stream);
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("duplex_probe is macOS only");
}
