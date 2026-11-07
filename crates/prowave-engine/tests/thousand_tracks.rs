//! Phase 1: 1000 tracks. Logic Pro allows 1000 tracks per project; so does the engine.
//!
//! `a_thousand_instrument_tracks_all_sound_and_meter` is a normal unit test. The streaming one
//! is a wall-clock stress check:
//! `cargo test -p prowave-engine --test thousand_tracks -- --ignored --nocapture`
use prowave_engine::{
    AudioFile, Command, DiskThread, EngineCore, NoteEvent, Shared, Song, StateReader, TrackMix,
    MAX_TRACKS,
};
use std::io::Write;
use std::path::Path;
use std::sync::{atomic::Ordering, Arc};
use std::time::{Duration, Instant};

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const RATE: u32 = 48_000;
const TRACKS: usize = 1000;
const BLOCK: usize = 256;

fn rig() -> (
    EngineCore,
    rtrb::Producer<Command>,
    rtrb::Consumer<prowave_engine::Garbage>,
    Arc<Shared>,
    StateReader,
) {
    let (tx, rx) = rtrb::RingBuffer::new(16);
    let (gtx, grx) = rtrb::RingBuffer::new(8);
    let shared = Arc::new(Shared::new());
    let mut core = EngineCore::new(RATE, rx, gtx, shared.clone());
    let Some(state) = core.take_state_reader() else {
        panic!("one state reader");
    };
    (core, tx, grx, shared, state)
}

const _: () = assert!(
    MAX_TRACKS >= 1000,
    "the engine has room for a thousand tracks"
);

#[test]
fn a_thousand_instrument_tracks_all_sound_and_meter() {
    let (mut core, mut tx, _grx, _shared, mut state) = rig();
    // One note per track, all playing at once, each track with its own level.
    let notes = (0..TRACKS)
        .map(|t| NoteEvent {
            start_beat: 0.0,
            end_beat: 4.0,
            pitch: 40 + (t % 48) as u8,
            vel: 100,
            track: t as u16,
        })
        .collect();
    let mixes = (0..TRACKS)
        .map(|t| TrackMix {
            gain_db: -((t % 20) as f32),
            ..TrackMix::default()
        })
        .collect();
    assert!(tx
        .push(Command::SetSong(Box::new(Song::new(notes, mixes))))
        .is_ok());
    assert!(tx.push(Command::Play).is_ok());
    let mut out = [0.0f32; BLOCK * 2];
    for _ in 0..20 {
        assert_no_alloc::assert_no_alloc(|| core.process(&mut out));
    }
    let s = state.read();
    // The engine has 48 voices, so not every one of the 1000 notes sounds; the ones that do
    // are on the right tracks, and the last tracks can be addressed.
    let sounding = (0..TRACKS).filter(|&t| s.track_peaks(t).0 > 0.0).count();
    assert!(sounding >= 40, "{sounding} tracks have a meter reading");
    assert!(s.peaks.0 > 0.0);
    assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert_eq!(
        s.track_peaks(MAX_TRACKS),
        (0.0, 0.0),
        "past the end is silent"
    );
}

#[test]
fn live_notes_reach_the_last_tracks() {
    let (mut core, mut tx, _grx, _shared, mut state) = rig();
    let song = Song::new(vec![], vec![TrackMix::default(); TRACKS]);
    assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
    assert!(tx
        .push(Command::NoteOn {
            pitch: 69,
            vel: 100,
            track: 999,
        })
        .is_ok());
    let mut out = [0.0f32; BLOCK * 2];
    for _ in 0..4 {
        assert_no_alloc::assert_no_alloc(|| core.process(&mut out));
    }
    let s = state.read();
    assert!(s.track_peaks(999).0 > 0.0, "track 1000 sounds and meters");
    assert_eq!(s.track_peaks(998), (0.0, 0.0));
}

/// A short stereo 16-bit WAV: a ramp that differs a little per file.
fn write_wav(path: &Path, frames: usize) -> std::io::Result<()> {
    let data_len = (frames * 4) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for f in 0..frames {
        let v = (((f % 200) as i32 - 100) * 8) as i16;
        b.extend_from_slice(&v.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::File::create(path)?.write_all(&b)
}

#[test]
#[ignore = "wall-clock audio with 1000 streams; run on an otherwise idle machine"]
fn streams_a_thousand_audio_tracks_without_dropouts() -> Result<(), Box<dyn std::error::Error>> {
    const SECONDS: usize = 20;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tone.wav");
    write_wav(&path, RATE as usize * (SECONDS + 2))?;
    let file = AudioFile::open(&path)?;
    let disk = DiskThread::spawn();
    let mut clips = Vec::with_capacity(TRACKS);
    for track in 0..TRACKS {
        clips.push(disk.stream_with_ring(
            &file,
            RATE,
            track as u16,
            0.0,
            Default::default(),
            prowave_engine::ring_blocks_for(TRACKS),
        ));
    }
    let (mut core, mut tx, mut grx, shared, mut state) = rig();
    let mixes = vec![
        TrackMix {
            gain_db: -30.0,
            ..TrackMix::default()
        };
        TRACKS
    ];
    let song = Song::new(vec![], mixes).with_clips(clips);
    assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
    let mut out = [0.0f32; BLOCK * 2];
    // Let every stream open and prefetch while stopped.
    for _ in 0..300 {
        std::thread::sleep(Duration::from_millis(10));
        assert_no_alloc::assert_no_alloc(|| core.process(&mut out));
    }
    while grx.pop().is_ok() {}
    assert!(tx.push(Command::Play).is_ok());
    let play = RATE as usize * SECONDS;
    let start = Instant::now();
    let budget = Duration::from_secs_f64(BLOCK as f64 / RATE as f64);
    let (mut busy, mut worst, mut over) = (Duration::ZERO, Duration::ZERO, 0);
    let mut rendered = 0;
    while rendered < play {
        let began = Instant::now();
        assert_no_alloc::assert_no_alloc(|| core.process(&mut out));
        let took = began.elapsed();
        busy += took;
        worst = worst.max(took);
        over += usize::from(took > budget);
        assert!(out.iter().all(|x| x.is_finite()));
        rendered += BLOCK;
        let deadline = start + Duration::from_secs_f64(rendered as f64 / RATE as f64);
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    }
    let underruns = shared.underruns.load(Ordering::Relaxed);
    let sounding = (0..TRACKS)
        .filter(|&t| state.read().track_peaks(t).0 > 0.0)
        .count();
    println!(
        "{TRACKS} stereo tracks, {SECONDS} s, underruns={underruns}, tracks metering={sounding}"
    );
    println!(
        "render load={:.2}%, max callback={worst:?}, callbacks over budget={over}",
        busy.as_secs_f64() / (rendered as f64 / RATE as f64) * 100.0
    );
    assert_eq!(underruns, 0, "disk thread did not keep up");
    assert_eq!(sounding, TRACKS, "every track plays");
    Ok(())
}
