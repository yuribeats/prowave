//! Window > Get Free Plug-ins: a list of free synths and effects, and an Install key that
//! fetches and installs the checked ones.
//!
//! The work is done by one shell script compiled into the program
//! (`assets/free-plugins/install-free-plugins.sh`). The list is in the script too: nothing is
//! read from a Pro Wave server. The script is written to a private temporary folder and run
//! with `/bin/sh`, off the interface thread; what it prints comes back over a channel.
//!
//! The script's contract (all this module relies on):
//!
//! - `--list` prints one plug-in a line, TAB-separated, without a header:
//!   `id kind name maker mode tag page_url description`. `kind` is `synth` or `effect`; `mode`
//!   is `auto` (the script can install it) or `manual` (it has to be fetched from `page_url`).
//! - `--gui --only id1,id2` installs those. The script asks for the administrator password
//!   itself when it needs it; Pro Wave never does. `--dry-run` installs nothing.
//! - While it installs it prints `@@ start <id>`, `@@ ok <id>`, `@@ fail <id> <reason>`,
//!   `@@ manual <id> <url>` and, last, `@@ done ok=<n> fail=<n> manual=<n>`. Any other line is
//!   log text. It ends with 0 when it ran to the end, failures included.
//! - Cancel ends the script's whole process group.
use crate::hw::{self, Cap};
use crate::i18n::tr;
use crate::midi_monitor::hw_utilities as hu;
use crate::theme::{self, Face};
use egui::{pos2, vec2, Align2, Rect, Sense};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// The installer, compiled in.
pub const SCRIPT: &str = include_str!("../assets/free-plugins/install-free-plugins.sh");

/// Lines the log keeps.
const LOG_LINES: usize = 200;

// ---------------------------------------------------------------- the list

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Synth,
    Effect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The script fetches and installs it.
    Auto,
    /// It has to be fetched by hand from its page.
    Manual,
}

/// One plug-in of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub maker: String,
    pub mode: Mode,
    /// Empty, or a short note such as "Needs account".
    pub tag: String,
    pub page_url: String,
    pub description: String,
}

/// One line of `--list`; `None` for a line that is not a plug-in (too few fields, an unknown
/// kind or mode, no id).
pub fn parse_entry(line: &str) -> Option<Entry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut f = line.splitn(8, '\t');
    let id = f.next()?.trim();
    let kind = match f.next()?.trim() {
        "synth" => Kind::Synth,
        "effect" => Kind::Effect,
        _ => return None,
    };
    let name = f.next()?.trim();
    let maker = f.next()?.trim();
    let mode = match f.next()?.trim() {
        "auto" => Mode::Auto,
        "manual" => Mode::Manual,
        _ => return None,
    };
    let tag = f.next()?.trim();
    let page_url = f.next()?.trim();
    let description = f.next()?.trim();
    // An id goes into a comma-separated argument and comes back in space-separated lines.
    if id.is_empty() || id.contains([',', ' ']) || name.is_empty() {
        return None;
    }
    Some(Entry {
        id: id.into(),
        kind,
        name: name.into(),
        maker: maker.into(),
        mode,
        tag: tag.into(),
        page_url: page_url.into(),
        description: description.into(),
    })
}

/// What `--list` printed, as plug-ins (lines that are not plug-ins are left out; of two lines
/// with one id the first counts).
pub fn parse_list(text: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for e in text.lines().filter_map(parse_entry) {
        if !out.iter().any(|x| x.id == e.id) {
            out.push(e);
        }
    }
    out
}

/// Whether `e` shows for `search` (its name, maker, description or tag holds it).
fn matches(e: &Entry, search: &str) -> bool {
    let s = search.trim().to_lowercase();
    s.is_empty()
        || [&e.name, &e.maker, &e.description, &e.tag]
            .iter()
            .any(|t| t.to_lowercase().contains(&s))
}

// ---------------------------------------------------------------- what the script says

/// One line the script printed while installing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Start(String),
    Ok(String),
    Fail {
        id: String,
        reason: String,
    },
    Manual {
        id: String,
        url: String,
    },
    Done {
        ok: u32,
        fail: u32,
        manual: u32,
    },
    /// Any other line.
    Log(String),
}

/// The count after `key=` in a `@@ done` line.
fn count(rest: &str, key: &str) -> Option<u32> {
    rest.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
        .and_then(|n| n.parse().ok())
}

/// `line` as an event. A `@@` line that does not follow the contract is log text.
pub fn parse_line(line: &str) -> Event {
    let line = line.trim_end_matches(['\r', '\n']);
    let log = || Event::Log(line.to_string());
    let Some(rest) = line.strip_prefix("@@ ") else {
        return log();
    };
    let (word, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    let rest = rest.trim();
    let (id, more) = rest.split_once(' ').unwrap_or((rest, ""));
    let more = more.trim();
    match word {
        "done" => match (
            count(rest, "ok"),
            count(rest, "fail"),
            count(rest, "manual"),
        ) {
            (Some(ok), Some(fail), Some(manual)) => Event::Done { ok, fail, manual },
            _ => log(),
        },
        _ if id.is_empty() => log(),
        "start" => Event::Start(id.into()),
        "ok" => Event::Ok(id.into()),
        "fail" => Event::Fail {
            id: id.into(),
            reason: more.into(),
        },
        "manual" => Event::Manual {
            id: id.into(),
            url: more.into(),
        },
        _ => log(),
    }
}

/// Where one plug-in stands in the run.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum RowState {
    #[default]
    Idle,
    Working,
    Ok,
    /// With the script's reason.
    Failed(String),
    /// The script could not install it: it is to be fetched from this page.
    Manual(String),
}

/// One run of the installer, as the window shows it.
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub running: bool,
    pub states: HashMap<String, RowState>,
    /// The latest lines, oldest first.
    pub log: VecDeque<String>,
    /// The script's own count, once it printed it.
    pub done: Option<(u32, u32, u32)>,
    /// Plug-ins reported installed in this run.
    pub installed: usize,
}

impl Run {
    /// A run starts: the states and the log of the one before are dropped.
    pub fn begin(&mut self) {
        *self = Run {
            running: true,
            ..Run::default()
        };
    }

    pub fn state(&self, id: &str) -> &RowState {
        static IDLE: RowState = RowState::Idle;
        self.states.get(id).unwrap_or(&IDLE)
    }

    pub fn push_log(&mut self, line: &str) {
        let line = line.trim_end();
        if line.is_empty() {
            return;
        }
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line.to_string());
    }

    /// A line the script printed.
    pub fn line(&mut self, line: &str) {
        let ev = parse_line(line);
        match &ev {
            Event::Log(text) => self.push_log(text),
            // The marker is the script's; the rest reads as it is.
            _ => self.push_log(line.trim_start_matches("@@ ")),
        }
        self.apply(ev);
    }

    pub fn apply(&mut self, ev: Event) {
        match ev {
            Event::Start(id) => {
                self.states.insert(id, RowState::Working);
            }
            Event::Ok(id) => {
                self.installed += 1;
                self.states.insert(id, RowState::Ok);
            }
            Event::Fail { id, reason } => {
                self.states.insert(id, RowState::Failed(reason));
            }
            Event::Manual { id, url } => {
                self.states.insert(id, RowState::Manual(url));
            }
            Event::Done { ok, fail, manual } => self.done = Some((ok, fail, manual)),
            Event::Log(_) => {}
        }
    }

    /// The script ended (`code` is `None` when a signal ended it). A plug-in it was still
    /// working on is no longer being worked on.
    pub fn exited(&mut self, code: Option<i32>) {
        self.running = false;
        for s in self.states.values_mut() {
            if *s == RowState::Working {
                *s = RowState::Idle;
            }
        }
        match code {
            Some(0) => {}
            Some(c) => self.push_log(&format!("exit {c}")),
            None => self.push_log("stopped"),
        }
    }
}

