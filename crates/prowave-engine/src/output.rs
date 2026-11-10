//! cpal output: owns the stream, the command producer and the shared state.
use crate::core::{
    dsp_load, AudioClip, ClipEdit, Command, EngineCore, EngineState, Garbage, MidiOutEvent, Shared,
    StateReader,
};
use crate::disk::{AudioError, AudioFile, DiskThread};
use crate::mtc::MtcMessage;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Error, ErrorKind, FromSample, OutputCallbackInfo, SampleFormat, SizedSample,
    Stream, StreamConfig, SupportedBufferSize,
};
use rtrb::{Producer, RingBuffer};
use std::fmt;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

const MAX_BLOCK_FRAMES: usize = 8192;
const GARBAGE_CAPACITY: usize = 32;

/// UI-side queue accounting. A credit is one free place in the garbage queue: an accepted
/// state replacement takes one, and gets it back when its old state has come back and been
/// freed here, or when the engine says it replaced nothing (`Shared::retired_empty`).
struct CommandQueue {
    tx: Producer<Command>,
    garbage: rtrb::Consumer<Garbage>,
    credits: usize,
    shared: Arc<Shared>,
    /// `Shared::retired_empty` as last read.
    empty_seen: u32,
}

impl CommandQueue {
    /// The UI end for an [`EngineCore`] made with the other ends of `tx` and `garbage` and
    /// with `shared`; `garbage` holds [`GARBAGE_CAPACITY`].
    fn new(tx: Producer<Command>, garbage: rtrb::Consumer<Garbage>, shared: Arc<Shared>) -> Self {
        let empty_seen = shared.retired_empty.load(Ordering::Relaxed);
        Self {
            tx,
            garbage,
            credits: GARBAGE_CAPACITY,
            shared,
            empty_seen,
        }
    }

    /// Frees what the engine sent back and takes the credits that are free again.
    fn collect(&mut self) {
        while self.garbage.pop().is_ok() {
            self.credits += 1;
        }
        // Read late is read low: a credit then comes back one call later, never too early.
        let empty = self.shared.retired_empty.load(Ordering::Relaxed);
        self.credits += empty.wrapping_sub(self.empty_seen) as usize;
        self.empty_seen = empty;
    }

    /// Whether a state-replacing command sent now is accepted. Nothing but this thread's
    /// own sends takes credits or room, so the answer holds until the next send.
    fn can_replace_state(&mut self) -> bool {
        self.collect();
        self.credits > 0 && !self.tx.is_full()
    }

