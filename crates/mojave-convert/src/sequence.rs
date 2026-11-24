//! Walk-throughs: frame sequences of one walk across a converted map,
//! captured by the engine itself, and the animated GIF made of them.
//!
//! The engine's replay mode writes only the final frame, so a sequence is
//! made by replaying the same deterministic log cut after later and later
//! ticks, one engine process per frame, several at once (`--jobs`). The
//! number of jobs never changes a frame.
//!
//! The walk is a list of hexes (`--through`, routed on the written map from
//! the entry, in hops), or a scene file in the showcase capture format
//! ([`Scene`]). The camera is the same as in `shots`: stationary by
//! default (the character walks across a still room, with a discrete pan
//! where the next hop would leave the frame's inner box), following with
//! `--follow-camera`. A frame is never cut between a re-centre's `Home`
//! and its freeze.
//!
//! Output, under `<dest>/<name>/` (`<dest>` defaults to
//! `<out>/walkthrough`): per view `<view>/fNNNN.png` and
//! `<name>-<view>.gif`, the full replay log `<view>/walk.jsonl`, and
//! `index.json`.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use vault_formats::HexGrid;

use crate::config::ConvertError;
use crate::replay::{
    Hop, LogOptions, Recentre, SETTLE_TICKS, TICK_MS, View, build_log, frame_size, plan_camera,
    run_jobs,
};
use crate::shots::{
    CameraKind, Framing, MOVED_FRACTION, ViewArgs, changed_fraction, framed_hop, io, route_hops,
    setup, stage, view_zoom,
};

/// Ticks a running step takes, measured on engine frames (9 steps in
/// about 74 ticks).
pub const RUN_TICKS_PER_STEP: u64 = 8;
/// The same walking (9 steps in about 210 ticks).
pub const WALK_TICKS_PER_STEP: u64 = 23;
/// Extra ticks per hop.
pub const DEFAULT_SLACK: u64 = 2;
/// Ticks after the walk.
pub const DEFAULT_TAIL: u64 = 30;
/// Ticks between frames: the engine advances a running figure every 4.
pub const DEFAULT_EVERY: u64 = 4;
/// The last frame's extra time in the GIF, in centiseconds.
pub const DEFAULT_HOLD_CS: u16 = 150;
/// The most frames one walk-through captures.
pub const MAX_FRAMES: usize = 2000;

/// A scene file: the capture format of the showcase scripts. Unknown keys
/// are ignored. Either `path` (direct walks between consecutive hexes, the
/// first being where the player starts), `walks` (`[hex, ticks]` pairs) or
/// `through` (hexes routed on the map, as `--through`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Scene {
    pub name: Option<String>,
    pub map: Option<String>,
    /// `classic`, `wide` or `game`.
    pub view: Option<String>,
    pub zoom: Option<f32>,
    pub run: Option<bool>,
    /// Ticks per step.
    pub tps: Option<u64>,
    pub slack: Option<u64>,
    pub settle: Option<u64>,
    pub path: Vec<i32>,
    pub walks: Vec<(i32, u64)>,
    pub through: Vec<i32>,
    /// The first frame's tick.
    pub first: Option<u64>,
    pub every: Option<u64>,
    pub frames: Option<usize>,
    pub tail: Option<u64>,
    /// Start capturing when the walk to `path[start_after]` is due to end,
    /// plus `start_pad` ticks.
    pub start_after: Option<usize>,
    pub start_pad: Option<u64>,
    /// Follow the player (as `--follow-camera`).
    pub follow: Option<bool>,
}

impl Scene {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("scene: {e}"))
    }
}

/// What the player walks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Walk {
    /// Routed on the written map from the entry through each hex in turn.
    Through(Vec<i32>),
    /// Direct walks between consecutive hexes; the first is the start.
    Path(Vec<i32>),
    /// Direct walks from the entry, each with its own ticks.
    Walks(Vec<(i32, u64)>),
}