// ---------------------------------------------------------------- running the script

/// What the script's threads send back.
#[derive(Debug, PartialEq, Eq)]
pub enum Msg {
    /// A line of its output.
    Out(String),
    /// A line of its error output.
    Err(String),
    /// It ended, with this code (`None`: ended by a signal).
    Exit(Option<i32>),
}

/// The script, running.
#[cfg_attr(not(unix), allow(dead_code))]
pub struct Job {
    pub rx: Receiver<Msg>,
    /// The script's process id, which is its process group's too.
    pid: u32,
    ended: Arc<AtomicBool>,
}

impl Job {
    /// Ends the script and everything it started: TERM to its process group now, KILL two
    /// seconds later if it is still there.
    pub fn cancel(&self) {
        if self.ended.load(Ordering::Acquire) {
            return;
        }
        signal_group(self.pid, "TERM");
        let (pid, ended) = (self.pid, self.ended.clone());
        let _ = std::thread::Builder::new()
            .name("free-plugins-kill".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if !ended.load(Ordering::Acquire) {
                    signal_group(pid, "KILL");
                }
            });
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn signal_group(pid: u32, signal: &str) {
    let _ = std::process::Command::new("/bin/kill")
        .args(["-s", signal, "--", &format!("-{pid}")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// The arguments that install `ids`.
pub fn install_args(ids: &[String], dry_run: bool) -> Vec<String> {
    let mut args = vec!["--gui".to_string()];
    if dry_run {
        args.push("--dry-run".into());
    }
    args.push("--only".into());
    args.push(ids.join(","));
    args
}

/// A new folder only this user can enter, in the temporary folder.
#[cfg(unix)]
fn private_dir() -> std::io::Result<std::path::PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let base = std::env::temp_dir();
    let mut last = None;
    for _ in 0..16 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = base.join(format!(
            "prowave-free-plugins-{}-{}-{nanos:09}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // `create` fails on a folder that is already there, so the folder is ours and new.
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no temporary folder")))
}

/// Sends what `from` prints, a line at a time (bytes that are not UTF-8 are replaced).
#[cfg(unix)]
fn pump_lines(
    from: impl std::io::Read,
    tx: &std::sync::mpsc::Sender<Msg>,
    make: fn(String) -> Msg,
    wake: &(dyn Fn() + Send + Sync),
) {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(from);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\r', '\n']);
                if tx.send(make(line.to_string())).is_err() {
                    break;
                }
                wake();
            }
        }
    }
}

/// Writes `script` to a new private folder (the file for its owner only) and runs it with
/// `/bin/sh` and `args` in a process group of its own. Its lines and its end come back through
/// the job; `wake` is called after each (to repaint).
#[cfg(unix)]
pub fn spawn(
    script: &str,
    args: &[String],
    wake: impl Fn() + Send + Sync + 'static,
) -> std::io::Result<Job> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let dir = private_dir()?;
    let path = dir.join("install-free-plugins.sh");
    let started = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)?;
        f.write_all(script.as_bytes())?;
        drop(f);
        Command::new("/bin/sh")
            .arg(&path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
    })();
    let mut child = match started {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let ended = Arc::new(AtomicBool::new(false));
    let pid = child.id();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
    let errors = child.stderr.take().map(|err| {
        let (tx, wake) = (tx.clone(), wake.clone());
        std::thread::Builder::new()
            .name("free-plugins-err".into())
            .spawn(move || pump_lines(err, &tx, Msg::Err, &*wake))
    });
    let out = child.stdout.take();
    let ended_in = ended.clone();
    let reader = std::thread::Builder::new()
        .name("free-plugins".into())
        .spawn(move || {
            if let Some(out) = out {
                pump_lines(out, &tx, Msg::Out, &*wake);
            }
            let code = child.wait().ok().and_then(|s| s.code());
            ended_in.store(true, Ordering::Release);
            // Everything it printed goes ahead of its end.
            if let Some(Ok(t)) = errors {
                let _ = t.join();
            }
            let _ = std::fs::remove_dir_all(&dir);
            let _ = tx.send(Msg::Exit(code));
            wake();
        });
    if let Err(e) = reader {
        signal_group(pid, "KILL");
        return Err(e);
    }
    Ok(Job { rx, pid, ended })
}

#[cfg(not(unix))]
pub fn spawn(
    _script: &str,
    _args: &[String],
    _wake: impl Fn() + Send + Sync + 'static,
) -> std::io::Result<Job> {
    Err(std::io::Error::other(
        "the plug-in installer runs on macOS only",
    ))
}

/// Opens `url` in the browser (web addresses only).
fn open_page(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    let mut c = if cfg!(target_os = "macos") {
        std::process::Command::new("/usr/bin/open")
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let _ = c.arg(url).spawn();
}

// ---------------------------------------------------------------- the window

/// Window > Get Free Plug-ins.
#[derive(Default)]
pub struct FreePlugins {
    pub open: bool,
    /// The list, once the script gave it.
    catalog: Option<Vec<Entry>>,
    /// The script, asked for its list.
    listing: Option<Job>,
    list_text: String,
    /// Asking for the list failed: not asked again until the window is opened again.
    list_failed: bool,
    checked: HashSet<String>,
    search: String,
    install: Option<Job>,
    /// The run was told to stop (Cancel, the window closed, the program quitting).
    stopping: bool,
    run: Run,
}

impl FreePlugins {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.list_failed = false;
        }
    }

    /// Takes what the script sent since the last frame. Returns true when a run ended that
    /// installed something (the plug-in lists are to be read again).
    fn poll(&mut self) -> bool {
        if let Some(job) = &self.listing {
            let mut exit = None;
            for m in job.rx.try_iter() {
                match m {
                    Msg::Out(l) => {
                        self.list_text.push_str(&l);
                        self.list_text.push('\n');
                    }
                    Msg::Err(l) => self.run.push_log(&l),
                    Msg::Exit(code) => exit = Some(code),
                }
            }
            if let Some(code) = exit {
                self.listing = None;
                let list = parse_list(&std::mem::take(&mut self.list_text));
                if code == Some(0) && !list.is_empty() {
                    self.catalog = Some(list);
                } else {
                    self.list_failed = true;
                    self.run.exited(code);
                }
            }
        }
        let mut installed = false;
        if let Some(job) = &self.install {
            let mut exit = None;
            for m in job.rx.try_iter() {
                match m {
                    Msg::Out(l) => self.run.line(&l),
                    Msg::Err(l) => self.run.push_log(&l),
                    Msg::Exit(code) => exit = Some(code),
                }
            }
            if let Some(code) = exit {
                self.install = None;
                // A run that was told to stop ends with the script's own code for that.
                let code = if self.stopping { None } else { code };
                self.stopping = false;
                self.run.exited(code);
                // What is installed needs no second run.
                let run = &self.run;
                self.checked.retain(|id| *run.state(id) != RowState::Ok);
                installed = self.run.installed > 0 && !dry_run();
            }
        }
        installed
    }

    /// Asks the script for its list (once; again after the window was closed on a failure).
    fn load(&mut self, ctx: &egui::Context) {
        if self.catalog.is_some() || self.listing.is_some() || self.list_failed {
            return;
        }
        let ctx = ctx.clone();
        match spawn(SCRIPT, &["--list".to_string()], move || {
            ctx.request_repaint()
        }) {
            Ok(job) => self.listing = Some(job),
            Err(e) => {
                self.list_failed = true;
                self.run.push_log(&e.to_string());
            }
        }
    }

    /// Runs the script for the checked plug-ins, in the list's order.
    fn start(&mut self, ctx: &egui::Context) {
        if self.install.is_some() {
            return;
        }
        let ids = self.chosen();
        if ids.is_empty() {
            return;
        }
        self.run.begin();
        self.stopping = false;
        let ctx = ctx.clone();
        match spawn(SCRIPT, &install_args(&ids, dry_run()), move || {
            ctx.request_repaint()
        }) {
            Ok(job) => self.install = Some(job),
            Err(e) => {
                self.run.push_log(&e.to_string());
                self.run.running = false;
            }
        }
    }

    /// Stops a run: ends the script and everything it started.
    pub fn stop(&mut self) {
        // Once: it is called every frame while the window is closed.
        if self.stopping {
            return;
        }
        if let Some(job) = &self.install {
            self.stopping = true;
            job.cancel();
        }
    }

    /// The checked plug-ins the script can install, in the list's order.
    fn chosen(&self) -> Vec<String> {
        self.catalog
            .iter()
            .flatten()
            .filter(|e| e.mode == Mode::Auto && self.checked.contains(&e.id))
            .map(|e| e.id.clone())
            .collect()
    }
}

