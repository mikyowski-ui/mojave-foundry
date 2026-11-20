//! Tests on synthetic archives only. Nothing here reads a game file.

use std::io::Write;

use crate::fixture::BsaBuilder;
use crate::{
    BsaArchive, BsaError, HashKind, SIZE_COMPRESSION_TOGGLE, flags, hash_file, hash_folder,
    hash_path,
};

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// Deterministic filler that compresses a little but not to nothing.
fn pattern(seed: u32, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..len)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if i % 3 == 0 {
                (state >> 24) as u8
            } else {
                b'a'
            }
        })
        .collect()
}

fn sample_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("textures/clutter/crate01.dds", pattern(1, 700)),
        ("textures/clutter/crate01_n.dds", pattern(2, 300)),
        ("Textures\\Architecture\\Wall.DDS", pattern(3, 1200)),
        ("meshes/clutter/crate01.nif", pattern(4, 900)),
        ("meshes/clutter/idle.kf", pattern(5, 64)),
        ("sound/fx/door.wav", pattern(6, 2048)),
        ("readme.txt", b"synthetic".to_vec()),
        ("data/empty.bin", Vec::new()),
    ]
}

fn build(compressed: bool, embed: bool) -> Vec<u8> {
    let mut builder = BsaBuilder::new();
    builder.compressed(compressed).embed_names(embed);
    for (path, data) in sample_files() {
        builder.add(path, &data);
    }
    builder.build()
}

fn normalized(path: &str) -> String {
    path.to_ascii_lowercase().replace('/', "\\")
}

fn assert_round_trip(archive: &BsaArchive, files: &[(&str, Vec<u8>)]) {
    assert_eq!(archive.entries().len(), files.len());
    for (path, data) in files {
        let entry = archive
            .find(path)
            .unwrap_or_else(|| panic!("missing {path}"));
        assert_eq!(entry.path, normalized(path));
        assert_eq!(&archive.read(entry).unwrap(), data, "{path}");
    }
}

// ---------------------------------------------------------------- hashes

/// Values computed from the documented algorithm by an independent script
/// and, for the low words, by hand.
#[test]
fn hash_vectors() {
    assert_eq!(hash_file("a.dds"), 0x8ddb_a9c5_6101_80e1);
    assert_eq!(hash_file("ab.nif"), 0x92cd_45fd_6102_8062);
    assert_eq!(hash_file("abc.kf"), 0x1711_e3e9_6103_62e3);
    assert_eq!(hash_file("test.wav"), 0x9733_d003_f404_7374);
    assert_eq!(hash_file("readme.txt"), 0xc7ed_dcea_7206_6d65);
    assert_eq!(hash_file("noext"), 0x006f_1bb6_6e05_7874);
    assert_eq!(hash_file("crate01.dds"), 0x761c_9ba5_6307_b0b1);
    assert_eq!(hash_file("x"), 0x7801_0078);

    assert_eq!(hash_folder(""), 0);
    assert_eq!(hash_folder("a"), 0x6101_0061);
    assert_eq!(hash_folder("ab"), 0x6102_0062);
    assert_eq!(hash_folder("abc"), 0x6103_6263);
    assert_eq!(hash_folder("textures"), 0xd507_789e_7408_6573);
    assert_eq!(hash_folder("textures\\clutter"), 0x048b_f47c_7410_6572);
    assert_eq!(
        hash_folder("meshes\\architecture\\goodsprings"),
        0x1627_dca8_6d1f_6773
    );
}

#[test]
fn hash_ignores_case_and_separator_style() {
    assert_eq!(hash_file("Crate01.DDS"), hash_file("crate01.dds"));
    assert_eq!(
        hash_folder("Textures/Clutter"),
        hash_folder("textures\\clutter")
    );
    assert_eq!(
        hash_path("Textures/Clutter/Crate01.DDS"),
        (hash_folder("textures\\clutter"), hash_file("crate01.dds"))
    );
    assert_eq!(hash_path("readme.txt"), (0, hash_file("readme.txt")));
}

