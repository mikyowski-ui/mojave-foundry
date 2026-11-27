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
