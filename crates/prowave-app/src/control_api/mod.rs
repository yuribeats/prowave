//! The control interface: another program on this Mac (Claude Code, through
//! `prowavectl`) drives the running app over a Unix socket. `docs/CONTROL-API.md` is the
//! reference; its method table is made from `registry.rs`.
//!
//! How a request travels: a socket thread reads a line and parses it (`server.rs`,
//! `protocol.rs`), puts the request in a channel and wakes the interface. The interface thread
//! takes it in [`pump`], which `App::logic` calls before every frame and also while the window
//! is hidden. The method runs there, on the project and the interface state, through the
//! functions the menus, the mixer and the Tracks area call; it never talks to the audio
//! thread. The engine hears of the change the way it hears of a click: the frame that follows
//! compares the project with what the engine has (`engine_sync.rs`). Each call that changes
//! the project ends with one Undo step.
use crate::App;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod events;
pub(crate) mod headless;
mod ids;
mod m_core;
mod m_kits;
mod m_midi_fx;
mod m_rack;
mod m_rack_follow;
mod m_regions;
mod m_tracks;
mod m_ui;
mod params;
pub(crate) mod protocol;
pub(crate) mod registry;
mod render;
mod server;
mod state;
#[cfg(test)]
mod tests;

use protocol::{Code, RpcError};
use registry::Reply;
use server::{Incoming, Server, Shared};

/// Turns the control socket off when set (to anything but "0" or nothing).
pub const OFF_VAR: &str = "PROWAVE_NO_CONTROL";
/// Another place for the socket than the settings folder.
pub const SOCKET_VAR: &str = "PROWAVE_CONTROL_SOCKET";
/// The socket's file name in the settings folder.
pub const SOCKET_NAME: &str = "control.sock";
/// The public edition: the socket is off unless this is set to 1, or a file of this name
/// (`control-on`) is in the settings folder.
pub const ON_VAR: &str = "PROWAVE_CONTROL";
pub const ON_FILE: &str = "control-on";

/// Whether the user turned the public edition's socket on (Help > Allow Control by Other
/// Programs, or `PROWAVE_CONTROL=1`).
pub fn allowed() -> bool {
    std::env::var_os(ON_VAR).is_some_and(|v| v == "1")
        || crate::recent_files::config_dir().is_some_and(|d| d.join(ON_FILE).exists())
}

/// Remembers the choice of Help > Allow Control by Other Programs for the next start.
pub fn set_allowed(on: bool) {
    if cfg!(test) {
        return;
    }
    let Some(dir) = crate::recent_files::config_dir() else {
        return;
    };
    let file = dir.join(ON_FILE);
    let done = if on {
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&file, b""))
    } else {
        std::fs::remove_file(&file).or_else(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(e),
        })
    };
    if let Err(e) = done {
        tracing::warn!("could not save the control setting: {e}");
    }
}

/// The longest the interface thread works on waiting requests before it draws a frame.
const PUMP_BUDGET: Duration = Duration::from_millis(40);

/// The socket and everything the methods keep between calls.
pub struct Control {
    server: Server,
    core: Core,
}

/// What the methods keep between calls.
pub struct Core {
    pub ids: ids::Ids,
    pub shared: Arc<Shared>,
    pub socket: PathBuf,
    pub times: Times,
    pub watch: events::Watch,
    /// Inside `batch`: the Undo step is made once, at its end.
    pub in_batch: bool,
    /// The app runs without a window (`--control-headless`): pictures come from the software
    /// renderer, and quitting ends the loop.
    pub headless: bool,
    /// Pictures asked for and not made yet.
    pub shots: Vec<m_ui::Shot>,
    /// `app.quit` was called (the headless loop ends on it).
    pub quit: bool,
    pub started: Instant,
    /// `rpc.wait` calls not answered yet.
    pub waits: Vec<Wait>,
    /// `export.bounce` and `render.range` calls whose bounce is still running.
    pub bounces: Vec<render::Pending>,
    /// What the bounce events last told the subscribed clients.
    pub bounce_seen: render::Seen,
}

/// An `rpc.wait` call: answered once this many frames have been drawn, or at its time limit.
pub struct Wait {
    pub conn: u64,
    pub id: Option<Value>,
    pub frames: u32,
    pub asked: Instant,
    pub limit: Duration,
}

