//! AI packages (`PACK`): what an actor does, where, when and to what.
//!
//! Measured on both retail masters (3,266 packages in Fallout 3, 4,163 in
//! New Vegas), which share the layout: `PKDT` (8 or 12 bytes), locations
//! `PLDT` and `PLD2` (12), the schedule `PSDT` (8), targets `PTDT` and
//! `PTD2` (12 or 16), `CTDA` conditions, dialogue data `PKDD` (12 to 24),
//! the combat style `CNAM`, and three event blocks opened by the empty
//! markers `POBA` (on begin), `POEA` (on end) and `POCA` (on change), each
//! with an idle `INAM`, an embedded script and a topic `TNAM`.
//!
//! The idle lists (`IDLA` and the rest), weapon data (`PKW3`), patrol,
//! escort, eat and flee data stay in `unknown`.

use serde::{Deserialize, Serialize};

use super::{Common, Condition, ScriptBody, typed_record};
use crate::bytes::{Rd, Wr, one_form, set};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// A package's `PKDT`: the general flags and the package type, then the
/// remaining bytes as stored.
///
/// # Layout
///
/// Measured on both retail masters (3,021 packages of Fallout 3 and 4,150
/// of New Vegas have the 12-byte form, 245 and 13 the 8-byte one):
///
/// | Offset | Size | Field |
/// |---:|---:|---|
/// | 0 | 4 | general flags ([`PackageData::flags`]) |
/// | 4 | 1 | the package type |
/// | 5 | 1 | unused (0 or the uninitialised-memory fill `0xCD`) |
/// | 6 | 2 | behaviour flags ([`PackageData::behaviour_flags`]) |
/// | 8 | 2 | type-specific flags ([`PackageData::type_flags`]); 12-byte form only |
/// | 10 | 2 | unused; 12-byte form only |
///
/// [`PackageData::rest`] holds the bytes from offset 5 on as stored, 3 in
/// the 8-byte form and 7 in the 12-byte one; the accessors read it. Many
/// packages have `0xCD` fill bytes (uninitialised memory of the editor that
/// wrote them) in the unused places, and in the behaviour flags of some;
/// [`PackageData::behaviour_flags`] reads that fill as "not set" instead
/// of as flags.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PackageData {
    /// General flags, as stored; see the `OFFERS_SERVICES` family of
    /// constants and [`PackageData::general_flag_names`].
    pub flags: u32,
    /// The package type; see [`PackageData::type_name`].
    pub package_type: u8,
    /// The bytes after the type, as stored.
    pub rest: Vec<u8>,
}

/// The general flags of a `PKDT`, by bit, with the editor's names.
///
/// The names are the check-box labels of the editor's package dialog,
/// matched to bits by the public format documentation and checked for
/// plausibility on the masters (travel and escort packages carry "must
/// reach location" far more often than sleep packages, use-weapon packages
/// all have "weapons drawn", guards often "no warn/attack"). Bits 11, 14,
/// 15 and 16 are not named.
pub const GENERAL_FLAG_NAMES: &[(u32, &str)] = &[
    (PackageData::OFFERS_SERVICES, "offers services"),
    (PackageData::MUST_REACH_LOCATION, "must reach location"),
    (PackageData::MUST_COMPLETE, "must complete"),
    (
        PackageData::LOCK_DOORS_AT_START,
        "lock doors at package start",
    ),
    (PackageData::LOCK_DOORS_AT_END, "lock doors at package end"),
    (
        PackageData::LOCK_DOORS_AT_LOCATION,
        "lock doors at location",
    ),
    (
        PackageData::UNLOCK_DOORS_AT_START,
        "unlock doors at package start",
    ),
    (
        PackageData::UNLOCK_DOORS_AT_END,
        "unlock doors at package end",
    ),
    (
        PackageData::UNLOCK_DOORS_AT_LOCATION,
        "unlock doors at location",
    ),
    (PackageData::CONTINUE_IF_PC_NEAR, "continue if PC near"),
    (PackageData::ONCE_PER_DAY, "once per day"),
    (
        PackageData::SKIP_FALLOUT_BEHAVIOUR,
        "skip Fallout behaviour",
    ),
    (PackageData::ALWAYS_RUN, "always run"),
    (PackageData::ALWAYS_SNEAK, "always sneak"),
    (PackageData::ALLOW_SWIMMING, "allow swimming"),
    (PackageData::ALLOW_FALLS, "allow falls"),
    (PackageData::ARMOR_UNEQUIPPED, "armor unequipped"),
    (PackageData::WEAPONS_UNEQUIPPED, "weapons unequipped"),
    (PackageData::DEFENSIVE_COMBAT, "defensive combat"),
    (PackageData::WEAPONS_DRAWN, "weapons drawn"),
    (PackageData::NO_IDLE_ANIMS, "no idle animations"),
    (PackageData::PRETEND_IN_COMBAT, "pretend in combat"),
    (
        PackageData::CONTINUE_DURING_COMBAT,
        "continue during combat",
    ),
    (PackageData::NO_COMBAT_ALERT, "no combat alert"),
    (PackageData::NO_WARN_ATTACK, "no warn/attack"),
];

