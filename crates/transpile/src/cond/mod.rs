//! Condition lowering (spec 3.3, 4.8, 4.9): a `CTDA` list to one
//! [`Cond`].
//!
//! - **OR runs.** A condition with the OR flag is OR-ed with the next one,
//!   and the runs are AND-ed: `[A or, B, C or, D]` is `(A ∨ B) ∧ (C ∨ D)`.
//!   A trailing OR flag closes its run.
//! - **Terms.** Each function lowers by its row of [`functions::TABLE`]:
//!   a runtime call with the condition's run-on as the call's `on`
//!   (0 subject, 1 target, 2 the condition's reference, 3 combat target,
//!   4 linked reference); a variable read; an actor-value read; or a
//!   fallback.
//! - **Form lists.** `IsInList` (index 372) becomes the disjunction of
//!   `GetIsID` terms over the list's members (nested lists flattened), or
//!   its negation, from the comparison; a list the plugin list does not
//!   hold is a fallback of 0.
//! - **Fallbacks** are never silently true (4.9): the function takes its
//!   fallback value (0 for a function with no row) and the comparison is
//!   evaluated with it. See [`Cond`] for how the result is written.
//! - **Constant folding.** Literal comparisons and fallbacks are decided
//!   here; [`fold`] decides the speaker and map terms where the facts are
//!   known (4.2), and [`simplify`] cleans the tree.

pub mod fold;
pub mod functions;

use std::collections::BTreeSet;

use crate::diag::{Category, Diag};
use crate::ir::{
    ActorValue, Call, CmpOp, Cond, Expr, FormKey, Func, FuncId, RefExpr, SimId, SimKind, VarRef,
    VarTy,
};
use crate::obscript::functions::{self as sigs, Arg};
use crate::obscript::names::NameTable;
use crate::source::{CondValue, SourceCondition};

pub use fold::{Facts, SubjectFacts, fold, simplify};
pub use functions::{CondFunction, Lowering};

/// What the conditions of one owner name, for the build's selection, and
/// what lowering found.
#[derive(Debug, Clone, Default)]
pub struct CondUses {
    /// Globals compared against or read.
    pub globals: BTreeSet<FormKey>,
    /// Placed references run on or named by a reference parameter.
    pub refs: BTreeSet<FormKey>,
    /// Quests named by a parameter.
    pub quests: BTreeSet<FormKey>,
    /// Diagnostics, owned by the owner.
    pub diags: Vec<Diag>,
}

