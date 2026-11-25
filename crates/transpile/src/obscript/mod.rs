//! The source games' script language front end (spec 5): [`lex`],
//! [`parse`], name resolution ([`names`], `resolve`), the function
//! signature table ([`functions`]) and the lowering of statements to IR.
//! Code generation is not here (task T8).
//!
//! [`compile`] never fails: whatever does not lex, parse or resolve is a
//! diagnostic on the script, with a line and a column, and the rest of the
//! script is compiled.

pub mod ast;
pub mod functions;
pub mod lex;
mod lower;
pub mod names;
pub mod parse;
mod resolve;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub use lower::is_supported;
pub use names::{NameTable, NamedForm, is_placed_sig};
pub use parse::Mode;

use crate::diag::Diag;
use crate::ir::{
    Block, Event, FormKey, Script, ScriptKind, SimId, SimKind, VarDecl, VarRef, VarTy,
};
use crate::texts::TextTable;
use resolve::{Resolver, ScopeVar};

/// A local variable as the record declares it (`SLSD` and `SCVR`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredVar {
    /// The variable's index.
    pub index: u32,
    /// Its name.
    pub name: String,
    /// Flags; bit 0 marks an integer.
    pub flags: u8,
}

/// One script to compile.
#[derive(Debug, Clone)]
pub struct ScriptInput<'a> {
    /// Its id ([`SimKind::Script`]).
    pub id: SimId,
    /// The record's editor id, or the owner's for a result script.
    pub editor_id: String,
    /// What it is. A [`ScriptKind::Result`] parses as a statement list.
    pub kind: ScriptKind,
    /// The source text.
    pub source: &'a str,
    /// The record's variable list (`SLSD`/`SCVR`); may be empty.
    pub declared: &'a [DeclaredVar],
    /// The record's reference list (`SCRO`), resolved.
    pub references: &'a [FormKey],
    /// The quest whose variables this script's bare names mean: the
    /// quest of a quest script or of a stage script.
    pub quest: Option<FormKey>,
    /// Another script whose variables the script sees by bare name: a
    /// dialogue result script sees its speaker's.
    pub implicit: Option<(SimId, Vec<VarDecl>)>,
}

/// What [`compile`] gives.
#[derive(Debug, Clone)]
pub struct Compiled {
    /// The syntax tree.
    pub ast: ast::ScriptAst,
    /// The IR, with every diagnostic.
    pub script: Script,
    /// Every form a name of the script resolved to.
    pub named: BTreeSet<NamedForm>,
}

/// The variables a script declares, with their indices: the record's
/// index for a name it lists, else the next free index after the
/// record's, in declaration order. A name the record lists and the source
/// does not declare is kept, typed by the record's flag.
pub fn declarations(source: &str, declared: &[DeclaredVar]) -> Vec<VarDecl> {
    let mut diags = Vec::new();
    let ast = parse::parse(source, Mode::Record, &mut diags);
    merge_declarations(&ast.decls, declared)
}

fn merge_declarations(decls: &[ast::DeclAst], declared: &[DeclaredVar]) -> Vec<VarDecl> {
    let mut out: Vec<VarDecl> = Vec::new();
    let mut next = declared.iter().map(|d| d.index).max().unwrap_or(0) + 1;
    for decl in decls {
        if out
            .iter()
            .any(|v| v.name.eq_ignore_ascii_case(&decl.name.text))
        {
            continue;
        }
        let index = match declared
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(&decl.name.text))
        {
            Some(d) => d.index,
            None => {
                let index = next;
                next += 1;
                index
            }
        };
        out.push(VarDecl {
            index,
            name: decl.name.text.clone(),
            ty: decl.ty,
        });
    }
    for d in declared {
        if !out.iter().any(|v| v.name.eq_ignore_ascii_case(&d.name)) && !d.name.is_empty() {
            out.push(VarDecl {
                index: d.index,
                name: d.name.clone(),
                ty: if d.flags & 1 != 0 {
                    VarTy::Int
                } else {
                    VarTy::Float
                },
            });
        }
    }
    out.sort_by_key(|v| v.index);
    out
}

/// Compiles one script: parse, resolve, lower. Never fails.
pub fn compile(input: &ScriptInput<'_>, names: &NameTable, texts: &mut TextTable) -> Compiled {
    let mode = match input.kind {
        ScriptKind::Result(_) => Mode::Result,
        _ => Mode::Record,
    };
    let mut diags = Vec::new();
    let ast = parse::parse(input.source, mode, &mut diags);
    let vars = merge_declarations(&ast.decls, input.declared);

    let own_quest = match input.kind {
        ScriptKind::Quest => input.quest.clone(),
        _ => None,
    };
    let mut scope: Vec<ScopeVar> = vars
        .iter()
        .map(|v| ScopeVar {
            name: v.name.clone(),
            var: match &own_quest {
                Some(quest) => VarRef::Quest {
                    quest: SimId::of(SimKind::Quest, quest),
                    index: v.index,
                    ty: v.ty,
                },
                None => VarRef::Local {
                    script: input.id.clone(),
                    index: v.index,
                    ty: v.ty,
                },
            },
        })
        .collect();
    if own_quest.is_none()
        && let Some(quest) = &input.quest
        && let Some(quest_vars) = names.quest_vars(quest)
    {
        scope.extend(quest_vars.iter().map(|v| ScopeVar {
            name: v.name.clone(),
            var: VarRef::Quest {
                quest: SimId::of(SimKind::Quest, quest),
                index: v.index,
                ty: v.ty,
            },
        }));
    }
    if let Some((script, implicit)) = &input.implicit {
        scope.extend(implicit.iter().map(|v| ScopeVar {
            name: v.name.clone(),
            var: VarRef::Local {
                script: script.clone(),
                index: v.index,
                ty: v.ty,
            },
        }));
    }

    let mut resolver = Resolver {
        id: &input.id,
        scope,
        references: input.references,
        names,
        texts,
        diags: Vec::new(),
        named: BTreeSet::new(),
        quiet: false,
    };
    let blocks: Vec<Block> = match mode {
        Mode::Record => ast.blocks.iter().map(|b| resolver.block(b)).collect(),
        Mode::Result => vec![Block {
            event: Event::Result,
            body: resolver.stmts(&ast.body),
        }],
    };
    let named = std::mem::take(&mut resolver.named);
    let mut all: Vec<Diag> = diags
        .into_iter()
        .map(|d| d.owned_by(&input.id))
        .chain(std::mem::take(&mut resolver.diags))
        .collect();
    all.sort_by_key(|d| d.at.map_or((u32::MAX, u32::MAX), |s| (s.line, s.column)));
    Compiled {
        script: Script {
            id: input.id.clone(),
            editor_id: input.editor_id.clone(),
            kind: input.kind.clone(),
            vars,
            blocks,
            diagnostics: all,
        },
        ast,
        named,
    }
}
