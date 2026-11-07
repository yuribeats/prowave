//! Untrusted floating-point WAV samples must not poison waveform previews or playback.
use prowave_engine::{AudioFile, Command, DiskThread, EngineCore, Shared, Song, TrackMix};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn float_wav(rate: u32) -> Vec<u8> {
    let samples = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.25,
        -0.5,
        0.125,
    ];
    let data_len = 48_000u32 * 4;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..48_000 {
        bytes.extend_from_slice(&samples[frame % samples.len()].to_le_bytes());
    }
    bytes
}

#[test]
fn wav_above_supported_rate_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("prowave-unsupported-rate.wav");
    std::fs::write(&path, float_wav(192_001))?;
    assert!(AudioFile::open(path).is_err());
    Ok(())
}

#[test]
fn non_finite_wav_playback_does_not_allocate() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("prowave-nonfinite.wav");
    std::fs::write(&path, float_wav(48_000))?;
    let file = AudioFile::open(path)?;
    let peaks = file.peaks(100)?;
    assert!(peaks.iter().all(|&(lo, hi)| lo == -0.5 && hi == 0.25));
    let disk = DiskThread::spawn();
    let clip = disk.stream(&file, 48_000, 0, 0.0);
    let song = Song::new(vec![], vec![TrackMix::default()]).with_clips(vec![clip]);
    let (mut tx, rx) = rtrb::RingBuffer::new(8);
    let (gtx, _grx) = rtrb::RingBuffer::new(8);
    let shared = Arc::new(Shared::new());
    let mut core = EngineCore::new(48_000, rx, gtx, shared.clone());
    assert!(tx.push(Command::SetSong(Box::new(song))).is_ok());
    assert!(tx.push(Command::SetMasterGain { linear: 1.0 }).is_ok());
    let mut output = [0.0; 512];
    assert_no_alloc::assert_no_alloc(|| core.process(&mut output));
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(10));
        assert_no_alloc::assert_no_alloc(|| core.process(&mut output));
    }
    assert!(tx.push(Command::Play).is_ok());
    let expected = [0.0, 0.0, 0.0, 0.25, -0.5, 0.125];
    for block in 0..20 {
        assert_no_alloc::assert_no_alloc(|| core.process(&mut output));
        for (i, frame) in output.as_chunks::<2>().0.iter().enumerate() {
            let reference = expected[(block * 256 + i) % 6];
            for value in frame {
                assert!(value.is_finite());
                assert!((value - reference).abs() < 1e-6);
            }
        }
    }
    assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
    Ok(())
}
