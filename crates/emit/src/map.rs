//! Small builders for `vault_formats::map::MapFile`, which has public
//! fields but no constructor. Serialization is `MapFile::write`, wrapped by
//! [`crate::FormatWriters::map`].

use crate::EmitError;
use crate::proto::stat::MAX_HIT_POINTS;
use std::collections::{BTreeMap, BTreeSet};
use vault_formats::fid::CRITTER_CATEGORY;
use vault_formats::map::{
    CritterState, MapFile, MapHeader, MapObject, MapObjects, MapScript, ObjectDetail, ObjectState,
    ScriptBlock, ScriptCategory, ScriptPrefix,
};
use vault_formats::mod_manifest::{MAX_LOCALS, SCRIPT_INDEXES};
use vault_formats::pro::{
    BasicFlags, Category, CombatFlags, Light, MapFlags, PackedLocation, PrototypeData,
    PrototypeLookup, ScriptFlags,
};
use vault_formats::{Fid, GameProfile, Prototype};

/// A map name is stored in 16 bytes; this writer keeps a terminating NUL.
pub const MAP_NAME_BYTES: usize = 15;

/// An empty Fallout 2 (version 20) map: no variables, no scripts, no
/// objects, blank floors and roofs on elevation 0, elevations 1 and 2
/// omitted, entrance at `entry_hex` on elevation 0 facing 0, darkness 1.
/// `name` is the header name (conventionally the file name in capitals).
pub fn empty_map(name: &str, map_number: i32, entry_hex: i32) -> Result<MapFile, EmitError> {
    if !name.is_ascii() || name.is_empty() || name.len() > MAP_NAME_BYTES || name.contains('\0') {
        return Err(EmitError::Path {
            path: name.into(),
            message: format!("a map name is 1 to {MAP_NAME_BYTES} ASCII bytes"),
        });
    }
    if !(0..40_000).contains(&entry_hex) {
        return Err(EmitError::Path {
            path: name.into(),
            message: format!("entry hex {entry_hex} is outside 0 to 39999"),
        });
    }
    let mut stored = [0; 16];
    stored[..name.len()].copy_from_slice(name.as_bytes());
    Ok(MapFile {
        header: MapHeader {
            version: GameProfile::Fallout2.map_version(),
            name: stored,
            entry_hex,
            entry_elevation: 0,
            entry_orientation: 0,
            stored_local_count: 0,
            script_reference: 0,
            flags: MapFlags(MapFlags::OMIT_ELEVATION_1.0 | MapFlags::OMIT_ELEVATION_2.0),
            darkness: 1,
            stored_global_count: 0,
            map_number,
            last_visit: 0,
            reserved: [0; 44],
        },
        mvars: Vec::new(),
        lvars: Vec::new(),
        tiles: [
            Some(vec![vault_formats::map::TilePair::BLANK; 10_000]),
            None,
            None,
        ],
        scripts: std::array::from_fn(|_| ScriptCategory {
            declared_count: 0,
            blocks: Vec::new(),
        }),
        objects: MapObjects {
            total: 0,
            elevations: std::array::from_fn(|_| Vec::new()),
            consumed_len: 0,
        },
        consumed_len: 0,
        diagnostics: Vec::new(),
    })
}

/// Where the player enters a map: the header's entry hex, elevation and
/// orientation (SPEC-003 §2.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapEntry {
    /// 0 to 39,999.
    pub hex: i32,
    /// 0 to 2; must be an elevation the map has.
    pub elevation: u8,
    /// 0 (north-east) to 5.
    pub orientation: u8,
}

/// 10,000 blank tile pairs (floor and roof index 1, SPEC-003 §2.8): one
/// elevation's tile block before any floor is laid.
pub fn blank_tiles() -> Vec<vault_formats::map::TilePair> {
    vec![vault_formats::map::TilePair::BLANK; 10_000]
}

