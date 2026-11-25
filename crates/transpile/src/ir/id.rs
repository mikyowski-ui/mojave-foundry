//! Identifiers and text (contracts 11.1, spec 3.1 and 3.2).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Every IR object is named by the source form it came from:
/// `"fallout3.esm:01a2b3"`, the MIR's form identity (contracts section 2).
///
/// Ordered as text. Two keys of one plugin order by local id, because the
/// local id is always six lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FormKey(pub String);

impl FormKey {
    /// The key of a form of a plugin (`plugin` is lowercased).
    pub fn new(plugin: &str, local_id: u32) -> Self {
        Self(format!(
            "{}:{:06x}",
            plugin.to_ascii_lowercase(),
            local_id & 0x00FF_FFFF
        ))
    }

    /// The text form.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The converter's own pseudo-form, owner of its labels.
    pub fn converter() -> Self {
        Self("mojave-foundry:000000".to_owned())
    }
}

impl From<&mir::FormKey> for FormKey {
    fn from(key: &mir::FormKey) -> Self {
        Self(key.to_string())
    }
}

impl From<mir::FormKey> for FormKey {
    fn from(key: mir::FormKey) -> Self {
        Self(key.to_string())
    }
}

impl fmt::Display for FormKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What kind of IR object a [`SimId`] names. The order of the variants is
/// the order of [`SimId`]s, and so of every numbering derived from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimKind {
    /// A dialogue topic (`DIAL`); `sub` 0.
    Topic,
    /// A response group (`INFO`); `sub` 0.
    Info,
    /// A quest (`QUST`); `sub` 0 for the quest itself. Quest-state globals
    /// use the sub codes of [`SimId::quest_state`].
    Quest,
    /// A quest stage: `form` the quest, `sub` the stage index.
    Stage,
    /// A quest objective: `form` the quest, `sub` the objective index.
    Objective,
    /// A quest variable: `form` the quest, `sub` the variable index.
    QuestVar,
    /// A `GLOB`; `sub` 0.
    Global,
    /// A script: a `SCPT` (`sub` 0) or an embedded result script, named
    /// by its owner form and [`SimId::result_script`]'s sub codes.
    Script,
    /// An object-script variable: `form` the script, `sub` the index.
    ScriptVar,
    /// An AI package (`PACK`); `sub` 0.
    Package,
    /// An enable-parent group: `form` the parent reference; `sub` 0.
    Group,
    /// A terminal (`TERM`); `sub` 0.
    Terminal,
    /// A terminal menu item: `form` the terminal, `sub` the item's
    /// position from 0.
    MenuItem,
    /// A note (`NOTE`); `sub` 0.
    Note,
    /// An actor base (`NPC_`, `CREA`, or a leveled list of them); `sub` 0.
    Actor,
    /// A placed reference; `sub` 0.
    Ref,
    /// A converter text: `form` [`FormKey::converter`], `sub` the label
    /// number.
    Text,
}

impl SimKind {
    /// The lowercase name used in the text form of a [`SimId`].
    pub fn name(self) -> &'static str {
        match self {
            Self::Topic => "topic",
            Self::Info => "info",
            Self::Quest => "quest",
            Self::Stage => "stage",
            Self::Objective => "objective",
            Self::QuestVar => "quest_var",
            Self::Global => "global",
            Self::Script => "script",
            Self::ScriptVar => "script_var",
            Self::Package => "package",
            Self::Group => "group",
            Self::Terminal => "terminal",
            Self::MenuItem => "menu_item",
            Self::Note => "note",
            Self::Actor => "actor",
            Self::Ref => "ref",
            Self::Text => "text",
        }
    }

    /// Every kind, in order.
    pub const ALL: [SimKind; 17] = [
        Self::Topic,
        Self::Info,
        Self::Quest,
        Self::Stage,
        Self::Objective,
        Self::QuestVar,
        Self::Global,
        Self::Script,
        Self::ScriptVar,
        Self::Package,
        Self::Group,
        Self::Terminal,
        Self::MenuItem,
        Self::Note,
        Self::Actor,
        Self::Ref,
        Self::Text,
    ];
}

/// An IR identifier: kind plus form plus a sub-index where one form makes
/// several objects (a quest's stage 10, a terminal's menu item 2).
///
/// Serialised as the text `"<kind>/<form>/<sub>"`, for example
/// `"stage/fallout3.esm:01a2b3/10"`, so that it can key a JSON object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimId {
    /// What it names.
    pub kind: SimKind,
    /// The source form.
    pub form: FormKey,
    /// The sub-index (see [`SimKind`]); 0 when the form makes one object.
    pub sub: u32,
}

/// Sub codes of the quest-state globals (spec 6.1) under
/// [`SimKind::Quest`].
pub mod quest_sub {
    /// The quest itself.
    pub const QUEST: u32 = 0;
    /// `G_q_STAGE`, the highest stage set.
    pub const STAGE: u32 = 1;
    /// `G_q_FLAGS`: running, completed, failed, start-enabled-applied.
    pub const FLAGS: u32 = 2;
    /// `G_q_DONE_i`: `DONE + i`, one global per 31 stages.
    pub const DONE: u32 = 0x100;
    /// `G_q_PEND_i`: `PEND + i`.
    pub const PEND: u32 = 0x200;
}

