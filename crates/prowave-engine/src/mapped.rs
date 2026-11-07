//! Memory-mapped PCM sources. A WAV or AIFF file that holds plain PCM (16, 24 or 32-bit integer
//! or 32 or 64-bit float samples) is mapped into memory and read straight from the map: the disk
//! thread copies and converts the samples it needs with no decoder, no read calls and no copy
//! through a buffer, and a seek is a change of one number. Anything else (compressed audio,
//! unusual layouts, files too damaged to trust) is left to the decoder in `disk.rs`, which gives
//! the same samples.
//!
//! A mapped file must not shrink while it is in use: reading a page that is no longer there
//! kills the process on Unix. Windows refuses to shorten a mapped file, so only Unix is at risk;
//! there [`Mapped::still_whole`] is asked every few hundred milliseconds and the disk thread
//! goes on with the decoder if the file got shorter. Setting `PROWAVE_NO_MMAP=1` turns mapping
//! off.
use memmap2::Mmap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Frames read from the map by one [`Mapped::next_stereo`] call.
const CHUNK_FRAMES: u64 = 4096;

/// How the samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    I16Le,
    I24Le,
    I32Le,
    F32Le,
    F64Le,
    I16Be,
    I24Be,
    I32Be,
    F32Be,
    F64Be,
}

impl Format {
    fn bytes(self) -> usize {
        match self {
            Self::I16Le | Self::I16Be => 2,
            Self::I24Le | Self::I24Be => 3,
            Self::I32Le | Self::I32Be | Self::F32Le | Self::F32Be => 4,
            Self::F64Le | Self::F64Be => 8,
        }
    }

    /// One sample as a float in -1..1 (as the decoder converts it).
    fn sample(self, b: &[u8]) -> f32 {
        match self {
            Self::I16Le => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
            Self::I16Be => f32::from(i16::from_be_bytes([b[0], b[1]])) / 32768.0,
            Self::I24Le => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            Self::I24Be => (i32::from_be_bytes([b[0], b[1], b[2], 0]) >> 8) as f32 / 8_388_608.0,
            Self::I32Le => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            Self::I32Be => i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            Self::F32Le => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Self::F32Be => f32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            Self::F64Le => {
                f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
            }
            Self::F64Be => {
                f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
            }
        }
    }
}

/// What the header of a file says about its samples.
struct Layout {
    format: Format,
    channels: usize,
    sample_rate: u32,
    /// Where the samples start and how many frames there are.
    data_start: usize,
    frames: u64,
}

/// A PCM file read through a memory map.
pub(crate) struct Mapped {
    map: Mmap,
    /// Keeps the file open for the length checks.
    file: File,
    path: PathBuf,
    /// The length of the file when it was mapped.
    len: u64,
    format: Format,
    data_start: usize,
    pub(crate) sample_rate: u32,
    pub(crate) channels: usize,
    pub(crate) frames: u64,
    /// The next frame [`Mapped::next_stereo`] returns.
    pos: u64,
    /// When the file's length was last looked at.
    checked: Instant,
    /// The file got shorter: nothing is read from the map any more.
    broken: bool,
}

