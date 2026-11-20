//! Builders for synthetic DDS files.
//!
//! [`DdsFile`] writes a header around any payload, which is what a test
//! with hand-built blocks needs. The `encode_*` functions turn an RGBA8
//! image into a complete single-level file. The block encoders pick
//! endpoints from the bounding box of each block: quality is modest, but
//! the files are valid and the output is deterministic.
//!
//! Every function here panics when `rgba` is not `width * height * 4`
//! bytes or a dimension is zero. Inputs come from test code, not files.

use crate::bc::{self, Texels};
use crate::header::{CAPS2_CUBEMAP, MAGIC};

pub use crate::header::{DDPF_ALPHA, DDPF_ALPHAPIXELS, DDPF_FOURCC, DDPF_LUMINANCE, DDPF_RGB};

/// How a [`DdsFile`] describes its pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// A FourCC code such as `*b"DXT1"`.
    FourCc([u8; 4]),
    /// Uncompressed pixels: the pixel format flags (`DDPF_*`), the pixel
    /// size in bits and the four channel masks.
    Masks {
        flags: u32,
        bit_count: u32,
        r_mask: u32,
        g_mask: u32,
        b_mask: u32,
        a_mask: u32,
    },
    /// A `DX10` FourCC followed by the extension header with this DXGI
    /// format number.
    Dx10 { dxgi_format: u32 },
}

/// A DDS file to be written: header fields and the payload as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdsFile {
    pub width: u32,
    pub height: u32,
    /// Mip levels the payload holds. 1 means no mip chain.
    pub mip_count: u32,
    /// Marks the file as a cube map with all six faces.
    pub cubemap: bool,
    pub pixel_format: PixelFormat,
    /// Everything after the headers.
    pub data: Vec<u8>,
}

impl DdsFile {
    /// A single-level, non-cube file.
    pub fn new(width: u32, height: u32, pixel_format: PixelFormat, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            mip_count: 1,
            cubemap: false,
            pixel_format,
            data,
        }
    }

    /// Writes the magic, the 124-byte header, the DX10 extension when the
    /// pixel format asks for one, and the payload.
    pub fn to_bytes(&self) -> Vec<u8> {
        const DDSD_CAPS: u32 = 0x1;
        const DDSD_HEIGHT: u32 = 0x2;
        const DDSD_WIDTH: u32 = 0x4;
        const DDSD_PITCH: u32 = 0x8;
        const DDSD_PIXELFORMAT: u32 = 0x1000;
        const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
        const DDSD_LINEARSIZE: u32 = 0x8_0000;
        const DDSCAPS_COMPLEX: u32 = 0x8;
        const DDSCAPS_TEXTURE: u32 = 0x1000;
        const DDSCAPS_MIPMAP: u32 = 0x40_0000;
        const CAPS2_ALL_FACES: u32 = 0xFC00;

        let (width, height) = (u64::from(self.width), u64::from(self.height));
        let (pitch_flag, pitch) = match self.pixel_format {
            PixelFormat::Masks { bit_count, .. } => {
                (DDSD_PITCH, width * u64::from(bit_count).div_ceil(8))
            }
            PixelFormat::FourCc(code) if &code == b"DXT1" => {
                (DDSD_LINEARSIZE, width.div_ceil(4) * height.div_ceil(4) * 8)
            }
            PixelFormat::FourCc(_) => {
                (DDSD_LINEARSIZE, width.div_ceil(4) * height.div_ceil(4) * 16)
            }
            PixelFormat::Dx10 { .. } => (0, 0),
        };

        let mut flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT | pitch_flag;
        let mut caps = DDSCAPS_TEXTURE;
        if self.mip_count > 1 {
            flags |= DDSD_MIPMAPCOUNT;
            caps |= DDSCAPS_COMPLEX | DDSCAPS_MIPMAP;
        }
        let mut caps2 = 0;
        if self.cubemap {
            caps |= DDSCAPS_COMPLEX;
            caps2 = CAPS2_CUBEMAP | CAPS2_ALL_FACES;
        }

        // Pixel format: size, flags, FourCC, bit count, four masks.
        let pixel_format: [u32; 8] = match self.pixel_format {
            PixelFormat::FourCc(code) => [32, DDPF_FOURCC, u32::from_le_bytes(code), 0, 0, 0, 0, 0],
            PixelFormat::Dx10 { .. } => {
                [32, DDPF_FOURCC, u32::from_le_bytes(*b"DX10"), 0, 0, 0, 0, 0]
            }
            PixelFormat::Masks {
                flags,
                bit_count,
                r_mask,
                g_mask,
                b_mask,
                a_mask,
            } => [32, flags, 0, bit_count, r_mask, g_mask, b_mask, a_mask],
        };

        let mut words: Vec<u32> = vec![
            124,
            flags,
            self.height,
            self.width,
            u32::try_from(pitch).unwrap_or(0),
            0,
            if self.mip_count > 1 {
                self.mip_count
            } else {
                0
            },
        ];
        words.extend([0; 11]);
        words.extend(pixel_format);
        words.extend([caps, caps2, 0, 0, 0]);
        debug_assert_eq!(words.len() * 4, 124);
        if let PixelFormat::Dx10 { dxgi_format } = self.pixel_format {
            // Format, dimension (3 is a 2D texture), misc flags (4 marks a
            // cube), array size, misc flags 2.
            let misc = if self.cubemap { 4 } else { 0 };
            words.extend([dxgi_format, 3, misc, 1, 0]);
        }

        let mut out = Vec::with_capacity(4 + words.len() * 4 + self.data.len());
        out.extend_from_slice(&MAGIC);
        for word in words {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&self.data);
        out
    }
}

