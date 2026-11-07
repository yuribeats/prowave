//! The Scripter MIDI FX (Logic Pro: Scripter): a short program that changes a track's notes.
//! The language is Rhai, which has no access to files, the network or anything else of the
//! computer: a script can only look at the notes it is given and hand notes back. It is also
//! held to a budget of operations, so a script that never ends is stopped instead of freezing
//! the app.
//!
//! A script defines one of two functions:
//!
//! ```text
//! // Called for every note. Give back the note, an array of notes, or () to drop it.
//! fn process_note(n) {
//!     n.pitch += 12;
//!     n
//! }
//!
//! // Or called once with all the notes of the track, as an array.
//! fn process(notes) {
//!     notes.filter(|n| n.vel > 40)
//! }
//! ```
//!
//! A note is a map with `start` and `end` (beats from the start of the song, numbers with a
//! fraction), `pitch` and `vel` (whole numbers). Pitch and velocity are brought back into
//! range, and a note whose end is not after its start is dropped.
use prowave_engine::NoteEvent;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Scope, AST};
use std::cell::Cell;
use std::rc::Rc;

/// Longest script, in characters.
pub const MAX_SCRIPT_CHARS: usize = 20_000;
/// Operations the whole run of a script may make (every note it is called for together).
const MAX_OPERATIONS: u64 = 20_000_000;
/// Notes a script may hand back.
const MAX_NOTES_OUT: usize = 1_000_000;
/// The longest a run of a script may take, whatever it does (a script that fills an array
/// without end is slow long before it uses an unreasonable amount of memory).
const MAX_TIME: std::time::Duration = std::time::Duration::from_millis(1500);

/// A script to start from.
pub const DEFAULT_SCRIPT: &str = "\
// Called for every note. Give back the note, an array of notes, or () to drop it.
// A note has start and end (beats), pitch and vel.
fn process_note(n) {
    n.pitch += 12; // up an octave
    n
}
";

fn engine(spent: Rc<Cell<u64>>, notes: usize) -> Engine {
    let mut e = Engine::new();
    e.set_max_call_levels(32);
    e.set_max_expr_depths(64, 32);
    // Room for the notes it is given (twice over) and a little more.
    e.set_max_array_size((notes * 2 + 1024).min(MAX_NOTES_OUT));
    let began = std::time::Instant::now();
    e.set_max_map_size(64);
    e.set_max_string_size(1_000);
    e.set_max_variables(256);
    // One budget for the whole run, not one for each call.
    e.on_progress(move |_| {
        spent.set(spent.get() + 1);
        let n = spent.get();
        let late = n.is_multiple_of(256) && began.elapsed() > MAX_TIME;
        (n > MAX_OPERATIONS || late).then(|| "the script took too long".into())
    });
    e
}

/// What went wrong in a script, in words.
fn describe(e: &EvalAltResult) -> String {
    match e {
        EvalAltResult::ErrorTerminated(..) => "the script took too long".into(),
        other => other.to_string(),
    }
}

fn compile(e: &Engine, source: &str) -> Result<AST, String> {
    if source.chars().count() > MAX_SCRIPT_CHARS {
        return Err(format!(
            "the script is longer than {MAX_SCRIPT_CHARS} characters"
        ));
    }
    let ast = e.compile(source).map_err(|err| err.to_string())?;
    let has = |name: &str, args: usize| {
        ast.iter_functions()
            .any(|f| f.name == name && f.params.len() == args)
    };
    if !has("process_note", 1) && !has("process", 1) {
        return Err("define fn process_note(n) or fn process(notes)".into());
    }
    Ok(ast)
}

fn to_map(n: &NoteEvent) -> Dynamic {
    let mut m = Map::new();
    m.insert("start".into(), Dynamic::from(n.start_beat));
    m.insert("end".into(), Dynamic::from(n.end_beat));
    m.insert("pitch".into(), Dynamic::from(i64::from(n.pitch)));
    m.insert("vel".into(), Dynamic::from(i64::from(n.vel)));
    Dynamic::from(m)
}

/// A number from a script as a float, whichever way it was written.
fn number(d: &Dynamic) -> Option<f64> {
    d.as_float()
        .ok()
        .or_else(|| d.as_int().ok().map(|i| i as f64))
        .filter(|f| f.is_finite())
}

