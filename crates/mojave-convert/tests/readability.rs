//! The readability dials on the linked, two-storey mini mart
//! (`common::mini_mart::write_linked`): the world turned by 0, 90, 180, 270
//! and 45 degrees, spread out and its walls compressed, through the whole
//! map stage of a layer.
//!
//! For every quarter turn: the door stays linked both ways and the ramp's
//! stair link holds (`verify_links` clean), every person stands on the
//! turned image of the hex the unturned build gave them (within one hex,
//! plus the steps either build moved them) and faces the hex direction
//! nearest the turned source heading, the walkable hexes per elevation
//! stay within the rasterisation tolerance below, the placement index
//! names only objects the written maps hold and carries the turn, and two
//! builds give the same bytes. A turn of 0, or the table with every dial at
//! its default, is the run without it: the same key (the stage reuses its
//! output) and the same bytes.
//!
//! **Walkable tolerance.** A hex is walkable when a navmesh triangle covers
//! it (within the coverage rule's tolerance). The hex lattice is not
//! invariant under a quarter turn: odd columns sit a quarter of a square
//! lower, so a turned floor's edge rows land on lattice points differently
//! and each boundary hex can come or go. The tolerance is the boundary: 20 %
//! of the count, at least 12 hexes (the store's floor is 1,200 by 800
//! units, about 17 by 11 hexes, so about 60 boundary hexes).

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::mini_mart;
use mojave_convert::readability::ReadabilitySpec;
use mojave_convert::{Inputs, Install, pipeline};
use vault_formats::HexGrid;

/// One person line of a layer report: `- NAME (REF) on elevation E hex H,
/// facing F, moved S steps`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Person {
    elevation: u8,
    hex: i32,
    facing: u8,
    steps: i32,
}

