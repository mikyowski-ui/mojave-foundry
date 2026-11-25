//! The function signature table (spec 5.2, 5.3, 5.9, 4.8): every function
//! the spec says is compiled, folded or dropped, with its arguments.
//!
//! Names are the public function list's; aliases are its short forms.
//! `index` is the public condition-function index, given only for the
//! functions a condition can use (spec 4.8 and the package conditions);
//! the script-only functions have none here, because nothing in Pass 2
//! needs their opcode.
//!
//! The parser asks this table how many arguments a call inside an
//! expression takes (spec 5.2); the resolver asks it what kind each
//! argument is; the report counts calls by [`Status`].

use std::collections::HashMap;
use std::sync::OnceLock;

/// What Pass 2 does with a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    /// Lowered to engine words (spec 5.9, 4.8).
    Compiled,
    /// Evaluated by the front end into a plain expression (`GetSelf` is
    /// the running object, `GetGlobalValue g` the global's variable).
    Folded,
    /// Known, and dropped with a report line (spec 5.3, 5.9's last row,
    /// the fallback functions of 4.8).
    Dropped,
}

impl Status {
    /// Lowercase name for the report.
    pub fn name(self) -> &'static str {
        match self {
            Self::Compiled => "compiled",
            Self::Folded => "folded",
            Self::Dropped => "dropped",
        }
    }
}

/// The kind of one argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arg {
    /// An integer.
    Int,
    /// A float.
    Float,
    /// A number of either kind.
    Number,
    /// A string literal.
    Str,
    /// A base form (item, actor base, message, note, package, faction,
    /// cell, ...).
    Form,
    /// A placed reference, `player`, or a `ref` variable.
    Ref,
    /// A quest.
    Quest,
    /// A `GLOB`.
    Global,
    /// An actor value name (`Health`, `Speech`).
    ActorValue,
    /// A variable name of another script (`GetQuestVariable q var`).
    Var,
    /// A bare word the function takes as is: an animation group, an axis
    /// (`X`, `Y`, `Z`), a sex (`Male`, `Female`), a game setting name.
    Keyword,
    /// Anything.
    Any,
}

/// One function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    /// The canonical name.
    pub name: &'static str,
    /// Other spellings.
    pub aliases: &'static [&'static str],
    /// The public condition-function index, for the condition functions.
    pub index: Option<u16>,
    /// The arguments, in order; the first `min` are required.
    pub args: &'static [Arg],
    /// How many arguments are required.
    pub min: u8,
    /// What Pass 2 does with it.
    pub status: Status,
}

impl Signature {
    /// The most arguments it takes.
    pub fn max(&self) -> usize {
        self.args.len()
    }

