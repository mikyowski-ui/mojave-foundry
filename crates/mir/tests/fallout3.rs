//! Extracting a Fallout 3 plugin list, on the synthetic install of
//! `mir::fixture::fallout3_town`: the game is detected and recorded, a
//! child worldspace takes its parent's land, the older subrecord sizes
//! decode, terminals, notes and packages have their tables, and the
//! referenced set names every reference something points at.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use fnv_esm::fixture::{CellSpec, PluginBuilder, WorldSpec};
use fnv_esm::{Cell, RawRecord, World};
use mir::fixture::{CHILD_WORLDSPACE, Fallout3Town, fallout3_town, mini_town};
use mir::{
    ExtractOptions, ExtractSummary, GameVariant, Mir, MirError, PrimitiveDoc, PrimitiveShape,
    RawStore, RefReason, Slice, extract, extract_for_game,
};

struct Run {
    _install: tempfile::TempDir,
    town: Fallout3Town,
    out: PathBuf,
    summary: ExtractSummary,
}

fn run() -> Run {
    let install = tempfile::tempdir().unwrap();
    let town = fallout3_town(install.path());
    let out = install.path().join("mir");
    let summary = extract(&town.options(&out, 2)).unwrap();
    Run {
        _install: install,
        town,
        out,
        summary,
    }
}

#[test]
fn the_game_is_detected_and_recorded() {
    let r = run();
    assert_eq!(r.summary.game, Some(GameVariant::Fallout3));
    assert_eq!(r.summary.bad_records, 0);
    assert_eq!(r.summary.issues, 0);
    let mir = Mir::open(&r.out).unwrap();
    assert_eq!(mir.game(), GameVariant::Fallout3);
    assert_eq!(mir.manifest().records_revision, mir::RECORDS_REVISION);
    let records = mir.records().unwrap();
    assert_eq!(records.game().unwrap(), GameVariant::Fallout3);
    assert_eq!(records.meta("game").unwrap().as_deref(), Some("fallout3"));
    let manifest = fs::read_to_string(r.out.join("manifest.json")).unwrap();
    assert!(manifest.contains("\"game\": \"fallout3\""));
    let report = mir.report_text().unwrap();
    assert!(report.contains("Game: Fallout 3 (`fallout3`)."), "{report}");
    assert!(report.contains("Not converted (New Vegas only): none."));
    assert!(report.contains("Terminals: 1 (3 menu items). Notes: 1. Packages: 1, 1 of them"));

    // The New Vegas fixture is New Vegas.
    let install = tempfile::tempdir().unwrap();
    let nv = mini_town(install.path());
    let out = install.path().join("mir");
    let summary = extract(&nv.options(&out, 1)).unwrap();
    assert_eq!(summary.game, Some(GameVariant::FalloutNewVegas));
    assert_eq!(
        Mir::open(&out).unwrap().game(),
        GameVariant::FalloutNewVegas
    );
}

#[test]
fn the_game_can_be_named_and_a_change_rebuilds_the_records() {
    let r = run();
    let options = r.town.options(&r.out, 1);
    let again = extract_for_game(&options, Some(GameVariant::Fallout3)).unwrap();
    assert!(!again.records_rebuilt);
    let other = extract_for_game(&options, Some(GameVariant::FalloutNewVegas)).unwrap();
    assert!(other.records_rebuilt);
    let mir = Mir::open(&r.out).unwrap();
    assert_eq!(mir.game(), GameVariant::FalloutNewVegas);
    assert_eq!(
        mir.records().unwrap().game().unwrap(),
        GameVariant::FalloutNewVegas
    );
}

fn tiny_install(version: f32, extra: Vec<RawRecord>) -> (tempfile::TempDir, ExtractOptions) {
    let install = tempfile::tempdir().unwrap();
    let data = install.path().join("Data");
    fs::create_dir_all(&data).unwrap();
    let mut plugin = PluginBuilder::new();
    plugin.version(version).master_file(true);
    for record in extra {
        plugin.add_raw(record);
    }
    plugin.world(
        WorldSpec::new(World::new(0x800u32, "TinyWorld"))
            .with_cell(CellSpec::new(Cell::exterior(0x801u32, 0, 0))),
    );
    fs::write(data.join("Tiny.esm"), plugin.build()).unwrap();
    let options = ExtractOptions {
        data_dir: data,
        plugins: vec!["Tiny.esm".to_owned()],
        out: install.path().join("mir"),
        slice: Slice {
            name: "tiny".to_owned(),
            worldspace: "TinyWorld".to_owned(),
            x: 0..=0,
            y: 0..=0,
            interior_depth: 0,
        },
        jobs: 1,
        raw_store: RawStore::All,
    };
    (install, options)
}

