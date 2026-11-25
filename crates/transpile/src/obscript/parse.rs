//! The parser (spec 5.2), line by line:
//!
//! ```text
//! script   := header? decl* block*
//! header   := ("scn" | "scriptname") IDENT
//! decl     := ("short" | "int" | "long" | "float" | "ref" | "reference") IDENT
//! block    := "begin" IDENT arg* NL stmt* "end"
//! stmt     := "set" lvalue "to" expr | if | "return" | call
//! if       := "if" expr NL stmt* ("elseif" expr NL stmt*)* ("else" NL stmt*)? "endif"
//! call     := (ref ".")? IDENT arg* ("," arg)*
//! expr     := or ; or := and ("||" and)* ; and := cmp ("&&" cmp)*
//! cmp      := sum (("==" | "!=" | "<" | "<=" | ">" | ">=") sum)?
//! sum      := term (("+" | "-") term)* ; term := unary (("*" | "/") unary)*
//! unary    := ("-" | "!")? atom
//! atom     := NUMBER | STRING | lvalue | call-in-expression | "(" expr ")"
//! lvalue   := IDENT | IDENT "." IDENT
//! ```
//!
//! Beyond the grammar, as the source compiler accepts them: `%` at the
//! level of `*`, `=` as `==` (with a warning), and comparisons chaining
//! left to right. Declarations are taken anywhere; one inside a block is
//! noted.
//!
//! **Recovery.** Every line is one statement. A line that does not parse
//! becomes [`StmtKind::Invalid`] with a diagnostic, and the next line
//! starts afresh, so one bad line never loses the script. A missing
//! `endif` or `end` is closed where the enclosing construct ends, with a
//! diagnostic.
//!
//! **Calls in expressions** take as many arguments as the signature
//! table's maximum, while the next token can start an argument. A name
//! the table does not know, followed by something that can start an
//! argument, is an unknown function: it takes arguments until an
//! operator or the line's end (spec 5.2); the resolver warns about it.

use std::collections::HashSet;

use super::ast::{BlockAst, CallAst, DeclAst, ExprAst, Name, Path, ScriptAst, StmtAst, StmtKind};
use super::functions;
use super::lex::{Line, Tok, Token, lex};
use crate::diag::{Category, Diag, Span};
use crate::ir::{BinOp, CmpOp, UnOp, VarTy};

/// What is being parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A `SCPT` record: header, declarations, blocks.
    Record,
    /// An embedded result script: statements only.
    Result,
}

/// Parses a source text. Never fails: what does not parse is in `diags`.
pub fn parse(source: &str, mode: Mode, diags: &mut Vec<Diag>) -> ScriptAst {
    let lines = lex(source);
    let locals = declared_names(&lines);
    let mut parser = Parser {
        mode,
        locals,
        ast: ScriptAst::default(),
        block: None,
        ifs: Vec::new(),
        diags,
        seen_code: false,
    };
    for line in &lines {
        parser.line(line);
    }
    parser.finish();
    parser.ast
}

