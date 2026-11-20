//! DDS decoder: BC1, BC2, BC3, BC5 and uncompressed surfaces to RGBA.
//!
//! [`decode`] returns the top mip level as RGBA8, row-major, top row first,
//! straight alpha. Stored bytes pass through: no gamma conversion is
//! applied, and an sRGB DXGI format decodes exactly like its linear twin.
//!
//! Rounding, where the format leaves it open:
//!
//! - a 5- or 6-bit channel widens to 8 bits by repeating its high bits
//!   (`31 -> 255`, `1 -> 8`); so does any mask-described channel narrower
//!   than 8 bits, and a wider one keeps its top 8 bits;
//! - interpolated block colours and alphas use truncating integer division
//!   on the widened 8-bit endpoints, for example `(2 * c0 + c1) / 3`.
//!
//! [`fixture`] holds simple encoders, so that tests in this workspace build
//! their textures in memory.

mod bc;
mod error;
pub mod fixture;
mod header;
mod masks;

pub use error::DdsError;

/// Largest width or height that is decoded.
pub const MAX_DIMENSION: u32 = 16384;

/// A decoded image: RGBA8, row-major, top row first, straight alpha,
/// `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The pixel format of a DDS file, as far as this decoder distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DdsFormat {
    /// DXT1: 8-byte blocks, 565 endpoints, optional 1-bit alpha.
    Bc1,
    /// DXT3: explicit 4-bit alpha, then a colour block.
    Bc2,
    /// DXT5: interpolated alpha, then a colour block.
    Bc3,
    /// ATI2: two interpolated channels, red then green. Blue is rebuilt as
    /// the third component of a unit normal and alpha is 255.
    Bc5,
    /// Whole pixels of `bit_count` bits, little-endian, one bit mask per
    /// channel. A zero colour mask reads as 0 and a zero alpha mask as 255.
    /// With `luminance` set, `r_mask` is the luminance mask and green and
    /// blue copy red.
    Uncompressed {
        bit_count: u32,
        r_mask: u32,
        g_mask: u32,
        b_mask: u32,
        a_mask: u32,
        luminance: bool,
    },
}

/// What the headers of a DDS file say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DdsInfo {
    pub width: u32,
    pub height: u32,
    /// Number of mip levels stored, at least 1. Only the first is decoded.
    pub mip_count: u32,
    pub format: DdsFormat,
    /// The file is a cube map. [`decode`] returns its first face, which is
    /// the +X face in a complete cube map.
    pub cubemap: bool,
}

/// Reads the headers without touching the pixel data.
pub fn info(bytes: &[u8]) -> Result<DdsInfo, DdsError> {
    Ok(header::parse(bytes)?.info)
}

/// Decodes the top mip level (of the first face, for a cube map) to RGBA8.
pub fn decode(bytes: &[u8]) -> Result<Image, DdsError> {
    let parsed = header::parse(bytes)?;
    let info = parsed.info;
    let (width, height) = (info.width as usize, info.height as usize);

    // The payload must be present in full before the output is allocated.
    let needed = header::top_level_len(info.format, info.width, info.height);
    let available = bytes.len().saturating_sub(parsed.data_offset);
    if needed > available as u64 {
        return Err(DdsError::Truncated {
            offset: parsed.data_offset,
            what: "top mip level",
            needed,
            available: available as u64,
        });
    }
    let data = &bytes[parsed.data_offset..parsed.data_offset + needed as usize];

    // At most 16384 * 16384 * 4 bytes, and never more than 8 output bytes
    // per payload byte, so a small file cannot ask for a large buffer.
    let mut rgba = vec![0u8; width * height * 4];
    match info.format {
        DdsFormat::Bc1 => bc::decode(data, width, height, 8, &mut rgba, bc::bc1_block),
        DdsFormat::Bc2 => bc::decode(data, width, height, 16, &mut rgba, bc::bc2_block),
        DdsFormat::Bc3 => bc::decode(data, width, height, 16, &mut rgba, bc::bc3_block),
        DdsFormat::Bc5 => bc::decode(data, width, height, 16, &mut rgba, bc::bc5_block),
        DdsFormat::Uncompressed {
            bit_count,
            r_mask,
            g_mask,
            b_mask,
            a_mask,
            luminance,
        } => masks::decode(
            data,
            (bit_count / 8) as usize,
            [r_mask, g_mask, b_mask, a_mask],
            luminance,
            &mut rgba,
        ),
    }
    Ok(Image {
        width: info.width,
        height: info.height,
        rgba,
    })
}

#[cfg(test)]
mod tests;
