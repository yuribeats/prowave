//! Logic-style instrument-track audition: same-pitch notes release independently.
use prowave_engine::{Command, EngineCore, Shared, Song, TrackMix};
use std::sync::Arc;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

#[test]
fn independent_live_note_release_does_not_allocate() {
    let (mut tx, rx) = rtrb::RingBuffer::new(16);
    let (gtx, _grx) = rtrb::RingBuffer::new(8);
    let mut core = EngineCore::new(48_000, rx, gtx, Arc::new(Shared::new()));
    let tracks = vec![
        TrackMix {
            pan: -1.0,
            ..TrackMix::default()
        },
        TrackMix {
            pan: 1.0,
            ..TrackMix::default()
        },
    ];
    assert!(tx
        .push(Command::SetSong(Box::new(Song::new(vec![], tracks))))
        .is_ok());
    for track in 0..2 {
        assert!(tx
            .push(Command::NoteOn {
                pitch: 69,
                vel: 100,
                track
            })
            .is_ok());
    }
    let mut audio = vec![0.0; 48_000]; // half a second, stereo
    assert_no_alloc::assert_no_alloc(|| core.process(&mut audio));
    for channel in 0..2 {
        assert!(audio
            .iter()
            .skip(channel)
            .step_by(2)
            .any(|x| x.abs() > 0.01));
    }
    assert!(tx
        .push(Command::NoteOffTrack {
            pitch: 69,
            track: 0
        })
        .is_ok());
    assert_no_alloc::assert_no_alloc(|| core.process(&mut audio)); // release tail
    assert_no_alloc::assert_no_alloc(|| core.process(&mut audio));
    assert!(audio.iter().all(|x| x.is_finite()));
    assert!(audio.iter().step_by(2).all(|x| x.abs() < 1e-6));
    assert!(audio.iter().skip(1).step_by(2).any(|x| x.abs() > 0.01));
    assert!(tx
        .push(Command::NoteOffTrack {
            pitch: 69,
            track: 1
        })
        .is_ok());
    assert_no_alloc::assert_no_alloc(|| core.process(&mut audio));
    assert_no_alloc::assert_no_alloc(|| core.process(&mut audio));
    assert!(audio.iter().all(|x| x.abs() < 1e-6));
}
