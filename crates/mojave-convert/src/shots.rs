//! The shots stage: screenshots of a converted map, taken by the engine
//! itself, headless, through its replay mode.
//!
//! For each viewpoint the player is walked from the map's entry along a
//! route of the written map ([`crate::route`]), in hops short enough for the
//! engine's capped route search, and the engine captures the frame at the
//! end. The camera follows the player. Two views per viewpoint: the
//! 640 × 480 game view with the interface, and the unlocked widescreen view
//! at 1920 × 1080, zoom 0.5 ([`DEFAULT_WIDE_ZOOM`]; 0.75 on an interior
//! demo, [`INTERIOR_WIDE_ZOOM`]). The engine draws the wide view at its own
//! pixel scale whatever the zoom: zoom 0.5 captures 1707 × 760 pixels of
//! map, 0.75 about 1138 × 507, 1.0 854 × 380 (a saloon fits 0.75).
//!
//! On an interior demo (`map-features.json` names the room) `--auto`
//! aims at the entrance, then at the people placed, those with a name of
//! their own first, then at the largest walls, and the viewpoints may lie
//! [`INTERIOR_SPREAD`] steps apart.
//!
//! Output, under `<out>/shots/`: `<map>-NN-<view>.png`, `index.json`, and
//! `contact.png`. The replay logs, raw frames and engine logs are kept in
//! `work/` for inspection.
//!
//! The engine reports no player position, so arrival is judged by frames
//! of the wide view, whose camera follows the player to the map's edges
//! (the game view's stops short of them): a viewpoint's frame is compared
//! with the entry's (did the player move at all) and with the same walk
//! less its last hop (did the last hop move). Both must differ in more
//! than [`MOVED_FRACTION`] of the pixels.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use foundry_gallery::{Anchor, Background, Columns, SheetOptions, Tile, contact_sheet};
use serde::Serialize;
use vault_formats::HexGrid;

use crate::config::ConvertError;
use crate::map::{Feature, MapFeatures};
use crate::route::{self, Route, WalkMap};

/// Milliseconds per engine tick in a replay log.
pub const TICK_MS: u64 = 16;
/// Ticks before the first walk: the map settles.
const SETTLE_TICKS: u64 = 30;
/// Ticks after the last hop, before the frame is captured.
const FINAL_TICKS: u64 = 40;
/// A viewpoint counts as reached when its frame differs from the
/// comparison frame in more than this fraction of the pixels: a camera
/// that scrolled changes nearly all of them, an idling townsperson a few.
pub const MOVED_FRACTION: f64 = 0.10;

fn stage(error: impl std::fmt::Display) -> ConvertError {
    ConvertError::stage("shots", error)
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ConvertError + '_ {
    move |source| ConvertError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Ticks a hop of `steps` hexes gets: running, a step takes about 16
/// ticks; the rest is slack for the route search and the turn.
pub fn hop_ticks(steps: usize) -> u64 {
    40 + 18 * steps as u64
}

/// One of the two views a viewpoint is captured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum View {
    /// The 640 × 480 game view with the interface.
    Game,
    /// The unlocked widescreen view at 1920 × 1080, zoomed.
    Wide,
}

impl View {
    pub const ALL: [View; 2] = [View::Game, View::Wide];

    pub fn name(self) -> &'static str {
        match self {
            View::Game => "game",
            View::Wide => "wide",
        }
    }

    fn surface(self) -> (u32, u32) {
        match self {
            View::Game => (640, 480),
            View::Wide => (1920, 1080),
        }
    }
}

/// The wide view's zoom on an exterior map.
pub const DEFAULT_WIDE_ZOOM: f32 = 0.5;
/// The wide view's zoom on an interior demo: a frame about 1138 × 507,
/// which a small room fills.
pub const INTERIOR_WIDE_ZOOM: f32 = 0.75;

/// A replay input log: the surface, for the wide view the enhanced
/// settings (at `zoom`), ticks to settle, then per hop a walk to its
/// waypoint and the ticks to get there, then ticks before the capture.
/// `hops` holds each waypoint and the steps it is from the one before.
pub fn replay_log(view: View, hops: &[(i32, usize)], zoom: f32) -> String {
    let mut out = String::new();
    let mut id = 0u64;
    let mut now = 0u64;
    let mut line = |out: &mut String, body: String| {
        id += 1;
        out.push_str(&format!("{{\"command_id\":{id},{body}}}\n"));
    };
    let (width, height) = view.surface();
    line(
        &mut out,
        format!("\"kind\":\"surface\",\"width\":{width},\"height\":{height}"),
    );
    if view == View::Wide {
        line(
            &mut out,
            format!(
                "\"kind\":\"enhanced\",\"settings\":{{\"viewport_mode\":\"unlocked_widescreen\",\
                 \"default_zoom\":{zoom:?},\"smooth_camera\":false,\"roof_peek\":\"hidden\",\
                 \"ui_scale\":null}}"
            ),
        );
    }
    let mut ticks = |out: &mut String, n: u64, line: &mut dyn FnMut(&mut String, String)| {
        for _ in 0..n {
            now += TICK_MS;
            line(out, format!("\"kind\":\"tick\",\"now_ms\":{now}"));
        }
    };
    ticks(&mut out, SETTLE_TICKS, &mut line);
    for &(hex, steps) in hops {
        line(
            &mut out,
            format!("\"kind\":\"bot\",\"action\":{{\"op\":\"walk\",\"hex\":{hex},\"run\":true}}"),
        );
        ticks(&mut out, hop_ticks(steps), &mut line);
    }
    ticks(&mut out, FINAL_TICKS, &mut line);
    out
}