/// The behaviour flags of a `PKDT` (offset 6), by bit, with the editor's
/// names for the Fallout behaviour options.
pub const BEHAVIOUR_FLAG_NAMES: &[(u16, &str)] = &[
    (PackageData::HELLOS_TO_PLAYER, "hellos to player"),
    (PackageData::RANDOM_CONVERSATIONS, "random conversations"),
    (
        PackageData::OBSERVE_COMBAT_BEHAVIOUR,
        "observe combat behaviour",
    ),
    (
        PackageData::GREET_CORPSE_BEHAVIOUR,
        "greet corpse behaviour",
    ),
    (
        PackageData::REACTION_TO_PLAYER_ACTIONS,
        "reaction to player actions",
    ),
    (
        PackageData::FRIENDLY_FIRE_COMMENTS,
        "friendly fire comments",
    ),
    (
        PackageData::AGGRO_RADIUS_BEHAVIOUR,
        "aggro radius behaviour",
    ),
    (PackageData::ALLOW_IDLE_CHATTER, "allow idle chatter"),
    (PackageData::AVOID_RADIATION, "avoid radiation"),
];

/// The type-specific flags of a sandbox or wander package (offset 8), by
/// bit. Measured on the masters: sandbox packages use bits 0 to 5 (eating
/// and sleeping and wandering most), wander packages the same; no other
/// package type's bits are named here.
pub const SANDBOX_FLAG_NAMES: &[(u16, &str)] = &[
    (PackageData::SANDBOX_ALLOW_EATING, "allow eating"),
    (PackageData::SANDBOX_ALLOW_SLEEPING, "allow sleeping"),
    (
        PackageData::SANDBOX_ALLOW_CONVERSATIONS,
        "allow conversations",
    ),
    (
        PackageData::SANDBOX_ALLOW_IDLE_MARKERS,
        "allow idle markers",
    ),
    (
        PackageData::SANDBOX_ALLOW_FURNITURE_USE,
        "allow furniture use",
    ),
    (PackageData::SANDBOX_ALLOW_WANDERING, "allow wandering"),
];

