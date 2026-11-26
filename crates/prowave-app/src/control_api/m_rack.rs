//! Methods for the Channel Rack's channels and feel: what a rack holds (each channel's class,
//! the swing and offsets of each class), a channel's class, the swing of a class, a channel's
//! steps, and the groove as a whole. A change of class, swing or groove moves the steps of
//! every plain MIDI region of the rack's track to where the new feel plays them. Also each
//! channel's volume and pan, one hit (a whole step or a part of a split one) with its velocity,
//! splitting a step, and the amounts of swing, offsets and velocity that play.
use super::params::P;
use super::protocol::RpcError;
use super::registry::{m, p, Method, Reply, Ty};
use super::state;
use super::{ok, Cx};
use crate::channel_rack::{self, RackState};
use crate::kit_classify::DrumClass;
use crate::rack_groove::{Cell, Feel, Groove, Sub, MAX_DIV, SLOTS};
use prowave_engine::RACK_BASE_NOTE;
use serde_json::{json, Value};

/// The class ids a channel can have.
const CLASS_IDS: [&str; 16] = [
    "kick",
    "snare",
    "clap",
    "closed_hat",
    "open_hat",
    "rim",
    "tom_low",
    "tom_mid",
    "tom_high",
    "perc",
    "shaker",
    "crash",
    "ride",
    "808",
    "fx",
    "other",
];

/// The rack track a request names, or the selected track when it plays the rack.
pub(super) fn req_rack(cx: &mut Cx, params: &P) -> Result<usize, RpcError> {
    if params.has("track") {
        let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
        if channel_rack::rack_state(&cx.app.project, ti).is_none() {
            return Err(RpcError::failed(format!(
                "track \"{}\" does not play the Channel Rack",
                cx.app.project.tracks[ti].name
            )));
        }
        return Ok(ti);
    }
    channel_rack::rack_track(&cx.app.project).ok_or_else(|| {
        RpcError::failed("the selected track does not play the Channel Rack: name a `track`")
    })
}

fn feel_json(f: &Feel) -> Value {
    json!({
        "swing": f.swing,
        "sub": f.sub.id(),
        "offsets": f.offsets.iter().map(|o| super::params::round6(f64::from(*o))).collect::<Vec<_>>(),
        "offset": pct(f.offset_amount),
    })
}

/// What `rack.get` (and every change here) answers.
pub(super) fn rack_json(cx: &mut Cx, ti: usize) -> Value {
    let p = &cx.app.project;
    let st = channel_rack::rack_state(p, ti).unwrap_or_default();
    let ri = channel_rack::find_step_region(p, ti, cx.app.ui.tracks.selected);
    let region = ri.map(|ri| &p.tracks[ti].regions[ri]);
    let steps = region.map_or(16, |r| {
        ((p.region_len_beats(r) / 0.25).round() as usize).clamp(16, 64)
    });
    let no_notes = Vec::new();
    let notes = region.map_or(&no_notes, |r| &r.notes);
    let channels: Vec<Value> = st
        .channels
        .iter()
        .enumerate()
        .map(|(c, ch)| {
            let on = channel_rack::steps_on(notes, &st, c, steps);
            let feel = st.feel(c);
            let hits: Vec<Value> = channel_rack::hits_of(notes, &st, c, steps)
                .iter()
                .map(|(cell, vel, _)| {
                    json!({
                        "step": cell.step + 1, "sub": cell.sub, "div": cell.div, "vel": vel,
                        "start": super::params::round6(f64::from(feel.place(*cell))),
                    })
                })
                .collect();
            json!({
                "channel": c + 1,
                "note": RACK_BASE_NOTE + c as u8,
                "name": ch.name,
                "path": ch.path,
                "class": ch.class.id(),
                "gain_db": super::params::round6(f64::from(ch.gain_db)),
                "pan": super::params::round6(f64::from(ch.pan)),
                "mute": ch.mute,
                "solo": ch.solo,
                "swing": st.groove.feel(ch.class).swing,
                "offset": pct(st.groove.feel(ch.class).offset_amount),
                "steps": on.iter().map(|k| k + 1).collect::<Vec<_>>(),
                "starts": on.iter().map(|k| super::params::round6(f64::from(feel.at(*k)))).collect::<Vec<_>>(),
                "hits": hits,
            })
        })
        .collect();
    let classes: serde_json::Map<String, Value> = st
        .groove
        .classes
        .iter()
        .map(|(c, f)| (c.id().to_string(), feel_json(f)))
        .collect();
    let region_id = ri.map(|ri| cx.core.ids.region_id(ti, ri));
    json!({
        "track": state::strip_id(&cx.core.ids, ti),
        "region": region_id,
        "kit": st.kit,
        "kit_dir": st.kit_dir,
        "steps": steps,
        "groove": {
            "name": st.groove.name,
            "source": st.groove.source,
            "straight": st.groove.is_straight(),
            "default": feel_json(&st.groove.default),
            "classes": classes,
        },
        "amounts": {
            "groove": pct(st.groove.swing_amount),
            "offset": pct(st.groove.offset_amount),
            "velocity": pct(st.velocity_amount),
        },
        "velocity": st.velocity,
        "pattern": {
            "name": st.pattern.name,
            "source": st.pattern.source,
            "steps": st.pattern.steps,
            "hits": st.pattern.hits.len(),
        },
        "copying": crate::kit_browser::copying(&cx.app.ui.kits) || channel_rack::copying_default(),
        "channels": channels,
    })
}

