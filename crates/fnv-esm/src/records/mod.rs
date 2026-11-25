//! Typed records and the table that maps a record signature to its decoder.
//!
//! # Decoding rules
//!
//! - A decoder never fails. A subrecord it does not know, a repeat of one it
//!   expects once, or one whose size matches no layout it knows, is left
//!   untouched in the typed record's `unknown` list, in file order. Nothing
//!   is dropped.
//! - Padding and unused bytes inside a decoded struct are not kept in the
//!   typed form. [`crate::RecordRef::to_raw`] gives the lossless view.
//! - Every typed record can be written back with [`TypedRecord::encode`].
//!   For a value built in code, `decode(encode(x)) == x`. Encoding writes
//!   the decoded fields in a fixed order and then the `unknown` list, so it
//!   is not a byte-exact copy of a record read from a file.

use serde::{Deserialize, Serialize};

use crate::bytes::{Rd, Wr, one_form, set, zstring};
use crate::formid::FormId;
use crate::header::{RecordFlags, RecordMeta};
use crate::sig::{Sig, sigs};
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// Runs a decoder's per-subrecord expression over `$subs`. A subrecord the
/// expression does not use is offered to the common fields, then kept in
/// `unknown`.
macro_rules! decode_loop {
    ($record:ident, $subs:ident, |$sub:ident| $field:expr) => {
        for $sub in $subs {
            let used: bool = $field;
            if !used && !$record.common.take($sub) {
                $record.unknown.push($sub.to_raw());
            }
        }
    };
}

mod actors;
mod appearance;
mod condition;
mod dialogue;
mod faction;
mod land;
mod leveled;
mod message;
mod navmesh;
mod note;
mod objects;
mod package;
mod placed;
mod quest;
mod script;
mod settings;
mod terminal;
mod world;

pub use actors::{
    ActorBase, AiData, Creature, CreatureData, FactionRank, Npc, NpcSkills, NpcStats, Rgb,
};
pub use appearance::{Eyes, Hair, HeadPart, Race, RaceData, RacePart, RaceParts};
pub use condition::{
    Comparison, Condition, ConditionValue, REFERENCE_PARAMETER_FUNCTIONS, reference_parameter_slots,
};
pub use dialogue::{
    Info, InfoData, Response, ResponseData, Topic, TopicData, speech_difficulty_name,
};
pub use faction::{CombatReaction, Faction, FactionData, FactionRelation};
pub use land::{
    AlphaLayer, AlphaPoint, BaseLayer, LAND_GRID, LAND_VERTICES, Land, LandHavok, LandTexture,
    QUADRANT_GRID, TextureSet, decode_vhgt, encode_vhgt, quadrant_origin,
};
pub use leveled::{FormList, LeveledEntry, LeveledExtra, LeveledKind, LeveledList};
pub use message::{Message, MessageButton};
pub use navmesh::{NavDoorLink, NavEdge, NavExternalLink, NavTriangle, Navmesh};
pub use note::Note;
pub use objects::{
    Ammo, AmmoData, AmmoData2, Armor, ArmorAddon, ArmorData, ArmorStats, BipedData, Container,
    ContainerData, ContainerItem, Door, Effect, EffectData, Ingestible, IngestibleData, ItemData,
    MiscItem, Static, Weapon, WeaponCombat, WeaponCritical, WeaponData,
};
pub use package::{
    BEHAVIOUR_FLAG_NAMES, GENERAL_FLAG_NAMES, Package, PackageData, PackageDialogue, PackageEvent,
    PackageLocation, PackageSchedule, PackageTarget, SANDBOX_FLAG_NAMES,
};
pub use placed::{EnableParent, Lock, PlacedKind, PlacedRef, Placement, Primitive, Teleport};
pub use quest::{Quest, QuestData, QuestLogEntry, QuestObjective, QuestStage, QuestTarget};
pub use script::{LocalVariable, Script, ScriptBody, ScriptHeader, ScriptReference};
pub use settings::{GameSetting, GameSettingValue, Global, GlobalType};
pub use terminal::{Terminal, TerminalData, TerminalMenuItem};
pub use world::{
    Cell, CellGrid, CellLighting, LandDefaults, MapData, MapOffset, World, WorldBounds,
};

/// A record as an owned header and subrecord list, nothing decoded.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RawRecord {
    /// The record's signature.
    pub sig: Sig,
    /// The header fields.
    pub meta: RecordMeta,
    /// The subrecords, in file order.
    pub subrecords: Vec<RawSubrecord>,
}

