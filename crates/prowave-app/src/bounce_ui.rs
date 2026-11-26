//! The Bounce dialog (File > Bounce > Project or Section, File > Export > All Tracks as Audio
//! Files), its progress display, and the frame's part of a running bounce (`bounce.rs` does
//! the work). The control interface starts and watches bounces through here too.
use crate::bounce::{self, Depth, Format, Job, Progress, Report, Settings};
use crate::hw::{self, Cap};
use crate::i18n::{tr, trf};
use crate::model::Project;
use crate::spectrogram_view::hw_vocal;
use crate::theme;
use egui::{pos2, vec2, Id, Rect, Sense};
use std::path::PathBuf;

/// The tail choices of the dialog: (least seconds kept, most seconds kept).
const TAILS: [(f64, f64); 6] = [
    (0.0, 0.0),
    (1.0, 20.0),
    (1.0, 1.0),
    (2.0, 2.0),
    (5.0, 5.0),
    (10.0, 10.0),
];
/// Where "Auto" is in [`TAILS`]: at least a second, and on while something is still heard.
const TAIL_AUTO: usize = 1;

/// What the dialog is set to.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    /// The cycle range instead of the whole project.
    pub cycle: bool,
    pub format: Format,
    pub depth: Depth,
    pub normalize: bool,
    /// Index into [`TAILS`].
    pub tail: usize,
    /// 0: the mix; 1: each track; 2: both.
    pub tracks: usize,
}

/// The Bounce dialog, the running bounce and how the last one ended.
#[derive(Default)]
pub struct BounceUi {
    pub dialog: Option<Dialog>,
    /// Bounce was pressed in the dialog: the file is asked for next.
    picked: Option<Dialog>,
    pub job: Option<Job>,
    /// How the last bounce ended, with its number.
    pub last: Option<(u64, Result<Report, String>)>,
    /// The number of the bounce running, or of the last one.
    pub serial: u64,
    /// The panel of the last bounce is shown (what it wrote and what it left out).
    result_open: bool,
    /// The settings of the dialog as they were last, for the next time it opens.
    remembered: Option<Dialog>,
    /// Bounce in Place: its dialog, a bounce asked for, the one running, and the dialog's
    /// settings as they were last (`bounce_in_place.rs`).
    pub in_place_dialog: Option<crate::bounce_in_place::Dialog>,
    pub in_place_go: Option<crate::bounce_in_place::Request>,
    pub in_place: Option<crate::bounce_in_place::Run>,
    pub in_place_remembered: Option<crate::bounce_in_place::Options>,
}

impl BounceUi {
    /// How far the running bounce is.
    pub fn progress(&self) -> Option<Progress> {
        self.job
            .as_ref()
            .map(Job::progress)
            .or_else(|| self.in_place.as_ref().map(|r| r.progress()))
    }

    /// Whether one of the bounce dialogs is open (the key commands wait).
    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some() || self.in_place_dialog.is_some()
    }
}

/// Opens the dialog (`each_track`: set to write every track into a file of its own).
pub fn open(p: &Project, s: &mut crate::UiState, each_track: bool) {
    let b = &mut s.bounce;
    if b.job.is_some() || b.in_place.is_some() {
        return;
    }
    let mut d = b.remembered.clone().unwrap_or(Dialog {
        cycle: false,
        format: Format::Wav,
        depth: Depth::Pcm24,
        normalize: false,
        tail: TAIL_AUTO,
        tracks: 0,
    });
    d.cycle = p.cycle && p.cycle_end > p.cycle_start;
    d.tracks = usize::from(each_track);
    b.dialog = Some(d);
    b.result_open = false;
}

/// The bounce the dialog `d` asks for, into `path`.
fn settings_of(d: &Dialog, p: &Project, path: PathBuf) -> Settings {
    let (tail_seconds, max_tail_seconds) = TAILS[d.tail.min(TAILS.len() - 1)];
    Settings {
        path,
        range: if d.cycle {
            Some((p.cycle_start, p.cycle_end))
        } else {
            bounce::whole_range(p)
        },
        format: d.format,
        depth: d.depth,
        normalize: d.normalize,
        tail_seconds,
        max_tail_seconds,
        mix: d.tracks != 1,
        stems: d.tracks >= 1,
        only: None,
        // The file panel has asked about a file that is there already.
        overwrite: true,
        strip: None,
    }
}

