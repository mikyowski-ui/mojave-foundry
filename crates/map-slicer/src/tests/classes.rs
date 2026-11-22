//! Each classification rule.

use super::*;
use crate::{Class, MeshFacts, Role, SkipReason, classify};

fn facts(hash: &str) -> MeshFacts {
    MeshFacts::of(&TestAssets::standard().mesh(hash).unwrap())
}

fn class_of(r: &Reference, hash: &str) -> Class {
    classify(r, Some(&facts(hash)), 140.0, &SlicerConfig::default())
}

fn make(kind: &str, model: &str, hash: &str) -> Reference {
    reference(1, kind, model, hash, &flat(), [0.0, 0.0], 0.0, 0.0)
}

#[test]
fn flags_and_kinds_skip() {
    let mut r = make("STAT", "meshes\\test\\house.nif", "house");
    r.flags.deleted = true;
    assert_eq!(class_of(&r, "house"), Class::Skip(SkipReason::Deleted));
    r.flags.deleted = false;
    r.flags.initially_disabled = true;
    assert_eq!(class_of(&r, "house"), Class::Skip(SkipReason::Disabled));
    let actor = make("NPC_", "meshes\\test\\house.nif", "house");
    assert_eq!(
        class_of(&actor, "house"),
        Class::Skip(SkipReason::Kind("NPC_".to_owned()))
    );
}

#[test]
fn no_mesh_and_patterns_skip() {
    let mut r = make("STAT", "meshes\\test\\house.nif", "house");
    r.mesh = None;
    assert_eq!(class_of(&r, "house"), Class::Skip(SkipReason::NoMesh));
    let r = make("STAT", "meshes\\test\\house.nif", "house");
    assert_eq!(
        classify(&r, None, 140.0, &SlicerConfig::default()),
        Class::Skip(SkipReason::NoMesh)
    );
    let effect = make("STAT", "meshes\\effects\\glow.nif", "house");
    assert_eq!(
        class_of(&effect, "house"),
        Class::Skip(SkipReason::Pattern("effects\\".to_owned()))
    );
    let empty = MeshFacts::of(&StaticMesh::default());
    assert_eq!(
        classify(&r, Some(&empty), 140.0, &SlicerConfig::default()),
        Class::Skip(SkipReason::NoGeometry)
    );
}

#[test]
fn flat_meshes_go_to_the_ground() {
    let road = make("STAT", "meshes\\test\\strip.nif", "road");
    assert_eq!(class_of(&road, "road"), Class::GroundFlat);
    // A tall mesh whose path says it is road is flattened too.
    let named = make("STAT", "meshes\\test\\roadkerb.nif", "slab");
    assert_eq!(class_of(&named, "slab"), Class::GroundFlat);
}

#[test]
fn rocks_and_huge_meshes_are_relief() {
    let rock = make("STAT", "meshes\\test\\rockpile.nif", "slab");
    assert_eq!(class_of(&rock, "slab"), Class::GroundRelief);
    let huge = make("STAT", "meshes\\test\\plateau.nif", "huge");
    assert_eq!(class_of(&huge, "huge"), Class::GroundRelief);
}

#[test]
fn small_props_are_sprites_and_roles_follow_the_kind() {
    let prop = make("STAT", "meshes\\test\\crate.nif", "crate");
    assert_eq!(class_of(&prop, "crate"), Class::Sprite(Role::Scenery));
    let door = make("DOOR", "meshes\\test\\house.nif", "house");
    assert_eq!(class_of(&door, "house"), Class::Sprite(Role::Door));
    let chest = make("CONT", "meshes\\test\\crate.nif", "crate");
    assert_eq!(class_of(&chest, "crate"), Class::Sprite(Role::Container));
    let can = make("MISC", "meshes\\test\\crate.nif", "crate");
    assert_eq!(class_of(&can, "crate"), Class::Sprite(Role::Item));
    assert_eq!(Role::Item.category(), crate::ArtCategory::Items);
    assert_eq!(Role::Door.category(), crate::ArtCategory::Scenery);
    assert!(!Role::Item.blocking() && Role::Scenery.blocking());
}

#[test]
fn buildings_and_long_thin_walls_are_sliced() {
    let house = make("STAT", "meshes\\test\\house.nif", "house");
    assert_eq!(class_of(&house, "house"), Class::Sliced);
    // Small in area, but five metres long: not a sprite.
    let fence = make("STAT", "meshes\\test\\fence.nif", "fence");
    assert_eq!(class_of(&fence, "fence"), Class::Sliced);
    // Scale counts: the crate scaled by 4 covers about eight hexes.
    let mut big = make("STAT", "meshes\\test\\crate.nif", "crate");
    big.transform.scale = 4.0;
    assert_eq!(class_of(&big, "crate"), Class::Sliced);
}

