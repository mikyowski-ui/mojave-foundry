//! Layers of several maps and elevations (`DESIGN-levels.md`): an exterior
//! and an interior with a door pair linked both ways, a door leading
//! outside the build, three storeys on one map, five storeys on two maps,
//! the shared art budget, isolated layers and determinism. Every exit grid
//! is followed, on the maps parsed back through `vault-formats`, to a
//! walkable arrival hex that holds no grid.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use common::saloon::{DOOR, SALOON};
use common::scenes::full_base;
use common::*;
use map_builder::exits::RETURN_RADIUS;
use map_builder::storeys::{self, SplitOptions};
use map_builder::{
    ArtCategory, BuildError, BuiltLayer, ElevationInput, LayerInput, LayerOptions, LinkKind,
    MapInput, Placement, Role, build_layer,
};
use mir::{CellDoc, CellLocation, CellRef, FormKey, GroundSampler, NavTriangleDoc, NavmeshDoc};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::{ByteSource, GameProfile, HexGrid, LayeredVfs, LocalPrototypeCatalog};
use vault_formats::{MemoryDir, Vfs};

fn mounted(layer: &emit::ModLayer, base: &BTreeMap<String, Vec<u8>>) -> LayeredVfs {
    let tree = |files: Vec<(String, Vec<u8>)>| {
        MemoryDir::new(
            files
                .into_iter()
                .map(|(path, bytes)| (path, ByteSource::new(bytes).unwrap())),
        )
        .unwrap()
    };
    let mut files: Vec<(String, Vec<u8>)> = layer
        .files()
        .iter()
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    files.push((
        "vault-mod.toml".into(),
        layer.manifest_text().unwrap().into_bytes(),
    ));
    LayeredVfs::new(
        Arc::new(tree(base.clone().into_iter().collect())),
        vec![vault_formats::ModLayer::from_tree("test", tree(files))],
    )
}

/// Every map of the layer parsed back, by number.
fn parsed(built: &BuiltLayer, base: &BTreeMap<String, Vec<u8>>) -> BTreeMap<u32, MapFile> {
    let vfs = mounted(&built.layer, base);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    built
        .layer
        .manifest()
        .maps
        .iter()
        .map(|m| {
            let bytes = vfs.read(&format!("maps/{}", m.file)).unwrap();
            let map = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).unwrap();
            assert!(map.diagnostics.is_empty(), "{:?}", map.diagnostics);
            (m.number, map)
        })
        .collect()
}

/// Exit grids of one elevation: hex -> (map, hex, elevation, orientation).
fn grids(map: &MapFile, elevation: usize) -> BTreeMap<i32, (i32, i32, i32, i32)> {
    map.objects.elevations[elevation]
        .iter()
        .filter_map(|object| match object.state {
            ObjectState::Other {
                detail:
                    ObjectDetail::ExitGrid {
                        map,
                        hex,
                        elevation,
                        orientation,
                    },
                ..
            } => Some((object.hex.unwrap(), (map, hex, elevation, orientation))),
            _ => None,
        })
        .collect()
}

/// Follow every exit grid of every real map to its destination: a map of
/// the layer, onto a hex walkable there (for real maps, by the build's own
/// grid) that holds no exit grid, and never hex 0. Returns how many grids
/// were followed.
fn follow_every_grid(built: &BuiltLayer, base: &BTreeMap<String, Vec<u8>>) -> usize {
    let maps = parsed(built, base);
    let mut followed = 0;
    for (number, map) in &maps {
        for e in 0..3 {
            for (hex, (to, to_hex, to_e, orientation)) in grids(map, e) {
                let target = &maps[&(to as u32)];
                assert!(to_hex > 0, "map {number} grid {hex} keeps the entrance");
                assert!((0..=5).contains(&orientation));
                assert!(
                    !grids(target, to_e as usize).contains_key(&to_hex),
                    "map {number} elevation {e} grid {hex} arrives on a grid of map {to}"
                );
                if let Some(real) = built.maps.iter().find(|m| m.number == to as u32) {
                    assert!(
                        real.walkable[to_e as usize][to_hex as usize],
                        "map {number} elevation {e} grid {hex} arrives on hex {to_hex} of map \
                         {to} elevation {to_e}, not walkable"
                    );
                }
                followed += 1;
            }
        }
    }
    followed
}

/// One elevation of a scene.
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

/// A scene as one map of a layer.
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

const TOWN_DOOR: u32 = 0xe01;

/// The town with a door into the saloon, and the saloon whose door leads
/// back to it; the town door's grid arrival point is inside the saloon by
/// its door, the saloon door's is three hexes south of the town door.
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

fn layer<'a>(
    maps: Vec<MapInput<'a>>,
    base: &'a BTreeMap<String, Vec<u8>>,
    art: &'a [map_builder::CritterArt],
) -> LayerInput<'a> {
    LayerInput {
        layer_id: "synthetic-layer",
        layer_name: "Synthetic layer",
        base_files: base,
        critter_art: art,
        maps,
        stairs: Vec::new(),
        options: LayerOptions::default(),
    }
}

