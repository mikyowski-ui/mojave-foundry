//! Quantizes RGBA renders to the Fallout 2 palette with reserved-index protection.
//!
//! Images are the plain buffers of `docs/contracts.md` section 5: RGBA8
//! row-major with straight alpha, and indexed row-major with index 0
//! transparent. A pixel with alpha below [`ALPHA_THRESHOLD`] becomes index 0;
//! every other pixel becomes the nearest index in [`PaletteMap::allowed`],
//! so index 0, the colour-cycling ranges, the red pulse entry and unmapped
//! entries are never produced for opaque pixels. Colour distance is
//! Euclidean distance in Oklab (see [`PaletteMap`]).
//!
//! Everything is deterministic: identical inputs give identical bytes on
//! every machine (the colour maths avoids platform maths functions).

mod color;
pub mod fixture;
mod map;

pub use color::oklab_from_rgb8;
pub use map::{PaletteMap, Reserved};

use vault_formats::Palette;

/// Alpha at or above this value is opaque.
pub const ALPHA_THRESHOLD: u8 = 128;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum QuantizeError {
    #[error("a {width} x {height} image needs {expected} bytes, the buffer holds {actual}")]
    BufferLength {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
    #[error("a {width} x {height} image is too large to address")]
    TooLarge { width: u32, height: u32 },
    #[error("dither strength {0} is not a finite, non-negative number")]
    InvalidStrength(f32),
    #[error("outline factor {0} is not in 0..=1")]
    InvalidFactor(f32),
    #[error("the palette leaves no index that an opaque pixel may use")]
    NoAllowedIndex,
    #[error("rim options: {0}")]
    InvalidRim(String),
}

/// How [`quantize`] treats colours between palette entries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dither {
    /// Nearest colour only. For sprites.
    None,
    /// An 8 × 8 Bayer threshold added to Oklab lightness before the lookup.
    /// For static ground. `strength` is the peak-to-peak span of the
    /// lightness offset in Oklab `L` units (`L` runs 0 to 1); the offset at a
    /// pixel depends only on its absolute coordinates, so tiles quantized
    /// with [`quantize_at`] meet seamlessly. Zero is identical to `None`.
    Ordered { strength: f32 },
}

impl Dither {
    /// A mild default: about one step of a fine grey ramp.
    pub const DEFAULT_STRENGTH: f32 = 0.04;

    /// Ordered dithering at [`Dither::DEFAULT_STRENGTH`].
    pub const fn ordered() -> Self {
        Dither::Ordered {
            strength: Self::DEFAULT_STRENGTH,
        }
    }
}

/// The 8 × 8 Bayer index matrix, `BAYER[y][x]`, values 0..64.
pub const BAYER: [[u8; 8]; 8] = bayer();

const fn bayer() -> [[u8; 8]; 8] {
    let mut matrix = [[0u8; 8]; 8];
    let mut y = 0;
    while y < 8 {
        let mut x = 0;
        while x < 8 {
            let mut value = 0u8;
            let mut bit = 0;
            while bit < 3 {
                let shift = 2 * (2 - bit);
                value |= ((((x ^ y) >> bit) & 1) << (shift + 1)) as u8;
                value |= (((y >> bit) & 1) << shift) as u8;
                bit += 1;
            }
            matrix[y][x] = value;
            x += 1;
        }
        y += 1;
    }
    matrix
}

/// Quantizes an RGBA8 image whose pixel `(0, 0)` sits at absolute position
/// `(0, 0)`. Equivalent to [`quantize_at`] with origin `[0, 0]`.
pub fn quantize(
    rgba: &[u8],
    width: u32,
    height: u32,
    map: &PaletteMap,
    dither: Dither,
) -> Result<Vec<u8>, QuantizeError> {
    quantize_at(rgba, width, height, map, dither, [0, 0])
}