impl PackageData {
    /// General flag: the actor offers services (barter, repair, ...).
    pub const OFFERS_SERVICES: u32 = 0x0000_0001;
    /// General flag: the package ends only when the actor reaches its
    /// location.
    pub const MUST_REACH_LOCATION: u32 = 0x0000_0002;
    /// General flag: the package cannot be interrupted by a later one.
    pub const MUST_COMPLETE: u32 = 0x0000_0004;
    /// General flag: lock doors at the package start.
    pub const LOCK_DOORS_AT_START: u32 = 0x0000_0008;
    /// General flag: lock doors at the package end.
    pub const LOCK_DOORS_AT_END: u32 = 0x0000_0010;
    /// General flag: lock doors at the location.
    pub const LOCK_DOORS_AT_LOCATION: u32 = 0x0000_0020;
    /// General flag: unlock doors at the package start.
    pub const UNLOCK_DOORS_AT_START: u32 = 0x0000_0040;
    /// General flag: unlock doors at the package end.
    pub const UNLOCK_DOORS_AT_END: u32 = 0x0000_0080;
    /// General flag: unlock doors at the location.
    pub const UNLOCK_DOORS_AT_LOCATION: u32 = 0x0000_0100;
    /// General flag: continue if the player is near.
    pub const CONTINUE_IF_PC_NEAR: u32 = 0x0000_0200;
    /// General flag: the package runs once a day.
    pub const ONCE_PER_DAY: u32 = 0x0000_0400;
    /// General flag: skip the Fallout-specific behaviour (greetings,
    /// chatter).
    pub const SKIP_FALLOUT_BEHAVIOUR: u32 = 0x0000_1000;
    /// General flag: run instead of walking.
    pub const ALWAYS_RUN: u32 = 0x0000_2000;
    /// General flag: sneak.
    pub const ALWAYS_SNEAK: u32 = 0x0002_0000;
    /// General flag: allow swimming.
    pub const ALLOW_SWIMMING: u32 = 0x0004_0000;
    /// General flag: allow falls.
    pub const ALLOW_FALLS: u32 = 0x0008_0000;
    /// General flag: armour unequipped.
    pub const ARMOR_UNEQUIPPED: u32 = 0x0010_0000;
    /// General flag: weapons unequipped.
    pub const WEAPONS_UNEQUIPPED: u32 = 0x0020_0000;
    /// General flag: defensive combat.
    pub const DEFENSIVE_COMBAT: u32 = 0x0040_0000;
    /// General flag: weapons drawn.
    pub const WEAPONS_DRAWN: u32 = 0x0080_0000;
    /// General flag: no idle animations.
    pub const NO_IDLE_ANIMS: u32 = 0x0100_0000;
    /// General flag: pretend to be in combat.
    pub const PRETEND_IN_COMBAT: u32 = 0x0200_0000;
    /// General flag: continue during combat.
    pub const CONTINUE_DURING_COMBAT: u32 = 0x0400_0000;
    /// General flag: no combat alert.
    pub const NO_COMBAT_ALERT: u32 = 0x0800_0000;
    /// General flag: no warn or attack.
    pub const NO_WARN_ATTACK: u32 = 0x1000_0000;

    /// Behaviour flag: hellos to the player.
    pub const HELLOS_TO_PLAYER: u16 = 0x0001;
    /// Behaviour flag: random conversations.
    pub const RANDOM_CONVERSATIONS: u16 = 0x0002;
    /// Behaviour flag: observe combat behaviour.
    pub const OBSERVE_COMBAT_BEHAVIOUR: u16 = 0x0004;
    /// Behaviour flag: greet corpse behaviour.
    pub const GREET_CORPSE_BEHAVIOUR: u16 = 0x0008;
    /// Behaviour flag: reaction to player actions.
    pub const REACTION_TO_PLAYER_ACTIONS: u16 = 0x0010;
    /// Behaviour flag: friendly fire comments.
    pub const FRIENDLY_FIRE_COMMENTS: u16 = 0x0020;
    /// Behaviour flag: aggro radius behaviour.
    pub const AGGRO_RADIUS_BEHAVIOUR: u16 = 0x0040;
    /// Behaviour flag: allow idle chatter.
    pub const ALLOW_IDLE_CHATTER: u16 = 0x0080;
    /// Behaviour flag: avoid radiation.
    pub const AVOID_RADIATION: u16 = 0x0100;

    /// The behaviour flags' uninitialised-memory fill, which the older
    /// packages store.
    pub const BEHAVIOUR_FILL: u16 = 0xCDCD;

