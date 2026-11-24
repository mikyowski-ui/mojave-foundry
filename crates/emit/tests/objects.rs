//! Map objects with state: a locked container with nested contents, a
//! loaded weapon and a charged item on the ground, locked and open doors,
//! stairs, a ladder and an elevator, and an armed, armoured critter. The
//! map reads back through `MapFile::parse_for` (with a `ProtoSet` and with
//! the engine's catalog over a mounted layer) equal to what was written,
//! and writes back the same bytes. All values are synthetic.

use emit::animation::{AttackMode, WeaponCode};
use emit::fighter::{FighterSpec, fighter_prototype};
use emit::items::{
    AmmoSpec, ArmorSpec, DamageType, DrugSpec, KeySpec, MiscSpec, WeaponSpec, ammo, armor, drug,
    drug_stat, key, misc_item_with, weapon,
};
use emit::map::{MapEntry, blank_tiles, empty_map, map_with_elevations};
use emit::objects::{
    ItemState, Loadout, LockState, ProtoSet, Stack, container_object, critter_with_loadout,
    door_object, fill_inventory, item_object, transit_object,
};
use emit::proto::{
    self, DoorSpec, ItemStats, Material, ObjectFlags, ProtoCommon, container, door, stat,
};
use emit::transit::{Destination, LadderKind, elevator, ladder, stairs};
use emit::{CritterPlacement, CritterSpec, FormatWriters, LocalWriters, ProtoList};
use vault_formats::map::{MapFile, MapObject, ObjectDetail, ObjectState, TilePair};
use vault_formats::pro::{BasicFlags, Category, LockFlags, PackedLocation, PrototypeData};
use vault_formats::vfs::{ByteSource, MemoryDir, MemoryMount};
use vault_formats::{Fid, GameProfile, LocalPrototypeCatalog, Prototype};

const PISTOL: u32 = 1;
const ROUNDS: u32 = 2;
const STIM: u32 = 3;
const KEY: u32 = 4;
const LAMP: u32 = 5;
const LEATHER: u32 = 6;
const CHEST: u32 = 7;
const BAG: u32 = 8;
const DOOR: u32 = 0x0200_0001;
const STAIRS: u32 = 0x0200_0002;
const LADDER: u32 = 0x0200_0003;
const ELEVATOR: u32 = 0x0200_0004;
const RAIDER: u32 = 0x0100_0001;
const MAP_NUMBER: i32 = 200;

fn common(pid: u32, flags: ObjectFlags) -> ProtoCommon {
    let category = Category::from_pid(pid).unwrap();
    let art = match category {
        Category::Item => 0,
        Category::Critter => 1,
        _ => 2,
    };
    ProtoCommon {
        pid,
        fid: Fid::new(art, (pid & 0xfff) as u16, 0, 0, 0),
        message_base: proto::default_message_base(pid & 0xff_ffff).unwrap(),
        flags,
        material: Material::Metal,
    }
}

fn stats(size: i32) -> ItemStats {
    ItemStats {
        size,
        weight: 2,
        cost: 10,
        inventory_art: Some(Fid::new(7, 3, 0, 0, 0)),
        sound: 0,
    }
}

