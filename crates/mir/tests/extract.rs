//! End-to-end tests of the extractor and the reader on the synthetic
//! install of `mir::fixture::mini_town`.

use std::collections::BTreeMap;
use std::f32::consts::FRAC_PI_4;
use std::fs;
use std::path::{Path, PathBuf};

use mir::fixture::{CORRUPT_TEXTURE, INTERIOR, MISSING_MESH, MiniTown, WORLDSPACE, mini_town};
use mir::{
    AssetStatus, CellDoc, CellLocation, ExtractSummary, FormKey, Mir, MirError, RefGroup, RefKind,
    extract,
};

struct Run {
    _install: tempfile::TempDir,
    _out: tempfile::TempDir,
    town: MiniTown,
    out: PathBuf,
    summary: ExtractSummary,
}

fn run(jobs: usize) -> Run {
    let install = tempfile::tempdir().unwrap();
    let out_dir = tempfile::tempdir().unwrap();
    let town = mini_town(install.path());
    let out = out_dir.path().join("mir");
    let summary = extract(&town.options(&out, jobs)).unwrap();
    Run {
        _install: install,
        _out: out_dir,
        town,
        out,
        summary,
    }
}

/// Every file under `dir`, by relative path.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn assert_same(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) {
    assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>());
    for (path, bytes) in a {
        assert!(bytes == &b[path], "{path} differs");
    }
}

fn cell(mir: &Mir, x: i32, y: i32) -> CellDoc {
    mir.cell(WORLDSPACE, x, y).unwrap().unwrap()
}

#[test]
fn a_full_extract_writes_every_artifact() {
    let r = run(2);
    let files = snapshot(&r.out);
    for name in [
        "records.sqlite",
        "manifest.json",
        "report.md",
        "interiors/mthouseinterior.json",
    ] {
        assert!(files.contains_key(name), "{name} missing");
    }
    for x in -1..=1 {
        for y in -1..=1 {
            let name = format!("cells/minitownworld/{x}_{y}.json");
            assert!(files.contains_key(&name), "{name} missing");
        }
    }
    let mir = Mir::open(&r.out).unwrap();
    let manifest = mir.manifest();
    assert_eq!(manifest.cells.len(), 9);
    assert_eq!(manifest.interiors.len(), 1);
    assert_eq!(manifest.interiors[0].id, "mthouseinterior");
    assert_eq!(manifest.interiors[0].depth, 1);
    assert_eq!(manifest.plugins[0].name, "minitown.esm");
    assert_eq!(
        manifest
            .archives
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["minitown - meshes.bsa", "minitown - textures.bsa"]
    );
    let glbs = files
        .keys()
        .filter(|k| k.starts_with("meshes/") && k.ends_with(".glb"))
        .count();
    let pngs = files
        .keys()
        .filter(|k| k.starts_with("textures/") && k.ends_with(".png"))
        .count();
    assert_eq!(glbs, manifest.mesh_info.len());
    assert_eq!(pngs, manifest.texture_info.len());
    // house, fence, rock, door, crate, tin can = bottle, sign; then the
    // actors: the human skeleton, two heads, the female upper body, the
    // female hands and the male right hand, hair, duster, hat, glove,
    // goggles, the dog's skeleton and body.
    assert_eq!(glbs, 7 + 14);
    // ... and the lost crate and the dog's tail, which are missing.
    assert_eq!(r.summary.meshes.referenced, 9 + 15);
    assert_eq!(r.summary.meshes.converted, 7 + 14);
    assert_eq!(r.summary.meshes.failed, 2);
    assert_eq!(r.summary.actors, 3);
    assert_eq!(manifest.actors.len(), 3);
    assert!(files.contains_key("actors/minitown.esm_000863.json"));
    assert!(r.summary.records_rebuilt);
    assert_eq!(r.summary.exterior_cells, 9);
    assert_eq!(r.summary.interior_cells, 1);
    assert!(manifest.issues.is_empty(), "{:?}", manifest.issues);
}

