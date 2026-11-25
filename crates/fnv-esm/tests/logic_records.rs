//! The records and fields added for the simulation layer: package flags,
//! info flags and `KNAM`, globals, game settings, messages, factions, AI
//! data, the condition parameters that name references and the embedded
//! script contexts. All data is invented; both the 0.94 and the 1.34 forms
//! of the sub-records that differ are covered.

use fnv_esm::*;

fn sub(sig: &[u8; 4], data: &[u8]) -> RawSubrecord {
    RawSubrecord::new(sig, data)
}

/// Encodes and decodes again; asserts nothing was left unknown and the
/// round trip is exact.
fn round_trip(record: impl Into<Record>) -> Record {
    let record = record.into();
    let raw = record.to_raw();
    let decoded = raw.decode();
    assert_eq!(decoded, record, "{} did not round-trip", record.sig());
    assert!(decoded.unknown_subrecords().is_empty());
    assert_eq!(decoded.to_raw(), raw);
    decoded
}

fn pkdt(general: u32, package_type: u8, tail: &[u8]) -> Vec<u8> {
    let mut data = general.to_le_bytes().to_vec();
    data.push(package_type);
    data.extend_from_slice(tail);
    data
}

fn decode(sig: &[u8; 4], id: u32, subs: Vec<RawSubrecord>) -> Record {
    RawRecord {
        sig: Sig::new(sig),
        meta: RecordMeta::new(id),
        subrecords: subs,
    }
    .decode()
}

// ---------------------------------------------------------------- packages

#[test]
fn a_twelve_byte_pkdt_has_all_three_flag_words() {
    let general = PackageData::MUST_REACH_LOCATION | PackageData::ALWAYS_RUN | (1 << 28);
    let data = PackageData::new(
        PackageData::TYPE_SANDBOX,
        general,
        PackageData::HELLOS_TO_PLAYER | PackageData::ALLOW_IDLE_CHATTER,
        PackageData::SANDBOX_ALLOW_EATING | PackageData::SANDBOX_ALLOW_WANDERING,
    );
    assert!(!data.is_short_form());
    assert!(data.must_reach_location());
    assert!(data.always_run());
    assert!(!data.always_sneak());
    assert!(!data.must_complete());
    assert_eq!(
        data.general_flag_names(),
        ["must reach location", "always run", "no warn/attack"]
    );
    assert_eq!(
        data.behaviour_flags(),
        Some(PackageData::HELLOS_TO_PLAYER | PackageData::ALLOW_IDLE_CHATTER)
    );
    assert_eq!(
        data.behaviour_flag_names(),
        ["hellos to player", "allow idle chatter"]
    );
    assert_eq!(
        data.sandbox_flag(PackageData::SANDBOX_ALLOW_EATING),
        Some(true)
    );
    assert_eq!(
        data.sandbox_flag(PackageData::SANDBOX_ALLOW_SLEEPING),
        Some(false)
    );
    assert_eq!(
        data.sandbox_flag_names(),
        ["allow eating", "allow wandering"]
    );

    let package = Package::new(0x0100_2000, "TestSandbox", 12).with_data(data.clone());
    let decoded = round_trip(package);
    let Record::Package(decoded) = decoded else {
        panic!("not a package")
    };
    assert_eq!(decoded.data, Some(data));
}

#[test]
fn an_eight_byte_pkdt_has_no_type_flags() {
    let data = PackageData::short(
        PackageData::TYPE_SANDBOX,
        PackageData::ONCE_PER_DAY,
        PackageData::AVOID_RADIATION,
    );
    assert!(data.is_short_form());
    assert_eq!(data.type_flags(), None);
    assert_eq!(data.sandbox_flag(PackageData::SANDBOX_ALLOW_EATING), None);
    assert!(data.sandbox_flag_names().is_empty());
    assert_eq!(data.behaviour_flags(), Some(PackageData::AVOID_RADIATION));
    let bytes = pkdt(data.flags, 12, &[0, 0x00, 0x01]);
    let record = decode(b"PACK", 0x0100_2001, vec![sub(b"PKDT", &bytes)]);
    let Record::Package(package) = &record else {
        panic!("not a package")
    };
    assert_eq!(package.data.as_ref(), Some(&data));
    assert!(record.unknown_subrecords().is_empty());
    round_trip(Package::new(0x0100_2002, "TestShort", 12).with_data(data));
}

