//! Storeys of an interior for a layer of several elevations
//! (`DESIGN-levels.md` (c)).
//!
//! `mir::storeys` joins every triangle within a gap of the level so far,
//! so a stair or ramp merges the floors it joins. [`split_storeys`] cuts
//! the interior cells' navmesh into **floors** and **connectors** instead:
//!
//! 1. **Slope.** A triangle's slope is the gradient of its plane, rise
//!    over run (`|n_xy| / |n_z|` of its normal; a vertical triangle is
//!    infinitely steep). At most [`SplitOptions::flat_max`] it is floor, at
//!    least [`SplitOptions::steep_min`] a connector; in between it takes
//!    the class most of its neighbours have (hysteresis, iterated in
//!    triangle order to a fixed point, at most one pass per triangle),
//!    and one still undecided is floor below the middle of the two
//!    thresholds, else a connector. A gently sloping floor stays whole, a
//!    long ramp stays whole.
//! 2. **Neighbours** are triangles sharing an edge by position (corners
//!    within half a unit), across the navmeshes of every interior cell:
//!    that is what the stored `links` name inside one navmesh, and it
//!    also joins separate navmeshes. Without any shared edge (synthetic
//!    triangles, a mesh of T-junctions) the split falls back to height
//!    bands: floor triangles joined in order of their lowest corner while
//!    the next starts within [`SplitOptions::gap`] of the level's top (the
//!    `mir::storeys` rule, over floor triangles only), and connectors
//!    grouped by overlapping plan boxes.
//! 3. **Floors** are the connected floor triangles, each kept within the
//!    band [`SplitOptions::below`] / [`SplitOptions::above`] around its
//!    area-weighted median height: a component drifting out of it (a
//!    ramp modelled flat) is split at the band edge. A floor smaller than
//!    [`SplitOptions::min_storey_area`] (a dais, a table top) joins the
//!    floor it touches, else the floor whose band holds it.
//! 4. **Landings.** Each connector touches a floor where one of its
//!    corners lies on (within [`SplitOptions::tolerance`] in plan) a
//!    triangle of that floor and within [`SplitOptions::step_z`] of that
//!    triangle's plane: the landing is the mean of those corners.
//!
//! Floors are sorted by height. [`layout`] puts up to three on the
//! elevations of one map and the rest on further maps (`<stem>`,
//! `<stem>b`, ...), and turns each connector between floors adjacent in
//! height into a [`crate::StairLink`]: paired exit grids, since the engine
//! changes elevation only through exit grids.

use std::collections::{BTreeMap, BTreeSet};

use mir::CellDoc;

use crate::walk::covers;
use crate::{
    BuildError, INTERIOR_BAND_ABOVE, INTERIOR_BAND_BELOW, InteriorBuild, Landing, StairLink,
};

/// The split's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitOptions {
    /// The steepest floor, rise over run. Default 0.25.
    pub flat_max: f32,
    /// The gentlest connector, rise over run. Default 0.45.
    pub steep_min: f32,
    /// The band a floor keeps around its median height. Defaults
    /// [`INTERIOR_BAND_BELOW`] and [`INTERIOR_BAND_ABOVE`].
    pub below: f32,
    pub above: f32,
    /// Floors smaller than this (units², seen from above) join a neighbour.
    /// Default 10,000 (a square of 100 units).
    pub min_storey_area: f32,
    /// Plan tolerance of a landing's contact, units. Default 24 (the
    /// builder's navmesh tolerance).
    pub tolerance: f32,
    /// Height tolerance of a landing's contact, units. Default 40.
    pub step_z: f32,
    /// The z-band fallback's gap, units. Default `mir::storeys`' 120.
    pub gap: f32,
}

impl Default for SplitOptions {
    fn default() -> Self {
        Self {
            flat_max: 0.25,
            steep_min: 0.45,
            below: INTERIOR_BAND_BELOW,
            above: INTERIOR_BAND_ABOVE,
            min_storey_area: 10_000.0,
            tolerance: 24.0,
            step_z: 40.0,
            gap: mir::DEFAULT_STOREY_GAP,
        }
    }
}

/// One floor.
#[derive(Debug, Clone, PartialEq)]
pub struct Floor {
    /// The area-weighted median of its triangles' centroid heights.
    pub floor_z: f32,
    /// Lowest and highest corner.
    pub z_range: [f32; 2],
    /// Area seen from above, units².
    pub area: f32,
    pub triangles: usize,
    /// Plan bounding box and area-weighted centre.
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub centre: [f32; 2],
}

