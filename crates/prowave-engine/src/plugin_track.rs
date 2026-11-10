//! A track's plug-ins (Logic: the Instrument slot and the Audio FX slots): an instrument and
//! a chain of effects, each processing whole blocks.
//!
//! The engine mixes one frame at a time and a plug-in renders one block, so they meet through
//! a block of buffering: what the instrument renders, and what the effects return, is heard
//! one block later. The notes reach the instrument at the frame they were due, so timing
//! within the block is exact; the block of delay is made up by delay compensation.
//!
//! A track that is not live sends its plug-ins to the playback thread (see
//! [`crate::prerender`]): its frames, notes and automation go there through a lane and come
//! back the pre-render delay later.

use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicU32, Ordering};

/// Something that processes whole blocks: an Audio Unit on macOS, or a test double.
pub trait BlockProcessor: Send {
    /// A MIDI message `offset` frames into the next block (ignored by plain effects).
    fn midi(&mut self, status: u8, data1: u8, data2: u8, offset: u32);
    /// One block: `input` (ignored by instruments) into `out`. No allocation.
    fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32]));
    /// The delay it adds, in seconds.
    fn latency_seconds(&self) -> f64;
    /// Sets parameter `index` (native effects; others ignore it). Real-time safe.
    fn set_parameter(&mut self, _index: usize, _value: f32) {}
    /// Whether it takes parameter changes at a frame of the next block itself
    /// ([`BlockProcessor::set_parameter_at`]); otherwise the block is cut where they fall.
    fn takes_param_offsets(&self) -> bool {
        false
    }
    /// Sets parameter `index` to `value` from frame `offset` of the next block (only called
    /// when [`BlockProcessor::takes_param_offsets`]). Real-time safe.
    fn set_parameter_at(&mut self, index: u32, value: f32, _offset: u32) {
        self.set_parameter(index as usize, value);
    }
    /// Whether it follows the transport (then [`BlockProcessor::transport`] is called).
    fn wants_transport(&self) -> bool {
        false
    }
    /// Before each block: the beat of each of its frames, NaN while the transport is stopped.
    /// No allocation.
    fn transport(&mut self, _beats: &[f64]) {}
    /// Before each block: where the transport stood when the block began (VST3 plug-ins read
    /// it as their process context). No allocation.
    fn block_transport(&mut self, _t: &BlockTransport) {}
}

/// The transport at the start of a block, as a plug-in host passes it on (VST3:
/// `ProcessContext`; Logic: the host's tempo and position).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockTransport {
    pub playing: bool,
    /// The engine does not know when the app records; always false for now.
    pub recording: bool,
    /// Project time of the block's first frame, in samples.
    pub sample_pos: i64,
    /// Quarter notes from the project start (the playhead, also while stopped).
    pub beat: f64,
    /// Quarter note at which the bar containing `beat` began.
    pub bar_start_beat: f64,
    pub tempo: f64,
    pub sig: (u8, u8),
    /// Cycle locators in quarter notes, when cycling.
    pub cycle: Option<(f64, f64)>,
}

impl Default for BlockTransport {
    fn default() -> Self {
        Self {
            playing: false,
            recording: false,
            sample_pos: 0,
            beat: 0.0,
            bar_start_beat: 0.0,
            tempo: 120.0,
            sig: (4, 4),
            cycle: None,
        }
    }
}

#[cfg(target_os = "macos")]
impl BlockProcessor for crate::au::AuInstance {
    fn midi(&mut self, status: u8, data1: u8, data2: u8, offset: u32) {
        crate::au::AuInstance::midi(self, status, data1, data2, offset);
    }
    fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        crate::au::AuInstance::process(self, input, out);
    }
    fn latency_seconds(&self) -> f64 {
        crate::au::AuInstance::latency_seconds(self)
    }
    fn set_parameter(&mut self, index: usize, value: f32) {
        crate::au::AuInstance::set_parameter(self, index as u32, value, 0);
    }
    fn takes_param_offsets(&self) -> bool {
        true
    }
    fn set_parameter_at(&mut self, index: u32, value: f32, offset: u32) {
        crate::au::AuInstance::set_parameter(self, index, value, offset);
    }
    fn block_transport(&mut self, t: &BlockTransport) {
        crate::au::AuInstance::set_transport(self, t);
    }
}

/// MIDI messages one block can hold for an instrument; more are dropped.
const MAX_EVENTS: usize = 1024;
/// Automation changes one block can hold for the plug-ins; more are dropped.
const MAX_PARAM_EVENTS: usize = 2048;
/// Notes, controllers and automation a lane carries ahead of the playback thread.
const LANE_EVENTS: usize = 2048;
/// Frames a track fades over when its plug-ins move between the I/O callback and the
/// playback thread, or when a locate drops what was rendered ahead (2.7 ms at 48 kHz).
pub(crate) const FADE_FRAMES: usize = 128;
const FADE_STEP: f32 = 1.0 / FADE_FRAMES as f32;

/// An automation change for one plug-in: `slot` 0 is the instrument, `k + 1` effect `k`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ParamEvent {
    offset: u32,
    slot: u8,
    index: u32,
    value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Event {
    offset: u32,
    status: u8,
    data1: u8,
    data2: u8,
}

/// One frame into a lane: what reaches the effects, and the transport's beat (NaN: stopped).
#[derive(Clone, Copy, Default)]
pub(crate) struct InFrame {
    l: f32,
    r: f32,
    beat: f64,
}

/// One frame out of a lane: the instrument's sound and the effects' output.
#[derive(Clone, Copy, Default)]
pub(crate) struct OutFrame {
    inst: (f32, f32),
    fx: (f32, f32),
}

/// What travels with the frames, at its frame of the lane.
#[derive(Clone, Copy)]
pub(crate) enum LaneEvent {
    Midi(u64, Event),
    Param(u64, ParamEvent),
    Transport(u64, BlockTransport),
    /// `n` frames from this one that the I/O callback could not send (the lane was full
    /// because the playback thread fell behind): they come out silent, so the track keeps
    /// its time.
    Gap(u64, u64),
}

impl LaneEvent {
    fn frame(&self) -> u64 {
        match self {
            Self::Midi(f, _) | Self::Param(f, _) | Self::Transport(f, _) | Self::Gap(f, _) => *f,
        }
    }
}

/// A note, controller or automation change waiting for a live track's content delay.
#[derive(Clone, Copy)]
enum Delayed {
    Midi(Event),
    Param(ParamEvent),
}