/// The hops of a route: waypoints at most `hop` steps apart, closer near
/// the exit grids in `exits` so the engine's own route between two never
/// steps on one ([`route::safe_waypoints`]), each with the steps from the
/// one before.
pub fn hops(route: &Route, hop: usize, exits: &[i32]) -> Vec<(i32, usize)> {
    let points = route::safe_waypoints(&route.path, hop, exits);
    let mut at = 0usize;
    points
        .into_iter()
        .map(|hex| {
            let index = route.path.iter().position(|h| *h == hex).unwrap_or(at);
            let steps = index.saturating_sub(at);
            at = index;
            (hex, steps)
        })
        .collect()
}

/// An RGB frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 3` bytes.
    pub rgb: Vec<u8>,
}

impl Frame {
    pub fn to_rgba(&self) -> Vec<u8> {
        self.rgb
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect()
    }
}

/// Decodes a binary PPM (`P6`): whitespace-separated width, height and
/// maximum value, `#` comments allowed between them, one whitespace byte,
/// then the samples. A maximum below 255 is scaled up; above 255 (two
/// bytes per sample) is refused. Sizes are checked before allocating.
pub fn decode_ppm(bytes: &[u8]) -> Result<Frame, String> {
    if bytes.len() < 2 || &bytes[..2] != b"P6" {
        return Err("not a binary PPM (no P6 magic)".to_owned());
    }
    let mut at = 2usize;
    let mut fields = [0u64; 3];
    for (n, field) in fields.iter_mut().enumerate() {
        loop {
            match bytes.get(at) {
                Some(b'#') => {
                    while bytes.get(at).is_some_and(|b| *b != b'\n') {
                        at += 1;
                    }
                }
                Some(b) if b.is_ascii_whitespace() => at += 1,
                _ => break,
            }
        }
        let start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
            if at - start > 9 {
                return Err(format!("header field {n} at offset {start} is too long"));
            }
        }
        if at == start {
            return Err(format!("header field {n}: no number at offset {start}"));
        }
        *field = std::str::from_utf8(&bytes[start..at])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("header field {n} at offset {start} is not a number"))?;
    }
    if !bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        return Err(format!("no whitespace after the header at offset {at}"));
    }
    at += 1;
    let [width, height, max] = fields;
    if width == 0 || height == 0 {
        return Err(format!("empty image {width} x {height}"));
    }
    if width > u64::from(foundry_gallery::MAX_DIMENSION)
        || height > u64::from(foundry_gallery::MAX_DIMENSION)
        || width * height > foundry_gallery::MAX_PIXELS
    {
        return Err(format!("image {width} x {height} is too large"));
    }
    if max == 0 || max > 255 {
        return Err(format!("maximum sample {max} is not in 1 to 255"));
    }
    let len = (width * height * 3) as usize;
    let samples = bytes.get(at..at + len).ok_or_else(|| {
        format!(
            "{len} sample bytes expected at offset {at}, {} there",
            bytes.len() - at
        )
    })?;
    let rgb = if max == 255 {
        samples.to_vec()
    } else {
        samples
            .iter()
            .map(|s| (u64::from((*s).min(max as u8)) * 255 / max) as u8)
            .collect()
    };
    Ok(Frame {
        width: width as u32,
        height: height as u32,
        rgb,
    })
}

/// The fraction of pixels that differ between two frames; 1 when their
/// sizes differ.
pub fn changed_fraction(a: &Frame, b: &Frame) -> f64 {
    if a.width != b.width || a.height != b.height || a.rgb.len() != b.rgb.len() {
        return 1.0;
    }
    let pixels = a.rgb.len() / 3;
    if pixels == 0 {
        return 0.0;
    }
    let changed = a
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .zip(b.rgb.as_chunks::<3>().0)
        .filter(|(x, y)| x != y)
        .count();
    changed as f64 / pixels as f64
}

/// A place to stand and photograph from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewpoint {
    /// `entry`, `at <hex>`, or the feature's kind and model.
    pub label: String,
    pub route: Route,
}

/// The last segment of a model path, without its extension.
fn short_model(model: &str) -> &str {
    let name = model.rsplit(['\\', '/']).next().unwrap_or(model);
    name.strip_suffix(".nif").unwrap_or(name)
}

/// Automatic viewpoints are first sought at least this many hexes from the
/// grid's border: nearer, the game view's camera may stop short of the
/// player. On a real map it followed at rows 61 to 63 and stopped for
/// viewpoints at rows 1 to 36; the wide view followed everywhere.
pub const EDGE_MARGIN: i32 = 40;

/// The least hex steps between a person's viewpoint (a feature of kind
/// `person`) and every other.
pub const PERSON_SPREAD: i32 = 1;

/// The default least hex steps between two automatic viewpoints.
pub const DEFAULT_SPREAD: i32 = 30;
/// The same on an interior demo: a room is a few dozen hexes across.
pub const INTERIOR_SPREAD: i32 = 5;

fn inside_margin(hex: i32) -> bool {
    HexGrid::coordinates(hex).is_ok_and(|[c, r]| {
        (EDGE_MARGIN..200 - EDGE_MARGIN).contains(&c)
            && (EDGE_MARGIN..200 - EDGE_MARGIN).contains(&r)
    })
}

