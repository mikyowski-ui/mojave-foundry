//! Every slice file checked in under `slices/` parses and names itself
//! after its file. An exterior slice centres its map window inside its own
//! cell range and extracts the window's cells plus one all round (the map
//! stage reads that margin for objects reaching in). An interior slice
//! names its room, reaches it through the cells of one of the exterior
//! slices (or the built-in Goodsprings one), and places everyone.

use std::collections::BTreeMap;
use std::path::Path;

use mojave_convert::SliceSpec;

fn read_slices() -> BTreeMap<String, SliceSpec> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../slices");
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("slices/ exists") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("readable");
        let spec =
            SliceSpec::from_toml(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("a stem");
        assert_eq!(spec.name, stem, "{}", path.display());
        assert_eq!(spec.plugins, ["FalloutNV.esm"], "{stem}");
        assert_eq!(spec.interior_depth, 1, "{stem}");
        // What the parser reads, it writes back.
        assert_eq!(SliceSpec::from_toml(&spec.to_toml()), Ok(spec.clone()));
        out.insert(stem.to_owned(), spec);
    }
    out
}

#[test]
fn every_checked_in_exterior_slice_parses() {
    let slices = read_slices();
    let mut names = Vec::new();
    for (stem, spec) in slices.iter().filter(|(_, s)| !s.is_interior()) {
        let center = spec
            .map_center
            .unwrap_or_else(|| panic!("{stem}: no map_center"));
        for (axis, range, value) in [("x", spec.x, center[0]), ("y", spec.y, center[1])] {
            let low = range[0] as f32 * 4096.0;
            let high = (range[1] + 1) as f32 * 4096.0;
            assert!(
                (low..high).contains(&value),
                "{stem}: map_center {axis} {value} outside cells {range:?}"
            );
        }
        // The default calibration: 140 units a floor square.
        let window = mir::MapWindow::from_center(center, 140.0);
        let (xs, ys) = window.cells(1);
        assert!(
            spec.x[0] <= *xs.start() && *xs.end() <= spec.x[1],
            "{stem}: cells x {:?} do not hold the window's plus one, {xs:?}",
            spec.x
        );
        assert!(
            spec.y[0] <= *ys.start() && *ys.end() <= spec.y[1],
            "{stem}: cells y {:?} do not hold the window's plus one, {ys:?}",
            spec.y
        );
        names.push(spec.name.clone());
    }
    assert_eq!(names, ["helios", "nellis", "primm", "strip"]);
}

#[test]
fn every_checked_in_interior_slice_names_its_room_and_its_town() {
    let slices = read_slices();
    let goodsprings = SliceSpec::preset("goodsprings").expect("the built-in slice");
    let town = |name: &str| -> SliceSpec {
        if name == "goodsprings" {
            goodsprings.clone()
        } else {
            slices[name].clone()
        }
    };
    let expected = [
        ("saloon", "GSProspectorSaloonInterior", "goodsprings"),
        ("tops", "TOPSCasino", "strip"),
        ("ultra-luxe", "ULCasino", "strip"),
        ("vikki-vance", "VikkiAndVance", "primm"),
    ];
    let interiors: Vec<&str> = slices
        .iter()
        .filter(|(_, s)| s.is_interior())
        .map(|(stem, _)| stem.as_str())
        .collect();
    assert_eq!(
        interiors,
        expected.iter().map(|e| e.0).collect::<Vec<_>>(),
        "every interior slice is checked here"
    );
    for (stem, room, exterior) in expected {
        let spec = &slices[stem];
        let town = town(exterior);
        assert_eq!(spec.interior.as_deref(), Some(room), "{stem}");
        assert_eq!(
            (&spec.worldspace, spec.x, spec.y),
            (&town.worldspace, town.x, town.y),
            "{stem}: the cells of {exterior}"
        );
        assert!(
            spec.place_disabled,
            "{stem}: the owner chose to place everyone"
        );
        // The defaults are valid for every room.
        let slicer = spec.interior_slice(spec.floor_z.unwrap_or(0.0));
        assert!(slicer.floor_band[1] < slicer.cut_height, "{stem}");
        let build = spec.interior_build(spec.floor_z.unwrap_or(0.0));
        assert!(build.below >= 0.0 && build.above >= 0.0, "{stem}");
    }
    // The Tops: its main floor, the sunken area in both bands, people on
    // the raised level moved down.
    let tops = &slices["tops"];
    assert_eq!(tops.floor_z, Some(8128.0));
    assert!(tops.clamp_levels);
    let build = tops.interior_build(8128.0);
    assert!(build.in_band(8064.0) && !build.in_band(8448.0));
    let slicer = tops.interior_slice(8128.0);
    assert!(slicer.floor_band[0] <= 8064.0 - 8128.0);
    // The Ultra-Luxe: the floor is set between its sunken pit and its upper
    // ring, and both bands span the two.
    let luxe = &slices["ultra-luxe"];
    assert_eq!(luxe.floor_z, Some(7616.0));
    let build = luxe.interior_build(7616.0);
    assert!(build.in_band(7554.0) && build.in_band(7680.0));
    // Vikki & Vance: the default floor, with the pit 64 units lower in band.
    let vikki = &slices["vikki-vance"];
    assert_eq!(vikki.floor_z, None);
    let build = vikki.interior_build(7296.0);
    assert!(build.in_band(7232.0));
    // No room but the Tops moves people between levels or recentres.
    for stem in ["saloon", "ultra-luxe", "vikki-vance"] {
        let spec = &slices[stem];
        assert!(!spec.clamp_levels, "{stem}");
        assert_eq!(spec.map_center, None, "{stem}");
    }
}
