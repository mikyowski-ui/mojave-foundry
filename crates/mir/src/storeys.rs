//! The storeys of a cell: its navmesh triangles split into levels by
//! height.
//!
//! An interior cell has no land, so the height its floor stands at comes
//! from its navigation meshes. [`storeys`] takes every triangle of every
//! navmesh of a cell, treats each as the height interval its corners span,
//! and joins intervals into one level while the next one starts no more
//! than `gap` above the highest point of the level so far. A level is then
//! described by a robust floor height, its walkable area and its extent.
//!
//! Ramps and stairs span the heights between the floors they join, so they
//! merge those floors into one level; a level is "everything connected in
//! height", not necessarily one flat floor. Its floor height is the
//! area-weighted median of the triangle heights, which is the height of
//! most of its floor, not of the stairs.
//!
//! Everything here is in source space: X east, Y north, Z up, game units.

use crate::model::{CellDoc, NavmeshDoc};

/// The default largest vertical step (game units) inside one level: more
/// than a stair step or a dais, less than a storey.
pub const DEFAULT_STOREY_GAP: f32 = 120.0;

/// One level of a cell's navmeshes.
#[derive(Debug, Clone, PartialEq)]
pub struct Storey {
    /// The floor height: the median of the triangle centroid heights,
    /// weighted by horizontal area (by count when every triangle is
    /// vertical).
    pub floor_z: f32,
    /// The lowest and highest triangle corner of the level.
    pub z_range: [f32; 2],
    /// Walkable area: the summed horizontal (XY-projected) area of the
    /// level's triangles, in game units².
    pub area: f32,
    /// Triangles in the level.
    pub triangles: usize,
    /// The south-west corner of the level's XY bounding box.
    pub min: [f32; 2],
    /// The north-east corner of the level's XY bounding box.
    pub max: [f32; 2],
}

impl Storey {
    /// Whether height `z` falls inside the level's height range.
    pub fn contains_z(&self, z: f32) -> bool {
        z >= self.z_range[0] && z <= self.z_range[1]
    }

    /// The centre of the level's XY bounding box.
    pub fn center(&self) -> [f32; 2] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
        ]
    }
}

/// The storeys of a cell with [`DEFAULT_STOREY_GAP`]; see
/// [`storeys_with_gap`].
pub fn storeys(doc: &CellDoc) -> Vec<Storey> {
    storeys_with_gap(doc, DEFAULT_STOREY_GAP)
}

/// The storeys of a cell's navmeshes, largest area first (ties: lower
/// floor first). See [`navmesh_storeys`].
pub fn storeys_with_gap(doc: &CellDoc, gap: f32) -> Vec<Storey> {
    navmesh_storeys(&doc.navmeshes, gap)
}

/// One triangle's numbers.
struct Piece {
    z_lo: f32,
    z_hi: f32,
    centroid_z: f32,
    area: f32,
    min: [f32; 2],
    max: [f32; 2],
}

/// The levels of a set of navmeshes: every triangle whose corners are all
/// present and finite, joined into one level while the next triangle (in
/// order of its lowest corner) starts at most `gap` above the highest
/// corner of the level so far. A negative or NaN `gap` is 0; an infinite
/// one gives a single level. Empty when there are no usable triangles.
/// Sorted largest area first, then lower floor first.
pub fn navmesh_storeys(navmeshes: &[NavmeshDoc], gap: f32) -> Vec<Storey> {
    let gap = gap.max(0.0);
    let mut pieces = Vec::new();
    for mesh in navmeshes {
        for index in 0..mesh.triangles.len() {
            let Some(corners) = mesh.corners(index) else {
                continue;
            };
            if !corners.iter().flatten().all(|v| v.is_finite()) {
                continue;
            }
            let [a, b, c] = corners;
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            let zs = [a[2], b[2], c[2]];
            pieces.push(Piece {
                z_lo: zs.iter().copied().fold(f32::INFINITY, f32::min),
                z_hi: zs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
                centroid_z: (a[2] + b[2] + c[2]) / 3.0,
                area: cross.abs() * 0.5,
                min: [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1])],
                max: [a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1])],
            });
        }
    }
    pieces.sort_by(|p, q| p.z_lo.total_cmp(&q.z_lo).then(p.z_hi.total_cmp(&q.z_hi)));

    let mut levels: Vec<Vec<Piece>> = Vec::new();
    let mut top = f32::NEG_INFINITY;
    for piece in pieces {
        match levels.last_mut() {
            Some(level) if piece.z_lo - top <= gap => {
                top = top.max(piece.z_hi);
                level.push(piece);
            }
            _ => {
                top = piece.z_hi;
                levels.push(vec![piece]);
            }
        }
    }

    let mut out: Vec<Storey> = levels.into_iter().map(describe).collect();
    out.sort_by(|a, b| {
        b.area
            .total_cmp(&a.area)
            .then(a.floor_z.total_cmp(&b.floor_z))
    });
    out
}

