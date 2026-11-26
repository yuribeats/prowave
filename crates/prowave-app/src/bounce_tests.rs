//! Bounces of small projects, without a window or a sound card: what is written is what the
//! project plays, to the sample where that can be said.
//!
//! Everything is at 48 kHz and 120 BPM in 4/4: a beat is 24000 frames, a bar 96000.
#![allow(clippy::unwrap_used)]
use super::*;
use crate::model::{AudioRegion, AutoTarget, Insert, Note, Region, RegionParams, SendSlot, Track};
use crate::plugins::PluginHost;
use std::path::Path;

const SR: u32 = 48_000;
const BEAT: usize = 24_000;
const BAR: usize = 4 * BEAT;

/// A sound of `frames` frames no two stretches of which are alike: each sample is its own
/// number, left and right different, within +-0.5, and the first frame is not silent.
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

fn region(name: &str, bar: f32, bars: f32) -> Region {
    Region {
        missing: None,
        name: name.into(),
        start_bar: bar,
        len_bars: bars,
        notes: Vec::new(),
        controls: Vec::new(),
        folder: None,
        audio: None,
        params: RegionParams::default(),
        generated: None,
    }
}

fn audio_region(file: prowave_engine::AudioFile, bar: f32, bars: f32) -> Region {
    Region {
        audio: Some(AudioRegion {
            file,
            peaks: Default::default(),
            flex: None,
        }),
        ..region("Audio", bar, bars)
    }
}

/// A project at 120 BPM whose one track is an audio track playing `file` for a bar from
/// bar `bar`.
fn audio_project(file: prowave_engine::AudioFile, bar: f32) -> Project {
    let mut p = Project::empty();
    p.tempo = 120.0;
    // The Stereo Out's fader at 0 dB (a new project has it lower).
    p.master_vol = 1.0;
    p.tracks.clear();
    let mut t = Track::new("Audio", TrackKind::Audio);
    t.regions.push(audio_region(file, bar, 1.0));
    p.tracks.push(t);
    p
}

fn settings(path: &Path) -> Settings {
    Settings {
        depth: Depth::Float32,
        tail_seconds: 0.5,
        max_tail_seconds: 0.5,
        ..Settings::mix_to(path)
    }
}

fn bounce(p: &mut Project, s: Settings) -> Report {
    run_blocking(p, &mut PluginHost::default(), SR, s).unwrap()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// The samples of frames `from..to`.
fn frames(x: &[f32], from: usize, to: usize) -> &[f32] {
    &x[from * 2..to * 2]
}

/// The largest difference between `a` and `b`, sample by sample.
fn difference(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).fold(0.0, |m, (x, y)| m.max((x - y).abs()))
}

#[test]
fn an_audio_only_project_nulls_against_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let source = noise(BAR, 7);
    let file = write(dir.path(), "source.wav", &source);
    let mut p = audio_project(file, 2.0);
    let out = dir.path().join("mix.wav");
    let r = bounce(&mut p, settings(&out));
    assert!(r.not_rendered.is_empty(), "{:?}", r.not_rendered);
    assert_eq!(r.files.len(), 1);
    // Bars 1 and 2, and half a second of tail.
    assert_eq!(r.files[0].frames as usize, 2 * BAR + SR as usize / 2);
    assert_eq!((r.from, r.to, r.range_seconds), (0.0, 8.0, 4.0));
    let got = read(&out);
    assert_eq!(got.len(), (2 * BAR + SR as usize / 2) * 2);
    // Nothing before the region, to the sample: its first frame is the file's first.
    assert_eq!(peak(frames(&got, 0, BAR)), 0.0, "silence before the region");
    assert!((got[BAR * 2] - source[0]).abs() < 1.0e-6);
    let diff = difference(frames(&got, BAR, 2 * BAR), &source);
    assert!(diff < 2.0e-7, "the region differs from its file by {diff}");
    assert_eq!(peak(&got[2 * BAR * 2..]), 0.0, "silence after the region");
    assert!((r.files[0].peak - peak(&source)).abs() < 1.0e-6);
    assert!(r
        .rendered
        .iter()
        .any(|(i, name, _)| *i == 0 && name == "Audio"));
    // The project itself is as it was: still at its start, not playing.
    assert!(!p.playing && p.position_beats == 0.0);

    // The strip's fader and pan and the Stereo Out's fader are in it: 6 dB down on the
    // track, hard left, and the Stereo Out at a half.
    p.tracks[0].volume_db = -6.0;
    p.tracks[0].pan = -1.0;
    p.master_vol = 0.5;
    let out = dir.path().join("turned.wav");
    bounce(&mut p, settings(&out));
    let got = read(&out);
    let region = frames(&got, BAR, 2 * BAR);
    let gain = 10f32.powf(-6.0 / 20.0) * std::f32::consts::SQRT_2 * 0.5;
    let (mut left, mut right) = (0f32, 0f32);
    for (g, s) in region
        .as_chunks::<2>()
        .0
        .iter()
        .zip(source.as_chunks::<2>().0)
    {
        left = left.max((g[0] - s[0] * gain).abs());
        right = right.max(g[1].abs());
    }
    assert!(left < 1.0e-6, "the left channel is off by {left}");
    assert!(right < 1.0e-6, "hard left leaves {right} on the right");
}

