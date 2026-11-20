//! A mutable view of an RGBA8 buffer with clipped drawing primitives.

use crate::GalleryError;
use crate::image::expected_len;

/// A borrowed RGBA8 image (contracts section 5) that drawing operations
/// write into. Every operation clips to the image bounds.
#[derive(Debug)]
pub struct Canvas<'a> {
    rgba: &'a mut [u8],
    width: u32,
    height: u32,
}

impl<'a> Canvas<'a> {
    /// Wraps `rgba`, which must hold exactly `width * height * 4` bytes.
    pub fn new(rgba: &'a mut [u8], width: u32, height: u32) -> Result<Self, GalleryError> {
        let expected = expected_len(width, height)?;
        if rgba.len() as u64 != expected {
            return Err(GalleryError::BufferLength {
                what: "canvas",
                width,
                height,
                expected,
                actual: rgba.len(),
            });
        }
        Ok(Self {
            rgba,
            width,
            height,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixel at (`x`, `y`), or `None` outside the canvas.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let at = self.offset(i64::from(x), i64::from(y))?;
        let mut out = [0; 4];
        out.copy_from_slice(&self.rgba[at..at + 4]);
        Some(out)
    }

    fn offset(&self, x: i64, y: i64) -> Option<usize> {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return None;
        }
        usize::try_from((y * i64::from(self.width) + x) * 4).ok()
    }

    /// Clipped range of `[start, start + len)` against `[0, limit)`.
    fn clip(start: i64, len: i64, limit: u32) -> Option<(usize, usize)> {
        let lo = start.max(0);
        let hi = start.saturating_add(len).min(i64::from(limit));
        if hi <= lo {
            return None;
        }
        Some((usize::try_from(lo).ok()?, usize::try_from(hi).ok()?))
    }

    /// Overwrites a rectangle with `colour`.
    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, colour: [u8; 4]) {
        let Some((x0, x1)) = Self::clip(i64::from(x), i64::from(width), self.width) else {
            return;
        };
        let Some((y0, y1)) = Self::clip(i64::from(y), i64::from(height), self.height) else {
            return;
        };
        let stride = self.width as usize * 4;
        for row in y0..y1 {
            let line = &mut self.rgba[row * stride + x0 * 4..row * stride + x1 * 4];
            for pixel in line.as_chunks_mut::<4>().0 {
                pixel.copy_from_slice(&colour);
            }
        }
    }

    /// Draws a one-pixel rectangle outline.
    pub fn outline_rect(&mut self, x: i32, y: i32, width: u32, height: u32, colour: [u8; 4]) {
        if width == 0 || height == 0 {
            return;
        }
        let right = i64::from(x) + i64::from(width) - 1;
        let bottom = i64::from(y) + i64::from(height) - 1;
        self.fill_rect(x, y, width, 1, colour);
        self.fill_rect(x, saturate_i32(bottom), width, 1, colour);
        self.fill_rect(x, y, 1, height, colour);
        self.fill_rect(saturate_i32(right), y, 1, height, colour);
    }

    /// Alpha-blends `colour` (straight alpha) over a rectangle.
    pub(crate) fn blend_rect_i64(
        &mut self,
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        colour: [u8; 4],
    ) {
        let Some((x0, x1)) = Self::clip(x, width, self.width) else {
            return;
        };
        let Some((y0, y1)) = Self::clip(y, height, self.height) else {
            return;
        };
        let stride = self.width as usize * 4;
        for row in y0..y1 {
            let line = &mut self.rgba[row * stride + x0 * 4..row * stride + x1 * 4];
            for pixel in line.as_chunks_mut::<4>().0 {
                blend_into(pixel, colour);
            }
        }
    }

    /// Alpha-blends one pixel; outside the canvas it is a no-op.
    pub fn blend_pixel(&mut self, x: i64, y: i64, colour: [u8; 4]) {
        if let Some(at) = self.offset(x, y) {
            blend_into(&mut self.rgba[at..at + 4], colour);
        }
    }

    /// Fills a rectangle with a checkerboard of `size`-pixel squares whose
    /// phase starts at the rectangle's own top-left corner.
    pub fn checker_rect(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        size: u32,
        colours: [[u8; 4]; 2],
    ) {
        let size = size.max(1);
        let Some((x0, x1)) = Self::clip(i64::from(x), i64::from(width), self.width) else {
            return;
        };
        let Some((y0, y1)) = Self::clip(i64::from(y), i64::from(height), self.height) else {
            return;
        };
        let stride = self.width as usize * 4;
        for row in y0..y1 {
            let ty = (row as i64 - i64::from(y)) / i64::from(size);
            for column in x0..x1 {
                let tx = (column as i64 - i64::from(x)) / i64::from(size);
                let colour = colours[((tx + ty) & 1) as usize];
                let at = row * stride + column * 4;
                self.rgba[at..at + 4].copy_from_slice(&colour);
            }
        }
    }
}

fn saturate_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Straight-alpha "source over" onto an opaque or translucent destination.
fn blend_into(dst: &mut [u8], src: [u8; 4]) {
    let a = u32::from(src[3]);
    if a == 255 {
        dst.copy_from_slice(&src);
        return;
    }
    if a == 0 {
        return;
    }
    let inv = 255 - a;
    for channel in 0..3 {
        let value = u32::from(src[channel]) * a + u32::from(dst[channel]) * inv;
        dst[channel] = ((value + 127) / 255) as u8;
    }
    let out_a = a + (u32::from(dst[3]) * inv + 127) / 255;
    dst[3] = out_a.min(255) as u8;
}
