//! The lowering of the testkit's town (spec 12.1 "Codegen", "Tables",
//! "Determinism"): every program loads under the engine's Fallout 2 word
//! set with no stack-model finding; the tables read back through the
//! engine's parsers; two lowerings, and a lowering of the town built from
//! records in the opposite order, give the same bytes.

mod support;

use std::sync::OnceLock;

use support::{assert_program_ok, town_facts};
use transpile::ir::{BaseFiles, Module, SimConfig};
use transpile::lowered::LowerOutput;
use transpile::{lower, lower_layer, testkit};

fn town() -> &'static Module {
    static TOWN: OnceLock<Module> = OnceLock::new();
    TOWN.get_or_init(testkit::town)
}

/// A base `scripts.lst` of three rows, a `vault13.gam` of two globals and
/// small base Pip-Boy tables, all invented.
pub fn base_files() -> BaseFiles {
    BaseFiles {
        scripts_lst: Some(
            "test0.int ; first # local_vars=1\ntest1.int ; second\ntest2.int ; third # local_vars=2\n"
                .into(),
        ),
        vault13_gam: Some("GAME_GLOBAL_VARS:\nGVAR_ONE := 1;\nGVAR_TWO := 0;\n".into()),
        quests_txt: Some("1500, 101, 0, 1, 2\n".into()),
        holodisk_txt: Some("1, 1000, 1500\n".into()),
        quests_msg: Some("{101}{}{An old quest.}\n".into()),
        pipboy_msg: Some("{1000}{}{Old disk}\n".into()),
        map_msg: Some("{1500}{}{Somewhere}\n".into()),
    }
}

fn lowered() -> &'static LowerOutput {
    static OUT: OnceLock<LowerOutput> = OnceLock::new();
    OUT.get_or_init(|| {
        lower_layer(
            town(),
            &town_facts(false),
            &base_files(),
            &SimConfig::default(),
        )
        .expect("the town lowers")
    })
}

#[test]
fn every_program_loads_and_has_no_stack_finding() {
    let out = lowered();
    let ints: Vec<_> = out
        .lowered
        .files
        .iter()
        .filter(|(p, _)| p.ends_with(".int"))
        .collect();
    assert!(
        ints.len() >= 6,
        "{:?}",
        out.lowered.files.keys().collect::<Vec<_>>()
    );
    let dump = std::env::var_os("TRANSPILE_DUMP_INTS");
    for (path, bytes) in ints {
        assert_program_ok(path, bytes);
        if let Some(dir) = &dump {
            let file = std::path::Path::new(dir).join(path.trim_start_matches("scripts/"));
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(file, bytes).unwrap();
        }
    }
    let kinds: Vec<&str> = out.plan.programs.iter().map(|p| p.kind).collect();
    for kind in ["map", "critter", "object", "terminal", "note", "dispatcher"] {
        assert!(kinds.contains(&kind), "no {kind} program: {kinds:?}");
    }
}

#[test]
fn every_program_has_root_then_start() {
    for program in &lowered().plan.programs {
        assert_eq!(program.procedures[0], "root", "{}", program.stem);
        assert_eq!(program.procedures[1], "start", "{}", program.stem);
    }
}

