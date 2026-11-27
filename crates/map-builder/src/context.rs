//! Lower-level context: on an elevation above the ground, the blank floor
//! near its own walkable hexes shows the elevation below, dimmed and
//! desaturated, as the fixed camera would see it from above.
//!
//! An upper elevation (an exterior level above the ground, an interior
//! storey above the lowest) draws only its own floor; the rest of its
//! squares are blank and the engine shows black there. This module makes
//! floor art from a picture of the elevation below instead (the caller's
//! composite of that elevation as the engine draws it: its floor tiles and
//! every object), so that the decks hang over the town, not over a void.
//! It only ever paints floor squares: nothing it makes is an object, so
//! walkability, blocking, links and objects are those of a build without it.
//!
//! **Where.** [`reach_squares`]: the squares holding a hex within `reach`
//! hex steps of a walkable hex of the upper elevation.
//!
//! **Parallax.** One storey is 96 pixels up on screen (`docs/contracts.md`
//! section 4: `screen_y = ... - 96 e`). The elevation below draws a deck
//! standing `e` storeys above its ground `96 e` pixels above that ground;
//! the upper elevation draws the same deck on its own hexes, at `e = 0`. So
//! the upper elevation's view is the lower one's moved down the screen by
//! `96 e` pixels, and a context pixel at screen `p` shows the composite at
//! `p - (0, shift)` ([`paint`]), `shift = 96 e` rounded. The decks of the
//! composite then sit right under the upper elevation's own deck floor.
//!
//! **Look.** Each pixel is the mean colour of the composite's drawn pixels
//! in the 3 by 3 block around its sample (a light blur: the composite is
//! dithered, and the dither would make every square a tile of its own),
//! then desaturated towards its luma (`c + (Y - c) d`), then scaled by the
//! dim (`lower_level_dim`, the brightness kept: 0.35 is 35 %). A sample
//! with nothing drawn (index 0) stays transparent, so the void below the
//! lower elevation stays black. The caller quantizes and deduplicates the
//! squares as floor tiles, with a tile cap of its own.

use std::collections::{BTreeSet, VecDeque};

use vault_formats::HexGrid;

use crate::walk::Frame;

/// Floor tile size, pixels.
const TILE_WIDTH: i64 = 80;
const TILE_HEIGHT: i64 = 36;
const GRID: HexGrid = HexGrid { origin: [0, 0] };

/// How the context is drawn ([`paint`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextLook {
    /// The brightness kept, 0 to 1 (`lower_level_dim`).
    pub dim: f64,
    /// How much colour is taken out, 0 to 1 (`lower_level_desaturate`).
    pub desaturate: f64,
    /// Pixels the lower elevation's picture is moved down the screen
    /// (module notes, parallax).
    pub shift: i64,
}

/// A picture of the elevation below, in map screen space: indexed pixels,
/// pixel `(0, 0)` at `origin`, index 0 not drawn.
#[derive(Debug, Clone, Copy)]
pub struct Composite<'a> {
    pub pixels: &'a [u8],
    pub origin: [i32; 2],
    pub width: u32,
    pub height: u32,
    /// The palette's display colours, by index.
    pub palette: &'a [[u8; 3]; 256],
}

/// One square's context: an 80 by 36 RGBA image (straight alpha) at the
/// square's tile box, opaque on the square's own pixels where the picture
/// holds something, transparent elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSquare {
    pub square: i32,
    /// The tile box's corner in map screen space.
    pub origin: [i32; 2],
    pub rgba: Vec<u8>,
    /// Opaque pixels.
    pub drawn: usize,
}

/// The squares holding a hex within `reach` hex steps of a hex `walkable`
/// marks (40,000 flags), sorted.
pub fn reach_squares(walkable: &[bool], reach: u32) -> BTreeSet<i32> {
    let mut depth = vec![u32::MAX; 40_000];
    let mut queue = VecDeque::new();
    for (hex, &w) in walkable.iter().enumerate().take(40_000) {
        if w {
            depth[hex] = 0;
            queue.push_back(hex as i32);
        }
    }
    while let Some(hex) = queue.pop_front() {
        let d = depth[hex as usize];
        if d >= reach {
            continue;
        }
        for next in crate::walk::neighbours(hex) {
            if depth[next as usize] == u32::MAX {
                depth[next as usize] = d + 1;
                queue.push_back(next);
            }
        }
    }
    (0..40_000)
        .filter(|&h| depth[h as usize] != u32::MAX)
        .filter_map(Frame::square_of_hex)
        .collect()
}

/// The shift in pixels for `heights` (storeys above the ground of the
/// elevation below), as [`ContextLook::shift`]: `96 e` for the median `e`,
/// rounded; 0 for none.
pub fn shift_of(mut heights: Vec<f64>, storey_pixels: f64) -> i64 {
    heights.retain(|h| h.is_finite());
    if heights.is_empty() {
        return 0;
    }
    heights.sort_by(f64::total_cmp);
    let median = heights[heights.len() / 2];
    (median * storey_pixels).round() as i64
}

