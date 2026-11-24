//! Film strips: one labelled row of frames per animation, for looking at
//! a character's whole sprite set on one page.
//!
//! Every frame of a row sits in a cell of the row's common size, placed
//! by its own anchor (the point the engine puts on the hex centre), so a
//! figure that walks, falls or is pushed keeps its place across the row.
//! A marked frame (an animation's action frame) gets a red frame around
//! its cell; every cell shows its anchor as a small cross.

use crate::canvas::Canvas;
use crate::font::{draw_text, line_height, truncate_to_width};
use crate::image::{Image, MAX_DIMENSION, MAX_PIXELS, check_rgba};
use crate::{GalleryError, encode_png};

/// One frame of a strip.
#[derive(Debug, Clone, Copy)]
pub struct StripFrame<'a> {
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// Where the frame's anchor (the hex centre it stands on) lies, in
    /// the frame's own pixels; may be outside it.
    pub anchor: [i32; 2],
    /// Draw the marker around it.
    pub marked: bool,
    /// A short caption under the cell (a frame number); may be empty.
    pub caption: &'a str,
}

/// One row: a label and its frames, left to right.
#[derive(Debug, Clone)]
pub struct Strip<'a> {
    pub label: String,
    pub frames: Vec<StripFrame<'a>>,
}

/// How a page of strips looks.
#[derive(Debug, Clone)]
pub struct StripOptions {
    pub title: Option<String>,
    /// Whole-pixel magnification, 1 to 8.
    pub zoom: u32,
    /// Width of the label column, pixels at zoom 1 (the label is cut to it).
    pub label_width: u32,
    pub background: [u8; 4],
    pub cell_background: [u8; 4],
}

impl Default for StripOptions {
    fn default() -> Self {
        StripOptions {
            title: None,
            zoom: 2,
            label_width: 150,
            background: [40, 40, 44, 255],
            cell_background: [96, 96, 96, 255],
        }
    }
}

const PAD: u32 = 4;
const TEXT: [u8; 4] = [235, 235, 235, 255];
const MARK: [u8; 4] = [230, 40, 40, 255];
const CROSS: [u8; 4] = [255, 220, 0, 255];

