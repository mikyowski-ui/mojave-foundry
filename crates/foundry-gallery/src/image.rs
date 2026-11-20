//! Buffer validation and the owned RGBA8 image type.

use crate::GalleryError;

/// Largest width or height of any image this crate allocates.
pub const MAX_DIMENSION: u32 = 16_384;
/// Largest pixel count of any image this crate allocates (256 MiB of RGBA8).
pub const MAX_PIXELS: u64 = 1 << 26;

/// A rectangle in image pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// An owned RGBA8 image (contracts section 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// `width * height * 4`, computed without overflow.
pub(crate) fn expected_len(width: u32, height: u32) -> Result<u64, GalleryError> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(GalleryError::TooLarge {
            width: u64::from(width),
            height: u64::from(height),
        })
}

/// Checks that `rgba` holds exactly `width * height * 4` bytes.
pub(crate) fn check_rgba(
    what: &'static str,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<(), GalleryError> {
    let expected = expected_len(width, height)?;
    if rgba.len() as u64 != expected {
        return Err(GalleryError::BufferLength {
            what,
            width,
            height,
            expected,
            actual: rgba.len(),
        });
    }
    Ok(())
}

/// Refuses an image this crate would not allocate.
pub(crate) fn check_allocation(width: u64, height: u64) -> Result<(), GalleryError> {
    let too_large = width > u64::from(MAX_DIMENSION)
        || height > u64::from(MAX_DIMENSION)
        || width.saturating_mul(height) > MAX_PIXELS;
    if too_large {
        return Err(GalleryError::TooLarge { width, height });
    }
    Ok(())
}
