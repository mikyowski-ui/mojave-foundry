//! Ground colour at a source point: texture layers, vertex colour, relief.

use std::collections::BTreeMap;

use foundry_render::{Calibration, Projection};
use mir::{CELL_UNITS, GroundSampler, Image, LAND_GRID, LAND_SPACING, LandDoc, LandLayer, Mir};

use crate::texture::Prefiltered;
use crate::{TerrainError, TerrainOptions};

/// Vertices along one side of a texture quadrant.
const QUAD_GRID: usize = 17;
/// Grid squares along one side of a quadrant.
const QUAD_SQUARES: f64 = 16.0;
/// Weights at or below this are dropped from a pixel's blend.
const WEIGHT_EPSILON: f32 = 1.0 / 1024.0;
/// A missing texture's weight above this marks the pixel as affected.
const COUNT_EPSILON: f32 = 1.0 / 255.0;
/// Most cells loaded along one axis; a calibration needing more is refused.
const MAX_CELLS_PER_AXIS: i64 = 64;

/// Where the terrain comes from: a MIR directory, or memory in tests.
pub trait LandSource {
    /// The land of exterior cell `(x, y)`; `None` when the cell was not
    /// extracted or has no land.
    fn land(&self, x: i32, y: i32) -> Result<Option<LandDoc>, TerrainError>;
    /// A texture by content hash; `None` when missing or undecodable.
    fn texture(&self, hash: &str) -> Option<Image>;
}

/// The land of one worldspace in a MIR directory.
pub struct MirLand<'a> {
    pub mir: &'a Mir,
    pub worldspace: &'a str,
}

impl LandSource for MirLand<'_> {
    fn land(&self, x: i32, y: i32) -> Result<Option<LandDoc>, TerrainError> {
        Ok(self
            .mir
            .cell(self.worldspace, x, y)?
            .and_then(|doc| doc.land))
    }

    fn texture(&self, hash: &str) -> Option<Image> {
        self.mir.texture(hash).ok()
    }
}

/// Land and textures held in memory.
#[derive(Debug, Clone, Default)]
pub struct MemoryLand {
    pub cells: BTreeMap<(i32, i32), LandDoc>,
    pub textures: BTreeMap<String, Image>,
}

impl LandSource for MemoryLand {
    fn land(&self, x: i32, y: i32) -> Result<Option<LandDoc>, TerrainError> {
        Ok(self.cells.get(&(x, y)).cloned())
    }

    fn texture(&self, hash: &str) -> Option<Image> {
        self.textures.get(hash).cloned()
    }
}

/// Map screen space to source space on the ground plane (`e = 0`).
#[derive(Debug, Clone)]
pub(crate) struct Geometry {
    /// Source point of screen `(0, 0)`.
    origin: [f64; 2],
    /// `d(x, y) / d(screen_x, screen_y)`, row-major.
    jacobian: [[f64; 2]; 2],
    /// Source → tile `(s, t)`, linear part, row-major.
    tile_linear: [[f64; 2]; 2],
    /// Source normal → map axes `(s, t, e)` in common units, row-major.
    normal_linear: [[f64; 3]; 3],
}

fn invert2(m: [[f64; 2]; 2]) -> Option<[[f64; 2]; 2]> {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    Some([
        [m[1][1] / det, -m[0][1] / det],
        [-m[1][0] / det, m[0][0] / det],
    ])
}

fn mul2(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
    [
        [
            a[0][0] * b[0][0] + a[0][1] * b[1][0],
            a[0][0] * b[0][1] + a[0][1] * b[1][1],
        ],
        [
            a[1][0] * b[0][0] + a[1][1] * b[1][0],
            a[1][0] * b[0][1] + a[1][1] * b[1][1],
        ],
    ]
}