#[test]
fn meshes_and_textures_read_back() {
    let r = run(1);
    let mir = Mir::open(&r.out).unwrap();
    let manifest = mir.manifest();
    for (hash, info) in &manifest.mesh_info {
        let mesh = mir.mesh(hash).unwrap();
        assert_eq!(mesh.primitives.len() as u32, info.primitives);
        assert!(!mesh.primitives.is_empty());
        for p in &mesh.primitives {
            assert!(p.is_well_formed(), "{hash}");
            assert_eq!(p.indices.len() % 3, 0);
            assert!(p.indices.iter().all(|&i| (i as usize) < p.positions.len()));
            assert!(!p.material.textures.is_empty());
        }
        assert!(info.bounds.is_some());
        let bytes = fs::read(r.out.join("meshes").join(format!("{hash}.glb"))).unwrap();
        assert_eq!(&bytes[..4], b"glTF");
        nif::glb::read(&bytes).unwrap();
    }
    // The house is several floor squares wide and has walls and a roof.
    let house = manifest.mesh_hash("minitown\\house.nif").unwrap();
    let info = &manifest.mesh_info[house];
    assert_eq!(info.primitives, 2);
    let [min, max] = info.bounds.unwrap();
    assert!((max[0] - min[0] - 600.0).abs() < 1e-3);
    assert_eq!(info.skipped_blocks.get("BSFurnitureMarker"), Some(&1));
    assert!(info.collision);
    for (hash, info) in &manifest.texture_info {
        let image = mir.texture(hash).unwrap();
        assert_eq!((image.width, image.height), (info.width, info.height));
        assert_eq!(image.rgba.len() as u32, info.width * info.height * 4);
        assert!(matches!((info.width, info.height), (16, 16) | (8, 8)));
    }
    // Lookups normalise the path, so a mesh material's path works as is.
    let wall = mir.texture_hash("Textures/MiniTown/Wall.DDS").unwrap();
    assert_eq!(mir.texture(wall).unwrap().width, 16);
    assert!(mir.texture_hash("minitown\\wall.dds").is_some());
    assert!(mir.texture_hash("textures\\nothing.dds").is_none());
    assert!(matches!(mir.mesh("../etc"), Err(MirError::Mesh { .. })));
    assert!(matches!(mir.texture("zz"), Err(MirError::Texture { .. })));
}

