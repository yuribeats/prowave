//! MIDI Time Code (Logic Pro: Project Settings > Synchronization > MTC): the SMPTE time sent
//! over MIDI as quarter-frame messages, four to a frame, and as a full-frame SysEx message
//! when the position jumps. Pro Wave sends it ([`MtcGenerator`], run by the engine so the
//! messages leave on the audio clock) and follows it ([`MtcDecoder`], run by the app).
//!
//! A time code takes eight quarter frames (two frames) to send, one nibble each: frame, seconds,
//! minutes and hours low and high, the last one also telling the frame rate. The time sent is
//! the frame of the first piece, so a receiver that has the eighth adds two frames to know
//! where the sender is now.
use crate::tempo_map::{Smpte, SmpteRate};

/// The status byte of a quarter-frame message.
pub const QUARTER_FRAME: u8 = 0xf1;

/// One raw MIDI message ready to send: a quarter frame (2 bytes) or a full frame (10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MtcMessage {
    len: u8,
    bytes: [u8; 10],
}

impl MtcMessage {
    fn quarter(data: u8) -> Self {
        let mut bytes = [0; 10];
        bytes[0] = QUARTER_FRAME;
        bytes[1] = data;
        Self { len: 2, bytes }
    }

    fn full(bytes: [u8; 10]) -> Self {
        Self { len: 10, bytes }
    }

    /// The message as MIDI bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

/// The two-bit code of a frame rate in the hours byte. MTC has no separate 29.97 non-drop
/// rate: it is sent as 30, which counts the same labels.
pub fn rate_code(rate: SmpteRate) -> u8 {
    match rate {
        SmpteRate::Fps24 => 0,
        SmpteRate::Fps25 => 1,
        SmpteRate::Fps29_97Drop => 2,
        SmpteRate::Fps29_97NonDrop | SmpteRate::Fps30 => 3,
    }
}

/// The rate a code stands for.
pub fn rate_of_code(code: u8) -> SmpteRate {
    match code & 3 {
        0 => SmpteRate::Fps24,
        1 => SmpteRate::Fps25,
        2 => SmpteRate::Fps29_97Drop,
        _ => SmpteRate::Fps30,
    }
}

/// The full-frame message for `t`: `F0 7F 7F 01 01 hh mm ss ff F7`. Receivers locate to it.
pub fn full_frame(t: Smpte, rate: SmpteRate) -> MtcMessage {
    MtcMessage::full([
        0xf0,
        0x7f,
        0x7f,
        0x01,
        0x01,
        (rate_code(rate) << 5) | (t.h & 0x1f),
        t.m & 0x3f,
        t.s & 0x3f,
        t.f & 0x1f,
        0xf7,
    ])
}

/// The data byte of quarter frame `piece` (0-7) of the time code `t`: the piece number in the
/// high nibble and a nibble of the time in the low one.
pub fn quarter_frame_data(piece: u8, t: Smpte, rate: SmpteRate) -> u8 {
    let nibble = match piece & 7 {
        0 => t.f & 0xf,
        1 => t.f >> 4,
        2 => t.s & 0xf,
        3 => t.s >> 4,
        4 => t.m & 0xf,
        5 => t.m >> 4,
        6 => t.h & 0xf,
        _ => ((t.h >> 4) & 1) | (rate_code(rate) << 1),
    };
    ((piece & 7) << 4) | (nibble & 0xf)
}

/// Sends the time code of a running transport: a quarter frame every quarter of a frame, and a
/// full frame when the position jumps.
#[derive(Clone, Copy, Debug)]
pub struct MtcGenerator {
    on: bool,
    rate: SmpteRate,
    /// Number of the next quarter frame to send (four to a frame, counted from 00:00:00:00);
    /// always a multiple of 8 when a time code starts, so every one begins with piece 0.
    next: u64,
}

/// The most quarter frames one [`MtcGenerator::tick`] sends. A position that ran further than
/// this since the last tick (a stall) is located again instead of being replayed.
const MAX_BURST: u64 = 8;
/// The most frames a position can be (about 1,000 years at 30 fps).
const MAX_FRAMES: f64 = 1.0e12;

impl Default for MtcGenerator {
    fn default() -> Self {
        Self {
            on: false,
            rate: SmpteRate::Fps25,
            next: 0,
        }
    }
}

impl MtcGenerator {
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Turns the output on or off and sets the frame rate. Call [`MtcGenerator::locate`]
    /// afterwards so receivers learn where the transport is.
    pub fn configure(&mut self, on: bool, rate: SmpteRate) {
        self.on = on;
        self.rate = rate;
    }

