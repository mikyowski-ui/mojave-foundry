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

use crate::{BuildError, BuildOptions, INTERIOR_BAND_ABOVE, INTERIOR_BAND_BELOW, InteriorBuild};

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
    /// Interior builds: triangles skipped as outside the floor level's
    /// band (an upper storey, a cellar); exterior levels: triangles of
    /// other levels. Always 0 on a one-level exterior build.
    pub triangles_off_level: usize,
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

/// Hex centres in tile space, binned by floor square, for rasterizing
/// triangles.
pub(crate) struct Raster {
    pub centres: Vec<[f64; 2]>,
    buckets: Vec<Vec<u32>>,
}

impl Raster {
    /// Every hex whose centre the tile-space triangle `tri` covers (within
    /// `tolerance` tiles), in bucket order.
    pub(crate) fn hexes_under(&self, tri: [[f64; 2]; 3], tolerance: f64) -> Vec<u32> {
        let lo = [0, 1]
            .map(|axis| tri.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min) - tolerance);
        let hi = [0, 1].map(|axis| {
            tri.iter()
                .map(|p| p[axis])
                .fold(f64::NEG_INFINITY, f64::max)
                + tolerance
        });
        let mut out = Vec::new();
        if hi[0] < 0.0 || hi[1] < 0.0 || lo[0] >= BUCKETS as f64 || lo[1] >= BUCKETS as f64 {
            return out;
        }
        for by in bucket(lo[1])..=bucket(hi[1]) {
            for bx in bucket(lo[0])..=bucket(hi[0]) {
                for &hex in &self.buckets[by * BUCKETS + bx] {
                    if covers(self.centres[hex as usize], tri, tolerance) {
                        out.push(hex);
                    }
                }
            }
        }
        out
    }

    pub(crate) fn new(frame: &Frame) -> Self {
        let centres: Vec<[f64; 2]> = (0..HEXES as i32)
            .map(|hex| frame.hex_centre_tile(hex).unwrap_or([f64::NAN; 2]))
            .collect();
        let mut buckets = vec![Vec::<u32>::new(); BUCKETS * BUCKETS];
        for (hex, c) in centres.iter().enumerate() {
            if c[0].is_finite() && c[1].is_finite() {
                buckets[bucket(c[1]) * BUCKETS + bucket(c[0])].push(hex as u32);
            }
        }
        Self { centres, buckets }
    }

    /// Mark every hex whose centre the tile-space triangle `tri` covers
    /// (within `tolerance` tiles).
    fn fill(&self, tri: [[f64; 2]; 3], tolerance: f64, covered: &mut [bool]) {
        let lo = [0, 1]
            .map(|axis| tri.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min) - tolerance);
        let hi = [0, 1].map(|axis| {
            tri.iter()
                .map(|p| p[axis])
                .fold(f64::NEG_INFINITY, f64::max)
                + tolerance
        });
        if hi[0] < 0.0 || hi[1] < 0.0 || lo[0] >= BUCKETS as f64 || lo[1] >= BUCKETS as f64 {
            return;
        }
        for by in bucket(lo[1])..=bucket(hi[1]) {
            for bx in bucket(lo[0])..=bucket(hi[0]) {
                for &hex in &self.buckets[by * BUCKETS + bx] {
                    let hex = hex as usize;
                    if !covered[hex] && covers(self.centres[hex], tri, tolerance) {
                        covered[hex] = true;
                    }
                }
            }
        }
    }

    /// Set, in `masks`, the bit of every sample (hex centre plus
    /// `offsets[k]`, tile space) the triangle covers.
    fn fill_samples(&self, tri: [[f64; 2]; 3], offsets: &[[f64; 2]], masks: &mut [u64]) {
        // A hex's samples lie within about half a tile of its centre.
        let reach = offsets
            .iter()
            .map(|o| o[0].abs().max(o[1].abs()))
            .fold(0.0f64, f64::max);
        let lo =
            [0, 1].map(|axis| tri.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min) - reach);
        let hi = [0, 1].map(|axis| {
            tri.iter()
                .map(|p| p[axis])
                .fold(f64::NEG_INFINITY, f64::max)
                + reach
        });
        if hi[0] < 0.0 || hi[1] < 0.0 || lo[0] >= BUCKETS as f64 || lo[1] >= BUCKETS as f64 {
            return;
        }
        for by in bucket(lo[1])..=bucket(hi[1]) {
            for bx in bucket(lo[0])..=bucket(hi[0]) {
                for &hex in &self.buckets[by * BUCKETS + bx] {
                    let hex = hex as usize;
                    let c = self.centres[hex];
                    for (k, o) in offsets.iter().enumerate() {
                        let bit = 1u64 << k;
                        if masks[hex] & bit == 0 && covers([c[0] + o[0], c[1] + o[1]], tri, 0.0) {
                            masks[hex] |= bit;
                        }
                    }
                }
            }
        }
    }
}

