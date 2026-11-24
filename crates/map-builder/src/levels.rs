//! Exterior levels (crate notes, "Exterior levels"): one exterior map of
//! up to three elevations, cut from the source navmesh where walkable
//! surfaces lie over one another.
//!
//! A Fallout 2 elevation is a screen of its own: the engine draws only the
//! elevation the player is on, and changes elevation only through exit
//! grids. Two walkable surfaces may therefore share an elevation exactly
//! when they never lie over one another in plan with a person's height
//! between them; a catwalk over a crater floor, a ramp over the street
//! below it, a roof with navmesh over the room under it cannot.
//! [`split_levels`] finds the fewest such cuts:
//!
//! 1. **Triangles** of the exterior cells' navmeshes, their neighbours
//!    across shared edges (by position, across navmeshes: the rule of
//!    [`crate::storeys`]) and their slope class with hysteresis (floor
//!    below [`LevelOptions::flat_max`], connector above
//!    [`LevelOptions::steep_min`], in between the class of most neighbours).
//! 2. **Regions** are the connected triangles of one class: flat floors
//!    (terrain, decks, roofs, platforms) and connectors (ramps, stairs).
//!    The terrain's own drift never splits a floor here (an exterior floor
//!    follows the land); only overlap does.
//! 3. **Overlap.** Every triangle is rasterized onto the hexes whose
//!    centre it holds (no tolerance: surfaces side by side at different
//!    heights do not overlap along their common edge), with its plane's
//!    height at each hex centre. Two regions **conflict** when
//!    they cover at least [`LevelOptions::min_conflict_hexes`] common hexes
//!    with more than [`LevelOptions::clearance`] between their heights there;
//!    the lower one (by the majority of those hexes) is *below*. A region
//!    that conflicts with itself (a ramp turning over itself) is cut by
//!    height bands of [`LevelOptions::clearance`] around its median until
//!    no piece does ([`LevelRegion::split`]).
//! 4. **Levels.** Each region's level is the longest chain of conflicting
//!    regions below it (0 when nothing walkable lies under it: a raised
//!    platform over solid ground stays on the ground level). Then, largest
//!    region first, a region moves to the level most of its neighbours are
//!    on when that keeps every conflict strictly ordered (lower below
//!    higher), repeated to a fixed point: a small landing between two
//!    upper decks joins them instead of costing two transitions. A region
//!    whose chain is longer than [`LevelOptions::max_levels`] goes to the
//!    level where it conflicts with nothing that most of its neighbours are
//!    on (order aside: it may stand on a lower elevation than a surface it
//!    lies over), else to one where every region in its way can move to a
//!    level free of conflicts, else stays unplaced ([`LevelSplit::unplaced`]):
//!    its triangles are walked on no elevation.
//! 5. **Transitions.** Wherever two neighbouring triangles are on different
//!    levels the walker crosses an elevation: their contact points (the
//!    corners of one lying on the other, within the tolerance in plan and
//!    [`LevelOptions::step_z`] in height), grouped by region pair and by
//!    distance ([`LevelOptions::link_radius`]), are one [`LevelLink`] each.
//!    Its two ends lie [`LevelOptions::nudge`] units into each side from the
//!    contact's mean point, so each elevation's exit grids stand on that
//!    elevation's last walkable hexes before the cut, at the foot or the
//!    head of the ramp, and each arrival a few steps into the other side
//!    ([`LevelSplit::stair_links`] makes them [`crate::StairLink`]s on one
//!    map).
//!
//! Without any shared edge (synthetic triangles) neighbours are triangles
//! of one class whose plan boxes touch (within the tolerance) and whose
//! heights meet within [`LevelOptions::step_z`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mir::{CellDoc, GroundSampler};

use crate::storeys::{self, Class, Tri, components, plane_z};
use crate::walk::{Frame, Raster, covers};
use crate::{ExteriorLevel, Landing, StairLink};

/// The split's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelOptions {
    /// The steepest floor and the gentlest connector, rise over run.
    /// Defaults 0.25 and 0.45 ([`crate::storeys::SplitOptions`]).
    pub flat_max: f32,
    pub steep_min: f32,
    /// Two surfaces over one another with more than this between them
    /// (units) cannot share an elevation: about a person's height. Default
    /// 120.
    pub clearance: f32,
    /// Common hexes two regions need to conflict. Default 2 (one hex is an
    /// edge's rounding).
    pub min_conflict_hexes: usize,
    /// Plan tolerance of contacts and of finding the surface under a point,
    /// units. Default 24 ([`crate::BuildOptions::navmesh_tolerance`]).
    pub tolerance: f32,
    /// Height tolerance of a contact, units. Default 40.
    pub step_z: f32,
    /// Contact points of one region pair further apart than this (units)
    /// are separate transitions. Default 210 (three hexes).
    pub link_radius: f32,
    /// How far into each side a transition's ends lie, units. Default 35
    /// (half a hex).
    pub nudge: f32,
    /// Elevations a map has: 1 to 3. Default 3.
    pub max_levels: u8,
}

impl Default for LevelOptions {
    fn default() -> Self {
        Self {
            flat_max: 0.25,
            steep_min: 0.45,
            clearance: 120.0,
            min_conflict_hexes: 2,
            tolerance: 24.0,
            step_z: 40.0,
            link_radius: 210.0,
            nudge: 35.0,
            max_levels: 3,
        }
    }
}

