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

#[derive(Default)]
pub struct PluginHost {
    /// Per engine track: the key last sent.
    sent: Vec<Vec<u64>>,
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
}

impl PluginHost {
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

    /// Reads every running plug-in's settings into the project, before it is saved.
    pub fn save_states(&self, p: &mut Project) {
        #[cfg(target_os = "macos")]
        for slot in p
            .tracks
            .iter_mut()
            .flat_map(|t| t.au_instrument.iter_mut().chain(t.au_effects.iter_mut()))
            .chain(p.master_au_effects.iter_mut())
        {
            if let Some(h) = self.handles.get(&slot.uid) {
                if let Some(state) = h.state() {
                    slot.state = Some(state);
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = p;
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
        engine: &mut prowave_engine::Engine,
        max_frames: usize,
    ) -> Vec<String> {
        let mut failed = Vec::new();
        let n = p.tracks.len().min(prowave_engine::MAX_TRACKS);
        let len = n.max(self.sent.len());
        self.sent.resize(len, Vec::new());
        self.latency.resize(len, 0.0);
        self.shape.resize(len, (false, false));
        for i in 0..len {
            let key = if i < n { key_of(p, i) } else { Vec::new() };
            if key == self.sent[i] {
                continue;
            }
            // The settings of the instances that go away are kept in the project.
            self.save_states(p);
            #[cfg(target_os = "macos")]
            for uid in &self.sent[i] {
                self.windows.remove(uid);
                // The watch goes before its unit (the engine frees that later).
                self.watches.remove(uid);
                self.handles.remove(uid);
            }
            for uid in &self.sent[i] {
                self.params.remove(uid);
            }
            let plugins = if key.is_empty() {
                None
            } else {
                Some(Box::new(self.open_track(
                    p,
                    i,
                    engine.sample_rate(),
                    max_frames,
                    &mut failed,
                )))
            };
            self.latency[i] = plugins.as_ref().map_or(0.0, |t| t.latency_seconds());
            self.shape[i] = plugins
                .as_ref()
                .map_or((false, false), |t| (t.has_instrument(), t.has_effects()));
            if engine.send(prowave_engine::Command::SetPlugins {
                track: i as u16,
                plugins,
            }) {
                self.sent[i] = key;
            }
        }
        self.sync_master(p, engine, max_frames, &mut failed);
        failed
    }

    /// The Stereo Out's plug-ins, as a track's: new instances, with their settings, whenever
    /// the ones that are not bypassed change.
    fn sync_master(
        &mut self,
        p: &mut Project,
        engine: &mut prowave_engine::Engine,
        max_frames: usize,
        failed: &mut Vec<String>,
    ) {
        let key = master_key(p);
        if key == self.master_sent {
            return;
        }
        self.save_states(p);
        #[cfg(target_os = "macos")]
        for uid in &self.master_sent {
            self.windows.remove(uid);
            self.watches.remove(uid);
            self.handles.remove(uid);
        }
        for uid in &self.master_sent {
            self.params.remove(uid);
        }
        let plugins = if key.is_empty() {
            None
        } else {
            Some(Box::new(self.open_master(
                p,
                engine.sample_rate(),
                max_frames,
                failed,
            )))
        };
        let latency = plugins
            .as_ref()
            .map_or_else(Vec::new, |t| t.effect_latencies().to_vec());
        if engine.send(prowave_engine::Command::SetMasterPlugins(plugins)) {
            self.master_sent = key;
            self.master_latency = latency;
            // A new chain starts with every plug-in on.
            self.master_off_sent = None;
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
                    self.handles.insert(slot.uid, h.into());
                    // Its parameters, for the automation choosers, and a watch on its window.
                    let list = h.parameters();
                    let ids: Vec<u32> = list.iter().map(|a| a.id).collect();
                    if let Some(w) = prowave_engine::au::AuParamWatch::new(h, &ids) {
                        self.watches.insert(slot.uid, w);
                    }
                    self.params.insert(
                        slot.uid,
                        list.into_iter()
                            .map(|a| crate::automation::AuParamInfo {
                                id: a.id,
                                name: a.name,
                                min: a.min,
                                max: a.max,
                                stepped: a.stepped,
                            })
                            .collect(),
                    );
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
                    self.params.insert(slot.uid, list);
                    self.handles.insert(slot.uid, h.into());
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
mod tests {
    use super::*;

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
