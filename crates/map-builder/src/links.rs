//! Phase B (`DESIGN-levels.md` (c), (d)): exits between the plans.
//!
//! - **Stairs grids** first: each [`crate::StairLink`] end gets exit grids
//!   on the walkable hexes at its landing (the hex under the landing and
//!   its neighbours, on the side towards the other landing; the nearest
//!   ring further out, up to [`crate::BuildOptions::door_reach`] steps,
//!   when none is walkable): the one nearest the landing and its
//!   neighbours among those, so one to four grids. A hex already holding
//!   a door's grid is never taken.
//! - **Stub exits** next, as `build_map` makes them, their return hexes
//!   avoiding every grid of the elevation.
//! - **Door links** ([`crate::LayerOptions::link_doors`]): a door whose
//!   destination cell a map of the layer is built from leads there. The
//!   destination is the map and elevation the partner door is placed on,
//!   else the one whose band holds the arrival point's height (else the
//!   nearest band). The arrival hex is a walkable hex 2 to
//!   [`crate::exits::RETURN_RADIUS`] steps from the partner door (around
//!   the arrival point without one), never a grid hex, never hex 0 or the
//!   border; first in the component of the partner door's own grids, then
//!   in the largest component, then anywhere; nearest the arrival point
//!   (the source's teleport position), ties to the lower hex. It faces
//!   away from the partner door (the teleport's rotation without one).
//! - **Stairs links**: each end's grids lead to the other end's arrival
//!   hex: a walkable hex within [`crate::exits::RETURN_RADIUS`] steps of
//!   the other landing, 2 or more steps from that end's own grids, never a
//!   grid hex, by the same tiers and nearest the landing, facing away from
//!   it. A same-map stair leads to the map's own number, so the arrival
//!   hex is never 0 (the engine would keep the entrance).
//!
//! Whatever cannot be linked is a [`SkippedTeleport`] with the reason; the
//! other direction is still linked, one way.

use std::collections::{BTreeMap, BTreeSet};

use vault_formats::HexGrid;

use crate::exits::{self, DoorKind, RETURN_RADIUS, hexes_around, orientation_towards};
use crate::islands::{self, Components, on_border};
use crate::plan::{CellIndex, MapPlan, PlannedLink};
use crate::walk::{Frame, HEXES};
use crate::{LayerInput, LinkKind, SkippedTeleport, StairLink};

/// The grids of a landing on one elevation (module notes): hexes and the
/// steps from the landing's hex to the nearest of them.
fn landing_grids(
    frame: &Frame,
    walkable: &[bool],
    landing: [f32; 3],
    other: [f32; 3],
    exclude: &BTreeSet<i32>,
    reach: i32,
) -> Option<(Vec<i32>, i32)> {
    let at = frame.tile_of_source(f64::from(landing[0]), f64::from(landing[1]));
    let centre = frame.hex_of_tile(at)?;
    let to = frame.tile_of_source(f64::from(other[0]), f64::from(other[1]));
    let d = [to[0] - at[0], to[1] - at[1]];
    let length = d[0].hypot(d[1]);
    let toward = (length > 1e-6).then(|| [d[0] / length, d[1] / length]);
    let offset = |hex: i32| {
        frame
            .hex_centre_tile(hex)
            .map(|c| [c[0] - at[0], c[1] - at[1]])
    };
    // A quarter of a tile of slack: the hex under the landing counts.
    let side = |hex: i32| match (toward, offset(hex)) {
        (Some(u), Some(o)) => o[0] * u[0] + o[1] * u[1] >= -0.25,
        (None, Some(_)) => true,
        (_, None) => false,
    };
    let usable = |hex: i32| {
        walkable.get(hex as usize).copied().unwrap_or(false)
            && !exclude.contains(&hex)
            && !on_border(hex)
    };
    let distance = |hex: i32| offset(hex).map_or(f64::INFINITY, |o| o[0].hypot(o[1]));
    let pick = |ring: Vec<i32>| -> Option<Vec<i32>> {
        let first = *ring
            .iter()
            .min_by(|a, b| distance(**a).total_cmp(&distance(**b)).then(a.cmp(b)))?;
        let mut out: Vec<i32> = ring
            .into_iter()
            .filter(|hex| *hex == first || HexGrid::lattice_distance(*hex, first) == Ok(1))
            .collect();
        out.sort_unstable();
        Some(out)
    };
    // The landing's hex and its neighbours, on the stair's side first.
    let near: Vec<i32> = hexes_around(centre, 1, false)
        .into_iter()
        .filter(|hex| usable(*hex))
        .collect();
    let sided: Vec<i32> = near.iter().copied().filter(|hex| side(*hex)).collect();
    for ring in [sided, near] {
        if let Some(grid) = pick(ring) {
            let steps = grid
                .iter()
                .filter_map(|hex| HexGrid::lattice_distance(centre, *hex).ok())
                .min()
                .unwrap_or(0);
            return Some((grid, steps));
        }
    }
    for steps in 2..=reach {
        let ring: Vec<i32> = hexes_around(centre, steps, true)
            .into_iter()
            .filter(|hex| usable(*hex))
            .collect();
        if let Some(grid) = pick(ring) {
            return Some((grid, steps));
        }
    }
    None
}

