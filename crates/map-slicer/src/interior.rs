//! Interior cells: one common floor and a ceiling cut.
//!
//! An interior cell has no land. Its references and navmesh sit in the
//! cell's own coordinates around some floor height, and its kit pieces hold
//! floor, walls and ceiling in one mesh. Seen from above, as the engine
//! shows a map, the ceilings, beams, hanging lights and any upper storey
//! would cover the room. So with [`crate::SlicerConfig::interior`] set:
//!
//! 1. **One ground.** Every reference stands on [`InteriorSlice::floor_z`],
//!    whatever terrain sampler is passed: its ground point is its origin
//!    dropped to that height, its height above it the drawn offset (crate
//!    documentation, "Map screen space and anchoring"). A bottle on a table
//!    is drawn where it stands, at the hex under its origin.
//! 2. **Floor bake.** Up-facing triangles (at least `floor_min_up`, either
//!    face when two-sided) whose corners all lie within
//!    [`InteriorSlice::floor_band`] of the floor, from every rendered
//!    piece, leave the piece and are baked into the ground overlays at
//!    their true height (a dais reads as a dais, in line with its sides),
//!    composited first, under rugs and decals. A room piece keeps only its
//!    walls; a piece that is all floor places nothing upright. The baked
//!    hexes are walkable: nothing of the piece stands on them.
//! 3. **Ceiling cut.** Geometry above `floor_z + cut_height` is clipped
//!    away (a wall keeps its lower part); one-sided triangles facing down
//!    above the floor band (ceilings and soffits under the cut, undersides)
//!    are dropped. References whose whole placed bounds lie above the cut,
//!    or that reach above it but hang higher than
//!    [`InteriorSlice::hang_clearance`] (lights, fans, ceiling signs), or
//!    lie wholly below the floor band (a lower storey), are skipped:
//!    [`crate::SkipReason::AboveCut`], [`crate::SkipReason::Hanging`],
//!    [`crate::SkipReason::BelowFloor`].
//! 4. **Classes.** A flat mesh (a rug, a decal) is ground flat only when it
//!    lies within the floor band; one lifted off the floor (papers on a
//!    desk) is classified as an upright object of its size. Relief and
//!    terrain patterns do not apply (cave rock is a wall indoors), and a
//!    footprint above `max_sliced_hexes` (a one-piece interior shell) is
//!    sliced: floor baked, walls cut into strips under the cut. The budget
//!    ladder may still make it a landmark or relief; both get the same
//!    floor bake and cut.
//! 5. **Blocking.** Strips block as outdoors (near the footprint, never an
//!    open hex), but a room piece's strips are now only its walls: the
//!    baked floor owns no strip, so its hexes stay walkable. Cut walls
//!    placed as relief or as a landmark have no top face left, so they
//!    block by outline instead: the hexes within the strips' reach (0.3
//!    floor squares) of a triangle more than `step_height` above the
//!    floor, open hexes excepted.
//!
//! The render cache is keyed by the origin's height above the floor too
//! (rounded to a unit), since the cut and the floor band depend on it.
//!
//! Doors, containers and carried items are never clipped and give no
//! floor: they are drawn whole (the three skips above still apply).
//! Walking surfaces above the floor band (a bar top, a table) are not
//! turned into floor pieces ([`crate::Role::FloorPiece`]) indoors: they
//! stay part of the object and keep occluding.

use std::collections::{BTreeMap, BTreeSet};

use nif::{Primitive, StaticMesh};

use crate::floor;
use crate::hex::Plane;

/// Interior slicing: see the module documentation. Heights are source
/// units; all but `floor_z` are relative to the floor.
#[derive(Debug, Clone, PartialEq)]
pub struct InteriorSlice {
    /// The floor's source height (the cell's own coordinates): the ground
    /// every reference stands on.
    pub floor_z: f32,
    /// Geometry higher than this above the floor is clipped away. The
    /// default, 210, is one engine storey (3 m at 70 units a metre).
    pub cut_height: f32,
    /// `[low, high]`: an up-facing triangle whose corners all lie in this
    /// band above the floor is floor and is baked into the ground. The
    /// default, `[-16, 32]`, holds kit floors modelled a little off the
    /// cell's floor height, doorsteps, low steps and daises, and stays
    /// under seats (about 40) and table tops (about 55). The low end also
    /// bounds the lower storey: a reference wholly below it is skipped.
    pub floor_band: [f32; 2],
    /// A reference reaching above the cut whose lowest point is higher
    /// than this above the floor hangs from the ceiling and is skipped. The
    /// default, 160, keeps posters and most paintings on the walls (their
    /// lower edges about 120 to 160 up in a real saloon, clipped at the
    /// cut) and skips lamps, fans, chandeliers and ceiling signs (165 and
    /// up there). Wall art hung higher is skipped with them.
    pub hang_clearance: f32,
}

