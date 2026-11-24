//! Both games through one front end, on the synthetic installs: the
//! Fallout 3 one (`mir::fixture::fallout3_town`, `HEDR` 0.94) and the New
//! Vegas-like mini town (1.34). The game is decided before extract and
//! recorded in the intermediate directory; the flags for the wrong game
//! are refused with a message naming both.

mod common;

use std::process::Command;

use mojave_convert::{GameVariant, Inputs, Install, SliceSpec, pipeline};

fn fo3_slice(town: &mir::fixture::Fallout3Town) -> SliceSpec {
    SliceSpec {
        name: "miniwaste".to_owned(),
        worldspace: town.slice.worldspace.clone(),
        plugins: town.plugins.clone(),
        x: [*town.slice.x.start(), *town.slice.x.end()],
        y: [*town.slice.y.start(), *town.slice.y.end()],
        interior_depth: town.slice.interior_depth,
        ..SliceSpec::default()
    }
}

#[test]
fn a_fallout3_install_extracts_as_fallout3() {
    let fixture = common::fixture();
    let town = mir::fixture::fallout3_town(&fixture.root.path().join("fo3"));
    let out = fixture.root.path().join("out-fo3");
    let slice = fo3_slice(&town);
    let inputs = Inputs::for_install(
        &Install::any(&town.data_dir, None),
        &fixture.f2,
        &out,
        slice.clone(),
        2,
    )
    .expect("inputs");
    assert_eq!(inputs.game, GameVariant::Fallout3);
    let summary = pipeline::extract(&inputs).expect("extract");
    assert_eq!(summary.game, Some(GameVariant::Fallout3));
    let mir = mir::Mir::open(&inputs.mir_dir()).expect("open");
    assert_eq!(mir.game(), GameVariant::Fallout3);
    let report = std::fs::read_to_string(inputs.mir_dir().join("report.md")).expect("report");
    assert!(report.contains("Fallout 3"), "{report}");

    // The New Vegas flag on this install is refused before anything runs.
    let error = Inputs::for_install(
        &Install::new_vegas(&town.data_dir),
        &fixture.f2,
        &fixture.root.path().join("never"),
        slice,
        1,
    )
    .expect_err("the wrong game")
    .to_string();
    assert!(
        error.starts_with("--fnv ") && error.contains("Fallout 3"),
        "{error}"
    );
    assert!(!fixture.root.path().join("never").exists());
}

#[test]
fn the_binary_takes_the_game_flags() {
    let fixture = common::fixture();
    let town = mir::fixture::fallout3_town(&fixture.root.path().join("fo3"));
    let slice_path = fixture.root.path().join("miniwaste.toml");
    std::fs::write(&slice_path, fo3_slice(&town).to_toml()).expect("slice file");
    let run = |args: &[&str], dir: &std::path::Path, out: &str| {
        Command::new(env!("CARGO_BIN_EXE_mojave-convert"))
            .arg("extract")
            .args(args)
            .arg(dir)
            .arg("--f2")
            .arg(&fixture.f2)
            .arg("--out")
            .arg(fixture.root.path().join(out))
            .arg("--slice")
            .arg(&slice_path)
            .output()
            .expect("the binary runs")
    };
    for (args, out) in [
        (&["--fo3"][..], "a"),
        (&["--game-dir"][..], "b"),
        (&["--game", "fo3", "--game-dir"][..], "c"),
    ] {
        let output = run(args, &town.data_dir, out);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // The wrong game, by alias or by --game, and a mismatched pair.
    for args in [
        &["--fnv"][..],
        &["--game", "fnv", "--game-dir"][..],
        &["--game", "fnv", "--fo3"][..],
    ] {
        let output = run(args, &town.data_dir, "d");
        assert!(!output.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Fallout 3") && stderr.contains("New Vegas"),
            "{args:?}: {stderr}"
        );
    }
    // A New Vegas slice under --fo3 is refused too.
    let nv_slice = fixture.root.path().join("minitown.toml");
    let mut spec = fixture.slice.clone();
    spec.game = Some(GameVariant::FalloutNewVegas);
    std::fs::write(&nv_slice, spec.to_toml()).expect("slice file");
    let output = Command::new(env!("CARGO_BIN_EXE_mojave-convert"))
        .args(["extract", "--fo3"])
        .arg(&fixture.fnv)
        .arg("--f2")
        .arg(&fixture.f2)
        .arg("--out")
        .arg(fixture.root.path().join("e"))
        .arg("--slice")
        .arg(&nv_slice)
        .output()
        .expect("the binary runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--fo3 asks for Fallout 3"), "{stderr}");
}
