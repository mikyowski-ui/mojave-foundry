//! The simulation stage (task T9) on synthetic output directories.
//!
//! The main test runs the linked two-storey mini mart with the Pass 2
//! records (`common::mini_mart::write_sim`: a clerk with a greeting that
//! sets a quest stage, a top-level topic, an enable group, a locked door
//! with a key, a placed terminal, a sandbox package on a marker the slicer
//! skips) through `all` with `sim = true`, then checks the layer: every
//! program loads and passes the stack model of the engine's own VM, the
//! tables parse back, the maps carry the script records, a second run is a
//! cache hit, a forced one is byte-identical, `--report-only` writes
//! nothing in the layer, and a run with `sim` off gives the map stage's
//! layer exactly.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::mini_mart::{self, key};
use mojave_convert::placements::{self, MarkerSource, Origin};
use mojave_convert::sim::{self, SimInputs, SimOptions};
use mojave_convert::{Inputs, Install, pipeline};
use vault_formats::map::MapFile;
use vault_formats::{ByteSource, GameProfile, LayeredVfs, LocalPrototypeCatalog, MemoryDir, Vfs};

fn tree_of(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    common::tree(dir).into_iter().collect()
}

fn writing(f2: &Path) -> SimOptions {
    SimOptions {
        f2: Some(f2.to_owned()),
        ..SimOptions::default()
    }
}

/// Every `.int` of a layer through `vault_vm`: loaded under the Fallout 2
/// word set and checked by the stack model. Returns the failures.
fn check_programs(layer: &BTreeMap<String, Vec<u8>>) -> (usize, Vec<String>) {
    use vault_vm::stack_check::{self, Table};
    use vault_vm::{IntProgram, WordSet};
    let mut count = 0;
    let mut failures = Vec::new();
    for (path, bytes) in layer {
        if !path.ends_with(".int") {
            continue;
        }
        count += 1;
        match IntProgram::parse_in(bytes, &WordSet::FALLOUT2, false) {
            Err(e) => failures.push(format!("{path}: refused: {e}")),
            Ok(program) => {
                let findings =
                    stack_check::check(path, &program, &Table::new(WordSet::FALLOUT2), &[]);
                if !findings.is_empty() {
                    failures.push(format!("{path}: {}", stack_check::summary(&findings)));
                }
            }
        }
    }
    (count, failures)
}

/// The layer's maps, parsed with the layer's prototypes.
fn maps_of(layer: &BTreeMap<String, Vec<u8>>, files: &[&str]) -> BTreeMap<String, MapFile> {
    let tree = |files: Vec<(String, Vec<u8>)>| {
        MemoryDir::new(
            files
                .into_iter()
                .map(|(path, bytes)| (path, ByteSource::new(bytes).expect("bytes"))),
        )
        .expect("a tree")
    };
    let files_in: Vec<(String, Vec<u8>)> =
        layer.iter().map(|(p, b)| (p.clone(), b.clone())).collect();
    let vfs = LayeredVfs::new(
        Arc::new(tree(Vec::new())),
        vec![vault_formats::ModLayer::from_tree("test", tree(files_in))],
    );
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    files
        .iter()
        .map(|file| {
            let bytes = vfs.read(file).expect("the map");
            let map = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).expect("parses");
            ((*file).to_owned(), map)
        })
        .collect()
}

struct Run {
    _fixture: common::Fixture,
    f2: PathBuf,
    on: Inputs,
}

fn run_pipeline(fixture: &common::Fixture, layer_file: &Path, out: &str) -> Inputs {
    let mart_data = fixture.root.path().join("sim").join("Data");
    let mut inputs = Inputs::from_args(
        &Install::any(&mart_data, None),
        Some(layer_file.to_str().expect("utf-8")),
        &fixture.f2,
        &fixture.root.path().join(out),
        2,
    )
    .expect("inputs");
    inputs.standalone = true;
    inputs
}

