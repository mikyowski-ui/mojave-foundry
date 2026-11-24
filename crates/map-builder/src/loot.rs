//! Items, loot and fighters: the optional catalogue a build takes
//! ([`crate::BuildOptions::catalogue`], [`crate::LayerOptions::catalogue`])
//! and what it reports.
//!
//! Without a catalogue nothing here runs and every build is byte for byte
//! what it was (`tests/golden.rs`). With one:
//!
//! - **Item prototypes.** One per [`ItemDef`] (not per placement), in
//!   `proto/items`, numbered in sorted key order: the first definition
//!   takes the list position after the base list, the next one the
//!   position after it, and so on, before any other item prototype of the
//!   build. References between definitions are resolved by key: a weapon's
//!   default ammunition ([`ItemDef::ammo`]) gives its ammunition PID and,
//!   when the weapon's calibre is 0, its calibre (a different non-zero
//!   calibre is an input error); a miscellaneous item's
//!   [`ItemDef::ammo`] is its power source; an armour's
//!   [`ItemDef::figure`] names the critter sprite sets its wearer takes; a
//!   key whose code is 0 takes its PID's list position as its code. Names
//!   and descriptions go to `pro_item.msg` ([`emit::describe`]).
//! - **Art.** The ground art ([`GroundArt`]) is a placed reference's
//!   sprite or one supplied, in the items list; the inventory icon
//!   ([`ItemIcon`], or the ground art when none is given) is reduced into
//!   90 × 61 ([`emit::icon::INVENTORY_ICON_MAX`]) and written to
//!   `art/inven` (one FRM per distinct icon). Both lists are checked
//!   against the 4,096-entry budget with the other art, before anything is
//!   written ([`crate::BuildError::FidBudget`]).
//! - **Placed references** ([`PlacedLoot`], by form key): an item lying on
//!   the map ([`emit::objects::item_object`], `count` objects on its hex),
//!   a container with its capacity, lock and contents
//!   ([`emit::objects::container_object`]), a door with its lock and key
//!   ([`emit::objects::door_object`]; the key code is the door
//!   prototype's), or a real name and description for scenery, walls and
//!   floor pieces. A container or door named here gets a prototype of its
//!   own per distinct (art, name, capacity or key), so its look-at text is
//!   its name rather than its model path.
//! - **Critters** ([`CritterLoot`], by the critter's reference): a
//!   [`emit::FighterSpec`] for the prototype ([`emit::fighter_prototype`];
//!   without one today's bare [`emit::CritterSpec`]), the AI packet and
//!   team, and a loadout by item keys ([`emit::objects::critter_with_loadout`]).
//!   A weapon in the right hand changes the critter's art to the weapon's
//!   code, so its sprite set must hold that code's stand and walk files; a
//!   set that does not carries the weapon instead (reported in
//!   [`LootReport::skipped`]).
//!
//! Every object is built through one [`emit::ProtoSet`], so the written
//! maps read back through `MapFile::parse_for`; [`crate::verify_loot`]
//! checks the result. Inventories respect the format's limits: quantities
//! 1 to `i32::MAX`, contents that fit below a container's capacity, and at
//! most 100,000 records in a map counting every inventory entry; anything
//! else is an input error naming the reference.

use std::collections::BTreeMap;

use emit::objects::{ItemState, LockState};
use emit::proto::Material;
use emit::{AmmoSpec, ArmorSpec, DrugSpec, FighterSpec, KeySpec, MiscSpec, ProtoText, WeaponSpec};

use crate::ArtImage;

/// What an item is, as the `emit` constructor that builds it takes it.
/// PIDs inside a spec (a weapon's `ammo`, a miscellaneous item's power
/// source, an armour's figures) are filled by the builder from
/// [`ItemDef::ammo`] and [`ItemDef::figure`]; give them as `None`.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Weapon(WeaponSpec),
    Armor(ArmorSpec),
    Ammo(AmmoSpec),
    Drug(DrugSpec),
    Key(KeySpec),
    Misc(MiscSpec),
}

impl ItemKind {
    /// The word used in reports: `weapon`, `armor`, `ammo`, `drug`, `key`,
    /// `misc`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Weapon(_) => "weapon",
            Self::Armor(_) => "armor",
            Self::Ammo(_) => "ammo",
            Self::Drug(_) => "drug",
            Self::Key(_) => "key",
            Self::Misc(_) => "misc",
        }
    }
}