/// An amount (0 to 1) in percent, to a tenth.
fn pct(a: f32) -> f64 {
    (f64::from(a) * 1000.0).round() / 10.0
}

fn get(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    ok(rack_json(cx, ti))
}

fn class_param(params: &P, key: &str) -> Result<Option<DrumClass>, RpcError> {
    let id = params.req_str(key)?;
    if id == "all" {
        return Ok(None);
    }
    DrumClass::from_id(id)
        .filter(|c| channel_rack::CHANNEL_CLASSES.contains(c))
        .map(Some)
        .ok_or_else(|| RpcError::invalid(key, format!("no class \"{id}\"")))
}

fn req_channel(cx: &Cx, params: &P, ti: usize) -> Result<usize, RpcError> {
    let n = channel_rack::rack_state(&cx.app.project, ti).map_or(0, |s| s.channels.len());
    let c = params.i64("channel").unwrap_or(0);
    if c < 1 || c as usize > n {
        return Err(RpcError::invalid(
            "channel",
            format!("the rack has channels 1 to {n}"),
        ));
    }
    Ok(c as usize - 1)
}

fn set_class(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let c = req_channel(cx, params, ti)?;
    let Some(class) = class_param(params, "class")? else {
        return Err(RpcError::invalid("class", "one class, not all"));
    };
    channel_rack::set_class(&mut cx.app.project, ti, c, class);
    ok(rack_json(cx, ti))
}

fn swing(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let class = class_param(params, "class")?;
    let pct = params.req_f64("pct")? as f32;
    let sub = match params.str("sub") {
        Some(s) => Some(Sub::parse(s).ok_or_else(|| RpcError::invalid("sub", "16th or 8th"))?),
        None => None,
    };
    let mut g = channel_rack::rack_state(&cx.app.project, ti)
        .unwrap_or_default()
        .groove;
    g.set_swing(class, pct, sub);
    let moved = channel_rack::set_groove(&mut cx.app.project, ti, g).unwrap_or(0);
    let mut v = rack_json(cx, ti);
    v["moved"] = json!(moved);
    ok(v)
}

fn offset(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let class = class_param(params, "class")?;
    let pct = params.req_f64("pct")?;
    let mut g = channel_rack::rack_state(&cx.app.project, ti)
        .unwrap_or_default()
        .groove;
    g.set_offset(class, (pct / 100.0) as f32);
    let moved = channel_rack::set_groove(&mut cx.app.project, ti, g).unwrap_or(0);
    let mut v = rack_json(cx, ti);
    v["moved"] = json!(moved);
    ok(v)
}

fn steps(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let c = req_channel(cx, params, ti)?;
    let Some(list) = params.arr("steps") else {
        return Err(RpcError::invalid("steps", "a list of step numbers, from 1"));
    };
    let mut want = Vec::new();
    for s in list {
        match s.as_i64() {
            Some(k) if (1..=64).contains(&k) => want.push(k as usize - 1),
            _ => return Err(RpcError::invalid("steps", "step numbers 1 to 64")),
        }
    }
    let selected = cx.app.ui.tracks.selected;
    let p = &mut cx.app.project;
    let st: RackState = channel_rack::rack_state(p, ti).unwrap_or_default();
    let ri = channel_rack::step_region(p, ti, selected);
    let region = &mut p.tracks[ti].regions[ri];
    for k in 0..64 {
        channel_rack::set_step(region, &st, c, k, want.contains(&k));
    }
    p.song_rev += 1;
    cx.core.ids.reconcile(&cx.app.project);
    ok(rack_json(cx, ti))
}

