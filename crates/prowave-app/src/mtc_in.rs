//! Chasing MIDI Time Code (Logic Pro: Synchronization > External sync). With a MIDI input port
//! chosen for MTC the transport follows the time code that arrives there: it starts when the
//! quarter frames start, jumps to a full-frame message, stops when they stop, and jumps again if
//! it ends up more than a few frames from the sender (the two clocks never run at exactly the
//! same speed; this is "chase", not a sample-locked slave).

use crate::model::Project;
use midir::{Ignore, MidiInput, MidiInputConnection};
use prowave_engine::{MtcDecoder, MtcEvent};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::{Duration, Instant};

/// How long the time code may be silent before the transport stops.
pub const STALL: Duration = Duration::from_millis(300);
/// How far, in seconds, the playhead may drift from the sender's before it jumps: about
/// one and a half frames at 25 fps.
pub const TOLERANCE: f64 = 0.06;
/// How often to try again to open a port that could not be opened.
const RETRY: Duration = Duration::from_secs(3);
/// Time codes kept waiting for the app.
const QUEUE: usize = 1024;

/// The names of the MIDI input ports on this computer, without the system's "Through" port.
pub fn ports() -> Vec<String> {
    let Ok(input) = MidiInput::new(crate::APP_NAME) else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .filter(|n| !n.to_lowercase().contains("through"))
        .collect()
}

/// The connection to the port MTC comes from, and the time codes read from it.
pub struct MtcIn {
    connection: Option<MidiInputConnection<()>>,
    /// The port `connection` is to (or was last tried for).
    port: Option<String>,
    tx: SyncSender<(MtcEvent, Instant)>,
    rx: Receiver<(MtcEvent, Instant)>,
    last_try: Option<Instant>,
    ctx: egui::Context,
}

impl MtcIn {
    pub fn new(ctx: egui::Context) -> Self {
        // Bounded: a device that floods the port cannot fill the memory while the app is busy.
        let (tx, rx) = sync_channel(QUEUE);
        Self {
            connection: None,
            port: None,
            tx,
            rx,
            last_try: None,
            ctx,
        }
    }

    /// Call once per frame with the project's MTC input port: connects, reconnects when the
    /// port changed, disconnects when there is none.
    pub fn sync(&mut self, wanted: Option<&str>) {
        if self.port.as_deref() != wanted {
            self.connection = None;
            self.port = wanted.map(str::to_string);
            self.last_try = None;
        }
        if self.connection.is_none()
            && self.port.is_some()
            && self.last_try.is_none_or(|t| t.elapsed() >= RETRY)
        {
            self.connect();
        }
    }

    fn connect(&mut self) {
        self.last_try = Some(Instant::now());
        let Some(name) = self.port.clone() else {
            return;
        };
        let Ok(mut input) = MidiInput::new("Pro Wave MTC") else {
            return;
        };
        // Time code arrives as system messages, which midir would drop by default.
        input.ignore(Ignore::ActiveSense);
        let Some(port) = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).is_ok_and(|n| n == name))
        else {
            return;
        };
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let mut decoder = MtcDecoder::new();
        match input.connect(
            &port,
            "prowave-mtc-in",
            move |_stamp, msg, _| {
                if let Some(event) = decoder.feed(msg) {
                    let _ = tx.try_send((event, Instant::now()));
                    ctx.request_repaint();
                }
            },
            (),
        ) {
            Ok(connection) => {
                tracing::info!("MTC input: {name}");
                self.connection = Some(connection);
            }
            Err(e) => tracing::warn!("cannot open MTC input {name}: {e}"),
        }
    }

    /// The time codes that arrived since the last call.
    pub fn poll(&mut self, out: &mut Vec<(MtcEvent, Instant)>) {
        while let Ok(e) = self.rx.try_recv() {
            out.push(e);
        }
    }
}

/// Moves the transport to follow the time code that arrives.
#[derive(Default)]
pub struct Chase {
    /// The sender has been running; when it goes quiet the transport stops.
    running: bool,
    last_event: Option<Instant>,
}

impl Chase {
    /// Whether time code is arriving (or has just stopped), so the app should look again soon
    /// to notice that it stopped.
    pub fn is_following(&self) -> bool {
        self.running
    }

    /// Applies `events` (oldest first, with the time each arrived) to the project at `now`.
    pub fn update(&mut self, p: &mut Project, events: &[(MtcEvent, Instant)], now: Instant) {
        if p.mtc_in.is_none() {
            self.running = false;
            return;
        }
        for &(event, at) in events {
            self.apply(p, event, at, now);
        }
        if self.running
            && self
                .last_event
                .is_none_or(|t| now.duration_since(t) > STALL)
        {
            self.running = false;
            p.playing = false;
        }
    }

