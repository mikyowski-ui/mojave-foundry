//! The script front end on hand-written snippets (spec 12.1, "Parser"):
//! every grammar rule, case variants, comments, malformed input.

use transpile::diag::{Category, Severity};
use transpile::ir::{
    BinOp, Block, Call, CmpOp, Event, Expr, FormKey, Func, RefExpr, ResultOwner, Script,
    ScriptKind, SimId, SimKind, Stmt, UnOp, VarDecl, VarRef, VarTy,
};
use transpile::obscript::{self, Compiled, NameTable, ScriptInput};
use transpile::texts::TextTable;

fn key(id: u32) -> FormKey {
    FormKey::new("testtown.esm", id)
}

const QUEST: u32 = 0x100;
const QUEST_SCRIPT: u32 = 0x101;
const DOOR_REF: u32 = 0x200;
const MARKER_REF: u32 = 0x201;
const MAYOR_REF: u32 = 0x202;
const MAYOR: u32 = 0x300;
const MAYOR_SCRIPT: u32 = 0x301;
const KEY: u32 = 0x400;
const MESSAGE: u32 = 0x401;
const NOTE: u32 = 0x402;
const SANDBOX: u32 = 0x403;
const COUNT: u32 = 0x500;
const RATE: u32 = 0x501;

fn names() -> NameTable {
    let mut names = NameTable::new();
    names.set_plugins(vec!["testtown.esm".into()]);
    for (edid, id, sig) in [
        ("TSQuest", QUEST, "QUST"),
        ("TSQuestScript", QUEST_SCRIPT, "SCPT"),
        ("TSDoorRef", DOOR_REF, "REFR"),
        ("TSMarkerRef", MARKER_REF, "REFR"),
        ("TSMayorRef", MAYOR_REF, "ACHR"),
        ("TSMayor", MAYOR, "NPC_"),
        ("TSKey", KEY, "KEYM"),
        ("TSMessage", MESSAGE, "MESG"),
        ("TSNote", NOTE, "NOTE"),
        ("TSSandbox", SANDBOX, "PACK"),
        ("TSGlobalCount", COUNT, "GLOB"),
        ("TSGlobalRate", RATE, "GLOB"),
    ] {
        names.add_form(edid, key(id), sig);
    }
    names.set_quest_script(key(QUEST), key(QUEST_SCRIPT));
    names.set_script_vars(
        key(QUEST_SCRIPT),
        vec![
            VarDecl {
                index: 1,
                name: "qCount".into(),
                ty: VarTy::Int,
            },
            VarDecl {
                index: 2,
                name: "qRate".into(),
                ty: VarTy::Float,
            },
        ],
    );
    names.set_ref_base(key(MAYOR_REF), key(MAYOR));
    names.set_base_script(key(MAYOR), key(MAYOR_SCRIPT));
    names.set_script_vars(
        key(MAYOR_SCRIPT),
        vec![VarDecl {
            index: 4,
            name: "mood".into(),
            ty: VarTy::Int,
        }],
    );
    names
}

fn compile(source: &str, kind: ScriptKind) -> Compiled {
    let id = SimId::of(SimKind::Script, &key(0x900));
    let input = ScriptInput {
        id,
        editor_id: "TSTest".into(),
        kind,
        source,
        declared: &[],
        references: &[],
        quest: None,
        implicit: None,
    };
    obscript::compile(&input, &names(), &mut TextTable::new())
}

fn record(source: &str) -> Compiled {
    compile(source, ScriptKind::Object)
}

fn problems(script: &Script) -> Vec<(u32, Category)> {
    script
        .diagnostics
        .iter()
        .filter(|d| d.severity != Severity::Note)
        .map(|d| (d.at.map_or(0, |a| a.line), d.category))
        .collect()
}

fn local(index: u32, ty: VarTy) -> VarRef {
    VarRef::Local {
        script: SimId::of(SimKind::Script, &key(0x900)),
        index,
        ty,
    }
}

fn known(call: &Call) -> Option<&'static str> {
    match &call.func {
        Func::Known(id) => Some(id.name()),
        Func::Unsupported { .. } => None,
    }
}

fn set_value(block: &Block, n: usize) -> &Expr {
    match &block.body[n] {
        Stmt::Set(_, value) => value,
        other => panic!("statement {n} is {other:?}"),
    }
}

