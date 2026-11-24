//! Why an island is an island ([`IslandDiagnosis`]), and reach through the
//! links of a layer ([`layer_reach`]).
//!
//! The written elevation's components say what a walker can reach; they do
//! not say whether the source joins two of them. Here every component
//! apart from the main one is checked against the elevation's own ground
//! rule with nothing blocked (the navmesh in the elevation's band or
//! level, the slope rule where a cell has none): when that ground joins it
//! to the main component, the island is cut off by art the slicer placed
//! (a wall strip, a blocking sprite, a footprint, relief) and the hexes in
//! the way are named; when it does not, the source itself keeps them apart
//! (another navmesh, a deck reached only by stairs).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::islands;
use crate::plan::{ElevationPlan, MapPlan};
use crate::walk::{self, HEXES, Verdict, neighbours};
use crate::{LinkKind, Role};

/// One component of an elevation other than the main one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IslandDiagnosis {
    /// Its lowest hex and its size, in the elevation's planned walkable
    /// grid (passages included).
    pub first_hex: i32,
    pub size: usize,
    /// Whether the elevation's ground rule with nothing blocked joins it to
    /// the main component.
    pub ground_joins: bool,
    /// When it does not: whether the same ground under the tolerance
    /// coverage rule ([`crate::walk::NavmeshCoverage::Tolerance`], every
    /// triangle within 24 units of a hex centre) would, as for a narrow
    /// flight of steps a stricter rule leaves uncovered. `false` when the
    /// elevation already uses that rule.
    pub joins_with_tolerance: bool,
    /// On the shortest such way: each hex that is not walkable and what
    /// keeps it so (`wall strip <reference>`, `scenery <reference>`,
    /// `footprint`, `relief`, or the verdict).
    pub way: Vec<(i32, String)>,
    /// Grids, arrivals and critters on it.
    pub holds: Vec<String>,
    /// Reached from the layer's start through links ([`layer_reach`]);
    /// `None` until the layer is checked.
    pub reached: Option<bool>,
}

/// What stands on a hex that keeps it from being walked.
fn obstacle(elevation: &ElevationPlan, hex: i32, footprints: &BTreeSet<i32>) -> String {
    let mut what: Vec<String> = elevation
        .placements
        .iter()
        .filter(|p| p.hex == hex && crate::plan::effective_blocking(p.role, p.blocking))
        .map(|p| {
            let role = match p.role {
                Role::WallStrip => "wall strip",
                Role::Door => "door",
                Role::Container => "container",
                Role::Item => "item",
                Role::FloorPiece => "floor piece",
                Role::Scenery => "scenery",
            };
            format!("{role} {}", p.reference)
        })
        .collect();
    what.sort();
    what.dedup();
    if footprints.contains(&hex) {
        what.push("footprint".into());
    }
    if elevation.input.blocked_hexes.contains(&hex) {
        what.push("relief".into());
    }
    if what.is_empty() {
        what.push(format!("{:?}", elevation.verdicts[hex as usize]));
    }
    what.join(", ")
}

/// Labels of the grids, arrivals and critters on each hex of `elevation`.
fn holdings(plan: &MapPlan, e: usize) -> BTreeMap<i32, Vec<String>> {
    let elevation = &plan.elevations[e];
    let mut out: BTreeMap<i32, Vec<String>> = BTreeMap::new();
    for exit in &elevation.door_exits {
        for hex in &exit.grid_hexes {
            out.entry(*hex)
                .or_default()
                .push(format!("exit {}", exit.door));
        }
    }
    for link in &elevation.links {
        let label = match link.kind {
            LinkKind::Door => format!("door {}", link.from),
            LinkKind::Stairs => format!("stairs {}", link.from),
        };
        for hex in &link.grid {
            out.entry(*hex).or_default().push(label.clone());
        }
    }
    for (hex, what) in &elevation.arrivals {
        out.entry(*hex).or_default().push(what.clone());
    }
    for spot in &elevation.spots {
        out.entry(spot.hex).or_default().push(format!(
            "critter {}",
            plan.input.critters[spot.index].reference
        ));
    }
    out
}

