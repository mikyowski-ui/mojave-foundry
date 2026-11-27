//! Quest state and the Pip-Boy (spec 6, task T5).
//!
//! [`compile`] adds to the module:
//!
//! - **Quest-state globals** (spec 6.1) for every quest: `G_q_STAGE`
//!   ([`quest_sub::STAGE`]), `G_q_FLAGS` ([`quest_sub::FLAGS`], bits
//!   [`quest_flags`]; a start-enabled quest starts running), one
//!   `G_q_DONE_i` and one `G_q_PEND_i` per 31 stages ([`stage_bit`] says
//!   which global and bit a stage uses), and one `G_q_OBJ_n` per objective
//!   (the objective's own [`SimKind::Objective`] id, values
//!   [`objective_state`]).
//! - **Variables** (spec 5.4): one global per quest variable some compiled
//!   code or condition reads or writes ([`SimKind::QuestVar`], `sub` the
//!   index), one per hoisted object-script variable ([`SimKind::ScriptVar`]
//!   keyed by the owner reference), and the karma shadow global
//!   ([`SimId::karma`], owner decision 13) when anything rewards or reads
//!   karma.
//! - **Pip-Boy rows** (spec 6.3, owner decision 8): a [`Journal`] for every
//!   quest with a name and at least one objective with text.
//! - **Ordering reports**: a block that sets a stage and later in the same
//!   block reads what that stage's scripts change (spec 6.2:
//!   [`Category::StageOrdering`]); statements after a self-destroying word
//!   (spec 5.5: [`Category::SelfDestroyOrdering`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::build::Pass;
use crate::diag::{Category, Diag};
use crate::ir::{
    ActorValue, Call, Expr, FormKey, Func, Global, GlobalPurpose, Journal, JournalRow, Module,
    Quest, RefExpr, SimId, SimKind, Stmt, VarRef, VarTy, global_name, objective_state, quest_flags,
    quest_sub, visit,
};

/// Stage bits per global (spec 6.1).
pub const BITS_PER_GLOBAL: u32 = 31;

/// The `(global, bit)` of a stage's done and pending bits: the stage's
/// position among the quest's stages, sorted by index; the done global is
/// `quest_state(q, DONE + global)`, the pending one `PEND + global`.
pub fn stage_bit(quest: &Quest, index: i16) -> Option<(u32, u32)> {
    let position = quest.stages.iter().position(|s| s.index == index)?;
    let position = u32::try_from(position).ok()?;
    Some((position / BITS_PER_GLOBAL, position % BITS_PER_GLOBAL))
}

/// How many done (and pending) globals a quest needs.
pub fn stage_globals(quest: &Quest) -> u32 {
    let count = u32::try_from(quest.stages.len()).unwrap_or(u32::MAX);
    count.div_ceil(BITS_PER_GLOBAL)
}

fn global(module: &mut Module, id: SimId, purpose: GlobalPurpose, name: String, initial: i32) {
    global_typed(module, id, purpose, name, VarTy::Int, initial);
}

fn global_typed(
    module: &mut Module,
    id: SimId,
    purpose: GlobalPurpose,
    name: String,
    ty: VarTy,
    initial: i32,
) {
    module.globals.entry(id.clone()).or_insert(Global {
        id,
        purpose,
        name,
        ty,
        initial,
    });
}

/// Adds quest state, variables, Pip-Boy rows and the ordering reports.
pub fn compile(pass: &mut Pass<'_>) {
    state_globals(pass.module);
    variables(pass);
    journal(pass);
    ordering(pass);
}

fn state_globals(module: &mut Module) {
    let quests: Vec<Quest> = module.quests.values().cloned().collect();
    for quest in &quests {
        let form = &quest.id.form;
        let name = global_name(if quest.editor_id.is_empty() {
            form.as_str()
        } else {
            &quest.editor_id
        });
        global(
            module,
            SimId::quest_state(form, quest_sub::STAGE),
            GlobalPurpose::QuestState,
            format!("{name}_STAGE"),
            0,
        );
        let flags = if quest.start_enabled {
            quest_flags::RUNNING | quest_flags::APPLIED
        } else {
            0
        };
        global(
            module,
            SimId::quest_state(form, quest_sub::FLAGS),
            GlobalPurpose::QuestState,
            format!("{name}_FLAGS"),
            flags,
        );
        for i in 0..stage_globals(quest) {
            global(
                module,
                SimId::quest_state(form, quest_sub::DONE + i),
                GlobalPurpose::QuestState,
                format!("{name}_DONE{i}"),
                0,
            );
            global(
                module,
                SimId::quest_state(form, quest_sub::PEND + i),
                GlobalPurpose::QuestState,
                format!("{name}_PEND{i}"),
                0,
            );
        }
        for objective in &quest.objectives {
            global(
                module,
                objective.id.clone(),
                GlobalPurpose::QuestState,
                format!("{name}_OBJ{}", objective.index),
                objective_state::HIDDEN,
            );
        }
    }
}

