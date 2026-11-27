//! The AI package runner's input (spec 7, task T6).
//!
//! [`compile`] makes one [`ActorRun`] per actor base placed in the slice
//! (`Module::runs`), and the team list (`Module::teams`):
//!
//! - **Slots** (spec 7.2, 7.3): the base's packages in `PKID` order, each
//!   with its runner (Travel, Sandbox, Follow, Guard, Wait), its schedule,
//!   its condition folded for the actor as the subject (spec 4.2's static
//!   terms), "always run" (general flag [`ALWAYS_RUN`], checked against the
//!   real packages: see the module's report) and its places. A type no
//!   runner covers (flee, use weapon, unknown), a location the slice cannot
//!   resolve, or a follow without a target is a counted fallback: the
//!   actor waits where it is ([`Category::PackageFallback`]). Eat and Sleep
//!   travel to their place and stand: their furniture action is gap F
//!   ([`Category::FurnitureAction`]).
//! - **Places** (spec 7.3, "Locations to hexes"): near a reference, on the
//!   map that reference is placed on; in a cell, on the map holding the
//!   cell's placed references, anchored at the actor's own hex when it
//!   stands there; near the editor location, the actor's own hex; near the
//!   current location, where it stands; the linked reference's hex. The
//!   radius is `ceil(radius / 70)` hexes (at least 10 for a whole cell).
//!   Sandbox candidates: up to 16 walkable hexes within the radius in the
//!   anchor's component, sorted by distance then hex, every k-th
//!   ([`sample`]), from the map stage's walkable masks
//!   (`SimSlice::walkable`); without them the anchor is the only candidate
//!   ([`Category::NoWalkable`]).
//! - **Combat** (spec 7.5, owner decision 11): hostile to the player when
//!   aggression is at least 1 and a faction is an enemy of `PlayerFaction`
//!   (either side's relation), or aggression is at least 2 and no faction
//!   is a friend or ally of it, or the placement index marks it hostile;
//!   the team is the first of `SimConfig::team_factions` the actor is a
//!   member of (the town's own faction: all its members one team), else
//!   the non-crime faction (no "crime" in its editor id and no crime
//!   tracking) with the most relations, so townsfolk share a team and
//!   raiders share another.
//! - **Presence** (spec 7.6, owner decision 5): the maps holding the actor,
//!   its presence copies included, when there is more than one; a place on
//!   a map with no copy is dropped with a note. Named actors with copies
//!   get the shared dead global ([`actor_sub::DEAD`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use vault_formats::geometry::HexGrid;

use crate::build::Pass;
use crate::cond;
use crate::diag::{Category, Diag};
use crate::ir::{
    ActorRun, Anchor, Combat, FormKey, Global, GlobalPurpose, HexAt, Location, Package,
    PackageKind, Place, PlacedKind, PlacedObject, RunSlot, RunTarget, Runner, SimId, SimKind,
    Target, VarTy, actor_sub, global_name,
};

/// `PKDT` general flag "always run": run rather than walk. Measured on the
/// Fallout 3 master: set on 432 of 3,266 packages, mostly travel (222),
/// follow (37), guard and patrol, and on editor ids that say rush, flee,
/// hurry or follow; plausible.
pub const ALWAYS_RUN: u32 = 0x0000_2000;

/// `PKDT` general flag "must reach location": recorded, not used (the
/// runner always walks to its goal).
pub const MUST_REACH_LOCATION: u32 = 0x0000_0002;

/// Most sandbox candidates per place.
pub const CANDIDATES: usize = 16;

/// Source units per hex (contracts section 4: a hex is half a floor
/// square of 140 units).
pub const UNITS_PER_HEX: i32 = 70;

/// Hexes of a radius in source units: `ceil(radius / 70)`.
pub fn radius_hexes(radius: i32) -> u32 {
    u32::try_from(radius.max(0))
        .unwrap_or(0)
        .div_ceil(UNITS_PER_HEX as u32)
}

