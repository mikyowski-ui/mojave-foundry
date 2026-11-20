//! The plugin walker: the `TES4` header, then groups and records with the
//! chain of groups that encloses each one.
//!
//! Everything borrows from the one byte slice the caller holds. A record is
//! decompressed only when its data is asked for, and a group is skipped by
//! adding its size to the cursor.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::bytes::Rd;
use crate::error::EsmError;
use crate::formid::{FormId, MasterList};
use crate::header::{
    CellChildKind, GroupHeader, GroupKind, HEADER_LEN, RecordFlags, RecordHeader, RecordMeta,
};
use crate::records::{RawRecord, Record, decode_record};
use crate::sig::{Sig, sigs};
use crate::strings::decode_zstring;
use crate::subrecord::{RawSubrecord, SubrecordIter, SubrecordRef};

/// Deepest group nesting the walker accepts. A valid plugin nests six deep
/// (top, world children, block, sub-block, cell children, persistent).
pub const MAX_GROUP_DEPTH: usize = 16;

/// Default cap on the decompressed size of one record: 256 MiB.
pub const DEFAULT_MAX_DECOMPRESSED: usize = 256 << 20;

/// First allocation, and smallest growth step, of an inflate buffer.
const INFLATE_CHUNK: usize = 64 << 10;

/// The decoded `TES4` record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tes4Header {
    /// Header fields of the `TES4` record. [`RecordFlags::MASTER`] marks a
    /// master file.
    pub meta: RecordMeta,
    /// File format version from `HEDR`.
    pub version: f32,
    /// Number of records and groups the file says it holds.
    pub record_count: u32,
    /// Next free object id from `HEDR`.
    pub next_object_id: u32,
    /// The author (`CNAM`).
    pub author: Option<String>,
    /// The description (`SNAM`).
    pub description: Option<String>,
    /// Master file names (`MAST`), in file order, as stored.
    pub masters: Vec<String>,
    /// Every other subrecord of the header, in file order.
    pub unknown: Vec<RawSubrecord>,
}

impl Tes4Header {
    /// Whether the master flag is set.
    pub fn is_master(&self) -> bool {
        self.meta.flags.has(RecordFlags::MASTER)
    }

    fn parse(record: &RecordRef<'_>) -> Result<Self, EsmError> {
        let data = record.data()?;
        let mut hedr = None;
        let mut author = None;
        let mut description = None;
        let mut masters = Vec::new();
        let mut unknown = Vec::new();
        for sub in data.subrecords() {
            let sub = sub?;
            match &sub.sig.0 {
                b"HEDR" if hedr.is_none() && sub.data.len() >= 12 => {
                    let mut rd = Rd::new(sub.data);
                    hedr = rd.f32().and_then(|v| Some((v, rd.u32()?, rd.u32()?)));
                }
                b"CNAM" if author.is_none() => author = Some(decode_zstring(sub.data)),
                b"SNAM" if description.is_none() => {
                    description = Some(decode_zstring(sub.data));
                }
                b"MAST" => masters.push(decode_zstring(sub.data)),
                // The 8-byte size placeholder that follows each master.
                b"DATA" if sub.data.len() == 8 && sub.data.iter().all(|&b| b == 0) => {}
                _ => unknown.push(sub.to_raw()),
            }
        }
        let Some((version, record_count, next_object_id)) = hedr else {
            return Err(EsmError::MissingHedr {
                offset: record.offset,
                sig: sigs::TES4,
            });
        };
        Ok(Self {
            meta: record.header.meta,
            version,
            record_count,
            next_object_id,
            author,
            description,
            masters,
            unknown,
        })
    }
}

/// A plugin file held in memory.
#[derive(Debug, Clone)]
pub struct Plugin<'a> {
    data: &'a [u8],
    header: Tes4Header,
    body: usize,
}

