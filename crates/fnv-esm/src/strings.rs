//! Text decoding.
//!
//! Strings in these plugins are single-byte, NUL-terminated, Windows-1252.
//! Decoding here is total and reversible:
//!
//! - bytes `0x00..=0x7F` and `0xA0..=0xFF` map to the code point of the same
//!   value;
//! - bytes `0x80..=0x9F` map to their Windows-1252 characters;
//! - the five bytes Windows-1252 leaves undefined (`0x81`, `0x8D`, `0x8F`,
//!   `0x90`, `0x9D`) map to the C1 control code point of the same value.
//!
//! So every byte string has exactly one `String` form and
//! [`encode_cp1252`] gives the bytes back.

/// Windows-1252 characters for bytes `0x80..=0x9F`.
const HIGH: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

fn decode_byte(byte: u8) -> char {
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

fn encode_char(c: char) -> Option<u8> {
    let code = u32::from(c);
    match code {
        0x00..=0x7F | 0xA0..=0xFF => u8::try_from(code).ok(),
        _ => HIGH
            .iter()
            .position(|&h| h == c)
            .and_then(|i| u8::try_from(0x80 + i).ok()),
    }
}

/// Decodes every byte of `bytes` as Windows-1252, including any NUL.
pub fn decode_cp1252(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| decode_byte(b)).collect()
}

/// Decodes a NUL-terminated string: the bytes before the first NUL, or all
/// of them when there is none.
pub fn decode_zstring(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    decode_cp1252(&bytes[..end])
}

/// Encodes text as Windows-1252, or `None` when a character has no
/// single-byte form.
pub fn encode_cp1252(text: &str) -> Option<Vec<u8>> {
    text.chars().map(encode_char).collect()
}

/// Encodes text as Windows-1252, writing `?` for a character that has no
/// single-byte form.
pub fn encode_cp1252_lossy(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| encode_char(c).unwrap_or(b'?'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_round_trips() {
        let all: Vec<u8> = (0u8..=255).collect();
        let text = decode_cp1252(&all);
        assert_eq!(text.chars().count(), 256);
        assert_eq!(encode_cp1252(&text).as_deref(), Some(all.as_slice()));
    }

    #[test]
    fn ascii_is_unchanged_and_high_bytes_are_cp1252() {
        assert_eq!(decode_zstring(b"Plain text\0ignored"), "Plain text");
        assert_eq!(decode_zstring(b"no terminator"), "no terminator");
        assert_eq!(
            decode_cp1252(&[0x93, 0x41, 0x94, 0x85, 0xE9]),
            "\u{201C}A\u{201D}\u{2026}\u{E9}"
        );
    }

    #[test]
    fn unencodable_characters_are_reported() {
        assert_eq!(encode_cp1252("\u{4E2D}"), None);
        assert_eq!(encode_cp1252_lossy("a\u{4E2D}b"), b"a?b");
    }
}
