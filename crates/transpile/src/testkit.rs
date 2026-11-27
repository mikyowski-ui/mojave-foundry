//! A small synthetic town for tests (spec 12.1 "Front end"), compiled
//! always so that every test of the crate (and the lowering's VM tests)
//! can build exactly the same IR.
//!
//! [`town`] writes the plugin with the `fnv_esm` builders, extracts it
//! with `mir` into a fresh directory under the system's temporary
//! directory, builds it and removes the directory. Everything in it is
//! invented. It holds: two townsfolk and a raider; a greeting with a
//! say-once INFO and a random group; a topic tree (a top-level topic, a
//! choice, a topic given by `AddTopic`); a Fallout 3 challenge with the
//! shared failure topic; a New Vegas threshold pair; a quest with three
//! stages and two objectives; an enable group with an opposite child and
//! a group of one; a locked door with a key; a locked terminal with a
//! sub-menu and a password note; a message box; a sandbox and a travel
//! package; three factions, one an enemy of the player's; and the game
//! settings the checks read.
//!
//! The form ids are in [`forms`]; [`key`] makes their form keys.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use fnv_esm::fixture::{CellSpec, FALLOUT3_PLUGIN_VERSION, PluginBuilder, WorldSpec};
use fnv_esm::{
    ActorBase, AiData, Cell, CombatReaction, Comparison, Condition, Door, Faction, FactionRank,
    FormId, GameSetting, GameSettingValue, Info, LocalVariable, Lock, Message, Note, Npc, Package,
    PackageData, PackageEvent, PackageLocation, PackageSchedule, PlacedKind, PlacedRef, Quest,
    QuestLogEntry, QuestObjective, QuestStage, QuestTarget, RawRecord, Response, Script,
    ScriptBody, ScriptReference, Static, Terminal, TerminalData, TerminalMenuItem, Topic,
    TopicData, World,
};

use crate::ir::{
    FormKey, Module, PlacedGroup, PlacedLock, PlacedObject, Placement, SimConfig, SimSlice,
    WalkMask,
};

/// The plugin's file name.
pub const PLUGIN: &str = "TestTown.esm";

/// The map the town is placed on.
pub const MAP: &str = "tstown";

/// The town's form ids (local ids of [`PLUGIN`]).
pub mod forms {
    #![allow(missing_docs)]
    pub const MARKER: u32 = 0x800;
    pub const DOOR: u32 = 0x801;
    pub const NOTE: u32 = 0x802;
    pub const TERMINAL: u32 = 0x803;
    pub const SUB_TERMINAL: u32 = 0x804;
    pub const KEY: u32 = 0x805;
    pub const MAYOR: u32 = 0x810;
    pub const GUARD: u32 = 0x811;
    pub const RAIDER: u32 = 0x812;
    pub const TOWN_FACTION: u32 = 0x820;
    pub const PLAYER_FACTION: u32 = 0x821;
    pub const RAIDER_FACTION: u32 = 0x822;
    pub const VOICE: u32 = 0x830;
    pub const AV_SPEECH: u32 = 0x840;
    pub const AV_REPAIR: u32 = 0x841;
    pub const SANDBOX: u32 = 0x850;
    pub const TRAVEL: u32 = 0x851;
    pub const DOOR_SCRIPT: u32 = 0x860;
    pub const MAYOR_SCRIPT: u32 = 0x861;
    pub const QUEST_SCRIPT: u32 = 0x862;
    pub const QUEST: u32 = 0x870;
    pub const OTHER_QUEST: u32 = 0x871;
    pub const COUNT: u32 = 0x880;
    pub const RATE: u32 = 0x881;
    pub const GREETING: u32 = 0x890;
    pub const HELLO_INFO: u32 = 0x891;
    pub const JOB: u32 = 0x892;
    pub const EITHER_INFO: u32 = 0x893;
    pub const GUARD_INFO: u32 = 0x894;
    pub const JOB_INFO: u32 = 0x895;
    pub const BRIBE: u32 = 0x896;
    pub const BRIBE_INFO: u32 = 0x897;
    pub const FAILURE: u32 = 0x898;
    pub const FAILURE_INFO: u32 = 0x899;
    pub const DETAILS: u32 = 0x89B;
    pub const DETAILS_INFO: u32 = 0x89C;
    pub const RUMOR: u32 = 0x89D;
    pub const RUMOR_INFO: u32 = 0x89E;
    pub const FIX: u32 = 0x8C0;
    pub const FIX_PASS: u32 = 0x8C1;
    pub const FIX_FAIL: u32 = 0x8C2;
    pub const MESSAGE: u32 = 0x8B0;
    pub const SETTINGS: u32 = 0x8D0;
    pub const WORLD: u32 = 0x900;
    pub const CELL: u32 = 0x901;
    pub const R_MAYOR: u32 = 0xC01;
    pub const R_GUARD: u32 = 0xC02;
    pub const R_MARKER: u32 = 0xC03;
    pub const R_DOOR: u32 = 0xC04;
    pub const R_PARENT: u32 = 0xC05;
    pub const R_CHILD: u32 = 0xC06;
    pub const R_OPPOSITE: u32 = 0xC07;
    pub const R_TERMINAL: u32 = 0xC08;
    pub const R_NOTE: u32 = 0xC09;
    pub const R_LONELY: u32 = 0xC0A;
    pub const R_FAR: u32 = 0xC0B;
    pub const R_RAIDER: u32 = 0xC0C;
}

