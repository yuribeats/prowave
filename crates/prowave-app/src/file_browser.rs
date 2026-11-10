//! The Loop Browser and the All Files browser (the two buttons at the right of the control bar):
//! a folder list; double-click a file to add it to the project.
//!
//! The Loop Browser shows the audio files of a folder of your choice, which it remembers. For
//! Apple Loops (`.caf`) it also reads the tags (tempo, key, category) on a background thread,
//! shows them under the file name and lets the search find them. It does not yet stretch a
//! loop to the project's tempo and key.
use crate::apple_loops::{self, LoopTags};
use crate::i18n::tr;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

/// Width of the pane.
pub const WIDTH: f32 = 280.0;
/// A folder with more entries than this shows only these.
const MAX_ENTRIES: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Loops,
    Files,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    name: String,
    folder: bool,
}

/// A background read of the Apple Loops tags of the files in a folder.
struct TagScan {
    results: Receiver<(PathBuf, Option<LoopTags>)>,
    /// Set when the folder on show changes, so the thread stops early.
    cancel: Arc<AtomicBool>,
}

impl TagScan {
    /// Starts reading the tags of `paths`.
    fn start(paths: Vec<PathBuf>) -> Option<Self> {
        let (tx, results) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("loop-tags".into())
            .spawn(move || {
                for path in paths {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let tags = apple_loops::read_file(&path);
                    if tx.send((path, tags)).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self { results, cancel })
    }
}

impl Drop for TagScan {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct BrowserView {
    /// The pane on show, if any.
    pub open: Option<Tab>,
    dir: Option<PathBuf>,
    /// What `entries` lists.
    listed: Option<(Tab, PathBuf)>,
    entries: Vec<Entry>,
    search: String,
    /// The Apple Loops tags read so far (`None`: the file has none), and the read still running.
    tags: HashMap<PathBuf, Option<LoopTags>>,
    scan: Option<TagScan>,
}

impl BrowserView {
    /// A control bar button: opens its pane, or closes it when it is the one on show.
    pub fn toggle(&mut self, tab: Tab) {
        self.open = (self.open != Some(tab)).then_some(tab);
        self.dir = None;
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn loop_folder_file() -> Option<PathBuf> {
    crate::recent_files::config_dir().map(|d| d.join("loop-folder"))
}

/// Where a pane opens: the remembered loop folder, else the Music folder or the home folder.
fn start_dir(tab: Tab) -> PathBuf {
    let saved = (tab == Tab::Loops)
        .then(loop_folder_file)
        .flatten()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|text| PathBuf::from(text.trim()))
        .filter(|d| d.is_dir());
    saved.unwrap_or_else(|| {
        let home = home().unwrap_or_else(|| PathBuf::from("."));
        let music = home.join("Music");
        if tab == Tab::Loops && music.is_dir() {
            music
        } else {
            home
        }
    })
}

fn wanted(tab: Tab, path: &Path) -> bool {
    crate::import::is_audio_path(path) || tab == Tab::Files && crate::midi_file::is_midi_path(path)
}

/// The folders and the files `tab` can add, folders first, each sorted by name. Hidden entries
/// are left out. A folder that cannot be read lists nothing.
fn list(dir: &Path, tab: Tab) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = read
        .flatten()
        .take(MAX_ENTRIES)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let folder = path.is_dir();
            (!name.starts_with('.') && (folder || wanted(tab, &path))).then_some(Entry {
                path,
                name,
                folder,
            })
        })
        .collect();
    entries.sort_by_key(|e| (!e.folder, e.name.to_lowercase()));
    entries
}

fn matches(name: &str, search: &str) -> bool {
    name.to_lowercase().contains(&search.trim().to_lowercase())
}

/// Whether a file shows for `search`: its name, or any of its Apple Loops tags, matches.
fn entry_matches(name: &str, tags: Option<&LoopTags>, search: &str) -> bool {
    matches(name, search) || tags.is_some_and(|t| t.matches(&search.trim().to_lowercase()))
}

pub fn show(ui: &mut egui::Ui, s: &mut crate::UiState) {
    let v = &mut s.browser;
    let Some(tab) = v.open else { return };
    let dir = v.dir.get_or_insert_with(|| start_dir(tab)).clone();
    if v.listed.as_ref() != Some(&(tab, dir.clone())) {
        v.entries = list(&dir, tab);
        v.listed = Some((tab, dir.clone()));
        // Read the tags of the loops of the new folder in the background.
        v.tags.clear();
        let loops = v
            .entries
            .iter()
            .filter(|e| !e.folder && tab == Tab::Loops)
            .map(|e| e.path.clone())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("caf")))
            .collect::<Vec<_>>();
        v.scan = if loops.is_empty() {
            None
        } else {
            TagScan::start(loops)
        };
    }
    if let Some(scan) = &v.scan {
        loop {
            match scan.results.try_recv() {
                Ok((path, tags)) => {
                    v.tags.insert(path, tags);
                }
                Err(TryRecvError::Empty) => {
                    ui.ctx().request_repaint();
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    v.scan = None;
                    break;
                }
            }
        }
    }
    pane(ui, tab, &dir, v);
    let (enter, add) = rows(ui, tab, v);
    if enter.is_some() {
        v.dir = enter;
        v.search.clear();
    }
    s.open_paths.extend(add);
}

