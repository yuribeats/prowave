//! The host side of VST 3: the COM objects Pro Wave hands to a plug-in (see `vst3.rs`).
//!
//! - [`HostApp`]: `IHostApplication`, the host's name, and the messages and attribute lists
//!   a plug-in's component and controller send each other.
//! - [`ComponentHandler`]: `IComponentHandler`, the controller telling the host that the user
//!   moved a knob in the plug-in's window (passed on to the processor in the next block).
//! - [`MemoryStream`]: `IBStream` over bytes, for saving and restoring settings.
//! - [`EventList`] and [`ParamChanges`]: the notes and parameter changes of one block. Both are
//!   made with all their memory on the UI thread; filling and reading them never allocates.
//!
//! The bindings come from the `vst3` crate (coupler-rs/vst3-rs, MIT OR Apache-2.0), generated
//! from Steinberg's VST 3 SDK headers. Steinberg relicensed the SDK under the MIT License in
//! 2025 (github.com/steinbergmedia/vst3sdk, LICENSE.txt: "MIT License, Copyright (c) 2026,
//! Steinberg Media Technologies GmbH", checked 2026-11-06), which is compatible with this
//! GPL-3.0-or-later program.
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;
use vst3::Steinberg::Vst::{
    Event, IAttributeList, IAttributeListTrait, IComponentHandler, IComponentHandlerTrait,
    IEventList, IEventListTrait, IHostApplication, IHostApplicationTrait, IMessage, IMessageTrait,
    IParamValueQueue, IParamValueQueueTrait, IParameterChanges, IParameterChangesTrait, ParamID,
    ParamValue, String128, TChar,
};
use vst3::Steinberg::{
    int32, int64, kInvalidArgument, kNoInterface, kResultFalse, kResultOk, tresult, uint32,
    FIDString, IBStream, IBStreamTrait, TUID,
};
use vst3::{Class, ComWrapper, Interface};

fn guid_of(t: *const TUID) -> Option<[u8; 16]> {
    // SAFETY: the plug-in passes a pointer to 16 bytes (or null).
    unsafe { t.as_ref() }.map(|t| t.map(|c| c as u8))
}

/// Writes `s` into a VST 3 UTF-16 string of 128 units (ends with a zero).
pub fn write_string128(s: &str, out: &mut String128) {
    let mut n = 0;
    for u in s.encode_utf16().take(127) {
        out[n] = u as TChar;
        n += 1;
    }
    out[n] = 0;
}

/// A VST 3 UTF-16 string up to its zero.
pub fn read_string128(s: &[TChar]) -> String {
    let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

// ---------------------------------------------------------------- IHostApplication

/// `IHostApplication`: Pro Wave's name, and the messages a plug-in's halves exchange.
pub struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        let Some(name) = name.as_mut() else {
            return kInvalidArgument;
        };
        write_string128("Pro Wave", name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        let (Some(cid), Some(iid)) = (guid_of(cid), guid_of(iid)) else {
            return kInvalidArgument;
        };
        if obj.is_null() {
            return kInvalidArgument;
        }
        if cid == IMessage::IID && iid == IMessage::IID {
            if let Some(p) = ComWrapper::new(HostMessage::default()).to_com_ptr::<IMessage>() {
                *obj = p.into_raw().cast();
                return kResultOk;
            }
        }
        if cid == IAttributeList::IID && iid == IAttributeList::IID {
            if let Some(p) = ComWrapper::new(AttrList::default()).to_com_ptr::<IAttributeList>() {
                *obj = p.into_raw().cast();
                return kResultOk;
            }
        }
        *obj = std::ptr::null_mut();
        kNoInterface
    }
}

// ---------------------------------------------------------------- IMessage, IAttributeList

#[derive(Clone)]
enum Attr {
    Int(i64),
    Float(f64),
    /// UTF-16 with its zero.
    Str(Vec<TChar>),
    Bin(Vec<u8>),
}

/// `IAttributeList`: named values a message carries.
#[derive(Default)]
pub struct AttrList {
    values: Mutex<HashMap<Vec<u8>, Attr>>,
}

fn attr_key(id: *const c_char) -> Option<Vec<u8>> {
    // SAFETY: attribute ids are zero-terminated C strings (or null).
    (!id.is_null()).then(|| unsafe { CStr::from_ptr(id) }.to_bytes().to_vec())
}

