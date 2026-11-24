//! Furniture: footprints for the map builder, and bodies that block.
//!
//! A pool table, a bar counter or a shelf can be walked into on a written
//! map. A single sprite blocks only the hex it is anchored on, and a sliced
//! object's strips do not block a hex in [`crate::SlicerConfig::open_hexes`];
//! indoors the caller opens every hex the navmesh comes within its
//! tolerance of (24 units, where hexes are about 70 across), so wherever
//! the navmesh runs beside or under a table its strips were written
//! non-blocking. With [`crate::SlicerConfig::furniture`] set:
//!
//! 1. **Which references are furniture.** Free-standing pieces (the lowest
//!    point of the placed bounds at most `max_lift` above the floor or the
//!    terrain under the origin) that are not doors, carried items, floor
//!    pieces, landmarks or ground art: scenery sprites (vegetation and
//!    wall plates excepted) and containers, and sliced objects of at most
//!    `max_hexes` of floor; none of them structural: at most `max_height`
//!    tall and, indoors, not reaching the ceiling cut (a wall's corner
//!    post is often a sprite, and its box would close the doorway beside
//!    it), nor a wall the cutaway takes. `exclude_patterns` and
//!    `exclude_refs` opt models and references out. Each one decided is
//!    listed with its reason in [`crate::FurnitureStats`].
//! 2. **Footprints** (`footprints`). Each piece of furniture gets its placed
//!    bounding footprint ([`crate::Footprint::of`]`(transform,
//!    bounds).polygon`, source `x, y`) in [`crate::Sliced::footprints`], for
//!    `map_builder::BuildOptions::footprints`, which blocks every hex its
//!    base covers. A sliced piece whose body (rule 3) covers less than
//!    `min_fill` of the hexes inside its box (a curved bar, an L-shaped
//!    counter: the box would close the aisle behind it) gets none and is
//!    listed as hollow; its body still blocks with `block_open`. A
//!    footprint that would split the free floor (the open hexes nothing
//!    blocks) into more pieces is not given either (`keeps a passage
//!    open`): a box drawn across a narrow passage must not strand half a
//!    room.
//! 3. **Bodies win over open hexes** (`block_open`). The hexes whose centre
//!    lies within `reach` floor squares of the ground outline of a
//!    triangle of a sliced piece of furniture that stands between
//!    `SlicerConfig::step_height` and `head_height` above the floor (a
//!    table top, a counter, a shelf, their sides and legs) are its body:
//!    its strips there block, open hex or not, and the hexes are returned
//!    in [`crate::Sliced::blocked_hexes`]. Elsewhere the open hex still
//!    wins, so an aisle, a doorway or the floor beside a table stays open:
//!    nothing of the piece stands over it.

use std::collections::{BTreeMap, BTreeSet};

use nif::StaticMesh;

use crate::hex::Plane;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// Furniture footprints and blocking (module documentation).
#[derive(Debug, Clone, PartialEq)]
pub struct FurnitureRule {
    /// Emit [`crate::Sliced::footprints`].
    pub footprints: bool,
    /// The body of a sliced piece of furniture blocks its hexes even where
    /// they are open hexes (rule 3).
    pub block_open: bool,
    /// A sliced piece is furniture up to this footprint (hexes)...
    pub max_hexes: f64,
    /// ...and this height (units).
    pub max_height: f32,
    /// A piece is free-standing when its lowest point is at most this far
    /// (units) above the floor (indoors) or the terrain under its origin.
    pub max_lift: f32,
    /// A sliced piece's footprint is given only when its body covers at
    /// least this share of the hexes inside its box.
    pub min_fill: f64,
    /// Geometry below this height above the floor is body (rule 3).
    pub head_height: f32,
    /// A hex is under the body within this many floor squares of a
    /// triangle's ground outline.
    pub reach: f64,
    /// Model patterns that are never furniture (the crate's pattern rules).
    pub exclude_patterns: Vec<String>,
    /// Form keys that are never furniture.
    pub exclude_refs: BTreeSet<String>,
}

impl Default for FurnitureRule {
    /// Tables, counters, shelves, booths, freezers, display cases: at most
    /// 24 hexes and 180 units, standing within 24 units of the floor; body
    /// up to 110 units (a head), within 0.15 floor squares (about a third
    /// of a hex) of a hex centre; footprints at 60 % fill.
    fn default() -> Self {
        FurnitureRule {
            footprints: true,
            block_open: true,
            max_hexes: 24.0,
            max_height: 180.0,
            max_lift: 24.0,
            min_fill: 0.6,
            head_height: 110.0,
            reach: 0.15,
            exclude_patterns: strings(&["*rug", "*carpet", "*doormat"]),
            exclude_refs: BTreeSet::new(),
        }
    }
}

impl FurnitureRule {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let finite = |v: f64| v.is_finite() && v >= 0.0;
        if !(finite(self.max_hexes)
            && finite(f64::from(self.max_height))
            && self.max_lift.is_finite()
            && finite(self.min_fill)
            && self.head_height.is_finite()
            && finite(self.reach))
        {
            return Err("furniture thresholds must be finite and not negative".into());
        }
        Ok(())
    }
}

/// One reference the furniture rule decided on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FurnitureChoice {
    pub reference: String,
    pub model: String,
    /// `scenery sprite`, `container` or `sliced` when it is furniture
    /// (with a footprint unless `hollow`); otherwise why not.
    pub reason: String,
    /// Whether its footprint is in [`crate::Sliced::footprints`].
    pub footprint: bool,
}

/// What the furniture rule did ([`crate::BudgetReport::furniture`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FurnitureStats {
    /// Furniture and the references considered but refused, sorted by
    /// reference.
    pub choices: Vec<FurnitureChoice>,
    /// Footprints given.
    pub footprints: usize,
    /// Hexes under furniture bodies.
    pub body_hexes: usize,
    /// Open hexes those bodies block (rule 3).
    pub open_blocked: usize,
}

impl FurnitureStats {
    /// Choices by reason.
    pub fn by_reason(&self) -> BTreeMap<&str, usize> {
        let mut out = BTreeMap::new();
        for c in &self.choices {
            *out.entry(c.reason.as_str()).or_insert(0) += 1;
        }
        out
    }
}

/// The body hexes of `mesh` placed by `matrix` (the full placement) over a
/// floor at `floor_z` (module documentation, rule 3), added to `out`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn body_hexes(
    plane: &Plane,
    mesh: &StaticMesh,
    matrix: &[[f32; 4]; 3],
    floor_z: f32,
    step: f32,
    head: f32,
    reach: f64,
    out: &mut BTreeSet<i32>,
) {
    for primitive in mesh.primitives.iter().filter(|p| p.is_well_formed()) {
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
            let high = corners.iter().map(|p| p[2]).fold(f64::MIN, f64::max) - f64::from(floor_z);
            let low = corners.iter().map(|p| p[2]).fold(f64::MAX, f64::min) - f64::from(floor_z);
            if high <= f64::from(step) || low >= f64::from(head) {
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
