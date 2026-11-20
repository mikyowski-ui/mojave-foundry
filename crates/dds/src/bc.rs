//! Block decoders. Every block covers 4 x 4 texels, stored row by row from
//! the top-left texel; index bits are read from the least significant end.

pub(crate) type Texels = [[u8; 4]; 16];

/// Widens a 565 colour to 8 bits per channel by repeating the high bits.
pub(crate) fn rgb565(c: u16) -> [u8; 3] {
    let r = u32::from(c >> 11) & 0x1F;
    let g = u32::from(c >> 5) & 0x3F;
    let b = u32::from(c) & 0x1F;
    [
        ((r << 3) | (r >> 2)) as u8,
        ((g << 2) | (g >> 4)) as u8,
        ((b << 3) | (b >> 2)) as u8,
    ]
}

/// The four colours of a colour block.
///
/// With `punch_through` (BC1 only) and `c0 <= c1`, the third colour is the
/// midpoint and the fourth is transparent black. Otherwise the block has
/// two interpolated colours at one third and two thirds.
pub(crate) fn color_palette(c0: u16, c1: u16, punch_through: bool) -> [[u8; 4]; 4] {
    let a = rgb565(c0);
    let b = rgb565(c1);
    let mix = |wa: u16, wb: u16, div: u16| -> [u8; 4] {
        let ch = |i: usize| ((wa * u16::from(a[i]) + wb * u16::from(b[i])) / div) as u8;
        [ch(0), ch(1), ch(2), 255]
    };
    let first = [a[0], a[1], a[2], 255];
    let second = [b[0], b[1], b[2], 255];
    if punch_through && c0 <= c1 {
        [first, second, mix(1, 1, 2), [0, 0, 0, 0]]
    } else {
        [first, second, mix(2, 1, 3), mix(1, 2, 3)]
    }
}

/// The eight values of an interpolated alpha block.
///
/// With `a0 > a1` there are six interpolated values. Otherwise there are
/// four, followed by 0 and 255.
pub(crate) fn alpha_palette(a0: u8, a1: u8) -> [u8; 8] {
    let (x, y) = (u16::from(a0), u16::from(a1));
    let mut out = [a0, a1, 0, 0, 0, 0, 0, 255];
    if a0 > a1 {
        for i in 1..7u16 {
            out[usize::from(i) + 1] = (((7 - i) * x + i * y) / 7) as u8;
        }
    } else {
        for i in 1..5u16 {
            out[usize::from(i) + 1] = (((5 - i) * x + i * y) / 5) as u8;
        }
    }
    out
}

fn u16_le(bytes: &[u8]) -> u16 {
    u16::from(bytes[0]) | (u16::from(bytes[1]) << 8)
}

/// Decodes an 8-byte colour block into `texels`.
fn color_block(block: &[u8], punch_through: bool, texels: &mut Texels) {
    let palette = color_palette(u16_le(&block[0..2]), u16_le(&block[2..4]), punch_through);
    for (i, texel) in texels.iter_mut().enumerate() {
        let index = (block[4 + i / 4] >> (2 * (i % 4))) & 3;
        *texel = palette[usize::from(index)];
    }
}

/// Decodes an 8-byte interpolated alpha block into 16 values.
fn alpha_block(block: &[u8]) -> [u8; 16] {
    let palette = alpha_palette(block[0], block[1]);
    let bits = block[2..8]
        .iter()
        .enumerate()
        .fold(0u64, |acc, (i, &b)| acc | (u64::from(b) << (8 * i)));
    let mut out = [0u8; 16];
    for (i, value) in out.iter_mut().enumerate() {
        *value = palette[((bits >> (3 * i)) & 7) as usize];
    }
    out
}

pub(crate) fn bc1_block(block: &[u8], texels: &mut Texels) {
    color_block(block, true, texels);
}

pub(crate) fn bc2_block(block: &[u8], texels: &mut Texels) {
    color_block(&block[8..16], false, texels);
    for (i, texel) in texels.iter_mut().enumerate() {
        let nibble = (block[i / 2] >> (4 * (i % 2))) & 0xF;
        texel[3] = nibble * 17;
    }
}

pub(crate) fn bc3_block(block: &[u8], texels: &mut Texels) {
    color_block(&block[8..16], false, texels);
    let alpha = alpha_block(&block[0..8]);
    for (texel, a) in texels.iter_mut().zip(alpha) {
        texel[3] = a;
    }
}

/// Blue of a unit normal whose X and Y are stored in red and green as
/// `value / 255 * 2 - 1`, stored back the same way.
pub(crate) fn normal_blue(r: u8, g: u8) -> u8 {
    let x = f64::from(r) / 255.0 * 2.0 - 1.0;
    let y = f64::from(g) / 255.0 * 2.0 - 1.0;
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    ((z * 0.5 + 0.5) * 255.0).round() as u8
}

pub(crate) fn bc5_block(block: &[u8], texels: &mut Texels) {
    let red = alpha_block(&block[0..8]);
    let green = alpha_block(&block[8..16]);
    for (i, texel) in texels.iter_mut().enumerate() {
        *texel = [red[i], green[i], normal_blue(red[i], green[i]), 255];
    }
}

/// Decodes `data`, which holds exactly the blocks of a `width` x `height`
/// image, into `rgba`. Texels of edge blocks outside the image are dropped.
pub(crate) fn decode(
    data: &[u8],
    width: usize,
    height: usize,
    block_len: usize,
    rgba: &mut [u8],
    decode_block: fn(&[u8], &mut Texels),
) {
    let blocks_wide = width.div_ceil(4);
    let mut texels: Texels = [[0; 4]; 16];
    for (index, block) in data.chunks_exact(block_len).enumerate() {
        decode_block(block, &mut texels);
        let x0 = (index % blocks_wide) * 4;
        let y0 = (index / blocks_wide) * 4;
        for y in y0..(y0 + 4).min(height) {
            for x in x0..(x0 + 4).min(width) {
                let at = (y * width + x) * 4;
                rgba[at..at + 4].copy_from_slice(&texels[(y - y0) * 4 + (x - x0)]);
            }
        }
    }
}
