//! Islands, passages, the entrance and far doorsteps on synthetic scenes:
//! a town cut in two by a blocked line with a door on the far side, the
//! same cut by real walls, a tall strip over the old entrance, a door two
//! steps from walkable ground, and connected scenes left byte for byte.

mod common;

use std::collections::BTreeSet;

use common::*;
use map_builder::cover::{Cover, Sight};
use map_builder::islands::{self, components};
use map_builder::walk::{Frame, HEXES};
use map_builder::{ArtCategory, Built, Role, build_map};
use mir::{CellLocation, FormKey};
use vault_formats::map::MapFile;
use vault_formats::{ByteSource, GameProfile, HexGrid, LocalPrototypeCatalog, MemoryDir};

/// Every hex whose centre's tile coordinates `keep` accepts.
fn hexes_where(frame: &Frame, keep: impl Fn(f64, f64) -> bool) -> Vec<i32> {
    (0..HEXES as i32)
        .filter(|hex| frame.hex_centre_tile(*hex).is_some_and(|[s, t]| keep(s, t)))
        .collect()
}

/// The exterior map of a build, parsed back.
fn exterior(built: &Built) -> MapFile {
    let tree = MemoryDir::new(
        built
            .layer
            .files()
            .iter()
            .map(|(p, b)| (p.clone(), ByteSource::new(b.clone()).unwrap())),
    )
    .unwrap();
    let catalog = LocalPrototypeCatalog::for_game(&tree, GameProfile::Fallout2);
    MapFile::parse_for(
        &built.layer.files()[&built.report.map_file],
        &catalog,
        GameProfile::Fallout2,
    )
    .unwrap()
}

/// Whether, on the written map and by the engine's rule, a walker from the
/// entrance reaches `hex`.
fn reaches(built: &Built, hex: i32) -> bool {
    let map = exterior(built);
    let blocked = islands::engine_blocked(&map.objects.elevations[0]);
    let comps = components(|h| !blocked[h as usize]);
    comps.of(map.header.entry_hex).is_some() && comps.of(map.header.entry_hex) == comps.of(hex)
}

/// The centre cell's index in a town scene's cells.
fn centre_cell(scene: &Scene) -> usize {
    scene
        .cells
        .iter()
        .position(|c| matches!(c.location, CellLocation::Exterior { x: 0, y: 0, .. }))
        .unwrap()
}

