//! What the build reads: plain, serialisable copies of the MIR records it
//! needs, with every form resolved to a [`FormKey`]. [`load`] fills it
//! from a MIR; tests fill it by hand. Every map is keyed by form and every
//! list is in file order, so [`crate::build::build_module`] is a pure
//! function of it.
//!
//! The fields that the extraction does not decode yet (task T1: package
//! flags, `INFO` `KNAM`, typed `GLOB`/`MESG`/`FACT`, AI data) are here
//! with the shape the build expects; [`load`] fills what today's MIR has.

mod corpus;
mod load;
mod resolve_form;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use corpus::{Corpus, EmbeddedScript};
pub use load::{load, load_corpus};
pub use resolve_form::FormResolver;

use crate::ir::{AiData, FormKey};
use crate::obscript::{self, DeclaredVar, NameTable};

/// The comparison value of a condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CondValue {
    /// A literal.
    Literal(f64),
    /// A `GLOB`.
    Global(FormKey),
}

/// One condition (`CTDA`), as stored, with its parameters also read as
/// forms where they resolve to one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceCondition {
    /// The function index.
    pub function: u16,
    /// `==`, `!=`, `>`, `>=`, `<`, `<=`, or the stored code of an unknown
    /// comparison (`"6"`, `"7"`).
    pub comparison: String,
    /// The comparison value.
    pub value: CondValue,
    /// The first parameter, as stored.
    pub param1: u32,
    /// The second parameter, as stored.
    pub param2: u32,
    /// `param1` as a form of the plugin list, when it is one.
    pub param1_form: Option<FormKey>,
    /// `param2` as a form of the plugin list, when it is one.
    pub param2_form: Option<FormKey>,
    /// 0 subject, 1 target, 2 reference, 3 combat target, 4 linked
    /// reference.
    pub run_on: u32,
    /// The reference for run-on 2.
    pub reference: Option<FormKey>,
    /// OR-ed with the next condition.
    pub is_or: bool,
}

impl SourceCondition {
    /// A condition on the subject comparing `function` with a literal.
    pub fn literal(function: u16, comparison: &str, value: f64) -> Self {
        Self {
            function,
            comparison: comparison.to_owned(),
            value: CondValue::Literal(value),
            param1: 0,
            param2: 0,
            param1_form: None,
            param2_form: None,
            run_on: 0,
            reference: None,
            is_or: false,
        }
    }

    /// Sets the first parameter to a form.
    #[must_use]
    pub fn with_form(mut self, form: FormKey) -> Self {
        self.param1_form = Some(form);
        self
    }

    /// Sets the raw parameters.
    #[must_use]
    pub fn with_params(mut self, param1: u32, param2: u32) -> Self {
        self.param1 = param1;
        self.param2 = param2;
        self
    }

    /// Sets the run-on target.
    #[must_use]
    pub fn on(mut self, run_on: u32, reference: Option<FormKey>) -> Self {
        self.run_on = run_on;
        self.reference = reference;
        self
    }

    /// Sets the OR flag.
    #[must_use]
    pub fn or(mut self) -> Self {
        self.is_or = true;
        self
    }
}

/// A script embedded in another record.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SourceBody {
    /// The source text.
    pub source: String,
    /// Its variables (`SLSD`/`SCVR`), usually none.
    pub vars: Vec<DeclaredVar>,
    /// Its reference list (`SCRO`), resolved, in order.
    pub references: Vec<FormKey>,
}

/// A script record (`SCPT`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceScript {
    /// Editor id.
    pub editor_id: Option<String>,
    /// 0 object, 1 quest, 0x100 effect.
    pub script_type: u32,
    /// The script.
    pub body: SourceBody,
}

/// One response of an `INFO`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceResponse {
    /// The response number.
    pub number: u8,
    /// Emotion type.
    pub emotion_type: u32,
    /// Emotion value.
    pub emotion_value: i32,
    /// The text.
    pub text: String,
}

/// A response group (`INFO`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// The topic.
    pub topic: FormKey,
    /// The quest (`QSTI`).
    pub quest: Option<FormKey>,
    /// File order inside the topic group: (plugin load index, offset).
    pub order: (u32, u64),
    /// The dialogue type.
    pub info_type: u8,
    /// The two flag bytes, little-endian.
    pub flags: u16,
    /// The speaker (`ANAM`).
    pub speaker: Option<FormKey>,
    /// The prompt (`RNAM`).
    pub prompt: Option<String>,
    /// Responses, in order.
    pub responses: Vec<SourceResponse>,
    /// Conditions, in order.
    pub conditions: Vec<SourceCondition>,
    /// `TCLT`.
    pub choices: Vec<FormKey>,
    /// `TCLF`.
    pub link_from: Vec<FormKey>,
    /// `NAME`.
    pub add_topics: Vec<FormKey>,
    /// The script before `NEXT`.
    pub begin: Option<SourceBody>,
    /// The script after `NEXT`.
    pub end: Option<SourceBody>,
    /// `DNAM`, as stored.
    pub challenge: Option<u32>,
    /// `KNAM`: the check's label actor value (task T1; `None` today).
    pub label: Option<u32>,
}

