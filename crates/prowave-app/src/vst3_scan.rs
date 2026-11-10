//! The VST 3 plug-ins on this Mac, for the slots' menus.
//!
//! Loading a plug-in runs its code, and a broken one can crash the program that loads it.
//! So each `.vst3` bundle is scanned by a helper: this same program started as
//! `prowave --vst3-scan <bundle>`, which loads it, lists its classes as one line of JSON and
//! exits. A helper that crashes, or takes longer than [`SCAN_TIMEOUT`], gets its bundle
//! quarantined (the list `plugins.rs` keeps for plug-ins that crashed Pro Wave). Results are
//! cached in the settings folder by bundle path and modification time, so only new or
//! updated bundles are scanned again. The scan runs in the background; the menus are
//! refreshed when it ends.
//!
//! `prowave --vst3-check [--editor DIR] <bundle>...` instantiates every class of the given
//! bundles, processes a second of silence and a note, round-trips the settings and
//! optionally opens the window (screenshot by window number), printing one JSON line each.
//! `prowave --vst3-check-all [--shots DIR] [--editor-for a,b,..]` runs that for every
//! bundle, each in its own helper, and prints a summary.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long one bundle's scan may take.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(30);
/// Helpers running at once.
const PARALLEL: usize = 6;
/// What a helper's answer line starts with (plug-ins print their own lines too).
const MARK: &str = "PROWAVE-VST3 ";
const CACHE_VERSION: u64 = 1;

/// A VST 3 class the menus offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vst3Entry {
    /// `vst3:` and the class id.
    pub id: String,
    /// The bundle.
    pub path: String,
    pub name: String,
    pub vendor: String,
    pub instrument: bool,
    pub sub_categories: String,
    pub version: String,
}

/// One bundle as last scanned.
#[derive(Clone, Debug, PartialEq)]
pub struct BundleScan {
    pub path: String,
    pub mtime: u64,
    pub classes: Vec<Vst3Entry>,
    /// Why the scan failed (it is not tried again until the bundle changes).
    pub error: Option<String>,
}

/// What a helper made of a bundle.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Classes(Vec<Vst3Entry>),
    /// It loaded but said no (not a VST 3, no factory...).
    Failed(String),
    /// The helper died.
    Crashed(String),
    TimedOut,
}

fn entry_json(e: &Vst3Entry) -> Value {
    json!({
        "id": e.id, "name": e.name, "vendor": e.vendor, "instrument": e.instrument,
        "sub_categories": e.sub_categories, "version": e.version,
    })
}

fn entry_of(v: &Value, path: &str) -> Option<Vst3Entry> {
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let id = s("id");
    prowave_project_id_ok(&id).then(|| Vst3Entry {
        id,
        path: path.to_string(),
        name: s("name"),
        vendor: s("vendor"),
        instrument: v
            .get("instrument")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        sub_categories: s("sub_categories"),
        version: s("version"),
    })
}

/// A `vst3:` id with 32 hex digits.
fn prowave_project_id_ok(id: &str) -> bool {
    id.strip_prefix("vst3:")
        .is_some_and(|h| h.len() == 32 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

fn scan_json(s: &BundleScan) -> Value {
    json!({
        "path": s.path, "mtime": s.mtime, "error": s.error,
        "classes": s.classes.iter().map(entry_json).collect::<Vec<_>>(),
    })
}

/// The cache as text.
pub fn cache_text(scans: &[BundleScan]) -> String {
    json!({
        "version": CACHE_VERSION,
        "bundles": scans.iter().map(scan_json).collect::<Vec<_>>(),
    })
    .to_string()
}

/// The cache from text (empty if it is not ours or is from another version).
pub fn parse_cache(text: &str) -> Vec<BundleScan> {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    if v.get("version").and_then(Value::as_u64) != Some(CACHE_VERSION) {
        return Vec::new();
    }
    let Some(list) = v.get("bundles").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let path = b.get("path")?.as_str()?.to_string();
            let classes = b
                .get("classes")
                .and_then(Value::as_array)
                .map(|cs| cs.iter().filter_map(|c| entry_of(c, &path)).collect())
                .unwrap_or_default();
            Some(BundleScan {
                mtime: b.get("mtime")?.as_u64()?,
                error: b.get("error").and_then(Value::as_str).map(String::from),
                classes,
                path,
            })
        })
        .collect()
}

