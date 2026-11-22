//! An interior cell built as a standalone demo map: a floor at z 1000 and a
//! balcony at z 1300 beside it, a teleport door, and three residents, one of
//! them on the balcony.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::*;
use map_builder::walk::{self, HEXES, Verdict};
use map_builder::{
    ArtCategory, BLANK_SQUARE, BuildOptions, InteriorBuild, Role, build_map, dominant_floor_z,
    navmesh_centre, read_layer, verify_layer,
};
use mir::{CellDoc, CellLocation, FormKey, GroundSampler, MapWindow, Reference};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::{ByteSource, GameProfile, HexGrid, LayeredVfs, LocalPrototypeCatalog};
use vault_formats::{MemoryDir, Vfs};

const FLOOR_Z: f32 = 1000.0;
const BALCONY_Z: f32 = 1300.0;
const SALOON: u32 = 0x7000;
const DOOR: u32 = 0x7101;
const RESIDENTS: [(u32, [f32; 3]); 3] = [
    (0x7201, [-200.0, 0.0, 1000.0]),
    (0x7202, [200.0, 100.0, 1010.0]),
    // On the balcony.
    (0x7203, [700.0, 0.0, 1300.0]),
];
/// Where the door stands: the floor's south wall.
const DOOR_AT: [f32; 3] = [0.0, -290.0, 1000.0];

fn form(id: u32) -> String {
    FormKey::new(PLUGIN, id).to_string()
}

fn resident(id: u32, position: [f32; 3]) -> Reference {
    let mut reference = teleport_door(id, position, 0, 0, None, None);
    reference.kind = mir::RefKind::Npc;
    reference.base_kind = Some("NPC_".into());
    reference.teleport = None;
    reference
}

/// The saloon: floor `[-400, 400] × [-300, 300]` at z 1000, balcony
/// `[500, 900] × [-300, 300]` at z 1300, a door out to an exterior cell,
/// and the residents.
fn saloon_cell() -> CellDoc {
    let mut references = vec![teleport_door(
        DOOR,
        DOOR_AT,
        0x7901,
        0x7900,
        None,
        Some((0, 0)),
    )];
    for (id, position) in RESIDENTS {
        references.push(resident(id, position));
    }
    references.sort_by(|a, b| a.form.cmp(&b.form));
    CellDoc {
        schema_version: mir::SCHEMA_VERSION,
        form: FormKey::new(PLUGIN, SALOON),
        editor_id: Some("SynthSaloon".into()),
        name: Some("Synthetic Saloon".into()),
        location: CellLocation::Interior,
        flags: 1,
        water_height: None,
        land: None,
        references,
        navmeshes: vec![
            navmesh(0x7301, &rectangle(-400.0, -300.0, 400.0, 300.0), FLOOR_Z),
            navmesh(0x7302, &rectangle(500.0, -300.0, 900.0, 300.0), BALCONY_Z),
        ],
    }
}

fn interior(clamp_levels: bool, entrance_near: Option<[f32; 2]>) -> BuildOptions {
    BuildOptions {
        interior: Some(InteriorBuild {
            entrance_near,
            clamp_levels,
            ..InteriorBuild::new(FLOOR_Z)
        }),
        ..BuildOptions::default()
    }
}

fn full_base() -> BTreeMap<String, Vec<u8>> {
    let mut base = base_files(10);
    base.extend(critter_base_files());
    base
}

/// The saloon scene over an empty ground sampler; half the squares blank,
/// and the library's second tile all transparent.
fn saloon(options: BuildOptions) -> Scene {
    let projection = projection();
    let window = MapWindow::from_center([0.0, 0.0], projection.tile_units());
    let frame = frame(&window, &projection);
    let door_hex = frame
        .hex_of_source(f64::from(DOOR_AT[0]), f64::from(DOOR_AT[1]))
        .unwrap();
    let critters = RESIDENTS
        .iter()
        .enumerate()
        .map(|(i, (id, p))| critter(0, &form(*id), &format!("Resident {i}"), [p[0], p[1]], 0.0))
        .collect();
    Scene {
        projection,
        window,
        sampler: GroundSampler::default(),
        tiles: vec![vec![10; 80 * 36], vec![0; 80 * 36]],
        squares: (0..10_000)
            .map(|q| match q % 3 {
                0 => 0,
                1 => 1,
                _ => BLANK_SQUARE,
            })
            .collect(),
        art: vec![
            image(ArtCategory::Scenery, 24, 50, 70, "saloon door"),
            image(ArtCategory::Scenery, 20, 30, 40, "bar stool"),
        ],
        placements: vec![
            placement(0, door_hex, Role::Door, true, &form(DOOR)),
            placement(
                1,
                frame.hex_of_source(-100.0, 150.0).unwrap(),
                Role::Scenery,
                true,
                "synthetic.esm:007401",
            ),
        ],
        blocked: Vec::new(),
        cells: vec![saloon_cell()],
        base: full_base(),
        critter_art: vec![sprite_set("resident", 3)],
        critters,
        options,
        name: "saloon".into(),
        number: None,
    }
}

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

