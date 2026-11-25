//! The tables revision 2 added for the simulation layer, on a synthetic
//! plugin written in both games' forms (`HEDR` 0.94 with the older
//! sub-record sizes, 1.34 with the newer): globals, game settings,
//! messages, factions, actor AI data and factions through templates, infos
//! with flags and links, package flags and events, embedded script
//! contexts, the two new reasons a reference is pointed at, and the counted
//! skips. Everything in it is invented.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use fnv_esm::fixture::{CellSpec, PluginBuilder, WorldSpec};
use fnv_esm::{
    ActorBase, AiData, Cell, CombatReaction, Comparison, Condition, Creature, Faction, FactionData,
    FactionRank, FormId, GameSetting, GameSettingValue, Global, GlobalType, Info, InfoData,
    LocalVariable, Message, Npc, Package, PackageData, PackageEvent, PlacedKind, PlacedRef, Quest,
    QuestLogEntry, QuestStage, RawRecord, RawSubrecord, Response, ScriptBody, ScriptHeader,
    ScriptReference, Static, Terminal, TerminalMenuItem, Topic, TypedRecord, World,
};
use mir::{
    ExtractOptions, FormKey, GameVariant, Mir, MirError, RawStore, Records, RefReason,
    ScriptRefKind, Slice, extract,
};

const PLUGIN: &str = "Tiny.esm";

fn key(local: u32) -> FormKey {
    FormKey::new(PLUGIN, local)
}

fn replace(raw: &mut RawRecord, sig: &[u8; 4], data: Vec<u8>) {
    let sub = raw
        .subrecords
        .iter_mut()
        .find(|s| &s.sig.0 == sig)
        .expect("subrecord to replace");
    sub.data = data;
}

fn ai(aggression: u8, radius: i32) -> AiData {
    AiData {
        aggression,
        confidence: 3,
        energy_level: 50,
        responsibility: 50,
        mood: 0,
        services: 0x7F,
        trained_skill: 0,
        trainer_level: 0,
        assistance: 1,
        aggro_radius_behaviour: 1,
        aggro_radius: radius,
    }
}

fn body(source: &str, references: Vec<ScriptReference>, names: &[&str]) -> ScriptBody {
    let mut body = ScriptBody::from_source(source);
    body.header = Some(ScriptHeader {
        ref_count: references.len() as u32,
        compiled_size: 0,
        variable_count: names.len() as u32,
        script_type: 0,
        flags: 1,
    });
    body.references = references;
    body.locals = names
        .iter()
        .enumerate()
        .map(|(i, name)| LocalVariable {
            index: i as u32 + 1,
            flags: u8::from(i % 2 == 0),
            name: Some((*name).to_owned()),
        })
        .collect();
    body
}

