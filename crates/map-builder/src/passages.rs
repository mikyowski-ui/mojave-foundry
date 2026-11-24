//! Joining islands to the largest component (crate notes, "Passages").

use std::collections::{BTreeMap, BTreeSet};

use mir::{CellDoc, GroundSampler};

use crate::cover::Cover;
use crate::islands::{self, Components, OpenCost, on_border, shortest_passage, widen};
use crate::walk::{self, Frame, HEXES, Verdict, neighbours};
use crate::{BuildOptions, IslandReport, PassageReport, Placement, Role};

/// Something a walker must reach: any one of `hexes`, described by `what`.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub hexes: Vec<i32>,
    pub what: String,
}

/// Per hex, whether and at what cost it may be opened (crate notes).
/// `blocks` says whether a placement blocks movement in the walkability
/// pass.
#[allow(clippy::too_many_arguments)]
pub(crate) fn open_costs(
    frame: &Frame,
    sampler: &GroundSampler,
    cells: &[CellDoc],
    options: &BuildOptions,
    walkable: &[bool],
    placements: &[Placement],
    blocks: impl Fn(&Placement) -> bool,
    cover: &Cover,
) -> Vec<Option<OpenCost>> {
    let mut protected = vec![false; HEXES];
    for placement in placements {
        let hex = placement.hex as usize;
        if placement.role == Role::WallStrip || placement.role == Role::Door || blocks(placement) {
            protected[hex] = true;
        }
        if placement.role == Role::Door {
            for next in neighbours(placement.hex) {
                protected[next as usize] = true;
            }
        }
    }
    for hex in &options.protected_hexes {
        if let Some(slot) = usize::try_from(*hex)
            .ok()
            .and_then(|h| protected.get_mut(h))
        {
            *slot = true;
        }
    }
    // Walkability with nothing blocking: what the ground itself allows.
    let (free, _) = walk::verdicts(frame, sampler, cells, &[], options);
    (0..HEXES as i32)
        .map(|hex| {
            let h = hex as usize;
            if walkable[h] || protected[h] || on_border(hex) {
                return None;
            }
            let cost = match free[h] {
                Verdict::Walkable => OpenCost::Standable,
                // An interior's ground is its floor level's navmesh only:
                // off it there is wall or void, never ground to open.
                Verdict::OffNavmesh if options.interior.is_some() => return None,
                // Nor above an exterior's ground level.
                Verdict::OffNavmesh if options.level.as_ref().is_some_and(|l| l.level > 0) => {
                    return None;
                }
                Verdict::OffNavmesh => {
                    let [x, y] = frame.source_of_tile(frame.hex_centre_tile(hex)?);
                    let slope = sampler.slope(x as f32, y as f32)?;
                    if slope > options.slope_limit {
                        return None;
                    }
                    OpenCost::OffNavmesh
                }
                _ => return None,
            };
            (!cover.stands_on(hex)).then_some(cost)
        })
        .collect()
}

/// The island a target should be joined through: `None` when one of its
/// hexes is already in `main` (or none is walkable), else the
/// lowest-numbered (largest) component among its hexes.
fn island_of(comps: &Components, main: usize, target: &Target) -> Option<usize> {
    let ids: Vec<usize> = target.hexes.iter().filter_map(|h| comps.of(*h)).collect();
    if ids.contains(&main) {
        return None;
    }
    ids.into_iter().min()
}

