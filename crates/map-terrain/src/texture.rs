//! Land textures prefiltered for the map's fixed pixel footprint.
//!
//! The map is a parallel projection of a flat plane, so one screen pixel
//! covers the same source area everywhere. Each texture is therefore
//! filtered once, to the mip level that matches one ground sub-sample, and
//! then sampled bilinearly with wrap-around.
//!
//! **Orientation.** A texture repeats every `repeat` source units on both
//! axes, aligned to the source origin: texel column `u` grows east with
//! `x`, texel row `v` grows south (image top row at the north edge of each
//! repeat), so `u = x / repeat · width`, `v = −y / repeat · height`.

use mir::Image;

/// Largest texture side accepted; anything larger is treated as missing.
pub(crate) const MAX_SIDE: u32 = 8192;

/// One land texture at the level that matches the pixel footprint.
#[derive(Debug, Clone)]
pub(crate) struct Prefiltered {
    width: usize,
    height: usize,
    texels: Vec<[f32; 3]>,
    /// Texels per source unit, along `u` and `v`.
    scale: [f64; 2],
    /// Mean colour of the full-resolution image, 0 to 255.
    pub mean: [f32; 3],
}

struct Level {
    width: usize,
    height: usize,
    texels: Vec<[f32; 3]>,
}

impl Level {
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        self.texels[y * self.width + x]
    }

    /// Bilinear sample at continuous texel coordinates (texel centres at
    /// `i + 0.5`), wrapping on both axes.
    fn bilinear(&self, u: f64, v: f64) -> [f32; 3] {
        LevelRef {
            width: self.width,
            height: self.height,
            texels: &self.texels,
        }
        .bilinear(u, v)
    }

    /// Unsharp mask: adds `amount` of (texel − 3 × 3 box blur) to every
    /// texel, wrapping at the edges, clamped to 0 to 255.
    fn sharpen(&mut self, amount: f32) {
        let (w, h) = (self.width, self.height);
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            let ys = [(y + h - 1) % h, y, (y + 1) % h];
            for x in 0..w {
                let xs = [(x + w - 1) % w, x, (x + 1) % w];
                let mut blur = [0.0f32; 3];
                for &sy in &ys {
                    for &sx in &xs {
                        let t = self.at(sx, sy);
                        for k in 0..3 {
                            blur[k] += t[k];
                        }
                    }
                }
                let t = self.at(x, y);
                out.push(
                    [0, 1, 2].map(|k| (t[k] + amount * (t[k] - blur[k] / 9.0)).clamp(0.0, 255.0)),
                );
            }
        }
        self.texels = out;
    }

    /// The next mip level: 2 × 2 box filter, wrapping odd edges.
    fn half(&self) -> Level {
        let width = (self.width / 2).max(1);
        let height = (self.height / 2).max(1);
        let mut texels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let xs = [(2 * x) % self.width, (2 * x + 1) % self.width];
                let ys = [(2 * y) % self.height, (2 * y + 1) % self.height];
                let mut sum = [0.0f32; 3];
                for &sy in &ys {
                    for &sx in &xs {
                        let t = self.at(sx, sy);
                        for k in 0..3 {
                            sum[k] += t[k];
                        }
                    }
                }
                texels.push(sum.map(|v| v * 0.25));
            }
        }
        Level {
            width,
            height,
            texels,
        }
    }
}

