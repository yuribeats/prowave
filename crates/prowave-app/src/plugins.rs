//! The Audio Units on a track (Logic: the Instrument slot and the Audio FX slots), and the
//! running instances behind them.
//!
//! A project names each plug-in by its Audio Unit id and keeps its settings. The host opens
//! the instances on the UI thread (some take a second), restores their settings and hands
//! them to the engine; their windows and settings stay reachable through handles.
use crate::model::Project;

/// One plug-in in a track's slot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginSlot {
    /// The Audio Unit, `type:subtype:manufacturer`.
    pub id: String,
    /// "Manufacturer: Name", for the slot.
    pub name: String,
    /// Its settings as the plug-in gave them (a binary property list).
    pub state: Option<Vec<u8>>,
    /// Bypassed (Logic: the slot's power button): not sent to the engine.
    pub bypass: bool,
    /// Tells this slot's instance apart while the program runs; not saved.
    pub uid: u64,
}

/// Settings bytes as a project stores them (base64), and back.
pub fn encode_state(state: &Option<Vec<u8>>) -> String {
    use base64::Engine as _;
    state
        .as_ref()
        .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
        .unwrap_or_default()
}

pub fn decode_state(text: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    (!text.is_empty())
        .then(|| base64::engine::general_purpose::STANDARD.decode(text).ok())
        .flatten()
}

// ---------------------------------------------------------------- crash quarantine
//
// Plug-ins run in this process, so one that crashes while it starts takes Pro Wave with it.
// Before a plug-in is opened its id is written down, and removed once it is open; if the file
// is still there at the next start, that plug-in crashed the program and is not opened
// automatically again (a project that holds it would otherwise crash on every open). Picking
// it again in a slot lets it try once more.

fn setting(name: &str) -> Option<std::path::PathBuf> {
    crate::recent_files::config_dir().map(|d| d.join(name))
}

static QUARANTINE: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

/// The plug-ins that crashed Pro Wave and are kept off.
pub fn quarantined() -> Vec<String> {
    let Ok(mut q) = QUARANTINE.lock() else {
        return Vec::new();
    };
    q.get_or_insert_with(|| {
        setting("plugin-quarantine")
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|t| {
                t.lines()
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    })
    .clone()
}

fn save_quarantine(ids: Vec<String>) {
    if let Ok(mut q) = QUARANTINE.lock() {
        *q = Some(ids.clone());
    }
    if cfg!(test) {
        return;
    }
    if let Some(f) = setting("plugin-quarantine") {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(f, ids.join("\n")));
        }
    }
}

/// Keeps plug-in (or VST3 bundle) `id` off from now on.
pub fn quarantine(id: &str) {
    let mut ids = quarantined();
    if !ids.iter().any(|q| q == id) {
        ids.push(id.to_string());
        save_quarantine(ids);
    }
}

/// Lets plug-in `id` be opened again (the user picked it in a slot).
pub fn release(id: &str) {
    let mut ids = quarantined();
    if ids.iter().any(|q| q == id) {
        ids.retain(|q| q != id);
        save_quarantine(ids);
    }
}

/// At start: a plug-in that was being opened when the program last ended crashed it. It is
/// kept off from now on; its name is returned for the status line.
pub fn check_last_crash() -> Option<String> {
    let f = setting("plugin-opening")?;
    let id = std::fs::read_to_string(&f).ok()?;
    let _ = std::fs::remove_file(&f);
    let id = id.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let mut ids = quarantined();
    if !ids.contains(&id) {
        ids.push(id.clone());
        save_quarantine(ids);
    }
    Some(id)
}

/// Marks plug-in `id` as being opened (`None`: done).
fn opening(id: Option<&str>) {
    if cfg!(test) {
        return;
    }
    let Some(f) = setting("plugin-opening") else {
        return;
    };
    match id {
        Some(id) => {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&f, id));
            }
        }
        None => {
            let _ = std::fs::remove_file(&f);
        }
    }
}

static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A fresh [`PluginSlot::uid`].
pub fn new_uid() -> u64 {
    NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl PluginSlot {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            state: None,
            bypass: false,
            uid: new_uid(),
        }
    }
}

/// An Audio Unit a slot's menu offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuChoice {
    pub id: String,
    pub manufacturer: String,
    pub name: String,
    /// An instrument (the Instrument slot); otherwise an effect (the Audio FX slots).
    pub instrument: bool,
    /// The same plug-in as a VST3 when `id` is its Audio Unit (the menu offers both).
    pub alt_id: Option<String>,
}

impl AuChoice {
    pub fn label(&self) -> String {
        format!("{}: {}", self.manufacturer, self.name)
    }

    /// The id to load: the VST3 when it exists and VST3 is preferred, else `id`.
    pub fn preferred_id(&self, prefer_vst3: bool) -> &str {
        match &self.alt_id {
            Some(alt) if prefer_vst3 => alt,
            _ => &self.id,
        }
    }
}

/// The format of the plug-in with this id, for tooltips: "AU", "VST3" or "" (Pro Wave's
/// own).
pub fn format_label(id: &str) -> &'static str {
    if id.starts_with("vst3:") {
        "VST3"
    } else if is_au_id(id) {
        "AU"
    } else {
        ""
    }
}

/// `type:subtype:manufacturer`, three four-character codes.
pub fn is_au_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split(':').collect();
    parts.len() == 3 && parts.iter().all(|p| p.len() == 4)
}

/// Whether menus load the VST3 of a plug-in that is installed as both (else the AU).
pub fn prefer_vst3() -> bool {
    setting("plugin-format")
        .and_then(|f| std::fs::read_to_string(f).ok())
        .is_some_and(|t| t.trim() == "vst3")
}

pub fn set_prefer_vst3(on: bool) {
    if cfg!(test) {
        return;
    }
    if let Some(f) = setting("plugin-format") {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir)
                .and_then(|_| std::fs::write(f, if on { "vst3" } else { "au" }));
        }
    }
}

/// A maker or plug-in name reduced for matching: lower case, letters and digits, without
/// company suffixes ("FabFilter" = "Fabfilter", "Valhalla DSP, LLC" = "Valhalla DSP").
fn match_key(s: &str) -> String {
    const SUFFIXES: [&str; 10] = [
        "gmbh", "inc", "llc", "ltd", "limited", "corp", "co", "ag", "bv", "sa",
    ];
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .filter(|w| !SUFFIXES.contains(&w.as_str()))
        .collect::<Vec<_>>()
        .join("")
}

/// The Audio Units and the VST3s as one menu list. A plug-in installed in both formats is
/// one entry (its AU, with the VST3 as `alt_id`); a VST3 whose maker is spelled differently
/// from its AUs ("Soundtoys" and "SoundToys") takes the AUs' spelling, so each maker is one
/// submenu.
pub fn merge_choices(aus: Vec<AuChoice>, vst3s: Vec<AuChoice>) -> Vec<AuChoice> {
    let mut out = aus;
    let au_count = out.len();
    for v in vst3s {
        let (maker, name) = (match_key(&v.manufacturer), match_key(&v.name));
        let twin = out[..au_count].iter().position(|a| {
            a.alt_id.is_none()
                && a.instrument == v.instrument
                && match_key(&a.manufacturer) == maker
                && match_key(&a.name) == name
        });
        if let Some(i) = twin {
            out[i].alt_id = Some(v.id);
            continue;
        }
        // The same VST3 class twice (two copies of a bundle): once.
        if out[au_count..].iter().any(|o| o.id == v.id) {
            continue;
        }
        let spelled = out[..au_count]
            .iter()
            .find(|a| match_key(&a.manufacturer) == maker)
            .map(|a| a.manufacturer.clone());
        out.push(AuChoice {
            manufacturer: spelled.unwrap_or(v.manufacturer),
            ..v
        });
    }
    out
}

