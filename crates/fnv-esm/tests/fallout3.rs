//! Fallout 3 support, on invented data: game detection from the plugin
//! header, the older subrecord sizes Fallout 3 plugins use, and the
//! terminal, note and package decoders both games share.

use fnv_esm::fixture::{FALLOUT3_PLUGIN_VERSION, PLUGIN_VERSION, PluginBuilder};
use fnv_esm::*;

fn header(version: f32) -> Tes4Header {
    let bytes = PluginBuilder::new().version(version).build();
    Plugin::parse(&bytes).expect("plugin").header().clone()
}

#[test]
fn the_game_comes_from_the_hedr_version() {
    let fo3 = header(FALLOUT3_PLUGIN_VERSION);
    let nv = header(PLUGIN_VERSION);
    assert_eq!(
        GameVariant::detect(&fo3, "Fallout3.esm", None),
        Ok(GameVariant::Fallout3)
    );
    assert_eq!(
        GameVariant::detect(&nv, "FalloutNV.esm", None),
        Ok(GameVariant::FalloutNewVegas)
    );
    // Any other name goes by the version alone: add-ons, test plugins.
    assert_eq!(
        GameVariant::detect(&fo3, "Anchorage.esm", None),
        Ok(GameVariant::Fallout3)
    );
    assert_eq!(
        GameVariant::detect(&nv, "MiniTown.esm", None),
        Ok(GameVariant::FalloutNewVegas)
    );
    // Within the tolerance.
    let near = header(0.9401);
    assert_eq!(
        GameVariant::detect(&near, "x.esm", None),
        Ok(GameVariant::Fallout3)
    );
    // Through the plugin.
    let bytes = PluginBuilder::new()
        .version(FALLOUT3_PLUGIN_VERSION)
        .build();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    assert_eq!(
        plugin.detect_game("fallout3.esm", None),
        Ok(GameVariant::Fallout3)
    );
}