/// Starts a bounce of the app's project. One runs at a time. Returns its number.
pub fn start(app: &mut crate::App, settings: Settings) -> Result<u64, String> {
    if app.ui.bounce.job.is_some() || app.ui.bounce.in_place.is_some() {
        return Err(tr("A bounce is already running").to_string());
    }
    let rate = app.sync.as_ref().map_or(48_000, |s| s.engine.sample_rate());
    let job = Job::start(&app.project, rate, settings)?;
    let b = &mut app.ui.bounce;
    b.serial += 1;
    b.job = Some(job);
    b.result_open = false;
    Ok(b.serial)
}

/// Once a frame, before it is drawn: asks for the file of a bounce the dialog started, and
/// moves the running bounce on.
pub fn tend(app: &mut crate::App, ctx: &egui::Context) {
    if let Some(d) = app.ui.bounce.picked.take() {
        let name = if app.project.name.trim().is_empty() {
            "Bounce".to_string()
        } else {
            app.project.name.trim().to_string()
        };
        let ext = d.format.extension();
        let mut panel = rfd::FileDialog::new()
            .set_file_name(format!("{name}.{ext}"))
            .add_filter(d.format.name(), &[ext]);
        if let Some(dir) = app.path.as_ref().and_then(|f| f.parent()) {
            panel = panel.set_directory(dir);
        }
        if let Some(mut path) = panel.save_file() {
            if Format::of_path(&path) != Some(d.format) {
                path.set_extension(ext);
            }
            let settings = settings_of(&d, &app.project, path);
            if let Err(e) = start(app, settings) {
                app.ui.status = Some(trf("Bounce failed: {}", &[&e]));
            }
        }
    }
    tend_in_place(app, ctx);
    let Some(mut job) = app.ui.bounce.job.take() else {
        return;
    };
    let busy = app.ui.flex.busy() || app.ui.fit.busy();
    match job.poll(&mut app.project, &mut app.plugins, busy) {
        None => {
            app.ui.bounce.job = Some(job);
            // The bounce goes on while the window is hidden.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        Some(result) => {
            let b = &mut app.ui.bounce;
            app.ui.status = Some(match &result {
                Ok(r) => trf(
                    "Bounced {} s to {}",
                    &[
                        &format!("{:.1}", r.files.first().map_or(0.0, |f| f.seconds)),
                        &where_to(r),
                    ],
                ),
                Err(e) if e == "cancelled" => tr("Bounce cancelled").to_string(),
                Err(e) => trf("Bounce failed: {}", &[e]),
            });
            // What a bounce leaves out is shown until it is closed; so is a failure.
            b.result_open = match &result {
                Ok(r) => !r.not_rendered.is_empty(),
                Err(e) => e != "cancelled",
            };
            b.last = Some((b.serial, result));
            ctx.request_repaint();
        }
    }
}

/// Starts a Bounce in Place asked for, and moves the running one on; when it is over its
/// regions are in the project and the status line says so.
fn tend_in_place(app: &mut crate::App, ctx: &egui::Context) {
    if let Some(req) = app.ui.bounce.in_place_go.take() {
        if app.ui.bounce.job.is_none() && app.ui.bounce.in_place.is_none() {
            let rate = app.sync.as_ref().map_or(48_000, |s| s.engine.sample_rate());
            let dir = crate::project_io::recordings_dir();
            match crate::bounce_in_place::Run::start(&app.project, &req, rate, &dir) {
                Ok(run) => {
                    let b = &mut app.ui.bounce;
                    b.serial += 1;
                    b.in_place = Some(run);
                    b.result_open = false;
                }
                Err(e) => app.ui.status = Some(trf("Bounce failed: {}", &[&e])),
            }
        }
    }
    let Some(mut run) = app.ui.bounce.in_place.take() else {
        return;
    };
    let busy = app.ui.flex.busy() || app.ui.fit.busy();
    let polled = run.poll(
        &mut app.project,
        &mut app.plugins,
        busy,
        Some(ctx),
        &mut app.ui.tracks,
    );
    let Some(result) = polled else {
        app.ui.bounce.in_place = Some(run);
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        return;
    };
    let b = &mut app.ui.bounce;
    app.ui.status = Some(match &result {
        Ok(_) if run.what() == crate::bounce_in_place::What::Convert => trf(
            "Converted {} regions to new audio files",
            &[&run.regions_bounced()],
        ),
        Ok(_) => trf("Bounced {} regions in place", &[&run.regions_bounced()]),
        Err(e) if e == "cancelled" => tr("Bounce cancelled").to_string(),
        Err(e) => trf("Bounce failed: {}", &[e]),
    });
    b.result_open = match &result {
        Ok(done) => !done.report.not_rendered.is_empty(),
        Err(e) => e != "cancelled",
    };
    b.last = Some((b.serial, result.map(|d| d.report)));
    ctx.request_repaint();
}