    /// Sandbox and wander flag: allow eating.
    pub const SANDBOX_ALLOW_EATING: u16 = 0x0001;
    /// Sandbox and wander flag: allow sleeping.
    pub const SANDBOX_ALLOW_SLEEPING: u16 = 0x0002;
    /// Sandbox and wander flag: allow conversations.
    pub const SANDBOX_ALLOW_CONVERSATIONS: u16 = 0x0004;
    /// Sandbox and wander flag: allow idle markers.
    pub const SANDBOX_ALLOW_IDLE_MARKERS: u16 = 0x0008;
    /// Sandbox and wander flag: allow furniture use.
    pub const SANDBOX_ALLOW_FURNITURE_USE: u16 = 0x0010;
    /// Sandbox and wander flag: allow wandering.
    pub const SANDBOX_ALLOW_WANDERING: u16 = 0x0020;

    /// The package type of a sandbox package.
    pub const TYPE_SANDBOX: u8 = 12;
    /// The package type of a wander package.
    pub const TYPE_WANDER: u8 = 5;

    /// A `PKDT` of the 12-byte form: the type, general flags, behaviour
    /// flags and type-specific flags.
    pub fn new(package_type: u8, flags: u32, behaviour: u16, type_flags: u16) -> Self {
        let rest = Wr::default()
            .u8(0)
            .u16(behaviour)
            .u16(type_flags)
            .zeros(2)
            .done();
        Self {
            flags,
            package_type,
            rest,
        }
    }

    /// A `PKDT` of the 8-byte form, which has no type-specific flags.
    pub fn short(package_type: u8, flags: u32, behaviour: u16) -> Self {
        let rest = Wr::default().u8(0).u16(behaviour).done();
        Self {
            flags,
            package_type,
            rest,
        }
    }

    /// Whether this is the 8-byte form, which has no type-specific flags.
    pub fn is_short_form(&self) -> bool {
        self.rest.len() == 3
    }

    /// Whether every bit of `mask` is set in the general flags.
    pub fn has_flags(&self, mask: u32) -> bool {
        self.flags & mask == mask
    }