impl<'a> Plugin<'a> {
    /// Reads the `TES4` header. Nothing after it is touched yet.
    ///
    /// # Errors
    ///
    /// [`EsmError::NotAPlugin`] when the file does not start with `TES4`,
    /// and the framing errors of a truncated or malformed header.
    pub fn parse(data: &'a [u8]) -> Result<Self, EsmError> {
        match read_item(data, 0, data.len())? {
            Item::Record(record, next) if record.header.sig == sigs::TES4 => Ok(Self {
                data,
                header: Tes4Header::parse(&record)?,
                body: next,
            }),
            Item::Record(record, _) => Err(EsmError::NotAPlugin {
                offset: 0,
                sig: record.header.sig,
            }),
            Item::Group(..) => Err(EsmError::NotAPlugin {
                offset: 0,
                sig: sigs::GRUP,
            }),
        }
    }

    /// The decoded `TES4` header.
    pub fn header(&self) -> &Tes4Header {
        &self.header
    }

    /// The whole file.
    pub fn bytes(&self) -> &'a [u8] {
        self.data
    }

    /// The master list for resolving this plugin's form ids.
    /// `plugin_name` is the plugin's own file name.
    pub fn master_list(&self, plugin_name: &str) -> MasterList {
        MasterList::new(plugin_name, &self.header.masters)
    }

    /// Walks every group and record after the header, depth first.
    pub fn walk(&self) -> Walker<'a> {
        Walker::new(self.data, self.body, self.data.len(), Context::default())
    }

    /// Iterates the top level only. Each group is read as a header and
    /// stepped over, so skipping one costs nothing; call
    /// [`GroupRef::walk`] on the ones wanted.
    pub fn top_level(&self) -> TopLevel<'a> {
        TopLevel {
            data: self.data,
            pos: self.body,
            done: false,
        }
    }

    /// The top-level group with the given label, if the file has one.
    ///
    /// # Errors
    ///
    /// Framing errors met before the group is found.
    pub fn top_group(&self, label: Sig) -> Result<Option<GroupRef<'a>>, EsmError> {
        for item in self.top_level() {
            if let TopItem::Group(group) = item?
                && group.header.kind() == GroupKind::Top(label)
            {
                return Ok(Some(group));
            }
        }
        Ok(None)
    }
}

/// One thing at the top level of a plugin, after the header.
#[derive(Debug, Clone, Copy)]
pub enum TopItem<'a> {
    /// A top-level group.
    Group(GroupRef<'a>),
    /// A record outside any group. Valid plugins have none.
    Record(RecordRef<'a>),
}

/// Iterator over the top level of a plugin. See [`Plugin::top_level`].
#[derive(Debug, Clone)]
pub struct TopLevel<'a> {
    data: &'a [u8],
    pos: usize,
    done: bool,
}

impl<'a> Iterator for TopLevel<'a> {
    type Item = Result<TopItem<'a>, EsmError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.pos >= self.data.len() {
            return None;
        }
        match read_item(self.data, self.pos, self.data.len()) {
            Ok(Item::Group(header, end)) => {
                let group = GroupRef {
                    header,
                    data: self.data,
                    start: self.pos + HEADER_LEN,
                    end,
                };
                self.pos = end;
                Some(Ok(TopItem::Group(group)))
            }
            Ok(Item::Record(record, next)) => {
                self.pos = next;
                Some(Ok(TopItem::Record(record)))
            }
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }
}

/// A group whose contents have not been read yet.
#[derive(Debug, Clone, Copy)]
pub struct GroupRef<'a> {
    /// The group's header.
    pub header: GroupHeader,
    data: &'a [u8],
    start: usize,
    end: usize,
}

impl<'a> GroupRef<'a> {
    /// The group's contents, without its header.
    pub fn contents(&self) -> &'a [u8] {
        self.data.get(self.start..self.end).unwrap_or(&[])
    }

    /// Walks the group's contents. The group itself is the first entry of
    /// the walker's context and gets no start or end event.
    pub fn walk(&self) -> Walker<'a> {
        let context = Context {
            chain: vec![self.header],
        };
        Walker::new(self.data, self.start, self.end, context)
    }
}