/// Arguments of `mojave-convert walkthrough`.
#[derive(Debug, Clone, PartialEq, clap::Args)]
pub struct WalkthroughArgs {
    /// A vault-platform executable.
    #[arg(long)]
    pub engine: PathBuf,
    /// The Fallout 2 install directory (holding master.dat and critter.dat).
    #[arg(long)]
    pub f2: PathBuf,
    /// The --out directory of a converted slice (holding mods/mojave).
    #[arg(long)]
    pub out: PathBuf,
    /// The map's file name under maps/ (default: the map stage's map).
    #[arg(long)]
    pub map: Option<String>,
    /// A hex to walk through, in order (repeatable): routed on the written
    /// map from the entry, around blockers and off exit grids. A blocked
    /// hex is replaced by the reachable hex nearest it.
    #[arg(
        long,
        value_name = "HEX",
        required_unless_present = "scene",
        conflicts_with = "scene"
    )]
    pub through: Vec<i32>,
    /// A scene JSON in the showcase capture format (path or walks,
    /// view, zoom, run, tps, slack, first, every, frames, tail).
    #[arg(long, value_name = "FILE")]
    pub scene: Option<PathBuf>,
    /// The walk-through's name (default: the scene's, else "walk").
    #[arg(long)]
    pub name: Option<String>,
    #[command(flatten)]
    pub views: ViewArgs,
    /// The wide view's zoom (default 0.5, or 0.75 on an interior demo).
    #[arg(long)]
    pub wide_zoom: Option<f32>,
    /// Ticks between frames.
    #[arg(long)]
    pub every: Option<u64>,
    /// At most this many frames (default: the whole walk and its tail).
    #[arg(long)]
    pub frames: Option<usize>,
    /// Ticks after the walk.
    #[arg(long)]
    pub tail: Option<u64>,
    /// Walk instead of running.
    #[arg(long)]
    pub walk: bool,
    /// Ticks each step gets (default 8 running, 23 walking).
    #[arg(long)]
    pub ticks_per_step: Option<u64>,
    /// Steps per walk command (shortened so that a hop fits a stationary
    /// frame).
    #[arg(long, default_value_t = 15)]
    pub hop: usize,
    /// Engine processes at once.
    #[arg(long, default_value_t = 3)]
    pub jobs: usize,
    /// Seconds an engine run may take before it is stopped.
    #[arg(long, default_value_t = 600)]
    pub timeout: u64,
    /// Centiseconds per GIF frame (default: the frames' own time, `every`
    /// ticks of 16 ms).
    #[arg(long)]
    pub delay: Option<u16>,
    /// Extra centiseconds on the GIF's last frame.
    #[arg(long, default_value_t = DEFAULT_HOLD_CS)]
    pub hold: u16,
    /// Write the PNG frames only.
    #[arg(long)]
    pub no_gif: bool,
}

/// How a walk-through is captured.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkthroughOptions {
    pub engine: PathBuf,
    pub f2: PathBuf,
    pub out: PathBuf,
    pub map: Option<String>,
    /// The walk-through goes to `<dest>/<name>/`.
    pub dest: PathBuf,
    pub name: String,
    pub walk: Walk,
    pub framing: Framing,
    pub wide_zoom: Option<f32>,
    pub run: bool,
    pub ticks_per_step: u64,
    pub slack: u64,
    pub settle: u64,
    pub tail: u64,
    /// The first frame's tick (default: the first after the settle ticks).
    pub first: Option<u64>,
    /// `(k, pad)`: start at the end of the k-th walk plus `pad` ticks.
    pub start_after: Option<(usize, u64)>,
    pub every: u64,
    pub frames: Option<usize>,
    pub hop: usize,
    pub jobs: usize,
    pub timeout: Duration,
    pub delay_cs: Option<u16>,
    pub hold_cs: u16,
    pub gif: bool,
}

fn parse_view(name: &str) -> Result<View, String> {
    match name {
        "classic" => Ok(View::Classic),
        "wide" => Ok(View::Wide),
        "game" => Ok(View::Game),
        other => Err(format!(
            "scene view {other:?}: expected classic, wide or game"
        )),
    }
}

