//! The mini Super-Duper Mart: a synthetic Fallout 3 version plugin
//! (`HEDR` 0.94) and its archives, the converter's end-to-end loot
//! fixture (mirroring `crates/map-builder/tests/common/mart.rs`, but as
//! records the extract reads).
//!
//! One exterior cell with the store's door; the store's interior: two
//! rooms (the shop floor south of a wall along `y = 100`, a storeroom
//! north of it) on one navmesh, a locked storeroom door whose key lies in
//! the register, shelves and paper debris (clutter), a cash register
//! (caps, stimpaks, the key, rounds) and a locked locker holding a leveled
//! loot list, a hunting rifle and a stack of rounds on the floor, three
//! raiders (one with a pistol and leather armour, two bare-handed; very
//! aggressive, in a raider faction that is the player's enemy) and a
//! clerk (unaggressive). Everything in it is invented.

use std::path::{Path, PathBuf};

use fnv_bsa::fixture::BsaBuilder;
use fnv_esm::fixture::{CellSpec, FALLOUT3_PLUGIN_VERSION, PluginBuilder, WorldSpec, flat_heights};
use fnv_esm::{
    ActorBase, Armor, ArmorData, BipedData, Cell, Container, ContainerItem, Door, Eyes,
    FactionRank, FormId, Hair, Land, LeveledEntry, LeveledKind, LeveledList, Lock, MiscItem,
    NavTriangle, Navmesh, Npc, NpcStats, PlacedKind, PlacedRef, Race, RaceData, RacePart,
    RaceParts, RawRecord, RawSubrecord, Rgb, Static, World,
};
use nif::fixture::{box_mesh, walk_kf};

pub const PLUGIN: &str = "MiniMart.esm";
pub const WORLDSPACE: &str = "MMWorld";
pub const INTERIOR: &str = "MMMartInterior";

// Base forms.
const WORLD: u32 = 0x800;
const PERSISTENT_CELL: u32 = 0x801;
const CELL: u32 = 0x802;
const LAND: u32 = 0x803;
const INTERIOR_CELL: u32 = 0x804;
const NAVMESH: u32 = 0x805;
pub const PLAYER_FACTION: u32 = 0x810;
pub const RAIDER_FACTION: u32 = 0x811;
pub const SHOP_FACTION: u32 = 0x812;
const WALL: u32 = 0x820;
const SHELF: u32 = 0x821;
const DEBRIS: u32 = 0x822;
const FRONT_DOOR_BASE: u32 = 0x823;
const STORE_DOOR_BASE: u32 = 0x824;
const REGISTER: u32 = 0x825;
const LOCKER: u32 = 0x826;
pub const PISTOL: u32 = 0x830;
pub const RIFLE: u32 = 0x831;
pub const AMMO_10MM: u32 = 0x832;
pub const AMMO_308: u32 = 0x833;
pub const LEATHER: u32 = 0x834;
pub const STIMPAK: u32 = 0x835;
pub const CAPS: u32 = 0x836;
pub const STORE_KEY: u32 = 0x837;
const RESTORE_HEALTH: u32 = 0x838;
pub const LOOT_LIST: u32 = 0x839;
const RACE: u32 = 0x840;
const HAIR: u32 = 0x841;
const EYES: u32 = 0x842;
pub const GUNNER: u32 = 0x850;
pub const RAIDER: u32 = 0x851;
pub const CLERK: u32 = 0x852;

// References.
const R_DOOR_OUT: u32 = 0x900;
pub const R_FRONT_DOOR: u32 = 0x901;
pub const R_STORE_DOOR: u32 = 0x902;
const R_WALL_WEST: u32 = 0x903;
const R_WALL_EAST: u32 = 0x904;
const R_SHELVES: [u32; 3] = [0x905, 0x906, 0x907];
pub const R_DEBRIS: [u32; 5] = [0x908, 0x909, 0x90A, 0x90B, 0x90C];
pub const R_REGISTER: u32 = 0x910;
pub const R_LOCKER: u32 = 0x911;
pub const R_RIFLE: u32 = 0x912;
pub const R_AMMO: u32 = 0x913;
pub const R_GUNNER: u32 = 0x920;
pub const R_RAIDERS: [u32; 2] = [0x921, 0x922];
pub const R_CLERK: u32 = 0x923;

/// What [`write`] wrote.
pub struct MiniMart {
    pub data_dir: PathBuf,
    pub slice: mojave_convert::SliceSpec,
}

