//! Hidden objects, hidden critters and the exact reference-to-object map
//! (`BuildOptions::hidden`, `CritterPlacement::hidden`,
//! `BuiltMap::objects`) on the mini mart (`tests/common/mart.rs`): built
//! through `build_layer`, parsed back through `vault-formats` and checked
//! object by object.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use common::mart::*;
use common::saloon::form;
use common::{critter, placement};
use map_builder::{BuiltLayer, Role, build_layer};
use vault_formats::map::{MapFile, MapObject, ObjectDetail, ObjectState};
use vault_formats::pro::{BasicFlags, LockFlags};
use vault_formats::{ByteSource, GameProfile, LayeredVfs, LocalPrototypeCatalog, MemoryDir, Vfs};

/// The enable group's children and the hidden item and container.
const HIDDEN_WALL: u32 = 0x8701;
const NORMAL_CHILD: u32 = 0x8702;
const HIDDEN_ITEM: u32 = 0x8703;
const HIDDEN_BOX: u32 = 0x8704;
const HIDDEN_PERSON: u32 = 0x8205;
const COPY: &str = "synthetic.esm:008204@lot";

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

fn parsed(built: &BuiltLayer, base: &BTreeMap<String, Vec<u8>>, file: &str) -> MapFile {
    let vfs = mounted(&built.layer, base);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let bytes = vfs.read(file).unwrap();
    let map = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).unwrap();
    assert!(map.diagnostics.is_empty(), "{:?}", map.diagnostics);
    map
}

fn objects(map: &MapFile) -> Vec<&MapObject> {
    map.objects.elevations.iter().flatten().collect()
}

fn by_id(map: &MapFile, id: u32) -> &MapObject {
    objects(map)
        .into_iter()
        .find(|o| o.id == id)
        .unwrap_or_else(|| panic!("no object {id}"))
}

/// The mart with an enable group's worth of extras on the store: a hidden
/// shelf (a child that starts disabled), a visible one (a child that
/// starts enabled), a hidden rifle on the floor, a hidden box, a hidden
/// person, and on the lot the hidden copy of the clerk.
fn with_group(hidden: bool) -> Mart {
    let mut mart = mart();
    let store = &mut mart.mart;
    let frame = store.frame();
    let at = |x: f32, y: f32| frame.hex_of_source(f64::from(x), f64::from(y)).unwrap();
    store.placements.push(placement(
        4,
        at(-150.0, 40.0),
        Role::Scenery,
        true,
        &form(HIDDEN_WALL),
    ));
    store.placements.push(placement(
        4,
        at(-150.0, -40.0),
        Role::Scenery,
        true,
        &form(NORMAL_CHILD),
    ));
    store.placements.push(placement(
        5,
        at(0.0, -100.0),
        Role::Item,
        false,
        &form(HIDDEN_ITEM),
    ));
    store.placements.push(placement(
        7,
        at(150.0, 40.0),
        Role::Container,
        true,
        &form(HIDDEN_BOX),
    ));
    store.placements.sort_by_key(|p| p.hex);
    let mut person = critter(
        1,
        &form(HIDDEN_PERSON),
        "Hidden Clerk",
        [100.0, -200.0],
        0.0,
    );
    person.hidden = true;
    store.critters.push(person);
    if hidden {
        store.options.hidden = [HIDDEN_WALL, HIDDEN_ITEM, HIDDEN_BOX]
            .into_iter()
            .map(form)
            .collect();
    }
    // The clerk's copy on the lot, asked for at the lot's door.
    let lot = &mut mart.lot;
    let frame = lot.frame();
    let door = lot
        .placements
        .iter()
        .find(|p| p.reference == form(LOT_DOOR))
        .unwrap()
        .hex;
    let [x, y] = common::hex_point(&frame, door);
    let mut copy = critter(1, COPY, "Clerk", [x, y], 0.0);
    copy.hidden = true;
    copy.presence_of = Some(form(CLERK.0));
    lot.critters.push(copy);
    mart
}

fn build(mart: &Mart) -> BuiltLayer {
    build_layer(&layer(mart, Some(Arc::new(catalogue())))).unwrap()
}

