//! Subrecords: the `(signature, u16 size, payload)` fields inside a record,
//! with the `XXXX` escape for payloads of 64 KiB and more.

use serde::{Deserialize, Serialize};

use crate::bytes::{Wr, zstring_bytes};
use crate::error::EsmError;
use crate::formid::FormId;
use crate::sig::{Sig, sigs};

/// Bytes of a subrecord header: signature and `u16` size.
pub const SUBRECORD_HEADER_LEN: usize = 6;

/// An owned subrecord, exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RawSubrecord {
    /// The subrecord's signature.
    pub sig: Sig,
    /// The payload.
    pub data: Vec<u8>,
}

impl RawSubrecord {
    /// A subrecord with the given payload.
    pub fn new(sig: &[u8; 4], data: impl Into<Vec<u8>>) -> Self {
        Self {
            sig: Sig::new(sig),
            data: data.into(),
        }
    }

    /// Borrows this subrecord as a [`SubrecordRef`] with offset 0.
    pub fn view(&self) -> SubrecordRef<'_> {
        SubrecordRef {
            sig: self.sig,
            data: &self.data,
            offset: 0,
        }
    }
}

/// A subrecord borrowed from a record's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubrecordRef<'a> {
    /// The subrecord's signature.
    pub sig: Sig,
    /// The payload.
    pub data: &'a [u8],
    /// File offset of the subrecord header; for a compressed record, the
    /// offset of the record header.
    pub offset: usize,
}

impl SubrecordRef<'_> {
    /// Copies the subrecord.
    pub fn to_raw(&self) -> RawSubrecord {
        RawSubrecord {
            sig: self.sig,
            data: self.data.to_vec(),
        }
    }
}

/// Iterates the subrecords of one record's data. After an error it yields
/// nothing more.
#[derive(Debug, Clone)]
pub struct SubrecordIter<'a> {
    bytes: &'a [u8],
    pos: usize,
    record_sig: Sig,
    base_offset: usize,
    offsets_move: bool,
    failed: bool,
}

impl<'a> SubrecordIter<'a> {
    /// Iterates `bytes`, the uncompressed data of a record with signature
    /// `record_sig`.
    ///
    /// `base_offset` is the file offset of `bytes[0]` when `offsets_move`,
    /// otherwise a fixed offset reported for every subrecord and error (the
    /// record header, for data that was decompressed).
    pub fn new(bytes: &'a [u8], record_sig: Sig, base_offset: usize, offsets_move: bool) -> Self {
        Self {
            bytes,
            pos: 0,
            record_sig,
            base_offset,
            offsets_move,
            failed: false,
        }
    }

    fn offset_of(&self, pos: usize) -> usize {
        if self.offsets_move {
            self.base_offset.saturating_add(pos)
        } else {
            self.base_offset
        }
    }

    fn truncated(&self, pos: usize, what: &'static str, needed: usize) -> EsmError {
        EsmError::Truncated {
            offset: self.offset_of(pos),
            sig: self.record_sig,
            what,
            needed: needed as u64,
            remaining: (self.bytes.len() - pos.min(self.bytes.len())) as u64,
        }
    }

    /// Reads a 6-byte header at the cursor.
    fn header(&mut self) -> Result<(Sig, u16), EsmError> {
        let Some(head) = self
            .bytes
            .get(self.pos..)
            .and_then(|rest| rest.first_chunk::<SUBRECORD_HEADER_LEN>())
        else {
            return Err(self.truncated(self.pos, "subrecord header", SUBRECORD_HEADER_LEN));
        };
        let sig = Sig([head[0], head[1], head[2], head[3]]);
        let size = u16::from_le_bytes([head[4], head[5]]);
        self.pos += SUBRECORD_HEADER_LEN;
        Ok((sig, size))
    }