    const fn alias(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    const fn cond(mut self, index: u16) -> Self {
        self.index = Some(index);
        self
    }
}

const fn row(name: &'static str, args: &'static [Arg], min: u8, status: Status) -> Signature {
    Signature {
        name,
        aliases: &[],
        index: None,
        args,
        min,
        status,
    }
}

/// Compiled, all arguments required.
const fn c(name: &'static str, args: &'static [Arg]) -> Signature {
    row(name, args, args.len() as u8, Status::Compiled)
}

/// Compiled, the first `min` arguments required.
const fn co(name: &'static str, args: &'static [Arg], min: u8) -> Signature {
    row(name, args, min, Status::Compiled)
}

/// Folded, all arguments required.
const fn f(name: &'static str, args: &'static [Arg]) -> Signature {
    row(name, args, args.len() as u8, Status::Folded)
}

/// Dropped, the first `min` arguments required.
const fn d(name: &'static str, args: &'static [Arg], min: u8) -> Signature {
    row(name, args, min, Status::Dropped)
}

use Arg::{ActorValue as Av, Any, Float, Form, Global, Int, Keyword, Number, Quest, Ref, Str, Var};

const NINE: &[Arg] = &[Any, Any, Any, Any, Any, Any, Any, Any, Any];
const SHOW_MESSAGE: &[Arg] = &[Form, Any, Any, Any, Any, Any, Any, Any, Any, Any];
const MESSAGE_BOX: &[Arg] = &[Str, Any, Any, Any, Any, Any, Any, Any, Any, Any];
const CONTROLS: &[Arg] = &[Int, Int, Int, Int, Int, Int, Int];

/// The table. Order is irrelevant to lookups; it is grouped by the spec's
/// sections for reading.
pub static TABLE: &[Signature] = &[
    // ---- spec 5.9: quests
    c("SetStage", &[Quest, Int]),
    c("GetStage", &[Quest]).cond(58),
    c("GetStageDone", &[Quest, Int]).cond(59),
    c("StartQuest", &[Quest]),
    c("StopQuest", &[Quest]),
    c("GetQuestRunning", &[Quest]).cond(56),
    co("SetObjectiveDisplayed", &[Quest, Int, Int], 2),
    co("SetObjectiveCompleted", &[Quest, Int, Int], 2),
    c("GetObjectiveDisplayed", &[Quest, Int]),
    c("GetObjectiveCompleted", &[Quest, Int]),
    // ---- world state
    co("Enable", &[Int], 0),
    co("Disable", &[Int], 0),
    c("GetDisabled", &[]).cond(35),
    // ---- items and notes
    co("AddItem", &[Form, Int, Int], 2),
    co("RemoveItem", &[Form, Int, Int], 2),
    c("GetItemCount", &[Form]).cond(47),
    co("RemoveAllItems", &[Ref, Int], 0),
    c("AddNote", &[Form]),
    c("GetHasNote", &[Form]).cond(382),
    c("RemoveNote", &[Form]),
    // ---- messages
    co("ShowMessage", SHOW_MESSAGE, 1),
    c("GetButtonPressed", &[]),
    // ---- movement, death, AI
    co("MoveTo", &[Ref, Float, Float, Float], 1),
    co("Kill", &[Ref, Int, Int], 0).alias(&["KillActor"]),
    c("GetDead", &[]).cond(46),
    c("GetDeadCount", &[Form]).cond(84),
    c("EvaluatePackage", &[]).alias(&["EVP"]),
    c("AddScriptPackage", &[Form]),
    co("RemoveScriptPackage", &[Form], 0),
    c("ResetAI", &[]),
    co("StartCombat", &[Ref], 0),
    // ---- actor values
    c("SetActorValue", &[Av, Number]).alias(&["SetAV"]),
    c("ModActorValue", &[Av, Number]).alias(&["ModAV"]),
    c("RestoreActorValue", &[Av, Number]).alias(&["RestoreAV"]),
    c("GetActorValue", &[Av]).alias(&["GetAV"]).cond(14),
    c("GetBaseActorValue", &[Av])
        .alias(&["GetBaseAV"])
        .cond(277),
    c("GetPermanentActorValue", &[Av]).cond(495),
    // ---- rewards, barter, locks, doors
    c("RewardXP", &[Int]),
    c("RewardKarma", &[Int]),
    co("ShowBarterMenu", &[Int], 0),
    co("Lock", &[Int, Int], 0),
    co("Unlock", &[Int], 0),
    c("GetLocked", &[]).cond(5),
    c("SetOpenState", &[Int]),
    co("Activate", &[Ref, Int], 0),
    // ---- time and chance
    c("GetSecondsPassed", &[]).cond(12),
    c("GetCurrentTime", &[]).cond(18),
    c("GetRandomPercent", &[]).cond(77),
    // ---- actors
    co("PlaceLeveledActorAtMe", &[Form, Int], 1),
    c("AddToFaction", &[Form, Int]),
    c("RemoveFromFaction", &[Form]),
    c("SetFactionRank", &[Form, Int]),
    co("DisablePlayerControls", CONTROLS, 0),
    co("EnablePlayerControls", CONTROLS, 0),
    co("Say", &[Form, Int, Ref, Int], 1),
    co("SayTo", &[Ref, Form, Int, Int], 2),
    // ---- dialogue state (spec 4.5)
    c("AddTopic", &[Form]),
    // ---- spec 4.8: condition functions with a runtime lowering
    c("GetIsID", &[Form]).cond(72),
    c("GetInCell", &[Form]).cond(67),
    c("GetTalkedToPC", &[]).cond(50),
    c("GetPCIsSex", &[Keyword]).cond(131),
    c("GetScriptVariable", &[Ref, Var]).cond(53),
    c("GetIsSex", &[Keyword]).cond(70),
    c("GetEquipped", &[Form]).cond(182),
    c("GetInFaction", &[Form]).cond(71),
    c("GetIsCurrentPackage", &[Form]).cond(161),
    c("GetHealthPercentage", &[]).cond(431),
    c("GetDistance", &[Ref]).cond(1),
    c("IsInInterior", &[]).cond(300),
    c("IsInCombat", &[]).cond(289),
    c("GetInWorldspace", &[Form]).cond(310),
    c("GetInSameCell", &[Ref]).cond(32),
    c("GetIsReference", &[Ref]).cond(136),
    c("GetDetected", &[Ref]).cond(45),
    // Static on the speaker (spec 4.2); at run time a comparison of the
    // base record's fields, which the dialogue compiler folds.
    c("GetIsVoiceType", &[Form]).cond(427),
    c("GetIsRace", &[Form]).cond(69),
    c("GetIsClass", &[Form]).cond(68),
    c("GetIsCreature", &[]).cond(64),
    // ---- folded by the front end
    f("GetSelf", &[]).alias(&["This"]),
    f("GetActionRef", &[]),
    f("IsActionRef", &[Ref]),
    f("GetLinkedRef", &[]),
    f("GetGlobalValue", &[Global]).cond(74),
    f("GetQuestVariable", &[Quest, Var]).cond(79),
    // ---- spec 4.8: condition functions with a fallback value
    d("GetInZone", &[Form], 1).cond(446),
    d("GetOffersServicesNow", &[], 0).cond(255),
    d("HasPerk", &[Form, Int], 1).cond(449),
    d("GetMapMarkerVisible", &[], 0).cond(492),
    d("GetDisposition", &[Ref], 1).cond(76),
    d("Exists", &[Ref], 1).cond(415),
    d("HasFriendDisposition", &[], 0).cond(403),
    d("IsCombatTarget", &[Ref], 0).cond(515),
    d("GetSleeping", &[], 0).cond(49),
    d("WhichServiceMenu", &[], 0).cond(323),
    d("IsRaining", &[], 0).cond(62),
    // ---- spec 5.9's unsupported row, and 5.9 "dropped until the audio
    // brief"
    d("PlaySound", &[Form, Int], 1),
    d("PlaySound3D", &[Form], 1),
    d("AddPerk", &[Form, Int], 1),
    d("RemovePerk", &[Form], 1),
    d("AddSpell", &[Form], 1),
    d("RemoveSpell", &[Form], 1),
    d("AddAchievement", &[Int], 1),
    d("ApplyImageSpaceModifier", &[Form, Float], 1).alias(&["IMod"]),
    d("RemoveImageSpaceModifier", &[Form], 1).alias(&["RIMod"]),
    d("SetCellImageSpace", &[Form, Form], 2),
    d("SetEssential", &[Form, Int], 2),
    d("SetOwnership", &[Form], 0),
    d("ModPCMiscStat", &[Any, Int], 2),
    d("PlayIdle", &[Form], 1),
    d("SetUnconscious", &[Int], 1),
    d("SetPlayerTeammate", &[Int], 1),
    d("SetCombatStyle", &[Form], 1),
    d("ShowMap", &[Form, Int], 1),
    d("MatchRace", &[Ref], 1),
    d("MatchFaceGeometry", &[Ref, Int], 1),
    // ---- known and not in spec 5.9: dropped with a report line
    d("DamageActorValue", &[Av, Number], 2).alias(&["DamageAV"]),
    d("ForceActorValue", &[Av, Number], 2).alias(&["ForceAV"]),
    d("ModPCSkill", &[Av, Number], 2),
    d("StopCombat", &[Ref], 0),
    d("PlaceAtMe", &[Form, Int, Int, Int], 1),
    d("PushActorAway", &[Ref, Number], 2),
    d("PlayGroup", &[Keyword, Int], 1),
    d("IsAnimPlaying", &[Keyword], 0),
    d("GetGameSetting", &[Keyword], 1).alias(&["GetGS"]),
    d("GetPos", &[Keyword], 1),
    d("SetPos", &[Keyword, Float], 2),
    d("GetAngle", &[Keyword], 1),
    d("SetAngle", &[Keyword, Float], 2),
    d("GetStartingPos", &[Keyword], 1),
    d("GetStartingAngle", &[Keyword], 1),
    d("StartConversation", &[Ref, Form, Ref, Ref, Int, Int], 1),
    d("MessageBox", MESSAGE_BOX, 1),
    d("SetRestrained", &[Int], 1),
    d("SetGhost", &[Int], 1),
    d("SetDestroyed", &[Int], 1),
    d("SetAlert", &[Int], 1),
    d("SetAlly", &[Form, Form, Int, Int], 2),
    d("SetEnemy", &[Form, Form, Int, Int], 2),
    d("CastImmediateOnSelf", &[Form], 1).alias(&["CIOS"]),
    d("Dispel", &[Form], 1),
    d("Resurrect", &[Int], 0),
    d("GetOpenState", &[], 0).cond(157),
    d("MenuMode", &[Int], 0).cond(36),
    d("PrintC", NINE, 0).alias(&["PrintToConsole"]),
];

/// Every name and alias, lowercase, to its row.
fn names() -> &'static HashMap<String, &'static Signature> {
    static NAMES: OnceLock<HashMap<String, &'static Signature>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut map = HashMap::new();
        for sig in TABLE {
            map.insert(sig.name.to_ascii_lowercase(), sig);
            for alias in sig.aliases {
                map.insert(alias.to_ascii_lowercase(), sig);
            }
        }
        map
    })
}