/// The Audio Units and VST3s on this Mac, for the slots' menus (none elsewhere). The VST3s
/// are those the last scan found (see `vst3_scan.rs`).
pub fn choices() -> Vec<AuChoice> {
    #[cfg(target_os = "macos")]
    {
        let aus = prowave_engine::au::list()
            .into_iter()
            .map(|i| AuChoice {
                instrument: i.kind == prowave_engine::au::AuKind::Instrument,
                id: i.id,
                manufacturer: i.manufacturer,
                name: i.name,
                alt_id: None,
            })
            .collect();
        let vst3s = crate::vst3_scan::known()
            .into_iter()
            .map(|e| AuChoice {
                id: e.id,
                manufacturer: e.vendor,
                name: e.name,
                instrument: e.instrument,
                alt_id: None,
            })
            .collect();
        merge_choices(aus, vst3s)
    }
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

/// Whether a built-in instrument is made again when its settings change (the Channel Rack).
fn remade_on_change(s: &PluginSlot) -> bool {
    crate::channel_rack::is_rack(s)
}

/// What a track's plug-ins are, as far as the engine is concerned: the uids, in order,
/// instrument first (0 for none).
fn key_of(p: &Project, i: usize) -> Vec<u64> {
    let Some(t) = p.tracks.get(i) else {
        return Vec::new();
    };
    let ident = |s: &PluginSlot| {
        if remade_on_change(s) {
            let mut h = 0xcbf2_9ce4_8422_2325u64 ^ s.uid;
            for b in s.state.iter().flatten() {
                h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            h.max(1)
        } else {
            s.uid
        }
    };
    let mut key = vec![t.au_instrument.as_ref().map_or(0, ident)];
    key.extend(t.au_effects.iter().filter(|s| !s.bypass).map(|s| s.uid));
    if key.len() == 1 && key[0] == 0 {
        key.clear();
    }
    key
}

/// What the Stereo Out's plug-ins are, as far as the engine is concerned: the uids of those
/// that are not bypassed, in order.
fn master_key(p: &Project) -> Vec<u64> {
    p.master_au_effects
        .iter()
        .filter(|s| !s.bypass)
        .map(|s| s.uid)
        .collect()
}

/// The strip (a track, or `mixer::MASTER_STRIP`) and plug-in slot (0 the instrument, `k + 1`
/// Audio FX `k`) of slot `uid`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn slot_of(p: &Project, uid: u64) -> Option<(usize, u8)> {
    let instrument = p
        .tracks
        .iter()
        .position(|t| t.au_instrument.as_ref().is_some_and(|s| s.uid == uid));
    if let Some(ti) = instrument {
        return Some((ti, 0));
    }
    p.find_au_effect(uid).map(|(i, k)| (i, k as u8 + 1))
}

/// The plug-ins Low Latency Mode passes over on the master bus: bit `k` for running effect `k`
/// when it delays the sound more than `limit_s` seconds and `live` (a track is played live).
pub fn low_latency_mask(latencies: &[f64], live: bool, limit_s: f64) -> u64 {
    if !live {
        return 0;
    }
    latencies
        .iter()
        .take(64)
        .enumerate()
        .filter(|(_, l)| **l > limit_s)
        .fold(0, |m, (k, _)| m | 1 << k)
}

/// The running plug-ins and their windows (macOS).
/// The name of plug-in `slot` of track `track` as the engine numbers them: 0 the instrument,
/// `k + 1` the `k`-th Audio FX that is not bypassed (the order the track's plug-ins are made
/// in). A slot whose plug-in failed to open shifts the ones after it; the name is then a guess.
pub fn slot_name(p: &Project, track: usize, slot: u8) -> String {
    // The Stereo Out's plug-ins (effects only, numbered like a track's).
    if track == usize::from(prowave_engine::MASTER_TRACK) {
        let out = crate::i18n::tr("Stereo Out");
        return match p
            .master_au_effects
            .iter()
            .filter(|s| !s.bypass)
            .nth(usize::from(slot).saturating_sub(1))
        {
            Some(s) if slot > 0 => format!("{} ({out})", s.name),
            _ => out.to_string(),
        };
    }
    let Some(t) = p.tracks.get(track) else {
        return String::new();
    };
    let name = if slot == 0 {
        t.au_instrument.as_ref().map(|s| s.name.clone())
    } else {
        t.au_effects
            .iter()
            .filter(|s| !s.bypass)
            .nth(usize::from(slot) - 1)
            .map(|s| s.name.clone())
    };
    match name {
        Some(n) => format!("{n} ({})", t.name),
        None => t.name.clone(),
    }
}

/// Where the host sends a track's plug-ins: the engine (a stand-in in tests, which have no
/// audio device).
pub trait PluginSink {
    fn sample_rate(&self) -> u32;
    /// False when the command could not be queued (it is tried again next frame).
    fn send(&mut self, cmd: prowave_engine::Command) -> bool;
    /// Whether a plug-in command sent now would be taken. A refused command is freed with
    /// its plug-ins, so none is opened for one.
    fn can_replace_state(&mut self) -> bool;
}

impl PluginSink for prowave_engine::Engine {
    fn sample_rate(&self) -> u32 {
        prowave_engine::Engine::sample_rate(self)
    }
    fn send(&mut self, cmd: prowave_engine::Command) -> bool {
        prowave_engine::Engine::send(self, cmd)
    }
    fn can_replace_state(&mut self) -> bool {
        prowave_engine::Engine::can_replace_state(self)
    }
}

/// Every plug-in slot of the project: the tracks' instruments and Audio FX, and the Stereo
/// Out's.
pub fn slots_mut(p: &mut Project) -> impl Iterator<Item = &mut PluginSlot> {
    p.tracks
        .iter_mut()
        .flat_map(|t| t.au_instrument.iter_mut().chain(t.au_effects.iter_mut()))
        .chain(p.master_au_effects.iter_mut())
}

/// How often a caller that asks every frame has the plug-ins' settings read.
const POLL_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// [`PluginHost::sync`], and the settings of the instances it took away are put into the Undo
/// steps that hold their slots: undoing a deleted track or a removed plug-in brings the plug-in
/// back as the user last had it, not as it was last saved.
pub fn sync_with_history(
    host: &mut PluginHost,
    p: &mut Project,
    history: &mut crate::undo::History,
    engine: &mut impl PluginSink,
    max_frames: usize,
) -> Vec<String> {
    let failed = host.sync(p, engine, max_frames);
    let retired = std::mem::take(&mut host.retired);
    if !retired.is_empty() {
        history.keep_plugin_settings(&retired);
    }
    failed
}

#[derive(Default)]
pub struct PluginHost {
    /// Per engine track: the key last sent.
    sent: Vec<Vec<u64>>,
    /// By slot uid: the settings each running plug-in had when they were last the same as its
    /// slot's in the project (it was opened with them, or they were read into the project).
    /// Settings that differ from these were changed in the plug-in since.
    read: std::collections::HashMap<u64, Vec<u8>>,
    /// The last settings of the instances taken away since [`sync_with_history`] last looked.
    retired: Vec<(u64, Vec<u8>)>,
    /// When [`PluginHost::settings_changed_polled`] last read the settings, and its answer.
    polled: std::cell::Cell<Option<(std::time::Instant, bool)>>,
    /// Stand-ins for running plug-ins, by slot uid.
    #[cfg(test)]
    fakes: std::collections::HashMap<u64, tests::FakePlugin>,
    #[cfg(target_os = "macos")]
    handles: std::collections::HashMap<u64, prowave_engine::plugin_handle::PluginHandle>,
    #[cfg(target_os = "macos")]
    windows: std::collections::HashMap<u64, prowave_engine::plugin_handle::PluginWindow>,
    /// What each running Audio Unit's window changes by hand (for automation), by slot uid.
    #[cfg(target_os = "macos")]
    watches: std::collections::HashMap<u64, prowave_engine::au::AuParamWatch>,
    /// The host's frame of each open plug-in (its rack unit), by slot uid.
    #[cfg(target_os = "macos")]
    units: std::collections::HashMap<u64, crate::plugin_unit::UnitState>,
    /// The parameters of each running Audio Unit and VST3, by slot uid (the automation
    /// choosers).
    params: std::collections::HashMap<u64, Vec<crate::automation::AuParamInfo>>,
    /// Plug-in delay of each track in seconds (without the block of buffering).
    latency: Vec<f64>,
    /// Whether each track's running plug-ins have an instrument and any effects.
    shape: Vec<(bool, bool)>,
    /// The Stereo Out's: the key last sent, the delay of each running effect in seconds, and
    /// the Low Latency Mode mask last sent (`None`: not since the plug-ins were made).
    master_sent: Vec<u64>,
    master_latency: Vec<f64>,
    master_off_sent: Option<u64>,
    /// The plug-ins opened for the command being made. They are reachable (`handles`,
    /// `watches`, `params`) only once the engine has taken the command: a refused command is
    /// freed with its plug-ins, and nothing here may point at them then.
    #[cfg(target_os = "macos")]
    opened: Vec<Opened>,
    /// A change is waiting for the engine to have room for it: try again next frame.
    waiting: bool,
    /// Tracks (and the Stereo Out) whose old instances are retired here while the engine has
    /// not taken the command that replaces them yet. They are not retired a second time: a
    /// slot they held may run a new instance on another track by then.
    retired_tracks: std::collections::HashSet<usize>,
    master_retired: bool,
}

/// A plug-in just opened, not yet the engine's.
#[cfg(target_os = "macos")]
struct Opened {
    uid: u64,
    handle: prowave_engine::plugin_handle::PluginHandle,
    params: Vec<crate::automation::AuParamInfo>,
}

impl PluginHost {
    /// The engine took the command: the plug-ins opened for it can be reached from now on.
    /// An Audio Unit's window is watched from here on too (the watch must go before its unit,
    /// so there is none while the unit could still be freed with a refused command).
    fn register_opened(&mut self) {
        #[cfg(target_os = "macos")]
        for o in std::mem::take(&mut self.opened) {
            if let prowave_engine::plugin_handle::PluginHandle::Au(h) = &o.handle {
                let ids: Vec<u32> = o.params.iter().map(|a| a.id).collect();
                if let Some(w) = prowave_engine::au::AuParamWatch::new(*h, &ids) {
                    self.watches.insert(o.uid, w);
                }
            }
            self.params.insert(o.uid, o.params);
            self.handles.insert(o.uid, o.handle);
        }
    }

    /// The plug-ins opened for a command are not the engine's (it refused the command and
    /// they are gone, or they were opened for something else): nothing here points at them.
    fn forget_opened(&mut self) {
        #[cfg(target_os = "macos")]
        self.opened.clear();
    }

    /// Whether a change could not be sent yet (the engine had no room): [`PluginHost::sync`]
    /// sends it when called again.
    pub fn needs_retry(&self) -> bool {
        self.waiting
    }

    /// Puts the parameters of the running Audio Units where the automation choosers find them.
    pub fn list_parameters(&self, au: &mut crate::automation::AuParams) {
        if au.0.len() != self.params.len() || self.params.keys().any(|k| !au.0.contains_key(k)) {
            au.0 = self.params.clone();
        }
    }

    /// The parameters the user moved in Audio Unit windows since the last call: (track,
    /// plug-in slot (0 the instrument, `k + 1` Audio FX `k`), parameter, value).
    pub fn take_param_moves(&self, p: &Project) -> Vec<(usize, u8, u32, f32)> {
        #[cfg(target_os = "macos")]
        {
            let mut out = Vec::new();
            for (uid, w) in &self.watches {
                let moves = w.take();
                if moves.is_empty() {
                    continue;
                }
                let Some((ti, slot)) = slot_of(p, *uid) else {
                    continue;
                };
                out.extend(moves.into_iter().map(|(id, v)| (ti, slot, id, v)));
            }
            // A VST3's window reports its edits to the host (performEdit).
            for (uid, h) in &self.handles {
                let prowave_engine::plugin_handle::PluginHandle::Vst3(v) = h else {
                    continue;
                };
                let moves = v.take_edits();
                if moves.is_empty() {
                    continue;
                }
                let Some((ti, slot)) = slot_of(p, *uid) else {
                    continue;
                };
                out.extend(
                    moves
                        .into_iter()
                        .map(|(index, value)| (ti, slot, index as u32, value as f32)),
                );
            }
            out
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = p;
            Vec::new()
        }
    }

    /// Once a frame: an open Audio Unit window is told the values its automation plays, so its
    /// knobs follow the envelopes (the engine sets them without telling the window).
    pub fn show_automation(&self, p: &Project, v: &crate::automation::AutoView, beat: f64) {
        #[cfg(target_os = "macos")]
        for uid in self.windows.keys() {
            let (Some((ti, slot)), Some(h)) = (slot_of(p, *uid), self.handles.get(uid)) else {
                continue;
            };
            let Some(auto) = p.strip_auto(ti) else {
                continue;
            };
            for l in auto.plugins.iter().filter(|l| l.slot == slot) {
                let target = prowave_engine::AutoTarget::Plugin {
                    slot,
                    param: l.param,
                };
                let Some(n) = auto.played(target, beat).filter(|_| !l.points.is_empty()) else {
                    continue;
                };
                match h {
                    prowave_engine::plugin_handle::PluginHandle::Au(a) => {
                        a.notify_view(l.param, self.watches.get(uid));
                    }
                    // A VST3 lane holds the parameter's normalized value itself.
                    prowave_engine::plugin_handle::PluginHandle::Vst3(v) => {
                        v.set_parameter_shown(l.param as usize, f64::from(n));
                    }
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (p, v, beat);
        #[cfg(target_os = "macos")]
        let _ = v;
    }

    /// Whether track `i`'s running plug-ins have an instrument, and any effects.
    pub fn shape(&self, i: usize) -> (bool, bool) {
        self.shape.get(i).copied().unwrap_or_default()
    }

    /// Track `i`'s plug-in delay in seconds (without the engine's block of buffering).
    pub fn latency_seconds(&self, i: usize) -> f64 {
        self.latency.get(i).copied().unwrap_or(0.0)
    }

    /// The delay of each running Stereo Out plug-in, in seconds, in chain order.
    pub fn master_latencies(&self) -> &[f64] {
        &self.master_latency
    }

    /// Low Latency Mode on the Stereo Out: while a track is played live (`live`), its plug-ins
    /// that delay the sound more than `limit_s` seconds are passed over, as its slow built-in
    /// effects are bypassed. The engine is told only when that changes.
    pub fn sync_master_low_latency(
        &mut self,
        engine: &mut prowave_engine::Engine,
        live: bool,
        limit_s: f64,
    ) {
        let mask = low_latency_mask(&self.master_latency, live, limit_s);
        if self.master_off_sent == Some(mask) {
            return;
        }
        if engine.send(prowave_engine::Command::SetMasterPluginsOff(mask)) {
            self.master_off_sent = Some(mask);
        }
    }

    /// The settings of the running plug-in in slot `uid`, as it has them now. UI thread (a big
    /// plug-in can take milliseconds).
    fn live_state(&self, uid: u64) -> Option<Vec<u8>> {
        #[cfg(test)]
        if let Some(f) = self.fakes.get(&uid) {
            return Some(f.state());
        }
        #[cfg(target_os = "macos")]
        {
            self.handles.get(&uid).and_then(|h| h.state())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = uid;
            None
        }
    }

    /// Reads every running plug-in's settings into the project, before it is saved.
    pub fn save_states(&mut self, p: &mut Project) {
        self.pull_states(p);
    }

    /// [`PluginHost::save_states`]; returns the slots whose settings were read.
    fn pull_states(&mut self, p: &mut Project) -> Vec<u64> {
        let mut pulled = Vec::new();
        for slot in slots_mut(p) {
            if let Some(state) = self.live_state(slot.uid) {
                self.read.insert(slot.uid, state.clone());
                slot.state = Some(state);
                pulled.push(slot.uid);
            }
        }
        self.polled.set(None);
        pulled
    }

    /// Whether a running plug-in's settings were changed in the plug-in (its window, a preset,
    /// anything it keeps) since they were last the same as the project's. The settings are
    /// read from every running plug-in now: this is for the moment the question is asked, not
    /// for every frame.
    pub fn settings_changed(&self) -> bool {
        self.read
            .iter()
            .any(|(uid, was)| self.live_state(*uid).is_some_and(|now| now != *was))
    }

    /// [`PluginHost::settings_changed`] for a caller that asks every frame: the settings are
    /// read at most twice a second.
    pub fn settings_changed_polled(&self) -> bool {
        if let Some((at, answer)) = self.polled.get() {
            if at.elapsed() < POLL_EVERY {
                return answer;
            }
        }
        let answer = self.settings_changed();
        self.polled.set(Some((std::time::Instant::now(), answer)));
        answer
    }

    /// The instances in slots `gone` go away: their settings as they are now are kept for
    /// [`sync_with_history`] (`pulled`: the slots [`PluginHost::pull_states`] has just read),
    /// and their handles, windows and parameter lists are dropped.
    fn retire(&mut self, gone: &[u64], pulled: &[u64]) {
        for uid in gone {
            let state = if pulled.contains(uid) {
                self.read.get(uid).cloned()
            } else {
                self.live_state(*uid)
            };
            if let Some(state) = state {
                self.retired.push((*uid, state));
            }
            #[cfg(target_os = "macos")]
            {
                self.windows.remove(uid);
                // The watch goes before its unit (the engine frees that later).
                self.watches.remove(uid);
                self.handles.remove(uid);
            }
            #[cfg(test)]
            self.fakes.remove(uid);
            self.params.remove(uid);
            self.read.remove(uid);
        }
    }

    /// The settings the plug-ins in `opened` start with are the project's.
    fn note_opened(&mut self, opened: &[u64]) {
        for uid in opened {
            if let Some(state) = self.live_state(*uid) {
                self.read.insert(*uid, state);
            }
        }
        self.polled.set(None);
    }

    /// Once a frame: closed windows are forgotten (with their unit, when it shows the
    /// plug-in's window), open ones follow their plug-in's size.
    pub fn tend_windows(&mut self) {
        #[cfg(target_os = "macos")]
        {
            for (uid, w) in &self.windows {
                if !w.is_open() && self.units.get(uid).is_some_and(|u| !u.generic) {
                    self.units.remove(uid);
                }
            }
            self.windows.retain(|_, w| w.is_open());
            for w in self.windows.values() {
                w.fit();
            }
        }
    }

    /// Opens (or brings forward) the plug-in in slot `uid`: its rack unit, and its own window
    /// unless the unit shows the host's editor.
    pub fn show_window(&mut self, uid: u64, title: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let unit = self.units.entry(uid).or_default();
            unit.title = title.to_string();
            if unit.generic {
                return Ok(());
            }
            self.open_window(uid, title)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (uid, title);
            Err("plug-ins run on macOS only".into())
        }
    }

    /// Opens (or brings forward) the plug-in's own window.
    fn open_window(&mut self, uid: u64, title: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            self.windows.retain(|_, w| w.is_open());
            if let Some(w) = self.windows.get(&uid) {
                w.show();
                return Ok(());
            }
            let h = self.handles.get(&uid).ok_or("the plug-in is not running")?;
            let w = h.open_window(title)?;
            self.windows.insert(uid, w);
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (uid, title);
            Err("plug-ins run on macOS only".into())
        }
    }

    /// Once a frame: draws the rack unit of each open plug-in and does what its keys ask
    /// (bypass, presets, compare, the view, the host editor's moves).
    pub fn show_units(&mut self, ctx: &egui::Context, p: &mut Project) {
        #[cfg(target_os = "macos")]
        {
            let uids: Vec<u64> = self.units.keys().copied().collect();
            for uid in uids {
                let Some((ti, slot)) = slot_of(p, uid) else {
                    // The plug-in left its slot: its unit goes too.
                    self.units.remove(&uid);
                    self.windows.remove(&uid);
                    continue;
                };
                let handle = self.handles.get(&uid).cloned();
                // A window the engine's rebuild closed comes back (the unit shows it).
                if let (Some(u), Some(_)) = (self.units.get(&uid), &handle) {
                    if !u.generic && !self.windows.contains_key(&uid) {
                        let title = u.title.clone();
                        let _ = self.open_window(uid, &title);
                    }
                }
                let (badge, strip) = crate::fx_ui::strip_plate(p, ti);
                let found = if slot == 0 {
                    p.tracks.get(ti).and_then(|t| t.au_instrument.as_ref())
                } else {
                    p.strip_plugins(ti)
                        .and_then(|fx| fx.get(usize::from(slot) - 1))
                };
                let Some(found) = found else { continue };
                let name = found.name.clone();
                let bypassed = (slot > 0).then_some(found.bypass);
                let auto = p.strip_auto(ti).cloned().unwrap_or_default();
                let beat = p.position_beats;
                let automated = |key: u32| {
                    let target = prowave_engine::AutoTarget::Plugin { slot, param: key };
                    (!auto.lane(target).is_empty())
                        .then(|| !auto.is_overridden(target) && auto.played(target, beat).is_some())
                };
                let at = crate::plugin_unit::Where {
                    badge: &badge,
                    strip: &strip,
                    name: &name,
                    bypassed,
                    automated: &automated,
                };
                let Some(unit) = self.units.get_mut(&uid) else {
                    continue;
                };
                let out = crate::plugin_unit::draw(ctx, uid, unit, handle.as_ref(), &at);
                if let Some(h) = &handle {
                    for (key, value) in &out.edits {
                        h.edit(*key, *value);
                    }
                    if let Some(n) = out.preset {
                        h.set_preset(n);
                    }
                    if let Some(side) = out.compare {
                        unit.ab[unit.side] = h.state();
                        if unit.ab[side].is_none() {
                            unit.ab[side] = unit.ab[unit.side].clone();
                        }
                        if let Some(state) = &unit.ab[side] {
                            h.set_state(state);
                        }
                        unit.side = side;
                    }
                }
                match out.view {
                    Some(true) => {
                        unit.generic = true;
                        self.windows.remove(&uid);
                    }
                    Some(false) => {
                        unit.generic = false;
                        let title = unit.title.clone();
                        let _ = self.open_window(uid, &title);
                    }
                    None => {}
                }
                if out.bypass && slot > 0 {
                    if let (Some(h), Some(e)) = (
                        &handle,
                        p.strip_plugins_mut(ti)
                            .and_then(|(fx, _)| fx.get_mut(usize::from(slot) - 1)),
                    ) {
                        if let Some(state) = h.state() {
                            e.state = Some(state);
                        }
                    }
                    if let Some(e) = p
                        .strip_plugins_mut(ti)
                        .and_then(|(fx, _)| fx.get_mut(usize::from(slot) - 1))
                    {
                        e.bypass = !e.bypass;
                    }
                }
                if out.close {
                    self.units.remove(&uid);
                    self.windows.remove(&uid);
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (ctx, p);
    }

    /// Brings the engine's plug-ins in line with the project: a track whose plug-ins changed
    /// gets new instances, with their settings. Returns what failed to open.
    pub fn sync(
        &mut self,
        p: &mut Project,
        engine: &mut impl PluginSink,
        max_frames: usize,
    ) -> Vec<String> {
        let mut failed = Vec::new();
        let n = p.tracks.len().min(prowave_engine::MAX_TRACKS);
        let len = n.max(self.sent.len());
        self.sent.resize(len, Vec::new());
        self.latency.resize(len, 0.0);
        self.shape.resize(len, (false, false));
        let keys: Vec<Vec<u64>> = (0..len)
            .map(|i| if i < n { key_of(p, i) } else { Vec::new() })
            .collect();
        // A track whose old instances are retired already is sent whatever its key is now
        // (an Undo may have put the old key back: its plug-ins are opened again).
        let changed: Vec<usize> = (0..len)
            .filter(|&i| keys[i] != self.sent[i] || self.retired_tracks.contains(&i))
            .collect();
        let master = master_key(p);
        let master_changed = master != self.master_sent || self.master_retired;
        self.waiting = false;
        if changed.is_empty() && !master_changed {
            return failed;
        }
        // No room in the engine now: nothing is closed or opened for a command it would
        // refuse (and free, plug-ins and all). The same change is found next frame.
        if !engine.can_replace_state() {
            self.waiting = true;
            return failed;
        }
        // The settings of the instances that go away are kept: in the project for the slots
        // still in it, and for Undo for all of them. Every changed track's old instances go
        // before any new one is opened, so a plug-in that moved to another track (one above
        // it was deleted) keeps the handle of its new instance. Each goes once: a track still
        // waiting for room next frame is not retired again.
        let pulled = self.pull_states(p);
        let mut gone: Vec<u64> = Vec::new();
        for &i in &changed {
            if self.retired_tracks.insert(i) {
                gone.extend(self.sent[i].iter().copied());
            }
        }
        if master_changed && !self.master_retired {
            self.master_retired = true;
            gone.extend(self.master_sent.iter().copied());
        }
        self.retire(&gone, &pulled);
        for (i, key) in keys.into_iter().enumerate() {
            if !changed.contains(&i) {
                continue;
            }
            // Nothing is opened for a command the engine would refuse: the rest waits.
            if !engine.can_replace_state() {
                self.waiting = true;
                break;
            }
            let plugins = if key.is_empty() {
                None
            } else {
                #[cfg(test)]
                let fake = self.open_fake_track(p, i, max_frames);
                #[cfg(not(test))]
                let fake = None;
                let track = match fake {
                    Some(track) => track,
                    None => self.open_track(p, i, engine.sample_rate(), max_frames, &mut failed),
                };
                Some(Box::new(track))
            };
            self.latency[i] = plugins.as_ref().map_or(0.0, |t| t.latency_seconds());
            self.shape[i] = plugins
                .as_ref()
                .map_or((false, false), |t| (t.has_instrument(), t.has_effects()));
            if engine.send(prowave_engine::Command::SetPlugins {
                track: i as u16,
                plugins,
            }) {
                self.retired_tracks.remove(&i);
                self.register_opened();
                // Read once the plug-ins can be reached: they start with the project's settings.
                self.note_opened(&key);
                self.sent[i] = key;
            } else {
                self.forget_opened();
                self.waiting = true;
            }
        }
        if master_changed {
            self.sync_master(p, master, engine, max_frames, &mut failed);
        }
        failed
    }

    /// The Stereo Out's plug-ins, as a track's: new instances, with their settings, whenever
    /// the ones that are not bypassed change (`key`; the old instances are retired already).
    fn sync_master(
        &mut self,
        p: &mut Project,
        key: Vec<u64>,
        engine: &mut impl PluginSink,
        max_frames: usize,
        failed: &mut Vec<String>,
    ) {
        // As for a track: nothing is opened for a command the engine would refuse.
        if !engine.can_replace_state() {
            self.waiting = true;
            return;
        }
        let plugins = if key.is_empty() {
            None
        } else {
            let chain = self.open_master(p, engine.sample_rate(), max_frames, failed);
            Some(Box::new(chain))
        };
        let latency = plugins
            .as_ref()
            .map_or_else(Vec::new, |t| t.effect_latencies().to_vec());
        if engine.send(prowave_engine::Command::SetMasterPlugins(plugins)) {
            self.master_latency = latency;
            // A new chain starts with every plug-in on.
            self.master_off_sent = None;
            self.master_retired = false;
            self.register_opened();
            self.note_opened(&key);
            self.master_sent = key;
        } else {
            self.forget_opened();
            self.waiting = true;
        }
    }

    #[cfg(target_os = "macos")]
    fn open_master(
        &mut self,
        p: &mut Project,
        sample_rate: u32,
        max_frames: usize,
        failed: &mut Vec<String>,
    ) -> prowave_engine::PluginTrack {
        let effects = p
            .master_au_effects
            .iter_mut()
            .filter(|s| !s.bypass)
            .filter_map(|s| self.open_slot(s, sample_rate, max_frames, failed))
            .collect();
        prowave_engine::PluginTrack::new(None, effects, max_frames)
    }

    #[cfg(not(target_os = "macos"))]
    fn open_master(
        &mut self,
        _p: &mut Project,
        _sample_rate: u32,
        max_frames: usize,
        failed: &mut Vec<String>,
    ) -> prowave_engine::PluginTrack {
        failed.push("plug-ins run on macOS only".into());
        prowave_engine::PluginTrack::new(None, Vec::new(), max_frames)
    }

    #[cfg(target_os = "macos")]
    fn open_track(
        &mut self,
        p: &mut Project,
        i: usize,
        sample_rate: u32,
        max_frames: usize,
        failed: &mut Vec<String>,
    ) -> prowave_engine::PluginTrack {
        let t = &mut p.tracks[i];
        let instrument = t
            .au_instrument
            .as_mut()
            .and_then(|s| self.open_slot(s, sample_rate, max_frames, failed));
        let effects = t
            .au_effects
            .iter_mut()
            .filter(|s| !s.bypass)
            .filter_map(|s| self.open_slot(s, sample_rate, max_frames, failed))
            .collect();
        prowave_engine::PluginTrack::new(instrument, effects, max_frames)
    }

    /// Opens the plug-in in `slot` (its settings restored), or says why it cannot.
    #[cfg(target_os = "macos")]
    fn open_slot(
        &mut self,
        slot: &mut PluginSlot,
        sample_rate: u32,
        max_frames: usize,
        failed: &mut Vec<String>,
    ) -> Option<Box<dyn prowave_engine::BlockProcessor>> {
        use prowave_engine::au::{find, AuInstance};
        use prowave_engine::BlockProcessor;
        {
            if slot.uid == 0 {
                slot.uid = new_uid();
            }
            if let Some(info) = prowave_engine::native_info(&slot.id) {
                let values = crate::native_fx_ui::values(slot, info);
                if crate::tune_lane::is_tune(slot) {
                    // Pro Wave Tune hears the project's key and MIDI notes from the app.
                    let (fx, remote) = prowave_engine::open_tune_block(sample_rate, &values);
                    crate::tune_lane::attach(slot.uid, remote);
                    return Some(fx);
                }
                return prowave_engine::open_native_block(&slot.id, sample_rate, &values);
            }
            if crate::channel_rack::is_rack(slot) {
                let (rack, errors) =
                    prowave_engine::SampleRack::new(&crate::channel_rack::specs(slot), sample_rate);
                failed.extend(errors);
                return Some(Box::new(rack));
            }
            if prowave_engine::vst3::is_vst3_id(&slot.id) {
                return self.open_vst3(slot, sample_rate, max_frames, failed);
            }
            let Some(info) = find(&slot.id) else {
                failed.push(format!("{} is not installed", slot.name));
                return None;
            };
            if quarantined().contains(&slot.id) {
                failed.push(format!(
                    "{} crashed Pro Wave and is turned off: pick it again in its slot to try it",
                    slot.name
                ));
                return None;
            }
            opening(Some(&slot.id));
            let opened = match AuInstance::new(&info, sample_rate, max_frames) {
                Ok(au) => {
                    let h = au.handle();
                    if let Some(state) = &slot.state {
                        h.set_state(state);
                    }
                    // Its parameters, for the automation choosers. It is reachable, and its
                    // window watched, once the engine has it (`register_opened`).
                    let params = h
                        .parameters()
                        .into_iter()
                        .map(|a| crate::automation::AuParamInfo {
                            id: a.id,
                            name: a.name,
                            min: a.min,
                            max: a.max,
                            stepped: a.stepped,
                        })
                        .collect();
                    self.opened.push(Opened {
                        uid: slot.uid,
                        handle: h.into(),
                        params,
                    });
                    Some(Box::new(au) as Box<dyn BlockProcessor>)
                }
                Err(e) => {
                    failed.push(e);
                    None
                }
            };
            opening(None);
            opened
        }
    }

    /// Opens the VST3 in `slot` (with the same crash guard as Audio Units).
    #[cfg(target_os = "macos")]
    fn open_vst3(
        &mut self,
        slot: &PluginSlot,
        sample_rate: u32,
        max_frames: usize,
        failed: &mut Vec<String>,
    ) -> Option<Box<dyn prowave_engine::BlockProcessor>> {
        use prowave_engine::vst3::{parse_id, Vst3Instance};
        let (Some(path), Some(cid)) = (crate::vst3_scan::path_of(&slot.id), parse_id(&slot.id))
        else {
            failed.push(format!("{} (VST3) is not installed", slot.name));
            return None;
        };
        let off = quarantined();
        let bundle_id = crate::vst3_scan::quarantine_id(
            &path,
            crate::vst3_scan::bundle_mtime(std::path::Path::new(&path)),
        );
        if off.contains(&slot.id) || off.contains(&bundle_id) {
            failed.push(format!(
                "{} crashed Pro Wave and is turned off: pick it again in its slot to try it",
                slot.name
            ));
            return None;
        }
        opening(Some(&slot.id));
        let opened =
            match Vst3Instance::new(std::path::Path::new(&path), &cid, sample_rate, max_frames) {
                Ok(inst) => {
                    let h = inst.handle();
                    if let Some(state) = &slot.state {
                        h.set_state(state);
                    }
                    // Its automatable parameters, as normalized values (VST3's own range).
                    let list = (0..h.parameter_count())
                        .filter_map(|k| {
                            let (name, free, _steps) = h.parameter_info(k)?;
                            free.then_some(crate::automation::AuParamInfo {
                                id: k as u32,
                                name,
                                min: 0.0,
                                max: 1.0,
                                stepped: false,
                            })
                        })
                        .collect();
                    self.opened.push(Opened {
                        uid: slot.uid,
                        handle: h.into(),
                        params: list,
                    });
                    Some(Box::new(inst) as Box<dyn prowave_engine::BlockProcessor>)
                }
                Err(e) => {
                    failed.push(e);
                    None
                }
            };
        opening(None);
        opened
    }

    #[cfg(not(target_os = "macos"))]
    fn open_track(
        &mut self,
        _p: &mut Project,
        _i: usize,
        _sample_rate: u32,
        max_frames: usize,
        failed: &mut Vec<String>,
    ) -> prowave_engine::PluginTrack {
        failed.push("plug-ins run on macOS only".into());
        prowave_engine::PluginTrack::new(None, Vec::new(), max_frames)
    }
}

#[cfg(test)]
impl PluginHost {
    /// The stand-in running in slot `uid`.
    pub(crate) fn fake(&self, uid: u64) -> Option<tests::FakePlugin> {
        self.fakes.get(&uid).cloned()
    }

    /// Track `i`'s plug-ins as stand-ins, when every one of them is one (their id starts with
    /// [`tests::FAKE_ID`]): each is opened with its slot's settings, as a real one is.
    fn open_fake_track(
        &mut self,
        p: &mut Project,
        i: usize,
        max_frames: usize,
    ) -> Option<prowave_engine::PluginTrack> {
        let t = p.tracks.get_mut(i)?;
        let is_fake = |s: &PluginSlot| s.id.starts_with(tests::FAKE_ID);
        if !t.au_instrument.iter().chain(&t.au_effects).all(is_fake) {
            return None;
        }
        let mut open = |s: &mut PluginSlot| -> Box<dyn prowave_engine::BlockProcessor> {
            if s.uid == 0 {
                s.uid = new_uid();
            }
            let fake = tests::FakePlugin::with_settings(s.state.as_deref().unwrap_or_default());
            self.fakes.insert(s.uid, fake.clone());
            Box::new(fake)
        };
        let instrument = t.au_instrument.as_mut().map(&mut open);
        let effects = t
            .au_effects
            .iter_mut()
            .filter(|s| !s.bypass)
            .map(&mut open)
            .collect();
        Some(prowave_engine::PluginTrack::new(
            instrument, effects, max_frames,
        ))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The id (prefix) of a stand-in plug-in.
    pub(crate) const FAKE_ID: &str = "fake:";

    /// A stand-in for a third-party plug-in at the host's boundary (the engine's stub effects
    /// have no settings): it passes sound through and has settings that a hand on its window
    /// changes without the project hearing of it, as an Audio Unit or a VST3 does.
    #[derive(Clone, Default)]
    pub(crate) struct FakePlugin(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl FakePlugin {
        pub(crate) fn with_settings(bytes: &[u8]) -> Self {
            Self(std::sync::Arc::new(std::sync::Mutex::new(bytes.to_vec())))
        }
        pub(crate) fn state(&self) -> Vec<u8> {
            self.0.lock().map(|s| s.clone()).unwrap_or_default()
        }
        /// The user turns a knob in the plug-in's own window.
        pub(crate) fn turn_knob(&self, settings: &[u8]) {
            if let Ok(mut s) = self.0.lock() {
                *s = settings.to_vec();
            }
        }
    }

    impl prowave_engine::BlockProcessor for FakePlugin {
        fn midi(&mut self, _status: u8, _d1: u8, _d2: u8, _offset: u32) {}
        fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
            out.0.copy_from_slice(input.0);
            out.1.copy_from_slice(input.1);
        }
        fn latency_seconds(&self) -> f64 {
            0.0
        }
    }

    /// The engine's end of the plug-in commands, without an audio device.
    #[derive(Default)]
    pub(crate) struct FakeSink {
        pub sent: Vec<prowave_engine::Command>,
        /// How many more commands are taken (`None`: any number).
        pub room: Option<usize>,
    }

    impl PluginSink for FakeSink {
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn send(&mut self, cmd: prowave_engine::Command) -> bool {
            match &mut self.room {
                Some(0) => return false,
                Some(n) => *n -= 1,
                None => {}
            }
            self.sent.push(cmd);
            true
        }
        fn can_replace_state(&mut self) -> bool {
            self.room != Some(0)
        }
    }

    /// A project with one instrument track holding a stand-in plug-in with `settings`, the
    /// host running it, and an Undo history: the app after the plug-in was put in its slot.
    pub(crate) fn project_with_fake(
        settings: &[u8],
    ) -> (Project, PluginHost, crate::undo::History, FakeSink, u64) {
        let mut p = Project::empty();
        let ti = p.add_track(crate::model::TrackKind::Instrument);
        let mut slot = PluginSlot::new("fake:synth", "Fake: Synth");
        slot.state = Some(settings.to_vec());
        let uid = slot.uid;
        p.tracks[ti].au_instrument = Some(slot);
        let mut host = PluginHost::default();
        let mut history = crate::undo::History::new(&p);
        let mut sink = FakeSink::default();
        let failed = sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        assert!(failed.is_empty(), "{failed:?}");
        (p, host, history, sink, uid)
    }

    fn settings_of(p: &Project, uid: u64) -> Option<Vec<u8>> {
        p.tracks
            .iter()
            .flat_map(|t| t.au_instrument.iter().chain(&t.au_effects))
            .find(|s| s.uid == uid)
            .and_then(|s| s.state.clone())
    }

    #[test]
    fn a_knob_turned_in_a_plugin_is_seen_as_changed_settings_until_they_are_read() {
        let (mut p, mut host, _history, _sink, uid) = project_with_fake(b"as saved");
        let plugin = host.fake(uid).expect("the plug-in runs");
        assert_eq!(
            plugin.state(),
            b"as saved",
            "opened with the slot's settings"
        );
        assert!(!host.settings_changed());

        plugin.turn_knob(b"turned");
        assert!(host.settings_changed());
        assert_eq!(
            settings_of(&p, uid).as_deref(),
            Some(&b"as saved"[..]),
            "the project has not heard of it"
        );

        // Reading them into the project (a save) makes the two the same again.
        host.save_states(&mut p);
        assert_eq!(settings_of(&p, uid).as_deref(), Some(&b"turned"[..]));
        assert!(!host.settings_changed());
    }

    #[test]
    fn a_frame_by_frame_caller_reads_the_settings_at_most_twice_a_second() {
        let (_p, host, _history, _sink, uid) = project_with_fake(b"a");
        assert!(!host.settings_changed_polled());
        host.fake(uid).expect("runs").turn_knob(b"b");
        assert!(
            !host.settings_changed_polled(),
            "the answer of a moment ago"
        );
        assert!(host.settings_changed(), "asked now: read now");
        std::thread::sleep(POLL_EVERY + std::time::Duration::from_millis(50));
        assert!(host.settings_changed_polled());
    }

    #[test]
    fn deleting_a_track_and_undoing_it_brings_the_plugin_back_as_it_last_was() {
        let (mut p, mut host, mut history, mut sink, uid) = project_with_fake(b"as inserted");
        let ti = p.tracks.len() - 1;
        host.fake(uid).expect("runs").turn_knob(b"as tweaked");

        // The user deletes the track; the frame ends (the edit is recorded) and the next
        // frame's sync takes the instance away.
        p.tracks.remove(ti);
        p.song_rev += 1;
        assert!(history.record(&p));
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        assert!(host.fake(uid).is_none(), "the instance is gone");

        assert!(history.undo(&mut p));
        assert_eq!(
            settings_of(&p, uid).as_deref(),
            Some(&b"as tweaked"[..]),
            "the track is back with the settings the plug-in had, not the saved ones"
        );
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        let again = host.fake(uid).expect("opened again");
        assert_eq!(again.state(), b"as tweaked");
        assert!(!history.record(&p), "bringing it back was not an edit");
        assert!(history.can_redo());

        // The same when the sync comes before the edit is recorded.
        again.turn_knob(b"tweaked again");
        p.tracks.remove(ti);
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        assert!(history.record(&p));
        assert!(history.undo(&mut p));
        assert_eq!(settings_of(&p, uid).as_deref(), Some(&b"tweaked again"[..]));
    }

    #[test]
    fn removing_a_plugin_from_its_slot_and_undoing_it_keeps_its_last_settings() {
        let (mut p, mut host, mut history, mut sink, uid) = project_with_fake(b"one");
        let ti = p.tracks.len() - 1;
        host.fake(uid).expect("runs").turn_knob(b"two");
        p.tracks[ti].au_instrument = None;
        assert!(history.record(&p));
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        assert!(history.undo(&mut p));
        assert_eq!(settings_of(&p, uid).as_deref(), Some(&b"two"[..]));
    }

    #[test]
    fn a_plugin_under_a_deleted_track_stays_reachable() {
        let (mut p, mut host, mut history, mut sink, uid) = project_with_fake(b"low");
        // A second plug-in track above it, then deleted: the first one's track moves up.
        let below = p.tracks.len() - 1;
        let mut above = p.tracks[below].clone();
        above.au_instrument = Some(PluginSlot::new("fake:other", "Fake: Other"));
        p.tracks.insert(below, above);
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        host.fake(uid).expect("runs").turn_knob(b"low, tweaked");
        p.tracks.remove(below);
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);

        let plugin = host.fake(uid).expect("still running, on its new track");
        assert_eq!(plugin.state(), b"low, tweaked");
        assert_eq!(settings_of(&p, uid).as_deref(), Some(&b"low, tweaked"[..]));
        plugin.turn_knob(b"later");
        assert!(host.settings_changed(), "its later changes are still seen");
    }

    #[test]
    fn a_moved_plugin_stays_reachable_when_the_engine_takes_the_change_over_two_frames() {
        let (mut p, mut host, mut history, mut sink, uid) = project_with_fake(b"low");
        let below = p.tracks.len() - 1;
        let mut above = p.tracks[below].clone();
        above.au_instrument = Some(PluginSlot::new("fake:other", "Fake: Other"));
        p.tracks.insert(below, above);
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        host.fake(uid).expect("runs").turn_knob(b"low, tweaked");
        p.tracks.remove(below);

        // Two tracks changed, the engine has room for one command.
        let before = sink.sent.len();
        sink.room = Some(1);
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        assert_eq!(sink.sent.len(), before + 1, "one command went");
        assert!(host.needs_retry(), "the other waits");
        let plugin = host.fake(uid).expect("running on its new track");
        assert_eq!(plugin.state(), b"low, tweaked");

        // No room at all: nothing is opened, closed or sent.
        sink.room = Some(0);
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        assert_eq!(sink.sent.len(), before + 1);
        assert!(host.needs_retry());
        assert!(host.fake(uid).is_some());

        // Room again: the old track's command goes, and the new instance is still reachable.
        sink.room = None;
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        assert_eq!(sink.sent.len(), before + 2);
        assert!(!host.needs_retry());
        let plugin = host.fake(uid).expect("not retired with its old track");
        assert_eq!(plugin.state(), b"low, tweaked");
        plugin.turn_knob(b"later");
        assert!(host.settings_changed(), "its later changes are still seen");
        // Nothing is left to do.
        sync_with_history(&mut host, &mut p, &mut history, &mut sink, 512);
        assert_eq!(sink.sent.len(), before + 2);
    }

    fn c(id: &str, maker: &str, name: &str, instrument: bool) -> AuChoice {
        AuChoice {
            id: id.into(),
            manufacturer: maker.into(),
            name: name.into(),
            instrument,
            alt_id: None,
        }
    }

    fn v(n: u8, maker: &str, name: &str, instrument: bool) -> AuChoice {
        c(
            &format!("vst3:{}", format!("{n:02X}").repeat(16)),
            maker,
            name,
            instrument,
        )
    }

    #[test]
    fn a_plugin_in_both_formats_is_one_entry() {
        let aus = vec![
            c("aufx:EchB:SToy", "Soundtoys", "EchoBoy", false),
            // Soundtoys' MIDI-taking effects are a second AU type of the same maker.
            c("aumf:Dcp2:SToy", "Soundtoys", "Decapitator", false),
            c("aumu:Dls :appl", "Apple", "DLSMusicDevice", true),
            c("aufx:FQ4 :FabF", "FabFilter", "Pro-Q 4", false),
        ];
        let vst3s = vec![
            v(1, "Soundtoys", "EchoBoy", false),
            v(2, "SoundToys", "Decapitator", false),
            v(3, "FabFilter", "Pro-Q 4", false),
            // Only a VST3, from a maker that has AUs spelled another way.
            v(4, "Soundtoys, Inc.", "PanMan", false),
            // Only a VST3, new maker.
            v(5, "vubeatz", "VU202", true),
            // The same class found twice (two copies of a bundle).
            v(5, "vubeatz", "VU202", true),
            // Same name as an AU but an instrument: not the same plug-in.
            v(6, "FabFilter", "Pro-Q 4", true),
        ];
        let all = merge_choices(aus, vst3s);
        let echo = all.iter().find(|x| x.name == "EchoBoy").unwrap();
        assert_eq!(echo.id, "aufx:EchB:SToy");
        assert_eq!(
            echo.alt_id.as_deref(),
            Some(v(1, "", "", false).id.as_str())
        );
        assert_eq!(echo.preferred_id(false), "aufx:EchB:SToy");
        assert_eq!(echo.preferred_id(true), v(1, "", "", false).id);
        assert!(all
            .iter()
            .find(|x| x.name == "Decapitator")
            .is_some_and(|d| d.alt_id.is_some()));
        assert_eq!(all.iter().filter(|x| x.name == "EchoBoy").count(), 1);
        assert_eq!(all.iter().filter(|x| x.name == "VU202").count(), 1);
        let pan = all.iter().find(|x| x.name == "PanMan").unwrap();
        assert_eq!(pan.manufacturer, "Soundtoys", "one Soundtoys submenu");
        assert_eq!(all.iter().filter(|x| x.name == "Pro-Q 4").count(), 2);
        assert_eq!(all.len(), 4 + 3);
        // The menu's makers: no maker twice.
        let mut makers: Vec<&str> = all.iter().map(|x| x.manufacturer.as_str()).collect();
        makers.sort_by_key(|m| m.to_lowercase());
        makers.dedup();
        assert_eq!(makers, vec!["Apple", "FabFilter", "Soundtoys", "vubeatz"]);
    }

    #[test]
    fn low_latency_passes_over_only_slow_master_plugins_while_live() {
        let lat = [0.0, 0.0005, 0.002, 0.25];
        assert_eq!(low_latency_mask(&lat, false, 0.001), 0, "not live: all on");
        assert_eq!(low_latency_mask(&lat, true, 0.001), 0b1100);
        assert_eq!(low_latency_mask(&[], true, 0.001), 0);
    }

    /// Real Audio Units on the Stereo Out: Apple's AUDelay (and AUPeakLimiter when there) are
    /// opened as the app opens them, rendered offline on the master bus with an automated
    /// parameter, and their settings go through a saved project file and back.
    #[cfg(target_os = "macos")]
    #[test]
    fn real_audio_units_on_the_stereo_out_render_automate_save_and_load() {
        use prowave_engine::{
            AutoLane, AutoPoint, AutoTarget, Command, EngineCore, Shared, MASTER_TRACK,
        };
        const SR: u32 = 48_000;
        let mut p = Project::empty();
        p.master_au_effects
            .push(PluginSlot::new("aufx:dely:appl", "Apple: AUDelay"));
        let limiter = prowave_engine::au::find("aufx:lmtr:appl").is_some();
        if limiter {
            p.master_au_effects
                .push(PluginSlot::new("aufx:lmtr:appl", "Apple: AUPeakLimiter"));
        }
        let mut host = PluginHost::default();
        let mut failed = Vec::new();
        let chain = host.open_master(&mut p, SR, 1024, &mut failed);
        host.register_opened();
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(chain.effect_latencies().len(), p.master_au_effects.len());
        eprintln!("master plug-in delays (s): {:?}", chain.effect_latencies());
        // Their parameters are there for the Stereo Out's automation choosers.
        let delay_uid = p.master_au_effects[0].uid;
        let mix = host.params[&delay_uid]
            .iter()
            .find(|a| a.name.to_lowercase().contains("mix"))
            .cloned()
            .expect("AUDelay has a dry/wet mix");
        let devices = crate::automation::devices(
            &p,
            crate::automation::Strip::Master,
            &crate::automation::AuParams(host.params.clone()),
        );
        assert!(devices.iter().any(|d| d.name.contains("AUDelay")));

        // Offline: a tone through the master bus, the delay all wet (its line starts empty:
        // near silence), then all dry (the tone), by its automation lane.
        let (mut tx, rx) = rtrb::RingBuffer::new(32);
        let (gtx, _grx) = rtrb::RingBuffer::new(32);
        let mut core = EngineCore::new(SR, rx, gtx, std::sync::Arc::new(Shared::new()));
        let lane = |v: f32| {
            Command::SetAutomation(vec![AutoLane {
                lo: mix.min,
                hi: mix.max,
                ..AutoLane::new(
                    MASTER_TRACK,
                    AutoTarget::Plugin {
                        slot: 1,
                        param: mix.id,
                    },
                    vec![AutoPoint::new(0.0, v)],
                )
            }
            .sanitized()])
        };
        for cmd in [
            Command::SetMasterGain { linear: 1.0 },
            Command::SetTestTone {
                hz: 440.0,
                gain: 0.25,
            },
            Command::SetMasterPlugins(Some(Box::new(chain))),
            lane(1.0),
        ] {
            assert!(tx.push(cmd).is_ok());
        }
        let rms = |core: &mut EngineCore| {
            let mut sum = 0.0f64;
            let mut buf = vec![0f32; 512 * 2];
            for _ in 0..20 {
                core.process(&mut buf);
                sum += buf
                    .iter()
                    .map(|x| f64::from(*x) * f64::from(*x))
                    .sum::<f64>();
            }
            (sum / (20.0 * 1024.0)).sqrt() as f32
        };
        let wet = rms(&mut core);
        assert!(tx.push(lane(0.0)).is_ok());
        rms(&mut core);
        let dry = rms(&mut core);
        let tone = 0.25 / std::f32::consts::SQRT_2;
        eprintln!("master AUDelay: wet {wet:.4}, dry {dry:.4}, tone {tone:.4}");
        assert!(
            wet < tone * 0.1,
            "all wet, the delay line is still empty: {wet}"
        );
        assert!(
            (dry - tone).abs() < tone * 0.2,
            "all dry passes the tone: {dry}"
        );

        // The settings the automation left are saved with the project and come back.
        host.save_states(&mut p);
        assert!(p.master_au_effects.iter().all(|s| s.state.is_some()));
        if let Some(last) = p.master_au_effects.last_mut() {
            last.bypass = limiter;
        }
        let dir = tempfile::tempdir().expect("a folder");
        let path = dir.path().join("master.prowave");
        crate::project_io::save_project(&p, &path).expect("saved");
        let doc = prowave_project::load(&path, dir.path()).expect("loaded");
        let (mut q, _) = crate::project_io::from_doc(&doc, dir.path(), &egui::Context::default());
        assert_eq!(q.master_au_effects.len(), p.master_au_effects.len());
        for (a, b) in q.master_au_effects.iter().zip(&p.master_au_effects) {
            assert_eq!(
                (&a.id, &a.name, &a.state, a.bypass),
                (&b.id, &b.name, &b.state, b.bypass)
            );
        }
        // Opened again from the file, the delay has the dry mix it was saved with; a bypassed
        // plug-in is not opened.
        let mut again = PluginHost::default();
        let chain = again.open_master(&mut q, SR, 1024, &mut failed);
        again.register_opened();
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(chain.effect_latencies().len(), 1);
        let Some(prowave_engine::plugin_handle::PluginHandle::Au(h)) =
            again.handles.get(&q.master_au_effects[0].uid)
        else {
            panic!("the delay runs");
        };
        assert_eq!(h.parameter(mix.id), Some(mix.min));
        drop(chain);
    }

    /// A command the engine refuses is freed with its plug-ins. Until the engine has taken
    /// it, the host has no way to them (no handle, no window watch, no parameters), so
    /// nothing is left pointing at a freed Audio Unit; opening again for the next try works.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_plugin_is_reachable_only_once_the_engine_has_its_command() {
        if prowave_engine::au::find("aufx:dely:appl").is_none() {
            eprintln!("skipped: Apple's AUDelay is not on this machine");
            return;
        }
        let mut p = Project::empty();
        p.tracks[0]
            .au_effects
            .push(PluginSlot::new("aufx:dely:appl", "Apple: AUDelay"));
        p.master_au_effects
            .push(PluginSlot::new("aufx:dely:appl", "Apple: AUDelay"));
        let mut host = PluginHost::default();
        let mut failed = Vec::new();
        let nothing = |host: &PluginHost| {
            host.handles.is_empty() && host.watches.is_empty() && host.params.is_empty()
        };

        // Opened for a command: not reachable yet.
        let track = host.open_track(&mut p, 0, 48_000, 1024, &mut failed);
        let master = host.open_master(&mut p, 48_000, 1024, &mut failed);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(host.opened.len(), 2);
        assert!(nothing(&host));
        // The engine refuses: the plug-ins are freed with the command, and forgotten here.
        drop((track, master));
        host.forget_opened();
        assert!(host.opened.is_empty() && nothing(&host));
        // Whatever the host does next finds nothing to ask.
        let before = p.tracks[0].au_effects[0].state.clone();
        host.save_states(&mut p);
        assert_eq!(p.tracks[0].au_effects[0].state, before);
        assert!(host.take_param_moves(&p).is_empty());

        // The next try: opened again, taken by the engine, reachable, and still running.
        let track = host.open_track(&mut p, 0, 48_000, 1024, &mut failed);
        assert!(failed.is_empty(), "{failed:?}");
        host.register_opened();
        let uid = p.tracks[0].au_effects[0].uid;
        assert!(host.opened.is_empty());
        assert!(host.handles.contains_key(&uid) && host.params.contains_key(&uid));
        host.save_states(&mut p);
        assert!(p.tracks[0].au_effects[0].state.is_some(), "it answers");
        // The watch goes before its unit.
        host.watches.clear();
        host.handles.clear();
        drop(track);
    }

    #[test]
    fn formats_are_told_apart() {
        assert_eq!(format_label("aufx:DEC :SToy"), "AU");
        assert_eq!(format_label(&format!("vst3:{}", "0".repeat(32))), "VST3");
        assert_eq!(format_label("prowave:lemon-limiter"), "");
        assert_eq!(match_key("Valhalla DSP, LLC"), match_key("ValhallaDSP"));
        assert_ne!(match_key("Pro-Q 4"), match_key("Pro-Q 3"));
    }

    /// Pictures of the plug-in rack units over real Apple Audio Units: the frame alone (the
    /// plug-in's own window beside it) and the host-drawn editor. Ignored; checks nothing:
    /// `FX_PNG_DIR=/some/dir cargo test -p prowave-app plugin_unit_pictures -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "pictures of the plug-in units"]
    fn plugin_unit_pictures() {
        let dir = std::env::var_os("FX_PNG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-fx"));
        let _ = std::fs::create_dir_all(&dir);
        let floor = egui::Color32::from_rgb(0xcf, 0xc8, 0xb8);
        for (id, name, generic, file, h) in [
            (
                "aufx:dely:appl",
                "Apple: AUDelay",
                false,
                "plugin-frame-audelay.png",
                300.0,
            ),
            (
                "aufx:dely:appl",
                "Apple: AUDelay",
                true,
                "plugin-generic-audelay.png",
                560.0,
            ),
            (
                "aufx:mrev:appl",
                "Apple: AUMatrixReverb",
                true,
                "plugin-generic-aumatrixreverb.png",
                980.0,
            ),
            (
                "aufx:nbeq:appl",
                "Apple: AUNBandEQ",
                true,
                "plugin-generic-aunbandeq.png",
                980.0,
            ),
        ] {
            if prowave_engine::au::find(id).is_none() {
                continue;
            }
            let mut p = Project::empty();
            p.master_au_effects.push(PluginSlot::new(id, name));
            let mut host = PluginHost::default();
            let mut failed = Vec::new();
            // The chain keeps the units alive while they are drawn.
            let chain = host.open_master(&mut p, 48_000, 512, &mut failed);
            host.register_opened();
            assert!(failed.is_empty(), "{failed:?}");
            let uid = p.master_au_effects[0].uid;
            let handle = host.handles.get(&uid).cloned();
            let mut unit = crate::plugin_unit::UnitState {
                generic,
                title: name.into(),
                ..Default::default()
            };
            let none = |_: u32| None;
            crate::ui_test::render_png(
                760.0,
                h,
                2.0,
                floor,
                |ui| {
                    crate::theme::apply(ui.ctx());
                    let at = crate::plugin_unit::Where {
                        badge: "ST",
                        strip: "Stereo Out",
                        name,
                        bypassed: Some(false),
                        automated: &none,
                    };
                    crate::plugin_unit::draw(ui.ctx(), uid, &mut unit, handle.as_ref(), &at);
                },
                &dir.join(file),
            )
            .unwrap();
            drop(chain);
        }
    }
}