#[test]
fn every_grammar_rule() {
    let compiled = record(include_str!("fixtures/obscript/grammar.txt"));
    let script = &compiled.script;
    assert_eq!(problems(script), vec![], "{:#?}", script.diagnostics);
    assert_eq!(
        compiled.ast.header.as_ref().map(|h| h.text.as_str()),
        Some("TSGrammarScript")
    );
    let types: Vec<VarTy> = script.vars.iter().map(|v| v.ty).collect();
    use VarTy::{Float, Int, Ref};
    assert_eq!(types, [Int, Int, Int, Float, Ref, Ref]);
    assert_eq!(
        script.vars.iter().map(|v| v.index).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6]
    );
    let events: Vec<&Event> = script.blocks.iter().map(|b| &b.event).collect();
    assert_eq!(
        events,
        [
            &Event::GameMode,
            &Event::OnActivate,
            &Event::OnTriggerEnter {
                by: Some(RefExpr::Player)
            },
            &Event::OnDeath { killer: None },
        ]
    );
    let game = &script.blocks[0];
    // set counter to counter + 1
    assert_eq!(
        game.body[0],
        Stmt::Set(
            local(1, Int),
            Expr::Binary(
                BinOp::Add,
                Box::new(Expr::Var(local(1, Int))),
                Box::new(Expr::Int(1))
            )
        )
    );
    // ( rate * 2 - 1 ) / 4: * before -, parentheses first.
    let Expr::Binary(BinOp::Div, sum, four) = set_value(game, 1) else {
        panic!()
    };
    assert_eq!(**four, Expr::Int(4));
    assert!(
        matches!(&**sum, Expr::Binary(BinOp::Sub, product, _) if matches!(**product, Expr::Binary(BinOp::Mul, _, _)))
    );
    assert!(matches!(set_value(game, 2), Expr::Unary(UnOp::Neg, _)));
    assert!(matches!(set_value(game, 3), Expr::Unary(UnOp::Not, _)));
    // a && b || c is (a && b) || c; comparisons bind tighter.
    let Stmt::If { arms, otherwise } = &game.body[4] else {
        panic!()
    };
    assert_eq!(arms.len(), 2);
    assert_eq!(otherwise, &vec![Stmt::Return]);
    let Expr::Binary(BinOp::Or, and, eq) = &arms[0].0 else {
        panic!("{:?}", arms[0].0)
    };
    assert!(matches!(**and, Expr::Binary(BinOp::And, _, _)));
    assert!(matches!(**eq, Expr::Binary(BinOp::Cmp(CmpOp::Eq), _, _)));
    assert_eq!(
        arms[1].1,
        vec![Stmt::Set(local(5, Ref), Expr::Ref(RefExpr::Player))]
    );
    // if ( GetStage TSQuest >= 20 ): the call takes one argument.
    let Stmt::If { arms, .. } = &game.body[5] else {
        panic!()
    };
    let Expr::Binary(BinOp::Cmp(CmpOp::Ge), call, twenty) = &arms[0].0 else {
        panic!()
    };
    assert_eq!(**twenty, Expr::Int(20));
    let Expr::Call(call) = &**call else { panic!() };
    assert_eq!(known(call), Some("GetStage"));
    assert_eq!(call.args, vec![Expr::Form(key(QUEST))]);
    let Stmt::Call(set_stage) = &arms[0].1[0] else {
        panic!()
    };
    assert_eq!(known(set_stage), Some("SetStage"));
    assert_eq!(set_stage.args, vec![Expr::Form(key(QUEST)), Expr::Int(30)]);
    // Reference calls, a comma between arguments.
    let Stmt::Call(lock) = &game.body[6] else {
        panic!()
    };
    assert_eq!(lock.on, Some(RefExpr::Placed(key(DOOR_REF))));
    let Stmt::Call(add) = &game.body[7] else {
        panic!()
    };
    assert_eq!(add.on, Some(RefExpr::Player));
    assert_eq!(add.args, vec![Expr::Form(key(KEY)), Expr::Int(1)]);
    assert!(matches!(
        set_value(game, 8),
        Expr::Binary(BinOp::Add, call, two) if matches!(**call, Expr::Call(_)) && **two == Expr::Int(2)
    ));
    // Quest and reference variables.
    let quest = SimId::of(SimKind::Quest, &key(QUEST));
    assert_eq!(
        set_value(game, 9),
        &Expr::Var(VarRef::Quest {
            quest: quest.clone(),
            index: 2,
            ty: Float
        })
    );
    assert_eq!(
        game.body[10],
        Stmt::Set(
            VarRef::Quest {
                quest,
                index: 1,
                ty: Int
            },
            Expr::Int(7)
        )
    );
    assert_eq!(
        set_value(game, 11),
        &Expr::Var(VarRef::Hoisted {
            owner: key(MAYOR_REF),
            script: SimId::of(SimKind::Script, &key(MAYOR_SCRIPT)),
            index: 4,
            ty: Int
        })
    );
    // IsActionRef is folded into a comparison with the action reference.
    let Stmt::If { arms, .. } = &script.blocks[1].body[0] else {
        panic!()
    };
    let Expr::Binary(BinOp::Cmp(CmpOp::Eq), folded, _) = &arms[0].0 else {
        panic!()
    };
    assert_eq!(
        **folded,
        Expr::Binary(
            BinOp::Cmp(CmpOp::Eq),
            Box::new(Expr::Ref(RefExpr::Action)),
            Box::new(Expr::Ref(RefExpr::Player))
        )
    );
    assert!(compiled.named.iter().any(|n| n.form == key(QUEST)));
}