    fn send(&mut self, cmd: Command) -> bool {
        self.collect();
        let retires = cmd.retires_state();
        if retires && self.credits == 0 {
            return false;
        }
        if self.tx.push(cmd).is_err() {
            return false;
        }
        if retires {
            self.credits -= 1;
        }
        true
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    #[test]
    fn reserves_reclamation_capacity_and_recovers_after_collection() {
        let (tx, mut rx) = RingBuffer::new(8);
        let (mut gtx, garbage) = RingBuffer::new(1);
        let mut queue = CommandQueue::new(tx, garbage, Arc::new(Shared::new()));
        queue.credits = 1;
        assert!(queue.send(Command::SetSong(Box::default())));
        assert!(!queue.send(Command::SetTempoMap(Box::default())));
        // Transport is still deliverable even when all reclamation credits are reserved.
        assert!(queue.send(Command::Stop));
        assert!(matches!(rx.pop(), Ok(Command::SetSong(_))));
        assert!(matches!(rx.pop(), Ok(Command::Stop)));
        assert!(gtx.push(Garbage::Song(Box::default())).is_ok());
        assert!(queue.send(Command::SetTempoMap(Box::default())));
        assert_eq!(queue.credits, 0);
        assert!(matches!(rx.pop(), Ok(Command::SetTempoMap(_))));
    }

    #[test]
    fn rejected_command_does_not_consume_a_reclamation_credit() {
        let (tx, mut rx) = RingBuffer::new(1);
        let (_gtx, garbage) = RingBuffer::new(1);
        let mut queue = CommandQueue::new(tx, garbage, Arc::new(Shared::new()));
        queue.credits = 1;
        assert!(queue.send(Command::Play));
        assert!(!queue.send(Command::SetNotes(vec![])));
        assert_eq!(queue.credits, 1);
        assert!(rx.pop().is_ok());
        assert!(queue.send(Command::SetNotes(vec![])));
        assert_eq!(queue.credits, 0);
    }

    use crate::core::Song;
    use crate::fx::Pdc;
    use crate::plugin_track::{BlockProcessor, PluginTrack};
    use std::sync::atomic::AtomicBool;

    /// The queue and the engine as [`Engine::start_device`] makes them, without a device.
    fn rig() -> (CommandQueue, EngineCore) {
        let (tx, rx) = RingBuffer::<Command>::new(1024);
        let (gtx, garbage) = RingBuffer::<Garbage>::new(GARBAGE_CAPACITY);
        let shared = Arc::new(Shared::new());
        let core = EngineCore::new(48_000, rx, gtx, shared.clone());
        (CommandQueue::new(tx, garbage, shared), core)
    }

    /// One audio callback, then a command that replaces nothing: the engine takes what was
    /// sent, and the queue collects what came back.
    fn settle(queue: &mut CommandQueue, core: &mut EngineCore) {
        let mut block = [0.0f32; 512];
        core.process(&mut block);
        assert!(queue.send(Command::SetMetronome { on: false }));
        core.process(&mut block);
    }

    /// A plug-in that does nothing and says when it is freed.
    struct Probe(Arc<AtomicBool>);

    impl BlockProcessor for Probe {
        fn midi(&mut self, _status: u8, _d1: u8, _d2: u8, _offset: u32) {}
        fn process(&mut self, _input: (&[f32], &[f32]), _out: (&mut [f32], &mut [f32])) {}
        fn latency_seconds(&self) -> f64 {
            0.0
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn plugin_track() -> Option<Box<PluginTrack>> {
        let probe: Box<dyn BlockProcessor> = Box::new(Probe(Arc::new(AtomicBool::new(false))));
        Some(Box::new(PluginTrack::new(None, vec![probe], 1024)))
    }

    fn sine_player() -> Option<Box<crate::soundfont::SfPlayer>> {
        let Ok(bank) =
            crate::soundfont::SoundFontBank::load(&crate::soundfont::test_font::sine_font())
        else {
            panic!("the test SoundFont loads");
        };
        let Some(preset) = bank.presets().first().cloned() else {
            panic!("a preset");
        };
        let Ok(player) = crate::soundfont::SfPlayer::new(&bank, &preset, 48_000.0) else {
            panic!("a player");
        };
        Some(Box::new(player))
    }

    /// Every command that takes a reclamation credit, by name.
    fn retiring(name: &str) -> Command {
        match name {
            "SetSong" => Command::SetSong(Box::new(Song::new(vec![], vec![]))),
            "SetNotes" => Command::SetNotes(vec![]),
            "SetControls" => Command::SetControls(vec![]),
            "SetTempoMap" => Command::SetTempoMap(Box::default()),
            "SetAutomation" => Command::SetAutomation(vec![]),
            "SetConvolver" => Command::SetConvolver {
                track: 0,
                slot: 0,
                conv: Box::new(crate::convolver::Convolver::new(&[[1.0, 0.0]])),
            },
            "SetInserts" => Command::SetInserts {
                track: 0,
                chain: Box::default(),
            },
            "SetMasterInserts" => Command::SetMasterInserts(Box::default()),
            "SetPdc" => Command::SetPdc {
                track: 0,
                pdc: Box::new(Pdc::new(64, 0)),
            },
            "SetPlugins" => Command::SetPlugins {
                track: 0,
                plugins: plugin_track(),
            },
            "SetMasterPlugins" => Command::SetMasterPlugins(plugin_track()),
            "SetInput" => {
                let (_tx, rx) = RingBuffer::<f32>::new(64);
                Command::SetInput(Some(Box::new(crate::monitor::InputFeed::new(
                    rx, 2, 48_000,
                ))))
            }
            "SetTrackSoundFont" => Command::SetTrackSoundFont {
                track: 0,
                player: sine_player(),
            },
            other => panic!("no such command: {other}"),
        }
    }

    const RETIRING: [&str; 13] = [
        "SetSong",
        "SetNotes",
        "SetControls",
        "SetTempoMap",
        "SetAutomation",
        "SetConvolver",
        "SetInserts",
        "SetMasterInserts",
        "SetPdc",
        "SetPlugins",
        "SetMasterPlugins",
        "SetInput",
        "SetTrackSoundFont",
    ];

    /// Each command that takes a credit, sent once to a new engine and once more: after the
    /// engine has run, the credit is back.
    #[test]
    fn every_reclamation_credit_comes_back_once_the_engine_has_run() {
        let mut first_time = Vec::new();
        let mut second_time = Vec::new();
        for name in RETIRING {
            let (mut queue, mut core) = rig();
            assert!(retiring(name).retires_state(), "{name} takes a credit");
            assert!(queue.send(retiring(name)));
            settle(&mut queue, &mut core);
            if queue.credits != GARBAGE_CAPACITY {
                first_time.push((name, queue.credits));
            }
            let before = queue.credits;
            assert!(queue.send(retiring(name)));
            settle(&mut queue, &mut core);
            if queue.credits != before {
                second_time.push((name, queue.credits));
            }
        }
        assert!(
            first_time.is_empty() && second_time.is_empty(),
            "credits of {GARBAGE_CAPACITY} not given back: sent to a new engine \
             {first_time:?}; sent again {second_time:?}"
        );
    }

    /// A project of 16 tracks where each gets its first effect and its first delay
    /// compensation, one audio callback after every command: notes can still be edited.
    #[test]
    fn edits_are_still_accepted_after_first_time_strip_commands() {
        let (mut queue, mut core) = rig();
        let mut accepted = 0;
        for track in 0..16u16 {
            for cmd in [
                Command::SetInserts {
                    track,
                    chain: Box::default(),
                },
                Command::SetPdc {
                    track,
                    pdc: Box::new(Pdc::new(64, 0)),
                },
            ] {
                accepted += usize::from(queue.send(cmd));
                settle(&mut queue, &mut core);
            }
        }
        // However long the engine runs afterwards.
        for _ in 0..100 {
            settle(&mut queue, &mut core);
        }
        let credits = queue.credits;
        let notes = queue.send(Command::set_notes(vec![]));
        let song = queue.send(Command::SetSong(Box::new(Song::new(vec![], vec![]))));
        assert!(
            notes && song,
            "after {accepted} accepted first-time strip commands the queue has {credits} \
             credits: SetNotes accepted {notes}, SetSong accepted {song}"
        );
    }

    /// Changing a track's SoundFont takes a credit and gives that one back, no more.
    #[test]
    fn replacing_a_soundfont_does_not_add_a_credit() {
        let (mut queue, mut core) = rig();
        for _ in 0..3 {
            let cmd = Command::SetTrackSoundFont {
                track: 0,
                player: sine_player(),
            };
            assert!(cmd.retires_state());
            assert!(queue.send(cmd));
            assert!(queue.credits < GARBAGE_CAPACITY);
            settle(&mut queue, &mut core);
            assert_eq!(queue.credits, GARBAGE_CAPACITY);
        }
        assert_eq!(
            queue.credits, GARBAGE_CAPACITY,
            "three SoundFonts on one track, two of them replacing one"
        );
    }

    /// The SoundFont player a new one replaces goes back to the UI thread to be freed there,
    /// also when the way back is full at that moment.
    #[test]
    fn a_replaced_soundfont_player_comes_back_when_the_way_back_is_full() {
        let (mut tx, rx) = RingBuffer::<Command>::new(64);
        let (gtx, mut garbage) = RingBuffer::<Garbage>::new(1);
        let mut core = EngineCore::new(48_000, rx, gtx, Arc::new(Shared::new()));
        let mut block = [0.0f32; 512];
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 0,
                player: sine_player(),
            })
            .is_ok());
        core.process(&mut block);
        // The old song fills the way back; the old player comes right after it.
        assert!(tx.push(Command::SetSong(Box::default())).is_ok());
        assert!(tx
            .push(Command::SetTrackSoundFont {
                track: 0,
                player: None,
            })
            .is_ok());
        let mut players = 0;
        for _ in 0..8 {
            core.process(&mut block);
            while let Ok(item) = garbage.pop() {
                players += usize::from(matches!(item, Garbage::SoundFont(_)));
            }
        }
        assert_eq!(players, 1, "SoundFont players that came back");
    }

