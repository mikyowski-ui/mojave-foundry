//! Terrain queries over extracted cells, and the window of a worldspace
//! that becomes one Fallout 2 map.
//!
//! Everything here is in source space (`docs/contracts.md` section 1): X
//! east, Y north, Z up, game units.

use std::collections::BTreeMap;

use crate::error::MirError;
use crate::model::{CELL_UNITS, LAND_GRID, LAND_SPACING, LandDoc};
use crate::reader::Mir;

/// Floor squares along one side of a Fallout 2 map.
pub const MAP_SIDE_SQUARES: f32 = 100.0;

/// The square of a worldspace that one map covers. Its north-west corner is
/// the tile-space origin: `s` grows east from `x_west`, `t` grows south
/// from `y_north` (contracts section 4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapWindow {
    pub x_west: f32,
    pub y_north: f32,
    /// Side length in game units.
    pub size: f32,
}

impl MapWindow {
    /// The window centred on `center`, for floor squares of `tile_units`.
    pub fn from_center(center: [f32; 2], tile_units: f32) -> Self {
        let size = MAP_SIDE_SQUARES * tile_units;
        Self {
            x_west: center[0] - size * 0.5,
            y_north: center[1] + size * 0.5,
            size,
        }
    }

    /// The anchor to hand a projection: the north-west corner at height
    /// zero.
    pub fn anchor(&self) -> [f32; 3] {
        [self.x_west, self.y_north, 0.0]
    }

    /// Whether a point's ground position is inside the window.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x_west
            && x < self.x_west + self.size
            && y <= self.y_north
            && y > self.y_north - self.size
    }

    /// Tile-space ground coordinates of a source point, in floor squares.
    pub fn tile(&self, x: f32, y: f32) -> [f32; 2] {
        let scale = MAP_SIDE_SQUARES / self.size;
        [(x - self.x_west) * scale, (self.y_north - y) * scale]
    }

    /// The source point of tile-space ground coordinates.
    pub fn source(&self, s: f32, t: f32) -> [f32; 2] {
        let scale = self.size / MAP_SIDE_SQUARES;
        [self.x_west + s * scale, self.y_north - t * scale]
    }

    /// The inclusive ranges of exterior cells the window touches, widened
    /// by `margin` cells on every side.
    pub fn cells(
        &self,
        margin: i32,
    ) -> (std::ops::RangeInclusive<i32>, std::ops::RangeInclusive<i32>) {
        let cell = |v: f32| (v / CELL_UNITS).floor() as i32;
        (
            cell(self.x_west) - margin..=cell(self.x_west + self.size) + margin,
            cell(self.y_north - self.size) - margin..=cell(self.y_north) + margin,
        )
    }
}

struct Grid {
    heights: Vec<f32>,
    normals: Option<Vec<[i8; 3]>>,
}

/// Bilinear terrain height, normal and slope over a block of cells.
///
/// A sampler made by [`GroundSampler::flat`] also has a constant-height
/// floor under every point that no loaded cell covers: an interior cell,
/// which has no land, then still gives every consumer a height (the floor),
/// a zero gradient and slope and an upright normal everywhere.
#[derive(Default)]
pub struct GroundSampler {
    cells: BTreeMap<(i32, i32), Grid>,
    /// The height of the flat floor under points no cell covers.
    flat: Option<f32>,
}

impl GroundSampler {
    /// Loads the land of every cell the window touches, plus one cell of
    /// margin so that objects just outside still find their ground.
    pub fn load(mir: &Mir, worldspace: &str, window: &MapWindow) -> Result<Self, MirError> {
        let mut sampler = Self::default();
        let (xs, ys) = window.cells(1);
        for y in ys {
            for x in xs.clone() {
                if let Some(doc) = mir.cell(worldspace, x, y)?
                    && let Some(land) = &doc.land
                {
                    sampler.insert(x, y, land);
                }
            }
        }
        Ok(sampler)
    }

    /// Flat ground at height `z` everywhere: every finite point has height
    /// `z`, gradient `[0, 0]`, slope 0 and normal `[0, 0, 1]`. Meant for an
    /// interior cell, whose floor height comes from its navmesh (see
    /// [`crate::storeys()`]). Cells added later with
    /// [`GroundSampler::insert`] take precedence over the flat floor where
    /// they lie. A non-finite `z` gives no flat floor, the same as
    /// [`GroundSampler::default`].
    pub fn flat(z: f32) -> Self {
        Self {
            cells: BTreeMap::new(),
            flat: z.is_finite().then_some(z),
        }
    }