/// What [`write_linked`] wrote: the install, and a layer file naming the
/// car park outside and the two-storey store.
pub struct LinkedMart {
    pub data_dir: PathBuf,
    pub layer_file: PathBuf,
}

/// What [`write_groups`] adds to the linked mart: an enable group in the
/// store (a parent that starts disabled, a normal child that starts
/// disabled, an opposite child that starts enabled, a rifle and a clerk
/// that are children too), a trigger volume, a linked reference, and a
/// travel package that takes the clerk to the car park's door.
pub const R_GROUP_PARENT: u32 = 0x940;
pub const R_GROUP_CHILD: u32 = 0x941;
pub const R_GROUP_OPPOSITE: u32 = 0x942;
pub const R_GROUP_RIFLE: u32 = 0x943;
pub const R_GROUP_CLERK: u32 = 0x944;
pub const R_TRIGGER: u32 = 0x945;
/// A plain door whose linked reference is the locker.
pub const R_LINKED_DOOR: u32 = 0x946;
const TRIGGER_BASE: u32 = 0x827;
const CLERK_PACKAGE: u32 = 0x860;

/// The upper storey's height (a mezzanine east of the shop floor, up a
/// ramp of slope 1).
pub const UPPER_Z: f32 = 200.0;
/// The shelf and the raider standing up there.
pub const R_UPPER_SHELF: u32 = 0x930;
pub const R_UPPER_RAIDER: u32 = 0x931;

/// A form key of the plugin, as text.
pub fn key(id: u32) -> String {
    mir::FormKey::new(PLUGIN, id).to_string()
}