/// `PROWAVE_FREE_PLUGINS_DRY_RUN=1`: the script is run with `--dry-run` (it installs nothing),
/// for trying the window out.
fn dry_run() -> bool {
    std::env::var_os("PROWAVE_FREE_PLUGINS_DRY_RUN").is_some_and(|v| v == "1")
}

/// The window, and its script's messages. A run does not outlive the window: closing it, or
/// quitting the program, stops the script (as dropping the job does, see [`Job`]).
pub fn show(ctx: &egui::Context, s: &mut crate::UiState) {
    let f = &mut s.free_plugins;
    if f.poll() {
        // New plug-ins: the VST 3 folders are scanned again; the main loop then reads the
        // Audio Unit and VST 3 lists again (`vst3_scan::take_update`).
        crate::vst3_scan::rescan_now();
    }
    if !f.open || ctx.input(|i| i.viewport().close_requested()) {
        f.stop();
    }
    if !f.open {
        return;
    }
    f.load(ctx);
    if f.run.running || f.listing.is_some() {
        // The working LEDs blink.
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
    let mut action = None;
    let (open, _) = hu::window(
        ctx,
        egui::Id::unique("free_plugins"),
        tr("Get Free Plug-ins"),
        "",
        860.0,
        Some((vec2(860.0, 640.0), vec2(700.0, 420.0))),
        |ui| action = panel(ui, f),
    );
    match action {
        Some(Action::Install) => f.start(ctx),
        Some(Action::Cancel) => f.stop(),
        None => {}
    }
    f.open = open;
    if !open {
        f.stop();
    }
}

enum Action {
    Install,
    Cancel,
}

/// A row of the list: a section's head or a plug-in (its place in the catalog).
enum Item {
    /// With how many plug-ins it shows, and whether any of them can be checked.
    Head(Kind, usize, bool),
    Plugin(usize),
}

const ROW_H: f32 = 34.0;
const LOG_H: f32 = 116.0;

/// The inside of the window: the search field and the Install key, the list, the log.
fn panel(ui: &mut egui::Ui, f: &mut FreePlugins) -> Option<Action> {
    let id = egui::Id::unique("free_plugins_panel");
    let w = ui.available_width();
    let g = ui.painter().clone();
    let running = f.run.running;
    let mut action = None;

    // Search, and Install (Cancel while it runs).
    let (row, _) = ui.allocate_exact_size(vec2(w, 38.0), Sense::hover());
    let label = tr("Search");
    let lw = hw::silk_width(&g, label, 11.0).min(90.0);
    hw::silk(
        &g,
        pos2(row.left(), row.top() + 15.0),
        Align2::LEFT_CENTER,
        label,
        11.0,
        theme::INK2,
    );
    let install_label = tr("Install");
    let cancel_label = tr("Cancel");
    let kw = hu::key_width(&g, install_label, 11.0, false)
        .max(hu::key_width(&g, cancel_label, 11.0, false))
        .max(110.0);
    let key_r = Rect::from_min_size(pos2(row.right() - kw, row.top() + 1.0), vec2(kw, 30.0));
    let field = Rect::from_min_max(
        pos2(row.left() + lw + 10.0, row.top() + 3.0),
        pos2(
            (row.left() + lw + 10.0 + 320.0).min(key_r.left() - 16.0),
            row.top() + 27.0,
        ),
    );
    hw::paint_field(&g, field, "", false, false);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink2(vec2(4.0, 3.0))));
    crate::hw_sampler::fields(&mut child, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut f.search)
                .id(id.with("search"))
                .frame(egui::Frame::NONE)
                .font(theme::font(Face::Bold, 11.0))
                .text_color(theme::ON_DARK)
                .desired_width(field.width() - 8.0),
        );
    });
    if running {
        if hu::key(
            ui,
            key_r,
            id.with("cancel"),
            Cap::Bone,
            false,
            None,
            cancel_label,
            true,
        )
        .clicked()
        {
            action = Some(Action::Cancel);
        }
    } else {
        let any = !f.chosen().is_empty();
        if hu::key(
            ui,
            key_r,
            id.with("install"),
            Cap::Dark,
            false,
            None,
            install_label,
            any,
        )
        .clicked()
        {
            action = Some(Action::Install);
        }
    }

    // The list: the synths, then the effects.
    let empty = Vec::new();
    let catalog = f.catalog.as_ref().unwrap_or(&empty);
    let mut items = Vec::new();
    for kind in [Kind::Synth, Kind::Effect] {
        let rows: Vec<usize> = catalog
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == kind && matches(e, &f.search))
            .map(|(i, _)| i)
            .collect();
        let any_auto = rows.iter().any(|i| catalog[*i].mode == Mode::Auto);
        items.push(Item::Head(kind, rows.len(), any_auto));
        items.extend(rows.into_iter().map(Item::Plugin));
    }
    let list_h = (ui.available_height() - LOG_H - 10.0).max(ROW_H * 3.0);
    let blink = (ui.input(|i| i.time) * 2.0).fract() < 0.5;
    let checked = &mut f.checked;
    let run = &f.run;
    let mut pick: Option<(Kind, bool)> = None;
    egui::ScrollArea::vertical()
        .id_salt("free_plugins_list")
        .max_height(list_h)
        .min_scrolled_height(list_h)
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, items.len(), |ui, range| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for item in &items[range] {
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                match item {
                    Item::Head(kind, n, any_auto) => {
                        if let Some(all) = head(ui, r, *kind, *n, !running && *any_auto) {
                            pick = Some((*kind, all));
                        }
                    }
                    Item::Plugin(i) => {
                        let e = &catalog[*i];
                        let was = checked.contains(&e.id);
                        if plugin_row(ui, r, e, run.state(&e.id), was, !running, blink) {
                            if was {
                                checked.remove(&e.id);
                            } else {
                                checked.insert(e.id.clone());
                            }
                        }
                    }
                }
            }
        });
    if let Some((kind, all)) = pick {
        for e in catalog
            .iter()
            .filter(|e| e.kind == kind && e.mode == Mode::Auto && matches(e, &f.search))
        {
            if all {
                f.checked.insert(e.id.clone());
            } else {
                f.checked.remove(&e.id);
            }
        }
    }

    // The log: the script's latest lines, the newest at the bottom.
    ui.add_space(10.0);
    let h = ui.available_height().max(LOG_H);
    let (lcd_r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let glass = hw::lcd(&g, lcd_r);
    let size = 10.5;
    let pitch = 16.0;
    let lines = ((glass.height() - 10.0) / pitch).floor().max(1.0) as usize;
    let first = f.run.log.len().saturating_sub(lines);
    let g = g.with_clip_rect(glass);
    for (k, line) in f.run.log.iter().skip(first).enumerate() {
        let shown = hw::fit(
            &g,
            &hu_printable(&g, line, size),
            Face::Lcd,
            size,
            glass.width() - 20.0,
        );
        hw::text(
            &g,
            pos2(
                glass.left() + 10.0,
                glass.top() + 6.0 + pitch * (k as f32 + 0.5),
            ),
            Align2::LEFT_CENTER,
            &shown,
            Face::Lcd,
            size,
            theme::LCD_INK,
        );
    }
    action
}

