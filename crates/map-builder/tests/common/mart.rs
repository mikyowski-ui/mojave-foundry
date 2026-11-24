//! The mini mart: a parking lot (the town scene) and a two-room store
//! (an interior: the shop floor and a storeroom behind a wall with a
//! locked door), linked by a teleport door pair. Clutter, a counter of
//! non-blocking strips and a wide pool table, three raiders on a hostile
//! team (a pistol and leather armour, a rifle, bare hands), a clerk with no
//! catalogue entry, two containers (a register holding a pistol, ammunition,
//! stimpaks, caps and the storeroom key; a locked locker), a rifle and an
//! ammunition box on the floor. The converter's end-to-end "mini
//! Super-Duper Mart" fixture mirrors it.

use std::collections::BTreeMap;
use std::sync::Arc;

use emit::items::drug_stat;
use emit::objects::{ItemState, LockState};
use emit::proto::ItemStats;
use emit::{
    AmmoSpec, ArmorSpec, AttackMode, CritterSpec, DamageType, DrugSpec, FighterSpec, KeySpec,
    MiscSpec, ProtoText, WeaponCode, WeaponSpec,
};
use map_builder::{
    ArtCategory, ArtImage, BuildOptions, Catalogue, CritterArt, CritterLoot, ElevationInput,
    GroundArt, GroundTiles, Held, InteriorBuild, ItemDef, ItemIcon, ItemKind, LayerInput,
    LayerOptions, MapInput, PlacedLoot, Role,
};
use mir::{CellDoc, CellLocation, CellRef, FormKey, GroundSampler, MapWindow};

use super::saloon::{form, resident};
use super::scenes::full_base;
use super::*;

pub const FLOOR_Z: f32 = 1000.0;
pub const MART: u32 = 0x8000;
/// The lot's door into the mart, and the mart's front door back out.
pub const LOT_DOOR: u32 = 0x8e01;
pub const FRONT_DOOR: u32 = 0x8101;
/// The locked door between the shop floor and the storeroom.
pub const STORE_DOOR: u32 = 0x8102;
pub const FRONT_AT: [f32; 3] = [0.0, -290.0, FLOOR_Z];
/// The wall between the rooms runs along this `y`; the storeroom is north.
pub const WALL_Y: f32 = 100.0;
pub const COUNTER: u32 = 0x8401;
pub const POOL_TABLE: u32 = 0x8402;
pub const SHELVES: [u32; 2] = [0x8403, 0x8404];
pub const WALL: u32 = 0x8405;
pub const RIFLE_REF: u32 = 0x8501;
pub const AMMO_REF: u32 = 0x8502;
pub const REGISTER: u32 = 0x8601;
pub const LOCKER: u32 = 0x8602;
pub const RAIDERS: [(u32, [f32; 3]); 3] = [
    (0x8201, [-300.0, -100.0, FLOOR_Z]),
    (0x8202, [-250.0, 40.0, FLOOR_Z]),
    (0x8203, [320.0, -220.0, FLOOR_Z]),
];
pub const CLERK: (u32, [f32; 3]) = (0x8204, [60.0, -40.0, FLOOR_Z]);
pub const RAIDER_TEAM: i32 = 5;
pub const RAIDER_PACKET: i32 = 1;
/// Where the counter and the pool table stand (source rectangles).
pub const COUNTER_BOX: [f32; 4] = [-240.0, -175.0, -60.0, -125.0];
pub const POOL_BOX: [f32; 4] = [110.0, -190.0, 210.0, -110.0];

/// The item keys of the catalogue.
pub const PISTOL: &str = "synthetic.esm:00f001";
pub const RIFLE: &str = "synthetic.esm:00f002";
pub const AMMO_10MM: &str = "synthetic.esm:00f003";
pub const AMMO_308: &str = "synthetic.esm:00f004";
pub const LEATHER: &str = "synthetic.esm:00f005";
pub const STIMPAK: &str = "synthetic.esm:00f006";
pub const CAPS: &str = "synthetic.esm:00f007";
pub const STORE_KEY: &str = "synthetic.esm:00f008";

/// Critter art: 0 raiders (with pistol and rifle files), 1 the clerk, 2
/// the leather armour's figure.
pub fn critter_art() -> Vec<CritterArt> {
    let mut raider = sprite_set("raider", 7);
    for suffix in ["ha", "ja"] {
        raider
            .frms
            .insert(suffix.into(), critter_frm_bytes(4, 0.0, 8));
    }
    for suffix in ["hb", "jb"] {
        raider
            .frms
            .insert(suffix.into(), critter_frm_bytes(8, 0.125, 8));
    }
    vec![
        raider,
        sprite_set("clerk", 9),
        sprite_set("leather figure", 11),
    ]
}

