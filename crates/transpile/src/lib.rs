//! The simulation layer's front end (Pass 2, `docs/simulation-layer-spec.md`,
//! contracts 11): dialogue, scripts, packages, quests, terminals and
//! enable-parent state, read from the MIR into one typed IR shared by
//! Fallout 3 and New Vegas.
//!
//! - [`ir`]: the IR types (contracts 11.1) and the module.
//! - [`build()`]: MIR records of the slice's placed set, placements and
//!   configuration to a [`ir::Module`]; [`source`] is what it reads.
//! - [`cond`]: condition lowering with the function table of spec 4.8 and
//!   the fallback policy of 4.9.
//! - [`obscript`]: the script language front end (spec 5): lexer, parser,
//!   names, the function signature table, lowering to IR statements.
//! - [`diag`]: diagnostics with line and column and counted categories.
//! - [`texts`]: the text table and the converter's own labels.
//!
//! Later modules (`dialogue`, `quest`, `state`, `terminals`, `packages`,
//! `codegen`, tasks T5, T6, T8) sit beside these and take a [`ir::Module`].
//! Malformed source never stops a build: it is a diagnostic.

pub mod build;
pub mod cond;
pub mod diag;
mod error;
pub mod ir;
pub mod obscript;
pub mod source;
pub mod texts;

pub use build::{build, build_module};
pub use error::TranspileError;