impl Geometry {
    /// Reads the maps from the projection: its screen vectors for tile →
    /// screen, its `tile_from_source` (probed at a zero anchor) for source
    /// → tile, so yaw and units come from one place.
    pub fn new(projection: &Projection, anchor: [f32; 3]) -> Result<Self, TerrainError> {
        let [vs, vt, _] = projection.screen_vectors();
        let screen_from_tile = [
            [f64::from(vs[0]), f64::from(vt[0])],
            [f64::from(vs[1]), f64::from(vt[1])],
        ];
        const PROBE: f32 = 1000.0;
        let probe = |p: [f32; 3]| {
            projection
                .tile_from_source([0.0; 3], p)
                .map(|v| f64::from(v) / f64::from(PROBE))
        };
        let dx = probe([PROBE, 0.0, 0.0]);
        let dy = probe([0.0, PROBE, 0.0]);
        let dz = probe([0.0, 0.0, PROBE]);
        let tile_linear = [[dx[0], dy[0]], [dx[1], dy[1]]];
        let tile_from_screen = invert2(screen_from_tile).ok_or(TerrainError::Projection)?;
        let source_from_tile = invert2(tile_linear).ok_or(TerrainError::Projection)?;
        let jacobian = mul2(source_from_tile, tile_from_screen);
        let units = [
            f64::from(projection.tile_units()),
            f64::from(projection.tile_units()),
            f64::from(projection.storey_units()),
        ];
        let normal_linear =
            [0, 1, 2].map(|r| [dx[r] * units[r], dy[r] * units[r], dz[r] * units[r]]);
        let geometry = Geometry {
            origin: [f64::from(anchor[0]), f64::from(anchor[1])],
            jacobian,
            tile_linear,
            normal_linear,
        };
        if !geometry
            .jacobian
            .iter()
            .flatten()
            .chain(geometry.normal_linear.iter().flatten())
            .all(|v| v.is_finite())
        {
            return Err(TerrainError::Projection);
        }
        Ok(geometry)
    }

    /// Source `(x, y)` of a map screen point.
    pub fn source(&self, screen: [f64; 2]) -> [f64; 2] {
        let j = &self.jacobian;
        [
            self.origin[0] + j[0][0] * screen[0] + j[0][1] * screen[1],
            self.origin[1] + j[1][0] * screen[0] + j[1][1] * screen[1],
        ]
    }

    /// Source offset of a screen offset.
    pub fn source_delta(&self, delta: [f64; 2]) -> [f64; 2] {
        let j = &self.jacobian;
        [
            j[0][0] * delta[0] + j[0][1] * delta[1],
            j[1][0] * delta[0] + j[1][1] * delta[1],
        ]
    }

    /// Source units² covered by one screen pixel.
    pub fn pixel_area(&self) -> f64 {
        let j = &self.jacobian;
        (j[0][0] * j[1][1] - j[0][1] * j[1][0]).abs()
    }

    /// Source point of tile-space ground coordinates.
    pub fn source_of_tile(&self, st: [f64; 2]) -> Option<[f64; 2]> {
        let inv = invert2(self.tile_linear)?;
        Some([
            self.origin[0] + inv[0][0] * st[0] + inv[0][1] * st[1],
            self.origin[1] + inv[1][0] * st[0] + inv[1][1] * st[1],
        ])
    }

    /// A source-space normal turned into map axes, unit length.
    fn map_normal(&self, n: [f32; 3]) -> [f32; 3] {
        let m = &self.normal_linear;
        let n = n.map(f64::from);
        let out = [0, 1, 2].map(|r| m[r][0] * n[0] + m[r][1] * n[1] + m[r][2] * n[2]);
        let length = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2]).sqrt();
        if length > 1e-12 {
            out.map(|v| (v / length) as f32)
        } else {
            [0.0, 0.0, 1.0]
        }
    }
}

/// The relief term: Lambert key light plus ambient, relative to flat
/// ground, scaled by `relief`.
#[derive(Debug, Clone)]
pub(crate) struct Shading {
    light: [f32; 3],
    intensity: f32,
    ambient: f32,
    flat: f32,
    relief: f32,
}

impl Shading {
    pub fn new(calibration: &Calibration, geometry: &Geometry, relief: f32) -> Self {
        let mut shading = Shading {
            light: calibration.light(),
            intensity: calibration.key_light_intensity,
            ambient: calibration.ambient,
            flat: 1.0,
            relief,
        };
        shading.flat = shading.lit(geometry.map_normal([0.0, 0.0, 1.0]));
        shading
    }

