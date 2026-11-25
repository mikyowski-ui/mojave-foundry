//! World state, terminals, notes, references, actors and globals
//! (contracts 11.1, spec 3.8; `Actor`, `Global` and `RefUses` are settled
//! here, task T2).

use serde::{Deserialize, Serialize};

use super::expr::{Cond, VarTy};
use super::id::{FormKey, SimId, TextId};

/// An enable-parent group (spec 8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnableGroup {
    /// [`super::SimKind::Group`] of the parent reference.
    pub id: SimId,
    /// The parent starts enabled.
    pub initially_enabled: bool,
    /// The children in the slice, sorted, with their opposite flag.
    pub children: Vec<(FormKey, bool)>,
}

/// A terminal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Terminal {
    /// [`super::SimKind::Terminal`].
    pub id: SimId,
    /// The header text.
    pub header: Option<TextId>,
    /// 0 very easy .. 4 very hard, 5 key only.
    pub difficulty: u8,
    /// Flags, as stored.
    pub flags: u8,
    /// Server type, as stored.
    pub server: u8,
    /// The note holding the password.
    pub password: Option<SimId>,
    /// The menu, in order.
    pub items: Vec<MenuItem>,
}

/// A terminal menu item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuItem {
    /// [`super::SimKind::MenuItem`].
    pub id: SimId,
    /// The menu text.
    pub text: TextId,
    /// The result text.
    pub result: Option<TextId>,
    /// Flag "add note".
    pub add_note: bool,
    /// The note shown.
    pub show_note: Option<SimId>,
    /// The sub-terminal opened.
    pub submenu: Option<SimId>,
    /// The item's script.
    pub script: Option<SimId>,
    /// When the item is shown.
    pub condition: Cond,
}

/// A note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// [`super::SimKind::Note`].
    pub id: SimId,
    /// The title.
    pub title: Option<TextId>,
    /// The content.
    pub body: NoteBody,
}

/// A note's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteBody {
    /// A text note.
    Text(TextId),
    /// A holotape: its topic's responses are its text.
    Voice {
        /// The topic.
        topic: Option<SimId>,
    },
    /// An image, by texture path.
    Image(String),
    /// A sound, by form key text.
    Sound(String),
}

/// Why a placed reference is needed at run time: a bit set.
///
/// Serialised as the sorted list of the names of its bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct RefUses(pub u8);

impl RefUses {
    /// Its base has a script, or a script names it.
    pub const SCRIPT: RefUses = RefUses(0x01);
    /// A condition runs on it or names it.
    pub const CONDITION: RefUses = RefUses(0x02);
    /// A package's location or target.
    pub const PACKAGE: RefUses = RefUses(0x04);
    /// A quest objective's target.
    pub const QUEST_TARGET: RefUses = RefUses(0x08);
    /// An enable parent, or a child of one.
    pub const ENABLE_PARENT: RefUses = RefUses(0x10);
    /// Another needed reference's linked reference.
    pub const LINKED: RefUses = RefUses(0x20);

    const NAMES: [(RefUses, &'static str); 6] = [
        (Self::SCRIPT, "script"),
        (Self::CONDITION, "condition"),
        (Self::PACKAGE, "package"),
        (Self::QUEST_TARGET, "quest_target"),
        (Self::ENABLE_PARENT, "enable_parent"),
        (Self::LINKED, "linked"),
    ];

    /// No use.
    pub const NONE: RefUses = RefUses(0);

    /// Whether every bit of `other` is set.
    pub fn contains(self, other: RefUses) -> bool {
        self.0 & other.0 == other.0
    }

    /// Sets the bits of `other`.
    pub fn insert(&mut self, other: RefUses) {
        self.0 |= other.0;
    }

    /// Whether no bit is set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The names of the bits set, in bit order.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .iter()
            .filter(|(bit, _)| self.contains(*bit))
            .map(|&(_, name)| name)
            .collect()
    }
}

impl Serialize for RefUses {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.names().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RefUses {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(deserializer)?;
        let mut uses = RefUses::NONE;
        for name in names {
            let bit = Self::NAMES
                .iter()
                .find(|(_, n)| *n == name)
                .map(|&(bit, _)| bit)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown use {name:?}")))?;
            uses.insert(bit);
        }
        Ok(uses)
    }
}

/// Every placed reference something needs at run time (spec 5.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefEntry {
    /// The reference.
    pub form: FormKey,
    /// Its editor id.
    pub editor_id: Option<String>,
    /// Its base.
    pub base: FormKey,
    /// The map it is placed on (the placement's map stem).
    pub area: String,
    /// A critter: it stores its own slot (spec 5.8).
    pub mobile: bool,
    /// Why it is needed.
    pub why: RefUses,
}

