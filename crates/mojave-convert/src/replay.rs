//! Replay input logs for the engine's headless replay mode, the stationary
//! camera's framing plan, and the engine runs themselves.
//!
//! The engine has no camera record in its replay input. Its enhanced view
//! follows the player's drawn position every tick until a middle-button
//! drag (even one of zero length), arrow keys, edge panning or an
//! off-centre wheel zoom stops it, and `Home` turns following back on. So a
//! stationary camera is made of input the engine already reads, sent after
//! the settle ticks:
//!
//! 1. `pointer_moved` to a point inside the world area, away from the
//!    edge band (2 % of the canvas);
//! 2. `pointer_button` Middle pressed;
//! 3. `pointer_moved` to the same point, or to a point `d` surface pixels
//!    away, which pans the view by `d / (ui scale * zoom)` world pixels;
//! 4. `pointer_button` Middle released;
//! 5. `pointer_left`, without which the engine draws its hex cursor (a
//!    32 x 16 outline) at the pointer in every later frame.
//!
//! A discrete re-centre is `Home` pressed and released, one tick (the
//! camera snaps to the player: `smooth_camera` is off), then another such
//! drag. All of it was checked on engine frames: the frozen view is
//! byte-identical to the following view before the walk, stays put while
//! the player walks, and `Home` plus a zero-length drag after the walk is
//! byte-identical to the following view at the same tick.
//!
//! The Vanilla 640 x 480 view re-centres on the player at every hex step
//! and no input holds it still; it is always the engine-following view.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use vault_formats::HexGrid;

use crate::shots::{Frame, decode_ppm};

/// Milliseconds per engine tick in a replay log.
pub const TICK_MS: u64 = 16;
/// Ticks before the first walk: the map settles.
pub const SETTLE_TICKS: u64 = 30;
/// Ticks after the last hop, before the frame is captured.
pub const FINAL_TICKS: u64 = 40;
/// The stationary camera's default inner box: this fraction of the
/// frame's width and height is kept clear on each side.
pub const DEFAULT_MARGIN: f64 = 0.15;
/// The largest margin accepted: the inner box keeps a tenth of the frame.
pub const MAX_MARGIN: f64 = 0.45;
/// The classic frame's zoom: the engine's own pixel scale.
pub const CLASSIC_ZOOM: f32 = 1.0;
/// The Fallout 2 interface frame, in interface pixels, and its bar.
const UI_FRAME: [f32; 2] = [640.0, 480.0];
const BAR_HEIGHT: f32 = 100.0;
/// The enhanced view's zoom range.
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 3.0;
/// The player's drawn extent around its hex centre, in world pixels:
/// `[left, top, right, bottom]`. A standing critter is about 70 pixels
/// tall above its hex.
pub const PLAYER_BOX: [f64; 4] = [-16.0, -72.0, 16.0, 8.0];
/// The most a hex step moves a hex centre, in world pixels: 32 across,
/// 12 down.
const STEP_PIXELS: [f64; 2] = [32.0, 12.0];
/// A drag stroke stays within this fraction of the world area around its
/// centre, well inside the edge band.
const STROKE_SPAN: f64 = 0.8;

/// One way of looking at the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum View {
    /// The 640 × 480 game view with the interface. Vanilla: the engine
    /// re-centres it on the player at every step.
    Game,
    /// The enhanced view on a 640 × 480 surface at zoom 1.0: a 640 × 380
    /// frame of map, the classic proportions, no interface bar.
    Classic,
    /// The enhanced (unlocked widescreen) view on a 1920 × 1080 surface,
    /// zoomed; the frame is the world canvas without the interface bar.
    Wide,
}

impl View {
    pub fn name(self) -> &'static str {
        match self {
            View::Game => "game",
            View::Classic => "classic",
            View::Wide => "wide",
        }
    }

    /// The surface the engine is told it draws on.
    pub fn surface(self) -> [u32; 2] {
        match self {
            View::Game | View::Classic => [640, 480],
            View::Wide => [1920, 1080],
        }
    }

    /// Whether the enhanced presentation draws it (the only one whose
    /// camera can be held still).
    pub fn enhanced(self) -> bool {
        self != View::Game
    }

    /// `enhanced` or `vanilla`.
    pub fn kind(self) -> &'static str {
        if self.enhanced() {
            "enhanced"
        } else {
            "vanilla"
        }
    }
}

