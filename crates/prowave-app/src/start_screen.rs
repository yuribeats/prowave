//! Project Chooser, shown by File > New from Template…: start an empty project, reopen a
//! recent one or start from a template, as in Logic.
//! Choosing queues a File command (`UiState::file_cmd`), so unsaved changes are asked about
//! like everywhere else.
use crate::file_commands::FileCmd;
use crate::hw::{self, Cap, Legend};
use crate::i18n::tr;
use crate::model::{Project, Track, TrackKind};
use crate::theme::{self, Face};
use crate::track_icons::{draw_icon, IconKind};
use crate::tracks_area::hw_tracks;
use crate::transport::hw_transport;
use crate::UiState;
use egui::{pos2, vec2, Align2, Key, Rect, Sense, Stroke, StrokeKind};
use std::path::PathBuf;

/// A project to start from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Template {
    /// One software instrument track (File > New).
    Empty,
    Songwriter,
    Electronic,
    Podcast,
}

impl Template {
    pub const ALL: [Template; 4] = [
        Template::Empty,
        Template::Songwriter,
        Template::Electronic,
        Template::Podcast,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Template::Empty => "Empty Project",
            Template::Songwriter => "Songwriter",
            Template::Electronic => "Electronic",
            Template::Podcast => "Podcast",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Template::Empty => "One software instrument track.",
            Template::Songwriter => "Piano, drummer, bass and vocal tracks.",
            Template::Electronic => "Synths, drum machine and a reverb aux.",
            Template::Podcast => "Two voice tracks and a music bed.",
        }
    }

    fn icon(self) -> IconKind {
        match self {
            Template::Empty => IconKind::Note,
            Template::Songwriter => IconKind::Piano,
            Template::Electronic => IconKind::Synth,
            Template::Podcast => IconKind::Mic,
        }
    }

    /// The new, untitled project this template starts. Its instrument and Drummer tracks
    /// have no instrument loaded yet.
    pub fn project(self) -> Project {
        let track = Track::new;
        let mut p = Project::empty();
        match self {
            Template::Empty => return p,
            Template::Songwriter => {
                p.tracks = vec![
                    track("Piano", TrackKind::Instrument),
                    track("Drummer", TrackKind::Drummer),
                    track("Bass", TrackKind::Audio),
                    track("Lead Vocal", TrackKind::Audio),
                ];
            }
            Template::Electronic => {
                p.tempo = 124.0;
                p.tracks = vec![
                    track("Drums", TrackKind::Drummer),
                    track("Synth Bass", TrackKind::Instrument),
                    track("Pad", TrackKind::Instrument),
                    track("Reverb", TrackKind::Aux),
                ];
            }
            Template::Podcast => {
                p.tracks = vec![
                    track("Host", TrackKind::Audio),
                    track("Guest", TrackKind::Audio),
                    track("Music Bed", TrackKind::Audio),
                ];
            }
        }
        p.name = "Untitled".into();
        p.selected_track = 0;
        p
    }
}

/// Sidebar sections of the chooser.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    NewProject,
    Recent,
    Templates,
}

impl Section {
    const ALL: [Section; 3] = [Section::NewProject, Section::Recent, Section::Templates];

    fn label(self) -> &'static str {
        match self {
            Section::NewProject => tr("New Project"),
            Section::Recent => tr("Recent"),
            Section::Templates => tr("Project Templates"),
        }
    }

    /// The templates the section shows as tiles (none for Recent).
    fn items(self) -> Vec<Template> {
        match self {
            Section::NewProject => vec![Template::Empty],
            Section::Templates => Template::ALL.to_vec(),
            Section::Recent => Vec::new(),
        }
    }
}

/// State of the open chooser.
#[derive(Clone, Debug, PartialEq)]
pub struct Chooser {
    pub section: Section,
    /// Highlighted tile or row in the section.
    pub selected: usize,
}

impl Chooser {
    /// File > New from Template…: over the open project, which Cancel goes back to.
    pub fn templates() -> Self {
        Self {
            section: Section::Templates,
            selected: 0,
        }
    }
}

