//! The Tracks area with View > Colour Spectrogram on: where a section boundary lands on
//! screen, pictures of a made-up song at several zooms, and how long a four-minute file takes.
//! Declared from `tracks_area.rs` (its private drawing is reached from here).
use super::*;
use crate::colour_spectrogram::tests::{song, temp_dir, write};
use crate::model::{AudioRegion, RegionParams};
use prowave_engine::AudioFile;

/// The made-up song's sections: (kind, bars). 36 bars at 120 bpm, 72 s: intro, verse,
/// chorus, breakdown, chorus, outro (see `colour_spectrogram::tests::song`).
const SONG: [(usize, u32); 6] = [(0, 4), (1, 8), (2, 8), (3, 4), (2, 8), (4, 4)];
const SECTION_NAMES: [&str; 6] = ["INTRO", "VERSE", "CHORUS", "BREAK", "CHORUS", "OUTRO"];

fn audio_region(
    file: &AudioFile,
    name: &str,
    start_bar: f32,
    len_bars: f32,
    offset: f64,
) -> Region {
    let a = AudioRegion {
        file: file.clone(),
        peaks: Default::default(),
        flex: None,
    };
    if let Ok(pk) = file.peaks(4000) {
        let _ = a.peaks.set(pk);
    }
    Region {
        missing: None,
        name: name.into(),
        start_bar,
        len_bars,
        notes: Vec::new(),
        controls: Vec::new(),
        folder: None,
        generated: None,
        audio: Some(a),
        params: RegionParams {
            offset_secs: offset,
            ..RegionParams::default()
        },
    }
}

/// Waits until the app's cache has the file's picture, so a drawing shows it.
fn made(file: &AudioFile) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while crate::colour_spectrogram::paint_ready(file).is_none() {
        assert!(std::time::Instant::now() < deadline, "no picture made");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A project with the song on track 1 (bar 1 to 37), a four-bar region cut from its first
/// chorus on track 2 (bar 9, from 24 s into the file), and a marker at each section.
fn project(file: &AudioFile) -> (Project, crate::UiState) {
    let mut p = Project::empty();
    p.tracks.clear();
    let mut t = Track::new("Song", TrackKind::Audio);
    t.regions.push(audio_region(file, "Song", 1.0, 36.0, 0.0));
    p.tracks.push(t);
    let mut t = Track::new("Chorus Cut", TrackKind::Audio);
    t.color = theme::TRACK_PALETTE[3];
    t.regions
        .push(audio_region(file, "Chorus Cut", 9.0, 4.0, 24.0));
    p.tracks.push(t);
    let mut bar = 0.0;
    for ((_, bars), name) in SONG.iter().zip(SECTION_NAMES) {
        let i = p.add_marker(bar * 4.0);
        p.markers[i].name = name.into();
        bar += f64::from(*bars);
    }
    let mut s = crate::UiState::new();
    s.tracks.show_markers = true;
    s.tracks.colour_spectra = true;
    s.tracks.row_h = 96.0;
    (p, s)
}

fn song_file(dir: &std::path::Path) -> AudioFile {
    write(dir, "song.wav", &song(44_100, &SONG), 44_100)
}

/// Steady sound with no beats, so the only change is the section's: a low tone up to 24 s
/// (bar 13), high tones to 40 s (bar 21), the low tone again to 72 s.
fn steady_file(dir: &std::path::Path) -> AudioFile {
    let rate = 44_100u32;
    let n = 72 * rate as usize;
    let mut x = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / f64::from(rate);
        let s = |hz: f64| (std::f64::consts::TAU * hz * t).sin();
        let v = if (24.0..40.0).contains(&t) {
            0.2 * s(4000.0) + 0.15 * s(7500.0) + 0.1 * s(600.0)
        } else {
            0.4 * s(70.0) + 0.1 * s(140.0)
        };
        x.push(v as f32);
        x.push(v as f32);
    }
    write(dir, "steady.wav", &x, rate)
}

fn render(
    p: &mut Project,
    s: &mut crate::UiState,
    w: f32,
    h: f32,
    ppp: f32,
) -> (image::RgbaImage, egui::Context) {
    let meters = crate::mixer::Meters::default();
    crate::ui_test::render_image(w, h, ppp, theme::BONE(), |ui| {
        theme::apply(ui.ctx());
        show(ui, p, s, &meters)
    })
    .unwrap()
}

/// Pixel column `x` of row `y` as numbers.
fn rgb(img: &image::RgbaImage, x: i64, y: i64) -> [f64; 3] {
    let c = img.get_pixel(x as u32, y as u32).0;
    [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])]
}

