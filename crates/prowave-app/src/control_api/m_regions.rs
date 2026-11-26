//! Methods for regions and their notes, importing files, markers and the Tempo and Signature
//! tracks.
use super::params::{place, place_json, place_params, req_place, round6, P};
use super::protocol::{Code, RpcError};
use super::registry::{m, p, Method, Reply, Ty};
use super::{ok, state, Cx};
use crate::lyrics::Lyric;
use crate::model::{Note, Project, Region, RegionParams, TrackKind, MAX_BPM, MIN_BPM};
use serde_json::{json, Value};

/// Most notes one call may write.
const MAX_NOTES: usize = 200_000;
/// The latest start and the longest length of a note, in quarter notes.
const MAX_NOTE_BEATS: f64 = 1.0e6;

fn region_out(cx: &mut Cx, ti: usize, ri: usize, notes: bool) -> Value {
    cx.core.ids.reconcile(&cx.app.project);
    state::region_json(&cx.app.project, &cx.core.ids, ti, ri, notes)
}

fn region_get(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let (ti, ri) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
    let notes = params.bool("notes").unwrap_or(true);
    ok(state::region_json(
        &cx.app.project,
        &cx.core.ids,
        ti,
        ri,
        notes,
    ))
}

/// A length given as `length_bars` or `length_beats`, in quarter notes from `start`.
fn length_beats(pr: &Project, params: &P, start: f64) -> Result<Option<f64>, RpcError> {
    if let Some(bars) = params.f64("length_bars") {
        let bar0 = pr.beats_to_bar(start);
        return Ok(Some(pr.bar_to_beats(bar0 + bars) - start));
    }
    Ok(params.f64("length_beats"))
}

/// Sets the region's length to `beats` quarter notes, as dragging its end does.
fn set_length(pr: &mut Project, ti: usize, ri: usize, beats: f64) {
    let start = pr.region_start_beats(&pr.tracks[ti].regions[ri]);
    let start_bar = pr.tracks[ti].regions[ri].start_bar;
    let end_bar = pr.beats_to_bar(start + beats) as f32 + 1.0;
    pr.tracks[ti].regions[ri].len_bars = (end_bar - start_bar).max(1.0 / 64.0);
    pr.song_rev += 1;
}

/// The notes of a request, checked: [{pitch, start, length, velocity?, lyric?, mute?}], with
/// start and length in quarter notes from the region's start.
fn read_notes(list: &[Value]) -> Result<Vec<Note>, RpcError> {
    if list.len() > MAX_NOTES {
        return Err(RpcError::invalid(
            "notes",
            format!("{} notes in one call; the limit is {MAX_NOTES}", list.len()),
        ));
    }
    let mut out = Vec::with_capacity(list.len());
    for (i, v) in list.iter().enumerate() {
        let bad = |what: &str| {
            RpcError::invalid(
                "notes",
                format!(
                    "note {} {what}: a note is {{\"pitch\": 0-127, \"start\": quarter notes from the region's start, \"length\": quarter notes, \"velocity\": 1-127 (default 100), \"lyric\": text, \"mute\": bool}}",
                    i + 1
                ),
            )
        };
        let Some(o) = v.as_object() else {
            return Err(bad("is not an object"));
        };
        if let Some(extra) = o.keys().find(|k| {
            !["pitch", "start", "length", "velocity", "lyric", "mute"].contains(&k.as_str())
        }) {
            return Err(bad(&format!("has an unknown field `{extra}`")));
        }
        let num = |key: &str| o.get(key).and_then(Value::as_f64).filter(|x| x.is_finite());
        let pitch = num("pitch").ok_or_else(|| bad("needs `pitch`"))?;
        if !(0.0..=127.0).contains(&pitch) || pitch.fract() != 0.0 {
            return Err(bad(&format!(
                "has pitch {pitch}; a pitch is a whole number from 0 to 127"
            )));
        }
        let start = num("start").ok_or_else(|| bad("needs `start`"))?;
        if start < 0.0 {
            return Err(bad("starts before the region does"));
        }
        let length = num("length").ok_or_else(|| bad("needs `length`"))?;
        if length <= 0.0 {
            return Err(bad("needs a `length` above 0"));
        }
        if start > MAX_NOTE_BEATS || length > MAX_NOTE_BEATS {
            return Err(bad("is more than a million quarter notes in or long"));
        }
        let vel = match o.get("velocity") {
            None | Some(Value::Null) => 100.0,
            Some(x) => x
                .as_f64()
                .filter(|x| (1.0..=127.0).contains(x))
                .ok_or_else(|| bad("has a velocity outside 1 to 127"))?,
        };
        let lyric = match o.get("lyric") {
            None | Some(Value::Null) => Lyric::NONE,
            Some(Value::String(text)) => Lyric::new(text),
            Some(_) => return Err(bad("has a `lyric` that is not text")),
        };
        out.push(Note {
            start: start as f32,
            len: length as f32,
            pitch: pitch as u8,
            vel: vel as u8,
            mute: o.get("mute").and_then(Value::as_bool).unwrap_or(false),
            lyric,
        });
    }
    Ok(out)
}

