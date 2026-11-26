//! Methods for Follow Rack Groove (Track Inspector: Follow Rack): a MIDI track playing its
//! notes on the hits of one drum class of a Channel Rack, Groove Q, the quantize toward those
//! hits (see `rack_follow.rs`), and Groove Flex, its Flex Time version for audio tracks (see
//! `flex_edit.rs`).
use super::params::P;
use super::protocol::RpcError;
use super::registry::{m, p, Method, Reply, Ty};
use super::state;
use super::{ok, Cx};
use crate::model::TrackKind;
use crate::rack_follow::{self as rf, FollowRack};
use serde_json::{json, Value};

/// The class choices: auto, hat (the closed hat), and every rack class id.
const CLASSES: [&str; 18] = [
    "auto",
    "hat",
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

fn flex_result() -> &'static str {
    "{track, strength (0-100 %), rack (from 1), rack_track (null: no rack), class (the choice), pulls_to (the class it pulls to now; null: no rack), regions: [{region, own (null: the track's), strength, hits (null: not found yet), markers, mode, finding_hits}], busy (the hits being found or the sound being made)}"
}

fn groove_result() -> &'static str {
    "{track, strength (0-100 %), live, rack (from 1), rack_track (null: no rack), class (the choice), pulls_to (the class it pulls to now; null: no rack), follow_rack (Follow Rack on), regions: [{region, own (null: the track's), strength}]}"
}

/// What `rack.follow_get` (and `rack.follow`) answers for track `ti`.
fn follow_json(cx: &Cx, ti: usize) -> Value {
    let p = &cx.app.project;
    let f = p.tracks[ti].params.follow_rack;
    let racks = rf::racks(p);
    let rack = rf::rack_of(p, &f);
    let target = rf::target(p, ti);
    json!({
        "track": cx.core.ids.track_id(ti),
        "on": f.on,
        "rack": u32::from(f.rack) + 1,
        "rack_track": rack.map(|r| cx.core.ids.track_id(r)),
        "class": rf::class_id(f.class),
        "auto": rf::auto_of(p, ti).id(),
        "follows": target.map(|(_, c)| c.id()),
        "velocity": f.velocity,
        "groove_q": f.groove_q,
        "live": f.live,
        "racks": racks
            .iter()
            .enumerate()
            .map(|(n, i)| json!({"rack": n + 1, "track": cx.core.ids.track_id(*i), "name": p.tracks[*i].name}))
            .collect::<Vec<_>>(),
    })
}

fn req_midi_track(cx: &Cx, params: &P) -> Result<usize, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let t = &cx.app.project.tracks[ti];
    if !t.kind.has_regions() || t.kind == TrackKind::Audio {
        return Err(RpcError::failed(format!(
            "track \"{}\" holds no MIDI regions",
            t.name
        )));
    }
    Ok(ti)
}

fn get(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_midi_track(cx, params)?;
    ok(follow_json(cx, ti))
}

fn set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_midi_track(cx, params)?;
    let mut f: FollowRack = cx.app.project.tracks[ti].params.follow_rack;
    if let Some(on) = params.bool("on") {
        f.on = on;
    }
    if let Some(n) = params.i64("rack") {
        let count = rf::racks(&cx.app.project).len() as i64;
        if n < 1 || n > count.max(1) {
            return Err(RpcError::invalid(
                "rack",
                format!(
                    "the project has {count} Channel Rack(s): 1 to {}",
                    count.max(1)
                ),
            ));
        }
        f.rack = (n - 1).min(255) as u8;
    }
    if let Some(id) = params.str("class") {
        f.class = rf::parse_class(id)
            .ok_or_else(|| RpcError::invalid("class", format!("no class \"{id}\"")))?;
    }
    if let Some(v) = params.bool("velocity") {
        f.velocity = v;
    }
    let p = &mut cx.app.project;
    if p.tracks[ti].params.follow_rack != f {
        p.tracks[ti].params.follow_rack = f;
        p.song_rev += 1;
    }
    ok(follow_json(cx, ti))
}

/// What `rack.groove_q` answers for track `ti`.
fn groove_json(cx: &Cx, ti: usize) -> Value {
    let p = &cx.app.project;
    let t = &p.tracks[ti];
    let f = t.params.follow_rack;
    let target = rf::groove_target(p, ti);
    json!({
        "track": cx.core.ids.track_id(ti),
        "strength": f.groove_q,
        "live": f.live,
        "rack": u32::from(f.rack) + 1,
        "rack_track": target.map(|(r, _)| cx.core.ids.track_id(r)),
        "class": rf::class_id(f.class),
        "pulls_to": target.map(|(_, c)| c.id()),
        "follow_rack": f.on,
        "regions": t
            .regions
            .iter()
            .enumerate()
            .map(|(ri, r)| json!({
                "region": cx.core.ids.region_id(ti, ri),
                "own": r.params.groove_q,
                "strength": rf::groove_q_of(t, r),
            }))
            .collect::<Vec<_>>(),
    })
}

