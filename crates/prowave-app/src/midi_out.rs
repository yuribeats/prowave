//! MIDI out for External MIDI tracks: the notes the engine sends for those tracks go to the
//! MIDI port and channel each track is set to. A thread of its own does the sending, so a slow
//! or missing MIDI device never holds up the audio or the interface.
//!
//! On macOS every message carries the host time the audio frame it belongs to is heard at,
//! and CoreMIDI delivers it then: the timing follows the audio clock to the sample, not the
//! moment this thread happens to wake. Elsewhere messages go out as soon as they are seen.
//!
//! A track can also name a sound (bank and program), sent when it is routed; pitch bend,
//! channel pressure and System Exclusive from its regions go out on its port and channel.
//!
//! MIDI clock (24 a quarter note, Start / Stop / Continue / Song Position) goes to the ports
//! chosen in the time display's menu or the MIDI Monitor (Send MIDI Clock), kept for this
//! computer.
//!
//! The same thread sends MIDI Time Code, which the engine makes on the audio clock, to the port
//! the project names (`Project::mtc_out`).

use crate::model::{Project, TrackKind, CHANNEL_PRESSURE, PITCH_BEND, SYSEX_BYTE};
use midir::{MidiOutput, MidiOutputConnection};
use prowave_engine::{MidiOutEvent, MtcMessage};
use rtrb::Consumer;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the sending thread sleeps between looks at the queue.
const POLL: Duration = Duration::from_micros(250);
/// How often to try again to open a port that could not be opened.
const RETRY: Duration = Duration::from_secs(3);

/// Where one track's notes go: a port (by name), a channel 1-16, and the sound the track asks
/// for when it is routed: a bank and a program (`None` leaves the device as it is).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Route {
    pub port: Option<String>,
    pub channel: u8,
    pub program: Option<u8>,
    pub bank: Option<u16>,
    /// How much earlier than its audio frame each message goes out, in nanoseconds: the
    /// device's measured delay, so its sound coming back lines up with the song.
    pub advance_ns: u64,
}

/// The ports MIDI clock goes to (Send MIDI Clock), kept for this computer.
static CLOCK_PORTS: Mutex<Option<Vec<String>>> = Mutex::new(None);

fn clock_file() -> Option<std::path::PathBuf> {
    crate::recent_files::config_dir().map(|d| d.join("midi-clock-ports"))
}

/// The ports MIDI clock goes to.
pub fn clock_ports() -> Vec<String> {
    let Ok(mut g) = CLOCK_PORTS.lock() else {
        return Vec::new();
    };
    g.get_or_insert_with(|| {
        clock_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|t| {
                t.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    })
    .clone()
}

/// Turns MIDI clock to `port` on or off, and remembers it.
pub fn set_clock_port(port: &str, on: bool) {
    let mut ports = clock_ports();
    ports.retain(|p| p != port);
    if on {
        ports.push(port.to_string());
    }
    if let Ok(mut g) = CLOCK_PORTS.lock() {
        *g = Some(ports.clone());
    }
    if cfg!(test) {
        return;
    }
    if let Some(f) = clock_file() {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(f, ports.join("\n")));
        }
    }
}

/// The bytes of a system message from the engine (clock, start, song position...).
pub fn system_message(status: u8, spp: u16) -> ([u8; 3], usize) {
    match status {
        0xF2 => ([0xF2, (spp & 0x7f) as u8, ((spp >> 7) & 0x7f) as u8], 3),
        s => ([s, 0, 0], 1),
    }
}

/// The raw MIDI message for a note.
pub fn message(on: bool, channel: u8, pitch: u8, vel: u8) -> [u8; 3] {
    let status = if on { 0x90 } else { 0x80 };
    [
        status | (channel.clamp(1, 16) - 1),
        pitch.min(127),
        vel.min(127),
    ]
}

/// The raw MIDI message for a controller change (the sustain pedal is controller 64).
pub fn controller_message(channel: u8, number: u8, value: u8) -> [u8; 3] {
    [
        0xb0 | (channel.clamp(1, 16) - 1),
        number.min(127),
        value.min(127),
    ]
}

/// The pitch bend message of a channel: the value is 14 bits, the least significant 7 first.
pub fn pitch_bend_message(channel: u8, value: u16) -> [u8; 3] {
    let v = value.min(16383);
    [
        0xe0 | (channel.clamp(1, 16) - 1),
        (v & 0x7f) as u8,
        (v >> 7) as u8,
    ]
}

/// The channel pressure message of a channel.
pub fn channel_pressure_message(channel: u8, value: u16) -> [u8; 2] {
    [0xd0 | (channel.clamp(1, 16) - 1), value.min(127) as u8]
}

/// The longest System Exclusive message the sender will hold back (a longer one is dropped).
const MAX_SYSEX: usize = 4096;

