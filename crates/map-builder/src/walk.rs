//! Walkability, the sealing border and the hex geometry they share.
//!
//! Hex centres come from `HexGrid::hex_center` (map screen space,
//! contracts section 9) and are taken to tile space through the inverse
//! of the projection's ground-plane screen matrix; tile space is taken to
//! source space through the inverse of `Projection::tile_from_source`, so
//! the window's yaw is honoured. Navmesh triangles are rasterized in tile
//! space over hex centres binned by floor square.

use std::collections::{BTreeSet, VecDeque};

use foundry_render::Projection;
use mir::{CellDoc, GroundSampler, MapWindow};
use vault_formats::HexGrid;

use crate::{BuildError, BuildOptions};

/// Hexes on a map.
pub const HEXES: usize = 40_000;

/// The window's geometry: source space, tile space and map screen space.
#[derive(Debug, Clone)]
pub struct Frame {
    anchor: [f64; 2],
    /// Tile `(s, t)` per source unit east and north.
    linear: [[f64; 2]; 2],
    inverse: [[f64; 2]; 2],
    /// Tile `(s, t)` per screen pixel.
    screen_inverse: [[f64; 2]; 2],
    /// Screen pixels per tile `(s, t)`.
    screen: [[f64; 2]; 2],
    tile_units: f64,
}

fn invert(m: [[f64; 2]; 2]) -> Option<[[f64; 2]; 2]> {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    Some([
        [m[1][1] / det, -m[0][1] / det],
        [-m[1][0] / det, m[0][0] / det],
    ])
}

fn apply(m: &[[f64; 2]; 2], v: [f64; 2]) -> [f64; 2] {
    [
        m[0][0] * v[0] + m[0][1] * v[1],
        m[1][0] * v[0] + m[1][1] * v[1],
    ]
}

impl Frame {
    pub fn new(window: &MapWindow, projection: &Projection) -> Result<Self, BuildError> {
        let anchor3 = window.anchor();
        let anchor = [f64::from(anchor3[0]), f64::from(anchor3[1])];
        // Read the linear part of source -> tile from the projection itself,
        // with a step large enough that f32 rounding does not matter.
        const STEP: f32 = 1000.0;
        let at = |dx: f32, dy: f32| {
            let p = projection
                .tile_from_source(anchor3, [anchor3[0] + dx, anchor3[1] + dy, anchor3[2]]);
            [f64::from(p[0]), f64::from(p[1])]
        };
        let origin = at(0.0, 0.0);
        let east = at(STEP, 0.0);
        let north = at(0.0, STEP);
        let step = f64::from(STEP);
        let linear = [
            [(east[0] - origin[0]) / step, (north[0] - origin[0]) / step],
            [(east[1] - origin[1]) / step, (north[1] - origin[1]) / step],
        ];
        let inverse = invert(linear).ok_or_else(|| {
            BuildError::Input("the projection's ground plane is degenerate".into())
        })?;
        let vectors = projection.screen_vectors();
        let screen = [
            [f64::from(vectors[0][0]), f64::from(vectors[1][0])],
            [f64::from(vectors[0][1]), f64::from(vectors[1][1])],
        ];
        let screen_inverse = invert(screen).ok_or_else(|| {
            BuildError::Input("the projection's screen matrix is degenerate".into())
        })?;
        Ok(Self {
            anchor,
            linear,
            inverse,
            screen_inverse,
            screen,
            tile_units: f64::from(projection.tile_units()),
        })
    }

    /// Source units per floor square.
    pub fn tile_units(&self) -> f64 {
        self.tile_units
    }

    /// Ground-plane tile coordinates `(s, t)` of a source point.
    pub fn tile_of_source(&self, x: f64, y: f64) -> [f64; 2] {
        apply(&self.linear, [x - self.anchor[0], y - self.anchor[1]])
    }

    /// The source point `(x, y)` of ground-plane tile coordinates.
    pub fn source_of_tile(&self, st: [f64; 2]) -> [f64; 2] {
        let d = apply(&self.inverse, st);
        [self.anchor[0] + d[0], self.anchor[1] + d[1]]
    }

    /// Tile coordinates of map screen pixel `p` on the ground plane.
    pub fn tile_of_screen(&self, p: [f64; 2]) -> [f64; 2] {
        apply(&self.screen_inverse, p)
    }

    /// Map screen position of ground-plane tile coordinates.
    pub fn screen_of_tile(&self, st: [f64; 2]) -> [f64; 2] {
        apply(&self.screen, st)
    }