/// The arrival hex on an elevation (module notes).
struct Arrival<'a> {
    frame: &'a Frame,
    walkable: &'a [bool],
    components: &'a Components,
    /// Every grid hex of the elevation.
    grids: &'a BTreeSet<i32>,
}

impl Arrival<'_> {
    /// The best hex among `around`'s disc of [`RETURN_RADIUS`] steps that
    /// is at least 2 steps from every hex of `keep_off`, nearest `target`
    /// (source point), preferring the components of `prefer`.
    fn find(&self, around: i32, keep_off: &[i32], target: [f32; 3], prefer: &[i32]) -> Option<i32> {
        let point = self
            .frame
            .tile_of_source(f64::from(target[0]), f64::from(target[1]));
        let preferred: BTreeSet<usize> = prefer
            .iter()
            .filter_map(|hex| self.components.of(*hex))
            .collect();
        let mut best: Option<(u8, f64, i32)> = None;
        for hex in hexes_around(around, RETURN_RADIUS, false) {
            if hex <= 0
                || hex as usize >= HEXES
                || !self.walkable[hex as usize]
                || self.grids.contains(&hex)
                || on_border(hex)
                || keep_off
                    .iter()
                    .any(|k| HexGrid::lattice_distance(*k, hex).unwrap_or(0) < 2)
            {
                continue;
            }
            let Some(c) = self.frame.hex_centre_tile(hex) else {
                continue;
            };
            let tier = match self.components.of(hex) {
                Some(id) if preferred.contains(&id) => 0,
                Some(0) => 1,
                _ => 2,
            };
            let d = (c[0] - point[0]).powi(2) + (c[1] - point[1]).powi(2);
            let better = match best {
                None => true,
                Some((bt, bd, bh)) => (tier, d, hex) < (bt, bd, bh),
            };
            if better {
                best = Some((tier, d, hex));
            }
        }
        best.map(|(_, _, hex)| hex)
    }
}

/// Where a door placement is: map, elevation, hex.
type DoorAt = (usize, usize, i32);