/// The plug-ins of a track with their buffers: on the I/O thread while the track is live,
/// on the playback thread while it is pre-rendered. Made on the UI thread with all its
/// memory; neither thread allocates for it.
pub(crate) struct LaneWork {
    pub(crate) id: u32,
    pub(crate) track: u16,
    instrument: Option<Box<dyn BlockProcessor>>,
    effects: Vec<Box<dyn BlockProcessor>>,
    /// The instrument's last block, and what reaches the effects / what they returned.
    inst: [Vec<f32>; 2],
    fx_in: [Vec<f32>; 2],
    fx_out: [Vec<f32>; 2],
    tmp: [Vec<f32>; 2],
    /// Frames of the last finished block (what `inst` and `fx_out` hold).
    ready: usize,
    follows: bool,
    /// Time each plug-in took in the last block, in nanoseconds (0 the instrument).
    slot_ns: Vec<u64>,
    // The playback thread's ends of the lane, and its scratch for a process buffer.
    in_rx: Consumer<InFrame>,
    ev_rx: Consumer<LaneEvent>,
    out_tx: Producer<OutFrame>,
    events: Vec<Event>,
    params: Vec<ParamEvent>,
    beats: Vec<f64>,
    /// The lane frame the next process buffer starts at.
    frame: u64,
    /// The newest transport the lane carried, and its frame.
    last_bt: BlockTransport,
    last_bt_frame: u64,
    /// Frames of a gap still to come out as silence.
    gap_left: u64,
    /// Effects passed over for now, one bit per effect (Low Latency Mode on the master bus).
    fx_off: u64,
}

impl LaneWork {
    /// One block of `n` frames: the instrument plays `events` and the effects process
    /// `fx_in`, with the automation in `params` at its frames; `beats` is the transport's beat
    /// at each frame for an instrument that follows it. Reports each plug-in's time to
    /// `shared` (load over the block's real time at `sr`).
    #[allow(clippy::too_many_arguments)]
    fn run_block(
        &mut self,
        n: usize,
        events: &[Event],
        params: &[ParamEvent],
        beats: &[f64],
        bt: &BlockTransport,
        sr: f32,
        shared: Option<&crate::core::Shared>,
    ) {
        let n = n.min(self.inst[0].len());
        let last = n.saturating_sub(1) as u32;
        let follows = self.follows;
        if let Some(inst) = self.instrument.as_mut() {
            let started = Stopwatch::start();
            inst.block_transport(bt);
            let mine = |e: &&ParamEvent| e.slot == 0;
            if inst.takes_param_offsets() || !params.iter().any(|e| e.slot == 0) {
                for e in params.iter().filter(mine) {
                    inst.set_parameter_at(e.index, e.value, e.offset.min(last));
                }
                if follows {
                    let m = n.min(beats.len());
                    inst.transport(&beats[..m]);
                }
                for e in events.iter() {
                    inst.midi(e.status, e.data1, e.data2, e.offset.min(last));
                }
                let [l, r] = &mut self.inst;
                inst.process((&[], &[]), (&mut l[..n], &mut r[..n]));
            } else {
                // The instrument takes changes only between blocks: the block is cut at each
                // change, and each piece gets its own notes and beats.
                let mut a = 0usize;
                let mut k = 0usize;
                while a < n {
                    // Every change due by frame `a` is applied before the piece from `a`.
                    while k < params.len() && (params[k].offset as usize) <= a {
                        if params[k].slot == 0 {
                            inst.set_parameter(params[k].index as usize, params[k].value);
                        }
                        k += 1;
                    }
                    let b = params[k..]
                        .iter()
                        .filter(mine)
                        .map(|e| e.offset as usize)
                        .next()
                        .unwrap_or(n)
                        .min(n)
                        .max(a + 1);
                    if follows {
                        let m = b.min(beats.len());
                        inst.transport(&beats[a.min(m)..m]);
                    }
                    for e in events.iter() {
                        let o = (e.offset.min(last)) as usize;
                        if o >= a && o < b {
                            inst.midi(e.status, e.data1, e.data2, (o - a) as u32);
                        }
                    }
                    let [l, r] = &mut self.inst;
                    inst.process((&[], &[]), (&mut l[a..b], &mut r[a..b]));
                    a = b;
                }
            }
            if let Some(t) = self.slot_ns.first_mut() {
                *t = started.ns();
            }
        }
        if !self.effects.is_empty() {
            let [il, ir] = &self.fx_in;
            let [ol, or] = &mut self.fx_out;
            ol[..n].copy_from_slice(&il[..n]);
            or[..n].copy_from_slice(&ir[..n]);
            let off = self.fx_off;
            for (j, fx) in self.effects.iter_mut().enumerate() {
                if j < 64 && off & (1 << j) != 0 {
                    continue;
                }
                let started = Stopwatch::start();
                fx.block_transport(bt);
                let slot = (j + 1) as u8;
                let [tl, tr] = &mut self.tmp;
                if fx.takes_param_offsets() || !params.iter().any(|e| e.slot == slot) {
                    for e in params.iter().filter(|e| e.slot == slot) {
                        fx.set_parameter_at(e.index, e.value, e.offset.min(last));
                    }
                    fx.process((&ol[..n], &or[..n]), (&mut tl[..n], &mut tr[..n]));
                } else {
                    let mut a = 0usize;
                    let mut k = 0usize;
                    while a < n {
                        while k < params.len() && (params[k].offset as usize) <= a {
                            if params[k].slot == slot {
                                fx.set_parameter(params[k].index as usize, params[k].value);
                            }
                            k += 1;
                        }
                        let b = params[k..]
                            .iter()
                            .filter(|e| e.slot == slot)
                            .map(|e| e.offset as usize)
                            .next()
                            .unwrap_or(n)
                            .min(n)
                            .max(a + 1);
                        fx.process((&ol[a..b], &or[a..b]), (&mut tl[a..b], &mut tr[a..b]));
                        a = b;
                    }
                }
                ol[..n].copy_from_slice(&tl[..n]);
                or[..n].copy_from_slice(&tr[..n]);
                if let Some(t) = self.slot_ns.get_mut(j + 1) {
                    *t = started.ns();
                }
            }
        }
        self.ready = n;
        if let Some(shared) = shared {
            let block_ns = n as f64 * 1e9 / f64::from(sr.max(1.0));
            if block_ns > 0.0 {
                for (slot, &ns) in self.slot_ns.iter().enumerate() {
                    if ns == 0 || (slot == 0 && self.instrument.is_none()) {
                        continue;
                    }
                    let load = (ns as f64 / block_ns) as f32;
                    shared.record_plugin_load(self.track, slot as u8, load);
                }
            }
        }
    }

