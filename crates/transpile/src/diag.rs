//! Diagnostics: what the front end found, with a position in a script's
//! source where it has one, and a category the report counts. Errors are
//! data: nothing in the front end panics on malformed input.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ir::SimId;

/// A position in a script's source: line and column, from 1. The column
/// counts characters.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Span {
    /// Line, from 1.
    pub line: u32,
    /// Column, from 1.
    pub column: u32,
}

impl Span {
    /// A position.
    pub fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// How bad it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Something was lost: a statement, a block, a condition term.
    Error,
    /// Something was approximated.
    Warning,
    /// Worth counting, nothing lost.
    Note,
}

/// What kind of finding; the report groups by [`Category::group`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    // ---- lexer
    /// A character the language does not use.
    BadCharacter,
    /// A string with no closing quote on its line.
    UnterminatedString,
    /// A number that does not parse.
    BadNumber,
    // ---- parser
    /// A token where the grammar wants another.
    UnexpectedToken,
    /// Tokens left after a complete statement.
    TrailingTokens,
    /// An expression that does not parse.
    BadExpression,
    /// A `set` without `to`, or without a variable.
    BadSet,
    /// A declaration without a name.
    BadDeclaration,
    /// A declaration inside a block, or in a result script.
    MisplacedDeclaration,
    /// A `begin` without a block type, or nested in a block.
    BadBlock,
    /// A block with no `end`.
    MissingEnd,
    /// An `end` with no `begin`.
    StrayEnd,
    /// An `if` with no `endif`.
    MissingEndif,
    /// An `elseif`, `else` or `endif` with no `if`.
    StrayElse,
    /// A statement outside any block of a script record.
    OutsideBlock,
    /// A `begin` in a result script.
    BlockInResult,
    /// A `scn` that is not the first line, or a second one.
    MisplacedHeader,
    /// `=` used as a comparison.
    AssignAsCompare,
    // ---- names
    /// A name that is no variable, form or function.
    UnresolvedName,
    /// `quest.var` where the quest has no such variable.
    UnresolvedQuestVar,
    /// `ref.var` where the reference's script has no such variable, or
    /// the reference has no script.
    UnresolvedRefVar,
    /// A name with several forms, none on the script's reference list.
    AmbiguousName,
    /// A function called on something that is not a reference.
    CallOnNonReference,
    /// A base actor used as a reference, taken as its only placed
    /// reference.
    BaseAsReference,
    /// `set` on something that is not a variable.
    BadAssignment,
    // ---- functions
    /// A function the signature table does not have.
    UnknownFunction,
    /// A function the table lists as dropped in Pass 2.
    DroppedFunction,
    /// A function given too few or too many arguments.
    ArgumentCount,
    /// An argument of the wrong kind.
    ArgumentKind,
    // ---- blocks and statements
    /// A block type Pass 2 does not run (spec 5.3).
    UnsupportedBlock,
    /// A block whose statements are all unsupported calls.
    EmptyBlock,
    // ---- conditions
    /// A condition term taken with a fallback value (spec 4.9).
    ConditionFallback,
    /// A condition function with no table entry.
    ConditionUnknownFunction,
    /// A condition parameter that names no usable form.
    ConditionBadParameter,
    /// A condition run on something Pass 2 cannot name (combat target).
    ConditionRunOn,
    /// A comparison code outside 0 to 5.
    ConditionComparison,
    // ---- build
    /// A form the build needs is not in the MIR.
    MissingRecord,
    /// A reference named by the slice's code that is not placed in it.
    RefOutsideSlice,
    /// A source flag Pass 2 ignores by owner decision (essential).
    IgnoredFlag,
    /// The quest closure stopped at its depth.
    QuestDepth,
    /// A game setting the build reads is missing; a default is used.
    MissingSetting,
    // ---- dialogue (task T5, spec 4)
    /// Bark INFOs of a speaker that are not lowered (hello, combat,
    /// detection, conversation; owner decision 14).
    DialogueBark,
    /// A speaker whose every greeting INFO folds away: the converter
    /// greeting label opens the hub (spec 4.9).
    GreetingLabel,
    /// A challenge whose speaker has no failure INFO (spec 4.7).
    NoFailureTopic,
    /// A check labelled by a perk, not an actor value.
    PerkLabel,
    /// A topic an INFO links to that is not in the MIR.
    MissingTopic,
    /// A dialogue topic a script says (`Say`, `SayTo`) that its speaker
    /// cannot say.
    SayUnresolved,
    // ---- quests and scripts (spec 5.5, 6.2)
    /// A block sets a stage and then reads what that stage's script
    /// changes, before the script has run (spec 6.2).
    StageOrdering,
    /// Statements after a self-destroying word (spec 5.5).
    SelfDestroyOrdering,
    /// A quest without a name or objective text gets no Pip-Boy rows.
    NoJournal,
    // ---- world state (spec 8)
    /// The placement index and the MIR disagree about a group.
    GroupMismatch,
    /// A child of a group that the map stage did not place.
    ChildNotPlaced,
    /// `Enable` or `Disable` of a reference that is no group's parent.
    NotAGroup,
    // ---- terminals, notes, locks (spec 9)
    /// A sub-terminal not in the MIR.
    MissingSubTerminal,
    /// A lock whose key did not convert: the lock is left to the skill.
    KeyNotConverted,
    /// A lock level off the 0/25/50/75/100 grid, rounded up.
    LockLevelRounded,
    /// An image or sound note: the holodisk shows its title only.
    NoteTitleOnly,
    /// A note's body is empty.
    EmptyNote,
    /// A message a script shows is not in the MIR.
    MissingMessage,
    /// A message text with format codes (`%`), kept as written.
    MessageFormat,
    // ---- packages (spec 7)
    /// A package type no runner covers, or an unusable location: the
    /// actor stands (counted fallback).
    PackageFallback,
    /// A furniture action (eat, sleep): the actor stands at the place
    /// (gap F).
    FurnitureAction,
    /// No walkable data for a sandbox: its anchor is its only candidate.
    NoWalkable,
    /// A package location on a map the slice does not hold.
    LocationOutsideSlice,
    /// A presence copy (spec 7.6), for the report.
    PresenceCopy,
    /// A `SimConfig::team_factions` editor id that names no faction of the
    /// plugin list (spec 7.5).
    UnknownTeamFaction,
}