/// The destination elevation of a map for a point at height `z`: the
/// elevation whose band holds it, else the nearest band; band-less
/// elevations hold every height. `None` when the map's frame does not
/// cover the point.
fn elevation_for(plan: &MapPlan, point: [f32; 3]) -> Option<(usize, f32)> {
    plan.frame
        .hex_of_source(f64::from(point[0]), f64::from(point[1]))?;
    // An exterior map of levels: the elevation whose surface the point is
    // on.
    if let Some(e) = plan
        .level_geometry
        .as_ref()
        .and_then(|g| g.elevation_of(point))
    {
        return Some((e, 0.0));
    }
    plan.elevations
        .iter()
        .enumerate()
        .map(|(e, elevation)| {
            let distance = match &elevation.options.interior {
                None => 0.0,
                Some(band) => {
                    let [low, high] = band.band();
                    if point[2] < low {
                        low - point[2]
                    } else if point[2] > high {
                        point[2] - high
                    } else {
                        0.0
                    }
                }
            };
            (e, distance)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
}

/// Phase B over every plan (module notes).
pub(crate) fn resolve(
    plans: &mut [MapPlan],
    layer: &LayerInput,
    index: &CellIndex,
) -> Result<(), crate::BuildError> {
    // Stairs grids.
    for (s, stair) in layer.stairs.iter().enumerate() {
        for i in 0..2 {
            let end = stair.ends[i];
            let other = stair.ends[1 - i];
            let plan = &plans[end.map];
            let elevation = &plan.elevations[usize::from(end.elevation)];
            let exclude = elevation.candidate_grids();
            let found = landing_grids(
                &plan.frame,
                &elevation.walkable,
                end.point,
                other.point,
                &exclude,
                plan.input.options.door_reach.max(2),
            );
            let elevation = &mut plans[end.map].elevations[usize::from(end.elevation)];
            match found {
                Some((grid, steps)) => elevation.stair_grids.push((s, grid, steps)),
                None => elevation.skipped.push(SkippedTeleport {
                    door: format!("stairs {}", stair.label),
                    reason: format!(
                        "no walkable hex at its landing ({:.0}, {:.0}, {:.0}) on elevation {} \
                         within {} steps",
                        end.point[0],
                        end.point[1],
                        end.point[2],
                        end.elevation,
                        plans[end.map].input.options.door_reach.max(2)
                    ),
                }),
            }
        }
    }
    // Stub exits, their returns avoiding every grid of the elevation.
    for plan in plans.iter_mut() {
        for elevation in &mut plan.elevations {
            let grids = elevation.candidate_grids();
            let mut skipped = std::mem::take(&mut elevation.skipped);
            elevation.door_exits = exits::stub_exits(
                &plan.frame,
                &elevation.walkable,
                &elevation.doors,
                &grids,
                &mut skipped,
            );
            elevation.skipped = skipped;
        }
    }
    let any_link = plans.iter().any(|p| {
        p.elevations
            .iter()
            .any(|e| e.doors.iter().any(|d| d.kind == DoorKind::Link))
    });
    if !any_link && layer.stairs.is_empty() {
        return Ok(());
    }
    // Per elevation: its grids and components (before passages).
    let mut grids: BTreeMap<(usize, usize), BTreeSet<i32>> = BTreeMap::new();
    let mut components: BTreeMap<(usize, usize), Components> = BTreeMap::new();
    for (m, plan) in plans.iter().enumerate() {
        for (e, elevation) in plan.elevations.iter().enumerate() {
            grids.insert((m, e), elevation.candidate_grids());
            let walkable = &elevation.walkable;
            components.insert((m, e), islands::components(|h| walkable[h as usize]));
        }
    }
    let mut doors_at: BTreeMap<String, Vec<DoorAt>> = BTreeMap::new();
    for (m, plan) in plans.iter().enumerate() {
        for (e, elevation) in plan.elevations.iter().enumerate() {
            for placement in elevation.placements.iter() {
                if placement.role == crate::Role::Door {
                    doors_at
                        .entry(placement.reference.clone())
                        .or_default()
                        .push((m, e, placement.hex));
                }
            }
        }
    }
    let arrival = |m: usize, e: usize| Arrival {
        frame: &plans[m].frame,
        walkable: &plans[m].elevations[e].walkable,
        components: &components[&(m, e)],
        grids: &grids[&(m, e)],
    };
    // Door links.
    let mut results: Vec<(usize, usize, Result<PlannedLink, SkippedTeleport>)> = Vec::new();
    for (m, plan) in plans.iter().enumerate() {
        for (e, elevation) in plan.elevations.iter().enumerate() {
            for door in elevation.doors.iter().filter(|d| d.kind == DoorKind::Link) {
                let skip = |reason: String| SkippedTeleport {
                    door: door.door.clone(),
                    reason,
                };
                let cell = door.cell.form.to_string();
                let owners = index.cells.get(&cell).cloned().unwrap_or_default();
                let point = door.teleport.position;
                let partner = doors_at
                    .get(&door.teleport.door.to_string())
                    .and_then(|places| places.iter().find(|(pm, _, _)| owners.contains(pm)))
                    .copied();
                let destination = match partner {
                    Some((pm, pe, _)) => Some((pm, pe)),
                    None => owners
                        .iter()
                        .filter_map(|om| {
                            elevation_for(&plans[*om], point).map(|(oe, d)| (d, *om, oe))
                        })
                        .min_by(|a, b| a.0.total_cmp(&b.0).then((a.1, a.2).cmp(&(b.1, b.2))))
                        .map(|(_, om, oe)| (om, oe)),
                };
                let Some((dm, de)) = destination else {
                    results.push((
                        m,
                        e,
                        Err(skip(format!(
                            "its arrival point ({:.0}, {:.0}) lies outside every map built from \
                             {cell}",
                            point[0], point[1]
                        ))),
                    ));
                    continue;
                };
                let target_plan = &plans[dm];
                let finite = point.iter().all(|v| v.is_finite());
                let (around, keep_off, prefer) = match partner {
                    Some((_, _, hex)) => {
                        let partner_grids: Vec<i32> = target_plan.elevations[de]
                            .doors
                            .iter()
                            .filter(|d| d.door_hex == hex)
                            .flat_map(|d| d.grid.iter().copied())
                            .collect();
                        (Some(hex), vec![hex], partner_grids)
                    }
                    None => (
                        finite
                            .then(|| {
                                target_plan
                                    .frame
                                    .hex_of_source(f64::from(point[0]), f64::from(point[1]))
                            })
                            .flatten(),
                        Vec::new(),
                        Vec::new(),
                    ),
                };
                let target = if finite {
                    point
                } else {
                    match around.and_then(|h| target_plan.frame.hex_centre_tile(h)) {
                        Some(c) => {
                            let p = target_plan.frame.source_of_tile(c);
                            [p[0] as f32, p[1] as f32, 0.0]
                        }
                        None => [0.0; 3],
                    }
                };
                let found = around
                    .and_then(|around| arrival(dm, de).find(around, &keep_off, target, &prefer));
                let Some(hex) = found else {
                    results.push((
                        m,
                        e,
                        Err(skip(format!(
                            "no walkable hex for the arrival on {} elevation {de}{}",
                            target_plan.stem,
                            match partner {
                                Some((_, _, hex)) => format!(
                                    " 2 to {RETURN_RADIUS} steps from the partner door on hex \
                                     {hex}"
                                ),
                                None =>
                                    format!(" within {RETURN_RADIUS} steps of its arrival point"),
                            }
                        ))),
                    ));
                    continue;
                };
                let orientation = match partner {
                    Some((_, _, partner_hex)) => orientation_towards(partner_hex, hex),
                    None => target_plan
                        .frame
                        .orientation_of_yaw(door.teleport.rotation[2])
                        .unwrap_or(0),
                };
                results.push((
                    m,
                    e,
                    Ok(PlannedLink {
                        kind: LinkKind::Door,
                        from: door.door.clone(),
                        door_hex: Some(door.door_hex),
                        grid: door.grid.clone(),
                        steps: door.steps,
                        to: (dm, de as u8, hex, orientation),
                    }),
                ));
            }
        }
    }
    // Stairs links.
    for (s, stair) in layer.stairs.iter().enumerate() {
        for i in 0..2 {
            let from = stair.ends[i];
            let to = stair.ends[1 - i];
            let (fm, fe) = (from.map, usize::from(from.elevation));
            let (tm, te) = (to.map, usize::from(to.elevation));
            let Some((_, grid, steps)) = plans[fm].elevations[fe]
                .stair_grids
                .iter()
                .find(|g| g.0 == s)
                .cloned()
            else {
                continue;
            };
            let skip = |reason: String| SkippedTeleport {
                door: format!("stairs {}", stair.label),
                reason,
            };
            let own: Vec<i32> = plans[tm].elevations[te]
                .stair_grids
                .iter()
                .find(|g| g.0 == s)
                .map(|g| g.1.clone())
                .unwrap_or_default();
            let landing = plans[tm]
                .frame
                .hex_of_source(f64::from(to.point[0]), f64::from(to.point[1]));
            let found = landing.and_then(|around| {
                arrival(tm, te)
                    .find(around, &own, to.point, &own)
                    .map(|hex| (hex, around))
            });
            results.push((
                fm,
                fe,
                match found {
                    Some((hex, around)) => Ok(PlannedLink {
                        kind: LinkKind::Stairs,
                        from: stair.label.clone(),
                        door_hex: None,
                        grid,
                        steps,
                        to: (tm, te as u8, hex, orientation_towards(around, hex)),
                    }),
                    None => Err(skip(format!(
                        "no walkable hex for the arrival on {} elevation {te} within \
                         {RETURN_RADIUS} steps of its landing ({:.0}, {:.0}, {:.0}), 2 or more \
                         from its grids",
                        plans[tm].stem, to.point[0], to.point[1], to.point[2]
                    ))),
                },
            ));
        }
    }
    for (m, e, result) in results {
        match result {
            Ok(link) => {
                let (dm, de, hex, _) = link.to;
                let what = match link.kind {
                    LinkKind::Door => format!("arrival of {}", link.from),
                    LinkKind::Stairs => format!("arrival of stairs {}", link.from),
                };
                plans[dm].elevations[usize::from(de)]
                    .arrivals
                    .push((hex, what));
                plans[m].elevations[e].links.push(link);
            }
            Err(skip) => plans[m].elevations[e].skipped.push(skip),
        }
    }
    Ok(())
}

/// Check the stairs' ends against the maps (indices, elevations).
pub(crate) fn validate_stairs(
    stairs: &[StairLink],
    maps: &[crate::MapInput],
) -> Result<(), crate::BuildError> {
    for stair in stairs {
        for end in &stair.ends {
            let Some(map) = maps.get(end.map) else {
                return Err(crate::BuildError::Input(format!(
                    "stairs {} name map {}; the layer has {}",
                    stair.label,
                    end.map,
                    maps.len()
                )));
            };
            if usize::from(end.elevation) >= map.elevations.len()
                || !end.point.iter().all(|v| v.is_finite())
            {
                return Err(crate::BuildError::Input(format!(
                    "stairs {} name elevation {} of map {} (it has {}) or a point that is not \
                     finite",
                    stair.label,
                    end.elevation,
                    map.map_name,
                    map.elevations.len()
                )));
            }
        }
        if stair.ends[0].map == stair.ends[1].map
            && stair.ends[0].elevation == stair.ends[1].elevation
        {
            return Err(crate::BuildError::Input(format!(
                "stairs {} join an elevation to itself",
                stair.label
            )));
        }
    }
    Ok(())
}