/// Where a bounce went: its file, or the folder when it wrote several.
fn where_to(r: &Report) -> String {
    match r.files.as_slice() {
        [one] => one.path.display().to_string(),
        [first, ..] => first.path.parent().map_or_else(
            || first.path.display().to_string(),
            |d| d.display().to_string(),
        ),
        [] => String::new(),
    }
}

/// The dialog, the progress of a running bounce, and what the last one left out.
pub fn show(ctx: &egui::Context, p: &Project, s: &mut crate::UiState) {
    show_dialog(ctx, p, s);
    crate::bounce_in_place::show_dialog(ctx, p, s);
    show_progress(ctx, p, s);
    show_result(ctx, p, s);
}

fn show_dialog(ctx: &egui::Context, p: &Project, s: &mut crate::UiState) {
    let Some(mut d) = s.bounce.dialog.clone() else {
        return;
    };
    let id = Id::unique("prowave_bounce_dialog");
    let cycle_there = p.cycle_end > p.cycle_start;
    let mut go = false;
    let mut cancel = false;
    let (open, _) = hw_vocal::window(ctx, id, tr("Bounce"), &p.name, 380.0, None, |ui| {
        let ranges = [tr("Project"), tr("Cycle Range")];
        let field = hw_vocal::row(ui, tr("Range"));
        let shown = ranges[usize::from(d.cycle)];
        let items: &[&str] = if cycle_there { &ranges } else { &ranges[..1] };
        if let Some(i) = hw_vocal::dropdown(
            ui,
            id.with("range"),
            field,
            shown,
            items,
            Some(usize::from(d.cycle)),
        ) {
            d.cycle = i == 1;
        }
        let formats = [Format::Wav, Format::Aiff];
        let field = hw_vocal::row(ui, tr("Format"));
        let names: Vec<&str> = formats.iter().map(|f| f.name()).collect();
        if let Some(i) = hw_vocal::dropdown(
            ui,
            id.with("format"),
            field,
            d.format.name(),
            &names,
            formats.iter().position(|f| *f == d.format),
        ) {
            d.format = formats[i];
            if d.format == Format::Aiff && d.depth == Depth::Float32 {
                d.depth = Depth::Pcm24;
            }
        }
        let depths: &[Depth] = if d.format == Format::Aiff {
            &[Depth::Pcm16, Depth::Pcm24]
        } else {
            &[Depth::Pcm16, Depth::Pcm24, Depth::Float32]
        };
        let field = hw_vocal::row(ui, tr("Bit Depth"));
        let names: Vec<&str> = depths.iter().map(|x| x.name()).collect();
        if let Some(i) = hw_vocal::dropdown(
            ui,
            id.with("depth"),
            field,
            d.depth.name(),
            &names,
            depths.iter().position(|x| *x == d.depth),
        ) {
            d.depth = depths[i];
        }
        let on_off = [tr("Off"), tr("On")];
        let field = hw_vocal::row(ui, tr("Normalize"));
        if let Some(i) = hw_vocal::dropdown(
            ui,
            id.with("normalize"),
            field,
            on_off[usize::from(d.normalize)],
            &on_off,
            Some(usize::from(d.normalize)),
        ) {
            d.normalize = i == 1;
        }
        let tails = [tr("Off"), tr("Auto"), "1 s", "2 s", "5 s", "10 s"];
        let field = hw_vocal::row(ui, tr("Tail"));
        if let Some(i) = hw_vocal::dropdown(
            ui,
            id.with("tail"),
            field,
            tails[d.tail.min(tails.len() - 1)],
            &tails,
            Some(d.tail),
        ) {
            d.tail = i;
        }
        let tracks = [tr("Mix"), tr("Each Track"), tr("Mix and Each Track")];
        let field = hw_vocal::row(ui, tr("Tracks"));
        if let Some(i) = hw_vocal::dropdown(
            ui,
            id.with("tracks"),
            field,
            tracks[d.tracks.min(2)],
            &tracks,
            Some(d.tracks),
        ) {
            d.tracks = i;
        }
        hw_vocal::gap(ui, 12.0);
        let clicked = hw_vocal::key_row(
            ui,
            id.with("keys"),
            &[
                (tr("Cancel"), Cap::Bone, None, true),
                (tr("Bounce"), Cap::Orange, None, true),
            ],
        );
        cancel = clicked[0];
        go = clicked[1];
    });
    let b = &mut s.bounce;
    b.remembered = Some(d.clone());
    if go {
        b.picked = Some(d);
        b.dialog = None;
    } else if cancel || !open {
        b.dialog = None;
    } else {
        b.dialog = Some(d);
    }
}