    /// Tile coordinates of a hex centre (`HexGrid::hex_center`).
    pub fn hex_centre_tile(&self, hex: i32) -> Option<[f64; 2]> {
        let centre = HexGrid::default().hex_center(hex).ok()?;
        Some(self.tile_of_screen([centre[0] as f64, centre[1] as f64]))
    }

    /// The hex under a ground-plane tile point: projected to the screen,
    /// then `HexGrid::pick_hex`.
    pub fn hex_of_tile(&self, st: [f64; 2]) -> Option<i32> {
        let p = self.screen_of_tile(st);
        if !p[0].is_finite() || !p[1].is_finite() {
            return None;
        }
        let x = p[0].round();
        let y = p[1].round();
        if x.abs() > 1e9 || y.abs() > 1e9 {
            return None;
        }
        HexGrid::default().pick_hex([x as i32, y as i32])
    }

    /// The hex under a source point's ground position.
    pub fn hex_of_source(&self, x: f64, y: f64) -> Option<i32> {
        self.hex_of_tile(self.tile_of_source(x, y))
    }

    /// The square under a hex centre (`HexGrid::pick_square`).
    pub fn square_of_hex(hex: i32) -> Option<i32> {
        let centre = HexGrid::default().hex_center(hex).ok()?;
        HexGrid::default().pick_square([centre[0] as i32, centre[1] as i32], false)
    }

    /// The source-space ground direction `(east, north)`, unit length, of
    /// hex direction `orientation` (0 to 5: NE, E, SE, SW, W, NW): the
    /// screen step `emit::HEX_STEPS[orientation]` taken to the ground
    /// plane through the projection's inverse, then to source space.
    /// `None` for an orientation above 5.
    pub fn ground_direction(&self, orientation: u8) -> Option<[f64; 2]> {
        let step = emit::HEX_STEPS.get(usize::from(orientation))?;
        let tile = self.tile_of_screen([f64::from(step[0]), f64::from(step[1])]);
        let source = apply(&self.inverse, tile);
        let length = source[0].hypot(source[1]);
        (length > 0.0 && length.is_finite()).then(|| [source[0] / length, source[1] / length])
    }

    /// The hex direction (0 to 5) a critter faces for a placed reference's
    /// rotation about Z, `yaw` in radians.
    ///
    /// Facing convention (contracts section 1): a reference is turned by
    /// `Rz(-rz)`, positive clockwise seen from above, so positive yaw turns
    /// north towards east. A reference with zero rotation faces its local
    /// +Y, which is north (the source game's actor convention; section 1
    /// fixes the sign, not the forward axis). Facing is therefore
    /// `(sin yaw, cos yaw)` in `(east, north)`, the image of +Y under
    /// `mir::Transform::matrix`. The result is the direction whose
    /// [`Frame::ground_direction`] has the largest dot product with that
    /// facing (the smallest angle); an exact tie goes to the lower
    /// direction. `None` for a non-finite yaw.
    pub fn orientation_of_yaw(&self, yaw: f32) -> Option<u8> {
        if !yaw.is_finite() {
            return None;
        }
        let (sin, cos) = f64::from(yaw).sin_cos();
        let mut best: Option<(f64, u8)> = None;
        for orientation in 0..6u8 {
            let g = self.ground_direction(orientation)?;
            let dot = g[0] * sin + g[1] * cos;
            if best.is_none_or(|(b, _)| dot > b) {
                best = Some((dot, orientation));
            }
        }
        best.map(|(_, orientation)| orientation)
    }

    /// The source yaw (radians, in `(-π, π]`, convention of
    /// [`Frame::orientation_of_yaw`]) whose facing projects onto hex
    /// direction `orientation`'s screen step: `atan2(east, north)` of its
    /// [`Frame::ground_direction`]. A sprite rendered at this yaw walks
    /// exactly along `emit::HEX_STEPS[orientation]` on screen, and
    /// [`Frame::orientation_of_yaw`] maps it back to `orientation`.
    /// `None` for an orientation above 5.
    pub fn yaw_of_orientation(&self, orientation: u8) -> Option<f32> {
        let g = self.ground_direction(orientation)?;
        Some(g[0].atan2(g[1]) as f32)
    }
}

