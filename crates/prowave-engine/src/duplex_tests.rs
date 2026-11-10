//! Tests of [`super`] (the Core Audio duplex stream).
//!
//! - [`pure`]: no system call at all; these also run under miri (`cargo xtask check`). The
//!   callback is run on made-up buffer lists with a made-up clock.
//! - [`hal`]: calls into Core Audio / Core Foundation that need no device and start no I/O.
//! - [`device`]: opens a real device and only ever plays silence; `#[ignore]`d.
use super::*;

/// Samples the HAL would own, with a guard either side to show a write or read out of bounds.
struct Region {
    mem: Vec<f32>,
    len: usize,
}

const GUARD: usize = 16;
/// What the guards hold. No test renders or feeds this value.
const CANARY: f32 = -7777.0;
/// What an output buffer holds before the callback: not silence.
const STALE: f32 = 0.5;

impl Region {
    fn filled(len: usize, value: f32) -> Self {
        let mut mem = vec![CANARY; len + 2 * GUARD];
        mem[GUARD..GUARD + len].fill(value);
        Self { mem, len }
    }

    /// An output buffer of `len` samples as the HAL may hand it over: with something in it.
    fn stale(len: usize) -> Self {
        Self::filled(len, STALE)
    }

    fn holding(samples: &[f32]) -> Self {
        let mut r = Self::filled(samples.len(), 0.0);
        r.mem[GUARD..GUARD + samples.len()].copy_from_slice(samples);
        r
    }

    /// A buffer of `channels` channels over all of the samples.
    fn buffer(&mut self, channels: u32) -> AudioBuffer {
        self.buffer_claiming(channels, self.len * 4)
    }

    /// A buffer that says it holds `bytes` bytes (the caller keeps that within the samples).
    fn buffer_claiming(&mut self, channels: u32, bytes: usize) -> AudioBuffer {
        assert!(bytes <= self.len * 4);
        AudioBuffer {
            mNumberChannels: channels,
            mDataByteSize: bytes as u32,
            mData: self.mem[GUARD..].as_mut_ptr().cast(),
        }
    }

    fn samples(&self) -> &[f32] {
        &self.mem[GUARD..GUARD + self.len]
    }

    /// Nothing wrote outside the samples.
    fn intact(&self) -> bool {
        let (before, rest) = self.mem.split_at(GUARD);
        before.iter().chain(&rest[self.len..]).all(|s| *s == CANARY)
    }
}

fn null_buffer(channels: u32, bytes: u32) -> AudioBuffer {
    AudioBuffer {
        mNumberChannels: channels,
        mDataByteSize: bytes,
        mData: std::ptr::null_mut(),
    }
}

fn stamp(host: u64) -> AudioTimeStamp {
    // SAFETY: a C struct of integers and floats; all zeroes is a valid value of it.
    let mut t: AudioTimeStamp = unsafe { std::mem::zeroed() };
    t.mHostTime = host;
    t
}

fn info(in_skip: usize, in_channels: usize, out_channels: usize) -> DuplexInfo {
    DuplexInfo {
        device_name: String::new(),
        input_name: None,
        output_uid: String::new(),
        input_uid: None,
        sample_rate: 48_000,
        buffer_frames: 64,
        in_channels,
        out_channels,
        in_skip,
        input_latency_frames: 0,
        output_latency_frames: 0,
        aggregate: false,
    }
}

/// What the rendering closure writes at frame `f` of channel `c`: never 0, never the canary.
fn tone(f: usize, c: usize) -> f32 {
    (f * 100 + c + 1) as f32
}

/// What a test feeds as input at frame `f` of device channel `c`.
fn mic(f: usize, c: usize) -> f32 {
    -((f * 100 + c + 1) as f32)
}

fn interleaved(
    frames: usize,
    channels: usize,
    first_channel: usize,
    of: fn(usize, usize) -> f32,
) -> Vec<f32> {
    (0..frames)
        .flat_map(|f| (0..channels).map(move |c| of(f, first_channel + c)))
        .collect()
}

mod pure {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What the closure was called with.
    #[derive(Clone, Debug, Default, PartialEq)]
    struct Seen {
        calls: usize,
        input: Vec<f32>,
        in_channels: usize,
        out_channels: usize,
        out_len: usize,
        /// The output block was all zero when the closure got it.
        out_was_silent: bool,
        output_latency_us: u32,
        input_latency_us: u32,
        now_ns: u64,
        sample_time: f64,
    }

    /// A context set up as `start_counted` does, whose closure notes what it gets and
    /// renders [`tone`] on every channel when `render` is set.
    fn ctx_for(info: &DuplexInfo, render: bool) -> (Ctx<impl FnMut(Block<'_>)>, Rc<RefCell<Seen>>) {
        let seen = Rc::new(RefCell::new(Seen::default()));
        let seen_rt = seen.clone();
        let mut ctx = Ctx::new(move |b: Block<'_>| {
            let mut s = seen_rt.borrow_mut();
            s.calls += 1;
            s.input = b.input.to_vec();
            s.in_channels = b.in_channels;
            s.out_channels = b.out_channels;
            s.out_len = b.output.len();
            s.out_was_silent = b.output.iter().all(|x| *x == 0.0);
            s.output_latency_us = b.output_latency_us;
            s.input_latency_us = b.input_latency_us;
            s.now_ns = b.now_ns;
            s.sample_time = b.sample_time;
            if render && b.out_channels > 0 {
                for (f, frame) in b.output.chunks_exact_mut(b.out_channels).enumerate() {
                    for (c, x) in frame.iter_mut().enumerate() {
                        *x = tone(f, c);
                    }
                }
            }
        });
        ctx.configure(info);
        (ctx, seen)
    }

    /// One callback with a clock on which a host tick is a nanosecond.
    fn run_at<F: FnMut(Block<'_>)>(
        ctx: &mut Ctx<F>,
        ins: &[AudioBuffer],
        outs: &[AudioBuffer],
        (now, in_t, out_t): (&AudioTimeStamp, &AudioTimeStamp, &AudioTimeStamp),
    ) {
        // SAFETY: every buffer the tests build points into a live `Region` (or is null) and
        // claims no more bytes than the region has; the context comes from `ctx_for`.
        unsafe {
            run_block(ctx, ins, outs, now, in_t, out_t, |later, earlier| {
                later.saturating_sub(earlier)
            });
        }
    }

    fn run<F: FnMut(Block<'_>)>(ctx: &mut Ctx<F>, ins: &[AudioBuffer], outs: &[AudioBuffer]) {
        run_at(ctx, ins, outs, (&stamp(0), &stamp(0), &stamp(0)));
    }

    // ---- the callback: output

    #[test]
    fn an_interleaved_stereo_output_gets_the_rendered_frames_in_order() {
        let (mut ctx, seen) = ctx_for(&info(0, 0, 2), true);
        let mut out = Region::stale(8 * 2);
        run(&mut ctx, &[], &[out.buffer(2)]);
        assert_eq!(out.samples(), interleaved(8, 2, 0, tone));
        assert!(out.intact());
        let seen = seen.borrow();
        assert_eq!((seen.calls, seen.out_len, seen.out_channels), (1, 16, 2));
        assert!(
            seen.out_was_silent,
            "the block is zeroed before the closure"
        );
    }

    #[test]
    fn a_mono_output_gets_the_one_channel() {
        let (mut ctx, _) = ctx_for(&info(0, 0, 1), true);
        let mut out = Region::stale(5);
        run(&mut ctx, &[], &[out.buffer(1)]);
        assert_eq!(out.samples(), interleaved(5, 1, 0, tone));
        assert!(out.intact());
    }

    #[test]
    fn non_interleaved_outputs_each_get_their_own_channel() {
        let (mut ctx, seen) = ctx_for(&info(0, 0, 2), true);
        let (mut left, mut right) = (Region::stale(6), Region::stale(6));
        run(&mut ctx, &[], &[left.buffer(1), right.buffer(1)]);
        assert_eq!(left.samples(), interleaved(6, 1, 0, tone));
        assert_eq!(right.samples(), interleaved(6, 1, 1, tone));
        assert!(left.intact() && right.intact());
        assert_eq!(seen.borrow().out_len, 12);
    }