/// Quantizes an RGBA8 image whose pixel `(0, 0)` sits at absolute position
/// `origin`. Only ordered dithering uses the position.
pub fn quantize_at(
    rgba: &[u8],
    width: u32,
    height: u32,
    map: &PaletteMap,
    dither: Dither,
    origin: [i32; 2],
) -> Result<Vec<u8>, QuantizeError> {
    let (w, pixels) = check_buffer(rgba.len(), width, height, 4)?;
    let strength = match dither {
        Dither::None => 0.0,
        Dither::Ordered { strength } => {
            if !strength.is_finite() || strength < 0.0 {
                return Err(QuantizeError::InvalidStrength(strength));
            }
            f64::from(strength)
        }
    };
    let mut out = Vec::with_capacity(pixels);
    if pixels == 0 {
        return Ok(out);
    }
    for (y, row) in rgba.chunks_exact(w * 4).enumerate() {
        let by = (y as i64 + i64::from(origin[1])).rem_euclid(8) as usize;
        for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
            let index = if px[3] < ALPHA_THRESHOLD {
                0
            } else if strength == 0.0 {
                map.nearest([px[0], px[1], px[2]])
            } else {
                let bx = (x as i64 + i64::from(origin[0])).rem_euclid(8) as usize;
                let shift = ((f64::from(BAYER[by][bx]) + 0.5) / 64.0 - 0.5) * strength;
                map.nearest_shifted([px[0], px[1], px[2]], shift)
            };
            out.push(index);
        }
    }
    Ok(out)
}

/// Options for [`outline_with`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutlineOptions {
    /// Linear-light multiplier applied to an edge pixel's colour, `0..=1`.
    pub factor: f32,
}

impl Default for OutlineOptions {
    fn default() -> Self {
        Self { factor: 0.5 }
    }
}

/// Darkens the 1-pixel inner edge of an indexed image in place, with
/// [`OutlineOptions::default`].
pub fn outline(
    indexed: &mut [u8],
    width: u32,
    height: u32,
    map: &PaletteMap,
) -> Result<(), QuantizeError> {
    outline_with(indexed, width, height, map, &OutlineOptions::default())
}

/// Darkens the 1-pixel inner edge of an indexed image in place.
///
/// A pixel is an edge when it is opaque (non-zero) and one of its four
/// neighbours is index 0 or lies outside the image. An edge pixel becomes
/// the nearest allowed index to its own colour with linear light scaled by
/// `factor`; when that entry is not darker in Oklab lightness than the
/// original, the original stays (a reserved original is still replaced).
/// Transparency never changes and no reserved index is written.
pub fn outline_with(
    indexed: &mut [u8],
    width: u32,
    height: u32,
    map: &PaletteMap,
    options: &OutlineOptions,
) -> Result<(), QuantizeError> {
    let (w, pixels) = check_buffer(indexed.len(), width, height, 1)?;
    let factor = options.factor;
    if !(0.0..=1.0).contains(&factor) {
        return Err(QuantizeError::InvalidFactor(factor));
    }
    if pixels == 0 {
        return Ok(());
    }
    let factor = f64::from(factor);
    let darker: [u8; 256] = std::array::from_fn(|i| {
        let index = i as u8;
        let linear = map.colour(index).map(|c| color::linear8(c) * factor);
        let candidate = map.nearest_linear(linear);
        if map.is_allowed(index) && map.oklab(candidate)[0] >= map.oklab(index)[0] {
            index
        } else {
            candidate
        }
    });
    let h = pixels / w;
    let transparent = |indexed: &[u8], x: usize, y: usize, dx: isize, dy: isize| match (
        x.checked_add_signed(dx),
        y.checked_add_signed(dy),
    ) {
        (Some(nx), Some(ny)) if nx < w && ny < h => indexed[ny * w + nx] == 0,
        _ => true,
    };
    // Edges are found against the input; replacing a non-zero index with
    // another non-zero one cannot change what a later pixel sees.
    for y in 0..h {
        for x in 0..w {
            let at = y * w + x;
            if indexed[at] == 0 {
                continue;
            }
            let edge = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .into_iter()
                .any(|(dx, dy)| transparent(indexed, x, y, dx, dy));
            if edge {
                indexed[at] = darker[usize::from(indexed[at])];
            }
        }
    }
    Ok(())
}

/// Options for [`rim`], the outline of map sprites and critters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RimOptions {
    /// Linear-light multiplier for the rim colour, `0..=4`: below 1 a dark
    /// outline, above 1 a highlight.
    pub factor: f32,
    /// Rim width in pixels, 1 or 2 (steps between 4-neighbours).
    pub width: u8,
    /// `false`: the rim is the opaque pixels within `width` of the
    /// silhouette's edge (the image keeps its size). `true`: the rim is
    /// drawn around the silhouette, on transparent pixels within `width` of
    /// it, and the image grows by `width` on every side.
    pub outer: bool,
}

impl RimOptions {
    /// The inner 1-pixel darkening of [`outline_with`] at `factor`.
    pub const fn inner(factor: f32) -> Self {
        RimOptions {
            factor,
            width: 1,
            outer: false,
        }
    }