/// A dialogue topic (`DIAL`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceTopic {
    /// Editor id.
    pub editor_id: Option<String>,
    /// The topic text.
    pub name: Option<String>,
    /// The type.
    pub topic_type: u8,
    /// The flags.
    pub flags: u8,
    /// `PNAM`.
    pub priority: f32,
}

/// A stage log entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceLogEntry {
    /// `QSDT`.
    pub flags: u8,
    /// Conditions.
    pub conditions: Vec<SourceCondition>,
    /// Log text.
    pub text: Option<String>,
    /// Result script.
    pub script: Option<SourceBody>,
    /// `NAM0`.
    pub next_quest: Option<FormKey>,
}

/// A quest stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceStage {
    /// Index.
    pub index: i16,
    /// Log entries, in order.
    pub entries: Vec<SourceLogEntry>,
}

/// A quest objective.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceObjective {
    /// Index.
    pub index: i32,
    /// Text.
    pub text: Option<String>,
    /// Targets and their conditions.
    pub targets: Vec<(FormKey, Vec<SourceCondition>)>,
}

/// A quest (`QUST`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceQuest {
    /// Editor id.
    pub editor_id: Option<String>,
    /// Name.
    pub name: Option<String>,
    /// The quest script.
    pub script: Option<FormKey>,
    /// `DATA` flags.
    pub flags: u8,
    /// `DATA` priority.
    pub priority: u8,
    /// `DATA` delay.
    pub delay: Option<f32>,
    /// Stages, in order.
    pub stages: Vec<SourceStage>,
    /// Objectives, in order.
    pub objectives: Vec<SourceObjective>,
}

/// A global (`GLOB`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceGlobal {
    /// Editor id.
    pub editor_id: Option<String>,
    /// `FNAM`: `s` short, `l` long, `f` float.
    pub kind: char,
    /// `FLTV`.
    pub value: f32,
}

/// A terminal menu item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceMenuItem {
    /// Text.
    pub text: Option<String>,
    /// Result text.
    pub result: Option<String>,
    /// Flags; bit 0 adds the display note.
    pub flags: u8,
    /// The note shown.
    pub display_note: Option<FormKey>,
    /// The sub-terminal.
    pub sub_terminal: Option<FormKey>,
    /// The item's script.
    pub script: Option<SourceBody>,
    /// Conditions.
    pub conditions: Vec<SourceCondition>,
}

/// A terminal (`TERM`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceTerminal {
    /// Editor id.
    pub editor_id: Option<String>,
    /// Header.
    pub header: Option<String>,
    /// Difficulty.
    pub difficulty: u8,
    /// Flags.
    pub flags: u8,
    /// Server type.
    pub server: u8,
    /// Password note.
    pub password_note: Option<FormKey>,
    /// Menu items.
    pub items: Vec<SourceMenuItem>,
}

/// A note (`NOTE`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceNote {
    /// Editor id.
    pub editor_id: Option<String>,
    /// Title.
    pub title: Option<String>,
    /// 0 sound, 1 text, 2 image, 3 voice.
    pub note_type: u8,
    /// Text.
    pub text: Option<String>,
    /// A voice note's topic.
    pub topic: Option<FormKey>,
    /// The sound.
    pub sound: Option<FormKey>,
    /// An image note's texture.
    pub texture: Option<String>,
}

/// A package location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    /// Type, as stored.
    pub location_type: i32,
    /// The form, for the types whose value is one.
    pub form: Option<FormKey>,
    /// The raw value.
    pub value: u32,
    /// Radius.
    pub radius: i32,
}

/// A package target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTarget {
    /// Type, as stored.
    pub target_type: i32,
    /// The form.
    pub form: Option<FormKey>,
    /// The raw value.
    pub value: u32,
    /// Count or distance.
    pub count: i32,
}

/// An AI package (`PACK`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourcePackage {
    /// Editor id.
    pub editor_id: Option<String>,
    /// Type.
    pub package_type: u8,
    /// General flags.
    pub flags: u32,
    /// Behaviour flags (task T1; 0 today).
    pub behaviour_flags: u16,
    /// Type-specific flags (task T1; 0 today).
    pub type_flags: u16,
    /// `(month, day of week, date, hour, duration)`, when stored.
    pub schedule: Option<(i8, i8, u8, i8, i32)>,
    /// Location.
    pub location: Option<SourceLocation>,
    /// Target.
    pub target: Option<SourceTarget>,
    /// Conditions.
    pub conditions: Vec<SourceCondition>,
    /// Begin, end and change scripts, by event.
    pub events: Vec<(crate::ir::PackageEvent, Option<SourceBody>)>,
}

