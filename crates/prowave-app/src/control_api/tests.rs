//! Tests of the control interface: the wire format, the method list and its documents, every
//! method family against an app that has no window, and the socket with real connections.
#![allow(clippy::unwrap_used)]
use super::protocol::{self, Code, RpcError};
use super::registry::{self, Reply};
use super::{dispatch, Control, Core};
use crate::file_commands::FileCmd;
use crate::App;
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// An app without a window and the control state, called as the socket would call them.
struct Rig {
    app: App,
    ctx: egui::Context,
    core: Core,
}

impl Rig {
    fn new() -> Self {
        Self {
            app: super::headless::app_without_window(None),
            ctx: egui::Context::default(),
            core: Core {
                ids: super::ids::Ids::default(),
                shared: std::sync::Arc::new(super::server::Shared::new()),
                socket: PathBuf::from("/nowhere/control.sock"),
                times: super::Times::default(),
                watch: super::events::Watch::default(),
                in_batch: false,
                headless: true,
                shots: Vec::new(),
                quit: false,
                started: Instant::now(),
                waits: Vec::new(),
                bounces: Vec::new(),
                bounce_seen: super::render::Seen::default(),
            },
        }
    }

    fn try_call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let map = match params {
            Value::Object(m) => m,
            Value::Null => Map::new(),
            other => panic!("params must be an object: {other}"),
        };
        match dispatch(
            &mut self.app,
            &self.ctx,
            &mut self.core,
            1,
            Some(json!(1)),
            method,
            &map,
        )? {
            Reply::Now(v) => Ok(v),
            Reply::Later => self.later(),
        }
    }

    /// The answer of a call that is answered later. A bounce: the app's frames are run until
    /// it ends, as the window runs them, and what the caller would be sent is given.
    fn later(&mut self) -> Result<Value, RpcError> {
        let Some(pending) = self.core.bounces.pop() else {
            return Ok(Value::Null);
        };
        let began = Instant::now();
        loop {
            crate::bounce_ui::tend(&mut self.app, &self.ctx);
            if let Some(answer) = super::render::answer(&self.app, &self.core, &pending) {
                return answer;
            }
            assert!(
                began.elapsed() < Duration::from_secs(300),
                "the bounce never ended"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        match self.try_call(method, params.clone()) {
            Ok(v) => v,
            Err(e) => panic!(
                "{method} {params} failed: {} ({})",
                e.message,
                e.code.name()
            ),
        }
    }

    fn err(&mut self, method: &str, params: Value) -> RpcError {
        match self.try_call(method, params.clone()) {
            Ok(v) => panic!("{method} {params} should fail, gave {v}"),
            Err(e) => e,
        }
    }

    fn state(&mut self) -> Value {
        self.call("state.get", json!({}))
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

// ------------------------------------------------------------------ the wire format

#[test]
fn a_request_line_is_parsed_and_its_id_kept() {
    let r =
        protocol::parse(r#"{"jsonrpc":"2.0","id":7,"method":"transport.play","params":{"bar":5}}"#)
            .unwrap();
    assert_eq!(r.id, Some(json!(7)));
    assert_eq!(r.method, "transport.play");
    assert_eq!(r.params.get("bar"), Some(&json!(5)));
    // A string id, no params; a notification has no id.
    let r = protocol::parse(r#"{"jsonrpc":"2.0","id":"a","method":"rpc.ping"}"#).unwrap();
    assert_eq!((r.id, r.params.len()), (Some(json!("a")), 0));
    let r = protocol::parse(r#"{"jsonrpc":"2.0","method":"rpc.ping","params":null}"#).unwrap();
    assert_eq!(r.id, None);
}

#[test]
fn bad_lines_get_the_right_error_and_the_id_when_it_could_be_read() {
    let code = |line: &str| {
        let (id, e) = protocol::parse(line).unwrap_err();
        (id, e.code)
    };
    assert_eq!(code("{not json"), (Value::Null, Code::ParseError));
    assert_eq!(code("[1,2]"), (Value::Null, Code::InvalidRequest));
    assert_eq!(
        code(r#"{"id":3,"method":"x"}"#),
        (json!(3), Code::InvalidRequest)
    );
    assert_eq!(
        code(r#"{"jsonrpc":"2.0","id":4}"#),
        (json!(4), Code::InvalidRequest)
    );
    assert_eq!(
        code(r#"{"jsonrpc":"2.0","id":{"a":1},"method":"x"}"#),
        (Value::Null, Code::InvalidRequest)
    );
    assert_eq!(
        code(r#"{"jsonrpc":"2.0","id":5,"method":"x","params":[1]}"#),
        (json!(5), Code::InvalidParams)
    );
}

#[test]
fn a_response_and_an_error_have_the_json_rpc_shape() {
    let ok: Value =
        serde_json::from_str(&protocol::response(&json!(9), &Ok(json!({"a": 1})))).unwrap();
    assert_eq!(ok, json!({"jsonrpc": "2.0", "id": 9, "result": {"a": 1}}));
    let e = RpcError::invalid("bar", "bar must be 1 or more").with("min", 1);
    let err: Value = serde_json::from_str(&protocol::response(&json!("x"), &Err(e))).unwrap();
    assert_eq!(err["id"], json!("x"));
    assert_eq!(err["error"]["code"], json!(-32602));
    assert_eq!(err["error"]["message"], json!("bar must be 1 or more"));
    assert_eq!(
        err["error"]["data"],
        json!({"code": "invalid_params", "param": "bar", "min": 1})
    );
    let ev: Value = serde_json::from_str(&protocol::event(
        "transport.started",
        json!({"bar": 2}).as_object().unwrap().clone(),
    ))
    .unwrap();
    assert_eq!(
        ev,
        json!({"jsonrpc": "2.0", "method": "event", "params": {"event": "transport.started", "bar": 2}})
    );
    // Every code has its own number and name.
    let mut numbers: Vec<i64> = Code::ALL.iter().map(|c| c.number()).collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), Code::ALL.len());
}

// ------------------------------------------------------------------ the method list

#[test]
fn every_method_is_described_once_and_completely() {
    let all = registry::all();
    assert!(all.len() >= 90, "{} methods", all.len());
    let mut names: Vec<&str> = all.iter().map(|m| m.name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(names.len(), before, "a method name is used twice");
    for m in all {
        assert!(
            m.name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
            "{}",
            m.name
        );
        assert!(
            m.summary.ends_with('.'),
            "{}: summary {:?}",
            m.name,
            m.summary
        );
        assert!(!m.result.is_empty(), "{}: no result", m.name);
        let mut params: Vec<&str> = m.params.iter().map(|p| p.name).collect();
        params.sort_unstable();
        let n = params.len();
        params.dedup();
        assert_eq!(params.len(), n, "{}: a parameter is listed twice", m.name);
        for p in &m.params {
            assert!(!p.desc.is_empty(), "{}.{}", m.name, p.name);
            if !p.values.is_empty() {
                assert_eq!(p.ty, registry::Ty::Str, "{}.{}", m.name, p.name);
            }
        }
    }
    let d = registry::describe(None, Some(Path::new("/x/control.sock")));
    assert_eq!(d["protocol"], json!(protocol::PROTOCOL));
    assert_eq!(d["methods"].as_array().unwrap().len(), all.len());
    assert_eq!(d["errors"].as_array().unwrap().len(), Code::ALL.len());
    let one = registry::describe(Some("transport.locate"), None);
    let m = &one["methods"][0];
    assert_eq!(m["group"], json!("transport"));
    let bar = m["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "bar")
        .unwrap();
    assert_eq!(
        (&bar["type"], &bar["required"], &bar["unit"], &bar["min"]),
        (&json!("number"), &json!(false), &json!("bars"), &json!(1.0))
    );
}

#[test]
fn parameters_are_checked_before_a_method_runs() {
    let mut r = Rig::new();
    let e = r.err("transport.nope", json!({}));
    assert_eq!(e.code, Code::MethodNotFound);
    assert!(e.message.contains("transport.play"), "{}", e.message);
    let e = r.err("transport.locate", json!({"bars": 5}));
    assert_eq!(e.code, Code::InvalidParams);
    assert!(
        e.message.contains("no parameter `bars`") && e.message.contains("bar,"),
        "{}",
        e.message
    );
    let e = r.err("track.rename", json!({"track": 1}));
    assert!(
        e.message.contains("needs the parameter `name`"),
        "{}",
        e.message
    );
    let e = r.err("transport.locate", json!({"bar": "five"}));
    assert!(e.message.contains("must be a number"), "{}", e.message);
    let e = r.err("transport.tempo", json!({"bpm": 2000}));
    assert!(e.message.contains("from 5 to 990 bpm"), "{}", e.message);
    assert_eq!(e.data.get("max"), Some(&json!(990.0)));
    let e = r.err("track.add", json!({"kind": "tuba"}));
    assert!(e.message.contains("instrument, audio"), "{}", e.message);
    let e = r.err("transport.locate", json!({}));
    assert!(e.message.contains("a place is needed"), "{}", e.message);
    let e = r.err("transport.locate", json!({"bar": 2, "seconds": 1}));
    assert!(e.message.contains("not several"), "{}", e.message);
    let e = r.err("transport.locate", json!({"position": "5.9"}));
    assert!(e.message.contains("beats go from 1 to 4"), "{}", e.message);
    let e = r.err("rpc.describe", json!({"method": "nope"}));
    assert_eq!(e.code, Code::NotFound);
}

/// Where the repository is, from the app crate.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const DOC_START: &str = "<!-- methods:start (made from crates/prowave-app/src/control_api/registry.rs; do not edit by hand) -->";
const DOC_END: &str = "<!-- methods:end -->";

/// The method table in `docs/CONTROL-API.md` and the list `prowavectl` carries are made from
/// the registry. `PROWAVE_CONTROL_REGEN=1 cargo test -p prowave-app control_api` writes them
/// again; without it this fails when either is not what the registry gives.
#[test]
fn the_reference_document_and_the_clients_list_match_the_registry() {
    let regen = std::env::var_os("PROWAVE_CONTROL_REGEN").is_some();
    let doc_path = repo().join("docs/CONTROL-API.md");
    let doc = std::fs::read_to_string(&doc_path).unwrap();
    let (a, b) = (doc.find(DOC_START).unwrap(), doc.find(DOC_END).unwrap());
    let want = format!(
        "{}{DOC_START}\n{}{}",
        &doc[..a],
        registry::markdown(),
        &doc[b..]
    );
    let list_path = repo().join("crates/prowavectl/methods.json");
    let list = registry::embedded_list();
    if regen {
        std::fs::write(&doc_path, &want).unwrap();
        std::fs::write(&list_path, &list).unwrap();
        return;
    }
    assert!(
        doc == want,
        "docs/CONTROL-API.md's method table is stale: run the tests once with PROWAVE_CONTROL_REGEN=1"
    );
    assert!(
        std::fs::read_to_string(&list_path).unwrap() == list,
        "crates/prowavectl/methods.json is stale: run the tests once with PROWAVE_CONTROL_REGEN=1"
    );
    for name in super::events::NAMES {
        assert!(
            doc.contains(&format!("`{name}`")),
            "the document does not name the event {name}"
        );
    }
    for code in Code::ALL {
        assert!(
            doc.contains(&format!("`{}`", code.name())),
            "the document does not name the error {}",
            code.name()
        );
    }
}

// ------------------------------------------------------------------ transport and project

#[test]
fn the_transport_is_driven_in_bars_positions_and_seconds() {
    let mut r = Rig::new();
    let t = r.call("transport.tempo", json!({"bpm": 100}));
    assert_eq!(f(&t["bpm"]), 100.0);
    assert_eq!(r.app.project.tempo, 100.0);
    // Bar 5 of 4/4 is 16 quarter notes in; at 100 bpm that is 9.6 seconds.
    let t = r.call("transport.locate", json!({"bar": 5}));
    assert_eq!(r.app.project.position_beats, 16.0);
    assert_eq!(s(&t["playhead"]["position"]), "5.1.1.1");
    assert!((f(&t["playhead"]["seconds"]) - 9.6).abs() < 1e-6);
    r.call("transport.locate", json!({"position": "2.3"}));
    assert_eq!(r.app.project.position_beats, 6.0);
    r.call("transport.locate", json!({"position": "2.3.3.121"}));
    assert!((r.app.project.position_beats - 6.625).abs() < 1e-9);
    r.call("transport.locate", json!({"seconds": 6}));
    assert!((r.app.project.position_beats - 10.0).abs() < 1e-9);
    r.call("transport.locate", json!({"beat": 3.5}));
    assert_eq!(r.app.project.position_beats, 3.5);

    let t = r.call(
        "transport.cycle",
        json!({"on": true, "bar": 1, "end_bar": 9}),
    );
    assert_eq!(
        (
            r.app.project.cycle,
            r.app.project.cycle_start,
            r.app.project.cycle_end
        ),
        (true, 0.0, 32.0)
    );
    assert_eq!(f(&t["cycle"]["end"]["bar"]), 9.0);
    let e = r.err("transport.cycle", json!({"bar": 9, "end_bar": 3}));
    assert!(e.message.contains("end after it starts"), "{}", e.message);

    let t = r.call("transport.play", json!({"bar": 3}));
    assert_eq!(
        (t["playing"].as_bool(), r.app.project.playing),
        (Some(true), true)
    );
    assert_eq!(r.app.project.position_beats, 8.0);
    r.call("transport.record", json!({}));
    assert!(r.app.project.recording);
    r.call("transport.stop", json!({}));
    assert!(!r.app.project.playing && !r.app.project.recording);

    r.call(
        "transport.signature",
        json!({"numerator": 6, "denominator": 8}),
    );
    assert_eq!(r.app.project.sig, (6, 8));
    let sg = r.call(
        "transport.signature",
        json!({"numerator": 3, "denominator": 4, "from_bar": 5}),
    );
    assert_eq!(
        sg["signatures"][1],
        json!({"bar": 5, "numerator": 3, "denominator": 4})
    );
    r.call("signature.remove", json!({"bar": 5}));
    assert!(r.app.project.sig_changes.is_empty());
    assert_eq!(
        r.err(
            "transport.signature",
            json!({"numerator": 4, "denominator": 3})
        )
        .code,
        Code::InvalidParams
    );

    let mt = r.call("transport.metronome", json!({"on": true, "volume_db": -20}));
    assert_eq!(
        (mt["on"].as_bool(), f(&mt["volume_db"])),
        (Some(true), -20.0)
    );
    let ci = r.call("transport.count_in", json!({"on": true, "bars": 2}));
    assert_eq!(ci, json!({"on": true, "bars": 2}));
    assert!(r.app.project.metronome && r.app.project.count_in);
}

#[test]
fn a_change_is_one_undo_step_sets_the_dirty_flag_and_is_saved_and_opened() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Rig::new();
    assert_eq!(r.call("project.get", json!({}))["dirty"], json!(false));
    r.call("transport.tempo", json!({"bpm": 87}));
    let info = r.call("project.get", json!({}));
    assert_eq!(
        (&info["dirty"], &info["can_undo"]),
        (&json!(true), &json!(true))
    );
    // Undo takes exactly that change back, and Redo does it again.
    let u = r.call("project.undo", json!({}));
    assert_eq!((&u["done"], &u["can_redo"]), (&json!(true), &json!(true)));
    assert_eq!(r.app.project.tempo, 120.0);
    assert_eq!(r.call("project.get", json!({}))["dirty"], json!(false));
    r.call("project.redo", json!({}));
    assert_eq!(r.app.project.tempo, 87.0);
    assert_eq!(r.call("project.undo", json!({}))["done"], json!(true));
    assert_eq!(r.call("project.undo", json!({}))["done"], json!(false));
    r.call("project.redo", json!({}));

    // A project that was never saved has no file to save to.
    assert!(r
        .err("project.save", json!({}))
        .message
        .contains("project.save_as"));
    let file = dir.path().join("Song.prowave");
    let saved = r.call("project.save_as", json!({"path": file}));
    assert_eq!(
        (s(&saved["name"]), &saved["dirty"]),
        ("Song", &json!(false))
    );
    assert!(file.is_file());
    let busy = dir.path().join("Busy.prowave");
    std::fs::write(&busy, b"x").unwrap();
    assert!(r
        .err("project.save_as", json!({"path": busy}))
        .message
        .contains("already there"));
    assert!(r
        .err("project.save_as", json!({"path": "Song.prowave"}))
        .message
        .contains("not an absolute path"));

    // Opening another project is refused while there are unsaved changes, unless told.
    r.call("track.add", json!({"kind": "audio", "name": "Vox"}));
    let other = dir.path().join("Other.prowave");
    r.call("project.save_copy", json!({"path": other}));
    let e = r.err("project.open", json!({"path": other}));
    assert_eq!(e.code, Code::UnsavedChanges);
    assert_eq!(r.app.project.name, "Song");
    let e = r.err(
        "project.open",
        json!({"path": dir.path().join("nope.prowave")}),
    );
    assert_eq!(e.code, Code::NotFound);
    let opened = r.call(
        "project.open",
        json!({"path": other, "if_unsaved": "discard"}),
    );
    assert_eq!(
        (s(&opened["name"]), &opened["dirty"]),
        ("Other", &json!(false))
    );
    assert_eq!(r.app.project.tracks.len(), 2);
    assert_eq!(r.app.project.tempo, 87.0);
    // The first file was saved before the track was added.
    r.call("project.open", json!({"path": file}));
    assert_eq!(r.app.project.tracks.len(), 1);
    r.call("transport.tempo", json!({"bpm": 140}));
    let n = r.call("project.new", json!({"if_unsaved": "save"}));
    assert_eq!((s(&n["name"]), &n["path"]), ("Untitled", &Value::Null));
    r.call("project.open", json!({"path": file}));
    assert_eq!(r.app.project.tempo, 140.0, "if_unsaved: save saved it");
    r.call("transport.tempo", json!({"bpm": 150}));
    assert!(r
        .err("project.revert", json!({"discard_changes": false}))
        .message
        .contains("discard_changes: true"));
    r.call("project.revert", json!({"discard_changes": true}));
    assert_eq!(r.app.project.tempo, 140.0);
}

#[test]
fn a_batch_is_one_undo_step_and_stops_at_the_first_failure() {
    let mut r = Rig::new();
    let out = r.call(
        "batch",
        json!({"calls": [
            {"method": "transport.tempo", "params": {"bpm": 90}},
            {"method": "track.add", "params": {"kind": "audio"}},
            {"method": "track.rename", "params": {"track": 2, "name": "Guitar"}},
            {"method": "transport.get"},
        ]}),
    );
    assert_eq!(out["count"], json!(4));
    assert_eq!(s(&out["results"][2]["name"]), "Guitar");
    assert_eq!((r.app.project.tempo, r.app.project.tracks.len()), (90.0, 2));
    // One Undo takes all of it back.
    r.call("project.undo", json!({}));
    assert_eq!(
        (r.app.project.tempo, r.app.project.tracks.len()),
        (120.0, 1)
    );
    assert_eq!(r.call("project.undo", json!({}))["done"], json!(false));

    let calls = json!([
        {"method": "transport.tempo", "params": {"bpm": 95}},
        {"method": "track.rename", "params": {"track": 9, "name": "x"}},
        {"method": "transport.tempo", "params": {"bpm": 99}},
    ]);
    let e = r.err("batch", json!({"calls": calls}));
    assert_eq!(e.code, Code::NotFound);
    assert_eq!(e.data.get("failed_index"), Some(&json!(1)));
    assert_eq!(e.data.get("failed_method"), Some(&json!("track.rename")));
    assert!(
        e.message.starts_with("call 2 of 3 (`track.rename`) failed"),
        "{}",
        e.message
    );
    assert_eq!(
        r.app.project.tempo, 95.0,
        "the calls before the failure stay"
    );
    r.call("project.undo", json!({}));
    assert_eq!(r.app.project.tempo, 120.0);
    let e = r.err("batch", json!({"calls": calls, "rollback": true}));
    assert_eq!(e.data.get("rolled_back"), Some(&json!(true)));
    assert_eq!(r.app.project.tempo, 120.0);
    assert_eq!(r.call("project.get", json!({}))["dirty"], json!(false));
    let e = r.err(
        "batch",
        json!({"calls": [{"method": "batch", "params": {"calls": []}}]}),
    );
    assert_eq!(e.code, Code::NotSupported);
    let e = r.err("batch", json!({"calls": [5]}));
    assert_eq!(e.code, Code::InvalidParams);
}

// ------------------------------------------------------------------ tracks, effects, parameters

#[test]
fn tracks_are_added_set_moved_and_removed_and_keep_their_ids() {
    let mut r = Rig::new();
    let first = s(&r.call("track.list", json!({}))["tracks"][0]["id"]).to_string();
    let a = r.call("track.add", json!({"kind": "audio", "name": "Vox"}));
    let b = r.call("track.add", json!({}));
    let (a_id, b_id) = (s(&a["id"]).to_string(), s(&b["id"]).to_string());
    assert_eq!(
        (s(&a["kind"]), s(&a["name"]), s(&b["kind"])),
        ("audio", "Vox", "instrument")
    );
    assert_eq!(b["number"], json!(3));

    let t = r.call(
        "track.set",
        json!({"track": "Vox", "mute": true, "solo": true, "arm": true, "monitor": true,
               "volume_db": -6.5, "pan": -0.25, "input": "stereo:3", "output": "bus:2"}),
    );
    assert_eq!(
        (
            &t["mute"],
            &t["solo"],
            &t["arm"],
            &t["monitor"],
            f(&t["volume_db"]),
            f(&t["pan"])
        ),
        (
            &json!(true),
            &json!(true),
            &json!(true),
            &json!(true),
            -6.5,
            -0.25
        )
    );
    assert_eq!((s(&t["input"]), s(&t["output"])), ("stereo:3", "bus:2"));
    let vox = r
        .app
        .project
        .tracks
        .iter()
        .find(|t| t.name == "Vox")
        .unwrap();
    assert!(vox.mute && vox.solo && vox.rec && vox.monitor);
    assert_eq!((vox.volume_db, vox.pan, vox.output), (-6.5, -0.25, 2));
    assert!(
        r.app
            .project
            .tracks
            .iter()
            .any(|t| t.kind == crate::model::TrackKind::Aux),
        "the bus got its Aux strip"
    );
    assert_eq!(
        r.err("track.set", json!({"track": b_id, "monitor": true}))
            .code,
        Code::NotSupported
    );
    assert!(r
        .err("track.set", json!({"track": a_id, "input": "left"}))
        .message
        .contains("mono:N"));
    assert!(r
        .err("track.set", json!({"track": "Nobody", "mute": true}))
        .message
        .contains("the tracks are"));
    assert_eq!(
        r.err("track.set", json!({"track": 99, "mute": true})).code,
        Code::NotFound
    );

    let send = r.call(
        "track.send",
        json!({"track": a_id, "bus": 3, "level": 0.8, "pre_fader": true}),
    );
    assert_eq!(
        send["sends"][0],
        json!({"slot": 1, "bus": 3, "level": 0.8, "pre_fader": true})
    );
    r.call(
        "track.send",
        json!({"track": a_id, "bus": 3, "remove": true}),
    );
    let vox = r
        .app
        .project
        .tracks
        .iter()
        .find(|t| t.name == "Vox")
        .unwrap();
    assert!(vox.sends.is_empty());

    // The first track goes to the end; every track keeps its id.
    let moved = r.call("track.move", json!({"track": first, "to": 1024}));
    let n = r.app.project.tracks.len();
    assert_eq!(moved["number"], json!(n));
    assert_eq!(r.app.project.tracks[n - 1].name, "Inst 1");
    assert_eq!(
        s(&r.call("track.get", json!({"track": a_id}))["name"]),
        "Vox"
    );
    assert_eq!(
        s(&r.call("track.get", json!({"track": b_id}))["name"]),
        "Inst 2"
    );
    r.call("track.rename", json!({"track": b_id, "name": "Keys"}));
    assert_eq!(
        s(&r.call("track.get", json!({"track": b_id}))["name"]),
        "Keys"
    );

    let gone = r.call("track.remove", json!({"track": a_id}));
    assert_eq!(s(&gone["name"]), "Vox");
    assert_eq!(
        r.err("track.get", json!({"track": a_id})).code,
        Code::NotFound
    );
    assert_eq!(
        s(&r.call("track.get", json!({"track": first}))["name"]),
        "Inst 1"
    );
    // Undo brings the track back.
    r.call("project.undo", json!({}));
    assert!(r.app.project.tracks.iter().any(|t| t.name == "Vox"));
}

/// The native instruments are listed, go into a track by id or by name, and their parameters
/// and windows are reached as a native effect's are.
#[test]
fn native_instruments_are_listed_set_and_their_parameters_moved() {
    let mut r = Rig::new();
    let list = r.call("plugin.list", json!({"kind": "instrument"}));
    let ids: Vec<&str> = list["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| s(&p["id"]))
        .collect();
    for info in prowave_engine::native_instruments() {
        assert!(ids.contains(&info.id), "{} is not in {ids:?}", info.id);
        let set = r.call("instrument.set", json!({"track": 1, "id": info.id}));
        assert_eq!(s(&set["instrument"]["id"]), info.id);
        let by_name = r.call("instrument.set", json!({"track": 1, "id": info.name}));
        assert_eq!(s(&by_name["instrument"]["id"]), info.id);
        let np = r.call("plugin.params", json!({"track": 1, "instrument": true}));
        let listed = np["devices"][0]["params"].as_array().unwrap();
        assert_eq!(listed.len(), info.params.len(), "{}", info.id);
        // The last parameter to the bottom of its range: kept in the slot, and the running
        // instrument is told.
        let last = listed.last().unwrap();
        let target = f(&last["min"]);
        r.app.ui.native_edits.clear();
        r.call(
            "param.set",
            json!({"track": 1, "param": last["param"], "value": target}),
        );
        let slot = r.app.project.tracks[0].au_instrument.as_ref().unwrap();
        let kept = crate::native_fx_ui::values(slot, info);
        assert_eq!(
            f64::from(kept[info.params.len() - 1]),
            target,
            "{}",
            info.id
        );
        assert_eq!(r.app.ui.native_edits.len(), 1);
        assert_eq!(
            f(&r.call("param.get", json!({"track": 1, "param": last["param"]}))["value"]),
            target
        );
    }
}

#[test]
fn instruments_and_effects_are_listed_loaded_ordered_bypassed_and_removed() {
    let mut r = Rig::new();
    let list = r.call("plugin.list", json!({"kind": "instrument"}));
    let ids: Vec<&str> = list["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| s(&p["id"]))
        .collect();
    assert!(
        ids.contains(&"prowave:rack") && ids.contains(&"prowave:808"),
        "{ids:?}"
    );
    let fx = r.call(
        "plugin.list",
        json!({"kind": "effect", "search": "compress"}),
    );
    assert_eq!(s(&fx["plugins"][0]["id"]), "builtin:Compressor");

    let set = r.call("instrument.set", json!({"track": 1, "id": "Channel Rack"}));
    assert_eq!(s(&set["instrument"]["id"]), "prowave:rack");
    assert!(r.app.project.tracks[0].au_instrument.is_some());
    assert!(
        r.app.ui.mixer.open_plugin.is_none(),
        "no window unless asked for"
    );
    let t = r.call(
        "track.add",
        json!({"instrument": "prowave:808", "name": "Sub"}),
    );
    assert_eq!(s(&t["instrument"]["id"]), "prowave:808");
    assert!(r
        .err(
            "instrument.set",
            json!({"track": 1, "id": "Nonesuch Synth"})
        )
        .message
        .contains("plugin.list"));
    r.call("track.add", json!({"kind": "audio"}));
    assert_eq!(
        r.err("instrument.set", json!({"track": 3, "id": "808"}))
            .code,
        Code::NotSupported
    );
    assert_eq!(
        r.call("instrument.remove", json!({"track": 1}))["removed"],
        json!(true)
    );

    // Effects: two built-in ones and a native one on a track, one on the Stereo Out.
    let c = r.call(
        "effect.add",
        json!({"track": 1, "id": "builtin:Compressor"}),
    );
    assert_eq!(s(&c["effect"]["ref"]), "insert:1");
    assert!(
        r.app.ui.mixer.fx_window.is_none(),
        "no window unless asked for"
    );
    r.call("effect.add", json!({"track": 1, "id": "Channel EQ"}));
    let native = &prowave_engine::native_effects()[0];
    let nat = r.call("effect.add", json!({"track": 1, "id": native.id}));
    let nat_ref = s(&nat["effect"]["ref"]).to_string();
    assert!(nat_ref.starts_with("plugin:"), "{nat_ref}");
    assert_eq!(nat["effects"].as_array().unwrap().len(), 3);
    r.call("effect.add", json!({"track": "master", "id": "Chorus"}));
    assert_eq!(r.app.project.master_inserts.len(), 1);

    let by = r.call("effect.bypass", json!({"track": 1, "effect": "Compressor"}));
    assert_eq!(by["effect"]["bypass"], json!(true));
    assert!(r.app.project.tracks[0].inserts[0].bypass);
    r.call(
        "effect.bypass",
        json!({"track": 1, "effect": "insert:1", "on": false}),
    );
    assert!(!r.app.project.tracks[0].inserts[0].bypass);
    r.call(
        "effect.bypass",
        json!({"track": 1, "effect": nat_ref, "on": true}),
    );
    assert!(r.app.project.tracks[0].au_effects[0].bypass);

    let mv = r.call(
        "effect.move",
        json!({"track": 1, "effect": "Channel EQ", "by": -1}),
    );
    assert_eq!(s(&mv["effect"]["ref"]), "insert:1");
    assert_eq!(r.app.project.tracks[0].inserts[0].name(), "Channel EQ");
    let e = r.err("effect.remove", json!({"track": 1, "effect": "insert:7"}));
    assert!(e.message.contains("its effects are"), "{}", e.message);
    r.call("effect.remove", json!({"track": 1, "effect": "insert:2"}));
    r.call("effect.remove", json!({"track": 1, "effect": nat_ref}));
    assert_eq!(
        (
            r.app.project.tracks[0].inserts.len(),
            r.app.project.tracks[0].au_effects.len()
        ),
        (1, 0)
    );
    r.call(
        "effect.remove",
        json!({"track": "master", "effect": "insert:1"}),
    );
    assert!(r.app.project.master_inserts.is_empty());
}

#[test]
fn every_parameter_of_a_strip_is_listed_read_and_set() {
    let mut r = Rig::new();
    r.call("effect.add", json!({"track": 1, "id": "Compressor"}));
    let native = &prowave_engine::native_effects()[0];
    let nat = r.call("effect.add", json!({"track": 1, "id": native.id}));
    let all = r.call("plugin.params", json!({"track": 1}));
    let devices: Vec<&str> = all["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| s(&d["device"]))
        .collect();
    assert_eq!(devices[..2], ["mixer", "insert:1"]);
    assert_eq!(devices[2], s(&nat["effect"]["ref"]));

    // The mixer: the fader in dB.
    let v = r.call(
        "param.set",
        json!({"track": 1, "param": "volume", "value": -12}),
    );
    assert_eq!((f(&v["value"]), s(&v["text"])), (-12.0, "-12.0 dB"));
    assert_eq!(r.app.project.tracks[0].volume_db, -12.0);
    r.call(
        "param.set",
        json!({"track": 1, "param": "mute", "value": 1}),
    );
    assert!(r.app.project.tracks[0].mute);
    let e = r.err("param.set", json!({"track": 1, "param": "pan", "value": 3}));
    assert!(e.message.contains("goes from -1 to 1"), "{}", e.message);

    // A built-in effect's parameter, by key and by name, in its own units.
    let comp = r.call("plugin.params", json!({"track": 1, "effect": "insert:1"}));
    let first = &comp["devices"][0]["params"][0];
    let (key, name) = (
        s(&first["param"]).to_string(),
        s(&first["name"]).to_string(),
    );
    let (lo, hi) = (f(&first["min"]), f(&first["max"]));
    let mid = ((lo + hi) / 2.0 * 100.0).round() / 100.0;
    r.call("param.set", json!({"track": 1, "param": key, "value": mid}));
    assert!((f64::from(r.app.project.tracks[0].inserts[0].params[0]) - mid).abs() < 1e-3);
    let got = r.call(
        "param.get",
        json!({"track": 1, "param": name, "effect": "insert:1"}),
    );
    assert!((f(&got["value"]) - mid).abs() < 1e-3);
    assert_eq!(s(&got["device"]), "insert:1");

    // A native effect keeps its settings in its slot, and the running effect is told.
    let np = r.call(
        "plugin.params",
        json!({"track": 1, "effect": nat["effect"]["ref"]}),
    );
    let p0 = &np["devices"][0]["params"][0];
    let target = f(&p0["max"]);
    r.call(
        "param.set",
        json!({"track": 1, "param": p0["param"], "value": target}),
    );
    let slot = &r.app.project.tracks[0].au_effects[0];
    assert_eq!(
        f64::from(crate::native_fx_ui::values(slot, native)[0]),
        target
    );
    assert_eq!(r.app.ui.native_edits.len(), 1);
    assert_eq!(
        f(&r.call("param.get", json!({"track": 1, "param": p0["param"]}))["value"]),
        target
    );

    // The Stereo Out's fader.
    r.call(
        "param.set",
        json!({"track": "master", "param": "volume", "value": -6}),
    );
    assert!((r.app.project.master_vol - 0.501_187).abs() < 1e-4);
    assert_eq!(
        r.err("param.get", json!({"track": 1, "param": "wobble"}))
            .code,
        Code::NotFound
    );
    assert_eq!(
        r.err("preset.list", json!({"track": 1, "effect": "insert:1"}))
            .code,
        Code::NotSupported
    );
    let presets = r.call(
        "preset.list",
        json!({"track": 1, "effect": nat["effect"]["ref"]}),
    );
    assert!(presets["presets"].is_array());
}

#[test]
fn automation_points_are_written_read_and_cleared_in_the_parameters_own_units() {
    let mut r = Rig::new();
    let lane = r.call(
        "automation.set",
        json!({"track": 1, "param": "volume", "points": [
            {"bar": 1, "value": -20}, {"bar": 5, "value": 0}, {"position": "9.1", "value": -6},
        ]}),
    );
    let pts = lane["lane"]["points"].as_array().unwrap();
    assert_eq!(pts.len(), 3);
    assert_eq!((f(&pts[1]["at"]["bar"]), f(&pts[1]["value"])), (5.0, 0.0));
    let model = &r.app.project.tracks[0].auto.volume;
    assert_eq!((model[2].beat, model[2].value), (32.0, -6.0));
    // An effect's parameter is kept as a position in the lane and read back in its own units.
    r.call("effect.add", json!({"track": 1, "id": "Compressor"}));
    let comp = r.call("plugin.params", json!({"track": 1, "effect": "insert:1"}));
    let p0 = &comp["devices"][0]["params"][0];
    let (lo, hi) = (f(&p0["min"]), f(&p0["max"]));
    let set = r.call(
        "automation.set",
        json!({"track": 1, "param": p0["param"], "points": [{"bar": 2, "value": hi}, {"bar": 3, "value": lo}]}),
    );
    let got = set["lane"]["points"].as_array().unwrap();
    assert!((f(&got[0]["value"]) - hi).abs() < (hi - lo).abs() * 1e-3 + 1e-6);
    assert!((f(&got[1]["value"]) - lo).abs() < (hi - lo).abs() * 1e-3 + 1e-6);
    let fx_lane = &r.app.project.tracks[0].auto.fx[0];
    assert_eq!(
        (fx_lane.points[0].value, fx_lane.points[1].value),
        (1.0, 0.0)
    );

    let all = r.call("automation.get", json!({"track": 1}));
    assert_eq!(all["lanes"].as_array().unwrap().len(), 2);
    let e = r.err(
        "automation.set",
        json!({"track": 1, "param": "pan", "points": [{"bar": 1, "value": 9}]}),
    );
    assert!(e.message.contains("point 1"), "{}", e.message);
    assert!(
        r.app.project.tracks[0].auto.pan.is_empty(),
        "a bad point changes nothing"
    );
    // replace empties the lane first.
    r.call("automation.set", json!({"track": 1, "param": "volume", "replace": true, "points": [{"bar": 2, "value": -3}]}));
    assert_eq!(r.app.project.tracks[0].auto.volume.len(), 1);
    r.call("automation.clear", json!({"track": 1, "param": "volume"}));
    assert!(r.app.project.tracks[0].auto.volume.is_empty());
    r.call("automation.clear", json!({"track": 1, "all": true}));
    assert_eq!(
        r.call("automation.get", json!({"track": 1}))["lanes"],
        json!([])
    );
    assert!(r
        .err("automation.clear", json!({"track": 1}))
        .message
        .contains("all: true"));
}

// ------------------------------------------------------------------ regions and files

fn chord(start: f64) -> Value {
    json!([
        {"pitch": 60, "start": start, "length": 4, "velocity": 90, "lyric": "la"},
        {"pitch": 64, "start": start, "length": 4},
        {"pitch": 67, "start": start, "length": 4, "mute": true},
    ])
}

#[test]
fn midi_regions_are_made_and_their_notes_written_with_lyrics() {
    let mut r = Rig::new();
    let made = r.call(
        "region.create",
        json!({"track": 1, "bar": 5, "length_bars": 2, "name": "Chords", "notes": chord(0.0)}),
    );
    let id = s(&made["id"]).to_string();
    assert_eq!(
        (
            f(&made["start_bar"]),
            f(&made["length_bars"]),
            &made["note_count"]
        ),
        (5.0, 2.0, &json!(3))
    );
    let region = &r.app.project.tracks[0].regions[0];
    assert_eq!(
        (region.start_bar, region.len_bars, region.notes.len()),
        (5.0, 2.0, 3)
    );
    assert_eq!(
        (
            region.notes[0].lyric.as_str(),
            region.notes[0].vel,
            region.notes[1].vel
        ),
        ("la", 90, 100)
    );
    assert!(region.notes[2].mute);
    assert_eq!(
        r.app.ui.tracks.selected,
        Some((0, 0)),
        "the new region is selected"
    );

    let got = r.call("region.get", json!({"region": id}));
    assert_eq!(
        got["notes"][0],
        json!({"pitch": 60, "start": 0.0, "length": 4.0, "velocity": 90, "lyric": "la"})
    );
    assert_eq!(got["notes"][2]["mute"], json!(true));
    assert_eq!(
        r.call("region.get", json!({"region": "Chords", "notes": false}))
            .get("notes"),
        None
    );

    // Notes past the end lengthen the region to the bar that holds the last one.
    let added = r.call(
        "region.notes.add",
        json!({"region": id, "notes": chord(12.0)}),
    );
    assert_eq!(
        (
            &added["grown"],
            &added["note_count"],
            f(&added["length_bars"])
        ),
        (&json!(true), &json!(6), 4.0)
    );
    let set = r.call(
        "region.notes.set",
        json!({"region": id, "notes": [{"pitch": 72, "start": 1, "length": 0.5}]}),
    );
    assert_eq!(
        (&set["grown"], &set["note_count"]),
        (&json!(false), &json!(1))
    );
    assert_eq!(r.app.project.tracks[0].regions[0].notes[0].pitch, 72);

    for (bad, why) in [
        (json!([{"pitch": 128, "start": 0, "length": 1}]), "0 to 127"),
        (
            json!([{"pitch": 60, "start": -1, "length": 1}]),
            "before the region",
        ),
        (json!([{"pitch": 60, "start": 0, "length": 0}]), "above 0"),
        (json!([{"pitch": 60, "start": 0}]), "needs `length`"),
        (
            json!([{"pitch": 60, "start": 0, "length": 1, "vel": 3}]),
            "unknown field `vel`",
        ),
        (json!([7]), "not an object"),
    ] {
        let e = r.err("region.notes.set", json!({"region": id, "notes": bad}));
        assert!(e.message.contains(why), "{}", e.message);
    }
    assert_eq!(
        r.app.project.tracks[0].regions[0].notes.len(),
        1,
        "a bad note changes nothing"
    );
    // A region without a length is as long as its notes, in whole bars; an empty one 4 bars.
    let auto = r.call(
        "region.create",
        json!({"track": 1, "bar": 20, "notes": chord(4.0)}),
    );
    assert_eq!(f(&auto["length_bars"]), 2.0);
    assert_eq!(
        f(&r.call("region.create", json!({"track": 1, "bar": 30}))["length_bars"]),
        4.0
    );
    r.call("track.add", json!({"kind": "audio"}));
    assert_eq!(
        r.err("region.create", json!({"track": 2, "bar": 1})).code,
        Code::NotSupported
    );
}

#[test]
fn regions_are_moved_copied_resized_split_looped_and_deleted() {
    let mut r = Rig::new();
    r.call("track.add", json!({"name": "Other"}));
    let made = r.call(
        "region.create",
        json!({"track": 1, "bar": 1, "length_bars": 4, "name": "A", "notes": chord(0.0)}),
    );
    let id = s(&made["id"]).to_string();

    let moved = r.call("region.move", json!({"region": id, "bar": 9}));
    assert_eq!(
        (s(&moved["id"]), f(&moved["start_bar"])),
        (id.as_str(), 9.0)
    );
    let moved = r.call("region.move", json!({"region": id, "track": "Other"}));
    assert_eq!(
        s(&moved["id"]),
        id,
        "the id follows the region to another track"
    );
    assert_eq!(
        (
            r.app.project.tracks[0].regions.len(),
            r.app.project.tracks[1].regions.len()
        ),
        (0, 1)
    );
    assert!(r
        .err("region.move", json!({"region": id}))
        .message
        .contains("say where it goes"));

    let copy = r.call("region.copy", json!({"region": id, "bar": 1, "track": 1}));
    let copy_id = s(&copy["id"]).to_string();
    assert_ne!(copy_id, id);
    assert_eq!(
        (f(&copy["start_bar"]), &copy["note_count"]),
        (1.0, &json!(3))
    );
    assert_eq!(
        s(&r.call("region.get", json!({"region": id}))["track"]),
        s(&moved["track"])
    );

    let sized = r.call(
        "region.resize",
        json!({"region": copy_id, "length_bars": 8}),
    );
    assert_eq!(f(&sized["length_bars"]), 8.0);
    let sized = r.call("region.resize", json!({"region": copy_id, "end_bar": 5}));
    assert_eq!(f(&sized["length_bars"]), 4.0);
    assert!(r
        .err("region.resize", json!({"region": copy_id, "end_bar": 1}))
        .message
        .contains("end after it starts"));

    let cut = r.call("region.split", json!({"region": copy_id, "bar": 3}));
    assert_eq!(
        (s(&cut["left"]["id"]), f(&cut["left"]["length_bars"])),
        (copy_id.as_str(), 2.0)
    );
    assert_eq!(
        (
            f(&cut["right"]["start_bar"]),
            f(&cut["right"]["length_bars"])
        ),
        (3.0, 2.0)
    );
    let right = s(&cut["right"]["id"]).to_string();
    assert!(r
        .err("region.split", json!({"region": copy_id, "bar": 30}))
        .message
        .contains("inside the region"));
    let joined = r.call("region.join", json!({"regions": [copy_id, right]}));
    assert_eq!(f(&joined["length_bars"]), 4.0);
    assert_eq!(r.app.project.tracks[0].regions.len(), 1);

    let looped = r.call("region.loop", json!({"region": copy_id, "repeats": 2}));
    assert_eq!(f(&looped["loop_beats"]), 32.0);
    let looped = r.call("region.loop", json!({"region": copy_id, "end_bar": 9}));
    assert_eq!(f(&looped["loop_beats"]), 16.0);
    assert_eq!(
        f(&r.call("region.loop", json!({"region": copy_id, "off": true}))["loop_beats"]),
        0.0
    );

    let set = r.call("region.set", json!({"region": copy_id, "name": "Hook", "mute": true, "transpose": 12, "quantize_beats": 0.25}));
    assert_eq!(
        (s(&set["name"]), &set["mute"], &set["transpose"]),
        ("Hook", &json!(true), &json!(12))
    );
    assert_eq!(
        r.err("region.set", json!({"region": "Hook", "gain_db": 3}))
            .code,
        Code::NotSupported
    );

    let gone = r.call("region.delete", json!({"region": "Hook"}));
    assert_eq!(s(&gone["deleted"]), copy_id);
    assert_eq!(
        r.err("region.get", json!({"region": copy_id})).code,
        Code::NotFound
    );
    r.call("project.undo", json!({}));
    assert_eq!(
        r.app.project.tracks[0].regions.len(),
        1,
        "Undo brings the region back"
    );
}

#[test]
fn midi_and_audio_files_are_imported_with_the_apps_own_import() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Rig::new();
    let two =
        json!([{"pitch": 60, "start": 0, "length": 2}, {"pitch": 64, "start": 2, "length": 2}]);
    r.call(
        "region.create",
        json!({"track": 1, "bar": 1, "length_bars": 2, "notes": two}),
    );
    let mid = dir.path().join("part.mid");
    let out = r.call("export.midi", json!({"path": mid}));
    assert_eq!(out["whole_project"], json!(true));
    assert!(r
        .err("export.midi", json!({"path": mid}))
        .message
        .contains("already there"));
    let imported = r.call("import.midi", json!({"path": mid, "bar": 9}));
    assert_eq!(imported["tracks_made"], json!(1));
    let region = &imported["tracks"][0]["regions"][0];
    assert_eq!(
        (f(&region["start_bar"]), &region["note_count"]),
        (9.0, &json!(2))
    );
    assert_eq!(r.app.project.tracks.len(), 2);
    assert_eq!(
        r.err("import.midi", json!({"path": dir.path().join("none.mid")}))
            .code,
        Code::NotFound
    );

    // Audio: two seconds of a tone, as a new track at bar 5 and then onto that track at 9.
    let wav = dir.path().join("tone.wav");
    let tone: Vec<f32> = (0..88_200).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
    crate::fit_grid::tests::write_wav(&wav, &tone);
    let a = r.call(
        "import.audio",
        json!({"path": wav, "bar": 5, "fit_to_grid": false}),
    );
    assert_eq!(
        (s(&a["kind"]), f(&a["start_bar"]), &a["fitting"]),
        ("audio", 5.0, &json!(false))
    );
    assert_eq!(
        f(&a["length_bars"]),
        1.0,
        "two seconds at 120 bpm is one bar"
    );
    assert_eq!(r.app.project.tracks.len(), 3);
    assert_eq!(r.app.project.tracks[2].name, "tone");
    assert_eq!(
        r.app.project.position_beats, 0.0,
        "the playhead is where it was"
    );
    let b = r.call(
        "import.audio",
        json!({"path": wav, "bar": 9, "track": "tone", "fit_to_grid": false}),
    );
    assert_eq!((s(&b["track"]), f(&b["start_bar"])), (s(&a["track"]), 9.0));
    assert_eq!(
        (
            r.app.project.tracks.len(),
            r.app.project.tracks[2].regions.len()
        ),
        (3, 2)
    );
    assert_eq!(
        r.err("import.audio", json!({"path": wav, "track": 1})).code,
        Code::NotSupported
    );
    assert!(r
        .err("import.audio", json!({"path": mid}))
        .message
        .contains("not an audio file"));
    let set = r.call(
        "region.set",
        json!({"region": b["id"], "gain_db": -3, "fade_in_beats": 1}),
    );
    assert_eq!(
        (
            f(&set["audio"]["gain_db"]),
            f(&set["audio"]["fade_in_beats"])
        ),
        (-3.0, 1.0)
    );
    // Fit to Grid starts the app's background analysis.
    let c = r.call(
        "import.audio",
        json!({"path": wav, "bar": 13, "fit_to_grid": true}),
    );
    assert_eq!(
        (&c["fit_to_grid"], &c["fitting"]),
        (&json!(true), &json!(true))
    );
    assert_eq!(r.state()["busy"]["fit_to_grid"], json!(true));
}

#[test]
fn markers_and_tempo_changes_are_added_changed_and_removed() {
    let mut r = Rig::new();
    r.call("marker.add", json!({"bar": 5, "name": "Verse"}));
    let m = r.call("marker.add", json!({"bar": 1}));
    assert_eq!(m["marker"], json!(1));
    assert_eq!(m["markers"][1]["name"], json!("Verse"));
    r.call(
        "marker.set",
        json!({"marker": "Verse", "name": "Chorus", "bar": 9}),
    );
    assert_eq!(
        (
            r.app.project.markers[1].name.as_str(),
            r.app.project.markers[1].beat
        ),
        ("Chorus", 32.0)
    );
    assert!(r
        .err("marker.set", json!({"marker": 2, "bar": 1}))
        .message
        .contains("another marker"));
    assert!(r
        .err("marker.delete", json!({"marker": "Bridge"}))
        .message
        .contains("the markers are"));
    r.call("marker.delete", json!({"marker": 1}));
    assert_eq!(
        r.call("marker.list", json!({}))["markers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let t = r.call("tempo.add", json!({"bar": 9, "bpm": 140, "ramp": false}));
    assert_eq!(t["tempo_changes"][1]["bpm"], json!(140.0));
    assert_eq!(
        f(&r.call("transport.tempo", json!({"bar": 10}))["bpm"]),
        140.0
    );
    assert_eq!(
        f(&r.call("transport.tempo", json!({"bar": 2}))["bpm"]),
        120.0
    );
    r.call("tempo.remove", json!({"bar": 9}));
    assert!(r.app.project.tempo_changes.is_empty());
    assert_eq!(
        r.err("tempo.remove", json!({"bar": 9})).code,
        Code::NotFound
    );
}

// ------------------------------------------------------------------ the interface

#[test]
fn menus_are_listed_and_chosen_and_file_windows_are_refused() {
    let mut r = Rig::new();
    let list = r.call("menu.list", json!({}));
    let entries = list["entries"].as_array().unwrap();
    assert!(entries.len() > 60, "{} entries", entries.len());
    let find = |id: &str| {
        entries
            .iter()
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("no {id}"))
    };
    assert_eq!(find("Undo")["enabled"], json!(false));
    assert_eq!(
        (
            s(&find("Save As…")["menu"]),
            &find("Save As…")["opens_system_window"]
        ),
        ("File", &json!(true))
    );
    assert_eq!(find("Metronome…")["submenu"], json!("Project Settings"));
    assert_eq!(
        r.call("menu.list", json!({"menu": "edit"}))["entries"][0]["menu"],
        json!("Edit")
    );

    r.call("menu.invoke", json!({"id": "New Audio Track"}));
    assert_eq!(r.app.project.tracks.len(), 2);
    let inspector = r.app.ui.show_inspector;
    r.call("menu.invoke", json!({"id": "Show Inspector"}));
    assert_eq!(r.app.ui.show_inspector, !inspector);
    r.call("menu.invoke", json!({"id": "Metronome"}));
    assert!(r.app.ui.settings_tab.is_some(), "the … may be left out");
    let e = r.err("menu.invoke", json!({"id": "Open…"}));
    assert_eq!(e.code, Code::NotSupported);
    assert!(e.message.contains("project.open"), "{}", e.message);
    assert_eq!(
        r.err("menu.invoke", json!({"id": "Undo History Party"}))
            .code,
        Code::NotFound
    );
    assert_eq!(
        r.err("menu.invoke", json!({"id": "Join"})).code,
        Code::NotSupported
    );
}

#[test]
fn windows_the_view_and_the_selection_are_set() {
    let mut r = Rig::new();
    let w = r.call("ui.window", json!({"id": "piano_roll"}));
    assert_eq!(w, json!({"id": "piano_roll", "open": true}));
    assert!(r.app.ui.bottom == crate::Bottom::PianoRoll);
    r.call("ui.window", json!({"id": "mixer"}));
    assert!(r.app.ui.bottom == crate::Bottom::Mixer && r.app.ui.show_mixer);
    r.call(
        "ui.window",
        json!({"id": "project_settings", "tab": "audio"}),
    );
    assert!(r.app.ui.settings_tab == Some(crate::project_settings::SettingsTab::Audio));
    r.call(
        "ui.window",
        json!({"id": "project_settings", "open": false}),
    );
    r.call("ui.window", json!({"id": "tempo_track"}));
    let list = r.call("ui.windows", json!({}));
    let open = |id: &str| {
        list["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id)
            .unwrap()["open"]
            .clone()
    };
    assert_eq!(
        (open("mixer"), open("tempo_track"), open("project_settings")),
        (json!(true), json!(true), json!(false))
    );
    assert_eq!(
        r.err("ui.window", json!({"id": "basement"})).code,
        Code::InvalidParams
    );

    // Bars 5 to 9 on lanes 1000 points wide: 250 points a bar, scrolled to bar 5.
    let v = r.call("ui.view", json!({"bar": 5, "end_bar": 9}));
    assert_eq!((f(&v["bar_width"]), f(&v["first_bar"])), (250.0, 5.0));
    assert_eq!(r.app.ui.tracks.scroll_x, 1000.0);

    r.call("track.add", json!({"name": "Two"}));
    let a = r.call(
        "region.create",
        json!({"track": 1, "bar": 1, "select": false}),
    );
    let b = r.call(
        "region.create",
        json!({"track": 2, "bar": 1, "select": false}),
    );
    let sel = r.call("ui.select", json!({"regions": [b["id"], a["id"]]}));
    assert_eq!(sel["regions"], json!([a["id"], b["id"]]));
    assert_eq!(
        (r.app.ui.tracks.selected, r.app.project.selected_track),
        (Some((1, 0)), 1)
    );
    r.call("ui.select", json!({"track": 1, "none": true}));
    assert_eq!(
        (r.app.ui.tracks.selected, r.app.project.selected_track),
        (None, 0)
    );
    assert_eq!(
        r.call("ui.select", json!({"all_regions": true}))["regions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    r.app.ui.status = Some("disk too slow".into());
    assert_eq!(
        r.call("ui.status", json!({"clear": true}))["status"],
        json!("disk too slow")
    );
    assert!(r.app.ui.status.is_none());
    // A picture is asked for and answered later, once a frame has been drawn.
    assert_eq!(
        r.try_call("ui.screenshot", json!({"scale": 0.25})).unwrap(),
        Value::Null
    );
    assert_eq!(r.core.shots.len(), 1);
    assert!(r
        .err("ui.screenshot", json!({"path": "/tmp/a.jpg"}))
        .message
        .contains(".png"));
    let n = r.call(
        "midi.note",
        json!({"pitch": 64, "velocity": 80, "track": 2}),
    );
    assert_eq!(n["on"], json!(true));
    assert!(matches!(
        r.app.ui.audition[0],
        crate::piano_roll::Audition::On {
            pitch: 64,
            vel: 80,
            track: 1
        }
    ));
    r.call("midi.panic", json!({}));
    assert!(r.app.ui.midi_panic && r.app.ui.audition.is_empty());
}

#[test]
fn an_open_question_stops_changes_until_it_is_answered() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Rig::new();
    assert_eq!(r.call("ui.dialog", json!({}))["dialog"], Value::Null);
    r.call("transport.tempo", json!({"bpm": 99}));
    // File > New with unsaved changes asks first.
    let ctx = r.ctx.clone();
    r.app.run_file_cmd(FileCmd::New, &ctx);
    let d = r.call("ui.dialog", json!({}));
    assert_eq!(s(&d["dialog"]["id"]), "save_changes");
    assert_eq!(
        d["dialog"]["buttons"],
        json!(["save", "dont_save", "cancel"])
    );
    assert!(s(&d["dialog"]["text"]).contains("Untitled"));
    // Changes are refused and say why; reading still works.
    let e = r.err("transport.tempo", json!({"bpm": 80}));
    assert_eq!(e.code, Code::DialogOpen);
    assert!(
        e.message.contains("ui.dialog.answer") && e.message.contains("dont_save"),
        "{}",
        e.message
    );
    assert_eq!(e.data["dialog"]["id"], json!("save_changes"));
    assert_eq!(r.state()["dialog"]["id"], json!("save_changes"));
    assert_eq!(r.app.project.tempo, 99.0);
    assert!(r
        .err("ui.dialog.answer", json!({"button": "maybe"}))
        .message
        .contains("has the buttons"));
    // Save on a project without a file needs the file.
    assert!(r
        .err("ui.dialog.answer", json!({"button": "save"}))
        .message
        .contains("give `path`"));
    let a = r.call("ui.dialog.answer", json!({"button": "cancel"}));
    assert_eq!(
        (&a["answered"], &a["dialog"]),
        (&json!("save_changes"), &Value::Null)
    );
    assert_eq!(
        r.app.project.tempo, 99.0,
        "cancelled: the project is still there"
    );

    r.app.run_file_cmd(FileCmd::New, &ctx);
    let file = dir.path().join("Kept.prowave");
    r.call("ui.dialog.answer", json!({"button": "save", "path": file}));
    assert!(file.is_file());
    assert_eq!(
        (r.app.project.name.as_str(), r.app.project.tempo),
        ("Untitled", 120.0)
    );

    // Quitting says what happens to unsaved changes and never waits on the question.
    r.call("transport.tempo", json!({"bpm": 70}));
    let q = r.call("app.quit", json!({"if_unsaved": "cancel"}));
    assert_eq!(q, json!({"quitting": false, "unsaved_changes": true}));
    assert!(!r.core.quit);
    assert!(r
        .err("app.quit", json!({}))
        .message
        .contains("needs the parameter `if_unsaved`"));
    assert!(r
        .err("app.quit", json!({"if_unsaved": "save"}))
        .message
        .contains("give `path`"));
    let q = r.call("app.quit", json!({"if_unsaved": "discard"}));
    assert_eq!(q["quitting"], json!(true));
    assert!(r.core.quit && r.app.ui.ask_save.is_none());

    // The list of missing audio files is a question too.
    let mut r = Rig::new();
    r.app.ui.missing_audio = vec!["take 3.wav".into()];
    let d = r.call("ui.dialog", json!({}));
    assert_eq!(
        (s(&d["dialog"]["id"]), &d["dialog"]["detail"]),
        ("missing_audio", &json!(["take 3.wav"]))
    );
    r.call("ui.dialog.answer", json!({"button": "ok"}));
    assert!(r.app.ui.missing_audio.is_empty());
    assert_eq!(
        r.err("ui.dialog.answer", json!({"button": "ok"})).code,
        Code::NotFound
    );
}

#[test]
fn a_range_is_rendered_offline_to_a_wav_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Rig::new();
    r.call(
        "region.create",
        json!({"track": 1, "bar": 1, "length_bars": 2, "notes": chord(0.0)}),
    );
    let wav = dir.path().join("mix.wav");
    let out = r.call(
        "render.range",
        json!({"bar": 1, "end_bar": 3, "path": wav, "tail_seconds": 0.5}),
    );
    // Two bars at 120 bpm are four seconds, and the tail.
    assert_eq!(
        (f(&out["seconds"]), &out["sample_rate"]),
        (4.5, &json!(48_000))
    );
    assert_eq!(out["rendered_tracks"].as_array().unwrap().len(), 1);
    // A track without an instrument makes no sound, and the result says so.
    assert_eq!(
        (&out["silent"], &out["peak_db"]),
        (&json!(true), &Value::Null)
    );
    let bytes = std::fs::read(&wav).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(bytes.len(), 44 + 216_000 * 8);
    assert!(r
        .err("render.range", json!({"bar": 1, "end_bar": 3, "path": wav}))
        .message
        .contains("already there"));
    assert!(r
        .err(
            "render.range",
            json!({"bar": 5, "end_bar": 2, "path": dir.path().join("b.wav")})
        )
        .message
        .contains("end after it starts"));
    assert!(r
        .err("render.range", json!({"path": dir.path().join("b.mp3")}))
        .message
        .contains(".wav"));
    let one = r.call(
        "render.range",
        json!({"track": 1, "path": wav, "overwrite": true}),
    );
    assert_eq!(
        f(&one["to"]["bar"]),
        3.0,
        "without an end it runs to the end of the last region"
    );
    assert_eq!(r.call("project.get", json!({}))["can_undo"], json!(true));
}

#[test]
fn a_project_is_bounced_to_a_file_with_each_track_beside_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Rig::new();
    r.call(
        "region.create",
        json!({"track": 1, "bar": 1, "length_bars": 2, "notes": chord(0.0)}),
    );
    let wav = dir.path().join("Song.wav");
    let out = r.call(
        "export.bounce",
        json!({"path": wav, "bit_depth": 16, "stems": true, "tail_seconds": 0.5}),
    );
    // Two bars at 120 bpm are four seconds, and the tail asked for.
    assert_eq!(
        (
            f(&out["seconds"]),
            f(&out["range_seconds"]),
            &out["sample_rate"]
        ),
        (4.5, 4.0, &json!(48_000))
    );
    assert_eq!(s(&out["format"]), "WAV, 16-bit");
    assert_eq!(out["complete"], json!(true), "{out}");
    assert_eq!(out["not_rendered"], json!([]));
    // The mix, and the one track that has something to play.
    let files = out["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{out}");
    assert_eq!(
        (&files[0]["track"], &files[0]["name"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(files[1]["track"], out["rendered_tracks"][0]["track"]);
    for file in files {
        let bytes = std::fs::read(s(&file["path"])).unwrap();
        assert_eq!(bytes.len(), 44 + 216_000 * 4, "{file}");
    }
    assert_eq!(s(&out["path"]), wav.to_string_lossy());
    let status = r.call("export.bounce.status", json!({}));
    assert_eq!(
        (&status["running"], &status["ok"], &status["result"]["path"]),
        (&json!(false), &json!(true), &out["path"])
    );
    // Mistakes are answered at once.
    assert!(r
        .err("export.bounce", json!({"path": wav}))
        .message
        .contains("already there"));
    assert!(r
        .err("export.bounce", json!({"path": dir.path().join("a.mp3")}))
        .message
        .contains(".wav or .aiff"));
    assert!(r
        .err(
            "export.bounce",
            json!({"path": dir.path().join("a.aiff"), "bit_depth": 32})
        )
        .message
        .contains("AIFF"));
    assert_eq!(
        r.err(
            "export.bounce",
            json!({"path": dir.path().join("a.wav"), "bit_depth": 20})
        )
        .code,
        Code::InvalidParams
    );
    assert_eq!(
        r.err(
            "export.bounce",
            json!({"path": dir.path().join("a.wav"), "bar": 5, "end_bar": 2})
        )
        .code,
        Code::InvalidParams
    );
    // Without waiting: answered at once, one bounce at a time, and it can be stopped.
    let later = dir.path().join("later.wav");
    let begun = r.call("export.bounce", json!({"path": later, "wait": false}));
    assert_eq!(begun["started"], json!(true));
    assert!(r
        .err("export.bounce", json!({"path": dir.path().join("c.wav")}))
        .message
        .contains("already running"));
    assert_eq!(
        r.call("export.bounce.status", json!({}))["running"],
        json!(true)
    );
    assert_eq!(
        r.call("export.bounce.cancel", json!({}))["cancelled"],
        json!(true)
    );
    let began = Instant::now();
    while r.app.ui.bounce.job.is_some() {
        crate::bounce_ui::tend(&mut r.app, &r.ctx);
        assert!(began.elapsed() < Duration::from_secs(60));
    }
    let status = r.call("export.bounce.status", json!({}));
    assert_eq!(
        (&status["running"], &status["ok"]),
        (&json!(false), &json!(false))
    );
    assert!(s(&status["error"]).contains("cancelled"));
    assert!(!later.exists());
    assert_eq!(
        r.call("export.bounce.cancel", json!({}))["cancelled"],
        json!(false)
    );
    // A bounce changes nothing in the project.
    assert!(!r.app.project.playing && r.app.project.position_beats == 0.0);
}

/// Wrong, odd and hostile parameters to every method: each call must come back (as a result
/// or an error), never take the app down.
#[test]
fn no_method_panics_on_bad_input() {
    let mut r = Rig::new();
    r.call("region.create", json!({"track": 1, "bar": 1, "name": "R"}));
    let odd = [
        json!(null),
        json!(-1),
        json!(0),
        json!(1e300),
        json!(""),
        json!("t999"),
        json!("r999"),
        json!("plugin:999"),
        json!("insert:0"),
        json!("0.0.0.0"),
        json!("99999999.1"),
        json!([]),
        json!([{}]),
        json!({}),
        json!(true),
        json!("../../etc/passwd"),
        json!("\u{0}\u{202e}"),
    ];
    let mut calls = 0;
    for m in registry::all() {
        // Not the ones that would end the test or replace the rig's state for the rest.
        if matches!(m.name, "app.quit" | "batch") {
            continue;
        }
        for v in &odd {
            let mut one = Map::new();
            let mut all = Map::new();
            for p in &m.params {
                all.insert(p.name.to_string(), v.clone());
            }
            let _ = r.try_call(m.name, Value::Object(all));
            // A bounce the call started is not waited for here.
            if let Some(job) = r.app.ui.bounce.job.take() {
                job.cancel();
            }
            r.core.bounces.clear();
            if matches!(m.name, "render.range" | "export.bounce") {
                continue;
            }
            for p in &m.params {
                one.clear();
                one.insert(p.name.to_string(), v.clone());
                if p.name != "track" && m.params.iter().any(|q| q.name == "track") {
                    one.insert("track".into(), json!(1));
                }
                if p.name != "region" && m.params.iter().any(|q| q.name == "region") {
                    one.insert("region".into(), json!("R"));
                }
                let _ = r.try_call(m.name, Value::Object(one.clone()));
                calls += 1;
            }
        }
        r.app.ui.ask_save = None;
        r.app.ui.ask_revert = false;
        r.app.ui.missing_audio.clear();
        if r.app.project.tracks.is_empty() {
            r.app.project = crate::model::Project::empty();
        }
    }
    assert!(calls > 2000, "{calls} calls");
    r.state();
}

// ------------------------------------------------------------------ the socket

/// A socket path short enough for a Unix socket (the temp folder's own path is too long).
fn socket_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("pwc")
        .tempdir_in("/tmp")
        .unwrap()
}

/// The app with its control socket at `path`, pumped by the calling thread.
struct Live {
    app: App,
    ctx: egui::Context,
}

impl Live {
    fn start(path: &Path) -> Self {
        let mut app = super::headless::app_without_window(None);
        app.control = Some(Control::start_at(path, false).unwrap());
        Self {
            app,
            ctx: egui::Context::default(),
        }
    }

    /// Serves requests until `done` says so (or five seconds pass).
    fn serve_until(&mut self, done: impl Fn() -> bool) {
        let began = Instant::now();
        while !done() {
            super::pump(&mut self.app, &self.ctx);
            assert!(
                began.elapsed() < Duration::from_secs(20),
                "the client never finished"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        super::pump(&mut self.app, &self.ctx);
    }

    /// Runs `client` on its own thread against the socket while this thread serves it.
    fn with_client<T: Send + 'static>(
        &mut self,
        path: &Path,
        client: impl FnOnce(Client) -> T + Send + 'static,
    ) -> T {
        let path = path.to_path_buf();
        let handle = std::thread::spawn(move || client(Client::connect(&path)));
        self.serve_until(|| handle.is_finished());
        handle.join().unwrap()
    }
}

struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next: u64,
}

impl Client {
    fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            stream,
            next: 1,
        }
    }

    fn send(&mut self, line: &str) {
        self.stream.write_all(line.as_bytes()).unwrap();
        self.stream.write_all(b"\n").unwrap();
    }

    fn line(&mut self) -> Value {
        let mut text = String::new();
        self.reader.read_line(&mut text).unwrap();
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text:?}"))
    }

    /// Sends a request and reads lines until its answer; events on the way are returned too.
    fn call(&mut self, method: &str, params: Value) -> (Value, Vec<Value>) {
        let id = self.next;
        self.next += 1;
        self.send(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
        );
        let mut events = Vec::new();
        loop {
            let v = self.line();
            if v.get("id") == Some(&json!(id)) {
                return (v, events);
            }
            events.push(v);
        }
    }
}

#[test]
fn requests_go_over_the_socket_and_change_the_project() {
    let dir = socket_dir();
    let path = dir.path().join("c.sock");
    let mut live = Live::start(&path);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let results = live.with_client(&path, |mut c| {
        let version = c.call("rpc.version", json!({})).0;
        let tempo = c.call("transport.tempo", json!({"bpm": 96})).0;
        let track = c
            .call("track.add", json!({"kind": "audio", "name": "Gtr"}))
            .0;
        let bad = c.call("transport.tempo", json!({"bpm": "fast"})).0;
        // A notification gets no answer: the next line is the ping's.
        c.send(r#"{"jsonrpc":"2.0","method":"transport.locate","params":{"bar":3}}"#);
        let ping = c.call("rpc.ping", json!({})).0;
        let stats = c.call("rpc.stats", json!({})).0;
        (version, tempo, track, bad, ping, stats)
    });
    let (version, tempo, track, bad, ping, stats) = results;
    assert_eq!(version["result"]["protocol"], json!(protocol::PROTOCOL));
    assert_eq!(tempo["result"]["bpm"], json!(96.0));
    assert_eq!(track["result"]["name"], json!("Gtr"));
    assert_eq!(bad["error"]["data"]["code"], json!("invalid_params"));
    assert_eq!(ping["result"], json!({"pong": true}));
    assert!(f(&stats["result"]["handling"]["requests"]) >= 6.0);
    // The project in the app is what the calls made it.
    assert_eq!(
        (live.app.project.tempo, live.app.project.tracks.len()),
        (96.0, 2)
    );
    assert_eq!(live.app.project.position_beats, 8.0);
    assert!(live.app.has_unsaved_changes());
    assert!(live.app.history.can_undo());

    // The socket file goes with the app.
    drop(live);
    assert!(!path.exists());
}

#[test]
fn rubbish_a_huge_line_and_a_client_that_leaves_do_no_harm() {
    let dir = socket_dir();
    let path = dir.path().join("c.sock");
    let mut live = Live::start(&path);
    let out = live.with_client(&path, |mut c| {
        c.send("{this is not json");
        let parse = c.line();
        c.send("");
        c.send("[1,2,3]");
        let shape = c.line();
        c.stream.write_all(&[0xff, 0xfe, b'\n']).unwrap();
        let utf8 = c.line();
        // A line over the limit is thrown away and answered; the connection lives on.
        let big = vec![b'x'; protocol::MAX_LINE + 4096];
        c.stream.write_all(&big).unwrap();
        c.stream.write_all(b"\n").unwrap();
        let too_large = c.line();
        let after = c.call("rpc.ping", json!({})).0;
        (parse, shape, utf8, too_large, after)
    });
    assert_eq!(out.0["error"]["data"]["code"], json!("parse_error"));
    assert_eq!(out.1["error"]["data"]["code"], json!("invalid_request"));
    assert_eq!(out.2["error"]["data"]["code"], json!("parse_error"));
    assert_eq!(out.3["error"]["data"]["code"], json!("too_large"));
    assert_eq!(out.3["error"]["data"]["limit"], json!(protocol::MAX_LINE));
    assert_eq!(out.4["result"], json!({"pong": true}));

    // A client sends requests and is gone before they are answered; another one half a line.
    {
        let mut gone = UnixStream::connect(&path).unwrap();
        for i in 0..50 {
            let line = json!({"jsonrpc": "2.0", "id": i, "method": "state.get"}).to_string();
            gone.write_all(line.as_bytes()).unwrap();
            gone.write_all(b"\n").unwrap();
        }
        let mut half = UnixStream::connect(&path).unwrap();
        half.write_all(br#"{"jsonrpc":"2.0","id":1,"meth"#).unwrap();
    }
    std::thread::sleep(Duration::from_millis(50));
    for _ in 0..20 {
        super::pump(&mut live.app, &live.ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    // The app still serves: three clients at once.
    let handles: Vec<_> = (0..3)
        .map(|k| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut c = Client::connect(&path);
                (0..20)
                    .map(|_| {
                        c.call("transport.locate", json!({"bar": k + 2})).0["result"]["playing"]
                            .clone()
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    live.serve_until(|| handles.iter().all(|h| h.is_finished()));
    for h in handles {
        assert_eq!(h.join().unwrap(), vec![json!(false); 20]);
    }
    let clients = live.app.control.as_ref().unwrap().core.shared.clients();
    assert!(clients <= 3, "{clients} connections left open");
}

#[test]
fn a_subscribed_connection_is_told_what_happens() {
    let dir = socket_dir();
    let path = dir.path().join("c.sock");
    let mut live = Live::start(&path);
    let (sub, events) = live.with_client(&path, |mut c| {
        let sub = c.call("events.subscribe", json!({})).0;
        // The app looks once before anything is reported.
        c.call("rpc.wait", json!({"frames": 2}));
        let mut events = Vec::new();
        events.extend(c.call("transport.play", json!({"bar": 2})).1);
        events.extend(c.call("rpc.wait", json!({"frames": 2})).1);
        events.extend(c.call("transport.stop", json!({})).1);
        events.extend(c.call("track.add", json!({"name": "New One"})).1);
        // The slower looks (the track list, the content) come within a quarter second.
        events.extend(
            c.call("rpc.wait", json!({"frames": 400, "timeout_ms": 700}))
                .1,
        );
        events.extend(c.call("events.unsubscribe", json!({})).1);
        c.call("transport.play", json!({}));
        c.call("rpc.wait", json!({"frames": 3}));
        let after = c.call("rpc.ping", json!({})).1;
        assert!(after.is_empty(), "no events after unsubscribing: {after:?}");
        (sub, events)
    });
    assert_eq!(sub["result"]["subscribed"], json!(true));
    let names: Vec<&str> = events.iter().map(|e| s(&e["params"]["event"])).collect();
    for want in [
        "transport.started",
        "transport.stopped",
        "track.added",
        "project.changed",
    ] {
        assert!(names.contains(&want), "no {want} in {names:?}");
    }
    let added = events
        .iter()
        .find(|e| e["params"]["event"] == "track.added")
        .unwrap();
    assert_eq!(
        (&added["method"], &added["params"]["name"]),
        (&json!("event"), &json!("New One"))
    );
    assert!(added.get("id").is_none());
}

#[test]
fn a_socket_left_by_a_run_that_is_gone_is_replaced_and_a_live_one_is_not() {
    let dir = socket_dir();
    let path = dir.path().join("c.sock");
    // A file left behind: nothing answers on it.
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    let first = Control::start_at(&path, false).unwrap();
    let Err(e) = Control::start_at(&path, false) else {
        panic!("a second app took the socket of a running one");
    };
    assert!(e.contains("in use by another running Pro Wave"), "{e}");
    drop(first);
    assert!(!path.exists());
    // A private folder is made for it, for the user alone.
    let own = dir.path().join("settings").join("control.sock");
    let c = Control::start_at(&own, true).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(own.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    drop(c);
}

#[test]
fn midi_fx_and_the_arpeggiator_are_set_by_name_and_undone() {
    use crate::midi_fx::MidiFx;
    let mut r = Rig::new();
    assert_eq!(
        r.call("midi_fx.list", json!({"track": 1}))["effects"],
        json!([])
    );
    // A track without an Arpeggiator gets one, with what is named set.
    let a = r.call(
        "midi_fx.arpeggiator",
        json!({
            "track": 1, "rate": "1/8T", "mode": "down_up", "octaves": 3, "gate": 120,
            "swing": 30, "step_velocity": 40, "latch": true, "trigger": "song_position",
            "length": 6,
            "steps": [
                {"step": 2, "on": false},
                {"step": 3, "velocity": 127, "tie": true, "transpose": -12},
                {"step": 6, "chord": true},
            ],
        }),
    );
    assert_eq!((s(&a["kind"]), f(&a["index"])), ("arpeggiator", 1.0));
    let arp = &a["arpeggiator"];
    assert_eq!(
        (s(&arp["rate"]), s(&arp["mode"]), s(&arp["trigger"])),
        ("1/8T", "down_up", "song_position")
    );
    assert_eq!(arp["steps"].as_array().unwrap().len(), 6);
    assert_eq!(
        arp["steps"][2],
        json!({"step": 3, "on": true, "velocity": 127, "tie": true, "chord": false, "transpose": -12})
    );
    let MidiFx::Arpeggiator(set) = r.app.project.tracks[0].midi_fx[0].fx else {
        panic!("an arpeggiator");
    };
    assert_eq!(
        (
            set.octaves,
            set.gate,
            set.swing,
            set.step_velocity,
            set.latch,
            set.sync,
            set.length
        ),
        (3, 120, 30, 40, true, true, 6)
    );
    assert!(!set.steps[1].on && set.steps[5].chord && set.steps[0].on);
    // Only what is named changes; a preset brings its steps and length.
    let b = r.call("midi_fx.arpeggiator", json!({"track": 1, "gate": 50}));
    assert_eq!(f(&b["arpeggiator"]["octaves"]), 3.0);
    assert_eq!(b["arpeggiator"]["steps"][2]["tie"], json!(true));
    let c = r.call(
        "midi_fx.arpeggiator",
        json!({"track": 1, "preset": "Offbeat"}),
    );
    assert_eq!(f(&c["arpeggiator"]["length"]), 8.0);
    assert_eq!(c["arpeggiator"]["steps"][0]["on"], json!(false));
    let d = r.call(
        "midi_fx.arpeggiator",
        json!({"track": 1, "randomize": true}),
    );
    assert_ne!(d["arpeggiator"]["steps"], c["arpeggiator"]["steps"]);
    // Values outside their ranges and names that do not exist are refused.
    for bad in [
        json!({"track": 1, "octaves": 9}),
        json!({"track": 1, "rate": "1/5"}),
        json!({"track": 1, "steps": [{"step": 40, "on": true}]}),
        json!({"track": 1, "steps": [{"step": 1, "transpose": 99}]}),
        json!({"track": 1, "steps": [{"on": true}]}),
        json!({"track": 1, "index": 4}),
    ] {
        r.err("midi_fx.arpeggiator", bad);
    }
    // The chain: add, switch off, set by numbers, remove.
    let chain = r.call("midi_fx.add", json!({"track": 1, "kind": "transposer"}));
    assert_eq!(chain["effects"].as_array().unwrap().len(), 2);
    let t = r.call(
        "midi_fx.set",
        json!({"track": 1, "index": 2, "on": false, "settings": [7]}),
    );
    assert_eq!(
        (t["on"].clone(), t["settings"].clone()),
        (json!(false), json!([7.0]))
    );
    r.err("midi_fx.arpeggiator", json!({"track": 1, "index": 2}));
    r.err("midi_fx.add", json!({"track": 1, "kind": "polka"}));
    r.err("midi_fx.remove", json!({"track": 1, "index": 3}));
    let left = r.call("midi_fx.remove", json!({"track": 1, "index": 1}));
    assert_eq!(left["effects"][0]["kind"], json!("transposer"));
    assert_eq!(r.app.project.tracks[0].midi_fx.len(), 1);
    // Each change is an Undo step.
    assert_eq!(r.call("project.undo", json!({}))["done"], json!(true));
    assert_eq!(r.app.project.tracks[0].midi_fx.len(), 2);
}

#[test]
fn kits_are_found_read_and_loaded_into_a_rack() {
    let lib = tempfile::tempdir().unwrap();
    let wav = |rel: &str| {
        let path = lib.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let tone: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let stereo: Vec<f32> = tone.iter().flat_map(|s| [*s, *s]).collect();
        prowave_engine::write_wav(&path, &stereo, 44_100).unwrap();
    };
    for f in [
        "BD.wav",
        "SD.wav",
        "CP.wav",
        "CH.wav",
        "OH.wav",
        "Loops/90bpm/x.wav",
    ] {
        wav(&format!("Machines/Roland TR-808/{f}"));
    }
    wav("Machines/LinnDrum/kick.wav");
    wav("Machines/LinnDrum/snare.wav");
    let empty = tempfile::tempdir().unwrap();
    let root = lib.path().to_string_lossy().into_owned();
    let mut r = Rig::new();
    // Nothing set: no folders in a test, and no error.
    let st = r.call("kits.status", json!({}));
    assert_eq!(st["kits"], 0);
    let st = r.call(
        "kits.folders",
        json!({"folders": ["/Volumes/NOT-MOUNTED-HERE/KITS", root]}),
    );
    assert_eq!(st["kits"], 2);
    assert_eq!(st["folders"][0]["present"], false);
    assert_eq!(st["folders"][1]["kits"], 2);
    assert!(st["scan_ms"].as_f64().is_some());
    let found = r.call("kits.search", json!({"query": "roland 808"}));
    assert_eq!(found["total"], 1);
    let path = s(&found["kits"][0]["path"]).to_string();
    assert_eq!(s(&found["kits"][0]["source"]), "Machines");
    let kit = r.call("kits.get", json!({"path": path}));
    assert_eq!(kit["files"], 6);
    assert_eq!(s(&kit["kind"]), "files");
    let classes: Vec<&str> = kit["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| s(&c["class"]))
        .collect();
    assert_eq!(classes, ["kick", "snare", "clap", "closed_hat", "open_hat"]);
    // A load makes a rack track; it is one Undo step.
    let rack = r.call("kits.load", json!({"path": path}));
    assert_eq!(s(&rack["kit"]), "Roland TR-808");
    assert_eq!(rack["channels"].as_array().unwrap().len(), 5);
    assert_eq!(s(&rack["channels"][0]["name"]), "BD");
    let tracks = r.app.project.tracks.len();
    // Another kit into the same rack (it is the selected track), then one sample on channel 8.
    let linn = format!("{root}/Machines/LinnDrum");
    let rack = r.call("kits.load", json!({"path": linn}));
    assert_eq!(r.app.project.tracks.len(), tracks, "the same rack");
    assert_eq!(s(&rack["kit"]), "LinnDrum");
    let rack = r.call(
        "kits.assign",
        json!({"file": format!("{path}/CP.wav"), "channel": 8}),
    );
    assert_eq!(rack["channels"].as_array().unwrap().len(), 8);
    assert_eq!(s(&rack["channels"][7]["name"]), "CP");
    r.call("project.undo", json!({}));
    let slot = r.app.project.tracks[tracks - 1]
        .au_instrument
        .clone()
        .unwrap();
    assert_eq!(crate::channel_rack::decode(&slot.state).len(), 2);
    // Favourites, and what is refused.
    r.call("kits.favorite", json!({"path": linn}));
    assert_eq!(
        r.call("kits.search", json!({"favorites": true}))["total"],
        1
    );
    assert_eq!(
        r.err("kits.get", json!({"path": "/no/such/kit"})).code,
        Code::NotFound
    );
    assert_eq!(
        r.err("kits.load", json!({"path": empty.path()})).code,
        Code::Failed
    );
    r.call("ui.window", json!({"id": "kits", "open": true}));
    assert!(r.app.ui.kits.open);
}

/// rack.offset: each drum class's own share of its offsets, under the rack's offset amount;
/// rack.get gives it per channel and per feel.
#[test]
fn rack_offset_sets_one_drum_classes_offset_amount() {
    let mut r = Rig::new();
    r.app.project = crate::channel_rack::picture_project(5);
    let mut offsets = vec![0.0; 16];
    offsets[4] = 0.04;
    r.call(
        "rack.groove",
        json!({"groove": {"default": {"swing": 50, "offsets": offsets}}}),
    );
    let rack = r.call("rack.offset", json!({"class": "snare", "pct": 50}));
    assert_eq!(rack["channels"][1]["offset"], json!(50.0));
    assert_eq!(rack["channels"][0]["offset"], json!(100.0));
    assert_eq!(rack["groove"]["classes"]["snare"]["offset"], json!(50.0));
    assert!(rack["moved"].as_u64().unwrap() > 0);
    // The snare's step 5 plays half its offset.
    let starts = rack["channels"][1]["starts"].as_array().unwrap();
    assert_eq!(starts[0], json!(1.02));
    let rack = r.call("rack.offset", json!({"class": "all", "pct": 0}));
    assert!(rack["channels"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["offset"] == json!(0.0)));
    assert_eq!(
        r.err("rack.offset", json!({"class": "tuba", "pct": 10}))
            .code,
        Code::InvalidParams
    );
}
