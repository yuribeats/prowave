//! Plays a short phrase on the default output device and prints transport/meter state.
use prowave_engine::{Command, Engine, NoteEvent, Song, TrackMix};
use std::{thread::sleep, time::Duration};

fn main() {
    let mut e = match Engine::start() {
        Ok(e) => e,
        Err(err) => {
            eprintln!("engine failed: {err}");
            std::process::exit(1);
        }
    };
    println!("device {} @ {} Hz", e.device_name(), e.sample_rate());
    let notes = [60u8, 64, 67, 72]
        .iter()
        .enumerate()
        .map(|(i, p)| NoteEvent {
            start_beat: i as f64,
            end_beat: i as f64 + 0.9,
            pitch: *p,
            vel: 100,
            track: 0,
        })
        .collect();
    e.send(Command::SetSong(Box::new(Song::new(
        notes,
        vec![TrackMix::default()],
    ))));
    e.send(Command::SetMetronome { on: true });
    e.send(Command::Play);
    for _ in 0..8 {
        sleep(Duration::from_millis(250));
        let (st, xruns) = (
            *e.state(),
            e.shared().xruns.load(std::sync::atomic::Ordering::Relaxed),
        );
        let (l, r) = st.peaks;
        println!(
            "beat {:.2} peak {:.2}/{:.2} xruns {}",
            st.position_beats, l, r, xruns
        );
    }
}
