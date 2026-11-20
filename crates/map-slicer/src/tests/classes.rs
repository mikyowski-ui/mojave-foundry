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