fn zstr(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

fn bytes(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// A navmesh over the rooms: a grid of 200-unit squares from (-600,
/// -400) to (600, 400), at height 0, links across shared edges. With
/// `upper`, also a ramp from x 600 to 800 (y -400 to -200) up to a
/// mezzanine of 200-unit squares from (800, -400) to (1200, 400) at
/// [`UPPER_Z`].
fn navmesh(upper: bool) -> Navmesh {
    // Quads as corner heights over a 200-unit square at (x, y).
    let mut quads: Vec<([f32; 2], [f32; 2])> = Vec::new();
    for j in 0..4 {
        for i in 0..6 {
            quads.push((
                [-600.0 + 200.0 * i as f32, -400.0 + 200.0 * j as f32],
                [0.0, 0.0],
            ));
        }
    }
    if upper {
        quads.push(([600.0, -400.0], [0.0, UPPER_Z]));
        for j in 0..4 {
            for i in 0..2 {
                quads.push((
                    [800.0 + 200.0 * i as f32, -400.0 + 200.0 * j as f32],
                    [UPPER_Z, UPPER_Z],
                ));
            }
        }
    }
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let index = |p: [f32; 3], vertices: &mut Vec<[f32; 3]>| -> i16 {
        match vertices.iter().position(|v| *v == p) {
            Some(i) => i as i16,
            None => {
                vertices.push(p);
                (vertices.len() - 1) as i16
            }
        }
    };
    let mut corners: Vec<[i16; 3]> = Vec::new();
    for ([x, y], [west, east]) in quads {
        let a = index([x, y, west], &mut vertices);
        let b = index([x + 200.0, y, east], &mut vertices);
        let c = index([x + 200.0, y + 200.0, east], &mut vertices);
        let d = index([x, y + 200.0, west], &mut vertices);
        corners.push([a, b, c]);
        corners.push([a, c, d]);
    }
    let triangles = corners
        .iter()
        .map(|t| {
            let mut edges = [-1i16; 3];
            for (e, edge) in edges.iter_mut().enumerate() {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                if let Some(other) = corners
                    .iter()
                    .position(|o| (0..3).any(|k| o[k] == b && o[(k + 1) % 3] == a))
                {
                    *edge = other as i16;
                }
            }
            NavTriangle {
                vertices: *t,
                edges,
                flags: 0,
            }
        })
        .collect();
    Navmesh::new(NAVMESH, INTERIOR_CELL, vertices, triangles)
}

/// `AIDT`: aggression, confidence, energy, responsibility, mood, then
/// zeros (20 bytes, the layout both games share).
fn ai_data(aggression: u8) -> RawSubrecord {
    let mut data = vec![0u8; 20];
    data[0] = aggression;
    data[1] = 2;
    data[2] = 50;
    data[3] = 50;
    RawSubrecord::new(b"AIDT", data)
}

fn bc1(rgb: [u8; 3]) -> Vec<u8> {
    let rgba: Vec<u8> = std::iter::repeat_n([rgb[0], rgb[1], rgb[2], 255], 256)
        .flatten()
        .collect();
    dds::fixture::encode_bc1(16, 16, &rgba)
}

/// Writes the install under `dir` (`dir/Data/...`).
pub fn write(dir: &Path) -> MiniMart {
    write_install(dir, false, false)
}

/// The linked mart (as [`write_linked`]) with the references of
/// [`R_GROUP_PARENT`] and the rest, and `groups.layer.toml`: the same two
/// maps with `enable_groups = true` and `presence_copies = true`.
pub fn write_groups(dir: &Path) -> LinkedMart {
    let mart = write_linked_with(dir, true);
    let layer = "name = \"minimart-groups\"\nstart = \"mmlot\"\npresence_copies = true\n\n\
                 [all_maps]\nwire = true\nenable_groups = true\n\n\
                 [[maps]]\nslice = \"lot.toml\"\nmap = \"mmlot\"\n\n\
                 [[maps]]\nslice = \"store.toml\"\nmap = \"mmstore\"\nstoreys = \"auto\"\n";
    let layer_file = dir.join("groups.layer.toml");
    std::fs::write(&layer_file, layer).expect("the layer file");
    LinkedMart {
        data_dir: mart.data_dir,
        layer_file,
    }
}

/// The install with the mezzanine (a shelf and a raider up there), and
/// beside it `lot.toml` (the car park outside the store's door),
/// `store.toml` (the store, its storeys split) and `mart.layer.toml`
/// naming both as one layer whose start map is the car park.
pub fn write_linked(dir: &Path) -> LinkedMart {
    write_linked_with(dir, false)
}

fn write_linked_with(dir: &Path, groups: bool) -> LinkedMart {
    let mart = write_install(dir, true, groups);
    let lot = format!(
        "name = \"minimart-lot\"\ngame = \"fo3\"\nworldspace = \"{WORLDSPACE}\"\n\
         plugins = [\"{PLUGIN}\"]\nx = [0, 0]\ny = [0, 0]\ninterior_depth = 1\n\
         map_center = [2048.0, 2048.0]\nloot = true\nanimations = \"combat\"\n"
    );
    std::fs::write(dir.join("lot.toml"), lot).expect("lot.toml");
    std::fs::write(dir.join("store.toml"), mart.slice.to_toml()).expect("store.toml");
    let layer = "name = \"minimart-linked\"\nstart = \"mmlot\"\n\n[all_maps]\nwire = true\n\n\
                 [[maps]]\nslice = \"lot.toml\"\nmap = \"mmlot\"\n\n\
                 [[maps]]\nslice = \"store.toml\"\nmap = \"mmstore\"\nstoreys = \"auto\"\n";
    let layer_file = dir.join("mart.layer.toml");
    std::fs::write(&layer_file, layer).expect("the layer file");
    LinkedMart {
        data_dir: mart.data_dir,
        layer_file,
    }
}

fn write_install(dir: &Path, upper: bool, groups: bool) -> MiniMart {
    let data = dir.join("Data");
    std::fs::create_dir_all(&data).expect("data dir");
    let mut plugin = PluginBuilder::new();
    plugin
        .version(FALLOUT3_PLUGIN_VERSION)
        .master_file(true)
        .author("mojave-foundry mini mart fixture");

    // Factions: raiders and the player are enemies; the shop is neutral.
    let relation = |other: u32, modifier: i32, reaction: u32| {
        bytes(&[
            &other.to_le_bytes(),
            &modifier.to_le_bytes(),
            &reaction.to_le_bytes(),
        ])
    };
    plugin.add_raw(
        RawRecord::new(b"FACT", PLAYER_FACTION)
            .with(b"EDID", zstr("PlayerFaction"))
            .with(b"FULL", zstr("Player Faction"))
            .with(b"XNAM", relation(RAIDER_FACTION, -100, 1))
            .with(b"DATA", vec![0, 0, 0, 0]),
    );
    plugin.add_raw(
        RawRecord::new(b"FACT", RAIDER_FACTION)
            .with(b"EDID", zstr("MMRaiderFaction"))
            .with(b"FULL", zstr("Raiders"))
            .with(b"XNAM", relation(PLAYER_FACTION, -100, 1))
            .with(b"DATA", vec![3, 0, 0, 0]),
    );
    plugin.add_raw(
        RawRecord::new(b"FACT", SHOP_FACTION)
            .with(b"EDID", zstr("MMShopFaction"))
            .with(b"FULL", zstr("Shopkeepers"))
            .with(b"DATA", vec![0, 0, 0, 0]),
    );

    // Statics, doors and containers.
    plugin.add(Static::new(WALL, "MMWall", "minimart\\wall.nif"));
    plugin.add(Static::new(SHELF, "MMShelf", "minimart\\shelf.nif"));
    plugin.add(Static::new(
        DEBRIS,
        "MMPaperDebris",
        "minimart\\paperdebris01.nif",
    ));
    if groups {
        plugin.add_raw(
            RawRecord::new(b"ACTI", TRIGGER_BASE)
                .with(b"EDID", zstr("MMStoreTrigger"))
                .with(b"MODL", zstr("minimart\\wall.nif")),
        );
        let mut package = fnv_esm::Package::new(CLERK_PACKAGE, "MMClerkToTheLot", 6);
        package.location = Some(fnv_esm::PackageLocation {
            location_type: fnv_esm::PackageLocation::NEAR_REFERENCE,
            value: R_DOOR_OUT,
            radius: 256,
        });
        plugin.add(package);
    }
    plugin.add(Door::new(
        FRONT_DOOR_BASE,
        "MMFrontDoor",
        "Front Door",
        "minimart\\door.nif",
    ));
    plugin.add(Door::new(
        STORE_DOOR_BASE,
        "MMStoreDoor",
        "Storeroom Door",
        "minimart\\storedoor.nif",
    ));
    plugin.add(Container::new(
        REGISTER,
        "MMRegister",
        "Cash Register",
        "minimart\\register.nif",
        vec![
            ContainerItem::new(CAPS, 50),
            ContainerItem::new(STIMPAK, 2),
            ContainerItem::new(STORE_KEY, 1),
            ContainerItem::new(AMMO_10MM, 12),
        ],
    ));
    plugin.add(Container::new(
        LOCKER,
        "MMLocker",
        "Storeroom Locker",
        "minimart\\locker.nif",
        vec![
            ContainerItem::new(LOOT_LIST, 2),
            ContainerItem::new(AMMO_308, 10),
        ],
    ));
    plugin.add(LeveledList::new(
        LeveledKind::Item,
        LOOT_LIST,
        "MMLootList",
        vec![
            LeveledEntry::new(1, AMMO_10MM, 12),
            LeveledEntry::new(1, STIMPAK, 1),
            LeveledEntry::new(5, RIFLE, 1),
        ],
    ));

    // Items. Weapons: DATA (value, health, weight, damage, clip), DNAM
    // (animation type at 0, flags at 12, action points at 68, skill at
    // 104), NAM0 (ammunition).
    let weapon = |id: u32,
                  editor_id: &str,
                  name: &str,
                  model: &str,
                  animation: u32,
                  damage: i16,
                  clip: u8,
                  ammo: u32| {
        let data = bytes(&[
            &150i32.to_le_bytes(),
            &100i32.to_le_bytes(),
            &4.0f32.to_le_bytes(),
            &damage.to_le_bytes(),
            &[clip],
        ]);
        let mut dnam = vec![0u8; 136];
        dnam[0..4].copy_from_slice(&animation.to_le_bytes());
        dnam[48..52].copy_from_slice(&1024.0f32.to_le_bytes());
        dnam[64..68].copy_from_slice(&1.0f32.to_le_bytes());
        dnam[68..72].copy_from_slice(&20.0f32.to_le_bytes());
        dnam[104..108].copy_from_slice(&41u32.to_le_bytes());
        RawRecord::new(b"WEAP", id)
            .with(b"EDID", zstr(editor_id))
            .with(b"FULL", zstr(name))
            .with(b"MODL", zstr(model))
            .with(b"NAM0", ammo.to_le_bytes().to_vec())
            .with(b"DATA", data)
            .with(b"DNAM", dnam)
    };
    plugin.add_raw(weapon(
        PISTOL,
        "MMWeap10mmPistol",
        "10mm Pistol",
        "minimart\\pistol.nif",
        3,
        8,
        12,
        AMMO_10MM,
    ));
    plugin.add_raw(weapon(
        RIFLE,
        "MMWeapHuntingRifle",
        "Hunting Rifle",
        "minimart\\rifle.nif",
        5,
        25,
        5,
        AMMO_308,
    ));
    // Ammunition: DATA (speed, flags, three unused, value, clip rounds).
    let ammo = |id: u32, editor_id: &str, name: &str, value: i32, pack: u8| {
        let data = bytes(&[
            &1.0f32.to_le_bytes(),
            &[0, 0, 0, 0],
            &value.to_le_bytes(),
            &[pack],
        ]);
        RawRecord::new(b"AMMO", id)
            .with(b"EDID", zstr(editor_id))
            .with(b"FULL", zstr(name))
            .with(b"MODL", zstr("minimart\\ammobox.nif"))
            .with(b"DATA", data)
    };
    plugin.add_raw(ammo(AMMO_10MM, "MMAmmo10mm", "10mm Round", 1, 12));
    plugin.add_raw(ammo(AMMO_308, "MMAmmo308", ".308 Caliber Round", 2, 5));
    plugin.add(Armor {
        common: fnv_esm::Common::new(LEATHER)
            .with_editor_id("MMArmorLeather")
            .with_name("Leather Armor")
            .with_model("minimart\\armor\\leather.nif"),
        biped: Some(BipedData {
            biped_flags: BipedData::SLOT_UPPER_BODY,
            general_flags: 0,
        }),
        data: Some(ArmorData {
            value: 160,
            health: 100,
            weight: 15.0,
        }),
        // Armour rating 24 (stored in hundredths), Fallout 3's 4-byte form.
        stats: Some(fnv_esm::ArmorStats {
            armor_rating: 2400,
            damage_threshold: None,
            flags: 0,
        }),
        ..Armor::default()
    });
    // A stimpak: one restore-health effect of 30.
    plugin.add_raw(RawRecord::new(b"MGEF", RESTORE_HEALTH).with(b"EDID", zstr("MMRestoreHealth")));
    let effect = bytes(&[
        &30u32.to_le_bytes(),
        &0u32.to_le_bytes(),
        &0u32.to_le_bytes(),
        &0u32.to_le_bytes(),
        &16i32.to_le_bytes(),
    ]);
    let enit = bytes(&[
        &25i32.to_le_bytes(),
        &[0, 0, 0, 0],
        &0u32.to_le_bytes(),
        &0.0f32.to_le_bytes(),
        &0u32.to_le_bytes(),
    ]);
    plugin.add_raw(
        RawRecord::new(b"ALCH", STIMPAK)
            .with(b"EDID", zstr("MMStimpak"))
            .with(b"FULL", zstr("Stimpak"))
            .with(b"MODL", zstr("minimart\\stimpak.nif"))
            .with(b"DATA", 0.0f32.to_le_bytes().to_vec())
            .with(b"ENIT", enit)
            .with(b"EFID", RESTORE_HEALTH.to_le_bytes().to_vec())
            .with(b"EFIT", effect),
    );
    plugin.add(MiscItem::new(
        CAPS,
        "MMCaps",
        "Bottle Cap",
        "minimart\\cap.nif",
        1,
        0.0,
    ));
    plugin.add_raw(
        RawRecord::new(b"KEYM", STORE_KEY)
            .with(b"EDID", zstr("MMStoreKey"))
            .with(b"FULL", zstr("Storeroom Key"))
            .with(b"MODL", zstr("minimart\\key.nif"))
            .with(
                b"DATA",
                bytes(&[&0i32.to_le_bytes(), &0.0f32.to_le_bytes()]),
            ),
    );

    // People: a race with a head, an upper body and a skinned right hand.
    let part = |index: u32, model: Option<&str>, texture: Option<&str>| {
        RacePart::new(index, model, texture)
    };
    let mut race = Race::new(RACE, "MMRace", "Wastelander");
    race.data = Some(RaceData {
        height: [1.0, 1.0],
        weight: [1.0, 1.0],
        flags: 1,
        ..RaceData::default()
    });
    race.default_hair = Some([FormId(HAIR), FormId(HAIR)]);
    race.head = Some(RaceParts {
        male: vec![part(0, Some("MiniMart\\Characters\\HeadMale.NIF"), None)],
        female: vec![part(0, Some("MiniMart\\Characters\\HeadMale.NIF"), None)],
    });
    let body = vec![
        part(0, Some("MiniMart\\Characters\\UpperBodyMale.nif"), None),
        part(2, Some("MiniMart\\Characters\\RightHandMale.nif"), None),
    ];
    race.body = Some(RaceParts {
        male: body.clone(),
        female: body,
    });
    race.hairs = vec![FormId(HAIR)];
    race.eyes = vec![FormId(EYES)];
    plugin.add(race);
    plugin.add(Hair::new(
        HAIR,
        "MMHair",
        "MiniMart\\Characters\\Hair.NIF",
        "MiniMart\\Characters\\Hair.dds",
    ));
    plugin.add(Eyes::new(EYES, "MMEyes", "MiniMart\\Characters\\Eyes.dds"));
    let person = |id: u32,
                  editor_id: &str,
                  name: &str,
                  health: i32,
                  level: i16,
                  aggression: u8,
                  faction: u32,
                  inventory: Vec<ContainerItem>| {
        let mut npc = Npc::new(id, editor_id, name);
        npc.common.model = Some("Characters\\_Male\\Skeleton.NIF".to_owned());
        npc.stats = Some(NpcStats {
            base_health: health,
            attributes: [6, 5, 6, 3, 4, 6, 4],
        });
        npc.base = Some(ActorBase {
            level,
            ..ActorBase::default()
        });
        npc.race = Some(FormId(RACE));
        npc.hair = Some(FormId(HAIR));
        npc.eyes = Some(FormId(EYES));
        npc.hair_color = Some(Rgb {
            r: 60,
            g: 40,
            b: 20,
        });
        npc.factions = vec![FactionRank {
            faction: FormId(faction),
            rank: 0,
        }];
        npc.inventory = inventory;
        npc.unknown.push(ai_data(aggression));
        npc
    };
    plugin.add(person(
        GUNNER,
        "MMRaiderGunner",
        "Raider",
        60,
        4,
        2,
        RAIDER_FACTION,
        vec![
            ContainerItem::new(PISTOL, 1),
            ContainerItem::new(LEATHER, 1),
            ContainerItem::new(AMMO_10MM, 24),
            ContainerItem::new(STIMPAK, 1),
        ],
    ));
    plugin.add(person(
        RAIDER,
        "MMRaider",
        "Raider",
        40,
        2,
        2,
        RAIDER_FACTION,
        vec![ContainerItem::new(CAPS, 12)],
    ));
    let mut clerk = person(
        CLERK,
        "MMClerk",
        "Clerk",
        30,
        1,
        0,
        SHOP_FACTION,
        Vec::new(),
    );
    if groups {
        clerk.packages = vec![FormId(CLERK_PACKAGE)];
    }
    plugin.add(clerk);

    // The exterior: one flat cell with the store's door.
    let door_out_at = [2048.0, 2048.0, 0.0];
    let front_at = [0.0, -390.0, 0.0];
    let mut world = WorldSpec::new(World::new(WORLD, WORLDSPACE));
    let mut persistent = CellSpec::new(Cell::exterior(PERSISTENT_CELL, 0, 0));
    persistent.persistent = vec![
        PlacedRef::new(
            PlacedKind::Object,
            R_DOOR_OUT,
            FRONT_DOOR_BASE,
            door_out_at,
            [0.0; 3],
        )
        .with_teleport(
            R_FRONT_DOOR,
            [front_at[0], front_at[1] + 60.0, 0.0],
            [0.0; 3],
        )
        .persistent(),
    ];
    world = world.with_persistent_cell(persistent);
    let mut land = Land::new(LAND).with_heights(&flat_heights(0.0));
    land.flags = Some(1);
    land.normals = Some(fnv_esm::fixture::up_normals());
    world = world.with_cell(CellSpec::new(Cell::exterior(CELL, 0, 0)).with_land(land));
    plugin.world(world);

    // The store.
    let at =
        |id: u32, base: u32, p: [f32; 3]| PlacedRef::new(PlacedKind::Object, id, base, p, [0.0; 3]);
    let mut store_door = at(R_STORE_DOOR, STORE_DOOR_BASE, [0.0, 100.0, 0.0]);
    store_door.lock = Some(Lock {
        level: 50,
        key: FormId(STORE_KEY),
        flags: 0,
    });
    let mut locker = at(R_LOCKER, LOCKER, [300.0, 300.0, 0.0]);
    locker.lock = Some(Lock {
        level: 25,
        key: FormId::NULL,
        flags: 0,
    });
    let mut rounds = at(R_AMMO, AMMO_10MM, [-60.0, -300.0, 0.0]);
    rounds.count = Some(30);
    let mut interior = Cell::interior(INTERIOR_CELL, INTERIOR);
    interior.common.full_name = Some("Mini Mart".to_owned());
    let mut inside = CellSpec::new(interior)
        .with_navmesh(navmesh(upper))
        .with_persistent(
            at(R_FRONT_DOOR, FRONT_DOOR_BASE, front_at)
                .with_teleport(
                    R_DOOR_OUT,
                    [door_out_at[0], door_out_at[1] - 100.0, 0.0],
                    [0.0, 0.0, 3.1],
                )
                .persistent(),
        )
        .with_temporary(store_door)
        .with_temporary(at(R_WALL_WEST, WALL, [-340.0, 100.0, 0.0]))
        .with_temporary(at(R_WALL_EAST, WALL, [340.0, 100.0, 0.0]))
        .with_temporary(at(R_SHELVES[0], SHELF, [-350.0, -200.0, 0.0]))
        .with_temporary(at(R_SHELVES[1], SHELF, [-350.0, 260.0, 0.0]))
        .with_temporary(at(R_SHELVES[2], SHELF, [360.0, -150.0, 0.0]))
        .with_temporary(at(R_REGISTER, REGISTER, [200.0, -260.0, 0.0]))
        .with_temporary(locker)
        .with_temporary(at(R_RIFLE, RIFLE, [-120.0, -300.0, 0.0]))
        .with_temporary(rounds);
    for (i, id) in R_DEBRIS.iter().enumerate() {
        let x = -200.0 + 90.0 * i as f32;
        inside = inside.with_temporary(at(*id, DEBRIS, [x, -120.0 + 40.0 * (i % 2) as f32, 0.0]));
    }
    let npc = |id: u32, base: u32, p: [f32; 3]| {
        PlacedRef::new(PlacedKind::Npc, id, base, p, [0.0, 0.0, 1.0])
    };
    inside = inside
        .with_temporary(npc(R_GUNNER, GUNNER, [-300.0, -60.0, 0.0]))
        .with_temporary(npc(R_RAIDERS[0], RAIDER, [-200.0, 20.0, 0.0]))
        .with_temporary(npc(R_RAIDERS[1], RAIDER, [100.0, 260.0, 0.0]))
        .with_temporary(npc(R_CLERK, CLERK, [120.0, -200.0, 0.0]));
    if groups {
        // The group: a parent that starts disabled (a shelf), a child that
        // follows it, an opposite child that starts enabled, a rifle and a
        // clerk that follow it too, and a trigger volume.
        let mut linked_door = at(R_LINKED_DOOR, FRONT_DOOR_BASE, [-400.0, 300.0, 0.0]);
        linked_door.linked_ref = Some(FormId(R_LOCKER));
        inside = inside
            .with_temporary(at(R_GROUP_PARENT, SHELF, [-100.0, 300.0, 0.0]).initially_disabled())
            .with_temporary(
                at(R_GROUP_CHILD, SHELF, [-200.0, 300.0, 0.0])
                    .with_enable_parent(R_GROUP_PARENT, false)
                    .initially_disabled(),
            )
            .with_temporary(
                at(R_GROUP_OPPOSITE, SHELF, [0.0, 300.0, 0.0])
                    .with_enable_parent(R_GROUP_PARENT, true),
            )
            .with_temporary(
                at(R_GROUP_RIFLE, RIFLE, [-250.0, -350.0, 0.0])
                    .with_enable_parent(R_GROUP_PARENT, false)
                    .initially_disabled(),
            )
            .with_temporary(
                npc(R_GROUP_CLERK, CLERK, [250.0, 180.0, 0.0])
                    .with_enable_parent(R_GROUP_PARENT, false)
                    .initially_disabled(),
            )
            .with_temporary(
                at(R_TRIGGER, TRIGGER_BASE, [0.0, -50.0, 0.0]).with_primitive(fnv_esm::Primitive {
                    bounds: [60.0, 60.0, 60.0],
                    shape: fnv_esm::Primitive::SHAPE_BOX,
                    ..fnv_esm::Primitive::default()
                }),
            )
            .with_temporary(linked_door);
    }
    if upper {
        inside = inside
            .with_temporary(at(R_UPPER_SHELF, SHELF, [1000.0, 250.0, UPPER_Z]))
            .with_temporary(npc(R_UPPER_RAIDER, RAIDER, [1000.0, -100.0, UPPER_Z]));
    }
    plugin.interior(inside);
    std::fs::write(data.join(PLUGIN), plugin.build()).expect("plugin");

    // Meshes and textures.
    let mut meshes = BsaBuilder::new();
    let tex = |name: &str| format!("textures\\minimart\\{name}.dds");
    let props: [(&str, [f32; 3], &str); 14] = [
        ("wall", [520.0, 20.0, 200.0], "wall"),
        ("shelf", [200.0, 40.0, 120.0], "wood"),
        ("paperdebris01", [24.0, 24.0, 6.0], "paper"),
        ("door", [100.0, 10.0, 200.0], "wood"),
        ("storedoor", [110.0, 12.0, 200.0], "wood"),
        ("register", [40.0, 30.0, 30.0], "metal"),
        ("locker", [60.0, 40.0, 160.0], "metal"),
        ("pistol", [20.0, 4.0, 12.0], "metal"),
        ("rifle", [60.0, 6.0, 8.0], "wood"),
        ("ammobox", [10.0, 8.0, 6.0], "metal"),
        ("stimpak", [8.0, 8.0, 10.0], "paper"),
        ("key", [6.0, 2.0, 2.0], "metal"),
        ("cap", [3.0, 3.0, 1.0], "metal"),
        ("armor\\leather", [50.0, 25.0, 100.0], "leather"),
    ];
    for (name, size, texture) in props {
        meshes.add(
            &format!("meshes\\minimart\\{name}.nif"),
            &box_mesh(size, &tex(texture)),
        );
    }
    let skin = tex("skin");
    meshes.add(mir::HUMAN_SKELETON, &mir::fixture::human_skeleton());
    meshes.add(
        "meshes\\minimart\\characters\\righthandmale.nif",
        &mir::fixture::textured_skinned_body(&skin),
    );
    for (name, size) in [("headmale", 20.0), ("upperbodymale", 60.0), ("hair", 22.0)] {
        meshes.add(
            &format!("meshes\\minimart\\characters\\{name}.nif"),
            &box_mesh([size, size * 0.5, size * 2.0], &skin),
        );
    }
    meshes.add(mir::HUMAN_STAND, &mir::fixture::stand_kf());
    meshes.add(mir::HUMAN_WALK_MALE, &walk_kf());
    for (name, kf) in [
        ("h2hattackright_a.kf", mir::fixture::punch_kf()),
        ("locomotion\\male\\mtfastforward.kf", mir::fixture::run_kf()),
        ("idleanims\\mt_knockdownfaceup.kf", mir::fixture::fall_kf()),
        ("1hpaim.kf", mir::fixture::aim_kf()),
        ("1hpattackright.kf", mir::fixture::fire_kf()),
    ] {
        meshes.add(&format!("{}{name}", mir::HUMAN_ANIMATIONS), &kf);
    }
    std::fs::write(data.join("MiniMart - Meshes.bsa"), meshes.build()).expect("meshes");
    let mut textures = BsaBuilder::new();
    for (name, rgb) in [
        ("wall", [190, 180, 160]),
        ("wood", [120, 90, 60]),
        ("paper", [220, 215, 200]),
        ("metal", [130, 130, 140]),
        ("leather", [110, 70, 40]),
        ("skin", [200, 150, 120]),
    ] {
        textures.add(&tex(name), &bc1(rgb));
    }
    std::fs::write(data.join("MiniMart - Textures.bsa"), textures.build()).expect("textures");

    let slice = mojave_convert::SliceSpec::from_toml(&format!(
        "name = \"minimart\"\ngame = \"fo3\"\nworldspace = \"{WORLDSPACE}\"\n\
         plugins = [\"{PLUGIN}\"]\nx = [0, 0]\ny = [0, 0]\ninterior_depth = 1\n\
         interior = \"{INTERIOR}\"\nplace_disabled = true\nloot = true\n\
         animations = \"combat\"\nfootprints = true\n\n[declutter]\nprofile = \"readable\"\n"
    ))
    .expect("the slice parses");
    MiniMart {
        data_dir: data,
        slice,
    }
}
