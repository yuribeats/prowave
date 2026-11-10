//! File > New… / Open… / Open Recent / Save / Save As… / Save A Copy As… / Revert to Saved:
//! native dialogs (rfd), key commands and the "save changes?" and "revert?" prompts.
use crate::i18n::{tr, trf};
use crate::model::Project;
use crate::start_screen::Template;
use crate::{project_io, App};
use egui::{Key, KeyboardShortcut, Modifiers};
use prowave_project::ProjectDoc;
use rfd::FileDialog;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileCmd {
    New,
    /// Closes the project and leaves a new, empty one.
    Close,
    Open,
    Save,
    SaveAs,
    /// Writes the project to another file, which does not become the current one.
    SaveCopy,
    /// Reloads the project from its file, after asking.
    Revert,
    /// Opens `UiState::recent_pick` (File > Open Recent).
    OpenRecent,
    /// Starts a new project from a template (Project Chooser).
    NewFrom(Template),
}

impl FileCmd {
    /// Save As before Save: egui ignores an extra Shift when matching Cmd+S.
    const ALL: [FileCmd; 4] = [FileCmd::SaveAs, FileCmd::Save, FileCmd::Open, FileCmd::New];

    /// Logic's key commands: Cmd+N, Cmd+O, Cmd+S, Shift+Cmd+S (Ctrl on Windows/Linux). The
    /// other commands are menu-only.
    pub fn shortcut(self) -> Option<KeyboardShortcut> {
        let (modifiers, key) = match self {
            FileCmd::New => (Modifiers::COMMAND, Key::N),
            FileCmd::Open => (Modifiers::COMMAND, Key::O),
            FileCmd::Save => (Modifiers::COMMAND, Key::S),
            FileCmd::SaveAs => (Modifiers::COMMAND | Modifiers::SHIFT, Key::S),
            FileCmd::Close
            | FileCmd::SaveCopy
            | FileCmd::Revert
            | FileCmd::OpenRecent
            | FileCmd::NewFrom(_) => return None,
        };
        Some(KeyboardShortcut::new(modifiers, key))
    }

    /// The File menu entry that runs this command.
    pub fn from_menu(label: &str) -> Option<Self> {
        match label {
            "New…" => Some(FileCmd::New),
            "Close Project" => Some(FileCmd::Close),
            "Open…" => Some(FileCmd::Open),
            "Save" => Some(FileCmd::Save),
            "Save As…" => Some(FileCmd::SaveAs),
            "Save A Copy As…" => Some(FileCmd::SaveCopy),
            "Revert to Saved" => Some(FileCmd::Revert),
            _ => None,
        }
    }
}

/// The file command whose key command was pressed this frame, if any (consumes the keys).
pub fn pressed(ctx: &egui::Context) -> Option<FileCmd> {
    FileCmd::ALL.into_iter().find(|c| {
        c.shortcut()
            .is_some_and(|s| ctx.input_mut(|i| i.consume_shortcut(&s)))
    })
}

/// What gets saved, minus state that changes on its own (playhead, selected track), so that
/// playing or clicking around does not count as an unsaved change.
pub fn content(p: &Project) -> ProjectDoc {
    let mut doc = project_io::to_doc(p).0;
    doc.position_beats = 0.0;
    doc.selected_track = 0;
    doc
}

/// Whether the project has changes that are not in its file: what every "save changes?"
/// question asks (New, Open, Open Recent, Close, Revert, the updater; a prompt on Quit would
/// ask it too). The project's own data is compared with what was last saved or opened, and the
/// running plug-ins are asked whether their settings changed, because a knob turned in a
/// plug-in's window changes nothing in the project until its settings are read.
pub fn unsaved(p: &Project, saved: &ProjectDoc, plugins: &crate::plugins::PluginHost) -> bool {
    content(p) != *saved || plugins.settings_changed()
}

/// [`unsaved`] for a caller that asks every frame (the updater's window): reading a big
/// plug-in's settings takes milliseconds, so the plug-ins are asked at most twice a second.
pub fn unsaved_polled(
    p: &Project,
    saved: &ProjectDoc,
    plugins: &crate::plugins::PluginHost,
) -> bool {
    content(p) != *saved || plugins.settings_changed_polled()
}

fn dialog() -> FileDialog {
    FileDialog::new().add_filter(tr("Pro Wave Project"), &[prowave_project::EXTENSION])
}