/// The report section of spec 10.5 a finding belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    /// `## Dialogue`
    Dialogue,
    /// `## Scripts`
    Scripts,
    /// `## Quests`
    Quests,
    /// `## World state`
    WorldState,
    /// `## Terminals and notes`, with `## Locks`.
    Terminals,
    /// `## Packages`
    Packages,
    /// Build-wide findings (`## Summary`).
    Build,
}

impl Section {
    /// Every section, in report order.
    pub const ALL: [Section; 7] = [
        Section::Dialogue,
        Section::Scripts,
        Section::Quests,
        Section::WorldState,
        Section::Terminals,
        Section::Packages,
        Section::Build,
    ];

    /// The lowercase name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dialogue => "dialogue",
            Self::Scripts => "scripts",
            Self::Quests => "quests",
            Self::WorldState => "state",
            Self::Terminals => "terminals",
            Self::Packages => "packages",
            Self::Build => "build",
        }
    }

    /// The section of the things a [`SimId`] kind names.
    pub fn of_kind(kind: crate::ir::SimKind) -> Self {
        use crate::ir::SimKind::*;
        match kind {
            Topic | Info => Self::Dialogue,
            Quest | Stage | Objective | QuestVar => Self::Quests,
            Global | Text | Message | Actor => Self::Build,
            Script | ScriptVar => Self::Scripts,
            Package => Self::Packages,
            Group | Ref => Self::WorldState,
            Terminal | MenuItem | Note => Self::Terminals,
        }
    }
}

