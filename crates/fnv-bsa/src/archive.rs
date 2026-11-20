use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use flate2::read::ZlibDecoder;

use crate::error::BsaError;
use crate::hash::{archive_bytes, archive_string, hash_file_bytes, hash_folder_bytes};
use crate::{SIZE_COMPRESSION_TOGGLE, SIZE_MASK, flags};

const MAGIC: [u8; 4] = *b"BSA\0";
const VERSION: u32 = 104;
const HEADER_LEN: u64 = 36;
const RECORD_LEN: u64 = 16;

/// Default cap on the size of one entry, stored or decompressed: 1 GiB.
pub const DEFAULT_MAX_ENTRY_SIZE: u64 = 1 << 30;

/// Table bytes are read from a file in chunks of this size.
const CHUNK: usize = 64 * 1024;

/// A decompression buffer starts no larger than this, however large the
/// entry claims to be, and grows only as the zlib stream really produces
/// bytes.
const PREALLOC_LIMIT: u64 = 16 << 20;

/// One file in an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BsaEntry {
    /// Full path, lowercase, `\` separators. When the archive stores no
    /// folder names or no file names, the missing part is `#` followed by
    /// the 16 hex digits of its hash.
    pub path: String,
    /// Bytes the entry occupies in the archive, including the embedded name
    /// and the 4-byte original size of a compressed entry.
    pub stored_size: u32,
    /// Whether the payload is a zlib stream.
    pub compressed: bool,
    /// Offset of the stored bytes from the start of the archive.
    pub offset: u64,
    /// Hash stored in the folder record.
    pub folder_hash: u64,
    /// Hash stored in the file record.
    pub file_hash: u64,
}

/// Which kind of record a [`HashMismatch`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashKind {
    Folder,
    File,
}

/// A record whose stored hash differs from the hash of its stored name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashMismatch {
    pub kind: HashKind,
    /// The folder path, or the full path of the file.
    pub path: String,
    pub stored: u64,
    pub computed: u64,
}

#[derive(Debug)]
enum Source {
    Memory(Vec<u8>),
    File { file: Mutex<File>, len: u64 },
}

impl Source {
    fn len(&self) -> u64 {
        match self {
            Source::Memory(bytes) => bytes.len() as u64,
            Source::File { len, .. } => *len,
        }
    }

    /// Fills `buf` from `offset`. Callers check the range against `len`
    /// first; a range that is out of bounds anyway is an error, not a panic.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        match self {
            Source::Memory(bytes) => {
                let slice = usize::try_from(offset)
                    .ok()
                    .and_then(|start| Some(start..start.checked_add(buf.len())?))
                    .and_then(|range| bytes.get(range))
                    .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
                buf.copy_from_slice(slice);
                Ok(())
            }
            Source::File { file, .. } => {
                let mut file = file.lock().unwrap_or_else(|poison| poison.into_inner());
                file.seek(SeekFrom::Start(offset))?;
                file.read_exact(buf)
            }
        }
    }
}

/// Sequential, bounds-checked reader over the table region of a source.
struct TableReader<'a> {
    source: &'a Source,
    len: u64,
    pos: u64,
    buf: Vec<u8>,
    buf_start: u64,
}

impl<'a> TableReader<'a> {
    fn new(source: &'a Source) -> Self {
        Self {
            source,
            len: source.len(),
            pos: 0,
            buf: Vec::new(),
            buf_start: 0,
        }
    }

    fn remaining(&self) -> u64 {
        self.len - self.pos
    }