#[test]
fn an_unknown_version_is_an_error_unless_the_game_is_named() {
    let (_install, options) = tiny_install(1.0, Vec::new());
    match extract(&options) {
        Err(MirError::Plugin { plugin, source }) => {
            assert_eq!(plugin, "tiny.esm");
            assert!(matches!(source, fnv_esm::EsmError::UnknownGame { .. }));
        }
        other => panic!("expected an unknown game, got {other:?}"),
    }
    let summary = extract_for_game(&options, Some(GameVariant::Fallout3)).unwrap();
    assert_eq!(summary.game, Some(GameVariant::Fallout3));
}

#[test]
fn new_vegas_only_records_are_named_and_counted() {
    let records = vec![
        RawRecord::new(b"REPU", 0x900u32),
        RawRecord::new(b"REPU", 0x901u32),
        RawRecord::new(b"CHAL", 0x902u32),
    ];
    let (_install, options) = tiny_install(fnv_esm::fixture::PLUGIN_VERSION, records);
    let summary = extract(&options).unwrap();
    assert_eq!(summary.game, Some(GameVariant::FalloutNewVegas));
    let report = Mir::open(&options.out).unwrap().report_text().unwrap();
    assert!(
        report.contains("Not converted (New Vegas only): 3 records: `CHAL` 1, `REPU` 2.\n"),
        "{report}"
    );
}

/// The Fallout 3 master holds one record whose own form id names master
/// index 1 though the file has no masters. The engine reads such an id as
/// the file's own; so does the extractor, with a note instead of an error.
#[test]
fn an_own_form_id_past_the_master_list_is_the_plugins_own() {
    let stray = RawRecord::new(b"STAT", 0x0100_0950u32).with(b"EDID", b"Stray\0".to_vec());
    let (_install, options) = tiny_install(fnv_esm::fixture::FALLOUT3_PLUGIN_VERSION, vec![stray]);
    let summary = extract(&options).unwrap();
    assert_eq!(summary.bad_records, 0);
    assert_eq!(summary.notes, 1);
    let mir = Mir::open(&options.out).unwrap();
    let note = &mir.manifest().notes[0];
    assert_eq!(note.subject, "tiny.esm:000950");
    assert!(
        note.error.contains("past the plugin's 0 masters"),
        "{}",
        note.error
    );
    let records = mir.records().unwrap();
    assert!(records.bad_records().unwrap().is_empty());
    assert_eq!(
        records.find_editor_id("Stray").unwrap(),
        [mir::FormKey::new("tiny.esm", 0x950)]
    );
}

#[test]
fn a_child_worldspace_takes_its_parents_land() {
    let r = run();
    let mir = Mir::open(&r.out).unwrap();
    let own_less = mir.cell(CHILD_WORLDSPACE, 0, 0).unwrap().unwrap();
    let land = own_less.land.expect("the parent's land");
    assert_eq!(land.form, r.town.parent_land);
    assert!(land.heights.iter().all(|&h| h == 100.0));
    let own = mir.cell(CHILD_WORLDSPACE, 1, 0).unwrap().unwrap();
    assert_eq!(
        own.land.as_ref().expect("its own land").form,
        r.town.child_land
    );
    // (2, 0) has no cell record; the persistent reference standing there
    // is a note, not an issue, and is left out.
    assert!(mir.cell(CHILD_WORLDSPACE, 2, 0).unwrap().is_none());
    let manifest = mir.manifest();
    assert!(manifest.issues.is_empty(), "{:?}", manifest.issues);
    assert_eq!(r.summary.notes, 2);
    let subjects: Vec<&str> = manifest.notes.iter().map(|n| n.subject.as_str()).collect();
    assert!(subjects.contains(&r.town.gridless_ref.to_string().as_str()));
    assert!(subjects.contains(&r.town.child_cell.to_string().as_str()));
    assert!(
        manifest
            .notes
            .iter()
            .any(|n| n.error.contains("has no cell record"))
    );
    assert!(
        manifest
            .notes
            .iter()
            .any(|n| n.error.contains("land of the parent"))
    );
    // The other persistent reference stands in (1, 0).
    assert!(own.reference(&r.town.persistent_ref).is_some());
    assert!(mir.report_text().unwrap().contains("2 notes.\n"));
}