    /// The editor's names of the general flags that are set, in bit order.
    pub fn general_flag_names(&self) -> Vec<&'static str> {
        GENERAL_FLAG_NAMES
            .iter()
            .filter(|(mask, _)| self.flags & mask != 0)
            .map(|&(_, name)| name)
            .collect()
    }

    /// The general flag "offers services".
    pub fn offers_services(&self) -> bool {
        self.has_flags(Self::OFFERS_SERVICES)
    }

    /// The general flag "must reach location".
    pub fn must_reach_location(&self) -> bool {
        self.has_flags(Self::MUST_REACH_LOCATION)
    }

    /// The general flag "must complete".
    pub fn must_complete(&self) -> bool {
        self.has_flags(Self::MUST_COMPLETE)
    }

    /// The general flag "always run": the actor runs, else it walks.
    pub fn always_run(&self) -> bool {
        self.has_flags(Self::ALWAYS_RUN)
    }

    /// The general flag "always sneak".
    pub fn always_sneak(&self) -> bool {
        self.has_flags(Self::ALWAYS_SNEAK)
    }

    /// The general flag "once per day".
    pub fn once_per_day(&self) -> bool {
        self.has_flags(Self::ONCE_PER_DAY)
    }

    /// The general flag "continue if PC near".
    pub fn continue_if_pc_near(&self) -> bool {
        self.has_flags(Self::CONTINUE_IF_PC_NEAR)
    }

    /// The general flag "weapons drawn".
    pub fn weapons_drawn(&self) -> bool {
        self.has_flags(Self::WEAPONS_DRAWN)
    }

    /// The behaviour flags as stored, fill included; `None` when `PKDT`
    /// has no bytes after the type.
    pub fn behaviour_flags_raw(&self) -> Option<u16> {
        let bytes: [u8; 2] = self.rest.get(1..3)?.try_into().ok()?;
        Some(u16::from_le_bytes(bytes))
    }

    /// The behaviour flags, or `None` when the record stores the
    /// uninitialised-memory fill [`PackageData::BEHAVIOUR_FILL`] (an
    /// engine bug's leftover: those bits were never chosen).
    pub fn behaviour_flags(&self) -> Option<u16> {
        self.behaviour_flags_raw()
            .filter(|&flags| flags != Self::BEHAVIOUR_FILL)
    }

    /// The editor's names of the behaviour flags that are set.
    pub fn behaviour_flag_names(&self) -> Vec<&'static str> {
        let flags = self.behaviour_flags().unwrap_or(0);
        BEHAVIOUR_FLAG_NAMES
            .iter()
            .filter(|(mask, _)| flags & mask != 0)
            .map(|&(_, name)| name)
            .collect()
    }

    /// The type-specific flags; `None` in the 8-byte form, which has none.
    pub fn type_flags(&self) -> Option<u16> {
        let bytes: [u8; 2] = self.rest.get(3..5)?.try_into().ok()?;
        Some(u16::from_le_bytes(bytes))
    }

    /// Whether the type-specific flags are the sandbox set (a sandbox or
    /// wander package).
    pub fn has_sandbox_flags(&self) -> bool {
        matches!(self.package_type, Self::TYPE_SANDBOX | Self::TYPE_WANDER)
    }

    /// One sandbox or wander option: `None` for any other package type
    /// and in the 8-byte form.
    pub fn sandbox_flag(&self, mask: u16) -> Option<bool> {
        if !self.has_sandbox_flags() {
            return None;
        }
        self.type_flags().map(|flags| flags & mask != 0)
    }

    /// The editor's names of the sandbox options that are set, in bit
    /// order; empty for any other package type.
    pub fn sandbox_flag_names(&self) -> Vec<&'static str> {
        match self.type_flags().filter(|_| self.has_sandbox_flags()) {
            Some(flags) => SANDBOX_FLAG_NAMES
                .iter()
                .filter(|(mask, _)| flags & mask != 0)
                .map(|&(_, name)| name)
                .collect(),
            None => Vec::new(),
        }
    }

    /// The package type's name in the editor's list, for the codes both
    /// games use: find, follow, escort, eat, sleep, wander, travel,
    /// accompany, use item at, ambush, flee not combat, sandbox, patrol,
    /// guard, dialogue and use weapon.
    pub fn type_name(&self) -> Option<&'static str> {
        Some(match self.package_type {
            0 => "find",
            1 => "follow",
            2 => "escort",
            3 => "eat",
            4 => "sleep",
            5 => "wander",
            6 => "travel",
            7 => "accompany",
            8 => "use item at",
            9 => "ambush",
            10 => "flee not combat",
            12 => "sandbox",
            13 => "patrol",
            14 => "guard",
            15 => "dialogue",
            16 => "use weapon",
            _ => return None,
        })
    }

    fn parse(data: &[u8]) -> Option<Self> {
        if !matches!(data.len(), 8 | 12) {
            return None;
        }
        let mut rd = Rd::new(data);
        Some(Self {
            flags: rd.u32()?,
            package_type: rd.u8()?,
            rest: data.get(5..)?.to_vec(),
        })
    }

    fn bytes(&self) -> Vec<u8> {
        Wr::default()
            .u32(self.flags)
            .u8(self.package_type)
            .bytes(&self.rest)
            .done()
    }
}

/// Where a package takes place (`PLDT`, `PLD2`; 12 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PackageLocation {
    /// The location type, as stored: 0 near a reference, 1 in a cell,
    /// 2 near the current location, 3 near the editor location, 4 an object
    /// id, 5 an object type, 6 near the linked reference, 7 at the
    /// package's location.
    pub location_type: i32,
    /// A form id for types 0, 1 and 4, an object type code for type 5, and
    /// unused otherwise; as stored.
    pub value: u32,
    /// The radius, game units.
    pub radius: i32,
}

impl PackageLocation {
    /// Near a placed reference.
    pub const NEAR_REFERENCE: i32 = 0;
    /// In a cell.
    pub const IN_CELL: i32 = 1;
    /// A base object.
    pub const OBJECT_ID: i32 = 4;