#[test]
fn an_exterior_and_an_interior_are_linked_both_ways() {
    let (town, saloon) = town_and_saloon();
    let base = full_base();
    let input = layer(
        vec![map_of(&town), map_of(&saloon)],
        &base,
        &saloon.critter_art,
    );
    let built = build_layer(&input).unwrap();
    assert!(built.stubs.is_empty(), "{:?}", built.stubs);
    assert_eq!(built.maps.len(), 2);
    let (outside, inside) = (&built.maps[0], &built.maps[1]);
    let first = outside.report.first_map_number;
    assert_eq!((outside.number, inside.number), (first, first + 1));
    assert_eq!(
        outside.links.len(),
        1,
        "{:?}",
        outside.report.skipped_teleports
    );
    assert_eq!(
        inside.links.len(),
        1,
        "{:?}",
        inside.report.skipped_teleports
    );
    let out = &outside.links[0];
    let back = &inside.links[0];
    assert_eq!(out.kind, LinkKind::Door);
    assert_eq!((out.to_number, out.to_elevation), (first + 1, 0));
    assert_eq!((back.to_number, back.to_elevation), (first, 0));
    assert!(out.returns && back.returns);
    // Each arrival is near the partner door, 2 or more steps from it, off
    // the partner's grids.
    for (link, partner) in [(out, back), (back, out)] {
        let partner_door = partner.door_hex.unwrap();
        let steps = HexGrid::lattice_distance(link.to_hex, partner_door).unwrap();
        assert!((2..=RETURN_RADIUS).contains(&steps), "{link:?}");
        assert!(!partner.grid_hexes.contains(&link.to_hex));
    }
    // The saloon's arrival is the walkable hex nearest the teleport point.
    let saloon_frame = saloon.frame();
    let nearest = saloon_frame.hex_of_source(0.0, -200.0).unwrap();
    assert!(HexGrid::lattice_distance(out.to_hex, nearest).unwrap() <= 1);
    assert_eq!(
        follow_every_grid(&built, &base),
        out.grid_hexes.len() + back.grid_hexes.len()
    );
    assert_eq!(built.links.links, 2);
    assert!(built.links.one_way.is_empty());
    // Arrivals are in the main component of the written maps.
    for map in &built.maps {
        let e = &map.elevations[0];
        assert_eq!(e.arrivals.len(), 1);
        assert_eq!(e.arrivals[0].2, e.components.main, "{:?}", map.file);
    }
    // No door is reported as not converted; the saloon's residents stand.
    assert!(inside.report.skipped_teleports.is_empty());
    assert_eq!(inside.report.critters.len(), 2);
    assert!(inside.report.interior.is_some());
    // maps.txt holds both, on elevation 0.
    let text = String::from_utf8(built.layer.files()["data/maps.txt"].clone()).unwrap();
    assert!(text.contains(&format!("[Map {first:03}]\nmap_name=synth\n")));
    assert!(text.contains(&format!(
        "[Map {:03}]\nmap_name=saloon\nrandom_start_point_0=elev:0",
        first + 1
    )));
    // One blocker, one exit marker: a scenery art entry each map does not
    // multiply.
    let scenery = &built.maps[0].report.art_lists["scenery"];
    assert_eq!(built.maps[1].report.art_lists["scenery"], *scenery);
    // Determinism.
    let again = build_layer(&input).unwrap();
    assert_eq!(again.layer.files(), built.layer.files());
    assert_eq!(
        again.layer.manifest_text().unwrap(),
        built.layer.manifest_text().unwrap()
    );
    assert_eq!(again.maps[1].report, built.maps[1].report);
}

#[test]
fn a_door_outside_the_build_keeps_its_stub_or_is_reported() {
    let (mut town, mut saloon) = town_and_saloon();
    // A town door into an interior that is not built.
    let frame = town.frame();
    let hex = hex_at(&frame, 56.0, 48.0);
    let [x, y] = hex_point(&frame, hex);
    let centre = town
        .cells
        .iter()
        .position(|c| matches!(c.location, CellLocation::Exterior { x: 0, y: 0, .. }))
        .unwrap();
    town.cells[centre].references.push(teleport_door(
        0xe02,
        [x, y, 100.0],
        0xe03,
        0xd09,
        Some("Elsewhere"),
        None,
    ));
    town.cells[centre]
        .references
        .sort_by(|a, b| a.form.cmp(&b.form));
    town.placements.push(placement(
        4,
        hex,
        Role::Door,
        true,
        &FormKey::new(PLUGIN, 0xe02).to_string(),
    ));
    // A saloon door into another interior that is not built.
    let sframe = saloon.frame();
    let shex = sframe.hex_of_source(300.0, -290.0).unwrap();
    let mut other = teleport_door(0x7102, [300.0, -290.0, 1000.0], 0x7a01, 0x7a00, None, None);
    other.teleport.as_mut().unwrap().position = [0.0, 0.0, 0.0];
    saloon.cells[0].references.push(other);
    saloon.cells[0]
        .references
        .sort_by(|a, b| a.form.cmp(&b.form));
    saloon.placements.push(placement(
        0,
        shex,
        Role::Door,
        true,
        &FormKey::new(PLUGIN, 0x7102).to_string(),
    ));
    let base = full_base();
    let built = build_layer(&layer(
        vec![map_of(&town), map_of(&saloon)],
        &base,
        &saloon.critter_art,
    ))
    .unwrap();
    // The stub is numbered after both real maps.
    let report = &built.maps[0].report;
    let first = report.first_map_number;
    assert_eq!(built.stubs, vec![("synt_000.map".to_owned(), first + 2)]);
    assert_eq!(report.exits.len(), 1);
    assert_eq!(report.exits[0].stub_number, first + 2);
    assert_eq!(built.maps[0].links.len(), 1);
    let inside = &built.maps[1].report;
    let skipped: Vec<&str> = inside
        .skipped_teleports
        .iter()
        .map(|s| s.door.as_str())
        .collect();
    assert_eq!(skipped, [FormKey::new(PLUGIN, 0x7102).to_string()]);
    assert!(
        inside.skipped_teleports[0]
            .reason
            .contains("outside the build")
    );
    assert_eq!(built.maps[1].links.len(), 1);
    follow_every_grid(&built, &base);
    assert!(built.links.one_way.is_empty());
}