fn cache_file() -> Option<PathBuf> {
    crate::recent_files::config_dir().map(|d| d.join("vst3-cache.json"))
}

fn load_cache() -> Vec<BundleScan> {
    cache_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|t| parse_cache(&t))
        .unwrap_or_default()
}

fn save_cache(scans: &[BundleScan]) {
    if let Some(f) = cache_file() {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(f, cache_text(scans)));
        }
    }
}

/// When the bundle last changed: the newest of the bundle folder, its Info.plist and its
/// code (an update can replace the code without touching the folder), in seconds.
pub fn bundle_mtime(path: &Path) -> u64 {
    let secs = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs())
    };
    let mut t = secs(path).max(secs(&path.join("Contents/Info.plist")));
    if let Ok(rd) = std::fs::read_dir(path.join("Contents/MacOS")) {
        for e in rd.flatten() {
            t = t.max(secs(&e.path()));
        }
    }
    t
}

/// The quarantine id of a bundle that crashed or hung its scan. It holds the bundle's time,
/// so an updated bundle is scanned again.
pub fn quarantine_id(path: &str, mtime: u64) -> String {
    format!("vst3-bundle:{path}@{mtime}")
}

/// What [`rescan`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Bundles scanned now that listed their classes.
    pub ok: usize,
    /// Bundles taken from the cache.
    pub cached: usize,
    /// Bundles that loaded but failed (named, with why).
    pub failed: Vec<(String, String)>,
    /// Bundles that crashed or hung their scan now (to quarantine), with why.
    pub quarantined: Vec<(String, String)>,
    /// Their quarantine ids.
    pub quarantine_ids: Vec<String>,
    /// Bundles left alone because they were quarantined before.
    pub skipped: Vec<String>,
}

/// Brings `cache` in line with `bundles`: an entry whose path and time match is kept, the
/// rest are scanned with `scan` (in parallel); a bundle in `quarantined` is skipped. Returns
/// the new cache (bundles that are gone are dropped) and what happened.
pub fn rescan(
    cache: &[BundleScan],
    bundles: &[PathBuf],
    quarantined: &[String],
    scan: &(dyn Fn(&Path) -> Outcome + Sync),
) -> (Vec<BundleScan>, Report) {
    let mut report = Report::default();
    let mut out: Vec<Option<BundleScan>> = vec![None; bundles.len()];
    let mut todo = Vec::new();
    for (i, b) in bundles.iter().enumerate() {
        let path = b.to_string_lossy().to_string();
        let mtime = bundle_mtime(b);
        if quarantined.contains(&quarantine_id(&path, mtime)) {
            report.skipped.push(path);
            continue;
        }
        match cache.iter().find(|c| c.path == path && c.mtime == mtime) {
            Some(c) => {
                report.cached += 1;
                out[i] = Some(c.clone());
            }
            None => todo.push((i, path, mtime)),
        }
    }
    let results: Mutex<Vec<(usize, String, u64, Outcome)>> = Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..PARALLEL.min(todo.len()) {
            s.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some((i, path, mtime)) = todo.get(k) else {
                    break;
                };
                let o = scan(Path::new(path));
                if let Ok(mut r) = results.lock() {
                    r.push((*i, path.clone(), *mtime, o));
                }
            });
        }
    });
    for (i, path, mtime, o) in results.into_inner().unwrap_or_default() {
        let (classes, error) = match o {
            Outcome::Classes(c) => {
                report.ok += 1;
                (c, None)
            }
            Outcome::Failed(e) => {
                report.failed.push((path.clone(), e.clone()));
                (Vec::new(), Some(e))
            }
            Outcome::Crashed(e) => {
                report.quarantine_ids.push(quarantine_id(&path, mtime));
                report.quarantined.push((path, e));
                continue;
            }
            Outcome::TimedOut => {
                report.quarantine_ids.push(quarantine_id(&path, mtime));
                report
                    .quarantined
                    .push((path, format!("no answer in {}s", SCAN_TIMEOUT.as_secs())));
                continue;
            }
        };
        out[i] = Some(BundleScan {
            path,
            mtime,
            classes,
            error,
        });
    }
    (out.into_iter().flatten().collect(), report)
}

