//! What the placements' art covers on screen: whether a figure standing on
//! a hex would be hidden or drawn over, and whether wall art stands on a
//! hex.
//!
//! Every placement is drawn as the engine draws a hex object: its frame's
//! top-left corner at the hex centre (`HexGrid::hex_center`) plus the FRM
//! shift plus the object's displacement, less half the frame's width
//! (rounded down) and its height less one. Only opaque pixels (index other
//! than 0) count. Floor pieces and items lie flat under every character and
//! are left out.
//!
//! **Draw order.** The engine draws objects in ascending hex number, so
//! art anchored on a lower hex than the figure's is drawn before it
//! (behind); art anchored on the same or a higher hex is drawn over it.
//!
//! **Figure box.** A figure stands with its frame's bottom centre on the
//! hex centre: the box is `width` pixels wide, centred, and `height` tall,
//! ending on the centre's row ([`crate::BuildOptions::entrance_box`]).
//!
//! **Art standing on a hex** ([`Cover::stands_on`]): an opaque pixel inside
//! the hex's ground box (17 × 13 pixels about its centre) that belongs to
//! a wall strip anchored in the same hex row (the strip itself, or a merged
//! run of that row), or to the landmark band whose rows hold the hex's
//! row. The slicer gives each pixel to the hex its surface stands over, so
//! a strip pixel on a same-row hex's ground point is the foot of a wall
//! there, while a roof or eave over hexes further south lands on rows to
//! the north. A band holds the pixels of a run of hex rows and is anchored
//! at its lowest hex, so its rows run from its anchor's row to the row
//! before the next band of the same reference (the last band's rows run
//! on): its pixel on a ground point of those rows is the landmark standing
//! there (its blocked core), while a pixel of a band further south on the
//! ground of a northern row is the landmark hiding ground behind it.
//! Landmark bands are the scenery placements whose reference has several
//! scenery placements, or whose art is at least [`LANDMARK_WIDTH`] wide
//! (a band of its own, its rows from its anchor's on).

use std::collections::{BTreeMap, BTreeSet};

use vault_formats::HexGrid;

use crate::{ArtImage, Placement, Role};

/// Scenery art at least this wide counts as a landmark band.
pub const LANDMARK_WIDTH: u32 = 96;
/// Half the ground box's width and height about a hex centre.
const GROUND_HALF: [i64; 2] = [8, 6];
/// Side of the spatial index's cells, in pixels.
const CELL: i64 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Wall,
    /// A landmark band and the hex rows it holds, first and last.
    Landmark(i32, i32),
    Other,
}

#[derive(Debug, Clone, Copy)]
struct Sprite {
    hex: i32,
    left: i64,
    top: i64,
    width: i64,
    height: i64,
    art: usize,
    class: Class,
}

/// How a figure on a hex is drawn against the art around it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sight {
    /// No art overlaps the figure's box.
    #[default]
    Clear,
    /// Only art drawn before the figure (behind it) overlaps its box.
    Behind,
    /// Art drawn after the figure overlaps its box: it is drawn over.
    Covered,
}

/// The placements' opaque art, indexed by screen position.
#[derive(Debug, Clone)]
pub struct Cover<'a> {
    art: &'a [ArtImage],
    sprites: Vec<Sprite>,
    cells: BTreeMap<(i64, i64), Vec<u32>>,
}

fn cell_range(lo: i64, hi: i64) -> std::ops::RangeInclusive<i64> {
    lo.div_euclid(CELL)..=hi.div_euclid(CELL)
}

