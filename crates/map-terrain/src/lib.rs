//! Ground rendering, floor-tile slicing and tile-library deduplication.
//!
//! [`build_ground`] turns the terrain inside one [`MapWindow`] into the
//! floor of a Fallout 2 map (`docs/contracts.md` section 9.1):
//!
//! 1. **Ground colour**, per screen pixel of map screen space
//!    (`HexGrid::default()`): the pixel centre is taken to the ground plane
//!    through the projection's screen vectors and its `tile_from_source`,
//!    so yaw and units come from [`Projection`] alone. At the source point,
//!    the quadrant's base layer is blended with each alpha layer in
//!    ascending layer order (opacity bilinear over the quadrant's 17 × 17
//!    grid), multiplied by the vertex colour (bilinear over 33 × 33) and by
//!    the relief term. Textures are sampled at `supersample²` points per
//!    pixel from a level prefiltered to the sub-sample footprint (see
//!    [`TerrainOptions::texture_repeat_units`] for the orientation); the
//!    smooth terms are evaluated at the pixel centre.
//! 2. **Relief.** `shade(n) = ambient + intensity · max(0, n · l)` with the
//!    calibration's key light in map axes, exactly as the sprite renderer
//!    lights a surface; the ground is multiplied by
//!    `max(0, 1 + relief · (shade(n) / shade(up) − 1))`, so flat ground
//!    keeps its texture colour and `relief` scales the departure.
//! 3. **Tone and overlays.** The land colour takes the optional tone curve
//!    ([`TerrainOptions::tone`], none by default), then the overlays are
//!    alpha-composited over it in the order given.
//! 4. **Quantization** per tile with `quantize_at` and ordered dither at
//!    the tile's absolute corner, so the pattern is continuous across
//!    tiles.
//! 5. **Deduplication** into at most `library_cap` tiles (see the
//!    `library` module): exact duplicates, then either weighted k-means on
//!    Oklab thumbnails with medoid representatives or a greedy merge
//!    ([`LibraryMethod`]), then a seam-aware choice per square.
//!
//! **Tile shape.** Square `q`'s tile is the 80 × 36 box at
//! `square_corner(q, false)`; a box pixel belongs to `q` exactly when
//! `pick_square` returns `q`, and every other pixel is index 0.
//!
//! **Dither and the tile lattice.** Neighbouring square corners differ by
//! `(48, −12)` and `(32, 24)`. Modulo the 8 × 8 Bayer period these are
//! `(0, 4)` and `(0, 0)`: every corner has `x ≡ 0 (mod 8)` and only two
//! vertical phases occur, by the parity of `s`. So dithering at absolute
//! coordinates costs at most a factor of two in exact duplicates (a flat,
//! uniform region gives two tiles, not one), and there are no dither seams.
//! Quantizing at a canonical origin would save that one tile per flat
//! colour but put a four-row phase jump on every other tile edge; this
//! crate keeps absolute coordinates.
//!
//! **No land.** A pixel over a cell that was not extracted, or that has no
//! land, takes [`TerrainOptions::fallback_rgb`] and is counted.
//!
//! **Blank ground (interiors).** With [`TerrainOptions::blank_without_land`]
//! a pixel with no land is instead black under its overlays, and, where no
//! overlay covers it at all (alpha 0 in every overlay), it is left index 0:
//! not drawn, so the engine's black shows. A square none of whose pixels
//! has land or overlay coverage is *blank*: its tile is all index 0 and it
//! is flagged in [`Ground::blank_squares`], so a map writer can give it the
//! engine's blank tile instead. In the library the blank tile is a class of
//! its own, never substituted for a drawn tile nor by one (unless
//! `library_cap` is below the number of phase classes, when classes are
//! ignored; blank squares still keep the blank tile then).
//!
//! **Default and missing textures.** A layer the source draws with the
//! engine's default land texture (a quadrant with no base layer, a layer
//! with a null land texture; `mir::LandLayer::defaulted`) is an ordinary
//! layer here, drawn with that texture, and is counted apart
//! ([`GroundStats::defaulted_texture_pixels`]). A layer whose texture is
//! missing (its file was not extracted or does not decode, or, in a
//! document written before the default land texture was extracted, a
//! quadrant with no base layer) is counted as missing and drawn from its
//! neighbours: a blended layer is left out, so the layers below it show
//! through, and a base takes the base texture of the nearest quadrant that
//! has one. Only with nothing to borrow anywhere does a base take the mean
//! colour of the textures found. Setting
//! [`TerrainOptions::missing_texture_rgb`] instead draws every missing
//! layer in that flat tone.

