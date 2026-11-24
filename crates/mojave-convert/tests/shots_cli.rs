//! The camera and walk-through flags: how they parse and what they mean.

use std::path::PathBuf;
use std::process::Command;

use clap::Parser;
use mojave_convert::replay::View;
use mojave_convert::sequence::{
    RUN_TICKS_PER_STEP, Scene, WALK_TICKS_PER_STEP, Walk, WalkthroughArgs, WalkthroughOptions,
    frame_ticks,
};
use mojave_convert::shots::{DEFAULT_MARGIN, ViewArgs};

#[derive(Parser)]
struct Views {
    #[command(flatten)]
    views: ViewArgs,
}

#[derive(Parser)]
struct Walkthrough {
    #[command(flatten)]
    args: WalkthroughArgs,
}

fn views(args: &[&str]) -> Result<ViewArgs, clap::Error> {
    Views::try_parse_from(std::iter::once("t").chain(args.iter().copied())).map(|v| v.views)
}

fn walkthrough(args: &[&str]) -> Result<WalkthroughArgs, clap::Error> {
    let base = ["t", "--engine", "e", "--f2", "f", "--out", "o"];
    Walkthrough::try_parse_from(base.iter().chain(args).copied()).map(|w| w.args)
}

#[test]
fn the_default_is_one_stationary_classic_frame() {
    let framing = views(&[]).unwrap().framing().unwrap();
    assert_eq!(framing.views, [View::Classic]);
    assert!(!framing.follow);
    assert_eq!(framing.margin, DEFAULT_MARGIN);
    assert_eq!(views(&[]).unwrap().dest, None);
}

#[test]
fn follow_camera_alone_restores_the_game_and_wide_views() {
    let framing = views(&["--follow-camera"]).unwrap().framing().unwrap();
    assert_eq!(framing.views, [View::Game, View::Wide]);
    assert!(framing.follow);
    // With a view named, just that view, following.
    let framing = views(&["--follow-camera", "--classic"])
        .unwrap()
        .framing()
        .unwrap();
    assert_eq!(framing.views, [View::Classic]);
}

#[test]
fn views_come_in_a_fixed_order_whatever_the_flags_order() {
    let framing = views(&[
        "--wide",
        "--game-view",
        "--classic",
        "--margin",
        "0.25",
        "--dest",
        "x",
    ])
    .unwrap();
    assert_eq!(framing.dest, Some(PathBuf::from("x")));
    let framing = framing.framing().unwrap();
    assert_eq!(framing.views, [View::Game, View::Classic, View::Wide]);
    assert_eq!(framing.margin, 0.25);
    assert!(!framing.follow);
}

#[test]
fn a_margin_outside_the_frame_is_refused() {
    assert!(views(&["--margin", "0.6"]).unwrap().framing().is_err());
    assert!(views(&["--margin=-0.1"]).unwrap().framing().is_err());
    assert!(views(&["--margin", "0"]).unwrap().framing().is_ok());
    assert!(views(&["--margin", "wide"]).is_err());
}

#[test]
fn a_walkthrough_needs_through_hexes_or_a_scene_but_not_both() {
    assert!(walkthrough(&[]).is_err());
    assert!(walkthrough(&["--through", "100", "--scene", "s.json"]).is_err());
    let args = walkthrough(&["--through", "20905", "--through", "19695", "--jobs", "4"]).unwrap();
    assert_eq!(args.through, [20905, 19695]);
    assert_eq!(args.jobs, 4);
    let options = WalkthroughOptions::from_args(&args, None).unwrap();
    assert_eq!(options.walk, Walk::Through(vec![20905, 19695]));
    assert_eq!(options.framing.views, [View::Classic]);
    assert!(!options.framing.follow);
    assert!(options.run);
    assert_eq!(options.ticks_per_step, RUN_TICKS_PER_STEP);
    assert_eq!(options.every, 4);
    assert_eq!(options.name, "walk");
    assert_eq!(options.dest, PathBuf::from("o").join("walkthrough"));
    assert!(options.gif);
}

