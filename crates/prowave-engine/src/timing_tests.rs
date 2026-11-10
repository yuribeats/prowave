//! Where the click, a region's beat and a transport-following instrument come out, to the
//! sample, with the delay compensation the app plans (the timing audit of 2026-11-09): the
//! click waits for the output delay like every track, the transport starts with a click on
//! its beat, and a live track's instrument follows the transport as late as its regions.
#![allow(clippy::unwrap_used)]

use crate::core::{Command, EngineCore, NoteEvent, Shared, Song, TrackMix};
use crate::plugin_track::tests::Fake;
use crate::plugin_track::PluginTrack;
use crate::prerender::{channel, plugin_host_delay, Prerender};
use crate::tempo_map::TempoMap;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;

/// The app's default Process Buffer.
const PROCESS: u32 = 256;
/// 1-based bars compared.
const BARS: [u32; 3] = [2, 5, 9];

struct Rig {
    core: EngineCore,
    tx: Producer<Command>,
    workers: Vec<Prerender>,
    block: usize,
    _garbage: Consumer<crate::core::Garbage>,
}

impl Rig {
    fn new(sr: u32, block: usize) -> Self {
        let (tx, rx) = RingBuffer::new(256);
        let (gtx, grx) = RingBuffer::new(64);
        let shared = Arc::new(Shared::new());
        let mut core = EngineCore::new(sr, rx, gtx, shared.clone());
        let mut workers = Vec::new();
        for _ in 0..2 {
            let (port, w, _trash) = channel(shared.clone());
            core.set_worker(port);
            workers.push(w);
        }
        let mut r = Self {
            core,
            tx,
            workers,
            block,
            _garbage: grx,
        };
        r.send(Command::SetMasterGain { linear: 1.0 });
        r
    }

    fn send(&mut self, c: Command) {
        assert!(self.tx.push(c).is_ok());
    }

    /// `frames` frames in blocks of the rig's size, running the playback threads after each;
    /// the left channel.
    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames);
        let mut buf = vec![0f32; self.block * 2];
        let mut left = frames;
        while left > 0 {
            let n = left.min(self.block);
            self.core.set_device_time(self.block as u32, 0, 0);
            let (core, workers) = (&mut self.core, &mut self.workers);
            assert_no_alloc::assert_no_alloc(|| {
                core.process(&mut buf[..n * 2]);
                for w in workers.iter_mut() {
                    w.step();
                }
            });
            out.extend(buf[..n * 2].iter().step_by(2));
            left -= n;
        }
        out
    }
}

/// The first sample at or after `from` that is not silent.
fn onset(x: &[f32], from: usize) -> Option<usize> {
    (from..x.len()).find(|&i| x[i].abs() > 1e-9)
}

/// The onset nearest after 30 ms before each bar in `BARS`.
fn bar_onsets(x: &[f32], map: &TempoMap, sr: u32) -> Vec<Option<usize>> {
    BARS.iter()
        .map(|&b| {
            let exact = map.seconds_at(map.beat_of_bar(b - 1)) * f64::from(sr);
            onset(x, (exact - 0.03 * f64::from(sr)).max(0.0) as usize)
        })
        .collect()
}

fn frames_for(map: &TempoMap, sr: u32) -> usize {
    (map.seconds_at(map.beat_of_bar(BARS[2])) * f64::from(sr)) as usize
}

/// A short note on track 0 at the start of every bar in `BARS`.
fn bar_notes(map: &TempoMap) -> Vec<NoteEvent> {
    BARS.iter()
        .map(|&b| {
            let beat = map.beat_of_bar(b - 1);
            NoteEvent {
                start_beat: beat,
                end_beat: beat + 0.25,
                pitch: 69,
                vel: 127,
                track: 0,
            }
        })
        .collect()
}

/// How track 0 sounds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    /// The built-in synth: no plug-ins, no delay.
    Synth,
    /// A plug-in instrument, pre-rendered or live.
    Instrument { live: bool },
    /// A plug-in instrument and a plug-in effect, pre-rendered or live.
    InstrumentFx { live: bool },
}