/// How long the last requests took from their line being read to their answer being queued.
#[derive(Default)]
pub struct Times {
    recent: std::collections::VecDeque<f32>,
    pub count: u64,
}

impl Times {
    fn add(&mut self, ms: f32) {
        self.count += 1;
        if self.recent.len() >= 512 {
            self.recent.pop_front();
        }
        self.recent.push_back(ms);
    }

    pub fn to_json(&self) -> Value {
        let mut v: Vec<f32> = self.recent.iter().copied().collect();
        v.sort_by(f32::total_cmp);
        let at = |q: f32| {
            v.get(((v.len() as f32 - 1.0) * q).round() as usize)
                .map_or(0.0, |x| params::round3(f64::from(*x)))
        };
        let mean = if v.is_empty() {
            0.0
        } else {
            params::round3(f64::from(v.iter().sum::<f32>() / v.len() as f32))
        };
        json!({
            "requests": self.count,
            "measured": v.len(),
            "mean_ms": mean,
            "median_ms": at(0.5),
            "p95_ms": at(0.95),
            "max_ms": at(1.0),
        })
    }
}

/// What a method works with.
pub struct Cx<'a> {
    pub app: &'a mut App,
    pub ctx: &'a egui::Context,
    pub core: &'a mut Core,
    /// The connection the request came from.
    pub conn: u64,
    /// The request's id (`None`: a notification, which gets no answer).
    pub id: Option<Value>,
}

impl Cx<'_> {
    /// The id table, matched to the project as it is now.
    pub fn ids(&mut self) -> &ids::Ids {
        self.core.ids.reconcile(&self.app.project);
        &self.core.ids
    }
}

impl Control {
    /// Opens the socket where the environment and the settings folder say, unless it is
    /// turned off. A failure is logged and the app runs without it.
    pub fn start() -> Option<Self> {
        if std::env::var_os(OFF_VAR).is_some_and(|v| !v.is_empty() && v != "0") {
            tracing::info!("control socket off ({OFF_VAR})");
            return None;
        }
        // The public edition opens it only when asked to.
        {
            if !allowed() {
                tracing::info!("control socket off (set {ON_VAR}=1 to turn it on)");
                return None;
            }
        }
        let (path, private_dir) = match std::env::var_os(SOCKET_VAR).filter(|v| !v.is_empty()) {
            Some(p) => (PathBuf::from(p), false),
            None => (crate::recent_files::config_dir()?.join(SOCKET_NAME), true),
        };
        match Self::start_at(&path, private_dir) {
            Ok(c) => {
                tracing::info!("control socket at {}", path.display());
                Some(c)
            }
            Err(e) => {
                tracing::warn!("no control socket: {e}");
                None
            }
        }
    }

    pub fn start_at(path: &Path, private_dir: bool) -> Result<Self, String> {
        let server = Server::start(path, private_dir)?;
        let core = Core {
            ids: ids::Ids::default(),
            shared: server.shared.clone(),
            socket: path.to_path_buf(),
            times: Times::default(),
            watch: events::Watch::default(),
            in_batch: false,
            headless: false,
            shots: Vec::new(),
            quit: false,
            started: Instant::now(),
            waits: Vec::new(),
            bounces: Vec::new(),
            bounce_seen: render::Seen::default(),
        };
        Ok(Self { server, core })
    }
}