    fn lit(&self, n: [f32; 3]) -> f32 {
        let lambert = n[0] * self.light[0] + n[1] * self.light[1] + n[2] * self.light[2];
        self.ambient + self.intensity * lambert.max(0.0)
    }

    /// The multiplier for a map-axes unit normal: 1 on flat ground.
    pub fn factor(&self, n: [f32; 3]) -> f32 {
        if self.flat <= 1e-6 || self.relief == 0.0 {
            return 1.0;
        }
        (1.0 + self.relief * (self.lit(n) / self.flat - 1.0)).max(0.0)
    }
}

/// A texture reference: a slot in [`Terrain::textures`], or the missing
/// tone.
type Slot = Option<u32>;

/// Where a layer's texture comes from, for the stats and for what a layer
/// with nothing to draw does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Origin {
    /// The layer's own land texture.
    #[default]
    Own,
    /// The engine's default land texture, standing in for a layer that
    /// names no usable one ([`LandLayer::defaulted`]).
    Defaulted,
    /// The layer's texture is missing: a file that is not in the MIR, or
    /// no texture at all (a quadrant without a base layer in a document
    /// written before the default land texture was extracted).
    Missing,
}

impl Origin {
    fn of(layer: &LandLayer, slot: Slot) -> Self {
        match (slot, layer.is_defaulted()) {
            (None, _) => Origin::Missing,
            (Some(_), true) => Origin::Defaulted,
            (Some(_), false) => Origin::Own,
        }
    }
}

#[derive(Debug, Clone)]
struct Layer {
    texture: Slot,
    origin: Origin,
    /// 17 × 17, row 0 south, column 0 west.
    opacity: Vec<f32>,
}

impl Layer {
    fn opacity_at(&self, u: f64, v: f64) -> f32 {
        bilinear(&self.opacity, QUAD_GRID, u, v)
    }
}

#[derive(Debug, Clone, Default)]
struct Quadrant {
    /// The base layer's texture. When the base is missing (no base layer,
    /// or its texture missing) this is the nearest quadrant's base texture
    /// ([`Terrain::borrow_bases`]), or `None`, the missing tone.
    base: Slot,
    /// [`Origin::Missing`] for a missing base, even when one is borrowed.
    base_origin: Origin,
    /// Blended layers, in ascending layer order.
    layers: Vec<Layer>,
}

#[derive(Debug, Clone)]
struct Cell {
    quadrants: [Quadrant; 4],
    /// 33 × 33 vertex colours as multipliers 0 to 1.
    colors: Option<Vec<[f32; 3]>>,
}

/// Bilinear interpolation on a `side × side` row-major grid at continuous
/// vertex coordinates `(u, v)` = (column, row), clamped to the grid.
fn bilinear(grid: &[f32], side: usize, u: f64, v: f64) -> f32 {
    let last = (side - 2) as f64;
    let u = u.clamp(0.0, last + 1.0);
    let v = v.clamp(0.0, last + 1.0);
    let c = u.floor().min(last);
    let r = v.floor().min(last);
    let fu = (u - c) as f32;
    let fv = (v - r) as f32;
    let (c, r) = (c as usize, r as usize);
    let at = |r: usize, c: usize| grid[r * side + c];
    let south = at(r, c) + (at(r, c + 1) - at(r, c)) * fu;
    let north = at(r + 1, c) + (at(r + 1, c + 1) - at(r + 1, c)) * fu;
    south + (north - south) * fv
}

/// Per-worker scratch space.
#[derive(Default)]
pub(crate) struct Scratch {
    weights: Vec<(Slot, Origin, f32)>,
}

/// One evaluated ground pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sample {
    /// 0 to 255, not clamped.
    pub rgb: [f32; 3],
    /// No land here: the fallback colour.
    pub fallback: bool,
    /// A missing texture contributed, or would have: a missing blended
    /// layer that was left out still counts.
    pub missing: bool,
    /// The engine's default land texture contributed.
    pub defaulted: bool,
}

