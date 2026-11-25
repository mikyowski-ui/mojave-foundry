//! Static evaluation and simplification of conditions.
//!
//! [`fold`] decides the terms spec 4.2 allows at build time, on the
//! subject only: `GetIsID`, `GetIsVoiceType`, `GetIsSex`, `GetIsRace`,
//! `GetIsClass`, `GetIsCreature` from the base record; `GetInFaction` from
//! the base's factions unless a script changes that faction for the base;
//! `GetIsReference` when the base has exactly one placement; and
//! `IsInInterior` from the map. A term whose fact is unknown stays.
//!
//! [`simplify`] applies the Boolean identities. A [`Cond::Fallback`] leaf
//! is true and is kept, so that it stays visible, unless the conjunction
//! or disjunction it is in is decided by another term.

use crate::ir::{Call, CmpOp, Cond, Expr, FormKey, Func, RefExpr};

/// What is known about the subject of a condition: the speaker of a
/// dialogue, the owner of a package.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubjectFacts {
    /// The base record.
    pub base: FormKey,
    /// Its voice type.
    pub voice: Option<FormKey>,
    /// Female.
    pub female: bool,
    /// Its race.
    pub race: Option<FormKey>,
    /// Its class.
    pub class: Option<FormKey>,
    /// A creature.
    pub creature: bool,
    /// Its factions.
    pub factions: Vec<FormKey>,
    /// Factions some script adds or removes for this base: a runtime term.
    pub dynamic_factions: Vec<FormKey>,
    /// Its one placed reference, when it has exactly one.
    pub only_reference: Option<FormKey>,
}

/// What [`fold`] may use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facts {
    /// The subject.
    pub subject: Option<SubjectFacts>,
    /// Whether the map is an interior.
    pub interior: Option<bool>,
}

fn on_subject(on: &Option<RefExpr>) -> bool {
    matches!(on, None | Some(RefExpr::SelfRef))
}

fn first_form(call: &Call) -> Option<&FormKey> {
    match call.args.first() {
        Some(Expr::Form(form)) => Some(form),
        Some(Expr::Ref(RefExpr::Placed(form))) => Some(form),
        _ => None,
    }
}

fn first_int(call: &Call) -> Option<i32> {
    match call.args.first() {
        Some(Expr::Int(v)) => Some(*v),
        _ => None,
    }
}

/// The value of a static call, when the facts decide it.
fn static_value(call: &Call, facts: &Facts) -> Option<f64> {
    let Func::Known(id) = &call.func else {
        return None;
    };
    let truth = |b: bool| Some(if b { 1.0 } else { 0.0 });
    if id.name() == "IsInInterior" {
        return facts.interior.and_then(truth);
    }
    if !on_subject(&call.on) {
        return None;
    }
    let subject = facts.subject.as_ref()?;
    match id.name() {
        "GetIsID" => truth(first_form(call)? == &subject.base),
        "GetIsVoiceType" => truth(Some(first_form(call)?) == subject.voice.as_ref()),
        "GetIsRace" => truth(Some(first_form(call)?) == subject.race.as_ref()),
        "GetIsClass" => truth(Some(first_form(call)?) == subject.class.as_ref()),
        "GetIsSex" => truth((first_int(call)? == 1) == subject.female),
        "GetIsCreature" => truth(subject.creature),
        "GetInFaction" => {
            let faction = first_form(call)?;
            if subject.dynamic_factions.contains(faction) {
                None
            } else {
                truth(subject.factions.contains(faction))
            }
        }
        "GetIsReference" => {
            let only = subject.only_reference.as_ref()?;
            truth(first_form(call)? == only)
        }
        _ => None,
    }
}

fn literal(expr: &Expr) -> Option<f64> {
    match expr {
        Expr::Int(v) => Some(f64::from(*v)),
        Expr::Float(v) => Some(f64::from(*v)),
        _ => None,
    }
}

/// Folds the terms the facts decide, then simplifies.
pub fn fold(cond: &Cond, facts: &Facts) -> Cond {
    let folded = match cond {
        Cond::Not(inner) => Cond::Not(Box::new(fold(inner, facts))),
        Cond::All(items) => Cond::All(items.iter().map(|c| fold(c, facts)).collect()),
        Cond::Any(items) => Cond::Any(items.iter().map(|c| fold(c, facts)).collect()),
        Cond::Cmp { lhs, op, rhs } => {
            let value = match lhs {
                Expr::Call(call) => static_value(call, facts),
                _ => None,
            };
            match (value, literal(rhs)) {
                (Some(lhs), Some(rhs)) => {
                    if op.holds(lhs, rhs) {
                        Cond::True
                    } else {
                        Cond::False
                    }
                }
                _ => cond.clone(),
            }
        }
        other => other.clone(),
    };
    simplify(&folded)
}

fn holds(lhs: &Expr, op: CmpOp, rhs: &Expr) -> Option<bool> {
    Some(op.holds(literal(lhs)?, literal(rhs)?))
}

/// Applies the Boolean identities: decided comparisons of literals, double
/// negation, true and false in conjunctions and disjunctions, nested
/// conjunctions and disjunctions flattened, one-element lists unwrapped.
pub fn simplify(cond: &Cond) -> Cond {
    match cond {
        Cond::Cmp { lhs, op, rhs } => match holds(lhs, *op, rhs) {
            Some(true) => Cond::True,
            Some(false) => Cond::False,
            None => cond.clone(),
        },
        Cond::Not(inner) => match simplify(inner) {
            Cond::True => Cond::False,
            Cond::False => Cond::True,
            Cond::Not(x) => *x,
            other => Cond::Not(Box::new(other)),
        },
        Cond::All(items) => {
            let mut out = Vec::new();
            for item in items {
                match simplify(item) {
                    Cond::True => {}
                    Cond::False => return Cond::False,
                    Cond::All(inner) => out.extend(inner),
                    other => out.push(other),
                }
            }
            if out.iter().any(is_failed_fallback) {
                return Cond::False;
            }
            match out.len() {
                0 => Cond::True,
                1 => out.remove(0),
                _ => Cond::All(out),
            }
        }
        Cond::Any(items) => {
            let mut out = Vec::new();
            for item in items {
                match simplify(item) {
                    Cond::False => {}
                    Cond::True => return Cond::True,
                    Cond::Any(inner) => out.extend(inner),
                    other => out.push(other),
                }
            }
            out.retain(|c| !is_failed_fallback(c));
            if let Some(leaf) = out.iter().find(|c| matches!(c, Cond::Fallback { .. })) {
                return leaf.clone();
            }
            match out.len() {
                0 => Cond::False,
                1 => out.remove(0),
                _ => Cond::Any(out),
            }
        }
        Cond::True | Cond::False | Cond::Fallback { .. } => cond.clone(),
    }
}

fn is_failed_fallback(cond: &Cond) -> bool {
    matches!(cond, Cond::Not(inner) if matches!(**inner, Cond::Fallback { .. }))
}
