//! Map objects with state: items with their instance words, containers
//! and critters with inventories, locked doors and containers, stairs and
//! ladders.
//!
//! Records are `vault_formats::map::MapObject` (SPEC-003 §2.10 to §2.12).
//! Every non-critter has a mutable state word at record byte `0x54`
//! (`ObjectState::Other::word`) and the subtype's words after it
//! (`ObjectDetail`), chosen by the object's **prototype**, so a map reads
//! back only through a lookup that knows each PID ([`ProtoSet`] here, or
//! `LocalPrototypeCatalog` over a mounted layer).
//!
//! An inventory is a list of `(quantity, child record)` entries written
//! depth first after the parent's state; a quantity of 3 is one
//! representative child standing for three (SPEC-003 §2.12). Children are
//! not placed: their hex is -1, and they take the parent's elevation.
//!
//! **Base-map rules** that decide what a written value reads back as
//! (SPEC-003 §3.3): on a map without the saved bit (every map this crate
//! writes) a weapon's loaded rounds are reset to its prototype capacity,
//! and an ammunition PID of -1 to the prototype default. So a placed
//! weapon is always full; [`item_object`] writes exactly what the engine
//! will read.

use crate::EmitError;
use crate::map::CritterPlacement;
use std::collections::BTreeMap;
use vault_formats::map::{
    InventoryEntry, MAX_INVENTORY_DEPTH, MapObject, ObjectDetail, ObjectState,
};
use vault_formats::pro::{
    BasicFlags, Category, ItemData, LockFlags, PrototypeData, PrototypeInfo, PrototypeLookup,
    SceneryData,
};
use vault_formats::{Fid, ParseError, Prototype};

/// Prototypes by PID: what a constructor of this module reads (subtype,
/// size, capacity, defaults), and a `PrototypeLookup` for
/// `MapFile::parse_for`.
#[derive(Debug, Clone, Default)]
pub struct ProtoSet {
    prototypes: BTreeMap<u32, Prototype>,
}

impl ProtoSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a prototype.
    pub fn insert(&mut self, prototype: Prototype) {
        self.prototypes.insert(prototype.pid, prototype);
    }

    pub fn get(&self, pid: u32) -> Option<&Prototype> {
        self.prototypes.get(&pid)
    }

    fn require(&self, pid: u32) -> Result<&Prototype, EmitError> {
        self.get(pid)
            .ok_or_else(|| EmitError::Prototype(format!("no prototype for PID {pid:#010x}")))
    }

    pub fn len(&self) -> usize {
        self.prototypes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.prototypes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Prototype> {
        self.prototypes.values()
    }
}

impl PrototypeLookup for ProtoSet {
    fn lookup(&self, pid: u32) -> Result<PrototypeInfo, ParseError> {
        self.get(pid)
            .map(Prototype::info)
            .ok_or_else(|| ParseError {
                format: "PRO",
                offset: 0,
                message: format!("no prototype {pid:#010x}"),
            })
    }
}

/// A lock's state bits (SPEC-003 §2.6; SPEC-013 §2.3, §3.8). For a door
/// they live in its door-state word (`ObjectDetail::Door`), bit 0 being
/// open; for a container in its item state word (`ObjectState::Other::word`).
/// The engine sets and clears them only through scripts and the jam rules;
/// it has no lock difficulty of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LockState {
    /// `0x02000000`: a container refuses to open with `proto.msg` 487; a
    /// door plays its locked sound, and its script decides.
    pub locked: bool,
    /// `0x04000000`: every skill on it is refused with `misc.msg` 2001.
    pub jammed: bool,
}

impl LockState {
    pub const LOCKED: Self = Self {
        locked: true,
        jammed: false,
    };

    pub fn bits(self) -> u32 {
        let mut bits = 0;
        if self.locked {
            bits |= LockFlags::LOCKED.0;
        }
        if self.jammed {
            bits |= LockFlags::JAMMED.0;
        }
        bits
    }
}

/// One inventory entry: a representative record and how many it stands
/// for (at least 1).
#[derive(Debug, Clone)]
pub struct Stack {
    pub object: MapObject,
    pub quantity: u32,
}

impl Stack {
    pub fn new(object: MapObject, quantity: u32) -> Self {
        Self { object, quantity }
    }
}

