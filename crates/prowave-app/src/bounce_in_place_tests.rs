//! Bounce in Place and Join into a new file, without a window or a sound card: the files are
//! what the bounce's engine plays, the regions land where the sources were, and the project
//! changes in one undo step.
//!
//! Everything is at 48 kHz and 120 BPM in 4/4: a beat is 24000 frames, a bar 96000.
#![allow(clippy::unwrap_used)]
use super::*;
use crate::model::{AudioRegion, Insert, Region, RegionParams, SendSlot, Track};
use crate::plugins::PluginHost;
use crate::ui_test::Harness;
use egui::{Event, Key, Modifiers};
use std::path::Path;
use std::time::Duration;

const SR: u32 = 48_000;
const BAR: usize = 96_000;

/// Sound no two stretches of which are alike, within +-0.5, first frame not silent.
fn noise(frames: usize, seed: u32) -> Vec<f32> {
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(frames * 2);
    for _ in 0..frames * 2 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        out.push((x as f32 / u32::MAX as f32) - 0.5);
    }
    out[0] = 0.4;
    out[1] = -0.3;
    out
}

fn write(dir: &Path, name: &str, stereo: &[f32]) -> prowave_engine::AudioFile {
    let path = dir.join(name);
    prowave_engine::write_wav(&path, stereo, SR).unwrap();
    prowave_engine::AudioFile::open(&path).unwrap()
}

fn read(path: &Path) -> Vec<f32> {
    prowave_engine::AudioFile::open(path)
        .unwrap()
        .read_stereo()
        .unwrap()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

fn difference(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).fold(0.0, |m, (x, y)| m.max((x - y).abs()))
}

fn audio_region(name: &str, file: prowave_engine::AudioFile, bar: f32, bars: f32) -> Region {
    Region {
        missing: None,
        name: name.into(),
        start_bar: bar,
        len_bars: bars,
        notes: Vec::new(),
        controls: Vec::new(),
        folder: None,
        audio: Some(AudioRegion {
            file,
            peaks: Default::default(),
            flex: None,
        }),
        params: RegionParams::default(),
        generated: None,
    }
}

/// An audio track "Audio" with region "A" (`one`, bar 2) and "B" (`two`, bar 4), each a bar
/// long, through a Delay, its fader at -6 dB and pan left of centre, a send to Aux 1; the
/// Stereo Out at half with a Compressor. None of the Stereo Out is in a Bounce in Place.
fn project(dir: &Path, one: &[f32], two: &[f32]) -> Project {
    let mut p = Project::empty();
    p.tempo = 120.0;
    p.tracks.clear();
    p.master_vol = 0.5;
    p.master_inserts
        .push(Insert::new(prowave_engine::FxKind::Compressor));
    let mut t = Track::new("Audio", TrackKind::Audio);
    t.regions
        .push(audio_region("A", write(dir, "one.wav", one), 2.0, 1.0));
    t.regions
        .push(audio_region("B", write(dir, "two.wav", two), 4.0, 1.0));
    t.inserts.push(Insert::new(prowave_engine::FxKind::Delay));
    t.volume_db = -6.0;
    t.pan = -0.5;
    t.sends = vec![SendSlot {
        bus: Some(1),
        level: 0.8,
        pre: false,
    }];
    p.tracks.push(t);
    p.add_track(TrackKind::Aux);
    p.selected_track = 0;
    p
}