fn people(report: &str) -> BTreeMap<String, Person> {
    let mut out = BTreeMap::new();
    for line in report.lines() {
        let Some((left, right)) = line.split_once(" on elevation ") else {
            continue;
        };
        let Some(open) = left.rfind('(') else {
            continue;
        };
        let reference = left[open + 1..].trim_end_matches(')').to_owned();
        let words: Vec<&str> = right.split([' ', ',']).filter(|w| !w.is_empty()).collect();
        // E hex H facing F moved S steps
        if words.len() < 7 || words[1] != "hex" || words[3] != "facing" {
            continue;
        }
        out.insert(
            reference,
            Person {
                elevation: words[0].parse().unwrap(),
                hex: words[2].parse().unwrap(),
                facing: words[4].parse().unwrap(),
                steps: words[6].parse().unwrap(),
            },
        );
    }
    out
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

/// A frame of the default projection: tile space and hex directions only.
fn frame() -> map_builder::walk::Frame {
    let projection =
        foundry_render::Projection::new(&foundry_render::Calibration::default()).unwrap();
    let window = mir::MapWindow::from_center([0.0, 0.0], projection.tile_units());
    map_builder::walk::Frame::new(&window, &projection).unwrap()
}

/// The hex under the image of `hex`'s centre turned clockwise (seen from
/// above) by `degrees` about the window's centre, tile (50, 50).
fn turned_hex(frame: &map_builder::walk::Frame, hex: i32, degrees: f64) -> Option<i32> {
    let [s, t] = frame.hex_centre_tile(hex)?;
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (ds, dt) = (s - 50.0, t - 50.0);
    frame.hex_of_tile([50.0 + cos * ds - sin * dt, 50.0 + sin * ds + cos * dt])
}

struct Build {
    files: BTreeMap<String, Vec<u8>>,
    report: String,
    walkable: BTreeMap<(String, u8), u32>,
}

fn build(inputs: &Inputs) -> Build {
    let summary = mojave_convert::map::map(inputs).expect("the map stage");
    assert!(!summary.reused);
    read(inputs)
}

fn read(inputs: &Inputs) -> Build {
    let layer = map_builder::read_layer(&inputs.mod_dir()).expect("the layer reads back");
    let report = std::fs::read_to_string(inputs.out.join("map-report.md")).expect("report");
    let facts = mojave_convert::placements::read_facts(&inputs.out)
        .expect("map-sim.json")
        .expect("written");
    let walkable = facts
        .maps
        .iter()
        .flat_map(|m| {
            m.walkable
                .iter()
                .map(|w| ((m.stem.clone(), w.elevation), w.count))
        })
        .collect();
    Build {
        files: layer.files().clone(),
        report,
        walkable,
    }
}

/// `verify_layer`, `verify_links` (no link one way, the door and the stairs
/// both ways) and the placement index against the written maps.
fn check_layer(inputs: &Inputs, degrees: f64) {
    let base = BTreeMap::new();
    let layer = map_builder::read_layer(&inputs.mod_dir()).unwrap();
    let verified = map_builder::verify_layer(&layer, &base).expect("verify_layer");
    assert_eq!(verified.maps.len(), 2);
    let links = map_builder::verify_links(&layer, &base).expect("verify_links");
    assert!(links.one_way.is_empty(), "{degrees}: {links:?}");
    assert!(
        links.links >= 4,
        "{degrees}: door and stairs both ways: {links:?}"
    );

    let index = mojave_convert::placements::read(&inputs.out)
        .expect("the index reads")
        .expect("written");
    for note in &index.notes {
        assert!(
            !note.contains("does not hold") && !note.contains("bear out"),
            "{degrees}: {note}"
        );
    }
    let (_, written) = mojave_convert::placements::read_written_maps(&inputs.mod_dir()).unwrap();
    for placed in &index.placed {
        let Some(id) = placed.object_id else { continue };
        let map = written.iter().find(|w| w.stem == placed.map).unwrap();
        let object = map
            .objects
            .iter()
            .find(|o| o.id == id && o.elevation == placed.elevation)
            .unwrap_or_else(|| panic!("{degrees}: object {id} of {placed:?} not written"));
        assert_eq!(object.hex, placed.hex, "{degrees}: {placed:?}");
    }
    let expected = (degrees != 0.0).then_some(degrees);
    assert_eq!(index.world_yaw_deg, expected);
    for entry in index.maps.iter().filter(|m| m.stem.starts_with("mm")) {
        assert_eq!(entry.world_yaw_deg, expected, "{entry:?}");
    }
}

#[test]
fn the_linked_mart_turns_spreads_and_compresses() {
    let fixture = common::fixture();
    let dir = fixture.root.path().join("linked");
    let mart = mini_mart::write_linked(&dir);
    let inputs_for = |layer_file: &Path, out: &Path| {
        let mut inputs = Inputs::from_args(
            &Install::any(&mart.data_dir, None),
            Some(layer_file.to_str().unwrap()),
            &fixture.f2,
            out,
            2,
        )
        .expect("inputs");
        inputs.standalone = true;
        inputs
    };
    let out: PathBuf = fixture.root.path().join("out-turns");
    let mut inputs = inputs_for(&mart.layer_file, &out);
    pipeline::all(&inputs).expect("the pipeline runs");
    let baseline = read(&inputs);
    check_layer(&inputs, 0.0);
    let base_people = people(&baseline.report);
    assert!(base_people.len() >= 5, "{}", baseline.report);
    assert!(baseline.report.contains("## FID budget"));
    assert!(baseline.report.contains("## Readability dials"));

    // A warm copy of the run (its `cache/map.key` included): without a
    // flag it is a cache hit with the same bytes; any dial that differs
    // from its default is in the stage's key, so it rebuilds.
    let warm_out = fixture.root.path().join("out-warm");
    copy_dir(&out, &warm_out);
    let mut warm = inputs_for(&mart.layer_file, &warm_out);
    assert!(mojave_convert::map::map(&warm).unwrap().reused);
    assert_eq!(read(&warm).files, baseline.files);
    for flag in [
        "units_per_hex=56",
        "wall_z_scale=0.78",
        "world_yaw_deg=90",
        "mark_links=true",
    ] {
        warm.readability_cli = ReadabilitySpec::parse_assignment(flag).unwrap();
        assert!(
            !mojave_convert::map::map(&warm).unwrap().reused,
            "{flag} must rebuild a warm copy"
        );
    }

    // A turn of 0 on the command line, and a layer file whose top-level
    // table sets every built dial to its default: the same key.
    inputs.readability_cli = ReadabilitySpec::parse_assignment("world_yaw_deg=0").unwrap();
    assert!(mojave_convert::map::map(&inputs).unwrap().reused);
    let with_table = dir.join("defaults.layer.toml");
    let text = std::fs::read_to_string(&mart.layer_file).unwrap()
        + "\n[readability]\nworld_yaw_deg = 0.0\nunits_per_hex = 70.0\nwall_z_scale = 1.0\n";
    std::fs::write(&with_table, text).unwrap();
    assert!(
        mojave_convert::map::map(&inputs_for(&with_table, &out))
            .unwrap()
            .reused
    );
    // And built afresh at 0, byte for byte the run without the key.
    let zero_out = fixture.root.path().join("out-zero");
    copy_dir(&out, &zero_out);
    std::fs::remove_file(zero_out.join("cache").join("map.key")).unwrap();
    let mut zero = inputs_for(&mart.layer_file, &zero_out);
    zero.readability_cli = ReadabilitySpec::parse_assignment("world_yaw_deg=0").unwrap();
    assert_eq!(build(&zero).files, baseline.files);

    // The mart's people face the way their records say (all of them turned
    // 0 in the fixture, so the hex direction nearest a source heading of 0).
    let frame = frame();
    let mir = mir::Mir::open(&inputs.mir_dir()).unwrap();
    let store = mir
        .interior(mini_mart::INTERIOR)
        .unwrap()
        .expect("the store");
    let heading: BTreeMap<String, f32> = store
        .references
        .iter()
        .map(|r| (r.form.to_string(), r.transform.rotation[2]))
        .collect();

    let mut ninety = None;
    for degrees in [90.0, 180.0, 270.0] {
        inputs.readability_cli =
            ReadabilitySpec::parse_assignment(&format!("world_yaw_deg={degrees}")).unwrap();
        let turned = build(&inputs);
        check_layer(&inputs, degrees);
        assert_ne!(turned.files, baseline.files, "{degrees}: the map changed");
        let yaw = mir::WorldYaw::new([0.0, 0.0], degrees);
        let now = people(&turned.report);
        assert_eq!(now.len(), base_people.len(), "{}", turned.report);
        for (reference, before) in &base_people {
            let after = now[reference];
            assert_eq!(after.elevation, before.elevation, "{degrees}: {reference}");
            let image = turned_hex(&frame, before.hex, degrees).expect("on the grid");
            let off = HexGrid::lattice_distance(after.hex, image).unwrap();
            assert!(
                off <= 1 + before.steps + after.steps,
                "{degrees}: {reference} on {} but the image of {} is {image} ({off} off)",
                after.hex,
                before.hex
            );
            // Facing: the direction nearest the turned heading.
            let rz = heading[reference];
            assert_eq!(
                Some(after.facing),
                frame.orientation_of_yaw(yaw.apply_yaw(rz)),
                "{degrees}: {reference}"
            );
            assert_eq!(Some(before.facing), frame.orientation_of_yaw(rz));
            // A quarter turn and the rest of the circle come back.
            let back = mir::WorldYaw::new([0.0, 0.0], 360.0 - degrees);
            assert_eq!(
                frame.orientation_of_yaw(back.apply_yaw(yaw.apply_yaw(rz))),
                Some(before.facing)
            );
        }
        for (key, count) in &baseline.walkable {
            let other = turned.walkable[key];
            let tolerance = (*count / 5).max(12);
            assert!(
                count.abs_diff(other) <= tolerance,
                "{degrees}: {key:?} walkable {other}, unturned {count} (tolerance {tolerance})"
            );
        }
        if degrees == 90.0 {
            ninety = Some(turned.files);
        }
    }
    // Two builds of one turn: the same bytes.
    inputs.readability_cli = ReadabilitySpec::parse_assignment("world_yaw_deg=90").unwrap();
    assert_eq!(Some(build(&inputs).files), ninety);

    // Any angle builds and keeps its links.
    inputs.readability_cli = ReadabilitySpec::parse_assignment("world_yaw_deg=45").unwrap();
    build(&inputs);
    check_layer(&inputs, 45.0);

    // Spread out and compressed: more walkable hexes, links kept, the
    // budget reported.
    inputs.readability_cli = ReadabilitySpec::parse_assignment("units_per_hex=56")
        .unwrap()
        .over(&ReadabilitySpec::parse_assignment("wall_z_scale=0.78").unwrap());
    let spread = build(&inputs);
    check_layer(&inputs, 0.0);
    let store_floor = ("mmstore".to_owned(), 0u8);
    assert!(
        spread.walkable[&store_floor] > baseline.walkable[&store_floor] * 5 / 4,
        "{} vs {}",
        spread.walkable[&store_floor],
        baseline.walkable[&store_floor]
    );
    for line in [
        "units_per_hex = 56",
        "wall_z_scale = 0.78",
        "## FID budget",
        "| walls |",
    ] {
        assert!(
            spread.report.contains(line),
            "no {line:?} in\n{}",
            spread.report
        );
    }
}

/// A spread-out window that leaves a link off the map is refused, naming
/// the door; one that leaves only scenery off is reported.
#[test]
fn a_link_the_spread_window_leaves_off_is_refused() {
    let fixture = common::fixture();
    let dir = fixture.root.path().join("clip");
    let mart = mini_mart::write_linked(&dir);
    // The car park's window moved off its door: today's 14,000-unit window
    // still holds the door, a 4,000-unit one (20 units per hex) does not.
    let layer = std::fs::read_to_string(&mart.layer_file).unwrap().replace(
        "map = \"mmlot\"\n",
        "map = \"mmlot\"\nmap_center = [4100.0, 4100.0]\n",
    );
    let file = dir.join("moved.layer.toml");
    std::fs::write(&file, layer).unwrap();
    let mut inputs = Inputs::from_args(
        &Install::any(&mart.data_dir, None),
        Some(file.to_str().unwrap()),
        &fixture.f2,
        &fixture.root.path().join("out-clip"),
        2,
    )
    .unwrap();
    inputs.standalone = true;
    pipeline::all(&inputs).expect("today's window holds the door");
    inputs.readability_cli = ReadabilitySpec::parse_assignment("units_per_hex=20").unwrap();
    let error = mojave_convert::map::map(&inputs)
        .expect_err("the door falls outside")
        .to_string();
    assert!(error.contains("links to map mmstore"), "{error}");
}

/// The `## Ground` statistics' blank squares of one map elevation in a
/// layer report.
fn blank_squares(report: &str, map: &str, elevation: u8) -> usize {
    let start = report
        .find(&format!("# Map `maps/{map}.map`"))
        .unwrap_or_else(|| panic!("no map {map} in the report"));
    let rest = &report[start..];
    let at = rest
        .find(&format!("## Elevation {elevation} "))
        .unwrap_or_else(|| panic!("no elevation {elevation} of {map}"));
    let rest = &rest[at..];
    let field = rest.find("blank_squares: ").expect("ground statistics");
    rest[field + "blank_squares: ".len()..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// The walkable runs of every map elevation, from `map-sim.json`.
fn walk_runs(inputs: &Inputs) -> BTreeMap<(String, u8), String> {
    let facts = mojave_convert::placements::read_facts(&inputs.out)
        .unwrap()
        .unwrap();
    facts
        .maps
        .iter()
        .flat_map(|m| {
            m.walkable
                .iter()
                .map(|w| ((m.stem.clone(), w.elevation), w.runs.clone()))
        })
        .collect()
}

/// The lower-level context on the mart's upper storey, and the keys of the
/// later passes: each rebuilds a warm copy; the context fills blank floor
/// near the storey's walkable hexes and changes nothing else; two builds
/// are the same.
#[test]
fn the_storey_below_shows_through_and_every_later_dial_rebuilds() {
    let fixture = common::fixture();
    let dir = fixture.root.path().join("context");
    let mart = mini_mart::write_linked(&dir);
    let inputs_for = |layer_file: &Path, out: &Path| {
        let mut inputs = Inputs::from_args(
            &Install::any(&mart.data_dir, None),
            Some(layer_file.to_str().unwrap()),
            &fixture.f2,
            out,
            2,
        )
        .expect("inputs");
        inputs.standalone = true;
        inputs
    };
    let out: PathBuf = fixture.root.path().join("out-context");
    let mut inputs = inputs_for(&mart.layer_file, &out);
    pipeline::all(&inputs).expect("the pipeline runs");
    let off = read(&inputs);
    let off_runs = walk_runs(&inputs);
    let off_objects = mojave_convert::placements::read_written_maps(&inputs.mod_dir())
        .unwrap()
        .1;

    // Every new key in the stage's key: a warm copy rebuilds.
    let warm_out = fixture.root.path().join("out-context-warm");
    copy_dir(&out, &warm_out);
    let mut warm = inputs_for(&mart.layer_file, &warm_out);
    for flag in [
        "lower_level_dim=0.35",
        "lower_level_desaturate=0.25",
        "lower_level_reach=6",
        "lower_level_tiles=64",
        "link_cue_strength=0.6",
        "fit_view_border=true",
    ] {
        warm.readability_cli = ReadabilitySpec::parse_assignment(flag).unwrap();
        assert!(
            !mojave_convert::map::map(&warm).unwrap().reused,
            "{flag} must rebuild a warm copy"
        );
    }
    // The dials of the declutter pass, on the store (whose slice has it on),
    // as a map's own table: they rebuild too.
    let layer_text = std::fs::read_to_string(&mart.layer_file).unwrap();
    for table in ["ground_calm = 0.5", "foreground_kerb_narrow = 16.0"] {
        let file = dir.join("dialled.layer.toml");
        std::fs::write(
            &file,
            format!("{layer_text}\n[maps.readability]\n{table}\n"),
        )
        .unwrap();
        let dialled = inputs_for(&file, &warm_out);
        assert!(
            !mojave_convert::map::map(&dialled).unwrap().reused,
            "{table} must rebuild a warm copy"
        );
        assert!(
            read(&dialled)
                .report
                .contains(table.split(' ').next().unwrap())
        );
    }
    // On a map without the pass, they are refused with the way out.
    warm.readability_cli = ReadabilitySpec::parse_assignment("ground_calm=0.5").unwrap();
    let error = mojave_convert::map::map(&warm).unwrap_err().to_string();
    assert!(error.contains("--declutter readable"), "{error}");

    // The context on the store's upper storey.
    inputs.readability_cli = ReadabilitySpec::parse_assignment("lower_level_dim=0.35")
        .unwrap()
        .over(&ReadabilitySpec::parse_assignment("lower_level_desaturate=0.25").unwrap());
    let on = build(&inputs);
    let (before, after) = (
        blank_squares(&off.report, "mmstore", 1),
        blank_squares(&on.report, "mmstore", 1),
    );
    assert!(
        after < before,
        "blank squares {after} with the context, {before} without"
    );
    assert_eq!(
        blank_squares(&on.report, "mmstore", 0),
        blank_squares(&off.report, "mmstore", 0)
    );
    assert!(
        on.report
            .contains("Lower-level context (`lower_level_dim` 0.35"),
        "{}",
        on.report
    );
    assert!(
        on.report.contains("| mmstore elevation 1 |"),
        "{}",
        on.report
    );
    // Walkable hexes, every object and every link as without it.
    assert_eq!(walk_runs(&inputs), off_runs);
    let on_objects = mojave_convert::placements::read_written_maps(&inputs.mod_dir())
        .unwrap()
        .1;
    assert_eq!(on_objects.len(), off_objects.len());
    for (a, b) in on_objects.iter().zip(&off_objects) {
        assert_eq!((&a.stem, a.number), (&b.stem, b.number));
        assert_eq!(a.objects, b.objects, "{}", a.stem);
    }
    check_layer(&inputs, 0.0);
    let tiles = String::from_utf8(on.files["art/tiles/tiles.lst"].clone()).unwrap();
    assert!(tiles.lines().count() <= 4096);
    assert_ne!(on.files, off.files);
    // Two builds: the same bytes.
    std::fs::remove_file(out.join("cache").join("map.key")).unwrap();
    assert_eq!(build(&inputs).files, on.files);
}