/// The uncompressed layouts [`encode_uncompressed`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncompressedLayout {
    /// 32 bits, bytes in the order blue, green, red, alpha.
    Bgra8,
    /// 32 bits, bytes in the order red, green, blue, alpha.
    Rgba8,
    /// 32 bits, bytes blue, green, red and one unused byte. Alpha is lost.
    Bgrx8,
    /// 24 bits, bytes blue, green, red. Alpha is lost.
    Bgr8,
    /// 16 bits: 5 red, 6 green, 5 blue. Alpha is lost.
    Rgb565,
    /// 8 bits of luminance, taken from the red channel. Alpha is lost.
    L8,
    /// 16 bits: luminance from the red channel in the low byte, alpha in
    /// the high byte.
    A8L8,
}

impl UncompressedLayout {
    /// The header description of this layout.
    pub fn pixel_format(self) -> PixelFormat {
        let (flags, bit_count, r_mask, g_mask, b_mask, a_mask) = match self {
            Self::Bgra8 => (
                DDPF_RGB | DDPF_ALPHAPIXELS,
                32,
                0x00FF_0000,
                0x0000_FF00,
                0x0000_00FF,
                0xFF00_0000,
            ),
            Self::Rgba8 => (
                DDPF_RGB | DDPF_ALPHAPIXELS,
                32,
                0x0000_00FF,
                0x0000_FF00,
                0x00FF_0000,
                0xFF00_0000,
            ),
            Self::Bgrx8 => (DDPF_RGB, 32, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0),
            Self::Bgr8 => (DDPF_RGB, 24, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0),
            Self::Rgb565 => (DDPF_RGB, 16, 0xF800, 0x07E0, 0x001F, 0),
            Self::L8 => (DDPF_LUMINANCE, 8, 0xFF, 0, 0, 0),
            Self::A8L8 => (DDPF_LUMINANCE | DDPF_ALPHAPIXELS, 16, 0x00FF, 0, 0, 0xFF00),
        };
        PixelFormat::Masks {
            flags,
            bit_count,
            r_mask,
            g_mask,
            b_mask,
            a_mask,
        }
    }
}