#[test]
fn records_sqlite_has_the_typed_rows() {
    let r = run(3);
    let mir = Mir::open(&r.out).unwrap();
    let records = mir.records().unwrap();
    let counts = records.record_counts().unwrap();
    let count = |sig: &str| counts.get(sig).copied().unwrap_or(0);
    assert_eq!(count("WRLD"), 1);
    assert_eq!(count("CELL"), 11); // 9 grid cells, the persistent cell, the interior
    assert_eq!(count("LAND"), 9);
    assert_eq!(count("NAVM"), 1);
    assert_eq!(count("STAT"), 4);
    assert_eq!(count("DOOR"), 1);
    assert_eq!(count("CONT"), 1);
    assert_eq!(count("MISC"), 3);
    assert_eq!(count("ARMO"), 4);
    assert_eq!(count("ARMA"), 1);
    assert_eq!(count("FLST"), 1);
    assert_eq!(count("LVLI"), 1);
    assert_eq!(count("LVLN"), 1);
    assert_eq!(count("HAIR"), 1);
    assert_eq!(count("EYES"), 1);
    assert_eq!(count("NPC_"), 3);
    assert_eq!(count("CREA"), 1);
    assert_eq!(count("ACHR"), 2);
    assert_eq!(count("ACRE"), 1);
    assert_eq!(count("REFR"), 15); // 14, and the trigger volume
    assert_eq!(count("ACTI"), 1);
    assert_eq!(count("SCPT"), 1);
    assert_eq!(count("QUST"), 1);
    assert_eq!(count("DIAL"), 1);
    assert_eq!(count("INFO"), 1);
    assert_eq!(count("RACE"), 1);
    assert_eq!(r.summary.records, counts.values().sum::<u64>() as usize);
    assert_eq!(
        records.meta("schema_version").unwrap().as_deref(),
        Some("3")
    );
    assert!(records.bad_records().unwrap().is_empty());

    // Base objects.
    let door_ref = records.placed_ref(&r.town.exterior_door).unwrap().unwrap();
    assert_eq!(door_ref.base_sig.as_deref(), Some("DOOR"));
    assert_eq!(door_ref.teleport_door.as_ref(), Some(&r.town.interior_door));
    let door = records
        .base_object(door_ref.base.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(door.kind, "DOOR");
    assert_eq!(door.editor_id.as_deref(), Some("MTHouseDoor"));
    assert_eq!(door.model.as_deref(), Some("meshes\\minitown\\door.nif"));
    assert_eq!(door.script.as_ref(), Some(&r.town.script));
    assert!(door.door.is_some());
    let crate_ref = records.placed_ref(&r.town.crate_ref).unwrap().unwrap();
    let container = records
        .base_object(crate_ref.base.as_ref().unwrap())
        .unwrap()
        .unwrap();
    let items = container.container.unwrap().items;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].1, 3);
    let can = records
        .base_object(items[0].0.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        (can.kind.as_str(), can.value, can.weight),
        ("MISC", Some(1), Some(0.5))
    );
    let fence = records.placed_ref(&r.town.rotated_fence).unwrap().unwrap();
    assert!((fence.rotation.unwrap()[2] - f64::from(FRAC_PI_4)).abs() < 1e-6);
    let sheriff = records.placed_ref(&r.town.sheriff_ref).unwrap().unwrap();
    assert_eq!(sheriff.kind, "npc");
    assert_eq!(
        records.find_editor_id("mtsheriff").unwrap(),
        std::slice::from_ref(&r.town.sheriff)
    );

    // Logic.
    let source = records.script_source(&r.town.script).unwrap().unwrap();
    assert!(source.contains("SetStage MTMainQuest 10"));
    let scripts = records.scripts().unwrap();
    assert_eq!(scripts.len(), 1);
    let stages = records.quest_stages(&r.town.quest).unwrap();
    assert_eq!(stages.iter().map(|s| s.stage).collect::<Vec<_>>(), [10, 20]);
    assert_eq!(records.topic_infos(&r.town.topic).unwrap().len(), 1);
    let functions = records.condition_function_counts().unwrap();
    assert_eq!(functions.get(&72), Some(&2));
    assert_eq!(functions.get(&58), Some(&2));
    assert_eq!(records.embedded_scripts().unwrap().len(), 2);
    let npcs = records.npc_appearances().unwrap();
    assert_eq!(npcs.len(), 3);
    assert!(npcs[0].outfit.contains("minitown.esm:00085d"));
    let infos: i64 = records
        .connection()
        .query_row("SELECT infos FROM topics", [], |row| row.get(0))
        .unwrap();
    assert_eq!(infos, 1);
}

/// The raw store gives back a record's bytes exactly, for a compressed
/// record and a plain one.
#[test]
fn the_raw_store_is_lossless() {
    let r = run(1);
    let mir = Mir::open(&r.out).unwrap();
    let records = mir.records().unwrap();
    let plugin_bytes = fs::read(r.town.data_dir.join(mir::fixture::PLUGIN)).unwrap();
    let plugin = fnv_esm::Plugin::parse(&plugin_bytes).unwrap();
    let mut found = 0;
    let mut walker = plugin.walk();
    while let Some(record) = walker.next_record().unwrap() {
        let form = FormKey::new("minitown.esm", record.form_id().local_id());
        let compressed = record.flags().is_compressed();
        let wanted = compressed || form == r.town.script;
        if !wanted {
            continue;
        }
        let data = record.data().unwrap();
        let raw = records.raw_subrecords(&form).unwrap();
        assert!(!raw.is_empty());
        let subrecords: Vec<fnv_esm::RawSubrecord> = raw
            .iter()
            .map(|(sig, bytes)| {
                let sig: [u8; 4] = sig.as_bytes().try_into().unwrap();
                fnv_esm::RawSubrecord::new(&sig, bytes.clone())
            })
            .collect();
        assert_eq!(
            fnv_esm::fixture::encode_subrecords(&subrecords),
            data.bytes(),
            "{form}"
        );
        found += 1;
    }
    assert_eq!(found, 2);
}

