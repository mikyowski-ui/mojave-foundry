//! Expressions and conditions (contracts 11.1, spec 3.3).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::id::{FormKey, SimId, TextId};

/// A value expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expr {
    /// An integer literal.
    Int(i32),
    /// A float literal.
    Float(f32),
    /// A string literal of a script.
    Str(TextId),
    /// A variable.
    Var(VarRef),
    /// A base form: becomes a PID, a message id, ...
    Form(FormKey),
    /// A reference.
    Ref(RefExpr),
    /// A function call.
    Call(Box<Call>),
    /// A unary operation.
    Unary(UnOp, Box<Expr>),
    /// A binary operation.
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

/// A reference-valued expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefExpr {
    /// The player (`player`, `playerref`); a condition's target.
    Player,
    /// The object running the script; a condition's subject.
    SelfRef,
    /// The action reference (`GetActionRef`, `IsActionRef`).
    Action,
    /// A placed reference, by form.
    Placed(FormKey),
    /// A `ref` variable.
    Var(VarRef),
    /// The linked reference of another reference (`XLKR`).
    Linked(Box<RefExpr>),
}

/// A function call, on a reference or on the running object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// The function.
    pub func: Func,
    /// What it runs on; `None` is the running object (or no object, for a
    /// function that needs none).
    pub on: Option<RefExpr>,
    /// The arguments, in source order.
    pub args: Vec<Expr>,
}

/// The function of a [`Call`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Func {
    /// A function of the signature table that Pass 2 compiles or folds.
    Known(FuncId),
    /// Anything else: a function the table drops, or one it does not know.
    /// Code generation drops it (statements) or reads 0 (expressions);
    /// the report counts it.
    Unsupported {
        /// The name as written (or the table's name, when known).
        name: String,
    },
}

/// A function of [`crate::obscript::functions::TABLE`], by its canonical
/// name (as the public function list spells it, for example `GetStage`).
///
/// Serialised as that name; reading a name the table does not have fails,
/// so a snapshot always names a function this build knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FuncId(&'static str);

impl FuncId {
    /// The id of a table function, by name (case does not matter); `None`
    /// when the table does not have it.
    pub fn named(name: &str) -> Option<Self> {
        crate::obscript::functions::lookup(name).map(|sig| Self(sig.name))
    }

    /// The canonical name.
    pub fn name(self) -> &'static str {
        self.0
    }

    /// The table row.
    pub fn signature(self) -> &'static crate::obscript::functions::Signature {
        crate::obscript::functions::lookup(self.0)
            .unwrap_or_else(|| unreachable!("a FuncId is made only from the table"))
    }

    pub(crate) const fn from_table(name: &'static str) -> Self {
        Self(name)
    }
}

impl fmt::Display for FuncId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for FuncId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for FuncId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::named(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown function {text:?}")))
    }
}

/// A variable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VarRef {
    /// An object-script variable.
    Local {
        /// The script ([`super::SimKind::Script`]).
        script: SimId,
        /// The variable's index, as declared (`SLSD`).
        index: u32,
        /// Its type.
        ty: VarTy,
    },
    /// A quest-script variable.
    Quest {
        /// The quest ([`super::SimKind::Quest`]).
        quest: SimId,
        /// The variable's index in the quest's script.
        index: u32,
        /// Its type.
        ty: VarTy,
    },
    /// A `GLOB` ([`super::SimKind::Global`]).
    Global(SimId),
    /// An object-script variable read or written from outside its script
    /// (`ref.var`): stored in a campaign global (spec 5.4).
    Hoisted {
        /// The placed reference whose script holds it.
        owner: FormKey,
        /// That script.
        script: SimId,
        /// The variable's index.
        index: u32,
        /// Its type.
        ty: VarTy,
    },
    /// A procedure-local temporary.
    Temp(u32),
}

/// A variable's type. `short`, `int` and `long` are [`VarTy::Int`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VarTy {
    /// An integer.
    Int,
    /// A float: fixed point times 1,000 where it is stored (spec 5.4).
    Float,
    /// A reference: its slot number where it is stored (spec 5.8).
    Ref,
}

/// A unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnOp {
    /// `-x`
    Neg,
    /// `!x`: 1 when `x` is 0, else 0.
    Not,
}

/// A binary operator of an expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`, which the spec's grammar does not list; kept because the
    /// source compiler accepts it.
    Mod,
    /// `&&`: 1 when both are non-zero.
    And,
    /// `||`: 1 when either is non-zero.
    Or,
    /// A comparison: 1 when it holds, else 0.
    Cmp(CmpOp),
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmpOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl CmpOp {
    /// Parses a condition's comparison text (`mir::ConditionRow`).
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "==" => Self::Eq,
            "!=" => Self::Ne,
            "<" => Self::Lt,
            "<=" => Self::Le,
            ">" => Self::Gt,
            ">=" => Self::Ge,
            _ => return None,
        })
    }

    /// The comparison as text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }

    /// Evaluates the comparison on two numbers.
    pub fn holds(self, lhs: f64, rhs: f64) -> bool {
        match self {
            Self::Eq => lhs == rhs,
            Self::Ne => lhs != rhs,
            Self::Lt => lhs < rhs,
            Self::Le => lhs <= rhs,
            Self::Gt => lhs > rhs,
            Self::Ge => lhs >= rhs,
        }
    }

    /// The comparison that holds exactly when this one does not.
    pub fn negated(self) -> Self {
        match self {
            Self::Eq => Self::Ne,
            Self::Ne => Self::Eq,
            Self::Lt => Self::Ge,
            Self::Le => Self::Gt,
            Self::Gt => Self::Le,
            Self::Ge => Self::Lt,
        }
    }
}

/// A condition: a `CTDA` list, a package's or a stage entry's conditions.
///
/// **Fallbacks** (spec 4.9). A term whose function Pass 2 cannot evaluate
/// takes the function's fallback value, and its comparison is evaluated
/// with that value. When the comparison value is a literal the result is
/// known at build time and the term becomes a [`Cond::Fallback`] leaf when
/// it holds, or `Not(Fallback)` when it does not: a `Fallback` leaf is
/// **true**. It stays in the tree, so that every consumer can see it,
/// unless the conjunction or disjunction it is in is decided (a failed
/// fallback in a conjunction makes it false). When the comparison value is
/// a `GLOB`, the term is `Cmp { lhs: Float(value), .. }`. Every fallback
/// also writes a diagnostic, which is what the report counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cond {
    /// Always holds.
    True,
    /// Never holds.
    False,
    /// Negation.
    Not(Box<Cond>),
    /// Conjunction; empty is true.
    All(Vec<Cond>),
    /// Disjunction; empty is false.
    Any(Vec<Cond>),
    /// A comparison of two values.
    Cmp {
        /// Left side: usually the function's value.
        lhs: Expr,
        /// The operator.
        op: CmpOp,
        /// Right side: the comparison value.
        rhs: Expr,
    },
    /// A term evaluated at build time with a fallback value; true (see
    /// above).
    Fallback {
        /// The function index.
        function: u16,
        /// The function's name, or `"(index n)"`.
        name: String,
        /// The value the function was taken to return.
        value: f32,
        /// Whether the report lists it as a loss; false only for the
        /// functions whose fallback is the designed answer (spec 4.8).
        report: bool,
    },
}