/// The viewpoints of a run: the entry; each `at` hex in order; then, while
/// fewer than `auto` have been picked in all, the hexes reachable nearest
/// the largest features (largest first), each at least `spread` hex steps
/// from every viewpoint already picked ([`PERSON_SPREAD`] for a person).
/// Features whose viewpoint lies
/// within [`EDGE_MARGIN`] of the border are taken only when the others run
/// out, and a model already picked only when every other model has been
/// tried.
pub fn pick_viewpoints(
    map: &WalkMap,
    features: &[Feature],
    at: &[i32],
    auto: usize,
    spread: i32,
) -> Vec<Viewpoint> {
    let mut picked = vec![Viewpoint {
        label: "entry".to_owned(),
        route: Route {
            requested: map.entry,
            destination: map.entry,
            path: vec![map.entry],
        },
    }];
    for &hex in at {
        if let Some(route) = map.route_to(hex) {
            picked.push(Viewpoint {
                label: format!("at {hex}"),
                route,
            });
        }
    }
    let mut auto_count = 1usize;
    let mut tried: BTreeSet<i32> = BTreeSet::new();
    let routes: Vec<(&Feature, Route)> = features
        .iter()
        .filter(|f| tried.insert(f.hex))
        .filter_map(|f| map.route_to(f.hex).map(|route| (f, route)))
        .collect();
    let inner = routes.iter().filter(|(_, r)| inside_margin(r.destination));
    let outer = routes.iter().filter(|(_, r)| !inside_margin(r.destination));
    let ordered: Vec<&(&Feature, Route)> = inner.chain(outer).collect();
    // A model already shot is taken again only when every other model has
    // had its turn: the second crate of a row is not a view of the map.
    let mut models: BTreeSet<&str> = BTreeSet::new();
    for repeat in [false, true] {
        for (feature, route) in &ordered {
            if auto_count >= auto {
                break;
            }
            if !repeat && models.contains(feature.model.as_str()) {
                continue;
            }
            // People stand close together (at a bar): each needs only
            // its own viewpoint.
            let least = if feature.kind == "person" {
                spread.min(PERSON_SPREAD)
            } else {
                spread
            };
            let far_enough = picked.iter().all(|v| {
                HexGrid::lattice_distance(v.route.destination, route.destination)
                    .is_ok_and(|d| d >= least)
            });
            if !far_enough {
                continue;
            }
            models.insert(feature.model.as_str());
            picked.push(Viewpoint {
                label: format!("{} {}", feature.kind, short_model(&feature.model)),
                route: route.clone(),
            });
            auto_count += 1;
        }
    }
    picked
}

/// How `shots` is run.
#[derive(Debug, Clone)]
pub struct ShotsOptions {
    /// A `vault-platform` executable.
    pub engine: PathBuf,
    /// The Fallout 2 install (holds master.dat and critter.dat).
    pub f2: PathBuf,
    /// The `--out` directory of a converted slice (holds mods/mojave).
    pub out: PathBuf,
    /// The map under `maps/`; default the exterior map of the last map
    /// stage run, else the first the layer registers.
    pub map: Option<String>,
    pub at: Vec<i32>,
    pub auto: usize,
    /// Steps per walk command.
    pub hop: usize,
    /// Least hex steps between two automatic viewpoints; default
    /// [`DEFAULT_SPREAD`], or [`INTERIOR_SPREAD`] on an interior demo.
    pub spread: Option<i32>,
    /// The wide view's zoom; default [`DEFAULT_WIDE_ZOOM`], or
    /// [`INTERIOR_WIDE_ZOOM`] on an interior demo.
    pub wide_zoom: Option<f32>,
    /// Engine processes at once.
    pub jobs: usize,
    /// Longest an engine run may take before it is stopped.
    pub timeout: Duration,
}

/// One PNG of `index.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Shot {
    pub viewpoint: usize,
    pub label: String,
    pub requested_hex: i32,
    /// Where the route ends.
    pub hex: i32,
    pub steps: usize,
    pub hops: usize,
    pub view: &'static str,
    /// Relative to the shots directory; empty when the engine gave no frame.
    pub file: String,
    pub width: u32,
    pub height: u32,
    /// The wide view differs from the entry's (false for a viewpoint that
    /// is the entry). Judged on the wide view for both views, since only
    /// its camera follows the player up to the map's edges.
    pub moved: bool,
    /// ...and from the same walk without its last hop: the player made it.
    pub arrived: bool,
    /// This view's camera moved on the last hop. The game view's camera
    /// stops short of the map's edges, so near one the player can walk
    /// out of the 640 x 480 frame.
    pub camera_followed: bool,
    /// Fractions of this view's pixels that differ from the entry's frame
    /// and from the frame without the last hop.
    pub changed_from_entry: f64,
    pub changed_by_last_hop: f64,
    /// What went wrong with this capture, if anything.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Index<'a> {
    map: &'a str,
    entry_hex: i32,
    arrival_check: String,
    shots: &'a [Shot],
}

/// What [`shots`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotsSummary {
    pub map: String,
    pub viewpoints: usize,
    pub shots: Vec<Shot>,
    pub dir: PathBuf,
}

/// The run kinds of a viewpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    View(View),
    /// The same view without the last hop.
    Check(View),
}

impl Run {
    fn name(self) -> String {
        match self {
            Run::View(view) => view.name().to_owned(),
            Run::Check(view) => format!("{}-check", view.name()),
        }
    }
}

struct Job {
    viewpoint: usize,
    run: Run,
    log: String,
    stem: String,
}

/// A file of the Fallout 2 install by any of its usual spellings.
fn find_archive(f2: &Path, name: &str) -> Result<PathBuf, ConvertError> {
    [name.to_owned(), name.to_ascii_uppercase()]
        .iter()
        .map(|n| f2.join(n))
        .find(|p| p.is_file())
        .ok_or_else(|| ConvertError::F2 {
            path: f2.to_owned(),
            reason: format!("no {name} here"),
        })
}