impl Prefiltered {
    /// Prefilters `image` for sub-samples that each cover `footprint`
    /// source units (the square root of their area) with a texture that
    /// repeats every `repeat` units. `lod_bias` moves the chosen mip level
    /// (negative is finer, so sharper and noisier), and `sharpen` adds that
    /// much of the level's own detail (the level minus its 3 × 3 blur) back
    /// on top; both act on the texture, never on a tile, so squares still
    /// meet without a seam. `None` when the image is empty, too large, or
    /// its buffer does not match its size.
    pub fn new(
        image: &Image,
        repeat: f64,
        footprint: f64,
        lod_bias: f64,
        sharpen: f32,
    ) -> Option<Self> {
        if image.width == 0
            || image.height == 0
            || image.width > MAX_SIDE
            || image.height > MAX_SIDE
            || image.rgba.len() != image.width as usize * image.height as usize * 4
            || !(repeat.is_finite() && repeat > 0.0)
        {
            return None;
        }
        let width = image.width as usize;
        let height = image.height as usize;
        let texels: Vec<[f32; 3]> = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
            .collect();
        let mut sum = [0.0f64; 3];
        for t in &texels {
            for k in 0..3 {
                sum[k] += f64::from(t[k]);
            }
        }
        let count = texels.len() as f64;
        let mean = sum.map(|v| (v / count) as f32);
        let mut level = Level {
            width,
            height,
            texels,
        };
        // Texels per sub-sample at full resolution; its log2 is the mip
        // level. Fractional levels blend the two neighbours.
        let texels_per_sample = footprint * (width.max(height) as f64) / repeat;
        let target = if texels_per_sample > 1.0 {
            (texels_per_sample.log2() + lod_bias).max(0.0)
        } else {
            0.0
        };
        let mut current = 0.0;
        while current + 1.0 <= target && (level.width > 1 || level.height > 1) {
            level = level.half();
            current += 1.0;
        }
        let fraction = (target - current).clamp(0.0, 1.0) as f32;
        if fraction > 0.0 && (level.width > 1 || level.height > 1) {
            let coarse = level.half();
            let sx = coarse.width as f64 / level.width as f64;
            let sy = coarse.height as f64 / level.height as f64;
            for y in 0..level.height {
                for x in 0..level.width {
                    let c = coarse.bilinear((x as f64 + 0.5) * sx, (y as f64 + 0.5) * sy);
                    let t = &mut level.texels[y * level.width + x];
                    for k in 0..3 {
                        t[k] += (c[k] - t[k]) * fraction;
                    }
                }
            }
        }
        if sharpen > 0.0 {
            level.sharpen(sharpen);
        }
        Some(Prefiltered {
            width: level.width,
            height: level.height,
            scale: [level.width as f64 / repeat, level.height as f64 / repeat],
            texels: level.texels,
            mean,
        })
    }

    /// The filtered colour (0 to 255) at a source point.
    pub fn sample(&self, x: f64, y: f64) -> [f32; 3] {
        let level = LevelRef {
            width: self.width,
            height: self.height,
            texels: &self.texels,
        };
        level.bilinear(x * self.scale[0], -y * self.scale[1])
    }
}

/// A borrowed view with the same sampling as [`Level`].
struct LevelRef<'a> {
    width: usize,
    height: usize,
    texels: &'a [[f32; 3]],
}

