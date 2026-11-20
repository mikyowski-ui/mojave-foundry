//! Record and group headers. Both are 24 bytes in this game.

use serde::{Deserialize, Serialize};

use crate::formid::FormId;
use crate::sig::Sig;

/// Bytes of a record header and of a group header.
pub const HEADER_LEN: usize = 24;

/// The flags field of a record header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecordFlags(pub u32);

impl RecordFlags {
    /// On `TES4`: the plugin is a master file.
    pub const MASTER: u32 = 0x0000_0001;
    /// The record is deleted.
    pub const DELETED: u32 = 0x0000_0020;
    /// On a placed reference: persistent. (On a base object: quest item.)
    pub const PERSISTENT: u32 = 0x0000_0400;
    /// On a placed reference: initially disabled.
    pub const INITIALLY_DISABLED: u32 = 0x0000_0800;
    /// The record is ignored.
    pub const IGNORED: u32 = 0x0000_1000;
    /// Visible when distant.
    pub const VISIBLE_WHEN_DISTANT: u32 = 0x0000_8000;
    /// The record data is a `u32` decompressed size and a zlib stream.
    pub const COMPRESSED: u32 = 0x0004_0000;

    /// Whether every bit of `mask` is set.
    pub const fn has(self, mask: u32) -> bool {
        self.0 & mask == mask
    }

    /// Whether the record is deleted.
    pub const fn is_deleted(self) -> bool {
        self.has(Self::DELETED)
    }

    /// Whether the placed reference is persistent.
    pub const fn is_persistent(self) -> bool {
        self.has(Self::PERSISTENT)
    }

    /// Whether the placed reference starts disabled.
    pub const fn is_initially_disabled(self) -> bool {
        self.has(Self::INITIALLY_DISABLED)
    }

    /// Whether the record data is compressed in the file.
    pub const fn is_compressed(self) -> bool {
        self.has(Self::COMPRESSED)
    }

    /// Whether the record is visible when distant.
    pub const fn is_visible_when_distant(self) -> bool {
        self.has(Self::VISIBLE_WHEN_DISTANT)
    }
}

/// The header fields that travel with a record however it is represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecordMeta {
    /// The record's form id, as stored (not resolved against masters).
    pub form_id: FormId,
    /// The record flags.
    pub flags: RecordFlags,
    /// Version-control field, as stored.
    pub revision: u32,
    /// Form version, as stored.
    pub form_version: u16,
    /// The last two header bytes, as stored.
    pub unknown: u16,
}

impl RecordMeta {
    /// Header fields for a new record: the form id, everything else zero.
    pub fn new(form_id: impl Into<FormId>) -> Self {
        Self {
            form_id: form_id.into(),
            ..Self::default()
        }
    }
}

/// A record header as read from a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHeader {
    /// The record's signature.
    pub sig: Sig,
    /// Bytes of record data in the file, after the header. For a compressed
    /// record this is the compressed size including the 4-byte prefix.
    pub data_size: u32,
    /// The remaining header fields.
    pub meta: RecordMeta,
}

/// Which children group of a cell a record sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CellChildKind {
    /// Persistent children: always loaded.
    Persistent,
    /// Temporary children: loaded with the cell. `LAND` and `NAVM` live
    /// here too.
    Temporary,
    /// Visible-when-distant children.
    VisibleDistant,
}

/// A group's type and decoded label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    /// Type 0: a top-level group of one record type.
    Top(Sig),
    /// Type 1: the children of a worldspace.
    WorldChildren(FormId),
    /// Type 2: an interior cell block.
    InteriorBlock(i32),
    /// Type 3: an interior cell sub-block.
    InteriorSubBlock(i32),
    /// Type 4: an exterior cell block, in block coordinates.
    ExteriorBlock {
        /// Block X (cell X divided by 32, rounded down).
        x: i16,
        /// Block Y.
        y: i16,
    },
    /// Type 5: an exterior cell sub-block, in sub-block coordinates.
    ExteriorSubBlock {
        /// Sub-block X (cell X divided by 8, rounded down).
        x: i16,
        /// Sub-block Y.
        y: i16,
    },
    /// Type 6: the children of a cell.
    CellChildren(FormId),
    /// Type 7: the `INFO` children of a topic.
    TopicChildren(FormId),
    /// Types 8, 9 and 10: one of a cell's three children groups.
    CellChild(CellChildKind, FormId),
    /// Any other type; see [`GroupHeader::group_type`] and
    /// [`GroupHeader::label`].
    Unknown,
}

/// A group header as read from a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupHeader {
    /// Byte offset of the header in the file.
    pub offset: usize,
    /// Size of the group including this header.
    pub size: u32,
    /// The label, as stored. Its meaning depends on the type.
    pub label: [u8; 4],
    /// The group type, as stored.
    pub group_type: i32,
    /// The stamp field, as stored.
    pub stamp: u32,
    /// The last four header bytes, as stored.
    pub unknown: u32,
}

impl GroupHeader {
    /// The type and decoded label.
    ///
    /// Block and sub-block labels of exterior cells are stored Y first:
    /// two `i16`, Y then X. Checked against a retail master, where every
    /// exterior cell's grid position fits its block and sub-block labels
    /// read this way and none fits only the other way.
    pub fn kind(&self) -> GroupKind {
        let label = self.label;
        let form = FormId(u32::from_le_bytes(label));
        let number = i32::from_le_bytes(label);
        let y = i16::from_le_bytes([label[0], label[1]]);
        let x = i16::from_le_bytes([label[2], label[3]]);
        match self.group_type {
            0 => GroupKind::Top(Sig(label)),
            1 => GroupKind::WorldChildren(form),
            2 => GroupKind::InteriorBlock(number),
            3 => GroupKind::InteriorSubBlock(number),
            4 => GroupKind::ExteriorBlock { x, y },
            5 => GroupKind::ExteriorSubBlock { x, y },
            6 => GroupKind::CellChildren(form),
            7 => GroupKind::TopicChildren(form),
            8 => GroupKind::CellChild(CellChildKind::Persistent, form),
            9 => GroupKind::CellChild(CellChildKind::Temporary, form),
            10 => GroupKind::CellChild(CellChildKind::VisibleDistant, form),
            _ => GroupKind::Unknown,
        }
    }
}