#[test]
fn a_partner_without_an_arrival_is_skipped_and_the_other_way_still_links() {
    let (mut town, saloon) = town_and_saloon();
    // Wall the town door in two steps out: its doorstep stays, no hex 2 to
    // 10 steps from it is walkable.
    let frame = town.frame();
    let door = hex_at(&frame, 48.0, 48.0);
    town.blocked.extend(
        map_builder::exits::hexes_around(door, RETURN_RADIUS, false)
            .into_iter()
            .filter(|h| HexGrid::lattice_distance(*h, door).unwrap() >= 2),
    );
    town.options.reconnect = false;
    let base = full_base();
    let built = build_layer(&layer(
        vec![map_of(&town), map_of(&saloon)],
        &base,
        &saloon.critter_art,
    ))
    .unwrap();
    assert_eq!(built.maps[0].links.len(), 1);
    assert!(!built.maps[0].links[0].returns);
    assert!(built.maps[1].links.is_empty());
    let skipped = &built.maps[1].report.skipped_teleports;
    assert_eq!(skipped.len(), 1);
    assert!(
        skipped[0]
            .reason
            .contains("no walkable hex for the arrival"),
        "{skipped:?}"
    );
    assert_eq!(built.links.one_way.len(), 1);
    assert!(
        built.maps[0]
            .report
            .notes
            .iter()
            .any(|n| n.contains("nothing there leads back"))
    );
}

#[test]
fn isolated_layers_convert_no_door_and_refuse_stairs() {
    let (town, saloon) = town_and_saloon();
    let base = full_base();
    let mut input = layer(
        vec![map_of(&town), map_of(&saloon)],
        &base,
        &saloon.critter_art,
    );
    input.options.isolated = true;
    let built = build_layer(&input).unwrap();
    assert!(built.stubs.is_empty());
    assert!(built.verify.maps.iter().all(|m| m.exit_grids == 0));
    assert!(built.maps.iter().all(|m| m.links.is_empty()));
    assert_eq!(
        built.maps[1].report.interior.as_ref().unwrap().doors.len(),
        1
    );
    input.stairs = vec![map_builder::StairLink {
        label: "s".into(),
        ends: [
            map_builder::Landing {
                map: 0,
                elevation: 0,
                point: [0.0; 3],
            },
            map_builder::Landing {
                map: 1,
                elevation: 0,
                point: [0.0; 3],
            },
        ],
    }];
    assert!(matches!(build_layer(&input), Err(BuildError::Input(m)) if m.contains("isolated")));
}

#[test]
fn the_shared_art_budget_overflows_naming_the_maps() {
    let mut a = town();
    a.name = "towna".into();
    a.base = base_files(4000);
    for i in 0..60u32 {
        a.art
            .push(image(ArtCategory::Scenery, 10 + i, 10, 90, "pile"));
    }
    let mut b = town();
    b.name = "townb".into();
    for i in 0..60u32 {
        b.art
            .push(image(ArtCategory::Scenery, 10 + i, 11, 91, "heap"));
    }
    let base = base_files(4000);
    // Each map alone fits.
    for scene in [&a, &b] {
        build_layer(&layer(vec![map_of(scene)], &base, &[])).unwrap();
    }
    let error = build_layer(&layer(vec![map_of(&a), map_of(&b)], &base, &[])).unwrap_err();
    let BuildError::FidBudget {
        category, largest, ..
    } = &error
    else {
        panic!("{error}");
    };
    assert_eq!(*category, "scenery");
    let names: BTreeSet<&str> = largest.iter().map(|(l, _)| l.as_str()).collect();
    assert!(
        names.contains("towna") && names.contains("townb"),
        "{error}"
    );
    assert!(error.to_string().contains("towna"));
}