#[test]
fn hidden_children_are_written_hidden_and_take_no_part_in_the_plan() {
    let mart = with_group(true);
    let built = build(&mart);
    let store = &built.maps[1];
    let map = parsed(&built, &mart.base, "maps/mart.map");

    // The hidden shelf, rifle and box carry the hidden flag; the other
    // child does not.
    let object_of = |reference: u32| -> Vec<&MapObject> {
        store
            .objects
            .iter()
            .filter(|o| o.reference == form(reference))
            .map(|o| by_id(&map, o.id))
            .collect()
    };
    for reference in [HIDDEN_WALL, HIDDEN_ITEM, HIDDEN_BOX] {
        let found = object_of(reference);
        assert_eq!(found.len(), 1, "{reference:#x}");
        assert!(
            found[0].flags.contains(BasicFlags::HIDDEN),
            "{reference:#x}"
        );
    }
    let normal = object_of(NORMAL_CHILD);
    assert_eq!(normal.len(), 1);
    assert!(!normal[0].flags.contains(BasicFlags::HIDDEN));

    // A hidden shelf keeps its blocking flags (a script shows it later)
    // but stops no walker; the visible one does.
    let shelf = object_of(HIDDEN_WALL)[0];
    assert!(!shelf.flags.contains(BasicFlags::NONBLOCKING));
    assert!(!map_builder::islands::stops_movement(
        shelf.flags,
        shelf.art_id
    ));
    assert!(map_builder::islands::stops_movement(
        normal[0].flags,
        normal[0].art_id
    ));
    let blocked = map_builder::islands::engine_blocked(&map.objects.elevations[0]);
    assert!(!blocked[shelf.hex.unwrap() as usize]);
    assert!(blocked[normal[0].hex.unwrap() as usize]);

    // The same map built without the three hidden placements has the same
    // walkable grid, the same entrance and the same people and perches.
    let mut control = with_group(true);
    control.mart.placements.retain(|p| {
        ![HIDDEN_WALL, HIDDEN_ITEM, HIDDEN_BOX]
            .map(form)
            .contains(&p.reference)
    });
    control.mart.options.hidden.clear();
    let plain = build(&control);
    assert_eq!(store.walkable, plain.maps[1].walkable);
    assert_eq!(store.report.entry_hex, plain.maps[1].report.entry_hex);
    assert_eq!(store.report.critters, plain.maps[1].report.critters);
    assert_eq!(store.report.perches, plain.maps[1].report.perches);
    assert_eq!(
        store.report.hidden_objects.len(),
        3,
        "{:?}",
        store.report.hidden_objects
    );
    assert_eq!(store.report.objects, plain.maps[1].report.objects + 3);
}

#[test]
fn hidden_critters_are_hidden_copies_that_count_for_nothing() {
    let mart = with_group(true);
    let built = build(&mart);
    let store = &built.maps[1];
    let lot = &built.maps[0];

    // Not placed people: left out of the report's critters, listed apart.
    assert!(
        store
            .report
            .critters
            .iter()
            .all(|c| c.reference != form(HIDDEN_PERSON))
    );
    assert!(lot.report.critters.is_empty(), "{:?}", lot.report.critters);
    let person = store
        .report
        .hidden_critters
        .iter()
        .find(|c| c.placed.reference == form(HIDDEN_PERSON))
        .expect("the hidden person is placed");
    assert_eq!(person.presence_of, None);
    let copy = lot
        .report
        .hidden_critters
        .iter()
        .find(|c| c.placed.reference == COPY)
        .expect("the copy is placed");
    assert_eq!(copy.presence_of.as_deref(), Some(form(CLERK.0).as_str()));

    // Written hidden, the copy on a walkable hex that no link keeps clear.
    let lot_map = parsed(&built, &mart.base, "maps/lot.map");
    let written = objects(&lot_map)
        .into_iter()
        .find(|o| o.hex == Some(copy.placed.hex) && o.pid >> 24 == 1)
        .expect("the copy's object");
    assert!(written.flags.contains(BasicFlags::HIDDEN));
    assert!(lot.walkable[0][copy.placed.hex as usize]);
    let mut clear: BTreeSet<i32> = BTreeSet::new();
    for link in &lot.links {
        clear.extend(&link.grid_hexes);
        clear.insert(link.to_hex);
    }
    for other in &built.maps {
        for link in &other.links {
            if link.to_file == lot.file {
                clear.insert(link.to_hex);
            }
        }
    }
    assert!(!clear.is_empty());
    assert!(!clear.contains(&copy.placed.hex), "{clear:?}");
    // The copy was asked for at the door's own hex, which is clear.
    assert_ne!(
        copy.placed.hex,
        mart.lot
            .placements
            .iter()
            .find(|p| p.reference == form(LOT_DOOR))
            .unwrap()
            .hex
    );

    // A hidden critter takes no hex a visible one stands on.
    let visible: BTreeSet<i32> = store.report.critters.iter().map(|c| c.hex).collect();
    assert!(!visible.contains(&person.placed.hex));
}