/// The chain of groups enclosing the walker's current position, outermost
/// first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Context {
    chain: Vec<GroupHeader>,
}

impl Context {
    /// The enclosing groups, outermost first.
    pub fn chain(&self) -> &[GroupHeader] {
        &self.chain
    }

    /// Number of enclosing groups.
    pub fn depth(&self) -> usize {
        self.chain.len()
    }

    fn find<T>(&self, pick: impl Fn(GroupKind) -> Option<T>) -> Option<T> {
        self.chain.iter().rev().find_map(|g| pick(g.kind()))
    }

    /// The label of the enclosing top-level group.
    pub fn top(&self) -> Option<Sig> {
        match self.chain.first().map(GroupHeader::kind) {
            Some(GroupKind::Top(sig)) => Some(sig),
            _ => None,
        }
    }

    /// Whether this position is inside the top-level `CELL` group, which
    /// holds the interior cells and everything in them. Exterior cells are
    /// inside the `WRLD` group instead, under a worldspace.
    pub fn is_interior(&self) -> bool {
        self.top() == Some(sigs::CELL)
    }

    /// The worldspace whose children group encloses this position.
    pub fn worldspace(&self) -> Option<FormId> {
        self.find(|k| match k {
            GroupKind::WorldChildren(form) => Some(form),
            _ => None,
        })
    }

    /// The cell whose children group encloses this position. A `CELL`
    /// record is not inside its own children group, so this is `None` for
    /// the cell record itself.
    pub fn cell(&self) -> Option<FormId> {
        self.find(|k| match k {
            GroupKind::CellChildren(form) | GroupKind::CellChild(_, form) => Some(form),
            _ => None,
        })
    }

    /// Whether this position is in a cell's persistent, temporary or
    /// visible-when-distant children.
    pub fn cell_child_kind(&self) -> Option<CellChildKind> {
        self.find(|k| match k {
            GroupKind::CellChild(kind, _) => Some(kind),
            _ => None,
        })
    }

    /// The topic whose children group encloses this position.
    pub fn topic(&self) -> Option<FormId> {
        self.find(|k| match k {
            GroupKind::TopicChildren(form) => Some(form),
            _ => None,
        })
    }

    /// The enclosing exterior cell block, as `(x, y)` block coordinates.
    pub fn exterior_block(&self) -> Option<(i16, i16)> {
        self.find(|k| match k {
            GroupKind::ExteriorBlock { x, y } => Some((x, y)),
            _ => None,
        })
    }

    /// The enclosing exterior cell sub-block, as `(x, y)`.
    pub fn exterior_sub_block(&self) -> Option<(i16, i16)> {
        self.find(|k| match k {
            GroupKind::ExteriorSubBlock { x, y } => Some((x, y)),
            _ => None,
        })
    }

    /// The enclosing interior cell block number.
    pub fn interior_block(&self) -> Option<i32> {
        self.find(|k| match k {
            GroupKind::InteriorBlock(n) => Some(n),
            _ => None,
        })
    }

    /// The enclosing interior cell sub-block number.
    pub fn interior_sub_block(&self) -> Option<i32> {
        self.find(|k| match k {
            GroupKind::InteriorSubBlock(n) => Some(n),
            _ => None,
        })
    }
}

/// What the walker found next.
#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    /// A group starts. It is now the innermost entry of the context; call
    /// [`Walker::skip_group`] to step over its contents.
    GroupStart(GroupHeader),
    /// The group ends. It has left the context.
    GroupEnd(GroupHeader),
    /// A record. [`Walker::context`] gives the groups around it.
    Record(RecordRef<'a>),
}

