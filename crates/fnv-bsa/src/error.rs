use std::io;

use thiserror::Error;

/// Everything that can go wrong while opening an archive or reading an entry.
///
/// Offsets are byte offsets from the start of the archive.
#[derive(Debug, Error)]
pub enum BsaError {
    #[error("I/O error at offset {offset}: {source}")]
    Io {
        offset: u64,
        #[source]
        source: io::Error,
    },

    #[error("truncated at offset {offset}: {what} needs {needed} bytes, {available} remain")]
    Truncated {
        offset: u64,
        what: &'static str,
        needed: u64,
        available: u64,
    },

    #[error("not a BSA archive: magic {found:02x?} at offset 0")]
    BadMagic { found: [u8; 4] },

    #[error("unsupported BSA version {version} at offset 4: only version 104 is read")]
    UnsupportedVersion { version: u32 },

    #[error(
        "big-endian (Xbox 360) archive, flags {flags:#x} at offset 12: only little-endian \
         archives are read"
    )]
    BigEndian { flags: u32 },

    #[error(
        "folder records offset {value} at offset 8 is outside the archive ({archive_len} bytes) \
         or inside the header"
    )]
    BadHeaderOffset { value: u32, archive_len: u64 },

    #[error("{what} count {count} at offset {offset} needs {needed} bytes, {available} remain")]
    CountTooLarge {
        offset: u64,
        what: &'static str,
        count: u64,
        needed: u64,
        available: u64,
    },

    #[error(
        "file count mismatch at offset {offset}: the header says {header}, the folder records \
         say {folders}"
    )]
    FileCountMismatch {
        offset: u64,
        header: u32,
        folders: u64,
    },

    #[error("unterminated {what} at offset {offset}")]
    UnterminatedName { offset: u64, what: &'static str },

    #[error(
        "entry {path:?} at offset {offset}: {size} stored bytes run past the end of the archive \
         ({archive_len} bytes)"
    )]
    EntryOutOfBounds {
        path: String,
        offset: u64,
        size: u64,
        archive_len: u64,
    },

    #[error("entry {path:?} at offset {offset}: {size} bytes exceed the limit of {limit}")]
    EntryTooLarge {
        path: String,
        offset: u64,
        size: u64,
        limit: u64,
    },

    #[error("entry {path:?} at offset {offset}: malformed {what}")]
    BadEntryData {
        path: String,
        offset: u64,
        what: &'static str,
    },

    #[error("entry {path:?} at offset {offset}: zlib stream is invalid: {source}")]
    Decompress {
        path: String,
        offset: u64,
        #[source]
        source: io::Error,
    },

    #[error(
        "entry {path:?} at offset {offset}: decompressed to {actual} bytes, the entry says \
         {expected}"
    )]
    SizeMismatch {
        path: String,
        offset: u64,
        expected: u64,
        actual: u64,
    },
}