/// What a region is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RegionKind {
    /// Connected floor triangles: terrain, a deck, a roof.
    Floor,
    /// Connected connector triangles: a ramp, stairs.
    Connector,
}

/// One region (module notes, 2).
#[derive(Debug, Clone, PartialEq)]
pub struct LevelRegion {
    pub kind: RegionKind,
    /// Its level, `None` when unplaced.
    pub level: Option<u8>,
    /// The longest chain of conflicting regions below it (step 4).
    pub chain: usize,
    pub triangles: usize,
    /// Seen from above, units².
    pub area: f32,
    /// Lowest and highest corner, and the area-weighted mean height.
    pub z_range: [f32; 2],
    pub mean_z: f32,
    /// Plan bounding box.
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// Hexes whose centre it holds.
    pub hexes: usize,
    /// The area-weighted mean height of its centroids above the terrain,
    /// where there is terrain.
    pub above_terrain: Option<f32>,
    /// Cut from a region that lay over itself.
    pub split: bool,
}

/// A transition between two levels (module notes, 5).
#[derive(Debug, Clone, PartialEq)]
pub struct LevelLink {
    /// `levels L-H #n`.
    pub label: String,
    /// The regions on either side, lower level first.
    pub regions: [usize; 2],
    pub levels: [u8; 2],
    /// Each end's point (source coordinates), on its own side.
    pub points: [[f32; 3]; 2],
    /// Contact points it was made from.
    pub contacts: usize,
}

/// One level's totals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelSummary {
    pub level: u8,
    pub regions: usize,
    pub triangles: usize,
    pub area: f32,
    /// Hexes whose centre some triangle of the level holds.
    pub hexes: usize,
    pub z_range: [f32; 2],
}

/// What [`split_levels`] found.
#[derive(Debug, Clone, Default)]
pub struct LevelSplit {
    pub regions: Vec<LevelRegion>,
    pub links: Vec<LevelLink>,
    /// One per level that holds a region, by level.
    pub levels: Vec<LevelSummary>,
    /// Triangles read, and how many are connectors.
    pub triangles: usize,
    pub connector_triangles: usize,
    /// Whether shared edges were found; `false`: the box fallback.
    pub neighbours: bool,
    /// Region pairs that conflict.
    pub conflicts: usize,
    /// Every conflicting pair: `(lower, upper, hexes)`, the regions and the
    /// common hexes with more than the clearance between them.
    pub conflict_pairs: Vec<(usize, usize, usize)>,
    /// Levels the data asks for: the longest chain plus one.
    pub needed_levels: usize,
    /// Regions with no level (their chain is too long and every level
    /// conflicts).
    pub unplaced: Vec<usize>,
    tris: Vec<Tri>,
    region_of: Vec<usize>,
    tolerance: f32,
    step_z: f32,
}

/// The area-weighted mean centroid height of `members`.
fn mean_z(tris: &[Tri], members: &[usize]) -> f32 {
    let (sum, weight) = members.iter().fold((0.0f64, 0.0f64), |(s, w), i| {
        let a = f64::from(tris[*i].area.max(1e-6));
        (s + a * f64::from(tris[*i].centroid[2]), w + a)
    });
    if weight > 0.0 {
        (sum / weight) as f32
    } else {
        0.0
    }
}

/// The area-weighted median centroid height of `members`.
fn median_z(tris: &[Tri], members: &[usize]) -> f32 {
    let mut zs: Vec<(f32, f32)> = members
        .iter()
        .map(|i| (tris[*i].centroid[2], tris[*i].area.max(1e-6)))
        .collect();
    zs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f32 = zs.iter().map(|z| z.1).sum();
    let mut run = 0.0;
    for (z, w) in &zs {
        run += w;
        if run >= total / 2.0 {
            return *z;
        }
    }
    zs.last().map_or(0.0, |z| z.0)
}

/// A triangle's plane height at `(x, y)`, clamped to its corners' range
/// (a hex centre within the tolerance outside a steep triangle would
/// otherwise extrapolate).
fn clamped_z(t: &Tri, x: f32, y: f32) -> f32 {
    let lo = t.corners.iter().map(|c| c[2]).fold(f32::INFINITY, f32::min);
    let hi = t
        .corners
        .iter()
        .map(|c| c[2])
        .fold(f32::NEG_INFINITY, f32::max);
    plane_z(t, x, y).clamp(lo, hi)
}

/// How far (units, in plan) [`LevelSplit::level_of_point`] looks for a
/// surface beside a point with none under it.
pub const NEAR_RADIUS: f32 = 160.0;

fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let length = d[0] * d[0] + d[1] * d[1];
    let u = if length > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a[0] + u * d[0] - p[0]).hypot(a[1] + u * d[1] - p[1])
}

fn plan(t: &Tri) -> [[f64; 2]; 3] {
    t.corners.map(|v| [f64::from(v[0]), f64::from(v[1])])
}