/// Which hexes a navmesh triangle covers ([`BuildOptions::navmesh_coverage`]).
///
/// A hex is about 70 source units across (half a floor square), and the
/// navmesh stops at furniture: a pool table or a counter is a hole in it.
/// The tolerance rule calls a hex covered when its centre comes within
/// [`BuildOptions::navmesh_tolerance`] (24 units) of a triangle, so a hex
/// whose centre lies under the edge of a table counts as floor. The two
/// stricter rules are for interiors.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum NavmeshCoverage {
    /// The hex centre lies inside a triangle or within
    /// [`BuildOptions::navmesh_tolerance`] source units of it. The default,
    /// and the rule of every earlier build.
    #[default]
    Tolerance,
    /// The hex centre lies inside a triangle (no tolerance).
    Centre,
    /// At least this share (0 to 1) of the hex's own ground lies inside
    /// the triangles: the hex's screen pixels (those `HexGrid::pick_hex`
    /// gives the hex), sampled every [`SHARE_STEP`] pixels, taken to the
    /// ground plane. Above 0; 1 asks for every sample.
    Share(f32),
}

/// Screen pixels between the samples of [`NavmeshCoverage::Share`].
pub const SHARE_STEP: i64 = 4;

/// The screen offsets, from a hex's centre, of the samples of
/// [`NavmeshCoverage::Share`]: every [`SHARE_STEP`] pixels, those that
/// `pick_hex` gives the hex itself (the shape is the same for every hex).
fn share_offsets() -> Vec<[i64; 2]> {
    let grid = HexGrid::default();
    let hex = 100 * 200 + 100;
    let Ok(centre) = grid.hex_center(hex) else {
        return vec![[0, 0]];
    };
    let mut out = Vec::new();
    let mut dy = -16;
    while dy <= 16 {
        let mut dx = -24;
        while dx <= 24 {
            let p = [centre[0] + dx, centre[1] + dy];
            if grid.pick_hex([p[0] as i32, p[1] as i32]) == Some(hex) {
                out.push([dx, dy]);
            }
            dx += SHARE_STEP;
        }
        dy += SHARE_STEP;
    }
    if out.is_empty() {
        out.push([0, 0]);
    }
    out
}

/// Hexes covered by triangles under a [`NavmeshCoverage`] rule.
struct Coverage {
    raster: Raster,
    mode: NavmeshCoverage,
    /// Tiles.
    tolerance: f64,
    covered: Vec<bool>,
    /// [`NavmeshCoverage::Share`]: per hex, the samples covered so far.
    masks: Vec<u64>,
    offsets: Vec<[f64; 2]>,
}