impl InteriorSlice {
    /// The defaults for a floor at `floor_z`.
    pub fn at(floor_z: f32) -> Self {
        InteriorSlice {
            floor_z,
            cut_height: 210.0,
            floor_band: [-16.0, 32.0],
            hang_clearance: 160.0,
        }
    }

    /// The source height of the cut.
    pub fn cut_z(&self) -> f32 {
        self.floor_z + self.cut_height
    }

    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        let finite = self.floor_z.is_finite()
            && self.cut_height.is_finite()
            && self.hang_clearance.is_finite()
            && self.floor_band.iter().all(|v| v.is_finite());
        if !finite {
            return Err("interior heights must be finite");
        }
        if !(self.floor_band[0] <= self.floor_band[1] && self.floor_band[1] < self.cut_height) {
            return Err("interior floor_band must rise and stay under cut_height");
        }
        Ok(())
    }

    /// Why a reference whose placed bounds span `z` (source heights) is
    /// skipped indoors, if it is.
    pub(crate) fn skip(&self, z: [f32; 2]) -> Option<crate::SkipReason> {
        let [lo, hi] = z.map(|v| v - self.floor_z);
        if lo >= self.cut_height {
            Some(crate::SkipReason::AboveCut)
        } else if hi < self.floor_band[0] {
            Some(crate::SkipReason::BelowFloor)
        } else if hi > self.cut_height && lo > self.hang_clearance {
            Some(crate::SkipReason::Hanging)
        } else {
            None
        }
    }

    /// Whether a flat reference whose lowest point is at `lo` (source
    /// height) lies on the floor.
    pub(crate) fn on_floor(&self, lo: f32) -> bool {
        lo - self.floor_z <= self.floor_band[1]
    }
}

impl Default for InteriorSlice {
    /// The defaults for a floor at height 0.
    fn default() -> Self {
        InteriorSlice::at(0.0)
    }
}

/// One placed mesh indoors: its walls and its floor, both mesh-local.
#[derive(Debug, Clone, Default)]
pub(crate) struct Split {
    /// Everything that is not floor, under the cut; `None` when nothing is
    /// left.
    pub walls: Option<StaticMesh>,
    /// The floor triangles; `None` without any.
    pub floor: Option<StaticMesh>,
    /// Triangles moved to the floor.
    pub floor_triangles: usize,
    /// One-sided down-facing triangles above the floor band, dropped.
    pub ceiling_triangles: usize,
    /// Triangles wholly or partly above the cut.
    pub cut_triangles: usize,
}

/// New vertices of one primitive, made where an edge crosses the cut.
struct Grown<'a> {
    source: &'a Primitive,
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Option<Vec<[f32; 2]>>,
    colors: Option<Vec<[f32; 4]>>,
    /// Edge (low index, high index) to its new vertex.
    edges: BTreeMap<(u32, u32), u32>,
}

impl<'a> Grown<'a> {
    fn new(source: &'a Primitive) -> Self {
        Grown {
            source,
            positions: source.positions.clone(),
            normals: source.normals.clone(),
            uvs: source.uvs.clone(),
            colors: source.colors.clone(),
            edges: BTreeMap::new(),
        }
    }

    fn grew(&self) -> bool {
        self.positions.len() > self.source.positions.len()
    }