/// The 0-100 % strength in `key`.
fn strength(params: &P, key: &str) -> Result<Option<u8>, RpcError> {
    match params.i64(key) {
        None => Ok(None),
        Some(n) if (0..=i64::from(rf::MAX_GROOVE_Q)).contains(&n) => Ok(Some(n as u8)),
        Some(_) => Err(RpcError::invalid(key, "a strength from 0 to 100 (%)")),
    }
}

/// Sets Groove Q of the track `ti` (or of one of its regions) from `params`.
fn set_groove(cx: &mut Cx, params: &P, ti: usize) -> Result<(), RpcError> {
    let mut f: FollowRack = cx.app.project.tracks[ti].params.follow_rack;
    let region = match params.get("region") {
        Some(_) => {
            let (t, r) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
            if t != ti {
                return Err(RpcError::invalid("region", "a region of the track"));
            }
            Some(r)
        }
        None => None,
    };
    let s = strength(params, "strength")?;
    if let (None, Some(s)) = (region, s) {
        f.groove_q = s;
    }
    if let Some(v) = params.bool("live") {
        f.live = v;
    }
    if let Some(n) = params.i64("rack") {
        let count = rf::racks(&cx.app.project).len() as i64;
        if n < 1 || n > count.max(1) {
            return Err(RpcError::invalid(
                "rack",
                format!(
                    "the project has {count} Channel Rack(s): 1 to {}",
                    count.max(1)
                ),
            ));
        }
        f.rack = (n - 1).min(255) as u8;
    }
    if let Some(id) = params.str("class") {
        f.class = rf::parse_class(id)
            .ok_or_else(|| RpcError::invalid("class", format!("no class \"{id}\"")))?;
    }
    let p = &mut cx.app.project;
    let mut changed = p.tracks[ti].params.follow_rack != f;
    p.tracks[ti].params.follow_rack = f;
    if let Some(r) = region {
        let own = if params.bool("track_strength") == Some(true) {
            None
        } else {
            s.or(p.tracks[ti].regions[r].params.groove_q)
        };
        let q = &mut p.tracks[ti].regions[r].params.groove_q;
        changed |= *q != own;
        *q = own;
    }
    if changed {
        p.song_rev += 1;
    }
    Ok(())
}

/// What `rack.groove_flex` answers for audio track `ti`: Groove Q's answer, with each region's
/// hits and markers.
fn flex_json(cx: &Cx, ti: usize) -> Value {
    let mut v = groove_json(cx, ti);
    let t = &cx.app.project.tracks[ti];
    if let Some(regions) = v.get_mut("regions").and_then(Value::as_array_mut) {
        for (ri, (r, j)) in t.regions.iter().zip(regions.iter_mut()).enumerate() {
            let f = r.audio.as_ref().and_then(|a| a.flex.as_ref());
            j["hits"] = json!(f.and_then(|f| f.groove_onsets.as_ref()).map(Vec::len));
            j["markers"] = json!(f.map_or(0, |f| f.markers.len()));
            j["mode"] = json!(f.map(|f| f.mode.name()));
            j["finding_hits"] = json!(cx.app.ui.flex.finding_hits(ti, ri));
        }
    }
    v["busy"] = json!(cx.app.ui.flex.busy());
    v
}

fn req_audio_track(cx: &Cx, params: &P) -> Result<usize, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let t = &cx.app.project.tracks[ti];
    if t.kind != TrackKind::Audio {
        return Err(RpcError::failed(format!(
            "track \"{}\" is not an audio track",
            t.name
        )));
    }
    Ok(ti)
}

fn flex_get(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_audio_track(cx, params)?;
    ok(flex_json(cx, ti))
}

fn flex_set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_audio_track(cx, params)?;
    if params.get("live").is_some() {
        return Err(RpcError::invalid("live", "Groove Live is for MIDI tracks"));
    }
    set_groove(cx, params, ti)?;
    ok(flex_json(cx, ti))
}

fn groove_get(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_midi_track(cx, params)?;
    ok(groove_json(cx, ti))
}

fn groove_set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = req_midi_track(cx, params)?;
    set_groove(cx, params, ti)?;
    ok(groove_json(cx, ti))
}