/// An LCD across the panel, `height` tall; returns its glass.
fn lcd(ui: &mut egui::Ui, height: f32) -> Rect {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    hw::lcd(ui.painter(), r).shrink2(vec2(8.0, 5.0))
}

fn show_progress(ctx: &egui::Context, p: &Project, s: &mut crate::UiState) {
    let Some(pr) = s.bounce.progress() else {
        return;
    };
    if progress_panel(ctx, p, &pr) {
        if let Some(job) = s.bounce.job.as_ref() {
            job.cancel();
        }
        if let Some(run) = s.bounce.in_place.as_ref() {
            run.cancel();
        }
    }
}

/// The progress display of a running bounce. Returns whether Cancel or the close key was
/// pressed (the panel shows as long as the bounce runs, so closing it cancels the bounce).
fn progress_panel(ctx: &egui::Context, p: &Project, pr: &Progress) -> bool {
    let id = Id::unique("prowave_bounce_progress");
    let mut cancel = false;
    let (open, _) = hw_vocal::window(ctx, id, tr("Bounce"), &p.name, 380.0, None, |ui| {
        let glass = lcd(ui, 62.0);
        let g = ui.painter().clone();
        let what = if pr.preparing {
            tr("Preparing").to_string()
        } else {
            format!("{} {}%", tr("Bouncing"), (pr.fraction * 100.0).floor())
        };
        let line = match (&pr.track, pr.files) {
            (Some(track), _) => format!("{what}  {}/{}  {track}", pr.file, pr.files),
            (None, n) if n > 1 => format!("{what}  {}/{}  {}", pr.file, pr.files, tr("Mix")),
            _ => what,
        };
        let text = Rect::from_min_max(glass.min, pos2(glass.right(), glass.center().y + 4.0));
        hw_vocal::lcd_text(&g, text, &line, 12.0, 2, theme::LCD_INK());
        let bar = Rect::from_min_max(
            pos2(glass.left(), glass.bottom() - 9.0),
            pos2(glass.right(), glass.bottom() - 2.0),
        );
        if pr.preparing {
            hw_vocal::lcd_chaser(&g, bar, ui.input(|i| i.time));
        } else {
            hw_vocal::lcd_ladder(&g, bar, pr.fraction);
        }
        hw_vocal::gap(ui, 12.0);
        cancel = hw_vocal::key_row(
            ui,
            id.with("keys"),
            &[(tr("Cancel"), Cap::Bone, None, true)],
        )[0];
    });
    cancel || !open
}

fn show_result(ctx: &egui::Context, p: &Project, s: &mut crate::UiState) {
    if !s.bounce.result_open || s.bounce.job.is_some() {
        return;
    }
    let Some((_, result)) = s.bounce.last.as_ref() else {
        return;
    };
    let (head, lines): (String, Vec<String>) = match result {
        Ok(r) => (
            trf(
                "Bounced {} s to {}",
                &[
                    &format!("{:.1}", r.files.first().map_or(0.0, |f| f.seconds)),
                    &where_to(r),
                ],
            ),
            r.not_rendered.clone(),
        ),
        Err(e) => (trf("Bounce failed: {}", &[e]), Vec::new()),
    };
    if result_panel(ctx, p, &head, &lines) {
        s.bounce.result_open = false;
    }
}

