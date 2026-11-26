//! Methods for the drum-kit library and the Kit Browser (the Kits key of the Channel Rack):
//! the library folders, a search of the kits, a kit's samples and where a load puts them,
//! loading a kit or one sample into a rack, and the audition. The folders are read before the
//! answer here; the kits' files are read after it, in the background (`listing` in the status
//! while they are), and a kit asked for before its files are read is read then.
use super::params::P;
use super::protocol::RpcError;
use super::registry::{m, p, Method, Reply, Ty};
use super::state;
use super::{ok, Cx};
use crate::channel_rack;
use crate::kit_browser;
use crate::kit_files::Kit;
use crate::kit_library::Filter;
use serde_json::{json, Value};

/// The kit list: what is set, whether it is read and on, how many of its kits were found.
fn list_json(v: &kit_browser::KitBrowser) -> Value {
    let types: Vec<Value> = v
        .index
        .types
        .iter()
        .map(|(t, n)| json!({ "type": t, "kits": n }))
        .collect();
    json!({
        "path": v.settings.kit_list,
        "on": v.settings.kit_list_on,
        "active": v.list_active(),
        "file": v.list_now().map(|l| l.file.clone()),
        "rows": v.list_now().map(|l| l.rows.len()),
        "matched": v.index.list_matched(),
        "types": types,
        "error": match &v.list {
            Some(Err(e)) => Some(e.clone()),
            _ => None,
        },
    })
}

fn status_json(cx: &Cx) -> Value {
    let v = &cx.app.ui.kits;
    let folders: Vec<Value> = v
        .settings
        .folders
        .iter()
        .map(|f| {
            let info = v.index.roots.iter().find(|r| &r.path == f);
            json!({
                "path": f,
                "present": info.is_some_and(|r| r.present),
                "kits": info.map_or(0, |r| r.kits),
            })
        })
        .collect();
    let sources: Vec<Value> = v
        .index
        .sources
        .iter()
        .map(|(name, kits)| json!({ "name": name, "kits": kits }))
        .collect();
    let listed = v.index.kits.iter().filter(|k| k.listed.is_some()).count();
    json!({
        "folders": folders,
        "kits": if v.list_active() { v.results_all() } else { v.index.kits.len() },
        "all_kits": v.index.kits.len(),
        "kit_list": list_json(v),
        "listed": listed,
        "pools": v.index.kits.iter().filter(|k| k.is_pool()).count(),
        "sources": sources,
        "scanning": v.scanning,
        "listing": v.listing.map(|(done, total)| json!({ "done": done, "total": total })),
        "indexing": v.indexing(),
        "scan_ms": v.last_scan.map(|d| super::params::round3(d.as_secs_f64() * 1000.0)),
        "last_listing": v.last_listing.map(|(read, d)| json!({
            "read": read,
            "ms": super::params::round3(d.as_secs_f64() * 1000.0),
        })),
        "favorites": v.settings.favorites.len(),
        "audition": v.settings.audition,
        "default_kit": v.settings.default_kit,
        "copy_kits": v.settings.copy_kits.id(),
        "copying": kit_browser::copying(v),
        "local": v.local_dir().map(|d| d.to_string_lossy().into_owned()),
    })
}

/// The index is there: the folders read once before the first answer, and what the
/// background scan has done since taken in.
fn ready(cx: &mut Cx) {
    let v = &mut cx.app.ui.kits;
    v.start();
    v.poll();
    if v.last_scan.is_none() {
        v.scan_now(false);
    }
}

fn status(cx: &mut Cx, _: &P) -> Result<Reply, RpcError> {
    ready(cx);
    ok(status_json(cx))
}

fn rescan(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    cx.app.ui.kits.scan_now(params.bool("full").unwrap_or(true));
    ok(status_json(cx))
}

