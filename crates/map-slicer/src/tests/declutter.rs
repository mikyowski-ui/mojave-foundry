//! The declutter and readability pass on a synthetic cluttered room.

use std::collections::BTreeSet;

use mir::{FormKey, LockDoc};

use super::*;
use crate::hex::Plane;
use crate::{
    ClutterRules, Cutaway, DeclutterProfile, DensityRule, InteriorSlice, SkipReason, floor_stats,
};

const FLOOR_Z: f32 = 2048.0;
/// Half the room's side: walls stand at +-HALF.
const HALF: f32 = 560.0;

fn on_floor() -> GroundSampler {
    sampler(|_, _| FLOOR_Z)
}

fn assets() -> TestAssets {
    TestAssets::new()
        .with(
            "floor",
            mesh(&nif::fixture::quad([2.0 * HALF, 2.0 * HALF], ROOF)),
        )
        .with(
            "wall",
            mesh(&nif::fixture::box_mesh([2.0 * HALF, 16.0, 200.0], WALL)),
        )
        .with(
            "bar",
            mesh(&nif::fixture::box_mesh([400.0, 60.0, 110.0], WALL)),
        )
        .with(
            "table",
            mesh(&nif::fixture::box_mesh([80.0, 80.0, 55.0], ROOF)),
        )
        .with("can", mesh(&nif::fixture::box_mesh([8.0, 8.0, 12.0], ROOF)))
        .with("paper", mesh(&nif::fixture::quad([20.0, 25.0], WALL)))
        .with(
            "small",
            mesh(&nif::fixture::box_mesh([20.0, 20.0, 30.0], WALL)),
        )
        .with(
            "door",
            mesh(&nif::fixture::box_mesh([100.0, 10.0, 140.0], WALL)),
        )
        .with(
            "beam",
            mesh(&nif::fixture::box_mesh([600.0, 16.0, 14.0], WALL)),
        )
        .with(
            "lamp",
            mesh(&nif::fixture::box_mesh([20.0, 20.0, 40.0], ROOF)),
        )
}

fn key(id: u32) -> String {
    FormKey::new("test.esm", id).to_string()
}

fn put(id: u32, kind: &str, hash: &str, xy: [f32; 2], lift: f32, yaw: f32) -> Reference {
    reference(
        id,
        kind,
        &format!("meshes\\test\\{hash}.nif"),
        hash,
        &on_floor(),
        xy,
        lift,
        yaw,
    )
}

const SOUTH_WALL: u32 = 2;
const NORTH_WALL: u32 = 3;
const TERMINAL: u32 = 20;
const CONTAINER: u32 = 21;
const DOOR: u32 = 22;
const QUEST: u32 = 23;
const BEAM: u32 = 24;
const LAMP: u32 = 25;
const CANS: std::ops::Range<u32> = 100..120;
const PAPERS: std::ops::Range<u32> = 200..210;

