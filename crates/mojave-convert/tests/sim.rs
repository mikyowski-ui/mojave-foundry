//! The `sim` scaffold on a synthetic output directory: the mini Super-Duper
//! Mart run through the whole pipeline (`common::mini_mart`, read only),
//! then the source install and the Fallout 2 install deleted, then the
//! stage run on what is left.

mod common;

use std::path::PathBuf;

use common::mini_mart::{self, key};
use mojave_convert::placements::{self, Origin, PlacedKind};
use mojave_convert::sim::{self, SimInputs};
use mojave_convert::{Inputs, Install, pipeline};

struct Built {
    _fixture: common::Fixture,
    inputs: Inputs,
}

impl Built {
    fn out(&self) -> PathBuf {
        self.inputs.out.clone()
    }
}

/// The mini mart converted to an output directory, with both installs
/// removed afterwards: nothing but `--out` is left to read.
fn built() -> Built {
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
    pipeline::all(&inputs).expect("the pipeline runs");
    std::fs::remove_dir_all(fixture.root.path().join("mart")).expect("remove the source install");
    std::fs::remove_dir_all(&fixture.f2).expect("remove the Fallout 2 install");
    Built {
        _fixture: fixture,
        inputs,
    }
}

fn count(index: &placements::PlacementIndex, kind: PlacedKind) -> usize {
    index.placed.iter().filter(|p| p.kind == kind).count()
}

#[test]
fn the_scaffold_runs_on_the_output_alone_and_changes_nothing() {
    let built = built();
    let out = built.out();
    let layer_before = common::tree(&built.inputs.mod_dir());
    assert!(!layer_before.is_empty());
    // The map stage wrote the index; without it the stage reconstructs one.
    assert!(out.join(placements::FILE_NAME).is_file());
    std::fs::remove_file(out.join(placements::FILE_NAME)).expect("remove the index");
    assert!(!out.join(sim::REPORT_FILE).exists());

    let inputs = SimInputs::load(&out).expect("loads without the install");
    assert!(!inputs.from_file);
    assert_eq!(inputs.index.origin, Origin::Reconstructed);
    assert_eq!(inputs.game, mojave_convert::GameVariant::Fallout3);
    assert!(
        inputs
            .warnings
            .iter()
            .any(|w| w.starts_with(placements::FILE_NAME) && w.contains("reconstructed")),
        "{:?}",
        inputs.warnings
    );

    // The reconstruction sees what the map test sees: four people, the two
    // filled containers, the rifle and the rounds on the floor, doors.
    let index = &inputs.index;
    assert_eq!(count(index, PlacedKind::Critter), 4, "{index:?}");
    assert_eq!(count(index, PlacedKind::Container), 2);
    assert!(count(index, PlacedKind::Item) >= 2);
    assert!(count(index, PlacedKind::Door) >= 1);
    assert!(index.placed.iter().all(|p| p.reference.is_none()));
    assert!(
        index
            .placed
            .iter()
            .all(|p| p.object_id.is_some() && p.pid.is_some())
    );
    assert!(index.unplaced.is_empty());
    let critters: Vec<_> = index.placed.iter().filter(|p| p.critter).collect();
    assert!(
        critters
            .iter()
            .all(|c| c.team.is_some() && c.ai_packet.is_some())
    );
    let ids: Vec<(u8, Option<u32>)> = index
        .placed
        .iter()
        .map(|p| (p.elevation, p.object_id))
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "sorted by elevation and object id");

    let report = sim::run(&inputs);
    assert_eq!(report.critters.placed, 4);
    assert_eq!(
        report.critters.unresolved, 4,
        "no references without the file"
    );
    assert_eq!(report.logic.scripts, 0);
    assert!(report.cache_key.starts_with("sim-1:"));
    assert_eq!(report.cache_key.len(), "sim-1:".len() + 64);

    sim::write_report(&out, &report).expect("writes the report");
    let first = std::fs::read(out.join(sim::REPORT_FILE)).expect("report");
    let text = String::from_utf8(first.clone()).expect("utf-8");
    for needle in [
        "# Simulation layer report (scaffold)",
        "No scripts, dialogue or packages are compiled yet",
        "- Programs compiled: 0 (Pass 2)",
        "- Game: Fallout 3",
        "- Placement index: reconstructed from the built maps",
        "## Warnings",
        "## Source logic for the placed set",
        "## Whole extract (for scale)",
        "| critters | 4 |",
        &report.cache_key,
    ] {
        assert!(text.contains(needle), "no {needle:?} in\n{text}");
    }
    let root = built._fixture.root.path().to_string_lossy().into_owned();
    assert!(!text.contains(&root), "a path in the report");

    // A second run: same key, same bytes.
    let again = SimInputs::load(&out).expect("loads again");
    let second_report = sim::run(&again);
    assert_eq!(second_report.cache_key, report.cache_key);
    sim::write_report(&out, &second_report).expect("writes again");
    assert_eq!(
        std::fs::read(out.join(sim::REPORT_FILE)).expect("report"),
        first
    );

    // The layer is as it was.
    assert_eq!(common::tree(&built.inputs.mod_dir()), layer_before);

    // The key follows its inputs: the map stage's key.
    std::fs::write(out.join("cache").join("map.key"), "another map run\n").expect("key");
    let changed = SimInputs::load(&out).expect("loads");
    assert_ne!(changed.cache_key(), report.cache_key);
}