impl LevelRef<'_> {
    fn bilinear(&self, u: f64, v: f64) -> [f32; 3] {
        let fu = u - 0.5;
        let fv = v - 0.5;
        let u0 = fu.floor();
        let v0 = fv.floor();
        let au = (fu - u0) as f32;
        let av = (fv - v0) as f32;
        let x0 = (u0 as i64).rem_euclid(self.width as i64) as usize;
        let y0 = (v0 as i64).rem_euclid(self.height as i64) as usize;
        let x1 = if x0 + 1 == self.width { 0 } else { x0 + 1 };
        let y1 = if y0 + 1 == self.height { 0 } else { y0 + 1 };
        let row0 = y0 * self.width;
        let row1 = y1 * self.width;
        let a = self.texels[row0 + x0];
        let b = self.texels[row0 + x1];
        let c = self.texels[row1 + x0];
        let d = self.texels[row1 + x1];
        let mut out = [0.0f32; 3];
        for k in 0..3 {
            let top = a[k] + (b[k] - a[k]) * au;
            let bottom = c[k] + (d[k] - c[k]) * au;
            out[k] = top + (bottom - top) * av;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = f(x, y);
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        Image {
            width,
            height,
            rgba,
        }
    }

    #[test]
    fn orientation_puts_the_top_row_north_and_column_zero_west() {
        // Top half red, bottom half blue; left half bright, right half dark.
        let img = image(8, 8, |x, y| {
            let v = if x < 4 { 250 } else { 50 };
            if y < 4 { [v, 0, 0] } else { [0, 0, v] }
        });
        let t = Prefiltered::new(&img, 512.0, 1.0, 0.0, 0.0).expect("texture");
        // y = 0.75 R is in the north half of the repeat [0, R]: top rows.
        let north = t.sample(100.0, 0.75 * 512.0);
        let south = t.sample(100.0, 0.25 * 512.0);
        assert!(north[0] > 200.0 && north[2] < 10.0, "{north:?}");
        assert!(south[2] > 200.0 && south[0] < 10.0, "{south:?}");
        let west = t.sample(0.2 * 512.0, 0.75 * 512.0);
        let east = t.sample(0.7 * 512.0, 0.75 * 512.0);
        assert!(west[0] > east[0] + 100.0);
        // Repeats every 512 units, in both directions.
        assert_eq!(
            t.sample(100.0, 300.0),
            t.sample(100.0 - 5120.0, 300.0 + 1024.0)
        );
    }

    #[test]
    fn large_footprints_pick_a_blurred_level() {
        let img = image(
            64,
            64,
            |x, y| {
                if (x + y) % 2 == 0 { [255; 3] } else { [0; 3] }
            },
        );
        let sharp = Prefiltered::new(&img, 64.0, 0.5, 0.0, 0.0).expect("texture");
        let blurred = Prefiltered::new(&img, 64.0, 8.0, 0.0, 0.0).expect("texture");
        assert_eq!(sharp.width, 64);
        assert_eq!(blurred.width, 8);
        let c = blurred.sample(10.3, 20.7);
        assert!((c[0] - 127.5).abs() < 1.0, "{c:?}");
        assert!((blurred.mean[0] - 127.5).abs() < 0.01);
    }

    #[test]
    fn a_finer_level_and_sharpening_add_contrast_and_still_wrap() {
        // Soft stripes 8 texels wide.
        let img = image(64, 64, |x, _| {
            let v = (127.5 + 100.0 * (f64::from(x) * std::f64::consts::PI / 4.0).sin()) as u8;
            [v; 3]
        });
        let spread = |t: &Prefiltered| {
            let values: Vec<f32> = (0..64)
                .map(|i| t.sample(f64::from(i) + 0.5, 3.0)[0])
                .collect();
            let (lo, hi) = values
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            hi - lo
        };
        // A footprint of 4 texels picks level 2 by default.
        let plain = Prefiltered::new(&img, 64.0, 4.0, 0.0, 0.0).expect("texture");
        let finer = Prefiltered::new(&img, 64.0, 4.0, -1.0, 0.0).expect("texture");
        let sharper = Prefiltered::new(&img, 64.0, 4.0, 0.0, 1.0).expect("texture");
        assert_eq!((plain.width, finer.width), (16, 32));
        assert!(spread(&finer) > spread(&plain) + 10.0);
        assert!(spread(&sharper) > spread(&plain) + 5.0);
        // A bias cannot go below the full-resolution level.
        let most = Prefiltered::new(&img, 64.0, 4.0, -9.0, 0.0).expect("texture");
        assert_eq!(most.width, 64);
        // Still a function of the source point, one repeat apart.
        for t in [&finer, &sharper] {
            assert_eq!(t.sample(10.25, 20.5), t.sample(10.25 + 64.0, 20.5 - 128.0));
        }
    }

    #[test]
    fn bad_images_are_refused() {
        let mut img = image(4, 4, |_, _| [1, 2, 3]);
        img.rgba.pop();
        assert!(Prefiltered::new(&img, 512.0, 1.0, 0.0, 0.0).is_none());
        let empty = Image {
            width: 0,
            height: 4,
            rgba: Vec::new(),
        };
        assert!(Prefiltered::new(&empty, 512.0, 1.0, 0.0, 0.0).is_none());
        let ok = image(3, 5, |_, _| [9, 9, 9]);
        let t = Prefiltered::new(&ok, 512.0, 400.0, 0.0, 0.0).expect("odd sizes work");
        assert_eq!(t.sample(-1.0, 7.0), [9.0, 9.0, 9.0]);
    }
}