/// Samples up to [`CANDIDATES`] walkable hexes within `radius` of `anchor`
/// in its component (spec 7.3): a breadth-first walk over the walkable
/// hexes from the anchor (or from the nearest walkable hex within the
/// radius when the anchor is not walkable), the hexes found sorted by
/// distance to the anchor and then by number, and every k-th taken, k the
/// count over 16 rounded up. Deterministic.
pub fn sample(anchor: u32, radius: u32, walkable: &BTreeSet<u32>) -> Vec<u32> {
    let distance = |a: u32, b: u32| {
        HexGrid::lattice_distance(a as i32, b as i32).map_or(u32::MAX, |d| d.max(0) as u32)
    };
    let start = if walkable.contains(&anchor) {
        Some(anchor)
    } else {
        walkable
            .iter()
            .copied()
            .filter(|h| distance(anchor, *h) <= radius)
            .min_by_key(|h| (distance(anchor, *h), *h))
    };
    let Some(start) = start else {
        return Vec::new();
    };
    let mut seen: BTreeSet<u32> = BTreeSet::from([start]);
    let mut queue: VecDeque<u32> = VecDeque::from([start]);
    let mut found: Vec<(u32, u32)> = Vec::new();
    while let Some(hex) = queue.pop_front() {
        found.push((distance(anchor, hex), hex));
        for orientation in 0..6u8 {
            let Ok(Some(next)) = HexGrid::neighbor(hex as i32, orientation) else {
                continue;
            };
            let next = next as u32;
            if walkable.contains(&next) && distance(anchor, next) <= radius && seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    found.sort_unstable();
    let step = found.len().div_ceil(CANDIDATES).max(1);
    found
        .iter()
        .step_by(step)
        .map(|&(_, h)| h)
        .take(CANDIDATES)
        .collect()
}

/// Adds the actors' runs, the teams and the presence copies.
pub fn compile(pass: &mut Pass<'_>) {
    let dynamic = crate::dialogue::dynamic_factions(pass);
    let player_faction = pass
        .source
        .factions
        .iter()
        .find(|(_, f)| {
            f.editor_id
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case("PlayerFaction"))
        })
        .map(|(k, _)| k.clone());
    let actors: Vec<FormKey> = pass
        .module
        .actors
        .values()
        .filter(|a| !a.placements.is_empty())
        .map(|a| a.base.clone())
        .collect();
    let walkable: BTreeMap<(String, u8), BTreeSet<u32>> = pass
        .slice
        .walkable
        .iter()
        .flat_map(|(map, masks)| {
            masks.iter().map(move |m| {
                (
                    (map.clone(), m.elevation),
                    m.hexes.iter().copied().collect(),
                )
            })
        })
        .collect();
    let mut no_walk: BTreeSet<String> = BTreeSet::new();
    let mut teams: BTreeSet<FormKey> = BTreeSet::new();
    for base in actors {
        let facts = crate::dialogue::facts(pass, &base, &dynamic);
        let actor = pass.module.actors[&base].clone();
        // Where the actor stands: its placements, copies included.
        let mut objects: Vec<PlacedObject> = actor
            .placements
            .iter()
            .flat_map(|r| pass.placed.get(r).cloned().unwrap_or_default())
            .collect();
        objects.sort();
        let maps: BTreeSet<String> = objects.iter().map(|o| o.map.clone()).collect();
        let named = actor.placements.len() == 1;
        let mut slots = Vec::new();
        for package_id in &actor.packages {
            let Some(package) = pass.module.packages.get(package_id).cloned() else {
                continue;
            };
            let slot = slot(
                pass,
                &actor.base,
                &package,
                &objects,
                &facts,
                &walkable,
                &mut no_walk,
            );
            slots.push(slot);
        }
        // Places on a map with no copy of the actor.
        for slot in &mut slots {
            let before = slot.places.len();
            slot.places.retain(|p| maps.contains(&p.map));
            if slot.places.len() < before {
                let dropped = before - slot.places.len();
                pass.module.diagnostics.push(
                    Diag::warning(
                        Category::LocationOutsideSlice,
                        format!("{dropped} place(s) on a map where the actor has no copy; dropped"),
                    )
                    .owned_by(&slot.package)
                    .about(actor.editor_id.clone().unwrap_or_default()),
                );
                if slot.places.is_empty() && slot.fallback.is_none() && needs_place(slot.runner) {
                    slot.fallback = Some("its place is on a map without a copy".into());
                    slot.runner = Runner::Wait;
                }
            }
        }
        let combat = combat(pass, &actor.base, &objects, player_faction.as_ref());
        teams.extend(combat.team.clone());
        let presence: Vec<String> = if named && maps.len() > 1 {
            maps.iter().cloned().collect()
        } else {
            Vec::new()
        };
        if !presence.is_empty() {
            let name = actor.editor_id.clone().unwrap_or_default();
            pass.diag(
                Diag::note(
                    Category::PresenceCopy,
                    format!(
                        "present on {} maps: {}",
                        presence.len(),
                        presence.join(", ")
                    ),
                )
                .owned_by(&SimId::of(SimKind::Actor, &base))
                .about(name.clone()),
            );
            if named && let Some(reference) = actor.placements.first() {
                let id = SimId::new(SimKind::Ref, reference.clone(), actor_sub::DEAD);
                pass.module.globals.entry(id.clone()).or_insert(Global {
                    id,
                    purpose: GlobalPurpose::Actor,
                    name: global_name(&format!("{name}_DEAD")),
                    ty: VarTy::Int,
                    initial: 0,
                });
            }
        }
        pass.module.runs.insert(
            base.clone(),
            ActorRun {
                base: base.clone(),
                slots,
                combat,
                factions: actor.factions.iter().map(|(f, _)| f.clone()).collect(),
                presence,
            },
        );
    }
    for map in no_walk {
        pass.diag(
            Diag::warning(
                Category::NoWalkable,
                format!(
                    "no walkable hex near a sandbox anchor on map {map} (or no walkable data): \
                     the anchor is its only candidate"
                ),
            )
            .about(map),
        );
    }
    pass.module.teams = teams.into_iter().collect();
    for wanted in &pass.config.team_factions {
        let known = pass.source.factions.values().any(|f| {
            f.editor_id
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(wanted))
        });
        if !known {
            pass.diag(
                Diag::warning(
                    Category::UnknownTeamFaction,
                    "a configured team faction is not a faction of the plugin list; ignored",
                )
                .about(wanted.clone()),
            );
        }
    }
}