fn sort_notes(notes: &mut [Note]) {
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
}

fn region_create(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let pr = &mut cx.app.project;
    let kind = pr.tracks[ti].kind;
    if !kind.has_regions() || kind == TrackKind::Audio {
        return Err(RpcError::new(
            Code::NotSupported,
            format!(
                "a MIDI region goes on an instrument, drummer or external MIDI track; \"{}\" is a {} track (an audio region comes from import.audio or a recording)",
                pr.tracks[ti].name,
                state::kind_name(kind)
            ),
        ));
    }
    let start = req_place(pr, params, "")?;
    let notes = match params.arr("notes") {
        Some(list) => read_notes(list)?,
        None => Vec::new(),
    };
    let len = match length_beats(pr, params, start)? {
        Some(l) if l > 0.0 => l,
        Some(_) => {
            return Err(RpcError::invalid(
                "length_bars",
                "a region's length must be above 0",
            ))
        }
        // As long as its notes, in whole bars (four bars when it has none).
        None => {
            let last = notes
                .iter()
                .map(|n| f64::from(n.start + n.len))
                .fold(0.0, f64::max);
            if last > 0.0 {
                let end_bar = pr.beats_to_bar(start + last).ceil();
                pr.bar_to_beats(end_bar.max(pr.beats_to_bar(start) + 1.0)) - start
            } else {
                pr.bar_to_beats(pr.beats_to_bar(start) + 4.0) - start
            }
        }
    };
    let mut notes = notes;
    sort_notes(&mut notes);
    let start_bar = pr.beats_to_bar(start) as f32 + 1.0;
    let end_bar = pr.beats_to_bar(start + len) as f32 + 1.0;
    let name = params
        .str("name")
        .map(str::to_string)
        .unwrap_or_else(|| pr.tracks[ti].name.clone());
    pr.tracks[ti].regions.push(Region {
        name,
        start_bar,
        len_bars: (end_bar - start_bar).max(1.0 / 64.0),
        notes,
        controls: Vec::new(),
        folder: None,
        audio: None,
        missing: None,
        params: RegionParams::default(),
        generated: None,
    });
    pr.song_rev += 1;
    let ri = pr.tracks[ti].regions.len() - 1;
    if params.bool("select").unwrap_or(true) {
        pr.selected_track = ti;
        cx.app.ui.tracks.select_only(ti, ri);
    }
    cx.ctx.request_repaint();
    ok(region_out(cx, ti, ri, false))
}

/// A MIDI region with notes of its own (not a take folder, a Pattern or a Drummer region,
/// whose notes the app makes).
fn plain_midi(pr: &Project, ti: usize, ri: usize) -> Result<(), RpcError> {
    let r = &pr.tracks[ti].regions[ri];
    let why = if r.is_audio() {
        "an audio region has no notes"
    } else if r.folder.is_some() {
        "a take folder's notes are its comp: flatten it in the app first"
    } else if r.generated.is_some() {
        "a Pattern or Drummer region makes its own notes (Region menu > Convert to MIDI Region makes them yours)"
    } else {
        return Ok(());
    };
    Err(RpcError::new(Code::NotSupported, why))
}

fn region_notes(cx: &mut Cx, params: &P, append: bool) -> Result<Reply, RpcError> {
    let (ti, ri) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
    plain_midi(&cx.app.project, ti, ri)?;
    let Some(list) = params.arr("notes") else {
        return Err(RpcError::invalid(
            "notes",
            "`notes` is needed: a list of notes",
        ));
    };
    let new = read_notes(list)?;
    let pr = &mut cx.app.project;
    let last = new
        .iter()
        .map(|n| f64::from(n.start + n.len))
        .fold(0.0, f64::max);
    {
        let r = &mut pr.tracks[ti].regions[ri];
        if append {
            if r.notes.len() + new.len() > MAX_NOTES {
                return Err(RpcError::invalid(
                    "notes",
                    format!("the region would hold more than {MAX_NOTES} notes"),
                ));
            }
            r.notes.extend(new);
        } else {
            r.notes = new;
        }
        sort_notes(&mut r.notes);
    }
    // A region too short for its notes grows to the bar that holds the last one, unless told
    // not to (notes past the end of a region are not played).
    let len = pr.region_len_beats(&pr.tracks[ti].regions[ri]);
    let mut grown = false;
    if last > len + 1e-6 && params.bool("grow").unwrap_or(true) {
        let start = pr.region_start_beats(&pr.tracks[ti].regions[ri]);
        let end_bar = pr.beats_to_bar(start + last).ceil();
        let new_len = pr.bar_to_beats(end_bar) - start;
        set_length(pr, ti, ri, new_len);
        grown = true;
    }
    pr.song_rev += 1;
    cx.ctx.request_repaint();
    let mut out = region_out(cx, ti, ri, false);
    if let Some(o) = out.as_object_mut() {
        o.insert("grown".into(), grown.into());
    }
    ok(out)
}

fn region_notes_set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    region_notes(cx, params, false)
}

fn region_notes_add(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    region_notes(cx, params, true)
}