    /// Tells receivers the transport is at `frames` (real frames from 00:00:00:00, with a
    /// fraction): a full-frame message now, and quarter frames from the next time code that
    /// starts at or after it.
    pub fn locate(&mut self, frames: f64, mut send: impl FnMut(MtcMessage)) {
        if !self.on || !frames.is_finite() {
            return;
        }
        // A day is far less than this; it only keeps wild values from overflowing.
        let frames = frames.clamp(0.0, MAX_FRAMES);
        send(full_frame(
            self.rate.smpte_of_frames(frames.floor()),
            self.rate,
        ));
        let quarters = (frames * 4.0).ceil() as u64;
        self.next = quarters.div_ceil(8) * 8;
    }

    /// The transport is at `frames`: sends the quarter frames that are due.
    pub fn tick(&mut self, frames: f64, mut send: impl FnMut(MtcMessage)) {
        if !self.on || !frames.is_finite() {
            return;
        }
        let due = (frames.clamp(0.0, MAX_FRAMES) * 4.0).floor() as u64;
        if due >= self.next + MAX_BURST {
            self.locate(frames, send);
            return;
        }
        while self.next <= due {
            let piece = (self.next % 8) as u8;
            let first_frame = (self.next - piece as u64) / 4;
            let t = self.rate.smpte_of_frames(first_frame as f64);
            send(MtcMessage::quarter(quarter_frame_data(piece, t, self.rate)));
            self.next += 1;
        }
    }
}

/// What a receiver learns from the messages it is fed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MtcEvent {
    /// A full-frame message: the sender is at this frame (stopped, or just jumped).
    Locate { frames: i64, rate: SmpteRate },
    /// Eight quarter frames in a row: the sender is running and has reached this frame.
    Running { frames: i64, rate: SmpteRate },
}

/// Reads MIDI Time Code from raw MIDI messages.
#[derive(Clone, Copy, Debug, Default)]
pub struct MtcDecoder {
    /// The piece the next quarter frame must be to continue the time code.
    expected: u8,
    nibbles: [u8; 8],
}

impl MtcDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one MIDI message; `Some` when it completes a time code or is a full frame.
    pub fn feed(&mut self, msg: &[u8]) -> Option<MtcEvent> {
        match *msg {
            [QUARTER_FRAME, data] => self.quarter(data),
            [0xf0, 0x7f, _, 0x01, 0x01, hh, mm, ss, ff, 0xf7] => {
                self.expected = 0;
                let rate = rate_of_code(hh >> 5);
                let t = checked(hh & 0x1f, mm, ss, ff, rate)?;
                Some(MtcEvent::Locate {
                    frames: rate.frames_of(t),
                    rate,
                })
            }
            _ => None,
        }
    }

    fn quarter(&mut self, data: u8) -> Option<MtcEvent> {
        if data & 0x80 != 0 {
            return None;
        }
        let (piece, nibble) = (data >> 4, data & 0xf);
        if piece != self.expected {
            // Out of step (a lost message, or a reversed or restarted sequence): wait for the
            // next piece 0.
            self.expected = 0;
            if piece != 0 {
                return None;
            }
        }
        self.nibbles[piece as usize] = nibble;
        if piece < 7 {
            self.expected = piece + 1;
            return None;
        }
        self.expected = 0;
        let n = &self.nibbles;
        let rate = rate_of_code(n[7] >> 1);
        let t = checked(
            n[6] | ((n[7] & 1) << 4),
            n[4] | (n[5] << 4),
            n[2] | (n[3] << 4),
            n[0] | (n[1] << 4),
            rate,
        )?;
        // The time sent is that of piece 0, two frames ago.
        Some(MtcEvent::Running {
            frames: rate.frames_of(t) + 2,
            rate,
        })
    }
}