    /// Returns the next `n` bytes, `n <= CHUNK`.
    fn take(&mut self, n: usize, what: &'static str) -> Result<&[u8], BsaError> {
        let available = self.remaining();
        if n as u64 > available || n > CHUNK {
            return Err(BsaError::Truncated {
                offset: self.pos,
                what,
                needed: n as u64,
                available,
            });
        }
        let buffered = self.pos >= self.buf_start
            && self.pos + n as u64 <= self.buf_start + self.buf.len() as u64;
        if !buffered {
            let size = available.min(CHUNK as u64) as usize;
            self.buf.clear();
            self.buf.resize(size, 0);
            self.buf_start = self.pos;
            if let Err(source) = self.source.read_at(self.pos, &mut self.buf) {
                self.buf.clear();
                return Err(BsaError::Io {
                    offset: self.pos,
                    source,
                });
            }
        }
        let start = (self.pos - self.buf_start) as usize;
        self.pos += n as u64;
        Ok(&self.buf[start..start + n])
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], BsaError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N, what)?);
        Ok(out)
    }

    fn u8(&mut self, what: &'static str) -> Result<u8, BsaError> {
        Ok(self.array::<1>(what)?[0])
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, BsaError> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }

    fn u64(&mut self, what: &'static str) -> Result<u64, BsaError> {
        Ok(u64::from_le_bytes(self.array(what)?))
    }

    /// Checks that `count` records of `RECORD_LEN` bytes fit in what remains.
    fn check_records(&self, count: u64, at: u64, what: &'static str) -> Result<(), BsaError> {
        let needed = count.saturating_mul(RECORD_LEN);
        let available = self.remaining();
        if needed > available {
            return Err(BsaError::CountTooLarge {
                offset: at,
                what,
                count,
                needed,
                available,
            });
        }
        Ok(())
    }

    /// Reads a zero-terminated string that must end before `end`.
    fn cstring(&mut self, end: u64, what: &'static str) -> Result<Vec<u8>, BsaError> {
        let start = self.pos;
        let mut out = Vec::new();
        loop {
            if self.pos >= end {
                return Err(BsaError::UnterminatedName {
                    offset: start,
                    what,
                });
            }
            match self.u8(what)? {
                0 => return Ok(out),
                b => out.push(b),
            }
        }
    }
}

#[derive(Debug)]
struct Folder {
    /// Stored name, or `None` when the archive has no folder names.
    name: Option<String>,
    hash: u64,
}

/// A version 104 BSA archive, opened for random access.
#[derive(Debug)]
pub struct BsaArchive {
    source: Source,
    archive_flags: u32,
    file_flags: u32,
    /// Folder records in table order.
    folders: Vec<Folder>,
    /// Entries sorted by path.
    entries: Vec<BsaEntry>,
    max_entry_size: u64,
}

impl BsaArchive {
    /// Opens an archive file. Only the header, the records and the names are
    /// read; entry data stays on disk until [`BsaArchive::read`] asks for it.
    pub fn open(path: &Path) -> Result<Self, BsaError> {
        let io_error = |source| BsaError::Io { offset: 0, source };
        let file = File::open(path).map_err(io_error)?;
        let len = file.metadata().map_err(io_error)?.len();
        Self::parse(Source::File {
            file: Mutex::new(file),
            len,
        })
    }

