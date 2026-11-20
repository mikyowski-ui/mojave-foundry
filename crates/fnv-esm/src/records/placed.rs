//! Placed references: `REFR` (objects), `ACHR` (NPCs) and `ACRE`
//! (creatures).

use serde::{Deserialize, Serialize};

use super::{Common, TypedRecord};
use crate::bytes::{Rd, Wr, one_f32, one_form, one_i32, set};
use crate::formid::FormId;
use crate::header::{RecordFlags, RecordMeta};
use crate::sig::{Sig, sigs};
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// Which record type a placed reference is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PlacedKind {
    /// `REFR`: a placed object.
    #[default]
    Object,
    /// `ACHR`: a placed NPC.
    Npc,
    /// `ACRE`: a placed creature.
    Creature,
}

impl PlacedKind {
    /// The record signature.
    pub fn sig(self) -> Sig {
        match self {
            Self::Object => sigs::REFR,
            Self::Npc => sigs::ACHR,
            Self::Creature => sigs::ACRE,
        }
    }
}

/// Position and rotation (`DATA`), as stored: game units and radians, in
/// the source space of `docs/contracts.md` section 1.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Placement {
    /// Position `(x, y, z)`.
    pub position: [f32; 3],
    /// Rotation `(rx, ry, rz)` in radians.
    pub rotation: [f32; 3],
}

/// A door's teleport destination (`XTEL`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Teleport {
    /// The destination door reference.
    pub door: FormId,
    /// Where the traveller arrives.
    pub position: [f32; 3],
    /// The traveller's rotation on arrival, radians.
    pub rotation: [f32; 3],
    /// Flags, as stored; 0 when the stored form is the short 28 bytes.
    pub flags: u32,
}

/// An enable parent (`XESP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EnableParent {
    /// The parent reference.
    pub reference: FormId,
    /// Flags, as stored. See [`EnableParent::FLAG_OPPOSITE`].
    pub flags: u8,
}

impl EnableParent {
    /// Enabled when the parent is disabled, and the reverse.
    pub const FLAG_OPPOSITE: u8 = 0x01;

    /// Whether the enable state is the opposite of the parent's.
    pub fn is_opposite(&self) -> bool {
        self.flags & Self::FLAG_OPPOSITE != 0
    }
}

/// A lock (`XLOC`).
///
/// Checked against a retail master: levels are 0, 25, 50, 75, 100 and 255
/// (needs a key), the key field names keys, and the bytes after the flags
/// are zero. The three bytes after the level and after the flags are
/// padding and hold leftovers, not data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Lock {
    /// The lock level, as stored.
    pub level: u8,
    /// The key; null when there is none.
    pub key: FormId,
    /// Flags, as stored.
    pub flags: u8,
}

/// A primitive volume attached to a reference (`XPRM`, 32 bytes): the
/// shape of a trigger, a room marker or a portal.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Primitive {
    /// Extent along the reference's local X, Y and Z, as stored, in game
    /// units and before the reference's scale. Public documentation
    /// describes these as half-extents (a box reaches from `-bounds` to
    /// `+bounds`; for a sphere the first is the radius); that reading has
    /// not been verified here.
    pub bounds: [f32; 3],
    /// Editor display colour, red, green and blue from 0 to 1.
    pub color: [f32; 3],
    /// The seventh float, as stored; between 0 and 1 wherever it occurs.
    pub unknown: f32,
    /// The shape, as stored. See the `SHAPE_*` constants.
    pub shape: u32,
}

impl Primitive {
    /// A box.
    pub const SHAPE_BOX: u32 = 1;
    /// A sphere.
    pub const SHAPE_SPHERE: u32 = 2;
    /// A portal box.
    pub const SHAPE_PORTAL_BOX: u32 = 3;
}

