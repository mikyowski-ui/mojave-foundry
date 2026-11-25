//! Parses every script record and every embedded result script of a MIR
//! database and prints what the front end found: scripts parsed,
//! statements, parse errors by category, unresolved names by category, and
//! function calls by signature-table status with the most used unknown
//! names. Reads only; writes nothing.
//!
//! ```text
//! cargo run -p transpile --example parse_all -- <mir dir or records.sqlite> [top]
//! ```
//!
//! With `SHOW=<category>` set it also prints every diagnostic of that
//! category with its source line, to the terminal only.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::ExitCode;

use transpile::diag::Severity;
use transpile::ir::{FormKey, PackageEvent, ResultOwner, ScriptKind, SimId, SimKind, script_sub};
use transpile::obscript::functions::{self, Status};
use transpile::obscript::{self, Compiled, ScriptInput};
use transpile::source::{self, EmbeddedScript};
use transpile::texts::TextTable;

#[derive(Default)]
struct Stats {
    records: usize,
    embedded: usize,
    skipped: usize,
    statements: usize,
    blocks: usize,
    unsupported_blocks: usize,
    panics: usize,
    with_errors: usize,
    parse: BTreeMap<String, [usize; 3]>,
    names: BTreeMap<String, [usize; 3]>,
    unresolved: BTreeMap<String, usize>,
    calls: BTreeMap<&'static str, usize>,
    distinct: BTreeMap<&'static str, BTreeMap<String, usize>>,
    block_types: BTreeMap<String, usize>,
}

impl Stats {
    fn add(&mut self, compiled: &Compiled, source: &str) {
        if let Ok(show) = std::env::var("SHOW") {
            let lines: Vec<&str> = source.lines().collect();
            for d in &compiled.script.diagnostics {
                if d.category.name() == show {
                    let line =
                        d.at.and_then(|a| lines.get((a.line as usize).saturating_sub(1)))
                            .copied()
                            .unwrap_or("");
                    println!(
                        "SHOW {}: {} | {}",
                        compiled.script.editor_id,
                        d.message,
                        line.trim()
                    );
                }
            }
        }
        self.statements += compiled.ast.statement_count();
        self.blocks += compiled.ast.blocks.len();
        for block in &compiled.ast.blocks {
            *self.block_types.entry(block.name.key()).or_default() += 1;
        }
        let mut errors = false;
        for d in &compiled.script.diagnostics {
            let group = d.category.group();
            if d.severity == Severity::Error {
                errors = true;
            }
            match group {
                "lex" | "parse" => {
                    self.parse.entry(d.category.name()).or_default()[severity(d.severity)] += 1
                }
                "name" => {
                    self.names.entry(d.category.name()).or_default()[severity(d.severity)] += 1;
                    *self
                        .unresolved
                        .entry(format!(
                            "{} {}",
                            d.category.name(),
                            d.subject.to_ascii_lowercase()
                        ))
                        .or_default() += 1;
                }
                "block" if d.category.name() == "unsupported_block" => {
                    self.unsupported_blocks += 1;
                }
                _ => {}
            }
        }
        if errors {
            self.with_errors += 1;
        }
        for call in compiled.ast.calls() {
            let status = match functions::lookup(&call.name.text) {
                Some(sig) => match sig.status {
                    Status::Compiled => "compiled",
                    Status::Folded => "folded",
                    Status::Dropped => "dropped",
                },
                None => "unknown",
            };
            *self.calls.entry(status).or_default() += 1;
            let name = functions::lookup(&call.name.text).map_or_else(
                || call.name.text.to_ascii_lowercase(),
                |s| s.name.to_owned(),
            );
            *self
                .distinct
                .entry(status)
                .or_default()
                .entry(name)
                .or_default() += 1;
        }
    }
}

fn severity(s: Severity) -> usize {
    match s {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Note => 2,
    }
}

fn top(map: &BTreeMap<String, usize>, n: usize) -> Vec<(&String, &usize)> {
    let mut list: Vec<_> = map.iter().collect();
    list.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    list.truncate(n);
    list
}

