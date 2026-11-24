//! The share rule for readability and the entrance (`cover::SightRule`),
//! on the scenes of `tests/golden.rs`: by default nothing changes; with
//! `SightRule::VISIBLE` the readability counts and, for the entrance, the
//! hexes listed below change. Also the reports' critter and interior
//! summaries.

mod common;

use common::saloon::{DOOR_AT, interior, saloon};
use common::scenes::{add_door, cut_town, door_scene, doors_scene, full_base, populated, scene};
use common::*;
use map_builder::cover::{Sight, SightRule};
use map_builder::{BuildReport, ComponentReport, CritterComponent, PlacedCritter, build_map};

/// The scenes of `tests/golden.rs`, by name.
fn scenes() -> Vec<(&'static str, Scene)> {
    let mut out = Vec::new();
    out.push(("town", town()));
    let mut s = town();
    s.base = base_files(10);
    out.push(("town-base", s));
    out.push(("critter-town", scene()));
    out.push(("door-scene", door_scene()));
    out.push(("doors", doors_scene()));
    out.push(("populated", populated()));
    out.push(("cut-town", cut_town().0));
    let mut s = town();
    let frame = s.frame();
    add_door(&mut s, hex_at(&frame, 48.0, 48.0), 0xb01, 0xd01, None);
    add_door(
        &mut s,
        hex_at(&frame, 56.0, 48.0),
        0xb02,
        0xd01,
        Some(hex_point(&frame, hex_at(&frame, 56.0, 45.0))),
    );
    s.base = full_base();
    s.critter_art = vec![sprite_set("townsman", 1)];
    s.critters = vec![critter(
        0,
        "synthetic.esm:00c001",
        "Ann",
        hex_point(&frame, hex_at(&frame, 51.0, 53.0)),
        0.0,
    )];
    out.push(("connected", s));
    out.push(("saloon", saloon(interior(false, None))));
    out.push((
        "saloon-clamped",
        saloon(interior(true, Some([DOOR_AT[0], DOOR_AT[1]]))),
    ));
    out
}

/// The entrance hex each scene moves to under the share rule (the
/// others keep theirs): found by this test on 2026-11-23.
const MOVED: &[(&str, i32, i32)] = &[("saloon-clamped", 20698, 20700)];

#[test]
fn the_share_rule_is_opt_in_and_moves_only_the_listed_entrances() {
    let mut moved = Vec::new();
    for (name, scene) in scenes() {
        let plain = build_map(&scene.input()).unwrap();
        // Defaults: the any-pixel rule for both.
        assert_eq!(scene.options.entrance_sight, SightRule::AnyPixel);
        assert_eq!(scene.options.readability_sight, SightRule::AnyPixel);
        let mut s = scene;
        s.options.readability_sight = SightRule::VISIBLE;
        let read = build_map(&s.input()).unwrap();
        // Readability alone changes no byte of the layer and no entrance.
        assert_eq!(read.layer.files(), plain.layer.files(), "{name}");
        assert_eq!(read.report.entry_hex, plain.report.entry_hex, "{name}");
        let (a, b) = (&plain.report.readability, &read.report.readability);
        assert_eq!(a.walkable_hexes, b.walkable_hexes);
        assert!(b.clear >= a.clear, "{name}: {a:?} {b:?}");
        assert_eq!(b.behind, 0, "{name}: the share rule never answers Behind");
        assert_eq!(b.clear + b.covered, b.walkable_hexes);
        s.options.entrance_sight = SightRule::VISIBLE;
        let aimed = build_map(&s.input()).unwrap();
        assert_ne!(aimed.report.entrance.sight, Sight::Behind);
        if aimed.report.entry_hex != plain.report.entry_hex {
            moved.push((name, plain.report.entry_hex, aimed.report.entry_hex));
        }
    }
    for (name, from, to) in &moved {
        println!("    (\"{name}\", {from}, {to}),");
    }
    assert_eq!(moved, MOVED);
}

fn placed(reference: &str, hex: i32) -> PlacedCritter {
    PlacedCritter {
        reference: reference.into(),
        index: 0,
        hex,
        orientation: 0,
        steps: 0,
        moved: 0.0,
        pid: 0,
        stem: String::new(),
    }
}

#[test]
fn the_critter_summary_names_the_critters_the_entrance_cannot_reach() {
    let report = BuildReport {
        critters: vec![
            placed("synthetic.esm:00c001", 100),
            placed("synthetic.esm:00c002", 200),
        ],
        components: ComponentReport {
            sizes: vec![50, 10],
            first_hexes: vec![1, 2],
            main: Some(0),
            critters: vec![
                CritterComponent {
                    reference: "synthetic.esm:00c001".into(),
                    hex: 100,
                    component: Some(0),
                },
                CritterComponent {
                    reference: "synthetic.esm:00c002".into(),
                    hex: 200,
                    component: Some(1),
                },
            ],
            ..ComponentReport::default()
        },
        ..BuildReport::default()
    };
    assert_eq!(
        report.components.critter_reach("synthetic.esm:00c002"),
        Some((Some(1), false))
    );
    assert_eq!(report.components.stranded_critters().len(), 1);
    let lines = report.critters_summary();
    assert!(
        lines[0].contains("1 outside the entrance's component: synthetic.esm:00c002 (hex 200)"),
        "{lines:?}"
    );
    assert!(lines[1].contains("component 0, the entrance's"));
    assert!(lines[2].contains("component 1, not reachable from the entrance"));
}
