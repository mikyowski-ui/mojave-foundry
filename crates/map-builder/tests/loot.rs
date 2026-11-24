//! Items, loot and fighters (`map_builder::loot`) and furniture footprints
//! (`map_builder::furniture`) on the mini mart (`tests/common/mart.rs`):
//! built through `build_layer`, parsed back through `vault-formats` and
//! checked object by object.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::mart::*;
use common::saloon::form;
use map_builder::{
    BuildError, BuiltLayer, Catalogue, Held, PlacedLoot, SuspectKind, build_layer, build_map,
};
use vault_formats::map::{MapFile, MapObject, ObjectDetail, ObjectState};
use vault_formats::pro::{BasicFlags, ItemData, LockFlags, PrototypeData, SceneryData};
use vault_formats::{
    ByteSource, Fid, GameProfile, LayeredVfs, LocalPrototypeCatalog, MemoryDir, MsgCatalog,
    Prototype, Vfs,
};

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

/// The written map `file` parsed back through the mounted catalogue.
fn parsed(built: &BuiltLayer, base: &BTreeMap<String, Vec<u8>>, file: &str) -> MapFile {
    let vfs = mounted(&built.layer, base);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let bytes = vfs.read(file).unwrap();
    let map = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).unwrap();
    assert!(map.diagnostics.is_empty(), "{:?}", map.diagnostics);
    map
}

/// The layer's prototype of `pid`.
fn prototype(built: &BuiltLayer, pid: u32) -> Prototype {
    let file = built
        .layer
        .files()
        .iter()
        .filter(|(path, _)| path.ends_with(".pro"))
        .map(|(_, bytes)| Prototype::parse_for(bytes, GameProfile::Fallout2).unwrap())
        .find(|p| p.pid == pid);
    file.unwrap_or_else(|| panic!("no prototype {pid:#010x}"))
}

fn messages(built: &BuiltLayer, stem: &str) -> MsgCatalog {
    MsgCatalog::parse_strict(&built.layer.files()[&format!("text/english/game/{stem}.msg")])
        .unwrap()
}

fn name_of(built: &BuiltLayer, pid: u32) -> String {
    let p = prototype(built, pid);
    let stem = match pid >> 24 {
        0 => "pro_item",
        1 => "pro_crit",
        2 => "pro_scen",
        3 => "pro_wall",
        _ => unreachable!(),
    };
    String::from_utf8(
        messages(built, stem)
            .get(p.message_base)
            .unwrap()
            .text
            .clone(),
    )
    .unwrap()
}

fn build(mart: &Mart, catalogue: Option<Catalogue>) -> Result<BuiltLayer, BuildError> {
    build_layer(&layer(mart, catalogue.map(Arc::new)))
}

fn objects(map: &MapFile) -> Vec<&MapObject> {
    map.objects.elevations.iter().flatten().collect()
}

fn pid_of(built: &BuiltLayer, key: &str) -> u32 {
    built
        .loot
        .as_ref()
        .unwrap()
        .items
        .iter()
        .find(|i| i.key == key)
        .unwrap()
        .pid
}

fn hex_of(scene: &common::Scene, reference: u32) -> i32 {
    scene
        .placements
        .iter()
        .find(|p| p.reference == form(reference))
        .unwrap()
        .hex
}