    /// What the engine holds that a command can replace, as the test expects it.
    #[derive(Default)]
    struct Held {
        chains: [bool; 6],
        pdc: [bool; 6],
        plugins: [bool; 6],
        fonts: [bool; 6],
        master_chain: bool,
        master_plugins: bool,
        input: bool,
    }

    /// A long random run of every state-replacing command, first-time and replacing, to
    /// tracks that exist and to one that does not, in bursts and with the engine running in
    /// between: there are never more credits than places, the only refusals are for want of
    /// a credit, every old state comes back to be freed here, and once the engine has run
    /// all 32 credits are back.
    #[test]
    fn credits_all_come_back_after_a_long_random_run_of_state_replacements() {
        let (mut queue, mut core) = rig();
        let mut held = Held::default();
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rand = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as usize
        };
        let mut block = [0.0f32; 256];
        // Accepted commands, the old states expected back, refusals.
        let (mut accepted, mut expected, mut refused) = (0u32, 0u32, 0u32);
        for _ in 0..4000 {
            // One time in eight a track the engine does not have: what was sent comes back.
            let real = rand(8) > 0;
            let t = rand(6);
            let track = if real { t as u16 } else { u16::MAX };
            let some = rand(3) > 0;
            // (the command, whether something comes back for it, what to note when accepted)
            let kind = rand(13);
            let (cmd, back) = match kind {
                0 => (retiring("SetSong"), true),
                1 => (retiring("SetNotes"), true),
                2 => (retiring("SetControls"), true),
                3 => (retiring("SetTempoMap"), true),
                4 => (retiring("SetAutomation"), true),
                5 => (retiring("SetConvolver"), true),
                6 => (
                    Command::SetInserts {
                        track,
                        chain: Box::default(),
                    },
                    !real || held.chains[t],
                ),
                7 => (retiring("SetMasterInserts"), held.master_chain),
                8 => (
                    Command::SetPdc {
                        track,
                        pdc: Box::new(Pdc::new(rand(64), rand(64))),
                    },
                    !real || held.pdc[t],
                ),
                9 => (
                    Command::SetPlugins {
                        track,
                        plugins: some.then(plugin_track).flatten(),
                    },
                    if real { held.plugins[t] } else { some },
                ),
                10 => (
                    Command::SetMasterPlugins(some.then(plugin_track).flatten()),
                    held.master_plugins,
                ),
                11 => {
                    let feed = some.then(|| {
                        let (_tx, rx) = RingBuffer::<f32>::new(64);
                        Box::new(crate::monitor::InputFeed::new(rx, 2, 48_000))
                    });
                    (Command::SetInput(feed), held.input)
                }
                _ => (
                    Command::SetTrackSoundFont {
                        track,
                        player: some.then(sine_player).flatten(),
                    },
                    if real { held.fonts[t] } else { some },
                ),
            };
            assert!(cmd.retires_state());
            let before = queue.credits;
            if queue.send(cmd) {
                accepted += 1;
                expected += u32::from(back);
                match kind {
                    6 if real => held.chains[t] = true,
                    7 => held.master_chain = true,
                    8 if real => held.pdc[t] = true,
                    9 if real => held.plugins[t] = some,
                    10 => held.master_plugins = some,
                    11 => held.input = some,
                    12 if real => held.fonts[t] = some,
                    _ => {}
                }
            } else {
                refused += 1;
                assert_eq!(queue.credits, 0, "refused only for want of a credit");
                assert!(before <= GARBAGE_CAPACITY);
            }
            assert!(
                queue.credits <= GARBAGE_CAPACITY,
                "more credits than places"
            );
            assert_eq!(
                queue.can_replace_state(),
                queue.credits > 0,
                "the question and the send agree"
            );
            // Bursts: often several commands before the engine runs again.
            for _ in 0..[0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2][rand(12)] {
                // Nothing is made or freed in the audio callback (debug builds stop here).
                assert_no_alloc::assert_no_alloc(|| core.process(&mut block));
            }
        }
        for _ in 0..4 {
            settle(&mut queue, &mut core);
        }
        assert_eq!(queue.credits, GARBAGE_CAPACITY, "all credits are back");
        let empty = queue.shared.retired_empty.load(Ordering::Relaxed);
        assert_eq!(
            accepted - empty,
            expected,
            "old states that came back, of {accepted} commands ({refused} more refused)"
        );
        assert!(
            refused > 0 && expected > 1000 && empty > 100,
            "the run covers the cases: {refused} refused, {expected} back, {empty} empty"
        );
    }

    /// A refused command is freed inside `send`, with the plug-ins in it: whatever the caller
    /// kept to reach them (the app keeps a raw Audio Unit handle per plug-in) points at
    /// nothing from then on.
    #[test]
    fn a_refused_plugins_command_frees_its_plugins_before_send_returns() {
        let (tx, _rx) = RingBuffer::<Command>::new(8);
        let (_gtx, garbage) = RingBuffer::<Garbage>::new(1);
        // The one credit is out: an edit the engine has not run yet.
        let mut queue = CommandQueue::new(tx, garbage, Arc::new(Shared::new()));
        queue.credits = 1;
        assert!(queue.send(Command::SetNotes(vec![])));
        let freed = Arc::new(AtomicBool::new(false));
        let probe: Box<dyn BlockProcessor> = Box::new(Probe(freed.clone()));
        let plugins = Some(Box::new(PluginTrack::new(Some(probe), vec![], 1024)));
        assert!(!freed.load(Ordering::SeqCst));
        assert!(!queue.send(Command::SetPlugins { track: 0, plugins }));
        assert!(
            freed.load(Ordering::SeqCst),
            "the plug-in of the refused command is gone when send returns"
        );
    }
}