fn embedded_input<'a>(
    e: &'a EmbeddedScript,
    names: &obscript::NameTable,
    speakers: &BTreeMap<FormKey, FormKey>,
) -> Option<ScriptInput<'a>> {
    let owner = &e.owner;
    let context = e.context.as_str();
    let (id, kind, quest) = if let Some(rest) = context.strip_prefix("stage ") {
        let mut parts = rest.split(" entry ");
        let stage: i16 = parts.next()?.parse().ok()?;
        let entry: u32 = parts.next()?.parse().ok()?;
        (
            SimId::stage_script(owner, stage, entry),
            ScriptKind::Result(ResultOwner::Stage(SimId::stage(owner, stage))),
            Some(owner.clone()),
        )
    } else if let Some(seq) = context.strip_prefix("item ") {
        let seq: u32 = seq.parse().ok()?;
        (
            SimId::result_script(owner, script_sub::MENU_ITEM + seq),
            ScriptKind::Result(ResultOwner::MenuItem(SimId::new(
                SimKind::MenuItem,
                owner.clone(),
                seq,
            ))),
            None,
        )
    } else {
        let info = SimId::of(SimKind::Info, owner);
        let package = SimId::of(SimKind::Package, owner);
        match context {
            "result begin" => (
                SimId::result_script(owner, script_sub::INFO_BEGIN),
                ScriptKind::Result(ResultOwner::InfoBegin(info)),
                None,
            ),
            "result end" => (
                SimId::result_script(owner, script_sub::INFO_END),
                ScriptKind::Result(ResultOwner::InfoEnd(info)),
                None,
            ),
            "on begin" => (
                SimId::result_script(owner, script_sub::PACKAGE_BEGIN),
                ScriptKind::Result(ResultOwner::PackageEvent(package, PackageEvent::Begin)),
                None,
            ),
            "on end" => (
                SimId::result_script(owner, script_sub::PACKAGE_END),
                ScriptKind::Result(ResultOwner::PackageEvent(package, PackageEvent::End)),
                None,
            ),
            "on change" => (
                SimId::result_script(owner, script_sub::PACKAGE_CHANGE),
                ScriptKind::Result(ResultOwner::PackageEvent(package, PackageEvent::Change)),
                None,
            ),
            _ => return None,
        }
    };
    let implicit = if e.owner_sig == "INFO" {
        speakers.get(owner).and_then(|base| {
            let script = names.base_script(base)?;
            let vars = names.script_vars(script)?.to_vec();
            Some((SimId::of(SimKind::Script, script), vars))
        })
    } else {
        None
    };
    Some(ScriptInput {
        id,
        editor_id: owner.to_string(),
        kind,
        source: &e.body.source,
        declared: &e.body.vars,
        references: &e.body.references,
        quest,
        implicit,
    })
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next().map(PathBuf::from) else {
        eprintln!("usage: parse_all <mir dir or records.sqlite> [top]");
        return ExitCode::from(2);
    };
    let n: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(25);
    let file = if path.is_dir() {
        path.join("records.sqlite")
    } else {
        path
    };
    let records = match mir::Records::open(&file) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("open: {e}");
            return ExitCode::FAILURE;
        }
    };
    let corpus = match source::load_corpus(&records) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("read: {e}");
            return ExitCode::FAILURE;
        }
    };
    let names = corpus.source.name_table();
    let quest_of: BTreeMap<&FormKey, &FormKey> = corpus
        .source
        .quests
        .iter()
        .filter_map(|(q, quest)| quest.script.as_ref().map(|s| (s, q)))
        .collect();
    let mut texts = TextTable::new();
    let mut stats = Stats::default();

    for (form, script) in &corpus.source.scripts {
        let kind = match script.script_type {
            1 => ScriptKind::Quest,
            0x100 => ScriptKind::Effect,
            _ => ScriptKind::Object,
        };
        let input = ScriptInput {
            id: SimId::of(SimKind::Script, form),
            editor_id: script.editor_id.clone().unwrap_or_default(),
            kind,
            source: &script.body.source,
            declared: &script.body.vars,
            references: &script.body.references,
            quest: quest_of.get(form).map(|q| (*q).clone()),
            implicit: None,
        };
        match catch_unwind(AssertUnwindSafe(|| {
            obscript::compile(&input, &names, &mut texts)
        })) {
            Ok(compiled) => stats.add(&compiled, input.source),
            Err(_) => stats.panics += 1,
        }
        stats.records += 1;
    }
    for e in &corpus.embedded {
        let Some(input) = embedded_input(e, &names, &corpus.info_speakers) else {
            stats.skipped += 1;
            continue;
        };
        match catch_unwind(AssertUnwindSafe(|| {
            obscript::compile(&input, &names, &mut texts)
        })) {
            Ok(compiled) => stats.add(&compiled, input.source),
            Err(_) => stats.panics += 1,
        }
        stats.embedded += 1;
    }

    println!("database: {}", file.display());
    println!("game: {}", corpus.source.game);
    println!(
        "scripts parsed: {} ({} records, {} embedded; {} embedded of an unknown context skipped)",
        stats.records + stats.embedded,
        stats.records,
        stats.embedded,
        stats.skipped
    );
    println!("panics: {}", stats.panics);
    println!("statements: {}", stats.statements);
    println!(
        "blocks: {} ({} of a type Pass 2 does not run)",
        stats.blocks, stats.unsupported_blocks
    );
    println!("scripts with an error diagnostic: {}", stats.with_errors);
    println!("lexer and parser findings by category (errors, warnings, notes):");
    for (category, [e, w, n]) in &stats.parse {
        println!("  {category}: {e}, {w}, {n}");
    }
    println!("name findings by category (errors, warnings, notes):");
    for (category, [e, w, n]) in &stats.names {
        println!("  {category}: {e}, {w}, {n}");
    }
    println!("top unresolved names:");
    for (name, count) in top(&stats.unresolved, n) {
        println!("  {count:6} {name}");
    }
    println!("function calls by status (calls, distinct names):");
    for status in ["compiled", "folded", "dropped", "unknown"] {
        let calls = stats.calls.get(status).copied().unwrap_or(0);
        let distinct = stats.distinct.get(status).map_or(0, BTreeMap::len);
        println!("  {status}: {calls} calls, {distinct} names");
    }
    let empty = BTreeMap::new();
    println!("top unknown functions:");
    for (name, count) in top(stats.distinct.get("unknown").unwrap_or(&empty), n) {
        println!("  {count:6} {name}");
    }
    println!("top dropped functions:");
    for (name, count) in top(stats.distinct.get("dropped").unwrap_or(&empty), n.min(15)) {
        println!("  {count:6} {name}");
    }
    println!("block types:");
    for (name, count) in top(&stats.block_types, 40) {
        println!("  {count:6} {name}");
    }
    let (compiled, folded, dropped) =
        functions::status_counts()
            .iter()
            .fold((0, 0, 0), |acc, (s, n)| match s {
                Status::Compiled => (*n, acc.1, acc.2),
                Status::Folded => (acc.0, *n, acc.2),
                Status::Dropped => (acc.0, acc.1, *n),
            });
    println!("signature table: {compiled} compiled, {folded} folded, {dropped} dropped");
    if stats.panics > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
