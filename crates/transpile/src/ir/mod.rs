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
pub mod visit;
mod world;

pub use dialogue::{
    ActorValue, CheckSettings, Dialogue, DialogueCounts, FailureRoute, HubEntry, Info, InfoFlags,
    Node, Pick, Remember, Response, Skill, SkillCheck, SpeakerSet, SpeakerTopic, Stat, Topic,
    TopicKind,
};
pub use expr::{BinOp, Call, CmpOp, Cond, Expr, Func, FuncId, RefExpr, UnOp, VarRef, VarTy};
pub use id::{
    BadSimId, FormKey, SimId, SimKind, Text, TextId, TextRole, actor_sub, once_sub, quest_sub,
    script_sub,
};
pub use module::{
    BaseFiles, Lowered, Module, PlacedGroup, PlacedKind, PlacedLock, PlacedObject, Placement,
    SimConfig, SimSlice, WalkMask,
};
pub use package::{
    ActorRun, Anchor, Combat, HexAt, Location, Package, PackageEvent, PackageKind, Place, RunSlot,
    RunTarget, Runner, Schedule, Target,
};
pub use quest::{
    Journal, JournalRow, LogEntry, Objective, Quest, Stage, objective_state, quest_flags,
};
pub use script::{Block, Event, ResultOwner, Script, ScriptKind, Stmt, VarDecl};
pub use world::{
    Actor, ActorKind, AiData, EnableGroup, Global, GlobalPurpose, Hack, Holodisk, HolodiskSource,
    Lock, MenuItem, MessageBox, Note, NoteBody, RefEntry, RefUses, StateChild, Terminal,
    TerminalProgram, fixed_point, global_name,
};