/// An empty Fallout 2 (version 20) map with one to three elevations: no
/// variables, scripts or objects, darkness 1. `tiles[e]` is elevation
/// `e`'s 10,000 tile pairs (row-major squares, [`blank_tiles`] for a bare
/// floor), or `None` for an elevation the map does not have, whose
/// omission bit (`OMIT_ELEVATION_0` to `_2`) is then set.
///
/// [`empty_map`] is this with `[Some(blank), None, None]` and entry
/// elevation and orientation 0.
///
/// `MapFile::write` recomputes the omission bits: it omits **every**
/// elevation whose tiles are all blank (floor and roof 1) and that holds
/// no saved object, present here or not (SPEC-003 §3.2). Such an
/// elevation reads back as `None`. Give each elevation that must survive
/// a non-blank tile or an object.
///
/// Refused: a name `empty_map` refuses, an entry hex outside 0 to 39,999,
/// orientation above 5, an entry elevation above 2 or without tiles, and a
/// tile block that is not 10,000 pairs.
pub fn map_with_elevations(
    name: &str,
    map_number: i32,
    entry: MapEntry,
    tiles: [Option<Vec<vault_formats::map::TilePair>>; 3],
) -> Result<MapFile, EmitError> {
    let mut map = empty_map(name, map_number, entry.hex)?;
    let bad = |message: String| {
        Err(EmitError::Path {
            path: name.into(),
            message,
        })
    };
    if entry.orientation > 5 {
        return bad(format!(
            "entry orientation {} is outside 0 to 5",
            entry.orientation
        ));
    }
    if tiles
        .get(usize::from(entry.elevation))
        .is_none_or(Option::is_none)
    {
        return bad(format!(
            "entry elevation {} is not one of the map's elevations",
            entry.elevation
        ));
    }
    let mut flags = 0;
    for (elevation, block) in tiles.iter().enumerate() {
        match block {
            None => flags |= MapFlags::OMIT_ELEVATION_0.0 << elevation,
            Some(block) if block.len() != 10_000 => {
                return bad(format!(
                    "elevation {elevation} has {} tile pairs, not 10000",
                    block.len()
                ));
            }
            Some(_) => {}
        }
    }
    map.header.entry_elevation = entry.elevation;
    map.header.entry_orientation = entry.orientation;
    map.header.flags = MapFlags(flags);
    map.tiles = tiles;
    Ok(map)
}

/// What a placed critter record needs (SPEC-003 §2.10 and §2.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CritterPlacement {
    /// The object's unique id within the map.
    pub id: u32,
    /// A file-backed critter PID (category 1, list position 1 or more).
    pub pid: u32,
    /// The stand FID: category 1, animation 0.
    pub fid: Fid,
    pub hex: i32,
    pub elevation: u8,
    /// 0 (north-east) to 5.
    pub orientation: u8,
    /// The instance's basic flags, normally the prototype's.
    pub flags: BasicFlags,
    pub ai_packet: i32,
    pub team: i32,
    /// Current hit points, above 0: at 0 the engine treats the critter as
    /// dead.
    pub hit_points: i32,
}

impl CritterPlacement {
    /// A placement of `prototype` (a critter): its PID, art, flags, AI
    /// packet and team, at full hit points (maximum hit points, base plus
    /// bonus).
    pub fn from_prototype(
        prototype: &Prototype,
        id: u32,
        hex: i32,
        elevation: u8,
        orientation: u8,
    ) -> Result<Self, EmitError> {
        let PrototypeData::Critter(critter) = &prototype.data else {
            return Err(EmitError::Critter(format!(
                "PID {:#010x} is not a critter prototype",
                prototype.pid
            )));
        };
        let hit_points =
            critter.base_stats[MAX_HIT_POINTS].saturating_add(critter.bonus_stats[MAX_HIT_POINTS]);
        Ok(Self {
            id,
            pid: prototype.pid,
            fid: Fid(prototype.art_id),
            hex,
            elevation,
            orientation,
            flags: prototype.flags,
            ai_packet: critter.ai,
            team: critter.team,
            hit_points,
        })
    }
}

