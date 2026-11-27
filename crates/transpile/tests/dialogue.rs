//! The dialogue compiler on the testkit's town (spec 4).

use transpile::dialogue::checks::fo3_chance;
use transpile::ir::{
    ActorValue, Cond, FailureRoute, GlobalPurpose, Module, Pick, Remember, SimId, SimKind, Skill,
    SkillCheck, Stat, once_sub,
};
use transpile::testkit::{self, forms::*, key};

fn info(id: u32) -> SimId {
    SimId::of(SimKind::Info, &key(id))
}

fn topic(id: u32) -> SimId {
    SimId::of(SimKind::Topic, &key(id))
}

fn module() -> Module {
    testkit::town()
}

#[test]
fn one_dialogue_per_speaking_base() {
    let m = module();
    let speakers: Vec<&str> = m.dialogues.values().map(|d| d.editor_id.as_str()).collect();
    assert_eq!(
        speakers,
        ["TSMayor", "TSGuard"],
        "the raider has nothing to say"
    );
    for d in m.dialogues.values() {
        assert_eq!(d.greeting, Some(topic(GREETING)));
        assert!(!d.greeting_label);
        // Ordinals are 1-based positions in INFO id order.
        for (i, n) in d.nodes.iter().enumerate() {
            assert_eq!(n.ordinal as usize, i + 1);
        }
        let mut infos: Vec<&SimId> = d.nodes.iter().map(|n| &n.info).collect();
        let sorted = {
            let mut s = infos.clone();
            s.sort();
            s
        };
        assert_eq!(infos, sorted);
        infos.dedup();
        assert_eq!(infos.len(), d.nodes.len());
    }
}

#[test]
fn speaker_terms_are_folded_out() {
    let m = module();
    let mayor = &m.dialogues[&key(MAYOR)];
    let guard = &m.dialogues[&key(GUARD)];
    let node =
        |d: &transpile::ir::Dialogue, i: u32| d.nodes.iter().find(|n| n.info == info(i)).cloned();
    // GetIsID mayor folds to true for the mayor, false for the guard.
    assert_eq!(node(mayor, HELLO_INFO).unwrap().condition, Cond::True);
    assert!(node(guard, HELLO_INFO).is_none());
    // The OR of the two speakers folds to true; the stage term stays.
    let either = node(guard, EITHER_INFO).unwrap();
    assert!(
        matches!(either.condition, Cond::Cmp { .. }),
        "{:?}",
        either.condition
    );
    // The guard's own INFO keeps its fallback and its quest variable.
    let own = node(guard, GUARD_INFO).unwrap();
    let Cond::All(terms) = &own.condition else {
        panic!("{:?}", own.condition)
    };
    assert!(
        terms
            .iter()
            .any(|t| matches!(t, Cond::Fallback { function: 255, .. }))
    );
    assert!(node(mayor, GUARD_INFO).is_none());
}

#[test]
fn random_groups_and_picks() {
    let m = module();
    let guard = &m.dialogues[&key(GUARD)];
    let ordinal = |i: u32| {
        guard
            .nodes
            .iter()
            .find(|n| n.info == info(i))
            .unwrap()
            .ordinal
    };
    assert_eq!(
        guard.topics[&topic(GREETING)].picks,
        vec![Pick::Random(vec![
            ordinal(EITHER_INFO),
            ordinal(GUARD_INFO)
        ])]
    );
    // For the mayor the random run has one member: a plain pick after the
    // say-once INFO.
    let mayor = &m.dialogues[&key(MAYOR)];
    let ordinal = |i: u32| {
        mayor
            .nodes
            .iter()
            .find(|n| n.info == info(i))
            .unwrap()
            .ordinal
    };
    assert_eq!(
        mayor.topics[&topic(GREETING)].picks,
        vec![
            Pick::Info(ordinal(HELLO_INFO)),
            Pick::Info(ordinal(EITHER_INFO))
        ]
    );
}