/// A sphere primitive on an activator of the Fallout 3 install (header
/// version 0.94) is a trigger volume with its shape and bounds kept; the
/// New Vegas install's box is checked in `extract.rs`.
#[test]
fn a_fallout3_trigger_volume_keeps_its_primitive() {
    let r = run();
    let mir = Mir::open(&r.out).unwrap();
    let doc = mir
        .cell(CHILD_WORLDSPACE, 0, 0)
        .unwrap()
        .expect("cell 0, 0");
    let trigger = doc.reference(&r.town.trigger_volume).unwrap();
    assert!(trigger.trigger_volume);
    assert_eq!(trigger.base_kind.as_deref(), Some("ACTI"));
    assert_eq!(
        trigger.primitive,
        Some(PrimitiveDoc {
            bounds: [192.0, 192.0, 192.0],
            shape: PrimitiveShape::Sphere,
        })
    );
    let flagged: Vec<_> = doc.references.iter().filter(|r| r.trigger_volume).collect();
    assert_eq!(flagged.len(), 1);
    // Nothing else carries a primitive, and the field stays out of their JSON.
    assert_eq!(
        doc.references
            .iter()
            .filter(|r| r.primitive.is_some())
            .count(),
        1
    );
    let json = fs::read_to_string(r.out.join("cells/mwtownworld/0_0.json")).unwrap();
    let compact: String = json.split_whitespace().collect();
    assert_eq!(compact.matches("\"primitive\"").count(), 1);
    assert!(compact.contains("\"shape\":\"sphere\""));
}

