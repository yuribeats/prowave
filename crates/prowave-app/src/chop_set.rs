//! Pad sets: a song's pads prepared ahead of time (an extension: the SP-303 has nothing like
//! it), kept per song as `<chops root>/<song file stem>/set.json`. Save Pads as Its Chops (the
//! song's menu in the Sampler's browser) writes the pads as they are; one click on the song
//! then puts the whole set back on the pads.
//!
//! The format (`"format": "prowave-pad-set"`, version 1):
//!
//! ```json
//! {"format": "prowave-pad-set", "version": 1, "song": "Song", "bpm": 112.4,
//!  "pads": [{"name": "Chorus 8b", "file": "chorus-8b.flac", "start": 0, "end": 0,
//!            "gate": false, "loop": true, "bpm": 112.6, "bars": 8, "confidence": 0.9}]}
//! ```
//!
//! `file` is relative to the set's folder or absolute; `start`/`end` are frames of the file
//! (`end` 0: its end). The song's stems sit in `<stems root>/<song file stem>/*.flac`, listed by
//! the browser under the song so each can be loaded on its own.
use crate::sampler::{add_source, Pad, SamplerState, PADS};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "prowave-pad-set";
pub const SET_FILE: &str = "set.json";

/// One pad of a set.
#[derive(Clone, Debug, PartialEq)]
pub struct SetPad {
    pub name: String,
    /// Absolute once read.
    pub file: String,
    pub start: u64,
    pub end: u64,
    pub gate: bool,
    pub looping: bool,
    /// The loop's own tempo (its local bar grid), so Fit to tempo and Follow work.
    pub bpm: Option<f64>,
    pub bars: Option<f64>,
    pub confidence: Option<f64>,
}

/// A song's pads.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PadSet {
    pub song: String,
    pub bpm: Option<f64>,
    pub pads: Vec<SetPad>,
}

/// A separated part of a song, loadable on its own.
#[derive(Clone, Debug, PartialEq)]
pub struct StemFile {
    pub name: String,
    pub path: PathBuf,
}

fn app_dir(sub: &str) -> Option<PathBuf> {
    crate::recent_files::config_dir().map(|d| d.join(sub))
}

/// Where the sets live (`chops` in the settings folder).
pub fn chops_root() -> Option<PathBuf> {
    app_dir("chops")
}

/// Where the songs' stems live.
pub fn stems_root() -> Option<PathBuf> {
    app_dir("stems")
}

fn song_key(song: &Path) -> Option<String> {
    song.file_stem().map(|s| s.to_string_lossy().into_owned())
}

/// The set file of `song` under `root` (it may not exist).
pub fn set_path(root: &Path, song: &Path) -> Option<PathBuf> {
    Some(root.join(song_key(song)?).join(SET_FILE))
}

fn finite(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

/// Reads a set from its JSON; relative files are taken from `dir`.
pub fn parse(text: &str, dir: &Path) -> Result<PadSet, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if v["format"].as_str() != Some(FORMAT) {
        return Err("not a pad set".into());
    }
    if v["version"].as_u64().unwrap_or(0) != 1 {
        return Err(format!("pad set version {} not known", v["version"]));
    }
    let list = v["pads"].as_array().ok_or("a pad set without pads")?;
    let mut pads = Vec::new();
    for p in list.iter().take(PADS) {
        let Some(file) = p["file"].as_str().filter(|f| !f.is_empty()) else {
            continue;
        };
        let path = Path::new(file);
        let file = if path.is_absolute() {
            file.to_string()
        } else {
            dir.join(path).to_string_lossy().into_owned()
        };
        let start = p["start"].as_u64().unwrap_or(0);
        let end = p["end"].as_u64().unwrap_or(0);
        pads.push(SetPad {
            name: p["name"].as_str().unwrap_or_default().to_string(),
            file,
            start,
            end: if end != 0 && end <= start { 0 } else { end },
            gate: p["gate"].as_bool().unwrap_or(false),
            looping: p["loop"].as_bool().unwrap_or(false),
            bpm: finite(&p["bpm"]).filter(|x| (20.0..=400.0).contains(x)),
            bars: finite(&p["bars"]).filter(|x| *x > 0.0),
            confidence: finite(&p["confidence"]).map(|x| x.clamp(0.0, 1.0)),
        });
    }
    if pads.is_empty() {
        return Err("a pad set without pads".into());
    }
    Ok(PadSet {
        song: v["song"].as_str().unwrap_or_default().to_string(),
        bpm: finite(&v["bpm"]).filter(|x| *x > 0.0),
        pads,
    })
}