#[test]
fn uninitialised_fill_in_the_behaviour_flags_is_not_a_flag_set() {
    // The editor's 0xCD fill: byte 5, the behaviour word and the last two
    // bytes.
    let bytes = pkdt(0x1000, 6, &[0xCD, 0xCD, 0xCD, 0x00, 0x00, 0xCD, 0xCD]);
    let record = decode(b"PACK", 0x0100_2003, vec![sub(b"PKDT", &bytes)]);
    let Record::Package(package) = record else {
        panic!("not a package")
    };
    let data = package.data.expect("PKDT");
    assert_eq!(data.behaviour_flags_raw(), Some(0xCDCD));
    assert_eq!(data.behaviour_flags(), None);
    assert!(data.behaviour_flag_names().is_empty());
    assert_eq!(data.type_flags(), Some(0));
    assert_eq!(data.package_type, 6);
    assert_eq!(data.type_name(), Some("travel"));
}

#[test]
fn other_pkdt_sizes_stay_unknown() {
    for len in [0usize, 4, 7, 9, 11, 13, 16] {
        let bytes = vec![3u8; len];
        let record = decode(b"PACK", 0x0100_2004, vec![sub(b"PKDT", &bytes)]);
        let Record::Package(package) = &record else {
            panic!("not a package")
        };
        assert!(package.data.is_none(), "size {len}");
        assert_eq!(record.unknown_subrecords(), [sub(b"PKDT", &bytes)]);
    }
}

#[test]
fn package_events_keep_idle_topic_and_script() {
    let begin = PackageEvent::new()
        .with_idle(0x0100_2100u32)
        .with_topic(0x0100_2101u32)
        .with_script_source("scn TestPkgBegin\r\nbegin Function {}\r\nend\r\n");
    let end = PackageEvent::new().with_topic(0x0100_2102u32);
    let package = Package::new(0x0100_2005, "TestEvents", 5)
        .with_on_begin(begin.clone())
        .with_on_end(end.clone());
    let Record::Package(decoded) = round_trip(package) else {
        panic!("not a package")
    };
    assert_eq!(decoded.on_begin, Some(begin));
    assert_eq!(decoded.on_end, Some(end));
    assert_eq!(decoded.on_change, None);
}

// ---------------------------------------------------------------- dialogue

#[test]
fn info_flags_have_names_in_both_data_forms() {
    let all = InfoData::GOODBYE
        | InfoData::RANDOM
        | InfoData::SAY_ONCE
        | InfoData::RUN_IMMEDIATELY
        | InfoData::INFO_REFUSAL
        | InfoData::RANDOM_END
        | InfoData::SPEECH_CHALLENGE
        | InfoData::SAY_ONCE_A_DAY
        | InfoData::ALWAYS_DARKEN;
    let data = InfoData::new(0, 1, all);
    assert!(data.has_second_flag_byte());
    assert!(data.is_goodbye() && data.is_random() && data.is_say_once());
    assert!(data.is_run_immediately() && data.is_info_refusal() && data.is_random_end());
    assert!(data.is_speech_challenge() && data.is_say_once_a_day() && data.is_always_darken());
    assert!(!data.is_run_for_rumors());
    assert_eq!(InfoData::SPEECH_CHALLENGE, 0x80);

    // The 4-byte form straight from bytes: first byte 0x80, second 0x01.
    let record = decode(
        b"INFO",
        0x0100_2200,
        vec![
            sub(b"DATA", &[2, 1, 0x80, 0x01]),
            sub(b"DNAM", &4u32.to_le_bytes()),
            sub(b"KNAM", &0x0100_2210u32.to_le_bytes()),
        ],
    );
    let Record::Info(info) = &record else {
        panic!("not an info")
    };
    let data = info.data.expect("DATA");
    assert!(data.is_speech_challenge() && data.is_say_once_a_day());
    assert!(!data.is_always_darken());
    assert_eq!((data.info_type, data.next_speaker), (2, 1));
    assert_eq!(info.knam, Some(FormId(0x0100_2210)));
    assert_eq!(info.speech_challenge, Some(4));
    assert_eq!(speech_difficulty_name(4), Some("Hard"));
    assert_eq!(speech_difficulty_name(0), None);
    assert!(record.unknown_subrecords().is_empty());

    // The 3-byte form has no second flag byte.
    let record = decode(b"INFO", 0x0100_2201, vec![sub(b"DATA", &[0, 0, 0x84])]);
    let Record::Info(info) = &record else {
        panic!("not an info")
    };
    let data = info.data.expect("DATA");
    assert!(data.short && !data.has_second_flag_byte());
    assert!(data.is_speech_challenge() && data.is_say_once());
    assert!(!data.is_say_once_a_day());
    assert_eq!(
        record.to_raw().subrecords[0],
        sub(b"DATA", &[0, 0, 0x84]),
        "the 3-byte form is written back as it was"
    );
}