/// The map file name under `maps/`.
fn resolve_map(options: &ShotsOptions, layer: &Path) -> Result<String, ConvertError> {
    let strip = |name: &str| name.trim_start_matches("maps/").to_owned();
    if let Some(map) = &options.map {
        return Ok(strip(map));
    }
    if let Ok(features) = MapFeatures::read(&options.out) {
        return Ok(strip(&features.map_file));
    }
    let manifest_path = layer.join("vault-mod.toml");
    let text = std::fs::read_to_string(&manifest_path).map_err(io(&manifest_path))?;
    let manifest = vault_formats::mod_manifest::ModManifest::parse(&text)
        .map_err(|e| stage(format!("{}: {e}", manifest_path.display())))?;
    manifest
        .maps
        .first()
        .map(|m| m.file.clone())
        .ok_or_else(|| stage("the layer registers no map; pass --map"))
}

/// Runs the engine once over a replay log and returns the captured frame.
#[allow(clippy::too_many_arguments)]
fn run_engine(
    options: &ShotsOptions,
    archives: &(PathBuf, PathBuf),
    load_order: &Path,
    map: &str,
    log: &Path,
    commands: usize,
    work: &Path,
    stem: &str,
) -> Result<Frame, String> {
    let frame_path = work.join(format!("{stem}.ppm"));
    let _ = std::fs::remove_file(&frame_path);
    let stdout_path = work.join(format!("{stem}.out"));
    let stderr_path = work.join(format!("{stem}.err"));
    let file =
        |path: &Path| std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()));
    let mut child = Command::new(&options.engine)
        .arg("--replay-inputs")
        .arg(log)
        .arg("--replay-frame")
        .arg(&frame_path)
        .args(["--game-profile", "fallout2"])
        .arg("--master-dat")
        .arg(&archives.0)
        .arg("--critter-dat")
        .arg(&archives.1)
        .arg("--mod-list")
        .arg(load_order)
        .arg("--preview-map")
        .arg(format!("maps/{map}"))
        .stdin(Stdio::null())
        .stdout(file(&stdout_path)?)
        .stderr(file(&stderr_path)?)
        .spawn()
        .map_err(|e| format!("{}: {e}", options.engine.display()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > options.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "stopped after {} s (see work/{stem}.err)",
                    options.timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("waiting for the engine: {e}")),
        }
    };
    if !status.success() {
        return Err(format!(
            "the engine exited with {status} (see work/{stem}.err)"
        ));
    }
    // The engine's last line says how many commands it replayed.
    let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
    let replayed = stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .and_then(|v| v.get("commands").and_then(serde_json::Value::as_u64));
    if replayed != Some(commands as u64) {
        return Err(format!(
            "the engine replayed {replayed:?} of {commands} commands (see work/{stem}.out)"
        ));
    }
    let bytes =
        std::fs::read(&frame_path).map_err(|e| format!("no frame at work/{stem}.ppm: {e}"))?;
    decode_ppm(&bytes).map_err(|e| format!("work/{stem}.ppm: {e}"))
}