/// The answer to the "save changes?" prompt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Answer {
    Save,
    DontSave,
    Cancel,
}

impl App {
    /// Runs a File menu command. New / Open / Open Recent first ask what to do with unsaved
    /// changes, and Revert asks before throwing them away.
    pub fn run_file_cmd(&mut self, cmd: FileCmd, ctx: &egui::Context) {
        let discards = matches!(
            cmd,
            FileCmd::New
                | FileCmd::Close
                | FileCmd::Open
                | FileCmd::OpenRecent
                | FileCmd::NewFrom(_)
        );
        if cmd == FileCmd::Revert {
            self.request_revert();
        } else if discards && self.has_unsaved_changes() {
            self.ui.ask_save = Some(cmd);
        } else {
            self.do_file_cmd(cmd, ctx);
        }
        ctx.request_repaint();
    }

    /// [`unsaved`] for this window's project.
    pub fn has_unsaved_changes(&self) -> bool {
        unsaved(&self.project, &self.saved, &self.plugins)
    }

    fn do_file_cmd(&mut self, cmd: FileCmd, ctx: &egui::Context) {
        match cmd {
            FileCmd::New | FileCmd::Close => self.replace(Project::empty(), None),
            FileCmd::NewFrom(t) => self.replace(t.project(), None),
            FileCmd::Open => {
                if let Some(path) = dialog().set_title(tr("Open Project")).pick_file() {
                    self.open(&path, ctx);
                }
            }
            FileCmd::Save => {
                self.save();
            }
            FileCmd::SaveAs => {
                self.save_as();
            }
            FileCmd::SaveCopy => {
                self.save_copy();
            }
            FileCmd::OpenRecent => {
                if let Some(path) = self.ui.recent_pick.take() {
                    self.open(&path, ctx);
                }
            }
            FileCmd::Revert => self.revert(ctx),
        }
    }

