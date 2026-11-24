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
/// remaining bytes as stored (3 in the 8-byte form, 7 in the 12-byte one;
/// they hold more flags whose layout is not settled here).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PackageData {
    /// General flags, as stored.
    pub flags: u32,
    /// The package type; see [`PackageData::type_name`].
    pub package_type: u8,
    /// The bytes after the type, as stored.
    pub rest: Vec<u8>,
}

impl PackageData {
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