    fn apply(&mut self, p: &mut Project, event: MtcEvent, at: Instant, now: Instant) {
        let (frames, rate, running) = match event {
            MtcEvent::Running { frames, rate } => (frames, rate, true),
            MtcEvent::Locate { frames, rate } => (frames, rate, false),
        };
        // A time code with another number of frames to the second than the project's cannot be
        // placed on its timeline.
        if rate.nominal_fps() != p.smpte_rate.nominal_fps() {
            return;
        }
        // The message is older by the time the app took to see it.
        let late = now.saturating_duration_since(at).as_secs_f64();
        let secs = (frames - p.smpte_offset_frames()) as f64 / p.smpte_rate.frames_per_second()
            + if running { late } else { 0.0 };
        let map = p.tempo_map();
        let target = map.beat_at(secs).max(0.0);
        if !running {
            // A locate: wherever the sender is, the playhead goes there.
            p.position_beats = target;
            return;
        }
        self.running = true;
        self.last_event = Some(at);
        if secs < 0.0 {
            // Before the project starts: wait at the start for the time code to arrive.
            p.playing = false;
            p.position_beats = 0.0;
            return;
        }
        if !p.playing {
            p.position_beats = target;
            p.playing = true;
        } else if (map.seconds_at(p.position_beats) - secs).abs() > TOLERANCE {
            p.position_beats = target;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prowave_engine::SmpteRate;

    fn project() -> Project {
        let mut p = Project::empty();
        p.tempo = 120.0;
        p.smpte_rate = SmpteRate::Fps25;
        p.smpte_start = [1, 0, 0, 0];
        p.mtc_in = Some("Port".into());
        p.position_beats = 0.0;
        p
    }

    /// 01:00:00:00 at 25 fps.
    const START: i64 = 25 * 3600;

    fn running(frames: i64) -> MtcEvent {
        MtcEvent::Running {
            frames,
            rate: SmpteRate::Fps25,
        }
    }

    #[test]
    fn the_transport_starts_where_the_time_code_is() {
        let (mut p, mut chase) = (project(), Chase::default());
        let t0 = Instant::now();
        // 4 seconds in: beat 8 at 120 bpm.
        chase.update(&mut p, &[(running(START + 100), t0)], t0);
        assert!(p.playing);
        assert!(
            (p.position_beats - 8.0).abs() < 1e-9,
            "{}",
            p.position_beats
        );
        assert!(chase.is_following());
    }

    #[test]
    fn a_message_that_took_a_while_to_arrive_is_made_up_for() {
        let (mut p, mut chase) = (project(), Chase::default());
        let t0 = Instant::now();
        let later = t0 + Duration::from_millis(100);
        chase.update(&mut p, &[(running(START + 100), t0)], later);
        // 100 ms is 0.2 beat at 120 bpm.
        assert!(
            (p.position_beats - 8.2).abs() < 1e-6,
            "{}",
            p.position_beats
        );
    }

    #[test]
    fn small_differences_are_left_and_large_ones_jump() {
        let (mut p, mut chase) = (project(), Chase::default());
        let t0 = Instant::now();
        chase.update(&mut p, &[(running(START + 100), t0)], t0);
        // The engine runs a little ahead (0.04 s = 0.08 beat): no jump.
        p.position_beats += 0.08;
        let t1 = t0 + Duration::from_millis(100);
        chase.update(&mut p, &[(running(START + 100), t1)], t1);
        assert!((p.position_beats - 8.08).abs() < 1e-9);
        // A second ahead: it jumps back.
        p.position_beats += 2.0;
        chase.update(&mut p, &[(running(START + 100), t1)], t1);
        assert!((p.position_beats - 8.0).abs() < 1e-9);
    }

    #[test]
    fn silence_stops_the_transport_and_a_full_frame_only_locates() {
        let (mut p, mut chase) = (project(), Chase::default());
        let t0 = Instant::now();
        chase.update(&mut p, &[(running(START + 25), t0)], t0);
        assert!(p.playing);
        chase.update(&mut p, &[], t0 + Duration::from_millis(200));
        assert!(p.playing, "not silent for long enough yet");
        chase.update(&mut p, &[], t0 + Duration::from_millis(400));
        assert!(!p.playing && !chase.is_following());
        // The sender stopped at 10 seconds: a full frame moves the playhead, no start.
        let locate = MtcEvent::Locate {
            frames: START + 250,
            rate: SmpteRate::Fps25,
        };
        chase.update(&mut p, &[(locate, t0)], t0 + Duration::from_millis(500));
        assert!(!p.playing);
        assert!((p.position_beats - 20.0).abs() < 1e-9);
    }

    #[test]
    fn before_the_project_start_it_waits() {
        let (mut p, mut chase) = (project(), Chase::default());
        let t0 = Instant::now();
        // One second before 01:00:00:00.
        chase.update(&mut p, &[(running(START - 25), t0)], t0);
        assert!(!p.playing);
        assert_eq!(p.position_beats, 0.0);
        assert!(chase.is_following(), "still watching for the start");
        let t1 = t0 + Duration::from_millis(100);
        chase.update(&mut p, &[(running(START), t1)], t1);
        assert!(p.playing);
    }

    #[test]
    fn other_frame_rates_and_a_project_without_a_port_are_ignored() {
        let (mut p, mut chase) = (project(), Chase::default());
        let t0 = Instant::now();
        let other = MtcEvent::Running {
            frames: START + 100,
            rate: SmpteRate::Fps24,
        };
        chase.update(&mut p, &[(other, t0)], t0);
        assert!(!p.playing && !chase.is_following());
        p.mtc_in = None;
        chase.update(&mut p, &[(running(START + 100), t0)], t0);
        assert!(!p.playing);
        // 29.97 non-drop and 30 count the same labels.
        let mut q = project();
        q.smpte_rate = SmpteRate::Fps29_97NonDrop;
        q.smpte_start = [0, 0, 0, 0];
        let thirty = MtcEvent::Running {
            frames: 30,
            rate: SmpteRate::Fps30,
        };
        Chase::default().update(&mut q, &[(thirty, t0)], t0);
        assert!(q.playing);
        assert!((q.position_beats - 2.0 * 30.0 * 1001.0 / 30_000.0).abs() < 1e-9);
    }
}