/// The full synthetic base plus an inventory art list.
pub fn mart_base() -> BTreeMap<String, Vec<u8>> {
    let mut base = full_base();
    base.insert(
        "art/inven/inven.lst".into(),
        b"binv0000.frm\nbinv0001.frm\nbinv0002.frm\n".to_vec(),
    );
    base
}

/// An image whose bottom row is on its hex centre (a wall's foot).
pub fn standing(
    category: ArtCategory,
    width: u32,
    height: u32,
    value: u8,
    source: &str,
) -> ArtImage {
    ArtImage {
        category,
        width,
        height,
        pixels: vec![value; (width * height) as usize],
        shift: [0, 0],
        source: source.into(),
    }
}

/// The hexes along source `y` from `x0` to `x1`, sampled finely, in order.
pub fn hexes_along(frame: &map_builder::walk::Frame, y: f32, x0: f32, x1: f32) -> Vec<i32> {
    let mut out: Vec<i32> = Vec::new();
    let mut x = x0;
    while x <= x1 {
        if let Some(hex) = frame.hex_of_source(f64::from(x), f64::from(y))
            && !out.contains(&hex)
        {
            out.push(hex);
        }
        x += 4.0;
    }
    out
}

fn rectangle_corners([x0, y0, x1, y1]: [f32; 4]) -> Vec<[f32; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// The furniture footprints of the counter and the pool table.
pub fn footprints() -> BTreeMap<String, Vec<[f32; 2]>> {
    [
        (form(COUNTER), rectangle_corners(COUNTER_BOX)),
        (form(POOL_TABLE), rectangle_corners(POOL_BOX)),
    ]
    .into()
}

/// The mart interior cell: one floor navmesh over both rooms, its doors,
/// its people.
pub fn mart_cell(outside: &CellDoc, back: [f32; 2]) -> CellDoc {
    let mut front = teleport_door(FRONT_DOOR, FRONT_AT, LOT_DOOR, 0, None, Some((0, 0)));
    let teleport = front.teleport.as_mut().unwrap();
    teleport.cell = Some(CellRef {
        form: outside.form.clone(),
        editor_id: outside.editor_id.clone(),
        location: outside.location.clone(),
    });
    teleport.position = [back[0], back[1], 100.0];
    let mut references = vec![front];
    for (id, position) in RAIDERS.iter().chain(std::iter::once(&CLERK)) {
        references.push(resident(*id, *position));
    }
    references.sort_by(|a, b| a.form.cmp(&b.form));
    CellDoc {
        schema_version: mir::SCHEMA_VERSION,
        form: FormKey::new(PLUGIN, MART),
        editor_id: Some("SynthMart".into()),
        name: Some("Synthetic Mart".into()),
        location: CellLocation::Interior,
        flags: 1,
        water_height: None,
        land: None,
        references,
        navmeshes: vec![navmesh(
            0x8301,
            &rectangle(-400.0, -300.0, 400.0, 300.0),
            FLOOR_Z,
        )],
    }
}

/// Both scenes of the mini mart.
pub struct Mart {
    pub lot: Scene,
    pub mart: Scene,
    pub base: BTreeMap<String, Vec<u8>>,
    pub critter_art: Vec<CritterArt>,
}

pub fn mart() -> Mart {
    // The lot: the town, with a door into the mart.
    let mut lot = town();
    let frame = lot.frame();
    let door_hex = hex_at(&frame, 48.0, 48.0);
    let [x, y] = hex_point(&frame, door_hex);
    let centre = super::scenes::centre_cell(&lot);
    let mut door = teleport_door(
        LOT_DOOR,
        [x, y, 100.0],
        FRONT_DOOR,
        MART,
        Some("SynthMart"),
        None,
    );
    door.teleport.as_mut().unwrap().position = [0.0, -220.0, FLOOR_Z];
    lot.cells[centre].references.push(door);
    lot.cells[centre]
        .references
        .sort_by(|a, b| a.form.cmp(&b.form));
    lot.placements
        .push(placement(4, door_hex, Role::Door, true, &form(LOT_DOOR)));
    lot.base = mart_base();
    lot.name = "lot".into();
    let back = hex_point(
        &frame,
        vault_formats::HexGrid::step(door_hex, 2, 3).unwrap(),
    );
    let outside = lot.cells[centre].clone();

    // The store.
    let projection = projection();
    let window = MapWindow::from_center([0.0, 0.0], projection.tile_units());
    let frame = super::frame(&window, &projection);
    let at = |x: f32, y: f32| frame.hex_of_source(f64::from(x), f64::from(y)).unwrap();
    let art = vec![
        image(ArtCategory::Scenery, 24, 50, 70, "mart door"),
        image(ArtCategory::Walls, 32, 60, 30, "mart wall"),
        standing(ArtCategory::Walls, 32, 40, 33, "counter strip"),
        standing(ArtCategory::Scenery, 64, 30, 44, "pool table"),
        image(ArtCategory::Scenery, 20, 30, 41, "shelf"),
        image(ArtCategory::Items, 30, 8, 52, "rifle on the floor"),
        image(ArtCategory::Items, 10, 8, 53, "ammo box"),
        image(ArtCategory::Items, 18, 22, 54, "register"),
        image(ArtCategory::Items, 20, 40, 55, "locker"),
        image(ArtCategory::Scenery, 24, 50, 71, "storeroom door"),
    ];
    let store_door = at(0.0, WALL_Y);
    let mut placements = vec![
        placement(
            0,
            at(FRONT_AT[0], FRONT_AT[1]),
            Role::Door,
            true,
            &form(FRONT_DOOR),
        ),
        placement(9, store_door, Role::Door, true, &form(STORE_DOOR)),
    ];
    for hex in hexes_along(&frame, WALL_Y, -460.0, 460.0) {
        if hex != store_door {
            placements.push(placement(1, hex, Role::WallStrip, true, &form(WALL)));
        }
    }
    // The counter: strips the slicer left non-blocking over open hexes.
    let [cx0, cy0, cx1, cy1] = COUNTER_BOX;
    for hex in hexes_along(&frame, (cy0 + cy1) / 2.0, cx0 + 10.0, cx1 - 10.0) {
        placements.push(placement(2, hex, Role::WallStrip, false, &form(COUNTER)));
    }
    let [px0, py0, px1, py1] = POOL_BOX;
    placements.push(placement(
        3,
        at((px0 + px1) / 2.0, (py0 + py1) / 2.0),
        Role::Scenery,
        true,
        &form(POOL_TABLE),
    ));
    placements.push(placement(
        4,
        at(-350.0, 250.0),
        Role::Scenery,
        true,
        &form(SHELVES[0]),
    ));
    placements.push(placement(
        4,
        at(350.0, -40.0),
        Role::Scenery,
        true,
        &form(SHELVES[1]),
    ));
    placements.push(placement(
        5,
        at(-100.0, -250.0),
        Role::Item,
        false,
        &form(RIFLE_REF),
    ));
    placements.push(placement(
        6,
        at(-60.0, -250.0),
        Role::Item,
        false,
        &form(AMMO_REF),
    ));
    placements.push(placement(
        7,
        at(250.0, -60.0),
        Role::Container,
        true,
        &form(REGISTER),
    ));
    placements.push(placement(
        8,
        at(250.0, 220.0),
        Role::Container,
        true,
        &form(LOCKER),
    ));
    placements.sort_by_key(|p| p.hex);
    let critters = RAIDERS
        .iter()
        .enumerate()
        .map(|(i, (id, p))| critter(0, &form(*id), &format!("Raider {i}"), [p[0], p[1]], 0.0))
        .chain(std::iter::once(critter(
            1,
            &form(CLERK.0),
            "Clerk",
            [CLERK.1[0], CLERK.1[1]],
            0.0,
        )))
        .collect();
    let mart = Scene {
        projection,
        window,
        sampler: GroundSampler::default(),
        tiles: vec![vec![10; 80 * 36]],
        squares: vec![0; 10_000],
        art,
        placements,
        blocked: Vec::new(),
        cells: vec![mart_cell(&outside, back)],
        base: mart_base(),
        critter_art: Vec::new(),
        critters,
        options: BuildOptions {
            interior: Some(InteriorBuild::new(FLOOR_Z)),
            ..BuildOptions::default()
        },
        name: "mart".into(),
        number: None,
    };
    Mart {
        lot,
        mart,
        base: mart_base(),
        critter_art: critter_art(),
    }
}

/// A small item sprite of its own.
fn sprite(value: u8, source: &str) -> GroundArt {
    GroundArt::Sprite(image(ArtCategory::Items, 12, 10, value, source))
}

fn def(kind: ItemKind, name: &str, ground: GroundArt) -> ItemDef {
    ItemDef {
        ground,
        ..ItemDef::new(kind, ProtoText::new(name, &format!("A {name}.")))
    }
}

fn stats(size: i32, weight: i32, cost: i32) -> ItemStats {
    ItemStats {
        size,
        weight,
        cost,
        ..ItemStats::default()
    }
}

/// A raider's fighter spec.
pub fn raider(hit_points: i32) -> FighterSpec {
    let mut spec = FighterSpec::new(CritterSpec::human(hit_points, RAIDER_PACKET, RAIDER_TEAM));
    spec.critter.special = [6, 5, 6, 3, 4, 6, 4];
    spec.critter.experience = 50;
    spec.skills[emit::Skill::SmallGuns.index()] = Some(60);
    spec
}

/// The mart's catalogue.
pub fn catalogue() -> Catalogue {
    let mut pistol = WeaponSpec::new(
        WeaponCode::Pistol,
        [5, 12],
        DamageType::Normal,
        AttackMode::Single,
        5,
        20,
    );
    pistol.capacity = 12;
    pistol.stats = stats(2, 3, 250);
    let mut rifle = WeaponSpec::new(
        WeaponCode::Shotgun,
        [8, 20],
        DamageType::Normal,
        AttackMode::Single,
        5,
        30,
    );
    rifle.capacity = 5;
    rifle.two_handed = true;
    rifle.stats = stats(5, 9, 500);
    let ammo = |caliber, quantity| AmmoSpec {
        stats: stats(1, 1, 20),
        caliber,
        quantity,
        ac_modifier: 0,
        dr_modifier: 0,
        multiplier: 1,
        divisor: 1,
    };
    let leather = ArmorSpec {
        stats: stats(10, 15, 700),
        armor_class: 15,
        resistances: [25, 20, 20, 10, 0, 20, 0],
        thresholds: [2, 0, 0, 0, 0, 0, 0],
        perk: None,
        male_art: None,
        female_art: None,
    };
    let mut stimpak = DrugSpec::instant(drug_stat::CURRENT_HP, 10);
    stimpak.stats = stats(0, 0, 175);
    let mut items = BTreeMap::new();
    let mut pistol_def = def(
        ItemKind::Weapon(pistol),
        "10mm pistol",
        sprite(60, "pistol"),
    );
    pistol_def.ammo = Some(AMMO_10MM.into());
    pistol_def.icon = Some(ItemIcon::Indexed {
        width: 4,
        height: 3,
        pixels: vec![0, 61, 61, 0, 61, 61, 61, 61, 0, 61, 0, 0],
    });
    items.insert(PISTOL.to_owned(), pistol_def);
    let mut rifle_def = def(
        ItemKind::Weapon(rifle),
        "Hunting rifle",
        GroundArt::FirstPlacement,
    );
    rifle_def.ammo = Some(AMMO_308.into());
    items.insert(RIFLE.to_owned(), rifle_def);
    items.insert(
        AMMO_10MM.to_owned(),
        def(
            ItemKind::Ammo(ammo(101, 24)),
            "10mm rounds",
            GroundArt::Reference(form(AMMO_REF)),
        ),
    );
    items.insert(
        AMMO_308.to_owned(),
        def(
            ItemKind::Ammo(ammo(102, 20)),
            ".308 rounds",
            sprite(62, "308"),
        ),
    );
    let mut leather_def = def(
        ItemKind::Armor(leather),
        "Leather armor",
        sprite(63, "leather"),
    );
    leather_def.figure = [Some(2), Some(2)];
    items.insert(LEATHER.to_owned(), leather_def);
    items.insert(
        STIMPAK.to_owned(),
        def(ItemKind::Drug(stimpak), "Stimpak", sprite(64, "stimpak")),
    );
    let mut caps = def(
        ItemKind::Misc(MiscSpec {
            stats: stats(0, 0, 1),
            ..MiscSpec::default()
        }),
        "Bottle caps",
        sprite(65, "caps"),
    );
    caps.icon = Some(ItemIcon::Rgba {
        width: 2,
        height: 2,
        rgba: vec![
            200, 180, 20, 255, 200, 180, 20, 255, 0, 0, 0, 0, 190, 170, 30, 255,
        ],
    });
    items.insert(CAPS.to_owned(), caps);
    items.insert(
        STORE_KEY.to_owned(),
        def(
            ItemKind::Key(KeySpec {
                stats: stats(0, 0, 0),
                code: 0,
            }),
            "Storeroom key",
            sprite(66, "key"),
        ),
    );

    let mut placed = BTreeMap::new();
    placed.insert(
        form(RIFLE_REF),
        PlacedLoot::Item {
            item: RIFLE.into(),
            count: 1,
            state: ItemState::default(),
        },
    );
    placed.insert(
        form(AMMO_REF),
        PlacedLoot::Item {
            item: AMMO_10MM.into(),
            count: 2,
            state: ItemState {
                rounds: Some(10),
                ..ItemState::default()
            },
        },
    );
    placed.insert(
        form(REGISTER),
        PlacedLoot::Container {
            text: Some(ProtoText::new("Cash register", "An old cash register.")),
            capacity: Some(60),
            lock: LockState::default(),
            contents: vec![
                Held::new(PISTOL, 1),
                Held::new(AMMO_10MM, 2),
                Held::new(STIMPAK, 3),
                Held::new(CAPS, 50),
                Held::new(STORE_KEY, 1),
            ],
        },
    );
    placed.insert(
        form(LOCKER),
        PlacedLoot::Container {
            text: Some(ProtoText::new("Storeroom locker", "A dented locker.")),
            capacity: None,
            lock: LockState::LOCKED,
            contents: vec![Held::new(AMMO_308, 2)],
        },
    );
    placed.insert(
        form(STORE_DOOR),
        PlacedLoot::Door {
            text: Some(ProtoText::new("Storeroom door", "Staff only.")),
            lock: LockState::LOCKED,
            key: Some(STORE_KEY.into()),
        },
    );
    placed.insert(
        form(COUNTER),
        PlacedLoot::Named(ProtoText::new("Store counter", "A long counter.")),
    );
    placed.insert(
        form(POOL_TABLE),
        PlacedLoot::Named(ProtoText::new("Pool table", "Felt worn through.")),
    );
    // A reference no map places: reported, not refused.
    placed.insert(
        "synthetic.esm:00dead".into(),
        PlacedLoot::Named(ProtoText::new("Gone", "")),
    );

    let mut critters = BTreeMap::new();
    critters.insert(
        form(RAIDERS[0].0),
        CritterLoot {
            fighter: Some(raider(30)),
            right_hand: Some(Held::new(PISTOL, 1)),
            worn: Some(Held::new(LEATHER, 1)),
            carried: vec![Held::new(AMMO_10MM, 1), Held::new(STIMPAK, 1)],
            ..CritterLoot::default()
        },
    );
    critters.insert(
        form(RAIDERS[1].0),
        CritterLoot {
            fighter: Some(raider(28)),
            right_hand: Some(Held::new(RIFLE, 1)),
            carried: vec![Held::new(CAPS, 12)],
            ..CritterLoot::default()
        },
    );
    critters.insert(
        form(RAIDERS[2].0),
        CritterLoot {
            fighter: Some(raider(25)),
            ..CritterLoot::default()
        },
    );
    Catalogue {
        items,
        placed,
        critters,
        icon_palette: vec![(0, [0, 0, 0]), (90, [200, 180, 20]), (91, [20, 20, 200])],
    }
}

/// One elevation of a scene.
pub fn elevation(scene: &Scene) -> ElevationInput<'_> {
    ElevationInput {
        sampler: &scene.sampler,
        ground: GroundTiles {
            tiles: &scene.tiles,
            squares: &scene.squares,
        },
        art: &scene.art,
        placements: &scene.placements,
        blocked_hexes: &scene.blocked,
        interior: scene.options.interior.clone(),
        level: None,
    }
}

/// A scene as one map of a layer.
pub fn map_of(scene: &Scene) -> MapInput<'_> {
    let mut options = scene.options.clone();
    options.interior = None;
    MapInput {
        map_name: &scene.name,
        map_number: scene.number,
        window: scene.window,
        projection: &scene.projection,
        elevations: vec![elevation(scene)],
        entrance_elevation: None,
        cells: &scene.cells,
        critters: &scene.critters,
        options,
    }
}

/// The mart as a linked layer of two maps, with `catalogue`.
pub fn layer<'a>(mart: &'a Mart, catalogue: Option<Arc<Catalogue>>) -> LayerInput<'a> {
    LayerInput {
        layer_id: "mini-mart",
        layer_name: "Mini mart",
        base_files: &mart.base,
        critter_art: &mart.critter_art,
        maps: vec![map_of(&mart.lot), map_of(&mart.mart)],
        stairs: Vec::new(),
        options: LayerOptions {
            catalogue,
            ..LayerOptions::default()
        },
    }
}