    /// The form the location names, for the types whose value is one.
    pub fn form(&self) -> Option<FormId> {
        matches!(
            self.location_type,
            Self::NEAR_REFERENCE | Self::IN_CELL | Self::OBJECT_ID
        )
        .then_some(FormId(self.value))
        .filter(|f| !f.is_null())
    }

    fn parse(data: &[u8]) -> Option<Self> {
        let mut rd = Rd::exact(data, 12)?;
        Some(Self {
            location_type: rd.i32()?,
            value: rd.u32()?,
            radius: rd.i32()?,
        })
    }

    fn bytes(&self) -> Vec<u8> {
        Wr::default()
            .i32(self.location_type)
            .u32(self.value)
            .i32(self.radius)
            .done()
    }
}

/// When a package runs (`PSDT`, 8 bytes). −1 means "any".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PackageSchedule {
    /// Month, 0 to 11, or −1.
    pub month: i8,
    /// Day of the week, 0 Sunday to 6, or −1 (and higher codes for
    /// weekdays, weekends and the like).
    pub day_of_week: i8,
    /// Day of the month; 0 is any.
    pub date: u8,
    /// Starting hour, 0 to 23, or −1.
    pub hour: i8,
    /// Duration in hours.
    pub duration: i32,
}

impl PackageSchedule {
    fn parse(data: &[u8]) -> Option<Self> {
        let mut rd = Rd::exact(data, 8)?;
        Some(Self {
            month: rd.u8()? as i8,
            day_of_week: rd.u8()? as i8,
            date: rd.u8()?,
            hour: rd.u8()? as i8,
            duration: rd.i32()?,
        })
    }

    fn bytes(&self) -> Vec<u8> {
        Wr::default()
            .u8(self.month as u8)
            .u8(self.day_of_week as u8)
            .u8(self.date)
            .u8(self.hour as u8)
            .i32(self.duration)
            .done()
    }
}

/// What a package acts on (`PTDT`, `PTD2`): 16 bytes, or 12 in an older
/// form (26 packages of the Fallout 3 master) without the last field.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PackageTarget {
    /// The target type, as stored: 0 a specific reference, 1 an object id,
    /// 2 an object type, 3 the linked reference.
    pub target_type: i32,
    /// A form id for types 0 and 1, an object type code for type 2, unused
    /// otherwise; as stored.
    pub value: u32,
    /// The count or distance, as stored.
    pub count: i32,
    /// The last field of the 16-byte form, as stored; `None` in the
    /// 12-byte form.
    pub extra: Option<f32>,
}

impl PackageTarget {
    /// A specific placed reference.
    pub const SPECIFIC_REFERENCE: i32 = 0;
    /// A base object.
    pub const OBJECT_ID: i32 = 1;

    /// The form the target names, for the types whose value is one.
    pub fn form(&self) -> Option<FormId> {
        matches!(self.target_type, Self::SPECIFIC_REFERENCE | Self::OBJECT_ID)
            .then_some(FormId(self.value))
            .filter(|f| !f.is_null())
    }

    fn parse(data: &[u8]) -> Option<Self> {
        if !matches!(data.len(), 12 | 16) {
            return None;
        }
        let mut rd = Rd::new(data);
        Some(Self {
            target_type: rd.i32()?,
            value: rd.u32()?,
            count: rd.i32()?,
            extra: rd.f32(),
        })
    }

    fn bytes(&self) -> Vec<u8> {
        let w = Wr::default()
            .i32(self.target_type)
            .u32(self.value)
            .i32(self.count);
        match self.extra {
            Some(extra) => w.f32(extra),
            None => w,
        }
        .done()
    }
}

/// A dialogue package's `PKDD`: field of view and topic, then the rest as
/// stored. The record is 24 bytes in nearly every package of both masters;
/// 12, 16 and 20 occur in a few of Fallout 3's.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PackageDialogue {
    /// Field of view, degrees.
    pub fov: f32,
    /// The topic; null when there is none.
    pub topic: FormId,
    /// The bytes after the topic, as stored.
    pub rest: Vec<u8>,
}