#[test]
fn stems_and_numbers_are_checked_across_maps() {
    let a = town();
    let mut b = town();
    let base = BTreeMap::new();
    assert!(matches!(
        build_layer(&layer(vec![map_of(&a), map_of(&b)], &base, &[])),
        Err(BuildError::Input(m)) if m.contains("share the file stem")
    ));
    b.name = "other".into();
    b.number = Some(66);
    let built = build_layer(&layer(vec![map_of(&a), map_of(&b)], &base, &[])).unwrap();
    // The given number is honoured, the other takes the next free one.
    assert_eq!((built.maps[0].number, built.maps[1].number), (67, 66));
}

// --- Storeys ---------------------------------------------------------------

/// A quad of two triangles, its height linear in x from `z0` to `z1`.
fn quad(x0: f32, x1: f32, z0: f32, z1: f32) -> Vec<[[f32; 3]; 3]> {
    let (y0, y1) = (-300.0, 300.0);
    vec![
        [[x0, y0, z0], [x1, y0, z1], [x1, y1, z1]],
        [[x0, y0, z0], [x1, y1, z1], [x0, y1, z0]],
    ]
}

/// Floor `f` at z `300 f`, x `[1100 f - 400, 1100 f + 400]`, a ramp of
/// slope 1 up to the next.
fn tower_cell(floors: usize) -> CellDoc {
    let mut t = Vec::new();
    for f in 0..floors {
        let z = 300.0 * f as f32;
        let x = 1100.0 * f as f32;
        t.extend(quad(x - 400.0, x + 400.0, z, z));
        if f + 1 < floors {
            t.extend(quad(x + 400.0, x + 700.0, z, z + 300.0));
        }
    }
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for tri in &t {
        let base = vertices.len() as u32;
        vertices.extend_from_slice(tri);
        triangles.push(NavTriangleDoc {
            vertices: [base, base + 1, base + 2],
            links: [-1; 3],
            flags: 0,
        });
    }
    let mut references = Vec::new();
    for f in 0..floors {
        let mut r = teleport_door(
            0x8200 + f as u32,
            [1100.0 * f as f32, 100.0, 300.0 * f as f32],
            0,
            0,
            None,
            None,
        );
        r.kind = mir::RefKind::Npc;
        r.teleport = None;
        references.push(r);
    }
    // Between floors 0 and 1, on the ramp.
    let mut r = teleport_door(0x8280, [550.0, 0.0, 150.0], 0, 0, None, None);
    r.kind = mir::RefKind::Npc;
    r.teleport = None;
    references.push(r);
    CellDoc {
        schema_version: mir::SCHEMA_VERSION,
        form: FormKey::new(PLUGIN, 0x8000),
        editor_id: Some("SynthTower".into()),
        name: None,
        location: CellLocation::Interior,
        flags: 1,
        water_height: None,
        land: None,
        references,
        navmeshes: vec![NavmeshDoc {
            form: FormKey::new(PLUGIN, 0x8001),
            vertices,
            triangles,
            cover_triangles: Vec::new(),
            door_links: Vec::new(),
            external_links: Vec::new(),
        }],
    }
}

/// Everything a tower layer borrows.
struct Tower {
    projection: foundry_render::Projection,
    window: mir::MapWindow,
    sampler: GroundSampler,
    tiles: Vec<Vec<u8>>,
    squares: Vec<u16>,
    art: Vec<map_builder::ArtImage>,
    /// Per floor, one stool.
    placements: Vec<Vec<Placement>>,
    cells: Vec<CellDoc>,
    critters: Vec<map_builder::CritterPlacement>,
    critter_art: Vec<map_builder::CritterArt>,
    split: storeys::StoreySplit,
    layout: storeys::StoreyLayout,
    base: BTreeMap<String, Vec<u8>>,
}

fn tower(floors: usize, stem: &str) -> Tower {
    let projection = projection();
    let window = mir::MapWindow::from_center([1100.0, 0.0], projection.tile_units());
    let frame = frame(&window, &projection);
    let cells = vec![tower_cell(floors)];
    let split = storeys::split_storeys(&cells, &SplitOptions::default());
    assert_eq!(split.floors.len(), floors, "{:?}", split.floors);
    let layout = storeys::layout(&split, stem).unwrap();
    let placements = (0..floors)
        .map(|f| {
            vec![placement(
                0,
                frame
                    .hex_of_source(1100.0 * f as f64 - 200.0, -150.0)
                    .unwrap(),
                Role::Scenery,
                true,
                &format!("synthetic.esm:0083{f:02x}"),
            )]
        })
        .collect();
    let mut critters: Vec<_> = (0..floors)
        .map(|f| {
            critter(
                0,
                &FormKey::new(PLUGIN, 0x8200 + f as u32).to_string(),
                &format!("Floor {f}"),
                [1100.0 * f as f32, 100.0],
                0.0,
            )
        })
        .collect();
    critters.push(critter(
        0,
        &FormKey::new(PLUGIN, 0x8280).to_string(),
        "On the ramp",
        [550.0, 0.0],
        0.0,
    ));
    Tower {
        projection,
        window,
        sampler: GroundSampler::default(),
        tiles: library(),
        squares: squares(),
        art: vec![image(ArtCategory::Scenery, 20, 30, 40, "stool")],
        placements,
        cells,
        critters,
        critter_art: vec![sprite_set("resident", 5)],
        split,
        layout,
        base: full_base(),
    }
}