/// Instance words to override on a placed item. `None` takes the
/// prototype's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemState {
    /// A weapon's loaded ammunition PID (an ammunition prototype of the
    /// weapon's calibre); `None` is the prototype default. Rounds are
    /// always the capacity (module notes).
    pub ammo: Option<u32>,
    /// An ammunition object's rounds in its front pack, 1 to the pack
    /// size; `None` is a full pack.
    pub rounds: Option<i32>,
    /// A miscellaneous item's current charges, 0 to its maximum.
    pub charges: Option<i32>,
    /// A key's own key code; `None` copies the prototype's.
    pub key: Option<i32>,
}

fn record(id: u32, prototype: &Prototype, hex: Option<i32>, elevation: u8) -> MapObject {
    MapObject {
        id,
        hex,
        displacement: [0, 0],
        screen: [0, 0],
        frame: 0,
        orientation: 0,
        art_id: prototype.art_id,
        flags: prototype.flags,
        stored_elevation: i32::from(elevation),
        elevation,
        pid: prototype.pid,
        combat_id: -1,
        light: prototype.light.unwrap_or(vault_formats::pro::Light {
            radius: 0,
            intensity: 0,
        }),
        outline: 0,
        script_id: -1,
        cached_script_file: -1,
        inventory_capacity: 0,
        state: ObjectState::Other {
            word: 0,
            detail: ObjectDetail::Plain,
        },
        inventory: Vec::new(),
        owner: None,
        file_offset: 0,
    }
}

fn refuse<T>(pid: u32, message: String) -> Result<T, EmitError> {
    Err(EmitError::Prototype(format!("PID {pid:#010x}: {message}")))
}

fn check_place(hex: Option<i32>, elevation: u8) -> Result<(), EmitError> {
    if hex.is_some_and(|hex| !(0..40_000).contains(&hex)) || elevation > 2 {
        return Err(EmitError::Path {
            path: format!("hex {hex:?} elevation {elevation}"),
            message: "a placed object needs hex 0 to 39999 and elevation 0 to 2".into(),
        });
    }
    Ok(())
}

/// An item record for `prototype` (an item): on `hex` (or carried, `None`)
/// at `elevation`, facing 0, frame 0, no script, the prototype's art,
/// flags and light, and the instance words its subtype has (SPEC-003
/// §2.11): a weapon's rounds (= capacity) and ammunition PID, an
/// ammunition object's rounds, a miscellaneous item's charges, a key's
/// code. Armour, containers and drugs have none.
///
/// Refused: a non-item prototype, an ammunition PID that is not
/// ammunition of the weapon's calibre (when `set` holds it), rounds
/// outside 1 to the pack size, charges outside 0 to the maximum, an
/// override that does not apply to the subtype.
pub fn item_object(
    set: &ProtoSet,
    pid: u32,
    id: u32,
    hex: Option<i32>,
    elevation: u8,
    state: ItemState,
) -> Result<MapObject, EmitError> {
    check_place(hex, elevation)?;
    let prototype = set.require(pid)?;
    let PrototypeData::Item(item) = &prototype.data else {
        return Err(EmitError::Prototype(format!(
            "PID {pid:#010x} is not an item prototype"
        )));
    };
    let unused = |name: &str, used: bool| -> Result<(), EmitError> {
        if used {
            return refuse(
                pid,
                format!("a {:?} has no {name} word", item.data.subtype()),
            );
        }
        Ok(())
    };
    let detail = match &item.data {
        ItemData::Weapon(weapon) => {
            unused("rounds", state.rounds.is_some())?;
            unused("charges", state.charges.is_some())?;
            unused("key", state.key.is_some())?;
            let ammo_pid = match state.ammo {
                None => weapon.ammo_pid,
                Some(ammo) => {
                    if weapon.capacity == 0 {
                        return Err(EmitError::Prototype(format!(
                            "PID {pid:#010x}: a weapon without a magazine takes no ammunition"
                        )));
                    }
                    if let Some(PrototypeData::Item(round)) = set.get(ammo).map(|p| &p.data) {
                        match round.data {
                            ItemData::Ammunition { caliber, .. } if caliber == weapon.caliber => {}
                            _ => {
                                return Err(EmitError::Prototype(format!(
                                    "PID {pid:#010x}: {ammo:#010x} is not ammunition of calibre {}",
                                    weapon.caliber
                                )));
                            }
                        }
                    } else {
                        return Err(EmitError::Prototype(format!(
                            "PID {pid:#010x}: no ammunition prototype {ammo:#010x}"
                        )));
                    }
                    ammo as i32
                }
            };
            ObjectDetail::Weapon {
                rounds: weapon.capacity,
                ammo_pid,
            }
        }
        ItemData::Ammunition { quantity, .. } => {
            unused("ammunition PID", state.ammo.is_some())?;
            unused("charges", state.charges.is_some())?;
            unused("key", state.key.is_some())?;
            let rounds = state.rounds.unwrap_or(*quantity);
            if !(1..=*quantity).contains(&rounds) {
                return refuse(pid, format!("{rounds} rounds in a pack of {quantity}"));
            }
            ObjectDetail::Ammunition(rounds)
        }
        ItemData::Misc { charges, .. } => {
            unused("ammunition PID", state.ammo.is_some())?;
            unused("rounds", state.rounds.is_some())?;
            unused("key", state.key.is_some())?;
            let current = state.charges.unwrap_or(*charges);
            if !(0..=*charges).contains(&current) {
                return refuse(pid, format!("{current} charges of at most {charges}"));
            }
            ObjectDetail::Charges(current)
        }
        ItemData::Key { code } => {
            unused("ammunition PID", state.ammo.is_some())?;
            unused("rounds", state.rounds.is_some())?;
            unused("charges", state.charges.is_some())?;
            ObjectDetail::Key(state.key.unwrap_or(*code))
        }
        ItemData::Armor(_) | ItemData::Container { .. } | ItemData::Drug(_) => {
            if state != ItemState::default() {
                return refuse(
                    pid,
                    "armour, containers and drugs have no instance words".into(),
                );
            }
            ObjectDetail::Plain
        }
    };
    let mut object = record(id, prototype, hex, elevation);
    object.state = ObjectState::Other { word: 0, detail };
    Ok(object)
}

