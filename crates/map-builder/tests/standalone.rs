//! Standalone mode: the walls and tiles lists start from their reserved
//! base entries only, everything else is extended as before, the layer
//! still mounts and resolves, and art that overflows the shared lists fits.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::*;
use map_builder::{
    ArtListBudget, ArtListReport, BuildError, BuildOptions, art_budget, build_map, read_layer,
    verify_layer,
};
use vault_formats::map::MapFile;
use vault_formats::pro::PrototypeLookup;
use vault_formats::{
    ArtCatalog, ByteSource, Fid, FrmArchive, GameProfile, LayeredVfs, LocalPrototypeCatalog,
    LstKind, LstTable, MemoryDir, MsgCatalog, Prototype, Vfs,
};

const CAP: usize = emit::FID_INDEX_CAP;

fn standalone() -> BuildOptions {
    BuildOptions {
        standalone: true,
        ..BuildOptions::default()
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

fn list(entries: usize, stem: &str) -> Vec<u8> {
    (0..entries)
        .map(|i| format!("{stem}{i:04}.frm\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn a_standalone_town_mounts_over_its_base_and_resolves_everything() {
    let mut scene = town();
    scene.base = base_files(10);
    let shared = build_map(&scene.input()).unwrap();
    scene.options = standalone();
    let built = build_map(&scene.input()).unwrap();
    let files = built.layer.files();

    // The two lists start from their reserved entries only.
    assert_eq!(files["art/walls/walls.lst"], b"mfw00000.frm\n");
    assert_eq!(
        files["art/tiles/tiles.lst"],
        b"bti0000.frm\nbti0001.frm\nmft00000.frm\nmft00001.frm\n"
    );
    // Everything else is what a shared build writes: the same paths, and
    // the same bytes but where a wall FID or a floor index is stored.
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        shared.layer.files().keys().collect::<Vec<_>>()
    );
    for (path, bytes) in shared.layer.files() {
        let differs = ["art/walls/walls.lst", "art/tiles/tiles.lst", "maps/"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
            || (path.starts_with("proto/walls/") && path.ends_with(".pro"));
        if !differs {
            assert_eq!(files.get(path), Some(bytes), "{path}");
        }
    }
    // The wall prototype list is extended, not replaced: base PIDs keep
    // their positions, and the new wall prototype shows the new wall art.
    assert_eq!(
        files["proto/walls/walls.lst"],
        shared.layer.files()["proto/walls/walls.lst"]
    );
    let wall =
        Prototype::parse_for(&files["proto/walls/00000002.pro"], GameProfile::Fallout2).unwrap();
    assert_eq!(Fid(wall.art_id), Fid::new(3, 0, 0, 0, 0));
    let wall_art: Vec<&String> = files
        .keys()
        .filter(|p| p.starts_with("art/walls/"))
        .collect();
    assert_eq!(wall_art, ["art/walls/mfw00000.frm", "art/walls/walls.lst"]);

    // Free entries, before and after.
    let report = &built.report;
    let expect = |replaced, base_entries, kept, added, shipped| ArtListReport {
        replaced,
        base_entries,
        kept,
        added,
        free_before: CAP - kept,
        free_after: CAP - kept - added,
        shipped,
    };
    assert_eq!(report.art_lists["walls"], expect(true, 4, 0, 1, true));
    assert_eq!(report.art_lists["tiles"], expect(true, 5, 2, 2, true));
    assert_eq!(report.art_lists["scenery"], expect(false, 10, 10, 3, true));
    assert_eq!(report.art_lists["items"], expect(false, 3, 3, 2, true));
    assert_eq!(report.art_lists["misc"], expect(false, 40, 40, 0, false));
    assert!(!report.art_lists.contains_key("critters"));
    assert_eq!(report.art_entries["walls"], 1);
    assert_eq!(report.art_entries["tiles"], 2);
    assert_eq!(
        report.verify.replaced_art_lists,
        ["walls".to_owned(), "tiles".to_owned()]
    );
    assert!(shared.report.verify.replaced_art_lists.is_empty());
    assert_eq!(report.verify.prototype_fids, report.verify.prototypes);

    // Parsed back over the base, every object FID and floor tile names a
    // file the layer holds, except the base's reserved tiles 0 and 1.
    let vfs = mounted(&built.layer, &scene.base);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let map = MapFile::parse_for(
        &vfs.read("maps/synth.map").unwrap(),
        &catalog,
        GameProfile::Fallout2,
    )
    .unwrap();
    assert!(map.diagnostics.is_empty());
    let art = ArtCatalog::load(&vfs).unwrap();
    // Every prototype of the layer, by PID.
    let prototypes: BTreeMap<u32, Prototype> = files
        .iter()
        .filter(|(path, _)| path.ends_with(".pro"))
        .map(|(_, bytes)| {
            let prototype = Prototype::parse_for(bytes, GameProfile::Fallout2).unwrap();
            (prototype.pid, prototype)
        })
        .collect();
    let mut walls = 0;
    for object in &map.objects.elevations[0] {
        let fid = Fid(object.art_id);
        let path = art.path(fid).unwrap();
        assert!(files.contains_key(&path), "{path}");
        FrmArchive::parse(&vfs.read(&path).unwrap()).unwrap();
        if fid.category() == 3 {
            assert_eq!(fid.base(), 0, "the first wall is index 0");
            walls += 1;
        }
        // The object's prototype is the layer's and carries the same FID.
        catalog.lookup(object.pid).unwrap();
        assert_eq!(prototypes[&object.pid].art_id, object.art_id);
    }
    assert_eq!(walls, 2);
    let tiles = map.tiles[0].as_ref().unwrap();
    assert_eq!((tiles[0].floor(), tiles[1].floor()), (2, 3));
    for tile in tiles {
        let path = art.path(Fid::new(4, tile.floor(), 0, 0, 0)).unwrap();
        assert!(files.contains_key(&path), "{path}");
        assert_eq!(tile.roof(), 1);
    }
    assert_eq!(
        art.path(Fid::new(4, 1, 0, 0, 0)).unwrap(),
        "art/tiles/bti0001.frm",
        "the blank tile is the install's"
    );
    // The lists parse back through vault-formats.
    for path in ["art/walls/walls.lst", "art/tiles/tiles.lst"] {
        LstTable::parse(&files[path], LstKind::Art).unwrap();
    }
    MsgCatalog::parse_strict(&files["text/english/game/pro_wall.msg"]).unwrap();

    // Written, read back and verified the same.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("standalone");
    built.layer.write(&dir).unwrap();
    let back = read_layer(&dir).unwrap();
    assert_eq!(back.files(), built.layer.files());
    assert_eq!(verify_layer(&back, &scene.base).unwrap(), report.verify);
    // Deterministic.
    assert_eq!(
        build_map(&scene.input()).unwrap().layer.files(),
        built.layer.files()
    );
}

#[test]
fn the_budget_matches_the_build_in_both_modes() {
    let mut scene = town();
    scene.base = base_files(10);
    for mode in [false, true] {
        scene.options.standalone = mode;
        let budget = art_budget(&scene.base, mode).unwrap();
        let built = build_map(&scene.input()).unwrap();
        for (directory, list) in &built.report.art_lists {
            let entry = budget[directory];
            assert_eq!(entry.replaced, list.replaced, "{directory}");
            assert_eq!(entry.base_entries, list.base_entries, "{directory}");
            assert_eq!(entry.kept, list.kept, "{directory}");
            assert_eq!(entry.free, list.free_before, "{directory}");
        }
    }
    let shared = art_budget(&scene.base, false).unwrap();
    let alone = art_budget(&scene.base, true).unwrap();
    let entry = |replaced, base_entries, kept, builder| ArtListBudget {
        replaced,
        base_entries,
        kept,
        free: CAP - kept,
        builder,
    };
    assert_eq!(shared["walls"], entry(false, 4, 4, 0));
    assert_eq!(shared["tiles"], entry(false, 5, 5, 0));
    assert_eq!(alone["walls"], entry(true, 4, 0, 0));
    assert_eq!(alone["tiles"], entry(true, 5, 2, 0));
    for directory in ["items", "scenery", "misc", "critters"] {
        assert_eq!(shared[directory], alone[directory], "{directory}");
    }
    assert_eq!(shared["scenery"], entry(false, 10, 10, 1));
    assert_eq!(shared["scenery"].available(), CAP - 11);
    assert_eq!(shared["misc"].builder, 1);
    assert_eq!(shared["critters"], entry(false, 0, 0, 0));
    assert_eq!(alone["walls"].available(), CAP);
    assert_eq!(alone["tiles"].available(), CAP - 2);

    // Without a base every list starts empty; the tiles list pads its two
    // reserved entries itself.
    let empty = BTreeMap::new();
    for mode in [false, true] {
        let budget = art_budget(&empty, mode).unwrap();
        assert_eq!(budget["tiles"], entry(mode, 0, 0, 2));
        assert_eq!(budget["tiles"].available(), CAP - 2);
        assert_eq!(budget["walls"], entry(mode, 0, 0, 0));
    }
    // A base list that does not load is named.
    let mut broken = scene.base.clone();
    broken.insert("art/walls/walls.lst".into(), list(CAP + 1, "w"));
    match art_budget(&broken, true) {
        Err(BuildError::Base { file, .. }) => assert_eq!(file, "art/walls/walls.lst"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn without_a_base_standalone_writes_the_same_files() {
    let mut scene = town();
    let shared = build_map(&scene.input()).unwrap();
    scene.options = standalone();
    let alone = build_map(&scene.input()).unwrap();
    assert_eq!(shared.layer.files(), alone.layer.files());
    assert!(alone.report.art_lists["walls"].replaced);
    assert!(!shared.report.art_lists["walls"].replaced);
    assert_eq!(alone.report.art_lists["tiles"].kept, 0);
    assert_eq!(alone.report.art_lists["tiles"].added, 4);
    // Nothing to replace: no base list was dropped.
    assert!(alone.report.verify.replaced_art_lists.is_empty());
}

#[test]
fn a_shared_build_reports_its_lists_unreplaced() {
    let mut scene = town();
    scene.base = base_files(10);
    let built = build_map(&scene.input()).unwrap();
    let walls = built.report.art_lists["walls"];
    assert!(!walls.replaced);
    assert_eq!((walls.base_entries, walls.kept, walls.added), (4, 4, 1));
    assert_eq!((walls.free_before, walls.free_after), (CAP - 4, CAP - 5));
    let tiles = built.report.art_lists["tiles"];
    assert_eq!((tiles.kept, tiles.added, tiles.free_after), (5, 2, CAP - 7));
    assert!(
        built.layer.files()["art/walls/walls.lst"]
            .starts_with(&base_files(10)["art/walls/walls.lst"])
    );
}

#[test]
fn art_that_overflows_the_shared_lists_fits_standalone() {
    let mut scene = town();
    scene.base = base_files(10);
    // Walls full; tiles one short of full, so the two library tiles do not
    // fit either.
    scene
        .base
        .insert("art/walls/walls.lst".into(), list(CAP, "bwa"));
    scene
        .base
        .insert("art/tiles/tiles.lst".into(), list(CAP - 1, "bti"));
    match build_map(&scene.input()) {
        Err(BuildError::FidBudget {
            category,
            base,
            requested,
            ..
        }) => assert_eq!((category, base, requested), ("walls", CAP, CAP + 1)),
        other => panic!("{other:?}"),
    }
    scene
        .base
        .insert("art/walls/walls.lst".into(), list(4, "bwa"));
    match build_map(&scene.input()) {
        Err(BuildError::FidBudget {
            category,
            requested,
            ..
        }) => assert_eq!((category, requested), ("tiles", CAP + 1)),
        other => panic!("{other:?}"),
    }
    scene
        .base
        .insert("art/walls/walls.lst".into(), list(CAP, "bwa"));
    scene.options = standalone();
    let built = build_map(&scene.input()).unwrap();
    let report = &built.report;
    assert_eq!(report.art_lists["walls"].base_entries, CAP);
    assert_eq!(report.art_lists["walls"].free_after, CAP - 1);
    assert_eq!(report.art_lists["tiles"].base_entries, CAP - 1);
    assert_eq!(report.art_lists["tiles"].free_after, CAP - 4);
    assert_eq!(art_budget(&scene.base, true).unwrap()["walls"].free, CAP);
    assert_eq!(art_budget(&scene.base, false).unwrap()["walls"].free, 0);
    verify_layer(&built.layer, &scene.base).unwrap();
}

#[test]
fn verification_still_refuses_what_it_should() {
    let mut scene = town();
    scene.base = base_files(10);
    scene.options = standalone();
    let built = build_map(&scene.input()).unwrap();
    let refused = |layer: &emit::ModLayer| match verify_layer(layer, &scene.base) {
        Err(BuildError::Verify { file, message }) => (file, message),
        other => panic!("{other:?}"),
    };

    // A wall FRM the layer lacks: its index is the layer's own now.
    let mut layer = emit::ModLayer::new("synthetic-map", "Synthetic map", "0").unwrap();
    for (path, bytes) in built.layer.files() {
        if path != "art/walls/mfw00000.frm" {
            layer.insert(path, bytes.clone()).unwrap();
        }
    }
    let mut with_maps = layer.clone();
    for map in &built.layer.manifest().maps {
        with_maps.register_map(&map.file, map.number).unwrap();
    }
    let (_, message) = refused(&with_maps);
    assert!(message.contains("art/walls/mfw00000.frm"), "{message}");

    // A list that may not be standalone drops its base entries.
    let mut layer = built.layer.clone();
    layer
        .insert("art/scenery/scenery.lst", b"mfs00000.frm\n".to_vec())
        .unwrap();
    assert_eq!(refused(&layer).0, "art/scenery/scenery.lst");

    // A standalone tiles list without the reserved entries.
    let mut layer = built.layer.clone();
    layer
        .insert(
            "art/tiles/tiles.lst",
            b"mft00000.frm\nmft00001.frm\n".to_vec(),
        )
        .unwrap();
    assert_eq!(refused(&layer).0, "art/tiles/tiles.lst");

    // A floor tile past the list.
    let mut layer = built.layer.clone();
    layer
        .insert(
            "art/tiles/tiles.lst",
            b"bti0000.frm\nbti0001.frm\n".to_vec(),
        )
        .unwrap();
    assert_eq!(refused(&layer).0, "maps/synth.map");
}