/// Moves or copies a region the way a drag in the Tracks area does.
fn drop_region(cx: &mut Cx, params: &P, copy: bool) -> Result<Reply, RpcError> {
    let (ti, ri) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
    let pr = &cx.app.project;
    let start = pr.region_start_beats(&pr.tracks[ti].regions[ri]);
    let to = place(pr, params, "")?;
    let to_track = match params.get("track") {
        Some(v) => state::track_ref(pr, &cx.core.ids, v, "track")?,
        None => ti,
    };
    if to.is_none() && to_track == ti {
        return Err(RpcError::invalid(
            "bar",
            "say where it goes: a place (bar, position, seconds or beat), a `track`, or both",
        ));
    }
    let delta = to.unwrap_or(start) - start;
    let rows = to_track as isize - ti as isize;
    let group = [(ti, ri, start)];
    if !crate::region_edit::fits_rows(pr, &group, rows, true) {
        let region_kind = state::region_kind(&pr.tracks[ti].regions[ri]);
        return Err(RpcError::new(
            Code::NotSupported,
            format!(
                "a {region_kind} region cannot go on \"{}\", a {} track (audio regions go on audio tracks, the others on tracks that play MIDI)",
                pr.tracks[to_track].name,
                state::kind_name(pr.tracks[to_track].kind)
            ),
        ));
    }
    let xfade = params.bool("crossfade") == Some(true);
    let landed = crate::region_edit::drop_regions(
        &mut cx.app.project,
        &cx.app.ui.auto,
        &group,
        delta,
        rows,
        copy,
        xfade,
    );
    let Some(&(lt, lr)) = landed.first() else {
        return Err(RpcError::failed("the region could not be put there"));
    };
    cx.app.ui.tracks.select_many(&landed);
    cx.app.project.selected_track = lt;
    cx.ctx.request_repaint();
    ok(region_out(cx, lt, lr, false))
}

fn region_move(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    drop_region(cx, params, false)
}

fn region_copy(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    drop_region(cx, params, true)
}

fn region_resize(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let (ti, ri) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
    let pr = &mut cx.app.project;
    let start = pr.region_start_beats(&pr.tracks[ti].regions[ri]);
    let len = match (length_beats(pr, params, start)?, place(pr, params, "end_")?) {
        (Some(l), _) => l,
        (None, Some(end)) => end - start,
        (None, None) => {
            return Err(RpcError::invalid(
                "length_bars",
                "give the new length (length_bars or length_beats) or where it ends (end_bar, end_position, end_seconds or end_beat)",
            ))
        }
    };
    if len <= 0.0 {
        return Err(RpcError::invalid(
            "length_bars",
            "a region must end after it starts",
        ));
    }
    if pr.tracks[ti].regions[ri].generated.is_none() && pr.tracks[ti].regions[ri].folder.is_some() {
        return Err(RpcError::new(
            Code::NotSupported,
            "a take folder keeps the length of its takes",
        ));
    }
    set_length(pr, ti, ri, len);
    cx.ctx.request_repaint();
    ok(region_out(cx, ti, ri, false))
}

fn region_split(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let (ti, ri) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
    let pr = &mut cx.app.project;
    let at = req_place(pr, params, "")?;
    if !pr.can_split_region(ti, ri, at) {
        let r = &pr.tracks[ti].regions[ri];
        let (start, end) = (pr.region_start_beats(r), pr.region_end_beats(r));
        let why = if at <= start + 1e-6 || at >= end - 1e-6 {
            format!(
                "the cut must be inside the region, which runs from bar {} to bar {}",
                round6(pr.beats_to_bar(start) + 1.0),
                round6(pr.beats_to_bar(end) + 1.0)
            )
        } else {
            "take folders, Pattern and Drummer regions and reversed audio are not cut".to_string()
        };
        return Err(RpcError::new(Code::NotSupported, why));
    }
    if !pr.split_region(ti, ri, at) {
        return Err(RpcError::failed("Cannot cut this region here"));
    }
    cx.app.ui.tracks.select_only(ti, ri + 1);
    cx.ctx.request_repaint();
    cx.core.ids.reconcile(&cx.app.project);
    let (pr, ids) = (&cx.app.project, &cx.core.ids);
    ok(json!({
        "left": state::region_json(pr, ids, ti, ri, false),
        "right": state::region_json(pr, ids, ti, ri + 1, false),
    }))
}

fn region_join(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let Some(list) = params.arr("regions") else {
        return Err(RpcError::invalid(
            "regions",
            "`regions` is needed: the regions to join",
        ));
    };
    let mut picked = Vec::new();
    for v in list {
        picked.push(state::region_ref(
            &cx.app.project,
            &cx.core.ids,
            v,
            "regions",
        )?);
    }
    cx.app.ui.tracks.select_many(&picked);
    if !crate::region_edit::can_join(&cx.app.project, &cx.app.ui.tracks) {
        return Err(RpcError::new(
            Code::NotSupported,
            "These regions cannot be joined: Edit > Join takes two or more plain MIDI regions (or audio regions cut from one file, next to each other) on one track",
        ));
    }
    crate::region_edit::join_selected(&mut cx.app.project, &mut cx.app.ui.tracks);
    cx.ctx.request_repaint();
    let Some((ti, ri)) = cx.app.ui.tracks.selected else {
        return ok(json!({"joined": true}));
    };
    ok(region_out(cx, ti, ri, false))
}