/// The first team number (spec 7.5): the faction `module.teams[i]` is team
/// `TEAM_BASE + i`, above the player's 0 and the map stage's hostile (66)
/// and friendly (99) defaults.
pub const TEAM_BASE: i32 = 100;

/// The team number of an actor base: its faction's place in
/// `Module::teams`, from [`TEAM_BASE`]; `None` when it has no team.
pub fn team_number(module: &crate::ir::Module, base: &FormKey) -> Option<i32> {
    let team = module.runs.get(base)?.combat.team.as_ref()?;
    let index = module.teams.iter().position(|t| t == team)?;
    Some(TEAM_BASE + i32::try_from(index).ok()?)
}

fn needs_place(runner: Runner) -> bool {
    matches!(runner, Runner::Travel | Runner::Sandbox | Runner::Guard)
}

/// The player's reference (`PlayerRef`, local id `0x14` of the first
/// plugin) or base (`Player`, `0x07`).
fn is_player(pass: &Pass<'_>, form: &FormKey) -> bool {
    let first = pass
        .source
        .plugins
        .first()
        .map(String::as_str)
        .unwrap_or("");
    form.as_str()
        .rsplit_once(':')
        .is_some_and(|(p, id)| p == first && (id == "000014" || id == "000007"))
}

/// A resolved location on one map: the map, the anchor, and its hex
/// when known.
type Spot = (String, Anchor, Option<HexAt>);

