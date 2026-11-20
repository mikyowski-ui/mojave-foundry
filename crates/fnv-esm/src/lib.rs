//! Table-driven walker and typed extraction for ESM/ESP plugin files.
//!
//! The layout read here is the one with 24-byte record and group headers,
//! `u16` subrecord sizes with the `XXXX` extended-size escape, and
//! zlib-compressed records flagged in the record header.
//!
//! # Reading
//!
//! [`Plugin::parse`] borrows the whole file as one byte slice and reads the
//! `TES4` header. From there:
//!
//! - [`Plugin::walk`] gives a [`Walker`]: a depth-first stream of
//!   [`Event`]s. [`Walker::context`] is the chain of groups around the
//!   current record, which says which worldspace and cell a record belongs
//!   to and whether a reference is persistent, temporary or visible when
//!   distant. [`Walker::skip_group`] steps over a group.
//! - [`Plugin::top_level`] reads only the top-level group headers, so a
//!   consumer picks the groups it wants and never touches the rest.
//! - A [`RecordRef`] is a header and a borrowed payload. Nothing is
//!   decompressed or copied until [`RecordRef::data`],
//!   [`RecordRef::to_raw`] or [`RecordRef::decode`] is called.
//!
//! # Typed records
//!
//! [`DECODERS`] maps a record signature to its decoder; [`Record`] has one
//! variant per typed record. Unknown record types and unknown subrecords
//! are never an error: an unknown record comes back whole as
//! [`Record::Unknown`], and every typed record keeps the subrecords its
//! decoder did not consume in its `unknown` list. See [`records`] for the
//! decoding rules.
//!
//! # Form ids
//!
//! Form ids are reported as stored. [`MasterList::resolve`] turns one into
//! the owning plugin's lowercase file name and a 24-bit local id.
//!
//! # Text
//!
//! Strings are decoded as Windows-1252, without loss; see [`strings`].
//!
//! # Robustness
//!
//! Every size read from a file is checked against the bytes that remain in
//! its container before anything is sliced or allocated. Group nesting is
//! capped at [`MAX_GROUP_DEPTH`] and decompressed record size at
//! [`DEFAULT_MAX_DECOMPRESSED`] (or the caller's limit). Malformed input
//! gives an [`EsmError`] with the byte offset and the record signature.
//!
//! # Fixtures
//!
//! [`fixture::PluginBuilder`] writes synthetic plugins. Every test in this
//! crate builds its input with it; none needs a game install.

mod bytes;
mod error;
pub mod fixture;
mod formid;
mod header;
pub mod records;
mod sig;
pub mod strings;
mod subrecord;
mod walker;

pub use error::EsmError;
pub use formid::{FormId, MasterList, ResolvedFormId};
pub use header::{
    CellChildKind, GroupHeader, GroupKind, HEADER_LEN, RecordFlags, RecordHeader, RecordMeta,
};
pub use records::*;
pub use sig::{Sig, sigs};
pub use subrecord::{RawSubrecord, SUBRECORD_HEADER_LEN, SubrecordIter, SubrecordRef};
pub use walker::{
    Context, DEFAULT_MAX_DECOMPRESSED, Event, GroupRef, MAX_GROUP_DEPTH, Plugin, RecordData,
    RecordRef, Tes4Header, TopItem, TopLevel, Walker,
};