/// A room: floor, four walls, a bar, three tables, 20 cans and 10 papers,
/// a terminal, a container, a locked door, a quest object as small as a
/// can, a beam and a hanging lamp.
fn room() -> Vec<Reference> {
    let quarter = std::f32::consts::FRAC_PI_2;
    let mut refs = vec![
        put(1, "STAT", "floor", [0.0, 0.0], 0.0, 0.0),
        put(SOUTH_WALL, "STAT", "wall", [0.0, -HALF], 0.0, 0.0),
        put(NORTH_WALL, "STAT", "wall", [0.0, HALF], 0.0, 0.0),
        put(4, "STAT", "wall", [HALF, 0.0], 0.0, quarter),
        put(5, "STAT", "wall", [-HALF, 0.0], 0.0, quarter),
        put(10, "STAT", "bar", [0.0, 200.0], 0.0, 0.0),
        put(11, "STAT", "table", [-280.0, 0.0], 0.0, 0.0),
        put(12, "STAT", "table", [280.0, 0.0], 0.0, 0.0),
        put(13, "STAT", "table", [0.0, -300.0], 0.0, 0.0),
        put(TERMINAL, "TERM", "small", [120.0, 200.0], 110.0, 0.0),
        put(CONTAINER, "CONT", "small", [-400.0, 380.0], 0.0, 0.0),
        put(QUEST, "STAT", "can", [20.0, -320.0], 55.0, 0.0),
        put(BEAM, "STAT", "beam", [0.0, -120.0], 100.0, 0.0),
        reference(
            LAMP,
            "STAT",
            "meshes\\clutter\\lights\\hanginglamp01.nif",
            "lamp",
            &on_floor(),
            [-150.0, -200.0],
            105.0,
            0.0,
        ),
    ];
    let mut door = put(DOOR, "DOOR", "door", [-380.0, 100.0], 0.0, quarter);
    door.lock = Some(LockDoc {
        level: 50,
        key: None,
        flags: 0,
    });
    refs.push(door);
    let tables = [[-280.0, 0.0], [280.0, 0.0], [0.0, -300.0]];
    for (n, id) in CANS.enumerate() {
        let (xy, lift) = if n < 12 {
            let t = tables[n % 3];
            let k = (n / 3) as f32;
            ([t[0] - 20.0 + 13.0 * k, t[1] + 15.0], 55.0)
        } else {
            ([-160.0 + 40.0 * (n - 12) as f32, 190.0], 110.0)
        };
        refs.push(put(id, "STAT", "can", xy, lift, 0.0));
    }
    for (n, id) in PAPERS.enumerate() {
        let xy = [-300.0 + 65.0 * n as f32, -420.0 + 30.0 * (n % 3) as f32];
        refs.push(put(id, "STAT", "paper", xy, 0.0, 0.3 * n as f32));
    }
    refs
}

/// The hexes inside the room, 80 units clear of the walls.
fn walkers() -> BTreeSet<i32> {
    let plane = Plane::new(&projection(), window().x_west, window().y_north).unwrap();
    (0..40_000)
        .filter(|&h| {
            plane.hex_tile(h).is_some_and(|t| {
                let [x, y] = plane.source_of_tile(t);
                x.abs() <= f64::from(HALF) - 80.0 && y.abs() <= f64::from(HALF) - 80.0
            })
        })
        .collect()
}

fn indoors(declutter: Option<DeclutterProfile>) -> SlicerConfig {
    SlicerConfig {
        interior: Some(InteriorSlice {
            cut_height: 150.0,
            ..InteriorSlice::at(FLOOR_Z)
        }),
        open_hexes: walkers(),
        declutter,
        ..SlicerConfig::default()
    }
}

fn readable() -> DeclutterProfile {
    DeclutterProfile {
        protected_refs: [key(QUEST)].into_iter().collect(),
        ..DeclutterProfile::readable()
    }
}

fn slice(config: &SlicerConfig) -> Sliced {
    run(&assets(), &room(), &on_floor(), &big_budget(), config).expect("slices")
}

fn of(sliced: &Sliced, id: u32) -> Vec<&Placement> {
    let k = key(id);
    sliced
        .placements
        .iter()
        .filter(|p| p.reference == k)
        .collect()
}

fn skip_of(sliced: &Sliced, id: u32) -> Option<&SkipReason> {
    let k = key(id);
    sliced
        .skipped
        .iter()
        .find(|s| s.reference == k)
        .map(|s| &s.reason)
}

/// The topmost opaque screen row of the placements.
fn top_row(sliced: &Sliced, placements: &[&Placement]) -> i64 {
    let mut top = i64::MAX;
    for p in placements {
        let a = &sliced.art[p.art];
        let o = frame_origin(a, p);
        for y in 0..a.height as usize {
            if (0..a.width as usize).any(|x| a.pixels[y * a.width as usize + x] != 0) {
                top = top.min(o[1] + y as i64);
                break;
            }
        }
    }
    top
}

/// Hexes a reference blocks: its blocking placements, and the blocked
/// hexes near them.
fn blocking(sliced: &Sliced, id: u32) -> BTreeSet<i32> {
    of(sliced, id)
        .iter()
        .filter(|p| p.blocking)
        .map(|p| p.hex)
        .collect()
}