#[test]
fn only_the_floor_level_is_walkable_and_no_terrain_is_read() {
    let scene = saloon(interior(false, None));
    let frame = scene.frame();
    let options = &scene.options;
    let (verdicts, stats) = walk::verdicts(&frame, &scene.sampler, &scene.cells, &[], options);
    let floor = frame.hex_of_source(0.0, 0.0).unwrap();
    let balcony = frame.hex_of_source(700.0, 0.0).unwrap();
    let outside = frame.hex_of_source(0.0, 600.0).unwrap();
    assert_eq!(verdicts[floor as usize], Verdict::Walkable);
    assert_eq!(verdicts[balcony as usize], Verdict::OffNavmesh);
    assert_eq!(verdicts[outside as usize], Verdict::OffNavmesh);
    assert_eq!(stats.triangles, 4);
    assert_eq!(stats.triangles_rasterized, 2);
    assert_eq!(stats.triangles_off_level, 2);
    assert_eq!(
        (
            stats.slope_only,
            stats.rejected_slope,
            stats.rejected_no_ground
        ),
        (0, 0, 0)
    );
    assert_eq!(stats.walkable, stats.covered);
    // Roughly the floor's area: 800 × 600 units, a hex about 70 × 61.
    assert!((80..160).contains(&stats.walkable), "{}", stats.walkable);

    // Steep terrain under the window changes nothing: no lookups.
    let steep = sampler(&scene.window, |x, _| x * 5.0);
    let (again, again_stats) = walk::verdicts(&frame, &steep, &scene.cells, &[], options);
    assert_eq!(again, verdicts);
    assert_eq!(again_stats, stats);
    assert_eq!(
        walk::interior_verdicts(
            &frame,
            &scene.cells,
            &[],
            options,
            options.interior.as_ref().unwrap()
        ),
        (verdicts.clone(), stats.clone())
    );

    // A blocked hex stays a blocked one.
    let mut blocked = vec![false; HEXES];
    blocked[floor as usize] = true;
    let (with_block, _) = walk::verdicts(&frame, &steep, &scene.cells, &blocked, options);
    assert_eq!(with_block[floor as usize], Verdict::Blocked);

    // The standable hexes are the floor level's covered ones.
    let standable = walk::standable_hexes(&frame, &steep, &scene.cells, options);
    let walkable: Vec<i32> = (0..HEXES as i32)
        .filter(|h| verdicts[*h as usize] == Verdict::Walkable)
        .collect();
    assert_eq!(standable, walkable);

    // The balcony as the floor level instead.
    let upstairs = BuildOptions {
        interior: Some(InteriorBuild::new(BALCONY_Z)),
        ..BuildOptions::default()
    };
    let (up, _) = walk::verdicts(&frame, &scene.sampler, &scene.cells, &[], &upstairs);
    assert_eq!(up[floor as usize], Verdict::OffNavmesh);
    assert_eq!(up[balcony as usize], Verdict::Walkable);
}

#[test]
fn floor_level_and_centre_helpers() {
    let cells = vec![saloon_cell()];
    let z = dominant_floor_z(&cells).unwrap();
    assert!((z - FLOOR_Z).abs() < 1e-3, "{z}");
    let centre = navmesh_centre(&cells, &InteriorBuild::new(FLOOR_Z)).unwrap();
    assert!(
        centre[0].abs() < 1e-3 && centre[1].abs() < 1e-3,
        "{centre:?}"
    );
    let up = navmesh_centre(&cells, &InteriorBuild::new(BALCONY_Z)).unwrap();
    assert!((up[0] - 700.0).abs() < 1e-3 && up[1].abs() < 1e-3, "{up:?}");
    assert_eq!(navmesh_centre(&cells, &InteriorBuild::new(5000.0)), None);
    // Exterior cells are not an interior's.
    assert_eq!(dominant_floor_z(&[exterior_cell(0, 0)]), None);
}

