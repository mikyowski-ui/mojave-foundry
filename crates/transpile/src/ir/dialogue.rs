//! Dialogue (contracts 11.1, spec 3.4).

use serde::{Deserialize, Serialize};

use super::expr::{CmpOp, Cond};
use super::id::{FormKey, SimId, TextId};
use super::script::Block;

/// A dialogue topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    /// [`super::SimKind::Topic`].
    pub id: SimId,
    /// Editor id, as stored (empty when the record has none).
    pub editor_id: String,
    /// The topic type.
    pub kind: TopicKind,
    /// The topic's text.
    pub name: Option<TextId>,
    /// Priority (`PNAM`; 0 when absent).
    pub priority: f32,
    /// `DIAL` flag `0x02`: listed in the hub.
    pub top_level: bool,
    /// The slice's INFOs of the topic, in file order inside the topic group.
    pub infos: Vec<SimId>,
}

/// A topic type, as stored: 0 to 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicKind {
    /// 0
    Topic,
    /// 1
    Conversation,
    /// 2
    Combat,
    /// 3
    Persuasion,
    /// 4
    Detection,
    /// 5
    Service,
    /// 6
    Misc,
    /// 7
    Radio,
}

impl TopicKind {
    /// The kind of a stored type number; `None` for an unknown one.
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Topic,
            1 => Self::Conversation,
            2 => Self::Combat,
            3 => Self::Persuasion,
            4 => Self::Detection,
            5 => Self::Service,
            6 => Self::Misc,
            7 => Self::Radio,
            _ => return None,
        })
    }
}

/// A response group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    /// [`super::SimKind::Info`].
    pub id: SimId,
    /// Its topic.
    pub topic: SimId,
    /// Its quest.
    pub quest: SimId,
    /// Who can say it (spec 4.2).
    pub speakers: SpeakerSet,
    /// The conditions. The front end keeps every term; the dialogue
    /// compiler folds the speaker terms out per speaker.
    pub condition: Cond,
    /// The flags.
    pub flags: InfoFlags,
    /// The responses, in order.
    pub responses: Vec<Response>,
    /// `RNAM`; else the topic name.
    pub prompt: Option<TextId>,
    /// `TCLT`, in order.
    pub choices: Vec<SimId>,
    /// `TCLF`.
    pub link_from: Vec<SimId>,
    /// `NAME`.
    pub add_topics: Vec<SimId>,
    /// The result script before `NEXT`.
    pub begin: Option<Block>,
    /// The result script after `NEXT`.
    pub end: Option<Block>,
    /// A speech challenge or a stat gate; filled by the dialogue compiler.
    pub check: Option<SkillCheck>,
}

/// The flags of an `INFO` the compiler uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InfoFlags {
    /// Ends the conversation.
    pub goodbye: bool,
    /// Member of a random group.
    pub random: bool,
    /// Last member of a random group.
    pub random_end: bool,
    /// Never said twice.
    pub say_once: bool,
    /// Not said twice on one game day.
    pub say_once_a_day: bool,
    /// The end script runs right after the begin script.
    pub run_immediately: bool,
    /// Said, and the player stays in the same choice list.
    pub info_refusal: bool,
}

/// One response line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// The response number, from 1.
    pub number: u8,
    /// The text.
    pub text: TextId,
    /// Emotion type and value, as stored.
    pub emotion: (u32, i32),
}

/// Who can say an INFO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerSet {
    /// These actor bases (from `ANAM` and `GetIsID` on the subject), sorted.
    Bases(Vec<FormKey>),
    /// No speaker term: anyone whose other conditions pass.
    Any,
}

/// A speech challenge or a stat gate (spec 4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillCheck {
    /// Fallout 3: a chance shown on the option, one roll, two branches.
    Percent {
        /// The skill rolled (Speech).
        skill: Skill,
        /// `DNAM`, 1 to 5.
        difficulty: u8,
        /// `KNAM`.
        label: Option<ActorValue>,
        /// Where a failure goes.
        failure: FailureRoute,
        /// Owner decision 1: [`Remember::Forever`].
        remember: Remember,
    },
    /// New Vegas, and Fallout 3 attribute/skill gates: a comparison, no
    /// roll.
    Threshold {
        /// What is compared.
        stat: Stat,
        /// How.
        op: CmpOp,
        /// Against what, compared 1:1 (owner decision 2).
        value: i32,
        /// The label's actor value.
        label: Option<ActorValue>,
        /// New Vegas: shown with a "25/50" label when not met, and
        /// selectable (owner decision 15).
        shown_when_failed: bool,
        /// The INFO with the opposite comparison.
        partner: Option<SimId>,
    },
}

/// Where a failed challenge goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureRoute {
    /// The first passing INFO of a shared topic (`SpeechChallengeFailure`).
    SharedTopic(SimId),
    /// The next INFO of the same topic.
    NextInfo,
}

/// How long a challenge's outcome is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Remember {
    /// Rolled on every selection.
    PerSelection,
    /// Rolled once (owner decision 1).
    Forever,
}

/// A player attribute or skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stat {
    /// A Fallout 2 skill.
    Skill(Skill),
    /// A SPECIAL attribute, 0 Strength to 6 Luck.
    Special(u8),
}