/// The panel of a bounce that left something out, or failed: `head` on the LCD and `lines`
/// under "Not in the Bounce". Returns whether it was closed.
fn result_panel(ctx: &egui::Context, p: &Project, head: &str, lines: &[String]) -> bool {
    let id = Id::unique("prowave_bounce_result");
    let mut close = false;
    let (open, _) = hw_vocal::window(ctx, id, tr("Bounce"), &p.name, 460.0, None, |ui| {
        let glass = lcd(ui, 62.0);
        hw_vocal::lcd_text(ui.painter(), glass, head, 12.0, 3, theme::LCD_INK());
        if !lines.is_empty() {
            hw_vocal::gap(ui, 6.0);
            hw_vocal::section(ui, tr("Not in the Bounce"));
            for line in lines {
                let mut job = egui::text::LayoutJob::single_section(
                    line.clone(),
                    egui::TextFormat {
                        font_id: theme::font(theme::Face::Body, 12.0),
                        color: theme::INK(),
                        ..Default::default()
                    },
                );
                job.wrap.max_width = ui.available_width();
                let galley = ui.painter().layout_job(job);
                let (r, _) = ui.allocate_exact_size(
                    vec2(ui.available_width(), galley.size().y + 6.0),
                    Sense::hover(),
                );
                ui.painter()
                    .galley(pos2(r.left(), r.top() + 3.0), galley, theme::INK());
            }
        }
        hw_vocal::gap(ui, 12.0);
        close = hw_vocal::key_row(ui, id.with("keys"), &[(tr("Close"), Cap::Bone, None, true)])[0];
    });
    close || !open
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The progress panel's close key cancels the bounce (the panel stays while it runs).
    #[test]
    fn the_progress_panels_close_key_cancels() {
        let p = Project::demo();
        let pr = Progress {
            fraction: 0.5,
            file: 1,
            files: 1,
            track: None,
            preparing: false,
        };
        let mut h = crate::ui_test::Harness::new(1000.0, 700.0);
        let mut cancel = false;
        h.settle(|ui| cancel |= progress_panel(ui.ctx(), &p, &pr));
        assert!(!cancel);
        let at = hw_vocal::close_key_center(&h.ctx, Id::unique("prowave_bounce_progress"))
            .expect("the panel is on screen");
        h.click(at, |ui| cancel |= progress_panel(ui.ctx(), &p, &pr));
        assert!(cancel, "the close key cancels");
    }

    /// Pictures of the Bounce dialog, its progress display and the panel of a bounce that left
    /// something out: `MANUAL_PNG_DIR=… cargo test -p prowave-app bounce_window_pictures --
    /// --ignored`.
    #[test]
    #[ignore = "pictures of the windows"]
    fn bounce_window_pictures() {
        let mut p = Project::demo();
        p.name = "Untitled".into();
        p.cycle_start = 0.0;
        p.cycle_end = 16.0;
        let mut s = crate::UiState::new();
        open(&p, &mut s, false);
        let shot = |name: &str, h: f32, ui: &mut dyn FnMut(&egui::Context)| {
            crate::ui_test::render_png(
                520.0,
                h,
                2.0,
                theme::BONE3(),
                |u| {
                    theme::apply(u.ctx());
                    ui(u.ctx())
                },
                &hw_vocal::picture_path(name),
            )
            .unwrap();
        };
        shot("bounce-dialog.png", 360.0, &mut |ctx| {
            show_dialog(ctx, &p, &mut s)
        });
        assert!(s.bounce.dialog.is_some(), "the dialog stays open");
        let pr = Progress {
            fraction: 0.42,
            file: 3,
            files: 7,
            track: Some("Kick".into()),
            preparing: false,
        };
        shot("bounce-progress.png", 200.0, &mut |ctx| {
            progress_panel(ctx, &p, &pr);
        });
        let lines = vec![
            "\"Lead\": 2 of the Singer's 6 phrases have not been sung yet (open the project and let it sing, then bounce again)".to_string(),
            "could not be opened: Example Synth is not installed".to_string(),
        ];
        shot("bounce-result.png", 300.0, &mut |ctx| {
            result_panel(ctx, &p, "Bounced 97.0 s to Untitled.wav", &lines);
        });
    }
}