    /// A darkened outline drawn around the silhouette, one pixel wide.
    pub const fn outer(factor: f32) -> Self {
        RimOptions {
            factor,
            width: 1,
            outer: true,
        }
    }

    fn validate(&self) -> Result<(), QuantizeError> {
        if !(self.factor.is_finite() && (0.0..=4.0).contains(&self.factor)) {
            return Err(QuantizeError::InvalidRim(format!(
                "factor {} is not in 0..=4",
                self.factor
            )));
        }
        if !(1..=2).contains(&self.width) {
            return Err(QuantizeError::InvalidRim(format!(
                "width {} is not 1 or 2",
                self.width
            )));
        }
        Ok(())
    }
}

impl Default for RimOptions {
    /// The gallery outline: inner, 1 pixel, linear light halved.
    fn default() -> Self {
        RimOptions::inner(0.5)
    }
}

/// An image with its rim, and where its pixel `(0, 0)` lies relative to
/// the input's (`[-width, -width]` for an outer rim, else `[0, 0]`): add
/// `offset` to the input's origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rimmed {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub offset: [i32; 2],
}

/// Outlines an indexed image (index 0 transparent) for map readability:
/// interactive map sprites (items, containers, doors, terminals) and
/// critters. A rim pixel takes the nearest allowed index to the linear
/// light of its source colour times `factor`: its own colour for an inner
/// rim, the colour of the nearest opaque pixel (4-neighbour steps, then
/// left, right, up, down) for an outer one. An inner rim pixel whose
/// candidate is not darker (a highlight: not lighter) in Oklab lightness
/// keeps its index, unless that index is reserved. No reserved index and
/// no index 0 is written on an opaque pixel. With width 1, `outer` false
/// and a factor up to 1 this is exactly [`outline_with`].
pub fn rim(
    indexed: &[u8],
    width: u32,
    height: u32,
    map: &PaletteMap,
    options: &RimOptions,
) -> Result<Rimmed, QuantizeError> {
    let (w, pixels) = check_buffer(indexed.len(), width, height, 1)?;
    options.validate()?;
    let h = pixels.checked_div(w).unwrap_or(0);
    let factor = f64::from(options.factor);
    let lighten = factor > 1.0;
    let scaled: [u8; 256] = std::array::from_fn(|i| {
        let linear = map.colour(i as u8).map(|c| color::linear8(c) * factor);
        map.nearest_linear(linear)
    });
    let inner: [u8; 256] = std::array::from_fn(|i| {
        let index = i as u8;
        let candidate = scaled[i];
        let (old, new) = (map.oklab(index)[0], map.oklab(candidate)[0]);
        let no_gain = if lighten { new <= old } else { new >= old };
        if map.is_allowed(index) && no_gain {
            index
        } else {
            candidate
        }
    });
    let grow = if options.outer {
        usize::from(options.width)
    } else {
        0
    };
    let (ow, oh) = (w + 2 * grow, h + 2 * grow);
    let (Ok(out_w), Ok(out_h)) = (u32::try_from(ow), u32::try_from(oh)) else {
        return Err(QuantizeError::TooLarge { width, height });
    };
    let mut out = vec![0u8; ow * oh];
    for y in 0..h {
        out[(y + grow) * ow + grow..(y + grow) * ow + grow + w]
            .copy_from_slice(&indexed[y * w..(y + 1) * w]);
    }
    // Steps to the nearest transparent pixel (inner) or opaque pixel
    // (outer), up to the rim width, with the source pixel for an outer rim.
    let steps: [(isize, isize); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    let at = |x: usize, y: usize, dx: isize, dy: isize| -> Option<usize> {
        let nx = x.checked_add_signed(dx)?;
        let ny = y.checked_add_signed(dy)?;
        (nx < ow && ny < oh).then_some(ny * ow + nx)
    };
    let source = out.clone();
    let reach = usize::from(options.width);
    for y in 0..oh {
        for x in 0..ow {
            let here = y * ow + x;
            if options.outer {
                if source[here] != 0 {
                    continue;
                }
                // The nearest opaque pixel within the rim width.
                let mut found = None;
                'search: for d in 1..=reach as isize {
                    for &(sx, sy) in &steps {
                        if let Some(i) = at(x, y, sx * d, sy * d)
                            && source[i] != 0
                        {
                            found = Some(source[i]);
                            break 'search;
                        }
                    }
                    if d == 2 {
                        for (sx, sy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                            if let Some(i) = at(x, y, sx, sy)
                                && source[i] != 0
                            {
                                found = Some(source[i]);
                                break 'search;
                            }
                        }
                    }
                }
                if let Some(v) = found {
                    out[here] = scaled[usize::from(v)];
                }
            } else {
                if source[here] == 0 {
                    continue;
                }
                let transparent = |i: Option<usize>| i.is_none_or(|i| source[i] == 0);
                let mut edge = steps.iter().any(|&(dx, dy)| transparent(at(x, y, dx, dy)));
                if !edge && reach == 2 {
                    edge = steps
                        .iter()
                        .any(|&(dx, dy)| transparent(at(x, y, 2 * dx, 2 * dy)))
                        || [(-1, -1), (1, -1), (-1, 1), (1, 1)]
                            .iter()
                            .any(|&(dx, dy)| transparent(at(x, y, dx, dy)));
                }
                if edge {
                    out[here] = inner[usize::from(source[here])];
                }
            }
        }
    }
    let offset = -(grow as i32);
    Ok(Rimmed {
        pixels: out,
        width: out_w,
        height: out_h,
        offset: [offset, offset],
    })
}

