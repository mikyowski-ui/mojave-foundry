//! Uncompressed pixels described by one bit mask per channel.

/// One channel of a pixel: where it sits and how wide it is.
#[derive(Clone, Copy)]
struct Channel {
    mask: u32,
    shift: u32,
    bits: u32,
}

impl Channel {
    fn new(mask: u32) -> Self {
        Self {
            mask,
            shift: if mask == 0 { 0 } else { mask.trailing_zeros() },
            bits: mask.count_ones(),
        }
    }

    /// The channel of `pixel` as 8 bits, or `None` when the mask is empty.
    fn get(self, pixel: u32) -> Option<u8> {
        if self.mask == 0 {
            return None;
        }
        Some(widen((pixel & self.mask) >> self.shift, self.bits))
    }
}

/// Scales a `bits`-wide value to 8 bits: a narrow value repeats its bits to
/// fill the byte, a wide one keeps its top 8 bits.
pub(crate) fn widen(value: u32, bits: u32) -> u8 {
    if bits >= 8 {
        return (value >> (bits - 8)) as u8;
    }
    let mut out = 0u32;
    let mut filled = 0;
    while filled < 8 {
        out = (out << bits) | value;
        filled += bits;
    }
    (out >> (filled - 8)) as u8
}

/// Decodes `data`, which holds exactly `rgba.len() / 4` pixels of
/// `bytes_per_pixel` bytes each, little-endian, rows from the top.
///
/// `masks` is red, green, blue, alpha. A missing colour channel reads as 0
/// and missing alpha as 255. With `luminance`, green and blue copy red.
pub(crate) fn decode(
    data: &[u8],
    bytes_per_pixel: usize,
    masks: [u32; 4],
    luminance: bool,
    rgba: &mut [u8],
) {
    let [r, g, b, a] = masks.map(Channel::new);
    for (source, out) in data
        .chunks_exact(bytes_per_pixel)
        .zip(rgba.as_chunks_mut::<4>().0.iter_mut())
    {
        let pixel = source
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &byte)| acc | (u32::from(byte) << (8 * i)));
        let red = r.get(pixel).unwrap_or(0);
        let (green, blue) = if luminance {
            (red, red)
        } else {
            (g.get(pixel).unwrap_or(0), b.get(pixel).unwrap_or(0))
        };
        *out = [red, green, blue, a.get(pixel).unwrap_or(255)];
    }
}
