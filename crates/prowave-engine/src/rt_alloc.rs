//! Counts the memory allocations made on the audio thread, for release builds.
//!
//! Debug builds abort on such an allocation (`assert_no_alloc`'s `AllocDisabler`). A release
//! build must not: an Audio Unit or VST 3 plug-in may allocate in its own render call, and
//! aborting would lose the session. `assert_no_alloc`'s own release mode (`warn_release`)
//! does not fit either: it counts per thread, where no other thread can read the number, and
//! prints to stderr from the guarded thread (a system call in the audio callback).
//!
//! So the release app installs [`CountingAlloc`] as its global allocator. Every allocator
//! call reads one thread-local byte: the section the calling thread is in. Outside a section
//! (every thread but the audio threads, and those between blocks) that is all it costs.
//! Inside one, the call is counted in atomics (no lock, no log, no allocation) and then goes
//! to the system allocator as usual. Another thread reads the totals with [`counts`].
//!
//! Sections:
//! - [`guarded`]: our audio-thread code. Counted as [`AllocCounts::ours`].
//! - [`plugin_call`]: a call into a third-party plug-in, inside a guarded section. Counted as
//!   [`AllocCounts::plugin`].
//! - [`host_callback`]: our code called back by a plug-in from inside its call. Ours again.
//!
//! Only calls to Rust's global allocator are seen. A C or C++ plug-in that calls `malloc`
//! itself is invisible here; `plugin` counts what Rust code allocates while a plug-in call is
//! on the stack (a plug-in written in Rust that is linked into this binary, or host code the
//! plug-in calls back that is not marked with [`host_callback`]).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Not in a guarded section: allocator calls are not counted.
const OFF: u8 = 0;
/// In our own audio-thread code.
const OURS: u8 = 1;
/// In a call into a third-party plug-in.
const PLUGIN: u8 = 2;

thread_local! {
    /// The section this thread is in. Constant-initialized and without a destructor, so
    /// reading it never allocates and never registers anything.
    static SECTION: Cell<u8> = const { Cell::new(OFF) };
}

/// One section kind's counters.
struct Counters {
    allocs: AtomicU64,
    frees: AtomicU64,
    reallocs: AtomicU64,
    largest: AtomicUsize,
}

impl Counters {
    const fn new() -> Self {
        Self {
            allocs: AtomicU64::new(0),
            frees: AtomicU64::new(0),
            reallocs: AtomicU64::new(0),
            largest: AtomicUsize::new(0),
        }
    }

    fn read(&self) -> Tally {
        Tally {
            allocs: self.allocs.load(Ordering::Relaxed),
            frees: self.frees.load(Ordering::Relaxed),
            reallocs: self.reallocs.load(Ordering::Relaxed),
            largest_bytes: self.largest.load(Ordering::Relaxed),
        }
    }
}

static OURS_COUNTERS: Counters = Counters::new();
static PLUGIN_COUNTERS: Counters = Counters::new();

/// What the allocator was asked to do.
#[derive(Clone, Copy)]
enum Call {
    Alloc,
    Free,
    Realloc,
}

/// Allocator calls counted in one kind of section since the process started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// New blocks asked for.
    pub allocs: u64,
    /// Blocks given back.
    pub frees: u64,
    /// Blocks resized.
    pub reallocs: u64,
    /// The largest size asked for (a new block's, or a resized block's new size), in bytes.
    pub largest_bytes: usize,
}

impl Tally {
    /// All calls, of every kind.
    pub fn total(&self) -> u64 {
        self.allocs + self.frees + self.reallocs
    }
}

/// What [`counts`] reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocCounts {
    /// Calls from our own audio-thread code: each one breaks the engine's rule.
    pub ours: Tally,
    /// Calls made while a third-party plug-in's call was on the stack.
    pub plugin: Tally,
}

/// The allocator calls counted so far. Any thread; lock-free; does not allocate. All zero
/// unless [`CountingAlloc`] is the global allocator.
pub fn counts() -> AllocCounts {
    AllocCounts {
        ours: OURS_COUNTERS.read(),
        plugin: PLUGIN_COUNTERS.read(),
    }
}

/// Counts one allocator call if this thread is in a section. The one thread-local read
/// every allocation in the program pays.
#[inline]
fn note(call: Call, size: usize) {
    // A thread that is being torn down reads as "not in a section".
    let section = SECTION.try_with(Cell::get).unwrap_or(OFF);
    if section != OFF {
        record(section, call, size);
    }
}

#[cold]
fn record(section: u8, call: Call, size: usize) {
    let c = if section == PLUGIN {
        &PLUGIN_COUNTERS
    } else {
        &OURS_COUNTERS
    };
    match call {
        Call::Alloc => &c.allocs,
        Call::Free => &c.frees,
        Call::Realloc => &c.reallocs,
    }
    .fetch_add(1, Ordering::Relaxed);
    if !matches!(call, Call::Free) {
        c.largest.fetch_max(size, Ordering::Relaxed);
    }
}

/// The system allocator, counting the calls made inside a section (see the module
/// documentation). Never refuses, aborts or logs.
pub struct CountingAlloc;

// SAFETY: every call goes to `System` unchanged; `note` only reads a thread-local byte and
// adds to atomics, so it cannot allocate or re-enter the allocator.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(Call::Alloc, layout.size());
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(Call::Alloc, layout.size());
        System.alloc_zeroed(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(Call::Free, layout.size());
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(Call::Realloc, new_size);
        System.realloc(ptr, layout, new_size)
    }
}

/// Puts the thread in a section until dropped, then back in the one it was in.
struct Enter(u8);

impl Enter {
    fn new(section: impl FnOnce(u8) -> u8) -> Self {
        let before = SECTION.try_with(Cell::get).unwrap_or(OFF);
        let _ = SECTION.try_with(|s| s.set(section(before)));
        Self(before)
    }
}

impl Drop for Enter {
    fn drop(&mut self) {
        let _ = SECTION.try_with(|s| s.set(self.0));
    }
}

/// Runs `f` as audio-thread code that must not allocate. Debug builds abort if it does (as
/// `assert_no_alloc` always did here); release builds count it as ours. Nesting is fine;
/// inside a [`plugin_call`] it stays the plug-in's.
pub fn guarded<T>(f: impl FnOnce() -> T) -> T {
    let _section = Enter::new(|before| if before == OFF { OURS } else { before });
    assert_no_alloc::assert_no_alloc(f)
}

/// Runs `f`, a call into a third-party plug-in (its render, its MIDI or parameter entry).
/// Inside a guarded section, what is allocated meanwhile counts as the plug-in's, not ours.
/// Outside one (the interface thread, an offline bounce) nothing is counted. Debug builds
/// are unchanged: an allocation through our allocator in there still aborts.
pub fn plugin_call<T>(f: impl FnOnce() -> T) -> T {
    let _section = Enter::new(|before| if before == OFF { OFF } else { PLUGIN });
    f()
}

/// Runs `f`, our own code that a plug-in calls back from inside its call (an event list it
/// writes to, say): what it allocates is ours again.
pub fn host_callback<T>(f: impl FnOnce() -> T) -> T {
    let _section = Enter::new(|before| if before == PLUGIN { OURS } else { before });
    f()
}