impl AttrList {
    fn set(&self, id: *const c_char, v: Attr) -> tresult {
        let (Some(k), Ok(mut m)) = (attr_key(id), self.values.lock()) else {
            return kInvalidArgument;
        };
        m.insert(k, v);
        kResultOk
    }
    fn get(&self, id: *const c_char) -> Option<Attr> {
        let k = attr_key(id)?;
        self.values.lock().ok()?.get(&k).cloned()
    }
}

impl Class for AttrList {
    type Interfaces = (IAttributeList,);
}

impl IAttributeListTrait for AttrList {
    unsafe fn setInt(&self, id: *const c_char, value: int64) -> tresult {
        self.set(id, Attr::Int(value))
    }
    unsafe fn getInt(&self, id: *const c_char, value: *mut int64) -> tresult {
        match (self.get(id), value.as_mut()) {
            (Some(Attr::Int(v)), Some(out)) => {
                *out = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setFloat(&self, id: *const c_char, value: f64) -> tresult {
        self.set(id, Attr::Float(value))
    }
    unsafe fn getFloat(&self, id: *const c_char, value: *mut f64) -> tresult {
        match (self.get(id), value.as_mut()) {
            (Some(Attr::Float(v)), Some(out)) => {
                *out = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setString(&self, id: *const c_char, string: *const TChar) -> tresult {
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut v = Vec::new();
        let mut i = 0;
        loop {
            let c = *string.add(i);
            v.push(c);
            if c == 0 {
                break;
            }
            i += 1;
        }
        self.set(id, Attr::Str(v))
    }
    unsafe fn getString(
        &self,
        id: *const c_char,
        string: *mut TChar,
        size_in_bytes: uint32,
    ) -> tresult {
        let Some(Attr::Str(v)) = self.get(id) else {
            return kResultFalse;
        };
        let room = size_in_bytes as usize / std::mem::size_of::<TChar>();
        if string.is_null() || room == 0 {
            return kInvalidArgument;
        }
        let n = v.len().min(room);
        std::ptr::copy_nonoverlapping(v.as_ptr(), string, n);
        *string.add(n - 1) = 0;
        kResultOk
    }
    unsafe fn setBinary(
        &self,
        id: *const c_char,
        data: *const c_void,
        size_in_bytes: uint32,
    ) -> tresult {
        let bytes = if data.is_null() || size_in_bytes == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data.cast::<u8>(), size_in_bytes as usize).to_vec()
        };
        self.set(id, Attr::Bin(bytes))
    }
    unsafe fn getBinary(
        &self,
        id: *const c_char,
        data: *mut *const c_void,
        size_in_bytes: *mut uint32,
    ) -> tresult {
        // The bytes must outlive the call: they stay in the list (a fresh copy is never made).
        let (Some(k), Ok(m)) = (attr_key(id), self.values.lock()) else {
            return kInvalidArgument;
        };
        match (m.get(&k), data.as_mut(), size_in_bytes.as_mut()) {
            (Some(Attr::Bin(b)), Some(d), Some(n)) => {
                *d = b.as_ptr().cast();
                *n = b.len() as u32;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
}

/// `IMessage`: an id and its attributes.
pub struct HostMessage {
    id: Mutex<Vec<u8>>,
    attrs: ComWrapper<AttrList>,
}

impl Default for HostMessage {
    fn default() -> Self {
        Self {
            id: Mutex::new(vec![0]),
            attrs: ComWrapper::new(AttrList::default()),
        }
    }
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> FIDString {
        // The bytes live as long as the message unless the id is set again.
        self.id
            .lock()
            .map(|v| v.as_ptr().cast())
            .unwrap_or(std::ptr::null())
    }
    unsafe fn setMessageID(&self, id: FIDString) {
        if let (Some(k), Ok(mut v)) = (attr_key(id), self.id.lock()) {
            *v = k;
            v.push(0);
        }
    }
    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attrs
            .as_com_ref::<IAttributeList>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}

// ---------------------------------------------------------------- IComponentHandler

/// Parameter edits the plug-in's window makes, waiting for the processor's next block.
pub type EditQueue = rtrb::Consumer<(ParamID, ParamValue)>;

/// `IComponentHandler`: the controller reports edits; the processor hears them next block.
pub struct ComponentHandler {
    edits: Mutex<rtrb::Producer<(ParamID, ParamValue)>>,
    /// The same edits for the host's interface (automation: a hand on a knob overrides or
    /// records it), read on the UI thread with [`ComponentHandler::take_seen`].
    seen: Mutex<Vec<(ParamID, ParamValue)>>,
    /// `restartComponent` flags not yet acted on.
    pub restart: AtomicI32,
}

impl ComponentHandler {
    /// The handler and the processor's end of its queue.
    pub fn new(capacity: usize) -> (Self, EditQueue) {
        let (tx, rx) = rtrb::RingBuffer::new(capacity);
        (
            Self {
                edits: Mutex::new(tx),
                seen: Mutex::new(Vec::new()),
                restart: AtomicI32::new(0),
            },
            rx,
        )
    }

    /// The edits the plug-in's window made since the last call, in order.
    pub fn take_seen(&self) -> Vec<(ParamID, ParamValue)> {
        self.seen
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }
}

/// Most window edits kept between two reads of [`ComponentHandler::take_seen`].
const SEEN_ROOM: usize = 4096;

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        if let Ok(mut q) = self.edits.lock() {
            let _ = q.push((id, value));
        }
        if let Ok(mut s) = self.seen.lock() {
            if s.len() < SEEN_ROOM {
                s.push((id, value));
            }
        }
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.restart.fetch_or(flags, Ordering::Relaxed);
        kResultOk
    }
}

// ---------------------------------------------------------------- IBStream

/// `IBStream` over bytes in memory.
#[derive(Default)]
pub struct MemoryStream {
    inner: Mutex<(Vec<u8>, usize)>,
}

impl MemoryStream {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            inner: Mutex::new((bytes.to_vec(), 0)),
        }
    }
    pub fn bytes(&self) -> Vec<u8> {
        self.inner.lock().map(|g| g.0.clone()).unwrap_or_default()
    }
    pub fn rewind(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.1 = 0;
        }
    }
}

impl Class for MemoryStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, n: int32, read: *mut int32) -> tresult {
        let Ok(mut g) = self.inner.lock() else {
            return kResultFalse;
        };
        let (data, pos) = &mut *g;
        let n = (n.max(0) as usize).min(data.len().saturating_sub(*pos));
        if n > 0 {
            if buffer.is_null() {
                return kInvalidArgument;
            }
            std::ptr::copy_nonoverlapping(data.as_ptr().add(*pos), buffer.cast::<u8>(), n);
        }
        *pos += n;
        if let Some(r) = read.as_mut() {
            *r = n as i32;
        }
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, n: int32, written: *mut int32) -> tresult {
        let Ok(mut g) = self.inner.lock() else {
            return kResultFalse;
        };
        let n = n.max(0) as usize;
        if n > 0 && buffer.is_null() {
            return kInvalidArgument;
        }
        let (data, pos) = &mut *g;
        if data.len() < *pos + n {
            data.resize(*pos + n, 0);
        }
        if n > 0 {
            std::ptr::copy_nonoverlapping(buffer.cast::<u8>(), data.as_mut_ptr().add(*pos), n);
        }
        *pos += n;
        if let Some(w) = written.as_mut() {
            *w = n as i32;
        }
        kResultOk
    }
    unsafe fn seek(&self, offset: int64, mode: int32, result: *mut int64) -> tresult {
        use vst3::Steinberg::IBStream_::IStreamSeekMode_::{kIBSeekCur, kIBSeekEnd, kIBSeekSet};
        let Ok(mut g) = self.inner.lock() else {
            return kResultFalse;
        };
        let mode = u32::try_from(mode).ok();
        let base = if mode == Some(kIBSeekSet) {
            0
        } else if mode == Some(kIBSeekCur) {
            g.1 as i64
        } else if mode == Some(kIBSeekEnd) {
            g.0.len() as i64
        } else {
            return kInvalidArgument;
        };
        let to = base + offset;
        if to < 0 {
            return kResultFalse;
        }
        g.1 = to as usize;
        if let Some(r) = result.as_mut() {
            *r = to;
        }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        let Ok(g) = self.inner.lock() else {
            return kResultFalse;
        };
        match pos.as_mut() {
            Some(p) => {
                *p = g.1 as i64;
                kResultOk
            }
            None => kInvalidArgument,
        }
    }
}