/// A depth-first walk over groups and records.
///
/// ```
/// use fnv_esm::{Event, Plugin, fixture::PluginBuilder};
///
/// let bytes = PluginBuilder::new().build();
/// let plugin = Plugin::parse(&bytes)?;
/// let mut walker = plugin.walk();
/// while let Some(event) = walker.next_event()? {
///     if let Event::Record(record) = event {
///         let _cell = walker.context().cell();
///         let _typed = record.decode()?;
///     }
/// }
/// # Ok::<(), fnv_esm::EsmError>(())
/// ```
#[derive(Debug, Clone)]
pub struct Walker<'a> {
    data: &'a [u8],
    pos: usize,
    end: usize,
    /// End offsets of the groups this walker opened; parallel to the tail
    /// of `context.chain`.
    ends: Vec<usize>,
    context: Context,
    done: bool,
}

impl<'a> Walker<'a> {
    fn new(data: &'a [u8], pos: usize, end: usize, context: Context) -> Self {
        Self {
            data,
            pos,
            end: end.min(data.len()),
            ends: Vec::new(),
            context,
            done: false,
        }
    }

    /// The groups enclosing the event most recently returned.
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// Byte offset of the next header to be read.
    pub fn offset(&self) -> usize {
        self.pos
    }

    /// Steps over the rest of the innermost group this walker opened. Called
    /// right after its [`Event::GroupStart`], that is the whole group. The
    /// next event is that group's [`Event::GroupEnd`].
    pub fn skip_group(&mut self) {
        if let Some(&end) = self.ends.last() {
            self.pos = end;
        }
    }

    /// The next event, or `None` at the end. After an error the walk is
    /// over and this returns `None`.
    ///
    /// # Errors
    ///
    /// Framing errors: a header or payload that runs past its container, a
    /// group smaller than its header, or nesting past [`MAX_GROUP_DEPTH`].
    pub fn next_event(&mut self) -> Result<Option<Event<'a>>, EsmError> {
        if self.done {
            return Ok(None);
        }
        let result = self.step();
        if !matches!(result, Ok(Some(_))) {
            self.done = true;
        }
        result
    }

    /// The next record, stepping over group events.
    ///
    /// # Errors
    ///
    /// As [`Walker::next_event`].
    pub fn next_record(&mut self) -> Result<Option<RecordRef<'a>>, EsmError> {
        while let Some(event) = self.next_event()? {
            if let Event::Record(record) = event {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    fn step(&mut self) -> Result<Option<Event<'a>>, EsmError> {
        let limit = self.ends.last().copied().unwrap_or(self.end);
        if self.pos >= limit {
            // Only a group this walker opened is closed; the base context of
            // a group walk stays in place.
            if self.ends.pop().is_none() {
                return Ok(None);
            }
            return Ok(self.context.chain.pop().map(Event::GroupEnd));
        }
        match read_item(self.data, self.pos, limit)? {
            Item::Group(header, end) => {
                if self.context.chain.len() >= MAX_GROUP_DEPTH {
                    return Err(EsmError::GroupNestingTooDeep {
                        offset: self.pos,
                        sig: sigs::GRUP,
                        limit: MAX_GROUP_DEPTH,
                    });
                }
                self.pos += HEADER_LEN;
                self.ends.push(end);
                self.context.chain.push(header);
                Ok(Some(Event::GroupStart(header)))
            }
            Item::Record(record, next) => {
                self.pos = next;
                Ok(Some(Event::Record(record)))
            }
        }
    }
}

impl<'a> Iterator for Walker<'a> {
    type Item = Result<Event<'a>, EsmError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_event().transpose()
    }
}

/// A record whose data has not been read yet.
#[derive(Debug, Clone, Copy)]
pub struct RecordRef<'a> {
    /// The record's header.
    pub header: RecordHeader,
    /// Byte offset of the header in the file.
    pub offset: usize,
    payload: &'a [u8],
}

impl<'a> RecordRef<'a> {
    /// The record's signature.
    pub fn sig(&self) -> Sig {
        self.header.sig
    }

    /// The record's form id, as stored.
    pub fn form_id(&self) -> FormId {
        self.header.meta.form_id
    }

    /// The record flags.
    pub fn flags(&self) -> RecordFlags {
        self.header.meta.flags
    }

