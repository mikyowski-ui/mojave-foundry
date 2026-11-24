//! Doors that teleport into interiors, and the exits that replace the
//! teleport.
//!
//! **Footprint.** For a door on hex `D`, every walkable neighbour of `D`
//! (lattice distance 1) holds an exit grid into the interior's stub map:
//! a walker reaches the door only through them.
//!
//! **A door with no walkable neighbour** (its doorstep inside a blocked
//! footprint) gets a doorstep further out instead of being skipped: the
//! nearest walkable hexes 2 to `reach` steps away
//! ([`crate::BuildOptions::door_reach`]) on the door's **outer side**. The
//! outer side is the half plane, through the door's centre, facing the
//! interior door's own teleport destination (where a traveller arrives
//! outside); without that destination every direction counts. On the
//! nearest ring that has such a hex, the doorstep is the one nearest the
//! outward ray from the door (nearest the door when there is no ray; ties
//! to the lower hex) plus its neighbours on that ring that are walkable
//! and on the outer side too: one to three exit grids.
//! [`DoorExit::grid_steps`] says how far out they are.
//!
//! The player comes back out
//! on the **return hex**: the walkable hex nearest the interior door's own
//! teleport destination (its arrival point outside), among hexes 2 to
//! [`RETURN_RADIUS`] steps from `D` that hold no exit grid, so that
//! arriving never re-enters; without that teleport, the nearest such hex
//! to the door. The return orientation faces away from the door.
//!
//! **Stub map.** One per door, so that an interior with several doors
//! lets the player out at the one they came in by: the entrance is the
//! grid's centre hex [`STUB_ENTRY`], floor tiles cover every hex within
//! [`STUB_FLOOR_RADIUS`] steps of it, and every hex exactly
//! [`STUB_EXIT_RADIUS`] steps away holds an exit grid back to the exterior
//! map at that door's return hex, so any walk away from the entrance
//! leaves. Stubs are numbered in door order (reference form key).

use std::collections::{BTreeMap, BTreeSet};

use mir::{CellDoc, CellLocation, Reference};
use vault_formats::HexGrid;

use crate::walk::{Frame, HEXES, Verdict, neighbours};
use crate::{Placement, Role, SkippedTeleport};

/// How far from the door the return hex may be.
pub const RETURN_RADIUS: i32 = 10;
/// The stub map's entrance: the middle of the grid.
pub const STUB_ENTRY: i32 = 100 * 200 + 100;
/// Stub floor and exit ring radii, in hex steps from the entrance.
pub const STUB_FLOOR_RADIUS: i32 = 4;
pub const STUB_EXIT_RADIUS: i32 = 3;

/// One door that becomes an exit.
#[derive(Debug, Clone, PartialEq)]
pub struct DoorExit {
    /// Index into the placements.
    pub placement: usize,
    pub door: String,
    pub door_hex: i32,
    /// The interior cell's form key and editor id.
    pub interior: String,
    pub interior_editor_id: Option<String>,
    pub grid_hexes: Vec<i32>,
    /// Hex steps from the door to its exit grids: 1 when they stand next
    /// to it, more for a doorstep found further out.
    pub grid_steps: i32,
    pub return_hex: i32,
    pub return_orientation: u8,
    pub return_from_teleport: bool,
}

/// The default of [`crate::BuildOptions::door_reach`]: the doorsteps
/// measured on the dense maps lay 2 to 7 steps from their doors.
pub const DOOR_REACH: i32 = 8;

/// The doorstep of a door on `door_hex` with no walkable neighbour (module
/// notes): its hexes, sorted, and their distance from the door.
fn far_doorstep(
    frame: &Frame,
    door_hex: i32,
    outward: Option<[f64; 2]>,
    walkable: &[bool],
    reach: i32,
) -> Option<(Vec<i32>, i32)> {
    let door = frame.hex_centre_tile(door_hex)?;
    let offset = |hex: i32| {
        frame
            .hex_centre_tile(hex)
            .map(|c| [c[0] - door[0], c[1] - door[1]])
    };
    let outer = |hex: i32| match (outward, offset(hex)) {
        (Some(u), Some(d)) => d[0] * u[0] + d[1] * u[1] > 0.0,
        (None, Some(_)) => true,
        (_, None) => false,
    };
    for steps in 2..=reach {
        let ring: Vec<i32> = hexes_around(door_hex, steps, true)
            .into_iter()
            .filter(|hex| walkable.get(*hex as usize).copied().unwrap_or(false) && outer(*hex))
            .collect();
        // The hex nearest the outward ray (largest cosine), or without a
        // ray the nearest the door.
        let score = |hex: i32| -> f64 {
            let d = offset(hex).unwrap_or([0.0; 2]);
            let length = d[0].hypot(d[1]).max(1e-9);
            match outward {
                Some(u) => -(d[0] * u[0] + d[1] * u[1]) / length,
                None => length,
            }
        };
        let Some(&first) = ring
            .iter()
            .min_by(|a, b| score(**a).total_cmp(&score(**b)).then(a.cmp(b)))
        else {
            continue;
        };
        let mut step: Vec<i32> = ring
            .iter()
            .copied()
            .filter(|hex| *hex == first || HexGrid::lattice_distance(*hex, first) == Ok(1))
            .collect();
        step.sort_unstable();
        return Some((step, steps));
    }
    None
}