#[test]
fn the_entrance_is_near_the_given_point_or_the_navmesh_centre() {
    let near_door = [0.0, -200.0];
    let scene = saloon(interior(false, Some(near_door)));
    let built = build_map(&scene.input()).unwrap();
    let frame = scene.frame();
    let target = frame
        .hex_of_source(f64::from(near_door[0]), f64::from(near_door[1]))
        .unwrap();
    let entry = built.report.entry_hex;
    let steps = HexGrid::lattice_distance(target, entry).unwrap();
    assert!(
        steps <= 3,
        "entrance {entry} is {steps} steps from {target}"
    );
    assert!(built.walkable[entry as usize]);
    assert!(built.report.entrance.in_main);
    let account = built.report.interior.as_ref().unwrap();
    assert_eq!(account.entrance_near, near_door);
    assert!(account.entrance_near_given);

    let centred = build_map(&saloon(interior(false, None)).input()).unwrap();
    let centre = frame.hex_of_source(0.0, 0.0).unwrap();
    let entry = centred.report.entry_hex;
    assert!(HexGrid::lattice_distance(centre, entry).unwrap() <= 3);
    let account = centred.report.interior.as_ref().unwrap();
    assert!(!account.entrance_near_given);
    assert!(account.entrance_near[0].abs() < 1e-3 && account.entrance_near[1].abs() < 1e-3);
}

#[test]
fn a_one_map_layer_with_the_door_reported_and_the_balcony_resident_skipped() {
    let scene = saloon(interior(false, Some([0.0, -200.0])));
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    let door_hex = scene.placements[0].hex;

    // One map, numbered and registered as usual; no exits, no stubs.
    assert_eq!(report.map_file, "maps/saloon.map");
    let number = report.first_map_number;
    assert_eq!(report.map_number, number);
    assert_eq!(report.maps, vec![("saloon.map".to_owned(), number)]);
    assert!(report.exits.is_empty());
    assert!(report.maps_txt);
    let maps: Vec<&String> = built
        .layer
        .files()
        .keys()
        .filter(|p| p.starts_with("maps/"))
        .collect();
    assert_eq!(maps, vec!["maps/saloon.map"]);
    let text = String::from_utf8(built.layer.files()["data/maps.txt"].clone()).unwrap();
    assert_eq!(text.matches("[Map ").count(), 3, "{text}");
    assert!(text.contains(&format!("[Map {number:03}]\nmap_name=saloon\n")));
    assert_eq!(report.verify.maps.len(), 1);
    assert_eq!(report.verify.maps[0].exit_grids, 0);
    assert!(
        !built.layer.files().contains_key("proto/misc/misc.lst"),
        "no exit-grid prototype"
    );

    // The door: drawn, reported, not converted.
    let account = report.interior.as_ref().unwrap();
    assert_eq!(
        account.cells,
        vec![(form(SALOON), Some("SynthSaloon".into()))]
    );
    assert_eq!(account.band, [960.0, 1100.0]);
    assert_eq!(account.doors.len(), 1);
    let door = &account.doors[0];
    assert_eq!(door.door, form(DOOR));
    assert_eq!(door.hex, Some(door_hex));
    assert_eq!(door.destination, Some(form(0x7900)));
    assert_eq!(report.skipped_teleports.len(), 1);
    assert_eq!(report.skipped_teleports[0].door, form(DOOR));
    assert!(
        report.skipped_teleports[0]
            .reason
            .starts_with("not converted on an interior demo"),
        "{}",
        report.skipped_teleports[0].reason
    );

    // Two residents on the floor, the third skipped as on the balcony.
    assert_eq!(report.critters.len(), 2);
    for critter in &report.critters {
        assert!(built.walkable[critter.hex as usize]);
        assert!(critter.steps <= 2, "{critter:?}");
    }
    assert_eq!(report.skipped_critters.len(), 1);
    let skipped = &report.skipped_critters[0];
    assert_eq!(skipped.reference, form(RESIDENTS[2].0));
    assert!(
        skipped.reason.starts_with("on another level"),
        "{}",
        skipped.reason
    );
    assert_eq!(account.other_level, vec![skipped.clone()]);
    assert!(account.clamped.is_empty() && account.unknown_level.is_empty());

    // The written map: no exit grid, the door and the critters on it,
    // blank squares drawn blank, the transparent library tile not written.
    let vfs = mounted(&built.layer, &scene.base);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let map = MapFile::parse_for(
        &vfs.read("maps/saloon.map").unwrap(),
        &catalog,
        GameProfile::Fallout2,
    )
    .unwrap();
    let objects = &map.objects.elevations[0];
    assert!(!objects.iter().any(|o| matches!(
        o.state,
        ObjectState::Other {
            detail: ObjectDetail::ExitGrid { .. },
            ..
        }
    )));
    assert!(objects.iter().any(|o| o.hex == Some(door_hex)));
    let tiles = map.tiles[0].as_ref().unwrap();
    for (square, pair) in tiles.iter().enumerate() {
        if square % 3 != 0 {
            assert_eq!(pair.floor(), 1, "square {square}");
        } else {
            assert_ne!(pair.floor(), 1, "square {square}");
        }
    }
    assert_eq!(report.art_entries.get("tiles"), Some(&1));
    assert_eq!(map.header.entry_hex, report.entry_hex);

    // The summary says all of it.
    let summary = report.interior_summary().join("\n");
    assert!(
        summary.contains(&format!("interior demo: maps/saloon.map (map {number})")),
        "{summary}"
    );
    assert!(
        summary.contains(&format!(
            "door {} to {}: not converted on an interior demo (drawn on hex {door_hex})",
            form(DOOR),
            form(0x7900)
        )),
        "{summary}"
    );
    assert!(summary.contains(&format!(
        "skipped {}: on another level",
        form(RESIDENTS[2].0)
    )));
    assert_eq!(summary.matches("\nplaced ").count(), 2, "{summary}");

    // The layer survives a write and a read from disk.
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().join("layer");
    built.layer.write(&dir).unwrap();
    let back = read_layer(&dir).unwrap();
    assert_eq!(verify_layer(&back, &scene.base).unwrap(), report.verify);
}

