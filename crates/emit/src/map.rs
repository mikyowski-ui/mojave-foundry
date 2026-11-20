//! Small builders for `vault_formats::map::MapFile`, which has public
//! fields but no constructor. Serialization is `MapFile::write`, wrapped by
//! [`crate::FormatWriters::map`].

use crate::EmitError;
use crate::proto::stat::MAX_HIT_POINTS;
use vault_formats::fid::CRITTER_CATEGORY;
use vault_formats::map::{
    CritterState, MapFile, MapHeader, MapObject, MapObjects, ObjectDetail, ObjectState,
    ScriptCategory,
};
use vault_formats::pro::{BasicFlags, Category, CombatFlags, Light, MapFlags, PrototypeData};
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
