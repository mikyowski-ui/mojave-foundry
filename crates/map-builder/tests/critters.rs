//! Non-player characters on the synthetic town: placement, facing, the
//! sprite sets, prototypes and names, verification, determinism, and that
//! a build without critters has not moved.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use common::*;
use map_builder::critters::{CRITTER_RADIUS, DEFAULT_CRITTER_TEAM};
use map_builder::walk::{Frame, HEXES};
use map_builder::{BuildError, Built, Role, build_map, read_layer, verify_layer};
use mir::{CellLocation, FormKey};
use vault_formats::map::{MapFile, ObjectState};
use vault_formats::pro::{PrototypeData, PrototypeKind, PrototypeLookup};
use vault_formats::{
    ArtCatalog, ByteSource, Fid, FrmArchive, GameProfile, HexGrid, LayeredVfs,
    LocalPrototypeCatalog, MemoryDir, MsgCatalog, Vfs,
};

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

/// The town's layers before critters existed: without a base, and over
/// `base_files(10)`.
const BEFORE_EMPTY: u64 = 0x402c_95f0_06c1_2ebc;
const BEFORE_BASE: u64 = 0x5950_90e4_3aae_a43e;

fn full_base() -> BTreeMap<String, Vec<u8>> {
    let mut base = base_files(10);
    base.extend(critter_base_files());
    base
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

fn parse_map(vfs: &LayeredVfs, path: &str) -> MapFile {
    let catalog = LocalPrototypeCatalog::for_game(vfs, GameProfile::Fallout2);
    MapFile::parse_for(&vfs.read(path).unwrap(), &catalog, GameProfile::Fallout2).unwrap()
}

/// The town with a full base and two sprite sets (a third unused).
fn scene() -> Scene {
    let mut scene = town();
    scene.base = full_base();
    scene.critter_art = vec![
        sprite_set("townsman", 1),
        sprite_set("townswoman", 2),
        sprite_set("unused", 3),
    ];
    scene
}

/// The free walkable hex nearest `position` within the radius, by brute
/// force over the whole grid.
fn nearest_free(
    frame: &Frame,
    walkable: &[bool],
    position: [f32; 2],
    excluded: &BTreeSet<i32>,
) -> Option<i32> {
    let target = frame.tile_of_source(f64::from(position[0]), f64::from(position[1]));
    let origin = frame.hex_of_tile(target)?;
    let mut best: Option<(f64, i32)> = None;
    for hex in 0..HEXES as i32 {
        if !walkable[hex as usize]
            || excluded.contains(&hex)
            || HexGrid::lattice_distance(origin, hex).unwrap() > CRITTER_RADIUS
        {
            continue;
        }
        let c = frame.hex_centre_tile(hex).unwrap();
        let d = (c[0] - target[0]).powi(2) + (c[1] - target[1]).powi(2);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, hex));
        }
    }
    best.map(|(_, hex)| hex)
}