/// Neighbours without shared edges: same class, plan boxes within the
/// tolerance, heights within `step_z`.
fn box_neighbours(tris: &[Tri], class: &[Class], tolerance: f32, step_z: f32) -> Vec<Vec<usize>> {
    let bbox = |i: usize| {
        let c = &tris[i].corners;
        let lo = [0, 1, 2].map(|k| c.iter().map(|v| v[k]).fold(f32::INFINITY, f32::min));
        let hi = [0, 1, 2].map(|k| c.iter().map(|v| v[k]).fold(f32::NEG_INFINITY, f32::max));
        (lo, hi)
    };
    let boxes: Vec<_> = (0..tris.len()).map(bbox).collect();
    (0..tris.len())
        .map(|i| {
            let (la, ha) = boxes[i];
            (0..tris.len())
                .filter(|j| {
                    let (lb, hb) = boxes[*j];
                    *j != i
                        && class[*j] == class[i]
                        && (0..2).all(|k| la[k] <= hb[k] + tolerance && lb[k] <= ha[k] + tolerance)
                        && la[2] <= hb[2] + step_z
                        && lb[2] <= ha[2] + step_z
                })
                .collect()
        })
        .collect()
}

/// Per hex: the triangles covering it and their heights there.
type HexCover = BTreeMap<u32, Vec<(usize, f32)>>;

/// Whether `members` lie over themselves somewhere (more than `clearance`
/// between two of their heights on one hex).
fn overlaps_itself(members: &[usize], hexes: &[Vec<(u32, f32)>], clearance: f32) -> bool {
    let mut span: BTreeMap<u32, [f32; 2]> = BTreeMap::new();
    for i in members {
        for (hex, z) in &hexes[*i] {
            let s = span.entry(*hex).or_insert([*z, *z]);
            s[0] = s[0].min(*z);
            s[1] = s[1].max(*z);
            if s[1] - s[0] > clearance {
                return true;
            }
        }
    }
    false
}

