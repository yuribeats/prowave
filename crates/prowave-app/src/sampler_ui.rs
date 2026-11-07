//! The Sampler's window, an OS window of its own (an egui viewport per sampler): the file
//! browser on the left, the sample editor in the middle, and on the right the front panel of
//! the Boss Dr. Sample SP-303, laid out and coloured after the hardware (photos and the
//! manual's panel drawing, p. 7; see `docs/SP303-NOTES.md` §10).
//!
//! The panel works as the SP-303's does where it can (page references by each key); what the
//! hardware has no equivalent for (the browser, chopping, Stretch, banks E–H) is outside it,
//! in the editor, marked as an extension.
use crate::chop_set::{self, PadSet, StemFile};
use crate::i18n::{tr, trf};
use crate::model::Project;
use crate::sampler::{
    self, pad_label, Pad, PitchMode, Quantize, SamplerState, TimeMode, HARDWARE_BANKS, PADS,
    PADS_PER_BANK,
};
use egui::{pos2, vec2, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Ui};
use prowave_engine::sample_tools::{self, Grade};
use prowave_engine::{SampleBuf, SamplerMsg, SpFxKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// The hardware's colours (sampled from photos, `docs/SP303-NOTES.md` §10).
const BEZEL: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1c);
const PLATE: Color32 = Color32::from_rgb(0xd4, 0xd2, 0xcc);
const BLOCK: Color32 = Color32::from_rgb(0x1e, 0x1f, 0x28);
const RED: Color32 = Color32::from_rgb(0xc8, 0x32, 0x3c);
const KEY: Color32 = Color32::from_rgb(0xbd, 0xba, 0xa4);
const PAD: Color32 = Color32::from_rgb(0xc4, 0xc1, 0xae);
const EXT: Color32 = Color32::from_rgb(0x2a, 0x2c, 0x2d);
const LIT: Color32 = Color32::from_rgb(0xff, 0x2a, 0x3a);
const LED_OFF: Color32 = Color32::from_rgb(0x3a, 0x16, 0x18);
const DISPLAY: Color32 = Color32::from_rgb(0x1a, 0x14, 0x14);
const CAP: Color32 = Color32::from_rgb(0xef, 0xe9, 0xdc);
const INK: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
// The editor and browser follow Pro Wave's dark grey.
const WAVE_BG: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x22);
const WAVE: Color32 = Color32::from_rgb(0x8f, 0xd0, 0x8a);
const CHOP: Color32 = Color32::from_rgb(0xf0, 0xc8, 0x40);
const RANGE: Color32 = Color32::from_rgba_premultiplied(0x30, 0x48, 0x70, 0x70);
const SELECT: Color32 = Color32::from_rgba_premultiplied(0x50, 0x50, 0x20, 0x60);

const PANEL_W: f32 = 404.0;
const PANEL_H: f32 = 640.0;
const BROWSER_W: f32 = 330.0;

/// Which keys the three CTRL knobs answer to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum KnobMode {
    #[default]
    Effect,
    /// START/END/LEVEL (manual p. 38).
    StartEndLevel,
    /// TIME/BPM (manual pp. 41–43).
    TimeBpm,
    /// LENGTH and QUANTIZE (CTRL 3; manual p. 45).
    Length,
    Quantize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ChopMode {
    #[default]
    Manual,
    Transients,
    Equal,
    Beats,
}

/// One sampler window's state (not saved with the project).
pub struct View {
    current: usize,
    bank: usize,
    knobs: KnobMode,
    pattern_select: bool,
    pattern: usize,
    pattern_bank: usize,
    /// The pattern being recorded: its region.
    rec: Option<usize>,
    remain: bool,
    hold: bool,
    latched: Vec<usize>,
    /// A held pad pressed to stop it: its release is not a new hold.
    unlatch: Option<usize>,
    del: bool,
    del_target: Option<usize>,
    mark: u8,
    mark_start: u64,
    taps: Vec<f64>,
    /// What the display shows for a moment, and until when.
    flash: Option<(String, f64)>,
    /// A relative START/END knob move: the pad's start and end when it began.
    nudge: Option<(u64, u64)>,
    /// Where the START and END knobs are during such a move (0.5: centre).
    nudge_knob: [f32; 2],
    held: Option<usize>,
    keys_down: Vec<(Key, usize)>,
    // Editor.
    view: (f64, f64),
    view_path: String,
    selection: Option<(u64, u64)>,
    drag: Option<Drag>,
    pub chop: ChopMode,
    sensitivity: f32,
    divisions: usize,
    beats: f64,
    snap: bool,
    status: Option<String>,
    peaks: HashMap<String, Arc<Vec<(f32, f32)>>>,
    /// The preview playing: the file, the frame scale of its buffer (sync), and its buffer.
    preview: Option<Preview>,
    /// Where the panel was drawn last.
    panel: Rect,
    /// A pad set picked while the song plays: it goes on the pads at the next bar.
    pending: Option<Pending>,
}

/// A pad set waiting for its bar line.
#[derive(Clone)]
struct Pending {
    set: Arc<PadSet>,
    song: String,
    /// The bar line (beats), and where the playhead was last seen.
    at: f64,
    last: f64,
}

#[derive(Clone)]
struct Preview {
    path: String,
    /// Frames of the played buffer per frame of the file (1, or the sync stretch).
    scale: f64,
    /// The head of the file is playing; the whole file takes over when it is decoded.
    head: bool,
    synced: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Drag {
    Start,
    End,
    Chop(usize),
    Select(u64),
}

impl Default for View {
    fn default() -> Self {
        Self {
            current: 0,
            bank: 0,
            knobs: KnobMode::Effect,
            pattern_select: false,
            pattern: 0,
            pattern_bank: 0,
            rec: None,
            remain: false,
            hold: false,
            latched: Vec::new(),
            unlatch: None,
            del: false,
            del_target: None,
            mark: 0,
            mark_start: 0,
            taps: Vec::new(),
            flash: None,
            nudge: None,
            nudge_knob: [0.5; 2],
            held: None,
            keys_down: Vec::new(),
            view: (0.0, 0.0),
            view_path: String::new(),
            selection: None,
            drag: None,
            chop: ChopMode::Manual,
            sensitivity: 0.5,
            divisions: 8,
            beats: 1.0,
            snap: true,
            status: None,
            peaks: HashMap::new(),
            preview: None,
            panel: Rect::NOTHING,
            pending: None,
        }
    }
}

/// The file browser (one for all sampler windows).
#[derive(Default)]
pub struct Browser {
    pub folder: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    pub filter: String,
    pub selected: Option<PathBuf>,
    pub sync: bool,
    scanned: bool,
    /// Where pad sets and stems are looked for (`None` until first asked: the app's folders).
    roots: Option<(Option<PathBuf>, Option<PathBuf>)>,
    /// Each song's pad set (`None`: chops not ready), read once.
    sets: HashMap<PathBuf, Option<Arc<PadSet>>>,
    stems: HashMap<PathBuf, Arc<Vec<StemFile>>>,
    /// The rows drawn last, for the tests.
    #[cfg(test)]
    rows: Vec<(PathBuf, Rect)>,
}

impl Browser {
    pub fn set_folder(&mut self, dir: &Path) {
        self.folder = Some(dir.to_path_buf());
        self.files = sampler::scan(dir);
        self.scanned = true;
        self.sets.clear();
        self.stems.clear();
    }

    fn roots(&mut self) -> (Option<PathBuf>, Option<PathBuf>) {
        self.roots
            .get_or_insert_with(|| (chop_set::chops_root(), chop_set::stems_root()))
            .clone()
    }

    /// The pad set made for `song`, if there is one.
    fn set_of(&mut self, song: &Path) -> Option<Arc<PadSet>> {
        if let Some(s) = self.sets.get(song) {
            return s.clone();
        }
        let set = self
            .roots()
            .0
            .and_then(|root| chop_set::find(&root, song))
            .map(Arc::new);
        self.sets.insert(song.to_path_buf(), set.clone());
        set
    }

    /// The song's stems, if it was separated.
    fn stems_of(&mut self, song: &Path) -> Arc<Vec<StemFile>> {
        if let Some(s) = self.stems.get(song) {
            return s.clone();
        }
        let list = Arc::new(
            self.roots()
                .1
                .map(|root| chop_set::stems_of(&root, song))
                .unwrap_or_default(),
        );
        self.stems.insert(song.to_path_buf(), list.clone());
        list
    }

    fn visible(&self) -> Vec<&PathBuf> {
        let f = self.filter.to_lowercase();
        self.files
            .iter()
            .filter(|p| f.is_empty() || p.to_string_lossy().to_lowercase().contains(&f))
            .collect()
    }
}

/// The open sampler windows.
#[derive(Default)]
pub struct SamplerWindows {
    /// The slots (uids) whose windows are open.
    pub open: Vec<u64>,
    views: HashMap<u64, View>,
    pub browser: Browser,
}

impl SamplerWindows {
    pub fn open(&mut self, uid: u64) {
        if !self.open.contains(&uid) {
            self.open.push(uid);
        }
    }
}

// ------------------------------------------------------------------ engine helpers

fn send(uid: u64, m: SamplerMsg) -> bool {
    sampler::with_runtime(|rt| rt.remote(uid).is_some_and(|r| r.send(m).is_ok()))
}

fn running(uid: u64) -> bool {
    sampler::with_runtime(|rt| rt.remote(uid).is_some())
}

fn now(ui: &Ui) -> f64 {
    ui.input(|i| i.time)
}

impl View {
    fn flash(&mut self, text: impl Into<String>, t: f64) {
        self.flash = Some((text.into(), t + 1.2));
    }
}

/// The stretch that puts `path` in time with the project, when its tempo is known and differs.
fn sync_spec(b: &Browser, p: &Project, path: &str) -> Option<sampler::RenderSpec> {
    if !b.sync {
        return None;
    }
    let bpm = sampler::project_bpm(p);
    let fb = sampler::with_runtime(|rt| rt.info.get(Path::new(path)).and_then(|i| i.bpm))?;
    if (fb - bpm).abs() <= 0.05 {
        return None;
    }
    let factor = fb / bpm;
    Some(sampler::RenderSpec {
        path: path.to_string(),
        edit: sample_tools::SliceEdit::default(),
        stretch: Some(((factor * 1e6).round() / 1e6, 0.0)),
    })
}

/// Starts the preview of `path` from frame `from` of the file (decoding it if needed).
fn preview(uid: u64, v: &mut View, b: &Browser, p: &Project, path: &str, from: u64) {
    let sync = sync_spec(b, p, path);
    let (buf, head, synced, scale) = sampler::with_runtime(|rt| {
        let full = rt.source(path);
        let Some(full) = full else {
            return (rt.heads.get(path).cloned(), true, false, 1.0);
        };
        // In sync: the file stretched to the project tempo, once that is ready.
        if let Some(spec) = &sync {
            if let Some(s) = rt.prepared(spec, full.clone()) {
                let factor = spec.stretch.map_or(1.0, |x| x.0);
                return (Some(s), false, true, factor);
            }
        }
        (Some(full), false, false, 1.0)
    });
    let Some(buf) = buf else {
        v.preview = Some(Preview {
            path: path.to_string(),
            scale: 1.0,
            head: true,
            synced: false,
        });
        return;
    };
    let from = ((from as f64) * scale) as u64;
    if head && from >= buf.frames() as u64 {
        return;
    }
    let to = buf.frames() as u64;
    send(
        uid,
        SamplerMsg::Preview {
            buf,
            from,
            to,
            looping: false,
            gain: 1.0,
        },
    );
    v.preview = Some(Preview {
        path: path.to_string(),
        scale,
        head,
        synced,
    });
}

fn stop_preview(uid: u64, v: &mut View) {
    send(uid, SamplerMsg::StopPreview);
    v.preview = None;
}

/// Where the preview is, in frames of the file.
fn preview_frame(uid: u64, v: &View) -> Option<u64> {
    let pv = v.preview.as_ref()?;
    let at = sampler::with_runtime(|rt| rt.preview_at(uid))?;
    Some((at as f64 / pv.scale) as u64)
}

/// Plays frames `from..to` of the decoded `path` (the editor's audition).
fn audition(uid: u64, v: &mut View, path: &str, from: u64, to: u64) {
    let Some(buf) = sampler::with_runtime(|rt| rt.source(path)) else {
        return;
    };
    send(
        uid,
        SamplerMsg::Preview {
            buf,
            from,
            to,
            looping: false,
            gain: 1.0,
        },
    );
    v.preview = Some(Preview {
        path: path.to_string(),
        scale: 1.0,
        head: false,
        synced: false,
    });
}

/// A preview that started on a file's head carries on in the whole file once it is decoded;
/// a synced preview moves to the stretched sound when it is ready.
fn follow_preview(uid: u64, v: &mut View, b: &Browser, p: &Project) {
    let Some(pv) = v.preview.clone() else { return };
    let playing = sampler::with_runtime(|rt| rt.preview_at(uid));
    let ready = sampler::with_runtime(|rt| {
        matches!(rt.decoded.get(&pv.path), Some(sampler::Decoded::Ready(_)))
    });
    if pv.head && ready {
        let at = playing.map_or(0, |f| (f as f64 / pv.scale) as u64);
        preview(uid, v, b, p, &pv.path, at);
    } else if pv.head && playing.is_none() {
        // Nothing playing yet: the head may have arrived.
        let head = sampler::with_runtime(|rt| rt.heads.contains_key(&pv.path));
        if head {
            preview(uid, v, b, p, &pv.path, 0);
        }
    } else if b.sync && !pv.synced && !pv.head && playing.is_some() {
        if sync_spec(b, p, &pv.path)
            .is_some_and(|spec| sampler::with_runtime(|rt| rt.has_prepared(&spec)))
        {
            let at = playing.map_or(0, |f| (f as f64 / pv.scale) as u64);
            preview(uid, v, b, p, &pv.path, at);
        }
    } else if playing.is_none() && !pv.head {
        v.preview = None;
    }
}

// ------------------------------------------------------------------ the window

/// Every open sampler window.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    let w = &mut s.sampler;
    w.open.retain(|uid| sampler::track_of(p, *uid).is_some());
    let open = w.open.clone();
    for uid in open {
        let Some(t) = sampler::track_of(p, uid) else {
            continue;
        };
        let title = format!("Sampler — {}", p.tracks[t].name);
        let id = egui::ViewportId::from_hash_of(("prowave-sampler", uid));
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([BROWSER_W + 560.0 + PANEL_W + 40.0, PANEL_H + 24.0])
            .with_min_inner_size([900.0, PANEL_H + 24.0]);
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ui, _class| {
            if ui.input(|i| i.viewport().close_requested()) {
                close = true;
            }
            let v = w.views.entry(uid).or_default();
            if !w.browser.scanned {
                w.browser.scanned = true;
                if let Some(dir) = sampler::default_folder() {
                    w.browser.set_folder(&dir);
                }
            }
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(Color32::from_gray(0x2a))
                        .inner_margin(8.0),
                )
                .show(ui, |ui| window(ui, p, uid, v, &mut w.browser));
        });
        if close {
            w.open.retain(|u| *u != uid);
            send(uid, SamplerMsg::StopPreview);
        }
    }
}