    #[test]
    fn streams_of_different_widths_take_consecutive_channels() {
        let (mut ctx, _) = ctx_for(&info(0, 0, 6), true);
        let (mut a, mut b, mut c) = (Region::stale(4 * 2), Region::stale(4), Region::stale(4 * 3));
        run(&mut ctx, &[], &[a.buffer(2), b.buffer(1), c.buffer(3)]);
        assert_eq!(a.samples(), interleaved(4, 2, 0, tone));
        assert_eq!(b.samples(), interleaved(4, 1, 2, tone));
        assert_eq!(c.samples(), interleaved(4, 3, 3, tone));
        assert!(a.intact() && b.intact() && c.intact());
    }

    #[test]
    fn device_channels_beyond_the_rendered_ones_are_silenced() {
        // The stream renders 2 channels; the device has 4 in one stream and 2 more in another.
        let (mut ctx, _) = ctx_for(&info(0, 0, 2), true);
        let (mut main, mut more) = (Region::stale(3 * 4), Region::stale(3 * 2));
        run(&mut ctx, &[], &[main.buffer(4), more.buffer(2)]);
        for (f, frame) in main.samples().as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(*frame, [tone(f, 0), tone(f, 1), 0.0, 0.0]);
        }
        assert_eq!(more.samples(), [0.0; 6]);
        assert!(main.intact() && more.intact());
    }

    #[test]
    fn fewer_device_channels_than_rendered_get_the_first_ones() {
        let (mut ctx, _) = ctx_for(&info(0, 0, 4), true);
        let mut out = Region::stale(3 * 2);
        run(&mut ctx, &[], &[out.buffer(2)]);
        assert_eq!(out.samples(), interleaved(3, 2, 0, tone));
        assert!(out.intact());
    }

    #[test]
    fn the_output_is_silence_when_the_closure_writes_nothing() {
        let (mut ctx, _) = ctx_for(&info(0, 0, 2), false);
        let mut out = Region::stale(8 * 2);
        run(&mut ctx, &[], &[out.buffer(2)]);
        assert_eq!(out.samples(), [0.0; 16]);
        assert!(out.intact());
    }

    #[test]
    fn each_callback_starts_from_a_silent_block() {
        let (mut ctx, seen) = ctx_for(&info(0, 0, 2), true);
        let mut out = Region::stale(8 * 2);
        run(&mut ctx, &[], &[out.buffer(2)]);
        run(&mut ctx, &[], &[out.buffer(2)]);
        let seen = seen.borrow();
        assert_eq!(seen.calls, 2);
        assert!(seen.out_was_silent, "what the call before rendered is gone");
    }

    #[test]
    fn an_output_with_no_data_is_skipped_but_keeps_its_channels() {
        let (mut ctx, seen) = ctx_for(&info(0, 0, 3), true);
        let mut real = Region::stale(4);
        // 4 frames of 2 channels, but no memory behind it.
        run(&mut ctx, &[], &[null_buffer(2, 32), real.buffer(1)]);
        assert_eq!(
            real.samples(),
            interleaved(4, 1, 2, tone),
            "channel 2, not 0"
        );
        assert!(real.intact());
        assert_eq!(seen.borrow().out_len, 4 * 3);
    }

    #[test]
    fn outputs_with_no_channels_are_ignored() {
        let (mut ctx, _) = ctx_for(&info(0, 0, 2), true);
        let (mut empty, mut out) = (Region::stale(8), Region::stale(4 * 2));
        run(&mut ctx, &[], &[empty.buffer(0), out.buffer(2)]);
        assert_eq!(out.samples(), interleaved(4, 2, 0, tone));
        assert_eq!(empty.samples(), [STALE; 8], "no channels: nothing to write");
        assert!(empty.intact() && out.intact());
    }

    #[test]
    fn a_call_without_output_buffers_does_not_reach_the_closure() {
        // What an input-only call would look like: the stream always has an output, so
        // there is no block to render and the input is not passed on.
        let (mut ctx, seen) = ctx_for(&info(0, 2, 2), true);
        let mut input = Region::holding(&interleaved(4, 2, 0, mic));
        run(&mut ctx, &[input.buffer(2)], &[]);
        run(&mut ctx, &[input.buffer(2)], &[null_buffer(0, 64)]);
        assert_eq!(seen.borrow().calls, 0);
        assert!(input.intact());
    }

    #[test]
    fn an_output_of_no_bytes_gives_a_callback_of_no_frames() {
        let (mut ctx, seen) = ctx_for(&info(0, 2, 2), true);
        let mut out = Region::stale(8);
        let mut input = Region::holding(&interleaved(4, 2, 0, mic));
        run(&mut ctx, &[input.buffer(2)], &[out.buffer_claiming(2, 0)]);
        let seen = seen.borrow();
        assert_eq!((seen.calls, seen.out_len, seen.input.len()), (1, 0, 0));
        assert_eq!(
            out.samples(),
            [STALE; 8],
            "it claimed no bytes: none are written"
        );
        assert!(out.intact());
    }

    #[test]
    fn a_shorter_second_output_is_written_only_as_far_as_it_goes() {
        let (mut ctx, _) = ctx_for(&info(0, 0, 2), true);
        let (mut left, mut right) = (Region::stale(8), Region::stale(8));
        // The right stream says it holds 3 frames, not 8.
        run(
            &mut ctx,
            &[],
            &[left.buffer(1), right.buffer_claiming(1, 3 * 4)],
        );
        assert_eq!(left.samples(), interleaved(8, 1, 0, tone));
        assert_eq!(right.samples()[..3], interleaved(3, 1, 1, tone));
        assert_eq!(
            right.samples()[3..],
            [STALE; 5],
            "beyond its byte size: not ours"
        );
        assert!(left.intact() && right.intact());
    }

    #[test]
    fn a_byte_size_that_is_not_whole_frames_is_rounded_down() {
        let (mut ctx, seen) = ctx_for(&info(0, 0, 2), true);
        let mut out = Region::stale(2 * 5 + 1);
        // 5 stereo frames, one sample and 3 bytes more.
        run(
            &mut ctx,
            &[],
            &[out.buffer_claiming(2, (2 * 5 + 1) * 4 - 1)],
        );
        assert_eq!(out.samples()[..10], interleaved(5, 2, 0, tone));
        assert_eq!(seen.borrow().out_len, 10);
        assert!(out.intact());
    }

    #[test]
    fn more_frames_than_one_callback_carries_are_cut_to_the_limit() {
        let frames = MAX_FRAMES + 50;
        let (mut ctx, seen) = ctx_for(&info(0, 1, 1), true);
        let mut out = Region::stale(frames);
        let mut input = Region::holding(&interleaved(frames, 1, 0, mic));
        run(&mut ctx, &[input.buffer(1)], &[out.buffer(1)]);
        let seen = seen.borrow();
        assert_eq!(seen.out_len, MAX_FRAMES);
        assert_eq!(seen.input, interleaved(MAX_FRAMES, 1, 0, mic));
        assert_eq!(
            out.samples()[..MAX_FRAMES],
            interleaved(MAX_FRAMES, 1, 0, tone)
        );
        assert!(out.intact() && input.intact());
    }

    // The widest and longest block there can be: every index into the context's own buffers
    // is at its largest. (Too slow for miri: 800,000 samples each way.)
    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_largest_block_stays_inside_the_contexts_buffers() {
        // A device with more channels than the stream takes.
        let (mut ctx, seen) = ctx_for(&info(3, 500, 500), true);
        assert_eq!(
            (ctx.in_channels, ctx.out_channels),
            (MAX_IN_CHANNELS, MAX_OUT_CHANNELS)
        );
        let frames = MAX_FRAMES + 7;
        let (dev_in, dev_out) = (3 + MAX_IN_CHANNELS + 5, MAX_OUT_CHANNELS + 6);
        let mut out = Region::stale(frames * dev_out);
        let mut input = Region::holding(&interleaved(frames, dev_in, 0, mic));
        run(
            &mut ctx,
            &[input.buffer(dev_in as u32)],
            &[out.buffer(dev_out as u32)],
        );
        let seen = seen.borrow();
        assert_eq!(seen.out_len, MAX_FRAMES * MAX_OUT_CHANNELS);
        assert_eq!(seen.input, interleaved(MAX_FRAMES, MAX_IN_CHANNELS, 3, mic));
        let last = &out.samples()[(MAX_FRAMES - 1) * dev_out..MAX_FRAMES * dev_out];
        assert_eq!(
            last[..MAX_OUT_CHANNELS],
            interleaved(MAX_FRAMES, MAX_OUT_CHANNELS, 0, tone)
                [(MAX_FRAMES - 1) * MAX_OUT_CHANNELS..]
        );
        assert_eq!(last[MAX_OUT_CHANNELS..], [0.0; 6]);
        assert!(out.intact() && input.intact());
    }

    // ---- the callback: input

    #[test]
    fn an_interleaved_stereo_input_reaches_the_closure_exactly() {
        let (mut ctx, seen) = ctx_for(&info(0, 2, 2), true);
        let fed = interleaved(8, 2, 0, mic);
        let (mut input, mut out) = (Region::holding(&fed), Region::stale(8 * 2));
        run(&mut ctx, &[input.buffer(2)], &[out.buffer(2)]);
        let seen = seen.borrow();
        assert_eq!(seen.input, fed);
        assert_eq!(seen.in_channels, 2);
        assert_eq!(input.samples(), fed, "the input is only read");
        assert!(input.intact());
    }

    #[test]
    fn a_mono_input_reaches_the_closure_exactly() {
        let (mut ctx, seen) = ctx_for(&info(0, 1, 2), true);
        let fed = interleaved(8, 1, 0, mic);
        let (mut input, mut out) = (Region::holding(&fed), Region::stale(8 * 2));
        run(&mut ctx, &[input.buffer(1)], &[out.buffer(2)]);
        assert_eq!(seen.borrow().input, fed);
    }

    #[test]
    fn non_interleaved_inputs_are_interleaved_in_channel_order() {
        let (mut ctx, seen) = ctx_for(&info(0, 3, 2), true);
        let mut streams = [
            Region::holding(&interleaved(5, 1, 0, mic)),
            Region::holding(&interleaved(5, 1, 1, mic)),
            Region::holding(&interleaved(5, 1, 2, mic)),
        ];
        let ins: Vec<AudioBuffer> = streams.iter_mut().map(|s| s.buffer(1)).collect();
        let mut out = Region::stale(5 * 2);
        run(&mut ctx, &ins, &[out.buffer(2)]);
        assert_eq!(seen.borrow().input, interleaved(5, 3, 0, mic));
    }

    #[test]
    fn a_joined_devices_leading_inputs_are_skipped() {
        // An aggregate: the output device's own 2 inputs come first, then the chosen
        // input's 2, then a stream nobody asked for.
        let (mut ctx, seen) = ctx_for(&info(2, 2, 2), true);
        let mut theirs = Region::holding(&interleaved(4, 2, 0, mic));
        let mut ours = Region::holding(&interleaved(4, 2, 2, mic));
        let mut extra = Region::holding(&interleaved(4, 2, 4, mic));
        let mut out = Region::stale(4 * 2);
        run(
            &mut ctx,
            &[theirs.buffer(2), ours.buffer(2), extra.buffer(2)],
            &[out.buffer(2)],
        );
        assert_eq!(seen.borrow().input, interleaved(4, 2, 2, mic));
    }

    #[test]
    fn the_skip_can_fall_inside_one_wide_stream() {
        // One 6-channel stream; ours are device channels 1, 2 and 3.
        let (mut ctx, seen) = ctx_for(&info(1, 3, 2), true);
        let mut input = Region::holding(&interleaved(4, 6, 0, mic));
        let mut out = Region::stale(4 * 2);
        run(&mut ctx, &[input.buffer(6)], &[out.buffer(2)]);
        assert_eq!(seen.borrow().input, interleaved(4, 3, 1, mic));
    }

    #[test]
    fn the_skip_can_split_a_stream_between_two_devices() {
        // Streams of 2 + 2 channels with 1 skipped: ours are channels 1 and 2, one from each.
        let (mut ctx, seen) = ctx_for(&info(1, 2, 2), true);
        let mut a = Region::holding(&interleaved(4, 2, 0, mic));
        let mut b = Region::holding(&interleaved(4, 2, 2, mic));
        let mut out = Region::stale(4 * 2);
        run(&mut ctx, &[a.buffer(2), b.buffer(2)], &[out.buffer(2)]);
        assert_eq!(seen.borrow().input, interleaved(4, 2, 1, mic));
    }

    #[test]
    fn input_channels_the_device_does_not_deliver_are_silent() {
        let (mut ctx, seen) = ctx_for(&info(0, 4, 2), true);
        let mut input = Region::holding(&interleaved(4, 2, 0, mic));
        let mut out = Region::stale(4 * 2);
        run(&mut ctx, &[input.buffer(2)], &[out.buffer(2)]);
        let seen = seen.borrow();
        assert_eq!(seen.in_channels, 4);
        for (f, frame) in seen.input.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(*frame, [mic(f, 0), mic(f, 1), 0.0, 0.0]);
        }
    }

    #[test]
    fn an_output_only_call_gives_a_silent_input_of_the_right_size() {
        let (mut ctx, seen) = ctx_for(&info(0, 2, 2), true);
        let mut input = Region::holding(&interleaved(4, 2, 0, mic));
        let mut out = Region::stale(4 * 2);
        // First with input, so the context's input buffer holds something.
        run(&mut ctx, &[input.buffer(2)], &[out.buffer(2)]);
        run(&mut ctx, &[], &[out.buffer(2)]);
        assert_eq!(seen.borrow().input, [0.0; 8], "not the call before's input");
        assert_eq!(out.samples(), interleaved(4, 2, 0, tone));
    }

    #[test]
    fn an_input_with_no_data_or_no_bytes_is_silence() {
        let (mut ctx, seen) = ctx_for(&info(0, 2, 2), true);
        let mut input = Region::holding(&interleaved(4, 2, 0, mic));
        let mut out = Region::stale(4 * 2);
        run(&mut ctx, &[input.buffer(2)], &[out.buffer(2)]);
        run(&mut ctx, &[null_buffer(2, 32)], &[out.buffer(2)]);
        assert_eq!(seen.borrow().input, [0.0; 8]);
        run(&mut ctx, &[input.buffer(2)], &[out.buffer(2)]);
        run(&mut ctx, &[input.buffer_claiming(2, 0)], &[out.buffer(2)]);
        assert_eq!(seen.borrow().input, [0.0; 8]);
    }

    #[test]
    fn an_input_with_no_data_still_counts_its_channels() {
        let (mut ctx, seen) = ctx_for(&info(0, 3, 2), true);
        let mut real = Region::holding(&interleaved(4, 1, 2, mic));
        let mut out = Region::stale(4 * 2);
        run(
            &mut ctx,
            &[null_buffer(2, 32), real.buffer(1)],
            &[out.buffer(2)],
        );
        for (f, frame) in seen.borrow().input.as_chunks::<3>().0.iter().enumerate() {
            assert_eq!(*frame, [0.0, 0.0, mic(f, 2)]);
        }
    }

    #[test]
    fn a_short_input_is_read_only_as_far_as_it_goes() {
        let (mut ctx, seen) = ctx_for(&info(0, 2, 2), true);
        let mut input = Region::holding(&interleaved(8, 2, 0, mic));
        let mut out = Region::stale(8 * 2);
        // 8 frames are due; the input says it holds 3 (and one sample more).
        run(
            &mut ctx,
            &[input.buffer_claiming(2, (3 * 2 + 1) * 4)],
            &[out.buffer(2)],
        );
        let seen = seen.borrow();
        assert_eq!(seen.input[..6], interleaved(3, 2, 0, mic));
        assert_eq!(
            seen.input[6..],
            [0.0; 10],
            "nothing is read past its byte size"
        );
    }

    #[test]
    fn an_input_longer_than_the_output_is_cut_to_the_callback() {
        let (mut ctx, seen) = ctx_for(&info(0, 2, 2), true);
        let mut input = Region::holding(&interleaved(16, 2, 0, mic));
        let mut out = Region::stale(8 * 2);
        run(&mut ctx, &[input.buffer(2)], &[out.buffer(2)]);
        assert_eq!(seen.borrow().input, interleaved(8, 2, 0, mic));
    }

    #[test]
    fn a_stream_without_input_passes_none_on() {
        let (mut ctx, seen) = ctx_for(&info(0, 0, 2), true);
        let mut input = Region::holding(&interleaved(4, 2, 0, mic));
        let mut out = Region::stale(4 * 2);
        run(&mut ctx, &[input.buffer(2)], &[out.buffer(2)]);
        let seen = seen.borrow();
        assert_eq!((seen.input.len(), seen.in_channels), (0, 0));
    }

    #[test]
    fn the_closure_may_pass_the_input_through() {
        // Software monitoring: what comes in goes out in the same callback.
        let mut ctx = Ctx::new(|b: Block<'_>| b.output.copy_from_slice(b.input));
        ctx.configure(&info(0, 2, 2));
        let fed = interleaved(8, 2, 0, mic);
        let (mut input, mut out) = (Region::holding(&fed), Region::stale(8 * 2));
        run(&mut ctx, &[input.buffer(2)], &[out.buffer(2)]);
        assert_eq!(out.samples(), fed);
    }

    #[test]
    fn the_callback_does_not_allocate() {
        let mut ctx = Ctx::new(|b: Block<'_>| b.output.copy_from_slice(b.input));
        ctx.configure(&info(1, 2, 2));
        let mut input = Region::holding(&interleaved(64, 3, 0, mic));
        let mut out = Region::stale(64 * 2);
        let (ins, outs) = ([input.buffer(3)], [out.buffer(2)]);
        // Aborts the test run if anything inside allocates or frees (debug builds).
        assert_no_alloc::assert_no_alloc(|| run(&mut ctx, &ins, &outs));
        assert_eq!(out.samples(), interleaved(64, 2, 1, mic));
    }

    // ---- the callback: times

    #[test]
    fn the_block_carries_the_clock_and_the_latencies() {
        let mut inf = info(0, 2, 2);
        // 48 frames at 48 kHz: 1 ms; 96: 2 ms.
        (inf.output_latency_frames, inf.input_latency_frames) = (48, 96);
        let (mut ctx, seen) = ctx_for(&inf, true);
        let mut out = Region::stale(4 * 2);
        // Now is 5 ms; the input is from 0.5 ms before, the output is heard 1.5 ms after.
        let mut out_t = stamp(6_500_000);
        (out_t.mFlags.0, out_t.mSampleTime) = (1, 4096.0);
        run_at(
            &mut ctx,
            &[],
            &[out.buffer(2)],
            (&stamp(5_000_000), &stamp(4_500_000), &out_t),
        );
        let seen = seen.borrow();
        assert_eq!(seen.now_ns, 5_000_000);
        assert_eq!(seen.output_latency_us, 1_500 + 1_000);
        assert_eq!(seen.input_latency_us, 500 + 2_000);
        assert_eq!(seen.sample_time, 4096.0);
    }

    #[test]
    fn a_time_stamp_without_a_sample_time_gives_a_negative_one() {
        let (mut ctx, seen) = ctx_for(&info(0, 0, 2), true);
        let mut out = Region::stale(4 * 2);
        let mut out_t = stamp(0);
        // Host time valid (2), sample time not (1).
        (out_t.mFlags.0, out_t.mSampleTime) = (2, 4096.0);
        run_at(
            &mut ctx,
            &[],
            &[out.buffer(2)],
            (&stamp(0), &stamp(0), &out_t),
        );
        assert_eq!(seen.borrow().sample_time, -1.0);
    }

    // ---- latency and buffer arithmetic

    #[test]
    fn frames_become_nanoseconds_at_the_sample_rate() {
        assert_eq!(frames_to_ns(48, 48_000), 1_000_000);
        assert_eq!(frames_to_ns(441, 44_100), 10_000_000);
        assert_eq!(frames_to_ns(0, 48_000), 0);
        assert_eq!(frames_to_ns(96_000, 96_000), 1_000_000_000);
        // A device that reports no rate does not divide by zero.
        assert_eq!(frames_to_ns(2, 0), 2_000_000_000);
    }

    #[test]
    fn a_latency_is_the_clock_distance_plus_the_devices_in_microseconds() {
        assert_eq!(latency_us(0, 0), 0);
        assert_eq!(latency_us(1_500_000, 500_000), 2_000);
        assert_eq!(latency_us(999, 0), 0);
        assert_eq!(latency_us(u64::MAX / 2, 0), u32::MAX, "held at the largest");
    }

    #[test]
    fn a_devices_extra_latency_is_the_sum_of_its_parts() {
        assert_eq!(extra_latency_frames(0, 0, 0), 0);
        assert_eq!(extra_latency_frames(24, 32, 7), 63);
    }

    #[test]
    fn configuring_the_context_takes_the_layout_and_the_latencies() {
        let mut inf = info(4, 2, 6);
        (
            inf.sample_rate,
            inf.input_latency_frames,
            inf.output_latency_frames,
        ) = (96_000, 96, 48);
        let (ctx, _) = ctx_for(&inf, false);
        assert_eq!((ctx.in_skip, ctx.in_channels, ctx.out_channels), (4, 2, 6));
        assert_eq!((ctx.in_extra_ns, ctx.out_extra_ns), (1_000_000, 500_000));
        assert_eq!(ctx.input.len(), MAX_FRAMES * MAX_IN_CHANNELS);
        assert_eq!(ctx.output.len(), MAX_FRAMES * MAX_OUT_CHANNELS);
    }

    #[test]
    fn the_context_never_takes_more_channels_than_its_buffers_hold() {
        let (ctx, _) = ctx_for(&info(0, MAX_IN_CHANNELS + 1, MAX_OUT_CHANNELS + 1), false);
        assert_eq!(
            (ctx.in_channels, ctx.out_channels),
            (MAX_IN_CHANNELS, MAX_OUT_CHANNELS)
        );
    }

    #[test]
    fn the_buffer_asked_for_is_what_was_wanted_within_what_the_device_takes() {
        assert_eq!(pick_frames(DEFAULT_BUFFER_FRAMES, Some((16.0, 4096.0))), 64);
        assert_eq!(pick_frames(8, Some((16.0, 4096.0))), 16);
        assert_eq!(pick_frames(8192, Some((16.0, 4096.0))), 4096);
        assert_eq!(pick_frames(256, Some((256.0, 256.0))), 256);
        assert_eq!(pick_frames(64, Some((14.5, 4096.0))), 64);
        // A device that gives no range is asked for what was wanted.
        assert_eq!(pick_frames(128, None), 128);
        assert_eq!(pick_frames(0, None), 1);
    }

    #[test]
    fn the_buffer_asked_for_is_never_more_than_a_callback_carries() {
        assert_eq!(pick_frames(100_000, None), MAX_FRAMES as u32);
        assert_eq!(
            pick_frames(100_000, Some((16.0, 65_536.0))),
            MAX_FRAMES as u32
        );
        assert_eq!(
            pick_frames(64, Some((16_384.0, 65_536.0))),
            MAX_FRAMES as u32
        );
    }

    // ---- which channels are the stream's

    #[test]
    fn an_output_only_stream_takes_no_input_channels() {
        assert_eq!(channel_layout(0, 2, None, false), (0, 0, 2));
        // Even when the device has inputs.
        assert_eq!(channel_layout(16, 16, None, false), (0, 0, 16));
    }

    #[test]
    fn one_device_for_both_takes_all_its_channels() {
        assert_eq!(channel_layout(2, 2, None, true), (0, 2, 2));
        assert_eq!(channel_layout(16, 18, None, true), (0, 16, 18));
    }

    #[test]
    fn a_joined_pair_takes_the_inputs_inputs_and_the_outputs_outputs() {
        // Speakers (0 in, 2 out) + microphone (1 in, 0 out).
        assert_eq!(channel_layout(1, 2, Some((0, 2)), true), (0, 1, 2));
        // An interface (16 in, 16 out) as output + another (2 in, 2 out) as input: the
        // aggregate has 18 and 18.
        assert_eq!(channel_layout(18, 18, Some((16, 16)), true), (16, 2, 16));
        // The aggregate is still without the input's streams: no input, not an underflow.
        assert_eq!(channel_layout(16, 16, Some((16, 16)), true), (16, 0, 16));
        assert_eq!(channel_layout(0, 2, Some((16, 2)), true), (16, 0, 2));
    }

    #[test]
    fn channel_counts_are_cut_to_what_the_stream_carries() {
        assert_eq!(
            channel_layout(128, 128, None, true),
            (0, MAX_IN_CHANNELS, MAX_OUT_CHANNELS)
        );
        assert_eq!(
            channel_layout(64 + 64, 64 + 64, Some((64, 64)), true),
            (64, MAX_IN_CHANNELS, MAX_OUT_CHANNELS)
        );
    }

    #[test]
    fn a_joined_pair_is_named_after_both_devices() {
        let speakers = || "Speakers".to_owned();
        assert_eq!(
            device_label(speakers(), Some("Mic"), true),
            "Mic + Speakers"
        );
        assert_eq!(
            device_label(speakers(), Some("Speakers"), false),
            "Speakers"
        );
        assert_eq!(device_label(speakers(), None, false), "Speakers");
        assert_eq!(device_label(speakers(), None, true), "Speakers");
    }

    // ---- uids and the aggregate device

    #[test]
    fn our_own_aggregate_devices_are_told_by_their_uid() {
        assert!(is_own_aggregate(&aggregate_uid(4242, 0)));
        assert!(is_own_aggregate("prowave.duplex.1.2"));
        assert!(!is_own_aggregate("BuiltInSpeakerDevice"));
        assert!(!is_own_aggregate("com.prowave.duplex.1.2"));
        assert!(!is_own_aggregate("prowave.duple"));
        assert!(!is_own_aggregate(""));
    }

    #[test]
    fn every_aggregate_gets_a_uid_of_its_own() {
        assert_eq!(aggregate_uid(4242, 7), "prowave.duplex.4242.7");
        assert_ne!(aggregate_uid(4242, 0), aggregate_uid(4242, 1));
        assert_ne!(aggregate_uid(1, 0), aggregate_uid(2, 0));
    }

    #[test]
    fn the_aggregate_is_private_clocked_by_the_output_with_drift_correction_on_the_input() {
        let spec = aggregate_spec("out-uid", "in-uid", 4242, 3);
        assert_eq!(spec.uid, aggregate_uid(4242, 3));
        assert_eq!(spec.name, "ProWave Duplex");
        assert!(spec.private, "not listed for other programs");
        assert!(!spec.stacked);
        assert_eq!(spec.master, "out-uid", "the output is the clock");
        // The output first: `channel_layout` counts on its channels coming first. The
        // clock is not drift-compensated, the device following it is.
        assert_eq!(
            spec.subs,
            [("out-uid".to_owned(), false), ("in-uid".to_owned(), true)]
        );
    }

    // ---- blocks of bytes from the HAL

    /// Object ids as the HAL writes them: 4 bytes each, in 8-byte-aligned storage.
    fn id_words(ids: &[u32]) -> Vec<u64> {
        ids.chunks(2)
            .map(|pair| {
                let (a, b) = (
                    pair[0].to_ne_bytes(),
                    pair.get(1).copied().unwrap_or(0).to_ne_bytes(),
                );
                u64::from_ne_bytes([a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]])
            })
            .collect()
    }

    #[test]
    fn object_ids_are_read_up_to_the_size_given() {
        assert_eq!(
            object_ids(&id_words(&[41, 42, 43, 44]), 16),
            [41, 42, 43, 44]
        );
        assert_eq!(object_ids(&id_words(&[41, 42, 43]), 12), [41, 42, 43]);
        assert_eq!(object_ids(&id_words(&[41, 42, 43]), 8), [41, 42]);
        // Not a whole id: left out.
        assert_eq!(object_ids(&id_words(&[41, 42]), 7), [41]);
        assert_eq!(object_ids(&id_words(&[41]), 3), [0u32; 0]);
        assert_eq!(object_ids(&[0], 0), [0u32; 0]);
        assert_eq!(object_ids(&[], 0), [0u32; 0]);
    }

    #[test]
    fn object_ids_are_never_read_past_the_block() {
        assert_eq!(object_ids(&id_words(&[41, 42]), 4000), [41, 42]);
    }

    /// A stream configuration as the HAL writes it: an `AudioBufferList` that claims
    /// `claimed` buffers and holds one per entry of `channels`.
    fn config_words(claimed: u32, channels: &[u32]) -> Vec<u64> {
        let c = claimed.to_ne_bytes();
        let mut words = vec![u64::from_ne_bytes([c[0], c[1], c[2], c[3], 0, 0, 0, 0])];
        for n in channels {
            let n = n.to_ne_bytes();
            // mNumberChannels, mDataByteSize (0), then mData (null).
            words.push(u64::from_ne_bytes([n[0], n[1], n[2], n[3], 0, 0, 0, 0]));
            words.push(0);
        }
        words
    }

    #[test]
    fn a_stream_configuration_gives_the_channels_of_all_its_streams() {
        let count = |channels: &[u32]| {
            let words = config_words(channels.len() as u32, channels);
            stream_config_channels(&words, words.len() * 8)
        };
        assert_eq!(count(&[]), 0);
        assert_eq!(count(&[2]), 2);
        assert_eq!(count(&[1, 1]), 2);
        assert_eq!(count(&[2, 1, 3, 64]), 70);
    }

    #[test]
    fn a_stream_configuration_is_never_read_past_its_bytes() {
        // Claims 1000 streams, holds 2.
        let words = config_words(1000, &[2, 4]);
        assert_eq!(stream_config_channels(&words, words.len() * 8), 6);
        // The size cuts the second stream short: only the whole one counts.
        assert_eq!(stream_config_channels(&words, 8 + 16 + 15), 2);
        assert_eq!(stream_config_channels(&words, 8), 0);
        // Too short for the count itself (`get_bytes` gives one zeroed word then).
        assert_eq!(stream_config_channels(&words, 4), 0);
        assert_eq!(stream_config_channels(&[0], 0), 0);
        assert_eq!(stream_config_channels(&[], 0), 0);
        // A size beyond the storage is held to the storage.
        assert_eq!(stream_config_channels(&words, 1 << 20), 6);
    }

    #[test]
    fn the_buffers_of_a_list_are_the_ones_it_says_it_has() {
        let mut words = config_words(3, &[2, 1, 6]);
        let list = NonNull::from(&mut words[..]).cast::<AudioBufferList>();
        // SAFETY: `words` holds a list of 3 buffers, is 8-byte aligned and outlives `bufs`.
        let bufs = unsafe { buffers(list) };
        let channels: Vec<u32> = bufs.iter().map(|b| b.mNumberChannels).collect();
        assert_eq!(channels, [2, 1, 6]);
        assert!(bufs
            .iter()
            .all(|b| b.mData.is_null() && b.mDataByteSize == 0));

        let mut none = config_words(0, &[]);
        let list = NonNull::from(&mut none[..]).cast::<AudioBufferList>();
        // SAFETY: a list of no buffers needs only its count, which `none` holds.
        assert!(unsafe { buffers(list) }.is_empty());
    }

    // ---- the overload listener

    #[test]
    fn every_overload_notice_is_counted() {
        let count = Arc::new(AtomicU32::new(0));
        let client = Arc::as_ptr(&count).cast_mut().cast::<c_void>();
        let addr = address(
            kAudioDeviceProcessorOverload,
            kAudioObjectPropertyScopeGlobal,
        );
        for _ in 0..3 {
            // SAFETY: `client` points to the counter, which outlives the call.
            assert_eq!(
                unsafe { on_overload(7, 1, NonNull::from(&addr), client) },
                0
            );
        }
        assert_eq!(count.load(Ordering::Relaxed), 3);
        // SAFETY: a null client is allowed: nothing is counted.
        assert_eq!(
            unsafe { on_overload(7, 1, NonNull::from(&addr), std::ptr::null_mut()) },
            0
        );
    }

    // ---- switching devices

    fn only(output: &str) -> Devices {
        Devices {
            output: Some(output.to_owned()),
            input: None,
            with_input: false,
        }
    }

    /// A stream with no HAL behind it: devices are names, and some cannot be opened or run.
    /// It panics if `switch_rig` breaks a rule the real stream's safety rests on.
    #[derive(Default)]
    struct FakeRig {
        cannot_open: Vec<&'static str>,
        cannot_run: Vec<&'static str>,
        /// The device adopted last.
        current: Option<String>,
        /// The devices of the last switch that worked (the real stream's `devices`).
        home: String,
        running: bool,
        log: Vec<String>,
    }

    impl FakeRig {
        /// Running on `output`, as after `start`.
        fn on(output: &str) -> Self {
            Self {
                current: Some(output.to_owned()),
                home: output.to_owned(),
                running: true,
                ..Self::default()
            }
        }

        /// As `DuplexStream::switch` does it.
        fn switch(&mut self, to: &str) -> Switched {
            self.log.clear();
            let old = only(&self.home);
            let ended = switch_rig(self, &only(to), &old);
            if ended == Switched::Moved {
                self.home = to.to_owned();
            }
            ended
        }

        fn running_on(&self) -> Option<&str> {
            self.current.as_deref().filter(|_| self.running)
        }
    }

    impl Rig for FakeRig {
        type Opened = String;

        fn halt(&mut self) {
            self.running = false;
            self.log.push("halt".to_owned());
        }

        fn open(&mut self, devices: &Devices) -> Result<String, String> {
            assert!(
                !self.running,
                "a device is opened while the proc still runs"
            );
            let name = devices.output.clone().unwrap_or_default();
            self.log.push(format!("open {name}"));
            if self.cannot_open.contains(&name.as_str()) {
                return Err(format!("no output device {name}"));
            }
            Ok(name)
        }

        fn adopt(&mut self, opened: String) {
            assert!(!self.running, "the context is changed under a running proc");
            self.log.push(format!("adopt {opened}"));
            self.current = Some(opened);
        }

        fn run(&mut self) -> Result<(), String> {
            assert!(!self.running, "a second proc is started");
            let name = self.current.clone().unwrap_or_default();
            self.log.push(format!("run {name}"));
            if self.cannot_run.contains(&name.as_str()) {
                return Err(format!("AudioDeviceStart failed on {name}"));
            }
            self.running = true;
            Ok(())
        }
    }

    #[test]
    fn a_switch_stops_the_old_device_before_it_touches_anything() {
        let mut rig = FakeRig::on("old");
        assert_eq!(rig.switch("new"), Switched::Moved);
        assert_eq!(rig.log, ["halt", "open new", "adopt new", "run new"]);
        assert_eq!(rig.running_on(), Some("new"));
        assert_eq!(rig.home, "new");
    }

    #[test]
    fn a_switch_to_the_same_devices_reopens_them() {
        let mut rig = FakeRig::on("old");
        assert_eq!(rig.switch("old"), Switched::Moved);
        assert_eq!(rig.running_on(), Some("old"));
    }

    #[test]
    fn when_the_new_devices_cannot_be_opened_the_old_ones_run_again() {
        let mut rig = FakeRig::on("old");
        rig.cannot_open = vec!["new"];
        assert_eq!(
            rig.switch("new"),
            Switched::Kept("no output device new".to_owned())
        );
        assert_eq!(rig.running_on(), Some("old"), "the sound goes on");
        assert_eq!(rig.home, "old", "the next switch falls back to these");
        assert_eq!(
            rig.log,
            ["halt", "open new", "open old", "adopt old", "run old"]
        );
    }

    // ---- regression: a switch that fails after the new devices opened, or twice over

    #[test]
    fn when_the_new_devices_open_but_do_not_start_the_old_ones_run_again() {
        let mut rig = FakeRig::on("old");
        rig.cannot_run = vec!["new"];
        assert_eq!(
            rig.switch("new"),
            Switched::Kept("AudioDeviceStart failed on new".to_owned())
        );
        assert_eq!(rig.running_on(), Some("old"), "the sound goes on");
        assert_eq!(rig.home, "old");
        // The new device is let go of (its aggregate removed) before the old one is opened.
        assert_eq!(
            rig.log,
            [
                "halt",
                "open new",
                "adopt new",
                "run new",
                "halt",
                "open old",
                "adopt old",
                "run old"
            ]
        );
    }

    #[test]
    fn when_the_old_devices_do_not_come_back_either_the_error_says_audio_is_stopped() {
        let mut rig = FakeRig::on("old");
        rig.cannot_open = vec!["new", "old"];
        let Switched::Stopped(why) = rig.switch("new") else {
            panic!("not reported as stopped");
        };
        assert!(why.contains("no output device new"), "{why}");
        assert!(why.contains("no output device old"), "{why}");
        assert!(why.contains("audio is stopped"), "{why}");
        assert_eq!(rig.running_on(), None);
        assert_eq!(rig.home, "old");
    }

    #[test]
    fn when_the_old_devices_reopen_but_do_not_start_the_stream_is_reported_stopped() {
        let mut rig = FakeRig::on("old");
        rig.cannot_open = vec!["new"];
        rig.cannot_run = vec!["old"];
        let Switched::Stopped(why) = rig.switch("new") else {
            panic!("not reported as stopped");
        };
        assert!(why.contains("AudioDeviceStart failed on old"), "{why}");
        assert_eq!(rig.running_on(), None);
        assert_eq!(rig.log.last().map(String::as_str), Some("halt"));
    }

    #[test]
    fn a_stopped_stream_comes_back_with_the_next_switch() {
        let mut rig = FakeRig::on("old");
        rig.cannot_open = vec!["new", "old"];
        assert!(matches!(rig.switch("new"), Switched::Stopped(_)));
        // The device is plugged in again.
        rig.cannot_open.clear();
        assert_eq!(rig.switch("new"), Switched::Moved);
        assert_eq!(rig.running_on(), Some("new"));

        // And when only the old one returns, a failed switch still falls back to it.
        let mut rig = FakeRig::on("old");
        rig.cannot_open = vec!["new", "old"];
        rig.cannot_run = vec!["other"];
        assert!(matches!(rig.switch("new"), Switched::Stopped(_)));
        rig.cannot_open = vec!["new"];
        assert!(matches!(rig.switch("other"), Switched::Kept(_)));
        assert_eq!(rig.running_on(), Some("old"));
    }

    // ---- regression: output frames beyond the callback's go out as silence

    #[test]
    fn frames_beyond_the_limit_go_out_as_silence() {
        let frames = MAX_FRAMES + 50;
        let (mut ctx, _) = ctx_for(&info(0, 0, 1), true);
        let mut out = Region::stale(frames);
        run(&mut ctx, &[], &[out.buffer(1)]);
        assert_eq!(
            out.samples()[..MAX_FRAMES],
            interleaved(MAX_FRAMES, 1, 0, tone)
        );
        assert_eq!(
            out.samples()[MAX_FRAMES..],
            [0.0; 50],
            "not what the buffer held before"
        );
        assert!(out.intact());
    }

    #[test]
    fn a_longer_second_output_is_silent_past_the_callback() {
        let (mut ctx, _) = ctx_for(&info(0, 0, 2), true);
        let (mut left, mut right) = (Region::stale(4), Region::stale(8));
        // The first stream sets the callback's length: 4 frames. The second holds 8.
        run(&mut ctx, &[], &[left.buffer(1), right.buffer(1)]);
        assert_eq!(left.samples(), interleaved(4, 1, 0, tone));
        assert_eq!(right.samples()[..4], interleaved(4, 1, 1, tone));
        assert_eq!(right.samples()[4..], [0.0; 4]);
        assert!(left.intact() && right.intact());
    }

    #[test]
    fn a_first_output_of_no_bytes_leaves_the_others_silent() {
        let (mut ctx, seen) = ctx_for(&info(0, 0, 2), true);
        let (mut left, mut right) = (Region::stale(8), Region::stale(8));
        run(
            &mut ctx,
            &[],
            &[left.buffer_claiming(1, 0), right.buffer(1)],
        );
        assert_eq!(seen.borrow().out_len, 0, "a callback of no frames");
        assert_eq!(right.samples(), [0.0; 8]);
        assert_eq!(left.samples(), [STALE; 8], "it claimed no bytes");
        assert!(left.intact() && right.intact());
    }

    #[test]
    fn a_whole_sample_after_the_last_whole_frame_is_silenced_too() {
        let (mut ctx, _) = ctx_for(&info(0, 0, 2), true);
        let mut out = Region::stale(2 * 5 + 1);
        // 5 stereo frames and one sample more.
        run(&mut ctx, &[], &[out.buffer(2)]);
        assert_eq!(out.samples()[..10], interleaved(5, 2, 0, tone));
        assert_eq!(out.samples()[10], 0.0);
        assert!(out.intact());
    }

    // ---- regression: numbers a driver reports must not panic

    #[test]
    fn a_nonsense_buffer_range_does_not_panic() {
        // Least above most.
        assert_eq!(pick_frames(64, Some((512.0, 256.0))), 256);
        // Not numbers: as if the device gave no range on that side.
        assert_eq!(pick_frames(64, Some((f64::NAN, 4096.0))), 64);
        assert_eq!(pick_frames(64, Some((16.0, f64::NAN))), 64);
        assert_eq!(pick_frames(64, Some((f64::NAN, f64::NAN))), 64);
        assert_eq!(
            pick_frames(64, Some((f64::INFINITY, f64::INFINITY))),
            MAX_FRAMES as u32
        );
        // Never a buffer of no frames.
        assert_eq!(pick_frames(64, Some((-8.0, -1.0))), 1);
        assert_eq!(pick_frames(64, Some((0.0, 0.0))), 1);
    }

    #[test]
    fn a_nonsense_latency_does_not_overflow() {
        assert_eq!(extra_latency_frames(u32::MAX, 1, 1), u32::MAX);
        assert_eq!(extra_latency_frames(1, u32::MAX, 0), u32::MAX);
        assert_eq!(extra_latency_frames(0, 1, u32::MAX), u32::MAX);
    }
}

