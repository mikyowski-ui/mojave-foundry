use super::*;
use crate::ir::{CmpOp, VarDecl};
use crate::obscript::functions as sigs;

fn key(id: u32) -> FormKey {
    FormKey::new("test.esm", id)
}

fn owner() -> SimId {
    SimId::of(SimKind::Info, &key(0x900))
}

fn names() -> NameTable {
    let mut names = NameTable::new();
    names.set_quest_script(key(0x10), key(0x11));
    names.set_script_vars(
        key(0x11),
        vec![VarDecl {
            index: 3,
            name: "fTimer".into(),
            ty: VarTy::Float,
        }],
    );
    names.set_ref_base(key(0x20), key(0x21));
    names.set_base_script(key(0x21), key(0x22));
    names.set_script_vars(
        key(0x22),
        vec![VarDecl {
            index: 1,
            name: "done".into(),
            ty: VarTy::Int,
        }],
    );
    names
}

fn lower_one(condition: SourceCondition) -> (Cond, CondUses) {
    let mut uses = CondUses::default();
    let cond = lower(&[condition], &owner(), &names(), &mut uses);
    (cond, uses)
}

/// A condition for a table row, with plausible parameters: a form for
/// every form parameter, actor value 43 (Speech), 1 for an integer.
fn sample(row: &CondFunction, value: f64) -> SourceCondition {
    let mut c = SourceCondition::literal(row.index, "==", value).with_params(1, 3);
    if let Some(sig) = sigs::lookup(row.name)
        && let Some(first) = sig.args.first()
    {
        match first {
            Arg::ActorValue => c.param1 = 43,
            Arg::Int | Arg::Number | Arg::Var | Arg::Any => {}
            _ => c.param1_form = Some(key(0x10)),
        }
    }
    if row.index == 53 {
        c.param1_form = Some(key(0x20));
        c.param2 = 1;
    }
    c
}

fn call_name(expr: &Expr) -> Option<&'static str> {
    match expr {
        Expr::Call(call) => match &call.func {
            Func::Known(id) => Some(id.name()),
            Func::Unsupported { .. } => None,
        },
        _ => None,
    }
}

/// Every row of spec 4.8's table (and the additions) lowers by its class.
#[test]
fn every_table_function_lowers_by_its_class() {
    for row in functions::TABLE {
        match row.lowering {
            Lowering::Runtime | Lowering::Static => {
                let (cond, uses) = lower_one(sample(row, 1.0));
                let Cond::Cmp { lhs, op, rhs } = &cond else {
                    panic!("{}: {cond:?}", row.name);
                };
                assert_eq!(call_name(lhs), Some(row.name), "{}", row.name);
                assert_eq!(*op, CmpOp::Eq);
                assert_eq!(*rhs, Expr::Int(1));
                let Expr::Call(call) = lhs else {
                    unreachable!()
                };
                assert_eq!(call.on, Some(RefExpr::SelfRef), "{}", row.name);
                let sig = sigs::lookup(row.name).unwrap();
                assert_eq!(call.args.len(), sig.max(), "{}", row.name);
                assert!(uses.diags.is_empty(), "{}: {:?}", row.name, uses.diags);
            }
            Lowering::Variable => {
                let (cond, _) = lower_one(sample(row, 1.0));
                let Cond::Cmp { lhs, .. } = &cond else {
                    panic!("{}: {cond:?}", row.name);
                };
                assert!(matches!(lhs, Expr::Var(_)), "{}: {lhs:?}", row.name);
            }
            Lowering::ActorValue => {
                let (cond, _) = lower_one(sample(row, 50.0));
                let Cond::Cmp { lhs, rhs, .. } = &cond else {
                    panic!("{}: {cond:?}", row.name);
                };
                assert_eq!(call_name(lhs), Some(row.name));
                assert_eq!(*rhs, Expr::Int(50));
            }
            Lowering::FormList => {
                // With no list in the name table: a reported fallback.
                let (cond, uses) = lower_one(sample(row, 1.0));
                assert!(
                    matches!(&cond, Cond::Not(inner) if matches!(**inner, Cond::Fallback { .. })),
                    "{}: {cond:?}",
                    row.name
                );
                assert!(!uses.diags.is_empty());
            }
            Lowering::Fallback { value, report } => {
                let (held, uses) = lower_one(sample(row, f64::from(value)));
                assert_eq!(
                    held,
                    Cond::Fallback {
                        function: row.index,
                        name: row.name.to_owned(),
                        value,
                        report
                    },
                    "{}",
                    row.name
                );
                assert_eq!(uses.diags.len(), 1);
                assert_eq!(uses.diags[0].category, Category::ConditionFallback);
                let (failed, _) = lower_one(sample(row, f64::from(value) + 1.0));
                assert!(
                    matches!(&failed, Cond::Not(inner) if matches!(**inner, Cond::Fallback { .. })),
                    "{}: {failed:?}",
                    row.name
                );
            }
        }
    }
}