/// An inventory picture (index 0, or alpha under 128, transparent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemIcon {
    /// Palette indices, row-major.
    Indexed {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    /// RGBA8, row-major, mapped to the nearest colour of
    /// [`Catalogue::icon_palette`] (squared RGB distance, lowest index on a
    /// tie). Quantizing with the converter's own palette map and passing
    /// [`ItemIcon::Indexed`] gives a better picture.
    Rgba {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

/// Where an item's ground art (its prototype's FID, `art/items`) comes
/// from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GroundArt {
    /// The sprite of the first placed reference ([`PlacedLoot::Item`])
    /// naming the item, in map, elevation and placement order. An item no
    /// reference places needs one of the others.
    #[default]
    FirstPlacement,
    /// The sprite of this placed reference (a form key with a
    /// [`crate::Role::Item`] placement).
    Reference(String),
    /// A sprite of its own ([`crate::ArtCategory::Items`]), for items that
    /// exist only in containers and inventories.
    Sprite(ArtImage),
}

/// One item definition, keyed in [`Catalogue::items`] by a stable string
/// (the source base form key).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemDef {
    pub kind: ItemKind,
    /// Name and description, already folded ([`ProtoText::new`]).
    pub text: ProtoText,
    /// The inventory icon; `None` reduces the ground art.
    pub icon: Option<ItemIcon>,
    pub ground: GroundArt,
    /// A weapon's default ammunition, or a miscellaneous item's power
    /// source: the key of an [`ItemKind::Ammo`] definition. Refused on any
    /// other kind.
    pub ammo: Option<String>,
    /// An armour's wearer figure, male then female: indices into the
    /// layer's critter sprite sets ([`crate::BuildInput::critter_art`]),
    /// which the layer then holds whether or not a critter shows them.
    /// Refused on any other kind.
    pub figure: [Option<usize>; 2],
    pub material: Material,
}

impl ItemDef {
    /// `kind` named by `text`: ground art from its first placement, the
    /// icon from the ground art, no ammunition, no figure, stone.
    pub fn new(kind: ItemKind, text: ProtoText) -> Self {
        Self {
            kind,
            text,
            icon: None,
            ground: GroundArt::FirstPlacement,
            ammo: None,
            figure: [None, None],
            material: Material::default(),
        }
    }
}

/// An item held in an inventory: the definition's key, how many (one
/// representative record stands for them all), and its instance words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub item: String,
    pub quantity: u32,
    pub state: ItemState,
}

impl Held {
    /// `quantity` of `item` with the prototype's instance words.
    pub fn new(item: &str, quantity: u32) -> Self {
        Self {
            item: item.to_owned(),
            quantity,
            state: ItemState::default(),
        }
    }
}

/// What a placed reference ([`crate::Placement::reference`]) becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacedLoot {
    /// An item lying on the map (a [`crate::Role::Item`] placement):
    /// `count` objects of the definition `item` on its hex (ammunition:
    /// `count` packs), each with `state`.
    Item {
        item: String,
        count: u32,
        state: ItemState,
    },
    /// A container (a [`crate::Role::Container`] placement).
    Container {
        /// Look-at name and description; `None` keeps the generated one.
        text: Option<ProtoText>,
        /// Size units; `None` takes [`crate::BuildOptions::container_capacity`].
        capacity: Option<i32>,
        lock: LockState,
        contents: Vec<Held>,
    },
    /// A door (a [`crate::Role::Door`] placement).
    Door {
        text: Option<ProtoText>,
        lock: LockState,
        /// The key's item key ([`ItemKind::Key`]): its code becomes the
        /// door prototype's key code.
        key: Option<String>,
    },
    /// A real name for scenery, wall strips and floor pieces.
    Named(ProtoText),
}

/// A critter's fight and kit (by its [`crate::CritterPlacement::reference`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CritterLoot {
    /// The prototype's figures ([`emit::rebalance::fighter_spec`]); `None`
    /// keeps today's bare critter (the placement's hit points and sex).
    pub fighter: Option<FighterSpec>,
    /// The AI packet and team; `None` takes the fighter's, else the
    /// layer's ([`crate::LayerOptions::critter_ai_packet`],
    /// [`crate::LayerOptions::critter_team`]). A team must be above 0
    /// (0 is the player's); a packet a supplied base `data/ai.txt` lacks is
    /// an input error.
    pub ai_packet: Option<i32>,
    pub team: Option<i32>,
    /// Held in the right hand (the hand non-player critters use), the left
    /// hand, worn (armour), and carried; hands and worn hold one each.
    pub right_hand: Option<Held>,
    pub left_hand: Option<Held>,
    pub worn: Option<Held>,
    pub carried: Vec<Held>,
}

/// Everything the builder needs about a layer's items (crate notes,
/// "Loot"). Keys are stable strings: item keys are the converter's own
/// (the source base form key), reference keys are form keys.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Catalogue {
    pub items: BTreeMap<String, ItemDef>,
    /// By placed reference.
    pub placed: BTreeMap<String, PlacedLoot>,
    /// By critter reference.
    pub critters: BTreeMap<String, CritterLoot>,
    /// The colours [`ItemIcon::Rgba`] maps to: palette index and RGB, index
    /// 0 never (it is transparent). Needed only by RGBA icons.
    pub icon_palette: Vec<(u8, [u8; 3])>,
}

/// A catalogue entry the build left out, or changed, and why.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LootSkip {
    /// `item <key>`, `reference <form>`, `critter <form>`.
    pub entry: String,
    pub reason: String,
}