/// What the walkability pass counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WalkStats {
    pub walkable: usize,
    /// Navmesh triangles seen, rasterized, and skipped as high above the
    /// terrain or as unreadable (a vertex index out of range).
    pub triangles: usize,
    pub triangles_rasterized: usize,
    pub triangles_too_high: usize,
    pub triangles_unreadable: usize,
    /// Hexes some triangle covers.
    pub covered: usize,
    /// Hexes in a cell without a navmesh, judged by slope alone.
    pub slope_only: usize,
    /// Hexes rejected for no navmesh coverage, too steep a slope, no
    /// terrain (in a cell without a navmesh), or a blocked hex or blocking
    /// placement.
    pub rejected_navmesh: usize,
    pub rejected_slope: usize,
    pub rejected_no_ground: usize,
    pub rejected_blocked: usize,
}

/// Why a hex is, or is not, walkable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verdict {
    /// Not on the grid's projected ground.
    #[default]
    OffGrid,
    Walkable,
    /// In a cell with a navmesh, under no triangle of it.
    OffNavmesh,
    TooSteep,
    /// In a cell without a navmesh, and without terrain.
    NoGround,
    /// A blocked hex, or the hex of a blocking placement.
    Blocked,
}

fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn segment_distance_sq(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let length = d[0] * d[0] + d[1] * d[1];
    let u = if length > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + u * d[0] - p[0], a[1] + u * d[1] - p[1]];
    q[0] * q[0] + q[1] * q[1]
}

/// Whether `p` lies inside triangle `abc` or within `tolerance` of it.
pub fn covers(p: [f64; 2], [a, b, c]: [[f64; 2]; 3], tolerance: f64) -> bool {
    let area = cross(a, b, c);
    if area.abs() > 1e-12 {
        let d1 = cross(a, b, p);
        let d2 = cross(b, c, p);
        let d3 = cross(c, a, p);
        let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        if !(negative && positive) {
            return true;
        }
    }
    let limit = tolerance.max(0.0).powi(2);
    segment_distance_sq(p, a, b) <= limit
        || segment_distance_sq(p, b, c) <= limit
        || segment_distance_sq(p, c, a) <= limit
}

/// Side of the bucket grid: one bucket per floor square, plus a margin.
const BUCKETS: usize = 102;

fn bucket(v: f64) -> usize {
    (v.floor().clamp(0.0, (BUCKETS - 1) as f64)) as usize
}

/// The walkability grid (see the crate notes) and its counts. `blocked`
/// holds the caller's blocked hexes and the hexes of blocking placements.
pub fn walkability(
    frame: &Frame,
    sampler: &GroundSampler,
    cells: &[CellDoc],
    blocked: &[bool],
    options: &BuildOptions,
) -> (Vec<bool>, WalkStats) {
    let (verdicts, stats) = verdicts(frame, sampler, cells, blocked, options);
    let walkable = verdicts.iter().map(|v| *v == Verdict::Walkable).collect();
    (walkable, stats)
}