/// The quest and hoisted variables the module uses, and karma.
fn variables(pass: &mut Pass<'_>) {
    let mut used: BTreeSet<VarRef> = BTreeSet::new();
    let mut karma = false;
    let module = &*pass.module;
    for (_, body) in visit::module_bodies(module) {
        visit::vars_in_stmts(body, &mut |v| {
            used.insert(v.clone());
        });
        for call in visit::calls_in_stmts(body) {
            if matches!(&call.func, Func::Known(id) if id.name() == "RewardKarma") {
                karma = true;
            }
            if is_karma_read(call) {
                karma = true;
            }
        }
    }
    for cond in visit::module_conditions(module) {
        visit::exprs_in_cond(cond, &mut |e| {
            visit::vars_in_expr(e, &mut |v| {
                used.insert(v.clone());
            });
            if let Expr::Call(call) = e
                && is_karma_read(call)
            {
                karma = true;
            }
        });
    }
    for var in used {
        match var {
            VarRef::Quest { quest, index, ty } => {
                let editor = pass
                    .module
                    .quests
                    .get(&quest)
                    .map(|q| q.editor_id.clone())
                    .unwrap_or_else(|| quest.form.as_str().to_owned());
                let var_name = pass
                    .names
                    .quest_vars(&quest.form)
                    .and_then(|vars| vars.iter().find(|v| v.index == index))
                    .map_or_else(|| format!("V{index}"), |v| v.name.clone());
                global_typed(
                    pass.module,
                    SimId::new(SimKind::QuestVar, quest.form.clone(), index),
                    GlobalPurpose::QuestVar,
                    global_name(&format!("{editor}_{var_name}")),
                    ty,
                    0,
                );
            }
            VarRef::Hoisted {
                owner,
                script,
                index,
                ty,
            } => {
                let editor = pass
                    .source
                    .refs
                    .get(&owner)
                    .and_then(|r| r.editor_id.clone())
                    .unwrap_or_else(|| owner.as_str().to_owned());
                let var_name = pass
                    .names
                    .script_vars(&script.form)
                    .and_then(|vars| vars.iter().find(|v| v.index == index))
                    .map_or_else(|| format!("V{index}"), |v| v.name.clone());
                global_typed(
                    pass.module,
                    SimId::new(SimKind::ScriptVar, owner.clone(), index),
                    GlobalPurpose::Actor,
                    global_name(&format!("{editor}_{var_name}")),
                    ty,
                    0,
                );
            }
            _ => {}
        }
    }
    if karma {
        global(
            pass.module,
            SimId::karma(),
            GlobalPurpose::Actor,
            "KARMA".to_owned(),
            0,
        );
    }
}

/// A read of the karma actor value (23).
fn is_karma_read(call: &Call) -> bool {
    let Func::Known(id) = &call.func else {
        return false;
    };
    matches!(
        id.name(),
        "GetActorValue" | "GetBaseActorValue" | "GetPermanentActorValue"
    ) && matches!(call.args.first(), Some(Expr::Int(v)) if ActorValue(*v as u32) == ActorValue(23))
}

fn journal(pass: &mut Pass<'_>) {
    let quests: Vec<Quest> = pass.module.quests.values().cloned().collect();
    for quest in quests {
        let rows: Vec<JournalRow> = quest
            .objectives
            .iter()
            .filter_map(|o| {
                Some(JournalRow {
                    objective: o.id.clone(),
                    text: o.text?,
                    global: o.id.clone(),
                    display_at: objective_state::DISPLAYED,
                    completed_at: objective_state::COMPLETED,
                })
            })
            .collect();
        match (quest.name, rows.is_empty()) {
            (Some(heading), false) => {
                pass.module.journal.insert(
                    quest.id.clone(),
                    Journal {
                        quest: quest.id.clone(),
                        heading,
                        rows,
                    },
                );
            }
            _ if !quest.objectives.is_empty() => {
                pass.diag(
                    Diag::note(
                        Category::NoJournal,
                        "quest has objectives but no name or no objective text; no Pip-Boy rows",
                    )
                    .owned_by(&quest.id)
                    .about(quest.editor_id.clone()),
                );
            }
            _ => {}
        }
    }
}

/// What a stage's scripts change: variables set, objectives set, stages
/// set.
#[derive(Debug, Default)]
struct Effects {
    vars: BTreeSet<VarRef>,
    objectives: BTreeSet<(FormKey, i32)>,
    stages: BTreeSet<FormKey>,
}

fn form_arg(call: &Call, i: usize) -> Option<&FormKey> {
    match call.args.get(i) {
        Some(Expr::Form(form)) => Some(form),
        _ => None,
    }
}

fn int_arg(call: &Call, i: usize) -> Option<i32> {
    match call.args.get(i) {
        Some(Expr::Int(v)) => Some(*v),
        _ => None,
    }
}

fn known(call: &Call) -> Option<&'static str> {
    match &call.func {
        Func::Known(id) => Some(id.name()),
        Func::Unsupported { .. } => None,
    }
}