fn prototypes() -> Vec<Prototype> {
    let item = ObjectFlags::DECAL;
    let pistol = WeaponSpec {
        stats: stats(2),
        caliber: 101,
        ammo: Some(ROUNDS),
        capacity: 12,
        ..WeaponSpec::new(
            WeaponCode::Pistol,
            [4, 10],
            DamageType::Normal,
            AttackMode::Single,
            5,
            20,
        )
    };
    let rounds = AmmoSpec {
        stats: stats(1),
        caliber: 101,
        quantity: 20,
        ac_modifier: 0,
        dr_modifier: 0,
        multiplier: 1,
        divisor: 1,
    };
    let leather = ArmorSpec {
        stats: stats(5),
        armor_class: 10,
        resistances: [20; 7],
        thresholds: [2; 7],
        perk: None,
        male_art: Some(Fid::new(1, 1, 0, 0, 0)),
        female_art: None,
    };
    let mut raider = FighterSpec::new(CritterSpec::human(40, 1, 7));
    raider.resistances = leather.resistances;
    raider.thresholds = leather.thresholds;
    raider.armor_class = Some(15);
    raider.skills[0] = Some(70);
    let destination = Destination {
        map: 12,
        hex: 20_100,
        elevation: 1,
        orientation: 0,
    };
    vec![
        weapon(common(PISTOL, item), &pistol).unwrap(),
        ammo(common(ROUNDS, item), &rounds).unwrap(),
        drug(
            common(STIM, item),
            &DrugSpec {
                stats: stats(1),
                ..DrugSpec::instant(drug_stat::CURRENT_HP, 10)
            },
        )
        .unwrap(),
        key(
            common(KEY, item),
            &KeySpec {
                stats: stats(1),
                code: 31,
            },
        )
        .unwrap(),
        misc_item_with(
            common(LAMP, item),
            &MiscSpec {
                stats: stats(1),
                charges: 30,
                ..MiscSpec::default()
            },
        )
        .unwrap(),
        armor(common(LEATHER, item), &leather).unwrap(),
        container(common(CHEST, ObjectFlags::SOLID), stats(20), 30, true).unwrap(),
        container(common(BAG, item), stats(2), 5, false).unwrap(),
        door(
            common(DOOR, ObjectFlags::SOLID),
            DoorSpec {
                locked: true,
                key: 31,
                ..DoorSpec::default()
            },
        )
        .unwrap(),
        stairs(common(STAIRS, ObjectFlags::SOLID), destination, true, 0).unwrap(),
        ladder(
            common(LADDER, ObjectFlags::SOLID),
            LadderKind::Up,
            destination,
            false,
            0,
        )
        .unwrap(),
        elevator(common(ELEVATOR, ObjectFlags::SOLID), 2, 1, 0).unwrap(),
        fighter_prototype(common(RAIDER, ObjectFlags::SOLID), &raider).unwrap(),
    ]
}

fn set() -> ProtoSet {
    let mut set = ProtoSet::new();
    for prototype in prototypes() {
        set.insert(prototype);
    }
    set
}

fn carried(set: &ProtoSet, pid: u32, id: u32, state: ItemState) -> MapObject {
    item_object(set, pid, id, None, 0, state).unwrap()
}

/// Every object of the test map, in hex order.
fn objects(set: &ProtoSet) -> Vec<MapObject> {
    let mut bag = carried(set, BAG, 13, ItemState::default());
    fill_inventory(
        &mut bag,
        vec![Stack::new(carried(set, KEY, 14, ItemState::default()), 1)],
    )
    .unwrap();
    let chest = container_object(
        set,
        CHEST,
        10,
        Some(20_000),
        0,
        LockState::LOCKED,
        vec![
            Stack::new(carried(set, PISTOL, 11, ItemState::default()), 1),
            Stack::new(
                carried(
                    set,
                    ROUNDS,
                    12,
                    ItemState {
                        rounds: Some(7),
                        ..ItemState::default()
                    },
                ),
                3,
            ),
            Stack::new(bag, 1),
            Stack::new(carried(set, STIM, 15, ItemState::default()), 2),
        ],
    )
    .unwrap();
    let ground_pistol = item_object(
        set,
        PISTOL,
        20,
        Some(20_002),
        0,
        ItemState {
            ammo: Some(ROUNDS),
            ..ItemState::default()
        },
    )
    .unwrap();
    let lamp = item_object(
        set,
        LAMP,
        21,
        Some(20_004),
        0,
        ItemState {
            charges: Some(17),
            ..ItemState::default()
        },
    )
    .unwrap();
    let ground_key = item_object(
        set,
        KEY,
        22,
        Some(20_006),
        0,
        ItemState {
            key: Some(99),
            ..ItemState::default()
        },
    )
    .unwrap();
    let locked = door_object(
        set,
        DOOR,
        30,
        20_200,
        0,
        LockState {
            locked: true,
            jammed: true,
        },
        None,
    )
    .unwrap();
    let open = door_object(set, DOOR, 31, 20_202, 0, LockState::default(), Some(3)).unwrap();
    let stairs = transit_object(set, STAIRS, 40, 20_400, 0, None).unwrap();
    let ladder = transit_object(set, LADDER, 41, 20_402, 0, Some(MAP_NUMBER)).unwrap();
    let lift = transit_object(set, ELEVATOR, 42, 20_404, 0, None).unwrap();
    let raider = critter(set);
    vec![
        chest,
        ground_pistol,
        lamp,
        ground_key,
        locked,
        open,
        stairs,
        ladder,
        lift,
        raider,
    ]
}

