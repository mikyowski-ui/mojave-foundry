//! Every slice file checked in under `slices/` parses and names itself
//! after its file. An exterior slice centres its map window inside its own
//! cell range and extracts the window's cells plus one all round (the map
//! stage reads that margin for objects reaching in). An interior slice
//! names its room, reaches it through the cells of one of the exterior
//! slices (or the built-in Goodsprings one), and places everyone.
//!
//! New Vegas slices are in `slices/`, Fallout 3 slices in `slices/fo3/`;
//! each says (or its plugins imply) its own game. Layer files
//! (`*.layer.toml`) beside them resolve to slices of one game and
//! worldspace with distinct map stems.

use std::collections::BTreeMap;
use std::path::Path;

use mojave_convert::{GameVariant, SliceSpec};

fn read_slices() -> BTreeMap<String, SliceSpec> {
    read_dir("", GameVariant::FalloutNewVegas)
}

fn read_fo3_slices() -> BTreeMap<String, SliceSpec> {
    read_dir("fo3", GameVariant::Fallout3)
}

fn read_dir(sub: &str, game: GameVariant) -> BTreeMap<String, SliceSpec> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../slices")
        .join(sub);
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("slices/ exists") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml")
            || mojave_convert::layer::is_layer_file(&path.to_string_lossy())
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("readable");
        let spec =
            SliceSpec::from_toml(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("a stem");
        assert_eq!(spec.name, stem, "{}", path.display());
        assert_eq!(spec.plugins, [game.master_name()], "{stem}");
        assert_eq!(spec.stated_game(), Some(game), "{stem}");
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

#[test]
fn every_fallout3_slice_parses_and_names_its_game() {
    let slices = read_fo3_slices();
    let names: Vec<&str> = slices.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "megaton",
            "megaton-saloon",
            "springvale",
            "super-duper-mart",
            "super-duper-mart-exterior"
        ]
    );
    for (stem, spec) in &slices {
        assert_eq!(
            spec.game,
            Some(GameVariant::Fallout3),
            "{stem}: game = \"fo3\""
        );
    }
    // The exterior windows: centred in their cells, which hold the window
    // plus one cell all round, except Megaton, whose worldspace has the
    // three by three cells of the town only.
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
        let margin = if stem == "megaton" { -1 } else { 1 };
        let window = mir::MapWindow::from_center(center, 140.0);
        let (xs, ys) = window.cells(margin);
        assert!(
            spec.x[0] <= *xs.start() && *xs.end() <= spec.x[1],
            "{stem}: cells x {:?} do not hold {xs:?}",
            spec.x
        );
        assert!(
            spec.y[0] <= *ys.start() && *ys.end() <= spec.y[1],
            "{stem}: cells y {:?} do not hold {ys:?}",
            spec.y
        );
    }
    assert_eq!(slices["megaton"].worldspace, "MegatonWorld");
    assert_eq!(
        (slices["megaton"].x, slices["megaton"].y),
        ([-2, 0], [-8, -6])
    );
    // The Super-Duper Mart's centre lies in cell (4, -5), by its doors.
    let mart = &slices["super-duper-mart-exterior"];
    let [cx, cy] = mart.map_center.expect("a centre");
    assert_eq!(
        ((cx / 4096.0).floor() as i32, (cy / 4096.0).floor() as i32),
        (4, -5)
    );
    // The rooms and the exterior cells their doors are in.
    for (stem, room, town) in [
        (
            "super-duper-mart",
            "SuperDuperMart",
            ("Wasteland", [3, 5], [-6, -4]),
        ),
        (
            "megaton-saloon",
            "MegatonMoriartysSaloon",
            ("MegatonWorld", [-2, 0], [-8, -6]),
        ),
    ] {
        let spec = &slices[stem];
        assert_eq!(spec.interior.as_deref(), Some(room), "{stem}");
        assert_eq!((spec.worldspace.as_str(), spec.x, spec.y), town, "{stem}");
        assert!(spec.place_disabled, "{stem}");
        let slicer = spec.interior_slice(0.0);
        assert!(slicer.floor_band[1] < slicer.cut_height, "{stem}");
    }
    assert_eq!(slices["megaton-saloon"].cut_height, Some(120.0));
}

