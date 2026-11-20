//! The 128-byte DDS header and the 20-byte DX10 extension.

use crate::{DdsError, DdsFormat, DdsInfo, MAX_DIMENSION};

pub(crate) const MAGIC: [u8; 4] = *b"DDS ";
pub(crate) const HEADER_LEN: usize = 128;
pub(crate) const DX10_LEN: usize = 20;

// Field offsets from the start of the file.
const OFF_SIZE: usize = 4;
const OFF_HEIGHT: usize = 12;
const OFF_WIDTH: usize = 16;
const OFF_DEPTH: usize = 24;
const OFF_MIP_COUNT: usize = 28;
const OFF_PF_SIZE: usize = 76;
const OFF_PF_FLAGS: usize = 80;
const OFF_FOURCC: usize = 84;
const OFF_BIT_COUNT: usize = 88;
const OFF_R_MASK: usize = 92;
const OFF_CAPS2: usize = 112;
const OFF_DXGI_FORMAT: usize = 128;
const OFF_DX10_DIMENSION: usize = 132;
const OFF_DX10_MISC: usize = 136;

/// Pixel format flag: the alpha mask is valid.
pub const DDPF_ALPHAPIXELS: u32 = 0x1;
/// Pixel format flag: alpha-only pixels.
pub const DDPF_ALPHA: u32 = 0x2;
/// Pixel format flag: the FourCC is valid.
pub const DDPF_FOURCC: u32 = 0x4;
/// Pixel format flag: the colour masks are valid.
pub const DDPF_RGB: u32 = 0x40;
pub(crate) const DDPF_YUV: u32 = 0x200;
/// Pixel format flag: the red mask holds luminance.
pub const DDPF_LUMINANCE: u32 = 0x2_0000;
pub(crate) const DDPF_BUMPDUDV: u32 = 0x8_0000;

pub(crate) const CAPS2_CUBEMAP: u32 = 0x200;
pub(crate) const CAPS2_VOLUME: u32 = 0x20_0000;

const DX10_DIMENSION_TEXTURE2D: u32 = 3;
const DX10_MISC_CUBE: u32 = 0x4;

pub(crate) struct Parsed {
    pub info: DdsInfo,
    /// Offset of the first byte of pixel data.
    pub data_offset: usize,
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(word)
}

fn unsupported(offset: usize, what: impl Into<String>) -> DdsError {
    DdsError::Unsupported {
        offset,
        what: what.into(),
    }
}

const fn masks(bit_count: u32, r: u32, g: u32, b: u32, a: u32) -> DdsFormat {
    DdsFormat::Uncompressed {
        bit_count,
        r_mask: r,
        g_mask: g,
        b_mask: b,
        a_mask: a,
        luminance: false,
    }
}

/// Maps a DXGI format number to the equivalent decoder format. sRGB
/// variants map like their linear twins: stored bytes pass through.
fn dxgi_format(format: u32) -> Option<DdsFormat> {
    Some(match format {
        // R8G8B8A8_UNORM, R8G8B8A8_UNORM_SRGB
        28 | 29 => masks(32, 0xFF, 0xFF00, 0xFF_0000, 0xFF00_0000),
        // R8G8_UNORM
        49 => masks(16, 0xFF, 0xFF00, 0, 0),
        // R8_UNORM
        61 => masks(8, 0xFF, 0, 0, 0),
        // A8_UNORM
        65 => masks(8, 0, 0, 0, 0xFF),
        // BC1_UNORM, BC1_UNORM_SRGB
        71 | 72 => DdsFormat::Bc1,
        // BC2_UNORM, BC2_UNORM_SRGB
        74 | 75 => DdsFormat::Bc2,
        // BC3_UNORM, BC3_UNORM_SRGB
        77 | 78 => DdsFormat::Bc3,
        // BC5_UNORM
        83 => DdsFormat::Bc5,
        // B5G6R5_UNORM
        85 => masks(16, 0xF800, 0x07E0, 0x001F, 0),
        // B5G5R5A1_UNORM
        86 => masks(16, 0x7C00, 0x03E0, 0x001F, 0x8000),
        // B8G8R8A8_UNORM, B8G8R8A8_UNORM_SRGB
        87 | 91 => masks(32, 0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000),
        // B8G8R8X8_UNORM, B8G8R8X8_UNORM_SRGB
        88 | 93 => masks(32, 0xFF_0000, 0xFF00, 0xFF, 0),
        // B4G4R4A4_UNORM
        115 => masks(16, 0x0F00, 0x00F0, 0x000F, 0xF000),
        _ => return None,
    })
}