#[test]
fn a_sparse_mesh_is_one_sprite_or_skipped() {
    // Bounds of a big box, but almost no triangle area: wires.
    let wires = MeshFacts {
        bounds: Some([[-1000.0, -50.0, 0.0], [1000.0, 50.0, 900.0]]),
        surface_area: Some(2000.0),
        triangles: 12,
    };
    let r = make("STAT", "meshes\\test\\pole.nif", "house");
    assert_eq!(
        classify(&r, Some(&wires), 140.0, &SlicerConfig::default()),
        Class::Sprite(Role::Scenery)
    );
    let skip = SlicerConfig {
        sparse_as_sprite: false,
        ..SlicerConfig::default()
    };
    assert_eq!(
        classify(&r, Some(&wires), 140.0, &skip),
        Class::Skip(SkipReason::Sparse)
    );
    let off = SlicerConfig {
        sparse_ratio: 0.0,
        ..SlicerConfig::default()
    };
    assert_eq!(classify(&r, Some(&wires), 140.0, &off), Class::Sliced);
}

#[test]
fn a_rotated_footprint_keeps_its_area() {
    let f = facts("house");
    let straight = crate::Footprint::of(&Transform::default(), f.bounds.unwrap());
    let turned = crate::Footprint::of(
        &Transform {
            rotation: [0.0, 0.0, 0.6],
            ..Transform::default()
        },
        f.bounds.unwrap(),
    );
    assert!((straight.area - 840.0 * 560.0).abs() < 1.0);
    assert!((turned.area - straight.area).abs() < 1.0);
    assert!((straight.hexes(140.0) - 96.0).abs() < 1e-6);
}

#[test]
fn trigger_volumes_skip() {
    let mut r = make("ACTI", "meshes\\test\\house.nif", "house");
    r.trigger_volume = true;
    assert_eq!(
        class_of(&r, "house"),
        Class::Skip(SkipReason::TriggerVolume)
    );
    assert_eq!(SkipReason::TriggerVolume.label(), "trigger volume");
}

#[test]
fn huge_meshes_that_are_not_terrain_are_landmarks() {
    let tower = make("STAT", "meshes\\test\\casinotower.nif", "huge");
    assert_eq!(class_of(&tower, "huge"), Class::Landmark);
    // Terrain by its path stays relief, at any size.
    let hill = make("STAT", "meshes\\landscape\\test\\bigslope.nif", "huge");
    assert_eq!(class_of(&hill, "huge"), Class::GroundRelief);
    // Off, a huge mesh is relief as in the first pass.
    let off = SlicerConfig {
        landmarks: false,
        ..SlicerConfig::default()
    };
    assert_eq!(
        classify(&tower, Some(&facts("huge")), 140.0, &off),
        Class::GroundRelief
    );
    // Below the threshold nothing changes.
    let house = make("STAT", "meshes\\test\\casinotower.nif", "house");
    assert_eq!(class_of(&house, "house"), Class::Sliced);
}

