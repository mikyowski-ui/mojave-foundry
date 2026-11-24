//! Prototypes: constructors with documented defaults, the prototype lists
//! and PID numbering, and the name/description message entries.
//!
//! Layouts are `vault_formats::pro::Prototype`'s (SPEC-003 §2.1-§2.6; a
//! Fallout 2 non-critter prototype is laid out the same, and a Fallout 2
//! critter prototype adds one last word, 416 bytes, SPEC-016 §3.1).
//! A PID is `category × 2^24 + n`, where `n` is the prototype's one-based
//! line in `proto/<category>/<category>.lst`; the catalog
//! (`LocalPrototypeCatalog`) reads that line's file name and opens
//! `proto/<category>/<name>`, whose stored PID must equal the request.
//!
//! The defaults below are this crate's choices, not format rules: light
//! radius and intensity 0, no script (-1), extended flags 0 unless stated,
//! the uninterpreted stored byte 0 (scenery and items use it as their sound
//! letter; pass one where it matters).

use crate::{EmitError, lst, msg::MsgEntry};
use vault_formats::pro::{
    BasicFlags, Category, ContainerFlags, Critter, CritterFlags, ExtendedFlags, Item, ItemData,
    Light, LockFlags, PrototypeData, Scenery, SceneryData,
};
use vault_formats::{Fid, LstKind, LstTable, Prototype};

/// The largest one-based list position a PID can hold (24 bits).
pub const PID_INDEX_CAP: u32 = 0x00ff_ffff;
/// The exit-grid PIDs are miscellaneous prototypes 16 to 23
/// (SPEC-013 §3.22, SPEC-003 §3.3).
pub const EXIT_GRID_PIDS: std::ops::RangeInclusive<u32> = 0x0500_0010..=0x0500_0017;
/// The script word for "no script".
pub const NO_SCRIPT: i32 = -1;
/// The message base for "no message".
pub const NO_MESSAGE: i32 = -1;
/// The PID word for "no prototype".
pub const NO_PID: i32 = -1;

/// Art categories (the FID's category field) each prototype kind draws
/// from.
pub(crate) const ART_ITEMS: u8 = 0;
pub(crate) const ART_CRITTERS: u8 = 1;
pub(crate) const ART_SCENERY: u8 = 2;
const ART_WALLS: u8 = 3;
const ART_MISC: u8 = 5;
pub(crate) const ART_INVENTORY: u8 = 7;

/// SPEC-003 §2.3: materials 0 to 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum Material {
    Glass = 0,
    Metal = 1,
    Plastic = 2,
    Wood = 3,
    Dirt = 4,
    #[default]
    Stone = 5,
    Cement = 6,
    Leather = 7,
}

/// The basic-flag bits a converted object usually needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectFlags {
    /// Blocks movement: clear means the `NONBLOCKING` bit (`0x10`).
    pub blocking: bool,
    /// Light passes through (`0x20000000`).
    pub light_through: bool,
    /// Projectiles pass through (`0x80000000`).
    pub shoot_through: bool,
    /// Drawn flat, under upright objects (`0x8`).
    pub flat: bool,
    /// Cannot be highlighted (`0x1000`).
    pub no_highlight: bool,
    /// Never cut away by the see-through circle around the player
    /// (`0x8000`); drawn normally otherwise.
    pub never_cut: bool,
}

impl ObjectFlags {
    /// Blocks movement, light and projectiles: a wall or a solid prop.
    pub const SOLID: Self = Self {
        blocking: true,
        light_through: false,
        shoot_through: false,
        flat: false,
        no_highlight: false,
        never_cut: false,
    };
    /// Blocks movement only: a fence or railing.
    pub const BARRIER: Self = Self {
        blocking: true,
        light_through: true,
        shoot_through: true,
        flat: false,
        no_highlight: false,
        never_cut: false,
    };
    /// Blocks nothing and lies flat: ground decals and markers.
    pub const DECAL: Self = Self {
        blocking: false,
        light_through: true,
        shoot_through: true,
        flat: true,
        no_highlight: true,
        never_cut: false,
    };
    /// A walking surface above the terrain (a porch floor): blocks nothing,
    /// lies flat under whoever stands on it, and stays whole inside the
    /// see-through circle.
    pub const FLOOR: Self = Self {
        blocking: false,
        light_through: true,
        shoot_through: true,
        flat: true,
        no_highlight: true,
        never_cut: true,
    };

