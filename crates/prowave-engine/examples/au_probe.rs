//! Opens Audio Units and renders through them, silently (nothing is played).
//!
//! cargo run --release -p prowave-engine --example au_probe -- --list
//! cargo run --release -p prowave-engine --example au_probe -- --test 'aufx:DEC :SToy'
//! cargo run --release -p prowave-engine --example au_probe -- --all [--frames N]
//! cargo run --release -p prowave-engine --example au_probe -- --presets 'aumu:Dexd:DGSB'
//! cargo run --release -p prowave-engine --example au_probe -- --all-presets [--timeout S]
//! cargo run --release -p prowave-engine --example au_probe -- --apply ID \
//!     [--preset NAME | --state-file FILE] [--out FILE]
//!
//! `--apply` loads a factory preset or saved settings, checks the unit took them, renders one
//! second silently (a chord for instruments) and can write the settings it ends with.
//!
//! `--skip-file FILE` (ids, one per line) keeps `--all` and `--all-presets` from opening
//! those units at all. `--all` tests every unit in its own process, so one that crashes or
//! hangs (a license dialog) cannot stop the rest.
#[cfg(target_os = "macos")]
fn main() {
    use prowave_engine::au::{find, list, AuInstance, AuKind};
    use std::time::{Duration, Instant};

    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let frames: usize = value("--frames").and_then(|v| v.parse().ok()).unwrap_or(64);
    // `--skip-file FILE`: unit ids (one per line) that --all and --all-presets never open,
    // e.g. copy-protected units whose protection shows a dialog in an unsigned program.
    let skip: Vec<String> = value("--skip-file")
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|t| t.lines().map(|l| l.trim_end().to_string()).collect())
        .unwrap_or_default();
    let skipped = |id: &str| skip.iter().any(|s| s == id);

    if args.iter().any(|a| a == "--list") {
        for i in list() {
            println!("{:<16} {:?}  {}", i.id, i.kind, i.label());
        }
        return;
    }

    if let Some(id) = value("--test") {
        let Some(info) = find(&id) else {
            println!("{id}\tnot installed");
            return;
        };
        let t = Instant::now();
        let mut au = match AuInstance::new(&info, 48_000, 4096) {
            Ok(a) => a,
            Err(e) => {
                println!("{}\tFAILED to open: {e}", info.label());
                return;
            }
        };
        let open_ms = t.elapsed().as_millis();
        let input: Vec<f32> = (0..frames)
            .map(|i| 0.25 * (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect();
        let (mut l, mut r) = (vec![0f32; frames], vec![0f32; frames]);
        let blocks = 48_000 / frames; // one second
        let (mut peak, mut worst) = (0f32, Duration::ZERO);
        let mut total = Duration::ZERO;
        for b in 0..blocks {
            // The transport plays at 117.02 BPM in 4/4: what the unit's host callbacks answer.
            let beat = (b * frames) as f64 / 48_000.0 * 117.02 / 60.0;
            au.set_transport(&prowave_engine::BlockTransport {
                playing: true,
                recording: false,
                sample_pos: (b * frames) as i64,
                beat,
                bar_start_beat: (beat / 4.0).floor() * 4.0,
                tempo: 117.02,
                sig: (4, 4),
                cycle: None,
            });
            if info.kind != AuKind::Effect {
                if b == 1 {
                    au.midi(0x90, 60, 100, 0);
                }
                if b == blocks / 2 {
                    au.midi(0x80, 60, 0, 0);
                }
            }
            let t = Instant::now();
            au.process((&input, &input), (&mut l, &mut r));
            let d = t.elapsed();
            total += d;
            worst = worst.max(d);
            peak = l.iter().chain(&r).fold(peak, |m, x| m.max(x.abs()));
        }
        let budget = Duration::from_secs_f64(frames as f64 / 48_000.0);
        println!(
            "{}\topen {open_ms} ms\tpeak {peak:.3}\tlatency {:.2} ms\trender avg {:.0} us, max {:.0} us (budget {:.0} us)\terrors {}\thost calls {}",
            info.label(),
            au.latency_seconds() * 1000.0,
            total.as_secs_f64() * 1e6 / blocks as f64,
            worst.as_secs_f64() * 1e6,
            budget.as_secs_f64() * 1e6,
            au.errors(),
            au.host_callback_calls()
        );
        return;
    }

    // Factory presets of one unit: "number<TAB>name" per line.
    if let Some(id) = value("--presets") {
        let Some(info) = find(&id) else {
            println!("{id}\tnot installed");
            return;
        };
        let au = match AuInstance::new(&info, 48_000, 4096) {
            Ok(a) => a,
            Err(e) => {
                println!("{}\tFAILED to open: {e}", info.label());
                return;
            }
        };
        for (n, name) in au.handle().factory_presets() {
            println!("{n}\t{name}");
        }
        return;
    }

    // Applies a factory preset (`--preset NAME`) or saved settings (`--state-file FILE`: an
    // .aupreset or a project's class info), proves it took, renders a second silently and
    // writes the resulting settings to `--out FILE`.
    if let Some(id) = value("--apply") {
        let Some(info) = find(&id) else {
            println!("{id}\tMISSING\tnot installed");
            return;
        };
        let mut au = match AuInstance::new(&info, 48_000, 4096) {
            Ok(a) => a,
            Err(e) => {
                println!("{id}\tFAILED\t{e}");
                return;
            }
        };
        let h = au.handle();
        let before = h.state().unwrap_or_default();
        let mut applied = "default".to_string();
        if let Some(want) = value("--preset") {
            let presets = h.factory_presets();
            let Some((n, name)) = presets
                .into_iter()
                .find(|(_, name)| name.trim() == want.trim())
            else {
                println!("{id}\tNO_PRESET\t{want}");
                return;
            };
            if !h.set_factory_preset(n) {
                println!("{id}\tREFUSED\t{name}");
                return;
            }
            applied = format!("factory {n} {name}");
        }
        if let Some(file) = value("--state-file") {
            let Ok(bytes) = std::fs::read(&file) else {
                println!("{id}\tNO_FILE\t{file}");
                return;
            };
            if !h.set_state(&bytes) {
                println!("{id}\tREFUSED\t{file}");
                return;
            }
            applied = format!("state {file}");
        }
        // `--set NAME=VALUE` (repeatable): parameters by name, after the preset or state.
        let params = h.parameters();
        let mut set = Vec::new();
        for (k, a) in args.iter().enumerate() {
            if a != "--set" {
                continue;
            }
            let Some((name, v)) = args.get(k + 1).and_then(|s| s.rsplit_once('=')) else {
                continue;
            };
            let Ok(v) = v.trim().parse::<f32>() else {
                println!("{id}\tBAD_VALUE\t{name}");
                return;
            };
            let found = params
                .iter()
                .find(|p| p.name == name)
                .or_else(|| params.iter().find(|p| p.name.eq_ignore_ascii_case(name)));
            let Some(p) = found else {
                println!("{id}\tNO_PARAM\t{name}");
                return;
            };
            if v < p.min || v > p.max {
                println!("{id}\tOUT_OF_RANGE\t{name}={v} ({}..{})", p.min, p.max);
                return;
            }
            au.set_parameter(p.id, v, 0);
            set.push((p.id, p.name.clone(), v));
        }
        // One second: a chord for instruments, a tone for effects.
        let input: Vec<f32> = (0..frames)
            .map(|i| 0.25 * (i as f32 * 220.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect();
        let (mut l, mut r) = (vec![0f32; frames], vec![0f32; frames]);
        let mut peak = 0f32;
        let mut finite = true;
        for b in 0..48_000 / frames {
            // Effects that take MIDI (aumf) get none: some follow its notes (pitch).
            if info.kind == AuKind::Instrument && b == 1 {
                for note in [48, 55, 60, 64] {
                    au.midi(0x90, note, 100, 0);
                }
            }
            au.process((&input, &input), (&mut l, &mut r));
            for x in l.iter().chain(&r) {
                finite &= x.is_finite();
                peak = peak.max(x.abs());
            }
        }
        // The parameters set must read back (within the unit's own rounding).
        let mut readback = Vec::new();
        for (pid, name, v) in &set {
            let got = h.parameter(*pid).unwrap_or(f32::NAN);
            // A unit that snaps a value to its nearest step still took it when it shows the
            // same text for both.
            let same_text = h
                .parameter_text(*pid, got)
                .is_some_and(|t| Some(t) == h.parameter_text(*pid, *v));
            if (got - v).abs() > 0.02 * (1.0 + v.abs()) && !same_text {
                readback.push(format!("{name}={v}->{got}"));
            }
        }
        let present = h
            .present_preset()
            .map(|(n, name)| format!("{n} {name}"))
            .unwrap_or_default();
        let after = h.state().unwrap_or_default();
        if let Some(out) = value("--out") {
            let _ = std::fs::write(out, &after);
        }
        let status = if readback.is_empty() {
            "OK"
        } else {
            "PARAM_MISMATCH"
        };
        println!(
            "{id}\t{status}\t{applied}\tset {}\tpresent={present}\tstate {} bytes\tchanged={}\tpeak {peak:.3}\tfinite={finite}\terrors {}\t{}",
            set.len(),
            after.len(),
            after != before,
            au.errors(),
            readback.join(" ")
        );
        return;
    }

    // Parameters of one unit: "id<TAB>name<TAB>min<TAB>max<TAB>default<TAB>value<TAB>stepped".
    if let Some(id) = value("--params") {
        let Some(info) = find(&id) else {
            println!("{id}\tnot installed");
            return;
        };
        let au = match AuInstance::new(&info, 48_000, 4096) {
            Ok(a) => a,
            Err(e) => {
                println!("{id}\tFAILED to open: {e}");
                return;
            }
        };
        let h = au.handle();
        // `--texts V,V,…`: also the unit's own text for each of these values.
        let probe_values: Vec<f32> = value("--texts")
            .map(|t| t.split(',').filter_map(|x| x.trim().parse().ok()).collect())
            .unwrap_or_default();
        for p in h.parameters() {
            let names = h.parameter_value_names(p.id);
            let texts: Vec<String> = probe_values
                .iter()
                .map(|v| {
                    let v = p.min + (p.max - p.min) * v;
                    format!("{v}={}", h.parameter_text(p.id, v).unwrap_or_default())
                })
                .collect();
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                p.id,
                p.name,
                p.min,
                p.max,
                p.default,
                h.parameter(p.id).unwrap_or(f32::NAN),
                p.stepped,
                names.join("|"),
                texts.join("  ")
            );
        }
        return;
    }

    // Factory presets of every unit, each in its own process with a timeout:
    // "id<TAB>label<TAB>count<TAB>name | name | …".
    if args.iter().any(|a| a == "--all-presets") {
        let me = std::env::current_exe().ok();
        let secs: u64 = value("--timeout")
            .and_then(|v| v.parse().ok())
            .unwrap_or(40);
        for info in list().into_iter().filter(|i| !skipped(&i.id)) {
            let Some(exe) = &me else { return };
            let child = std::process::Command::new(exe)
                .args(["--presets", &info.id])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn();
            let Ok(mut child) = child else { continue };
            let mut out = child.stdout.take();
            let reader = std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(o) = out.as_mut() {
                    use std::io::Read;
                    let _ = o.read_to_string(&mut text);
                }
                text
            });
            let deadline = Instant::now() + Duration::from_secs(secs);
            let status = loop {
                match child.try_wait() {
                    Ok(Some(s)) => break Some(s),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    _ => {
                        let _ = child.kill();
                        break None;
                    }
                }
            };
            let text = reader.join().unwrap_or_default();
            let names: Vec<&str> = text
                .lines()
                // Some units print banners to stdout: only "number<TAB>name" lines count.
                .filter_map(|l| l.split_once('\t'))
                .filter(|(n, _)| n.parse::<i32>().is_ok())
                .map(|(_, name)| name.trim())
                .collect();
            match status {
                None => println!("{}\t{}\tHUNG", info.id, info.label()),
                Some(s) if !s.success() => println!("{}\t{}\tCRASHED", info.id, info.label()),
                _ if text.contains("FAILED") => {
                    println!("{}\t{}\tFAILED", info.id, info.label())
                }
                _ => println!(
                    "{}\t{}\t{}\t{}",
                    info.id,
                    info.label(),
                    names.len(),
                    names.join(" | ")
                ),
            }
        }
        return;
    }

    if args.iter().any(|a| a == "--all") {
        let me = std::env::current_exe().ok();
        for info in list()
            .into_iter()
            .filter(|i| !i.id.ends_with(":appl") && !skipped(&i.id))
        {
            let Some(exe) = &me else { return };
            let child = std::process::Command::new(exe)
                .args(["--test", &info.id, "--frames", &frames.to_string()])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn();
            let Ok(mut child) = child else { continue };
            let deadline = Instant::now() + Duration::from_secs(40);
            let status = loop {
                match child.try_wait() {
                    Ok(Some(s)) => break Some(s),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(100))
                    }
                    _ => {
                        let _ = child.kill();
                        break None;
                    }
                }
            };
            let mut text = String::new();
            if let Some(mut out) = child.stdout.take() {
                use std::io::Read;
                let _ = out.read_to_string(&mut text);
            }
            match status {
                None => println!("{}\tHUNG (killed after 40 s)", info.label()),
                Some(s) if !s.success() => println!("{}\tCRASHED ({s})", info.label()),
                _ => print!("{text}"),
            }
        }
        return;
    }
    eprintln!("use --list, --test ID or --all");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("au_probe is macOS only");
}