mod library;
mod parallel;
mod terrain;
mod texture;

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use foundry_render::{Calibration, Projection, Tone};
use mir::{GroundSampler, MapWindow, Mir};
use quantize::{Dither, PaletteMap};
use vault_formats::geometry::HexGrid;

pub use terrain::{LandSource, MemoryLand, MirLand};

/// Width of a floor tile in pixels.
pub const TILE_WIDTH: u32 = 80;
/// Height of a floor tile in pixels.
pub const TILE_HEIGHT: u32 = 36;
/// Floor squares in a map.
pub const SQUARES: usize = 10_000;
/// Bytes in one tile image.
pub const TILE_PIXELS: usize = (TILE_WIDTH * TILE_HEIGHT) as usize;
/// Most pixels [`Ground::compose_rect`] paints.
pub const MAX_COMPOSE_PIXELS: u64 = 1 << 26;
/// The colour of ground without land, by default.
pub const DEFAULT_FALLBACK_RGB: [u8; 3] = [92, 84, 70];

/// The revision of what [`build_ground`] makes. Bump it whenever its
/// tiles, squares or statistics for the same inputs change: caches of
/// grounds key on it (mojave-convert's terrain cache), and an older
/// revision's entries are then never read.
pub const OUTPUT_REVISION: u32 = 1;

/// Everything that can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum TerrainError {
    #[error("library cap must be at least 1, got {0}")]
    LibraryCap(usize),
    #[error("option {key}: {reason}")]
    InvalidOption { key: &'static str, reason: String },
    #[error(
        "overlay {index}: a {width} x {height} image needs {expected} bytes, the buffer holds {actual}"
    )]
    Overlay {
        index: usize,
        width: u32,
        height: u32,
        expected: u64,
        actual: usize,
    },
    #[error("the projection does not map the ground plane to the screen invertibly")]
    Projection,
    #[error("a {width} x {height} composition exceeds {MAX_COMPOSE_PIXELS} pixels")]
    ComposeTooLarge { width: u32, height: u32 },
    #[error(transparent)]
    Mir(#[from] mir::MirError),
    #[error(transparent)]
    Quantize(#[from] quantize::QuantizeError),
}

/// An RGBA8 image (straight alpha) in map screen space: pixel `(0, 0)` is
/// at `origin`.
#[derive(Debug, Clone, Copy)]
pub struct Overlay<'a> {
    pub origin: [i32; 2],
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

/// How the library is reduced to the cap when more distinct tiles remain
/// (see the `library` module). Both keep a substitute inside its tile's
/// phase class, and both end with the same seam-aware choice per square.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LibraryMethod {
    /// Weighted k-means on Oklab thumbnails, per class, with the cap shared
    /// among the classes in proportion to their distinct tiles; medoids
    /// are the library. The default, and what every run made before
    /// [`LibraryMethod::GreedyMerge`] existed.
    #[default]
    KMeans,
    /// Start from every distinct tile and repeatedly drop the library tile
    /// whose squares lose least by moving to their next best library tile
    /// (full-resolution error, weighted by squares), until the cap is met.
    /// The cap goes wherever it saves most error, across classes too, and
    /// distinctive tiles are the last to go. Lower error than k-means at
    /// every cap measured; meant for large caps (thousands of tiles).
    GreedyMerge,
}