impl Floor {
    /// The builder's band for this floor: `floor_z` with `options`' band,
    /// entrance near the floor's centre.
    pub fn interior_build(&self, options: &SplitOptions) -> InteriorBuild {
        InteriorBuild {
            below: options.below,
            above: options.above,
            entrance_near: None,
            ..InteriorBuild::new(self.floor_z)
        }
    }
}

/// Where a connector meets a floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoreyLanding {
    /// Index into [`StoreySplit::floors`].
    pub floor: usize,
    pub point: [f32; 3],
}

/// A stair, ramp or ladder's navmesh: connected connector triangles.
#[derive(Debug, Clone, PartialEq)]
pub struct Connector {
    pub triangles: usize,
    pub z_range: [f32; 2],
    /// One per floor it touches, by floor index.
    pub landings: Vec<StoreyLanding>,
}

/// What [`split_storeys`] found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoreySplit {
    /// Sorted by `floor_z` (ties: larger area first).
    pub floors: Vec<Floor>,
    pub connectors: Vec<Connector>,
    /// Triangles read, and how many are floor and connector.
    pub triangles: usize,
    pub floor_triangles: usize,
    pub connector_triangles: usize,
    /// Whether neighbours were found; `false`: the z-band fallback.
    pub neighbours: bool,
}

/// One readable triangle.
#[derive(Debug, Clone)]
pub(crate) struct Tri {
    pub corners: [[f32; 3]; 3],
    pub slope: f32,
    /// Seen from above.
    pub area: f32,
    pub centroid: [f32; 3],
    /// Its navmesh's form key and its index there.
    pub navmesh: String,
    pub index: u32,
}

impl Tri {
    /// `None` for a triangle with a corner that is not finite or with no
    /// area at all.
    pub(crate) fn new(c: [[f32; 3]; 3], navmesh: &str, index: u32) -> Option<Tri> {
        if !c.iter().flatten().all(|v| v.is_finite()) {
            return None;
        }
        let u = [c[1][0] - c[0][0], c[1][1] - c[0][1], c[1][2] - c[0][2]];
        let v = [c[2][0] - c[0][0], c[2][1] - c[0][1], c[2][2] - c[0][2]];
        let n = [
            f64::from(u[1] * v[2] - u[2] * v[1]),
            f64::from(u[2] * v[0] - u[0] * v[2]),
            f64::from(u[0] * v[1] - u[1] * v[0]),
        ];
        let horizontal = n[0].hypot(n[1]);
        let length = horizontal.hypot(n[2]);
        if length <= 0.0 {
            return None;
        }
        let slope = if n[2].abs() <= 1e-9 * length {
            f32::INFINITY
        } else {
            (horizontal / n[2].abs()) as f32
        };
        Some(Tri {
            corners: c,
            slope,
            area: (0.5 * n[2].abs()) as f32,
            centroid: std::array::from_fn(|k| (c[0][k] + c[1][k] + c[2][k]) / 3.0),
            navmesh: navmesh.to_owned(),
            index,
        })
    }
}

/// Every readable navmesh triangle of the interior (`interior`) or the
/// exterior cells among `cells`, in cell, navmesh and triangle order.
pub(crate) fn read_triangles(cells: &[CellDoc], interior: bool) -> Vec<Tri> {
    let mut out = Vec::new();
    for cell in cells.iter().filter(|c| c.is_interior() == interior) {
        for navmesh in &cell.navmeshes {
            let form = navmesh.form.to_string();
            for index in 0..navmesh.triangles.len() {
                let Some(c) = navmesh.corners(index) else {
                    continue;
                };
                if let Some(tri) = Tri::new(c, &form, index as u32) {
                    out.push(tri);
                }
            }
        }
    }
    out
}

fn read(cells: &[CellDoc]) -> Vec<Tri> {
    read_triangles(cells, true)
}

/// A corner's position key, half-unit precision.
fn key(p: [f32; 3]) -> [i64; 3] {
    p.map(|v| (f64::from(v) * 2.0).round() as i64)
}

