//! Textures: the source trait, mip chains and the reference sampler.

use std::borrow::Cow;
use std::collections::BTreeMap;

/// Largest accepted texture side. Larger images count as missing.
pub const MAX_TEXTURE_SIDE: u32 = 8192;

/// An RGBA8 image: row-major, top row first, straight alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

impl Texture {
    /// Whether the size fields match the buffer and the limits.
    pub fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= MAX_TEXTURE_SIDE
            && self.height <= MAX_TEXTURE_SIDE
            && (self.width as usize) * (self.height as usize) * 4 == self.rgba.len()
    }

    /// A one-texel texture.
    pub fn solid(rgba: [u8; 4]) -> Self {
        Texture {
            width: 1,
            height: 1,
            rgba: rgba.to_vec(),
        }
    }
}

/// Looks up textures by the source path a material names (lowercase, `\`
/// separators). `None` means the texture is missing.
pub trait TextureSource {
    /// The image for `path`, or `None` when there is none.
    fn texture(&self, path: &str) -> Option<Cow<'_, Texture>>;
}

impl TextureSource for BTreeMap<String, Texture> {
    fn texture(&self, path: &str) -> Option<Cow<'_, Texture>> {
        self.get(path).map(Cow::Borrowed)
    }
}

/// A source with no textures at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTextures;

impl TextureSource for NoTextures {
    fn texture(&self, _path: &str) -> Option<Cow<'_, Texture>> {
        None
    }
}

/// One level of a mip chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Level {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A texture and its box-filtered reductions down to 1 × 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MipChain {
    pub levels: Vec<Level>,
}

impl MipChain {
    /// Builds the chain. Each level halves each side (rounding down, never
    /// below 1) and averages 2 × 2 blocks, colour weighted by alpha so that
    /// transparent texels do not bleed their colour.
    pub fn new(texture: &Texture) -> Self {
        let mut levels = vec![Level {
            width: texture.width,
            height: texture.height,
            rgba: texture.rgba.clone(),
        }];
        while let Some(last) = levels.last() {
            if last.width == 1 && last.height == 1 {
                break;
            }
            let next = reduce(last);
            levels.push(next);
        }
        MipChain { levels }
    }

    /// The level to sample for a triangle whose texels-per-sample ratio
    /// (square root of the area ratio) is `ratio`.
    pub fn level_for(&self, ratio: f64) -> u32 {
        let max = self.levels.len().saturating_sub(1) as u32;
        if !(ratio.is_finite() && ratio > 1.0) {
            return if ratio.is_infinite() { max } else { 0 };
        }
        (ratio.log2().floor() as u32).min(max)
    }

    /// Bilinear sample with repeat wrap at texel centres, in 0..1.
    pub fn sample(&self, level: u32, uv: [f32; 2]) -> [f32; 4] {
        let Some(l) = self.levels.get(level as usize).or(self.levels.last()) else {
            return [0.5, 0.5, 0.5, 1.0];
        };
        let (w, h) = (l.width as i64, l.height as i64);
        let x = uv[0] * l.width as f32 - 0.5;
        let y = uv[1] * l.height as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (xi, yi) = (x0 as i64, y0 as i64);
        let (xa, xb) = (xi.rem_euclid(w) as usize, (xi + 1).rem_euclid(w) as usize);
        let (ya, yb) = (yi.rem_euclid(h) as usize, (yi + 1).rem_euclid(h) as usize);
        let stride = l.width as usize;
        let texel = |x: usize, y: usize| {
            let i = (y * stride + x) * 4;
            let p = &l.rgba[i..i + 4];
            [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
        };
        let (t00, t10, t01, t11) = (texel(xa, ya), texel(xb, ya), texel(xa, yb), texel(xb, yb));
        let mut out = [0.0; 4];
        for (c, value) in out.iter_mut().enumerate() {
            let top = t00[c] + (t10[c] - t00[c]) * fx;
            let bottom = t01[c] + (t11[c] - t01[c]) * fx;
            *value = (top + (bottom - top) * fy) * (1.0 / 255.0);
        }
        out
    }
}

fn reduce(src: &Level) -> Level {
    let width = (src.width / 2).max(1);
    let height = (src.height / 2).max(1);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    let sw = src.width as usize;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let xs = [(2 * x).min(sw - 1), (2 * x + 1).min(sw - 1)];
            let ys = [
                (2 * y).min(src.height as usize - 1),
                (2 * y + 1).min(src.height as usize - 1),
            ];
            let mut sum = [0u32; 4];
            let mut weighted = [0u32; 3];
            for &yy in &ys {
                for &xx in &xs {
                    let i = (yy * sw + xx) * 4;
                    let a = u32::from(src.rgba[i + 3]);
                    for c in 0..3 {
                        let v = u32::from(src.rgba[i + c]);
                        sum[c] += v;
                        weighted[c] += v * a;
                    }
                    sum[3] += a;
                }
            }
            let alpha_sum = sum[3];
            for c in 0..3 {
                let v = (weighted[c] + alpha_sum / 2)
                    .checked_div(alpha_sum)
                    .unwrap_or((sum[c] + 2) / 4);
                rgba.push(v as u8);
            }
            rgba.push(((alpha_sum + 2) / 4) as u8);
        }
    }
    Level {
        width,
        height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(size: u32) -> Texture {
        let mut rgba = Vec::new();
        for y in 0..size {
            for x in 0..size {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Texture {
            width: size,
            height: size,
            rgba,
        }
    }

    #[test]
    fn mip_chain_reaches_one_texel() {
        let chain = MipChain::new(&checker(16));
        let sizes: Vec<u32> = chain.levels.iter().map(|l| l.width).collect();
        assert_eq!(sizes, [16, 8, 4, 2, 1]);
        // A checkerboard averages to mid-grey.
        assert_eq!(&chain.levels[1].rgba[..4], &[128, 128, 128, 255]);
        let odd = MipChain::new(&Texture {
            width: 5,
            height: 1,
            rgba: vec![9; 20],
        });
        let sizes: Vec<(u32, u32)> = odd.levels.iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(sizes, [(5, 1), (2, 1), (1, 1)]);
    }

    #[test]
    fn transparent_texels_do_not_bleed() {
        let tex = Texture {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 0],
        };
        let chain = MipChain::new(&tex);
        assert_eq!(chain.levels[1].rgba, [255, 0, 0, 128]);
    }

    #[test]
    fn level_choice_follows_the_ratio() {
        let chain = MipChain::new(&checker(1024));
        assert_eq!(chain.level_for(0.5), 0);
        assert_eq!(chain.level_for(1.0), 0);
        assert_eq!(chain.level_for(2.5), 1);
        assert_eq!(chain.level_for(10.0), 3);
        assert_eq!(chain.level_for(1.0e9), 10);
        assert_eq!(chain.level_for(f64::NAN), 0);
    }

    #[test]
    fn sampling_wraps_and_hits_texel_centres() {
        let tex = Texture {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 255, 255, 255, 255],
        };
        let chain = MipChain::new(&tex);
        assert_eq!(chain.sample(0, [0.25, 0.5])[0], 0.0);
        assert_eq!(chain.sample(0, [0.75, 0.5])[0], 1.0);
        assert_eq!(chain.sample(0, [1.25, 0.5])[0], 0.0);
        // Halfway between the right texel and the wrapped left one.
        assert!((chain.sample(0, [1.0, 0.5])[0] - 0.5).abs() < 1e-6);
    }
}
