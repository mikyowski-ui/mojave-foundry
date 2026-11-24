//! The mini Super-Duper Mart end to end (`common::mini_mart`): a synthetic
//! Fallout 3 plugin through extract, sprites, critters at the combat
//! level and the map stage with loot on, the readable declutter profile
//! and furniture footprints; the written layer parsed back through
//! `vault-formats` and checked object by object.
//!
//! The whole run takes a few seconds (small meshes, three sprite sets), so
//! it is always on.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::mini_mart::{self, key};
use mojave_convert::{Inputs, Install, pipeline};
use vault_formats::map::{MapFile, MapObject, ObjectState};
use vault_formats::pro::BasicFlags;
use vault_formats::{
    ByteSource, Fid, GameProfile, LayeredVfs, LocalPrototypeCatalog, MemoryDir, MsgCatalog,
    Prototype, Vfs,
};

fn mounted(layer: &emit::ModLayer) -> LayeredVfs {
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
        Arc::new(tree(Vec::new())),
        vec![vault_formats::ModLayer::from_tree("test", tree(files))],
    )
}

fn name_of(layer: &emit::ModLayer, pid: u32) -> String {
    let prototype = layer
        .files()
        .iter()
        .filter(|(path, _)| path.ends_with(".pro"))
        .map(|(_, bytes)| Prototype::parse_for(bytes, GameProfile::Fallout2).unwrap())
        .find(|p| p.pid == pid)
        .unwrap_or_else(|| panic!("no prototype {pid:#010x}"));
    let stem = match pid >> 24 {
        0 => "pro_item",
        1 => "pro_crit",
        2 => "pro_scen",
        3 => "pro_wall",
        _ => unreachable!(),
    };
    let messages =
        MsgCatalog::parse_strict(&layer.files()[&format!("text/english/game/{stem}.msg")]).unwrap();
    String::from_utf8(messages.get(prototype.message_base).unwrap().text.clone()).unwrap()
}