/// The map record of a placed critter: `placement`'s id, hex, elevation,
/// orientation, art, PID, flags, AI packet, team and hit points; frame 0,
/// no displacement, combat id -1, light 0, no script (-1), empty inventory
/// of capacity 0, last attacker -1, and reaction, damage last turn,
/// maneuvers, action points, combat status, radiation and poison 0. The
/// record is 128 bytes (SPEC-003 §2.11).
///
/// Refused: hit points 0 or below, a hex outside 0 to 39,999, elevation
/// above 2, orientation above 5, a PID that is not a file-backed critter
/// PID, a FID that is not critter art for animation 0, and the
/// `TEMPORARY` flag (such an object is not saved) or the `0x400` special
/// bit (cleared when a map loads a critter, so it would not read back).
pub fn critter_object(placement: &CritterPlacement) -> Result<MapObject, EmitError> {
    let bad = |message: String| Err(EmitError::Critter(message));
    if placement.hit_points <= 0 {
        return bad(format!(
            "current hit points {} must be above 0; at 0 the critter is dead",
            placement.hit_points
        ));
    }
    if !(0..40_000).contains(&placement.hex) {
        return bad(format!("hex {} is outside 0 to 39999", placement.hex));
    }
    if placement.elevation > 2 || placement.orientation > 5 {
        return bad(format!(
            "elevation {} or orientation {} is out of range (0 to 2, 0 to 5)",
            placement.elevation, placement.orientation
        ));
    }
    if Category::from_pid(placement.pid).ok() != Some(Category::Critter)
        || placement.pid & 0x00ff_ffff == 0
    {
        return bad(format!(
            "PID {:#010x} is not a file-backed critter PID",
            placement.pid
        ));
    }
    let fid = placement.fid;
    if fid.category() != CRITTER_CATEGORY || fid.animation() != 0 || fid.selector() != 0 {
        return bad(format!(
            "FID {:#010x} is not critter stand art (category 1, animation 0)",
            fid.0
        ));
    }
    if placement.flags.contains(BasicFlags::TEMPORARY)
        || placement.flags.contains(BasicFlags::SPECIAL)
    {
        return bad(format!(
            "flags {:#010x} hold TEMPORARY or the 0x400 special bit",
            placement.flags.0
        ));
    }
    Ok(MapObject {
        id: placement.id,
        hex: Some(placement.hex),
        displacement: [0, 0],
        screen: [0, 0],
        frame: 0,
        orientation: placement.orientation,
        art_id: fid.0,
        flags: placement.flags,
        stored_elevation: i32::from(placement.elevation),
        elevation: placement.elevation,
        pid: placement.pid,
        combat_id: -1,
        light: Light {
            radius: 0,
            intensity: 0,
        },
        outline: 0,
        script_id: -1,
        cached_script_file: -1,
        inventory_capacity: 0,
        state: ObjectState::Critter(CritterState {
            reaction: 0,
            damage: 0,
            maneuvers: 0,
            ap: 0,
            results: CombatFlags(0),
            ai: placement.ai_packet,
            team: placement.team,
            attacker: -1,
            hp: placement.hit_points,
            radiation: 0,
            poison: 0,
        }),
        inventory: Vec::new(),
        owner: None,
        file_offset: 0,
    })
}

/// A scriptless, inventory-less object of `pid` with art `fid` on `hex`,
/// facing 0, frame 0: the record a wall, scenery, generic item or
/// miscellaneous marker needs. Doors, ladders, stairs, elevators, exit
/// grids, weapons, ammunition, keys and powered items carry extra words:
/// set `state` to the matching `ObjectDetail` for those.
pub fn plain_object(
    id: u32,
    pid: u32,
    fid: Fid,
    hex: i32,
    elevation: u8,
    flags: BasicFlags,
) -> MapObject {
    MapObject {
        id,
        hex: Some(hex),
        displacement: [0, 0],
        screen: [0, 0],
        frame: 0,
        orientation: 0,
        art_id: fid.0,
        flags,
        stored_elevation: i32::from(elevation),
        elevation,
        pid,
        combat_id: -1,
        light: Light {
            radius: 0,
            intensity: 0,
        },
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

// ---- Attaching scripts (simulation layer, spec 10.4) -----------------------------

/// What an object-bound script record is for: SPEC-003 section 2.9's
/// category 3 (every object but a critter) or 4 (a critter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectScript {
    /// A scenery, item, wall or miscellaneous object's script.
    Item,
    /// A critter's script.
    Critter,
}

/// Where a script record attaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptSite {
    /// The object with this unique id (top level or in an inventory); its
    /// PID must be a critter's for [`ObjectScript::Critter`] and any other
    /// category's for [`ObjectScript::Item`].
    Object { id: u32, kind: ObjectScript },
    /// A spatial script (category 1): fires on a hex within `radius`.
    Spatial {
        hex: i32,
        elevation: u8,
        radius: i32,
    },
}