/// The inside of one sampler window.
pub fn window(ui: &mut Ui, p: &mut Project, uid: u64, v: &mut View, b: &mut Browser) {
    let Some(t) = sampler::track_of(p, uid) else {
        return;
    };
    let before = sampler::state_of(p, uid).unwrap_or_default();
    let mut st = before.clone();
    follow_preview(uid, v, b, p);
    let shown = editor_path(&st, v, b);
    sampler::with_runtime(|rt| {
        shown.iter().for_each(|x| rt.pin(x));
        if let Some(pv) = &v.preview {
            rt.pin(&pv.path);
        }
    });
    if sampler::with_runtime(|rt| rt.busy()) || v.preview.is_some() || p.playing {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(30));
    }
    keys(ui, p, uid, t, &mut st, v, b);
    pending_set(p, &mut st, v);
    // Three columns: the browser at the left, the panel at the right, the editor between
    // (each kept inside its own rectangle).
    let outer = ui.available_rect_before_wrap();
    let height = PANEL_H.max(outer.height());
    let panel_rect = Rect::from_min_size(
        pos2(
            (outer.right() - PANEL_W).max(outer.left() + BROWSER_W + 310.0),
            outer.top(),
        ),
        vec2(PANEL_W, PANEL_H),
    );
    let browser_rect = Rect::from_min_size(outer.min, vec2(BROWSER_W, height));
    let mid_rect = Rect::from_min_max(
        pos2(browser_rect.right() + 10.0, outer.top()),
        pos2(panel_rect.left() - 10.0, outer.top() + height),
    );
    let down = egui::Layout::top_down(egui::Align::Min);
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(browser_rect).layout(down),
        |ui| {
            ui.set_clip_rect(browser_rect);
            browser(ui, p, uid, &mut st, v, b);
        },
    );
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(mid_rect).layout(down),
        |ui| {
            ui.set_clip_rect(mid_rect);
            editor(ui, p, uid, t, &mut st, v, b);
        },
    );
    v.panel = panel_rect;
    panel(ui, panel_rect, p, uid, t, &mut st, v, b);
    ui.allocate_rect(browser_rect.union(panel_rect), Sense::hover());
    if st != before {
        sampler::store(p, uid, &st);
    }
}

/// Puts the song's pad set on the pads: at once when the song is stopped, else at the next bar
/// line (its files are read in the meantime, so the swap is ready when the bar comes).
fn load_set(p: &Project, st: &mut SamplerState, v: &mut View, song: &Path, set: Arc<PadSet>) {
    let name = song
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if p.playing {
        sampler::with_runtime(|rt| {
            for pad in &set.pads {
                rt.source(&pad.file);
                rt.pin(&pad.file);
            }
        });
        let now = p.position_beats;
        v.pending = Some(Pending {
            at: chop_set::next_bar(now, sampler::beats_per_bar(p)),
            last: now,
            set,
            song: name.clone(),
        });
        v.status = Some(trf(
            "The chops of {} go on the pads at the next bar.",
            &[&name],
        ));
        return;
    }
    v.pending = None;
    let n = chop_set::apply(&set, st);
    v.current = 0;
    v.bank = 0;
    v.status = Some(trf("{} pads loaded from the chops of {}", &[&n, &name]));
}

/// The pads as they are now become the song's pad set (replacing the one made for it).
fn save_set(b: &mut Browser, st: &SamplerState, song: &Path) -> String {
    let name = song
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let Some(path) = b.roots().0.and_then(|root| chop_set::set_path(&root, song)) else {
        return tr("No folder for chops.").to_string();
    };
    match chop_set::write(&chop_set::from_state(st, &name), &path) {
        Ok(()) => {
            b.sets.remove(song);
            trf("The pads are saved as the chops of {}.", &[&name])
        }
        Err(e) => trf("cannot save {}: {}", &[&path.display(), &e]),
    }
}

/// A waiting pad set goes on the pads when its bar line comes.
fn pending_set(p: &Project, st: &mut SamplerState, v: &mut View) {
    let Some(w) = v.pending.as_mut() else { return };
    let now = p.position_beats;
    if !chop_set::due(w.at, w.last, now, p.playing) {
        w.last = now;
        // The files keep being read while it waits.
        sampler::with_runtime(|rt| w.set.pads.iter().for_each(|x| rt.pin(&x.file)));
        return;
    }
    let Some(w) = v.pending.take() else { return };
    let n = chop_set::apply(&w.set, st);
    v.current = 0;
    v.bank = 0;
    v.status = Some(trf("{} pads loaded from the chops of {}", &[&n, &w.song]));
}

/// The computer keyboard while the window has focus: 1–8 play the bank's pads (the SP-303's
/// pad numbers), the arrows move through the browser, Space or Enter start and stop the
/// preview.
fn keys(
    ui: &mut Ui,
    p: &mut Project,
    uid: u64,
    t: usize,
    st: &mut SamplerState,
    v: &mut View,
    b: &mut Browser,
) {
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    const PAD_KEYS: [Key; 8] = [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
    ];
    let (pressed, released, up, down, toggle) = ui.input(|i| {
        let pressed: Vec<Key> = PAD_KEYS
            .iter()
            .copied()
            .filter(|k| i.key_pressed(*k))
            .collect();
        let released: Vec<Key> = PAD_KEYS
            .iter()
            .copied()
            .filter(|k| i.key_released(*k))
            .collect();
        (
            pressed,
            released,
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::Space) || i.key_pressed(Key::Enter),
        )
    });
    for k in pressed {
        if v.keys_down.iter().any(|(x, _)| *x == k) {
            continue;
        }
        let n = PAD_KEYS.iter().position(|x| *x == k).unwrap_or(0);
        let pad = v.bank * PADS_PER_BANK + n;
        pad_down(ui, p, uid, t, st, v, pad);
        v.keys_down.push((k, pad));
    }
    for k in released {
        if let Some(i) = v.keys_down.iter().position(|(x, _)| *x == k) {
            let (_, pad) = v.keys_down.remove(i);
            pad_up(uid, st, v, pad);
        }
    }
    if up || down {
        let list: Vec<PathBuf> = b.visible().into_iter().cloned().collect();
        if !list.is_empty() {
            let at = b
                .selected
                .as_ref()
                .and_then(|s| list.iter().position(|x| x == s));
            let next = match (at, down) {
                (None, _) => 0,
                (Some(i), true) => (i + 1).min(list.len() - 1),
                (Some(i), false) => i.saturating_sub(1),
            };
            b.selected = Some(list[next].clone());
            if v.preview.is_some() {
                let path = list[next].to_string_lossy().into_owned();
                preview(uid, v, b, p, &path, 0);
            }
        }
    }
    if toggle {
        if v.preview.is_some() {
            stop_preview(uid, v);
        } else if let Some(sel) = b.selected.clone() {
            preview(uid, v, b, p, &sel.to_string_lossy(), 0);
        }
    }
}

/// A pad pressed (on screen, by key).
fn pad_down(
    ui: &Ui,
    p: &mut Project,
    uid: u64,
    t: usize,
    st: &mut SamplerState,
    v: &mut View,
    k: usize,
) {
    let time = now(ui);
    v.held = Some(k);
    if v.pattern_select && k < HARDWARE_BANKS * PADS_PER_BANK {
        // Pads choose patterns (manual p. 44); a pattern plays at once, in a cycle.
        let n = v.pattern_bank * PADS_PER_BANK + k % PADS_PER_BANK;
        v.pattern = n;
        let r = sampler::ensure_pattern(p, t, n, st.pattern_len);
        let region = &p.tracks[t].regions[r];
        p.cycle_start = p.region_start_beats(region);
        p.cycle_end = p.region_end_beats(region);
        p.cycle = true;
        p.position_beats = p.cycle_start;
        p.playing = true;
        v.flash(pad_label(n), time);
        return;
    }
    if v.del {
        if v.rec.is_some() {
            // Erase while recording: the pad's notes go (manual p. 46).
            if let (Some(r), Some(note)) = (v.rec, sampler::note_for_pad(st, k)) {
                if let Some(region) = p.tracks[t].regions.get_mut(r) {
                    region.notes.retain(|n| n.pitch != note);
                    p.song_rev += 1;
                }
            }
            v.flash("ErS", time);
        } else {
            v.del_target = Some(k);
            v.current = k;
            v.flash(pad_label(k), time);
        }
        return;
    }
    if v.remain {
        // REMAIN + pad: the effect on or off for it (manual p. 22).
        st.pads[k].fx = !st.pads[k].fx;
        v.flash(if st.pads[k].fx { "on" } else { "oFF" }, time);
        return;
    }
    v.current = k;
    v.held = Some(k);
    if st.pads[k].is_empty() {
        return;
    }
    if v.hold && v.latched.contains(&k) {
        // A held pad pressed again stops (manual p. 19).
        v.latched.retain(|x| *x != k);
        v.unlatch = Some(k);
        send(uid, SamplerMsg::Release { pad: k as u8 });
        return;
    }
    send(
        uid,
        SamplerMsg::Hit {
            pad: k as u8,
            vel: 127,
        },
    );
    if let Some(r) = v.rec {
        if p.playing {
            if let Some(note) = sampler::note_for_pad(st, k) {
                sampler::record_hit(p, t, r, note, st.quantize);
            }
        }
    }
}

fn pad_up(uid: u64, st: &SamplerState, v: &mut View, k: usize) {
    v.held = None;
    if v.unlatch == Some(k) {
        v.unlatch = None;
        return;
    }
    if v.hold && st.pads.get(k).is_some_and(|p| p.gate) {
        if !v.latched.contains(&k) {
            v.latched.push(k);
        }
        return;
    }
    send(uid, SamplerMsg::Release { pad: k as u8 });
}

// ------------------------------------------------------------------ the browser

fn browser(
    ui: &mut Ui,
    p: &mut Project,
    uid: u64,
    st: &mut SamplerState,
    v: &mut View,
    b: &mut Browser,
) {
    ui.horizontal(|ui| {
        if ui.button(tr("Choose Folder…")).clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                sampler::remember_folder(&dir);
                b.set_folder(&dir);
            }
        }
        if b.folder.is_some() && ui.button(tr("Rescan")).clicked() {
            if let Some(dir) = b.folder.clone() {
                b.set_folder(&dir);
            }
        }
    });
    if let Some(dir) = &b.folder {
        ui.label(
            egui::RichText::new(dir.file_name().unwrap_or_default().to_string_lossy())
                .color(Color32::from_gray(0xe0))
                .strong(),
        )
        .on_hover_text(dir.display().to_string());
    } else {
        ui.label(tr("No folder chosen."));
    }
    ui.horizontal(|ui| {
        ui.label(tr("Search"));
        ui.add(egui::TextEdit::singleline(&mut b.filter).desired_width(f32::INFINITY));
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut b.sync, tr("Preview in sync"))
            .on_hover_text(tr(
                "Stretch the preview to the project tempo when the file's tempo is known.",
            ));
        if v.preview.is_some() && ui.button(tr("Stop")).clicked() {
            stop_preview(uid, v);
        }
    });
    let list: Vec<PathBuf> = b.visible().into_iter().cloned().collect();
    ui.label(
        egui::RichText::new(trf("{} files", &[&list.len()]))
            .size(11.0)
            .color(Color32::from_gray(0xb0)),
    );
    // The selected song's stems are listed under it.
    let stems = b
        .selected
        .clone()
        .filter(|s| list.contains(s))
        .map(|s| (s.clone(), b.stems_of(&s)));
    let mut rows: Vec<(PathBuf, Option<StemFile>)> = Vec::with_capacity(list.len());
    for path in &list {
        rows.push((path.clone(), None));
        if let Some((song, stems)) = &stems {
            if song == path {
                rows.extend(stems.iter().map(|x| (path.clone(), Some(x.clone()))));
            }
        }
    }
    let row_h = 34.0;
    let previewing = v.preview.as_ref().map(|x| x.path.clone());
    let mut clicked: Option<PathBuf> = None;
    let mut clicked_stem: Option<PathBuf> = None;
    let mut load: Option<PathBuf> = None;
    let mut load_chops: Option<PathBuf> = None;
    let mut save_chops: Option<PathBuf> = None;
    #[cfg(test)]
    b.rows.clear();
    egui::ScrollArea::vertical()
        .id_salt("sampler-files")
        .max_height(ui.available_height() - 4.0)
        .show_rows(ui, row_h, rows.len(), |ui, range| {
            for i in range {
                let (song, stem) = &rows[i];
                let path = stem.as_ref().map_or(song, |x| &x.path);
                let key = path.to_string_lossy().into_owned();
                let set = if stem.is_none() { b.set_of(song) } else { None };
                let info = sampler::with_runtime(|rt| {
                    rt.analyse(path);
                    rt.info.get(path).cloned()
                });
                let selected = stem.is_none() && b.selected.as_ref() == Some(path);
                let playing = previewing.as_deref() == Some(key.as_str());
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                #[cfg(test)]
                b.rows.push((path.clone(), rect));
                let g = ui.painter();
                let bg = if selected {
                    Color32::from_rgb(0x2f, 0x5a, 0x9e)
                } else if stem.is_some() {
                    Color32::from_rgb(0x26, 0x30, 0x40)
                } else if i % 2 == 0 {
                    Color32::from_gray(0x33)
                } else {
                    Color32::from_gray(0x2d)
                };
                g.rect_filled(rect, 0.0, bg);
                let indent = if stem.is_some() { 18.0 } else { 0.0 };
                let name = match stem {
                    Some(x) => x.name.clone(),
                    None => path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                };
                let font = FontId::proportional(12.0);
                let fitted = elide(ui, &name, &font, rect.width() - 30.0 - indent);
                g.text(
                    rect.left_top() + vec2(22.0 + indent, 3.0),
                    Align2::LEFT_TOP,
                    fitted,
                    font,
                    Color32::WHITE,
                );
                if playing {
                    g.text(
                        rect.left_top() + vec2(5.0 + indent, 3.0),
                        Align2::LEFT_TOP,
                        "▶",
                        FontId::proportional(12.0),
                        Color32::from_rgb(0x9f, 0xe8, 0x8f),
                    );
                }
                let mut detail = match &info {
                    Some(i) if i.failed => tr("cannot be read").to_string(),
                    Some(i) => {
                        let mut parts = Vec::new();
                        if let Some(s) = i.seconds {
                            parts.push(format!("{}:{:02}", (s / 60.0) as u32, (s % 60.0) as u32));
                        }
                        if let Some(bpm) = i.bpm {
                            parts.push(format!("{bpm:.1} BPM"));
                        }
                        if let Some(k) = &i.key {
                            parts.push(k.clone());
                        }
                        parts.join("   ")
                    }
                    None => tr("analysing…").to_string(),
                };
                let mut detail_colour = Color32::from_gray(0xc8);
                if let (None, Some(set)) = (stem, &set) {
                    detail = format!("{detail}   {}", trf("{} chops", &[&set.pads.len()]));
                    detail_colour = Color32::from_rgb(0xb8, 0xe0, 0xb0);
                }
                g.text(
                    rect.left_top() + vec2(22.0 + indent, 19.0),
                    Align2::LEFT_TOP,
                    detail,
                    FontId::proportional(11.0),
                    detail_colour,
                );
                let hover = match &set {
                    Some(_) => format!(
                        "{name}\n{}",
                        tr("Click: the song's chops go on the pads, and it previews. Double-click: the whole song onto the current pad.")
                    ),
                    None => name,
                };
                let resp = resp.on_hover_text(hover);
                if resp.clicked() {
                    if stem.is_some() {
                        clicked_stem = Some(path.clone());
                    } else {
                        clicked = Some(path.clone());
                    }
                }
                if resp.double_clicked() {
                    load = Some(path.clone());
                }
                resp.context_menu(|ui| {
                    if stem.is_none()
                        && ui
                            .add_enabled(set.is_some(), egui::Button::new(tr("Load Chops")))
                            .clicked()
                    {
                        load_chops = Some(path.clone());
                        ui.close();
                    }
                    if ui.button(tr("Load onto Current Pad")).clicked() {
                        load = Some(path.clone());
                        ui.close();
                    }
                    if stem.is_none()
                        && ui
                            .add_enabled(
                                st.pads.iter().any(|x| !x.is_empty()),
                                egui::Button::new(tr("Save Pads as Its Chops")),
                            )
                            .clicked()
                    {
                        save_chops = Some(path.clone());
                        ui.close();
                    }
                });
            }
        });
    if let Some(song) = save_chops {
        v.status = Some(save_set(b, st, &song));
    }
    if let Some(path) = clicked {
        let key = path.to_string_lossy().into_owned();
        let same = previewing.as_deref() == Some(key.as_str());
        b.selected = Some(path.clone());
        if same {
            stop_preview(uid, v);
        } else {
            preview(uid, v, b, p, &key, 0);
            // One click on a song with chops puts them on the pads as well.
            if load.is_none() {
                load_chops = Some(path);
            }
        }
    }
    if let Some(path) = clicked_stem {
        let key = path.to_string_lossy().into_owned();
        if previewing.as_deref() == Some(key.as_str()) {
            stop_preview(uid, v);
        } else {
            preview(uid, v, b, p, &key, 0);
        }
    }
    if let Some(song) = load_chops {
        if let Some(set) = b.set_of(&song) {
            load_set(p, st, v, &song, set);
        }
    }
    if let Some(path) = load {
        // The double-click's first click put the chops on; the double-click puts the one file
        // on the current pad instead, as before.
        v.pending = None;
        load_file(st, v, &path);
    }
}