    /// The record data as stored: compressed when the flag says so.
    pub fn stored_data(&self) -> &'a [u8] {
        self.payload
    }

    /// The record data, decompressed if need be, with the default size cap
    /// [`DEFAULT_MAX_DECOMPRESSED`]. Uncompressed data is borrowed.
    ///
    /// # Errors
    ///
    /// The decompression errors of [`RecordRef::data_with_limit`].
    pub fn data(&self) -> Result<RecordData<'a>, EsmError> {
        self.data_with_limit(DEFAULT_MAX_DECOMPRESSED)
    }

    /// The record data, decompressed if need be, refusing a record that
    /// declares more than `max_decompressed` bytes.
    ///
    /// # Errors
    ///
    /// [`EsmError::DecompressedTooLarge`], [`EsmError::Decompress`],
    /// [`EsmError::DecompressedSizeMismatch`], or [`EsmError::Truncated`]
    /// when the 4-byte size prefix is missing. Each concerns this record
    /// only: the walk it came from is unaffected and can go on. A wrong
    /// Adler-32 checksum is not an error; see [`RecordData::checksum_ok`].
    pub fn data_with_limit(&self, max_decompressed: usize) -> Result<RecordData<'a>, EsmError> {
        let sig = self.header.sig;
        if !self.flags().is_compressed() {
            return Ok(RecordData {
                sig,
                bytes: Cow::Borrowed(self.payload),
                base_offset: self.offset.saturating_add(HEADER_LEN),
                offsets_move: true,
                checksum_ok: true,
                meta: self.header.meta,
            });
        }
        let (bytes, checksum_ok) = inflate(self.payload, max_decompressed, sig, self.offset)?;
        Ok(RecordData {
            sig,
            bytes: Cow::Owned(bytes),
            base_offset: self.offset,
            offsets_move: false,
            checksum_ok,
            meta: self.header.meta,
        })
    }

    /// The record as an owned header and subrecord list, nothing decoded.
    ///
    /// # Errors
    ///
    /// Decompression and subrecord framing errors.
    pub fn to_raw(&self) -> Result<RawRecord, EsmError> {
        self.data()?.to_raw()
    }

    /// Decodes the record through the decoder table. A record type with no
    /// decoder comes back as [`Record::Unknown`], with every subrecord kept.
    ///
    /// # Errors
    ///
    /// Decompression and subrecord framing errors. The contents of a
    /// subrecord never cause an error: one a decoder cannot read stays in
    /// the typed record's `unknown` list.
    pub fn decode(&self) -> Result<Record, EsmError> {
        self.data()?.decode()
    }
}

/// The uncompressed data of one record.
#[derive(Debug, Clone)]
pub struct RecordData<'a> {
    sig: Sig,
    bytes: Cow<'a, [u8]>,
    base_offset: usize,
    offsets_move: bool,
    checksum_ok: bool,
    meta: RecordMeta,
}

impl RecordData<'_> {
    /// Decodes the record through the decoder table, like
    /// [`RecordRef::decode`], without reading the data a second time. Use
    /// it to look at [`RecordData::checksum_ok`] before decoding.
    ///
    /// # Errors
    ///
    /// Subrecord framing errors.
    pub fn decode(&self) -> Result<Record, EsmError> {
        let subrecords = self.collect()?;
        Ok(decode_record(self.sig, self.meta, &subrecords))
    }

    /// The record as an owned header and subrecord list, like
    /// [`RecordRef::to_raw`].
    ///
    /// # Errors
    ///
    /// Subrecord framing errors.
    pub fn to_raw(&self) -> Result<RawRecord, EsmError> {
        let subrecords = self
            .subrecords()
            .map(|sub| sub.map(|s| s.to_raw()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RawRecord {
            sig: self.sig,
            meta: self.meta,
            subrecords,
        })
    }

    /// Whether the zlib stream's Adler-32 checksum matched the data.
    /// Always `true` for a record that was not compressed.
    ///
    /// A mismatch (or a missing checksum) is not an error: the deflate data
    /// reached its end marker and gave exactly the declared size, which is
    /// all the game asks of it, and shipped master files hold records whose
    /// checksum is wrong and whose data is sound.
    pub fn checksum_ok(&self) -> bool {
        self.checksum_ok
    }

    /// The uncompressed bytes: the record's subrecords.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Whether the bytes had to be decompressed.
    pub fn was_compressed(&self) -> bool {
        !self.offsets_move
    }

    /// Iterates the subrecords, resolving `XXXX` extended sizes.
    pub fn subrecords(&self) -> SubrecordIter<'_> {
        SubrecordIter::new(&self.bytes, self.sig, self.base_offset, self.offsets_move)
    }

    /// All the subrecords.
    ///
    /// # Errors
    ///
    /// Subrecord framing errors.
    pub fn collect(&self) -> Result<Vec<SubrecordRef<'_>>, EsmError> {
        self.subrecords().collect()
    }
}