    /// Playback thread: renders the next `p` frames of the lane if they have all arrived and
    /// there is room for what comes out. No allocation, no waiting.
    pub(crate) fn process_chunk(
        &mut self,
        p: usize,
        sr: f32,
        shared: Option<&crate::core::Shared>,
    ) -> bool {
        let mut p = p.min(self.fx_in[0].len()).min(self.beats.len());
        // A gap here comes out as silence, a piece at a time (it can be longer than the
        // lane); a gap further on ends the block before it.
        if self.gap_left == 0 {
            if let Some((at, n)) = self.next_gap() {
                if at <= self.frame {
                    // Notes before and in the gap are lost with it (their frames never came).
                    while self.ev_rx.peek().is_ok_and(|e| e.frame() < at + n) {
                        let _ = self.ev_rx.pop();
                    }
                    self.frame = at;
                    self.gap_left = n;
                } else {
                    p = p.min((at - self.frame) as usize);
                }
            }
        }
        if self.gap_left > 0 {
            let n = (self.gap_left as usize).min(self.out_tx.slots());
            if n == 0 {
                return false;
            }
            if let Ok(mut chunk) = self.out_tx.write_chunk_uninit(n) {
                let (a, b) = chunk.as_mut_slices();
                for slot in a.iter_mut().chain(b.iter_mut()) {
                    slot.write(OutFrame::default());
                }
                // SAFETY: every slot of the chunk was written just above.
                unsafe { chunk.commit_all() };
            }
            self.frame += n as u64;
            self.gap_left -= n as u64;
            return true;
        }
        if p == 0 || self.in_rx.slots() < p || self.out_tx.slots() < p {
            return false;
        }
        if let Ok(chunk) = self.in_rx.read_chunk(p) {
            let (a, b) = chunk.as_slices();
            for (i, f) in a.iter().chain(b.iter()).enumerate() {
                self.fx_in[0][i] = f.l;
                self.fx_in[1][i] = f.r;
                self.beats[i] = f.beat;
            }
            chunk.commit_all();
        }
        let f0 = self.frame;
        let end = f0 + p as u64;
        let mut bt = advance(self.last_bt, f0.saturating_sub(self.last_bt_frame), sr);
        let mut events = std::mem::take(&mut self.events);
        let mut params = std::mem::take(&mut self.params);
        events.clear();
        params.clear();
        while let Ok(e) = self.ev_rx.peek() {
            if e.frame() >= end {
                break;
            }
            let Ok(e) = self.ev_rx.pop() else { break };
            let offset = |f: u64| f.saturating_sub(f0) as u32;
            match e {
                LaneEvent::Midi(f, ev) => {
                    if events.len() < events.capacity() {
                        events.push(Event {
                            offset: offset(f),
                            ..ev
                        });
                    }
                }
                LaneEvent::Param(f, pe) => {
                    if params.len() < params.capacity() {
                        params.push(ParamEvent {
                            offset: offset(f),
                            ..pe
                        });
                    }
                }
                LaneEvent::Transport(f, t) => {
                    if f <= f0 {
                        bt = advance(t, f0 - f, sr);
                    } else if t.playing && !bt.playing {
                        // The transport starts inside this process buffer: the plug-ins
                        // play from its first frame, from before the start.
                        bt = pre_roll(t, f - f0, sr);
                    }
                    self.last_bt = t;
                    self.last_bt_frame = f;
                }
                LaneEvent::Gap(..) => {}
            }
        }
        let beats = std::mem::take(&mut self.beats);
        self.run_block(p, &events, &params, &beats, &bt, sr, shared);
        self.beats = beats;
        self.events = events;
        self.params = params;
        if let Ok(mut chunk) = self.out_tx.write_chunk_uninit(p) {
            let (a, b) = chunk.as_mut_slices();
            for (i, slot) in a.iter_mut().chain(b.iter_mut()).enumerate() {
                slot.write(OutFrame {
                    inst: (self.inst[0][i], self.inst[1][i]),
                    fx: (self.fx_out[0][i], self.fx_out[1][i]),
                });
            }
            // SAFETY: every slot of the chunk was written just above.
            unsafe { chunk.commit_all() };
        }
        self.frame = end;
        true
    }

    /// The first gap the lane holds, as (frame, frames). Looks without taking.
    fn next_gap(&mut self) -> Option<(u64, u64)> {
        let n = self.ev_rx.slots();
        let chunk = self.ev_rx.read_chunk(n).ok()?;
        let (a, b) = chunk.as_slices();
        // The chunk is dropped without being committed: nothing is taken.
        a.iter().chain(b.iter()).find_map(|e| match e {
            LaneEvent::Gap(at, n) => Some((*at, *n)),
            _ => None,
        })
    }

    /// Empties the playback thread's ends of the lane (the I/O thread holds the whole
    /// lane then). The notes and automation it had not played yet are handed to `keep`.
    fn drain(&mut self, mut keep: impl FnMut(LaneEvent)) {
        let n = self.in_rx.slots();
        if let Ok(c) = self.in_rx.read_chunk(n) {
            c.commit_all();
        }
        while let Ok(e) = self.ev_rx.pop() {
            keep(e);
        }
    }
}

/// Sorts `v` by `key`, keeping the order of equal keys, without allocating (insertion sort:
/// the lists are short and almost always sorted already).
fn sort_by_offset<T: Copy>(v: &mut [T], key: impl Fn(&T) -> u32) {
    if v.windows(2).all(|w| key(&w[0]) <= key(&w[1])) {
        return;
    }
    for i in 1..v.len() {
        let x = v[i];
        let mut j = i;
        while j > 0 && key(&v[j - 1]) > key(&x) {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = x;
    }
}

/// Measures a plug-in's time (no clock in the browser build).
struct Stopwatch {
    #[cfg(not(target_arch = "wasm32"))]
    at: std::time::Instant,
}

impl Stopwatch {
    #[inline]
    fn start() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            at: std::time::Instant::now(),
        }
    }

    #[inline]
    fn ns(&self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        return self.at.elapsed().as_nanos() as u64;
        #[cfg(target_arch = "wasm32")]
        0
    }
}

/// `t`, `frames` later.
pub(crate) fn advance(mut t: BlockTransport, frames: u64, sr: f32) -> BlockTransport {
    if frames > 0 {
        t.sample_pos += frames as i64;
        if t.playing {
            t.beat += frames as f64 / f64::from(sr.max(1.0)) * t.tempo / 60.0;
        }
    }
    t
}

/// The transport `frames` before `t` starts playing, for a block whose frame `frames` is
/// the first one played: playing already, its position moved back (a short pre-roll, as
/// hosts give it), so that [`advance`] by `frames` lands exactly on `t`.
pub(crate) fn pre_roll(mut t: BlockTransport, frames: u64, sr: f32) -> BlockTransport {
    t.playing = true;
    if frames > 0 {
        t.sample_pos -= frames as i64;
        t.beat -= frames as f64 / f64::from(sr.max(1.0)) * t.tempo / 60.0;
        let bar = f64::from(t.sig.0.max(1)) * 4.0 / f64::from(t.sig.1.max(1));
        while t.beat < t.bar_start_beat {
            t.bar_start_beat -= bar;
        }
    }
    t
}