fn critter(set: &ProtoSet) -> MapObject {
    let prototype = set.get(RAIDER).unwrap();
    let placement = CritterPlacement::from_prototype(prototype, 50, 20_600, 0, 3).unwrap();
    critter_with_loadout(
        &placement,
        set,
        Loadout {
            right_hand: Some(carried(set, PISTOL, 51, ItemState::default())),
            left_hand: None,
            worn: Some(carried(set, LEATHER, 52, ItemState::default())),
            carried: vec![
                Stack::new(carried(set, ROUNDS, 53, ItemState::default()), 2),
                Stack::new(carried(set, STIM, 54, ItemState::default()), 1),
            ],
        },
    )
    .unwrap()
}

fn map(objects: Vec<MapObject>) -> MapFile {
    let mut map = empty_map("OBJECTS.MAP", MAP_NUMBER, 20_100).unwrap();
    map.objects.total = objects.len();
    map.objects.elevations[0] = objects;
    map
}

/// Field-for-field equality, recursively, except the reader's own file
/// offsets.
fn assert_same(got: &MapObject, expected: &MapObject) {
    let id = expected.id;
    assert_eq!(got.id, expected.id);
    assert_eq!(got.hex, expected.hex, "{id}");
    assert_eq!(got.displacement, expected.displacement, "{id}");
    assert_eq!(got.screen, expected.screen, "{id}");
    assert_eq!(got.frame, expected.frame, "{id}");
    assert_eq!(got.orientation, expected.orientation, "{id}");
    assert_eq!(got.art_id, expected.art_id, "{id}");
    assert_eq!(got.flags, expected.flags, "{id}");
    assert_eq!(got.stored_elevation, expected.stored_elevation, "{id}");
    assert_eq!(got.elevation, expected.elevation, "{id}");
    assert_eq!(got.pid, expected.pid, "{id}");
    assert_eq!(got.combat_id, expected.combat_id, "{id}");
    assert_eq!(got.light, expected.light, "{id}");
    assert_eq!(got.outline, expected.outline, "{id}");
    assert_eq!(got.script_id, expected.script_id, "{id}");
    assert_eq!(got.cached_script_file, expected.cached_script_file, "{id}");
    assert_eq!(got.inventory_capacity, expected.inventory_capacity, "{id}");
    assert_eq!(got.state, expected.state, "{id}");
    assert_eq!(got.owner, expected.owner, "{id}");
    assert_eq!(got.inventory.len(), expected.inventory.len(), "{id}");
    for (a, b) in got.inventory.iter().zip(&expected.inventory) {
        assert_eq!(a.quantity, b.quantity, "{id}");
        assert_same(&a.object, &b.object);
    }
}

#[test]
fn a_map_of_objects_with_state_reads_back_equal() {
    let set = set();
    let written = map(objects(&set));
    let bytes = LocalWriters.map(&written).unwrap();
    let parsed = MapFile::parse_for(&bytes, &set, GameProfile::Fallout2).unwrap();
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let got = &parsed.objects.elevations[0];
    assert_eq!(got.len(), written.objects.elevations[0].len());
    for (a, b) in got.iter().zip(&written.objects.elevations[0]) {
        assert_same(a, b);
    }
    assert_eq!(LocalWriters.map(&parsed).unwrap(), bytes);
}

