//! Stream versions other than 34: Fallout 3 ships a few meshes at Bethesda
//! stream versions 14, 21 and 26 (trigger and test meshes) and some files
//! at 20.0.0.4. A mesh at any of them is refused with
//! [`NifError::UnsupportedVersion`], never a panic, so that the extractor
//! can count it as a skip.

use nif::fixture::building;
use nif::{HEADER_LINE, STREAM_VERSION};
use nif::{NifError, parse};

#[test]
fn older_stream_versions_are_a_clean_skip() {
    let good = building([8.0, 6.0, 4.0], "textures\\w.dds", "textures\\r.dds");
    assert!(parse(&good).is_ok());
    let version_at = HEADER_LINE.len() + 1;
    let stream_at = version_at + 13;
    for stream in [14u32, 21, 26] {
        let mut older = good.clone();
        older[stream_at..stream_at + 4].copy_from_slice(&stream.to_le_bytes());
        let error = parse(&older).unwrap_err();
        assert!(error.is_unsupported_version(), "{error}");
        assert!(
            matches!(error, NifError::UnsupportedVersion { stream_version, .. } if stream_version == stream),
            "{error}"
        );
        // Every truncation of it is an error too, never a panic.
        for cut in 0..older.len().min(256) {
            let _ = parse(&older[..cut]);
        }
    }
    let mut v20004 = good.clone();
    v20004[version_at..version_at + 4].copy_from_slice(&0x1400_0004u32.to_le_bytes());
    let error = parse(&v20004).unwrap_err();
    assert!(error.is_unsupported_version());
    assert!(error.to_string().contains("20.0.0.4"));
    // Other failures are not version skips.
    assert!(!parse(b"not a mesh").unwrap_err().is_unsupported_version());
    assert_eq!(STREAM_VERSION, 34);
}