fn built() -> (Run, mojave_convert::pipeline::AllSummary) {
    let fixture = common::fixture();
    let mart = mini_mart::write_sim(&fixture.root.path().join("sim"));
    let on = run_pipeline(&fixture, &mart.layer_file, "out-sim");
    let summary = pipeline::all(&on).expect("the pipeline runs");
    (
        Run {
            f2: fixture.f2.clone(),
            _fixture: fixture,
            on,
        },
        summary,
    )
}

#[test]
fn all_with_sim_writes_a_layer_the_engine_loads_and_reruns_change_nothing() {
    let (run, summary) = built();
    let out = run.on.out.clone();
    let layer_dir = run.on.mod_dir();

    // The map stage's new inputs: markers, walk masks, exits.
    let index = placements::read(&out).expect("reads").expect("the index");
    assert_eq!(index.origin, Origin::Stage);
    let marker = index
        .markers
        .iter()
        .find(|m| m.reference == key(mini_mart::R_MARKER))
        .unwrap_or_else(|| panic!("the skipped marker has a hex: {:?}", index.markers));
    assert_eq!(marker.source, MarkerSource::Cell);
    assert_eq!(marker.map, "mmstore");
    let facts = placements::read_facts(&out)
        .expect("reads")
        .expect("map-sim.json");
    let store = facts
        .maps
        .iter()
        .find(|m| m.stem == "mmstore")
        .expect("the store");
    assert!(store.interior);
    assert!(store.walkable.iter().any(|w| w.count > 0), "{store:?}");
    let hexes = store.walkable[0].hexes().expect("runs parse");
    assert_eq!(hexes.len() as u32, store.walkable[0].count);
    assert!(
        facts.maps.iter().any(|m| !m.exits.is_empty()),
        "a door leads to the other map: {facts:?}"
    );
    assert!(
        !out.join("mods/mojave")
            .join(placements::FACTS_FILE)
            .exists()
    );

    // The stage ran inside `all` and wrote the layer.
    let outcome = summary.sim.expect("sim ran");
    assert!(!outcome.cached);
    let report = outcome.report.expect("a report");
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    // The layer file's `[sim_options]` reached the build: the map stage
    // left it for the stage, the stage built with it (the unknown faction
    // is reported), and a `--config` `[sim]` table is laid over it.
    let options = std::fs::read_to_string(out.join(mojave_convert::layer::SIM_OPTIONS_FILE))
        .expect("sim-options.toml");
    assert!(options.contains("MMShopFaction"), "{options}");
    let listed = ["MMShopFaction".to_owned(), "MMNoSuchFaction".to_owned()];
    let packages = report
        .build_sections
        .iter()
        .find(|s| s.section == Some(transpile::diag::Section::Packages))
        .expect("the packages section");
    assert!(
        packages
            .reasons
            .contains_key("unknown_team_faction MMNoSuchFaction"),
        "{:?}",
        packages.reasons
    );
    assert!(
        report
            .key_parts
            .iter()
            .any(|(name, value)| name == "sim options" && value.contains("MMShopFaction"))
    );
    let config = |given: transpile::ir::SimConfig| {
        SimInputs::load(
            &out,
            &SimOptions {
                config: given,
                ..SimOptions::default()
            },
        )
        .expect("loads")
        .config
    };
    let base = config(transpile::ir::SimConfig::default());
    assert_eq!(base.team_factions, listed);
    let deeper = config(transpile::ir::SimConfig {
        quest_depth: 3,
        ..Default::default()
    });
    assert_eq!(
        (deeper.quest_depth, deeper.team_factions),
        (3, listed.to_vec())
    );
    let replaced = config(transpile::ir::SimConfig {
        team_factions: vec!["Other".into()],
        ..Default::default()
    });
    assert_eq!(replaced.team_factions, ["Other"]);
    assert!(report.program_count > 0, "{report:?}");
    let layer = tree_of(&layer_dir);
    let (programs, failures) = check_programs(&layer);
    assert_eq!(programs, report.program_count);
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(layer.contains_key("scripts/mfquest.int"), "the dispatcher");
    assert!(
        layer
            .keys()
            .any(|p| p.starts_with("text/english/dialog/md")),
        "the clerk's dialogue file"
    );

    // The tables parse back through the engine's own parsers.
    use vault_formats::fallout2::lines;
    use vault_formats::fallout2::text::Diagnostics;
    use vault_formats::{GamFile, GamSection, LstKind, LstTable};
    let lst = LstTable::parse(&layer["scripts/scripts.lst"], LstKind::Script).expect("lst");
    assert!(lst.entries().len() >= report.program_count);
    GamFile::parse(&layer["data/vault13.gam"], GamSection::Campaign).expect("gam");
    let quests =
        lines::quests(&layer["data/quests.txt"], &mut Diagnostics::default()).expect("quests.txt");
    assert!(!quests.is_empty(), "MMQuest's Pip-Boy row");
    if let Some(holodisks) = layer.get("data/holodisk.txt") {
        lines::holodisks(holodisks, &mut Diagnostics::default()).expect("holodisk.txt");
    }
    for (path, bytes) in &layer {
        if path.ends_with(".msg") {
            vault_formats::msg::MsgCatalog::parse_strict(bytes)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
        }
    }
    let manifest = vault_formats::mod_manifest::ModManifest::parse(
        &String::from_utf8(layer["vault-mod.toml"].clone()).expect("utf-8"),
    )
    .expect("manifest");
    assert_eq!(manifest.global_scripts.len(), 1, "{manifest:?}");
    assert_eq!(manifest.hooks.len(), 1);

    // The clerk's object carries its program; the store has its map
    // script.
    let maps = maps_of(&layer, &["maps/mmstore.map", "maps/mmlot.map"]);
    let store_map = &maps["maps/mmstore.map"];
    let clerk = index
        .placed
        .iter()
        .find(|p| p.reference.as_deref() == Some(key(mini_mart::R_CLERK).as_str()))
        .expect("the clerk is placed");
    let object = store_map.objects.elevations[usize::from(clerk.elevation)]
        .iter()
        .find(|o| Some(o.id) == clerk.object_id)
        .expect("the clerk's object");
    assert!(object.script_id >= 0, "the clerk has a script record");
    let attached = report
        .maps
        .iter()
        .find(|m| m.map == "mmstore")
        .expect("attachments on the store");
    assert!(attached.objects > 0);
    assert!(attached.map_script.is_some());
    assert_eq!(
        store_map.header.script_reference,
        attached.map_script.expect("a map script") as i32 + 1,
        "the header stores the map script's row plus one"
    );

    // The report: sections, no path.
    let text = std::fs::read_to_string(out.join(sim::REPORT_FILE)).expect("report");
    for needle in [
        "# Simulation layer report",
        "## Sections (the lowering)",
        "| dialogue |",
        "| packages |",
        "## Tables written",
        "## Attachments per map",
        "## Engine gaps hit",
        "## Cache key",
        &outcome.cache_key,
    ] {
        assert!(text.contains(needle), "no {needle:?} in\n{text}");
    }
    let root = run._fixture.root.path().to_string_lossy().into_owned();
    assert!(!text.contains(&root), "a path in the report");

    // A second run: a cache hit that changes nothing.
    let started = std::time::Instant::now();
    let again = sim::sim(&out, &writing(&run.f2)).expect("again");
    assert!(again.cached, "a cache hit");
    assert!(started.elapsed().as_secs_f64() < 5.0);
    assert_eq!(again.cache_key, outcome.cache_key);
    assert_eq!(tree_of(&layer_dir), layer);

    // `all` again: the map stage reuses its output, sim is a cache hit.
    let summary = pipeline::all(&run.on).expect("all again");
    assert!(summary.map.reused);
    assert!(summary.sim.expect("sim").cached);
    assert_eq!(tree_of(&layer_dir), layer);

    // A forced run: byte-identical layer and report.
    let report_before = std::fs::read(out.join(sim::REPORT_FILE)).expect("report");
    let forced = sim::sim(
        &out,
        &SimOptions {
            force: true,
            ..writing(&run.f2)
        },
    )
    .expect("forced");
    assert!(!forced.cached);
    assert_eq!(forced.cache_key, outcome.cache_key);
    assert_eq!(tree_of(&layer_dir), layer, "attaching does not stack");
    assert_eq!(
        std::fs::read(out.join(sim::REPORT_FILE)).expect("report"),
        report_before
    );

    // `--report-only`: no install, nothing in the layer or the cache.
    let cache_before = tree_of(&out.join("cache"));
    std::fs::remove_file(out.join(sim::REPORT_FILE)).expect("remove the report");
    let only = sim::sim(
        &out,
        &SimOptions {
            report_only: true,
            ..SimOptions::default()
        },
    )
    .expect("report only");
    assert!(!only.cached);
    let only_report = only.report.expect("a report");
    assert!(only_report.errors.is_empty(), "{:?}", only_report.errors);
    assert!(only_report.program_count > 0);
    assert!(out.join(sim::REPORT_FILE).is_file());
    assert_eq!(tree_of(&layer_dir), layer);
    assert_eq!(tree_of(&out.join("cache")), cache_before);

    // Writing without an install is refused before anything is touched.
    let error = sim::sim(
        &out,
        &SimOptions {
            force: true,
            ..SimOptions::default()
        },
    )
    .expect_err("no --f2");
    assert!(error.to_string().starts_with("--f2"), "{error}");

    // With `sim` off, the layer is the map stage's alone: no scripts, and
    // every file the stage replaced is the map stage's own copy.
    let off_file = run.on.slice_file.clone().expect("the layer file");
    let off_text = std::fs::read_to_string(&off_file)
        .expect("layer file")
        .replace("sim = true\n", "");
    let off_path = off_file.with_file_name("nosim.layer.toml");
    std::fs::write(&off_path, off_text).expect("write");
    let off = run_pipeline(&run._fixture, &off_path, "out-nosim");
    let off_summary = pipeline::all(&off).expect("the pipeline runs");
    assert!(off_summary.sim.is_none());
    let off_layer = tree_of(&off.mod_dir());
    assert!(!off_layer.keys().any(|p| p.starts_with("scripts/")));
    let mut expected = layer.clone();
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out.join(sim::STATE_DIR).join("state.json")).expect("state"),
    )
    .expect("json");
    for path in state["written"].as_object().expect("written").keys() {
        expected.remove(path);
    }
    // `README.txt` is written from the manifest, which lists the scripts.
    expected.insert("README.txt".into(), off_layer["README.txt"].clone());
    assert!(
        !String::from_utf8_lossy(&off_layer["README.txt"]).contains("mfquest"),
        "no script in the sim-off README"
    );
    for (path, bytes) in tree_of(&out.join(sim::STATE_DIR).join("pristine")) {
        expected.insert(path, bytes);
    }
    assert_eq!(
        off_layer.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    let differ: Vec<&String> = off_layer
        .iter()
        .filter(|(p, b)| expected.get(*p) != Some(*b))
        .map(|(p, _)| p)
        .collect();
    assert!(
        differ.is_empty(),
        "the sim-off layer is the map stage's: {differ:?}"
    );
}