/// Takes the screenshots: see the module documentation.
pub fn shots(options: &ShotsOptions) -> Result<ShotsSummary, ConvertError> {
    if !options.engine.is_file() {
        return Err(stage(format!(
            "--engine {}: not a file",
            options.engine.display()
        )));
    }
    let archives = (
        find_archive(&options.f2, "master.dat")?,
        find_archive(&options.f2, "critter.dat")?,
    );
    let layer = options.out.join("mods").join(crate::map::LAYER_ID);
    if !layer.join("vault-mod.toml").is_file() {
        return Err(stage(format!(
            "no mod layer at {}; run the map stage first",
            layer.display()
        )));
    }
    let map = resolve_map(options, &layer)?;
    let walk_map = WalkMap::load(&layer, &map).map_err(stage)?;
    let read = match MapFeatures::read(&options.out) {
        Ok(f) if f.map_file.trim_start_matches("maps/") == map => Some(f),
        Ok(_) => None,
        Err(error) => {
            if options.auto > 1 {
                eprintln!(
                    "shots: {error}; without it --auto picks only the entry (re-run the map stage)"
                );
            }
            None
        }
    };
    let interior = read.as_ref().is_some_and(|f| f.interior.is_some());
    let (mut features, mut people) = read.map(|f| (f.features, f.people)).unwrap_or_default();
    // A file written before the current ranking is ranked again.
    crate::map::rank_features(&mut features);
    // Indoors the people come first: they are what a room is shown for.
    crate::map::rank_people(&mut people);
    let features: Vec<Feature> = crate::map::people_features(&people)
        .into_iter()
        .chain(features)
        .collect();
    let spread = options.spread.unwrap_or(if interior {
        INTERIOR_SPREAD
    } else {
        DEFAULT_SPREAD
    });
    let viewpoints = pick_viewpoints(&walk_map, &features, &options.at, options.auto, spread);
    let wide_zoom = options.wide_zoom.unwrap_or(if interior {
        INTERIOR_WIDE_ZOOM
    } else {
        DEFAULT_WIDE_ZOOM
    });
    if !(wide_zoom.is_finite() && wide_zoom > 0.0) {
        return Err(stage(format!(
            "--wide-zoom {wide_zoom}: expected a positive number"
        )));
    }
    for (n, v) in viewpoints.iter().enumerate() {
        eprintln!(
            "shots: viewpoint {n:02} {}: hex {} (asked {}), {} steps",
            v.label,
            v.route.destination,
            v.route.requested,
            v.route.steps()
        );
    }

    let dir = options.out.join("shots");
    let work = dir.join("work");
    std::fs::create_dir_all(&work).map_err(io(&work))?;
    // The same file `scripts/play.sh` writes: the layer, relative to it.
    let load_order = options.out.join("load_order.toml");
    std::fs::write(&load_order, "layers = [\"mods/mojave\"]\n").map_err(io(&load_order))?;
    let map_stem = map.trim_end_matches(".map").to_owned();
    let exits: Vec<i32> = walk_map.exits.iter().map(|e| e.hex).collect();

    let mut jobs: Vec<Job> = Vec::new();
    for (n, viewpoint) in viewpoints.iter().enumerate() {
        let all_hops = hops(&viewpoint.route, options.hop, &exits);
        let mut runs = vec![Run::View(View::Game), Run::View(View::Wide)];
        if !all_hops.is_empty() {
            runs.extend([Run::Check(View::Game), Run::Check(View::Wide)]);
        }
        for run in runs {
            let (view, hop_list) = match run {
                Run::View(view) => (view, &all_hops[..]),
                Run::Check(view) => (view, &all_hops[..all_hops.len() - 1]),
            };
            jobs.push(Job {
                viewpoint: n,
                run,
                log: replay_log(view, hop_list, wide_zoom),
                stem: format!("{map_stem}-{n:02}-{}", run.name()),
            });
        }
    }

    let queue: Mutex<VecDeque<usize>> = Mutex::new((0..jobs.len()).collect());
    let results: Vec<Mutex<Option<Result<Frame, String>>>> =
        jobs.iter().map(|_| Mutex::new(None)).collect();
    let mut log_error: Option<ConvertError> = None;
    for job in &jobs {
        let path = work.join(format!("{}.jsonl", job.stem));
        if let Err(e) = std::fs::write(&path, &job.log) {
            log_error = Some(ConvertError::Io { path, source: e });
            break;
        }
    }
    if let Some(error) = log_error {
        return Err(error);
    }
    std::thread::scope(|scope| {
        for _ in 0..options.jobs.clamp(1, 8) {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().ok().and_then(|mut q| q.pop_front());
                    let Some(i) = next else { break };
                    let job = &jobs[i];
                    let started = Instant::now();
                    let result = run_engine(
                        options,
                        &archives,
                        &load_order,
                        &map,
                        &work.join(format!("{}.jsonl", job.stem)),
                        job.log.lines().count(),
                        &work,
                        &job.stem,
                    );
                    eprintln!(
                        "shots: {} {} in {:.1} s",
                        job.stem,
                        if result.is_ok() { "captured" } else { "FAILED" },
                        started.elapsed().as_secs_f64()
                    );
                    if let Ok(mut slot) = results[i].lock() {
                        *slot = Some(result);
                    }
                }
            });
        }
    });
    let mut frames: Vec<Option<Result<Frame, String>>> = results
        .into_iter()
        .map(|m| m.into_inner().ok().flatten())
        .collect();
    let mut take = |viewpoint: usize, run: Run| -> Result<Frame, String> {
        jobs.iter()
            .position(|j| j.viewpoint == viewpoint && j.run == run)
            .and_then(|i| frames[i].take())
            .unwrap_or_else(|| Err("not run".to_owned()))
    };

    let mut shots = Vec::new();
    let mut entry: [Option<Frame>; 2] = [None, None];
    for (n, viewpoint) in viewpoints.iter().enumerate() {
        let still = viewpoint.route.steps() == 0;
        let frames = View::ALL.map(|view| take(n, Run::View(view)));
        let checks = View::ALL.map(|view| (!still).then(|| take(n, Run::Check(view))));
        if n == 0 {
            entry = std::array::from_fn(|v| frames[v].as_ref().ok().cloned());
        }
        // Per view: changed from the entry, changed by the last hop.
        let changes: [Option<(f64, f64)>; 2] =
            std::array::from_fn(|v| match (&frames[v], &checks[v], &entry[v]) {
                (Ok(frame), Some(Ok(check)), Some(first)) => Some((
                    changed_fraction(frame, first),
                    changed_fraction(frame, check),
                )),
                _ => None,
            });
        let wide = View::ALL.iter().position(|v| *v == View::Wide).unwrap_or(1);
        let (moved, arrived) = match changes[wide] {
            // Standing still, the player is where it was asked to be.
            _ if still => (false, true),
            Some((from_entry, by_last)) => {
                let moved = from_entry > MOVED_FRACTION;
                (moved, moved && by_last > MOVED_FRACTION)
            }
            None => (false, false),
        };
        let check_error: Vec<String> = checks
            .iter()
            .flatten()
            .filter_map(|c| c.as_ref().err())
            .map(|e| format!("arrival check: {e}"))
            .collect();
        for (v, (view, frame)) in View::ALL.into_iter().zip(frames).enumerate() {
            let (from_entry, by_last) = changes[v].unwrap_or((0.0, 0.0));
            let mut shot = Shot {
                viewpoint: n,
                label: viewpoint.label.clone(),
                requested_hex: viewpoint.route.requested,
                hex: viewpoint.route.destination,
                steps: viewpoint.route.steps(),
                hops: hops(&viewpoint.route, options.hop, &exits).len(),
                view: view.name(),
                file: String::new(),
                width: 0,
                height: 0,
                moved,
                arrived,
                camera_followed: still || by_last > MOVED_FRACTION,
                changed_from_entry: (from_entry * 1e4).round() / 1e4,
                changed_by_last_hop: (by_last * 1e4).round() / 1e4,
                error: (!check_error.is_empty()).then(|| check_error.join("; ")),
            };
            match frame {
                Ok(frame) => {
                    let name = format!("{map_stem}-{n:02}-{}.png", view.name());
                    let path = dir.join(&name);
                    let png =
                        foundry_gallery::encode_png(&frame.to_rgba(), frame.width, frame.height)
                            .map_err(stage)?;
                    std::fs::write(&path, png).map_err(io(&path))?;
                    shot.file = name;
                    shot.width = frame.width;
                    shot.height = frame.height;
                }
                Err(e) => shot.error = Some(e),
            }
            shots.push(shot);
        }
    }

    let index = Index {
        map: &map,
        entry_hex: walk_map.entry,
        arrival_check: format!(
            "frame comparison on the wide view, whose camera follows the player to the map's \
             edges: moved when it differs from the entry's in more than {:.0} % of the pixels, \
             arrived when it also differs that much from the same walk without its last hop; \
             the engine reports no player position",
            MOVED_FRACTION * 100.0
        ),
        shots: &shots,
    };
    let index_path = dir.join("index.json");
    let json = serde_json::to_string_pretty(&index).map_err(stage)?;
    std::fs::write(&index_path, json + "\n").map_err(io(&index_path))?;
    contact(&dir, &map, &shots)?;
    Ok(ShotsSummary {
        map,
        viewpoints: viewpoints.len(),
        shots,
        dir,
    })
}

