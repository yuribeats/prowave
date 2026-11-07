//! A Pitch Editor edit is made offline into a file; playing it is ordinary disk streaming,
//! with no allocation (and no resynthesis) on the audio thread.
use prowave_engine::melody::{find_notes, Algorithm};
use prowave_engine::note_render::{render_notes, NoteChange, SegmentCache, DEFAULT_RESYNTH};
use prowave_engine::{AudioFile, Command, DiskThread, EngineCore, Shared, Song, TrackMix};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const RATE: u32 = 48_000;

#[test]
fn a_pitch_edited_note_plays_from_its_file_without_allocating(
) -> Result<(), Box<dyn std::error::Error>> {
    // A sung-like tone, half a second of A3, moved up 3 semitones.
    let n = RATE as usize / 2;
    let mut phase = 0.0f64;
    let mono: Vec<f32> = (0..n)
        .map(|i| {
            phase += 220.0 / f64::from(RATE);
            let p = std::f64::consts::TAU * phase;
            let env = (i as f64 / 480.0).min(1.0).min((n - i) as f64 / 480.0);
            ((p.sin() + 0.5 * (2.0 * p).sin() + 0.3 * (3.0 * p).sin()) * 0.2 * env) as f32
        })
        .collect();
    let stereo: Vec<f32> = mono.iter().flat_map(|v| [*v, *v]).collect();
    let found = find_notes(&mono, RATE, Algorithm::Melodic, &[]);
    let b = found.blobs.first().ok_or("no note found")?;
    let change = NoteChange {
        shift: 3.0,
        ..NoteChange::none(b.start, b.end)
    };
    let cache = SegmentCache::default();
    let (made, _) = render_notes(
        &stereo,
        RATE,
        &found.track,
        &[change],
        DEFAULT_RESYNTH,
        &cache,
        1,
    );
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("edited.wav");
    prowave_engine::write_wav(&path, &made, RATE)?;

    let file = AudioFile::open(&path)?;
    let disk = DiskThread::spawn();
    let clip = disk.stream(&file, RATE, 0, 0.0);
    let song = Song::new(vec![], vec![TrackMix::default()]).with_clips(vec![clip]);
    let (mut tx, rx) = rtrb::RingBuffer::new(8);
    let (gtx, _grx) = rtrb::RingBuffer::new(8);
    let shared = Arc::new(Shared::new());
    let mut core = EngineCore::new(RATE, rx, gtx, shared.clone());
    assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
    assert!(tx.push(Command::SetMasterGain { linear: 1.0 }).is_ok());
    let mut output = [0.0f32; 512];
    assert_no_alloc::assert_no_alloc(|| core.process(&mut output));
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(10));
        assert_no_alloc::assert_no_alloc(|| core.process(&mut output));
    }
    assert!(tx.push(Command::Play).is_ok());
    // The blocks played are the made file's samples.
    let mut heard = Vec::new();
    for _ in 0..40 {
        assert_no_alloc::assert_no_alloc(|| core.process(&mut output));
        heard.extend_from_slice(&output);
    }
    let compare = heard.len().min(made.len());
    let worst = heard[..compare]
        .iter()
        .zip(&made[..compare])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < 1e-4,
        "played sound differs from the file by {worst}"
    );
    assert!(heard.iter().any(|v| v.abs() > 0.05), "silent");
    assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
    Ok(())
}
