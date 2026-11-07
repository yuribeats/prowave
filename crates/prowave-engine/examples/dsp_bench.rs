//! DSP benchmark: renders a dense arrangement offline (no audio device) and reports how much
//! of the real-time budget the engine uses. Run in release:
//! `cargo run --release -p prowave-engine --example dsp_bench`
use prowave_engine::{
    Command, EngineCore, Kit, NoteEvent, Shared, Song, SynthPatch, TempoEvent, TempoMap, TrackMix,
    Wave,
};
use std::sync::Arc;
use std::time::Instant;

const SR: u32 = 48_000;
const BLOCK: usize = 128;
const SECONDS: usize = 20;

fn main() {
    let (mut tx, rx) = rtrb::RingBuffer::new(256);
    let (gtx, _grx) = rtrb::RingBuffer::new(32);
    let mut core = EngineCore::new(SR, rx, gtx, Arc::new(Shared::new()));

    // 16 tracks; most voices are filtered saws with vibrato (the expensive path), plus drums.
    let tracks = 16u8;
    let mut notes = Vec::new();
    for t in 0..tracks {
        for i in 0..(SECONDS * 4) {
            for k in 0..3u8 {
                notes.push(NoteEvent {
                    start_beat: i as f64 * 0.5,
                    end_beat: i as f64 * 0.5 + 2.0,
                    pitch: 40 + (t * 3 + k * 4 + (i % 7) as u8) % 48,
                    vel: 100,
                    track: t as u16,
                });
            }
        }
    }
    let song = Song::new(notes, vec![TrackMix::default(); tracks as usize]);
    let cmds = [
        Command::SetSong(Box::new(song)),
        Command::SetTempoMap(Box::new(TempoMap::new(
            vec![
                TempoEvent {
                    beat: 0.0,
                    bpm: 120.0,
                    curve: prowave_engine::TempoCurve::Step,
                },
                TempoEvent {
                    beat: 16.0,
                    bpm: 96.0,
                    curve: prowave_engine::TempoCurve::Step,
                },
            ],
            vec![],
        ))),
        Command::SetMetronome { on: true },
        Command::Play,
    ];
    for c in cmds {
        let _ = tx.push(c);
    }
    let synth = SynthPatch {
        wave_a: Wave::Saw,
        wave_b: Wave::Square,
        level_b: 0.6,
        semi_b: 7.0,
        detune_b: 8.0,
        cutoff: 2_000.0,
        resonance: 0.4,
        filter_env: 0.8,
        vibrato_hz: 5.0,
        vibrato_cents: 12.0,
        sustain: 0.7,
        ..SynthPatch::default()
    };
    for t in 0..tracks {
        let patch = if t == 0 {
            SynthPatch {
                kit: Kit::Acoustic,
                ..SynthPatch::default()
            }
        } else {
            synth
        };
        let _ = tx.push(Command::SetTrackPatch {
            track: t as u16,
            patch,
        });
    }

    let mut buf = vec![0f32; BLOCK * 2];
    let blocks = SECONDS * SR as usize / BLOCK;
    let budget = BLOCK as f64 / SR as f64;
    let mut worst = 0f64;
    let start = Instant::now();
    for _ in 0..blocks {
        let t = Instant::now();
        core.process(&mut buf);
        worst = worst.max(t.elapsed().as_secs_f64() / budget);
    }
    let total = start.elapsed().as_secs_f64();
    println!(
        "{SECONDS} s of audio, {BLOCK}-frame blocks @ {SR} Hz, 48 voices busy: \
         {:.2} ms render, average load {:.1}%, worst block {:.1}%",
        total * 1000.0,
        total / SECONDS as f64 * 100.0,
        worst * 100.0
    );
}
