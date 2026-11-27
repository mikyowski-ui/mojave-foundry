//! The lowering (spec 10, task T8): a [`Module`] to the files of the layer,
//! the map attachments, the manifest rows and the report's counts.
//!
//! [`lower`] is the contract's entry point (files and diagnostics only);
//! [`lower_layer`] takes the facts the module does not carry
//! ([`LowerFacts`]: placement index, PIDs, the scripts of non-actor bases,
//! locks, game settings) and returns the plan as well.
//!
//! Pure and deterministic: no clock, no path, no hash-map order reaches
//! the output; programs are generated one by one in row order.

use std::collections::BTreeMap;

use emit::map::{ObjectScript, ScriptAttachment, ScriptSite};

use crate::codegen::{self, Ctx, Program};
use crate::diag::{Category, Diag};
use crate::error::TranspileError;
use crate::ir::{BaseFiles, Lowered, Module, PlacedKind, SimConfig};
use crate::lowered::{
    Counts, GlobalScriptRow, HookRow, LowerFacts, LowerOutput, LoweredPlan, MapPlan, Outcome,
    ProgramPlan, SectionCounts, Summary,
};
use crate::numbering::{self, ProgramKey, object_key};
use crate::tables;

/// The dispatcher's repeat, in steps (spec 5.11).
pub const DISPATCHER_REPEAT: u32 = 6;

/// Lowers a module with no facts beyond it: the programs and tables it
/// alone determines. Nothing is attached to a map (no placement index).
///
/// # Errors
///
/// When a base file does not parse, or a table cannot hold what the
/// module asks for (an id the base already holds).
pub fn lower(
    module: &Module,
    base: &BaseFiles,
    config: &SimConfig,
) -> Result<Lowered, TranspileError> {
    lower_layer(module, &LowerFacts::default(), base, config).map(|out| out.lowered)
}

fn failed(message: String) -> TranspileError {
    TranspileError::Slice(format!("lowering: {message}"))
}

fn merge(into: &mut SectionCounts, from: &SectionCounts) {
    into.compiled += from.compiled;
    into.folded += from.folded;
    into.dropped += from.dropped;
    into.fallback += from.fallback;
    for (reason, r) in &from.reasons {
        let entry = into.reasons.entry(reason.clone()).or_default();
        entry.count += r.count;
        for owner in &r.owners {
            if entry.owners.len() < crate::lowered::Reason::OWNERS && !entry.owners.contains(owner)
            {
                entry.owners.push(owner.clone());
            }
        }
    }
}

fn merge_counts(into: &mut Counts, from: &Counts) {
    merge(&mut into.dialogue, &from.dialogue);
    merge(&mut into.scripts, &from.scripts);
    merge(&mut into.quests, &from.quests);
    merge(&mut into.state, &from.state);
    merge(&mut into.terminals, &from.terminals);
    merge(&mut into.packages, &from.packages);
}