/// Every bundle in the VST 3 folders.
pub fn all_bundles() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut v: Vec<PathBuf> = prowave_engine::vst3::plugin_dirs()
            .iter()
            .flat_map(|d| prowave_engine::vst3::find_bundles(d))
            .collect();
        v.dedup();
        v
    }
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

/// Ends the process now with `code`, after flushing what it printed, without running
/// static destructors or atexit handlers: some plug-ins crash in theirs (Native
/// Instruments' Qt-based ones segfault in `QApplication::~QApplication`), which would make
/// macOS write a crash report for a run that worked.
pub fn exit_now(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    extern "C" {
        fn _exit(code: i32) -> !;
    }
    // SAFETY: `_exit` ends the process at once; nothing after it runs.
    unsafe { _exit(code) }
}

/// What a helper printed, and how it died if it crashed.
type HelperRun = (Option<String>, String);

/// Runs `args` on this program, reading its stdout, for at most `timeout`.
fn run_helper(args: &[&std::ffi::OsStr], timeout: Duration) -> Result<HelperRun, Outcome> {
    use std::io::Read;
    let exe = std::env::current_exe().map_err(|e| Outcome::Failed(e.to_string()))?;
    let mut child = std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| Outcome::Failed(format!("the helper did not start: {e}")))?;
    // Read stdout on a thread so a chatty plug-in cannot fill the pipe and stall.
    let mut out = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Outcome::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(Outcome::Crashed(e.to_string())),
        }
    };
    let text = reader.join().unwrap_or_default();
    if status.code().is_none() {
        #[cfg(unix)]
        let why = {
            use std::os::unix::process::ExitStatusExt;
            format!("crashed (signal {})", status.signal().unwrap_or(0))
        };
        #[cfg(not(unix))]
        let why = "crashed".to_string();
        return Ok((Some(why), text));
    }
    Ok((None, text))
}

fn marked_lines(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|l| l.strip_prefix(MARK))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Scans one bundle in a helper process.
pub fn scan_in_helper(path: &Path) -> Outcome {
    let args = [std::ffi::OsStr::new("--vst3-scan"), path.as_os_str()];
    let (crash, text) = match run_helper(&args, SCAN_TIMEOUT) {
        Ok(r) => r,
        Err(o) => return o,
    };
    // A helper that crashed after giving its whole answer (while exiting) still scanned.
    let Some(v) = marked_lines(&text).pop() else {
        return match crash {
            Some(c) => Outcome::Crashed(format!("the scan {c}")),
            None => Outcome::Failed("the scan gave no answer".into()),
        };
    };
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Outcome::Failed(e.to_string());
    }
    let p = path.to_string_lossy();
    Outcome::Classes(
        v.get("classes")
            .and_then(Value::as_array)
            .map(|cs| cs.iter().filter_map(|c| entry_of(c, &p)).collect())
            .unwrap_or_default(),
    )
}

/// `--vst3-scan <bundle>`: the helper's side. Prints one marked JSON line.
pub fn run_scan_helper(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage: prowave --vst3-scan <bundle.vst3>");
        return 2;
    };
    #[cfg(target_os = "macos")]
    {
        match prowave_engine::vst3::scan_bundle(Path::new(path)) {
            Ok(classes) => {
                let list: Vec<Value> = classes
                    .iter()
                    .map(|c| {
                        entry_json(&Vst3Entry {
                            id: c.id(),
                            path: path.clone(),
                            name: c.name.clone(),
                            vendor: c.vendor.clone(),
                            instrument: c.instrument(),
                            sub_categories: c.sub_categories.clone(),
                            version: c.version.clone(),
                        })
                    })
                    .collect();
                println!("{MARK}{}", json!({ "classes": list }));
            }
            Err(e) => println!("{MARK}{}", json!({ "error": e })),
        }
        0
    }
    #[cfg(not(target_os = "macos"))]
    {
        println!(
            "{MARK}{}",
            json!({ "error": "VST 3 hosting is macOS only" })
        );
        0
    }
}