#[test]
fn hash_extension_bits() {
    let plain = hash_file("abcd.xyz") as u32;
    assert_eq!(plain, 0x6104_6364);
    assert_eq!(hash_file("abcd.kf") as u32, plain | 0x80);
    assert_eq!(hash_file("abcd.nif") as u32, plain | 0x8000);
    assert_eq!(hash_file("abcd.dds") as u32, plain | 0x8080);
    assert_eq!(hash_file("abcd.wav") as u32, plain | 0x8000_0000);
    // A folder name is hashed whole: its dot is an ordinary character.
    assert_ne!(hash_folder("abcd.dds"), hash_file("abcd.dds"));
    assert_eq!(hash_folder("abcd.dds") as u32, 0x6108_6473);
}

// ----------------------------------------------------------- round trips

#[test]
fn round_trip_every_archive_flag_combination() {
    for compressed in [false, true] {
        for embed in [false, true] {
            let bytes = build(compressed, embed);
            let archive = BsaArchive::from_bytes(bytes).unwrap();
            assert_eq!(archive.embeds_names(), embed);
            assert_eq!(archive.archive_flags() & flags::COMPRESSED != 0, compressed);
            assert_round_trip(&archive, &sample_files());
            assert!(archive.entries().iter().all(|e| e.compressed == compressed));
            assert!(archive.verify_hashes().is_empty());
        }
    }
}

#[test]
fn per_file_override_inverts_the_default() {
    for default in [false, true] {
        for embed in [false, true] {
            let a = pattern(7, 500);
            let b = pattern(8, 500);
            let c = pattern(9, 500);
            let bytes = BsaBuilder::new()
                .compressed(default)
                .embed_names(embed)
                .add("dir/default.bin", &a)
                .add_with_compression("dir/same.bin", &b, default)
                .add_with_compression("dir/other.bin", &c, !default)
                .build();
            let archive = BsaArchive::from_bytes(bytes.clone()).unwrap();
            let entry = |p: &str| archive.find(p).unwrap();
            assert_eq!(entry("dir/default.bin").compressed, default);
            assert_eq!(entry("dir/same.bin").compressed, default);
            assert_eq!(entry("dir/other.bin").compressed, !default);
            assert_eq!(archive.read(entry("dir/default.bin")).unwrap(), a);
            assert_eq!(archive.read(entry("dir/same.bin")).unwrap(), b);
            assert_eq!(archive.read(entry("dir/other.bin")).unwrap(), c);

            // Exactly one file record carries the override bit.
            let toggled = file_records(&bytes)
                .iter()
                .filter(|r| r.size & SIZE_COMPRESSION_TOGGLE != 0)
                .count();
            assert_eq!(toggled, 1);
        }
    }
}

#[test]
fn stored_size_counts_the_prefixes() {
    let data = vec![0x55u8; 100];
    let plain = BsaBuilder::new().add("d/f.bin", &data).build();
    let embedded = BsaBuilder::new()
        .embed_names(true)
        .add("d/f.bin", &data)
        .build();
    let plain = BsaArchive::from_bytes(plain).unwrap();
    let embedded = BsaArchive::from_bytes(embedded).unwrap();
    assert_eq!(plain.entries()[0].stored_size, 100);
    // One length byte plus the seven bytes of `d\f.bin`.
    assert_eq!(embedded.entries()[0].stored_size, 108);

    let packed = BsaBuilder::new()
        .compressed(true)
        .add("d/f.bin", &data)
        .build();
    let packed = BsaArchive::from_bytes(packed).unwrap();
    let entry = &packed.entries()[0];
    assert!(entry.compressed);
    assert!(entry.stored_size < 100);
    assert_eq!(packed.read(entry).unwrap(), data);
}

#[test]
fn embedded_name_is_the_full_path() {
    let bytes = BsaBuilder::new()
        .embed_names(true)
        .add("Textures/A/B.dds", b"xyz")
        .build();
    let archive = BsaArchive::from_bytes(bytes.clone()).unwrap();
    let entry = &archive.entries()[0];
    let at = entry.offset as usize;
    let expected = b"textures\\a\\b.dds";
    assert_eq!(usize::from(bytes[at]), expected.len());
    assert_eq!(&bytes[at + 1..at + 1 + expected.len()], expected);
    assert_eq!(&bytes[at + 1 + expected.len()..], b"xyz");
}