#[test]
fn variables_read_quests_globals_and_hoisted_scripts() {
    let quest = lower_one(
        SourceCondition::literal(79, ">=", 2.0)
            .with_form(key(0x10))
            .with_params(0x10, 3),
    )
    .0;
    assert_eq!(
        quest,
        Cond::Cmp {
            lhs: Expr::Var(VarRef::Quest {
                quest: SimId::of(SimKind::Quest, &key(0x10)),
                index: 3,
                ty: VarTy::Float
            }),
            op: CmpOp::Ge,
            rhs: Expr::Int(2)
        }
    );
    let (global, uses) = lower_one(SourceCondition::literal(74, "==", 1.0).with_form(key(0x30)));
    assert!(matches!(
        global,
        Cond::Cmp {
            lhs: Expr::Var(VarRef::Global(_)),
            ..
        }
    ));
    assert!(uses.globals.contains(&key(0x30)));
    let mut script = SourceCondition::literal(53, "==", 1.0).with_params(0x20, 1);
    script.param1_form = Some(key(0x20));
    let (hoisted, uses) = lower_one(script);
    assert!(matches!(
        hoisted,
        Cond::Cmp {
            lhs: Expr::Var(VarRef::Hoisted { index: 1, .. }),
            ..
        }
    ));
    assert!(uses.refs.contains(&key(0x20)));
    // Without a reference whose script is known the call stays.
    let (call, _) = lower_one(SourceCondition::literal(53, "==", 1.0).with_params(0, 1));
    assert!(matches!(
        call,
        Cond::Cmp {
            lhs: Expr::Call(_),
            ..
        }
    ));
}

#[test]
fn actor_values_follow_spec_4_7() {
    let body_part = lower_one(SourceCondition::literal(14, "==", 100.0).with_params(26, 0)).0;
    assert!(matches!(
        body_part,
        Cond::Fallback {
            value: 100.0,
            report: false,
            ..
        }
    ));
    let unmapped = lower_one(SourceCondition::literal(14, ">", 0.0).with_params(54, 0)).0;
    assert!(matches!(unmapped, Cond::Not(_)), "{unmapped:?}");
    for av in [5, 11, 16, 23, 32, 45] {
        let cond = lower_one(SourceCondition::literal(14, ">=", 1.0).with_params(av, 0)).0;
        assert!(matches!(cond, Cond::Cmp { .. }), "{av}: {cond:?}");
    }
}

#[test]
fn or_runs_are_anded() {
    // [A or, B, C or, D] is (A or B) and (C or D).
    let t = |f: u16| SourceCondition::literal(f, "==", 1.0);
    let mut uses = CondUses::default();
    let cond = lower(
        &[t(50).or(), t(77), t(18).or(), t(289)],
        &owner(),
        &names(),
        &mut uses,
    );
    let Cond::All(runs) = cond else {
        panic!("{cond:?}")
    };
    assert_eq!(runs.len(), 2);
    assert!(
        runs.iter()
            .all(|r| matches!(r, Cond::Any(items) if items.len() == 2))
    );
    // A trailing OR closes its run; no conditions is true.
    let cond = lower(&[t(50), t(77).or()], &owner(), &names(), &mut uses);
    assert!(matches!(cond, Cond::All(ref items) if items.len() == 2));
    assert_eq!(lower(&[], &owner(), &names(), &mut uses), Cond::True);
}