    /// The vertex where edge `a`-`b` meets the cut, `t` along it from `a`.
    fn cross(&mut self, a: u32, b: u32, t: f32) -> u32 {
        // One vertex per edge, whichever way it is walked.
        let (key, t) = if a < b {
            ((a, b), t)
        } else {
            ((b, a), 1.0 - t)
        };
        if let Some(&v) = self.edges.get(&key) {
            return v;
        }
        let (i, j) = (key.0 as usize, key.1 as usize);
        let lerp3 = |p: [f32; 3], q: [f32; 3]| [0, 1, 2].map(|k| p[k] + (q[k] - p[k]) * t);
        let v = self.positions.len() as u32;
        let p = lerp3(self.positions[i], self.positions[j]);
        self.positions.push(p);
        if let Some(n) = &mut self.normals {
            let m = lerp3(n[i], n[j]);
            let length = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
            n.push(if length > 0.0 && length.is_finite() {
                m.map(|c| c / length)
            } else {
                n[i]
            });
        }
        if let Some(uv) = &mut self.uvs {
            let (p, q) = (uv[i], uv[j]);
            uv.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
        }
        if let Some(c) = &mut self.colors {
            let (p, q) = (c[i], c[j]);
            c.push([0, 1, 2, 3].map(|k| p[k] + (q[k] - p[k]) * t));
        }
        self.edges.insert(key, v);
        v
    }

    fn into_primitive(self, indices: Vec<u32>) -> Primitive {
        let grew = self.grew();
        let mut out = Primitive {
            positions: self.positions,
            normals: self.normals,
            uvs: self.uvs,
            colors: self.colors,
            indices,
            ..self.source.clone()
        };
        if grew {
            // Skin weights do not follow interpolated vertices; a static
            // render never reads them.
            out.skin = None;
            out.skinned = false;
        }
        out
    }
}

fn mesh_of(primitives: Vec<Primitive>, like: &StaticMesh) -> Option<StaticMesh> {
    (!primitives.is_empty()).then(|| StaticMesh {
        primitives,
        collision: like.collision.clone(),
        skipped_blocks: like.skipped_blocks.clone(),
    })
}

/// Splits `mesh`, placed by `matrix` (a rotation and scale, no
/// translation) with its origin `lift` above the floor, into walls under
/// the cut and floor (module documentation). `min_up` is the floor test's
/// least upward normal component.
pub(crate) fn split(
    mesh: &StaticMesh,
    matrix: &[[f32; 4]; 3],
    lift: f32,
    interior: &InteriorSlice,
    min_up: f32,
) -> Split {
    let mut out = Split::default();
    let mut walls = Vec::new();
    let mut floors = Vec::new();
    let cut = interior.cut_height;
    for primitive in mesh.primitives.iter().filter(|p| p.is_well_formed()) {
        let placed: Vec<[f32; 3]> = primitive
            .positions
            .iter()
            .map(|p| {
                [0, 1, 2].map(|r| {
                    matrix[r][0] * p[0] + matrix[r][1] * p[1] + matrix[r][2] * p[2] + matrix[r][3]
                })
            })
            .collect();
        let two_sided = primitive.material.two_sided;
        let mut grown = Grown::new(primitive);
        let mut wall_indices = Vec::new();
        let mut floor_indices = Vec::new();
        for triangle in primitive.indices.as_chunks::<3>().0 {
            let corners = triangle.map(|i| placed[i as usize]);
            if floor::is_floor(corners, two_sided, lift, interior.floor_band, min_up) {
                floor_indices.extend_from_slice(triangle);
                out.floor_triangles += 1;
                continue;
            }
            let heights = corners.map(|p| p[2] + lift);
            let low = heights.iter().copied().fold(f32::INFINITY, f32::min);
            if !two_sided
                && low > interior.floor_band[1]
                && floor::normal_up(corners).is_some_and(|up| up <= -min_up)
            {
                out.ceiling_triangles += 1;
                continue;
            }
            let under = heights.map(|h| h <= cut);
            if under.iter().all(|u| *u) {
                wall_indices.extend_from_slice(triangle);
                continue;
            }
            out.cut_triangles += 1;
            if !under.iter().any(|u| *u) {
                continue;
            }
            // One plane against a triangle (Sutherland-Hodgman): the part
            // under the cut is a triangle or a quad, in the same winding.
            let mut polygon: Vec<u32> = Vec::with_capacity(4);
            for k in 0..3 {
                let (a, b) = (k, (k + 1) % 3);
                if under[a] {
                    polygon.push(triangle[a]);
                }
                if under[a] != under[b] {
                    let t = (cut - heights[a]) / (heights[b] - heights[a]);
                    polygon.push(grown.cross(triangle[a], triangle[b], t.clamp(0.0, 1.0)));
                }
            }
            for k in 1..polygon.len().saturating_sub(1) {
                wall_indices.extend_from_slice(&[polygon[0], polygon[k], polygon[k + 1]]);
            }
        }
        if !floor_indices.is_empty() {
            floors.push(Primitive {
                indices: floor_indices,
                ..primitive.clone()
            });
        }
        if !wall_indices.is_empty() {
            walls.push(grown.into_primitive(wall_indices));
        }
    }
    out.walls = mesh_of(walls, mesh);
    out.floor = mesh_of(floors, mesh);
    out
}