/// The diagnosis of every component of elevation `e` apart from the one
/// holding `main_hex` (the largest without one), largest first.
pub(crate) fn diagnose(plan: &MapPlan, e: usize, main_hex: Option<i32>) -> Vec<IslandDiagnosis> {
    let elevation = &plan.elevations[e];
    let walkable = &elevation.walkable;
    let comps = islands::components(|h| walkable[h as usize]);
    if comps.len() < 2 {
        return Vec::new();
    }
    let main = main_hex.and_then(|h| comps.of(h)).unwrap_or(0);
    let (free, _) = walk::verdicts(
        &plan.frame,
        elevation.input.sampler,
        plan.input.cells,
        &[],
        &elevation.options,
    );
    let ground = |h: i32| free[h as usize] == Verdict::Walkable || walkable[h as usize];
    let ground_comps = islands::components(ground);
    let main_ground = comps.first_hex(main).and_then(|h| ground_comps.of(h));
    let tolerance_comps = (elevation.options.navmesh_coverage != walk::NavmeshCoverage::Tolerance)
        .then(|| {
            let options = crate::BuildOptions {
                navmesh_coverage: walk::NavmeshCoverage::Tolerance,
                ..elevation.options.clone()
            };
            let (free, _) = walk::verdicts(
                &plan.frame,
                elevation.input.sampler,
                plan.input.cells,
                &[],
                &options,
            );
            islands::components(|h| free[h as usize] == Verdict::Walkable || walkable[h as usize])
        });
    let holds = holdings(plan, e);
    let main_mask = comps.mask(main);
    let mut out = Vec::new();
    for id in 0..comps.len() {
        if id == main {
            continue;
        }
        let Some(first_hex) = comps.first_hex(id) else {
            continue;
        };
        let joins = main_ground.is_some() && ground_comps.of(first_hex) == main_ground;
        let joins_with_tolerance = !joins
            && tolerance_comps.as_ref().is_some_and(|c| {
                let main = comps.first_hex(main).and_then(|h| c.of(h));
                main.is_some() && c.of(first_hex) == main
            });
        let mut way = Vec::new();
        if joins {
            // Breadth first over the free ground from the island to the
            // main component.
            let mut parent = vec![-2i32; HEXES];
            let mut queue = VecDeque::new();
            for hex in comps.members(id) {
                parent[hex as usize] = -1;
                queue.push_back(hex);
            }
            let mut end = None;
            while let Some(hex) = queue.pop_front() {
                if main_mask[hex as usize] {
                    end = Some(hex);
                    break;
                }
                for next in neighbours(hex) {
                    if parent[next as usize] == -2 && ground(next) {
                        parent[next as usize] = hex;
                        queue.push_back(next);
                    }
                }
            }
            let mut at = end.map_or(-1, |h| parent[h as usize]);
            while at >= 0 {
                if !walkable[at as usize] {
                    way.push((at, obstacle(elevation, at, &elevation.footprints.hexes)));
                }
                at = parent[at as usize];
            }
            way.reverse();
        }
        let mut held: Vec<String> = comps
            .members(id)
            .filter_map(|h| holds.get(&h))
            .flatten()
            .cloned()
            .collect();
        held.sort();
        held.dedup();
        out.push(IslandDiagnosis {
            first_hex,
            size: comps.sizes()[id],
            ground_joins: joins,
            joins_with_tolerance,
            way,
            holds: held,
            reached: None,
        });
    }
    out.sort_by(|a, b| b.size.cmp(&a.size).then(a.first_hex.cmp(&b.first_hex)));
    out
}

/// Reach through a layer's links: per map and elevation, the walkable
/// hexes reached from `start` (map index, elevation, hex) by walking and
/// by stepping on exit grids that lead to another map or elevation of the
/// layer. `walkable[m][e]` is the planned walkable grid, `links[m][e]` the
/// grids and their destination `(map, elevation, hex)`.
#[allow(clippy::type_complexity)]
pub fn layer_reach(
    walkable: &[Vec<Vec<bool>>],
    links: &[Vec<Vec<(Vec<i32>, (usize, usize, i32))>>],
    start: (usize, usize, i32),
) -> Vec<Vec<Vec<bool>>> {
    let mut reached: Vec<Vec<Vec<bool>>> = walkable
        .iter()
        .map(|m| m.iter().map(|_| vec![false; HEXES]).collect())
        .collect();
    let mut seeds = VecDeque::new();
    seeds.push_back(start);
    while let Some((m, e, hex)) = seeds.pop_front() {
        let Some(grid) = walkable.get(m).and_then(|w| w.get(e)) else {
            continue;
        };
        if !(0..HEXES as i32).contains(&hex) || reached[m][e][hex as usize] {
            continue;
        }
        // A grid hex itself may not be marked walkable: still stand on it.
        let mut queue = VecDeque::new();
        reached[m][e][hex as usize] = true;
        queue.push_back(hex);
        let grids: BTreeMap<i32, Vec<(usize, usize, i32)>> = {
            let mut g: BTreeMap<i32, Vec<(usize, usize, i32)>> = BTreeMap::new();
            for (hexes, to) in links.get(m).and_then(|l| l.get(e)).into_iter().flatten() {
                for h in hexes {
                    g.entry(*h).or_default().push(*to);
                }
            }
            g
        };
        while let Some(at) = queue.pop_front() {
            // Stepping on a grid changes map or elevation: no walking on.
            if let Some(tos) = grids.get(&at) {
                seeds.extend(tos.iter().copied());
                continue;
            }
            for next in neighbours(at) {
                let n = next as usize;
                if !reached[m][e][n] && (grid[n] || grids.contains_key(&next)) {
                    reached[m][e][n] = true;
                    queue.push_back(next);
                }
            }
        }
    }
    reached
}

