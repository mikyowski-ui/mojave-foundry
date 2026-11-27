//! The build on a small synthetic plugin (spec 12.1, "Front end"): built
//! in memory with the `fnv_esm` builders, extracted with `mir`, built into
//! a module whose JSON is the snapshot. Everything in it is invented.

use std::path::Path;

use fnv_esm::fixture::{CellSpec, FALLOUT3_PLUGIN_VERSION, PluginBuilder, WorldSpec};
use fnv_esm::{
    ActorBase, Cell, Comparison, Condition, Door, FactionRank, FormId, Info, LocalVariable, Note,
    Npc, Package, PackageLocation, PlacedKind, PlacedRef, Quest, QuestLogEntry, QuestObjective,
    QuestStage, QuestTarget, RawRecord, Response, Script, ScriptBody, ScriptReference, Static,
    Terminal, TerminalMenuItem, Topic, TopicData, World,
};
use transpile::diag::Category;
use transpile::ir::{
    Cond, Expr, FormKey, GlobalPurpose, Module, PlacedObject, Placement, RefUses, SimConfig, SimId,
    SimKind, SimSlice, SpeakerSet, Stmt, TextRole, VarRef, VarTy,
};

const PLUGIN: &str = "TestTown.esm";

const MARKER: u32 = 0x800;
const DOOR: u32 = 0x801;
const NOTE: u32 = 0x802;
const TERMINAL: u32 = 0x803;
const SUB_TERMINAL: u32 = 0x804;
const MAYOR: u32 = 0x810;
const GUARD: u32 = 0x811;
const FACTION: u32 = 0x820;
const VOICE: u32 = 0x830;
const SANDBOX: u32 = 0x850;
const TRAVEL: u32 = 0x851;
const DOOR_SCRIPT: u32 = 0x860;
const MAYOR_SCRIPT: u32 = 0x861;
const QUEST_SCRIPT: u32 = 0x862;
const QUEST: u32 = 0x870;
const OTHER_QUEST: u32 = 0x871;
const COUNT: u32 = 0x880;
const RATE: u32 = 0x881;
const GREETING: u32 = 0x890;
const JOB: u32 = 0x892;
const WORLD: u32 = 0x900;
const CELL: u32 = 0x901;
const R_MAYOR: u32 = 0xC01;
const R_GUARD: u32 = 0xC02;
const R_MARKER: u32 = 0xC03;
const R_DOOR: u32 = 0xC04;
const R_PARENT: u32 = 0xC05;
const R_CHILD: u32 = 0xC06;
const R_OPPOSITE: u32 = 0xC07;
const R_TERMINAL: u32 = 0xC08;
const R_NOTE: u32 = 0xC09;
const R_LONELY: u32 = 0xC0A;
const R_FAR: u32 = 0xC0B;

fn key(id: u32) -> FormKey {
    FormKey::new(PLUGIN, id)
}

fn zstr(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    bytes
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
    let mut info = Info::new(form, QUEST, responses);
    if let Some(data) = &mut info.data {
        data.flags = Some(flags);
    }
    info
}

/// The records of the plugin, each as a closure that adds it, so that a
/// test can add them in another order.
/// Adds one or more records to a plugin.
type Adder = Box<dyn Fn(&mut PluginBuilder)>;