#[test]
fn the_built_in_slices_are_per_game() {
    assert_eq!(
        SliceSpec::default_name(GameVariant::FalloutNewVegas),
        "goodsprings"
    );
    assert_eq!(SliceSpec::default_name(GameVariant::Fallout3), "springvale");
    let goodsprings = SliceSpec::preset("goodsprings").expect("built in");
    assert_eq!(
        goodsprings.stated_game(),
        Some(GameVariant::FalloutNewVegas)
    );
    // Goodsprings states no game key: its plugins say it, and its TOML (in
    // the map stage's cache key) is what it always was.
    assert_eq!(goodsprings.game, None);
    assert!(!goodsprings.to_toml().contains("game"));
    // The built-in Springvale is the checked-in file.
    let springvale = SliceSpec::preset("springvale").expect("built in");
    assert_eq!(springvale, read_fo3_slices()["springvale"]);
}

#[test]
fn every_checked_in_layer_resolves() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../slices");
    let mut found = BTreeMap::new();
    for (sub, game) in [
        ("", GameVariant::FalloutNewVegas),
        ("fo3", GameVariant::Fallout3),
    ] {
        let dir = root.join(sub);
        for entry in std::fs::read_dir(&dir).expect("slices/ exists") {
            let path = entry.expect("an entry").path();
            if !mojave_convert::layer::is_layer_file(&path.to_string_lossy()) {
                continue;
            }
            let layer = mojave_convert::layer::LayerSpec::load(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let stem = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".layer.toml"))
                .expect("a layer file name");
            assert_eq!(layer.name, stem, "{}", path.display());
            for map in &layer.maps {
                assert_eq!(map.slice.stated_game(), Some(game), "{stem}");
                assert!(
                    map.slice.loot == Some(true),
                    "{stem}: loot on in every layer"
                );
            }
            let stems: Vec<String> = layer.maps.iter().map(|m| m.slice.map_stem()).collect();
            found.insert(stem.to_owned(), stems);
        }
    }
    assert_eq!(
        found,
        BTreeMap::from([
            (
                "goodsprings-linked".to_owned(),
                vec!["goodspri".to_owned(), "saloon".to_owned()]
            ),
            (
                "megaton".to_owned(),
                vec!["megaton", "moriarty", "crater", "clinic"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            ),
            (
                "springvale".to_owned(),
                vec!["springva".to_owned(), "bandits".to_owned()]
            ),
            (
                "super-duper-mart".to_owned(),
                vec!["sdmart".to_owned(), "sdmartin".to_owned()]
            ),
        ])
    );
}

#[test]
fn island_fixes_are_on_unless_a_file_says_false() {
    use mojave_convert::layer::LayerSpec;
    let dir = Path::new(".");
    let layer = |extra: &str| {
        LayerSpec::from_toml(
            &format!("name = \"x\"\n{extra}\n[[maps]]\nslice = \"goodsprings\"\n"),
            dir,
        )
        .unwrap()
    };
    assert!(layer("").close_unreached);
    assert!(layer("close_unreached = true").close_unreached);
    assert!(!layer("close_unreached = false").close_unreached);
    // Slices: absent means the builder's default (on); false is accepted.
    let head = "name = \"a\"\nworldspace = \"W\"\nplugins = [\"F.esm\"]\nx = [0, 0]\ny = [0, 0]\n";
    let none = SliceSpec::from_toml(head).unwrap();
    assert_eq!(none.close_unreached, None);
    assert_eq!(none.coverage_bridges, None);
    let off = SliceSpec::from_toml(&format!(
        "{head}close_unreached = false\ncoverage_bridges = false\n"
    ))
    .unwrap();
    assert_eq!(off.close_unreached, Some(false));
    assert_eq!(off.coverage_bridges, Some(false));
    let options = map_builder::BuildOptions::default();
    assert!(options.close_unreached && options.coverage_bridges);
    // A single slice made into a layer follows its own key.
    let slice = SliceSpec {
        close_unreached: Some(false),
        ..SliceSpec::preset("goodsprings").unwrap()
    };
    assert!(!LayerSpec::single(&slice).close_unreached);
    assert!(LayerSpec::single(&SliceSpec::preset("goodsprings").unwrap()).close_unreached);
}
