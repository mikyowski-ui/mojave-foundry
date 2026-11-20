//! The NIF header: version, block type table, block sizes, string table.

use crate::error::{Location, NifError};
use crate::reader::Reader;

/// The only file version this crate reads: 20.2.0.7.
pub const VERSION: u32 = 0x1402_0007;
/// The only user version this crate reads.
pub const USER_VERSION: u32 = 11;
/// The only Bethesda stream version this crate reads.
pub const STREAM_VERSION: u32 = 34;
/// The text every supported file starts with, before the newline.
pub const HEADER_LINE: &str = "Gamebryo File Format, Version 20.2.0.7";

const HEADER_PREFIX: &[u8] = b"Gamebryo File Format, Version ";
const OLD_HEADER_PREFIX: &[u8] = b"NetImmerse File Format, Version ";
const MAX_HEADER_LINE: usize = 96;

/// Everything stored before the first block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The header line, without its newline.
    pub header_line: String,
    /// File version, `0x14020007`.
    pub version: u32,
    /// Endianness byte; always 1 (little endian) in a file that parsed.
    pub endian: u8,
    /// User version, 11.
    pub user_version: u32,
    /// Bethesda stream version, 34.
    pub stream_version: u32,
    /// The three export strings: author, process script, export script.
    pub export_strings: [String; 3],
    /// Block type names; `block_type_index` points into this.
    pub block_types: Vec<String>,
    /// For each block, the index of its type name.
    pub block_type_index: Vec<u16>,
    /// For each block, its size in bytes.
    pub block_sizes: Vec<u32>,
    /// The string table that block names index into.
    pub strings: Vec<String>,
    /// The stored maximum string length (informational; not trusted).
    pub max_string_length: u32,
    /// Group sizes.
    pub groups: Vec<u32>,
}

impl Header {
    /// Number of blocks in the file.
    pub fn num_blocks(&self) -> u32 {
        // The parser read this count as a `u32`.
        self.block_sizes.len() as u32
    }
}

fn dotted(version: u32) -> String {
    let b = version.to_be_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

/// Stream versions [`crate::parse_animation`] also accepts: older
/// animation files of the same file and user version. Their blocks are
/// still checked against their declared sizes.
pub const ANIMATION_STREAM_VERSIONS: std::ops::RangeInclusive<u32> = 14..=STREAM_VERSION;

/// Parses the header, accepting any stream version in `streams`, and
/// returns it with the offset of the first block.
pub(crate) fn parse_header_with(
    data: &[u8],
    streams: std::ops::RangeInclusive<u32>,
) -> Result<(Header, usize), NifError> {
    let window = &data[..data.len().min(MAX_HEADER_LINE)];
    let old_format = window.starts_with(OLD_HEADER_PREFIX);
    if !window.starts_with(HEADER_PREFIX) && !old_format {
        return Err(NifError::NotNif {
            offset: 0,
            reason: "missing the \"Gamebryo File Format\" header line",
        });
    }
    let Some(newline) = window.iter().position(|&b| b == b'\n') else {
        return Err(NifError::NotNif {
            offset: 0,
            reason: "header line has no terminating newline",
        });
    };
    let header_line = crate::reader::latin1(&data[..newline]);
    if old_format {
        let text = &header_line[OLD_HEADER_PREFIX.len()..];
        return Err(NifError::UnsupportedVersion {
            offset: 0,
            version: text.to_owned(),
            user_version: 0,
            stream_version: 0,
        });
    }

    let loc = Location::Header;
    let mut r = Reader::new(&data[newline + 1..], newline + 1, &loc, &[], 0);

    let at = r.offset();
    let version = r.u32("version")?;
    if version != VERSION {
        return Err(NifError::UnsupportedVersion {
            offset: at,
            version: dotted(version),
            user_version: 0,
            stream_version: 0,
        });
    }
    let at = r.offset();
    let endian = r.u8("endianness")?;
    if endian != 1 {
        // 0 is big endian, which no supported file uses.
        return Err(r.bad_value(at, "endianness (only little endian, 1)", i64::from(endian)));
    }
    let at = r.offset();
    let user_version = r.u32("user version")?;
    let num_blocks_at = r.offset();
    let num_blocks = r.u32("block count")?;
    let stream_version = if user_version >= 10 {
        r.u32("stream version")?
    } else {
        0
    };
    if user_version != USER_VERSION || !streams.contains(&stream_version) {
        return Err(NifError::UnsupportedVersion {
            offset: at,
            version: dotted(version),
            user_version,
            stream_version,
        });
    }
    let export_strings = [
        r.short_string("export string (author)")?,
        r.short_string("export string (process script)")?,
        r.short_string("export string (export script)")?,
    ];

    let num_types = r.count_u16(4, "block type name")?;
    let mut block_types = Vec::with_capacity(num_types);
    for _ in 0..num_types {
        block_types.push(r.sized_string("block type name")?);
    }

    // Each block costs two bytes of type index and four of size, here in the
    // header alone, which bounds the block count before any allocation.
    let num_blocks = r.check_count(u64::from(num_blocks), 6, "block", num_blocks_at)?;
    let mut block_type_index = Vec::with_capacity(num_blocks);
    for _ in 0..num_blocks {
        let at = r.offset();
        let index = r.u16("block type index")?;
        if usize::from(index) >= block_types.len() {
            return Err(r.bad_value(at, "block type index", i64::from(index)));
        }
        block_type_index.push(index);
    }
    let block_sizes = r.array(num_blocks, 4, "block size", Reader::u32)?;

    let num_strings_at = r.offset();
    let num_strings = r.u32("string count")?;
    let max_string_length = r.u32("maximum string length")?;
    let num_strings = r.check_count(u64::from(num_strings), 4, "string", num_strings_at)?;
    let mut strings = Vec::with_capacity(num_strings);
    for _ in 0..num_strings {
        strings.push(r.sized_string("string table entry")?);
    }

    let num_groups = r.count_u32(4, "group")?;
    let groups = r.array(num_groups, 4, "group", Reader::u32)?;

    let end = r.offset();
    Ok((
        Header {
            header_line,
            version,
            endian,
            user_version,
            stream_version,
            export_strings,
            block_types,
            block_type_index,
            block_sizes,
            strings,
            max_string_length,
            groups,
        },
        end,
    ))
}