impl RawRecord {
    /// An empty record.
    pub fn new(sig: &[u8; 4], form_id: impl Into<FormId>) -> Self {
        Self {
            sig: Sig::new(sig),
            meta: RecordMeta::new(form_id),
            subrecords: Vec::new(),
        }
    }

    /// Adds a subrecord.
    #[must_use]
    pub fn with(mut self, sig: &[u8; 4], data: impl Into<Vec<u8>>) -> Self {
        self.subrecords.push(RawSubrecord::new(sig, data));
        self
    }

    /// Decodes through the decoder table, as [`crate::RecordRef::decode`]
    /// does.
    pub fn decode(&self) -> Record {
        let subs: Vec<SubrecordRef<'_>> = self.subrecords.iter().map(RawSubrecord::view).collect();
        decode_record(self.sig, self.meta, &subs)
    }
}

/// Object bounds (`OBND`): two corners in game units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ObjectBounds {
    /// The minimum corner, `(x, y, z)`.
    pub min: [i16; 3],
    /// The maximum corner, `(x, y, z)`.
    pub max: [i16; 3],
}

impl ObjectBounds {
    fn parse(data: &[u8]) -> Option<Self> {
        let mut rd = Rd::exact(data, 12)?;
        Some(Self {
            min: [rd.i16()?, rd.i16()?, rd.i16()?],
            max: [rd.i16()?, rd.i16()?, rd.i16()?],
        })
    }

    fn bytes(&self) -> Vec<u8> {
        self.min
            .iter()
            .chain(&self.max)
            .fold(Wr::default(), |w, &v| w.i16(v))
            .done()
    }
}

/// The fields every typed record has.
///
/// `EDID`, `FULL`, `MODL`, `SCRI` and `OBND` are read here for every record
/// type, the first occurrence of each. On a placed reference that carries a
/// map marker, `full_name` is the marker's name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Common {
    /// Form id, record flags and the other header fields.
    pub meta: RecordMeta,
    /// The editor id (`EDID`).
    pub editor_id: Option<String>,
    /// The display name (`FULL`).
    pub full_name: Option<String>,
    /// The model path (`MODL`), as stored.
    pub model: Option<String>,
    /// The attached script (`SCRI`).
    pub script: Option<FormId>,
    /// The object bounds (`OBND`).
    pub bounds: Option<ObjectBounds>,
}

impl Common {
    /// Common fields for a new record with the given form id.
    pub fn new(form_id: impl Into<FormId>) -> Self {
        Self::from_meta(RecordMeta::new(form_id))
    }

    pub(crate) fn from_meta(meta: RecordMeta) -> Self {
        Self {
            meta,
            ..Self::default()
        }
    }

    /// The record's form id, as stored.
    pub fn form_id(&self) -> FormId {
        self.meta.form_id
    }

    /// The record flags.
    pub fn flags(&self) -> RecordFlags {
        self.meta.flags
    }

    /// Sets the editor id.
    #[must_use]
    pub fn with_editor_id(mut self, editor_id: &str) -> Self {
        self.editor_id = Some(editor_id.to_owned());
        self
    }

    /// Sets the display name.
    #[must_use]
    pub fn with_name(mut self, name: &str) -> Self {
        self.full_name = Some(name.to_owned());
        self
    }

    /// Sets the model path.
    #[must_use]
    pub fn with_model(mut self, model: &str) -> Self {
        self.model = Some(model.to_owned());
        self
    }

    /// Sets the attached script.
    #[must_use]
    pub fn with_script(mut self, script: impl Into<FormId>) -> Self {
        self.script = Some(script.into());
        self
    }

    /// Sets the object bounds.
    #[must_use]
    pub fn with_bounds(mut self, min: [i16; 3], max: [i16; 3]) -> Self {
        self.bounds = Some(ObjectBounds { min, max });
        self
    }

    /// Sets record flag bits, for example [`RecordFlags::COMPRESSED`] to
    /// have the fixture builder compress the record.
    #[must_use]
    pub fn with_flags(mut self, bits: u32) -> Self {
        self.meta.flags.0 |= bits;
        self
    }

    pub(crate) fn take(&mut self, sub: &SubrecordRef<'_>) -> bool {
        match &sub.sig.0 {
            b"EDID" => set(&mut self.editor_id, zstring(sub.data)),
            b"FULL" => set(&mut self.full_name, zstring(sub.data)),
            b"MODL" => set(&mut self.model, zstring(sub.data)),
            b"SCRI" => set(&mut self.script, one_form(sub.data)),
            b"OBND" => set(&mut self.bounds, ObjectBounds::parse(sub.data)),
            _ => false,
        }
    }

