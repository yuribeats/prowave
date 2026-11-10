//! Memory allocations on the audio thread, as the release build counts them
//! (`prowave_engine::rt_alloc`): the interface reads the counters about once a second and
//! writes one log line when the count of our own code's has risen. A debug build aborts on
//! such an allocation instead, so there the counters stay at zero.

use prowave_engine::rt_alloc::{AllocCounts, Tally};

/// Seconds between two reads of the counters.
const READ_EVERY: f64 = 1.0;
/// The least time between two log lines, however fast the count rises.
const LOG_EVERY: f64 = 60.0;

/// The interface's view of the counters.
#[derive(Default)]
pub struct AllocReport {
    /// Allocator calls from our own audio-thread code so far (what the CPU meter's tooltip
    /// shows).
    pub ours: u64,
    /// The count the last log line named.
    logged: u64,
    next_read: f64,
    next_log: f64,
}

impl AllocReport {
    /// Takes `counts` if a second has passed since the last read (`now` in seconds, never
    /// going back). Returns the line to log when our own count has risen since the last
    /// line and that line is at least a minute old.
    pub fn poll(&mut self, now: f64, counts: impl FnOnce() -> AllocCounts) -> Option<String> {
        if now < self.next_read {
            return None;
        }
        self.next_read = now + READ_EVERY;
        let c = counts();
        self.ours = c.ours.total();
        if self.ours <= self.logged || now < self.next_log {
            return None;
        }
        let new = self.ours - self.logged;
        self.logged = self.ours;
        self.next_log = now + LOG_EVERY;
        Some(format!(
            "audio thread: {} allocator calls from Pro Wave's own code so far ({new} new): {}; \
             inside plug-in calls: {}",
            self.ours,
            describe(&c.ours),
            c.plugin.total()
        ))
    }
}

/// The parts of a count, for a log or a terminal.
fn describe(t: &Tally) -> String {
    format!(
        "{} allocations, {} frees, {} resizes, largest request {} bytes",
        t.allocs, t.frees, t.reallocs, t.largest_bytes
    )
}

/// The lines `--dsp-load` prints at the end of its run: both counts as they stand, then
/// whether the counter is live (without that, a zero could also mean "not installed").
pub fn terminal_lines() -> String {
    if cfg!(debug_assertions) {
        return "audio-thread allocations: not counted in a debug build (it aborts on one)".into();
    }
    let c = prowave_engine::rt_alloc::counts();
    format!(
        "audio-thread allocations, ours: {} ({})\n\
         audio-thread allocations, inside plug-in calls: {} ({})\n\
         allocation counter: {}",
        c.ours.total(),
        describe(&c.ours),
        c.plugin.total(),
        describe(&c.plugin),
        if counter_is_live() {
            "live (a test allocation was counted)"
        } else {
            "NOT live (a test allocation was not counted)"
        }
    )
}

/// Allocates once in a guarded section on this thread and says whether the count rose. The
/// counters keep that test allocation, so this is for the end of a terminal check only.
/// Never in a debug build: there the allocation aborts.
fn counter_is_live() -> bool {
    let before = prowave_engine::rt_alloc::counts().ours.allocs;
    prowave_engine::rt_alloc::guarded(|| {
        drop(std::hint::black_box(Vec::<u8>::with_capacity(
            std::hint::black_box(64),
        )));
    });
    prowave_engine::rt_alloc::counts().ours.allocs > before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours(allocs: u64) -> AllocCounts {
        AllocCounts {
            ours: Tally {
                allocs,
                frees: 0,
                reallocs: 0,
                largest_bytes: 64,
            },
            plugin: Tally::default(),
        }
    }

    #[test]
    fn nothing_is_logged_while_the_count_stays_at_zero() {
        let mut r = AllocReport::default();
        for s in 0..300 {
            assert_eq!(r.poll(f64::from(s), || ours(0)), None);
        }
        assert_eq!(r.ours, 0);
    }

    #[test]
    fn the_counters_are_read_once_a_second() {
        let mut r = AllocReport::default();
        let mut reads = 0;
        // 60 frames a second for three seconds.
        for frame in 0..180 {
            r.poll(f64::from(frame) / 60.0, || {
                reads += 1;
                ours(0)
            });
        }
        assert_eq!(reads, 3);
    }

    #[test]
    fn a_rise_is_logged_once_and_not_again_until_it_rises_more() {
        let mut r = AllocReport::default();
        let line = r.poll(0.0, || ours(3)).unwrap();
        assert!(line.contains("3 allocator calls"), "{line}");
        assert!(line.contains("(3 new)"), "{line}");
        assert_eq!(r.ours, 3);
        for s in 1..600 {
            assert_eq!(r.poll(f64::from(s), || ours(3)), None, "second {s}");
        }
        let line = r.poll(600.0, || ours(5)).unwrap();
        assert!(line.contains("5 allocator calls"), "{line}");
        assert!(line.contains("(2 new)"), "{line}");
    }

    #[test]
    fn a_count_that_keeps_rising_is_logged_once_a_minute_at_most() {
        let mut r = AllocReport::default();
        let mut lines = Vec::new();
        for s in 0..600u32 {
            lines.extend(r.poll(f64::from(s), || ours(u64::from(s) + 1)));
        }
        assert_eq!(lines.len(), 10, "{lines:?}");
        assert!(lines[1].contains("61 allocator calls"), "{}", lines[1]);
        assert!(lines[1].contains("(60 new)"), "{}", lines[1]);
        // What the tooltip shows follows every read all the same.
        assert_eq!(r.ours, 600);
    }

    #[test]
    fn plugin_allocations_alone_are_not_logged() {
        let mut r = AllocReport::default();
        let c = AllocCounts {
            ours: Tally::default(),
            plugin: Tally {
                allocs: 9,
                frees: 9,
                reallocs: 0,
                largest_bytes: 1024,
            },
        };
        assert_eq!(r.poll(0.0, || c), None);
        assert_eq!(r.ours, 0);
    }
}
