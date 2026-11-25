//! Name resolution and expression lowering (spec 5.2).
//!
//! A name is looked up in this order: a variable of the script (its own,
//! then the variables it can see unqualified: a stage script sees its
//! quest's, a dialogue result script its speaker's), `player` and
//! `playerref`, a placed reference's editor id, any other form's editor id,
//! and last a function of the signature table. A name with several forms
//! resolves through the script's own reference list (`SCRO`); a placed
//! reference wins over a base form.

use std::collections::BTreeSet;

use super::ast::{CallAst, ExprAst, Name, Path};
use super::functions::{self, Arg, Signature, Status};
use super::names::{NameTable, NamedForm};
use crate::diag::{Category, Diag, Span};
use crate::ir::{
    ActorValue, BinOp, Call, CmpOp, Expr, FormKey, Func, FuncId, RefExpr, SimId, SimKind, TextRole,
    VarRef,
};
use crate::texts::TextTable;

/// A variable visible to the script by its bare name.
#[derive(Debug, Clone)]
pub(crate) struct ScopeVar {
    pub name: String,
    pub var: VarRef,
}

/// What lowering a call gave.
pub(crate) enum Lowered {
    /// A value (a folded call, or a call inside an expression).
    Expr(Expr),
    /// A call statement.
    Call(Call),
    /// Nothing to run.
    Nothing,
}

/// Resolves the names of one script.
pub(crate) struct Resolver<'a> {
    pub id: &'a SimId,
    pub scope: Vec<ScopeVar>,
    pub references: &'a [FormKey],
    pub names: &'a NameTable,
    pub texts: &'a mut TextTable,
    pub diags: Vec<Diag>,
    pub named: BTreeSet<NamedForm>,
    /// Set while lowering the arguments of a call that is dropped anyway:
    /// names that do not resolve there are not reported.
    pub quiet: bool,
}

