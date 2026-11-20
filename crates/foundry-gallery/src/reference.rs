//! Original Fallout 2 art as RGBA8, for comparison cells.

use vault_formats::{FrmArchive, Palette};

use crate::GalleryError;

/// Converts an indexed image (contracts section 5) to RGBA8 through
/// `palette`'s neutral display colours. Index 0 becomes fully transparent
/// black; every other index is opaque.
pub fn indexed_to_rgba(
    indexed: &[u8],
    width: u32,
    height: u32,
    palette: &Palette,
) -> Result<Vec<u8>, GalleryError> {
    let expected = u64::from(width) * u64::from(height);
    if indexed.len() as u64 != expected {
        return Err(GalleryError::BufferLength {
            what: "indexed image",
            width,
            height,
            expected,
            actual: indexed.len(),
        });
    }
    let colours = palette.rgba8();
    let mut out = Vec::with_capacity(indexed.len() * 4);
    for &index in indexed {
        if index == vault_formats::palette::TRANSPARENT_INDEX {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            out.extend_from_slice(&colours[usize::from(index)]);
        }
    }
    Ok(out)
}

/// One FRM frame as `(rgba, width, height)`, or `None` when the direction
/// or frame does not exist.
pub fn frm_frame_rgba(
    frm: &FrmArchive,
    direction: usize,
    frame: usize,
    palette: &Palette,
) -> Option<(Vec<u8>, u32, u32)> {
    let record = frm.frame(direction, frame)?;
    let width = u32::from(record.width);
    let height = u32::from(record.height);
    let rgba = indexed_to_rgba(record.pixels(), width, height, palette).ok()?;
    Some((rgba, width, height))
}