/// The plugin: `fo3` selects the older sizes of the 0.94 form.
fn plugin(fo3: bool) -> Vec<u8> {
    let mut b = PluginBuilder::new();
    b.version(if fo3 { 0.94 } else { 1.34 }).master_file(true);

    // Globals, settings, a message.
    b.add(Global::new(0x900u32, "TestShort", GlobalType::Short, 3.0));
    b.add(Global::new(0x901u32, "TestFloat", GlobalType::Float, 0.25));
    b.add(GameSetting::new(
        0x902u32,
        "fSpeechChallengeMultiplier",
        GameSettingValue::Float(1.5),
    ));
    b.add(GameSetting::new(
        0x903u32,
        "iSpeechChallengeDifficultyHard",
        GameSettingValue::Int(40),
    ));
    b.add(GameSetting::new(
        0x904u32,
        "sTestText",
        GameSettingValue::String("Hello".into()),
    ));
    b.add(
        Message::new(0x910u32, "TestMessage", "A title", "Some text")
            .with_flags(Message::MESSAGE_BOX)
            .with_button("Yes")
            .with_conditional_button(
                "No",
                vec![Condition::float(74, Comparison::Equal, 1.0).with_params(0x900, 0)],
            ),
    );
    b.add(
        Message::new(0x911u32, "TestNotice", "", "Quick")
            .with_flags(Message::AUTO_DISPLAY)
            .with_display_time(4),
    );

    // Factions.
    b.add(Faction::new(0x920u32, "PlayerFaction").with_relation(
        0x921u32,
        -100,
        CombatReaction::Enemy,
    ));
    b.add(
        Faction::new(0x921u32, "RaiderFaction")
            .with_flags(FactionData::EVIL, FactionData::TRACK_CRIME)
            .with_relation(0x920u32, -100, CombatReaction::Enemy),
    );
    let mut town = Faction::new(0x922u32, "TownFaction")
        .with_relation(0x920u32, 0, CombatReaction::Friend)
        .to_raw();
    if fo3 {
        // The older forms: a 1-byte DATA and an 8-byte relation.
        replace(&mut town, b"DATA", vec![0x01]);
        let mut xnam = 0x920u32.to_le_bytes().to_vec();
        xnam.extend_from_slice(&5i32.to_le_bytes());
        replace(&mut town, b"XNAM", xnam);
    }
    b.add_raw(town);

    // Actors with templates.
    let mut base = Npc::new(0x930u32, "RaiderBase", "Raider");
    base.ai = Some(ai(1, 500));
    base.voice = Some(FormId(0x940));
    base.factions = vec![FactionRank {
        faction: FormId(0x921),
        rank: 1,
    }];
    b.add(base);
    let mut child = Npc::new(0x931u32, "RaiderChild", "Raider kid");
    child.base = Some(ActorBase {
        template_flags: 0x0001 | 0x0004 | 0x0010,
        ..ActorBase::default()
    });
    child.template = Some(FormId(0x930));
    child.ai = Some(ai(0, 0));
    child.voice = Some(FormId(0x941));
    child.factions = vec![FactionRank {
        faction: FormId(0x922),
        rank: 0,
    }];
    b.add(child);
    let mut own = Npc::new(0x932u32, "OwnData", "Own");
    own.template = Some(FormId(0x930));
    own.ai = Some(ai(0, 100));
    own.voice = Some(FormId(0x941));
    b.add(own);
    let mut lost = Npc::new(0x934u32, "LostTemplate", "Lost");
    lost.base = Some(ActorBase {
        template_flags: 0x0010,
        ..ActorBase::default()
    });
    lost.ai = Some(ai(2, 0));
    b.add(lost);
    let mut beast = Creature::new(
        0x933u32,
        "TestBeast",
        "Beast",
        "creatures\\beast\\skeleton.nif",
    );
    beast.ai = Some(ai(2, 1000));
    b.add(beast);

    // Packages: 12-byte, 8-byte with the fill, and a size no layout fits.
    let general = PackageData::MUST_REACH_LOCATION | PackageData::ALWAYS_RUN;
    let events = PackageEvent::new()
        .with_idle(0x950u32)
        .with_topic(0x960u32)
        .with_script(body(
            "scn PkgBegin\r\nbegin Function {}\r\nend\r\n",
            vec![
                ScriptReference::Form(FormId(0x9A0)),
                ScriptReference::Local(2),
            ],
            &["tick", "who"],
        ));
    b.add(
        Package::new(0x951u32, "TestSandbox", 12)
            .with_data(PackageData::new(
                12,
                general,
                PackageData::HELLOS_TO_PLAYER,
                PackageData::SANDBOX_ALLOW_EATING | PackageData::SANDBOX_ALLOW_WANDERING,
            ))
            .with_on_begin(events)
            .with_on_end(PackageEvent::new().with_topic(0x961u32)),
    );
    b.add(
        Package::new(0x952u32, "TestOld", 6).with_data(PackageData::short(
            6,
            PackageData::ONCE_PER_DAY,
            PackageData::BEHAVIOUR_FILL,
        )),
    );
    let mut odd = Package::new(0x953u32, "TestOdd", 5).to_raw();
    replace(&mut odd, b"PKDT", vec![0; 10]);
    b.add_raw(odd);

    // Dialogue: a speech challenge, a plain info in the older 3-byte form.
    b.add_raw(RawRecord::new(b"AVIF", 0x970u32));
    let mut quest = Quest::new(0x980u32, "TestQuest", "A quest");
    quest.stages.push(QuestStage {
        index: 10,
        entries: vec![QuestLogEntry {
            script: body(
                "SetStage TestQuest 20",
                vec![
                    ScriptReference::Form(FormId(0x9A1)),
                    ScriptReference::Form(FormId(0xDEAD_0001)),
                ],
                &[],
            ),
            ..QuestLogEntry::new("Go")
        }],
    });
    b.add(quest);
    let mut challenge = Info::new(0x990u32, 0x980u32, vec![Response::new(1, "Fine.")])
        .with_flags(InfoData::SPEECH_CHALLENGE | InfoData::SAY_ONCE | InfoData::SAY_ONCE_A_DAY)
        .with_knam(0x970u32)
        .with_speech_challenge(4)
        .with_choices(vec![FormId(0x961), FormId(0x962)])
        .with_conditions(vec![
            Condition::float(72, Comparison::Equal, 1.0).with_params(0x930, 0),
            Condition::float(1, Comparison::Less, 400.0).with_params(0x9A0, 0),
            Condition::float(1, Comparison::Less, 100.0).with_params(0x14, 0),
        ])
        .with_result_end(body(
            "player.AddItem Caps 5",
            vec![ScriptReference::Form(FormId(0x9A2))],
            &[],
        ));
    // A subrecord nothing reads.
    challenge
        .unknown
        .push(RawSubrecord::new(b"SNDD", [1, 2, 3, 4]));
    let mut plain_info = Info::new(0x991u32, 0x980u32, vec![Response::new(1, "Hm.")])
        .with_flags(InfoData::GOODBYE)
        .with_conditions(vec![
            Condition::float(136, Comparison::Equal, 1.0).with_params(0x9A1, 0),
        ]);
    if fo3 {
        plain_info.data = Some(InfoData::short_form(0, 0, 0x01));
    }
    b.topic(
        Topic::new(0x961u32, "TestTopic", "Ask about it", 0x980u32),
        vec![challenge, plain_info],
    );
    b.topic(
        Topic::new(0x962u32, "OtherTopic", "Other", 0x980u32),
        Vec::new(),
    );

    // A terminal with a script.
    let mut terminal = Terminal::new(0x9C0u32, "TestTerminal", "Terminal", "HELLO");
    terminal
        .items
        .push(TerminalMenuItem::new("Open", "Opened."));
    terminal.items.push(TerminalMenuItem {
        script: body(
            "Door.Unlock",
            vec![ScriptReference::Form(FormId(0x9A1))],
            &["opened"],
        ),
        ..TerminalMenuItem::new("Unlock", "Unlocked.")
    });
    b.add(terminal);

    // References: two doors that teleport to each other's destination,
    // one plain reference, and a base to place.
    b.add(Static::new(0x9B0u32, "TestBase", "test\\base.nif"));
    b.world(
        WorldSpec::new(World::new(0x800u32, "TinyWorld")).with_cell(
            CellSpec::new(Cell::exterior(0x801u32, 0, 0))
                .with_temporary(PlacedRef::new(
                    PlacedKind::Object,
                    0x9A0u32,
                    0x9B0u32,
                    [100.0, 100.0, 0.0],
                    [0.0; 3],
                ))
                .with_temporary(
                    PlacedRef::new(
                        PlacedKind::Object,
                        0x9A1u32,
                        0x9B0u32,
                        [200.0, 100.0, 0.0],
                        [0.0; 3],
                    )
                    .with_teleport(0x9A3u32, [0.0; 3], [0.0; 3]),
                )
                .with_temporary(PlacedRef::new(
                    PlacedKind::Object,
                    0x9A2u32,
                    0x9B0u32,
                    [300.0, 100.0, 0.0],
                    [0.0; 3],
                ))
                .with_temporary(PlacedRef::new(
                    PlacedKind::Object,
                    0x9A3u32,
                    0x9B0u32,
                    [400.0, 100.0, 0.0],
                    [0.0; 3],
                ))
                .with_temporary(PlacedRef::new(
                    PlacedKind::Object,
                    0x9A4u32,
                    0x9B0u32,
                    [500.0, 100.0, 0.0],
                    [0.0; 3],
                )),
        ),
    );
    b.build()
}