/// Runs the requests that are waiting and sends the events that are due. Called by
/// `App::logic`: before every frame, and when a request arrives while the window is hidden
/// (the socket thread asks for a repaint, which runs the logic even then).
pub fn pump(app: &mut App, ctx: &egui::Context) {
    let Some(mut control) = app.control.take() else {
        return;
    };
    control.server.shared.set_wake(ctx);
    let began = Instant::now();
    let mut handled = 0usize;
    // Each turn here is a frame gone by for the calls that wait for frames.
    let shared = control.core.shared.clone();
    control.core.waits.retain_mut(|w| {
        w.frames = w.frames.saturating_sub(1);
        if w.frames > 0 && w.asked.elapsed() < w.limit {
            return true;
        }
        if let Some(id) = &w.id {
            let ms = params::round3(w.asked.elapsed().as_secs_f64() * 1000.0);
            shared.respond(w.conn, id, &Ok(json!({"waited_ms": ms})));
        }
        false
    });
    while let Ok(incoming) = control.server.incoming.try_recv() {
        handle(app, ctx, &mut control.core, incoming);
        handled += 1;
        if began.elapsed() > PUMP_BUDGET {
            break;
        }
    }
    m_ui::tend_shots(&mut control.core);
    let bouncing = render::tend(app, &mut control.core);
    let listeners = control.core.shared.subscribers() > 0;
    if listeners {
        events::watch(app, &mut control.core);
    } else {
        control.core.watch = events::Watch::default();
    }
    if handled > 0 {
        // The frame that follows hands the change to the engine and to Undo. A hidden window
        // would run no frame, so it is asked for one.
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint();
    } else if listeners && (app.project.playing || app.ui.fit.busy()) {
        // Someone is waiting for the playhead or for a job: keep looking while hidden too.
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint_after(Duration::from_millis(50));
    } else if bouncing || !control.core.bounces.is_empty() {
        // The bounce's own frames keep it going; its end is answered on the frame after.
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint_after(Duration::from_millis(16));
    } else if !control.core.waits.is_empty() {
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint();
    } else if !control.core.shots.is_empty() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    app.control = Some(control);
}

fn handle(app: &mut App, ctx: &egui::Context, core: &mut Core, incoming: Incoming) {
    let Incoming { conn, request, at } = incoming;
    let result = dispatch(
        app,
        ctx,
        core,
        conn,
        request.id.clone(),
        &request.method,
        &request.params,
    );
    core.times.add(at.elapsed().as_secs_f32() * 1000.0);
    let Some(id) = request.id else {
        return;
    };
    match result {
        Ok(Reply::Now(v)) => core.shared.respond(conn, &id, &Ok(v)),
        Ok(Reply::Later) => {}
        Err(e) => core.shared.respond(conn, &id, &Err(e)),
    }
}

/// Runs one method: checks its parameters, refuses a change while a question is open, and
/// after a change makes the Undo step.
pub(crate) fn dispatch(
    app: &mut App,
    ctx: &egui::Context,
    core: &mut Core,
    conn: u64,
    id: Option<Value>,
    method: &str,
    params: &Map<String, Value>,
) -> Result<Reply, RpcError> {
    let m = registry::find(method).ok_or_else(|| registry::unknown(method))?;
    registry::validate(m, params)?;
    if m.changes {
        if let Some(d) = m_ui::open_dialog(app) {
            return Err(RpcError::new(
                Code::DialogOpen,
                format!(
                    "the app is asking \"{}\" and takes no changes until it is answered: call ui.dialog.answer with one of: {}",
                    d.text,
                    d.buttons.join(", ")
                ),
            )
            .with("dialog", d.to_json()));
        }
    }
    core.ids.reconcile(&app.project);
    let out = {
        let mut cx = Cx {
            app,
            ctx,
            core,
            conn,
            id,
        };
        (m.run)(&mut cx, &params::P(params))
    };
    if m.changes {
        if !core.in_batch {
            commit(app);
        }
        core.ids.reconcile(&app.project);
    }
    out
}

/// After a change: the Undo step, as the app makes one when a gesture ends, and the
/// selection kept to what is still there.
pub(crate) fn commit(app: &mut App) {
    // A recording, or a file being fitted to the grid, is one edit: its step is made when
    // it ends (see `App::track_history`).
    let busy = app.ui.rec.active() || app.ui.audio_rec.active() || app.ui.fit.busy();
    if !busy {
        app.history
            .record_followed(&mut app.project, &mut app.ui.flex);
        app.frames_since_record = 0;
    }
    app.ui.can_undo = app.history.can_undo();
    app.ui.can_redo = app.history.can_redo();
    app.ui.tracks.prune(&app.project);
    if !app.project.tracks.is_empty() {
        app.project.selected_track = app.project.selected_track.min(app.project.tracks.len() - 1);
    }
}

/// `v` as a method's result.
pub(crate) fn ok(v: Value) -> Result<Reply, RpcError> {
    Ok(Reply::Now(v))
}
