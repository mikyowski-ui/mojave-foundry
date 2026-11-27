//! Dialogue behaviour on a hand-built IR (the testkit's random group
//! folds to one INFO per speaker): a random group drawn by `random(1, n)`
//! over its passing members, and say-once-a-day.

mod support;

use std::collections::BTreeMap;

use support::{Instance, World, load_messages};
use transpile::ir::{
    Actor, ActorKind, Cond, Dialogue, DialogueCounts, FormKey, Global, GlobalPurpose, Info,
    InfoFlags, Module, Node, Pick, Response, SimId, SimKind, SpeakerSet, SpeakerTopic, Text,
    TextId, TextRole, Topic, TopicKind, VarTy, once_sub,
};
use transpile::{ir::BaseFiles, ir::SimConfig, lower};

fn key(id: u32) -> FormKey {
    FormKey::new("hand.esm", id)
}

fn text(module: &mut Module, owner: &SimId, source: &str) -> TextId {
    module.texts.push(Text {
        owner: owner.clone(),
        source: source.into(),
        role: TextRole::Response,
    });
    TextId(module.texts.len() as u32 - 1)
}

/// One speaker whose greeting is a random group of two INFOs, the second
/// said once a day.
fn module() -> Module {
    let mut m = Module::default();
    let speaker = key(0x10);
    let topic = SimId::of(SimKind::Topic, &key(0x20));
    let quest = SimId::of(SimKind::Quest, &key(0x30));
    let infos = [key(0x21), key(0x22)];
    let lines = ["First line.", "Second line."];
    for (i, form) in infos.iter().enumerate() {
        let id = SimId::of(SimKind::Info, form);
        let t = text(&mut m, &id, lines[i]);
        m.infos.insert(
            id.clone(),
            Info {
                id: id.clone(),
                topic: topic.clone(),
                quest: quest.clone(),
                speakers: SpeakerSet::Any,
                condition: Cond::True,
                flags: InfoFlags {
                    random: true,
                    random_end: i == 1,
                    say_once_a_day: i == 1,
                    ..InfoFlags::default()
                },
                responses: vec![Response {
                    number: 1,
                    text: t,
                    emotion: (0, 0),
                }],
                prompt: None,
                choices: Vec::new(),
                link_from: Vec::new(),
                add_topics: Vec::new(),
                begin: None,
                end: None,
                check: None,
            },
        );
    }
    m.topics.insert(
        topic.clone(),
        Topic {
            id: topic.clone(),
            editor_id: "GREETING".into(),
            kind: TopicKind::Topic,
            name: None,
            priority: 0.0,
            top_level: false,
            infos: infos.iter().map(|f| SimId::of(SimKind::Info, f)).collect(),
        },
    );
    let day = SimId::of(SimKind::Info, &infos[1]).with_sub(once_sub::SAY_DAY);
    m.globals.insert(
        day.clone(),
        Global {
            id: day,
            purpose: GlobalPurpose::OnceFlag,
            name: "DAY".into(),
            ty: VarTy::Int,
            initial: -1,
        },
    );
    m.actors.insert(
        speaker.clone(),
        Actor {
            base: speaker.clone(),
            editor_id: Some("HandSpeaker".into()),
            kind: ActorKind::Npc,
            script: None,
            packages: Vec::new(),
            factions: Vec::new(),
            voice: None,
            race: None,
            class: None,
            female: false,
            essential: false,
            ai: None,
            placements: vec![key(0x40)],
        },
    );
    let nodes = infos
        .iter()
        .enumerate()
        .map(|(i, form)| Node {
            ordinal: i as u32 + 1,
            info: SimId::of(SimKind::Info, form),
            topic: topic.clone(),
            condition: Cond::True,
            prompt: None,
            choices: Vec::new(),
            failure: None,
        })
        .collect();
    m.dialogues.insert(
        speaker.clone(),
        Dialogue {
            speaker,
            editor_id: "HandSpeaker".into(),
            nodes,
            topics: BTreeMap::from([(
                topic.clone(),
                SpeakerTopic {
                    topic: topic.clone(),
                    picks: vec![Pick::Random(vec![1, 2])],
                },
            )]),
            greeting: Some(topic),
            greeting_label: false,
            hub: Vec::new(),
            counts: DialogueCounts::default(),
        },
    );
    m
}

#[test]
fn the_random_group_draws_among_passing_members_and_say_once_a_day_holds() {
    let module = module();
    let out = lower(&module, &BaseFiles::default(), &SimConfig::default()).unwrap();
    let bytes = &out.files["scripts/md000001.int"];
    let mut world = World::new(1, 13);
    world.globals[0] = -1;
    world.self_obj = 50;
    load_messages(
        &mut world,
        1,
        &out.files["text/english/dialog/md000001.msg"],
    );
    let mut speaker = Instance::load("md000001.int", bytes, &mut world);
    let mut say = |world: &mut World, draw: i32| {
        world.transcript.clear();
        world.randoms.push_back(draw);
        speaker.call(world, "talk_p_proc");
        let reply = world.replies()[0].clone();
        speaker.choose_text(world, "Goodbye.");
        reply
    };
    assert_eq!(say(&mut world, 1), "First line.");
    assert_eq!(say(&mut world, 2), "Second line.");
    assert_eq!(world.globals[0], 0, "said on day 0");
    // The same day: only the first member passes, whatever the draw.
    assert_eq!(say(&mut world, 1), "First line.");
    // The next day it can be drawn again.
    world.game_time = 864_000;
    assert_eq!(say(&mut world, 2), "Second line.");
    assert_eq!(world.globals[0], 1);
}

// ---- the greeting on a later talk (spec 4.1, 4.9) ----------------------

