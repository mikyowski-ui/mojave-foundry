//! Prototype constructors round trip through `Prototype::parse`, resolve
//! through `LocalPrototypeCatalog` over an in-memory VFS, and serve a map
//! that `MapFile::parse_for` reads back.

use emit::map::{empty_map, plain_object};
use emit::proto::{
    self, DoorSpec, ItemStats, Material, ObjectFlags, ProtoCommon, check_prototype, container,
    door, exit_grid, message_entries, misc_item, scenery, wall,
};
use emit::{FormatWriters, LocalWriters, MsgFile, ProtoList};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::pro::{
    BasicFlags, Category, LockFlags, PrototypeData, PrototypeKind, PrototypeLookup, ScenerySubtype,
};
use vault_formats::vfs::{ByteSource, MemoryDir, MemoryMount};
use vault_formats::{Fid, GameProfile, LocalPrototypeCatalog, MsgCatalog, Prototype};

fn common(category: Category, index: u32, art: u8, flags: ObjectFlags) -> ProtoCommon {
    ProtoCommon {
        pid: proto::pid(category, index).unwrap(),
        fid: Fid::new(art, index as u16, 0, 0, 0),
        message_base: proto::default_message_base(index).unwrap(),
        flags,
        material: Material::Wood,
    }
}

fn all() -> Vec<Prototype> {
    vec![
        wall(common(Category::Wall, 3, 3, ObjectFlags::SOLID)).unwrap(),
        scenery(common(Category::Scenery, 1, 2, ObjectFlags::BARRIER)).unwrap(),
        door(
            common(Category::Scenery, 2, 2, ObjectFlags::SOLID),
            DoorSpec {
                locked: true,
                key: 77,
                sound: b'a',
                ..DoorSpec::default()
            },
        )
        .unwrap(),
        misc_item(
            common(Category::Item, 1, 0, ObjectFlags::DECAL),
            ItemStats {
                cost: 25,
                inventory_art: Some(Fid::new(7, 9, 0, 0, 0)),
                ..ItemStats::default()
            },
        )
        .unwrap(),
        container(
            common(Category::Item, 2, 0, ObjectFlags::SOLID),
            ItemStats::default(),
            100,
            true,
        )
        .unwrap(),
        exit_grid(0, Fid::new(5, 33, 0, 0, 0), -1).unwrap(),
    ]
}

#[test]
fn every_constructor_round_trips_under_both_games() {
    for prototype in all() {
        let bytes = LocalWriters.pro(&prototype).unwrap();
        assert_eq!(bytes, prototype.write());
        assert_eq!(Prototype::parse(&bytes).unwrap(), prototype);
        assert_eq!(
            Prototype::parse_for(&bytes, GameProfile::Fallout2).unwrap(),
            prototype
        );
        check_prototype(&prototype).unwrap();
    }
}

#[test]
fn constructors_set_the_documented_bits() {
    let [wall, scenery, door, item, container, grid] = all().try_into().unwrap();
    assert!(!wall.flags.contains(BasicFlags::NONBLOCKING));
    assert!(!wall.flags.contains(BasicFlags::LIGHT_THROUGH));
    assert!(scenery.flags.contains(BasicFlags::LIGHT_THROUGH));
    assert!(scenery.flags.contains(BasicFlags::PROJECTILE_THROUGH));
    assert!(!scenery.flags.contains(BasicFlags::NONBLOCKING));
    assert!(door.permits_use());
    let PrototypeData::Scenery(data) = &door.data else {
        panic!("a door is scenery")
    };
    assert_eq!(data.stored_byte, b'a');
    assert!(matches!(
        data.data,
        vault_formats::pro::SceneryData::Door { state, key: 77 } if state == LockFlags::LOCKED
    ));
    assert!(item.permits_pickup());
    assert!(item.flags.contains(BasicFlags::FLAT));
    assert!(item.flags.contains(BasicFlags::NONBLOCKING));
    assert!(container.permits_use() && !container.permits_pickup());
    assert_eq!(grid.pid, 0x0500_0010);
    assert_eq!(grid.script, None);
    assert_eq!(wall.script, Some(-1));
    assert_eq!(wall.message_base, 300);
}