/// Lowers a module with the facts of the placement and the plugin.
///
/// # Errors
///
/// As [`lower`].
pub fn lower_layer(
    module: &Module,
    facts: &LowerFacts,
    base: &BaseFiles,
    config: &SimConfig,
) -> Result<LowerOutput, TranspileError> {
    let mut diagnostics: Vec<Diag> = Vec::new();
    if config.sfall {
        diagnostics.push(Diag::note(
            Category::IgnoredFlag,
            "sfall mode asked for; Pass 2 emits plain Fallout 2 words only",
        ));
    }
    let numbers = numbering::number(module, facts, base, &mut diagnostics).map_err(failed)?;
    let ctx = Ctx::new(module, &numbers, facts);

    let mut programs: Vec<Program> = Vec::with_capacity(numbers.programs.len());
    for number in &numbers.programs {
        programs.push(codegen::program(&ctx, number).map_err(failed)?);
    }

    let mut counts = Counts::default();
    for program in &programs {
        merge_counts(&mut counts, &program.counts);
        diagnostics.extend(program.diagnostics.iter().cloned());
    }
    count_static(module, facts, &mut counts);

    let tables = tables::build(module, &numbers, &programs, base).map_err(failed)?;
    diagnostics.extend(tables.diagnostics.iter().cloned());
    let mut files = tables.files;
    let mut sources = BTreeMap::new();
    for program in &programs {
        files.insert(
            format!("scripts/{}.int", program.stem),
            program.bytes.clone(),
        );
        sources.insert(program.stem.clone(), program.source.clone());
    }

    // Map attachments (spec 10.4).
    let mut maps: BTreeMap<String, MapPlan> = BTreeMap::new();
    for (stem, number) in &numbers.maps {
        maps.insert(
            stem.clone(),
            MapPlan {
                map: stem.clone(),
                map_number: *number,
                map_script: numbers
                    .program(&ProgramKey::Map(stem.clone()))
                    .map(|p| p.row),
                attachments: Vec::new(),
            },
        );
    }
    let mut placement = facts.placement.clone();
    placement.sort();
    let row_of =
        |key: &ProgramKey| -> Option<&Program> { numbers.by_key.get(key).map(|&i| &programs[i]) };
    for object in &placement.objects {
        let key = if object.kind == PlacedKind::Critter {
            object
                .base
                .clone()
                .map(ProgramKey::Critter)
                .filter(|k| numbers.by_key.contains_key(k))
        } else {
            object_key(module, facts, object)
        };
        let Some(key) = key else {
            continue;
        };
        let Some(program) = row_of(&key) else {
            continue;
        };
        let Some(id) = object.object_id else {
            diagnostics.push(
                Diag::warning(
                    Category::RefOutsideSlice,
                    format!(
                        "a placed object of {} on {} has no object id; no script attached",
                        key.owner(),
                        object.map
                    ),
                )
                .about(object.map.clone()),
            );
            continue;
        };
        let kind = if object.kind == PlacedKind::Critter {
            ObjectScript::Critter
        } else {
            ObjectScript::Item
        };
        if let Some(plan) = maps.get_mut(&object.map) {
            plan.attachments.push(ScriptAttachment {
                site: ScriptSite::Object { id, kind },
                script_index: program.row,
                local_vars: program.local_vars,
            });
        }
    }

    let dispatcher = numbers.program(&ProgramKey::Dispatcher);
    let global_scripts = dispatcher
        .map(|p| {
            vec![GlobalScriptRow {
                file: format!("{}.int", p.stem),
                repeat: DISPATCHER_REPEAT,
                kind: 0,
            }]
        })
        .unwrap_or_default();
    let hooks = dispatcher
        .map(|p| {
            vec![HookRow {
                point: "HOOK_GAMEMODECHANGE".to_owned(),
                file: format!("{}.int", p.stem),
                procedure: "mode_changed".to_owned(),
            }]
        })
        .unwrap_or_default();

    let summary = Summary {
        programs: programs.len() as u32,
        rows: programs.len() as u32,
        globals: numbers.globals.len() as u32,
        messages: tables.messages,
        holodisks: tables.holodisks,
        quest_rows: tables.quest_rows,
        text_changes: tables.text_changes,
    };
    let plan = LoweredPlan {
        programs: programs
            .iter()
            .map(|p| ProgramPlan {
                stem: p.stem.clone(),
                row: p.row,
                kind: p.key.kind(),
                owner: p.key.owner(),
                editor_id: p.editor_id.clone(),
                local_vars: p.local_vars,
                procedures: p.procedures.clone(),
                messages: !p.messages.is_empty(),
            })
            .collect(),
        maps: maps.into_values().collect(),
        global_scripts,
        hooks,
        counts,
        summary,
        sources,
    };
    diagnostics.sort();
    diagnostics.dedup();
    Ok(LowerOutput {
        lowered: Lowered { files, diagnostics },
        plan,
    })
}

/// Counts what no program counts: group children and notes.
fn count_static(module: &Module, facts: &LowerFacts, counts: &mut Counts) {
    for group in module.groups.values() {
        for (child, _) in &group.children {
            let placed = facts.placement.of_reference(child).next();
            let owner = child.to_string();
            match placed {
                Some(o) if o.pid.is_some() || o.kind == PlacedKind::Critter => {
                    counts.state.count(Outcome::Compiled, "", &owner);
                }
                Some(_) => counts.state.count(
                    Outcome::Dropped,
                    "group child placed with no PID in the index",
                    &owner,
                ),
                None => counts.state.count(
                    Outcome::Dropped,
                    "group child not placed (the map stage leaves disabled children out until T7)",
                    &owner,
                ),
            }
        }
    }
    for note in module.notes.values() {
        counts
            .terminals
            .count(Outcome::Compiled, "", &note.id.form.to_string());
    }
}