/// The context of every square of `squares` (module notes): squares the
/// picture shows nothing on are left out.
pub fn paint(
    composite: &Composite<'_>,
    squares: &BTreeSet<i32>,
    look: &ContextLook,
) -> Vec<ContextSquare> {
    let (w, h) = (i64::from(composite.width), i64::from(composite.height));
    let sample = |x: i64, y: i64| -> Option<[u8; 3]> {
        let (cx, cy) = (
            x - i64::from(composite.origin[0]),
            y - i64::from(composite.origin[1]),
        );
        if cx < 0 || cy < 0 || cx >= w || cy >= h {
            return None;
        }
        let index = *composite.pixels.get((cy * w + cx) as usize)?;
        (index != 0).then(|| composite.palette[usize::from(index)])
    };
    let dim = look.dim.clamp(0.0, 1.0);
    let desaturate = look.desaturate.clamp(0.0, 1.0);
    let mut out = Vec::new();
    for &q in squares {
        let Some(corner) = GRID.square_corner(q, false) else {
            continue;
        };
        let mut rgba = vec![0u8; (TILE_WIDTH * TILE_HEIGHT * 4) as usize];
        let mut drawn = 0usize;
        for py in 0..TILE_HEIGHT {
            for px in 0..TILE_WIDTH {
                let p = [corner[0] + px, corner[1] + py];
                let (Ok(x), Ok(y)) = (i32::try_from(p[0]), i32::try_from(p[1])) else {
                    continue;
                };
                if GRID.pick_square([x, y], false) != Some(q) {
                    continue;
                }
                let (sx, sy) = (p[0], p[1] - look.shift);
                if sample(sx, sy).is_none() {
                    continue;
                }
                let (mut sum, mut n) = ([0.0f64; 3], 0.0f64);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if let Some(c) = sample(sx + dx, sy + dy) {
                            for k in 0..3 {
                                sum[k] += f64::from(c[k]);
                            }
                            n += 1.0;
                        }
                    }
                }
                let c = sum.map(|s| s / n);
                let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                let at = ((py * TILE_WIDTH + px) * 4) as usize;
                for k in 0..3 {
                    let v = (c[k] + (luma - c[k]) * desaturate) * dim;
                    rgba[at + k] = v.round().clamp(0.0, 255.0) as u8;
                }
                rgba[at + 3] = 255;
                drawn += 1;
            }
        }
        if drawn > 0 {
            out.push(ContextSquare {
                square: q,
                origin: [corner[0] as i32, corner[1] as i32],
                rgba,
                drawn,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> [[u8; 3]; 256] {
        std::array::from_fn(|i| [i as u8, (255 - i) as u8, 128])
    }

    #[test]
    fn reach_takes_the_squares_near_walkable_hexes() {
        let mut walkable = vec![false; 40_000];
        let centre = 100 * 200 + 100;
        walkable[centre] = true;
        let none = reach_squares(&walkable, 0);
        assert_eq!(none.len(), 1);
        let near = reach_squares(&walkable, 4);
        let far = reach_squares(&walkable, 12);
        assert!(
            near.len() > 4 && far.len() > 4 * near.len(),
            "{} {}",
            near.len(),
            far.len()
        );
        assert!(near.is_subset(&far));
        assert!(reach_squares(&vec![false; 40_000], 12).is_empty());
    }

    #[test]
    fn the_shift_is_the_median_height_in_storey_pixels() {
        assert_eq!(shift_of(vec![], 96.0), 0);
        assert_eq!(shift_of(vec![1.0, 2.0, 1.1, f64::NAN], 96.0), 106);
        assert_eq!(shift_of(vec![0.5], 96.0), 48);
    }

    #[test]
    fn a_picture_is_dimmed_desaturated_and_moved_down_by_the_shift() {
        let (origin, width, height) = ([0, 0], 4000u32, 3000u32);
        // The left half drawn in index 200, the right half not at all.
        let mut pixels = vec![0u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width / 2 {
                pixels[(y * width + x) as usize] = 200;
            }
        }
        let palette = palette();
        let composite = Composite {
            pixels: &pixels,
            origin,
            width,
            height,
            palette: &palette,
        };
        let squares: BTreeSet<i32> = (0..10_000)
            .filter(|&q| {
                GRID.square_corner(q, false)
                    .is_some_and(|c| (200..3700).contains(&c[0]) && (200..2700).contains(&c[1]))
            })
            .collect();
        assert!(!squares.is_empty());
        let look = ContextLook {
            dim: 0.5,
            desaturate: 0.0,
            shift: 0,
        };
        let painted = paint(&composite, &squares, &look);
        assert!(!painted.is_empty());
        for s in &painted {
            assert!(s.origin[0] < 2000, "only the drawn half: {:?}", s.origin);
            for px in s.rgba.chunks(4).filter(|p| p[3] == 255) {
                // Colour 200 is (200, 55, 128): halved, inside the block
                // (the edge column averages only drawn pixels).
                assert_eq!(&px[..3], &[100, 28, 64]);
            }
        }
        // Grey at full desaturation.
        let grey = paint(
            &composite,
            &squares,
            &ContextLook {
                desaturate: 1.0,
                ..look
            },
        );
        for s in &grey {
            for px in s.rgba.chunks(4).filter(|p| p[3] == 255) {
                assert_eq!(px[0], px[1]);
                assert_eq!(px[1], px[2]);
            }
        }
        // A shift down the screen samples higher up: a picture drawn only
        // above y = 1000 shows on squares below it.
        let mut top = vec![0u8; (width * height) as usize];
        for y in 0..1000 {
            for x in 0..width {
                top[(y * width + x) as usize] = 9;
            }
        }
        let shifted = Composite {
            pixels: &top,
            ..composite
        };
        let still = paint(&shifted, &squares, &look);
        let moved = paint(&shifted, &squares, &ContextLook { shift: 500, ..look });
        let lowest = |v: &[ContextSquare]| v.iter().map(|s| s.origin[1]).max().unwrap();
        assert!(lowest(&moved) >= lowest(&still) + 450);
        // Deterministic.
        assert_eq!(paint(&shifted, &squares, &look), still);
    }
}