fn records() -> Vec<Adder> {
    let mut out: Vec<Adder> = Vec::new();
    out.push(Box::new(|p| {
        p.add(Static::new(MARKER, "TSMarker", ""));
    }));
    out.push(Box::new(|p| {
        let door = Door::new(DOOR, "TSDoor", "Door", "");
        p.add(Door {
            common: door.common.clone().with_script(DOOR_SCRIPT),
            ..door
        });
    }));
    out.push(Box::new(|p| {
        p.add(Note::text(
            NOTE,
            "TSNote",
            "Mayor's Log",
            "Day one. The door sticks.",
        ));
    }));
    out.push(Box::new(|p| {
        let mut sub = TerminalMenuItem::new("Read the log", "");
        sub.display_note = Some(FormId(NOTE));
        sub.flags = Some(1);
        let mut unlock = TerminalMenuItem::new("Unlock the door", "Unlocked.");
        unlock.script = body("TSDoorRef.Unlock", &[R_DOOR]);
        unlock.conditions =
            vec![Condition::float(58, Comparison::GreaterOrEqual, 10.0).with_params(QUEST, 0)];
        let mut open = TerminalMenuItem::new("Maintenance", "");
        open.sub_terminal = Some(FormId(SUB_TERMINAL));
        let mut terminal =
            Terminal::new(TERMINAL, "TSTerminal", "Town Terminal", "WELCOME, CITIZEN");
        terminal.password_note = Some(FormId(NOTE));
        terminal.items = vec![sub, unlock, open];
        p.add(terminal);
        let mut back = Terminal::new(SUB_TERMINAL, "TSSubTerminal", "Maintenance", "MAINTENANCE");
        back.items = vec![TerminalMenuItem::new(
            "Run diagnostics",
            "All systems nominal.",
        )];
        p.add(back);
    }));
    out.push(Box::new(|p| {
        let mut mayor = Npc::new(MAYOR, "TSMayor", "Mayor");
        mayor.common.script = Some(FormId(MAYOR_SCRIPT));
        mayor.packages = vec![FormId(SANDBOX)];
        mayor.factions = vec![FactionRank {
            faction: FormId(FACTION),
            rank: 1,
        }];
        mayor.voice = Some(FormId(VOICE));
        // Female and essential.
        mayor.base = Some(ActorBase {
            flags: 0x03,
            ..ActorBase::default()
        });
        p.add(mayor);
    }));
    out.push(Box::new(|p| {
        let mut guard = Npc::new(GUARD, "TSGuard", "Guard");
        guard.packages = vec![FormId(TRAVEL)];
        guard.voice = Some(FormId(VOICE));
        p.add(guard);
    }));
    out.push(Box::new(|p| {
        p.add_raw(RawRecord::new(b"FACT", FACTION).with(b"EDID", zstr("TSTownFaction")));
        p.add_raw(RawRecord::new(b"VTYP", VOICE).with(b"EDID", zstr("TSVoice")));
    }));
    out.push(Box::new(|p| {
        let mut sandbox = Package::new(SANDBOX, "TSMayorSandbox", 12);
        sandbox.location = Some(PackageLocation {
            location_type: 0,
            value: R_MARKER,
            radius: 512,
        });
        sandbox.on_begin = Some(fnv_esm::PackageEvent {
            idle: Some(FormId::NULL),
            script: body("SetStage TSQuest 20", &[QUEST]),
            topic: Some(FormId::NULL),
        });
        p.add(sandbox);
        let mut travel = Package::new(TRAVEL, "TSGuardTravel", 6);
        travel.location = Some(PackageLocation {
            location_type: 0,
            value: R_DOOR,
            radius: 0,
        });
        travel.conditions = vec![Condition::float(77, Comparison::Less, 50.0)];
        p.add(travel);
    }));
    out.push(Box::new(|p| {
        let source = "scn TSDoorScript\r\nshort opened\r\n\r\nbegin OnActivate\r\n\tif IsActionRef player == 1\r\n\t\tset opened to 1\r\n\t\tTSParentRef.Enable\r\n\t\tTSLonelyRef.Enable\r\n\t\tTSFarRef.Disable\r\n\t\tActivate\r\n\tendif\r\nend\r\n";
        let mut script = Script::new(DOOR_SCRIPT, "TSDoorScript", source);
        script.body.references = vec![
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
    }));
    out.push(Box::new(|p| {
        let source = "scn TSMayorScript\nshort mood\n\nbegin GameMode\n\tif GetStage TSQuest >= 10 && TSGlobalCount > 2\n\t\tset mood to 1\n\tendif\nend\n";
        let mut script = Script::new(MAYOR_SCRIPT, "TSMayorScript", source);
        script.body.locals = vec![LocalVariable {
            index: 1,
            flags: 1,
            name: Some("mood".into()),
        }];
        p.add(script);
    }));
    out.push(Box::new(|p| {
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
    }));
    out.push(Box::new(|p| {
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
            script: body("StartQuest TSOtherQuest", &[OTHER_QUEST]),
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
    }));
    out.push(Box::new(|p| {
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
    }));
    out.push(Box::new(|p| {
        // A greeting with a say-once INFO for the mayor, a random pair for
        // both speakers, and one for the guard with a fallback term.
        let mut hello = info(0x891, 0x04, &["Welcome to town.", "Mind the door."]);
        hello.conditions = vec![is_id(MAYOR)];
        hello.choices = vec![FormId(JOB)];
        hello.result_end = body(
            "SetStage TSQuest 10\nset TSQuest.qCount to TSQuest.qCount + 1\nset mood to 2",
            &[QUEST],
        );
        let mut either = info(0x893, 0x02, &["Nice day."]);
        either.conditions = vec![
            is_id(MAYOR).or(),
            is_id(GUARD),
            Condition::global(58, Comparison::GreaterOrEqual, COUNT).with_params(QUEST, 0),
        ];
        let mut guard = info(0x894, 0x20, &["Move along."]);
        guard.conditions = vec![
            is_id(GUARD),
            Condition::float(255, Comparison::Equal, 1.0),
            Condition::float(79, Comparison::GreaterOrEqual, 2.0).with_params(QUEST, 1),
        ];
        let greeting = Topic::new(GREETING, "GREETING", "", QUEST);
        p.topic(greeting, vec![hello, either, guard]);
        let mut job = info(0x895, 0x01, &["Find my key."]);
        job.conditions = vec![is_id(MAYOR)];
        job.prompt = Some("Got any work?".into());
        let mut topic = Topic::new(JOB, "TSJobTopic", "Work", QUEST);
        topic.data = Some(TopicData {
            topic_type: 0,
            flags: Some(0x02),
        });
        p.topic(topic, vec![job]);
    }));
    out
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
    vec![
        named(
            PlacedRef::new(PlacedKind::Npc, R_MAYOR, MAYOR, at(1), [0.0; 3]),
            "TSMayorRef",
        ),
        PlacedRef::new(PlacedKind::Npc, R_GUARD, GUARD, at(2), [0.0; 3]),
        named(object(R_MARKER, MARKER, 3), "TSMarkerRef"),
        named(object(R_DOOR, DOOR, 4), "TSDoorRef"),
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
    ]
}

/// Writes the plugin with its records in the given order and extracts it.
fn extract(dir: &Path, reversed: bool) -> mir::Mir {
    let data = dir.join("Data");
    std::fs::create_dir_all(&data).unwrap();
    let mut plugin = PluginBuilder::new();
    plugin
        .version(FALLOUT3_PLUGIN_VERSION)
        .master_file(true)
        .author("transpile test");
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
    std::fs::write(data.join(PLUGIN), plugin.build()).unwrap();
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
    .unwrap();
    mir::Mir::open(&out).unwrap()
}

/// What the map stage placed: every reference but `TSFarRef`.
fn slice(reversed: bool) -> SimSlice {
    use transpile::ir::PlacedKind::*;
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
    ];
    let mut objects: Vec<PlacedObject> = placed
        .iter()
        .enumerate()
        .map(|(i, &(reference, base, kind))| PlacedObject {
            map: "tstown".into(),
            map_number: 1,
            elevation: 0,
            hex: 10_000 + 2 * i as u32,
            kind,
            object_id: Some(i as u32 + 1),
            pid: None,
            reference: Some(key(reference)),
            base: Some(key(base)),
            hidden: false,
            ..PlacedObject::default()
        })
        .collect();
    if reversed {
        objects.reverse();
    }
    SimSlice {
        name: "testtown".into(),
        placement: Placement { objects },
        ..SimSlice::default()
    }
}