/// The messages that select a sound on a channel: Bank Select (controllers 0 and 32, the most
/// and least significant 7 bits of the bank) when there is a bank, then Program Change.
pub fn program_messages(channel: u8, program: Option<u8>, bank: Option<u16>) -> Vec<Vec<u8>> {
    let ch = channel.clamp(1, 16) - 1;
    let mut out = Vec::new();
    if let Some(bank) = bank {
        let bank = bank.min(16383);
        out.push(vec![0xb0 | ch, 0, ((bank >> 7) & 0x7f) as u8]);
        out.push(vec![0xb0 | ch, 32, (bank & 0x7f) as u8]);
    }
    if let Some(program) = program {
        out.push(vec![0xc0 | ch, program.min(127)]);
    }
    out
}

/// MIDI panic for one channel: All Sound Off (CC 120), All Notes Off (CC 123) and Reset All
/// Controllers (CC 121).
pub fn panic_messages(channel: u8) -> [[u8; 3]; 3] {
    [
        controller_message(channel, 120, 0),
        controller_message(channel, 123, 0),
        controller_message(channel, 121, 0),
    ]
}

/// The names of the MIDI output ports on this computer, without the system's "Through" port.
pub fn ports() -> Vec<String> {
    let Ok(out) = MidiOutput::new(crate::APP_NAME) else {
        return Vec::new();
    };
    out.ports()
        .iter()
        .filter_map(|p| out.port_name(p).ok())
        .filter(|n| !n.to_lowercase().contains("through"))
        .collect()
}

/// The routes of a project, by track: only External MIDI tracks have a port.
pub fn routes_of(p: &Project) -> Vec<Route> {
    p.tracks
        .iter()
        .map(|t| {
            if t.kind == TrackKind::ExternalMidi {
                Route {
                    port: t.midi_port.clone(),
                    channel: t.midi_channel,
                    program: t.midi_program,
                    bank: t.midi_bank,
                    advance_ns: 0,
                }
            } else {
                Route::default()
            }
        })
        .collect()
}

/// Where bytes go when CoreMIDI does not take them: an open MIDI output port, or a recorder
/// in tests.
pub trait Sink: Send {
    /// Sends one message now; `false` when the device refused it.
    fn send(&mut self, bytes: &[u8]) -> bool;
}

struct Port(MidiOutputConnection);

impl Sink for Port {
    fn send(&mut self, bytes: &[u8]) -> bool {
        self.0.send(bytes).is_ok()
    }
}

/// Opens the sink of a port by name (`None` if it is not there).
pub type Opener = Box<dyn FnMut(&str) -> Option<Box<dyn Sink>> + Send>;

/// The queues the engine gives the MIDI sender: the tracks' notes and controllers with the
/// MIDI clock (as system messages, stamped on the audio clock), and MIDI Time Code.
pub struct Queues {
    pub notes: Consumer<MidiOutEvent>,
    pub mtc: Consumer<MtcMessage>,
}