/// The enhanced view's layout on a surface, by the engine's own
/// arithmetic: the interface scale fits the 640 × 480 frame, the bar takes
/// 100 interface pixels at the bottom, and the world canvas covers the rest
/// at `ui_scale * zoom` surface pixels per world pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub surface: [u32; 2],
    pub ui_scale: f64,
    pub zoom: f64,
    /// Surface pixels per world pixel.
    pub per_world: f64,
    /// The world area's size in surface pixels (from the surface's top
    /// left corner).
    pub world_area: [f64; 2],
    /// The world canvas in world pixels: the captured frame's size.
    pub canvas: [u32; 2],
}

/// The enhanced layout of `surface` at `zoom` (clamped to 0.5 to 3.0). The
/// engine may raise the zoom further to keep a large map's canvas within
/// its limits; the interiors and windows converted here never reach that.
pub fn layout(surface: [u32; 2], zoom: f32) -> Layout {
    let [width, height] = surface.map(|v| v.max(1) as f32);
    let ui_scale = (width / UI_FRAME[0]).min(height / UI_FRAME[1]);
    let zoom = if zoom.is_nan() {
        1.0
    } else {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    };
    let world = [width, (height - BAR_HEIGHT * ui_scale).max(1.0)];
    let per_world = f64::from(ui_scale * zoom);
    let canvas = world.map(|side| (f64::from(side) / per_world).ceil().max(1.0) as u32);
    Layout {
        surface,
        ui_scale: f64::from(ui_scale),
        zoom: f64::from(zoom),
        per_world,
        world_area: world.map(f64::from),
        canvas,
    }
}

/// The captured frame's size for a view: the world canvas of an enhanced
/// view, the whole 640 × 480 game view.
pub fn frame_size(view: View, zoom: f32) -> [u32; 2] {
    if view.enhanced() {
        layout(view.surface(), zoom).canvas
    } else {
        view.surface()
    }
}

/// One walk command: its waypoint, the hex steps from the waypoint before,
/// the ticks it gets, and the hexes it is expected to cross.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub hex: i32,
    pub steps: usize,
    pub ticks: u64,
    /// From the hop's start to its end, both included (just the two ends
    /// when the route is not known).
    pub path: Vec<i32>,
    /// The hop ends on an exit grid: the hex the player arrives on, on the
    /// grid's map and elevation (a crossing; the next hop is on that map).
    pub lands: Option<i32>,
}

/// A re-centre of the stationary camera, made before a hop starts.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recentre {
    /// The hop it comes before (0 is the first walk).
    pub before_hop: usize,
    /// Where the player stands then (the camera's `Home` snaps there).
    pub player_hex: i32,
    /// The new centre, in world pixels of the default grid.
    pub centre: [f64; 2],
    /// The hex under the new centre.
    pub centre_hex: Option<i32>,
}

/// Where a stationary camera looks: where it settles after the first
/// freeze, and the re-centres along the walk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CameraPlan {
    /// The player's hex at the first freeze (the map's entry).
    pub start_hex: i32,
    /// The first centre, in world pixels of the default grid.
    pub centre: [f64; 2],
    /// The hex under it.
    pub centre_hex: Option<i32>,
    /// The frame the plan was made for, in world pixels.
    pub canvas: [u32; 2],
    pub margin: f64,
    pub recentres: Vec<Recentre>,
}

fn hex_point(hex: i32) -> Option<[f64; 2]> {
    let [x, y] = HexGrid::default().hex_center(hex).ok()?;
    Some([x as f64, y as f64])
}

fn point_hex(point: [f64; 2]) -> Option<i32> {
    let [x, y] = point.map(|v| v.round());
    if !(-1e9..1e9).contains(&x) || !(-1e9..1e9).contains(&y) {
        return None;
    }
    HexGrid::default()
        .pick_hex([x as i32, y as i32])
        .filter(|hex| (0..40_000).contains(hex))
}

/// The player's drawn box standing on `hex`.
fn hex_box(hex: i32) -> Option<[f64; 4]> {
    let [x, y] = hex_point(hex)?;
    Some([
        x + PLAYER_BOX[0],
        y + PLAYER_BOX[1],
        x + PLAYER_BOX[2],
        y + PLAYER_BOX[3],
    ])
}