/// One speaker: a say-once greeting, then (when `generic`) a greeting
/// with no condition; and a top-level topic "Ask" in the hub, whose INFO
/// always passes.
fn later_talk_module(generic: bool) -> Module {
    let mut m = module();
    let speaker = key(0x10);
    let quest = SimId::of(SimKind::Quest, &key(0x30));
    let greeting = SimId::of(SimKind::Topic, &key(0x20));
    let ask = SimId::of(SimKind::Topic, &key(0x50));
    m.infos.clear();
    m.globals.clear();
    let mut lines = vec![(key(0x21), "First meeting.", greeting.clone(), true)];
    if generic {
        lines.push((key(0x22), "Back again?", greeting.clone(), false));
    }
    lines.push((key(0x51), "Ask away.", ask.clone(), false));
    for (form, line, topic, once) in &lines {
        let id = SimId::of(SimKind::Info, form);
        let t = text(&mut m, &id, line);
        m.infos.insert(
            id.clone(),
            Info {
                id: id.clone(),
                topic: topic.clone(),
                quest: quest.clone(),
                speakers: SpeakerSet::Any,
                condition: Cond::True,
                flags: InfoFlags {
                    say_once: *once,
                    ..InfoFlags::default()
                },
                responses: vec![Response {
                    number: 1,
                    text: t,
                    emotion: (0, 0),
                }],
                prompt: None,
                choices: Vec::new(),
                link_from: Vec::new(),
                add_topics: Vec::new(),
                begin: None,
                end: None,
                check: None,
            },
        );
        if *once {
            let flag = id.with_sub(once_sub::SAY_ONCE);
            m.globals.insert(
                flag.clone(),
                Global {
                    id: flag,
                    purpose: GlobalPurpose::OnceFlag,
                    name: "ONCE".into(),
                    ty: VarTy::Int,
                    initial: 0,
                },
            );
        }
    }
    let ask_name = text(&mut m, &ask, "Ask");
    let greeting_infos: Vec<SimId> = lines
        .iter()
        .filter(|l| l.2 == greeting)
        .map(|l| SimId::of(SimKind::Info, &l.0))
        .collect();
    m.topics.get_mut(&greeting).unwrap().infos = greeting_infos.clone();
    m.topics.insert(
        ask.clone(),
        Topic {
            id: ask.clone(),
            editor_id: "HandAsk".into(),
            kind: TopicKind::Topic,
            name: Some(ask_name),
            priority: 0.0,
            top_level: true,
            infos: vec![SimId::of(SimKind::Info, &key(0x51))],
        },
    );
    // Ordinals in INFO id order: 0x21, (0x22), 0x51.
    let nodes: Vec<Node> = lines
        .iter()
        .enumerate()
        .map(|(i, (form, _, topic, _))| Node {
            ordinal: i as u32 + 1,
            info: SimId::of(SimKind::Info, form),
            topic: topic.clone(),
            condition: Cond::True,
            prompt: (topic == &ask).then_some(ask_name),
            choices: Vec::new(),
            failure: None,
        })
        .collect();
    let greeting_picks = (1..=greeting_infos.len() as u32).map(Pick::Info).collect();
    let ask_ordinal = lines.len() as u32;
    let dialogue = m.dialogues.get_mut(&speaker).unwrap();
    dialogue.nodes = nodes;
    dialogue.topics = BTreeMap::from([
        (
            greeting.clone(),
            SpeakerTopic {
                topic: greeting.clone(),
                picks: greeting_picks,
            },
        ),
        (
            ask.clone(),
            SpeakerTopic {
                topic: ask.clone(),
                picks: vec![Pick::Info(ask_ordinal)],
            },
        ),
    ]);
    dialogue.hub = vec![transpile::ir::HubEntry {
        topic: ask,
        given: None,
    }];
    m
}

/// Talks twice (leaving with Goodbye); returns each talk's first reply
/// and the options it offered.
fn talk_twice(module: &Module) -> Vec<(String, Vec<String>)> {
    let out = lower(module, &BaseFiles::default(), &SimConfig::default()).unwrap();
    let bytes = &out.files["scripts/md000001.int"];
    let mut world = World::new(module.globals.len(), 13);
    world.self_obj = 50;
    load_messages(
        &mut world,
        1,
        &out.files["text/english/dialog/md000001.msg"],
    );
    let mut speaker = Instance::load("md000001.int", bytes, &mut world);
    let mut talks = Vec::new();
    for _ in 0..2 {
        world.transcript.clear();
        speaker.call(&mut world, "talk_p_proc");
        let reply = world.replies()[0].clone();
        let options = world.option_texts();
        assert!(
            !speaker.choose_text(&mut world, "Goodbye."),
            "Goodbye ends the talk"
        );
        talks.push((reply, options));
    }
    talks
}

#[test]
fn a_later_talk_says_the_generic_greeting_after_the_say_once_one() {
    let talks = talk_twice(&later_talk_module(true));
    assert_eq!(talks[0].0, "First meeting.");
    assert_eq!(
        talks[1].0, "Back again?",
        "the say-once greeting is not repeated"
    );
    assert_eq!(talks[1].1, ["Ask", "Goodbye."], "its hub");
}

#[test]
fn a_later_talk_with_no_passing_greeting_says_the_label_and_offers_the_hub() {
    let talks = talk_twice(&later_talk_module(false));
    assert_eq!(talks[0].0, "First meeting.");
    assert_eq!(
        talks[1].0,
        transpile::texts::LABELS
            .iter()
            .find(|(n, _)| *n == transpile::texts::GREETING_LABEL)
            .unwrap()
            .1,
        "the converter's greeting label, not [Continue]"
    );
    assert_ne!(talks[1].0, "[Continue]");
    assert_eq!(talks[1].1, ["Ask", "Goodbye."], "the hub, not only Goodbye");
}