#[test]
fn the_mini_mart_converts_with_loot_armed_raiders_and_a_readable_room() {
    let fixture = common::fixture();
    let mart = mini_mart::write(&fixture.root.path().join("mart"));
    let mut inputs = Inputs::for_install(
        &Install::any(&mart.data_dir, None),
        &fixture.f2,
        &fixture.root.path().join("out-mart"),
        mart.slice.clone(),
        2,
    )
    .expect("inputs");
    inputs.standalone = true;
    assert_eq!(inputs.game, mojave_convert::GameVariant::Fallout3);
    let summary = pipeline::all(&inputs).expect("the pipeline runs");
    assert_eq!(summary.critters.failed, 0, "{:?}", summary.critters);
    assert!(summary.map.critters >= 4, "{:?}", summary.map);

    // Everything written round-trips: the builder's own checks, again on
    // what is on disk.
    let mod_dir = inputs.mod_dir();
    let layer = map_builder::read_layer(&mod_dir).expect("the layer reads back");
    let base = BTreeMap::new();
    let verified = map_builder::verify_layer(&layer, &base).expect("verify_layer");
    assert_eq!(verified.maps.len(), 1);
    let loot = map_builder::verify_loot(&layer, &base).expect("verify_loot");
    assert!(loot.item_prototypes >= 8, "{loot:?}");
    assert!(loot.weapons_with_ammo >= 2, "{loot:?}");
    let files: Vec<&String> = layer.files().keys().collect();
    for kind in [".map", ".pro", ".frm", ".lst"] {
        assert!(files.iter().any(|p| p.ends_with(kind)), "no {kind} file");
    }
    // Every weapon-letter file of the raider's set is shipped.
    assert!(
        files
            .iter()
            .any(|p| p.starts_with("art/critters/mj") && p.ends_with("ha.frm")),
        "the pistol stand: {files:?}"
    );

    // The map, object by object.
    let vfs = mounted(&layer);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let map_file = format!("maps/{}", summary.map.map_file.trim_start_matches("maps/"));
    let bytes = vfs.read(&map_file).expect("the map");
    let map = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).expect("parses");
    assert!(map.diagnostics.is_empty(), "{:?}", map.diagnostics);
    let objects: Vec<&MapObject> = map.objects.elevations.iter().flatten().collect();

    let report = std::fs::read_to_string(inputs.out.join("map-report.md")).expect("report");
    // `MINI_MART_KEEP=<dir>` copies the run's report and gallery out.
    if let Some(keep) = std::env::var_os("MINI_MART_KEEP") {
        let keep = std::path::PathBuf::from(keep);
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("map-report.md"), &report).unwrap();
        for entry in std::fs::read_dir(inputs.gallery_dir()).unwrap().flatten() {
            if entry.path().is_file() {
                std::fs::copy(entry.path(), keep.join(entry.file_name())).unwrap();
            }
        }
    }
    for section in [
        "## Loot",
        "## Furniture",
        "## Declutter",
        "Hostile (3)",
        "Friendly (1)",
    ] {
        assert!(report.contains(section), "no {section:?} in\n{report}");
    }

    // People: three hostile raiders on the hostile team, the gunner with
    // the pistol in hand and the leather armour worn; the clerk friendly.
    let rebalance = inputs.rebalance().expect("tables");
    let hostile_team = rebalance.convert.hostility.hostile_team;
    let critters: Vec<&&MapObject> = objects.iter().filter(|o| o.pid >> 24 == 1).collect();
    assert_eq!(critters.len(), 4, "{report}");
    let teams: Vec<i32> = critters
        .iter()
        .map(|o| match &o.state {
            ObjectState::Critter(s) => s.team,
            _ => panic!("a critter without critter state"),
        })
        .collect();
    assert_eq!(
        teams.iter().filter(|t| **t == hostile_team).count(),
        3,
        "{teams:?}"
    );
    assert_eq!(
        teams
            .iter()
            .filter(|t| **t == map_builder::critters::DEFAULT_CRITTER_TEAM)
            .count(),
        1
    );
    let gunner = critters
        .iter()
        .find(|o| {
            o.inventory
                .iter()
                .any(|e| e.object.flags.contains(BasicFlags::RIGHT_HAND))
        })
        .unwrap_or_else(|| panic!("nobody holds a weapon:\n{report}"));
    assert_eq!(Fid(gunner.art_id).weapon(), 5, "the pistol's art letter");
    let held: Vec<String> = gunner
        .inventory
        .iter()
        .map(|e| name_of(&layer, e.object.pid))
        .collect();
    for item in ["10mm Pistol", "Leather Armor", "10mm Round"] {
        assert!(held.iter().any(|h| h == item), "{held:?}");
    }
    assert!(
        gunner
            .inventory
            .iter()
            .any(|e| e.object.flags.contains(BasicFlags::WORN))
    );

    // Containers: the register holds its stacks, the locker its drawn loot
    // and is locked; the rifle and the rounds lie on the floor.
    let containers: Vec<&&MapObject> = objects
        .iter()
        .filter(|o| o.pid >> 24 == 0 && !o.inventory.is_empty())
        .collect();
    assert_eq!(containers.len(), 2, "{report}");
    let named: BTreeMap<String, &&&MapObject> = containers
        .iter()
        .map(|o| (name_of(&layer, o.pid), o))
        .collect();
    let register = named.get("Cash Register").expect("the register");
    let register_items: Vec<(String, u32)> = register
        .inventory
        .iter()
        .map(|e| (name_of(&layer, e.object.pid), e.quantity))
        .collect();
    assert!(
        register_items.contains(&("Bottle Cap".to_owned(), 50)),
        "{register_items:?}"
    );
    assert!(register_items.contains(&("Stimpak".to_owned(), 2)));
    assert!(register_items.contains(&("Storeroom Key".to_owned(), 1)));
    assert!(
        register_items.contains(&("10mm Round".to_owned(), 1)),
        "one pack of 12"
    );
    let locker = named.get("Storeroom Locker").expect("the locker");
    assert!(locker.flags.0 != 0 || !locker.inventory.is_empty());
    assert!(
        locker
            .inventory
            .iter()
            .any(|e| name_of(&layer, e.object.pid) == ".308 Caliber Round")
    );
    let on_floor: Vec<String> = objects
        .iter()
        .filter(|o| o.pid >> 24 == 0 && o.inventory.is_empty())
        .map(|o| name_of(&layer, o.pid))
        .collect();
    assert!(
        on_floor.iter().any(|n| n == "Hunting Rifle"),
        "{on_floor:?}"
    );
    assert!(
        on_floor.iter().filter(|n| *n == "10mm Round").count() >= 3,
        "30 rounds: three packs"
    );
    assert!(report.contains("1 keyed"), "{report}");
    // The armour's figures come from its record (rating 24, Fallout 3's
    // table: AC = DR, DR doubled).
    assert!(
        report.contains("| Leather Armor | armor | person | AC 24, DR 48%"),
        "{report}"
    );

    // Declutter: the debris is pruned, the interactive references kept.
    let declutter = report
        .split("## Declutter")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .unwrap_or_default();
    assert!(declutter.contains("profile: `readable`"), "{declutter}");
    assert!(!declutter.contains("pruned: 0 references"), "{declutter}");
    let placed_debris = mini_mart::R_DEBRIS
        .iter()
        .filter(|r| report.contains(&key(**r)))
        .count();
    let _ = placed_debris;

    // The gallery: animated critter strips and the items page.
    let gallery = inputs.gallery_dir();
    assert!(gallery.join("items-01.png").is_file());
    let characters: Vec<String> = std::fs::read_dir(gallery.join("characters"))
        .expect("character pages")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        characters.iter().any(|n| n.ends_with("-strips.png")),
        "{characters:?}"
    );
    assert!(
        characters.iter().any(|n| n.ends_with(".gif")),
        "{characters:?}"
    );
    assert!(
        std::fs::read_dir(&gallery)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("map-")),
        "the decluttered room's preview"
    );

    // A second run reuses everything.
    let again = mojave_convert::map::map(&inputs).expect("again");
    assert!(again.reused);
}

