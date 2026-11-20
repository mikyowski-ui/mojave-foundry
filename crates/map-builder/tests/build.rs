//! Whole builds on synthetic scenes: prototypes and objects per role,
//! the sealing border, numbering, the budget, verification, determinism
//! and re-runs.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use common::*;
use map_builder::walk::{HEXES, reachable};
use map_builder::{BuildError, Role, base_file_paths, build_map, read_layer, verify_layer};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::pro::{BasicFlags, ItemSubtype, PrototypeKind, PrototypeLookup, ScenerySubtype};
use vault_formats::{
    ArtCatalog, ByteSource, Fid, GameProfile, LayeredVfs, LocalPrototypeCatalog, MemoryDir, Vfs,
};

/// The layer mounted over the scene's base, as the engine would see it.
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

#[test]
fn prototypes_and_objects_per_role_parse_back() {
    let scene = town();
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    assert_eq!(report.map_file, "maps/synth.map");
    assert_eq!(report.map_number, 66);
    assert_eq!(report.art_duplicates, 1, "the repeated wall shares a FRM");
    // Walls 1, scenery 2 + blocker, items 2; tiles 2 reserved + 2.
    assert_eq!(report.art_entries["walls"], 1);
    assert_eq!(report.art_entries["scenery"], 3);
    assert_eq!(report.art_entries["items"], 2);
    assert_eq!(report.art_entries["tiles"], 4);
    // Prototypes are per (art index, role, blocking): the two walls name
    // two art indices (one shared FRM), so two wall prototypes; scenery,
    // door and blocker; container and item.
    assert_eq!(report.prototypes["walls"], 2);
    assert_eq!(report.prototypes["scenery"], 3);
    assert_eq!(report.prototypes["items"], 2);
    assert!(!report.prototypes.contains_key("misc"), "no exits, no misc");

    let vfs = mounted(&built.layer, &scene.base);
    let map = parse_map(&vfs, "maps/synth.map");
    assert_eq!(map.header.version, 20);
    assert_eq!(map.header.map_number, 66);
    assert_eq!(&map.header.name[..9], b"SYNTH.MAP");
    assert!(map.diagnostics.is_empty());
    assert!(built.walkable[map.header.entry_hex as usize]);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let art = ArtCatalog::load(&vfs).unwrap();
    let objects = &map.objects.elevations[0];
    assert_eq!(objects.len(), report.objects);
    let ids: BTreeSet<u32> = objects.iter().map(|o| o.id).collect();
    assert_eq!(ids.len(), objects.len(), "object ids are unique");
    for object in objects {
        let path = art.path(Fid(object.art_id)).unwrap();
        assert!(built.layer.files().contains_key(&path), "{path}");
        vault_formats::FrmArchive::parse(&vfs.read(&path).unwrap()).unwrap();
    }
    // Each placement is object id = its index + 1.
    let by_id: BTreeMap<u32, _> = objects.iter().map(|o| (o.id, o)).collect();
    let expected = [
        (Role::WallStrip, PrototypeKind::Wall),
        (Role::WallStrip, PrototypeKind::Wall),
        (
            Role::Scenery,
            PrototypeKind::Scenery(ScenerySubtype::Generic),
        ),
        (Role::Container, PrototypeKind::Item(ItemSubtype::Container)),
        (Role::Item, PrototypeKind::Item(ItemSubtype::Misc)),
        (Role::Door, PrototypeKind::Scenery(ScenerySubtype::Door)),
    ];
    for (index, (placement, (role, kind))) in scene.placements.iter().zip(expected).enumerate() {
        assert_eq!(placement.role, role);
        let object = by_id[&(index as u32 + 1)];
        assert_eq!(object.hex, Some(placement.hex));
        if kind != PrototypeKind::Wall {
            assert_eq!(catalog.lookup(object.pid).unwrap().kind, kind);
        } else {
            assert_eq!(object.pid >> 24, 3);
        }
        let blocks = !object.flags.contains(BasicFlags::NONBLOCKING);
        assert_eq!(blocks, placement.blocking && role != Role::Item, "{role:?}");
        match role {
            Role::Door => assert!(matches!(
                object.state,
                ObjectState::Other {
                    detail: ObjectDetail::Door(_),
                    ..
                }
            )),
            Role::Item => assert!(matches!(
                object.state,
                ObjectState::Other {
                    detail: ObjectDetail::Charges(0),
                    ..
                }
            )),
            _ => {}
        }
    }
    // Names in the prototype message files.
    let messages = vault_formats::MsgCatalog::parse_strict(
        &built.layer.files()["text/english/game/pro_scen.msg"],
    )
    .unwrap();
    assert_eq!(messages.get(100).unwrap().text, b"Scenery (rock)");
    // Floor tiles are library tiles 2 and 3 (0 and 1 are reserved), no roof.
    let tiles = map.tiles[0].as_ref().unwrap();
    assert_eq!((tiles[0].floor(), tiles[1].floor()), (2, 3));
    assert!(tiles.iter().all(|t| t.roof() == 1));
    assert!(built.report.verify.maps.len() == 1);
}

