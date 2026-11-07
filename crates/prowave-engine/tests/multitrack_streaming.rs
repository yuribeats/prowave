//! Phase 1: 24 simultaneous stereo audio regions, equivalent to Logic audio-track playback.
//! Run explicitly: cargo test -p prowave-engine --test multitrack_streaming -- --ignored --nocapture
//! Wall-clock pacing makes this a stress check, not a deterministic CI unit test.
use prowave_engine::{AudioFile, Command, DiskThread, EngineCore, Shared, Song, TrackMix};
use std::io::Write;
use std::path::Path;
use std::sync::{atomic::Ordering, Arc};
use std::time::{Duration, Instant};

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const RATE: u32 = 48_000;
const TRACKS: usize = 24;
const BLOCK: usize = 256;
const PLAY_FRAMES: usize = RATE as usize * 60;
const FILE_FRAMES: usize = RATE as usize * 61;
const PCM_SCALE: f32 = 8_388_608.0;

// Distinct integer waveforms per track allow a sample-by-sample reference mix, including
// 24-bit low bytes (so a decoder accidentally truncating to 16 bits fails this check).
fn sample(track: usize, frame: usize) -> i32 {
    let period = 128 + track * 16;
    ((frame % period) as i32 - (period / 2) as i32) * (track as i32 + 1) * 2
}

fn write_pcm24(path: &Path, track: usize) -> std::io::Result<()> {
    let data_len = (FILE_FRAMES * 6) as u32;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&2u16.to_le_bytes()); // stereo
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 6).to_le_bytes());
    bytes.extend_from_slice(&6u16.to_le_bytes());
    bytes.extend_from_slice(&24u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..FILE_FRAMES {
        let left = sample(track, frame);
        for value in [left, -left / 2] {
            bytes.extend_from_slice(&value.to_le_bytes()[..3]);
        }
    }
    std::fs::File::create(path)?.write_all(&bytes)
}

#[test]
#[ignore = "60 seconds of wall-clock audio; run on an otherwise idle machine"]
fn streams_24_pcm24_tracks_without_dropouts() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let disk = DiskThread::spawn();
    let mut clips = Vec::with_capacity(TRACKS);
    for track in 0..TRACKS {
        let path = dir.path().join(format!("prowave-track-{track:02}.wav"));
        write_pcm24(&path, track)?;
        let file = AudioFile::open(path)?;
        assert_eq!(
            (file.sample_rate, file.channels, file.frames),
            (RATE, 2, FILE_FRAMES as u64)
        );
        clips.push(disk.stream(&file, RATE, track as u16, 0.0));
    }
    let (mut tx, rx) = rtrb::RingBuffer::new(16);
    let (gtx, mut grx) = rtrb::RingBuffer::new(8);
    let shared = Arc::new(Shared::new());
    let mut core = EngineCore::new(RATE, rx, gtx, shared.clone());
    let mut state = core.take_state_reader().expect("state reader");
    let song = Song::new(vec![], vec![TrackMix::default(); TRACKS]).with_clips(clips);
    assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
    assert!(tx.push(Command::SetMasterGain { linear: 1.0 }).is_ok());
    let mut output = [0.0; BLOCK * 2];
    assert_no_alloc::assert_no_alloc(|| core.process(&mut output));
    // Allow initial file opening and seek prefetch while transport is stopped. Repeated
    // servicing discards stale generations; this does not consume the current audio.
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(10));
        assert_no_alloc::assert_no_alloc(|| core.process(&mut output));
    }
    while grx.pop().is_ok() {}
    assert!(tx.push(Command::Play).is_ok());
    let start = Instant::now();
    let budget = Duration::from_secs_f64(BLOCK as f64 / RATE as f64);
    let mut render_time = Duration::ZERO;
    let mut max_render = Duration::ZERO;
    let mut over_budget = 0;
    let mut max_error = 0.0f32;
    let mut rendered = 0;
    while rendered < PLAY_FRAMES {
        let frames = BLOCK.min(PLAY_FRAMES - rendered);
        let began = Instant::now();
        assert_no_alloc::assert_no_alloc(|| core.process(&mut output[..frames * 2]));
        let elapsed = began.elapsed();
        render_time += elapsed;
        max_render = max_render.max(elapsed);
        over_budget += usize::from(elapsed > budget);
        for (offset, stereo) in output[..frames * 2].as_chunks::<2>().0.iter().enumerate() {
            let sum: i32 = (0..TRACKS).map(|t| sample(t, rendered + offset)).sum();
            let reference = sum as f32 / PCM_SCALE;
            assert!(stereo.iter().all(|s| s.is_finite()));
            max_error = max_error.max((stereo[0] - reference).abs());
            max_error = max_error.max((stereo[1] + reference / 2.0).abs());
        }
        rendered += frames;
        let deadline = start + Duration::from_secs_f64(rendered as f64 / RATE as f64);
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    }
    let underruns = shared.underruns.load(Ordering::Relaxed);
    println!("24 stereo tracks, 48 kHz/24-bit, {rendered} frames, 256-frame blocks");
    println!("underruns={underruns}, max sample error={max_error:.9}");
    println!(
        "render load={:.2}%, max callback={max_render:?}, callbacks over budget={over_budget}",
        render_time.as_secs_f64() / (rendered as f64 / RATE as f64) * 100.0
    );
    assert_eq!(underruns, 0, "disk thread did not keep up");
    assert!(
        max_error < 0.000001,
        "missing, duplicated, or corrupted audio samples"
    );
    assert!((state.read().position_beats - 120.0).abs() < 1e-6);
    Ok(())
}