/// The fallback path: no placement index, no install, report only.
#[test]
fn report_only_runs_on_a_reconstructed_index_and_bad_indexes_are_refused() {
    let fixture = common::fixture();
    let mart = mini_mart::write(&fixture.root.path().join("mart"));
    let mut inputs = Inputs::for_install(
        &Install::any(&mart.data_dir, None),
        &fixture.f2,
        &fixture.root.path().join("out-mart"),
        mart.slice.clone(),
        2,
    )
    .expect("inputs");
    inputs.standalone = true;
    let summary = pipeline::all(&inputs).expect("the pipeline runs");
    assert!(summary.sim.is_none(), "sim is off by default");
    let out = inputs.out.clone();
    let layer_before = common::tree(&inputs.mod_dir());
    std::fs::remove_dir_all(fixture.root.path().join("mart")).expect("remove the source");
    std::fs::remove_dir_all(&fixture.f2).expect("remove the Fallout 2 install");

    let good = SimInputs::load(&out, &SimOptions::default())
        .expect("loads")
        .index;
    std::fs::remove_file(out.join(placements::FILE_NAME)).expect("remove the index");
    let options = SimOptions {
        report_only: true,
        ..SimOptions::default()
    };
    let loaded = SimInputs::load(&out, &options).expect("loads without the install");
    assert!(!loaded.from_file);
    assert!(
        loaded
            .warnings
            .iter()
            .any(|w| w.starts_with(placements::FILE_NAME)),
        "{:?}",
        loaded.warnings
    );
    let outcome = sim::sim(&out, &options).expect("report only");
    let report = outcome.report.expect("report");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(outcome.cache_key.starts_with("sim-1:"));
    assert_eq!(outcome.cache_key.len(), "sim-1:".len() + 64);
    assert_eq!(common::tree(&inputs.mod_dir()), layer_before);
    let first = std::fs::read(out.join(sim::REPORT_FILE)).expect("report");
    sim::sim(&out, &options).expect("again");
    assert_eq!(
        std::fs::read(out.join(sim::REPORT_FILE)).expect("report"),
        first
    );

    // The key follows the map stage's key.
    let key = SimInputs::load(&out, &options).expect("loads").cache_key();
    std::fs::write(out.join("cache").join("map.key"), "another map run\n").expect("key");
    assert_ne!(
        SimInputs::load(&out, &options).expect("loads").cache_key(),
        key
    );

    let mut stale = good.clone();
    stale.maps[0].stem = "elsewhere".to_owned();
    placements::write(&out, &stale).expect("writes");
    let error = SimInputs::load(&out, &options).expect_err("a map the layer does not have");
    assert!(
        error.to_string().contains("not in the layer's manifest"),
        "{error}"
    );
    let mut other_game = good;
    other_game.game = "fnv".to_owned();
    placements::write(&out, &other_game).expect("writes");
    let error = SimInputs::load(&out, &options).expect_err("another game");
    assert!(error.to_string().contains("mixes two runs"), "{error}");

    std::fs::remove_file(out.join(placements::FILE_NAME)).expect("remove");
    std::fs::remove_dir_all(out.join("mir")).expect("remove the MIR");
    let error = SimInputs::load(&out, &options).expect_err("no MIR");
    assert!(error.to_string().starts_with("mir:"), "{error}");
}

