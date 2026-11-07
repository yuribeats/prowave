//! MIDI Time Code out: the engine sends the SMPTE time of the playhead on the audio clock.
use prowave_engine::{
    Command, EngineCore, MtcDecoder, MtcEvent, MtcMessage, Shared, SmpteRate, TempoCurve,
    TempoEvent, TempoMap,
};
use std::sync::Arc;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const RATE: u32 = 48_000;
/// Project start: 01:00:00:00 at 25 fps.
const OFFSET: i64 = 25 * 3600;

struct Rig {
    core: EngineCore,
    tx: rtrb::Producer<Command>,
    mtc: rtrb::Consumer<MtcMessage>,
}

fn rig() -> Rig {
    let (tx, rx) = rtrb::RingBuffer::new(32);
    let (gtx, _grx) = rtrb::RingBuffer::new(8);
    let mut core = EngineCore::new(RATE, rx, gtx, Arc::new(Shared::new()));
    let Some(mtc) = core.take_mtc_reader() else {
        panic!("reader");
    };
    Rig { core, tx, mtc }
}

fn run(rig: &mut Rig, frames: usize) {
    let mut out = vec![0.0f32; frames * 2];
    assert_no_alloc::assert_no_alloc(|| rig.core.process(&mut out));
}

fn messages(rig: &mut Rig) -> Vec<MtcMessage> {
    std::iter::from_fn(|| rig.mtc.pop().ok()).collect()
}

fn mtc_on(rig: &mut Rig, rate: SmpteRate) {
    assert!(rig
        .tx
        .push(Command::SetMtc {
            on: true,
            rate,
            offset_frames: OFFSET,
        })
        .is_ok());
}

#[test]
fn turning_it_on_locates_and_playing_sends_quarter_frames() {
    let mut r = rig();
    mtc_on(&mut r, SmpteRate::Fps25);
    run(&mut r, 256);
    let first = messages(&mut r);
    assert_eq!(first.len(), 1, "a full frame tells receivers where we are");
    let mut dec = MtcDecoder::new();
    assert_eq!(
        dec.feed(first[0].bytes()),
        Some(MtcEvent::Locate {
            frames: OFFSET,
            rate: SmpteRate::Fps25
        })
    );

    // Two seconds at 120 bpm, in 480-frame blocks like a low-latency device: 25 fps, four
    // quarter frames to a frame.
    assert!(r.tx.push(Command::Play).is_ok());
    let mut quarters = 0;
    let mut running = Vec::new();
    for _ in 0..200 {
        run(&mut r, 480);
        for m in messages(&mut r) {
            if m.bytes().len() == 2 {
                quarters += 1;
            }
            if let Some(MtcEvent::Running { frames, .. }) = dec.feed(m.bytes()) {
                running.push(frames);
            }
        }
    }
    // 200 blocks of 10 ms: 2 s. The pause before the first group may eat a few messages.
    assert!((quarters - 200i32).abs() <= 10, "{quarters} quarter frames");
    assert!(!running.is_empty());
    // Time codes rise with the clock, a group (2 frames) at a time, never more than a frame
    // ahead or behind the playhead.
    let last = *running.last().unwrap_or(&0);
    assert!(
        (last - (OFFSET + 50)).abs() <= 3,
        "after 2 s the code is {last}, start {OFFSET}"
    );
    assert!(running.windows(2).all(|w| w[1] - w[0] == 2));
}

#[test]
fn a_seek_sends_a_full_frame_and_a_tempo_map_keeps_the_time_code_honest() {
    let mut r = rig();
    mtc_on(&mut r, SmpteRate::Fps25);
    run(&mut r, 64);
    messages(&mut r);

    // Beat 8 at 120 bpm is 4 s in.
    assert!(r.tx.push(Command::SetPosition { beat: 8.0 }).is_ok());
    run(&mut r, 64);
    let m = messages(&mut r);
    let mut dec = MtcDecoder::new();
    assert_eq!(
        dec.feed(m[0].bytes()),
        Some(MtcEvent::Locate {
            frames: OFFSET + 100,
            rate: SmpteRate::Fps25
        })
    );

    // A tempo ramp: 60 -> 120 bpm over the first 8 beats takes 8 / 90 * 60 * ln 2 / ... use
    // the map itself as the reference.
    let map = TempoMap::new(
        vec![
            TempoEvent {
                beat: 0.0,
                bpm: 60.0,
                curve: TempoCurve::Linear,
            },
            TempoEvent {
                beat: 8.0,
                bpm: 120.0,
                curve: TempoCurve::Step,
            },
        ],
        vec![],
    );
    let secs = map.seconds_at(8.0);
    assert!(r.tx.push(Command::SetTempoMap(Box::new(map))).is_ok());
    run(&mut r, 64);
    let m = messages(&mut r);
    let expect = OFFSET + (secs * 25.0) as i64;
    assert_eq!(
        dec.feed(m[0].bytes()),
        Some(MtcEvent::Locate {
            frames: expect,
            rate: SmpteRate::Fps25
        }),
        "the playhead is at a different time now, so receivers are told"
    );

    // Turned off, nothing more is sent.
    assert!(r
        .tx
        .push(Command::SetMtc {
            on: false,
            rate: SmpteRate::Fps25,
            offset_frames: OFFSET
        })
        .is_ok());
    assert!(r.tx.push(Command::Play).is_ok());
    run(&mut r, 4800);
    assert!(messages(&mut r).is_empty());
}

#[test]
fn drop_frame_time_code_is_sent_as_such() {
    let mut r = rig();
    assert!(r
        .tx
        .push(Command::SetMtc {
            on: true,
            rate: SmpteRate::Fps29_97Drop,
            offset_frames: 0,
        })
        .is_ok());
    // Beat 3600 at 120 bpm: 30 minutes in.
    assert!(r.tx.push(Command::SetPosition { beat: 3600.0 }).is_ok());
    run(&mut r, 64);
    let m = messages(&mut r);
    let last = m[m.len() - 1].bytes();
    // hh byte: rate code 2 (29.97 drop) in bits 5-6.
    assert_eq!(last[5] >> 5, 2);
    let mut dec = MtcDecoder::new();
    let Some(MtcEvent::Locate { frames, rate }) = dec.feed(last) else {
        panic!("a full frame");
    };
    assert_eq!(rate, SmpteRate::Fps29_97Drop);
    // 1800 s at 30000/1001 fps, to the frame.
    assert_eq!(frames, (1800.0 * 30_000.0 / 1_001.0) as i64);
}