/// Tunables of [`build_ground`].
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainOptions {
    /// Most tiles in the library; at least 1 (default 2000).
    pub library_cap: usize,
    /// How the library meets the cap (default [`LibraryMethod::KMeans`]).
    pub library_method: LibraryMethod,
    /// Ordered dither strength in Oklab lightness (default
    /// [`Dither::DEFAULT_STRENGTH`]); 0 is no dither.
    pub dither_strength: f32,
    /// How strongly the terrain normal departs from flat shading: 0 is
    /// flat, 1 is the sprite renderer's lighting (default 1, at most 8).
    pub relief: f32,
    /// Worker threads; 0 is treated as 1 (default 1). Never changes output.
    pub jobs: usize,
    /// Ground sub-samples per pixel along each axis, 1 to 4 (default 2).
    pub supersample: u32,
    /// Source units over which a land texture repeats once (default 512, a
    /// calibration item). Texture column grows east, row grows south; the
    /// repeat is aligned to the source origin, so textures run on across
    /// cell edges.
    pub texture_repeat_units: f32,
    /// Round the repeat to the nearest even number of floor squares (512
    /// becomes 560, four squares, at 140 units a square), so that squares
    /// four apart see the textures at the same offset and deduplication can
    /// substitute tiles without breaking texture detail at their edges
    /// (default true). The change of scale is under 10 %.
    pub snap_repeat_to_tiles: bool,
    /// Added to the mip level each land texture is filtered to: negative
    /// is finer, so sharper (default 0, from -3 to 3).
    pub texture_lod_bias: f32,
    /// How much of each filtered texture's fine detail is added back on
    /// top, an unsharp mask in texture space (default 0, at most 4).
    pub texture_sharpen: f32,
    /// `None` (the default): a missing texture is drawn from its
    /// neighbours (see the crate documentation), and a base with nothing
    /// to borrow takes the mean colour of every land texture that was
    /// found (mid grey when none was). `Some`: every missing layer is drawn
    /// in this flat tone, which shows the holes.
    pub missing_texture_rgb: Option<[u8; 3]>,
    /// Colour of ground with no land (default [`DEFAULT_FALLBACK_RGB`]).
    /// Not used with [`TerrainOptions::blank_without_land`].
    pub fallback_rgb: [u8; 3],
    /// For interiors (default false): ground with no land is black under
    /// the overlays and not drawn (index 0) where no overlay covers it, and
    /// squares with nothing drawn at all are blank squares
    /// ([`Ground::blank_squares`]). See the crate documentation.
    pub blank_without_land: bool,
    /// Lloyd iterations of the vector quantizer (default 8, at most 100).
    pub cluster_iterations: u32,
    /// Weight of seam continuity against a square's own error when the
    /// quantizer picks its substitute (default 1, 0 is off, at most 100).
    pub seam_weight: f32,
    /// A tone curve for the land colour (the declutter pass's floor tone,
    /// `foundry_render::Tone`): applied to each pixel's land colour after
    /// texture, vertex colour and relief, **before** the overlays are
    /// composited over it, then the result is quantized. Overlays are left
    /// alone because the slicer tones its baked floor and flat overlays
    /// itself. Ground with no land under
    /// [`TerrainOptions::blank_without_land`] (black) is not toned. `None`
    /// (the default) or [`foundry_render::Tone::IDENTITY`] changes no byte.
    pub tone: Option<Tone>,
    /// The darkest a land vertex colour may make the ground, 0 to 1: each
    /// vertex colour multiplier `c` becomes `floor + (1 - floor) c`. The
    /// source paints shade into vertex colours (Fallout 3's Megaton crater
    /// down to 0.1) that its own sunlight lifts; multiplied as they are,
    /// with dark textures and the relief term, such ground quantizes to
    /// black. Default 0: the colours as stored, every earlier byte.
    pub vertex_color_floor: f32,
}

impl Default for TerrainOptions {
    fn default() -> Self {
        TerrainOptions {
            library_cap: 2000,
            library_method: LibraryMethod::KMeans,
            dither_strength: Dither::DEFAULT_STRENGTH,
            relief: 1.0,
            jobs: 1,
            supersample: 2,
            texture_repeat_units: 512.0,
            snap_repeat_to_tiles: true,
            texture_lod_bias: 0.0,
            texture_sharpen: 0.0,
            missing_texture_rgb: None,
            fallback_rgb: DEFAULT_FALLBACK_RGB,
            blank_without_land: false,
            cluster_iterations: 8,
            seam_weight: 1.0,
            tone: None,
            vertex_color_floor: 0.0,
        }
    }
}