/// The side of the pane's padding.
const PAD: f32 = 12.0;
/// A list row, and one with a line of tags under the name.
const ROW_H: f32 = 30.0;
const ROW_TAGS_H: f32 = 46.0;

/// `path` as the LCD shows it: whole when it fits `room`, else its last folders after "…/".
fn path_tail(g: &egui::Painter, path: &Path, size: f32, room: f32) -> String {
    use crate::hw;
    use crate::theme::Face;
    let full = path.display().to_string();
    let w = |s: &str| {
        hw::galley(g, s, Face::Lcd, size, crate::theme::INK)
            .size()
            .x
    };
    if w(&full) <= room {
        return full;
    }
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    for skip in 1..parts.len() {
        let tail = format!("…/{}", parts[skip..].join("/"));
        if w(&tail) <= room {
            return tail;
        }
    }
    hw::fit(
        g,
        parts.last().map_or("", String::as_str),
        Face::Lcd,
        size,
        room,
    )
}

/// The top of the pane: the title strip with the number of entries on a small LCD, the Up and
/// Choose Folder keys, the search field and the folder on show.
fn pane(ui: &mut egui::Ui, tab: Tab, dir: &Path, v: &mut BrowserView) {
    use crate::hw::{self, Cap};
    use crate::midi_monitor::hw_utilities as hu;
    use crate::theme::{self, Face};
    use egui::{pos2, vec2, Align2, Rect, Sense};
    let g = ui.painter().clone();
    let all = ui.max_rect();
    hw::chassis(&g, all);
    hw::seam_v(&g, all.left(), all.y_range());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let w = ui.available_width();
    let id = egui::Id::unique(("browser", tab == Tab::Loops));

    // Title strip.
    let (head, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    hw::gradient(&g, head, 0.0, theme::BONE_TOP, theme::BONE);
    let title = match tab {
        Tab::Loops => tr("Loops"),
        Tab::Files => tr("All Files"),
    };
    let t = hw::text(
        &g,
        pos2(head.left() + PAD + 2.0, head.center().y),
        Align2::LEFT_CENTER,
        &title.to_uppercase(),
        Face::SilkX,
        14.0,
        theme::INK,
    );
    g.rect_filled(
        Rect::from_center_size(pos2(t.right() + 16.0, head.center().y), vec2(9.0, 9.0)),
        1.0,
        theme::ORANGE,
    );
    let count = v
        .entries
        .iter()
        .filter(|e| {
            entry_matches(
                &e.name,
                v.tags.get(&e.path).and_then(Option::as_ref),
                &v.search,
            )
        })
        .count()
        .min(9999);
    let lr = Rect::from_min_size(
        pos2(head.right() - PAD - 58.0, head.top() + 8.0),
        vec2(58.0, 24.0),
    );
    hw::lcd_glass(&g, lr);
    let digits = format!("{count:>4}");
    let dw = hw::seg_width(&digits, 13.0);
    hw::seg(
        &g,
        pos2(lr.right() - 7.0 - dw, lr.top() + 5.5),
        &digits,
        13.0,
        theme::LCD_INK,
        true,
    );
    hw::seam_h(&g, head.x_range(), head.bottom() - 2.0);

    // Up, and the loop folder.
    let (keys, _) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
    let up = Rect::from_min_size(pos2(keys.left() + PAD, keys.top() + 9.0), vec2(64.0, 28.0));
    let can_up = dir.parent().is_some();
    let resp = hu::key(ui, up, id.with("up"), Cap::Bone, false, None, "", can_up);
    {
        let mut p = g.clone();
        if !can_up {
            p.set_opacity(0.42);
        }
        hu::icon_up(&p, pos2(up.left() + 15.0, up.center().y), theme::INK);
        hw::text(
            &p,
            pos2(up.left() + 27.0, up.center().y),
            Align2::LEFT_CENTER,
            &tr("Up").to_uppercase(),
            Face::Bold,
            11.0,
            theme::INK,
        );
    }
    if resp.clicked() {
        v.dir = dir.parent().map(Path::to_path_buf);
    }
    if tab == Tab::Loops {
        let r = Rect::from_min_max(
            pos2(up.right() + 8.0, up.top()),
            pos2(keys.right() - PAD, up.bottom()),
        );
        if hu::key(
            ui,
            r,
            id.with("choose"),
            Cap::Bone,
            false,
            None,
            tr("Choose Folder…"),
            true,
        )
        .clicked()
        {
            if let Some(picked) = rfd::FileDialog::new().pick_folder() {
                if let Some(file) = loop_folder_file() {
                    let saved = file
                        .parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| std::fs::write(&file, picked.to_string_lossy().as_bytes()));
                    if let Err(e) = saved {
                        tracing::warn!("could not save the loop folder: {e}");
                    }
                }
                v.dir = Some(picked);
            }
        }
    }

    // Search.
    let (row, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
    let label = tr("Search");
    let lw = hw::silk_width(&g, label, 11.0).min(70.0);
    hw::silk(
        &g,
        pos2(row.left() + PAD, row.center().y - 2.0),
        Align2::LEFT_CENTER,
        label,
        11.0,
        theme::INK2,
    );
    let field = Rect::from_min_max(
        pos2(row.left() + PAD + lw + 10.0, row.top() + 2.0),
        pos2(row.right() - PAD, row.top() + 26.0),
    );
    hw::paint_field(&g, field, "", false, false);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink2(vec2(4.0, 3.0))));
    crate::hw_sampler::fields(&mut child, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut v.search)
                .frame(egui::Frame::NONE)
                .font(theme::font(Face::Bold, 11.0))
                .text_color(theme::ON_DARK)
                .desired_width(field.width() - 8.0),
        );
    });

    // The folder on show, on an LCD.
    let (row, _) = ui.allocate_exact_size(vec2(w, 50.0), Sense::hover());
    let glass = Rect::from_min_max(
        pos2(row.left() + PAD, row.top() + 6.0),
        pos2(row.right() - PAD, row.bottom() - 8.0),
    );
    hw::lcd_glass(&g, glass);
    hu::lcd_label(&g, glass.left_top() + vec2(7.0, 4.0), tr("Folder"));
    let tail = path_tail(&g, dir, 10.5, glass.width() - 14.0);
    hw::text(
        &g,
        pos2(glass.left() + 7.0, glass.bottom() - 10.0),
        Align2::LEFT_CENTER,
        &tail,
        Face::Lcd,
        10.5,
        theme::LCD_INK,
    );
    let (sep, _) = ui.allocate_exact_size(vec2(w, 2.0), Sense::hover());
    hw::seam_h(&g, sep.x_range(), sep.top());
}