// ---------------------------------------------------------------- IEventList

/// `IEventList` with room for a fixed number of events, made up front. Only the thread that
/// processes the plug-in touches it (the host fills it, the plug-in reads it during
/// `process`), so plain cells are enough.
pub struct EventList {
    events: UnsafeCell<Box<[Event]>>,
    len: UnsafeCell<usize>,
}

// SAFETY: used by one thread at a time (the processing thread); the instance that owns it is
// handed between threads whole, through the engine's queue.
unsafe impl Send for EventList {}
// SAFETY: as above: never touched from two threads at once.
unsafe impl Sync for EventList {}

impl EventList {
    pub fn with_capacity(n: usize) -> Self {
        // SAFETY: an all-zero Event is a valid plain-data value.
        let blank: Event = unsafe { std::mem::zeroed() };
        Self {
            events: UnsafeCell::new(vec![blank; n.max(1)].into_boxed_slice()),
            len: UnsafeCell::new(0),
        }
    }
    /// Adds an event; false when full. No allocation.
    pub fn push(&self, e: Event) -> bool {
        // SAFETY: one thread at a time (see the type).
        unsafe {
            let events = &mut *self.events.get();
            let len = &mut *self.len.get();
            if *len >= events.len() {
                return false;
            }
            events[*len] = e;
            *len += 1;
            true
        }
    }
    pub fn len(&self) -> usize {
        // SAFETY: one thread at a time.
        unsafe { *self.len.get() }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn clear(&self) {
        // SAFETY: one thread at a time.
        unsafe { *self.len.get() = 0 }
    }
    pub fn get(&self, i: usize) -> Option<Event> {
        // SAFETY: one thread at a time.
        unsafe { (&*self.events.get())[..*self.len.get()].get(i).copied() }
    }
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.len() as i32
    }
    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        match (
            usize::try_from(index).ok().and_then(|i| self.get(i)),
            e.as_mut(),
        ) {
            (Some(ev), Some(out)) => {
                *out = ev;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }
    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        match e.as_ref() {
            Some(ev) if self.push(*ev) => kResultOk,
            _ => kResultFalse,
        }
    }
}