/// Runs `req` to its end on this thread.
fn run(p: &mut Project, req: &Request, dir: &Path, view: &mut TracksView) -> Result<Done, String> {
    let mut run = Run::start(p, req, SR, dir)?;
    let mut host = PluginHost::default();
    let began = std::time::Instant::now();
    loop {
        if let Some(r) = run.poll(p, &mut host, false, None, view) {
            return r;
        }
        assert!(began.elapsed() < Duration::from_secs(120));
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn request(p: &Project, what: What, options: Options, sel: &[(usize, usize)]) -> Request {
    Request {
        what,
        options,
        pieces: pieces_of(p, what, sel),
    }
}

/// The file of the region at (`t`, `r`).
fn file_of(p: &Project, t: usize, r: usize) -> PathBuf {
    p.tracks[t].regions[r]
        .audio
        .as_ref()
        .unwrap()
        .file
        .path
        .clone()
}

#[test]
fn regions_in_place_null_against_a_direct_bounce_and_land_on_a_new_track_at_their_bar() {
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    let (one, two) = (noise(BAR, 3), noise(BAR, 5));
    let mut p = project(dir.path(), &one, &two);
    let before = p.clone();
    let mut s = crate::UiState::new();
    s.tracks.select_many(&[(0, 0)]);
    let req = request(&p, What::Regions, Options::default(), &s.tracks.selection());
    assert_eq!(req.pieces, vec![(0, vec![0])]);
    let done = run(&mut p, &req, &media, &mut s.tracks).unwrap();
    assert!(done.report.not_rendered.is_empty(), "{:?}", done.report);

    // A new audio track under the source, with the fader, pan and send of the source.
    assert_eq!(p.tracks.len(), 3);
    let new = &p.tracks[1];
    assert_eq!(
        (new.name.as_str(), new.kind),
        ("Audio_bip", TrackKind::Audio)
    );
    assert_eq!((new.volume_db, new.pan), (-6.0, -0.5));
    assert_eq!(new.sends, before.tracks[0].sends);
    assert!(new.inserts.is_empty() && new.au_effects.is_empty());
    assert_eq!(p.tracks[2].kind, TrackKind::Aux);
    // Its one region starts at bar 2, as "A" does, and is selected.
    assert_eq!(new.regions.len(), 1);
    assert_eq!(new.regions[0].start_bar, 2.0);
    assert_eq!(done.made, vec![(1, 0)]);
    assert_eq!(s.tracks.selection(), vec![(1, 0)]);
    // The source is left as it was.
    assert_eq!(p.tracks[0].regions.len(), 2);
    assert!(!p.tracks[0].regions[0].params.mute);
    // The file is in the media folder, as long as the region says.
    let path = file_of(&p, 1, 0);
    assert!(path.starts_with(&media));
    let got = read(&path);
    let frames = got.len() / 2;
    let bars = frames as f32 / BAR as f32;
    assert!((new.regions[0].len_bars - bars).abs() < 1.0e-4);
    // The Delay rings on after the region: there is a tail.
    assert!(frames > BAR + SR as usize / 2, "{frames} frames");

    // A direct bounce of the same range of the same track, with only "A" on it, out of its
    // own strip (fader at 0 dB, centre, no send, nothing on the Stereo Out): the same file.
    let mut q = before.clone();
    q.tracks[0].regions.remove(1);
    q.tracks[0].volume_db = 0.0;
    q.tracks[0].pan = 0.0;
    q.tracks[0].sends.clear();
    q.master_vol = 1.0;
    q.master_inserts.clear();
    let direct = dir.path().join("direct.wav");
    let settings = Settings {
        range: Some((4.0, 8.0)),
        depth: Depth::Float32,
        tail_seconds: 0.0,
        max_tail_seconds: 20.0,
        only: Some(0),
        ..Settings::mix_to(&direct)
    };
    bounce::run_blocking(&mut q, &mut PluginHost::default(), SR, settings).unwrap();
    let want = read(&direct);
    assert_eq!(got.len(), want.len());
    let diff = difference(&got, &want);
    assert!(diff < 1.0e-6, "differs from the direct bounce by {diff}");
    // And it is the Delay's sound, not the file's.
    assert!(difference(&got[..BAR * 2], &one) > 1.0e-3);
}

#[test]
fn a_bounce_in_place_is_one_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = project(dir.path(), &noise(BAR, 7), &noise(BAR, 9));
    let mut history = crate::undo::History::new(&p);
    let mut s = crate::UiState::new();
    s.tracks.select_many(&[(0, 0), (0, 1)]);
    let options = Options {
        source: Source::Mute,
        ..Options::default()
    };
    let req = request(&p, What::Regions, options, &s.tracks.selection());
    run(&mut p, &req, &dir.path().join("media"), &mut s.tracks).unwrap();
    assert_eq!(p.tracks.len(), 3);
    assert!(p.tracks[0].regions.iter().all(|r| r.params.mute));
    // Both regions in one file: one region from bar 2 to past bar 5.
    assert_eq!(p.tracks[1].regions.len(), 1);
    assert_eq!(p.tracks[1].regions[0].start_bar, 2.0);
    assert!(p.tracks[1].regions[0].len_bars >= 3.0);
    assert!(history.record(&p));
    assert!(history.undo(&mut p));
    assert_eq!(p.tracks.len(), 2);
    assert!(p.tracks[0].regions.iter().all(|r| !r.params.mute));
    assert!(!history.can_undo(), "one step held all of it");
}

#[test]
fn bypass_and_include_automation_change_what_is_in_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let one = noise(BAR, 11);
    let mut p = project(dir.path(), &one, &noise(BAR, 13));
    let mut s = crate::UiState::new();
    // Bypass, no tail: the region as its file plays, to the sample.
    let options = Options {
        bypass: true,
        tail: false,
        ..Options::default()
    };
    let req = request(&p, What::Regions, options, &[(0, 0)]);
    run(&mut p, &req, &dir.path().join("m1"), &mut s.tracks).unwrap();
    let got = read(&file_of(&p, 1, 0));
    assert_eq!(got.len(), BAR * 2);
    assert!(difference(&got, &one) < 2.0e-7);
    assert_eq!(p.tracks[1].regions[0].len_bars, 1.0);

    // Include Volume/Pan Automation: the fader and pan are in the file and the new track is
    // at 0 dB, centre (hard left here, so the right side is silent).
    let mut p = project(dir.path(), &one, &noise(BAR, 13));
    p.tracks[0].pan = -1.0;
    let options = Options {
        bypass: true,
        tail: false,
        automation: true,
        ..Options::default()
    };
    let req = request(&p, What::Regions, options, &[(0, 0)]);
    run(&mut p, &req, &dir.path().join("m2"), &mut s.tracks).unwrap();
    assert_eq!((p.tracks[1].volume_db, p.tracks[1].pan), (0.0, 0.0));
    let got = read(&file_of(&p, 1, 0));
    let gain = 10f32.powf(-6.0 / 20.0) * std::f32::consts::SQRT_2;
    let (mut left, mut right) = (0f32, 0f32);
    for (g, x) in got.as_chunks::<2>().0.iter().zip(one.as_chunks::<2>().0) {
        left = left.max((g[0] - x[0] * gain).abs());
        right = right.max(g[1].abs());
    }
    assert!(left < 1.0e-6 && right < 1.0e-6, "{left} {right}");
}