fn build(reversed: bool) -> Module {
    let dir = tempfile::tempdir().unwrap();
    let mir = extract(dir.path(), reversed);
    transpile::build(&mir, &slice(reversed), &SimConfig::default()).unwrap()
}

#[test]
fn module_snapshot() {
    let module = build(false);
    let json = module.to_json();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/build/town.module.json"
    );
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(path, &json).unwrap();
    }
    let expected = std::fs::read_to_string(path).expect("snapshot; run with UPDATE_SNAPSHOTS=1");
    assert_eq!(json, expected);
}

#[test]
fn stable_under_reordering_and_repeated_builds() {
    let first = build(false).to_json();
    assert_eq!(build(false).to_json(), first, "two builds differ");
    assert_eq!(
        build(true).to_json(),
        first,
        "reordered input changes the module"
    );
}

#[test]
fn build_module_ignores_source_list_order() {
    let dir = tempfile::tempdir().unwrap();
    let mir = extract(dir.path(), false);
    let mut source = transpile::source::load(&mir, &slice(false)).unwrap();
    let config = SimConfig::default();
    let first = transpile::build_module(&source, &slice(false), &config).unwrap();
    source.names.reverse();
    source.plugins = source.plugins.clone();
    let second = transpile::build_module(&source, &slice(true), &config).unwrap();
    assert_eq!(first.to_json(), second.to_json());
}