#[test]
fn patterns_match_segments_and_word_starts_not_substrings() {
    // What the substring rule got wrong: rockets and a town named for
    // boulders are buildings.
    for model in [
        "meshes\\test\\rocketcrane01.nif",
        "meshes\\test\\rocket\\launchtower.nif",
        "meshes\\dungeons\\bouldercity\\bcruinhouse01.nif",
        "meshes\\architecture\\bouldercity\\bcstorefront02.nif",
        "meshes\\test\\shamrocksign.nif",
    ] {
        let r = make("STAT", model, "house");
        assert_eq!(class_of(&r, "house"), Class::Sliced, "{model}");
    }
    // The shapes of the paths the defaults were tuned on keep their class.
    for model in [
        "meshes\\landscape\\test_rocks\\tst_rockcanyon06.nif",
        "meshes\\landscape\\test_rocks\\tst_cliffcanyonl01.nif",
        "meshes\\landscape\\test_rocks\\tst_rockcanyonrubblepile03.nif",
        "meshes\\landscape\\rocks\\cliffs\\cliffverti_t1.nif",
        "meshes\\landscape\\rocks\\cliffs\\canyontestentrance01.nif",
        "meshes\\architecture\\suburban\\rubblepiles\\testrubblepile01.nif",
        "meshes\\test\\boulder01.nif",
        "meshes\\test\\nvboulderpile.nif",
        "meshes\\dungeons\\bouldercity\\boulder02.nif",
        "meshes\\test\\rocket\\rockpile01.nif",
    ] {
        let r = make("STAT", model, "house");
        assert_eq!(class_of(&r, "house"), Class::GroundRelief, "{model}");
    }
    for model in [
        "meshes\\landscape\\roads\\testroadcurve01.nif",
        "meshes\\scol\\scolroadstraight01b.nif",
        "meshes\\scol\\scolparkinglottest01.nif",
        "meshes\\architecture\\test\\parkinglot\\testbarricade09.nif",
    ] {
        let r = make("STAT", model, "slab");
        assert_eq!(class_of(&r, "slab"), Class::GroundFlat, "{model}");
    }
    for model in [
        "meshes\\landscape\\trees\\testjoshua01.nif",
        "meshes\\landscape\\plants\\testgourd.nif",
        "meshes\\test\\shrubs\\creosotebushxl.nif",
        "meshes\\test\\wastelandshrub02.nif",
        "meshes\\test\\nv_barrelcactus09.nif",
    ] {
        let r = make("STAT", model, "house");
        assert_eq!(
            class_of(&r, "house"),
            Class::Sprite(Role::Scenery),
            "{model}"
        );
    }
    for (model, pattern) in [
        ("meshes\\test\\testwallmarker.nif", "marker"),
        ("meshes\\markertestheading.nif", "marker"),
        ("meshes\\effects\\ambient\\fxtest01.nif", "effects\\"),
        ("meshes\\test\\fx\\glow.nif", "fx\\"),
        ("meshes\\test\\fxglow.nif", "\\fx"),
    ] {
        let r = make("STAT", model, "house");
        assert_eq!(
            class_of(&r, "house"),
            Class::Skip(SkipReason::Pattern(pattern.to_owned())),
            "{model}"
        );
    }
}

/// A 700 × 700 box 40 units tall, or a strip of it.
fn slab_facts(size: [f32; 3], area: Option<f64>) -> MeshFacts {
    MeshFacts {
        bounds: Some([
            [-size[0] / 2.0, -size[1] / 2.0, 0.0],
            [size[0] / 2.0, size[1] / 2.0, size[2]],
        ]),
        surface_area: area,
        triangles: 12,
    }
}

#[test]
fn low_paved_slabs_flatten_but_kerbs_stay_upright() {
    let config = SlicerConfig::default();
    let class = |model: &str, f: &MeshFacts| {
        classify(&make("STAT", model, "slab"), Some(f), 140.0, &config)
    };
    let box_area = |s: [f32; 3]| {
        let [x, y, z] = s.map(f64::from);
        Some(2.0 * (x * y + x * z + y * z))
    };
    for height in [36.0, 40.0, 46.0] {
        let size = [700.0, 700.0, height];
        let slab = slab_facts(size, box_area(size));
        for model in [
            "meshes\\test\\airbase\\runway01.nif",
            "meshes\\test\\strip\\sidewalk04.nif",
            "meshes\\test\\nvtarmacpatch.nif",
        ] {
            assert_eq!(class(model, &slab), Class::GroundFlat, "{model} {height}");
        }
        // Known only by its bounds, a slab still flattens.
        assert_eq!(
            class("meshes\\test\\runway02.nif", &slab_facts(size, None)),
            Class::GroundFlat
        );
        // The same box under another name is not a slab.
        assert_eq!(class("meshes\\test\\planter01.nif", &slab), Class::Sliced);
    }
    // A kerb: narrow.
    let kerb = [700.0, 20.0, 40.0];
    assert_eq!(
        class(
            "meshes\\test\\sidewalkedge01.nif",
            &slab_facts(kerb, box_area(kerb))
        ),
        Class::Sliced
    );
    assert_eq!(
        class(
            "meshes\\test\\sidewalkcurb01.nif",
            &slab_facts([700.0, 700.0, 40.0], None)
        ),
        Class::Sliced
    );
    // A kerb ring around a block: a wide box with little area.
    let ring = slab_facts([700.0, 700.0, 40.0], Some(4.0 * 700.0 * 60.0));
    assert_eq!(
        class("meshes\\test\\sidewalkring01.nif", &ring),
        Class::Sliced
    );
    // Taller than a slab: upright.
    let tall = [700.0, 700.0, 60.0];
    assert_eq!(
        class(
            "meshes\\test\\runway03.nif",
            &slab_facts(tall, box_area(tall))
        ),
        Class::Sliced
    );
}

#[test]
fn the_footprint_width_is_its_narrowest_side() {
    let f = crate::Footprint::of(
        &Transform {
            rotation: [0.0, 0.0, 0.6],
            ..Transform::default()
        },
        [[-350.0, -10.0, 0.0], [350.0, 10.0, 40.0]],
    );
    assert!((f.width() - 20.0).abs() < 1e-3, "{}", f.width());
}