#[test]
fn the_mini_mart_reads_back_object_by_object() {
    let mart = mart();
    let built = build(&mart, Some(catalogue())).unwrap();
    let loot = built.loot.as_ref().unwrap();
    let kinds: Vec<(&str, usize)> = loot
        .item_prototypes
        .iter()
        .map(|(k, n)| (k.as_str(), *n))
        .collect();
    assert_eq!(
        kinds,
        [
            ("ammo", 2),
            ("armor", 1),
            ("drug", 1),
            ("key", 1),
            ("misc", 1),
            ("weapon", 2)
        ]
    );
    // The two maps are linked both ways through the teleport door pair.
    assert!(built.stubs.is_empty(), "{:?}", built.stubs);
    assert_eq!(built.links.links, 2);
    assert!(built.links.one_way.is_empty());

    let map = parsed(&built, &mart.base, "maps/mart.map");
    let all = objects(&map);
    let (pistol, rifle, ammo10, ammo308, leather, key) = (
        pid_of(&built, PISTOL),
        pid_of(&built, RIFLE),
        pid_of(&built, AMMO_10MM),
        pid_of(&built, AMMO_308),
        pid_of(&built, LEATHER),
        pid_of(&built, STORE_KEY),
    );

    // Weapons agree with their ammunition; every item has inventory art.
    let weapon = |pid| match prototype(&built, pid).data {
        PrototypeData::Item(item) => match item.data {
            ItemData::Weapon(w) => (w.ammo_pid, w.caliber, w.capacity, item.inventory_art),
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert_eq!(weapon(pistol).0, ammo10 as i32);
    assert_eq!(weapon(pistol).1, 101);
    assert_eq!(weapon(rifle).0, ammo308 as i32);
    assert_eq!(weapon(rifle).1, 102);
    for item in &loot.items {
        assert_eq!(Fid(item.inventory_fid).category(), 7, "{item:?}");
        assert!(loot.check.no_inventory_art.iter().all(|p| *p != item.pid));
    }
    assert_eq!(name_of(&built, pistol), "10mm pistol");
    assert_eq!(loot.check.weapons_with_ammo, 2);
    assert!(loot.check.maps >= 2);
    assert!(loot.check.inventory_records >= 11);
    // The town's own generic item has none, and is listed.
    assert_eq!(loot.check.no_inventory_art.len(), 1);
    // The armour shows the figure set's stand art.
    let PrototypeData::Item(item) = prototype(&built, leather).data else {
        panic!()
    };
    let ItemData::Armor(armour) = item.data else {
        panic!()
    };
    let figure = Fid(armour.male_art as u32);
    assert_eq!((figure.category(), figure.animation()), (1, 0));
    assert_eq!(armour.female_art, armour.male_art);

    // The rifle and two ammunition packs on the floor.
    let rifle_hex = hex_of(&mart.mart, RIFLE_REF);
    let on_floor: Vec<_> = all.iter().filter(|o| o.pid == rifle).collect();
    assert_eq!(on_floor.len(), 1);
    assert_eq!(on_floor[0].hex, Some(rifle_hex));
    assert!(matches!(
        on_floor[0].state,
        ObjectState::Other { detail: ObjectDetail::Weapon { rounds: 5, ammo_pid }, .. }
            if ammo_pid == ammo308 as i32
    ));
    // The ammunition box was placed on the front door's exit grid, so the
    // builder moved it to the nearest free hex off the link (crate notes,
    // "Link clearance"); the rifle, beside a grid, stays: doors keep only
    // their grid hexes and arrival hexes clear.
    let ammo_input = hex_of(&mart.mart, AMMO_REF);
    let mut link_hexes = std::collections::BTreeSet::new();
    for built_map in &built.maps {
        for link in &built_map.links {
            link_hexes.extend(link.grid_hexes.iter().copied());
            if link.to_file == "maps/mart.map" {
                link_hexes.insert(link.to_hex);
            }
        }
    }
    assert!(link_hexes.contains(&ammo_input));
    assert!(!link_hexes.contains(&rifle_hex));
    let moved: usize = built
        .maps
        .iter()
        .flat_map(|m| &m.elevations)
        .map(|e| e.moved_off_links)
        .sum();
    assert_eq!(moved, 1);
    let packs: Vec<_> = all.iter().filter(|o| o.pid == ammo10).collect();
    assert_eq!(packs.len(), 2);
    for pack in &packs {
        let at = pack.hex.unwrap();
        assert!(!link_hexes.contains(&at));
        assert_eq!(
            vault_formats::HexGrid::lattice_distance(ammo_input, at),
            Ok(1)
        );
        assert!(matches!(
            pack.state,
            ObjectState::Other {
                detail: ObjectDetail::Ammunition(10),
                ..
            }
        ));
    }

    // The register holds the pistol, ammunition, stimpaks, caps and the
    // key; the locker is locked.
    let is_container = |pid: u32| {
        pid >> 24 == 0
            && matches!(
                prototype(&built, pid).data,
                PrototypeData::Item(ref i) if matches!(i.data, ItemData::Container { .. })
            )
    };
    let container_at = |hex: i32| {
        *all.iter()
            .find(|o| o.hex == Some(hex) && is_container(o.pid))
            .unwrap()
    };
    let register = container_at(hex_of(&mart.mart, REGISTER));
    let held: Vec<(u32, u32)> = register
        .inventory
        .iter()
        .map(|e| (e.object.pid, e.quantity))
        .collect();
    assert_eq!(
        held,
        [
            (pistol, 1),
            (ammo10, 2),
            (pid_of(&built, STIMPAK), 3),
            (pid_of(&built, CAPS), 50),
            (key, 1)
        ]
    );
    assert!(register.inventory.iter().all(|e| e.object.hex.is_none()));
    assert!(matches!(register.state, ObjectState::Other { word: 0, .. }));
    assert_eq!(name_of(&built, register.pid), "Cash register");
    let PrototypeData::Item(item) = prototype(&built, register.pid).data else {
        panic!()
    };
    assert!(matches!(
        item.data,
        ItemData::Container { capacity: 60, .. }
    ));
    let locker = container_at(hex_of(&mart.mart, LOCKER));
    assert!(
        matches!(locker.state, ObjectState::Other { word, .. } if word & LockFlags::LOCKED.0 != 0)
    );
    assert_eq!(locker.inventory[0].object.pid, ammo308);
    assert_eq!(name_of(&built, locker.pid), "Storeroom locker");

    // The storeroom door is locked, and its prototype's key code is the
    // key's.
    let door_hex = hex_of(&mart.mart, STORE_DOOR);
    let door = all.iter().find(|o| o.hex == Some(door_hex)).unwrap();
    assert!(matches!(
        door.state,
        ObjectState::Other { detail: ObjectDetail::Door(flags), .. } if flags.0 & LockFlags::LOCKED.0 != 0
    ));
    let PrototypeData::Scenery(scenery) = prototype(&built, door.pid).data else {
        panic!()
    };
    let SceneryData::Door { key: door_key, .. } = scenery.data else {
        panic!()
    };
    let PrototypeData::Item(item) = prototype(&built, key).data else {
        panic!()
    };
    let ItemData::Key { code } = item.data else {
        panic!()
    };
    assert_eq!(door_key, code);
    assert_eq!(code, (key & 0x00ff_ffff) as i32);
    assert_eq!(name_of(&built, door.pid), "Storeroom door");
    // The counter is named; the front door keeps its generated name.
    let counter = all
        .iter()
        .find(|o| o.hex == Some(hex_of(&mart.mart, COUNTER)))
        .unwrap();
    assert_eq!(name_of(&built, counter.pid), "Store counter");

    // Raiders: armed, armoured, hostile; the unarmed one empty-handed; the
    // clerk as before.
    let critters: Vec<&&MapObject> = all.iter().filter(|o| o.pid >> 24 == 1).collect();
    assert_eq!(critters.len(), 4);
    let mart_map = &built.maps[1];
    let critter = |reference: u32| {
        let placed = mart_map
            .report
            .critters
            .iter()
            .find(|c| c.reference == form(reference))
            .unwrap();
        **critters.iter().find(|o| o.hex == Some(placed.hex)).unwrap()
    };
    let state = |o: &MapObject| match &o.state {
        ObjectState::Critter(s) => (s.ai, s.team, s.hp),
        _ => panic!(),
    };
    let first = critter(RAIDERS[0].0);
    assert_eq!(state(first), (RAIDER_PACKET, RAIDER_TEAM, 30));
    assert_eq!(Fid(first.art_id).weapon(), 5);
    let slots: Vec<(u32, bool, bool)> = first
        .inventory
        .iter()
        .map(|e| {
            (
                e.object.pid,
                e.object.flags.contains(BasicFlags::WORN),
                e.object.flags.contains(BasicFlags::RIGHT_HAND),
            )
        })
        .collect();
    assert_eq!(
        slots,
        [
            (leather, true, false),
            (pistol, false, true),
            (ammo10, false, false),
            (pid_of(&built, STIMPAK), false, false)
        ]
    );
    let second = critter(RAIDERS[1].0);
    assert_eq!(state(second), (RAIDER_PACKET, RAIDER_TEAM, 28));
    assert_eq!(Fid(second.art_id).weapon(), 7);
    let third = critter(RAIDERS[2].0);
    assert_eq!(state(third), (RAIDER_PACKET, RAIDER_TEAM, 25));
    assert!(third.inventory.is_empty());
    assert_eq!(Fid(third.art_id).weapon(), 0);
    let clerk = critter(CLERK.0);
    assert_eq!(state(clerk).1, map_builder::critters::DEFAULT_CRITTER_TEAM);
    assert!(clerk.inventory.is_empty());

    // The counts.
    let mart_loot = &loot.maps[1];
    assert_eq!(mart_loot.placed_items, 3);
    assert_eq!(
        (
            mart_loot.containers,
            mart_loot.filled_containers,
            mart_loot.container_stacks
        ),
        (2, 2, 6)
    );
    assert_eq!(
        (
            mart_loot.locked_doors,
            mart_loot.locked_containers,
            mart_loot.keyed_doors
        ),
        (1, 1, 1)
    );
    assert_eq!((mart_loot.fighters, mart_loot.armed_critters), (3, 2));
    assert_eq!(
        loot.maps[0],
        map_builder::MapLoot {
            file: "maps/lot.map".into(),
            ..map_builder::MapLoot::default()
        }
    );
    assert_eq!(loot.critter_prototypes, 3);
    // Counter, pool table, register, locker, storeroom door.
    assert_eq!(loot.named_prototypes, 5);
    assert_eq!(loot.skipped.len(), 1, "{:?}", loot.skipped);
    assert!(loot.skipped[0].entry.contains("00dead"));
    let summary = loot.loot_summary();
    assert!(
        summary.iter().any(|l| l.contains("8 (2 ammo")),
        "{summary:?}"
    );
    assert!(summary.iter().any(|l| l.starts_with("skipped reference")));

    // Determinism.
    let again = build(&mart, Some(catalogue())).unwrap();
    assert_eq!(again.layer.files(), built.layer.files());
    assert_eq!(again.loot, built.loot);
}

#[test]
fn item_pids_follow_sorted_keys() {
    let mart = mart();
    let built = build(&mart, Some(catalogue())).unwrap();
    let items = &built.loot.as_ref().unwrap().items;
    let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    // Consecutive, right after the base list's two prototypes.
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item.pid, 3 + i as u32, "{item:?}");
        assert_eq!(item.file, format!("proto/items/{:08}.pro", 3 + i));
    }
    // A key sorting first shifts every other by one, and nothing else.
    let mut more = catalogue();
    let mut extra = more.items[CAPS].clone();
    extra.text = emit::ProtoText::new("Spare caps", "");
    more.items.insert("synthetic.esm:000001".into(), extra);
    let shifted = build(&mart, Some(more)).unwrap();
    let after = &shifted.loot.as_ref().unwrap().items;
    assert_eq!(after[0].pid, 3);
    for (before, now) in items.iter().zip(&after[1..]) {
        assert_eq!(
            (before.key.as_str(), before.pid + 1),
            (now.key.as_str(), now.pid)
        );
    }
}

