//! Logic-style cycle transport must preserve elapsed time across fractional sample boundaries.
use prowave_engine::{Command, EngineCore, Shared, TempoCurve, TempoEvent, TempoMap};
use std::sync::Arc;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn render(chunk: usize) -> f64 {
    let map = TempoMap::new(
        vec![
            TempoEvent {
                beat: 0.0,
                bpm: 137.0,
                curve: TempoCurve::Step,
            },
            TempoEvent {
                beat: 0.5,
                bpm: 83.0,
                curve: TempoCurve::Step,
            },
        ],
        vec![],
    );
    let (start, end) = (0.2, 0.73);
    let start_secs = map.seconds_at(start);
    let duration = map.seconds_at(end) - start_secs;
    let expected = map.beat_at(start_secs + 2.0f64.rem_euclid(duration));
    let (mut tx, rx) = rtrb::RingBuffer::new(8);
    let (gtx, _grx) = rtrb::RingBuffer::new(8);
    let shared = Arc::new(Shared::new());
    let mut core = EngineCore::new(48_000, rx, gtx, shared.clone());
    let mut state = core.take_state_reader().expect("state reader");
    for cmd in [
        Command::SetTempoMap(Box::new(map)),
        Command::SetPosition { beat: start },
        Command::SetCycle {
            on: true,
            start_beat: start,
            end_beat: end,
        },
        Command::Play,
    ] {
        assert!(tx.push(cmd).is_ok());
    }
    let mut output = vec![0.0; chunk * 2];
    let mut remaining = 96_000;
    while remaining > 0 {
        let n = remaining.min(chunk);
        assert_no_alloc::assert_no_alloc(|| core.process(&mut output[..n * 2]));
        assert!(output[..n * 2].iter().all(|s| *s == 0.0));
        remaining -= n;
    }
    let actual = state.read().position_beats;
    assert!(
        (actual - expected).abs() < 1e-8,
        "cycle drift: actual={actual}, expected={expected}"
    );
    actual
}

#[test]
fn cycle_preserves_time_across_tempo_changes_and_does_not_allocate() {
    assert!((render(127) - render(512)).abs() < 1e-12);
}
