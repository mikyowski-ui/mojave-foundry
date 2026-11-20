//! The M3 exit check: `mojave-convert all` on the synthetic mini town
//! (terrain, navmesh, a multi-hex building, a door into an interior, props,
//! a synthetic palette) produces a mod layer whose map, prototypes, art,
//! lists and manifest all parse back through `vault-formats`.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use mojave_convert::{map, pipeline};
use vault_formats::frm::FrmArchive;
use vault_formats::lst::{LstKind, LstTable};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::mod_manifest::ModManifest;
use vault_formats::pro::Prototype;
use vault_formats::pro_catalog::LocalPrototypeCatalog;
use vault_formats::vfs::{ByteSource, MemoryDir, MemoryMount};
use vault_formats::{GameProfile, MemoryProvider, VfsPath};

/// The layer mounted the way the engine mounts loose files.
fn mount(files: &[(String, Vec<u8>)]) -> MemoryProvider {
    let tree = MemoryDir::new(
        files
            .iter()
            .filter(|(path, _)| path != "vault-mod.toml" && path != "README.txt")
            .map(|(path, bytes)| (path.clone(), ByteSource::new(bytes.clone()).expect("bytes"))),
    )
    .expect("a valid tree");
    MemoryMount {
        profile: GameProfile::Fallout2,
        loose: Some(tree),
        ..MemoryMount::default()
    }
    .mount()
    .expect("mounts")
}

/// Parses every file of a written layer with `vault-formats` directly,
/// independently of the builder's own self-check. Returns counts by kind.
fn parse_layer(dir: &Path) -> BTreeMap<&'static str, usize> {
    let files = common::tree(dir);
    assert!(!files.is_empty(), "the layer is empty");
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();

    let manifest_text = files
        .iter()
        .find(|(path, _)| path == "vault-mod.toml")
        .map(|(_, bytes)| String::from_utf8(bytes.clone()).expect("utf-8 manifest"))
        .expect("vault-mod.toml");
    let manifest = ModManifest::parse(&manifest_text).expect("the manifest parses");
    assert_eq!(manifest.id.as_deref(), Some(map::LAYER_ID));
    assert!(!manifest.maps.is_empty(), "the manifest registers the maps");

    // The layer as the engine would mount it, for prototype lookups.
    let provider = mount(&files);
    let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);

    for (path, bytes) in &files {
        let extension = path.rsplit('.').next().unwrap_or("");
        match extension {
            "frm" => {
                let frm = FrmArchive::parse(bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
                assert!(frm.frame(0, 0).is_some(), "{path} has no frame");
                *counts.entry("frm").or_default() += 1;
            }
            "lst" => {
                let vfs_path = VfsPath::new(path).unwrap_or_else(|e| panic!("{path}: {e}"));
                let table = LstTable::parse(bytes, LstKind::for_path(&vfs_path))
                    .unwrap_or_else(|e| panic!("{path}: {e}"));
                assert!(!table.entries().is_empty(), "{path} is empty");
                if path.starts_with("art/") {
                    assert!(table.entries().len() <= 4096, "{path} is over the FID cap");
                }
                *counts.entry("lst").or_default() += 1;
            }
            "pro" => {
                Prototype::parse_for(bytes, GameProfile::Fallout2)
                    .unwrap_or_else(|e| panic!("{path}: {e}"));
                *counts.entry("pro").or_default() += 1;
            }
            "msg" => {
                vault_formats::MsgCatalog::parse_strict(bytes)
                    .unwrap_or_else(|e| panic!("{path}: {e}"));
                *counts.entry("msg").or_default() += 1;
            }
            "map" => {
                let parsed = MapFile::parse_for(bytes, &catalog, GameProfile::Fallout2)
                    .unwrap_or_else(|e| panic!("{path}: {e}"));
                assert_eq!(parsed.header.version, 20, "{path} is not MAP v20");
                assert!(
                    parsed.diagnostics.is_empty(),
                    "{path}: {:?}",
                    parsed.diagnostics
                );
                assert!(parsed.tiles[0].is_some(), "{path} has no floor");
                // Writing the parsed map back gives the same bytes.
                assert_eq!(&parsed.write().expect("writes"), bytes, "{path} round trip");
                *counts.entry("map").or_default() += 1;
            }
            _ => {}
        }
    }
    counts
}

/// The exterior map of a layer, parsed.
fn exterior_map(dir: &Path, file: &str) -> MapFile {
    let files = common::tree(dir);
    let provider = mount(&files);
    let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);
    let bytes = &files
        .iter()
        .find(|(path, _)| path == file)
        .unwrap_or_else(|| panic!("{file} is in the layer"))
        .1;
    MapFile::parse_for(bytes, &catalog, GameProfile::Fallout2).expect("parses")
}