#[test]
fn cell_documents_have_land_references_teleports_and_navmeshes() {
    let r = run(2);
    let mir = Mir::open(&r.out).unwrap();
    assert_eq!(mir.cells().count(), 9);
    assert!(mir.cell(WORLDSPACE, 5, 5).unwrap().is_none());
    let centre = cell(&mir, 0, 0);
    assert_eq!(centre.form, r.town.centre_cell);
    assert!(
        matches!(&centre.location, CellLocation::Exterior { worldspace, x: 0, y: 0, .. } if worldspace == "minitownworld")
    );

    // Land: the slope and the ridge, row 0 south, column 0 west.
    let land = centre.land.as_ref().unwrap();
    assert_eq!(land.heights.len(), 33 * 33);
    for (row, column) in [(0, 0), (12, 0), (16, 20), (32, 32)] {
        let x = column as f32 * 128.0;
        let y = row as f32 * 128.0;
        let want = mir::fixture::terrain_height(x, y);
        let got = land.height(row, column).unwrap();
        assert!(
            (got - want).abs() <= 8.0,
            "({row}, {column}): {got} vs {want}"
        );
    }
    assert!(
        land.height(12, 0).unwrap() - land.height(10, 0).unwrap() > 300.0,
        "the ridge"
    );
    assert_eq!(land.normals.as_ref().unwrap().len(), 33 * 33);
    assert_eq!(land.layers.len(), 6);
    assert_eq!(land.layers[0].layer, -1);
    assert_eq!(
        land.layers[0].texture.as_deref(),
        Some("textures\\landscape\\mtdirt.dds")
    );
    assert!(land.layers.iter().all(|l| l.texture_hash.is_some()));
    let grass: Vec<_> = land.layers.iter().filter(|l| l.layer == 0).collect();
    assert_eq!(grass.len(), 2);
    assert_eq!(
        grass[0].texture.as_deref(),
        Some("textures\\landscape\\mtgrass.dds")
    );
    assert_eq!(grass[0].land_texture_editor_id.as_deref(), Some("MTGrass"));
    assert_eq!(grass[0].opacity.len(), 17);

    // References, including the door filed in the persistent cell.
    let house = centre.reference(&r.town.house).unwrap();
    assert_eq!(house.group, RefGroup::Temporary);
    assert_eq!(house.base_kind.as_deref(), Some("STAT"));
    assert!(house.mesh.is_some());
    let door = centre.reference(&r.town.exterior_door).unwrap();
    assert_eq!(door.group, RefGroup::WorldPersistent);
    assert!(door.flags.persistent);
    let teleport = door.teleport.as_ref().unwrap();
    assert_eq!(teleport.door, r.town.interior_door);
    let target = teleport.cell.as_ref().unwrap();
    assert_eq!(target.form, r.town.interior_cell);
    assert_eq!(target.location, CellLocation::Interior);
    assert_eq!(target.editor_id.as_deref(), Some(INTERIOR));

    // The navmesh: 3 × 3 squares, two triangles each, linked.
    assert_eq!(centre.navmeshes.len(), 1);
    let nav = &centre.navmeshes[0];
    assert_eq!(nav.form, r.town.navmesh);
    assert_eq!(nav.vertices.len(), 16);
    assert_eq!(nav.triangles.len(), 18);
    assert!(
        nav.triangles
            .iter()
            .any(|t| t.links.iter().filter(|&&l| l >= 0).count() == 3)
    );
    assert_eq!(nav.door_links.len(), 1);
    assert_eq!(nav.door_links[0].door, r.town.exterior_door);
    assert!(nav.corners(17).is_some());

    // The persistent NPC stands in cell (1, 0); the fence is rotated.
    let east = cell(&mir, 1, 0);
    let sheriff = east.reference(&r.town.sheriff_ref).unwrap();
    assert_eq!(
        (sheriff.kind, sheriff.group),
        (RefKind::Npc, RefGroup::WorldPersistent)
    );
    assert_eq!(sheriff.base, r.town.sheriff);
    let fence = east.reference(&r.town.rotated_fence).unwrap();
    assert!((fence.transform.rotation[2] - FRAC_PI_4).abs() < 1e-6);
    assert!(
        east.references
            .iter()
            .any(|r| r.group == RefGroup::VisibleDistant)
    );
    let m = fence.transform.matrix();
    let c = FRAC_PI_4.cos();
    assert!((m[0][0] - c).abs() < 1e-6 && (m[0][1] - c).abs() < 1e-6 && (m[1][0] + c).abs() < 1e-6);
    let rock = cell(&mir, -1, 0);
    assert_eq!(
        rock.reference(&r.town.scaled_rock).unwrap().transform.scale,
        2.0
    );
    let bottle = cell(&mir, 1, 1);
    let parent = bottle
        .reference(&r.town.bottle)
        .unwrap()
        .enable_parent
        .as_ref()
        .unwrap();
    assert_eq!(parent.reference, r.town.scaled_rock);
    assert!(parent.opposite);
    let disabled = cell(&mir, 0, -1);
    assert!(disabled.references[0].flags.initially_disabled);
    // References are sorted by form.
    for doc in [&centre, &east] {
        assert!(doc.references.windows(2).all(|w| w[0].form < w[1].form));
    }

    // The interior, and its door back out.
    let inside = mir.interior(INTERIOR).unwrap().unwrap();
    assert!(inside.is_interior());
    assert!(inside.land.is_none());
    assert_eq!(inside.references.len(), 6);
    assert_eq!(inside.name.as_deref(), Some("House"));
    let back = inside.reference(&r.town.interior_door).unwrap();
    let out = back.teleport.as_ref().unwrap();
    assert_eq!(out.door, r.town.exterior_door);
    assert!(matches!(
        &out.cell.as_ref().unwrap().location,
        CellLocation::Exterior { x: 0, y: 0, worldspace, .. } if worldspace == "minitownworld"
    ));
    assert_eq!(mir.interior("mthouseinterior").unwrap(), Some(inside));
    assert!(mir.interior("nowhere").unwrap().is_none());
}