/// A whole file onto the current pad (the SP-303 samples onto a pad; here a file is taken).
fn load_file(st: &mut SamplerState, v: &mut View, path: &Path) {
    let key = path.to_string_lossy().into_owned();
    // Not decoded yet: the pad plays the whole file (its end found when it is read).
    let (frames, rate) = sampler::with_runtime(|rt| rt.source(&key))
        .map_or((0, 44_100), |b| (b.frames() as u64, b.rate));
    sampler::assign(st, v.current, &key, 0, frames, rate);
    let bpm = sampler::with_runtime(|rt| rt.info.get(path).and_then(|i| i.bpm));
    let key_name = sampler::with_runtime(|rt| rt.info.get(path).and_then(|i| i.key.clone()));
    if let Some(src) = st.pads[v.current].source {
        st.sources[src].bpm = st.sources[src].bpm.or(bpm);
        st.sources[src].key = st.sources[src].key.clone().or(key_name);
        if st.pads[v.current].bpm.is_none() || bpm.is_some() {
            st.pads[v.current].bpm = bpm.or(st.pads[v.current].bpm);
        }
    }
    v.status = Some(trf(
        "{} loaded onto pad {}",
        &[
            &key.rsplit('/').next().unwrap_or(&key),
            &pad_label(v.current),
        ],
    ));
}

fn elide(ui: &Ui, text: &str, font: &FontId, width: f32) -> String {
    let fits = |t: &str| {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        }) <= width
    };
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let t: String = chars[..mid].iter().collect::<String>() + "…";
        if fits(&t) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

// ------------------------------------------------------------------ the editor

/// The file the editor shows: the current pad's, else the browser's selection.
fn editor_path(st: &SamplerState, v: &View, b: &Browser) -> Option<String> {
    st.pads[v.current]
        .source
        .and_then(|i| st.sources.get(i))
        .map(|s| s.path.clone())
        .or_else(|| {
            b.selected
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
        })
}

fn peaks_of(v: &mut View, path: &str, buf: &SampleBuf) -> Arc<Vec<(f32, f32)>> {
    if let Some(p) = v.peaks.get(path) {
        return p.clone();
    }
    // Min and max of every 256 frames, once; finer views read the samples.
    let mut out = Vec::with_capacity(buf.frames() / 256 + 1);
    for chunk in buf.data.chunks(512) {
        let (mut lo, mut hi) = (0f32, 0f32);
        for f in chunk.as_chunks::<2>().0 {
            let m = (f[0] + f[1]) * 0.5;
            lo = lo.min(m);
            hi = hi.max(m);
        }
        out.push((lo, hi));
    }
    v.peaks.retain(|k, _| k == path);
    let a = Arc::new(out);
    v.peaks.insert(path.to_string(), a.clone());
    a
}

#[allow(clippy::too_many_arguments)]
fn editor(
    ui: &mut Ui,
    p: &mut Project,
    uid: u64,
    t: usize,
    st: &mut SamplerState,
    v: &mut View,
    b: &Browser,
) {
    let k = v.current;
    let path = editor_path(st, v, b);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("PAD {}", pad_label(k)))
                .strong()
                .size(15.0)
                .color(Color32::WHITE),
        );
        let name = if st.pads[k].is_empty() {
            tr("(empty)").to_string()
        } else {
            st.pads[k].name.clone()
        };
        ui.label(egui::RichText::new(name).color(Color32::from_gray(0xe0)));
        if !running(uid) {
            ui.label(
                egui::RichText::new(tr("Sound is off: no audio device."))
                    .color(Color32::from_rgb(0xff, 0x90, 0x80)),
            );
        }
    });
    if let Some(s) = &v.status {
        ui.label(
            egui::RichText::new(s)
                .size(11.0)
                .color(Color32::from_gray(0xd0)),
        );
    }
    let Some(path) = path else {
        ui.add_space(30.0);
        ui.label(tr(
            "Pick a file in the browser, then press REC or double-click it to put it on the pad.",
        ));
        return;
    };
    let buf = sampler::with_runtime(|rt| rt.source(&path));
    let Some(buf) = buf else {
        let (progress, error) =
            sampler::with_runtime(|rt| (rt.progress(&path), rt.error(&path).map(String::from)));
        match error {
            Some(e) => {
                ui.label(
                    egui::RichText::new(trf("cannot read {}: {}", &[&path, &e]))
                        .color(Color32::from_rgb(0xff, 0x90, 0x80)),
                );
            }
            None => {
                ui.add(
                    egui::ProgressBar::new(progress.unwrap_or(0.0))
                        .text(tr("Decoding…"))
                        .desired_width(ui.available_width()),
                );
            }
        }
        return;
    };
    let frames = buf.frames() as u64;
    let rate = buf.rate;
    if v.view_path != path || v.view.1 <= 0.0 {
        v.view_path = path.clone();
        v.view = (0.0, frames.max(1) as f64);
        v.selection = None;
    }
    let src_index = st.pads[k].source.filter(|i| st.sources[*i].path == path);
    // Toolbar.
    ui.horizontal(|ui| {
        if ui.button("−").on_hover_text(tr("Zoom out")).clicked() {
            zoom(v, 2.0, None, frames);
        }
        if ui.button("+").on_hover_text(tr("Zoom in")).clicked() {
            zoom(v, 0.5, None, frames);
        }
        if ui.button(tr("Fit")).clicked() {
            v.view = (0.0, frames as f64);
        }
        ui.checkbox(&mut v.snap, tr("Snap to zero crossing"));
        if ui.button(tr("Audition")).clicked() {
            let (a, z) = v
                .selection
                .or_else(|| src_index.map(|_| sampler::pad_range(&st.pads[k], frames)))
                .unwrap_or((0, frames));
            audition(uid, v, &path, a, z);
        }
        if ui.button(tr("Stop")).clicked() {
            stop_preview(uid, v);
        }
    });
    // The waveform.
    let peaks = peaks_of(v, &path, &buf);
    let height = 190.0;
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
    let g = ui.painter_at(rect);
    g.rect_filled(rect, 3.0, WAVE_BG);
    let (v0, vlen) = v.view;
    let x_of = |f: f64| rect.left() + ((f - v0) / vlen) as f32 * rect.width();
    let f_of = |x: f32| ((x - rect.left()) / rect.width()) as f64 * vlen + v0;
    let mid = rect.center().y;
    let half = rect.height() * 0.45;
    let px = rect.width().max(1.0) as usize;
    for i in 0..px {
        let a = (v0 + vlen * i as f64 / px as f64).max(0.0) as u64;
        let z = ((v0 + vlen * (i + 1) as f64 / px as f64) as u64).clamp(a + 1, frames.max(1));
        let (mut lo, mut hi) = (0f32, 0f32);
        if z - a >= 512 {
            for pk in peaks
                .iter()
                .take((z / 256) as usize + 1)
                .skip((a / 256) as usize)
            {
                lo = lo.min(pk.0);
                hi = hi.max(pk.1);
            }
        } else {
            for f in a..z.min(frames) {
                let m = (buf.data[f as usize * 2] + buf.data[f as usize * 2 + 1]) * 0.5;
                lo = lo.min(m);
                hi = hi.max(m);
            }
        }
        let x = rect.left() + i as f32 + 0.5;
        g.line_segment(
            [
                pos2(x, mid - hi.min(1.0) * half),
                pos2(x, mid - lo.max(-1.0) * half),
            ],
            Stroke::new(1.0, WAVE),
        );
    }
    // The pad's range, its handles, the chops, the selection, the playhead.
    let range = src_index.map(|_| sampler::pad_range(&st.pads[k], frames));
    if let Some((a, z)) = range {
        let r = Rect::from_x_y_ranges(x_of(a as f64)..=x_of(z as f64), rect.y_range());
        g.rect_filled(r, 0.0, RANGE);
        for (f, label) in [(a, "S"), (z, "E")] {
            let x = x_of(f as f64);
            g.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(2.0, Color32::WHITE),
            );
            g.text(
                pos2(x + 3.0, rect.top() + 2.0),
                Align2::LEFT_TOP,
                label,
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
    }
    let chops: Vec<u64> = src_index
        .map(|i| st.sources[i].chops.clone())
        .unwrap_or_default();
    for (n, c) in chops.iter().enumerate() {
        let x = x_of(*c as f64);
        g.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.5, CHOP),
        );
        g.text(
            pos2(x + 2.0, rect.bottom() - 14.0),
            Align2::LEFT_TOP,
            format!("{}", n + 1),
            FontId::proportional(10.0),
            CHOP,
        );
    }
    if let Some((a, z)) = v.selection {
        let r = Rect::from_x_y_ranges(x_of(a as f64)..=x_of(z as f64), rect.y_range());
        g.rect_filled(r, 0.0, SELECT);
    }
    if let Some(at) =
        preview_frame(uid, v).filter(|_| v.preview.as_ref().is_some_and(|x| x.path == path))
    {
        let x = x_of(at as f64);
        g.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.5, Color32::from_rgb(0xff, 0x60, 0x60)),
        );
    }
    // Zoom with the wheel at the pointer; pan with a sideways scroll.
    if resp.hovered() {
        let (scroll, zoom_delta) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        let at = ui.input(|i| i.pointer.hover_pos()).map(|p| f_of(p.x));
        if zoom_delta != 1.0 {
            zoom(v, 1.0 / f64::from(zoom_delta), at, frames);
        } else if scroll.y.abs() > 0.5 {
            zoom(
                v,
                if scroll.y > 0.0 { 0.85 } else { 1.0 / 0.85 },
                at,
                frames,
            );
        }
        if scroll.x.abs() > 0.5 {
            v.view.0 = (v.view.0 - f64::from(scroll.x) / f64::from(rect.width()) * v.view.1)
                .clamp(0.0, (frames as f64 - v.view.1).max(0.0));
        }
    }
    let snap_on = v.snap;
    let snap = |f: u64| {
        if snap_on {
            sample_tools::zero_crossing(&buf, f as usize, (rate / 200) as usize) as u64
        } else {
            f
        }
    };
    let near = |x: f32, f: u64| (x - x_of(f as f64)).abs() < 6.0;
    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let f = f_of(pos.x).clamp(0.0, frames as f64) as u64;
            v.drag = Some(match range {
                Some((a, _)) if near(pos.x, a) => Drag::Start,
                Some((_, z)) if near(pos.x, z) => Drag::End,
                _ => match chops.iter().position(|c| near(pos.x, *c)) {
                    Some(i) if v.chop == ChopMode::Manual => Drag::Chop(i),
                    _ => Drag::Select(f),
                },
            });
        }
    }
    if resp.dragged() {
        if let (Some(d), Some(pos)) = (v.drag, resp.interact_pointer_pos()) {
            let f = f_of(pos.x).clamp(0.0, frames as f64) as u64;
            match d {
                Drag::Start => {
                    let end = sampler::pad_range(&st.pads[k], frames).1;
                    st.pads[k].start = snap(f).min(end.saturating_sub(1));
                }
                Drag::End => {
                    let start = st.pads[k].start;
                    st.pads[k].end = snap(f).max(start + 1).min(frames);
                }
                Drag::Chop(i) => {
                    if let Some(si) = src_index {
                        if let Some(c) = st.sources[si].chops.get_mut(i) {
                            *c = snap(f);
                        }
                    }
                }
                Drag::Select(from) => v.selection = Some((from.min(f), from.max(f))),
            }
        }
    }
    if resp.drag_stopped() {
        if let Some(Drag::Select(_)) = v.drag {
            if let Some((a, z)) = v.selection {
                let (a, z) = (snap(a), snap(z));
                v.selection = Some((a, z));
                if z > a + 64 {
                    audition(uid, v, &path, a, z);
                }
            }
        }
        if let (Some(Drag::Chop(_)), Some(si)) = (v.drag, src_index) {
            st.sources[si].chops.sort_unstable();
        }
        v.drag = None;
    }
    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let f = f_of(pos.x).clamp(0.0, frames as f64) as u64;
            match (v.chop, src_index) {
                (ChopMode::Manual, Some(si)) => {
                    let c = &mut st.sources[si].chops;
                    c.push(snap(f));
                    c.sort_unstable();
                    c.dedup();
                }
                _ => {
                    v.selection = None;
                    audition(uid, v, &path, f, frames);
                }
            }
        }
    }
    if resp.secondary_clicked() {
        if let (Some(pos), Some(si)) = (resp.interact_pointer_pos(), src_index) {
            if let Some(i) = chops.iter().position(|c| near(pos.x, *c)) {
                st.sources[si].chops.remove(i);
            } else {
                v.selection = None;
            }
        }
    }
    ui.label(
        egui::RichText::new(tr("Drag to select and hear a part. Drag S and E to trim the pad. In Manual, click to add a chop point, drag one to move it, right-click to delete it. Scroll to zoom."))
            .size(10.5)
            .color(Color32::from_gray(0xb8)),
    );
    // Chop (an extension: the SP-303 has none).
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(tr("CHOP (not on the SP-303)"))
                .strong()
                .color(Color32::WHITE),
        );
        for (m, name) in [
            (ChopMode::Manual, tr("Manual")),
            (ChopMode::Transients, tr("Transients")),
            (ChopMode::Equal, tr("Equal")),
            (ChopMode::Beats, tr("Beats")),
        ] {
            ui.selectable_value(&mut v.chop, m, name);
        }
    });
    let pad_bpm = st.pads[k].bpm.or(src_index.and_then(|i| st.sources[i].bpm));
    ui.horizontal_wrapped(|ui| {
        match v.chop {
            ChopMode::Manual => {}
            ChopMode::Transients => {
                ui.label(tr("Sensitivity"));
                ui.add(egui::Slider::new(&mut v.sensitivity, 0.0..=1.0).show_value(false));
            }
            ChopMode::Equal => {
                ui.label(tr("Slices"));
                ui.add(egui::DragValue::new(&mut v.divisions).range(2..=64));
            }
            ChopMode::Beats => {
                ui.label(tr("Beats per slice"));
                egui::ComboBox::from_id_salt("chop-beats")
                    .selected_text(format!("{}", v.beats))
                    .show_ui(ui, |ui| {
                        for x in [0.25, 0.5, 1.0, 2.0, 4.0] {
                            ui.selectable_value(&mut v.beats, x, format!("{x}"));
                        }
                    });
                ui.label(match pad_bpm {
                    Some(bpm) => trf("at {} BPM", &[&format!("{bpm:.1}")]),
                    None => tr("type the pad's BPM below first").to_string(),
                });
            }
        }
        let can = src_index.is_some();
        if v.chop != ChopMode::Manual
            && ui.add_enabled(can, egui::Button::new(tr("Chop"))).clicked()
        {
            if let Some(si) = src_index {
                let (a, z) = sampler::pad_range(&st.pads[k], frames);
                let (a, z) = (a as usize, z as usize);
                let points: Vec<usize> = match v.chop {
                    ChopMode::Transients => sample_tools::transients(&buf, a, z, v.sensitivity),
                    ChopMode::Equal => sample_tools::equal_divisions(a, z, v.divisions),
                    ChopMode::Beats => pad_bpm
                        .map(|bpm| sample_tools::beat_divisions(a, z, rate, bpm, v.beats))
                        .unwrap_or_default(),
                    ChopMode::Manual => Vec::new(),
                };
                let c = &mut st.sources[si].chops;
                c.retain(|x| (*x as usize) < a || (*x as usize) >= z);
                c.extend(points.into_iter().filter(|x| *x > a).map(|x| x as u64));
                c.sort_unstable();
                c.dedup();
                v.status = Some(trf("{} chop points", &[&c.len()]));
            }
        }
        if ui
            .add_enabled(can, egui::Button::new(tr("Clear Chops")))
            .clicked()
        {
            if let Some(si) = src_index {
                st.sources[si].chops.clear();
            }
        }
        if ui
            .add_enabled(
                can && !chops.is_empty(),
                egui::Button::new(tr("Slices to Pads")),
            )
            .on_hover_text(tr(
                "Each slice onto a pad, from this pad on (into banks E–H past D).",
            ))
            .clicked()
        {
            let made = sampler::slices_to_pads(st, k, k, &chops, rate, frames);
            v.status = Some(trf("{} slices on pads {} on", &[&made, &pad_label(k)]));
        }
        if ui
            .add_enabled(
                v.selection.is_some(),
                egui::Button::new(tr("Selection to Pad")),
            )
            .clicked()
        {
            if let Some((a, z)) = v.selection {
                sampler::assign(st, k, &path, a, z, rate);
            }
        }
    });
    // Extension banks (E–H) for big chops.
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(tr("Banks E–H (not on the SP-303)"))
                .size(11.0)
                .color(Color32::from_gray(0xc0)),
        );
        for bank in HARDWARE_BANKS..sampler::BANKS {
            let name = ((b'A' + bank as u8) as char).to_string();
            if ui.selectable_label(v.bank == bank, name).clicked() {
                v.bank = bank;
            }
        }
    });
    ui.separator();
    pad_settings(ui, p, uid, t, st, v, &buf, src_index.is_some());
}