#[allow(clippy::too_many_arguments)]
fn slot(
    pass: &mut Pass<'_>,
    base: &FormKey,
    package: &Package,
    objects: &[PlacedObject],
    facts: &cond::Facts,
    walkable: &BTreeMap<(String, u8), BTreeSet<u32>>,
    no_walk: &mut BTreeSet<String>,
) -> RunSlot {
    let mut runner = package.kind.runner();
    let mut fallback: Option<String> = None;
    if runner == Runner::None {
        fallback = Some(format!("package type {:?} has no runner", package.kind));
        runner = Runner::Wait;
    }
    if matches!(package.kind, PackageKind::Eat | PackageKind::Sleep) {
        pass.diag(
            Diag::note(
                Category::FurnitureAction,
                format!(
                    "{:?}: travels to its place and stands (gap F)",
                    package.kind
                ),
            )
            .owned_by(&package.id)
            .about(format!("{:?}", package.kind)),
        );
    }
    // Places.
    let mut places: Vec<Place> = Vec::new();
    let own_hex = |map: &str| {
        objects.iter().find(|o| o.map == map).map(|o| HexAt {
            elevation: o.elevation,
            hex: o.hex,
        })
    };
    let actor_maps: BTreeSet<String> = objects.iter().map(|o| o.map.clone()).collect();
    let near_ref = |pass: &Pass<'_>, r: &FormKey| -> Vec<(String, Anchor, HexAt)> {
        pass.placed
            .get(r)
            .into_iter()
            .flatten()
            .map(|o| {
                let at = HexAt {
                    elevation: o.elevation,
                    hex: o.hex,
                };
                // A marker has no object: its hex is a constant.
                let anchor = if o.marker {
                    Anchor::Hex(at)
                } else {
                    Anchor::Ref(r.clone())
                };
                (o.map.clone(), anchor, at)
            })
            .collect()
    };
    let linked = objects
        .iter()
        .find_map(|o| o.linked_ref.clone())
        .or_else(|| {
            pass.module.actors[base]
                .placements
                .iter()
                .find_map(|r| pass.source.refs.get(r).and_then(|s| s.linked_ref.clone()))
        });
    let resolved: Option<(Vec<Spot>, i32)> = match &package.location {
        None => None,
        Some(Location::NearRef { r, radius }) => Some((
            near_ref(pass, r)
                .into_iter()
                .map(|(m, a, h)| (m, a, Some(h)))
                .collect(),
            *radius,
        )),
        Some(Location::InCell { cell, radius }) => {
            let mut on: BTreeMap<String, HexAt> = BTreeMap::new();
            for (reference, placed) in pass.placed {
                if pass
                    .source
                    .refs
                    .get(reference)
                    .and_then(|r| r.cell.as_ref())
                    == Some(cell)
                {
                    for o in placed {
                        on.entry(o.map.clone()).or_insert(HexAt {
                            elevation: o.elevation,
                            hex: o.hex,
                        });
                    }
                }
            }
            let list = on
                .into_iter()
                .map(|(map, hex)| match own_hex(&map) {
                    Some(own) => (map, Anchor::Editor, Some(own)),
                    None => (map, Anchor::Hex(hex), Some(hex)),
                })
                .collect();
            Some((list, (*radius).max(10 * UNITS_PER_HEX)))
        }
        Some(Location::NearEditor { radius }) => Some((
            actor_maps
                .iter()
                .map(|m| (m.clone(), Anchor::Editor, own_hex(m)))
                .collect(),
            *radius,
        )),
        Some(Location::NearCurrent { radius }) => Some((
            actor_maps
                .iter()
                .map(|m| (m.clone(), Anchor::Current, None))
                .collect(),
            *radius,
        )),
        Some(Location::AtPackageLocation) => Some((
            actor_maps
                .iter()
                .map(|m| (m.clone(), Anchor::Current, None))
                .collect(),
            0,
        )),
        Some(Location::NearLinkedRef { radius }) => Some((
            linked
                .iter()
                .flat_map(|r| near_ref(pass, r))
                .map(|(m, a, h)| (m, a, Some(h)))
                .collect(),
            *radius,
        )),
        Some(Location::ObjectId(form)) => {
            // The placed references of that base on the actor's maps.
            let mut list = Vec::new();
            for (reference, placed) in pass.placed {
                for o in placed {
                    if o.base.as_ref() == Some(form) && actor_maps.contains(&o.map) {
                        list.push((
                            o.map.clone(),
                            Anchor::Ref(reference.clone()),
                            Some(HexAt {
                                elevation: o.elevation,
                                hex: o.hex,
                            }),
                        ));
                    }
                }
            }
            list.dedup_by(|a, b| a.0 == b.0);
            Some((list, 0))
        }
        Some(Location::ObjectType(_)) => Some((Vec::new(), 0)),
    };
    if let Some((list, radius)) = &resolved {
        let hexes = radius_hexes(*radius);
        for (map, anchor, at) in list {
            let candidates = match (runner, at) {
                (Runner::Sandbox, Some(at)) => {
                    let found: Vec<HexAt> = walkable
                        .get(&(map.clone(), at.elevation))
                        .map(|mask| sample(at.hex, hexes, mask))
                        .unwrap_or_default()
                        .into_iter()
                        .map(|hex| HexAt {
                            elevation: at.elevation,
                            hex,
                        })
                        .collect();
                    if found.is_empty() {
                        no_walk.insert(map.clone());
                        vec![*at]
                    } else {
                        found
                    }
                }
                _ => Vec::new(),
            };
            places.push(Place {
                map: map.clone(),
                anchor: anchor.clone(),
                radius: hexes,
                candidates,
            });
        }
    }
    places.sort_by(|a, b| a.map.cmp(&b.map));
    places.dedup_by(|a, b| a.map == b.map);
    if needs_place(runner) && places.is_empty() && fallback.is_none() {
        fallback = Some(match &package.location {
            None => "no location".to_owned(),
            Some(l) => format!("location {l:?} is not placed in the slice"),
        });
        runner = Runner::Wait;
    }
    // Target.
    let target = match &package.target {
        Some(Target::Ref(r) | Target::Object(r)) if is_player(pass, r) => Some(RunTarget::Player),
        Some(Target::Ref(r)) => Some(RunTarget::Ref(r.clone())),
        Some(Target::Object(form)) => pass
            .placed
            .iter()
            .find(|(_, objs)| objs.iter().any(|o| o.base.as_ref() == Some(form)))
            .map(|(r, _)| RunTarget::Ref(r.clone())),
        Some(Target::LinkedRef) => linked.map(RunTarget::Ref),
        Some(Target::ObjectType(_)) | None => None,
    };
    if runner == Runner::Follow && target.is_none() && fallback.is_none() {
        fallback = Some("a follow without a usable target".to_owned());
        runner = Runner::Wait;
    }
    if let Some(reason) = &fallback {
        pass.diag(
            Diag::warning(
                Category::PackageFallback,
                format!("{reason}: the actor waits"),
            )
            .owned_by(&package.id)
            .about(format!("{:?}", package.kind)),
        );
    }
    RunSlot {
        package: package.id.clone(),
        runner,
        kind: package.kind,
        run: package.flags & ALWAYS_RUN != 0,
        condition: cond::fold(&package.condition, facts),
        schedule: package.schedule,
        places,
        target,
        fallback,
    }
}

