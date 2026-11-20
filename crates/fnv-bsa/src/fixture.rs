//! Writer for synthetic version 104 archives.
//!
//! [`BsaBuilder`] turns in-memory files into a complete archive: hashes are
//! computed, folder and file records are sorted by hash, and every offset
//! and total in the header is filled in. Output is deterministic: the same
//! files and options give the same bytes, whatever order they were added in.

use std::collections::BTreeMap;
use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::hash::{archive_bytes, archive_string, hash_file_bytes, hash_folder_bytes};
use crate::{SIZE_COMPRESSION_TOGGLE, SIZE_MASK, flags};

#[derive(Debug, Clone)]
struct FileSpec {
    data: Vec<u8>,
    /// `None` follows the archive default.
    compressed: Option<bool>,
}

/// Builds a version 104 archive in memory.
///
/// ```
/// use fnv_bsa::{BsaArchive, fixture::BsaBuilder};
///
/// let bytes = BsaBuilder::new()
///     .compressed(true)
///     .add("textures/clutter/crate.dds", b"pixels")
///     .build();
/// let archive = BsaArchive::from_bytes(bytes).unwrap();
/// let entry = archive.find("Textures\\Clutter\\Crate.dds").unwrap();
/// assert_eq!(archive.read(entry).unwrap(), b"pixels");
/// ```
#[derive(Debug, Clone, Default)]
pub struct BsaBuilder {
    compressed: bool,
    embed_names: bool,
    files: BTreeMap<String, FileSpec>,
}

struct OutFile {
    name: Vec<u8>,
    hash: u64,
    size_field: u32,
    payload: Vec<u8>,
}

struct OutFolder {
    name: Vec<u8>,
    hash: u64,
    files: Vec<OutFile>,
}

impl BsaBuilder {
    /// An empty, uncompressed archive without embedded names.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the archive's compression default.
    pub fn compressed(&mut self, yes: bool) -> &mut Self {
        self.compressed = yes;
        self
    }

    /// Sets whether each entry's data starts with its full path, the way
    /// texture archives are written.
    pub fn embed_names(&mut self, yes: bool) -> &mut Self {
        self.embed_names = yes;
        self
    }

    /// Adds a file that follows the archive's compression default. The path
    /// is stored lowercase with `\` separators; adding the same path again
    /// replaces the earlier file.
    pub fn add(&mut self, path: &str, bytes: &[u8]) -> &mut Self {
        self.insert(path, bytes, None)
    }

    /// Adds a file with its own compression setting. When it differs from
    /// the archive default, the entry's size field carries the override bit.
    pub fn add_with_compression(
        &mut self,
        path: &str,
        bytes: &[u8],
        compressed: bool,
    ) -> &mut Self {
        self.insert(path, bytes, Some(compressed))
    }

    fn insert(&mut self, path: &str, bytes: &[u8], compressed: Option<bool>) -> &mut Self {
        let key = archive_string(&archive_bytes(path));
        let key = key.trim_matches('\\').to_owned();
        self.files.insert(
            key,
            FileSpec {
                data: bytes.to_vec(),
                compressed,
            },
        );
        self
    }