#[test]
fn replace_puts_the_dry_file_where_the_regions_were() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (noise(BAR, 15), noise(BAR, 17));
    let mut p = project(dir.path(), &one, &two);
    let mut s = crate::UiState::new();
    let pieces = pieces_of(&p, What::Regions, &[(0, 0)]);
    assert!(can_replace(&p, &pieces));
    let options = Options {
        destination: Destination::Replace,
        ..Options::default()
    };
    let req = request(&p, What::Regions, options, &[(0, 0)]);
    let done = run(&mut p, &req, &dir.path().join("media"), &mut s.tracks).unwrap();
    assert_eq!(p.tracks.len(), 2, "no new track");
    assert_eq!(done.made, vec![(0, 0)]);
    let t = &p.tracks[0];
    assert_eq!(t.regions.len(), 2);
    assert_eq!(
        (t.regions[0].name.as_str(), t.regions[0].start_bar),
        ("Audio_bip", 2.0)
    );
    assert_eq!(t.regions[1].name, "B");
    // The strip still has its Delay and fader, so they are not in the file: it is "A" as
    // its file plays.
    assert_eq!(t.inserts.len(), 1);
    let got = read(&file_of(&p, 0, 0));
    assert!(difference(&got[..BAR * 2], &one) < 2.0e-7);
    assert_eq!(peak(&got[BAR * 2..]), 0.0);
    // An instrument track's regions cannot be replaced by audio: not offered, and refused.
    let mut q = Project::empty();
    q.tracks.clear();
    let mut inst = Track::new("Inst", TrackKind::Instrument);
    inst.regions.push(Region {
        audio: None,
        ..audio_region("M", write(dir.path(), "x.wav", &one), 1.0, 1.0)
    });
    q.tracks.push(inst);
    let pieces = pieces_of(&q, What::Regions, &[(0, 0)]);
    assert!(!can_replace(&q, &pieces));
    let req = Request {
        what: What::Regions,
        options: Options {
            destination: Destination::Replace,
            ..Options::default()
        },
        pieces,
    };
    assert!(Run::start(&q, &req, SR, &dir.path().join("q")).is_err());
}