fn region_delete(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let (ti, ri) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
    let id = cx.core.ids.region_id(ti, ri);
    let name = cx.app.project.tracks[ti].regions[ri].name.clone();
    // Select it and delete the selection, as the Delete key does.
    cx.app.ui.tracks.select_only(ti, ri);
    crate::tracks_area::delete_selected(&mut cx.app.project, &mut cx.app.ui.tracks);
    cx.app.ui.piano.selected.clear();
    cx.ctx.request_repaint();
    ok(json!({"deleted": id, "name": name}))
}

fn region_set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let (ti, ri) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
    let pr = &mut cx.app.project;
    let audio = pr.tracks[ti].regions[ri].is_audio();
    for key in ["gain_db", "fade_in_beats", "fade_out_beats", "reverse"] {
        if params.has(key) && !audio {
            return Err(RpcError::new(
                Code::NotSupported,
                format!("`{key}` is for audio regions; this is a MIDI region"),
            ));
        }
    }
    for key in ["transpose", "velocity_offset", "quantize_beats"] {
        if params.has(key) && audio {
            return Err(RpcError::new(
                Code::NotSupported,
                format!("`{key}` is for MIDI regions; this is an audio region"),
            ));
        }
    }
    let total = pr.region_len_beats(&pr.tracks[ti].regions[ri]);
    let r = &mut pr.tracks[ti].regions[ri];
    if let Some(name) = params.str("name") {
        if name.trim().is_empty() {
            return Err(RpcError::invalid("name", "a region's name cannot be empty"));
        }
        r.name = name.trim().chars().take(64).collect();
    }
    if let Some(on) = params.bool("mute") {
        r.params.mute = on;
    }
    if let Some(beats) = params.f32("loop_beats") {
        // The loop runs for this many quarter notes after the region's end; 0 ends it.
        let most = total as f32 * crate::model::MAX_LOOP_REPEATS as f32;
        r.params.loop_beats = beats.clamp(0.0, most.max(0.0));
    }
    if let Some(n) = params.i64("transpose") {
        r.params.transpose = n.clamp(-96, 96) as i8;
    }
    if let Some(n) = params.i64("velocity_offset") {
        r.params.velocity = n.clamp(-99, 99) as i8;
    }
    if let Some(q) = params.f32("quantize_beats") {
        r.params.quantize = (q > 0.0).then_some(q);
    }
    if let Some(db) = params.f32("gain_db") {
        r.params.gain_db = db.clamp(crate::model::MIN_REGION_GAIN, crate::model::MAX_REGION_GAIN);
    }
    if let Some(b) = params.f32("fade_in_beats") {
        r.params.fade_in = b.clamp(0.0, total as f32);
    }
    if let Some(b) = params.f32("fade_out_beats") {
        r.params.fade_out = b.clamp(0.0, total as f32);
    }
    if let Some(on) = params.bool("reverse") {
        r.params.reverse = on;
    }
    pr.song_rev += 1;
    cx.ctx.request_repaint();
    ok(region_out(cx, ti, ri, false))
}

fn region_loop(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let (ti, ri) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
    let pr = &mut cx.app.project;
    let len = pr.region_len_beats(&pr.tracks[ti].regions[ri]);
    let end = pr.region_end_beats(&pr.tracks[ti].regions[ri]);
    let beats = if params.bool("off") == Some(true) {
        0.0
    } else if let Some(n) = params.f64("repeats") {
        n * len
    } else if let Some(until) = place(pr, params, "end_")? {
        if until <= end {
            return Err(RpcError::invalid(
                "end_bar",
                format!(
                    "the loop must end after the region does (bar {})",
                    round6(pr.beats_to_bar(end) + 1.0)
                ),
            ));
        }
        until - end
    } else {
        return Err(RpcError::invalid(
            "repeats",
            "give `repeats` (how many times it plays again), where the loop ends (end_bar, end_position, end_seconds or end_beat), or off: true",
        ));
    };
    let most = len * crate::model::MAX_LOOP_REPEATS as f64;
    pr.tracks[ti].regions[ri].params.loop_beats = beats.clamp(0.0, most) as f32;
    pr.song_rev += 1;
    cx.ctx.request_repaint();
    ok(region_out(cx, ti, ri, false))
}

// ------------------------------------------------------------------ files in and out