fn zoom(v: &mut View, factor: f64, at: Option<f64>, frames: u64) {
    let (v0, len) = v.view;
    let new_len = (len * factor).clamp(64.0, frames.max(64) as f64);
    let at = at.unwrap_or(v0 + len / 2.0);
    let rel = (at - v0) / len;
    let start = (at - rel * new_len).clamp(0.0, (frames as f64 - new_len).max(0.0));
    v.view = (start, new_len);
}

/// The current pad's settings, below the waveform.
#[allow(clippy::too_many_arguments)]
fn pad_settings(
    ui: &mut Ui,
    p: &mut Project,
    uid: u64,
    t: usize,
    st: &mut SamplerState,
    v: &mut View,
    buf: &SampleBuf,
    on_source: bool,
) {
    let k = v.current;
    if !on_source || st.pads[k].is_empty() {
        ui.label(tr("This pad is empty."));
        return;
    }
    let bpm_project = sampler::project_bpm(p);
    let bar = sampler::beats_per_bar(p);
    let frames = buf.frames() as u64;
    let src = st.pads[k].source.unwrap_or(0);
    egui::ScrollArea::vertical()
        .id_salt("pad-settings")
        .show(ui, |ui| {
            egui::Grid::new("pad-grid")
                .num_columns(4)
                .spacing(vec2(10.0, 4.0))
                .show(ui, |ui| {
                    let pad = &mut st.pads[k];
                    ui.label(tr("Pitch"));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut pad.semis)
                                .range(-24..=24)
                                .suffix(" st"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut pad.cents)
                                .range(-100..=100)
                                .suffix(" ct"),
                        );
                    });
                    ui.label(tr("Mode"));
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut pad.mode, PitchMode::Repitch, tr("Repitch"))
                            .on_hover_text(tr(
                                "Speed and pitch change together, like a record sped up.",
                            ));
                        ui.selectable_value(&mut pad.mode, PitchMode::Stretch, tr("Stretch"))
                            .on_hover_text(tr(
                                "Pitch and length change apart (prepared in the background).",
                            ));
                    });
                    ui.end_row();

                    ui.label(tr("Time"));
                    ui.horizontal(|ui| {
                        let mut kind = match pad.time {
                            TimeMode::Off => 0,
                            TimeMode::Ratio(_) => 1,
                            TimeMode::Follow => 2,
                            TimeMode::Bars(_) => 3,
                        };
                        let old = kind;
                        egui::ComboBox::from_id_salt("time-kind")
                            .selected_text(
                                [tr("Off"), tr("Tempo ratio"), tr("Follow tempo"), tr("Bars")]
                                    [kind],
                            )
                            .show_ui(ui, |ui| {
                                for (i, n) in
                                    [tr("Off"), tr("Tempo ratio"), tr("Follow tempo"), tr("Bars")]
                                        .into_iter()
                                        .enumerate()
                                {
                                    ui.selectable_value(&mut kind, i, n);
                                }
                            });
                        if kind != old {
                            pad.time = match kind {
                                1 => TimeMode::Ratio(1.0),
                                2 => TimeMode::Follow,
                                3 => TimeMode::Bars(1.0),
                                _ => TimeMode::Off,
                            };
                        }
                        match &mut pad.time {
                            TimeMode::Ratio(r) => {
                                ui.add(
                                    egui::DragValue::new(r)
                                        .range(0.5..=1.3)
                                        .speed(0.005)
                                        .suffix("×"),
                                );
                            }
                            TimeMode::Bars(bars) => {
                                ui.add(egui::DragValue::new(bars).range(0.25..=32.0).speed(0.05));
                            }
                            _ => {}
                        }
                    });
                    ui.label(tr("Fit to tempo"));
                    ui.horizontal(|ui| {
                        for bars in [1.0, 2.0, 4.0] {
                            if ui.button(trf("{} bars", &[&bars])).clicked() {
                                pad.time = TimeMode::Bars(bars);
                            }
                        }
                    });
                    ui.end_row();

                    // Tempo: the file's, the pad's, and what it plays at.
                    let (a, z) = sampler::pad_range(pad, frames);
                    let seconds = (z - a) as f64 / f64::from(buf.rate.max(1));
                    let factor = sampler::length_factor(pad, seconds, bpm_project, bar);
                    ui.label(tr("Pad BPM"));
                    ui.horizontal(|ui| {
                        let mut bpm = pad.bpm.unwrap_or(0.0);
                        if ui
                            .add(
                                egui::DragValue::new(&mut bpm)
                                    .range(0.0..=300.0)
                                    .speed(0.1)
                                    .max_decimals(1),
                            )
                            .changed()
                        {
                            pad.bpm = (bpm >= 20.0).then_some(bpm);
                        }
                        if ui.small_button("½").clicked() {
                            pad.bpm = pad.bpm.map(|b| b / 2.0);
                        }
                        if ui.small_button("×2").clicked() {
                            pad.bpm = pad.bpm.map(|b| b * 2.0);
                        }
                    });
                    ui.label(tr("Result"));
                    let source_bpm = st.sources.get(src).and_then(|s| s.bpm);
                    let result = pad.bpm.map(|b| b / factor);
                    ui.label(
                        egui::RichText::new(format!(
                            "{}\n{}   {}",
                            trf(
                                "file {}",
                                &[&source_bpm.map_or("—".into(), |b| format!("{b:.1}"))]
                            ),
                            trf(
                                "plays {}",
                                &[&result.map_or("—".into(), |b| format!("{b:.1} BPM"))]
                            ),
                            trf("length ×{}", &[&format!("{factor:.3}")])
                        ))
                        .color(Color32::WHITE),
                    );
                    ui.end_row();

                    ui.label(tr("Gain"));
                    ui.add(
                        egui::DragValue::new(&mut pad.gain_db)
                            .range(-60.0..=24.0)
                            .speed(0.1)
                            .suffix(" dB"),
                    );
                    ui.label(tr("Fades"));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut pad.fade_in)
                                .range(0.0..=10.0)
                                .speed(0.001)
                                .prefix("in ")
                                .suffix(" s"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut pad.fade_out)
                                .range(0.0..=10.0)
                                .speed(0.001)
                                .prefix("out ")
                                .suffix(" s"),
                        );
                    });
                    ui.end_row();

                    ui.label(tr("Sound"));
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut pad.normalize, tr("Normalize"));
                        ui.checkbox(&mut pad.reverse, tr("Reverse"));
                    });
                    ui.label(tr("Plays"));
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut pad.gate, "GATE");
                        ui.checkbox(&mut pad.looping, "LOOP");
                    });
                    ui.end_row();

                    ui.label(tr("Level"));
                    ui.add(egui::Slider::new(&mut pad.level, 0..=127));
                    ui.label(tr("Pan"));
                    ui.add(egui::Slider::new(&mut pad.pan, -1.0..=1.0).show_value(false));
                    ui.end_row();

                    ui.label(tr("Choke group"));
                    ui.add(egui::DragValue::new(&mut pad.choke).range(0..=16))
                        .on_hover_text(tr("Pads in the same group cut each other off (0: none)."));
                    ui.label(tr("Velocity"));
                    ui.add(egui::Slider::new(&mut pad.vel_sens, 0.0..=1.0).show_value(false));
                    ui.end_row();

                    ui.label(tr("Attack / Decay"));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut pad.attack)
                                .range(0.0..=5.0)
                                .speed(0.001)
                                .suffix(" s"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut pad.decay)
                                .range(0.0..=5.0)
                                .speed(0.001)
                                .suffix(" s"),
                        );
                    });
                    ui.label(tr("Sustain / Release"));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut pad.sustain)
                                .range(0.0..=1.0)
                                .speed(0.01),
                        );
                        ui.add(
                            egui::DragValue::new(&mut pad.release)
                                .range(0.0..=5.0)
                                .speed(0.001)
                                .suffix(" s"),
                        );
                    });
                    ui.end_row();

                    ui.label(tr("Low-pass"));
                    ui.add(
                        egui::Slider::new(&mut pad.cutoff, 40.0..=20_000.0)
                            .logarithmic(true)
                            .suffix(" Hz"),
                    );
                    ui.label(tr("Resonance"));
                    ui.add(egui::Slider::new(&mut pad.resonance, 0.0..=1.0).show_value(false));
                    ui.end_row();
                });
            ui.horizontal_wrapped(|ui| {
                let mut chromatic = st.chromatic == Some(k);
                if ui
                    .checkbox(&mut chromatic, tr("Chromatic"))
                    .on_hover_text(tr("Every key plays this pad, a semitone apart."))
                    .changed()
                {
                    st.chromatic = chromatic.then_some(k);
                }
                if st.chromatic.is_some() {
                    ui.label(tr("Root"));
                    ui.add(egui::DragValue::new(&mut st.root).range(0..=127));
                }
                ui.checkbox(&mut st.consolidate, tr("Copy files into the project"));
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button(tr("Slices to MIDI Pattern")).clicked() {
                    match sampler::slices_to_pattern(p, t, st, k) {
                        Some(_) => {
                            v.status = Some(
                                tr("A region with the slices is on the track at the playhead.")
                                    .into(),
                            )
                        }
                        None => v.status = Some(tr("No slices of this file on the pads.").into()),
                    }
                }
                if ui.button(tr("Slice to Audio Track")).clicked() {
                    let made = sampler::with_runtime(|rt| {
                        let spec = sampler::render_spec(st, k, frames, buf.rate, bpm_project, bar)?;
                        let src = rt.source(&spec.path)?;
                        Some(sampler::render(&src, &spec))
                    });
                    match made {
                        Some(sound) => {
                            let name = format!("{} {}", st.pads[k].name, pad_label(k));
                            match sampler::slice_to_audio_track(
                                p,
                                &sound,
                                &name,
                                &crate::project_io::recordings_dir(),
                                ui.ctx(),
                            ) {
                                Ok(_) => {
                                    v.status = Some(
                                        tr("The slice is on a new audio track at the playhead.")
                                            .into(),
                                    )
                                }
                                Err(e) => v.status = Some(e),
                            }
                        }
                        None => v.status = Some(tr("The pad's sound is not ready yet.").into()),
                    }
                }
                let _ = uid;
            });
        });
}

