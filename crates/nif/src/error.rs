//! The crate's single error type.

use std::fmt;

/// Where in a NIF file an error was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// The file header, up to and including the group list.
    Header,
    /// One block, by its index in the file and its type name.
    Block {
        /// Zero-based block index.
        index: u32,
        /// Block type name from the header's type table.
        type_name: String,
    },
    /// The footer (root list) after the last block.
    Footer,
}

impl Location {
    pub(crate) fn block(index: u32, type_name: &str) -> Self {
        Location::Block {
            index,
            type_name: type_name.to_owned(),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Location::Header => f.write_str("header"),
            Location::Block { index, type_name } => write!(f, "block {index} ({type_name})"),
            Location::Footer => f.write_str("footer"),
        }
    }
}

/// Every failure this crate reports. Offsets are absolute byte offsets into
/// the input (the NIF file, or the `.glb` file for [`NifError::Glb`]).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NifError {
    /// The input does not start with a NIF header line.
    #[error("offset {offset:#x}: not a NIF file: {reason}")]
    NotNif {
        /// Byte offset of the problem.
        offset: usize,
        /// What was wrong.
        reason: &'static str,
    },

    /// A NIF file, but not version 20.2.0.7 / user 11 / stream 34.
    #[error(
        "offset {offset:#x}: unsupported NIF version {version} (user version {user_version}, \
         stream version {stream_version}); only 20.2.0.7 / 11 / 34 is supported"
    )]
    UnsupportedVersion {
        /// Byte offset of the field that decided it.
        offset: usize,
        /// Dotted version, for example `20.0.0.5`.
        version: String,
        /// User version (0 when it was not read).
        user_version: u32,
        /// Bethesda stream version (0 when absent or not read).
        stream_version: u32,
    },

    /// A fixed-size field does not fit in the bytes that remain. Inside a
    /// block, "remain" means within the block's declared size.
    #[error(
        "{location}, offset {offset:#x}: {what} needs {needed} bytes but {remaining} remain \
         (inside a block the limit is its declared size)"
    )]
    Truncated {
        /// Header, block or footer.
        location: Location,
        /// Byte offset of the field.
        offset: usize,
        /// The field being read.
        what: &'static str,
        /// Bytes the field needs.
        needed: u64,
        /// Bytes available.
        remaining: u64,
    },

    /// A count read from the file needs more bytes than remain.
    #[error(
        "{location}, offset {offset:#x}: {what} count {count} needs {needed} bytes but \
         {remaining} remain"
    )]
    CountTooLarge {
        /// Header, block or footer.
        location: Location,
        /// Byte offset of the count field.
        offset: usize,
        /// The array being counted.
        what: &'static str,
        /// The count as stored.
        count: u64,
        /// Bytes the array would need at its minimum element size.
        needed: u64,
        /// Bytes available after the count field.
        remaining: u64,
    },

    /// A field holds a value that cannot be right.
    #[error("{location}, offset {offset:#x}: invalid {what}: {value}")]
    BadValue {
        /// Header, block or footer.
        location: Location,
        /// Byte offset of the field.
        offset: usize,
        /// The field.
        what: &'static str,
        /// The stored value.
        value: i64,
    },

    /// A block this crate parses field by field did not end where the header
    /// says it ends. The hand-written layout for this block type is wrong or
    /// incomplete for this file.
    #[error(
        "block {index} ({type_name}) at offset {offset:#x}: declared size is {declared} bytes \
         but the parser consumed {consumed}, leaving {} unread from offset {:#x}; the \
         hand-written layout of {type_name} does not match this file",
        declared - consumed,
        offset + *consumed as usize
    )]
    BlockSizeMismatch {
        /// Zero-based block index.
        index: u32,
        /// Block type name.
        type_name: String,
        /// Byte offset where the block starts.
        offset: usize,
        /// Size from the header's block size table.
        declared: u32,
        /// Bytes the parser read.
        consumed: u32,
    },

    /// The parsed blocks do not form a usable scene: a cyclic node graph, a
    /// triangle index past the vertex count, a non-finite position.
    #[error("block {index} ({type_name}) at offset {offset:#x}: {reason}")]
    Scene {
        /// Zero-based block index.
        index: u32,
        /// Block type name.
        type_name: String,
        /// Byte offset where the block starts.
        offset: usize,
        /// What was wrong.
        reason: String,
    },

    /// A file that parsed but holds no usable animation.
    #[error("animation: {reason}")]
    Animation {
        /// What was wrong.
        reason: String,
    },

    /// A `.glb` file that [`crate::glb::read`] cannot accept.
    #[error("glb, offset {offset:#x}: {reason}")]
    Glb {
        /// Byte offset into the `.glb` (0 when the problem is in the JSON).
        offset: usize,
        /// What was wrong.
        reason: String,
    },
}
