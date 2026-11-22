//! What the slicer is told: the free art entries and the thresholds.

use std::collections::BTreeSet;

/// Free entries in each art list: what the base game leaves of the 4,096
/// a FID can address (contracts section 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub walls: usize,
    pub scenery: usize,
    pub items: usize,
}

impl Budget {
    /// The free entries of one list.
    pub fn free(&self, category: crate::ArtCategory) -> usize {
        match category {
            crate::ArtCategory::Walls => self.walls,
            crate::ArtCategory::Scenery => self.scenery,
            crate::ArtCategory::Items => self.items,
        }
    }
}

/// What the slicer does when an art list would overflow: steps applied in
/// order, cheapest loss first, each only while a list is still over (see
/// the crate documentation, "Budget"). Every step taken is recorded in
/// [`crate::BudgetReport::ladder`]; only when the last one still does not
/// fit is the run a [`crate::SliceError::FidBudget`]. A run that fits at
/// once is never touched. Each step re-renders the window, so a run that
/// climbs the whole ladder costs a few times a plain one.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetLadder {
    /// Climb at all. Off, an overflow is an error at once.
    pub enabled: bool,
    /// Step 1 (walls): perimeter runs merge up to this many hexes.
    pub perimeter_run: usize,
    /// Step 2 (walls): strips under this many pixels join a neighbour.
    pub min_strip_pixels: usize,
    /// Step 3 (walls to scenery): at most this many sliced models, largest
    /// first, become landmarks.
    pub max_landmarks: usize,
    /// Step 4 (walls, scenery): demote the smallest contributors of an
    /// overflowing list to ground relief, fewest entries first.
    pub demote: bool,
    /// Steps 3 and 4 pick from fresh numbers and re-run at most this many
    /// times each.
    pub max_rounds: usize,
}

impl Default for BudgetLadder {
    fn default() -> Self {
        BudgetLadder {
            enabled: true,
            perimeter_run: 8,
            min_strip_pixels: 192,
            max_landmarks: 64,
            demote: true,
            max_rounds: 4,
        }
    }
}

