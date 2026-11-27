//! The readability turn (`world_yaw_deg`) at the builder: a town and a
//! saloon linked both ways, every source input turned by `mir::WorldYaw`
//! as the map stage turns it (the cells' references and navmeshes about
//! each map's window centre, each teleport by the turn of the map it leads
//! to, the people's positions and headings, the slicer's placements and
//! blocked hexes as the turned slicer would place them), then built.
//!
//! For 0, 90, 180 and 270 degrees: both doors link both ways, every grid
//! arrives on a walkable hex, the residents stand on the turned image of
//! their unturned hex (within one hex: the lattice is not invariant under
//! a quarter turn, odd columns sit a quarter square lower) and face the hex
//! direction nearest their turned heading, the walkable count stays within
//! the boundary tolerance (a fifth of the count, at least 12 hexes: each
//! boundary hex of a turned floor may come or go), two builds give the
//! same bytes, and 0 degrees is the untouched build byte for byte.

mod common;

use std::collections::BTreeMap;

use common::saloon::{DOOR, SALOON};
use common::scenes::full_base;
use common::*;
use map_builder::{BuiltLayer, ElevationInput, LayerInput, LayerOptions, MapInput, Role};
use mir::{CellLocation, CellRef, FormKey, WorldYaw};
use vault_formats::HexGrid;

const TOWN_DOOR: u32 = 0xe01;

fn elevation(scene: &Scene) -> ElevationInput<'_> {
    ElevationInput {
        sampler: &scene.sampler,
        ground: map_builder::GroundTiles {
            tiles: &scene.tiles,
            squares: &scene.squares,
        },
        art: &scene.art,
        placements: &scene.placements,
        blocked_hexes: &scene.blocked,
        interior: scene.options.interior.clone(),
        level: None,
    }
}

fn map_of(scene: &Scene) -> MapInput<'_> {
    let mut options = scene.options.clone();
    options.interior = None;
    MapInput {
        map_name: &scene.name,
        map_number: scene.number,
        window: scene.window,
        projection: &scene.projection,
        elevations: vec![elevation(scene)],
        entrance_elevation: None,
        cells: &scene.cells,
        critters: &scene.critters,
        options,
    }
}

/// The town with a door into the saloon and the saloon's door back (as
/// `tests/layers.rs` builds them).
fn town_and_saloon() -> (Scene, Scene) {
    let mut town = town();
    let frame = town.frame();
    let door_hex = hex_at(&frame, 48.0, 48.0);
    let [x, y] = hex_point(&frame, door_hex);
    let centre = town
        .cells
        .iter()
        .position(|c| matches!(c.location, CellLocation::Exterior { x: 0, y: 0, .. }))
        .unwrap();
    let mut door = teleport_door(
        TOWN_DOOR,
        [x, y, 100.0],
        DOOR,
        SALOON,
        Some("SynthSaloon"),
        None,
    );
    door.teleport.as_mut().unwrap().position = [0.0, -200.0, 1000.0];
    town.cells[centre].references.push(door);
    town.cells[centre]
        .references
        .sort_by(|a, b| a.form.cmp(&b.form));
    town.placements.push(placement(
        4,
        door_hex,
        Role::Door,
        true,
        &FormKey::new(PLUGIN, TOWN_DOOR).to_string(),
    ));
    town.base = full_base();
    let outside = town.cells[centre].form.clone();
    let back = hex_point(&frame, HexGrid::step(door_hex, 2, 3).unwrap());

    let mut saloon = common::saloon::saloon(common::saloon::interior(false, None));
    let cell = &mut saloon.cells[0];
    let door = cell
        .references
        .iter_mut()
        .find(|r| r.form == FormKey::new(PLUGIN, DOOR))
        .unwrap();
    let teleport = door.teleport.as_mut().unwrap();
    teleport.door = FormKey::new(PLUGIN, TOWN_DOOR);
    teleport.cell = Some(CellRef {
        form: outside,
        editor_id: Some("SynthCell00".into()),
        location: CellLocation::Exterior {
            worldspace: "synthworld".into(),
            worldspace_form: FormKey::new(PLUGIN, 0x800),
            x: 0,
            y: 0,
        },
    });
    teleport.position = [back[0], back[1], 100.0];
    (town, saloon)
}

/// The window centre a scene turns about.
fn centre_of(scene: &Scene) -> [f32; 2] {
    [
        scene.window.x_west + scene.window.size * 0.5,
        scene.window.y_north - scene.window.size * 0.5,
    ]
}

/// A hex as the turned slicer would place the same thing: the hex under
/// the turned source point of its centre.
fn turned_hex(frame: &map_builder::walk::Frame, yaw: &WorldYaw, hex: i32) -> i32 {
    let [x, y] = hex_point(frame, hex);
    let [x, y] = yaw.apply_xy(x, y);
    frame
        .hex_of_source(f64::from(x), f64::from(y))
        .expect("on the map")
}