#[test]
fn a_placement_file_wins_and_resolves_references_through_the_mir() {
    let built = built();
    let out = built.out();
    std::fs::remove_file(out.join(placements::FILE_NAME)).expect("the stage's index");
    let fallback = SimInputs::load(&out).expect("fallback");
    let fallback_key = fallback.cache_key();

    // A stage-style file: the four people get the fixture's references.
    let mut index = fallback.index.clone();
    index.origin = Origin::Stage;
    let people = [
        mini_mart::R_GUNNER,
        mini_mart::R_RAIDERS[0],
        mini_mart::R_RAIDERS[1],
        mini_mart::R_CLERK,
    ];
    let mut next = people.iter();
    for entry in index.placed.iter_mut().filter(|p| p.critter) {
        entry.reference = Some(key(*next.next().expect("four people")));
    }
    placements::write(&out, &index).expect("writes the index");

    let inputs = SimInputs::load(&out).expect("loads with the file");
    assert!(inputs.from_file);
    assert!(inputs.warnings.is_empty(), "{:?}", inputs.warnings);
    assert_eq!(inputs.index.placed.len(), index.placed.len());
    // The bases came from the MIR's placed references.
    let report = sim::run(&inputs);
    assert_eq!(report.critters.placed, 4);
    assert_eq!(report.critters.unresolved, 0, "{:?}", inputs.index.placed);
    assert!(
        inputs.index.placed.iter().filter(|p| p.critter).all(|p| {
            p.base.is_some() && matches!(p.base_kind.as_deref(), Some("NPC_" | "CREA"))
        })
    );
    assert_eq!(report.objects.unresolved, report.objects.placed);
    assert_ne!(report.cache_key, fallback_key);
    let text = report.to_markdown();
    assert!(
        text.contains("- Placement index: `map-placements.json`"),
        "{text}"
    );
    assert!(!text.contains("## Warnings"));
}

#[test]
fn an_index_from_another_run_is_refused() {
    let built = built();
    let out = built.out();
    let good = SimInputs::load(&out).expect("fallback").index;

    let mut stale = good.clone();
    stale.maps[0].stem = "elsewhere".to_owned();
    placements::write(&out, &stale).expect("writes");
    let error = SimInputs::load(&out).expect_err("a map the layer does not have");
    assert!(
        error.to_string().contains("not in the layer's manifest"),
        "{error}"
    );

    let mut other_game = good;
    other_game.game = "fnv".to_owned();
    placements::write(&out, &other_game).expect("writes");
    let error = SimInputs::load(&out).expect_err("another game");
    assert!(error.to_string().contains("mixes two runs"), "{error}");

    std::fs::remove_file(out.join(placements::FILE_NAME)).expect("remove");
    std::fs::remove_dir_all(out.join("mir")).expect("remove the MIR");
    let error = SimInputs::load(&out).expect_err("no MIR");
    assert!(error.to_string().starts_with("mir:"), "{error}");
}