fn check_input(width: u32, height: u32, rgba: &[u8]) {
    assert!(width > 0 && height > 0, "empty image");
    assert_eq!(
        rgba.len() as u64,
        u64::from(width) * u64::from(height) * 4,
        "rgba must be width * height * 4 bytes"
    );
}

/// Rounds an 8-bit colour to the nearest 565 value.
pub fn pack_565(rgb: [u8; 3]) -> u16 {
    let scale = |v: u8, max: u16| (u16::from(v) * max + 127) / 255;
    (scale(rgb[0], 31) << 11) | (scale(rgb[1], 63) << 5) | scale(rgb[2], 31)
}

/// Writes an uncompressed file in the given layout.
pub fn encode_uncompressed(
    width: u32,
    height: u32,
    rgba: &[u8],
    layout: UncompressedLayout,
) -> Vec<u8> {
    check_input(width, height, rgba);
    let mut data = Vec::new();
    for px in rgba.as_chunks::<4>().0.iter() {
        let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
        match layout {
            UncompressedLayout::Bgra8 => data.extend_from_slice(&[b, g, r, a]),
            UncompressedLayout::Rgba8 => data.extend_from_slice(&[r, g, b, a]),
            UncompressedLayout::Bgrx8 => data.extend_from_slice(&[b, g, r, 0]),
            UncompressedLayout::Bgr8 => data.extend_from_slice(&[b, g, r]),
            UncompressedLayout::Rgb565 => {
                data.extend_from_slice(&pack_565([r, g, b]).to_le_bytes());
            }
            UncompressedLayout::L8 => data.push(r),
            UncompressedLayout::A8L8 => data.extend_from_slice(&[r, a]),
        }
    }
    DdsFile::new(width, height, layout.pixel_format(), data).to_bytes()
}

/// Collects the 4 x 4 block at block coordinates `(bx, by)`. Texels past
/// the right or bottom edge repeat the nearest edge texel.
fn block_texels(width: u32, height: u32, rgba: &[u8], bx: u32, by: u32) -> Texels {
    let mut texels: Texels = [[0; 4]; 16];
    for (i, texel) in texels.iter_mut().enumerate() {
        let x = (bx * 4 + i as u32 % 4).min(width - 1) as usize;
        let y = (by * 4 + i as u32 / 4).min(height - 1) as usize;
        let at = (y * width as usize + x) * 4;
        texel.copy_from_slice(&rgba[at..at + 4]);
    }
    texels
}

/// Index of the palette entry nearest to `value`; the first wins a tie.
fn nearest<const N: usize>(palette: &[[u8; N]], value: &[u8]) -> usize {
    let distance = |entry: &[u8; N]| -> u32 {
        entry
            .iter()
            .zip(value)
            .map(|(&p, &v)| u32::from(p.abs_diff(v)).pow(2))
            .sum()
    };
    let mut best = 0;
    for (i, entry) in palette.iter().enumerate() {
        if distance(entry) < distance(&palette[best]) {
            best = i;
        }
    }
    best
}

/// Encodes an 8-byte colour block. With `punch_through`, texels whose alpha
/// is below 128 become transparent, which puts the block in three-colour
/// mode; without it the block is always read as four colours.
pub fn encode_color_block(texels: &[[u8; 4]; 16], punch_through: bool) -> [u8; 8] {
    let transparent = |t: &[u8; 4]| punch_through && t[3] < 128;
    let mut lo = [255u8; 3];
    let mut hi = [0u8; 3];
    let mut any_opaque = false;
    for texel in texels.iter().filter(|t| !transparent(t)) {
        any_opaque = true;
        for c in 0..3 {
            lo[c] = lo[c].min(texel[c]);
            hi[c] = hi[c].max(texel[c]);
        }
    }
    if !any_opaque {
        lo = [0; 3];
        hi = [0; 3];
    }
    // `hi` is at least `lo` in every channel, so it also packs higher.
    let (mut c0, mut c1) = (pack_565(hi), pack_565(lo));
    if texels.iter().any(transparent) {
        // Three-colour mode needs c0 <= c1.
        std::mem::swap(&mut c0, &mut c1);
    }
    let palette = bc::color_palette(c0, c1, punch_through);
    // In three-colour mode entry 3 is the transparent one: opaque texels
    // choose among the first three.
    let opaque_entries = if punch_through && c0 <= c1 { 3 } else { 4 };
    let colours: Vec<[u8; 3]> = palette[..opaque_entries]
        .iter()
        .map(|p| [p[0], p[1], p[2]])
        .collect();

    let mut block = [0u8; 8];
    block[0..2].copy_from_slice(&c0.to_le_bytes());
    block[2..4].copy_from_slice(&c1.to_le_bytes());
    for (i, texel) in texels.iter().enumerate() {
        let index = if transparent(texel) {
            3
        } else {
            nearest(&colours, &texel[..3]) as u8
        };
        block[4 + i / 4] |= index << (2 * (i % 4));
    }
    block
}