// ------------------------------------------------------------------ the SP-303 panel

/// The SP-303's front panel.
#[allow(clippy::too_many_arguments)]
fn panel(
    ui: &mut Ui,
    r: Rect,
    p: &mut Project,
    uid: u64,
    t: usize,
    st: &mut SamplerState,
    v: &mut View,
    b: &mut Browser,
) {
    let time = now(ui);
    let g = ui.painter().clone();
    let at = |x: f32, y: f32| r.min + vec2(x, y);
    let rect = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(at(x, y), vec2(w, h));
    let bold = |s: f32| FontId::new(s, egui::FontFamily::Proportional);
    g.rect_filled(r, 12.0, BEZEL);
    let plate = r.shrink(10.0);
    g.rect_filled(plate, 6.0, PLATE);
    // Logo strip.
    let (mark, model) = ("PRO WAVE", "SAMPLER");
    g.text(at(26.0, 22.0), Align2::LEFT_CENTER, mark, bold(20.0), INK);
    g.text(
        at(PANEL_W - 24.0, 22.0),
        Align2::RIGHT_CENTER,
        model,
        bold(16.0),
        INK,
    );
    // The black block.
    let block = rect(16.0, 38.0, PANEL_W - 32.0, 262.0);
    g.rect_filled(block, 4.0, BLOCK);
    g.text(
        at(240.0, 50.0),
        Align2::CENTER_CENTER,
        "EFFECTS CONTROL",
        bold(11.0),
        Color32::from_rgb(0xff, 0x4a, 0x50),
    );
    g.line_segment([at(120.0, 58.0), at(380.0, 58.0)], Stroke::new(1.0, RED));
    // VOLUME.
    g.text(
        at(58.0, 64.0),
        Align2::CENTER_CENTER,
        "VOLUME",
        bold(11.0),
        Color32::WHITE,
    );
    let mut vol = st.volume;
    if knob(
        ui,
        at(58.0, 100.0),
        21.0,
        &mut vol,
        Color32::WHITE,
        ("prowave-sampler-vol", uid),
    ) {
        st.volume = vol;
        v.flash(format!("{}", (vol * 127.0).round()), time);
    }
    g.text(
        at(36.0, 128.0),
        Align2::CENTER_CENTER,
        "MIN",
        bold(8.0),
        Color32::WHITE,
    );
    g.text(
        at(80.0, 128.0),
        Align2::CENTER_CENTER,
        "MAX",
        bold(8.0),
        Color32::WHITE,
    );
    // CTRL 1–3 (manual p. 7: the legends printed under each).
    let legends = [
        ("CTRL 1", "CUTOFF", "TIME/START"),
        ("CTRL 2", "RESONANCE", "BPM/END"),
        ("CTRL 3 / MFX", "DRIVE", "LEVEL"),
    ];
    for (i, (top, a, bb)) in legends.into_iter().enumerate() {
        let cx = 150.0 + i as f32 * 92.0;
        g.text(
            at(cx, 66.0),
            Align2::CENTER_CENTER,
            top,
            bold(10.0),
            Color32::WHITE,
        );
        let mut value = knob_value(p, st, v, i);
        let resp_changed = knob(
            ui,
            at(cx, 100.0),
            22.0,
            &mut value,
            LIT,
            ("prowave-sampler-ctrl", uid, i),
        );
        if resp_changed {
            set_knob(p, st, v, i, value, time);
        }
        g.text(
            at(cx, 132.0),
            Align2::CENTER_CENTER,
            a,
            bold(10.0),
            Color32::WHITE,
        );
        g.text(
            at(cx, 144.0),
            Align2::CENTER_CENTER,
            bb,
            bold(9.0),
            Color32::WHITE,
        );
        // What the knob does now (the hardware shows it on the display as it turns).
        g.text(
            at(cx, 158.0),
            Align2::CENTER_CENTER,
            knob_role(st, v, i),
            FontId::proportional(9.0),
            Color32::from_rgb(0xff, 0xd0, 0xa0),
        );
    }
    // Knob let go: a START/END move is kept and the knob returns to the centre (manual p. 40).
    if v.knobs == KnobMode::StartEndLevel && !ui.input(|i| i.pointer.any_down()) {
        v.nudge = None;
        v.nudge_knob = [0.5; 2];
    }
    // PEAK LED.
    let peak = sampler::with_runtime(|rt| (0..PADS).any(|k| rt.is_playing(uid, k)));
    g.circle_filled(at(370.0, 132.0), 3.5, if peak { LIT } else { LED_OFF });
    g.text(
        at(370.0, 142.0),
        Align2::CENTER_CENTER,
        "PEAK",
        bold(8.0),
        Color32::WHITE,
    );
    g.line_segment(
        [at(24.0, 168.0), at(PANEL_W - 24.0, 168.0)],
        Stroke::new(1.0, Color32::from_gray(0xc0)),
    );
    // BPM/VALUE display.
    g.text(
        at(70.0, 178.0),
        Align2::CENTER_CENTER,
        "BPM/VALUE",
        bold(11.0),
        Color32::WHITE,
    );
    let shown = display_text(p, st, v, time);
    let blink_on = (time * 3.0).fract() < 0.6;
    seven_segment(
        &g,
        rect(26.0, 188.0, 90.0, 40.0),
        &shown,
        if v.del || v.mark == 1 || v.mark == 2 {
            blink_on
        } else {
            true
        },
    );
    // DSP EFFECTS.
    g.text(
        at(212.0, 178.0),
        Align2::CENTER_CENTER,
        "DSP EFFECTS",
        bold(11.0),
        Color32::WHITE,
    );
    let fx_keys = [
        (SpFxKind::FilterDrive, "FILTER\n+DRIVE", 0, 0),
        (SpFxKind::Pitch, "PITCH", 1, 0),
        (SpFxKind::Delay, "DELAY", 2, 0),
        (SpFxKind::VinylSim, "VINYL\nSIM", 1, 1),
        (SpFxKind::Isolator, "ISOLATOR", 2, 1),
    ];
    for (kind, label, col, row) in fx_keys {
        let k_rect = rect(
            132.0 + col as f32 * 58.0,
            188.0 + row as f32 * 32.0,
            52.0,
            26.0,
        );
        let lit = st.fx.on && st.fx.kind == kind;
        if key(
            ui,
            &g,
            k_rect,
            label,
            lit,
            ("prowave-sampler-fx", uid, col, row),
        )
        .clicked()
        {
            effect_key(st, v, kind, time);
        }
    }
    // MFX: click to switch on; scroll or drag up/down on it to pick 1–21 (the hardware: hold
    // MFX and turn CTRL 3, p. 22), or click a name in the list below.
    let mfx_rect = rect(132.0, 220.0, 52.0, 26.0);
    let mfx_kind = SpFxKind::from_mfx(st.mfx).unwrap_or(SpFxKind::Reverb);
    let mfx_lit = st.fx.on && st.fx.kind.mfx_number().is_some();
    let resp = key(
        ui,
        &g,
        mfx_rect,
        "MFX",
        mfx_lit,
        ("prowave-sampler-mfx", uid),
    );
    let scroll = if resp.hovered() {
        ui.input(|i| i.smooth_scroll_delta.y)
    } else {
        0.0
    };
    let dy = if resp.dragged() {
        -resp.drag_delta().y
    } else {
        0.0
    };
    if scroll.abs() > 4.0 || dy.abs() > 6.0 {
        let step = if scroll + dy > 0.0 { 1 } else { -1 };
        pick_mfx(st, v, step_mfx(st.mfx, step), time);
    }
    if resp.clicked() {
        effect_key(st, v, mfx_kind, time);
    }
    let _ = resp.on_hover_text(tr(
        "Scroll or drag to pick the MFX type (or click its name below).",
    ));
    // SAMPLE EDIT.
    g.line_segment(
        [at(312.0, 186.0), at(312.0, 246.0)],
        Stroke::new(1.0, Color32::WHITE),
    );
    g.text(
        at(352.0, 178.0),
        Align2::CENTER_CENTER,
        "SAMPLE EDIT",
        bold(10.0),
        Color32::WHITE,
    );
    if key(
        ui,
        &g,
        rect(320.0, 188.0, 62.0, 26.0),
        "START/END\n/LEVEL",
        v.knobs == KnobMode::StartEndLevel,
        ("prowave-sampler-sel", uid),
    )
    .clicked()
    {
        v.knobs = if v.knobs == KnobMode::StartEndLevel {
            KnobMode::Effect
        } else {
            KnobMode::StartEndLevel
        };
    }
    if key(
        ui,
        &g,
        rect(320.0, 220.0, 62.0, 26.0),
        "TIME/\nBPM",
        v.knobs == KnobMode::TimeBpm,
        ("prowave-sampler-tb", uid),
    )
    .clicked()
    {
        v.knobs = if v.knobs == KnobMode::TimeBpm {
            KnobMode::Effect
        } else {
            KnobMode::TimeBpm
        };
    }
    // The MFX list printed under the keys (clickable here).
    g.line_segment(
        [at(24.0, 252.0), at(PANEL_W - 24.0, 252.0)],
        Stroke::new(1.0, Color32::from_gray(0xc0)),
    );
    for n in 1..=21usize {
        let col = (n - 1) / 4;
        let row = (n - 1) % 4;
        let kind = SpFxKind::from_mfx(n).unwrap_or(SpFxKind::Reverb);
        let xs = [22.0, 80.0, 144.0, 190.0, 262.0, 330.0];
        let pos = at(xs[col], 258.0 + row as f32 * 10.0);
        let label = format!("{n} {}", kind.name());
        let color = if !kind.available() {
            Color32::from_gray(0x90)
        } else if st.mfx == n {
            Color32::from_rgb(0xff, 0x80, 0x80)
        } else {
            Color32::WHITE
        };
        let tr_rect = g.text(
            pos,
            Align2::LEFT_TOP,
            &label,
            FontId::proportional(7.0),
            color,
        );
        let resp = ui.interact(
            tr_rect,
            ui.make_persistent_id(("prowave-sampler-mfxlist", uid, n)),
            Sense::click(),
        );
        let resp = if kind.available() {
            resp
        } else {
            resp.on_hover_text(tr("Not made in Pro Wave."))
        };
        if resp.clicked() && kind.available() {
            pick_mfx(st, v, n, time);
        }
    }
    // PATTERN SEQUENCER bar.
    let bar = rect(16.0, 312.0, 296.0, 34.0);
    g.rect_filled(bar, 9.0, RED);
    g.text(
        at(26.0, 322.0),
        Align2::LEFT_CENTER,
        "PATTERN",
        bold(11.0),
        Color32::WHITE,
    );
    g.text(
        at(26.0, 335.0),
        Align2::LEFT_CENTER,
        "SEQUENCER",
        bold(11.0),
        Color32::WHITE,
    );
    if key(
        ui,
        &g,
        rect(104.0, 316.0, 56.0, 26.0),
        "PATTERN\nSELECT",
        v.pattern_select,
        ("prowave-sampler-ps", uid),
    )
    .clicked()
    {
        v.pattern_select = !v.pattern_select;
    }
    g.rect_filled(rect(166.0, 322.0, 30.0, 14.0), 2.0, Color32::WHITE);
    g.text(
        at(181.0, 329.0),
        Align2::CENTER_CENTER,
        "CTRL 3",
        bold(7.0),
        INK,
    );
    if key(
        ui,
        &g,
        rect(198.0, 316.0, 52.0, 26.0),
        "LENGTH",
        v.knobs == KnobMode::Length,
        ("prowave-sampler-len", uid),
    )
    .clicked()
    {
        v.knobs = if v.knobs == KnobMode::Length {
            KnobMode::Effect
        } else {
            KnobMode::Length
        };
        v.flash(format!("{}", st.pattern_len), time);
    }
    if key(
        ui,
        &g,
        rect(254.0, 316.0, 54.0, 26.0),
        "QUANTIZE",
        v.knobs == KnobMode::Quantize,
        ("prowave-sampler-q", uid),
    )
    .clicked()
    {
        v.knobs = if v.knobs == KnobMode::Quantize {
            KnobMode::Effect
        } else {
            KnobMode::Quantize
        };
        v.flash(st.quantize.code(), time);
    }
    g.text(
        at(200.0, 354.0),
        Align2::CENTER_CENTER,
        "CTRL 2 = BPM ADJ",
        bold(8.0),
        INK,
    );
    g.text(
        at(352.0, 308.0),
        Align2::CENTER_CENTER,
        "EFFECT GRAB",
        bold(8.0),
        INK,
    );
    if key(
        ui,
        &g,
        rect(322.0, 316.0, 60.0, 26.0),
        "TAP\nTEMPO",
        false,
        ("prowave-sampler-tap", uid),
    )
    .clicked()
    {
        tap(p, st, v, time);
    }
    g.line_segment(
        [at(20.0, 364.0), at(PANEL_W - 20.0, 364.0)],
        Stroke::new(1.0, INK),
    );
    // Row: CANCEL, REMAIN, SAMPLE MODE.
    let row1 = 386.0;
    g.text(
        at(42.0, row1 - 10.0),
        Align2::CENTER_CENTER,
        "PATTERN STOP",
        bold(7.0),
        INK,
    );
    if key(
        ui,
        &g,
        rect(18.0, row1, 48.0, 24.0),
        "CANCEL",
        false,
        ("prowave-sampler-cancel", uid),
    )
    .clicked()
    {
        cancel(p, uid, v);
    }
    g.text(
        at(94.0, row1 - 10.0),
        Align2::CENTER_CENTER,
        "CURRENT PAD",
        bold(7.0),
        INK,
    );
    if key(
        ui,
        &g,
        rect(70.0, row1, 48.0, 24.0),
        "REMAIN",
        v.remain,
        ("prowave-sampler-remain", uid),
    )
    .clicked()
    {
        v.remain = !v.remain;
    }
    let mode_box = rect(122.0, row1 - 4.0, 264.0, 32.0);
    g.rect_stroke(
        mode_box,
        6.0,
        Stroke::new(2.0, RED),
        egui::StrokeKind::Inside,
    );
    g.text(
        at(290.0, row1 - 12.0),
        Align2::CENTER_CENTER,
        "SAMPLE MODE",
        bold(10.0),
        RED,
    );
    g.text(
        at(150.0, row1 - 14.0),
        Align2::CENTER_CENTER,
        "LIT LONG / BLINK LO-FI",
        bold(6.5),
        INK,
    );
    let cur = v.current;
    let pad = st.pads[cur].clone();
    let grade_lit = pad.grade != Grade::Standard && (pad.grade == Grade::Long || blink_on);
    let mode_keys: [(&str, bool); 5] = [
        ("LONG/\nLO-FI", grade_lit),
        ("STEREO", pad.stereo),
        ("GATE", pad.gate),
        ("LOOP", pad.looping),
        ("REVERSE", pad.reverse),
    ];
    for (i, (label, lit)) in mode_keys.into_iter().enumerate() {
        let kr = rect(126.0 + i as f32 * 52.0, row1, 48.0, 24.0);
        if key(
            ui,
            &g,
            kr,
            label,
            lit && !pad.is_empty(),
            ("prowave-sampler-mode", uid, i),
        )
        .clicked()
        {
            let p = &mut st.pads[cur];
            match i {
                // Grade: STANDARD → LONG (lit) → LO-FI (blinking) (manual p. 27).
                0 => {
                    p.grade = match p.grade {
                        Grade::Standard => Grade::Long,
                        Grade::Long => Grade::LoFi,
                        Grade::LoFi => Grade::Standard,
                    };
                    v.flash(
                        match p.grade {
                            Grade::Standard => "Std",
                            Grade::Long => "Lon",
                            Grade::LoFi => "LoF",
                        },
                        time,
                    );
                }
                1 => p.stereo = !p.stereo,
                2 => p.gate = !p.gate,
                3 => p.looping = !p.looping,
                _ => p.reverse = !p.reverse,
            }
        }
    }
    // Row: DEL, REC, RE-SAMPLE, MARK, BANK.
    let row2 = 438.0;
    if key(
        ui,
        &g,
        rect(18.0, row2, 44.0, 26.0),
        "DEL",
        v.del && blink_on,
        ("prowave-sampler-del", uid),
    )
    .clicked()
    {
        del_key(st, v, time);
    }
    let rec_box = rect(66.0, row2 - 4.0, 54.0, 34.0);
    g.rect_stroke(
        rec_box,
        4.0,
        Stroke::new(2.0, RED),
        egui::StrokeKind::Inside,
    );
    g.text(
        at(110.0, row2 - 14.0),
        Align2::CENTER_CENTER,
        "SAMPLING",
        bold(10.0),
        RED,
    );
    g.text(
        at(93.0, row2 + 38.0),
        Align2::CENTER_CENTER,
        "PATTERN REC",
        bold(8.0),
        RED,
    );
    if key(
        ui,
        &g,
        rect(70.0, row2, 46.0, 26.0),
        "REC",
        v.rec.is_some(),
        ("prowave-sampler-rec", uid),
    )
    .clicked()
    {
        rec_key(p, t, st, v, b, time);
    }
    if key(
        ui,
        &g,
        rect(124.0, row2, 54.0, 26.0),
        "RE-\nSAMPLE",
        false,
        ("prowave-sampler-resample", uid),
    )
    .clicked()
    {
        resample_key(st, v, p, time);
    }
    g.text(
        at(209.0, row2 - 10.0),
        Align2::CENTER_CENTER,
        "START POINT",
        bold(7.0),
        INK,
    );
    g.text(
        at(209.0, row2 + 34.0),
        Align2::CENTER_CENTER,
        "END POINT",
        bold(7.0),
        INK,
    );
    let mark_lit = v.mark == 3 || ((v.mark == 1 || v.mark == 2) && blink_on);
    if key(
        ui,
        &g,
        rect(184.0, row2, 50.0, 26.0),
        "MARK",
        mark_lit,
        ("prowave-sampler-mark", uid),
    )
    .clicked()
    {
        mark_key(uid, st, v, time);
    }
    g.text(
        at(310.0, row2 - 12.0),
        Align2::CENTER_CENTER,
        "BANK",
        bold(11.0),
        INK,
    );
    let bank_frame = rect(240.0, row2 - 4.0, 146.0, 34.0);
    g.rect_filled(bank_frame, 6.0, BEZEL);
    for i in 0..HARDWARE_BANKS {
        let x = 244.0 + i as f32 * 35.0 + if i >= 2 { 4.0 } else { 0.0 };
        let label = ((b'A' + i as u8) as char).to_string();
        let lit = if v.pattern_select {
            v.pattern_bank == i
        } else {
            v.bank == i
        };
        if key(
            ui,
            &g,
            rect(x, row2, 30.0, 26.0),
            &label,
            lit,
            ("prowave-sampler-bank", uid, i),
        )
        .clicked()
        {
            if v.pattern_select {
                v.pattern_bank = i;
            } else {
                v.bank = i;
            }
        }
    }
    g.line_segment(
        [at(313.0, row2 + 2.0), at(313.0, row2 + 24.0)],
        Stroke::new(1.5, Color32::WHITE),
    );
    g.text(
        at(278.0, row2 + 38.0),
        Align2::CENTER_CENTER,
        "INTERNAL",
        bold(8.0),
        INK,
    );
    g.text(
        at(350.0, row2 + 38.0),
        Align2::CENTER_CENTER,
        "CARD",
        bold(8.0),
        INK,
    );
    g.line_segment(
        [at(20.0, 488.0), at(PANEL_W - 20.0, 488.0)],
        Stroke::new(1.0, INK),
    );
    // The pads (4 × 2), HOLD and EXT SOURCE.
    for n in 0..PADS_PER_BANK {
        let (col, row) = (n % 4, n / 4);
        let pr = rect(
            22.0 + col as f32 * 72.0,
            498.0 + row as f32 * 60.0,
            64.0,
            52.0,
        );
        let k = v.bank * PADS_PER_BANK + n;
        let lit = if v.pattern_select {
            v.pattern == v.pattern_bank * PADS_PER_BANK + n
        } else {
            sampler::with_runtime(|rt| rt.is_playing(uid, k))
                || (v.remain && st.pads[k].fx && blink_on)
        };
        let id = ui.make_persistent_id(("prowave-sampler-pad", uid, n));
        let resp = ui.interact(pr, id, Sense::click_and_drag());
        let fill = if lit { LIT } else { PAD };
        g.rect_filled(pr, 4.0, fill);
        g.rect_stroke(
            pr,
            4.0,
            Stroke::new(
                if k == v.current { 2.5 } else { 1.0 },
                if k == v.current {
                    Color32::from_rgb(0x20, 0x60, 0xd0)
                } else {
                    Color32::from_gray(0x70)
                },
            ),
            egui::StrokeKind::Inside,
        );
        let text = if lit { Color32::WHITE } else { INK };
        g.text(
            pr.center() - vec2(0.0, 6.0),
            Align2::CENTER_CENTER,
            format!("{}", n + 1),
            bold(24.0),
            text,
        );
        let sub = if v.pattern_select {
            let pn = v.pattern_bank * PADS_PER_BANK + n;
            if sampler::pattern_region(p, t, pn).is_some() {
                "Ptn".to_string()
            } else {
                String::new()
            }
        } else if st.pads[k].is_empty() {
            String::new()
        } else {
            st.pads[k].name.chars().take(9).collect()
        };
        g.text(
            pr.center_bottom() - vec2(0.0, 9.0),
            Align2::CENTER_CENTER,
            sub,
            FontId::proportional(8.5),
            text,
        );
        if resp.is_pointer_button_down_on()
            && v.held != Some(k)
            && !v.keys_down.iter().any(|(_, x)| *x == k)
        {
            pad_down(ui, p, uid, t, st, v, k);
        }
        if v.held == Some(k) && !resp.is_pointer_button_down_on() {
            pad_up(uid, st, v, k);
        }
        let _ = resp.on_hover_text(if st.pads[k].is_empty() {
            pad_label(k)
        } else {
            format!("{} — {}", pad_label(k), st.pads[k].name)
        });
    }
    let hold_r = rect(312.0, 498.0, 70.0, 52.0);
    let hold_resp = ui.interact(
        hold_r,
        ui.make_persistent_id(("prowave-sampler-hold", uid)),
        Sense::click(),
    );
    g.rect_filled(hold_r, 4.0, if v.hold { LIT } else { PAD });
    g.text(
        hold_r.center(),
        Align2::CENTER_CENTER,
        "HOLD",
        bold(15.0),
        if v.hold { Color32::WHITE } else { INK },
    );
    if hold_resp
        .on_hover_text(tr(
            "Gate pads stay on after they are let go; press a held pad again to stop it.",
        ))
        .clicked()
    {
        v.hold = !v.hold;
        if !v.hold {
            for k in std::mem::take(&mut v.latched) {
                send(uid, SamplerMsg::Release { pad: k as u8 });
            }
        }
    }
    let ext_r = rect(312.0, 558.0, 70.0, 52.0);
    g.rect_filled(ext_r, 4.0, EXT);
    g.text(
        ext_r.center() - vec2(0.0, 7.0),
        Align2::CENTER_CENTER,
        "EXT",
        bold(13.0),
        Color32::from_gray(0x9a),
    );
    g.text(
        ext_r.center() + vec2(0.0, 8.0),
        Align2::CENTER_CENTER,
        "SOURCE",
        bold(13.0),
        Color32::from_gray(0x9a),
    );
    let _ = ui
        .interact(ext_r, ui.make_persistent_id(("prowave-sampler-ext", uid)), Sense::hover())
        .on_hover_text(tr("Not available: an instrument slot has no audio input. Use the browser to bring sound in."));
    g.text(
        at(170.0, 622.0),
        Align2::CENTER_CENTER,
        "SAMPLE/PATTERN",
        bold(10.0),
        INK,
    );
}