fn union(a: Option<[f64; 4]>, b: Option<[f64; 4]>) -> Option<[f64; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Half the inner box's size, in world pixels.
fn inner_half(canvas: [u32; 2], margin: f64) -> [f64; 2] {
    let keep = (0.5 - margin.clamp(0.0, MAX_MARGIN)).max(0.0);
    canvas.map(|side| f64::from(side) * keep)
}

fn fits(bbox: [f64; 4], half: [f64; 2]) -> bool {
    bbox[2] - bbox[0] <= 2.0 * half[0] && bbox[3] - bbox[1] <= 2.0 * half[1]
}

fn inside(bbox: [f64; 4], centre: [f64; 2], half: [f64; 2]) -> bool {
    bbox[0] >= centre[0] - half[0]
        && bbox[2] <= centre[0] + half[0]
        && bbox[1] >= centre[1] - half[1]
        && bbox[3] <= centre[1] + half[1]
}

fn middle(bbox: [f64; 4]) -> [f64; 2] {
    [(bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0]
}

/// The most hex steps a hop may take and still always fit the inner box
/// together with the place it starts from, whatever its direction (a step
/// moves a hex centre at most 32 pixels across or 12 down). At least 1.
pub fn fit_steps(canvas: [u32; 2], margin: f64) -> usize {
    let half = inner_half(canvas, margin);
    let free = [
        2.0 * half[0] - (PLAYER_BOX[2] - PLAYER_BOX[0]),
        2.0 * half[1] - (PLAYER_BOX[3] - PLAYER_BOX[1]),
    ];
    let steps = (free[0] / STEP_PIXELS[0]).min(free[1] / STEP_PIXELS[1]);
    if steps.is_finite() && steps >= 1.0 {
        steps.floor() as usize
    } else {
        1
    }
}

/// Plans a stationary camera for a walk from `start` over `hops`, on a
/// frame of `canvas` world pixels whose inner box keeps `margin` of the
/// frame clear on each side.
///
/// The first centre frames the longest run of hops from the start whose
/// hexes (each with the player's drawn box) fit the inner box together:
/// the middle of their bounding box. A hop that would take the player out
/// of the inner box around the current centre gets a re-centre before it,
/// to the middle of the longest run that fits from there on (the hop
/// itself always counts, so a hop too long to fit is centred on its own).
/// Re-centres happen only at hop boundaries, never during a walk.
pub fn plan_camera(start: i32, hops: &[Hop], canvas: [u32; 2], margin: f64) -> CameraPlan {
    let half = inner_half(canvas, margin);
    let hop_box = |from: i32, hop: &Hop| {
        hop.path
            .iter()
            .fold(union(hex_box(from), hex_box(hop.hex)), |b, h| {
                union(b, hex_box(*h))
            })
    };
    // The bounding box of the run of hops from `first` that fits; a run
    // ends with a hop that crosses an exit grid (the next is elsewhere).
    let run = |first: usize, from: i32| {
        let mut bbox = hex_box(from);
        let mut at = from;
        for (k, hop) in hops[first..].iter().enumerate() {
            let next = union(bbox, hop_box(at, hop));
            if k > 0 && !next.is_some_and(|b| fits(b, half)) {
                break;
            }
            bbox = next;
            at = hop.hex;
            if hop.lands.is_some() {
                break;
            }
        }
        bbox
    };
    let fallback = hex_point(start).unwrap_or([0.0, 0.0]);
    let mut centre = run(0, start).map_or(fallback, middle);
    let first = centre;
    let mut planned_from = 0usize;
    let mut recentres = Vec::new();
    let mut at = start;
    for (k, hop) in hops.iter().enumerate() {
        let seen = hop_box(at, hop).is_none_or(|b| inside(b, centre, half));
        // After a crossing the camera always re-centres: the engine loaded
        // another map (or elevation) and centred its view on the player.
        let crossed = k > 0 && hops[k - 1].lands.is_some();
        if crossed || (k != planned_from && !seen) {
            let next = run(k, at).map_or(centre, middle);
            if next != centre || crossed {
                centre = next;
                recentres.push(Recentre {
                    before_hop: k,
                    player_hex: at,
                    centre,
                    centre_hex: point_hex(centre),
                });
            }
            planned_from = k;
        }
        at = hop.lands.unwrap_or(hop.hex);
    }
    CameraPlan {
        start_hex: start,
        centre: first,
        centre_hex: point_hex(first),
        canvas,
        margin,
        recentres,
    }
}

/// What a replay log is built for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogOptions {
    pub view: View,
    /// The enhanced view's zoom (ignored by the game view).
    pub zoom: f32,
    /// Walk commands run (else walk).
    pub run: bool,
    pub settle: u64,
    pub tail: u64,
}

impl LogOptions {
    /// The options `shots` uses: run, [`SETTLE_TICKS`], [`FINAL_TICKS`].
    pub fn shots(view: View, zoom: f32) -> Self {
        Self {
            view,
            zoom,
            run: true,
            settle: SETTLE_TICKS,
            tail: FINAL_TICKS,
        }
    }
}

/// A replay input log, one JSON record per line, with where each tick
/// ends so that the same log can be cut after any tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayLog {
    lines: Vec<String>,
    /// Lines through each tick: `tick_ends[t - 1]` for the `t`-th tick.
    tick_ends: Vec<usize>,
    /// Per tick: a frame cut after it would show a camera about to move
    /// (before the first freeze, or between `Home` and its freeze).
    unsettled: Vec<bool>,
    /// Per re-centre of the plan, the tick after which the camera holds
    /// its new centre.
    recentre_ticks: Vec<usize>,
    now_ms: u64,
}