/// The tower's maps: one per layout map, its floors as elevations.
fn tower_maps(t: &Tower) -> Vec<MapInput<'_>> {
    t.layout
        .maps
        .iter()
        .enumerate()
        .map(|(m, map)| MapInput {
            map_name: &map.name,
            map_number: None,
            window: t.window,
            projection: &t.projection,
            elevations: map
                .floors
                .iter()
                .map(|f| ElevationInput {
                    sampler: &t.sampler,
                    ground: map_builder::GroundTiles {
                        tiles: &t.tiles,
                        squares: &t.squares,
                    },
                    art: &t.art,
                    placements: &t.placements[*f],
                    blocked_hexes: &[],
                    interior: Some(t.split.floors[*f].interior_build(&SplitOptions::default())),
                    level: None,
                })
                .collect(),
            entrance_elevation: None,
            cells: &t.cells,
            // Every critter is offered to the first map; the others only
            // see their own.
            critters: if m == 0 { &t.critters } else { &[] },
            options: map_builder::BuildOptions::default(),
        })
        .collect()
}

#[test]
fn three_storeys_are_three_elevations_linked_by_their_stairs() {
    let t = tower(3, "tower");
    let maps = tower_maps(&t);
    assert_eq!(maps.len(), 1);
    let mut input = layer(maps, &t.base, &t.critter_art);
    input.stairs = t.layout.stair_links(0);
    assert_eq!(input.stairs.len(), 2);
    let built = build_layer(&input).unwrap();
    let map = &built.maps[0];
    assert_eq!(map.elevations.len(), 3);
    // Two stairs, both ways, each to the map's own number.
    assert_eq!(map.links.len(), 4, "{:?}", map.report.skipped_teleports);
    for link in &map.links {
        assert_eq!(link.kind, LinkKind::Stairs);
        assert_eq!(link.to_number, map.number);
        assert_ne!(link.to_elevation, link.elevation);
        assert!(link.to_hex > 0 && link.returns);
        // Never within one step of the destination's own grids for it.
        let partner = map
            .links
            .iter()
            .find(|l| l.from == link.from && l.elevation == link.to_elevation)
            .unwrap();
        for hex in &partner.grid_hexes {
            assert!(HexGrid::lattice_distance(*hex, link.to_hex).unwrap() >= 2);
        }
    }
    assert!(built.links.one_way.is_empty());
    assert_eq!(
        follow_every_grid(&built, &t.base),
        map.links.iter().map(|l| l.grid_hexes.len()).sum::<usize>()
    );
    // Every elevation written: tiles and objects on each, none omitted, ids
    // unique across them.
    let maps = parsed(&built, &t.base);
    let file = &maps[&map.number];
    assert_eq!(file.header.flags.0 & 0xe, 0);
    assert!(file.tiles.iter().all(Option::is_some));
    let ids: Vec<u32> = file
        .objects
        .elevations
        .iter()
        .flatten()
        .map(|o| o.id)
        .collect();
    let unique: BTreeSet<u32> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len());
    assert_eq!(file.header.entry_elevation, 0);
    // Critters on the elevation of their height; the one on the ramp is on
    // another level.
    let on = |e: usize| {
        file.objects.elevations[e]
            .iter()
            .filter(|o| matches!(o.state, ObjectState::Critter(_)))
            .count()
    };
    assert_eq!((on(0), on(1), on(2)), (1, 1, 1));
    assert_eq!(map.critter_elevations, [0, 1, 2]);
    assert_eq!(map.report.skipped_critters.len(), 1);
    assert!(
        map.report.skipped_critters[0]
            .reason
            .starts_with("on another level")
    );
    // Readability and walkability per elevation, each its own floor.
    for (e, report) in map.elevations.iter().enumerate() {
        assert!(report.readability.walkable_hexes > 0, "elevation {e}");
        assert_eq!(report.critters.len(), 1);
        assert_eq!(
            report.band,
            Some(
                t.split.floors[e]
                    .interior_build(&SplitOptions::default())
                    .band()
            )
        );
    }
    let frame = frame(&t.window, &t.projection);
    let floor1 = frame.hex_of_source(1100.0, -150.0).unwrap();
    let floor1 = floor1 as usize;
    assert!(!map.walkable[0][floor1] && map.walkable[1][floor1] && !map.walkable[2][floor1]);
    let text = String::from_utf8(built.layer.files()["data/maps.txt"].clone()).unwrap();
    assert!(text.contains("map_name=tower\nrandom_start_point_0=elev:0, tile_num:"));
    // Determinism.
    let again = build_layer(&input).unwrap();
    assert_eq!(again.layer.files(), built.layer.files());
}