/// Expands an indexed image to RGBA8 for previews: index 0 is transparent
/// black, every other index opaque at the palette's 8-bit display colour.
pub fn to_rgba(indexed: &[u8], palette: &Palette) -> Vec<u8> {
    let display = palette.rgba8();
    let mut out = Vec::with_capacity(indexed.len() * 4);
    for &index in indexed {
        if index == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            out.extend_from_slice(&display[usize::from(index)]);
        }
    }
    out
}

/// Calibration numbers for one quantized image.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantizeStats {
    /// Source pixels with alpha at or above [`ALPHA_THRESHOLD`].
    pub opaque_pixels: u64,
    /// Mean Oklab distance between each opaque source pixel and the display
    /// colour of its index (0 when there are no opaque pixels).
    pub mean_error: f64,
    /// Largest such distance.
    pub max_error: f64,
    /// Pixels whose transparency disagrees between source and result
    /// (opaque source with index 0, or transparent source with another).
    pub transparency_mismatches: u64,
    /// How many pixels use each index.
    pub histogram: [u64; 256],
}

impl QuantizeStats {
    /// The indices used at least once, ascending.
    pub fn used_indices(&self) -> Vec<u8> {
        (0..=255u8)
            .filter(|i| self.histogram[usize::from(*i)] > 0)
            .collect()
    }
}

/// Compares a source RGBA8 image with its quantized result.
pub fn stats(
    rgba: &[u8],
    indexed: &[u8],
    width: u32,
    height: u32,
    map: &PaletteMap,
) -> Result<QuantizeStats, QuantizeError> {
    check_buffer(rgba.len(), width, height, 4)?;
    check_buffer(indexed.len(), width, height, 1)?;
    let mut result = QuantizeStats {
        opaque_pixels: 0,
        mean_error: 0.0,
        max_error: 0.0,
        transparency_mismatches: 0,
        histogram: [0; 256],
    };
    let mut total = 0.0;
    for (px, &index) in rgba.as_chunks::<4>().0.iter().zip(indexed) {
        result.histogram[usize::from(index)] += 1;
        let opaque = px[3] >= ALPHA_THRESHOLD;
        if opaque != (index != 0) {
            result.transparency_mismatches += 1;
        }
        if opaque {
            result.opaque_pixels += 1;
            let lab = oklab_from_rgb8([px[0], px[1], px[2]]);
            let error = color::distance2(lab, map.oklab(index)).sqrt();
            total += error;
            if error > result.max_error {
                result.max_error = error;
            }
        }
    }
    if result.opaque_pixels > 0 {
        result.mean_error = total / result.opaque_pixels as f64;
    }
    Ok(result)
}

/// Validates `len` against `width × height × bytes_per_pixel`; returns the
/// width and the pixel count as `usize`.
fn check_buffer(
    len: usize,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
) -> Result<(usize, usize), QuantizeError> {
    let too_large = QuantizeError::TooLarge { width, height };
    let w = usize::try_from(width).map_err(|_| too_large.clone())?;
    let h = usize::try_from(height).map_err(|_| too_large.clone())?;
    let pixels = w.checked_mul(h).ok_or(too_large.clone())?;
    let expected = pixels.checked_mul(bytes_per_pixel).ok_or(too_large)?;
    if len != expected {
        return Err(QuantizeError::BufferLength {
            width,
            height,
            expected,
            actual: len,
        });
    }
    Ok((w, pixels))
}