#[test]
fn a_bounce_done_twice_gives_the_same_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write(dir.path(), "source.wav", &noise(BAR, 3));
    let mut p = audio_project(file, 1.0);
    // Effects with memory and a send to an Aux with an echo.
    p.tracks[0]
        .inserts
        .push(Insert::new(prowave_engine::FxKind::Compressor));
    let aux = p.add_track(TrackKind::Aux);
    p.tracks[aux]
        .inserts
        .push(Insert::new(prowave_engine::FxKind::Delay));
    p.tracks[0].sends = vec![SendSlot {
        bus: Some(1),
        level: 0.8,
        pre: false,
    }];
    p.tracks[0].auto.set_point(AutoTarget::Pan, 0.0, -1.0);
    p.tracks[0].auto.set_point(AutoTarget::Pan, 4.0, 1.0);
    let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
    let mut s = settings(&a);
    s.depth = Depth::Pcm24;
    s.tail_seconds = 1.0;
    s.max_tail_seconds = 20.0;
    bounce(&mut p, s.clone());
    s.path = b.clone();
    bounce(&mut p, s);
    let (x, y) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert!(x.len() > 44 + BAR * 6);
    assert!(x == y, "two bounces of one project differ");
}

#[test]
fn mute_solo_loops_and_the_cycle_range_are_as_in_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (noise(BAR, 11), noise(BAR, 23));
    let mut p = audio_project(write(dir.path(), "one.wav", &one), 1.0);
    let mut t = Track::new("Second", TrackKind::Audio);
    t.regions
        .push(audio_region(write(dir.path(), "two.wav", &two), 2.0, 1.0));
    p.tracks.push(t);
    let path = |n: &str| dir.path().join(n);

    // Both: bar 1 is the first file, bar 2 the second.
    bounce(&mut p, settings(&path("both.wav")));
    let got = read(&path("both.wav"));
    assert!(difference(frames(&got, 0, BAR), &one) < 2.0e-7);
    assert!(difference(frames(&got, BAR, 2 * BAR), &two) < 2.0e-7);

    // Mute: the muted track is not in the file, and the result says it is muted, not missing.
    p.tracks[0].mute = true;
    let r = bounce(&mut p, settings(&path("muted.wav")));
    let got = read(&path("muted.wav"));
    assert_eq!(peak(frames(&got, 0, BAR)), 0.0);
    assert!(difference(frames(&got, BAR, 2 * BAR), &two) < 2.0e-7);
    assert_eq!(r.muted, vec!["Audio".to_string()]);
    assert!(r.not_rendered.is_empty());
    p.tracks[0].mute = false;

    // Solo: only the soloed track.
    p.tracks[0].solo = true;
    let r = bounce(&mut p, settings(&path("solo.wav")));
    let got = read(&path("solo.wav"));
    assert!(difference(frames(&got, 0, BAR), &one) < 2.0e-7);
    assert_eq!(peak(frames(&got, BAR, 2 * BAR)), 0.0);
    assert_eq!(r.muted, vec!["Second".to_string()]);
    p.tracks[0].solo = false;

    // A loop: the first region again in bars 2 and 3, on top of the second track in bar 2.
    p.tracks[0].regions[0].params.loop_beats = 8.0;
    p.tracks[1].mute = true;
    let r = bounce(&mut p, settings(&path("loop.wav")));
    let got = read(&path("loop.wav"));
    assert_eq!(r.files[0].frames as usize, 3 * BAR + SR as usize / 2);
    for bar in 0..3 {
        let diff = difference(frames(&got, bar * BAR, (bar + 1) * BAR), &one);
        assert!(diff < 2.0e-7, "repeat {bar} differs by {diff}");
    }
    p.tracks[0].regions[0].params.loop_beats = 0.0;
    p.tracks[1].mute = false;

    // Cycle on: its range, from its start. Bar 2 alone is the second file.
    p.cycle = true;
    p.cycle_start = 4.0;
    p.cycle_end = 8.0;
    let r = bounce(&mut p, settings(&path("cycle.wav")));
    let got = read(&path("cycle.wav"));
    assert_eq!((r.from, r.to), (4.0, 8.0));
    assert_eq!(r.files[0].frames as usize, BAR + SR as usize / 2);
    assert!(difference(frames(&got, 0, BAR), &two) < 2.0e-7);
    // A range given wins over the cycle: beats 2 to 6, the first file from its middle.
    let mut s = settings(&path("range.wav"));
    s.range = Some((2.0, 6.0));
    bounce(&mut p, s);
    let got = read(&path("range.wav"));
    assert!(difference(frames(&got, 0, 2 * BEAT), frames(&one, 2 * BEAT, BAR)) < 2.0e-7);
    assert!(difference(frames(&got, 2 * BEAT, BAR), frames(&two, 0, 2 * BEAT)) < 2.0e-7);
    assert!(p.cycle, "the project's own Cycle is left on");
}