impl WalkthroughOptions {
    /// The options of `args`, with `scene` (the parsed `--scene` file)
    /// filling what the command line leaves out.
    pub fn from_args(args: &WalkthroughArgs, scene: Option<&Scene>) -> Result<Self, String> {
        let default = Scene::default();
        let scene = scene.unwrap_or(&default);
        let mut framing = args.views.framing()?;
        let named = args.views.game_view || args.views.classic || args.views.wide;
        if !named {
            framing.views = match &scene.view {
                Some(view) => vec![parse_view(view)?],
                None => vec![View::Classic],
            };
        }
        framing.follow |= scene.follow.unwrap_or(false);
        let walk = if !args.through.is_empty() {
            Walk::Through(args.through.clone())
        } else if !scene.through.is_empty() {
            Walk::Through(scene.through.clone())
        } else if scene.path.len() >= 2 {
            Walk::Path(scene.path.clone())
        } else if !scene.walks.is_empty() {
            Walk::Walks(scene.walks.clone())
        } else {
            return Err(
                "nothing to walk: give --through, or a scene with path, walks or through"
                    .to_owned(),
            );
        };
        let run = !args.walk && scene.run.unwrap_or(true);
        let wide_zoom = args
            .wide_zoom
            .or(scene.zoom.filter(|_| framing.views.contains(&View::Wide)));
        if let Some(zoom) = wide_zoom
            && !(zoom.is_finite() && zoom > 0.0)
        {
            return Err(format!("--wide-zoom {zoom}: expected a positive number"));
        }
        let every = args.every.or(scene.every).unwrap_or(DEFAULT_EVERY).max(1);
        let name = args
            .name
            .clone()
            .or_else(|| scene.name.clone())
            .unwrap_or_else(|| "walk".to_owned());
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            || name.starts_with('.')
        {
            return Err(format!(
                "--name {name:?}: letters, digits, '-', '_' and '.' only"
            ));
        }
        Ok(Self {
            engine: args.engine.clone(),
            f2: args.f2.clone(),
            out: args.out.clone(),
            map: args.map.clone().or_else(|| scene.map.clone()),
            dest: args
                .views
                .dest
                .clone()
                .unwrap_or_else(|| args.out.join("walkthrough")),
            name,
            walk,
            framing,
            wide_zoom,
            run,
            ticks_per_step: args.ticks_per_step.or(scene.tps).unwrap_or(if run {
                RUN_TICKS_PER_STEP
            } else {
                WALK_TICKS_PER_STEP
            }),
            slack: scene.slack.unwrap_or(DEFAULT_SLACK),
            settle: scene.settle.unwrap_or(SETTLE_TICKS),
            tail: args.tail.or(scene.tail).unwrap_or(DEFAULT_TAIL),
            first: scene.first.filter(|f| *f > 0),
            start_after: scene.start_after.map(|k| (k, scene.start_pad.unwrap_or(0))),
            every,
            frames: args.frames.or(scene.frames),
            hop: args.hop.max(1),
            jobs: args.jobs,
            timeout: Duration::from_secs(args.timeout),
            delay_cs: args.delay,
            hold_cs: args.hold,
            gif: !args.no_gif,
        })
    }
}

/// The hops of a walk, each with `ticks_per_step * steps + slack` ticks
/// (or its own, for [`Walk::Walks`]).
pub fn walk_hops(
    walk: &Walk,
    map: &crate::route::WalkMap,
    hop: usize,
    ticks_per_step: u64,
    slack: u64,
) -> Result<Vec<Hop>, String> {
    let ticks = |steps: usize| ticks_per_step * steps as u64 + slack;
    let distance = |a: i32, b: i32| {
        HexGrid::lattice_distance(a, b).map_err(|e| format!("hexes {a} and {b}: {e}"))
    };
    let exits: Vec<i32> = map.exits.iter().map(|e| e.hex).collect();
    let mut hops = Vec::new();
    match walk {
        Walk::Through(points) => {
            let mut from = map.entry;
            for &point in points {
                let route = map
                    .route_between(from, point)
                    .ok_or_else(|| format!("no route from hex {from} toward hex {point}"))?;
                if route.destination != point {
                    eprintln!(
                        "walkthrough: hex {point} is not reachable; walking to hex {} instead",
                        route.destination
                    );
                }
                hops.extend(route_hops(&route, hop, &exits, ticks));
                from = route.destination;
            }
        }
        Walk::Path(path) => {
            if path.first() != Some(&map.entry) {
                eprintln!(
                    "walkthrough: the scene's path starts at hex {:?}, the player at the entry {}",
                    path.first(),
                    map.entry
                );
            }
            for pair in path.windows(2) {
                let steps = distance(pair[0], pair[1])? as usize;
                hops.push(Hop {
                    hex: pair[1],
                    steps,
                    ticks: ticks(steps),
                    path: pair.to_vec(),
                });
            }
        }
        Walk::Walks(walks) => {
            let mut from = map.entry;
            for &(hex, after) in walks {
                let steps = distance(from, hex)? as usize;
                hops.push(Hop {
                    hex,
                    steps,
                    ticks: after,
                    path: vec![from, hex],
                });
                from = hex;
            }
        }
    }
    Ok(hops)
}