/// The floor area (square source units, on the ground plane) of `mesh`
/// placed by `matrix` (the full placement, translation included), and the
/// hexes whose centre lies under it, added to `hexes`.
pub(crate) fn floor_cover(
    plane: &Plane,
    mesh: &StaticMesh,
    matrix: &[[f32; 4]; 3],
    hexes: &mut BTreeSet<i32>,
) -> f64 {
    let mut area = 0.0;
    for primitive in mesh.primitives.iter().filter(|p| p.is_well_formed()) {
        let placed: Vec<[f64; 2]> = primitive
            .positions
            .iter()
            .map(|p| {
                [0, 1].map(|r| {
                    f64::from(
                        matrix[r][0] * p[0]
                            + matrix[r][1] * p[1]
                            + matrix[r][2] * p[2]
                            + matrix[r][3],
                    )
                })
            })
            .collect();
        for triangle in primitive.indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|i| placed[i as usize]);
            let twice = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if !twice.is_finite() || twice.abs() < 1.0e-9 {
                continue;
            }
            area += twice.abs() / 2.0;
            let tile = [a, b, c].map(|p| plane.tile_of_source(p));
            hexes.extend(plane.hexes_in(&tile));
        }
    }
    area
}

/// Hexes the walls of a split mesh block indoors, added to `out`: those
/// whose centre lies within `reach` floor squares of the ground outline of
/// a triangle reaching more than `step` above the floor. `matrix` is the
/// full placement and `floor_z` the floor's source height. (Cut at the
/// ceiling, a wall has no top face left, so the relief rule, hexes under a
/// high triangle, would leave it open.)
pub(crate) fn blocked_by_walls(
    plane: &Plane,
    walls: &StaticMesh,
    matrix: &[[f32; 4]; 3],
    floor_z: f32,
    step: f32,
    reach: f64,
    out: &mut BTreeSet<i32>,
) {
    for primitive in walls.primitives.iter().filter(|p| p.is_well_formed()) {
        let placed: Vec<[f64; 3]> = primitive
            .positions
            .iter()
            .map(|p| {
                [0, 1, 2].map(|r| {
                    f64::from(
                        matrix[r][0] * p[0]
                            + matrix[r][1] * p[1]
                            + matrix[r][2] * p[2]
                            + matrix[r][3],
                    )
                })
            })
            .collect();
        for triangle in primitive.indices.as_chunks::<3>().0 {
            let corners = triangle.map(|i| placed[i as usize]);
            if corners.iter().flatten().any(|v| !v.is_finite()) {
                continue;
            }
            let high = corners.iter().map(|p| p[2]).fold(f64::MIN, f64::max);
            if high - f64::from(floor_z) <= f64::from(step) {
                continue;
            }
            let tile = corners.map(|p| plane.tile_of_source([p[0], p[1]]));
            let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
            for p in &tile {
                for k in 0..2 {
                    lo[k] = lo[k].min(p[k] - reach);
                    hi[k] = hi[k].max(p[k] + reach);
                }
            }
            let around = [
                [lo[0], lo[1]],
                [hi[0], lo[1]],
                [hi[0], hi[1]],
                [lo[0], hi[1]],
            ];
            for h in plane.hexes_in(&around) {
                if out.contains(&h) {
                    continue;
                }
                if plane
                    .hex_tile(h)
                    .is_some_and(|c| crate::slice::distance_to_polygon(&tile, c) <= reach)
                {
                    out.insert(h);
                }
            }
        }
    }
}