/// What the highlighted item does when chosen.
fn choice(c: &Chooser, recent: &[PathBuf]) -> Option<Choice> {
    match c.section {
        Section::NewProject => Some(Choice::Template(Template::Empty)),
        Section::Recent => recent.get(c.selected).cloned().map(Choice::Recent),
        // The other sections choose their highlighted tile.
        _ => c
            .section
            .items()
            .get(c.selected)
            .copied()
            .map(Choice::Template),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Choice {
    Template(Template),
    Recent(PathBuf),
}

/// Queues the File command for `choice`.
fn choose(choice: Choice, s: &mut UiState) {
    match choice {
        Choice::Template(t) => s.file_cmd = Some(FileCmd::NewFrom(t)),
        Choice::Recent(path) => {
            s.recent_pick = Some(path);
            s.file_cmd = Some(FileCmd::OpenRecent);
        }
    }
}

/// A tile in the New Project / Templates grid: a bone faceplate with the template's icon in
/// a recessed well, its name in silkscreen and its description as a silkscreen caption. The
/// selected tile is marked one way only, the orange top edge of a selected channel strip.
/// Returns (clicked, double-clicked).
fn tile(ui: &egui::Ui, r: Rect, t: Template, selected: bool, key: usize) -> (bool, bool) {
    let resp = ui.interact(r, egui::Id::unique(("chooser_tile", key)), Sense::click());
    let g = ui.painter();
    hw::faceplate(g, r, 4.0, selected);
    if resp.hovered() && !selected {
        g.rect_stroke(r, 4.0, Stroke::new(1.0, theme::SEAM_DK), StrokeKind::Inside);
    }
    let well = Rect::from_center_size(pos2(r.center().x, r.top() + 50.0), vec2(64.0, 64.0));
    hw_tracks::well(g, well, 4.0);
    draw_icon(g, well.shrink(14.0), t.icon());
    let (name, about) = (tr(t.name()), tr(t.about()));
    let shown = hw::fit(g, &name.to_uppercase(), Face::SilkX, 14.0, r.width() - 16.0);
    hw::text(
        g,
        pos2(r.center().x, well.bottom() + 17.0),
        Align2::CENTER_CENTER,
        &shown,
        Face::SilkX,
        14.0,
        theme::INK,
    );
    // The description: silkscreen caps, centred, wrapped to the tile.
    let size = 11.0;
    let about = about.trim_end_matches('.').to_uppercase();
    let mut job = egui::text::LayoutJob::single_section(
        about,
        egui::TextFormat {
            font_id: theme::font(Face::Silk, size),
            color: theme::INK2,
            extra_letter_spacing: theme::tracking(Face::Silk) * size,
            line_height: Some(size * 1.35),
            ..Default::default()
        },
    );
    job.wrap.max_width = r.width() - 24.0;
    job.halign = egui::Align::Center;
    let gal = g.layout_job(job);
    g.galley(pos2(r.center().x, well.bottom() + 32.0), gal, theme::INK2);
    (resp.clicked(), resp.double_clicked())
}

/// Days since 1970-01-01 as (year, month, day), in UTC.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// When the project at `path` was last saved, as 2026-11-08 (None when it is not there).
fn modified(path: &std::path::Path) -> Option<String> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let (y, m, d) = civil((secs / 86_400) as i64);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// A recent project's row: a numbered pad, then a slot with the name in silkscreen and the
/// folder under it, and the date it was saved on a small LCD at the right. The highlighted
/// row is a black slot with its LED lit, as a loaded plug-in slot.
fn recent_row(
    ui: &egui::Ui,
    g: &egui::Painter,
    r: Rect,
    k: usize,
    path: &std::path::Path,
    on: bool,
) -> egui::Response {
    let resp = ui.interact(r, egui::Id::unique(("chooser_recent", k)), Sense::click());
    let pad = Rect::from_min_size(r.min, vec2(r.height(), r.height()));
    hw::track_pad(g, pad, k + 1, on, resp.hovered());
    let slot = Rect::from_min_max(pos2(pad.right() + 10.0, r.top()), r.max);
    let kind = if on {
        hw::SlotKind::Filled
    } else {
        hw::SlotKind::Plain
    };
    hw::paint_slot(g, slot, kind, "", Some(on), false, resp.hovered());
    let (ink, dim) = if on {
        (theme::ON_DARK, theme::ON_DARK_DIM)
    } else {
        (theme::INK, theme::INK2)
    };
    // The date on a strip of LCD glass.
    let mut right = slot.right() - 10.0;
    if let Some(date) = modified(path) {
        let h = 13.0;
        let w = hw::seg_width(&date, h) + 16.0;
        let glass = Rect::from_min_size(pos2(right - w, slot.center().y - 12.0), vec2(w, 24.0));
        hw::lcd_glass(g, glass);
        hw::seg(
            g,
            pos2(glass.left() + 8.0, glass.center().y - h / 2.0),
            &date,
            h,
            theme::LCD_INK,
            false,
        );
        right = glass.left() - 14.0;
    }
    let x = slot.left() + 24.0;
    let room = right - x;
    let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("?");
    hw::text(
        g,
        pos2(x, slot.center().y - 7.0),
        Align2::LEFT_CENTER,
        &hw::fit(g, &name.to_uppercase(), Face::SilkX, 15.0, room),
        Face::SilkX,
        15.0,
        ink,
    );
    let folder = path
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    hw::text(
        g,
        pos2(x, slot.center().y + 10.0),
        Align2::LEFT_CENTER,
        &hw::fit(g, &folder, Face::Bold, 10.5, room),
        Face::Bold,
        10.5,
        dim,
    );
    resp
}

/// A text key of the footer, as wide as its legend, its left edge at `x` (or its right edge
/// when `from_right`). Drawn without a click when not `enabled`. Returns its rect and whether
/// it was clicked.
#[allow(clippy::too_many_arguments)]
fn text_key(
    ui: &egui::Ui,
    g: &egui::Painter,
    (x, from_right): (f32, bool),
    cy: f32,
    label: &str,
    cap: Cap,
    enabled: bool,
    key: &str,
) -> (Rect, bool) {
    let shown = label.to_uppercase();
    let w = hw::galley(g, &shown, Face::Bold, 12.0, theme::INK).size().x + 36.0;
    let left = if from_right { x - w } else { x };
    let r = Rect::from_min_size(pos2(left, cy - 15.0), vec2(w, 30.0));
    if !enabled {
        let face = hw::paint_key(g, r, Cap::Bone, false, false);
        hw::text(
            g,
            face.center(),
            Align2::CENTER_CENTER,
            &shown,
            Face::Bold,
            12.0,
            theme::PRINT.gamma_multiply(0.6),
        );
        return (r, false);
    }
    let (resp, _) = hw::key(
        ui,
        g,
        r,
        egui::Id::unique(("chooser_key", key)),
        cap,
        false,
        None,
        Legend::Text(&shown),
    );
    (r, resp.clicked())
}

/// Draws the chooser over the whole window.
pub fn show(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(mut c) = s.chooser.clone() else {
        return;
    };
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, Sense::hover());
    let g = ui.painter().clone();
    g.rect_filled(area, 0.0, theme::GRID_BG);

    // Sidebar: the wordmark, then a key per section; the open section's key is latched (a
    // black key, bone print, LED lit).
    let side = Rect::from_min_size(area.min, vec2(232.0, area.height()));
    hw::chassis(&g, side);
    hw::seam_v(&g, side.right() - 3.0, side.y_range());
    // The wordmark, set smaller rather than cut when the name is long, the orange square,
    // and the model line under it.
    let word = crate::APP_NAME
        .split_once(" (")
        .map_or(crate::APP_NAME, |(n, _)| n)
        .to_uppercase();
    let room = side.width() - 70.0;
    let mut size = 16.0;
    while size > 11.0
        && hw::galley(&g, &word, Face::SilkX, size, theme::INK)
            .size()
            .x
            > room
    {
        size -= 0.5;
    }
    let wm = hw::text(
        &g,
        side.left_top() + vec2(22.0, 24.0),
        Align2::LEFT_CENTER,
        &hw::fit(&g, &word, Face::SilkX, size, room),
        Face::SilkX,
        size,
        theme::INK,
    );
    g.rect_filled(
        Rect::from_center_size(pos2(wm.right() + 20.0, wm.center().y), vec2(10.0, 10.0)),
        1.0,
        theme::ORANGE,
    );
    hw::silk(
        &g,
        side.left_top() + vec2(22.0, 42.0),
        Align2::LEFT_CENTER,
        "PW-1 Digital Audio Workstation",
        9.5,
        theme::INK2,
    );
    hw::seam_h(&g, side.left()..=side.right() - 3.0, side.top() + 58.0);
    for (cx, cy, a) in [
        (side.left() + 9.0, side.bottom() - 9.0, -30.0),
        (side.right() - 13.0, side.bottom() - 9.0, 45.0),
    ] {
        hw::screw(&g, pos2(cx, cy), 4.5, a);
    }
    for (k, sec) in Section::ALL.into_iter().enumerate() {
        let r = Rect::from_min_size(
            side.left_top() + vec2(18.0, 78.0 + k as f32 * 40.0),
            vec2(side.width() - 39.0, 29.0),
        );
        let on = c.section == sec;
        let (resp, body) = hw::key(
            ui,
            &g,
            r,
            egui::Id::unique(("chooser_section", k)),
            if on { Cap::Dark } else { Cap::Bone },
            on,
            Some(on),
            Legend::Custom,
        );
        let ink = if on { theme::ON_DARK } else { theme::INK };
        let label = hw::fit(
            &g,
            &sec.label().to_uppercase(),
            Face::Bold,
            11.5,
            body.width() - 12.0,
        );
        hw::text(
            &g,
            pos2(body.left() + 6.0, body.center().y),
            Align2::LEFT_CENTER,
            &label,
            Face::Bold,
            11.5,
            ink,
        );
        if resp.clicked() && c.section != sec {
            c.section = sec;
            c.selected = 0;
        }
    }

    // Content: a faceplate screwed into the floor, its section named by a bracket.
    let main = Rect::from_min_max(pos2(side.right(), area.top()), area.max);
    let footer_h = 60.0;
    let plate = Rect::from_min_max(
        main.min + vec2(18.0, 18.0),
        pos2(main.right() - 18.0, main.bottom() - footer_h - 14.0),
    );
    hw::faceplate(&g, plate, 4.0, false);
    for (cx, cy, a) in [
        (plate.left() + 10.0, plate.top() + 10.0, 20.0),
        (plate.right() - 10.0, plate.top() + 10.0, -40.0),
        (plate.left() + 10.0, plate.bottom() - 10.0, 60.0),
        (plate.right() - 10.0, plate.bottom() - 10.0, 5.0),
    ] {
        hw::screw(&g, pos2(cx, cy), 4.5, a);
    }
    let body = Rect::from_min_max(
        plate.min + vec2(30.0, 18.0),
        pos2(plate.right() - 30.0, plate.bottom() - 24.0),
    );
    hw_transport::bracket(
        &g,
        body.left(),
        body.right(),
        body.top() + 6.0,
        c.section.label(),
        13.0,
    );
    let list_top = body.top() + 32.0;
    let recent: Vec<PathBuf> = s.recent.paths().to_vec();
    let mut chosen = None;
    match c.section {
        Section::Recent => {
            if recent.is_empty() {
                hw::text(
                    &g,
                    pos2(body.left(), list_top),
                    Align2::LEFT_TOP,
                    tr("No recent projects. Start one from New Project or Project Templates."),
                    Face::Body,
                    13.0,
                    theme::INK2,
                );
            }
            for (k, path) in recent.iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(body.left(), list_top + k as f32 * 52.0),
                    vec2(body.width(), 44.0),
                );
                let resp = recent_row(ui, &g, r, k, path, c.selected == k);
                if resp.clicked() {
                    c.selected = k;
                }
                if resp.double_clicked() {
                    chosen = Some(Choice::Recent(path.clone()));
                }
            }
        }
        // The sections of tiles.
        _ => {
            let items = c.section.items();
            let (w, h, gap) = (190.0, 184.0, 18.0);
            let per_row = (((body.width() + gap) / (w + gap)).floor() as usize).max(1);
            // A short section fills one row with wider tiles instead of leaving a gap.
            let (per_row, w) = if items.len() <= 4 && body.width() >= 4.0 * 160.0 + 3.0 * gap {
                let n = 4usize;
                (
                    n,
                    ((body.width() - (n - 1) as f32 * gap) / n as f32).min(240.0),
                )
            } else {
                (per_row, w)
            };
            for (k, t) in items.iter().enumerate() {
                let (row, col) = (k / per_row, k % per_row);
                let r = Rect::from_min_size(
                    pos2(
                        body.left() + col as f32 * (w + gap),
                        list_top + row as f32 * (h + gap),
                    ),
                    vec2(w, h),
                );
                let (clicked, double) = tile(ui, r, *t, c.selected == k, k);
                if clicked {
                    c.selected = k;
                }
                if double {
                    chosen = Some(Choice::Template(*t));
                }
            }
        }
    }

    // Footer: Open… on the left; Cancel and Choose (the orange key) on the right.
    let footer = Rect::from_min_max(pos2(main.left(), main.bottom() - footer_h), main.max);
    hw::chassis(&g, footer);
    hw::seam_h(&g, footer.x_range(), footer.top());
    let cy = footer.center().y;
    let mut cancel = false;
    let open = text_key(
        ui,
        &g,
        (footer.left() + 24.0, false),
        cy,
        tr("Open an existing project…"),
        Cap::Bone,
        true,
        "open",
    );
    if open.1 {
        s.file_cmd = Some(FileCmd::Open);
    }
    let can_choose = choice(&c, &recent).is_some();
    let (choose_r, choose_hit) = text_key(
        ui,
        &g,
        (footer.right() - 24.0, true),
        cy,
        tr("Choose"),
        Cap::Orange,
        can_choose,
        "choose",
    );
    if choose_hit {
        chosen = choice(&c, &recent);
    }
    let back = text_key(
        ui,
        &g,
        (choose_r.left() - 10.0, true),
        cy,
        tr("Cancel"),
        Cap::Bone,
        true,
        "cancel",
    );
    if back.1 {
        cancel = true;
    }

    // Return chooses, Escape goes back to the open project.
    if !ui.ctx().egui_wants_keyboard_input() {
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            chosen = chosen.or_else(|| choice(&c, &recent));
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            cancel = true;
        }
    }

    if cancel {
        s.chooser = None;
        return;
    }
    // The chooser stays up until the File command has opened a project, so a cancelled
    // "save changes?" prompt or a failed open lands back here.
    s.chooser = Some(c);
    if let Some(choice) = chosen {
        choose(choice, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_start_untitled_projects_with_their_tracks() {
        for t in Template::ALL {
            let p = t.project();
            assert_eq!(p.name, "Untitled", "{t:?}");
            assert!(!p.tracks.is_empty(), "{t:?}");
            assert_eq!(p.selected_track, 0);
        }
        assert_eq!(Template::Empty.project().tracks.len(), 1);
        let kinds: Vec<_> = Template::Electronic
            .project()
            .tracks
            .iter()
            .map(|t| t.kind)
            .collect();
        assert!(kinds.contains(&TrackKind::Aux) && kinds.contains(&TrackKind::Drummer));
    }

    /// Pictures of the chooser: the templates and the recent projects (`PROWAVE_UI_PNG_DIR`,
    /// else the temp folder).
    #[test]
    #[ignore = "pictures"]
    fn pictures_project_chooser() {
        let dir = std::env::var_os("PROWAVE_UI_PNG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-transport"));
        let _ = std::fs::create_dir_all(&dir);
        for (section, selected, name) in [
            (Section::Templates, 1, "chooser-templates.png"),
            (Section::Recent, 1, "chooser-recent.png"),
        ] {
            let mut s = UiState::new();
            // An in-memory list (UiState::new never touches the saved one). Two of the
            // projects exist, so their rows show the date they were saved.
            let saved = std::env::temp_dir()
                .join("prowave-chooser")
                .join("Pro Wave");
            let _ = std::fs::create_dir_all(&saved);
            let night = saved.join("Night Drive.prowave");
            let beat = saved.join("Beat Tape 4.prowave");
            for f in [&night, &beat] {
                let _ = std::fs::write(f, b"");
            }
            for p in [
                night.as_path(),
                beat.as_path(),
                std::path::Path::new("/Users/me/Desktop/Sketch 12.prowave"),
            ] {
                s.recent.add(p);
            }
            s.chooser = Some(Chooser { section, selected });
            crate::ui_test::render_png(
                1100.0,
                620.0,
                2.0,
                theme::BONE,
                |ui| {
                    theme::apply(ui.ctx());
                    show(ui, &mut s)
                },
                &dir.join(name),
            )
            .unwrap();
        }
    }

    #[test]
    fn saved_dates_are_civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_399), (2025, 11, 7));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }

    #[test]
    fn choosing_queues_the_matching_file_command() {
        let recent = vec![PathBuf::from("/music/Night Drive.prowave")];
        let mut c = Chooser::templates();
        c.selected = 1;
        assert_eq!(
            choice(&c, &recent),
            Some(Choice::Template(Template::Songwriter))
        );
        c.section = Section::Recent;
        c.selected = 0;
        let mut s = UiState::new();
        choose(choice(&c, &recent).unwrap(), &mut s);
        assert_eq!(s.file_cmd, Some(FileCmd::OpenRecent));
        assert_eq!(s.recent_pick, Some(recent[0].clone()));

        c.selected = 3;
        assert_eq!(choice(&c, &recent), None, "nothing to choose past the list");
        c.section = Section::NewProject;
        choose(choice(&c, &[]).unwrap(), &mut s);
        assert_eq!(s.file_cmd, Some(FileCmd::NewFrom(Template::Empty)));
    }
}
