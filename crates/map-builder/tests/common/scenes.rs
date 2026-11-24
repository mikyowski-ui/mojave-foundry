//! Scene builders shared by several test files and by the golden
//! digests (`tests/golden.rs`).

use std::collections::BTreeMap;

use map_builder::Role;
use map_builder::walk::{Frame, HEXES};
use mir::{CellLocation, FormKey};

use super::*;

pub fn full_base() -> BTreeMap<String, Vec<u8>> {
    let mut base = base_files(10);
    base.extend(critter_base_files());
    base
}

/// The town with a full base and two sprite sets (a third unused).
pub fn scene() -> Scene {
    let mut scene = town();
    scene.base = full_base();
    scene.critter_art = vec![
        sprite_set("townsman", 1),
        sprite_set("townswoman", 2),
        sprite_set("unused", 3),
    ];
    scene
}

/// The town plus one door teleporting into an interior.
pub fn door_scene() -> Scene {
    let mut scene = scene();
    let frame = scene.frame();
    let hex = hex_at(&frame, 48.0, 48.0);
    let [x, y] = hex_point(&frame, hex);
    let centre = scene
        .cells
        .iter()
        .position(|c| matches!(c.location, CellLocation::Exterior { x: 0, y: 0, .. }))
        .unwrap();
    scene.cells[centre].references.push(teleport_door(
        0xb01,
        [x, y, 100.0],
        0xc01,
        0xd01,
        Some("SynthInterior"),
        None,
    ));
    scene.cells[centre]
        .references
        .sort_by(|a, b| a.form.cmp(&b.form));
    scene.placements.push(placement(
        4,
        hex,
        Role::Door,
        true,
        &FormKey::new(PLUGIN, 0xb01).to_string(),
    ));
    scene
}

/// Four critters: two the same character, one another name on the same
/// art, one on the second set.
pub fn populated() -> Scene {
    let mut scene = scene();
    let frame = scene.frame();
    let at = |s, t| hex_point(&frame, hex_at(&frame, s, t));
    scene.critters = vec![
        critter(1, "synthetic.esm:00a101", "Mae", at(46.0, 52.0), 0.5),
        critter(0, "synthetic.esm:00a102", "Townsman", at(50.0, 52.0), 2.0),
        critter(0, "synthetic.esm:00a103", "Townsman", at(54.0, 52.0), -1.0),
        critter(0, "synthetic.esm:00a104", "Joe", at(50.0, 56.0), 3.0),
    ];
    scene.critters[0].female = true;
    scene.critters[0].hit_points = 18;
    scene
}

/// The town scene plus two doors into one interior, one into another, one
/// to another exterior position, and one whose destination is unknown.
pub fn doors_scene() -> Scene {
    let mut scene = town();
    let frame = scene.frame();
    let door_art = 4;
    let specs = [
        (0xb01, hex_at(&frame, 48.0, 48.0), 0xc01, 0xd01, None),
        (0xb02, hex_at(&frame, 56.0, 48.0), 0xc02, 0xd01, None),
        (0xb03, hex_at(&frame, 48.0, 57.0), 0xc03, 0xd02, None),
        (
            0xb04,
            hex_at(&frame, 57.0, 57.0),
            0xc04,
            0xd03,
            Some((1, 1)),
        ),
    ];
    let centre = scene
        .cells
        .iter()
        .position(|c| matches!(c.location, CellLocation::Exterior { x: 0, y: 0, .. }))
        .unwrap();
    for (form, hex, destination, interior, exterior) in specs {
        let [x, y] = hex_point(&frame, hex);
        scene.cells[centre].references.push(teleport_door(
            form,
            [x, y, 100.0],
            destination,
            interior,
            Some("SynthInterior"),
            exterior,
        ));
        scene.placements.push(placement(
            door_art,
            hex,
            Role::Door,
            true,
            &FormKey::new(PLUGIN, form).to_string(),
        ));
    }
    // A door whose teleport has no destination cell.
    let hex = hex_at(&frame, 52.0, 58.0);
    let [x, y] = hex_point(&frame, hex);
    let mut lost = teleport_door(0xb05, [x, y, 100.0], 0xc05, 0xd04, None, None);
    lost.teleport.as_mut().unwrap().cell = None;
    scene.cells[centre].references.push(lost);
    scene.placements.push(placement(
        door_art,
        hex,
        Role::Door,
        true,
        &FormKey::new(PLUGIN, 0xb05).to_string(),
    ));
    // References must stay sorted by form for `CellDoc::reference`.
    scene.cells[centre]
        .references
        .sort_by(|a, b| a.form.cmp(&b.form));
    scene
}

/// Every hex whose centre's tile coordinates `keep` accepts.
pub fn hexes_where(frame: &Frame, keep: impl Fn(f64, f64) -> bool) -> Vec<i32> {
    (0..HEXES as i32)
        .filter(|hex| frame.hex_centre_tile(*hex).is_some_and(|[s, t]| keep(s, t)))
        .collect()
}

/// The centre cell's index in a town scene's cells.
pub fn centre_cell(scene: &Scene) -> usize {
    scene
        .cells
        .iter()
        .position(|c| matches!(c.location, CellLocation::Exterior { x: 0, y: 0, .. }))
        .unwrap()
}

/// Add a door on `hex` teleporting into interior `interior`, whose own
/// door teleports back to `back` (source space), when given.
pub fn add_door(scene: &mut Scene, hex: i32, form: u32, interior: u32, back: Option<[f32; 2]>) {
    let frame = scene.frame();
    let [x, y] = hex_point(&frame, hex);
    let centre = centre_cell(scene);
    let inner_door = form + 0x100;
    scene.cells[centre].references.push(teleport_door(
        form,
        [x, y, 100.0],
        inner_door,
        interior,
        Some("SynthInterior"),
        None,
    ));
    scene.cells[centre]
        .references
        .sort_by(|a, b| a.form.cmp(&b.form));
    scene.placements.push(placement(
        4,
        hex,
        Role::Door,
        true,
        &FormKey::new(PLUGIN, form).to_string(),
    ));
    if let Some([bx, by]) = back {
        let mut cell = exterior_cell(9, 9);
        cell.form = FormKey::new(PLUGIN, interior);
        cell.location = CellLocation::Interior;
        let mut door = teleport_door(
            inner_door,
            [0.0, 0.0, 0.0],
            form,
            0x7777,
            None,
            Some((0, 0)),
        );
        door.teleport.as_mut().unwrap().position = [bx, by, 100.0];
        cell.references.push(door);
        scene.cells.push(cell);
    }
}

/// The town cut along tile row 52 (two hexes deep, far wider than the
/// navmesh), with a teleporting door in the smaller, southern part.
pub fn cut_town() -> (Scene, Vec<i32>) {
    let mut scene = town();
    let frame = scene.frame();
    let line = hexes_where(&frame, |s, t| {
        (25.0..75.0).contains(&s) && (52.0..53.0).contains(&t)
    });
    // The town's own item stands on the line; keep it, it blocks nothing.
    scene.blocked.extend(&line);
    let door = hex_at(&frame, 50.0, 57.0);
    add_door(&mut scene, door, 0xb01, 0xd01, None);
    (scene, line)
}