/// One script to attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptAttachment {
    pub site: ScriptSite,
    /// The script's row in `scripts.lst` (or a manifest `[[scripts]]`
    /// index): 0 to `0x00FF_FFFF`.
    pub script_index: u32,
    /// The record's local variable count, validated and kept in memory. A
    /// map that is not a saved game does not store it (a record reads back
    /// with 0, and the engine takes the count from the script list row at
    /// load); a saved map's record keeps it.
    pub local_vars: u32,
}

const SPATIAL_STREAM: usize = 1;
const ITEM_STREAM: usize = 3;
const CRITTER_STREAM: usize = 4;

/// Where an object is: elevation, index in that elevation, then inventory
/// indexes down to it.
type ObjectPath = (usize, Vec<usize>);

fn index_objects(
    objects: &[MapObject],
    path: &mut Vec<usize>,
    elevation: usize,
    found: &mut BTreeMap<u32, Vec<ObjectPath>>,
) {
    for (index, object) in objects.iter().enumerate() {
        path.push(index);
        found
            .entry(object.id)
            .or_default()
            .push((elevation, path.clone()));
        for (child_index, child) in object.inventory.iter().enumerate() {
            path.push(child_index);
            index_inventory(&child.object, path, elevation, found);
            path.pop();
        }
        path.pop();
    }
}

fn index_inventory(
    object: &MapObject,
    path: &mut Vec<usize>,
    elevation: usize,
    found: &mut BTreeMap<u32, Vec<ObjectPath>>,
) {
    found
        .entry(object.id)
        .or_default()
        .push((elevation, path.clone()));
    for (index, child) in object.inventory.iter().enumerate() {
        path.push(index);
        index_inventory(&child.object, path, elevation, found);
        path.pop();
    }
}

fn object_at<'a>(map: &'a mut MapFile, (elevation, path): &ObjectPath) -> &'a mut MapObject {
    let mut object = &mut map.objects.elevations[*elevation][path[0]];
    for index in &path[1..] {
        object = &mut object.inventory[*index].object;
    }
    object
}

fn blank_script_slot() -> MapScript {
    MapScript {
        id: 0,
        link: 0,
        prefix: ScriptPrefix::None,
        stored_flags: ScriptFlags(0),
        flags: ScriptFlags(0),
        file_index: 0,
        object_id: 0,
        local_base: 0,
        stored_local_count: 0,
        local_count: 0,
        result: 0,
        event: 0,
        parameter: 0,
        action: 0,
        override_default: 0,
        execution_state: 0,
        quantity: 0,
        annotations: 0,
        file_offset: 0,
    }
}

/// `records` laid out as `MapFile::write` and the parser expect: blocks of
/// 16 slots, the last padded.
fn category_of(records: Vec<MapScript>) -> ScriptCategory {
    let declared_count = records.len();
    let blocks = records
        .chunks(16)
        .map(|chunk| {
            let mut slots = chunk.to_vec();
            slots.resize_with(16, blank_script_slot);
            ScriptBlock {
                slots,
                active: chunk.len(),
            }
        })
        .collect();
    ScriptCategory {
        declared_count,
        blocks,
    }
}

