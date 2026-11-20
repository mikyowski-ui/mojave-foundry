//! Doors that teleport into interiors, and the exits that replace the
//! teleport.
//!
//! **Footprint.** For a door on hex `D`, every walkable neighbour of `D`
//! (lattice distance 1) holds an exit grid into the interior's stub map:
//! a walker reaches the door only through them. The player comes back out
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
    pub return_hex: i32,
    pub return_orientation: u8,
    pub return_from_teleport: bool,
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

/// Plan the exits of every teleporting door among `placements`.
pub fn plan(
    frame: &Frame,
    placements: &[Placement],
    cells: &[CellDoc],
    verdicts: &[Verdict],
) -> (Vec<DoorExit>, Vec<SkippedTeleport>, usize) {
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
        if cell.location != CellLocation::Interior {
            skipped.push(SkippedTeleport {
                door: placement.reference.clone(),
                reason: format!(
                    "a teleport to another exterior position ({}); not converted in this pass",
                    cell.form
                ),
            });
            continue;
        }
        let grid: Vec<i32> = {
            let mut grid: Vec<i32> = neighbours(placement.hex)
                .filter(|hex| walkable[*hex as usize])
                .collect();
            grid.sort_unstable();
            grid
        };
        if grid.is_empty() {
            skipped.push(SkippedTeleport {
                door: placement.reference.clone(),
                reason: format!(
                    "no walkable hex next to the door on hex {} ({})",
                    placement.hex,
                    surroundings(placement.hex, verdicts)
                ),
            });
            continue;
        }
        // The interior door's own teleport is where a traveller arrives
        // outside.
        let back = cells
            .iter()
            .filter(|c| c.is_interior() && c.form == cell.form)
            .find_map(|c| c.reference(&teleport.door))
            .and_then(|door| door.teleport.as_ref())
            .map(|t| t.position);
        doors.push((index, reference, teleport, cell, grid, back));
    }
    let all_grids: BTreeSet<i32> = doors.iter().flat_map(|d| d.4.iter().copied()).collect();
    let mut exits = Vec::new();
    for (index, _reference, _teleport, cell, grid, back) in doors {
        let placement = &placements[index];
        let door_hex = placement.hex;
        let target = match back {
            Some(p) if p.iter().all(|v| v.is_finite()) => {
                Some(frame.tile_of_source(f64::from(p[0]), f64::from(p[1])))
            }
            _ => None,
        };
        let from_teleport = target.is_some();
        let target = target.or_else(|| frame.hex_centre_tile(door_hex));
        let Some(target) = target else {
            continue;
        };
        let mut best: Option<(f64, i32)> = None;
        for hex in hexes_around(door_hex, RETURN_RADIUS, false) {
            if hex as usize >= HEXES
                || !walkable[hex as usize]
                || all_grids.contains(&hex)
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
        let Some((_, return_hex)) = best else {
            skipped.push(SkippedTeleport {
                door: placement.reference.clone(),
                reason: format!(
                    "no walkable hex 2 to {RETURN_RADIUS} steps from the door to return to"
                ),
            });
            continue;
        };
        exits.push(DoorExit {
            placement: index,
            door: placement.reference.clone(),
            door_hex,
            interior: cell.form.to_string(),
            interior_editor_id: cell.editor_id.clone(),
            grid_hexes: grid,
            return_hex,
            return_orientation: orientation_towards(door_hex, return_hex),
            return_from_teleport: from_teleport,
        });
    }
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
