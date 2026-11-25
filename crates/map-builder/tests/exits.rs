//! Door -> exit grid -> stub map -> exit grid back, on hand-built cells and
//! on the synthetic install of `mir::fixture::mini_town`.

mod common;

use std::collections::BTreeMap;

use common::scenes::doors_scene;
use common::*;
use map_builder::exits::STUB_ENTRY;
use map_builder::{ArtCategory, BuildOptions, Built, GroundTiles, Role, build_map};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::pro::PrototypeLookup;
use vault_formats::{ByteSource, GameProfile, HexGrid, LocalPrototypeCatalog, MemoryDir};

/// Every exit grid of a map: hex -> (map, hex, elevation, orientation).
fn grids(map: &MapFile) -> BTreeMap<i32, (i32, i32, i32, i32)> {
    map.objects.elevations[0]
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

fn parse(built: &Built, base: &BTreeMap<String, Vec<u8>>, path: &str) -> MapFile {
    let mut files: BTreeMap<String, Vec<u8>> = base.clone();
    files.extend(built.layer.files().clone());
    let tree = MemoryDir::new(
        files
            .into_iter()
            .map(|(p, b)| (p, ByteSource::new(b).unwrap())),
    )
    .unwrap();
    let catalog = LocalPrototypeCatalog::for_game(&tree, GameProfile::Fallout2);
    let map =
        MapFile::parse_for(&built.layer.files()[path], &catalog, GameProfile::Fallout2).unwrap();
    for object in map.objects.elevations.iter().flatten() {
        if object.pid >> 24 == 2 || object.pid >> 24 == 0 {
            catalog.lookup(object.pid).unwrap();
        }
    }
    map
}

/// Check the chain: exterior grids lead to the stub's entrance, the stub's
/// ring leads back onto a walkable exterior hex that holds no grid.
fn check_chain(built: &Built, base: &BTreeMap<String, Vec<u8>>) {
    let report = &built.report;
    assert!(!report.exits.is_empty());
    let exterior = parse(built, base, &report.map_file);
    let outside = grids(&exterior);
    for exit in &report.exits {
        assert!(!exit.grid_hexes.is_empty());
        for hex in &exit.grid_hexes {
            assert_eq!(
                HexGrid::lattice_distance(*hex, exit.door_hex).unwrap(),
                1,
                "grids stand next to the door"
            );
            assert!(built.walkable[*hex as usize]);
            assert_eq!(outside[hex], (exit.stub_number as i32, STUB_ENTRY, 0, 0));
        }
        let stub = parse(built, base, &format!("maps/{}", exit.stub_file));
        assert_eq!(stub.header.map_number, exit.stub_number as i32);
        assert_eq!(stub.header.entry_hex, STUB_ENTRY);
        let inside = grids(&stub);
        assert_eq!(inside.len(), 18);
        let (map, hex, elevation, orientation) = *inside.values().next().unwrap();
        assert!(
            inside
                .values()
                .all(|g| *g == (map, hex, elevation, orientation))
        );
        assert_eq!(map, report.map_number as i32);
        assert_eq!(elevation, 0);
        assert!((0..6).contains(&orientation));
        assert!(built.walkable[hex as usize], "the return hex is walkable");
        assert!(!outside.contains_key(&hex), "returning does not re-enter");
        let back = HexGrid::lattice_distance(hex, exit.door_hex).unwrap();
        assert!(
            (2..=map_builder::exits::RETURN_RADIUS).contains(&back),
            "the stub returns to its own door"
        );
        assert_eq!(hex, exit.return_hex);
        // The stub's entrance stands on a floor tile, inside the ring.
        let square = map_builder::walk::Frame::square_of_hex(STUB_ENTRY).unwrap();
        assert_ne!(stub.tiles[0].as_ref().unwrap()[square as usize].floor(), 1);
        assert!(!inside.contains_key(&STUB_ENTRY));
    }
    let registered: Vec<u32> = built
        .layer
        .manifest()
        .maps
        .iter()
        .map(|m| m.number)
        .collect();
    let numbers: Vec<u32> = report.maps.iter().map(|(_, n)| *n).collect();
    assert_eq!(registered, numbers);
}

#[test]
fn doors_become_exits_to_one_stub_per_door() {
    let scene = doors_scene();
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    assert_eq!(report.exits.len(), 3);
    assert_eq!(report.maps.len(), 4, "the exterior and one stub per door");
    assert_eq!(report.maps[0], ("synth.map".into(), 66));
    assert_eq!(report.maps[1], ("synt_000.map".into(), 67));
    assert_eq!(report.maps[2], ("synt_001.map".into(), 68));
    assert_eq!(report.maps[3], ("synt_002.map".into(), 69));
    // Two doors open on one interior, and each still has a stub of its
    // own: leaving comes out at the door that was used.
    let shared: Vec<_> = report
        .exits
        .iter()
        .filter(|e| e.interior == report.exits[0].interior)
        .collect();
    assert_eq!(shared.len(), 2, "two doors share one interior");
    assert_ne!(shared[0].stub_number, shared[1].stub_number);
    assert_ne!(shared[0].return_hex, shared[1].return_hex);
    assert!(report.exits.iter().all(|e| !e.return_from_teleport));
    assert_eq!(report.skipped_teleports.len(), 2);
    assert!(
        report
            .skipped_teleports
            .iter()
            .any(|s| s.reason.contains("another exterior"))
    );
    assert!(
        report
            .skipped_teleports
            .iter()
            .any(|s| s.reason.contains("plugin list"))
    );
    // Misc prototypes padded to 16 for the exit grid.
    let misc = std::str::from_utf8(&built.layer.files()["proto/misc/misc.lst"]).unwrap();
    assert_eq!(misc.lines().count(), 16);
    check_chain(&built, &scene.base);
}

#[test]
fn with_a_base_the_exit_grid_prototype_is_the_installs() {
    let mut scene = doors_scene();
    scene.base = base_files(10);
    let built = build_map(&scene.input()).unwrap();
    assert!(!built.layer.files().contains_key("proto/misc/misc.lst"));
    assert!(built.layer.files().contains_key("art/misc/misc.lst"));
    assert_eq!(built.report.map_number, 151);
    let text = std::str::from_utf8(&built.layer.files()["data/maps.txt"]).unwrap();
    for number in [151, 152, 153] {
        assert!(text.contains(&format!("[Map {number}]")));
    }
    check_chain(&built, &scene.base);
}

#[test]
fn standalone_exits_lead_out_and_back_over_the_installs_blank_tile() {
    let mut scene = doors_scene();
    scene.base = base_files(10);
    scene.options.standalone = true;
    let built = build_map(&scene.input()).unwrap();
    check_chain(&built, &scene.base);
    let files = built.layer.files();
    assert!(files["art/tiles/tiles.lst"].starts_with(b"bti0000.frm\nbti0001.frm\nmft00000.frm\n"));
    assert!(
        !files.contains_key("proto/misc/misc.lst"),
        "misc is the install's"
    );
    // Stub maps lay the blank tile 1, the install's, outside their floor;
    // verification resolved it with every other floor.
    let stub = &built.report.exits[0].stub_file;
    let map = parse(&built, &scene.base, &format!("maps/{stub}"));
    assert!(
        map.tiles[0]
            .as_ref()
            .unwrap()
            .iter()
            .any(|t| t.floor() == 1)
    );
    assert_eq!(built.report.verify.replaced_art_lists, ["walls", "tiles"]);
    assert_eq!(built.report.verify.maps.len(), built.report.maps.len());
}

#[test]
fn the_mini_town_door_leads_into_its_house_and_back() {
    let install = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let town = mir::fixture::mini_town(install.path());
    let dir = out.path().join("mir");
    mir::extract(&town.options(&dir, 1)).unwrap();
    let mir_dir = mir::Mir::open(&dir).unwrap();

    let projection = projection();
    let door_ref = mir_dir
        .cell(mir::fixture::WORLDSPACE, 0, 0)
        .unwrap()
        .unwrap()
        .references
        .into_iter()
        .find(|r| r.form == town.exterior_door)
        .unwrap();
    let door_position = door_ref.transform.position;
    let window = mir::MapWindow::from_center(
        [door_position[0], door_position[1]],
        projection.tile_units(),
    );
    let frame = frame(&window, &projection);
    let sampler = mir::GroundSampler::load(&mir_dir, mir::fixture::WORLDSPACE, &window).unwrap();
    let mut cells = Vec::new();
    let (xs, ys) = window.cells(0);
    for y in ys {
        for x in xs.clone() {
            if let Some(cell) = mir_dir.cell(mir::fixture::WORLDSPACE, x, y).unwrap() {
                cells.push(cell);
            }
        }
    }
    cells.push(mir_dir.interior(mir::fixture::INTERIOR).unwrap().unwrap());

    let door_hex = frame
        .hex_of_source(f64::from(door_position[0]), f64::from(door_position[1]))
        .unwrap();
    let art = vec![
        image(ArtCategory::Scenery, 24, 50, 70, "door"),
        image(ArtCategory::Walls, 32, 60, 30, "house wall"),
    ];
    let wall_hex = HexGrid::step(door_hex, 0, 2).unwrap();
    let placements = vec![
        placement(
            0,
            door_hex,
            Role::Door,
            true,
            &town.exterior_door.to_string(),
        ),
        placement(1, wall_hex, Role::WallStrip, true, &town.house.to_string()),
    ];
    let tiles = library();
    let squares = squares();
    let base = BTreeMap::new();
    let input = map_builder::BuildInput {
        map_name: "minitown",
        map_number: None,
        layer_id: "mini-town",
        layer_name: "Mini town",
        window,
        sampler: &sampler,
        projection: &projection,
        ground: GroundTiles {
            tiles: &tiles,
            squares: &squares,
        },
        art: &art,
        placements: &placements,
        blocked_hexes: &[],
        cells: &cells,
        base_files: &base,
        critter_art: &[],
        critters: &[],
        options: BuildOptions {
            // The door's island is too far from the entrance to join (a
            // passage of 12 hexes, the limit 6): this test follows the
            // chain through its grids, which closing the island would
            // make not walkable.
            close_unreached: false,
            ..BuildOptions::default()
        },
    };
    let built = build_map(&input).unwrap();
    let report = &built.report;
    assert_eq!(report.exits.len(), 1, "{:?}", report.skipped_teleports);
    let exit = &report.exits[0];
    assert_eq!(exit.door, town.exterior_door.to_string());
    assert_eq!(exit.interior, town.interior_cell.to_string());
    assert_eq!(
        exit.interior_editor_id.as_deref(),
        Some(mir::fixture::INTERIOR)
    );
    assert!(
        exit.return_from_teleport,
        "the interior door's teleport gives the return point"
    );
    assert_eq!(exit.stub_file, "mini_000.map");
    // The navmesh is in the centre cell only; elsewhere, slope decides.
    assert!(report.walk.triangles > 0 && report.walk.covered > 0);
    assert!(report.walk.slope_only > 0);
    check_chain(&built, &base);

    // The whole layer survives a write and a read from disk.
    let target = out.path().join("layer");
    built.layer.write(&target).unwrap();
    let back = map_builder::read_layer(&target).unwrap();
    assert_eq!(
        map_builder::verify_layer(&back, &base).unwrap(),
        report.verify
    );
}