fn feel_from(v: &Value) -> Result<Feel, RpcError> {
    let mut f = Feel::default();
    if let Some(s) = v["swing"].as_f64() {
        f.swing = (s as f32).clamp(crate::rack_groove::SWING_MIN, crate::rack_groove::SWING_MAX);
    }
    if let Some(s) = v["sub"].as_str() {
        f.sub = Sub::parse(s).ok_or_else(|| RpcError::invalid("groove", "sub: 16th or 8th"))?;
    }
    if let Some(a) = v["offsets"].as_array() {
        if a.len() != SLOTS {
            return Err(RpcError::invalid("groove", "offsets: 16 numbers"));
        }
        for (k, o) in a.iter().enumerate() {
            let x = o
                .as_f64()
                .ok_or_else(|| RpcError::invalid("groove", "offsets: 16 numbers"))?;
            f.offsets[k] = (x as f32).clamp(
                -crate::rack_groove::OFFSET_MAX,
                crate::rack_groove::OFFSET_MAX,
            );
        }
    }
    if let Some(a) = v["offset"].as_f64() {
        f.offset_amount = (a / 100.0).clamp(0.0, 1.0) as f32;
    }
    Ok(f)
}

fn groove(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let g = match params.get("groove") {
        None | Some(Value::Null) => Groove::default(),
        Some(v) if v.is_object() => {
            let mut g = Groove {
                name: v["name"].as_str().unwrap_or_default().to_string(),
                source: v["source"].as_str().unwrap_or_default().to_string(),
                default: feel_from(&v["default"])?,
                ..Groove::default()
            };
            for (k, f) in v["classes"].as_object().into_iter().flatten() {
                let c = DrumClass::from_id(k)
                    .ok_or_else(|| RpcError::invalid("groove", format!("no class \"{k}\"")))?;
                g.classes.insert(c, feel_from(f)?);
            }
            g
        }
        Some(_) => {
            return Err(RpcError::invalid(
                "groove",
                "an object, or null for straight",
            ))
        }
    };
    let moved = channel_rack::set_groove(&mut cx.app.project, ti, g).unwrap_or(0);
    let mut v = rack_json(cx, ti);
    v["moved"] = json!(moved);
    ok(v)
}

fn mix(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let c = req_channel(cx, params, ti)?;
    let old = channel_rack::rack_state(&cx.app.project, ti).unwrap_or_default();
    let mut new = old.clone();
    let ch = &mut new.channels[c];
    if let Some(db) = params.f64("gain_db") {
        ch.gain_db = (db as f32).clamp(-48.0, 12.0);
    }
    if let Some(pan) = params.f64("pan") {
        ch.pan = (pan as f32).clamp(-1.0, 1.0);
    }
    if let Some(m) = params.bool("mute") {
        ch.mute = m;
    }
    if let Some(m) = params.bool("solo") {
        ch.solo = m;
    }
    channel_rack::commit(&mut cx.app.project, ti, &old, new);
    ok(rack_json(cx, ti))
}

fn amounts(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let old = channel_rack::rack_state(&cx.app.project, ti).unwrap_or_default();
    let mut new = old.clone();
    let amount = |k: &str| params.f64(k).map(|x| (x / 100.0).clamp(0.0, 1.0) as f32);
    if let Some(a) = amount("groove") {
        new.groove.swing_amount = a;
    }
    if let Some(a) = amount("offset") {
        new.groove.offset_amount = a;
    }
    if let Some(a) = amount("velocity") {
        new.velocity_amount = a;
    }
    if let Some(v) = params.i64("new_velocity") {
        new.velocity = v.clamp(1, 127) as u8;
    }
    let moved = channel_rack::commit(&mut cx.app.project, ti, &old, new);
    let mut v = rack_json(cx, ti);
    v["moved"] = json!(moved);
    ok(v)
}

/// The cell a request names: `step` (from 1), `sub` (from 0) of `div` parts.
fn req_cell(params: &P) -> Result<Cell, RpcError> {
    let k = params.i64("step").unwrap_or(0);
    if !(1..=64).contains(&k) {
        return Err(RpcError::invalid("step", "a step, 1 to 64"));
    }
    let div = params.i64("div").unwrap_or(1);
    if !(1..=i64::from(MAX_DIV)).contains(&div) {
        return Err(RpcError::invalid("div", "1 to 4"));
    }
    let sub = params.i64("sub").unwrap_or(0);
    if sub < 0 || sub >= div {
        return Err(RpcError::invalid("sub", "0 to div - 1"));
    }
    Ok(Cell::new(k as usize - 1, sub as u8, div as u8))
}