/// The hexes `radius` steps or fewer from `centre` (exactly `radius` when
/// `ring`), sorted.
pub fn hexes_around(centre: i32, radius: i32, ring: bool) -> Vec<i32> {
    let Ok([c, r]) = HexGrid::coordinates(centre) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for row in (r - 2 * radius - 1).max(0)..=(r + 2 * radius + 1).min(199) {
        for column in (c - radius - 1).max(0)..=(c + radius + 1).min(199) {
            let hex = column + 200 * row;
            let Ok(d) = HexGrid::lattice_distance(centre, hex) else {
                continue;
            };
            if (ring && d == radius) || (!ring && d <= radius) {
                out.push(hex);
            }
        }
    }
    out.sort_unstable();
    out
}

/// The orientation (0 to 5) of the neighbour of `from` nearest `to`.
pub fn orientation_towards(from: i32, to: i32) -> u8 {
    (0..6u8)
        .filter_map(|o| {
            let next = HexGrid::neighbor(from, o).ok().flatten()?;
            Some((HexGrid::lattice_distance(next, to).ok()?, o))
        })
        .min()
        .map_or(0, |(_, o)| o)
}

/// Why a door has no walkable neighbour: its neighbours by verdict, and
/// how far the nearest walkable hex is.
fn surroundings(door_hex: i32, verdicts: &[Verdict]) -> String {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for hex in neighbours(door_hex) {
        let label = match verdicts.get(hex as usize).copied().unwrap_or_default() {
            Verdict::Walkable => "walkable",
            Verdict::OffNavmesh => "off the navmesh",
            Verdict::TooSteep => "too steep",
            Verdict::NoGround => "without ground",
            Verdict::Blocked => "blocked",
            Verdict::OffGrid => "off the grid",
        };
        *counts.entry(label).or_default() += 1;
    }
    let around: Vec<String> = counts
        .iter()
        .map(|(label, count)| format!("{count} {label}"))
        .collect();
    let nearest = (2..=RETURN_RADIUS).find(|radius| {
        hexes_around(door_hex, *radius, true)
            .iter()
            .any(|hex| verdicts.get(*hex as usize) == Some(&Verdict::Walkable))
    });
    match nearest {
        Some(steps) => format!(
            "neighbours: {}; nearest walkable hex {steps} steps away",
            around.join(", ")
        ),
        None => format!(
            "neighbours: {}; no walkable hex within {RETURN_RADIUS} steps",
            around.join(", ")
        ),
    }
}

/// What a teleport door becomes, decided before its grids are searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DoorKind {
    /// Exit grids into a stub map of its own (a destination outside the
    /// build).
    Stub,
    /// Exit grids into a map of the layer (crate notes, "Door links").
    Link,
}

/// A teleport door's exit grids, before its destination hex is chosen.
#[derive(Debug, Clone)]
pub(crate) struct DoorGrid {
    /// Index into the placements.
    pub placement: usize,
    pub door: String,
    pub door_hex: i32,
    pub teleport: mir::TeleportDoc,
    pub cell: mir::CellRef,
    pub grid: Vec<i32>,
    pub steps: i32,
    /// Where a traveller coming the other way arrives here: the partner
    /// door's own teleport position, in this map's source coordinates.
    pub back: Option<[f32; 3]>,
    pub kind: DoorKind,
}