/// The names every declaration line declares, lowercase: a local hides a
/// function of the same name.
fn declared_names(lines: &[Line]) -> HashSet<String> {
    lines
        .iter()
        .filter_map(|line| match line.tokens.as_slice() {
            [first, second, ..] => match (&first.tok, &second.tok) {
                (Tok::Ident(kw), Tok::Ident(name)) if decl_type(kw).is_some() => {
                    Some(name.to_ascii_lowercase())
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn decl_type(keyword: &str) -> Option<VarTy> {
    match keyword.to_ascii_lowercase().as_str() {
        "short" | "int" | "long" => Some(VarTy::Int),
        "float" => Some(VarTy::Float),
        "ref" | "reference" => Some(VarTy::Ref),
        _ => None,
    }
}

const KEYWORDS: &[&str] = &[
    "scn",
    "scriptname",
    "begin",
    "end",
    "if",
    "elseif",
    "else",
    "endif",
    "set",
    "to",
    "return",
];

fn is_keyword(word: &str) -> bool {
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(word))
}

struct IfFrame {
    span: Span,
    arms: Vec<(ExprAst, Vec<StmtAst>)>,
    otherwise: Option<Vec<StmtAst>>,
}

struct Parser<'d> {
    mode: Mode,
    locals: HashSet<String>,
    ast: ScriptAst,
    block: Option<(Span, BlockAst)>,
    ifs: Vec<IfFrame>,
    diags: &'d mut Vec<Diag>,
    seen_code: bool,
}

impl Parser<'_> {
    fn err(&mut self, category: Category, span: Span, message: impl Into<String>) {
        self.diags.push(Diag::error(category, message).at(span));
    }

    fn line(&mut self, line: &Line) {
        let Some(first) = line.tokens.first() else {
            // Nothing but characters the language does not use.
            if self.mode == Mode::Result || self.block.is_some() {
                self.diags.extend(line.diags.iter().cloned());
            }
            return;
        };
        let span = first.span;
        let keyword = match &first.tok {
            Tok::Ident(word) => word.to_ascii_lowercase(),
            _ => String::new(),
        };
        // Outside any block of a record only the header, declarations and
        // `begin` mean anything; the source compiler never compiles the
        // rest, so one warning stands for the line and its lexical errors.
        let structural = matches!(keyword.as_str(), "scn" | "scriptname" | "begin" | "end")
            || decl_type(&keyword).is_some();
        if self.mode == Mode::Record && self.block.is_none() && !structural {
            self.seen_code = true;
            self.diags.push(
                Diag::warning(
                    Category::OutsideBlock,
                    "a line outside any block is never run; dropped",
                )
                .at(span),
            );
            return;
        }
        self.diags.extend(line.diags.iter().cloned());
        let rest = &line.tokens[1..];
        match keyword.as_str() {
            "scn" | "scriptname" => self.header(span, rest),
            "begin" => self.begin(span, rest),
            "end" => self.end(span),
            "if" => {
                let cond = self.expression_line(rest, span, "if");
                self.ifs.push(IfFrame {
                    span,
                    arms: vec![(cond, Vec::new())],
                    otherwise: None,
                });
            }
            "elseif" => {
                let cond = self.expression_line(rest, span, "elseif");
                match self.ifs.last_mut() {
                    Some(frame) if frame.otherwise.is_none() => frame.arms.push((cond, Vec::new())),
                    Some(_) => self.err(Category::StrayElse, span, "elseif after else"),
                    None => self.err(Category::StrayElse, span, "elseif without if"),
                }
            }
            "else" => {
                if !rest.is_empty() {
                    self.diags.push(
                        Diag::warning(Category::TrailingTokens, "tokens after else are ignored")
                            .at(rest[0].span),
                    );
                }
                match self.ifs.last_mut() {
                    Some(frame) if frame.otherwise.is_none() => frame.otherwise = Some(Vec::new()),
                    Some(_) => self.err(Category::StrayElse, span, "a second else"),
                    None => self.err(Category::StrayElse, span, "else without if"),
                }
            }
            "endif" => match self.ifs.pop() {
                Some(frame) => self.close_if(frame),
                None => self.err(Category::StrayElse, span, "endif without if"),
            },
            word if decl_type(word).is_some() => self.declaration(span, &keyword, rest),
            _ => {
                let kind = self.statement(&line.tokens);
                self.push_stmt(StmtAst { kind, span });
            }
        }
    }

    fn header(&mut self, span: Span, rest: &[Token]) {
        let name = match rest.first() {
            Some(Token {
                tok: Tok::Ident(text),
                span,
            }) => Some(Name {
                text: text.clone(),
                span: *span,
            }),
            _ => None,
        };
        if self.mode == Mode::Result || self.ast.header.is_some() || self.seen_code {
            self.diags.push(
                Diag::warning(Category::MisplacedHeader, "script name not at the start").at(span),
            );
        }
        match name {
            Some(name) if self.ast.header.is_none() => self.ast.header = Some(name),
            Some(_) => {}
            None => self.err(Category::UnexpectedToken, span, "scn without a name"),
        }
    }

    fn declaration(&mut self, span: Span, keyword: &str, rest: &[Token]) {
        let Some(ty) = decl_type(keyword) else {
            return;
        };
        match rest.first() {
            Some(Token {
                tok: Tok::Ident(text),
                span: name_span,
            }) if !is_keyword(text) => {
                if rest.len() > 1 {
                    self.diags.push(
                        Diag::note(
                            Category::TrailingTokens,
                            "tokens after a declaration are ignored",
                        )
                        .at(rest[1].span),
                    );
                }
                if self.mode == Mode::Result || self.block.is_some() {
                    self.diags.push(
                        Diag::note(
                            Category::MisplacedDeclaration,
                            format!("declaration of {text} inside a block or result script"),
                        )
                        .at(span)
                        .about(text.clone()),
                    );
                }
                self.ast.decls.push(DeclAst {
                    ty,
                    name: Name {
                        text: text.clone(),
                        span: *name_span,
                    },
                });
            }
            _ => self.err(
                Category::BadDeclaration,
                span,
                format!("{keyword} without a variable name"),
            ),
        }
    }

    fn begin(&mut self, span: Span, rest: &[Token]) {
        if self.mode == Mode::Result {
            self.err(
                Category::BlockInResult,
                span,
                "begin in a result script; its statements are kept",
            );
            return;
        }
        if self.block.is_some() {
            self.err(
                Category::MissingEnd,
                span,
                "begin inside a block: the open block ends here",
            );
            self.close_block();
        }
        self.seen_code = true;
        let Some(Token {
            tok: Tok::Ident(text),
            span: name_span,
        }) = rest.first()
        else {
            self.err(Category::BadBlock, span, "begin without a block type");
            // Keep the statements up to the matching end in a nameless
            // block so that they are counted and the end matches.
            self.block = Some((
                span,
                BlockAst {
                    name: Name {
                        text: String::new(),
                        span,
                    },
                    args: Vec::new(),
                    body: Vec::new(),
                },
            ));
            return;
        };
        let mut cursor = Cursor::new(&rest[1..], span);
        let mut args = Vec::new();
        while !cursor.at_end() {
            if cursor.eat(&Tok::Comma) {
                continue;
            }
            match self.arg(&mut cursor, true) {
                Some(arg) => args.push(arg),
                None => {
                    let at = cursor.span();
                    self.err(
                        Category::BadBlock,
                        at,
                        "a block argument that does not parse",
                    );
                    break;
                }
            }
        }
        self.block = Some((
            span,
            BlockAst {
                name: Name {
                    text: text.clone(),
                    span: *name_span,
                },
                args,
                body: Vec::new(),
            },
        ));
    }

    fn end(&mut self, span: Span) {
        if self.block.is_none() {
            if self.mode == Mode::Result {
                self.err(Category::StrayEnd, span, "end in a result script");
            } else {
                self.err(Category::StrayEnd, span, "end without begin");
            }
            return;
        }
        self.close_block();
    }

    fn close_block(&mut self) {
        while let Some(frame) = self.ifs.pop() {
            self.err(Category::MissingEndif, frame.span, "if without endif");
            self.close_if(frame);
        }
        if let Some((_, block)) = self.block.take() {
            self.ast.blocks.push(block);
        }
    }

    fn close_if(&mut self, frame: IfFrame) {
        let stmt = StmtAst {
            kind: StmtKind::If {
                arms: frame.arms,
                otherwise: frame.otherwise.unwrap_or_default(),
            },
            span: frame.span,
        };
        self.push_stmt(stmt);
    }

    fn push_stmt(&mut self, stmt: StmtAst) {
        if let Some(frame) = self.ifs.last_mut() {
            match &mut frame.otherwise {
                Some(otherwise) => otherwise.push(stmt),
                None => {
                    if let Some((_, body)) = frame.arms.last_mut() {
                        body.push(stmt);
                    }
                }
            }
            return;
        }
        match (&mut self.block, self.mode) {
            (Some((_, block)), _) => block.body.push(stmt),
            (None, Mode::Result) => self.ast.body.push(stmt),
            (None, Mode::Record) => {
                self.seen_code = true;
                self.diags.push(
                    Diag::error(
                        Category::OutsideBlock,
                        "a statement outside any block is never run; dropped",
                    )
                    .at(stmt.span),
                );
            }
        }
    }

    fn finish(&mut self) {
        if let Some((span, _)) = &self.block {
            let span = *span;
            self.err(Category::MissingEnd, span, "block without end");
            self.close_block();
        }
        while let Some(frame) = self.ifs.pop() {
            self.err(Category::MissingEndif, frame.span, "if without endif");
            self.close_if(frame);
        }
    }

    /// An `if` or `elseif` condition: one expression to the line's end.
    fn expression_line(&mut self, tokens: &[Token], span: Span, what: &str) -> ExprAst {
        let mut cursor = Cursor::new(tokens, span);
        if cursor.at_end() {
            self.err(
                Category::BadExpression,
                span,
                format!("{what} without a condition; taken as false"),
            );
            return ExprAst::Int(0, span);
        }
        match self.expr(&mut cursor) {
            Some(expr) => {
                if !cursor.at_end() {
                    let at = cursor.span();
                    self.diags.push(
                        Diag::error(
                            Category::TrailingTokens,
                            format!("tokens after the {what} condition are ignored"),
                        )
                        .at(at),
                    );
                }
                expr
            }
            None => {
                let at = cursor.span();
                self.err(
                    Category::BadExpression,
                    at,
                    format!("{what} condition does not parse; taken as false"),
                );
                ExprAst::Int(0, span)
            }
        }
    }

    /// A statement line that is not a keyword line.
    fn statement(&mut self, tokens: &[Token]) -> StmtKind {
        let span = tokens[0].span;
        // `ref.set var to value`, which the source compiler reads as
        // `set ref.var to value`.
        if let [
            reference @ Token {
                tok: Tok::Ident(_), ..
            },
            Token { tok: Tok::Dot, .. },
            Token {
                tok: Tok::Ident(set),
                span: set_span,
            },
            rest @ ..,
        ] = tokens
            && set.eq_ignore_ascii_case("set")
            && matches!(
                rest.first(),
                Some(Token {
                    tok: Tok::Ident(_),
                    ..
                })
            )
        {
            self.diags.push(
                Diag::note(
                    Category::UnexpectedToken,
                    "ref.set var to value read as set ref.var to value",
                )
                .at(*set_span),
            );
            let mut rewritten = vec![
                Token {
                    tok: Tok::Ident("set".to_owned()),
                    span: *set_span,
                },
                reference.clone(),
                Token {
                    tok: Tok::Dot,
                    span: *set_span,
                },
            ];
            rewritten.extend(rest.iter().cloned());
            return self.statement(&rewritten);
        }
        let word = match &tokens[0].tok {
            Tok::Ident(word) => word.to_ascii_lowercase(),
            _ => String::new(),
        };
        let mut cursor = Cursor::new(tokens, span);
        let kind = match word.as_str() {
            "set" => {
                cursor.next();
                self.set(&mut cursor, span)
            }
            "return" => {
                cursor.next();
                Some(StmtKind::Return)
            }
            "to" => {
                self.err(Category::UnexpectedToken, span, "a line starting with to");
                return StmtKind::Invalid;
            }
            "" => {
                self.err(
                    Category::UnexpectedToken,
                    span,
                    "a statement must start with a name",
                );
                return StmtKind::Invalid;
            }
            _ => self.call_statement(&mut cursor).map(StmtKind::Call),
        };
        match kind {
            Some(kind) if cursor.at_end() => kind,
            Some(_) => {
                let at = cursor.span();
                self.err(Category::TrailingTokens, at, "tokens after the statement");
                StmtKind::Invalid
            }
            None => StmtKind::Invalid,
        }
    }

    fn set(&mut self, cursor: &mut Cursor<'_>, span: Span) -> Option<StmtKind> {
        let Some(target) = self.path(cursor) else {
            self.err(Category::BadSet, span, "set without a variable");
            return None;
        };
        match cursor.peek() {
            Some(Tok::Ident(word)) if word.eq_ignore_ascii_case("to") => {
                cursor.next();
            }
            _ => {
                let at = cursor.span();
                self.err(Category::BadSet, at, "set without to");
                return None;
            }
        }
        if cursor.at_end() {
            self.err(Category::BadSet, span, "set without a value");
            return None;
        }
        let Some(value) = self.expr(cursor) else {
            let at = cursor.span();
            self.err(Category::BadExpression, at, "the value does not parse");
            return None;
        };
        Some(StmtKind::Set { target, value })
    }

    fn path(&mut self, cursor: &mut Cursor<'_>) -> Option<Path> {
        let first = cursor.name()?;
        if cursor.peek() == Some(&Tok::Dot)
            && let Some(Tok::Ident(_)) = cursor.peek_at(1)
        {
            cursor.next();
            let member = cursor.name()?;
            return Some(Path::Member(first, member));
        }
        Some(Path::Simple(first))
    }

    /// `[ref.]Function arg ...` to the line's end.
    fn call_statement(&mut self, cursor: &mut Cursor<'_>) -> Option<CallAst> {
        let first = cursor.name()?;
        let (on, name) = if cursor.peek() == Some(&Tok::Dot) {
            cursor.next();
            match cursor.name() {
                Some(name) => (Some(first), name),
                None => {
                    let at = cursor.span();
                    self.err(Category::UnexpectedToken, at, "a name must follow the dot");
                    return None;
                }
            }
        } else {
            (None, first)
        };
        let mut args = Vec::new();
        while !cursor.at_end() {
            if cursor.eat(&Tok::Comma) {
                continue;
            }
            match self.arg(cursor, true) {
                Some(arg) => args.push(arg),
                None => {
                    let at = cursor.span();
                    self.err(
                        Category::UnexpectedToken,
                        at,
                        "an argument that does not parse",
                    );
                    return None;
                }
            }
        }
        Some(CallAst { on, name, args })
    }

    /// One argument: a literal, a path, a parenthesised expression, or
    /// (in a statement) a negated one.
    fn arg(&mut self, cursor: &mut Cursor<'_>, allow_minus: bool) -> Option<ExprAst> {
        let span = cursor.span();
        match cursor.peek()? {
            Tok::Int(v) => {
                let v = *v;
                cursor.next();
                Some(ExprAst::Int(v, span))
            }
            Tok::Float(v) => {
                let v = *v;
                cursor.next();
                Some(ExprAst::Float(v, span))
            }
            Tok::Str(s) => {
                let s = s.clone();
                cursor.next();
                Some(ExprAst::Str(s, span))
            }
            Tok::LParen => {
                cursor.next();
                let inner = self.expr(cursor)?;
                if !cursor.eat(&Tok::RParen) {
                    return None;
                }
                Some(inner)
            }
            Tok::Minus if allow_minus => {
                cursor.next();
                let inner = self.arg(cursor, false)?;
                Some(negate(inner, span))
            }
            Tok::Ident(word) if !is_keyword(word) => self.path(cursor).map(ExprAst::Path),
            _ => None,
        }
    }

    fn expr(&mut self, cursor: &mut Cursor<'_>) -> Option<ExprAst> {
        self.or(cursor)
    }

    fn or(&mut self, cursor: &mut Cursor<'_>) -> Option<ExprAst> {
        let mut lhs = self.and(cursor)?;
        while cursor.peek() == Some(&Tok::OrOr) {
            let span = cursor.span();
            cursor.next();
            let rhs = self.and(cursor)?;
            lhs = ExprAst::Binary(BinOp::Or, Box::new(lhs), Box::new(rhs), span);
        }
        Some(lhs)
    }

    fn and(&mut self, cursor: &mut Cursor<'_>) -> Option<ExprAst> {
        let mut lhs = self.cmp(cursor)?;
        while cursor.peek() == Some(&Tok::AndAnd) {
            let span = cursor.span();
            cursor.next();
            let rhs = self.cmp(cursor)?;
            lhs = ExprAst::Binary(BinOp::And, Box::new(lhs), Box::new(rhs), span);
        }
        Some(lhs)
    }

    fn cmp(&mut self, cursor: &mut Cursor<'_>) -> Option<ExprAst> {
        let mut lhs = self.sum(cursor)?;
        loop {
            let span = cursor.span();
            let op = match cursor.peek() {
                Some(Tok::EqEq) => CmpOp::Eq,
                Some(Tok::Eq) => {
                    self.diags
                        .push(Diag::warning(Category::AssignAsCompare, "= taken as ==").at(span));
                    CmpOp::Eq
                }
                Some(Tok::NotEq) => CmpOp::Ne,
                Some(Tok::Lt) => CmpOp::Lt,
                Some(Tok::Le) => CmpOp::Le,
                Some(Tok::Gt) => CmpOp::Gt,
                Some(Tok::Ge) => CmpOp::Ge,
                _ => return Some(lhs),
            };
            cursor.next();
            let rhs = self.sum(cursor)?;
            lhs = ExprAst::Binary(BinOp::Cmp(op), Box::new(lhs), Box::new(rhs), span);
        }
    }

    fn sum(&mut self, cursor: &mut Cursor<'_>) -> Option<ExprAst> {
        let mut lhs = self.term(cursor)?;
        loop {
            let span = cursor.span();
            let op = match cursor.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => return Some(lhs),
            };
            cursor.next();
            let rhs = self.term(cursor)?;
            lhs = ExprAst::Binary(op, Box::new(lhs), Box::new(rhs), span);
        }
    }

    fn term(&mut self, cursor: &mut Cursor<'_>) -> Option<ExprAst> {
        let mut lhs = self.unary(cursor)?;
        loop {
            let span = cursor.span();
            let op = match cursor.peek() {
                Some(Tok::Star) => BinOp::Mul,
                Some(Tok::Slash) => BinOp::Div,
                Some(Tok::Percent) => BinOp::Mod,
                _ => return Some(lhs),
            };
            cursor.next();
            let rhs = self.unary(cursor)?;
            lhs = ExprAst::Binary(op, Box::new(lhs), Box::new(rhs), span);
        }
    }

    fn unary(&mut self, cursor: &mut Cursor<'_>) -> Option<ExprAst> {
        let span = cursor.span();
        match cursor.peek() {
            Some(Tok::Minus) => {
                cursor.next();
                let inner = self.unary(cursor)?;
                Some(negate(inner, span))
            }
            Some(Tok::Bang) => {
                cursor.next();
                let inner = self.unary(cursor)?;
                Some(ExprAst::Unary(UnOp::Not, Box::new(inner), span))
            }
            _ => self.atom(cursor),
        }
    }

    fn atom(&mut self, cursor: &mut Cursor<'_>) -> Option<ExprAst> {
        match cursor.peek()? {
            Tok::Int(_) | Tok::Float(_) | Tok::Str(_) | Tok::LParen => self.arg(cursor, false),
            Tok::Ident(word) if !is_keyword(word) => {
                let first = cursor.name()?;
                // `a.b`: a call when b is a function, when a is the player,
                // or when an argument follows; else a member variable.
                if cursor.peek() == Some(&Tok::Dot)
                    && let Some(Tok::Ident(member)) = cursor.peek_at(1)
                {
                    let player = matches!(first.key().as_str(), "player" | "playerref");
                    let call = functions::lookup(member).is_some()
                        || player
                        || starts_arg(cursor.peek_at(2));
                    cursor.next();
                    let member = cursor.name()?;
                    if call {
                        let args = self.expression_args(cursor, &member);
                        return Some(ExprAst::Call(CallAst {
                            on: Some(first),
                            name: member,
                            args,
                        }));
                    }
                    return Some(ExprAst::Path(Path::Member(first, member)));
                }
                let key = first.key();
                if self.locals.contains(&key) {
                    return Some(ExprAst::Path(Path::Simple(first)));
                }
                if functions::lookup(&key).is_some() || starts_arg(cursor.peek()) {
                    let args = self.expression_args(cursor, &first);
                    return Some(ExprAst::Call(CallAst {
                        on: None,
                        name: first,
                        args,
                    }));
                }
                Some(ExprAst::Path(Path::Simple(first)))
            }
            _ => None,
        }
    }

    /// The arguments of a call inside an expression.
    fn expression_args(&mut self, cursor: &mut Cursor<'_>, name: &Name) -> Vec<ExprAst> {
        let limit = functions::lookup(&name.text).map_or(usize::MAX, |sig| sig.max());
        let mut args = Vec::new();
        while args.len() < limit {
            if args.is_empty() {
                // No comma before the first argument.
            } else if cursor.peek() == Some(&Tok::Comma) {
                cursor.next();
            }
            if !starts_arg(cursor.peek()) {
                break;
            }
            match self.arg(cursor, false) {
                Some(arg) => args.push(arg),
                None => break,
            }
        }
        args
    }
}

/// Whether a token can start an argument of a call inside an expression.
fn starts_arg(tok: Option<&Tok>) -> bool {
    match tok {
        Some(Tok::Int(_) | Tok::Float(_) | Tok::Str(_)) => true,
        Some(Tok::Ident(word)) => !is_keyword(word),
        _ => false,
    }
}

fn negate(inner: ExprAst, span: Span) -> ExprAst {
    match inner {
        ExprAst::Int(v, _) => ExprAst::Int(-v, span),
        ExprAst::Float(v, _) => ExprAst::Float(-v, span),
        other => ExprAst::Unary(UnOp::Neg, Box::new(other), span),
    }
}

/// A position in one line's tokens.
struct Cursor<'t> {
    tokens: &'t [Token],
    pos: usize,
    /// Where the line starts, for a position at its end.
    line: Span,
}

impl<'t> Cursor<'t> {
    fn new(tokens: &'t [Token], line: Span) -> Self {
        Self {
            tokens,
            pos: 0,
            line,
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn peek(&self) -> Option<&'t Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }

    fn peek_at(&self, ahead: usize) -> Option<&'t Tok> {
        self.tokens.get(self.pos + ahead).map(|t| &t.tok)
    }

    fn span(&self) -> Span {
        match self.tokens.get(self.pos) {
            Some(t) => t.span,
            None => self
                .tokens
                .last()
                .map_or(self.line, |t| Span::new(t.span.line, t.span.column + 1)),
        }
    }

    fn next(&mut self) {
        self.pos += 1;
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.peek() == Some(tok) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn name(&mut self) -> Option<Name> {
        match self.tokens.get(self.pos) {
            Some(Token {
                tok: Tok::Ident(text),
                span,
            }) if !is_keyword(text) => {
                self.pos += 1;
                Some(Name {
                    text: text.clone(),
                    span: *span,
                })
            }
            _ => None,
        }
    }
}