/// Add a door on `hex` teleporting into interior `interior`, whose own
/// door teleports back to `back` (source space), when given.
fn add_door(scene: &mut Scene, hex: i32, form: u32, interior: u32, back: Option<[f32; 2]>) {
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
fn cut_town() -> (Scene, Vec<i32>) {
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

#[test]
fn a_town_cut_by_a_blocked_line_gets_a_passage_to_its_door() {
    let (scene, line) = cut_town();
    // Without reconnecting: the door's island is reported, left alone.
    let mut off = cut_town().0;
    off.options.reconnect = false;
    let before = build_map(&off.input()).unwrap();
    let report = &before.report;
    assert!(report.passages.is_empty());
    assert_eq!(report.islands_left.len(), 1, "{:?}", report.islands_left);
    let island = &report.islands_left[0];
    assert!(island.reason.contains("reconnect"));
    assert!(island.holds.iter().any(|h| h.starts_with("exit ")));
    assert!(island.holds.iter().any(|h| h.starts_with("return of ")));
    assert_eq!(report.components.unreachable_exits(), 1);
    assert_eq!(report.components.stranded_returns(), 1);
    assert!(report.components.sizes.len() >= 2);
    let exit = &report.exits[0];
    assert!(!reaches(&before, exit.grid_hexes[0]));

    // With it (the default): one passage across the line.
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    assert_eq!(report.passages.len(), 1, "{:?}", report.islands_left);
    assert!(report.islands_left.is_empty());
    let passage = &report.passages[0];
    assert!((1..=3).contains(&passage.length), "{passage:?}");
    assert_eq!(passage.opened.len(), passage.length, "no rings by default");
    assert_eq!(report.opened_hexes, passage.length);
    assert_eq!(passage.island_size, before.report.components.sizes[1]);
    let line: BTreeSet<i32> = line.into_iter().collect();
    for hex in &passage.opened {
        assert!(line.contains(hex), "only the line is opened: {hex}");
        assert!(built.walkable[*hex as usize]);
    }
    assert_eq!(passage.off_navmesh, 0, "the navmesh covers the line");
    // The written map: the exit and its return are reached from the
    // entrance, by the engine's rule, and the border is still sealed.
    let exit = &report.exits[0];
    assert!(reaches(&built, exit.grid_hexes[0]));
    assert!(reaches(&built, exit.return_hex));
    assert_eq!(report.components.unreachable_exits(), 0);
    assert_eq!(report.components.stranded_returns(), 0);
    assert_eq!(report.components.main, Some(0));
    assert_eq!(report.components.leaked, 0);
    assert!(report.components.sizes[0] > before.report.components.sizes[0]);

    // Wider passages are an option.
    let mut wide = cut_town().0;
    wide.options.passage_rings = 1;
    let built = build_map(&wide.input()).unwrap();
    let passage = &built.report.passages[0];
    assert!(passage.opened.len() > passage.length);
    assert!(passage.opened.iter().all(|h| line.contains(h)));
    // Too short a limit leaves the island, with the length it would need.
    let mut short = cut_town().0;
    short.options.passage_max_length = 1;
    let built = build_map(&short.input()).unwrap();
    if passage.length > 1 {
        assert!(built.report.passages.is_empty());
        assert!(
            built.report.islands_left[0].reason.contains("limit is 1"),
            "{:?}",
            built.report.islands_left
        );
    }
}

#[test]
fn a_real_wall_is_never_opened() {
    // The same cut made of blocking wall strips.
    let mut scene = town();
    let frame = scene.frame();
    let line = hexes_where(&frame, |s, t| {
        (25.0..75.0).contains(&s) && (52.0..53.0).contains(&t)
    });
    for (index, hex) in line.iter().enumerate() {
        scene.placements.push(placement(
            0,
            *hex,
            Role::WallStrip,
            true,
            &format!("synthetic.esm:0e{index:04x}"),
        ));
    }
    let door = hex_at(&frame, 50.0, 57.0);
    add_door(&mut scene, door, 0xb01, 0xd01, None);
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    assert!(report.passages.is_empty(), "{:?}", report.passages);
    assert_eq!(report.islands_left.len(), 1);
    assert!(
        report.islands_left[0].reason.contains("passage"),
        "{:?}",
        report.islands_left
    );
    assert_eq!(report.components.unreachable_exits(), 1);
    assert!(!reaches(&built, report.exits[0].grid_hexes[0]));

    // Blocked hexes with a landmark standing on them: its pixels cover the
    // line's ground, so they are its core, and stay closed too.
    let (mut scene, line) = cut_town();
    let grid = HexGrid::default();
    let centres: Vec<[i64; 2]> = line.iter().map(|h| grid.hex_center(*h).unwrap()).collect();
    let x0 = centres.iter().map(|c| c[0]).min().unwrap() - 12;
    let x1 = centres.iter().map(|c| c[0]).max().unwrap() + 12;
    let y0 = centres.iter().map(|c| c[1]).min().unwrap() - 12;
    let y1 = centres.iter().map(|c| c[1]).max().unwrap() + 12;
    let (width, height) = ((x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32);
    // Anchored on the line's lowest hex, as the slicer anchors a band, so
    // the band's rows are the line's.
    let anchor = *line.iter().min().unwrap();
    let at = grid.hex_center(anchor).unwrap();
    let bottom_centre = [x0 + i64::from(width / 2), y1];
    let mut band = image(ArtCategory::Scenery, width, height, 90, "landmark band");
    band.shift = [
        (bottom_centre[0] - at[0]) as i16,
        (bottom_centre[1] - at[1]) as i16,
    ];
    scene.art.push(band);
    let art = scene.art.len() - 1;
    scene.placements.push(placement(
        art,
        anchor,
        Role::Scenery,
        false,
        "synthetic.esm:0f0001",
    ));
    let cover = Cover::new(&scene.art, &scene.placements);
    assert!(line.iter().all(|h| cover.stands_on(*h)));
    let built = build_map(&scene.input()).unwrap();
    assert!(
        built.report.passages.is_empty(),
        "{:?}",
        built.report.passages
    );
    assert_eq!(built.report.islands_left.len(), 1);
    // Named by the caller, a line hex stays closed as well.
    let (mut scene, line) = cut_town();
    scene.options.protected_hexes = line.clone();
    let built = build_map(&scene.input()).unwrap();
    assert!(built.report.passages.is_empty());
}

#[test]
fn the_entrance_avoids_a_hex_a_tall_strip_covers() {
    let mut scene = town();
    let old = build_map(&scene.input()).unwrap().report;
    assert_eq!(old.entrance.sight, Sight::Clear);
    assert_eq!(old.entrance.steps_from_centre, 0);
    let before = old.entry_hex;
    // A tall, non-blocking strip two rows south of it: drawn after the
    // figure, over its box.
    let mut tall = image(ArtCategory::Walls, 64, 200, 80, "tall strip");
    tall.shift = [0, 0];
    scene.art.push(tall);
    let art = scene.art.len() - 1;
    let front = HexGrid::step(before, 2, 2).unwrap();
    scene.placements.push(placement(
        art,
        front,
        Role::WallStrip,
        false,
        "synthetic.esm:0f0002",
    ));
    let cover = Cover::new(&scene.art, &scene.placements);
    assert_eq!(
        cover.sight(before, map_builder::ENTRANCE_BOX),
        Sight::Covered
    );
    let built = build_map(&scene.input()).unwrap();
    let entrance = built.report.entrance;
    assert_ne!(entrance.hex, before);
    assert_eq!(entrance.sight, Sight::Clear);
    assert_eq!(
        cover.sight(entrance.hex, map_builder::ENTRANCE_BOX),
        Sight::Clear
    );
    assert!(entrance.in_main);
    assert!(built.walkable[entrance.hex as usize]);
    assert!(entrance.steps_from_centre <= 6, "{entrance:?}");
    assert_eq!(exterior(&built).header.entry_hex, entrance.hex);
    assert_eq!(built.report.components.main, Some(0));
}

#[test]
fn a_door_two_steps_from_walkable_ground_gets_its_exit() {
    let mut scene = town();
    let frame = scene.frame();
    let door = hex_at(&frame, 50.0, 48.0);
    // Its doorstep is blocked all round; the interior's door brings the
    // traveller out south of it.
    scene.blocked.extend(map_builder::walk::neighbours(door));
    let back = hex_point(&frame, hex_at(&frame, 50.0, 50.0));
    add_door(&mut scene, door, 0xb07, 0xd07, Some(back));
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    assert_eq!(report.exits.len(), 1, "{:?}", report.skipped_teleports);
    let exit = &report.exits[0];
    assert_eq!(exit.grid_steps, 2);
    assert!((1..=3).contains(&exit.grid_hexes.len()), "{exit:?}");
    let centre = frame.hex_centre_tile(door).unwrap();
    let out = frame.tile_of_source(f64::from(back[0]), f64::from(back[1]));
    for hex in &exit.grid_hexes {
        assert_eq!(HexGrid::lattice_distance(door, *hex).unwrap(), 2);
        assert!(built.walkable[*hex as usize]);
        let c = frame.hex_centre_tile(*hex).unwrap();
        let dot =
            (c[0] - centre[0]) * (out[0] - centre[0]) + (c[1] - centre[1]) * (out[1] - centre[1]);
        assert!(dot > 0.0, "on the outer side: {hex}");
    }
    // The return rule is unchanged: walkable, 2 or more steps out, no grid.
    assert!(exit.return_from_teleport);
    assert!(built.walkable[exit.return_hex as usize]);
    assert!(!exit.grid_hexes.contains(&exit.return_hex));
    assert!(HexGrid::lattice_distance(door, exit.return_hex).unwrap() >= 2);
    assert!(reaches(&built, exit.grid_hexes[0]));
    assert_eq!(report.components.unreachable_exits(), 0);

    // Not allowed to look that far: skipped, as before.
    scene.options.door_reach = 1;
    let built = build_map(&scene.input()).unwrap();
    assert!(built.report.exits.is_empty());
    assert!(
        built.report.skipped_teleports[0]
            .reason
            .contains("no walkable hex next to the door"),
        "{:?}",
        built.report.skipped_teleports
    );
}

/// FNV-1a over every layer path, file and the manifest text.
fn digest(layer: &emit::ModLayer) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    };
    for (path, bytes) in layer.files() {
        feed(path.as_bytes());
        feed(bytes);
    }
    feed(layer.manifest_text().unwrap().as_bytes());
    hash
}

#[test]
fn a_connected_scene_is_unchanged_byte_for_byte() {
    // The town before islands existed (`tests/critters.rs` pins the same
    // digests), and a connected town with doors and a critter, with and
    // without reconnecting.
    let mut scene = town();
    assert_eq!(
        digest(&build_map(&scene.input()).unwrap().layer),
        0x402c_95f0_06c1_2ebc
    );
    scene.base = base_files(10);
    assert_eq!(
        digest(&build_map(&scene.input()).unwrap().layer),
        0x5950_90e4_3aae_a43e
    );

    let mut scene = town();
    let frame = scene.frame();
    add_door(&mut scene, hex_at(&frame, 48.0, 48.0), 0xb01, 0xd01, None);
    add_door(&mut scene, hex_at(&frame, 56.0, 48.0), 0xb02, 0xd01, None);
    scene.base = base_files(10);
    scene.base.extend(critter_base_files());
    scene.critter_art = vec![sprite_set("townsman", 1)];
    scene.critters = vec![critter(
        0,
        "synthetic.esm:00c001",
        "Ann",
        hex_point(&frame, hex_at(&frame, 51.0, 53.0)),
        0.0,
    )];
    let on = build_map(&scene.input()).unwrap();
    assert!(on.report.passages.is_empty() && on.report.islands_left.is_empty());
    assert_eq!(on.report.exits.len(), 2);
    assert_eq!(on.report.components.unreachable_exits(), 0);
    assert_eq!(on.report.components.critters.len(), 1);
    assert_eq!(on.report.components.critters[0].component, Some(0));
    scene.options.reconnect = false;
    let off = build_map(&scene.input()).unwrap();
    assert_eq!(on.layer.files(), off.layer.files());
    assert_eq!(digest(&on.layer), digest(&off.layer));
}