/// [`walkability`] with the reason for every hex.
pub fn verdicts(
    frame: &Frame,
    sampler: &GroundSampler,
    cells: &[CellDoc],
    blocked: &[bool],
    options: &BuildOptions,
) -> (Vec<Verdict>, WalkStats) {
    let mut stats = WalkStats::default();
    let centres: Vec<[f64; 2]> = (0..HEXES as i32)
        .map(|hex| frame.hex_centre_tile(hex).unwrap_or([f64::NAN; 2]))
        .collect();
    let mut buckets = vec![Vec::<u32>::new(); BUCKETS * BUCKETS];
    for (hex, c) in centres.iter().enumerate() {
        if c[0].is_finite() && c[1].is_finite() {
            buckets[bucket(c[1]) * BUCKETS + bucket(c[0])].push(hex as u32);
        }
    }
    let tolerance = f64::from(options.navmesh_tolerance.max(0.0)) / frame.tile_units();
    let max_height = f64::from(options.navmesh_max_height);
    let mut covered = vec![false; HEXES];
    let mut nav_cells = BTreeSet::new();
    for cell in cells.iter().filter(|cell| !cell.is_interior()) {
        let mir::CellLocation::Exterior { x, y, .. } = &cell.location else {
            continue;
        };
        for navmesh in &cell.navmeshes {
            if !navmesh.triangles.is_empty() {
                nav_cells.insert((*x, *y));
            }
            for index in 0..navmesh.triangles.len() {
                stats.triangles += 1;
                let Some(corners) = navmesh.corners(index) else {
                    stats.triangles_unreadable += 1;
                    continue;
                };
                if corners.iter().flatten().any(|v| !v.is_finite()) {
                    stats.triangles_unreadable += 1;
                    continue;
                }
                let too_high = corners.iter().all(|v| {
                    sampler
                        .height(v[0], v[1])
                        .is_some_and(|ground| f64::from(v[2] - ground) > max_height)
                });
                if too_high {
                    stats.triangles_too_high += 1;
                    continue;
                }
                stats.triangles_rasterized += 1;
                let tri = corners.map(|v| frame.tile_of_source(f64::from(v[0]), f64::from(v[1])));
                let lo = [0, 1].map(|axis| {
                    tri.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min) - tolerance
                });
                let hi = [0, 1].map(|axis| {
                    tri.iter()
                        .map(|p| p[axis])
                        .fold(f64::NEG_INFINITY, f64::max)
                        + tolerance
                });
                if hi[0] < 0.0 || hi[1] < 0.0 || lo[0] >= BUCKETS as f64 || lo[1] >= BUCKETS as f64
                {
                    continue;
                }
                for by in bucket(lo[1])..=bucket(hi[1]) {
                    for bx in bucket(lo[0])..=bucket(hi[0]) {
                        for &hex in &buckets[by * BUCKETS + bx] {
                            let hex = hex as usize;
                            if !covered[hex] && covers(centres[hex], tri, tolerance) {
                                covered[hex] = true;
                            }
                        }
                    }
                }
            }
        }
    }
    let limit = options.slope_limit;
    let mut verdicts = vec![Verdict::OffGrid; HEXES];
    for hex in 0..HEXES {
        let c = centres[hex];
        if !c[0].is_finite() {
            continue;
        }
        if covered[hex] {
            stats.covered += 1;
        }
        let [x, y] = frame.source_of_tile(c);
        let cell = mir::cell_of(&[x as f32, y as f32, 0.0]);
        let has_navmesh = nav_cells.contains(&cell);
        if !has_navmesh {
            stats.slope_only += 1;
        } else if !covered[hex] {
            stats.rejected_navmesh += 1;
            verdicts[hex] = Verdict::OffNavmesh;
            continue;
        }
        match sampler.slope(x as f32, y as f32) {
            Some(slope) if slope <= limit => {}
            Some(_) => {
                stats.rejected_slope += 1;
                verdicts[hex] = Verdict::TooSteep;
                continue;
            }
            // No terrain: a covering navmesh is trusted, slope alone is not.
            None if has_navmesh => {}
            None => {
                stats.rejected_no_ground += 1;
                verdicts[hex] = Verdict::NoGround;
                continue;
            }
        }
        if blocked.get(hex).copied().unwrap_or(false) {
            stats.rejected_blocked += 1;
            verdicts[hex] = Verdict::Blocked;
            continue;
        }
        verdicts[hex] = Verdict::Walkable;
        stats.walkable += 1;
    }
    (verdicts, stats)
}

/// The hexes the source game's own navigation data lets an actor stand
/// on: in a cell with a navmesh, under a triangle of it, and no steeper
/// than the slope limit. Sorted. A slicer that blocks a building by its
/// bounding footprint should leave these open (a porch, a canopy).
pub fn standable_hexes(
    frame: &Frame,
    sampler: &GroundSampler,
    cells: &[CellDoc],
    options: &BuildOptions,
) -> Vec<i32> {
    let (cover, _) = verdicts(frame, sampler, cells, &[], options);
    let nav_cells: BTreeSet<(i32, i32)> = cells
        .iter()
        .filter(|cell| cell.navmeshes.iter().any(|n| !n.triangles.is_empty()))
        .filter_map(|cell| match &cell.location {
            mir::CellLocation::Exterior { x, y, .. } => Some((*x, *y)),
            _ => None,
        })
        .collect();
    (0..HEXES as i32)
        .filter(|&hex| {
            cover[hex as usize] == Verdict::Walkable
                && frame.hex_centre_tile(hex).is_some_and(|c| {
                    let [x, y] = frame.source_of_tile(c);
                    nav_cells.contains(&mir::cell_of(&[x as f32, y as f32, 0.0]))
                })
        })
        .collect()
}

/// The six neighbours of `hex` that are on the grid.
pub fn neighbours(hex: i32) -> impl Iterator<Item = i32> {
    (0..6u8).filter_map(move |orientation| HexGrid::neighbor(hex, orientation).ok().flatten())
}