// ---------------------------------------------------------------- IParameterChanges

/// `IParamValueQueue`: one parameter's changes in a block, with a fixed number of points.
pub struct ParamQueue {
    id: UnsafeCell<ParamID>,
    points: UnsafeCell<Box<[(i32, f64)]>>,
    len: UnsafeCell<usize>,
}

// SAFETY: one thread at a time, like [`EventList`].
unsafe impl Send for ParamQueue {}
// SAFETY: as above.
unsafe impl Sync for ParamQueue {}

impl ParamQueue {
    fn new(points: usize) -> Self {
        Self {
            id: UnsafeCell::new(0),
            points: UnsafeCell::new(vec![(0, 0.0); points.max(1)].into_boxed_slice()),
            len: UnsafeCell::new(0),
        }
    }
    fn id(&self) -> ParamID {
        // SAFETY: one thread at a time.
        unsafe { *self.id.get() }
    }
    fn reset(&self, id: ParamID) {
        // SAFETY: one thread at a time.
        unsafe {
            *self.id.get() = id;
            *self.len.get() = 0;
        }
    }
    fn len(&self) -> usize {
        // SAFETY: one thread at a time.
        unsafe { *self.len.get() }
    }
    /// Adds a point, keeping them in time order; a point at the same offset replaces the
    /// last one. When full, the last point takes the new value. Returns its index.
    fn add(&self, offset: i32, value: f64) -> usize {
        // SAFETY: one thread at a time.
        unsafe {
            let points = &mut *self.points.get();
            let len = &mut *self.len.get();
            if *len > 0 && points[*len - 1].0 >= offset {
                points[*len - 1] = (points[*len - 1].0, value);
                return *len - 1;
            }
            if *len == points.len() {
                points[*len - 1].1 = value;
                return *len - 1;
            }
            points[*len] = (offset, value);
            *len += 1;
            *len - 1
        }
    }
    fn point(&self, i: usize) -> Option<(i32, f64)> {
        // SAFETY: one thread at a time.
        unsafe { (&*self.points.get())[..*self.len.get()].get(i).copied() }
    }
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id()
    }
    unsafe fn getPointCount(&self) -> int32 {
        self.len() as i32
    }
    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        let Some((o, v)) = usize::try_from(index).ok().and_then(|i| self.point(i)) else {
            return kInvalidArgument;
        };
        if let Some(p) = offset.as_mut() {
            *p = o;
        }
        if let Some(p) = value.as_mut() {
            *p = v;
        }
        kResultOk
    }
    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        let i = self.add(offset, value);
        if let Some(p) = index.as_mut() {
            *p = i as i32;
        }
        kResultOk
    }
}