/// The ticks frames are cut at: from `first` every `every` ticks to
/// `total`, at most `frames` of them.
pub fn frame_ticks(first: u64, every: u64, total: u64, frames: Option<usize>) -> Vec<u64> {
    let first = first.max(1);
    let mut ticks = Vec::new();
    let mut t = first;
    while t <= total && ticks.len() < frames.unwrap_or(MAX_FRAMES).min(MAX_FRAMES) {
        ticks.push(t);
        t += every.max(1);
    }
    if ticks.is_empty() && total >= 1 {
        ticks.push(total);
    }
    ticks
}

/// One view of a walk-through in `index.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SequenceView {
    pub view: &'static str,
    pub view_kind: &'static str,
    pub surface: [u32; 2],
    pub zoom: f32,
    /// The frames' size.
    pub width: u32,
    pub height: u32,
    pub stationary: bool,
    pub camera: &'static str,
    pub camera_centre_hex: Option<i32>,
    pub recentres: Vec<Recentre>,
    /// Per re-centre, the first frame that shows it.
    pub recentre_frames: Vec<usize>,
    /// The tick each frame was cut after.
    pub ticks: Vec<u64>,
    pub frames: usize,
    /// Relative to the walk-through's directory.
    pub frame_dir: String,
    pub png_bytes: u64,
    pub gif: Option<String>,
    pub gif_bytes: u64,
    /// GIF frames after identical neighbours are merged.
    pub gif_frames: usize,
    pub gif_colours: usize,
    pub gif_colours_merged: usize,
    pub delay_cs: u16,
    /// The largest fraction of pixels changed between two consecutive
    /// frames, and the frames (index of the later) where more than
    /// [`MOVED_FRACTION`] changed: the camera moved.
    pub max_change: f64,
    pub camera_moves: Vec<usize>,
    /// The largest change between two consecutive frames where the camera
    /// did not move: the people and the player alone.
    pub max_still_change: f64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct HopRecord {
    hex: i32,
    steps: usize,
    ticks: u64,
}

#[derive(Debug, Clone, Serialize)]
struct SequenceIndex<'a> {
    name: &'a str,
    map: &'a str,
    entry_hex: i32,
    run: bool,
    settle: u64,
    every: u64,
    hops: Vec<HopRecord>,
    views: &'a [SequenceView],
}

/// What [`walkthrough`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkthroughSummary {
    pub dir: PathBuf,
    pub map: String,
    pub hops: usize,
    pub views: Vec<SequenceView>,
}