/// Where a track's plug-ins are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// In the I/O callback.
    Home,
    /// On the playback thread.
    Away,
    /// On their way back from the playback thread.
    Returning,
}

static NEXT_LANE: AtomicU32 = AtomicU32::new(1);

/// One track's plug-ins and the blocks between them and the engine. Made on the UI thread
/// with all its memory; the audio thread never allocates for it.
pub struct PluginTrack {
    /// The plug-ins: here while they run in the I/O callback, `None` while they are on the
    /// playback thread.
    work: Option<Box<LaneWork>>,
    id: u32,
    has_inst: bool,
    has_fx: bool,
    n_effects: usize,
    follows: bool,
    /// The I/O thread's ends of the lane.
    in_tx: Producer<InFrame>,
    ev_tx: Producer<LaneEvent>,
    out_rx: Consumer<OutFrame>,
    events: Vec<Event>,
    params: Vec<ParamEvent>,
    /// Notes and automation from the regions of a live track, waiting for its content delay,
    /// by the frame they are due.
    delayed: std::collections::VecDeque<(u64, Delayed)>,
    content_delay: u32,
    /// The transport's beat at each frame of this block (NaN: stopped).
    beats: Vec<f64>,
    /// Pre-rendered: what reaches the effects during this block, and what came back for it.
    hal_in: [Vec<f32>; 2],
    out_inst: [Vec<f32>; 2],
    out_fx: [Vec<f32>; 2],
    /// Which frames of this block came back from the playback thread.
    real: Vec<bool>,
    /// The transport when this block began.
    block_transport: BlockTransport,
    place: Place,
    /// Whether the plug-ins should be on the playback thread, and the delay they have there.
    want_away: bool,
    delay: u64,
    /// The track's fade (see [`FADE_FRAMES`]): its gain now and where it is going.
    gain: f32,
    target: f32,
    /// Frames sent to the lane, and the next output to take from it.
    pushed: u64,
    popped: u64,
    /// Outputs for lane frames before this are dropped (they were rendered before a locate).
    skip_until: u64,
    /// A locate is fading the track out; then outputs for frames before this are dropped.
    flush_at: Option<u64>,
    /// The rest of this block was rendered before a locate.
    stale: bool,
    /// Frames this track has seen in the I/O callback, and the length of this block.
    now: u64,
    block_n: usize,
    /// Frames the lane did not have in time, since the engine last asked.
    pub(crate) underruns: u32,
    /// The playback thread the plug-ins are on (an index into the engine's).
    worker: usize,
    /// Frames not sent since `gap_at` because the lane was full (see [`LaneEvent::Gap`]).
    gap: u64,
    gap_at: u64,
    /// The delay of each effect in seconds, asked once when the track is made (UI thread).
    fx_latency: Vec<f64>,
    /// Effects passed over for now, one bit per effect (Low Latency Mode on the master bus).
    fx_off: u64,
}

impl PluginTrack {
    /// `max_frames`: the longest block the engine processes at once.
    pub fn new(
        instrument: Option<Box<dyn BlockProcessor>>,
        effects: Vec<Box<dyn BlockProcessor>>,
        max_frames: usize,
    ) -> Self {
        let frames = max_frames.max(crate::prerender::MAX_PROCESS_BUFFER);
        let buf = || [vec![0f32; frames], vec![0f32; frames]];
        let follows = instrument.as_ref().is_some_and(|i| i.wants_transport());
        let (in_tx, in_rx) = RingBuffer::new(crate::prerender::LANE_FRAMES);
        let (ev_tx, ev_rx) = RingBuffer::new(LANE_EVENTS);
        let (out_tx, out_rx) = RingBuffer::new(crate::prerender::LANE_FRAMES);
        let id = NEXT_LANE.fetch_add(1, Ordering::Relaxed);
        let slots = 1 + effects.len();
        let (has_inst, has_fx) = (instrument.is_some(), !effects.is_empty());
        let n_effects = effects.len();
        let fx_latency = effects.iter().map(|e| e.latency_seconds()).collect();
        let work = LaneWork {
            id,
            track: 0,
            instrument,
            effects,
            inst: buf(),
            fx_in: buf(),
            fx_out: buf(),
            tmp: buf(),
            ready: 0,
            follows,
            slot_ns: vec![0; slots],
            in_rx,
            ev_rx,
            out_tx,
            events: Vec::with_capacity(LANE_EVENTS),
            params: Vec::with_capacity(LANE_EVENTS),
            beats: vec![f64::NAN; frames],
            frame: 0,
            last_bt: BlockTransport::default(),
            last_bt_frame: 0,
            gap_left: 0,
            fx_off: 0,
        };
        Self {
            work: Some(Box::new(work)),
            id,
            has_inst,
            has_fx,
            n_effects,
            follows,
            in_tx,
            ev_tx,
            out_rx,
            events: Vec::with_capacity(MAX_EVENTS),
            params: Vec::with_capacity(MAX_PARAM_EVENTS),
            delayed: std::collections::VecDeque::with_capacity(MAX_EVENTS),
            content_delay: 0,
            beats: vec![f64::NAN; frames],
            hal_in: buf(),
            out_inst: buf(),
            out_fx: buf(),
            real: vec![false; frames],
            block_transport: BlockTransport::default(),
            place: Place::Home,
            want_away: false,
            delay: 0,
            gain: 1.0,
            target: 1.0,
            pushed: 0,
            popped: 0,
            skip_until: 0,
            flush_at: None,
            stale: false,
            now: 0,
            block_n: 0,
            underruns: 0,
            worker: 0,
            gap: 0,
            gap_at: 0,
            fx_latency,
            fx_off: 0,
        }
    }

    pub fn has_instrument(&self) -> bool {
        self.has_inst
    }

    pub fn has_effects(&self) -> bool {
        self.has_fx
    }

    /// Whether any effect is on (not passed over with [`PluginTrack::set_effects_off`]).
    pub fn has_active_effects(&self) -> bool {
        (0..self.n_effects).any(|k| !self.effect_off(k))
    }

    /// The longest block the buffers hold, in frames.
    pub fn capacity(&self) -> usize {
        self.real.len()
    }

    /// Passes over the effects whose bit is set in `mask` (bit `k`: effect `k`) until told
    /// otherwise: they are not run and add no delay. Real-time safe. Used on the master bus,
    /// whose plug-ins always run in the I/O callback; plug-ins on the playback thread keep
    /// the mask they left with until they come back.
    pub fn set_effects_off(&mut self, mask: u64) {
        self.fx_off = mask;
        if let Some(w) = self.work.as_mut() {
            w.fx_off = mask;
        }
    }