#[test]
fn instance_words_hold_the_state() {
    let set = set();
    let objects = objects(&set);
    let state = |object: &MapObject| match &object.state {
        ObjectState::Other { word, detail } => (*word, detail.clone()),
        ObjectState::Critter(_) => panic!("not a critter"),
    };
    let chest = &objects[0];
    assert_eq!(state(chest), (LockFlags::LOCKED.0, ObjectDetail::Plain));
    assert_eq!(chest.inventory.len(), 4);
    assert_eq!(chest.inventory_capacity, 4);
    let pistol = &chest.inventory[0].object;
    assert_eq!(pistol.hex, None);
    assert_eq!(pistol.owner, Some(10));
    // A carried weapon is full with its default ammunition.
    assert_eq!(
        state(pistol).1,
        ObjectDetail::Weapon {
            rounds: 12,
            ammo_pid: ROUNDS as i32
        }
    );
    assert_eq!(chest.inventory[1].quantity, 3);
    assert_eq!(
        state(&chest.inventory[1].object).1,
        ObjectDetail::Ammunition(7)
    );
    let bag = &chest.inventory[2].object;
    assert_eq!(bag.inventory[0].object.owner, Some(13));
    assert_eq!(state(&bag.inventory[0].object).1, ObjectDetail::Key(31));
    assert_eq!(state(&objects[2]).1, ObjectDetail::Charges(17));
    assert_eq!(state(&objects[3]).1, ObjectDetail::Key(99));
    assert_eq!(
        state(&objects[4]).1,
        ObjectDetail::Door(LockFlags(LockFlags::LOCKED.0 | LockFlags::JAMMED.0))
    );
    let open = &objects[5];
    assert_eq!(state(open).1, ObjectDetail::Door(LockFlags::OPEN));
    assert_eq!(open.frame, 3);
    assert!(open.flags.contains(BasicFlags::NONBLOCKING));
    assert!(open.flags.contains(BasicFlags::PROJECTILE_THROUGH));
    let packed = PackedLocation(20_100 | (1 << 29));
    assert_eq!(
        state(&objects[6]).1,
        ObjectDetail::Stairs {
            map: 12,
            destination: packed
        }
    );
    assert_eq!(
        state(&objects[7]).1,
        ObjectDetail::Ladder {
            map: Some(MAP_NUMBER),
            destination: packed
        }
    );
    assert_eq!(
        state(&objects[8]).1,
        ObjectDetail::Elevator { kind: 2, level: 1 }
    );
}

#[test]
fn a_ladder_without_a_map_leads_within_its_own_map() {
    let set = set();
    let ladder = transit_object(&set, LADDER, 41, 20_402, 0, None).unwrap();
    let bytes = LocalWriters.map(&map(vec![ladder])).unwrap();
    let parsed = MapFile::parse_for(&bytes, &set, GameProfile::Fallout2).unwrap();
    let ObjectState::Other { detail, .. } = &parsed.objects.elevations[0][0].state else {
        panic!("a ladder")
    };
    assert!(matches!(
        detail,
        ObjectDetail::Ladder {
            map: Some(MAP_NUMBER),
            ..
        }
    ));
}

#[test]
fn an_armed_critter_holds_shows_and_wears_its_loadout() {
    let set = set();
    let raider = critter(&set);
    // The art takes the pistol's weapon code.
    let fid = Fid(raider.art_id);
    assert_eq!((fid.weapon(), fid.animation(), fid.base()), (5, 0, 1));
    let flags: Vec<u32> = raider.inventory.iter().map(|e| e.object.flags.0).collect();
    assert!(flags[0] & BasicFlags::WORN.0 != 0);
    assert!(flags[1] & BasicFlags::RIGHT_HAND.0 != 0);
    assert_eq!(flags[2] & 0x0700_0000, 0);
    assert_eq!(raider.inventory[2].quantity, 2);
    let ObjectState::Critter(state) = &raider.state else {
        panic!("a critter")
    };
    // Target HP 40 held through the bonus array; the fighter's AC and
    // protection are its prototype's.
    assert_eq!(state.hp, 40);
    let PrototypeData::Critter(prototype) = &set.get(RAIDER).unwrap().data else {
        panic!("a critter prototype")
    };
    assert_eq!(
        prototype.base_stats[stat::MAX_HIT_POINTS] + prototype.bonus_stats[stat::MAX_HIT_POINTS],
        40
    );
    assert_eq!(
        prototype.base_stats[stat::ARMOR_CLASS] + prototype.bonus_stats[stat::ARMOR_CLASS],
        15
    );
    assert_eq!(prototype.base_stats[24], 20);
    assert_eq!(prototype.base_stats[17], 2);
    // Small Guns 70 = base 35 + AG 5 + 30 points.
    assert_eq!(prototype.skills[0], 30);
    let bytes = set.get(RAIDER).unwrap().write();
    assert_eq!(bytes.len(), 416);
}