#[test]
fn the_profile_off_changes_nothing() {
    let plain = slice(&indoors(None));
    assert!(plain.report.declutter.is_none());
    assert!(!plain.skipped.iter().any(|s| s.reason.is_declutter()));
    // A profile that protects but turns no rule on draws the same bytes.
    let protecting = slice(&indoors(Some(DeclutterProfile::default())));
    assert_eq!(protecting.art, plain.art);
    assert_eq!(protecting.placements, plain.placements);
    assert_eq!(protecting.overlays, plain.overlays);
    assert_eq!(protecting.blocked_hexes, plain.blocked_hexes);
    assert_eq!(protecting.skipped, plain.skipped);
    let stats = protecting.report.declutter.as_ref().expect("stats");
    assert_eq!(stats.pruned_total(), 0);
    assert!(stats.protected_references >= 3);
    // The report without the pass prints as before: no declutter line.
    assert!(!plain.report.to_string().contains("Declutter"));
    assert!(protecting.report.to_string().contains("Declutter"));
    // And again: deterministic.
    assert_eq!(slice(&indoors(None)), plain);
}

#[test]
fn the_readable_profile_prunes_clutter_and_keeps_what_matters() {
    let plain = slice(&indoors(None));
    let sliced = slice(&indoors(Some(readable())));
    // Every can and paper is gone, by a declutter rule.
    for id in CANS.chain(PAPERS) {
        assert_eq!(skip_of(&plain, id), None, "{id} in the plain run");
        let reason = skip_of(&sliced, id).unwrap_or_else(|| panic!("{id} kept"));
        assert!(reason.is_declutter(), "{id}: {reason:?}");
        assert!(of(&sliced, id).is_empty());
    }
    // The beam and the hanging lamp hang over walkable floor.
    assert_eq!(skip_of(&sliced, BEAM), Some(&SkipReason::Overhang));
    assert_eq!(skip_of(&sliced, LAMP), Some(&SkipReason::Overhang));
    assert!(!of(&plain, BEAM).is_empty() && !of(&plain, LAMP).is_empty());
    // Protected things all stay, the quest object as small as a can too.
    for id in [TERMINAL, CONTAINER, DOOR, QUEST, 10, 11, 12, 13, NORTH_WALL] {
        assert!(!of(&sliced, id).is_empty(), "{id} lost");
        assert_eq!(skip_of(&sliced, id), None, "{id}");
    }
    let stats = sliced.report.declutter.as_ref().expect("stats");
    assert_eq!(stats.protected.get("caller"), Some(&1));
    assert!(stats.protected.get("kind").is_some_and(|n| *n >= 3));
    assert!(stats.pruned.get("clutter").is_some_and(|n| *n >= 20));
    assert_eq!(stats.pruned.get("overhang"), Some(&2));
    let stat = stats.by_kind.get("STAT").expect("STAT tally");
    assert!(stat.pruned >= 32 && stat.kept >= 9, "{stat:?}");
    // The outline is on the terminal, container and door sprites.
    assert!(stats.outlined >= 3, "{}", stats.outlined);
    assert!(sliced.report.to_string().contains("Declutter:"));
}