#[test]
fn rows_follow_the_base_list_and_tables_read_back() {
    let out = lowered();
    let files = &out.lowered.files;
    let lst = vault_formats::LstTable::parse(
        &files["scripts/scripts.lst"],
        vault_formats::LstKind::Script,
    )
    .unwrap();
    assert_eq!(lst.entries().len(), 3 + out.plan.programs.len());
    for program in &out.plan.programs {
        let entry = &lst.entries()[program.row as usize];
        assert_eq!(
            entry.name.as_deref(),
            Some(format!("{}.int", program.stem).as_bytes())
        );
        assert_eq!(entry.local_vars, Some(program.local_vars as i32));
        if program.messages {
            let path = format!("text/english/dialog/{}.msg", program.stem);
            vault_formats::MsgCatalog::parse_strict(&files[&path]).unwrap();
        }
    }
    let gam = vault_formats::GamFile::parse(
        &files["data/vault13.gam"],
        vault_formats::GamSection::Campaign,
    )
    .unwrap();
    assert_eq!(gam.values.len(), 2 + town().globals.len());
    assert_eq!(&gam.values[..2], &[1, 0]);
    let mut diags = vault_formats::fallout2::text::Diagnostics::default();
    let quests =
        vault_formats::fallout2::lines::quests(&files["data/quests.txt"], &mut diags).unwrap();
    assert_eq!(quests.len(), 1 + out.plan.summary.quest_rows as usize);
    assert!(out.plan.summary.quest_rows >= 2);
    let disks =
        vault_formats::fallout2::lines::holodisks(&files["data/holodisk.txt"], &mut diags).unwrap();
    assert_eq!(disks.len(), 1 + out.plan.summary.holodisks as usize);
    for msg in ["quests", "pipboy", "map"] {
        let catalog = vault_formats::MsgCatalog::parse_strict(
            &files[&format!("text/english/game/{msg}.msg")],
        )
        .unwrap();
        assert!(catalog.entries().len() >= 2, "{msg}");
    }
    let pipboy =
        vault_formats::MsgCatalog::parse_strict(&files["text/english/game/pipboy.msg"]).unwrap();
    let texts: Vec<String> = pipboy
        .entries()
        .values()
        .map(|m| String::from_utf8_lossy(&m.text).into_owned())
        .collect();
    assert!(texts.iter().any(|t| t == "**END-DISK**"));
    // The note reads in the Pip-Boy: title, then its wrapped lines.
    assert!(texts.iter().any(|t| t == "Mayor's Log"));
    assert!(
        texts.iter().any(|t| t == "Day one. The door sticks."),
        "{texts:?}"
    );
    assert!(texts.iter().any(|t| t == "**END-PAR**"));
}

#[test]
fn the_plan_attaches_scripts_and_registers_the_dispatcher() {
    let plan = &lowered().plan;
    assert_eq!(plan.maps.len(), 1);
    let map = &plan.maps[0];
    assert!(map.map_script.is_some());
    // Mayor, guard, raider, door, terminal, note.
    assert_eq!(map.attachments.len(), 6, "{:?}", map.attachments);
    assert_eq!(plan.global_scripts.len(), 1);
    assert_eq!(plan.global_scripts[0].file, "mfquest.int");
    assert_eq!(plan.hooks[0].procedure, "mode_changed");
    assert!(plan.counts.dialogue.compiled > 0);
    assert!(plan.counts.quests.compiled > 0);
    assert!(plan.counts.state.compiled > 0);
}

#[test]
fn two_lowerings_and_reversed_input_give_identical_bytes() {
    let again = lower_layer(
        town(),
        &town_facts(false),
        &base_files(),
        &SimConfig::default(),
    )
    .unwrap();
    assert_eq!(again.lowered, lowered().lowered);
    let reversed_module = testkit::town_with(true);
    let reversed = lower_layer(
        &reversed_module,
        &town_facts(true),
        &base_files(),
        &SimConfig::default(),
    )
    .unwrap();
    assert_eq!(reversed.lowered.files, lowered().lowered.files);
}

#[test]
fn lower_without_facts_still_writes_loadable_programs() {
    let out = lower(town(), &BaseFiles::default(), &SimConfig::default()).unwrap();
    for (path, bytes) in out.files.iter().filter(|(p, _)| p.ends_with(".int")) {
        assert_program_ok(path, bytes);
    }
}

#[test]
fn the_engines_mod_layer_accepts_the_files_and_rows() {
    let out = lowered();
    let mut layer = emit::ModLayer::new("simtest", "Sim test", "1").unwrap();
    for (path, bytes) in &out.lowered.files {
        layer.insert(path, bytes.clone()).unwrap();
    }
    for row in &out.plan.global_scripts {
        layer
            .add_global_script(&row.file, row.repeat, row.kind)
            .unwrap();
    }
    for hook in &out.plan.hooks {
        layer
            .add_hook(&hook.point, &hook.file, &hook.procedure)
            .unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    layer.write(&dir.path().join("layer")).unwrap();
}