    /// Opens an archive held in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, BsaError> {
        Self::parse(Source::Memory(bytes))
    }

    fn parse(source: Source) -> Result<Self, BsaError> {
        let archive_len = source.len();
        let mut r = TableReader::new(&source);

        if archive_len < HEADER_LEN {
            return Err(BsaError::Truncated {
                offset: 0,
                what: "header",
                needed: HEADER_LEN,
                available: archive_len,
            });
        }
        let magic: [u8; 4] = r.array("header")?;
        if magic != MAGIC {
            return Err(BsaError::BadMagic { found: magic });
        }
        let version = r.u32("header")?;
        if version != VERSION {
            return Err(BsaError::UnsupportedVersion { version });
        }
        let records_offset = r.u32("header")?;
        let archive_flags = r.u32("header")?;
        if archive_flags & flags::BIG_ENDIAN != 0 {
            return Err(BsaError::BigEndian {
                flags: archive_flags,
            });
        }
        let folder_count = r.u32("header")?;
        let file_count = r.u32("header")?;
        let _total_folder_name_len = r.u32("header")?;
        let total_file_name_len = r.u32("header")?;
        let file_flags = r.u32("header")?;

        if u64::from(records_offset) < HEADER_LEN || u64::from(records_offset) > archive_len {
            return Err(BsaError::BadHeaderOffset {
                value: records_offset,
                archive_len,
            });
        }
        r.pos = u64::from(records_offset);

        let has_dir_names = archive_flags & flags::DIR_NAMES != 0;
        let has_file_names = archive_flags & flags::FILE_NAMES != 0;
        let default_compressed = archive_flags & flags::COMPRESSED != 0;

        // Both record tables must fit in the file before anything is sized
        // from their counts.
        r.check_records(u64::from(folder_count), 16, "folder record")?;
        r.check_records(
            u64::from(folder_count) + u64::from(file_count),
            20,
            "file record",
        )?;

        struct RawFolder {
            hash: u64,
            count: u32,
        }
        let mut raw_folders = Vec::with_capacity(folder_count as usize);
        for _ in 0..folder_count {
            let hash = r.u64("folder record")?;
            let count = r.u32("folder record")?;
            // The record's offset field is not needed: file record blocks
            // follow the folder records in the same order.
            let _offset = r.u32("folder record")?;
            raw_folders.push(RawFolder { hash, count });
        }

        struct RawFile {
            folder: usize,
            hash: u64,
            size: u32,
            offset: u32,
        }
        let mut folders = Vec::with_capacity(raw_folders.len());
        let mut raw_files: Vec<RawFile> = Vec::with_capacity(file_count as usize);
        let mut seen: u64 = 0;
        for (index, raw) in raw_folders.iter().enumerate() {
            let block_start = r.pos;
            let name = if has_dir_names {
                let n = usize::from(r.u8("folder name length")?);
                let bytes = r.take(n, "folder name")?;
                // The length counts a terminating zero.
                let name = bytes.split(|&b| b == 0).next().unwrap_or(&[]);
                Some(archive_string(name))
            } else {
                None
            };
            seen += u64::from(raw.count);
            if seen > u64::from(file_count) {
                return Err(BsaError::FileCountMismatch {
                    offset: block_start,
                    header: file_count,
                    folders: seen,
                });
            }
            r.check_records(u64::from(raw.count), block_start, "file record")?;
            for _ in 0..raw.count {
                let hash = r.u64("file record")?;
                let size = r.u32("file record")?;
                let offset = r.u32("file record")?;
                raw_files.push(RawFile {
                    folder: index,
                    hash,
                    size,
                    offset,
                });
            }
            folders.push(Folder {
                name,
                hash: raw.hash,
            });
        }
        if seen != u64::from(file_count) {
            return Err(BsaError::FileCountMismatch {
                offset: r.pos,
                header: file_count,
                folders: seen,
            });
        }

        let names_end = if has_file_names {
            let needed = u64::from(total_file_name_len);
            if needed > r.remaining() {
                return Err(BsaError::Truncated {
                    offset: r.pos,
                    what: "file name block",
                    needed,
                    available: r.remaining(),
                });
            }
            r.pos + needed
        } else {
            r.pos
        };

        let mut entries = Vec::with_capacity(raw_files.len());
        for raw in &raw_files {
            let folder = &folders[raw.folder];
            let mut path = match &folder.name {
                Some(name) => name.clone(),
                None => format!("#{:016x}", folder.hash),
            };
            if !path.is_empty() {
                path.push('\\');
            }
            if has_file_names {
                path.push_str(&archive_string(&r.cstring(names_end, "file name")?));
            } else {
                path.push_str(&format!("#{:016x}", raw.hash));
            }
            let toggled = raw.size & SIZE_COMPRESSION_TOGGLE != 0;
            entries.push(BsaEntry {
                path,
                stored_size: raw.size & SIZE_MASK,
                compressed: default_compressed != toggled,
                offset: u64::from(raw.offset),
                folder_hash: folder.hash,
                file_hash: raw.hash,
            });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(Self {
            source,
            archive_flags,
            file_flags,
            folders,
            entries,
            max_entry_size: DEFAULT_MAX_ENTRY_SIZE,
        })
    }

    /// Sets the largest entry [`BsaArchive::read`] will return, in bytes.
    /// The limit applies to the stored size and to the decompressed size.
    pub fn with_max_entry_size(mut self, limit: u64) -> Self {
        self.max_entry_size = limit;
        self
    }

    /// Sets the largest entry [`BsaArchive::read`] will return, in bytes.
    pub fn set_max_entry_size(&mut self, limit: u64) {
        self.max_entry_size = limit;
    }

    /// The current per-entry size limit in bytes.
    pub fn max_entry_size(&self) -> u64 {
        self.max_entry_size
    }

    /// The archive flags from the header (see [`crate::flags`]).
    pub fn archive_flags(&self) -> u32 {
        self.archive_flags
    }

    /// The file-type flags from the header.
    pub fn file_flags(&self) -> u32 {
        self.file_flags
    }

    /// Whether each entry's data starts with its embedded path.
    pub fn embeds_names(&self) -> bool {
        self.archive_flags & flags::EMBED_NAMES != 0
    }

    /// Size of the archive in bytes.
    pub fn archive_len(&self) -> u64 {
        self.source.len()
    }

    /// All entries, sorted by path.
    pub fn entries(&self) -> &[BsaEntry] {
        &self.entries
    }

    /// Finds an entry by path. Case does not matter, and `/` and `\` are
    /// both accepted as separators. Leading separators are ignored.
    ///
    /// When the archive stores no names, the entry is found by the hashes of
    /// the path instead.
    pub fn find(&self, path: &str) -> Option<&BsaEntry> {
        let key = archive_string(&archive_bytes(path));
        let key = key.trim_start_matches('\\');
        let first = self.entries.partition_point(|e| e.path.as_str() < key);
        if let Some(entry) = self.entries.get(first).filter(|e| e.path == key) {
            return Some(entry);
        }
        let named = flags::DIR_NAMES | flags::FILE_NAMES;
        if self.archive_flags & named == named {
            return None;
        }
        let (folder_hash, file_hash) = crate::hash_path(key);
        self.entries
            .iter()
            .find(|e| e.folder_hash == folder_hash && e.file_hash == file_hash)
    }

    /// Reads one entry and returns its payload: the embedded name is
    /// skipped and compressed data is inflated.
    pub fn read(&self, entry: &BsaEntry) -> Result<Vec<u8>, BsaError> {
        let archive_len = self.source.len();
        let size = u64::from(entry.stored_size);
        let in_bounds = entry
            .offset
            .checked_add(size)
            .is_some_and(|end| end <= archive_len);
        if !in_bounds {
            return Err(BsaError::EntryOutOfBounds {
                path: entry.path.clone(),
                offset: entry.offset,
                size,
                archive_len,
            });
        }
        if size > self.max_entry_size {
            return Err(BsaError::EntryTooLarge {
                path: entry.path.clone(),
                offset: entry.offset,
                size,
                limit: self.max_entry_size,
            });
        }
        // `size` is at most 30 bits and no more than the archive holds.
        let mut stored = vec![0u8; size as usize];
        self.source
            .read_at(entry.offset, &mut stored)
            .map_err(|source| BsaError::Io {
                offset: entry.offset,
                source,
            })?;

        let mut start = 0usize;
        if self.embeds_names() {
            let name_len = stored.first().map(|&n| usize::from(n));
            match name_len {
                Some(n) if n < stored.len() => start = 1 + n,
                _ => {
                    return Err(BsaError::BadEntryData {
                        path: entry.path.clone(),
                        offset: entry.offset,
                        what: "embedded name",
                    });
                }
            }
        }

        if !entry.compressed {
            stored.drain(..start);
            return Ok(stored);
        }

        let body = &stored[start..];
        let body_offset = entry.offset + start as u64;
        if body.is_empty() {
            return Ok(Vec::new());
        }
        let Some((size_bytes, stream)) = body.split_first_chunk::<4>() else {
            return Err(BsaError::BadEntryData {
                path: entry.path.clone(),
                offset: body_offset,
                what: "original size of a compressed entry",
            });
        };
        let original = u64::from(u32::from_le_bytes(*size_bytes));
        if original > self.max_entry_size {
            return Err(BsaError::EntryTooLarge {
                path: entry.path.clone(),
                offset: body_offset,
                size: original,
                limit: self.max_entry_size,
            });
        }
        let mut out = Vec::with_capacity(original.min(PREALLOC_LIMIT) as usize);
        // One byte past the declared size is enough to detect a stream that
        // is longer than it claims, and bounds the output either way.
        ZlibDecoder::new(stream)
            .take(original + 1)
            .read_to_end(&mut out)
            .map_err(|source| BsaError::Decompress {
                path: entry.path.clone(),
                offset: body_offset + 4,
                source,
            })?;
        if out.len() as u64 != original {
            return Err(BsaError::SizeMismatch {
                path: entry.path.clone(),
                offset: body_offset,
                expected: original,
                actual: out.len() as u64,
            });
        }
        Ok(out)
    }

    /// Recomputes every name hash and returns the records whose stored hash
    /// differs. Folder mismatches come first, in table order, then file
    /// mismatches in path order. Records without a stored name are skipped.
    pub fn verify_hashes(&self) -> Vec<HashMismatch> {
        let mut out = Vec::new();
        for folder in &self.folders {
            let Some(name) = &folder.name else { continue };
            let computed = hash_folder_bytes(&archive_bytes(name));
            if computed != folder.hash {
                out.push(HashMismatch {
                    kind: HashKind::Folder,
                    path: name.clone(),
                    stored: folder.hash,
                    computed,
                });
            }
        }
        if self.archive_flags & flags::FILE_NAMES != 0 {
            for entry in &self.entries {
                let name = entry.path.rsplit('\\').next().unwrap_or(&entry.path);
                let computed = hash_file_bytes(&archive_bytes(name));
                if computed != entry.file_hash {
                    out.push(HashMismatch {
                        kind: HashKind::File,
                        path: entry.path.clone(),
                        stored: entry.file_hash,
                        computed,
                    });
                }
            }
        }
        out
    }
}