/// Neighbours across shared edges, sorted.
pub(crate) fn neighbours(tris: &[Tri]) -> Vec<Vec<usize>> {
    let mut edges: BTreeMap<([i64; 3], [i64; 3]), Vec<usize>> = BTreeMap::new();
    for (i, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let a = key(t.corners[k]);
            let b = key(t.corners[(k + 1) % 3]);
            let edge = if a <= b { (a, b) } else { (b, a) };
            edges.entry(edge).or_default().push(i);
        }
    }
    let mut out = vec![BTreeSet::new(); tris.len()];
    for list in edges.values() {
        for &a in list {
            for &b in list {
                if a != b {
                    out[a].insert(b);
                }
            }
        }
    }
    out.into_iter().map(|s| s.into_iter().collect()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Floor,
    Connector,
    Undecided,
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

/// Connected components of `members` through `adjacent` restricted to
/// `keep`, in order of their lowest member.
pub(crate) fn components(
    members: &[usize],
    adjacent: &[Vec<usize>],
    keep: &dyn Fn(usize) -> bool,
) -> Vec<Vec<usize>> {
    let set: BTreeSet<usize> = members.iter().copied().filter(|i| keep(*i)).collect();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for &start in &set {
        if !seen.insert(start) {
            continue;
        }
        let mut comp = vec![start];
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            for &j in &adjacent[i] {
                if set.contains(&j) && seen.insert(j) {
                    comp.push(j);
                    stack.push(j);
                }
            }
        }
        comp.sort_unstable();
        out.push(comp);
    }
    out
}

fn describe(tris: &[Tri], members: &[usize]) -> Floor {
    let mut z_range = [f32::INFINITY, f32::NEG_INFINITY];
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let mut area = 0.0f64;
    let mut centre = [0.0f64; 2];
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
        for k in 0..2 {
            centre[k] += f64::from(t.area) * f64::from(t.centroid[k]);
        }
    }
    let centre = if area > 0.0 {
        [(centre[0] / area) as f32, (centre[1] / area) as f32]
    } else {
        [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0]
    };
    Floor {
        floor_z: median_z(tris, members),
        z_range,
        area: area as f32,
        triangles: members.len(),
        min,
        max,
        centre,
    }
}

/// The height of triangle `t`'s plane at `(x, y)` (its mean height for a
/// vertical one).
pub(crate) fn plane_z(t: &Tri, x: f32, y: f32) -> f32 {
    let [a, b, c] = t.corners;
    let det = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if det.abs() <= f32::EPSILON {
        return t.centroid[2];
    }
    let l1 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / det;
    let l2 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / det;
    l1 * a[2] + l2 * b[2] + (1.0 - l1 - l2) * c[2]
}

/// Split the interior cells' navmesh into floors and connectors (module
/// notes).
pub fn split_storeys(cells: &[CellDoc], options: &SplitOptions) -> StoreySplit {
    let tris = read(cells);
    let adjacent = neighbours(&tris);
    let linked = adjacent.iter().any(|n| !n.is_empty());
    // 1. Classes with hysteresis.
    let class = classify(&tris, &adjacent, options.flat_max, options.steep_min);
    split_classified(tris, adjacent, linked, class, options)
}