#[test]
fn what_the_engine_cannot_use_is_refused() {
    let set = set();
    let none = ItemState::default();
    // Wrong instance words for the subtype.
    assert!(
        item_object(
            &set,
            LEATHER,
            1,
            None,
            0,
            ItemState {
                rounds: Some(1),
                ..none
            }
        )
        .is_err()
    );
    assert!(
        item_object(
            &set,
            ROUNDS,
            1,
            None,
            0,
            ItemState {
                rounds: Some(21),
                ..none
            }
        )
        .is_err()
    );
    assert!(
        item_object(
            &set,
            ROUNDS,
            1,
            None,
            0,
            ItemState {
                rounds: Some(0),
                ..none
            }
        )
        .is_err()
    );
    assert!(
        item_object(
            &set,
            LAMP,
            1,
            None,
            0,
            ItemState {
                charges: Some(31),
                ..none
            }
        )
        .is_err()
    );
    // Ammunition that is not ammunition, or not of the calibre.
    assert!(
        item_object(
            &set,
            PISTOL,
            1,
            None,
            0,
            ItemState {
                ammo: Some(STIM),
                ..none
            }
        )
        .is_err()
    );
    assert!(
        item_object(
            &set,
            PISTOL,
            1,
            None,
            0,
            ItemState {
                ammo: Some(77),
                ..none
            }
        )
        .is_err()
    );
    // Not an item; off the grid.
    assert!(item_object(&set, DOOR, 1, None, 0, none).is_err());
    assert!(item_object(&set, KEY, 1, Some(40_000), 0, none).is_err());
    // A chest of capacity 30 refuses size 30 (the engine's strict test).
    let pistols = |n| vec![Stack::new(carried(&set, PISTOL, 2, none), n)];
    assert!(container_object(&set, CHEST, 1, Some(1), 0, LockState::LOCKED, pistols(14)).is_ok());
    assert!(container_object(&set, CHEST, 1, Some(1), 0, LockState::LOCKED, pistols(15)).is_err());
    assert!(container_object(&set, CHEST, 1, Some(1), 0, LockState::LOCKED, pistols(0)).is_err());
    assert!(container_object(&set, KEY, 1, Some(1), 0, LockState::LOCKED, Vec::new()).is_err());
    assert!(door_object(&set, STAIRS, 1, 1, 0, LockState::LOCKED, None).is_err());
    assert!(transit_object(&set, DOOR, 1, 1, 0, None).is_err());
    // A temporary child is not saved.
    let mut temporary = carried(&set, KEY, 3, none);
    temporary.flags = BasicFlags(temporary.flags.0 | BasicFlags::TEMPORARY.0);
    let mut bag = carried(&set, BAG, 4, none);
    assert!(fill_inventory(&mut bag, vec![Stack::new(temporary, 1)]).is_err());
    // Slots.
    let placement =
        CritterPlacement::from_prototype(set.get(RAIDER).unwrap(), 9, 100, 0, 0).unwrap();
    let wrong_hand = Loadout {
        right_hand: Some(carried(&set, LEATHER, 5, none)),
        ..Loadout::default()
    };
    assert!(critter_with_loadout(&placement, &set, wrong_hand).is_err());
    let wrong_worn = Loadout {
        worn: Some(carried(&set, PISTOL, 5, none)),
        ..Loadout::default()
    };
    assert!(critter_with_loadout(&placement, &set, wrong_worn).is_err());
    // An unarmed critter keeps weapon code 0.
    let bare = critter_with_loadout(&placement, &set, Loadout::default()).unwrap();
    assert_eq!(Fid(bare.art_id).weapon(), 0);
}