/// The size `object` and what it holds take in a container: size × the
/// quantity of each entry, direct contents only (SPEC-011 §3.14).
fn contents_size(set: &ProtoSet, stacks: &[Stack]) -> Result<i64, EmitError> {
    let mut total = 0_i64;
    for stack in stacks {
        let prototype = set.require(stack.object.pid)?;
        let size = match &prototype.data {
            PrototypeData::Item(item) => item.size,
            _ => {
                return Err(EmitError::Prototype(format!(
                    "PID {:#010x} is not an item and cannot be held",
                    prototype.pid
                )));
            }
        };
        total += i64::from(size) * i64::from(stack.quantity);
    }
    Ok(total)
}

fn depth(object: &MapObject) -> usize {
    object
        .inventory
        .iter()
        .map(|entry| 1 + depth(&entry.object))
        .max()
        .unwrap_or(0)
}

fn count(object: &MapObject) -> usize {
    1 + object
        .inventory
        .iter()
        .map(|entry| count(&entry.object))
        .sum::<usize>()
}

fn adopt(child: &mut MapObject, owner: u32, elevation: u8) {
    child.hex = None;
    child.owner = Some(owner);
    child.elevation = elevation;
    child.stored_elevation = i32::from(elevation);
    let id = child.id;
    for entry in &mut child.inventory {
        adopt(&mut entry.object, id, elevation);
    }
}

/// Give `parent` the inventory `stacks`, in order: each child unplaced
/// (hex -1), at the parent's elevation, owned by it; the capacity word
/// the entry count. Refused: a quantity of 0, a temporary child (the
/// writer drops it), a critter child, and a tree deeper than the
/// parser's 64 levels or with more than 100,000 records.
pub fn fill_inventory(parent: &mut MapObject, stacks: Vec<Stack>) -> Result<(), EmitError> {
    for stack in &stacks {
        if stack.quantity == 0 || stack.quantity > i32::MAX as u32 {
            return Err(EmitError::Prototype(format!(
                "object {}: quantity {} is outside 1 to {}",
                stack.object.id,
                stack.quantity,
                i32::MAX
            )));
        }
        if stack.object.flags.contains(BasicFlags::TEMPORARY) {
            return Err(EmitError::Prototype(format!(
                "object {} is temporary, and a temporary child is not saved",
                stack.object.id
            )));
        }
        if Category::from_pid(stack.object.pid)? != Category::Item {
            return Err(EmitError::Prototype(format!(
                "object {} (PID {:#010x}) is not an item",
                stack.object.id, stack.object.pid
            )));
        }
    }
    let elevation = parent.elevation;
    parent.inventory = stacks
        .into_iter()
        .map(|mut stack| {
            adopt(&mut stack.object, parent.id, elevation);
            InventoryEntry {
                quantity: stack.quantity,
                object: stack.object,
            }
        })
        .collect();
    parent.inventory_capacity = parent.inventory.len();
    if depth(parent) > MAX_INVENTORY_DEPTH {
        return Err(EmitError::Prototype(format!(
            "object {}: inventory nested deeper than {MAX_INVENTORY_DEPTH}",
            parent.id
        )));
    }
    if count(parent) > vault_formats::map::MAX_OBJECTS {
        return Err(EmitError::Prototype(format!(
            "object {}: more than {} records in one tree",
            parent.id,
            vault_formats::map::MAX_OBJECTS
        )));
    }
    Ok(())
}