mod hal {
    use super::*;

    #[test]
    fn the_host_clock_goes_forward_in_nanoseconds() {
        let a = host_now_ns();
        std::thread::sleep(Duration::from_millis(2));
        let b = host_now_ns();
        assert!(a > 0);
        let passed = b - a;
        assert!(
            (2_000_000..2_000_000_000).contains(&passed),
            "{passed} ns for a 2 ms sleep"
        );
    }

    #[test]
    fn host_time_and_nanoseconds_convert_both_ways() {
        assert_eq!(host_time_from_ns(0), 0);
        let second = host_time_from_ns(1_000_000_000);
        assert!(second > 0);
        let back = host_ns(second, 0);
        assert!(back.abs_diff(1_000_000_000) < 1_000, "{back}");
        // The distance between two readings, whatever the start.
        let far = host_ns(second * 3, second * 2);
        assert!(far.abs_diff(1_000_000_000) < 1_000, "{far}");
    }

    #[test]
    fn a_time_stamp_from_before_gives_no_distance() {
        assert_eq!(host_ns(5, 10), 0);
        assert_eq!(host_ns(10, 10), 0);
        assert_eq!(host_ns(0, u64::MAX), 0);
    }

    fn text(d: &CFDictionary<CFString, CFType>, key: &str) -> Option<String> {
        let value = d.get(&CFString::from_str(key))?;
        Some(value.downcast_ref::<CFString>()?.to_string())
    }