    fn read(&mut self) -> Result<SubrecordRef<'a>, EsmError> {
        let start = self.pos;
        let (mut sig, size) = self.header()?;
        let mut len = usize::from(size);
        if sig == sigs::XXXX {
            if size != 4 {
                return Err(EsmError::BadExtendedSize {
                    offset: self.offset_of(start),
                    sig: self.record_sig,
                    reason: "the marker's own size is not 4",
                });
            }
            let Some(ext) = self
                .bytes
                .get(self.pos..)
                .and_then(|rest| rest.first_chunk::<4>())
            else {
                return Err(self.truncated(self.pos, "extended subrecord size", 4));
            };
            len = usize::try_from(u32::from_le_bytes(*ext)).unwrap_or(usize::MAX);
            self.pos += 4;
            if self.pos >= self.bytes.len() {
                return Err(EsmError::BadExtendedSize {
                    offset: self.offset_of(start),
                    sig: self.record_sig,
                    reason: "no subrecord follows the marker",
                });
            }
            // The size field of the subrecord after the marker is ignored.
            (sig, _) = self.header()?;
        }
        let Some(data) = self.bytes.get(self.pos..).and_then(|rest| rest.get(..len)) else {
            return Err(self.truncated(self.pos, "subrecord data", len));
        };
        self.pos += len;
        Ok(SubrecordRef {
            sig,
            data,
            offset: self.offset_of(start),
        })
    }
}

impl<'a> Iterator for SubrecordIter<'a> {
    type Item = Result<SubrecordRef<'a>, EsmError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.pos >= self.bytes.len() {
            return None;
        }
        let item = self.read();
        if item.is_err() {
            self.failed = true;
        }
        Some(item)
    }
}

/// Collects subrecords for a typed record's `encode`.
#[derive(Default)]
pub(crate) struct Out(pub(crate) Vec<RawSubrecord>);

impl Out {
    pub(crate) fn raw(&mut self, sig: &[u8; 4], data: Vec<u8>) {
        self.0.push(RawSubrecord::new(sig, data));
    }

    pub(crate) fn opt(&mut self, sig: &[u8; 4], data: Option<Vec<u8>>) {
        if let Some(data) = data {
            self.raw(sig, data);
        }
    }

    pub(crate) fn zstr(&mut self, sig: &[u8; 4], text: &Option<String>) {
        self.opt(sig, text.as_deref().map(zstring_bytes));
    }

    pub(crate) fn form(&mut self, sig: &[u8; 4], form: Option<FormId>) {
        self.opt(sig, form.map(|f| Wr::default().form(f).done()));
    }

    /// One subrecord per form id.
    pub(crate) fn form_each(&mut self, sig: &[u8; 4], forms: &[FormId]) {
        for &form in forms {
            self.form(sig, Some(form));
        }
    }

    /// One subrecord holding all the form ids; nothing when there are none.
    pub(crate) fn form_array(&mut self, sig: &[u8; 4], forms: &[FormId]) {
        if !forms.is_empty() {
            let data = forms.iter().fold(Wr::default(), |w, &f| w.form(f)).done();
            self.raw(sig, data);
        }
    }

    pub(crate) fn f32(&mut self, sig: &[u8; 4], value: Option<f32>) {
        self.opt(sig, value.map(|v| v.to_le_bytes().to_vec()));
    }

    pub(crate) fn u32(&mut self, sig: &[u8; 4], value: Option<u32>) {
        self.opt(sig, value.map(|v| v.to_le_bytes().to_vec()));
    }

    pub(crate) fn i32(&mut self, sig: &[u8; 4], value: Option<i32>) {
        self.opt(sig, value.map(|v| v.to_le_bytes().to_vec()));
    }

    pub(crate) fn u16(&mut self, sig: &[u8; 4], value: Option<u16>) {
        self.opt(sig, value.map(|v| v.to_le_bytes().to_vec()));
    }

    pub(crate) fn u8(&mut self, sig: &[u8; 4], value: Option<u8>) {
        self.opt(sig, value.map(|v| vec![v]));
    }

    /// Appends the subrecords a decoder did not consume and returns the lot.
    pub(crate) fn finish(mut self, unknown: &[RawSubrecord]) -> Vec<RawSubrecord> {
        self.0.extend_from_slice(unknown);
        self.0
    }
}