/// The activator with a primitive is a trigger volume with its stand-in
/// model and mesh kept; no other reference is, and the flag stays out of
/// the JSON of every other reference.
#[test]
fn a_primitive_on_an_activator_is_a_trigger_volume() {
    let r = run(2);
    let mir = Mir::open(&r.out).unwrap();
    let doc = cell(&mir, -1, 1);
    let trigger = doc.reference(&r.town.trigger_volume).unwrap();
    assert!(trigger.trigger_volume);
    assert_eq!(trigger.base, r.town.trigger_base);
    assert_eq!(trigger.base_kind.as_deref(), Some("ACTI"));
    assert_eq!(
        trigger.model.as_deref(),
        Some(mir::fixture::TRIGGER_STAND_IN)
    );
    assert!(trigger.mesh.is_some());
    // The primitive's extents and shape are kept (header version 1.34; the
    // Fallout 3 install's sphere is checked in `fallout3.rs`).
    assert_eq!(
        trigger.primitive,
        Some(mir::PrimitiveDoc {
            bounds: [256.0, 192.0, 128.0],
            shape: mir::PrimitiveShape::Box,
        })
    );

    let mut flagged = Vec::new();
    for key in mir.cells() {
        for reference in &cell(&mir, key.x, key.y).references {
            if reference.trigger_volume {
                flagged.push(reference.form.clone());
            }
        }
    }
    let inside = mir.interior(INTERIOR).unwrap().unwrap();
    assert!(inside.references.iter().all(|r| !r.trigger_volume));
    assert_eq!(flagged, std::slice::from_ref(&r.town.trigger_volume));

    let compact = |text: String| text.split_whitespace().collect::<String>();
    let json = compact(fs::read_to_string(r.out.join("cells/minitownworld/-1_1.json")).unwrap());
    assert_eq!(json.matches("\"trigger_volume\":true").count(), 1);
    for (x, y) in [(0, 0), (1, 0)] {
        let other =
            fs::read_to_string(r.out.join(format!("cells/minitownworld/{x}_{y}.json"))).unwrap();
        assert!(!other.contains("trigger_volume"));
    }
    let report = fs::read_to_string(r.out.join("report.md")).unwrap();
    assert!(report.contains("- trigger volumes: 1 (1 with a stand-in model, not drawn)"));
}