    fn number(d: &CFDictionary<CFString, CFType>, key: &str) -> Option<i32> {
        let value = d.get(&CFString::from_str(key))?;
        value.downcast_ref::<CFNumber>()?.as_i32()
    }

    fn c_key(key: &std::ffi::CStr) -> &str {
        key.to_str().unwrap_or("not utf-8")
    }

    #[test]
    fn a_sub_device_entry_carries_the_uid_and_the_drift_setting() {
        use objc2_core_audio::{kAudioSubDeviceDriftCompensationKey, kAudioSubDeviceUIDKey};
        let (uid, drift) = (
            c_key(kAudioSubDeviceUIDKey),
            c_key(kAudioSubDeviceDriftCompensationKey),
        );
        let clock = sub_device_entry("out-uid", false);
        assert_eq!(clock.len(), 2);
        assert_eq!(text(&clock, uid).as_deref(), Some("out-uid"));
        assert_eq!(number(&clock, drift), Some(0));
        let follower = sub_device_entry("in-uid", true);
        assert_eq!(text(&follower, uid).as_deref(), Some("in-uid"));
        assert_eq!(number(&follower, drift), Some(1));
    }

    #[test]
    fn the_aggregate_description_uses_the_hals_keys() {
        use objc2_core_audio::{
            kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
            kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
            kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceUIDKey,
        };
        let spec = aggregate_spec("out-uid", "in-uid", 4242, 3);
        let desc = aggregate_description(&spec);
        assert_eq!(desc.len(), 6);
        assert_eq!(
            text(&desc, c_key(kAudioAggregateDeviceUIDKey)).as_deref(),
            Some("prowave.duplex.4242.3")
        );
        assert_eq!(
            text(&desc, c_key(kAudioAggregateDeviceNameKey)).as_deref(),
            Some("ProWave Duplex")
        );
        assert_eq!(
            number(&desc, c_key(kAudioAggregateDeviceIsPrivateKey)),
            Some(1)
        );
        assert_eq!(
            number(&desc, c_key(kAudioAggregateDeviceIsStackedKey)),
            Some(0)
        );
        assert_eq!(
            text(&desc, c_key(kAudioAggregateDeviceMainSubDeviceKey)).as_deref(),
            Some("out-uid")
        );
        let subs = desc.get(&CFString::from_str(c_key(
            kAudioAggregateDeviceSubDeviceListKey,
        )));
        let count = subs
            .as_deref()
            .and_then(|s| s.downcast_ref::<CFArray>())
            .map(|a| a.len());
        assert_eq!(count, Some(2));
    }