/// The slope classes with hysteresis (module notes, 1): floor or
/// connector for every triangle.
pub(crate) fn classify(
    tris: &[Tri],
    adjacent: &[Vec<usize>],
    flat_max: f32,
    steep_min: f32,
) -> Vec<Class> {
    let options = SplitOptions {
        flat_max,
        steep_min,
        ..SplitOptions::default()
    };
    let mut class: Vec<Class> = tris
        .iter()
        .map(|t| {
            if t.slope <= options.flat_max {
                Class::Floor
            } else if t.slope >= options.steep_min {
                Class::Connector
            } else {
                Class::Undecided
            }
        })
        .collect();
    let between: Vec<usize> = (0..tris.len())
        .filter(|i| class[*i] == Class::Undecided)
        .collect();
    for _ in 0..=between.len() {
        let mut changed = false;
        for &i in &between {
            let floor = adjacent[i]
                .iter()
                .filter(|j| class[**j] == Class::Floor)
                .count();
            let connector = adjacent[i]
                .iter()
                .filter(|j| class[**j] == Class::Connector)
                .count();
            let new = if floor > connector {
                Class::Floor
            } else if connector > floor {
                Class::Connector
            } else {
                class[i]
            };
            if new != class[i] {
                class[i] = new;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let middle = (options.flat_max + options.steep_min) / 2.0;
    for i in between {
        if class[i] == Class::Undecided {
            class[i] = if tris[i].slope < middle {
                Class::Floor
            } else {
                Class::Connector
            };
        }
    }
    class
}

fn split_classified(
    tris: Vec<Tri>,
    adjacent: Vec<Vec<usize>>,
    linked: bool,
    class: Vec<Class>,
    options: &SplitOptions,
) -> StoreySplit {
    let floor_tris: Vec<usize> = (0..tris.len())
        .filter(|i| class[*i] == Class::Floor)
        .collect();
    let connector_tris: Vec<usize> = (0..tris.len())
        .filter(|i| class[*i] == Class::Connector)
        .collect();
    // 2-3. Floors.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    if linked {
        let mut work = components(&floor_tris, &adjacent, &|_| true);
        let mut guard = 0;
        while let Some(group) = work.pop() {
            guard += 1;
            if guard > 4 * tris.len() + 4 {
                groups.push(group);
                continue;
            }
            let m = median_z(&tris, &group);
            let in_band = |i: usize| {
                let z = tris[i].centroid[2];
                z >= m - options.below && z <= m + options.above
            };
            let seed = group
                .iter()
                .copied()
                .filter(|i| in_band(*i))
                .min_by(|a, b| {
                    (tris[*a].centroid[2] - m)
                        .abs()
                        .total_cmp(&(tris[*b].centroid[2] - m).abs())
                        .then(a.cmp(b))
                });
            let Some(seed) = seed else {
                groups.push(group);
                continue;
            };
            let core = components(&group, &adjacent, &in_band)
                .into_iter()
                .find(|c| c.contains(&seed))
                .unwrap_or_else(|| vec![seed]);
            if core.len() == group.len() {
                groups.push(group);
                continue;
            }
            let core_set: BTreeSet<usize> = core.iter().copied().collect();
            let rest: Vec<usize> = group
                .into_iter()
                .filter(|i| !core_set.contains(i))
                .collect();
            groups.push(core);
            work.extend(components(&rest, &adjacent, &|_| true));
        }
    } else {
        let mut order = floor_tris.clone();
        let low = |i: usize| {
            tris[i]
                .corners
                .iter()
                .map(|c| c[2])
                .fold(f32::INFINITY, f32::min)
        };
        let high = |i: usize| {
            tris[i]
                .corners
                .iter()
                .map(|c| c[2])
                .fold(f32::NEG_INFINITY, f32::max)
        };
        order.sort_by(|a, b| low(*a).total_cmp(&low(*b)).then(a.cmp(b)));
        let mut top = f32::NEG_INFINITY;
        for i in order {
            match groups.last_mut() {
                Some(group) if low(i) - top <= options.gap.max(0.0) => {
                    top = top.max(high(i));
                    group.push(i);
                }
                _ => {
                    top = high(i);
                    groups.push(vec![i]);
                }
            }
        }
        for group in &mut groups {
            group.sort_unstable();
        }
    }
    groups.sort_by_key(|g| g[0]);
    // Small floors join the one they touch, else the one whose band holds
    // them.
    let area = |g: &[usize]| g.iter().map(|i| tris[*i].area).sum::<f32>();
    // Small floors that touch nothing and no band holds, by first triangle:
    // storeys of their own after all.
    let mut alone: BTreeSet<usize> = BTreeSet::new();
    while let Some(small) = (0..groups.len())
        .filter(|g| {
            groups.len() > 1
                && area(&groups[*g]) < options.min_storey_area
                && !alone.contains(&groups[*g][0])
        })
        .min_by(|a, b| {
            area(&groups[*a])
                .total_cmp(&area(&groups[*b]))
                .then(a.cmp(b))
        })
    {
        let members: BTreeSet<usize> = groups[small].iter().copied().collect();
        let touching = (0..groups.len()).filter(|g| *g != small).find(|g| {
            groups[*g]
                .iter()
                .any(|i| adjacent[*i].iter().any(|j| members.contains(j)))
        });
        let m = median_z(&tris, &groups[small]);
        let holding = || {
            (0..groups.len()).filter(|g| *g != small).find(|g| {
                let fz = median_z(&tris, &groups[*g]);
                m >= fz - options.below && m <= fz + options.above
            })
        };
        let Some(into) = touching.or_else(holding) else {
            alone.insert(groups[small][0]);
            continue;
        };
        let moved = std::mem::take(&mut groups[small]);
        groups[into].extend(moved);
        groups[into].sort_unstable();
        groups.remove(small);
    }
    let mut floors: Vec<(Floor, Vec<usize>)> = groups
        .into_iter()
        .map(|g| (describe(&tris, &g), g))
        .collect();
    floors.sort_by(|a, b| {
        a.0.floor_z
            .total_cmp(&b.0.floor_z)
            .then(b.0.area.total_cmp(&a.0.area))
            .then(a.1[0].cmp(&b.1[0]))
    });
    // 4. Connectors and their landings.
    let connector_groups: Vec<Vec<usize>> = if linked {
        components(&connector_tris, &adjacent, &|_| true)
    } else {
        // Overlapping plan boxes (within the tolerance) and heights.
        let bbox = |i: usize| {
            let c = &tris[i].corners;
            let lo = [0, 1, 2].map(|k| c.iter().map(|v| v[k]).fold(f32::INFINITY, f32::min));
            let hi = [0, 1, 2].map(|k| c.iter().map(|v| v[k]).fold(f32::NEG_INFINITY, f32::max));
            (lo, hi)
        };
        let touch = |a: usize, b: usize| {
            let (la, ha) = bbox(a);
            let (lb, hb) = bbox(b);
            (0..2).all(|k| la[k] <= hb[k] + options.tolerance && lb[k] <= ha[k] + options.tolerance)
                && la[2] <= hb[2] + options.step_z
                && lb[2] <= ha[2] + options.step_z
        };
        let near: Vec<Vec<usize>> = (0..tris.len())
            .map(|i| {
                if class[i] != Class::Connector {
                    return Vec::new();
                }
                connector_tris
                    .iter()
                    .copied()
                    .filter(|j| *j != i && touch(i, *j))
                    .collect()
            })
            .collect();
        components(&connector_tris, &near, &|_| true)
    };
    let tolerance = f64::from(options.tolerance.max(0.0));
    let mut connectors = Vec::new();
    for group in connector_groups {
        let mut z_range = [f32::INFINITY, f32::NEG_INFINITY];
        let mut corners: BTreeMap<[i64; 3], [f32; 3]> = BTreeMap::new();
        for i in &group {
            for c in &tris[*i].corners {
                z_range[0] = z_range[0].min(c[2]);
                z_range[1] = z_range[1].max(c[2]);
                corners.entry(key(*c)).or_insert(*c);
            }
        }
        let mut landings = Vec::new();
        for (f, (_, members)) in floors.iter().enumerate() {
            let mut sum = [0.0f64; 3];
            let mut count = 0usize;
            for c in corners.values() {
                let p = [f64::from(c[0]), f64::from(c[1])];
                let on = members.iter().any(|i| {
                    let t = &tris[*i];
                    let tri = t.corners.map(|v| [f64::from(v[0]), f64::from(v[1])]);
                    covers(p, tri, tolerance)
                        && (plane_z(t, c[0], c[1]) - c[2]).abs() <= options.step_z
                });
                if on {
                    for k in 0..3 {
                        sum[k] += f64::from(c[k]);
                    }
                    count += 1;
                }
            }
            if count > 0 {
                landings.push(StoreyLanding {
                    floor: f,
                    point: sum.map(|s| (s / count as f64) as f32),
                });
            }
        }
        connectors.push(Connector {
            triangles: group.len(),
            z_range,
            landings,
        });
    }
    StoreySplit {
        floors: floors.into_iter().map(|(f, _)| f).collect(),
        connectors,
        triangles: tris.len(),
        floor_triangles: floor_tris.len(),
        connector_triangles: connector_tris.len(),
        neighbours: linked,
    }
}

/// One map of a [`StoreyLayout`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreyMap {
    /// The map's name: the stem for the first, then the stem's first seven
    /// letters and `b`, `c`, ...
    pub name: String,
    /// Floor indices, elevation 0 first.
    pub floors: Vec<usize>,
}

/// A connector between two floors adjacent in height.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreyStair {
    pub label: String,
    pub floors: [usize; 2],
    pub points: [[f32; 3]; 2],
}

