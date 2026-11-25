//! Scripts (contracts 11.1, spec 3.5).

use serde::{Deserialize, Serialize};

use super::expr::{Call, Expr, RefExpr, VarRef, VarTy};
use super::id::{FormKey, SimId};
use super::package::PackageEvent;
use crate::diag::Diag;

/// A compiled script: a `SCPT` record or an embedded result script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Script {
    /// [`super::SimKind::Script`].
    pub id: SimId,
    /// The `SCPT`'s editor id, or the owner's for a result script.
    pub editor_id: String,
    /// What it is.
    pub kind: ScriptKind,
    /// Its variables, by index.
    pub vars: Vec<VarDecl>,
    /// Its blocks, in source order. A result script has one
    /// [`Event::Result`] block.
    pub blocks: Vec<Block>,
    /// What the front end found, in source order.
    pub diagnostics: Vec<Diag>,
}

/// What a [`Script`] is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptKind {
    /// Script type 0.
    Object,
    /// Script type 1.
    Quest,
    /// Script type 0x100.
    Effect,
    /// An embedded result script.
    Result(ResultOwner),
}

/// Where a result script is embedded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultOwner {
    /// Before an INFO's `NEXT`.
    InfoBegin(SimId),
    /// After an INFO's `NEXT`.
    InfoEnd(SimId),
    /// A stage log entry ([`super::SimKind::Stage`]).
    Stage(SimId),
    /// A terminal menu item.
    MenuItem(SimId),
    /// A package event.
    PackageEvent(SimId, PackageEvent),
}

/// A declared variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarDecl {
    /// The index (`SLSD`; else the declaration order from 1).
    pub index: u32,
    /// The name, as declared.
    pub name: String,
    /// The type.
    pub ty: VarTy,
}

/// A block of a script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    /// When it runs.
    pub event: Event,
    /// What it does.
    pub body: Vec<Stmt>,
}

/// A block type (`begin <name>`), case-insensitive in the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// `GameMode`
    GameMode,
    /// `MenuMode`
    MenuMode,
    /// `OnActivate`
    OnActivate,
    /// `OnLoad`
    OnLoad,
    /// `OnDeath [killer]`
    OnDeath {
        /// Runs only for this killer.
        killer: Option<RefExpr>,
    },
    /// `OnHit [by]`
    OnHit {
        /// Runs only for this attacker.
        by: Option<RefExpr>,
    },
    /// `OnTrigger [by]`
    OnTrigger {
        /// Runs only for this mover.
        by: Option<RefExpr>,
    },
    /// `OnTriggerEnter [by]`
    OnTriggerEnter {
        /// Runs only for this mover.
        by: Option<RefExpr>,
    },
    /// `OnTriggerLeave [by]`
    OnTriggerLeave {
        /// Runs only for this mover.
        by: Option<RefExpr>,
    },
    /// `OnStartCombat`
    OnStartCombat,
    /// `OnCombatEnd`
    OnCombatEnd,
    /// `SayToDone`
    SayToDone,
    /// `OnPackageStart [package]`
    OnPackageStart(Option<FormKey>),
    /// `OnPackageEnd [package]`
    OnPackageEnd(Option<FormKey>),
    /// `OnPackageChange [package]`
    OnPackageChange(Option<FormKey>),
    /// `OnAdd`
    OnAdd,
    /// `OnEquip`
    OnEquip,
    /// `OnUnequip`
    OnUnequip,
    /// `ScriptEffectStart`
    ScriptEffectStart,
    /// `ScriptEffectUpdate`
    ScriptEffectUpdate,
    /// `ScriptEffectFinish`
    ScriptEffectFinish,
    /// The body of a result script.
    Result,
    /// Any other block type, by its name as written.
    Other(String),
}

/// A statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stmt {
    /// `set v to e`
    Set(VarRef, Expr),
    /// `if` / `elseif` arms, then `else`.
    If {
        /// Condition and body of each arm, in order.
        arms: Vec<(Expr, Vec<Stmt>)>,
        /// The `else` body (empty when there is none).
        otherwise: Vec<Stmt>,
    },
    /// A function call as a statement.
    Call(Call),
    /// `return`
    Return,
}