#[test]
fn every_block_type() {
    let compiled = record(include_str!("fixtures/obscript/blocks.txt"));
    let script = &compiled.script;
    let sandbox = Some(key(SANDBOX));
    let player = Some(RefExpr::Player);
    assert_eq!(
        script
            .blocks
            .iter()
            .map(|b| b.event.clone())
            .collect::<Vec<_>>(),
        vec![
            Event::GameMode,
            Event::MenuMode,
            Event::OnActivate,
            Event::OnLoad,
            Event::OnDeath {
                killer: player.clone()
            },
            Event::OnHit { by: player.clone() },
            Event::OnTrigger { by: player.clone() },
            Event::OnTriggerEnter { by: player.clone() },
            Event::OnTriggerLeave { by: player },
            Event::OnStartCombat,
            Event::OnCombatEnd,
            Event::SayToDone,
            Event::OnPackageStart(sandbox.clone()),
            Event::OnPackageEnd(sandbox),
            Event::OnPackageChange(None),
            Event::OnAdd,
            Event::OnEquip,
            Event::OnUnequip,
            Event::ScriptEffectStart,
            Event::ScriptEffectUpdate,
            Event::ScriptEffectFinish,
            Event::Other("OnSomethingNew".into()),
        ]
    );
    let unsupported: Vec<u32> = script
        .diagnostics
        .iter()
        .filter(|d| d.category == Category::UnsupportedBlock)
        .map(|d| d.at.unwrap().line)
        .collect();
    // MenuMode, SayToDone, OnAdd, OnEquip, OnUnequip, the three effect
    // blocks and the unknown one (spec 5.3).
    assert_eq!(unsupported, [7, 27, 35, 37, 39, 41, 43, 45, 47]);
    assert_eq!(problems(script).len(), unsupported.len());
}

#[test]
fn case_does_not_matter() {
    let compiled = record(include_str!("fixtures/obscript/case.txt"));
    let script = &compiled.script;
    assert_eq!(problems(script), vec![], "{:#?}", script.diagnostics);
    assert!(script.blocks.iter().all(|b| b.event == Event::GameMode));
    assert_eq!(script.blocks.len(), 4);
    let Stmt::If { arms, otherwise } = &script.blocks[0].body[0] else {
        panic!()
    };
    assert_eq!(
        arms[0].1,
        vec![Stmt::Set(local(1, VarTy::Int), Expr::Int(1))]
    );
    let Stmt::Set(var, Expr::Call(call)) = &arms[1].1[0] else {
        panic!()
    };
    assert_eq!(var, &local(2, VarTy::Float));
    assert_eq!(known(call), Some("GetStage"));
    let Stmt::Call(set_stage) = &otherwise[0] else {
        panic!()
    };
    assert_eq!(set_stage.args[0], Expr::Form(key(QUEST)));
    let Stmt::If { arms, .. } = &script.blocks[1].body[0] else {
        panic!()
    };
    let Stmt::Call(set_av) = &arms[0].1[0] else {
        panic!()
    };
    assert_eq!(known(set_av), Some("SetActorValue"));
    assert_eq!(set_av.args, vec![Expr::Int(16), Expr::Int(50)]);
    let Stmt::Call(strength) = &arms[0].1[1] else {
        panic!()
    };
    assert_eq!(strength.args[0], Expr::Int(5));
}