#[test]
fn five_storeys_spill_into_a_second_map() {
    let t = tower(5, "tower");
    let maps = tower_maps(&t);
    assert_eq!(maps.len(), 2);
    let mut input = layer(maps, &t.base, &t.critter_art);
    input.stairs = t.layout.stair_links(0);
    assert_eq!(input.stairs.len(), 4);
    let built = build_layer(&input).unwrap();
    assert_eq!(built.maps[0].file, "maps/tower.map");
    assert_eq!(built.maps[1].file, "maps/towerb.map");
    assert_eq!(built.maps[0].elevations.len(), 3);
    assert_eq!(built.maps[1].elevations.len(), 2);
    let across: Vec<_> = built.maps[0]
        .links
        .iter()
        .filter(|l| l.to_number == built.maps[1].number)
        .collect();
    assert_eq!(across.len(), 1);
    assert_eq!((across[0].elevation, across[0].to_elevation), (2, 0));
    let down: Vec<_> = built.maps[1]
        .links
        .iter()
        .filter(|l| l.to_number == built.maps[0].number)
        .collect();
    assert_eq!(down.len(), 1);
    assert_eq!((down[0].elevation, down[0].to_elevation), (0, 2));
    assert!(built.links.one_way.is_empty());
    assert_eq!(built.links.links, 8);
    follow_every_grid(&built, &t.base);
    // The critters of floors 3 and 4 are offered to map 0 only: they are
    // on another level there, so only floors 0 to 2 are populated.
    assert_eq!(built.maps[0].report.critters.len(), 3);
}

#[test]
fn the_link_check_refuses_grids_onto_blocked_hexes_grids_and_hex_zero() {
    let (town, saloon) = town_and_saloon();
    let base = full_base();
    let built = build_layer(&layer(
        vec![map_of(&town), map_of(&saloon)],
        &base,
        &saloon.critter_art,
    ))
    .unwrap();
    let inside = &built.maps[1];
    let outside = &built.maps[0];
    let maps = parsed(&built, &base);
    // A blocked town hex (the rock), a town grid hex, and hex 0.
    let rock = town.placements[2].hex;
    let town_grid = outside.links[0].grid_hexes[0];
    for (to, expect) in [
        (rock, "blocks"),
        (town_grid, "arrives on an exit grid"),
        (0, "keep the entrance"),
    ] {
        let mut map = maps[&inside.number].clone();
        for object in &mut map.objects.elevations[0] {
            if let ObjectState::Other {
                detail: ObjectDetail::ExitGrid { hex, .. },
                ..
            } = &mut object.state
            {
                *hex = to;
            }
        }
        let mut layer = built.layer.clone();
        layer.insert(&inside.file, map.write().unwrap()).unwrap();
        let error = map_builder::verify_links(&layer, &base).unwrap_err();
        assert!(error.to_string().contains(expect), "{error}");
    }
    // A grid removed: the other way becomes one-way.
    let mut map = maps[&inside.number].clone();
    map.objects.elevations[0].retain(|o| {
        !matches!(
            o.state,
            ObjectState::Other {
                detail: ObjectDetail::ExitGrid { .. },
                ..
            }
        )
    });
    let mut layer = built.layer.clone();
    layer.insert(&inside.file, map.write().unwrap()).unwrap();
    let check = map_builder::verify_links(&layer, &base).unwrap();
    assert_eq!(check.one_way.len(), 1);
    assert!(check.one_way[0].starts_with("synth.map elevation 0 -> saloon.map"));
}

#[test]
fn reach_is_reported_and_what_the_start_cannot_reach_is_closed() {
    let t = tower(3, "tower");
    let maps = tower_maps(&t);
    let mut input = layer(maps, &t.base, &t.critter_art);
    // Only the stairs between floors 0 and 1: floor 2 has no way in.
    input.stairs = t.layout.stair_links(0);
    input
        .stairs
        .retain(|s| s.ends.iter().all(|e| e.elevation < 2));
    assert_eq!(input.stairs.len(), 1);
    let open = build_layer(&input).unwrap();
    let e = &open.maps[0].elevations;
    let (reached0, planned0) = e[0].reached.expect("reach");
    assert_eq!(reached0, planned0);
    assert_eq!(e[1].reached.map(|r| r.0 == r.1), Some(true));
    assert_eq!(e[2].reached.map(|r| r.0), Some(0));
    assert!(e[2].closed.is_empty(), "off by default");
    // Closed: elevation 2 holds nothing walkable, and says what it closed.
    input.options.close_unreached = true;
    let closed = build_layer(&input).unwrap();
    let e = &closed.maps[0].elevations;
    assert!(!e[2].closed.is_empty());
    assert_eq!(e[2].reached, Some((0, 0)));
    assert!(closed.maps[0].walkable[2].iter().all(|w| !w));
    assert_eq!(e[0].reached, open.maps[0].elevations[0].reached);
    // Its critter now has nowhere to stand.
    assert!(closed.maps[0].critter_elevations.iter().all(|e| *e < 2));
    // Deterministic.
    assert_eq!(
        build_layer(&input).unwrap().layer.files(),
        closed.layer.files()
    );
}