    /// The basic flag word.
    pub fn bits(self) -> BasicFlags {
        let mut bits = 0;
        if !self.blocking {
            bits |= BasicFlags::NONBLOCKING.0;
        }
        if self.light_through {
            bits |= BasicFlags::LIGHT_THROUGH.0;
        }
        if self.shoot_through {
            bits |= BasicFlags::PROJECTILE_THROUGH.0;
        }
        if self.flat {
            bits |= BasicFlags::FLAT.0;
        }
        if self.no_highlight {
            bits |= BasicFlags::NO_HIGHLIGHT.0;
        }
        if self.never_cut {
            bits |= BasicFlags::NO_TRANSLUCENCY.0;
        }
        BasicFlags(bits)
    }
}

/// What every constructor takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtoCommon {
    /// The full PID; its category must be the constructor's.
    pub pid: u32,
    /// The object's map art.
    pub fid: Fid,
    /// Name message number; the description is the next one.
    /// [`NO_MESSAGE`] for none.
    pub message_base: i32,
    pub flags: ObjectFlags,
    pub material: Material,
}

/// An item's inventory figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStats {
    /// Counted against a container's capacity.
    pub size: i32,
    pub weight: i32,
    pub cost: i32,
    /// Inventory art (category 7); `None` stores -1.
    pub inventory_art: Option<Fid>,
    /// The item's sound letter, the stored byte.
    pub sound: u8,
}

impl Default for ItemStats {
    fn default() -> Self {
        Self {
            size: 1,
            weight: 1,
            cost: 0,
            inventory_art: None,
            sound: 0,
        }
    }
}

/// A door's initial state: the prototype's door-state word (SPEC-003
/// §2.5, `SceneryData::Door::state`) and key code. A placed door carries
/// its own copy of the state word (`ObjectDetail::Door`), which is what the
/// engine reads and changes (SPEC-003 §2.11, SPEC-013 §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoorSpec {
    pub open: bool,
    pub locked: bool,
    /// The jammed bit `0x04000000`: every skill on the door is refused
    /// with `misc.msg` 2001 until the engine clears it (each midnight, a
    /// long absence, or a scripted open or close; SPEC-013 §3.3, §3.8).
    pub jammed: bool,
    /// The key code; -1 for none (this crate's default).
    pub key: i32,
    /// The door's sound letter, the stored byte.
    pub sound: u8,
}

impl Default for DoorSpec {
    fn default() -> Self {
        Self {
            open: false,
            locked: false,
            jammed: false,
            key: -1,
            sound: 0,
        }
    }
}

/// A PID from its category and one-based list position.
pub fn pid(category: Category, index: u32) -> Result<u32, EmitError> {
    if index == 0 || index > PID_INDEX_CAP {
        return Err(EmitError::Prototype(format!(
            "list position {index} is outside 1 to {PID_INDEX_CAP}"
        )));
    }
    Ok(((category as u32) << 24) | index)
}

/// The conventional prototype file name for list position `index`:
/// eight decimal digits and `.pro`.
pub fn prototype_file_name(index: u32) -> String {
    format!("{index:08}.pro")
}

/// `proto/<category>/<category>.lst`.
pub fn prototype_list_path(category: Category) -> String {
    let directory = category.directory();
    format!("proto/{directory}/{directory}.lst")
}

/// `proto/<category>/<file>`.
pub fn prototype_path(category: Category, file: &str) -> String {
    format!(
        "proto/{}/{}",
        category.directory(),
        file.to_ascii_lowercase()
    )
}

/// The game message file holding a category's prototype names and
/// descriptions: `text/<language>/game/pro_<stem>.msg` (the stems of
/// SPEC-024b's message file table).
pub fn message_file_path(category: Category, language: &str) -> String {
    let stem = match category {
        Category::Item => "pro_item",
        Category::Critter => "pro_crit",
        Category::Scenery => "pro_scen",
        Category::Wall => "pro_wall",
        Category::Tile => "pro_tile",
        Category::Misc => "pro_misc",
    };
    format!("text/{}/game/{stem}.msg", language.to_ascii_lowercase())
}