#[test]
fn view_colour_spectrogram_turns_it_on_and_off() {
    let mut p = Project::empty();
    let mut s = crate::UiState::new();
    let label = "Colour Spectrogram";
    assert!(crate::menus::is_checkable(label) && crate::menus::enabled(label, &p, &s));
    assert!(!crate::menus::checked(label, &p, &s), "off at first");
    crate::menus::apply(label, &mut p, &mut s);
    assert!(s.tracks.colour_spectra && crate::menus::checked(label, &p, &s));
    crate::menus::apply(label, &mut p, &mut s);
    assert!(!s.tracks.colour_spectra);
}

#[test]
fn a_section_boundary_lands_on_its_grid_line() {
    let dir = temp_dir("boundary");
    let file = steady_file(&dir);
    made(&file);
    // The verse ends and the chorus begins at bar 13 (24 s, exactly frame 375 of the
    // picture); the chorus ends at bar 21 (40 s, frame 625).
    for &(bar_w, scroll, ppp) in &[
        (96.0f32, 600.0f32, 1.0f32),
        (300.0, 2950.0, 2.0),
        (600.0, 6950.0, 2.0),
        (233.1, 2701.37, 1.5),
    ] {
        let (mut p, mut s) = project(&file);
        s.tracks.bar_w = bar_w;
        s.tracks.scroll_x = scroll;
        let (img, ctx) = render(&mut p, &mut s, 1400.0, 420.0, ppp);
        let lanes = ctx.read_response(id("lanes")).expect("drawn").rect;
        let beat_px = f64::from(bar_w) / 4.0;
        let edge = |beat: f64| {
            ((f64::from(lanes.left()) + beat * beat_px - f64::from(scroll)) * f64::from(ppp))
                .round() as i64
        };
        // The middle of track 1's body, away from bar lines.
        let y = ((lanes.top() + 60.0) * ppp) as i64;
        let (lo, hi) = (
            (lanes.left() * ppp).ceil() as i64 + 2,
            (lanes.right() * ppp).floor() as i64 - 2,
        );
        let mut seen = 0;
        for beat in [48.0, 80.0] {
            let want = edge(beat);
            // Where the colour changes most within a bar either side.
            let reach = (bar_w * ppp) as i64;
            if want - 60 < lo || want + 60 > hi {
                continue;
            }
            // Pixels a texel covers: the change is spread over about that many.
            let texel = (0.064 * 2.0 * beat_px * f64::from(ppp)).max(1.0);
            let k = texel.ceil() as i64 + 2;
            // The bar lines are drawn over the picture: their columns are left out.
            let line_w = ppp.round().max(1.0) as i64;
            let on_line = |x: i64| {
                (0..60).any(|bar| {
                    let e = edge(f64::from(bar) * 4.0);
                    x >= e && x < e + line_w
                })
            };
            let mean = |a: i64, b: i64| {
                let cols: Vec<[f64; 3]> = (a..b)
                    .filter(|x| !on_line(*x))
                    .map(|x| rgb(&img, x, y))
                    .collect();
                let n = cols.len().max(1) as f64;
                cols.iter().fold([0.0; 3], |s, c| {
                    [s[0] + c[0] / n, s[1] + c[1] / n, s[2] + c[2] / n]
                })
            };
            // The column where the colours before and after differ most.
            let score = |x: i64| {
                let (l, r) = (mean(x - k, x), mean(x, x + k));
                ((l[0] - r[0]).abs() + (l[1] - r[1]).abs() + (l[2] - r[2]).abs()) as f32
            };
            let range = (want - reach).max(lo + k)..(want + reach).min(hi - k);
            let got = range
                .max_by(|a, b| score(*a).total_cmp(&score(*b)))
                .unwrap();
            let (l, r) = (mean(got - k, got), mean(got, got + k));
            println!("{bar_w} points a bar, {ppp}x, beat {beat}: before {l:.0?}, after {r:.0?}");
            assert!(
                (got - want).abs() <= 1,
                "{bar_w} points a bar, {ppp}x: beat {beat} changes colour at column {got}, its grid line is {want} (a texel is {texel:.1} pixels)"
            );
            seen += 1;
        }
        assert!(seen > 0, "{bar_w}: no boundary on screen");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Pictures of the Tracks area with the made-up song, colour spectrogram on, at several zooms
/// (and one with it off, to compare):
/// `MANUAL_PNG_DIR=… cargo test -p prowave-app colour_spectrogram_pictures -- --ignored`.
#[test]
#[ignore = "pictures of the Tracks area"]
fn colour_spectrogram_pictures() {
    let dir = temp_dir("pictures");
    let file = song_file(&dir);
    made(&file);
    let shots: [(&str, f32, f32, bool, f32); 6] = [
        // name, points a bar, scroll, on, track height
        ("colour-spectrogram-fit.png", 30.0, 0.0, true, 96.0),
        (
            "colour-spectrogram-fit-waveform.png",
            30.0,
            0.0,
            false,
            96.0,
        ),
        ("colour-spectrogram-96.png", 96.0, 0.0, true, 96.0),
        ("colour-spectrogram-300.png", 300.0, 2950.0, true, 96.0),
        ("colour-spectrogram-600.png", 600.0, 6950.0, true, 130.0),
        ("colour-spectrogram-small.png", 30.0, 0.0, true, 44.0),
    ];
    for (name, bar_w, scroll, on, row_h) in shots {
        let (mut p, mut s) = project(&file);
        s.tracks.bar_w = bar_w;
        s.tracks.scroll_x = scroll;
        s.tracks.colour_spectra = on;
        s.tracks.row_h = row_h;
        if name.contains("600") {
            s.tracks.select_only(1, 0);
        }
        let (img, _) = render(&mut p, &mut s, 1500.0, 330.0, 2.0);
        img.save(crate::spectrogram_view::hw_vocal::picture_path(name))
            .unwrap();
    }
}

/// How long the colour spectrogram of a four-minute stereo file takes to make, to read back,
/// and to draw: `cargo test --release -p prowave-app colour_spectrogram_timing -- --ignored
/// --nocapture`.
#[test]
#[ignore = "timing"]
fn colour_spectrogram_timing() {
    use std::time::Instant;
    let dir = temp_dir("timing");
    let sections = [
        (0, 8),
        (1, 16),
        (2, 16),
        (3, 8),
        (2, 16),
        (1, 16),
        (2, 32),
        (4, 8),
    ];
    let x = song(44_100, &sections);
    let file = write(&dir, "four-minutes.wav", &x, 44_100);
    drop(x);
    println!(
        "file: {:.1} s, {} Hz stereo, {:.1} MB",
        file.duration_secs(),
        file.sample_rate,
        std::fs::metadata(&file.path).unwrap().len() as f64 / 1e6
    );
    let t = Instant::now();
    let pic = crate::colour_spectrogram::measure(&file).unwrap();
    println!(
        "measure (decode + analysis): {:.0} ms, {} columns",
        t.elapsed().as_secs_f64() * 1000.0,
        pic.frames
    );
    let t = Instant::now();
    let _ = crate::pitch_editor::content_hash(&file.path).unwrap();
    println!("content hash: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let cache = crate::colour_spectrogram::Cache::new(dir.join("cache"));
    let t = Instant::now();
    let deadline = Instant::now() + std::time::Duration::from_secs(120);
    while cache.get(None, &file).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    println!(
        "first ask to picture ready (hash + measure + write cache): {:.0} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    let next = crate::colour_spectrogram::Cache::new(dir.join("cache"));
    let t = Instant::now();
    while next.get(None, &file).is_none() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    println!(
        "next start, from the disk cache: {:.0} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    // Drawing: a 1400-point region holding the whole song, then zoomed in.
    let cache2 = next.clone();
    let ctx = egui::Context::default();
    for (label, width) in [
        ("whole song in 1400 points", 1400.0f32),
        ("zoomed in, 1400 of 30000 points", 30_000.0),
    ] {
        let secs = file.duration_secs();
        let mut times = Vec::new();
        for i in 0..60 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 200.0))),
                time: Some(f64::from(i) / 60.0),
                ..Default::default()
            };
            ctx.set_pixels_per_point(2.0);
            let file = file.clone();
            let cache = cache2.clone();
            let _ = ctx.run_ui(input, |ui| {
                let g = ui.painter().clone();
                let t = Instant::now();
                let area = Rect::from_min_size(pos2(0.0, 20.0), vec2(1400.0, 120.0));
                let drawn = crate::colour_spectrogram::paint_from(
                    &cache,
                    &g,
                    area,
                    &file,
                    |x| {
                        let f = f64::from(x / width) * secs;
                        (0.0..=secs).contains(&f).then_some(f)
                    },
                    Color32::WHITE,
                );
                assert!(drawn);
                times.push(t.elapsed().as_secs_f64() * 1e6);
            });
        }
        let first = times[0];
        let mut rest = times[1..].to_vec();
        rest.sort_by(f64::total_cmp);
        println!(
            "draw, {label}: first frame (textures made) {first:.0} µs, then median {:.0} µs, worst {:.0} µs",
            rest[rest.len() / 2],
            rest[rest.len() - 1]
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