/// The sending thread and the routes it follows.
pub struct MidiOut {
    /// Counts [`MidiOut::panic`] requests; the thread answers each new count once.
    panic: Arc<AtomicU32>,
    /// A test note asked for in the MIDI Monitor.
    test: Arc<Mutex<Option<(String, u8)>>>,
    routes: Arc<Mutex<Vec<Route>>>,
    mtc_port: Arc<Mutex<Option<String>>>,
    last: Vec<Route>,
    last_mtc: Option<String>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MidiOut {
    /// Starts sending what the engine delivers, to the ports the app names; on macOS through
    /// CoreMIDI at each message's host time.
    pub fn start(queues: Queues) -> Self {
        let open: Opener =
            Box::new(|name: &str| open_port(name).map(|c| Box::new(Port(c)) as Box<dyn Sink>));
        let routes = Arc::new(Mutex::new(Vec::new()));
        let mtc_port = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let panic = Arc::new(AtomicU32::new(0));
        let test = Arc::new(Mutex::new(None));
        let ports = Ports {
            routes: routes.clone(),
            mtc: mtc_port.clone(),
        };
        let (pa, s, tn) = (panic.clone(), stop.clone(), test.clone());
        let thread = std::thread::Builder::new()
            .name("prowave-midi-out".into())
            .spawn(move || {
                let mut sender = Sender::new(queues, ports, pa, s, open);
                sender.test = tn;
                #[cfg(target_os = "macos")]
                {
                    sender.core = CoreOut::new();
                }
                sender.run();
            })
            .ok();
        Self {
            panic,
            test,
            routes,
            mtc_port,
            last: Vec::new(),
            last_mtc: None,
            stop,
            thread,
        }
    }

    /// A test note (middle C, a quarter second) to `port` on `channel` (MIDI Monitor).
    pub fn test_note(&self, port: String, channel: u8) {
        if let Ok(mut t) = self.test.lock() {
            *t = Some((port, channel));
        }
    }

    /// MIDI panic: Note Off for every note still sounding, then All Sound Off, All Notes Off
    /// and Reset All Controllers on all 16 channels of every port a track uses.
    pub fn panic(&self) {
        self.panic.fetch_add(1, Ordering::AcqRel);
    }

    /// Call once per frame: passes the tracks' ports, channels and sounds, and the port of MIDI
    /// Time Code, on when they changed.
    pub fn sync(&mut self, p: &Project) {
        let now = routes_of(p);
        if now != self.last {
            if let Ok(mut routes) = self.routes.lock() {
                routes.clone_from(&now);
            }
            self.last = now;
        }
        if p.mtc_out != self.last_mtc {
            if let Ok(mut port) = self.mtc_port.lock() {
                port.clone_from(&p.mtc_out);
            }
            self.last_mtc.clone_from(&p.mtc_out);
        }
    }
}

impl Drop for MidiOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What the app tells the sender, shared with it: the routes and the MTC port.
struct Ports {
    routes: Arc<Mutex<Vec<Route>>>,
    mtc: Arc<Mutex<Option<String>>>,
}

struct Sender {
    events: Consumer<MidiOutEvent>,
    mtc_queue: Consumer<MtcMessage>,
    ports: Ports,
    panic: Arc<AtomicU32>,
    panic_seen: u32,
    stop: Arc<AtomicBool>,
    open: Opener,
    connections: HashMap<String, Box<dyn Sink>>,
    failed: HashMap<String, Instant>,
    /// Notes that were started and not yet released: (track, port, channel, pitch). A note is
    /// released on the port it started on, even when its track was routed elsewhere since.
    sounding: HashSet<(u16, String, u8, u8)>,
    /// The System Exclusive message each track is in the middle of, byte by byte.
    sysex: HashMap<u16, Vec<u8>>,
    /// The routes as they were the last time they were followed.
    known: Vec<Route>,
    /// macOS: CoreMIDI, which sends at a time stamp.
    #[cfg(target_os = "macos")]
    core: Option<CoreOut>,
    test: Arc<Mutex<Option<(String, u8)>>>,
}

/// CoreMIDI output that schedules each message for its host time.
#[cfg(target_os = "macos")]
struct CoreOut {
    _client: coremidi::Client,
    port: coremidi::OutputPort,
    dests: HashMap<String, coremidi::Destination>,
    looked: HashMap<String, Instant>,
}

#[cfg(target_os = "macos")]
impl CoreOut {
    fn new() -> Option<Self> {
        let client = coremidi::Client::new(crate::APP_NAME).ok()?;
        let port = client.output_port("Pro Wave Out").ok()?;
        Some(Self {
            _client: client,
            port,
            dests: HashMap::new(),
            looked: HashMap::new(),
        })
    }

    fn destination(&mut self, name: &str) -> Option<coremidi::Destination> {
        if let Some(d) = self.dests.get(name) {
            return Some(d.clone());
        }
        if self.looked.get(name).is_some_and(|t| t.elapsed() < RETRY) {
            return None;
        }
        self.looked.insert(name.to_string(), Instant::now());
        let found = coremidi::Destinations
            .into_iter()
            .find(|d| d.display_name().as_deref() == Some(name))?;
        self.dests.insert(name.to_string(), found.clone());
        Some(found)
    }

    /// Whether it was handed to CoreMIDI.
    fn send(&mut self, port: &str, bytes: &[u8], at_ns: u64) -> bool {
        let Some(dest) = self.destination(port) else {
            return false;
        };
        let stamp = if at_ns == 0 {
            0
        } else {
            prowave_engine::duplex::host_time_from_ns(at_ns)
        };
        let packets = coremidi::PacketBuffer::new(stamp, bytes);
        if self.port.send(&dest, &packets).is_ok() {
            true
        } else {
            // The device went away: look for it again later.
            self.dests.remove(port);
            false
        }
    }
}

impl Sender {
    fn new(
        queues: Queues,
        ports: Ports,
        panic: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
        open: Opener,
    ) -> Self {
        let panic_seen = panic.load(Ordering::Acquire);
        Self {
            events: queues.notes,
            mtc_queue: queues.mtc,
            ports,
            panic,
            panic_seen,
            stop,
            open,
            connections: HashMap::new(),
            failed: HashMap::new(),
            sounding: HashSet::new(),
            sysex: HashMap::new(),
            known: Vec::new(),
            #[cfg(target_os = "macos")]
            core: None,
            test: Arc::new(Mutex::new(None)),
        }
    }

    fn run(mut self) {
        while !self.stop.load(Ordering::Acquire) {
            self.follow_routes();
            self.follow_panic();
            self.follow_test_note();
            while let Ok(ev) = self.events.pop() {
                self.send(ev);
            }
            while let Ok(m) = self.mtc_queue.pop() {
                self.send_mtc(&m);
            }
            std::thread::sleep(POLL);
        }
        self.release_all();
    }