#[test]
fn a_storey_can_walk_its_own_floor_triangles() {
    let t = tower(3, "tower");
    let mut maps = tower_maps(&t);
    // The floors' own triangles instead of the band: the same walkable
    // ground on this tower, whose floors are flat.
    let band: Vec<Vec<bool>> = {
        let mut input = layer(tower_maps(&t), &t.base, &t.critter_art);
        input.stairs = t.layout.stair_links(0);
        build_layer(&input).unwrap().maps[0].walkable.clone()
    };
    for (e, elevation) in maps[0].elevations.iter_mut().enumerate() {
        let floor = &t.split.floors[t.layout.maps[0].floors[e]];
        assert!(!floor.members.is_empty());
        let mut interior = elevation.interior.clone().unwrap();
        interior.triangles = Some(std::sync::Arc::new(floor.members.iter().cloned().collect()));
        elevation.interior = Some(interior);
    }
    let mut input = layer(maps, &t.base, &t.critter_art);
    input.stairs = t.layout.stair_links(0);
    let built = build_layer(&input).unwrap();
    assert_eq!(built.maps[0].walkable, band);
}

// --- Link clearance (crate notes, "Link clearance") ---------------------------

/// The hexes of the ground items and containers (art category 0) on one
/// elevation of a written map.
fn item_hexes(map: &MapFile, elevation: usize) -> Vec<i32> {
    let mut hexes: Vec<i32> = map.objects.elevations[elevation]
        .iter()
        .filter(|o| vault_formats::Fid(o.art_id).category() == 0)
        .filter_map(|o| o.hex)
        .collect();
    hexes.sort_unstable();
    hexes
}

/// The keep-clear set of one elevation of a built layer, read from the
/// links the report lists.
fn keep_clear_of(built: &BuiltLayer, map: usize, elevation: u8) -> BTreeSet<i32> {
    let file = &built.maps[map].file;
    let mut clear = BTreeSet::new();
    for m in &built.maps {
        for link in &m.links {
            if m.file == *file && link.elevation == elevation {
                clear.extend(map_builder::clearance::keep_clear(
                    [(link.kind, link.grid_hexes.as_slice())],
                    [],
                ));
            }
            if link.to_file == *file && link.to_elevation == elevation {
                clear.insert(link.to_hex);
            }
        }
    }
    clear
}

fn distance_to(set: &[i32], hex: i32) -> i32 {
    set.iter()
        .map(|g| HexGrid::lattice_distance(*g, hex).unwrap())
        .min()
        .unwrap()
}

#[test]
fn loot_on_or_beside_a_stair_is_moved_clear_and_the_rest_stays() {
    let mut t = tower(3, "tower");
    let item_art = t.art.len();
    t.art
        .push(image(ArtCategory::Items, 16, 16, 50, "crate on the floor"));
    // Without items: where the stair's grids and arrivals are.
    let (grid, arrivals, walkable, taken) = {
        let mut input = layer(tower_maps(&t), &t.base, &t.critter_art);
        input.stairs = t.layout.stair_links(0);
        let plain = build_layer(&input).unwrap();
        let map = &plain.maps[0];
        let grid: Vec<i32> = map
            .links
            .iter()
            .filter(|l| l.elevation == 0 && l.kind == LinkKind::Stairs)
            .flat_map(|l| l.grid_hexes.iter().copied())
            .collect();
        let arrivals: Vec<i32> = map
            .links
            .iter()
            .filter(|l| l.to_elevation == 0)
            .map(|l| l.to_hex)
            .collect();
        let taken: Vec<i32> = t.placements[0].iter().map(|p| p.hex).collect();
        (grid, arrivals, map.walkable[0].clone(), taken)
    };
    assert!(!grid.is_empty() && !arrivals.is_empty());
    let free =
        |hex: i32| walkable[hex as usize] && !arrivals.contains(&hex) && !taken.contains(&hex);
    let on_grid = grid[0];
    let beside = (0..40_000)
        .find(|h| free(*h) && !grid.contains(h) && distance_to(&grid, *h) == 1)
        .expect("a walkable hex beside the grid");
    let two_away = (0..40_000)
        .find(|h| free(*h) && distance_to(&grid, *h) == 2)
        .expect("a walkable hex 2 steps from the grid");
    for (n, hex) in [on_grid, beside, two_away].into_iter().enumerate() {
        t.placements[0].push(placement(
            item_art,
            hex,
            Role::Item,
            false,
            &format!("synthetic.esm:0090{n:02x}"),
        ));
    }
    let mut input = layer(tower_maps(&t), &t.base, &t.critter_art);
    input.stairs = t.layout.stair_links(0);
    let built = build_layer(&input).unwrap();
    let map = &built.maps[0];
    let written = parsed(&built, &t.base);
    let hexes = item_hexes(&written[&map.number], 0);
    assert_eq!(hexes.len(), 3);
    // The item two steps out stays; the other two moved off the set.
    assert!(hexes.contains(&two_away));
    assert!(!hexes.contains(&on_grid) && !hexes.contains(&beside));
    assert_eq!(map.elevations[0].moved_off_links, 2);
    assert_eq!(map.elevations[1].moved_off_links, 0);
    assert!(
        map.report
            .notes
            .iter()
            .any(|n| n == "moved off link hexes: 2"),
        "{:?}",
        map.report.notes
    );
    let clear = keep_clear_of(&built, 0, 0);
    let comps = map_builder::islands::components(|h| map.walkable[0][h as usize]);
    for hex in &hexes {
        assert!(!clear.contains(hex), "item on keep-clear hex {hex}");
        assert!(map.walkable[0][*hex as usize]);
        assert_eq!(comps.of(*hex), comps.of(on_grid), "same component");
    }
    // The check on the written maps agrees, and counts what it looked at.
    let check = map_builder::verify_clearance(&built.layer, &t.base, &built.maps).unwrap();
    assert!(check.hexes > 0 && check.objects >= 3, "{check:?}");
    // Deterministic: two builds are identical.
    let again = build_layer(&input).unwrap();
    assert_eq!(again.layer.files(), built.layer.files());
    assert_eq!(again.maps[0].report, built.maps[0].report);
    // Without the stairs nothing is moved and every item stays where it
    // was placed.
    let mut open = layer(tower_maps(&t), &t.base, &t.critter_art);
    open.stairs = Vec::new();
    let unlinked = build_layer(&open).unwrap();
    assert_eq!(unlinked.maps[0].elevations[0].moved_off_links, 0);
    let mut placed = vec![on_grid, beside, two_away];
    placed.sort_unstable();
    assert_eq!(
        item_hexes(&parsed(&unlinked, &t.base)[&unlinked.maps[0].number], 0),
        placed
    );
}