/// One item prototype the catalogue made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemPrototypeReport {
    pub key: String,
    pub pid: u32,
    /// `proto/items/<file>`.
    pub file: String,
    /// [`ItemKind::label`].
    pub kind: &'static str,
    /// The folded name, as written to `pro_item.msg`.
    pub name: String,
    /// The prototype's art and inventory art.
    pub ground_fid: u32,
    pub inventory_fid: u32,
}

/// What one map's objects got from the catalogue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapLoot {
    /// `maps/<stem>.map`.
    pub file: String,
    /// Item objects lying on the map.
    pub placed_items: usize,
    /// Containers of the catalogue, those holding anything, and their
    /// inventory entries.
    pub containers: usize,
    pub filled_containers: usize,
    pub container_stacks: usize,
    /// Doors and containers locked, and jammed.
    pub locked_doors: usize,
    pub locked_containers: usize,
    pub jammed: usize,
    /// Doors with a key code.
    pub keyed_doors: usize,
    /// Critters with a fighter spec, with anything in a hand, and with a
    /// weapon in the right hand.
    pub fighters: usize,
    pub equipped_critters: usize,
    pub armed_critters: usize,
    /// Inventory entries of critters.
    pub critter_stacks: usize,
    /// Placements whose prototype carries a real name (scenery, walls,
    /// floor pieces, doors, containers).
    pub named_placements: usize,
    /// Records in inventories (every entry counted once).
    pub inventory_records: usize,
}

impl MapLoot {
    fn add(&mut self, other: &MapLoot) {
        self.placed_items += other.placed_items;
        self.containers += other.containers;
        self.filled_containers += other.filled_containers;
        self.container_stacks += other.container_stacks;
        self.locked_doors += other.locked_doors;
        self.locked_containers += other.locked_containers;
        self.jammed += other.jammed;
        self.keyed_doors += other.keyed_doors;
        self.fighters += other.fighters;
        self.equipped_critters += other.equipped_critters;
        self.armed_critters += other.armed_critters;
        self.critter_stacks += other.critter_stacks;
        self.named_placements += other.named_placements;
        self.inventory_records += other.inventory_records;
    }
}

/// What a catalogue made, over the whole layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LootReport {
    /// Item prototypes by [`ItemKind::label`].
    pub item_prototypes: BTreeMap<String, usize>,
    /// Every one, in PID order.
    pub items: Vec<ItemPrototypeReport>,
    /// Scenery, wall, door and container prototypes made for a named or
    /// keyed reference.
    pub named_prototypes: usize,
    /// Fighter and kitted critter prototypes.
    pub critter_prototypes: usize,
    /// New `art/inven` entries.
    pub inventory_art: usize,
    /// Supplied ground sprites written ([`GroundArt::Sprite`]).
    pub ground_sprites: usize,
    /// Per map, in layer order.
    pub maps: Vec<MapLoot>,
    /// Entries left out or changed, sorted.
    pub skipped: Vec<LootSkip>,
    /// [`crate::verify_loot`] over the written layer.
    pub check: crate::LootCheck,
}

impl LootReport {
    /// Every map's counts added up.
    pub fn totals(&self) -> MapLoot {
        let mut total = MapLoot {
            file: "all maps".into(),
            ..MapLoot::default()
        };
        for map in &self.maps {
            total.add(map);
        }
        total
    }

    /// Plain-language lines for the map report.
    pub fn loot_summary(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let kinds: Vec<String> = self
            .item_prototypes
            .iter()
            .map(|(kind, n)| format!("{n} {kind}"))
            .collect();
        lines.push(format!(
            "item prototypes: {} ({}); {} inventory icons, {} ground sprites of their own",
            self.items.len(),
            if kinds.is_empty() {
                "none".to_owned()
            } else {
                kinds.join(", ")
            },
            self.inventory_art,
            self.ground_sprites
        ));
        let describe = |m: &MapLoot| {
            format!(
                "{}: {} items on the ground; {} containers ({} filled, {} stacks, {} locked); {} \
                 locked doors ({} keyed); {} jammed; {} fighters, {} armed, {} with a hand item, \
                 {} carried stacks; {} named placements",
                m.file,
                m.placed_items,
                m.containers,
                m.filled_containers,
                m.container_stacks,
                m.locked_containers,
                m.locked_doors,
                m.keyed_doors,
                m.jammed,
                m.fighters,
                m.armed_critters,
                m.equipped_critters,
                m.critter_stacks,
                m.named_placements
            )
        };
        if self.maps.len() > 1 {
            lines.push(describe(&self.totals()));
        }
        for map in &self.maps {
            lines.push(describe(map));
        }
        lines.push(format!(
            "prototypes: {} named or keyed scenery, door and container, {} critter",
            self.named_prototypes, self.critter_prototypes
        ));
        let c = &self.check;
        lines.push(format!(
            "check: {} item prototypes, {} weapons with ammunition agree, {} inventory records \
             resolve, {} message pairs present, {} maps round-trip",
            c.item_prototypes, c.weapons_with_ammo, c.inventory_records, c.messages, c.maps
        ));
        for skip in &self.skipped {
            lines.push(format!("skipped {}: {}", skip.entry, skip.reason));
        }
        lines
    }
}
