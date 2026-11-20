//! A synthetic Fallout 2 format palette for tests, built from arithmetic
//! alone. It is not derived from any game palette.
//!
//! Layout (6-bit channels):
//!
//! | Indices | Contents |
//! |---|---|
//! | 0 | black, the transparency key |
//! | [`GREYS`] | a 32-step grey ramp, levels 1, 3, …, 63 |
//! | [`HUE_RAMPS`] | 12 hues 30° apart × 16 value levels; even hues at saturation 0.85, odd hues at 0.5 |
//! | [`UNMAPPED`] | entries with a channel above 63 |
//! | 229..=253 | the cycling ranges: fully saturated colours at the hues between the ramps, used nowhere else |
//! | 254 | the red pulse entry: pure red, used nowhere else |
//! | [`DUPLICATE_WHITE`] | white again, the same colour as the last grey |
//!
//! The RGB555 lookup holds, for every 5-bit colour, the nearest allowed entry
//! by squared 6-bit distance.

use std::ops::RangeInclusive;

/// The grey ramp.
pub const GREYS: RangeInclusive<u8> = 1..=32;
/// The hue ramps: entry `33 + 16·hue + level`.
pub const HUE_RAMPS: RangeInclusive<u8> = 33..=224;
/// Deliberately unmapped entries.
pub const UNMAPPED: RangeInclusive<u8> = 225..=228;
/// A second white, duplicating the last grey (index 32).
pub const DUPLICATE_WHITE: u8 = 255;

/// The 256 six-bit triples of the synthetic palette (unmapped entries hold
/// channels above 63).
pub fn synthetic_triples() -> [[u8; 3]; 256] {
    let mut triples = [[0u8; 3]; 256];
    for (step, index) in GREYS.enumerate() {
        let level = (2 * step + 1) as u8;
        triples[usize::from(index)] = [level; 3];
    }
    for hue in 0..12 {
        let saturation = if hue % 2 == 0 { 0.85 } else { 0.5 };
        for level in 0..16 {
            let value = f64::from(level + 1) / 16.0;
            let index = 33 + 16 * hue + level as usize;
            triples[index] = hsv6(30.0 * hue as f64, saturation, value);
        }
    }
    let unmapped = [[64, 0, 0], [255, 255, 255], [0, 100, 0], [63, 63, 64]];
    for (index, triple) in UNMAPPED.zip(unmapped) {
        triples[usize::from(index)] = triple;
    }
    for (offset, triple) in triples[229..=253].iter_mut().enumerate() {
        let hue = 15.0 + 30.0 * (offset % 12) as f64;
        let value = match offset / 12 {
            0 => 1.0,
            1 => 0.55,
            _ => 0.3,
        };
        *triple = hsv6(hue, 1.0, value);
    }
    triples[254] = [63, 0, 0];
    triples[usize::from(DUPLICATE_WHITE)] = [63, 63, 63];
    triples
}

/// A valid 0x8300-byte palette file: the triples of [`synthetic_triples`]
/// followed by the RGB555 lookup.
pub fn synthetic_palette() -> Vec<u8> {
    let triples = synthetic_triples();
    let mut bytes = Vec::with_capacity(0x8300);
    for triple in &triples {
        bytes.extend_from_slice(triple);
    }
    let candidates: Vec<(usize, [i32; 3])> = (1..256)
        .filter(|i| !(225..=254).contains(i))
        .map(|i| (i, triples[i].map(i32::from)))
        .collect();
    for key in 0..32768usize {
        let target = [(key >> 10) & 31, (key >> 5) & 31, key & 31].map(|c| (2 * c) as i32);
        let mut best = (i32::MAX, 0usize);
        for (index, rgb) in &candidates {
            let d: i32 = (0..3).map(|c| (rgb[c] - target[c]).pow(2)).sum();
            if d < best.0 {
                best = (d, *index);
            }
        }
        bytes.push(best.1 as u8);
    }
    bytes
}

/// HSV (hue in degrees, saturation and value in `0..=1`) to a 6-bit triple.
fn hsv6(hue: f64, saturation: f64, value: f64) -> [u8; 3] {
    let chroma = value * saturation;
    let sector = (hue / 60.0).rem_euclid(6.0);
    let x = chroma * (1.0 - ((sector % 2.0) - 1.0).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    [r, g, b].map(|c| ((c + m) * 63.0).round().clamp(0.0, 63.0) as u8)
}
