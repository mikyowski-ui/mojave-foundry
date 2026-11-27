//! The quest dispatcher at the sizes real slices reach (regressions of the
//! Super-Duper Mart and Megaton layers): a stage script that switches an
//! enable group (its `grp_<n>` applier must be emitted), and a dispatcher
//! of hundreds of quests and thousands of stages that the engine's stack
//! model must accept with no finding.

mod support;

use support::{Instance, Obj, World, assert_program_ok};
use transpile::ir::{
    BaseFiles, Block, Call, CmpOp, Cond, EnableGroup, Event, Expr, FormKey, Func, FuncId, Global,
    GlobalPurpose, LogEntry, Module, Objective, PlacedKind, PlacedObject, Placement, Quest,
    RefExpr, ResultOwner, Script, ScriptKind, SimConfig, SimId, SimKind, Stage, Stmt, VarRef,
    VarTy, quest_sub,
};
use transpile::lower_layer;
use transpile::lowered::LowerFacts;

fn key(id: u32) -> FormKey {
    FormKey::new("big.esm", id)
}

fn call(name: &str, on: Option<RefExpr>, args: Vec<Expr>) -> Stmt {
    Stmt::Call(Call {
        func: Func::Known(FuncId::named(name).expect(name)),
        on,
        args,
    })
}

fn global(m: &mut Module, id: SimId, purpose: GlobalPurpose, initial: i32) {
    m.globals.insert(
        id.clone(),
        Global {
            id,
            purpose,
            name: "G".into(),
            ty: VarTy::Int,
            initial,
        },
    );
}

/// A quest of `stages` stages (indices 10, 20, ...), each with one log
/// entry whose script runs `body(stage)`, `objectives` objectives, and its
/// quest-state globals as the middle allocates them.
fn add_quest(
    m: &mut Module,
    form: FormKey,
    stages: u32,
    objectives: i32,
    game_mode: Option<Vec<Stmt>>,
    body: impl Fn(i16) -> Vec<Stmt>,
) {
    let quest = SimId::of(SimKind::Quest, &form);
    let mut stage_list = Vec::new();
    for s in 1..=stages {
        let index = (s * 10) as i16;
        let script = SimId::stage_script(&form, index, 0);
        m.scripts.insert(
            script.clone(),
            Script {
                id: script.clone(),
                editor_id: "Stage".into(),
                kind: ScriptKind::Result(ResultOwner::Stage(SimId::stage(&form, index))),
                vars: Vec::new(),
                blocks: vec![Block {
                    event: Event::Result,
                    body: body(index),
                }],
                diagnostics: Vec::new(),
            },
        );
        stage_list.push(Stage {
            id: SimId::stage(&form, index),
            index,
            entries: vec![LogEntry {
                complete: s == stages,
                fail: false,
                condition: Cond::Cmp {
                    lhs: Expr::Call(Box::new(Call {
                        func: Func::Known(FuncId::named("GetStage").unwrap()),
                        on: None,
                        args: vec![Expr::Form(form.clone())],
                    })),
                    op: CmpOp::Ge,
                    rhs: Expr::Int(i32::from(index)),
                },
                text: None,
                script: Some(script),
                next_quest: None,
            }],
        });
    }
    let script = game_mode.map(|body| {
        let id = SimId::of(SimKind::Script, &form.clone());
        m.scripts.insert(
            id.clone(),
            Script {
                id: id.clone(),
                editor_id: "QuestScript".into(),
                kind: ScriptKind::Quest,
                vars: Vec::new(),
                blocks: vec![Block {
                    event: Event::GameMode,
                    body,
                }],
                diagnostics: Vec::new(),
            },
        );
        id
    });
    let objectives: Vec<Objective> = (1..=objectives)
        .map(|i| Objective {
            id: SimId::objective(&form, i * 10),
            index: i * 10,
            text: None,
            targets: Vec::new(),
        })
        .collect();
    global(
        m,
        SimId::quest_state(&form, quest_sub::STAGE),
        GlobalPurpose::QuestState,
        0,
    );
    global(
        m,
        SimId::quest_state(&form, quest_sub::FLAGS),
        GlobalPurpose::QuestState,
        1,
    );
    for i in 0..stages.div_ceil(31) {
        global(
            m,
            SimId::quest_state(&form, quest_sub::DONE + i),
            GlobalPurpose::QuestState,
            0,
        );
        global(
            m,
            SimId::quest_state(&form, quest_sub::PEND + i),
            GlobalPurpose::QuestState,
            0,
        );
    }
    for o in &objectives {
        global(m, o.id.clone(), GlobalPurpose::QuestState, 0);
    }
    m.quests.insert(
        quest.clone(),
        Quest {
            id: quest,
            editor_id: format!("Q{}", form.as_str()),
            name: None,
            start_enabled: true,
            priority: 0,
            delay_seconds: None,
            script,
            vars: Vec::new(),
            stages: stage_list,
            objectives,
            condition: Cond::True,
        },
    );
}

fn dispatcher_bytes(m: &Module, facts: &LowerFacts) -> Vec<u8> {
    let out = lower_layer(m, facts, &BaseFiles::default(), &SimConfig::default())
        .expect("the module lowers");
    out.lowered.files["scripts/mfquest.int"].clone()
}

fn placed(map_number: u32, reference: &FormKey, hex: u32, pid: u32, hidden: bool) -> PlacedObject {
    PlacedObject {
        map: "big".into(),
        map_number,
        elevation: 0,
        hex,
        kind: PlacedKind::Scenery,
        object_id: Some(hex),
        pid: Some(pid),
        reference: Some(reference.clone()),
        base: Some(key(0x900)),
        hidden,
        ..PlacedObject::default()
    }
}