#[test]
fn an_unknown_item_key_is_an_input_error_naming_the_reference() {
    let mart = mart();
    let mut bad = catalogue();
    bad.placed.insert(
        form(AMMO_REF),
        PlacedLoot::Item {
            item: "synthetic.esm:0bad01".into(),
            count: 1,
            state: Default::default(),
        },
    );
    let error = build(&mart, Some(bad)).unwrap_err();
    let text = error.to_string();
    assert!(matches!(error, BuildError::Input(_)), "{text}");
    assert!(
        text.contains(&form(AMMO_REF)) && text.contains("0bad01"),
        "{text}"
    );
    // In a critter's kit too.
    let mut bad = catalogue();
    bad.critters
        .get_mut(&form(RAIDERS[2].0))
        .unwrap()
        .carried
        .push(Held::new("synthetic.esm:0bad02", 1));
    let text = build(&mart, Some(bad)).unwrap_err().to_string();
    assert!(
        text.contains(&form(RAIDERS[2].0)) && text.contains("0bad02"),
        "{text}"
    );
}

#[test]
fn inventories_keep_the_formats_limits() {
    let mart = mart();
    // The register's contents (size 4) do not fit below capacity 3.
    let mut small = catalogue();
    if let Some(PlacedLoot::Container { capacity, .. }) = small.placed.get_mut(&form(REGISTER)) {
        *capacity = Some(3);
    }
    let text = build(&mart, Some(small)).unwrap_err().to_string();
    assert!(
        text.starts_with("input:") && text.contains(&form(REGISTER)),
        "{text}"
    );
    // A hand holds one.
    let mut two = catalogue();
    two.critters
        .get_mut(&form(RAIDERS[0].0))
        .unwrap()
        .right_hand
        .as_mut()
        .unwrap()
        .quantity = 2;
    let text = build(&mart, Some(two)).unwrap_err().to_string();
    assert!(text.contains("holds one"), "{text}");
    // Armour goes on the body, not in a hand.
    let mut armour = catalogue();
    armour
        .critters
        .get_mut(&form(RAIDERS[2].0))
        .unwrap()
        .left_hand = Some(Held::new(LEATHER, 1));
    let text = build(&mart, Some(armour)).unwrap_err().to_string();
    assert!(text.contains("left hand"), "{text}");
    // An entry of the wrong kind for its placement.
    let mut wrong = catalogue();
    wrong.placed.insert(
        form(COUNTER),
        PlacedLoot::Item {
            item: RIFLE.into(),
            count: 1,
            state: Default::default(),
        },
    );
    let text = build(&mart, Some(wrong)).unwrap_err().to_string();
    assert!(text.contains("WallStrip"), "{text}");
}