    /// Writes the archive.
    ///
    /// # Panics
    ///
    /// Panics when the input cannot be represented: a folder path longer
    /// than 254 bytes, a full path longer than 255 bytes while names are
    /// embedded, an empty file name, an entry of 1 GiB or more, or an
    /// archive of 4 GiB or more.
    pub fn build(&self) -> Vec<u8> {
        // Group by folder. A path without a separator is in the root folder,
        // whose name is empty.
        let mut by_folder: BTreeMap<Vec<u8>, Vec<OutFile>> = BTreeMap::new();
        for (path, spec) in &self.files {
            let full = archive_bytes(path);
            let (folder, name) = match full.iter().rposition(|&b| b == b'\\') {
                Some(sep) => (full[..sep].to_vec(), full[sep + 1..].to_vec()),
                None => (Vec::new(), full.clone()),
            };
            assert!(!name.is_empty(), "empty file name in {path:?}");
            assert!(folder.len() <= 254, "folder path too long in {path:?}");

            let compressed = spec.compressed.unwrap_or(self.compressed);
            let mut payload = Vec::new();
            if self.embed_names {
                let len = u8::try_from(full.len())
                    .unwrap_or_else(|_| panic!("path too long to embed: {path:?}"));
                payload.push(len);
                payload.extend_from_slice(&full);
            }
            if compressed {
                let original = u32::try_from(spec.data.len())
                    .unwrap_or_else(|_| panic!("entry too large: {path:?}"));
                payload.extend_from_slice(&original.to_le_bytes());
                let mut encoder = ZlibEncoder::new(payload, Compression::default());
                encoder
                    .write_all(&spec.data)
                    .expect("writing to a Vec cannot fail");
                payload = encoder.finish().expect("writing to a Vec cannot fail");
            } else {
                payload.extend_from_slice(&spec.data);
            }
            let stored = u32::try_from(payload.len())
                .ok()
                .filter(|&n| n <= SIZE_MASK)
                .unwrap_or_else(|| panic!("entry too large: {path:?}"));
            let toggle = if compressed != self.compressed {
                SIZE_COMPRESSION_TOGGLE
            } else {
                0
            };
            by_folder.entry(folder).or_default().push(OutFile {
                hash: hash_file_bytes(&name),
                name,
                size_field: stored | toggle,
                payload,
            });
        }

        let mut folders: Vec<OutFolder> = by_folder
            .into_iter()
            .map(|(name, mut files)| {
                files.sort_by(|a, b| (a.hash, &a.name).cmp(&(b.hash, &b.name)));
                OutFolder {
                    hash: hash_folder_bytes(&name),
                    name,
                    files,
                }
            })
            .collect();
        folders.sort_by(|a, b| (a.hash, &a.name).cmp(&(b.hash, &b.name)));

        let file_count: usize = folders.iter().map(|f| f.files.len()).sum();
        let total_folder_names: usize = folders.iter().map(|f| f.name.len() + 1).sum();
        let total_file_names: usize = folders
            .iter()
            .flat_map(|f| &f.files)
            .map(|f| f.name.len() + 1)
            .sum();
        let records_start = 36 + 16 * folders.len();
        // Each block is a length byte, the name with its terminator, and the
        // folder's file records.
        let blocks_len = folders.len() + total_folder_names + 16 * file_count;
        let data_start = records_start + blocks_len + total_file_names;
        let data_len: usize = folders
            .iter()
            .flat_map(|f| &f.files)
            .map(|f| f.payload.len())
            .sum();
        let u32_of = |n: usize| u32::try_from(n).expect("archive exceeds 4 GiB");
        u32_of(data_start + data_len + total_file_names);

        let mut archive_flags = flags::DIR_NAMES | flags::FILE_NAMES;
        if self.compressed {
            archive_flags |= flags::COMPRESSED;
        }
        if self.embed_names {
            archive_flags |= flags::EMBED_NAMES;
        }
        let file_flags = folders
            .iter()
            .flat_map(|f| &f.files)
            .fold(0u32, |acc, f| acc | file_type_flag(&f.name));

        let mut out = Vec::with_capacity(data_start + data_len);
        out.extend_from_slice(b"BSA\0");
        for field in [
            104,
            36,
            archive_flags,
            u32_of(folders.len()),
            u32_of(file_count),
            u32_of(total_folder_names),
            u32_of(total_file_names),
            file_flags,
        ] {
            out.extend_from_slice(&field.to_le_bytes());
        }

        // Folder records. The offset field is the position of the folder's
        // block plus the length of the file name block.
        let mut block_offset = records_start;
        for folder in &folders {
            out.extend_from_slice(&folder.hash.to_le_bytes());
            out.extend_from_slice(&u32_of(folder.files.len()).to_le_bytes());
            out.extend_from_slice(&u32_of(block_offset + total_file_names).to_le_bytes());
            block_offset += 1 + folder.name.len() + 1 + 16 * folder.files.len();
        }

        // File record blocks.
        let mut data_offset = data_start;
        for folder in &folders {
            out.push(folder.name.len() as u8 + 1);
            out.extend_from_slice(&folder.name);
            out.push(0);
            for file in &folder.files {
                out.extend_from_slice(&file.hash.to_le_bytes());
                out.extend_from_slice(&file.size_field.to_le_bytes());
                out.extend_from_slice(&u32_of(data_offset).to_le_bytes());
                data_offset += file.payload.len();
            }
        }

        // File names, then data, both in record order.
        for file in folders.iter().flat_map(|f| &f.files) {
            out.extend_from_slice(&file.name);
            out.push(0);
        }
        debug_assert_eq!(out.len(), data_start);
        for file in folders.iter().flat_map(|f| &f.files) {
            out.extend_from_slice(&file.payload);
        }
        out
    }
}

/// The header's file-type bit for a file name, by extension.
fn file_type_flag(name: &[u8]) -> u32 {
    let ext = match name.iter().rposition(|&b| b == b'.') {
        Some(dot) => &name[dot + 1..],
        None => &[][..],
    };
    match ext {
        b"nif" => 0x1,
        b"dds" => 0x2,
        b"xml" => 0x4,
        b"wav" => 0x8,
        b"mp3" | b"ogg" => 0x10,
        b"txt" | b"html" | b"bat" | b"scc" => 0x20,
        b"spt" => 0x40,
        b"tex" | b"fnt" => 0x80,
        _ => 0x100,
    }
}