/// A channel mask must be one run of set bits inside the pixel.
fn check_mask(mask: u32, bit_count: u32, offset: usize) -> Result<(), DdsError> {
    let fits = bit_count == 32 || mask >> bit_count == 0;
    let contiguous = mask == 0
        || (mask >> mask.trailing_zeros())
            .wrapping_add(1)
            .is_power_of_two();
    if fits && contiguous {
        Ok(())
    } else {
        Err(DdsError::BadHeader {
            offset,
            what: "channel mask",
            value: mask,
        })
    }
}

fn masked_format(bytes: &[u8], pf_flags: u32) -> Result<DdsFormat, DdsError> {
    if pf_flags & (DDPF_YUV | DDPF_BUMPDUDV) != 0 {
        return Err(unsupported(
            OFF_PF_FLAGS,
            format!("pixel format flags {pf_flags:#x} (YUV or signed bump map)"),
        ));
    }
    let bit_count = le32(bytes, OFF_BIT_COUNT);
    if !matches!(bit_count, 8 | 16 | 24 | 32) {
        return Err(unsupported(
            OFF_BIT_COUNT,
            format!("uncompressed pixel of {bit_count} bits"),
        ));
    }
    let mut channel = [0u32; 4];
    for (i, mask) in channel.iter_mut().enumerate() {
        let offset = OFF_R_MASK + 4 * i;
        *mask = le32(bytes, offset);
        check_mask(*mask, bit_count, offset)?;
    }
    let [r, g, b, a] = channel;

    // The flags say which masks mean anything. A writer that left them out
    // is read by which masks it filled in.
    let rgb = pf_flags & DDPF_RGB != 0;
    let luminance = pf_flags & DDPF_LUMINANCE != 0;
    let alpha_only = pf_flags & DDPF_ALPHA != 0;
    let has_alpha = pf_flags & DDPF_ALPHAPIXELS != 0;
    let format = if rgb || (!luminance && !alpha_only && (g | b) != 0) {
        let a = if has_alpha || !rgb { a } else { 0 };
        masks(bit_count, r, g, b, a)
    } else if luminance || (!alpha_only && r != 0) {
        DdsFormat::Uncompressed {
            bit_count,
            r_mask: r,
            g_mask: 0,
            b_mask: 0,
            a_mask: if has_alpha || !luminance { a } else { 0 },
            luminance: true,
        }
    } else {
        masks(bit_count, 0, 0, 0, a)
    };
    let DdsFormat::Uncompressed {
        r_mask,
        g_mask,
        b_mask,
        a_mask,
        ..
    } = format
    else {
        return Ok(format);
    };
    if r_mask | g_mask | b_mask | a_mask == 0 {
        return Err(unsupported(
            OFF_PF_FLAGS,
            format!("pixel format flags {pf_flags:#x} with no channel masks"),
        ));
    }
    Ok(format)
}

