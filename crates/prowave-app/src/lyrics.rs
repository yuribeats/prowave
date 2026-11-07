//! Lyrics on notes: each note can carry a word or syllable, kept in the note and saved with it.

/// Most bytes of a lyric (UTF-8); longer text is cut at a character boundary.
pub const MAX_LYRIC: usize = 31;

/// A note's lyric, kept in the note itself (no allocation, so notes stay `Copy`).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Lyric {
    len: u8,
    bytes: [u8; MAX_LYRIC],
}

impl Lyric {
    /// No lyric.
    pub const NONE: Lyric = Lyric {
        len: 0,
        bytes: [0; MAX_LYRIC],
    };

    /// `text`, trimmed, cut to [`MAX_LYRIC`] bytes at a character boundary.
    pub fn new(text: &str) -> Self {
        let text = text.trim();
        let mut end = text.len().min(MAX_LYRIC);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut bytes = [0u8; MAX_LYRIC];
        bytes[..end].copy_from_slice(&text.as_bytes()[..end]);
        Self {
            len: end as u8,
            bytes,
        }
    }

    pub fn as_str(&self) -> &str {
        // Built from a `&str` cut at a character boundary.
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl std::fmt::Debug for Lyric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lyric_is_kept_in_the_note_and_cut_at_a_character_boundary() {
        assert_eq!(Lyric::new("  row ").as_str(), "row");
        assert!(Lyric::NONE.is_empty());
        let long = "é".repeat(40);
        let l = Lyric::new(&long);
        assert!(l.as_str().len() <= MAX_LYRIC && l.as_str().chars().all(|c| c == 'é'));
    }
}