#[test]
fn choices_hub_and_given_topics() {
    let m = module();
    let mayor = &m.dialogues[&key(MAYOR)];
    let node = |i: u32| mayor.nodes.iter().find(|n| n.info == info(i)).unwrap();
    assert_eq!(node(HELLO_INFO).choices, vec![topic(JOB)]);
    assert_eq!(node(JOB_INFO).choices, vec![topic(DETAILS)]);
    assert!(node(DETAILS_INFO).choices.is_empty(), "back to the hub");
    assert!(m.infos[&info(DETAILS_INFO)].flags.goodbye);
    // Option text: the prompt, else the topic's name.
    let text = |id: Option<transpile::ir::TextId>| m.texts[id.unwrap().0 as usize].source.clone();
    assert_eq!(text(node(JOB_INFO).prompt), "Got any work?");
    assert_eq!(text(node(DETAILS_INFO).prompt), "Where is it?");
    // The hub: top-level topics, and the topic AddTopic gives behind its
    // flag; neither the greeting nor the failure topic.
    let hub: Vec<(SimId, bool)> = mayor
        .hub
        .iter()
        .map(|h| (h.topic.clone(), h.given.is_some()))
        .collect();
    assert_eq!(
        hub,
        vec![
            (topic(JOB), false),
            (topic(BRIBE), false),
            (topic(RUMOR), true)
        ]
    );
    let given = topic(RUMOR).with_sub(once_sub::GIVEN);
    assert_eq!(mayor.hub[2].given.as_ref(), Some(&given));
    assert_eq!(m.globals[&given].purpose, GlobalPurpose::OnceFlag);
}

#[test]
fn the_fo3_challenge_and_its_failure_route() {
    let m = module();
    let bribe = &m.infos[&info(BRIBE_INFO)];
    assert_eq!(
        bribe.check,
        Some(SkillCheck::Percent {
            skill: Skill::Speech,
            difficulty: 2,
            label: Some(ActorValue(43)),
            failure: FailureRoute::SharedTopic(topic(FAILURE)),
            remember: Remember::Forever,
        })
    );
    let mayor = &m.dialogues[&key(MAYOR)];
    let node = mayor
        .nodes
        .iter()
        .find(|n| n.info == info(BRIBE_INFO))
        .unwrap();
    assert_eq!(node.failure, Some(topic(FAILURE)));
    assert_eq!(mayor.topics[&topic(FAILURE)].picks.len(), 1);
    // The failure INFO links from the challenge's topic; that link is the
    // failure route, never an option after a passed challenge.
    assert_eq!(
        m.infos[&info(FAILURE_INFO)].link_from,
        vec![topic(BRIBE)],
        "the testkit links the failure INFO from the challenge topic"
    );
    assert!(
        !node.choices.contains(&topic(FAILURE)),
        "the failure topic is offered after the challenge: {:?}",
        node.choices
    );
    // Remembered forever: two flags.
    for sub in [once_sub::ATTEMPTED, once_sub::PASSED] {
        assert!(m.globals.contains_key(&info(BRIBE_INFO).with_sub(sub)));
    }
    // The plugin's settings, and the formula at them.
    assert_eq!(m.settings.speech_difficulty, [10, 40, 55, 70, 80]);
    assert_eq!(fo3_chance(50, 2, &m.settings), 60);
}

#[test]
fn the_new_vegas_threshold_pair() {
    let m = module();
    let pass = m.infos[&info(FIX_PASS)].check.clone();
    let fail = m.infos[&info(FIX_FAIL)].check.clone();
    let Some(SkillCheck::Threshold {
        stat,
        value,
        shown_when_failed,
        partner,
        label,
        ..
    }) = pass
    else {
        panic!("{pass:?}")
    };
    assert_eq!(stat, Stat::Skill(Skill::Repair));
    assert_eq!(value, 50);
    assert!(shown_when_failed);
    assert_eq!(partner, Some(info(FIX_FAIL)));
    assert_eq!(label, Some(ActorValue(39)));
    assert!(matches!(
        fail,
        Some(SkillCheck::Threshold { partner: Some(p), shown_when_failed: true, .. }) if p == info(FIX_PASS)
    ));
    let guard = &m.dialogues[&key(GUARD)];
    assert_eq!(guard.topics[&topic(FIX)].picks.len(), 2);
    assert_eq!(guard.counts.thresholds, 2);
}