#[test]
fn border_blockers_seal_exactly_like_blocking_every_hex() {
    let scene = town();
    let built = build_map(&scene.input()).unwrap();
    let vfs = mounted(&built.layer, &scene.base);
    let map = parse_map(&vfs, "maps/synth.map");
    let entry = map.header.entry_hex;
    let mut stops = vec![false; HEXES];
    let mut doors = vec![false; HEXES];
    let mut blockers = 0;
    for object in &map.objects.elevations[0] {
        let hex = object.hex.unwrap() as usize;
        let door = matches!(
            object.state,
            ObjectState::Other {
                detail: ObjectDetail::Door(_),
                ..
            }
        );
        // The engine stops a walker at scenery and wall art that blocks.
        let blocks = !object.flags.contains(BasicFlags::NONBLOCKING)
            && matches!(Fid(object.art_id).category(), 1..=3);
        if door {
            doors[hex] = true;
        } else if blocks {
            stops[hex] = true;
        }
        if object.flags.contains(BasicFlags::NO_HIGHLIGHT) && blocks {
            blockers += 1;
        }
    }
    assert_eq!(blockers, built.report.blockers);
    assert!(built.report.blockers > 0);
    // The container blocks only through the blocker on its hex.
    let container = scene.placements[3].hex as usize;
    assert!(stops[container]);
    let border = reachable(entry, |hex| !stops[hex as usize]);
    let full = reachable(entry, |hex| {
        built.walkable[hex as usize] || doors[hex as usize]
    });
    assert_eq!(border, full);
    let reached = border.iter().filter(|r| **r).count();
    assert!(reached > 100 && reached < 3000, "{reached}");
    // Far fewer blockers than non-walkable hexes.
    let closed = built.walkable.iter().filter(|w| !**w).count();
    assert!(built.report.blockers * 10 < closed);
}

#[test]
fn numbering_without_and_with_a_base_map_table() {
    let mut scene = town();
    let built = build_map(&scene.input()).unwrap();
    assert_eq!(built.report.first_map_number, 66);
    assert!(!built.report.maps_txt);
    assert!(!built.layer.files().contains_key("data/maps.txt"));
    assert_eq!(built.layer.manifest().maps[0].number, 66);

    scene.base = base_files(10);
    let built = build_map(&scene.input()).unwrap();
    assert_eq!(built.report.first_map_number, 151);
    assert_eq!(built.report.map_number, 151);
    assert!(built.report.maps_txt);
    let text = std::str::from_utf8(&built.layer.files()["data/maps.txt"]).unwrap();
    assert!(
        text.starts_with("; synthetic map table\n[Map 000]"),
        "the base is kept"
    );
    assert!(text.contains("[Map 151]\nmap_name=synth\n"));
    assert!(built.report.verify.maps_txt);

    scene.number = Some(200);
    assert_eq!(build_map(&scene.input()).unwrap().report.map_number, 200);
    scene.number = Some(100);
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::Input(_))
    ));
    scene.number = Some(1000);
    assert!(
        matches!(build_map(&scene.input()), Err(BuildError::Input(_))),
        "maps.txt stops at 999"
    );
    scene.options.write_maps_txt = false;
    let built = build_map(&scene.input()).unwrap();
    assert_eq!(built.report.map_number, 1000);
    assert!(!built.layer.files().contains_key("data/maps.txt"));
}

#[test]
fn a_base_is_extended_not_replaced() {
    let mut scene = town();
    scene.base = base_files(10);
    let built = build_map(&scene.input()).unwrap();
    let files = built.layer.files();
    let scenery = std::str::from_utf8(&files["art/scenery/scenery.lst"]).unwrap();
    assert!(scenery.starts_with("bsc0000.frm\n"));
    assert_eq!(scenery.lines().count(), 13);
    assert!(scenery.ends_with("mfs00000.frm\nmfs00001.frm\nmfs00002.frm\n"));
    // Tiles: the base has five, so nothing is reserved and the library
    // starts at index 5.
    let vfs = mounted(&built.layer, &scene.base);
    let map = parse_map(&vfs, "maps/synth.map");
    assert_eq!(map.tiles[0].as_ref().unwrap()[0].floor(), 5);
    let protos = std::str::from_utf8(&files["proto/scenery/scenery.lst"]).unwrap();
    assert!(protos.starts_with("00000001.pro\n00000002.pro\n00000003.pro\n00000004.pro"));
    let messages =
        vault_formats::MsgCatalog::parse_strict(&files["text/english/game/pro_scen.msg"]).unwrap();
    assert_eq!(messages.get(100).unwrap().text, b"Base name");
    assert_eq!(messages.get(400).unwrap().text, b"Scenery (rock)");

    // A partial base would clobber the install's lists: refused.
    let mut partial = base_files(10);
    partial.remove(base_file_paths()[0]);
    scene.base = partial;
    let error = build_map(&scene.input()).unwrap_err();
    assert!(error.to_string().contains("art/items/items.lst"), "{error}");
}

