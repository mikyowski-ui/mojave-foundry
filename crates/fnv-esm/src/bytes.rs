//! Bounds-checked little-endian reading and writing of subrecord payloads.

use crate::formid::FormId;
use crate::strings::{decode_zstring, encode_cp1252_lossy};

/// A cursor over a payload. Every read returns `None` instead of running
/// past the end.
pub(crate) struct Rd<'a> {
    bytes: &'a [u8],
}

impl<'a> Rd<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// A cursor over `bytes`, only when it is exactly `len` bytes long.
    pub(crate) fn exact(bytes: &'a [u8], len: usize) -> Option<Self> {
        (bytes.len() == len).then_some(Self { bytes })
    }

    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.bytes.len() < n {
            return None;
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Some(head)
    }

    pub(crate) fn arr<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    pub(crate) fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.arr::<1>().map(|b| b[0])
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        self.arr().map(u16::from_le_bytes)
    }

    pub(crate) fn i16(&mut self) -> Option<i16> {
        self.arr().map(i16::from_le_bytes)
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.arr().map(u32::from_le_bytes)
    }

    pub(crate) fn i32(&mut self) -> Option<i32> {
        self.arr().map(i32::from_le_bytes)
    }

    pub(crate) fn f32(&mut self) -> Option<f32> {
        self.arr().map(f32::from_le_bytes)
    }

    pub(crate) fn form(&mut self) -> Option<FormId> {
        self.u32().map(FormId)
    }

    pub(crate) fn f32x3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }
}

/// A payload being written.
#[derive(Default)]
pub(crate) struct Wr(pub(crate) Vec<u8>);

impl Wr {
    pub(crate) fn bytes(mut self, bytes: &[u8]) -> Self {
        self.0.extend_from_slice(bytes);
        self
    }

    pub(crate) fn zeros(mut self, n: usize) -> Self {
        self.0.resize(self.0.len() + n, 0);
        self
    }

    pub(crate) fn u8(self, v: u8) -> Self {
        self.bytes(&[v])
    }

    pub(crate) fn u16(self, v: u16) -> Self {
        self.bytes(&v.to_le_bytes())
    }

    pub(crate) fn i16(self, v: i16) -> Self {
        self.bytes(&v.to_le_bytes())
    }

    pub(crate) fn u32(self, v: u32) -> Self {
        self.bytes(&v.to_le_bytes())
    }

    pub(crate) fn i32(self, v: i32) -> Self {
        self.bytes(&v.to_le_bytes())
    }

    pub(crate) fn f32(self, v: f32) -> Self {
        self.bytes(&v.to_le_bytes())
    }

    pub(crate) fn form(self, v: FormId) -> Self {
        self.u32(v.0)
    }

    pub(crate) fn f32x3(self, v: [f32; 3]) -> Self {
        self.f32(v[0]).f32(v[1]).f32(v[2])
    }

    pub(crate) fn done(self) -> Vec<u8> {
        self.0
    }
}

/// A whole payload that is one form id.
pub(crate) fn one_form(data: &[u8]) -> Option<FormId> {
    Rd::exact(data, 4)?.form()
}

/// A whole payload that is one `f32`.
pub(crate) fn one_f32(data: &[u8]) -> Option<f32> {
    Rd::exact(data, 4)?.f32()
}

/// A whole payload that is one `u32`.
pub(crate) fn one_u32(data: &[u8]) -> Option<u32> {
    Rd::exact(data, 4)?.u32()
}

/// A whole payload that is one `i32`.
pub(crate) fn one_i32(data: &[u8]) -> Option<i32> {
    Rd::exact(data, 4)?.i32()
}

/// A whole payload that is one `u16`.
pub(crate) fn one_u16(data: &[u8]) -> Option<u16> {
    Rd::exact(data, 2)?.u16()
}

/// A whole payload that is one `i16`.
pub(crate) fn one_i16(data: &[u8]) -> Option<i16> {
    Rd::exact(data, 2)?.i16()
}

/// A whole payload that is one byte.
pub(crate) fn one_u8(data: &[u8]) -> Option<u8> {
    Rd::exact(data, 1)?.u8()
}

/// A whole payload that is an array of form ids.
pub(crate) fn form_array(data: &[u8]) -> Option<Vec<FormId>> {
    if !data.len().is_multiple_of(4) {
        return None;
    }
    let mut rd = Rd::new(data);
    let mut out = Vec::with_capacity(data.len() / 4);
    while let Some(form) = rd.form() {
        out.push(form);
    }
    Some(out)
}

/// A whole payload that is a NUL-terminated string. Never fails.
pub(crate) fn zstring(data: &[u8]) -> Option<String> {
    Some(decode_zstring(data))
}

/// Text as a NUL-terminated Windows-1252 payload.
pub(crate) fn zstring_bytes(text: &str) -> Vec<u8> {
    let mut bytes = encode_cp1252_lossy(text);
    bytes.push(0);
    bytes
}

/// Fills an empty slot. Returns whether the value was stored, so that a
/// repeated or unparsable subrecord stays with the unconsumed ones.
pub(crate) fn set<T>(slot: &mut Option<T>, value: Option<T>) -> bool {
    match (slot.is_none(), value) {
        (true, Some(v)) => {
            *slot = Some(v);
            true
        }
        _ => false,
    }
}

/// Appends a parsed value. Returns whether there was one.
pub(crate) fn push<T>(list: &mut Vec<T>, value: Option<T>) -> bool {
    match value {
        Some(v) => {
            list.push(v);
            true
        }
        None => false,
    }
}