    /// An `AudioBufferList` of `bufs`, in storage aligned as the HAL's.
    fn list_of(bufs: &[AudioBuffer]) -> Vec<u64> {
        let mut words = vec![0u64; 1 + 2 * bufs.len()];
        let list = words.as_mut_ptr().cast::<AudioBufferList>();
        // SAFETY: `words` has room for the count and `bufs.len()` buffers of 16 bytes each,
        // 8-byte aligned.
        unsafe {
            std::ptr::addr_of_mut!((*list).mNumberBuffers).write(bufs.len() as u32);
            let first = std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>();
            for (i, b) in bufs.iter().enumerate() {
                first.add(i).write(*b);
            }
        }
        words
    }

    fn list_ptr(words: &mut [u64]) -> NonNull<AudioBufferList> {
        NonNull::from(words).cast()
    }

    #[test]
    fn the_io_proc_runs_a_callback_from_the_hals_own_pointers() {
        let mut heard = Vec::new();
        let mut ctx = Box::new(Ctx::new(|b: Block<'_>| {
            heard.extend_from_slice(b.input);
            b.output.copy_from_slice(b.input);
            assert!(b.output_latency_us >= 1_000 && b.input_latency_us >= 2_000);
        }));
        let mut inf = info(0, 2, 2);
        (inf.output_latency_frames, inf.input_latency_frames) = (48, 96);
        ctx.configure(&inf);
        let fed = interleaved(8, 2, 0, mic);
        let (mut input, mut left, mut right) =
            (Region::holding(&fed), Region::stale(8), Region::stale(8));
        let mut ins = list_of(&[input.buffer(2)]);
        let mut outs = list_of(&[left.buffer(1), right.buffer(1)]);
        let now = host_time_from_ns(1_000_000_000);
        let (now, in_t, out_t) = (stamp(now), stamp(now), stamp(now));
        fn proc_of<F: FnMut(Block<'_>)>(_: &Ctx<F>) -> AudioDeviceIOProc {
            Some(io_proc::<F>)
        }
        let Some(io) = proc_of(&ctx) else {
            panic!("no proc");
        };
        let client = std::ptr::addr_of_mut!(*ctx).cast::<c_void>();
        // SAFETY: what the HAL would pass: lists whose buffers point into live regions, time
        // stamps, and the context of the same closure type; nothing else uses the context.
        let status = unsafe {
            io(
                0,
                NonNull::from(&now),
                list_ptr(&mut ins),
                NonNull::from(&in_t),
                list_ptr(&mut outs),
                NonNull::from(&out_t),
                client,
            )
        };
        assert_eq!(status, 0);
        drop(ctx);
        assert_eq!(heard, fed);
        assert_eq!(left.samples(), interleaved(8, 1, 0, mic));
        assert_eq!(right.samples(), interleaved(8, 1, 1, mic));
        assert!(left.intact() && right.intact() && input.intact());
    }

    #[test]
    fn the_io_proc_does_not_look_at_the_input_list_of_an_output_only_stream() {
        let mut ctx = Box::new(Ctx::new(|b: Block<'_>| {
            assert!(b.input.is_empty());
            b.output.fill(0.25);
        }));
        ctx.configure(&info(0, 0, 2));
        let mut out = Region::stale(8 * 2);
        let mut outs = list_of(&[out.buffer(2)]);
        let t = stamp(0);
        fn proc_of<F: FnMut(Block<'_>)>(_: &Ctx<F>) -> AudioDeviceIOProc {
            Some(io_proc::<F>)
        }
        let Some(io) = proc_of(&ctx) else {
            panic!("no proc");
        };
        let client = std::ptr::addr_of_mut!(*ctx).cast::<c_void>();
        // SAFETY: as above; the input list is a pointer to nothing, which the proc must not
        // read when the stream has no input channels.
        let status = unsafe {
            io(
                0,
                NonNull::from(&t),
                NonNull::dangling(),
                NonNull::from(&t),
                list_ptr(&mut outs),
                NonNull::from(&t),
                client,
            )
        };
        assert_eq!(status, 0);
        assert_eq!(out.samples(), [0.25; 16]);
        assert!(out.intact());
    }

    #[test]
    fn listing_devices_leaves_out_our_own_aggregates() {
        // A read of the HAL's device list: no device is opened, and a Mac with no audio
        // device at all gives an empty list.
        let all = devices();
        assert!(all.iter().all(|d| !is_own_aggregate(&d.uid)));
        assert!(all.iter().filter(|d| d.default_output).count() <= 1);
        assert!(all.iter().filter(|d| d.default_input).count() <= 1);
    }

    #[test]
    fn a_device_that_does_not_exist_is_an_error_before_anything_is_started() {
        let nowhere = Devices {
            output: Some("prowave.test.no-such-device".to_owned()),
            input: None,
            with_input: false,
        };
        let mut made = false;
        let started = DuplexStream::start(None, &nowhere, |_| {
            made = true;
            |_: Block<'_>| {}
        });
        let Err(e) = started else {
            panic!("a stream on a device that does not exist");
        };
        assert_eq!(e, "no output device prowave.test.no-such-device");
        assert!(
            !made,
            "no callback is built for a device that was not opened"
        );
    }
}