/// The critter objects of the exterior map, by hex.
fn critter_objects(
    built: &Built,
    base: &BTreeMap<String, Vec<u8>>,
) -> BTreeMap<i32, (u8, u32, u32, i32)> {
    let vfs = mounted(&built.layer, base);
    let map = parse_map(&vfs, &built.report.map_file);
    map.objects.elevations[0]
        .iter()
        .filter_map(|object| match &object.state {
            ObjectState::Critter(state) => Some((
                object.hex.unwrap(),
                (object.orientation, object.pid, object.art_id, state.hp),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn critters_stand_on_the_nearest_free_walkable_hex_and_never_share_one() {
    let mut scene = scene();
    let frame = scene.frame();
    let open = hex_at(&frame, 51.0, 53.0);
    let rock = scene.placements[2].hex;
    let at_open = hex_point(&frame, open);
    // Off the centre of a hex: still nearest that hex.
    let near_open = [at_open[0] + 20.0, at_open[1] - 10.0];
    scene.critters = vec![
        critter(0, "synthetic.esm:00c001", "Ann", near_open, 0.0),
        // The same spot again, and a third time: the next nearest hexes.
        critter(0, "synthetic.esm:00c002", "Ben", near_open, 0.0),
        critter(1, "synthetic.esm:00c003", "Cal", near_open, 0.0),
        // On the rock, which blocks.
        critter(
            0,
            "synthetic.esm:00c004",
            "Dee",
            hex_point(&frame, rock),
            0.0,
        ),
        // Far off the navmesh: nothing walkable within reach.
        critter(
            0,
            "synthetic.esm:00c005",
            "Eve",
            hex_point(&frame, hex_at(&frame, 15.0, 15.0)),
            0.0,
        ),
    ];
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    assert_eq!(report.critters.len(), 4, "{:?}", report.skipped_critters);
    assert_eq!(report.skipped_critters.len(), 1);
    assert_eq!(report.skipped_critters[0].reference, "synthetic.esm:00c005");
    assert_eq!(report.skipped_critters[0].index, 4);
    assert!(
        report.skipped_critters[0]
            .reason
            .contains("no free walkable hex")
    );

    assert_eq!(report.critters[0].hex, open);
    assert_eq!(report.critters[0].steps, 0);
    assert!(report.critters[0].moved < 30.0);
    let mut excluded: BTreeSet<i32> = [report.entry_hex].into();
    for placed in &report.critters {
        let wanted = nearest_free(
            &frame,
            &built.walkable,
            scene.critters[placed.index].position,
            &excluded,
        );
        assert_eq!(Some(placed.hex), wanted, "{}", placed.reference);
        assert!(built.walkable[placed.hex as usize]);
        assert!(
            excluded.insert(placed.hex),
            "two critters on hex {}",
            placed.hex
        );
    }
    assert_ne!(report.critters[3].hex, rock);
    assert!(report.critters[3].steps >= 1);
    assert!(report.critters[1].moved > report.critters[0].moved);

    // On the map: one critter object per placed critter, on its hex.
    let objects = critter_objects(&built, &scene.base);
    assert_eq!(objects.len(), 4);
    for placed in &report.critters {
        assert_eq!(objects[&placed.hex].1, placed.pid);
    }
    // Critters leave walkability and the border alone.
    let mut plain = town();
    plain.base = full_base();
    let without = build_map(&plain.input()).unwrap();
    assert_eq!(without.walkable, built.walkable);
    assert_eq!(without.report.blockers, built.report.blockers);
}

/// The town plus one door teleporting into an interior.
fn door_scene() -> Scene {
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

#[test]
fn no_critter_takes_an_exit_grid_a_door_the_entrance_or_a_return_hex() {
    let mut scene = door_scene();
    let first = build_map(&scene.input()).unwrap();
    let exit = first.report.exits[0].clone();
    let entry = first.report.entry_hex;
    let frame = scene.frame();
    let mut targets: Vec<i32> = exit.grid_hexes.clone();
    targets.extend([exit.return_hex, exit.door_hex, entry]);
    scene.critters = targets
        .iter()
        .enumerate()
        .map(|(i, hex)| {
            critter(
                0,
                &format!("synthetic.esm:00d{i:03}"),
                "Guard",
                hex_point(&frame, *hex),
                1.0,
            )
        })
        .collect();
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    assert_eq!(report.exits, first.report.exits, "critters move no exit");
    assert_eq!(report.entry_hex, entry, "critters move no entrance");
    assert_eq!(report.critters.len(), targets.len());
    let forbidden: BTreeSet<i32> = targets.iter().copied().collect();
    let hexes: BTreeSet<i32> = report.critters.iter().map(|c| c.hex).collect();
    assert_eq!(hexes.len(), report.critters.len());
    assert!(
        hexes.is_disjoint(&forbidden),
        "{hexes:?} meets {forbidden:?}"
    );
    for placed in &report.critters {
        assert!(placed.steps >= 1);
    }
    // The map's grids are where they were, and no critter is on one.
    let objects = critter_objects(&built, &scene.base);
    assert!(objects.keys().all(|hex| !forbidden.contains(hex)));
    // Every critter reached through the mounted layer is still walkable
    // ground: the border did not seal it in.
    for hex in objects.keys() {
        assert!(built.walkable[*hex as usize]);
    }
}

#[test]
fn orientation_and_yaw_round_trip_from_the_projection() {
    let check = |frame: &Frame| {
        for d in 0..6u8 {
            let yaw = frame.yaw_of_orientation(d).unwrap();
            assert_eq!(frame.orientation_of_yaw(yaw), Some(d), "direction {d}");
            for wobble in [-0.2f32, 0.2] {
                assert_eq!(frame.orientation_of_yaw(yaw + wobble), Some(d));
            }
            assert_eq!(
                frame.orientation_of_yaw(yaw + std::f32::consts::TAU),
                Some(d)
            );
            // That yaw's facing projects onto the direction's hex step.
            let facing = [f64::from(yaw.sin()), f64::from(yaw.cos())];
            let from = frame.tile_of_source(1000.0, 1000.0);
            let to = frame.tile_of_source(1000.0 + 100.0 * facing[0], 1000.0 + 100.0 * facing[1]);
            let a = frame.screen_of_tile(from);
            let b = frame.screen_of_tile(to);
            let screen = [b[0] - a[0], b[1] - a[1]];
            let step = emit::HEX_STEPS[usize::from(d)].map(f64::from);
            let cross = screen[0] * step[1] - screen[1] * step[0];
            let dot = screen[0] * step[0] + screen[1] * step[1];
            assert!(dot > 0.0, "direction {d}");
            assert!(
                cross.abs() < 1e-3 * dot,
                "direction {d}: {screen:?} vs {step:?}"
            );
        }
        assert_eq!(frame.orientation_of_yaw(f32::NAN), None);
        assert_eq!(frame.yaw_of_orientation(6), None);
    };
    let scene = town();
    let frame = scene.frame();
    check(&frame);
    // Under the default projection the north-west step is due north, so a
    // reference with zero rotation faces NW; positive rotation turns it
    // clockwise seen from above, towards east, so it reaches NE (0) first.
    assert_eq!(frame.orientation_of_yaw(0.0), Some(5));
    assert_eq!(frame.orientation_of_yaw(0.6), Some(0));
    assert_eq!(frame.orientation_of_yaw(-0.6), Some(4));
    assert_eq!(frame.orientation_of_yaw(std::f32::consts::PI), Some(2));
    assert!(frame.yaw_of_orientation(5).unwrap().abs() < 1e-6);
    // A window turned by a quarter turn turns the mapping with it.
    let calibration = foundry_render::Calibration {
        yaw_quarter_turns: 1,
        ..Default::default()
    };
    let projection = foundry_render::Projection::new(&calibration).unwrap();
    let turned = Frame::new(&window(&projection), &projection).unwrap();
    check(&turned);
    assert_ne!(turned.yaw_of_orientation(5), frame.yaw_of_orientation(5));

    // On the map: one critter per direction, at the yaw of that direction.
    let mut scene = self::scene();
    scene.critters = (0..6u8)
        .map(|d| {
            let hex = hex_at(&frame, 44.0 + 3.0 * f64::from(d), 52.0);
            critter(
                0,
                &format!("synthetic.esm:00e00{d}"),
                "Walker",
                hex_point(&frame, hex),
                frame.yaw_of_orientation(d).unwrap(),
            )
        })
        .collect();
    let built = build_map(&scene.input()).unwrap();
    assert_eq!(built.report.critters.len(), 6);
    let objects = critter_objects(&built, &scene.base);
    for (d, placed) in built.report.critters.iter().enumerate() {
        assert_eq!(usize::from(placed.orientation), d);
        assert_eq!(usize::from(objects[&placed.hex].0), d);
    }
}

#[test]
fn a_sprite_set_without_a_walk_or_with_a_bad_file_is_refused() {
    let frame = scene().frame();
    let at = hex_point(&frame, hex_at(&frame, 50.0, 52.0));
    for (case, change) in [
        (
            "no walk",
            (|set: &mut map_builder::CritterArt| {
                set.frms.remove("ab");
            }) as fn(&mut map_builder::CritterArt),
        ),
        ("no stand", |set| {
            set.frms.remove("aa");
        }),
        ("bad suffix", |set| {
            let bytes = set.frms["ao"].clone();
            set.frms.insert("a1".into(), bytes);
        }),
        ("not an FRM", |set| {
            set.frms.insert("ao".into(), vec![1, 2, 3]);
        }),
    ] {
        let mut scene = scene();
        change(&mut scene.critter_art[1]);
        scene.critters = vec![critter(1, "synthetic.esm:00f001", "Ann", at, 0.0)];
        match build_map(&scene.input()) {
            Err(BuildError::Input(message)) => {
                assert!(message.contains("townswoman"), "{case}: {message}")
            }
            other => panic!("{case}: {other:?}"),
        }
    }
    // An art index out of range, the player's team, a missing packet.
    let mut scene = scene();
    scene.critters = vec![critter(7, "synthetic.esm:00f002", "Ann", at, 0.0)];
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::Input(_))
    ));
    scene.critters[0].art = 0;
    scene.options.critter_team = 0;
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::Input(_))
    ));
    scene.options.critter_team = DEFAULT_CRITTER_TEAM;
    scene.options.critter_ai_packet = Some(9);
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::Input(_))
    ));
    // A base that lacks the critter lists would clobber the install's.
    scene.options.critter_ai_packet = None;
    scene.base.remove("proto/critters/critters.lst");
    let error = build_map(&scene.input()).unwrap_err();
    assert!(
        error.to_string().contains("proto/critters/critters.lst"),
        "{error}"
    );
    // An unusable critter is skipped, not an error.
    let mut scene = self::scene();
    scene.critters = vec![critter(0, "synthetic.esm:00f003", "Ann", at, 0.0)];
    scene.critters[0].hit_points = 0;
    let built = build_map(&scene.input()).unwrap();
    assert!(built.report.critters.is_empty());
    assert!(
        built.report.skipped_critters[0]
            .reason
            .contains("hit points")
    );
    assert_eq!(
        digest(&built.layer),
        digest(&build_map(&self::scene().input()).unwrap().layer)
    );
}