#[test]
fn the_near_wall_is_cut_to_a_kerb_and_still_blocks() {
    let without = DeclutterProfile {
        cutaway: None,
        ..readable()
    };
    let uncut = slice(&indoors(Some(without)));
    let sliced = slice(&indoors(Some(readable())));
    // The south wall stands between the camera and the room: its strips
    // lose everything above the kerb.
    let south_uncut = of(&uncut, SOUTH_WALL);
    let south = of(&sliced, SOUTH_WALL);
    assert!(!south_uncut.is_empty() && !south.is_empty());
    let (top_uncut, top) = (top_row(&uncut, &south_uncut), top_row(&sliced, &south));
    // 120 units of shell cut to 35: about 39 pixels lower.
    assert!(top > top_uncut + 25, "top {top} vs {top_uncut}");
    // It blocks every hex it blocked before.
    let mut blocks: BTreeSet<i32> = blocking(&sliced, SOUTH_WALL);
    blocks.extend(sliced.blocked_hexes.iter().copied());
    for h in blocking(&uncut, SOUTH_WALL) {
        assert!(blocks.contains(&h), "hex {h} opened");
    }
    // The far wall hides nobody: untouched.
    let north_uncut = of(&uncut, NORTH_WALL);
    let north = of(&sliced, NORTH_WALL);
    assert_eq!(north.len(), north_uncut.len());
    for (a, b) in north.iter().zip(&north_uncut) {
        assert_eq!(sliced.art[a.art], uncut.art[b.art]);
        assert_eq!(
            (a.hex, a.displacement, a.blocking),
            (b.hex, b.displacement, b.blocking)
        );
    }
    let c = &sliced.report.declutter.as_ref().unwrap().cutaway;
    assert!(c.strips_cut > 0 && c.pixels_removed > 0, "{c:?}");
    assert!(c.walkers_covered_after < c.walkers_covered_before, "{c:?}");
    // The open floor grows against the room without the pass.
    let d = sliced.report.declutter.as_ref().unwrap();
    let (before, after) = (d.floor_before.unwrap(), d.floor_after.unwrap());
    assert!(
        after.open_ratio() > before.open_ratio(),
        "{before} -> {after}"
    );
    assert!(after.covered < before.covered, "{before} -> {after}");
    assert_eq!(before.floor_hexes, after.floor_hexes);
    // The public metric agrees with the report on the plain run.
    let plain = slice(&indoors(None));
    let mut floor = walkers();
    floor.extend(sliced.report.interior.as_ref().unwrap().floor_hexes.iter());
    let measured = floor_stats(&plain, &walkers(), &floor, [48, 80], 960);
    assert_eq!(measured, before);
}

#[test]
fn a_shell_gets_the_lower_cut_and_a_prop_keeps_its_own() {
    let plain = slice(&indoors(None));
    let shells = DeclutterProfile {
        cutaway: None,
        ..readable()
    };
    let sliced = slice(&indoors(Some(shells)));
    // Walls are shells: cut at 120 instead of 150, about 14 pixels lower.
    let north = top_row(&sliced, &of(&sliced, NORTH_WALL));
    let north_plain = top_row(&plain, &of(&plain, NORTH_WALL));
    assert!(north > north_plain + 8, "{north} vs {north_plain}");
    // The bar (110 tall, 60 deep) is a prop: whole.
    assert_eq!(
        top_row(&sliced, &of(&sliced, 10)),
        top_row(&plain, &of(&plain, 10))
    );
    let d = sliced.report.declutter.as_ref().unwrap();
    assert_eq!(d.shell_cut_references, 4);
}

#[test]
fn readable_runs_are_deterministic() {
    let config = indoors(Some(readable()));
    let a = slice(&config);
    let b = slice(&config);
    assert_eq!(a, b);
    assert_eq!(a.report.to_string(), b.report.to_string());
}

#[test]
fn a_dense_pile_keeps_its_largest() {
    // Ten small boxes on one spot outdoors, 30 to 39 units across.
    let mut assets = TestAssets::new();
    let ground = flat();
    let mut refs = Vec::new();
    for n in 0..10u32 {
        let hash = format!("box{n}");
        assets = assets.with(
            &hash,
            mesh(&nif::fixture::box_mesh([30.0 + n as f32, 30.0, 40.0], WALL)),
        );
        refs.push(reference(
            300 + n,
            "STAT",
            &format!("meshes\\test\\{hash}.nif"),
            &hash,
            &ground,
            [3.0 * n as f32, 2.0 * (n % 3) as f32],
            0.0,
            0.0,
        ));
    }
    let profile = DeclutterProfile {
        density: Some(DensityRule::default()),
        ..DeclutterProfile::default()
    };
    let config = SlicerConfig {
        declutter: Some(profile),
        ..SlicerConfig::default()
    };
    let sliced = run(&assets, &refs, &ground, &big_budget(), &config).expect("slices");
    let kept: BTreeSet<String> = sliced
        .placements
        .iter()
        .map(|p| p.reference.clone())
        .collect();
    assert_eq!(kept, [key(309), key(308)].into_iter().collect());
    let density = sliced
        .skipped
        .iter()
        .filter(|s| s.reason == SkipReason::ClutterDensity)
        .count();
    assert_eq!(density, 8);
    // Without the rule, all ten stand.
    let plain = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    let all: BTreeSet<&String> = plain.placements.iter().map(|p| &p.reference).collect();
    assert_eq!(all.len(), 10);
}