#[test]
fn result_scripts_are_statement_lists() {
    let info = SimId::of(SimKind::Info, &key(0x800));
    let compiled = compile(
        include_str!("fixtures/obscript/result.txt"),
        ScriptKind::Result(ResultOwner::InfoEnd(info)),
    );
    let script = &compiled.script;
    assert_eq!(problems(script), vec![], "{:#?}", script.diagnostics);
    assert_eq!(script.blocks.len(), 1);
    assert_eq!(script.blocks[0].event, Event::Result);
    let body = &script.blocks[0].body;
    assert_eq!(body.len(), 7);
    let Stmt::Call(karma) = &body[5] else {
        panic!()
    };
    assert_eq!(known(karma), Some("RewardKarma"));
    assert_eq!(karma.args, vec![Expr::Int(-5)]);
    let Stmt::Call(unlock) = &body[3] else {
        panic!()
    };
    assert_eq!(unlock.on, Some(RefExpr::Placed(key(DOOR_REF))));
}

#[test]
fn calls_by_signature() {
    let compiled = record(include_str!("fixtures/obscript/calls.txt"));
    let script = &compiled.script;
    let body = &script.blocks[0].body;
    let Stmt::Call(move_to) = &body[0] else {
        panic!()
    };
    assert_eq!(
        move_to.args,
        vec![
            Expr::Ref(RefExpr::Placed(key(MARKER_REF))),
            Expr::Int(0),
            Expr::Int(0),
            Expr::Int(-100)
        ]
    );
    let Stmt::Set(_, Expr::Binary(BinOp::Mul, distance, two)) = &body[3] else {
        panic!("{:?}", body[3])
    };
    assert_eq!(**two, Expr::Int(2));
    let Expr::Call(distance) = &**distance else {
        panic!()
    };
    assert_eq!(distance.args, vec![Expr::Ref(RefExpr::Player)]);
    let value = |n: usize| match &body[n] {
        Stmt::Set(_, v) => v.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(value(5), Expr::Ref(RefExpr::SelfRef));
    assert_eq!(value(6), Expr::Ref(RefExpr::Action));
    assert_eq!(
        value(7),
        Expr::Ref(RefExpr::Linked(Box::new(RefExpr::SelfRef)))
    );
    assert_eq!(
        value(8),
        Expr::Var(VarRef::Global(SimId::of(SimKind::Global, &key(COUNT))))
    );
    assert!(matches!(
        value(9),
        Expr::Var(VarRef::Quest { index: 1, .. })
    ));
    let Stmt::Call(enable) = &body[11] else {
        panic!()
    };
    assert_eq!(known(enable), Some("Enable"));
    assert!(matches!(enable.on, Some(RefExpr::Var(_))));
    let Stmt::Call(sound) = &body[12] else {
        panic!()
    };
    assert_eq!(
        sound.func,
        Func::Unsupported {
            name: "PlaySound".into()
        }
    );
    let Stmt::Call(unknown) = &body[13] else {
        panic!()
    };
    assert_eq!(
        unknown.func,
        Func::Unsupported {
            name: "FrobnicateWidget".into()
        }
    );
    assert_eq!(unknown.args.len(), 2);
    // An unknown function in an expression takes atoms up to the operator.
    let Expr::Binary(BinOp::Add, call, one) = value(14) else {
        panic!()
    };
    assert_eq!(*one, Expr::Int(1));
    assert!(matches!(*call, Expr::Call(ref c) if c.args.len() == 1));
    for n in [15, 16, 17] {
        assert!(
            matches!(value(n), Expr::Call(ref c) if matches!(c.func, Func::Unsupported { .. })),
            "{n}: {:?}",
            value(n)
        );
    }
    assert!(matches!(value(18), Expr::Binary(BinOp::Add, _, _)));
    let names: Vec<Option<&str>> = body[19..23]
        .iter()
        .map(|s| match s {
            Stmt::Call(c) => known(c),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        [Some("EvaluatePackage"), Some("Kill"), Some("Kill"), None]
    );
    assert_eq!(
        value(23),
        Expr::Call(Box::new(Call {
            func: Func::Known(transpile::ir::FuncId::named("GetIsSex").unwrap()),
            on: None,
            args: vec![Expr::Int(1)],
        }))
    );
    let categories: Vec<Category> = problems(script).into_iter().map(|(_, c)| c).collect();
    assert_eq!(
        categories,
        [
            Category::DroppedFunction,
            Category::UnknownFunction,
            Category::UnknownFunction,
            Category::UnknownFunction,
            Category::UnknownFunction,
            Category::UnknownFunction,
            Category::DroppedFunction,
        ],
        "{:#?}",
        script.diagnostics
    );
}

#[test]
fn malformed_lines_cost_only_themselves() {
    let compiled = record(include_str!("fixtures/obscript/malformed.txt"));
    let script = &compiled.script;
    use Category::*;
    assert_eq!(
        problems(script),
        vec![
            (5, BadDeclaration),
            (8, OutsideBlock),
            (11, BadSet),
            (12, BadSet),
            (13, BadSet),
            (14, BadExpression),
            (15, TrailingTokens),
            (16, BadCharacter),
            (16, TrailingTokens),
            (17, DroppedFunction),
            (17, UnterminatedString),
            (18, StrayElse),
            (19, StrayElse),
            (20, StrayElse),
            (26, StrayElse),
            (29, UnresolvedName),
            (30, UnresolvedQuestVar),
            (31, UnknownFunction),
            (32, UnexpectedToken),
            (33, BadExpression),
            (33, MissingEndif),
            (35, MissingEndif),
            (38, OutsideBlock),
            (39, StrayEnd),
            (41, BadBlock),
            (41, UnsupportedBlock),
            (45, MissingEnd),
        ],
        "{:#?}",
        script.diagnostics
    );
    let notes: Vec<(u32, Category)> = script
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Note)
        .map(|d| (d.at.map_or(0, |a| a.line), d.category))
        .collect();
    assert_eq!(notes, [(6, TrailingTokens)]);
    // The declarations that parse are kept; so is every good statement.
    assert_eq!(
        script
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>(),
        ["ok", "trailing"]
    );
    assert_eq!(script.blocks.len(), 3);
    let game = &script.blocks[0].body;
    assert!(game.contains(&Stmt::Set(local(1, VarTy::Int), Expr::Int(2))));
    let Some(Stmt::If { otherwise, .. }) = game.iter().find(|s| matches!(s, Stmt::If { .. }))
    else {
        panic!()
    };
    assert_eq!(otherwise.len(), 2);
    assert_eq!(script.blocks[1].event, Event::Other(String::new()));
    assert_eq!(script.blocks[2].event, Event::OnLoad);
    assert_eq!(script.blocks[2].body.len(), 1);
}

#[test]
fn positions_are_line_and_column() {
    let compiled = record("begin GameMode\n\tset ok 5\nend\n");
    let d = &compiled.script.diagnostics[0];
    assert_eq!(d.category, Category::BadSet);
    let at = d.at.unwrap();
    assert_eq!((at.line, at.column), (2, 9));
    assert_eq!(d.owner, Some(SimId::of(SimKind::Script, &key(0x900))));
}

#[test]
fn nothing_panics_on_noise() {
    let samples = [
        "",
        "\0\0\0",
        "begin",
        "end end end",
        "if if if",
        "set set to to",
        "((((((((((((",
        "a.b.c.d.e",
        ".disable",
        "begin GameMode\nset x to -\nend",
        "begin GameMode\nset x to 1 +\nend",
        "begin GameMode\nfoo \"\nend",
        "scn\nscn x\nscn y",
        "short\nfloat\nref",
        "begin GameMode\n player.\nend",
        "begin GameMode\n set a.b to\nend",
        "begin GameMode\n 99999999999999999999999\nend",
        "\u{feff}begin GameMode\r\nend\r",
        "begin GameMode\n if x == \nendif\nend",
    ];
    for sample in samples {
        let _ = record(sample);
        let _ = compile(
            sample,
            ScriptKind::Result(ResultOwner::InfoBegin(SimId::of(SimKind::Info, &key(1)))),
        );
    }
}

/// The IR of the grammar fixture, as JSON: the snapshot that pins the
/// serialised form. `UPDATE_SNAPSHOTS=1` rewrites it.
#[test]
fn grammar_snapshot() {
    let compiled = record(include_str!("fixtures/obscript/grammar.txt"));
    let mut json = serde_json::to_string_pretty(&compiled.script).unwrap();
    json.push('\n');
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/obscript/grammar.ir.json"
    );
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(path, &json).unwrap();
    }
    let expected = std::fs::read_to_string(path).expect("snapshot; run with UPDATE_SNAPSHOTS=1");
    assert_eq!(json, expected);
}
