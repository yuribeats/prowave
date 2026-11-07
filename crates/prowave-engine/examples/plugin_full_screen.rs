//! Opens one plug-in's window (Audio Unit or VST 3; no sound), presses its green button,
//! and checks: it fills the screen, it is laid out (scaled or centred) as expected, a click
//! lands where the control is drawn, and leaving full screen restores the window exactly.
//! Then drags its corner (programmatically) to 1.5x and checks the scaling. Screenshots of
//! that window only (`screencapture -l`) go to `dir`.
//!
//! cargo run --release -p prowave-engine --example plugin_full_screen -- 'aumf:FQ4p:FabF' dir
//! cargo run --release -p prowave-engine --example plugin_full_screen -- \
//!     '/Library/Audio/Plug-Ins/VST3/FabFilter Pro-Q 4.vst3' dir [--dclick 0.5,0.6]
//!
//! `--dclick fx,fy` double-clicks the point at those fractions of the plug-in's own view
//! (from its bottom left) while in full screen, through a synthesized event, and lists the
//! parameters that changed.
#[cfg(target_os = "macos")]
fn main() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventMask, NSEventModifierFlags,
        NSEventType,
    };
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRunLoop, NSSize};
    use prowave_engine::au::{find, AuInstance};
    use prowave_engine::plugin_handle::{PluginHandle, PluginWindow};
    use prowave_engine::vst3::{module, Vst3Instance};
    use std::path::{Path, PathBuf};

    let args: Vec<String> = std::env::args().collect();
    let (Some(id), Some(dir)) = (args.get(1), args.get(2)) else {
        println!("usage: plugin_full_screen <au id | path.vst3> <dir> [--dclick fx,fy]");
        return;
    };
    let dclick: Option<(f64, f64)> = args
        .iter()
        .position(|a| a == "--dclick")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| {
            let (a, b) = s.split_once(',')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        });
    let dir = PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // Keep the instance alive as long as its window.
    enum Inst {
        Au(AuInstance),
        Vst3(Box<Vst3Instance>),
    }
    let (inst, label) = if id.ends_with(".vst3") {
        let path = Path::new(id);
        let Some(class) = module(path)
            .ok()
            .and_then(|m| m.classes().into_iter().next())
        else {
            println!("{id}: no class");
            return;
        };
        match Vst3Instance::new(path, &class.cid, 48_000, 512) {
            Ok(i) => (Inst::Vst3(Box::new(i)), format!("{} (VST3)", class.name)),
            Err(e) => {
                println!("open failed: {e}");
                return;
            }
        }
    } else {
        let Some(info) = find(id) else {
            println!("{id} not installed");
            return;
        };
        match AuInstance::new(&info, 48_000, 512) {
            Ok(a) => (Inst::Au(a), format!("{} (AU)", info.label())),
            Err(e) => {
                println!("open failed: {e}");
                return;
            }
        }
    };
    let h: PluginHandle = match &inst {
        Inst::Au(a) => a.handle().into(),
        Inst::Vst3(v) => v.handle().into(),
    };
    let params = |h: &PluginHandle| -> Vec<(String, f64)> {
        match h {
            PluginHandle::Au(a) => a
                .parameters()
                .into_iter()
                .map(|p| (p.name.clone(), f64::from(a.parameter(p.id).unwrap_or(0.0))))
                .collect(),
            PluginHandle::Vst3(v) => (0..v.parameter_count())
                .map(|i| {
                    let name = v.parameter_info(i).map(|p| p.0).unwrap_or_default();
                    (name, v.parameter(i).unwrap_or(0.0))
                })
                .collect(),
        }
    };
    let w: PluginWindow = match h.open_window(&label) {
        Ok(w) => w,
        Err(e) => {
            println!("window failed: {e}");
            return;
        }
    };
    app.finishLaunching();
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    // As the app's event loop does: events dispatched (full-screen transitions need them),
    // and the window tended about once a frame.
    let pump = |w: &PluginWindow, secs: f64| {
        let end = NSDate::dateWithTimeIntervalSinceNow(secs);
        while end.timeIntervalSinceNow() > 0.0 {
            let until = NSDate::dateWithTimeIntervalSinceNow(0.02);
            // SAFETY: AppKit's run loop mode constant.
            let mode = unsafe { NSDefaultRunLoopMode };
            if let Some(ev) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                mode,
                true,
            ) {
                app.sendEvent(&ev);
            }
            w.fit();
        }
    };
    let slug: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let shot = |w: &PluginWindow, tag: &str| {
        let file = dir.join(format!("{slug}-{tag}.png"));
        let mut ok = false;
        for _ in 0..4 {
            let _ = std::fs::remove_file(&file);
            ok = std::process::Command::new("screencapture")
                .args(["-x", "-o", "-l", &w.window_number().to_string()])
                .arg(&file)
                .status()
                .is_ok_and(|s| s.success())
                && file.exists();
            if ok {
                break;
            }
            pump(w, 0.5);
        }
        println!(
            "  shot {tag}: {} {}",
            file.display(),
            if ok { "" } else { "FAILED" }
        );
    };
    // Where the layout math says point q of the view shows (window coordinates).
    let expect = |w: &PluginWindow, q: (f64, f64)| {
        let ((nw, nh), (cw, ch)) = w.sizes();
        let mut s = (cw / nw).min(ch / nh);
        if (s - 1.0).abs() < 1e-3 {
            s = 1.0;
        }
        (
            (cw - s * nw) / 2.0 + s * q.0,
            (ch - s * nh) / 2.0 + s * q.1,
            s,
        )
    };
    let check_click = |w: &PluginWindow, tag: &str| {
        let ((nw, nh), _) = w.sizes();
        let q = (nw * 0.3, nh * 0.6);
        let (ex, ey, s) = expect(w, q);
        match w.probe_click(q.0, q.1) {
            Some(c) => println!(
                "  click {tag}: scale {s:.3}; view point ({:.1},{:.1}) shows at ({:.1},{:.1}) [math ({ex:.1},{ey:.1})], hits plug-in {}, arrives at ({:.1},{:.1})",
                q.0, q.1, c.shown.0, c.shown.1, c.hits_plugin, c.arrives.0, c.arrives.1
            ),
            None => println!("  click {tag}: no content view"),
        }
    };
    let Some(win) = app.windowWithWindowNumber(w.window_number()) else {
        println!("no window");
        return;
    };
    let state = |tag: &str| {
        let f = win.frame();
        let (n, c) = w.sizes();
        println!(
            "  {tag}: full {} level {} hides {} frame ({:.0},{:.0} {:.0}x{:.0}) view {:.0}x{:.0} content {:.0}x{:.0}",
            w.is_full_screen(),
            win.level(),
            win.hidesOnDeactivate(),
            f.origin.x,
            f.origin.y,
            f.size.width,
            f.size.height,
            n.0,
            n.1,
            c.0,
            c.1
        );
    };
    println!("{label}: window {}", w.window_number());
    pump(&w, 1.5);
    state("open");
    shot(&w, "1-normal");
    let before = win.frame();

    w.toggle_full_screen();
    pump(&w, 3.0);
    state("full screen");
    shot(&w, "2-full");
    check_click(&w, "full");
    if let Some((fx, fy)) = dclick {
        let ((nw, nh), _) = w.sizes();
        let (x, y, _) = expect(&w, (nw * fx, nh * fy));
        let p0 = params(&h);
        for count in 1..=2 {
            for ty in [NSEventType::LeftMouseDown, NSEventType::LeftMouseUp] {
                let ev = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                    ty,
                    NSPoint::new(x, y),
                    NSEventModifierFlags::empty(),
                    0.0,
                    w.window_number(),
                    None,
                    0,
                    count,
                    1.0,
                );
                if let Some(ev) = ev {
                    win.sendEvent(&ev);
                }
            }
            pump(&w, 0.1);
        }
        pump(&w, 0.8);
        let p1 = params(&h);
        let changed: Vec<String> = p0
            .iter()
            .zip(&p1)
            .filter(|(a, b)| (a.1 - b.1).abs() > 1e-6)
            .map(|(a, b)| format!("{} {:.3}->{:.3}", a.0, a.1, b.1))
            .collect();
        println!(
            "  double-click at window ({x:.1},{y:.1}): {} parameters changed {:?}",
            changed.len(),
            changed.iter().take(6).collect::<Vec<_>>()
        );
        shot(&w, "3-full-clicked");
    }

    w.toggle_full_screen();
    pump(&w, 3.0);
    state("left full screen");
    let after = win.frame();
    println!(
        "  frame restored exactly: {}",
        before.origin == after.origin && before.size == after.size
    );
    shot(&w, "4-restored");

    // The corner dragged (as AppKit does for a drag, keeping the content's shape): to 1.3x
    // the plug-in's size, or to 80% of the window when that would not fit on the screen.
    let ((nw, nh), (cw, ch)) = w.sizes();
    let k = if nw * 1.3 < 1400.0 && nh * 1.3 < 850.0 {
        1.3
    } else {
        0.8 * cw / nw
    };
    win.setContentSize(NSSize::new(nw * k, nh * k));
    let _ = ch;
    pump(&w, 1.0);
    state("dragged");
    check_click(&w, "dragged");
    shot(&w, "5-scaled");
    drop(w);
    pump_only(0.2);
    drop(inst);

    fn pump_only(secs: f64) {
        let until = NSDate::dateWithTimeIntervalSinceNow(secs);
        NSRunLoop::currentRunLoop().runUntilDate(&until);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