#[test]
fn a_weapon_the_sprite_set_cannot_show_is_carried() {
    let mut mart = mart();
    mart.critter_art[0].frms.remove("ja");
    mart.critter_art[0].frms.remove("jb");
    let built = build(&mart, Some(catalogue())).unwrap();
    let loot = built.loot.as_ref().unwrap();
    assert_eq!(loot.maps[1].armed_critters, 1);
    let skip = loot
        .skipped
        .iter()
        .find(|s| s.entry.contains(&form(RAIDERS[1].0)))
        .unwrap();
    assert!(skip.reason.contains("ja and jb"), "{skip:?}");
    let map = parsed(&built, &mart.base, "maps/mart.map");
    let rifle = pid_of(&built, RIFLE);
    let carrier = objects(&map)
        .into_iter()
        .find(|o| o.inventory.iter().any(|e| e.object.pid == rifle))
        .unwrap();
    assert_eq!(Fid(carrier.art_id).weapon(), 0);
    assert!(
        carrier
            .inventory
            .iter()
            .all(|e| !e.object.flags.contains(BasicFlags::RIGHT_HAND))
    );
}

#[test]
fn without_a_catalogue_nothing_changes() {
    let mart = mart();
    let plain = build(&mart, None).unwrap();
    assert!(plain.loot.is_none());
    // An empty catalogue writes nothing either: the same bytes, the same
    // reports.
    let empty = build(&mart, Some(Catalogue::default())).unwrap();
    assert_eq!(empty.layer.files(), plain.layer.files());
    assert_eq!(
        empty.layer.manifest_text().unwrap(),
        plain.layer.manifest_text().unwrap()
    );
    for (a, b) in empty.maps.iter().zip(&plain.maps) {
        assert_eq!(a.report, b.report);
    }
    let loot = empty.loot.unwrap();
    assert!(loot.items.is_empty() && loot.skipped.is_empty());
    // Without a catalogue items stay generic misc prototypes.
    let map = parsed(&plain, &mart.base, "maps/mart.map");
    assert!(objects(&map).iter().all(|o| o.inventory.is_empty()));
}