#[test]
fn the_engines_catalog_reads_the_map_from_a_mounted_layer() {
    let mut items = ProtoList::new(Category::Item);
    let mut scenery = ProtoList::new(Category::Scenery);
    let mut critters = ProtoList::new(Category::Critter);
    let mut files = Vec::new();
    for prototype in prototypes() {
        let list = match prototype.category {
            Category::Item => &mut items,
            Category::Scenery => &mut scenery,
            _ => &mut critters,
        };
        let (pid, file) = list.push_next().unwrap();
        assert_eq!(pid, prototype.pid);
        files.push((
            proto::prototype_path(prototype.category, &file),
            LocalWriters.pro(&prototype).unwrap(),
        ));
    }
    for list in [&items, &scenery, &critters] {
        files.push((list.list_path(), list.to_bytes()));
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
    let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);
    let set = set();
    let written = map(objects(&set));
    let bytes = LocalWriters.map(&written).unwrap();
    let parsed = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).unwrap();
    for (a, b) in parsed.objects.elevations[0]
        .iter()
        .zip(&written.objects.elevations[0])
    {
        assert_same(a, b);
    }
}

#[test]
fn maps_with_one_to_three_elevations() {
    // `empty_map` is the one-elevation case.
    let one = empty_map("LEVELS.MAP", 9, 20_100).unwrap();
    let same = map_with_elevations(
        "LEVELS.MAP",
        9,
        MapEntry {
            hex: 20_100,
            elevation: 0,
            orientation: 0,
        },
        [Some(blank_tiles()), None, None],
    )
    .unwrap();
    assert_eq!(
        LocalWriters.map(&one).unwrap(),
        LocalWriters.map(&same).unwrap()
    );
    assert_eq!(one.header.flags, same.header.flags);

    // Three elevations, each with a floor, entering on the second.
    let floor = |index: u16| {
        let mut tiles = blank_tiles();
        tiles[0] = TilePair {
            stored: 0x0001_0000 | u32::from(index),
            word: 0x0001_0000 | u32::from(index),
        };
        tiles
    };
    let entry = MapEntry {
        hex: 12_345,
        elevation: 1,
        orientation: 4,
    };
    let three = map_with_elevations(
        "LEVELS.MAP",
        9,
        entry,
        [Some(floor(5)), Some(floor(6)), Some(floor(7))],
    )
    .unwrap();
    assert_eq!(three.header.flags.0 & 0xe, 0);
    let bytes = LocalWriters.map(&three).unwrap();
    let parsed = MapFile::parse_for(&bytes, &ProtoSet::new(), GameProfile::Fallout2).unwrap();
    assert_eq!(parsed.header.entry_elevation, 1);
    assert_eq!(parsed.header.entry_orientation, 4);
    for elevation in 0..3 {
        assert_eq!(parsed.tiles[elevation], three.tiles[elevation]);
    }
    assert_eq!(LocalWriters.map(&parsed).unwrap(), bytes);

    // Two elevations, the first omitted; an object keeps a blank one.
    let set = set();
    let mut two = map_with_elevations(
        "LEVELS.MAP",
        9,
        MapEntry {
            hex: 100,
            elevation: 2,
            orientation: 0,
        },
        [None, Some(floor(5)), Some(blank_tiles())],
    )
    .unwrap();
    assert_eq!(two.header.flags.0 & 0xe, 0x2);
    two.objects.elevations[2] =
        vec![item_object(&set, KEY, 1, Some(101), 2, ItemState::default()).unwrap()];
    two.objects.total = 1;
    let bytes = LocalWriters.map(&two).unwrap();
    let parsed = MapFile::parse_for(&bytes, &set, GameProfile::Fallout2).unwrap();
    assert!(parsed.tiles[0].is_none());
    assert_eq!(parsed.tiles[1], two.tiles[1]);
    assert_eq!(parsed.tiles[2], two.tiles[2]);
    assert_eq!(parsed.objects.elevations[2][0].elevation, 2);

    // Refusals.
    let tiles = || [Some(blank_tiles()), None, None];
    let at = |elevation, orientation| MapEntry {
        hex: 1,
        elevation,
        orientation,
    };
    assert!(map_with_elevations("LEVELS.MAP", 9, at(1, 0), tiles()).is_err());
    assert!(map_with_elevations("LEVELS.MAP", 9, at(3, 0), tiles()).is_err());
    assert!(map_with_elevations("LEVELS.MAP", 9, at(0, 6), tiles()).is_err());
    assert!(
        map_with_elevations(
            "LEVELS.MAP",
            9,
            at(0, 0),
            [Some(vec![TilePair::BLANK; 3]), None, None]
        )
        .is_err()
    );
    assert!(map_with_elevations("", 9, at(0, 0), tiles()).is_err());
}