pub fn methods() -> Vec<Method> {
    let track = || {
        p(
            "track",
            Ty::Any,
            "the MIDI track: its id (\"t3\"), its name, or its number in the list (from 1)",
        )
        .req()
    };
    let result = "{track, on, rack (from 1), rack_track (null: no rack), class (the choice: auto or a class), auto (what Auto picks for the track), follows (the class it plays with now; null: off or no rack), velocity, groove_q (Groove Q, 0-100 %), live (Groove Live), racks: [{rack, track, name}]}";
    vec![
        m("rack.follow_get", "Follow Rack Groove of a MIDI track: whether its notes play on the hits of one drum class of a Channel Rack where the rack plays them (a loaded pattern's at their played times; a note where the class plays no hit stays), which rack, which class (Auto picks kick for 808s and bass, the closed hat for dense sixteenths, else the snare) and whether the hits' velocities accent the notes. The notes stay where they are stored; playback, the bounce and the MIDI export (unless switched off) take the rack's timing.",
            vec![track()], result, get),
        m("rack.follow", "Sets Follow Rack Groove of a MIDI track (the parameters left out stay as they are).",
            vec![
                track(),
                p("on", Ty::Bool, "follow a rack"),
                p("rack", Ty::Int, "which rack: 1 the first track playing a Channel Rack, 2 the second…").range(1.0, 256.0),
                p("class", Ty::Str, "the class whose hits the notes play on: auto, hat (the closed hat), snare, kick, or another rack class").vals(&CLASSES),
                p("velocity", Ty::Bool, "the velocity of the rack's hit carrying each note, over the mean of the class's hits, scales the note's"),
            ],
            result, set).changes(),
        m("rack.groove_q_get", "Groove Q of a MIDI track: a second quantize, toward a Channel Rack's hits instead of the straight grid. Each note is pulled from where it is toward the target Follow Rack gives the nearest step (the region's Quantize grid, sixteenths when off) for the class Follow picks: on a step the class has a hit on, where that one hit plays (a loaded pattern's at its played time, never a mean); between two hits at most a sixteenth apart, its share of the way between them; where the class plays no hit the note stays. By the strength. A region takes its track's strength unless it has its own. The notes stay where they are stored; playback, the bounce and the MIDI export take it.",
            vec![track()], groove_result(), groove_get),
        m("rack.groove_q", "Sets Groove Q of a MIDI track, or with `region` of one of its regions (the parameters left out stay as they are).",
            vec![
                track(),
                p("strength", Ty::Int, "how far the notes are pulled to the groove: 0 (off) to 100 (on it); the track's, or the region's with `region`").unit("%").range(0.0, 100.0),
                p("region", Ty::Any, "a region of the track (id \"r12\" or name): `strength` and `track_strength` are its own"),
                p("track_strength", Ty::Bool, "with `region`: true makes the region take its track's strength again"),
                p("live", Ty::Bool, "Groove Live: keys played live while the song plays wait for a step the groove plays late, by the strength (never earlier); the take is recorded where the keys were pressed"),
                p("rack", Ty::Int, "which rack: 1 the first track playing a Channel Rack, 2 the second…").range(1.0, 256.0),
                p("class", Ty::Str, "the class whose hits the notes are pulled to (shared with Follow Rack)").vals(&CLASSES),
            ],
            groove_result(), groove_set).changes(),
        m("rack.groove_flex_get", "Groove Flex of an audio track: Flex Time toward a Channel Rack's groove. The hits of each audio region are found once (in the background) and each moves toward the target Follow Rack gives the nearest sixteenth (the region's Quantize grid when set) for the class Follow picks (where that step's own hit plays; a hit with no rack hit there stays) (Auto: Kick for 808 and bass by name, the closed hat for dense hits, else the snare), by the strength; the sound between hits is stretched without changing its pitch (WSOLA, the region's Flex mode, Rhythmic when it had none), the region keeps its length and the recording is never changed (0 %: it plays as recorded). Playback and the bounce take it once the sound is made (`busy` false).",
            vec![p("track", Ty::Any, "the audio track: its id (\"t3\"), its name, or its number in the list (from 1)").req()],
            flex_result(), flex_get),
        m("rack.groove_flex", "Sets Groove Flex of an audio track, or with `region` of one of its regions (the parameters left out stay as they are). Setting it to 0 removes the Groove Flex markers.",
            vec![
                p("track", Ty::Any, "the audio track: its id (\"t3\"), its name, or its number in the list (from 1)").req(),
                p("strength", Ty::Int, "how far the hits are pulled to the groove: 0 (off) to 100 (on it); the track's, or the region's with `region`").unit("%").range(0.0, 100.0),
                p("region", Ty::Any, "an audio region of the track (id \"r12\" or name): `strength` and `track_strength` are its own"),
                p("track_strength", Ty::Bool, "with `region`: true makes the region take its track's strength again"),
                p("rack", Ty::Int, "which rack: 1 the first track playing a Channel Rack, 2 the second…").range(1.0, 256.0),
                p("class", Ty::Str, "the class whose hits the audio hits are pulled to").vals(&CLASSES),
            ],
            flex_result(), flex_set).changes(),
    ]
}