/// The prepared terrain of every cell the map can see.
pub(crate) struct Terrain {
    x0: i32,
    y0: i32,
    nx: usize,
    ny: usize,
    cells: Vec<Option<Cell>>,
    textures: Vec<Prefiltered>,
    missing_rgb: [f32; 3],
    /// Whether a missing layer is drawn from its neighbours (a blended
    /// layer left out, a base borrowed) rather than in the missing tone:
    /// true unless the caller set the tone.
    substitute: bool,
    fallback_rgb: [f32; 3],
    /// Sub-sample offsets in source units, relative to the pixel centre.
    offsets: Vec<[f64; 2]>,
    pub cells_with_land: usize,
    pub cells_without_land: usize,
    pub missing_layers: usize,
    /// Of `missing_layers`, those that name no texture file at all.
    pub unnamed_layers: usize,
    /// Missing bases drawn with a neighbouring quadrant's base texture.
    pub borrowed_bases: usize,
    pub defaulted_layers: usize,
    pub textures_loaded: usize,
    pub textures_missing: usize,
}

/// The quadrant index (0 SW, 1 SE, 2 NW, 3 NE) and the position inside it,
/// for cell-grid coordinates.
fn quadrant_of(u: f64, v: f64) -> (usize, f64, f64) {
    let east = u >= QUAD_SQUARES;
    let north = v >= QUAD_SQUARES;
    let lu = if east { u - QUAD_SQUARES } else { u };
    let lv = if north { v - QUAD_SQUARES } else { v };
    (usize::from(north) * 2 + usize::from(east), lu, lv)
}