/// A contact page of PNG files of any size, each under its label, scaled
/// down to fit `max_cell`, `columns` to a row: for choosing and showing
/// frames (`mojave-convert contact`).
pub fn contact_page(
    entries: &[(String, PathBuf)],
    title: Option<&str>,
    columns: u32,
    max_cell: [u32; 2],
    out: &Path,
) -> Result<(), ConvertError> {
    let mut images = Vec::new();
    for (label, path) in entries {
        let bytes = std::fs::read(path).map_err(io(path))?;
        let image = foundry_gallery::decode_png(&bytes)
            .map_err(|e| stage(format!("{}: {e}", path.display())))?;
        images.push((label.as_str(), image));
    }
    if images.is_empty() {
        return Err(stage("contact: no pictures given"));
    }
    let no_metrics: Vec<(String, String)> = Vec::new();
    let tiles: Vec<Tile<'_>> = images
        .iter()
        .map(|(label, image)| Tile {
            label,
            metrics: &no_metrics,
            rgba: &image.rgba,
            width: image.width,
            height: image.height,
            compare: None,
        })
        .collect();
    let options = SheetOptions {
        columns: Columns::Fixed(columns.max(1)),
        max_cell,
        anchor: Anchor::Centre,
        background: Background::Solid([0, 0, 0, 255]),
        title: title.map(str::to_owned),
        ..SheetOptions::default()
    };
    let sheet = contact_sheet(&tiles, &options).map_err(stage)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    std::fs::write(out, sheet.to_png().map_err(stage)?).map_err(io(out))?;
    Ok(())
}

