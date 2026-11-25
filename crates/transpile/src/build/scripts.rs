//! Names and script compilation for the build.

use super::Builder;
use crate::diag::{Category, Diag};
use crate::ir::{
    Call, Expr, FormKey, Func, RefExpr, RefUses, ScriptKind, SimId, SimKind, Stmt, VarDecl, VarRef,
};
use crate::obscript::{self, ScriptInput};
use crate::source::SourceBody;

/// Every call of a statement list, nested ones included.
fn calls<'s>(stmts: &'s [Stmt], out: &mut Vec<&'s Call>) {
    fn expr_calls<'e>(expr: &'e Expr, out: &mut Vec<&'e Call>) {
        match expr {
            Expr::Call(call) => {
                out.push(call);
                for arg in &call.args {
                    expr_calls(arg, out);
                }
            }
            Expr::Unary(_, inner) => expr_calls(inner, out),
            Expr::Binary(_, a, b) => {
                expr_calls(a, out);
                expr_calls(b, out);
            }
            _ => {}
        }
    }
    for stmt in stmts {
        match stmt {
            Stmt::Set(_, value) => expr_calls(value, out),
            Stmt::If { arms, otherwise } => {
                for (cond, body) in arms {
                    expr_calls(cond, out);
                    calls(body, out);
                }
                calls(otherwise, out);
            }
            Stmt::Call(call) => {
                out.push(call);
                for arg in &call.args {
                    expr_calls(arg, out);
                }
            }
            Stmt::Return => {}
        }
    }
}

impl Builder<'_> {
    /// Compiles a script record once; returns its id.
    pub(super) fn compile_record(
        &mut self,
        form: &FormKey,
        quest: Option<&FormKey>,
    ) -> Option<SimId> {
        let id = SimId::of(SimKind::Script, form);
        if self.module.scripts.contains_key(&id) {
            return Some(id);
        }
        let Some(script) = self.source.scripts.get(form) else {
            self.diag(
                Diag::warning(
                    Category::MissingRecord,
                    format!("script {form} is not in the MIR"),
                )
                .about(form.as_str()),
            );
            return None;
        };
        let kind = match script.script_type {
            1 => ScriptKind::Quest,
            0x100 => ScriptKind::Effect,
            _ => ScriptKind::Object,
        };
        let quest = match kind {
            ScriptKind::Quest => quest.cloned(),
            _ => None,
        };
        let input = ScriptInput {
            id: id.clone(),
            editor_id: script.editor_id.clone().unwrap_or_default(),
            kind,
            source: &script.body.source,
            declared: &script.body.vars,
            references: &script.body.references,
            quest,
            implicit: None,
        };
        self.absorb(&input);
        Some(id)
    }

    /// Compiles an embedded result script; returns its id.
    pub(super) fn compile_body(
        &mut self,
        id: SimId,
        editor_id: &str,
        kind: ScriptKind,
        body: &SourceBody,
        quest: Option<&FormKey>,
        implicit: Option<(SimId, Vec<VarDecl>)>,
    ) -> SimId {
        let input = ScriptInput {
            id: id.clone(),
            editor_id: editor_id.to_owned(),
            kind,
            source: &body.source,
            declared: &body.vars,
            references: &body.references,
            quest: quest.cloned(),
            implicit,
        };
        self.absorb(&input);
        id
    }

    /// Compiles and records what the script names.
    fn absorb(&mut self, input: &ScriptInput<'_>) {
        let compiled = obscript::compile(input, &self.names, &mut self.texts);
        for named in &compiled.named {
            match named.sig.as_str() {
                "QUST" => {
                    self.uses.quests.insert(named.form.clone());
                }
                "GLOB" => {
                    self.uses.globals.insert(named.form.clone());
                }
                "NOTE" => {
                    self.uses.notes.insert(named.form.clone());
                }
                _ if named.is_placed() => self.uses.mark(&named.form, RefUses::SCRIPT),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for block in &compiled.script.blocks {
            calls(&block.body, &mut found);
        }
        for call in found {
            if let Func::Known(id) = &call.func
                && matches!(id.name(), "Enable" | "Disable")
                && let Some(RefExpr::Placed(target)) = &call.on
            {
                self.uses.enabled.insert(target.clone());
            }
        }
        // Hoisted variables name their owner reference.
        for var in hoisted(&compiled.script) {
            self.uses.mark(&var, RefUses::SCRIPT);
        }
        self.module
            .scripts
            .insert(compiled.script.id.clone(), compiled.script);
    }

    /// The variables of the script on an actor base, for a dialogue
    /// result script spoken by it.
    pub(super) fn speaker_vars(&self, base: &FormKey) -> Option<(SimId, Vec<VarDecl>)> {
        let script = self.names.base_script(base)?;
        let vars = self.names.script_vars(script)?.to_vec();
        Some((SimId::of(SimKind::Script, script), vars))
    }
}

/// The owners of the hoisted variables a script uses.
fn hoisted(script: &crate::ir::Script) -> Vec<FormKey> {
    fn expr(e: &Expr, out: &mut Vec<FormKey>) {
        match e {
            Expr::Var(VarRef::Hoisted { owner, .. }) => out.push(owner.clone()),
            Expr::Call(call) => call.args.iter().for_each(|a| expr(a, out)),
            Expr::Unary(_, inner) => expr(inner, out),
            Expr::Binary(_, a, b) => {
                expr(a, out);
                expr(b, out);
            }
            _ => {}
        }
    }
    fn stmts(list: &[Stmt], out: &mut Vec<FormKey>) {
        for s in list {
            match s {
                Stmt::Set(var, value) => {
                    if let VarRef::Hoisted { owner, .. } = var {
                        out.push(owner.clone());
                    }
                    expr(value, out);
                }
                Stmt::If { arms, otherwise } => {
                    for (c, body) in arms {
                        expr(c, out);
                        stmts(body, out);
                    }
                    stmts(otherwise, out);
                }
                Stmt::Call(call) => call.args.iter().for_each(|a| expr(a, out)),
                Stmt::Return => {}
            }
        }
    }
    let mut out = Vec::new();
    for block in &script.blocks {
        stmts(&block.body, &mut out);
    }
    out.sort();
    out.dedup();
    out
}