#[test]
fn the_object_map_is_exact() {
    let mart = with_group(true);
    let built = build(&mart);
    for (m, file) in [(0, "maps/lot.map"), (1, "maps/mart.map")] {
        let map = parsed(&built, &mart.base, file);
        let refs = &built.maps[m].objects;
        assert!(!refs.is_empty());
        let all = objects(&map);
        let mut seen = BTreeSet::new();
        for entry in refs {
            let object = all
                .iter()
                .find(|o| o.id == entry.id)
                .unwrap_or_else(|| panic!("no object {} for {}", entry.id, entry.reference));
            assert!(seen.insert(entry.id), "object {} listed twice", entry.id);
            assert_eq!(object.hex, Some(entry.hex), "{}", entry.reference);
            assert_eq!(object.pid, entry.pid, "{}", entry.reference);
            assert_eq!(
                usize::from(object.elevation),
                usize::from(entry.elevation),
                "{}",
                entry.reference
            );
            assert_eq!(
                object.flags.contains(BasicFlags::HIDDEN),
                entry.hidden,
                "{}",
                entry.reference
            );
        }
        // Everything but the exit grids and the border blockers is a
        // placement or a critter, and is listed.
        let others = all
            .iter()
            .filter(|o| {
                let exit = matches!(
                    o.state,
                    ObjectState::Other {
                        detail: ObjectDetail::ExitGrid { .. },
                        ..
                    }
                );
                !exit && !seen.contains(&o.id)
            })
            .count();
        assert_eq!(
            others, built.maps[m].report.blockers,
            "{file}: only the blockers are unlisted"
        );
    }
    // The moved items of a link, the packs of ammunition and the doors are
    // in it too: the references of the store's placements all appear.
    let store: BTreeSet<&str> = built.maps[1]
        .objects
        .iter()
        .map(|o| o.reference.as_str())
        .collect();
    for placement in &mart.mart.placements {
        assert!(
            store.contains(placement.reference.as_str()),
            "{}",
            placement.reference
        );
    }
}

#[test]
fn locks_are_written_on_doors_and_containers() {
    let mart = with_group(true);
    let built = build(&mart);
    let map = parsed(&built, &mart.base, "maps/mart.map");
    let id_of = |reference: u32| {
        built.maps[1]
            .objects
            .iter()
            .find(|o| o.reference == form(reference))
            .unwrap()
            .id
    };
    let ObjectState::Other { detail, .. } = &by_id(&map, id_of(STORE_DOOR)).state else {
        panic!("the door is not a door record");
    };
    let ObjectDetail::Door(state) = detail else {
        panic!("{detail:?}");
    };
    assert_ne!(state.0 & LockFlags::LOCKED.0, 0, "the storeroom door");
    let ObjectState::Other { word, .. } = &by_id(&map, id_of(LOCKER)).state else {
        panic!("the locker is not an item");
    };
    assert_ne!(word & LockFlags::LOCKED.0, 0, "the locker");
    let ObjectState::Other { word, .. } = &by_id(&map, id_of(REGISTER)).state else {
        panic!("the register is not an item");
    };
    assert_eq!(word & LockFlags::LOCKED.0, 0, "the register is open");
}

#[test]
fn two_builds_are_byte_identical() {
    let mart = with_group(true);
    let a = build(&mart);
    let b = build(&mart);
    assert_eq!(a.layer.files(), b.layer.files());
    for m in 0..2 {
        assert_eq!(a.maps[m].objects, b.maps[m].objects);
        assert_eq!(
            a.maps[m].report.hidden_critters,
            b.maps[m].report.hidden_critters
        );
    }
}

#[test]
fn without_hidden_nothing_changes() {
    // A layer with no hidden option and no hidden critter is what it was.
    let plain = mart();
    let a = build_layer(&layer(&plain, Some(Arc::new(catalogue())))).unwrap();
    let b = build_layer(&layer(&plain, Some(Arc::new(catalogue())))).unwrap();
    assert_eq!(a.layer.files(), b.layer.files());
    assert!(a.maps.iter().all(|m| m.report.hidden_critters.is_empty()));
    assert!(a.maps.iter().all(|m| m.report.hidden_objects.is_empty()));
}