fn folders(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let Some(list) = params.arr("folders") else {
        return Err(RpcError::invalid(
            "folders",
            "`folders` is needed: the library folders, a list of paths",
        ));
    };
    let mut folders = Vec::with_capacity(list.len());
    for f in list {
        let Some(text) = f.as_str() else {
            return Err(RpcError::invalid("folders", "`folders` is a list of paths"));
        };
        let path = super::params::expand_path(text).map_err(|e| RpcError::invalid("folders", e))?;
        folders.push(path.to_string_lossy().into_owned());
    }
    let v = &mut cx.app.ui.kits;
    v.set_folders(folders);
    v.scan_now(false);
    ok(status_json(cx))
}

fn search(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    ready(cx);
    let v = &mut cx.app.ui.kits;
    let filter = Filter {
        query: params.str("query").unwrap_or("").to_string(),
        source: params.str("source").map(str::to_string),
        best: params.bool("best").unwrap_or(false),
        favorites: params.bool("favorites").unwrap_or(false),
        list: params.bool("list").unwrap_or(true) && v.list_active(),
        kind: params.str("type").map(str::to_string),
    };
    let hits = crate::kit_library::search_keeping(
        &v.index,
        &filter,
        &v.settings.favorites,
        &v.settings.default_kit,
    );
    let offset = params.i64("offset").unwrap_or(0).max(0) as usize;
    let limit = params.i64("limit").unwrap_or(50).clamp(1, 1000) as usize;
    let kits: Vec<Value> = hits
        .iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            let k = &v.index.kits[*i as usize];
            json!({
                "path": k.path,
                "source": k.source,
                "name": k.name,
                "files": k.files,
                "license": k.license,
                "best": k.best,
                "favorite": v.is_favorite(&k.path),
                "channels": k.pads,
                "pool": k.is_pool(),
                "rank": k.list.map(|l| l.0),
                "type": k.list.and_then(|l| v.index.types.get(usize::from(l.1)).map(|t| t.0.clone())),
            })
        })
        .collect();
    ok(json!({ "total": hits.len(), "offset": offset, "kits": kits }))
}

/// The kit in the folder `path` names, read now.
fn read(cx: &mut Cx, params: &P) -> Result<(String, std::sync::Arc<Kit>), RpcError> {
    ready(cx);
    let path = params.req_path("path")?;
    if !path.is_dir() {
        return Err(RpcError::not_found(format!(
            "there is no folder {}",
            path.display()
        )));
    }
    let path = path.to_string_lossy().into_owned();
    let kit = cx.app.ui.kits.select_now(&path);
    Ok((path, kit))
}

fn kit_json(path: &str, kit: &Kit, offset: usize, limit: usize) -> Value {
    let samples: Vec<Value> = kit
        .samples
        .iter()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(i, s)| {
            json!({
                "index": i,
                "file": s.rel,
                "name": s.name,
                "class": s.class.id(),
            })
        })
        .collect();
    let channels: Vec<Value> = kit
        .pads
        .iter()
        .enumerate()
        .map(|(k, pad)| match pad {
            Some(i) => json!({
                "channel": k + 1,
                "file": kit.samples[*i].rel,
                "class": kit.samples[*i].class.id(),
            }),
            None => json!({ "channel": k + 1, "file": null, "class": null }),
        })
        .collect();
    let classes: Vec<Value> = kit
        .classes
        .iter()
        .map(|(c, n)| json!({ "class": c.id(), "files": n }))
        .collect();
    let audition: Vec<&str> = kit
        .audition_samples()
        .into_iter()
        .map(|i| kit.samples[i].rel.as_str())
        .collect();
    json!({
        "path": path,
        "kind": kit.kind.id(),
        "files": kit.samples.len(),
        "classes": classes,
        "offset": offset,
        "samples": samples,
        "channels": channels,
        "audition": audition,
    })
}

fn get(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let (path, kit) = read(cx, params)?;
    let offset = params.i64("offset").unwrap_or(0).max(0) as usize;
    let limit = params.i64("limit").unwrap_or(200).max(0) as usize;
    ok(kit_json(&path, &kit, offset, limit))
}