impl PackageDialogue {
    fn parse(data: &[u8]) -> Option<Self> {
        if !matches!(data.len(), 12 | 16 | 20 | 24) {
            return None;
        }
        let mut rd = Rd::new(data);
        Some(Self {
            fov: rd.f32()?,
            topic: rd.form()?,
            rest: data.get(8..)?.to_vec(),
        })
    }

    fn bytes(&self) -> Vec<u8> {
        Wr::default()
            .f32(self.fov)
            .form(self.topic)
            .bytes(&self.rest)
            .done()
    }
}

/// One of a package's event blocks: on begin (`POBA`), on end (`POEA`) or
/// on change (`POCA`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PackageEvent {
    /// The idle played (`INAM`); null when there is none.
    pub idle: Option<FormId>,
    /// The script run; empty when there is none.
    pub script: ScriptBody,
    /// The topic said (`TNAM`); null when there is none.
    pub topic: Option<FormId>,
}

impl PackageEvent {
    /// An event with nothing in it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the idle.
    #[must_use]
    pub fn with_idle(mut self, idle: impl Into<FormId>) -> Self {
        self.idle = Some(idle.into());
        self
    }

    /// Sets the topic.
    #[must_use]
    pub fn with_topic(mut self, topic: impl Into<FormId>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Sets the script to one with source text.
    #[must_use]
    pub fn with_script_source(mut self, source: &str) -> Self {
        self.script = ScriptBody::from_source(source);
        self
    }

    /// Sets the whole script, for one with references and variables.
    #[must_use]
    pub fn with_script(mut self, script: ScriptBody) -> Self {
        self.script = script;
        self
    }
}

/// An AI package (`PACK`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Package {
    /// The common fields.
    pub common: Common,
    /// Flags and type (`PKDT`).
    pub data: Option<PackageData>,
    /// The location (`PLDT`).
    pub location: Option<PackageLocation>,
    /// The second location (`PLD2`).
    pub location2: Option<PackageLocation>,
    /// The schedule (`PSDT`).
    pub schedule: Option<PackageSchedule>,
    /// The target (`PTDT`).
    pub target: Option<PackageTarget>,
    /// The second target (`PTD2`).
    pub target2: Option<PackageTarget>,
    /// Conditions (`CTDA`), in file order.
    pub conditions: Vec<Condition>,
    /// Dialogue data (`PKDD`).
    pub dialogue: Option<PackageDialogue>,
    /// The combat style (`CNAM`).
    pub combat_style: Option<FormId>,
    /// The on-begin event (`POBA` and what follows it).
    pub on_begin: Option<PackageEvent>,
    /// The on-end event (`POEA` and what follows it).
    pub on_end: Option<PackageEvent>,
    /// The on-change event (`POCA` and what follows it).
    pub on_change: Option<PackageEvent>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

/// Which event block the decoder is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    None,
    Begin,
    End,
    Change,
}