    /// A test note asked for in the MIDI Monitor: on 5 ms from now, off 250 ms later.
    fn follow_test_note(&mut self) {
        let test = self.test.lock().ok().and_then(|mut t| t.take());
        let Some((port, ch)) = test else {
            return;
        };
        #[cfg(target_os = "macos")]
        let (on_at, off_at) = {
            let now = prowave_engine::duplex::host_now_ns();
            (now + 5_000_000, now + 255_000_000)
        };
        #[cfg(not(target_os = "macos"))]
        let (on_at, off_at) = (0, 0);
        self.emit(&port, &message(true, ch, 60, 100), on_at);
        self.emit(&port, &message(false, ch, 60, 0), off_at);
    }

    /// The current routes (a copy, so the lock is not held while sending).
    fn current_routes(&self) -> Vec<Route> {
        self.ports
            .routes
            .lock()
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    /// Looks for routes that changed since the last look. A track that moved to another port or
    /// channel releases the notes it had sounding there; a track that now names a sound gets it.
    fn follow_routes(&mut self) {
        let now = self.current_routes();
        if now == self.known {
            return;
        }
        let old = std::mem::take(&mut self.known);
        for track in 0..old.len().max(now.len()) {
            let before = old.get(track).cloned().unwrap_or_default();
            let after = now.get(track).cloned().unwrap_or_default();
            if before == after {
                continue;
            }
            if let Some(port) = &before.port {
                if before.port != after.port || before.channel != after.channel {
                    self.release_track(track as u16, port, before.channel);
                }
            }
            if let Some(port) = after.port.clone() {
                let same_device = before.port.as_deref() == Some(port.as_str());
                if !same_device || before.program != after.program || before.bank != after.bank {
                    for msg in program_messages(after.channel, after.program, after.bank) {
                        self.emit(&port, &msg, 0);
                    }
                }
            }
        }
        self.known = now;
    }

    /// Releases the notes `track` has sounding on `port` and `channel`.
    fn release_track(&mut self, track: u16, port: &str, channel: u8) {
        let ours: Vec<(u16, String, u8, u8)> = self
            .sounding
            .iter()
            .filter(|(t, p, c, _)| *t == track && p == port && *c == channel)
            .cloned()
            .collect();
        for (_, p, c, pitch) in ours {
            self.sounding.remove(&(track, p.clone(), c, pitch));
            self.emit(&p, &message(false, c, pitch, 0), 0);
        }
    }

    /// MIDI panic: Note Off for what sounds, then All Sound Off, All Notes Off and Reset All
    /// Controllers on every channel of every output in use.
    fn follow_panic(&mut self) {
        let seen = self.panic.load(Ordering::Acquire);
        if seen == self.panic_seen {
            return;
        }
        self.panic_seen = seen;
        self.release_all();
        let mut ports: Vec<String> = self
            .current_routes()
            .into_iter()
            .filter_map(|r| r.port)
            .collect();
        ports.sort();
        ports.dedup();
        for port in &ports {
            for channel in 1..=16 {
                for msg in panic_messages(channel) {
                    self.emit(port, &msg, 0);
                }
            }
        }
    }

    fn route_of(&self, track: u16) -> Option<(String, u8, u64)> {
        let routes = self.ports.routes.lock().ok()?;
        let r = routes.get(track as usize)?;
        Some((r.port.clone()?, r.channel.clamp(1, 16), r.advance_ns))
    }

    fn send(&mut self, mut ev: MidiOutEvent) {
        if ev.system != 0 {
            let (bytes, n) = system_message(ev.system, ev.spp);
            for port in clock_ports() {
                self.emit(&port, &bytes[..n], ev.at_ns);
            }
            return;
        }
        let Some((port, channel, advance)) = self.route_of(ev.track) else {
            return;
        };
        if ev.at_ns != 0 {
            // Never 0, which means "now".
            ev.at_ns = ev.at_ns.saturating_sub(advance).max(1);
        }
        if ev.controller {
            match ev.pitch {
                PITCH_BEND => {
                    self.emit(&port, &pitch_bend_message(channel, ev.value), ev.at_ns);
                }
                CHANNEL_PRESSURE => {
                    self.emit(
                        &port,
                        &channel_pressure_message(channel, ev.value),
                        ev.at_ns,
                    );
                }
                SYSEX_BYTE => {
                    self.sysex_byte(ev.track, &port, ev.value.min(255) as u8, ev.at_ns);
                }
                number => {
                    self.emit(
                        &port,
                        &controller_message(channel, number, ev.vel),
                        ev.at_ns,
                    );
                }
            }
            return;
        }
        let key = (ev.track, port.clone(), channel, ev.pitch);
        if ev.on {
            self.sounding.insert(key);
        } else if !self.sounding.remove(&key) {
            // Its Note On never went out (no port, or another one): nothing to release.
            return;
        }
        self.emit(&port, &message(ev.on, channel, ev.pitch, ev.vel), ev.at_ns);
    }

    /// Sends a MIDI Time Code message to the project's MTC port, if it names one.
    fn send_mtc(&mut self, m: &MtcMessage) {
        let port = self.ports.mtc.lock().ok().and_then(|p| p.clone());
        if let Some(port) = port {
            self.emit(&port, m.bytes(), 0);
        }
    }

    /// One byte of a System Exclusive message. The message starts at 0xF0 and goes out whole at
    /// its 0xF7 (at that byte's time); bytes that come without a start are ignored, and a
    /// message that grows too long is dropped.
    fn sysex_byte(&mut self, track: u16, port: &str, byte: u8, at_ns: u64) {
        let done = {
            let buf = self.sysex.entry(track).or_default();
            if byte == 0xf0 {
                buf.clear();
                buf.push(byte);
            } else if !buf.is_empty() {
                if buf.len() >= MAX_SYSEX {
                    buf.clear();
                } else {
                    buf.push(byte);
                }
            }
            if byte == 0xf7 && buf.len() > 1 {
                Some(std::mem::take(buf))
            } else {
                None
            }
        };
        if let Some(message) = done {
            self.emit(port, &message, at_ns);
        }
    }

    /// Sends `bytes` to `port`, at host time `at_ns` where CoreMIDI can schedule it (macOS),
    /// else now (opening the port when needed). Returns whether it went out.
    fn emit(&mut self, port: &str, bytes: &[u8], at_ns: u64) -> bool {
        crate::midi_monitor::record(crate::midi_monitor::Dir::Out, port, bytes, at_ns);
        #[cfg(target_os = "macos")]
        if let Some(core) = self.core.as_mut() {
            if core.send(port, bytes, at_ns) {
                return true;
            }
        }
        let _ = at_ns;
        match self.connection(port) {
            Some(sink) => sink.send(bytes),
            None => false,
        }
    }

    /// The open connection to `port`, opening it when needed (and again every few seconds
    /// while it cannot be opened).
    fn connection(&mut self, port: &str) -> Option<&mut Box<dyn Sink>> {
        if !self.connections.contains_key(port) {
            if self.failed.get(port).is_some_and(|t| t.elapsed() < RETRY) {
                return None;
            }
            match (self.open)(port) {
                Some(sink) => {
                    self.failed.remove(port);
                    self.connections.insert(port.to_string(), sink);
                }
                None => {
                    self.failed.insert(port.to_string(), Instant::now());
                    return None;
                }
            }
        }
        self.connections.get_mut(port)
    }

    /// Releases every note still sounding (the app is closing, the thread stopped, or panic).
    fn release_all(&mut self) {
        let sounding: Vec<_> = self.sounding.drain().collect();
        for (_, port, channel, pitch) in sounding {
            self.emit(&port, &message(false, channel, pitch, 0), 0);
        }
    }
}

fn open_port(port: &str) -> Option<MidiOutputConnection> {
    let out = MidiOutput::new(crate::APP_NAME).ok()?;
    let found = out
        .ports()
        .into_iter()
        .find(|p| out.port_name(p).is_ok_and(|n| n == port))?;
    out.connect(&found, "prowave-out").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use midir::os::unix::VirtualInput;
    use std::sync::Mutex as StdMutex;

    /// Everything sent, as (port, bytes), in order.
    type Log = Arc<StdMutex<Vec<(String, Vec<u8>)>>>;

    struct Rec {
        port: String,
        log: Log,
    }

    impl Sink for Rec {
        fn send(&mut self, bytes: &[u8]) -> bool {
            self.log
                .lock()
                .unwrap()
                .push((self.port.clone(), bytes.to_vec()));
            true
        }
    }

    /// A sender with recording ports, not started as a thread: the tests drive it directly.
    /// The sender, the queue that feeds it, what it sent, its routes and its panic counter.
    type Rig = (
        Sender,
        rtrb::Producer<MidiOutEvent>,
        Log,
        Arc<Mutex<Vec<Route>>>,
        Arc<AtomicU32>,
    );

    fn rig(routes: Vec<Route>) -> Rig {
        let (tx, rx) = rtrb::RingBuffer::new(64);
        let (_mtc_tx, mtc_rx) = rtrb::RingBuffer::new(8);
        let log: Log = Arc::new(StdMutex::new(Vec::new()));
        let shared = Arc::new(Mutex::new(routes));
        let panic = Arc::new(AtomicU32::new(0));
        let l = log.clone();
        let open: Opener = Box::new(move |name: &str| {
            Some(Box::new(Rec {
                port: name.to_string(),
                log: l.clone(),
            }) as Box<dyn Sink>)
        });
        let sender = Sender::new(
            Queues {
                notes: rx,
                mtc: mtc_rx,
            },
            Ports {
                routes: shared.clone(),
                mtc: Arc::new(Mutex::new(None)),
            },
            panic.clone(),
            Arc::new(AtomicBool::new(false)),
            open,
        );
        (sender, tx, log, shared, panic)
    }

    fn ev(track: u16, on: bool, pitch: u8) -> MidiOutEvent {
        MidiOutEvent {
            track,
            on,
            pitch,
            vel: 90,
            ..MidiOutEvent::default()
        }
    }

    /// Runs what is queued, as the thread does between two looks at the routes.
    fn pump(sender: &mut Sender) {
        sender.follow_routes();
        sender.follow_panic();
        while let Ok(e) = sender.events.pop() {
            sender.send(e);
        }
    }

    fn sent(log: &Log) -> Vec<(String, Vec<u8>)> {
        log.lock().unwrap().clone()
    }

    fn route(port: &str, channel: u8) -> Route {
        Route {
            port: Some(port.into()),
            channel,
            ..Route::default()
        }
    }

    #[test]
    fn notes_are_raw_midi_on_their_channel() {
        assert_eq!(message(true, 1, 60, 100), [0x90, 60, 100]);
        assert_eq!(message(false, 1, 60, 0), [0x80, 60, 0]);
        assert_eq!(message(true, 16, 127, 127), [0x9f, 127, 127]);
        assert_eq!(
            message(true, 0, 200, 200),
            [0x90, 127, 127],
            "out of range is repaired"
        );
        assert_eq!(
            message(false, 10, 36, 64),
            [0x89, 36, 64],
            "channel 10 is drums"
        );
    }

    #[test]
    fn system_messages_are_one_byte_and_song_position_three() {
        assert_eq!(system_message(0xF8, 0), ([0xF8, 0, 0], 1));
        assert_eq!(system_message(0xFA, 0), ([0xFA, 0, 0], 1));
        assert_eq!(system_message(0xF2, 200), ([0xF2, 72, 1], 3));
    }

    #[test]
    fn pitch_bend_and_pressure_are_raw_midi_on_their_channel() {
        assert_eq!(pitch_bend_message(1, 8192), [0xe0, 0, 0x40]);
        assert_eq!(pitch_bend_message(3, 16383), [0xe2, 0x7f, 0x7f]);
        assert_eq!(
            pitch_bend_message(1, u16::MAX),
            [0xe0, 0x7f, 0x7f],
            "clamped"
        );
        assert_eq!(channel_pressure_message(16, 90), [0xdf, 90]);
        assert_eq!(channel_pressure_message(1, 500), [0xd0, 127]);
    }

    #[test]
    fn controllers_are_raw_midi_on_their_channel() {
        assert_eq!(controller_message(1, 64, 127), [0xb0, 64, 127]);
        assert_eq!(controller_message(16, 1, 0), [0xbf, 1, 0]);
        assert_eq!(controller_message(0, 200, 200), [0xb0, 127, 127]);
    }

    #[test]
    fn a_sound_is_bank_select_then_program_change() {
        assert_eq!(
            program_messages(3, Some(5), Some(0x1234)),
            vec![vec![0xb2, 0, 0x24], vec![0xb2, 32, 0x34], vec![0xc2, 5]],
            "bank 0x1234 = MSB 0x24, LSB 0x34"
        );
        assert_eq!(program_messages(1, Some(127), None), vec![vec![0xc0, 127]]);
        assert_eq!(
            program_messages(1, None, Some(1)),
            vec![vec![0xb0, 0, 0], vec![0xb0, 32, 1]]
        );
        assert!(
            program_messages(1, None, None).is_empty(),
            "no sound asked for"
        );
        assert_eq!(
            program_messages(1, Some(200), Some(65535)).last().unwrap(),
            &vec![0xc0, 127]
        );
    }

    #[test]
    fn only_external_tracks_have_a_route() {
        let mut p = Project::empty();
        p.tracks.clear();
        p.tracks
            .push(crate::model::Track::new("Inst", TrackKind::Instrument));
        let mut ext = crate::model::Track::new("Ext", TrackKind::ExternalMidi);
        ext.midi_port = Some("Synth".into());
        ext.midi_channel = 3;
        ext.midi_program = Some(9);
        ext.midi_bank = Some(2);
        p.tracks.push(ext);
        let routes = routes_of(&p);
        assert_eq!(routes[0], Route::default());
        assert_eq!(
            routes[1],
            Route {
                port: Some("Synth".into()),
                channel: 3,
                program: Some(9),
                bank: Some(2),
                advance_ns: 0,
            }
        );
    }

    #[test]
    fn notes_go_to_the_routed_port_and_channel_only() {
        let (mut sender, mut tx, log, _routes, _) = rig(vec![Route::default(), route("Synth", 3)]);
        for track in [0u16, 1, 9] {
            assert!(tx.push(ev(track, true, 60)).is_ok());
        }
        pump(&mut sender);
        assert_eq!(sent(&log), vec![("Synth".to_string(), vec![0x92, 60, 90])]);
    }

    #[test]
    fn a_missing_port_is_not_fatal_and_its_note_is_released_later() {
        let (mut sender, mut tx, log, _routes, _) = rig(vec![route("No Such Port 7f3a", 1)]);
        // The opener finds every port in the tests, so make this one missing.
        sender.open = Box::new(|_: &str| None);
        assert!(tx.push(ev(0, true, 60)).is_ok());
        pump(&mut sender);
        assert!(sent(&log).is_empty());
        assert!(sender.failed.contains_key("No Such Port 7f3a"));
        assert!(sender.connections.is_empty());
        assert!(sender
            .sounding
            .contains(&(0, "No Such Port 7f3a".to_string(), 1, 60)));
        sender.release_all();
        assert!(sender.sounding.is_empty());
    }

    #[test]
    fn moving_a_track_to_another_port_releases_its_sounding_notes_on_the_old_one() {
        let (mut sender, mut tx, log, routes, _) = rig(vec![route("A", 1)]);
        assert!(tx.push(ev(0, true, 60)).is_ok());
        pump(&mut sender);
        // The track is re-routed while the note sounds, to another port and channel.
        *routes.lock().unwrap() = vec![route("B", 2)];
        assert!(tx.push(ev(0, false, 60)).is_ok());
        pump(&mut sender);
        let out = sent(&log);
        assert!(out.contains(&("A".to_string(), vec![0x90, 60, 90])));
        assert!(
            out.contains(&("A".to_string(), vec![0x80, 60, 0])),
            "released on A: {out:?}"
        );
        assert!(
            !out.iter().any(|(p, _)| p == "B"),
            "nothing goes to B for a note that never started there: {out:?}"
        );
        assert!(sender.sounding.is_empty());
    }

    #[test]
    fn a_routed_sound_is_sent_when_the_route_is_set() {
        let (mut sender, _tx, log, routes, _) = rig(vec![Route::default()]);
        pump(&mut sender);
        assert!(sent(&log).is_empty(), "no port, no sound");
        *routes.lock().unwrap() = vec![Route {
            port: Some("Synth".into()),
            channel: 2,
            program: Some(40),
            bank: Some(1),
            advance_ns: 0,
        }];
        pump(&mut sender);
        assert_eq!(
            sent(&log),
            vec![
                ("Synth".to_string(), vec![0xb1, 0, 0]),
                ("Synth".to_string(), vec![0xb1, 32, 1]),
                ("Synth".to_string(), vec![0xc1, 40]),
            ]
        );
        // Nothing changed, so nothing is sent again.
        pump(&mut sender);
        assert_eq!(sent(&log).len(), 3);
    }

    #[test]
    fn panic_releases_every_channel_of_every_port_in_use() {
        let (mut sender, mut tx, log, _routes, panic) = rig(vec![route("Synth", 1)]);
        assert!(tx.push(ev(0, true, 64)).is_ok());
        pump(&mut sender);
        log.lock().unwrap().clear();
        panic.fetch_add(1, Ordering::AcqRel);
        pump(&mut sender);
        let out = sent(&log);
        assert_eq!(
            out[0],
            ("Synth".to_string(), vec![0x80, 64, 0]),
            "the sounding note is released first"
        );
        assert_eq!(
            out.len(),
            1 + 48,
            "all sound off, all notes off and reset controllers on 16 channels"
        );
        assert!(out.iter().all(|(p, _)| p == "Synth"));
        assert!(out.contains(&("Synth".to_string(), vec![0xb0, 123, 0])));
        assert!(out.contains(&("Synth".to_string(), vec![0xbf, 120, 0])));
        assert!(out.contains(&("Synth".to_string(), vec![0xb5, 121, 0])));
        assert!(sender.sounding.is_empty());
        // One request is answered once; nothing else is pending.
        log.lock().unwrap().clear();
        pump(&mut sender);
        assert!(sent(&log).is_empty());
    }

    #[test]
    fn stopping_releases_every_note_still_sounding() {
        let (mut sender, mut tx, log, _routes, _) = rig(vec![route("Synth", 1), route("Other", 2)]);
        assert!(tx.push(ev(0, true, 60)).is_ok());
        assert!(tx.push(ev(1, true, 64)).is_ok());
        pump(&mut sender);
        log.lock().unwrap().clear();
        sender.release_all();
        let mut out = sent(&log);
        out.sort();
        assert_eq!(
            out,
            vec![
                ("Other".to_string(), vec![0x81, 64, 0]),
                ("Synth".to_string(), vec![0x80, 60, 0]),
            ]
        );
    }

    /// Sends through a real ALSA port: a virtual input that midir creates here, which the sender
    /// then opens by name like any keyboard. Skipped (with a note) where the system has no MIDI
    /// sequencer.
    #[cfg(target_os = "linux")]
    #[test]
    fn bytes_reach_a_real_midi_port() {
        let Ok(input) = midir::MidiInput::new("prowave-test-sink") else {
            println!("no MIDI system: skipped");
            return;
        };
        let got: Arc<StdMutex<Vec<Vec<u8>>>> = Arc::new(StdMutex::new(Vec::new()));
        let g = got.clone();
        let name = "ProWave Test Sink";
        let Ok(_sink) = input.create_virtual(
            name,
            move |_, msg, _| g.lock().unwrap().push(msg.to_vec()),
            (),
        ) else {
            println!("no virtual MIDI port: skipped");
            return;
        };
        std::thread::sleep(Duration::from_millis(200));
        // The system names the port with its client ("client:port address"); a route stores the
        // name exactly as the port list gives it, so the test uses that name too.
        let Some(name) = ports().into_iter().find(|n| n.contains(name)) else {
            println!("the virtual port is not listed: skipped");
            return;
        };

        let (mut tx, rx) = rtrb::RingBuffer::new(16);
        let (_mtc_tx, mtc_rx) = rtrb::RingBuffer::new(8);
        let routes = Arc::new(Mutex::new(vec![Route {
            port: Some(name.clone()),
            channel: 4,
            ..Route::default()
        }]));
        let mut sender = Sender::new(
            Queues {
                notes: rx,
                mtc: mtc_rx,
            },
            Ports {
                routes,
                mtc: Arc::new(Mutex::new(None)),
            },
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicBool::new(false)),
            Box::new(|port: &str| open_port(port).map(|c| Box::new(Port(c)) as Box<dyn Sink>)),
        );
        assert!(tx.push(ev(0, true, 67)).is_ok());
        pump(&mut sender);
        // Pitch bend and a SysEx message on the same route.
        assert!(tx.push(ctl(0, PITCH_BEND, 8192)).is_ok());
        for b in [0xf0u8, 0x7e, 0x7f, 0xf7] {
            assert!(tx.push(ctl(0, SYSEX_BYTE, u16::from(b))).is_ok());
        }
        // The engine's Note Off carries velocity 0.
        assert!(tx
            .push(MidiOutEvent {
                track: 0,
                on: false,
                pitch: 67,
                vel: 0,
                ..MidiOutEvent::default()
            })
            .is_ok());
        pump(&mut sender);

        // In the order they were queued: note on, bend, SysEx, note off.
        // Channel 4 is status 0xE3 for a bend.
        let want = vec![
            message(true, 4, 67, 90).to_vec(),
            vec![0xe3, 0, 0x40],
            vec![0xf0, 0x7e, 0x7f, 0xf7],
            message(false, 4, 67, 0).to_vec(),
        ];
        for _ in 0..100 {
            if got.lock().unwrap().len() >= want.len() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            *got.lock().unwrap(),
            want,
            "the port received the note on and off"
        );
    }