/// `line` in capitals, without the characters the LCD face cannot draw.
fn hu_printable(g: &egui::Painter, line: &str, size: f32) -> String {
    crate::spectrogram_view::hw_vocal::printable(g, &line.to_uppercase(), Face::Lcd, size)
}

/// A section's head: its name and how many plug-ins it shows in a bracket, then the All and
/// None keys. Returns `Some(true)` for All, `Some(false)` for None.
fn head(ui: &egui::Ui, r: Rect, kind: Kind, n: usize, enabled: bool) -> Option<bool> {
    let g = ui.painter().clone();
    let (name, salt) = match kind {
        Kind::Synth => (tr("Synths"), "synths"),
        Kind::Effect => (tr("Effects"), "effects"),
    };
    let labels = [tr("All"), tr("None")];
    let widths = labels.map(|l| hu::key_width(&g, l, 10.0, false).max(58.0));
    let total = widths[0] + widths[1] + 6.0;
    let mut x = r.right() - total - 4.0;
    hw::bracket(
        &g,
        r.left(),
        x - 12.0,
        r.center().y + 2.0,
        &format!("{name}  {n}"),
    );
    let mut out = None;
    for (i, (label, kw)) in labels.iter().zip(widths).enumerate() {
        let kr = Rect::from_min_size(pos2(x, r.center().y - 9.0), vec2(kw, 22.0));
        x += kw + 6.0;
        if hu::key(
            ui,
            kr,
            egui::Id::unique(("free_plugins_head", salt, i)),
            Cap::Bone,
            false,
            None,
            label,
            enabled,
        )
        .clicked()
        {
            out = Some(i == 0);
        }
    }
    out
}