#[test]
fn constructors_refuse_mismatched_inputs() {
    let mut bad = common(Category::Wall, 1, 3, ObjectFlags::SOLID);
    bad.pid = proto::pid(Category::Scenery, 1).unwrap();
    assert!(wall(bad).is_err());
    assert!(scenery(common(Category::Scenery, 1, 3, ObjectFlags::SOLID)).is_err());
    assert!(exit_grid(8, Fid::new(5, 33, 0, 0, 0), -1).is_err());
    assert!(proto::pid(Category::Item, 0).is_err());
    assert!(proto::pid(Category::Item, 0x0100_0000).is_err());
    let stats = ItemStats {
        inventory_art: Some(Fid::new(0, 1, 0, 0, 0)),
        ..ItemStats::default()
    };
    assert!(misc_item(common(Category::Item, 1, 0, ObjectFlags::SOLID), stats).is_err());
    let mut prototype = all().remove(0);
    prototype.light = None;
    assert!(LocalWriters.pro(&prototype).is_err());
    let mut prototype = all().remove(0);
    prototype.category = Category::Misc;
    assert!(LocalWriters.pro(&prototype).is_err());
}

/// A synthetic base scenery list plus our appended prototypes, mounted
/// as a Fallout 2 loose corpus and resolved by the engine's catalog.
fn mounted() -> (vault_formats::vfs::MemoryProvider, Vec<Prototype>) {
    let base = b"00000001.pro\n00000002.pro\n";
    let mut scenery_list = ProtoList::from_base(Category::Scenery, base).unwrap();
    let (door_pid, door_file) = scenery_list.push_next().unwrap();
    let (rock_pid, rock_file) = scenery_list.push_next().unwrap();
    assert_eq!(
        (door_pid, door_file.as_str()),
        (0x0200_0003, "00000003.pro")
    );
    assert_eq!(rock_pid, 0x0200_0004);
    assert_eq!(scenery_list.push("00000001.PRO").unwrap(), 0x0200_0001);
    let mut wall_list = ProtoList::new(Category::Wall);
    let (wall_pid, wall_file) = wall_list.push_next().unwrap();
    let mut misc_list = ProtoList::new(Category::Misc);
    for index in 1..=16 {
        misc_list.push(&proto::prototype_file_name(index)).unwrap();
    }
    let (grid_pid, grid_file) = misc_list.push_next().unwrap();
    assert_eq!(grid_pid, 0x0500_0011);

    let with_pid = |art, pid: u32, flags| ProtoCommon {
        pid,
        fid: Fid::new(art, 5, 0, 0, 0),
        message_base: proto::default_message_base(pid & 0xff_ffff).unwrap(),
        flags,
        material: Material::Stone,
    };
    let door = door(
        with_pid(2, door_pid, ObjectFlags::SOLID),
        DoorSpec::default(),
    )
    .unwrap();
    let rock = scenery(with_pid(2, rock_pid, ObjectFlags::SOLID)).unwrap();
    let wall = wall(with_pid(3, wall_pid, ObjectFlags::SOLID)).unwrap();
    let grid = exit_grid(1, Fid::new(5, 34, 0, 0, 0), -1).unwrap();
    assert_eq!(grid.pid, grid_pid);

    let mut files: Vec<(String, Vec<u8>)> = vec![
        (scenery_list.list_path(), scenery_list.to_bytes()),
        (wall_list.list_path(), wall_list.to_bytes()),
        (misc_list.list_path(), misc_list.to_bytes()),
    ];
    for (category, file, prototype) in [
        (Category::Scenery, &door_file, &door),
        (Category::Scenery, &rock_file, &rock),
        (Category::Wall, &wall_file, &wall),
        (Category::Misc, &grid_file, &grid),
    ] {
        files.push((
            proto::prototype_path(category, file),
            LocalWriters.pro(prototype).unwrap(),
        ));
    }
    let tree = MemoryDir::new(
        files
            .into_iter()
            .map(|(path, bytes)| (path, ByteSource::new(bytes).unwrap())),
    )
    .unwrap();
    let provider = MemoryMount {
        profile: GameProfile::Fallout2,
        loose: Some(tree),
        ..MemoryMount::default()
    }
    .mount()
    .unwrap();
    (provider, vec![door, rock, wall, grid])
}