/// Captures a walk-through: see the module documentation.
pub fn walkthrough(options: &WalkthroughOptions) -> Result<WalkthroughSummary, ConvertError> {
    let setup = setup(
        &options.engine,
        &options.f2,
        &options.out,
        options.map.as_deref(),
        options.timeout,
    )?;
    let interior = crate::map::MapFeatures::read(&options.out)
        .is_ok_and(|f| f.interior.is_some() && f.map_file.trim_start_matches("maps/") == setup.map);
    let wide_zoom = options.wide_zoom.unwrap_or(if interior {
        crate::shots::INTERIOR_WIDE_ZOOM
    } else {
        crate::shots::DEFAULT_WIDE_ZOOM
    });
    let hop = framed_hop(options.hop, &options.framing, wide_zoom);
    let hops = walk_hops(
        &options.walk,
        &setup.walk_map,
        hop,
        options.ticks_per_step,
        options.slack,
    )
    .map_err(stage)?;
    if hops.is_empty() {
        return Err(stage(
            "the walk goes nowhere: every hex given is where the player is",
        ));
    }
    let dir = options.dest.join(&options.name);
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let walked: u64 = options.settle + hops.iter().map(|h| h.ticks).sum::<u64>();
    let first = match (options.first, options.start_after) {
        (Some(first), _) => first,
        (None, Some((k, pad))) => {
            options.settle + hops.iter().take(k).map(|h| h.ticks).sum::<u64>() + pad
        }
        (None, None) => options.settle + 1,
    };
    eprintln!(
        "walkthrough: {} hops, {} steps, {} ticks of walking from hex {}",
        hops.len(),
        hops.iter().map(|h| h.steps).sum::<usize>(),
        walked - options.settle,
        setup.walk_map.entry
    );

    let mut views = Vec::new();
    for &view in &options.framing.views {
        let zoom = view_zoom(view, wide_zoom);
        let camera = options.framing.camera(view);
        let plan = (camera == CameraKind::Stationary).then(|| {
            plan_camera(
                setup.walk_map.entry,
                &hops,
                frame_size(view, zoom),
                options.framing.margin,
            )
        });
        let log_options = LogOptions {
            view,
            zoom,
            run: options.run,
            settle: options.settle,
            tail: options.tail,
        };
        let log = build_log(&log_options, &hops, plan.as_ref());
        let total = log.ticks_total() as u64;
        // Frames are never cut where the camera is about to move.
        let mut ticks: Vec<u64> = Vec::new();
        for t in frame_ticks(first, options.every, total, options.frames) {
            if let Some(t) = log.next_settled(t as usize).map(|t| t as u64)
                && ticks.last() != Some(&t)
            {
                ticks.push(t);
            }
        }
        if ticks.is_empty() {
            return Err(stage(
                "no frame to capture: the walk is shorter than its first tick",
            ));
        }
        let view_dir = dir.join(view.name());
        let work = view_dir.join("work");
        std::fs::create_dir_all(&work).map_err(io(&work))?;
        let full_log = view_dir.join("walk.jsonl");
        std::fs::write(&full_log, log.text()).map_err(io(&full_log))?;
        let jobs: Vec<crate::replay::EngineJob> = ticks
            .iter()
            .enumerate()
            .map(|(k, t)| crate::replay::EngineJob {
                stem: format!("f{k:04}"),
                log: log.cut(*t as usize).0,
            })
            .collect();
        eprintln!(
            "walkthrough: {} {} frames, every {} ticks from tick {}",
            view.name(),
            jobs.len(),
            options.every,
            ticks[0]
        );
        let results = run_jobs(
            &setup.engine,
            &jobs,
            &work,
            options.jobs,
            true,
            "walkthrough",
        );
        let mut errors = Vec::new();
        let mut frames = Vec::new();
        let mut png_bytes = 0u64;
        for (k, result) in results.into_iter().enumerate() {
            match result {
                Ok(frame) => {
                    let path = view_dir.join(format!("f{k:04}.png"));
                    let png =
                        foundry_gallery::encode_png(&frame.to_rgba(), frame.width, frame.height)
                            .map_err(stage)?;
                    png_bytes += png.len() as u64;
                    std::fs::write(&path, png).map_err(io(&path))?;
                    frames.push((k, frame));
                }
                Err(e) => errors.push(format!("frame {k}: {e}")),
            }
        }
        if frames.is_empty() {
            return Err(stage(format!(
                "the engine captured no frame of the {} view: {}",
                view.name(),
                errors.first().map_or("", String::as_str)
            )));
        }
        let _ = std::fs::remove_dir(&work);
        let changes: Vec<f64> = frames
            .windows(2)
            .map(|w| changed_fraction(&w[0].1, &w[1].1))
            .collect();
        let camera_moves: Vec<usize> = changes
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > MOVED_FRACTION)
            .map(|(i, _)| frames[i + 1].0)
            .collect();
        let max_change = changes.iter().copied().fold(0.0, f64::max);
        let max_still_change = changes
            .iter()
            .copied()
            .filter(|c| *c <= MOVED_FRACTION)
            .fold(0.0, f64::max);
        let delay_cs = options.delay_cs.unwrap_or_else(|| {
            ((options.every * TICK_MS) as f64 / 10.0)
                .round()
                .clamp(2.0, 65535.0) as u16
        });
        let (gif, gif_stats) = if options.gif {
            let refs: Vec<&crate::shots::Frame> = frames.iter().map(|(_, f)| f).collect();
            let mut delays = vec![delay_cs; refs.len()];
            if let Some(last) = delays.last_mut() {
                *last = last.saturating_add(options.hold_cs);
            }
            let (bytes, stats) = gif::encode(&refs, &delays, 0).map_err(stage)?;
            let name = format!("{}-{}.gif", options.name, view.name());
            let path = dir.join(&name);
            std::fs::write(&path, &bytes).map_err(io(&path))?;
            (Some(name), stats)
        } else {
            (None, gif::GifStats::default())
        };
        let recentre_frames = log
            .recentre_ticks()
            .iter()
            .filter_map(|t| ticks.iter().position(|f| *f >= *t as u64))
            .collect();
        let round = |x: f64| (x * 1e4).round() / 1e4;
        views.push(SequenceView {
            view: view.name(),
            view_kind: view.kind(),
            surface: view.surface(),
            zoom,
            width: frames[0].1.width,
            height: frames[0].1.height,
            stationary: camera == CameraKind::Stationary,
            camera: camera.label(),
            camera_centre_hex: plan.as_ref().and_then(|p| p.centre_hex),
            recentres: plan.map(|p| p.recentres).unwrap_or_default(),
            recentre_frames,
            ticks,
            frames: frames.len(),
            frame_dir: view.name().to_owned(),
            png_bytes,
            gif,
            gif_bytes: gif_stats.bytes as u64,
            gif_frames: gif_stats.frames_written,
            gif_colours: gif_stats.colours,
            gif_colours_merged: gif_stats.merged,
            delay_cs,
            max_change: round(max_change),
            camera_moves,
            max_still_change: round(max_still_change),
            errors,
        });
    }
    let index = SequenceIndex {
        name: &options.name,
        map: &setup.map,
        entry_hex: setup.walk_map.entry,
        run: options.run,
        settle: options.settle,
        every: options.every,
        hops: hops
            .iter()
            .map(|h| HopRecord {
                hex: h.hex,
                steps: h.steps,
                ticks: h.ticks,
            })
            .collect(),
        views: &views,
    };
    let index_path = dir.join("index.json");
    let json = serde_json::to_string_pretty(&index).map_err(stage)?;
    std::fs::write(&index_path, json + "\n").map_err(io(&index_path))?;
    Ok(WalkthroughSummary {
        dir,
        map: setup.map,
        hops: hops.len(),
        views,
    })
}

