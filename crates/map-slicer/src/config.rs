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

/// Classification thresholds and slicing options. Lengths are source
/// units after the reference's scale; footprints are counted in hexes
/// (a hex is a quarter of a floor square in area).
///
/// The defaults were tuned on a real desert town window (see the crate
/// documentation); every one can be overridden.
#[derive(Debug, Clone, PartialEq)]
pub struct SlicerConfig {
    /// Model path fragments (normalised: lowercase, `\` separators) whose
    /// references are skipped: effects, markers, light shafts. Matched as
    /// substrings.
    pub skip_patterns: Vec<String>,
    /// Model path fragments rendered as ground relief: rocks, cliffs.
    pub relief_patterns: Vec<String>,
    /// Model path fragments always rendered flat onto the ground, whatever
    /// their height: road pieces whose bounds include a kerb or a slope.
    pub flat_patterns: Vec<String>,
    /// Model path fragments that are one sprite whatever their size:
    /// vegetation, which Fallout 2 also draws as single scenery sprites.
    pub sprite_patterns: Vec<String>,
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
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

impl Default for SlicerConfig {
    fn default() -> Self {
        SlicerConfig {
            skip_patterns: strings(&["effects\\", "marker", "fx\\", "\\fx", "lightbeam", "godray"]),
            relief_patterns: strings(&["rock", "cliff", "canyon", "boulder", "mesa", "rubble"]),
            flat_patterns: strings(&["road", "decal", "tracks", "parkinglot"]),
            sprite_patterns: strings(&[
                "trees\\", "plants\\", "creosote", "shrub", "bush", "cactus",
            ]),
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
        Ok(())
    }

    pub(crate) fn matches(patterns: &[String], model: &str) -> Option<String> {
        patterns
            .iter()
            .find(|p| !p.is_empty() && model.contains(p.as_str()))
            .cloned()
    }
}
