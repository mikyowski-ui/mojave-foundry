//! Statements and blocks to IR (spec 3.5, 5.3).

use super::ast::{BlockAst, ExprAst, Path, StmtAst, StmtKind};
use super::resolve::{Lowered, Resolver};
use crate::diag::{Category, Diag};
use crate::ir::{Block, Event, Expr, Func, RefExpr, Stmt};

/// Whether Pass 2 runs a block type (spec 5.3).
pub fn is_supported(event: &Event) -> bool {
    !matches!(
        event,
        Event::MenuMode
            | Event::SayToDone
            | Event::OnAdd
            | Event::OnEquip
            | Event::OnUnequip
            | Event::ScriptEffectStart
            | Event::ScriptEffectUpdate
            | Event::ScriptEffectFinish
            | Event::Other(_)
    )
}

impl Resolver<'_> {
    /// A block; its type and arguments become the [`Event`].
    pub fn block(&mut self, ast: &BlockAst) -> Block {
        let event = self.event(ast);
        if !is_supported(&event) {
            let name = if ast.name.text.is_empty() {
                "(none)".to_owned()
            } else {
                ast.name.text.clone()
            };
            let diag = Diag::warning(
                Category::UnsupportedBlock,
                format!("block {name} is not run in Pass 2"),
            )
            .owned_by(self.id)
            .at(ast.name.span)
            .about(name);
            self.diags.push(diag);
        }
        let body = self.stmts(&ast.body);
        if !body.is_empty() && only_unsupported(&body) {
            let diag = Diag::warning(
                Category::EmptyBlock,
                format!("every statement of block {} is unsupported", ast.name.text),
            )
            .owned_by(self.id)
            .at(ast.name.span)
            .about(ast.name.text.clone());
            self.diags.push(diag);
        }
        Block { event, body }
    }

    fn event_ref(&mut self, args: &[ExprAst]) -> Option<RefExpr> {
        match args.first() {
            Some(ExprAst::Path(Path::Simple(name))) => self.reference(name),
            Some(other) => {
                let diag =
                    Diag::warning(Category::BadBlock, "a block argument that is no reference")
                        .owned_by(self.id)
                        .at(other.span());
                self.diags.push(diag);
                None
            }
            None => None,
        }
    }

    fn event_form(&mut self, args: &[ExprAst]) -> Option<crate::ir::FormKey> {
        let arg = args.first()?;
        match self.expr(arg) {
            Expr::Form(form) => Some(form),
            _ => None,
        }
    }

    fn event(&mut self, ast: &BlockAst) -> Event {
        let args = &ast.args;
        match ast.name.key().as_str() {
            "gamemode" => Event::GameMode,
            "menumode" => Event::MenuMode,
            "onactivate" => Event::OnActivate,
            "onload" => Event::OnLoad,
            "ondeath" => Event::OnDeath {
                killer: self.event_ref(args),
            },
            "onhit" => Event::OnHit {
                by: self.event_ref(args),
            },
            "ontrigger" => Event::OnTrigger {
                by: self.event_ref(args),
            },
            "ontriggerenter" => Event::OnTriggerEnter {
                by: self.event_ref(args),
            },
            "ontriggerleave" => Event::OnTriggerLeave {
                by: self.event_ref(args),
            },
            "onstartcombat" => Event::OnStartCombat,
            "oncombatend" => Event::OnCombatEnd,
            "saytodone" => Event::SayToDone,
            "onpackagestart" => Event::OnPackageStart(self.event_form(args)),
            "onpackageend" => Event::OnPackageEnd(self.event_form(args)),
            "onpackagechange" => Event::OnPackageChange(self.event_form(args)),
            "onadd" => Event::OnAdd,
            "onequip" => Event::OnEquip,
            "onunequip" => Event::OnUnequip,
            "scripteffectstart" => Event::ScriptEffectStart,
            "scripteffectupdate" => Event::ScriptEffectUpdate,
            "scripteffectfinish" => Event::ScriptEffectFinish,
            _ => Event::Other(ast.name.text.clone()),
        }
    }

    /// Statements, in order; lines that did not parse are left out (their
    /// diagnostic is already written).
    pub fn stmts(&mut self, stmts: &[StmtAst]) -> Vec<Stmt> {
        let mut out = Vec::new();
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Set { target, value } => {
                    let value = self.expr(value);
                    if let Some(var) = self.target(target) {
                        out.push(Stmt::Set(var, value));
                    }
                }
                StmtKind::If { arms, otherwise } => {
                    let arms = arms
                        .iter()
                        .map(|(cond, body)| (self.expr(cond), self.stmts(body)))
                        .collect();
                    let otherwise = self.stmts(otherwise);
                    out.push(Stmt::If { arms, otherwise });
                }
                StmtKind::Call(call) => match self.call(call, true) {
                    Lowered::Call(call) => out.push(Stmt::Call(call)),
                    Lowered::Expr(Expr::Call(call)) => out.push(Stmt::Call(*call)),
                    Lowered::Expr(_) | Lowered::Nothing => {}
                },
                StmtKind::Return => out.push(Stmt::Return),
                StmtKind::Invalid => {}
            }
        }
        out
    }
}

/// Whether every statement is an unsupported call.
fn only_unsupported(stmts: &[Stmt]) -> bool {
    stmts.iter().all(|s| {
        matches!(
            s,
            Stmt::Call(crate::ir::Call {
                func: Func::Unsupported { .. },
                ..
            })
        )
    })
}