/// `mojave-convert walkthrough`: reads the scene, captures, prints a
/// line per view.
pub fn walkthrough_cli(args: &WalkthroughArgs) -> Result<(), String> {
    let scene = match &args.scene {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("--scene {}: {e}", path.display()))?;
            Some(Scene::parse(&text)?)
        }
        None => None,
    };
    let options = WalkthroughOptions::from_args(args, scene.as_ref())?;
    let summary = walkthrough(&options).map_err(|e| e.to_string())?;
    for view in &summary.views {
        println!(
            "{:<7} {:>4} frames {}x{} camera {} ({} re-centres) png {:.2} MB gif {} {:.2} MB ({} frames, {} colours){}",
            view.view,
            view.frames,
            view.width,
            view.height,
            view.camera,
            view.recentres.len(),
            view.png_bytes as f64 / 1e6,
            view.gif.as_deref().unwrap_or("-"),
            view.gif_bytes as f64 / 1e6,
            view.gif_frames,
            view.gif_colours,
            if view.errors.is_empty() {
                String::new()
            } else {
                format!("; {} frames failed", view.errors.len())
            }
        );
    }
    println!(
        "walkthrough: {} hops on {}; index {}",
        summary.hops,
        summary.map,
        summary.dir.join("index.json").display()
    );
    Ok(())
}

/// An animated GIF89a encoder: one global palette of the frames' most
/// used colours, LZW written here, and per frame only the rectangle that
/// changed since the frame before (unchanged pixels inside it transparent
/// when that helps). Frames identical to the one before are merged into it
/// by adding their time.
pub mod gif {
    use std::collections::HashMap;

    use serde::Serialize;

    use crate::shots::Frame;

    /// The palette index left for transparency.
    pub const TRANSPARENT: u8 = 255;
    /// Colours kept in the palette; others map to the nearest kept one.
    pub const MAX_COLOURS: usize = 255;
    /// Largest GIF dimension.
    const MAX_SIDE: u32 = 65_535;

    /// What [`encode`] wrote.
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
    pub struct GifStats {
        pub frames_in: usize,
        /// After merging identical neighbours.
        pub frames_written: usize,
        /// Distinct colours in the frames.
        pub colours: usize,
        /// Of those, how many were mapped to a nearby kept colour.
        pub merged: usize,
        pub bytes: usize,
        /// Pixels written in all frame rectangles.
        pub rect_pixels: u64,
    }