/// The exit grids of every teleporting door among `placements` (module
/// notes): `classify` decides, from the destination cell, what each door
/// becomes or why it is skipped; `back` finds the partner door's arrival
/// point here. Skips come in placement order; the count of door
/// placements whose reference the cells do not hold is returned too.
pub(crate) fn door_grids(
    frame: &Frame,
    placements: &[Placement],
    cells: &[CellDoc],
    verdicts: &[Verdict],
    reach: i32,
    classify: impl Fn(&Placement, &mir::CellRef) -> Result<DoorKind, String>,
    back: impl Fn(&mir::TeleportDoc, &mir::CellRef) -> Option<[f32; 3]>,
) -> (Vec<DoorGrid>, Vec<SkippedTeleport>, usize) {
    let walkable: Vec<bool> = verdicts.iter().map(|v| *v == Verdict::Walkable).collect();
    let mut references: BTreeMap<String, &Reference> = BTreeMap::new();
    for cell in cells {
        for reference in &cell.references {
            references
                .entry(reference.form.to_string())
                .or_insert(reference);
        }
    }
    let mut skipped = Vec::new();
    let mut unknown = 0;
    let mut doors = Vec::new();
    for (index, placement) in placements.iter().enumerate() {
        if placement.role != Role::Door {
            continue;
        }
        let Some(reference) = references.get(&placement.reference) else {
            unknown += 1;
            continue;
        };
        let Some(teleport) = &reference.teleport else {
            continue;
        };
        let Some(cell) = &teleport.cell else {
            skipped.push(SkippedTeleport {
                door: placement.reference.clone(),
                reason: "the destination door is not in the plugin list".into(),
            });
            continue;
        };
        let kind = match classify(placement, cell) {
            Ok(kind) => kind,
            Err(reason) => {
                skipped.push(SkippedTeleport {
                    door: placement.reference.clone(),
                    reason,
                });
                continue;
            }
        };
        let back = back(teleport, cell);
        let mut grid: Vec<i32> = neighbours(placement.hex)
            .filter(|hex| walkable[*hex as usize])
            .collect();
        grid.sort_unstable();
        let mut steps = 1;
        if grid.is_empty() {
            let outward = back
                .filter(|p| p.iter().all(|v| v.is_finite()))
                .and_then(|p| {
                    let to = frame.tile_of_source(f64::from(p[0]), f64::from(p[1]));
                    let from = frame.hex_centre_tile(placement.hex)?;
                    let d = [to[0] - from[0], to[1] - from[1]];
                    let length = d[0].hypot(d[1]);
                    (length > 1e-6).then(|| [d[0] / length, d[1] / length])
                });
            match far_doorstep(frame, placement.hex, outward, &walkable, reach) {
                Some((far, at)) => {
                    grid = far;
                    steps = at;
                }
                None => {
                    skipped.push(SkippedTeleport {
                        door: placement.reference.clone(),
                        reason: format!(
                            "no walkable hex next to the door on hex {} nor on its outer side \
                             within {} steps ({})",
                            placement.hex,
                            reach.max(1),
                            surroundings(placement.hex, verdicts)
                        ),
                    });
                    continue;
                }
            }
        }
        doors.push(DoorGrid {
            placement: index,
            door: placement.reference.clone(),
            door_hex: placement.hex,
            teleport: teleport.clone(),
            cell: cell.clone(),
            grid,
            steps,
            back,
            kind,
        });
    }
    (doors, skipped, unknown)
}

