//! The crate's single error type.

use thiserror::Error;

use crate::formid::FormId;
use crate::sig::Sig;

/// Everything that can go wrong while reading a plugin.
///
/// Parse errors carry the byte offset in the file and the signature of the
/// record (or `GRUP`) being read. Inside a compressed record there is no
/// file offset for a subrecord, so the offset is that of the record header.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EsmError {
    /// A header, size prefix or payload needs more bytes than remain.
    #[error(
        "{sig} at offset {offset:#x}: truncated {what}: need {needed} bytes, {remaining} remain"
    )]
    Truncated {
        /// Byte offset in the file.
        offset: usize,
        /// Signature of the record or group being read.
        sig: Sig,
        /// What was being read.
        what: &'static str,
        /// Bytes required.
        needed: u64,
        /// Bytes available in the enclosing container.
        remaining: u64,
    },

    /// The file does not start with a `TES4` record.
    #[error("{sig} at offset {offset:#x}: not a plugin: the first record must be TES4")]
    NotAPlugin {
        /// Byte offset in the file.
        offset: usize,
        /// The signature found instead.
        sig: Sig,
    },

    /// The `TES4` record has no usable `HEDR` subrecord.
    #[error("{sig} at offset {offset:#x}: plugin header has no valid HEDR subrecord")]
    MissingHedr {
        /// Byte offset of the `TES4` record.
        offset: usize,
        /// Always `TES4`.
        sig: Sig,
    },

    /// A group declares a size smaller than its own header.
    #[error("{sig} at offset {offset:#x}: group size {size} is smaller than the 24-byte header")]
    GroupSizeTooSmall {
        /// Byte offset of the group header.
        offset: usize,
        /// Always `GRUP`.
        sig: Sig,
        /// The declared size.
        size: u32,
    },

    /// Groups are nested deeper than any valid plugin nests them.
    #[error("{sig} at offset {offset:#x}: groups nested deeper than {limit}")]
    GroupNestingTooDeep {
        /// Byte offset of the group header.
        offset: usize,
        /// Always `GRUP`.
        sig: Sig,
        /// The nesting limit.
        limit: usize,
    },

    /// An `XXXX` extended-size marker is malformed.
    #[error("{sig} at offset {offset:#x}: bad XXXX extended-size marker: {reason}")]
    BadExtendedSize {
        /// Byte offset of the marker (or of the record when compressed).
        offset: usize,
        /// Signature of the enclosing record.
        sig: Sig,
        /// What is wrong with it.
        reason: &'static str,
    },

    /// A compressed record declares more decompressed bytes than allowed.
    #[error("{sig} at offset {offset:#x}: decompressed size {declared} exceeds the limit {limit}")]
    DecompressedTooLarge {
        /// Byte offset of the record header.
        offset: usize,
        /// Signature of the record.
        sig: Sig,
        /// Size declared by the record.
        declared: u64,
        /// The limit in force.
        limit: u64,
    },

    /// The zlib stream of a compressed record is invalid.
    #[error("{sig} at offset {offset:#x}: invalid zlib stream: {message}")]
    Decompress {
        /// Byte offset of the record header.
        offset: usize,
        /// Signature of the record.
        sig: Sig,
        /// The decoder's message.
        message: String,
    },

    /// The zlib stream ended before, or ran past, the declared size.
    #[error(
        "{sig} at offset {offset:#x}: zlib stream gives {actual} bytes where {declared} are declared"
    )]
    DecompressedSizeMismatch {
        /// Byte offset of the record header.
        offset: usize,
        /// Signature of the record.
        sig: Sig,
        /// Size declared by the record.
        declared: u64,
        /// Bytes produced; `declared + 1` stands for "more than declared".
        actual: u64,
    },

    /// A form id's top byte points past the plugin's master list.
    #[error("form id {form_id}: master index {index} out of range for {master_count} masters")]
    MasterIndexOutOfRange {
        /// The form id being resolved.
        form_id: FormId,
        /// Its top byte.
        index: u8,
        /// Number of masters the plugin lists.
        master_count: usize,
    },

    /// The first plugin's `HEDR` version is neither game's; see
    /// [`crate::GameVariant::detect`].
    #[error(
        "plugin {plugin}: HEDR version {version} is neither Fallout 3 (0.94) nor New Vegas \
         (1.34); name the game explicitly to read it anyway"
    )]
    UnknownGame {
        /// The plugin's file name.
        plugin: String,
        /// The stored version, as text.
        version: String,
    },

    /// The first plugin is named for one game and declares the other's
    /// `HEDR` version.
    #[error(
        "plugin {plugin}: the file name is {name_game}'s master but HEDR version {version} is \
         {version_game}'s; name the game explicitly to read it anyway"
    )]
    GameMismatch {
        /// The plugin's file name.
        plugin: String,
        /// The stored version, as text.
        version: String,
        /// The game the version names.
        version_game: crate::GameVariant,
        /// The game the file name names.
        name_game: crate::GameVariant,
    },
}

impl EsmError {
    /// The byte offset the error was detected at, when it has one.
    pub fn offset(&self) -> Option<usize> {
        match self {
            Self::Truncated { offset, .. }
            | Self::NotAPlugin { offset, .. }
            | Self::MissingHedr { offset, .. }
            | Self::GroupSizeTooSmall { offset, .. }
            | Self::GroupNestingTooDeep { offset, .. }
            | Self::BadExtendedSize { offset, .. }
            | Self::DecompressedTooLarge { offset, .. }
            | Self::Decompress { offset, .. }
            | Self::DecompressedSizeMismatch { offset, .. } => Some(*offset),
            Self::MasterIndexOutOfRange { .. }
            | Self::UnknownGame { .. }
            | Self::GameMismatch { .. } => None,
        }
    }

    /// The record or group signature the error belongs to, when it has one.
    pub fn sig(&self) -> Option<Sig> {
        match self {
            Self::Truncated { sig, .. }
            | Self::NotAPlugin { sig, .. }
            | Self::MissingHedr { sig, .. }
            | Self::GroupSizeTooSmall { sig, .. }
            | Self::GroupNestingTooDeep { sig, .. }
            | Self::BadExtendedSize { sig, .. }
            | Self::DecompressedTooLarge { sig, .. }
            | Self::Decompress { sig, .. }
            | Self::DecompressedSizeMismatch { sig, .. } => Some(*sig),
            Self::MasterIndexOutOfRange { .. } => None,
            Self::UnknownGame { .. } | Self::GameMismatch { .. } => Some(crate::sig::sigs::TES4),
        }
    }
}
