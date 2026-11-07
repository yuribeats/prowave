//! Verify the master-fader envelope on actual rendered samples, including stopped audition.
use prowave_engine::{Command, EngineCore, Shared};
use std::sync::Arc;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn rig() -> (EngineCore, rtrb::Producer<Command>) {
    let (mut tx, rx) = rtrb::RingBuffer::new(8);
    let (gtx, _grx) = rtrb::RingBuffer::new(8);
    let core = EngineCore::new(48_000, rx, gtx, Arc::new(Shared::new()));
    assert!(tx.push(Command::SetMasterGain { linear: 1.0 }).is_ok());
    assert!(tx
        .push(Command::SetTestTone {
            hz: 440.0,
            gain: 0.2
        })
        .is_ok());
    (core, tx)
}

#[test]
fn master_ramp_matches_audio_envelope_and_does_not_allocate() {
    let (mut reference, _tx) = rig();
    let (mut changed, mut tx) = rig();
    let mut a = [0.0; 128];
    let mut b = [0.0; 128];
    reference.process(&mut a);
    changed.process(&mut b);
    assert_eq!(a, b, "stopped-state initialization must be immediate");
    assert!(tx.push(Command::SetMasterGain { linear: 0.0 }).is_ok());
    let mut sample = 0;
    for _ in 0..8 {
        assert_no_alloc::assert_no_alloc(|| reference.process(&mut a));
        assert_no_alloc::assert_no_alloc(|| changed.process(&mut b));
        for (original, faded) in a.as_chunks::<2>().0.iter().zip(b.as_chunks::<2>().0) {
            sample += 1;
            let gain = (1.0 - sample as f32 / 240.0).max(0.0);
            for channel in 0..2 {
                assert!((faded[channel] - original[channel] * gain).abs() < 1e-6);
            }
        }
    }
    assert!(b.iter().all(|s| *s == 0.0), "target gain must be exact");
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(tx.push(Command::SetMasterGain { linear: value }).is_ok());
        assert_no_alloc::assert_no_alloc(|| changed.process(&mut b));
        assert!(b.iter().all(|s| *s == 0.0));
    }
}
