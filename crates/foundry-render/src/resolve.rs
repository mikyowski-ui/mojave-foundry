//! The resolve both backends share: sub-samples to sprite pixels.

/// Supersampled colour and depth. A sub-sample is covered when its alpha is
/// at least 128; covered sub-samples carry their final straight colour.
pub(crate) struct Samples {
    /// Width in sub-samples.
    pub width: usize,
    /// Height in sub-samples.
    pub height: usize,
    pub color: Vec<[u8; 4]>,
    pub depth: Vec<f32>,
}

impl Samples {
    pub fn new(width: usize, height: usize) -> Self {
        Samples {
            width,
            height,
            color: vec![[0; 4]; width * height],
            depth: vec![f32::INFINITY; width * height],
        }
    }
}

/// Settings of the resolve.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolveSettings {
    pub supersample: u32,
    pub alpha_threshold: f32,
    pub contrast: f32,
}

/// The contrast curve as a table: `(c - 0.5) · contrast + 0.5`, clamped.
fn contrast_table(contrast: f32) -> [u8; 256] {
    let mut table = [0u8; 256];
    for (i, out) in table.iter_mut().enumerate() {
        *out = if contrast == 1.0 {
            i as u8
        } else {
            let c = ((i as f32 / 255.0 - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
            (c * 255.0 + 0.5) as u8
        };
    }
    table
}

/// Box-filters `samples` down by the supersampling factor.
///
/// Colour is averaged premultiplied (uncovered sub-samples count as
/// transparent black), so the straight colour of a pixel is the mean of its
/// covered sub-samples. A pixel is opaque when at least `alpha_threshold`
/// of its sub-samples are covered, and fully transparent otherwise. Depth is
/// the nearest covered sub-sample, `f32::INFINITY` for a transparent pixel.
pub(crate) fn resolve(samples: &Samples, settings: ResolveSettings) -> (Vec<u8>, Vec<f32>) {
    let ss = settings.supersample.max(1) as usize;
    let (width, height) = (samples.width / ss, samples.height / ss);
    let per_pixel = (ss * ss) as f32;
    let needed = ((settings.alpha_threshold * per_pixel).ceil() as u32).max(1);
    let table = contrast_table(settings.contrast);
    let mut rgba = vec![0u8; width * height * 4];
    let mut depth = vec![f32::INFINITY; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut count = 0u32;
            let mut sum = [0u32; 3];
            let mut nearest = f32::INFINITY;
            for sy in 0..ss {
                let row = (y * ss + sy) * samples.width + x * ss;
                for i in row..row + ss {
                    let c = samples.color[i];
                    if c[3] >= 128 {
                        count += 1;
                        for k in 0..3 {
                            sum[k] += u32::from(c[k]);
                        }
                        let d = samples.depth[i];
                        if d < nearest {
                            nearest = d;
                        }
                    }
                }
            }
            if count >= needed {
                let o = (y * width + x) * 4;
                for k in 0..3 {
                    let mean = (2 * sum[k] + count) / (2 * count);
                    rgba[o + k] = table[mean.min(255) as usize];
                }
                rgba[o + 3] = 255;
                depth[y * width + x] = nearest;
            }
        }
    }
    (rgba, depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ResolveSettings {
        ResolveSettings {
            supersample: 4,
            alpha_threshold: 0.5,
            contrast: 1.0,
        }
    }

    /// One pixel, `covered` of its 16 sub-samples set to `color` with
    /// depths 10, 9, 8, ...
    fn pixel(covered: usize, color: [u8; 3]) -> Samples {
        let mut s = Samples::new(4, 4);
        for i in 0..covered {
            s.color[i] = [color[0], color[1], color[2], 255];
            s.depth[i] = 10.0 - i as f32;
        }
        s
    }

    #[test]
    fn alpha_is_hard_at_the_threshold() {
        let (rgba, depth) = resolve(&pixel(7, [200, 100, 50]), settings());
        assert_eq!(rgba, [0, 0, 0, 0]);
        assert_eq!(depth, [f32::INFINITY]);
        let (rgba, depth) = resolve(&pixel(8, [200, 100, 50]), settings());
        // Un-premultiplied: the colour of the covered sub-samples, not
        // darkened by the uncovered half.
        assert_eq!(rgba, [200, 100, 50, 255]);
        assert_eq!(depth, [3.0]);
        let high = ResolveSettings {
            alpha_threshold: 1.0,
            ..settings()
        };
        assert_eq!(resolve(&pixel(15, [1, 2, 3]), high).0[3], 0);
        assert_eq!(resolve(&pixel(16, [1, 2, 3]), high).0[3], 255);
    }

    #[test]
    fn colour_is_the_rounded_mean() {
        let mut s = pixel(16, [10, 10, 10]);
        s.color[0] = [25, 10, 255, 255];
        let (rgba, _) = resolve(&s, settings());
        // (15 · 10 + 25) / 16 = 10.9375 → 11; (15 · 10 + 255) / 16 = 25.3 → 25.
        assert_eq!(rgba, [11, 10, 25, 255]);
    }

    #[test]
    fn contrast_stretches_around_mid_grey() {
        let doubled = ResolveSettings {
            contrast: 2.0,
            ..settings()
        };
        let (rgba, _) = resolve(&pixel(16, [64, 128, 230]), doubled);
        // 128 / 255 sits just above 0.5, so it moves up by one.
        assert_eq!(rgba, [1, 129, 255, 255]);
        assert_eq!(contrast_table(1.0)[77], 77);
    }
}