/// Split the exterior cells' navmesh into levels (module notes). `frame`
/// is the map's ([`Frame::new`] of its window and projection); `sampler`
/// only gives [`LevelRegion::above_terrain`].
pub fn split_levels(
    cells: &[CellDoc],
    frame: &Frame,
    sampler: &GroundSampler,
    options: &LevelOptions,
) -> LevelSplit {
    let tris = storeys::read_triangles(cells, false);
    let n = tris.len();
    let shared = storeys::neighbours(&tris);
    let linked = shared.iter().any(|v| !v.is_empty());
    let class = storeys::classify(&tris, &shared, options.flat_max, options.steep_min);
    let adjacent = if linked {
        shared
    } else {
        box_neighbours(&tris, &class, options.tolerance, options.step_z)
    };
    // Hexes whose centre lies inside every triangle, with its height
    // there. No tolerance: two surfaces side by side at different heights
    // (a ramp beside the floor) would otherwise overlap along their edge.
    let raster = Raster::new(frame);
    let tolerance = 0.0;
    let hexes: Vec<Vec<(u32, f32)>> = tris
        .iter()
        .map(|t| {
            let tri = t
                .corners
                .map(|v| frame.tile_of_source(f64::from(v[0]), f64::from(v[1])));
            raster
                .hexes_under(tri, tolerance)
                .into_iter()
                .map(|hex| {
                    let [x, y] = frame.source_of_tile(raster.centres[hex as usize]);
                    (hex, clamped_z(t, x as f32, y as f32))
                })
                .collect()
        })
        .collect();
    // 2. Regions: connected triangles of one class, then cut where one
    // lies over itself.
    let mut groups: Vec<(Vec<usize>, bool)> = Vec::new();
    for kind in [Class::Floor, Class::Connector] {
        let members: Vec<usize> = (0..n).filter(|i| class[*i] == kind).collect();
        for group in components(&members, &adjacent, &|_| true) {
            let mut work = vec![(group, false)];
            let mut guard = 0usize;
            while let Some((group, split)) = work.pop() {
                guard += 1;
                if group.len() < 2
                    || guard > 4 * n + 4
                    || !overlaps_itself(&group, &hexes, options.clearance)
                {
                    groups.push((group, split));
                    continue;
                }
                let m = median_z(&tris, &group);
                let half = options.clearance.max(1.0) / 2.0;
                let in_band = |i: usize| (tris[i].centroid[2] - m).abs() <= half;
                let seed = group.iter().copied().min_by(|a, b| {
                    (tris[*a].centroid[2] - m)
                        .abs()
                        .total_cmp(&(tris[*b].centroid[2] - m).abs())
                        .then(a.cmp(b))
                });
                let core = seed
                    .and_then(|seed| {
                        components(&group, &adjacent, &in_band)
                            .into_iter()
                            .find(|c| c.contains(&seed))
                    })
                    .filter(|c| c.len() < group.len())
                    .unwrap_or_else(|| {
                        // No band cut: halve by height.
                        group
                            .iter()
                            .copied()
                            .filter(|i| tris[*i].centroid[2] <= m)
                            .collect()
                    });
                if core.is_empty() || core.len() == group.len() {
                    groups.push((group, split));
                    continue;
                }
                let core_set: BTreeSet<usize> = core.iter().copied().collect();
                let rest: Vec<usize> = group
                    .iter()
                    .copied()
                    .filter(|i| !core_set.contains(i))
                    .collect();
                for piece in components(&core, &adjacent, &|_| true) {
                    work.push((piece, true));
                }
                for piece in components(&rest, &adjacent, &|_| true) {
                    work.push((piece, true));
                }
            }
        }
    }
    groups.sort_by_key(|g| g.0[0]);
    let mut region_of = vec![usize::MAX; n];
    for (r, (members, _)) in groups.iter().enumerate() {
        for i in members {
            region_of[*i] = r;
        }
    }
    let count = groups.len();
    // 3. Conflicts between regions, with which one is below.
    let mut cover: HexCover = BTreeMap::new();
    for (i, list) in hexes.iter().enumerate() {
        for (hex, z) in list {
            cover.entry(*hex).or_default().push((i, *z));
        }
    }
    // (lower region, upper region) -> [hexes where the first is below,
    // hexes where the second is].
    let mut votes: BTreeMap<(usize, usize), [usize; 2]> = BTreeMap::new();
    for entries in cover.values() {
        // One height range per region on this hex.
        let mut spans: BTreeMap<usize, [f32; 2]> = BTreeMap::new();
        for (i, z) in entries {
            let s = spans.entry(region_of[*i]).or_insert([*z, *z]);
            s[0] = s[0].min(*z);
            s[1] = s[1].max(*z);
        }
        let list: Vec<(usize, [f32; 2])> = spans.into_iter().collect();
        for a in 0..list.len() {
            for b in a + 1..list.len() {
                let (ra, sa) = list[a];
                let (rb, sb) = list[b];
                if sb[0] - sa[1] > options.clearance {
                    votes.entry((ra, rb)).or_default()[0] += 1;
                } else if sa[0] - sb[1] > options.clearance {
                    votes.entry((ra, rb)).or_default()[1] += 1;
                }
            }
        }
    }
    let mut describe: Vec<LevelRegion> = groups
        .iter()
        .map(|(members, split)| {
            let mut z_range = [f32::INFINITY, f32::NEG_INFINITY];
            let mut min = [f32::INFINITY; 2];
            let mut max = [f32::NEG_INFINITY; 2];
            let mut area = 0.0f64;
            let mut above = (0.0f64, 0.0f64);
            let mut covered = BTreeSet::new();
            for i in members {
                let t = &tris[*i];
                for c in &t.corners {
                    z_range[0] = z_range[0].min(c[2]);
                    z_range[1] = z_range[1].max(c[2]);
                    for k in 0..2 {
                        min[k] = min[k].min(c[k]);
                        max[k] = max[k].max(c[k]);
                    }
                }
                area += f64::from(t.area);
                if let Some(g) = sampler.height(t.centroid[0], t.centroid[1]) {
                    let a = f64::from(t.area.max(1e-6));
                    above.0 += a * f64::from(t.centroid[2] - g);
                    above.1 += a;
                }
                covered.extend(hexes[*i].iter().map(|(h, _)| *h));
            }
            LevelRegion {
                kind: if class[members[0]] == Class::Connector {
                    RegionKind::Connector
                } else {
                    RegionKind::Floor
                },
                level: None,
                chain: 0,
                triangles: members.len(),
                area: area as f32,
                z_range,
                mean_z: mean_z(&tris, members),
                min,
                max,
                hexes: covered.len(),
                above_terrain: (above.1 > 0.0).then(|| (above.0 / above.1) as f32),
                split: *split,
            }
        })
        .collect();
    // Constraints: (lower, upper).
    let mut constraints: Vec<(usize, usize)> = Vec::new();
    let mut conflict_pairs: Vec<(usize, usize, usize)> = Vec::new();
    for ((a, b), [a_below, b_below]) in &votes {
        if a_below + b_below < options.min_conflict_hexes.max(1) {
            continue;
        }
        let a_lower = match a_below.cmp(b_below) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => (describe[*a].mean_z, *a) <= (describe[*b].mean_z, *b),
        };
        constraints.push(if a_lower { (*a, *b) } else { (*b, *a) });
        let (lower, upper) = if a_lower { (*a, *b) } else { (*b, *a) };
        conflict_pairs.push((lower, upper, a_below + b_below));
    }
    conflict_pairs.sort_unstable();
    constraints.sort_unstable();
    constraints.dedup();
    // 4. Levels: the longest chain below each region.
    let mut chain = vec![0usize; count];
    for _ in 0..=count {
        let mut changed = false;
        for (lower, upper) in &constraints {
            let want = chain[*lower] + 1;
            if want > chain[*upper] && want <= count {
                chain[*upper] = want;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let max_levels = usize::from(options.max_levels.clamp(1, 3));
    let mut level: Vec<Option<usize>> = chain
        .iter()
        .map(|c| (*c < max_levels).then_some(*c))
        .collect();
    let mut below_of: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut above_of: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (lower, upper) in &constraints {
        below_of[*upper].push(*lower);
        above_of[*lower].push(*upper);
    }
    // Region neighbours, weighted by the triangle pairs that touch.
    let mut touching: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for (i, list) in adjacent.iter().enumerate() {
        for j in list {
            let (a, b) = (region_of[i], region_of[*j]);
            if a != b {
                *touching.entry((a, b)).or_default() += 1;
            }
        }
    }
    let mut neighbours_of: Vec<Vec<(usize, usize)>> = vec![Vec::new(); count];
    for ((a, b), w) in &touching {
        neighbours_of[*a].push((*b, *w));
    }
    let fits = |level: &[Option<usize>], r: usize, k: usize| {
        below_of[r]
            .iter()
            .all(|l| level[*l].is_none_or(|lk| lk < k))
            && above_of[r]
                .iter()
                .all(|u| level[*u].is_none_or(|uk| uk > k))
    };
    // Unplaced regions: any level that keeps the order, lowest first.
    let mut by_size: Vec<usize> = (0..count).collect();
    by_size.sort_by(|a, b| {
        describe[*b]
            .area
            .total_cmp(&describe[*a].area)
            .then(a.cmp(b))
    });
    for _ in 0..8 {
        let mut changed = false;
        for &r in &by_size {
            let mut weight = vec![0usize; max_levels];
            for (other, w) in &neighbours_of[r] {
                if let Some(k) = level[*other] {
                    weight[k] += w;
                }
            }
            let current = level[r];
            let mut best = current;
            let mut best_weight = current.map_or(0, |k| weight[k]);
            for (k, w) in weight.iter().enumerate() {
                if *w > best_weight && Some(k) != current && fits(&level, r, k) {
                    best = Some(k);
                    best_weight = *w;
                }
            }
            if best.is_none() {
                best = (0..max_levels).find(|k| fits(&level, r, *k));
            }
            if best != current {
                level[r] = best;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Still unplaced (a chain longer than the levels): any level where
    // nothing it conflicts with stands, order aside; else one where every
    // region in the way can move to a level free of conflicts itself.
    let mut conflicting: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (lower, upper) in &constraints {
        conflicting[*lower].push(*upper);
        conflicting[*upper].push(*lower);
    }
    let free = |level: &[Option<usize>], r: usize, k: usize, except: usize| {
        conflicting[r]
            .iter()
            .all(|o| *o == except || level[*o] != Some(k))
    };
    for &r in &by_size {
        if level[r].is_some() {
            continue;
        }
        let weight = |k: usize| {
            neighbours_of[r]
                .iter()
                .filter(|(o, _)| level[*o] == Some(k))
                .map(|(_, w)| *w)
                .sum::<usize>()
        };
        let direct = (0..max_levels)
            .filter(|k| free(&level, r, *k, usize::MAX))
            .max_by(|a, b| weight(*a).cmp(&weight(*b)).then(b.cmp(a)));
        if let Some(k) = direct {
            level[r] = Some(k);
            continue;
        }
        'levels: for k in 0..max_levels {
            let blockers: Vec<usize> = conflicting[r]
                .iter()
                .copied()
                .filter(|o| level[*o] == Some(k))
                .collect();
            let mut moves = Vec::new();
            for b in &blockers {
                let Some(to) = (0..max_levels).find(|to| *to != k && free(&level, *b, *to, r))
                else {
                    continue 'levels;
                };
                moves.push((*b, to));
            }
            // The blockers must not land on one another either.
            let mut trial = level.clone();
            for (b, to) in &moves {
                trial[*b] = Some(*to);
            }
            trial[r] = Some(k);
            let sound = moves
                .iter()
                .all(|(b, to)| free(&trial, *b, *to, usize::MAX))
                && free(&trial, r, k, usize::MAX);
            if sound {
                level = trial;
                break;
            }
        }
    }
    let mut unplaced = Vec::new();
    for (r, region) in describe.iter_mut().enumerate() {
        region.chain = chain[r];
        region.level = level[r].map(|k| k as u8);
        if region.level.is_none() {
            unplaced.push(r);
        }
    }
    // 5. Transitions.
    let tol = f64::from(options.tolerance.max(0.0));
    let on = |p: [f32; 3], t: &Tri| {
        covers([f64::from(p[0]), f64::from(p[1])], plan(t), tol)
            && (plane_z(t, p[0], p[1]) - p[2]).abs() <= options.step_z
    };
    // (lower region, upper region) -> contact points and, per point, the
    // centroids of the triangles on either side.
    type Contact = ([f32; 3], [f32; 2], [f32; 2]);
    let mut contacts: BTreeMap<(usize, usize), BTreeMap<[i64; 3], Contact>> = BTreeMap::new();
    for (i, list) in adjacent.iter().enumerate() {
        for &j in list {
            if j <= i {
                continue;
            }
            let (ri, rj) = (region_of[i], region_of[j]);
            let (Some(li), Some(lj)) = (describe[ri].level, describe[rj].level) else {
                continue;
            };
            if li == lj {
                continue;
            }
            // Lower level first.
            let (a, b, ta, tb) = if li < lj {
                (ri, rj, &tris[i], &tris[j])
            } else {
                (rj, ri, &tris[j], &tris[i])
            };
            let entry = contacts.entry((a, b)).or_default();
            for p in ta.corners.iter().filter(|p| on(**p, tb)) {
                entry.insert(
                    p.map(|v| (f64::from(v) * 2.0).round() as i64),
                    (
                        *p,
                        [ta.centroid[0], ta.centroid[1]],
                        [tb.centroid[0], tb.centroid[1]],
                    ),
                );
            }
            for p in tb.corners.iter().filter(|p| on(**p, ta)) {
                entry.insert(
                    p.map(|v| (f64::from(v) * 2.0).round() as i64),
                    (
                        *p,
                        [ta.centroid[0], ta.centroid[1]],
                        [tb.centroid[0], tb.centroid[1]],
                    ),
                );
            }
        }
    }
    let mut links = Vec::new();
    let mut seen: BTreeMap<(u8, u8), usize> = BTreeMap::new();
    for ((a, b), points) in contacts {
        let points: Vec<Contact> = points.into_values().collect();
        // Clusters by distance, in point order.
        let mut cluster = vec![usize::MAX; points.len()];
        let mut next = 0;
        for start in 0..points.len() {
            if cluster[start] != usize::MAX {
                continue;
            }
            cluster[start] = next;
            let mut stack = vec![start];
            while let Some(p) = stack.pop() {
                for q in 0..points.len() {
                    let d =
                        (points[p].0[0] - points[q].0[0]).hypot(points[p].0[1] - points[q].0[1]);
                    if cluster[q] == usize::MAX && d <= options.link_radius {
                        cluster[q] = next;
                        stack.push(q);
                    }
                }
            }
            next += 1;
        }
        for c in 0..next {
            let members: Vec<&Contact> = points
                .iter()
                .zip(&cluster)
                .filter(|(_, k)| **k == c)
                .map(|(p, _)| p)
                .collect();
            let k = members.len() as f32;
            let mean = |f: &dyn Fn(&Contact) -> f32| members.iter().map(|m| f(m)).sum::<f32>() / k;
            let m = [mean(&|p| p.0[0]), mean(&|p| p.0[1]), mean(&|p| p.0[2])];
            let side = |toward: [f32; 2]| {
                let d = [toward[0] - m[0], toward[1] - m[1]];
                let length = d[0].hypot(d[1]);
                if length > 1e-3 {
                    let step = options.nudge.min(length);
                    [
                        m[0] + d[0] / length * step,
                        m[1] + d[1] / length * step,
                        m[2],
                    ]
                } else {
                    m
                }
            };
            let ca = [mean(&|p| p.1[0]), mean(&|p| p.1[1])];
            let cb = [mean(&|p| p.2[0]), mean(&|p| p.2[1])];
            let levels = [
                describe[a].level.unwrap_or(0),
                describe[b].level.unwrap_or(0),
            ];
            let number = seen.entry((levels[0], levels[1])).or_default();
            *number += 1;
            links.push(LevelLink {
                label: format!("levels {}-{} #{number}", levels[0], levels[1]),
                regions: [a, b],
                levels,
                points: [side(ca), side(cb)],
                contacts: members.len(),
            });
        }
    }
    // Totals per level.
    let mut levels: Vec<LevelSummary> = Vec::new();
    for k in 0..max_levels as u8 {
        let members: Vec<usize> = (0..count)
            .filter(|r| describe[*r].level == Some(k))
            .collect();
        if members.is_empty() {
            continue;
        }
        let mut hexes_of = BTreeSet::new();
        let mut triangles = 0;
        for (i, r) in region_of.iter().enumerate() {
            if describe[*r].level == Some(k) {
                triangles += 1;
                hexes_of.extend(hexes[i].iter().map(|(h, _)| *h));
            }
        }
        levels.push(LevelSummary {
            level: k,
            regions: members.len(),
            triangles,
            area: members.iter().map(|r| describe[*r].area).sum(),
            hexes: hexes_of.len(),
            z_range: [
                members
                    .iter()
                    .map(|r| describe[*r].z_range[0])
                    .fold(f32::INFINITY, f32::min),
                members
                    .iter()
                    .map(|r| describe[*r].z_range[1])
                    .fold(f32::NEG_INFINITY, f32::max),
            ],
        });
    }
    let connector_triangles = class.iter().filter(|c| **c == Class::Connector).count();
    LevelSplit {
        regions: describe,
        links,
        levels,
        triangles: n,
        connector_triangles,
        neighbours: linked,
        conflicts: constraints.len(),
        conflict_pairs,
        needed_levels: chain.iter().max().map_or(0, |c| c + 1),
        unplaced,
        tris,
        region_of,
        tolerance: options.tolerance,
        step_z: options.step_z,
    }
}

impl LevelSplit {
    /// The triangles of `level` for [`crate::ElevationInput::level`] (and
    /// [`crate::BuildOptions::level`]).
    pub fn exterior_level(&self, level: u8) -> ExteriorLevel {
        let triangles: BTreeSet<(String, u32)> = self
            .tris
            .iter()
            .zip(&self.region_of)
            .filter(|(_, r)| self.regions[**r].level == Some(level))
            .map(|(t, _)| (t.navmesh.clone(), t.index))
            .collect();
        ExteriorLevel {
            level,
            triangles: Arc::new(triangles),
        }
    }

    /// The levels used, 1 to 3 (1 when nothing lies over anything).
    pub fn level_count(&self) -> u8 {
        self.levels.iter().map(|l| l.level + 1).max().unwrap_or(1)
    }

    /// The transitions as [`StairLink`]s on map `map` of a
    /// [`crate::LayerInput`], elevation = level.
    pub fn stair_links(&self, map: usize) -> Vec<StairLink> {
        self.links
            .iter()
            .map(|link| StairLink {
                label: link.label.clone(),
                ends: [0, 1].map(|k| Landing {
                    map,
                    elevation: link.levels[k],
                    point: link.points[k],
                }),
            })
            .collect()
    }

    /// The triangles whose plan holds `(x, y)` within the tolerance, with
    /// their plane height there.
    fn under(&self, x: f32, y: f32) -> impl Iterator<Item = (usize, f32)> + '_ {
        let tol = f64::from(self.tolerance.max(0.0));
        self.tris
            .iter()
            .enumerate()
            .filter(move |(_, t)| covers([f64::from(x), f64::from(y)], plan(t), tol))
            .map(move |(i, t)| (i, clamped_z(t, x, y)))
    }

    /// The level of the surface nearest a point in height among those under
    /// it (a critter, a teleport's arrival point); with none under `(x, y)`
    /// (a door standing in its wall), the placed triangle within
    /// [`NEAR_RADIUS`] units in plan nearest it, plan distance plus height
    /// difference. `None` when there is neither.
    pub fn level_of_point(&self, p: [f32; 3]) -> Option<u8> {
        let under = self
            .under(p[0], p[1])
            .filter_map(|(i, z)| self.regions[self.region_of[i]].level.map(|l| (l, z)))
            .min_by(|a, b| {
                (a.1 - p[2])
                    .abs()
                    .total_cmp(&(b.1 - p[2]).abs())
                    .then(a.0.cmp(&b.0))
            })
            .map(|(l, _)| l);
        if under.is_some() {
            return under;
        }
        let q = [f64::from(p[0]), f64::from(p[1])];
        self.tris
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let level = self.regions[self.region_of[i]].level?;
                let [a, b, c] = plan(t);
                let d = [(a, b), (b, c), (c, a)]
                    .into_iter()
                    .map(|(u, v)| segment_distance(q, u, v))
                    .fold(f64::INFINITY, f64::min);
                (d <= f64::from(NEAR_RADIUS)).then(|| {
                    let z = t.corners.iter().map(|c| c[2]).sum::<f32>() / 3.0;
                    (d + f64::from((z - p[2]).abs()), level)
                })
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, l)| l)
    }

    /// The level of the highest surface at or below `p` (within
    /// [`LevelOptions::step_z`]): where a placed reference standing at `p`
    /// is drawn; `None` when nothing lies under it (ground, level 0).
    pub fn reference_level(&self, p: [f32; 3]) -> Option<u8> {
        self.under(p[0], p[1])
            .filter(|(_, z)| *z <= p[2] + self.step_z)
            .filter_map(|(i, z)| self.regions[self.region_of[i]].level.map(|l| (l, z)))
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(l, _)| l)
    }

    /// The height of `level`'s surface at `(x, y)` (the highest of its
    /// triangles there); `None` where the level has none.
    pub fn surface_height(&self, level: u8, x: f32, y: f32) -> Option<f32> {
        self.under(x, y)
            .filter(|(i, _)| self.regions[self.region_of[*i]].level == Some(level))
            .map(|(_, z)| z)
            .max_by(f32::total_cmp)
    }

    /// Plain-language lines: the levels, the transitions, what is unplaced.
    pub fn summary(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "{} navmesh triangles ({} connectors) in {} regions; {} overlapping pairs; the \
             data asks for {} levels, {} used{}",
            self.triangles,
            self.connector_triangles,
            self.regions.len(),
            self.conflicts,
            self.needed_levels.max(1),
            self.level_count(),
            if self.neighbours {
                ""
            } else {
                " (no shared edge: box neighbours)"
            }
        )];
        for level in &self.levels {
            lines.push(format!(
                "level {}: {} regions, {} triangles, {:.0} square units, {} hexes, z {:.0} to {:.0}",
                level.level,
                level.regions,
                level.triangles,
                level.area,
                level.hexes,
                level.z_range[0],
                level.z_range[1]
            ));
        }
        for link in &self.links {
            lines.push(format!(
                "{}: ({:.0}, {:.0}, {:.0}) on level {} to ({:.0}, {:.0}, {:.0}) on level {}, {} \
                 contact points",
                link.label,
                link.points[0][0],
                link.points[0][1],
                link.points[0][2],
                link.levels[0],
                link.points[1][0],
                link.points[1][1],
                link.points[1][2],
                link.levels[1],
                link.contacts
            ));
        }
        for r in &self.unplaced {
            let region = &self.regions[*r];
            lines.push(format!(
                "unplaced: region {r} ({:?}, {} triangles, {:.0} square units, z {:.0} to {:.0}): \
                 {} surfaces lie under it",
                region.kind,
                region.triangles,
                region.area,
                region.z_range[0],
                region.z_range[1],
                region.chain
            ));
        }
        lines
    }
}

