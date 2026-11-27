//! The package runner's input on the testkit's town (spec 7): runners,
//! places and sandbox candidates, schedules, folded conditions, combat
//! hand-off and teams.

use transpile::ir::{Anchor, Cond, HexAt, PackageKind, Runner, SimId, SimKind};
use transpile::packages::{CANDIDATES, radius_hexes};
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
