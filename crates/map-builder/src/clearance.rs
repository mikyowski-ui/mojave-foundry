//! Keep-clear hexes of links (crate notes, "Link clearance").
//!
//! A stair or door between maps or elevations is a walk-over exit grid, so
//! a walker that steps on one is carried away. A ground item, a container
//! or a critter standing on a grid (or beside a stair) draws walkers onto
//! it: an autoplay bot that collects items first walks onto the grid and
//! leaves the map. This module decides which hexes of an elevation stay
//! clear and moves what was placed there.
//!
//! The keep-clear set of an elevation is, over its [`crate::LinkKind::Door`]
//! and [`crate::LinkKind::Stairs`] links:
//!
//! - every exit grid hex of a link, and every arrival hex another link
//!   leads to here (radius [`DOOR_KEEP_CLEAR_RADIUS`] for doors, whose mats
//!   legitimately have clutter nearby);
//! - every hex within [`STAIRS_KEEP_CLEAR_RADIUS`] steps of a stair grid
//!   hex.
//!
//! Stub exits (doors leading outside the build) are not links and keep
//! nothing clear, so a layer without links is written byte for byte as
//! before.
//!
//! [`move_off_links`] runs after the walkable grids are final (passages
//! opened, unreached components closed) and before critters and loot are
//! placed: every ground item, and every container that does not block its
//! hex, in the set moves to the nearest free walkable hex outside it, in
//! the same walkable component when one has room, ties to the lowest hex.
//! A container that blocks movement is left where it is: moving it would
//! change the walkable grid the plan is built on, and it cannot stand on a
//! grid hex (those are walkable). Critters are placed clear of the whole
//! set ([`elevation_keep_clear`]).

use std::collections::BTreeSet;

use crate::exits::hexes_around;
use crate::islands::{self, Components, on_border};
use crate::plan::{ElevationPlan, MapPlan, effective_blocking};
use crate::walk::HEXES;
use crate::{LinkKind, Role};

/// Steps around a stair's exit grid hexes that stay clear of loot, items
/// and critters.
pub const STAIRS_KEEP_CLEAR_RADIUS: i32 = 1;

/// Steps around a door link's exit grid hexes and arrival hexes that stay
/// clear: none beyond the hexes themselves.
pub const DOOR_KEEP_CLEAR_RADIUS: i32 = 0;

/// The farthest ring searched for a free hex.
const SEARCH_RADIUS: i32 = 199;

/// The keep-clear hexes of links: `grids` are the exit grid hexes of each
/// link by kind, `arrivals` the hexes other links lead to.
pub fn keep_clear<'a>(
    grids: impl IntoIterator<Item = (LinkKind, &'a [i32])>,
    arrivals: impl IntoIterator<Item = i32>,
) -> BTreeSet<i32> {
    let mut set = BTreeSet::new();
    for (kind, hexes) in grids {
        let radius = match kind {
            LinkKind::Door => DOOR_KEEP_CLEAR_RADIUS,
            LinkKind::Stairs => STAIRS_KEEP_CLEAR_RADIUS,
        };
        for hex in hexes {
            if radius == 0 {
                set.insert(*hex);
            } else {
                set.extend(hexes_around(*hex, radius, false));
            }
        }
    }
    set.extend(arrivals);
    set
}

/// The keep-clear hexes of one planned elevation.
pub(crate) fn elevation_keep_clear(elevation: &ElevationPlan) -> BTreeSet<i32> {
    keep_clear(
        elevation
            .links
            .iter()
            .map(|link| (link.kind, link.grid.as_slice())),
        elevation.arrivals.iter().map(|(hex, _)| *hex),
    )
}

/// Whether a placement is moved off the set: ground items, and containers
/// that do not block their hex (module notes).
fn movable(elevation: &ElevationPlan, index: usize) -> bool {
    let placement = &elevation.placements[index];
    match placement.role {
        Role::Item => true,
        Role::Container => {
            !effective_blocking(placement.role, placement.blocking)
                && !elevation.footprints.hexes.contains(&placement.hex)
        }
        _ => false,
    }
}

/// The nearest hex to `from` that `free` accepts, ring by ring, the lowest
/// hex of the nearest ring; in component `home` when given.
fn nearest(
    from: i32,
    comps: &Components,
    home: Option<usize>,
    free: impl Fn(i32) -> bool,
) -> Option<i32> {
    (1..=SEARCH_RADIUS).find_map(|radius| {
        hexes_around(from, radius, true)
            .into_iter()
            .find(|hex| free(*hex) && home.is_none_or(|id| comps.of(*hex) == Some(id)))
    })
}