#[test]
fn join_bounces_audio_regions_of_two_files_into_one() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (noise(BAR, 19), noise(BAR, 21));
    let mut p = project(dir.path(), &one, &two);
    let mut s = crate::UiState::new();
    s.tracks.select_many(&[(0, 0), (0, 1)]);
    // Not pieces of one file: Join cannot put them together by itself...
    assert!(!crate::region_edit::can_join(&p, &s.tracks));
    // ...but the menu entry and its key work, and ask for a bounce.
    assert!(crate::menus::enabled("Join", &p, &s));
    crate::menus::apply("Join", &mut p, &mut s);
    let req = s.bounce.in_place_go.take().expect("a bounce is asked for");
    assert_eq!(req.what, What::Join);
    assert_eq!(req.pieces, vec![(0, vec![0, 1])]);
    assert_eq!(
        p.tracks[0].regions.len(),
        2,
        "nothing changes until it is done"
    );
    let mut history = crate::undo::History::new(&p);
    let done = run(&mut p, &req, &dir.path().join("media"), &mut s.tracks).unwrap();
    // One region from bar 2 to the end of bar 4, named as the first, on the same track.
    assert_eq!(p.tracks.len(), 2);
    let t = &p.tracks[0];
    assert_eq!(t.regions.len(), 1);
    assert_eq!(done.made, vec![(0, 0)]);
    assert_eq!(s.tracks.selection(), vec![(0, 0)]);
    assert_eq!(
        (
            t.regions[0].name.as_str(),
            t.regions[0].start_bar,
            t.regions[0].len_bars
        ),
        ("A", 2.0, 3.0)
    );
    // The regions as they play, without the strip (Delay, fader, pan): A, a bar of silence, B.
    let got = read(&file_of(&p, 0, 0));
    assert_eq!(got.len(), 3 * BAR * 2);
    assert!(difference(&got[..BAR * 2], &one) < 2.0e-7);
    assert_eq!(peak(&got[BAR * 2..BAR * 4]), 0.0);
    assert!(difference(&got[BAR * 4..], &two) < 2.0e-7);
    assert_eq!(t.inserts.len(), 1, "the strip keeps its effect");
    assert!(history.record(&p));
    assert!(history.undo(&mut p));
    assert_eq!(p.tracks[0].regions.len(), 2);

    // Pieces of one file are still joined at once, without a bounce.
    let mut p = project(dir.path(), &one, &two);
    p.position_beats = 6.0;
    let mut s = crate::UiState::new();
    s.tracks.select_many(&[(0, 0)]);
    crate::menus::apply("Split at Playhead", &mut p, &mut s);
    s.tracks.select_many(&[(0, 0), (0, 1)]);
    assert!(crate::region_edit::can_join(&p, &s.tracks));
    crate::menus::apply("Join", &mut p, &mut s);
    assert!(s.bounce.in_place_go.is_none());
    assert_eq!(p.tracks[0].regions.len(), 2);
}

#[test]
fn tracks_in_place_bounces_every_region_of_the_selected_track_and_mutes_it() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (noise(BAR, 23), noise(BAR, 25));
    let mut p = project(dir.path(), &one, &two);
    let mut s = crate::UiState::new();
    p.selected_track = 0;
    let options = Options {
        source: Source::Mute,
        bypass: true,
        tail: false,
        ..Options::default()
    };
    let req = request(&p, What::Tracks, options, &[]);
    assert_eq!(req.pieces, vec![(0, vec![0, 1])]);
    run(&mut p, &req, &dir.path().join("media"), &mut s.tracks).unwrap();
    assert!(p.tracks[0].mute, "the source track is muted");
    assert!(p.tracks[0].regions.iter().all(|r| !r.params.mute));
    let new = &p.tracks[1];
    assert_eq!(new.regions.len(), 1);
    assert_eq!(
        (new.regions[0].start_bar, new.regions[0].len_bars),
        (2.0, 3.0)
    );
    let got = read(&file_of(&p, 1, 0));
    assert!(difference(&got[..BAR * 2], &one) < 2.0e-7);
    assert!(difference(&got[BAR * 4..], &two) < 2.0e-7);
    // The track's own Mute is not in the file: a muted track is bounced as it would sound.
    let mut s2 = crate::UiState::new();
    let req = request(&p, What::Tracks, Options::default(), &[]);
    assert!(run(&mut p, &req, &dir.path().join("m2"), &mut s2.tracks).is_ok());
    assert!(peak(&read(&file_of(&p, 1, 0))) > 0.1);
}