fn effects_of(stmts: &[Stmt], effects: &mut Effects) {
    for stmt in stmts {
        match stmt {
            Stmt::Set(var, _) => {
                effects.vars.insert(var.clone());
            }
            Stmt::If { arms, otherwise } => {
                for (_, body) in arms {
                    effects_of(body, effects);
                }
                effects_of(otherwise, effects);
            }
            Stmt::Call(call) => match known(call) {
                Some("SetObjectiveDisplayed" | "SetObjectiveCompleted") => {
                    if let (Some(q), Some(n)) = (form_arg(call, 0), int_arg(call, 1)) {
                        effects.objectives.insert((q.clone(), n));
                    }
                }
                Some("SetStage" | "StartQuest" | "StopQuest") => {
                    if let Some(q) = form_arg(call, 0) {
                        effects.stages.insert(q.clone());
                    }
                }
                _ => {}
            },
            Stmt::Return => {}
        }
    }
}

/// Whether a statement reads one of the effects.
fn reads(stmt: &Stmt, effects: &Effects) -> bool {
    let mut hit = false;
    let one = std::slice::from_ref(stmt);
    let mut check_var = |v: &VarRef| {
        if effects.vars.contains(v) {
            hit = true;
        }
    };
    match stmt {
        // The target of a `set` is written, not read.
        Stmt::Set(_, value) => visit::vars_in_expr(value, &mut check_var),
        _ => visit::vars_in_stmts(one, &mut check_var),
    }
    for call in visit::calls_in_stmts(one) {
        match known(call) {
            Some("GetObjectiveDisplayed" | "GetObjectiveCompleted") => {
                if let (Some(q), Some(n)) = (form_arg(call, 0), int_arg(call, 1))
                    && effects.objectives.contains(&(q.clone(), n))
                {
                    hit = true;
                }
            }
            Some("GetStage" | "GetStageDone" | "GetQuestRunning")
                if form_arg(call, 0).is_some_and(|q| effects.stages.contains(q)) =>
            {
                hit = true;
            }
            _ => {}
        }
    }
    hit
}

fn ordering(pass: &mut Pass<'_>) {
    // The effects of each stage's log-entry scripts.
    let mut stage_effects: BTreeMap<(FormKey, i32), Effects> = BTreeMap::new();
    for quest in pass.module.quests.values() {
        for stage in &quest.stages {
            let mut effects = Effects::default();
            for entry in &stage.entries {
                if let Some(script) = entry
                    .script
                    .as_ref()
                    .and_then(|s| pass.module.scripts.get(s))
                {
                    for block in &script.blocks {
                        effects_of(&block.body, &mut effects);
                    }
                }
            }
            if !effects.vars.is_empty()
                || !effects.objectives.is_empty()
                || !effects.stages.is_empty()
            {
                stage_effects.insert((quest.id.form.clone(), i32::from(stage.index)), effects);
            }
        }
    }
    let mut found: Vec<Diag> = Vec::new();
    for script in pass.module.scripts.values() {
        for block in &script.blocks {
            scan(
                &block.body,
                &script.id,
                &stage_effects,
                pass.module,
                &mut found,
            );
        }
    }
    pass.module.diagnostics.extend(found);
}

/// Scans one statement list (and the nested ones) for the two orderings.
fn scan(
    stmts: &[Stmt],
    owner: &SimId,
    stage_effects: &BTreeMap<(FormKey, i32), Effects>,
    module: &Module,
    found: &mut Vec<Diag>,
) {
    for (i, stmt) in stmts.iter().enumerate() {
        match stmt {
            Stmt::If { arms, otherwise } => {
                for (_, body) in arms {
                    scan(body, owner, stage_effects, module, found);
                }
                scan(otherwise, owner, stage_effects, module, found);
            }
            Stmt::Call(call) => {
                let rest = &stmts[i + 1..];
                match known(call) {
                    Some("SetStage") => {
                        let (Some(q), Some(s)) = (form_arg(call, 0), int_arg(call, 1)) else {
                            continue;
                        };
                        let Some(effects) = stage_effects.get(&(q.clone(), s)) else {
                            continue;
                        };
                        if rest.iter().any(|later| reads(later, effects)) {
                            let quest = module
                                .quests
                                .get(&SimId::of(SimKind::Quest, q))
                                .map_or_else(|| q.as_str().to_owned(), |q| q.editor_id.clone());
                            found.push(
                                Diag::warning(
                                    Category::StageOrdering,
                                    format!(
                                        "sets {quest} stage {s} and then reads what its stage \
                                         script changes; the stage script runs later (spec 6.2)"
                                    ),
                                )
                                .owned_by(owner)
                                .about(format!("{quest} {s}")),
                            );
                        }
                    }
                    Some("Kill")
                        if matches!(call.on, None | Some(RefExpr::SelfRef)) && !rest.is_empty() =>
                    {
                        found.push(
                            Diag::warning(
                                Category::SelfDestroyOrdering,
                                format!(
                                    "{} statement(s) after Kill on itself do not run (spec 5.5)",
                                    rest.len()
                                ),
                            )
                            .owned_by(owner)
                            .about("Kill"),
                        );
                    }
                    _ => {}
                }
            }
            Stmt::Set(..) | Stmt::Return => {}
        }
    }
}
