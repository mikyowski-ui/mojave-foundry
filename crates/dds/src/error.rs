use thiserror::Error;

/// Everything that can go wrong while reading a DDS file.
///
/// Offsets are byte offsets from the start of the file.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DdsError {
    #[error("truncated at offset {offset}: {what} needs {needed} bytes, {available} remain")]
    Truncated {
        offset: usize,
        what: &'static str,
        needed: u64,
        available: u64,
    },

    #[error("not a DDS file: magic {found:02x?} at offset 0")]
    BadMagic { found: [u8; 4] },

    #[error("bad {what} at offset {offset}: {value}")]
    BadHeader {
        offset: usize,
        what: &'static str,
        value: u32,
    },

    #[error(
        "bad dimensions {width} x {height} at offset 12: each side must be 1 to {limit} pixels"
    )]
    BadDimensions { width: u32, height: u32, limit: u32 },

    #[error("unsupported at offset {offset}: {what}")]
    Unsupported { offset: usize, what: String },
}