fn hit(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let c = req_channel(cx, params, ti)?;
    let cell = req_cell(params)?;
    let on = params.bool("on").unwrap_or(true);
    let vel = match params.i64("vel") {
        Some(v) if (1..=127).contains(&v) => Some(v as u8),
        Some(_) => return Err(RpcError::invalid("vel", "1 to 127")),
        None => None,
    };
    let selected = cx.app.ui.tracks.selected;
    let p = &mut cx.app.project;
    let st: RackState = channel_rack::rack_state(p, ti).unwrap_or_default();
    let ri = channel_rack::step_region(p, ti, selected);
    let region = &mut p.tracks[ti].regions[ri];
    channel_rack::set_hit(region, &st, c, cell, on, None);
    // A velocity for a hit there already goes through the pattern it may be part of.
    let changed = match (on, vel) {
        (true, Some(v)) => channel_rack::set_hit_velocity(region, &st, c, cell, v),
        _ => None,
    };
    if let Some(new) = changed {
        channel_rack::commit(p, ti, &st, new);
    }
    p.song_rev += 1;
    cx.core.ids.reconcile(&cx.app.project);
    ok(rack_json(cx, ti))
}

fn pattern(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let steps = params.i64("steps").unwrap_or(16);
    if !(1..=64).contains(&steps) {
        return Err(RpcError::invalid("steps", "1 to 64"));
    }
    let Some(list) = params.arr("hits") else {
        return Err(RpcError::invalid(
            "hits",
            "a list of {class, step, sub, div, vel}",
        ));
    };
    let mut hits = Vec::new();
    for h in list {
        let class = h["class"]
            .as_str()
            .and_then(DrumClass::from_id)
            .filter(|c| channel_rack::CHANNEL_CLASSES.contains(c))
            .ok_or_else(|| RpcError::invalid("hits", "each hit needs a class"))?;
        let vel = h["vel"]
            .as_i64()
            .unwrap_or(i64::from(channel_rack::VELOCITY))
            .clamp(1, 127) as u8;
        // A place in quarter notes instead: the nearest step or part of one.
        if let Some(beat) = h["beat"].as_f64().filter(|b| b.is_finite() && *b >= 0.0) {
            let cell = Cell::of_straight(beat);
            if cell.step >= steps as usize {
                return Err(RpcError::invalid("hits", "a beat past the pattern's end"));
            }
            hits.push((class, cell, vel, None));
            continue;
        }
        let k = h["step"].as_i64().unwrap_or(0);
        let div = h["div"].as_i64().unwrap_or(1);
        let sub = h["sub"].as_i64().unwrap_or(0);
        if !(1..=steps).contains(&k)
            || !(1..=i64::from(MAX_DIV)).contains(&div)
            || sub < 0
            || sub >= div
        {
            return Err(RpcError::invalid(
                "hits",
                "step 1 to steps, div 1 to 4, sub 0 to div - 1",
            ));
        }
        hits.push((
            class,
            Cell::new(k as usize - 1, sub as u8, div as u8),
            vel,
            None,
        ));
    }
    let pat = channel_rack::RackPattern {
        name: params.str("name").unwrap_or_default().to_string(),
        source: String::new(),
        steps: steps as usize,
        hits,
        ..channel_rack::RackPattern::default()
    };
    let groove = channel_rack::rack_state(&cx.app.project, ti)
        .unwrap_or_default()
        .groove;
    let selected = cx.app.ui.tracks.selected;
    let done = channel_rack::load_pattern(
        &mut cx.app.project,
        Some(ti),
        selected,
        &pat,
        groove,
        None,
        channel_rack::PatternTempo::Session,
    );
    cx.core.ids.reconcile(&cx.app.project);
    let mut v = rack_json(cx, ti);
    v["loaded"] = json!({"hits": done.hits, "dropped": done.dropped, "channels_added": done.added});
    ok(v)
}