/// Classification thresholds and slicing options. Lengths are source
/// units after the reference's scale; footprints are counted in hexes
/// (a hex is a quarter of a floor square in area).
///
/// The defaults were tuned on a real desert town window (see the crate
/// documentation); every one can be overridden.
#[derive(Debug, Clone, PartialEq)]
pub struct SlicerConfig {
    /// Model path patterns (normalised: lowercase, `\` separators) whose
    /// references are skipped: effects, markers, light shafts. Every
    /// pattern list is matched by the rules of the `pattern` module: `dir\`
    /// a whole directory, `\name` a segment start, `*name` anywhere, a
    /// plain `name` at a word start, `!text` an exclusion.
    pub skip_patterns: Vec<String>,
    /// Model path patterns rendered as ground relief: rocks, cliffs.
    pub relief_patterns: Vec<String>,
    /// Model path patterns always rendered flat onto the ground, whatever
    /// their height: road pieces whose bounds include a kerb or a slope.
    pub flat_patterns: Vec<String>,
    /// Model path patterns that are one sprite whatever their size:
    /// vegetation, which Fallout 2 also draws as single scenery sprites.
    pub sprite_patterns: Vec<String>,
    /// Leading name prefixes after which a plain pattern also starts a
    /// word: `scol` (static collections: `scolroad…`) and `nv`.
    pub name_prefixes: Vec<String>,
    /// Model path patterns of paved slabs (runways, sidewalks) that are
    /// flattened when lower than `slab_height`, even above `flat_height`,
    /// provided they are slab-shaped: at least `slab_min_width` across
    /// their narrowest side, and (when the triangle area is known) with at
    /// least `slab_min_fill` times their footprint in triangle area. A kerb
    /// or a barrier is narrow, and a kerb ring around a block has little
    /// area for its box, so both stay upright and keep blocking.
    pub slab_patterns: Vec<String>,
    /// Below this height (units), a slab matching `slab_patterns` is flat.
    pub slab_height: f32,
    /// A flattened slab is at least this wide (units) across.
    pub slab_min_width: f64,
    /// ...and its triangle area at least this fraction of its footprint.
    pub slab_min_fill: f64,
    /// Model path patterns of terrain pieces: above `max_sliced_hexes` they
    /// stay ground relief instead of becoming landmarks. Below it they are
    /// classified as usual.
    pub terrain_patterns: Vec<String>,
    /// A mesh above `max_sliced_hexes` that is not terrain (it matches no
    /// relief or terrain pattern) is a landmark: rendered upright, cut into
    /// pieces and placed as scenery (see the crate documentation,
    /// "Landmarks"). Off, it is ground relief as in the first pass.
    pub landmarks: bool,
    /// How far beyond the edges of the engine's map view (640 × 380 under
    /// the 640 × 480 interface) a placed object's hex centre may lie and the
    /// object still be drawn, in pixels: `[left, top, right, bottom]`. The
    /// engine draws an object only when its hex is among the hexes a
    /// refresh visits, a fixed lattice over the view widened by 320 pixels
    /// each side and 240 above and below (the `engine_view` module); every
    /// hex centre within 335, 253, 320 and 230 pixels of the left, top,
    /// right and bottom edge is visited, corners included. The defaults
    /// keep 15 or 16 pixels in hand. A piece's anchor is chosen so that its
    /// hex is visited whenever any of its pixels is in the view.
    pub landmark_reach: [u32; 4],
    /// A landmark piece's drawn extent is at most this many pixels wide and
    /// high before sorting cuts it further. Each side must be less than the
    /// matching sum of `landmark_reach` (left plus right, top plus bottom):
    /// the anchor must lie within the reach of the piece's far edges.
    pub landmark_piece_max: [u32; 2],
    /// A landmark is cut into at most this many pieces, so it costs at
    /// most this many scenery entries per distinct render. Pieces past the
    /// visibility cut are spent on sorting (crate documentation).
    pub landmark_max_pieces: usize,
    /// The screen box of a standing character, `[width, height]` pixels,
    /// its bottom centre on its hex centre: the region a landmark piece is
    /// checked against for sorting.
    pub walker_box: [u32; 2],
    /// A walker's box may hold this many landmark pixels sorted the wrong
    /// way before it counts: specks and antenna tips do not.
    pub landmark_sort_tolerance: usize,
    /// When set, the hexes the map builder can make walkable at all (under
    /// the navmesh and the slope limit, before any blocking): landmark
    /// pieces are sorted for walkers on these hexes (and open hexes) only.
    /// Unset, every hex a landmark does not block counts, which costs more
    /// pieces but assumes nothing.
    pub landmark_walkers: Option<BTreeSet<i32>>,
    /// What to do when an art list would overflow.
    pub ladder: BudgetLadder,
    /// Base kinds that are not static scenery (actors, sounds); skipped.
    pub skip_kinds: Vec<String>,
    /// Base kinds of carried items: one sprite each, in the items list.
    pub item_kinds: Vec<String>,
    /// Base kinds of doors.
    pub door_kinds: Vec<String>,
    /// Base kinds of containers.
    pub container_kinds: Vec<String>,
    /// Below this height (units), a mesh is flattened onto the ground.
    pub flat_height: f32,
    /// At most this footprint (hexes), a scenery mesh is one sprite.
    pub sprite_max_hexes: f64,
    /// A sprite's footprint is also at most this long (units), so a thin
    /// fence or wall section is sliced even when its area is small.
    pub sprite_max_span: f64,
    /// Above this footprint (hexes), a mesh is ground relief, not strips.
    pub max_sliced_hexes: f64,
    /// A mesh whose triangle area is below this fraction of its footprint
    /// area is sparse: wires and cables spanning a huge box. Only meshes
    /// larger than a sprite are tested. 0 turns the test off.
    pub sparse_ratio: f64,
    /// A sparse mesh becomes one scenery sprite (its pole drawn, its wires
    /// riding along) instead of being skipped.
    pub sparse_as_sprite: bool,
    /// A strip with fewer opaque pixels than this is merged into the
    /// nearest strip of the same object.
    pub min_strip_pixels: usize,
    /// Merge runs of interior strip hexes (all six neighbours also own
    /// strips of the object) along a hex row into one strip. Exact for the
    /// engine's row-major draw order; keeps roofs from costing one entry
    /// per floor hex.
    pub merge_interior_runs: bool,
    /// Merge runs of consecutive blocking perimeter hexes along a row into
    /// strips of at most this many hexes. Exact for the same reason; 1
    /// keeps one strip per perimeter hex.
    pub max_perimeter_run: usize,
    /// Geometry this high (units) above the terrain blocks a relief hex.
    pub step_height: f32,
    /// Skip a reference whose enable parent starts it disabled.
    pub follow_enable_parents: bool,
    /// Rotations are rounded to this many radians before they key a render.
    pub rotation_quantum: f32,
    /// Scales are rounded to this before they key a render.
    pub scale_quantum: f32,
    /// Side of the largest window handed to the renderer, in pixels. 1,024
    /// fits the renderer's sample limit at supersample 4; a window it still
    /// refuses is split.
    pub render_tile: u32,
    /// A sliced or sprite render larger than this many pixels is skipped.
    pub max_canvas_pixels: u64,
    /// Height of a flattened mesh, as a fraction of its true height: keeps
    /// the transform invertible and the draw order of its parts stable.
    pub flatten: f32,
    /// Decoded textures kept between renders, in bytes.
    pub texture_cache_bytes: usize,
    /// How many contributors the budget report and error list.
    pub top_contributors: usize,
    /// Hexes the source game's navigation data lets an actor stand on. A
    /// sliced object's bounding footprint does not block them, and they
    /// never merge into a run: a porch or a canopy stays open, with one
    /// strip per hex so that it sorts against whoever stands there.
    pub open_hexes: BTreeSet<i32>,
    /// A surface of a sliced object is a floor when it stands this far
    /// (units, lowest and highest) above the terrain under the object's
    /// origin: above a doorstep's rise, below a table top.
    pub floor_band: [f32; 2],
    /// ...and faces up at least this much (the cosine of its tilt).
    pub floor_min_up: f32,
    /// Floors are kept this many hex steps around an open hex, so the
    /// see-through circle around someone standing there finds floor too.
    pub floor_reach: i32,
    /// Interior mode (an interior cell: no land, one floor, a ceiling cut);
    /// see [`crate::InteriorSlice`]. Off (`None`, the default), nothing
    /// changes.
    pub interior: Option<crate::InteriorSlice>,
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

impl Default for SlicerConfig {
    fn default() -> Self {
        SlicerConfig {
            skip_patterns: strings(&[
                "effects\\",
                "*marker",
                "fx\\",
                "\\fx",
                "*lightbeam",
                "*godray",
            ]),
            relief_patterns: strings(&[
                "rock",
                "cliff",
                "canyon",
                "boulder",
                "mesa",
                "rubble",
                "!rocket",
                "!bouldercity",
            ]),
            flat_patterns: strings(&[
                "road",
                "*decal",
                "tracks",
                "parkinglot",
                "!roadsign",
                "!roadblock",
            ]),
            sprite_patterns: strings(&[
                "trees\\", "plants\\", "creosote", "*shrub", "*bush", "*cactus", "!ambush",
            ]),
            name_prefixes: strings(&["nv", "scol"]),
            slab_patterns: strings(&[
                "runway",
                "sidewalk",
                "tarmac",
                "taxiway",
                "helipad",
                "airstrip",
                "pavement",
                "!sidewalkcurb",
                "!sidewalkkerb",
            ]),
            slab_height: 48.0,
            slab_min_width: 140.0,
            slab_min_fill: 0.9,
            terrain_patterns: strings(&["landscape\\", "terrain", "plateau", "dune"]),
            landmarks: true,
            landmark_reach: [320, 238, 304, 214],
            landmark_piece_max: [512, 352],
            landmark_max_pieces: 160,
            walker_box: [48, 80],
            landmark_sort_tolerance: 24,
            landmark_walkers: None,
            ladder: BudgetLadder::default(),
            skip_kinds: strings(&["NPC_", "CREA", "LVLN", "LVLC", "SOUN", "IDLM", "ASPC"]),
            item_kinds: strings(&[
                "ALCH", "AMMO", "ARMO", "BOOK", "CCRD", "CHIP", "CMNY", "IMOD", "KEYM", "MISC",
                "NOTE", "WEAP",
            ]),
            door_kinds: strings(&["DOOR"]),
            container_kinds: strings(&["CONT"]),
            flat_height: 35.0,
            sprite_max_hexes: 2.0,
            sprite_max_span: 150.0,
            max_sliced_hexes: 2_500.0,
            sparse_ratio: 0.15,
            sparse_as_sprite: true,
            min_strip_pixels: 48,
            merge_interior_runs: true,
            max_perimeter_run: 3,
            step_height: 16.0,
            follow_enable_parents: true,
            rotation_quantum: 1.0e-4,
            scale_quantum: 1.0e-4,
            render_tile: 1024,
            max_canvas_pixels: 48_000_000,
            flatten: 1.0e-3,
            texture_cache_bytes: 512 << 20,
            top_contributors: 12,
            open_hexes: BTreeSet::new(),
            floor_band: [8.0, 80.0],
            floor_min_up: 0.9,
            floor_reach: 2,
            interior: None,
        }
    }
}

impl SlicerConfig {
    pub(crate) fn validate(&self) -> Result<(), crate::SliceError> {
        let bad = |what: &str| Err(crate::SliceError::Config(what.to_owned()));
        if !(self.flat_height.is_finite() && self.flat_height >= 0.0) {
            return bad("flat_height must be a finite, non-negative length");
        }
        if !(self.sprite_max_hexes.is_finite() && self.sprite_max_hexes >= 0.0) {
            return bad("sprite_max_hexes must be finite and non-negative");
        }
        if self.max_sliced_hexes.is_nan() || self.max_sliced_hexes < self.sprite_max_hexes {
            return bad("max_sliced_hexes must be at least sprite_max_hexes");
        }
        if !(self.sparse_ratio.is_finite() && self.sparse_ratio >= 0.0) {
            return bad("sparse_ratio must be finite and non-negative");
        }
        if !(self.step_height.is_finite() && self.step_height >= 0.0) {
            return bad("step_height must be finite and non-negative");
        }
        if !(self.rotation_quantum.is_finite() && self.rotation_quantum > 0.0)
            || !(self.scale_quantum.is_finite() && self.scale_quantum > 0.0)
        {
            return bad("rotation_quantum and scale_quantum must be positive");
        }
        if !(self.floor_band.iter().all(|v| v.is_finite())
            && self.floor_band[0] <= self.floor_band[1]
            && self.floor_min_up > 0.0
            && self.floor_min_up <= 1.0
            && self.floor_reach >= 0)
        {
            return bad(
                "floor_band must rise, floor_min_up be in (0, 1] and floor_reach not negative",
            );
        }
        if !(self.flatten.is_finite() && self.flatten > 0.0 && self.flatten <= 1.0) {
            return bad("flatten must be in (0, 1]");
        }
        if !(self.slab_height.is_finite()
            && self.slab_min_width.is_finite()
            && self.slab_min_fill.is_finite())
        {
            return bad("slab_height, slab_min_width and slab_min_fill must be finite");
        }
        if self.landmark_max_pieces == 0 {
            return bad("landmark_max_pieces must be at least 1");
        }
        let [left, top, right, bottom] = self.landmark_reach.map(u64::from);
        let [width, height] = self.landmark_piece_max.map(u64::from);
        if width == 0 || height == 0 || width >= left + right || height >= top + bottom {
            return bad(
                "landmark_piece_max must be positive and less than landmark_reach's left + right \
                 and top + bottom",
            );
        }
        if self.walker_box.contains(&0) {
            return bad("walker_box must be positive");
        }
        if let Some(interior) = &self.interior {
            interior
                .validate()
                .map_err(|e| crate::SliceError::Config(e.to_owned()))?;
        }
        Ok(())
    }

    /// The first pattern of `patterns` matching the normalised `model`.
    pub(crate) fn matches(&self, patterns: &[String], model: &str) -> Option<String> {
        crate::pattern::first_match(patterns, model, &self.name_prefixes)
    }
}