/// Join every island that holds a target to the largest component (crate
/// notes), or to the component of `hub` when that hex is walkable,
/// opening hexes in `walkable`. `costs` is only called when some island
/// needs a passage. With `enabled` false nothing is opened and every such
/// island is reported as left alone.
pub(crate) fn reconnect(
    walkable: &mut [bool],
    targets: &[Target],
    costs: impl FnOnce() -> Vec<Option<OpenCost>>,
    options: &BuildOptions,
    enabled: bool,
    hub: Option<i32>,
) -> (Vec<PassageReport>, Vec<IslandReport>) {
    let comps = islands::components(|h| walkable[h as usize]);
    let mut passages = Vec::new();
    let mut left = Vec::new();
    let hub = hub.and_then(|hex| comps.of(hex).map(|id| (hex, id)));
    let Some((main_hex, main)) = hub.or_else(|| comps.first_hex(0).map(|hex| (hex, 0))) else {
        return (passages, left);
    };
    let mut holdings: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for target in targets {
        if let Some(id) = island_of(&comps, main, target) {
            holdings.entry(id).or_default().push(target.what.clone());
        }
    }
    if holdings.is_empty() {
        return (passages, left);
    }
    let limit = options.passage_max_length;
    if !enabled {
        for (id, holds) in holdings {
            left.push(IslandReport {
                island_hex: comps.first_hex(id).unwrap_or(-1),
                island_size: comps.sizes()[id],
                holds,
                reason: "reconnecting is off (BuildOptions::reconnect)".into(),
            });
        }
        return (passages, left);
    }
    let mut cost = costs();
    let mut current = comps.clone();
    for (id, holds) in holdings {
        let island_hex = comps.first_hex(id).unwrap_or(-1);
        let island_size = comps.sizes()[id];
        let (Some(main_now), Some(now)) = (current.of(main_hex), current.of(island_hex)) else {
            continue;
        };
        if now == main_now {
            // An earlier passage joined it as well.
            continue;
        }
        let leave = |reason: String| IslandReport {
            island_hex,
            island_size,
            holds: holds.clone(),
            reason,
        };
        if current.members(now).all(on_border) {
            left.push(leave(
                "it lies on the map's outer border, which the engine never steps off".into(),
            ));
            continue;
        }
        let reached = current.mask(main_now);
        let target = current.mask(now);
        match shortest_passage(&reached, &target, &cost, Some(limit)) {
            Some(passage) => {
                let opened = widen(&passage.hexes, options.passage_rings, &reached, &cost);
                for hex in &opened {
                    walkable[*hex as usize] = true;
                    cost[*hex as usize] = None;
                }
                passages.push(PassageReport {
                    from: passage.from,
                    to: passage.to,
                    length: passage.hexes.len(),
                    off_navmesh: passage.off_navmesh,
                    opened,
                    island_size,
                    island_hex,
                    holds: holds.clone(),
                });
                current = islands::components(|h| walkable[h as usize]);
            }
            None => {
                let reason = match shortest_passage(&reached, &target, &cost, None) {
                    Some(longer) => format!(
                        "the shortest passage through openable hexes is {} hexes long; the \
                         limit is {limit}",
                        longer.hexes.len()
                    ),
                    None => "no passage through openable hexes: every way crosses wall art, \
                             blocking scenery, a door's doorstep, steep ground or the map border"
                        .into(),
                };
                left.push(leave(reason));
            }
        }
    }
    (passages, left)
}

/// The targets of a build: exits (any grid hex), return hexes, and
/// critters where they would stand; with links, after the stub exits' grids, each link's grids
/// (`(what, hexes)`); after the return hexes, every arrival hex other
/// links lead to here (`(hex, what)`), each hex once.
pub(crate) fn targets_with(
    exits: &[crate::exits::DoorExit],
    links: &[(String, Vec<i32>)],
    arrivals: &[(i32, String)],
    critters: &[(String, i32)],
) -> Vec<Target> {
    let mut out = Vec::new();
    for exit in exits {
        out.push(Target {
            hexes: exit.grid_hexes.clone(),
            what: format!("exit {}", exit.door),
        });
    }
    for (what, hexes) in links {
        out.push(Target {
            hexes: hexes.clone(),
            what: what.clone(),
        });
    }
    let mut returns = BTreeSet::new();
    for exit in exits {
        if returns.insert(exit.return_hex) {
            out.push(Target {
                hexes: vec![exit.return_hex],
                what: format!("return of {}", exit.door),
            });
        }
    }
    for (hex, what) in arrivals {
        if returns.insert(*hex) {
            out.push(Target {
                hexes: vec![*hex],
                what: what.clone(),
            });
        }
    }
    for (reference, hex) in critters {
        out.push(Target {
            hexes: vec![*hex],
            what: format!("critter {reference}"),
        });
    }
    out
}