/// A Fallout 2 skill, in Fallout 2 order (SPEC-006 section 3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Skill {
    /// 0
    SmallGuns,
    /// 1
    BigGuns,
    /// 2
    EnergyWeapons,
    /// 3
    Unarmed,
    /// 4
    Melee,
    /// 5
    Throwing,
    /// 6
    FirstAid,
    /// 7
    Doctor,
    /// 8
    Sneak,
    /// 9
    Lockpick,
    /// 10
    Steal,
    /// 11
    Traps,
    /// 12
    Science,
    /// 13
    Repair,
    /// 14
    Speech,
    /// 15
    Barter,
    /// 16
    Gambling,
    /// 17
    Outdoorsman,
}

/// A source actor value, by its stored number (spec 4.7: 5 to 11 the
/// SPECIAL attributes, 16 Health, 23 Karma, 32 to 45 the skills).
///
/// Serialised as the number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActorValue(pub u32);

/// The actor value names of the public function documentation, by number,
/// as both games' scripts spell them (New Vegas renames 44 and adds 73 to
/// 76).
const ACTOR_VALUES: &[(u32, &str)] = &[
    (0, "Aggression"),
    (1, "Confidence"),
    (2, "Energy"),
    (3, "Responsibility"),
    (4, "Mood"),
    (5, "Strength"),
    (6, "Perception"),
    (7, "Endurance"),
    (8, "Charisma"),
    (9, "Intelligence"),
    (10, "Agility"),
    (11, "Luck"),
    (12, "ActionPoints"),
    (13, "CarryWeight"),
    (14, "CritChance"),
    (15, "HealRate"),
    (16, "Health"),
    (17, "MeleeDamage"),
    (18, "DamageResist"),
    (19, "PoisonResist"),
    (20, "RadResist"),
    (21, "SpeedMult"),
    (22, "Fatigue"),
    (23, "Karma"),
    (24, "XP"),
    (25, "PerceptionCondition"),
    (26, "EnduranceCondition"),
    (27, "LeftAttackCondition"),
    (28, "RightAttackCondition"),
    (29, "LeftMobilityCondition"),
    (30, "RightMobilityCondition"),
    (31, "BrainCondition"),
    (32, "Barter"),
    (33, "BigGuns"),
    (34, "EnergyWeapons"),
    (35, "Explosives"),
    (36, "Lockpick"),
    (37, "Medicine"),
    (38, "MeleeWeapons"),
    (39, "Repair"),
    (40, "Science"),
    (41, "SmallGuns"),
    (41, "Guns"),
    (42, "Sneak"),
    (43, "Speech"),
    (44, "Throwing"),
    (44, "Survival"),
    (45, "Unarmed"),
    (46, "InventoryWeight"),
    (47, "Paralysis"),
    (48, "Invisibility"),
    (49, "Chameleon"),
    (50, "NightEye"),
    (51, "DetectLifeRange"),
    (52, "FireResist"),
    (53, "WaterBreathing"),
    (54, "RadiationRads"),
    (55, "BloodyMess"),
    (56, "UnarmedDamage"),
    (57, "Assistance"),
    (58, "ElectricResist"),
    (59, "FrostResist"),
    (60, "EnergyResist"),
    (61, "EmpResist"),
    (62, "Variable01"),
    (63, "Variable02"),
    (64, "Variable03"),
    (65, "Variable04"),
    (66, "Variable05"),
    (67, "Variable06"),
    (68, "Variable07"),
    (69, "Variable08"),
    (70, "Variable09"),
    (71, "Variable10"),
    (72, "IgnoreCrippledLimbs"),
    (73, "Dehydration"),
    (74, "Hunger"),
    (75, "SleepDeprivation"),
    (76, "DamageThreshold"),
];

impl ActorValue {
    /// The actor value a script names (case does not matter).
    pub fn named(name: &str) -> Option<Self> {
        ACTOR_VALUES
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(name))
            .map(|&(number, _)| Self(number))
    }

    /// The first name of the number, when the list has it.
    pub fn name(self) -> Option<&'static str> {
        ACTOR_VALUES
            .iter()
            .find(|(n, _)| *n == self.0)
            .map(|&(_, name)| name)
    }

    /// The Fallout 2 skill or SPECIAL attribute it maps to (spec 4.7), or
    /// `None` when it has no such read.
    pub fn stat(self) -> Option<Stat> {
        use Skill::*;
        Some(match self.0 {
            5..=11 => Stat::Special((self.0 - 5) as u8),
            32 => Stat::Skill(Barter),
            33 => Stat::Skill(BigGuns),
            34 => Stat::Skill(EnergyWeapons),
            35 => Stat::Skill(Traps),
            36 => Stat::Skill(Lockpick),
            37 => Stat::Skill(Doctor),
            38 => Stat::Skill(Melee),
            39 => Stat::Skill(Repair),
            40 => Stat::Skill(Science),
            41 => Stat::Skill(SmallGuns),
            42 => Stat::Skill(Sneak),
            43 => Stat::Skill(Speech),
            45 => Stat::Skill(Unarmed),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_values_map_to_the_spec_table() {
        assert_eq!(ActorValue::named("speech"), Some(ActorValue(43)));
        assert_eq!(ActorValue::named("SURVIVAL"), Some(ActorValue(44)));
        assert_eq!(ActorValue(44).name(), Some("Throwing"));
        assert_eq!(ActorValue(9).stat(), Some(Stat::Special(4)));
        assert_eq!(ActorValue(35).stat(), Some(Stat::Skill(Skill::Traps)));
        assert_eq!(ActorValue(37).stat(), Some(Stat::Skill(Skill::Doctor)));
        assert_eq!(ActorValue(16).stat(), None);
        assert_eq!(Skill::Speech as u8, 14);
        assert_eq!(Skill::Outdoorsman as u8, 17);
    }
}