/// A message base of 100 × the list position: room for the name and
/// description, and the numbering SPEC-003's fixture uses (position 1,
/// base 100). `None` when it would overflow.
pub fn default_message_base(index: u32) -> Option<i32> {
    i32::try_from(index).ok()?.checked_mul(100)
}

/// The name (`base`) and description (`base + 1`) entries.
pub fn message_entries(
    message_base: i32,
    name: &[u8],
    description: &[u8],
) -> Result<[MsgEntry; 2], EmitError> {
    let next = message_base.checked_add(1).ok_or_else(|| {
        EmitError::Prototype(format!("message base {message_base} has no next number"))
    })?;
    if message_base < 0 {
        return Err(EmitError::Prototype(format!(
            "message base {message_base} is negative"
        )));
    }
    Ok([
        MsgEntry::new(message_base, name),
        MsgEntry::new(next, description),
    ])
}

pub(crate) fn check_common(
    common: &ProtoCommon,
    category: Category,
    art: u8,
) -> Result<(), EmitError> {
    let found = Category::from_pid(common.pid)?;
    if found != category {
        return Err(EmitError::Prototype(format!(
            "PID {:#010x} is a {found:?} PID, not {category:?}",
            common.pid
        )));
    }
    if common.pid & PID_INDEX_CAP == 0 {
        return Err(EmitError::Prototype(format!(
            "PID {:#010x} has list position 0",
            common.pid
        )));
    }
    if common.fid.category() != art {
        return Err(EmitError::Prototype(format!(
            "{category:?} art comes from art category {art}, and FID {:#010x} is category {}",
            common.fid.0,
            common.fid.category()
        )));
    }
    Ok(())
}

pub(crate) fn finish(
    common: &ProtoCommon,
    category: Category,
    extended: ExtendedFlags,
    data: PrototypeData,
) -> Prototype {
    let mut prototype = Prototype {
        pid: common.pid,
        category,
        message_base: common.message_base,
        art_id: common.fid.0,
        light: (category != Category::Tile).then_some(Light {
            radius: 0,
            intensity: 0,
        }),
        flags: common.flags.bits(),
        extended,
        script: (category != Category::Misc).then_some(NO_SCRIPT),
        data,
        consumed_len: 0,
        diagnostics: Vec::new(),
    };
    // What the parser reports for these bytes, so parse(write(p)) == p.
    prototype.consumed_len = prototype.write().len();
    prototype
}

/// A wall: art from `art/walls`, extended flags 0.
pub fn wall(common: ProtoCommon) -> Result<Prototype, EmitError> {
    check_common(&common, Category::Wall, ART_WALLS)?;
    Ok(finish(
        &common,
        Category::Wall,
        ExtendedFlags(0),
        PrototypeData::Wall {
            material: common.material as i32,
        },
    ))
}

/// Generic scenery (subtype 5): art from `art/scenery`, its retained word
/// 0, sound letter 0.
pub fn scenery(common: ProtoCommon) -> Result<Prototype, EmitError> {
    check_common(&common, Category::Scenery, ART_SCENERY)?;
    Ok(finish(
        &common,
        Category::Scenery,
        ExtendedFlags(0),
        PrototypeData::Scenery(Scenery {
            material: common.material as i32,
            stored_byte: 0,
            data: SceneryData::Generic(0),
        }),
    ))
}