/// A rubber key: pale grey with black print, red with white print when lit.
fn key(
    ui: &Ui,
    g: &egui::Painter,
    r: Rect,
    label: &str,
    lit: bool,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> egui::Response {
    let resp = ui.interact(r, ui.make_persistent_id(id), Sense::click_and_drag());
    let fill = if lit {
        LIT
    } else if resp.is_pointer_button_down_on() {
        Color32::from_rgb(0xa8, 0xa5, 0x90)
    } else {
        KEY
    };
    g.rect_filled(r, 3.0, fill);
    g.rect_stroke(
        r,
        3.0,
        Stroke::new(1.0, Color32::from_gray(0x60)),
        egui::StrokeKind::Inside,
    );
    let lines: Vec<&str> = label.split('\n').collect();
    let size = if lines.len() > 1 { 8.0 } else { 9.0 };
    let color = if lit { Color32::WHITE } else { INK };
    for (i, l) in lines.iter().enumerate() {
        let dy = (i as f32 - (lines.len() as f32 - 1.0) / 2.0) * (size + 1.0);
        g.text(
            r.center() + vec2(0.0, dy),
            Align2::CENTER_CENTER,
            *l,
            FontId::proportional(size),
            color,
        );
    }
    resp
}

/// A knob: black, an off-white cap with a pointer, a scale of ticks. Dragged up or right it
/// turns up; double-click returns it to the middle. Returns whether it moved.
fn knob(
    ui: &Ui,
    c: Pos2,
    radius: f32,
    value: &mut f32,
    ticks: Color32,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> bool {
    let g = ui.painter();
    let r = Rect::from_center_size(c, vec2(radius * 2.4, radius * 2.4));
    let resp = ui.interact(r, ui.make_persistent_id(id), Sense::click_and_drag());
    let start = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    for i in 0..=10 {
        let a = start + sweep * i as f32 / 10.0;
        let (s, co) = a.sin_cos();
        g.line_segment(
            [
                c + vec2(co, s) * (radius + 3.0),
                c + vec2(co, s) * (radius + 7.0),
            ],
            Stroke::new(1.5, ticks),
        );
    }
    g.circle_filled(c, radius, Color32::from_gray(0x14));
    g.circle_filled(c, radius * 0.62, CAP);
    let a = start + sweep * value.clamp(0.0, 1.0);
    let (s, co) = a.sin_cos();
    g.line_segment(
        [
            c + vec2(co, s) * radius * 0.15,
            c + vec2(co, s) * radius * 0.6,
        ],
        Stroke::new(2.5, INK),
    );
    let mut changed = false;
    if resp.dragged() {
        let d = resp.drag_delta();
        let nv = (*value + (d.x - d.y) * 0.005).clamp(0.0, 1.0);
        if nv != *value {
            *value = nv;
            changed = true;
        }
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.1 {
            *value = (*value + scroll * 0.002).clamp(0.0, 1.0);
            changed = true;
        }
    }
    if resp.double_clicked() {
        *value = 0.5;
        changed = true;
    }
    changed
}

/// The 3-digit red LED display.
fn seven_segment(g: &egui::Painter, r: Rect, text: &str, on: bool) {
    g.rect_filled(r, 3.0, DISPLAY);
    let chars: Vec<char> = text.chars().collect();
    // Right-aligned in three places.
    let mut cells = [' '; 3];
    let n = chars.len().min(3);
    for i in 0..n {
        cells[3 - n + i] = chars[chars.len() - n + i];
    }
    let w = r.width() / 3.0;
    for (i, ch) in cells.iter().enumerate() {
        let cell = Rect::from_min_size(
            r.min + vec2(i as f32 * w + 5.0, 5.0),
            vec2(w - 10.0, r.height() - 10.0),
        );
        let mask = if on { segments(*ch) } else { 0 };
        draw_digit(g, cell, mask);
    }
}

fn segments(c: char) -> u8 {
    match c {
        '0' | 'O' | 'D' => 0x3F,
        '1' | 'I' => 0x06,
        '2' | 'Z' => 0x5B,
        '3' => 0x4F,
        '4' => 0x66,
        '5' | 'S' | 's' => 0x6D,
        '6' => 0x7D,
        '7' => 0x07,
        '8' | 'B' => 0x7F,
        '9' => 0x6F,
        'A' | 'a' => 0x77,
        'b' => 0x7C,
        'C' => 0x39,
        'c' => 0x58,
        'd' => 0x5E,
        'E' | 'e' => 0x79,
        'F' | 'f' => 0x71,
        'G' | 'g' => 0x3D,
        'H' | 'K' | 'k' | 'X' | 'x' => 0x76,
        'h' => 0x74,
        'i' => 0x04,
        'J' | 'j' => 0x1E,
        'L' | 'l' => 0x38,
        'n' | 'N' | 'M' | 'm' => 0x54,
        'o' => 0x5C,
        'P' | 'p' => 0x73,
        'q' | 'Q' => 0x67,
        'r' | 'R' => 0x50,
        't' | 'T' => 0x78,
        'U' | 'V' | 'W' => 0x3E,
        'u' | 'v' | 'w' => 0x1C,
        'y' | 'Y' => 0x6E,
        '-' => 0x40,
        '_' => 0x08,
        _ => 0,
    }
}

fn draw_digit(g: &egui::Painter, r: Rect, mask: u8) {
    let (x0, x1) = (r.left(), r.right());
    let (y0, ym, y1) = (r.top(), r.center().y, r.bottom());
    let segs = [
        [pos2(x0 + 2.0, y0), pos2(x1 - 2.0, y0)],
        [pos2(x1, y0 + 2.0), pos2(x1, ym - 2.0)],
        [pos2(x1, ym + 2.0), pos2(x1, y1 - 2.0)],
        [pos2(x0 + 2.0, y1), pos2(x1 - 2.0, y1)],
        [pos2(x0, ym + 2.0), pos2(x0, y1 - 2.0)],
        [pos2(x0, y0 + 2.0), pos2(x0, ym - 2.0)],
        [pos2(x0 + 2.0, ym), pos2(x1 - 2.0, ym)],
    ];
    for (i, s) in segs.iter().enumerate() {
        let on = mask & (1 << i) != 0;
        g.line_segment(*s, Stroke::new(3.0, if on { LIT } else { LED_OFF }));
    }
}

// ------------------------------------------------------------------ panel behaviour

fn step_mfx(n: usize, step: i32) -> usize {
    let mut x = n as i32;
    for _ in 0..21 {
        x = (x - 1 + step).rem_euclid(21) + 1;
        if SpFxKind::from_mfx(x as usize).is_some_and(|k| k.available()) {
            break;
        }
    }
    x as usize
}

fn pick_mfx(st: &mut SamplerState, v: &mut View, n: usize, time: f64) {
    let Some(kind) = SpFxKind::from_mfx(n).filter(|k| k.available()) else {
        return;
    };
    st.mfx = n;
    v.flash(format!("{n}"), time);
    if st.fx.on && st.fx.kind.mfx_number().is_some() && st.fx.kind != kind {
        st.fx.kind = kind;
        st.fx.ctrl = kind.defaults();
    }
}

/// An effect key: on, off, or (with REMAIN) on for every pad (manual pp. 22–23).
fn effect_key(st: &mut SamplerState, v: &mut View, kind: SpFxKind, time: f64) {
    if v.remain {
        for pad in st.pads.iter_mut().filter(|p| !p.is_empty()) {
            pad.fx = true;
        }
        v.remain = false;
        if st.fx.kind != kind {
            st.fx.ctrl = kind.defaults();
        }
        st.fx.kind = kind;
        st.fx.on = true;
        v.flash("ALL", time);
        return;
    }
    if st.fx.on && st.fx.kind == kind {
        st.fx.on = false;
        v.flash("oFF", time);
    } else {
        if st.fx.kind != kind {
            st.fx.ctrl = kind.defaults();
        }
        st.fx.kind = kind;
        st.fx.on = true;
        // A newly effected sound with no pads set to it: the current pad (manual p. 22).
        if !st.pads.iter().any(|p| p.fx) {
            st.pads[v.current].fx = true;
        }
        v.flash(kind.codes()[0], time);
    }
}

/// What knob `i` does now, for the line under it.
fn knob_role(st: &SamplerState, v: &View, i: usize) -> String {
    match (v.knobs, i) {
        (KnobMode::StartEndLevel, 0) => "start".into(),
        (KnobMode::StartEndLevel, 1) => "end".into(),
        (KnobMode::StartEndLevel, _) => "level".into(),
        (KnobMode::TimeBpm, 0) if !v.pattern_select => "time".into(),
        (KnobMode::TimeBpm, 1) => if v.pattern_select {
            "pattern BPM"
        } else {
            "pad BPM"
        }
        .into(),
        (KnobMode::Length, 2) => "length".into(),
        (KnobMode::Quantize, 2) => "quantize".into(),
        (KnobMode::Effect, _) if st.fx.kind.available() => st.fx.kind.knobs()[i].to_lowercase(),
        _ => String::new(),
    }
}

fn knob_value(p: &Project, st: &SamplerState, v: &View, i: usize) -> f32 {
    let pad = &st.pads[v.current];
    match (v.knobs, i) {
        (KnobMode::StartEndLevel, 2) => f32::from(pad.level) / 127.0,
        (KnobMode::StartEndLevel, i) => v.nudge_knob[i.min(1)],
        (KnobMode::TimeBpm, 0) => match pad.time {
            TimeMode::Off => 0.0,
            TimeMode::Ratio(r) => (0.02 + (r as f32 - 0.5) / 0.8 * 0.95).clamp(0.02, 0.97),
            _ => 1.0,
        },
        (KnobMode::TimeBpm, 1) => {
            let bpm = if v.pattern_select {
                Some(f64::from(p.tempo))
            } else {
                pad.bpm
            };
            bpm.map_or(0.0, |b| ((b as f32 - 40.0) / 160.0).clamp(0.01, 1.0))
        }
        (KnobMode::Length, 2) => (st.pattern_len as f32 - 1.0) / 98.0,
        (KnobMode::Quantize, 2) => {
            Quantize::ALL
                .iter()
                .position(|q| *q == st.quantize)
                .unwrap_or(0) as f32
                / 4.0
        }
        (KnobMode::Effect, _) => st.fx.ctrl[i],
        _ => 0.5,
    }
}

/// The SP-303's tempo steps (manual p. 43): even numbers at the slow and fast ends, and only
/// 180, 183, 186, 190, 193, 196, 200 at the top.
fn bpm_step(x: f64) -> f64 {
    let b = x.round().clamp(40.0, 200.0);
    if b >= 180.0 {
        *[180.0, 183.0, 186.0, 190.0, 193.0, 196.0, 200.0]
            .iter()
            .min_by(|a, c| (*a - b).abs().total_cmp(&(*c - b).abs()))
            .unwrap_or(&180.0)
    } else if b <= 60.0 || b >= 160.0 {
        (b / 2.0).round() * 2.0
    } else {
        b
    }
}

fn set_knob(p: &mut Project, st: &mut SamplerState, v: &mut View, i: usize, value: f32, time: f64) {
    let k = v.current;
    match (v.knobs, i) {
        (KnobMode::StartEndLevel, 2) => {
            st.pads[k].level = (value * 127.0).round() as u8;
            v.flash(format!("{}", st.pads[k].level), time);
        }
        (KnobMode::StartEndLevel, _) => {
            // Centre is no change; a whole turn either way moves about 130 ms; Start and End
            // stay at least 100 ms apart (manual p. 40).
            let Some(src) = st.pads[k].source.and_then(|s| st.sources.get(s)) else {
                return;
            };
            let Some(buf) = sampler::with_runtime(|rt| rt.source(&src.path)) else {
                return;
            };
            let frames = buf.frames() as u64;
            let rate = f64::from(buf.rate);
            v.nudge_knob[i.min(1)] = value;
            let (a, z) = *v
                .nudge
                .get_or_insert(sampler::pad_range(&st.pads[k], frames));
            let shift = (f64::from(value) - 0.5) * 2.0 * 0.13 * rate;
            let gap = (0.1 * rate) as u64;
            if i == 0 {
                let s = (a as f64 + shift).clamp(0.0, z.saturating_sub(gap) as f64) as u64;
                st.pads[k].start = s;
                v.flash(
                    format!("{}", (shift / rate * 1000.0).round() as i64 / 10),
                    time,
                );
            } else {
                let e = (z as f64 + shift).clamp((a + gap) as f64, frames as f64) as u64;
                st.pads[k].end = e;
                v.flash(
                    format!("{}", (shift / rate * 1000.0).round() as i64 / 10),
                    time,
                );
            }
        }
        (KnobMode::TimeBpm, 0) if !v.pattern_select => {
            // Time Modify: left "oFF", ½ to 1.3 times the tempo, right "Ptn" (manual p. 42).
            st.pads[k].time = if value < 0.02 {
                v.flash("oFF", time);
                TimeMode::Off
            } else if value > 0.97 {
                v.flash("Ptn", time);
                TimeMode::Follow
            } else {
                let r = 0.5 + f64::from((value - 0.02) / 0.95) * 0.8;
                v.flash(format!("{}", (r * 100.0).round()), time);
                TimeMode::Ratio(r)
            };
        }
        (KnobMode::TimeBpm, 1) => {
            let bpm = (value > 0.005).then(|| bpm_step(40.0 + f64::from(value) * 160.0));
            if v.pattern_select {
                if let Some(b) = bpm {
                    p.tempo = b as f32;
                    p.song_rev += 1;
                }
            } else {
                st.pads[k].bpm = bpm;
            }
            v.flash(bpm.map_or("---".to_string(), |b| format!("{b:.0}")), time);
        }
        (KnobMode::Length, 2) => {
            // 1–20 bars, then steps of 4 up to 99 (manual p. 45).
            let raw = 1.0 + value * 98.0;
            let bars = if raw <= 20.0 {
                raw.round()
            } else {
                (20.0 + ((raw - 20.0) / 4.0).round() * 4.0).min(99.0)
            };
            st.pattern_len = bars as u32;
            v.flash(format!("{}", st.pattern_len), time);
        }
        (KnobMode::Quantize, 2) => {
            st.quantize = Quantize::ALL[((value * 4.0).round() as usize).min(4)];
            v.flash(st.quantize.code(), time);
        }
        (KnobMode::Effect, _) => {
            st.fx.ctrl[i] = value;
            let shown = if st.fx.kind == SpFxKind::Delay && i == 0 {
                prowave_engine::DELAY_NOTES[prowave_engine::delay_note(value)]
                    .0
                    .to_string()
            } else {
                format!("{}", (value * 127.0).round())
            };
            v.flash(shown, time);
        }
        _ => {}
    }
}

fn display_text(p: &Project, st: &SamplerState, v: &View, time: f64) -> String {
    if let Some((t, until)) = &v.flash {
        if time < *until {
            return t.clone();
        }
    }
    if v.del {
        return "dEL".into();
    }
    if v.mark == 1 || v.mark == 2 {
        return "---".into();
    }
    match v.knobs {
        KnobMode::Length => return format!("{}", st.pattern_len),
        KnobMode::Quantize => return st.quantize.code().into(),
        KnobMode::TimeBpm if !v.pattern_select => {
            return st.pads[v.current]
                .bpm
                .map_or("---".into(), |b| format!("{}", b.round()))
        }
        _ => {}
    }
    if v.pattern_select {
        return "Ptn".into();
    }
    format!("{}", p.tempo.round())
}

/// TAP TEMPO: four taps set the tempo to their quarter notes (manual pp. 31, 43).
fn tap(p: &mut Project, st: &mut SamplerState, v: &mut View, time: f64) {
    v.taps.retain(|t| time - t < 2.5);
    v.taps.push(time);
    if v.taps.len() > 4 {
        v.taps.remove(0);
    }
    if v.taps.len() >= 2 {
        let gaps: Vec<f64> = v.taps.windows(2).map(|w| w[1] - w[0]).collect();
        let bpm = (60.0 / (gaps.iter().sum::<f64>() / gaps.len() as f64)).clamp(40.0, 200.0);
        if v.knobs == KnobMode::TimeBpm && !v.pattern_select {
            st.pads[v.current].bpm = Some((bpm * 10.0).round() / 10.0);
        } else {
            p.tempo = bpm.round() as f32;
            p.song_rev += 1;
        }
        v.flash(format!("{}", bpm.round()), time);
    }
}

/// CANCEL (PATTERN STOP): stops the pattern and the preview, and backs out of DEL and MARK.
fn cancel(p: &mut Project, uid: u64, v: &mut View) {
    if v.rec.is_some() || v.pattern_select {
        p.playing = false;
    }
    v.rec = None;
    v.del = false;
    v.del_target = None;
    v.mark = 0;
    v.remain = false;
    stop_preview(uid, v);
    for k in std::mem::take(&mut v.latched) {
        send(uid, SamplerMsg::Release { pad: k as u8 });
    }
}

/// DEL, pad, DEL deletes the pad's sample (manual p. 33).
fn del_key(st: &mut SamplerState, v: &mut View, time: f64) {
    if !v.del {
        v.del = true;
        v.del_target = None;
        return;
    }
    if let Some(k) = v.del_target.take() {
        st.pads[k] = Pad::default();
        v.flash("---", time);
    }
    v.del = false;
}

/// REC: with PATTERN SELECT, records the pattern (it loops and overdubs, manual pp. 44–45);
/// otherwise "samples" onto the current pad: the editor's selection, or the browser's file.
fn rec_key(p: &mut Project, t: usize, st: &mut SamplerState, v: &mut View, b: &Browser, time: f64) {
    if v.rec.is_some() {
        v.rec = None;
        v.flash("End", time);
        return;
    }
    if v.pattern_select {
        let r = sampler::ensure_pattern(p, t, v.pattern, st.pattern_len);
        let region = &p.tracks[t].regions[r];
        p.cycle_start = p.region_start_beats(region);
        p.cycle_end = p.region_end_beats(region);
        p.cycle = true;
        p.position_beats = p.cycle_start;
        p.playing = true;
        p.selected_track = t;
        v.rec = Some(r);
        v.pattern_select = false;
        v.flash("rEC", time);
        return;
    }
    let path = editor_path(st, v, b);
    match (path, v.selection) {
        (Some(path), Some((a, z))) => {
            let rate =
                sampler::with_runtime(|rt| rt.source(&path).map(|b| b.rate)).unwrap_or(44_100);
            sampler::assign(st, v.current, &path, a, z, rate);
            v.selection = None;
            v.flash("rEC", time);
        }
        _ => {
            if let Some(file) = b.selected.clone() {
                load_file(st, v, &file);
                v.flash("rEC", time);
            } else {
                v.status = Some(tr("Pick a file in the browser, then press REC or double-click it to put it on the pad.").into());
            }
        }
    }
}

/// RE-SAMPLE: the current pad through the effect onto the next empty pad (manual pp. 32–33).
/// The new sound is written as a file with the session's recordings.
fn resample_key(st: &mut SamplerState, v: &mut View, p: &Project, time: f64) {
    let k = v.current;
    let Some(src) = st.pads[k].source.and_then(|s| st.sources.get(s)).cloned() else {
        return;
    };
    let Some(dest) = (0..PADS)
        .map(|i| (k + 1 + i) % PADS)
        .find(|i| st.pads[*i].is_empty())
    else {
        v.status = Some(tr("No empty pad to resample onto.").into());
        return;
    };
    let bpm = sampler::project_bpm(p);
    let bar = sampler::beats_per_bar(p);
    let made = sampler::with_runtime(|rt| {
        let data = rt.source(&src.path)?;
        let spec = sampler::render_spec(st, k, data.frames() as u64, data.rate, bpm, bar)?;
        Some(sampler::render(&data, &spec))
    });
    let Some(sound) = made else {
        v.status = Some(tr("The pad's sound is not ready yet.").into());
        return;
    };
    let mut fx = st.fx;
    fx.bpm = bpm as f32;
    // The pad's own pitch (Repitch) is printed in too: resampled at its speed.
    let pitched = sampler::pad_params(&st.pads[k], 1.0).pitch;
    let sound = if pitched.abs() > 1e-4 {
        sample_tools::stretch(&sound, 2f64.powf(-f64::from(pitched) / 12.0), pitched)
    } else {
        sound
    };
    let out = if fx.on && st.pads[k].fx {
        sample_tools::resample(&sound, fx, 1.0, 2.0)
    } else {
        sound
    };
    let dir = crate::project_io::recordings_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = dir.join(format!("resample-{}-{stamp}.wav", pad_label(dest)));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|_| prowave_engine::write_wav(&path, &out.data, out.rate));
    match written {
        Ok(()) => {
            let key = path.to_string_lossy().into_owned();
            sampler::assign(st, dest, &key, 0, out.frames() as u64, out.rate);
            st.pads[dest].name = format!("{} rs", st.pads[k].name);
            v.flash(pad_label(dest), time);
            v.status = Some(trf("Resampled onto pad {}", &[&pad_label(dest)]));
        }
        Err(e) => v.status = Some(e.to_string()),
    }
}

/// MARK while the pad's file plays: first press plays it, the next sets the Start, the next
/// the End (manual pp. 38–39). With a BPM set, the End lands on a whole beat from the Start.
fn mark_key(uid: u64, st: &mut SamplerState, v: &mut View, time: f64) {
    let k = v.current;
    let Some(path) = st.pads[k]
        .source
        .and_then(|s| st.sources.get(s))
        .map(|s| s.path.clone())
    else {
        return;
    };
    let Some(buf) = sampler::with_runtime(|rt| rt.source(&path)) else {
        return;
    };
    let frames = buf.frames() as u64;
    match v.mark {
        0 | 3 => {
            let from = sampler::pad_range(&st.pads[k], frames).0;
            audition(uid, v, &path, from, frames);
            v.mark = 1;
        }
        1 => {
            v.mark_start = preview_frame(uid, v).unwrap_or(0);
            v.mark = 2;
        }
        _ => {
            let mut end = preview_frame(uid, v)
                .unwrap_or(frames)
                .max(v.mark_start + 1);
            if let Some(bpm) = st.pads[k].bpm {
                let beat = 60.0 / bpm * f64::from(buf.rate);
                let beats = ((end - v.mark_start) as f64 / beat).round().max(1.0);
                end = (v.mark_start + (beats * beat) as u64).min(frames);
            }
            st.pads[k].start = v.mark_start;
            st.pads[k].end = end;
            stop_preview(uid, v);
            v.mark = 3;
            v.flash("---", time);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::model::TrackKind;
    use crate::ui_test::Harness;

    fn project_with_sampler() -> (Project, u64) {
        let mut p = Project::empty();
        let t = p.add_track(TrackKind::Instrument);
        let slot = sampler::new_slot();
        let uid = slot.uid;
        p.tracks[t].au_instrument = Some(slot);
        (p, uid)
    }

    #[test]
    fn the_sp303_tempo_steps() {
        assert_eq!(bpm_step(53.0), 54.0);
        assert_eq!(bpm_step(95.0), 95.0);
        assert_eq!(bpm_step(171.0), 172.0);
        assert_eq!(bpm_step(188.0), 186.0);
        assert_eq!(bpm_step(250.0), 200.0);
    }

    #[test]
    fn effect_keys_switch_one_effect_and_remain_sets_every_pad() {
        let mut st = SamplerState::default();
        let mut v = View::default();
        effect_key(&mut st, &mut v, SpFxKind::VinylSim, 0.0);
        assert!(st.fx.on && st.fx.kind == SpFxKind::VinylSim);
        assert!(st.pads[0].fx, "the current pad takes the effect");
        effect_key(&mut st, &mut v, SpFxKind::Delay, 0.0);
        assert_eq!(st.fx.kind, SpFxKind::Delay);
        effect_key(&mut st, &mut v, SpFxKind::Delay, 0.0);
        assert!(!st.fx.on);
        sampler::assign(&mut st, 5, "/a.wav", 0, 10, 44_100);
        v.remain = true;
        effect_key(&mut st, &mut v, SpFxKind::Isolator, 0.0);
        assert!(st.pads[5].fx && st.fx.on);
        assert_eq!(step_mfx(16, 1), 18, "SLICER is skipped");
        assert_eq!(step_mfx(21, 1), 1);
    }

    #[test]
    fn the_display_shows_letters_and_numbers() {
        assert_eq!(segments('8'), 0x7F);
        assert_eq!(segments('P'), 0x73);
        assert_eq!(segments('-'), 0x40);
    }

    #[test]
    fn the_window_draws_and_a_pad_click_selects_the_pad() {
        let (mut p, uid) = project_with_sampler();
        let mut v = View::default();
        let mut b = Browser::default();
        let mut h = Harness::new(1300.0, 700.0);
        h.settle(|ui| window(ui, &mut p, uid, &mut v, &mut b));
        // Pad 6 of bank A: second column, second row.
        let pad6 = v.panel.min + vec2(22.0 + 72.0 + 32.0, 498.0 + 60.0 + 26.0);
        h.click(pad6, |ui| window(ui, &mut p, uid, &mut v, &mut b));
        assert_eq!(v.current, 5, "pad 6 is the current pad");
    }

    /// A folder with one song that has a pad set and stems, and one that has neither.
    fn chops_folder(dir: &Path) -> Browser {
        let songs = dir.join("songs");
        std::fs::create_dir_all(&songs).unwrap();
        let ready = songs.join("Ready Song.mp3");
        let plain = songs.join("Plain Song.mp3");
        for f in [&ready, &plain] {
            std::fs::write(f, b"x").unwrap();
        }
        let (chops, stems) = (dir.join("chops"), dir.join("stems"));
        let mut set = chop_set::PadSet {
            song: "Ready Song".into(),
            bpm: Some(100.0),
            pads: Vec::new(),
        };
        for (name, looping) in [
            ("Full Track", false),
            ("Instrumental Chorus 8b", true),
            ("Vocal Chorus 8b", true),
        ] {
            set.pads.push(chop_set::SetPad {
                name: name.into(),
                file: if looping {
                    chops.join("Ready Song").join(format!("{name}.flac"))
                } else {
                    ready.clone()
                }
                .to_string_lossy()
                .into_owned(),
                start: 0,
                end: 0,
                gate: !looping,
                looping,
                bpm: Some(100.0),
                bars: looping.then_some(8.0),
                confidence: Some(0.9),
            });
        }
        chop_set::write(&set, &chop_set::set_path(&chops, &ready).unwrap()).unwrap();
        let sd = stems.join("Ready Song");
        std::fs::create_dir_all(&sd).unwrap();
        for f in ["drums.flac", "vocals.flac"] {
            std::fs::write(sd.join(f), b"x").unwrap();
        }
        let mut b = Browser {
            scanned: true,
            roots: Some((Some(chops), Some(stems))),
            ..Browser::default()
        };
        b.files = vec![plain, ready];
        b
    }

    fn row(b: &Browser, path: &Path) -> Pos2 {
        b.rows
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, r)| r.center())
            .expect("the row is drawn")
    }

    #[test]
    fn one_click_on_a_song_puts_its_chops_on_the_pads_and_previews_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut p, uid) = project_with_sampler();
        let mut v = View::default();
        let mut b = chops_folder(dir.path());
        let ready = b.files[1].clone();
        let mut h = Harness::new(1300.0, 700.0);
        h.settle(|ui| window(ui, &mut p, uid, &mut v, &mut b));
        h.click(row(&b, &ready), |ui| {
            window(ui, &mut p, uid, &mut v, &mut b)
        });
        let st = sampler::state_of(&p, uid).unwrap();
        let names: Vec<&str> = st.pads[..3].iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            ["Full Track", "Instrumental Chorus 8b", "Vocal Chorus 8b"]
        );
        assert!(st.pads[0].gate && !st.pads[0].looping);
        assert!(st.pads[1].looping && st.pads[1].bpm == Some(100.0));
        assert!(st.pads[3..].iter().all(Pad::is_empty));
        assert_eq!(b.selected.as_ref(), Some(&ready));
        assert_eq!(
            v.preview.as_ref().map(|x| x.path.clone()),
            Some(ready.to_string_lossy().into_owned()),
            "the song previews as before"
        );
        // Its stems are listed under it now that it is selected, and each previews.
        h.settle(|ui| window(ui, &mut p, uid, &mut v, &mut b));
        let at = b.rows.iter().position(|(x, _)| *x == ready).unwrap();
        let under: Vec<String> = b.rows[at + 1..at + 3]
            .iter()
            .map(|(x, _)| x.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(under, ["vocals.flac", "drums.flac"]);
        let drums = b.rows[at + 2].0.clone();
        h.time_passes(1.0);
        h.click(row(&b, &drums), |ui| {
            window(ui, &mut p, uid, &mut v, &mut b)
        });
        assert_eq!(
            v.preview.as_ref().map(|x| x.path.clone()),
            Some(drums.to_string_lossy().into_owned())
        );
        assert_eq!(
            sampler::state_of(&p, uid).unwrap().pads[1].name,
            "Instrumental Chorus 8b",
            "a stem's click does not touch the pads"
        );
    }

    #[test]
    fn a_song_without_chops_previews_and_loads_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let (mut p, uid) = project_with_sampler();
        let mut v = View::default();
        let mut b = chops_folder(dir.path());
        let plain = b.files[0].clone();
        let mut h = Harness::new(1300.0, 700.0);
        h.settle(|ui| window(ui, &mut p, uid, &mut v, &mut b));
        assert_eq!(b.sets.get(&plain), Some(&None), "chops not ready");
        h.click(row(&b, &plain), |ui| {
            window(ui, &mut p, uid, &mut v, &mut b)
        });
        assert!(sampler::state_of(&p, uid)
            .unwrap()
            .pads
            .iter()
            .all(Pad::is_empty));
        assert!(v.preview.is_some());
        // A double-click puts the whole file on the current pad.
        h.time_passes(1.0);
        let at = row(&b, &plain);
        h.click(at, |ui| window(ui, &mut p, uid, &mut v, &mut b));
        h.click(at, |ui| window(ui, &mut p, uid, &mut v, &mut b));
        let st = sampler::state_of(&p, uid).unwrap();
        assert_eq!(st.pads[0].name, "Plain Song");
        assert!(st.pads[1].is_empty());
    }

    #[test]
    fn chops_picked_while_the_song_plays_go_on_at_the_next_bar() {
        let dir = tempfile::tempdir().unwrap();
        let (mut p, uid) = project_with_sampler();
        let mut v = View::default();
        let mut b = chops_folder(dir.path());
        let ready = b.files[1].clone();
        p.playing = true;
        p.position_beats = 5.5;
        let mut h = Harness::new(1300.0, 700.0);
        h.settle(|ui| window(ui, &mut p, uid, &mut v, &mut b));
        h.click(row(&b, &ready), |ui| {
            window(ui, &mut p, uid, &mut v, &mut b)
        });
        assert!(sampler::state_of(&p, uid).unwrap().pads[0].is_empty());
        assert_eq!(v.pending.as_ref().map(|x| x.at), Some(8.0));
        p.position_beats = 7.9;
        h.settle(|ui| window(ui, &mut p, uid, &mut v, &mut b));
        assert!(sampler::state_of(&p, uid).unwrap().pads[0].is_empty());
        p.position_beats = 8.02;
        h.settle(|ui| window(ui, &mut p, uid, &mut v, &mut b));
        assert!(v.pending.is_none());
        assert_eq!(
            sampler::state_of(&p, uid).unwrap().pads[2].name,
            "Vocal Chorus 8b"
        );
    }

    #[test]
    fn the_pads_save_as_a_songs_chops() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = chops_folder(dir.path());
        let plain = b.files[0].clone();
        let mut st = SamplerState::default();
        sampler::assign(&mut st, 0, "/x/a.wav", 10, 500, 44_100);
        st.pads[0].looping = true;
        assert_eq!(b.set_of(&plain), None);
        save_set(&mut b, &st, &plain);
        let set = b.set_of(&plain).expect("saved");
        assert_eq!(set.pads.len(), 1);
        assert_eq!((set.pads[0].start, set.pads[0].end), (10, 500));
        assert!(set.pads[0].looping);
    }

    #[test]
    fn picture_of_the_window() {
        let (mut p, uid) = project_with_sampler();
        // A drum loop in the decoded cache (no file needed): four hits a second.
        let mut data = vec![0.0f32; 88_200 * 2];
        for hit in 0..8 {
            for i in 0..4000 {
                let x = (i as f32 * 0.21).sin() * (-(i as f32) / 900.0).exp() * 0.9;
                data[(hit * 11_025 + i) * 2] = x;
                data[(hit * 11_025 + i) * 2 + 1] = x;
            }
        }
        let loop_buf = Arc::new(SampleBuf::new(44_100, data));
        // The runtime is shared with tests running at the same time, which may let go of it.
        let put = move || {
            sampler::with_runtime(|rt| {
                rt.decoded.insert(
                    "/records/break.wav".into(),
                    sampler::Decoded::Ready(loop_buf.clone()),
                )
            });
        };
        put();
        let mut st = sampler::state_of(&p, uid).unwrap();
        sampler::assign(&mut st, 0, "/records/break.wav", 0, 88_200, 44_100);
        st.sources[0].chops = vec![22_050, 44_100, 66_150];
        st.pads[0].name = "Funky Break".into();
        st.fx.on = true;
        st.fx.kind = SpFxKind::VinylSim;
        sampler::store(&mut p, uid, &st);
        let mut v = View::default();
        let mut b = Browser {
            scanned: true,
            ..Browser::default()
        };
        b.files = vec![
            PathBuf::from("/x/First Artist - A Long Compilation Title - 01 Opening Song.mp3"),
            PathBuf::from("/x/Second Artist - A Long Compilation Title - 06 Another Song.mp3"),
        ];
        b.selected = Some(b.files[0].clone());
        let out = std::env::temp_dir().join("prowave-sampler-window.png");
        crate::ui_test::render_png(
            1340.0,
            700.0,
            1.0,
            Color32::from_gray(0x2a),
            |ui| {
                put();
                window(ui, &mut p, uid, &mut v, &mut b)
            },
            &out,
        )
        .unwrap();
        assert!(out.exists());
    }
}