    #[inline]
    fn effect_off(&self, k: usize) -> bool {
        k < 64 && self.fx_off & (1 << k) != 0
    }

    /// The delay of each effect in seconds, as it said when the track was made.
    pub fn effect_latencies(&self) -> &[f64] {
        &self.fx_latency
    }

    /// The delay of the effects that are on, in seconds. Real-time safe (no plug-in is asked).
    pub fn active_effect_latency_seconds(&self) -> f64 {
        self.fx_latency
            .iter()
            .enumerate()
            .filter(|(k, _)| !self.effect_off(*k))
            .map(|(_, s)| *s)
            .sum()
    }

    /// The lane's id (it matches the plug-ins when they come back from the playback thread).
    pub(crate) fn lane_id(&self) -> u32 {
        self.id
    }

    /// The track the plug-ins belong to (for the heaviest plug-in report).
    pub(crate) fn set_track(&mut self, track: u16) {
        if let Some(w) = self.work.as_mut() {
            w.track = track;
        }
    }

    /// The playback thread the plug-ins are on, or were last on.
    pub(crate) fn worker(&self) -> usize {
        self.worker
    }

    /// Whether the plug-ins run on the playback thread now (or are on their way back).
    pub(crate) fn is_away(&self) -> bool {
        self.place != Place::Home
    }

    /// Parameter `index` of effect `effect` (in chain order). Audio thread. While the
    /// plug-ins are pre-rendered the change travels with the frames and is heard with them
    /// (up to the pre-render delay later, as in Logic).
    pub(crate) fn set_effect_param(&mut self, effect: usize, index: usize, value: f32) {
        match self.work.as_mut() {
            Some(w) => {
                if let Some(fx) = w.effects.get_mut(effect) {
                    fx.set_parameter(index, value);
                }
            }
            None => {
                if effect < 255 {
                    self.push_param(0, (effect + 1) as u8, index as u32, value);
                }
            }
        }
    }

    /// Automation: parameter `index` of plug-in `slot` (0 the instrument, `k + 1` effect `k`)
    /// goes to `value` from frame `offset` of this block. Audio thread, no allocation: past
    /// the queue's room a change is dropped (the next one, 32 frames later, carries on).
    pub(crate) fn push_param(&mut self, offset: u32, slot: u8, index: u32, value: f32) {
        let exists = if slot == 0 {
            self.has_inst
        } else {
            usize::from(slot) <= self.effect_count()
        };
        if exists && value.is_finite() && self.params.len() < self.params.capacity() {
            self.params.push(ParamEvent {
                offset,
                slot,
                index,
                value,
            });
        }
    }

    /// [`PluginTrack::push_param`] from the track's automation: on a live track it waits
    /// for the content delay, with the regions it belongs to.
    pub(crate) fn push_param_timeline(&mut self, offset: u32, slot: u8, index: u32, value: f32) {
        let late = offset.saturating_add(self.content_delay);
        if self.content_delay == 0 {
            self.push_param(offset, slot, index, value);
        } else if (late as usize) < self.block_n {
            // Due in this block still (a content delay shorter than the block).
            self.push_param(late, slot, index, value);
        } else if self.delayed.len() < self.delayed.capacity() {
            let due = self.now + u64::from(offset) + u64::from(self.content_delay);
            let e = ParamEvent {
                offset: 0,
                slot,
                index,
                value,
            };
            self.delayed.push_back((due, Delayed::Param(e)));
        }
    }

    fn effect_count(&self) -> usize {
        self.n_effects
    }

    /// The delay of the instrument and the effects, in seconds, without the blocks of
    /// buffering (which depend on the I/O buffer and the process buffer).
    pub fn latency_seconds(&self) -> f64 {
        self.work.as_ref().map_or(0.0, |w| {
            w.instrument
                .iter()
                .chain(w.effects.iter())
                .map(|p| p.latency_seconds())
                .sum()
        })
    }

    /// A MIDI message for the instrument at frame `offset` of this block.
    pub(crate) fn push_midi(&mut self, offset: u32, status: u8, data1: u8, data2: u8) {
        if self.has_inst && self.events.len() < self.events.capacity() {
            self.events.push(Event {
                offset,
                status,
                data1,
                data2,
            });
        }
    }

    /// [`PluginTrack::push_midi`] for a message from the track's regions: on a live track it
    /// waits for the content delay, so the regions stay in time while live notes do not wait.
    pub(crate) fn push_midi_timeline(&mut self, offset: u32, status: u8, data1: u8, data2: u8) {
        let late = offset.saturating_add(self.content_delay);
        if self.content_delay == 0 {
            self.push_midi(offset, status, data1, data2);
        } else if (late as usize) < self.block_n {
            // Due in this block still (a content delay shorter than the block).
            self.push_midi(late, status, data1, data2);
        } else if self.has_inst && self.delayed.len() < self.delayed.capacity() {
            let due = self.now + u64::from(offset) + u64::from(self.content_delay);
            let e = Event {
                offset: 0,
                status,
                data1,
                data2,
            };
            self.delayed.push_back((due, Delayed::Midi(e)));
        }
    }

    /// Frames the track's regions wait (a live track; 0 otherwise).
    pub(crate) fn set_content_delay(&mut self, frames: u32) {
        self.content_delay = frames;
    }

    /// Whether the instrument wants the transport's beat of every frame.
    #[inline]
    pub(crate) fn follows_transport(&self) -> bool {
        self.follows
    }

    /// The transport's beat at frame `i` of this block (NaN: stopped).
    #[inline]
    pub(crate) fn set_beat(&mut self, i: usize, beat: f64) {
        if let Some(b) = self.beats.get_mut(i) {
            *b = beat;
        }
    }

    /// Where the transport stood when this block began.
    #[inline]
    pub(crate) fn set_block_transport(&mut self, t: BlockTransport) {
        self.block_transport = t;
    }