#[test]
fn run_on_becomes_the_calls_target() {
    let on = |run_on: u32, reference: Option<FormKey>| {
        let (cond, uses) = lower_one(SourceCondition::literal(46, "==", 1.0).on(run_on, reference));
        (cond, uses)
    };
    let target = |cond: &Cond| match cond {
        Cond::Cmp {
            lhs: Expr::Call(call),
            ..
        } => call.on.clone(),
        _ => None,
    };
    assert_eq!(target(&on(1, None).0), Some(RefExpr::Player));
    let (reference, uses) = on(2, Some(key(0x40)));
    assert_eq!(target(&reference), Some(RefExpr::Placed(key(0x40))));
    assert!(uses.refs.contains(&key(0x40)));
    assert_eq!(
        target(&on(4, None).0),
        Some(RefExpr::Linked(Box::new(RefExpr::SelfRef)))
    );
    let (combat, uses) = on(3, None);
    assert!(matches!(combat, Cond::Not(_)));
    assert_eq!(uses.diags[0].category, Category::ConditionRunOn);
    let (missing, uses) = on(2, None);
    assert!(matches!(missing, Cond::Not(_)));
    assert_eq!(uses.diags[0].category, Category::ConditionBadParameter);
}

#[test]
fn unknown_functions_and_comparisons_are_reported() {
    let (unknown, uses) = lower_one(SourceCondition::literal(9999, "==", 0.0));
    assert!(matches!(
        unknown,
        Cond::Fallback {
            value: 0.0,
            report: true,
            ..
        }
    ));
    assert_eq!(uses.diags[0].category, Category::ConditionUnknownFunction);
    let (reserved, uses) = lower_one(SourceCondition::literal(46, "6", 1.0));
    assert_eq!(reserved, Cond::False);
    assert_eq!(uses.diags[0].category, Category::ConditionComparison);
    // A form parameter that names nothing: a fallback, reported.
    let (bad, uses) = lower_one(SourceCondition::literal(72, "==", 1.0));
    assert!(matches!(bad, Cond::Not(_)));
    assert_eq!(uses.diags[0].category, Category::ConditionBadParameter);
}

#[test]
fn global_comparison_values_stay_runtime() {
    let mut c = SourceCondition::literal(58, ">=", 0.0).with_form(key(0x10));
    c.value = CondValue::Global(key(0x31));
    let (cond, uses) = lower_one(c);
    assert!(matches!(
        cond,
        Cond::Cmp {
            rhs: Expr::Var(VarRef::Global(_)),
            ..
        }
    ));
    assert!(uses.globals.contains(&key(0x31)));
    // A fallback against a global keeps its value as the left side.
    let mut f = SourceCondition::literal(449, "==", 0.0).with_form(key(0x10));
    f.value = CondValue::Global(key(0x31));
    let (cond, _) = lower_one(f);
    assert!(matches!(cond, Cond::Cmp { lhs: Expr::Float(v), .. } if v == 0.0));
}

fn speaker() -> Facts {
    Facts {
        subject: Some(SubjectFacts {
            base: key(0x50),
            voice: Some(key(0x51)),
            female: true,
            race: Some(key(0x52)),
            class: Some(key(0x53)),
            creature: false,
            factions: vec![key(0x54)],
            dynamic_factions: vec![key(0x55)],
            only_reference: Some(key(0x56)),
        }),
        interior: Some(true),
    }
}

#[test]
fn speaker_terms_fold_on_the_subject() {
    let folded = |c: SourceCondition| fold(&lower_one(c).0, &speaker());
    let with = |f: u16, form: u32| SourceCondition::literal(f, "==", 1.0).with_form(key(form));
    assert_eq!(folded(with(72, 0x50)), Cond::True);
    assert_eq!(folded(with(72, 0x99)), Cond::False);
    assert_eq!(folded(with(427, 0x51)), Cond::True);
    assert_eq!(folded(with(69, 0x52)), Cond::True);
    assert_eq!(folded(with(68, 0x99)), Cond::False);
    assert_eq!(
        folded(SourceCondition::literal(70, "==", 1.0).with_params(1, 0)),
        Cond::True
    );
    assert_eq!(folded(SourceCondition::literal(64, "==", 1.0)), Cond::False);
    assert_eq!(folded(with(71, 0x54)), Cond::True);
    // A faction a script changes stays a runtime term.
    assert!(matches!(folded(with(71, 0x55)), Cond::Cmp { .. }));
    assert_eq!(folded(with(136, 0x56)), Cond::True);
    assert_eq!(
        folded(SourceCondition::literal(300, "==", 0.0)),
        Cond::False
    );
    // On another target the term stays.
    let on_player = with(72, 0x50).on(1, None);
    assert!(matches!(folded(on_player), Cond::Cmp { .. }));
    // Unknown facts keep the term.
    let unknown = fold(&lower_one(with(72, 0x50)).0, &Facts::default());
    assert!(matches!(unknown, Cond::Cmp { .. }));
}