/// The triangles of each level elevation of a map, for placing points
/// (critters, arrival points) on the elevation whose surface they stand on.
pub(crate) struct LevelGeometry {
    /// Per elevation with a level: its triangles.
    per: Vec<(usize, Vec<Tri>)>,
    tolerance: f64,
}

impl LevelGeometry {
    /// `None` when no elevation has a level.
    pub(crate) fn new(cells: &[CellDoc], levels: &[Option<&ExteriorLevel>]) -> Option<Self> {
        if levels.iter().all(Option::is_none) {
            return None;
        }
        let tris = storeys::read_triangles(cells, false);
        let per = levels
            .iter()
            .enumerate()
            .filter_map(|(e, level)| {
                let level = (*level)?;
                Some((
                    e,
                    tris.iter()
                        .filter(|t| level.triangles.contains(&(t.navmesh.clone(), t.index)))
                        .cloned()
                        .collect(),
                ))
            })
            .collect();
        Some(Self {
            per,
            tolerance: f64::from(LevelOptions::default().tolerance),
        })
    }

    /// How far `p` is from elevation `tris`'s surface: the height
    /// difference to a triangle under it, else plan distance plus height
    /// difference to one within [`NEAR_RADIUS`].
    fn score(&self, tris: &[Tri], p: [f32; 3]) -> Option<f64> {
        let q = [f64::from(p[0]), f64::from(p[1])];
        let under = tris
            .iter()
            .filter(|t| covers(q, plan(t), self.tolerance))
            .map(|t| f64::from((clamped_z(t, p[0], p[1]) - p[2]).abs()))
            .min_by(f64::total_cmp);
        if under.is_some() {
            return under;
        }
        tris.iter()
            .filter_map(|t| {
                let [a, b, c] = plan(t);
                let d = [(a, b), (b, c), (c, a)]
                    .into_iter()
                    .map(|(u, v)| segment_distance(q, u, v))
                    .fold(f64::INFINITY, f64::min);
                (d <= f64::from(NEAR_RADIUS)).then(|| {
                    let z = t.corners.iter().map(|c| c[2]).sum::<f32>() / 3.0;
                    // Beside a surface counts after every surface under it.
                    1e6 + d + f64::from((z - p[2]).abs())
                })
            })
            .min_by(f64::total_cmp)
    }

