//! The documented BSA name hash.
//!
//! A folder record and a file record each carry a 64-bit hash of their name.
//! Records are sorted by it, and the game looks entries up by it. The low 32
//! bits pack the last character, the one before it, the length and the first
//! character of the name without its extension. The high 32 bits are a
//! multiplicative hash of the characters in between, plus the same hash of
//! the extension.

/// Converts a path to the bytes an archive stores: lowercase ASCII, `\`
/// separators, one byte per character. A character above U+00FF cannot be
/// stored and becomes `?`.
pub(crate) fn archive_bytes(path: &str) -> Vec<u8> {
    path.chars()
        .map(|c| match c {
            '/' => b'\\',
            c if (c as u32) < 0x100 => (c as u32 as u8).to_ascii_lowercase(),
            _ => b'?',
        })
        .collect()
}

/// Converts stored name bytes to the `String` form used in entry paths.
pub(crate) fn archive_string(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            b'/' => '\\',
            b => char::from(b.to_ascii_lowercase()),
        })
        .collect()
}

fn rolling(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |h, &b| {
        h.wrapping_mul(0x1003F).wrapping_add(u32::from(b))
    })
}

/// Hashes a normalized stem and extension (the extension includes its dot).
fn hash_parts(stem: &[u8], ext: &[u8]) -> u64 {
    let n = stem.len();
    let mut low = 0u32;
    if n > 0 {
        low = u32::from(stem[n - 1]) | ((n as u32 & 0xFF) << 16) | (u32::from(stem[0]) << 24);
        if n > 2 {
            low |= u32::from(stem[n - 2]) << 8;
        }
    }
    match ext {
        b".kf" => low |= 0x80,
        b".nif" => low |= 0x8000,
        b".dds" => low |= 0x8080,
        b".wav" => low |= 0x8000_0000,
        _ => {}
    }
    let middle = if n > 3 { &stem[1..n - 2] } else { &[][..] };
    let high = rolling(middle).wrapping_add(rolling(ext));
    (u64::from(high) << 32) | u64::from(low)
}

pub(crate) fn hash_folder_bytes(folder: &[u8]) -> u64 {
    hash_parts(folder, &[])
}

pub(crate) fn hash_file_bytes(name: &[u8]) -> u64 {
    match name.iter().rposition(|&b| b == b'.') {
        Some(dot) => hash_parts(&name[..dot], &name[dot..]),
        None => hash_parts(name, &[]),
    }
}

/// Hash of a folder path such as `textures\clutter`. Case and the separator
/// style do not matter. A folder name is hashed whole: a dot in it does not
/// start an extension.
pub fn hash_folder(folder: &str) -> u64 {
    hash_folder_bytes(&archive_bytes(folder))
}

/// Hash of a file name without its folder, such as `crate01.dds`. Case does
/// not matter. The extension starts at the last dot.
pub fn hash_file(name: &str) -> u64 {
    hash_file_bytes(&archive_bytes(name))
}

/// Splits a full path at its last separator and returns
/// `(folder hash, file hash)`. A path without a separator is a file in the
/// root folder, whose name is empty.
pub fn hash_path(path: &str) -> (u64, u64) {
    let bytes = archive_bytes(path);
    match bytes.iter().rposition(|&b| b == b'\\') {
        Some(sep) => (
            hash_folder_bytes(&bytes[..sep]),
            hash_file_bytes(&bytes[sep + 1..]),
        ),
        None => (hash_folder_bytes(&[]), hash_file_bytes(&bytes)),
    }
}