impl Coverage {
    fn new(frame: &Frame, options: &BuildOptions) -> Self {
        let raster = Raster::new(frame);
        let mode = options.navmesh_coverage;
        let tolerance = match mode {
            NavmeshCoverage::Tolerance => {
                f64::from(options.navmesh_tolerance.max(0.0)) / frame.tile_units()
            }
            NavmeshCoverage::Centre | NavmeshCoverage::Share(_) => 0.0,
        };
        let (masks, offsets) = match mode {
            NavmeshCoverage::Share(_) => {
                let offsets: Vec<[f64; 2]> = share_offsets()
                    .into_iter()
                    .take(64)
                    .map(|o| frame.tile_of_screen([o[0] as f64, o[1] as f64]))
                    .collect();
                (vec![0u64; HEXES], offsets)
            }
            _ => (Vec::new(), Vec::new()),
        };
        Self {
            raster,
            mode,
            tolerance,
            covered: vec![false; HEXES],
            masks,
            offsets,
        }
    }

    fn add(&mut self, tri: [[f64; 2]; 3]) {
        match self.mode {
            NavmeshCoverage::Share(_) => {
                self.raster
                    .fill_samples(tri, &self.offsets, &mut self.masks)
            }
            _ => self.raster.fill(tri, self.tolerance, &mut self.covered),
        }
    }

    /// The raster and the covered hexes.
    fn finish(mut self) -> (Raster, Vec<bool>) {
        if let NavmeshCoverage::Share(share) = self.mode {
            let n = self.offsets.len() as f64;
            let need = (f64::from(share.clamp(0.0, 1.0)) * n).ceil().max(1.0) as u32;
            for (hex, mask) in self.masks.iter().enumerate() {
                self.covered[hex] = mask.count_ones() >= need;
            }
        }
        (self.raster, self.covered)
    }
}

/// [`walkability`] with the reason for every hex. With
/// [`BuildOptions::interior`] set this is [`interior_verdicts`], and
/// `sampler` is not read. With [`BuildOptions::level`] only that level's
/// triangles are rasterized, at any height above the terrain (no "too
/// high" filter; [`WalkStats::triangles_off_level`] counts the others), a
/// hex they cover is walkable unless blocked (the terrain slope under a
/// deck plays no part), and a hex in a cell without a navmesh is judged by
/// slope on level 0 and is [`Verdict::NoGround`] above it.
pub fn verdicts(
    frame: &Frame,
    sampler: &GroundSampler,
    cells: &[CellDoc],
    blocked: &[bool],
    options: &BuildOptions,
) -> (Vec<Verdict>, WalkStats) {
    if let Some(interior) = &options.interior {
        return interior_verdicts(frame, cells, blocked, options, interior);
    }
    let mut stats = WalkStats::default();
    let mut coverage = Coverage::new(frame, options);
    let max_height = f64::from(options.navmesh_max_height);
    let mut nav_cells = BTreeSet::new();
    for cell in cells.iter().filter(|cell| !cell.is_interior()) {
        let mir::CellLocation::Exterior { x, y, .. } = &cell.location else {
            continue;
        };
        for navmesh in &cell.navmeshes {
            if !navmesh.triangles.is_empty() {
                nav_cells.insert((*x, *y));
            }
            let form = options.level.as_ref().map(|_| navmesh.form.to_string());
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
                if let (Some(level), Some(form)) = (&options.level, &form) {
                    // Exterior levels: the level's triangles, at any height.
                    if !level.triangles.contains(&(form.clone(), index as u32)) {
                        stats.triangles_off_level += 1;
                        continue;
                    }
                    stats.triangles_rasterized += 1;
                    let tri =
                        corners.map(|v| frame.tile_of_source(f64::from(v[0]), f64::from(v[1])));
                    coverage.add(tri);
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
                coverage.add(tri);
            }
        }
    }
    let (raster, covered) = coverage.finish();
    let centres = &raster.centres;
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
        if let Some(level) = &options.level {
            // Exterior levels: the navmesh is trusted at any height; off it
            // the ground level keeps the slope rule, an upper level has no
            // ground.
            let verdict = if covered[hex] {
                None
            } else if has_navmesh {
                stats.rejected_navmesh += 1;
                Some(Verdict::OffNavmesh)
            } else if level.level > 0 {
                stats.rejected_no_ground += 1;
                Some(Verdict::NoGround)
            } else {
                stats.slope_only += 1;
                match sampler.slope(x as f32, y as f32) {
                    Some(slope) if slope <= limit => None,
                    Some(_) => {
                        stats.rejected_slope += 1;
                        Some(Verdict::TooSteep)
                    }
                    None => {
                        stats.rejected_no_ground += 1;
                        Some(Verdict::NoGround)
                    }
                }
            };
            if let Some(verdict) = verdict {
                verdicts[hex] = verdict;
                continue;
            }
            if blocked.get(hex).copied().unwrap_or(false) {
                stats.rejected_blocked += 1;
                verdicts[hex] = Verdict::Blocked;
                continue;
            }
            verdicts[hex] = Verdict::Walkable;
            stats.walkable += 1;
            continue;
        }
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

/// What an interior cell's navmesh triangle is to the floor level.
enum Level {
    /// A vertex index out of range, or a coordinate that is not finite.
    Unreadable,
    /// Some corner outside the band.
    Off,
    /// Every corner in the band.
    On([[f32; 3]; 3]),
}

/// A triangle's area seen from above, source units squared.
fn plan_area(c: &[[f32; 3]; 3]) -> f64 {
    let p = c.map(|v| [f64::from(v[0]), f64::from(v[1])]);
    0.5 * ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]))
        .abs()
}