/// An actor base (`NPC_`, `CREA`, `LVLN`, `LVLC`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceActor {
    /// Editor id.
    pub editor_id: Option<String>,
    /// Signature.
    pub sig: String,
    /// Object script.
    pub script: Option<FormKey>,
    /// Packages, in order.
    pub packages: Vec<FormKey>,
    /// Factions and ranks, in order.
    pub factions: Vec<(FormKey, i8)>,
    /// Voice type.
    pub voice: Option<FormKey>,
    /// Race.
    pub race: Option<FormKey>,
    /// Class.
    pub class: Option<FormKey>,
    /// Female.
    pub female: bool,
    /// `ACBS` flags.
    pub actor_flags: u32,
    /// AI data (task T1; `None` today).
    pub ai: Option<AiData>,
}

impl SourceActor {
    /// `ACBS` flag bit 1, "essential".
    pub fn essential(&self) -> bool {
        self.actor_flags & 0x02 != 0
    }
}

/// A base object of a placed reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBase {
    /// Signature.
    pub sig: String,
    /// Editor id.
    pub editor_id: Option<String>,
    /// Script.
    pub script: Option<FormKey>,
}

/// A placed reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// `REFR`, `ACHR`, `ACRE`.
    pub sig: String,
    /// Editor id.
    pub editor_id: Option<String>,
    /// Base.
    pub base: Option<FormKey>,
    /// Linked reference.
    pub linked_ref: Option<FormKey>,
    /// Enable parent and the opposite flag.
    pub enable_parent: Option<(FormKey, bool)>,
    /// Starts disabled.
    pub initially_disabled: bool,
    /// Persistent.
    pub persistent: bool,
}

/// One editor id of the plugin list.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceName {
    /// The form.
    pub form: FormKey,
    /// Its signature.
    pub sig: String,
    /// The editor id, as stored.
    pub editor_id: String,
}

/// Everything the build reads.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SimSource {
    /// `mir::GameVariant::id`.
    pub game: String,
    /// The plugins, in load order, lowercase.
    pub plugins: Vec<String>,
    /// Every editor id of the plugin list, by form.
    pub names: Vec<SourceName>,
    /// The base of every placed reference with an editor id, and of every
    /// reference the slice places.
    pub ref_bases: BTreeMap<FormKey, FormKey>,
    /// The script of every record that has one.
    pub base_scripts: BTreeMap<FormKey, FormKey>,
    /// The only placed reference of each actor base placed exactly once.
    pub unique_refs: BTreeMap<FormKey, FormKey>,
    /// The slice's references, their enable parents and linked references.
    pub refs: BTreeMap<FormKey, SourceRef>,
    /// The bases of the slice's references.
    pub bases: BTreeMap<FormKey, SourceBase>,
    /// The actor bases among them.
    pub actors: BTreeMap<FormKey, SourceActor>,
    /// Every script record.
    pub scripts: BTreeMap<FormKey, SourceScript>,
    /// The topics the slice's INFOs belong to or name.
    pub topics: BTreeMap<FormKey, SourceTopic>,
    /// The INFOs the slice's actors can say.
    pub infos: BTreeMap<FormKey, SourceInfo>,
    /// Every quest.
    pub quests: BTreeMap<FormKey, SourceQuest>,
    /// Every global.
    pub globals: BTreeMap<FormKey, SourceGlobal>,
    /// The slice's terminals and their sub-terminals.
    pub terminals: BTreeMap<FormKey, SourceTerminal>,
    /// Every note.
    pub notes: BTreeMap<FormKey, SourceNote>,
    /// The packages of the slice's actors.
    pub packages: BTreeMap<FormKey, SourcePackage>,
}

impl SimSource {
    /// The name table: every editor id, every script's variables, every
    /// quest's script, every reference's base and every base's script.
    pub fn name_table(&self) -> NameTable {
        let mut names = NameTable::new();
        names.set_plugins(self.plugins.clone());
        for name in &self.names {
            names.add_form(&name.editor_id, name.form.clone(), &name.sig);
        }
        for (form, script) in &self.scripts {
            names.set_script_vars(
                form.clone(),
                obscript::declarations(&script.body.source, &script.body.vars),
            );
        }
        for (form, quest) in &self.quests {
            if let Some(script) = &quest.script {
                names.set_quest_script(form.clone(), script.clone());
            }
        }
        for (reference, base) in &self.ref_bases {
            names.set_ref_base(reference.clone(), base.clone());
        }
        for (base, script) in &self.base_scripts {
            names.set_base_script(base.clone(), script.clone());
        }
        for (base, reference) in &self.unique_refs {
            names.set_unique_ref(base.clone(), reference.clone());
        }
        names
    }
}