#[test]
fn micro_clutter_needs_every_threshold_and_protection_wins() {
    let ground = flat();
    let assets = TestAssets::new()
        .with("can", mesh(&nif::fixture::box_mesh([8.0, 8.0, 12.0], ROOF)))
        .with(
            "post",
            mesh(&nif::fixture::box_mesh([8.0, 8.0, 90.0], ROOF)),
        );
    let mut locked = reference(
        2,
        "STAT",
        "meshes\\test\\can.nif",
        "can",
        &ground,
        [100.0, 0.0],
        0.0,
        0.0,
    );
    locked.lock = Some(LockDoc {
        level: 1,
        key: None,
        flags: 0,
    });
    let mut named = reference(
        3,
        "STAT",
        "meshes\\test\\can.nif",
        "can",
        &ground,
        [200.0, 0.0],
        0.0,
        0.0,
    );
    named.editor_id = Some("QuestCan".to_owned());
    let refs = vec![
        reference(
            1,
            "STAT",
            "meshes\\test\\can.nif",
            "can",
            &ground,
            [0.0, 0.0],
            0.0,
            0.0,
        ),
        locked,
        named,
        // Thin but tall: not micro-clutter.
        reference(
            4,
            "STAT",
            "meshes\\test\\post.nif",
            "post",
            &ground,
            [300.0, 0.0],
            0.0,
            0.0,
        ),
        // An item is protected by kind.
        reference(
            5,
            "MISC",
            "meshes\\test\\can.nif",
            "can",
            &ground,
            [400.0, 0.0],
            0.0,
            0.0,
        ),
    ];
    let config = SlicerConfig {
        declutter: Some(DeclutterProfile {
            clutter: Some(ClutterRules::default()),
            ..DeclutterProfile::default()
        }),
        ..SlicerConfig::default()
    };
    let sliced = run(&assets, &refs, &ground, &big_budget(), &config).unwrap();
    assert_eq!(skip_of(&sliced, 1), Some(&SkipReason::Clutter));
    for id in 2..=5 {
        assert_eq!(skip_of(&sliced, id), None, "{id}");
    }
    let d = sliced.report.declutter.as_ref().unwrap();
    assert_eq!(d.protected.get("lock"), Some(&1));
    assert_eq!(d.protected.get("editor id"), Some(&1));
    assert_eq!(d.protected.get("kind"), Some(&1));
    // A pattern drops whatever the size; unprotecting items lets the rule
    // reach them.
    let mut profile = DeclutterProfile {
        clutter: Some(ClutterRules {
            patterns: vec!["\\post".to_owned()],
            ..ClutterRules::default()
        }),
        ..DeclutterProfile::default()
    };
    profile.protected_kinds.retain(|k| k != "MISC");
    let config = SlicerConfig {
        declutter: Some(profile),
        ..SlicerConfig::default()
    };
    let sliced = run(&assets, &refs, &ground, &big_budget(), &config).unwrap();
    assert_eq!(
        skip_of(&sliced, 4),
        Some(&SkipReason::ClutterPattern("\\post".to_owned()))
    );
    assert_eq!(skip_of(&sliced, 5), Some(&SkipReason::Clutter));
}