/// A door (scenery subtype 0), with the use permission (`0x800`) set.
///
/// The key code is stored for scripts only: the engine has no lock
/// difficulty, no lockpick roll and no key test of its own; locks work
/// through the door's script (`obj_is_locked`, `obj_unlock`; SPEC-013
/// §3.4, §3.8). Without a script a locked door plays its locked sound and
/// opens anyway (SPEC-007 §3.11.4).
pub fn door(common: ProtoCommon, door: DoorSpec) -> Result<Prototype, EmitError> {
    check_common(&common, Category::Scenery, ART_SCENERY)?;
    let mut state = 0;
    if door.open {
        state |= LockFlags::OPEN.0;
    }
    if door.locked {
        state |= LockFlags::LOCKED.0;
    }
    if door.jammed {
        state |= LockFlags::JAMMED.0;
    }
    Ok(finish(
        &common,
        Category::Scenery,
        ExtendedFlags::USE,
        PrototypeData::Scenery(Scenery {
            material: common.material as i32,
            stored_byte: door.sound,
            data: SceneryData::Door {
                state: LockFlags(state),
                key: door.key,
            },
        }),
    ))
}

pub(crate) fn item(
    common: &ProtoCommon,
    stats: ItemStats,
    data: ItemData,
) -> Result<Item, EmitError> {
    if let Some(art) = stats.inventory_art
        && art.category() != ART_INVENTORY
    {
        return Err(EmitError::Prototype(format!(
            "inventory art comes from art category {ART_INVENTORY}, and FID {:#010x} is \
             category {}",
            art.0,
            art.category()
        )));
    }
    Ok(Item {
        material: common.material as i32,
        size: stats.size,
        weight: stats.weight,
        cost: stats.cost,
        inventory_art: stats.inventory_art.map_or(u32::MAX, |art| art.0),
        stored_byte: stats.sound,
        data,
    })
}

/// A miscellaneous item (item subtype 5) with no power source: power PID
/// -1, power type 0, charges 0. Items can be picked up by subtype.
pub fn misc_item(common: ProtoCommon, stats: ItemStats) -> Result<Prototype, EmitError> {
    check_common(&common, Category::Item, ART_ITEMS)?;
    let item = item(
        &common,
        stats,
        ItemData::Misc {
            power_pid: NO_PID,
            power_type: 0,
            charges: 0,
        },
    )?;
    Ok(finish(
        &common,
        Category::Item,
        ExtendedFlags(0),
        PrototypeData::Item(item),
    ))
}

/// A container (item subtype 1) holding up to `capacity` size units. It
/// can be used (opened) by subtype, and cannot be picked up: the
/// `0x8000` permission is left clear. `ground_reach` sets the container
/// flag `0x1`.
pub fn container(
    common: ProtoCommon,
    stats: ItemStats,
    capacity: i32,
    ground_reach: bool,
) -> Result<Prototype, EmitError> {
    check_common(&common, Category::Item, ART_ITEMS)?;
    if capacity < 0 {
        return Err(EmitError::Prototype(format!(
            "container capacity {capacity} is negative"
        )));
    }
    let flags = if ground_reach {
        ContainerFlags::GROUND_REACH
    } else {
        ContainerFlags(0)
    };
    let item = item(&common, stats, ItemData::Container { capacity, flags })?;
    Ok(finish(
        &common,
        Category::Item,
        ExtendedFlags(0),
        PrototypeData::Item(item),
    ))
}

/// An exit-grid marker: miscellaneous prototype `0x05000010 + variant`
/// (variant 0 to 7). A miscellaneous prototype has no script word and no
/// destination; the destination lives on each map object
/// (`ObjectDetail::ExitGrid`). Flags default to [`ObjectFlags::DECAL`].
/// The base game's lists normally hold these eight already; this builds
/// one for a list that does not.
pub fn exit_grid(variant: u8, fid: Fid, message_base: i32) -> Result<Prototype, EmitError> {
    let pid = EXIT_GRID_PIDS.start() + u32::from(variant);
    if !EXIT_GRID_PIDS.contains(&pid) {
        return Err(EmitError::Prototype(format!(
            "exit-grid variant {variant} is outside 0 to 7"
        )));
    }
    let common = ProtoCommon {
        pid,
        fid,
        message_base,
        flags: ObjectFlags::DECAL,
        material: Material::default(),
    };
    check_common(&common, Category::Misc, ART_MISC)?;
    Ok(finish(
        &common,
        Category::Misc,
        ExtendedFlags(0),
        PrototypeData::Misc,
    ))
}