fn import_audio(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let path = params.req_path("path")?;
    if !path.is_file() {
        return Err(RpcError::not_found(format!(
            "there is no file {}",
            path.display()
        )));
    }
    if !crate::import::is_audio_path(&path) {
        return Err(RpcError::invalid(
            "path",
            format!(
                "not an audio file: {} (the app reads WAV, AIFF, FLAC, MP3 and CAF)",
                path.display()
            ),
        ));
    }
    let at = place(&cx.app.project, params, "")?;
    let onto = match params.get("track") {
        Some(v) => {
            let t = state::track_ref(&cx.app.project, &cx.core.ids, v, "track")?;
            if cx.app.project.tracks[t].kind != TrackKind::Audio {
                return Err(RpcError::new(
                    Code::NotSupported,
                    format!(
                        "audio goes on an audio track; \"{}\" is a {} track (leave `track` out to get a new audio track)",
                        cx.app.project.tracks[t].name,
                        state::kind_name(cx.app.project.tracks[t].kind)
                    ),
                ));
            }
            Some(t)
        }
        None => None,
    };
    let fit = params.bool("fit_to_grid").unwrap_or(cx.app.ui.fit.on_drop);
    // The app's own import: a new audio track with the file as a region at the bar under
    // the playhead, which is put at the place asked for and back afterwards.
    let (playhead, selected) = (cx.app.project.position_beats, cx.app.project.selected_track);
    if let Some(at) = at {
        cx.app.project.position_beats = at;
    }
    let first = crate::fit_grid::would_be_first(&cx.app.project);
    let origin = cx.app.project.position_beats;
    let done = crate::import::import_audio(&mut cx.app.project, &path, cx.ctx);
    cx.app.project.position_beats = playhead;
    if let Err(e) = done {
        return Err(RpcError::failed(format!(
            "cannot import {}: {e}",
            path.display()
        )));
    }
    let mut ti = cx.app.project.tracks.len() - 1;
    let mut ri = 0;
    if let Some(target) = onto {
        // Onto a track that is there: the region moves over and the new track goes.
        if let Some(mut made) = cx.app.project.tracks.pop() {
            if let Some(region) = made.regions.pop() {
                cx.app.project.tracks[target].regions.push(region);
            }
        }
        ti = target;
        ri = cx.app.project.tracks[ti].regions.len() - 1;
        cx.app.project.selected_track = selected.min(cx.app.project.tracks.len() - 1);
    }
    cx.app.project.song_rev += 1;
    if fit {
        // Fit to Grid, as a drop on the window starts it: the file is read in the background
        // and the region fitted when that is done (the event import.fitted, or state.get's
        // busy.fit_to_grid going false).
        crate::fit_grid::start_drop(&cx.app.project, &mut cx.app.ui, ti, ri, first, origin);
    }
    cx.app.ui.tracks.select_only(ti, ri);
    cx.ctx.request_repaint();
    let mut out = region_out(cx, ti, ri, false);
    if let Some(o) = out.as_object_mut() {
        o.insert("fit_to_grid".into(), fit.into());
        o.insert("fitting".into(), cx.app.ui.fit.busy().into());
        if fit {
            o.insert(
                "note".into(),
                "Fit to Grid reads the file in the background: the region's place and length, and the project's tempo when this is its first audio, are final once state.get's busy.fit_to_grid is false (event: import.fitted)".into(),
            );
        }
    }
    ok(out)
}

fn import_midi(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let path = params.req_path("path")?;
    if !path.is_file() {
        return Err(RpcError::not_found(format!(
            "there is no file {}",
            path.display()
        )));
    }
    if !crate::midi_file::is_midi_path(&path) {
        return Err(RpcError::invalid(
            "path",
            format!("not a MIDI file: {} (.mid, .midi or .smf)", path.display()),
        ));
    }
    let at = place(&cx.app.project, params, "")?.unwrap_or(0.0);
    let before = cx.app.project.tracks.len();
    let made = crate::midi_file::import_path_at(&mut cx.app.project, &path, at)
        .map_err(|e| RpcError::failed(format!("cannot import {}: {e}", path.display())))?;
    cx.ctx.request_repaint();
    cx.core.ids.reconcile(&cx.app.project);
    let tracks: Vec<Value> = (before..cx.app.project.tracks.len())
        .map(|i| state::track_json(&cx.app.project, &cx.core.ids, i, true, false))
        .collect();
    ok(json!({
        "tracks_made": made,
        "tracks": tracks,
        "note": "each track of the file becomes a new track with one region; the file's tempo and signature replace the project's only when the project had no regions",
    }))
}

fn export_midi(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let path = params.req_path("path")?;
    let only = match params.get("region") {
        Some(v) => {
            let (t, r) = state::region_ref(&cx.app.project, &cx.core.ids, v, "region")?;
            if cx.app.project.tracks[t].regions[r].is_audio() {
                return Err(RpcError::new(
                    Code::NotSupported,
                    "an audio region has no MIDI to write",
                ));
            }
            Some((t, r))
        }
        None => None,
    };
    if path.exists() && params.bool("overwrite") != Some(true) {
        return Err(RpcError::failed(format!(
            "{} is already there: pass overwrite: true to replace it",
            path.display()
        )));
    }
    let follow = params.bool("apply_follow_groove").unwrap_or(true);
    crate::midi_file::export_path_with(&cx.app.project, only, &path, follow)
        .map_err(|e| RpcError::failed(format!("cannot write {}: {e}", path.display())))?;
    ok(json!({
        "path": path.display().to_string(),
        "whole_project": only.is_none(),
        "apply_follow_groove": follow,
    }))
}

