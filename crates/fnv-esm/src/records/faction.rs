//! Factions (`FACT`): flags and the relations to other factions.
//!
//! Measured on both retail masters (326 factions in Fallout 3, 682 in New
//! Vegas): `DATA` is 4 bytes (flags, second flags, two unused) in most and
//! 1 byte (the first flags) in 35 and 34; each relation is an `XNAM` of 12
//! bytes in all 666 and 1,314 of them: the other faction, a modifier and a
//! group combat reaction (0 neutral, 1 enemy, 2 ally, 3 friend, every
//! value of both masters). The older 8-byte `XNAM` without the reaction
//! is read too, as a relation whose reaction is `None`. Ranks (`RNAM`,
//! `MNAM`, `FNAM`, `INAM`), the crime multiplier (`CNAM`) and the reputation
//! (`WMI1`) stay in `unknown`.

use serde::{Deserialize, Serialize};

use super::{Common, typed_record};
use crate::bytes::{Rd, Wr, push, set};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// A faction's `DATA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FactionData {
    /// The first flag byte: see [`FactionData::HIDDEN_FROM_PC`] and the
    /// other constants.
    pub flags: u8,
    /// The second flag byte; `None` in the 1-byte form, which lacks it.
    pub flags2: Option<u8>,
}

impl FactionData {
    /// Flag: hidden from the player (not listed).
    pub const HIDDEN_FROM_PC: u8 = 0x01;
    /// Flag: evil.
    pub const EVIL: u8 = 0x02;
    /// Flag: special combat (the faction's members share combat help).
    pub const SPECIAL_COMBAT: u8 = 0x04;
    /// Second-byte flag: crimes against the faction are tracked.
    pub const TRACK_CRIME: u8 = 0x01;
    /// Second-byte flag: the faction's vendors buy stolen goods.
    pub const ALLOW_SELL: u8 = 0x02;
}

/// A group combat reaction: how the members of a faction treat the members
/// of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CombatReaction {
    /// 0.
    Neutral,
    /// 1: they attack each other.
    Enemy,
    /// 2: they help each other in combat.
    Ally,
    /// 3: they help each other, and do not attack.
    Friend,
    /// Any other code, as stored.
    Other(u32),
}

impl CombatReaction {
    /// The reaction of a stored code.
    pub fn from_code(code: u32) -> Self {
        match code {
            0 => Self::Neutral,
            1 => Self::Enemy,
            2 => Self::Ally,
            3 => Self::Friend,
            other => Self::Other(other),
        }
    }

    /// The stored code.
    pub fn code(self) -> u32 {
        match self {
            Self::Neutral => 0,
            Self::Enemy => 1,
            Self::Ally => 2,
            Self::Friend => 3,
            Self::Other(code) => code,
        }
    }

    /// `"neutral"`, `"enemy"`, `"ally"`, `"friend"` or `"other"`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Enemy => "enemy",
            Self::Ally => "ally",
            Self::Friend => "friend",
            Self::Other(_) => "other",
        }
    }
}

/// One relation of a faction (`XNAM`): what it thinks of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactionRelation {
    /// The other faction.
    pub faction: FormId,
    /// The disposition modifier, as stored.
    pub modifier: i32,
    /// The group combat reaction, as stored; `None` in the 8-byte form.
    pub combat_reaction: Option<u32>,
}

impl FactionRelation {
    /// The group combat reaction, when stored.
    pub fn reaction(&self) -> Option<CombatReaction> {
        self.combat_reaction.map(CombatReaction::from_code)
    }

    fn parse(data: &[u8]) -> Option<Self> {
        if !matches!(data.len(), 8 | 12) {
            return None;
        }
        let mut rd = Rd::new(data);
        Some(Self {
            faction: rd.form()?,
            modifier: rd.i32()?,
            combat_reaction: rd.u32(),
        })
    }

    fn bytes(&self) -> Vec<u8> {
        let w = Wr::default().form(self.faction).i32(self.modifier);
        match self.combat_reaction {
            Some(reaction) => w.u32(reaction),
            None => w,
        }
        .done()
    }
}

/// A faction (`FACT`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Faction {
    /// The common fields; `full_name` is the faction's name.
    pub common: Common,
    /// Flags (`DATA`).
    pub data: Option<FactionData>,
    /// Relations to other factions (`XNAM`), in file order.
    pub relations: Vec<FactionRelation>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Faction {
    /// A faction with an editor id and no flags set.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            data: Some(FactionData {
                flags: 0,
                flags2: Some(0),
            }),
            ..Self::default()
        }
    }

    /// Sets the flag bytes.
    #[must_use]
    pub fn with_flags(mut self, flags: u8, flags2: u8) -> Self {
        self.data = Some(FactionData {
            flags,
            flags2: Some(flags2),
        });
        self
    }

    /// Adds a relation of the 12-byte form.
    #[must_use]
    pub fn with_relation(
        mut self,
        other: impl Into<FormId>,
        modifier: i32,
        reaction: CombatReaction,
    ) -> Self {
        self.relations.push(FactionRelation {
            faction: other.into(),
            modifier,
            combat_reaction: Some(reaction.code()),
        });
        self
    }

    /// Whether the faction is hidden from the player.
    pub fn is_hidden_from_pc(&self) -> bool {
        self.data
            .is_some_and(|d| d.flags & FactionData::HIDDEN_FROM_PC != 0)
    }

    /// Whether the faction is evil.
    pub fn is_evil(&self) -> bool {
        self.data.is_some_and(|d| d.flags & FactionData::EVIL != 0)
    }

    /// Whether the faction has special combat.
    pub fn has_special_combat(&self) -> bool {
        self.data
            .is_some_and(|d| d.flags & FactionData::SPECIAL_COMBAT != 0)
    }

    /// Whether crimes against the faction are tracked; clear in the 1-byte
    /// form.
    pub fn tracks_crime(&self) -> bool {
        self.data
            .and_then(|d| d.flags2)
            .is_some_and(|f| f & FactionData::TRACK_CRIME != 0)
    }

    /// Whether the faction's vendors buy stolen goods; clear in the 1-byte
    /// form.
    pub fn allows_sell(&self) -> bool {
        self.data
            .and_then(|d| d.flags2)
            .is_some_and(|f| f & FactionData::ALLOW_SELL != 0)
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"DATA" if matches!(d.len(), 1 | 4) => set(
                    &mut self.data,
                    Some(FactionData {
                        flags: d[0],
                        flags2: (d.len() == 4).then(|| d[1]),
                    }),
                ),
                b"XNAM" => push(&mut self.relations, FactionRelation::parse(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.opt(
            b"DATA",
            self.data.map(|d| match d.flags2 {
                Some(flags2) => vec![d.flags, flags2, 0, 0],
                None => vec![d.flags],
            }),
        );
        for relation in &self.relations {
            out.raw(b"XNAM", relation.bytes());
        }
    }
}

typed_record!(Faction, FACT);
