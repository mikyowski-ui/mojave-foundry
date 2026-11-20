//! Leveled lists (`LVLN`, `LVLC`, `LVLI`) and form lists (`FLST`).

use serde::{Deserialize, Serialize};

use super::{Common, TypedRecord, typed_record};
use crate::bytes::{Rd, Wr, one_form, one_u8, set};
use crate::formid::FormId;
use crate::header::RecordMeta;
use crate::sig::{Sig, sigs};
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// Which record type a leveled list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum LeveledKind {
    /// `LVLN`: a leveled NPC.
    #[default]
    Npc,
    /// `LVLC`: a leveled creature.
    Creature,
    /// `LVLI`: a leveled item.
    Item,
}

impl LeveledKind {
    /// The record signature.
    pub fn sig(self) -> Sig {
        match self {
            Self::Npc => sigs::LVLN,
            Self::Creature => sigs::LVLC,
            Self::Item => sigs::LVLI,
        }
    }
}

/// Extra data of one entry (`COED`, 12 bytes, after its `LVLO`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LeveledExtra {
    /// The owner (an NPC or a faction); null when there is none.
    pub owner: FormId,
    /// The global variable or the required faction rank, as stored.
    pub value: u32,
    /// The item's condition, 0 to 1.
    pub condition: f32,
}

/// One entry of a leveled list (`LVLO`, 12 bytes: level, two unused bytes,
/// form, count, two unused bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LeveledEntry {
    /// The player level from which the entry can be picked.
    pub level: i16,
    /// The form: an actor, an item or another leveled list.
    pub form: FormId,
    /// How many.
    pub count: i16,
    /// Extra data (`COED`).
    pub extra: Option<LeveledExtra>,
}

impl LeveledEntry {
    /// An entry with no extra data.
    pub fn new(level: i16, form: impl Into<FormId>, count: i16) -> Self {
        Self {
            level,
            form: form.into(),
            count,
            extra: None,
        }
    }
}

/// A leveled list: `LVLN`, `LVLC` or `LVLI`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LeveledList {
    /// The common fields. `model` is the editor's preview model of a
    /// leveled actor.
    pub common: Common,
    /// Which of the three record types this is.
    pub kind: LeveledKind,
    /// Chance in percent that the list gives nothing (`LVLD`).
    pub chance_none: Option<u8>,
    /// Flags (`LVLF`), as stored. See the `FLAG_*` constants.
    pub flags: Option<u8>,
    /// A global variable that overrides the chance of none (`LVLG`).
    pub global: Option<FormId>,
    /// The entries (`LVLO`), in file order.
    pub entries: Vec<LeveledEntry>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl LeveledList {
    /// Picks from every entry at or below the player's level, not only the
    /// closest one.
    pub const FLAG_ALL_LEVELS: u8 = 0x01;
    /// Picks again for each unit of the count.
    pub const FLAG_EACH: u8 = 0x02;
    /// Gives every entry (`LVLI` only).
    pub const FLAG_USE_ALL: u8 = 0x04;

    /// A list of one kind with an editor id and entries.
    pub fn new(
        kind: LeveledKind,
        form_id: impl Into<FormId>,
        editor_id: &str,
        entries: Vec<LeveledEntry>,
    ) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            kind,
            chance_none: Some(0),
            flags: Some(0),
            entries,
            ..Self::default()
        }
    }

    /// Whether the list gives every entry rather than one.
    pub fn uses_all(&self) -> bool {
        self.flags.unwrap_or(0) & Self::FLAG_USE_ALL != 0
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"LVLD" => set(&mut self.chance_none, one_u8(d)),
                b"LVLF" => set(&mut self.flags, one_u8(d)),
                b"LVLG" => set(&mut self.global, one_form(d)),
                b"LVLO" => match parse_lvlo(d) {
                    Some(entry) => {
                        self.entries.push(entry);
                        true
                    }
                    None => false,
                },
                b"COED" => match self.entries.last_mut() {
                    Some(entry) => set(&mut entry.extra, parse_coed(d)),
                    None => false,
                },
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.u8(b"LVLD", self.chance_none);
        out.u8(b"LVLF", self.flags);
        out.form(b"LVLG", self.global);
        for entry in &self.entries {
            out.raw(
                b"LVLO",
                Wr::default()
                    .i16(entry.level)
                    .zeros(2)
                    .form(entry.form)
                    .i16(entry.count)
                    .zeros(2)
                    .done(),
            );
            out.opt(
                b"COED",
                entry.extra.map(|e| {
                    Wr::default()
                        .form(e.owner)
                        .u32(e.value)
                        .f32(e.condition)
                        .done()
                }),
            );
        }
    }
}

impl TypedRecord for LeveledList {
    fn decode(sig: Sig, meta: RecordMeta, subrecords: &[SubrecordRef<'_>]) -> Self {
        let kind = if sig == sigs::LVLC {
            LeveledKind::Creature
        } else if sig == sigs::LVLI {
            LeveledKind::Item
        } else {
            LeveledKind::Npc
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

fn parse_lvlo(data: &[u8]) -> Option<LeveledEntry> {
    let mut rd = Rd::exact(data, 12)?;
    let level = rd.i16()?;
    rd.skip(2)?;
    Some(LeveledEntry {
        level,
        form: rd.form()?,
        count: rd.i16()?,
        extra: None,
    })
}

fn parse_coed(data: &[u8]) -> Option<LeveledExtra> {
    let mut rd = Rd::exact(data, 12)?;
    Some(LeveledExtra {
        owner: rd.form()?,
        value: rd.u32()?,
        condition: rd.f32()?,
    })
}

/// A form list (`FLST`): forms in order, one `LNAM` each.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FormList {
    /// The common fields.
    pub common: Common,
    /// The forms (`LNAM`), in file order.
    pub forms: Vec<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl FormList {
    /// A list with an editor id and forms.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, forms: Vec<FormId>) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            forms,
            unknown: Vec::new(),
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            match &s.sig.0 {
                b"LNAM" => match one_form(s.data) {
                    Some(form) => {
                        self.forms.push(form);
                        true
                    }
                    None => false,
                },
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.form_each(b"LNAM", &self.forms);
    }
}

typed_record!(FormList, FLST);