/// Positions in a critter prototype's 35-word stat arrays (SPEC-003 §2.4).
pub mod stat {
    pub const STRENGTH: usize = 0;
    pub const PERCEPTION: usize = 1;
    pub const ENDURANCE: usize = 2;
    pub const CHARISMA: usize = 3;
    pub const INTELLIGENCE: usize = 4;
    pub const AGILITY: usize = 5;
    pub const LUCK: usize = 6;
    pub const MAX_HIT_POINTS: usize = 7;
    pub const MAX_ACTION_POINTS: usize = 8;
    pub const ARMOR_CLASS: usize = 9;
    pub const UNARMED_DAMAGE: usize = 10;
    pub const MELEE_DAMAGE: usize = 11;
    pub const CARRY_WEIGHT: usize = 12;
    pub const SEQUENCE: usize = 13;
    pub const HEALING_RATE: usize = 14;
    pub const CRITICAL_CHANCE: usize = 15;
    pub const BETTER_CRITICALS: usize = 16;
    /// 17 to 23 thresholds, 24 to 30 resistances, by damage type.
    pub const RADIATION_RESISTANCE: usize = 31;
    pub const POISON_RESISTANCE: usize = 32;
    pub const AGE: usize = 33;
    pub const GENDER: usize = 34;
    /// Words in each stat array.
    pub const COUNT: usize = 35;
}

/// Body type 0, biped (SPEC-003 §2.4); biped and robotic bodies open doors.
pub const BODY_BIPED: i32 = 0;
/// The highest body type, robotic.
pub const BODY_ROBOTIC: i32 = 2;
/// Kill categories 0 man and 1 woman (SPEC-003 §2.4); Fallout 2 numbers
/// them 0 to 18 (SPEC-018 §7.3).
pub const KILL_MAN: i32 = 0;
pub const KILL_WOMAN: i32 = 1;
pub const KILL_CATEGORY_MAX: i32 = 18;
/// The art word for "no talking head".
pub const NO_HEAD: u32 = u32::MAX;

/// A critter prototype's figures. [`CritterSpec::human`] gives the defaults
/// for a peaceful human; every field is public to change one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CritterSpec {
    /// Maximum hit points (stat 7), at least 1.
    pub hit_points: i32,
    /// A packet number of the game's `data/ai.txt`.
    pub ai_packet: i32,
    /// Team number; team 0 is the player's.
    pub team: i32,
    /// Strength to luck, each 1 to 10.
    pub special: [i32; 7],
    /// Gender stat 1 and, without `kill_category`, kill category woman.
    pub female: bool,
    /// Age, 16 to 35.
    pub age: i32,
    /// Experience the killer earns.
    pub experience: i32,
    /// Body type 0 to 2.
    pub body: i32,
    /// `None`: man or woman by `female`.
    pub kill_category: Option<i32>,
}

impl CritterSpec {
    /// A peaceful human: all seven primary stats 5, a male biped aged 25,
    /// no experience award, no script (set by [`critter_prototype`]).
    pub fn human(hit_points: i32, ai_packet: i32, team: i32) -> Self {
        Self {
            hit_points,
            ai_packet,
            team,
            special: [5; 7],
            female: false,
            age: 25,
            experience: 0,
            body: BODY_BIPED,
            kill_category: None,
        }
    }

    /// The kill category stored: the explicit one, else man or woman.
    pub fn kill_category(&self) -> i32 {
        self.kill_category
            .unwrap_or(if self.female { KILL_WOMAN } else { KILL_MAN })
    }

