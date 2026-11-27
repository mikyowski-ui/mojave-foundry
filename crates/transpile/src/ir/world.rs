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
    /// A locked door or container (spec 9.3; task T5).
    pub const LOCK: RefUses = RefUses(0x40);
    /// A placed terminal (spec 9.1; task T5).
    pub const TERMINAL: RefUses = RefUses(0x80);

    const NAMES: [(RefUses, &'static str); 8] = [
        (Self::SCRIPT, "script"),
        (Self::CONDITION, "condition"),
        (Self::PACKAGE, "package"),
        (Self::QUEST_TARGET, "quest_target"),
        (Self::ENABLE_PARENT, "enable_parent"),
        (Self::LINKED, "linked"),
        (Self::LOCK, "lock"),
        (Self::TERMINAL, "terminal"),
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

/// One child of an enable group on one map (spec 8.3): the map script's
/// table row. The lowering joins it with the placement (hex, elevation,
/// PID).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StateChild {
    /// The child reference (the parent itself for a group of one).
    pub reference: FormKey,
    /// Its group.
    pub group: SimId,
    /// Enabled when the group is off.
    pub opposite: bool,
    /// A critter: its own program applies the state (it moves).
    pub mobile: bool,
    /// The map stage wrote it with the hidden flag.
    pub hidden: bool,
}

/// A terminal program (spec 9.1): one per terminal base placed in the
/// slice, with the sub-terminals it reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProgram {
    /// The terminal base ([`super::SimKind::Terminal`]).
    pub terminal: SimId,
    /// The placed references of it, sorted.
    pub references: Vec<FormKey>,
    /// The menus of the no-head dialogue: the terminal first, then every
    /// sub-terminal reachable from it, depth first in item order, once.
    pub menus: Vec<SimId>,
    /// Sub-terminals an item names that are not in the MIR, by item.
    pub missing: Vec<SimId>,
    /// The lock: `None` for a terminal flagged unlocked.
    pub hack: Option<Hack>,
}

/// How a locked terminal opens (spec 9.1, owner decision 3: automatic at
/// the required Science, no minigame, no lockout).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hack {
    /// The Science skill needed (0, 25, 50, 75, 100 for difficulty 0 to
    /// 4); `None` for difficulty 5, password only.
    pub science: Option<u8>,
    /// The XP given on success (`iXPRewardHackComputer<level>`).
    pub xp: i32,
    /// The note whose possession offers "[Password]".
    pub password: Option<SimId>,
}

/// A lock on a door or container (spec 9.3, owner decision 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    /// The locked reference.
    pub reference: FormKey,
    /// Door or container.
    pub kind: super::PlacedKind,
    /// The source lock level, as stored (255 key only).
    pub level: u8,
    /// The Lockpick skill needed: the level rounded up to 0, 25, 50, 75 or
    /// 100; `None` for a key-only lock.
    pub lockpick: Option<u8>,
    /// The key's base form, when the lock names one.
    pub key: Option<FormKey>,
    /// The converted key item's PID, from the placement index; `None`
    /// when the key did not convert (the lock is then left to the skill,
    /// reported) or the index does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_pid: Option<u32>,
    /// The XP given on picking it (`iXPRewardPickLock<tier>`).
    pub xp: i32,
    /// The map stage set the locked bit on the written object.
    pub locked_bit: bool,
}

/// A Pip-Boy holodisk made from a note (spec 9.2, owner decision 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holodisk {
    /// The note.
    pub note: SimId,
    /// The global that holds it (purpose [`GlobalPurpose::Note`]; the
    /// note's own id).
    pub global: SimId,
    /// Its title.
    pub title: Option<TextId>,
    /// The body, one text per paragraph, in order; the lowering wraps
    /// lines and writes `**END-PAR**` between them. Empty for an image or
    /// sound note (title only, reported).
    pub paragraphs: Vec<TextId>,
    /// Where the body came from.
    pub source: HolodiskSource,
}

/// Where a holodisk's body came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HolodiskSource {
    /// A text note.
    Text,
    /// A voice note: its topic's responses, as text.
    Voice,
    /// An image note: title only.
    Image,
    /// A sound note: title only.
    Sound,
}

/// A message (`MESG`) a script shows (spec 5.9 `ShowMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageBox {
    /// [`super::SimKind::Message`].
    pub id: SimId,
    /// Editor id.
    pub editor_id: String,
    /// The title.
    pub title: Option<TextId>,
    /// The text; `%` format codes are kept as written and reported.
    pub text: Option<TextId>,
    /// A message box: waits for a button (a no-head dialogue whose options
    /// set the pressed button); otherwise a `display_msg` notice.
    pub message_box: bool,
    /// The buttons, in order, with the conditions that show them.
    pub buttons: Vec<(TextId, Cond)>,
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