impl ReplayLog {
    fn push(&mut self, body: &str) {
        let id = self.lines.len() + 1;
        self.lines.push(format!("{{\"command_id\":{id},{body}}}"));
    }

    fn ticks(&mut self, n: u64, unsettled: bool) {
        for _ in 0..n {
            self.now_ms += TICK_MS;
            let body = format!("\"kind\":\"tick\",\"now_ms\":{}", self.now_ms);
            self.push(&body);
            self.tick_ends.push(self.lines.len());
            self.unsettled.push(unsettled);
        }
    }

    fn pointer(&mut self, at: [f64; 2]) {
        let body = format!(
            "\"kind\":\"pointer_moved\",\"x\":{:.3},\"y\":{:.3}",
            at[0], at[1]
        );
        self.push(&body);
    }

    fn middle(&mut self, pressed: bool) {
        let body =
            format!("\"kind\":\"pointer_button\",\"button\":\"Middle\",\"pressed\":{pressed}");
        self.push(&body);
    }

    fn key(&mut self, key: &str, pressed: bool) {
        let body = format!(
            "\"kind\":\"key\",\"physical_key\":\"{key}\",\"pressed\":{pressed},\"repeat\":false"
        );
        self.push(&body);
    }

    /// Stops the camera following and pans it by `delta` world pixels:
    /// middle-button drags from the world area's centre (split into
    /// strokes that stay well inside the world area), then the pointer
    /// leaves the window so no cursor is drawn.
    fn freeze(&mut self, layout: &Layout, delta: [f64; 2]) {
        let centre = layout.world_area.map(|side| side / 2.0);
        let drag = delta.map(|d| d * layout.per_world);
        let strokes = (0..2)
            .map(|axis| (drag[axis].abs() / (STROKE_SPAN * layout.world_area[axis])).ceil())
            .fold(1.0f64, f64::max)
            .min(64.0);
        let step = drag.map(|d| d / strokes);
        for _ in 0..strokes as usize {
            // The view moves by the pointer's motion against the world.
            let from = [0, 1].map(|axis| centre[axis] + step[axis] / 2.0);
            let to = [0, 1].map(|axis| centre[axis] - step[axis] / 2.0);
            self.pointer(from);
            self.middle(true);
            self.pointer(to);
            self.middle(false);
        }
        self.push("\"kind\":\"pointer_left\"");
    }

    /// The whole log.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Ticks in the log.
    pub fn ticks_total(&self) -> usize {
        self.tick_ends.len()
    }

    /// Whether a frame cut after the `tick`-th tick (1-based) shows the
    /// camera where the plan puts it.
    pub fn settled(&self, tick: usize) -> bool {
        tick >= 1 && self.unsettled.get(tick - 1).is_some_and(|u| !u)
    }

    /// Per re-centre, the tick after which the camera holds its new
    /// centre (frames cut there or later show it).
    pub fn recentre_ticks(&self) -> &[usize] {
        &self.recentre_ticks
    }

    /// The first tick at or after `tick` a frame may be cut at.
    pub fn next_settled(&self, tick: usize) -> Option<usize> {
        (tick.max(1)..=self.ticks_total()).find(|t| self.settled(*t))
    }

