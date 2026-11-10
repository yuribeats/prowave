//! The release build's allocation counter (`rt_alloc`), with [`CountingAlloc`] as this test
//! binary's global allocator, as the release app installs it. Real allocations, real counts.

use prowave_engine::rt_alloc::{
    counts, guarded, host_callback, plugin_call, AllocCounts, CountingAlloc, Tally,
};
use std::hint::black_box;
use std::sync::Mutex;

#[global_allocator]
static COUNTING: CountingAlloc = CountingAlloc;

/// The counters are the whole process's: one test at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// What `f` added to the counters.
fn counted(f: impl FnOnce()) -> AllocCounts {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let before = counts();
    f();
    let after = counts();
    // The largest request is not a count: it is passed on as read.
    let diff = |a: Tally, b: Tally| Tally {
        allocs: a.allocs - b.allocs,
        frees: a.frees - b.frees,
        reallocs: a.reallocs - b.reallocs,
        largest_bytes: a.largest_bytes,
    };
    AllocCounts {
        ours: diff(after.ours, before.ours),
        plugin: diff(after.plugin, before.plugin),
    }
}

/// One allocation and one free of `bytes` bytes.
fn allocate(bytes: usize) {
    drop(black_box(Vec::<u8>::with_capacity(black_box(bytes))));
}

#[test]
fn an_allocation_in_a_guarded_section_counts_as_ours() {
    let c = counted(|| guarded(|| allocate(100)));
    assert_eq!((c.ours.allocs, c.ours.frees, c.ours.reallocs), (1, 1, 0));
    assert_eq!(c.ours.total(), 2);
    assert_eq!(c.plugin.total(), 0);
}

#[test]
fn an_allocation_in_a_plugin_call_counts_as_the_plugins() {
    let c = counted(|| guarded(|| plugin_call(|| allocate(100))));
    assert_eq!(c.ours.total(), 0);
    assert_eq!(
        (c.plugin.allocs, c.plugin.frees, c.plugin.reallocs),
        (1, 1, 0)
    );
}

#[test]
fn an_unguarded_allocation_counts_nothing() {
    let c = counted(|| {
        allocate(100);
        // Not on the audio thread: a plug-in call or a callback alone is not a section.
        plugin_call(|| allocate(100));
        host_callback(|| allocate(100));
    });
    assert_eq!(c.ours.total(), 0);
    assert_eq!(c.plugin.total(), 0);
}

#[test]
fn sections_nest_and_end() {
    let c = counted(|| {
        guarded(|| {
            allocate(8); // ours
            guarded(|| allocate(8)); // ours
            plugin_call(|| {
                allocate(8); // the plug-in's
                plugin_call(|| allocate(8)); // the plug-in's
                guarded(|| allocate(8)); // still the plug-in's
                host_callback(|| {
                    allocate(8); // ours
                    plugin_call(|| allocate(8)); // the plug-in's
                });
                allocate(8); // the plug-in's again
            });
            host_callback(|| allocate(8)); // ours
            allocate(8); // ours again
        });
        allocate(8); // after the section: not counted
    });
    assert_eq!((c.ours.allocs, c.ours.frees), (5, 5));
    assert_eq!((c.plugin.allocs, c.plugin.frees), (5, 5));
}

#[test]
fn a_section_ends_when_its_code_panics() {
    counted(|| {
        let r = std::panic::catch_unwind(|| guarded(|| plugin_call(|| panic!("in a section"))));
        assert!(r.is_err());
        let before = counts();
        allocate(8);
        assert_eq!(counts(), before, "the thread is still in a section");
    });
}

#[test]
fn growing_and_the_largest_request_are_recorded() {
    let c = counted(|| {
        let mut v: Vec<u8> = Vec::with_capacity(16);
        guarded(|| {
            v.reserve_exact(black_box(1 << 20));
            black_box(&mut v);
        });
        drop(v);
    });
    assert_eq!((c.ours.allocs, c.ours.frees, c.ours.reallocs), (0, 0, 1));
    assert!(c.ours.largest_bytes >= 1 << 20, "{}", c.ours.largest_bytes);
}

#[test]
fn another_threads_allocations_are_not_counted() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (go, done) = (AtomicBool::new(false), AtomicBool::new(false));
    let c = counted(|| {
        std::thread::scope(|s| {
            s.spawn(|| {
                while !go.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                for _ in 0..100 {
                    allocate(64);
                }
                done.store(true, Ordering::Release);
            });
            // This thread is in a section while the other one allocates.
            guarded(|| {
                go.store(true, Ordering::Release);
                while !done.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
            });
        });
    });
    assert_eq!(c.ours.total(), 0);
    assert_eq!(c.plugin.total(), 0);
}

#[test]
fn counting_and_the_sections_themselves_do_not_allocate() {
    let c = counted(|| {
        guarded(|| {
            for _ in 0..1000 {
                plugin_call(|| host_callback(|| black_box(counts())));
                black_box(guarded(counts));
            }
        });
    });
    assert_eq!(c.ours.total(), 0);
    assert_eq!(c.plugin.total(), 0);
}