    /// The elevation whose level surface `p` stands on (module notes);
    /// `None` when no level surface is under or near it.
    pub(crate) fn elevation_of(&self, p: [f32; 3]) -> Option<usize> {
        if !p.iter().all(|v| v.is_finite()) {
            return None;
        }
        self.per
            .iter()
            .filter_map(|(e, tris)| self.score(tris, p).map(|s| (s, *e)))
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, e)| e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mir::{CellLocation, FormKey, NavTriangleDoc, NavmeshDoc};

    fn cell(triangles: &[[[f32; 3]; 3]]) -> CellDoc {
        let mut vertices = Vec::new();
        let mut docs = Vec::new();
        for t in triangles {
            let base = vertices.len() as u32;
            vertices.extend_from_slice(t);
            docs.push(NavTriangleDoc {
                vertices: [base, base + 1, base + 2],
                links: [-1; 3],
                flags: 0,
            });
        }
        CellDoc {
            schema_version: mir::SCHEMA_VERSION,
            form: FormKey::new("synthetic.esm", 0x9200),
            editor_id: None,
            name: None,
            location: CellLocation::Exterior {
                worldspace: "synthworld".into(),
                worldspace_form: FormKey::new("synthetic.esm", 0x800),
                x: 0,
                y: 0,
            },
            flags: 0,
            water_height: None,
            land: None,
            references: Vec::new(),
            navmeshes: vec![NavmeshDoc {
                form: FormKey::new("synthetic.esm", 0x9201),
                vertices,
                triangles: docs,
                cover_triangles: Vec::new(),
                door_links: Vec::new(),
                external_links: Vec::new(),
            }],
        }
    }