#[test]
fn a_stage_script_that_enables_a_group_emits_its_applier_and_runs() {
    let mut m = Module::default();
    let parent = key(0x500);
    let child = key(0x501);
    let group = SimId::of(SimKind::Group, &parent);
    m.groups.insert(
        group.clone(),
        EnableGroup {
            id: group.clone(),
            initially_enabled: false,
            children: vec![(child.clone(), false)],
        },
    );
    global(&mut m, group.clone(), GlobalPurpose::Group, 0);
    let quest = key(0x100);
    add_quest(&mut m, quest.clone(), 1, 0, None, |_| {
        vec![call(
            "Enable",
            Some(RefExpr::Placed(parent.clone())),
            Vec::new(),
        )]
    });
    let facts = LowerFacts {
        placement: Placement {
            objects: vec![placed(1, &child, 1234, 0x0200_0001, true)],
        },
        ..LowerFacts::default()
    };
    let bytes = dispatcher_bytes(&m, &facts);
    let program = assert_program_ok("mfquest.int", &bytes);
    assert!(program.find_procedure(b"grp_1", true).is_some());

    // Run it: stage 10 pending, the dispatcher runs its script.
    let order: Vec<SimId> = m
        .globals_in_allocation_order()
        .iter()
        .map(|g| g.id.clone())
        .collect();
    let index = |id: &SimId| order.iter().position(|g| g == id).unwrap();
    let mut world = World::new(order.len(), 0);
    world.globals[index(&SimId::quest_state(&quest, quest_sub::FLAGS))] = 1;
    world.globals[index(&SimId::quest_state(&quest, quest_sub::STAGE))] = 10;
    world.globals[index(&SimId::quest_state(&quest, quest_sub::PEND))] = 1;
    world.objects.insert(
        7,
        Obj {
            pid: 0x0200_0001,
            hex: 1234,
            hidden: true,
            ..Obj::default()
        },
    );
    let mut dispatcher = Instance::load("mfquest.int", &bytes, &mut world);
    dispatcher.call(&mut world, "start");
    assert_eq!(world.globals[index(&group)], 1, "the group is on");
    assert!(!world.objects[&7].hidden, "its child is shown at once");
}

#[test]
fn a_dispatcher_of_hundreds_of_quests_has_no_stack_finding() {
    let mut m = Module::default();
    let parent = key(0x500);
    let group = SimId::of(SimKind::Group, &parent);
    m.groups.insert(
        group.clone(),
        EnableGroup {
            id: group.clone(),
            initially_enabled: true,
            children: Vec::new(),
        },
    );
    global(&mut m, group, GlobalPurpose::Group, 1);
    let count = SimId::of(SimKind::Global, &key(0x600));
    global(&mut m, count.clone(), GlobalPurpose::Glob, 0);
    // 400 quests of 12 stages, each stage script setting the next stage,
    // an objective, a group and a global; half with a long GameMode body.
    for q in 0..400u32 {
        let form = key(0x1000 + q);
        let other = form.clone();
        let var = count.clone();
        let parent = parent.clone();
        let game_mode = (q % 2 == 0).then(|| {
            (0..40)
                .map(|i| Stmt::If {
                    arms: vec![(
                        Expr::Binary(
                            transpile::ir::BinOp::Cmp(CmpOp::Gt),
                            Box::new(Expr::Var(VarRef::Global(count.clone()))),
                            Box::new(Expr::Int(i)),
                        ),
                        vec![
                            call(
                                "SetStage",
                                None,
                                vec![Expr::Form(form.clone()), Expr::Int(20)],
                            ),
                            Stmt::Set(
                                VarRef::Global(count.clone()),
                                Expr::Binary(
                                    transpile::ir::BinOp::Div,
                                    Box::new(Expr::Var(VarRef::Global(count.clone()))),
                                    Box::new(Expr::Var(VarRef::Global(count.clone()))),
                                ),
                            ),
                        ],
                    )],
                    otherwise: Vec::new(),
                })
                .collect()
        });
        add_quest(&mut m, form.clone(), 12, 3, game_mode, move |stage| {
            vec![
                call(
                    "SetStage",
                    None,
                    vec![Expr::Form(other.clone()), Expr::Int(i32::from(stage) + 10)],
                ),
                call(
                    "SetObjectiveDisplayed",
                    None,
                    vec![Expr::Form(other.clone()), Expr::Int(10), Expr::Int(1)],
                ),
                call("Disable", Some(RefExpr::Placed(parent.clone())), Vec::new()),
                Stmt::Set(
                    VarRef::Global(var.clone()),
                    Expr::Binary(
                        transpile::ir::BinOp::Add,
                        Box::new(Expr::Var(VarRef::Global(var.clone()))),
                        Box::new(Expr::Int(1)),
                    ),
                ),
            ]
        });
    }
    let bytes = dispatcher_bytes(&m, &LowerFacts::default());
    assert!(
        bytes.len() > 500_000,
        "a realistic size: {} bytes",
        bytes.len()
    );
    let program = assert_program_ok("mfquest.int", &bytes);
    assert!(program.find_procedure(b"grp_1", true).is_some());
    // Due times are bounded: 64 polling buckets for 200 polled quests.
    let polls = program
        .procedures()
        .iter()
        .filter(|p| p.name.starts_with(b"q_poll"))
        .count();
    assert_eq!(polls, 64);
}
