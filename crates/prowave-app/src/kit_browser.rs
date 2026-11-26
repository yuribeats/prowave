//! The Kit Browser (the Kits key of the Channel Rack): every drum kit of the library folders,
//! searched as it is typed, by source, the ranked ones, the favourites; the samples of the kit
//! picked, with what each one is; and the rack's first sixteen channels as pads.
//!
//! Load Kit puts the kit on the rack's channels ([`crate::kit_files::Kit::pads`]): the steps
//! stay, so a pattern plays the new kit (a kit of SoundFonts alone goes on a new track as its
//! SoundFont instead). A sample goes on one channel by a double-click or Return (the pad that
//! is lit), or by dragging it onto a pad here, onto a channel of the rack or onto its Add
//! Channel key. The triangle of a sample plays that sample on the engine's audition voice; with
//! Audition lit, picking a sample plays it and picking a kit plays its kick, snare and hat.
//!
//! The keys, while the window has them (it was clicked last): Up and Down move through the
//! list the arrows are in (the kits, or the kit's samples), Right goes to the samples and Left
//! back to the kits, Return loads the kit or puts the sample on the lit pad. Up, Down and
//! Return also work while the search field has the keys.
//!
//! The index is made on background threads ([`crate::kit_library`]): the list of kits is there
//! at once from the cache, the folders are read in milliseconds, and the kits' files are read
//! after that while the list is in use. A kit picked is made from its kept listing, so neither
//! the window nor the audio waits for a disk. A library folder that is not there (a drive that
//! is not mounted) shows an unlit lamp in Folders, marked missing, and nothing else.
//!
//! With a kit list set (`kit_list` in the settings, `kits.list`) and on, the kits are the
//! list's, in its order, and the left column holds its types instead of the sources;
//! right-click that column (or the Kit List key in Folders) to turn the list off or on.
use crate::channel_rack::{self, SampleDrag};
use crate::gear::{self, LedAt};
use crate::hw::{self, Cap};
use crate::i18n::tr;
use crate::kit_classify::{DrumClass, KIT_PADS};
use crate::kit_files::Kit;
use crate::kit_library::{
    self, Filter, Index, KitEntry, KitList, Opened, ScanJob, ScanMsg, Settings,
};
use crate::midi_monitor::hw_utilities as hu;
use crate::model::Project;
use crate::theme::{self, Face};
use egui::{pos2, vec2, Align2, Color32, Id, Pos2, Rect, Sense, Stroke, Ui};
use prowave_engine::{AuditionMsg, AuditionRemote, SampleBuf, AUDITION_LAYERS};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

pub const WIDTH: f32 = 920.0;
const PAD: f32 = 18.0;
const ROW: f32 = 24.0;
const LIST_H: f32 = 336.0;
const SOURCES_W: f32 = 196.0;
const KITS_W: f32 = 330.0;
const GAP: f32 = 10.0;
/// Silence after a sound on the audition voice (which loops): time to stop it in.
const TAIL_SECONDS: f64 = 0.6;
/// The longest a sample is played for.
const MAX_AUDITION_SECONDS: f64 = 8.0;
/// A kit's audition: its hits one every `KIT_STEP_SECONDS`, each at most `KIT_HIT_SECONDS`.
pub const KIT_STEP_SECONDS: f64 = 0.3;
const KIT_HIT_SECONDS: f64 = 0.9;

/// What is on the audition voice.
struct Playing {
    key: String,
    /// The phase of the loop where the sound ends and the silence starts.
    end: f32,
    last: f32,
    heard: bool,
}

/// A sound read for the audition: the buffer (sound and silence) and where the sound ends.
type Ready = Result<(SampleBuf, f32), String>;

/// The list the arrow keys move in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Column {
    #[default]
    Kits,
    Samples,
}

#[derive(Default)]
pub struct KitBrowser {
    pub open: bool,
    started: bool,
    /// Where the settings and the cache are kept (`None`: nowhere, as in tests).
    data: Option<PathBuf>,
    pub settings: Settings,
    pub index: Index,
    /// The folders are being read.
    pub scanning: bool,
    /// While the kits' files are read: kits done and kits to do.
    pub listing: Option<(usize, usize)>,
    /// How long the last reading of the folders took.
    pub last_scan: Option<Duration>,
    /// The last reading of the kits' files: how many were read and how long it took.
    pub last_listing: Option<(usize, Duration)>,
    scan: Option<ScanJob>,
    pub filter: Filter,
    /// Bumped when the index or the favourites change.
    rev: u64,
    results: Vec<u32>,
    results_for: Option<(Filter, u64)>,
    /// The kit picked (its folder).
    pub selected: Option<String>,
    kit: Option<Arc<Kit>>,
    kit_rx: Option<(String, Receiver<Opened>)>,
    /// A kit load waiting for its samples to be copied here: the copies, then (kit name, the
    /// channels' classes, the rack's track when there was one).
    copying: Option<CopyJob>,
    kit_stop: Option<Arc<AtomicBool>>,
    class: Option<DrumClass>,
    /// The sample picked in the kit's list (an index into its samples).
    pub sample: Option<usize>,
    /// The list the arrows move in.
    pub column: Column,
    /// The window has the keys: the last click was in it.
    pub keys: bool,
    /// Where the window was the last frame.
    rect: Option<Rect>,
    /// The rack channel a double-clicked sample goes on.
    pub pad: usize,
    pub show_folders: bool,
    scroll_to: Option<usize>,
    scroll_sample: Option<usize>,
    want: Option<(String, Receiver<Ready>)>,
    send: Option<(String, Arc<SampleBuf>, f32)>,
    playing: Option<Playing>,
    stop: bool,
    /// The kit whose audition waits for its files (Audition lit).
    kit_audition_due: bool,
    pub error: Option<String>,
    /// The kit list read from `settings.kit_list`, or why it could not be.
    pub list: Option<Result<Arc<KitList>, String>>,
    list_rx: Option<Receiver<Result<KitList, String>>>,
}

impl KitBrowser {
    /// A browser that keeps its settings and cache in `data` and looks in `folders`.
    #[cfg(test)]
    pub fn with(data: Option<PathBuf>, folders: Vec<String>) -> Self {
        let mut v = Self {
            data,
            started: true,
            ..Self::default()
        };
        v.settings.folders = folders;
        v
    }

