//! The syntax tree the parser builds: names as written, nothing resolved.

use serde::Serialize;

use crate::diag::Span;
use crate::ir::{BinOp, UnOp, VarTy};

/// A name as written, with its position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Name {
    /// The text as written.
    pub text: String,
    /// Where it starts.
    pub span: Span,
}

impl Name {
    /// The text, lowercase: how names compare.
    pub fn key(&self) -> String {
        self.text.to_ascii_lowercase()
    }
}

/// A variable or name: `x`, or `a.b` (a quest variable or another
/// reference's variable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Path {
    /// `x`
    Simple(Name),
    /// `a.b`
    Member(Name, Name),
}

impl Path {
    /// Where it starts.
    pub fn span(&self) -> Span {
        match self {
            Self::Simple(name) | Self::Member(name, _) => name.span,
        }
    }

    /// As written.
    pub fn text(&self) -> String {
        match self {
            Self::Simple(name) => name.text.clone(),
            Self::Member(a, b) => format!("{}.{}", a.text, b.text),
        }
    }
}

/// A call: `[ref.]Function arg ...`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CallAst {
    /// The reference before the dot.
    pub on: Option<Name>,
    /// The function.
    pub name: Name,
    /// The arguments.
    pub args: Vec<ExprAst>,
}

/// An expression.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExprAst {
    /// An integer literal.
    Int(i64, Span),
    /// A float literal.
    Float(f64, Span),
    /// A string literal.
    Str(String, Span),
    /// A name.
    Path(Path),
    /// A call.
    Call(CallAst),
    /// A unary operation.
    Unary(UnOp, Box<ExprAst>, Span),
    /// A binary operation; the span is the operator's.
    Binary(BinOp, Box<ExprAst>, Box<ExprAst>, Span),
}

impl ExprAst {
    /// Where it starts.
    pub fn span(&self) -> Span {
        match self {
            Self::Int(_, s) | Self::Float(_, s) | Self::Str(_, s) | Self::Unary(_, _, s) => *s,
            Self::Path(path) => path.span(),
            Self::Call(call) => call.on.as_ref().unwrap_or(&call.name).span,
            Self::Binary(_, lhs, _, _) => lhs.span(),
        }
    }
}

/// A statement and the position of its line's first token.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StmtAst {
    /// What it is.
    pub kind: StmtKind,
    /// Where it starts.
    pub span: Span,
}

/// A statement.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StmtKind {
    /// `set <path> to <expr>`
    Set {
        /// The variable.
        target: Path,
        /// The value.
        value: ExprAst,
    },
    /// `if` ... `endif`
    If {
        /// Condition and body of each arm.
        arms: Vec<(ExprAst, Vec<StmtAst>)>,
        /// The `else` body.
        otherwise: Vec<StmtAst>,
    },
    /// A call statement.
    Call(CallAst),
    /// `return`
    Return,
    /// A line that did not parse; a diagnostic says why.
    Invalid,
}

/// A variable declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclAst {
    /// The type keyword's type.
    pub ty: VarTy,
    /// The name.
    pub name: Name,
}

/// A block: `begin <type> [args]` ... `end`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlockAst {
    /// The block type as written.
    pub name: Name,
    /// The block's arguments.
    pub args: Vec<ExprAst>,
    /// The statements.
    pub body: Vec<StmtAst>,
}

/// A parsed script.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ScriptAst {
    /// `scn <name>`.
    pub header: Option<Name>,
    /// Declarations, in order.
    pub decls: Vec<DeclAst>,
    /// Blocks, in order.
    pub blocks: Vec<BlockAst>,
    /// A result script's statements (empty for a script record).
    pub body: Vec<StmtAst>,
}

impl ScriptAst {
    /// Statements, counted recursively (`if` counts itself and its arms'
    /// statements), invalid lines included.
    pub fn statement_count(&self) -> usize {
        fn count(stmts: &[StmtAst]) -> usize {
            stmts
                .iter()
                .map(|s| match &s.kind {
                    StmtKind::If { arms, otherwise } => {
                        1 + arms.iter().map(|(_, b)| count(b)).sum::<usize>() + count(otherwise)
                    }
                    _ => 1,
                })
                .sum()
        }
        count(&self.body) + self.blocks.iter().map(|b| count(&b.body)).sum::<usize>()
    }

    /// Every call, statements and calls inside expressions, in source
    /// order.
    pub fn calls(&self) -> Vec<&CallAst> {
        fn expr<'a>(e: &'a ExprAst, out: &mut Vec<&'a CallAst>) {
            match e {
                ExprAst::Call(call) => {
                    out.push(call);
                    call.args.iter().for_each(|a| expr(a, out));
                }
                ExprAst::Unary(_, inner, _) => expr(inner, out),
                ExprAst::Binary(_, a, b, _) => {
                    expr(a, out);
                    expr(b, out);
                }
                _ => {}
            }
        }
        fn stmts<'a>(list: &'a [StmtAst], out: &mut Vec<&'a CallAst>) {
            for s in list {
                match &s.kind {
                    StmtKind::Set { value, .. } => expr(value, out),
                    StmtKind::If { arms, otherwise } => {
                        for (c, body) in arms {
                            expr(c, out);
                            stmts(body, out);
                        }
                        stmts(otherwise, out);
                    }
                    StmtKind::Call(call) => {
                        out.push(call);
                        call.args.iter().for_each(|a| expr(a, out));
                    }
                    StmtKind::Return | StmtKind::Invalid => {}
                }
            }
        }
        let mut out = Vec::new();
        stmts(&self.body, &mut out);
        for block in &self.blocks {
            stmts(&block.body, &mut out);
        }
        out
    }
}
