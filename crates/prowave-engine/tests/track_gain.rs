//! Verify channel-strip smoothing with polyphonic stereo audio, not just parameter values.
use prowave_engine::{Command, EngineCore, Shared, Song, TrackMix};
use std::sync::Arc;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn rig() -> (EngineCore, rtrb::Producer<Command>) {
    let (mut tx, rx) = rtrb::RingBuffer::new(16);
    let (gtx, _grx) = rtrb::RingBuffer::new(8);
    let core = EngineCore::new(48_000, rx, gtx, Arc::new(Shared::new()));
    assert!(tx
        .push(Command::SetSong(Box::new(Song::new(
            vec![],
            vec![TrackMix::default()]
        ))))
        .is_ok());
    for pitch in [60, 67] {
        assert!(tx
            .push(Command::NoteOn {
                pitch,
                vel: 80,
                track: 0
            })
            .is_ok());
    }
    (core, tx)
}

#[test]
fn track_fader_and_pan_ramps_do_not_allocate() {
    let (mut reference, _tx) = rig();
    let (mut changed, mut tx) = rig();
    let mut a = [0.0; 128];
    let mut b = [0.0; 128];
    reference.process(&mut a);
    changed.process(&mut b);
    assert_eq!(a, b);
    let reduced = 10.0f32.powf(-6.0 / 20.0);
    let mut previous = [1.0, 1.0];
    for (db, pan, target) in [
        (-6.0, 0.0, [reduced, reduced]),
        (-6.0, -1.0, [reduced * std::f32::consts::SQRT_2, 0.0]),
        (0.0, 1.0, [0.0, std::f32::consts::SQRT_2]),
    ] {
        assert!(tx
            .push(Command::SetTrackMix {
                track: 0,
                mix: TrackMix {
                    gain_db: db,
                    pan,
                    ..TrackMix::default()
                },
            })
            .is_ok());
        let mut sample = 0;
        for _ in 0..4 {
            assert_no_alloc::assert_no_alloc(|| reference.process(&mut a));
            assert_no_alloc::assert_no_alloc(|| changed.process(&mut b));
            for (original, faded) in a.as_chunks::<2>().0.iter().zip(b.as_chunks::<2>().0) {
                sample += 1;
                let progress = (sample as f32 / 240.0).min(1.0);
                for channel in 0..2 {
                    let gain = previous[channel] + (target[channel] - previous[channel]) * progress;
                    assert!(faded[channel].is_finite());
                    assert!((faded[channel] - original[channel] * gain).abs() < 3e-6);
                }
            }
        }
        previous = target;
    }
    // Muting remains immediate even after changing a fader or pan.
    assert!(tx
        .push(Command::SetTrackMix {
            track: 0,
            mix: TrackMix {
                mute: true,
                ..TrackMix::default()
            },
        })
        .is_ok());
    assert_no_alloc::assert_no_alloc(|| changed.process(&mut b));
    assert!(b.iter().all(|s| *s == 0.0));
}