    /// A controller-kind event of a track, as the engine makes it.
    fn ctl(track: u16, pitch: u8, value: u16) -> MidiOutEvent {
        MidiOutEvent {
            track,
            on: false,
            pitch,
            vel: value.min(127) as u8,
            controller: true,
            value,
            ..MidiOutEvent::default()
        }
    }

    #[test]
    fn pitch_bend_pressure_and_sysex_reach_the_routed_port() {
        let (mut sender, mut tx, log, _routes, _) = rig(vec![route("Synth", 2)]);
        assert!(tx.push(ctl(0, PITCH_BEND, 16383)).is_ok());
        assert!(tx.push(ctl(0, CHANNEL_PRESSURE, 90)).is_ok());
        // A System Exclusive message is one byte per event; a stray byte with no start is ignored.
        for b in [0xf0u8, 0x41, 0x10, 0xf7] {
            assert!(tx.push(ctl(0, SYSEX_BYTE, u16::from(b))).is_ok());
        }
        assert!(tx.push(ctl(0, SYSEX_BYTE, 0x55)).is_ok());
        pump(&mut sender);
        assert_eq!(
            sent(&log),
            vec![
                ("Synth".to_string(), vec![0xe1, 0x7f, 0x7f]),
                ("Synth".to_string(), vec![0xd1, 90]),
                ("Synth".to_string(), vec![0xf0, 0x41, 0x10, 0xf7]),
            ]
        );
    }