#[test]
fn the_report_has_every_section_and_the_budget() {
    let r = run(2);
    let text = fs::read_to_string(r.out.join("report.md")).unwrap();
    for section in [
        "# MIR report",
        "## Records",
        "## References by base kind",
        "## Cells",
        "## Meshes and textures",
        "### Block types the mesh parser skipped",
        "`BSFurnitureMarker`",
        "## NPCs and appearance clusters",
        "## Actors",
        "Placed actor bases in the slice: 3 (2 NPCs, 1 creatures)",
        "## Script commands",
        "`setstage`",
        "`playsound`",
        "## Condition functions",
        "## Undecodable records and missing files",
        "lostcrate.nif",
        "sign.dds",
        "## Wall and scenery FID budget",
        "### Whole slice",
        "### One map window",
        "wall-strip entries",
        "scenery entries",
        "headroom",
    ] {
        assert!(text.contains(section), "report lacks {section:?}");
    }
    // Keywords and declared variables are not commands.
    assert!(!text.contains("`knocked`"));
    assert!(!text.contains("`endif`"));
    let mir = Mir::open(&r.out).unwrap();
    assert_eq!(
        mir::report(&mir, &mir::ReportParams::default()).unwrap(),
        text
    );
    assert_eq!(mir.report_text().unwrap(), text);
}

#[test]
fn runs_are_byte_identical_and_the_second_reuses_everything() {
    let r = run(1);
    let first = snapshot(&r.out);
    let again = extract(&r.town.options(&r.out, 1)).unwrap();
    assert!(!again.records_rebuilt);
    assert_eq!(again.meshes.converted, 0);
    assert_eq!(again.textures.converted, 0);
    assert_eq!(again.meshes.reused, r.summary.meshes.converted);
    assert_eq!(again.textures.reused, r.summary.textures.converted);
    assert!(r.summary.textures.converted > 0);
    assert_same(&first, &snapshot(&r.out));

    // A fresh directory, four threads: the same bytes.
    let other = tempfile::tempdir().unwrap();
    let out = other.path().join("mir");
    let four = extract(&r.town.options(&out, 4)).unwrap();
    assert!(four.records_rebuilt);
    assert_same(&first, &snapshot(&out));
}

#[test]
fn a_missing_mesh_and_a_corrupt_texture_are_reported() {
    let r = run(2);
    let mir = Mir::open(&r.out).unwrap();
    let manifest = mir.manifest();
    let missing = &manifest.meshes[MISSING_MESH];
    assert_eq!(missing.status, AssetStatus::Missing);
    assert!(missing.reason.is_some());
    let corrupt = &manifest.textures[CORRUPT_TEXTURE];
    assert_eq!(corrupt.status, AssetStatus::Undecodable);
    assert!(corrupt.hash.is_some());
    assert!(mir.texture_hash(CORRUPT_TEXTURE).is_none());
    let lost = cell(&mir, -1, -1);
    let reference = lost.reference(&r.town.lost_crate).unwrap();
    assert_eq!(reference.model.as_deref(), Some(MISSING_MESH));
    assert!(reference.mesh.is_none());
    // The sign mesh itself converted from its loose file.
    assert!(manifest.mesh_hash("meshes\\minitown\\sign.nif").is_some());
    assert_eq!(r.summary.textures.failed, 1);
}

#[test]
fn bad_options_and_directories_are_errors() {
    let install = tempfile::tempdir().unwrap();
    let town = mini_town(install.path());
    let out = install.path().join("out");
    let mut options = town.options(&out, 1);
    options.slice.worldspace = "NoSuchWorld".to_owned();
    assert!(matches!(
        extract(&options),
        Err(MirError::WorldspaceNotFound(_))
    ));
    options.plugins.clear();
    assert!(matches!(extract(&options), Err(MirError::Options(_))));
    let mut options = town.options(&out, 1);
    options.plugins = vec!["Absent.esm".to_owned()];
    assert!(matches!(extract(&options), Err(MirError::Io { .. })));
    assert!(Mir::open(&install.path().join("nothing")).is_err());

    // A manifest from another schema version is refused.
    let options = town.options(&out, 1);
    extract(&options).unwrap();
    let path = out.join("manifest.json");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replacen("\"schema_version\": 3", "\"schema_version\": 99", 1),
    )
    .unwrap();
    assert!(matches!(
        Mir::open(&out),
        Err(MirError::SchemaVersion { found: 99, .. })
    ));
    // So is the last version, which kept no primitive volumes: a cache
    // written then is rebuilt, not read.
    fs::write(
        &path,
        text.replacen("\"schema_version\": 3", "\"schema_version\": 2", 1),
    )
    .unwrap();
    assert!(matches!(
        Mir::open(&out),
        Err(MirError::SchemaVersion { found: 2, .. })
    ));
    // And extract rebuilds over it.
    let again = extract(&options).unwrap();
    assert!(again.records_rebuilt);
    Mir::open(&out).unwrap();
}