#[test]
fn files_are_written_in_each_format_and_normalized_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let source: Vec<f32> = noise(BAR, 5).iter().map(|x| x * 0.5).collect();
    let mut p = audio_project(write(dir.path(), "source.wav", &source), 1.0);
    let cases = [
        ("a16.wav", Format::Wav, Depth::Pcm16, 1.0 / 32_767.0),
        ("a24.wav", Format::Wav, Depth::Pcm24, 1.0 / 8_388_607.0),
        ("a32.wav", Format::Wav, Depth::Float32, 2.0e-7),
        ("a16.aiff", Format::Aiff, Depth::Pcm16, 1.0 / 32_767.0),
        ("a24.aiff", Format::Aiff, Depth::Pcm24, 1.0 / 8_388_607.0),
    ];
    for (name, format, depth, step) in cases {
        let out = dir.path().join(name);
        let mut s = settings(&out);
        s.format = format;
        s.depth = depth;
        let r = bounce(&mut p, s);
        assert_eq!(r.format, format!("{}, {}", format.name(), depth.name()));
        let file = prowave_engine::AudioFile::open(&out).unwrap();
        assert_eq!((file.sample_rate, file.channels), (SR, 2), "{name}");
        assert_eq!(file.frames as usize, BAR + SR as usize / 2, "{name}");
        let got = file.read_stereo().unwrap();
        let diff = difference(frames(&got, 0, BAR), &source);
        assert!(diff <= step, "{name}: off by {diff}");
    }
    // Normalize: the loudest sample at full scale, the rest in proportion.
    let out = dir.path().join("loud.wav");
    let mut s = settings(&out);
    s.normalize = true;
    let r = bounce(&mut p, s);
    let gain = 1.0 / peak(&source);
    assert!((r.files[0].gain - gain).abs() < 1.0e-4);
    assert!((r.files[0].peak - 1.0).abs() < 1.0e-6);
    let got = read(&out);
    assert!((peak(&got) - 1.0).abs() < 1.0e-6);
    let scaled: Vec<f32> = source.iter().map(|x| x * r.files[0].gain).collect();
    assert!(difference(frames(&got, 0, BAR), &scaled) < 1.0e-6);
    // An AIFF file has no float samples.
    let mut s = settings(&dir.path().join("f.aiff"));
    s.format = Format::Aiff;
    assert!(Job::start(&p, SR, s).is_err());
}

