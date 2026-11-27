//! The package runner's input on the testkit's town (spec 7): runners,
//! places and sandbox candidates, schedules, folded conditions, combat
//! hand-off and teams.

mod support;

use support::{World, town_facts};
use transpile::diag::Category;
use transpile::ir::{Anchor, Cond, HexAt, Module, PackageKind, Runner, SimConfig, SimId, SimKind};
use transpile::packages::{CANDIDATES, radius_hexes};
use transpile::source::{SourceFaction, SourceRelation};
use transpile::testkit::{self, MAP, forms::*, key};

#[test]
fn sandbox_and_travel_slots() {
    let m = testkit::town();
    let mayor = &m.runs[&key(MAYOR)];
    let [slot] = mayor.slots.as_slice() else {
        panic!("{:?}", mayor.slots)
    };
    assert_eq!(slot.package, SimId::of(SimKind::Package, &key(SANDBOX)));
    assert_eq!(
        (slot.kind, slot.runner),
        (PackageKind::Sandbox, Runner::Sandbox)
    );
    assert!(!slot.run);
    assert_eq!((slot.schedule.hour, slot.schedule.duration_hours), (8, 12));
    let [place] = slot.places.as_slice() else {
        panic!("{:?}", slot.places)
    };
    assert_eq!(place.map, MAP);
    assert_eq!(
        place.anchor,
        Anchor::Hex(HexAt {
            elevation: 0,
            hex: 10_004
        }),
        "a marker's hex is a constant"
    );
    assert_eq!(place.radius, radius_hexes(512));
    assert_eq!(place.candidates.len(), CANDIDATES);
    assert_eq!(
        place.candidates[0].hex, 10_004,
        "the marker's own hex first"
    );
    assert!(place.candidates.iter().all(|c| c.elevation == 0));

    let guard = &m.runs[&key(GUARD)];
    let slot = &guard.slots[0];
    assert_eq!(slot.runner, Runner::Travel);
    assert!(slot.run, "always run");
    assert_eq!(slot.places[0].anchor, Anchor::Ref(key(R_DOOR)));
    assert!(
        slot.places[0].candidates.is_empty(),
        "travel has no candidates"
    );
    assert!(
        matches!(slot.condition, Cond::Cmp { .. }),
        "GetRandomPercent stays"
    );
    assert!(slot.fallback.is_none());
}

#[test]
fn combat_and_teams() {
    let m = testkit::town();
    let combat = |base: u32| &m.runs[&key(base)].combat;
    assert!(combat(RAIDER).hostile, "an enemy of the player's faction");
    assert!(!combat(GUARD).hostile, "aggressive, but no enemy faction");
    assert!(!combat(MAYOR).hostile);
    assert_eq!(combat(MAYOR).team, Some(key(TOWN_FACTION)));
    assert_eq!(
        combat(GUARD).team,
        Some(key(TOWN_FACTION)),
        "townsfolk share a team"
    );
    assert_eq!(combat(RAIDER).team, Some(key(RAIDER_FACTION)));
    assert_eq!(m.teams, vec![key(TOWN_FACTION), key(RAIDER_FACTION)]);
    assert!(m.runs[&key(RAIDER)].slots.is_empty());
    assert!(m.runs.values().all(|r| r.presence.is_empty()), "one map");
}

/// The town with two more factions, each with more relations than the
/// town's own: the mayor is also in one, the guard in the other.
fn town_with_factions(config: &SimConfig) -> Module {
    let dir = tempfile::tempdir().unwrap();
    let mir = testkit::extract(dir.path(), false);
    let mut source = transpile::source::load(&mir, &testkit::slice(false)).unwrap();
    for (id, name) in [
        (0x823, "TSMayorHouseFaction"),
        (0x824, "TSGuardPostFaction"),
    ] {
        source.factions.insert(
            key(id),
            SourceFaction {
                editor_id: Some(name.into()),
                flags: 0,
                flags2: 0,
                relations: [TOWN_FACTION, PLAYER_FACTION, RAIDER_FACTION]
                    .into_iter()
                    .map(|other| SourceRelation {
                        other: key(other),
                        modifier: 0,
                        reaction: Some(0),
                    })
                    .collect(),
            },
        );
    }
    source
        .actors
        .get_mut(&key(MAYOR))
        .unwrap()
        .factions
        .push((key(0x823), 0));
    source
        .actors
        .get_mut(&key(GUARD))
        .unwrap()
        .factions
        .push((key(0x824), 0));
    transpile::build_module(&source, &testkit::slice(false), config).unwrap()
}

