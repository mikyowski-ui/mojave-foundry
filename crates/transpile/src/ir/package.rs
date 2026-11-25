//! AI packages (contracts 11.1, spec 3.7).

use serde::{Deserialize, Serialize};

use super::expr::Cond;
use super::id::{FormKey, SimId};

/// An AI package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Package {
    /// [`super::SimKind::Package`].
    pub id: SimId,
    /// Editor id, as stored.
    pub editor_id: String,
    /// The type.
    pub kind: PackageKind,
    /// `PKDT` general flags.
    pub flags: u32,
    /// `PKDT` behaviour flags (0 until the extraction decodes them).
    pub behaviour_flags: u16,
    /// `PKDT` type-specific flags (0 until the extraction decodes them).
    pub type_flags: u16,
    /// Where.
    pub location: Option<Location>,
    /// Whom or what.
    pub target: Option<Target>,
    /// When.
    pub schedule: Schedule,
    /// Conditions.
    pub condition: Cond,
    /// Begin, end and change scripts.
    pub events: Vec<(PackageEvent, Option<SimId>)>,
    /// The behaviour that runs it (spec 7.3).
    pub runner: Runner,
}

/// A package type; the stored numbers are in [`PackageKind::from_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    /// 0
    Find,
    /// 1
    Follow,
    /// 2
    Escort,
    /// 3
    Eat,
    /// 4
    Sleep,
    /// 5
    Wander,
    /// 6
    Travel,
    /// 7
    Accompany,
    /// 8
    UseItemAt,
    /// 9
    Ambush,
    /// 10
    FleeNotCombat,
    /// 12
    Sandbox,
    /// 13
    Patrol,
    /// 14
    Guard,
    /// 15
    Dialogue,
    /// 16
    UseWeapon,
    /// Any other stored number.
    Other(u8),
}

impl PackageKind {
    /// The kind of a stored type number.
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Find,
            1 => Self::Follow,
            2 => Self::Escort,
            3 => Self::Eat,
            4 => Self::Sleep,
            5 => Self::Wander,
            6 => Self::Travel,
            7 => Self::Accompany,
            8 => Self::UseItemAt,
            9 => Self::Ambush,
            10 => Self::FleeNotCombat,
            12 => Self::Sandbox,
            13 => Self::Patrol,
            14 => Self::Guard,
            15 => Self::Dialogue,
            16 => Self::UseWeapon,
            other => Self::Other(other),
        }
    }

    /// The runner of spec 7.3's table. Types the table does not name wait.
    pub fn runner(self) -> Runner {
        match self {
            Self::Travel
            | Self::Eat
            | Self::Sleep
            | Self::UseItemAt
            | Self::Find
            | Self::Patrol
            | Self::Ambush => Runner::Travel,
            Self::Sandbox | Self::Wander => Runner::Sandbox,
            Self::Follow | Self::Escort | Self::Accompany => Runner::Follow,
            Self::Guard => Runner::Guard,
            Self::Dialogue => Runner::Wait,
            Self::FleeNotCombat | Self::UseWeapon | Self::Other(_) => Runner::None,
        }
    }
}

/// A package location (`PLDT`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    /// 0: near a reference.
    NearRef {
        /// The reference.
        r: FormKey,
        /// Radius, game units.
        radius: i32,
    },
    /// 1: in a cell.
    InCell {
        /// The cell.
        cell: FormKey,
        /// Radius, game units.
        radius: i32,
    },
    /// 2: near the current location.
    NearCurrent {
        /// Radius, game units.
        radius: i32,
    },
    /// 3: near the editor location.
    NearEditor {
        /// Radius, game units.
        radius: i32,
    },
    /// 4: an object.
    ObjectId(FormKey),
    /// 5: an object type.
    ObjectType(u32),
    /// 6: near the linked reference.
    NearLinkedRef {
        /// Radius, game units.
        radius: i32,
    },
    /// 7: at the package location.
    AtPackageLocation,
}

/// A package target (`PTDT`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// 0: a specific reference.
    Ref(FormKey),
    /// 1: an object id.
    Object(FormKey),
    /// 2: an object type.
    ObjectType(u32),
    /// 3: the linked reference.
    LinkedRef,
}

/// When a package runs; -1 is "any".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// Month, or -1.
    pub month: i8,
    /// Day of the week, or -1.
    pub day_of_week: i8,
    /// Date (0 any).
    pub date: u8,
    /// Hour, or -1.
    pub hour: i8,
    /// Duration in hours.
    pub duration_hours: i32,
}

impl Schedule {
    /// Any time.
    pub const ANY: Schedule = Schedule {
        month: -1,
        day_of_week: -1,
        date: 0,
        hour: -1,
        duration_hours: 0,
    };
}

/// A package event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageEvent {
    /// `POBA`
    Begin,
    /// `POEA`
    End,
    /// `POCA`
    Change,
}

/// The behaviour that runs a package (spec 7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runner {
    /// Walk to a location, then stand.
    Travel,
    /// Walk among candidate hexes.
    Sandbox,
    /// Keep near a target.
    Follow,
    /// Stand facing a direction.
    Guard,
    /// Stand where it is.
    Wait,
    /// Not run.
    None,
}