#[test]
fn a_file_that_is_there_is_kept_unless_replacing_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = audio_project(write(dir.path(), "source.wav", &noise(BEAT, 5)), 1.0);
    let out = dir.path().join("mix.wav");
    std::fs::write(&out, b"mine").unwrap();
    let e = run_blocking(&mut p, &mut PluginHost::default(), SR, settings(&out)).unwrap_err();
    assert!(e.contains("already there"), "{e}");
    assert_eq!(std::fs::read(&out).unwrap(), b"mine");
    let mut s = settings(&out);
    s.overwrite = true;
    bounce(&mut p, s);
    assert_eq!(&std::fs::read(&out).unwrap()[..4], b"RIFF");
    // Nothing else is left beside it.
    let left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
    // A project with nothing in it has nothing to bounce, and says so.
    let mut empty = Project::empty();
    let e = run_blocking(
        &mut empty,
        &mut PluginHost::default(),
        SR,
        settings(&dir.path().join("none.wav")),
    )
    .unwrap_err();
    assert!(e.contains("nothing to bounce"), "{e}");
}

#[test]
fn a_cancelled_bounce_leaves_no_file() {
    let dir = tempfile::tempdir().unwrap();
    // Long enough that it is still rendering when it is cancelled.
    let mut p = audio_project(write(dir.path(), "source.wav", &noise(BAR, 5)), 1.0);
    p.tracks[0].regions[0].params.loop_beats = 4.0 * 2000.0;
    let out = dir.path().join("mix.wav");
    let mut job = Job::start(&p, SR, settings(&out)).unwrap();
    let mut host = PluginHost::default();
    let began = Instant::now();
    let result = loop {
        if let Some(r) = job.poll(&mut p, &mut host, false) {
            break r;
        }
        if !job.progress().preparing {
            job.cancel();
        }
        assert!(began.elapsed() < Duration::from_secs(120));
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(result.unwrap_err(), "cancelled");
    let left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, vec!["source.wav".to_string()]);
}

