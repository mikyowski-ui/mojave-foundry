//! Quests (contracts 11.1, spec 3.6).

use serde::{Deserialize, Serialize};

use super::expr::Cond;
use super::id::{FormKey, SimId, TextId};
use super::script::VarDecl;

/// A quest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quest {
    /// [`super::SimKind::Quest`], sub 0.
    pub id: SimId,
    /// Editor id, as stored.
    pub editor_id: String,
    /// The name.
    pub name: Option<TextId>,
    /// `DATA` flag bit 0.
    pub start_enabled: bool,
    /// `DATA` priority.
    pub priority: u8,
    /// `DATA` delay; `None` or 0 means the default.
    pub delay_seconds: Option<f32>,
    /// The quest script.
    pub script: Option<SimId>,
    /// The quest script's variables.
    pub vars: Vec<VarDecl>,
    /// Stages, by index.
    pub stages: Vec<Stage>,
    /// Objectives, by index.
    pub objectives: Vec<Objective>,
    /// The quest's own conditions, which every INFO of the quest must also
    /// pass (task T5; the dialogue compiler adds them to each node).
    #[serde(default = "cond_true")]
    pub condition: Cond,
}

fn cond_true() -> Cond {
    Cond::True
}

/// A quest stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    /// [`super::SimKind::Stage`].
    pub id: SimId,
    /// The stage index.
    pub index: i16,
    /// Log entries, in file order. Every entry whose condition passes
    /// runs, in this order, each condition tested when its entry is
    /// reached (spec 6.2).
    pub entries: Vec<LogEntry>,
}

/// A stage's log entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Flag bit 0.
    pub complete: bool,
    /// Flag bit 1.
    pub fail: bool,
    /// When the entry applies.
    pub condition: Cond,
    /// Log text.
    pub text: Option<TextId>,
    /// Result script.
    pub script: Option<SimId>,
    /// Quest started next.
    pub next_quest: Option<SimId>,
}

/// A quest objective.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Objective {
    /// [`super::SimKind::Objective`].
    pub id: SimId,
    /// The objective index.
    pub index: i32,
    /// Its text.
    pub text: Option<TextId>,
    /// Targets and their conditions: kept, not lowered (gap O).
    pub targets: Vec<(FormKey, Cond)>,
}

/// The bits of a quest's `G_q_FLAGS` global (spec 6.1).
pub mod quest_flags {
    /// The quest is running (`StartQuest`, start enabled).
    pub const RUNNING: i32 = 0x01;
    /// The quest is completed (a log entry's "complete quest").
    pub const COMPLETED: i32 = 0x02;
    /// The quest has failed.
    pub const FAILED: i32 = 0x04;
    /// The start-enabled flag was applied (set in the initial value of a
    /// start-enabled quest, with [`RUNNING`]).
    pub const APPLIED: i32 = 0x08;
}

/// The values of a `G_q_OBJ_n` global (spec 6.1, 6.3).
pub mod objective_state {
    /// Not shown.
    pub const HIDDEN: i32 = 0;
    /// Shown in the Pip-Boy.
    pub const DISPLAYED: i32 = 1;
    /// Shown struck through.
    pub const COMPLETED: i32 = 2;
}

/// A quest's Pip-Boy group (spec 6.3, owner decision 8: objectives
/// only): one heading and one `quests.txt` row per objective. Built by
/// [`crate::quest`] for every quest with a name and at least one objective
/// with text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    /// The quest.
    pub quest: SimId,
    /// The heading: the quest's name, a new `map.msg` message.
    pub heading: TextId,
    /// The rows, by objective index.
    pub rows: Vec<JournalRow>,
}

/// One `quests.txt` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRow {
    /// The objective.
    pub objective: SimId,
    /// Its text, a new `quests.msg` message.
    pub text: TextId,
    /// The global the row reads: the objective's `G_q_OBJ_n`
    /// ([`super::SimKind::Objective`] id).
    pub global: SimId,
    /// Shown from this value ([`objective_state::DISPLAYED`]).
    pub display_at: i32,
    /// Struck through from this value ([`objective_state::COMPLETED`]).
    pub completed_at: i32,
}