/// Encodes 16 values as an 8-byte interpolated block (the alpha half of
/// BC3, or one channel of BC5).
pub fn encode_alpha_block(values: &[u8; 16]) -> [u8; 8] {
    let a0 = values.iter().copied().max().unwrap_or(0);
    let a1 = values.iter().copied().min().unwrap_or(0);
    let palette = bc::alpha_palette(a0, a1).map(|v| [v]);
    let mut bits = 0u64;
    for (i, &value) in values.iter().enumerate() {
        bits |= (nearest(&palette, &[value]) as u64) << (3 * i);
    }
    let mut block = [0u8; 8];
    block[0] = a0;
    block[1] = a1;
    block[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    block
}

fn encode_blocks(
    width: u32,
    height: u32,
    rgba: &[u8],
    fourcc: [u8; 4],
    mut encode: impl FnMut(&Texels, &mut Vec<u8>),
) -> Vec<u8> {
    check_input(width, height, rgba);
    let mut data = Vec::new();
    for by in 0..height.div_ceil(4) {
        for bx in 0..width.div_ceil(4) {
            encode(&block_texels(width, height, rgba, bx, by), &mut data);
        }
    }
    DdsFile::new(width, height, PixelFormat::FourCc(fourcc), data).to_bytes()
}

fn channel(texels: &Texels, index: usize) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (value, texel) in out.iter_mut().zip(texels) {
        *value = texel[index];
    }
    out
}

/// Writes a `DXT1` file. Texels with alpha below 128 become transparent
/// black; all others are opaque.
pub fn encode_bc1(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    encode_blocks(width, height, rgba, *b"DXT1", |texels, out| {
        out.extend_from_slice(&encode_color_block(texels, true));
    })
}

/// Writes a `DXT3` file: alpha keeps its top four bits.
pub fn encode_bc2(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    encode_blocks(width, height, rgba, *b"DXT3", |texels, out| {
        let mut alpha = [0u8; 8];
        for (i, texel) in texels.iter().enumerate() {
            // Nearest of the 16 levels, which are 17 apart.
            let nibble = ((u16::from(texel[3]) + 8) / 17) as u8;
            alpha[i / 2] |= nibble << (4 * (i % 2));
        }
        out.extend_from_slice(&alpha);
        out.extend_from_slice(&encode_color_block(texels, false));
    })
}

/// Writes a `DXT5` file.
pub fn encode_bc3(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    encode_blocks(width, height, rgba, *b"DXT5", |texels, out| {
        out.extend_from_slice(&encode_alpha_block(&channel(texels, 3)));
        out.extend_from_slice(&encode_color_block(texels, false));
    })
}

/// Writes an `ATI2` file from the red and green channels. Blue and alpha
/// are not stored.
pub fn encode_bc5(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    encode_blocks(width, height, rgba, *b"ATI2", |texels, out| {
        out.extend_from_slice(&encode_alpha_block(&channel(texels, 0)));
        out.extend_from_slice(&encode_alpha_block(&channel(texels, 1)));
    })
}