    fn key(p: &[u8; 3]) -> u32 {
        u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])
    }

    /// The palette over all frames: the [`MAX_COLOURS`] most used colours
    /// (ties by colour value), the rest mapped to the nearest of them.
    pub struct Palette {
        pub colours: Vec<[u8; 3]>,
        map: HashMap<u32, u8>,
        pub distinct: usize,
        pub merged: usize,
    }

    impl Palette {
        pub fn build(frames: &[&Frame]) -> Self {
            let mut counts: HashMap<u32, u64> = HashMap::new();
            for frame in frames {
                for p in frame.rgb.as_chunks::<3>().0 {
                    *counts.entry(key(p)).or_default() += 1;
                }
            }
            let mut ranked: Vec<(u32, u64)> = counts.into_iter().collect();
            ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let colours: Vec<[u8; 3]> = ranked
                .iter()
                .take(MAX_COLOURS)
                .map(|(k, _)| [(k >> 16) as u8, (k >> 8) as u8, *k as u8])
                .collect();
            let mut map: HashMap<u32, u8> = ranked
                .iter()
                .take(MAX_COLOURS)
                .enumerate()
                .map(|(i, (k, _))| (*k, i as u8))
                .collect();
            for (k, _) in ranked.iter().skip(MAX_COLOURS) {
                let c = [(k >> 16) as i32, ((k >> 8) & 255) as i32, (k & 255) as i32];
                let nearest = colours
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, p)| {
                        let d = [0, 1, 2].map(|i| i32::from(p[i]) - c[i]);
                        3 * d[0] * d[0] + 4 * d[1] * d[1] + 2 * d[2] * d[2]
                    })
                    .map_or(0, |(i, _)| i as u8);
                map.insert(*k, nearest);
            }
            Self {
                colours,
                map,
                distinct: ranked.len(),
                merged: ranked.len().saturating_sub(MAX_COLOURS),
            }
        }

        pub fn indices(&self, frame: &Frame) -> Vec<u8> {
            frame
                .rgb
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| self.map.get(&key(p)).copied().unwrap_or(0))
                .collect()
        }
    }

    struct Bits {
        out: Vec<u8>,
        acc: u32,
        n: u32,
    }

    impl Bits {
        fn put(&mut self, code: u16, size: u8) {
            self.acc |= u32::from(code) << self.n;
            self.n += u32::from(size);
            while self.n >= 8 {
                self.out.push(self.acc as u8);
                self.acc >>= 8;
                self.n -= 8;
            }
        }

        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.out.push(self.acc as u8);
            }
            self.out
        }
    }

    /// GIF's variable-length LZW of 8-bit `indices` (minimum code size 8):
    /// a clear code first, codes growing from 9 to 12 bits, a clear code
    /// when the table is full, the end code last, packed least significant
    /// bit first.
    pub fn lzw(indices: &[u8]) -> Vec<u8> {
        const MIN: u8 = 8;
        let clear: u16 = 1 << MIN;
        let end = clear + 1;
        let mut bits = Bits {
            out: Vec::with_capacity(indices.len() / 2 + 16),
            acc: 0,
            n: 0,
        };
        let mut size = MIN + 1;
        let mut next = end + 1;
        // (prefix code << 8 | byte) -> code; 0 is "none" (no code is 0
        // after the 258 fixed ones).
        let mut table = vec![0u16; 4096 << 8];
        let mut used: Vec<usize> = Vec::new();
        bits.put(clear, size);
        let Some((&first, rest)) = indices.split_first() else {
            bits.put(end, size);
            return bits.finish();
        };
        let mut prefix = u16::from(first);
        for &byte in rest {
            let k = usize::from(prefix) << 8 | usize::from(byte);
            let code = table[k];
            if code != 0 {
                prefix = code;
                continue;
            }
            bits.put(prefix, size);
            if next == 1 << size && size < 12 {
                size += 1;
            }
            if next >= 4095 {
                bits.put(clear, size);
                for k in used.drain(..) {
                    table[k] = 0;
                }
                size = MIN + 1;
                next = end + 1;
            } else {
                table[k] = next;
                used.push(k);
                next += 1;
            }
            prefix = u16::from(byte);
        }
        bits.put(prefix, size);
        if next == 1 << size && size < 12 {
            size += 1;
        }
        bits.put(end, size);
        bits.finish()
    }

    fn sub_blocks(out: &mut Vec<u8>, data: &[u8]) {
        for chunk in data.chunks(255) {
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
        out.push(0);
    }

    /// The bounding rectangle `[x0, y0, x1, y1)` of the pixels where `a`
    /// and `b` (rows of `width`) differ, or `None` when they are equal.
    pub fn changed_rect(a: &[u8], b: &[u8], width: usize) -> Option<[usize; 4]> {
        let mut rect: Option<[usize; 4]> = None;
        for (y, (ra, rb)) in a.chunks(width).zip(b.chunks(width)).enumerate() {
            if ra == rb {
                continue;
            }
            let left = ra.iter().zip(rb).position(|(p, q)| p != q).unwrap_or(0);
            let right = width
                - ra.iter()
                    .rev()
                    .zip(rb.iter().rev())
                    .position(|(p, q)| p != q)
                    .unwrap_or(0);
            rect = Some(match rect {
                None => [left, y, right, y + 1],
                Some([x0, y0, x1, _]) => [x0.min(left), y0, x1.max(right), y + 1],
            });
        }
        rect
    }

    struct Pending {
        rect: [usize; 4],
        transparent: bool,
        data: Vec<u8>,
        delay: u32,
    }

    /// Encodes `frames` (all of one size) as a looping animated GIF
    /// (`loops` 0: for ever), frame `i` shown for `delays_cs[i]`
    /// centiseconds.
    pub fn encode(
        frames: &[&Frame],
        delays_cs: &[u16],
        loops: u16,
    ) -> Result<(Vec<u8>, GifStats), String> {
        let first = frames.first().ok_or("no frames to encode")?;
        let (width, height) = (first.width, first.height);
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(format!("a GIF cannot be {width} x {height}"));
        }
        if delays_cs.len() != frames.len() {
            return Err(format!(
                "{} delays for {} frames",
                delays_cs.len(),
                frames.len()
            ));
        }
        for (i, f) in frames.iter().enumerate() {
            if (f.width, f.height) != (width, height)
                || f.rgb.len() != width as usize * height as usize * 3
            {
                return Err(format!(
                    "frame {i} is {} x {}, not {width} x {height}",
                    f.width, f.height
                ));
            }
        }
        let palette = Palette::build(frames);
        let w = width as usize;
        let mut pending: Vec<Pending> = Vec::new();
        let mut prev: Option<Vec<u8>> = None;
        for (frame, delay) in frames.iter().zip(delays_cs) {
            let idx = palette.indices(frame);
            let delay = u32::from(*delay);
            match &prev {
                None => pending.push(Pending {
                    rect: [0, 0, w, height as usize],
                    transparent: false,
                    data: idx.clone(),
                    delay,
                }),
                Some(before) => match changed_rect(&idx, before, w) {
                    None => {
                        if let Some(last) = pending.last_mut() {
                            last.delay += delay;
                        }
                    }
                    Some([x0, y0, x1, y1]) => {
                        let mut sub = Vec::with_capacity((x1 - x0) * (y1 - y0));
                        let mut same = 0usize;
                        for y in y0..y1 {
                            let row = &idx[y * w + x0..y * w + x1];
                            let old = &before[y * w + x0..y * w + x1];
                            same += row.iter().zip(old).filter(|(a, b)| a == b).count();
                            sub.extend_from_slice(row);
                        }
                        let transparent = same * 10 > sub.len() * 3;
                        if transparent {
                            let mut at = 0;
                            for y in y0..y1 {
                                for old in &before[y * w + x0..y * w + x1] {
                                    if sub[at] == *old {
                                        sub[at] = TRANSPARENT;
                                    }
                                    at += 1;
                                }
                            }
                        }
                        pending.push(Pending {
                            rect: [x0, y0, x1, y1],
                            transparent,
                            data: sub,
                            delay,
                        });
                    }
                },
            }
            prev = Some(idx);
        }

        let mut out = b"GIF89a".to_vec();
        out.extend_from_slice(&(width as u16).to_le_bytes());
        out.extend_from_slice(&(height as u16).to_le_bytes());
        // A global table of 256 colours, 8 bits of colour resolution.
        out.extend_from_slice(&[0xF7, 0, 0]);
        for i in 0..256 {
            out.extend_from_slice(&palette.colours.get(i).copied().unwrap_or([0, 0, 0]));
        }
        out.extend_from_slice(b"\x21\xFF\x0BNETSCAPE2.0\x03\x01");
        out.extend_from_slice(&loops.to_le_bytes());
        out.push(0);
        let mut rect_pixels = 0u64;
        for frame in &pending {
            let [x0, y0, x1, y1] = frame.rect;
            // Disposal 1 (leave in place), and the transparency flag.
            let flags = (1u8 << 2) | u8::from(frame.transparent);
            out.extend_from_slice(&[0x21, 0xF9, 0x04, flags]);
            out.extend_from_slice(&(frame.delay.min(65_535) as u16).to_le_bytes());
            out.extend_from_slice(&[TRANSPARENT, 0]);
            out.push(0x2C);
            for v in [x0, y0, x1 - x0, y1 - y0] {
                out.extend_from_slice(&(v as u16).to_le_bytes());
            }
            out.push(0);
            out.push(8);
            sub_blocks(&mut out, &lzw(&frame.data));
            rect_pixels += frame.data.len() as u64;
        }
        out.push(0x3B);
        let stats = GifStats {
            frames_in: frames.len(),
            frames_written: pending.len(),
            colours: palette.distinct,
            merged: palette.merged,
            bytes: out.len(),
            rect_pixels,
        };
        Ok((out, stats))
    }
}