/// Floors on maps and elevations, and the stairs between them.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreyLayout {
    pub maps: Vec<StoreyMap>,
    pub stairs: Vec<StoreyStair>,
}

impl StoreyLayout {
    /// The map (index into [`StoreyLayout::maps`]) and elevation of a
    /// floor.
    pub fn place(&self, floor: usize) -> Option<(usize, u8)> {
        self.maps.iter().enumerate().find_map(|(m, map)| {
            map.floors
                .iter()
                .position(|f| *f == floor)
                .map(|e| (m, e as u8))
        })
    }

    /// The stairs as [`StairLink`]s, the layout's first map being map
    /// `first_map` of the [`crate::LayerInput`].
    pub fn stair_links(&self, first_map: usize) -> Vec<StairLink> {
        self.stairs
            .iter()
            .filter_map(|stair| {
                let a = self.place(stair.floors[0])?;
                let b = self.place(stair.floors[1])?;
                Some(StairLink {
                    label: stair.label.clone(),
                    ends: [
                        Landing {
                            map: first_map + a.0,
                            elevation: a.1,
                            point: stair.points[0],
                        },
                        Landing {
                            map: first_map + b.0,
                            elevation: b.1,
                            point: stair.points[1],
                        },
                    ],
                })
            })
            .collect()
    }
}