#[test]
fn empty_archive_and_empty_files() {
    let archive = BsaArchive::from_bytes(BsaBuilder::new().build()).unwrap();
    assert!(archive.entries().is_empty());
    assert!(archive.find("anything").is_none());
    assert!(archive.verify_hashes().is_empty());

    for compressed in [false, true] {
        for embed in [false, true] {
            let bytes = BsaBuilder::new()
                .compressed(compressed)
                .embed_names(embed)
                .add("a/empty.txt", b"")
                .build();
            let archive = BsaArchive::from_bytes(bytes).unwrap();
            assert_eq!(archive.read(&archive.entries()[0]).unwrap(), b"");
        }
    }
}

#[test]
fn root_folder_file_has_no_separator() {
    let bytes = BsaBuilder::new().add("/Top.TXT", b"root").build();
    let archive = BsaArchive::from_bytes(bytes).unwrap();
    assert_eq!(archive.entries()[0].path, "top.txt");
    assert_eq!(archive.entries()[0].folder_hash, 0);
    assert_eq!(
        archive.read(archive.find("TOP.txt").unwrap()).unwrap(),
        b"root"
    );
    assert!(archive.verify_hashes().is_empty());
}

#[test]
fn adding_a_path_twice_replaces_it() {
    let bytes = BsaBuilder::new()
        .add("a/b.txt", b"first")
        .add("A\\B.TXT", b"second")
        .build();
    let archive = BsaArchive::from_bytes(bytes).unwrap();
    assert_eq!(archive.entries().len(), 1);
    assert_eq!(archive.read(&archive.entries()[0]).unwrap(), b"second");
}

#[test]
fn builder_output_does_not_depend_on_insertion_order() {
    let mut forward = BsaBuilder::new();
    let mut backward = BsaBuilder::new();
    forward.compressed(true).embed_names(true);
    backward.compressed(true).embed_names(true);
    let files = sample_files();
    for (path, data) in &files {
        forward.add(path, data);
    }
    for (path, data) in files.iter().rev() {
        backward.add(path, data);
    }
    assert_eq!(forward.build(), backward.build());
}

#[test]
fn open_reads_the_same_archive_from_disk() {
    let bytes = build(true, true);
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    file.flush().unwrap();

    let on_disk = BsaArchive::open(file.path()).unwrap();
    let in_memory = BsaArchive::from_bytes(bytes.clone()).unwrap();
    assert_eq!(on_disk.entries(), in_memory.entries());
    assert_eq!(on_disk.archive_len(), bytes.len() as u64);
    assert_round_trip(&on_disk, &sample_files());
    // Entries can be read in any order, repeatedly.
    for entry in on_disk.entries().iter().rev() {
        assert_eq!(on_disk.read(entry).unwrap(), in_memory.read(entry).unwrap());
    }
}

#[test]
fn open_reads_tables_larger_than_one_buffer() {
    // Enough records and names that the tables span several 64 KiB chunks.
    let mut builder = BsaBuilder::new();
    let mut expected = Vec::new();
    for folder in 0..40 {
        for file in 0..120 {
            let path = format!("generated/folder_number_{folder:03}/file_number_{file:04}.bin");
            let data = format!("{folder}:{file}").into_bytes();
            builder.add(&path, &data);
            expected.push((path, data));
        }
    }
    let bytes = builder.build();
    let data_start = BsaArchive::from_bytes(bytes.clone())
        .unwrap()
        .entries()
        .iter()
        .map(|e| e.offset)
        .min()
        .unwrap();
    assert!(data_start > 2 * 64 * 1024);

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    file.flush().unwrap();
    let archive = BsaArchive::open(file.path()).unwrap();
    assert_eq!(archive.entries().len(), expected.len());
    for (path, data) in &expected {
        assert_eq!(&archive.read(archive.find(path).unwrap()).unwrap(), data);
    }
    assert!(archive.verify_hashes().is_empty());
}