/// A placed object, NPC or creature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedRef {
    /// The common fields. The record flags carry persistent and initially
    /// disabled.
    pub common: Common,
    /// Which of the three record types this is.
    pub kind: PlacedKind,
    /// The base form being placed (`NAME`).
    pub base: Option<FormId>,
    /// Position and rotation (`DATA`).
    pub placement: Option<Placement>,
    /// Scale (`XSCL`); 1.0 when the record has none.
    pub scale: f32,
    /// Teleport destination (`XTEL`).
    pub teleport: Option<Teleport>,
    /// Enable parent (`XESP`).
    pub enable_parent: Option<EnableParent>,
    /// Lock (`XLOC`).
    pub lock: Option<Lock>,
    /// Item count (`XCNT`).
    pub count: Option<i32>,
    /// Linked reference (`XLKR`).
    pub linked_ref: Option<FormId>,
    /// Owner (`XOWN`).
    pub owner: Option<FormId>,
    /// The owner's required faction rank (`XRNK`).
    pub owner_rank: Option<i32>,
    /// Encounter zone (`XEZN`).
    pub encounter_zone: Option<FormId>,
    /// Radius (`XRDS`), as stored. On a reference to a light it is often
    /// negative, which fits an adjustment to the base light's radius rather
    /// than a radius of its own; elsewhere it is positive.
    pub radius: Option<f32>,
    /// Primitive volume (`XPRM`).
    pub primitive: Option<Primitive>,
    /// Emittance source (`XEMI`): a light or a region.
    pub emittance: Option<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Default for PlacedRef {
    fn default() -> Self {
        Self {
            common: Common::default(),
            kind: PlacedKind::default(),
            base: None,
            placement: None,
            scale: 1.0,
            teleport: None,
            enable_parent: None,
            lock: None,
            count: None,
            linked_ref: None,
            owner: None,
            owner_rank: None,
            encounter_zone: None,
            radius: None,
            primitive: None,
            emittance: None,
            unknown: Vec::new(),
        }
    }
}

impl PlacedRef {
    /// A reference placing `base` at a position with a rotation in radians.
    pub fn new(
        kind: PlacedKind,
        form_id: impl Into<FormId>,
        base: impl Into<FormId>,
        position: [f32; 3],
        rotation: [f32; 3],
    ) -> Self {
        Self {
            common: Common::new(form_id),
            kind,
            base: Some(base.into()),
            placement: Some(Placement { position, rotation }),
            ..Self::default()
        }
    }

    /// Sets the scale.
    #[must_use]
    pub fn with_scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    /// Sets the teleport destination.
    #[must_use]
    pub fn with_teleport(
        mut self,
        door: impl Into<FormId>,
        position: [f32; 3],
        rotation: [f32; 3],
    ) -> Self {
        self.teleport = Some(Teleport {
            door: door.into(),
            position,
            rotation,
            flags: 0,
        });
        self
    }

    /// Sets the enable parent.
    #[must_use]
    pub fn with_enable_parent(mut self, reference: impl Into<FormId>, opposite: bool) -> Self {
        self.enable_parent = Some(EnableParent {
            reference: reference.into(),
            flags: if opposite {
                EnableParent::FLAG_OPPOSITE
            } else {
                0
            },
        });
        self
    }

    /// Whether the persistent record flag is set.
    pub fn is_persistent(&self) -> bool {
        self.common.meta.flags.is_persistent()
    }

    /// Whether the reference starts disabled (the initially-disabled record
    /// flag). An enable parent may still switch it on.
    pub fn is_initially_disabled(&self) -> bool {
        self.common.meta.flags.is_initially_disabled()
    }

    /// Whether the record is marked deleted: a plugin removing a reference
    /// that one of its masters placed.
    pub fn is_deleted(&self) -> bool {
        self.common.meta.flags.is_deleted()
    }

    /// Sets the persistent record flag.
    #[must_use]
    pub fn persistent(mut self) -> Self {
        self.common.meta.flags.0 |= RecordFlags::PERSISTENT;
        self
    }