#[test]
fn a_project_changed_during_the_bounce_is_left_alone_and_no_file_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = project(dir.path(), &noise(BAR, 27), &noise(BAR, 29));
    let mut s = crate::UiState::new();
    let req = request(&p, What::Regions, Options::default(), &[(0, 0)]);
    let media = dir.path().join("media");
    let mut r = Run::start(&p, &req, SR, &media).unwrap();
    // The region moves while it is being bounced.
    p.tracks[0].regions[0].start_bar = 6.0;
    let mut host = PluginHost::default();
    let result = loop {
        if let Some(x) = r.poll(&mut p, &mut host, false, None, &mut s.tracks) {
            break x;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(result.unwrap_err().contains("changed"));
    assert_eq!(p.tracks.len(), 2);
    assert_eq!(std::fs::read_dir(&media).unwrap().count(), 0);
}

/// An instrument (the Channel Rack playing a one-shot) bounced to audio: the new track has
/// the hit at its bar, and the source regions are muted.
#[cfg(target_os = "macos")]
#[test]
fn an_instrument_track_s_regions_become_audio() {
    use crate::model::Note;
    let dir = tempfile::tempdir().unwrap();
    let hit: Vec<f32> = noise(SR as usize / 10, 31)
        .iter()
        .map(|x| x * 0.4)
        .collect();
    let hit_file = write(dir.path(), "hit.wav", &hit);
    let mut p = Project::empty();
    p.tempo = 120.0;
    p.tracks.clear();
    let mut t = Track::new("Rack", TrackKind::Instrument);
    let mut slot = crate::channel_rack::new_slot();
    slot.state = Some(crate::channel_rack::encode(&[
        crate::channel_rack::RackChannel {
            path: hit_file.path.to_string_lossy().into_owned(),
            name: "hit".into(),
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            class: crate::kit_classify::DrumClass::Other,
        },
    ]));
    t.au_instrument = Some(slot);
    t.regions.push(Region {
        audio: None,
        notes: vec![Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start: 0.0,
            len: 0.5,
            pitch: prowave_engine::RACK_BASE_NOTE,
            vel: 127,
        }],
        ..audio_region("Hits", hit_file.clone(), 3.0, 1.0)
    });
    p.tracks.push(t);
    let mut s = crate::UiState::new();
    let options = Options {
        source: Source::Mute,
        tail: false,
        ..Options::default()
    };
    let req = request(&p, What::Regions, options, &[(0, 0)]);
    let done = run(&mut p, &req, &dir.path().join("media"), &mut s.tracks).unwrap();
    assert!(done.report.not_rendered.is_empty(), "{:?}", done.report);
    assert!(p.tracks[0].regions[0].params.mute);
    assert_eq!(p.tracks[1].kind, TrackKind::Audio);
    assert_eq!(p.tracks[1].regions[0].start_bar, 3.0);
    let got = read(&file_of(&p, 1, 0));
    assert_eq!(got.len(), BAR * 2);
    // The hit, on the region's first frame, and silence once it is over.
    assert!(got[0] != 0.0 || got[1] != 0.0);
    assert!(difference(&got[..hit.len()], &hit) < 1.0e-4);
    assert_eq!(peak(&got[hit.len() * 2..]), 0.0);
}

/// One frame as the app runs it: the menus' view keys, then the key commands, then the
/// bounce dialog.
fn app_frame(h: &mut Harness, p: &mut Project, s: &mut crate::UiState, events: Vec<Event>) {
    h.frame(events, |ui| {
        let ctx = ui.ctx().clone();
        crate::key_commands::begin_frame(&ctx);
        crate::menus::handle_keys(&ctx, p, s);
        crate::key_commands::handle(&ctx, p, s);
        show_dialog(&ctx, p, s);
    });
}

fn press(h: &mut Harness, p: &mut Project, s: &mut crate::UiState, key: Key, mods: Modifiers) {
    h.modifiers = mods;
    let ev = |pressed| Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: mods,
    };
    app_frame(h, p, s, vec![ev(true), ev(false)]);
    h.modifiers = Modifiers::NONE;
}

fn ctrl_cmd() -> Modifiers {
    Modifiers {
        ctrl: true,
        mac_cmd: cfg!(target_os = "macos"),
        command: true,
        ..Modifiers::NONE
    }
}