impl Mapped {
    /// Maps `path` if it is a PCM file this module reads.
    pub(crate) fn open(path: &Path) -> Option<Self> {
        if std::env::var_os("PROWAVE_NO_MMAP").is_some_and(|v| v == "1") {
            return None;
        }
        let file = File::open(path).ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() < 44 {
            return None;
        }
        // SAFETY: the map is only read. A file that is shortened by someone else while mapped
        // would fault on a read past its new end; `still_whole` is checked often enough to keep
        // that window short, and the module documentation says what is left. Windows does not
        // allow a mapped file to be shortened at all.
        let map = unsafe { Mmap::map(&file) }.ok()?;
        let layout = parse(&map)?;
        Some(Self {
            len: meta.len(),
            map,
            file,
            path: path.to_path_buf(),
            format: layout.format,
            data_start: layout.data_start,
            sample_rate: layout.sample_rate,
            channels: layout.channels,
            frames: layout.frames,
            pos: 0,
            checked: Instant::now(),
            broken: false,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn pos(&self) -> u64 {
        self.pos
    }

    /// Moves to `frame` (the end at most); returns where it is.
    pub(crate) fn seek(&mut self, frame: u64) -> u64 {
        self.pos = frame.min(self.frames);
        self.pos
    }

    /// Appends the next frames as stereo (a mono file on both sides, more than two channels
    /// cut to the first two). `false` at the end, or when the file got shorter.
    pub(crate) fn next_stereo(&mut self, out: &mut Vec<f32>) -> bool {
        if self.broken || self.pos >= self.frames {
            return false;
        }
        let n = CHUNK_FRAMES.min(self.frames - self.pos);
        let bytes = self.format.bytes();
        let align = bytes * self.channels;
        let start = self.data_start + self.pos as usize * align;
        let Some(data) = self.map.get(start..start + n as usize * align) else {
            self.broken = true;
            return false;
        };
        out.reserve(n as usize * 2);
        for frame in data.chunks_exact(align) {
            let l = finite(self.format.sample(&frame[..bytes]));
            let r = if self.channels > 1 {
                finite(self.format.sample(&frame[bytes..bytes * 2]))
            } else {
                l
            };
            out.push(l);
            out.push(r);
        }
        self.pos += n;
        true
    }

    /// Whether the file is at least as long as when it was mapped. Looks at the file system
    /// only every 250 ms; the answer in between is the last one.
    pub(crate) fn still_whole(&mut self) -> bool {
        if self.broken {
            return false;
        }
        if self.checked.elapsed() < std::time::Duration::from_millis(250) {
            return true;
        }
        self.checked = Instant::now();
        let whole = self.file.metadata().is_ok_and(|m| m.len() >= self.len);
        if !whole {
            self.broken = true;
        }
        whole
    }
}

fn finite(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

fn u16_at(b: &[u8], at: usize, big: bool) -> Option<u16> {
    let v: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
    Some(if big {
        u16::from_be_bytes(v)
    } else {
        u16::from_le_bytes(v)
    })
}

fn u32_at(b: &[u8], at: usize, big: bool) -> Option<u32> {
    let v: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    Some(if big {
        u32::from_be_bytes(v)
    } else {
        u32::from_le_bytes(v)
    })
}

/// The layout of a WAV or AIFF file, if it holds plain PCM that is stored just as it says.
fn parse(b: &[u8]) -> Option<Layout> {
    match (b.get(0..4)?, b.get(8..12)?) {
        (b"RIFF", b"WAVE") => parse_wav(b),
        (b"FORM", b"AIFF" | b"AIFC") => parse_aiff(b),
        _ => None,
    }
}

fn parse_wav(b: &[u8]) -> Option<Layout> {
    let mut fmt: Option<(Format, usize, u32)> = None;
    let mut pos = 12usize;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32_at(b, pos + 4, false)? as usize;
        let body = pos + 8;
        match id {
            b"fmt " => {
                if size < 16 {
                    return None;
                }
                let mut tag = u16_at(b, body, false)?;
                let channels = u16_at(b, body + 2, false)? as usize;
                let rate = u32_at(b, body + 4, false)?;
                let align = u16_at(b, body + 12, false)? as usize;
                let bits = u16_at(b, body + 14, false)?;
                if tag == 0xfffe {
                    // Extensible: the real format tag opens the sub-format GUID.
                    if size < 40 {
                        return None;
                    }
                    tag = u16_at(b, body + 24, false)?;
                }
                let format = match (tag, bits) {
                    (1, 16) => Format::I16Le,
                    (1, 24) => Format::I24Le,
                    (1, 32) => Format::I32Le,
                    (3, 32) => Format::F32Le,
                    (3, 64) => Format::F64Le,
                    _ => return None,
                };
                // Samples padded into wider containers are not read here.
                if channels == 0 || align != channels * format.bytes() {
                    return None;
                }
                fmt = Some((format, channels, rate));
            }
            b"data" => {
                let (format, channels, sample_rate) = fmt?;
                let avail = b.len() - body;
                // A length of zero or past the end of the file is a writer that did not finish;
                // the decoder knows what to make of those.
                if size == 0 || size > avail {
                    return None;
                }
                return Some(Layout {
                    format,
                    channels,
                    sample_rate,
                    data_start: body,
                    frames: (size / (channels * format.bytes())) as u64,
                });
            }
            _ => {}
        }
        pos = body.checked_add(size)?.checked_add(size & 1)?;
    }
    None
}

fn parse_aiff(b: &[u8]) -> Option<Layout> {
    let compressed = &b[8..12] == b"AIFC";
    let mut comm: Option<(Format, usize, u32, u64)> = None;
    let mut pos = 12usize;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32_at(b, pos + 4, true)? as usize;
        let body = pos + 8;
        match id {
            b"COMM" => {
                let channels = u16_at(b, body, true)? as usize;
                let frames = u32_at(b, body + 2, true)? as u64;
                let bits = u16_at(b, body + 6, true)?;
                let rate = extended(b.get(body + 8..body + 18)?)?;
                let kind: &[u8] = if compressed {
                    b.get(body + 18..body + 22)?
                } else {
                    b"NONE"
                };
                let format = match (kind, bits) {
                    (b"NONE" | b"twos", 16) => Format::I16Be,
                    (b"NONE" | b"twos", 24) => Format::I24Be,
                    (b"NONE" | b"twos", 32) => Format::I32Be,
                    (b"sowt", 16) => Format::I16Le,
                    (b"sowt", 24) => Format::I24Le,
                    (b"sowt", 32) => Format::I32Le,
                    (b"fl32" | b"FL32", 32) => Format::F32Be,
                    (b"fl64" | b"FL64", 64) => Format::F64Be,
                    _ => return None,
                };
                if channels == 0 {
                    return None;
                }
                comm = Some((format, channels, rate, frames));
            }
            b"SSND" => {
                let (format, channels, sample_rate, frames) = comm?;
                let offset = u32_at(b, body, true)? as usize;
                let data_start = body.checked_add(8)?.checked_add(offset)?;
                let end = body.checked_add(size)?;
                // The chunk must hold every frame the header counts, and the file the chunk.
                let need = (frames as usize).checked_mul(channels * format.bytes())?;
                if end > b.len() || data_start.checked_add(need)? > end || frames == 0 {
                    return None;
                }
                return Some(Layout {
                    format,
                    channels,
                    sample_rate,
                    data_start,
                    frames,
                });
            }
            _ => {}
        }
        pos = body.checked_add(size)?.checked_add(size & 1)?;
    }
    None
}

/// An AIFF sample rate: an 80-bit extended float, as a whole number of hertz.
fn extended(b: &[u8]) -> Option<u32> {
    let exponent = (u16::from_be_bytes([b[0], b[1]]) & 0x7fff) as i32;
    let mantissa = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if b[0] & 0x80 != 0 || mantissa == 0 {
        return None;
    }
    let shift = exponent - 16383 - 63;
    let rate = if shift >= 0 {
        mantissa.checked_shl(shift as u32)?
    } else {
        mantissa >> (-shift).min(63)
    };
    u32::try_from(rate).ok().filter(|r| *r > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aiff_sample_rates_are_read_from_extended_floats() {
        // 44100 = 0x400E AC44 0000 0000 0000
        let rate = [0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0];
        assert_eq!(extended(&rate), Some(44_100));
        let rate = [0x40, 0x0e, 0xbb, 0x80, 0, 0, 0, 0, 0, 0];
        assert_eq!(extended(&rate), Some(48_000));
        assert_eq!(extended(&[0; 10]), None);
        assert_eq!(extended(&[0xc0, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn samples_convert_like_the_decoder() {
        assert_eq!(Format::I16Le.sample(&[0x00, 0x80]), -1.0);
        assert_eq!(Format::I16Le.sample(&[0xff, 0x7f]), 32767.0 / 32768.0);
        assert_eq!(Format::I16Be.sample(&[0x40, 0x00]), 0.5);
        assert_eq!(Format::I24Le.sample(&[0x00, 0x00, 0x80]), -1.0);
        assert_eq!(Format::I24Be.sample(&[0x40, 0x00, 0x00]), 0.5);
        assert_eq!(Format::I32Le.sample(&[0, 0, 0, 0x80]), -1.0);
        assert_eq!(Format::F32Le.sample(&0.25f32.to_le_bytes()), 0.25);
        assert_eq!(Format::F64Be.sample(&(-0.5f64).to_be_bytes()), -0.5);
    }
}
