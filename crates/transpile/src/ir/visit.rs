//! Walking the IR: every expression, call and variable of statements and
//! conditions, in source order. The middle passes and the lowering use
//! these instead of their own recursions.

use super::expr::{Call, Cond, Expr, RefExpr, VarRef};
use super::module::Module;
use super::script::Stmt;

/// Calls `f` on every expression of `expr`, outermost first.
pub fn exprs_in_expr<'e>(expr: &'e Expr, f: &mut dyn FnMut(&'e Expr)) {
    f(expr);
    match expr {
        Expr::Call(call) => call.args.iter().for_each(|a| exprs_in_expr(a, f)),
        Expr::Unary(_, inner) => exprs_in_expr(inner, f),
        Expr::Binary(_, a, b) => {
            exprs_in_expr(a, f);
            exprs_in_expr(b, f);
        }
        _ => {}
    }
}

/// Calls `f` on every expression of a statement list, in order, nested
/// bodies included.
pub fn exprs_in_stmts<'s>(stmts: &'s [Stmt], f: &mut dyn FnMut(&'s Expr)) {
    for stmt in stmts {
        match stmt {
            Stmt::Set(_, value) => exprs_in_expr(value, f),
            Stmt::If { arms, otherwise } => {
                for (cond, body) in arms {
                    exprs_in_expr(cond, f);
                    exprs_in_stmts(body, f);
                }
                exprs_in_stmts(otherwise, f);
            }
            Stmt::Call(call) => call.args.iter().for_each(|a| exprs_in_expr(a, f)),
            Stmt::Return => {}
        }
    }
}

/// Every call of a statement list, statements and calls inside
/// expressions, in order.
pub fn calls_in_stmts(stmts: &[Stmt]) -> Vec<&Call> {
    let mut out = Vec::new();
    collect_calls(stmts, &mut out);
    out
}

fn collect_calls<'s>(stmts: &'s [Stmt], out: &mut Vec<&'s Call>) {
    fn in_expr<'e>(expr: &'e Expr, out: &mut Vec<&'e Call>) {
        match expr {
            Expr::Call(call) => {
                out.push(call);
                call.args.iter().for_each(|a| in_expr(a, out));
            }
            Expr::Unary(_, inner) => in_expr(inner, out),
            Expr::Binary(_, a, b) => {
                in_expr(a, out);
                in_expr(b, out);
            }
            _ => {}
        }
    }
    for stmt in stmts {
        match stmt {
            Stmt::Set(_, value) => in_expr(value, out),
            Stmt::If { arms, otherwise } => {
                for (cond, body) in arms {
                    in_expr(cond, out);
                    collect_calls(body, out);
                }
                collect_calls(otherwise, out);
            }
            Stmt::Call(call) => {
                out.push(call);
                call.args.iter().for_each(|a| in_expr(a, out));
            }
            Stmt::Return => {}
        }
    }
}

/// Every variable a statement list reads or writes, in order (a `set`
/// target first, then its value).
pub fn vars_in_stmts<'s>(stmts: &'s [Stmt], f: &mut dyn FnMut(&'s VarRef)) {
    for stmt in stmts {
        match stmt {
            Stmt::Set(var, value) => {
                f(var);
                vars_in_expr(value, f);
            }
            Stmt::If { arms, otherwise } => {
                for (cond, body) in arms {
                    vars_in_expr(cond, f);
                    vars_in_stmts(body, f);
                }
                vars_in_stmts(otherwise, f);
            }
            Stmt::Call(call) => {
                if let Some(RefExpr::Var(var)) = &call.on {
                    f(var);
                }
                call.args.iter().for_each(|a| vars_in_expr(a, f));
            }
            Stmt::Return => {}
        }
    }
}

/// Every variable an expression reads.
pub fn vars_in_expr<'e>(expr: &'e Expr, f: &mut dyn FnMut(&'e VarRef)) {
    match expr {
        Expr::Var(var) | Expr::Ref(RefExpr::Var(var)) => f(var),
        Expr::Call(call) => {
            if let Some(RefExpr::Var(var)) = &call.on {
                f(var);
            }
            call.args.iter().for_each(|a| vars_in_expr(a, f));
        }
        Expr::Unary(_, inner) => vars_in_expr(inner, f),
        Expr::Binary(_, a, b) => {
            vars_in_expr(a, f);
            vars_in_expr(b, f);
        }
        _ => {}
    }
}

/// Every expression of a condition, in order.
pub fn exprs_in_cond<'c>(cond: &'c Cond, f: &mut dyn FnMut(&'c Expr)) {
    match cond {
        Cond::Not(inner) => exprs_in_cond(inner, f),
        Cond::All(items) | Cond::Any(items) => items.iter().for_each(|c| exprs_in_cond(c, f)),
        Cond::Cmp { lhs, rhs, .. } => {
            exprs_in_expr(lhs, f);
            exprs_in_expr(rhs, f);
        }
        Cond::True | Cond::False | Cond::Fallback { .. } => {}
    }
}

/// Every condition the module holds, with nothing to say whose: INFOs,
/// packages, stage entries, objective targets, menu items, message
/// buttons, dialogue nodes and run slots.
pub fn module_conditions(module: &Module) -> Vec<&Cond> {
    let mut out: Vec<&Cond> = Vec::new();
    out.extend(module.infos.values().map(|i| &i.condition));
    out.extend(module.packages.values().map(|p| &p.condition));
    for quest in module.quests.values() {
        for stage in &quest.stages {
            out.extend(stage.entries.iter().map(|e| &e.condition));
        }
        for objective in &quest.objectives {
            out.extend(objective.targets.iter().map(|(_, c)| c));
        }
    }
    for terminal in module.terminals.values() {
        out.extend(terminal.items.iter().map(|i| &i.condition));
    }
    for message in module.messages.values() {
        out.extend(message.buttons.iter().map(|(_, c)| c));
    }
    for dialogue in module.dialogues.values() {
        out.extend(dialogue.nodes.iter().map(|n| &n.condition));
    }
    for run in module.runs.values() {
        out.extend(run.slots.iter().map(|s| &s.condition));
    }
    out
}

/// Every statement list the module holds: each block of each script (an
/// INFO's begin and end blocks are copies of its result scripts' blocks,
/// so they are not listed twice).
pub fn module_bodies(module: &Module) -> Vec<(&super::SimId, &[Stmt])> {
    let mut out: Vec<(&super::SimId, &[Stmt])> = Vec::new();
    for script in module.scripts.values() {
        for block in &script.blocks {
            out.push((&script.id, block.body.as_slice()));
        }
    }
    out
}