fn clamp_int(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

impl Resolver<'_> {
    fn diag(&mut self, diag: Diag, span: Span, subject: &str) {
        if self.quiet && diag.category.group() == "name" {
            return;
        }
        let diag = diag.owned_by(self.id).at(span).about(subject);
        self.diags.push(diag);
    }

    fn scope_var(&self, key: &str) -> Option<&VarRef> {
        self.scope
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(key))
            .map(|v| &v.var)
    }

    /// The form a name means, by the rules above; `None` when it names no
    /// form.
    fn pick(&mut self, name: &Name) -> Option<NamedForm> {
        let forms = self.names.forms(&name.text);
        let chosen = match forms {
            [] => {
                let raw = self.names.raw_form(&name.text)?;
                self.named.insert(raw.clone());
                return Some(raw);
            }
            [one] => one.clone(),
            many => {
                let listed: Vec<&NamedForm> = many
                    .iter()
                    .filter(|f| self.references.contains(&f.form))
                    .collect();
                if let [one] = listed.as_slice() {
                    (*one).clone()
                } else {
                    let pool: Vec<&NamedForm> = if listed.is_empty() {
                        many.iter().collect()
                    } else {
                        listed
                    };
                    let chosen = pool
                        .iter()
                        .find(|f| f.is_placed())
                        .copied()
                        .unwrap_or(pool[0])
                        .clone();
                    let message = format!(
                        "{} names {} forms; taking {}",
                        name.text,
                        many.len(),
                        chosen.form
                    );
                    self.diag(
                        Diag::warning(Category::AmbiguousName, message),
                        name.span,
                        &name.text,
                    );
                    chosen
                }
            }
        };
        self.named.insert(chosen.clone());
        Some(chosen)
    }

    fn unresolved(&mut self, category: Category, span: Span, text: &str, message: String) -> Expr {
        self.diag(Diag::error(category, message), span, text);
        Expr::Int(0)
    }

    /// A bare name as a value.
    fn simple(&mut self, name: &Name) -> Expr {
        let key = name.key();
        if let Some(var) = self.scope_var(&key) {
            return Expr::Var(var.clone());
        }
        if key == "player" || key == "playerref" {
            return Expr::Ref(RefExpr::Player);
        }
        if let Some(found) = self.pick(name) {
            return form_value(&found);
        }
        // A function, or a name shaped like one (`GetX`, `IsX`, `HasX`)
        // that no table knows: an unknown function of no arguments.
        if functions::lookup(&key).is_some() || looks_like_function(&key) {
            let call = CallAst {
                on: None,
                name: name.clone(),
                args: Vec::new(),
            };
            return match self.call(&call, false) {
                Lowered::Expr(expr) => expr,
                Lowered::Call(call) => Expr::Call(Box::new(call)),
                Lowered::Nothing => Expr::Int(0),
            };
        }
        self.unresolved(
            Category::UnresolvedName,
            name.span,
            &name.text,
            format!("{} is no variable, form or function", name.text),
        )
    }

    /// `a.b` as a value or an assignment target.
    pub fn member(&mut self, base: &Name, member: &Name) -> Option<VarRef> {
        let key = base.key();
        if self.scope_var(&key).is_some() {
            self.diag(
                Diag::error(
                    Category::UnresolvedRefVar,
                    format!(
                        "{}.{}: a variable of a reference held in a variable is not supported",
                        base.text, member.text
                    ),
                ),
                base.span,
                &base.text,
            );
            return None;
        }
        let Some(found) = self.pick(base) else {
            self.diag(
                Diag::error(
                    Category::UnresolvedName,
                    format!("{} is no quest or reference", base.text),
                ),
                base.span,
                &base.text,
            );
            return None;
        };
        if found.sig == "QUST" {
            let quest = SimId::of(SimKind::Quest, &found.form);
            let var = self
                .names
                .quest_vars(&found.form)
                .and_then(|vars| {
                    vars.iter()
                        .find(|v| v.name.eq_ignore_ascii_case(&member.text))
                })
                .map(|v| VarRef::Quest {
                    quest,
                    index: v.index,
                    ty: v.ty,
                });
            if var.is_none() {
                self.diag(
                    Diag::error(
                        Category::UnresolvedQuestVar,
                        format!("quest {} has no variable {}", base.text, member.text),
                    ),
                    member.span,
                    &format!("{}.{}", base.text, member.text),
                );
            }
            return var;
        }
        if found.is_placed() {
            let script = self.names.ref_script(&found.form).cloned();
            let var = script.as_ref().and_then(|script| {
                self.names
                    .script_vars(script)
                    .and_then(|vars| {
                        vars.iter()
                            .find(|v| v.name.eq_ignore_ascii_case(&member.text))
                    })
                    .map(|v| VarRef::Hoisted {
                        owner: found.form.clone(),
                        script: SimId::of(SimKind::Script, script),
                        index: v.index,
                        ty: v.ty,
                    })
            });
            if var.is_none() {
                self.diag(
                    Diag::error(
                        Category::UnresolvedRefVar,
                        format!(
                            "reference {} has no script variable {}",
                            base.text, member.text
                        ),
                    ),
                    member.span,
                    &format!("{}.{}", base.text, member.text),
                );
            }
            return var;
        }
        self.diag(
            Diag::error(
                Category::UnresolvedRefVar,
                format!("{} ({}) has no variables", base.text, found.sig),
            ),
            base.span,
            &base.text,
        );
        None
    }

    /// Whether `a.b` in an expression is a call of an unknown function
    /// `b` on the reference `a` rather than a variable: `a` is the player,
    /// a `ref` variable, or a placed reference whose script has no
    /// variable `b`.
    fn is_call_shaped(&self, base: &Name, member: &Name) -> bool {
        let key = base.key();
        if key == "player" || key == "playerref" {
            return true;
        }
        if let Some(var) = self.scope_var(&key) {
            return matches!(
                var,
                VarRef::Local {
                    ty: crate::ir::VarTy::Ref,
                    ..
                } | VarRef::Quest {
                    ty: crate::ir::VarTy::Ref,
                    ..
                }
            );
        }
        let forms = self.names.forms(&base.text);
        let Some(found) = forms.iter().find(|f| f.is_placed()) else {
            return false;
        };
        if forms.iter().any(|f| f.sig == "QUST") {
            return false;
        }
        let has_var = self
            .names
            .ref_script(&found.form)
            .and_then(|s| self.names.script_vars(s))
            .is_some_and(|vars| {
                vars.iter()
                    .any(|v| v.name.eq_ignore_ascii_case(&member.text))
            });
        !has_var
    }

    /// A path as an assignment target.
    pub fn target(&mut self, path: &Path) -> Option<VarRef> {
        match path {
            Path::Simple(name) => {
                if let Some(var) = self.scope_var(&name.key()) {
                    return Some(var.clone());
                }
                match self.simple(name) {
                    Expr::Var(var @ VarRef::Global(_)) => Some(var),
                    Expr::Int(0) => None,
                    _ => {
                        self.diag(
                            Diag::error(
                                Category::BadAssignment,
                                format!("{} is not a variable", name.text),
                            ),
                            name.span,
                            &name.text,
                        );
                        None
                    }
                }
            }
            Path::Member(base, member) => self.member(base, member),
        }
    }

    /// A name used as a reference.
    pub fn reference(&mut self, name: &Name) -> Option<RefExpr> {
        let key = name.key();
        if let Some(var) = self.scope_var(&key) {
            return Some(RefExpr::Var(var.clone()));
        }
        if key == "player" || key == "playerref" {
            return Some(RefExpr::Player);
        }
        match self.pick(name) {
            Some(found) if found.is_placed() => Some(RefExpr::Placed(found.form)),
            Some(found) if self.names.unique_ref(&found.form).is_some() => {
                let only = self.names.unique_ref(&found.form).cloned()?;
                self.diag(
                    Diag::note(
                        Category::BaseAsReference,
                        format!(
                            "{} ({}) taken as its only placed reference {only}",
                            name.text, found.sig
                        ),
                    ),
                    name.span,
                    &name.text,
                );
                Some(RefExpr::Placed(only))
            }
            Some(found) => {
                self.diag(
                    Diag::warning(
                        Category::CallOnNonReference,
                        format!("{} is a {} record, not a reference", name.text, found.sig),
                    ),
                    name.span,
                    &name.text,
                );
                None
            }
            None => {
                if let Some(Lowered::Expr(Expr::Ref(r))) = functions::lookup(&key).map(|_| {
                    self.call(
                        &CallAst {
                            on: None,
                            name: name.clone(),
                            args: Vec::new(),
                        },
                        false,
                    )
                }) {
                    return Some(r);
                }
                self.diag(
                    Diag::error(
                        Category::UnresolvedName,
                        format!("{} is no reference", name.text),
                    ),
                    name.span,
                    &name.text,
                );
                None
            }
        }
    }

    /// An expression.
    pub fn expr(&mut self, ast: &ExprAst) -> Expr {
        match ast {
            ExprAst::Int(v, _) => Expr::Int(clamp_int(*v)),
            ExprAst::Float(v, _) => Expr::Float(*v as f32),
            ExprAst::Str(s, _) => Expr::Str(self.texts.intern(self.id, TextRole::MessageBox, s)),
            ExprAst::Path(Path::Simple(name)) => self.simple(name),
            ExprAst::Path(Path::Member(base, member)) if self.is_call_shaped(base, member) => {
                let call = CallAst {
                    on: Some(base.clone()),
                    name: member.clone(),
                    args: Vec::new(),
                };
                match self.call(&call, false) {
                    Lowered::Expr(expr) => expr,
                    Lowered::Call(call) => Expr::Call(Box::new(call)),
                    Lowered::Nothing => Expr::Int(0),
                }
            }
            ExprAst::Path(Path::Member(base, member)) => match self.member(base, member) {
                Some(var) => Expr::Var(var),
                None => Expr::Int(0),
            },
            ExprAst::Call(call) => match self.call(call, false) {
                Lowered::Expr(expr) => expr,
                Lowered::Call(call) => Expr::Call(Box::new(call)),
                Lowered::Nothing => Expr::Int(0),
            },
            ExprAst::Unary(op, inner, _) => Expr::Unary(*op, Box::new(self.expr(inner))),
            ExprAst::Binary(op, lhs, rhs, _) => {
                Expr::Binary(*op, Box::new(self.expr(lhs)), Box::new(self.expr(rhs)))
            }
        }
    }

    /// One argument of a known function.
    fn arg(&mut self, kind: Arg, ast: &ExprAst) -> Expr {
        match (kind, ast) {
            (Arg::ActorValue, ExprAst::Path(Path::Simple(name))) => {
                match ActorValue::named(&name.text) {
                    Some(av) if self.scope_var(&name.key()).is_none() => {
                        Expr::Int(i32::try_from(av.0).unwrap_or(i32::MAX))
                    }
                    _ => self.expr(ast),
                }
            }
            (Arg::Keyword, ExprAst::Path(Path::Simple(name))) => {
                Expr::Int(keyword_value(&name.text))
            }
            (Arg::Ref, ExprAst::Path(Path::Simple(name))) => match self.reference(name) {
                Some(r) => Expr::Ref(r),
                None => Expr::Int(0),
            },
            _ => self.expr(ast),
        }
    }

    /// A call. `statement` says whether its value is unused.
    pub fn call(&mut self, call: &CallAst, statement: bool) -> Lowered {
        let span = call.name.span;
        let on = match &call.on {
            Some(name) => match self.reference(name) {
                Some(r) => Some(r),
                None => {
                    // The diagnostic is written; the call cannot run on
                    // what it names.
                    let args = call.args.iter().map(|a| self.expr(a)).collect();
                    let unsupported = Call {
                        func: Func::Unsupported {
                            name: call.name.text.clone(),
                        },
                        on: None,
                        args,
                    };
                    return if statement {
                        Lowered::Call(unsupported)
                    } else {
                        Lowered::Expr(Expr::Call(Box::new(unsupported)))
                    };
                }
            },
            None => None,
        };
        let Some(sig) = functions::lookup(&call.name.text) else {
            if call.on.is_none()
                && call.args.is_empty()
                && self.scope_var(&call.name.key()).is_some()
            {
                self.diag(
                    Diag::error(
                        Category::BadExpression,
                        format!("variable {} used as a statement", call.name.text),
                    ),
                    span,
                    &call.name.text,
                );
                return Lowered::Nothing;
            }
            self.diag(
                Diag::warning(
                    Category::UnknownFunction,
                    format!("{} is not in the signature table; dropped", call.name.text),
                ),
                span,
                &call.name.text,
            );
            let was = std::mem::replace(&mut self.quiet, true);
            let args = call.args.iter().map(|a| self.expr(a)).collect();
            self.quiet = was;
            let unknown = Call {
                func: Func::Unsupported {
                    name: call.name.text.clone(),
                },
                on,
                args,
            };
            return if statement {
                Lowered::Call(unknown)
            } else {
                Lowered::Expr(Expr::Call(Box::new(unknown)))
            };
        };
        let count = call.args.len();
        if count < usize::from(sig.min) || count > sig.max() {
            self.diag(
                Diag::warning(
                    Category::ArgumentCount,
                    format!(
                        "{} takes {} to {} arguments, given {count}",
                        sig.name,
                        sig.min,
                        sig.max()
                    ),
                ),
                span,
                sig.name,
            );
        }
        if sig.status == Status::Folded {
            let value = self.fold(sig, on, call);
            if statement {
                self.diag(
                    Diag::note(
                        Category::DroppedFunction,
                        format!("{} has no effect as a statement", sig.name),
                    ),
                    span,
                    sig.name,
                );
                return Lowered::Nothing;
            }
            return Lowered::Expr(value);
        }
        let quiet = self.quiet || sig.status == Status::Dropped;
        let was = std::mem::replace(&mut self.quiet, quiet);
        let args: Vec<Expr> = call
            .args
            .iter()
            .enumerate()
            .map(|(i, a)| self.arg(sig.args.get(i).copied().unwrap_or(Arg::Any), a))
            .collect();
        self.quiet = was;
        let func = match sig.status {
            Status::Compiled => Func::Known(FuncId::from_table(sig.name)),
            _ => {
                self.diag(
                    Diag::warning(
                        Category::DroppedFunction,
                        format!("{} is not supported in Pass 2; dropped", sig.name),
                    ),
                    span,
                    sig.name,
                );
                Func::Unsupported {
                    name: sig.name.to_owned(),
                }
            }
        };
        let lowered = Call { func, on, args };
        if statement {
            Lowered::Call(lowered)
        } else {
            Lowered::Expr(Expr::Call(Box::new(lowered)))
        }
    }

    /// The value of a folded function.
    fn fold(&mut self, sig: &Signature, on: Option<RefExpr>, call: &CallAst) -> Expr {
        match sig.name {
            "GetSelf" => Expr::Ref(on.unwrap_or(RefExpr::SelfRef)),
            "GetActionRef" => Expr::Ref(RefExpr::Action),
            "GetLinkedRef" => Expr::Ref(RefExpr::Linked(Box::new(on.unwrap_or(RefExpr::SelfRef)))),
            "IsActionRef" => {
                let other = match call.args.first() {
                    Some(arg) => self.arg(Arg::Ref, arg),
                    None => Expr::Int(0),
                };
                Expr::Binary(
                    BinOp::Cmp(CmpOp::Eq),
                    Box::new(Expr::Ref(RefExpr::Action)),
                    Box::new(other),
                )
            }
            "GetGlobalValue" => match call.args.first().map(|a| self.expr(a)) {
                Some(var @ Expr::Var(VarRef::Global(_))) => var,
                _ => {
                    self.diag(
                        Diag::error(Category::ArgumentKind, "GetGlobalValue needs a global"),
                        call.name.span,
                        sig.name,
                    );
                    Expr::Int(0)
                }
            },
            "GetQuestVariable" => match (call.args.first(), call.args.get(1)) {
                (
                    Some(ExprAst::Path(Path::Simple(quest))),
                    Some(ExprAst::Path(Path::Simple(var))),
                ) => match self.member(quest, var) {
                    Some(var) => Expr::Var(var),
                    None => Expr::Int(0),
                },
                _ => {
                    self.diag(
                        Diag::error(
                            Category::ArgumentKind,
                            "GetQuestVariable needs a quest and a variable name",
                        ),
                        call.name.span,
                        sig.name,
                    );
                    Expr::Int(0)
                }
            },
            other => unreachable!("{other} is not a folded function"),
        }
    }
}

/// Whether an unknown name reads as a function's: `Get`, `Is` or `Has`
/// followed by an upper-case letter or a digit in the usual spelling, or
/// any longer name with that prefix in lower case.
fn looks_like_function(key: &str) -> bool {
    ["get", "is", "has"]
        .iter()
        .any(|prefix| key.len() > prefix.len() + 2 && key.starts_with(prefix))
}

/// The value of a bare word argument ([`Arg::Keyword`]): a sex or an axis
/// by its number, anything else 0.
fn keyword_value(word: &str) -> i32 {
    match word.to_ascii_lowercase().as_str() {
        "male" | "x" => 0,
        "female" | "y" => 1,
        "z" => 2,
        _ => 0,
    }
}

/// The value of a named form.
fn form_value(found: &NamedForm) -> Expr {
    if found.is_placed() {
        Expr::Ref(RefExpr::Placed(found.form.clone()))
    } else if found.sig == "GLOB" {
        Expr::Var(VarRef::Global(SimId::of(SimKind::Global, &found.form)))
    } else {
        Expr::Form(found.form.clone())
    }
}
