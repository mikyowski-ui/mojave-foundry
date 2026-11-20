//! Bounds-checked little-endian reader over one region of a NIF file.

use crate::blocks::Ref;
use crate::error::{Location, NifError};

/// Reads from `data`, which is either the whole file (header, footer) or
/// exactly one block's declared extent. `base` is the absolute offset of
/// `data[0]`, so that every error carries a file offset.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    base: usize,
    loc: &'a Location,
    strings: &'a [String],
    num_blocks: u32,
    stream_version: u32,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(
        data: &'a [u8],
        base: usize,
        loc: &'a Location,
        strings: &'a [String],
        num_blocks: u32,
    ) -> Self {
        Reader {
            data,
            pos: 0,
            base,
            loc,
            strings,
            num_blocks,
            stream_version: crate::header::STREAM_VERSION,
        }
    }

    /// Sets the Bethesda stream version the region was written with.
    pub(crate) fn with_stream_version(mut self, stream_version: u32) -> Self {
        self.stream_version = stream_version;
        self
    }

    /// The Bethesda stream version the region was written with.
    pub(crate) fn stream_version(&self) -> u32 {
        self.stream_version
    }

    /// Absolute file offset of the next byte.
    pub(crate) fn offset(&self) -> usize {
        self.base + self.pos
    }

    /// Bytes read so far from this region.
    pub(crate) fn consumed(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn bad_value(&self, offset: usize, what: &'static str, value: i64) -> NifError {
        NifError::BadValue {
            location: self.loc.clone(),
            offset,
            what,
            value,
        }
    }

    pub(crate) fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], NifError> {
        if n > self.remaining() {
            return Err(NifError::Truncated {
                location: self.loc.clone(),
                offset: self.offset(),
                what,
                needed: n as u64,
                remaining: self.remaining() as u64,
            });
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn fixed<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], NifError> {
        let slice = self.take(N, what)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    pub(crate) fn u8(&mut self, what: &'static str) -> Result<u8, NifError> {
        Ok(self.fixed::<1>(what)?[0])
    }

    pub(crate) fn u16(&mut self, what: &'static str) -> Result<u16, NifError> {
        Ok(u16::from_le_bytes(self.fixed(what)?))
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32, NifError> {
        Ok(u32::from_le_bytes(self.fixed(what)?))
    }

    pub(crate) fn i32(&mut self, what: &'static str) -> Result<i32, NifError> {
        Ok(i32::from_le_bytes(self.fixed(what)?))
    }

    pub(crate) fn f32(&mut self, what: &'static str) -> Result<f32, NifError> {
        Ok(f32::from_le_bytes(self.fixed(what)?))
    }

    /// A one-byte boolean. Anything but 0 or 1 is an error: in practice it
    /// means the parser is misaligned, and saying so early is more useful
    /// than reading on.
    pub(crate) fn bool(&mut self, what: &'static str) -> Result<bool, NifError> {
        let at = self.offset();
        match self.u8(what)? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(self.bad_value(at, what, i64::from(other))),
        }
    }

    /// Checks that `count` elements of at least `elem_size` bytes fit in
    /// what remains. `at` is the offset of the count field, for the error.
    pub(crate) fn check_count(
        &self,
        count: u64,
        elem_size: usize,
        what: &'static str,
        at: usize,
    ) -> Result<usize, NifError> {
        let needed = count.saturating_mul(elem_size as u64);
        if needed > self.remaining() as u64 {
            return Err(NifError::CountTooLarge {
                location: self.loc.clone(),
                offset: at,
                what,
                count,
                needed,
                remaining: self.remaining() as u64,
            });
        }
        // `needed <= remaining <= usize::MAX`, and `elem_size >= 1` at every
        // call site, so the count itself fits.
        Ok(count as usize)
    }

    /// Reads a `u32` count and bounds it against the bytes remaining.
    pub(crate) fn count_u32(
        &mut self,
        elem_size: usize,
        what: &'static str,
    ) -> Result<usize, NifError> {
        let at = self.offset();
        let count = self.u32(what)?;
        self.check_count(u64::from(count), elem_size, what, at)
    }

    /// Reads a `u16` count and bounds it against the bytes remaining.
    pub(crate) fn count_u16(
        &mut self,
        elem_size: usize,
        what: &'static str,
    ) -> Result<usize, NifError> {
        let at = self.offset();
        let count = self.u16(what)?;
        self.check_count(u64::from(count), elem_size, what, at)
    }

    pub(crate) fn vec2(&mut self, what: &'static str) -> Result<[f32; 2], NifError> {
        Ok([self.f32(what)?, self.f32(what)?])
    }

    pub(crate) fn vec3(&mut self, what: &'static str) -> Result<[f32; 3], NifError> {
        Ok([self.f32(what)?, self.f32(what)?, self.f32(what)?])
    }

    pub(crate) fn vec4(&mut self, what: &'static str) -> Result<[f32; 4], NifError> {
        Ok([
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
        ])
    }

    /// Nine floats, three per row: row `r` dotted with a column vector gives
    /// component `r` of the rotated vector.
    pub(crate) fn matrix33(&mut self, what: &'static str) -> Result<[[f32; 3]; 3], NifError> {
        Ok([self.vec3(what)?, self.vec3(what)?, self.vec3(what)?])
    }

    /// `count` elements of `elem_size` bytes each; the caller has not yet
    /// checked the count.
    pub(crate) fn array<T>(
        &mut self,
        count: usize,
        elem_size: usize,
        what: &'static str,
        mut read: impl FnMut(&mut Self, &'static str) -> Result<T, NifError>,
    ) -> Result<Vec<T>, NifError> {
        let at = self.offset();
        let count = self.check_count(count as u64, elem_size, what, at)?;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(read(self, what)?);
        }
        Ok(out)
    }

    pub(crate) fn u16_array(
        &mut self,
        count: usize,
        what: &'static str,
    ) -> Result<Vec<u16>, NifError> {
        self.array(count, 2, what, Self::u16)
    }

    /// A string with a `u32` byte length, stored inline.
    pub(crate) fn sized_string(&mut self, what: &'static str) -> Result<String, NifError> {
        let len = self.count_u32(1, what)?;
        Ok(latin1(self.take(len, what)?))
    }

    /// A string with a `u8` byte length that includes its NUL terminator.
    pub(crate) fn short_string(&mut self, what: &'static str) -> Result<String, NifError> {
        let len = usize::from(self.u8(what)?);
        let bytes = self.take(len, what)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(latin1(&bytes[..end]))
    }

    /// A block reference: `-1` is none, anything else must name a block.
    pub(crate) fn reference(&mut self, what: &'static str) -> Result<Ref, NifError> {
        let at = self.offset();
        let raw = self.i32(what)?;
        if raw == -1 {
            return Ok(None);
        }
        match u32::try_from(raw) {
            Ok(index) if index < self.num_blocks => Ok(Some(index)),
            _ => Err(self.bad_value(at, what, i64::from(raw))),
        }
    }

    /// A `u32` count followed by that many references.
    pub(crate) fn ref_array(&mut self, what: &'static str) -> Result<Vec<Ref>, NifError> {
        let count = self.count_u32(4, what)?;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(self.reference(what)?);
        }
        Ok(out)
    }

    /// An index into the header's string table; `0xFFFFFFFF` is the empty
    /// string.
    pub(crate) fn string(&mut self, what: &'static str) -> Result<String, NifError> {
        let at = self.offset();
        let index = self.u32(what)?;
        if index == u32::MAX {
            return Ok(String::new());
        }
        match self.strings.get(index as usize) {
            Some(s) => Ok(s.clone()),
            None => Err(self.bad_value(at, what, i64::from(index))),
        }
    }

    /// Everything left in this region.
    pub(crate) fn rest(&mut self) -> Vec<u8> {
        let out = self.data[self.pos..].to_vec();
        self.pos = self.data.len();
        out
    }
}

/// Decodes bytes as Latin-1: lossless, deterministic, never fails.
pub(crate) fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}