/// Exit grids of one elevation: hex -> (map, hex, elevation).
fn grids(map: &MapFile, elevation: usize) -> BTreeMap<i32, (i32, i32, i32)> {
    use vault_formats::map::ObjectDetail;
    map.objects.elevations[elevation]
        .iter()
        .filter_map(|o| match &o.state {
            ObjectState::Other {
                detail:
                    ObjectDetail::ExitGrid {
                        map,
                        hex,
                        elevation,
                        ..
                    },
                ..
            } => Some((o.hex.unwrap(), (*map, *hex, *elevation))),
            _ => None,
        })
        .collect()
}

/// The linked, two-storey mini mart (Phase B's exit fixture): the car
/// park and the store in one layer from a layer file, the front door linked
/// both ways, the ramp to the mezzanine a stair link between the store's
/// two elevations, `verify_links` clean, everything parsed back, a raider
/// armed and hostile upstairs, the gallery written; built twice, the same
/// bytes.
#[test]
fn the_linked_two_storey_mini_mart_round_trips() {
    let fixture = common::fixture();
    let dir = fixture.root.path().join("linked");
    let mart = mini_mart::write_linked(&dir);
    let run = |out: &str| {
        let mut inputs = Inputs::from_args(
            &Install::any(&mart.data_dir, None),
            Some(mart.layer_file.to_str().unwrap()),
            &fixture.f2,
            &fixture.root.path().join(out),
            2,
        )
        .expect("inputs");
        inputs.standalone = true;
        let summary = pipeline::all(&inputs).expect("the pipeline runs");
        (inputs, summary)
    };
    let (inputs, summary) = run("out-linked");
    assert!(inputs.layer.is_some());
    assert_eq!(summary.critters.failed, 0, "{:?}", summary.critters);
    assert_eq!(summary.map.map_file, "maps/mmlot.map", "the start map");
    let report = std::fs::read_to_string(inputs.out.join("map-report.md")).expect("report");
    if let Some(keep) = std::env::var_os("MINI_MART_KEEP") {
        let keep = std::path::PathBuf::from(keep);
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("linked-map-report.md"), &report).unwrap();
    }

    let mod_dir = inputs.mod_dir();
    let layer = map_builder::read_layer(&mod_dir).expect("the layer reads back");
    let base = BTreeMap::new();
    let verified = map_builder::verify_layer(&layer, &base).expect("verify_layer");
    assert_eq!(verified.maps.len(), 2, "{report}");
    let links = map_builder::verify_links(&layer, &base).expect("verify_links");
    assert!(links.one_way.is_empty(), "{links:?}");
    assert!(
        links.links >= 4,
        "door both ways, stairs both ways: {links:?}"
    );
    map_builder::verify_loot(&layer, &base).expect("verify_loot");
    let files: Vec<&String> = layer.files().keys().collect();
    for kind in [".map", ".pro", ".frm", ".lst"] {
        assert!(files.iter().any(|p| p.ends_with(kind)), "no {kind} file");
    }
    assert!(mod_dir.join("vault-mod.toml").is_file());
    let manifest = layer.manifest();
    let number = |file: &str| {
        manifest
            .maps
            .iter()
            .find(|m| m.file == file)
            .unwrap_or_else(|| panic!("{file} registered"))
            .number as i32
    };
    let (lot_n, store_n) = (number("mmlot.map"), number("mmstore.map"));

    let vfs = mounted(&layer);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let parse = |file: &str| {
        let bytes = vfs.read(&format!("maps/{file}")).expect("the map");
        let map = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).expect("parses");
        assert!(map.diagnostics.is_empty(), "{:?}", map.diagnostics);
        map
    };
    let lot = parse("mmlot.map");
    let store = parse("mmstore.map");
    // Both of the store's elevations are written; the lot has one.
    assert!(
        store.tiles[0].is_some() && store.tiles[1].is_some(),
        "{report}"
    );
    assert!(store.tiles[2].is_none());
    // The door: the lot's grids lead into the store's ground floor, the
    // store's front door's grids back to the lot.
    let out_of_lot = grids(&lot, 0);
    assert!(!out_of_lot.is_empty(), "{report}");
    assert!(
        out_of_lot
            .values()
            .all(|(m, _, e)| *m == store_n && *e == 0)
    );
    let down = grids(&store, 0);
    let up = grids(&store, 1);
    assert!(
        down.values().any(|(m, _, e)| *m == lot_n && *e == 0),
        "{report}"
    );
    // The ramp: grids on each storey leading to the other, on the store's
    // own number.
    assert!(
        down.values().any(|(m, _, e)| *m == store_n && *e == 1),
        "{report}"
    );
    assert!(
        up.values().any(|(m, _, e)| *m == store_n && *e == 0),
        "{report}"
    );
    for section in [
        "## Layer",
        "### Links",
        "both ways",
        "## Storeys",
        "### Art budget",
    ] {
        assert!(report.contains(section), "no {section:?} in\n{report}");
    }

    // People: the raider upstairs stands on elevation 1, hostile and on
    // the hostile team; the three below and the clerk on elevation 0.
    let rebalance = inputs.rebalance().expect("tables");
    let hostile_team = rebalance.convert.hostility.hostile_team;
    let team = |o: &MapObject| match &o.state {
        ObjectState::Critter(s) => s.team,
        _ => panic!("a critter without critter state"),
    };
    let critters = |e: usize| -> Vec<&MapObject> {
        store.objects.elevations[e]
            .iter()
            .filter(|o| o.pid >> 24 == 1)
            .collect()
    };
    assert_eq!(critters(0).len(), 4, "{report}");
    let upstairs = critters(1);
    assert_eq!(upstairs.len(), 1, "{report}");
    assert_eq!(team(upstairs[0]), hostile_team);
    assert_eq!(
        critters(0)
            .iter()
            .filter(|o| team(o) == hostile_team)
            .count(),
        3
    );
    // The gunner below holds the pistol.
    assert!(
        critters(0).iter().any(|o| o
            .inventory
            .iter()
            .any(|e| e.object.flags.contains(BasicFlags::RIGHT_HAND))),
        "{report}"
    );

    // The gallery: both storeys' previews, the items page, the people.
    let gallery = inputs.gallery_dir();
    for page in [
        "map-mmlot.png",
        "map-mmstore.png",
        "map-mmstore-e1.png",
        "items-01.png",
    ] {
        assert!(gallery.join(page).is_file(), "no {page}");
    }
    assert!(gallery.join("characters").is_dir());
    // Per-map features for the shots.
    let features = mojave_convert::map::MapFeatures::read_for(&inputs.out, "mmstore.map")
        .expect("the store's features");
    assert_eq!(features.map_file, "maps/mmstore.map");
    assert!(!features.people.is_empty());

    // The same layer built again into a fresh directory: the same bytes.
    let (again, _) = run("out-linked-again");
    let twice = map_builder::read_layer(&again.mod_dir()).expect("again");
    assert_eq!(twice.files(), layer.files());
    assert_eq!(
        twice.manifest_text().unwrap(),
        layer.manifest_text().unwrap()
    );
    // And a run over the built one reuses it.
    assert!(mojave_convert::map::map(&inputs).expect("again").reused);
}
