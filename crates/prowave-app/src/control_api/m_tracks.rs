//! Methods for tracks and channel strips: the tracks themselves, their instruments and
//! effects, every parameter a strip has, automation, and live notes.
use super::params::{place_json, req_place, round6, P};
use super::protocol::{Code, RpcError};
use super::registry::{m, p, Method, Reply, Ty};
use super::state::{self, FxRef};
use super::{ok, Cx};
use crate::automation::{self, ParamDesc, Strip};
use crate::mixer::{InsertAct, MASTER_STRIP};
use crate::model::{Project, TrackKind};
use crate::plugins::PluginSlot;
use prowave_engine::{AutoTarget, FxKind, InputSource};
use serde_json::{json, Value};

// ------------------------------------------------------------------ tracks

fn track_list(cx: &mut Cx, _: &P) -> Result<Reply, RpcError> {
    cx.core.ids.reconcile(&cx.app.project);
    let p = &cx.app.project;
    let list: Vec<Value> = (0..p.tracks.len())
        .map(|i| state::track_json(p, &cx.core.ids, i, false, false))
        .collect();
    ok(json!({"tracks": list}))
}

fn track_get(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let notes = params.bool("notes").unwrap_or(true);
    ok(state::track_json(
        &cx.app.project,
        &cx.core.ids,
        ti,
        true,
        notes,
    ))
}

fn forget_region_selection(cx: &mut Cx) {
    cx.app.ui.tracks.clear_selection();
    cx.app.ui.piano.selected.clear();
}

fn track_add(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let kind_name = params.str("kind").unwrap_or("instrument");
    let Some(kind) = state::kind_from(kind_name) else {
        return Err(RpcError::invalid("kind", "unknown kind of track"));
    };
    if kind == TrackKind::Folder {
        return Err(RpcError::new(
            Code::NotSupported,
            "a folder is made from a track that is there: select the track (ui.select) and call menu.invoke with \"Create Track Stack\"",
        ));
    }
    if cx.app.project.tracks.len() >= prowave_engine::MAX_TRACKS {
        return Err(RpcError::failed(format!(
            "the engine plays {} tracks at most",
            prowave_engine::MAX_TRACKS
        )));
    }
    let ti = cx.app.project.add_track(kind);
    if let Some(name) = params.str("name") {
        crate::track_dialogs::rename_track(&mut cx.app.project, ti, name);
    }
    let mut note = None;
    if let Some(id) = params.str("instrument") {
        if let Err(e) = set_instrument(cx, ti, id, false) {
            note = Some(format!(
                "the track was added, but its instrument was not set: {}",
                e.message
            ));
        }
    }
    cx.ctx.request_repaint();
    cx.core.ids.reconcile(&cx.app.project);
    let mut out = state::track_json(&cx.app.project, &cx.core.ids, ti, true, false);
    if let (Some(o), Some(note)) = (out.as_object_mut(), note) {
        o.insert("warning".into(), note.into());
    }
    ok(out)
}

fn track_remove(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let (id, name) = (
        cx.core.ids.track_id(ti),
        cx.app.project.tracks[ti].name.clone(),
    );
    if cx.app.project.tracks.len() <= 1 {
        return Err(RpcError::failed(
            "a project keeps at least one track: this is its only one",
        ));
    }
    if !cx.app.project.delete_track(ti) {
        return Err(RpcError::failed(format!(
            "the app did not remove \"{name}\""
        )));
    }
    forget_region_selection(cx);
    ok(json!({"removed": id, "name": name, "tracks": cx.app.project.tracks.len()}))
}

fn track_rename(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let name = params.req_str("name")?;
    if !crate::track_dialogs::rename_track(&mut cx.app.project, ti, name) {
        return Err(RpcError::invalid("name", "a track's name cannot be empty"));
    }
    ok(json!({"id": cx.ids().track_id(ti), "name": cx.app.project.tracks[ti].name}))
}

fn track_move(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let from = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let pr = &mut cx.app.project;
    let n = pr.tracks.len();
    let to = (params.i64("to").unwrap_or(1).clamp(1, n as i64) - 1) as usize;
    if matches!(pr.tracks[from].kind, TrackKind::Folder | TrackKind::Vca) {
        return Err(RpcError::new(
            Code::NotSupported,
            "folder and VCA tracks stay where they are: other tracks point at their place",
        ));
    }
    if pr.tracks.iter().any(|t| t.kind == TrackKind::Folder) {
        return Err(RpcError::new(
            Code::NotSupported,
            "the project has track stacks (folders), whose tracks are told by their place: release the stacks before moving tracks",
        ));
    }
    if from != to {
        // The app has no gesture for this; the tracks are renumbered with the model's own
        // helpers, so VCA assignments keep pointing at the same tracks.
        let own_vca = pr.tracks[from].vca;
        let vca_name = own_vca
            .and_then(|v| pr.tracks.get(v))
            .map(|t| t.name.clone());
        let mut track = pr.tracks.remove(from);
        pr.vca_removed(from);
        track.vca = None;
        track.indent = 0;
        pr.tracks.insert(to, track);
        pr.vca_inserted(to);
        if let Some(name) = vca_name {
            let v = pr
                .tracks
                .iter()
                .position(|t| t.kind == TrackKind::Vca && t.name == name);
            pr.assign_vca(to, v);
        }
        pr.normalize_nesting();
        pr.selected_track = to;
        pr.song_rev += 1;
        forget_region_selection(cx);
    }
    let list = cx.ids().track_ids();
    ok(json!({"id": cx.core.ids.track_id(to), "number": to + 1, "order": list}))
}

fn parse_input(text: &str) -> Option<InputSource> {
    let (kind, n) = text.split_once(':')?;
    let n: u16 = n.parse().ok().filter(|n| *n >= 1)?;
    match kind {
        "mono" => Some(InputSource::Mono(n - 1)),
        "stereo" => Some(InputSource::Stereo(n - 1)),
        _ => None,
    }
}

fn track_set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let pr = &mut cx.app.project;
    let kind = pr.tracks[ti].kind;
    if let Some(name) = params.str("name") {
        if !crate::track_dialogs::rename_track(pr, ti, name) {
            return Err(RpcError::invalid("name", "a track's name cannot be empty"));
        }
    }
    if let Some(on) = params.bool("mute") {
        pr.set_mute(ti, on);
    }
    if let Some(on) = params.bool("solo") {
        pr.set_solo(ti, on);
    }
    if let Some(on) = params.bool("arm") {
        if !matches!(
            kind,
            TrackKind::Audio | TrackKind::Instrument | TrackKind::Drummer | TrackKind::ExternalMidi
        ) {
            return Err(RpcError::new(
                Code::NotSupported,
                format!("a {} track does not record", state::kind_name(kind)),
            ));
        }
        pr.tracks[ti].rec = on;
    }
    if let Some(on) = params.bool("monitor") {
        if kind != TrackKind::Audio {
            return Err(RpcError::new(
                Code::NotSupported,
                "input monitoring is for audio tracks",
            ));
        }
        pr.tracks[ti].monitor = on;
    }
    let beat = pr.position_beats;
    let playing = pr.playing;
    for (key, target) in [("volume_db", AutoTarget::Volume), ("pan", AutoTarget::Pan)] {
        if let Some(v) = params.f32(key) {
            // As a hand on the fader: an envelope on it is overridden until re-enabled.
            automation::moved(&mut pr.tracks[ti], target, v);
            automation::released(&mut pr.tracks[ti], target, beat, playing);
        }
    }
    if let Some(text) = params.str("input") {
        if kind != TrackKind::Audio {
            return Err(RpcError::new(
                Code::NotSupported,
                "only audio tracks have an input",
            ));
        }
        let Some(input) = parse_input(text) else {
            return Err(RpcError::invalid(
                "input",
                format!("`input` is \"mono:N\" or \"stereo:N\" (N is the first input channel, from 1); got \"{text}\""),
            ));
        };
        pr.tracks[ti].input = input;
    }
    if let Some(text) = params.str("output") {
        let bus = if text == "stereo_out" {
            Some(0)
        } else {
            text.strip_prefix("bus:")
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| (1..=8).contains(n))
        };
        let Some(bus) = bus else {
            return Err(RpcError::invalid(
                "output",
                format!("`output` is \"stereo_out\" or \"bus:1\" to \"bus:8\"; got \"{text}\""),
            ));
        };
        pr.tracks[ti].output = bus;
        if bus > 0 {
            // A bus is heard through its Aux strip, as when it is picked in the mixer.
            pr.ensure_aux_for_bus(bus);
        }
        pr.song_rev += 1;
    }
    if let Some(vca) = params.get("vca") {
        let target = if vca.as_str() == Some("none") {
            None
        } else {
            Some(state::track_ref(pr, &cx.core.ids, vca, "vca")?)
        };
        if !pr.assign_vca(ti, target) {
            return Err(RpcError::failed(
                "that track is not a VCA track (or the track cannot have one)",
            ));
        }
    }
    cx.ctx.request_repaint();
    cx.core.ids.reconcile(&cx.app.project);
    ok(state::track_json(
        &cx.app.project,
        &cx.core.ids,
        ti,
        false,
        false,
    ))
}