/// Up to three floors per map, in height order, the first map named
/// `stem` (module notes); a connector's landings on floors adjacent in
/// height become one stair each (a connector touching three floors gives
/// two). A stair between other floors, or a connector touching one floor,
/// is left out.
pub fn layout(split: &StoreySplit, stem: &str) -> Result<StoreyLayout, BuildError> {
    let count = split.floors.len();
    let mut maps = Vec::new();
    for (m, chunk) in (0..count).collect::<Vec<_>>().chunks(3).enumerate() {
        let name = if m == 0 {
            stem.to_owned()
        } else {
            let letter = u8::try_from(m)
                .ok()
                .and_then(|m| b'a'.checked_add(m))
                .filter(|l| *l <= b'z')
                .ok_or_else(|| {
                    BuildError::Input(format!("{count} storeys need more than 26 maps"))
                })?;
            let head: String = stem.chars().take(7).collect();
            format!("{head}{}", letter as char)
        };
        maps.push(StoreyMap {
            name,
            floors: chunk.to_vec(),
        });
    }
    let mut stairs = Vec::new();
    let mut seen: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for connector in &split.connectors {
        let mut landings = connector.landings.clone();
        landings.sort_by_key(|l| l.floor);
        for pair in landings.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if b.floor != a.floor + 1 {
                continue;
            }
            let n = seen.entry((a.floor, b.floor)).or_default();
            *n += 1;
            let label = if *n == 1 {
                format!("{}-{}", a.floor, b.floor)
            } else {
                format!("{}-{} #{n}", a.floor, b.floor)
            };
            stairs.push(StoreyStair {
                label,
                floors: [a.floor, b.floor],
                points: [a.point, b.point],
            });
        }
    }
    Ok(StoreyLayout { maps, stairs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mir::{CellLocation, FormKey, NavTriangleDoc, NavmeshDoc};

    /// A quad from `(x0, y0)` to `(x1, y1)`, height `z(x)` linear from `z0`
    /// at `x0` to `z1` at `x1`, as two triangles sharing their diagonal.
    fn quad(x0: f32, y0: f32, x1: f32, y1: f32, z0: f32, z1: f32) -> Vec<[[f32; 3]; 3]> {
        vec![
            [[x0, y0, z0], [x1, y0, z1], [x1, y1, z1]],
            [[x0, y0, z0], [x1, y1, z1], [x0, y1, z0]],
        ]
    }

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
            form: FormKey::new("synthetic.esm", 0x9000),
            editor_id: None,
            name: None,
            location: CellLocation::Interior,
            flags: 1,
            water_height: None,
            land: None,
            references: Vec::new(),
            navmeshes: vec![NavmeshDoc {
                form: FormKey::new("synthetic.esm", 0x9001),
                vertices,
                triangles: docs,
                cover_triangles: Vec::new(),
                door_links: Vec::new(),
                external_links: Vec::new(),
            }],
        }
    }

    /// Two floors 300 apart, joined by a steep ramp (slope 1) sharing their
    /// edges.
    fn two_floors() -> Vec<[[f32; 3]; 3]> {
        let mut t = quad(-400.0, -300.0, 400.0, 300.0, 0.0, 0.0);
        t.extend(quad(400.0, -300.0, 700.0, 300.0, 0.0, 300.0));
        t.extend(quad(700.0, -300.0, 1500.0, 300.0, 300.0, 300.0));
        t
    }

    #[test]
    fn a_steep_ramp_is_one_mir_level_but_two_floors_and_a_connector_here() {
        let doc = cell(&two_floors());
        assert_eq!(mir::storeys(&doc).len(), 1, "mir merges the ramp's floors");
        let split = split_storeys(std::slice::from_ref(&doc), &SplitOptions::default());
        assert!(split.neighbours);
        assert_eq!(split.floors.len(), 2);
        assert_eq!(split.floors[0].floor_z, 0.0);
        assert_eq!(split.floors[1].floor_z, 300.0);
        assert_eq!(split.connectors.len(), 1);
        let landings = &split.connectors[0].landings;
        assert_eq!(landings.len(), 2);
        assert_eq!(landings[0].floor, 0);
        assert_eq!(landings[0].point, [400.0, 0.0, 0.0]);
        assert_eq!(landings[1].point, [700.0, 0.0, 300.0]);
        let layout = layout(&split, "tower").unwrap();
        assert_eq!(layout.maps.len(), 1);
        assert_eq!(layout.stairs.len(), 1);
        let links = layout.stair_links(3);
        assert_eq!(links[0].ends[0].map, 3);
        assert_eq!(
            (links[0].ends[0].elevation, links[0].ends[1].elevation),
            (0, 1)
        );
    }

    #[test]
    fn a_gentle_slope_stays_one_floor_and_a_dais_joins_its_floor() {
        // Slope 0.35, between the thresholds, between two flat floors.
        let mut t = quad(-400.0, -300.0, 0.0, 300.0, 0.0, 0.0);
        t.extend(quad(0.0, -300.0, 200.0, 300.0, 0.0, 70.0));
        t.extend(quad(200.0, -300.0, 600.0, 300.0, 70.0, 70.0));
        // A dais 60 units square, 30 up, its walls vertical: its top is
        // its own floor component, under the minimum area.
        t.extend(quad(-300.0, -30.0, -240.0, 30.0, 30.0, 30.0));
        let split = split_storeys(&[cell(&t)], &SplitOptions::default());
        assert_eq!(split.floors.len(), 1, "{:?}", split.floors);
        assert!(split.connectors.is_empty());
        assert_eq!(split.floor_triangles, 8);
    }

    #[test]
    fn without_shared_edges_the_z_bands_give_the_same_split() {
        // The same floors as separate pieces: no edge shared (each quad's
        // second triangle moved a unit off its diagonal).
        let apart = |q: Vec<[[f32; 3]; 3]>| {
            let mut q = q;
            for c in &mut q[1] {
                c[1] += 1.0;
            }
            q
        };
        let mut t = apart(quad(-400.0, -300.0, 400.0, 300.0, 0.0, 0.0));
        t.extend(apart(quad(410.0, -300.0, 690.0, 300.0, 10.0, 290.0)));
        t.extend(apart(quad(700.0, -300.0, 1500.0, 300.0, 300.0, 300.0)));
        let split = split_storeys(&[cell(&t)], &SplitOptions::default());
        assert!(!split.neighbours);
        assert_eq!(split.floors.len(), 2);
        assert_eq!(split.connectors.len(), 1);
        assert_eq!(split.connectors[0].landings.len(), 2);
    }

    #[test]
    fn five_floors_make_two_maps() {
        let mut t = Vec::new();
        for f in 0..5 {
            let z = 300.0 * f as f32;
            let x = 1100.0 * f as f32;
            t.extend(quad(x - 400.0, -300.0, x + 400.0, 300.0, z, z));
            if f < 4 {
                t.extend(quad(x + 400.0, -300.0, x + 700.0, 300.0, z, z + 300.0));
                t.extend(quad(
                    x + 700.0,
                    -300.0,
                    x + 1100.0 - 400.0,
                    300.0,
                    z + 300.0,
                    z + 300.0,
                ));
            }
        }
        let split = split_storeys(&[cell(&t)], &SplitOptions::default());
        assert_eq!(split.floors.len(), 5, "{:?}", split.floors);
        let layout = layout(&split, "towering").unwrap();
        assert_eq!(layout.maps.len(), 2);
        assert_eq!(
            layout.maps[1].name,
            "toweringb".chars().take(7).collect::<String>() + "b"
        );
        assert_eq!(layout.maps[0].floors, [0, 1, 2]);
        assert_eq!(layout.maps[1].floors, [3, 4]);
        assert_eq!(layout.stairs.len(), 4);
        assert_eq!(layout.place(3), Some((1, 0)));
    }
}
