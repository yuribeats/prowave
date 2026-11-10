//! `cargo xtask <task>`: the project's checks in one place (AGENTS.md lists them).
//!
//! - `check`: fmt, clippy -D warnings, tests, rt-check, miri on the triple buffer and duplex callback tests,
//!   cargo-deny and wasm-check
//! - `wasm-check`: `cargo check --target wasm32-unknown-unknown` for the engine (a build check;
//!   no test runs in the browser)
//! - `rt-check`: the tests named `*does_not_allocate` (debug build, assert_no_alloc)
//! - `bench`: release build checked against the lightweight targets (startup, memory, binary
//!   size, 24-track DSP load); see bench.rs
//! - `status`: counts the working menu entries and roadmap items for the README; see status.rs
mod bench;
mod status;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Crates that must build for the browser.
const WASM_CRATES: &[&str] = &["prowave-engine"];
/// Crates whose tests without system calls run under miri (the lock-free triple buffer and
/// the duplex stream's callback).
const MIRI_CRATES: &[&str] = &["prowave-engine"];
/// The duplex stream's tests that make no system call (macOS; matches nothing elsewhere).
const MIRI_DUPLEX_TESTS: &str = "duplex::tests::pure";

fn root() -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.parent().unwrap_or(here).to_path_buf()
}

/// Project-local tool installs (`cargo install --root target/tools-root ...`) are put first on
/// PATH, so tools like cargo-deny work without writing to ~/.cargo/bin.
fn cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    let local = root().join("target").join("tools-root").join("bin");
    if local.is_dir() {
        let mut paths = vec![local];
        if let Some(p) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&p));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            c.env("PATH", joined);
        }
    }
    c.current_dir(root());
    c
}

fn crate_exists(name: &str) -> bool {
    root()
        .join("crates")
        .join(name)
        .join("Cargo.toml")
        .is_file()
}

struct Step<'a> {
    name: &'a str,
    program: &'a str,
    args: Vec<&'a str>,
    /// Extra environment variables for this step.
    env: Vec<(&'a str, &'a str)>,
}

fn step<'a>(name: &'a str, program: &'a str, args: &[&'a str]) -> Step<'a> {
    Step {
        name,
        program,
        args: args.to_vec(),
        env: Vec::new(),
    }
}

fn run(s: &Step) -> bool {
    println!("\n==> {} ({} {})", s.name, s.program, s.args.join(" "));
    match cmd(s.program)
        .args(&s.args)
        .envs(s.env.iter().copied())
        .status()
    {
        Ok(st) if st.success() => true,
        Ok(st) => {
            eprintln!("xtask: `{}` failed ({st})", s.name);
            false
        }
        Err(e) => {
            eprintln!("xtask: cannot run `{}`: {e}", s.program);
            false
        }
    }
}

fn tool_available(program: &str, args: &[&str]) -> bool {
    cmd(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// True if any `.rs` file under `crates/<name>/src` contains an `unsafe` block or impl.
fn has_unsafe(name: &str) -> bool {
    fn scan(dir: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|e| {
            let p = e.path();
            if p.is_dir() {
                scan(&p)
            } else if p.extension().is_some_and(|x| x == "rs") {
                std::fs::read_to_string(&p).is_ok_and(|src| {
                    src.lines()
                        .filter(|l| !l.trim_start().starts_with("//"))
                        .any(|l| {
                            l.contains("unsafe {")
                                || l.contains("unsafe impl")
                                || l.contains("unsafe fn")
                        })
                })
            } else {
                false
            }
        })
    }
    scan(&root().join("crates").join(name).join("src"))
}

fn wasm_steps() -> Vec<Step<'static>> {
    WASM_CRATES
        .iter()
        .filter(|c| crate_exists(c))
        .map(|c| {
            step(
                "wasm-check",
                "cargo",
                &["check", "-p", c, "--target", "wasm32-unknown-unknown"],
            )
        })
        .collect()
}

fn rt_step() -> Step<'static> {
    // Tests named `*does_not_allocate` wrap `process()` in `assert_no_alloc` (debug only).
    step(
        "rt-check",
        "cargo",
        &[
            "test",
            "--workspace",
            "--",
            "does_not_allocate",
            "--nocapture",
        ],
    )
}

fn check() -> bool {
    let mut steps = vec![
        step("fmt", "cargo", &["fmt", "--all", "--check"]),
        step(
            "clippy",
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        step(
            "test",
            "cargo",
            &["test", "--workspace", "--", "--nocapture"],
        ),
        rt_step(),
    ];

    let unsafe_crates: Vec<&str> = MIRI_CRATES
        .iter()
        .copied()
        .filter(|c| crate_exists(c) && has_unsafe(c))
        .collect();
    if unsafe_crates.is_empty() {
        println!("xtask: no `unsafe` code in {MIRI_CRATES:?}; miri skipped");
    } else {
        // Miri needs nightly; `rustup +nightly component add miri`. It runs the lock-free
        // triple buffer's tests and, on macOS, the Core Audio callback's on made-up buffer
        // lists (`duplex::tests::pure`); the engine's other `unsafe` is FFI, which miri
        // cannot run. On Windows the Stacked Borrows check trips over the test harness's own
        // allocator on every test, so it is turned off there; data races are still checked.
        for c in unsafe_crates {
            steps.push(Step {
                name: "miri",
                program: "cargo",
                args: vec![
                    "+nightly",
                    "miri",
                    "test",
                    "-p",
                    c,
                    "--lib",
                    "--",
                    "triple",
                    MIRI_DUPLEX_TESTS,
                ],
                env: if cfg!(windows) {
                    vec![("MIRIFLAGS", "-Zmiri-disable-stacked-borrows")]
                } else {
                    Vec::new()
                },
            });
        }
    }

    if tool_available("cargo", &["deny", "--version"]) {
        steps.push(step(
            "deny",
            "cargo",
            &["deny", "check", "advisories", "bans", "licenses", "sources"],
        ));
    } else {
        eprintln!(
            "xtask: cargo-deny is required: `cargo install cargo-deny --locked` \
             (or project-local: add `--root target/tools-root --target-dir target/tools`)"
        );
        return false;
    }

    steps.extend(wasm_steps());

    let mut failed = Vec::new();
    for s in &steps {
        if !run(s) {
            failed.push(s.name);
        }
    }
    if failed.is_empty() {
        println!("\nxtask check: all {} steps passed", steps.len());
        true
    } else {
        eprintln!("\nxtask check: FAILED {failed:?}");
        false
    }
}

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let ok = match task.as_str() {
        "check" => check(),
        "wasm-check" => wasm_steps().iter().all(run),
        "rt-check" => run(&rt_step()),
        "bench" => bench::bench(&root()),
        "status" => match status::run(&root()) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("xtask status: {e}");
                false
            }
        },
        _ => {
            eprintln!("usage: cargo xtask <check|wasm-check|rt-check|bench|status>");
            false
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
