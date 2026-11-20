//! Per-sprite numbers for a tile's metric lines.

use crate::GalleryError;
use crate::image::{Rect, check_rgba};

/// Alpha at or above which a pixel counts as opaque (the quantizer's
/// threshold for index 0, contracts section 8.2).
pub const OPAQUE_ALPHA: u8 = 128;

/// Opaque coverage and colour counts of one RGBA8 sprite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpriteMetrics {
    pub width: u32,
    pub height: u32,
    /// Pixels with alpha of at least [`OPAQUE_ALPHA`].
    pub opaque_pixels: u64,
    /// The smallest rectangle holding every opaque pixel; `None` if none.
    pub bounds: Option<Rect>,
    /// Distinct RGB values among opaque pixels (alpha ignored).
    pub distinct_colours: u64,
}

/// Measures an RGBA8 sprite.
pub fn sprite_metrics(rgba: &[u8], width: u32, height: u32) -> Result<SpriteMetrics, GalleryError> {
    check_rgba("sprite", rgba, width, height)?;
    let mut opaque = 0u64;
    let mut min = [u32::MAX, u32::MAX];
    let mut max = [0u32, 0u32];
    let mut colours: Vec<u32> = Vec::new();
    if width > 0 {
        for (y, row) in rgba.chunks_exact(width as usize * 4).enumerate() {
            let y = y as u32;
            for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
                if px[3] < OPAQUE_ALPHA {
                    continue;
                }
                let x = x as u32;
                opaque += 1;
                min = [min[0].min(x), min[1].min(y)];
                max = [max[0].max(x), max[1].max(y)];
                colours.push(u32::from_be_bytes([0, px[0], px[1], px[2]]));
            }
        }
    }
    colours.sort_unstable();
    colours.dedup();
    let bounds = (opaque > 0).then(|| Rect {
        x: min[0],
        y: min[1],
        width: max[0] - min[0] + 1,
        height: max[1] - min[1] + 1,
    });
    Ok(SpriteMetrics {
        width,
        height,
        opaque_pixels: opaque,
        bounds,
        distinct_colours: colours.len() as u64,
    })
}

impl SpriteMetrics {
    /// Metric lines for a [`crate::Tile`]: `size`, `opaque`, `bbox`
    /// (`x,y wxh`, or `none`) and `colours`, in that order.
    pub fn lines(&self) -> Vec<(String, String)> {
        let bbox = match self.bounds {
            Some(b) => format!("{},{} {}x{}", b.x, b.y, b.width, b.height),
            None => "none".to_owned(),
        };
        vec![
            ("size".to_owned(), format!("{}x{}", self.width, self.height)),
            ("opaque".to_owned(), self.opaque_pixels.to_string()),
            ("bbox".to_owned(), bbox),
            ("colours".to_owned(), self.distinct_colours.to_string()),
        ]
    }
}