fn describe(mut level: Vec<Piece>) -> Storey {
    let area: f64 = level.iter().map(|p| f64::from(p.area)).sum();
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let mut z_range = [f32::INFINITY, f32::NEG_INFINITY];
    for p in &level {
        for k in 0..2 {
            min[k] = min[k].min(p.min[k]);
            max[k] = max[k].max(p.max[k]);
        }
        z_range[0] = z_range[0].min(p.z_lo);
        z_range[1] = z_range[1].max(p.z_hi);
    }
    level.sort_by(|p, q| p.centroid_z.total_cmp(&q.centroid_z));
    let by_count = area <= 0.0;
    let total = if by_count { level.len() as f64 } else { area };
    let weight = |p: &Piece| if by_count { 1.0 } else { f64::from(p.area) };
    let mut seen = 0.0;
    let mut floor_z = level.last().map_or(0.0, |p| p.centroid_z);
    for p in &level {
        seen += weight(p);
        if seen >= total * 0.5 {
            floor_z = p.centroid_z;
            break;
        }
    }
    Storey {
        floor_z,
        z_range,
        area: area as f32,
        triangles: level.len(),
        min,
        max,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellLocation, NavTriangleDoc};
    use crate::paths::FormKey;

    /// A navmesh of axis-aligned rectangles, two triangles each, at the
    /// given heights: `(x0, y0, x1, y1, z)`.
    fn mesh(id: u32, rects: &[(f32, f32, f32, f32, f32)]) -> NavmeshDoc {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for &(x0, y0, x1, y1, z) in rects {
            let base = vertices.len() as u32;
            vertices.extend([[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]]);
            for t in [[0, 1, 2], [0, 2, 3]] {
                triangles.push(NavTriangleDoc {
                    vertices: t.map(|i| base + i),
                    links: [-1; 3],
                    flags: 0,
                });
            }
        }
        NavmeshDoc {
            form: FormKey::new("test.esm", id),
            vertices,
            triangles,
            cover_triangles: Vec::new(),
            door_links: Vec::new(),
            external_links: Vec::new(),
        }
    }

    fn interior(navmeshes: Vec<NavmeshDoc>) -> CellDoc {
        CellDoc {
            schema_version: crate::model::SCHEMA_VERSION,
            form: FormKey::new("test.esm", 0x100),
            editor_id: Some("TestRoom".into()),
            name: None,
            location: CellLocation::Interior,
            flags: 1,
            water_height: None,
            land: None,
            references: Vec::new(),
            navmeshes,
        }
    }

    #[test]
    fn one_flat_room_is_one_storey() {
        let doc = interior(vec![mesh(1, &[(-500.0, -300.0, 700.0, 900.0, 3456.0)])]);
        let levels = storeys(&doc);
        assert_eq!(levels.len(), 1);
        let room = &levels[0];
        assert_eq!(room.floor_z, 3456.0);
        assert_eq!(room.z_range, [3456.0, 3456.0]);
        assert_eq!(room.triangles, 2);
        assert!((room.area - 1200.0 * 1200.0).abs() < 1.0);
        assert_eq!(room.min, [-500.0, -300.0]);
        assert_eq!(room.max, [700.0, 900.0]);
        assert_eq!(room.center(), [100.0, 300.0]);
        assert!(room.contains_z(3456.0) && !room.contains_z(3457.0));
    }

    #[test]
    fn two_stacked_floors_are_two_storeys_largest_first() {
        // A small upper floor in a second navmesh, 256 units above.
        let doc = interior(vec![
            mesh(1, &[(0.0, 0.0, 1000.0, 1000.0, 0.0)]),
            mesh(2, &[(0.0, 0.0, 600.0, 400.0, 256.0)]),
        ]);
        let levels = storeys(&doc);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].floor_z, 0.0);
        assert!((levels[0].area - 1.0e6).abs() < 1.0);
        assert_eq!(levels[1].floor_z, 256.0);
        assert!((levels[1].area - 240_000.0).abs() < 1.0);
        assert_eq!(levels[1].max, [600.0, 400.0]);
        // A gap wider than the storey height joins them; the floor is still
        // the big one's.
        let joined = storeys_with_gap(&doc, 300.0);
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].floor_z, 0.0);
        assert_eq!(joined[0].triangles, 4);
        assert_eq!(joined[0].z_range, [0.0, 256.0]);
        assert_eq!(storeys_with_gap(&doc, f32::INFINITY).len(), 1);
        // A NaN or negative gap is zero: two levels still.
        assert_eq!(storeys_with_gap(&doc, f32::NAN).len(), 2);
        assert_eq!(storeys_with_gap(&doc, -5.0).len(), 2);
    }

    #[test]
    fn a_small_raised_dais_stays_on_its_floor() {
        // A 2000 × 2000 floor with a 400 × 300 dais 24 units up.
        let doc = interior(vec![mesh(
            1,
            &[
                (0.0, 0.0, 2000.0, 1000.0, 100.0),
                (0.0, 1000.0, 2000.0, 2000.0, 100.0),
                (800.0, 800.0, 1200.0, 1100.0, 124.0),
            ],
        )]);
        let levels = storeys(&doc);
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].floor_z, 100.0);
        assert_eq!(levels[0].z_range, [100.0, 124.0]);
        assert_eq!(levels[0].triangles, 6);
        // A stage taller than the gap is a level of its own, after the floor.
        let doc = interior(vec![mesh(
            1,
            &[
                (0.0, 0.0, 2000.0, 2000.0, 100.0),
                (800.0, 800.0, 1200.0, 1100.0, 260.0),
            ],
        )]);
        let levels = storeys(&doc);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].floor_z, 100.0);
        assert_eq!(levels[1].floor_z, 260.0);
        assert_eq!(levels[1].min, [800.0, 800.0]);
    }

    #[test]
    fn a_ramp_joins_the_floors_it_connects() {
        // Two floors 300 apart and a ramp between them, split into steps so
        // that no single triangle spans the whole rise.
        let mut rects = vec![
            (0.0, 0.0, 1000.0, 1000.0, 0.0),
            (0.0, 2000.0, 500.0, 2500.0, 300.0),
        ];
        for step in 0..5 {
            let y = 1000.0 + step as f32 * 200.0;
            rects.push((0.0, y, 200.0, y + 200.0, 60.0 * step as f32));
        }
        let levels = storeys(&interior(vec![mesh(1, &rects)]));
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].floor_z, 0.0);
        assert_eq!(levels[0].z_range, [0.0, 300.0]);
    }

    #[test]
    fn bad_triangles_and_empty_cells_give_nothing_or_are_skipped() {
        assert!(storeys(&interior(Vec::new())).is_empty());
        let mut bad = mesh(1, &[(0.0, 0.0, 10.0, 10.0, 5.0)]);
        bad.triangles[0].vertices = [0, 1, 99];
        bad.vertices[3][2] = f32::NAN;
        // One triangle out of range, the other with a NaN corner.
        assert!(storeys(&interior(vec![bad])).is_empty());
        // Vertical triangles only: no area, the floor by count.
        let mut wall = mesh(1, &[]);
        wall.vertices = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 50.0]];
        wall.triangles.push(NavTriangleDoc {
            vertices: [0, 1, 2],
            links: [-1; 3],
            flags: 0,
        });
        let levels = storeys(&interior(vec![wall]));
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].area, 0.0);
        assert!((levels[0].floor_z - 50.0 / 3.0).abs() < 1e-4);
    }
}
