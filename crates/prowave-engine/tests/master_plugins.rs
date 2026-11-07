//! Plug-in effects on the master bus (Stereo Out): they take every block of the summed mix,
//! in order, before the master fader, without adding a block of delay; they can be passed over
//! (Low Latency Mode), automated, and changed while the engine plays, all without allocating
//! on the audio thread.
use prowave_engine::{
    AutoLane, AutoPoint, AutoTarget, BlockProcessor, Command, EngineCore, Garbage, PluginTrack,
    Shared, Song, StateReader, TrackMix, MASTER_TRACK,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const SR: u32 = 48_000;
const MAX_FRAMES: usize = 1024;

/// A stub effect: `out = in * mul + add`, with parameter 0 setting `mul`. Counts its blocks
/// and frames, and says it delays the sound by `latency` seconds (it does not really).
struct Stub {
    mul: f32,
    add: f32,
    latency: f64,
    blocks: Arc<AtomicUsize>,
    frames: Arc<AtomicUsize>,
}

impl Stub {
    fn new(mul: f32, add: f32) -> Self {
        Self {
            mul,
            add,
            latency: 0.0,
            blocks: Arc::new(AtomicUsize::new(0)),
            frames: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl BlockProcessor for Stub {
    fn midi(&mut self, _status: u8, _d1: u8, _d2: u8, _offset: u32) {}
    fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(out.0.len(), Ordering::Relaxed);
        for i in 0..out.0.len() {
            out.0[i] = input.0[i] * self.mul + self.add;
            out.1[i] = input.1[i] * self.mul + self.add;
        }
    }
    fn latency_seconds(&self) -> f64 {
        self.latency
    }
    fn set_parameter(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.mul = value;
        }
    }
}

/// An engine playing two held notes on a track, plus the test tone, at unity master gain.
fn rig() -> (
    EngineCore,
    rtrb::Producer<Command>,
    rtrb::Consumer<Garbage>,
    StateReader,
) {
    let (mut tx, rx) = rtrb::RingBuffer::new(32);
    let (gtx, grx) = rtrb::RingBuffer::new(32);
    let mut core = EngineCore::new(SR, rx, gtx, Arc::new(Shared::new()));
    let state = core.take_state_reader().expect("state reader");
    let song = Song::new(vec![], vec![TrackMix::default()]);
    for cmd in [
        Command::SetMasterGain { linear: 1.0 },
        Command::SetSong(Box::new(song)),
        Command::SetTestTone {
            hz: 220.0,
            gain: 0.1,
        },
        Command::NoteOn {
            pitch: 60,
            vel: 80,
            track: 0,
        },
        Command::NoteOn {
            pitch: 67,
            vel: 80,
            track: 0,
        },
    ] {
        assert!(tx.push(cmd).is_ok());
    }
    (core, tx, grx, state)
}

fn master(effects: Vec<Box<dyn BlockProcessor>>) -> Command {
    Command::SetMasterPlugins(Some(Box::new(PluginTrack::new(None, effects, MAX_FRAMES))))
}

/// Renders the same blocks on both engines (sizes vary, as a device's may) and returns the
/// frames of each.
fn render(a: &mut EngineCore, b: &mut EngineCore, blocks: &[usize]) -> (Vec<f32>, Vec<f32>) {
    let (mut oa, mut ob) = (Vec::new(), Vec::new());
    for &n in blocks {
        let mut x = vec![0.0f32; n * 2];
        let mut y = vec![0.0f32; n * 2];
        assert_no_alloc::assert_no_alloc(|| a.process(&mut x));
        assert_no_alloc::assert_no_alloc(|| b.process(&mut y));
        oa.extend_from_slice(&x);
        ob.extend_from_slice(&y);
    }
    (oa, ob)
}

fn assert_mapped(reference: &[f32], got: &[f32], f: impl Fn(f32) -> f32, what: &str) {
    assert_eq!(reference.len(), got.len());
    assert!(
        reference.iter().any(|x| x.abs() > 0.05),
        "{what}: the mix has sound"
    );
    for (i, (r, g)) in reference.iter().zip(got).enumerate() {
        assert!(
            (f(*r) - g).abs() < 1e-5,
            "{what}: sample {i}: {g} vs {}",
            f(*r)
        );
    }
}

#[test]
fn master_plugins_take_every_block_of_the_mix_in_order_with_no_delay() {
    let (mut plain, _ptx, _pg, _ps) = rig();
    let (mut with, mut tx, _g, _s) = rig();
    let first = Stub::new(0.5, 0.0);
    let (blocks, frames) = (first.blocks.clone(), first.frames.clone());
    assert!(tx
        .push(master(vec![Box::new(first), Box::new(Stub::new(1.0, 0.1))]))
        .is_ok());
    let sizes = [64, 128, 480, 1024, 7, 256];
    let (a, b) = render(&mut plain, &mut with, &sizes);
    // Halved, then raised: in that order, at the same frame (no block of delay).
    assert_mapped(&a, &b, |x| x * 0.5 + 0.1, "halve then add");
    assert_eq!(blocks.load(Ordering::Relaxed), sizes.len(), "every block");
    assert_eq!(frames.load(Ordering::Relaxed), sizes.iter().sum::<usize>());

    // The other order gives the other answer.
    assert!(tx
        .push(master(vec![
            Box::new(Stub::new(1.0, 0.1)),
            Box::new(Stub::new(0.5, 0.0)),
        ]))
        .is_ok());
    let (a, b) = render(&mut plain, &mut with, &[256, 256]);
    assert_mapped(&a, &b, |x| (x + 0.1) * 0.5, "add then halve");
}

#[test]
fn passed_over_plugins_do_nothing_and_their_delay_goes() {
    let (mut plain, _ptx, _pg, _ps) = rig();
    let (mut with, mut tx, mut garbage, mut state) = rig();
    let mut slow = Stub::new(0.5, 0.0);
    slow.latency = 0.005;
    assert!(tx
        .push(master(vec![Box::new(slow), Box::new(Stub::new(1.0, 0.1))]))
        .is_ok());
    render(&mut plain, &mut with, &[128]);
    assert_eq!(
        state.read().master_latency_us,
        5000,
        "the delay is reported"
    );

    // Low Latency Mode passes over the slow one: the other still runs.
    assert!(tx.push(Command::SetMasterPluginsOff(0b01)).is_ok());
    let (a, b) = render(&mut plain, &mut with, &[128, 128]);
    assert_mapped(&a, &b, |x| x + 0.1, "the first passed over");
    assert_eq!(state.read().master_latency_us, 0);

    // Both passed over: the mix goes out as it is.
    assert!(tx.push(Command::SetMasterPluginsOff(0b11)).is_ok());
    let (a, b) = render(&mut plain, &mut with, &[128, 128]);
    assert_mapped(&a, &b, |x| x, "both passed over");

    // Taken away: the old ones go back to be freed off the audio thread.
    assert!(tx.push(Command::SetMasterPlugins(None)).is_ok());
    let (a, b) = render(&mut plain, &mut with, &[64]);
    assert_mapped(&a, &b, |x| x, "none");
    let mut freed = 0;
    while let Ok(g) = garbage.pop() {
        freed += usize::from(matches!(g, Garbage::Plugins(_)));
    }
    assert_eq!(freed, 1);
}

#[test]
fn a_master_plugin_applies_its_gain_to_an_offline_render_before_the_fader() {
    let (mut plain, mut ptx, _pg, _ps) = rig();
    let (mut with, mut tx, _g, _s) = rig();
    for t in [&mut ptx, &mut tx] {
        assert!(t.push(Command::SetMasterGain { linear: 0.5 }).is_ok());
    }
    assert!(tx
        .push(master(vec![Box::new(Stub::new(0.25, 0.0))]))
        .is_ok());
    // A second of audio, rendered as fast as it goes.
    let blocks = vec![512usize; SR as usize / 512];
    let (a, b) = render(&mut plain, &mut with, &blocks);
    assert_mapped(&a, &b, |x| x * 0.25, "a quarter of the mix");
    let peak = |v: &[f32]| v.iter().fold(0f32, |m, x| m.max(x.abs()));
    assert!((peak(&b) - peak(&a) * 0.25).abs() < 1e-5);
}

#[test]
fn a_master_plugin_parameter_follows_its_automation_lane() {
    let (mut plain, _ptx, _pg, _ps) = rig();
    let (mut with, mut tx, _g, _s) = rig();
    assert!(tx.push(master(vec![Box::new(Stub::new(1.0, 0.0))])).is_ok());
    // Parameter 0 of the first master plug-in (slot 1; 0 would be an instrument), its range
    // 0..=2, held at a quarter of the way: 0.5.
    let lane = AutoLane {
        lo: 0.0,
        hi: 2.0,
        ..AutoLane::new(
            MASTER_TRACK,
            AutoTarget::Plugin { slot: 1, param: 0 },
            vec![AutoPoint::new(0.0, 0.25)],
        )
    }
    .sanitized();
    assert!(tx.push(Command::SetAutomation(vec![lane])).is_ok());
    // The first reading lands at frame 0 of the first block.
    let (a, b) = render(&mut plain, &mut with, &[256, 256]);
    assert_mapped(&a, &b, |x| x * 0.5, "the lane's value");

    // New instances learn the lane's value at once.
    assert!(tx.push(master(vec![Box::new(Stub::new(1.0, 0.0))])).is_ok());
    let (a, b) = render(&mut plain, &mut with, &[256]);
    assert_mapped(&a, &b, |x| x * 0.5, "a new instance");

    // A native knob edit reaches the master plug-in too (no lane rules it once lanes go).
    assert!(tx.push(Command::SetAutomation(Vec::new())).is_ok());
    assert!(tx
        .push(Command::SetPluginParam {
            track: MASTER_TRACK,
            effect: 0,
            index: 0,
            value: 2.0,
        })
        .is_ok());
    let (a, b) = render(&mut plain, &mut with, &[256]);
    assert_mapped(&a, &b, |x| x * 2.0, "a knob edit");
}