impl<'a> Cover<'a> {
    /// Index every placement's art (floor pieces and items excepted).
    pub fn new(art: &'a [ArtImage], placements: &[Placement]) -> Self {
        // The anchor rows of each reference's scenery placements.
        let mut band_rows: BTreeMap<&str, BTreeSet<i32>> = BTreeMap::new();
        for p in placements.iter().filter(|p| p.role == Role::Scenery) {
            band_rows
                .entry(&p.reference)
                .or_default()
                .insert(p.hex / 200);
        }
        let mut per_reference: BTreeMap<&str, usize> = BTreeMap::new();
        for p in placements.iter().filter(|p| p.role == Role::Scenery) {
            *per_reference.entry(&p.reference).or_default() += 1;
        }
        let mut sprites = Vec::new();
        let mut cells: BTreeMap<(i64, i64), Vec<u32>> = BTreeMap::new();
        for p in placements {
            if matches!(p.role, Role::FloorPiece | Role::Item) {
                continue;
            }
            let Some(image) = art.get(p.art) else {
                continue;
            };
            if image.pixels.is_empty() || image.width == 0 || image.height == 0 {
                continue;
            }
            let Ok(centre) = HexGrid::default().hex_center(p.hex) else {
                continue;
            };
            let anchor = [
                centre[0] + i64::from(image.shift[0]) + i64::from(p.displacement[0]),
                centre[1] + i64::from(image.shift[1]) + i64::from(p.displacement[1]),
            ];
            let row = p.hex / 200;
            let class = match p.role {
                Role::WallStrip => Class::Wall,
                Role::Scenery
                    if per_reference
                        .get(p.reference.as_str())
                        .is_some_and(|n| *n > 1)
                        || image.width >= LANDMARK_WIDTH =>
                {
                    let last = band_rows
                        .get(p.reference.as_str())
                        .and_then(|rows| rows.range(row + 1..).next())
                        .map_or(i32::MAX, |next| next - 1);
                    Class::Landmark(row, last)
                }
                _ => Class::Other,
            };
            let sprite = Sprite {
                hex: p.hex,
                left: anchor[0] - i64::from(image.width / 2),
                top: anchor[1] - i64::from(image.height) + 1,
                width: i64::from(image.width),
                height: i64::from(image.height),
                art: p.art,
                class,
            };
            let index = sprites.len() as u32;
            for cy in cell_range(sprite.top, sprite.top + sprite.height - 1) {
                for cx in cell_range(sprite.left, sprite.left + sprite.width - 1) {
                    cells.entry((cx, cy)).or_default().push(index);
                }
            }
            sprites.push(sprite);
        }
        Self {
            art,
            sprites,
            cells,
        }
    }

    /// The sprites whose rectangle meets `[x0, x1] × [y0, y1]`, in index
    /// order.
    fn near(&self, x0: i64, y0: i64, x1: i64, y1: i64) -> Vec<u32> {
        let mut found = BTreeSet::new();
        for cy in cell_range(y0, y1) {
            for cx in cell_range(x0, x1) {
                if let Some(list) = self.cells.get(&(cx, cy)) {
                    found.extend(list.iter().copied());
                }
            }
        }
        found
            .into_iter()
            .filter(|i| {
                let s = &self.sprites[*i as usize];
                s.left <= x1 && s.left + s.width > x0 && s.top <= y1 && s.top + s.height > y0
            })
            .collect()
    }

    /// Whether sprite `index` has an opaque pixel in `[x0, x1] × [y0, y1]`.
    fn opaque_in(&self, index: u32, x0: i64, y0: i64, x1: i64, y1: i64) -> bool {
        let s = &self.sprites[index as usize];
        let image = &self.art[s.art];
        let (lx, hx) = ((x0 - s.left).max(0), (x1 - s.left).min(s.width - 1));
        let (ly, hy) = ((y0 - s.top).max(0), (y1 - s.top).min(s.height - 1));
        if lx > hx || ly > hy {
            return false;
        }
        (ly..=hy).any(|y| {
            let row = (y * s.width) as usize;
            image.pixels[row + lx as usize..=row + hx as usize]
                .iter()
                .any(|p| *p != 0)
        })
    }

    /// How a figure `[width, height]` pixels large standing on `hex` is
    /// drawn against the art (module notes).
    pub fn sight(&self, hex: i32, [width, height]: [u32; 2]) -> Sight {
        let Ok([cx, cy]) = HexGrid::default().hex_center(hex) else {
            return Sight::Covered;
        };
        let (w, h) = (i64::from(width.max(1)), i64::from(height.max(1)));
        let (x0, x1) = (cx - w / 2, cx - w / 2 + w - 1);
        let (y0, y1) = (cy - h + 1, cy);
        let mut sight = Sight::Clear;
        for index in self.near(x0, y0, x1, y1) {
            let front = self.sprites[index as usize].hex >= hex;
            if (front || sight == Sight::Clear) && self.opaque_in(index, x0, y0, x1, y1) {
                if front {
                    return Sight::Covered;
                }
                sight = Sight::Behind;
            }
        }
        sight
    }