    /// The log through its `tick`-th tick (1-based), and its line count.
    pub fn cut(&self, tick: usize) -> (String, usize) {
        let end = match tick {
            0 => 0,
            t => self.tick_ends[(t - 1).min(self.tick_ends.len().saturating_sub(1))],
        };
        let mut out = String::new();
        for line in &self.lines[..end.min(self.lines.len())] {
            out.push_str(line);
            out.push('\n');
        }
        (out, end)
    }
}

/// Builds a replay log: the surface, for an enhanced view the enhanced
/// settings (`smooth_camera` off, roofs hidden), the settle ticks, then
/// per hop a walk to its waypoint and its ticks, then the tail ticks.
///
/// With a `plan` (enhanced views only) the camera is frozen right after
/// the settle ticks, panned to the plan's first centre, and re-centred
/// (`Home`, a tick, a freeze panned to the new centre) before each hop the
/// plan names. Without one the camera follows the player.
pub fn build_log(options: &LogOptions, hops: &[Hop], plan: Option<&CameraPlan>) -> ReplayLog {
    let mut log = ReplayLog::default();
    let [width, height] = options.view.surface();
    log.push(&format!(
        "\"kind\":\"surface\",\"width\":{width},\"height\":{height}"
    ));
    let plan = plan.filter(|_| options.view.enhanced());
    if options.view.enhanced() {
        log.push(&format!(
            "\"kind\":\"enhanced\",\"settings\":{{\"viewport_mode\":\"unlocked_widescreen\",\
             \"default_zoom\":{:?},\"smooth_camera\":false,\"roof_peek\":\"hidden\",\
             \"ui_scale\":null}}",
            options.zoom
        ));
    }
    let layout = layout(options.view.surface(), options.zoom);
    let offset = |centre: [f64; 2], hex: i32| {
        let at = hex_point(hex).unwrap_or(centre);
        [centre[0] - at[0], centre[1] - at[1]]
    };
    log.ticks(options.settle, plan.is_some());
    if let Some(plan) = plan {
        log.freeze(&layout, offset(plan.centre, plan.start_hex));
    }
    for (k, hop) in hops.iter().enumerate() {
        if let Some(recentre) = plan.and_then(|p| p.recentres.iter().find(|r| r.before_hop == k)) {
            log.key("Home", true);
            log.key("Home", false);
            log.ticks(1, true);
            log.freeze(&layout, offset(recentre.centre, recentre.player_hex));
            log.recentre_ticks.push(log.ticks_total() + 1);
        }
        log.push(&format!(
            "\"kind\":\"bot\",\"action\":{{\"op\":\"walk\",\"hex\":{},\"run\":{}}}",
            hop.hex, options.run
        ));
        log.ticks(hop.ticks, false);
    }
    log.ticks(options.tail, false);
    log
}

/// A `vault-platform` executable and what it is run over.
#[derive(Debug, Clone)]
pub struct Engine {
    pub exe: PathBuf,
    pub master_dat: PathBuf,
    pub critter_dat: PathBuf,
    /// The `--mod-list` file.
    pub load_order: PathBuf,
    /// The map file name under `maps/`.
    pub map: String,
    /// Longest a run may take before it is stopped.
    pub timeout: Duration,
}

impl Engine {
    /// Runs the engine once over the replay log at `log` (of `commands`
    /// lines) and returns the frame it captured. Its frame, standard
    /// output and standard error go to `work/<stem>.{ppm,out,err}`.
    pub fn run(
        &self,
        log: &Path,
        commands: usize,
        work: &Path,
        stem: &str,
    ) -> Result<Frame, String> {
        let frame_path = work.join(format!("{stem}.ppm"));
        let _ = std::fs::remove_file(&frame_path);
        let stdout_path = work.join(format!("{stem}.out"));
        let stderr_path = work.join(format!("{stem}.err"));
        let file = |path: &Path| {
            std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))
        };
        let mut child = Command::new(&self.exe)
            .arg("--replay-inputs")
            .arg(log)
            .arg("--replay-frame")
            .arg(&frame_path)
            .args(["--game-profile", "fallout2"])
            .arg("--master-dat")
            .arg(&self.master_dat)
            .arg("--critter-dat")
            .arg(&self.critter_dat)
            .arg("--mod-list")
            .arg(&self.load_order)
            .arg("--preview-map")
            .arg(format!("maps/{}", self.map))
            .stdin(Stdio::null())
            .stdout(file(&stdout_path)?)
            .stderr(file(&stderr_path)?)
            .spawn()
            .map_err(|e| format!("{}: {e}", self.exe.display()))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() > self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "stopped after {} s (see {stem}.err)",
                        self.timeout.as_secs()
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(format!("waiting for the engine: {e}")),
            }
        };
        if !status.success() {
            return Err(format!("the engine exited with {status} (see {stem}.err)"));
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
                "the engine replayed {replayed:?} of {commands} commands (see {stem}.out)"
            ));
        }
        let bytes =
            std::fs::read(&frame_path).map_err(|e| format!("no frame at {stem}.ppm: {e}"))?;
        decode_ppm(&bytes).map_err(|e| format!("{stem}.ppm: {e}"))
    }
}