#[test]
fn a_bad_record_is_listed_and_skipped() {
    let install = tempfile::tempdir().unwrap();
    let town = mini_town(install.path());
    // Corrupt the compressed land record's zlib stream: the record cannot
    // be read, the extract goes on.
    let path = town.data_dir.join(mir::fixture::PLUGIN);
    let mut bytes = fs::read(&path).unwrap();
    let plugin = fnv_esm::Plugin::parse(&bytes).unwrap();
    let mut walker = plugin.walk();
    let mut target = None;
    while let Some(record) = walker.next_record().unwrap() {
        if record.flags().is_compressed() {
            target = Some(record.offset + 24 + 4);
        }
    }
    let at = target.unwrap();
    for b in &mut bytes[at..at + 8] {
        *b = 0xFF;
    }
    fs::write(&path, bytes).unwrap();
    let out = install.path().join("out");
    let summary = extract(&town.options(&out, 2)).unwrap();
    assert_eq!(summary.bad_records, 1);
    let mir = Mir::open(&out).unwrap();
    assert_eq!(mir.records().unwrap().bad_records().unwrap().len(), 1);
    assert!(cell(&mir, 0, 0).land.is_none());
    assert!(cell(&mir, 1, 0).land.is_some());
    assert!(!mir.manifest().issues.is_empty());
    assert!(mir.report_text().unwrap().contains("1 undecodable records"));
}

fn part_list(doc: &mir::ActorDoc) -> Vec<(mir::PartRole, &str, Option<&str>)> {
    doc.parts
        .iter()
        .map(|p| (p.role, p.path.as_str(), p.texture.as_deref()))
        .collect()
}

