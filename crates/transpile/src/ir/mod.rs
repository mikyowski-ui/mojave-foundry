//! The IR (`transpile::ir`): plain data, built from the MIR by
//! [`crate::build`], checked, then lowered. It never holds engine numbers
//! (contracts 11.1, spec 3). Every map is a `BTreeMap` and every list is in
//! source or [`SimId`] order, so [`Module::to_json`] is a stable snapshot.

mod dialogue;
mod expr;
mod id;
mod module;
mod package;
mod quest;
mod script;
mod world;

pub use dialogue::{
    ActorValue, FailureRoute, Info, InfoFlags, Remember, Response, Skill, SkillCheck, SpeakerSet,
    Stat, Topic, TopicKind,
};
pub use expr::{BinOp, Call, CmpOp, Cond, Expr, Func, FuncId, RefExpr, UnOp, VarRef, VarTy};
pub use id::{BadSimId, FormKey, SimId, SimKind, Text, TextId, TextRole, quest_sub, script_sub};
pub use module::{
    BaseFiles, Lowered, Module, PlacedKind, PlacedObject, Placement, SimConfig, SimSlice,
};
pub use package::{Location, Package, PackageEvent, PackageKind, Runner, Schedule, Target};
pub use quest::{LogEntry, Objective, Quest, Stage};
pub use script::{Block, Event, ResultOwner, Script, ScriptKind, Stmt, VarDecl};
pub use world::{
    Actor, ActorKind, AiData, EnableGroup, Global, GlobalPurpose, MenuItem, Note, NoteBody,
    RefEntry, RefUses, Terminal, fixed_point, global_name,
};