#[test]
fn all_builds_a_mod_layer_that_vault_formats_parses() {
    let fixture = common::fixture();
    let inputs = fixture.inputs("out", 2);
    let summary = pipeline::all(&inputs).expect("all stages");
    assert!(summary.extract.exterior_cells >= 9);
    assert!(summary.sprites.rendered >= 4);
    assert!(!summary.map.reused);
    assert_eq!(summary.gallery.pages, 1);

    let dir = inputs.mod_dir();
    assert!(dir.ends_with("mods/mojave"));
    let counts = parse_layer(&dir);
    for kind in ["frm", "lst", "pro", "msg", "map"] {
        assert!(
            counts.get(kind).copied().unwrap_or(0) > 0,
            "no {kind} file: {counts:?}"
        );
    }
    // The exterior and the interior stub behind the town's door.
    assert!(counts["map"] >= 2, "{counts:?}");
    assert!(summary.map.exits >= 1, "{:?}", summary.map);

    // The map has what the town has: floor everywhere, the multi-hex
    // building as several wall strips, the door, and an exit to the stub.
    let exterior = exterior_map(&dir, &summary.map.map_file);
    assert_eq!(exterior.header.map_number as u32, summary.map.map_number);
    let floor = exterior.tiles[0].as_ref().expect("floor");
    assert_eq!(floor.len(), 10_000);
    let objects = &exterior.objects.elevations[0];
    assert_eq!(
        exterior.objects.total,
        objects.len(),
        "everything is on elevation 0"
    );
    let walls = objects
        .iter()
        .filter(|o| (o.art_id >> 24) & 0xf == 3)
        .count();
    assert!(walls >= 4, "the building is sliced into strips: {walls}");
    let exits: Vec<_> = objects
        .iter()
        .filter_map(|o| match &o.state {
            ObjectState::Other {
                detail: ObjectDetail::ExitGrid { map, hex, .. },
                ..
            } => Some((*map, *hex)),
            _ => None,
        })
        .collect();
    assert!(!exits.is_empty(), "the door leads somewhere");
    for (map_number, hex) in &exits {
        assert_ne!(*map_number as u32, summary.map.map_number);
        assert!((0..40_000).contains(hex));
    }
    assert!(summary.map.walkable_hexes > 100, "{:?}", summary.map);
    assert!(
        summary.map.blockers > 0,
        "the ridge and the building are sealed off"
    );

    // Reports and previews sit beside the layer, never in it.
    assert!(inputs.out.join("map-report.md").is_file());
    assert!(inputs.mir_dir().join("report.md").is_file());
    let preview = std::fs::read(inputs.gallery_dir().join(format!(
        "map-{}.png",
        Path::new(&summary.map.map_file)
            .file_stem()
            .expect("stem")
            .to_string_lossy()
    )))
    .expect("map preview");
    foundry_gallery::decode_png(&preview).expect("a valid PNG");
}

#[test]
fn a_second_run_reuses_everything_and_changes_no_byte() {
    let fixture = common::fixture();
    let inputs = fixture.inputs("out", 2);
    pipeline::all(&inputs).expect("first run");
    let before = common::tree(&inputs.out);
    let again = pipeline::all(&inputs).expect("second run");
    assert_eq!(again.sprites.rendered, 0);
    assert!(again.map.reused);
    assert_eq!(again.extract.meshes.converted, 0);
    assert_eq!(common::tree(&inputs.out), before);
}

#[test]
fn output_does_not_depend_on_the_job_count() {
    let fixture = common::fixture();
    let one = fixture.inputs("out-1", 1);
    let four = fixture.inputs("out-4", 4);
    pipeline::all(&one).expect("one job");
    pipeline::all(&four).expect("four jobs");
    assert_eq!(common::tree(&one.mod_dir()), common::tree(&four.mod_dir()));
    assert_eq!(
        common::tree(&one.sprites_dir()),
        common::tree(&four.sprites_dir())
    );
}

#[test]
fn a_foreign_directory_is_never_overwritten() {
    let fixture = common::fixture();
    let inputs = fixture.inputs("out", 2);
    let dir = inputs.mod_dir();
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("keep.txt"), b"someone else's file").expect("file");
    let error = pipeline::all(&inputs).expect_err("the layer directory is not ours");
    assert!(error.to_string().starts_with("map:"), "{error}");
    assert_eq!(
        std::fs::read(dir.join("keep.txt")).expect("still there"),
        b"someone else's file"
    );
}

#[test]
fn the_binary_runs_all_from_a_slice_file() {
    let fixture = common::fixture();
    let slice_path = fixture.root.path().join("slice.toml");
    std::fs::write(&slice_path, fixture.slice.to_toml()).expect("slice file");
    let out = fixture.root.path().join("out-bin");
    let run = |command: &str| {
        Command::new(env!("CARGO_BIN_EXE_mojave-convert"))
            .arg(command)
            .arg("--fnv")
            .arg(&fixture.fnv)
            .arg("--f2")
            .arg(&fixture.f2)
            .arg("--out")
            .arg(&out)
            .arg("--slice")
            .arg(&slice_path)
            .arg("--jobs")
            .arg("2")
            .output()
            .expect("the binary runs")
    };
    let output = run("all");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let counts = parse_layer(&out.join("mods").join("mojave"));
    assert!(counts["map"] >= 2 && counts["frm"] > 0 && counts["pro"] > 0);

    // The library and the binary agree byte for byte.
    let inputs = fixture.inputs("out-lib", 2);
    pipeline::all(&inputs).expect("library run");
    assert_eq!(
        common::tree(&out.join("mods").join("mojave")),
        common::tree(&inputs.mod_dir())
    );

    // Each stage is also a subcommand of its own.
    for command in ["extract", "sprites", "map", "gallery"] {
        let output = run(command);
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Both installs are required on the command line.
    let missing = Command::new(env!("CARGO_BIN_EXE_mojave-convert"))
        .args(["all", "--fnv"])
        .arg(&fixture.fnv)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("the binary runs");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--f2"));
}