impl TerrainOptions {
    /// Checks every value against its range.
    pub fn validate(&self) -> Result<(), TerrainError> {
        let bad =
            |key: &'static str, reason: String| Err(TerrainError::InvalidOption { key, reason });
        if self.library_cap < 1 {
            return Err(TerrainError::LibraryCap(self.library_cap));
        }
        if !(self.dither_strength.is_finite() && self.dither_strength >= 0.0) {
            return bad(
                "dither_strength",
                format!("{} is not finite and non-negative", self.dither_strength),
            );
        }
        if !(self.relief.is_finite() && (0.0..=8.0).contains(&self.relief)) {
            return bad("relief", format!("{} is outside [0, 8]", self.relief));
        }
        if !(self.texture_lod_bias.is_finite() && (-3.0..=3.0).contains(&self.texture_lod_bias)) {
            return bad(
                "texture_lod_bias",
                format!("{} is outside [-3, 3]", self.texture_lod_bias),
            );
        }
        if !(self.texture_sharpen.is_finite() && (0.0..=4.0).contains(&self.texture_sharpen)) {
            return bad(
                "texture_sharpen",
                format!("{} is outside [0, 4]", self.texture_sharpen),
            );
        }
        if !(1..=4).contains(&self.supersample) {
            return bad(
                "supersample",
                format!("{} is outside [1, 4]", self.supersample),
            );
        }
        if !(self.texture_repeat_units.is_finite()
            && self.texture_repeat_units >= 1.0
            && self.texture_repeat_units <= 1.0e6)
        {
            return bad(
                "texture_repeat_units",
                format!("{} is outside [1, 1e6]", self.texture_repeat_units),
            );
        }
        if !(self.seam_weight.is_finite() && (0.0..=100.0).contains(&self.seam_weight)) {
            return bad(
                "seam_weight",
                format!("{} is outside [0, 100]", self.seam_weight),
            );
        }
        if !(self.vertex_color_floor.is_finite() && (0.0..=1.0).contains(&self.vertex_color_floor))
        {
            return bad(
                "vertex_color_floor",
                format!("{} is outside [0, 1]", self.vertex_color_floor),
            );
        }
        if self.cluster_iterations > 100 {
            return bad(
                "cluster_iterations",
                format!("{} is above 100", self.cluster_iterations),
            );
        }
        if let Some(tone) = &self.tone {
            tone.validate().map_err(|e| TerrainError::InvalidOption {
                key: "tone",
                reason: e.to_string(),
            })?;
        }
        Ok(())
    }
}

/// Numbers about one [`build_ground`] run. Everything except the timings
/// is deterministic.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroundStats {
    /// Tiles before deduplication: one per square.
    pub tiles_before: usize,
    /// Distinct tiles after exact deduplication.
    pub distinct_tiles: usize,
    /// `tiles_before − distinct_tiles`.
    pub exact_duplicates: usize,
    /// Clusters the vector quantizer formed; 0 when it was not needed.
    pub clusters: usize,
    /// The texture repeat used, after snapping.
    pub texture_repeat_used: f32,
    /// Phase classes along `s` and `t`: squares this many apart see the
    /// textures and the dither at the same offset.
    pub phase_period: [usize; 2],
    /// Tiles in the library.
    pub tiles_after: usize,
    /// Squares whose library tile is not their own tile.
    pub substituted_squares: usize,
    /// Mean, over all squares, of the per-pixel Oklab error of the tile
    /// each square uses against its own tile.
    pub mean_substitution_error: f64,
    /// The largest such error.
    pub worst_substitution_error: f64,
    /// Screen pixels (inside the map's squares) over ground with no land.
    pub fallback_pixels: u64,
    /// Squares with no land and no overlay coverage, drawn as the blank
    /// tile; always 0 without [`TerrainOptions::blank_without_land`].
    pub blank_squares: usize,
    /// Screen pixels to which a missing texture contributed (or, when it
    /// was left out, would have).
    pub missing_texture_pixels: u64,
    /// Layers (and quadrants without a base layer) whose texture is missing.
    pub missing_texture_layers: usize,
    /// Of `missing_texture_layers`, the ones that name no texture file at
    /// all; the others name a file that was not extracted or does not
    /// decode.
    pub unnamed_texture_layers: usize,
    /// Missing bases drawn with the base texture of the nearest quadrant.
    pub borrowed_base_layers: usize,
    /// Screen pixels to which the engine's default land texture
    /// contributed, drawn with it; not counted as missing.
    pub defaulted_texture_pixels: u64,
    /// Layers drawn with the engine's default land texture.
    pub defaulted_texture_layers: usize,
    /// Distinct land textures loaded, and the ones that were missing.
    pub textures_loaded: usize,
    pub textures_missing: usize,
    /// Cells under the map with land, and without.
    pub cells_with_land: usize,
    pub cells_without_land: usize,
    /// Loading cells and textures.
    pub load_time: Duration,
    /// Evaluating, compositing and quantizing the tiles.
    pub evaluate_time: Duration,
    /// Building the library.
    pub deduplicate_time: Duration,
}