/// What an actor base is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// `NPC_`
    Npc,
    /// `CREA`
    Creature,
    /// `LVLN` or `LVLC`: a leveled list of actors.
    Leveled,
}

/// The AI data of an actor (`AIDT`), after templates. Filled once the
/// extraction decodes it (task T1); absent before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AiData {
    /// 0 unaggressive, 1 aggressive, 2 very aggressive, 3 frenzied.
    pub aggression: u8,
    /// 0 cowardly .. 4 foolhardy.
    pub confidence: u8,
    /// Energy level.
    pub energy: u8,
    /// Responsibility.
    pub responsibility: u8,
    /// Mood.
    pub mood: u8,
    /// Services flags, as stored.
    pub services: u32,
    /// Assistance: 0 helps nobody, 1 allies, 2 friends and allies.
    pub assistance: u8,
}

/// An actor base placed in the slice (`Module::actors`, keyed by base).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    /// The base form.
    pub base: FormKey,
    /// Editor id, as stored.
    pub editor_id: Option<String>,
    /// What it is.
    pub kind: ActorKind,
    /// Its object script ([`super::SimKind::Script`]).
    pub script: Option<SimId>,
    /// Its packages, in `PKID` order ([`super::SimKind::Package`]).
    pub packages: Vec<SimId>,
    /// Its factions and ranks, in file order.
    pub factions: Vec<(FormKey, i8)>,
    /// Its voice type (`VTCK`).
    pub voice: Option<FormKey>,
    /// Its race.
    pub race: Option<FormKey>,
    /// Its class.
    pub class: Option<FormKey>,
    /// Female.
    pub female: bool,
    /// The essential flag, which Pass 2 ignores and reports (owner
    /// decision 12).
    pub essential: bool,
    /// The AI data.
    pub ai: Option<AiData>,
    /// Its placed references in the slice, sorted.
    pub placements: Vec<FormKey>,
}

/// What a campaign global stores; the allocation order of spec 10.2 is
/// this enum's order, then [`SimId`] order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalPurpose {
    /// Quest state (spec 6.1): stage, flags, done and pending bits,
    /// objectives.
    QuestState,
    /// A quest variable (spec 5.4).
    QuestVar,
    /// A source `GLOB`.
    Glob,
    /// An enable-parent group's state (spec 8.2).
    Group,
    /// A note's holodisk global (spec 9.2).
    Note,
    /// Say-once and say-once-a-day flags, topics given, challenge outcomes
    /// (spec 4.5).
    OnceFlag,
    /// Per-actor globals: dead, faction overrides, kill counters, hoisted
    /// variables.
    Actor,
}

/// One campaign global the layer appends to `vault13.gam`
/// (`MF_<KIND>_<NAME> := <initial>;`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Global {
    /// The global's id (its key in `Module::globals`): the source thing
    /// it stores.
    pub id: SimId,
    /// What it stores.
    pub purpose: GlobalPurpose,
    /// The `<NAME>` part: the source editor id, uppercase, characters
    /// outside `A-Z0-9_` replaced by `_`.
    pub name: String,
    /// The value's type: a float is stored times 1,000, rounded (spec 5.4).
    pub ty: VarTy,
    /// The initial value as stored (already scaled for a float).
    pub initial: i32,
}

/// Makes a global's `<NAME>` from an editor id.
pub fn global_name(editor_id: &str) -> String {
    editor_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// A float as a stored global value: times 1,000, rounded, saturated.
pub fn fixed_point(value: f64) -> i32 {
    let scaled = (value * 1000.0).round();
    if scaled.is_nan() {
        0
    } else {
        scaled.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_uses_serialise_as_names() {
        let mut uses = RefUses::CONDITION;
        uses.insert(RefUses::SCRIPT);
        assert_eq!(
            serde_json::to_string(&uses).unwrap(),
            r#"["script","condition"]"#
        );
        let back: RefUses = serde_json::from_str(r#"["condition","script"]"#).unwrap();
        assert_eq!(back, uses);
        assert!(serde_json::from_str::<RefUses>(r#"["nope"]"#).is_err());
    }

    #[test]
    fn global_names_and_fixed_point() {
        assert_eq!(global_name("MS11 Var-1"), "MS11_VAR_1");
        assert_eq!(fixed_point(1.2345), 1235);
        assert_eq!(fixed_point(-0.5), -500);
        assert_eq!(fixed_point(1e12), i32::MAX);
    }
}