/// Lowers a condition list. `owner` names it in diagnostics; `names`
/// gives quest and script variable types.
pub fn lower(
    conditions: &[SourceCondition],
    owner: &SimId,
    names: &NameTable,
    uses: &mut CondUses,
) -> Cond {
    let mut runs: Vec<Vec<Cond>> = Vec::new();
    let mut current: Vec<Cond> = Vec::new();
    for condition in conditions {
        current.push(term(condition, owner, names, uses));
        if !condition.is_or {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    let conjuncts: Vec<Cond> = runs
        .into_iter()
        .map(|mut run| {
            if run.len() == 1 {
                run.remove(0)
            } else {
                Cond::Any(run)
            }
        })
        .collect();
    let cond = match conjuncts.len() {
        0 => Cond::True,
        1 => conjuncts.into_iter().next().unwrap_or(Cond::True),
        _ => Cond::All(conjuncts),
    };
    simplify(&cond)
}

/// A literal as an expression: an integer when it is one.
fn number(value: f64) -> Expr {
    if value.fract() == 0.0 && value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX) {
        Expr::Int(value as i32)
    } else {
        Expr::Float(value as f32)
    }
}

fn fallback_term(
    condition: &SourceCondition,
    op: CmpOp,
    rhs: &Expr,
    value: f32,
    report: bool,
    owner: &SimId,
    uses: &mut CondUses,
) -> Cond {
    let name = functions::name_of(condition.function);
    let severity_diag = if report { Diag::warning } else { Diag::note };
    uses.diags.push(
        severity_diag(
            Category::ConditionFallback,
            format!("{name} taken as {value}"),
        )
        .owned_by(owner)
        .about(name.clone()),
    );
    let leaf = Cond::Fallback {
        function: condition.function,
        name,
        value,
        report,
    };
    match literal(rhs) {
        Some(rhs) if op.holds(f64::from(value), rhs) => leaf,
        Some(_) => Cond::Not(Box::new(leaf)),
        None => Cond::Cmp {
            lhs: Expr::Float(value),
            op,
            rhs: rhs.clone(),
        },
    }
}

fn literal(expr: &Expr) -> Option<f64> {
    match expr {
        Expr::Int(v) => Some(f64::from(*v)),
        Expr::Float(v) => Some(f64::from(*v)),
        _ => None,
    }
}

fn param(condition: &SourceCondition, i: usize) -> (u32, Option<&FormKey>) {
    match i {
        0 => (condition.param1, condition.param1_form.as_ref()),
        _ => (condition.param2, condition.param2_form.as_ref()),
    }
}

/// One term.
fn term(
    condition: &SourceCondition,
    owner: &SimId,
    names: &NameTable,
    uses: &mut CondUses,
) -> Cond {
    let Some(op) = CmpOp::parse(&condition.comparison) else {
        uses.diags.push(
            Diag::error(
                Category::ConditionComparison,
                format!(
                    "comparison code {} of {}; the term is false",
                    condition.comparison,
                    functions::name_of(condition.function)
                ),
            )
            .owned_by(owner)
            .about(functions::name_of(condition.function)),
        );
        return Cond::False;
    };
    let rhs = match &condition.value {
        CondValue::Literal(v) => number(*v),
        CondValue::Global(global) => {
            uses.globals.insert(global.clone());
            Expr::Var(VarRef::Global(SimId::of(SimKind::Global, global)))
        }
    };
    let Some(row) = functions::by_index(condition.function) else {
        uses.diags.push(
            Diag::warning(
                Category::ConditionUnknownFunction,
                format!(
                    "condition function {} has no table entry",
                    condition.function
                ),
            )
            .owned_by(owner)
            .about(functions::name_of(condition.function)),
        );
        return fallback_term(condition, op, &rhs, 0.0, true, owner, uses);
    };
    if let Lowering::Fallback { value, report } = row.lowering {
        return fallback_term(condition, op, &rhs, value, report, owner, uses);
    }

    // What it runs on.
    let on = match condition.run_on {
        0 => RefExpr::SelfRef,
        1 => RefExpr::Player,
        2 => match &condition.reference {
            Some(r) => {
                uses.refs.insert(r.clone());
                RefExpr::Placed(r.clone())
            }
            None => {
                uses.diags.push(
                    Diag::warning(
                        Category::ConditionBadParameter,
                        format!(
                            "{} runs on a reference that is not in the plugin list",
                            row.name
                        ),
                    )
                    .owned_by(owner)
                    .about(row.name),
                );
                return fallback_term(condition, op, &rhs, 0.0, true, owner, uses);
            }
        },
        4 => RefExpr::Linked(Box::new(RefExpr::SelfRef)),
        other => {
            uses.diags.push(
                Diag::warning(
                    Category::ConditionRunOn,
                    format!(
                        "{} runs on target {other} (combat target), which Pass 2 cannot name",
                        row.name
                    ),
                )
                .owned_by(owner)
                .about(row.name),
            );
            return fallback_term(condition, op, &rhs, 0.0, true, owner, uses);
        }
    };

    let lhs = match row.lowering {
        Lowering::Variable => match variable(condition, row, on.clone(), names, uses) {
            Some(lhs) => lhs,
            None => {
                uses.diags.push(
                    Diag::warning(
                        Category::ConditionBadParameter,
                        format!("{} names no usable variable", row.name),
                    )
                    .owned_by(owner)
                    .about(row.name),
                );
                return fallback_term(condition, op, &rhs, 0.0, true, owner, uses);
            }
        },
        Lowering::ActorValue => {
            let av = ActorValue(condition.param1);
            if (25..=30).contains(&av.0) {
                return fallback_term(condition, op, &rhs, 100.0, false, owner, uses);
            }
            if av.stat().is_none() && av.0 != 16 && av.0 != 23 {
                return fallback_term(condition, op, &rhs, 0.0, true, owner, uses);
            }
            runtime_call(row, on, vec![Expr::Int(av.0 as i32)])
        }
        Lowering::Runtime | Lowering::Static => {
            let Some(sig) = sigs::lookup(row.name) else {
                return fallback_term(condition, op, &rhs, 0.0, true, owner, uses);
            };
            let mut args = Vec::new();
            for (i, kind) in sig.args.iter().enumerate() {
                let (raw, form) = param(condition, i);
                let arg = match (kind, form) {
                    (Arg::Ref, Some(form)) => {
                        uses.refs.insert(form.clone());
                        Expr::Ref(RefExpr::Placed(form.clone()))
                    }
                    (Arg::Quest, Some(form)) => {
                        uses.quests.insert(form.clone());
                        Expr::Form(form.clone())
                    }
                    (Arg::Form | Arg::Global, Some(form)) => Expr::Form(form.clone()),
                    (Arg::Ref | Arg::Quest | Arg::Form | Arg::Global, None) => {
                        uses.diags.push(
                            Diag::warning(
                                Category::ConditionBadParameter,
                                format!(
                                    "{} parameter {} ({raw:#x}) names no form of the plugin list",
                                    row.name,
                                    i + 1
                                ),
                            )
                            .owned_by(owner)
                            .about(row.name),
                        );
                        return fallback_term(condition, op, &rhs, 0.0, true, owner, uses);
                    }
                    _ => Expr::Int(raw as i32),
                };
                args.push(arg);
            }
            runtime_call(row, on, args)
        }
        Lowering::FormList => return in_list(condition, op, &rhs, on, names, owner, uses),
        Lowering::Fallback { .. } => unreachable!("handled above"),
    };
    let cond = Cond::Cmp { lhs, op, rhs };
    simplify(&cond)
}

/// The most members an `IsInList` term expands to; a longer list (after
/// flattening) is a reported fallback of 0.
pub const MAX_LIST_MEMBERS: usize = 512;

/// The members of a form list, nested lists flattened (each list once),
/// in order, without repeats; `None` when `list` is not a known list.
fn list_members(list: &FormKey, names: &NameTable) -> Option<Vec<FormKey>> {
    let mut out: Vec<FormKey> = Vec::new();
    let mut seen: BTreeSet<FormKey> = BTreeSet::new();
    let mut pending: Vec<FormKey> = vec![list.clone()];
    names.form_list(list)?;
    while let Some(current) = pending.pop() {
        if !seen.insert(current.clone()) {
            continue;
        }
        let Some(members) = names.form_list(&current) else {
            if !out.contains(&current) {
                out.push(current);
            }
            continue;
        };
        pending.extend(members.iter().rev().cloned());
    }
    Some(out)
}

/// `IsInList list op value`: the function is 1 when the run-on's base is
/// a member and 0 otherwise, so the comparison decides which of the two
/// passes; the member test is one `GetIsID == 1` per member.
fn in_list(
    condition: &SourceCondition,
    op: CmpOp,
    rhs: &Expr,
    on: RefExpr,
    names: &NameTable,
    owner: &SimId,
    uses: &mut CondUses,
) -> Cond {
    let members = condition
        .param1_form
        .as_ref()
        .and_then(|list| list_members(list, names));
    let (Some(members), Some(value)) = (members, literal(rhs)) else {
        uses.diags.push(
            Diag::warning(
                Category::ConditionBadParameter,
                "IsInList names no form list of the plugin list, or compares with a global",
            )
            .owned_by(owner)
            .about("IsInList"),
        );
        return fallback_term(condition, op, rhs, 0.0, true, owner, uses);
    };
    if members.len() > MAX_LIST_MEMBERS {
        return fallback_term(condition, op, rhs, 0.0, true, owner, uses);
    }
    let Some(is_id) = functions::by_index(72) else {
        unreachable!("GetIsID is in the table")
    };
    let member = Cond::Any(
        members
            .into_iter()
            .map(|m| Cond::Cmp {
                lhs: runtime_call(is_id, on.clone(), vec![Expr::Form(m)]),
                op: CmpOp::Eq,
                rhs: Expr::Int(1),
            })
            .collect(),
    );
    let cond = match (op.holds(1.0, value), op.holds(0.0, value)) {
        (true, true) => Cond::True,
        (false, false) => Cond::False,
        (true, false) => member,
        (false, true) => Cond::Not(Box::new(member)),
    };
    simplify(&cond)
}

fn runtime_call(row: &CondFunction, on: RefExpr, args: Vec<Expr>) -> Expr {
    let func = match FuncId::named(row.name) {
        Some(id) => Func::Known(id),
        None => Func::Unsupported {
            name: row.name.to_owned(),
        },
    };
    Expr::Call(Box::new(Call {
        func,
        on: Some(on),
        args,
    }))
}

/// `GetGlobalValue`, `GetQuestVariable`, `GetScriptVariable`.
fn variable(
    condition: &SourceCondition,
    row: &CondFunction,
    on: RefExpr,
    names: &NameTable,
    uses: &mut CondUses,
) -> Option<Expr> {
    match row.index {
        74 => {
            let global = condition.param1_form.as_ref()?;
            uses.globals.insert(global.clone());
            Some(Expr::Var(VarRef::Global(SimId::of(
                SimKind::Global,
                global,
            ))))
        }
        79 => {
            let quest = condition.param1_form.as_ref()?;
            uses.quests.insert(quest.clone());
            let ty = names
                .quest_vars(quest)
                .and_then(|vars| vars.iter().find(|v| v.index == condition.param2))
                .map_or(VarTy::Int, |v| v.ty);
            Some(Expr::Var(VarRef::Quest {
                quest: SimId::of(SimKind::Quest, quest),
                index: condition.param2,
                ty,
            }))
        }
        53 => {
            // The reference whose script holds the variable is the first
            // parameter; without one the call stays for the dialogue
            // compiler, which knows the subject's script.
            if let Some(owner) = condition.param1_form.as_ref()
                && let Some(script) = names.ref_script(owner)
            {
                uses.refs.insert(owner.clone());
                let ty = names
                    .script_vars(script)
                    .and_then(|vars| vars.iter().find(|v| v.index == condition.param2))
                    .map_or(VarTy::Int, |v| v.ty);
                return Some(Expr::Var(VarRef::Hoisted {
                    owner: owner.clone(),
                    script: SimId::of(SimKind::Script, script),
                    index: condition.param2,
                    ty,
                }));
            }
            Some(runtime_call(
                row,
                on,
                vec![
                    Expr::Int(condition.param1 as i32),
                    Expr::Int(condition.param2 as i32),
                ],
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