/// What a finding counts as in its section (spec 10.5: compiled, folded,
/// dropped, fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Something was decided at build time and removed.
    Folded,
    /// Something was lost.
    Dropped,
    /// Something was approximated: a counted fallback.
    Fallback,
    /// Worth counting, nothing lost.
    Note,
}

impl Category {
    /// The report group: `lex`, `parse`, `name`, `function`, `block`,
    /// `condition` or `build`.
    pub fn group(self) -> &'static str {
        use Category::*;
        match self {
            BadCharacter | UnterminatedString | BadNumber => "lex",
            UnexpectedToken | TrailingTokens | BadExpression | BadSet | BadDeclaration
            | MisplacedDeclaration | BadBlock | MissingEnd | StrayEnd | MissingEndif
            | StrayElse | OutsideBlock | BlockInResult | MisplacedHeader | AssignAsCompare => {
                "parse"
            }
            UnresolvedName | UnresolvedQuestVar | UnresolvedRefVar | AmbiguousName
            | CallOnNonReference | BaseAsReference | BadAssignment => "name",
            UnknownFunction | DroppedFunction | ArgumentCount | ArgumentKind => "function",
            UnsupportedBlock | EmptyBlock => "block",
            ConditionFallback
            | ConditionUnknownFunction
            | ConditionBadParameter
            | ConditionRunOn
            | ConditionComparison => "condition",
            MissingRecord | RefOutsideSlice | IgnoredFlag | QuestDepth | MissingSetting => "build",
            DialogueBark | GreetingLabel | NoFailureTopic | PerkLabel | MissingTopic
            | SayUnresolved => "dialogue",
            StageOrdering | SelfDestroyOrdering | NoJournal => "quest",
            GroupMismatch | ChildNotPlaced | NotAGroup => "state",
            MissingSubTerminal | KeyNotConverted | LockLevelRounded | NoteTitleOnly | EmptyNote
            | MissingMessage | MessageFormat => "terminal",
            PackageFallback | FurnitureAction | NoWalkable | LocationOutsideSlice
            | PresenceCopy | UnknownTeamFaction => "package",
        }
    }

    /// The report section: the category's own for the T5 and T6 groups,
    /// else the owner's ([`Section::of_kind`]), else build.
    pub fn section(self, owner: Option<&SimId>) -> Section {
        match self.group() {
            "dialogue" => Section::Dialogue,
            "quest" => match self {
                Category::SelfDestroyOrdering => Section::Scripts,
                _ => Section::Quests,
            },
            "state" => Section::WorldState,
            "terminal" => Section::Terminals,
            "package" => Section::Packages,
            _ => owner.map_or(Section::Build, |o| Section::of_kind(o.kind)),
        }
    }

    /// What the finding counts as.
    pub fn outcome(self, severity: Severity) -> Outcome {
        use Category::*;
        match self {
            DialogueBark | UnsupportedBlock | DroppedFunction | UnknownFunction | EmptyBlock
            | MissingTopic | MissingSubTerminal | MissingMessage | ChildNotPlaced
            | SayUnresolved => Outcome::Dropped,
            ConditionFallback if severity == Severity::Note => Outcome::Note,
            ConditionFallback
            | ConditionUnknownFunction
            | ConditionBadParameter
            | ConditionRunOn
            | GreetingLabel
            | NoFailureTopic
            | PerkLabel
            | KeyNotConverted
            | NoteTitleOnly
            | PackageFallback
            | FurnitureAction
            | NoWalkable
            | LocationOutsideSlice
            | UnknownTeamFaction
            | MissingSetting => Outcome::Fallback,
            _ => match severity {
                Severity::Error => Outcome::Dropped,
                Severity::Warning | Severity::Note => Outcome::Note,
            },
        }
    }

    /// The category's name as serialised.
    pub fn name(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Diag {
    /// How bad.
    pub severity: Severity,
    /// What kind.
    pub category: Category,
    /// What it is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<SimId>,
    /// Where in the owner's source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Span>,
    /// The name, function or token concerned, when there is one: the key
    /// the report counts by.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    /// A sentence for a reader.
    pub message: String,
}

impl Diag {
    /// A finding with no owner and no position.
    pub fn new(severity: Severity, category: Category, message: impl Into<String>) -> Self {
        Self {
            severity,
            category,
            owner: None,
            at: None,
            subject: String::new(),
            message: message.into(),
        }
    }

    /// An error.
    pub fn error(category: Category, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, category, message)
    }

    /// A warning.
    pub fn warning(category: Category, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, category, message)
    }

    /// A note.
    pub fn note(category: Category, message: impl Into<String>) -> Self {
        Self::new(Severity::Note, category, message)
    }

    /// Sets the owner.
    #[must_use]
    pub fn owned_by(mut self, owner: &SimId) -> Self {
        self.owner = Some(owner.clone());
        self
    }

    /// Sets the position.
    #[must_use]
    pub fn at(mut self, span: Span) -> Self {
        self.at = Some(span);
        self
    }

    /// Sets the subject.
    #[must_use]
    pub fn about(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        write!(f, "{severity}[{}]", self.category.name())?;
        if let Some(owner) = &self.owner {
            write!(f, " {owner}")?;
        }
        if let Some(at) = self.at {
            write!(f, " at {at}")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// One section's counts for the report (spec 10.5): what was compiled,
/// folded, dropped and approximated, and the commonest reasons for the
/// last three.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SectionSummary {
    /// The section.
    pub section: Option<Section>,
    /// What the section compiled, by kind (`"nodes"`, `"quests"`, ...).
    pub compiled: std::collections::BTreeMap<String, u64>,
    /// Things decided at build time and removed.
    pub folded: u64,
    /// Things lost.
    pub dropped: u64,
    /// Counted fallbacks.
    pub fallback: u64,
    /// Notes.
    pub notes: u64,
    /// `category subject` to count, for every dropped or fallback finding.
    pub reasons: std::collections::BTreeMap<String, u64>,
}

impl SectionSummary {
    /// The `n` commonest reasons, most first, then by name.
    pub fn top_reasons(&self, n: usize) -> Vec<(&str, u64)> {
        let mut out: Vec<(&str, u64)> =
            self.reasons.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        out.truncate(n);
        out
    }
}

fn diags_of(module: &crate::ir::Module) -> Vec<&Diag> {
    module
        .diagnostics
        .iter()
        .chain(module.scripts.values().flat_map(|s| s.diagnostics.iter()))
        .collect()
}

/// The report's counts per section, in [`Section::ALL`] order. Folded,
/// dropped and fallback counts are the findings' outcomes plus what the
/// IR counts itself (INFOs folded away per speaker, bark and unreached
/// INFOs, package slots that fall back).
pub fn summarize(module: &crate::ir::Module) -> Vec<SectionSummary> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<Section, SectionSummary> = Section::ALL
        .iter()
        .map(|s| {
            (
                *s,
                SectionSummary {
                    section: Some(*s),
                    ..SectionSummary::default()
                },
            )
        })
        .collect();
    for diag in diags_of(module) {
        let section = diag.category.section(diag.owner.as_ref());
        let entry = out.entry(section).or_default();
        let outcome = diag.category.outcome(diag.severity);
        match outcome {
            Outcome::Folded => entry.folded += 1,
            Outcome::Dropped => entry.dropped += 1,
            Outcome::Fallback => entry.fallback += 1,
            Outcome::Note => entry.notes += 1,
        }
        if matches!(outcome, Outcome::Dropped | Outcome::Fallback) {
            let reason = if diag.subject.is_empty() || diag.subject.len() > 60 {
                diag.category.name()
            } else {
                format!("{} {}", diag.category.name(), diag.subject)
            };
            *entry.reasons.entry(reason).or_default() += 1;
        }
    }
    let mut put = |section: Section, what: &str, n: usize| {
        *out.entry(section)
            .or_default()
            .compiled
            .entry(what.to_owned())
            .or_default() += n as u64;
    };
    // Dialogue.
    put(Section::Dialogue, "speakers", module.dialogues.len());
    let nodes: usize = module.dialogues.values().map(|d| d.nodes.len()).sum();
    put(Section::Dialogue, "nodes", nodes);
    put(
        Section::Dialogue,
        "topics",
        module.dialogues.values().map(|d| d.topics.len()).sum(),
    );
    let mut folded = 0u64;
    let mut barks = 0u64;
    let mut challenges = 0usize;
    let mut thresholds = 0usize;
    for d in module.dialogues.values() {
        folded += u64::from(d.counts.folded_away);
        barks += u64::from(d.counts.barks) + u64::from(d.counts.unreached);
        challenges += d.counts.challenges as usize;
        thresholds += d.counts.thresholds as usize;
    }
    put(Section::Dialogue, "challenges", challenges);
    put(Section::Dialogue, "thresholds", thresholds);
    if let Some(d) = out.get_mut(&Section::Dialogue) {
        d.folded += folded;
        d.dropped += barks;
        if barks > 0 {
            *d.reasons
                .entry("barks and unreached INFOs".to_owned())
                .or_default() += barks;
        }
    }
    let mut put = |section: Section, what: &str, n: usize| {
        *out.entry(section)
            .or_default()
            .compiled
            .entry(what.to_owned())
            .or_default() += n as u64;
    };
    // Scripts.
    put(Section::Scripts, "scripts", module.scripts.len());
    let blocks: usize = module.scripts.values().map(|s| s.blocks.len()).sum();
    put(Section::Scripts, "blocks", blocks);
    let mut calls = 0usize;
    for script in module.scripts.values() {
        for block in &script.blocks {
            calls += crate::ir::visit::calls_in_stmts(&block.body)
                .iter()
                .filter(|c| matches!(c.func, crate::ir::Func::Known(_)))
                .count();
        }
    }
    put(Section::Scripts, "calls", calls);
    // Quests.
    put(Section::Quests, "quests", module.quests.len());
    put(
        Section::Quests,
        "stages",
        module.quests.values().map(|q| q.stages.len()).sum(),
    );
    put(
        Section::Quests,
        "objectives",
        module.quests.values().map(|q| q.objectives.len()).sum(),
    );
    put(
        Section::Quests,
        "pipboy rows",
        module.journal.values().map(|j| j.rows.len()).sum(),
    );
    // World state.
    put(Section::WorldState, "groups", module.groups.len());
    put(
        Section::WorldState,
        "table rows",
        module.state.values().map(Vec::len).sum(),
    );
    // Terminals, notes, locks.
    put(
        Section::Terminals,
        "terminals",
        module.terminal_programs.len(),
    );
    put(
        Section::Terminals,
        "menus",
        module
            .terminal_programs
            .values()
            .map(|t| t.menus.len())
            .sum(),
    );
    put(Section::Terminals, "locks", module.locks.len());
    put(Section::Terminals, "holodisks", module.holodisks.len());
    put(Section::Terminals, "messages", module.messages.len());
    // Packages.
    put(Section::Packages, "actors", module.runs.len());
    let slots: usize = module.runs.values().map(|r| r.slots.len()).sum();
    put(Section::Packages, "slots", slots);
    put(
        Section::Packages,
        "hostile",
        module.runs.values().filter(|r| r.combat.hostile).count(),
    );
    put(Section::Packages, "teams", module.teams.len());
    put(
        Section::Packages,
        "presence copies",
        module.runs.values().map(|r| r.presence.len()).sum(),
    );
    out.into_values().collect()
}