#[test]
fn the_older_subrecord_sizes_decode() {
    let r = run();
    let records = Mir::open(&r.out).unwrap().records().unwrap();
    let conn = records.connection();
    let (flags, responses): (Option<i64>, i64) = conn
        .query_row(
            "SELECT flags, responses FROM infos WHERE form = ?1",
            [r.town.info.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((flags, responses), (Some(5), 1));
    let (number, emotion, text): (Option<i64>, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT number, emotion_type, text FROM info_responses WHERE info = ?1",
            [r.town.info.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((number, emotion), (Some(1), Some(5)));
    assert_eq!(text.as_deref(), Some("Hello there."));
    let health: Option<i64> = conn
        .query_row(
            "SELECT base_health FROM npcs WHERE form = ?1",
            [r.town.settler.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(health, Some(75));
    let (skill, ap): (Option<i64>, Option<f64>) = conn
        .query_row(
            "SELECT skill, action_points FROM weapons WHERE form = ?1",
            [r.town.weapon.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((skill, ap), (Some(33), Some(30.0)));
}

#[test]
fn terminals_notes_and_packages_read_back() {
    let r = run();
    let records = Mir::open(&r.out).unwrap().records().unwrap();
    let terminals = records.terminals().unwrap();
    assert_eq!(terminals.len(), 1);
    let terminal = records.terminal(&r.town.terminal).unwrap().unwrap();
    assert_eq!(terminal, terminals[0]);
    assert_eq!(terminal.header_text.as_deref(), Some("WELCOME"));
    assert_eq!(terminal.items.len(), 3);
    assert_eq!(terminal.items[0].text.as_deref(), Some("Open the crate"));
    assert_eq!(terminal.items[0].result_text.as_deref(), Some("Opened."));
    assert_eq!(
        terminal.items[0].script_source.as_deref(),
        Some("crate.Activate player")
    );
    assert_eq!(terminal.items[1].display_note, Some(r.town.note.clone()));
    assert_eq!(terminal.items[2].conditions.len(), 1);
    assert_eq!(terminal.items[2].conditions[0].function, 14);
    assert_eq!(terminal.items[2].conditions[0].context, "item 2");

    let notes = records.notes().unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].form, r.town.note);
    assert_eq!(notes[0].note_type, Some(fnv_esm::Note::TYPE_TEXT));
    assert_eq!(notes[0].text.as_deref(), Some("Day one."));

    let package = records.package(&r.town.package).unwrap().unwrap();
    assert_eq!(records.packages().unwrap(), std::slice::from_ref(&package));
    assert_eq!(package.package_type, Some(12));
    assert_eq!(package.type_name.as_deref(), Some("sandbox"));
    let location = package.location.clone().unwrap();
    assert_eq!(location.form, Some(r.town.package_location.clone()));
    assert_eq!(location.radius, 256);
    let target = package.target.clone().unwrap();
    assert_eq!(target.form, Some(r.town.package_target.clone()));
    assert_eq!(package.schedule.map(|s| s.hour), Some(-1));
    assert_eq!(
        package.scripts,
        [("on begin".to_owned(), "SetStage MWQuest 10".to_owned())]
    );
    assert_eq!(
        records.actor_packages(&r.town.settler).unwrap(),
        std::slice::from_ref(&r.town.package)
    );
    assert_eq!(
        records.package_actors(&r.town.package).unwrap(),
        std::slice::from_ref(&r.town.settler)
    );
    let targets = records.quest_targets(&r.town.quest).unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].objective, 10);
    assert_eq!(targets[0].target, Some(r.town.quest_target.clone()));
    assert_eq!(
        records.script_references(&r.town.script).unwrap(),
        [("script".to_owned(), r.town.script_target.clone())]
    );
    assert_eq!(
        records.script_references(&r.town.terminal).unwrap(),
        [("item 0".to_owned(), r.town.terminal_target.clone())]
    );
}

#[test]
fn the_referenced_set_names_every_reason() {
    let r = run();
    let records = Mir::open(&r.out).unwrap().records().unwrap();
    let t = &r.town;
    let expected: BTreeMap<_, BTreeSet<RefReason>> = [
        (&t.quest_target, RefReason::QuestTarget),
        (&t.script_target, RefReason::Script),
        (&t.terminal_target, RefReason::Script),
        (&t.condition_target, RefReason::Condition),
        (&t.link_target, RefReason::LinkedRef),
        (&t.enable_parent, RefReason::EnableParent),
        (&t.enable_child, RefReason::EnableChild),
        (&t.package_target, RefReason::PackageTarget),
        (&t.package_location, RefReason::PackageLocation),
    ]
    .into_iter()
    .map(|(form, reason)| (form.clone(), BTreeSet::from([reason])))
    .collect();
    assert_eq!(records.referenced_ref_reasons().unwrap(), expected);
    let set = records.referenced_refs().unwrap();
    assert_eq!(set, expected.keys().cloned().collect());
    for unreferenced in [&t.plain_ref, &t.settler_ref, &t.link_source] {
        assert!(!set.contains(unreferenced));
    }
    let referrals = records.ref_referrals().unwrap();
    let by_terminal = referrals
        .iter()
        .find(|x| x.reference == t.terminal_target)
        .unwrap();
    assert_eq!(by_terminal.by, t.terminal);
    assert_eq!(by_terminal.context, "item 0");
    let child = referrals
        .iter()
        .find(|x| x.reason == RefReason::EnableChild)
        .unwrap();
    assert_eq!(child.by, t.enable_parent);
}

#[test]
fn a_directory_from_before_the_game_was_recorded_still_opens() {
    let r = run();
    let path = r.out.join("manifest.json");
    let text = fs::read_to_string(&path).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("game");
    object.remove("records_revision");
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let mir = Mir::open(&r.out).unwrap();
    assert_eq!(mir.game(), GameVariant::FalloutNewVegas);
    assert_eq!(mir.manifest().records_revision, 0);

    // A records.sqlite without the new tables and metadata: the game reads
    // as New Vegas and the new readers say what is missing.
    {
        let conn = rusqlite::Connection::open(r.out.join("records.sqlite")).unwrap();
        conn.execute_batch(
            "DELETE FROM meta WHERE key IN ('game', 'records_revision');
             DROP TABLE terminals; DROP TABLE packages; DROP TABLE quest_targets;",
        )
        .unwrap();
    }
    let records = mir.records().unwrap();
    assert_eq!(records.game().unwrap(), GameVariant::FalloutNewVegas);
    assert!(matches!(
        records.terminals(),
        Err(MirError::MissingTable("terminals"))
    ));
    assert!(matches!(
        records.referenced_refs(),
        Err(MirError::MissingTable("quest_targets"))
    ));
    let report = mir::report(&mir, &mir::ReportParams::default()).unwrap();
    assert!(report.contains("not listed (records.sqlite predates them)"));

    // Extracting again rebuilds it, because the manifest's revision is old.
    let again = extract(&r.town.options(&r.out, 1)).unwrap();
    assert!(again.records_rebuilt);
    let mir = Mir::open(&r.out).unwrap();
    assert_eq!(mir.game(), GameVariant::Fallout3);
    assert_eq!(mir.records().unwrap().terminals().unwrap().len(), 1);
}

#[test]
fn a_fallout3_extract_is_deterministic_across_job_counts() {
    let install = tempfile::tempdir().unwrap();
    let town = fallout3_town(install.path());
    let a = install.path().join("a");
    let b = install.path().join("b");
    extract(&town.options(&a, 1)).unwrap();
    extract(&town.options(&b, 4)).unwrap();
    for name in ["records.sqlite", "manifest.json", "report.md"] {
        assert!(
            fs::read(a.join(name)).unwrap() == fs::read(b.join(name)).unwrap(),
            "{name} differs"
        );
    }
}
