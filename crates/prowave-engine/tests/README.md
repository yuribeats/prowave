# ProWave engine stress checks

## 24-track streaming

Run on an otherwise idle machine:

```sh
cargo test -p prowave-engine --test multitrack_streaming -- --ignored --nocapture
```

The test generates 24 distinct stereo PCM WAV files at 48 kHz/24-bit in a temporary
directory. It opens them through `AudioFile`, streams them on the real background
disk thread, and renders 60 seconds in 256-frame blocks paced against a wall clock.
Playback exceeds the per-clip prefetch capacity, exercising continuous refill.

Every output sample is compared with an independently calculated mix. Assertions
check finite output, sample error below 0.000001, zero streaming underruns, and the
final transport position. Debug builds also detect allocations/deallocations inside
the callback. Timing is reported, not asserted, because OS scheduling varies.

Each clip currently reserves eight 1024-frame stereo blocks (~64 KiB, 170 ms at 48 kHz)
for prefetch. This bounds ring memory as clip counts grow; the 1000-track target still
requires raising the engine's current 64-track limit.

This test is ignored by default because it depends on wall-clock scheduling. It is
not a sound-device latency test, an app import test, or a cold-disk benchmark: newly
generated files can be served from the operating system's file cache. It does not
alone establish the complete Phase 1 acceptance criterion across platforms.

Windows debug result, 2026-10-19: 240,000 frames, zero underruns, maximum sample error
approximately 0.000000001, average render load 8.24%, maximum callback 1.0479 ms,
zero callbacks above the 5.333 ms budget. These are one local run's measurements,
not guaranteed performance targets. Temporary audio files are removed after the test.

Windows debug result, 2026-10-20: 2,880,000 frames over 60 seconds, zero underruns,
maximum sample error 0.000000001, average render load 9.38%, maximum callback 1.453 ms,
zero callbacks above the 5.333 ms budget. This is one local run, not a guaranteed target.

After reducing ring capacity: zero underruns, maximum sample error 0.000000001,
average render load 8.54%, maximum callback 1.344 ms, and zero callbacks over budget.

After bounding import registration work and simplifying tempo advancement: zero underruns,
maximum sample error 0.000000001, average render load 8.77%, maximum callback 1.8514 ms,
and zero callbacks over budget. Differences between local runs also reflect OS scheduling;
these figures do not establish a measured speedup from either change.