#[test]
fn decals_drop_or_fade() {
    let ground = flat();
    let assets = TestAssets::new().with("stain", mesh(&nif::fixture::quad([200.0, 200.0], ROOF)));
    let refs = vec![reference(
        1,
        "STAT",
        "meshes\\decals\\waterstain01.nif",
        "stain",
        &ground,
        [0.0, 0.0],
        0.0,
        0.0,
    )];
    let with = |decals| SlicerConfig {
        declutter: Some(DeclutterProfile {
            decals,
            ..DeclutterProfile::default()
        }),
        ..SlicerConfig::default()
    };
    let kept = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &with(crate::DecalMode::Keep),
    )
    .unwrap();
    assert_eq!(kept.overlays.len(), 1);
    let dropped = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &with(crate::DecalMode::Drop),
    )
    .unwrap();
    assert!(dropped.overlays.is_empty());
    assert_eq!(skip_of(&dropped, 1), Some(&SkipReason::Decal));
    let faded = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &with(crate::DecalMode::Fade(0.5)),
    )
    .unwrap();
    assert_eq!(faded.overlays.len(), 1);
    let (a, b) = (&kept.overlays[0], &faded.overlays[0]);
    assert_eq!((a.origin, a.width, a.height), (b.origin, b.width, b.height));
    for (pa, pb) in a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
    {
        assert_eq!(pa[..3], pb[..3]);
        assert_eq!(pb[3], if pa[3] == 0 { 0 } else { 128 });
    }
    assert_eq!(faded.report.declutter.as_ref().unwrap().decals_faded, 1);
}

#[test]
fn the_cutaway_is_opt_in_outdoors() {
    // The same south wall outdoors: walkers north of it.
    let ground = flat();
    let assets = assets();
    let refs = vec![reference(
        1,
        "STAT",
        "meshes\\test\\wall.nif",
        "wall",
        &ground,
        [0.0, -HALF],
        0.0,
        0.0,
    )];
    let config = |exteriors| SlicerConfig {
        landmark_walkers: Some(walkers()),
        declutter: Some(DeclutterProfile {
            cutaway: Some(Cutaway {
                exteriors,
                ..Cutaway::default()
            }),
            ..DeclutterProfile::default()
        }),
        ..SlicerConfig::default()
    };
    let plain = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    let off = run(&assets, &refs, &ground, &big_budget(), &config(false)).unwrap();
    assert_eq!(off.placements, plain.placements);
    assert_eq!(off.art, plain.art);
    let on = run(&assets, &refs, &ground, &big_budget(), &config(true)).unwrap();
    let c = &on.report.declutter.as_ref().unwrap().cutaway;
    assert!(c.strips_cut > 0, "{c:?}");
    fn all(s: &Sliced) -> Vec<&Placement> {
        s.placements.iter().collect()
    }
    assert!(top_row(&on, &all(&on)) > top_row(&plain, &all(&plain)) + 40);
}

#[test]
fn the_ladder_never_demotes_a_protected_terminal() {
    let ground = flat();
    let assets = TestAssets::standard().with(
        "term",
        mesh(&nif::fixture::box_mesh([40.0, 40.0, 60.0], ROOF)),
    );
    // Two scenery sprites of one entry each, one entry free: the ladder
    // demotes one, the terminal first by name.
    let refs = vec![
        reference(
            1,
            "TERM",
            "meshes\\a\\terminal.nif",
            "term",
            &ground,
            [0.0, 0.0],
            0.0,
            0.0,
        ),
        reference(
            2,
            "STAT",
            "meshes\\b\\crate.nif",
            "crate",
            &ground,
            [300.0, 0.0],
            0.0,
            0.0,
        ),
    ];
    let budget = Budget {
        scenery: 1,
        ..big_budget()
    };
    let plain = run(&assets, &refs, &ground, &budget, &SlicerConfig::default()).unwrap();
    assert_eq!(plain.report.ladder[0].models, ["meshes\\a\\terminal.nif"]);
    assert!(of(&plain, 1).is_empty());
    let config = SlicerConfig {
        declutter: Some(DeclutterProfile::default()),
        ..SlicerConfig::default()
    };
    let protected = run(&assets, &refs, &ground, &budget, &config).unwrap();
    assert_eq!(protected.report.ladder[0].models, ["meshes\\b\\crate.nif"]);
    assert_eq!(of(&protected, 1).len(), 1);
    assert!(of(&protected, 2).is_empty());
}