impl Terrain {
    /// Loads every cell under the map's squares (tile space `-2..102` on
    /// both axes) and the textures their layers name, prefiltered for a
    /// texture repeat of `repeat` source units.
    pub fn load(
        source: &dyn LandSource,
        geometry: &Geometry,
        options: &TerrainOptions,
        repeat: f64,
    ) -> Result<Self, TerrainError> {
        let corners = [[-2.0, -2.0], [102.0, -2.0], [-2.0, 102.0], [102.0, 102.0]];
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for st in corners {
            let p = geometry
                .source_of_tile(st)
                .ok_or(TerrainError::Projection)?;
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let cell = |v: f64| (v / f64::from(CELL_UNITS)).floor();
        let (cx0, cx1) = (cell(lo[0]), cell(hi[0]));
        let (cy0, cy1) = (cell(lo[1]), cell(hi[1]));
        let span = |a: f64, b: f64| b - a + 1.0;
        if !(span(cx0, cx1) <= MAX_CELLS_PER_AXIS as f64
            && span(cy0, cy1) <= MAX_CELLS_PER_AXIS as f64)
            || cx0 < f64::from(i32::MIN)
            || cy0 < f64::from(i32::MIN)
            || cx1 > f64::from(i32::MAX)
            || cy1 > f64::from(i32::MAX)
        {
            return Err(TerrainError::Projection);
        }
        let (x0, y0) = (cx0 as i32, cy0 as i32);
        let nx = (cx1 - cx0) as usize + 1;
        let ny = (cy1 - cy0) as usize + 1;

        let mut docs = Vec::with_capacity(nx * ny);
        for j in 0..ny {
            for i in 0..nx {
                docs.push(source.land(x0 + i as i32, y0 + j as i32)?);
            }
        }
        // Every texture any loaded layer names, loaded once in hash order.
        let mut hashes: BTreeMap<String, Slot> = BTreeMap::new();
        for doc in docs.iter().flatten() {
            for layer in &doc.layers {
                if let Some(hash) = &layer.texture_hash {
                    hashes.insert(hash.clone(), None);
                }
            }
        }
        let footprint = geometry.pixel_area().sqrt() / f64::from(options.supersample);
        let mut textures = Vec::new();
        let mut textures_missing = 0;
        for (hash, slot) in hashes.iter_mut() {
            match source.texture(hash).and_then(|image| {
                Prefiltered::new(
                    &image,
                    repeat,
                    footprint,
                    f64::from(options.texture_lod_bias),
                    options.texture_sharpen,
                )
            }) {
                Some(texture) => {
                    *slot = Some(textures.len() as u32);
                    textures.push(texture);
                }
                None => textures_missing += 1,
            }
        }
        let missing_rgb = match options.missing_texture_rgb {
            Some(rgb) => rgb.map(f32::from),
            None if !textures.is_empty() => {
                let mut sum = [0.0f64; 3];
                for t in &textures {
                    for k in 0..3 {
                        sum[k] += f64::from(t.mean[k]);
                    }
                }
                sum.map(|v| (v / textures.len() as f64) as f32)
            }
            None => [128.0; 3],
        };

        let mut terrain = Terrain {
            x0,
            y0,
            nx,
            ny,
            cells: Vec::with_capacity(nx * ny),
            textures,
            missing_rgb,
            substitute: options.missing_texture_rgb.is_none(),
            fallback_rgb: options.fallback_rgb.map(f32::from),
            offsets: Vec::new(),
            cells_with_land: 0,
            cells_without_land: 0,
            missing_layers: 0,
            unnamed_layers: 0,
            borrowed_bases: 0,
            defaulted_layers: 0,
            textures_loaded: 0,
            textures_missing,
        };
        terrain.textures_loaded = terrain.textures.len();
        for doc in &docs {
            let prepared = doc.as_ref().and_then(|land| terrain.prepare(land, &hashes));
            if prepared.is_some() {
                terrain.cells_with_land += 1;
            } else {
                terrain.cells_without_land += 1;
            }
            terrain.cells.push(prepared);
        }
        if terrain.substitute {
            terrain.borrow_bases();
        }
        let n = options.supersample;
        for j in 0..n {
            for i in 0..n {
                let d = [
                    (f64::from(i) + 0.5) / f64::from(n) - 0.5,
                    (f64::from(j) + 0.5) / f64::from(n) - 0.5,
                ];
                terrain.offsets.push(geometry.source_delta(d));
            }
        }
        Ok(terrain)
    }

    /// A cell's layers and colours; `None` for a grid of the wrong size.
    fn prepare(&mut self, land: &LandDoc, hashes: &BTreeMap<String, Slot>) -> Option<Cell> {
        if land.heights.len() != LAND_GRID * LAND_GRID {
            return None;
        }
        let slot_of = |layer: &LandLayer| {
            layer
                .texture_hash
                .as_ref()
                .and_then(|h| hashes.get(h).copied().flatten())
        };
        let mut quadrants: [Quadrant; 4] = Default::default();
        let mut missing = 0;
        let mut unnamed = 0;
        let mut defaulted = 0;
        let mut count = |origin: Origin, layer: Option<&LandLayer>| match origin {
            Origin::Own => {}
            Origin::Defaulted => defaulted += 1,
            Origin::Missing => {
                missing += 1;
                if layer.is_none_or(|l| l.texture.is_none() && l.texture_hash.is_none()) {
                    unnamed += 1;
                }
            }
        };
        for (index, quadrant) in quadrants.iter_mut().enumerate() {
            let mut layers: Vec<&LandLayer> = land
                .layers
                .iter()
                .filter(|l| usize::from(l.quadrant) == index)
                .collect();
            // Stable: equal layer numbers keep their document order.
            layers.sort_by_key(|l| l.layer);
            let base = layers.iter().find(|l| l.layer < 0);
            match base {
                Some(base) => {
                    quadrant.base = slot_of(base);
                    quadrant.base_origin = Origin::of(base, quadrant.base);
                    count(quadrant.base_origin, Some(base));
                }
                // No base layer: the game's default land texture, which a
                // document written before it was extracted lacks.
                None => {
                    quadrant.base_origin = Origin::Missing;
                    count(Origin::Missing, None);
                }
            }
            for layer in layers.iter().filter(|l| l.layer >= 0) {
                let mut opacity = vec![0.0f32; QUAD_GRID * QUAD_GRID];
                let mut any = false;
                for point in &layer.opacity {
                    let position = usize::from(point.position);
                    if position < opacity.len() && point.opacity.is_finite() {
                        opacity[position] = point.opacity.clamp(0.0, 1.0);
                        any |= opacity[position] > 0.0;
                    }
                }
                if !any {
                    continue;
                }
                let texture = slot_of(layer);
                let origin = Origin::of(layer, texture);
                count(origin, Some(layer));
                quadrant.layers.push(Layer {
                    texture,
                    origin,
                    opacity,
                });
            }
        }
        self.missing_layers += missing;
        self.unnamed_layers += unnamed;
        self.defaulted_layers += defaulted;
        let colors = land
            .colors
            .as_ref()
            .filter(|c| c.len() == LAND_GRID * LAND_GRID)
            .map(|c| {
                c.iter()
                    .map(|rgb| rgb.map(|v| f32::from(v) / 255.0))
                    .collect()
            });
        Some(Cell { quadrants, colors })
    }

    /// Gives every missing base the base texture of the nearest quadrant
    /// whose own base is drawn (its own texture or the default), by steps
    /// between edge-sharing quadrants across the loaded cells, so a hole in
    /// the ground takes the look of the ground around it. Ties go to the
    /// quadrant reached first from the sources in cell and quadrant order,
    /// east, west, north, south. A base with no such quadrant anywhere
    /// keeps the missing tone.
    fn borrow_bases(&mut self) {
        let (w, h) = (self.nx * 2, self.ny * 2);
        let at = |gx: usize, gy: usize| ((gy / 2) * self.nx + gx / 2, (gy % 2) * 2 + gx % 2);
        let mut nearest: Vec<Slot> = vec![None; w * h];
        let mut queue = std::collections::VecDeque::new();
        let mut any_missing = false;
        for gy in 0..h {
            for gx in 0..w {
                let (cell, q) = at(gx, gy);
                let Some(cell) = &self.cells[cell] else {
                    continue;
                };
                let quadrant = &cell.quadrants[q];
                if quadrant.base_origin == Origin::Missing {
                    any_missing = true;
                } else if quadrant.base.is_some() {
                    nearest[gy * w + gx] = quadrant.base;
                    queue.push_back((gx, gy));
                }
            }
        }
        if !any_missing {
            return;
        }
        while let Some((gx, gy)) = queue.pop_front() {
            let slot = nearest[gy * w + gx];
            let steps = [
                (gx + 1 < w).then(|| (gx + 1, gy)),
                gx.checked_sub(1).map(|x| (x, gy)),
                (gy + 1 < h).then(|| (gx, gy + 1)),
                gy.checked_sub(1).map(|y| (gx, y)),
            ];
            for (nx, ny) in steps.into_iter().flatten() {
                if nearest[ny * w + nx].is_none() {
                    nearest[ny * w + nx] = slot;
                    queue.push_back((nx, ny));
                }
            }
        }
        for gy in 0..h {
            for gx in 0..w {
                let (cell, q) = at(gx, gy);
                if let Some(cell) = &mut self.cells[cell] {
                    let quadrant = &mut cell.quadrants[q];
                    if quadrant.base_origin == Origin::Missing && quadrant.base.is_none() {
                        quadrant.base = nearest[gy * w + gx];
                        if quadrant.base.is_some() {
                            self.borrowed_bases += 1;
                        }
                    }
                }
            }
        }
    }

    /// The cell under a source point and the point's grid coordinates in
    /// it (0 to 32 along each axis).
    fn locate(&self, x: f64, y: f64) -> Option<(&Cell, f64, f64)> {
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        let units = f64::from(CELL_UNITS);
        let cx = (x / units).floor();
        let cy = (y / units).floor();
        let i = cx - f64::from(self.x0);
        let j = cy - f64::from(self.y0);
        if i < 0.0 || j < 0.0 || i >= self.nx as f64 || j >= self.ny as f64 {
            return None;
        }
        let cell = self.cells[j as usize * self.nx + i as usize].as_ref()?;
        let spacing = f64::from(LAND_SPACING);
        let last = (LAND_GRID - 1) as f64;
        let u = ((x - cx * units) / spacing).clamp(0.0, last);
        let v = ((y - cy * units) / spacing).clamp(0.0, last);
        Some((cell, u, v))
    }

    /// The ground colour of the screen pixel whose centre is at source
    /// `(x, y)`: layers sampled at every sub-sample, everything smooth
    /// (opacity, vertex colour, relief) at the centre.
    pub fn ground(
        &self,
        scratch: &mut Scratch,
        sampler: &GroundSampler,
        geometry: &Geometry,
        shading: &Shading,
        x: f64,
        y: f64,
    ) -> Sample {
        let Some((cell, u, v)) = self.locate(x, y) else {
            return Sample {
                rgb: self.fallback_rgb,
                fallback: true,
                missing: false,
                defaulted: false,
            };
        };
        let (index, lu, lv) = quadrant_of(u, v);
        let quadrant = &cell.quadrants[index];
        let weights = &mut scratch.weights;
        weights.clear();
        let mut remaining = 1.0f32;
        // What missing blended layers left out would have covered.
        let mut left_out = 0.0f32;
        for layer in quadrant.layers.iter().rev() {
            let alpha = layer.opacity_at(lu, lv);
            if alpha <= 0.0 {
                continue;
            }
            let weight = alpha * remaining;
            if self.substitute && layer.origin == Origin::Missing {
                // Nothing to draw: the layers below show through.
                left_out += weight;
                continue;
            }
            if weight > WEIGHT_EPSILON {
                weights.push((layer.texture, layer.origin, weight));
            }
            remaining *= 1.0 - alpha;
            if remaining <= WEIGHT_EPSILON {
                remaining = 0.0;
                break;
            }
        }
        if remaining > 0.0 {
            weights.push((quadrant.base, quadrant.base_origin, remaining));
        }
        let total: f32 = weights.iter().map(|(_, _, w)| w).sum();
        let mut rgb = [0.0f32; 3];
        let mut missing = 0.0f32;
        let mut defaulted = 0.0f32;
        let inv_samples = 1.0 / self.offsets.len() as f32;
        for &(slot, origin, weight) in weights.iter() {
            let weight = weight / total;
            match origin {
                Origin::Own => {}
                Origin::Defaulted => defaulted += weight,
                Origin::Missing => missing += weight,
            }
            match slot.and_then(|s| self.textures.get(s as usize)) {
                Some(texture) => {
                    let mut acc = [0.0f32; 3];
                    for offset in &self.offsets {
                        let c = texture.sample(x + offset[0], y + offset[1]);
                        for k in 0..3 {
                            acc[k] += c[k];
                        }
                    }
                    for k in 0..3 {
                        rgb[k] += acc[k] * inv_samples * weight;
                    }
                }
                None => {
                    for k in 0..3 {
                        rgb[k] += self.missing_rgb[k] * weight;
                    }
                }
            }
        }
        if let Some(colors) = &cell.colors {
            for (k, channel) in rgb.iter_mut().enumerate() {
                let grid = ColorChannel { colors, channel: k };
                *channel *= grid.at(u, v);
            }
        }
        if let Some(n) = sampler.normal(x as f32, y as f32) {
            let factor = shading.factor(geometry.map_normal(n));
            for c in &mut rgb {
                *c *= factor;
            }
        }
        Sample {
            rgb,
            fallback: false,
            missing: missing > COUNT_EPSILON || left_out > COUNT_EPSILON,
            defaulted: defaulted > COUNT_EPSILON,
        }
    }
}

/// One channel of a cell's vertex colours, interpolated bilinearly.
struct ColorChannel<'a> {
    colors: &'a [[f32; 3]],
    channel: usize,
}

impl ColorChannel<'_> {
    fn at(&self, u: f64, v: f64) -> f32 {
        let last = (LAND_GRID - 2) as f64;
        let c = u.floor().clamp(0.0, last);
        let r = v.floor().clamp(0.0, last);
        let fu = (u - c).clamp(0.0, 1.0) as f32;
        let fv = (v - r).clamp(0.0, 1.0) as f32;
        let (c, r) = (c as usize, r as usize);
        let at = |r: usize, c: usize| self.colors[r * LAND_GRID + c][self.channel];
        let south = at(r, c) + (at(r, c + 1) - at(r, c)) * fu;
        let north = at(r + 1, c) + (at(r + 1, c + 1) - at(r + 1, c)) * fu;
        south + (north - south) * fv
    }
}