#[test]
fn the_app_s_own_work_is_waited_for_before_the_project_is_taken() {
    let dir = tempfile::tempdir().unwrap();
    let source = noise(BAR, 9);
    let mut p = audio_project(write(dir.path(), "source.wav", &source), 1.0);
    let out = dir.path().join("mix.wav");
    let mut job = Job::start(&p, SR, settings(&out)).unwrap();
    let mut host = PluginHost::default();
    // While the app is busy (Flex is making a region's sound) nothing is taken...
    for _ in 0..20 {
        assert!(job.poll(&mut p, &mut host, true).is_none());
    }
    assert!(job.progress().preparing);
    // ...so a change the app makes meanwhile is in the bounce: the region moves to bar 2.
    p.tracks[0].regions[0].start_bar = 2.0;
    let r = loop {
        if let Some(r) = job.poll(&mut p, &mut host, false) {
            break r.unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(r.files[0].frames as usize, 2 * BAR + SR as usize / 2);
    let got = read(&out);
    assert_eq!(peak(frames(&got, 0, BAR)), 0.0);
    assert!(difference(frames(&got, BAR, 2 * BAR), &source) < 2.0e-7);
}

#[test]
fn a_tempo_change_is_played_as_the_app_plays_it() {
    let dir = tempfile::tempdir().unwrap();
    let source = noise(BAR, 13);
    let mut p = audio_project(write(dir.path(), "source.wav", &source), 3.0);
    // 120 BPM for two bars, then 60: bar 3 starts at 4 s whatever comes after it.
    p.tempo_changes.push(crate::model::TempoPoint {
        beat: 8.0,
        bpm: 60.0,
        ramp: false,
    });
    let out = dir.path().join("mix.wav");
    let r = bounce(&mut p, settings(&out));
    let got = read(&out);
    let start = 2 * BAR;
    assert_eq!(peak(frames(&got, 0, start)), 0.0);
    assert!(difference(frames(&got, start, start + BAR), &source) < 2.0e-7);
    // The region is a bar long at 60 BPM (4 s): the file plays for its 2 s, and the range
    // runs to the region's end.
    let map = p.tempo_map();
    let end = (map.seconds_at(r.to) * f64::from(SR)).round() as usize;
    assert_eq!(r.files[0].frames as usize, end + SR as usize / 2);
    assert!(end > start + BAR);
}

/// A one-shot for the Channel Rack: a tenth of a second of noise.
#[cfg(target_os = "macos")]
fn hit() -> Vec<f32> {
    noise(SR as usize / 10, 31)
        .iter()
        .map(|x| x * 0.4)
        .collect()
}

/// A project with an audio track (the file in bar 2), a Channel Rack track playing `hit`
/// at bars 1 and 3 through a Compressor, with a send to an Aux, and the audio track's
/// fader pulled down by automation halfway through its region.
#[cfg(target_os = "macos")]
fn song(dir: &Path, source: &[f32]) -> Project {
    let mut p = audio_project(write(dir, "source.wav", source), 2.0);
    let hit_file = write(dir, "hit.wav", &hit());
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
    let note = |start: f32| Note {
        lyric: crate::lyrics::Lyric::NONE,
        mute: false,
        start,
        len: 0.5,
        pitch: prowave_engine::RACK_BASE_NOTE,
        vel: 127,
    };
    t.regions.push(Region {
        notes: vec![note(0.0), note(8.0)],
        ..region("Hits", 1.0, 4.0)
    });
    t.inserts
        .push(Insert::new(prowave_engine::FxKind::Compressor));
    t.sends = vec![SendSlot {
        bus: Some(1),
        level: 1.0,
        pre: false,
    }];
    p.tracks.push(t);
    p.add_track(TrackKind::Aux);
    // The audio track: full level for half a bar, then far down.
    let a = &mut p.tracks[0].auto;
    a.set_point(AutoTarget::Volume, 0.0, 0.0);
    a.set_point(AutoTarget::Volume, 5.9, 0.0);
    a.set_point(AutoTarget::Volume, 6.0, -60.0);
    p
}

#[cfg(target_os = "macos")]
#[test]
fn audio_an_instrument_an_effect_a_send_and_automation_are_all_in_the_bounce() {
    let dir = tempfile::tempdir().unwrap();
    let source = noise(BAR, 17);
    let mut p = song(dir.path(), &source);
    let out = dir.path().join("mix.wav");
    let r = bounce(&mut p, settings(&out));
    assert!(r.not_rendered.is_empty(), "{:?}", r.not_rendered);
    assert_eq!(
        r.rendered
            .iter()
            .map(|(_, n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["Audio", "Rack"]
    );
    // Four bars (the Rack's region) and the tail.
    assert_eq!(r.files[0].frames as usize, 4 * BAR + SR as usize / 2);
    let got = read(&out);
    let tenth = SR as usize / 10;
    // The instrument's hits at bars 1 and 3, on the frame: silent before, loud from there.
    assert!(peak(frames(&got, 0, tenth)) > 0.02, "the hit at bar 1");
    assert!(
        got[0] != 0.0 || got[1] != 0.0,
        "the hit starts on the first frame"
    );
    let third = 2 * BAR;
    assert!(
        peak(frames(&got, third, third + tenth)) > 0.02,
        "the hit at bar 3"
    );
    // Nothing between the first hit and the audio region; the region starts on its frame.
    assert_eq!(peak(frames(&got, 2 * tenth, BAR)), 0.0);
    assert!(got[BAR * 2] != 0.0, "the region's first frame");
    // The audio region as its file while its fader is up (the first beat of bar 2)...
    let diff = difference(frames(&got, BAR, BAR + BEAT), frames(&source, 0, BEAT));
    assert!(diff < 1.0e-4, "the region differs from its file by {diff}");
    // ...and 60 dB down after the automation (the last beat of the bar).
    let late = peak(frames(&got, BAR + 3 * BEAT, 2 * BAR));
    assert!(late > 1.0e-5 && late < 1.0e-3, "{late}");
    // Silence after the last hit has rung out, to the end.
    assert_eq!(peak(frames(&got, third + 2 * tenth, 4 * BAR)), 0.0);

    // The send: without it the hit is only the strip's own; with it the Aux adds its own.
    let with = peak(frames(&got, 0, tenth));
    p.tracks[1].sends.clear();
    bounce(&mut p, settings(&dir.path().join("dry.wav")));
    let dry = read(&dir.path().join("dry.wav"));
    let without = peak(frames(&dry, 0, tenth));
    assert!(
        with > without * 1.5 && with < 1.0,
        "with the send {with}, without {without}"
    );
    // The effect: bypassed, the hit is another (the Compressor no longer turns it down).
    p.tracks[1].inserts[0].bypass = true;
    bounce(&mut p, settings(&dir.path().join("plain.wav")));
    let plain = read(&dir.path().join("plain.wav"));
    let plain_hit = frames(&plain, 0, tenth);
    assert!(difference(plain_hit, frames(&dry, 0, tenth)) > 1.0e-3);
    // Without effect or send the instrument is its sample, to the frame.
    assert!(difference(plain_hit, &hit()) < 1.0e-4);
}

#[cfg(target_os = "macos")]
#[test]
fn each_track_goes_into_a_file_of_its_own_and_they_add_up_to_the_mix() {
    let dir = tempfile::tempdir().unwrap();
    let source = noise(BAR, 19);
    let mut p = song(dir.path(), &source);
    // A track that is muted gets no file.
    let mut silent = Track::new("Muted", TrackKind::Audio);
    silent.mute = true;
    silent
        .regions
        .push(audio_region(write(dir.path(), "m.wav", &source), 1.0, 1.0));
    p.tracks.push(silent);
    let out = dir.path().join("Song.wav");
    let mut s = settings(&out);
    s.stems = true;
    let r = bounce(&mut p, s);
    assert!(r.not_rendered.is_empty(), "{:?}", r.not_rendered);
    let names: Vec<String> = r
        .files
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["Song.wav", "Song - 01 Audio.wav", "Song - 02 Rack.wav"]
    );
    assert_eq!(
        r.files.iter().map(|f| f.track.clone()).collect::<Vec<_>>(),
        [None, Some("Audio".to_string()), Some("Rack".to_string())]
    );
    assert_eq!(r.muted, vec!["Muted".to_string()]);
    let mix = read(&r.files[0].path);
    let audio = read(&r.files[1].path);
    let rack = read(&r.files[2].path);
    // All from the same start and of one length.
    assert!(audio.len() == mix.len() && rack.len() == mix.len());
    // The Rack's file has its hits (with what its send brings back) and nothing of the audio.
    assert_eq!(peak(frames(&rack, SR as usize / 5, 2 * BAR)), 0.0);
    assert_eq!(peak(frames(&audio, 0, BAR)), 0.0);
    let sum: Vec<f32> = audio.iter().zip(&rack).map(|(a, b)| a + b).collect();
    let diff = difference(&sum, &mix);
    assert!(diff < 1.0e-5, "the tracks differ from the mix by {diff}");
    // Only the tracks: no mix.
    let mut s = settings(&dir.path().join("Only.wav"));
    s.mix = false;
    s.stems = true;
    let r = bounce(&mut p, s);
    assert_eq!(r.files.len(), 2);
    assert!(!dir.path().join("Only.wav").exists());
    // One track into the file named.
    let one = dir.path().join("one.wav");
    let mut s = settings(&one);
    s.only = Some(1);
    let r = bounce(&mut p, s);
    assert_eq!(r.files.len(), 1);
    assert!(difference(&read(&one), &rack) < 1.0e-6);
}

#[cfg(target_os = "macos")]
#[test]
fn the_tail_goes_on_while_something_is_heard() {
    let dir = tempfile::tempdir().unwrap();
    let source = noise(BAR, 29);
    let mut p = audio_project(write(dir.path(), "source.wav", &source), 1.0);
    // An echo that outlasts the region.
    let mut delay = Insert::new(prowave_engine::FxKind::Delay);
    delay.params[1] = 0.6;
    p.tracks[0].inserts.push(delay);
    let out = dir.path().join("mix.wav");
    let mut s = settings(&out);
    s.tail_seconds = 0.25;
    s.max_tail_seconds = 30.0;
    let r = bounce(&mut p, s);
    let f = &r.files[0];
    assert!(f.tail_seconds > 0.3, "the echoes: {} s", f.tail_seconds);
    assert!(f.tail_seconds < 30.0, "it ends when they are gone");
    let got = read(&out);
    // The end is quiet: the last tenth of a second is below -80 dB, the stretch before the
    // quarter second of padding is not.
    let n = f.frames as usize;
    assert!(peak(frames(&got, n - SR as usize / 10, n)) <= 1.0e-4);
    let pad = SR as usize / 4;
    assert!(peak(frames(&got, n - pad - SR as usize / 10, n - pad + 2)) > 1.0e-4);
    // A fixed tail cuts it where it is told to.
    let fixed = dir.path().join("fixed.wav");
    let mut s = settings(&fixed);
    s.tail_seconds = 0.25;
    s.max_tail_seconds = 0.25;
    let r = bounce(&mut p, s);
    assert_eq!(r.files[0].frames as usize, BAR + SR as usize / 4);
}

/// Real Audio Units, Apple's own (on every Mac): an instrument (DLSMusicDevice) on a track,
/// an effect (AULowpass) on another, and one with a look-ahead (AUPeakLimiter) on the Stereo
/// Out. They are opened again for the bounce, told they render offline, and what they add in
/// delay is made up for: the audio region still starts on its frame.
#[cfg(target_os = "macos")]
#[test]
fn audio_units_are_in_the_bounce_and_their_delay_is_made_up_for() {
    use crate::plugins::PluginSlot;
    for id in ["aumu:dls :appl", "aufx:lpas:appl", "aufx:lmtr:appl"] {
        assert!(
            prowave_engine::au::find(id).is_some(),
            "{id} is not on this Mac"
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let source = noise(BAR, 37);
    // The audio in bar 3, through a low-pass filter.
    let mut p = audio_project(write(dir.path(), "source.wav", &source), 3.0);
    p.tracks[0]
        .au_effects
        .push(PluginSlot::new("aufx:lpas:appl", "Apple: AULowpass"));
    // A note of the instrument at the start of bar 1.
    let mut t = Track::new("Piano", TrackKind::Instrument);
    t.au_instrument = Some(PluginSlot::new("aumu:dls :appl", "Apple: DLSMusicDevice"));
    t.regions.push(Region {
        notes: vec![Note {
            lyric: crate::lyrics::Lyric::NONE,
            mute: false,
            start: 0.0,
            len: 0.5,
            pitch: 60,
            vel: 110,
        }],
        ..region("Note", 1.0, 1.0)
    });
    p.tracks.push(t);
    let plain = dir.path().join("plain.wav");
    let r = bounce(&mut p, settings(&plain));
    assert!(r.not_rendered.is_empty(), "{:?}", r.not_rendered);
    let got = read(&plain);
    let third = 2 * BAR;
    let check = |got: &[f32], what: &str| {
        // The instrument's note in bar 1, from its first few milliseconds on.
        let note = peak(frames(got, 0, BEAT));
        assert!(note > 0.01, "{what}: the instrument's note: {note}");
        assert!(
            peak(frames(got, 0, 480)) > 1.0e-4,
            "{what}: the note starts at the start"
        );
        // Quiet before the region, and the region from its own frame.
        let before = peak(frames(got, third - 4800, third));
        assert!(before < 1.0e-3, "{what}: before the region: {before}");
        let first = peak(frames(got, third, third + 8));
        assert!(first > 5.0e-3, "{what}: the region's first frames: {first}");
        // The filter changed the noise: it is there, and it is not the file any more.
        let region = frames(got, third, third + BAR);
        assert!(peak(region) > 0.05, "{what}: the region is heard");
        assert!(difference(region, &source) > 0.05, "{what}: unfiltered");
    };
    check(&got, "tracks");
    // The same with a limiter on the Stereo Out, which looks ahead: nothing moves.
    p.master_au_effects
        .push(PluginSlot::new("aufx:lmtr:appl", "Apple: AUPeakLimiter"));
    let limited = dir.path().join("limited.wav");
    let r = bounce(&mut p, settings(&limited));
    assert!(r.not_rendered.is_empty(), "{:?}", r.not_rendered);
    assert_eq!(r.files[0].frames as usize, 3 * BAR + SR as usize / 2);
    check(&read(&limited), "with the Stereo Out's limiter");
    // A plug-in that is not installed is named, and the rest is bounced.
    p.tracks[0]
        .au_effects
        .push(PluginSlot::new("aufx:none:none", "Nobody: Nothing"));
    let r = bounce(&mut p, settings(&dir.path().join("missing.wav")));
    assert_eq!(r.not_rendered.len(), 1, "{:?}", r.not_rendered);
    assert!(
        r.not_rendered[0].contains("Nobody: Nothing"),
        "{:?}",
        r.not_rendered
    );
}
