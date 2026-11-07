//! Exercises the public engine entry point with a deliberately stalled UI consumer.
use prowave_engine::{Command, EngineCore, NoteEvent, Shared, Song, TempoMap};
use std::sync::Arc;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

#[test]
fn state_replacement_burst_does_not_allocate() {
    let (mut tx, rx) = rtrb::RingBuffer::new(128);
    let (gtx, mut grx) = rtrb::RingBuffer::new(2);
    let shared = Arc::new(Shared::new());
    let mut core = EngineCore::new(48_000, rx, gtx, shared.clone());
    let mut state = core.take_state_reader().expect("state reader");
    for i in 0..60 {
        let notes = vec![NoteEvent {
            start_beat: 0.0,
            end_beat: 4.0,
            pitch: 60,
            vel: 100,
            track: 0,
        }];
        let cmd = match i % 3 {
            0 => Command::SetSong(Box::new(Song::new(notes, vec![]))),
            1 => Command::SetNotes(notes),
            _ => Command::SetTempoMap(Box::new(TempoMap::constant(120.0, (4, 4)))),
        };
        assert!(tx.push(cmd).is_ok());
    }
    assert!(tx.push(Command::Play).is_ok());
    let mut output = [0.0; 256];
    let mut reclaimed = 0;
    for _ in 0..30 {
        for _ in 0..4 {
            assert_no_alloc::assert_no_alloc(|| core.process(&mut output));
            assert!(output.iter().all(|s| s.is_finite()));
        }
        while grx.pop().is_ok() {
            reclaimed += 1;
        }
    }
    assert_eq!(reclaimed, 60);
    assert!(
        state.read().position_beats > 0.0,
        "transport resumes after the ordered edits"
    );
}