#[test]
fn walkthrough_flags_set_view_camera_and_timing() {
    let args = walkthrough(&[
        "--through",
        "1",
        "--follow-camera",
        "--wide",
        "--walk",
        "--every",
        "8",
        "--frames",
        "20",
        "--name",
        "tops-walk",
        "--dest",
        "out/camera",
        "--no-gif",
        "--delay",
        "9",
    ])
    .unwrap();
    let options = WalkthroughOptions::from_args(&args, None).unwrap();
    assert_eq!(options.framing.views, [View::Wide]);
    assert!(options.framing.follow);
    assert!(!options.run);
    assert_eq!(options.ticks_per_step, WALK_TICKS_PER_STEP);
    assert_eq!((options.every, options.frames), (8, Some(20)));
    assert_eq!(options.dest, PathBuf::from("out/camera"));
    assert_eq!(options.name, "tops-walk");
    assert_eq!(options.delay_cs, Some(9));
    assert!(!options.gif);
    let bad = walkthrough(&["--through", "1", "--name", "../x"]).unwrap();
    assert!(WalkthroughOptions::from_args(&bad, None).is_err());
}

#[test]
fn a_showcase_scene_fills_what_the_flags_leave_out() {
    let scene = Scene::parse(
        r#"{"name": "sal-wide", "layer": "ignored", "map": "saloon.map", "view": "wide",
            "run": true, "tps": 8, "slack": 2, "path": [20896, 20905, 19106],
            "start_after": 1, "start_pad": 30, "first": 0, "every": 4, "tail": 30,
            "zoom": 1.0, "gif": {"out": "ignored.gif"}}"#,
    )
    .unwrap();
    let args = walkthrough(&["--scene", "s.json"]).unwrap();
    let options = WalkthroughOptions::from_args(&args, Some(&scene)).unwrap();
    assert_eq!(options.name, "sal-wide");
    assert_eq!(options.map.as_deref(), Some("saloon.map"));
    assert_eq!(options.framing.views, [View::Wide]);
    assert_eq!(options.wide_zoom, Some(1.0));
    assert_eq!(options.walk, Walk::Path(vec![20896, 20905, 19106]));
    assert_eq!(
        (options.ticks_per_step, options.slack, options.tail),
        (8, 2, 30)
    );
    assert_eq!(options.first, None, "first 0 means from the start");
    assert_eq!(options.start_after, Some((1, 30)));
    // The command line wins over the scene.
    let args = walkthrough(&["--scene", "s.json", "--classic", "--every", "2"]).unwrap();
    let options = WalkthroughOptions::from_args(&args, Some(&scene)).unwrap();
    assert_eq!(options.framing.views, [View::Classic]);
    assert_eq!(options.wide_zoom, None, "a scene's zoom is the wide view's");
    assert_eq!(options.every, 2);
    // A scene with nothing to walk.
    let empty = Scene::parse(r#"{"name": "x"}"#).unwrap();
    assert!(WalkthroughOptions::from_args(&args, Some(&empty)).is_err());
    assert!(Scene::parse("[1]").is_err());
    let bad_view = Scene::parse(r#"{"view": "tv", "path": [1, 2]}"#).unwrap();
    let args = walkthrough(&["--scene", "s.json"]).unwrap();
    assert!(WalkthroughOptions::from_args(&args, Some(&bad_view)).is_err());
}

#[test]
fn frames_are_cut_every_few_ticks_up_to_the_end() {
    assert_eq!(frame_ticks(31, 4, 45, None), [31, 35, 39, 43]);
    assert_eq!(frame_ticks(31, 4, 45, Some(2)), [31, 35]);
    assert_eq!(frame_ticks(0, 0, 3, None), [1, 2, 3]);
    assert_eq!(frame_ticks(50, 4, 45, None), [45]);
    assert!(frame_ticks(1, 1, 0, None).is_empty());
}

#[test]
fn the_binary_lists_the_new_flags() {
    let exe = env!("CARGO_BIN_EXE_mojave-convert");
    let help = |sub: &str| {
        let out = Command::new(exe).args([sub, "--help"]).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let shots = help("shots");
    for flag in [
        "--follow-camera",
        "--game-view",
        "--wide",
        "--classic",
        "--margin",
        "--dest",
    ] {
        assert!(shots.contains(flag), "shots --help lacks {flag}");
    }
    let walk = help("walkthrough");
    for flag in [
        "--through",
        "--scene",
        "--every",
        "--frames",
        "--follow-camera",
        "--no-gif",
    ] {
        assert!(walk.contains(flag), "walkthrough --help lacks {flag}");
    }
    // A missing engine is a plain error, not a panic.
    let out = Command::new(exe)
        .args([
            "shots",
            "--engine",
            "/nonexistent",
            "--f2",
            "/nonexistent",
            "--out",
            "/nonexistent",
            "--margin",
            "0.7",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--margin"));
}
