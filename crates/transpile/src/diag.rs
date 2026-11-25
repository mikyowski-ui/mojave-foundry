//! Diagnostics: what the front end found, with a position in a script's
//! source where it has one, and a category the report counts. Errors are
//! data: nothing in the front end panics on malformed input.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ir::SimId;

/// A position in a script's source: line and column, from 1. The column
/// counts characters.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Span {
    /// Line, from 1.
    pub line: u32,
    /// Column, from 1.
    pub column: u32,
}

impl Span {
    /// A position.
    pub fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// How bad it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Something was lost: a statement, a block, a condition term.
    Error,
    /// Something was approximated.
    Warning,
    /// Worth counting, nothing lost.
    Note,
}

/// What kind of finding; the report groups by [`Category::group`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    // ---- lexer
    /// A character the language does not use.
    BadCharacter,
    /// A string with no closing quote on its line.
    UnterminatedString,
    /// A number that does not parse.
    BadNumber,
    // ---- parser
    /// A token where the grammar wants another.
    UnexpectedToken,
    /// Tokens left after a complete statement.
    TrailingTokens,
    /// An expression that does not parse.
    BadExpression,
    /// A `set` without `to`, or without a variable.
    BadSet,
    /// A declaration without a name.
    BadDeclaration,
    /// A declaration inside a block, or in a result script.
    MisplacedDeclaration,
    /// A `begin` without a block type, or nested in a block.
    BadBlock,
    /// A block with no `end`.
    MissingEnd,
    /// An `end` with no `begin`.
    StrayEnd,
    /// An `if` with no `endif`.
    MissingEndif,
    /// An `elseif`, `else` or `endif` with no `if`.
    StrayElse,
    /// A statement outside any block of a script record.
    OutsideBlock,
    /// A `begin` in a result script.
    BlockInResult,
    /// A `scn` that is not the first line, or a second one.
    MisplacedHeader,
    /// `=` used as a comparison.
    AssignAsCompare,
    // ---- names
    /// A name that is no variable, form or function.
    UnresolvedName,
    /// `quest.var` where the quest has no such variable.
    UnresolvedQuestVar,
    /// `ref.var` where the reference's script has no such variable, or
    /// the reference has no script.
    UnresolvedRefVar,
    /// A name with several forms, none on the script's reference list.
    AmbiguousName,
    /// A function called on something that is not a reference.
    CallOnNonReference,
    /// A base actor used as a reference, taken as its only placed
    /// reference.
    BaseAsReference,
    /// `set` on something that is not a variable.
    BadAssignment,
    // ---- functions
    /// A function the signature table does not have.
    UnknownFunction,
    /// A function the table lists as dropped in Pass 2.
    DroppedFunction,
    /// A function given too few or too many arguments.
    ArgumentCount,
    /// An argument of the wrong kind.
    ArgumentKind,
    // ---- blocks and statements
    /// A block type Pass 2 does not run (spec 5.3).
    UnsupportedBlock,
    /// A block whose statements are all unsupported calls.
    EmptyBlock,
    // ---- conditions
    /// A condition term taken with a fallback value (spec 4.9).
    ConditionFallback,
    /// A condition function with no table entry.
    ConditionUnknownFunction,
    /// A condition parameter that names no usable form.
    ConditionBadParameter,
    /// A condition run on something Pass 2 cannot name (combat target).
    ConditionRunOn,
    /// A comparison code outside 0 to 5.
    ConditionComparison,
    // ---- build
    /// A form the build needs is not in the MIR.
    MissingRecord,
    /// A reference named by the slice's code that is not placed in it.
    RefOutsideSlice,
    /// A source flag Pass 2 ignores by owner decision (essential).
    IgnoredFlag,
    /// The quest closure stopped at its depth.
    QuestDepth,
}

impl Category {
    /// The report group: `lex`, `parse`, `name`, `function`, `block`,
    /// `condition` or `build`.
    pub fn group(self) -> &'static str {
        use Category::*;
        match self {
            BadCharacter | UnterminatedString | BadNumber => "lex",
            UnexpectedToken | TrailingTokens | BadExpression | BadSet | BadDeclaration
            | MisplacedDeclaration | BadBlock | MissingEnd | StrayEnd | MissingEndif
            | StrayElse | OutsideBlock | BlockInResult | MisplacedHeader | AssignAsCompare => {
                "parse"
            }
            UnresolvedName | UnresolvedQuestVar | UnresolvedRefVar | AmbiguousName
            | CallOnNonReference | BaseAsReference | BadAssignment => "name",
            UnknownFunction | DroppedFunction | ArgumentCount | ArgumentKind => "function",
            UnsupportedBlock | EmptyBlock => "block",
            ConditionFallback
            | ConditionUnknownFunction
            | ConditionBadParameter
            | ConditionRunOn
            | ConditionComparison => "condition",
            MissingRecord | RefOutsideSlice | IgnoredFlag | QuestDepth => "build",
        }
    }

    /// The category's name as serialised.
    pub fn name(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Diag {
    /// How bad.
    pub severity: Severity,
    /// What kind.
    pub category: Category,
    /// What it is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<SimId>,
    /// Where in the owner's source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Span>,
    /// The name, function or token concerned, when there is one: the key
    /// the report counts by.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    /// A sentence for a reader.
    pub message: String,
}

impl Diag {
    /// A finding with no owner and no position.
    pub fn new(severity: Severity, category: Category, message: impl Into<String>) -> Self {
        Self {
            severity,
            category,
            owner: None,
            at: None,
            subject: String::new(),
            message: message.into(),
        }
    }

    /// An error.
    pub fn error(category: Category, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, category, message)
    }

    /// A warning.
    pub fn warning(category: Category, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, category, message)
    }

    /// A note.
    pub fn note(category: Category, message: impl Into<String>) -> Self {
        Self::new(Severity::Note, category, message)
    }

    /// Sets the owner.
    #[must_use]
    pub fn owned_by(mut self, owner: &SimId) -> Self {
        self.owner = Some(owner.clone());
        self
    }

    /// Sets the position.
    #[must_use]
    pub fn at(mut self, span: Span) -> Self {
        self.at = Some(span);
        self
    }

    /// Sets the subject.
    #[must_use]
    pub fn about(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        write!(f, "{severity}[{}]", self.category.name())?;
        if let Some(owner) = &self.owner {
            write!(f, " {owner}")?;
        }
        if let Some(at) = self.at {
            write!(f, " at {at}")?;
        }
        write!(f, ": {}", self.message)
    }
}