// ---------------------------------------------------------------- the app's list

static CLASSES: Mutex<Option<Vec<Vst3Entry>>> = Mutex::new(None);
static UPDATED: AtomicBool = AtomicBool::new(false);
static STATUS: Mutex<Option<String>> = Mutex::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);

fn classes_of(scans: &[BundleScan]) -> Vec<Vst3Entry> {
    scans
        .iter()
        .flat_map(|s| s.classes.iter().cloned())
        .collect()
}

/// The VST 3 classes known now (from the last scan, or the cache before it ends).
pub fn known() -> Vec<Vst3Entry> {
    let Ok(mut c) = CLASSES.lock() else {
        return Vec::new();
    };
    c.get_or_insert_with(|| {
        if cfg!(test) {
            Vec::new()
        } else {
            classes_of(&load_cache())
        }
    })
    .clone()
}

/// The bundle that holds class `id`.
pub fn path_of(id: &str) -> Option<String> {
    known().into_iter().find(|e| e.id == id).map(|e| e.path)
}

/// Starts the background scan (once).
pub fn start_background_scan() {
    if cfg!(test) || STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    spawn_scan();
}

/// Scans the VST 3 folders again now, in the background (plug-ins were installed while the
/// program runs). Bundles that did not change come from the cache. [`take_update`] reports
/// its end as it does the first scan's. While a scan runs, another one is queued after it.
pub fn rescan_now() {
    if cfg!(test) {
        return;
    }
    STARTED.store(true, Ordering::Relaxed);
    spawn_scan();
}

/// A scan is running.
static SCANNING: AtomicBool = AtomicBool::new(false);
/// Another scan was asked for while one ran.
static AGAIN: AtomicBool = AtomicBool::new(false);

fn spawn_scan() {
    if SCANNING.swap(true, Ordering::AcqRel) {
        AGAIN.store(true, Ordering::Release);
        return;
    }
    let started = std::thread::Builder::new()
        .name("vst3-scan".into())
        .spawn(|| loop {
            scan_once();
            UPDATED.store(true, Ordering::Release);
            if AGAIN.swap(false, Ordering::AcqRel) {
                continue;
            }
            SCANNING.store(false, Ordering::Release);
            // Asked for between the two lines above: this thread takes it, unless a new one
            // already has.
            if !AGAIN.swap(false, Ordering::AcqRel) || SCANNING.swap(true, Ordering::AcqRel) {
                break;
            }
        });
    if started.is_err() {
        SCANNING.store(false, Ordering::Release);
    }
}

/// One pass over the VST 3 folders: the cache and the list of classes are brought up to date.
fn scan_once() {
    let bundles = all_bundles();
    let cache = load_cache();
    let (scans, report) = rescan(
        &cache,
        &bundles,
        &crate::plugins::quarantined(),
        &scan_in_helper,
    );
    for id in &report.quarantine_ids {
        crate::plugins::quarantine(id);
    }
    save_cache(&scans);
    if let Ok(mut c) = CLASSES.lock() {
        *c = Some(classes_of(&scans));
    }
    if !report.quarantined.is_empty() {
        let names: Vec<String> = report
            .quarantined
            .iter()
            .map(|(p, _)| {
                Path::new(p)
                    .file_stem()
                    .map_or(p.clone(), |s| s.to_string_lossy().to_string())
            })
            .collect();
        if let Ok(mut s) = STATUS.lock() {
            *s = Some(format!(
                "VST3 scan: {} crashed or hung and are turned off",
                names.join(", ")
            ));
        }
    }
}