    fn square(x0: f32, x1: f32, z: f32) -> [[[f32; 3]; 3]; 2] {
        [
            [[x0, 1000.0, z], [x1, 1000.0, z], [x1, 1600.0, z]],
            [[x0, 1000.0, z], [x1, 1600.0, z], [x0, 1600.0, z]],
        ]
    }

    fn frame() -> Frame {
        let projection = foundry_render::Projection::new(&foundry_render::Calibration::default())
            .expect("projection");
        let window = mir::MapWindow::from_center([1300.0, 1300.0], projection.tile_units());
        Frame::new(&window, &projection).expect("frame")
    }

    #[test]
    fn surfaces_side_by_side_share_a_level_and_four_stacked_leave_one_unplaced() {
        let frame = frame();
        let sampler = GroundSampler::default();
        // Two squares side by side at different heights: no overlap.
        let side: Vec<_> = square(1000.0, 1300.0, 0.0)
            .into_iter()
            .chain(square(1300.0, 1600.0, 500.0))
            .collect();
        let split = split_levels(&[cell(&side)], &frame, &sampler, &LevelOptions::default());
        assert_eq!(split.level_count(), 1);
        assert!(split.links.is_empty() && split.conflicts == 0);
        // Four squares over one another.
        let stack: Vec<_> = [0.0, 300.0, 600.0, 900.0]
            .into_iter()
            .flat_map(|z| square(1000.0, 1600.0, z))
            .collect();
        let split = split_levels(&[cell(&stack)], &frame, &sampler, &LevelOptions::default());
        assert!(split.neighbours, "each square's diagonal");
        assert_eq!(split.needed_levels, 4);
        assert_eq!(split.level_count(), 3);
        assert_eq!(split.unplaced.len(), 1);
        assert_eq!(split.regions[split.unplaced[0]].z_range, [900.0, 900.0]);
        assert!(split.summary().iter().any(|l| l.starts_with("unplaced:")));
        // Two levels at most: the third and fourth are unplaced.
        let two = LevelOptions {
            max_levels: 2,
            ..LevelOptions::default()
        };
        let split = split_levels(&[cell(&stack)], &frame, &sampler, &two);
        assert_eq!(split.level_count(), 2);
        assert_eq!(split.unplaced.len(), 2);
        // Each level's triangles, and the point lookups.
        let split = split_levels(&[cell(&stack)], &frame, &sampler, &LevelOptions::default());
        assert_eq!(split.exterior_level(1).triangles.len(), 2);
        assert_eq!(split.level_of_point([1300.0, 1300.0, 310.0]), Some(1));
        assert_eq!(split.reference_level([1300.0, 1300.0, 650.0]), Some(2));
        assert_eq!(split.level_of_point([5000.0, 5000.0, 0.0]), None);
    }
}