#[test]
fn clamping_levels_places_the_balcony_resident_on_the_floor() {
    let scene = saloon(interior(true, None));
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    assert_eq!(report.critters.len(), 3, "{:?}", report.skipped_critters);
    assert!(report.skipped_critters.is_empty());
    let up = report
        .critters
        .iter()
        .find(|c| c.reference == form(RESIDENTS[2].0))
        .unwrap();
    assert!(built.walkable[up.hex as usize]);
    assert!(up.steps > 0 && up.steps <= map_builder::INTERIOR_CLAMP_RADIUS);
    let account = report.interior.as_ref().unwrap();
    assert_eq!(account.clamped, vec![(form(RESIDENTS[2].0), BALCONY_Z)]);
    assert!(account.other_level.is_empty());
    assert!(
        report
            .interior_summary()
            .join("\n")
            .contains("from another level (z 1300)")
    );
    // Every critter is in the entrance's component.
    let main = report.components.main;
    assert!(main.is_some());
    assert!(
        report
            .components
            .critters
            .iter()
            .all(|c| c.component == main)
    );
    assert_eq!(
        verify_layer(&built.layer, &scene.base).unwrap(),
        report.verify
    );
}

#[test]
fn interior_builds_are_deterministic_and_reject_bad_settings() {
    let scene = saloon(interior(false, None));
    let a = build_map(&scene.input()).unwrap();
    let b = build_map(&scene.input()).unwrap();
    assert_eq!(a.layer.files(), b.layer.files());
    assert_eq!(a.report, b.report);

    let mut bad = saloon(interior(false, None));
    bad.options.interior.as_mut().unwrap().below = -1.0;
    assert!(matches!(
        build_map(&bad.input()),
        Err(map_builder::BuildError::Input(_))
    ));
    let mut bad = saloon(interior(false, Some([f32::NAN, 0.0])));
    bad.options.interior.as_mut().unwrap().clamp_radius = 3;
    assert!(build_map(&bad.input()).is_err());

    // A floor level with no navmesh: nothing to walk on.
    let empty = saloon(BuildOptions {
        interior: Some(InteriorBuild::new(5000.0)),
        ..BuildOptions::default()
    });
    assert!(matches!(
        build_map(&empty.input()),
        Err(map_builder::BuildError::NoWalkableHex)
    ));

    // An exterior build of the same scene does not see the interior's
    // navmesh at all.
    let exterior = saloon(BuildOptions::default());
    assert!(build_map(&exterior.input()).is_err());
}