impl Shape {
    fn plugins(self) -> Option<(bool, bool)> {
        match self {
            Self::Synth => None,
            Self::Instrument { live } => Some((false, live)),
            Self::InstrumentFx { live } => Some((true, live)),
        }
    }
}

/// Sets up track 0 the way the app does (`engine_sync::pdc_plan_full` for one track): the
/// compensation level is the track's plug-in delay as if pre-rendered; a live track runs in
/// the I/O callback and its regions wait the difference (content delay). The output delay is
/// that level. Returns it.
fn set_up(r: &mut Rig, map: &TempoMap, shape: Shape, notes: Vec<NoteEvent>) -> usize {
    r.send(Command::SetTempoMap(Box::new(map.clone())));
    r.send(Command::SetSong(Box::new(Song::new(
        notes,
        vec![TrackMix::default()],
    ))));
    r.send(Command::SetProcessBuffer { frames: PROCESS });
    let io = r.block as u32;
    let Some((fx, live)) = shape.plugins() else {
        return 0;
    };
    let level = plugin_host_delay(true, fx, false, PROCESS, io);
    let actual = plugin_host_delay(true, fx, live, PROCESS, io);
    let effects: Vec<Box<dyn crate::plugin_track::BlockProcessor>> = if fx {
        vec![Box::new(Fake::halver())]
    } else {
        Vec::new()
    };
    let pt = PluginTrack::new(
        Some(Box::new(Fake::instrument())),
        effects,
        r.block.max(512),
    );
    r.send(Command::SetPlugins {
        track: 0,
        plugins: Some(Box::new(pt)),
    });
    r.send(Command::SetTrackLive { track: 0, live });
    r.send(Command::SetContentDelay {
        track: 0,
        frames: (level - actual) as u32,
    });
    r.send(Command::SetOutputDelay {
        strips: level as u32,
        master: level as u32,
    });
    level
}

/// The click with the metronome on and nothing else playing, and track 0's notes with the
/// metronome off: the onsets at each bar of each, and the output delay.
fn click_and_track(
    sr: u32,
    block: usize,
    map: &TempoMap,
    shape: Shape,
) -> (Vec<Option<usize>>, Vec<Option<usize>>, usize) {
    let frames = frames_for(map, sr);
    let mut r = Rig::new(sr, block);
    let delay = set_up(&mut r, map, shape, Vec::new());
    r.send(Command::SetMetronome { on: true });
    r.send(Command::Play);
    let click = bar_onsets(&r.render(frames), map, sr);
    let mut r = Rig::new(sr, block);
    set_up(&mut r, map, shape, bar_notes(map));
    r.send(Command::Play);
    let track = bar_onsets(&r.render(frames), map, sr);
    (click, track, delay)
}

#[test]
fn the_click_lands_on_the_same_sample_as_a_regions_beat() {
    let shapes = [
        Shape::Synth,
        Shape::Instrument { live: false },
        Shape::Instrument { live: true },
        Shape::InstrumentFx { live: false },
        Shape::InstrumentFx { live: true },
    ];
    println!("\nClick and region note onsets at bars {BARS:?}, in frames:");
    for sr in [44_100u32, 48_000] {
        for map in [
            TempoMap::constant(120.0, (4, 4)),
            TempoMap::constant(117.02, (4, 4)),
        ] {
            for block in [64usize, 127, 512] {
                for shape in shapes {
                    let (click, track, delay) = click_and_track(sr, block, &map, shape);
                    println!(
                        "  sr {sr} {:>6.2} bpm block {block:>3} {shape:?}: delay {delay}, \
                         click {click:?}, track {track:?}",
                        map.tempo_at(0.0)
                    );
                    assert!(click.iter().all(Option::is_some), "{click:?}");
                    assert_eq!(click, track, "sr {sr} block {block} {shape:?}");
                    // And both where the bar starts, after the delay (the transport's running
                    // sum of beats reaches a bar within a sample of the exact time).
                    for (k, &b) in BARS.iter().enumerate() {
                        let exact = map.seconds_at(map.beat_of_bar(b - 1)) * f64::from(sr);
                        let at = click[k].unwrap() as f64 - delay as f64;
                        assert!((at - exact).abs() < 1.01, "bar {b}: {at} vs {exact}");
                    }
                }
            }
        }
    }
}