/// Sub codes of embedded result scripts under [`SimKind::Script`], keyed
/// by their owner form.
pub mod script_sub {
    /// A `SCPT` record, keyed by its own form.
    pub const RECORD: u32 = 0;
    /// An `INFO`'s script before `NEXT`.
    pub const INFO_BEGIN: u32 = 1;
    /// An `INFO`'s script after `NEXT`.
    pub const INFO_END: u32 = 2;
    /// A package's begin, end and change scripts.
    pub const PACKAGE_BEGIN: u32 = 3;
    /// See [`PACKAGE_BEGIN`].
    pub const PACKAGE_END: u32 = 4;
    /// See [`PACKAGE_BEGIN`].
    pub const PACKAGE_CHANGE: u32 = 5;
    /// A terminal menu item's script: `MENU_ITEM + position`.
    pub const MENU_ITEM: u32 = 0x1000;
    /// A quest stage log entry's script: `STAGE + stage * 256 + entry`
    /// (stage 0 to 65,535, entry 0 to 255).
    pub const STAGE: u32 = 0x0100_0000;
}

impl SimId {
    /// A new id.
    pub fn new(kind: SimKind, form: FormKey, sub: u32) -> Self {
        Self { kind, form, sub }
    }

    /// The id of a form that makes one object.
    pub fn of(kind: SimKind, form: &FormKey) -> Self {
        Self::new(kind, form.clone(), 0)
    }

    /// A quest stage.
    pub fn stage(quest: &FormKey, index: i16) -> Self {
        Self::new(SimKind::Stage, quest.clone(), u32::from(index as u16))
    }

    /// A quest objective.
    pub fn objective(quest: &FormKey, index: i32) -> Self {
        Self::new(SimKind::Objective, quest.clone(), index as u32)
    }

    /// A quest-state global ([`quest_sub`]).
    pub fn quest_state(quest: &FormKey, sub: u32) -> Self {
        Self::new(SimKind::Quest, quest.clone(), sub)
    }

    /// The script embedded in `owner` at `sub` ([`script_sub`]).
    pub fn result_script(owner: &FormKey, sub: u32) -> Self {
        Self::new(SimKind::Script, owner.clone(), sub)
    }

    /// A stage log entry's script.
    pub fn stage_script(quest: &FormKey, stage: i16, entry: u32) -> Self {
        let stage = u32::from(stage as u16);
        Self::result_script(quest, script_sub::STAGE + stage * 256 + entry.min(255))
    }

    /// A converter label ([`crate::labels`]).
    pub fn label(number: u32) -> Self {
        Self::new(SimKind::Text, FormKey::converter(), number)
    }
}

impl fmt::Display for SimId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.kind.name(), self.form, self.sub)
    }
}

/// A text that is not a [`SimId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadSimId(pub String);

impl fmt::Display for BadSimId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a sim id: {:?}", self.0)
    }
}

impl std::error::Error for BadSimId {}

impl FromStr for SimId {
    type Err = BadSimId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || BadSimId(text.to_owned());
        let (kind, rest) = text.split_once('/').ok_or_else(bad)?;
        let (form, sub) = rest.rsplit_once('/').ok_or_else(bad)?;
        let kind = SimKind::ALL
            .into_iter()
            .find(|k| k.name() == kind)
            .ok_or_else(bad)?;
        let sub = sub.parse().map_err(|_| bad())?;
        Ok(Self::new(kind, FormKey(form.to_owned()), sub))
    }
}

impl Serialize for SimId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SimId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// An index into [`crate::ir::Module::texts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TextId(pub u32);

/// One text of a message table, as decoded (Windows-1252); `emit`
/// converts it once (spec 4.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    /// What the text belongs to.
    pub owner: SimId,
    /// The text.
    pub source: String,
    /// What it is.
    pub role: TextRole,
}

/// What a [`Text`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextRole {
    /// A response line of an `INFO`.
    Response,
    /// An `INFO`'s player prompt (`RNAM`).
    Prompt,
    /// A topic's text.
    TopicName,
    /// A quest's name.
    QuestName,
    /// An objective's text.
    Objective,
    /// A stage log entry's text.
    LogEntry,
    /// A terminal's header.
    TerminalHeader,
    /// A menu item's text.
    MenuItem,
    /// A menu item's result text.
    MenuResult,
    /// A note's title.
    NoteTitle,
    /// A note's text.
    NoteBody,
    /// A message box or a script's string literal.
    MessageBox,
    /// A converter label ([`crate::labels`]).
    Label,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_id_round_trips_as_text() {
        let id = SimId::stage(&FormKey::new("Fallout3.esm", 0x01a2b3), 10);
        assert_eq!(id.to_string(), "stage/fallout3.esm:01a2b3/10");
        assert_eq!(id.to_string().parse::<SimId>(), Ok(id.clone()));
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"stage/fallout3.esm:01a2b3/10\"");
        assert_eq!(serde_json::from_str::<SimId>(&json).unwrap(), id);
        assert!("nonsense".parse::<SimId>().is_err());
        assert!("stage/x/y".parse::<SimId>().is_err());
    }

    #[test]
    fn sim_ids_order_by_kind_then_form_then_sub() {
        let a = FormKey::new("a.esm", 2);
        let b = FormKey::new("a.esm", 1);
        let mut ids = [
            SimId::of(SimKind::Quest, &a),
            SimId::stage(&a, 5),
            SimId::of(SimKind::Info, &a),
            SimId::stage(&b, 9),
            SimId::stage(&a, 1),
        ];
        ids.sort();
        let text: Vec<String> = ids.iter().map(ToString::to_string).collect();
        assert_eq!(
            text,
            [
                "info/a.esm:000002/0",
                "quest/a.esm:000002/0",
                "stage/a.esm:000001/9",
                "stage/a.esm:000002/1",
                "stage/a.esm:000002/5",
            ]
        );
    }
}