#[test]
fn the_check_refuses_an_item_left_on_a_grid_hex() {
    let t = tower(2, "tower");
    let mut input = layer(tower_maps(&t), &t.base, &t.critter_art);
    input.stairs = t.layout.stair_links(0);
    let plain = build_layer(&input).unwrap();
    // The untouched layer passes.
    map_builder::verify_clearance(&plain.layer, &t.base, &plain.maps).unwrap();
    let grid = plain.maps[0]
        .links
        .iter()
        .find(|l| l.elevation == 0)
        .unwrap()
        .grid_hexes[0];
    // Put an item object on the grid by hand, on the written map: a copy
    // of a scenery object given an item's art, which is all the check
    // reads.
    let mut tampered = plain.layer.clone();
    let path = plain.maps[0].file.clone();
    let vfs = mounted(&plain.layer, &t.base);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let bytes = plain.layer.files()[&path].clone();
    let mut file = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).unwrap();
    let mut object = file.objects.elevations[0]
        .iter()
        .find(|o| vault_formats::Fid(o.art_id).category() == 2)
        .unwrap()
        .clone();
    object.art_id = vault_formats::Fid::new(0, 1, 0, 0, 0).0;
    object.hex = Some(grid);
    object.id = 99_999;
    file.objects.elevations[0].push(object);
    file.objects.total += 1;
    tampered.insert(&path, file.write().unwrap()).unwrap();
    let error = map_builder::verify_clearance(&tampered, &t.base, &plain.maps).unwrap_err();
    assert!(
        error.to_string().contains("on or beside an exit grid"),
        "{error}"
    );
}

#[test]
fn loot_on_a_door_arrival_is_moved_and_loot_beside_the_door_grid_stays() {
    let (mut town, saloon) = town_and_saloon();
    let base = full_base();
    let plain = build_layer(&layer(
        vec![map_of(&town), map_of(&saloon)],
        &base,
        &saloon.critter_art,
    ))
    .unwrap();
    // The saloon door's link arrives on the town.
    let arrival = plain.maps[1].links[0].to_hex;
    let grid = plain.maps[0].links[0].grid_hexes.clone();
    let taken: Vec<i32> = town.placements.iter().map(|p| p.hex).collect();
    let beside = (0..40_000)
        .find(|h| {
            distance_to(&grid, *h) == 1
                && !grid.contains(h)
                && *h != arrival
                && !taken.contains(h)
                && plain.maps[0].walkable[0][*h as usize]
        })
        .expect("a hex beside the door's grid");
    // Items are the town's art 2 (a crate).
    for (n, hex) in [arrival, beside].into_iter().enumerate() {
        town.placements.push(placement(
            2,
            hex,
            Role::Item,
            false,
            &format!("synthetic.esm:0091{n:02x}"),
        ));
    }
    let input = layer(
        vec![map_of(&town), map_of(&saloon)],
        &base,
        &saloon.critter_art,
    );
    let built = build_layer(&input).unwrap();
    let written = parsed(&built, &base);
    let town_items = item_hexes(&written[&built.maps[0].number], 0);
    let before = item_hexes(&parsed(&plain, &base)[&plain.maps[0].number], 0);
    let at = |items: &[i32], hex: i32| items.iter().filter(|h| **h == hex).count();
    assert_eq!(town_items.len(), before.len() + 2);
    assert!(
        at(&town_items, beside) > at(&before, beside),
        "radius 0 beside a door grid"
    );
    assert_eq!(
        at(&town_items, arrival),
        at(&before, arrival),
        "the item on the arrival hex moved off it"
    );
    assert_eq!(built.maps[0].elevations[0].moved_off_links, 1);
    assert!(keep_clear_of(&built, 0, 0).contains(&arrival));
    let again = build_layer(&input).unwrap();
    assert_eq!(again.layer.files(), built.layer.files());
}