#[test]
fn control_b_opens_regions_in_place_and_ok_asks_for_the_bounce() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = project(dir.path(), &noise(BAR, 1), &noise(BAR, 2));
    let mut s = crate::UiState::new();
    let mut h = Harness::new(1200.0, 800.0);
    // Nothing selected: greyed, and the key does nothing.
    assert!(!crate::menus::enabled("Regions in Place…", &p, &s));
    press(&mut h, &mut p, &mut s, Key::B, Modifiers::CTRL);
    assert!(s.bounce.in_place_dialog.is_none());
    s.tracks.select_many(&[(0, 1)]);
    assert!(crate::menus::enabled("Regions in Place…", &p, &s));
    assert!(crate::menus::implemented("Regions in Place…"));
    press(&mut h, &mut p, &mut s, Key::B, Modifiers::CTRL);
    let d = s
        .bounce
        .in_place_dialog
        .clone()
        .expect("the dialog is open");
    assert_eq!(d.what, What::Regions);
    // While it is open the other keys wait: L does not loop the region.
    press(&mut h, &mut p, &mut s, Key::L, Modifiers::NONE);
    assert_eq!(p.tracks[0].regions[1].params.loop_beats, 0.0);
    // OK asks for the bounce of the selected region, and closes the dialog.
    app_frame(&mut h, &mut p, &mut s, vec![]);
    let ok = h
        .ctx
        .read_response(
            egui::Id::unique("prowave_bounce_in_place")
                .with("keys")
                .with(1usize),
        )
        .expect("OK is there")
        .rect
        .center();
    h.time_passes(1.0);
    h.click(ok, |ui| {
        let ctx = ui.ctx().clone();
        show_dialog(&ctx, &p, &mut s);
    });
    assert!(s.bounce.in_place_dialog.is_none());
    let req = s.bounce.in_place_go.take().expect("asked for");
    assert_eq!((req.what, req.pieces), (What::Regions, vec![(0, vec![1])]));
    assert_eq!(req.options, Options::default());
    // A running bounce greys the entry.
    let run = Run::start(
        &p,
        &request(&p, What::Regions, Options::default(), &[(0, 1)]),
        SR,
        &dir.path().join("media"),
    )
    .unwrap();
    s.bounce.in_place = Some(run);
    assert!(!crate::menus::enabled("Regions in Place…", &p, &s));
    s.bounce.in_place = None;
}

#[test]
fn control_command_b_opens_tracks_in_place_from_the_selected_track() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = project(dir.path(), &noise(BAR, 1), &noise(BAR, 2));
    let mut s = crate::UiState::new();
    let mut h = Harness::new(1200.0, 800.0);
    p.selected_track = 1;
    // The Aux has no regions.
    assert!(!crate::menus::enabled("Tracks in Place…", &p, &s));
    p.selected_track = 0;
    assert!(crate::menus::enabled("Tracks in Place…", &p, &s));
    press(&mut h, &mut p, &mut s, Key::B, ctrl_cmd());
    let d = s
        .bounce
        .in_place_dialog
        .clone()
        .expect("the dialog is open");
    assert_eq!(d.what, What::Tracks);
    // The menu entry opens it too.
    s.bounce.in_place_dialog = None;
    crate::menus::apply("Tracks in Place…", &mut p, &mut s);
    assert_eq!(s.bounce.in_place_dialog.map(|d| d.what), Some(What::Tracks));
}

/// Region `r` of track 0 as it plays on its track without the strip (muted or not), rendered
/// by the bounce's engine into `path`.
fn region_alone(p: &Project, r: usize, path: &Path) -> Vec<f32> {
    let mut q = p.clone();
    let reg = &q.tracks[0].regions[r];
    let range = (q.region_start_beats(reg), q.region_loop_end_beats(reg));
    let settings = Settings {
        range: Some(range),
        depth: Depth::Float32,
        tail_seconds: 0.0,
        max_tail_seconds: 0.0,
        only: Some(0),
        strip: Some(Strip {
            track: 0,
            regions: vec![r],
            effects: false,
            fader: false,
            mute_automation: false,
            unmute_regions: true,
        }),
        ..Settings::mix_to(path)
    };
    bounce::run_blocking(&mut q, &mut PluginHost::default(), SR, settings).unwrap();
    read(path)
}