fn track_send(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let pr = &mut cx.app.project;
    let bus = params.i64("bus").unwrap_or(1) as u8;
    let existing = pr.tracks[ti].sends.iter().position(|s| s.bus == Some(bus));
    if params.bool("remove") == Some(true) {
        let Some(k) = existing else {
            return Err(RpcError::not_found(format!(
                "the track has no send to bus {bus}"
            )));
        };
        pr.tracks[ti].sends.remove(k);
        pr.tracks[ti].auto.send_removed(k);
    } else {
        let k = match existing {
            Some(k) => k,
            None => {
                if pr.tracks[ti].sends.len() >= crate::model::MAX_SENDS {
                    return Err(RpcError::failed(format!(
                        "a strip has {} sends at most",
                        crate::model::MAX_SENDS
                    )));
                }
                pr.tracks[ti].sends.push(crate::model::SendSlot {
                    bus: Some(bus),
                    level: 0.5,
                    pre: false,
                });
                pr.ensure_aux_for_bus(bus);
                pr.tracks[ti].sends.len() - 1
            }
        };
        if let Some(level) = params.f32("level") {
            pr.tracks[ti].sends[k].level = level.clamp(0.0, 1.0);
        }
        if let Some(pre) = params.bool("pre_fader") {
            pr.tracks[ti].sends[k].pre = pre;
        }
    }
    pr.song_rev += 1;
    cx.core.ids.reconcile(&cx.app.project);
    ok(state::track_json(
        &cx.app.project,
        &cx.core.ids,
        ti,
        false,
        false,
    ))
}

// ------------------------------------------------------------------ what can go in a slot

/// An instrument the app has built in: (id, name, what makes its slot).
type BuiltIn = (&'static str, &'static str, fn() -> PluginSlot);

/// The instruments the app has built in.
fn built_in_instruments() -> Vec<BuiltIn> {
    let all: &[BuiltIn] = &[
        (
            crate::channel_rack::RACK_ID,
            "Channel Rack",
            crate::channel_rack::new_slot,
        ),
        (crate::singer::SINGER_ID, "Singer", crate::singer::new_slot),
        (crate::bass808::BASS808_ID, "808", crate::bass808::new_slot),
    ];
    all.to_vec()
}

fn plugin_list(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let kind = params.str("kind");
    let search = params.str("search").map(str::to_lowercase);
    let mut out: Vec<Value> = Vec::new();
    for (id, name, _) in built_in_instruments() {
        out.push(json!({"id": id, "name": name, "maker": "Pro Wave", "kind": "instrument", "format": "built-in"}));
    }
    for info in prowave_engine::native_instruments() {
        out.push(json!({"id": info.id, "name": info.name, "maker": "Pro Wave", "kind": "instrument", "format": "built-in"}));
    }
    for k in FxKind::ALL {
        out.push(json!({"id": crate::plugins::builtin_fx_id(k), "name": k.name(), "maker": "Pro Wave", "kind": "effect", "format": "built-in"}));
    }
    for info in prowave_engine::native_effects() {
        out.push(json!({"id": info.id, "name": info.name, "maker": "Pro Wave", "kind": "effect", "format": "built-in"}));
    }
    let off = crate::plugins::quarantined();
    for c in &cx.app.ui.mixer.au_list {
        let mut o = json!({
            "id": c.id,
            "name": c.name,
            "maker": c.manufacturer,
            "kind": if c.instrument { "instrument" } else { "effect" },
            "format": crate::plugins::format_label(&c.id),
        });
        if let Some(map) = o.as_object_mut() {
            if let Some(alt) = &c.alt_id {
                map.insert("vst3_id".into(), alt.clone().into());
            }
            if off.contains(&c.id) {
                map.insert("turned_off_after_crash".into(), true.into());
            }
        }
        out.push(o);
    }
    // The rating and type the menus sort by (the settings file plugin-ratings.json).
    let ratings = crate::plugin_ratings::ratings();
    for e in &mut out {
        let id = e
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let alt = e.get("vst3_id").and_then(Value::as_str).map(String::from);
        if let (Some(r), Some(o)) = (ratings.of(&id, alt.as_deref()), e.as_object_mut()) {
            if let Some(x) = r.rating {
                o.insert("rating".into(), json!(x));
            }
            if !r.kind.is_empty() {
                o.insert("type".into(), json!(r.kind));
            }
            if !r.emulates.is_empty() {
                o.insert("emulates".into(), json!(r.emulates));
            }
            if r.master_chain {
                o.insert("master_chain".into(), json!(true));
            }
        }
    }
    let hidden = crate::plugins::hidden();
    out.retain(|e| {
        let is = |key: &str, want: &str| e.get(key).and_then(Value::as_str) == Some(want);
        let text = |key: &str| {
            e.get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase()
        };
        let hidden_here = hidden.hides(
            e.get("id").and_then(Value::as_str).unwrap_or(""),
            e.get("maker").and_then(Value::as_str).unwrap_or(""),
        ) || e
            .get("vst3_id")
            .and_then(Value::as_str)
            .is_some_and(|v| hidden.ids.iter().any(|h| h == v));
        !hidden_here
            && kind.is_none_or(|k| is("kind", k))
            && search.as_ref().is_none_or(|s| {
                text("name").contains(s)
                    || text("maker").contains(s)
                    || text("id").contains(s)
                    || text("emulates").contains(s)
            })
    });
    ok(json!({
        "count": out.len(),
        "plugins": out,
        "scanning": false,
        "note": "Audio Units and VST3s are those the app's last scan found; a plug-in in both formats is one entry (its AU id, with vst3_id). Hidden plug-ins (the settings file plugin-hidden, and Apple's Audio Units unless plugin-hide-apple holds 0) are left out; rating and type come from plugin-ratings.json",
    }))
}