/// The list: a recessed floor with a row a folder or a file. A click opens a folder, a
/// double-click adds a file. Returns the folder to open and the file to add.
fn rows(ui: &mut egui::Ui, tab: Tab, v: &BrowserView) -> (Option<PathBuf>, Option<PathBuf>) {
    use crate::hw;
    use crate::midi_monitor::hw_utilities as hu;
    use crate::theme::{self, Face};
    use egui::{pos2, vec2, Align2, Rect, Sense};
    let g = ui.painter().clone();
    let floor = Rect::from_min_max(ui.cursor().min, ui.max_rect().max);
    g.rect_filled(floor, 0.0, theme::BONE2);
    hw::inner_shadow(&g, floor, 0.0, 0.18);
    let mut enter = None;
    let mut add = None;
    egui::ScrollArea::vertical()
        .id_salt(("browser_rows", tab == Tab::Loops))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.add_space(4.0);
            for e in v.entries.iter().filter(|e| {
                let tags = v.tags.get(&e.path).and_then(Option::as_ref);
                entry_matches(&e.name, tags, &v.search)
            }) {
                let tags = v.tags.get(&e.path).and_then(Option::as_ref);
                let chips: Vec<String> = tags.map(LoopTags::parts).unwrap_or_default();
                let h = if chips.is_empty() { ROW_H } else { ROW_TAGS_H };
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
                let g = ui.painter();
                if resp.hovered() {
                    g.rect_filled(r.shrink2(vec2(4.0, 1.0)), 3.0, theme::BONE_HI);
                }
                let top = Rect::from_min_size(r.min, vec2(r.width(), ROW_H));
                let well = Rect::from_center_size(
                    pos2(r.left() + PAD + 11.0, top.center().y),
                    vec2(22.0, 22.0),
                );
                hu::well(g, well);
                let icon = well.shrink(5.0);
                let midi = crate::midi_file::is_midi_path(&e.path);
                if e.folder {
                    hu::icon_folder(g, icon, theme::INK);
                } else if midi {
                    hu::icon_midi(g, icon, theme::INK);
                } else {
                    hu::icon_wave(g, icon, theme::INK);
                }
                let name_x = well.right() + 9.0;
                let right = r.right() - PAD - if e.folder { 14.0 } else { 0.0 };
                let shown = hw::fit(g, &e.name, Face::Bold, 11.0, right - name_x);
                hw::text(
                    g,
                    pos2(name_x, top.center().y),
                    Align2::LEFT_CENTER,
                    &shown,
                    Face::Bold,
                    11.0,
                    theme::INK,
                );
                if e.folder {
                    hu::arrow(
                        g,
                        pos2(r.right() - PAD - 4.0, top.center().y),
                        8.0,
                        true,
                        theme::PRINT,
                    );
                }
                let mut x = name_x;
                for c in &chips {
                    let cr = hu::tag(g, pos2(x, top.bottom() + 5.0), c, 8.5);
                    x = cr.right() + 5.0;
                    if x > r.right() - PAD - 30.0 {
                        break;
                    }
                }
                g.hline(
                    r.left() + PAD..=r.right() - PAD,
                    r.bottom() - 0.5,
                    egui::Stroke::new(1.0, egui::Color32::from_black_alpha(22)),
                );
                if e.folder && resp.clicked() {
                    enter = Some(e.path.clone());
                } else if !e.folder && resp.double_clicked() {
                    add = Some(e.path.clone());
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        });
    (enter, add)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn folders_come_first_and_each_pane_lists_what_it_can_add() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["b.wav", "A.flac", "song.mid", "notes.txt", ".hidden.wav"] {
            std::fs::write(dir.path().join(f), b"").unwrap();
        }
        std::fs::create_dir(dir.path().join("Drums")).unwrap();
        assert_eq!(
            names(&list(dir.path(), Tab::Files)),
            ["Drums", "A.flac", "b.wav", "song.mid"]
        );
        assert_eq!(
            names(&list(dir.path(), Tab::Loops)),
            ["Drums", "A.flac", "b.wav"],
            "loops are audio"
        );
    }

    #[test]
    fn a_missing_folder_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(&dir.path().join("gone"), Tab::Files).is_empty());
    }

    #[test]
    fn the_buttons_open_and_close_their_pane() {
        let mut v = BrowserView::default();
        v.toggle(Tab::Loops);
        assert_eq!(v.open, Some(Tab::Loops));
        v.toggle(Tab::Files);
        assert_eq!(v.open, Some(Tab::Files), "the other pane takes over");
        v.toggle(Tab::Files);
        assert_eq!(v.open, None);
    }

    #[test]
    fn search_ignores_case_and_spaces() {
        assert!(matches("Funky Drums.wav", " drums "));
        assert!(matches("x.wav", ""));
        assert!(!matches("x.wav", "bass"));
    }

    /// A minimal Apple Loop: a CAF file whose metadata chunk names a category and key.
    fn loop_bytes(category: &str) -> Vec<u8> {
        let mut body = vec![
            0x29, 0x81, 0x92, 0x73, 0xb5, 0xbf, 0x4a, 0xef, 0xb7, 0x8d, 0x62, 0xd1, 0xef, 0x90,
            0xbb, 0x2c,
        ];
        body.extend_from_slice(&2u32.to_be_bytes());
        for text in ["category", category, "key signature", "G"] {
            body.extend_from_slice(text.as_bytes());
            body.push(0);
        }
        let mut file = b"caff\0\x01\0\0uuid".to_vec();
        file.extend_from_slice(&(body.len() as i64).to_be_bytes());
        file.extend(body);
        file
    }

    #[test]
    fn a_file_shows_for_a_search_that_matches_its_name_or_its_tags() {
        let tags = LoopTags {
            category: "Bass".into(),
            key: "G".into(),
            ..LoopTags::default()
        };
        assert!(entry_matches("Funky.caf", Some(&tags), "funk"));
        assert!(entry_matches("Funky.caf", Some(&tags), " BASS "));
        assert!(!entry_matches("Funky.caf", Some(&tags), "drums"));
        assert!(
            !entry_matches("Funky.caf", None, "bass"),
            "no tags, no match"
        );
        assert!(entry_matches("Funky.caf", None, ""));
    }

    #[test]
    fn the_tags_of_the_loops_in_a_folder_are_read_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let bass = dir.path().join("Bass One.caf");
        let plain = dir.path().join("plain.caf");
        std::fs::write(&bass, loop_bytes("Bass")).unwrap();
        std::fs::write(&plain, b"not a caf").unwrap();
        let scan = TagScan::start(vec![bass.clone(), plain.clone()]).unwrap();
        let mut found = HashMap::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while found.len() < 2 && std::time::Instant::now() < deadline {
            match scan.results.try_recv() {
                Ok((path, tags)) => {
                    found.insert(path, tags);
                }
                Err(TryRecvError::Empty) => std::thread::sleep(std::time::Duration::from_millis(5)),
                Err(TryRecvError::Disconnected) => break,
            }
        }
        assert_eq!(
            found[&bass].as_ref().map(|t| t.category.as_str()),
            Some("Bass")
        );
        assert_eq!(found[&plain], None, "a file without the tags has none");
    }

    #[test]
    fn the_loop_browser_draws_a_folder_of_loops_and_finds_them_by_tag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Groove.caf"), loop_bytes("Bass")).unwrap();
        std::fs::write(dir.path().join("Hits.caf"), loop_bytes("Drums")).unwrap();
        let mut s = crate::UiState::new();
        s.browser.open = Some(Tab::Loops);
        s.browser.dir = Some(dir.path().to_path_buf());
        let mut h = crate::ui_test::Harness::new(300.0, 600.0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while s.browser.tags.len() < 2 && std::time::Instant::now() < deadline {
            h.frame(vec![], |ui| show(ui, &mut s));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(s.browser.tags.len(), 2, "both loops were read");
        let drums: Vec<&str> = s
            .browser
            .entries
            .iter()
            .filter(|e| {
                let tags = s.browser.tags.get(&e.path).and_then(Option::as_ref);
                entry_matches(&e.name, tags, "drums")
            })
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(drums, ["Hits.caf"], "found by its category, not its name");
    }

    /// `loop-browser.png` and `all-files.png`: the pane on a folder of loops and files.
    #[test]
    #[ignore = "pictures"]
    fn picture_browser() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Loops").join("Funk Kit");
        std::fs::create_dir_all(&root).unwrap();
        for d in ["Drums", "Keys and Organs"] {
            std::fs::create_dir(root.join(d)).unwrap();
        }
        std::fs::write(root.join("Groove Bass.caf"), loop_bytes("Bass")).unwrap();
        std::fs::write(root.join("Tight Hats.caf"), loop_bytes("Drums")).unwrap();
        for f in [
            "Clav Stab 02.wav",
            "Rhodes Chords in G Minor With Tape Wobble.aif",
            "verse.mid",
        ] {
            std::fs::write(root.join(f), b"").unwrap();
        }
        for (tab, name) in [
            (Tab::Loops, "loop-browser.png"),
            (Tab::Files, "all-files.png"),
        ] {
            let mut s = crate::UiState::new();
            s.browser.open = Some(tab);
            s.browser.dir = Some(root.clone());
            let mut h = crate::ui_test::Harness::new(WIDTH, 700.0);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while (tab == Tab::Loops && s.browser.tags.len() < 2 || s.browser.listed.is_none())
                && std::time::Instant::now() < deadline
            {
                h.frame(vec![], |ui| show(ui, &mut s));
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            crate::ui_test::render_png(
                WIDTH,
                700.0,
                2.0,
                crate::theme::BONE,
                |ui| {
                    crate::theme::apply(ui.ctx());
                    show(ui, &mut s);
                },
                &crate::midi_monitor::hw_utilities::picture_path(name),
            )
            .unwrap();
        }
    }
}