#[test]
fn an_info_round_trips_with_knam_and_flags() {
    let info = Info::new(
        0x0100_2202,
        0x0100_2203u32,
        vec![Response::new(1, "Hello.")],
    )
    .with_flags(InfoData::RANDOM | InfoData::SAY_ONCE_A_DAY)
    .with_knam(0x0100_2204u32)
    .with_speech_challenge(2);
    let Record::Info(decoded) = round_trip(info.clone()) else {
        panic!("not an info")
    };
    assert_eq!(decoded, info);
    assert!(decoded.data.expect("DATA").is_say_once_a_day());
}

#[test]
fn a_repeated_or_short_knam_stays_unknown() {
    let record = decode(
        b"INFO",
        0x0100_2205,
        vec![
            sub(b"KNAM", &[1, 2, 3]),
            sub(b"KNAM", &7u32.to_le_bytes()),
            sub(b"KNAM", &8u32.to_le_bytes()),
        ],
    );
    let Record::Info(info) = &record else {
        panic!("not an info")
    };
    assert_eq!(info.knam, Some(FormId(7)));
    assert_eq!(record.unknown_subrecords().len(), 2);
}

// ----------------------------------------------------------- small records

#[test]
fn globals_have_a_type_and_a_value() {
    for ty in [GlobalType::Short, GlobalType::Long, GlobalType::Float] {
        let Record::Global(global) = round_trip(Global::new(0x0100_2300, "TestGlobal", ty, 3.5))
        else {
            panic!("not a global")
        };
        assert_eq!(global.value_type, Some(ty));
        assert_eq!(global.value, Some(3.5));
    }
    assert_eq!(GlobalType::from_code(b's'), GlobalType::Short);
    assert_eq!(GlobalType::from_code(b'x'), GlobalType::Other(b'x'));
    assert!(GlobalType::Long.is_integer());
    // A two-byte FNAM is not a type.
    let record = decode(b"GLOB", 0x0100_2301, vec![sub(b"FNAM", b"sl")]);
    assert_eq!(record.unknown_subrecords().len(), 1);
}

#[test]
fn settings_are_typed_by_their_name() {
    let settings = [
        GameSetting::new(0x0100_2400, "fTestFloat", GameSettingValue::Float(1.25)),
        GameSetting::new(0x0100_2401, "iTestInt", GameSettingValue::Int(-4)),
        GameSetting::new(
            0x0100_2402,
            "sTestText",
            GameSettingValue::String("Wait".into()),
        ),
        GameSetting::new(0x0100_2403, "uTestUnsigned", GameSettingValue::Unsigned(9)),
        GameSetting::new(0x0100_2404, "bTestFlag", GameSettingValue::Bool(true)),
    ];
    for setting in settings {
        let expect = setting.value.clone();
        let Record::GameSetting(decoded) = round_trip(setting) else {
            panic!("not a setting")
        };
        assert_eq!(decoded.value, expect);
    }
    let value = GameSettingValue::Int(7);
    assert_eq!(
        (value.as_f32(), value.as_i64(), value.kind()),
        (Some(7.0), Some(7), "int")
    );
    // A name with no known type letter, or a size the type does not fit,
    // keeps its DATA.
    let record = decode(
        b"GMST",
        0x0100_2405,
        vec![sub(b"EDID", b"xOdd\0"), sub(b"DATA", &1u32.to_le_bytes())],
    );
    assert_eq!(record.unknown_subrecords().len(), 1);
    let record = decode(
        b"GMST",
        0x0100_2406,
        vec![sub(b"EDID", b"fShort\0"), sub(b"DATA", &[1, 2])],
    );
    assert_eq!(record.unknown_subrecords().len(), 1);
}