#[derive(Debug)]
pub enum EngineError {
    NoOutputDevice,
    UnsupportedSampleFormat(String),
    Backend(String),
    AudioFile(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOutputDevice => write!(f, "no audio output device available"),
            Self::UnsupportedSampleFormat(s) => write!(f, "unsupported sample format {s}"),
            Self::Backend(s) => write!(f, "audio backend error: {s}"),
            Self::AudioFile(s) => write!(f, "audio file error: {s}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<AudioError> for EngineError {
    fn from(error: AudioError) -> Self {
        Self::AudioFile(error.to_string())
    }
}

/// Decodes `file` off the audio thread and returns its maximum absolute sample value.
/// Use `20 * log10(peak)` to calculate the non-destructive gain for normalization.
pub fn peak_of(file: &AudioFile) -> Result<f32, EngineError> {
    file.peak().map_err(EngineError::from)
}

/// The running audio output. Dropping it stops the sound.
enum Output {
    Cpal(#[allow(dead_code)] Stream),
    /// Windows: our own WASAPI stream with the smallest period the audio engine allows.
    #[cfg(windows)]
    Wasapi(#[allow(dead_code)] crate::wasapi::LowLatencyStream),
    /// macOS: one Core Audio callback for input and output.
    #[cfg(target_os = "macos")]
    Duplex(#[allow(dead_code)] Box<crate::duplex::DuplexStream>),
}

/// macOS: the way to the input inside the duplex callback.
#[cfg(target_os = "macos")]
struct TapPort {
    tx: Producer<Option<Box<crate::capture::InputTap>>>,
    trash: rtrb::Consumer<Box<crate::capture::InputTap>>,
    channels: u16,
    name: String,
}

/// UI-thread handle. Not `Send` on some platforms (cpal `Stream`); keep it on the UI thread.
pub struct Engine {
    /// The audio output first: dropping the engine stops the callback before the playback
    /// thread.
    _output: Output,
    /// The playback thread that pre-renders the tracks that are not live (see
    /// [`crate::prerender`]), and the plug-ins it gives back to be freed here.
    _worker: Vec<crate::prerender::WorkerThread>,
    worker_trash: Vec<crate::prerender::WorkerTrash>,
    commands: CommandQueue,
    shared: Arc<Shared>,
    state: StateReader,
    midi_out: Option<rtrb::Consumer<MidiOutEvent>>,
    mtc_out: Option<rtrb::Consumer<MtcMessage>>,
    /// The window's end of the preview loop (see [`crate::audition`]).
    audition: Option<crate::audition::AuditionRemote>,
    sample_rate: u32,
    device_name: String,
    disk: DiskThread,
    #[cfg(target_os = "macos")]
    tap: Option<TapPort>,
    buffer_frames: u32,
}

/// The running playback threads: the I/O callback's ports to them (handed to the core),
/// the threads, and what they give back to be freed.
struct Worker {
    ports: Vec<crate::prerender::WorkerPort>,
    thread: Vec<crate::prerender::WorkerThread>,
    trash: Vec<crate::prerender::WorkerTrash>,
}

/// Starts the playback threads, one per core the I/O thread and the UI leave over (none if
/// the system will not start a thread: then every plug-in runs in the I/O callback).
fn start_worker(shared: &Arc<Shared>) -> Worker {
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let count = crate::prerender::playback_threads(cores);
    let mut w = Worker {
        ports: Vec::new(),
        thread: Vec::new(),
        trash: Vec::new(),
    };
    for _ in 0..count {
        let (mut port, prerender, trash) = crate::prerender::channel(shared.clone());
        match crate::prerender::spawn(prerender, &mut port, shared.workgroup_device.clone()) {
            Ok(thread) => {
                w.ports.push(port);
                w.thread.push(thread);
                w.trash.push(trash);
            }
            Err(e) => {
                tracing::warn!("cannot start a playback thread ({e})");
                break;
            }
        }
    }
    if w.ports.is_empty() {
        tracing::warn!("no playback thread: every plug-in runs in the I/O callback");
    } else {
        tracing::info!("{} playback threads", w.ports.len());
    }
    w
}

/// Renders the engine into a device buffer of any sample format and channel count. Lives on
/// the audio thread; never allocates after it is made.
pub(crate) struct Renderer {
    core: EngineCore,
    shared: Arc<Shared>,
    scratch: Vec<f32>,
    channels: usize,
    sample_rate: u32,
    /// The device sample time the next callback should start at (negative: unknown).
    next_sample_time: f64,
}

impl Renderer {
    fn new(core: EngineCore, shared: Arc<Shared>, channels: usize, sample_rate: u32) -> Self {
        Self {
            core,
            shared,
            // Pre-allocated once, before the stream starts; the callback never allocates.
            scratch: vec![0f32; MAX_BLOCK_FRAMES * 2],
            channels: channels.max(1),
            sample_rate,
            next_sample_time: -1.0,
        }
    }

    /// Fills `data` (interleaved, `channels` per frame). `output_latency_us` is the delay from
    /// now until the first frame is heard, if the device says.
    pub(crate) fn fill<T: SizedSample + FromSample<f32>>(
        &mut self,
        data: &mut [T],
        output_latency_us: u32,
    ) {
        self.fill_at(data, output_latency_us, 0);
    }

    /// [`Renderer::fill`] with `now_ns`, the host clock at the callback (for MIDI time stamps).
    pub(crate) fn fill_at<T: SizedSample + FromSample<f32>>(
        &mut self,
        data: &mut [T],
        output_latency_us: u32,
        now_ns: u64,
    ) {
        self.fill_timed(data, output_latency_us, now_ns, -1.0);
    }

    /// [`Renderer::fill_at`] with the device's sample time of the first frame (negative:
    /// unknown): a callback that starts later than the last one ended was missed, which is
    /// counted as an overload, as is a callback that takes longer than its buffer.
    pub(crate) fn fill_timed<T: SizedSample + FromSample<f32>>(
        &mut self,
        data: &mut [T],
        output_latency_us: u32,
        now_ns: u64,
        sample_time: f64,
    ) {
        let frames = data.len() / self.channels.max(1);
        if sample_time >= 0.0 {
            // A jump back is a restart of the device (a new device or buffer), not a miss.
            if self.next_sample_time >= 0.0 && sample_time > self.next_sample_time + 0.5 {
                self.shared.late_callbacks.fetch_add(1, Ordering::Relaxed);
            }
            self.next_sample_time = sample_time + frames as f64;
        }
        // `Instant::now` reads the OS monotonic clock (no lock, no allocation).
        let started = Instant::now();
        let channels = self.channels;
        self.core
            .set_device_time((data.len() / channels) as u32, output_latency_us, now_ns);
        for chunk in data.chunks_mut(MAX_BLOCK_FRAMES * channels) {
            let frames = chunk.len() / channels;
            let buf = &mut self.scratch[..frames * 2];
            let core = &mut self.core;
            crate::rt_alloc::guarded(|| core.process(buf));
            for (i, frame) in chunk.chunks_exact_mut(channels).enumerate() {
                let (l, r) = (buf[i * 2], buf[i * 2 + 1]);
                for (c, s) in frame.iter_mut().enumerate() {
                    *s = T::from_sample(match c {
                        0 => l,
                        1 => r,
                        _ => 0.0,
                    });
                }
            }
        }
        let load = dsp_load(started.elapsed(), data.len() / channels, self.sample_rate);
        self.shared.record_dsp_load(load);
        if load > 1.0 {
            self.shared.late_callbacks.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Engine {
    /// Starts on the default output device with the device's default buffer size.
    pub fn start() -> Result<Self, EngineError> {
        Self::start_with(None)
    }

    /// Like [`Engine::start`], but asks for `buffer_frames` per callback (clamped to what the
    /// device supports). Smaller buffers lower latency (Logic: I/O Buffer Size).
    pub fn start_with(buffer_frames: Option<u32>) -> Result<Self, EngineError> {
        Self::start_with_devices(buffer_frames, None, None)
    }

    /// Like [`Engine::start_with`] on the output and input with these Core Audio uids
    /// (`None`: the system's default). Only macOS opens devices by uid; elsewhere they are
    /// ignored and the defaults are used.
    pub fn start_with_devices(
        buffer_frames: Option<u32>,
        output: Option<String>,
        input: Option<String>,
    ) -> Result<Self, EngineError> {
        #[cfg(not(target_os = "macos"))]
        let _ = (&output, &input);
        // Windows: the shared-mode audio engine's smallest period (often 2-3 ms instead of
        // 10 ms) without taking the device away from other programs. cpal below otherwise.
        #[cfg(windows)]
        if std::env::var_os("PROWAVE_NO_LOW_LATENCY").is_none() {
            match Self::start_wasapi(buffer_frames) {
                Ok(engine) => return Ok(engine),
                Err(e) => tracing::info!("low-latency output not available ({e}); using cpal"),
            }
        }
        // macOS: input and output in one Core Audio callback, so a monitored input is heard
        // in the same buffer it arrives in. cpal below otherwise.
        #[cfg(target_os = "macos")]
        if std::env::var_os("PROWAVE_NO_LOW_LATENCY").is_none() {
            let devices = crate::duplex::Devices {
                output: output.clone(),
                input: input.clone(),
                with_input: true,
            };
            // A device that is gone: the system's defaults.
            let started = Self::start_duplex(buffer_frames, &devices).or_else(|e| {
                if devices.output.is_none() && devices.input.is_none() {
                    return Err(e);
                }
                tracing::warn!("{e}; using the default devices");
                Self::start_duplex(
                    buffer_frames,
                    &crate::duplex::Devices {
                        with_input: true,
                        ..Default::default()
                    },
                )
            });
            match started {
                Ok(engine) => return Ok(engine),
                Err(e) => tracing::info!("duplex audio not available ({e}); using cpal"),
            }
        }
        let mut last = EngineError::NoOutputDevice;
        // The platform's default audio system first, then any other one that is available
        // (e.g. ALSA when a sound daemon is broken).
        let default_id = cpal::default_host().id();
        let mut ids = vec![default_id];
        ids.extend(
            cpal::available_hosts()
                .into_iter()
                .filter(|id| *id != default_id),
        );
        for id in ids {
            let Ok(host) = cpal::host_from_id(id) else {
                continue;
            };
            match Self::start_on_host(&host, buffer_frames) {
                Ok(engine) => return Ok(engine),
                Err(e) => {
                    tracing::warn!("audio system {id} failed: {e}");
                    last = e;
                }
            }
        }
        Err(last)
    }

    /// Starts on the ASIO host (Windows builds with the `asio` feature only).
    /// Use this to avoid shared-mode WASAPI buffering when a native ASIO driver is available.
    #[cfg(all(windows, feature = "asio"))]
    pub fn start_asio_with(buffer_frames: Option<u32>) -> Result<Self, EngineError> {
        let host = cpal::host_from_id(cpal::HostId::Asio)
            .map_err(|error| EngineError::Backend(error.to_string()))?;
        Self::start_on_host(&host, buffer_frames)
    }

    #[cfg(windows)]
    fn start_wasapi(buffer_frames: Option<u32>) -> Result<Self, EngineError> {
        let (tx, rx) = RingBuffer::<Command>::new(1024);
        let (gtx, grx) = RingBuffer::<Garbage>::new(GARBAGE_CAPACITY);
        let shared = Arc::new(Shared::new());
        let shared_rt = shared.clone();
        let mut worker = start_worker(&shared);
        let ports = std::mem::take(&mut worker.ports);
        let make = move |sample_rate: u32, channels: usize| {
            let mut core = EngineCore::new(sample_rate, rx, gtx, shared_rt.clone());
            for port in ports {
                core.set_worker(port);
            }
            let readers = core.take_readers();
            (
                Renderer::new(core, shared_rt, channels, sample_rate),
                readers,
            )
        };
        let (stream, info, readers) = crate::wasapi::LowLatencyStream::start(buffer_frames, make)
            .map_err(EngineError::Backend)?;
        let state = readers
            .state
            .ok_or_else(|| EngineError::Backend("no state reader".into()))?;
        tracing::info!(
            "audio output {} (WASAPI low latency, {} frames at {} Hz)",
            info.device_name,
            info.period_frames,
            info.sample_rate
        );
        Ok(Self {
            commands: CommandQueue::new(tx, grx, shared.clone()),
            shared,
            state,
            midi_out: readers.midi,
            mtc_out: readers.mtc,
            audition: readers.audition,
            sample_rate: info.sample_rate,
            device_name: info.device_name,
            _output: Output::Wasapi(stream),
            _worker: worker.thread,
            worker_trash: worker.trash,
            disk: DiskThread::spawn(),
            buffer_frames: info.period_frames,
        })
    }

    #[cfg(target_os = "macos")]
    fn start_duplex(
        buffer_frames: Option<u32>,
        devices: &crate::duplex::Devices,
    ) -> Result<Self, EngineError> {
        let (tx, rx) = RingBuffer::<Command>::new(1024);
        let (gtx, grx) = RingBuffer::<Garbage>::new(GARBAGE_CAPACITY);
        let (tap_tx, mut tap_rx) = RingBuffer::<Option<Box<crate::capture::InputTap>>>::new(4);
        let (mut trash_tx, trash) = RingBuffer::<Box<crate::capture::InputTap>>::new(8);
        let shared = Arc::new(Shared::new());
        let shared_rt = shared.clone();
        let mut readers = None;
        let mut worker = start_worker(&shared);
        let mut ports = Some(std::mem::take(&mut worker.ports));
        let overloads = shared.hal_overloads.clone();
        let started =
            crate::duplex::DuplexStream::start_counted(buffer_frames, devices, overloads, |info| {
                let mut core = EngineCore::new(info.sample_rate, rx, gtx, shared_rt.clone());
                for port in ports.take().into_iter().flatten() {
                    core.set_worker(port);
                }
                readers = Some(core.take_readers());
                let mut renderer =
                    Renderer::new(core, shared_rt, info.out_channels, info.sample_rate);
                let mut tap: Option<Box<crate::capture::InputTap>> = None;
                move |b: crate::duplex::Block<'_>| {
                    while let Ok(next) = tap_rx.pop() {
                        if let Some(old) = std::mem::replace(&mut tap, next) {
                            // The UI frees it; the ring is drained before every new tap.
                            let _ = trash_tx.push(old);
                        }
                    }
                    if let Some(t) = tap.as_mut() {
                        if b.in_channels > 0 {
                            crate::rt_alloc::guarded(|| t.feed(b.input, b.input_latency_us));
                        }
                    }
                    // Another device can have another number of outputs.
                    renderer.channels = b.out_channels.max(1);
                    renderer.fill_timed(b.output, b.output_latency_us, b.now_ns, b.sample_time);
                }
            })
            .map_err(EngineError::Backend)?;
        let (stream, info) = started;
        // The playback thread joins this device's I/O workgroup.
        shared
            .workgroup_device
            .store(stream.device_id(), Ordering::Relaxed);
        let readers = readers.ok_or_else(|| EngineError::Backend("no readers".into()))?;
        let state = readers
            .state
            .ok_or_else(|| EngineError::Backend("no state reader".into()))?;
        tracing::info!(
            "audio {} (Core Audio duplex, {} frames at {} Hz, {} in / {} out, device latency in {} / out {} frames)",
            info.device_name,
            info.buffer_frames,
            info.sample_rate,
            info.in_channels,
            info.out_channels,
            info.input_latency_frames,
            info.output_latency_frames,
        );
        let tap = Some(TapPort {
            tx: tap_tx,
            trash,
            channels: info.in_channels.min(usize::from(u16::MAX)) as u16,
            name: info.input_name.clone().unwrap_or_default(),
        });
        Ok(Self {
            commands: CommandQueue::new(tx, grx, shared.clone()),
            shared,
            state,
            midi_out: readers.midi,
            mtc_out: readers.mtc,
            audition: readers.audition,
            sample_rate: info.sample_rate,
            device_name: info.device_name,
            _output: Output::Duplex(Box::new(stream)),
            _worker: worker.thread,
            worker_trash: worker.trash,
            disk: DiskThread::spawn(),
            tap,
            buffer_frames: info.buffer_frames,
        })
    }

    /// macOS: the default input as it arrives in the output's own callback, when the engine
    /// runs on the duplex stream. Monitoring it adds no delay of its own (Logic: software
    /// monitoring at the I/O buffer size). Each call replaces the capture before it.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn duplex_input(&mut self) -> Option<crate::capture::Capture> {
        #[cfg(target_os = "macos")]
        {
            let port = self.tap.as_mut().filter(|p| p.channels > 0)?;
            while port.trash.pop().is_ok() {}
            let (capture, tap) =
                crate::capture::Capture::tapped(port.channels, self.sample_rate, &port.name);
            port.tx.push(Some(Box::new(tap))).ok()?;
            Some(capture)
        }
        #[cfg(not(target_os = "macos"))]
        None
    }

    /// macOS: asks for `frames` per audio callback while playing (Logic: I/O Buffer Size).
    /// Returns what the device took; `None` when the engine is not on the duplex stream.
    pub fn set_buffer_frames(&mut self, frames: u32) -> Option<u32> {
        #[cfg(target_os = "macos")]
        if let Output::Duplex(stream) = &mut self._output {
            let got = stream.set_buffer_frames(frames);
            self.buffer_frames = got;
            return Some(got);
        }
        let _ = frames;
        None
    }

    /// macOS: moves to the output and input with these Core Audio uids (`None`: the
    /// system's default) without stopping: the song, the playhead and the mix go on. A
    /// capture from [`Engine::duplex_input`] must be opened again afterwards. On failure the
    /// devices before stay.
    pub fn set_devices(
        &mut self,
        output: Option<String>,
        input: Option<String>,
    ) -> Result<(), EngineError> {
        #[cfg(target_os = "macos")]
        if let Output::Duplex(stream) = &mut self._output {
            let devices = crate::duplex::Devices {
                output,
                input,
                with_input: true,
            };
            let result = stream.switch(&devices);
            self.shared
                .workgroup_device
                .store(stream.device_id(), Ordering::Relaxed);
            let info = stream.info().clone();
            self.device_name = info.device_name.clone();
            self.buffer_frames = info.buffer_frames;
            if let Some(port) = self.tap.as_mut() {
                // The tap before was for the other input.
                let _ = port.tx.push(None);
                port.channels = info.in_channels.min(usize::from(u16::MAX)) as u16;
                port.name = info.input_name.clone().unwrap_or_default();
            }
            tracing::info!(
                "audio {} ({} frames at {} Hz, {} in / {} out)",
                info.device_name,
                info.buffer_frames,
                info.sample_rate,
                info.in_channels,
                info.out_channels
            );
            return result.map(|_| ()).map_err(EngineError::Backend);
        }
        let _ = (output, input);
        Err(EngineError::Backend(
            "devices can only be changed on the macOS duplex stream".into(),
        ))
    }

    /// macOS: the uids of the open output and input.
    pub fn device_uids(&self) -> (Option<String>, Option<String>) {
        #[cfg(target_os = "macos")]
        if let Output::Duplex(stream) = &self._output {
            let info = stream.info();
            return (Some(info.output_uid.clone()), info.input_uid.clone());
        }
        (None, None)
    }

    /// Whether [`Engine::duplex_input`] can give an input.
    pub fn has_duplex_input(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.tap.as_ref().is_some_and(|p| p.channels > 0);
        #[cfg(not(target_os = "macos"))]
        false
    }

    /// Frames per audio callback, when known (0 when the system chose).
    pub fn buffer_frames(&self) -> u32 {
        self.buffer_frames
    }

    /// What the devices add beyond the I/O buffer (converters, driver, safety offset), as
    /// (input, output) frames; (0, 0) when the system does not say (only the macOS duplex
    /// stream knows). The round trip of a monitored input is these plus two I/O buffers.
    pub fn device_latency_frames(&self) -> (u32, u32) {
        #[cfg(target_os = "macos")]
        if let Output::Duplex(stream) = &self._output {
            let info = stream.info();
            return (info.input_latency_frames, info.output_latency_frames);
        }
        (0, 0)
    }

    fn start_on_host(host: &cpal::Host, buffer_frames: Option<u32>) -> Result<Self, EngineError> {
        let mut last = EngineError::NoOutputDevice;
        if let Some(device) = host.default_output_device() {
            match Self::start_on(&device, buffer_frames) {
                Ok(engine) => return Ok(engine),
                Err(e) => {
                    tracing::warn!("default audio output failed: {e}");
                    last = e;
                }
            }
        }
        // The default device can be missing or busy (common with raw ALSA on Linux): try
        // every other output device before giving up.
        if let Ok(devices) = host.output_devices() {
            for device in devices {
                let name = device.id().map(|i| i.to_string()).unwrap_or_default();
                match Self::start_on(&device, buffer_frames) {
                    Ok(engine) => {
                        tracing::info!("using audio output {name}");
                        return Ok(engine);
                    }
                    Err(e) => {
                        tracing::debug!("audio output {name} failed: {e}");
                        last = e;
                    }
                }
            }
        }
        Err(last)
    }

    /// Starts on `device`. If the requested buffer size is refused, retries with its default.
    fn start_on(device: &cpal::Device, buffer_frames: Option<u32>) -> Result<Self, EngineError> {
        match Self::start_device(device, buffer_frames) {
            Err(e) if buffer_frames.is_some() => {
                tracing::warn!("buffer size {buffer_frames:?} refused ({e}); using the default");
                Self::start_device(device, None)
            }
            other => other,
        }
    }

    fn start_device(
        device: &cpal::Device,
        buffer_frames: Option<u32>,
    ) -> Result<Self, EngineError> {
        let device_name = device
            .id()
            .map(|i| i.to_string())
            .unwrap_or_else(|_| "default".into());
        let supported = device
            .default_output_config()
            .map_err(|e| EngineError::Backend(e.to_string()))?;
        let format = supported.sample_format();
        let range = match supported.buffer_size() {
            SupportedBufferSize::Range { min, max } => Some((*min, *max)),
            SupportedBufferSize::Unknown => None,
        };
        let mut config: StreamConfig = supported.into();
        if let Some(n) = buffer_frames {
            let n = range.map_or(n, |(lo, hi)| n.clamp(lo, hi));
            config.buffer_size = BufferSize::Fixed(n);
        }
        let sample_rate = config.sample_rate;

        let (tx, rx) = RingBuffer::<Command>::new(1024);
        let (gtx, grx) = RingBuffer::<Garbage>::new(GARBAGE_CAPACITY);
        let shared = Arc::new(Shared::new());
        let mut core = EngineCore::new(sample_rate, rx, gtx, shared.clone());
        let mut worker = start_worker(&shared);
        for port in std::mem::take(&mut worker.ports) {
            core.set_worker(port);
        }
        let readers = core.take_readers();
        let state = readers
            .state
            .ok_or_else(|| EngineError::Backend("no state reader".into()))?;
        let renderer = Renderer::new(core, shared.clone(), config.channels as usize, sample_rate);

        let stream = match format {
            SampleFormat::F32 => build::<f32>(device, config, renderer),
            SampleFormat::I16 => build::<i16>(device, config, renderer),
            SampleFormat::U16 => build::<u16>(device, config, renderer),
            SampleFormat::I32 => build::<i32>(device, config, renderer),
            SampleFormat::U32 => build::<u32>(device, config, renderer),
            SampleFormat::I8 => build::<i8>(device, config, renderer),
            SampleFormat::U8 => build::<u8>(device, config, renderer),
            SampleFormat::F64 => build::<f64>(device, config, renderer),
            other => return Err(EngineError::UnsupportedSampleFormat(other.to_string())),
        }?;
        stream
            .play()
            .map_err(|e| EngineError::Backend(e.to_string()))?;
        Ok(Self {
            commands: CommandQueue::new(tx, grx, shared.clone()),
            shared,
            state,
            midi_out: readers.midi,
            mtc_out: readers.mtc,
            audition: readers.audition,
            sample_rate,
            device_name,
            _output: Output::Cpal(stream),
            _worker: worker.thread,
            worker_trash: worker.trash,
            disk: DiskThread::spawn(),
            #[cfg(target_os = "macos")]
            tap: None,
            buffer_frames: match config.buffer_size {
                BufferSize::Fixed(n) => n,
                BufferSize::Default => 0,
            },
        })
    }

    /// Queue a command (Logic: transport or project edit). Returns `false` if command or
    /// state-reclamation capacity is exhausted. The rejected command is freed here on the
    /// UI thread; callers must retry their desired change before marking it synchronized.
    /// Whether a state-replacing command (a song, notes, effects, plug-ins, a SoundFont, ...)
    /// sent now is accepted. Ask before making something costly to send, such as opening a
    /// plug-in: a refused command is freed with all it holds. What the engine sent back is
    /// freed here, as in [`Engine::send`].
    pub fn can_replace_state(&mut self) -> bool {
        for trash in self.worker_trash.iter_mut() {
            while trash.pop().is_ok() {}
        }
        self.commands.can_replace_state()
    }

    pub fn send(&mut self, cmd: Command) -> bool {
        // Plug-ins the playback thread let go of are freed here, off the real-time threads.
        for trash in self.worker_trash.iter_mut() {
            while trash.pop().is_ok() {}
        }
        let ok = self.commands.send(cmd);
        if !ok {
            tracing::error!("engine command or reclamation queue full; command dropped");
        }
        ok
    }

    /// The MIDI the engine sends to external instruments, once: the app plays it out on the
    /// tracks' ports. `None` after the first call.
    pub fn take_midi_out(&mut self) -> Option<rtrb::Consumer<MidiOutEvent>> {
        self.midi_out.take()
    }

    /// The MIDI Time Code the engine sends while [`Command::SetMtc`] has it on, once: the app
    /// plays it out on the project's MTC port. `None` after the first call.
    pub fn take_mtc_out(&mut self) -> Option<rtrb::Consumer<MtcMessage>> {
        self.mtc_out.take()
    }

    /// The window's end of the preview loop that plays on the output apart from the tracks
    /// (see [`crate::audition`]).
    pub fn audition(&mut self) -> Option<&mut crate::audition::AuditionRemote> {
        self.audition.as_mut()
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }
    /// The playhead, transport and meters after the newest audio block, all from that block.
    pub fn state(&mut self) -> &EngineState {
        self.state.read()
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Streams `file` from disk as an audio region on `track` starting at `start_beat`.
    /// Put the result in [`crate::Song::with_clips`] and send it with `Command::SetSong`.
    pub fn stream_clip(&self, file: &AudioFile, track: u16, start_beat: f64) -> AudioClip {
        self.stream_clip_with(file, track, start_beat, ClipEdit::default())
    }

    /// [`Engine::stream_clip_with`] for a song with `clips_in_song` streamed regions in all:
    /// every clip buffers ahead as much as the song's 8 MiB prefetch budget allows
    /// (between 170 ms and 0.68 s), so a few regions stay safe against a held-up disk and
    /// hundreds of regions stay light. Pass the same count for every clip of the song.
    pub fn stream_clip_in(
        &self,
        file: &AudioFile,
        track: u16,
        start_beat: f64,
        edit: ClipEdit,
        clips_in_song: usize,
    ) -> AudioClip {
        self.disk.stream_with_ring(
            file,
            self.sample_rate,
            track,
            start_beat,
            edit,
            crate::disk::ring_blocks_for(clips_in_song),
        )
    }

    /// Streams an audio region with non-destructive gain, beat-based fades, and reverse.
    /// It buffers the most ahead; for a song with many regions use [`Engine::stream_clip_in`].
    pub fn stream_clip_with(
        &self,
        file: &AudioFile,
        track: u16,
        start_beat: f64,
        edit: ClipEdit,
    ) -> AudioClip {
        self.disk
            .stream_with(file, self.sample_rate, track, start_beat, edit)
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut renderer: Renderer,
) -> Result<Stream, EngineError>
where
    T: SizedSample + FromSample<f32>,
{
    let shared = renderer.shared.clone();
    let err_fn = move |err: Error| match err.kind() {
        ErrorKind::Xrun => {
            shared.xruns.fetch_add(1, Ordering::Relaxed);
        }
        _ => tracing::error!("audio stream error: {err}"),
    };
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &OutputCallbackInfo| {
                let ts = info.timestamp();
                let lat = ts
                    .playback
                    .duration_since(ts.callback)
                    .as_micros()
                    .min(u32::MAX as u128) as u32;
                renderer.fill(data, lat);
            },
            err_fn,
            None,
        )
        .map_err(|e| EngineError::Backend(e.to_string()))
}