fn divide(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let c = req_channel(cx, params, ti)?;
    let k = params.i64("step").unwrap_or(0);
    if !(1..=64).contains(&k) {
        return Err(RpcError::invalid("step", "a step, 1 to 64"));
    }
    let div = params.i64("div").unwrap_or(0);
    if !(1..=i64::from(MAX_DIV)).contains(&div) {
        return Err(RpcError::invalid("div", "1 to 4"));
    }
    let selected = cx.app.ui.tracks.selected;
    let p = &mut cx.app.project;
    let st: RackState = channel_rack::rack_state(p, ti).unwrap_or_default();
    let ri = channel_rack::step_region(p, ti, selected);
    channel_rack::divide_step(
        &mut p.tracks[ti].regions[ri],
        &st,
        c,
        k as usize - 1,
        div as u8,
    );
    p.song_rev += 1;
    cx.core.ids.reconcile(&cx.app.project);
    ok(rack_json(cx, ti))
}

fn split_tracks(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_rack(cx, params)?;
    let made = crate::rack_split::split(&mut cx.app.project, ti);
    if made.is_empty() {
        return Err(RpcError::failed("no channel of the rack has a sample"));
    }
    crate::tracks_area::forget_track_indexes(&mut cx.app.ui);
    cx.core.ids.reconcile(&cx.app.project);
    let tracks: Vec<Value> = made
        .iter()
        .map(|m| {
            json!({
                "track": state::strip_id(&cx.core.ids, m.track),
                "name": m.name,
                "channel": m.channel + 1,
                "notes": m.notes,
            })
        })
        .collect();
    let rack = made[0].track - 1;
    ok(json!({
        "track": state::strip_id(&cx.core.ids, rack),
        "name": cx.app.project.tracks[rack].name,
        "tracks": tracks,
    }))
}