    /// Sets the initially-disabled record flag.
    #[must_use]
    pub fn initially_disabled(mut self) -> Self {
        self.common.meta.flags.0 |= RecordFlags::INITIALLY_DISABLED;
        self
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        let mut scale = None;
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"NAME" => set(&mut self.base, one_form(d)),
                b"DATA" => set(&mut self.placement, parse_data(d)),
                b"XSCL" => set(&mut scale, one_f32(d)),
                b"XTEL" => set(&mut self.teleport, parse_xtel(d)),
                b"XESP" => set(&mut self.enable_parent, parse_xesp(d)),
                b"XLOC" => set(&mut self.lock, parse_xloc(d)),
                b"XCNT" => set(&mut self.count, one_i32(d)),
                b"XLKR" => set(&mut self.linked_ref, one_form(d)),
                b"XOWN" => set(&mut self.owner, one_form(d)),
                b"XRNK" => set(&mut self.owner_rank, one_i32(d)),
                b"XEZN" => set(&mut self.encounter_zone, one_form(d)),
                b"XRDS" => set(&mut self.radius, one_f32(d)),
                b"XPRM" => set(&mut self.primitive, parse_xprm(d)),
                b"XEMI" => set(&mut self.emittance, one_form(d)),
                _ => false,
            }
        });
        self.scale = scale.unwrap_or(1.0);
    }

    fn emit_fields(&self, out: &mut Out) {
        out.form(b"NAME", self.base);
        out.form(b"XEZN", self.encounter_zone);
        out.opt(
            b"XPRM",
            self.primitive.map(|p| {
                Wr::default()
                    .f32x3(p.bounds)
                    .f32x3(p.color)
                    .f32(p.unknown)
                    .u32(p.shape)
                    .done()
            }),
        );
        out.f32(b"XRDS", self.radius);
        out.form(b"XEMI", self.emittance);
        out.opt(
            b"XTEL",
            self.teleport.map(|t| {
                Wr::default()
                    .form(t.door)
                    .f32x3(t.position)
                    .f32x3(t.rotation)
                    .u32(t.flags)
                    .done()
            }),
        );
        out.form(b"XOWN", self.owner);
        out.i32(b"XRNK", self.owner_rank);
        out.opt(
            b"XLOC",
            self.lock.map(|l| {
                Wr::default()
                    .u8(l.level)
                    .zeros(3)
                    .form(l.key)
                    .u8(l.flags)
                    .zeros(11)
                    .done()
            }),
        );
        out.i32(b"XCNT", self.count);
        out.form(b"XLKR", self.linked_ref);
        out.opt(
            b"XESP",
            self.enable_parent
                .map(|p| Wr::default().form(p.reference).u8(p.flags).zeros(3).done()),
        );
        // Bit comparison, so that a NaN scale is still written.
        if self.scale.to_bits() != 1.0f32.to_bits() {
            out.f32(b"XSCL", Some(self.scale));
        }
        out.opt(
            b"DATA",
            self.placement
                .map(|p| Wr::default().f32x3(p.position).f32x3(p.rotation).done()),
        );
    }
}

impl TypedRecord for PlacedRef {
    fn decode(sig: Sig, meta: RecordMeta, subrecords: &[SubrecordRef<'_>]) -> Self {
        let kind = if sig == sigs::ACHR {
            PlacedKind::Npc
        } else if sig == sigs::ACRE {
            PlacedKind::Creature
        } else {
            PlacedKind::Object
        };
        let mut record = Self {
            common: Common::from_meta(meta),
            kind,
            ..Self::default()
        };
        record.decode_fields(subrecords);
        record
    }

    fn sig(&self) -> Sig {
        self.kind.sig()
    }

    fn common(&self) -> &Common {
        &self.common
    }

    fn unknown(&self) -> &[RawSubrecord] {
        &self.unknown
    }

    fn encode(&self) -> Vec<RawSubrecord> {
        let mut out = Out::default();
        self.common.emit(&mut out);
        self.emit_fields(&mut out);
        out.finish(&self.unknown)
    }
}

fn parse_data(data: &[u8]) -> Option<Placement> {
    let mut rd = Rd::exact(data, 24)?;
    Some(Placement {
        position: rd.f32x3()?,
        rotation: rd.f32x3()?,
    })
}

fn parse_xtel(data: &[u8]) -> Option<Teleport> {
    if !matches!(data.len(), 28 | 32) {
        return None;
    }
    let mut rd = Rd::new(data);
    Some(Teleport {
        door: rd.form()?,
        position: rd.f32x3()?,
        rotation: rd.f32x3()?,
        flags: rd.u32().unwrap_or(0),
    })
}

fn parse_xesp(data: &[u8]) -> Option<EnableParent> {
    let mut rd = Rd::exact(data, 8)?;
    Some(EnableParent {
        reference: rd.form()?,
        flags: rd.u8()?,
    })
}

fn parse_xprm(data: &[u8]) -> Option<Primitive> {
    let mut rd = Rd::exact(data, 32)?;
    Some(Primitive {
        bounds: rd.f32x3()?,
        color: rd.f32x3()?,
        unknown: rd.f32()?,
        shape: rd.u32()?,
    })
}

/// `XLOC` is 20 bytes; the shorter 12- and 16-byte forms share its first
/// 12: level, three unused, key, flags, three unused.
fn parse_xloc(data: &[u8]) -> Option<Lock> {
    if !matches!(data.len(), 12 | 16 | 20) {
        return None;
    }
    let mut rd = Rd::new(data);
    let level = rd.u8()?;
    rd.skip(3)?;
    Some(Lock {
        level,
        key: rd.form()?,
        flags: rd.u8()?,
    })
}