impl Package {
    /// A package of a type, at any time, with no location.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, package_type: u8) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            data: Some(PackageData {
                flags: 0,
                package_type,
                rest: vec![0; 7],
            }),
            schedule: Some(PackageSchedule {
                month: -1,
                day_of_week: -1,
                date: 0,
                hour: -1,
                duration: 0,
            }),
            ..Self::default()
        }
    }

    /// Replaces the `PKDT`.
    #[must_use]
    pub fn with_data(mut self, data: PackageData) -> Self {
        self.data = Some(data);
        self
    }

    /// Sets the on-begin event.
    #[must_use]
    pub fn with_on_begin(mut self, event: PackageEvent) -> Self {
        self.on_begin = Some(event);
        self
    }

    /// Sets the on-end event.
    #[must_use]
    pub fn with_on_end(mut self, event: PackageEvent) -> Self {
        self.on_end = Some(event);
        self
    }

    /// Sets the on-change event.
    #[must_use]
    pub fn with_on_change(mut self, event: PackageEvent) -> Self {
        self.on_change = Some(event);
        self
    }

    /// The event blocks that are present, in file order, with their names:
    /// `"on begin"`, `"on end"`, `"on change"`.
    pub fn events(&self) -> impl Iterator<Item = (&'static str, &PackageEvent)> {
        [
            ("on begin", &self.on_begin),
            ("on end", &self.on_end),
            ("on change", &self.on_change),
        ]
        .into_iter()
        .filter_map(|(name, event)| event.as_ref().map(|e| (name, e)))
    }

    fn event(&mut self, block: Block) -> Option<&mut PackageEvent> {
        match block {
            Block::None => None,
            Block::Begin => self.on_begin.as_mut(),
            Block::End => self.on_end.as_mut(),
            Block::Change => self.on_change.as_mut(),
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        let mut block = Block::None;
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"PKDT" => set(&mut self.data, PackageData::parse(d)),
                b"PLDT" => set(&mut self.location, PackageLocation::parse(d)),
                b"PLD2" => set(&mut self.location2, PackageLocation::parse(d)),
                b"PSDT" => set(&mut self.schedule, PackageSchedule::parse(d)),
                b"PTDT" => set(&mut self.target, PackageTarget::parse(d)),
                b"PTD2" => set(&mut self.target2, PackageTarget::parse(d)),
                b"CTDA" => match Condition::parse(d) {
                    Some(condition) => {
                        self.conditions.push(condition);
                        true
                    }
                    None => false,
                },
                b"PKDD" => set(&mut self.dialogue, PackageDialogue::parse(d)),
                b"CNAM" => set(&mut self.combat_style, one_form(d)),
                b"POBA" | b"POEA" | b"POCA" if d.is_empty() => {
                    let (slot, next) = match &s.sig.0 {
                        b"POBA" => (&mut self.on_begin, Block::Begin),
                        b"POEA" => (&mut self.on_end, Block::End),
                        _ => (&mut self.on_change, Block::Change),
                    };
                    if slot.is_none() {
                        *slot = Some(PackageEvent::default());
                        block = next;
                        true
                    } else {
                        block = Block::None;
                        false
                    }
                }
                b"INAM" => match (one_form(d), self.event(block)) {
                    (Some(form), Some(event)) => set(&mut event.idle, Some(form)),
                    _ => false,
                },
                b"TNAM" => match (one_form(d), self.event(block)) {
                    (Some(form), Some(event)) => set(&mut event.topic, Some(form)),
                    _ => false,
                },
                b"SCHR" | b"SCDA" | b"SCTX" | b"SLSD" | b"SCVR" | b"SCRO" | b"SCRV" => {
                    match self.event(block) {
                        Some(event) => event.script.take(s),
                        None => false,
                    }
                }
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.opt(b"PKDT", self.data.as_ref().map(PackageData::bytes));
        out.opt(b"PLDT", self.location.map(|l| l.bytes()));
        out.opt(b"PLD2", self.location2.map(|l| l.bytes()));
        out.opt(b"PSDT", self.schedule.map(|s| s.bytes()));
        out.opt(b"PTDT", self.target.map(|t| t.bytes()));
        out.opt(b"PTD2", self.target2.map(|t| t.bytes()));
        for condition in &self.conditions {
            out.raw(b"CTDA", condition.bytes());
        }
        out.opt(b"PKDD", self.dialogue.as_ref().map(PackageDialogue::bytes));
        out.form(b"CNAM", self.combat_style);
        for (marker, event) in [
            (b"POBA", &self.on_begin),
            (b"POEA", &self.on_end),
            (b"POCA", &self.on_change),
        ] {
            if let Some(event) = event {
                out.raw(marker, Vec::new());
                out.form(b"INAM", event.idle);
                event.script.emit(out);
                out.form(b"TNAM", event.topic);
            }
        }
    }
}

typed_record!(Package, PACK);