/// The click's onsets in `x` (each the first sound after 30 ms of silence).
fn clicks(x: &[f32]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut quiet = usize::MAX / 2;
    for (i, v) in x.iter().enumerate() {
        if v.abs() > 1e-9 {
            if quiet >= 1440 {
                out.push(i);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    out
}

#[test]
fn playback_starting_on_a_beat_clicks_on_it() {
    let sr = 48_000;
    let map = TempoMap::constant(117.02, (4, 4));
    let beat = 60.0 / 117.02 * f64::from(sr);
    for delay in [0u32, 512] {
        for (from, first) in [(0.0, 0.0), (4.0, 0.0), (5.0, 0.0), (4.5, 0.5)] {
            let mut r = Rig::new(sr, 64);
            r.send(Command::SetTempoMap(Box::new(map.clone())));
            r.send(Command::SetOutputDelay {
                strips: delay,
                master: delay,
            });
            r.send(Command::SetMetronome { on: true });
            r.send(Command::SetPosition { beat: from });
            r.send(Command::Play);
            let c = clicks(&r.render(sr as usize));
            // From mid-beat the first click is on the next beat.
            let want = (first * beat).ceil() as usize + delay as usize;
            println!("  play from beat {from} (output delay {delay}): clicks at {c:?}");
            assert_eq!(c[0], want, "from {from}, delay {delay}");
            assert_eq!(c[1], (first * beat + beat).ceil() as usize + delay as usize);
        }
    }
}

#[test]
fn the_downbeat_after_a_count_in_clicks() {
    let sr = 48_000;
    let step = 24_000;
    for (metronome, delay) in [(true, 0u32), (true, 1024), (false, 0)] {
        let mut r = Rig::new(sr, 64);
        r.send(Command::SetTempoMap(Box::new(TempoMap::constant(
            120.0,
            (4, 4),
        ))));
        r.send(Command::SetOutputDelay {
            strips: delay,
            master: delay,
        });
        r.send(Command::SetMetronome { on: metronome });
        r.send(Command::CountIn {
            clicks: 4,
            beats_per_click: 1.0,
        });
        let c = clicks(&r.render(6 * step));
        println!("  count-in, metronome {metronome}, output delay {delay}: clicks at {c:?}");
        let d = delay as usize;
        let count: Vec<usize> = (0..4).map(|k| k * step + d).collect();
        assert_eq!(c[..4], count[..]);
        if metronome {
            // The downbeat the transport starts on, then the beats after it.
            assert_eq!(c[4..], [4 * step + d, 5 * step + d][..]);
        } else {
            assert_eq!(c.len(), 4);
        }
    }
}

/// How a third-party plug-in reads the host's transport.
#[derive(Clone, Copy, Debug)]
enum Reads {
    /// The block's transport as the engine gives it.
    Block,
    /// A VST 3 plug-in: its `ProcessContext` (`vst3::fill_context`).
    #[cfg(target_os = "macos")]
    Vst3,
    /// An Audio Unit: the host callbacks (`au_host_time`).
    #[cfg(target_os = "macos")]
    Au,
}

/// A plug-in that plays the beat the host says each frame is at (beat / 1000; -1 while the
/// host says it is stopped), as an instrument or as an effect.
struct Reporter {
    reads: Reads,
    sr: f64,
    playing: bool,
    beat: f64,
    tempo: f64,
    #[cfg(target_os = "macos")]
    context: Box<vst3::Steinberg::Vst::ProcessContext>,
}

impl Reporter {
    fn new(reads: Reads, sr: u32) -> Self {
        Self {
            reads,
            sr: f64::from(sr),
            playing: false,
            beat: 0.0,
            tempo: 120.0,
            // SAFETY: ProcessContext is a plain C struct of numbers; all zeros is valid.
            #[cfg(target_os = "macos")]
            context: Box::new(unsafe { std::mem::zeroed() }),
        }
    }
}

impl crate::plugin_track::BlockProcessor for Reporter {
    fn midi(&mut self, _: u8, _: u8, _: u8, _: u32) {}
    fn process(&mut self, _input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        for i in 0..out.0.len().min(out.1.len()) {
            let v = if self.playing {
                ((self.beat + i as f64 / self.sr * self.tempo / 60.0) / 1000.0) as f32
            } else {
                -1.0
            };
            out.0[i] = v;
            out.1[i] = v;
        }
    }
    fn latency_seconds(&self) -> f64 {
        0.0
    }
    fn block_transport(&mut self, t: &crate::plugin_track::BlockTransport) {
        match self.reads {
            Reads::Block => {
                (self.playing, self.beat, self.tempo) = (t.playing, t.beat, t.tempo);
            }
            #[cfg(target_os = "macos")]
            Reads::Vst3 => {
                use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::kPlaying;
                let c = &mut *self.context;
                crate::vst3::fill_context(t, self.sr, 0, c);
                self.playing = c.state & kPlaying != 0;
                self.beat = c.projectTimeMusic;
                self.tempo = c.tempo;
            }
            #[cfg(target_os = "macos")]
            Reads::Au => {
                let (beat, tempo) = crate::au_host_time::beat_and_tempo(t);
                self.playing = crate::au_host_time::transport_state(t).playing;
                (self.beat, self.tempo) = (beat, tempo);
            }
        }
    }
}

/// The beat a `Reporter` on track 0 says each output frame is at, the track live or not.
fn reported_beats(
    sr: u32,
    block: usize,
    map: &TempoMap,
    reads: Reads,
    instrument: bool,
    live: bool,
) -> Vec<f32> {
    let mut r = Rig::new(sr, block);
    let io = block as u32;
    let level = plugin_host_delay(instrument, !instrument, false, PROCESS, io);
    let actual = plugin_host_delay(instrument, !instrument, live, PROCESS, io);
    let rep: Box<dyn crate::plugin_track::BlockProcessor> = Box::new(Reporter::new(reads, sr));
    let pt = if instrument {
        PluginTrack::new(Some(rep), Vec::new(), block.max(512))
    } else {
        PluginTrack::new(None, vec![rep], block.max(512))
    };
    for c in [
        Command::SetTempoMap(Box::new(map.clone())),
        Command::SetSong(Box::new(Song::new(vec![], vec![TrackMix::default()]))),
        Command::SetProcessBuffer { frames: PROCESS },
        Command::SetPlugins {
            track: 0,
            plugins: Some(Box::new(pt)),
        },
        Command::SetTrackLive { track: 0, live },
        Command::SetContentDelay {
            track: 0,
            frames: (level - actual) as u32,
        },
        Command::SetOutputDelay {
            strips: level as u32,
            master: level as u32,
        },
        Command::Play,
    ] {
        r.send(c);
    }
    r.render(sr as usize * 2)
}

/// A third-party plug-in on a live track reads the transport as late as its regions wait,
/// so for every output sample it says the same beat as when the track is not live.
#[test]
fn a_live_plug_in_reads_the_same_beat_as_a_pre_rendered_one() {
    #[allow(unused_mut)]
    let mut reads = vec![Reads::Block];
    #[cfg(target_os = "macos")]
    reads.extend([Reads::Vst3, Reads::Au]);
    let map = TempoMap::constant(117.02, (4, 4));
    println!("\nBeat a plug-in reads for the same output sample, live minus not live (frames):");
    for sr in [44_100u32, 48_000] {
        for block in [64usize, 127, 512] {
            for &how in &reads {
                for instrument in [true, false] {
                    let away = reported_beats(sr, block, &map, how, instrument, false);
                    let live = reported_beats(sr, block, &map, how, instrument, true);
                    let frames_per_beat = 60.0 / 117.02 * f64::from(sr);
                    // From 0.5 s, when both are in place and playing.
                    let worst = (sr as usize / 2..away.len())
                        .map(|i| {
                            assert!(away[i] > 0.0 && live[i] > 0.0, "{how:?} frame {i}");
                            (f64::from(live[i]) - f64::from(away[i])) * 1000.0 * frames_per_beat
                        })
                        .fold(0.0f64, |m, d| if d.abs() > m.abs() { d } else { m });
                    println!(
                        "  sr {sr} block {block:>3} {how:?} {}: worst {worst:+.3}",
                        if instrument { "instrument" } else { "effect" }
                    );
                    assert!(worst.abs() < 0.5, "{how:?} sr {sr} block {block}: {worst}");
                }
            }
        }
    }
}

/// How the transport starts in [`reported_from_start`].
#[derive(Clone, Copy, Debug)]
enum Start {
    Play,
    /// A count-in of two clicks a beat apart (ends in the middle of an I/O block).
    CountIn,
}

/// [`reported_beats`] from the first block the transport plays, started by `start` once the
/// plug-ins are in place; also the output delay and the frame the transport starts on.
fn reported_from_start(
    sr: u32,
    block: usize,
    map: &TempoMap,
    reads: Reads,
    instrument: bool,
    live: bool,
    start: Start,
) -> (Vec<f32>, usize, usize) {
    let mut r = Rig::new(sr, block);
    let io = block as u32;
    let level = plugin_host_delay(instrument, !instrument, false, PROCESS, io);
    let actual = plugin_host_delay(instrument, !instrument, live, PROCESS, io);
    let rep: Box<dyn crate::plugin_track::BlockProcessor> = Box::new(Reporter::new(reads, sr));
    let pt = if instrument {
        PluginTrack::new(Some(rep), Vec::new(), block.max(512))
    } else {
        PluginTrack::new(None, vec![rep], block.max(512))
    };
    let step = (60.0 / f64::from(map.tempo_at(0.0)) * f64::from(sr)).round() as usize;
    let (go, at) = match start {
        Start::Play => (Command::Play, 0),
        Start::CountIn => (
            Command::CountIn {
                clicks: 2,
                beats_per_click: 1.0,
            },
            2 * step,
        ),
    };
    for c in [
        Command::SetTempoMap(Box::new(map.clone())),
        Command::SetSong(Box::new(Song::new(vec![], vec![TrackMix::default()]))),
        Command::SetProcessBuffer { frames: PROCESS },
        Command::SetPlugins {
            track: 0,
            plugins: Some(Box::new(pt)),
        },
        Command::SetTrackLive { track: 0, live },
        Command::SetContentDelay {
            track: 0,
            frames: (level - actual) as u32,
        },
        Command::SetOutputDelay {
            strips: level as u32,
            master: level as u32,
        },
    ] {
        r.send(c);
    }
    // Stopped until the plug-ins are in place (moving to the playback thread fades them in).
    let settle = sr as usize / 4 / block * block;
    let mut x = r.render(settle);
    r.send(go);
    x.extend(r.render(at + sr as usize / 4));
    (x, level, settle + at)
}

/// The worst difference, in frames, between the beat `x` reports at each frame from `from` and
/// the beat the transport was at `delay` frames earlier, the transport playing from `start`
/// (`x` is the beat / 1000, as a [`Reporter`] plays it).
fn worst_from(x: &[f32], from: usize, delay: usize, start: usize, map: &TempoMap, sr: u32) -> f64 {
    let frames_per_beat = 60.0 / f64::from(map.tempo_at(0.0)) * f64::from(sr);
    (from..x.len())
        .map(|i| {
            let want = (i as f64 - (delay + start) as f64) / frames_per_beat;
            if x[i] < -0.5 {
                // Said stopped.
                f64::INFINITY
            } else {
                (f64::from(x[i]) * 1000.0 - want) * frames_per_beat
            }
        })
        .fold(0.0f64, |m, d| if d.abs() > m.abs() { d } else { m })
}

/// From the first frame the transport plays (after the output delay), a plug-in reads the
/// beat of that frame, live or not, after Play and after a count-in that ends inside a block:
/// it is never told "stopped" for the block the start falls in, nor first told a beat past
/// the start.
#[test]
fn a_plug_in_reads_the_start_beat_from_the_first_block() {
    #[allow(unused_mut)]
    let mut reads = vec![Reads::Block];
    #[cfg(target_os = "macos")]
    reads.extend([Reads::Vst3, Reads::Au]);
    let map = TempoMap::constant(117.02, (4, 4));
    println!("\nBeat a plug-in reads from the start, minus the exact one (frames):");
    for sr in [44_100u32, 48_000] {
        for block in [64usize, 127, 512] {
            for &how in &reads {
                for instrument in [true, false] {
                    for live in [false, true] {
                        for start in [Start::Play, Start::CountIn] {
                            let (x, delay, at) =
                                reported_from_start(sr, block, &map, how, instrument, live, start);
                            let worst = worst_from(&x, delay + at, delay, at, &map, sr);
                            println!(
                                "  sr {sr} block {block:>3} {how:?} {} live {live} {start:?}: \
                                 worst {worst:+.3}",
                                if instrument { "instrument" } else { "effect" }
                            );
                            assert!(
                                worst.abs() < 0.5,
                                "{how:?} sr {sr} block {block} live {live} {start:?}: {worst}"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// A plug-in on the Stereo Out hears the mix the whole output delay late, and an Aux strip's
/// plug-in hears its sends the strips' delay late: each reads the transport as late, so a
/// tempo-synced effect there stays on the beat of what it processes.
#[test]
fn stereo_out_and_aux_plug_ins_read_the_transport_as_late_as_their_input() {
    use crate::routing::Route;
    let sr = 48_000;
    let map = TempoMap::constant(117.02, (4, 4));
    for block in [64usize, 512] {
        for (strips, master) in [(0u32, 1024u32), (512, 1536)] {
            // The Stereo Out: its plug-ins process each block in place.
            let mut r = Rig::new(sr, block);
            let rep: Box<dyn crate::plugin_track::BlockProcessor> =
                Box::new(Reporter::new(Reads::Block, sr));
            for c in [
                Command::SetTempoMap(Box::new(map.clone())),
                Command::SetSong(Box::new(Song::new(vec![], vec![TrackMix::default()]))),
                Command::SetMasterPlugins(Some(Box::new(PluginTrack::new(
                    None,
                    vec![rep],
                    block.max(512),
                )))),
                Command::SetOutputDelay { strips, master },
                Command::Play,
            ] {
                r.send(c);
            }
            let x = r.render(sr as usize / 2);
            let m = master as usize;
            let worst = worst_from(&x, m, m, 0, &map, sr);
            println!("  block {block:>3} master delay {master}: Stereo Out worst {worst:+.3}");
            assert!(worst.abs() < 0.5, "Stereo Out, block {block}: {worst}");

            // An Aux strip (live: in the I/O callback, heard a block after its input).
            let mut r = Rig::new(sr, block);
            let rep: Box<dyn crate::plugin_track::BlockProcessor> =
                Box::new(Reporter::new(Reads::Block, sr));
            let aux = Route {
                bus_in: 1,
                ..Route::default()
            };
            for c in [
                Command::SetTempoMap(Box::new(map.clone())),
                Command::SetSong(Box::new(Song::new(vec![], vec![TrackMix::default()]))),
                Command::SetProcessBuffer { frames: PROCESS },
                Command::SetRoute {
                    track: 0,
                    route: aux,
                },
                Command::SetPlugins {
                    track: 0,
                    plugins: Some(Box::new(PluginTrack::new(None, vec![rep], block.max(512)))),
                },
                Command::SetTrackLive {
                    track: 0,
                    live: true,
                },
                Command::SetOutputDelay { strips, master },
                Command::Play,
            ] {
                r.send(c);
            }
            let x = r.render(sr as usize / 2);
            let heard = plugin_host_delay(false, true, true, PROCESS, block as u32);
            let late = strips as usize + heard;
            let worst = worst_from(&x, late, late, 0, &map, sr);
            println!("  block {block:>3} strips delay {strips}: Aux worst {worst:+.3}");
            assert!(
                worst.abs() < 0.5,
                "Aux, block {block} strips {strips}: {worst}"
            );
        }
    }
}