/// The floor of one map.
#[derive(Debug, Clone, PartialEq)]
pub struct Ground {
    /// The tile library: 80 × 36 indexed images, index 0 outside the
    /// parallelogram.
    pub tiles: Vec<Vec<u8>>,
    /// 10,000 entries: the library index of each square, numbered as
    /// `HexGrid` does.
    pub squares: Vec<u16>,
    /// With [`TerrainOptions::blank_without_land`], 10,000 entries: whether
    /// each square is blank (no land, no overlay coverage). Every blank
    /// square's library tile is the one all-zero tile, which no other square
    /// uses (but see the crate documentation on tiny caps). Empty without
    /// the option.
    pub blank_squares: Vec<bool>,
    pub stats: GroundStats,
}

/// The pixels of square `square`'s tile box that belong to it.
fn square_mask(grid: HexGrid, square: i32) -> Vec<usize> {
    let Some(corner) = grid.square_corner(square, false) else {
        return Vec::new();
    };
    let mut mask = Vec::new();
    for py in 0..TILE_HEIGHT as i64 {
        for px in 0..TILE_WIDTH as i64 {
            let point = [corner[0] + px, corner[1] + py];
            let (Ok(x), Ok(y)) = (i32::try_from(point[0]), i32::try_from(point[1])) else {
                continue;
            };
            if grid.pick_square([x, y], false) == Some(square) {
                mask.push(py as usize * TILE_WIDTH as usize + px as usize);
            }
        }
    }
    mask
}

/// The mask of `square` and the pixels along each of its sides.
fn square_shape(grid: HexGrid, square: i32) -> library::Shape {
    let mask = square_mask(grid, square);
    let mut edges: [Vec<usize>; 4] = Default::default();
    if let Some(corner) = grid.square_corner(square, false) {
        for &p in &mask {
            let x = corner[0] + (p % TILE_WIDTH as usize) as i64;
            let y = corner[1] + (p / TILE_WIDTH as usize) as i64;
            for (side, offset) in library::SIDES.iter().enumerate() {
                let touches = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                    let (Ok(nx), Ok(ny)) = (i32::try_from(x + dx), i32::try_from(y + dy)) else {
                        return false;
                    };
                    grid.pick_square([nx, ny], false) == Some(square + offset)
                });
                if touches {
                    edges[side].push(p);
                }
            }
        }
    }
    library::Shape { mask, edges }
}

impl Ground {
    /// Whether square `square` is blank ([`Ground::blank_squares`]); false
    /// without [`TerrainOptions::blank_without_land`] or off the map.
    pub fn is_blank(&self, square: usize) -> bool {
        self.blank_squares.get(square).copied().unwrap_or(false)
    }

    /// The library index of the blank tile, when some square is blank.
    pub fn blank_tile(&self) -> Option<u16> {
        let q = self.blank_squares.iter().position(|&b| b)?;
        self.squares.get(q).copied()
    }

    /// The screen rectangle that holds every square's tile box:
    /// `(origin, width, height)` in map screen space.
    pub fn bounds() -> ([i32; 2], u32, u32) {
        let grid = HexGrid::default();
        let mut lo = [i64::MAX; 2];
        let mut hi = [i64::MIN; 2];
        for q in [0, 99, 9900, 9999] {
            if let Some(c) = grid.square_corner(q, false) {
                for k in 0..2 {
                    lo[k] = lo[k].min(c[k]);
                }
                hi[0] = hi[0].max(c[0] + i64::from(TILE_WIDTH));
                hi[1] = hi[1].max(c[1] + i64::from(TILE_HEIGHT));
            }
        }
        (
            [lo[0] as i32, lo[1] as i32],
            (hi[0] - lo[0]) as u32,
            (hi[1] - lo[1]) as u32,
        )
    }

    /// The whole floor as one indexed image covering [`Ground::bounds`]:
    /// `(pixels, width, height)`, its pixel `(0, 0)` at `bounds().0`.
    pub fn compose(&self) -> (Vec<u8>, u32, u32) {
        let (origin, width, height) = Self::bounds();
        let pixels = self.paint(origin, width, height);
        (pixels, width, height)
    }