/// "A" with gain, fades and Reverse and its own colour; "B" muted and looped once.
fn convert_rig(dir: &Path, one: &[f32], two: &[f32]) -> Project {
    let mut p = project(dir, one, two);
    let a = &mut p.tracks[0].regions[0].params;
    a.gain_db = -6.0;
    a.fade_in = 1.0;
    a.fade_out = 0.5;
    a.reverse = true;
    a.color = Some(crate::theme::TRACK_PALETTE[4]);
    let b = &mut p.tracks[0].regions[1].params;
    b.mute = true;
    b.loop_beats = 4.0;
    p
}

#[test]
fn convert_makes_a_file_per_region_that_plays_as_the_region_did_in_its_place() {
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    let (one, two) = (noise(BAR, 41), noise(BAR, 43));
    let mut p = convert_rig(dir.path(), &one, &two);
    let before = p.clone();
    let want_a = region_alone(&before, 0, &dir.path().join("a_direct.wav"));
    let want_b = region_alone(&before, 1, &dir.path().join("b_direct.wav"));
    let mut s = crate::UiState::new();
    s.tracks.select_many(&[(0, 0), (0, 1)]);
    let req = request(&p, What::Convert, Options::default(), &s.tracks.selection());
    assert_eq!(
        req.pieces,
        vec![(0, vec![0]), (0, vec![1])],
        "one file each"
    );
    let mut history = crate::undo::History::new(&p);
    let done = run(&mut p, &req, &media, &mut s.tracks).unwrap();
    assert!(done.report.not_rendered.is_empty(), "{:?}", done.report);
    assert_eq!(done.made, vec![(0, 0), (0, 1)]);
    assert_eq!(s.tracks.selection(), vec![(0, 0), (0, 1)]);

    // In place: no new track, the strip as it was, the regions where they were.
    assert_eq!(p.tracks.len(), 2);
    let t = &p.tracks[0];
    assert_eq!(t.inserts.len(), 1);
    assert_eq!(t.regions.len(), 2);
    let (a, b) = (&t.regions[0], &t.regions[1]);
    assert_eq!((a.name.as_str(), a.start_bar), ("A", 2.0));
    assert_eq!((b.name.as_str(), b.start_bar), ("B", 4.0));
    // The loop is in B's file: the region is two bars long and loops no more.
    assert!((a.len_bars - 1.0).abs() < 1.0e-4, "{}", a.len_bars);
    assert!((b.len_bars - 2.0).abs() < 1.0e-4, "{}", b.len_bars);
    // What the files hold is gone from the regions; colour and mute stay with them.
    assert!(
        a.params
            == RegionParams {
                color: Some(crate::theme::TRACK_PALETTE[4]),
                ..RegionParams::default()
            }
    );
    assert!(b.params.mute && b.params.loop_beats == 0.0);
    // Two new files in the media folder, named after the regions.
    let (fa, fb) = (file_of(&p, 0, 0), file_of(&p, 0, 1));
    assert!(fa.starts_with(&media) && fb.starts_with(&media));
    assert_eq!(fa.file_name().unwrap(), "A.wav");
    assert_eq!(fb.file_name().unwrap(), "B.wav");

    // Each file nulls against a direct bounce of its region as it played.
    let (got_a, got_b) = (read(&fa), read(&fb));
    assert_eq!(got_a.len(), want_a.len());
    assert_eq!(got_b.len(), want_b.len());
    assert!(difference(&got_a, &want_a) < 1.0e-6);
    assert!(difference(&got_b, &want_b) < 1.0e-6);
    // A's file is A as it played (reversed, 6 dB down, faded), not its old file...
    assert!(difference(&got_a, &one) > 1.0e-2);
    assert!((peak(&got_a) / peak(&one) - 0.501).abs() < 0.01);
    assert_eq!(
        (got_a[0], got_a[1]),
        (0.0, 0.0),
        "the fade in starts at silence"
    );
    // ...and B's holds its sound although B is muted, twice over for the loop.
    assert!(difference(&got_b[..BAR * 2], &two) < 2.0e-7);
    assert!(difference(&got_b[BAR * 2..], &two) < 2.0e-7);

    // The new regions play what the old ones did.
    let now_a = region_alone(&p, 0, &dir.path().join("a_after.wav"));
    assert!(difference(&now_a, &want_a) < 1.0e-6);

    // One undo step gives the old regions back.
    assert!(history.record(&p));
    assert!(history.undo(&mut p));
    assert!(p.tracks[0].regions[0].params == before.tracks[0].regions[0].params);
    assert_eq!(file_of(&p, 0, 1), file_of(&before, 0, 1));
    assert!(!history.can_undo());
}