fn region_fit(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let (ti, ri) = state::req_region(&cx.app.project, &cx.core.ids, params, "region")?;
    if cx.app.project.tracks[ti].regions[ri].audio.is_none() {
        return Err(RpcError::new(
            Code::NotSupported,
            "Fit to Grid: this region has no recorded audio",
        ));
    }
    crate::fit_grid::start_region(&cx.app.project, &mut cx.app.ui, ti, ri);
    cx.ctx.request_repaint();
    ok(json!({
        "region": cx.core.ids.region_id(ti, ri),
        "fitting": cx.app.ui.fit.busy(),
        "status": cx.app.ui.status,
    }))
}

// ------------------------------------------------------------------ markers, tempo, signature

fn marker_ref(pr: &Project, params: &P) -> Result<usize, RpcError> {
    let listing = || {
        if pr.markers.is_empty() {
            "the project has no markers".to_string()
        } else {
            format!(
                "the markers are: {}",
                pr.markers
                    .iter()
                    .enumerate()
                    .map(|(i, mk)| format!("{} \"{}\"", i + 1, mk.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };
    match params.get("marker") {
        Some(Value::Number(n)) => {
            let i = n.as_f64().unwrap_or(0.0) as i64;
            if i >= 1 && (i as usize) <= pr.markers.len() {
                Ok(i as usize - 1)
            } else {
                Err(RpcError::not_found(format!(
                    "there is no marker number {n}; {}",
                    listing()
                )))
            }
        }
        Some(Value::String(name)) => {
            let found: Vec<usize> = (0..pr.markers.len())
                .filter(|i| pr.markers[*i].name.eq_ignore_ascii_case(name))
                .collect();
            match found.as_slice() {
                [i] => Ok(*i),
                [] => Err(RpcError::not_found(format!(
                    "there is no marker \"{name}\"; {}",
                    listing()
                ))),
                _ => Err(RpcError::invalid(
                    "marker",
                    format!(
                        "several markers are named \"{name}\": use its number; {}",
                        listing()
                    ),
                )),
            }
        }
        _ => Err(RpcError::invalid(
            "marker",
            format!(
                "`marker` is needed: a marker's number (from 1, in song order) or its name; {}",
                listing()
            ),
        )),
    }
}

fn marker_list(cx: &mut Cx, _: &P) -> Result<Reply, RpcError> {
    ok(json!({"markers": state::markers_json(&cx.app.project)}))
}

fn marker_add(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let pr = &mut cx.app.project;
    let at = place(pr, params, "")?.unwrap_or(pr.position_beats);
    let i = pr.add_marker(at);
    if let Some(name) = params.str("name") {
        if !name.trim().is_empty() {
            pr.markers[i].name = name.trim().to_string();
        }
    }
    cx.app.ui.tracks.show_markers = true;
    ok(json!({"marker": i + 1, "markers": state::markers_json(&cx.app.project)}))
}

fn marker_set(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let pr = &mut cx.app.project;
    let i = marker_ref(pr, params)?;
    if let Some(name) = params.str("name") {
        if name.trim().is_empty() {
            return Err(RpcError::invalid("name", "a marker's name cannot be empty"));
        }
        pr.markers[i].name = name.trim().to_string();
    }
    if let Some(at) = place(pr, params, "")? {
        if pr
            .markers
            .iter()
            .enumerate()
            .any(|(k, mk)| k != i && (mk.beat - at).abs() <= 1e-6)
        {
            return Err(RpcError::failed("another marker is at that place"));
        }
        pr.markers[i].beat = at;
        pr.tidy_markers();
    }
    ok(json!({"markers": state::markers_json(&cx.app.project)}))
}

fn marker_delete(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let pr = &mut cx.app.project;
    let i = marker_ref(pr, params)?;
    let gone = pr.markers.remove(i);
    ok(json!({"deleted": gone.name, "markers": state::markers_json(&cx.app.project)}))
}

fn tempo_list(cx: &mut Cx, _: &P) -> Result<Reply, RpcError> {
    ok(json!({
        "tempo_changes": state::tempo_json(&cx.app.project),
        "signatures": state::signatures_json(&cx.app.project),
    }))
}

fn tempo_add(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let pr = &mut cx.app.project;
    let at = req_place(pr, params, "")?;
    let bpm = params.req_f64("bpm")? as f32;
    let i = pr.add_tempo_change(at, bpm);
    if let Some(ramp) = params.bool("ramp") {
        // Point 0 is the start tempo; change `i` is point `i + 1`.
        let point = if i == usize::MAX { 0 } else { i + 1 };
        pr.set_tempo_ramp(point, ramp);
    }
    pr.song_rev += 1;
    cx.app.ui.tracks.show_tempo = true;
    ok(json!({"tempo_changes": state::tempo_json(&cx.app.project)}))
}

fn tempo_remove(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let pr = &mut cx.app.project;
    let at = req_place(pr, params, "")?;
    let Some(k) = pr
        .tempo_changes
        .iter()
        .position(|t| (t.beat - at).abs() <= 1e-3)
    else {
        return Err(RpcError::not_found(format!(
            "there is no tempo change at {}; the start tempo (bar 1) cannot be removed, only set (transport.tempo). tempo.list shows the changes",
            place_json(pr, at)["position"]
        )));
    };
    pr.tempo_changes.remove(k);
    pr.tidy_tempo();
    pr.song_rev += 1;
    ok(json!({"tempo_changes": state::tempo_json(&cx.app.project)}))
}

fn signature_remove(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let bar = params.i64("bar").unwrap_or(0).max(0) as u32;
    if bar <= 1 {
        return Err(RpcError::invalid(
            "bar",
            "bar 1 holds the song's own signature, which is set with transport.signature, not removed",
        ));
    }
    if !cx.app.project.remove_signature_at(bar) {
        return Err(RpcError::not_found(format!(
            "no signature change starts at bar {bar} (tempo.list shows them)"
        )));
    }
    ok(json!({"signatures": state::signatures_json(&cx.app.project)}))
}

pub fn methods() -> Vec<Method> {
    let region = || {
        p(
            "region",
            Ty::Str,
            "the region: its id (\"r12\") or its name when only one region has it",
        )
        .req()
    };
    let [bar, position, seconds, beat] = place_params("");
    let [end_bar, end_position, end_seconds, end_beat] = place_params("end_");
    let notes_desc = "the notes: [{\"pitch\": 60, \"start\": 0, \"length\": 1, \"velocity\": 100, \"lyric\": \"la\"}, ...]; start and length in quarter notes from the region's start; velocity (default 100), lyric and mute may be left out";
    vec![
        m("region.get", "One region: where it is, how long, and its notes or its audio file.",
            vec![region(), p("notes", Ty::Bool, "include a MIDI region's notes (default true)")],
            "{id, track, name, kind: midi|audio|pattern|drummer, start_bar, length_bars, start_beat, length_beats, start_seconds, mute, loop_beats, and for MIDI: transpose, velocity_offset, quantize_beats, note_count, take_folder, notes: [{pitch, start, length, velocity, mute?, lyric?}]; for audio: audio: {file, file_seconds, sample_rate, channels, flex, offset_seconds, gain_db, fade_in_beats, fade_out_beats, reverse}}",
            region_get),
        m("region.create", "Makes a MIDI region on a track, empty or with notes.",
            vec![
                p("track", Ty::Any, "the track: id, name or number").req(),
                bar, position, seconds, beat,
                p("length_bars", Ty::Num, "its length in bars (default: as long as its notes, in whole bars; 4 bars when empty)").unit("bars").range(0.0, 100_000.0),
                p("length_beats", Ty::Num, "or its length in quarter notes").unit("quarter notes").range(0.0, 1.0e7),
                p("name", Ty::Str, "its name (default: the track's)"),
                p("notes", Ty::Arr, notes_desc),
                p("select", Ty::Bool, "select it, so the editors show it (default true)"),
            ],
            "the new region as region.get gives it, without its notes", region_create).changes(),
        m("region.notes.set", "Replaces all the notes of a MIDI region.",
            vec![region(), p("notes", Ty::Arr, notes_desc).req(), p("grow", Ty::Bool, "lengthen the region to the bar that holds the last note (default true)")],
            "the region, with grown", region_notes_set).changes(),
        m("region.notes.add", "Adds notes to a MIDI region, keeping the ones it has.",
            vec![region(), p("notes", Ty::Arr, notes_desc).req(), p("grow", Ty::Bool, "lengthen the region to the bar that holds the last note (default true)")],
            "the region, with grown", region_notes_add).changes(),
        m("region.move", "Moves a region in time and/or to another track, as dragging it does (its automation goes along when the app is set to move it with regions).",
            vec![region(), bar, position, seconds, beat, p("track", Ty::Any, "the track to move it to (default: it stays on its track)"), p("crossfade", Ty::Bool, "audio: crossfade with a region it lands on")],
            "the region where it is now", region_move).changes(),
        m("region.copy", "Copies a region to another place and/or track, as Option-dragging it does.",
            vec![region(), bar, position, seconds, beat, p("track", Ty::Any, "the track the copy goes on (default: the region's own)"), p("crossfade", Ty::Bool, "audio: crossfade with a region it lands on")],
            "the copy", region_copy).changes(),
        m("region.resize", "Changes a region's length, as dragging its right edge does.",
            vec![
                region(),
                p("length_bars", Ty::Num, "the new length in bars").unit("bars").range(0.0, 100_000.0),
                p("length_beats", Ty::Num, "or in quarter notes").unit("quarter notes").range(0.0, 1.0e7),
                end_bar, end_position, end_seconds, end_beat,
            ],
            "the region", region_resize).changes(),
        m("region.split", "Cuts a region in two at a place (the Scissors tool).",
            vec![region(), bar, position, seconds, beat], "{left, right}", region_split).changes(),
        m("region.join", "Joins regions of one track into one (Edit > Join).",
            vec![p("regions", Ty::Arr, "the regions to join: ids or names").req()], "the joined region", region_join).changes(),
        m("region.loop", "Loops a region: it plays again and again after its end.",
            vec![
                region(),
                p("repeats", Ty::Num, "how many times it plays again after the first").range(0.0, 4096.0),
                end_bar, end_position, end_seconds, end_beat,
                p("off", Ty::Bool, "true: no loop"),
            ],
            "the region; loop_beats is how long the loop runs after the region's end", region_loop).changes(),
        m("region.set", "Sets a region's name, mute and Region Inspector values.",
            vec![
                region(),
                p("name", Ty::Str, "its name"),
                p("mute", Ty::Bool, "muted regions do not play"),
                p("loop_beats", Ty::Num, "how long the loop runs after the region's end; 0 is no loop").unit("quarter notes").range(0.0, 1.0e7),
                p("transpose", Ty::Int, "MIDI: semitones").unit("semitones").range(-96.0, 96.0),
                p("velocity_offset", Ty::Int, "MIDI: added to every note's velocity").range(-99.0, 99.0),
                p("quantize_beats", Ty::Num, "MIDI: playback quantize grid in quarter notes (0.25 is a sixteenth); 0 is off").range(0.0, 16.0),
                p("gain_db", Ty::Num, "audio: region gain").unit("dB").range(-96.0, 24.0),
                p("fade_in_beats", Ty::Num, "audio: fade in").unit("quarter notes").range(0.0, 1.0e6),
                p("fade_out_beats", Ty::Num, "audio: fade out").unit("quarter notes").range(0.0, 1.0e6),
                p("reverse", Ty::Bool, "audio: play it backwards"),
            ],
            "the region", region_set).changes(),
        m("region.delete", "Deletes a region.",
            vec![region()], "{deleted, name}", region_delete).changes(),
        m("region.fit_to_grid", "Fits an audio region to the grid where it is (Region menu > Flex Time > Fit to Grid). It sets the project's tempo when it is the project's only audio.",
            vec![region()], "{region, fitting, status}; the work runs in the background (event import.fitted)", region_fit).changes(),
        m("import.audio", "Imports an audio file with the app's own import: a new audio track (or a track that is there) gets the file as a region at the bar of the place given.",
            vec![
                p("path", Ty::Str, "the audio file (WAV, AIFF, FLAC, MP3, CAF): an absolute path, or one starting with ~/").req(),
                bar, position, seconds, beat,
                p("track", Ty::Any, "an audio track to put it on (default: a new audio track named after the file)"),
                p("fit_to_grid", Ty::Bool, "Fit to Grid: the project's first audio gives it its tempo, later files are stretched to it; either way onto the grid (default: the app's File > Import > Fit to Grid on Drop setting)"),
            ],
            "the new region, with fit_to_grid and fitting; without a place it goes at the bar under the playhead", import_audio).changes(),
        m("import.midi", "Imports a MIDI file: each of its tracks becomes a new track with one region.",
            vec![p("path", Ty::Str, "the .mid file").req(), bar, position, seconds, beat],
            "{tracks_made, tracks}; a place puts the file's start there (default: the start of the song)", import_midi).changes(),
        m("export.midi", "Writes the project's MIDI, or one region's, to a MIDI file.",
            vec![p("path", Ty::Str, "the .mid file to write").req(), p("region", Ty::Str, "only this region (default: every MIDI region)"), p("overwrite", Ty::Bool, "replace a file that is already there"), p("apply_follow_groove", Ty::Bool, "write the notes of tracks following a Channel Rack with its feel, as they play (default true); false: as stored, on the straight grid")],
            "{path, whole_project, apply_follow_groove}", export_midi),
        m("marker.list", "The markers, in song order.", vec![], "{markers: [{number, name, at, color}]}", marker_list),
        m("marker.add", "Adds a marker at a place (default: the playhead).",
            vec![bar, position, seconds, beat, p("name", Ty::Str, "its name (default \"Marker N\")")],
            "{marker: its number, markers}", marker_add).changes(),
        m("marker.set", "Renames a marker and/or moves it.",
            vec![p("marker", Ty::Any, "the marker: its number (from 1, in song order) or its name").req(), p("name", Ty::Str, "a new name"), bar, position, seconds, beat],
            "{markers}", marker_set).changes(),
        m("marker.delete", "Deletes a marker.",
            vec![p("marker", Ty::Any, "the marker: its number or its name").req()], "{deleted, markers}", marker_delete).changes(),
        m("tempo.list", "The Tempo track's points and the Signature track's changes.",
            vec![], "{tempo_changes: [{at, bpm, ramp}], signatures: [{bar, numerator, denominator}]}", tempo_list),
        m("tempo.add", "Adds a tempo change at a place (or replaces the one there).",
            vec![
                bar, position, seconds, beat,
                p("bpm", Ty::Num, "the tempo from there on").req().unit("bpm").range(f64::from(MIN_BPM), f64::from(MAX_BPM)),
                p("ramp", Ty::Bool, "this point ramps to the next one (a tempo curve)"),
            ],
            "{tempo_changes}", tempo_add).changes(),
        m("tempo.remove", "Removes the tempo change at a place.",
            vec![bar, position, seconds, beat], "{tempo_changes}", tempo_remove).changes(),
        m("signature.remove", "Removes the signature change that starts at a bar (transport.signature adds one).",
            vec![p("bar", Ty::Int, "the bar the change starts at (not 1)").req().range(2.0, 9999.0)],
            "{signatures}", signature_remove).changes(),
    ]
}