/// The note a script's value stands for, on `track`; `None` for what is not a usable note.
fn from_map(d: &Dynamic, track: u16) -> Option<NoteEvent> {
    let m = d.read_lock::<Map>()?;
    let get = |k: &str| m.get(k).and_then(number);
    let (start, end) = (get("start")?, get("end")?);
    let (pitch, vel) = (get("pitch")?, get("vel")?);
    (start >= 0.0 && end > start).then(|| NoteEvent {
        start_beat: start,
        end_beat: end,
        pitch: pitch.round().clamp(0.0, 127.0) as u8,
        vel: vel.round().clamp(1.0, 127.0) as u8,
        track,
    })
}

/// Notes from what a script returned: one note, an array of them, or nothing.
fn collect(out: &mut Vec<NoteEvent>, value: &Dynamic, track: u16) {
    if let Some(list) = value.read_lock::<Array>() {
        for item in list.iter() {
            if out.len() >= MAX_NOTES_OUT {
                return;
            }
            out.extend(from_map(item, track));
        }
    } else if out.len() < MAX_NOTES_OUT {
        out.extend(from_map(value, track));
    }
}

/// Runs `source` on `notes` (absolute beats). `Err` says what is wrong with the script.
pub fn run(source: &str, notes: &[NoteEvent]) -> Result<Vec<NoteEvent>, String> {
    let spent = Rc::new(Cell::new(0));
    let e = engine(spent, notes.len());
    let ast = compile(&e, source)?;
    let mut scope = Scope::new();
    let whole = ast
        .iter_functions()
        .any(|f| f.name == "process" && f.params.len() == 1);
    let mut out = Vec::new();
    let track = notes.first().map_or(0, |n| n.track);
    if whole {
        let list: Array = notes.iter().map(to_map).collect();
        let back: Dynamic = e
            .call_fn(&mut scope, &ast, "process", (list,))
            .map_err(|err| describe(&err))?;
        collect(&mut out, &back, track);
    } else {
        for n in notes {
            let back: Dynamic = e
                .call_fn(&mut scope, &ast, "process_note", (to_map(n),))
                .map_err(|err| describe(&err))?;
            collect(&mut out, &back, n.track);
            if out.len() >= MAX_NOTES_OUT {
                break;
            }
        }
    }
    Ok(out)
}

