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
}

/// A quest stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    /// [`super::SimKind::Stage`].
    pub id: SimId,
    /// The stage index.
    pub index: i16,
    /// Log entries, in file order.
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