#[test]
fn once_flags() {
    let m = module();
    let once = info(HELLO_INFO).with_sub(once_sub::SAY_ONCE);
    assert_eq!(m.globals[&once].purpose, GlobalPurpose::OnceFlag);
    assert_eq!(m.globals[&once].initial, 0);
    assert!(
        !m.globals
            .contains_key(&info(EITHER_INFO).with_sub(once_sub::SAY_ONCE))
    );
}

/// The town's source with a second, low-priority quest that is gated on
/// `IsInList` of a form list holding the mayor: three random greeting
/// INFOs of it, one first in file order and two last.
fn town_with_a_generic_quest() -> Module {
    use transpile::source::{SourceCondition, SourceInfo, SourceQuest, SourceResponse};
    let dir = tempfile::tempdir().unwrap();
    let mir = testkit::extract(dir.path(), false);
    let mut source = transpile::source::load(&mir, &testkit::slice(false)).unwrap();
    let (generic, list) = (key(0x8E0), key(0x8E1));
    source.quests.get_mut(&key(QUEST)).unwrap().priority = 50;
    source.quests.insert(
        generic.clone(),
        SourceQuest {
            editor_id: Some("TSGenericQuest".into()),
            name: None,
            script: None,
            flags: 1,
            priority: 5,
            delay: None,
            stages: Vec::new(),
            objectives: Vec::new(),
            conditions: vec![SourceCondition::literal(372, "==", 1.0).with_form(list.clone())],
        },
    );
    source.form_lists.insert(list, vec![key(MAYOR)]);
    for info in source.infos.values_mut() {
        if info.topic == key(GREETING) {
            info.order.1 += 10;
        }
    }
    for (form, order, flags) in [(0x8E2, 0, 0x02), (0x8E3, 100, 0x02), (0x8E4, 101, 0x02)] {
        source.infos.insert(
            key(form),
            SourceInfo {
                topic: key(GREETING),
                quest: Some(generic.clone()),
                order: (0, order),
                info_type: 0,
                flags,
                speaker: None,
                prompt: None,
                responses: vec![SourceResponse {
                    number: 1,
                    emotion_type: 0,
                    emotion_value: 0,
                    text: "Hello there.".into(),
                }],
                conditions: Vec::new(),
                choices: Vec::new(),
                link_from: Vec::new(),
                add_topics: Vec::new(),
                begin: None,
                end: None,
                challenge: None,
                label: None,
                knam: None,
                knam_sig: None,
            },
        );
    }
    transpile::build_module(&source, &testkit::slice(false), &Default::default()).unwrap()
}

#[test]
fn picks_follow_quest_priority_and_random_groups_stay_in_one_quest() {
    let m = town_with_a_generic_quest();
    let mayor = &m.dialogues[&key(MAYOR)];
    let ordinal = |i: u32| {
        mayor
            .nodes
            .iter()
            .find(|n| n.info == info(i))
            .unwrap_or_else(|| panic!("no node for {i:#x}"))
            .ordinal
    };
    // The quest of priority 50 first, in file order, though the generic
    // quest's first INFO comes before them in the file; then the generic
    // quest's, in file order, one random group of their own (not joined
    // to the random INFO of the other quest before them).
    assert_eq!(
        mayor.topics[&topic(GREETING)].picks,
        vec![
            Pick::Info(ordinal(HELLO_INFO)),
            Pick::Info(ordinal(EITHER_INFO)),
            Pick::Random(vec![ordinal(0x8E2), ordinal(0x8E3), ordinal(0x8E4)]),
        ]
    );
    // IsInList on the quest: the guard is not in the list, so the generic
    // quest's INFOs fold away for it.
    let guard = &m.dialogues[&key(GUARD)];
    assert!(
        guard
            .nodes
            .iter()
            .all(|n| ![0x8E2, 0x8E3, 0x8E4].map(info).contains(&n.info))
    );
}