#[test]
fn decorative_items_are_pruned_unless_referenced() {
    // Three "MISC" items on a table: two named decorative, one of those
    // also a quest reference; and a third left alone. The kind protects all
    // of them otherwise.
    let mut refs = room();
    let table = [280.0, 0.0];
    for (n, id) in [300u32, 301, 302].into_iter().enumerate() {
        refs.push(put(
            id,
            "MISC",
            "small",
            [table[0] - 20.0 + 20.0 * n as f32, table[1] - 15.0],
            55.0,
            0.0,
        ));
    }
    let mut profile = readable();
    profile.decorative_items = [key(300), key(301)].into_iter().collect();
    profile.protected_refs.insert(key(301));
    let config = indoors(Some(profile));
    let sliced = run(&assets(), &refs, &on_floor(), &big_budget(), &config).expect("slices");
    assert_eq!(skip_of(&sliced, 300), Some(&SkipReason::DecorativeItem));
    assert!(of(&sliced, 300).is_empty());
    assert_eq!(skip_of(&sliced, 301), None, "referenced: kept");
    assert!(!of(&sliced, 301).is_empty());
    assert_eq!(skip_of(&sliced, 302), None, "not named: kept by kind");
    assert!(!of(&sliced, 302).is_empty());
    let stats = sliced.report.declutter.as_ref().expect("stats");
    assert_eq!(stats.pruned.get("decorative item"), Some(&1));
    assert!(SkipReason::DecorativeItem.is_declutter());
    // An empty set changes nothing.
    let plain = run(
        &assets(),
        &refs,
        &on_floor(),
        &big_budget(),
        &indoors(Some(readable())),
    )
    .expect("slices");
    assert_eq!(skip_of(&plain, 300), None);
}

#[test]
fn multiply_blended_shapes_are_recognised() {
    let mut m = nif::Material::default();
    assert!(!crate::multiplies(&m), "no alpha property");
    // Blending on, source zero, destination source colour: a multiply.
    m.alpha_flags = Some(1 | (1 << 1) | (2 << 5) | 0x1000);
    assert!(crate::multiplies(&m));
    // Source destination colour, destination zero: a multiply too.
    m.alpha_flags = Some(1 | (4 << 1) | (1 << 5));
    assert!(crate::multiplies(&m));
    // Ordinary alpha blending (source alpha, inverse source alpha).
    m.alpha_flags = Some(1 | (6 << 1) | (7 << 5));
    assert!(!crate::multiplies(&m));
    // The multiply factors with blending off.
    m.alpha_flags = Some((1 << 1) | (2 << 5));
    assert!(!crate::multiplies(&m));
    // Additive glows (source alpha, destination one), textured or not.
    m.alpha_flags = Some(0x100d);
    assert!(crate::adds(&m) && crate::blends_onto_scene(&m));
    m.textures = vec!["textures\\effects\\fxwhite.dds".to_owned()];
    assert!(crate::blends_onto_scene(&m));
    // A textured multiply decal is drawn; an untextured one is not.
    m.alpha_flags = Some(1 | (1 << 1) | (2 << 5));
    assert!(!crate::blends_onto_scene(&m));
    m.textures.clear();
    assert!(crate::blends_onto_scene(&m));
    // Ordinary alpha blending is drawn.
    m.alpha_flags = Some(1 | (6 << 1) | (7 << 5));
    assert!(!crate::blends_onto_scene(&m));
}

#[test]
fn a_shape_sampling_one_texel_is_a_flat_fill() {
    let mut mesh = mesh(&nif::fixture::box_mesh([100.0, 100.0, 300.0], WALL));
    let mut p = mesh.primitives.remove(0);
    p.material.textures = vec!["textures\\test\\wall.dds".to_owned()];
    let n = p.positions.len();
    p.uvs = Some((0..n).map(|i| [i as f32 / n as f32, 0.5]).collect());
    assert!(!crate::fills_flat(&p), "a spread of texture coordinates");
    p.uvs = Some(vec![[0.8146, 0.0050]; n]);
    assert!(p.indices.len() >= 24);
    assert!(crate::fills_flat(&p));
    // Untextured, or too small to matter: not a fill.
    let mut bare = p.clone();
    bare.material.textures.clear();
    assert!(!crate::fills_flat(&bare));
    let mut tiny = p.clone();
    tiny.indices.truncate(6);
    assert!(!crate::fills_flat(&tiny));
}