/// The Adler-32 checksum of `bytes`, as a zlib stream carries it.
fn adler32(bytes: &[u8]) -> u32 {
    const MODULUS: u32 = 65_521;
    // 5552 is the most bytes whose sums cannot overflow a u32 before the
    // modulo is taken.
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MODULUS;
        b %= MODULUS;
    }
    (b << 16) | a
}

/// Decompresses a compressed record's payload: a `u32` decompressed size,
/// then a zlib stream. Returns the data and whether the stream's Adler-32
/// trailer matches it.
///
/// The deflate data must be whole: it has to reach its end marker and give
/// exactly the declared number of bytes. The checksum after it is compared
/// but not enforced, because shipped master files contain records whose
/// data is intact and whose checksum is not.
fn inflate(
    payload: &[u8],
    limit: usize,
    sig: Sig,
    offset: usize,
) -> Result<(Vec<u8>, bool), EsmError> {
    let Some((prefix, stream)) = payload.split_first_chunk::<4>() else {
        return Err(EsmError::Truncated {
            offset,
            sig,
            what: "compressed record size prefix",
            needed: 4,
            remaining: payload.len() as u64,
        });
    };
    let declared = u64::from(u32::from_le_bytes(*prefix));
    if declared > limit as u64 {
        return Err(EsmError::DecompressedTooLarge {
            offset,
            sig,
            declared,
            limit: limit as u64,
        });
    }
    let invalid = |message: String| EsmError::Decompress {
        offset,
        sig,
        message,
    };
    let too_long = EsmError::DecompressedSizeMismatch {
        offset,
        sig,
        declared,
        actual: declared + 1,
    };
    // The two-byte zlib header: deflate with a window of at most 32 KiB, a
    // header check that divides by 31, and no preset dictionary.
    let Some((&[method, flags], deflate)) = stream.split_first_chunk::<2>() else {
        return Err(invalid(
            "the stream is shorter than a zlib header".to_owned(),
        ));
    };
    let check = u16::from_be_bytes([method, flags]);
    if method & 0x0F != 8 || method >> 4 > 7 || !check.is_multiple_of(31) || flags & 0x20 != 0 {
        return Err(invalid(
            "the stream does not start with a zlib header".to_owned(),
        ));
    }
    // `declared <= limit`, so it fits a usize.
    let target = declared as usize;
    // The buffer grows with the bytes actually produced, never by what the
    // record merely declares, and never past one byte more than declared:
    // a stream that expands further is noticed without being expanded.
    let mut inflater = flate2::Decompress::new(false);
    let mut out: Vec<u8> = Vec::new();
    let mut consumed = 0usize;
    loop {
        if out.len() == out.capacity() {
            let room = target.saturating_add(1).saturating_sub(out.len());
            if room == 0 {
                return Err(too_long);
            }
            out.reserve_exact(room.min(out.len().max(INFLATE_CHUNK)));
        }
        let before = (inflater.total_in(), inflater.total_out());
        let input = deflate.get(consumed..).unwrap_or(&[]);
        let status = inflater
            .decompress_vec(input, &mut out, flate2::FlushDecompress::None)
            .map_err(|error| invalid(error.to_string()))?;
        consumed = usize::try_from(inflater.total_in()).unwrap_or(usize::MAX);
        if status == flate2::Status::StreamEnd {
            break;
        }
        let stalled = (inflater.total_in(), inflater.total_out()) == before;
        if stalled && out.len() < out.capacity() {
            // Room to write, nothing left to read, and no end marker.
            return Err(invalid("the stream ends before its end marker".to_owned()));
        }
    }
    if out.len() > target {
        return Err(too_long);
    }
    if out.len() < target {
        return Err(EsmError::DecompressedSizeMismatch {
            offset,
            sig,
            declared,
            actual: out.len() as u64,
        });
    }
    let trailer = deflate
        .get(consumed..)
        .and_then(|rest| rest.first_chunk::<4>())
        .map(|bytes| u32::from_be_bytes(*bytes));
    let checksum_ok = trailer == Some(adler32(&out));
    Ok((out, checksum_ok))
}