    /// The floor inside a screen rectangle, index 0 where no tile is.
    pub fn compose_rect(
        &self,
        origin: [i32; 2],
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, TerrainError> {
        if u64::from(width) * u64::from(height) > MAX_COMPOSE_PIXELS {
            return Err(TerrainError::ComposeTooLarge { width, height });
        }
        Ok(self.paint(origin, width, height))
    }

    fn paint(&self, origin: [i32; 2], width: u32, height: u32) -> Vec<u8> {
        let (w, h) = (i64::from(width), i64::from(height));
        let mut out = vec![0u8; (w * h) as usize];
        let grid = HexGrid::default();
        for (q, &entry) in self.squares.iter().enumerate().take(SQUARES) {
            let (Some(corner), Some(tile)) = (
                grid.square_corner(q as i32, false),
                self.tiles.get(usize::from(entry)),
            ) else {
                continue;
            };
            if tile.len() != TILE_PIXELS {
                continue;
            }
            let x0 = corner[0] - i64::from(origin[0]);
            let y0 = corner[1] - i64::from(origin[1]);
            if x0 >= w
                || y0 >= h
                || x0 + i64::from(TILE_WIDTH) <= 0
                || y0 + i64::from(TILE_HEIGHT) <= 0
            {
                continue;
            }
            for py in 0..i64::from(TILE_HEIGHT) {
                let y = y0 + py;
                if !(0..h).contains(&y) {
                    continue;
                }
                for px in 0..i64::from(TILE_WIDTH) {
                    let x = x0 + px;
                    let index = tile[(py * i64::from(TILE_WIDTH) + px) as usize];
                    if index != 0 && (0..w).contains(&x) {
                        out[(y * w + x) as usize] = index;
                    }
                }
            }
        }
        out
    }
}

/// Builds the floor of the map that `window` covers, from a MIR
/// directory. See the crate documentation.
///
/// The window gives the anchor (`window.anchor()`); the mapping from
/// source to tile space, yaw included, is the projection's.
#[allow(clippy::too_many_arguments)]
pub fn build_ground(
    mir: &Mir,
    worldspace: &str,
    window: &MapWindow,
    sampler: &GroundSampler,
    projection: &Projection,
    calibration: &Calibration,
    map: &PaletteMap,
    overlays: &[Overlay],
    options: &TerrainOptions,
) -> Result<Ground, TerrainError> {
    let source = MirLand { mir, worldspace };
    build_ground_from(
        &source,
        window,
        sampler,
        projection,
        calibration,
        map,
        overlays,
        options,
    )
}

/// [`build_ground`] over any [`LandSource`].
#[allow(clippy::too_many_arguments)]
pub fn build_ground_from(
    source: &dyn LandSource,
    window: &MapWindow,
    sampler: &GroundSampler,
    projection: &Projection,
    calibration: &Calibration,
    map: &PaletteMap,
    overlays: &[Overlay],
    options: &TerrainOptions,
) -> Result<Ground, TerrainError> {
    options.validate()?;
    for (index, overlay) in overlays.iter().enumerate() {
        let expected = u64::from(overlay.width) * u64::from(overlay.height) * 4;
        if overlay.rgba.len() as u64 != expected {
            return Err(TerrainError::Overlay {
                index,
                width: overlay.width,
                height: overlay.height,
                expected,
                actual: overlay.rgba.len(),
            });
        }
    }
    let started = Instant::now();
    let geometry = terrain::Geometry::new(projection, window.anchor())?;
    let shading = terrain::Shading::new(calibration, &geometry, options.relief);
    let tile_units = f64::from(projection.tile_units());
    let (repeat, phase_period) = if options.snap_repeat_to_tiles {
        let pairs = (f64::from(options.texture_repeat_units) / (2.0 * tile_units))
            .round()
            .clamp(1.0, 1000.0);
        (pairs * 2.0 * tile_units, [pairs as usize * 2; 2])
    } else {
        // Only the dither phase repeats: by the parity of s.
        (f64::from(options.texture_repeat_units), [2, 1])
    };
    let land = terrain::Terrain::load(source, &geometry, options, repeat)?;
    let load_time = started.elapsed();

    let started = Instant::now();
    let grid = HexGrid::default();
    let dither = Dither::Ordered {
        strength: options.dither_strength,
    };
    let evaluated = parallel::map_with(
        SQUARES,
        options.jobs,
        terrain::Scratch::default,
        |scratch, q| {
            evaluate_square(
                scratch,
                q as i32,
                grid,
                &land,
                sampler,
                &geometry,
                &shading,
                overlays,
                map,
                dither,
                options.blank_without_land,
                options.tone.filter(|t| !t.is_identity()),
            )
        },
    );
    let mut tiles = Vec::with_capacity(SQUARES);
    let mut fallback_pixels = 0;
    let mut missing_texture_pixels = 0;
    let mut defaulted_texture_pixels = 0;
    let mut blank_squares = Vec::new();
    for result in evaluated {
        let square = result?;
        fallback_pixels += square.fallback;
        missing_texture_pixels += square.missing;
        defaulted_texture_pixels += square.defaulted;
        tiles.push(square.indexed);
        if options.blank_without_land {
            blank_squares.push(square.blank);
        }
    }
    let blank_count = blank_squares.iter().filter(|&&b| b).count();
    let evaluate_time = started.elapsed();

    let started = Instant::now();
    let shape = square_shape(grid, 5050);
    // Blank squares form one class past the phase classes, so the blank
    // tile is never substituted for a drawn one nor by one.
    let blank_class = phase_period[0] * phase_period[1];
    let class_of: Vec<usize> = (0..SQUARES)
        .map(|q| {
            if blank_squares.get(q).copied().unwrap_or(false) {
                return blank_class;
            }
            let (s, t) = (99 - q % 100, q / 100);
            (s % phase_period[0]) * phase_period[1] + t % phase_period[1]
        })
        .collect();
    let library = library::build(
        tiles,
        &class_of,
        &shape,
        options.library_cap,
        options.library_method,
        options.cluster_iterations,
        options.seam_weight,
        map,
        options.jobs,
    );
    let mut library = library;
    if blank_count > 0 {
        settle_blank_tile(&mut library.tiles, &mut library.squares, &blank_squares);
    }
    let deduplicate_time = started.elapsed();

    let stats = GroundStats {
        tiles_before: SQUARES,
        distinct_tiles: library.distinct,
        exact_duplicates: SQUARES - library.distinct,
        clusters: library.clusters,
        texture_repeat_used: repeat as f32,
        phase_period,
        tiles_after: library.tiles.len(),
        substituted_squares: library.substituted,
        mean_substitution_error: library.mean_error,
        worst_substitution_error: library.worst_error,
        fallback_pixels,
        blank_squares: blank_count,
        missing_texture_pixels,
        missing_texture_layers: land.missing_layers,
        unnamed_texture_layers: land.unnamed_layers,
        borrowed_base_layers: land.borrowed_bases,
        defaulted_texture_pixels,
        defaulted_texture_layers: land.defaulted_layers,
        textures_loaded: land.textures_loaded,
        textures_missing: land.textures_missing,
        cells_with_land: land.cells_with_land,
        cells_without_land: land.cells_without_land,
        load_time,
        evaluate_time,
        deduplicate_time,
    };
    Ok(Ground {
        tiles: library.tiles,
        squares: library.squares,
        blank_squares,
        stats,
    })
}

/// Points every blank square at the library's all-zero tile, adding it
/// when a reduction (only with a cap below the number of classes) merged
/// it away.
fn settle_blank_tile(tiles: &mut Vec<Vec<u8>>, squares: &mut [u16], blank: &[bool]) {
    let zero = match tiles.iter().position(|t| t.iter().all(|&i| i == 0)) {
        Some(index) => index,
        None => {
            tiles.push(vec![0; TILE_PIXELS]);
            tiles.len() - 1
        }
    };
    for (entry, _) in squares.iter_mut().zip(blank).filter(|(_, b)| **b) {
        *entry = zero as u16;
    }
}

struct SquareTile {
    indexed: Vec<u8>,
    fallback: u64,
    missing: u64,
    defaulted: u64,
    /// `blank_without_land` left every pixel undrawn.
    blank: bool,
}

#[allow(clippy::too_many_arguments)]
fn evaluate_square(
    scratch: &mut terrain::Scratch,
    square: i32,
    grid: HexGrid,
    land: &terrain::Terrain,
    sampler: &GroundSampler,
    geometry: &terrain::Geometry,
    shading: &terrain::Shading,
    overlays: &[Overlay],
    map: &PaletteMap,
    dither: Dither,
    blank_without_land: bool,
    tone: Option<Tone>,
) -> Result<SquareTile, TerrainError> {
    let corner = grid
        .square_corner(square, false)
        .ok_or(TerrainError::Projection)?;
    let corner = [
        i32::try_from(corner[0]).map_err(|_| TerrainError::Projection)?,
        i32::try_from(corner[1]).map_err(|_| TerrainError::Projection)?,
    ];
    let (tw, th) = (TILE_WIDTH as i32, TILE_HEIGHT as i32);
    let touching: Vec<&Overlay> = overlays
        .iter()
        .filter(|o| {
            let (ox, oy) = (i64::from(o.origin[0]), i64::from(o.origin[1]));
            ox < i64::from(corner[0] + tw)
                && oy < i64::from(corner[1] + th)
                && ox + i64::from(o.width) > i64::from(corner[0])
                && oy + i64::from(o.height) > i64::from(corner[1])
        })
        .collect();
    let mut rgba = vec![0u8; TILE_PIXELS * 4];
    let mut fallback = 0;
    let mut missing = 0;
    let mut defaulted = 0;
    let mut drawn = 0u64;
    // Pass 1: the land colour of every pixel of the square.
    let mut land_pixels: Vec<(i32, i32, [f32; 3], bool)> = Vec::new();
    for py in 0..th {
        for px in 0..tw {
            let point = [corner[0] + px, corner[1] + py];
            if grid.pick_square(point, false) != Some(square) {
                continue;
            }
            let [x, y] = geometry.source([f64::from(point[0]) + 0.5, f64::from(point[1]) + 0.5]);
            let sample = land.ground(scratch, sampler, geometry, shading, x, y);
            fallback += u64::from(sample.fallback);
            missing += u64::from(sample.missing);
            defaulted += u64::from(sample.defaulted);
            let blank = blank_without_land && sample.fallback;
            let rgb = if blank {
                [0.0; 3]
            } else {
                sample.rgb.map(|c| c.clamp(0.0, 255.0))
            };
            land_pixels.push((px, py, rgb, blank));
        }
    }
    // The land tone, on the land colour only (crate notes on
    // `TerrainOptions::tone`); without one the colours are untouched.
    if let Some(tone) = tone {
        let mut buffer: Vec<u8> = land_pixels
            .iter()
            .flat_map(|(_, _, rgb, blank)| {
                let [r, g, b] = rgb.map(|c| (c + 0.5) as u8);
                [r, g, b, if *blank { 0 } else { 255 }]
            })
            .collect();
        tone.apply(&mut buffer);
        for (pixel, toned) in land_pixels.iter_mut().zip(buffer.as_chunks::<4>().0) {
            if !pixel.3 {
                pixel.2 = [toned[0], toned[1], toned[2]].map(f32::from);
            }
        }
    }
    // Pass 2: the overlays over it.
    for (px, py, land_rgb, blank) in land_pixels {
        let point = [corner[0] + px, corner[1] + py];
        let mut rgb = land_rgb;
        let mut covered = false;
        for overlay in &touching {
            let ox = i64::from(point[0]) - i64::from(overlay.origin[0]);
            let oy = i64::from(point[1]) - i64::from(overlay.origin[1]);
            if ox < 0 || oy < 0 || ox >= i64::from(overlay.width) || oy >= i64::from(overlay.height)
            {
                continue;
            }
            let at = ((oy * i64::from(overlay.width) + ox) * 4) as usize;
            let px = &overlay.rgba[at..at + 4];
            covered |= px[3] > 0;
            let alpha = f32::from(px[3]) / 255.0;
            for k in 0..3 {
                rgb[k] += (f32::from(px[k]) - rgb[k]) * alpha;
            }
        }
        if blank && !covered {
            // Not drawn: index 0.
            continue;
        }
        drawn += 1;
        let at = (py * tw + px) as usize * 4;
        for k in 0..3 {
            rgba[at + k] = (rgb[k] + 0.5) as u8;
        }
        rgba[at + 3] = 255;
    }
    let indexed = quantize::quantize_at(&rgba, TILE_WIDTH, TILE_HEIGHT, map, dither, corner)?;
    Ok(SquareTile {
        indexed,
        fallback,
        missing,
        defaulted,
        blank: blank_without_land && drawn == 0,
    })
}