#[test]
fn a_message_has_text_buttons_and_conditions_on_them() {
    let shown = Condition::float(74, Comparison::Equal, 1.0).with_params(0x0100_2501, 0);
    let message = Message::new(0x0100_2500, "TestMessage", "Title", "Body text")
        .with_flags(Message::MESSAGE_BOX | Message::AUTO_DISPLAY)
        .with_display_time(5)
        .with_button("Yes")
        .with_conditional_button("Maybe", vec![shown, shown.or()])
        .with_button("No");
    let Record::Message(decoded) = round_trip(message.clone()) else {
        panic!("not a message")
    };
    assert_eq!(decoded.title(), Some("Title"));
    assert!(decoded.is_message_box() && decoded.is_auto_display());
    assert_eq!(decoded.buttons.len(), 3);
    assert_eq!(decoded.buttons[1].conditions.len(), 2);
    assert_eq!(decoded, message);
    // A condition before any button belongs to nothing and stays unknown.
    let record = decode(
        b"MESG",
        0x0100_2502,
        vec![sub(b"CTDA", &shown.bytes_for_test())],
    );
    assert_eq!(record.unknown_subrecords().len(), 1);
}

#[test]
fn a_faction_has_flags_and_relations_in_every_stored_form() {
    let faction = Faction::new(0x0100_2600, "TestFaction")
        .with_flags(
            FactionData::EVIL | FactionData::SPECIAL_COMBAT,
            FactionData::TRACK_CRIME,
        )
        .with_relation(0x0100_2601u32, -10, CombatReaction::Enemy)
        .with_relation(0x0100_2602u32, 5, CombatReaction::Friend);
    let Record::Faction(decoded) = round_trip(faction.clone()) else {
        panic!("not a faction")
    };
    assert!(decoded.is_evil() && decoded.has_special_combat() && decoded.tracks_crime());
    assert!(!decoded.is_hidden_from_pc() && !decoded.allows_sell());
    assert_eq!(decoded.relations[0].reaction(), Some(CombatReaction::Enemy));
    assert_eq!(
        decoded.relations[1].reaction().map(CombatReaction::name),
        Some("friend")
    );
    assert_eq!(decoded, faction);

    // The 1-byte DATA and the 8-byte XNAM of the older forms.
    let mut xnam = 0x0100_2603u32.to_le_bytes().to_vec();
    xnam.extend_from_slice(&3i32.to_le_bytes());
    let record = decode(
        b"FACT",
        0x0100_2604,
        vec![sub(b"DATA", &[1]), sub(b"XNAM", &xnam)],
    );
    let Record::Faction(old) = &record else {
        panic!("not a faction")
    };
    assert!(old.is_hidden_from_pc());
    assert!(!old.tracks_crime());
    assert_eq!(old.data.expect("DATA").flags2, None);
    assert_eq!(old.relations[0].combat_reaction, None);
    assert_eq!(old.relations[0].modifier, 3);
    assert!(record.unknown_subrecords().is_empty());
    assert_eq!(record.to_raw().decode(), record);
    // Other sizes stay unknown.
    let record = decode(
        b"FACT",
        0x0100_2605,
        vec![sub(b"DATA", &[1, 2]), sub(b"XNAM", &[0; 9])],
    );
    assert_eq!(record.unknown_subrecords().len(), 2);
}