/// Whether a faction is a crime faction: its editor id says so.
fn is_crime(pass: &Pass<'_>, faction: &FormKey) -> bool {
    pass.source.factions.get(faction).is_some_and(|f| {
        f.editor_id
            .as_deref()
            .is_some_and(|e| e.to_ascii_lowercase().contains("crime"))
    })
}

/// How many relations name a faction or are its own: the factions that
/// matter to combat have them; voice, idle and quest-marker factions do
/// not.
fn relation_weight(pass: &Pass<'_>, faction: &FormKey) -> usize {
    let own = pass
        .source
        .factions
        .get(faction)
        .map_or(0, |f| f.relations.len());
    let inbound = pass
        .source
        .factions
        .values()
        .flat_map(|f| f.relations.iter())
        .filter(|r| &r.other == faction)
        .count();
    own + inbound
}

fn combat(
    pass: &Pass<'_>,
    base: &FormKey,
    objects: &[PlacedObject],
    player: Option<&FormKey>,
) -> Combat {
    let actor = &pass.module.actors[base];
    let ai = actor.ai.unwrap_or_default();
    let factions: Vec<&FormKey> = actor.factions.iter().map(|(f, _)| f).collect();
    let reaction = |a: &FormKey, b: &FormKey| {
        pass.source
            .factions
            .get(a)
            .and_then(|f| f.relations.iter().find(|r| &r.other == b))
            .and_then(|r| r.reaction)
    };
    let (mut enemy, mut friend) = (false, false);
    if let Some(player) = player {
        for f in &factions {
            if *f == player {
                friend = true;
            }
            for code in [reaction(f, player), reaction(player, f)]
                .into_iter()
                .flatten()
            {
                match code {
                    1 => enemy = true,
                    2 | 3 => friend = true,
                    _ => {}
                }
            }
        }
    }
    let marked = objects
        .iter()
        .any(|o| o.hostile && o.kind == PlacedKind::Critter);
    let (hostile, reason) = if ai.aggression >= 1 && enemy {
        (
            true,
            "aggressive, and a faction is an enemy of the player's".to_owned(),
        )
    } else if ai.aggression >= 2 && !friend {
        (
            true,
            "very aggressive, and no faction is a friend or ally".to_owned(),
        )
    } else if marked {
        (true, "the placement index marks it hostile".to_owned())
    } else {
        (
            false,
            format!(
                "aggression {}, {}",
                ai.aggression,
                if enemy {
                    "enemy faction"
                } else {
                    "no enemy faction"
                }
            ),
        )
    };
    // The team: the first configured team faction the actor is a member
    // of (`SimConfig::team_factions`, the town's own faction); else the
    // non-crime faction with the most relations, the first such in the
    // actor's order on a tie (spec 7.5's "first non-crime faction", made
    // robust to the order of templated actors' lists).
    let listed = pass.config.team_factions.iter().find_map(|wanted| {
        factions.iter().copied().find(|f| {
            pass.source
                .factions
                .get(*f)
                .and_then(|x| x.editor_id.as_deref())
                .is_some_and(|e| e.eq_ignore_ascii_case(wanted))
        })
    });
    let team = listed.cloned().or_else(|| {
        let mut team: Option<(&FormKey, usize)> = None;
        for f in factions
            .iter()
            .filter(|f| !is_crime(pass, f) && Some(**f) != player)
        {
            let weight = relation_weight(pass, f);
            if team.is_none_or(|(_, best)| weight > best) {
                team = Some((f, weight));
            }
        }
        team.map(|(f, _)| f.clone())
    });
    Combat {
        hostile,
        reason,
        team,
        aggression: ai.aggression,
        confidence: ai.confidence,
        assistance: ai.assistance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_in_hexes() {
        assert_eq!(radius_hexes(0), 0);
        assert_eq!(radius_hexes(1), 1);
        assert_eq!(radius_hexes(70), 1);
        assert_eq!(radius_hexes(512), 8);
        assert_eq!(radius_hexes(-5), 0);
    }

    #[test]
    fn sampling_is_deterministic_and_stays_in_the_component() {
        // A 12 by 12 block, and a separate island far from it.
        let mut walkable: BTreeSet<u32> = (0..12u32)
            .flat_map(|r| (0..12u32).map(move |c| (50 + r) * 200 + 100 + c))
            .collect();
        let island = 50 * 200 + 120;
        walkable.insert(island);
        let anchor = 55 * 200 + 105;
        let a = sample(anchor, 8, &walkable);
        assert_eq!(a, sample(anchor, 8, &walkable));
        assert_eq!(a.len(), CANDIDATES);
        assert_eq!(a[0], anchor, "the anchor itself comes first");
        assert!(!a.contains(&island));
        assert!(a.iter().all(|h| walkable.contains(h)));
        assert!(
            a.iter()
                .all(|h| HexGrid::lattice_distance(anchor as i32, *h as i32).unwrap() <= 8)
        );
        // An anchor off the mask starts from the nearest walkable hex.
        let off = 49 * 200 + 105;
        let b = sample(off, 3, &walkable);
        assert!(!b.is_empty() && !b.contains(&off));
        // Nothing walkable in range.
        assert!(sample(10, 2, &walkable).is_empty());
    }
}