/// Real devices. Nothing here renders a sample: the output block is zeroed before the
/// closure and the closures leave it so. On the build machine, name a silent virtual output
/// by its Core Audio uid in `PROWAVE_SHOT_OUTPUT` (as the app's pictures do) and, for the
/// joined-pair test, another device with inputs in `PROWAVE_DUPLEX_TEST_INPUT`.
///
/// Run them rarely and one at a time (`-- --ignored --test-threads=1 duplex::tests::device`).
mod device {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    /// One stream at a time, however the tests are run.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn silent_output() -> Devices {
        Devices {
            output: std::env::var("PROWAVE_SHOT_OUTPUT").ok(),
            input: None,
            with_input: false,
        }
    }

    /// What a silent stream's callbacks saw.
    #[derive(Default)]
    struct Heard {
        calls: AtomicUsize,
        /// Callbacks whose block was not silent on arrival, or not `frames * channels` long.
        bad: AtomicUsize,
    }

    fn start_silent(
        devices: &Devices,
        frames: Option<u32>,
    ) -> Result<(DuplexStream, DuplexInfo, Arc<Heard>), String> {
        let heard = Arc::new(Heard::default());
        let rt = heard.clone();
        let (stream, info) = DuplexStream::start(frames, devices, move |info| {
            let channels = info.out_channels;
            move |b: Block<'_>| {
                rt.calls.fetch_add(1, Ordering::Relaxed);
                let whole = b.out_channels == channels && b.output.len().is_multiple_of(channels);
                if !whole || b.output.iter().any(|s| *s != 0.0) {
                    rt.bad.fetch_add(1, Ordering::Relaxed);
                }
                // Nothing is written: silence.
            }
        })?;
        Ok((stream, info, heard))
    }

    fn wait() {
        std::thread::sleep(Duration::from_millis(300));
    }

    #[test]
    #[ignore = "needs an audio device"]
    fn a_silent_stream_runs_takes_a_new_buffer_size_and_stops_when_dropped() {
        let _one = ONE_AT_A_TIME.lock();
        let (mut stream, info, heard) = match start_silent(&silent_output(), None) {
            Ok(s) => s,
            Err(e) => panic!("{e}"),
        };
        eprintln!("opened {info:?}");
        assert!(info.out_channels > 0 && info.out_channels <= MAX_OUT_CHANNELS);
        assert!(info.sample_rate > 0 && info.buffer_frames > 0);
        assert_eq!(
            (info.in_channels, info.in_skip, info.aggregate),
            (0, 0, false)
        );
        assert!(!info.output_uid.is_empty());
        assert_ne!(stream.device_id(), 0);
        wait();
        let before = heard.calls.load(Ordering::Relaxed);
        assert!(before > 0, "the callback runs");

        let got = stream.set_buffer_frames(256);
        assert!(got > 0);
        assert_eq!(stream.info().buffer_frames, got);
        wait();
        let resized = heard.calls.load(Ordering::Relaxed);
        assert!(resized > before, "the callback goes on at the new size");

        // Dropped while the callback is running.
        drop(stream);
        let stopped = heard.calls.load(Ordering::Relaxed);
        wait();
        assert_eq!(
            heard.calls.load(Ordering::Relaxed),
            stopped,
            "no callback after the drop"
        );
        assert_eq!(heard.bad.load(Ordering::Relaxed), 0);
    }

    #[test]
    #[ignore = "needs an audio device"]
    fn a_switch_goes_on_with_the_same_callback_and_a_failed_one_keeps_the_devices() {
        let _one = ONE_AT_A_TIME.lock();
        let devices = silent_output();
        let (mut stream, info, heard) = match start_silent(&devices, None) {
            Ok(s) => s,
            Err(e) => panic!("{e}"),
        };
        wait();
        assert!(heard.calls.load(Ordering::Relaxed) > 0);

        // To the same devices: a full stop, reopen and start.
        let again = match stream.switch(&devices) {
            Ok(i) => i,
            Err(e) => panic!("{e}"),
        };
        assert_eq!(again.output_uid, info.output_uid);
        assert_eq!(
            again.sample_rate, info.sample_rate,
            "a switch keeps the engine's rate"
        );
        let before = heard.calls.load(Ordering::Relaxed);
        wait();
        assert!(
            heard.calls.load(Ordering::Relaxed) > before,
            "the same closure runs on"
        );

        // To a device that does not exist: an error, and the sound goes on as before.
        let nowhere = Devices {
            output: Some("prowave.test.no-such-device".to_owned()),
            input: None,
            with_input: false,
        };
        assert!(stream.switch(&nowhere).is_err());
        assert_eq!(stream.info().output_uid, info.output_uid);
        let before = heard.calls.load(Ordering::Relaxed);
        wait();
        assert!(
            heard.calls.load(Ordering::Relaxed) > before,
            "back on the devices before"
        );

        drop(stream);
        assert_eq!(heard.bad.load(Ordering::Relaxed), 0);
    }

    // Makes the private aggregate device and reads how it is laid out, without starting it:
    // no input is recorded and nothing is played.
    #[test]
    #[ignore = "needs an audio device"]
    fn two_devices_are_joined_with_the_outputs_channels_first() {
        let _one = ONE_AT_A_TIME.lock();
        let all = devices();
        for d in &all {
            eprintln!(
                "{:?}: {} ({} in, {} out)",
                d.uid, d.name, d.inputs, d.outputs
            );
        }
        let wanted_out = std::env::var("PROWAVE_SHOT_OUTPUT").ok();
        let Some(out) = all.iter().find(|d| match &wanted_out {
            Some(uid) => d.uid == *uid,
            None => d.default_output,
        }) else {
            panic!("no output device {wanted_out:?}");
        };
        let wanted_in = std::env::var("PROWAVE_DUPLEX_TEST_INPUT").ok();
        let Some(input) = all.iter().find(|d| match &wanted_in {
            Some(uid) => d.uid == *uid,
            None => d.inputs > 0 && d.uid != out.uid,
        }) else {
            assert!(wanted_in.is_none(), "no input device {wanted_in:?}");
            eprintln!("no second device with inputs: nothing to join");
            return;
        };
        assert_ne!(input.uid, out.uid, "two different devices are needed");
        let pair = Devices {
            output: Some(out.uid.clone()),
            input: Some(input.uid.clone()),
            with_input: true,
        };
        // SAFETY: HAL calls on devices just listed; the aggregate is destroyed below.
        let opened = match unsafe { Opened::open(&pair, None, None) } {
            Ok(o) => o,
            Err(e) => panic!("{e}"),
        };
        let info = opened.info.clone();
        let listed = devices();
        if let Some(agg) = opened.aggregate {
            // SAFETY: the aggregate this test just made; nothing runs on it.
            unsafe { AudioHardwareDestroyAggregateDevice(agg) };
        }
        eprintln!("joined {info:?}");
        assert!(info.aggregate, "the two devices were joined");
        assert_eq!(
            info.in_skip, out.inputs,
            "the output device's own inputs come first"
        );
        assert_eq!(info.in_channels, input.inputs.min(MAX_IN_CHANNELS));
        assert_eq!(info.out_channels, out.outputs.min(MAX_OUT_CHANNELS));
        assert_eq!(info.device_name, format!("{} + {}", input.name, out.name));
        assert_eq!(info.output_uid, out.uid);
        assert_eq!(info.input_uid.as_deref(), Some(input.uid.as_str()));
        assert_eq!(
            listed, all,
            "the private aggregate is not in the device list"
        );
    }
}