/// The row of a function, by name or alias (case does not matter).
pub fn lookup(name: &str) -> Option<&'static Signature> {
    if name.bytes().any(|b| b.is_ascii_uppercase()) {
        names().get(&name.to_ascii_lowercase()).copied()
    } else {
        names().get(name).copied()
    }
}

/// The row of a condition function, by its index.
pub fn by_index(index: u16) -> Option<&'static Signature> {
    static INDEX: OnceLock<HashMap<u16, &'static Signature>> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            TABLE
                .iter()
                .filter_map(|sig| sig.index.map(|i| (i, sig)))
                .collect()
        })
        .get(&index)
        .copied()
}

/// How many rows have each status.
pub fn status_counts() -> [(Status, usize); 3] {
    let count = |s: Status| TABLE.iter().filter(|sig| sig.status == s).count();
    [
        (Status::Compiled, count(Status::Compiled)),
        (Status::Folded, count(Status::Folded)),
        (Status::Dropped, count(Status::Dropped)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_aliases_and_indices_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for sig in TABLE {
            assert!(seen.insert(sig.name.to_ascii_lowercase()), "{}", sig.name);
            for alias in sig.aliases {
                assert!(seen.insert(alias.to_ascii_lowercase()), "{alias}");
            }
            assert!(usize::from(sig.min) <= sig.max(), "{}", sig.name);
        }
        let mut indices = std::collections::HashSet::new();
        for sig in TABLE.iter().filter_map(|s| s.index) {
            assert!(indices.insert(sig), "index {sig}");
        }
    }

    #[test]
    fn lookups_ignore_case_and_take_aliases() {
        assert_eq!(lookup("setav").map(|s| s.name), Some("SetActorValue"));
        assert_eq!(lookup("GETSTAGE").map(|s| s.name), Some("GetStage"));
        assert_eq!(lookup("evp").map(|s| s.status), Some(Status::Compiled));
        assert_eq!(by_index(72).map(|s| s.name), Some("GetIsID"));
        assert_eq!(lookup("NoSuchFunction"), None);
        let counts = status_counts();
        assert_eq!(counts.iter().map(|(_, n)| n).sum::<usize>(), TABLE.len());
    }
}