/// The set's JSON; files inside `dir` are written relative to it.
pub fn to_json(set: &PadSet, dir: &Path) -> String {
    let pads: Vec<Value> = set
        .pads
        .iter()
        .map(|p| {
            let file = Path::new(&p.file)
                .strip_prefix(dir)
                .map_or(p.file.clone(), |r| r.to_string_lossy().into_owned());
            json!({
                "name": p.name, "file": file, "start": p.start, "end": p.end,
                "gate": p.gate, "loop": p.looping, "bpm": p.bpm, "bars": p.bars,
                "confidence": p.confidence,
            })
        })
        .collect();
    serde_json::to_string_pretty(&json!({
        "format": FORMAT, "version": 1, "song": set.song, "bpm": set.bpm, "pads": pads,
    }))
    .unwrap_or_default()
}

pub fn read(path: &Path) -> Result<PadSet, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse(&text, path.parent().unwrap_or(Path::new("")))
}

pub fn write(set: &PadSet, path: &Path) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new(""));
    std::fs::create_dir_all(dir)?;
    std::fs::write(path, to_json(set, dir))
}

/// The set of `song` under `root`, if one has been made.
pub fn find(root: &Path, song: &Path) -> Option<PadSet> {
    let path = set_path(root, song)?;
    path.is_file().then(|| read(&path).ok()).flatten()
}

/// The pads the sampler has now, as a set (to save them for `song`).
pub fn from_state(s: &SamplerState, song: &str) -> PadSet {
    let pads = s
        .pads
        .iter()
        .filter_map(|p| {
            let src = s.sources.get(p.source?)?;
            Some(SetPad {
                name: p.name.clone(),
                file: src.path.clone(),
                start: p.start,
                end: p.end,
                gate: p.gate,
                looping: p.looping,
                bpm: p.bpm.or(src.bpm),
                bars: None,
                confidence: None,
            })
        })
        .collect();
    PadSet {
        song: song.to_string(),
        bpm: s.sources.first().and_then(|x| x.bpm),
        pads,
    }
}

/// Puts the set on the pads, in order from A1: every pad and source is replaced, the rest of
/// the settings (effect, volume, patterns' settings) stay. Returns how many pads it filled.
pub fn apply(set: &PadSet, s: &mut SamplerState) -> usize {
    s.sources.clear();
    s.pads = vec![Pad::default(); PADS];
    s.chromatic = None;
    let mut n = 0;
    for (k, p) in set.pads.iter().take(PADS).enumerate() {
        let src = add_source(s, &p.file);
        if s.sources[src].bpm.is_none() {
            s.sources[src].bpm = p.bpm.or(set.bpm);
        }
        s.pads[k] = Pad {
            source: Some(src),
            name: p.name.clone(),
            start: p.start,
            end: p.end,
            gate: p.gate,
            looping: p.looping,
            bpm: p.bpm,
            ..Pad::default()
        };
        n += 1;
    }
    n
}

/// How a stem file is called in the browser.
pub fn stem_name(key: &str) -> String {
    match key {
        "vocals" => "Vocals (lead + backing)",
        "lead" => "Lead Vocal",
        "backing" => "Backing Vocals",
        "instrumental" => "Instrumental",
        "drums" => "Drums",
        "bass" => "Bass",
        "guitar" => "Guitar",
        "piano" => "Piano / Keys",
        "other" => "Other",
        "horns" => "Horns (wind instruments)",
        "other_rest" => "Other without Horns",
        "rhythm" => "Rhythm Section (drums, bass, guitar, keys)",
        "orchestra" => "Orchestra (horns + other)",
        "kick" => "Kick",
        "snare" => "Snare",
        "toms" => "Toms",
        "hh" => "Hi-Hat",
        "ride" => "Ride",
        "crash" => "Crash",
        other => other,
    }
    .to_string()
}

const STEM_ORDER: [&str; 19] = [
    "vocals",
    "lead",
    "backing",
    "instrumental",
    "rhythm",
    "orchestra",
    "drums",
    "bass",
    "guitar",
    "piano",
    "horns",
    "other",
    "other_rest",
    "kick",
    "snare",
    "toms",
    "hh",
    "ride",
    "crash",
];