/// The return hex of a door on `door_hex` (module notes): the walkable hex
/// nearest `back` (else the door), 2 to [`RETURN_RADIUS`] steps from the
/// door, holding no grid of `grids`; and whether `back` gave the target.
/// `None` when no hex qualifies (or the door's hex has no centre, which
/// never happens on the grid).
pub(crate) fn return_hex(
    frame: &Frame,
    walkable: &[bool],
    door_hex: i32,
    back: Option<[f32; 3]>,
    grids: &BTreeSet<i32>,
) -> Option<Result<(i32, bool), ()>> {
    let target = match back {
        Some(p) if p.iter().all(|v| v.is_finite()) => {
            Some(frame.tile_of_source(f64::from(p[0]), f64::from(p[1])))
        }
        _ => None,
    };
    let from_teleport = target.is_some();
    let target = target.or_else(|| frame.hex_centre_tile(door_hex))?;
    let mut best: Option<(f64, i32)> = None;
    for hex in hexes_around(door_hex, RETURN_RADIUS, false) {
        if hex as usize >= HEXES
            || !walkable[hex as usize]
            || grids.contains(&hex)
            || HexGrid::lattice_distance(door_hex, hex).unwrap_or(0) < 2
        {
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
    Some(best.map(|(_, hex)| (hex, from_teleport)).ok_or(()))
}

/// The reason a door with no return hex is skipped.
pub(crate) fn no_return_reason() -> String {
    format!("no walkable hex 2 to {RETURN_RADIUS} steps from the door to return to")
}

/// The interior door's own teleport (`teleport.door` in the interior cell
/// `cell` among `cells`): where a traveller arrives outside.
pub(crate) fn interior_back(
    cells: &[CellDoc],
    teleport: &mir::TeleportDoc,
    cell: &mir::CellRef,
) -> Option<[f32; 3]> {
    cells
        .iter()
        .filter(|c| c.is_interior() && c.form == cell.form)
        .find_map(|c| c.reference(&teleport.door))
        .and_then(|door| door.teleport.as_ref())
        .map(|t| t.position)
}

/// Turn door grids of kind [`DoorKind::Stub`] into exits with their return
/// hexes, `grids` being every grid hex of the elevation; skips (no return
/// hex) are appended to `skipped` in door order.
pub(crate) fn stub_exits(
    frame: &Frame,
    walkable: &[bool],
    doors: &[DoorGrid],
    grids: &BTreeSet<i32>,
    skipped: &mut Vec<SkippedTeleport>,
) -> Vec<DoorExit> {
    let mut exits = Vec::new();
    for door in doors.iter().filter(|d| d.kind == DoorKind::Stub) {
        let Some(found) = return_hex(frame, walkable, door.door_hex, door.back, grids) else {
            continue;
        };
        let Ok((return_hex, from_teleport)) = found else {
            skipped.push(SkippedTeleport {
                door: door.door.clone(),
                reason: no_return_reason(),
            });
            continue;
        };
        exits.push(DoorExit {
            placement: door.placement,
            door: door.door.clone(),
            door_hex: door.door_hex,
            interior: door.cell.form.to_string(),
            interior_editor_id: door.cell.editor_id.clone(),
            grid_hexes: door.grid.clone(),
            grid_steps: door.steps,
            return_hex,
            return_orientation: orientation_towards(door.door_hex, return_hex),
            return_from_teleport: from_teleport,
        });
    }
    exits
}

/// Plan the exits of every teleporting door among `placements`; a door
/// with no walkable neighbour looks for a doorstep up to `reach` steps
/// out (module notes; below 2, it is skipped as before).
pub fn plan(
    frame: &Frame,
    placements: &[Placement],
    cells: &[CellDoc],
    verdicts: &[Verdict],
    reach: i32,
) -> (Vec<DoorExit>, Vec<SkippedTeleport>, usize) {
    let (doors, mut skipped, unknown) = door_grids(
        frame,
        placements,
        cells,
        verdicts,
        reach,
        |_, cell| {
            if cell.location == CellLocation::Interior {
                Ok(DoorKind::Stub)
            } else {
                Err(format!(
                    "a teleport to another exterior position ({}); not converted in this pass",
                    cell.form
                ))
            }
        },
        |teleport, cell| interior_back(cells, teleport, cell),
    );
    let walkable: Vec<bool> = verdicts.iter().map(|v| *v == Verdict::Walkable).collect();
    let all_grids: BTreeSet<i32> = doors.iter().flat_map(|d| d.grid.iter().copied()).collect();
    let exits = stub_exits(frame, &walkable, &doors, &all_grids, &mut skipped);
    (exits, skipped, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_and_discs() {
        assert_eq!(hexes_around(STUB_ENTRY, 0, false), vec![STUB_ENTRY]);
        assert_eq!(hexes_around(STUB_ENTRY, 1, true).len(), 6);
        assert_eq!(hexes_around(STUB_ENTRY, 3, true).len(), 18);
        assert_eq!(hexes_around(STUB_ENTRY, 4, false).len(), 61);
        let ring: Vec<i32> = {
            let mut n: Vec<i32> = neighbours(STUB_ENTRY).collect();
            n.sort_unstable();
            n
        };
        assert_eq!(hexes_around(STUB_ENTRY, 1, true), ring);
    }

    #[test]
    fn orientation_points_at_the_target() {
        for o in 0..6u8 {
            let next = HexGrid::neighbor(STUB_ENTRY, o).unwrap().unwrap();
            let far = HexGrid::step(STUB_ENTRY, o, 4).unwrap();
            assert_eq!(orientation_towards(STUB_ENTRY, next), o);
            assert_eq!(orientation_towards(STUB_ENTRY, far), o);
        }
    }
}