/// `contact.png`: every shot, the game view beside the wide view, with
/// its viewpoint, hex and arrival.
fn contact(dir: &Path, map: &str, shots: &[Shot]) -> Result<(), ConvertError> {
    struct Entry {
        label: String,
        metrics: Vec<(String, String)>,
        image: foundry_gallery::Image,
    }
    let mut entries = Vec::new();
    for shot in shots.iter().filter(|s| !s.file.is_empty()) {
        let path = dir.join(&shot.file);
        let bytes = std::fs::read(&path).map_err(io(&path))?;
        let image = foundry_gallery::decode_png(&bytes).map_err(stage)?;
        let mut metrics = vec![
            ("hex".to_owned(), shot.hex.to_string()),
            ("steps".to_owned(), shot.steps.to_string()),
            (
                "arrived".to_owned(),
                if shot.arrived { "yes" } else { "no" }.to_owned(),
            ),
        ];
        if !shot.camera_followed {
            metrics.push(("camera".to_owned(), "did not follow the player".to_owned()));
        }
        entries.push(Entry {
            label: format!("{:02} {} {}", shot.viewpoint, shot.view, shot.label),
            metrics,
            image,
        });
    }
    if entries.is_empty() {
        return Err(stage("the engine captured no frame; see shots/work/*.err"));
    }
    let tiles: Vec<Tile<'_>> = entries
        .iter()
        .map(|e| Tile {
            label: &e.label,
            metrics: &e.metrics,
            rgba: &e.image.rgba,
            width: e.image.width,
            height: e.image.height,
            compare: None,
        })
        .collect();
    let options = SheetOptions {
        columns: Columns::Fixed(2),
        max_cell: [960, 540],
        anchor: Anchor::Centre,
        background: Background::Solid([0, 0, 0, 255]),
        title: Some(format!("{map}: engine frames")),
        ..SheetOptions::default()
    };
    let sheet = contact_sheet(&tiles, &options).map_err(stage)?;
    let path = dir.join("contact.png");
    std::fs::write(&path, sheet.to_png().map_err(stage)?).map_err(io(&path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(log: &str) -> Vec<serde_json::Value> {
        log.lines()
            .map(|l| serde_json::from_str(l).expect("a JSON line"))
            .collect()
    }

    #[test]
    fn a_game_view_log_walks_each_hop_with_rising_ids_and_ticks() {
        let log = replay_log(View::Game, &[(20_100, 15), (20_300, 3)], 0.5);
        let lines = parse(&log);
        assert_eq!(lines[0]["kind"], "surface");
        assert_eq!(lines[0]["width"], 640);
        assert_eq!(lines[0]["height"], 480);
        assert!(lines.iter().all(|l| l["kind"] != "enhanced"));
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(line["command_id"], i as u64 + 1);
        }
        let ticks: Vec<u64> = lines
            .iter()
            .filter(|l| l["kind"] == "tick")
            .map(|l| l["now_ms"].as_u64().expect("a time"))
            .collect();
        assert_eq!(ticks[0], TICK_MS);
        assert!(ticks.windows(2).all(|w| w[1] == w[0] + TICK_MS));
        assert_eq!(
            ticks.len() as u64,
            SETTLE_TICKS + hop_ticks(15) + hop_ticks(3) + FINAL_TICKS
        );
        let walks: Vec<&serde_json::Value> = lines.iter().filter(|l| l["kind"] == "bot").collect();
        assert_eq!(walks.len(), 2);
        assert_eq!(walks[0]["action"]["op"], "walk");
        assert_eq!(walks[0]["action"]["hex"], 20_100);
        assert_eq!(walks[0]["action"]["run"], true);
        assert_eq!(walks[1]["action"]["hex"], 20_300);
        // The second walk comes after the first hop's ticks.
        let second = lines
            .iter()
            .position(|l| l["action"]["hex"] == 20_300)
            .unwrap();
        assert_eq!(second as u64, 1 + SETTLE_TICKS + 1 + hop_ticks(15));
    }

    #[test]
    fn a_wide_view_log_sets_the_enhanced_viewport_second() {
        let log = replay_log(View::Wide, &[], DEFAULT_WIDE_ZOOM);
        assert!(log.ends_with('\n'));
        let lines = parse(&log);
        assert_eq!(lines[0]["width"], 1920);
        assert_eq!(lines[0]["height"], 1080);
        assert_eq!(lines[1]["kind"], "enhanced");
        let settings = &lines[1]["settings"];
        assert_eq!(settings["viewport_mode"], "unlocked_widescreen");
        assert_eq!(settings["default_zoom"], 0.5);
        assert_eq!(settings["smooth_camera"], false);
        assert_eq!(settings["roof_peek"], "hidden");
        assert!(settings["ui_scale"].is_null());
        assert_eq!(lines.len() as u64, 2 + SETTLE_TICKS + FINAL_TICKS);
        // The exact first lines of the hand-written logs that worked.
        let zoomed = parse(&replay_log(View::Wide, &[], INTERIOR_WIDE_ZOOM));
        assert_eq!(zoomed[1]["settings"]["default_zoom"], 0.75);
        assert!(log.starts_with(
            "{\"command_id\":1,\"kind\":\"surface\",\"width\":1920,\"height\":1080}\n\
             {\"command_id\":2,\"kind\":\"enhanced\",\"settings\":{\"viewport_mode\":\"unlocked_widescreen\",\"default_zoom\":0.5,\"smooth_camera\":false,\"roof_peek\":\"hidden\",\"ui_scale\":null}}\n\
             {\"command_id\":3,\"kind\":\"tick\",\"now_ms\":16}\n"
        ));
    }

    #[test]
    fn hops_carry_the_steps_between_waypoints() {
        let path: Vec<i32> = (0..=32).map(|c| 100 + c + 200 * 100).collect();
        let route = Route {
            requested: *path.last().unwrap(),
            destination: *path.last().unwrap(),
            path,
        };
        let plain = hops(&route, 15, &[]);
        assert_eq!(plain, vec![(20_115, 15), (20_130, 15), (20_132, 2)]);
        assert_eq!(plain.iter().map(|h| h.1).sum::<usize>(), route.steps());
        // Beside an exit grid a hop stops short of it.
        let near = hops(&route, 15, &[20_120 - 200]);
        assert_eq!(near, vec![(20_115, 15), (20_120, 5), (20_132, 12)]);
        assert_eq!(near.iter().map(|h| h.1).sum::<usize>(), route.steps());
    }

    fn ppm(header: &str, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = header.as_bytes().to_vec();
        bytes.extend_from_slice(pixels);
        bytes
    }

    #[test]
    fn a_ppm_decodes_with_comments_and_scaled_samples() {
        let frame = decode_ppm(&ppm(
            "P6\n# made by a test\n2 1\n255\n",
            &[1, 2, 3, 4, 5, 6],
        ))
        .expect("decodes");
        assert_eq!((frame.width, frame.height), (2, 1));
        assert_eq!(frame.rgb, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(frame.to_rgba(), vec![1, 2, 3, 255, 4, 5, 6, 255]);
        // The engine's own header layout.
        let frame = decode_ppm(&ppm("P6\n1 1\n255\n", &[9, 8, 7])).expect("decodes");
        assert_eq!(frame.rgb, vec![9, 8, 7]);
        // A smaller maximum is scaled up; a sample over it is clamped.
        let frame = decode_ppm(&ppm("P6 1 1 15 ", &[15, 0, 99])).expect("decodes");
        assert_eq!(frame.rgb, vec![255, 0, 255]);
        // Trailing bytes after the samples are ignored.
        assert!(decode_ppm(&ppm("P6\n1 1\n255\n", &[1, 2, 3, 4])).is_ok());
    }

    #[test]
    fn a_bad_ppm_is_an_error_not_a_panic() {
        for (header, pixels) in [
            ("P3\n1 1\n255\n", &[0u8, 0, 0][..]),
            ("P6\n2 2\n255\n", &[0, 0, 0][..]),
            ("P6\n1 1\n65535\n", &[0, 0, 0, 0, 0, 0][..]),
            ("P6\n0 1\n255\n", &[][..]),
            ("P6\n99999 99999\n255\n", &[0][..]),
            ("P6\n1 1\n255", &[][..]),
            ("P6\n1\n", &[][..]),
            ("P6\n1234567890123 1\n255\n", &[][..]),
            ("", &[][..]),
        ] {
            assert!(
                decode_ppm(&ppm(header, pixels)).is_err(),
                "{header:?} should fail"
            );
        }
    }

    #[test]
    fn frames_compare_by_changed_pixels() {
        let a = Frame {
            width: 2,
            height: 2,
            rgb: vec![0; 12],
        };
        let mut b = a.clone();
        assert_eq!(changed_fraction(&a, &b), 0.0);
        b.rgb[4] = 1;
        assert_eq!(changed_fraction(&a, &b), 0.25);
        let c = Frame {
            width: 4,
            height: 1,
            rgb: vec![0; 12],
        };
        assert_eq!(changed_fraction(&a, &c), 1.0);
    }

    fn open_map(entry: i32) -> WalkMap {
        WalkMap {
            path: "maps/test.map".to_owned(),
            objects: 0,
            blockers: 0,
            blocked: vec![false; route::HEXES],
            exits: Vec::new(),
            entry,
        }
    }

    fn feature(hex: i32, model: &str) -> Feature {
        Feature {
            kind: "walls".to_owned(),
            reference: format!("a:{hex}"),
            model: model.to_owned(),
            hex,
            pieces: 10,
            pixels: 1000,
        }
    }

    #[test]
    fn automatic_viewpoints_start_at_the_entry_and_spread_out() {
        let entry = 100 + 200 * 100;
        let mut map = open_map(entry);
        // A sealed building at (140, 100): its viewpoint is beside it.
        let building = 140 + 200 * 100;
        map.blocked[building as usize] = true;
        for o in 0..6u8 {
            let n = HexGrid::neighbor(building, o).unwrap().unwrap();
            map.blocked[n as usize] = true;
        }
        let features = vec![
            feature(building, "architecture\\house01.nif"),
            // Too close to the building's viewpoint.
            feature(142 + 200 * 100, "a.nif"),
            feature(100 + 200 * 150, "tower.nif"),
            feature(60 + 200 * 60, "never reached: auto is 3"),
        ];
        let picked = pick_viewpoints(&map, &features, &[], 3, 30);
        let labels: Vec<&str> = picked.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, ["entry", "walls house01", "walls tower"]);
        assert_eq!(picked[0].route.steps(), 0);
        assert_eq!(
            HexGrid::lattice_distance(picked[1].route.destination, building),
            Ok(2)
        );
        assert!(
            picked
                .iter()
                .all(|v| !map.blocked[v.route.destination as usize])
        );

        // A feature by the border waits until the others are taken.
        let edge = vec![
            feature(100 + 200 * 5, "edge.nif"),
            feature(100 + 200 * 150, "tower.nif"),
        ];
        let labels: Vec<String> = pick_viewpoints(&map, &edge, &[], 3, 30)
            .into_iter()
            .map(|v| v.label)
            .collect();
        assert_eq!(labels, ["entry", "walls tower", "walls edge"]);

        // Explicit hexes come first, after the entry; auto 0 adds nothing.
        let picked = pick_viewpoints(&map, &features, &[entry + 5], 0, 30);
        let labels: Vec<String> = picked.iter().map(|v| v.label.clone()).collect();
        assert_eq!(labels, ["entry".to_owned(), format!("at {}", entry + 5)]);
    }

    #[test]
    fn people_close_together_each_get_a_viewpoint() {
        let entry = 100 + 200 * 100;
        let map = open_map(entry);
        let person = |hex: i32, name: &str| Feature {
            kind: "person".to_owned(),
            reference: format!("p:{hex}"),
            model: name.to_owned(),
            hex,
            pieces: 1,
            pixels: 0,
        };
        let features = vec![
            person(104 + 200 * 100, "Trudy"),
            person(106 + 200 * 100, "Sunny Smiles"),
            // Walls this close to a viewpoint keep the usual spread.
            feature(108 + 200 * 100, "wall.nif"),
        ];
        let labels: Vec<String> = pick_viewpoints(&map, &features, &[], 4, 5)
            .into_iter()
            .map(|v| v.label)
            .collect();
        assert_eq!(labels, ["entry", "person Trudy", "person Sunny Smiles"]);
    }

    #[test]
    fn a_model_is_shot_twice_only_when_the_others_are_taken() {
        let entry = 100 + 200 * 100;
        let map = open_map(entry);
        let features = vec![
            feature(60 + 200 * 60, "crate.nif"),
            feature(140 + 200 * 60, "crate.nif"),
            feature(60 + 200 * 140, "hangar.nif"),
        ];
        let labels: Vec<String> = pick_viewpoints(&map, &features, &[], 3, 30)
            .into_iter()
            .map(|v| v.label)
            .collect();
        assert_eq!(labels, ["entry", "walls crate", "walls hangar"]);
        let labels: Vec<String> = pick_viewpoints(&map, &features, &[], 4, 30)
            .into_iter()
            .map(|v| v.label)
            .collect();
        assert_eq!(
            labels,
            ["entry", "walls crate", "walls hangar", "walls crate"]
        );
    }
}