/// One plug-in: its check key (none on one to be fetched by hand), name, tag, maker and
/// description, then where it stands: the LED off, blinking while the script works on it and
/// lit once it is installed, a FAILED chip with the reason in place of the description, or the
/// Open Page key. Returns true when the check key was clicked.
fn plugin_row(
    ui: &egui::Ui,
    r: Rect,
    e: &Entry,
    state: &RowState,
    checked: bool,
    enabled: bool,
    blink: bool,
) -> bool {
    let g = ui.painter().clone();
    let cy = r.center().y - 1.0;
    let id = egui::Id::unique(("free_plugins_row", &e.id));
    let mut clicked = false;

    // What the right end holds.
    let page = match (state, e.mode) {
        (RowState::Manual(url), _) => Some(url.as_str()),
        (_, Mode::Manual) => Some(e.page_url.as_str()),
        _ => None,
    };
    let page_label = tr("Open Page");
    let right_w = hu::key_width(&g, page_label, 10.0, false).max(96.0);
    let right = Rect::from_min_max(
        pos2(r.right() - right_w - 4.0, r.top()),
        pos2(r.right() - 4.0, r.bottom()),
    );

    if e.mode == Mode::Auto {
        let kr = Rect::from_min_size(pos2(r.left() + 2.0, cy - 11.0), vec2(34.0, 22.0));
        if enabled {
            let (resp, face) = hw::key(
                ui,
                &g,
                kr,
                id.with("check"),
                Cap::Bone,
                checked,
                None,
                hw::Legend::None,
            );
            hw::led_sized(&g, face.center(), 3.0, checked);
            clicked = resp.clicked();
        } else {
            let mut faded = g.clone();
            faded.set_opacity(0.42);
            let face = hw::paint_key(&faded, kr, Cap::Bone, checked, false);
            hw::led_sized(&faded, face.center(), 3.0, checked);
        }
    }

    // Name (with its tag), maker, description.
    let x0 = r.left() + 48.0;
    let room = right.left() - 12.0 - x0;
    let name_w = (room * 0.36).clamp(150.0, 300.0);
    let maker_w = (room * 0.19).clamp(100.0, 170.0);
    let tag = e.tag.to_uppercase();
    let tag_w = if tag.is_empty() {
        0.0
    } else {
        hw::galley(&g, &tag, Face::Bold, 8.5, theme::INK2).size().x + 8.0 + 7.0
    };
    let shown = hw::fit(&g, &e.name, Face::Bold, 11.0, name_w - tag_w - 8.0);
    let nr = hw::text(
        &g,
        pos2(x0, cy),
        Align2::LEFT_CENTER,
        &shown,
        Face::Bold,
        11.0,
        theme::INK,
    );
    if !tag.is_empty() {
        hu::tag(&g, pos2(nr.right() + 7.0, cy), &tag, 8.5);
    }
    let maker_x = x0 + name_w;
    let shown = hw::fit(&g, &e.maker, Face::Body, 10.5, maker_w - 10.0);
    hw::text(
        &g,
        pos2(maker_x, cy),
        Align2::LEFT_CENTER,
        &shown,
        Face::Body,
        10.5,
        theme::INK2,
    );
    let text_x = maker_x + maker_w;
    let (text, ink) = match state {
        RowState::Failed(reason) if !reason.is_empty() => (reason.as_str(), theme::INK),
        _ => (e.description.as_str(), theme::PRINT),
    };
    let shown = hw::fit(&g, text, Face::Body, 10.5, right.left() - 12.0 - text_x);
    hw::text(
        &g,
        pos2(text_x, cy),
        Align2::LEFT_CENTER,
        &shown,
        Face::Body,
        10.5,
        ink,
    );

    // Where it stands.
    if let Some(url) = page {
        let kr = Rect::from_min_size(pos2(right.left(), cy - 11.0), vec2(right.width(), 22.0));
        if hu::key(
            ui,
            kr,
            id.with("page"),
            Cap::Bone,
            false,
            None,
            page_label,
            true,
        )
        .clicked()
        {
            open_page(url);
        }
    } else {
        let lit = match state {
            RowState::Working => blink,
            RowState::Ok => true,
            _ => false,
        };
        let led = pos2(right.right() - 10.0, cy);
        hw::led_sized(&g, led, 3.5, lit);
        if matches!(state, RowState::Failed(_)) {
            let label = tr("Failed").to_uppercase();
            let cw = hw::galley(&g, &label, Face::Bold, 8.5, theme::INK).size().x + 10.0;
            hu::chip(&g, pos2(led.x - 12.0 - cw, cy), &label, 8.5, false);
        }
    }
    hw::seam_h(&g, r.x_range(), r.bottom() - 1.0);
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// The script the tests run: it follows the contract, fetches nothing and installs
    /// nothing. It stays as it is when the real installer replaces `SCRIPT`.
    const STUB: &str = include_str!("../assets/free-plugins/contract-stub.sh");

    /// A list for the pictures and the state tests.
    fn sample() -> Vec<Entry> {
        let rows: [(&str, &str, &str, &str, &str, &str, &str); 14] = [
            (
                "surge-xt",
                "synth",
                "Surge XT",
                "Surge Synth Team",
                "auto",
                "",
                "Hybrid synthesizer with wavetable, FM and classic oscillators",
            ),
            (
                "dexed",
                "synth",
                "Dexed",
                "Digital Suburban",
                "auto",
                "",
                "Six-operator FM synth that reads DX7 patches",
            ),
            (
                "vital",
                "synth",
                "Vital",
                "Matt Tytel",
                "manual",
                "Needs account",
                "Spectral warping wavetable synth",
            ),
            (
                "odin2",
                "synth",
                "Odin 2",
                "TheWaveWarden",
                "auto",
                "",
                "24-voice semi-modular synth",
            ),
            (
                "ob-xd",
                "synth",
                "OB-Xd",
                "discoDSP",
                "auto",
                "Free edition",
                "Analog polysynth modelled on the OB-X and OB-Xa",
            ),
            (
                "helm",
                "synth",
                "Helm",
                "Matt Tytel",
                "auto",
                "",
                "Polyphonic synth with a step sequencer and formant filter",
            ),
            (
                "cardinal",
                "synth",
                "Cardinal",
                "DISTRHO",
                "auto",
                "",
                "Modular synthesizer rack as a plug-in",
            ),
            (
                "valhalla-supermassive",
                "effect",
                "Valhalla Supermassive",
                "Valhalla DSP",
                "manual",
                "Needs account",
                "Huge reverbs and delays",
            ),
            (
                "dragonfly-reverb",
                "effect",
                "Dragonfly Reverb",
                "Michael Willis",
                "auto",
                "",
                "Hall, room, plate and early-reflection reverbs",
            ),
            (
                "airwindows",
                "effect",
                "Airwindows Consolidated",
                "Airwindows",
                "auto",
                "",
                "Several hundred effects in one plug-in",
            ),
            (
                "chowtape",
                "effect",
                "CHOW Tape Model",
                "Chowdhury DSP",
                "auto",
                "",
                "Physical model of a reel-to-reel tape machine",
            ),
            (
                "tal-chorus-lx",
                "effect",
                "TAL-Chorus-LX",
                "TAL Software",
                "auto",
                "",
                "The chorus of a classic 1980s polysynth",
            ),
            (
                "zl-equalizer",
                "effect",
                "ZL Equalizer",
                "ZL Audio",
                "auto",
                "",
                "Dynamic equalizer with 16 bands",
            ),
            (
                "lsp-compressor",
                "effect",
                "LSP Compressor",
                "Linux Studio Plugins",
                "auto",
                "",
                "Compressor with sidechain and lookahead",
            ),
        ];
        rows.iter()
            .map(|(id, kind, name, maker, mode, tag, text)| {
                parse_entry(&format!(
                    "{id}\t{kind}\t{name}\t{maker}\t{mode}\t{tag}\thttps://example.invalid/{id}\t{text}"
                ))
                .unwrap()
            })
            .collect()
    }

    /// Runs `script` with `args` and collects everything it sends, to its end.
    fn run_to_end(script: &str, args: &[&str]) -> Vec<Msg> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let job = spawn(script, &args, || {}).unwrap();
        let mut got = Vec::new();
        loop {
            let m = job.rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let end = matches!(m, Msg::Exit(_));
            got.push(m);
            if end {
                return got;
            }
        }
    }

    fn out_text(msgs: &[Msg]) -> String {
        msgs.iter()
            .filter_map(|m| match m {
                Msg::Out(l) => Some(format!("{l}\n")),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_list_line_is_read_field_by_field() {
        let e = parse_entry(
            "ob-xd\tsynth\tOB-Xd\tdiscoDSP\tauto\tFree edition\thttps://www.discodsp.com/obxd/\tAnalog polysynth",
        )
        .unwrap();
        assert_eq!(
            e,
            Entry {
                id: "ob-xd".into(),
                kind: Kind::Synth,
                name: "OB-Xd".into(),
                maker: "discoDSP".into(),
                mode: Mode::Auto,
                tag: "Free edition".into(),
                page_url: "https://www.discodsp.com/obxd/".into(),
                description: "Analog polysynth".into(),
            }
        );
    }

    #[test]
    fn an_empty_tag_and_odd_characters_are_kept() {
        let e = parse_entry(
            "x1\teffect\tStub: fails \"always\" <é>\tNobody & Co.\tmanual\t\thttps://example.invalid/a?b=c&d=e\tDélai — 100 % wet; a = b, \"quoted\"\r",
        )
        .unwrap();
        assert_eq!(e.kind, Kind::Effect);
        assert_eq!(e.mode, Mode::Manual);
        assert_eq!(e.name, "Stub: fails \"always\" <é>");
        assert_eq!(e.maker, "Nobody & Co.");
        assert_eq!(e.tag, "");
        assert_eq!(e.page_url, "https://example.invalid/a?b=c&d=e");
        assert_eq!(e.description, "Délai — 100 % wet; a = b, \"quoted\"");
        // A tab inside the description stays in it: the description is the rest of the line.
        let e = parse_entry("x2\tsynth\tN\tM\tauto\t\tu\tone\ttwo").unwrap();
        assert_eq!(e.description, "one\ttwo");
        // An empty description and an empty maker are allowed.
        let e = parse_entry("x3\tsynth\tN\t\tauto\t\t\t").unwrap();
        assert_eq!((e.maker.as_str(), e.description.as_str()), ("", ""));
    }

    #[test]
    fn lines_that_are_not_plug_ins_are_left_out() {
        for bad in [
            "",
            "id\tkind\tname\tmaker\tmode\ttag\tpage_url\tdescription",
            "a\tsynth\tName\tMaker\tauto\t\turl",
            "a\tsampler\tName\tMaker\tauto\t\turl\ttext",
            "a\tsynth\tName\tMaker\tsometimes\t\turl\ttext",
            "\tsynth\tName\tMaker\tauto\t\turl\ttext",
            "a,b\tsynth\tName\tMaker\tauto\t\turl\ttext",
            "a b\tsynth\tName\tMaker\tauto\t\turl\ttext",
            "a\tsynth\t\tMaker\tauto\t\turl\ttext",
            "warning: something went wrong",
        ] {
            assert_eq!(parse_entry(bad), None, "{bad:?}");
        }
        let list = parse_list(
            "a\tsynth\tA\tM\tauto\t\tu\tt\nnoise\n\na\tsynth\tA again\tM\tauto\t\tu\tt\nb\teffect\tB\tM\tmanual\tNeeds account\tu\tt\n",
        );
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].name.as_str(), list[1].id.as_str()), ("A", "b"));
    }

    #[test]
    fn each_progress_line_is_read() {
        assert_eq!(
            parse_line("@@ start surge-xt"),
            Event::Start("surge-xt".into())
        );
        assert_eq!(
            parse_line("@@ ok surge-xt\r\n"),
            Event::Ok("surge-xt".into())
        );
        assert_eq!(
            parse_line("@@ fail dexed download failed: HTTP 404 (not found)"),
            Event::Fail {
                id: "dexed".into(),
                reason: "download failed: HTTP 404 (not found)".into()
            }
        );
        assert_eq!(
            parse_line("@@ fail dexed"),
            Event::Fail {
                id: "dexed".into(),
                reason: String::new()
            }
        );
        assert_eq!(
            parse_line("@@ manual vital https://vital.audio/?a=1&b=2"),
            Event::Manual {
                id: "vital".into(),
                url: "https://vital.audio/?a=1&b=2".into()
            }
        );
        assert_eq!(
            parse_line("@@ done ok=12 fail=1 manual=3"),
            Event::Done {
                ok: 12,
                fail: 1,
                manual: 3
            }
        );
        for log in [
            "Downloading Surge XT…",
            "",
            "@@",
            "@@ start",
            "@@ ok ",
            "@@ done",
            "@@ done ok=1 fail=x manual=0",
            "@@ later surge-xt",
            "@@start surge-xt",
            " @@ ok surge-xt",
        ] {
            assert_eq!(parse_line(log), Event::Log(log.into()), "{log:?}");
        }
    }

    #[test]
    fn a_recorded_run_moves_each_plug_in_through_its_states() {
        let mut run = Run::default();
        run.begin();
        assert!(run.running);
        assert_eq!(*run.state("surge-xt"), RowState::Idle);
        let transcript = ["@@ start surge-xt", "Downloading surge-xt", "Unpacking"];
        for l in transcript {
            run.line(l);
        }
        assert_eq!(*run.state("surge-xt"), RowState::Working);
        assert_eq!(*run.state("dexed"), RowState::Idle);
        for l in [
            "@@ ok surge-xt",
            "@@ start dexed",
            "curl: (22) The requested URL returned error: 404",
            "@@ fail dexed download failed",
            "@@ start vital",
            "@@ manual vital https://vital.audio",
            "@@ start odin2",
        ] {
            run.line(l);
        }
        assert_eq!(*run.state("surge-xt"), RowState::Ok);
        assert_eq!(
            *run.state("dexed"),
            RowState::Failed("download failed".into())
        );
        assert_eq!(
            *run.state("vital"),
            RowState::Manual("https://vital.audio".into())
        );
        assert_eq!(*run.state("odin2"), RowState::Working);
        assert_eq!(run.done, None);
        run.line("@@ ok odin2");
        run.line("@@ done ok=2 fail=1 manual=1");
        assert_eq!(run.done, Some((2, 1, 1)));
        assert_eq!(run.installed, 2);
        // Still running until the script itself has ended.
        assert!(run.running);
        run.exited(Some(0));
        assert!(!run.running);
        // The log has every line, the progress ones without their marker.
        let log: Vec<&str> = run.log.iter().map(String::as_str).collect();
        assert_eq!(log[0], "start surge-xt");
        assert_eq!(log[1], "Downloading surge-xt");
        assert_eq!(*log.last().unwrap(), "done ok=2 fail=1 manual=1");
        assert_eq!(log.len(), 12);
        // The next run starts clean.
        run.begin();
        assert!(run.log.is_empty() && run.states.is_empty() && run.installed == 0);
    }

    #[test]
    fn a_run_that_is_cut_short_leaves_nothing_working() {
        let mut run = Run::default();
        run.begin();
        run.line("@@ start surge-xt");
        run.line("@@ ok surge-xt");
        run.line("@@ start dexed");
        run.exited(None);
        assert!(!run.running);
        assert_eq!(*run.state("surge-xt"), RowState::Ok);
        assert_eq!(*run.state("dexed"), RowState::Idle);
        assert_eq!(run.done, None);
        assert_eq!(run.log.back().map(String::as_str), Some("stopped"));
        let mut run = Run::default();
        run.begin();
        run.exited(Some(2));
        assert_eq!(run.log.back().map(String::as_str), Some("exit 2"));
    }

    #[test]
    fn the_log_keeps_only_the_latest_lines() {
        let mut run = Run::default();
        for i in 0..LOG_LINES + 50 {
            run.line(&format!("line {i}"));
        }
        assert_eq!(run.log.len(), LOG_LINES);
        assert_eq!(run.log.front().map(String::as_str), Some("line 50"));
    }

    #[test]
    fn the_install_arguments_follow_the_contract() {
        let ids = vec!["a".to_string(), "b-2".to_string()];
        assert_eq!(install_args(&ids, false), ["--gui", "--only", "a,b-2"]);
        assert_eq!(
            install_args(&ids, true),
            ["--gui", "--dry-run", "--only", "a,b-2"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_stub_lists_its_plug_ins() {
        let got = run_to_end(STUB, &["--list"]);
        assert_eq!(got.last(), Some(&Msg::Exit(Some(0))));
        let text = out_text(&got);
        let list = parse_list(&text);
        assert_eq!(list.len(), text.lines().count(), "every line is a plug-in");
        assert!(list.len() >= 8);
        assert!(list.iter().any(|e| e.kind == Kind::Synth));
        assert!(list.iter().any(|e| e.kind == Kind::Effect));
        assert!(list
            .iter()
            .any(|e| e.mode == Mode::Manual && !e.tag.is_empty()));
        assert!(list
            .iter()
            .any(|e| e.mode == Mode::Auto && e.tag.is_empty()));
        let odd = list.iter().find(|e| e.id == "stub-broken").unwrap();
        assert_eq!(odd.name, "Stub: fails \"always\" <é>");
        assert_eq!(odd.maker, "Nobody & Co.");
        assert_eq!(odd.page_url, "https://example.invalid/a?b=c&d=e");
    }

    /// What the script the program ships prints for `--list` (it needs no network).
    #[cfg(unix)]
    fn shipped_list() -> String {
        let got = run_to_end(SCRIPT, &["--list"]);
        assert_eq!(got.last(), Some(&Msg::Exit(Some(0))));
        out_text(&got)
    }

    /// The installer the program ships: its list is the one the window was made for.
    #[cfg(unix)]
    #[test]
    fn the_shipped_script_lists_its_plug_ins() {
        let text = shipped_list();
        assert_eq!(text.lines().count(), 183);
        for l in text.lines() {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 8, "{l:?}");
            assert!(["synth", "effect"].contains(&f[1]), "{l:?}");
            assert!(["auto", "manual"].contains(&f[4]), "{l:?}");
            assert!(parse_entry(l).is_some(), "{l:?}");
        }
        let ids: HashSet<&str> = text.lines().filter_map(|l| l.split('\t').next()).collect();
        assert_eq!(ids.len(), 183, "an id is listed twice");
        let list = parse_list(&text);
        assert_eq!(list.len(), 183);
        let n = |f: &dyn Fn(&Entry) -> bool| list.iter().filter(|e| f(e)).count();
        assert_eq!(n(&|e| e.kind == Kind::Synth), 58);
        assert_eq!(n(&|e| e.kind == Kind::Effect), 125);
        assert_eq!(n(&|e| e.mode == Mode::Auto), 140);
        assert_eq!(n(&|e| e.mode == Mode::Manual), 43);
        for e in &list {
            assert!(
                ["", "Needs account", "Free edition"].contains(&e.tag.as_str()),
                "{e:?}"
            );
            assert!(
                e.page_url.starts_with("https://") || e.page_url.starts_with("http://"),
                "{e:?}"
            );
        }
    }

    /// The real installer downloads everything first and runs the installers at the end: a
    /// plug-in that comes as an installer stays "working" while later ones start and end.
    #[test]
    fn plug_ins_that_wait_for_their_installer_stay_working_until_the_end() {
        let mut run = Run::default();
        run.begin();
        for l in [
            "@@ start a-pkg",
            "  installer: A.pkg",
            "@@ start b-loose",
            "  copied B.vst3 to Library/Audio/Plug-Ins/VST3",
            "@@ ok b-loose",
            "@@ start c-pkg",
            "@@ start d-gone",
            "@@ fail d-gone download link not found, get it from https://example.invalid/d",
            "@@ start e-manual",
            "@@ manual e-manual https://example.invalid/e",
        ] {
            run.line(l);
        }
        assert_eq!(*run.state("a-pkg"), RowState::Working);
        assert_eq!(*run.state("c-pkg"), RowState::Working);
        assert_eq!(*run.state("b-loose"), RowState::Ok);
        for l in [
            "Running 2 installer(s). macOS asks for the password once.",
            "@@ ok a-pkg",
            "@@ fail c-pkg the installer reported an error",
            "@@ done ok=2 fail=2 manual=1",
        ] {
            run.line(l);
        }
        assert_eq!(*run.state("a-pkg"), RowState::Ok);
        assert_eq!(
            *run.state("c-pkg"),
            RowState::Failed("the installer reported an error".into())
        );
        assert_eq!(run.done, Some((2, 2, 1)));
        assert_eq!(run.installed, 2);
        run.exited(Some(0));
        assert!(!run.states.values().any(|s| *s == RowState::Working));
    }

    /// Whether process `pid` is there (`kill -0` fails on one that is not).
    #[cfg(unix)]
    fn alive(pid: &str) -> bool {
        std::process::Command::new("/bin/kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// Quitting the program, or closing the window, drops the job: that ends the script and
    /// what it started, as Cancel does.
    #[cfg(unix)]
    #[test]
    fn dropping_a_running_job_ends_the_script_and_what_it_started() {
        let script = "#!/bin/sh\nsleep 60 &\necho \"pids $$ $!\"\nsleep 60\n";
        let job = spawn(script, &[], || {}).unwrap();
        let line = match job.rx.recv_timeout(Duration::from_secs(30)).unwrap() {
            Msg::Out(l) => l,
            m => panic!("{m:?}"),
        };
        let pids: Vec<String> = line
            .strip_prefix("pids ")
            .unwrap()
            .split(' ')
            .map(str::to_string)
            .collect();
        assert_eq!(pids.len(), 2);
        assert!(pids.iter().all(|p| alive(p)));
        drop(job);
        let t0 = Instant::now();
        while pids.iter().any(|p| alive(p)) {
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "still running after the job was dropped"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The window stops its run when it is told to (it is, when it closes or the program
    /// quits) and the run then reads as stopped, not as a failure of the script.
    #[cfg(unix)]
    #[test]
    fn stopping_the_window_ends_its_run() {
        let script = "#!/bin/sh\ntrap 'echo \"@@ done ok=0 fail=0 manual=0\"; exit 130' TERM\necho \"@@ start slow\"\nsleep 60\n";
        let mut f = FreePlugins::default();
        f.run.begin();
        f.install = Some(spawn(script, &[], || {}).unwrap());
        let t0 = Instant::now();
        while *f.run.state("slow") != RowState::Working {
            assert!(t0.elapsed() < Duration::from_secs(30));
            f.poll();
            std::thread::sleep(Duration::from_millis(20));
        }
        f.stop();
        let t0 = Instant::now();
        while f.run.running {
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "the run did not end"
            );
            f.poll();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(f.install.is_none() && !f.stopping);
        assert_eq!(*f.run.state("slow"), RowState::Idle);
        assert_eq!(f.run.done, Some((0, 0, 0)));
        assert_eq!(f.run.log.back().map(String::as_str), Some("stopped"));
    }

    #[cfg(unix)]
    #[test]
    fn a_dry_run_of_the_stub_drives_the_states_to_the_end() {
        let ids: Vec<String> = ["surge-xt", "vital", "stub-broken", "nope"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let args = install_args(&ids, true);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let got = run_to_end(STUB, &args);
        let mut run = Run::default();
        run.begin();
        for m in &got {
            match m {
                Msg::Out(l) => run.line(l),
                Msg::Err(l) => run.push_log(l),
                Msg::Exit(code) => run.exited(*code),
            }
        }
        assert!(!run.running);
        assert_eq!(*run.state("surge-xt"), RowState::Ok);
        assert_eq!(
            *run.state("vital"),
            RowState::Manual("https://vital.audio".into())
        );
        assert_eq!(
            *run.state("stub-broken"),
            RowState::Failed("download failed (stub)".into())
        );
        assert_eq!(
            *run.state("nope"),
            RowState::Failed("unknown plug-in".into())
        );
        assert_eq!(run.done, Some((1, 2, 1)));
        assert_eq!(run.installed, 1);
        assert_eq!(
            run.log.back().map(String::as_str),
            Some("done ok=1 fail=2 manual=1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_stub_installs_nothing_without_dry_run_and_says_so() {
        let got = run_to_end(STUB, &["--gui", "--only", "dexed"]);
        let mut run = Run::default();
        run.begin();
        for m in &got {
            if let Msg::Out(l) = m {
                run.line(l);
            }
        }
        assert!(matches!(run.state("dexed"), RowState::Failed(r) if !r.is_empty()));
        assert_eq!(run.done, Some((0, 1, 0)));
        assert_eq!(got.last(), Some(&Msg::Exit(Some(0))));
    }

    #[cfg(unix)]
    #[test]
    fn a_script_that_cannot_start_ends_with_its_code_and_its_error_text() {
        let got = run_to_end(STUB, &["--no-such-option"]);
        assert_eq!(got.last(), Some(&Msg::Exit(Some(2))));
        assert!(got
            .iter()
            .any(|m| matches!(m, Msg::Err(l) if l.contains("unknown option"))));
    }

    #[cfg(unix)]
    #[test]
    fn the_script_file_is_private_and_removed_afterwards() {
        use std::os::unix::fs::PermissionsExt;
        // The script prints where it is and how it and its folder are protected.
        let script =
            "#!/bin/sh\necho \"$0\"\nls -ld \"$0\" | awk '{print $1}'\nls -ld \"$(dirname \"$0\")\" | awk '{print $1}'\n";
        let got = run_to_end(script, &[]);
        let text = out_text(&got);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{got:?}");
        let path = std::path::Path::new(lines[0]);
        assert_eq!(path.file_name().unwrap(), "install-free-plugins.sh");
        assert!(lines[1].starts_with("-rwx------"), "{}", lines[1]);
        assert!(lines[2].starts_with("drwx------"), "{}", lines[2]);
        assert!(!path.exists() && !path.parent().unwrap().exists());
        // Two runs never share a folder.
        let a = private_dir().unwrap();
        let b = private_dir().unwrap();
        assert_ne!(a, b);
        for d in [a, b] {
            assert_eq!(
                std::fs::metadata(&d).unwrap().permissions().mode() & 0o777,
                0o700
            );
            std::fs::remove_dir(&d).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancel_ends_the_script_and_what_it_started() {
        // A script with a child of its own, both of which would run for a minute.
        let script = "#!/bin/sh\necho \"@@ start slow\"\nsleep 60 &\necho \"child $!\"\nsleep 60\necho \"@@ ok slow\"\n";
        let job = spawn(script, &[], || {}).unwrap();
        let mut run = Run::default();
        run.begin();
        let mut child = None;
        while child.is_none() {
            match job.rx.recv_timeout(Duration::from_secs(30)).unwrap() {
                Msg::Out(l) => {
                    child = l.strip_prefix("child ").map(str::to_string);
                    run.line(&l);
                }
                m => panic!("{m:?}"),
            }
        }
        assert_eq!(*run.state("slow"), RowState::Working);
        let t0 = Instant::now();
        job.cancel();
        let end = loop {
            match job.rx.recv_timeout(Duration::from_secs(30)).unwrap() {
                Msg::Exit(code) => break code,
                Msg::Out(l) => run.line(&l),
                Msg::Err(_) => {}
            }
        };
        assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
        assert_eq!(end, None, "ended by the signal");
        run.exited(end);
        assert_eq!(*run.state("slow"), RowState::Idle);
        assert!(!run.running);
        // Its child is gone too.
        let alive = alive(&child.unwrap());
        assert!(!alive, "the script's child is still running");
    }

    #[test]
    fn search_looks_at_name_maker_description_and_tag() {
        let list = sample();
        let hit = |s: &str| -> Vec<&str> {
            list.iter()
                .filter(|e| matches(e, s))
                .map(|e| e.id.as_str())
                .collect()
        };
        assert_eq!(hit("").len(), list.len());
        assert_eq!(hit("  SURGE "), ["surge-xt"]);
        assert_eq!(hit("tytel"), ["vital", "helm"]);
        assert_eq!(hit("needs account"), ["vital", "valhalla-supermassive"]);
        assert_eq!(hit("tape"), ["chowtape"]);
        assert!(hit("no such thing").is_empty());
    }

    #[test]
    fn only_checked_plug_ins_the_script_can_install_are_chosen_in_list_order() {
        let mut f = FreePlugins {
            catalog: Some(sample()),
            ..FreePlugins::default()
        };
        for id in ["chowtape", "vital", "dexed", "gone"] {
            f.checked.insert(id.into());
        }
        assert_eq!(f.chosen(), ["dexed", "chowtape"]);
    }

    /// The window as the pictures show it: the sample list, a few plug-ins checked.
    fn pictured() -> crate::UiState {
        let mut s = crate::UiState::new();
        let f = &mut s.free_plugins;
        f.open = true;
        f.catalog = Some(sample());
        for id in ["surge-xt", "dexed", "odin2", "dragonfly-reverb", "chowtape"] {
            f.checked.insert(id.into());
        }
        s
    }

    fn draw(s: &mut crate::UiState, name: &str) {
        crate::ui_test::render_png(
            900.0,
            700.0,
            2.0,
            theme::BONE3,
            |ui| {
                theme::apply(ui.ctx());
                show(ui.ctx(), s);
            },
            &hu::picture_path(name),
        )
        .unwrap();
    }

    /// The manual's pictures, from the list of the installer the program ships:
    /// `manual-free-plugins.png` (a few plug-ins checked, before a run) and
    /// `manual-free-plugins-running.png` (in the middle of a run: one installed, one waiting
    /// for its installer, one failed, one being fetched). The `check-` ones show the longest
    /// names next to their tags and keys.
    #[cfg(unix)]
    #[test]
    #[ignore = "pictures"]
    fn picture_free_plugins_manual() {
        let list = parse_list(&shipped_list());
        let autos: Vec<String> = list
            .iter()
            .filter(|e| e.mode == Mode::Auto)
            .map(|e| e.id.clone())
            .collect();
        let state = |search: &str| {
            let mut s = crate::UiState::new();
            let f = &mut s.free_plugins;
            f.open = true;
            f.catalog = Some(list.clone());
            f.search = search.into();
            for id in autos.iter().take(4) {
                f.checked.insert(id.clone());
            }
            s
        };
        draw(&mut state(""), "manual-free-plugins.png");
        let mut s = state("");
        let run = &mut s.free_plugins.run;
        run.begin();
        let name = |id: &str| list.iter().find(|e| e.id == id).unwrap().name.clone();
        for l in [
            format!("@@ start {}", autos[0]),
            name(&autos[0]),
            "  copied to Library/Audio/Plug-Ins/VST3".to_string(),
            format!("@@ ok {}", autos[0]),
            format!("@@ start {}", autos[1]),
            name(&autos[1]),
            "  installer found".to_string(),
            format!("@@ start {}", autos[2]),
            name(&autos[2]),
            format!("@@ fail {} download failed: HTTP 404", autos[2]),
            format!("@@ start {}", autos[3]),
            name(&autos[3]),
        ] {
            run.line(&l);
        }
        draw(&mut s, "manual-free-plugins-running.png");
        for (search, file) in [
            ("valhalla", "check-valhalla.png"),
            ("meter", "check-meter.png"),
            ("needs account", "check-needs-account.png"),
            ("free edition", "check-free-edition.png"),
        ] {
            draw(&mut state(search), file);
        }
    }

    /// `free-plugins.png`: the list with five plug-ins checked, before a run.
    /// `free-plugins-running.png`: in the middle of a run (one installed, one failed, one
    /// being worked on).
    #[test]
    #[ignore = "pictures"]
    fn picture_free_plugins() {
        let mut s = pictured();
        draw(&mut s, "free-plugins.png");
        let mut s = pictured();
        let run = &mut s.free_plugins.run;
        run.begin();
        for l in [
            "@@ start surge-xt",
            "Downloading Surge XT",
            "Installing Surge XT.vst3",
            "@@ ok surge-xt",
            "@@ start dexed",
            "Downloading Dexed",
            "@@ fail dexed download failed: HTTP 404",
            "@@ start odin2",
            "Downloading Odin 2",
        ] {
            run.line(l);
        }
        draw(&mut s, "free-plugins-running.png");
    }
}