#[test]
fn actors_have_ai_data_and_a_voice() {
    let ai = AiData {
        aggression: 2,
        confidence: 4,
        energy_level: 50,
        responsibility: 50,
        mood: 0,
        services: 0x7F,
        trained_skill: -1,
        trainer_level: 0,
        assistance: 1,
        aggro_radius_behaviour: 1,
        aggro_radius: 1000,
    };
    assert_eq!(ai.aggression_name(), Some("very aggressive"));
    let mut npc = Npc::new(0x0100_2700, "TestRaider", "Raider");
    npc.ai = Some(ai);
    npc.voice = Some(FormId(0x0100_2701));
    let Record::Npc(decoded) = round_trip(npc.clone()) else {
        panic!("not an npc")
    };
    assert_eq!(decoded.ai, Some(ai));
    let mut creature = Creature::new(
        0x0100_2702,
        "TestBeast",
        "Beast",
        "creatures\\beast\\skeleton.nif",
    );
    creature.ai = Some(ai);
    let Record::Creature(decoded) = round_trip(creature) else {
        panic!("not a creature")
    };
    assert_eq!(decoded.ai, Some(ai));
    // Every size but 20 stays unknown, for both kinds.
    for sig in [b"NPC_", b"CREA"] {
        for len in [0usize, 8, 19, 21, 24] {
            let record = decode(sig, 0x0100_2703, vec![sub(b"AIDT", &vec![1u8; len])]);
            assert_eq!(record.unknown_subrecords().len(), 1, "{sig:?} {len}");
        }
    }
    // Bytes 5 to 7 hold garbage in the files; they are not kept and the
    // rest reads the same.
    let mut raw = ai.clone_bytes_for_test();
    raw[5..8].copy_from_slice(&[0x58, 0xCD, 0x07]);
    let record = decode(b"NPC_", 0x0100_2704, vec![sub(b"AIDT", &raw)]);
    let Record::Npc(npc) = record else {
        panic!("not an npc")
    };
    assert_eq!(npc.ai, Some(ai));
}

// ----------------------------------------------------------- conditions

#[test]
fn condition_parameters_that_name_references_are_found_by_function() {
    // GetDistance (1): the first parameter.
    let c = Condition::float(1, Comparison::Less, 500.0).with_params(0x0100_2800, 0);
    assert_eq!(c.reference_params(), [Some(FormId(0x0100_2800)), None]);
    // GetIsReference (136) and GetInSameCell (32).
    for function in [136, 32, 45, 53, 515] {
        let c = Condition::float(function, Comparison::Equal, 1.0).with_params(0x0100_2801, 9);
        assert_eq!(
            c.reference_params(),
            [Some(FormId(0x0100_2801)), None],
            "{function}"
        );
    }
    // The second parameter of 230, and none for a zero value.
    let c = Condition::float(230, Comparison::Equal, 1.0).with_params(0x0100_2802, 0x0100_2803);
    assert_eq!(c.reference_params(), [None, Some(FormId(0x0100_2803))]);
    let c = Condition::float(1, Comparison::Equal, 1.0).with_params(0, 0);
    assert_eq!(c.reference_params(), [None, None]);
    // GetDead (46) and GetDisabled (35) take no reference parameter.
    for function in [46, 35, 72, 58, 79] {
        let c = Condition::float(function, Comparison::Equal, 1.0).with_params(0x0100_2804, 0);
        assert_eq!(c.reference_params(), [None, None], "{function}");
    }
}

// --------------------------------------------------------------- scripts