/// The sealing border: every hex that is not walkable, holds no object
/// the engine blocks movement with (`engine_blocked`), and touches a hex
/// a walker can stand on or pass through (`walkable` or `open`, the hexes
/// of doors). Sorted by hex.
pub fn sealing_border(walkable: &[bool], engine_blocked: &[bool], open: &[bool]) -> Vec<i32> {
    let reach = |hex: i32| {
        let hex = hex as usize;
        walkable.get(hex).copied().unwrap_or(false) || open.get(hex).copied().unwrap_or(false)
    };
    (0..HEXES as i32)
        .filter(|&hex| {
            let h = hex as usize;
            !walkable[h] && !engine_blocked[h] && neighbours(hex).any(reach)
        })
        .collect()
}

/// The hexes reachable from `start` through hexes `passable` accepts,
/// stepping between neighbours.
pub fn reachable(start: i32, passable: impl Fn(i32) -> bool) -> Vec<bool> {
    let mut seen = vec![false; HEXES];
    if !(0..HEXES as i32).contains(&start) || !passable(start) {
        return seen;
    }
    let mut queue = VecDeque::from([start]);
    seen[start as usize] = true;
    while let Some(hex) = queue.pop_front() {
        for next in neighbours(hex) {
            if !seen[next as usize] && passable(next) {
                seen[next as usize] = true;
                queue.push_back(next);
            }
        }
    }
    seen
}

/// The walkable hex whose centre is nearest the ground point `target`
/// (tile space), skipping `excluded`; ties go to the lower hex.
pub fn nearest_walkable(
    frame: &Frame,
    walkable: &[bool],
    target: [f64; 2],
    excluded: &BTreeSet<i32>,
) -> Option<i32> {
    let mut best: Option<(f64, i32)> = None;
    for hex in 0..HEXES as i32 {
        if !walkable[hex as usize] || excluded.contains(&hex) {
            continue;
        }
        let Some(c) = frame.hex_centre_tile(hex) else {
            continue;
        };
        let d = (c[0] - target[0]).powi(2) + (c[1] - target[1]).powi(2);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, hex));
        }
    }
    best.map(|(_, hex)| hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        let calibration = foundry_render::Calibration::default();
        let projection = Projection::new(&calibration).unwrap();
        let window = MapWindow::from_center([0.0, 0.0], projection.tile_units());
        Frame::new(&window, &projection).unwrap()
    }

    #[test]
    fn hex_centres_follow_the_contract_formula() {
        let frame = frame();
        for hex in [0, 1, 199, 200, 20100, 39999, 12345] {
            let h = f64::from(199 - hex % 200);
            let r = f64::from(hex / 200);
            let reference = [h / 2.0, r / 2.0 + (h % 2.0) / 4.0];
            let centre = frame.hex_centre_tile(hex).unwrap();
            // The centre is (+16, +8) pixels from the reference.
            let offset = frame.tile_of_screen([16.0, 8.0]);
            assert!((centre[0] - reference[0] - offset[0]).abs() < 1e-9, "{hex}");
            assert!((centre[1] - reference[1] - offset[1]).abs() < 1e-9, "{hex}");
            assert_eq!(frame.hex_of_tile(centre), Some(hex));
        }
    }

    #[test]
    fn source_and_tile_space_invert() {
        let frame = frame();
        let st = frame.tile_of_source(123.0, -456.0);
        let back = frame.source_of_tile(st);
        assert!((back[0] - 123.0).abs() < 1e-3 && (back[1] + 456.0).abs() < 1e-3);
        // East is +s, south is +t.
        let east = frame.tile_of_source(263.0, -456.0);
        assert!((east[0] - st[0] - 1.0).abs() < 1e-4 && (east[1] - st[1]).abs() < 1e-4);
    }

    #[test]
    fn triangle_coverage_with_tolerance() {
        let tri = [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]];
        assert!(covers([1.0, 1.0], tri, 0.0));
        assert!(covers([2.0, 2.0], tri, 0.0), "on the edge");
        assert!(!covers([2.1, 2.1], tri, 0.0));
        assert!(covers([2.1, 2.1], tri, 0.2));
        assert!(!covers([-1.0, -1.0], tri, 0.5));
        // A degenerate triangle covers only what is near its segments.
        let flat = [[0.0, 0.0], [4.0, 0.0], [8.0, 0.0]];
        assert!(covers([3.0, 0.05], flat, 0.1));
        assert!(!covers([3.0, 1.0], flat, 0.1));
    }

    #[test]
    fn flood_fill_respects_passability() {
        let walls: BTreeSet<i32> = neighbours(20100).collect();
        let seen = reachable(20100, |hex| !walls.contains(&hex));
        assert_eq!(seen.iter().filter(|s| **s).count(), 1);
        let open = reachable(20100, |_| true);
        assert!(open.iter().all(|s| *s));
    }
}