    /// Before a block of `n` frames: `away` says whether the plug-ins should be on the
    /// playback thread, where they are heard `delay` frames after their input. Takes what the
    /// playback thread made for this block and releases the delayed notes that fall in it.
    pub(crate) fn begin_block(&mut self, n: usize, away: bool, delay: u32) {
        let n = n.min(self.real.len());
        self.block_n = n;
        self.stale = false;
        self.want_away = away && delay > 0;
        self.delay = u64::from(delay);
        self.target = match self.place {
            Place::Home if self.want_away => 0.0,
            Place::Away if !self.want_away => 0.0,
            Place::Returning => 0.0,
            _ => 1.0,
        };
        // Notes and automation of a live track's regions that are due in this block.
        let end = self.now + n as u64;
        while let Some(&(due, d)) = self.delayed.front() {
            if due >= end {
                break;
            }
            self.delayed.pop_front();
            let offset = due.saturating_sub(self.now) as u32;
            match d {
                Delayed::Midi(e) => {
                    if self.events.len() < self.events.capacity() {
                        self.events.push(Event { offset, ..e });
                    }
                }
                Delayed::Param(e) => {
                    if self.params.len() < self.params.capacity() {
                        self.params.push(ParamEvent { offset, ..e });
                    }
                }
            }
        }
        if self.place != Place::Away {
            return;
        }
        // The output of lane frame `q` is heard `delay` frames after it went in.
        let mut missed = false;
        for j in 0..n {
            self.real[j] = false;
            self.out_inst[0][j] = 0.0;
            self.out_inst[1][j] = 0.0;
            self.out_fx[0][j] = 0.0;
            self.out_fx[1][j] = 0.0;
            let want = self.pushed + j as u64;
            if want < self.delay {
                continue;
            }
            let q = want - self.delay;
            // Frames that came too late before are dropped now, to stay in time.
            while self.popped < q {
                if self.out_rx.pop().is_err() {
                    break;
                }
                self.popped += 1;
            }
            if self.popped != q {
                missed = true;
                continue;
            }
            match self.out_rx.pop() {
                Ok(f) => {
                    self.popped += 1;
                    if q >= self.skip_until {
                        self.real[j] = true;
                        self.out_inst[0][j] = f.inst.0;
                        self.out_inst[1][j] = f.inst.1;
                        self.out_fx[0][j] = f.fx.0;
                        self.out_fx[1][j] = f.fx.1;
                    }
                }
                Err(_) => missed = true,
            }
        }
        if missed {
            self.underruns += 1;
        }
        // The transport of this block's first frame travels with its frames (none while
        // frames are not sent: the lane's room is kept for the gap).
        if self.gap == 0 {
            let _ = self
                .ev_tx
                .push(LaneEvent::Transport(self.pushed, self.block_transport));
        }
    }

    /// The instrument's sound at frame `i` of this block; it also moves the track's fade.
    /// Call once per frame, before [`PluginTrack::through_effects`].
    #[inline]
    pub(crate) fn instrument_frame(&mut self, i: usize) -> (f32, f32) {
        let (real, out) = match self.place {
            Place::Home => match self.work.as_ref() {
                Some(w) if i < w.ready => (true, (w.inst[0][i], w.inst[1][i])),
                _ => (false, (0.0, 0.0)),
            },
            Place::Away => {
                let real = !self.stale && self.real.get(i).copied().unwrap_or(false);
                let out = if real {
                    (self.out_inst[0][i], self.out_inst[1][i])
                } else {
                    (0.0, 0.0)
                };
                (real, out)
            }
            Place::Returning => (false, (0.0, 0.0)),
        };
        let target = if self.flush_at.is_some() {
            0.0
        } else {
            self.target
        };
        if self.gain > target {
            self.gain = (self.gain - FADE_STEP).max(target);
        } else if self.gain < target && real {
            self.gain = (self.gain + FADE_STEP).min(target);
        }
        if self.gain == 0.0 {
            if let Some(at) = self.flush_at.take() {
                // Faded out: what was rendered before the locate is not heard.
                self.skip_until = at;
                self.stale = true;
            }
        }
        (out.0 * self.gain, out.1 * self.gain)
    }

    /// Frame `i` of the strip into the effects; what they made of it comes back (a block
    /// later in the I/O callback, the pre-render delay later on the playback thread).
    #[inline]
    pub(crate) fn through_effects(&mut self, i: usize, l: f32, r: f32) -> (f32, f32) {
        let g = self.gain;
        match self.place {
            Place::Home => {
                let Some(w) = self.work.as_mut() else {
                    return (0.0, 0.0);
                };
                if i >= w.fx_in[0].len() {
                    return (0.0, 0.0);
                }
                w.fx_in[0][i] = l;
                w.fx_in[1][i] = r;
                if i < w.ready {
                    (w.fx_out[0][i] * g, w.fx_out[1][i] * g)
                } else {
                    (0.0, 0.0)
                }
            }
            Place::Away => {
                if i >= self.hal_in[0].len() {
                    return (0.0, 0.0);
                }
                self.hal_in[0][i] = l;
                self.hal_in[1][i] = r;
                if !self.stale && self.real[i] {
                    (self.out_fx[0][i] * g, self.out_fx[1][i] * g)
                } else {
                    (0.0, 0.0)
                }
            }
            Place::Returning => (0.0, 0.0),
        }
    }

    /// Frame `i` of the block the effects just processed (after [`PluginTrack::end_block_with`]):
    /// the master bus feeds a whole block in, has it processed, and plays it at once. Only
    /// for plug-ins that run in the I/O callback (silence otherwise).
    #[inline]
    pub(crate) fn effect_output(&self, i: usize) -> (f32, f32) {
        match self.work.as_ref() {
            Some(w) if self.place == Place::Home && i < w.ready => (w.fx_out[0][i], w.fx_out[1][i]),
            _ => (0.0, 0.0),
        }
    }

    /// A locate: what was rendered ahead for the old position fades out and is not heard;
    /// the new position comes in after the pre-render delay. The notes waiting for a live
    /// track's content delay are dropped, except their Note Offs, which go out now.
    pub(crate) fn flush(&mut self) {
        let mut offs = 0;
        while let Some((_, d)) = self.delayed.pop_front() {
            if let Delayed::Midi(e) = d {
                if e.status & 0xf0 == 0x80
                    && offs < 256
                    && self.events.len() < self.events.capacity()
                {
                    self.events.push(Event { offset: 0, ..e });
                    offs += 1;
                }
            }
        }
        if self.place == Place::Away && self.gain > 0.0 {
            self.flush_at = Some(self.pushed);
        } else if self.place == Place::Away {
            self.skip_until = self.pushed;
        }
    }