/// A header read at some position: a group and where it ends, or a record
/// and where the next header starts.
enum Item<'a> {
    Group(GroupHeader, usize),
    Record(RecordRef<'a>, usize),
}

/// Reads the record or group header at `pos`. `limit` is the end of the
/// enclosing container; nothing is read at or past it.
fn read_item(data: &[u8], pos: usize, limit: usize) -> Result<Item<'_>, EsmError> {
    let limit = limit.min(data.len());
    let rest = data.get(pos..limit).unwrap_or(&[]);
    let Some(head) = rest.first_chunk::<HEADER_LEN>() else {
        let mut sig = [b'?'; 4];
        if let Some(found) = rest.first_chunk::<4>() {
            sig = *found;
        }
        return Err(EsmError::Truncated {
            offset: pos,
            sig: Sig(sig),
            what: "record or group header",
            needed: HEADER_LEN as u64,
            remaining: rest.len() as u64,
        });
    };
    let mut rd = Rd::new(head);
    // The header is 24 bytes, so none of these reads can fail; the fallbacks
    // only keep the code free of panics.
    let sig = Sig(rd.arr().unwrap_or_default());
    let size = rd.u32().unwrap_or_default();
    if sig == sigs::GRUP {
        let label = rd.arr().unwrap_or_default();
        let group_type = rd.i32().unwrap_or_default();
        let stamp = rd.u32().unwrap_or_default();
        let unknown = rd.u32().unwrap_or_default();
        if (size as usize) < HEADER_LEN {
            return Err(EsmError::GroupSizeTooSmall {
                offset: pos,
                sig,
                size,
            });
        }
        if u64::from(size) > rest.len() as u64 {
            return Err(EsmError::Truncated {
                offset: pos,
                sig,
                what: "group contents",
                needed: u64::from(size),
                remaining: rest.len() as u64,
            });
        }
        let header = GroupHeader {
            offset: pos,
            size,
            label,
            group_type,
            stamp,
            unknown,
        };
        return Ok(Item::Group(header, pos + size as usize));
    }
    let meta = RecordMeta {
        flags: RecordFlags(rd.u32().unwrap_or_default()),
        form_id: FormId(rd.u32().unwrap_or_default()),
        revision: rd.u32().unwrap_or_default(),
        form_version: rd.u16().unwrap_or_default(),
        unknown: rd.u16().unwrap_or_default(),
    };
    let body = &rest[HEADER_LEN..];
    if u64::from(size) > body.len() as u64 {
        return Err(EsmError::Truncated {
            offset: pos,
            sig,
            what: "record data",
            needed: u64::from(size),
            remaining: body.len() as u64,
        });
    }
    let payload = &body[..size as usize];
    let record = RecordRef {
        header: RecordHeader {
            sig,
            data_size: size,
            meta,
        },
        offset: pos,
        payload,
    };
    Ok(Item::Record(record, pos + HEADER_LEN + size as usize))
}