/// Draws every strip on one page.
///
/// Refused: a zoom outside 1 to 8, a frame whose buffer does not match its
/// size, and a page above the crate's size limits.
pub fn film_strips(strips: &[Strip<'_>], options: &StripOptions) -> Result<Image, GalleryError> {
    if !(1..=8).contains(&options.zoom) {
        return Err(GalleryError::InvalidOption("zoom must be 1 to 8"));
    }
    let zoom = options.zoom;
    // Per row: the union of every frame's rectangle about its anchor.
    struct Row {
        left: i64,
        top: i64,
        width: u32,
        height: u32,
    }
    let mut rows = Vec::with_capacity(strips.len());
    for strip in strips {
        let mut bounds: Option<[i64; 4]> = None;
        for frame in &strip.frames {
            check_rgba("strip frame", frame.rgba, frame.width, frame.height)?;
            let l = -i64::from(frame.anchor[0]);
            let t = -i64::from(frame.anchor[1]);
            let r = l + i64::from(frame.width);
            let b = t + i64::from(frame.height);
            bounds = Some(match bounds {
                None => [l.min(0), t.min(0), r.max(1), b.max(1)],
                Some([a, c, d, e]) => [a.min(l), c.min(t), d.max(r), e.max(b)],
            });
        }
        let [l, t, r, b] = bounds.unwrap_or([0, 0, 1, 1]);
        let too_large = || GalleryError::TooLarge {
            width: (r - l) as u64,
            height: (b - t) as u64,
        };
        rows.push(Row {
            left: l,
            top: t,
            width: u32::try_from(r - l).map_err(|_| too_large())?,
            height: u32::try_from(b - t).map_err(|_| too_large())?,
        });
    }
    let caption_height = line_height(1);
    let label_width = options.label_width;
    let title_height = if options.title.is_some() {
        line_height(2) + PAD
    } else {
        0
    };
    let mut page_width = u64::from(label_width + 2 * PAD);
    let mut page_height = u64::from(title_height + PAD);
    for (strip, row) in strips.iter().zip(&rows) {
        let cells = strip.frames.len().max(1) as u64;
        let cell = u64::from(row.width * zoom + PAD);
        page_width = page_width.max(u64::from(label_width + 2 * PAD) + cells * cell);
        page_height += u64::from(row.height * zoom + caption_height + 2 * PAD);
    }
    if page_width > u64::from(MAX_DIMENSION)
        || page_height > u64::from(MAX_DIMENSION)
        || page_width * page_height > MAX_PIXELS
    {
        return Err(GalleryError::TooLarge {
            width: page_width,
            height: page_height,
        });
    }
    let (width, height) = (page_width as u32, page_height as u32);
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&options.background);
    }
    let mut canvas = Canvas::new(&mut rgba, width, height)?;
    let mut y = PAD;
    if let Some(title) = &options.title {
        draw_text(&mut canvas, PAD as i32, y as i32, title, TEXT, 2);
        y += title_height;
    }
    for (strip, row) in strips.iter().zip(&rows) {
        let label = truncate_to_width(&strip.label, label_width, 1);
        draw_text(&mut canvas, PAD as i32, (y + PAD) as i32, &label, TEXT, 1);
        let mut x = label_width + 2 * PAD;
        let (cw, ch) = (row.width * zoom, row.height * zoom);
        for frame in &strip.frames {
            canvas.fill_rect(x as i32, y as i32, cw, ch, options.cell_background);
            // The anchor's place in the cell, at zoom 1.
            let ax = -row.left;
            let ay = -row.top;
            let fx = ax - i64::from(frame.anchor[0]);
            let fy = ay - i64::from(frame.anchor[1]);
            for py in 0..frame.height {
                for px in 0..frame.width {
                    let at = ((py * frame.width + px) * 4) as usize;
                    let colour = [
                        frame.rgba[at],
                        frame.rgba[at + 1],
                        frame.rgba[at + 2],
                        frame.rgba[at + 3],
                    ];
                    if colour[3] == 0 {
                        continue;
                    }
                    let sx = i64::from(x) + (fx + i64::from(px)) * i64::from(zoom);
                    let sy = i64::from(y) + (fy + i64::from(py)) * i64::from(zoom);
                    canvas.fill_rect(sx as i32, sy as i32, zoom, zoom, colour);
                }
            }
            let cx = i64::from(x) + ax * i64::from(zoom);
            let cy = i64::from(y) + ay * i64::from(zoom);
            canvas.fill_rect(cx as i32 - 3, cy as i32, 7, 1, CROSS);
            canvas.fill_rect(cx as i32, cy as i32 - 3, 1, 7, CROSS);
            if frame.marked {
                canvas.outline_rect(x as i32 - 1, y as i32 - 1, cw + 2, ch + 2, MARK);
                canvas.outline_rect(x as i32 - 2, y as i32 - 2, cw + 4, ch + 4, MARK);
            }
            if !frame.caption.is_empty() {
                draw_text(
                    &mut canvas,
                    x as i32,
                    (y + ch + 1) as i32,
                    &truncate_to_width(frame.caption, cw, 1),
                    TEXT,
                    1,
                );
            }
            x += cw + PAD;
        }
        y += ch + caption_height + 2 * PAD;
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// [`film_strips`] as PNG bytes.
pub fn film_strips_png(
    strips: &[Strip<'_>],
    options: &StripOptions,
) -> Result<Vec<u8>, GalleryError> {
    let image = film_strips(strips, options)?;
    encode_png(&image.rgba, image.width, image.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        (0..w * h).flat_map(|_| c).collect()
    }

    #[test]
    fn rows_share_a_cell_size_and_marks_show() {
        let a = solid(4, 8, [0, 200, 0, 255]);
        let b = solid(6, 3, [0, 0, 200, 255]);
        let strips = [Strip {
            label: "fall".into(),
            frames: vec![
                StripFrame {
                    rgba: &a,
                    width: 4,
                    height: 8,
                    anchor: [2, 8],
                    marked: false,
                    caption: "0",
                },
                StripFrame {
                    rgba: &b,
                    width: 6,
                    height: 3,
                    anchor: [-4, 3],
                    marked: true,
                    caption: "1",
                },
            ],
        }];
        let options = StripOptions {
            zoom: 1,
            label_width: 20,
            ..StripOptions::default()
        };
        let page = film_strips(&strips, &options).unwrap();
        let again = film_strips(&strips, &options).unwrap();
        assert_eq!(page, again);
        // The marker is drawn: some pixel has the mark colour.
        assert!(page.rgba.as_chunks::<4>().0.contains(&MARK));
        // Both frames' colours are on the page.
        assert!(page.rgba.as_chunks::<4>().0.contains(&[0, 200, 0, 255]));
        assert!(page.rgba.as_chunks::<4>().0.contains(&[0, 0, 200, 255]));
        assert!(film_strips(&strips, &StripOptions { zoom: 0, ..options }).is_err());
    }
}