    /// The 35 base stats: the primary stats, the maximum hit points, and
    /// the other derived values recomputed from the primaries as SPEC-006
    /// §3.1 does (maximum AP `5 + AG / 2`, AC `AG`, melee damage
    /// `max(ST - 5, 1)`, carry weight `25 + 25 ST`, sequence `2 PE`,
    /// healing rate `max(EN / 3, 1)`, critical chance `LK`, better
    /// criticals 0, radiation resistance `2 EN`, poison resistance `5 EN`),
    /// so they hold whether or not the engine recomputes them. Unarmed
    /// damage, thresholds and resistances are 0; then age and gender.
    pub fn base_stats(&self) -> [i32; stat::COUNT] {
        let [st, pe, en, _, _, ag, lk] = self.special;
        let mut stats = [0; stat::COUNT];
        stats[..7].copy_from_slice(&self.special);
        stats[stat::MAX_HIT_POINTS] = self.hit_points;
        stats[stat::MAX_ACTION_POINTS] = 5 + ag / 2;
        stats[stat::ARMOR_CLASS] = ag;
        stats[stat::MELEE_DAMAGE] = (st - 5).max(1);
        stats[stat::CARRY_WEIGHT] = 25 + 25 * st;
        stats[stat::SEQUENCE] = 2 * pe;
        stats[stat::HEALING_RATE] = (en / 3).max(1);
        stats[stat::CRITICAL_CHANCE] = lk;
        stats[stat::RADIATION_RESISTANCE] = 2 * en;
        stats[stat::POISON_RESISTANCE] = 5 * en;
        stats[stat::AGE] = self.age;
        stats[stat::GENDER] = i32::from(self.female);
        stats
    }

    pub(crate) fn check(&self) -> Result<(), EmitError> {
        let bad = |message: String| Err(EmitError::Critter(message));
        if self.hit_points < 1 {
            return bad(format!(
                "maximum hit points {} must be at least 1",
                self.hit_points
            ));
        }
        if self.ai_packet < 0 || self.team < 0 {
            return bad(format!(
                "AI packet {} and team {} must not be negative",
                self.ai_packet, self.team
            ));
        }
        if let Some(value) = self.special.iter().find(|value| !(1..=10).contains(*value)) {
            return bad(format!("primary stat {value} is outside 1 to 10"));
        }
        if !(16..=35).contains(&self.age) {
            return bad(format!("age {} is outside 16 to 35", self.age));
        }
        if self.experience < 0 {
            return bad(format!("experience {} is negative", self.experience));
        }
        if !(BODY_BIPED..=BODY_ROBOTIC).contains(&self.body) {
            return bad(format!("body type {} is outside 0 to 2", self.body));
        }
        if !(0..=KILL_CATEGORY_MAX).contains(&self.kill_category()) {
            return bad(format!(
                "kill category {} is outside 0 to {KILL_CATEGORY_MAX}",
                self.kill_category()
            ));
        }
        Ok(())
    }
}

/// A Fallout 2 critter prototype (416 bytes through `Prototype::write`,
/// SPEC-016 §3.1): art from `art/critters` (conventionally the stand FID,
/// animation 0), the stats of [`CritterSpec::base_stats`], bonus stats and
/// skill points 0, no critter flags, no talking head, unarmed damage type
/// 0 (normal), no script, extended flags 0. `common.material` is not
/// stored: a critter record has no material word. [`ObjectFlags::SOLID`]
/// suits a critter: it blocks movement and shots.
pub fn critter_prototype(common: ProtoCommon, spec: &CritterSpec) -> Result<Prototype, EmitError> {
    check_common(&common, Category::Critter, ART_CRITTERS)?;
    spec.check()?;
    Ok(finish(
        &common,
        Category::Critter,
        ExtendedFlags(0),
        PrototypeData::Critter(Box::new(Critter {
            head_art: NO_HEAD,
            ai: spec.ai_packet,
            team: spec.team,
            flags: CritterFlags(0),
            base_stats: spec.base_stats(),
            bonus_stats: [0; stat::COUNT],
            skills: [0; 18],
            body: spec.body,
            experience: spec.experience,
            kill_category: spec.kill_category(),
            damage_type: Some(0),
        })),
    ))
}