/// Whether `PROWAVE_QUIT_AFTER` seconds have passed since the first call (`None` when it is
/// not set).
pub fn quit_due() -> Option<bool> {
    static START: std::sync::OnceLock<Option<(Instant, f64)>> = std::sync::OnceLock::new();
    START
        .get_or_init(|| {
            std::env::var("PROWAVE_QUIT_AFTER")
                .ok()
                .and_then(|s| s.parse().ok())
                .map(|s| (Instant::now(), s))
        })
        .map(|(t, s)| t.elapsed().as_secs_f64() > s)
}

/// Whether a scan ended since the last call (the menus should be rebuilt), and a message
/// for the status line if something was quarantined.
pub fn take_update() -> Option<Option<String>> {
    UPDATED
        .swap(false, Ordering::Acquire)
        .then(|| STATUS.lock().ok().and_then(|mut s| s.take()))
}

// ---------------------------------------------------------------- checks

/// `--vst3-check [--editor DIR] <bundle>...`: every class of each bundle, one JSON line
/// each. Runs in a helper (`--vst3-check-all`) so a crash only loses that bundle.
pub fn run_check(args: &[String]) -> i32 {
    #[cfg(target_os = "macos")]
    {
        let mut editor: Option<PathBuf> = None;
        let mut bundles = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--editor" {
                editor = it.next().map(PathBuf::from);
            } else {
                bundles.push(PathBuf::from(a));
            }
        }
        for b in bundles {
            match prowave_engine::vst3::module(&b) {
                Ok(m) => {
                    let classes = m.classes();
                    if classes.is_empty() {
                        println!(
                            "{MARK}{}",
                            json!({"bundle": b, "ok": false, "stage": "scan", "error": "no audio classes"})
                        );
                    }
                    for c in classes {
                        let v = check_class(&b, &c, editor.as_deref());
                        println!("{MARK}{v}");
                    }
                }
                Err(e) => println!(
                    "{MARK}{}",
                    json!({"bundle": b, "ok": false, "stage": "load", "error": e})
                ),
            }
        }
        0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = args;
        1
    }
}