#[test]
fn option_command_f_converts_the_selected_audio_regions_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = convert_rig(dir.path(), &noise(BAR, 45), &noise(BAR, 47));
    // A MIDI region on an instrument track is not converted.
    let mut inst = Track::new("Inst", TrackKind::Instrument);
    inst.regions.push(Region {
        audio: None,
        ..audio_region("M", write(dir.path(), "m.wav", &noise(BAR, 1)), 1.0, 1.0)
    });
    p.tracks.push(inst);
    let label = "Convert Regions to New Audio Files";
    let mut s = crate::UiState::new();
    let mut h = Harness::new(1200.0, 800.0);
    // In Edit > Convert.
    assert!(crate::menus::implemented(label));
    assert_eq!(crate::menus::submenu("Convert"), Some(&[label][..]));
    assert!(crate::menus::MENUS
        .iter()
        .any(|(m, items)| *m == "Edit" && items.contains(&"Convert")));
    let alt_cmd = Modifiers {
        alt: true,
        mac_cmd: cfg!(target_os = "macos"),
        command: true,
        ..Modifiers::NONE
    };
    // Nothing selected, or only the MIDI region: greyed, and the key does nothing.
    assert!(!crate::menus::enabled(label, &p, &s));
    press(&mut h, &mut p, &mut s, Key::F, alt_cmd);
    assert!(s.bounce.in_place_go.is_none());
    s.tracks.select_many(&[(2, 0)]);
    assert!(!crate::menus::enabled(label, &p, &s));
    // The key asks for one file per selected audio region; the MIDI region is left out.
    s.tracks.select_many(&[(0, 1), (2, 0), (0, 0)]);
    assert!(crate::menus::enabled(label, &p, &s));
    press(&mut h, &mut p, &mut s, Key::F, alt_cmd);
    let req = s.bounce.in_place_go.take().expect("asked for");
    assert_eq!(req.what, What::Convert);
    assert_eq!(req.pieces, vec![(0, vec![0]), (0, vec![1])]);
    assert_eq!(p.tracks[0].regions[0].params.gain_db, -6.0, "nothing yet");
    // The menu asks for the same; a running bounce greys it.
    crate::menus::apply(label, &mut p, &mut s);
    assert_eq!(s.bounce.in_place_go.take(), Some(req.clone()));
    s.bounce.in_place = Some(Run::start(&p, &req, SR, &dir.path().join("media")).unwrap());
    assert!(!crate::menus::enabled(label, &p, &s));
    s.bounce.in_place.take().unwrap().cancel();
}

/// A Flex-edited region plays the file its Flex edit made, not its recording: Convert writes
/// that sound, and the new region has no Flex edit left to apply.
#[test]
fn convert_writes_a_flex_edited_region_as_it_plays() {
    use crate::flex_edit::FlexEdit;
    use prowave_engine::FlexMode;
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    let (flexed, recording) = (noise(BAR, 51), noise(BAR, 53));
    let mut p = project(dir.path(), &flexed, &noise(BAR, 55));
    let original = write(dir.path(), "recording.wav", &recording);
    let mut f = FlexEdit::new(original, FlexMode::Polyphonic);
    f.rendered = f.fingerprint();
    p.tracks[0].regions[0].audio.as_mut().unwrap().flex = Some(f);
    let want = region_alone(&p, 0, &dir.path().join("direct.wav"));
    assert!(
        difference(&want, &flexed) < 2.0e-7,
        "it plays the Flex render"
    );
    let mut s = crate::UiState::new();
    s.tracks.select_only(0, 0);
    let req = request(&p, What::Convert, Options::default(), &s.tracks.selection());
    run(&mut p, &req, &media, &mut s.tracks).unwrap();
    let a = &p.tracks[0].regions[0];
    assert!(
        a.audio.as_ref().unwrap().flex.is_none(),
        "the Flex edit is in the file"
    );
    let got = read(&file_of(&p, 0, 0));
    assert!(difference(&got, &want) < 1.0e-6);
    assert!(difference(&got, &recording) > 1.0e-2, "not the recording");
}