fn rack_json(cx: &Cx, ti: usize) -> Value {
    let slot = cx.app.project.tracks[ti].au_instrument.as_ref();
    let channels: Vec<Value> = slot
        .map(|s| channel_rack::decode(&s.state))
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(k, c)| json!({ "channel": k + 1, "name": c.name, "path": c.path, "class": c.class.id() }))
        .collect();
    json!({
        "track": state::strip_id(&cx.core.ids, ti),
        "kit": slot.map(|s| channel_rack::kit_of(&s.state)).unwrap_or_default(),
        "channels": channels,
    })
}

/// The rack track a request names, or the selected track when it plays the rack.
fn rack_of(cx: &mut Cx, params: &P) -> Result<Option<usize>, RpcError> {
    if !params.has("track") {
        return Ok(channel_rack::rack_track(&cx.app.project));
    }
    let ti = state::req_track(&cx.app.project, &cx.core.ids, params, "track")?;
    let is_rack = cx.app.project.tracks[ti]
        .au_instrument
        .as_ref()
        .is_some_and(channel_rack::is_rack);
    if !is_rack {
        return Err(RpcError::failed(format!(
            "track \"{}\" does not play the Channel Rack",
            cx.app.project.tracks[ti].name
        )));
    }
    Ok(Some(ti))
}