/// Refuse a prototype the parser would read back differently, or would
/// not read: a PID whose category is not the record's, list position 0
/// (other than the player PID), a data variant of another category, and
/// a missing light or script word, which the writer would store as 0 and
/// -1 and the parser would return as present.
pub fn check_prototype(prototype: &Prototype) -> Result<(), EmitError> {
    let category = Category::from_pid(prototype.pid)?;
    let bad = |message: String| Err(EmitError::Prototype(message));
    if category != prototype.category {
        return bad(format!(
            "PID {:#010x} is {category:?}, the record says {:?}",
            prototype.pid, prototype.category
        ));
    }
    if prototype.pid & PID_INDEX_CAP == 0 && prototype.pid != 0x0100_0000 {
        return bad(format!("PID {:#010x} has list position 0", prototype.pid));
    }
    let data_category = match &prototype.data {
        PrototypeData::Item(_) => Category::Item,
        PrototypeData::Critter(_) => Category::Critter,
        PrototypeData::Scenery(_) => Category::Scenery,
        PrototypeData::Wall { .. } => Category::Wall,
        PrototypeData::Tile { .. } => Category::Tile,
        PrototypeData::Misc => Category::Misc,
    };
    if data_category != category {
        return bad(format!(
            "{category:?} PID {:#010x} carries {data_category:?} data",
            prototype.pid
        ));
    }
    if prototype.light.is_none() != (category == Category::Tile) {
        return bad("a light pair is stored by every category but tiles".into());
    }
    if prototype.script.is_none() != (category == Category::Misc) {
        return bad("a script word is stored by every category but misc".into());
    }
    Ok(())
}

/// One category's prototype list: the base bytes plus appended files.
#[derive(Debug, Clone)]
pub struct ProtoList {
    category: Category,
    bytes: Vec<u8>,
    names: Vec<Option<Vec<u8>>>,
}

impl ProtoList {
    pub fn new(category: Category) -> Self {
        Self {
            category,
            bytes: Vec::new(),
            names: Vec::new(),
        }
    }

    /// Start from the base game's list, kept byte for byte.
    pub fn from_base(category: Category, base: &[u8]) -> Result<Self, EmitError> {
        let table = LstTable::parse(base, LstKind::Prototype)?;
        let names: Vec<_> = table.entries().iter().map(|e| e.name.clone()).collect();
        if names.len() > PID_INDEX_CAP as usize {
            return Err(EmitError::Prototype(format!(
                "the base list has {} entries; a PID addresses {PID_INDEX_CAP}",
                names.len()
            )));
        }
        let bytes = lst::terminate_list(base, LstKind::Prototype, names.len())?;
        Ok(Self {
            category,
            bytes,
            names,
        })
    }

    pub fn category(&self) -> Category {
        self.category
    }

    pub fn list_path(&self) -> String {
        prototype_list_path(self.category)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The PID of the last line naming `file` (without ASCII case).
    pub fn find(&self, file: &str) -> Option<u32> {
        let position = self.names.iter().rposition(|name| {
            name.as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(file.as_bytes()))
        })?;
        pid(self.category, position as u32 + 1).ok()
    }

    /// The PID of `file`, appending it when the list lacks it.
    pub fn push(&mut self, file: &str) -> Result<u32, EmitError> {
        if let Some(pid) = self.find(file) {
            return Ok(pid);
        }
        self.append(file)
    }

    fn append(&mut self, file: &str) -> Result<u32, EmitError> {
        let index = self.names.len() as u32 + 1;
        let pid = pid(self.category, index)?;
        let line =
            lst::encode_line(&lst::LstLine::prototype(file)).map_err(|message| EmitError::Lst {
                line: index as usize,
                message,
            })?;
        self.bytes.extend_from_slice(&line);
        self.names.push(Some(file.as_bytes().to_vec()));
        Ok(pid)
    }

    /// Append the next position; returns the PID and the file name.
    ///
    /// The name is the conventional one ([`prototype_file_name`]) when it is
    /// free. A base list does not always number its files by position, so
    /// that name can already belong to another row; sharing it would give
    /// two positions one file and one stored PID. The engine resolves a PID
    /// to whatever name its row holds, so in that case a name of this
    /// tool's own (`mfNNNNNN.pro`) is used instead.
    pub fn push_next(&mut self) -> Result<(u32, String), EmitError> {
        let index = self.names.len() as u32 + 1;
        let mut file = prototype_file_name(index);
        let mut attempt = index;
        while self.find(&file).is_some() {
            if attempt > 999_999 {
                return Err(EmitError::Prototype(format!(
                    "{} has no free file name for position {index}",
                    self.list_path()
                )));
            }
            file = format!("mf{attempt:06}.pro");
            attempt += 1;
        }
        let pid = self.append(&file)?;
        Ok((pid, file))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}
