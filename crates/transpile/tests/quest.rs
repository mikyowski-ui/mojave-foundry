//! Quest state, variables, Pip-Boy rows and the ordering reports on the
//! testkit's town (spec 6, 5.4, 5.5).

use transpile::diag::Category;
use transpile::ir::{
    GlobalPurpose, SimId, SimKind, TextRole, VarTy, objective_state, quest_flags, quest_sub,
};
use transpile::quest::{stage_bit, stage_globals};
use transpile::testkit::{self, forms::*, key};

#[test]
fn quest_state_globals() {
    let m = testkit::town();
    let q = key(QUEST);
    let quest = &m.quests[&SimId::of(SimKind::Quest, &q)];
    let g = |sub: u32| &m.globals[&SimId::quest_state(&q, sub)];
    assert_eq!(g(quest_sub::STAGE).initial, 0);
    assert_eq!(
        g(quest_sub::FLAGS).initial,
        quest_flags::RUNNING | quest_flags::APPLIED,
        "start enabled"
    );
    assert_eq!(stage_globals(quest), 1);
    assert_eq!(g(quest_sub::DONE).name, "TSQUEST_DONE0");
    assert_eq!(g(quest_sub::PEND).name, "TSQUEST_PEND0");
    assert_eq!(stage_bit(quest, 10), Some((0, 0)));
    assert_eq!(stage_bit(quest, 30), Some((0, 2)));
    assert_eq!(stage_bit(quest, 99), None);
    for n in [10, 20] {
        let obj = &m.globals[&SimId::objective(&q, n)];
        assert_eq!(obj.purpose, GlobalPurpose::QuestState);
        assert_eq!(obj.initial, objective_state::HIDDEN);
    }
    let other = SimId::quest_state(&key(OTHER_QUEST), quest_sub::FLAGS);
    assert_eq!(m.globals[&other].initial, 0);
}

#[test]
fn quest_variables_used_by_code_get_globals() {
    let m = testkit::town();
    let count = &m.globals[&SimId::new(SimKind::QuestVar, key(QUEST), 1)];
    assert_eq!(
        (count.purpose, count.ty),
        (GlobalPurpose::QuestVar, VarTy::Int)
    );
    assert_eq!(count.name, "TSQUEST_QCOUNT");
    let rate = &m.globals[&SimId::new(SimKind::QuestVar, key(QUEST), 2)];
    assert_eq!(rate.ty, VarTy::Float);
}

#[test]
fn pipboy_rows_are_objectives_only() {
    let m = testkit::town();
    let id = SimId::of(SimKind::Quest, &key(QUEST));
    let journal = &m.journal[&id];
    assert_eq!(m.texts[journal.heading.0 as usize].source, "Town Trouble");
    assert_eq!(
        m.texts[journal.heading.0 as usize].role,
        TextRole::QuestName
    );
    let rows: Vec<(u32, &str, i32, i32)> = journal
        .rows
        .iter()
        .map(|r| {
            (
                r.objective.sub,
                m.texts[r.text.0 as usize].source.as_str(),
                r.display_at,
                r.completed_at,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (10, "Talk to the mayor.", 1, 2),
            (20, "Open the door.", 1, 2)
        ]
    );
    assert!(journal.rows.iter().all(|r| r.global == r.objective));
    // The other quest has no objectives: no group, and nothing to say.
    assert!(
        !m.journal
            .contains_key(&SimId::of(SimKind::Quest, &key(OTHER_QUEST)))
    );
}

#[test]
fn ordering_reports() {
    let m = testkit::town();
    // The greeting sets stage 10 and then reads qCount, which stage 10's
    // script changes.
    let found: Vec<_> = m
        .diagnostics
        .iter()
        .filter(|d| d.category == Category::StageOrdering)
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].subject, "TSQuest 10");
    assert_eq!(
        found[0].owner.as_ref().map(|o| o.form.clone()),
        Some(key(HELLO_INFO))
    );
}
