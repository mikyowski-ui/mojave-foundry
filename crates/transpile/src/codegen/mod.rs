//! Code generation (spec 5.10, task T8): IR to assembler text under the
//! Fallout 2 word set (`emit::int::Words::FALLOUT2`, no sfall word, no
//! manifest scripting word), then to `.int` bytes with `emit::int::assemble`.
//!
//! - [`asm`]: procedures and programs with the stack discipline that makes
//!   the engine's stack model (SPEC-017 section 8) find nothing.
//! - [`words`]: the host words used, with their stack effects.
//! - `generator`: expressions, conditions, variables, references, statements.
//! - `calls`: the function lowering of spec 5.9 and 4.8.
//! - `dialogue`, `world`, `quest`, `packages`, `programs`: the procedures
//!   of each construct and each program kind.
//!
//! Programs take no arguments; values pass through program variables and
//! frame temporaries. Every host-called procedure ends with a placeholder
//! result above the saved context (E2). Record 0 is `root`, sharing
//! `start`'s body; record 1 is `start`, the engine's fallback for a
//! procedure a program does not have (SPEC-004 section 3.7), so every
//! `start` is idempotent.

pub(crate) mod asm;
mod calls;
mod dialogue;
mod generator;
mod packages;
mod programs;
mod quest;
pub mod words;
mod world;

use std::collections::BTreeMap;

use crate::diag::Diag;
use crate::lowered::Counts;
use crate::numbering::{ProgramKey, ProgramNumber};

pub(crate) use dialogue::DialoguePlan;
pub(crate) use generator::Ctx;

/// The most program variables (the global area, kept on the value stack
/// for the program's life) a program may have.
pub const MAX_PROGRAM_VARIABLES: usize = 100;

/// One generated program.
#[derive(Debug, Clone)]
pub struct Program {
    /// What it is for.
    pub key: ProgramKey,
    /// Its stem.
    pub stem: String,
    /// Its `scripts.lst` row.
    pub row: u32,
    /// The assembler source.
    pub source: String,
    /// The assembled `.int`.
    pub bytes: Vec<u8>,
    /// Its dialogue message file: id to source text (empty for none).
    pub messages: BTreeMap<i32, String>,
    /// Its row's local variable count.
    pub local_vars: u32,
    /// Its procedures in record order.
    pub procedures: Vec<String>,
    /// The `scripts.lst` comment.
    pub editor_id: String,
    /// What it counted.
    pub counts: Counts,
    /// What it found.
    pub diagnostics: Vec<Diag>,
}

/// Generates and assembles one program.
pub(crate) fn program(ctx: &Ctx<'_>, number: &ProgramNumber) -> Result<Program, String> {
    let mut gw = generator::Gen::new(ctx, number.key.clone(), number.list());
    gw.generate();
    let editor_id = comment(&gw.scope.owner);
    let title = format!("{} {} for {}", number.stem, number.key.kind(), editor_id);
    let (deepest, proc_name) = gw.asm.deepest().clone();
    if deepest > asm::MAX_FRAME_VALUES {
        return Err(format!(
            "{}: procedure {proc_name} holds {deepest} values above its frame (limit {})",
            number.stem,
            asm::MAX_FRAME_VALUES
        ));
    }
    if gw.asm.pvar_count() > MAX_PROGRAM_VARIABLES {
        return Err(format!(
            "{}: {} program variables (limit {MAX_PROGRAM_VARIABLES})",
            number.stem,
            gw.asm.pvar_count()
        ));
    }
    let source = gw.asm.text(&title);
    let bytes = emit::int::assemble(&source, &emit::int::Words::FALLOUT2)
        .map_err(|e| format!("{}: assembling: {e}", number.stem))?;
    Ok(Program {
        key: number.key.clone(),
        stem: number.stem.clone(),
        row: number.row,
        source,
        bytes,
        messages: gw.msgs,
        local_vars: gw.local_count,
        procedures: gw.asm.procedure_names(),
        editor_id,
        counts: gw.counts,
        diagnostics: gw.diags,
    })
}

/// A `scripts.lst` comment: printable ASCII without `#`, at most 60 bytes.
fn comment(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '#' {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(60);
    if out.is_empty() {
        out.push('-');
    }
    out
}