#[test]
fn the_critter_budget_names_the_list() {
    let mut scene = scene();
    let frame = scene.frame();
    let rows: Vec<u8> = (0..4095)
        .flat_map(|i| format!("c{i:05},0\n").into_bytes())
        .collect();
    scene.base.insert("art/critters/critters.lst".into(), rows);
    let at = |s| hex_point(&frame, hex_at(&frame, s, 52.0));
    scene.critters = vec![
        critter(0, "synthetic.esm:00f101", "Ann", at(46.0), 0.0),
        critter(1, "synthetic.esm:00f102", "Bea", at(50.0), 0.0),
    ];
    match build_map(&scene.input()) {
        Err(BuildError::FidBudget {
            category,
            base,
            requested,
            ..
        }) => assert_eq!((category, base, requested), ("critters", 4095, 4097)),
        other => panic!("{other:?}"),
    }
    scene.critters.pop();
    let built = build_map(&scene.input()).unwrap();
    assert_eq!(built.report.critter_sets[0].row, 4095);
}

/// Four critters: two the same character, one another name on the same
/// art, one on the second set.
fn populated() -> Scene {
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

#[test]
fn a_standalone_build_keeps_extending_the_critter_list() {
    let mut scene = populated();
    let shared = build_map(&scene.input()).unwrap();
    scene.options.standalone = true;
    let built = build_map(&scene.input()).unwrap();
    for path in [
        "art/critters/critters.lst",
        "proto/critters/critters.lst",
        "text/english/game/pro_crit.msg",
    ] {
        assert_eq!(
            built.layer.files()[path],
            shared.layer.files()[path],
            "{path}"
        );
    }
    let critters = built.report.art_lists["critters"];
    assert!(!critters.replaced && critters.shipped);
    assert_eq!(
        (critters.base_entries, critters.kept, critters.added),
        (2, 2, 2)
    );
    assert_eq!(critters.free_after, emit::FID_INDEX_CAP - 4);
    assert_eq!(built.report.verify.critters, 4);
    assert_eq!(built.report.verify.critter_rows, 2);
    assert_eq!(built.report.verify.replaced_art_lists, ["walls", "tiles"]);
}

#[test]
fn the_layer_verifies_and_the_map_parses_back_with_critter_objects() {
    let scene = populated();
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    let files = built.layer.files();
    assert_eq!(report.critters.len(), 4);
    assert_eq!(
        (report.critter_ai_packet, report.critter_team),
        (Some(3), Some(DEFAULT_CRITTER_TEAM))
    );
    // Sprite sets: the two used, in art order, after the two base rows.
    let sets: Vec<(usize, &str, u16)> = report
        .critter_sets
        .iter()
        .map(|s| (s.art, s.stem.as_str(), s.row))
        .collect();
    assert_eq!(sets, [(0, "mj0000", 2), (1, "mj0001", 3)]);
    assert_eq!(report.art_entries["critters"], 2);
    assert_eq!(
        report.critter_sets[0].files,
        [
            "art/critters/mj0000aa.frm",
            "art/critters/mj0000ab.frm",
            "art/critters/mj0000ao.frm"
        ]
    );
    assert!(
        !files.keys().any(|p| p.starts_with("art/critters/mj0002")),
        "the unused set"
    );
    let list = std::str::from_utf8(&files["art/critters/critters.lst"]).unwrap();
    assert_eq!(list, "hmbase,0\nhfbase,1\nmj0000,2\nmj0001,3\n");
    // Prototypes: Mae, Townsman (shared by two), Joe, after the base two.
    let protos: Vec<(u32, &str)> = report
        .critter_prototypes
        .iter()
        .map(|p| (p.pid, p.name.as_str()))
        .collect();
    assert_eq!(
        protos,
        [
            (0x0100_0003, "Mae"),
            (0x0100_0004, "Townsman"),
            (0x0100_0005, "Joe")
        ]
    );
    assert_eq!(report.prototypes["critters"], 3);
    assert_eq!(report.critters[1].pid, report.critters[2].pid);
    assert_eq!(
        std::str::from_utf8(&files["proto/critters/critters.lst"]).unwrap(),
        "00000001.pro\n00000002.pro\n00000003.pro\n00000004.pro\n00000005.pro\n"
    );
    // Verification counted every critter piece.
    let verify = &report.verify;
    assert_eq!(verify.critter_frm, 6);
    assert_eq!(verify.critter_rows, 2);
    assert_eq!(verify.critter_prototypes, 3);
    assert_eq!(verify.critters, 4);

    // Read back through the mounted layer as the engine would.
    let vfs = mounted(&built.layer, &scene.base);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let art = ArtCatalog::load(&vfs).unwrap();
    let messages =
        MsgCatalog::parse_strict(&vfs.read("text/english/game/pro_crit.msg").unwrap()).unwrap();
    assert_eq!(messages.get(100).unwrap().text, b"Base critter");
    let map = parse_map(&vfs, &report.map_file);
    assert!(map.diagnostics.is_empty());
    let mut seen = 0;
    for object in &map.objects.elevations[0] {
        let ObjectState::Critter(state) = &object.state else {
            continue;
        };
        seen += 1;
        let placed = report
            .critters
            .iter()
            .find(|c| c.hex == object.hex.unwrap())
            .unwrap();
        let input = &scene.critters[placed.index];
        assert_eq!(object.orientation, placed.orientation);
        assert_eq!(object.pid, placed.pid);
        assert_eq!(
            (state.hp, state.ai, state.team),
            (input.hit_points, 3, DEFAULT_CRITTER_TEAM)
        );
        assert_eq!((object.script_id, object.frame), (-1, 0));
        let fid = Fid(object.art_id);
        assert_eq!((fid.category(), fid.animation()), (1, 0));
        let stand = art.path(fid).unwrap();
        assert_eq!(stand, format!("art/critters/{}aa.frm", placed.stem));
        let walk = art.path(Fid::new(1, fid.base(), 1, 0, 0)).unwrap();
        assert_eq!(walk, format!("art/critters/{}ab.frm", placed.stem));
        for path in [stand, walk] {
            let frm = FrmArchive::parse(&vfs.read(&path).unwrap()).unwrap();
            assert_eq!(frm.stored_sequences(), 6);
        }
        let info = catalog.lookup(object.pid).unwrap();
        assert_eq!(info.kind, PrototypeKind::Critter);
        let prototype = vault_formats::Prototype::parse_for(
            &vfs.read(&format!("proto/critters/{:08}.pro", object.pid & 0xff_ffff))
                .unwrap(),
            GameProfile::Fallout2,
        )
        .unwrap();
        let PrototypeData::Critter(data) = &prototype.data else {
            panic!("a critter prototype")
        };
        assert_eq!(data.base_stats[34], i32::from(input.female));
        assert_eq!(prototype.art_id, object.art_id);
        assert_eq!(
            messages.get(prototype.message_base).unwrap().text,
            input.name.as_bytes()
        );
        assert_eq!(
            messages.get(prototype.message_base + 1).unwrap().text,
            input.description.as_bytes()
        );
    }
    assert_eq!(seen, 4);
    // Critters take the ids after every other object, so the others keep
    // theirs.
    let objects = &map.objects.elevations[0];
    let mut critter_ids: Vec<u32> = objects
        .iter()
        .filter(|o| matches!(o.state, ObjectState::Critter(_)))
        .map(|o| o.id)
        .collect();
    critter_ids.sort_unstable();
    let count = objects.len() as u32;
    assert_eq!(critter_ids, [count - 3, count - 2, count - 1, count]);

    // Written, read back and verified again; a broken critter file is named.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("out");
    built.layer.write(&dir).unwrap();
    let back = read_layer(&dir).unwrap();
    assert_eq!(verify_layer(&back, &scene.base).unwrap(), report.verify);
    let mut broken = built.layer.clone();
    broken
        .insert("art/critters/mj0001ab.frm", vec![0; 5])
        .unwrap();
    match verify_layer(&broken, &scene.base) {
        Err(BuildError::Verify { file, .. }) => assert_eq!(file, "art/critters/mj0001ab.frm"),
        other => panic!("{other:?}"),
    }
    let mut nameless = built.layer.clone();
    nameless
        .insert(
            "text/english/game/pro_crit.msg",
            b"{100}{}{Base critter}\n".to_vec(),
        )
        .unwrap();
    assert!(matches!(
        verify_layer(&nameless, &scene.base),
        Err(BuildError::Verify { .. })
    ));
}

#[test]
fn synthetic_builds_without_a_base_choose_the_fallback_packet() {
    let mut scene = populated();
    scene.base = BTreeMap::new();
    let built = build_map(&scene.input()).unwrap();
    assert_eq!(
        built.report.critter_ai_packet,
        Some(map_builder::critters::FALLBACK_AI_PACKET)
    );
    assert!(built.report.notes.iter().any(|n| n.contains("data/ai.txt")));
    assert_eq!(built.report.critter_sets[0].row, 0);
    assert_eq!(built.report.critter_prototypes[0].pid, 0x0100_0001);
    assert_eq!(built.report.verify.critters, 4);
    // A named packet needs no table.
    scene.options.critter_ai_packet = Some(7);
    scene.options.critter_team = 12;
    let built = build_map(&scene.input()).unwrap();
    let objects = critter_objects(&built, &scene.base);
    assert_eq!(objects.len(), 4);
    let vfs = mounted(&built.layer, &scene.base);
    let map = parse_map(&vfs, &built.report.map_file);
    for object in &map.objects.elevations[0] {
        if let ObjectState::Critter(state) = &object.state {
            assert_eq!((state.ai, state.team), (7, 12));
        }
    }
}

#[test]
fn the_same_input_gives_the_same_bytes() {
    let scene = populated();
    let first = build_map(&scene.input()).unwrap();
    let second = build_map(&scene.input()).unwrap();
    assert_eq!(first.layer.files(), second.layer.files());
    assert_eq!(
        first.layer.manifest_text().unwrap(),
        second.layer.manifest_text().unwrap()
    );
    assert_eq!(first.report, second.report);
}

#[test]
fn a_build_without_critters_has_not_moved() {
    let mut scene = town();
    assert_eq!(
        digest(&build_map(&scene.input()).unwrap().layer),
        BEFORE_EMPTY
    );
    scene.base = base_files(10);
    assert_eq!(
        digest(&build_map(&scene.input()).unwrap().layer),
        BEFORE_BASE
    );
    // The critter base files and unused sprite sets change nothing.
    scene.base = full_base();
    scene.critter_art = vec![sprite_set("unused", 9)];
    let built = build_map(&scene.input()).unwrap();
    assert_eq!(digest(&built.layer), BEFORE_BASE);
    assert!(built.report.critters.is_empty() && built.report.critter_sets.is_empty());
    assert_eq!(built.report.critter_ai_packet, None);
}