    /// After a block of `n` frames. In the I/O callback the instrument plays its notes and
    /// the effects process what reached them, ready for the next block; on the playback
    /// thread the frames, notes and automation go into the lane. Moves the plug-ins to the
    /// playback thread, or asks for them back, once the track has faded out.
    pub(crate) fn end_block_with(
        &mut self,
        frames: usize,
        sr: f32,
        ports: &mut [crate::prerender::WorkerPort],
        shared: Option<&crate::core::Shared>,
    ) {
        let n = frames.min(self.real.len());
        // Notes in the order of their frames (delayed ones are put in among the others);
        // a stable sort, so a Note Off and a Note On on the same frame keep their order.
        sort_by_offset(&mut self.events, |e| e.offset);
        sort_by_offset(&mut self.params, |e| e.offset);
        match self.place {
            Place::Home => {
                if let Some(w) = self.work.as_mut() {
                    let bt = self.block_transport;
                    let m = n.min(self.beats.len());
                    w.run_block(
                        n,
                        &self.events,
                        &self.params,
                        &self.beats[..m],
                        &bt,
                        sr,
                        shared,
                    );
                }
                self.events.clear();
                self.params.clear();
                if self.want_away && self.gain == 0.0 {
                    self.go_away(ports);
                }
            }
            Place::Away => {
                let mut room = self.in_tx.slots() >= n;
                // A run of blocks that could not be sent goes out as one gap, before the
                // frames after it; it must not be lost, or the track would lose its time.
                if room && self.gap > 0 {
                    if self
                        .ev_tx
                        .push(LaneEvent::Gap(self.gap_at, self.gap))
                        .is_ok()
                    {
                        self.gap = 0;
                    } else {
                        room = false;
                    }
                }
                if !room {
                    // The playback thread is far behind: this block is not sent, and it comes
                    // out silent, so the track keeps its time (it stays on the playback
                    // thread: bringing it here would overload the live tracks).
                    if self.gap == 0 {
                        self.gap_at = self.pushed;
                    }
                    self.gap += n as u64;
                    self.pushed += n as u64;
                    self.underruns += 1;
                } else {
                    if let Ok(mut chunk) = self.in_tx.write_chunk_uninit(n) {
                        let (a, b) = chunk.as_mut_slices();
                        let follows = self.follows;
                        for (i, slot) in a.iter_mut().chain(b.iter_mut()).enumerate() {
                            slot.write(InFrame {
                                l: self.hal_in[0][i],
                                r: self.hal_in[1][i],
                                beat: if follows { self.beats[i] } else { f64::NAN },
                            });
                        }
                        // SAFETY: every slot of the chunk was written just above.
                        unsafe { chunk.commit_all() };
                    }
                    for e in self.events.iter() {
                        let _ = self
                            .ev_tx
                            .push(LaneEvent::Midi(self.pushed + u64::from(e.offset), *e));
                    }
                    for e in self.params.iter() {
                        let _ = self
                            .ev_tx
                            .push(LaneEvent::Param(self.pushed + u64::from(e.offset), *e));
                    }
                    self.pushed += n as u64;
                }
                self.events.clear();
                self.params.clear();
                self.hal_in[0][..n].fill(0.0);
                self.hal_in[1][..n].fill(0.0);
                // Faded out: ask for them back.
                if !self.want_away && self.gain == 0.0 {
                    if let Some(port) = ports.get_mut(self.worker) {
                        if port.detach(self.id) {
                            self.place = Place::Returning;
                            self.gain = 0.0;
                        }
                    }
                }
            }
            Place::Returning => {
                // The notes wait for the plug-ins (late, but none is lost).
                for e in self.events.iter_mut() {
                    e.offset = 0;
                }
                for e in self.params.iter_mut() {
                    e.offset = 0;
                }
            }
        }
        self.now += n as u64;
    }

    /// [`PluginTrack::end_block_with`] without a playback thread (the plug-ins stay here).
    #[cfg(test)]
    pub(crate) fn end_block(&mut self, frames: usize) {
        self.end_block_with(frames, 48_000.0, &mut [], None);
    }

    /// Sends the plug-ins to the playback thread with the fewest tracks. They stay here if
    /// its queue is full.
    fn go_away(&mut self, ports: &mut [crate::prerender::WorkerPort]) {
        let Some(k) = (0..ports.len()).min_by_key(|&k| ports[k].lanes()) else {
            return;
        };
        let port = &mut ports[k];
        let Some(mut work) = self.work.take() else {
            return;
        };
        work.drain(|_| {});
        while self.out_rx.pop().is_ok() {}
        work.frame = 0;
        work.gap_left = 0;
        work.last_bt = self.block_transport;
        work.last_bt_frame = 0;
        work.ready = 0;
        work.fx_off = self.fx_off;
        self.pushed = 0;
        self.popped = 0;
        self.skip_until = 0;
        self.flush_at = None;
        self.gap = 0;
        match port.attach(work) {
            Ok(()) => {
                self.place = Place::Away;
                self.gain = 0.0;
                self.worker = k;
            }
            Err(work) => self.work = Some(work),
        }
    }

    /// The plug-ins came back from the playback thread: they run here from the next block
    /// and fade in. What the lane still held is dropped, except its notes and automation,
    /// which are played at the start of the next block.
    pub(crate) fn come_home(&mut self, mut work: Box<LaneWork>) {
        let (events, params) = (&mut self.events, &mut self.params);
        work.drain(|e| match e {
            LaneEvent::Midi(_, ev) => {
                if events.len() < events.capacity() {
                    events.push(Event { offset: 0, ..ev });
                }
            }
            LaneEvent::Param(_, pe) => {
                if params.len() < params.capacity() {
                    params.push(ParamEvent { offset: 0, ..pe });
                }
            }
            LaneEvent::Transport(..) | LaneEvent::Gap(..) => {}
        });
        while self.out_rx.pop().is_ok() {}
        work.ready = 0;
        work.fx_off = self.fx_off;
        self.work = Some(work);
        self.place = Place::Home;
        self.gain = 0.0;
        self.flush_at = None;
    }

    /// The plug-ins, when they are here (to be returned with the track when it goes away).
    pub(crate) fn lane_home(&self) -> bool {
        self.work.is_some()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An instrument that plays a constant while a note is held, and an effect that halves.
    pub(crate) struct Fake {
        pub instrument: bool,
        level: f32,
        start: Option<usize>,
    }

    impl Fake {
        pub(crate) fn instrument() -> Self {
            Self {
                instrument: true,
                level: 0.0,
                start: None,
            }
        }
        pub(crate) fn halver() -> Self {
            Self {
                instrument: false,
                level: 0.0,
                start: None,
            }
        }
    }

    impl BlockProcessor for Fake {
        fn midi(&mut self, status: u8, _d1: u8, _d2: u8, offset: u32) {
            if status & 0xf0 == 0x90 {
                self.level = 0.5;
                self.start = Some(offset as usize);
            } else if status & 0xf0 == 0x80 {
                self.level = 0.0;
            }
        }
        fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
            for i in 0..out.0.len() {
                if self.instrument {
                    let on = self.start.is_some_and(|s| i >= s) || self.start.is_none();
                    let v = if on { self.level } else { 0.0 };
                    out.0[i] = v;
                    out.1[i] = v;
                } else {
                    out.0[i] = input.0[i] * 0.5;
                    out.1[i] = input.1[i] * 0.5;
                }
            }
            self.start = None;
        }
        fn latency_seconds(&self) -> f64 {
            0.0
        }
    }