#[test]
fn every_embedded_script_is_listed_with_its_context() {
    let body = |source: &str| {
        let mut body = ScriptBody::from_source(source);
        body.references = vec![
            ScriptReference::Form(FormId(0x0100_2900)),
            ScriptReference::Local(2),
        ];
        body.locals = vec![LocalVariable {
            index: 1,
            flags: 0,
            name: Some("done".into()),
        }];
        body
    };
    let mut quest = Quest::new(0x0100_2901, "TestQuest", "Test");
    quest.stages.push(QuestStage {
        index: 10,
        entries: vec![
            QuestLogEntry::new("one"),
            QuestLogEntry {
                script: body("scn A"),
                ..QuestLogEntry::new("two")
            },
        ],
    });
    let record: Record = quest.into();
    let found = record.script_bodies();
    assert_eq!(found.len(), 1, "empty bodies are left out");
    assert_eq!(found[0].0, "stage 10 entry 1");
    assert_eq!(found[0].1.references.len(), 2);
    assert_eq!(found[0].1.locals[0].name.as_deref(), Some("done"));

    // A header and nothing else is blank, and left out.
    let header_only = ScriptBody {
        header: Some(ScriptHeader::default()),
        ..ScriptBody::default()
    };
    assert!(header_only.is_blank() && !header_only.is_empty());
    let blank_info: Record = Info::new(0x0100_2908, 0x0100_2903u32, Vec::new())
        .with_result_begin(header_only)
        .into();
    assert!(blank_info.script_bodies().is_empty());

    let info: Record = Info::new(0x0100_2902, 0x0100_2903u32, Vec::new())
        .with_result_begin(body("scn B"))
        .with_result_end(body("scn C"))
        .into();
    let names: Vec<String> = info.script_bodies().into_iter().map(|(c, _)| c).collect();
    assert_eq!(names, ["result begin", "result end"]);

    let mut terminal = Terminal::new(0x0100_2904, "TestTerm", "T", "H");
    terminal.items.push(TerminalMenuItem::new("a", "b"));
    terminal.items.push(TerminalMenuItem {
        script: body("scn D"),
        ..TerminalMenuItem::new("c", "d")
    });
    let terminal: Record = terminal.into();
    let names: Vec<String> = terminal
        .script_bodies()
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    assert_eq!(names, ["item 1"]);

    let package: Record = Package::new(0x0100_2905, "TestPkg", 5)
        .with_on_end(PackageEvent::new().with_script(body("scn E")))
        .into();
    let names: Vec<String> = package
        .script_bodies()
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    assert_eq!(names, ["on end"]);

    let script: Record = Script::new(0x0100_2906, "TestScript", "scn F").into();
    assert_eq!(script.script_bodies()[0].0, "script");
    let other: Record = Global::new(0x0100_2907, "G", GlobalType::Short, 0.0).into();
    assert!(other.script_bodies().is_empty());
}

// ------------------------------------------------------------- the tables

#[test]
fn the_new_records_are_in_the_logic_decoder_table() {
    for (sig, name) in [
        (sigs::GLOB, "Global"),
        (sigs::GMST, "GameSetting"),
        (sigs::MESG, "Message"),
        (sigs::FACT, "Faction"),
    ] {
        assert_eq!(decoder_for(sig).map(|d| d.type_name), Some(name));
        assert!(LOGIC_DECODERS.iter().any(|d| d.sig == sig));
        assert!(!DECODERS.iter().any(|d| d.sig == sig));
    }
}

/// Bytes of the helpers the public API does not offer, written here.
trait TestBytes {
    fn bytes_for_test(&self) -> Vec<u8>;
}

impl TestBytes for Condition {
    fn bytes_for_test(&self) -> Vec<u8> {
        // The record encoder writes conditions; borrow it through a
        // package.
        let mut package = Package::new(1u32, "X", 0);
        package.conditions.push(*self);
        let raw = package.to_raw();
        raw.subrecords
            .iter()
            .find(|s| &s.sig.0 == b"CTDA")
            .expect("CTDA")
            .data
            .clone()
    }
}

trait AiBytes {
    fn clone_bytes_for_test(&self) -> Vec<u8>;
}

impl AiBytes for AiData {
    fn clone_bytes_for_test(&self) -> Vec<u8> {
        let mut npc = Npc::new(1u32, "X", "X");
        npc.ai = Some(*self);
        npc.to_raw()
            .subrecords
            .iter()
            .find(|s| &s.sig.0 == b"AIDT")
            .expect("AIDT")
            .data
            .clone()
    }
}
