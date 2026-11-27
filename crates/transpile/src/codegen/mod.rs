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
    check_locals(&number.stem, &number.key, gw.local_count, &gw.asm)?;
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

/// The instance-local slice a program owns in the engine: its row's
/// `local_vars`, except for a map script and a manifest global script,
/// which own none (the engine runs both as system scripts, whose local
/// reads give -1 and writes are refused, and a global script has no map
/// record).
pub(crate) fn owned_locals(key: &ProgramKey, declared: u32) -> u32 {
    match key {
        ProgramKey::Map(_) | ProgramKey::Dispatcher => 0,
        _ => declared,
    }
}

/// Refuses a program that reads or writes an instance local outside the
/// slice it owns ([`owned_locals`]), or one whose index is not a constant:
/// the engine would read 0 and drop the write, with a world diagnostic
/// ("LVAR read N outside the script's slice").
pub(crate) fn check_locals(
    stem: &str,
    key: &ProgramKey,
    declared: u32,
    asm: &asm::ProgramAsm,
) -> Result<(), String> {
    let owned = owned_locals(key, declared);
    let Some((procedure, used)) = asm.local_offender(owned) else {
        return Ok(());
    };
    if used.dynamic {
        return Err(format!(
            "{stem}: procedure {procedure} uses an instance local whose index is not a constant"
        ));
    }
    let outside: Vec<String> = used
        .indices
        .iter()
        .filter(|&&i| i < 0 || i as u32 >= owned)
        .map(|i| i.to_string())
        .collect();
    Err(format!(
        "{stem}: procedure {procedure} uses instance local {} outside the {owned} the {} program owns",
        outside.join(", "),
        key.kind()
    ))
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

#[cfg(test)]
mod tests {
    use super::asm::{Proc, ProgramAsm};
    use super::words as hw;
    use super::*;

    fn program_with(body: impl FnOnce(&mut Proc)) -> ProgramAsm {
        let mut asm = ProgramAsm::default();
        let mut p = Proc::new("start");
        body(&mut p);
        asm.add(p);
        asm
    }

    fn read(p: &mut Proc, index: i32) {
        p.push_i(index);
        p.host(hw::LOCAL_VAR);
        p.pop();
    }

    /// `set_local_var(index, value)` with a value that reads another local.
    fn write(p: &mut Proc, index: i32, from: i32) {
        p.push_i(index);
        p.push_i(from);
        p.host(hw::LOCAL_VAR);
        p.push_i(1);
        p.binary("add");
        p.host(hw::SET_LOCAL_VAR);
    }

    #[test]
    fn the_locals_a_program_uses_are_its_constant_indices() {
        let asm = program_with(|p| {
            read(p, 2);
            write(p, 4, 0);
        });
        assert_eq!(
            asm.locals().indices.iter().copied().collect::<Vec<_>>(),
            [0, 2, 4]
        );
        assert!(!asm.locals().dynamic);
        assert_eq!(asm.locals().needed(), 5);
        assert!(
            check_locals("x", &ProgramKey::Dispatcher, 5, &asm).is_err(),
            "a global script owns none"
        );
    }

    #[test]
    fn a_local_outside_the_slice_is_refused_with_its_procedure() {
        let base = crate::ir::FormKey::new("synthetic.esm", 0x10);
        let key = ProgramKey::Critter(base);
        let asm = program_with(|p| write(p, 1, 0));
        assert!(check_locals("mc000001", &key, 2, &asm).is_ok());
        let error = check_locals("mc000001", &key, 1, &asm).unwrap_err();
        assert!(
            error.contains("procedure start") && error.contains("local 1"),
            "{error}"
        );
        assert!(check_locals("mm000001", &ProgramKey::Map("m".into()), 13, &asm).is_err());
        assert!(check_locals("mfquest", &ProgramKey::Dispatcher, 0, &program_with(|_| {})).is_ok());
    }

    #[test]
    fn an_index_that_is_not_a_constant_is_refused() {
        let asm = program_with(|p| {
            p.host(hw::GAME_TIME);
            p.host(hw::LOCAL_VAR);
            p.pop();
        });
        assert!(asm.locals().dynamic);
        let key = ProgramKey::Note(crate::ir::SimId::of(
            crate::ir::SimKind::Note,
            &crate::ir::FormKey::new("synthetic.esm", 0x11),
        ));
        let error = check_locals("mn000001", &key, 13, &asm).unwrap_err();
        assert!(error.contains("not a constant"), "{error}");
    }

    #[test]
    fn paths_that_join_keep_only_the_constants_they_agree_on() {
        // `set_local_var(4, cond ? 1 : 2)`: the index under the branch is
        // the same on both paths.
        let asm = program_with(|p| {
            p.push_i(4);
            let other = p.label();
            let end = p.label();
            p.branch_false(&other, |p| p.host(hw::GAME_TIME));
            p.push_i(1);
            p.jump(&end);
            p.place(&other);
            p.push_i(2);
            p.place(&end);
            p.host(hw::SET_LOCAL_VAR);
        });
        assert_eq!(
            asm.locals().indices.iter().copied().collect::<Vec<_>>(),
            [4]
        );
        assert!(!asm.locals().dynamic);
        // An index that differs by path is not a constant.
        let asm = program_with(|p| {
            let other = p.label();
            let end = p.label();
            p.branch_false(&other, |p| p.host(hw::GAME_TIME));
            p.push_i(1);
            p.jump(&end);
            p.place(&other);
            p.push_i(2);
            p.place(&end);
            p.host(hw::LOCAL_VAR);
            p.pop();
        });
        assert!(asm.locals().dynamic);
    }
}