#[test]
fn an_unknown_version_or_a_mismatch_is_an_error_unless_overridden() {
    let odd = header(1.0);
    let error = GameVariant::detect(&odd, "Odd.esm", None).unwrap_err();
    assert!(matches!(error, EsmError::UnknownGame { .. }), "{error}");
    assert!(error.to_string().contains("Odd.esm"));
    assert_eq!(
        GameVariant::detect(&odd, "Odd.esm", Some(GameVariant::Fallout3)),
        Ok(GameVariant::Fallout3)
    );
    let fo3 = header(FALLOUT3_PLUGIN_VERSION);
    let error = GameVariant::detect(&fo3, "FalloutNV.esm", None).unwrap_err();
    assert!(
        matches!(
            error,
            EsmError::GameMismatch {
                version_game: GameVariant::Fallout3,
                name_game: GameVariant::FalloutNewVegas,
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(
        GameVariant::detect(&fo3, "FalloutNV.esm", Some(GameVariant::FalloutNewVegas)),
        Ok(GameVariant::FalloutNewVegas)
    );
}

#[test]
fn game_names_and_ids() {
    for game in GameVariant::ALL {
        assert_eq!(game.id().parse::<GameVariant>(), Ok(game));
        assert_eq!(
            GameVariant::from_hedr_version(game.hedr_version()),
            Some(game)
        );
        assert_eq!(
            GameVariant::from_master_name(game.master_name()),
            Some(game)
        );
        let json = serde_json::to_string(&game).expect("json");
        assert_eq!(json, format!("\"{}\"", game.id()));
        assert_eq!(game.to_string(), game.id());
    }
    assert_eq!(
        GameVariant::from_master_name("Data\\FALLOUT3.ESM"),
        Some(GameVariant::Fallout3)
    );
    assert_eq!(GameVariant::from_master_name("BrokenSteel.esm"), None);
    assert!("fallout4".parse::<GameVariant>().is_err());
    assert!(is_new_vegas_only(Sig::new(b"REPU")));
    assert!(is_new_vegas_only(Sig::new(b"CHAL")));
    assert!(!is_new_vegas_only(sigs::TERM));
    assert_eq!(NEW_VEGAS_ONLY.len(), 14);
    // None of them has a typed decoder.
    assert!(NEW_VEGAS_ONLY.iter().all(|&sig| decoder_for(sig).is_none()));
}

fn decode(sig: &[u8; 4], subrecords: &[(&[u8; 4], Vec<u8>)]) -> Record {
    let mut raw = RawRecord::new(sig, 0x0100_0800u32);
    for (s, data) in subrecords {
        raw = raw.with(s, data.clone());
    }
    raw.decode()
}

fn trdt(len: usize) -> Vec<u8> {
    let mut data = vec![0xCD; len];
    data[..4].copy_from_slice(&5u32.to_le_bytes());
    data[4..8].copy_from_slice(&50i32.to_le_bytes());
    data[8..12].fill(0);
    data[12] = 1;
    if len >= 20 {
        data[16..20].copy_from_slice(&0x0100_0900u32.to_le_bytes());
    }
    if len >= 24 {
        data[20] = 1;
    }
    data
}

#[test]
fn info_responses_read_all_three_trdt_sizes() {
    let Record::Info(info) = decode(
        b"INFO",
        &[
            (b"TRDT", trdt(16)),
            (b"NAM1", b"short\0".to_vec()),
            (b"TRDT", trdt(20)),
            (b"NAM1", b"middle\0".to_vec()),
            (b"TRDT", trdt(24)),
            (b"NAM1", b"long\0".to_vec()),
        ],
    ) else {
        panic!("an INFO");
    };
    assert!(info.unknown.is_empty(), "{:?}", info.unknown);
    let data: Vec<ResponseData> = info.responses.iter().filter_map(|r| r.data).collect();
    assert_eq!(data.len(), 3);
    for d in &data {
        assert_eq!(
            (d.emotion_type, d.emotion_value, d.response_number),
            (5, 50, 1)
        );
    }
    assert_eq!(data[0].sound, FormId::NULL);
    assert_eq!(data[0].flags, 0);
    assert_eq!(data[1].sound, FormId(0x0100_0900));
    assert_eq!(data[1].flags, 0);
    assert_eq!(data[2].sound, FormId(0x0100_0900));
    assert_eq!(data[2].flags, 1);
    let texts: Vec<_> = info.responses.iter().map(|r| r.text.as_deref()).collect();
    assert_eq!(texts, [Some("short"), Some("middle"), Some("long")]);
    // Other sizes stay undecoded.
    let Record::Info(odd) = decode(b"INFO", &[(b"TRDT", vec![0; 18])]) else {
        panic!("an INFO");
    };
    assert!(odd.responses.is_empty());
    assert_eq!(odd.unknown.len(), 1);
}

#[test]
fn info_data_reads_two_three_and_four_bytes() {
    let flags = |data: &[u8]| {
        let Record::Info(info) = decode(b"INFO", &[(b"DATA", data.to_vec())]) else {
            panic!("an INFO");
        };
        assert!(info.unknown.is_empty());
        info.data.map(|d| (d.info_type, d.next_speaker, d.flags))
    };
    assert_eq!(flags(&[3, 1]), Some((3, 1, None)));
    assert_eq!(flags(&[3, 1, 0x05]), Some((3, 1, Some(0x05))));
    assert_eq!(flags(&[3, 1, 0x05, 0x01]), Some((3, 1, Some(0x0105))));
    let Record::Info(info) = decode(b"INFO", &[(b"DATA", vec![0; 5])]) else {
        panic!("an INFO");
    };
    assert_eq!(info.data, None);
    assert_eq!(info.unknown.len(), 1);
}

fn legacy_npc_data() -> Vec<u8> {
    let mut data: Vec<u8> = (10..24).collect();
    data.extend_from_slice(&75i32.to_le_bytes());
    data.extend_from_slice(&[5, 6, 4, 7, 5, 6, 3]);
    data
}

#[test]
fn the_older_25_byte_npc_data_fills_stats_and_skills() {
    let Record::Npc(npc) = decode(b"NPC_", &[(b"DATA", legacy_npc_data())]) else {
        panic!("an NPC");
    };
    assert!(npc.unknown.is_empty());
    assert_eq!(
        npc.stats,
        Some(NpcStats {
            base_health: 75,
            attributes: [5, 6, 4, 7, 5, 6, 3],
        })
    );
    let skills = npc.skills.expect("skills");
    assert_eq!(skills.values.to_vec(), (10..24).collect::<Vec<u8>>());
    assert_eq!(skills.offsets, [0; 14]);
    // Encoding writes the current forms, which decode the same.
    let raw = npc.to_raw();
    assert!(
        raw.subrecords
            .iter()
            .any(|s| &s.sig.0 == b"DNAM" && s.data.len() == 28)
    );
    assert!(
        raw.subrecords
            .iter()
            .any(|s| &s.sig.0 == b"DATA" && s.data.len() == 11)
    );
    assert_eq!(raw.decode(), Record::Npc(npc));

    // With a DNAM as well, the 25-byte DATA is kept whole.
    let Record::Npc(both) = decode(
        b"NPC_",
        &[(b"DATA", legacy_npc_data()), (b"DNAM", vec![7; 28])],
    ) else {
        panic!("an NPC");
    };
    assert_eq!(both.stats, None);
    assert_eq!(both.skills.map(|s| s.values), Some([7; 14]));
    assert_eq!(both.unknown.len(), 1);
    assert_eq!(both.unknown[0].data, legacy_npc_data());
}

fn dnam(len: usize) -> Vec<u8> {
    let mut d = vec![0u8; len];
    let put = |d: &mut Vec<u8>, at: usize, b: [u8; 4]| d[at..at + 4].copy_from_slice(&b);
    put(&mut d, 0, 5u32.to_le_bytes());
    put(&mut d, 8, 1.0f32.to_le_bytes());
    put(&mut d, 16, 0.5f32.to_le_bytes());
    put(&mut d, 20, 2.0f32.to_le_bytes());
    d[40] = 45;
    d[42] = 1;
    put(&mut d, 44, 256.0f32.to_le_bytes());
    put(&mut d, 48, 2048.0f32.to_le_bytes());
    put(&mut d, 64, 5.0f32.to_le_bytes());
    put(&mut d, 68, 24.0f32.to_le_bytes());
    put(&mut d, 92, 3.0f32.to_le_bytes());
    put(&mut d, 104, 41u32.to_le_bytes());
    d
}

#[test]
fn weapon_combat_fields_read_from_both_dnam_sizes() {
    for len in [136, 204] {
        let Record::Weapon(weapon) = decode(b"WEAP", &[(b"DNAM", dnam(len))]) else {
            panic!("a weapon");
        };
        // The block stays whole in `unknown`.
        assert_eq!(weapon.unknown.len(), 1);
        assert_eq!(weapon.unknown[0].data.len(), len);
        let combat = weapon.combat().expect("combat fields");
        assert_eq!(
            combat,
            WeaponCombat {
                animation_type: 5,
                reach: 1.0,
                min_spread: 0.5,
                spread: 2.0,
                vats_to_hit: 45,
                projectile_count: 1,
                min_range: 256.0,
                max_range: 2048.0,
                fire_rate: 5.0,
                action_points: 24.0,
                reload_time: 3.0,
                skill: 41,
            }
        );
    }
    assert_eq!(WeaponCombat::parse(&dnam(204)[..107]), None);
    assert!(WeaponCombat::parse(&dnam(204)[..108]).is_some());
}

fn terminal() -> Terminal {
    let mut hidden = TerminalMenuItem::new("Open the vault door", "Opening...");
    hidden.flags = Some(2);
    hidden.script = ScriptBody {
        references: vec![ScriptReference::Form(FormId(0x0100_0A00))],
        ..ScriptBody::from_source("ref door\r\nbegin\r\nend\r\n")
    };
    hidden.conditions = vec![Condition::float(72, Comparison::Equal, 1.0)];
    let mut note = TerminalMenuItem::new("Read the log", "");
    note.display_note = Some(FormId(0x0100_0B00));
    let mut sub = TerminalMenuItem::new("Maintenance", "");
    sub.sub_terminal = Some(FormId(0x0100_0C00));
    Terminal {
        sound: Some(FormId(0x0100_0D00)),
        password_note: Some(FormId(0x0100_0B01)),
        data: Some(TerminalData {
            difficulty: 3,
            flags: 0x02,
            server_type: 5,
        }),
        items: vec![hidden, note, sub],
        ..Terminal::new(
            0x0100_0800,
            "TestTerminal",
            "Overseer",
            "Welcome, Overseer.",
        )
    }
}

fn note(kind: u8) -> Note {
    let mut note = Note::text(0x0100_0B00, "TestNote", "A Note", "Dear diary");
    note.note_type = Some(kind);
    note.quests = vec![FormId(0x0100_0E00), FormId(0x0100_0E01)];
    if kind == Note::TYPE_VOICE {
        note.text = None;
        note.topic = Some(FormId(0x0100_0F00));
        note.sound = Some(FormId(0x0100_0F01));
    }
    if kind == Note::TYPE_IMAGE {
        note.texture = Some("notes\\test\\map.dds".to_owned());
    }
    note
}

fn package() -> Package {
    let mut package = Package::new(0x0100_1000, "TestSandbox", 12);
    package.location = Some(PackageLocation {
        location_type: PackageLocation::NEAR_REFERENCE,
        value: 0x0100_1100,
        radius: 512,
    });
    package.location2 = Some(PackageLocation {
        location_type: PackageLocation::IN_CELL,
        value: 0x0100_1101,
        radius: 0,
    });
    package.schedule = Some(PackageSchedule {
        month: -1,
        day_of_week: -1,
        date: 0,
        hour: 8,
        duration: 10,
    });
    package.target = Some(PackageTarget {
        target_type: PackageTarget::SPECIFIC_REFERENCE,
        value: 0x0100_1200,
        count: 0,
        extra: Some(0.0),
    });
    package.target2 = Some(PackageTarget {
        target_type: 2,
        value: 7,
        count: 1,
        extra: None,
    });
    package.conditions = vec![Condition::float(14, Comparison::Greater, 2.0)];
    package.dialogue = Some(PackageDialogue {
        fov: 100.0,
        topic: FormId(0x0100_1300),
        rest: vec![0; 16],
    });
    package.combat_style = Some(FormId(0x0100_1400));
    package.on_begin = Some(PackageEvent {
        idle: Some(FormId::NULL),
        script: ScriptBody {
            references: vec![ScriptReference::Form(FormId(0x0100_1201))],
            ..ScriptBody::from_source("begin\r\nend\r\n")
        },
        topic: Some(FormId::NULL),
    });
    package.on_end = Some(PackageEvent::default());
    package
}

#[test]
fn terminals_notes_and_packages_round_trip() {
    let records: Vec<Record> = vec![
        terminal().into(),
        Terminal::new(0x0100_0801, "TestEmpty", "Empty", "").into(),
        note(Note::TYPE_TEXT).into(),
        note(Note::TYPE_VOICE).into(),
        note(Note::TYPE_IMAGE).into(),
        note(Note::TYPE_SOUND).into(),
        package().into(),
        Package::new(0x0100_1001, "TestWander", 5).into(),
    ];
    for record in records {
        let raw = record.to_raw();
        let decoded = raw.decode();
        assert_eq!(decoded, record, "{} did not round-trip", record.sig());
        assert!(decoded.unknown_subrecords().is_empty());
        assert_eq!(decoded.to_raw(), raw);
    }
    for (sig, name) in [
        (sigs::TERM, "Terminal"),
        (sigs::NOTE, "Note"),
        (sigs::PACK, "Package"),
        (sigs::GLOB, "Global"),
        (sigs::GMST, "GameSetting"),
        (sigs::MESG, "Message"),
        (sigs::FACT, "Faction"),
    ] {
        assert_eq!(decoder_for(sig).map(|d| d.type_name), Some(name));
        assert!(LOGIC_DECODERS.iter().any(|d| d.sig == sig));
        assert!(!DECODERS.iter().any(|d| d.sig == sig));
    }
    assert_eq!(
        all_decoders().count(),
        DECODERS.len() + LOGIC_DECODERS.len()
    );
}

fn script_subrecords(source: &str, reference: u32) -> Vec<(&'static [u8; 4], Vec<u8>)> {
    let mut schr = vec![0u8; 20];
    schr[4..8].copy_from_slice(&1u32.to_le_bytes());
    vec![
        (b"SCHR", schr),
        (b"SCDA", vec![0x1D, 0, 0, 0]),
        (b"SCTX", source.as_bytes().to_vec()),
        (b"SCRO", reference.to_le_bytes().to_vec()),
    ]
}

fn ctda(function: u32) -> Vec<u8> {
    let mut data = vec![0u8; 28];
    data[8..12].copy_from_slice(&function.to_le_bytes());
    data
}

#[test]
fn a_terminal_reads_in_the_measured_order() {
    // EDID OBND FULL MODL DESC SNAM PNAM DNAM, then per item ITXT RNAM ANAM
    // (INAM | TNAM) SCHR ... CTDA, with the older 3-byte DNAM.
    let mut subs: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"EDID", b"TestTerm\0".to_vec()),
        (b"OBND", vec![0; 12]),
        (b"FULL", b"Terminal\0".to_vec()),
        (b"MODL", b"terminals\\t.nif\0".to_vec()),
        (b"MODT", vec![1; 24]),
        (b"DESC", b"HEADER\0".to_vec()),
        (b"SNAM", 0x0100_0D00u32.to_le_bytes().to_vec()),
        (b"PNAM", 0x0100_0B01u32.to_le_bytes().to_vec()),
        (b"DNAM", vec![1, 2, 9]),
        (b"ITXT", b"First\0".to_vec()),
        (b"RNAM", vec![0; 4]),
        (b"ANAM", vec![2]),
        (b"INAM", 0x0100_0B00u32.to_le_bytes().to_vec()),
    ];
    subs.extend(script_subrecords("", 0));
    subs.truncate(subs.len() - 3); // a bare SCHR, as most items have
    subs.extend([
        (b"ITXT", b"Second\0".to_vec()),
        (b"RNAM", b"Done.\0".to_vec()),
        (b"ANAM", vec![0]),
        (b"TNAM", 0x0100_0C00u32.to_le_bytes().to_vec()),
    ]);
    subs.extend(script_subrecords("set x to 1", 0x0100_0A00));
    subs.extend([(b"CTDA", ctda(72)), (b"CTDA", ctda(14))]);
    let refs: Vec<(&[u8; 4], Vec<u8>)> = subs.iter().map(|(s, d)| (*s, d.clone())).collect();
    let Record::Terminal(term) = decode(b"TERM", &refs) else {
        panic!("a terminal");
    };
    assert_eq!(term.common.editor_id.as_deref(), Some("TestTerm"));
    assert_eq!(term.header_text.as_deref(), Some("HEADER"));
    assert_eq!(
        term.data,
        Some(TerminalData {
            difficulty: 1,
            flags: 2,
            server_type: 9
        })
    );
    assert_eq!(term.items.len(), 2);
    let first = &term.items[0];
    assert_eq!(first.text.as_deref(), Some("First"));
    assert_eq!(first.result_text.as_deref(), Some(""));
    assert_eq!(first.display_note, Some(FormId(0x0100_0B00)));
    assert!(first.script.header.is_some() && first.script.source.is_none());
    let second = &term.items[1];
    assert_eq!(second.sub_terminal, Some(FormId(0x0100_0C00)));
    assert_eq!(second.script.source.as_deref(), Some("set x to 1"));
    assert_eq!(
        second.script.references,
        [ScriptReference::Form(FormId(0x0100_0A00))]
    );
    assert_eq!(second.conditions.len(), 2);
    // Only the model data is left.
    let left: Vec<String> = term.unknown.iter().map(|s| s.sig.to_string()).collect();
    assert_eq!(left, ["MODT"]);
}

#[test]
fn a_package_reads_its_event_blocks_and_short_forms() {
    let mut subs: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"EDID", b"TestPack\0".to_vec()),
        (b"PKDT", vec![0, 0, 0, 0, 6, 0xCD, 0xCD, 0xCD]),
        (
            b"PLDT",
            [3i32, 0, 1200]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        ),
        (b"PSDT", vec![0xFF, 0xFF, 0, 0xFF, 0, 0, 0, 0]),
        (
            b"PTDT",
            [0u32, 0x0100_1200, 256]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        ),
        (b"CTDA", ctda(72)),
        (b"PKDD", vec![0; 16]),
        (b"IDLF", vec![1]),
        (b"POBA", vec![]),
        (b"INAM", vec![0; 4]),
    ];
    subs.extend(script_subrecords("", 0).into_iter().take(1));
    subs.extend([
        (b"TNAM", vec![0; 4]),
        (b"POEA", vec![]),
        (b"INAM", vec![0; 4]),
    ]);
    subs.extend(script_subrecords("say hi", 0x0100_1201));
    subs.extend([
        (b"TNAM", 0x0100_1300u32.to_le_bytes().to_vec()),
        (b"POCA", vec![]),
        (b"INAM", vec![0; 4]),
    ]);
    subs.extend(script_subrecords("", 0).into_iter().take(1));
    subs.push((b"TNAM", vec![0; 4]));
    let Record::Package(pack) = decode(b"PACK", &subs) else {
        panic!("a package");
    };
    let data = pack.data.clone().expect("PKDT");
    assert_eq!((data.flags, data.package_type), (0, 6));
    assert_eq!(data.type_name(), Some("travel"));
    assert_eq!(data.rest, [0xCD; 3]);
    let location = pack.location.expect("PLDT");
    assert_eq!((location.location_type, location.radius), (3, 1200));
    assert_eq!(location.form(), None);
    let schedule = pack.schedule.expect("PSDT");
    assert_eq!(
        (schedule.month, schedule.day_of_week, schedule.hour),
        (-1, -1, -1)
    );
    let target = pack.target.expect("PTDT");
    assert_eq!(target.form(), Some(FormId(0x0100_1200)));
    assert_eq!((target.count, target.extra), (256, None));
    assert_eq!(pack.conditions.len(), 1);
    assert_eq!(pack.dialogue.as_ref().map(|d| d.rest.len()), Some(8));
    let events: Vec<&str> = pack.events().map(|(name, _)| name).collect();
    assert_eq!(events, ["on begin", "on end", "on change"]);
    let end = pack.on_end.as_ref().expect("POEA");
    assert_eq!(end.script.source.as_deref(), Some("say hi"));
    assert_eq!(end.topic, Some(FormId(0x0100_1300)));
    assert_eq!(
        end.script.references,
        [ScriptReference::Form(FormId(0x0100_1201))]
    );
    let left: Vec<String> = pack.unknown.iter().map(|s| s.sig.to_string()).collect();
    assert_eq!(left, ["IDLF"]);
    // INAM and TNAM outside an event block are not the package's.
    let Record::Package(stray) = decode(b"PACK", &[(b"INAM", vec![0; 4])]) else {
        panic!("a package");
    };
    assert_eq!(stray.unknown.len(), 1);
}