#[cfg(target_os = "macos")]
fn check_class(bundle: &Path, c: &prowave_engine::vst3::Vst3Class, editor: Option<&Path>) -> Value {
    use prowave_engine::vst3::{pump_main_loop, Vst3Instance};
    use prowave_engine::{BlockProcessor, BlockTransport};
    const SR: u32 = 48_000;
    const BLOCK: usize = 512;
    let label = format!("{}: {}", c.vendor, c.name);
    let mut v =
        json!({"bundle": bundle, "name": label, "id": c.id(), "instrument": c.instrument()});
    let t0 = Instant::now();
    let mut inst = match Vst3Instance::new(bundle, &c.cid, SR, BLOCK) {
        Ok(i) => i,
        Err(e) => {
            v["ok"] = json!(false);
            v["stage"] = json!("open");
            v["error"] = json!(e);
            return v;
        }
    };
    v["open_ms"] = json!(t0.elapsed().as_millis() as u64);
    v["latency_samples"] = json!((inst.latency_seconds() * f64::from(SR)).round() as i64);
    v["params"] = json!(inst.parameter_count());
    let blocks_per_s = SR as usize / BLOCK + 1;
    let zeros = vec![0f32; BLOCK];
    let (mut l, mut r) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
    let mut t = BlockTransport {
        playing: true,
        ..BlockTransport::default()
    };
    let nan = std::cell::Cell::new(0usize);
    let mut run =
        |inst: &mut Vst3Instance, input: &[f32], blocks: usize, t: &mut BlockTransport| {
            let mut peak = 0f32;
            let mut worst = Duration::ZERO;
            for _ in 0..blocks {
                inst.block_transport(t);
                let s = Instant::now();
                inst.process((input, input), (&mut l, &mut r));
                worst = worst.max(s.elapsed());
                t.sample_pos += BLOCK as i64;
                t.beat += BLOCK as f64 / f64::from(SR) * 2.0;
                for x in l.iter().chain(r.iter()) {
                    if !x.is_finite() {
                        nan.set(nan.get() + 1);
                    } else {
                        peak = peak.max(x.abs());
                    }
                }
            }
            (peak, worst)
        };
    let (silence_peak, w1) = run(&mut inst, &zeros, blocks_per_s, &mut t);
    let sine: Vec<f32> = (0..BLOCK)
        .map(|i| 0.25 * (i as f32 * 440.0 * std::f32::consts::TAU / SR as f32).sin())
        .collect();
    let (note_peak, w2) = if inst.is_instrument() {
        BlockProcessor::midi(&mut inst, 0x90, 60, 100, 0);
        let p = run(&mut inst, &zeros, blocks_per_s, &mut t);
        BlockProcessor::midi(&mut inst, 0x80, 60, 0, 0);
        let q = run(&mut inst, &zeros, blocks_per_s / 2, &mut t);
        (p.0.max(q.0), p.1.max(q.1))
    } else {
        run(&mut inst, &sine, blocks_per_s, &mut t)
    };
    v["silence_peak"] = json!(silence_peak);
    v["note_peak"] = json!(note_peak);
    v["nan_samples"] = json!(nan.get());
    v["process_errors"] = json!(inst.errors());
    v["worst_block_us"] = json!(w1.max(w2).as_micros() as u64);
    // Settings round trip: move the first free parameter, save, open a second instance,
    // restore, and compare.
    let h = inst.handle();
    // A continuous parameter if there is one (a stepped one snaps to its steps).
    let infos: Vec<(usize, i32)> = (0..h.parameter_count())
        .filter_map(|i| h.parameter_info(i).filter(|p| p.1).map(|p| (i, p.2)))
        .collect();
    let free = infos.iter().find(|p| p.1 == 0).or(infos.first()).copied();
    let mut round = json!({});
    if let Some((i, steps)) = free {
        let before = h.parameter(i).unwrap_or(0.0);
        let want = if before < 0.5 { 0.8 } else { 0.2 };
        h.set_parameter_shown(i, want);
        inst.set_parameter(i, want as f32);
        run(&mut inst, &zeros, 2, &mut t);
        let set = h.parameter(i).unwrap_or(f64::NAN);
        let state = h.state();
        let mut restored = f64::NAN;
        let mut same_bytes = false;
        if let Some(state) = &state {
            if let Ok(inst2) = Vst3Instance::new(bundle, &c.cid, SR, BLOCK) {
                let h2 = inst2.handle();
                let fresh = h2.parameter(i).unwrap_or(f64::NAN);
                if h2.set_state(state) {
                    restored = h2.parameter(i).unwrap_or(f64::NAN);
                    same_bytes = h2.state().as_deref() == Some(state.as_slice());
                }
                round["fresh"] = json!(fresh);
                drop(inst2);
            }
        }
        let title = h.parameter_info(i).map(|p| p.0).unwrap_or_default();
        round["param"] = json!(title);
        round["before"] = json!(before);
        round["set"] = json!(set);
        round["restored"] = json!(restored);
        round["state_bytes"] = json!(state.as_ref().map_or(0, Vec::len));
        round["same_bytes"] = json!(same_bytes);
        round["steps"] = json!(steps);
        // A stepped parameter comes back on one of its steps, near the value asked for.
        let tolerance = if steps > 0 {
            1.0 / f64::from(steps)
        } else {
            1e-4
        };
        round["ok"] =
            json!((restored - set).abs() <= tolerance + 1e-9 && (restored - before).abs() > 1e-6);
    } else {
        let state = h.state();
        round["state_bytes"] = json!(state.as_ref().map_or(0, Vec::len));
        round["ok"] = json!(state.is_some());
        round["param"] = json!(null);
    }
    v["state"] = round;
    if let Some(dir) = editor {
        let mut ed = json!({});
        match h.open_window(&label) {
            Ok(w) => {
                pump_main_loop(1.5);
                w.fit();
                pump_main_loop(0.3);
                let n = w.window_number();
                let _ = std::fs::create_dir_all(dir);
                let file = dir.join(format!("{}.png", label.replace(['/', ':', ' '], "_")));
                let shot = std::process::Command::new("screencapture")
                    .args(["-x", "-o", "-l", &n.to_string()])
                    .arg(&file)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                ed["window"] = json!(n);
                ed["shot"] = json!(shot.then(|| file.clone()));
                ed["open"] = json!(w.is_open());
                drop(w);
                pump_main_loop(0.3);
                ed["closed"] = json!(true);
            }
            Err(e) => ed["error"] = json!(e),
        }
        v["editor"] = ed;
    }
    drop(h);
    drop(inst);
    v["ok"] = json!(nan.get() == 0);
    v
}