use forms::*;

/// The form key of a local id of the town plugin.
pub fn key(id: u32) -> FormKey {
    FormKey::new(PLUGIN, id)
}

fn body(source: &str, references: &[u32]) -> ScriptBody {
    ScriptBody {
        references: references
            .iter()
            .map(|&r| ScriptReference::Form(FormId(r)))
            .collect(),
        ..ScriptBody::from_source(source)
    }
}

fn is_id(base: u32) -> Condition {
    Condition::float(72, Comparison::Equal, 1.0).with_params(base, 0)
}

fn info(form: u32, flags: u16, lines: &[&str]) -> Info {
    let responses = lines
        .iter()
        .enumerate()
        .map(|(i, text)| Response::new(i as u8 + 1, text))
        .collect();
    Info::new(form, QUEST, responses).with_flags(flags)
}

fn zstr(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

/// Adds one or more records to a plugin.
type Adder = Box<dyn Fn(&mut PluginBuilder)>;

fn small(p: &mut PluginBuilder) {
    p.add(Static::new(MARKER, "TSMarker", ""));
    let door = Door::new(DOOR, "TSDoor", "Door", "");
    p.add(Door {
        common: door.common.clone().with_script(DOOR_SCRIPT),
        ..door
    });
    p.add(Note::text(
        NOTE,
        "TSNote",
        "Mayor's Log",
        "Day one. The door sticks.\r\nThe password is OWL.",
    ));
    p.add_raw(RawRecord::new(b"KEYM", KEY).with(b"EDID", zstr("TSDoorKey")));
    p.add_raw(RawRecord::new(b"VTYP", VOICE).with(b"EDID", zstr("TSVoice")));
    p.add_raw(RawRecord::new(b"AVIF", AV_SPEECH).with(b"EDID", zstr("AVSpeech")));
    p.add_raw(RawRecord::new(b"AVIF", AV_REPAIR).with(b"EDID", zstr("AVRepair")));
    p.add(Faction::new(TOWN_FACTION, "TSTownFaction"));
    p.add(Faction::new(PLAYER_FACTION, "PlayerFaction"));
    p.add(
        Faction::new(RAIDER_FACTION, "TSRaiderFaction").with_relation(
            PLAYER_FACTION,
            -100,
            CombatReaction::Enemy,
        ),
    );
    p.add(
        Message::new(MESSAGE, "TSWelcomeMsg", "Welcome", "Welcome to %s.")
            .with_flags(Message::MESSAGE_BOX)
            .with_button("Enter")
            .with_conditional_button(
                "Knock",
                vec![Condition::float(58, Comparison::Less, 10.0).with_params(QUEST, 0)],
            ),
    );
    let settings: [(&str, GameSettingValue); 12] = [
        ("fSpeechChallengeMultiplier", GameSettingValue::Float(2.0)),
        ("fSpeechAutoSuccessThreshold", GameSettingValue::Float(0.9)),
        (
            "iSpeechChallengeDifficultyVeryEasy",
            GameSettingValue::Int(10),
        ),
        ("iSpeechChallengeDifficultyEasy", GameSettingValue::Int(40)),
        (
            "iSpeechChallengeDifficultyAverage",
            GameSettingValue::Int(55),
        ),
        ("iSpeechChallengeDifficultyHard", GameSettingValue::Int(70)),
        (
            "iSpeechChallengeDifficultyVeryHard",
            GameSettingValue::Int(80),
        ),
        ("iXPRewardSpeechChallengeVeryEasy", GameSettingValue::Int(5)),
        ("iXPRewardHackComputerVeryEasy", GameSettingValue::Int(20)),
        ("iXPRewardHackComputerEasy", GameSettingValue::Int(30)),
        ("iXPRewardPickLockEasy", GameSettingValue::Int(30)),
        ("iXPRewardPickLockAverage", GameSettingValue::Int(40)),
    ];
    for (i, (name, value)) in settings.into_iter().enumerate() {
        p.add(GameSetting::new(SETTINGS + i as u32, name, value));
    }
}

fn terminals(p: &mut PluginBuilder) {
    let mut read = TerminalMenuItem::new("Read the log", "");
    read.display_note = Some(FormId(NOTE));
    read.flags = Some(1);
    let mut unlock = TerminalMenuItem::new("Unlock the door", "Unlocked.");
    unlock.script = body("TSDoorRef.Unlock", &[R_DOOR]);
    unlock.conditions =
        vec![Condition::float(58, Comparison::GreaterOrEqual, 10.0).with_params(QUEST, 0)];
    let mut open = TerminalMenuItem::new("Maintenance", "");
    open.sub_terminal = Some(FormId(SUB_TERMINAL));
    let mut terminal = Terminal::new(TERMINAL, "TSTerminal", "Town Terminal", "WELCOME, CITIZEN");
    terminal.password_note = Some(FormId(NOTE));
    terminal.data = Some(TerminalData {
        difficulty: 1,
        flags: 0,
        server_type: 0,
    });
    terminal.items = vec![read, unlock, open];
    p.add(terminal);
    let mut back = Terminal::new(SUB_TERMINAL, "TSSubTerminal", "Maintenance", "MAINTENANCE");
    back.data = Some(TerminalData {
        difficulty: 0,
        flags: crate::terminals::UNLOCKED,
        server_type: 0,
    });
    back.items = vec![TerminalMenuItem::new(
        "Run diagnostics",
        "All systems nominal.",
    )];
    p.add(back);
}

fn actors(p: &mut PluginBuilder) {
    let mut mayor = Npc::new(MAYOR, "TSMayor", "Mayor");
    mayor.common.script = Some(FormId(MAYOR_SCRIPT));
    mayor.packages = vec![FormId(SANDBOX)];
    mayor.factions = vec![FactionRank {
        faction: FormId(TOWN_FACTION),
        rank: 1,
    }];
    mayor.voice = Some(FormId(VOICE));
    // Female and essential.
    mayor.base = Some(ActorBase {
        flags: 0x03,
        ..ActorBase::default()
    });
    mayor.ai = Some(AiData::default());
    p.add(mayor);
    let mut guard = Npc::new(GUARD, "TSGuard", "Guard");
    guard.packages = vec![FormId(TRAVEL)];
    guard.voice = Some(FormId(VOICE));
    guard.factions = vec![FactionRank {
        faction: FormId(TOWN_FACTION),
        rank: 0,
    }];
    guard.ai = Some(AiData {
        aggression: 1,
        confidence: 3,
        assistance: 2,
        ..AiData::default()
    });
    p.add(guard);
    let mut raider = Npc::new(RAIDER, "TSRaider", "Raider");
    raider.factions = vec![FactionRank {
        faction: FormId(RAIDER_FACTION),
        rank: 0,
    }];
    raider.ai = Some(AiData {
        aggression: 1,
        confidence: 2,
        ..AiData::default()
    });
    p.add(raider);
}

fn packages(p: &mut PluginBuilder) {
    let mut sandbox = Package::new(SANDBOX, "TSMayorSandbox", 12)
        .with_data(PackageData::new(12, 0, 0, 0x0020))
        .with_on_begin(PackageEvent::new().with_script(body("SetStage TSQuest 20", &[QUEST])));
    sandbox.location = Some(PackageLocation {
        location_type: 0,
        value: R_MARKER,
        radius: 512,
    });
    sandbox.schedule = Some(PackageSchedule {
        month: -1,
        day_of_week: -1,
        date: 0,
        hour: 8,
        duration: 12,
    });
    p.add(sandbox);
    let mut travel = Package::new(TRAVEL, "TSGuardTravel", 6).with_data(PackageData::new(
        6,
        PackageData::ALWAYS_RUN,
        0,
        0,
    ));
    travel.location = Some(PackageLocation {
        location_type: 0,
        value: R_DOOR,
        radius: 0,
    });
    travel.conditions = vec![Condition::float(77, Comparison::Less, 50.0)];
    p.add(travel);
}

fn scripts(p: &mut PluginBuilder) {
    let source = "scn TSDoorScript\r\nshort opened\r\n\r\nbegin OnActivate\r\n\tif IsActionRef player == 1\r\n\t\tset opened to 1\r\n\t\tShowMessage TSWelcomeMsg\r\n\t\tTSParentRef.Enable\r\n\t\tTSLonelyRef.Enable\r\n\t\tTSFarRef.Disable\r\n\t\tActivate\r\n\tendif\r\nend\r\n";
    let mut script = Script::new(DOOR_SCRIPT, "TSDoorScript", source);
    script.body.references = vec![
        ScriptReference::Form(FormId(MESSAGE)),
        ScriptReference::Form(FormId(R_PARENT)),
        ScriptReference::Form(FormId(R_LONELY)),
        ScriptReference::Form(FormId(R_FAR)),
    ];
    script.body.locals = vec![LocalVariable {
        index: 1,
        flags: 1,
        name: Some("opened".into()),
    }];
    p.add(script);
    let source = "scn TSMayorScript\nshort mood\n\nbegin GameMode\n\tif GetStage TSQuest >= 10 && TSGlobalCount > 2\n\t\tset mood to 1\n\tendif\nend\n";
    let mut script = Script::new(MAYOR_SCRIPT, "TSMayorScript", source);
    script.body.locals = vec![LocalVariable {
        index: 1,
        flags: 1,
        name: Some("mood".into()),
    }];
    p.add(script);
    let source = "scn TSQuestScript\nshort qCount\nfloat qRate\n\nbegin GameMode\n\tif qCount > 3\n\t\tSetObjectiveCompleted TSQuest 10 1\n\t\tset qRate to TSGlobalRate * 2\n\tendif\nend\n";
    let mut script = Script::new(QUEST_SCRIPT, "TSQuestScript", source);
    if let Some(header) = &mut script.body.header {
        header.script_type = 1;
    }
    script.body.locals = vec![
        LocalVariable {
            index: 1,
            flags: 1,
            name: Some("qCount".into()),
        },
        LocalVariable {
            index: 2,
            flags: 0,
            name: Some("qRate".into()),
        },
    ];
    p.add(script);
}

fn quests(p: &mut PluginBuilder) {
    let mut quest = Quest::new(QUEST, "TSQuest", "Town Trouble");
    quest.common.script = Some(FormId(QUEST_SCRIPT));
    if let Some(data) = &mut quest.data {
        data.flags = 1;
    }
    let met = QuestLogEntry {
        script: body(
            "SetObjectiveDisplayed TSQuest 10 1\nset qCount to qCount + 1",
            &[QUEST],
        ),
        ..QuestLogEntry::new("We met the mayor.")
    };
    let mut gated = QuestLogEntry::new("The guard is on duty.");
    gated.conditions = vec![
        is_id(GUARD).or(),
        Condition::float(46, Comparison::Equal, 0.0),
    ];
    let done = QuestLogEntry {
        flags: Some(1),
        script: body(
            "SetObjectiveDisplayed TSQuest 20 1\nStartQuest TSOtherQuest",
            &[QUEST, OTHER_QUEST],
        ),
        ..QuestLogEntry::new("The door is open.")
    };
    quest.stages = vec![
        QuestStage {
            index: 30,
            entries: vec![done],
        },
        QuestStage {
            index: 10,
            entries: vec![met],
        },
        QuestStage {
            index: 20,
            entries: vec![gated],
        },
    ];
    quest.objectives = vec![
        QuestObjective {
            index: 10,
            description: Some("Talk to the mayor.".into()),
            targets: vec![QuestTarget {
                target: FormId(R_MAYOR),
                flags: 0,
                conditions: Vec::new(),
            }],
        },
        QuestObjective {
            index: 20,
            description: Some("Open the door.".into()),
            targets: Vec::new(),
        },
    ];
    p.add(quest);
    p.add(Quest::new(OTHER_QUEST, "TSOtherQuest", "After the Door"));
    p.add_raw(
        RawRecord::new(b"GLOB", COUNT)
            .with(b"EDID", zstr("TSGlobalCount"))
            .with(b"FNAM", vec![b's'])
            .with(b"FLTV", 3.0f32.to_le_bytes().to_vec()),
    );
    p.add_raw(
        RawRecord::new(b"GLOB", RATE)
            .with(b"EDID", zstr("TSGlobalRate"))
            .with(b"FNAM", vec![b'f'])
            .with(b"FLTV", 0.5f32.to_le_bytes().to_vec()),
    );
}

fn top_level(form: u32, editor_id: &str, name: &str) -> Topic {
    let mut topic = Topic::new(form, editor_id, name, QUEST);
    topic.data = Some(TopicData {
        topic_type: 0,
        flags: Some(0x02),
    });
    topic
}

fn dialogue(p: &mut PluginBuilder) {
    // GREETING: a say-once INFO for the mayor, a random pair for both
    // speakers, one for the guard with a fallback term.
    let mut hello = info(HELLO_INFO, 0x04, &["Welcome to town.", "Mind the door."]);
    hello.conditions = vec![is_id(MAYOR)];
    hello.choices = vec![FormId(JOB)];
    hello.result_end = body(
        "SetStage TSQuest 10\nset TSQuest.qCount to TSQuest.qCount + 1\nset mood to 2",
        &[QUEST],
    );
    let mut either = info(EITHER_INFO, 0x02, &["Nice day."]);
    either.conditions = vec![
        is_id(MAYOR).or(),
        is_id(GUARD),
        Condition::global(58, Comparison::GreaterOrEqual, COUNT).with_params(QUEST, 0),
    ];
    let mut guard = info(GUARD_INFO, 0x20, &["Move along."]);
    guard.conditions = vec![
        is_id(GUARD),
        Condition::float(255, Comparison::Equal, 1.0),
        Condition::float(79, Comparison::GreaterOrEqual, 2.0).with_params(QUEST, 1),
    ];
    p.topic(
        Topic::new(GREETING, "GREETING", "", QUEST),
        vec![hello, either, guard],
    );
    // A tree: Work (top level) leads to Details, which gives Rumors.
    let mut job = info(JOB_INFO, 0, &["Find my key."]);
    job.conditions = vec![is_id(MAYOR)];
    job.prompt = Some("Got any work?".into());
    job.choices = vec![FormId(DETAILS)];
    p.topic(top_level(JOB, "TSJobTopic", "Work"), vec![job]);
    let mut details = info(DETAILS_INFO, 0x01, &["The guard has a spare."]);
    details.conditions = vec![is_id(MAYOR)];
    details.result_end = body("AddTopic TSRumorTopic", &[RUMOR]);
    p.topic(
        Topic::new(DETAILS, "TSDetailsTopic", "Where is it?", QUEST),
        vec![details],
    );
    let mut rumor = info(RUMOR_INFO, 0, &["They say the raiders came back."]);
    rumor.conditions = vec![is_id(GUARD).or(), is_id(MAYOR)];
    p.topic(
        Topic::new(RUMOR, "TSRumorTopic", "Heard anything?", QUEST),
        vec![rumor],
    );
    // A Fallout 3 challenge (Easy) and its shared failure topic.
    let mut bribe = info(BRIBE_INFO, 0x0080, &["Fine, take the key."])
        .with_speech_challenge(2)
        .with_knam(AV_SPEECH);
    bribe.conditions = vec![is_id(MAYOR)];
    bribe.prompt = Some("Give me the key.".into());
    bribe.result_end = body("SetStage TSQuest 30", &[QUEST]);
    p.topic(top_level(BRIBE, "TSBribeTopic", "The key"), vec![bribe]);
    let mut failed = info(FAILURE_INFO, 0, &["Nice try."]);
    failed.conditions = vec![is_id(MAYOR)];
    // As in the source games: the failure INFO links from the challenge's
    // topic (TCLF), which is its route, not a choice.
    failed.link_from = vec![FormId(BRIBE)];
    p.topic(
        Topic::new(FAILURE, "SpeechChallengeFailure", "", QUEST),
        vec![failed],
    );
    // A New Vegas threshold pair on Repair 50 for the guard.
    let repair = |op| {
        let mut c = Condition::float(14, op, 50.0).with_params(39, 0);
        c.run_on = 1;
        c
    };
    let mut pass = info(FIX_PASS, 0, &["You fixed it."]).with_knam(AV_REPAIR);
    pass.conditions = vec![is_id(GUARD), repair(Comparison::GreaterOrEqual)];
    pass.prompt = Some("Let me fix the gate.".into());
    let mut fail = info(FIX_FAIL, 0, &["You made it worse."]).with_knam(AV_REPAIR);
    fail.conditions = vec![is_id(GUARD), repair(Comparison::Less)];
    fail.prompt = Some("Let me fix the gate.".into());
    p.topic(top_level(FIX, "TSFixTopic", "The gate"), vec![pass, fail]);
}

fn records() -> Vec<Adder> {
    vec![
        Box::new(small),
        Box::new(terminals),
        Box::new(actors),
        Box::new(packages),
        Box::new(scripts),
        Box::new(quests),
        Box::new(dialogue),
    ]
}

fn references() -> Vec<PlacedRef> {
    let at = |i: u32| [100.0 + 150.0 * i as f32, 300.0, 0.0];
    let named = |mut r: PlacedRef, edid: &str| {
        r.common.editor_id = Some(edid.to_owned());
        r
    };
    let object = |form: u32, base: u32, i: u32| {
        PlacedRef::new(PlacedKind::Object, form, base, at(i), [0.0; 3])
    };
    let mut door = named(object(R_DOOR, DOOR, 4), "TSDoorRef");
    door.lock = Some(Lock {
        level: 50,
        key: FormId(KEY),
        flags: 0,
    });
    vec![
        named(
            PlacedRef::new(PlacedKind::Npc, R_MAYOR, MAYOR, at(1), [0.0; 3]),
            "TSMayorRef",
        ),
        PlacedRef::new(PlacedKind::Npc, R_GUARD, GUARD, at(2), [0.0; 3]),
        named(object(R_MARKER, MARKER, 3), "TSMarkerRef"),
        door,
        named(
            object(R_PARENT, MARKER, 5).initially_disabled(),
            "TSParentRef",
        ),
        object(R_CHILD, MARKER, 6).with_enable_parent(R_PARENT, false),
        object(R_OPPOSITE, MARKER, 7).with_enable_parent(R_PARENT, true),
        named(object(R_TERMINAL, TERMINAL, 8), "TSTerminalRef"),
        object(R_NOTE, NOTE, 9),
        named(
            object(R_LONELY, MARKER, 10).initially_disabled(),
            "TSLonelyRef",
        ),
        named(object(R_FAR, MARKER, 11), "TSFarRef"),
        PlacedRef::new(PlacedKind::Npc, R_RAIDER, RAIDER, at(12), [0.0; 3]),
    ]
}

/// The town plugin's bytes; `reversed` adds every record group and
/// reference in the opposite order.
pub fn plugin(reversed: bool) -> Vec<u8> {
    let mut plugin = PluginBuilder::new();
    plugin
        .version(FALLOUT3_PLUGIN_VERSION)
        .master_file(true)
        .author("transpile testkit");
    let mut adders = records();
    let mut refs = references();
    if reversed {
        adders.reverse();
        refs.reverse();
    }
    for add in &adders {
        add(&mut plugin);
    }
    let mut cell = CellSpec::new(Cell::exterior(CELL, 0, 0));
    cell.temporary = refs;
    plugin.world(WorldSpec::new(World::new(WORLD, "TSWorld")).with_cell(cell));
    plugin.build()
}

/// What the map stage placed: every reference but `TSFarRef`, on one map,
/// with the schema 2 fields (groups, the door's lock, the raider's
/// hostile flag); the parent, its child and the lonely reference start
/// hidden; the marker is a position with no object. The
/// walkable mask is a block of hexes around the marker. `reversed` lists
/// the objects in the opposite order.
pub fn slice(reversed: bool) -> SimSlice {
    use crate::ir::PlacedKind::*;
    let placed = [
        (R_MAYOR, MAYOR, Critter),
        (R_GUARD, GUARD, Critter),
        (R_MARKER, MARKER, Scenery),
        (R_DOOR, DOOR, Door),
        (R_PARENT, MARKER, Scenery),
        (R_CHILD, MARKER, Scenery),
        (R_OPPOSITE, MARKER, Scenery),
        (R_TERMINAL, TERMINAL, Terminal),
        (R_NOTE, NOTE, Item),
        (R_LONELY, MARKER, Scenery),
        (R_RAIDER, RAIDER, Critter),
    ];
    let group = |root: u32, opposite: bool, enabled: bool| {
        Some(PlacedGroup {
            root: key(root),
            opposite,
            initially_enabled: enabled,
        })
    };
    let mut objects: Vec<PlacedObject> = placed
        .iter()
        .enumerate()
        .map(|(i, &(reference, base, kind))| PlacedObject {
            map: MAP.into(),
            map_number: 1,
            elevation: 0,
            hex: 10_000 + 2 * i as u32,
            kind,
            // The marker is no object: the map stage skips markers and the
            // `sim` stage gives their projected hex.
            object_id: (reference != R_MARKER).then_some(i as u32 + 1),
            pid: None,
            marker: reference == R_MARKER,
            reference: Some(key(reference)),
            base: Some(key(base)),
            hidden: matches!(reference, R_PARENT | R_CHILD | R_LONELY),
            group: match reference {
                R_PARENT => group(R_PARENT, false, false),
                R_CHILD => group(R_PARENT, false, false),
                R_OPPOSITE => group(R_PARENT, true, true),
                _ => None,
            },
            lock: (reference == R_DOOR).then(|| PlacedLock {
                locked: true,
                level: 50,
                key: Some(key(KEY)),
                key_pid: Some(0x0000_0700),
            }),
            hostile: reference == R_RAIDER,
            ..PlacedObject::default()
        })
        .collect();
    if reversed {
        objects.reverse();
    }
    // A 13 by 13 block of walkable hexes around the marker (hex 10,004).
    let hexes: Vec<u32> = (44..57u32)
        .flat_map(|row| (0..13u32).map(move |col| row * 200 + col))
        .collect();
    SimSlice {
        name: "testtown".into(),
        placement: Placement { objects },
        walkable: [(
            MAP.to_owned(),
            vec![WalkMask {
                elevation: 0,
                hexes,
            }],
        )]
        .into_iter()
        .collect(),
        exits: Default::default(),
    }
}

/// Writes the plugin, extracts it into `dir` (`Data/` and `mir/`) and
/// opens the MIR.
///
/// # Panics
///
/// When the directory cannot be written or the extraction fails: the
/// testkit is for tests.
pub fn extract(dir: &Path, reversed: bool) -> mir::Mir {
    let data = dir.join("Data");
    std::fs::create_dir_all(&data).expect("testkit: create Data");
    std::fs::write(data.join(PLUGIN), plugin(reversed)).expect("testkit: write the plugin");
    let out = dir.join("mir");
    mir::extract(&mir::ExtractOptions {
        data_dir: data,
        plugins: vec![PLUGIN.to_owned()],
        out: out.clone(),
        slice: mir::Slice {
            name: "testtown".into(),
            worldspace: "TSWorld".into(),
            x: 0..=0,
            y: 0..=0,
            interior_depth: 0,
        },
        jobs: 1,
        raw_store: mir::RawStore::All,
    })
    .expect("testkit: extract the town");
    mir::Mir::open(&out).expect("testkit: open the MIR")
}

/// Builds the town with its records in the given order.
///
/// # Panics
///
/// As [`extract`], and when the build fails.
pub fn town_with(reversed: bool) -> Module {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "transpile-testkit-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mir = extract(&dir, reversed);
    let module = crate::build(&mir, &slice(reversed), &SimConfig::default())
        .expect("testkit: build the town");
    drop(mir);
    let _ = std::fs::remove_dir_all(&dir);
    module
}

/// The town's module: everything the front end and the middle passes
/// make of it.
///
/// # Panics
///
/// As [`town_with`].
pub fn town() -> Module {
    town_with(false)
}