/// A container record holding `contents`, with its lock bits in its item
/// state word.
///
/// Refused: a prototype that is not a container, contents that do not
/// fit (the engine refuses an addition when the total size would reach
/// the capacity, SPEC-011 §3.14, so the contents' total must stay below
/// it), and what [`fill_inventory`] refuses.
pub fn container_object(
    set: &ProtoSet,
    pid: u32,
    id: u32,
    hex: Option<i32>,
    elevation: u8,
    lock: LockState,
    contents: Vec<Stack>,
) -> Result<MapObject, EmitError> {
    check_place(hex, elevation)?;
    let prototype = set.require(pid)?;
    let capacity = match &prototype.data {
        PrototypeData::Item(item) => match item.data {
            ItemData::Container { capacity, .. } => capacity,
            _ => {
                return Err(EmitError::Prototype(format!(
                    "PID {pid:#010x} is not a container"
                )));
            }
        },
        _ => {
            return Err(EmitError::Prototype(format!(
                "PID {pid:#010x} is not a container"
            )));
        }
    };
    let size = contents_size(set, &contents)?;
    if !contents.is_empty() && size > 0 && size >= i64::from(capacity) {
        return Err(EmitError::Prototype(format!(
            "container {pid:#010x}: contents of size {size} do not fit below its capacity \
             {capacity}"
        )));
    }
    let mut object = record(id, prototype, hex, elevation);
    object.state = ObjectState::Other {
        word: lock.bits(),
        detail: ObjectDetail::Plain,
    };
    fill_inventory(&mut object, contents)?;
    Ok(object)
}

/// A door record: its door-state word is the lock bits plus open (bit 0)
/// when `open`. An open door also takes the open flag group
/// (`NONBLOCKING`, `LIGHT_THROUGH`, `PROJECTILE_THROUGH`) and its last
/// frame, as the engine leaves an opened door (SPEC-007 §3.11.4); pass
/// `last_frame` (the frame count of its art less one) for that. The key
/// code is the prototype's (`SceneryData::Door::key`): the record has no
/// key word.
pub fn door_object(
    set: &ProtoSet,
    pid: u32,
    id: u32,
    hex: i32,
    elevation: u8,
    lock: LockState,
    open: Option<usize>,
) -> Result<MapObject, EmitError> {
    check_place(Some(hex), elevation)?;
    let prototype = set.require(pid)?;
    if !matches!(
        &prototype.data,
        PrototypeData::Scenery(scenery) if matches!(scenery.data, SceneryData::Door { .. })
    ) {
        return Err(EmitError::Prototype(format!(
            "PID {pid:#010x} is not a door"
        )));
    }
    let mut object = record(id, prototype, Some(hex), elevation);
    let mut state = lock.bits();
    if let Some(last_frame) = open {
        state |= LockFlags::OPEN.0;
        object.frame = last_frame;
        object.flags = BasicFlags(object.flags.0 | 0xA000_0010);
    }
    object.state = ObjectState::Other {
        word: 0,
        detail: ObjectDetail::Door(LockFlags(state)),
    };
    Ok(object)
}

/// A stairs, ladder or elevator record, copying the prototype's
/// destination (SPEC-003 §2.11). A Fallout 2 ladder record also holds the
/// destination map, `ladder_map` (SPEC-016 §2); `None` writes the map's own
/// number. See [`crate::transit`]: the engine never reads these words.
pub fn transit_object(
    set: &ProtoSet,
    pid: u32,
    id: u32,
    hex: i32,
    elevation: u8,
    ladder_map: Option<i32>,
) -> Result<MapObject, EmitError> {
    check_place(Some(hex), elevation)?;
    let prototype = set.require(pid)?;
    let PrototypeData::Scenery(scenery) = &prototype.data else {
        return Err(EmitError::Prototype(format!(
            "PID {pid:#010x} is not scenery"
        )));
    };
    let detail = match scenery.data {
        SceneryData::Stairs { map, destination } => ObjectDetail::Stairs {
            map,
            destination: vault_formats::pro::PackedLocation(destination),
        },
        SceneryData::Elevator { kind, level } => ObjectDetail::Elevator { kind, level },
        SceneryData::LadderUp { destination } | SceneryData::LadderDown { destination } => {
            ObjectDetail::Ladder {
                map: ladder_map,
                destination: vault_formats::pro::PackedLocation(destination),
            }
        }
        _ => {
            return Err(EmitError::Prototype(format!(
                "PID {pid:#010x} is not stairs, a ladder or an elevator"
            )));
        }
    };
    let mut object = record(id, prototype, Some(hex), elevation);
    object.state = ObjectState::Other { word: 0, detail };
    Ok(object)
}