/// Per hex: walkable, and in a walkable component one of whose hexes
/// `reached` holds.
pub fn by_component(walkable: &[bool], reached: &[bool]) -> Vec<bool> {
    let comps = islands::components(|h| walkable[h as usize]);
    let mut hit = vec![false; comps.len()];
    for (h, r) in reached.iter().enumerate() {
        if *r && let Some(id) = comps.of(h as i32) {
            hit[id] = true;
        }
    }
    (0..HEXES as i32)
        .map(|h| comps.of(h).is_some_and(|id| hit[id]))
        .collect()
}

/// The planned walkable grids and links of every map, for [`layer_reach`].
#[allow(clippy::type_complexity)]
fn planned(
    plans: &[MapPlan],
) -> (
    Vec<Vec<Vec<bool>>>,
    Vec<Vec<Vec<(Vec<i32>, (usize, usize, i32))>>>,
) {
    let walkable = plans
        .iter()
        .map(|p| p.elevations.iter().map(|e| e.walkable.clone()).collect())
        .collect();
    let links = plans
        .iter()
        .map(|p| {
            p.elevations
                .iter()
                .map(|e| {
                    e.links
                        .iter()
                        .map(|l| (l.grid.clone(), (l.to.0, usize::from(l.to.1), l.to.2)))
                        .collect()
                })
                .collect()
        })
        .collect();
    (walkable, links)
}

/// Reach from the first map's entrance through the planned links, per map
/// and elevation, by whole components ([`by_component`]); `None` when the
/// first map has no entrance yet.
pub(crate) fn planned_reach(plans: &[MapPlan]) -> Option<Vec<Vec<Vec<bool>>>> {
    let first = plans.first()?;
    let start = (0, first.entrance_elevation, first.entrance_hex()?);
    let (walkable, links) = planned(plans);
    let reached = layer_reach(&walkable, &links, start);
    // Whole components: a grid hex is stepped on, never walked past, so
    // the hexes behind a row of grids count with the component they are in.
    let reached = reached
        .iter()
        .zip(&walkable)
        .map(|(map, w)| map.iter().zip(w).map(|(r, w)| by_component(w, r)).collect())
        .collect();
    Some(reached)
}

/// [`crate::LayerOptions::close_unreached`]: every walkable component no
/// walk from the first map's entrance reaches through the layer's links is
/// made not walkable, and recorded on its elevation
/// ([`ElevationPlan::closed`]) with what it held; the sealing border is
/// laid again. Returns the hexes closed.
pub(crate) fn close_unreached(plans: &mut [MapPlan]) -> usize {
    let Some(reached) = planned_reach(plans) else {
        return 0;
    };
    let mut total = 0;
    for (m, plan) in plans.iter_mut().enumerate() {
        for e in 0..plan.elevations.len() {
            let holds = holdings(plan, e);
            let elevation = &mut plan.elevations[e];
            let open: Vec<bool> = (0..HEXES)
                .map(|h| elevation.walkable[h] && !reached[m][e][h])
                .collect();
            if !open.iter().any(|v| *v) {
                continue;
            }
            let comps = islands::components(|h| open[h as usize]);
            for id in 0..comps.len() {
                let Some(first_hex) = comps.first_hex(id) else {
                    continue;
                };
                let mut held: Vec<String> = comps
                    .members(id)
                    .filter_map(|h| holds.get(&h))
                    .flatten()
                    .cloned()
                    .collect();
                held.sort();
                held.dedup();
                elevation.closed.push((first_hex, comps.sizes()[id], held));
                for h in comps.members(id) {
                    elevation.walkable[h as usize] = false;
                    total += 1;
                }
            }
            elevation.border = walk::sealing_border(
                &elevation.walkable,
                &elevation.engine_blocked,
                &elevation.open,
            );
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(from: i32, to: i32) -> Vec<bool> {
        let mut w = vec![false; HEXES];
        for x in from..=to {
            w[(100 * 200 + x) as usize] = true;
        }
        w
    }

    #[test]
    fn reach_follows_grids_and_stops_on_them() {
        // Elevation 0: a row of hexes 20..40 with a grid at 40 leading to
        // elevation 1, hex 100*200+60; elevation 1 holds the row 55..70 and
        // a separate row 80..90 nothing leads to.
        let e0 = row(20, 40);
        let mut e1 = row(55, 70);
        for x in 80..=90 {
            e1[(100 * 200 + x) as usize] = true;
        }
        let walkable = vec![vec![e0, e1.clone()]];
        let links = vec![vec![
            vec![(vec![100 * 200 + 40], (0, 1, 100 * 200 + 60))],
            vec![],
        ]];
        let reached = layer_reach(&walkable, &links, (0, 0, 100 * 200 + 25));
        assert!(reached[0][0][(100 * 200 + 30) as usize]);
        assert!(reached[0][1][(100 * 200 + 66) as usize]);
        assert!(!reached[0][1][(100 * 200 + 85) as usize]);
        // Whole components: a hex behind a grid counts with its component.
        let mut partial = vec![false; HEXES];
        partial[(100 * 200 + 56) as usize] = true;
        let whole = by_component(&e1, &partial);
        assert!(whole[(100 * 200 + 70) as usize]);
        assert!(!whole[(100 * 200 + 85) as usize]);
    }
}
