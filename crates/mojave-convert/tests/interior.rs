//! An interior slice on the synthetic mini town: the house's interior
//! becomes a standalone demo map with its people standing in it.
//!
//! The fixture's interior has no navmesh, so after extract the test gives
//! the room's document a flat synthetic one (a square floor around the
//! room's references), as a real room has.

mod common;

use mojave_convert::{SliceSpec, critters, map, pipeline, sprites};

/// A flat navmesh square `[-half, half]` on both axes at height `z`, two
/// triangles, in the interior document's JSON form.
fn navmesh_json(half: f32, z: f32) -> serde_json::Value {
    serde_json::json!({
        "form": "minitown.esm:000fff",
        "vertices": [[-half, -half, z], [half, -half, z], [half, half, z], [-half, half, z]],
        "triangles": [
            {"vertices": [0, 1, 2], "links": [-1, -1, 1], "flags": 0},
            {"vertices": [0, 2, 3], "links": [0, -1, -1], "flags": 0}
        ]
    })
}

fn interior_slice(base: &SliceSpec) -> SliceSpec {
    SliceSpec {
        name: "house".to_owned(),
        interior: Some(mir::fixture::INTERIOR.to_owned()),
        place_disabled: true,
        ..base.clone()
    }
}

#[test]
fn an_interior_slice_builds_one_walkable_room_with_its_people() {
    let fixture = common::fixture();
    let mut inputs = fixture.inputs("out-interior", 2);
    inputs.slice = interior_slice(&fixture.slice);

    // Without --standalone the run is refused before any stage.
    let error = pipeline::all(&inputs).expect_err("refused").to_string();
    assert!(error.contains("--standalone"), "{error}");
    assert!(!inputs.mir_dir().exists(), "nothing was extracted");
    inputs.standalone = true;

    pipeline::extract(&inputs).expect("extract");
    let doc_path = inputs.mir_dir().join("interiors").join(format!(
        "{}.json",
        mir::fixture::INTERIOR.to_ascii_lowercase()
    ));
    let text = std::fs::read_to_string(&doc_path).expect("the room was extracted");
    let mut doc: serde_json::Value = serde_json::from_str(&text).expect("json");
    doc["navmeshes"] = serde_json::json!([navmesh_json(500.0, 0.0)]);
    std::fs::write(&doc_path, serde_json::to_string(&doc).expect("json")).expect("write");

    let estimate = map::estimate(&inputs).expect("estimate");
    assert!(estimate.references > 0);
    assert!(estimate.report.interior.is_some(), "{}", estimate.report);

    sprites::sprites(&inputs).expect("sprites");
    critters::critters(&inputs).expect("critters");
    let summary = map::map(&inputs).expect("map");
    assert_eq!(summary.maps, 1, "{summary:?}");
    assert_eq!(summary.exits, 0, "{summary:?}");
    assert!(summary.walkable_hexes > 50, "{summary:?}");
    assert_eq!(summary.critters, 1, "the deputy: {summary:?}");

    let report = std::fs::read_to_string(inputs.out.join("map-report.md")).expect("report");
    for line in [
        "## Interior",
        "- cell: MTHouseInterior",
        "- floor: z 0.0, from the largest navmesh storey",
        "- storeys found: 1",
        "the room's first teleport door",
        "not converted on an interior demo",
        "People placed: 1",
        "a creature: creatures are not rendered yet",
    ] {
        assert!(report.contains(line), "no {line:?} in\n{report}");
    }

    let features = map::MapFeatures::read(&inputs.out).expect("map-features.json");
    assert_eq!(features.interior.as_deref(), Some(mir::fixture::INTERIOR));
    assert_eq!(features.people.len(), 1, "{features:?}");
    assert_eq!(features.map_file, summary.map_file);

    // The layer holds the one map.
    let maps: Vec<String> = common::tree(&inputs.mod_dir())
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| path.starts_with("maps/") && path.ends_with(".map"))
        .collect();
    assert_eq!(
        maps,
        [format!(
            "maps/{}",
            summary.map_file.trim_start_matches("maps/")
        )]
    );

    // Unchanged inputs: reused. A changed interior setting: rebuilt.
    assert!(map::map(&inputs).expect("again").reused);
    inputs.slice.cut_height = Some(150.0);
    assert!(!map::map(&inputs).expect("new cut").reused);
}

#[test]
fn people_the_data_starts_disabled_are_left_out_unless_asked() {
    let fixture = common::fixture();
    let mut inputs = fixture.inputs("out-interior-off", 2);
    inputs.slice = interior_slice(&fixture.slice);
    inputs.slice.place_disabled = false;
    inputs.standalone = true;
    pipeline::extract(&inputs).expect("extract");
    let doc_path = inputs.mir_dir().join("interiors").join(format!(
        "{}.json",
        mir::fixture::INTERIOR.to_ascii_lowercase()
    ));
    let text = std::fs::read_to_string(&doc_path).expect("the room was extracted");
    let mut doc: serde_json::Value = serde_json::from_str(&text).expect("json");
    doc["navmeshes"] = serde_json::json!([navmesh_json(500.0, 0.0)]);
    // The deputy starts disabled in this copy of the data.
    for reference in doc["references"].as_array_mut().expect("references") {
        if reference["base_kind"] == "NPC_" {
            reference["flags"]["initially_disabled"] = serde_json::json!(true);
        }
    }
    std::fs::write(&doc_path, serde_json::to_string(&doc).expect("json")).expect("write");
    critters::critters(&inputs).expect("critters");
    let summary = map::map(&inputs).expect("map");
    assert_eq!(summary.critters, 0, "{summary:?}");
    let report = std::fs::read_to_string(inputs.out.join("map-report.md")).expect("report");
    assert!(
        report.contains("starts disabled in the data (place_disabled is off)"),
        "{report}"
    );
    inputs.slice.place_disabled = true;
    assert_eq!(map::map(&inputs).expect("map").critters, 1);
}