/// Bytes of the top mip level of one face.
pub(crate) fn top_level_len(format: DdsFormat, width: u32, height: u32) -> u64 {
    let (width, height) = (u64::from(width), u64::from(height));
    let blocks = width.div_ceil(4) * height.div_ceil(4);
    match format {
        DdsFormat::Bc1 => blocks * 8,
        DdsFormat::Bc2 | DdsFormat::Bc3 | DdsFormat::Bc5 => blocks * 16,
        DdsFormat::Uncompressed { bit_count, .. } => width * height * u64::from(bit_count / 8),
    }
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Parsed, DdsError> {
    let truncated = |what, needed: usize| DdsError::Truncated {
        offset: 0,
        what,
        needed: needed as u64,
        available: bytes.len() as u64,
    };
    if bytes.len() < 4 {
        return Err(truncated("magic", 4));
    }
    if bytes[..4] != MAGIC {
        let mut found = [0u8; 4];
        found.copy_from_slice(&bytes[..4]);
        return Err(DdsError::BadMagic { found });
    }
    if bytes.len() < HEADER_LEN {
        return Err(truncated("header", HEADER_LEN));
    }

    let size = le32(bytes, OFF_SIZE);
    if size != 124 {
        return Err(DdsError::BadHeader {
            offset: OFF_SIZE,
            what: "header size (expected 124)",
            value: size,
        });
    }
    let pf_size = le32(bytes, OFF_PF_SIZE);
    if pf_size != 32 {
        return Err(DdsError::BadHeader {
            offset: OFF_PF_SIZE,
            what: "pixel format size (expected 32)",
            value: pf_size,
        });
    }

    let height = le32(bytes, OFF_HEIGHT);
    let width = le32(bytes, OFF_WIDTH);
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(DdsError::BadDimensions {
            width,
            height,
            limit: MAX_DIMENSION,
        });
    }

    let caps2 = le32(bytes, OFF_CAPS2);
    if caps2 & CAPS2_VOLUME != 0 && le32(bytes, OFF_DEPTH) > 1 {
        return Err(unsupported(OFF_CAPS2, "volume texture"));
    }
    let mut cubemap = caps2 & CAPS2_CUBEMAP != 0;
    let mut data_offset = HEADER_LEN;

    let pf_flags = le32(bytes, OFF_PF_FLAGS);
    let format = if pf_flags & DDPF_FOURCC != 0 {
        let mut fourcc = [0u8; 4];
        fourcc.copy_from_slice(&bytes[OFF_FOURCC..OFF_FOURCC + 4]);
        match &fourcc {
            b"DXT1" => DdsFormat::Bc1,
            b"DXT3" => DdsFormat::Bc2,
            b"DXT5" => DdsFormat::Bc3,
            b"ATI2" | b"BC5U" => DdsFormat::Bc5,
            b"DXT2" | b"DXT4" => {
                return Err(unsupported(
                    OFF_FOURCC,
                    format!(
                        "FourCC {:?} (premultiplied alpha)",
                        String::from_utf8_lossy(&fourcc)
                    ),
                ));
            }
            b"DX10" => {
                if bytes.len() < HEADER_LEN + DX10_LEN {
                    return Err(DdsError::Truncated {
                        offset: HEADER_LEN,
                        what: "DX10 header",
                        needed: DX10_LEN as u64,
                        available: (bytes.len() - HEADER_LEN) as u64,
                    });
                }
                data_offset = HEADER_LEN + DX10_LEN;
                let dimension = le32(bytes, OFF_DX10_DIMENSION);
                if dimension != DX10_DIMENSION_TEXTURE2D {
                    return Err(unsupported(
                        OFF_DX10_DIMENSION,
                        format!("DX10 resource dimension {dimension} (only 2D textures)"),
                    ));
                }
                cubemap |= le32(bytes, OFF_DX10_MISC) & DX10_MISC_CUBE != 0;
                let dxgi = le32(bytes, OFF_DXGI_FORMAT);
                dxgi_format(dxgi)
                    .ok_or_else(|| unsupported(OFF_DXGI_FORMAT, format!("DXGI format {dxgi}")))?
            }
            _ => {
                return Err(unsupported(
                    OFF_FOURCC,
                    format!(
                        "FourCC {:?} ({:#010x})",
                        String::from_utf8_lossy(&fourcc),
                        u32::from_le_bytes(fourcc)
                    ),
                ));
            }
        }
    } else {
        masked_format(bytes, pf_flags)?
    };

    Ok(Parsed {
        info: DdsInfo {
            width,
            height,
            mip_count: le32(bytes, OFF_MIP_COUNT).max(1),
            format,
            cubemap,
        },
        data_offset,
    })
}