/// The slot for the instrument or effect a request names: an id from `plugin.list`, or a
/// name ("Maker: Name" or the name alone when one plug-in has it).
fn slot_for(cx: &Cx, text: &str, instrument: bool) -> Result<PluginSlot, RpcError> {
    let low = text.to_lowercase();
    if instrument {
        if let Some((_, _, make)) = built_in_instruments()
            .into_iter()
            .find(|(id, name, _)| *id == text || name.to_lowercase() == low)
        {
            return Ok(make());
        }
        if let Some(info) = prowave_engine::native_instruments()
            .iter()
            .find(|i| i.id == text || i.name.to_lowercase() == low)
        {
            return Ok(crate::native_inst_ui::new_slot(info));
        }
    } else if let Some(info) = prowave_engine::native_effects()
        .iter()
        .find(|i| i.id == text || i.name.to_lowercase() == low)
    {
        return Ok(crate::native_fx_ui::new_slot(info));
    }
    let list = &cx.app.ui.mixer.au_list;
    let by_id = list.iter().find(|c| {
        c.instrument == instrument && (c.id == text || c.alt_id.as_deref() == Some(text))
    });
    if let Some(c) = by_id {
        return Ok(PluginSlot::new(text, &c.label()));
    }
    let named: Vec<&crate::plugins::AuChoice> = list
        .iter()
        .filter(|c| {
            c.instrument == instrument
                && (c.name.to_lowercase() == low || c.label().to_lowercase() == low)
        })
        .collect();
    let what = if instrument { "instrument" } else { "effect" };
    match named.as_slice() {
        [c] => {
            let id = c.preferred_id(crate::plugins::prefer_vst3());
            Ok(PluginSlot::new(id, &c.label()))
        }
        [] => {
            let near: Vec<String> = list
                .iter()
                .filter(|c| c.instrument == instrument && c.label().to_lowercase().contains(&low))
                .take(12)
                .map(|c| format!("\"{}\" ({})", c.label(), c.id))
                .collect();
            let hint = if near.is_empty() {
                format!("plugin.list {{\"kind\":\"{what}\"}} gives the ids")
            } else {
                format!("near matches: {}", near.join(", "))
            };
            Err(RpcError::not_found(format!(
                "there is no {what} \"{text}\" on this Mac; {hint}"
            )))
        }
        many => Err(RpcError::invalid(
            "id",
            format!(
                "{} {what}s are named \"{text}\"; use an id: {}",
                many.len(),
                many.iter()
                    .map(|c| format!("{} ({})", c.id, c.label()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn set_instrument(cx: &mut Cx, ti: usize, id: &str, open_window: bool) -> Result<Value, RpcError> {
    let kind = cx.app.project.tracks[ti].kind;
    if !matches!(kind, TrackKind::Instrument | TrackKind::Drummer) {
        return Err(RpcError::new(
            Code::NotSupported,
            format!(
                "a {} track has no instrument slot (instrument and drummer tracks do)",
                state::kind_name(kind)
            ),
        ));
    }
    let slot = slot_for(cx, id, true)?;
    // Picking a plug-in that was turned off after a crash lets it try again, as in the menu.
    crate::plugins::release(&slot.id);
    let out = state::slot_json(&slot);
    if open_window {
        cx.app.ui.mixer.open_plugin = Some((slot.uid, slot.name.clone()));
    }
    let t = &mut cx.app.project.tracks[ti];
    t.au_instrument = Some(slot);
    // One instrument plays the track.
    t.soundfont = None;
    cx.ctx.request_repaint();
    Ok(out)
}

fn instrument_set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    if let Some(path) = params.path("soundfont")? {
        let kind = cx.app.project.tracks[ti].kind;
        if !matches!(kind, TrackKind::Instrument | TrackKind::Drummer) {
            return Err(RpcError::new(
                Code::NotSupported,
                "only instrument and drummer tracks play a SoundFont",
            ));
        }
        let text = path.display().to_string();
        crate::soundfonts::load(&text).map_err(RpcError::failed)?;
        let t = &mut cx.app.project.tracks[ti];
        t.soundfont = Some(crate::model::SoundFontUse {
            path: text.clone(),
            bank: params.i64("bank").unwrap_or(0).clamp(0, 16383) as u16,
            program: params.i64("program").unwrap_or(0).clamp(0, 127) as u8,
        });
        t.au_instrument = None;
        cx.app.project.song_rev += 1;
        return ok(
            json!({"track": cx.ids().track_id(ti), "instrument": {"id": "soundfont", "name": text}}),
        );
    }
    let Some(id) = params.str("id") else {
        return Err(RpcError::invalid(
            "id",
            "give `id` (an instrument's id or name from plugin.list) or `soundfont` (the path of an .sf2 file)",
        ));
    };
    let slot = set_instrument(cx, ti, id, params.bool("open_window") == Some(true))?;
    ok(json!({
        "track": cx.ids().track_id(ti),
        "instrument": slot,
        "note": "a plug-in opens on the next frame: plugin.params lists its parameters once it runs (rpc.wait gives it the time); ui.status says why if it could not be opened",
    }))
}

fn instrument_remove(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let t = &mut cx.app.project.tracks[ti];
    let had = t.au_instrument.is_some() || t.soundfont.is_some();
    t.au_instrument = None;
    if t.soundfont.take().is_some() {
        cx.app.project.song_rev += 1;
    }
    ok(json!({"track": cx.ids().track_id(ti), "removed": had}))
}

// ------------------------------------------------------------------ effects

fn fx_json(pr: &Project, strip: usize, fx: FxRef) -> Value {
    let all = state::effects_json(pr, strip);
    let inserts = state::inserts_of(pr, strip).map_or(0, Vec::len);
    let i = match fx {
        FxRef::Insert(k) => k,
        FxRef::Plugin(j) => inserts + j,
    };
    all.get(i).cloned().unwrap_or(Value::Null)
}

fn req_fx(cx: &Cx, strip: usize, params: &P) -> Result<FxRef, RpcError> {
    match params.get("effect") {
        Some(v) => state::fx_ref(&cx.app.project, strip, v, "effect"),
        None => Err(RpcError::invalid(
            "effect",
            "`effect` is needed: \"insert:1\" (a built-in effect by its slot), \"plugin:<uid>\" or the effect's name",
        )),
    }
}

fn effect_add(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let text = params.req_str("id")?;
    let low = text.to_lowercase();
    let builtin = FxKind::ALL.into_iter().find(|k| {
        text.strip_prefix("builtin:")
            .is_some_and(|n| n.eq_ignore_ascii_case(k.name()))
            || k.name().to_lowercase() == low
    });
    let window_before = cx.app.ui.mixer.fx_window;
    let open = params.bool("open_window") == Some(true);
    let fx = if let Some(kind) = builtin {
        let before = state::inserts_of(&cx.app.project, strip).map_or(0, Vec::len);
        if before >= prowave_engine::MAX_INSERTS {
            return Err(RpcError::failed(format!(
                "a strip has {} built-in effects at most",
                prowave_engine::MAX_INSERTS
            )));
        }
        crate::mixer::apply_insert(
            &mut cx.app.project,
            &mut cx.app.ui.mixer,
            strip,
            0,
            InsertAct::Add(kind),
        );
        if !open {
            // The menu opens the new effect's window; a call opens it only when asked.
            cx.app.ui.mixer.fx_window = window_before;
        }
        FxRef::Insert(before)
    } else {
        let slot = slot_for(cx, text, false)?;
        crate::plugins::release(&slot.id);
        if open {
            cx.app.ui.mixer.open_plugin = Some((slot.uid, slot.name.clone()));
        }
        let Some((list, _)) = cx.app.project.strip_plugins_mut(strip) else {
            return Err(RpcError::not_found("that strip is gone"));
        };
        list.push(slot);
        FxRef::Plugin(list.len() - 1)
    };
    cx.ctx.request_repaint();
    let pr = &cx.app.project;
    ok(json!({
        "track": state::strip_id(&cx.core.ids, strip),
        "effect": fx_json(pr, strip, fx),
        "effects": state::effects_json(pr, strip),
    }))
}

fn effect_act(cx: &mut Cx, strip: usize, fx: FxRef, act: InsertAct) {
    let (pr, v) = (&mut cx.app.project, &mut cx.app.ui.mixer);
    match fx {
        FxRef::Insert(k) => crate::mixer::apply_insert(pr, v, strip, k, act),
        FxRef::Plugin(j) => crate::mixer::apply_plugin(pr, v, strip, j, act),
    }
    cx.ctx.request_repaint();
}

fn effect_remove(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let fx = req_fx(cx, strip, params)?;
    let removed = fx_json(&cx.app.project, strip, fx);
    effect_act(cx, strip, fx, InsertAct::Remove);
    ok(json!({
        "track": state::strip_id(&cx.core.ids, strip),
        "removed": removed,
        "effects": state::effects_json(&cx.app.project, strip),
    }))
}

fn effect_bypass(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let fx = req_fx(cx, strip, params)?;
    let is = |pr: &Project| match fx {
        FxRef::Insert(k) => state::inserts_of(pr, strip)
            .and_then(|l| l.get(k))
            .is_some_and(|i| i.bypass),
        FxRef::Plugin(j) => pr
            .strip_plugins(strip)
            .and_then(|l| l.get(j))
            .is_some_and(|s| s.bypass),
    };
    let want = params.bool("on").unwrap_or(!is(&cx.app.project));
    if is(&cx.app.project) != want {
        effect_act(cx, strip, fx, InsertAct::Bypass);
    }
    ok(json!({
        "track": state::strip_id(&cx.core.ids, strip),
        "effect": fx_json(&cx.app.project, strip, fx),
    }))
}

fn effect_move(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let mut fx = req_fx(cx, strip, params)?;
    let by = params.i64("by").unwrap_or(0);
    let (count, at) = match fx {
        FxRef::Insert(k) => (
            state::inserts_of(&cx.app.project, strip).map_or(0, Vec::len),
            k,
        ),
        FxRef::Plugin(j) => (cx.app.project.strip_plugins(strip).map_or(0, Vec::len), j),
    };
    let to = (at as i64 + by).clamp(0, count.saturating_sub(1) as i64) as usize;
    let mut now = at;
    while now != to {
        let step: isize = if to > now { 1 } else { -1 };
        effect_act(cx, strip, fx, InsertAct::Move(step));
        now = now.saturating_add_signed(step);
        fx = match fx {
            FxRef::Insert(_) => FxRef::Insert(now),
            FxRef::Plugin(_) => FxRef::Plugin(now),
        };
    }
    ok(json!({
        "track": state::strip_id(&cx.core.ids, strip),
        "effect": fx_json(&cx.app.project, strip, fx),
        "effects": state::effects_json(&cx.app.project, strip),
        "note": "built-in effects come before plug-ins in a strip; each kind is ordered among its own",
    }))
}

// ------------------------------------------------------------------ parameters

fn strip_of(strip: usize) -> Strip {
    automation::strip_of(strip)
}

/// Which device of the strip a parameter belongs to, as the reference the other methods take.
fn owner_ref(pr: &Project, strip: usize, target: AutoTarget) -> String {
    match target {
        AutoTarget::Fx { slot, .. } => format!("insert:{}", usize::from(slot) + 1),
        AutoTarget::Plugin { slot, .. } => automation::strip_plugin_slot(pr, strip_of(strip), slot)
            .map_or_else(|| "plugin:0".to_string(), |s| format!("plugin:{}", s.uid)),
        _ => "mixer".to_string(),
    }
}

/// The value of a parameter now, in its own units.
fn param_value(cx: &Cx, strip: usize, desc: &ParamDesc) -> Option<f32> {
    let pr = &cx.app.project;
    match desc.target {
        AutoTarget::Fx { slot, param } => state::inserts_of(pr, strip)?
            .get(usize::from(slot))?
            .params
            .get(usize::from(param))
            .copied(),
        AutoTarget::Plugin { slot, param } => {
            let s = automation::strip_plugin_slot(pr, strip_of(strip), slot)?;
            #[cfg(target_os = "macos")]
            if let Some(h) = cx.app.plugins.running(s.uid) {
                return h.value(param);
            }
            automation::plugin_values(s, slot)
                .into_iter()
                .find(|(t, _)| *t == desc.target)
                .map(|(_, n)| desc.value_at(n))
        }
        target if strip == MASTER_STRIP => automation::master_value(pr, target),
        target => automation::own_value(pr.tracks.get(strip)?, target),
    }
}

fn param_json(cx: &Cx, strip: usize, desc: &ParamDesc) -> Value {
    let value = param_value(cx, strip, desc);
    let mut o = json!({
        "param": automation::target_key(desc.target),
        "name": desc.name,
        "min": desc.lo,
        "max": desc.hi,
        "value": value.map(|v| round6(f64::from(v))),
    });
    if let Some(map) = o.as_object_mut() {
        if !desc.unit.is_empty() {
            map.insert("unit".into(), desc.unit.clone().into());
        }
        if desc.stepped {
            map.insert("stepped".into(), true.into());
        }
        if !desc.choices.is_empty() {
            map.insert("choices".into(), json!(desc.choices));
        }
        if let Some(v) = value {
            map.insert(
                "text".into(),
                automation::value_text(desc, desc_lane_value(desc, v)).into(),
            );
        }
    }
    o
}

/// A value in the parameter's own units as its lane holds it: dB, the pan and the switches
/// as they are, an effect's or a plug-in's parameter as a position 0..=1.
fn desc_lane_value(desc: &ParamDesc, own: f32) -> f32 {
    match desc.target {
        AutoTarget::Fx { .. } | AutoTarget::Plugin { .. } => desc.position_of(own),
        _ => own,
    }
}

fn devices(cx: &Cx, strip: usize) -> Vec<automation::Device> {
    automation::devices(&cx.app.project, strip_of(strip), &cx.app.ui.auto.au)
}

/// The description of the parameter a request names on `strip` ("volume", "pan", "mute",
/// "send:0", "fx:0:3", "plugin:1:42", or a parameter's name with `effect`/`instrument`).
fn req_param(cx: &Cx, strip: usize, params: &P) -> Result<ParamDesc, RpcError> {
    let text = params.req_str("param")?;
    let all: Vec<ParamDesc> = devices(cx, strip)
        .into_iter()
        .flat_map(|d| d.params)
        .collect();
    if let Some(target) = automation::target_from_key(text) {
        if let Some(d) = all.iter().find(|d| d.target == target) {
            return Ok(d.clone());
        }
    }
    // By name, among the parameters of the device named (or of the whole strip).
    let owner = match (params.get("effect"), params.bool("instrument")) {
        (Some(v), _) => Some(match state::fx_ref(&cx.app.project, strip, v, "effect")? {
            FxRef::Insert(k) => format!("insert:{}", k + 1),
            FxRef::Plugin(j) => cx
                .app
                .project
                .strip_plugins(strip)
                .and_then(|l| l.get(j))
                .map_or_else(String::new, |s| format!("plugin:{}", s.uid)),
        }),
        (None, Some(true)) => cx
            .app
            .project
            .tracks
            .get(strip)
            .and_then(|t| t.au_instrument.as_ref())
            .map(|s| format!("plugin:{}", s.uid)),
        _ => None,
    };
    let low = text.to_lowercase();
    let named: Vec<&ParamDesc> = all
        .iter()
        .filter(|d| d.name.to_lowercase() == low)
        .filter(|d| {
            owner
                .as_ref()
                .is_none_or(|o| owner_ref(&cx.app.project, strip, d.target) == *o)
        })
        .collect();
    match named.as_slice() {
        [d] => Ok((*d).clone()),
        [] => Err(RpcError::not_found(format!(
            "the strip has no parameter \"{text}\": plugin.params lists them (a plug-in's parameters are there once it runs); a parameter is named by its `param` key (\"volume\", \"fx:0:3\", \"plugin:1:42\") or by its name"
        ))),
        many => Err(RpcError::invalid(
            "param",
            format!(
                "{} parameters of the strip are named \"{text}\": use a key: {}",
                many.len(),
                many.iter()
                    .map(|d| automation::target_key(d.target))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn plugin_params(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let only = match (params.get("effect"), params.bool("instrument")) {
        (Some(v), _) => Some(match state::fx_ref(&cx.app.project, strip, v, "effect")? {
            FxRef::Insert(k) => format!("insert:{}", k + 1),
            FxRef::Plugin(j) => cx
                .app
                .project
                .strip_plugins(strip)
                .and_then(|l| l.get(j))
                .map_or_else(String::new, |s| format!("plugin:{}", s.uid)),
        }),
        (None, Some(true)) => Some(
            cx.app
                .project
                .tracks
                .get(strip)
                .and_then(|t| t.au_instrument.as_ref())
                .map(|s| format!("plugin:{}", s.uid))
                .ok_or_else(|| RpcError::not_found("the track has no plug-in instrument"))?,
        ),
        _ => None,
    };
    let mut groups: Vec<(String, String, Vec<Value>)> = Vec::new();
    for d in devices(cx, strip) {
        let Some(first) = d.params.first() else {
            continue;
        };
        let owner = owner_ref(&cx.app.project, strip, first.target);
        if only.as_ref().is_some_and(|o| *o != owner) {
            continue;
        }
        let list = d
            .params
            .iter()
            .map(|desc| param_json(cx, strip, desc))
            .collect();
        groups.push((owner, d.name.clone(), list));
    }
    // Plug-ins of the strip that list no parameters (not running, or none to automate).
    let pr = &cx.app.project;
    let mut slots: Vec<&PluginSlot> = Vec::new();
    if let Some(t) = pr.tracks.get(strip) {
        slots.extend(t.au_instrument.iter());
    }
    slots.extend(pr.strip_plugins(strip).into_iter().flatten());
    let mut out: Vec<Value> = groups
        .into_iter()
        .map(|(owner, name, list)| json!({"device": owner, "name": name, "running": true, "params": list}))
        .collect();
    for s in slots {
        let owner = format!("plugin:{}", s.uid);
        let listed = out
            .iter()
            .any(|d| d.get("device").and_then(Value::as_str) == Some(owner.as_str()));
        if !listed && only.as_ref().is_none_or(|o| *o == owner) {
            out.push(json!({
                "device": owner,
                "name": s.name,
                "running": false,
                "params": [],
                "note": "this plug-in lists no parameters: it is still opening (try again after rpc.wait), could not be opened (ui.status says why), or has none",
            }));
        }
    }
    ok(json!({"track": state::strip_id(&cx.core.ids, strip), "devices": out}))
}

fn param_get(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let desc = req_param(cx, strip, params)?;
    let mut out = param_json(cx, strip, &desc);
    if let Some(o) = out.as_object_mut() {
        o.insert(
            "device".into(),
            owner_ref(&cx.app.project, strip, desc.target).into(),
        );
        let lane = automation::auto_of(&cx.app.project, strip_of(strip))
            .map_or(0, |a| a.lane(desc.target).len());
        o.insert("automation_points".into(), lane.into());
    }
    ok(out)
}

fn param_set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let desc = req_param(cx, strip, params)?;
    let mut value = params.req_f64("value")? as f32;
    if !value.is_finite() {
        return Err(RpcError::invalid(
            "value",
            "`value` is too large to be a setting",
        ));
    }
    if desc.hi > desc.lo && (value < desc.lo || value > desc.hi) {
        return Err(RpcError::invalid(
            "value",
            format!(
                "\"{}\" goes from {} to {}{}; got {value}",
                desc.name,
                desc.lo,
                desc.hi,
                if desc.unit.is_empty() {
                    String::new()
                } else {
                    format!(" {}", desc.unit)
                }
            ),
        )
        .with("min", desc.lo)
        .with("max", desc.hi));
    }
    if desc.stepped {
        value = value.round();
    }
    let pr = &mut cx.app.project;
    let (beat, playing) = (pr.position_beats, pr.playing);
    match desc.target {
        AutoTarget::Fx { slot, param } => {
            // As the knob in the effect's window: the setting itself, in its own units.
            let ins = if strip == MASTER_STRIP {
                pr.master_inserts.get_mut(usize::from(slot))
            } else {
                pr.tracks
                    .get_mut(strip)
                    .and_then(|t| t.inserts.get_mut(usize::from(slot)))
            };
            if let Some(v) = ins.and_then(|i| i.params.get_mut(usize::from(param))) {
                *v = value;
            }
        }
        AutoTarget::Plugin { slot, param } => {
            let Some(s) = automation::strip_plugin_slot(pr, strip_of(strip), slot).cloned() else {
                return Err(RpcError::not_found("that plug-in is gone"));
            };
            if let Some(info) = crate::native_inst_ui::any_info(&s.id) {
                // A native effect keeps its settings in the slot; the running effect hears
                // of the move the way it does from its window.
                let mut values = crate::native_fx_ui::values(&s, info);
                let index = param as usize;
                if let Some(v) = values.get_mut(index) {
                    *v = value;
                }
                let bytes = crate::native_fx_ui::encode(&values);
                if let Some(live) = crate::plugins::slots_mut(pr).find(|x| x.uid == s.uid) {
                    live.state = Some(bytes);
                }
                cx.app.ui.native_edits.push((strip, s.uid, index, value));
            } else {
                #[cfg(target_os = "macos")]
                let handle = cx.app.plugins.running(s.uid);
                #[cfg(target_os = "macos")]
                match handle {
                    // As a hand on the plug-in's own knob (its window follows, automation
                    // hears it as a touch).
                    Some(h) => h.edit(param, value),
                    None => {
                        return Err(RpcError::new(
                            Code::NotSupported,
                            format!(
                                "\"{}\" keeps this parameter in its own settings and it is set in its window; only parameters of built-in effects, native effects and running Audio Units or VST3s can be set here",
                                s.name
                            ),
                        ))
                    }
                }
                #[cfg(not(target_os = "macos"))]
                return Err(RpcError::new(
                    Code::NotSupported,
                    "plug-ins run on macOS only",
                ));
            }
        }
        target if strip == MASTER_STRIP => {
            if target == AutoTarget::Volume {
                pr.master_vol = 10f32.powf(value / 20.0).min(2.0);
            }
        }
        AutoTarget::Mute => pr.set_mute(strip, value >= 0.5),
        target => {
            if let Some(t) = pr.tracks.get_mut(strip) {
                // As a hand on the control: an envelope on it is overridden until re-enabled.
                automation::moved(t, target, value);
                automation::released(t, target, beat, playing);
            }
        }
    }
    cx.ctx.request_repaint();
    let mut out = param_json(cx, strip, &desc);
    if let Some(o) = out.as_object_mut() {
        o.insert("set".into(), json!(round6(f64::from(value))));
    }
    ok(out)
}

/// The plug-in slot a request names with `effect` or `instrument`.
fn req_slot(cx: &Cx, strip: usize, params: &P) -> Result<PluginSlot, RpcError> {
    if params.bool("instrument") == Some(true) {
        return cx
            .app
            .project
            .tracks
            .get(strip)
            .and_then(|t| t.au_instrument.clone())
            .ok_or_else(|| RpcError::not_found("the track has no plug-in instrument"));
    }
    match req_fx(cx, strip, params)? {
        FxRef::Plugin(j) => cx
            .app
            .project
            .strip_plugins(strip)
            .and_then(|l| l.get(j))
            .cloned()
            .ok_or_else(|| RpcError::not_found("that plug-in is gone")),
        FxRef::Insert(_) => Err(RpcError::new(
            Code::NotSupported,
            "the built-in effects have no presets of their own; set their parameters with param.set",
        )),
    }
}

fn preset_list(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let slot = req_slot(cx, strip, params)?;
    let mut current = Value::Null;
    let presets: Vec<Value> = if let Some(info) = crate::native_inst_ui::any_info(&slot.id) {
        info.presets
            .iter()
            .enumerate()
            .map(|(n, pre)| json!({"number": n, "name": pre.name}))
            .collect()
    } else {
        #[cfg(target_os = "macos")]
        {
            let Some(h) = cx.app.plugins.running(slot.uid) else {
                return Err(RpcError::new(
                    Code::NotSupported,
                    format!("\"{}\" is not a running Audio Unit or VST3, or has no presets the app can list", slot.name),
                ));
            };
            if let Some((n, name)) = h.preset() {
                current = json!({"number": n, "name": name});
            }
            h.presets()
                .into_iter()
                .map(|(n, name)| json!({"number": n, "name": name}))
                .collect()
        }
        #[cfg(not(target_os = "macos"))]
        Vec::new()
    };
    ok(json!({
        "plugin": state::slot_json(&slot),
        "presets": presets,
        "current": current,
        "note": "Audio Units list their factory presets; a VST3's programs are not read by the app",
    }))
}

fn preset_load(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let slot = req_slot(cx, strip, params)?;
    let want_name = params.str("name").map(str::to_lowercase);
    let want_number = params.i64("number");
    if want_name.is_none() && want_number.is_none() {
        return Err(RpcError::invalid(
            "name",
            "give the preset's `name` or its `number` (preset.list gives both)",
        ));
    }
    if let Some(info) = crate::native_inst_ui::any_info(&slot.id) {
        let found = info.presets.iter().enumerate().find(|(n, pre)| {
            want_number == Some(*n as i64)
                || want_name.as_deref() == Some(pre.name.to_lowercase().as_str())
        });
        let Some((n, pre)) = found else {
            return Err(RpcError::not_found(format!(
                "\"{}\" has no such preset; it has: {}",
                slot.name,
                info.presets
                    .iter()
                    .map(|p| p.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        let mut values = crate::native_fx_ui::values(&slot, info);
        pre.apply(&mut values);
        let bytes = crate::native_fx_ui::encode(&values);
        if let Some(live) =
            crate::plugins::slots_mut(&mut cx.app.project).find(|x| x.uid == slot.uid)
        {
            live.state = Some(bytes);
        }
        for (index, value) in values.iter().enumerate() {
            cx.app
                .ui
                .native_edits
                .push((strip, slot.uid, index, *value));
        }
        cx.ctx.request_repaint();
        return ok(json!({"loaded": {"number": n, "name": pre.name}}));
    }
    #[cfg(target_os = "macos")]
    {
        let Some(h) = cx.app.plugins.running(slot.uid) else {
            return Err(RpcError::new(
                Code::NotSupported,
                format!(
                    "\"{}\" is not a running Audio Unit, or has no presets the app can load",
                    slot.name
                ),
            ));
        };
        let list = h.presets();
        let found = list.iter().find(|(n, name)| {
            want_number == Some(i64::from(*n))
                || want_name.as_deref() == Some(name.to_lowercase().as_str())
        });
        let Some((n, name)) = found else {
            return Err(RpcError::not_found(format!(
                "\"{}\" has no such preset (preset.list gives its {} presets)",
                slot.name,
                list.len()
            )));
        };
        if !h.set_preset(*n) {
            return Err(RpcError::failed(format!(
                "\"{}\" did not take preset {n} \"{name}\"",
                slot.name
            )));
        }
        cx.ctx.request_repaint();
        ok(json!({"loaded": {"number": n, "name": name}}))
    }
    #[cfg(not(target_os = "macos"))]
    Err(RpcError::new(
        Code::NotSupported,
        "plug-ins run on macOS only",
    ))
}

fn plugin_window(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let open = params.bool("open").unwrap_or(true);
    if params.bool("instrument") != Some(true) {
        if let FxRef::Insert(k) = req_fx(cx, strip, params)? {
            let v = &mut cx.app.ui.mixer;
            if open {
                v.fx_window = Some((strip, k));
            } else if v.fx_window == Some((strip, k)) {
                v.fx_window = None;
            }
            cx.ctx.request_repaint();
            return ok(json!({"effect": format!("insert:{}", k + 1), "open": open}));
        }
    }
    let slot = req_slot(cx, strip, params)?;
    if open {
        // The frame that follows opens it: the Channel Rack in the bottom panel, a native
        // effect's unit, or the plug-in's own window.
        cx.app.ui.mixer.open_plugin = Some((slot.uid, slot.name.clone()));
    } else {
        if cx.app.ui.native_window == Some(slot.uid) {
            cx.app.ui.native_window = None;
        }
        if cx.app.ui.native_inst_window == Some(slot.uid) {
            cx.app.ui.native_inst_window = None;
        }
        cx.app.plugins.close_window(slot.uid);
    }
    cx.ctx.request_repaint();
    ok(json!({
        "plugin": state::slot_json(&slot),
        "open": open,
        "open_before": cx.app.plugins.window_is_open(slot.uid),
        "note": "a plug-in's window is on the Mac's screen, not in ui.screenshot's picture; it opens or closes on the next frame",
    }))
}

// ------------------------------------------------------------------ automation

/// A lane's value in the parameter's own units.
fn lane_to_own(cx: &Cx, strip: usize, desc: &ParamDesc, lane: f32) -> f32 {
    match desc.target {
        AutoTarget::Fx { slot, param } => state::inserts_of(&cx.app.project, strip)
            .and_then(|l| l.get(usize::from(slot)))
            .and_then(|i| i.kind.params().get(usize::from(param)))
            .map_or(lane, |info| info.from_norm(lane)),
        AutoTarget::Plugin { .. } => desc.value_at(lane),
        _ => lane,
    }
}

fn own_to_lane(cx: &Cx, strip: usize, desc: &ParamDesc, own: f32) -> f32 {
    match desc.target {
        AutoTarget::Fx { slot, param } => state::inserts_of(&cx.app.project, strip)
            .and_then(|l| l.get(usize::from(slot)))
            .and_then(|i| i.kind.params().get(usize::from(param)))
            .map_or(own, |info| info.to_norm(own)),
        AutoTarget::Plugin { .. } => desc.position_of(own),
        _ => own,
    }
}

fn lane_json(cx: &Cx, strip: usize, desc: &ParamDesc) -> Value {
    let pr = &cx.app.project;
    let points: Vec<Value> = automation::auto_of(pr, strip_of(strip))
        .map(|a| a.lane(desc.target))
        .unwrap_or(&[])
        .iter()
        .map(|pt| {
            json!({
                "at": place_json(pr, pt.beat),
                "value": round6(f64::from(lane_to_own(cx, strip, desc, pt.value))),
                "curve": pt.curve,
            })
        })
        .collect();
    json!({
        "param": automation::target_key(desc.target),
        "name": desc.name,
        "unit": desc.unit,
        "min": desc.lo,
        "max": desc.hi,
        "points": points,
    })
}

fn automation_get(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let lanes: Vec<Value> = if params.has("param") {
        let desc = req_param(cx, strip, params)?;
        vec![lane_json(cx, strip, &desc)]
    } else {
        let all: Vec<ParamDesc> = devices(cx, strip)
            .into_iter()
            .flat_map(|d| d.params)
            .collect();
        let Some(auto) = automation::auto_of(&cx.app.project, strip_of(strip)) else {
            return Err(RpcError::not_found("that strip is gone"));
        };
        all.iter()
            .filter(|d| !auto.lane(d.target).is_empty())
            .map(|d| lane_json(cx, strip, d))
            .collect()
    };
    ok(json!({"track": state::strip_id(&cx.core.ids, strip), "lanes": lanes}))
}

fn automation_set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let desc = req_param(cx, strip, params)?;
    let Some(list) = params.arr("points") else {
        return Err(RpcError::invalid(
            "points",
            "`points` is needed: [{\"bar\": 1, \"value\": -6}, ...]",
        ));
    };
    // Every point is read before the lane is touched, so a bad one changes nothing.
    let mut points: Vec<(f64, f32)> = Vec::with_capacity(list.len());
    for (i, v) in list.iter().enumerate() {
        let Some(o) = v.as_object() else {
            return Err(RpcError::invalid(
                "points",
                format!(
                    "point {} must be an object like {{\"bar\": 5, \"value\": -6}}",
                    i + 1
                ),
            ));
        };
        let at = req_place(&cx.app.project, &P(o), "")
            .map_err(|e| RpcError::invalid("points", format!("point {}: {}", i + 1, e.message)))?;
        let Some(value) = o
            .get("value")
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite())
        else {
            return Err(RpcError::invalid(
                "points",
                format!("point {} needs a number `value`", i + 1),
            ));
        };
        let value = value as f32;
        if !value.is_finite() || (desc.hi > desc.lo && (value < desc.lo || value > desc.hi)) {
            return Err(RpcError::invalid(
                "points",
                format!(
                    "point {}: \"{}\" goes from {} to {}; got {value}",
                    i + 1,
                    desc.name,
                    desc.lo,
                    desc.hi
                ),
            ));
        }
        points.push((at, own_to_lane(cx, strip, &desc, value)));
    }
    let plugin_name = match desc.target {
        AutoTarget::Plugin { slot, .. } => {
            automation::strip_plugin_slot(&cx.app.project, strip_of(strip), slot)
                .map(|s| s.id.clone())
        }
        _ => None,
    };
    let replace = params.bool("replace") == Some(true);
    let Some(auto) = automation::auto_of_mut(&mut cx.app.project, strip_of(strip)) else {
        return Err(RpcError::not_found("that strip is gone"));
    };
    if replace {
        auto.clear_lane(desc.target);
    }
    if let Some(id) = &plugin_name {
        auto.plugin_lane(&desc, id);
    }
    for (beat, value) in points {
        auto.set_point(desc.target, beat, value);
    }
    cx.ctx.request_repaint();
    ok(json!({
        "track": state::strip_id(&cx.core.ids, strip),
        "lane": lane_json(cx, strip, &desc),
    }))
}

fn automation_clear(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let strip = state::req_strip(&cx.app.project, &cx.core.ids, params, "track")?;
    let target = if params.has("param") {
        Some(req_param(cx, strip, params)?.target)
    } else if params.bool("all") == Some(true) {
        None
    } else {
        return Err(RpcError::invalid(
            "param",
            "give `param` (the lane to empty) or all: true (every lane of the strip)",
        ));
    };
    let Some(auto) = automation::auto_of_mut(&mut cx.app.project, strip_of(strip)) else {
        return Err(RpcError::not_found("that strip is gone"));
    };
    match target {
        Some(t) => auto.clear_lane(t),
        None => auto.clear(),
    }
    ok(
        json!({"track": state::strip_id(&cx.core.ids, strip), "cleared": target.map(automation::target_key)}),
    )
}

// ------------------------------------------------------------------ live notes

fn midi_note(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let pitch = params.i64("pitch").unwrap_or(60).clamp(0, 127) as u8;
    let on = params.bool("on").unwrap_or(true);
    if on {
        let ti = match params.get("track") {
            Some(v) => state::track_ref(&cx.app.project, &cx.core.ids, v, "track")?,
            None => cx.app.project.selected_track,
        };
        let vel = params.i64("velocity").unwrap_or(100).clamp(1, 127) as u8;
        cx.app.ui.audition.push(crate::piano_roll::Audition::On {
            pitch,
            vel,
            track: ti as u16,
        });
    } else {
        cx.app
            .ui
            .audition
            .push(crate::piano_roll::Audition::Off { pitch });
    }
    cx.ctx.request_repaint();
    ok(json!({"pitch": pitch, "on": on, "audio_running": cx.app.sync.is_some()}))
}

fn midi_panic(cx: &mut Cx, _: &P) -> Result<Reply, RpcError> {
    cx.app.ui.midi_panic = true;
    cx.app.ui.audition.clear();
    cx.ctx.request_repaint();
    ok(json!({"sent": true}))
}

pub fn methods() -> Vec<Method> {
    let track = || {
        p(
            "track",
            Ty::Any,
            "the track: its id (\"t3\"), its name, or its number in the list (from 1)",
        )
        .req()
    };
    let strip = || {
        p(
            "track",
            Ty::Any,
            "the channel strip: a track's id, name or number, or \"master\" for the Stereo Out",
        )
        .req()
    };
    let effect = || {
        p("effect", Ty::Str, "an effect of the strip: \"insert:1\" (a built-in effect by its slot, from 1), \"plugin:<uid>\" (as state.get gives it) or its name")
    };
    let instrument = || {
        p(
            "instrument",
            Ty::Bool,
            "true: the track's instrument instead of an effect",
        )
    };
    let param = || {
        p("param", Ty::Str, "the parameter: its key from plugin.params (\"volume\", \"pan\", \"mute\", \"send:0\", \"fx:0:3\", \"plugin:1:42\") or its name").req()
    };
    vec![
        m("track.list", "The tracks in order, without their regions.",
            vec![], "{tracks: [track.get without regions]}", track_list),
        m("track.get", "One track: its strip settings, instrument, effects, sends, automation lanes and regions.",
            vec![track(), p("notes", Ty::Bool, "include the notes of its MIDI regions (default true)")],
            "{id, number, name, kind, selected, mute, solo, arm, monitor, volume_db, pan, color, output, input?, instrument?, effects: [{ref, id, name, format, bypass, values?}], sends, indent, vca?, group?, automation: [{param, points}], regions: [region.get]}",
            track_get),
        m("track.add", "Adds a track at the end of the list and selects it (Track > New ... Track).",
            vec![
                p("kind", Ty::Str, "the kind of track (default instrument)").vals(&["instrument", "audio", "drummer", "aux", "vca", "external_midi"]),
                p("name", Ty::Str, "its name (default: the app's, such as \"Inst 2\")"),
                p("instrument", Ty::Str, "for an instrument track: the instrument to load, an id or name from plugin.list"),
            ],
            "the new track as track.get gives it", track_add).changes(),
        m("track.remove", "Removes a track with its regions (Track > Delete Track). A folder is released instead: its tracks stay.",
            vec![track()], "{removed, name, tracks}", track_remove).changes(),
        m("track.rename", "Renames a track.",
            vec![track(), p("name", Ty::Str, "the new name").req()], "{id, name}", track_rename).changes(),
        m("track.move", "Puts a track at another place in the list.",
            vec![track(), p("to", Ty::Int, "its new number in the list, from 1").req().range(1.0, 1024.0)],
            "{id, number, order: [ids]}; the app has no gesture for this, so it is done here with the project's own renumbering; folder and VCA tracks cannot be moved",
            track_move).changes(),
        m("track.set", "Sets a track's strip: mute, solo, record arm, monitoring, level, pan, input, output, VCA, name.",
            vec![
                track(),
                p("mute", Ty::Bool, "mute (a folder mutes its tracks)"),
                p("solo", Ty::Bool, "solo"),
                p("arm", Ty::Bool, "record arm (R)"),
                p("monitor", Ty::Bool, "input monitoring (I), audio tracks"),
                p("volume_db", Ty::Num, "the fader").unit("dB").range(-60.0, 6.0),
                p("pan", Ty::Num, "the pan knob, -1 left to 1 right").range(-1.0, 1.0),
                p("input", Ty::Str, "audio tracks: \"mono:N\" or \"stereo:N\", N the first input channel from 1"),
                p("output", Ty::Str, "\"stereo_out\" or \"bus:1\" to \"bus:8\""),
                p("vca", Ty::Any, "the VCA track that moves this track's level (id, name or number), or \"none\""),
                p("name", Ty::Str, "a new name"),
            ],
            "the track as track.list gives it", track_set).changes(),
        m("track.send", "Adds, changes or removes a send from a track to a bus.",
            vec![
                track(),
                p("bus", Ty::Int, "the bus").req().range(1.0, 8.0),
                p("level", Ty::Num, "the send knob, 0 to 1").range(0.0, 1.0),
                p("pre_fader", Ty::Bool, "send before the fader"),
                p("remove", Ty::Bool, "take the send away"),
            ],
            "the track as track.list gives it; a bus gets its Aux strip when it has none", track_send).changes(),
        m("plugin.list", "The instruments and effects that can be loaded: the app's own, and the Audio Units and VST3s on this Mac. Hidden ones (the settings file plugin-hidden; Apple's Audio Units unless plugin-hide-apple holds 0) are left out.",
            vec![
                p("kind", Ty::Str, "only instruments or only effects").vals(&["instrument", "effect"]),
                p("search", Ty::Str, "only those whose name, maker, id or emulated hardware (emulates) contains this text"),
            ],
            "{count, plugins: [{id, name, maker, kind, format: built-in|AU|VST3, vst3_id?, turned_off_after_crash?, rating?, type?, emulates?, master_chain?}]}", plugin_list),
        m("instrument.set", "Loads an instrument on an instrument track, in place of the one it has.",
            vec![
                track(),
                p("id", Ty::Str, "the instrument: an id or name from plugin.list"),
                p("soundfont", Ty::Str, "instead of id: the path of a SoundFont (.sf2) file"),
                p("bank", Ty::Int, "with soundfont: the preset's bank (128 for drum kits)").range(0.0, 16383.0),
                p("program", Ty::Int, "with soundfont: the preset's program").range(0.0, 127.0),
                p("open_window", Ty::Bool, "also open the instrument's window on the Mac's screen (default false)"),
            ],
            "{track, instrument: {ref, id, name, format, bypass}}", instrument_set).changes(),
        m("instrument.remove", "Takes the instrument off a track.",
            vec![track()], "{track, removed}", instrument_remove).changes(),
        m("effect.add", "Adds an effect at the end of a strip's effects (a track's, or the Stereo Out's).",
            vec![
                strip(),
                p("id", Ty::Str, "the effect: an id or name from plugin.list (\"builtin:Compressor\", \"Channel EQ\", an Audio Unit's id ...)").req(),
                p("open_window", Ty::Bool, "also open the effect's window (default false)"),
            ],
            "{track, effect: {ref, id, name, format, bypass}, effects}", effect_add).changes(),
        m("effect.remove", "Removes an effect from a strip; its automation goes with it.",
            vec![strip(), effect().req()], "{track, removed, effects}", effect_remove).changes(),
        m("effect.bypass", "Bypasses an effect or turns it back on.",
            vec![strip(), effect().req(), p("on", Ty::Bool, "true bypasses, false turns it on (default: the other state)")],
            "{track, effect}", effect_bypass).changes(),
        m("effect.move", "Moves an effect up or down among the strip's effects of its kind.",
            vec![strip(), effect().req(), p("by", Ty::Int, "slots to move: negative is earlier in the chain").req().range(-64.0, 64.0)],
            "{track, effect, effects}", effect_move).changes(),
        m("plugin.params", "Every parameter of a strip, by device: the mixer (volume, pan, mute, sends), each built-in effect, the instrument and each plug-in, with its range and value now.",
            vec![strip(), effect(), instrument()],
            "{track, devices: [{device: mixer|insert:N|plugin:<uid>, name, running, params: [{param, name, min, max, value, text, unit?, stepped?, choices?}]}]}; with effect or instrument, only that device",
            plugin_params),
        m("param.get", "One parameter's value now, in its own units.",
            vec![strip(), param(), effect(), instrument()],
            "{param, name, min, max, value, text, unit?, device, automation_points}", param_get),
        m("param.set", "Sets a parameter, as a hand on its control does: the fader, pan, mute, a send, a built-in or native effect's knob, or a running Audio Unit's or VST3's parameter.",
            vec![strip(), param(), p("value", Ty::Num, "the new value in the parameter's own units (plugin.params gives its range; dB for volume, -1..1 for pan, 0 or 1 for mute)").req(), effect(), instrument()],
            "the parameter as param.get gives it, with `set`; a parameter that has automation is overridden by the move, as in the app",
            param_set).changes(),
        m("preset.list", "The presets of a plug-in: an Audio Unit's factory presets, or a native effect's.",
            vec![strip(), effect(), instrument()], "{plugin, presets: [{number, name}], current}", preset_list),
        m("preset.load", "Loads one of a plug-in's presets.",
            vec![strip(), effect(), instrument(), p("name", Ty::Str, "the preset's name"), p("number", Ty::Int, "or its number from preset.list")],
            "{loaded: {number, name}}", preset_load).changes(),
        m("plugin.window", "Opens or closes the window of an instrument or effect.",
            vec![strip(), effect(), instrument(), p("open", Ty::Bool, "true opens (the default), false closes")],
            "{plugin|effect, open}", plugin_window),
        m("automation.get", "The automation of a strip: one parameter's breakpoints, or every lane that has any.",
            vec![strip(), p("param", Ty::Str, "only this parameter (key or name)"), effect(), instrument()],
            "{track, lanes: [{param, name, unit, min, max, points: [{at: {bar, position, beat, seconds}, value, curve}]}]}; values are in the parameter's own units",
            automation_get),
        m("automation.set", "Writes automation breakpoints for a parameter of a strip.",
            vec![
                strip(), param(),
                p("points", Ty::Arr, "the breakpoints: [{\"bar\": 1, \"value\": -6}, ...]; each has a place (bar, position, seconds or beat) and a value in the parameter's own units; a point at a place that has one replaces its value").req(),
                p("replace", Ty::Bool, "empty the lane first (default false: the points are added to it)"),
                effect(), instrument(),
            ],
            "{track, lane}", automation_set).changes(),
        m("automation.clear", "Empties one automation lane of a strip, or all of them.",
            vec![strip(), p("param", Ty::Str, "the lane to empty (key or name)"), p("all", Ty::Bool, "true: every lane of the strip"), effect(), instrument()],
            "{track, cleared}", automation_clear).changes(),
        m("midi.note", "Plays a note on a track's instrument now, as a key of the Piano Roll's keyboard does (it is heard, not recorded into a region).",
            vec![
                p("track", Ty::Any, "the track (default: the selected one)"),
                p("pitch", Ty::Int, "the MIDI note, 60 is middle C").req().range(0.0, 127.0),
                p("velocity", Ty::Int, "how hard (default 100)").range(1.0, 127.0),
                p("on", Ty::Bool, "true starts the note (the default), false ends it"),
            ],
            "{pitch, on, audio_running}; every note started must be ended", midi_note),
        m("midi.panic", "Ends every sounding note (Record > Send All Notes Off).",
            vec![], "{sent}", midi_panic),
    ]
}