/// Turns a scene's own inputs by `yaw` (its teleports are the caller's).
fn turn(scene: &mut Scene, yaw: &WorldYaw) {
    let frame = scene.frame();
    for cell in &mut scene.cells {
        yaw.turn_cell(cell);
    }
    for critter in &mut scene.critters {
        critter.position = yaw.apply_xy(critter.position[0], critter.position[1]);
        critter.yaw = yaw.apply_yaw(critter.yaw);
    }
    for placement in &mut scene.placements {
        placement.hex = turned_hex(&frame, yaw, placement.hex);
    }
    for hex in &mut scene.blocked {
        *hex = turned_hex(&frame, yaw, *hex);
    }
    if let Some(interior) = &mut scene.options.interior
        && let Some([x, y]) = interior.entrance_near
    {
        interior.entrance_near = Some(yaw.apply_xy(x, y));
    }
    let sampler = std::mem::take(&mut scene.sampler);
    scene.sampler = sampler.turned(*yaw);
}

/// The town and the saloon turned by `degrees` each about its own centre.
fn turned_pair(degrees: f64) -> (Scene, Scene, WorldYaw, WorldYaw) {
    let (mut town, mut saloon) = town_and_saloon();
    let town_yaw = WorldYaw::new(centre_of(&town), degrees);
    let saloon_yaw = WorldYaw::new(centre_of(&saloon), degrees);
    turn(&mut town, &town_yaw);
    turn(&mut saloon, &saloon_yaw);
    // Each teleport arrives in the other map: its turn.
    for (scene, other) in [(&mut town, &saloon_yaw), (&mut saloon, &town_yaw)] {
        for cell in &mut scene.cells {
            for r in &mut cell.references {
                if let Some(t) = &mut r.teleport {
                    other.turn_teleport(t);
                }
            }
        }
    }
    (town, saloon, town_yaw, saloon_yaw)
}

fn build(town: &Scene, saloon: &Scene, base: &BTreeMap<String, Vec<u8>>) -> BuiltLayer {
    let input = LayerInput {
        layer_id: "synthetic-layer",
        layer_name: "Synthetic layer",
        base_files: base,
        critter_art: &saloon.critter_art,
        maps: vec![map_of(town), map_of(saloon)],
        stairs: Vec::new(),
        options: LayerOptions::default(),
    };
    map_builder::build_layer(&input).expect("the layer builds")
}

#[test]
fn a_turned_town_and_saloon_stay_linked_and_turn_their_people() {
    let base = full_base();
    let (town0, saloon0) = town_and_saloon();
    let plain = build(&town0, &saloon0, &base);
    let frame = saloon0.frame();
    let walkable = |built: &BuiltLayer| -> Vec<usize> {
        built
            .maps
            .iter()
            .map(|m| m.walkable[0].iter().filter(|w| **w).count())
            .collect()
    };
    let plain_walkable = walkable(&plain);
    let plain_people: BTreeMap<String, (i32, u8, i32)> = plain.maps[1]
        .report
        .critters
        .iter()
        .map(|c| (c.reference.clone(), (c.hex, c.orientation, c.steps)))
        .collect();
    assert_eq!(plain_people.len(), 2);

    for degrees in [0.0, 90.0, 180.0, 270.0] {
        let (town, saloon, _, saloon_yaw) = turned_pair(degrees);
        let built = build(&town, &saloon, &base);
        if degrees == 0.0 {
            assert_eq!(built.layer.files(), plain.layer.files(), "0 degrees");
        }
        // Both doors link, both ways; the links check is clean.
        for map in &built.maps {
            assert_eq!(
                map.links.len(),
                1,
                "{degrees}: {:?}",
                map.report.skipped_teleports
            );
            assert!(map.links[0].returns, "{degrees}: {:?}", map.links[0]);
        }
        assert!(
            built.links.one_way.is_empty(),
            "{degrees}: {:?}",
            built.links
        );
        assert_eq!(built.links.links, 2);
        // The people: on the image of their hex, facing the turned way.
        for c in &built.maps[1].report.critters {
            let (hex0, facing0, steps0) = plain_people[&c.reference];
            let image = turned_hex(&frame, &saloon_yaw, hex0);
            let off = HexGrid::lattice_distance(c.hex, image).unwrap();
            assert!(
                off <= 1 + steps0 + c.steps,
                "{degrees}: {} on {} but the image of {hex0} is {image}",
                c.reference,
                c.hex
            );
            assert_eq!(frame.orientation_of_yaw(0.0), Some(facing0));
            assert_eq!(
                Some(c.orientation),
                frame.orientation_of_yaw(saloon_yaw.apply_yaw(0.0)),
                "{degrees}"
            );
        }
        for (now, before) in walkable(&built).iter().zip(&plain_walkable) {
            let tolerance = (before / 5).max(12);
            assert!(
                now.abs_diff(*before) <= tolerance,
                "{degrees}: walkable {now}, unturned {before}"
            );
        }
        // Determinism.
        let again = build(&town, &saloon, &base);
        assert_eq!(again.layer.files(), built.layer.files(), "{degrees}");
    }

    // Any angle keeps the links.
    let (town, saloon, _, _) = turned_pair(45.0);
    let built = build(&town, &saloon, &base);
    assert!(built.links.one_way.is_empty(), "{:?}", built.links);
    assert_eq!(built.links.links, 2);
}