/// `--vst3-check-all [--shots DIR] [--editor-for a,b,..] [--timeout SECS]`: scans every
/// bundle (fresh, in helpers), then checks each in its own helper. Prints the results and a
/// summary; touches neither the cache nor the quarantine.
pub fn run_check_all(args: &[String]) -> i32 {
    let mut shots: Option<String> = None;
    let mut editor_for: Vec<String> = Vec::new();
    let mut timeout = Duration::from_secs(120);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--shots" => shots = it.next().cloned(),
            "--editor-for" => {
                editor_for = it
                    .next()
                    .map(|s| s.split(',').map(|x| x.trim().to_lowercase()).collect())
                    .unwrap_or_default()
            }
            "--timeout" => {
                timeout = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .map_or(timeout, Duration::from_secs)
            }
            _ => {}
        }
    }
    let bundles = all_bundles();
    println!("{} bundles", bundles.len());
    let (scans, report) = rescan(&[], &bundles, &[], &scan_in_helper);
    println!(
        "scan: {} ok, {} failed, {} crashed or hung",
        report.ok,
        report.failed.len(),
        report.quarantined.len()
    );
    for (p, e) in &report.failed {
        println!("  failed: {p}: {e}");
    }
    for (p, e) in &report.quarantined {
        println!("  crashed/hung: {p}: {e}");
    }
    let classes: usize = scans.iter().map(|s| s.classes.len()).sum();
    println!("{classes} classes in {} bundles", scans.len());
    let (mut ok, mut bad) = (0, Vec::new());
    let jobs: Vec<&BundleScan> = scans.iter().filter(|s| !s.classes.is_empty()).collect();
    // Bundles with a window to open are checked one at a time, after the others (which
    // run a few at once).
    let wants_editor = |s: &BundleScan| {
        let stem = Path::new(&s.path)
            .file_stem()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        shots.is_some() && editor_for.iter().any(|e| stem.contains(e.as_str()))
    };
    let check = |s: &BundleScan| {
        let mut a: Vec<std::ffi::OsString> = vec!["--vst3-check".into()];
        if let (Some(dir), true) = (&shots, wants_editor(s)) {
            a.push("--editor".into());
            a.push(dir.into());
        }
        a.push(s.path.clone().into());
        let refs: Vec<&std::ffi::OsStr> = a.iter().map(|x| x.as_os_str()).collect();
        run_helper(&refs, timeout)
    };
    let results: Mutex<Vec<(usize, Result<HelperRun, Outcome>)>> = Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let plain: Vec<usize> = (0..jobs.len())
        .filter(|i| !wants_editor(jobs[*i]))
        .collect();
    std::thread::scope(|sc| {
        for _ in 0..4 {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(&i) = plain.get(k) else {
                    break;
                };
                let r = check(jobs[i]);
                if let Ok(mut all) = results.lock() {
                    all.push((i, r));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_default();
    for i in (0..jobs.len()).filter(|i| wants_editor(jobs[*i])) {
        results.push((i, check(jobs[i])));
    }
    results.sort_by_key(|r| r.0);
    for (i, r) in results {
        let s = jobs[i];
        match r {
            Ok((crash, text)) => {
                let lines = marked_lines(&text);
                if let Some(c) = crash {
                    bad.push(format!(
                        "{}: {c} after {} of {} classes",
                        s.path,
                        lines.len(),
                        s.classes.len()
                    ));
                } else if lines.len() < s.classes.len() {
                    bad.push(format!(
                        "{}: ended early ({} of {} classes)",
                        s.path,
                        lines.len(),
                        s.classes.len()
                    ));
                }
                for v in lines {
                    println!("{v}");
                    if v.get("ok").and_then(Value::as_bool) == Some(true) {
                        ok += 1;
                    } else {
                        bad.push(format!(
                            "{}: {}",
                            v.get("name")
                                .or(v.get("bundle"))
                                .cloned()
                                .unwrap_or_default(),
                            v.get("error")
                                .cloned()
                                .unwrap_or(json!("NaN in the output"))
                        ));
                    }
                }
            }
            Err(o) => bad.push(format!("{}: {o:?}", s.path)),
        }
    }
    println!("check: {ok} classes ok, {} problems", bad.len());
    for b in &bad {
        println!("  {b}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, path: &str) -> Vst3Entry {
        Vst3Entry {
            id: id.into(),
            path: path.into(),
            name: "Pro-Q 4".into(),
            vendor: "FabFilter".into(),
            instrument: false,
            sub_categories: "Fx|EQ".into(),
            version: "4.0".into(),
        }
    }

    #[test]
    fn cache_round_trips() {
        let id = format!("vst3:{}", "AB".repeat(16));
        let scans = vec![
            BundleScan {
                path: "/x/A.vst3".into(),
                mtime: 42,
                classes: vec![entry(&id, "/x/A.vst3")],
                error: None,
            },
            BundleScan {
                path: "/x/B.vst3".into(),
                mtime: 7,
                classes: Vec::new(),
                error: Some("no factory".into()),
            },
        ];
        assert_eq!(parse_cache(&cache_text(&scans)), scans);
        assert!(parse_cache("{\"version\": 99, \"bundles\": []}").is_empty());
        assert!(parse_cache("not json").is_empty());
    }

    #[test]
    fn rescan_reuses_matching_entries_and_quarantines_crashes() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let mk = |n: &str| {
            let p = dir.path().join(n);
            std::fs::create_dir_all(p.join("Contents/MacOS")).unwrap_or_else(|e| panic!("{e}"));
            p
        };
        let (a, b, c, d) = (mk("A.vst3"), mk("B.vst3"), mk("C.vst3"), mk("D.vst3"));
        let s = |p: &PathBuf| p.to_string_lossy().to_string();
        let id = format!("vst3:{}", "01".repeat(16));
        // A is cached at its current time; B is cached at an old time (it changed).
        let cache = vec![
            BundleScan {
                path: s(&a),
                mtime: bundle_mtime(&a),
                classes: vec![entry(&id, &s(&a))],
                error: None,
            },
            BundleScan {
                path: s(&b),
                mtime: 1,
                classes: Vec::new(),
                error: None,
            },
            BundleScan {
                path: "/gone.vst3".into(),
                mtime: 1,
                classes: Vec::new(),
                error: None,
            },
        ];
        let scanned = Mutex::new(Vec::new());
        let scan = |p: &Path| {
            scanned
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .push(p.to_path_buf());
            match p.file_name().and_then(|n| n.to_str()) {
                Some("B.vst3") => Outcome::Classes(vec![entry(&id, &p.to_string_lossy())]),
                Some("C.vst3") => Outcome::Crashed("signal 11".into()),
                _ => Outcome::TimedOut,
            }
        };
        let quarantined = vec![quarantine_id(&s(&d), bundle_mtime(&d))];
        let bundles = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let (new, report) = rescan(&cache, &bundles, &quarantined, &scan);
        let mut seen = scanned.into_inner().unwrap_or_default();
        seen.sort();
        assert_eq!(
            seen,
            vec![b.clone(), c.clone()],
            "only changed, new bundles are scanned"
        );
        assert_eq!(report.cached, 1);
        assert_eq!(report.ok, 1);
        assert_eq!(report.skipped, vec![s(&d)]);
        assert_eq!(report.quarantined.len(), 1);
        assert_eq!(report.quarantined[0].0, s(&c));
        let paths: Vec<&str> = new.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![s(&a), s(&b)],
            "gone and crashed bundles are dropped"
        );
    }

    #[test]
    fn helper_answer_lines_are_found_among_plugin_noise() {
        let text = format!("plug-in says hi\n{MARK}{{\"classes\": []}}\nbye\n");
        assert_eq!(marked_lines(&text), vec![json!({"classes": []})]);
    }
}