/// Move every ground item and non-blocking container of the elevations of
/// `plan` that stands in the keep-clear set to the nearest free walkable
/// hex outside it (module notes). Counts them in
/// [`ElevationPlan::moved_off_links`].
pub(crate) fn move_off_links(plan: &mut MapPlan) {
    for elevation in &mut plan.elevations {
        let clear = elevation_keep_clear(elevation);
        if clear.is_empty() {
            continue;
        }
        let stuck: Vec<usize> = (0..elevation.placements.len())
            .filter(|i| clear.contains(&elevation.placements[*i].hex) && movable(elevation, *i))
            .collect();
        if stuck.is_empty() {
            continue;
        }
        let walkable = &elevation.walkable;
        let comps = islands::components(|h| walkable[h as usize]);
        let mut occupied: BTreeSet<i32> = elevation.placements.iter().map(|p| p.hex).collect();
        occupied.extend(&elevation.door_hexes);
        occupied.extend(elevation.grid_hexes());
        occupied.extend(elevation.arrival_hexes());
        occupied.extend(elevation.entrance.map(|e| e.hex));
        let mut moves: Vec<(usize, i32)> = Vec::new();
        for index in stuck {
            let from = elevation.placements[index].hex;
            let usable = |hex: i32, occupied: &BTreeSet<i32>| {
                hex > 0
                    && (hex as usize) < HEXES
                    && walkable[hex as usize]
                    && !on_border(hex)
                    && !clear.contains(&hex)
                    && !occupied.contains(&hex)
            };
            // The item's own component; one standing on ground nobody can
            // walk takes the component of the nearest walkable hex.
            let home = comps.of(from).or_else(|| {
                nearest(from, &comps, None, |h| {
                    h > 0 && (h as usize) < HEXES && walkable[h as usize]
                })
                .and_then(|h| comps.of(h))
            });
            let to = nearest(from, &comps, home, |h| usable(h, &occupied))
                .or_else(|| nearest(from, &comps, None, |h| usable(h, &occupied)));
            if let Some(to) = to {
                occupied.insert(to);
                moves.push((index, to));
            }
        }
        if moves.is_empty() {
            continue;
        }
        let containers = moves
            .iter()
            .any(|(i, _)| elevation.placements[*i].role == Role::Container);
        let list = elevation.placements.to_mut();
        for (index, to) in &moves {
            list[*index].hex = *to;
        }
        elevation.moved_off_links = moves.len();
        if containers {
            elevation.cover = crate::cover::Cover::new(elevation.input.art, &elevation.placements);
        }
    }
}

/// [`move_off_links`] for the hidden placements ([`crate::BuildOptions::hidden`]):
/// a hidden item or container (blocking or not: hidden objects block
/// nothing) that stands in the keep-clear set moves to the nearest free
/// walkable hex outside it, as a visible one does, because the layer's
/// check ([`crate::verify_clearance`]) reads every written object. It
/// avoids the hexes of visible placements, doors, grids, arrivals and the
/// entrance, and those other hidden placements already took. Not counted
/// in [`ElevationPlan::moved_off_links`].
pub(crate) fn move_hidden_off_links(plan: &mut MapPlan) {
    for elevation in &mut plan.elevations {
        if elevation.hidden_placements.is_empty() {
            continue;
        }
        let clear = elevation_keep_clear(elevation);
        if clear.is_empty() {
            continue;
        }
        let stuck: Vec<usize> = (0..elevation.hidden_placements.len())
            .filter(|i| {
                let p = &elevation.hidden_placements[*i];
                clear.contains(&p.hex) && matches!(p.role, Role::Item | Role::Container)
            })
            .collect();
        if stuck.is_empty() {
            continue;
        }
        let walkable = &elevation.walkable;
        let comps = islands::components(|h| walkable[h as usize]);
        let mut occupied: BTreeSet<i32> = elevation.placements.iter().map(|p| p.hex).collect();
        occupied.extend(elevation.hidden_placements.iter().map(|p| p.hex));
        occupied.extend(&elevation.door_hexes);
        occupied.extend(elevation.grid_hexes());
        occupied.extend(elevation.arrival_hexes());
        occupied.extend(elevation.entrance.map(|e| e.hex));
        let mut moves: Vec<(usize, i32)> = Vec::new();
        for index in stuck {
            let from = elevation.hidden_placements[index].hex;
            let usable = |hex: i32, occupied: &BTreeSet<i32>| {
                hex > 0
                    && (hex as usize) < HEXES
                    && walkable[hex as usize]
                    && !on_border(hex)
                    && !clear.contains(&hex)
                    && !occupied.contains(&hex)
            };
            let home = comps.of(from).or_else(|| {
                nearest(from, &comps, None, |h| {
                    h > 0 && (h as usize) < HEXES && walkable[h as usize]
                })
                .and_then(|h| comps.of(h))
            });
            let to = nearest(from, &comps, home, |h| usable(h, &occupied))
                .or_else(|| nearest(from, &comps, None, |h| usable(h, &occupied)));
            if let Some(to) = to {
                occupied.insert(to);
                moves.push((index, to));
            }
        }
        for (index, to) in moves {
            elevation.hidden_placements[index].hex = to;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stairs_keep_a_ring_and_doors_only_their_hexes() {
        let stairs = keep_clear([(LinkKind::Stairs, &[100 * 200 + 100][..])], []);
        assert!(stairs.contains(&(100 * 200 + 100)));
        assert_eq!(
            stairs.len(),
            7,
            "a hex and its six neighbours (radius {STAIRS_KEEP_CLEAR_RADIUS})"
        );
        let doors = keep_clear(
            [(LinkKind::Door, &[100 * 200 + 100][..])],
            [100 * 200 + 120],
        );
        assert_eq!(
            doors,
            BTreeSet::from([100 * 200 + 100, 100 * 200 + 120]),
            "radius {DOOR_KEEP_CLEAR_RADIUS}"
        );
    }
}
