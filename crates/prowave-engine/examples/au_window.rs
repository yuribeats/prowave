//! Opens one Audio Unit's window for a few seconds (no sound), and checks its settings
//! survive a save and restore.
//!
//! cargo run --release -p prowave-engine --example au_window -- 'aumf:FQ4p:FabF' [seconds]
#[cfg(target_os = "macos")]
fn main() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::{NSDate, NSRunLoop};
    use prowave_engine::au::{find, AuInstance};

    let args: Vec<String> = std::env::args().collect();
    let id = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "aumf:FQ4p:FabF".into());
    let secs: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4.0);
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let Some(info) = find(&id) else {
        println!("{id} not installed");
        return;
    };
    let au = match AuInstance::new(&info, 48_000, 512) {
        Ok(a) => a,
        Err(e) => {
            println!("open failed: {e}");
            return;
        }
    };
    let h = au.handle();
    let state = h.state();
    println!("state {} bytes", state.as_ref().map_or(0, Vec::len));
    if let Some(s) = &state {
        println!("restore {}", h.set_state(s));
    }
    match h.open_window(&info.label()) {
        Ok(w) => {
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            println!("window number {}", w.window_number());
            // As the app does once a frame: the window follows the view's size.
            let steps = (secs * 20.0) as usize;
            for _ in 0..steps {
                let until = NSDate::dateWithTimeIntervalSinceNow(0.05);
                NSRunLoop::currentRunLoop().runUntilDate(&until);
                w.fit();
            }
            println!("window open {}", w.is_open());
        }
        Err(e) => println!("window failed: {e}"),
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