    #[test]
    fn midi_clock_goes_to_the_clock_ports_and_nowhere_else() {
        let (mut sender, mut tx, log, _routes, _) = rig(vec![route("Synth", 1)]);
        set_clock_port("Clock Box 5e1c", true);
        let sys = |status: u8, spp: u16| MidiOutEvent {
            system: status,
            spp,
            at_ns: 1_000,
            ..MidiOutEvent::default()
        };
        assert!(tx.push(sys(0xFA, 0)).is_ok());
        assert!(tx.push(sys(0xF8, 0)).is_ok());
        assert!(tx.push(sys(0xF2, 16)).is_ok());
        assert!(tx.push(ev(0, true, 60)).is_ok());
        pump(&mut sender);
        set_clock_port("Clock Box 5e1c", false);
        let out: Vec<_> = sent(&log)
            .into_iter()
            .filter(|(p, _)| p == "Clock Box 5e1c" || p == "Synth")
            .collect();
        assert_eq!(
            out,
            vec![
                ("Clock Box 5e1c".to_string(), vec![0xfa]),
                ("Clock Box 5e1c".to_string(), vec![0xf8]),
                ("Clock Box 5e1c".to_string(), vec![0xf2, 16, 0]),
                ("Synth".to_string(), vec![0x90, 60, 90]),
            ]
        );
    }
}