pub fn methods() -> Vec<Method> {
    let track = || {
        p(
            "track",
            Ty::Any,
            "the rack's track: its id (\"t3\"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack",
        )
    };
    let channel = || {
        p("channel", Ty::Int, "the channel, from 1")
            .req()
            .range(1.0, 64.0)
    };
    let result = "{track, region (null: none yet), kit, steps, groove: {name, source, straight, default: feel, classes: {class: feel}}, amounts: {groove, offset, velocity (percent)}, velocity (of a hit added by hand), pattern: {name, source, steps, hits}, copying (a kit being copied here), channels: [{channel, note, name, path, class, gain_db, pan, mute, swing, offset (percent of its class's offsets played), steps: [from 1], starts: [quarter notes from the region's start], hits: [{step, sub, div, vel, start}]}]}";
    vec![
        m("rack.get", "A Channel Rack: each channel's sample, drum class, swing and steps (in the region the rack's window shows, with where each step's note starts), and the feel: the swing and sixteen offsets of each class that has its own, and of every other class. A feel is {swing, sub, offsets, offset (percent of the offsets the class plays, under the rack's offset amount)}.",
            vec![track()], result, get),
        m("rack.class", "Gives a channel another drum class; its steps move to that class's feel.",
            vec![track(), channel(), p("class", Ty::Str, "the class").req().vals(&CLASS_IDS)],
            result, set_class).changes(),
        m("rack.swing", "Sets the swing of one drum class (or of all) and moves the steps of its channels; the offsets stay.",
            vec![
                track(),
                p("class", Ty::Str, "the class, or all").req(),
                p("pct", Ty::Num, "the off-beat's place in its pair: 50 straight, 66.7 triplet").req().range(35.0, 85.0).unit("%"),
                p("sub", Ty::Str, "which off-beats swing: 16th (the e and a) or 8th (the and); left out: as it is").vals(&["16th", "8th"]),
            ],
            "the rack.get answer, with moved (notes moved)", swing).changes(),
        m("rack.offset", "Sets how much of its offsets one drum class (or every class) plays: the drum's own Offset, under the rack's Offset amount (rack.amounts offset), which scales every class. The steps of its channels move.",
            vec![
                track(),
                p("class", Ty::Str, "the class, or all").req(),
                p("pct", Ty::Num, "0: on the swung step, 100: the class's offsets as loaded").req().range(0.0, 100.0).unit("%"),
            ],
            "the rack.get answer, with moved (notes moved)", offset).changes(),
        m("rack.steps", "Sets which steps of a channel are on, in the region the rack's window shows; each note goes where the channel's feel plays its step.",
            vec![track(), channel(), p("steps", Ty::Arr, "the step numbers on, from 1 (the others go off)").req()],
            result, steps).changes(),
        m("rack.groove", "Sets the rack's whole feel (as rack.get gives it), or makes it straight; the steps move.",
            vec![track(), p("groove", Ty::Any, "{name, source, default: feel, classes: {class: feel}}, a feel {swing, sub (16th, 8th), offsets: [16 quarter notes, one a sixteenth of the bar], offset (percent of them played, default 100)}; null or left out: straight")],
            "the rack.get answer, with moved (notes moved)", groove).changes(),
        m("rack.mix", "Sets a channel's volume, pan, mute or solo (what its knobs and its Mute and Solo keys set).",
            vec![
                track(), channel(),
                p("gain_db", Ty::Num, "the volume").range(-48.0, 12.0).unit("dB"),
                p("pan", Ty::Num, "the pan: -1 left, 0 centre, 1 right").range(-1.0, 1.0),
                p("mute", Ty::Bool, "muted"),
                p("solo", Ty::Bool, "soloed: while any channel is, only the soloed ones play"),
            ],
            result, mix).changes(),
        m("rack.amounts", "Sets how much of the rack's groove plays: the swing (0: straight, 100: as loaded or set), the offsets, and the source's velocities (a loaded pattern's or groove's; 0: a pattern's hits at the rack's velocity when it was loaded, a groove's steps at their velocity before); and the rack's velocity (of a hit added by hand). The steps move and take their velocities.",
            vec![
                track(),
                p("groove", Ty::Num, "how much of each class's swing plays").range(0.0, 100.0).unit("%"),
                p("offset", Ty::Num, "how much of the offsets plays, over every class's own offset amount (rack.offset)").range(0.0, 100.0).unit("%"),
                p("velocity", Ty::Num, "how much of the loaded pattern's or groove's velocities plays").range(0.0, 100.0).unit("%"),
                p("new_velocity", Ty::Int, "the rack's velocity").range(1.0, 127.0),
            ],
            "the rack.get answer, with moved (notes moved)", amounts).changes(),
        m("rack.hit", "Turns one hit of a channel on or off, in the region the rack's window shows: a whole step (div 1) or part sub of a step split in div (2: 32nds, 3: triplets, 4: 64ths), with its velocity. A hit of the loaded pattern keeps the velocity it now stands for.",
            vec![
                track(), channel(),
                p("step", Ty::Int, "the step, from 1").req().range(1.0, 64.0),
                p("div", Ty::Int, "parts the step is split into (default 1)").range(1.0, 4.0),
                p("sub", Ty::Int, "the part, from 0 (default 0)").range(0.0, 3.0),
                p("on", Ty::Bool, "on (default) or off"),
                p("vel", Ty::Int, "its velocity (left out: the pattern's there, else the rack's)").range(1.0, 127.0),
            ],
            result, hit).changes(),
        m("rack.divide", "Splits a step of a channel into hits (a ratchet: 1 whole, 2 32nds, 3 triplets, 4 64ths), each at the loudest velocity the step had.",
            vec![
                track(), channel(),
                p("step", Ty::Int, "the step, from 1").req().range(1.0, 64.0),
                p("div", Ty::Int, "how many hits").req().range(1.0, 4.0),
            ],
            result, divide).changes(),
        m("rack.pattern", "Loads a pattern given as hits by drum class into a Channel Rack, onto the kit there: each hit on the first channel of its class (else of a class that stands in, else a new channel of the class), in the rack's region made as long as the pattern; its velocities are kept for the velocity amount. The groove stays, its amounts at 100 %.",
            vec![
                track(),
                p("hits", Ty::Arr, "[{class, step (from 1), div (1-4, default 1), sub (from 0), vel (default 100)}]; instead of step/div/sub a beat (quarter notes from the start) goes to the nearest step or part of one").req(),
                p("steps", Ty::Int, "how many steps long (default 16)").range(1.0, 64.0),
                p("name", Ty::Str, "its name"),
            ],
            "the rack.get answer, with loaded: {hits, dropped, channels_added}", pattern).changes(),
        m("rack.split_tracks", "Split to Tracks: puts each channel of the rack that has a sample on an instrument track of its own named for its drum class (Kick, Snare, a second kick Kick 2; a class without a name: the sample's), right under the rack's track, playing a Channel Rack with that channel alone (its sample, volume, pan, mute and class, the rack's groove and amounts). The channel's notes move there unchanged, on the new rack's channel 1, in regions where the rack's are. The rack's track stays, muted and named \"<name> (split)\". One Undo step.",
            vec![track()],
            "{track (the rack's), name, tracks: [{track, name, channel (the rack's, from 1), notes (moved)}]}", split_tracks).changes(),
    ]
}