/// The time code with these fields, if there is one at `rate`.
fn checked(h: u8, m: u8, s: u8, f: u8, rate: SmpteRate) -> Option<Smpte> {
    (h < 24 && m < 60 && s < 60 && (f as u64) < rate.nominal_fps()).then_some(Smpte {
        h,
        m,
        s,
        f,
        subframe: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u8, m: u8, s: u8, f: u8) -> Smpte {
        Smpte {
            h,
            m,
            s,
            f,
            subframe: 0,
        }
    }

    #[test]
    fn a_full_frame_is_the_standard_sysex() {
        let m = full_frame(t(1, 2, 3, 4), SmpteRate::Fps25);
        assert_eq!(
            m.bytes(),
            [0xf0, 0x7f, 0x7f, 0x01, 0x01, 0b0010_0001, 2, 3, 4, 0xf7]
        );
        let m = full_frame(t(23, 59, 59, 29), SmpteRate::Fps29_97Drop);
        assert_eq!(m.bytes()[5], (2 << 5) | 23);
    }

    #[test]
    fn the_eight_quarter_frames_carry_the_nibbles_and_the_rate() {
        let time = t(17, 42, 33, 21);
        let pieces: Vec<u8> = (0..8)
            .map(|p| quarter_frame_data(p, time, SmpteRate::Fps30))
            .collect();
        // Frame 21 = 0x15, seconds 33 = 0x21, minutes 42 = 0x2a, hours 17 = 0x11 | rate 3 << 1.
        assert_eq!(
            pieces,
            [0x05, 0x11, 0x21, 0x32, 0x4a, 0x52, 0x61, 0x77],
            "{pieces:x?}"
        );
    }

    #[test]
    fn what_is_sent_is_what_is_read_for_every_rate() {
        for rate in SmpteRate::ALL {
            for start in [0u64, 7, 1799, 89_999, 107_892, 2_000_000] {
                let mut dec = MtcDecoder::new();
                let mut seen = Vec::new();
                for piece in 0..8 {
                    let label = rate.smpte_of_frames(start as f64);
                    let msg = [QUARTER_FRAME, quarter_frame_data(piece, label, rate)];
                    if let Some(e) = dec.feed(&msg) {
                        seen.push(e);
                    }
                }
                let want_rate = rate_of_code(rate_code(rate));
                assert_eq!(
                    seen,
                    [MtcEvent::Running {
                        frames: start as i64 + 2,
                        rate: want_rate
                    }],
                    "{rate:?} from {start}"
                );
                // And the full frame says where the sender stands.
                let full = full_frame(rate.smpte_of_frames(start as f64), rate);
                assert_eq!(
                    dec.feed(full.bytes()),
                    Some(MtcEvent::Locate {
                        frames: start as i64,
                        rate: want_rate
                    })
                );
            }
        }
    }

    #[test]
    fn the_generator_sends_a_steady_stream_a_decoder_can_follow() {
        for rate in SmpteRate::ALL {
            let fps = rate.frames_per_second();
            let mut gen = MtcGenerator::default();
            gen.configure(true, rate);
            let mut dec = MtcDecoder::new();
            let mut sent = Vec::new();
            // Start one hour in, at a half frame, and play 12 s in 5 ms audio blocks.
            let start = 3600.0 * fps + 0.5;
            gen.locate(start, |m| sent.push(m));
            assert_eq!(sent.len(), 1, "a full frame first");
            let mut quarters = 0;
            let mut last_running = None;
            for block in 0..2400 {
                let now = start + (block as f64 * 0.005) * fps;
                let mut out = Vec::new();
                gen.tick(now, |m| out.push(m));
                for m in out {
                    assert_eq!(m.bytes().len(), 2);
                    quarters += 1;
                    if let Some(MtcEvent::Running { frames, .. }) = dec.feed(m.bytes()) {
                        // The decoder is within a frame of the transport.
                        assert!(
                            (frames as f64 - now).abs() <= 2.0,
                            "{rate:?}: {frames} vs {now}"
                        );
                        last_running = Some(frames);
                    }
                }
            }
            // 12 seconds at four per frame, give or take the wait for a group to start.
            let want = (12.0 * fps * 4.0) as i32;
            assert!(
                (quarters - want).abs() <= 10,
                "{rate:?}: {quarters} vs {want}"
            );
            assert!(last_running.is_some());
        }
    }

    #[test]
    fn a_stall_or_a_jump_locates_again_and_a_lost_message_is_ignored() {
        let mut gen = MtcGenerator::default();
        gen.configure(true, SmpteRate::Fps25);
        let mut out = Vec::new();
        gen.locate(100.0, |m| out.push(m));
        gen.tick(100.0, |m| out.push(m));
        out.clear();
        // A second later the generator does not replay a hundred quarter frames: it locates.
        gen.tick(125.0, |m| out.push(m));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bytes().len(), 10);
        // A decoder that misses a piece drops that time code and carries on with the next.
        let mut dec = MtcDecoder::new();
        let time = t(0, 0, 1, 0);
        let mut got = 0;
        for round in 0..2 {
            for p in 0..8u8 {
                if round == 0 && p == 3 {
                    continue;
                }
                let m = [QUARTER_FRAME, quarter_frame_data(p, time, SmpteRate::Fps25)];
                if dec.feed(&m).is_some() {
                    got += 1;
                }
            }
        }
        assert_eq!(got, 1);
        // Off, it sends nothing.
        let mut off = MtcGenerator::default();
        off.locate(10.0, |_| panic!("sent while off"));
        off.tick(10.0, |_| panic!("sent while off"));
    }

    #[test]
    fn bad_messages_are_ignored() {
        let mut dec = MtcDecoder::new();
        assert_eq!(dec.feed(&[]), None);
        assert_eq!(dec.feed(&[QUARTER_FRAME]), None);
        assert_eq!(dec.feed(&[QUARTER_FRAME, 0x90]), None, "not a data byte");
        assert_eq!(dec.feed(&[0x90, 60, 100]), None);
        // Seconds 60 does not exist.
        let bad = [0xf0, 0x7f, 0x7f, 0x01, 0x01, 0x21, 0, 60, 0, 0xf7];
        assert_eq!(dec.feed(&bad), None);
        // A frame past the rate's count: 25 at 25 fps.
        let bad = [0xf0, 0x7f, 0x7f, 0x01, 0x01, 0x21, 0, 0, 25, 0xf7];
        assert_eq!(dec.feed(&bad), None);
        // Other SysEx is not MTC.
        assert_eq!(dec.feed(&[0xf0, 0x7e, 0x7f, 0x09, 0x01, 0xf7]), None);
    }

    #[test]
    fn wild_positions_never_overflow() {
        let mut gen = MtcGenerator::default();
        gen.configure(true, SmpteRate::Fps29_97Drop);
        let mut sent = 0;
        for frames in [
            f64::MAX,
            1.0e300,
            1.0e13,
            1.0e12,
            -1.0e300,
            f64::MIN_POSITIVE,
        ] {
            gen.locate(frames, |_| sent += 1);
            gen.tick(frames, |_| sent += 1);
            gen.tick(0.0, |_| sent += 1);
        }
        assert!(sent > 0);
        gen.locate(f64::NAN, |_| panic!("not a position"));
        gen.tick(f64::INFINITY, |_| panic!("not a position"));
    }
}
