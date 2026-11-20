//! Reader for version 104 BSA archives.
//!
//! Version 104 is the archive layout with a 36-byte header and 16-byte
//! folder records. [`BsaArchive::open`] reads only the tables (header, folder
//! records, file records and names) and then seeks for each entry, so an
//! archive of several gigabytes costs a few megabytes of memory.
//!
//! Paths are exposed lowercase with `\` separators, as stored. Bytes above
//! 0x7F in a stored name map to the Unicode code point of the same value, so
//! that every stored name has exactly one `String` form.
//!
//! [`fixture::BsaBuilder`] writes archives from in-memory files. Tests in
//! this workspace build their inputs with it; no test needs a game install.

mod archive;
mod error;
pub mod fixture;
mod hash;

pub use archive::{BsaArchive, BsaEntry, DEFAULT_MAX_ENTRY_SIZE, HashKind, HashMismatch};
pub use error::BsaError;
pub use hash::{hash_file, hash_folder, hash_path};

/// Archive flag bits of the header field at offset 12.
pub mod flags {
    /// Folder names are stored before each folder's file records.
    pub const DIR_NAMES: u32 = 0x1;
    /// A block of file names follows the file records.
    pub const FILE_NAMES: u32 = 0x2;
    /// Entries are zlib-compressed unless their size field says otherwise.
    pub const COMPRESSED: u32 = 0x4;
    /// Big-endian archive written for the Xbox 360. Not supported.
    pub const BIG_ENDIAN: u32 = 0x40;
    /// Every entry's data starts with its full path as a length-prefixed
    /// string.
    pub const EMBED_NAMES: u32 = 0x100;
}

/// Bit of a file record's size field that inverts the archive's compression
/// default for that entry.
pub const SIZE_COMPRESSION_TOGGLE: u32 = 0x4000_0000;

/// Bits of a file record's size field that hold the stored size.
pub const SIZE_MASK: u32 = 0x3FFF_FFFF;

#[cfg(test)]
mod tests;