/// `IParameterChanges`: the parameters that change in a block, each with its queue. The
/// queues are made up front; a block with more parameters than queues drops the rest.
pub struct ParamChanges {
    queues: Box<[ComWrapper<ParamQueue>]>,
    used: UnsafeCell<usize>,
}

// SAFETY: one thread at a time, like [`EventList`].
unsafe impl Send for ParamChanges {}
// SAFETY: as above.
unsafe impl Sync for ParamChanges {}

impl ParamChanges {
    pub fn new(queues: usize, points: usize) -> Self {
        Self {
            queues: (0..queues.max(1))
                .map(|_| ComWrapper::new(ParamQueue::new(points)))
                .collect(),
            used: UnsafeCell::new(0),
        }
    }
    fn used(&self) -> usize {
        // SAFETY: one thread at a time.
        unsafe { *self.used.get() }
    }
    pub fn is_empty(&self) -> bool {
        self.used() == 0
    }
    pub fn clear(&self) {
        // SAFETY: one thread at a time.
        unsafe { *self.used.get() = 0 }
    }
    /// The queue of parameter `id`, taking a free one if it has none. No allocation.
    fn queue_of(&self, id: ParamID) -> Option<usize> {
        let used = self.used();
        if let Some(i) = self.queues[..used].iter().position(|q| q.id() == id) {
            return Some(i);
        }
        if used >= self.queues.len() {
            return None;
        }
        self.queues[used].reset(id);
        // SAFETY: one thread at a time.
        unsafe { *self.used.get() = used + 1 }
        Some(used)
    }
    /// Parameter `id` goes to `value` (normalized 0..1) at frame `offset`. No allocation.
    pub fn push(&self, id: ParamID, offset: i32, value: f64) -> bool {
        match self.queue_of(id) {
            Some(i) => {
                self.queues[i].add(offset, value.clamp(0.0, 1.0));
                true
            }
            None => false,
        }
    }
    /// The changes so far, for tests and checks: (id, offset, value).
    pub fn points(&self) -> Vec<(ParamID, i32, f64)> {
        let mut out = Vec::new();
        for q in &self.queues[..self.used()] {
            for i in 0..q.len() {
                if let Some((o, v)) = q.point(i) {
                    out.push((q.id(), o, v));
                }
            }
        }
        out
    }
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.used() as i32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        match usize::try_from(index) {
            Ok(i) if i < self.used() => self.queues[i]
                .as_com_ref::<IParamValueQueue>()
                .map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            _ => std::ptr::null_mut(),
        }
    }
    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        let Some(id) = id.as_ref() else {
            return std::ptr::null_mut();
        };
        let Some(i) = self.queue_of(*id) else {
            return std::ptr::null_mut();
        };
        if let Some(p) = index.as_mut() {
            *p = i as i32;
        }
        self.queues[i]
            .as_com_ref::<IParamValueQueue>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use vst3::ComPtr;

    fn note_on(offset: i32, pitch: i16) -> Event {
        // SAFETY: an all-zero Event is valid plain data.
        let mut e: Event = unsafe { std::mem::zeroed() };
        e.sampleOffset = offset;
        e.r#type = vst3::Steinberg::Vst::Event_::EventTypes_::kNoteOnEvent as u16;
        e.__field0.noteOn.pitch = pitch;
        e
    }

    #[test]
    fn event_list_fills_and_clears_without_allocating() {
        let list = ComWrapper::new(EventList::with_capacity(4));
        let ptr: ComPtr<IEventList> = list.to_com_ptr().unwrap();
        assert_no_alloc::assert_no_alloc(|| {
            for i in 0..6 {
                list.push(note_on(i, 60 + i as i16));
            }
        });
        // SAFETY: plain calls on our own object.
        unsafe {
            assert_eq!(ptr.getEventCount(), 4, "the list keeps what fits");
            let mut e: Event = std::mem::zeroed();
            assert_eq!(ptr.getEvent(2, &mut e), kResultOk);
            assert_eq!(e.sampleOffset, 2);
            assert_eq!(e.__field0.noteOn.pitch, 62);
            assert_ne!(ptr.getEvent(4, &mut e), kResultOk);
        }
        assert_no_alloc::assert_no_alloc(|| list.clear());
        assert!(list.is_empty());
    }

    #[test]
    fn param_changes_group_by_id_without_allocating() {
        let ch = ComWrapper::new(ParamChanges::new(2, 3));
        let ptr: ComPtr<IParameterChanges> = ch.to_com_ptr().unwrap();
        assert_no_alloc::assert_no_alloc(|| {
            assert!(ch.push(7, 0, 0.25));
            assert!(ch.push(9, 0, 2.0)); // clamped
            assert!(ch.push(7, 10, 0.5));
            assert!(!ch.push(11, 0, 0.1), "no free queue");
            // Same offset: the value is replaced; full queue: the last point moves.
            ch.push(7, 10, 0.6);
            ch.push(7, 20, 0.7);
            ch.push(7, 30, 0.8);
        });
        assert_eq!(
            ch.points(),
            vec![(7, 0, 0.25), (7, 10, 0.6), (7, 20, 0.8), (9, 0, 1.0)]
        );
        // SAFETY: plain calls on our own objects.
        unsafe {
            assert_eq!(ptr.getParameterCount(), 2);
            let q = ComPtr::<IParamValueQueue>::from_raw(ptr.getParameterData(1)).map(|q| {
                // from_raw takes over a reference the call did not give: add one back.
                std::mem::ManuallyDrop::new(q)
            });
            let q = q.unwrap();
            assert_eq!(q.getParameterId(), 9);
            let (mut o, mut v) = (0, 0.0);
            assert_eq!(q.getPoint(0, &mut o, &mut v), kResultOk);
            assert_eq!((o, v), (0, 1.0));
            assert!(ptr.getParameterData(2).is_null());
        }
        assert_no_alloc::assert_no_alloc(|| ch.clear());
        assert!(ch.is_empty());
    }

    #[test]
    fn memory_stream_reads_writes_and_seeks() {
        let s = ComWrapper::new(MemoryStream::default());
        let p: ComPtr<IBStream> = s.to_com_ptr().unwrap();
        // SAFETY: plain calls on our own object with live buffers.
        unsafe {
            let mut data = *b"hello";
            let mut n = 0;
            assert_eq!(p.write(data.as_mut_ptr().cast(), 5, &mut n), kResultOk);
            assert_eq!(n, 5);
            let mut at = 0;
            assert_eq!(p.seek(1, 0, &mut at), kResultOk);
            assert_eq!(at, 1);
            let mut out = [0u8; 10];
            assert_eq!(p.read(out.as_mut_ptr().cast(), 10, &mut n), kResultOk);
            assert_eq!((n, &out[..4]), (4, &b"ello"[..]));
            p.tell(&mut at);
            assert_eq!(at, 5);
        }
        assert_eq!(s.bytes(), b"hello");
    }

    #[test]
    fn host_makes_messages_with_attributes() {
        let host = ComWrapper::new(HostApp);
        let h: ComPtr<IHostApplication> = host.to_com_ptr().unwrap();
        // SAFETY: plain calls on our own objects.
        unsafe {
            let mut cid: TUID = IMessage::IID.map(|b| b as c_char);
            let mut iid = cid;
            let mut obj = std::ptr::null_mut();
            assert_eq!(h.createInstance(&mut cid, &mut iid, &mut obj), kResultOk);
            let msg = ComPtr::<IMessage>::from_raw(obj.cast()).unwrap();
            msg.setMessageID(c"hi".as_ptr());
            assert_eq!(CStr::from_ptr(msg.getMessageID()), c"hi");
            let attrs = msg.getAttributes();
            let a = std::mem::ManuallyDrop::new(ComPtr::<IAttributeList>::from_raw(attrs).unwrap());
            a.setInt(c"n".as_ptr(), 42);
            let mut v = 0;
            assert_eq!(a.getInt(c"n".as_ptr(), &mut v), kResultOk);
            assert_eq!(v, 42);
            let mut name: String128 = [0; 128];
            h.getName(&mut name);
            assert_eq!(read_string128(&name), "Pro Wave");
        }
    }
}