/// What a placed critter carries, uses and wears.
#[derive(Debug, Clone, Default)]
pub struct Loadout {
    /// Wielded in the right hand (basic flag `0x02000000`); the hand
    /// non-player critters use (SPEC-005 `wield_obj_critter`).
    pub right_hand: Option<MapObject>,
    /// Held in the left hand (`0x01000000`).
    pub left_hand: Option<MapObject>,
    /// Worn armour (`0x04000000`).
    pub worn: Option<MapObject>,
    /// Everything else, in inventory order.
    pub carried: Vec<Stack>,
}

/// A placed critter ([`crate::map::critter_object`]) with an inventory:
/// the worn armour, the hand items, then the carried stacks, each held
/// item a single unit with its slot's basic flag. When the right hand
/// holds a weapon, the critter's art takes that weapon's code (FID bits 12
/// to 15), which is how the engine shows, and tests, that it holds one
/// (SPEC-009 §3.12, SPEC-005 §3.8); the art must then include the
/// lettered stand, walk and attack files ([`crate::animation`]).
///
/// Worn armour only moves the figure for the player: a non-player
/// critter's protection is its prototype's (SPEC-011 §3.10), so put the
/// armour's values in its prototype ([`crate::fighter`]).
///
/// Refused: an item in the wrong slot (armour in a hand or anything but
/// armour worn), a non-item, and what [`crate::map::critter_object`] and
/// [`fill_inventory`] refuse.
pub fn critter_with_loadout(
    placement: &CritterPlacement,
    set: &ProtoSet,
    loadout: Loadout,
) -> Result<MapObject, EmitError> {
    let mut placement = *placement;
    let mut stacks = Vec::new();
    let subtype_of = |object: &MapObject| -> Result<ItemData, EmitError> {
        match &set.require(object.pid)?.data {
            PrototypeData::Item(item) => Ok(item.data.clone()),
            _ => Err(EmitError::Prototype(format!(
                "PID {:#010x} is not an item",
                object.pid
            ))),
        }
    };
    if let Some(mut worn) = loadout.worn {
        if !matches!(subtype_of(&worn)?, ItemData::Armor(_)) {
            return Err(EmitError::Critter(format!(
                "worn item {:#010x} is not armour",
                worn.pid
            )));
        }
        worn.flags = BasicFlags(worn.flags.0 | BasicFlags::WORN.0);
        stacks.push(Stack::new(worn, 1));
    }
    let mut weapon_code = 0;
    for (hand, flag) in [
        (loadout.right_hand, BasicFlags::RIGHT_HAND),
        (loadout.left_hand, BasicFlags::LEFT_HAND),
    ] {
        let Some(mut held) = hand else {
            continue;
        };
        let data = subtype_of(&held)?;
        if matches!(data, ItemData::Armor(_)) {
            return Err(EmitError::Critter(format!(
                "armour {:#010x} cannot be held in a hand",
                held.pid
            )));
        }
        if flag == BasicFlags::RIGHT_HAND
            && let ItemData::Weapon(weapon) = &data
        {
            weapon_code = u8::try_from(weapon.animation)
                .ok()
                .filter(|code| *code <= 10)
                .ok_or_else(|| {
                    EmitError::Critter(format!(
                        "weapon {:#010x} has weapon code {}",
                        held.pid, weapon.animation
                    ))
                })?;
        }
        held.flags = BasicFlags(held.flags.0 | flag.0);
        stacks.push(Stack::new(held, 1));
    }
    stacks.extend(loadout.carried);
    let fid = placement.fid;
    placement.fid = Fid::new(
        fid.category(),
        fid.base(),
        fid.animation(),
        weapon_code,
        fid.selector(),
    );
    let mut critter = crate::map::critter_object(&placement)?;
    fill_inventory(&mut critter, stacks)?;
    Ok(critter)
}