/// Every navmesh triangle of the interior cells among `cells`, judged
/// against the band.
fn interior_triangles<'a>(
    cells: &'a [CellDoc],
    interior: &'a InteriorBuild,
) -> impl Iterator<Item = Level> + 'a {
    cells
        .iter()
        .filter(|cell| cell.is_interior())
        .flat_map(|cell| cell.navmeshes.iter())
        .flat_map(move |navmesh| {
            (0..navmesh.triangles.len()).map(move |index| match navmesh.corners(index) {
                None => Level::Unreadable,
                Some(corners) if corners.iter().flatten().any(|v| !v.is_finite()) => {
                    Level::Unreadable
                }
                Some(corners) if corners.iter().all(|v| interior.in_band(v[2])) => {
                    Level::On(corners)
                }
                Some(_) => Level::Off,
            })
        })
}

/// Walkability of an interior demo (crate notes, "Interior demo"): a hex
/// is [`Verdict::Walkable`] when a navmesh triangle of an interior cell
/// whose every corner lies in [`InteriorBuild::band`] covers it (by
/// [`BuildOptions::navmesh_coverage`]: by default its centre, within
/// [`BuildOptions::navmesh_tolerance`]) and `blocked` does not hold
/// it; [`Verdict::OffNavmesh`] when no such triangle covers it (exterior
/// cells, slope and terrain play no part); [`Verdict::Blocked`] when one
/// does but it is blocked. [`WalkStats::triangles_off_level`] counts the
/// triangles outside the band; `slope_only`, `rejected_slope`,
/// `rejected_no_ground` and `triangles_too_high` stay 0.
pub fn interior_verdicts(
    frame: &Frame,
    cells: &[CellDoc],
    blocked: &[bool],
    options: &BuildOptions,
    interior: &InteriorBuild,
) -> (Vec<Verdict>, WalkStats) {
    let mut stats = WalkStats::default();
    let mut coverage = Coverage::new(frame, options);
    for level in interior_triangles(cells, interior) {
        stats.triangles += 1;
        match level {
            Level::Unreadable => stats.triangles_unreadable += 1,
            Level::Off => stats.triangles_off_level += 1,
            Level::On(corners) => {
                stats.triangles_rasterized += 1;
                let tri = corners.map(|v| frame.tile_of_source(f64::from(v[0]), f64::from(v[1])));
                coverage.add(tri);
            }
        }
    }
    let (raster, covered) = coverage.finish();
    let mut verdicts = vec![Verdict::OffGrid; HEXES];
    for hex in 0..HEXES {
        if !raster.centres[hex][0].is_finite() {
            continue;
        }
        if !covered[hex] {
            stats.rejected_navmesh += 1;
            verdicts[hex] = Verdict::OffNavmesh;
            continue;
        }
        stats.covered += 1;
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

/// The floor level of an interior: the height around which the interior
/// cells' navmesh has the most ground area. Triangles are binned by their
/// mean corner height in steps of 16 units, weighted by their area seen
/// from above; the heaviest run of bins within [`INTERIOR_BAND_BELOW`] +
/// [`INTERIOR_BAND_ABOVE`] wins (ties to the lower), and the result is the
/// area-weighted mean height of its triangles. `None` without a readable
/// triangle of positive area.
pub fn dominant_floor_z(cells: &[CellDoc]) -> Option<f32> {
    const BIN: f64 = 16.0;
    let mut triangles: Vec<(f64, f64)> = Vec::new();
    let everything = InteriorBuild {
        below: f32::INFINITY,
        above: f32::INFINITY,
        ..InteriorBuild::default()
    };
    for level in interior_triangles(cells, &everything) {
        if let Level::On(c) = level {
            let area = plan_area(&c);
            let z = c.iter().map(|v| f64::from(v[2])).sum::<f64>() / 3.0;
            if area > 0.0 && area.is_finite() && z.is_finite() {
                triangles.push((z, area));
            }
        }
    }
    if triangles.is_empty() {
        return None;
    }
    let mut bins: std::collections::BTreeMap<i64, f64> = std::collections::BTreeMap::new();
    for (z, area) in &triangles {
        *bins.entry((z / BIN).floor() as i64).or_default() += area;
    }
    let span = (f64::from(INTERIOR_BAND_BELOW + INTERIOR_BAND_ABOVE) / BIN).floor() as i64;
    let mut best: Option<(f64, i64)> = None;
    for &start in bins.keys() {
        let weight: f64 = bins.range(start..=start + span).map(|(_, a)| a).sum();
        if best.is_none_or(|(w, _)| weight > w) {
            best = Some((weight, start));
        }
    }
    let (_, start) = best?;
    let low = start as f64 * BIN;
    let high = (start + span + 1) as f64 * BIN;
    let (sum, weight) = triangles
        .iter()
        .filter(|(z, _)| *z >= low && *z < high)
        .fold((0.0, 0.0), |(s, w), (z, a)| (s + z * a, w + a));
    (weight > 0.0).then(|| (sum / weight) as f32)
}

/// The area-weighted centre `(x, y)` of the interior cells' navmesh
/// triangles in `interior`'s band, seen from above: the default entrance
/// target, and a good window centre. `None` when the band holds no
/// triangle of positive area.
pub fn navmesh_centre(cells: &[CellDoc], interior: &InteriorBuild) -> Option<[f32; 2]> {
    let mut sum = [0.0f64; 2];
    let mut weight = 0.0f64;
    for level in interior_triangles(cells, interior) {
        if let Level::On(c) = level {
            let p = c.map(|v| [f64::from(v[0]), f64::from(v[1])]);
            let area = plan_area(&c);
            if !area.is_finite() || area <= 0.0 {
                continue;
            }
            for axis in 0..2 {
                sum[axis] += area * (p[0][axis] + p[1][axis] + p[2][axis]) / 3.0;
            }
            weight += area;
        }
    }
    (weight > 0.0).then(|| [(sum[0] / weight) as f32, (sum[1] / weight) as f32])
}

/// The hexes the source game's own navigation data lets an actor stand
/// on: in a cell with a navmesh, under a triangle of it, and no steeper
/// than the slope limit. Sorted. A slicer that blocks a building by its
/// bounding footprint should leave these open (a porch, a canopy).
///
/// With [`BuildOptions::interior`]: the hexes the floor level's navmesh
/// covers ([`interior_verdicts`] with nothing blocked); `sampler` is not
/// read.
pub fn standable_hexes(
    frame: &Frame,
    sampler: &GroundSampler,
    cells: &[CellDoc],
    options: &BuildOptions,
) -> Vec<i32> {
    let (cover, _) = verdicts(frame, sampler, cells, &[], options);
    if options.interior.is_some() {
        return (0..HEXES as i32)
            .filter(|&hex| cover[hex as usize] == Verdict::Walkable)
            .collect();
    }
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

    /// An interior cell with two floor rectangles at z 0, a gap of `gap`
    /// units between them along x (a counter the navmesh stops at).
    fn gapped_floor(gap: f32) -> CellDoc {
        let rect = |x0: f32, x1: f32| {
            [
                [[x0, -400.0, 0.0], [x1, -400.0, 0.0], [x1, 400.0, 0.0]],
                [[x0, -400.0, 0.0], [x1, 400.0, 0.0], [x0, 400.0, 0.0]],
            ]
        };
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for t in rect(-600.0, -gap / 2.0)
            .into_iter()
            .chain(rect(gap / 2.0, 600.0))
        {
            let base = vertices.len() as u32;
            vertices.extend_from_slice(&t);
            triangles.push(mir::NavTriangleDoc {
                vertices: [base, base + 1, base + 2],
                links: [-1; 3],
                flags: 0,
            });
        }
        CellDoc {
            schema_version: mir::SCHEMA_VERSION,
            form: mir::FormKey::new("synthetic.esm", 0x9100),
            editor_id: None,
            name: None,
            location: mir::CellLocation::Interior,
            flags: 1,
            water_height: None,
            land: None,
            references: Vec::new(),
            navmeshes: vec![mir::NavmeshDoc {
                form: mir::FormKey::new("synthetic.esm", 0x9101),
                vertices,
                triangles,
                cover_triangles: Vec::new(),
                door_links: Vec::new(),
                external_links: Vec::new(),
            }],
        }
    }

    #[test]
    fn the_stricter_coverage_rules_drop_hexes_at_the_navmesh_edge() {
        let frame = frame();
        let cells = [gapped_floor(140.0)];
        let covered = |coverage: NavmeshCoverage| -> BTreeSet<i32> {
            let options = BuildOptions {
                interior: Some(InteriorBuild::new(0.0)),
                navmesh_coverage: coverage,
                ..BuildOptions::default()
            };
            standable_hexes(&frame, &GroundSampler::default(), &cells, &options)
                .into_iter()
                .collect()
        };
        let tolerance = covered(NavmeshCoverage::Tolerance);
        let centre = covered(NavmeshCoverage::Centre);
        let quarter = covered(NavmeshCoverage::Share(0.25));
        let half = covered(NavmeshCoverage::Share(0.5));
        let most = covered(NavmeshCoverage::Share(0.75));
        assert!(centre.is_subset(&tolerance) && centre.len() < tolerance.len());
        assert!(most.is_subset(&half) && half.is_subset(&quarter));
        assert!(most.len() < half.len() && half.len() < quarter.len());
        // Deep inside the floor every rule covers the hex; in the middle of
        // the gap none does (70 units from either edge).
        let inside = frame.hex_of_source(-350.0, 0.0).unwrap();
        let gap = frame.hex_of_source(0.0, 0.0).unwrap();
        for set in [&tolerance, &centre, &quarter, &half, &most] {
            assert!(set.contains(&inside));
            assert!(!set.contains(&gap));
        }
        // The default is the tolerance rule: verdicts are unchanged.
        let default = BuildOptions {
            interior: Some(InteriorBuild::new(0.0)),
            ..BuildOptions::default()
        };
        assert_eq!(default.navmesh_coverage, NavmeshCoverage::Tolerance);
        let samples = share_offsets();
        assert!(
            samples.len() > 8 && samples.len() <= 64,
            "{}",
            samples.len()
        );
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