/// The stems of `song` under `root`: the known ones in a fixed order, then any others by name.
pub fn stems_of(root: &Path, song: &Path) -> Vec<StemFile> {
    let Some(dir) = song_key(song).map(|k| root.join(k)) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<(usize, String, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| crate::sampler::is_sample_file(p))
        .filter_map(|p| {
            let key = p.file_stem()?.to_string_lossy().into_owned();
            let rank = STEM_ORDER
                .iter()
                .position(|x| *x == key)
                .unwrap_or(STEM_ORDER.len());
            Some((rank, key, p))
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .map(|(_, key, path)| StemFile {
            name: stem_name(&key),
            path,
        })
        .collect()
}

/// The next bar line after `beats` (quarter notes), for a set loaded while the song plays.
pub fn next_bar(beats: f64, bar_beats: f64) -> f64 {
    let bar = bar_beats.max(0.25);
    ((beats / bar).floor() + 1.0) * bar
}

/// Whether a set waiting for `at` is due: the playhead reached it, jumped back (a cycle
/// wrapped), or stopped.
pub fn due(at: f64, last: f64, now: f64, playing: bool) -> bool {
    !playing || now >= at - 1e-9 || now + 1e-9 < last
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::sampler::{assign, decode, encode};

    fn example() -> String {
        json!({
            "format": FORMAT, "version": 1, "song": "Hard to Be in Love", "bpm": 112.4,
            "pads": [
                {"name": "Full Track", "file": "/records/song.mp3", "gate": true, "loop": false,
                 "bpm": 112.4},
                {"name": "Instrumental Chorus 8b", "file": "inst-chorus-8b.flac", "loop": true,
                 "bpm": 112.6, "bars": 8, "confidence": 0.93},
                {"name": "Chorus 4b", "file": "chorus-4b.flac", "start": 10, "end": 5,
                 "loop": true, "bpm": 999.0},
                {"name": "no file"},
            ]
        })
        .to_string()
    }

    #[test]
    fn a_set_reads_with_its_files_found_next_to_it() {
        let set = parse(&example(), Path::new("/chops/song")).unwrap();
        assert_eq!(set.song, "Hard to Be in Love");
        assert_eq!(set.pads.len(), 3, "the pad without a file is skipped");
        assert_eq!(set.pads[0].file, "/records/song.mp3");
        assert!(set.pads[0].gate && !set.pads[0].looping);
        assert_eq!(set.pads[1].file, "/chops/song/inst-chorus-8b.flac");
        assert_eq!(set.pads[1].bars, Some(8.0));
        assert_eq!(set.pads[1].confidence, Some(0.93));
        // An end before the start means the file's end; a tempo out of range is dropped.
        assert_eq!((set.pads[2].start, set.pads[2].end), (10, 0));
        assert_eq!(set.pads[2].bpm, None);
        assert!(parse("{}", Path::new("/")).is_err());
        assert!(parse(
            r#"{"format": "prowave-pad-set", "version": 2, "pads": []}"#,
            Path::new("/")
        )
        .is_err());
    }

    #[test]
    fn a_set_saves_and_reads_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let set_file = dir.path().join("Song").join(SET_FILE);
        let set = parse(&example(), &dir.path().join("Song")).unwrap();
        write(&set, &set_file).unwrap();
        let text = std::fs::read_to_string(&set_file).unwrap();
        assert!(
            text.contains(r#""file": "inst-chorus-8b.flac""#),
            "relative inside its folder"
        );
        assert_eq!(read(&set_file).unwrap(), set);
    }

    #[test]
    fn the_pads_now_save_as_a_set_and_load_back() {
        let mut s = SamplerState::default();
        assign(&mut s, 0, "/records/a.wav", 0, 0, 44_100);
        assign(&mut s, 5, "/records/b.wav", 100, 4100, 44_100);
        s.pads[5].looping = true;
        s.pads[5].bpm = Some(95.0);
        let set = from_state(&s, "a");
        assert_eq!(set.pads.len(), 2);
        let mut t = SamplerState::default();
        assert_eq!(apply(&set, &mut t), 2);
        assert_eq!(t.pads[1].start, 100);
        assert!(t.pads[1].looping);
        assert_eq!(t.pads[1].bpm, Some(95.0));
    }

    #[test]
    fn applying_a_set_replaces_every_pad_and_keeps_the_rest() {
        let set = parse(&example(), Path::new("/chops/song")).unwrap();
        let mut s = SamplerState::default();
        for k in 0..PADS {
            assign(&mut s, k, &format!("/old/{k}.wav"), 0, 100, 44_100);
        }
        s.volume = 0.5;
        s.chromatic = Some(3);
        assert_eq!(apply(&set, &mut s), 3);
        assert_eq!(s.sources.len(), 3, "the old files are gone");
        assert!(s.pads[3..].iter().all(Pad::is_empty));
        assert_eq!(s.pads[0].name, "Full Track");
        assert!(s.pads[0].gate);
        assert!(s.pads[1].looping && !s.pads[1].gate);
        assert_eq!(s.pads[1].bpm, Some(112.6));
        assert_eq!(s.sources[s.pads[1].source.unwrap()].bpm, Some(112.6));
        // A pad without its own tempo takes the song's.
        assert_eq!(s.sources[s.pads[2].source.unwrap()].bpm, Some(112.4));
        assert_eq!(s.volume, 0.5);
        assert_eq!(s.chromatic, None);
        // And it survives the slot's save.
        assert_eq!(decode(&Some(encode(&s))), s);
    }

    #[test]
    fn a_set_longer_than_the_pads_fills_all_of_them() {
        let pads: Vec<Value> = (0..PADS + 10)
            .map(|k| json!({"name": format!("p{k}"), "file": format!("{k}.flac"), "loop": true}))
            .collect();
        let text = json!({"format": FORMAT, "version": 1, "pads": pads}).to_string();
        let set = parse(&text, Path::new("/c")).unwrap();
        let mut s = SamplerState::default();
        assert_eq!(apply(&set, &mut s), PADS);
        assert_eq!(s.pads[PADS - 1].name, format!("p{}", PADS - 1));
    }

    #[test]
    fn songs_find_their_set_and_stems_by_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let (chops, stems) = (dir.path().join("chops"), dir.path().join("stems"));
        let song = Path::new("/records/Artist - 01 Song.mp3");
        assert!(find(&chops, song).is_none(), "chops not ready");
        let set = parse(&example(), Path::new("/c")).unwrap();
        write(&set, &set_path(&chops, song).unwrap()).unwrap();
        assert_eq!(find(&chops, song).unwrap().pads.len(), 3);
        let sd = stems.join("Artist - 01 Song");
        std::fs::create_dir_all(&sd).unwrap();
        for f in [
            "kick.flac",
            "vocals.flac",
            "drums.flac",
            "zzz.flac",
            "meta.json",
        ] {
            std::fs::write(sd.join(f), b"x").unwrap();
        }
        let names: Vec<String> = stems_of(&stems, song).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["Vocals (lead + backing)", "Drums", "Kick", "zzz"]);
        assert!(stems_of(&stems, Path::new("/records/other.mp3")).is_empty());
    }

    /// The app's decoder and the analysis tools must agree on frame numbers when a pad points
    /// into a song: prints the decoded length and where the first sounds are, for comparing
    /// with `soundfile`. `PROWAVE_DECODE_CHECK=file cargo test -p prowave-app decode_offsets
    /// -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn decode_offsets() {
        let Ok(path) = std::env::var("PROWAVE_DECODE_CHECK") else {
            return;
        };
        let b = prowave_engine::sample_tools::decode(Path::new(&path), |_| true).unwrap();
        let first: Vec<usize> = (0..b.frames())
            .filter(|i| b.data[i * 2].abs() > 0.05)
            .take(3)
            .collect();
        let probe: Vec<String> = [100_000usize, 1_000_000]
            .iter()
            .map(|i| format!("{:.6}", b.data[i * 2]))
            .collect();
        eprintln!(
            "frames {} rate {} first {first:?} probe {probe:?}",
            b.frames(),
            b.rate
        );
    }

    #[test]
    fn a_set_loaded_while_playing_waits_for_the_next_bar() {
        assert_eq!(next_bar(0.0, 4.0), 4.0);
        assert_eq!(next_bar(5.5, 4.0), 8.0);
        assert_eq!(next_bar(8.0, 4.0), 12.0);
        assert_eq!(next_bar(1.0, 3.0), 3.0);
        assert!(!due(8.0, 5.5, 7.9, true));
        assert!(due(8.0, 7.9, 8.0, true));
        assert!(due(8.0, 7.9, 0.1, true), "the cycle wrapped");
        assert!(due(8.0, 5.5, 6.0, false), "stopped: at once");
    }
}