    /// Reads the settings and starts the first scan, once.
    pub fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        if cfg!(test) {
            // A test looks only where it says: never in the library of the machine it runs on.
            self.settings.folders.clear();
        } else {
            self.data = kit_library::data_dir();
        }
        if let Some(dir) = &self.data {
            self.settings = Settings::load(dir);
        }
        self.rescan_with(false);
    }

    /// Scans the folders again in the background and reads every kit's files again.
    pub fn rescan(&mut self) {
        self.rescan_with(true);
    }

    /// Scans the folders in the background; the kits' files are read again for every kit
    /// when `full`, else for the kits never read and those whose folder has changed.
    pub fn rescan_with(&mut self, full: bool) {
        self.read_list();
        self.scanning = true;
        self.listing = None;
        self.scan = Some(kit_library::spawn_scan(
            self.settings.folders.clone(),
            self.data.clone(),
            full,
        ));
    }

    /// Reads the folders now, on this thread (the control interface, which waits for it);
    /// the kits' files are then read in the background (every kit's when `full`).
    pub fn scan_now(&mut self, full: bool) -> Duration {
        self.start();
        self.scan = None;
        let cache = self
            .data
            .as_deref()
            .map(kit_library::Cache::load)
            .unwrap_or_default();
        let started = std::time::Instant::now();
        let (index, cache) = kit_library::scan(&self.settings.folders, cache);
        let took = started.elapsed();
        if let Some(dir) = &self.data {
            if let Err(e) = cache.save(dir) {
                tracing::warn!("could not write the kit index: {e}");
            }
        }
        self.take_index(index);
        self.last_scan = Some(took);
        // The background scan reads the folders again (milliseconds) and then the kits.
        self.rescan_with(full);
        took
    }

    fn take_index(&mut self, index: Index) {
        self.index = index;
        let list = match &self.list {
            Some(Ok(l)) => Some(l.clone()),
            _ => None,
        };
        self.index.apply_list(list.as_deref());
        self.rev += 1;
        if self
            .filter
            .source
            .as_ref()
            .is_some_and(|s| !self.index.sources.iter().any(|(n, _)| n == s))
        {
            self.filter.source = None;
        }
    }

    /// Takes what the background threads have done.
    pub fn poll(&mut self) {
        if let Some(job) = self.scan.take() {
            let mut done = false;
            loop {
                match job.rx.try_recv() {
                    Ok(ScanMsg::Cached(index)) => {
                        if self.index.kits.is_empty() {
                            self.take_index(index);
                        }
                    }
                    Ok(ScanMsg::Scanned(index, took)) => {
                        self.take_index(index);
                        self.last_scan = Some(took);
                        self.scanning = false;
                        // Nothing anywhere: the folders are what there is to look at.
                        if self.index.kits.is_empty() {
                            self.show_folders = true;
                        }
                    }
                    Ok(ScanMsg::Listing { done, total, index }) => {
                        self.take_index(index);
                        self.listing = Some((done, total));
                    }
                    Ok(ScanMsg::Listed(index, read, took)) => {
                        self.take_index(index);
                        self.listing = None;
                        self.last_listing = Some((read, took));
                        if self.index.kits.is_empty() {
                            self.show_folders = true;
                        }
                        done = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
            if done {
                self.scanning = false;
                self.listing = None;
            } else {
                self.scan = Some(job);
            }
        }
        if let Some(rx) = self.list_rx.take() {
            match rx.try_recv() {
                Ok(list) => self.take_list(list),
                Err(TryRecvError::Empty) => self.list_rx = Some(rx),
                Err(TryRecvError::Disconnected) => {}
            }
        }
        if let Some((path, rx)) = self.kit_rx.take() {
            match rx.try_recv() {
                Ok(opened) => self.take_kit(&path, opened),
                Err(TryRecvError::Empty) => self.kit_rx = Some((path, rx)),
                Err(TryRecvError::Disconnected) => {}
            }
        }
    }

    /// Whether a scan is still at work (reading the folders or the kits' files).
    pub fn indexing(&self) -> bool {
        self.scan.is_some()
    }

    fn take_kit(&mut self, path: &str, opened: Opened) {
        let Opened { kit, fresh } = opened;
        // What was read from the disk is kept, and the index learns the kit's counts.
        if let Some(k) = self.index.kits.iter_mut().find(|k| k.path == path) {
            k.files = Some(kit.samples.len() as u32);
            k.pads = Some(kit.pads.iter().flatten().count().min(255) as u8);
            if !fresh.is_empty() {
                if let Some(dir) = self.data.clone() {
                    let root = k.root.clone();
                    let _ = std::thread::Builder::new()
                        .name("kit-keep".into())
                        .spawn(move || kit_library::keep(&dir, &root, &fresh));
                }
            }
        }
        if self.selected.as_deref() == Some(path) {
            self.kit = Some(Arc::new(kit));
            self.class = None;
            self.sample = None;
            if std::mem::take(&mut self.kit_audition_due) {
                self.audition_kit();
            }
        }
    }

    /// The index's line for the kit in folder `path` (one not in the index: a plain folder).
    fn entry(&self, path: &str) -> KitEntry {
        self.index.find(path).cloned().unwrap_or_else(|| {
            let name = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            KitEntry::new("", path.to_string(), "", &name)
        })
    }

    /// Picks the kit in folder `path`: it is made from its kept listing (or read from its
    /// folder when it has none yet) on another thread. With Audition lit it is heard.
    pub fn select(&mut self, path: &str) {
        if self.selected.as_deref() == Some(path) {
            return;
        }
        self.selected = Some(path.to_string());
        self.kit = None;
        self.sample = None;
        self.kit_audition_due = self.settings.audition;
        // The read of the kit picked before is given up.
        if let Some(stop) = self.kit_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let entry = self.entry(path);
        let data = self.data.clone();
        let flag = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("kit-read".into())
            .spawn(move || {
                let opened = kit_library::open(data.as_deref(), &entry, &flag);
                if !flag.load(Ordering::Relaxed) {
                    let _ = tx.send(opened);
                }
            });
        if spawned.is_ok() {
            self.kit_rx = Some((path.to_string(), rx));
            self.kit_stop = Some(stop);
        }
    }

    /// Picks the kit and makes it now, on this thread (the control interface).
    pub fn select_now(&mut self, path: &str) -> Arc<Kit> {
        if let Some(stop) = self.kit_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.selected = Some(path.to_string());
        self.kit_rx = None;
        let entry = self.entry(path);
        let opened = kit_library::open(self.data.as_deref(), &entry, &AtomicBool::new(false));
        let kit = opened.kit.clone();
        self.take_kit(path, opened);
        self.kit.clone().unwrap_or_else(|| Arc::new(kit))
    }

    /// The kit picked, once it is made.
    pub fn kit(&self) -> Option<&Arc<Kit>> {
        self.kit.as_ref()
    }

    /// Picks sample `i` of the kit; with Audition lit it is heard.
    pub fn pick_sample(&mut self, i: usize) {
        let Some(kit) = self.kit.clone() else { return };
        if i >= kit.samples.len() {
            return;
        }
        self.sample = Some(i);
        self.column = Column::Samples;
        if self.settings.audition {
            self.audition_sample(i, false);
        }
    }

    /// Plays sample `i` of the kit picked, from its start; `toggle`: when it is the one
    /// playing, it stops instead (its triangle).
    pub fn audition_sample(&mut self, i: usize, toggle: bool) {
        let Some(kit) = self.kit.clone() else { return };
        if kit
            .samples
            .get(i)
            .is_none_or(|s| s.class == DrumClass::SoundFont)
        {
            return;
        }
        let path = kit.path_of(i);
        self.audition(
            format!("sample:{}", path.display()),
            vec![path],
            false,
            toggle,
        );
    }

    /// Plays any audio file on the audition voice, from its start.
    pub fn play_file(&mut self, path: &Path) {
        let key = format!("sample:{}", path.display());
        self.audition(key, vec![path.to_path_buf()], false, false);
    }

    /// Plays the kick, snare and hat of the kit picked.
    pub fn audition_kit(&mut self) {
        let Some(kit) = self.kit.clone() else { return };
        let Some(sel) = self.selected.clone() else {
            return;
        };
        let paths: Vec<PathBuf> = kit
            .audition_samples()
            .into_iter()
            .map(|i| kit.path_of(i))
            .collect();
        if paths.is_empty() {
            return;
        }
        self.audition(format!("kit:{sel}"), paths, true, false);
    }

    /// Lights or puts out Audition (kept with the settings).
    pub fn set_audition(&mut self, on: bool) {
        if self.settings.audition == on {
            return;
        }
        self.settings.audition = on;
        self.save_settings();
        if !on {
            self.stop_audition();
        }
    }

    /// The kits the filter keeps, as indices into the index.
    pub fn results(&mut self) -> &[u32] {
        self.filter.list = self.list_active();
        let key = (self.filter.clone(), self.rev);
        if self.results_for.as_ref() != Some(&key) {
            self.results = kit_library::search_keeping(
                &self.index,
                &self.filter,
                &self.settings.favorites,
                &self.settings.default_kit,
            );
            self.results_for = Some(key);
        }
        &self.results
    }

    /// How many kits the kit list shows with no search, type or other filter.
    pub fn results_all(&self) -> usize {
        let f = Filter {
            list: self.list_active(),
            ..Filter::default()
        };
        kit_library::search_keeping(
            &self.index,
            &f,
            &self.settings.favorites,
            &self.settings.default_kit,
        )
        .len()
    }

    /// The kit list read, when there is one.
    pub fn list_now(&self) -> Option<&KitList> {
        match &self.list {
            Some(Ok(l)) => Some(l),
            _ => None,
        }
    }

    /// The kit list is set, read, and on: the browser shows its kits only.
    pub fn list_active(&self) -> bool {
        self.settings.kit_list_on
            && !self.settings.kit_list.is_empty()
            && self.list_now().is_some_and(|l| !l.rows.is_empty())
    }

    /// Reads the kit list the settings name, on another thread (it may be on a share).
    fn read_list(&mut self) {
        let setting = self.settings.kit_list.clone();
        if setting.is_empty() {
            self.list_rx = None;
            if self.list.take().is_some() {
                self.reapply_list();
            }
            return;
        }
        let folders = self.settings.folders.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("kit-list".into())
            .spawn(move || {
                let _ = tx.send(kit_library::load_list(&setting, &folders));
            });
        if spawned.is_ok() {
            self.list_rx = Some(rx);
        }
    }

    /// Reads the kit list the settings name now, on this thread (the control interface).
    pub fn read_list_now(&mut self) {
        self.list_rx = None;
        if self.settings.kit_list.is_empty() {
            self.list = None;
            self.reapply_list();
            return;
        }
        let list = kit_library::load_list(&self.settings.kit_list, &self.settings.folders);
        self.take_list(list);
    }

    fn take_list(&mut self, list: Result<KitList, String>) {
        if let Err(e) = &list {
            tracing::warn!("kit list: {e}");
        }
        self.list = Some(list.map(Arc::new));
        self.reapply_list();
    }

    fn reapply_list(&mut self) {
        let list = match &self.list {
            Some(Ok(l)) => Some(l.clone()),
            _ => None,
        };
        self.index.apply_list(list.as_deref());
        if self
            .filter
            .kind
            .as_ref()
            .is_some_and(|t| !self.index.types.iter().any(|(n, _)| n == t))
        {
            self.filter.kind = None;
        }
        self.rev += 1;
    }

    /// Sets the kit list (a TSV's path; empty: none), turns it on, saves and reads it.
    pub fn set_kit_list(&mut self, path: &str) {
        self.start();
        self.settings.kit_list = path.trim().to_string();
        self.settings.kit_list_on = true;
        self.save_settings();
        self.read_list();
    }

    /// Turns the kit list on or off (kept with the settings).
    pub fn set_list_on(&mut self, on: bool) {
        self.start();
        if self.settings.kit_list_on != on {
            self.settings.kit_list_on = on;
            self.rev += 1;
            self.save_settings();
        }
    }

    fn save_settings(&self) {
        if let Some(dir) = &self.data {
            if let Err(e) = self.settings.save(dir) {
                tracing::warn!("could not save the kit library settings: {e}");
            }
        }
    }

    /// Sets the library folders, saves them and scans.
    pub fn set_folders(&mut self, folders: Vec<String>) {
        self.start();
        let mut seen = std::collections::HashSet::new();
        self.settings.folders = folders
            .into_iter()
            .filter(|f| !f.trim().is_empty() && seen.insert(f.clone()))
            .collect();
        self.save_settings();
        self.rescan_with(false);
    }

    /// Adds a library folder (what Add Folder does with the folder picked) and scans.
    pub fn add_folder(&mut self, dir: &Path) {
        let mut list = self.settings.folders.clone();
        list.push(dir.to_string_lossy().into_owned());
        self.set_folders(list);
    }

    pub fn set_favorite(&mut self, path: &str, on: bool) {
        let changed = if on {
            self.settings.favorites.insert(path.to_string())
        } else {
            self.settings.favorites.remove(path)
        };
        if changed {
            self.rev += 1;
            self.save_settings();
        }
    }

    /// Sets the kit new Channel Racks start with (empty: none) and saves it.
    pub fn set_default_kit(&mut self, path: &str) {
        self.start();
        if self.settings.default_kit != path {
            self.settings.default_kit = path.to_string();
            self.rev += 1;
            self.save_settings();
        }
    }

    /// Sets when loaded kits are copied to this computer, and saves it.
    pub fn set_copy_mode(&mut self, mode: crate::kit_copy::CopyMode) {
        self.start();
        if self.settings.copy_kits != mode {
            self.settings.copy_kits = mode;
            self.save_settings();
        }
    }

    /// Where loaded kits are copied (`None`: nowhere, as in tests).
    pub fn local_dir(&self) -> Option<PathBuf> {
        self.data.as_ref().map(|d| d.join("local"))
    }

    /// The kit of the index whose folder holds `file` (the innermost), as (folder, source,
    /// name).
    fn kit_holding(&self, file: &Path) -> Option<(PathBuf, String, String)> {
        self.index
            .kits
            .iter()
            .filter(|k| !k.path.is_empty() && file.starts_with(&k.path))
            .max_by_key(|k| k.path.len())
            .map(|k| (PathBuf::from(&k.path), k.source.clone(), k.name.clone()))
    }

    /// Where the rack plays `file` from: a copy on this computer when the setting says so
    /// (see [`crate::kit_copy`]), else `file`.
    pub fn localize(&self, file: &Path) -> PathBuf {
        let kit = self.kit_holding(file);
        crate::kit_copy::localize(
            self.local_dir().as_deref(),
            self.settings.copy_kits,
            kit.as_ref()
                .map(|(p, s, n)| (p.as_path(), s.as_str(), n.as_str())),
            file,
        )
    }

    /// [`Self::localize`] for the channels of a kit load from kit folder `kit`.
    pub fn localize_pads(&self, kit: &str, pads: &[Option<PathBuf>]) -> Vec<Option<PathBuf>> {
        let e = self.entry(kit);
        let root = PathBuf::from(&e.path);
        let source = if e.source.is_empty() {
            Path::new(kit)
                .parent()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            e.source.clone()
        };
        crate::kit_copy::localize_pads(
            self.local_dir().as_deref(),
            self.settings.copy_kits,
            Some((root.as_path(), source.as_str(), e.name.as_str())),
            pads,
        )
    }

    pub fn is_favorite(&self, path: &str) -> bool {
        self.settings.favorites.contains(path)
    }

    /// The name of the kit picked, from the index.
    fn selected_name(&self) -> String {
        let Some(path) = &self.selected else {
            return String::new();
        };
        self.entry(path).name
    }

    /// Plays `paths` (a kit's hits one after the other when `kit`) as `key`. `toggle`: the
    /// same key again stops it.
    fn audition(&mut self, key: String, paths: Vec<PathBuf>, kit: bool, toggle: bool) {
        if toggle
            && (self.playing.as_ref().is_some_and(|p| p.key == key)
                || self.want.as_ref().is_some_and(|w| w.0 == key))
        {
            self.stop_audition();
            return;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("kit-audition".into())
            .spawn(move || {
                let _ = tx.send(if kit {
                    kit_preview(&paths)
                } else {
                    paths
                        .first()
                        .ok_or_else(|| "nothing to play".to_string())
                        .and_then(|p| sample_preview(p))
                });
            });
        if spawned.is_ok() {
            // What was asked before and is not playing yet is given up; what plays is
            // replaced when the new sound is ready.
            self.send = None;
            self.stop = false;
            self.want = Some((key, rx));
        }
    }

    pub fn stop_audition(&mut self) {
        self.want = None;
        self.send = None;
        if self.playing.is_some() {
            self.stop = true;
        }
    }

    /// What is playing, or about to: `sample:<file>` or `kit:<folder>`.
    pub fn audition_key(&self) -> Option<&str> {
        self.playing
            .as_ref()
            .map(|p| p.key.as_str())
            .or(self.send.as_ref().map(|s| s.0.as_str()))
            .or(self.want.as_ref().map(|w| w.0.as_str()))
    }

    fn busy(&self) -> bool {
        self.scanning
            || self.scan.is_some()
            || self.kit_rx.is_some()
            || self.copying.is_some()
            || self.want.is_some()
            || self.send.is_some()
            || self.playing.is_some()
            || self.stop
    }
}

/// A kit load whose samples are being copied to this computer on another thread.
pub struct CopyJob {
    rx: Receiver<Vec<Option<PathBuf>>>,
    name: String,
    classes: Vec<crate::kit_classify::DrumClass>,
    track: Option<usize>,
}

/// The channels a kit load fills, as files.
pub fn pad_paths(kit: &Kit) -> Vec<Option<PathBuf>> {
    kit.pads.iter().map(|p| p.map(|i| kit.path_of(i))).collect()
}

/// The class of each channel a kit load fills: its sample's, or for an empty channel the
/// class the layout gives it.
pub fn pad_classes(kit: &Kit) -> Vec<crate::kit_classify::DrumClass> {
    kit.pads
        .iter()
        .enumerate()
        .map(|(k, p)| match p {
            Some(i) => kit.samples[*i].class,
            None => crate::kit_classify::PAD_LAYOUT
                .get(k)
                .copied()
                .unwrap_or(crate::kit_classify::DrumClass::Other),
        })
        .collect()
}

/// Loads the kit picked into the rack of the selected track (a new rack track when the
/// selected track has none); a kit of SoundFonts alone goes on a new instrument track as its
/// SoundFont (the drum preset when it has one). Returns the track.
pub fn load_selected(v: &mut KitBrowser, p: &mut Project) -> Option<usize> {
    let kit = v.kit.clone()?;
    let name = v.selected_name();
    if kit.pads.is_empty() {
        let sf = kit.soundfonts().first().map(|i| kit.path_of(*i))?;
        return Some(load_soundfont(p, &name, &v.localize(&sf)));
    }
    let folder = v
        .selected
        .clone()
        .unwrap_or_else(|| kit.root.to_string_lossy().into_owned());
    let pads = pad_paths(&kit);
    let classes = pad_classes(&kit);
    let track = channel_rack::rack_track(p);
    // Files to be copied from a network share are copied on another thread (a share can take
    // seconds a file); the kit goes on the rack when they are here (see `finish_copy`).
    let needs_copy = v.local_dir().is_some()
        && pads.iter().flatten().any(|f| {
            v.settings.copy_kits == crate::kit_copy::CopyMode::Always
                || crate::kit_copy::is_remote(f)
        });
    if needs_copy {
        let local = v.local_dir();
        let mode = v.settings.copy_kits;
        let e = v.entry(&folder);
        let source = if e.source.is_empty() {
            Path::new(&folder)
                .parent()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            e.source.clone()
        };
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("kit-copy".into())
            .spawn(move || {
                let root = PathBuf::from(&e.path);
                let got = crate::kit_copy::localize_pads(
                    local.as_deref(),
                    mode,
                    Some((root.as_path(), source.as_str(), e.name.as_str())),
                    &pads,
                );
                let _ = tx.send(got);
            });
        match spawned {
            Ok(_) => {
                v.copying = Some(CopyJob {
                    rx,
                    name,
                    classes,
                    track,
                });
                return None;
            }
            Err(e) => tracing::warn!("could not start copying the kit: {e}"),
        }
        return None;
    }
    let pads = v.localize_pads(&folder, &pads);
    let ti = channel_rack::load_kit(p, track, &name, &pads, &classes);
    channel_rack::set_kit_dir(p, ti, Path::new(&folder));
    Some(ti)
}

/// Puts a kit on the rack once its samples have been copied here; true while a copy is still
/// running (the caller asks for another frame). Returns the rack's track when it loaded.
pub fn finish_copy(v: &mut KitBrowser, p: &mut Project) -> (bool, Option<usize>) {
    let Some(job) = &v.copying else {
        return (false, None);
    };
    match job.rx.try_recv() {
        Ok(pads) => {
            let job = v.copying.take();
            let ti = job.map(|j| {
                let track = j
                    .track
                    .filter(|t| channel_rack::rack_state(p, *t).is_some());
                channel_rack::load_kit(p, track, &j.name, &pads, &j.classes)
            });
            (false, ti)
        }
        Err(mpsc::TryRecvError::Empty) => (true, None),
        Err(mpsc::TryRecvError::Disconnected) => {
            v.copying = None;
            (false, None)
        }
    }
}

/// Whether a kit's samples are being copied here.
pub fn copying(v: &KitBrowser) -> bool {
    v.copying.is_some()
}

/// A new instrument track named `name` playing the SoundFont `path`: its first drum preset
/// (bank 128), else its first preset. Returns the track.
pub fn load_soundfont(p: &mut Project, name: &str, path: &Path) -> usize {
    let path = path.to_string_lossy().into_owned();
    let (bank, program) = match crate::soundfonts::load(&path) {
        Ok(sf) => {
            let presets = sf.presets();
            presets
                .iter()
                .find(|pr| pr.bank == 128)
                .or(presets.first())
                .map_or((0, 0), |pr| (pr.bank, pr.program))
        }
        Err(e) => {
            tracing::warn!("{e}");
            (0, 0)
        }
    };
    let ti = p.add_track(crate::model::TrackKind::Instrument);
    if !name.is_empty() {
        p.tracks[ti].name = name.to_string();
    }
    p.tracks[ti].soundfont = Some(crate::model::SoundFontUse {
        path,
        bank,
        program,
    });
    p.selected_track = ti;
    p.song_rev += 1;
    ti
}

/// Puts `path` on channel `channel` of the selected track's rack (a new rack track when the
/// selected track has none), from a copy on this computer when the setting says so.
pub fn assign(v: &KitBrowser, p: &mut Project, channel: usize, path: &Path) -> bool {
    let ti = match channel_rack::rack_track(p) {
        Some(i) => i,
        None => channel_rack::load_kit(p, None, "", &[], &[]),
    };
    channel_rack::assign_sample(p, ti, channel, &v.localize(path))
}

// ------------------------------------------------------------------------------- audition

/// `buf` cut to `seconds`, faded out at the cut, with the silence the voice stops in after it.
fn with_tail(mut buf: SampleBuf, seconds: f64) -> (SampleBuf, f32) {
    let rate = f64::from(buf.rate.max(1));
    let keep = ((seconds * rate) as usize).min(buf.frames());
    if keep < buf.frames() {
        buf.data.truncate(keep * 2);
        let fade = ((0.02 * rate) as usize).min(keep);
        for i in 0..fade {
            let g = i as f32 / fade as f32;
            let f = keep - 1 - i;
            buf.data[f * 2] *= g;
            buf.data[f * 2 + 1] *= g;
        }
    }
    let sound = buf.frames().max(1);
    let tail = (TAIL_SECONDS * rate) as usize;
    buf.data.resize((sound + tail) * 2, 0.0);
    let end = sound as f32 / (sound + tail) as f32;
    (buf, end)
}

/// The first `seconds` of a file (all of it when shorter): the rest is not read, so a long
/// loop on a slow disk is heard as soon as its start is.
fn decode_head(path: &Path, seconds: f64) -> Result<SampleBuf, String> {
    let head: std::cell::RefCell<Option<SampleBuf>> = std::cell::RefCell::new(None);
    let whole = prowave_engine::sample_tools::decode_with_head(
        path,
        seconds,
        |b| *head.borrow_mut() = Some(b),
        // Stop reading once the head is there.
        |_| head.borrow().is_none(),
    );
    match head.into_inner() {
        Some(h) => Ok(h),
        None => whole,
    }
}

fn sample_preview(path: &Path) -> Ready {
    let buf = decode_head(path, MAX_AUDITION_SECONDS)?;
    if buf.frames() < 2 {
        return Err(format!("{}: empty", path.display()));
    }
    Ok(with_tail(buf, MAX_AUDITION_SECONDS))
}

/// A kit's hits one after the other, each in its place (a file that cannot be read leaves
/// its place silent).
fn kit_preview(paths: &[PathBuf]) -> Ready {
    let mut out: Option<SampleBuf> = None;
    for (hit, path) in paths.iter().enumerate() {
        let Ok(buf) = decode_head(path, KIT_HIT_SECONDS) else {
            continue;
        };
        if buf.frames() < 2 {
            continue;
        }
        let mix = out.get_or_insert_with(|| SampleBuf::new(buf.rate, Vec::new()));
        let rate = f64::from(mix.rate.max(1));
        let start = (hit as f64 * KIT_STEP_SECONDS * rate) as usize;
        let ratio = f64::from(buf.rate.max(1)) / rate;
        let frames =
            ((buf.frames() as f64 / ratio) as usize).min((KIT_HIT_SECONDS * rate) as usize);
        if mix.data.len() < (start + frames) * 2 {
            mix.data.resize((start + frames) * 2, 0.0);
        }
        let fade = ((0.03 * rate) as usize).min(frames);
        for i in 0..frames {
            let pos = i as f64 * ratio;
            let a = (pos as usize).min(buf.frames() - 1);
            let b = (a + 1).min(buf.frames() - 1);
            let t = (pos - a as f64) as f32;
            let g = if frames - i <= fade {
                (frames - i) as f32 / fade as f32
            } else {
                1.0
            };
            for c in 0..2 {
                let s = buf.data[a * 2 + c] + (buf.data[b * 2 + c] - buf.data[a * 2 + c]) * t;
                mix.data[(start + i) * 2 + c] += s * g;
            }
        }
    }
    let mut mix = out.ok_or_else(|| "nothing to play".to_string())?;
    // Hits that ring into each other are not let past full scale.
    let peak = mix.data.iter().fold(0f32, |m, s| m.max(s.abs()));
    if peak > 0.98 {
        let g = 0.98 / peak;
        for s in &mut mix.data {
            *s *= g;
        }
    }
    Ok(with_tail(mix, f64::MAX))
}

/// Tells the audition voice what to play and stops it when the sound has ended (the voice
/// loops; the silence after the sound is the time there is to stop it).
pub fn pump(v: &mut KitBrowser, remote: Option<&mut AuditionRemote>) {
    if let Some((key, rx)) = v.want.take() {
        match rx.try_recv() {
            Ok(Ok((buf, end))) => v.send = Some((key, Arc::new(buf), end)),
            Ok(Err(e)) => v.error = Some(e),
            Err(TryRecvError::Empty) => v.want = Some((key, rx)),
            Err(TryRecvError::Disconnected) => {}
        }
    }
    let Some(remote) = remote else {
        // No engine: nothing plays.
        v.send = None;
        v.playing = None;
        v.stop = false;
        return;
    };
    if v.stop {
        if remote.send(AuditionMsg::Stop).is_ok() {
            v.stop = false;
            v.playing = None;
        }
        return;
    }
    if let Some((key, buf, end)) = v.send.take() {
        let mut layers: prowave_engine::AuditionLayers = Default::default();
        layers[0] = Some(buf);
        let mut gains = [0.0f32; AUDITION_LAYERS];
        gains[0] = 1.0;
        let _ = remote.send(AuditionMsg::Sync(false));
        let _ = remote.send(AuditionMsg::Volume(1.0));
        let sent = remote.send(AuditionMsg::Play {
            layers,
            gains,
            beats: 4.0,
            from_start: true,
        });
        if sent.is_ok() {
            v.playing = Some(Playing {
                key,
                end,
                last: 0.0,
                heard: false,
            });
        }
        return;
    }
    if let Some(p) = &mut v.playing {
        match remote.status.phase() {
            Some(phase) => {
                let over = phase >= p.end || (p.heard && phase + 0.05 < p.last);
                p.heard = true;
                p.last = phase;
                if over {
                    v.stop = true;
                }
            }
            None if p.heard => v.playing = None,
            None => {}
        }
    }
    remote.collect();
}

/// Plays what the browser has been asked to audition on an audition voice of its own, as the
/// engine's plays it, and returns what came out (stereo, interleaved, at `rate`): the
/// audition heard without a sound card, for the control interface and the tests. Waits up to
/// `wait` for the sound to be read; ends when the browser stops the voice, or at `max_seconds`.
pub fn capture(v: &mut KitBrowser, rate: u32, max_seconds: f64, wait: Duration) -> Vec<f32> {
    const BLOCK: usize = 256;
    let (mut voice, mut remote) = prowave_engine::Auditioner::new(rate);
    let started = std::time::Instant::now();
    while v.want.is_some() && started.elapsed() < wait {
        pump(v, Some(&mut remote));
        if v.want.is_some() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let mut out = Vec::new();
    let most = (max_seconds * f64::from(rate)) as usize;
    let mut began = false;
    while out.len() / 2 < most {
        pump(v, Some(&mut remote));
        began |= v.playing.is_some();
        if began && v.playing.is_none() && v.send.is_none() && !v.stop {
            break;
        }
        voice.begin_block();
        for _ in 0..BLOCK {
            let (l, r) = voice.frame(f64::NAN, 0.0);
            out.push(l);
            out.push(r);
        }
        voice.end_block();
    }
    remote.collect();
    out
}

// ------------------------------------------------------------------------------- drawing

fn star_points(c: Pos2, r: f32) -> Vec<Pos2> {
    (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let rr = if i % 2 == 0 { r } else { r * 0.44 };
            pos2(c.x + a.cos() * rr, c.y + a.sin() * rr)
        })
        .collect()
}

/// A star: filled when `on`, an outline otherwise.
fn star(g: &egui::Painter, c: Pos2, r: f32, on: bool, color: Color32) {
    let pts = star_points(c, r);
    if on {
        for i in 0..5 {
            let tip = pts[i * 2];
            let a = pts[(i * 2 + 9) % 10];
            let b = pts[(i * 2 + 1) % 10];
            g.add(egui::Shape::convex_polygon(
                vec![tip, b, c, a],
                color,
                Stroke::NONE,
            ));
        }
    } else {
        g.add(egui::Shape::closed_line(pts, Stroke::new(1.2, color)));
    }
}

/// The triangle of a sample (a square while it plays).
fn play_glyph(g: &egui::Painter, c: Pos2, playing: bool, color: Color32) {
    if playing {
        g.rect_filled(Rect::from_center_size(c, vec2(8.0, 8.0)), 1.0, color);
    } else {
        g.add(egui::Shape::convex_polygon(
            vec![
                pos2(c.x - 3.5, c.y - 5.0),
                pos2(c.x + 5.0, c.y),
                pos2(c.x - 3.5, c.y + 5.0),
            ],
            color,
            Stroke::NONE,
        ));
    }
}

/// The floor of a list, with its name printed over it.
fn floor(g: &egui::Painter, rect: Rect, label: &str) -> Rect {
    hw::silk(
        g,
        pos2(rect.left() + 2.0, rect.top() + 6.0),
        Align2::LEFT_CENTER,
        label,
        10.5,
        theme::INK2(),
    );
    let r = Rect::from_min_max(pos2(rect.left(), rect.top() + 16.0), rect.max);
    g.rect_filled(r, 3.0, theme::BONE2());
    hw::inner_shadow(g, r, 3.0, 0.18);
    g.rect_stroke(
        r,
        3.0,
        Stroke::new(1.0, theme::SEAM()),
        egui::StrokeKind::Inside,
    );
    r.shrink(2.0)
}

/// The colours of a list row: the picked one is a black bar with bone print.
fn row_look(g: &egui::Painter, r: Rect, selected: bool, hovered: bool) -> (Color32, Color32) {
    if selected {
        g.rect_filled(r.shrink2(vec2(2.0, 1.0)), 3.0, theme::CAP());
        (theme::ON_DARK(), theme::ON_DARK_DIM())
    } else {
        if hovered {
            g.rect_filled(r.shrink2(vec2(2.0, 1.0)), 3.0, theme::BONE_HI());
        }
        (theme::INK(), theme::PRINT())
    }
}

fn number(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What the window asks of the project, done after it is drawn.
#[derive(Default)]
struct Act {
    load: bool,
    assign: Option<(usize, PathBuf)>,
    soundfont: Option<PathBuf>,
}

/// The rack's channels as the pads show them: (name, has a sample).
fn rack_pads(p: &Project) -> Vec<(String, bool)> {
    channel_rack::rack_track(p)
        .and_then(|i| p.tracks[i].au_instrument.as_ref())
        .map(|slot| {
            channel_rack::decode(&slot.state)
                .into_iter()
                .map(|c| (c.name, !c.path.is_empty()))
                .collect()
        })
        .unwrap_or_default()
}

/// The window's id.
fn window_id() -> Id {
    Id::unique("prowave_kit_browser")
}

/// The Kit Browser's window.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    let v = &mut s.kits;
    if finish_copy(v, p).0 {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    if !v.open {
        if v.audition_key().is_some() {
            v.stop_audition();
        }
        v.rect = None;
        return;
    }
    v.start();
    v.poll();
    let id = window_id();
    // The keys are the window's from when it opens, and after a click in it, until a click
    // somewhere else.
    if v.rect.is_none() {
        v.keys = true;
    }
    if ctx.input(|i| i.pointer.any_pressed()) {
        if let Some(at) = ctx.input(|i| i.pointer.press_origin()) {
            v.keys = ctx.layer_id_at(at).is_some_and(|l| l.id == id);
        }
    }
    let name = v.selected_name();
    let title = tr("Kits").to_uppercase();
    let plate = gear::Plate {
        model: &title,
        name: &name,
        strip: None,
        reset: false,
    };
    let pads = rack_pads(p);
    let mut act = Act::default();
    let (head, _) = gear::unit_window(ctx, id, WIDTH, &plate, None, |ui| {
        body(ui, id, v, &pads, &mut act);
    });
    v.rect = ctx.memory(|m| m.area_rect(id)).or(Some(Rect::NOTHING));
    if head.close {
        v.open = false;
        v.stop_audition();
    }
    if act.load {
        load_selected(v, p);
    }
    if let Some((channel, path)) = act.assign {
        assign(v, p, channel, &path);
    }
    if let Some(path) = act.soundfont {
        let name = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        load_soundfont(p, &name, &path);
    }
    // The sample being dragged, by the pointer.
    if let (Some(drag), Some(at)) = (
        egui::DragAndDrop::payload::<SampleDrag>(ctx),
        ctx.pointer_interact_pos(),
    ) {
        let g = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, id.with("drag")));
        let label = drag
            .0
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let r = Rect::from_min_size(at + vec2(12.0, 10.0), vec2(170.0, 22.0));
        hw::paint_slot(
            &g,
            r,
            hw::SlotKind::Filled,
            &label,
            Some(true),
            false,
            false,
        );
    }
    if v.busy() {
        ctx.request_repaint_after(Duration::from_millis(40));
    }
}

/// The arrow keys and Return pressed for the window this frame.
#[derive(Default)]
struct Nav {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    enter: bool,
}

/// Takes the window's keys out of the input, so the arrange area does not also get them:
/// Up, Down and Return while the search field has the keys (`typing`); those and Left and
/// Right while the window has them.
fn take_keys(ui: &Ui, typing: bool) -> Nav {
    use egui::{Key, Modifiers};
    ui.input_mut(|i| {
        let mut take = |k: Key| i.consume_key(Modifiers::NONE, k);
        Nav {
            up: take(Key::ArrowUp),
            down: take(Key::ArrowDown),
            enter: take(Key::Enter),
            left: !typing && take(Key::ArrowLeft),
            right: !typing && take(Key::ArrowRight),
        }
    })
}

/// The samples the list shows: all of them, or those of the class whose key is lit.
fn shown_samples(v: &KitBrowser) -> Vec<usize> {
    let Some(kit) = &v.kit else {
        return Vec::new();
    };
    (0..kit.samples.len())
        .filter(|i| v.class.is_none_or(|c| kit.samples[*i].class == c))
        .collect()
}

/// What the arrows and Return do.
fn navigate(v: &mut KitBrowser, nav: &Nav, typing: bool, act: &mut Act) {
    if typing || nav.left {
        v.column = Column::Kits;
    }
    if nav.right && v.kit.is_some() {
        v.column = Column::Samples;
        if v.sample.is_none() {
            if let Some(first) = shown_samples(v).first() {
                v.pick_sample(*first);
                v.scroll_sample = Some(0);
            }
        }
        return;
    }
    let step: i64 = match (nav.up, nav.down) {
        (true, false) => -1,
        (false, true) => 1,
        _ => 0,
    };
    match v.column {
        Column::Kits => {
            let results: Vec<u32> = v.results().to_vec();
            if step != 0 && !results.is_empty() {
                let at = v.selected.as_ref().and_then(|sel| {
                    results
                        .iter()
                        .position(|i| &v.index.kits[*i as usize].path == sel)
                });
                let to = match at {
                    Some(a) => (a as i64 + step).clamp(0, results.len() as i64 - 1) as usize,
                    None => 0,
                };
                let path = v.index.kits[results[to] as usize].path.clone();
                v.select(&path);
                v.scroll_to = Some(to);
            }
            if nav.enter {
                act.load = true;
            }
        }
        Column::Samples => {
            let shown = shown_samples(v);
            if step != 0 && !shown.is_empty() {
                let at = v.sample.and_then(|s| shown.iter().position(|i| *i == s));
                let to = match at {
                    Some(a) => (a as i64 + step).clamp(0, shown.len() as i64 - 1) as usize,
                    None => 0,
                };
                v.pick_sample(shown[to]);
                v.scroll_sample = Some(to);
            }
            if nav.enter {
                if let (Some(kit), Some(i)) = (v.kit.clone(), v.sample) {
                    if kit.samples[i].class == DrumClass::SoundFont {
                        act.soundfont = Some(kit.path_of(i));
                    } else {
                        act.assign = Some((v.pad, kit.path_of(i)));
                    }
                }
            }
        }
    }
}

fn body(ui: &mut Ui, id: Id, v: &mut KitBrowser, pads: &[(String, bool)], act: &mut Act) {
    let pads_h = 20.0 + 2.0 * 46.0;
    let total = 12.0 + 30.0 + 10.0 + (16.0 + LIST_H) + 14.0 + pads_h + 14.0 + 30.0;
    let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, total), Sense::hover());
    let g = ui.painter().clone();
    let left = rect.left() + PAD;
    let right = rect.right() - PAD;
    let mut y = rect.top() + 12.0;

    // ---- the search field, the filters, the count
    let count_w = 112.0;
    let keys: [(&str, f32); 3] = [
        (tr("All"), 62.0),
        (tr("Best"), 68.0),
        (tr("Favorites"), 112.0),
    ];
    let keys_w: f32 = keys.iter().map(|k| k.1 + 6.0).sum();
    let label = tr("Search");
    let lw = hw::silk_width(&g, label, 10.5);
    hw::silk(
        &g,
        pos2(left + 2.0, y + 15.0),
        Align2::LEFT_CENTER,
        label,
        10.5,
        theme::INK2(),
    );
    let field = Rect::from_min_max(
        pos2(left + lw + 12.0, y + 2.0),
        pos2(right - count_w - 10.0 - keys_w - 4.0, y + 28.0),
    );
    hw::paint_field(&g, field, "", false, false);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink2(vec2(6.0, 4.0))));
    let edit = crate::hw_sampler::fields(&mut child, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut v.filter.query)
                .id(id.with("search"))
                .frame(egui::Frame::NONE)
                .font(theme::font(Face::Bold, 12.0))
                .text_color(theme::ON_DARK())
                .desired_width(field.width() - 12.0),
        )
    });
    // The keys: the search field's, or the window's while no other field has them.
    let typing = edit.has_focus() || edit.lost_focus();
    if typing || (v.keys && !ui.ctx().egui_wants_keyboard_input()) {
        let nav = take_keys(ui, typing);
        navigate(v, &nav, typing, act);
    }
    let mut x = field.right() + 10.0;
    let lit = [
        !v.filter.best && !v.filter.favorites,
        v.filter.best,
        v.filter.favorites,
    ];
    for (k, (text, w)) in keys.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, y + 1.0), vec2(*w, 28.0));
        if gear::key(
            ui,
            r,
            id.with(("filter", k)),
            text,
            Cap::Bone,
            LedAt::Left,
            lit[k],
            lit[k],
        )
        .clicked()
        {
            v.filter.best = k == 1;
            v.filter.favorites = k == 2;
        }
        x += w + 6.0;
    }
    let total_kits = v.results().len();
    gear::lcd_chip(
        &g,
        Rect::from_min_max(pos2(right - count_w, y + 1.0), pos2(right, y + 29.0)),
        tr("Kits"),
        &total_kits.to_string(),
    );
    y += 30.0 + 10.0;

    // ---- the three lists, or the folders
    let lists = Rect::from_min_max(pos2(left, y), pos2(right, y + 16.0 + LIST_H));
    if v.show_folders {
        folders(ui, id, v, lists);
    } else {
        let sources = Rect::from_min_size(lists.min, vec2(SOURCES_W, lists.height()));
        let kits = Rect::from_min_size(
            pos2(sources.right() + GAP, lists.top()),
            vec2(KITS_W, lists.height()),
        );
        let kit = Rect::from_min_max(pos2(kits.right() + GAP, lists.top()), lists.max);
        source_list(ui, id, v, sources);
        kit_list(ui, id, v, kits, act);
        kit_pane(ui, id, v, kit, act);
    }
    y = lists.bottom() + 14.0;

    // ---- the rack's channels as pads
    hw::bracket(&g, left, right, y + 8.0, tr("Channel Rack"));
    y += 20.0;
    let cols = KIT_PADS / 2;
    let pw = (right - left - (cols as f32 - 1.0) * 6.0) / cols as f32;
    for k in 0..KIT_PADS {
        let r = Rect::from_min_size(
            pos2(
                left + (k % cols) as f32 * (pw + 6.0),
                y + (k / cols) as f32 * 46.0,
            ),
            vec2(pw, 40.0),
        );
        let resp = ui.interact(r, id.with(("pad", k)), Sense::click());
        let selected = v.pad == k;
        let (name, filled) = pads
            .get(k)
            .map(|(n, f)| (n.as_str(), *f))
            .unwrap_or(("", false));
        let cap = if selected { Cap::Orange } else { Cap::Bone };
        let face = hw::paint_key(&g, r, cap, resp.is_pointer_button_down_on(), resp.hovered());
        hw::silk(
            &g,
            pos2(face.left() + 7.0, face.top() + 9.0),
            Align2::LEFT_CENTER,
            &format!("{:02}", k + 1),
            10.0,
            cap.ink(),
        );
        hw::led_sized(&g, pos2(face.right() - 9.0, face.top() + 9.0), 2.5, filled);
        let shown = hw::fit(&g, name, Face::Bold, 10.5, face.width() - 12.0);
        hw::text(
            &g,
            pos2(face.left() + 7.0, face.bottom() - 11.0),
            Align2::LEFT_CENTER,
            &shown.to_uppercase(),
            Face::Bold,
            10.5,
            cap.ink(),
        );
        if resp.dnd_hover_payload::<SampleDrag>().is_some() {
            g.rect_stroke(
                r,
                4.0,
                Stroke::new(2.0, theme::ORANGE()),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(drop) = resp.dnd_release_payload::<SampleDrag>() {
            act.assign = Some((k, drop.0.clone()));
            v.pad = k;
        }
        if resp.clicked() {
            v.pad = k;
        }
    }
    y += 2.0 * 46.0 + 14.0 - 6.0;

    // ---- the foot: Load Kit, Audition, Folders
    let can_load = v
        .kit
        .as_ref()
        .is_some_and(|k| !k.pads.is_empty() || !k.soundfonts().is_empty());
    let r = Rect::from_min_size(pos2(left, y), vec2(170.0, 30.0));
    if can_load {
        if gear::key(
            ui,
            r,
            id.with("load"),
            tr("Load Kit"),
            Cap::Orange,
            LedAt::None,
            false,
            false,
        )
        .clicked()
        {
            act.load = true;
        }
    } else {
        hu::key(
            ui,
            r,
            id.with("load_off"),
            Cap::Bone,
            false,
            None,
            &tr("Load Kit").to_uppercase(),
            false,
        );
    }
    // Audition: lit, a sample or kit picked is heard; lighting it plays what is picked.
    let r = Rect::from_min_size(pos2(r.right() + 8.0, y), vec2(140.0, 30.0));
    let on = v.settings.audition;
    if gear::key(
        ui,
        r,
        id.with("audition"),
        tr("Audition"),
        Cap::Bone,
        LedAt::Left,
        on,
        on,
    )
    .clicked()
    {
        v.set_audition(!on);
        if !on {
            match (v.column, v.sample) {
                (Column::Samples, Some(i)) => v.audition_sample(i, false),
                _ => v.audition_kit(),
            }
        }
    }
    let r = Rect::from_min_max(pos2(right - 130.0, y), pos2(right, y + 30.0));
    if gear::key(
        ui,
        r,
        id.with("folders"),
        tr("Folders"),
        Cap::Bone,
        LedAt::Left,
        v.show_folders,
        v.show_folders,
    )
    .clicked()
    {
        v.show_folders = !v.show_folders;
    }
    // The lamp beside Folders is lit while the folders or the kits' files are being read.
    if v.scanning || v.indexing() {
        hw::led_sized(&g, pos2(r.left() - 14.0, r.center().y), 3.0, true);
    }
}

/// The left column: the sources, or with the kit list on its types. Right-click turns the kit
/// list off or on (when one is set).
fn source_list(ui: &mut Ui, id: Id, v: &mut KitBrowser, rect: Rect) {
    let g = ui.painter().clone();
    let by_type = v.list_active();
    let inner = floor(&g, rect, if by_type { tr("Type") } else { tr("Source") });
    // (name, kits): the column's rows after All.
    let rows: Vec<(String, usize)> = if by_type {
        v.index.types.clone()
    } else {
        v.index.sources.clone()
    };
    let all: usize = if by_type {
        v.results_all()
    } else {
        rows.iter().map(|s| s.1).sum()
    };
    let shown_pick = if by_type {
        v.filter.kind.clone()
    } else {
        v.filter.source.clone()
    };
    let n = rows.len() + 1;
    let mut pick: Option<Option<String>> = None;
    let mut list_on: Option<bool> = None;
    let has_list = !v.settings.kit_list.is_empty() && v.list_now().is_some();
    let column = ui.interact(rect, id.with("source_column"), Sense::click());
    if has_list {
        let on = v.settings.kit_list_on;
        column.context_menu(|ui| {
            if ui.selectable_label(on, tr("Kit List")).clicked() {
                list_on = Some(!on);
                ui.close();
            }
        });
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    egui::ScrollArea::vertical()
        .id_salt(id.with("sources"))
        .auto_shrink([false, false])
        .show_rows(&mut child, ROW, n, |ui, range| {
            for i in range {
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                let g = ui.painter();
                let (name, count, selected) = if i == 0 {
                    (tr("All").to_uppercase(), all, shown_pick.is_none())
                } else {
                    let (s, c) = &rows[i - 1];
                    (s.clone(), *c, shown_pick.as_ref() == Some(s))
                };
                let (ink, dim) = row_look(g, r, selected, resp.hovered());
                let num = number(count);
                let nr = hw::text(
                    g,
                    pos2(r.right() - 10.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    &num,
                    Face::Bold,
                    10.0,
                    dim,
                );
                let shown = hw::fit(g, &name, Face::Bold, 11.0, nr.left() - r.left() - 20.0);
                hw::text(
                    g,
                    pos2(r.left() + 10.0, r.center().y),
                    Align2::LEFT_CENTER,
                    &shown,
                    Face::Bold,
                    11.0,
                    ink,
                );
                if resp.clicked() {
                    pick = Some(if i == 0 {
                        None
                    } else {
                        Some(rows[i - 1].0.clone())
                    });
                }
                if has_list {
                    let on = v.settings.kit_list_on;
                    resp.context_menu(|ui| {
                        if ui.selectable_label(on, tr("Kit List")).clicked() {
                            list_on = Some(!on);
                            ui.close();
                        }
                    });
                }
            }
        });
    if let Some(choice) = pick {
        if by_type {
            v.filter.kind = choice;
        } else {
            v.filter.source = choice;
        }
    }
    if let Some(on) = list_on {
        v.set_list_on(on);
    }
}

fn kit_list(ui: &mut Ui, id: Id, v: &mut KitBrowser, rect: Rect, act: &mut Act) {
    let g = ui.painter().clone();
    let inner = floor(&g, rect, tr("Kit"));
    let results: Vec<u32> = v.results().to_vec();
    let mut pick: Option<String> = None;
    let mut fav: Option<(String, bool)> = None;
    let mut default: Option<String> = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let mut area = egui::ScrollArea::vertical()
        .id_salt(id.with("kits"))
        .auto_shrink([false, false]);
    if let Some(row) = v.scroll_to.take() {
        let want = (row as f32 * ROW - inner.height() / 2.0 + ROW / 2.0).max(0.0);
        area = area.vertical_scroll_offset(want);
    }
    area.show_rows(&mut child, ROW, results.len(), |ui, range| {
        for row in range {
            let k = &v.index.kits[results[row] as usize];
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
            let star_r = Rect::from_min_size(r.min, vec2(26.0, ROW));
            let star_resp = ui.interact(star_r, id.with(("fav", row)), Sense::click());
            let g = ui.painter();
            let selected = v.selected.as_deref() == Some(k.path.as_str());
            let (ink, dim) = row_look(g, r, selected, resp.hovered() || star_resp.hovered());
            let is_fav = v.settings.favorites.contains(&k.path);
            star(
                g,
                star_r.center(),
                6.0,
                is_fav,
                if is_fav { theme::ORANGE() } else { dim },
            );
            let mut text_right = r.right() - 10.0;
            if let Some(n) = k.files {
                let nr = hw::text(
                    g,
                    pos2(text_right, r.center().y),
                    Align2::RIGHT_CENTER,
                    &number(n as usize),
                    Face::Bold,
                    10.0,
                    dim,
                );
                text_right = nr.left() - 8.0;
            }
            let shown = hw::fit(
                g,
                &k.name,
                Face::Bold,
                11.0,
                text_right - star_r.right() - 2.0,
            );
            hw::text(
                g,
                pos2(star_r.right() + 2.0, r.center().y),
                Align2::LEFT_CENTER,
                &shown,
                Face::Bold,
                11.0,
                ink,
            );
            // The kit new racks start with: lit on its row's right edge.
            let is_default = v.settings.default_kit == k.path;
            if is_default {
                hw::led_sized(g, pos2(r.right() - 4.0, r.center().y), 2.5, true);
            }
            resp.context_menu(|ui| {
                if ui
                    .selectable_label(is_default, tr("Use for New Racks"))
                    .clicked()
                {
                    default = Some(if is_default {
                        String::new()
                    } else {
                        k.path.clone()
                    });
                    ui.close();
                }
            });
            if star_resp.clicked() {
                fav = Some((k.path.clone(), !is_fav));
            } else if resp.double_clicked() {
                pick = Some(k.path.clone());
                act.load = true;
            } else if resp.clicked() {
                pick = Some(k.path.clone());
            }
        }
    });
    if let Some((path, on)) = fav {
        v.set_favorite(&path, on);
    }
    if let Some(path) = default {
        v.set_default_kit(&path);
    }
    if let Some(path) = pick {
        v.column = Column::Kits;
        v.select(&path);
    }
}

/// The kit picked: its name, source and license on an LCD, the classes it holds as keys, and
/// its samples.
fn kit_pane(ui: &mut Ui, id: Id, v: &mut KitBrowser, rect: Rect, act: &mut Act) {
    let g = ui.painter().clone();
    let entry = v
        .selected
        .as_ref()
        .and_then(|sel| v.index.find(sel))
        .cloned();
    let glass = Rect::from_min_max(
        pos2(rect.left(), rect.top() + 2.0),
        pos2(rect.right(), rect.top() + 56.0),
    );
    gear::glass(&g, glass);
    let kit = v.kit.clone();
    if let Some(e) = &entry {
        let count = kit
            .as_ref()
            .map(|k| k.samples.len())
            .or(e.files.map(|n| n as usize));
        let mut name_right = glass.right() - 8.0;
        if let Some(n) = count {
            let text = n.to_string();
            let h = 16.0;
            let w = gear::seg7_width(&text, h);
            gear::seg7(
                &g,
                pos2(glass.right() - 8.0 - w, glass.top() + 8.0),
                &text,
                h,
                theme::LCD_INK(),
                false,
            );
            name_right -= w + 10.0;
        }
        let name = gear::fit(&g, &e.name, 13.0, name_right - glass.left() - 8.0);
        gear::lcd_text(
            &g,
            pos2(glass.left() + 8.0, glass.top() + 16.0),
            Align2::LEFT_CENTER,
            &name,
            13.0,
        );
        let line = if e.license.is_empty() {
            e.source.clone()
        } else {
            format!("{}  ·  {}", e.source, e.license)
        };
        let line = gear::fit(&g, &line, 9.0, glass.width() - 16.0);
        gear::lcd_label(
            &g,
            pos2(glass.left() + 8.0, glass.bottom() - 13.0),
            Align2::LEFT_CENTER,
            &line,
        );
    }
    // The classes the kit holds: a key each, lit when the list shows that class alone.
    let mut y = glass.bottom() + 8.0;
    let classes: Vec<(DrumClass, usize)> =
        kit.as_ref().map(|k| k.classes.clone()).unwrap_or_default();
    if classes.len() > 1 {
        let mut x = rect.left();
        for (c, _) in &classes {
            let w = hw::silk_width(&g, c.tag(), 9.5) + 18.0;
            if x + w > rect.right() {
                x = rect.left();
                y += 24.0;
            }
            let r = Rect::from_min_size(pos2(x, y), vec2(w, 20.0));
            let on = v.class == Some(*c);
            if gear::key(
                ui,
                r,
                id.with(("class", c.id())),
                c.tag(),
                if on { Cap::Dark } else { Cap::Bone },
                LedAt::None,
                false,
                on,
            )
            .clicked()
            {
                v.class = if on { None } else { Some(*c) };
            }
            x += w + 4.0;
        }
        y += 24.0;
    }
    let list = Rect::from_min_max(pos2(rect.left(), y + 2.0), rect.max);
    let inner = floor(&g, list, tr("Samples"));
    let Some(kit) = kit else {
        return;
    };
    let shown = shown_samples(v);
    let playing = v.audition_key().map(str::to_string);
    let mut hear: Option<usize> = None;
    let mut pick: Option<usize> = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let mut area = egui::ScrollArea::vertical()
        .id_salt(id.with(("samples", v.selected.as_deref().unwrap_or(""))))
        .auto_shrink([false, false]);
    if let Some(row) = v.scroll_sample.take() {
        let want = (row as f32 * ROW - inner.height() / 2.0 + ROW / 2.0).max(0.0);
        area = area.vertical_scroll_offset(want);
    }
    let picked = v.sample;
    let keys_here = v.column == Column::Samples;
    area.show_rows(&mut child, ROW, shown.len(), |ui, range| {
        for row in range {
            let i = shown[row];
            let sample = &kit.samples[i];
            let sf2 = sample.class == DrumClass::SoundFont;
            let (r, resp) = ui.allocate_exact_size(
                vec2(ui.available_width(), ROW),
                if sf2 {
                    Sense::click()
                } else {
                    Sense::click_and_drag()
                },
            );
            let well =
                Rect::from_center_size(pos2(r.left() + 17.0, r.center().y), vec2(20.0, 20.0));
            let hear_resp = ui.interact(well, id.with(("hear", i)), Sense::click());
            let g = ui.painter();
            let on_pad = kit.pads.contains(&Some(i));
            let selected = picked == Some(i);
            let (ink, _) = row_look(g, r, selected, resp.hovered() || hear_resp.hovered());
            // The row the arrows are on, while they move in the samples: an orange edge.
            if selected && keys_here {
                g.rect_stroke(
                    r.shrink2(vec2(2.0, 1.0)),
                    3.0,
                    Stroke::new(1.5, theme::ORANGE()),
                    egui::StrokeKind::Inside,
                );
            }
            let path = kit.path_of(i);
            let key = format!("sample:{}", path.display());
            hu::well(g, well);
            if !sf2 {
                play_glyph(
                    g,
                    well.center(),
                    playing.as_deref() == Some(key.as_str()),
                    theme::INK(),
                );
            }
            let tag = hu::tag(
                g,
                pos2(
                    r.right() - 8.0 - hw::silk_width(g, sample.class.tag(), 8.5) - 10.0,
                    r.center().y,
                ),
                sample.class.tag(),
                8.5,
            );
            // A lamp on the samples a kit load puts on the channels.
            if on_pad {
                hw::led_sized(g, pos2(tag.left() - 9.0, r.center().y), 2.5, true);
            }
            let name_x = well.right() + 8.0;
            let name = hw::fit(
                g,
                &sample.name,
                Face::Bold,
                11.0,
                tag.left() - 18.0 - name_x,
            );
            hw::text(
                g,
                pos2(name_x, r.center().y),
                Align2::LEFT_CENTER,
                &name,
                Face::Bold,
                11.0,
                ink,
            );
            if hear_resp.clicked() {
                hear = Some(i);
            } else if resp.double_clicked() {
                if sf2 {
                    act.soundfont = Some(path.clone());
                } else {
                    act.assign = Some((v.pad, path.clone()));
                }
            } else if resp.clicked() {
                pick = Some(i);
            }
            if !sf2 && resp.drag_started() {
                resp.dnd_set_drag_payload(SampleDrag(path));
            }
        }
    });
    if let Some(i) = hear {
        v.sample = Some(i);
        v.column = Column::Samples;
        v.audition_sample(i, true);
    } else if let Some(i) = pick {
        v.pick_sample(i);
    }
}

/// The library folders: a lamp (lit when the folder is there), the folder (marked missing when
/// it is not there), its kits, Remove; then Add Folder and Rescan, and what the last scan found
/// (the kits whose files are read so far, while they are being read).
fn folders(ui: &mut Ui, id: Id, v: &mut KitBrowser, rect: Rect) {
    let g = ui.painter().clone();
    let inner = floor(&g, rect, tr("Folders"));
    let mut remove: Option<usize> = None;
    let mut y = inner.top() + 6.0;
    let folders = v.settings.folders.clone();
    for (i, f) in folders.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(inner.left() + 6.0, y),
            vec2(inner.width() - 12.0, 30.0),
        );
        let info = v.index.roots.iter().find(|r| &r.path == f);
        let present = info.map_or_else(|| Path::new(f).is_dir(), |r| r.present);
        hw::led_sized(&g, pos2(r.left() + 10.0, r.center().y), 3.5, present);
        let key = Rect::from_min_max(
            pos2(r.right() - 96.0, r.top() + 2.0),
            pos2(r.right(), r.bottom() - 2.0),
        );
        let count = Rect::from_min_max(
            pos2(key.left() - 104.0, r.top() + 2.0),
            pos2(key.left() - 8.0, r.bottom() - 2.0),
        );
        if present {
            gear::lcd_chip(&g, count, "", &info.map_or(0, |r| r.kits).to_string());
        }
        let text = if present {
            f.clone()
        } else {
            format!("{f} · {}", tr("missing"))
        };
        let shown = hw::fit(&g, &text, Face::Bold, 11.5, count.left() - r.left() - 34.0);
        hw::text(
            &g,
            pos2(r.left() + 24.0, r.center().y),
            Align2::LEFT_CENTER,
            &shown,
            Face::Bold,
            11.5,
            if present {
                theme::INK()
            } else {
                theme::PRINT()
            },
        );
        if gear::key(
            ui,
            key,
            id.with(("remove", i)),
            tr("Remove"),
            Cap::Bone,
            LedAt::None,
            false,
            false,
        )
        .clicked()
        {
            remove = Some(i);
        }
        g.hline(
            r.left()..=r.right(),
            r.bottom() + 2.5,
            Stroke::new(1.0, Color32::from_black_alpha(22)),
        );
        y += 36.0;
    }
    y += 6.0;
    let add = Rect::from_min_size(pos2(inner.left() + 6.0, y), vec2(150.0, 28.0));
    if gear::key(
        ui,
        add,
        id.with("add_folder"),
        tr("Add Folder…"),
        Cap::Bone,
        LedAt::None,
        false,
        false,
    )
    .clicked()
    {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            v.add_folder(&dir);
        }
    }
    let rescan = Rect::from_min_size(pos2(add.right() + 8.0, y), vec2(120.0, 28.0));
    if gear::key(
        ui,
        rescan,
        id.with("rescan"),
        tr("Rescan"),
        Cap::Bone,
        LedAt::Left,
        v.scanning || v.indexing(),
        false,
    )
    .clicked()
    {
        v.rescan();
    }
    let text = if let Some((done, total)) = v.listing {
        Some(format!("{} / {}", number(done), number(total)))
    } else {
        v.last_scan.map(|took| {
            let ms = took.as_secs_f64() * 1000.0;
            if ms < 10.0 {
                format!("{ms:.1} ms")
            } else {
                format!("{ms:.0} ms")
            }
        })
    };
    if let Some(text) = text {
        gear::lcd_chip(
            &g,
            Rect::from_min_size(pos2(inner.right() - 6.0 - 150.0, y), vec2(150.0, 28.0)),
            "",
            &text,
        );
    }
    // The kit list, when one is set: its file and the kits of it found, and its key.
    if !v.settings.kit_list.is_empty() {
        let r = Rect::from_min_size(
            pos2(inner.left() + 6.0, y + 40.0),
            vec2(inner.width() - 12.0, 28.0),
        );
        let key = Rect::from_min_max(pos2(r.right() - 130.0, r.top()), r.max);
        let read = v.list_now().is_some();
        hw::led_sized(&g, pos2(r.left() + 10.0, r.center().y), 3.5, read);
        let text = match &v.list {
            Some(Ok(l)) => format!(
                "{} · {} / {}",
                l.file,
                number(v.index.list_matched()),
                number(l.rows.len())
            ),
            Some(Err(e)) => e.clone(),
            None => v.settings.kit_list.clone(),
        };
        let shown = hw::fit(&g, &text, Face::Bold, 11.5, key.left() - r.left() - 34.0);
        hw::text(
            &g,
            pos2(r.left() + 24.0, r.center().y),
            Align2::LEFT_CENTER,
            &shown,
            Face::Bold,
            11.5,
            if read { theme::INK() } else { theme::PRINT() },
        );
        let on = v.settings.kit_list_on;
        if gear::key(
            ui,
            key,
            id.with("kit_list"),
            tr("Kit List"),
            Cap::Bone,
            LedAt::Left,
            on,
            on,
        )
        .clicked()
        {
            v.set_list_on(!on);
        }
    }
    if let Some(i) = remove {
        let mut list = v.settings.folders.clone();
        list.remove(i);
        v.set_folders(list);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prowave-kitui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_wav(path: &Path, seconds: f32, hz: f32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let n = (seconds * 44_100.0) as usize;
        let stereo: Vec<f32> = (0..n)
            .flat_map(|i| {
                let env = 1.0 - i as f32 / n as f32;
                let y = (i as f32 * hz * std::f32::consts::TAU / 44_100.0).sin() * env * 0.6;
                [y, y]
            })
            .collect();
        prowave_engine::write_wav(path, &stereo, 44_100).unwrap();
    }

    /// A small library of real audio files: two sources, three kits.
    fn library(name: &str) -> PathBuf {
        let dir = temp(name);
        for (kit, files) in [
            (
                "Machines/Roland TR-808",
                vec![
                    "BD/BD0000.wav",
                    "SD/SD0010.wav",
                    "CP/CP.wav",
                    "CH/CH.wav",
                    "OH/OH00.wav",
                    "RS/RS.wav",
                    "LT/LT00.wav",
                    "MT/MT00.wav",
                    "HT/HT00.wav",
                    "CB/CB.wav",
                    "MA/MA.wav",
                    "CY/CY0000.wav",
                    "BD/BD0025.wav",
                    "SD/SD0025.wav",
                    "CL/CL.wav",
                    "HC/HC00.wav",
                    "LC/LC00.wav",
                    "Loops/120bpm/Beat 1.wav",
                ],
            ),
            ("Machines/LinnDrum", vec!["kick.wav", "sd.wav", "chh.wav"]),
            (
                "Packs/Dusty Breaks Vol 2",
                vec!["Kicks/Kick 1.wav", "Snares/Snare 1.wav"],
            ),
        ] {
            for (i, f) in files.iter().enumerate() {
                write_wav(&dir.join(kit).join(f), 0.12, 80.0 + 60.0 * i as f32);
            }
        }
        dir
    }

    fn wait(v: &mut KitBrowser) {
        for _ in 0..2000 {
            v.poll();
            if !v.scanning && !v.indexing() && v.kit_rx.is_none() && v.list_rx.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the background work did not finish");
    }

    #[test]
    fn a_kit_is_found_read_and_loaded_into_a_new_rack() {
        let lib = library("load");
        let data = temp("load-data");
        let mut v = KitBrowser::with(Some(data.clone()), vec![lib.to_string_lossy().into_owned()]);
        v.rescan();
        wait(&mut v);
        assert_eq!(v.index.kits.len(), 3);
        assert!(data.join("index.json").is_file(), "the index is cached");
        v.filter.query = "808".into();
        let hits = v.results().to_vec();
        assert_eq!(hits.len(), 1);
        let path = v.index.kits[hits[0] as usize].path.clone();
        v.select(&path);
        wait(&mut v);
        let kit = v.kit().unwrap().clone();
        assert_eq!(kit.samples.len(), 18);
        assert_eq!(kit.pads.len(), KIT_PADS);
        let mut p = Project::empty();
        let ti = load_selected(&mut v, &mut p).unwrap();
        let slot = p.tracks[ti].au_instrument.clone().unwrap();
        assert_eq!(channel_rack::kit_of(&slot.state), "Roland TR-808");
        let names: Vec<String> = channel_rack::decode(&slot.state)
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            names,
            [
                "BD0000", "SD0010", "CP", "CH", "OH00", "RS", "LT00", "MT00", "HT00", "CB", "MA",
                "CY0000", "BD0025", "SD0025", "CL", "HC00"
            ]
        );
        // The engine's rack reads every one of them.
        let specs = channel_rack::specs(&slot);
        let (_, errors) = prowave_engine::SampleRack::new(&specs, 48_000);
        assert!(errors.is_empty(), "{errors:?}");
        // The files are named where they are: nothing is copied.
        assert!(specs[0].path.starts_with(&*lib.to_string_lossy()));
        // The count learnt by opening the kit is in the index.
        assert_eq!(v.index.kits[hits[0] as usize].files, Some(18));
        // One sample onto a channel past the kit's.
        assert!(assign(&v, &mut p, 16, &kit.path_of(0)));
        assert_eq!(
            channel_rack::decode(&p.tracks[ti].au_instrument.as_ref().unwrap().state).len(),
            17
        );
        // Copying set to always: the kit loaded is copied into kit-library/local/<source>/<kit>
        // and the rack plays the copies; a sample put on a channel too.
        v.settings.copy_kits = crate::kit_copy::CopyMode::Always;
        assert_eq!(
            load_selected(&mut v, &mut p),
            None,
            "copied on another thread first"
        );
        assert!(copying(&v));
        let ti = loop {
            match finish_copy(&mut v, &mut p) {
                (_, Some(ti)) => break ti,
                (true, None) => std::thread::sleep(Duration::from_millis(5)),
                (false, None) => panic!("the copy ended without a load"),
            }
        };
        assert!(!copying(&v));
        let specs = channel_rack::specs(p.tracks[ti].au_instrument.as_ref().unwrap());
        let e = v.index.kits[hits[0] as usize].clone();
        let local = crate::kit_copy::kit_dir(&data.join("local"), &e.source, &e.name);
        assert!(
            specs[0].path.starts_with(&*local.to_string_lossy()),
            "{}",
            specs[0].path
        );
        assert!(
            specs
                .iter()
                .filter(|s| !s.path.is_empty())
                .all(|s| Path::new(&s.path).is_file()
                    && s.path.starts_with(&*local.to_string_lossy()))
        );
        let (_, errors) = prowave_engine::SampleRack::new(&specs, 48_000);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(assign(&v, &mut p, 17, &kit.path_of(1)));
        let ch = channel_rack::decode(&p.tracks[ti].au_instrument.as_ref().unwrap().state);
        assert!(ch[17].path.starts_with(&*local.to_string_lossy()));
        let _ = std::fs::remove_dir_all(lib);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn favourites_and_folders_are_kept() {
        let lib = library("fav");
        let data = temp("fav-data");
        let root = lib.to_string_lossy().into_owned();
        let mut v = KitBrowser::with(Some(data.clone()), vec![root.clone()]);
        v.rescan();
        wait(&mut v);
        let path = format!("{root}/Machines/LinnDrum");
        v.set_favorite(&path, true);
        v.filter.favorites = true;
        assert_eq!(v.results().len(), 1);
        v.set_favorite(&path, false);
        assert!(v.results().is_empty());
        v.set_favorite(&path, true);
        // A folder that is not there beside one that is: its kits are simply not listed.
        v.set_folders(vec!["/Volumes/NOT-MOUNTED-HERE/KITS".into(), root.clone()]);
        wait(&mut v);
        assert_eq!(v.index.kits.len(), 3);
        assert!(!v.index.roots[0].present && v.index.roots[1].present);
        assert!(!v.show_folders);
        let saved = Settings::load(&data);
        assert_eq!(saved.folders.len(), 2);
        assert!(saved.favorites.contains(&path));
        // Only the missing folder: no error, an empty list, and Folders shown.
        v.set_folders(vec!["/Volumes/NOT-MOUNTED-HERE/KITS".into()]);
        wait(&mut v);
        assert!(v.index.kits.is_empty() && v.show_folders && v.error.is_none());
        let _ = std::fs::remove_dir_all(lib);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn a_kit_list_shows_only_its_kits_until_it_is_turned_off() {
        let lib = library("list");
        std::fs::write(
            lib.join("BEST-PROWAVE.tsv"),
            "rank\tsource\tkit\tpath\ttype\tfile_count\tscore\treason\n\
             1\tMachines\tRoland TR-808\tMachines/Roland TR-808\tHip-hop\t18\t90\t808\n\
             2\tMachines\tLinnDrum\tMachines/LinnDrum\tDrum machines\t3\t80\tlinn\n",
        )
        .unwrap();
        let data = temp("list-data");
        let root = lib.to_string_lossy().into_owned();
        let mut v = KitBrowser::with(Some(data.clone()), vec![root.clone()]);
        v.rescan();
        wait(&mut v);
        let names = |v: &mut KitBrowser| -> Vec<String> {
            v.results()
                .to_vec()
                .iter()
                .map(|i| v.index.kits[*i as usize].name.clone())
                .collect()
        };
        assert_eq!(names(&mut v).len(), 3, "no list: every kit");
        assert!(!v.list_active());
        // Named by its file name alone: found in the library folder.
        v.set_kit_list("BEST-PROWAVE.tsv");
        wait(&mut v);
        assert!(v.list_active());
        assert_eq!(names(&mut v), ["Roland TR-808", "LinnDrum"]);
        assert_eq!(v.results_all(), 2);
        assert_eq!(
            v.index.types,
            [("Hip-hop".to_string(), 1), ("Drum machines".to_string(), 1)]
        );
        // A favourite is always there, first.
        let other = v
            .index
            .kits
            .iter()
            .find(|k| !k.path.contains("/Machines/"))
            .unwrap()
            .clone();
        v.set_favorite(&other.path, true);
        assert_eq!(
            names(&mut v),
            [other.name.as_str(), "Roland TR-808", "LinnDrum"]
        );
        v.set_favorite(&other.path, false);
        // The kit of new racks too (a folder inside it).
        v.set_default_kit(&format!("{}/Kicks", other.path));
        assert_eq!(names(&mut v).len(), 3);
        v.set_default_kit("");
        // One type.
        v.filter.kind = Some("Drum machines".into());
        assert_eq!(names(&mut v), ["LinnDrum"]);
        v.filter.kind = None;
        // Off: every kit again, and it stays off.
        v.set_list_on(false);
        assert!(!v.list_active());
        assert_eq!(names(&mut v).len(), 3);
        let saved = Settings::load(&data);
        assert_eq!(saved.kit_list, "BEST-PROWAVE.tsv");
        assert!(!saved.kit_list_on);
        v.set_list_on(true);
        assert_eq!(names(&mut v).len(), 2);
        // A rescan keeps it.
        v.rescan();
        wait(&mut v);
        assert_eq!(names(&mut v).len(), 2);
        // A list that is not there: every kit, and why.
        v.set_kit_list("/nowhere/NOPE.tsv");
        wait(&mut v);
        assert!(!v.list_active());
        assert!(matches!(v.list, Some(Err(_))));
        assert_eq!(names(&mut v).len(), 3);
        // Cleared: as before.
        v.set_kit_list("");
        wait(&mut v);
        assert!(v.list.is_none() && v.index.types.is_empty());
        assert_eq!(names(&mut v).len(), 3);
        let _ = std::fs::remove_dir_all(lib);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn an_audition_is_the_sound_and_then_silence_to_stop_in() {
        let lib = library("hear");
        let kick = lib.join("Machines/LinnDrum/kick.wav");
        let (buf, end) = sample_preview(&kick).unwrap();
        let sound = buf.frames() - (TAIL_SECONDS * 44_100.0) as usize;
        assert!((5290..=5294).contains(&sound), "{sound}");
        assert!((end - sound as f32 / buf.frames() as f32).abs() < 1e-6);
        assert!(buf.data[sound * 2..].iter().all(|s| *s == 0.0));
        assert!(buf.data[..sound * 2].iter().any(|s| s.abs() > 0.1));
        // A kit: its hits one after the other.
        let paths = vec![
            kick.clone(),
            lib.join("nothing.wav"),
            lib.join("Machines/LinnDrum/sd.wav"),
        ];
        let (mix, end) = kit_preview(&paths).unwrap();
        let step = (KIT_STEP_SECONDS * 44_100.0) as usize;
        // The file that cannot be read keeps its place, silent.
        assert!(mix.data[(step + 6000) * 2..(2 * step) * 2]
            .iter()
            .all(|s| *s == 0.0));
        assert!(mix.data[(2 * step + 200) * 2..(2 * step + 2000) * 2]
            .iter()
            .any(|s| s.abs() > 0.1));
        assert!(end > 0.3 && end < 1.0);
        assert!(kit_preview(&[]).is_err());
        // A long file: only its head is read for a preview.
        let long = lib.join("long.wav");
        write_wav(&long, 12.0, 220.0);
        let head = decode_head(&long, 1.0).unwrap();
        assert!(
            (44_100..44_100 * 3).contains(&head.frames()),
            "{}",
            head.frames()
        );
        assert_eq!(
            decode_head(&kick, 8.0).unwrap().frames(),
            5292,
            "a short one whole"
        );
        // Hits that ring into each other stay under full scale.
        let loud = lib.join("loud.wav");
        write_wav(&loud, 0.8, 60.0);
        let (mix, _) = kit_preview(&[loud.clone(), loud.clone(), loud]).unwrap();
        assert!(mix.data.iter().all(|s| s.abs() <= 0.981));
        // Without an engine nothing is left waiting.
        let mut v = KitBrowser::with(None, Vec::new());
        v.audition("sample:x".into(), vec![kick], false, true);
        for _ in 0..500 {
            pump(&mut v, None);
            if v.want.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(v.audition_key().is_none() && !v.busy());
        let _ = std::fs::remove_dir_all(lib);
    }

    /// The window and the Channel Rack under it, as the app draws them, for a harness that
    /// feeds them pointer and key events. The project and the state are reached through
    /// pointers so a test can look at them between frames.
    fn draw(p: *mut Project, s: *mut crate::UiState) -> impl FnMut(&mut Ui) {
        move |ui: &mut Ui| {
            // SAFETY: the harness calls this on the test's thread, one frame at a time; the
            // project and state outlive the closure and are only read between frames.
            let (p, s) = unsafe { (&mut *p, &mut *s) };
            crate::theme::apply(ui.ctx());
            let rack = Rect::from_min_size(pos2(0.0, 820.0), vec2(1400.0, 720.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(rack), |ui| {
                channel_rack::show(ui, p, s);
            });
            show(ui.ctx(), p, s);
        }
    }

    fn harness() -> crate::ui_test::Harness {
        crate::ui_test::Harness::new(1400.0, 1560.0)
    }

    /// Where the widget `id` was drawn the last frame.
    fn at(h: &crate::ui_test::Harness, id: Id) -> Rect {
        h.ctx
            .read_response(id)
            .unwrap_or_else(|| panic!("{id:?} was not drawn"))
            .rect
    }

    /// Frames until `done`.
    fn frames_until(
        h: &mut crate::ui_test::Harness,
        ui: &mut impl FnMut(&mut Ui),
        done: impl Fn() -> bool,
    ) {
        for _ in 0..3000 {
            h.frame(vec![], &mut *ui);
            if done() {
                h.settle(&mut *ui);
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the background work did not finish");
    }

    fn open_browser(name: &str) -> (PathBuf, PathBuf, Box<Project>, Box<crate::UiState>) {
        let lib = library(name);
        let data = temp(&format!("{name}-data"));
        let mut s = Box::new(crate::UiState::new());
        s.kits = KitBrowser::with(Some(data.clone()), vec![lib.to_string_lossy().into_owned()]);
        s.kits.open = true;
        s.kits.rescan();
        wait(&mut s.kits);
        (lib, data, Box::new(Project::empty()), s)
    }

    fn key_event(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn the_arrows_move_through_kits_and_samples_and_return_loads_and_assigns() {
        let (lib, data, mut p, mut s) = open_browser("keys");
        let (pp, sp): (*mut Project, *mut crate::UiState) = (&mut *p, &mut *s);
        // SAFETY (every `unsafe` below): read between frames on this thread.
        let st = || unsafe { &*sp };
        let mut ui = draw(pp, sp);
        let mut h = harness();
        h.settle(&mut ui);
        let kit_read = || st().kits.kit_rx.is_none();
        // The window has the keys when it opens: Down picks the first kit, then the next.
        h.key(egui::Key::ArrowDown, &mut ui);
        frames_until(&mut h, &mut ui, kit_read);
        assert_eq!(st().kits.selected_name(), "LinnDrum");
        h.key(egui::Key::ArrowDown, &mut ui);
        frames_until(&mut h, &mut ui, kit_read);
        assert_eq!(st().kits.selected_name(), "Roland TR-808");
        assert_eq!(st().kits.kit().unwrap().samples.len(), 18);
        h.key(egui::Key::ArrowUp, &mut ui);
        frames_until(&mut h, &mut ui, kit_read);
        assert_eq!(st().kits.selected_name(), "LinnDrum");
        // Right: into the samples (chh, kick, sd), the first picked; Down, the next ones.
        h.key(egui::Key::ArrowRight, &mut ui);
        assert_eq!(
            (st().kits.column, st().kits.sample),
            (Column::Samples, Some(0))
        );
        h.key(egui::Key::ArrowDown, &mut ui);
        h.key(egui::Key::ArrowDown, &mut ui);
        h.key(egui::Key::ArrowDown, &mut ui);
        assert_eq!(st().kits.sample, Some(2), "the last of three stays picked");
        // Return puts the sample on the lit pad (the first): a new rack.
        h.key(egui::Key::Enter, &mut ui);
        let rack = || {
            let p = unsafe { &*pp };
            let ti = channel_rack::rack_track(p).unwrap();
            let slot = p.tracks[ti].au_instrument.as_ref().unwrap();
            (
                channel_rack::kit_of(&slot.state),
                channel_rack::decode(&slot.state),
            )
        };
        assert_eq!(rack().1[0].name, "sd");
        // Left: back in the kits; Return loads the kit picked.
        h.key(egui::Key::ArrowLeft, &mut ui);
        h.key(egui::Key::Enter, &mut ui);
        let (kit, channels) = rack();
        let names: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["kick", "sd", "", "chh"]);
        assert_eq!(kit, "LinnDrum");
        // The arrows were the window's: nothing drawn after it saw them.
        let mut seen = false;
        h.frame(vec![key_event(egui::Key::ArrowDown)], |u| {
            ui(u);
            seen = u.input(|i| i.key_pressed(egui::Key::ArrowDown));
        });
        assert!(!seen);
        // A click outside the window: the keys are the arrange area's again.
        h.click(pos2(1380.0, 1550.0), &mut ui);
        assert!(!st().kits.keys);
        let mut seen = false;
        h.frame(vec![key_event(egui::Key::ArrowDown)], |u| {
            ui(u);
            seen = u.input(|i| i.key_pressed(egui::Key::ArrowDown));
        });
        assert!(seen);
        // A click in the window: its keys again.
        let search = at(&h, window_id().with("folders")).center();
        h.click(search, &mut ui);
        h.click(search, &mut ui);
        assert!(st().kits.keys);
        drop(ui);
        let _ = std::fs::remove_dir_all(lib);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn a_sample_dragged_lands_on_a_pad_a_rack_channel_and_add_channel() {
        let (lib, data, mut p, mut s) = open_browser("drag");
        let path = format!("{}/Machines/Roland TR-808", lib.display());
        s.kits.select(&path);
        wait(&mut s.kits);
        load_selected(&mut s.kits, &mut p);
        let kit = s.kits.kit().unwrap().clone();
        let ti = channel_rack::rack_track(&p).unwrap();
        let (pp, sp): (*mut Project, *mut crate::UiState) = (&mut *p, &mut *s);
        let channels = || {
            // SAFETY: read between frames on this thread.
            let p = unsafe { &*pp };
            channel_rack::decode(&p.tracks[ti].au_instrument.as_ref().unwrap().state)
        };
        assert_eq!(channels().len(), 16);
        let mut ui = draw(pp, sp);
        let mut h = harness();
        h.settle(&mut ui);
        let id = window_id();
        let index = |n: &str| kit.samples.iter().position(|x| x.name == n).unwrap();
        // A sample's row: right of its triangle.
        let row = |h: &crate::ui_test::Harness, i: usize| {
            at(h, id.with(("hear", i))).center() + vec2(120.0, 0.0)
        };
        let mut drag = |h: &mut crate::ui_test::Harness, from: Pos2, to: Pos2| {
            h.drag(
                &[from, from + vec2(20.0, 10.0), to - vec2(6.0, 6.0), to],
                egui::PointerButton::Primary,
                &mut ui,
            );
        };
        // Onto pad 5 of the window.
        let (from, to) = (
            row(&h, index("CB")),
            at(&h, id.with(("pad", 4usize))).center(),
        );
        drag(&mut h, from, to);
        assert_eq!(channels()[4].name, "CB");
        // Onto the rack's third channel.
        let (from, to) = (
            row(&h, index("CL")),
            at(&h, Id::unique(("rack_name", 2usize))).center(),
        );
        drag(&mut h, from, to);
        assert_eq!(channels()[2].name, "CL");
        // Onto Add Channel: a seventeenth channel.
        let (from, to) = (
            row(&h, index("CB")),
            at(&h, Id::unique("rack_add")).center(),
        );
        drag(&mut h, from, to);
        let after = channels();
        assert_eq!(after.len(), 17);
        assert_eq!(after[16].name, "CB");
        assert!(after[16].path.ends_with("CB/CB.wav"), "{}", after[16].path);
        let _ = std::fs::remove_dir_all(lib);
        let _ = std::fs::remove_dir_all(data);
    }

    /// Correlation of the left channels of `a` (from frame `at`) and `b`, over `n` frames.
    fn likeness(a: &[f32], at: usize, b: &[f32], n: usize) -> f64 {
        let (mut xy, mut xx, mut yy) = (0f64, 0f64, 0f64);
        for k in 0..n {
            let (Some(x), Some(y)) = (a.get((at + k) * 2), b.get(k * 2)) else {
                break;
            };
            let (x, y) = (f64::from(*x), f64::from(*y));
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        xy / (xx * yy).sqrt().max(1e-12)
    }

    #[test]
    fn audition_lit_plays_each_sample_and_each_kit_as_it_is_picked() {
        let (lib, data, mut p, mut s) = open_browser("audition");
        assert!(!s.kits.settings.audition, "off at first");
        let (pp, sp): (*mut Project, *mut crate::UiState) = (&mut *p, &mut *s);
        // SAFETY (every `unsafe` below): used between frames on this thread.
        let st = || unsafe { &mut *sp };
        let mut ui = draw(pp, sp);
        let mut h = harness();
        h.settle(&mut ui);
        let rate = 44_100usize;
        let kit_read = || st().kits.kit_rx.is_none() && st().kits.kit().is_some();
        // Audition off: picking a kit plays nothing.
        h.key(egui::Key::ArrowDown, &mut ui);
        frames_until(&mut h, &mut ui, kit_read);
        assert!(st().kits.audition_key().is_none());
        // Light it (its key in the window): it is kept, and it plays the kit picked: its kick,
        // snare and hat.
        h.click(at(&h, window_id().with("audition")).center(), &mut ui);
        assert!(st().kits.settings.audition);
        assert!(Settings::load(&data).audition, "kept");
        let linn = lib.join("Machines/LinnDrum");
        assert_eq!(
            st().kits.audition_key(),
            Some(format!("kit:{}", linn.display()).as_str())
        );
        let out = capture(&mut st().kits, 44_100, 5.0, Duration::from_secs(10));
        let step = (KIT_STEP_SECONDS * 44_100.0) as usize;
        let skip = rate / 100;
        for (k, f) in ["kick.wav", "sd.wav", "chh.wav"].iter().enumerate() {
            let src = prowave_engine::sample_tools::decode(&linn.join(f), |_| true).unwrap();
            // Past the voice's fade in, the first 50 ms of each hit are the file.
            let c = likeness(&out, k * step + skip, &src.data[skip * 2..], rate / 20);
            assert!(c > 0.99, "{f}: {c}");
        }
        assert!(out.len() / 2 < rate * 2, "it stops after the hits");
        // Down to the next kit: it is heard as it is picked.
        h.key(egui::Key::ArrowDown, &mut ui);
        frames_until(&mut h, &mut ui, kit_read);
        let tr808 = lib.join("Machines/Roland TR-808");
        assert_eq!(
            st().kits.audition_key(),
            Some(format!("kit:{}", tr808.display()).as_str())
        );
        let out = capture(&mut st().kits, 44_100, 5.0, Duration::from_secs(10));
        let kick =
            prowave_engine::sample_tools::decode(&tr808.join("BD/BD0000.wav"), |_| true).unwrap();
        assert!(likeness(&out, skip, &kick.data[skip * 2..], rate / 20) > 0.99);
        // Into the samples: each one is heard as the arrows reach it.
        h.key(egui::Key::ArrowRight, &mut ui);
        h.key(egui::Key::ArrowDown, &mut ui);
        let kit = st().kits.kit().unwrap().clone();
        let i = st().kits.sample.unwrap();
        assert_eq!(i, 1);
        let file = kit.path_of(i);
        assert_eq!(
            st().kits.audition_key(),
            Some(format!("sample:{}", file.display()).as_str())
        );
        let out = capture(&mut st().kits, 44_100, 5.0, Duration::from_secs(10));
        let src = prowave_engine::sample_tools::decode(&file, |_| true).unwrap();
        let sound = src.frames();
        assert!(
            out.len() / 2 >= sound,
            "the whole sample: {} of {sound}",
            out.len() / 2
        );
        let c = likeness(&out, skip, &src.data[skip * 2..], sound - skip);
        assert!(c > 0.995, "{c}");
        let peak = out.iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.3, "{peak}");
        // Its triangle while it plays stops it.
        st().kits.audition_sample(i, false);
        st().kits.audition_sample(i, true);
        assert!(st().kits.audition_key().is_none());
        drop(ui);
        let _ = std::fs::remove_dir_all(lib);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn a_kit_of_a_hundred_thousand_samples_draws_in_a_frame() {
        let mut s = Box::new(crate::UiState::new());
        s.kits = KitBrowser::with(None, Vec::new());
        s.kits.open = true;
        let listing = crate::kit_files::Listing {
            audio: (0..100_000)
                .map(|i| {
                    let (dir, name) = [
                        ("Kicks", "Kick"),
                        ("Snares", "Snare"),
                        ("Hats", "Hat"),
                        ("Perc", "Conga"),
                    ][i % 4];
                    format!("{dir}/{name} {i}.wav")
                })
                .collect(),
            ..crate::kit_files::Listing::empty()
        };
        s.kits.selected = Some("/lib/Huge".into());
        s.kits.kit = Some(Arc::new(listing.kit(Path::new("/lib/Huge"))));
        let mut p = Box::new(Project::empty());
        let (pp, sp): (*mut Project, *mut crate::UiState) = (&mut *p, &mut *s);
        let mut ui = draw(pp, sp);
        let mut h = harness();
        h.settle(&mut ui);
        h.key(egui::Key::ArrowRight, &mut ui);
        let started = std::time::Instant::now();
        for _ in 0..10 {
            h.key(egui::Key::ArrowDown, &mut ui);
        }
        let per = started.elapsed() / 10;
        drop(ui);
        assert_eq!(s.kits.column, Column::Samples);
        assert_eq!(s.kits.sample, Some(10));
        // Debug build: far above what a frame takes, far below a stall.
        assert!(per < Duration::from_millis(150), "{per:?} a frame");
        eprintln!("a frame with a 100,000-sample kit: {per:?}");
    }

    /// The window with a kit picked, the folders, and the rack with a kit loaded.
    #[test]
    #[ignore = "pictures"]
    fn kit_browser_pictures() {
        let lib = library("pictures");
        let root = lib.to_string_lossy().into_owned();
        std::fs::write(
            lib.join("MANIFEST.tsv"),
            "source\tkit\tlicense\taudio_or_instrument_files\n\
             Machines\tRoland TR-808\tCC BY-NC-ND 4.0\t18\n\
             Machines\tLinnDrum\tCC BY-NC-ND 4.0\t3\n\
             Packs\tDusty Breaks Vol 2\tRoyalty free\t2\n",
        )
        .unwrap();
        std::fs::write(lib.join("BEST.md"), "1. Machines / Roland TR-808\n").unwrap();
        let mut p = Project::empty();
        let mut s = crate::UiState::new();
        s.kits = KitBrowser::with(
            None,
            vec![root.clone(), "/Volumes/NOT-MOUNTED-HERE/DRUM-KITS".into()],
        );
        s.kits.open = true;
        s.kits.rescan();
        wait(&mut s.kits);
        let path = format!("{root}/Machines/Roland TR-808");
        s.kits.select(&path);
        wait(&mut s.kits);
        s.kits.set_favorite(&path, true);
        load_selected(&mut s.kits, &mut p);
        s.kits.pad = 2;
        // Audition lit, the arrows in the samples, the fourth one picked.
        s.kits.settings.audition = true;
        s.kits.column = Column::Samples;
        s.kits.sample = Some(3);
        let dir = std::env::var_os("PROWAVE_UI_PNG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shot = |name: &str, p: &mut Project, s: &mut crate::UiState| {
            crate::ui_test::render_png(
                980.0,
                760.0,
                2.0,
                crate::theme::BONE3(),
                |ui| {
                    crate::theme::apply(ui.ctx());
                    show(ui.ctx(), p, s)
                },
                &dir.join(name),
            )
            .unwrap();
        };
        shot("kit-browser.png", &mut p, &mut s);
        s.kits.show_folders = true;
        shot("kit-browser-folders.png", &mut p, &mut s);
        s.kits.show_folders = false;
        s.kits.filter.query = "linn".into();
        shot("kit-browser-search.png", &mut p, &mut s);
        crate::ui_test::render_png(
            1200.0,
            640.0,
            2.0,
            crate::theme::BONE(),
            |ui| {
                crate::theme::apply(ui.ctx());
                channel_rack::show(ui, &mut p, &mut s)
            },
            &dir.join("kit-browser-rack.png"),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(lib);
    }
}
