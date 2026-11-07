//! `cargo xtask bench`: builds the release app and checks it against the lightweight targets
//! in LINUX-MACOS-LIGHTWEIGHT.md §1 and §6. The app measures itself (`prowave --bench <file>`,
//! see crates/prowave-app/src/bench.rs); this side builds it, runs it, and compares.
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// One target: the report key, its limit, unit, and what it means.
struct Target {
    key: &'static str,
    max: f64,
    unit: &'static str,
    what: &'static str,
    /// Measured only when a window could be opened.
    needs_window: bool,
}

const TARGETS: &[Target] = &[
    Target {
        key: "binary_mb",
        max: 50.0,
        unit: "MB",
        what: "release binary size",
        needs_window: false,
    },
    Target {
        key: "startup_ms",
        max: 400.0,
        unit: "ms",
        what: "start to first frame",
        needs_window: true,
    },
    Target {
        key: "idle_rss_mb",
        max: 150.0,
        unit: "MB",
        what: "memory, empty project, idle",
        needs_window: true,
    },
    Target {
        key: "dsp24_load",
        max: 0.15,
        unit: "of a core",
        what: "24 instrument tracks playing",
        needs_window: false,
    },
    Target {
        key: "dsp24_rss_mb",
        max: 300.0,
        unit: "MB",
        what: "memory with 24 tracks",
        needs_window: false,
    },
];

/// How long the app may take to measure everything before it is stopped.
const TIMEOUT: Duration = Duration::from_secs(180);

fn value(report: &str, key: &str) -> Option<f64> {
    report
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .and_then(|v| v.trim().parse().ok())
}

/// Checks `report` (with `binary_mb` added) against the targets. Returns the lines to print
/// and whether everything measured is within its limit. A missing window-only value is a
/// skip (no display on this machine); any other missing value is a failure. With
/// `window_advisory`, window numbers over their limit are reported but do not fail: build
/// servers have no GPU, so startup and memory there measure a software renderer.
fn evaluate(report: &str, window_advisory: bool) -> (Vec<String>, bool) {
    let window_error = report.lines().find_map(|l| l.strip_prefix("window_error="));
    let mut lines = Vec::new();
    let mut ok = true;
    for t in TARGETS {
        match value(report, t.key) {
            Some(v) if v <= t.max => lines.push(format!(
                "  ok    {:<28} {v:>9.3} {} (limit {})",
                t.what, t.unit, t.max
            )),
            Some(v) if t.needs_window && window_advisory => lines.push(format!(
                "  note  {:<28} {v:>9.3} {} (limit {}; not enforced without a GPU)",
                t.what, t.unit, t.max
            )),
            Some(v) => {
                ok = false;
                lines.push(format!(
                    "  OVER  {:<28} {v:>9.3} {} (limit {})",
                    t.what, t.unit, t.max
                ));
            }
            None if t.needs_window && window_error.is_some() => lines.push(format!(
                "  skip  {:<28} no window could be opened here",
                t.what
            )),
            None => {
                ok = false;
                lines.push(format!("  MISSING {} ({})", t.what, t.key));
            }
        }
    }
    if let Some(e) = window_error {
        lines.push(format!("  (window: {e})"));
    }
    match value(report, "dsp24_peak") {
        Some(peak) if peak > 0.0 => {}
        _ => {
            ok = false;
            lines.push("  SILENT the 24-track render produced no sound".into());
        }
    }
    (lines, ok)
}

pub fn bench(root: &Path) -> bool {
    println!("\n==> bench: cargo build --release -p prowave-app");
    let built = Command::new("cargo")
        .args(["build", "--release", "-p", "prowave-app"])
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success());
    if !built {
        eprintln!("xtask bench: release build failed");
        return false;
    }
    let exe = root
        .join("target")
        .join("release")
        .join(format!("prowave{}", std::env::consts::EXE_SUFFIX));
    let binary_mb = match std::fs::metadata(&exe) {
        Ok(m) => m.len() as f64 / (1024.0 * 1024.0),
        Err(e) => {
            eprintln!("xtask bench: cannot read {}: {e}", exe.display());
            return false;
        }
    };

    let out = std::env::temp_dir().join(format!("prowave-bench-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    println!("==> bench: {} --bench {}", exe.display(), out.display());
    let started = Instant::now();
    let mut child = match Command::new(&exe).arg("--bench").arg(&out).spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("xtask bench: cannot start {}: {e}", exe.display());
            return false;
        }
    };
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                eprintln!("xtask bench: the app did not finish within {TIMEOUT:?}");
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                eprintln!("xtask bench: waiting for the app failed: {e}");
                return false;
            }
        }
    }
    let report = match std::fs::read_to_string(&out) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("xtask bench: the app wrote no report ({e})");
            return false;
        }
    };
    let _ = std::fs::remove_file(&out);
    let report = format!("{report}binary_mb={binary_mb:.2}\n");
    let advisory = std::env::var_os("PROWAVE_BENCH_WINDOW_ADVISORY").is_some();
    let (lines, ok) = evaluate(&report, advisory);
    // On GitHub Actions, also as annotations: they show on the run's page without signing in.
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        for l in &lines {
            let failed = l.contains("OVER") || l.contains("MISSING") || l.contains("SILENT");
            let level = if failed { "error" } else { "notice" };
            println!(
                "::{level} title=bench {}::{}",
                std::env::consts::OS,
                l.trim()
            );
        }
    }
    println!(
        "\nPro Wave lightweight targets ({} {}):",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    for l in lines {
        println!("{l}");
    }
    if ok {
        println!("\nxtask bench: all targets met");
    } else {
        eprintln!("\nxtask bench: FAILED");
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "startup_ms=210.5\nidle_rss_mb=96.0\ndsp24_load=0.031\n\
                        dsp24_peak=0.8\ndsp24_rss_mb=120.0\nbinary_mb=14.2\n";

    #[test]
    fn a_report_within_every_limit_passes() {
        let (lines, ok) = evaluate(GOOD, false);
        assert!(ok, "{lines:?}");
        assert_eq!(lines.len(), TARGETS.len());
    }

    #[test]
    fn a_value_over_its_limit_fails() {
        let slow = GOOD.replace("startup_ms=210.5", "startup_ms=812");
        let (lines, ok) = evaluate(&slow, false);
        assert!(!ok);
        assert!(lines
            .iter()
            .any(|l| l.contains("OVER") && l.contains("first frame")));
        // Without a GPU (build servers) the window numbers are only reported...
        assert!(evaluate(&slow, true).1);
        // ...but the DSP numbers still count.
        let heavy = GOOD.replace("dsp24_load=0.031", "dsp24_load=0.4");
        assert!(!evaluate(&heavy, true).1);
    }

    #[test]
    fn without_a_window_only_the_window_numbers_are_skipped() {
        let headless = "window_error=no display\ndsp24_load=0.03\ndsp24_peak=0.7\n\
                        dsp24_rss_mb=40\nbinary_mb=14\n";
        let (lines, ok) = evaluate(headless, false);
        assert!(ok, "{lines:?}");
        assert_eq!(lines.iter().filter(|l| l.contains("skip")).count(), 2);

        let (_, ok) = evaluate("window_error=x\ndsp24_peak=0.7\nbinary_mb=14\n", false);
        assert!(!ok, "the DSP numbers are always required");
    }

    #[test]
    fn a_silent_render_fails() {
        let (_, ok) = evaluate(&GOOD.replace("dsp24_peak=0.8", "dsp24_peak=0.000"), false);
        assert!(!ok);
    }
}