    /// The flat floor's height, when this sampler has one
    /// ([`GroundSampler::flat`]).
    pub fn flat_height(&self) -> Option<f32> {
        self.flat
    }

    /// Adds one cell's land. A grid of the wrong size is ignored.
    pub fn insert(&mut self, x: i32, y: i32, land: &LandDoc) {
        let expected = LAND_GRID * LAND_GRID;
        if land.heights.len() != expected {
            return;
        }
        self.cells.insert(
            (x, y),
            Grid {
                heights: land.heights.clone(),
                normals: land.normals.clone().filter(|n| n.len() == expected),
            },
        );
    }

    /// A content hash of everything the sampler answers from (every cell's
    /// heights and normals, and the flat floor), lowercase hex SHA-256: two
    /// samplers with one digest give every query the same answer. For cache
    /// keys.
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"ground-sampler-1");
        match self.flat {
            Some(z) => {
                hasher.update([1]);
                hasher.update(z.to_bits().to_le_bytes());
            }
            None => hasher.update([0]),
        }
        hasher.update((self.cells.len() as u64).to_le_bytes());
        for ((x, y), grid) in &self.cells {
            hasher.update(x.to_le_bytes());
            hasher.update(y.to_le_bytes());
            hasher.update((grid.heights.len() as u64).to_le_bytes());
            for h in &grid.heights {
                hasher.update(h.to_bits().to_le_bytes());
            }
            match &grid.normals {
                Some(normals) => {
                    hasher.update([1]);
                    for n in normals {
                        hasher.update(n.map(|c| c as u8));
                    }
                }
                None => hasher.update([0]),
            }
        }
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Whether there is no ground at all: no terrain was loaded and there
    /// is no flat floor.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.flat.is_none()
    }

    /// The flat floor's height at a point no loaded cell covers; `None` for
    /// a non-finite point, a point over a loaded cell, or no flat floor.
    fn flat_at(&self, x: f32, y: f32) -> Option<f32> {
        let z = self.flat?;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let cell = (
            (x / CELL_UNITS).floor() as i32,
            (y / CELL_UNITS).floor() as i32,
        );
        (!self.cells.contains_key(&cell)).then_some(z)
    }

    /// The cell, the grid square within it, and the position within that
    /// square, for a source point.
    fn locate(&self, x: f32, y: f32) -> Option<(&Grid, usize, usize, f32, f32)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let cx = (x / CELL_UNITS).floor();
        let cy = (y / CELL_UNITS).floor();
        let grid = self.cells.get(&(cx as i32, cy as i32))?;
        let last = (LAND_GRID - 2) as f32;
        let u = ((x - cx * CELL_UNITS) / LAND_SPACING).clamp(0.0, last + 1.0);
        let v = ((y - cy * CELL_UNITS) / LAND_SPACING).clamp(0.0, last + 1.0);
        let column = u.floor().min(last);
        let row = v.floor().min(last);
        Some((grid, row as usize, column as usize, u - column, v - row))
    }

    /// Terrain height at a source point, or `None` where no land was
    /// extracted (and there is no flat floor).
    pub fn height(&self, x: f32, y: f32) -> Option<f32> {
        if let Some(z) = self.flat_at(x, y) {
            return Some(z);
        }
        let (grid, row, column, fu, fv) = self.locate(x, y)?;
        let at = |r: usize, c: usize| grid.heights[r * LAND_GRID + c];
        let south = at(row, column) * (1.0 - fu) + at(row, column + 1) * fu;
        let north = at(row + 1, column) * (1.0 - fu) + at(row + 1, column + 1) * fu;
        Some(south * (1.0 - fv) + north * fv)
    }

    /// Height, or `fallback` where there is no land.
    pub fn height_or(&self, x: f32, y: f32, fallback: f32) -> f32 {
        self.height(x, y).unwrap_or(fallback)
    }

    /// The height gradient `(dz/dx, dz/dy)` of the grid square under a
    /// point; `[0, 0]` on the flat floor.
    pub fn gradient(&self, x: f32, y: f32) -> Option<[f32; 2]> {
        if self.flat_at(x, y).is_some() {
            return Some([0.0, 0.0]);
        }
        let (grid, row, column, fu, fv) = self.locate(x, y)?;
        let at = |r: usize, c: usize| grid.heights[r * LAND_GRID + c];
        let dx_south = at(row, column + 1) - at(row, column);
        let dx_north = at(row + 1, column + 1) - at(row + 1, column);
        let dy_west = at(row + 1, column) - at(row, column);
        let dy_east = at(row + 1, column + 1) - at(row, column + 1);
        Some([
            (dx_south * (1.0 - fv) + dx_north * fv) / LAND_SPACING,
            (dy_west * (1.0 - fu) + dy_east * fu) / LAND_SPACING,
        ])
    }

    /// Rise over run at a point: 0 is flat, 1 is a 45° slope.
    pub fn slope(&self, x: f32, y: f32) -> Option<f32> {
        let [dx, dy] = self.gradient(x, y)?;
        Some((dx * dx + dy * dy).sqrt())
    }

    /// The unit surface normal at a point: the stored vertex normals
    /// blended bilinearly, or the normal of the height gradient when the
    /// cell stores none; straight up on the flat floor.
    pub fn normal(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        if self.flat_at(x, y).is_some() {
            return Some([0.0, 0.0, 1.0]);
        }
        let (grid, row, column, fu, fv) = self.locate(x, y)?;
        let mut normal = [0.0f32; 3];
        if let Some(normals) = &grid.normals {
            let corners = [
                (row, column, (1.0 - fu) * (1.0 - fv)),
                (row, column + 1, fu * (1.0 - fv)),
                (row + 1, column, (1.0 - fu) * fv),
                (row + 1, column + 1, fu * fv),
            ];
            for (r, c, weight) in corners {
                let stored = normals[r * LAND_GRID + c];
                for axis in 0..3 {
                    normal[axis] += f32::from(stored[axis]) * weight;
                }
            }
        }
        let length = normal.iter().map(|v| v * v).sum::<f32>().sqrt();
        if length > 1e-3 {
            return Some(normal.map(|v| v / length));
        }
        let [dx, dy] = self.gradient(x, y)?;
        let length = (dx * dx + dy * dy + 1.0).sqrt();
        Some([-dx / length, -dy / length, 1.0 / length])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::FormKey;

    fn land(height: impl Fn(usize, usize) -> f32) -> LandDoc {
        let mut heights = Vec::with_capacity(LAND_GRID * LAND_GRID);
        for row in 0..LAND_GRID {
            for column in 0..LAND_GRID {
                heights.push(height(row, column));
            }
        }
        LandDoc {
            form: FormKey::new("test.esm", 1),
            heights,
            normals: None,
            colors: None,
            layers: Vec::new(),
        }
    }

    #[test]
    fn the_digest_follows_every_height_and_the_floor() {
        let mut a = GroundSampler::default();
        a.insert(0, 0, &land(|r, c| (r + c) as f32));
        let mut b = GroundSampler::default();
        b.insert(0, 0, &land(|r, c| (r + c) as f32));
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.digest().len(), 64);
        let mut c = GroundSampler::default();
        c.insert(
            0,
            0,
            &land(|r, c| {
                if r == 3 && c == 4 {
                    7.5
                } else {
                    (r + c) as f32
                }
            }),
        );
        let mut moved = GroundSampler::default();
        moved.insert(1, 0, &land(|r, c| (r + c) as f32));
        let digests = [
            a.digest(),
            c.digest(),
            moved.digest(),
            GroundSampler::flat(10.0).digest(),
            GroundSampler::flat(12.0).digest(),
            GroundSampler::default().digest(),
        ];
        let distinct: std::collections::BTreeSet<&String> = digests.iter().collect();
        assert_eq!(distinct.len(), digests.len());
    }

    #[test]
    fn a_window_maps_its_corners_to_tile_space() {
        let window = MapWindow::from_center([1000.0, -2000.0], 140.0);
        assert_eq!(window.size, 14_000.0);
        assert_eq!(window.tile(window.x_west, window.y_north), [0.0, 0.0]);
        let [s, t] = window.tile(1000.0, -2000.0);
        assert!((s - 50.0).abs() < 1e-4 && (t - 50.0).abs() < 1e-4);
        // East is +s, south is +t.
        assert!(window.tile(1140.0, -2000.0)[0] > s);
        assert!(window.tile(1000.0, -2140.0)[1] > t);
        let [x, y] = window.source(s, t);
        assert!((x - 1000.0).abs() < 1e-2 && (y + 2000.0).abs() < 1e-2);
        assert!(window.contains(1000.0, -2000.0));
        assert!(!window.contains(window.x_west - 1.0, -2000.0));
        assert!(!window.contains(1000.0, window.y_north - window.size - 1.0));
    }

    #[test]
    fn a_window_lists_the_cells_it_touches() {
        let window = MapWindow::from_center([0.0, 0.0], 140.0);
        let (xs, ys) = window.cells(0);
        assert_eq!(xs, -2..=1);
        assert_eq!(ys, -2..=1);
        let (xs, _) = window.cells(1);
        assert_eq!(xs, -3..=2);
    }

    #[test]
    fn heights_interpolate_and_cross_cell_edges() {
        let mut sampler = GroundSampler::default();
        // A plane rising east at 0.5 per unit, continuous over two cells.
        sampler.insert(0, 0, &land(|_, c| c as f32 * LAND_SPACING * 0.5));
        sampler.insert(
            1,
            0,
            &land(|_, c| (CELL_UNITS + c as f32 * LAND_SPACING) * 0.5),
        );
        assert_eq!(sampler.height(0.0, 0.0), Some(0.0));
        assert!((sampler.height(64.0, 10.0).unwrap() - 32.0).abs() < 1e-3);
        assert!((sampler.height(CELL_UNITS - 1.0, 100.0).unwrap() - 2047.5).abs() < 1e-2);
        assert!((sampler.height(CELL_UNITS + 1.0, 100.0).unwrap() - 2048.5).abs() < 1e-2);
        assert_eq!(sampler.height(-1.0, 0.0), None);
        assert_eq!(sampler.height_or(-1.0, 0.0, 7.0), 7.0);
        assert_eq!(sampler.height(f32::NAN, 0.0), None);
        let slope = sampler.slope(500.0, 500.0).unwrap();
        assert!((slope - 0.5).abs() < 1e-4);
        let normal = sampler.normal(500.0, 500.0).unwrap();
        // The surface rises to the east, so its normal leans west.
        assert!(normal[0] < 0.0 && normal[1].abs() < 1e-4 && normal[2] > 0.0);
        let length = normal.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((length - 1.0).abs() < 1e-4);
    }

    #[test]
    fn stored_normals_win_over_the_gradient() {
        let mut sampler = GroundSampler::default();
        let mut doc = land(|_, _| 10.0);
        doc.normals = Some(vec![[0, 89, 89]; LAND_GRID * LAND_GRID]);
        sampler.insert(0, 0, &doc);
        let normal = sampler.normal(100.0, 100.0).unwrap();
        assert!(normal[1] > 0.7 && normal[2] > 0.7);
    }

    #[test]
    fn flat_ground_answers_everywhere() {
        let sampler = GroundSampler::flat(3456.0);
        assert!(!sampler.is_empty());
        assert_eq!(sampler.flat_height(), Some(3456.0));
        // Interior coordinates are arbitrary: far from the origin, negative,
        // on cell edges.
        for (x, y) in [
            (0.0, 0.0),
            (-12_345.5, 987.25),
            (CELL_UNITS, -CELL_UNITS),
            (1.0e7, -1.0e7),
        ] {
            assert_eq!(sampler.height(x, y), Some(3456.0));
            assert_eq!(sampler.height_or(x, y, 0.0), 3456.0);
            assert_eq!(sampler.gradient(x, y), Some([0.0, 0.0]));
            assert_eq!(sampler.slope(x, y), Some(0.0));
            assert_eq!(sampler.normal(x, y), Some([0.0, 0.0, 1.0]));
        }
        assert_eq!(sampler.height(f32::NAN, 0.0), None);
        assert_eq!(sampler.slope(0.0, f32::INFINITY), None);
    }

    #[test]
    fn loaded_land_wins_over_the_flat_floor() {
        let mut sampler = GroundSampler::flat(-50.0);
        sampler.insert(0, 0, &land(|_, c| c as f32 * LAND_SPACING * 0.5));
        assert!((sampler.height(64.0, 10.0).unwrap() - 32.0).abs() < 1e-3);
        assert!((sampler.slope(500.0, 500.0).unwrap() - 0.5).abs() < 1e-4);
        assert_eq!(sampler.height(-1.0, 0.0), Some(-50.0));
        assert_eq!(sampler.slope(-1.0, 0.0), Some(0.0));
    }

    #[test]
    fn a_non_finite_flat_height_is_no_floor() {
        let sampler = GroundSampler::flat(f32::NAN);
        assert!(sampler.is_empty());
        assert_eq!(sampler.flat_height(), None);
        assert_eq!(sampler.height(0.0, 0.0), None);
        assert!(GroundSampler::default().is_empty());
    }

    #[test]
    fn a_wrong_sized_grid_is_ignored() {
        let mut sampler = GroundSampler::default();
        let mut doc = land(|_, _| 0.0);
        doc.heights.pop();
        sampler.insert(0, 0, &doc);
        assert!(sampler.is_empty());
    }
}