    /// The "Do you want to save the changes…?" prompt. Drawn by the app itself (not a system
    /// message box), so it looks the same everywhere and needs no helper program on Linux.
    pub fn show_save_prompt(&mut self, ctx: &egui::Context) {
        let Some(cmd) = self.ui.ask_save else {
            return;
        };
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::unique(("file", "save_prompt"))).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(crate::APP_NAME);
            ui.add_space(6.0);
            ui.label(trf(
                "Do you want to save the changes made to the project \"{}\"?",
                &[&self.project.name],
            ));
            ui.label(tr("Your changes will be lost if you don't save them."));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(tr("Don't Save")).clicked() {
                    answer = Some(Answer::DontSave);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Save")).clicked() {
                        answer = Some(Answer::Save);
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        answer = Some(Answer::Cancel);
                    }
                });
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(Answer::Cancel);
        }
        let Some(answer) = answer else {
            return;
        };
        self.ui.ask_save = None;
        let proceed = match answer {
            Answer::Save => self.save(),
            Answer::DontSave => true,
            Answer::Cancel => false,
        };
        if proceed {
            self.do_file_cmd(cmd, ctx);
        } else {
            self.ui.recent_pick = None;
        }
        ctx.request_repaint();
    }

    /// The "revert to the last saved version?" prompt, drawn like the save prompt.
    pub fn show_revert_prompt(&mut self, ctx: &egui::Context) {
        if !self.ui.ask_revert {
            return;
        }
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::unique(("file", "revert_prompt"))).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(crate::APP_NAME);
            ui.add_space(6.0);
            ui.label(trf(
                "Do you want to revert the project \"{}\" to the last saved version?",
                &[&self.project.name],
            ));
            ui.label(tr("All changes made since it was saved will be lost."));
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Revert")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr("Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let Some(revert) = answer else {
            return;
        };
        self.ui.ask_revert = false;
        if revert {
            self.revert(ctx);
        }
        ctx.request_repaint();
    }

    /// Revert needs a saved file and something to throw away; otherwise it says why not.
    fn request_revert(&mut self) {
        if self.path.is_none() {
            self.ui.status =
                Some(tr("nothing to revert to: the project has not been saved yet").into());
        } else if !self.has_unsaved_changes() {
            self.ui.status = Some(tr("no unsaved changes to revert").into());
        } else {
            self.ui.ask_revert = true;
        }
    }

    /// Reloads the project from its file, dropping unsaved changes (no prompt).
    fn revert(&mut self, ctx: &egui::Context) {
        if let Some(path) = self.path.clone() {
            self.open(&path, ctx);
        }
    }

    /// Opens `path`, replacing the current project (no prompt; callers ask first).
    pub fn open(&mut self, path: &Path, ctx: &egui::Context) {
        match project_io::open_project(path, ctx) {
            Ok((p, warnings)) => {
                self.replace(p, Some(path.to_path_buf()));
                self.ui.recent.add(path);
                if !warnings.is_empty() {
                    self.ui.status = Some(trf("missing media: {}", &[&warnings.join(", ")]));
                }
                self.ui.missing_audio = warnings;
            }
            Err(e) => {
                self.ui.recent.remove(path);
                self.ui.status = Some(trf("cannot open {}: {}", &[&path.display(), &e]));
            }
        }
    }

    /// Save to the current file, or ask for one. Returns false if cancelled or failed.
    fn save(&mut self) -> bool {
        match self.path.clone() {
            Some(path) => self.save_to(&path),
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) -> bool {
        let name = format!("{}.{}", self.project.name, prowave_project::EXTENSION);
        match dialog()
            .set_title(tr("Save As"))
            .set_file_name(name)
            .save_file()
        {
            Some(path) => self.save_to(&path),
            None => false,
        }
    }

    /// Save A Copy As…: writes the project elsewhere and keeps working in the current file.
    fn save_copy(&mut self) -> bool {
        let name = format!("{} copy.{}", self.project.name, prowave_project::EXTENSION);
        match dialog()
            .set_title(tr("Save A Copy As"))
            .set_file_name(name)
            .save_file()
        {
            Some(path) => self.save_copy_to(&path),
            None => false,
        }
    }

    /// Writes the project to `path` without making it the current file: the title, the unsaved
    /// state and Open Recent stay as they were.
    pub fn save_copy_to(&mut self, path: &Path) -> bool {
        self.plugins.save_states(&mut self.project);
        match project_io::write_project(&self.project, path, false) {
            Ok(project_io::Saved { path, missing }) => {
                self.ui.status = Some(trf("saved a copy: {}", &[&path.display()]));
                self.report_missing(&missing);
                true
            }
            Err(e) => {
                self.ui.status = Some(trf("cannot save {}: {}", &[&path.display(), &e]));
                false
            }
        }
    }

    pub fn save_to(&mut self, path: &Path) -> bool {
        self.plugins.save_states(&mut self.project);
        match project_io::write_project(&self.project, path, true) {
            Ok(project_io::Saved { path, missing }) => {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    self.project.name = stem.to_string();
                }
                self.ui.recent.add(&path);
                self.path = Some(path);
                self.saved = content(&self.project);
                self.ui.status = None;
                self.report_missing(&missing);
                true
            }
            Err(e) => {
                self.ui.status = Some(trf("cannot save {}: {}", &[&path.display(), &e]));
                false
            }
        }
    }

    /// After a save: the media files that could not be read and are not in the saved file are
    /// listed in a window (and the status line), by name or path.
    fn report_missing(&mut self, missing: &[PathBuf]) {
        if missing.is_empty() {
            return;
        }
        let names: Vec<String> = missing.iter().map(|m| project_io::shown_media(m)).collect();
        self.ui.status = Some(trf("missing media: {}", &[&names.join(", ")]));
        self.ui.missing_audio = names;
    }

    /// The window that lists audio files that could not be read (after opening or saving a
    /// project), until it is closed. Drawn like the save prompt.
    pub fn show_missing_audio(&mut self, ctx: &egui::Context) {
        if self.ui.missing_audio.is_empty() {
            return;
        }
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique(("file", "missing_audio"))).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(crate::APP_NAME);
            ui.add_space(6.0);
            ui.label(tr("These audio files are missing:"));
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .show(ui, |ui| {
                    for name in &self.ui.missing_audio {
                        ui.label(name);
                    }
                });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("OK")).clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.ui.missing_audio.clear();
            ctx.request_repaint();
        }
    }

    /// Swaps in a whole new project and resets editor state that indexes into the old one.
    fn replace(&mut self, mut p: Project, path: Option<PathBuf>) {
        // A new song revision makes `EngineSync` rebuild and swap the engine's song.
        p.song_rev = self.project.song_rev.wrapping_add(1);
        self.project = p;
        self.path = path;
        self.saved = content(&self.project);
        self.ui.reset_views();
        // Undo stays within one project.
        self.history.reset(&self.project);
        // A project is open now, so the Project Chooser has done its job.
        self.ui.chooser = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn app() -> App {
        let project = Project::demo();
        App {
            saved: content(&project),
            history: crate::undo::History::new(&project),
            project,
            ui: crate::UiState::new(),
            sync: None,
            plugins: crate::plugins::PluginHost::default(),
            path: None,
            native_menu: None,
            bench: None,
            shots: None,
            frames_since_record: 0,
            midi: None,
            midi_out: None,
            mtc_in: None,
            clock_in: crate::clock_in::ClockIn::default(),
            clock_follow: crate::clock_follow::ClockFollower::default(),
            chase: crate::mtc_in::Chase::default(),
            updater: crate::updater::Updater::new(),
        }
    }

    #[test]
    fn a_knob_turned_in_a_plugin_is_an_unsaved_change_for_every_command_that_asks() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut a = app();
        let (p, host, history, _sink, uid) = crate::plugins::tests::project_with_fake(b"saved");
        (a.project, a.plugins, a.history) = (p, host, history);
        assert!(a.save_to(&dir.path().join("Song")));
        let file = dir.path().join("Song.prowave");
        assert!(!a.has_unsaved_changes());
        a.run_file_cmd(FileCmd::Revert, &ctx);
        assert!(!a.ui.ask_revert, "nothing to revert");

        // Only a knob in the plug-in's own window: the project's data is as saved.
        a.plugins.fake(uid).unwrap().turn_knob(b"turned");
        assert_eq!(content(&a.project), a.saved);
        assert!(a.has_unsaved_changes());
        assert!(unsaved(&a.project, &a.saved, &a.plugins));
        let tracks = a.project.tracks.len();
        for cmd in [
            FileCmd::New,
            FileCmd::Close,
            FileCmd::Open,
            FileCmd::OpenRecent,
            FileCmd::NewFrom(Template::ALL[0]),
        ] {
            a.ui.ask_save = None;
            a.ui.recent_pick = Some(file.clone());
            a.run_file_cmd(cmd, &ctx);
            assert_eq!(a.ui.ask_save, Some(cmd), "{cmd:?} asks first");
            assert_eq!(
                a.project.tracks.len(),
                tracks,
                "not replaced until answered"
            );
        }
        a.ui.ask_save = None;
        a.run_file_cmd(FileCmd::Revert, &ctx);
        assert!(a.ui.ask_revert, "Revert has something to throw away");

        // Save reads the settings: they are in the file, and nothing is unsaved.
        assert!(a.save_to(&file));
        assert!(!a.has_unsaved_changes());
        let (opened, _) = project_io::open_project(&file, &ctx).unwrap();
        let slot = opened
            .tracks
            .last()
            .unwrap()
            .au_instrument
            .as_ref()
            .unwrap();
        assert_eq!(slot.state.as_deref(), Some(&b"turned"[..]));
    }

    #[test]
    fn a_save_with_a_missing_audio_file_succeeds_and_lists_the_file_in_a_window() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let wav = dir.path().join("from-usb.wav");
        project_io::tests::write_wav(&wav);
        let mut a = app();
        crate::import::import_audio(&mut a.project, &wav, &ctx).unwrap();
        let regions = |a: &App| a.project.tracks.last().unwrap().regions.len();
        std::fs::remove_file(&wav).unwrap();

        // Save, Save As and Save A Copy all go through, and each names the file.
        for copy in [false, true, false] {
            a.ui.missing_audio.clear();
            let to = dir.path().join(if copy { "Copy" } else { "Song" });
            let done = if copy {
                a.save_copy_to(&to)
            } else {
                a.save_to(&to)
            };
            assert!(done, "the save is not stopped");
            assert_eq!(a.ui.missing_audio, vec![wav.display().to_string()]);
            assert_eq!(regions(&a), 1);
        }
        assert_eq!(content(&a.project), a.saved, "saved: nothing left unsaved");

        // The window stays until it is closed (nobody clicks here).
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            a.show_missing_audio(ui.ctx())
        });
        out.textures_delta.clear();
        assert_eq!(a.ui.missing_audio.len(), 1);

        // Opened again: the region is there, the window names the file again, and the
        // project is not "changed" by it; saving keeps the region.
        a.ui.missing_audio.clear();
        let file = dir.path().join("Song.prowave");
        a.open(&file, &ctx);
        assert_eq!(a.ui.missing_audio, vec![wav.display().to_string()]);
        assert_eq!(regions(&a), 1);
        assert!(a.project.tracks.last().unwrap().regions[0]
            .missing
            .is_some());
        assert!(!a.has_unsaved_changes());
        assert!(a.save_to(&file));
        a.open(&file, &ctx);
        assert_eq!(regions(&a), 1);

        // The file is back: no window, and the region plays.
        project_io::tests::write_wav(&wav);
        a.open(&file, &ctx);
        assert!(a.ui.missing_audio.is_empty());
        assert!(a.project.tracks.last().unwrap().regions[0].audio.is_some());
    }

    #[test]
    fn playhead_and_selection_are_not_unsaved_changes() {
        let mut p = Project::demo();
        let before = content(&p);
        p.position_beats = 12.0;
        p.selected_track = 2;
        assert_eq!(content(&p), before);
        p.tempo += 1.0;
        assert_ne!(content(&p), before);
    }

    #[test]
    fn menu_entries_and_key_commands() {
        assert_eq!(FileCmd::from_menu("Save As…"), Some(FileCmd::SaveAs));
        assert_eq!(FileCmd::from_menu("Bounce"), None);
        assert_eq!(FileCmd::from_menu("Close Project"), Some(FileCmd::Close));
        let save_as = FileCmd::SaveAs.shortcut().unwrap();
        assert!(save_as.modifiers.shift && save_as.logical_key == Key::S);
        assert!(!FileCmd::Save.shortcut().unwrap().modifiers.shift);
        assert_eq!(
            FileCmd::from_menu("Save A Copy As…"),
            Some(FileCmd::SaveCopy)
        );
        assert_eq!(FileCmd::from_menu("Revert to Saved"), Some(FileCmd::Revert));
        assert!(FileCmd::SaveCopy.shortcut().is_none() && FileCmd::OpenRecent.shortcut().is_none());
    }

    #[test]
    fn save_then_open_tracks_path_name_and_clean_state() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut a = app();
        a.project.tempo = 97.0;
        assert_ne!(content(&a.project), a.saved);

        assert!(a.save_to(&dir.path().join("Night Drive")));
        let file = dir.path().join("Night Drive.prowave");
        assert_eq!(a.path.as_deref(), Some(file.as_path()));
        assert_eq!(a.project.name, "Night Drive");
        assert_eq!(content(&a.project), a.saved);

        let rev = a.project.song_rev;
        a.project.tempo = 120.0;
        a.open(&file, &ctx);
        assert_eq!(a.project.tempo, 97.0);
        assert_ne!(a.project.song_rev, rev, "engine must rebuild the song");
        assert_eq!(content(&a.project), a.saved);
    }

    #[test]
    fn new_with_unsaved_changes_asks_first() {
        let ctx = egui::Context::default();

        let mut clean = app();
        let rev = clean.project.song_rev;
        clean.run_file_cmd(FileCmd::New, &ctx);
        assert!(clean.ui.ask_save.is_none(), "nothing to lose, no prompt");
        assert_ne!(clean.project.song_rev, rev, "New replaced the project");

        let mut dirty = app();
        dirty.project.tempo += 1.0;
        let tracks = dirty.project.tracks.len();
        dirty.run_file_cmd(FileCmd::New, &ctx);
        assert_eq!(dirty.ui.ask_save, Some(FileCmd::New));
        assert_eq!(
            dirty.project.tracks.len(),
            tracks,
            "not replaced until answered"
        );

        // Drawing the prompt (nobody clicks) keeps it open.
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            dirty.show_save_prompt(ui.ctx())
        });
        out.textures_delta.clear();
        assert_eq!(dirty.ui.ask_save, Some(FileCmd::New));
    }

    #[test]
    fn save_commands_never_prompt() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut a = app();
        a.project.tempo += 1.0;
        a.save_to(&dir.path().join("x"));
        a.project.tempo += 1.0;
        a.run_file_cmd(FileCmd::Save, &ctx);
        assert!(a.ui.ask_save.is_none());
        assert_eq!(content(&a.project), a.saved);
    }

    #[test]
    fn open_failure_keeps_the_project_and_reports() {
        let mut a = app();
        let tracks = a.project.tracks.len();
        a.open(
            Path::new("does-not-exist.prowave"),
            &egui::Context::default(),
        );
        assert_eq!(a.project.tracks.len(), tracks);
        assert!(a.ui.status.is_some());
    }

    #[test]
    fn saving_and_opening_feed_open_recent_and_a_broken_file_is_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut a = app();
        assert!(a.save_to(&dir.path().join("One")));
        assert!(a.save_to(&dir.path().join("Two")));
        let (one, two) = (
            dir.path().join("One.prowave"),
            dir.path().join("Two.prowave"),
        );
        assert_eq!(a.ui.recent.paths(), [two.clone(), one.clone()]);

        a.open(&one, &ctx);
        assert_eq!(a.ui.recent.paths()[0], one);

        let bad = dir.path().join("Bad.prowave");
        std::fs::write(&bad, b"not a zip").unwrap();
        a.ui.recent.add(&bad);
        a.open(&bad, &ctx);
        assert!(!a.ui.recent.paths().contains(&bad));
        assert!(a.ui.status.as_deref().unwrap().starts_with("cannot open"));
    }

    #[test]
    fn open_recent_asks_before_discarding_changes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut a = app();
        assert!(a.save_to(&dir.path().join("Saved")));
        let file = dir.path().join("Saved.prowave");

        a.project.tempo = 61.0;
        a.ui.recent_pick = Some(file.clone());
        a.run_file_cmd(FileCmd::OpenRecent, &ctx);
        assert_eq!(a.ui.ask_save, Some(FileCmd::OpenRecent));
        assert_eq!(a.project.tempo, 61.0, "not replaced until answered");

        // "Don't Save": the pending project opens and the pick is used up.
        a.ui.ask_save = None;
        a.do_file_cmd(FileCmd::OpenRecent, &ctx);
        assert_ne!(a.project.tempo, 61.0);
        assert!(a.ui.recent_pick.is_none());
    }

    #[test]
    fn revert_needs_a_saved_file_and_a_confirmed_change() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut a = app();
        a.run_file_cmd(FileCmd::Revert, &ctx);
        assert!(!a.ui.ask_revert);
        assert!(a.ui.status.as_deref().unwrap().contains("not been saved"));

        assert!(a.save_to(&dir.path().join("Song")));
        a.run_file_cmd(FileCmd::Revert, &ctx);
        assert!(!a.ui.ask_revert);
        assert!(a
            .ui
            .status
            .as_deref()
            .unwrap()
            .contains("no unsaved changes"));

        let saved_tempo = a.project.tempo;
        a.project.tempo += 10.0;
        a.run_file_cmd(FileCmd::Revert, &ctx);
        assert!(a.ui.ask_revert, "asks before throwing changes away");
        assert_eq!(a.project.tempo, saved_tempo + 10.0);

        a.revert(&ctx);
        assert_eq!(a.project.tempo, saved_tempo);
        assert_eq!(content(&a.project), a.saved);
    }

    #[test]
    fn save_a_copy_leaves_the_current_project_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = app();
        assert!(a.save_to(&dir.path().join("Original")));
        let (path, name) = (a.path.clone(), a.project.name.clone());
        let recent = a.ui.recent.paths().to_vec();

        a.project.tempo += 5.0;
        assert!(a.save_copy_to(&dir.path().join("Backup")));
        let copy = dir.path().join("Backup.prowave");
        assert!(copy.is_file());
        assert_eq!((a.path.clone(), a.project.name.clone()), (path, name));
        assert_ne!(content(&a.project), a.saved, "still has unsaved changes");
        assert_eq!(a.ui.recent.paths(), recent.as_slice());

        // The copy holds the current state, tempo included.
        let (opened, _) = project_io::open_project(&copy, &egui::Context::default()).unwrap();
        assert_eq!(opened.tempo, a.project.tempo);
        assert_eq!(opened.name, "Backup");
    }

    #[test]
    fn an_edited_time_signature_is_saved_and_restored() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut a = app();
        assert!(a
            .project
            .step_signature(crate::model::SigPart::Numerator, 2));
        assert!(a
            .project
            .step_signature(crate::model::SigPart::Denominator, 1));
        assert_eq!(a.project.sig, (6, 8));
        assert_ne!(
            content(&a.project),
            a.saved,
            "a new signature is an unsaved change"
        );

        assert!(a.save_to(&dir.path().join("Waltz")));
        a.project.sig = (4, 4);
        a.open(&dir.path().join("Waltz.prowave"), &ctx);
        assert_eq!(a.project.sig, (6, 8));
        assert_eq!(a.project.bar_len(0), 3.0);
    }
}