fn load(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let track = rack_of(cx, params)?;
    let (path, kit) = read(cx, params)?;
    if kit.pads.is_empty() {
        // A kit of SoundFonts alone: on a new track, as its SoundFont.
        if let Some(i) = kit.soundfonts().first() {
            let name = cx
                .app
                .ui
                .kits
                .index
                .find(&path)
                .map(|k| k.name.clone())
                .unwrap_or_default();
            let sf = cx.app.ui.kits.localize(&kit.path_of(*i));
            let ti = kit_browser::load_soundfont(&mut cx.app.project, &name, &sf);
            cx.core.ids.reconcile(&cx.app.project);
            let t = &cx.app.project.tracks[ti];
            return ok(json!({
                "track": state::strip_id(&cx.core.ids, ti),
                "kit": t.name,
                "soundfont": t.soundfont.as_ref().map(|sf| json!({
                    "path": sf.path, "bank": sf.bank, "program": sf.program,
                })),
                "channels": [],
            }));
        }
        return Err(RpcError::failed(format!(
            "{path} has no drum samples to put on the channels"
        )));
    }
    let name = cx
        .app
        .ui
        .kits
        .index
        .kits
        .iter()
        .find(|k| k.path == path)
        .map(|k| k.name.clone())
        .unwrap_or_else(|| {
            std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    let pads = cx
        .app
        .ui
        .kits
        .localize_pads(&path, &kit_browser::pad_paths(&kit));
    let ti = channel_rack::load_kit(
        &mut cx.app.project,
        track,
        &name,
        &pads,
        &kit_browser::pad_classes(&kit),
    );
    cx.core.ids.reconcile(&cx.app.project);
    ok(rack_json(cx, ti))
}

fn assign(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let track = rack_of(cx, params)?;
    let file = params.req_path("file")?;
    if !file.is_file() {
        return Err(RpcError::not_found(format!(
            "there is no file {}",
            file.display()
        )));
    }
    let Some(channel) = params.i64("channel") else {
        return Err(RpcError::invalid(
            "channel",
            "`channel` is needed: the rack's channel, from 1",
        ));
    };
    let ti = match track {
        Some(ti) => ti,
        None => channel_rack::load_kit(&mut cx.app.project, None, "", &[], &[]),
    };
    cx.app.ui.kits.start();
    let file = cx.app.ui.kits.localize(&file);
    if !channel_rack::assign_sample(&mut cx.app.project, ti, channel.max(1) as usize - 1, &file) {
        return Err(RpcError::failed("the sample could not be put there"));
    }
    cx.core.ids.reconcile(&cx.app.project);
    ok(rack_json(cx, ti))
}

/// Peak, RMS and, for each source and where it starts in `out`, how alike the first
/// `window` seconds are (correlation of the left channels, -1 to 1).
fn compare(out: &[f32], rate: u32, sources: &[(std::path::PathBuf, f64)], window: f64) -> Value {
    let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
    let rms = (out
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / out.len().max(1) as f64)
        .sqrt();
    let hits: Vec<Value> = sources
        .iter()
        .map(|(file, at)| {
            let corr = prowave_engine::sample_tools::decode(file, |_| true)
                .ok()
                .map(|buf| {
                    let n = (window * f64::from(rate)) as usize;
                    let start = (at * f64::from(rate)) as usize;
                    let ratio = f64::from(buf.rate.max(1)) / f64::from(rate);
                    let (mut sxy, mut sxx, mut syy) = (0f64, 0f64, 0f64);
                    for k in 0..n {
                        let Some(x) = out.get((start + k) * 2) else {
                            break;
                        };
                        let pos = k as f64 * ratio;
                        let a = pos as usize;
                        if a + 1 >= buf.frames() {
                            break;
                        }
                        let t = (pos - a as f64) as f32;
                        let y = buf.data[a * 2] + (buf.data[(a + 1) * 2] - buf.data[a * 2]) * t;
                        let (x, y) = (f64::from(*x), f64::from(y));
                        sxy += x * y;
                        sxx += x * x;
                        syy += y * y;
                    }
                    if sxx > 0.0 && syy > 0.0 {
                        sxy / (sxx * syy).sqrt()
                    } else {
                        0.0
                    }
                });
            json!({
                "file": file,
                "at_seconds": at,
                "correlation": corr.map(|c| (c * 10_000.0).round() / 10_000.0),
            })
        })
        .collect();
    json!({
        "frames": out.len() / 2,
        "seconds": super::params::round3(out.len() as f64 / 2.0 / f64::from(rate.max(1))),
        "peak": (f64::from(peak) * 10_000.0).round() / 10_000.0,
        "rms": (rms * 10_000.0).round() / 10_000.0,
        "hits": hits,
    })
}

fn audition(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let capture = match params.str("capture") {
        Some(c) => {
            Some(super::params::expand_path(c).map_err(|e| RpcError::invalid("capture", e))?)
        }
        None => None,
    };
    // What is played, and where each sound starts in it.
    let sources: Vec<(std::path::PathBuf, f64)> = if params.has("file") {
        let file = params.req_path("file")?;
        if !file.is_file() {
            return Err(RpcError::not_found(format!(
                "there is no file {}",
                file.display()
            )));
        }
        let v = &mut cx.app.ui.kits;
        v.start();
        v.play_file(&file);
        vec![(file, 0.0)]
    } else if params.has("path") {
        let (_, kit) = read(cx, params)?;
        let v = &mut cx.app.ui.kits;
        v.audition_kit();
        kit.audition_samples()
            .into_iter()
            .enumerate()
            .map(|(k, i)| (kit.path_of(i), k as f64 * kit_browser::KIT_STEP_SECONDS))
            .collect()
    } else {
        return Err(RpcError::invalid(
            "file",
            "`file` (one sample) or `path` (a kit) is needed",
        ));
    };
    if sources.is_empty() {
        return Err(RpcError::failed("there is nothing in it to play"));
    }
    let Some(capture) = capture else {
        // Played on the engine's audition voice by the next frames.
        let files: Vec<&std::path::Path> = sources.iter().map(|s| s.0.as_path()).collect();
        return ok(json!({ "playing": files }));
    };
    let rate = prowave_engine::sample_tools::decode(&sources[0].0, |_| true)
        .map(|b| b.rate)
        .unwrap_or(44_100);
    let out = kit_browser::capture(
        &mut cx.app.ui.kits,
        rate,
        20.0,
        std::time::Duration::from_secs(30),
    );
    if let Some(e) = cx.app.ui.kits.error.take() {
        return Err(RpcError::failed(e));
    }
    prowave_engine::write_wav(&capture, &out, rate)
        .map_err(|e| RpcError::failed(format!("{}: {e}", capture.display())))?;
    let mut reply = compare(&out, rate, &sources, 0.08);
    reply["capture"] = json!(capture);
    reply["rate"] = json!(rate);
    ok(reply)
}

fn copy(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let v = &mut cx.app.ui.kits;
    v.start();
    if let Some(m) = params.str("mode") {
        let mode = crate::kit_copy::CopyMode::parse(m)
            .ok_or_else(|| RpcError::invalid("mode", "remote or always"))?;
        v.set_copy_mode(mode);
    }
    ok(json!({
        "mode": v.settings.copy_kits.id(),
        "local": v.local_dir().map(|d| d.to_string_lossy().into_owned()),
    }))
}

fn list(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    ready(cx);
    let v = &mut cx.app.ui.kits;
    if params.has("path") {
        let text = params.str("path").unwrap_or("").trim();
        let path = if text.is_empty() || !(text.starts_with('~') || text.starts_with('/')) {
            text.to_string()
        } else {
            super::params::expand_path(text)
                .map_err(|e| RpcError::invalid("path", e))?
                .to_string_lossy()
                .into_owned()
        };
        v.set_kit_list(&path);
        v.read_list_now();
    }
    if let Some(on) = params.bool("on") {
        v.set_list_on(on);
    }
    ok(list_json(v))
}

fn favorite(cx: &mut Cx, params: &P) -> Result<Reply, RpcError> {
    let path = params.req_path("path")?.to_string_lossy().into_owned();
    let on = params.bool("on").unwrap_or(true);
    let v = &mut cx.app.ui.kits;
    v.start();
    v.set_favorite(&path, on);
    ok(json!({ "path": path, "favorite": v.is_favorite(&path) }))
}

pub fn methods() -> Vec<Method> {
    let track = || {
        p(
            "track",
            Ty::Any,
            "the rack's track: its id (\"t3\"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack, else a new Channel Rack track",
        )
    };
    let list_result = "{path, on, active (set, read and on), file (the file read), rows, matched (kits of the list found), types: [{type, kits}], error}";
    let status_result = "{folders: [{path, present, kits}], kits (with the kit list active: the kits it shows), all_kits, kit_list: {path, on, active, file, rows, matched, types, error}, listed, pools, sources: [{name, kits}], scanning, listing: {done, total} or null, indexing, scan_ms, last_listing: {read, ms}, favorites, audition, default_kit, copy_kits (remote, always), copying (a kit load waits for its samples to be copied here), local}";
    let rack_result = "{track, kit, channels: [{channel, name, path, class}]}";
    vec![
        m("kits.status", "The drum-kit library: its folders (present: the folder is there now), the number of kits (listed: those whose files are read and kept; pools: sources of one-sample folders shown as one) and the sources. The first call reads the folders.",
            vec![], status_result, status),
        m("kits.rescan", "Reads the library folders again and waits for it (scan_ms is how long it took); then every kit's files are read again in the background (listing in kits.status while they are).",
            vec![p("full", Ty::Bool, "false: read again only the kits never read and those whose folder has changed (default true)")],
            status_result, rescan),
        m("kits.folders", "Sets the library folders (saved), then scans them. A folder that is not there is kept and shows present: false.",
            vec![p("folders", Ty::Arr, "the folders, a list of paths").req()],
            status_result, folders),
        m("kits.search", "The kits whose source or name holds every word of the query, as the Kit Browser lists them: with the kit list active, only its kits (and the favourites and the kit of new racks) in its order, grouped by type.",
            vec![
                p("query", Ty::Str, "words to look for (left out: every kit)"),
                p("source", Ty::Str, "only kits of this source"),
                p("best", Ty::Bool, "only kits the library's BEST.md ranks, in its order"),
                p("favorites", Ty::Bool, "only the favourites"),
                p("list", Ty::Bool, "false: every kit, as if the kit list were off (default: the kit list when it is active)"),
                p("type", Ty::Str, "with the kit list: only kits of this type of it"),
                p("limit", Ty::Int, "most kits to return (default 50)").range(1.0, 1000.0),
                p("offset", Ty::Int, "kits to skip (default 0)").range(0.0, 10_000_000.0),
            ],
            "{total, offset, kits: [{path, source, name, files, license, best, favorite, channels, pool, rank, type}]}", search),
        m("kits.get", "A kit's files (from its kept listing, else read from its folder): its audio and SoundFont files with the class of each (kick, snare, clap, closed_hat, open_hat, rim, tom_low, tom_mid, tom_high, perc, shaker, crash, ride, 808, fx, loop, other, sf2), the channels a load fills and the samples its audition plays.",
            vec![
                p("path", Ty::Str, "the kit's folder (a pool's: its source's folder)").req(),
                p("offset", Ty::Int, "samples to skip (default 0)").range(0.0, 100_000_000.0),
                p("limit", Ty::Int, "most samples to list (default 200)").range(0.0, 100_000_000.0),
            ],
            "{path, kind (files, sfz, hydrogen, drumgizmo), files, classes: [{class, files}], offset, samples: [{index, file, name, class}], channels: [{channel, file, class}], audition: [file]}", get),
        m("kits.load", "Loads a kit into a Channel Rack: its samples go on the channels kits.get lists (copied to this computer first as kits.copy says); levels, pans, mutes and steps stay. A kit of SoundFonts alone goes on a new instrument track as its SoundFont (its drum preset when it has one).",
            vec![p("path", Ty::Str, "the kit's folder").req(), track()],
            rack_result, load).changes(),
        m("kits.assign", "Puts one audio file on one channel of a Channel Rack (empty channels are added up to it).",
            vec![
                p("file", Ty::Str, "the audio file").req(),
                p("channel", Ty::Int, "the channel, from 1").req().range(1.0, 64.0),
                track(),
            ],
            rack_result, assign).changes(),
        m("kits.audition", "Plays one sample, or a kit's kick, snare and hat, on the audition voice as the Kit Browser does. With capture, nothing is played aloud: the audition voice's output is written to that WAV file instead and compared with the files played.",
            vec![
                p("file", Ty::Str, "one audio file"),
                p("path", Ty::Str, "a kit's folder (its kick, snare and hat)"),
                p("capture", Ty::Str, "a WAV file to write the audition into instead of playing it"),
            ],
            "{playing: [file]}, or with capture {capture, rate, frames, seconds, peak, rms, hits: [{file, at_seconds, correlation}]}", audition),
        m("kits.copy", "When a kit or sample loaded into a rack is copied to this computer (kit-library/local/<source>/<kit>/, the rack then plays the copies): remote copies only from folders on a network share (the default), always from every folder. Left out: tells the setting.",
            vec![p("mode", Ty::Str, "remote or always (saved)").vals(&["remote", "always"])],
            "{mode, local}", copy),
        m("kits.list", "The kit list: a TSV (header with path, and rank, type, reason) naming kits by their folder relative to the library folder. Set, the Kit Browser, kits.search and kits.status show only its kits (and the favourites and the kit of new racks), in rank order, grouped by type. A list path not found as written is looked for in the library folders (the same file on another Mac's mount). Left out: tells the setting.",
            vec![
                p("path", Ty::Str, "the TSV (saved; turns the list on); \"\" clears it"),
                p("on", Ty::Bool, "false: the list is kept but not used (saved), as the Kit Browser's Kit List switch"),
            ],
            list_result, list),
        m("kits.favorite", "Marks a kit as a favourite, or takes the mark off.",
            vec![
                p("path", Ty::Str, "the kit's folder").req(),
                p("on", Ty::Bool, "false takes the mark off (default true)"),
            ],
            "{path, favorite}", favorite),
    ]
}