/// A build error is a failed run: the report is still written, the error
/// lines go to stderr, the exit status is non-zero. `--report-only`
/// follows the same rule; a run with no error succeeds.
#[test]
fn a_lowering_error_fails_the_command_and_still_writes_the_report() {
    use std::process::Command;
    let fixture = common::fixture();
    let mart = mini_mart::write_sim(&fixture.root.path().join("sim"));
    let on = run_pipeline(&fixture, &mart.layer_file, "out-fail");
    pipeline::all(&on).expect("the pipeline runs");
    let out = on.out.clone();
    let exe = env!("CARGO_BIN_EXE_mojave-convert");
    let run = |extra: &[&str]| {
        Command::new(exe)
            .arg("sim")
            .arg("--out")
            .arg(&out)
            .args(extra)
            .output()
            .expect("runs")
    };
    let f2 = fixture.f2.to_str().expect("utf-8");

    // The good run, forced: success.
    let good = run(&["--f2", f2, "--force"]);
    assert!(good.status.success(), "{good:?}");

    // A base file the lowering cannot take: the install's vault13.gam has
    // a line that is not a variable.
    let data = fixture.f2.join("data").join("data");
    std::fs::create_dir_all(&data).expect("dir");
    std::fs::write(data.join("vault13.gam"), "GVAR_BROKEN := not a number;\n").expect("write");
    std::fs::remove_file(out.join(sim::REPORT_FILE)).expect("remove the report");
    for extra in [
        &["--f2", f2, "--force"][..],
        &["--report-only", "--f2", f2][..],
    ] {
        let bad = run(extra);
        let stderr = String::from_utf8_lossy(&bad.stderr);
        assert!(!bad.status.success(), "{extra:?}: {bad:?}");
        assert!(stderr.contains("build error: "), "{extra:?}: {stderr}");
        let report = std::fs::read_to_string(out.join(sim::REPORT_FILE)).expect("the report");
        assert!(report.contains("## Build errors"), "{report}");
        std::fs::remove_file(out.join(sim::REPORT_FILE)).expect("remove the report");
    }
}