#[test]
fn what_the_module_holds() {
    let m = build(false);
    // Labels come first, then the texts in SimId order of their phase.
    assert_eq!(m.texts[0].role, TextRole::Label);
    assert_eq!(m.texts[0].source, "[Continue]");

    // References: scripted, named by scripts, conditions, packages, quest
    // targets and enable groups; the one outside the slice is a note.
    let why = |r: u32| m.refs.get(&key(r)).map(|e| e.why);
    let has = |r: u32, bit: RefUses| why(r).is_some_and(|w| w.contains(bit));
    assert!(has(R_DOOR, RefUses::SCRIPT) && has(R_DOOR, RefUses::PACKAGE));
    assert!(has(R_MAYOR, RefUses::SCRIPT) && has(R_MAYOR, RefUses::QUEST_TARGET));
    assert!(has(R_MARKER, RefUses::PACKAGE));
    assert!(has(R_PARENT, RefUses::ENABLE_PARENT) && has(R_PARENT, RefUses::SCRIPT));
    assert!(has(R_CHILD, RefUses::ENABLE_PARENT));
    assert!(
        why(R_GUARD).is_none(),
        "nothing needs the guard's reference"
    );
    assert!(m.refs[&key(R_MAYOR)].mobile);
    assert_eq!(m.refs[&key(R_DOOR)].area, "tstown");
    assert!(!m.refs.contains_key(&key(R_FAR)));
    assert!(
        m.diagnostics
            .iter()
            .any(|d| d.category == Category::RefOutsideSlice && d.subject == key(R_FAR).as_str())
    );

    // Enable groups: the parent's two children with their opposite flags,
    // and a group of one for the reference a script enables.
    let group = &m.groups[&SimId::of(SimKind::Group, &key(R_PARENT))];
    assert!(!group.initially_enabled);
    assert_eq!(
        group.children,
        vec![(key(R_CHILD), false), (key(R_OPPOSITE), true)]
    );
    let lonely = &m.groups[&SimId::of(SimKind::Group, &key(R_LONELY))];
    assert_eq!(lonely.children, vec![(key(R_LONELY), false)]);

    // Globals: the two GLOBs the code names, and one per group.
    let count = &m.globals[&SimId::of(SimKind::Global, &key(COUNT))];
    assert_eq!(
        (count.ty, count.initial, count.purpose),
        (VarTy::Int, 3, GlobalPurpose::Glob)
    );
    let rate = &m.globals[&SimId::of(SimKind::Global, &key(RATE))];
    assert_eq!((rate.ty, rate.initial), (VarTy::Float, 500));
    assert_eq!(rate.name, "TSGLOBALRATE");
    // (The middle passes add quest state, notes and once-flags; the front
    // end's own globals keep their order among them.)
    let order: Vec<GlobalPurpose> = m
        .globals_in_allocation_order()
        .iter()
        .map(|g| g.purpose)
        .filter(|p| matches!(p, GlobalPurpose::Glob | GlobalPurpose::Group))
        .collect();
    assert_eq!(
        order,
        [
            GlobalPurpose::Glob,
            GlobalPurpose::Glob,
            GlobalPurpose::Group,
            GlobalPurpose::Group
        ]
    );

    // Quests: the one the slice names, then the one its stage script
    // starts; stages by index; stage scripts see the quest's variables.
    assert!(
        m.quests
            .contains_key(&SimId::of(SimKind::Quest, &key(OTHER_QUEST)))
    );
    let quest = &m.quests[&SimId::of(SimKind::Quest, &key(QUEST))];
    assert!(quest.start_enabled);
    assert_eq!(
        quest.stages.iter().map(|s| s.index).collect::<Vec<_>>(),
        [10, 20, 30]
    );
    assert!(quest.stages[2].entries[0].complete);
    assert_eq!(quest.vars.len(), 2);
    let stage_script = &m.scripts[quest.stages[0].entries[0].script.as_ref().unwrap()];
    assert_eq!(
        stage_script.blocks[0].body[1],
        Stmt::Set(
            VarRef::Quest {
                quest: quest.id.clone(),
                index: 1,
                ty: VarTy::Int
            },
            Expr::Binary(
                transpile::ir::BinOp::Add,
                Box::new(Expr::Var(VarRef::Quest {
                    quest: quest.id.clone(),
                    index: 1,
                    ty: VarTy::Int
                })),
                Box::new(Expr::Int(1))
            )
        )
    );
    assert!(matches!(quest.stages[1].entries[0].condition, Cond::Any(_)));

    // Dialogue: speakers from GetIsID; the OR run; a result script that
    // sees the speaker's variables; the fallback term.
    let info = |id: u32| &m.infos[&SimId::of(SimKind::Info, &key(id))];
    assert_eq!(info(0x891).speakers, SpeakerSet::Bases(vec![key(MAYOR)]));
    assert!(info(0x891).flags.say_once);
    assert_eq!(
        info(0x893).speakers,
        SpeakerSet::Bases(vec![key(MAYOR), key(GUARD)])
    );
    assert!(info(0x893).flags.random);
    assert!(info(0x894).flags.random_end);
    let Cond::All(terms) = &info(0x893).condition else {
        panic!("{:?}", info(0x893).condition)
    };
    assert!(matches!(terms[0], Cond::Any(_)));
    let end = info(0x891).end.as_ref().unwrap();
    assert_eq!(
        end.body[2],
        Stmt::Set(
            VarRef::Local {
                script: SimId::of(SimKind::Script, &key(MAYOR_SCRIPT)),
                index: 1,
                ty: VarTy::Int
            },
            Expr::Int(2)
        )
    );
    let Cond::All(guard_terms) = &info(0x894).condition else {
        panic!()
    };
    assert!(
        guard_terms
            .iter()
            .any(|t| matches!(t, Cond::Fallback { function: 255, .. }))
    );
    assert_eq!(
        info(0x891).choices,
        vec![SimId::of(SimKind::Topic, &key(JOB))]
    );
    let topic = &m.topics[&SimId::of(SimKind::Topic, &key(GREETING))];
    assert_eq!(
        topic.infos,
        [0x891, 0x893, 0x894].map(|i| SimId::of(SimKind::Info, &key(i)))
    );
    assert!(m.topics[&SimId::of(SimKind::Topic, &key(JOB))].top_level);
    assert!(info(0x895).flags.goodbye && info(0x895).prompt.is_some());

    // Terminals, notes, packages and actors.
    let terminal = &m.terminals[&SimId::of(SimKind::Terminal, &key(TERMINAL))];
    assert_eq!(terminal.items.len(), 3);
    assert!(terminal.items[0].add_note);
    assert_eq!(
        terminal.items[2].submenu,
        Some(SimId::of(SimKind::Terminal, &key(SUB_TERMINAL)))
    );
    assert!(terminal.items[1].script.is_some());
    assert!(
        m.terminals
            .contains_key(&SimId::of(SimKind::Terminal, &key(SUB_TERMINAL)))
    );
    assert!(m.notes.contains_key(&SimId::of(SimKind::Note, &key(NOTE))));
    let sandbox = &m.packages[&SimId::of(SimKind::Package, &key(SANDBOX))];
    assert_eq!(sandbox.runner, transpile::ir::Runner::Sandbox);
    assert!(sandbox.events[0].1.is_some());
    let mayor = &m.actors[&key(MAYOR)];
    assert!(mayor.female && mayor.essential);
    assert_eq!(mayor.factions, vec![(key(FACTION), 1)]);
    assert_eq!(mayor.placements, vec![key(R_MAYOR)]);
    assert_eq!(
        mayor.script,
        Some(SimId::of(SimKind::Script, &key(MAYOR_SCRIPT)))
    );
    assert!(
        m.diagnostics
            .iter()
            .any(|d| d.category == Category::IgnoredFlag)
    );

    // No script of the fixture has an error.
    for script in m.scripts.values() {
        assert!(
            script
                .diagnostics
                .iter()
                .all(|d| d.severity != transpile::diag::Severity::Error),
            "{}: {:#?}",
            script.id,
            script.diagnostics
        );
    }
}

#[test]
fn a_reference_placed_twice_on_one_map_is_an_error() {
    let mut slice = slice(false);
    let mut twice = slice.placement.objects[0].clone();
    twice.hex += 1;
    slice.placement.objects.push(twice);
    let source = transpile::source::SimSource::default();
    let error = transpile::build_module(&source, &slice, &SimConfig::default()).unwrap_err();
    assert!(error.to_string().contains("placed twice"), "{error}");
}
