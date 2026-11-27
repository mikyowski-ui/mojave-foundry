//! Checks a directory of `.int` programs the way the engine checks a mod
//! layer's scripts: each must load under the Fallout 2 word set
//! (`vault_vm::IntProgram::parse_in`) and pass the SPEC-017 section 8 stack
//! model (`vault_vm::stack_check`) with no finding. Prints one line per
//! program and exits non-zero when any fails.
//!
//! ```text
//! cargo run -p transpile --example check_ints -- <dir>
//! ```
//!
//! `vault-vm` is a dev-dependency of `transpile`; examples may use it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vault_vm::stack_check::{self, Table};
use vault_vm::{IntProgram, WordSet};

/// The `.int` files under `dir`, recursively, sorted.
fn programs(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            programs(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("int"))
        {
            out.push(path);
        }
    }
    Ok(())
}

/// `Ok` with the procedure count, or why the program fails.
fn check(path: &Path) -> Result<usize, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let program = IntProgram::parse_in(&bytes, &WordSet::FALLOUT2, false)
        .map_err(|e| format!("refused by the loader: {e}"))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let findings = stack_check::check(&name, &program, &Table::new(WordSet::FALLOUT2), &[]);
    if findings.is_empty() {
        Ok(program.procedures().len())
    } else {
        Err(format!("stack model: {}", stack_check::summary(&findings)))
    }
}

fn main() -> ExitCode {
    let Some(dir) = std::env::args_os().nth(1) else {
        eprintln!("usage: check_ints <dir>");
        return ExitCode::from(2);
    };
    let mut paths = Vec::new();
    if let Err(error) = programs(Path::new(&dir), &mut paths) {
        eprintln!("reading {}: {error}", Path::new(&dir).display());
        return ExitCode::from(2);
    }
    let mut failed = 0;
    println!("{:<40} {:<6} detail", "program", "result");
    for path in &paths {
        let shown = path
            .strip_prefix(&dir)
            .unwrap_or(path)
            .display()
            .to_string();
        match check(path) {
            Ok(procedures) => println!("{shown:<40} {:<6} {procedures} procedures", "pass"),
            Err(why) => {
                failed += 1;
                println!("{shown:<40} {:<6} {why}", "FAIL");
            }
        }
    }
    println!(
        "{} programs, {} pass, {failed} fail",
        paths.len(),
        paths.len() - failed
    );
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