#[test]
fn simplification_keeps_passed_fallbacks_visible() {
    let leaf = Cond::Fallback {
        function: 449,
        name: "HasPerk".into(),
        value: 0.0,
        report: true,
    };
    let x = Cond::Cmp {
        lhs: Expr::Var(VarRef::Temp(0)),
        op: CmpOp::Eq,
        rhs: Expr::Int(1),
    };
    assert_eq!(
        simplify(&Cond::All(vec![Cond::True, leaf.clone(), x.clone()])),
        Cond::All(vec![leaf.clone(), x.clone()])
    );
    assert_eq!(
        simplify(&Cond::All(vec![
            x.clone(),
            Cond::Not(Box::new(leaf.clone()))
        ])),
        Cond::False
    );
    assert_eq!(simplify(&Cond::Any(vec![x.clone(), leaf.clone()])), leaf);
    assert_eq!(
        simplify(&Cond::Not(Box::new(Cond::Not(Box::new(x.clone()))))),
        x
    );
    assert_eq!(simplify(&Cond::Any(vec![])), Cond::False);
    assert_eq!(
        simplify(&Cond::Cmp {
            lhs: Expr::Int(2),
            op: CmpOp::Gt,
            rhs: Expr::Float(1.5)
        }),
        Cond::True
    );
}

fn is_id(base: u32, on: RefExpr) -> Cond {
    Cond::Cmp {
        lhs: Expr::Call(Box::new(Call {
            func: Func::Known(FuncId::named("GetIsID").unwrap()),
            on: Some(on),
            args: vec![Expr::Form(key(base))],
        })),
        op: CmpOp::Eq,
        rhs: Expr::Int(1),
    }
}

/// `IsInList` (index 372) is a `GetIsID` disjunction over the list's
/// members, nested lists flattened, so the speaker fold decides it.
#[test]
fn is_in_list_is_a_get_is_id_disjunction_that_folds_on_the_speaker() {
    let mut names = names();
    names.set_form_list(key(0x50), vec![key(0x60), key(0x51), key(0x61)]);
    names.set_form_list(key(0x51), vec![key(0x62), key(0x60)]);
    let lower_with = |c: SourceCondition| {
        let mut uses = CondUses::default();
        let cond = lower(&[c], &owner(), &names, &mut uses);
        (cond, uses)
    };
    let member = Cond::Any(vec![
        is_id(0x60, RefExpr::SelfRef),
        is_id(0x62, RefExpr::SelfRef),
        is_id(0x61, RefExpr::SelfRef),
    ]);
    let (cond, uses) = lower_with(SourceCondition::literal(372, "==", 1.0).with_form(key(0x50)));
    assert_eq!(cond, member);
    assert!(uses.diags.is_empty(), "{:?}", uses.diags);
    let (cond, _) = lower_with(SourceCondition::literal(372, "==", 0.0).with_form(key(0x50)));
    assert_eq!(cond, Cond::Not(Box::new(member.clone())));
    let (cond, _) = lower_with(SourceCondition::literal(372, "<", 2.0).with_form(key(0x50)));
    assert_eq!(cond, Cond::True, "0 and 1 both pass");
    // On the player: runtime base compares.
    let (cond, _) = lower_with(
        SourceCondition::literal(372, "!=", 0.0)
            .with_form(key(0x51))
            .on(1, None),
    );
    assert_eq!(
        cond,
        Cond::Any(vec![
            is_id(0x62, RefExpr::Player),
            is_id(0x60, RefExpr::Player)
        ])
    );
    // Folded on a speaker: a member passes, another base fails.
    let facts = |base: u32| Facts {
        subject: Some(SubjectFacts {
            base: key(base),
            ..SubjectFacts::default()
        }),
        interior: None,
    };
    assert_eq!(fold(&member, &facts(0x62)), Cond::True);
    assert_eq!(fold(&member, &facts(0x70)), Cond::False);
}