    /// Whether wall art stands on `hex` (module notes).
    pub fn stands_on(&self, hex: i32) -> bool {
        let Ok([cx, cy]) = HexGrid::default().hex_center(hex) else {
            return false;
        };
        let row = hex / 200;
        let (x0, x1) = (cx - GROUND_HALF[0], cx + GROUND_HALF[0]);
        let (y0, y1) = (cy - GROUND_HALF[1], cy + GROUND_HALF[1]);
        self.near(x0, y0, x1, y1).into_iter().any(|index| {
            let s = &self.sprites[index as usize];
            let counts = match s.class {
                Class::Wall => s.hex / 200 == row,
                Class::Landmark(first, last) => (first..=last).contains(&row),
                Class::Other => false,
            };
            counts && self.opaque_in(index, x0, y0, x1, y1)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArtCategory;

    fn image(category: ArtCategory, width: u32, height: u32) -> ArtImage {
        ArtImage {
            category,
            width,
            height,
            pixels: vec![7; (width * height) as usize],
            shift: [0, 0],
            source: "test".into(),
        }
    }

    fn placement(art: usize, hex: i32, role: Role, reference: &str) -> Placement {
        Placement {
            art,
            hex,
            displacement: [0, 0],
            role,
            reference: reference.into(),
            blocking: true,
        }
    }

    #[test]
    fn a_tall_strip_in_front_covers_and_one_behind_does_not() {
        let art = vec![image(ArtCategory::Walls, 96, 120)];
        let at = 100 * 200 + 100;
        // Two rows south (larger hex) and so drawn later, 24 pixels lower.
        let front = at + 2 * 200;
        let cover = Cover::new(&art, &[placement(0, front, Role::WallStrip, "a")]);
        assert_eq!(cover.sight(at, [32, 72]), Sight::Covered);
        assert_eq!(cover.sight(front + 6 * 200, [32, 72]), Sight::Clear);
        let behind = at - 2 * 200;
        let cover = Cover::new(&art, &[placement(0, behind, Role::WallStrip, "a")]);
        assert_eq!(cover.sight(at, [32, 72]), Sight::Behind);
        // A floor piece never counts.
        let art = vec![image(ArtCategory::Scenery, 32, 120)];
        let cover = Cover::new(&art, &[placement(0, front, Role::FloorPiece, "a")]);
        assert_eq!(cover.sight(at, [32, 72]), Sight::Clear);
    }

    #[test]
    fn wall_art_stands_on_its_row_and_landmarks_anywhere() {
        let at = 100 * 200 + 100;
        // A strip three hexes wide anchored on `at`, reaching 20 pixels
        // below its centre: it stands on `at` and on its row neighbours,
        // not on the hex in the next row, which its pixels also reach.
        let mut wall = image(ArtCategory::Walls, 96, 40);
        wall.shift = [0, 20];
        let art = vec![wall, image(ArtCategory::Scenery, 40, 300)];
        let cover = Cover::new(&art, &[placement(0, at, Role::WallStrip, "a")]);
        assert!(cover.stands_on(at));
        assert!(cover.stands_on(at + 2), "same row, two columns over");
        assert!(!cover.stands_on(at + 200), "the next row");
        // A tall sprite at column 100, row 110 is drawn over the ground of
        // column 98, row 107, and stands on nothing, not even its own hex.
        let south = 100 + 200 * 110;
        let hidden = 98 + 200 * 107;
        let cover = Cover::new(&art, &[placement(1, south, Role::Scenery, "b")]);
        assert!(!cover.stands_on(hidden) && !cover.stands_on(south));
        assert_eq!(cover.sight(hidden, [32, 72]), Sight::Covered);
        // As bands of one landmark anchored on rows 110 and 112, it stands
        // on the ground of its rows (row 110 is the first band's), not on
        // the ground it hides further north.
        let cover = Cover::new(
            &art,
            &[
                placement(1, south, Role::Scenery, "b"),
                placement(1, south + 400, Role::Scenery, "b"),
            ],
        );
        assert!(cover.stands_on(south));
        assert!(!cover.stands_on(hidden));
    }
}