#[test]
fn actors_are_resolved_through_templates_lists_and_outfits() {
    use mir::PartRole::{Body, Hair, Head, Outfit};
    let r = run(2);
    let mir = Mir::open(&r.out).unwrap();
    let forms: Vec<&FormKey> = mir.actors().map(|e| &e.form).collect();
    assert_eq!(forms, [&r.town.sheriff, &r.town.deputy, &r.town.dog]);
    assert!(mir.actor(&r.town.farmer).unwrap().is_none());

    // The sheriff: the duster takes the upper body and, through its biped
    // model list, the left hand; the hat takes the hair; the vest loses
    // the upper body to the duster, which comes first.
    let sheriff = mir.actor(&r.town.sheriff).unwrap().unwrap();
    assert_eq!(sheriff.kind, mir::ActorKind::Npc);
    assert!(!sheriff.female);
    assert_eq!(sheriff.name.as_deref(), Some("Sheriff"));
    assert_eq!((sheriff.health, sheriff.level), (80, 6));
    assert_eq!(sheriff.race.as_ref(), Some(&r.town.race));
    assert_eq!(
        part_list(&sheriff),
        [
            (
                Body,
                "meshes\\minitown\\characters\\righthandmale.nif",
                Some("textures\\minitown\\characters\\handmale.dds")
            ),
            (Outfit, "meshes\\minitown\\armor\\duster.nif", None),
            (Outfit, "meshes\\minitown\\armor\\glove.nif", None),
            (Outfit, "meshes\\minitown\\armor\\hat.nif", None),
            (
                Head,
                "meshes\\minitown\\characters\\headmale.nif",
                Some("textures\\minitown\\characters\\headmale.dds")
            ),
        ]
    );
    assert!(sheriff.parts.iter().all(|p| p.mesh.is_some()));
    assert_eq!(sheriff.tint.hair, Some([90, 60, 30]));
    assert_eq!(sheriff.skeleton_path.as_deref(), Some(mir::HUMAN_SKELETON));
    assert_eq!(sheriff.stand_path.as_deref(), Some(mir::HUMAN_STAND));
    assert_eq!(sheriff.walk_path.as_deref(), Some(mir::HUMAN_WALK_MALE));
    assert!(sheriff.template_chains.is_empty());
    assert!(sheriff.notes.is_empty(), "{:?}", sheriff.notes);
    for part in &sheriff.parts {
        assert_eq!(
            mir.mesh_hash(&part.path),
            part.mesh.as_deref(),
            "{}",
            part.path
        );
        if let Some(texture) = &part.texture {
            assert!(mir.texture_hash(texture).is_some(), "{texture}");
        }
    }

    // The deputy: traits, model and name from the farmer, reached through
    // the leveled NPC list (its first lowest-level entry); stats and
    // inventory its own. The leveled item list gives the goggles, worn in
    // their male model for want of a female one.
    let deputy = mir.actor(&r.town.deputy).unwrap().unwrap();
    assert!(deputy.female);
    assert_eq!(deputy.name.as_deref(), Some("Farmer"));
    assert_eq!((deputy.health, deputy.level), (60, 3));
    assert_eq!(deputy.tint.hair, Some([200, 180, 60]));
    let chain = vec![r.town.settlers.clone(), r.town.farmer.clone()];
    for group in ["traits", "model", "base_data"] {
        assert_eq!(deputy.template_chains.get(group), Some(&chain), "{group}");
    }
    assert!(!deputy.template_chains.contains_key("inventory"));
    assert!(!deputy.template_chains.contains_key("stats"));
    assert_eq!(
        part_list(&deputy),
        [
            (
                Body,
                "meshes\\minitown\\characters\\lefthandfemale.nif",
                Some("textures\\minitown\\characters\\handfemale.dds")
            ),
            (
                Body,
                "meshes\\minitown\\characters\\righthandfemale.nif",
                Some("textures\\minitown\\characters\\handfemale.dds")
            ),
            (
                Body,
                "meshes\\minitown\\characters\\upperbodyfemale.nif",
                Some("textures\\minitown\\characters\\bodyfemale.dds")
            ),
            (Outfit, "meshes\\minitown\\armor\\goggles.nif", None),
            (
                Head,
                "meshes\\minitown\\characters\\headfemale.nif",
                Some("textures\\minitown\\characters\\headfemale.dds")
            ),
            (Hair, "meshes\\minitown\\characters\\hair.nif", None),
        ]
    );
    // No female walk in the data: the male one.
    assert_eq!(deputy.walk_path.as_deref(), Some(mir::HUMAN_WALK_MALE));
    // The inventory item outside the plugin is noted, nothing else.
    assert_eq!(deputy.notes.len(), 1, "{:?}", deputy.notes);
    assert!(deputy.notes[0].contains("minitown.esm:000fff"));
    assert_ne!(deputy.appearance, sheriff.appearance);
    assert_eq!(deputy.appearance.len(), 64);

    // The dog: body models in the skeleton's folder, one missing.
    let dog = mir.actor(&r.town.dog).unwrap().unwrap();
    assert_eq!(dog.kind, mir::ActorKind::Creature);
    assert_eq!(
        dog.skeleton_path.as_deref(),
        Some("meshes\\minitown\\creatures\\dog\\skeleton.nif")
    );
    assert_eq!(
        part_list(&dog),
        [
            (Body, "meshes\\minitown\\creatures\\dog\\dogbody.nif", None),
            (Body, mir::fixture::MISSING_PART, None),
        ]
    );
    assert!(dog.parts[0].mesh.is_some() && dog.parts[1].mesh.is_none());
    assert_eq!(dog.health, 25);
    assert!(dog.stand_path.is_none() && dog.walk_path.is_none());
    assert_eq!(dog.notes.len(), 1, "{:?}", dog.notes);
    assert!(dog.notes[0].contains("dogtail.nif"));

    // The creatures table reads body models against the skeleton's folder.
    let model: String = mir
        .records()
        .unwrap()
        .connection()
        .query_row("SELECT model FROM creatures", [], |row| row.get(0))
        .unwrap();
    assert_eq!(model, "meshes\\minitown\\creatures\\dog\\dogbody.nif");

    // The report counts them.
    let text = mir.report_text().unwrap();
    assert!(text.contains("1 resolved fully"), "{text}");
}