/// One engine run: a replay log and the file stem its outputs get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineJob {
    pub stem: String,
    pub log: String,
}

/// Runs every job, at most `parallel` engine processes at once (1 to 8),
/// writing each log to `work/<stem>.jsonl` first. With `tidy`, a run that
/// succeeded leaves nothing in `work/` (its log, frame and engine logs are
/// removed); a failed run's files stay for inspection. Results come back
/// in job order whatever the parallelism.
pub fn run_jobs(
    engine: &Engine,
    jobs: &[EngineJob],
    work: &Path,
    parallel: usize,
    tidy: bool,
    tag: &str,
) -> Vec<Result<Frame, String>> {
    let queue: Mutex<VecDeque<usize>> = Mutex::new((0..jobs.len()).collect());
    let results: Vec<Mutex<Option<Result<Frame, String>>>> =
        jobs.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..parallel.clamp(1, 8) {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().ok().and_then(|mut q| q.pop_front());
                    let Some(i) = next else { break };
                    let job = &jobs[i];
                    let started = Instant::now();
                    let log_path = work.join(format!("{}.jsonl", job.stem));
                    let result = std::fs::write(&log_path, &job.log)
                        .map_err(|e| format!("{}: {e}", log_path.display()))
                        .and_then(|()| {
                            engine.run(&log_path, job.log.lines().count(), work, &job.stem)
                        });
                    eprintln!(
                        "{tag}: {} {} in {:.1} s",
                        job.stem,
                        if result.is_ok() { "captured" } else { "FAILED" },
                        started.elapsed().as_secs_f64()
                    );
                    if tidy && result.is_ok() {
                        for ext in ["jsonl", "ppm", "out", "err"] {
                            let _ = std::fs::remove_file(work.join(format!("{}.{ext}", job.stem)));
                        }
                    }
                    if let Ok(mut slot) = results[i].lock() {
                        *slot = Some(result);
                    }
                }
            });
        }
    });
    results
        .into_iter()
        .map(|m| {
            m.into_inner()
                .ok()
                .flatten()
                .unwrap_or_else(|| Err("not run".to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_the_frames_the_engine_captured() {
        // Sizes read from engine frames.
        assert_eq!(layout([1920, 1080], 0.75).canvas, [1138, 507]);
        assert_eq!(layout([1920, 1080], 0.5).canvas, [1707, 760]);
        assert_eq!(layout([640, 480], 1.0).canvas, [640, 380]);
        assert_eq!(frame_size(View::Game, 1.0), [640, 480]);
        let wide = layout([1920, 1080], 0.75);
        assert_eq!(wide.ui_scale, 2.25);
        assert_eq!(wide.per_world, 1.6875);
        assert_eq!(wide.world_area, [1920.0, 855.0]);
    }

    #[test]
    fn a_long_pan_is_split_into_strokes_inside_the_world_area() {
        let classic = layout([640, 480], 1.0);
        let mut log = ReplayLog::default();
        log.freeze(&classic, [1000.0, -10.0]);
        let presses = log
            .lines()
            .iter()
            .filter(|l| l.contains("\"pressed\":true"))
            .count();
        assert_eq!(presses, 2);
        let mut total = 0.0;
        let mut last = None;
        for line in log.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v["kind"] == "pointer_moved" {
                let x = v["x"].as_f64().unwrap();
                assert!((64.0..576.0).contains(&x), "{x}");
                match last.take() {
                    None => last = Some(x),
                    Some(from) => total += from - x,
                }
            }
        }
        assert!((total - 1000.0).abs() < 0.01, "{total}");
        assert!(log.lines().last().unwrap().contains("pointer_left"));
    }
}
