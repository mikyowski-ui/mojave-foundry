//! Back-end-only types (task T8): what [`crate::lower::lower_layer`] needs
//! beyond the IR, and what it returns beyond [`crate::ir::Lowered`].
//!
//! None of this is IR: it never reaches a snapshot. [`LowerFacts`] carries
//! the facts the lowering reads that the module does not hold today (the
//! placement index, item PIDs, the scripts of non-actor bases, locks and the
//! plugin's game settings); [`LoweredPlan`] is what the `sim` stage needs to
//! finish the layer (map attachments, manifest rows) and to write the
//! report (per-section counts, program list).

use std::collections::{BTreeMap, BTreeSet};

use emit::map::ScriptAttachment;

use crate::ir::{FormKey, Placement, SimId};

/// What the lowering reads besides the module. Every field may be empty:
/// the lowering then drops what needs it and counts the loss.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LowerFacts {
    /// The placement index (contracts 11.2): maps, hexes, elevations, PIDs
    /// and object ids. Without it no program is attached to a map and no
    /// reference slot resolves.
    pub placement: Placement,
    /// Base form to PID for forms that are not placed (items the items
    /// stage converted: keys, items `AddItem` gives). A placed base's PID
    /// is also taken from [`Self::placement`].
    pub pids: BTreeMap<FormKey, u32>,
    /// Non-actor base form (door, container, activator, item) to its object
    /// script ([`crate::ir::SimKind::Script`]). The IR names the scripts of
    /// actor bases only (`Actor::script`).
    pub base_scripts: BTreeMap<FormKey, SimId>,
    /// Map stems that are interiors (`IsInInterior`).
    pub interiors: BTreeSet<String>,
}

impl LowerFacts {
    /// The facts of a slice: its placement, nothing else.
    pub fn of_slice(slice: &crate::ir::SimSlice) -> Self {
        Self {
            placement: slice.placement.clone(),
            ..Self::default()
        }
    }
}

/// The quest script delay when a quest has none (spec 5.6, calibration).
pub const QUEST_DELAY_SECONDS: f32 = 5.0;

/// Everything [`crate::lower::lower_layer`] makes.
#[derive(Debug, Clone, PartialEq)]
pub struct LowerOutput {
    /// The layer's files and the lowering's diagnostics.
    pub lowered: crate::ir::Lowered,
    /// What the `sim` stage needs to finish the layer and the report.
    pub plan: LoweredPlan,
}

/// The lowering's plan: programs, attachments, manifest rows, counts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LoweredPlan {
    /// Every program, in `scripts.lst` row order.
    pub programs: Vec<ProgramPlan>,
    /// Per map, the script records to attach (`emit::map::attach_scripts`)
    /// and the map script's row; sorted by map stem.
    pub maps: Vec<MapPlan>,
    /// `[[global_scripts]]` rows (spec 5.11).
    pub global_scripts: Vec<GlobalScriptRow>,
    /// `[[hooks]]` rows (spec 5.11).
    pub hooks: Vec<HookRow>,
    /// The report's counts (spec 10.5).
    pub counts: Counts,
    /// The report's summary line.
    pub summary: Summary,
    /// Each program's assembler source, by stem (for the report, the debug
    /// overlay and the program cache of spec 10.3).
    pub sources: BTreeMap<String, String>,
}

/// One program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramPlan {
    /// The file stem (`md000001`).
    pub stem: String,
    /// Its `scripts.lst` row (0-based script index).
    pub row: u32,
    /// What it is: `map`, `critter`, `object`, `terminal`, `note`,
    /// `dispatcher`.
    pub kind: &'static str,
    /// The source form it was made for (map stem for a map script).
    pub owner: String,
    /// The `scripts.lst` comment (editor id).
    pub editor_id: String,
    /// The local variable count of its row.
    pub local_vars: u32,
    /// Its procedures, in record order.
    pub procedures: Vec<String>,
    /// Whether it has a dialogue message file.
    pub messages: bool,
}

/// The script records of one map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapPlan {
    /// The map's stem.
    pub map: String,
    /// The map's number.
    pub map_number: u32,
    /// The map script's row, when the map has one.
    pub map_script: Option<u32>,
    /// The object attachments.
    pub attachments: Vec<ScriptAttachment>,
}

/// A `[[global_scripts]]` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalScriptRow {
    /// `<stem>.int`.
    pub file: String,
    /// Steps between runs of `start`.
    pub repeat: u32,
    /// The script type, 0 to 3.
    pub kind: u32,
}

/// A `[[hooks]]` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRow {
    /// The hook point (`HOOK_GAMEMODECHANGE`).
    pub point: String,
    /// `<stem>.int`.
    pub file: String,
    /// The procedure.
    pub procedure: String,
}

/// The summary of spec 10.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    /// Programs written.
    pub programs: u32,
    /// `scripts.lst` rows added.
    pub rows: u32,
    /// Campaign globals added.
    pub globals: u32,
    /// Message records written (dialogue and shared files).
    pub messages: u32,
    /// Holodisk rows added.
    pub holodisks: u32,
    /// `quests.txt` rows added.
    pub quest_rows: u32,
    /// Text characters changed by the MSG adapter (braces, characters
    /// outside Windows-1252, controls).
    pub text_changes: u32,
}

/// Per-section counts (spec 10.5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Counts {
    /// INFOs, topics, options, checks.
    pub dialogue: SectionCounts,
    /// Script blocks and statements.
    pub scripts: SectionCounts,
    /// Quests, stages, objectives.
    pub quests: SectionCounts,
    /// Enable groups and their children.
    pub state: SectionCounts,
    /// Terminals, notes, locks.
    pub terminals: SectionCounts,
    /// Packages.
    pub packages: SectionCounts,
}

/// The counts of one section, with the reasons for every loss.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SectionCounts {
    /// Lowered to engine words.
    pub compiled: u32,
    /// Decided at build time (folded to a constant, or folded away).
    pub folded: u32,
    /// Not lowered, by design or because something is missing.
    pub dropped: u32,
    /// Lowered with a fallback value.
    pub fallback: u32,
    /// Why, for the folded, dropped and fallback ones.
    pub reasons: BTreeMap<String, Reason>,
}

/// One reason line of the report.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reason {
    /// How many.
    pub count: u32,
    /// The first owners, by editor id or form, at most
    /// [`Reason::OWNERS`].
    pub owners: Vec<String>,
}

impl Reason {
    /// How many owners a reason keeps.
    pub const OWNERS: usize = 5;
}

/// What a count is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// [`SectionCounts::compiled`].
    Compiled,
    /// [`SectionCounts::folded`].
    Folded,
    /// [`SectionCounts::dropped`].
    Dropped,
    /// [`SectionCounts::fallback`].
    Fallback,
}

impl SectionCounts {
    /// Counts one item; a reason is kept for everything but a plain
    /// compile.
    pub fn count(&mut self, outcome: Outcome, reason: &str, owner: &str) {
        match outcome {
            Outcome::Compiled => self.compiled += 1,
            Outcome::Folded => self.folded += 1,
            Outcome::Dropped => self.dropped += 1,
            Outcome::Fallback => self.fallback += 1,
        }
        if outcome == Outcome::Compiled && reason.is_empty() {
            return;
        }
        let entry = self.reasons.entry(reason.to_owned()).or_default();
        entry.count += 1;
        if entry.owners.len() < Reason::OWNERS && !entry.owners.iter().any(|o| o == owner) {
            entry.owners.push(owner.to_owned());
        }
    }
}