/// The linked two-storey mini mart through the whole pipeline: the map
/// stage writes `map-placements.json` (origin `stage`) beside its report,
/// every placed object id is the written map's object at that elevation and
/// hex with that PID, and `sim` reads the file and resolves every critter.
#[test]
fn the_layer_stage_writes_the_index_and_sim_reads_it() {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use vault_formats::map::MapFile;
    use vault_formats::{
        ByteSource, GameProfile, LayeredVfs, LocalPrototypeCatalog, MemoryDir, Vfs,
    };

    let fixture = common::fixture();
    let mart = mini_mart::write_linked(&fixture.root.path().join("linked"));
    let mut inputs = Inputs::from_args(
        &Install::any(&mart.data_dir, None),
        Some(mart.layer_file.to_str().expect("utf-8")),
        &fixture.f2,
        &fixture.root.path().join("out-linked"),
        2,
    )
    .expect("inputs");
    inputs.standalone = true;
    pipeline::all(&inputs).expect("the pipeline runs");
    let out = inputs.out.clone();
    let text = std::fs::read_to_string(out.join(placements::FILE_NAME)).expect("the index");
    assert!(
        !inputs.mod_dir().join(placements::FILE_NAME).exists(),
        "not in the layer"
    );
    let index = placements::PlacementIndex::from_text(&text).expect("parses");
    assert_eq!(index.origin, Origin::Stage);
    assert_eq!(index.game, mojave_convert::GameVariant::Fallout3.id());
    let stems: Vec<&str> = index.maps.iter().map(|m| m.stem.as_str()).collect();
    assert!(
        stems.contains(&"mmlot") && stems.contains(&"mmstore"),
        "{stems:?}"
    );

    // Every object id is the written map's, at the same elevation and hex,
    // with the same PID.
    let layer = map_builder::read_layer(&inputs.mod_dir()).expect("reads back");
    let tree = |files: Vec<(String, Vec<u8>)>| {
        MemoryDir::new(
            files
                .into_iter()
                .map(|(path, bytes)| (path, ByteSource::new(bytes).expect("bytes"))),
        )
        .expect("a tree")
    };
    let mut files: Vec<(String, Vec<u8>)> = layer
        .files()
        .iter()
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    files.push((
        "vault-mod.toml".into(),
        layer.manifest_text().expect("manifest").into_bytes(),
    ));
    let vfs = LayeredVfs::new(
        Arc::new(tree(Vec::new())),
        vec![vault_formats::ModLayer::from_tree("test", tree(files))],
    );
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let mut objects: BTreeMap<(String, u8, u32), (i32, u32)> = BTreeMap::new();
    for entry in &index.maps {
        let bytes = vfs.read(&entry.file).expect("the map");
        let map = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).expect("parses");
        for (e, list) in map.objects.elevations.iter().enumerate() {
            for o in list {
                if let Some(hex) = o.hex {
                    objects.insert((entry.stem.clone(), e as u8, o.id), (hex, o.pid));
                }
            }
        }
    }
    assert!(!index.placed.is_empty());
    for p in &index.placed {
        let id = p.object_id.unwrap_or_else(|| panic!("{p:?} has an object"));
        let (hex, pid) = objects
            .get(&(p.map.clone(), p.elevation, id))
            .unwrap_or_else(|| panic!("{p:?} is not on its map"));
        assert_eq!((*hex, Some(*pid)), (p.hex, p.pid), "{p:?}");
        assert!(p.reference.is_some(), "{p:?}");
        assert_eq!(p.critter, *pid >> 24 == 1, "{p:?}");
    }
    let people = [
        mini_mart::R_GUNNER,
        mini_mart::R_RAIDERS[0],
        mini_mart::R_RAIDERS[1],
        mini_mart::R_CLERK,
    ];
    for person in people {
        let reference = key(person);
        let placed = index
            .placed
            .iter()
            .find(|p| p.reference.as_deref() == Some(reference.as_str()))
            .unwrap_or_else(|| panic!("{reference} placed"));
        assert_eq!(placed.kind, PlacedKind::Critter);
        assert!(placed.team.is_some() && placed.ai_packet.is_some());
    }
    assert!(count(&index, PlacedKind::Container) >= 1, "{index:?}");
    assert!(count(&index, PlacedKind::Door) >= 1, "{index:?}");
    // The raider upstairs is on the store's elevation 1.
    assert!(
        index
            .placed
            .iter()
            .any(|p| p.critter && p.map == "mmstore" && p.elevation == 1),
        "{index:?}"
    );

    // `sim` reads it: from the file, every critter's base known.
    let sim_inputs = SimInputs::load(&out).expect("loads");
    assert!(sim_inputs.from_file);
    assert_eq!(sim_inputs.index.origin, Origin::Stage);
    let report = sim::run(&sim_inputs);
    assert!(report.critters.placed >= 5, "{:?}", report.critters);
    assert_eq!(report.critters.unresolved, 0);
    assert!(
        !sim_inputs
            .warnings
            .iter()
            .any(|w| w.contains("not in the index")),
        "{:?}",
        sim_inputs.warnings
    );

    // A rerun reuses the stage and keeps the file.
    assert!(mojave_convert::map::map(&inputs).expect("again").reused);
    assert_eq!(
        std::fs::read_to_string(out.join(placements::FILE_NAME)).expect("the index"),
        text
    );
}