#[test]
fn open_reports_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let err = BsaArchive::open(&dir.path().join("absent.bsa")).unwrap_err();
    assert!(matches!(err, BsaError::Io { offset: 0, .. }));
}

// ---------------------------------------------------------------- lookup

#[test]
fn find_ignores_case_and_separator_style() {
    let archive = BsaArchive::from_bytes(build(false, false)).unwrap();
    let expected = archive.find("textures\\clutter\\crate01.dds").unwrap();
    for spelling in [
        "textures/clutter/crate01.dds",
        "Textures\\Clutter\\Crate01.DDS",
        "TEXTURES/clutter\\CRATE01.dds",
        "\\textures\\clutter\\crate01.dds",
        "/textures/clutter/crate01.dds",
    ] {
        assert_eq!(archive.find(spelling), Some(expected), "{spelling}");
    }
    assert!(archive.find("textures\\clutter").is_none());
    assert!(archive.find("textures\\clutter\\crate02.dds").is_none());
    assert!(archive.find("").is_none());
}

#[test]
fn entries_are_sorted_lowercase_backslash_paths() {
    let archive = BsaArchive::from_bytes(build(true, false)).unwrap();
    let paths: Vec<&str> = archive.entries().iter().map(|e| e.path.as_str()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
    let mut expected: Vec<String> = sample_files().iter().map(|(p, _)| normalized(p)).collect();
    expected.sort_unstable();
    assert_eq!(paths, expected);
}

// ------------------------------------------------- layout of built bytes

struct FolderRecord {
    hash: u64,
    count: u32,
    offset: u32,
}

struct FileRecord {
    hash: u64,
    size: u32,
    offset: u32,
}

fn folder_records(bytes: &[u8]) -> Vec<FolderRecord> {
    (0..u32_at(bytes, 16) as usize)
        .map(|i| {
            let at = 36 + 16 * i;
            FolderRecord {
                hash: u64_at(bytes, at),
                count: u32_at(bytes, at + 8),
                offset: u32_at(bytes, at + 12),
            }
        })
        .collect()
}

/// Walks the file record blocks by the folder records' offset fields.
fn file_records(bytes: &[u8]) -> Vec<FileRecord> {
    let names_len = u32_at(bytes, 28);
    let mut out = Vec::new();
    for folder in folder_records(bytes) {
        let mut at = (folder.offset - names_len) as usize;
        at += 1 + usize::from(bytes[at]);
        for _ in 0..folder.count {
            out.push(FileRecord {
                hash: u64_at(bytes, at),
                size: u32_at(bytes, at + 8),
                offset: u32_at(bytes, at + 12),
            });
            at += 16;
        }
    }
    out
}

#[test]
fn builder_writes_a_consistent_layout() {
    let bytes = build(false, false);
    assert_eq!(&bytes[..4], b"BSA\0");
    assert_eq!(u32_at(&bytes, 4), 104);
    assert_eq!(u32_at(&bytes, 8), 36);
    assert_eq!(u32_at(&bytes, 12), flags::DIR_NAMES | flags::FILE_NAMES);

    // Folders of the sample set: "", data, meshes\clutter, sound\fx,
    // textures\architecture, textures\clutter.
    let folder_names = [
        "",
        "data",
        "meshes\\clutter",
        "sound\\fx",
        "textures\\architecture",
        "textures\\clutter",
    ];
    let folders = folder_records(&bytes);
    assert_eq!(folders.len(), folder_names.len());
    assert_eq!(u32_at(&bytes, 20) as usize, sample_files().len());
    let folder_name_total: usize = folder_names.iter().map(|n| n.len() + 1).sum();
    assert_eq!(u32_at(&bytes, 24) as usize, folder_name_total);
    let file_name_total: usize = sample_files()
        .iter()
        .map(|(p, _)| p.rsplit(['/', '\\']).next().unwrap().len() + 1)
        .sum();
    assert_eq!(u32_at(&bytes, 28) as usize, file_name_total);
    // Meshes, textures, sounds, text and miscellaneous.
    assert_eq!(u32_at(&bytes, 32), 0x1 | 0x2 | 0x8 | 0x20 | 0x100);

    // Folder records ascend by hash, and each offset field points at the
    // folder's block, which starts with its length-prefixed name.
    let mut expected_hashes: Vec<u64> = folder_names.iter().map(|n| hash_folder(n)).collect();
    expected_hashes.sort_unstable();
    let stored: Vec<u64> = folders.iter().map(|f| f.hash).collect();
    assert_eq!(stored, expected_hashes);
    let mut block = 36 + 16 * folders.len();
    for folder in &folders {
        assert_eq!((folder.offset as usize), block + file_name_total);
        let len = usize::from(bytes[block]);
        let name = &bytes[block + 1..block + len];
        assert_eq!(bytes[block + len], 0);
        assert_eq!(hash_folder(std::str::from_utf8(name).unwrap()), folder.hash);
        // File records within a folder ascend by hash.
        let records = block + 1 + len;
        let hashes: Vec<u64> = (0..folder.count as usize)
            .map(|i| u64_at(&bytes, records + 16 * i))
            .collect();
        assert!(hashes.windows(2).all(|w| w[0] < w[1]));
        block = records + 16 * folder.count as usize;
    }

    // The file name block follows the last record block, in record order,
    // and the data follows the names with no gap and no overlap.
    let records = file_records(&bytes);
    let mut name_at = block;
    let mut data_at = block + file_name_total;
    for record in &records {
        let end = name_at + bytes[name_at..].iter().position(|&b| b == 0).unwrap();
        let name = std::str::from_utf8(&bytes[name_at..end]).unwrap();
        assert_eq!(hash_file(name), record.hash);
        name_at = end + 1;
        assert_eq!(record.offset as usize, data_at);
        data_at += record.size as usize;
    }
    assert_eq!(name_at, block + file_name_total);
    assert_eq!(data_at, bytes.len());
}

// ---------------------------------------------------------- verification

#[test]
fn verify_hashes_reports_without_failing_the_open() {
    let mut bytes = build(false, false);
    // Corrupt the first folder record's hash and the first file record's.
    let first_folder_hash = u64_at(&bytes, 36);
    bytes[36] ^= 0x01;
    let names_len = u32_at(&bytes, 28);
    let block = (u32_at(&bytes, 36 + 12) - names_len) as usize;
    let record = block + 1 + usize::from(bytes[block]);
    let first_file_hash = u64_at(&bytes, record);
    bytes[record + 7] ^= 0x80;

    let archive = BsaArchive::from_bytes(bytes).unwrap();
    assert_round_trip(&archive, &sample_files());
    let mismatches = archive.verify_hashes();
    assert_eq!(mismatches.len(), 2);
    assert_eq!(mismatches[0].kind, HashKind::Folder);
    assert_eq!(mismatches[0].computed, first_folder_hash);
    assert_eq!(mismatches[0].stored, first_folder_hash ^ 0x01);
    assert_eq!(mismatches[1].kind, HashKind::File);
    assert_eq!(mismatches[1].computed, first_file_hash);
    assert_eq!(mismatches[1].stored, first_file_hash ^ (0x80 << 56));
    assert_eq!(hash_path(&mismatches[1].path).1, first_file_hash);
}

// -------------------------------------------------------- malformed input

#[test]
fn rejects_wrong_magic_version_and_byte_order() {
    let good = build(false, false);

    let mut bad = good.clone();
    bad[0] = b'X';
    assert!(matches!(
        BsaArchive::from_bytes(bad).unwrap_err(),
        BsaError::BadMagic { found } if &found == b"XSA\0"
    ));

    for version in [103, 105, 0, u32::MAX] {
        let mut bad = good.clone();
        put_u32(&mut bad, 4, version);
        assert!(matches!(
            BsaArchive::from_bytes(bad).unwrap_err(),
            BsaError::UnsupportedVersion { version: v } if v == version
        ));
    }

    let mut bad = good.clone();
    let flags_field = u32_at(&bad, 12) | flags::BIG_ENDIAN;
    put_u32(&mut bad, 12, flags_field);
    let err = BsaArchive::from_bytes(bad).unwrap_err();
    assert!(matches!(err, BsaError::BigEndian { .. }));
    assert!(err.to_string().contains("big-endian"));

    for offset in [0, 35, u32::MAX, good.len() as u32 + 1] {
        let mut bad = good.clone();
        put_u32(&mut bad, 8, offset);
        assert!(matches!(
            BsaArchive::from_bytes(bad).unwrap_err(),
            BsaError::BadHeaderOffset { value, .. } if value == offset
        ));
    }
}

#[test]
fn every_truncation_is_an_error_or_a_clean_open() {
    for (compressed, embed) in [(false, false), (true, true)] {
        let good = build(compressed, embed);
        let data_start = BsaArchive::from_bytes(good.clone())
            .unwrap()
            .entries()
            .iter()
            .map(|e| e.offset)
            .min()
            .unwrap() as usize;
        for len in 0..good.len() {
            match BsaArchive::from_bytes(good[..len].to_vec()) {
                Err(_) => {}
                Ok(archive) => {
                    // The tables are whole, so only data can be missing.
                    assert!(len >= data_start, "opened with {len} bytes");
                    let failures = archive
                        .entries()
                        .iter()
                        .filter(|e| archive.read(e).is_err())
                        .count();
                    assert!(failures > 0, "nothing failed with {len} bytes");
                }
            }
        }
        assert!(matches!(
            BsaArchive::from_bytes(good[..20].to_vec()).unwrap_err(),
            BsaError::Truncated {
                offset: 0,
                needed: 36,
                available: 20,
                ..
            }
        ));
    }
}

#[test]
fn absurd_counts_are_rejected_before_allocation() {
    let good = build(false, false);

    let mut bad = good.clone();
    put_u32(&mut bad, 16, u32::MAX);
    assert!(matches!(
        BsaArchive::from_bytes(bad).unwrap_err(),
        BsaError::CountTooLarge {
            offset: 16,
            count: 0xFFFF_FFFF,
            ..
        }
    ));

    let mut bad = good.clone();
    put_u32(&mut bad, 20, u32::MAX);
    assert!(matches!(
        BsaArchive::from_bytes(bad).unwrap_err(),
        BsaError::CountTooLarge { offset: 20, .. }
    ));

    // A folder that claims more files than the header allows.
    let mut bad = good.clone();
    put_u32(&mut bad, 36 + 8, u32::MAX);
    assert!(matches!(
        BsaArchive::from_bytes(bad).unwrap_err(),
        BsaError::FileCountMismatch { .. }
    ));

    // The header claims more files than the folders hold.
    let mut bad = good.clone();
    let files = u32_at(&bad, 20);
    put_u32(&mut bad, 20, files + 1);
    assert!(matches!(
        BsaArchive::from_bytes(bad).unwrap_err(),
        BsaError::FileCountMismatch { header, folders, .. }
            if header == files + 1 && folders == u64::from(files)
    ));

    // A file name block longer than the file.
    let mut bad = good.clone();
    put_u32(&mut bad, 28, u32::MAX);
    assert!(matches!(
        BsaArchive::from_bytes(bad).unwrap_err(),
        BsaError::Truncated {
            what: "file name block",
            ..
        }
    ));

    // A file name block too short for the names it should hold.
    let mut bad = good.clone();
    put_u32(&mut bad, 28, 3);
    assert!(matches!(
        BsaArchive::from_bytes(bad).unwrap_err(),
        BsaError::UnterminatedName { .. }
    ));
}

#[test]
fn folder_name_running_past_the_end_is_truncation() {
    // One folder, no files; the name length byte claims 200 bytes.
    let mut bytes = b"BSA\0".to_vec();
    for field in [
        104u32,
        36,
        flags::DIR_NAMES | flags::FILE_NAMES,
        1,
        0,
        2,
        0,
        0,
    ] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    bytes.extend_from_slice(&hash_folder("a").to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&52u32.to_le_bytes());
    bytes.extend_from_slice(&[200, b'a', 0]);
    assert!(matches!(
        BsaArchive::from_bytes(bytes).unwrap_err(),
        BsaError::Truncated {
            offset: 53,
            what: "folder name",
            needed: 200,
            available: 2,
        }
    ));
}

/// Rewrites the first file record of a single-file archive.
fn single_file(compressed: bool, embed: bool, data: &[u8]) -> (Vec<u8>, usize) {
    let bytes = BsaBuilder::new()
        .compressed(compressed)
        .embed_names(embed)
        .add("d/f.bin", data)
        .build();
    // Header, one folder record, the block's name (length, "d", zero).
    let record = 36 + 16 + 3;
    (bytes, record)
}

#[test]
fn entry_data_out_of_bounds_is_an_error() {
    let (good, record) = single_file(false, false, b"0123456789");

    let mut bad = good.clone();
    put_u32(&mut bad, record + 12, u32::MAX);
    let archive = BsaArchive::from_bytes(bad).unwrap();
    assert!(matches!(
        archive.read(&archive.entries()[0]).unwrap_err(),
        BsaError::EntryOutOfBounds { .. }
    ));

    let mut bad = good.clone();
    put_u32(&mut bad, record + 8, 0x3FFF_FFFF);
    let archive = BsaArchive::from_bytes(bad).unwrap();
    assert_eq!(archive.entries()[0].stored_size, 0x3FFF_FFFF);
    assert!(matches!(
        archive.read(&archive.entries()[0]).unwrap_err(),
        BsaError::EntryOutOfBounds { .. }
    ));

    // The two top bits of the size field are flags, not size.
    let mut bad = good.clone();
    put_u32(&mut bad, record + 8, 10 | 0x8000_0000);
    let archive = BsaArchive::from_bytes(bad).unwrap();
    assert_eq!(archive.entries()[0].stored_size, 10);
    assert_eq!(archive.read(&archive.entries()[0]).unwrap(), b"0123456789");
}

#[test]
fn entry_size_limit_is_checked_before_reading() {
    let data = pattern(11, 4096);
    for compressed in [false, true] {
        let (bytes, _) = single_file(compressed, false, &data);
        let archive = BsaArchive::from_bytes(bytes).unwrap();
        assert_eq!(archive.max_entry_size(), crate::DEFAULT_MAX_ENTRY_SIZE);
        let entry = archive.entries()[0].clone();
        assert_eq!(archive.read(&entry).unwrap(), data);

        let mut archive = archive.with_max_entry_size(4095);
        assert!(matches!(
            archive.read(&entry).unwrap_err(),
            BsaError::EntryTooLarge { limit: 4095, .. }
        ));
        archive.set_max_entry_size(4096);
        assert_eq!(archive.read(&entry).unwrap(), data);
    }
}

#[test]
fn compressed_entry_with_an_absurd_original_size() {
    let (good, _) = single_file(true, false, &pattern(12, 256));
    let archive = BsaArchive::from_bytes(good.clone()).unwrap();
    let at = archive.entries()[0].offset as usize;

    // Larger than the default limit: refused outright.
    let mut bad = good.clone();
    put_u32(&mut bad, at, u32::MAX);
    let archive = BsaArchive::from_bytes(bad).unwrap();
    assert!(matches!(
        archive.read(&archive.entries()[0]).unwrap_err(),
        BsaError::EntryTooLarge {
            size: 0xFFFF_FFFF,
            ..
        }
    ));

    // Within the limit but wrong, in both directions.
    for claimed in [255u32, 257, 1 << 29] {
        let mut bad = good.clone();
        put_u32(&mut bad, at, claimed);
        let archive = BsaArchive::from_bytes(bad).unwrap();
        match archive.read(&archive.entries()[0]).unwrap_err() {
            BsaError::SizeMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, u64::from(claimed));
                // The reader stops one byte past a short claim, and at the
                // real end of the stream under a long one.
                assert_eq!(actual, 256);
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}

#[test]
fn corrupt_zlib_stream_is_an_error() {
    let (good, record) = single_file(true, false, &pattern(13, 512));
    let archive = BsaArchive::from_bytes(good.clone()).unwrap();
    let at = archive.entries()[0].offset as usize;

    let mut bad = good.clone();
    bad[at + 4] = 0xFF;
    bad[at + 5] = 0xFF;
    let archive = BsaArchive::from_bytes(bad).unwrap();
    assert!(matches!(
        archive.read(&archive.entries()[0]).unwrap_err(),
        BsaError::Decompress { .. }
    ));

    // A stream cut short: fewer stored bytes than the stream needs.
    let mut bad = good.clone();
    let size = u32_at(&bad, record + 8);
    put_u32(&mut bad, record + 8, size - 8);
    let archive = BsaArchive::from_bytes(bad).unwrap();
    assert!(archive.read(&archive.entries()[0]).is_err());

    // Too short to hold the original size at all.
    let mut bad = good.clone();
    put_u32(&mut bad, record + 8, 3);
    let archive = BsaArchive::from_bytes(bad).unwrap();
    assert!(matches!(
        archive.read(&archive.entries()[0]).unwrap_err(),
        BsaError::BadEntryData { .. }
    ));
}

#[test]
fn embedded_name_longer_than_the_entry_is_an_error() {
    let (good, record) = single_file(false, true, b"payload");
    let archive = BsaArchive::from_bytes(good.clone()).unwrap();
    let at = archive.entries()[0].offset as usize;

    let mut bad = good.clone();
    bad[at] = 255;
    let archive = BsaArchive::from_bytes(bad).unwrap();
    assert!(matches!(
        archive.read(&archive.entries()[0]).unwrap_err(),
        BsaError::BadEntryData {
            what: "embedded name",
            ..
        }
    ));

    // A zero-size entry has no room for the length byte.
    let mut bad = good.clone();
    put_u32(&mut bad, record + 8, 0);
    let archive = BsaArchive::from_bytes(bad).unwrap();
    assert!(matches!(
        archive.read(&archive.entries()[0]).unwrap_err(),
        BsaError::BadEntryData { .. }
    ));
}

#[test]
fn archive_without_names_is_addressed_by_hash() {
    // Hand-built: no folder names, no file names, one folder, one file.
    let (folder_hash, file_hash) = hash_path("meshes\\rock.nif");
    let mut bytes = b"BSA\0".to_vec();
    for field in [104u32, 36, 0, 1, 1, 0, 0, 0x1] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    bytes.extend_from_slice(&folder_hash.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&52u32.to_le_bytes());
    bytes.extend_from_slice(&file_hash.to_le_bytes());
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&68u32.to_le_bytes());
    bytes.extend_from_slice(b"mesh");

    let archive = BsaArchive::from_bytes(bytes).unwrap();
    let entry = &archive.entries()[0];
    assert_eq!(
        entry.path,
        format!("#{folder_hash:016x}\\#{file_hash:016x}")
    );
    assert_eq!(archive.find("Meshes/Rock.NIF"), Some(entry));
    assert_eq!(archive.find(&entry.path), Some(entry));
    assert!(archive.find("meshes/other.nif").is_none());
    assert_eq!(archive.read(entry).unwrap(), b"mesh");
    assert!(archive.verify_hashes().is_empty());
}

#[test]
fn random_corruption_never_panics() {
    let good = build(true, true);
    let table_len = BsaArchive::from_bytes(good.clone())
        .unwrap()
        .entries()
        .iter()
        .map(|e| e.offset)
        .min()
        .unwrap() as usize;
    let mut state = 0x9E37_79B9u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for round in 0..3000 {
        let mut bytes = good.clone();
        // Most rounds damage the tables; the rest damage anything.
        let span = if round % 4 == 0 {
            bytes.len()
        } else {
            table_len
        };
        for _ in 0..1 + next() % 4 {
            let at = next() as usize % span;
            bytes[at] = next() as u8;
        }
        if let Ok(archive) = BsaArchive::from_bytes(bytes) {
            // A small limit keeps a corrupted size from costing memory.
            let archive = archive.with_max_entry_size(1 << 20);
            for entry in archive.entries() {
                let _ = archive.read(entry);
            }
            let _ = archive.verify_hashes();
            let _ = archive.find("textures\\clutter\\crate01.dds");
        }
    }
}