    /// A stub plug-in for automation: it plays (instrument) or multiplies by (effect) its
    /// parameter 0, and notes what it was told. With `offsets` it takes changes at a frame
    /// itself, as an Audio Unit does.
    pub(crate) struct Gain {
        instrument: bool,
        offsets: bool,
        gain: f32,
        /// A change waiting for its frame (offset path).
        due: Option<(u32, f32)>,
        pub log: std::sync::Arc<std::sync::Mutex<Log>>,
    }

    /// What a [`Gain`] was told.
    #[derive(Default)]
    pub(crate) struct Log {
        /// (index, value, offset; `u32::MAX` without one) of every change, in order.
        pub told: Vec<(u32, f32, u32)>,
        /// The length of every piece `process` was asked for.
        pub pieces: Vec<usize>,
        pub notes: Vec<u32>,
    }

    impl Gain {
        pub(crate) fn new(instrument: bool, offsets: bool) -> Self {
            let log = std::sync::Arc::new(std::sync::Mutex::new(Log {
                told: Vec::with_capacity(64),
                pieces: Vec::with_capacity(64),
                notes: Vec::with_capacity(64),
            }));
            // The first lock may make the lock itself (macOS): not on the audio thread.
            drop(log.lock());
            Self {
                instrument,
                offsets,
                gain: 1.0,
                due: None,
                log,
            }
        }

        fn note(&self, f: impl FnOnce(&mut Log)) {
            if let Ok(mut l) = self.log.lock() {
                if l.told.len() < 64 && l.pieces.len() < 64 && l.notes.len() < 64 {
                    f(&mut l);
                }
            }
        }
    }

    impl BlockProcessor for Gain {
        fn midi(&mut self, _status: u8, _d1: u8, _d2: u8, offset: u32) {
            self.note(|l| l.notes.push(offset));
        }
        fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
            let n = out.0.len();
            self.note(|l| l.pieces.push(n));
            for i in 0..out.0.len() {
                if let Some((at, v)) = self.due {
                    if i as u32 >= at {
                        self.gain = v;
                        self.due = None;
                    }
                }
                let x = if self.instrument { 1.0 } else { input.0[i] };
                out.0[i] = x * self.gain;
                out.1[i] = x * self.gain;
            }
            if let Some((_, v)) = self.due.take() {
                self.gain = v;
            }
        }
        fn latency_seconds(&self) -> f64 {
            0.0
        }
        fn set_parameter(&mut self, index: usize, value: f32) {
            self.note(|l| l.told.push((index as u32, value, u32::MAX)));
            if index == 0 {
                self.gain = value;
            }
        }
        fn takes_param_offsets(&self) -> bool {
            self.offsets
        }
        fn set_parameter_at(&mut self, index: u32, value: f32, offset: u32) {
            self.note(|l| l.told.push((index, value, offset)));
            if index == 0 {
                self.due = Some((offset, value));
            }
        }
    }

    #[test]
    fn automation_reaches_an_effect_at_its_frame_with_or_without_offsets() {
        for offsets in [false, true] {
            let fx = Gain::new(false, offsets);
            let log = fx.log.clone();
            let mut p = PluginTrack::new(None, vec![Box::new(fx)], 64);
            for i in 0..64 {
                p.through_effects(i, 1.0, 1.0);
            }
            p.push_param(32, 1, 0, 0.25);
            // No such plug-in, and not a number: nothing.
            p.push_param(8, 2, 0, 0.5);
            p.push_param(8, 1, 0, f32::NAN);
            assert_no_alloc::assert_no_alloc(|| p.end_block(64));
            let heard: Vec<f32> = (0..64).map(|i| p.through_effects(i, 0.0, 0.0).0).collect();
            assert!(
                heard[..32].iter().all(|x| *x == 1.0),
                "{offsets}: {heard:?}"
            );
            assert!(
                heard[32..].iter().all(|x| *x == 0.25),
                "{offsets}: {heard:?}"
            );
            let g = log.lock().unwrap_or_else(|e| e.into_inner());
            if offsets {
                // An Audio Unit is told the frame and renders the block whole.
                assert_eq!(g.told, [(0, 0.25, 32)]);
                assert_eq!(g.pieces, [64]);
            } else {
                // Otherwise the block is cut where the change falls.
                assert_eq!(g.told, [(0, 0.25, u32::MAX)]);
                assert_eq!(g.pieces, [32, 32]);
            }
        }
    }

    #[test]
    fn an_instrument_cut_at_a_change_still_gets_its_notes_at_their_frames() {
        let inst = Gain::new(true, false);
        let log = inst.log.clone();
        let mut p = PluginTrack::new(Some(Box::new(inst)), Vec::new(), 64);
        p.push_midi(5, 0x90, 60, 100);
        p.push_midi(40, 0x80, 60, 0);
        p.push_param(16, 0, 0, 0.5);
        p.push_param(48, 0, 0, 0.0);
        assert_no_alloc::assert_no_alloc(|| p.end_block(64));
        assert_eq!(p.instrument_frame(15), (1.0, 1.0));
        assert_eq!(p.instrument_frame(16), (0.5, 0.5));
        assert_eq!(p.instrument_frame(47), (0.5, 0.5));
        assert_eq!(p.instrument_frame(48), (0.0, 0.0));
        {
            let g = log.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(g.pieces, [16, 32, 16]);
            // Note 5 lands in the first piece, note 40 at 24 of the second (16..48).
            assert_eq!(g.notes, [5, 24]);
        }
        // The next block has no changes: it is played whole.
        p.end_block(64);
        let g = log.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(g.pieces, [16, 32, 16, 64]);
    }

    #[test]
    fn notes_play_at_their_frame_one_block_later() {
        let mut p = PluginTrack::new(Some(Box::new(Fake::instrument())), Vec::new(), 64);
        p.push_midi(10, 0x90, 60, 100);
        p.end_block(64);
        assert_eq!(p.instrument_frame(9), (0.0, 0.0));
        assert_eq!(p.instrument_frame(10), (0.5, 0.5));
    }

    #[test]
    fn effects_return_a_block_later() {
        let mut p = PluginTrack::new(
            None,
            vec![Box::new(Fake::halver()), Box::new(Fake::halver())],
            8,
        );
        for i in 0..8 {
            assert_eq!(p.through_effects(i, 1.0, -1.0), (0.0, 0.0));
        }
        p.end_block(8);
        assert_eq!(p.through_effects(3, 0.0, 0.0), (0.25, -0.25));
    }
}