struct Run {
    _install: tempfile::TempDir,
    out: PathBuf,
    records: Records,
}

fn run(fo3: bool) -> Run {
    let install = tempfile::tempdir().unwrap();
    let data = install.path().join("Data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join(PLUGIN), plugin(fo3)).unwrap();
    let out = install.path().join("mir");
    let options = ExtractOptions {
        data_dir: data,
        plugins: vec![PLUGIN.to_owned()],
        out: out.clone(),
        slice: Slice {
            name: "tiny".to_owned(),
            worldspace: "TinyWorld".to_owned(),
            x: 0..=0,
            y: 0..=0,
            interior_depth: 0,
        },
        jobs: 2,
        raw_store: RawStore::All,
    };
    let summary = extract(&options).unwrap();
    assert_eq!(summary.bad_records, 0);
    let mir = Mir::open(&out).unwrap();
    assert_eq!(
        mir.game(),
        if fo3 {
            GameVariant::Fallout3
        } else {
            GameVariant::FalloutNewVegas
        }
    );
    let records = mir.records().unwrap();
    Run {
        _install: install,
        out,
        records,
    }
}

fn both(test: impl Fn(&Run, bool)) {
    for fo3 in [true, false] {
        test(&run(fo3), fo3);
    }
}

#[test]
fn globals_settings_and_messages_read_back() {
    both(|r, _| {
        let records = &r.records;
        let globals = records.globals().unwrap();
        assert_eq!(globals.len(), 2);
        let short = records.global(&key(0x900)).unwrap().unwrap();
        assert_eq!(short.editor_id.as_deref(), Some("TestShort"));
        assert_eq!(short.kind, Some(GlobalType::Short));
        assert_eq!(short.value, Some(3.0));
        let float = records.global_by_name("testfloat").unwrap().unwrap();
        assert_eq!(
            (float.kind, float.value),
            (Some(GlobalType::Float), Some(0.25))
        );

        assert_eq!(records.game_settings().unwrap().len(), 3);
        assert_eq!(
            records
                .game_setting_f32("fSpeechChallengeMultiplier")
                .unwrap(),
            Some(1.5)
        );
        assert_eq!(
            records
                .game_setting_i64("iSpeechChallengeDifficultyHard")
                .unwrap(),
            Some(40)
        );
        assert_eq!(
            records.game_setting("sTestText").unwrap().unwrap().value,
            Some(GameSettingValue::String("Hello".into()))
        );
        assert_eq!(records.game_setting_f32("sTestText").unwrap(), None);
        assert_eq!(records.game_setting_f32("fNoSuchThing").unwrap(), None);

        let messages = records.messages().unwrap();
        assert_eq!(messages.len(), 2);
        let message = records.message(&key(0x910)).unwrap().unwrap();
        assert_eq!(message.title.as_deref(), Some("A title"));
        assert_eq!(message.text.as_deref(), Some("Some text"));
        assert!(message.is_message_box() && !message.is_auto_display());
        assert_eq!(message.buttons.len(), 2);
        assert_eq!(message.buttons[0].text.as_deref(), Some("Yes"));
        assert!(message.buttons[0].conditions.is_empty());
        assert_eq!(message.buttons[1].conditions.len(), 1);
        assert_eq!(message.buttons[1].conditions[0].function, 74);
        assert_eq!(message.buttons[1].conditions[0].context, "button 1");
        let notice = records.message(&key(0x911)).unwrap().unwrap();
        assert!(notice.is_auto_display());
        assert_eq!(notice.display_time, Some(4));
        assert_eq!(notice.icon, None);
    });
}

#[test]
fn factions_have_flags_and_relations_in_both_forms() {
    both(|r, fo3| {
        let records = &r.records;
        assert_eq!(records.factions().unwrap().len(), 3);
        let player = records.faction_by_name("playerfaction").unwrap().unwrap();
        assert_eq!(player.form, key(0x920));
        assert_eq!(player.relations.len(), 1);
        assert_eq!(
            player.reaction_towards(&key(0x921)),
            Some(CombatReaction::Enemy)
        );
        assert_eq!(player.relations[0].modifier, -100);
        let raiders = records.faction(&key(0x921)).unwrap().unwrap();
        assert!(raiders.is_evil() && raiders.tracks_crime() && !raiders.is_hidden_from_pc());
        let town = records.faction(&key(0x922)).unwrap().unwrap();
        if fo3 {
            // The 1-byte DATA and the 8-byte relation of the older forms.
            assert_eq!(town.flags, Some(1));
            assert_eq!(town.flags2, None);
            assert!(town.is_hidden_from_pc() && !town.tracks_crime());
            assert_eq!(town.relations[0].combat_reaction, None);
            assert_eq!(town.relations[0].modifier, 5);
            assert_eq!(town.reaction_towards(&key(0x920)), None);
        } else {
            assert_eq!(town.flags2, Some(0));
            assert_eq!(
                town.reaction_towards(&key(0x920)),
                Some(CombatReaction::Friend)
            );
        }
        assert!(records.faction(&key(0xFFF)).unwrap().is_none());
        assert!(records.faction_by_name("NoSuchFaction").unwrap().is_none());
    });
}

#[test]
fn actor_ai_and_factions_follow_templates() {
    both(|r, _| {
        let records = &r.records;
        assert_eq!(records.actor_ais().unwrap().len(), 5);
        // The child takes AI data, voice and factions from its template.
        let child = records.actor_ai(&key(0x931)).unwrap().unwrap();
        assert_eq!(child.kind, "npc");
        let data = child.ai.expect("AIDT");
        assert_eq!((data.aggression, data.aggro_radius), (1, 500));
        assert_eq!(child.ai_from, Some(key(0x930)));
        assert_eq!(child.voice, Some(key(0x940)));
        assert_eq!(child.voice_from, Some(key(0x930)));
        let factions = records.actor_factions(&key(0x931)).unwrap();
        assert_eq!(factions.len(), 1);
        assert_eq!(factions[0].faction, Some(key(0x921)));
        assert_eq!((factions[0].rank, &factions[0].from), (1, &key(0x930)));
        // Without the flags, the actor's own data stays.
        let own = records.actor_ai(&key(0x932)).unwrap().unwrap();
        assert_eq!(own.ai.map(|a| a.aggro_radius), Some(100));
        assert_eq!(own.ai_from, Some(key(0x932)));
        assert_eq!(own.voice, Some(key(0x941)));
        assert!(records.actor_factions(&key(0x932)).unwrap().is_empty());
        // The AI flag with no template: own data, counted.
        let lost = records.actor_ai(&key(0x934)).unwrap().unwrap();
        assert_eq!(lost.ai.map(|a| a.aggression), Some(2));
        assert_eq!(lost.voice, None);
        // A creature.
        let beast = records.actor_ai(&key(0x933)).unwrap().unwrap();
        assert_eq!(beast.kind, "creature");
        assert_eq!(beast.ai.map(|a| a.aggro_radius), Some(1000));
        assert!(records.actor_ai(&key(0xFFF)).unwrap().is_none());
    });
}

#[test]
fn infos_have_flags_links_and_conditions() {
    both(|r, fo3| {
        let records = &r.records;
        let info = records.info(&key(0x990)).unwrap().unwrap();
        let data = info.data();
        assert!(data.is_speech_challenge() && data.is_say_once() && data.is_say_once_a_day());
        assert!(!data.is_goodbye() && !data.is_always_darken());
        assert_eq!(info.flag_bytes, 2);
        assert_eq!(info.knam, Some(key(0x970)));
        assert_eq!(info.knam_sig.as_deref(), Some("AVIF"));
        assert_eq!(info.speech_challenge, Some(4));
        assert_eq!(info.choices, [key(0x961), key(0x962)]);
        assert_eq!(info.responses.len(), 1);
        assert_eq!(info.responses[0].text.as_deref(), Some("Fine."));
        assert_eq!(info.conditions.len(), 3);
        assert_eq!(info.conditions[2].param1, 0x14);
        assert_eq!(info.conditions[2].param1_ref, None);
        assert_eq!(info.conditions[1].param1_ref, Some(key(0x9A0)));
        assert_eq!(info.conditions[0].param1_ref, None);
        assert_eq!(info.topic, Some(key(0x961)));

        let plain = records.info(&key(0x991)).unwrap().unwrap();
        assert!(plain.data().is_goodbye());
        assert!(!plain.data().is_say_once_a_day());
        assert_eq!(plain.flag_bytes, if fo3 { 1 } else { 2 });
        assert_eq!(plain.data().has_second_flag_byte(), !fo3);
        assert_eq!(plain.knam, None);

        let rows = records.topic_info_rows(&key(0x961)).unwrap();
        assert_eq!(
            rows.iter().map(|i| i.form.clone()).collect::<Vec<_>>(),
            [key(0x990), key(0x991)],
            "file order"
        );
        assert!(records.info(&key(0xFFF)).unwrap().is_none());
    });
}

#[test]
fn packages_have_flags_and_events() {
    both(|r, _| {
        let records = &r.records;
        let sandbox = records.package(&key(0x951)).unwrap().unwrap();
        assert_eq!(sandbox.pkdt_size, Some(12));
        assert_eq!(sandbox.behaviour_flags, Some(PackageData::HELLOS_TO_PLAYER));
        assert_eq!(
            sandbox.type_flags,
            Some(PackageData::SANDBOX_ALLOW_EATING | PackageData::SANDBOX_ALLOW_WANDERING)
        );
        let data = sandbox.data().expect("PKDT");
        assert!(data.must_reach_location() && data.always_run() && !data.must_complete());
        assert_eq!(
            data.sandbox_flag_names(),
            ["allow eating", "allow wandering"]
        );
        assert_eq!(sandbox.events.len(), 2);
        assert_eq!(sandbox.events[0].event, "on begin");
        assert_eq!(sandbox.events[0].idle, Some(key(0x950)));
        assert_eq!(sandbox.events[0].topic, Some(key(0x960)));
        assert!(
            sandbox.events[0]
                .script_source
                .as_deref()
                .is_some_and(|s| s.starts_with("scn PkgBegin"))
        );
        assert_eq!(sandbox.events[1].event, "on end");
        assert_eq!(sandbox.events[1].topic, Some(key(0x961)));
        assert_eq!(sandbox.events[1].script_source, None);
        // The 8-byte form and its fill.
        let old = records.package(&key(0x952)).unwrap().unwrap();
        assert_eq!(old.pkdt_size, Some(8));
        assert_eq!(old.type_flags, None);
        assert_eq!(old.behaviour_flags, None);
        assert_eq!(old.behaviour_flags_raw, Some(PackageData::BEHAVIOUR_FILL));
        assert!(old.data().expect("PKDT").once_per_day());
        // A PKDT of no known size reads as no data.
        let odd = records.package(&key(0x953)).unwrap().unwrap();
        assert_eq!((odd.flags, odd.pkdt_size), (None, None));
    });
}

#[test]
fn script_contexts_keep_the_whole_reference_list() {
    both(|r, _| {
        let records = &r.records;
        // A package event: a form, then a local variable.
        let ctx = records
            .script_context(&key(0x951), "on begin")
            .unwrap()
            .unwrap();
        assert_eq!(ctx.references.len(), 2);
        assert_eq!(ctx.references[0].index, 1);
        assert_eq!(ctx.references[0].form(), Some(&key(0x9A0)));
        assert_eq!(ctx.references[0].entry, ScriptRefKind::Form(0x9A0));
        assert_eq!(ctx.references[1].index, 2);
        assert_eq!(ctx.references[1].entry, ScriptRefKind::Local(2));
        assert_eq!(ctx.form_at(1), Some(&key(0x9A0)));
        assert_eq!(ctx.form_at(2), None);
        assert_eq!(ctx.locals.len(), 2);
        assert_eq!(ctx.local("WHO").map(|l| l.index), Some(2));
        assert!(ctx.locals[0].is_integer() && !ctx.locals[1].is_integer());
        assert_eq!(ctx.ref_count, Some(2));
        assert_eq!(ctx.variable_count, Some(2));
        assert!(ctx.has_source && !ctx.has_compiled);
        assert!(
            ctx.source
                .as_deref()
                .is_some_and(|s| s.contains("PkgBegin"))
        );
        assert_eq!(records.script_contexts(&key(0x951)).unwrap().len(), 1);

        // A quest stage: a form outside the plugin list keeps its place.
        let stage = records
            .script_context(&key(0x980), "stage 10 entry 0")
            .unwrap()
            .unwrap();
        assert_eq!(stage.references.len(), 2);
        assert_eq!(stage.references[0].form(), Some(&key(0x9A1)));
        assert_eq!(stage.references[1].form(), None);
        assert_eq!(stage.references[1].entry, ScriptRefKind::Form(0xDEAD_0001));
        // Dialogue results and a terminal item.
        let result = records
            .script_context(&key(0x990), "result end")
            .unwrap()
            .unwrap();
        assert_eq!(result.form_at(1), Some(&key(0x9A2)));
        assert!(
            records
                .script_context(&key(0x990), "result begin")
                .unwrap()
                .is_none()
        );
        let item = records
            .script_context(&key(0x9C0), "item 1")
            .unwrap()
            .unwrap();
        assert_eq!(item.local("opened").map(|l| l.flags), Some(1));
        // The older script_refs table still lists the forms.
        assert_eq!(
            records.script_references(&key(0x9C0)).unwrap(),
            [("item 1".to_owned(), key(0x9A1))]
        );
        assert!(records.script_contexts(&key(0xFFF)).unwrap().is_empty());
    });
}

#[test]
fn condition_parameters_and_teleports_are_referenced() {
    both(|r, _| {
        let records = &r.records;
        let reasons = records.referenced_ref_reasons().unwrap();
        let of = |local: u32| reasons.get(&key(local)).cloned().unwrap_or_default();
        // 0x9A0: a script reference of a package, a GetDistance parameter.
        assert!(of(0x9A0).contains(&RefReason::ConditionParam));
        assert!(of(0x9A0).contains(&RefReason::Script));
        // 0x9A1: GetIsReference parameter, script references, teleports.
        assert!(of(0x9A1).contains(&RefReason::ConditionParam));
        // 0x9A3: the destination of 0x9A1's teleport.
        assert_eq!(of(0x9A3), BTreeSet::from([RefReason::TeleportDestination]));
        // 0x9A4 is pointed at by nothing.
        assert!(of(0x9A4).is_empty());
        let referrals = records.ref_referrals().unwrap();
        let tele = referrals
            .iter()
            .find(|x| x.reason == RefReason::TeleportDestination)
            .unwrap();
        assert_eq!(
            (tele.reference.clone(), tele.by.clone()),
            (key(0x9A3), key(0x9A1))
        );
        let param = referrals
            .iter()
            .find(|x| x.reason == RefReason::ConditionParam && x.reference == key(0x9A0))
            .unwrap();
        assert_eq!(param.by, key(0x990));
        assert_eq!(param.context, "info parameter 1");
        // The declutter set keeps its old meaning: no revision 2 reason.
        let legacy = records.referenced_refs().unwrap();
        let all = records.referenced_refs_all().unwrap();
        assert!(!legacy.contains(&key(0x9A3)), "{legacy:?}");
        assert!(all.contains(&key(0x9A3)));
        assert!(legacy.is_subset(&all));
        // 0x9A0 is in the legacy set anyway, by a script.
        assert!(legacy.contains(&key(0x9A0)));
        for reason in [RefReason::ConditionParam, RefReason::TeleportDestination] {
            assert!(!reason.is_legacy());
        }
        assert!(RefReason::Script.is_legacy());
    });
}

#[test]
fn what_could_not_be_used_is_counted_by_name() {
    both(|r, fo3| {
        let skips = r.records.decode_skips().unwrap();
        let count = |area: &str, reason: &str| {
            skips
                .iter()
                .find(|s| s.area == area && s.reason.contains(reason))
                .map_or(0, |s| s.count)
        };
        // GetDistance from the player, who has no record.
        assert_eq!(count("CTDA/parameter", "names the player"), 1);
        assert_eq!(count("CTDA/parameter", "not a placed reference"), 0);
        // The size no PKDT layout fits.
        assert_eq!(count("PACK/PKDT", "size 10"), 1);
        // The 8-byte form and its fill.
        assert_eq!(count("PACK/PKDT", "8-byte form"), 1);
        assert_eq!(count("PACK/PKDT", "0xCDCD"), 1);
        // The older info and faction forms of Fallout 3.
        assert_eq!(count("INFO/DATA", "3-byte form"), u64::from(fo3));
        assert_eq!(count("FACT/DATA", "1-byte form"), u64::from(fo3));
        assert_eq!(count("FACT/XNAM", "8-byte relation"), u64::from(fo3));
        // An AI-data flag with no template.
        assert_eq!(count("NPC_/TPLT", "AI data"), 1);
        assert_eq!(count("NPC_/VTCK", "no voice type"), 1);
        assert_eq!(count("CREA/VTCK", "no voice type"), 1);
        // A subrecord of an info that nothing reads.
        assert_eq!(count("INFO/SNDD", "not decoded"), 1);
        // Every row is sorted and has a count.
        let mut sorted = skips.clone();
        sorted.sort_by(|a, b| (&a.area, &a.reason).cmp(&(&b.area, &b.reason)));
        assert_eq!(skips, sorted);
        assert!(skips.iter().all(|s| s.count > 0));
    });
}

#[test]
fn the_tables_are_the_same_for_every_job_count() {
    let tables = |jobs: usize| {
        let install = tempfile::tempdir().unwrap();
        let data = install.path().join("Data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join(PLUGIN), plugin(false)).unwrap();
        let options = ExtractOptions {
            data_dir: data,
            plugins: vec![PLUGIN.to_owned()],
            out: install.path().join("mir"),
            slice: Slice {
                name: "tiny".to_owned(),
                worldspace: "TinyWorld".to_owned(),
                x: 0..=0,
                y: 0..=0,
                interior_depth: 0,
            },
            jobs,
            raw_store: RawStore::All,
        };
        extract(&options).unwrap();
        let mir = Mir::open(&options.out).unwrap();
        let records = mir.records().unwrap();
        (
            records.actor_ais().unwrap(),
            records.decode_skips().unwrap(),
            records.script_contexts(&key(0x951)).unwrap(),
            records.referenced_ref_reasons().unwrap(),
        )
    };
    assert_eq!(tables(1), tables(4));
}

#[test]
fn a_file_without_the_tables_says_which_is_missing() {
    let r = run(false);
    // Open the SQLite file by hand and drop a revision 2 table: the typed
    // query reports it.
    let path = r.out.join("records.sqlite");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE globals; DROP TABLE decode_skips;")
            .unwrap();
    }
    let records = Records::open(&path).unwrap();
    assert!(matches!(
        records.globals(),
        Err(MirError::MissingTable("globals"))
    ));
    assert!(matches!(
        records.decode_skips(),
        Err(MirError::MissingTable("decode_skips"))
    ));
    // The rest still reads.
    assert_eq!(records.game_settings().unwrap().len(), 3);
}