    pub(crate) fn emit(&self, out: &mut Out) {
        out.zstr(b"EDID", &self.editor_id);
        out.opt(b"OBND", self.bounds.map(|b| b.bytes()));
        out.zstr(b"FULL", &self.full_name);
        out.zstr(b"MODL", &self.model);
        out.form(b"SCRI", self.script);
    }
}

/// What every typed record can do.
pub trait TypedRecord: Sized + Into<Record> {
    /// Decodes a record of this type. `sig` is the record's signature; it
    /// matters only to types that cover several signatures.
    fn decode(sig: Sig, meta: RecordMeta, subrecords: &[SubrecordRef<'_>]) -> Self;

    /// The signature this record is written under.
    fn sig(&self) -> Sig;

    /// The common fields.
    fn common(&self) -> &Common;

    /// The subrecords the decoder did not consume, in file order.
    fn unknown(&self) -> &[RawSubrecord];

    /// The record's subrecords: decoded fields in a fixed order, then
    /// [`TypedRecord::unknown`].
    fn encode(&self) -> Vec<RawSubrecord>;

    /// The record as a header and subrecord list.
    fn to_raw(&self) -> RawRecord {
        RawRecord {
            sig: self.sig(),
            meta: self.common().meta,
            subrecords: self.encode(),
        }
    }
}

/// The decoder of one record signature.
pub type DecodeFn = fn(Sig, RecordMeta, &[SubrecordRef<'_>]) -> Record;

/// One row of the decoder table.
#[derive(Debug, Clone, Copy)]
pub struct DecoderEntry {
    /// The record signature.
    pub sig: Sig,
    /// Name of the Rust type the record decodes to.
    pub type_name: &'static str,
    /// The decoder.
    pub decode: DecodeFn,
}

macro_rules! record_table {
    (
        core { $( $(#[$doc:meta])* $variant:ident($ty:ident) : $($sig:ident)|+ ; )* }
        logic { $( $(#[$ldoc:meta])* $lvariant:ident($lty:ident) : $($lsig:ident)|+ ; )* }
    ) => {
        record_table!(@inner
            [ $( $(#[$doc])* $variant($ty) : $($sig)|+ ; )* ]
            [ $( $(#[$ldoc])* $lvariant($lty) : $($lsig)|+ ; )* ]
            [ $( $(#[$doc])* $variant($ty) : $($sig)|+ ; )* $( $(#[$ldoc])* $lvariant($lty) : $($lsig)|+ ; )* ]
        );
    };
    (@inner
        [ $( $(#[$cdoc:meta])* $cvariant:ident($cty:ident) : $($csig:ident)|+ ; )* ]
        [ $( $(#[$gdoc:meta])* $gvariant:ident($gty:ident) : $($gsig:ident)|+ ; )* ]
        [ $( $(#[$doc:meta])* $variant:ident($ty:ident) : $($sig:ident)|+ ; )* ]
    ) => {
        /// A decoded record: one variant per typed record, and
        /// [`Record::Unknown`] for every signature the tables do not list.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub enum Record {
            $( $(#[$doc])* $variant($ty), )*
            /// A record type with no decoder, kept whole.
            Unknown(RawRecord),
        }

        $(
            impl From<$ty> for Record {
                fn from(record: $ty) -> Self {
                    Record::$variant(record)
                }
            }
        )*

        impl From<RawRecord> for Record {
            fn from(record: RawRecord) -> Self {
                Record::Unknown(record)
            }
        }

        /// The decoder table of the world, object, actor and dialogue
        /// records: one row per record signature with a typed decoder.
        /// [`LOGIC_DECODERS`] has the rest; [`decoder_for`] and
        /// [`all_decoders`] look in both.
        pub static DECODERS: &[DecoderEntry] = &[
            $( $(
                DecoderEntry {
                    sig: sigs::$csig,
                    type_name: stringify!($cty),
                    decode: |sig, meta, subs| {
                        Record::$cvariant(<$cty as TypedRecord>::decode(sig, meta, subs))
                    },
                },
            )+ )*
        ];

        /// The second decoder table: terminals, notes, AI packages,
        /// global variables, game settings, messages and factions, added
        /// for the logic a converter follows (what scripts, quests and
        /// packages point at, and the numbers they use). Kept apart from
        /// [`DECODERS`], whose rows the earlier decoders' tests
        /// enumerate.
        pub static LOGIC_DECODERS: &[DecoderEntry] = &[
            $( $(
                DecoderEntry {
                    sig: sigs::$gsig,
                    type_name: stringify!($gty),
                    decode: |sig, meta, subs| {
                        Record::$gvariant(<$gty as TypedRecord>::decode(sig, meta, subs))
                    },
                },
            )+ )*
        ];

        impl Record {
            /// The record's signature.
            pub fn sig(&self) -> Sig {
                match self {
                    $( Record::$variant(r) => TypedRecord::sig(r), )*
                    Record::Unknown(r) => r.sig,
                }
            }

            /// The common fields; `None` for [`Record::Unknown`].
            pub fn common(&self) -> Option<&Common> {
                match self {
                    $( Record::$variant(r) => Some(TypedRecord::common(r)), )*
                    Record::Unknown(_) => None,
                }
            }

            /// The header fields.
            pub fn meta(&self) -> &RecordMeta {
                match self {
                    $( Record::$variant(r) => &TypedRecord::common(r).meta, )*
                    Record::Unknown(r) => &r.meta,
                }
            }

            /// The subrecords no decoder consumed. For [`Record::Unknown`]
            /// that is all of them.
            pub fn unknown_subrecords(&self) -> &[RawSubrecord] {
                match self {
                    $( Record::$variant(r) => TypedRecord::unknown(r), )*
                    Record::Unknown(r) => &r.subrecords,
                }
            }

            /// The record as a header and subrecord list.
            pub fn to_raw(&self) -> RawRecord {
                match self {
                    $( Record::$variant(r) => TypedRecord::to_raw(r), )*
                    Record::Unknown(r) => r.clone(),
                }
            }
        }
    };
}

record_table! {
    core {
    /// `WRLD`: a worldspace.
    World(World): WRLD;
    /// `CELL`: an interior or exterior cell.
    Cell(Cell): CELL;
    /// `LAND`: the terrain of an exterior cell.
    Land(Land): LAND;
    /// `NAVM`: a navigation mesh.
    Navmesh(Navmesh): NAVM;
    /// `REFR`, `ACHR`, `ACRE`: a placed object, NPC or creature.
    Placed(PlacedRef): REFR | ACHR | ACRE;
    /// `STAT`: a static object.
    Static(Static): STAT;
    /// `DOOR`: a door.
    Door(Door): DOOR;
    /// `CONT`: a container.
    Container(Container): CONT;
    /// `MISC`: a miscellaneous item.
    Misc(MiscItem): MISC;
    /// `WEAP`: a weapon.
    Weapon(Weapon): WEAP;
    /// `ARMO`: a piece of armour.
    Armor(Armor): ARMO;
    /// `ALCH`: an ingestible.
    Ingestible(Ingestible): ALCH;
    /// `AMMO`: ammunition.
    Ammo(Ammo): AMMO;
    /// `NPC_`: a non-player character.
    Npc(Npc): NPC_;
    /// `CREA`: a creature.
    Creature(Creature): CREA;
    /// `QUST`: a quest.
    Quest(Quest): QUST;
    /// `DIAL`: a dialogue topic.
    Topic(Topic): DIAL;
    /// `INFO`: a dialogue response group.
    Info(Info): INFO;
    /// `SCPT`: a script.
    Script(Script): SCPT;
    /// `LTEX`: a land texture.
    LandTexture(LandTexture): LTEX;
    /// `TXST`: a texture set.
    TextureSet(TextureSet): TXST;
    /// `RACE`: a race, with its head and body parts.
    Race(Race): RACE;
    /// `HDPT`: a head part.
    HeadPart(HeadPart): HDPT;
    /// `HAIR`: a hair style.
    Hair(Hair): HAIR;
    /// `EYES`: eyes.
    Eyes(Eyes): EYES;
    /// `ARMA`: an armour addon.
    ArmorAddon(ArmorAddon): ARMA;
    /// `FLST`: a form list.
    FormList(FormList): FLST;
    /// `LVLN`, `LVLC`, `LVLI`: a leveled NPC, creature or item list.
    Leveled(LeveledList): LVLN | LVLC | LVLI;
    }
    logic {
    /// `TERM`: a terminal.
    Terminal(Terminal): TERM;
    /// `NOTE`: a note or holotape.
    Note(Note): NOTE;
    /// `PACK`: an AI package.
    Package(Package): PACK;
    /// `GLOB`: a global variable.
    Global(Global): GLOB;
    /// `GMST`: a game setting.
    GameSetting(GameSetting): GMST;
    /// `MESG`: a message.
    Message(Message): MESG;
    /// `FACT`: a faction.
    Faction(Faction): FACT;
    }
}

impl Record {
    /// The record's form id, as stored.
    pub fn form_id(&self) -> FormId {
        self.meta().form_id
    }

    /// The editor id, when the record is typed and has one.
    pub fn editor_id(&self) -> Option<&str> {
        self.common().and_then(|c| c.editor_id.as_deref())
    }
}

impl Record {
    /// Every script embedded in the record, as `(context, body)`, in file
    /// order: `"script"` for a script record, `"stage <index> entry <n>"`
    /// for a quest stage's log entry, `"result begin"` and `"result end"`
    /// for a dialogue response, `"item <n>"` for a terminal menu item and
    /// `"on begin"`, `"on end"` or `"on change"` for a package event.
    /// A script that is blank ([`ScriptBody::is_blank`]: nothing but, at
    /// most, its header) is left out.
    pub fn script_bodies(&self) -> Vec<(String, &ScriptBody)> {
        let mut out: Vec<(String, &ScriptBody)> = Vec::new();
        match self {
            Record::Script(s) => out.push(("script".to_owned(), &s.body)),
            Record::Quest(q) => {
                for stage in &q.stages {
                    for (n, entry) in stage.entries.iter().enumerate() {
                        out.push((format!("stage {} entry {n}", stage.index), &entry.script));
                    }
                }
            }
            Record::Info(i) => {
                out.push(("result begin".to_owned(), &i.result_begin));
                out.push(("result end".to_owned(), &i.result_end));
            }
            Record::Terminal(t) => {
                for (n, item) in t.items.iter().enumerate() {
                    out.push((format!("item {n}"), &item.script));
                }
            }
            Record::Package(p) => {
                for (name, event) in p.events() {
                    out.push((name.to_owned(), &event.script));
                }
            }
            _ => {}
        }
        out.retain(|(_, body)| !body.is_blank());
        out
    }
}

/// The table row for a record signature, if it has a typed decoder, from
/// [`DECODERS`] or [`LOGIC_DECODERS`].
pub fn decoder_for(sig: Sig) -> Option<&'static DecoderEntry> {
    all_decoders().find(|entry| entry.sig == sig)
}

/// Every row of both decoder tables: [`DECODERS`], then
/// [`LOGIC_DECODERS`].
pub fn all_decoders() -> impl Iterator<Item = &'static DecoderEntry> {
    DECODERS.iter().chain(LOGIC_DECODERS)
}

/// Decodes a record from its header fields and subrecords through the
/// decoder table. A signature with no row gives [`Record::Unknown`].
pub fn decode_record(sig: Sig, meta: RecordMeta, subrecords: &[SubrecordRef<'_>]) -> Record {
    match decoder_for(sig) {
        Some(entry) => (entry.decode)(sig, meta, subrecords),
        None => Record::Unknown(RawRecord {
            sig,
            meta,
            subrecords: subrecords.iter().map(SubrecordRef::to_raw).collect(),
        }),
    }
}

/// Implements [`TypedRecord`] for a type with `common` and `unknown` fields
/// and inherent `decode_fields` / `emit_fields` functions.
macro_rules! typed_record {
    ($ty:ident, $sig:ident) => {
        impl $crate::records::TypedRecord for $ty {
            fn decode(
                _sig: $crate::sig::Sig,
                meta: $crate::header::RecordMeta,
                subrecords: &[$crate::subrecord::SubrecordRef<'_>],
            ) -> Self {
                let mut record = Self {
                    common: $crate::records::Common::from_meta(meta),
                    ..Self::default()
                };
                record.decode_fields(subrecords);
                record
            }

            fn sig(&self) -> $crate::sig::Sig {
                $crate::sig::sigs::$sig
            }

            fn common(&self) -> &$crate::records::Common {
                &self.common
            }

            fn unknown(&self) -> &[$crate::subrecord::RawSubrecord] {
                &self.unknown
            }

            fn encode(&self) -> Vec<$crate::subrecord::RawSubrecord> {
                let mut out = $crate::subrecord::Out::default();
                self.common.emit(&mut out);
                self.emit_fields(&mut out);
                out.finish(&self.unknown)
            }
        }
    };
}
pub(crate) use typed_record;