#[test]
fn a_single_map_takes_the_catalogue_through_build_options() {
    let mut mart = mart();
    mart.mart.base = mart_base();
    mart.mart.critter_art = critter_art();
    mart.mart.options.catalogue = Some(Arc::new(catalogue()));
    let built = build_map(&mart.mart.input()).unwrap();
    let loot = built.loot.unwrap();
    assert_eq!(loot.items.len(), 8);
    assert_eq!(loot.maps[0].armed_critters, 2);
    // The front door is not converted on an interior demo.
    assert!(!built.report.skipped_teleports.is_empty());
}

#[test]
fn furniture_footprints_block_the_counter_and_the_table() {
    let mut mart = mart();
    let counter: Vec<i32> = mart
        .mart
        .placements
        .iter()
        .filter(|p| p.reference == form(COUNTER))
        .map(|p| p.hex)
        .collect();
    let table = hex_of(&mart.mart, POOL_TABLE);
    // Without footprints: the counter's strips stand on walkable hexes,
    // and the wide table blocks only its anchor; both are reported.
    let plain = build(&mart, None).unwrap();
    let inside = &plain.maps[1];
    assert!(counter.iter().all(|h| inside.walkable[0][*h as usize]));
    let kinds: Vec<(String, SuspectKind)> = inside
        .furniture
        .suspects
        .iter()
        .map(|s| (s.reference.clone(), s.kind))
        .collect();
    assert!(
        kinds.contains(&(form(COUNTER), SuspectKind::OpenStrips)),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(form(POOL_TABLE), SuspectKind::WideSprite)),
        "{kinds:?}"
    );
    assert!(
        inside
            .furniture
            .summary()
            .iter()
            .any(|l| l.contains("walkable furniture?"))
    );
    let around_table: Vec<i32> = map_builder::walk::neighbours(table)
        .filter(|h| inside.walkable[0][*h as usize])
        .collect();
    assert!(!around_table.is_empty());

    // With them: every hex of both bases is blocked, nothing is reported,
    // and the written map stops a walker there.
    mart.mart.options.footprints = footprints();
    let fixed = build(&mart, None).unwrap();
    let inside = &fixed.maps[1];
    let frame = mart.mart.frame();
    for (reference, corners) in footprints() {
        let hexes = map_builder::furniture::footprint_hexes(&frame, &corners);
        assert!(hexes.len() >= 3, "{reference}: {hexes:?}");
        assert!(
            hexes.iter().all(|h| !inside.walkable[0][*h as usize]),
            "{reference}"
        );
    }
    assert!(counter.iter().all(|h| !inside.walkable[0][*h as usize]));
    assert!(
        map_builder::furniture::footprint_hexes(&frame, &footprints()[&form(POOL_TABLE)])
            .contains(&table)
    );
    assert!(
        inside.furniture.suspects.is_empty(),
        "{:?}",
        inside.furniture.suspects
    );
    assert_eq!(
        (
            inside.furniture.footprints_given,
            inside.furniture.footprints_used
        ),
        (2, 2)
    );
    assert!(inside.furniture.blocked_hexes > 0);
    // No walkable hex lets a walker step onto a footprint hex: the sealing
    // border lays a blocker on each one next to the floor.
    let map = parsed(&fixed, &mart.base, "maps/mart.map");
    let blocked = map_builder::islands::engine_blocked(&map.objects.elevations[0]);
    for (_, corners) in footprints() {
        for hex in map_builder::furniture::footprint_hexes(&frame, &corners) {
            let next_to_floor =
                map_builder::walk::neighbours(hex).any(|n| inside.walkable[0][n as usize]);
            if next_to_floor {
                assert!(blocked[hex as usize], "hex {hex}");
            }
        }
    }
    // No furniture hex holds the entrance, a critter, an arrival, a link's
    // destination or an opened passage.
    let furniture: std::collections::BTreeSet<i32> = footprints()
        .values()
        .flat_map(|c| map_builder::furniture::footprint_hexes(&frame, c))
        .collect();
    assert!(!furniture.contains(&inside.report.entry_hex));
    assert!(!inside.report.critters.is_empty());
    for critter in &inside.report.critters {
        assert!(!furniture.contains(&critter.hex), "{}", critter.reference);
    }
    for elevation in &inside.elevations {
        for (what, hex, _) in &elevation.arrivals {
            assert!(!furniture.contains(hex), "{what}");
        }
        for passage in &elevation.passages {
            assert!(passage.opened.iter().all(|h| !furniture.contains(h)));
        }
    }
    let into_mart: Vec<_> = fixed
        .maps
        .iter()
        .flat_map(|m| &m.links)
        .filter(|l| l.to_file == inside.file)
        .collect();
    assert!(!into_mart.is_empty());
    for link in into_mart {
        assert!(!furniture.contains(&link.to_hex), "{}", link.from);
    }
    // The linked interior's summary lists its links instead of claiming
    // none.
    let summary = inside.interior_summary();
    assert!(
        summary.iter().all(|l| !l.contains("no exits")),
        "{summary:?}"
    );
    assert!(summary[0].starts_with("interior map:"), "{summary:?}");
    assert!(
        summary
            .iter()
            .any(|l| l.starts_with("door ") && l.contains("a link leads back")),
        "{summary:?}"
    );
    // The lot has none: its own walkability is unchanged (only its door's
    // arrival hex in the mart may move).
    assert_eq!(fixed.maps[0].furniture.footprints_given, 0);
    assert_eq!(fixed.maps[0].walkable, plain.maps[0].walkable);
}
