//! Stairs, ladders and elevators: scenery subtypes 1 to 4
//! (`vault_formats::pro::SceneryData`, SPEC-003 §2.5).
//!
//! **The engine never reads these destinations.** Stairs and ladder
//! instances store packed destination words, and an elevator its type and
//! level, but every move or map change is made by the object's
//! `use_p_proc` script (SPEC-013 §3.21, §3.23). Without a script, using
//! one shows `proto.msg` 480 and nothing moves. The fields are still
//! written correctly so that a script (or a later engine) can read them,
//! and so the map record reads back. For a transition that works without
//! a script, use an exit grid (`ObjectDetail::ExitGrid`).
//!
//! Each placed object copies its destination into its own record
//! ([`crate::objects::transit_object`]): stairs and Fallout 2 ladders hold
//! the destination map and the packed destination, elevators the type and
//! level (SPEC-003 §2.11, SPEC-016 §2).

use crate::EmitError;
use crate::proto::{ART_SCENERY, ProtoCommon, check_common, finish};
use vault_formats::Prototype;
use vault_formats::pro::{Category, ExtendedFlags, PrototypeData, Scenery, SceneryData};

/// Scenery extended flag `0x01`: the user reaches to the ground (10)
/// rather than to middle height (11) when using it (SPEC-007 §3.11.3,
/// SPEC-013 §3.23).
pub const GROUND_REACH: u32 = 0x01;

/// A packed destination (SPEC-003 §2.1): hex in bits 0 to 25,
/// orientation in 26 to 28, elevation in 29 to 31, plus the map number
/// the format keeps beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Destination {
    /// A `maps.txt` record number (SPEC-018 §4.2); stored as given.
    pub map: i32,
    /// 0 to 39,999.
    pub hex: u32,
    /// 0 to 2.
    pub elevation: u8,
    /// 0 to 5; the ordinary encoding leaves it 0.
    pub orientation: u8,
}

impl Destination {
    /// The packed word: `hex + 2^26 orientation + 2^29 elevation`.
    pub fn packed(self) -> Result<u32, EmitError> {
        if self.hex >= 40_000 || self.elevation > 2 || self.orientation > 5 {
            return Err(EmitError::Prototype(format!(
                "destination hex {} elevation {} orientation {} is outside 0-39999, 0-2, 0-5",
                self.hex, self.elevation, self.orientation
            )));
        }
        Ok(self.hex | (u32::from(self.orientation) << 26) | (u32::from(self.elevation) << 29))
    }
}

fn transit(
    common: &ProtoCommon,
    sound: u8,
    ground_reach: bool,
    data: SceneryData,
) -> Result<Prototype, EmitError> {
    check_common(common, Category::Scenery, ART_SCENERY)?;
    let mut extended = ExtendedFlags::USE.0;
    if ground_reach {
        extended |= GROUND_REACH;
    }
    Ok(finish(
        common,
        Category::Scenery,
        ExtendedFlags(extended),
        PrototypeData::Scenery(Scenery {
            material: common.material as i32,
            stored_byte: sound,
            data,
        }),
    ))
}

/// Stairs (scenery subtype 1): `SceneryData::Stairs { map, destination }`,
/// with the use permission and, when `ground_reach`, the ground reach.
/// `sound` is the scenery sound byte.
pub fn stairs(
    common: ProtoCommon,
    destination: Destination,
    ground_reach: bool,
    sound: u8,
) -> Result<Prototype, EmitError> {
    let data = SceneryData::Stairs {
        map: destination.map,
        destination: destination.packed()?,
    };
    transit(&common, sound, ground_reach, data)
}

/// Which way a ladder leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderKind {
    /// Subtype 3: the user climbs (animation 4) from the hex south-east
    /// of it (SPEC-013 §3.23).
    Up,
    /// Subtype 4: used like other scenery, with a reach.
    Down,
}

/// A ladder (scenery subtype 3 or 4). The prototype stores the packed
/// destination only; `destination.map` is for the placed record, which
/// under Fallout 2 holds the map as well (SPEC-016 §2), and is not
/// stored here.
pub fn ladder(
    common: ProtoCommon,
    kind: LadderKind,
    destination: Destination,
    ground_reach: bool,
    sound: u8,
) -> Result<Prototype, EmitError> {
    let packed = destination.packed()?;
    let data = match kind {
        LadderKind::Up => SceneryData::LadderUp {
            destination: packed,
        },
        LadderKind::Down => SceneryData::LadderDown {
            destination: packed,
        },
    };
    transit(&common, sound, ground_reach, data)
}

/// An elevator (scenery subtype 2): `SceneryData::Elevator { kind, level
/// }`, the elevator table's type (0 to 11 are the engine's, SPEC-013
/// §2.7) and level. Both are loaded and saved but never read: an elevator
/// is called by a script (`metarule` 15, SPEC-013 §3.21).
pub fn elevator(
    common: ProtoCommon,
    kind: i32,
    level: i32,
    sound: u8,
) -> Result<Prototype, EmitError> {
    if !(0..=11).contains(&kind) || level < 0 {
        return Err(EmitError::Prototype(format!(
            "elevator type {kind} or level {level} is outside 0-11, 0 or more"
        )));
    }
    transit(&common, sound, false, SceneryData::Elevator { kind, level })
}