#[test]
fn a_voice_note_names_its_topic() {
    let Record::Note(voice) = decode(
        b"NOTE",
        &[
            (b"DATA", vec![Note::TYPE_VOICE]),
            (b"ONAM", 0x0100_0E00u32.to_le_bytes().to_vec()),
            (b"TNAM", 0x0100_0F00u32.to_le_bytes().to_vec()),
            (b"SNAM", 0x0100_0F01u32.to_le_bytes().to_vec()),
        ],
    ) else {
        panic!("a note");
    };
    assert!(voice.is_voice());
    assert_eq!(voice.topic, Some(FormId(0x0100_0F00)));
    assert_eq!(voice.text, None);
    let Record::Note(text) = decode(
        b"NOTE",
        &[
            (b"DATA", vec![Note::TYPE_TEXT]),
            (b"TNAM", b"abc\0".to_vec()),
        ],
    ) else {
        panic!("a note");
    };
    assert_eq!(text.text.as_deref(), Some("abc"));
    assert_eq!(text.topic, None);
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }
}

#[test]
fn the_logic_decoders_take_any_bytes() {
    const SUBRECORDS: &[&[u8; 4]] = &[
        b"EDID", b"FULL", b"DESC", b"SNAM", b"PNAM", b"DNAM", b"ITXT", b"RNAM", b"ANAM", b"INAM",
        b"TNAM", b"SCHR", b"SCDA", b"SCTX", b"SLSD", b"SCVR", b"SCRO", b"SCRV", b"CTDA", b"DATA",
        b"ONAM", b"XNAM", b"ICON", b"PKDT", b"PLDT", b"PLD2", b"PSDT", b"PTDT", b"PTD2", b"PKDD",
        b"CNAM", b"POBA", b"POEA", b"POCA", b"FLTV", b"FNAM", b"KNAM", b"AIDT", b"VTCK",
    ];
    let mut rng = Lcg(0xF03);
    for entry in LOGIC_DECODERS {
        for sig in SUBRECORDS {
            for len in 0..=32usize {
                let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                let raw = RawRecord {
                    sig: entry.sig,
                    meta: RecordMeta::new(1u32),
                    subrecords: vec![RawSubrecord::new(sig, data)],
                };
                let record = raw.decode();
                assert_eq!(record.sig(), entry.sig);
                let unknown = record.unknown_subrecords();
                assert!(unknown.is_empty() || unknown == raw.subrecords.as_slice());
                let _ = record.to_raw();
            }
        }
        for _ in 0..500 {
            let subrecords: Vec<RawSubrecord> = (0..rng.below(16))
                .map(|_| {
                    let sig = SUBRECORDS[rng.below(SUBRECORDS.len())];
                    let len = [0, 1, 3, 4, 8, 12, 16, 20, 24, 28][rng.below(10)];
                    RawSubrecord::new(sig, (0..len).map(|_| rng.next() as u8).collect::<Vec<u8>>())
                })
                .collect();
            let raw = RawRecord {
                sig: entry.sig,
                meta: RecordMeta::new(2u32),
                subrecords,
            };
            let _ = raw.decode().to_raw().decode();
        }
    }
}

#[test]
fn a_fallout3_plugin_walks_and_decodes() {
    let mut builder = PluginBuilder::new();
    builder
        .version(FALLOUT3_PLUGIN_VERSION)
        .master_file(true)
        .add(terminal())
        .add(note(Note::TYPE_TEXT))
        .add(package());
    let bytes = builder.build();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    assert_eq!(plugin.header().version, FALLOUT3_PLUGIN_VERSION);
    assert_eq!(
        plugin.detect_game("Fallout3.esm", None),
        Ok(GameVariant::Fallout3)
    );
    let mut sigs_seen = Vec::new();
    let mut walker = plugin.walk();
    while let Some(record) = walker.next_record().expect("walk") {
        let decoded = record.decode().expect("decode");
        assert!(!matches!(decoded, Record::Unknown(_)));
        sigs_seen.push(decoded.sig().to_string());
    }
    sigs_seen.sort();
    assert_eq!(sigs_seen, ["NOTE", "PACK", "TERM"]);
}