#[test]
fn the_budget_error_names_the_list_and_its_contributors() {
    let mut scene = town();
    scene.base = base_files(4093);
    // Scenery needs 3 new entries (rock, door, blocker): 4096 fits.
    build_map(&scene.input()).unwrap();
    scene.base = base_files(4094);
    let error = build_map(&scene.input()).unwrap_err();
    match &error {
        BuildError::FidBudget {
            category,
            cap,
            base,
            requested,
            largest,
        } => {
            assert_eq!(
                (*category, *cap, *base, *requested),
                ("scenery", 4096, 4094, 4097)
            );
            let labels: Vec<&str> = largest.iter().map(|(l, _)| l.as_str()).collect();
            assert_eq!(labels, ["door", "invisible blocker", "rock"]);
        }
        other => panic!("{other}"),
    }
    let text = error.to_string();
    assert!(
        text.contains("scenery") && text.contains("4097") && text.contains("rock (1)"),
        "{text}"
    );
}

#[test]
fn bad_inputs_are_errors() {
    let mut scene = town();
    scene.placements[0].role = Role::Scenery; // wall art
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::Input(_))
    ));
    let mut scene = town();
    scene.placements[0].hex = 40_000;
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::Input(_))
    ));
    let mut scene = town();
    scene.placements[0].art = 99;
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::Input(_))
    ));
    let mut scene = town();
    scene.tiles[0].pop();
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::Input(_))
    ));
    let mut scene = town();
    scene.name = "far too long".into();
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::Input(_))
    ));
    let mut scene = town();
    scene.sampler = mir::GroundSampler::default();
    scene.cells.clear();
    assert!(matches!(
        build_map(&scene.input()),
        Err(BuildError::NoWalkableHex)
    ));
}

#[test]
fn verified_written_read_back_identical_and_rerun() {
    let mut scene = town();
    scene.base = base_files(10);
    let first = build_map(&scene.input()).unwrap();
    let second = build_map(&scene.input()).unwrap();
    assert_eq!(
        first.layer.files(),
        second.layer.files(),
        "byte-identical builds"
    );
    assert_eq!(
        first.layer.manifest_text().unwrap(),
        second.layer.manifest_text().unwrap()
    );
    assert_eq!(first.walkable, second.walkable);
    verify_layer(&first.layer, &scene.base).unwrap();

    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("out").join("synthetic");
    first.layer.write(&dir).unwrap();
    let back = read_layer(&dir).unwrap();
    assert_eq!(back.files(), first.layer.files());
    assert_eq!(back.manifest(), first.layer.manifest());
    let report = verify_layer(&back, &scene.base).unwrap();
    assert_eq!(report, first.report.verify);
    assert!(report.frm > 0 && report.prototypes > 0 && report.fids_resolved > 0);

    // A re-run into the same directory replaces it.
    scene.name = "other".into();
    let rerun = build_map(&scene.input()).unwrap();
    rerun.layer.write(&dir).unwrap();
    assert!(!dir.join("maps/synth.map").exists());
    assert!(dir.join("maps/other.map").exists());
    assert_eq!(read_layer(&dir).unwrap().files(), rerun.layer.files());

    // A corrupted file is named by the check.
    let mut broken = first.layer.clone();
    broken
        .insert("proto/scenery/00000004.pro", vec![0; 3])
        .unwrap();
    match verify_layer(&broken, &scene.base) {
        Err(BuildError::Verify { file, .. }) => assert_eq!(file, "proto/scenery/00000004.pro"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_floor_piece_lies_flat_blocks_nothing_and_is_never_cut() {
    let mut scene = town();
    let frame = scene.frame();
    let hex = hex_at(&frame, 50.0, 50.0);
    // Scenery art 1 ("rock"), placed as a walking surface.
    scene
        .placements
        .push(placement(1, hex, Role::FloorPiece, true, "test.esm:00f100"));
    let index = scene.placements.len() as u32;
    let built = build_map(&scene.input()).unwrap();
    assert!(built.walkable[hex as usize], "a floor piece blocks nothing");

    let vfs = mounted(&built.layer, &scene.base);
    let map = parse_map(&vfs, "maps/synth.map");
    let object = map.objects.elevations[0]
        .iter()
        .find(|o| o.id == index)
        .expect("the floor piece is on the map");
    assert_eq!(object.hex, Some(hex));
    assert_eq!(object.pid >> 24, 2, "a scenery prototype");
    assert_eq!(Fid(object.art_id).category(), 2, "scenery art");
    for flag in [
        BasicFlags::FLAT,
        BasicFlags::NONBLOCKING,
        BasicFlags::NO_TRANSLUCENCY,
    ] {
        assert!(object.flags.contains(flag), "{:#x} is set", flag.0);
    }
    // No other translucency bit: it is drawn solid.
    for flag in [
        BasicFlags::RED,
        BasicFlags::WALL_TRANSLUCENCY,
        BasicFlags::GLASS,
        BasicFlags::STEAM,
        BasicFlags::ENERGY,
    ] {
        assert!(!object.flags.contains(flag));
    }
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    assert_eq!(
        catalog.lookup(object.pid).unwrap().kind,
        PrototypeKind::Scenery(ScenerySubtype::Generic)
    );
}