#[test]
fn a_team_faction_unifies_its_members_and_leaves_the_others() {
    let team = |m: &Module, base: u32| m.runs[&key(base)].combat.team.clone();
    // The default rule: each takes its faction with the most relations.
    let m = town_with_factions(&SimConfig::default());
    assert_eq!(team(&m, MAYOR), Some(key(0x823)));
    assert_eq!(team(&m, GUARD), Some(key(0x824)));
    // The town's faction listed: both members on its team (case does not
    // matter); the raider, not a member, keeps its own.
    let config = SimConfig {
        team_factions: vec!["NoSuchFaction".into(), "tstownfaction".into()],
        ..SimConfig::default()
    };
    let m = town_with_factions(&config);
    assert_eq!(team(&m, MAYOR), Some(key(TOWN_FACTION)));
    assert_eq!(team(&m, GUARD), Some(key(TOWN_FACTION)));
    assert_eq!(team(&m, RAIDER), Some(key(RAIDER_FACTION)));
    assert_eq!(m.teams, vec![key(TOWN_FACTION), key(RAIDER_FACTION)]);
    assert!(
        m.diagnostics
            .iter()
            .any(|d| d.category == Category::UnknownTeamFaction && d.subject == "NoSuchFaction"),
        "an unknown editor id is reported"
    );
}

#[test]
fn team_factions_is_read_from_a_sim_table() {
    let config: SimConfig =
        serde_json::from_str(r#"{"team_factions": ["TSTownFaction"]}"#).unwrap();
    assert_eq!(config.team_factions, vec!["TSTownFaction".to_owned()]);
    assert_eq!(config.quest_depth, SimConfig::default().quest_depth);
    let config: SimConfig = serde_json::from_str("{}").unwrap();
    assert!(config.team_factions.is_empty());
}

/// Lowers `module` with the town's placement and runs each critter
/// program's `start` (after the map script's entry, E4) as its placed
/// object; returns the team each set, by actor base.
fn teams_set(module: &Module) -> Vec<(u32, Option<i32>)> {
    use transpile::ir::BaseFiles;
    let facts = town_facts(false);
    let out = transpile::lower_layer(module, &facts, &BaseFiles::default(), &SimConfig::default())
        .expect("the town lowers");
    let plan = |kind: &str, owner: Option<u32>| {
        out.plan
            .programs
            .iter()
            .find(|p| p.kind == kind && owner.is_none_or(|o| p.owner == key(o).to_string()))
            .unwrap_or_else(|| panic!("no {kind} program"))
    };
    let map = plan("map", None);
    let map_bytes = &out.lowered.files[&format!("scripts/{}.int", map.stem)];
    [(MAYOR, R_MAYOR), (GUARD, R_GUARD), (RAIDER, R_RAIDER)]
        .into_iter()
        .map(|(base, reference)| {
            let critter = plan("critter", Some(base));
            let mut world = World::new(module.globals.len(), critter.local_vars as usize);
            for (i, g) in module.globals_in_allocation_order().iter().enumerate() {
                world.globals[i] = g.initial;
            }
            let object = facts
                .placement
                .objects
                .iter()
                .find(|o| o.reference == Some(key(reference)))
                .and_then(|o| o.object_id)
                .expect("placed")
                + 100;
            support::Instance::load(&map.stem, map_bytes, &mut world)
                .call(&mut world, "map_enter_p_proc");
            world.self_obj = object;
            let bytes = &out.lowered.files[&format!("scripts/{}.int", critter.stem)];
            let mut program = support::Instance::load(&critter.stem, bytes, &mut world);
            let first = world.teams.get(&object).copied();
            // Map entry and a second start do not set it again.
            world.teams.clear();
            program.call(&mut world, "map_enter_p_proc");
            program.call(&mut world, "start");
            assert!(world.teams.is_empty(), "set once per critter");
            (base, first)
        })
        .collect()
}

#[test]
fn critter_programs_set_their_faction_team_at_start() {
    use transpile::packages::TEAM_BASE;
    // Default rule: the mayor and the guard on teams of their own
    // factions, the raider on the raiders'.
    let m = town_with_factions(&SimConfig::default());
    let teams = teams_set(&m);
    let team = |base: u32| teams.iter().find(|(b, _)| *b == base).unwrap().1;
    assert!(team(MAYOR).is_some_and(|t| t >= TEAM_BASE));
    assert_ne!(
        team(MAYOR),
        team(GUARD),
        "different factions, different teams"
    );
    assert_ne!(team(MAYOR), team(RAIDER));
    assert_ne!(team(GUARD), team(RAIDER));
    // The town faction listed: the mayor and the guard on one team, the
    // raider on another.
    let config = SimConfig {
        team_factions: vec!["TSTownFaction".into()],
        ..SimConfig::default()
    };
    let m = town_with_factions(&config);
    let teams = teams_set(&m);
    let team = |base: u32| teams.iter().find(|(b, _)| *b == base).unwrap().1;
    assert_eq!(team(MAYOR), team(GUARD), "one team for the town faction");
    assert!(team(MAYOR).is_some());
    assert_ne!(team(MAYOR), team(RAIDER));
    let expected = |faction: u32| {
        m.teams
            .iter()
            .position(|t| *t == key(faction))
            .map(|i| TEAM_BASE + i as i32)
    };
    assert_eq!(team(MAYOR), expected(TOWN_FACTION));
    assert_eq!(team(RAIDER), expected(RAIDER_FACTION));
}