#[test]
fn the_catalog_resolves_appended_prototypes() {
    let (provider, prototypes) = mounted();
    let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);
    let kinds = [
        PrototypeKind::Scenery(ScenerySubtype::Door),
        PrototypeKind::Scenery(ScenerySubtype::Generic),
        PrototypeKind::Wall,
        PrototypeKind::Misc,
    ];
    for (prototype, kind) in prototypes.iter().zip(kinds) {
        assert_eq!(catalog.lookup(prototype.pid).unwrap().kind, kind);
    }
    // The base entries name files the corpus lacks.
    assert!(catalog.lookup(0x0200_0001).is_err());
}

#[test]
fn a_map_of_the_prototypes_reads_back_through_the_catalog() {
    let (provider, prototypes) = mounted();
    let [door, rock, wall, grid] = prototypes.try_into().unwrap();
    let mut map = empty_map("TESTMAP.MAP", 200, 20100).unwrap();
    let mut door_object = plain_object(1, door.pid, Fid(door.art_id), 20101, 0, door.flags);
    door_object.state = ObjectState::Other {
        word: 0,
        detail: ObjectDetail::Door(LockFlags(0)),
    };
    let mut grid_object = plain_object(4, grid.pid, Fid(grid.art_id), 20300, 0, grid.flags);
    grid_object.state = ObjectState::Other {
        word: 0,
        detail: ObjectDetail::ExitGrid {
            map: 201,
            hex: 12345,
            elevation: 0,
            orientation: 2,
        },
    };
    map.objects.elevations[0] = vec![
        door_object,
        plain_object(2, rock.pid, Fid(rock.art_id), 20102, 0, rock.flags),
        plain_object(3, wall.pid, Fid(wall.art_id), 20103, 0, wall.flags),
        grid_object,
    ];
    map.objects.total = 4;
    let bytes = LocalWriters.map(&map).unwrap();
    let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);
    let parsed = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).unwrap();
    assert_eq!(parsed.header.version, 20);
    assert_eq!(parsed.header.map_number, 200);
    assert_eq!(parsed.header.entry_hex, 20100);
    assert_eq!(&parsed.header.name[..11], b"TESTMAP.MAP");
    let objects = &parsed.objects.elevations[0];
    assert_eq!(objects.len(), 4);
    for (got, expected) in objects.iter().zip(&map.objects.elevations[0]) {
        assert_eq!(got.pid, expected.pid);
        assert_eq!(got.hex, expected.hex);
        assert_eq!(got.art_id, expected.art_id);
        assert_eq!(got.flags, expected.flags);
        assert_eq!(got.state, expected.state);
    }
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    // Writing the parsed map again gives the same bytes.
    assert_eq!(LocalWriters.map(&parsed).unwrap(), bytes);
    // A map whose prototype the catalog lacks does not load.
    let lookup = |pid: u32| catalog.lookup(pid);
    let mut missing = map.clone();
    missing.objects.elevations[0][1].pid = 0x0200_0009;
    let bytes = LocalWriters.map(&missing).unwrap();
    assert!(MapFile::parse_for(&bytes, &lookup, GameProfile::Fallout2).is_err());
}

#[test]
fn names_and_descriptions_go_to_the_category_message_file() {
    assert_eq!(
        proto::message_file_path(Category::Scenery, "English"),
        "text/english/game/pro_scen.msg"
    );
    assert_eq!(
        proto::prototype_list_path(Category::Item),
        "proto/items/items.lst"
    );
    let base = b"{100}{}{Base name}\n{101}{}{Base description}\n";
    let mut file = MsgFile::from_base(base).unwrap();
    for entry in message_entries(300, b"Rock", b"A large rock.").unwrap() {
        file.push(&entry).unwrap();
    }
    assert!(
        message_entries(100, b"x", b"y")
            .unwrap()
            .iter()
            .all(|entry| file.push(entry).is_err())
    );
    let catalog = MsgCatalog::parse_strict(&file.to_bytes()).unwrap();
    assert_eq!(catalog.get(300).unwrap().text, b"Rock");
    assert_eq!(catalog.get(301).unwrap().text, b"A large rock.");
    assert_eq!(catalog.get(100).unwrap().text, b"Base name");
    assert!(message_entries(i32::MAX, b"", b"").is_err());
    assert!(message_entries(-1, b"", b"").is_err());
}