/// Add script records to a parsed map, bind objects to them, and set the
/// header's map script (SPEC-003 sections 2.7, 2.9 and 3.3).
///
/// - Each object attachment adds one record to the item (3) or critter (4)
///   category, with the lowest free serial from 1 in its category, and sets
///   the object's script id and cached script file. Each spatial
///   attachment adds a category-1 record. Records are appended after the
///   map's existing ones in a fixed order, by object id (spatial ones by
///   place), so the order of `attachments` does not matter.
/// - **Re-runs.** An object that already has a record of the same script
///   index keeps it, and a spatial record that already matches (same hex,
///   elevation, radius and script) is not added again, so applying the
///   same attachments to the result changes nothing. An object that
///   already has another script is refused.
/// - `map_script` is a script index; the header stores it plus one
///   (SPEC-003 section 3.3). The engine creates the map script from the
///   header alone, so no system-category record is written.
/// - Objects not named are untouched.
///
/// Refused, and the map left as it was: an id that is on no object (named
/// in the error), an id on several objects, a critter category on a
/// non-critter or the reverse, a script index over `0x00FF_FFFF`, more than
/// 1,000,000 local variables, a spatial hex outside 0 to 39,999, elevation
/// above 2 or negative radius, and an object that already has a different
/// script.
pub fn attach_scripts_to(
    map: &mut MapFile,
    attachments: &[ScriptAttachment],
    map_script: Option<u32>,
) -> Result<(), EmitError> {
    let bad = |message: String| Err(EmitError::Map(message));
    for attachment in attachments {
        if !SCRIPT_INDEXES.contains(&attachment.script_index) {
            return bad(format!(
                "script index {} is outside 0 to {}",
                attachment.script_index,
                SCRIPT_INDEXES.end()
            ));
        }
        if attachment.local_vars > MAX_LOCALS {
            return bad(format!(
                "{} local variables; the limit is {MAX_LOCALS}",
                attachment.local_vars
            ));
        }
        if let ScriptSite::Spatial {
            hex,
            elevation,
            radius,
        } = attachment.site
            && (!(0..40_000).contains(&hex) || elevation > 2 || radius < 0)
        {
            return bad(format!(
                "spatial script at hex {hex}, elevation {elevation}, radius {radius} is outside the map"
            ));
        }
    }
    if let Some(index) = map_script
        && !SCRIPT_INDEXES.contains(&index)
    {
        return bad(format!(
            "map script index {index} is outside 0 to {}",
            SCRIPT_INDEXES.end()
        ));
    }

    // The local count a record keeps in the file. A fresh (not saved) map's
    // records are read back with 0 here, and the count comes from the
    // script list row at load (SPEC-003 section 2.9), so 0 is what a second
    // run would find and write; a saved map keeps the count.
    let saved = map.header.flags.contains(MapFlags::SAVED);
    let written_locals = |count: u32| if saved { count as i32 } else { 0 };
    // Records are added in a fixed order whatever the order of the input:
    // by object id, spatial ones by place.
    let mut attachments = attachments.to_vec();
    attachments.sort_by_key(|attachment| match attachment.site {
        ScriptSite::Object { id, kind } => (
            0,
            u32::from(kind == ObjectScript::Critter),
            id,
            0,
            0,
            attachment.script_index,
        ),
        ScriptSite::Spatial {
            hex,
            elevation,
            radius,
        } => (
            1,
            0,
            hex as u32,
            u32::from(elevation),
            radius as u32,
            attachment.script_index,
        ),
    });
    let mut found: BTreeMap<u32, Vec<ObjectPath>> = BTreeMap::new();
    for (elevation, objects) in map.objects.elevations.iter().enumerate() {
        index_objects(objects, &mut Vec::new(), elevation, &mut found);
    }

    // The records by category (active ones), and the serials in use.
    let mut records: [Vec<MapScript>; 5] =
        std::array::from_fn(|category| map.scripts[category].active().cloned().collect());
    let mut used: [BTreeSet<u32>; 5] = std::array::from_fn(|category| {
        records[category]
            .iter()
            .filter(|record| record.id >> 24 == category as u32)
            .map(|record| record.id & 0x00ff_ffff)
            .collect()
    });
    let mut changed = [false; 5];
    // By record id, the first record's script index (the one lookups
    // find); and the spatial records by (script, packed place, radius).
    let mut script_of: BTreeMap<u32, u32> = BTreeMap::new();
    for record in records.iter().flatten() {
        script_of
            .entry(record.id)
            .or_insert(record.file_index as u32);
    }
    let mut spatial: BTreeSet<(u32, u32, i32)> = records[SPATIAL_STREAM]
        .iter()
        .filter_map(|record| match record.prefix {
            ScriptPrefix::Spatial { location, radius } => {
                Some((record.file_index as u32, location.0, radius))
            }
            _ => None,
        })
        .collect();
    let mut bindings: BTreeMap<ObjectPath, (u32, u32)> = BTreeMap::new();

    let new_serial = |used: &mut BTreeSet<u32>| -> Result<u32, EmitError> {
        let serial = (1..0x0100_0000)
            .find(|serial| !used.contains(serial))
            .ok_or_else(|| EmitError::Map("no free script serial".into()))?;
        used.insert(serial);
        Ok(serial)
    };

    for attachment in &attachments {
        match attachment.site {
            ScriptSite::Object { id, kind } => {
                let category = match kind {
                    ObjectScript::Item => ITEM_STREAM,
                    ObjectScript::Critter => CRITTER_STREAM,
                };
                let paths = found
                    .get(&id)
                    .ok_or_else(|| EmitError::Map(format!("object id {id} is not on the map")))?;
                if paths.len() != 1 {
                    return bad(format!("object id {id} is on {} objects", paths.len()));
                }
                let path = paths[0].clone();
                let object = object_at(map, &path);
                let is_critter = Category::from_pid(object.pid).ok() == Some(Category::Critter);
                if is_critter != (kind == ObjectScript::Critter) {
                    return bad(format!(
                        "object {id} (PID {:#010x}) {} a critter, and the script is a {} script",
                        object.pid,
                        if is_critter { "is" } else { "is not" },
                        if is_critter { "item" } else { "critter" },
                    ));
                }
                // An earlier attachment of this call, or an earlier run, may
                // have bound it already.
                let current = bindings.get(&path).map(|(_, index)| *index).or_else(|| {
                    (object.script_id >= 0)
                        .then(|| script_of.get(&(object.script_id as u32)).copied())
                        .flatten()
                });
                if let Some(index) = current {
                    if index == attachment.script_index {
                        continue;
                    }
                    return bad(format!(
                        "object {id} already has script {index}, not {}",
                        attachment.script_index
                    ));
                }
                if object.script_id >= 0 {
                    return bad(format!(
                        "object {id} names script record {:#010x}, which the map does not hold",
                        object.script_id
                    ));
                }
                let serial = new_serial(&mut used[category])?;
                let script_id = ((category as u32) << 24) | serial;
                let mut record = blank_script_slot();
                record.id = script_id;
                record.file_index = attachment.script_index as i32;
                record.object_id = id as i32;
                record.local_base = -1;
                record.stored_local_count = attachment.local_vars as i32;
                record.local_count = written_locals(attachment.local_vars);
                record.action = -1;
                records[category].push(record);
                changed[category] = true;
                bindings.insert(path, (script_id, attachment.script_index));
            }
            ScriptSite::Spatial {
                hex,
                elevation,
                radius,
            } => {
                let location = PackedLocation(hex as u32 | (u32::from(elevation) << 29));
                if !spatial.insert((attachment.script_index, location.0, radius)) {
                    continue;
                }
                let serial = new_serial(&mut used[SPATIAL_STREAM])?;
                let mut record = blank_script_slot();
                record.id = ((SPATIAL_STREAM as u32) << 24) | serial;
                record.prefix = ScriptPrefix::Spatial { location, radius };
                record.file_index = attachment.script_index as i32;
                record.object_id = -1;
                record.local_base = -1;
                record.stored_local_count = attachment.local_vars as i32;
                record.local_count = written_locals(attachment.local_vars);
                record.action = -1;
                records[SPATIAL_STREAM].push(record);
                changed[SPATIAL_STREAM] = true;
            }
        }
    }

    for (category, list) in records.into_iter().enumerate() {
        if changed[category] {
            map.scripts[category] = category_of(list);
        }
    }
    for (path, (script_id, script_index)) in bindings {
        let object = object_at(map, &path);
        object.script_id = script_id as i32;
        object.cached_script_file = script_index as i32;
    }
    if let Some(index) = map_script {
        map.header.script_reference = index as i32 + 1;
    }
    Ok(())
}

/// [`attach_scripts_to`] on a built map's bytes: parse them as a Fallout 2
/// map with `prototypes` (the map's records need their prototypes' sizes),
/// attach, and write the map back through [`MapFile::write`]. A second run
/// with the same attachments returns the same bytes. With nothing to attach
/// the result is the writer's form of the map, which is the input when the
/// input came from `MapFile::write`.
pub fn attach_scripts(
    map: &[u8],
    prototypes: &impl PrototypeLookup,
    attachments: &[ScriptAttachment],
    map_script: Option<u32>,
) -> Result<Vec<u8>, EmitError> {
    let mut parsed = MapFile::parse_for(map, prototypes, GameProfile::Fallout2)?;
    attach_scripts_to(&mut parsed, attachments, map_script)?;
    Ok(parsed.write()?)
}