/// Whether `source` is a script that works: it compiles and runs on a few notes.
pub fn check(source: &str) -> Result<(), String> {
    let sample = [
        NoteEvent {
            start_beat: 0.0,
            end_beat: 1.0,
            pitch: 60,
            vel: 100,
            track: 0,
        },
        NoteEvent {
            start_beat: 1.0,
            end_beat: 2.5,
            pitch: 127,
            vel: 1,
            track: 0,
        },
    ];
    run(source, &sample).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: f64, end: f64, pitch: u8, vel: u8) -> NoteEvent {
        NoteEvent {
            start_beat: start,
            end_beat: end,
            pitch,
            vel,
            track: 4,
        }
    }

    #[test]
    fn the_default_script_moves_every_note_up_an_octave() {
        assert_eq!(check(DEFAULT_SCRIPT), Ok(()));
        let out = run(
            DEFAULT_SCRIPT,
            &[note(0.0, 1.0, 60, 90), note(1.0, 2.0, 120, 90)],
        )
        .unwrap();
        // 120 + 12 is out of range: the note stops at the top, it is not dropped.
        assert_eq!(out.iter().map(|n| n.pitch).collect::<Vec<_>>(), [72, 127]);
    }

    #[test]
    fn notes_come_back_clamped_and_a_script_can_drop_split_and_add() {
        let up = "fn process_note(n) { n.pitch += 12; n }";
        let out = run(up, &[note(0.0, 1.0, 60, 90), note(1.0, 2.0, 120, 90)]).unwrap();
        assert_eq!(out.iter().map(|n| n.pitch).collect::<Vec<_>>(), [72, 127]);
        assert!(out.iter().all(|n| n.track == 4), "the track stays");
        let drop = "fn process_note(n) { if n.vel < 50 { () } else { n } }";
        let out = run(drop, &[note(0.0, 1.0, 60, 30), note(1.0, 2.0, 62, 80)]).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pitch, 62);
        let echo = "fn process_note(n) { let e = n; e.start += 0.5; e.end += 0.5; e.vel = n.vel / 2; [n, e] }";
        let out = run(echo, &[note(0.0, 0.5, 60, 100)]).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(
            (out[1].start_beat, out[1].end_beat, out[1].vel),
            (0.5, 1.0, 50)
        );
        // A note that ends before it starts, or starts before the song, is dropped.
        let bad = "fn process_note(n) { n.end = n.start - 1.0; n }";
        assert!(run(bad, &[note(1.0, 2.0, 60, 90)]).unwrap().is_empty());
        let early = "fn process_note(n) { n.start = -1.0; n }";
        assert!(run(early, &[note(1.0, 2.0, 60, 90)]).unwrap().is_empty());
        // Whole numbers where a fraction is expected, and wild velocities, are accepted.
        let ints = "fn process_note(n) { #{ start: 2, end: 3, pitch: 300, vel: -5 } }";
        let out = run(ints, &[note(0.0, 1.0, 60, 90)]).unwrap();
        assert_eq!((out[0].start_beat, out[0].pitch, out[0].vel), (2.0, 127, 1));
        // Something that is not a note is ignored.
        let junk = "fn process_note(n) { [1, \"two\", #{ start: 0.0 }, ()] }";
        assert!(run(junk, &[note(0.0, 1.0, 60, 90)]).unwrap().is_empty());
    }

    #[test]
    fn a_script_can_see_all_the_notes_at_once() {
        let s = "fn process(notes) { notes.filter(|n| n.pitch > 60) }";
        let out = run(s, &[note(0.0, 1.0, 60, 90), note(1.0, 2.0, 64, 90)]).unwrap();
        assert_eq!(out.iter().map(|n| n.pitch).collect::<Vec<_>>(), [64]);
        let rev = "fn process(notes) { let out = []; for n in notes { n.start += 1.0; n.end += 1.0; out.push(n); } out }";
        let out = run(rev, &[note(0.0, 1.0, 60, 90)]).unwrap();
        assert_eq!(out[0].start_beat, 1.0);
        // With no notes there is nothing to hand back.
        assert!(run(s, &[]).unwrap().is_empty());
    }

    #[test]
    fn broken_scripts_say_what_is_wrong() {
        assert!(
            check("fn process_note(n) { n.pitch += ; }").is_err(),
            "a typo"
        );
        assert!(check("let x = 1;")
            .unwrap_err()
            .contains("define fn process_note"));
        assert!(
            check("fn process_note(n) { n.nothing.deeper }").is_err(),
            "a runtime error"
        );
        assert!(check("fn process_note(n) { undefined_function(n) }").is_err());
        assert!(
            check("fn process_note(a, b) { a }").is_err(),
            "needs one argument"
        );
        assert!(check(&"x".repeat(MAX_SCRIPT_CHARS + 1))
            .unwrap_err()
            .contains("longer"));
        assert!(check("").is_err());
    }

    #[test]
    fn a_script_that_never_ends_is_stopped() {
        let began = std::time::Instant::now();
        let err = check("fn process_note(n) { loop { } }").unwrap_err();
        assert!(err.contains("too long"), "{err}");
        let err = check("fn process_note(n) { let a = []; loop { a.push(1); } }").unwrap_err();
        assert!(
            err.contains("too long") || err.to_lowercase().contains("size"),
            "{err}"
        );
        let err = check("fn process_note(n) { process_note(n) }").unwrap_err();
        assert!(
            err.to_lowercase().contains("stack")
                || err.contains("level")
                || err.contains("too long"),
            "{err}"
        );
        // The budget is shared by all the notes, not given again to each.
        let many: Vec<NoteEvent> = (0..2_000)
            .map(|i| note(i as f64, i as f64 + 1.0, 60, 90))
            .collect();
        let slow = "fn process_note(n) { let i = 0; while i < 100000 { i += 1; } n }";
        assert!(run(slow, &many).unwrap_err().contains("too long"));
        assert!(began.elapsed().as_secs() < 30, "{:?}", began.elapsed());
    }

    #[test]
    fn a_script_cannot_reach_files_or_the_system() {
        for s in [
            "import \"std\" as s; fn process_note(n) { n }",
            "fn process_note(n) { open(\"C:/x\") }",
            "fn process_note(n) { system(\"calc\") }",
        ] {
            assert!(check(s).is_err(), "{s}");
        }
    }
}
