//! Slices one interior cell of a real MIR directory in interior mode and
//! writes a picture of the result, to look at the floor bake and the
//! ceiling cut on real data.
//!
//! ```text
//! SLICER_MIR=out/<slice>/mir SLICER_F2=/path/to/fallout2 SLICER_CELL=<editor id> \
//!     cargo run --release -p map-slicer --example interior_check
//! ```
//!
//! - `SLICER_MIR`: the MIR directory (required).
//! - `SLICER_F2`: the Fallout 2 directory holding `master.dat` (required;
//!   only its palette is read).
//! - `SLICER_CELL`: the interior's editor id (required).
//! - `SLICER_FLOOR`: the floor height (default: the most common navmesh
//!   vertex height, rounded).
//! - `SLICER_CUT`: the cut height above the floor (default 210).
//! - `SLICER_CROP`: `x0,y0,x1,y1`, fractions of the picture to keep (to
//!   look closer).
//! - `SLICER_OUT`: output directory, which must sit under the workspace's
//!   ignored `out/` (default `out/interiors/scratch-slicer`).
//! - `SLICER_DECLUTTER`: `readable` compares interior mode without and
//!   with `DeclutterProfile::readable()` instead of interior and plain
//!   (`items` adds a third run with carried items unprotected). Both runs
//!   get the navmesh hexes in the floor band as open hexes, as the map
//!   stage gives them, within `SLICER_WALK=low,high` of the floor (default
//!   `-40,100`). `SLICER_KERB`, `SLICER_SHELL_CUT` override the preset's
//!   kerb and shell cut; `SLICER_BAND=low,high` the floor band.
//! - `SLICER_ZOOM`: draw every pixel n x n (1 to 8), to look closer.
//! - `SLICER_MARKS`: also write `<cell>-<run>-marks.png`, the picture with
//!   every walkable hex marked: green when open, red when hidden behind
//!   upright art, yellow when an object stands on it.
//!
//! Writes `<cell>-interior.png` (interior mode) and `<cell>-plain.png` (the
//! same references sliced as an exterior with no land, as before interior
//! mode), each the floor overlays and then every placement in the engine's
//! order, and prints both reports. Nothing is written outside the output
//! directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[path = "common/navmesh.rs"]
mod navmesh;

use foundry_render::{Calibration, CpuRenderer, Projection};
use std::collections::BTreeSet;

use map_slicer::{
    ArtImage, Budget, DeclutterProfile, InteriorSlice, MirAssets, Placement, SkipReason, Sliced,
    SlicerConfig,
};
use mir::{GroundSampler, MapWindow, Mir};
use quantize::PaletteMap;
use vault_formats::geometry::HexGrid;
use vault_formats::{Dat2Archive, Palette, Vfs};

fn main() {
    if let Err(message) = run() {
        eprintln!("interior_check: {message}");
        std::process::exit(1);
    }
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("set {name}"))
}

fn run() -> Result<(), String> {
    let mir_dir = PathBuf::from(env("SLICER_MIR")?);
    let f2 = PathBuf::from(env("SLICER_F2")?);
    let cell = env("SLICER_CELL")?;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|e| format!("workspace: {e}"))?;
    let out_root = workspace.join("out");
    let out = std::env::var_os("SLICER_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| out_root.join("interiors/scratch-slicer"));
    std::fs::create_dir_all(&out).map_err(|e| format!("create output directory: {e}"))?;
    let out = out.canonicalize().map_err(|e| format!("output: {e}"))?;
    if !out.starts_with(&out_root) {
        return Err("SLICER_OUT must sit under the workspace's out/ directory".into());
    }

    let dat = Dat2Archive::open(f2.join("master.dat")).map_err(|e| format!("master.dat: {e}"))?;
    let palette = Palette::parse(&dat.read("color.pal").map_err(|e| e.to_string())?)
        .map_err(|e| format!("color.pal: {e}"))?;
    let map = PaletteMap::try_new(&palette).map_err(|e| e.to_string())?;

    let mir = Mir::open(&mir_dir).map_err(|e| e.to_string())?;
    let doc = mir
        .interior(&cell)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no interior {cell} in {}", mir_dir.display()))?;
    let vertices: Vec<[f32; 3]> = doc
        .navmeshes
        .iter()
        .flat_map(|n| n.vertices.iter().copied())
        .collect();
    let floor_z = match std::env::var("SLICER_FLOOR") {
        Ok(v) => v
            .trim()
            .parse::<f32>()
            .map_err(|e| format!("SLICER_FLOOR: {e}"))?,
        Err(_) => {
            let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
            for v in &vertices {
                *counts.entry(v[2].round() as i64).or_default() += 1;
            }
            counts
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(z, _)| *z as f32)
                .ok_or("no navmesh to find the floor from; set SLICER_FLOOR")?
        }
    };
    let mut interior = InteriorSlice::at(floor_z);
    if let Ok(v) = std::env::var("SLICER_CUT") {
        interior.cut_height = v.trim().parse().map_err(|e| format!("SLICER_CUT: {e}"))?;
    }
    let pair = |name: &str| -> Result<Option<[f32; 2]>, String> {
        let Ok(text) = std::env::var(name) else {
            return Ok(None);
        };
        let v: Vec<f32> = text
            .split(',')
            .map(|v| v.trim().parse::<f32>().map_err(|e| format!("{name}: {e}")))
            .collect::<Result<_, _>>()?;
        <[f32; 2]>::try_from(v)
            .map(Some)
            .map_err(|_| format!("{name}: two numbers"))
    };
    if let Some(band) = pair("SLICER_BAND")? {
        interior.floor_band = band;
    }
    let walk = pair("SLICER_WALK")?.unwrap_or([-40.0, 100.0]);
    // The window centred on the navmesh (or the references without one).
    let points: Vec<[f32; 2]> = if vertices.is_empty() {
        doc.references
            .iter()
            .map(|r| [r.transform.position[0], r.transform.position[1]])
            .collect()
    } else {
        vertices.iter().map(|v| [v[0], v[1]]).collect()
    };
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in &points {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let calibration = Calibration::default();
    let projection = Projection::new(&calibration).map_err(|e| e.to_string())?;
    let window = MapWindow::from_center(centre, calibration.tile_units());
    println!(
        "{cell}: {} references, floor z {floor_z}, window centre {centre:?}",
        doc.references.len()
    );

    if let Ok(filter) = std::env::var("SLICER_LIST") {
        list_references(&mir, &doc, &projection, &interior, &filter)?;
    }
    let budget = Budget {
        walls: usize::MAX / 4,
        scenery: usize::MAX / 4,
        items: usize::MAX / 4,
    };
    let no_land = GroundSampler::default();
    let assets = MirAssets::new(&mir, SlicerConfig::default().texture_cache_bytes);
    let mut renderer = CpuRenderer::new(&calibration).map_err(|e| e.to_string())?;
    let stem = cell.to_ascii_lowercase();
    // `SLICER_CROP=x0,y0,x1,y1`: fractions of the picture to keep.
    let crop: Option<Vec<f64>> = match std::env::var("SLICER_CROP") {
        Ok(text) => Some(
            text.split(',')
                .map(|v| {
                    v.trim()
                        .parse::<f64>()
                        .map_err(|e| format!("SLICER_CROP: {e}"))
                })
                .collect::<Result<_, _>>()?,
        ),
        Err(_) => None,
    };
    let mode = std::env::var("SLICER_DECLUTTER").unwrap_or_default();
    let marks = std::env::var_os("SLICER_MARKS").is_some();
    let runs: Vec<(&str, SlicerConfig)> = if mode.is_empty() || mode == "off" {
        vec![
            (
                "interior",
                SlicerConfig {
                    interior: Some(interior.clone()),
                    ..SlicerConfig::default()
                },
            ),
            ("plain", SlicerConfig::default()),
        ]
    } else {
        let open = match std::env::var("SLICER_TOL") {
            // As the map stage opens them: within a tolerance of a triangle.
            Ok(v) => {
                let tolerance: f64 = v.trim().parse().map_err(|e| format!("SLICER_TOL: {e}"))?;
                let centres = navmesh::hex_sources(&projection, &window);
                navmesh::open_hexes(
                    std::slice::from_ref(&doc),
                    &centres,
                    &no_land,
                    Some(floor_z),
                    walk,
                    tolerance,
                )
            }
            Err(_) => navmesh_hexes(&doc, &projection, &window, floor_z, walk),
        };
        println!("{} navmesh hexes in the floor band", open.len());
        let base = SlicerConfig {
            interior: Some(interior.clone()),
            open_hexes: open,
            furniture: std::env::var_os("SLICER_FURNITURE")
                .map(|_| map_slicer::FurnitureRule::default()),
            wire: std::env::var_os("SLICER_WIRE").map(|_| map_slicer::WireRule::default()),
            ..SlicerConfig::default()
        };
        let mut profile = DeclutterProfile::readable();
        // `SLICER_OLD=doors,walls`: the cutaway as it was before the door
        // rule and the wall tests, to compare.
        let old = std::env::var("SLICER_OLD").unwrap_or_default();
        if let Some(c) = &mut profile.cutaway {
            if old.contains("doors") {
                c.doors = None;
            }
            if old.contains("walls") {
                c.kinds.clear();
                c.exclude_patterns.clear();
                c.reach_cut = false;
                c.room_height_ratio = 0.0;
            }
        }
        if let Ok(v) = std::env::var("SLICER_KERB")
            && let Some(c) = &mut profile.cutaway
        {
            c.kerb_height = v.trim().parse().map_err(|e| format!("SLICER_KERB: {e}"))?;
        }
        if let Ok(v) = std::env::var("SLICER_SHELL_CUT")
            && let Some(c) = &mut profile.shell_cut
        {
            c.cut_height = v
                .trim()
                .parse()
                .map_err(|e| format!("SLICER_SHELL_CUT: {e}"))?;
        }
        let mut runs = vec![
            ("before", base.clone()),
            (
                "readable",
                SlicerConfig {
                    declutter: Some(profile.clone()),
                    ..base.clone()
                },
            ),
        ];
        if mode == "items" {
            let mut items = profile;
            items
                .protected_kinds
                .retain(|k| !matches!(k.as_str(), "MISC" | "ALCH"));
            runs.push((
                "readable-items",
                SlicerConfig {
                    declutter: Some(items),
                    ..base
                },
            ));
        }
        runs
    };
    for (name, config) in runs {
        let sliced = map_slicer::slice_with(
            &assets,
            &doc.references,
            &window,
            &no_land,
            &projection,
            &mut renderer,
            &map,
            &budget,
            &config,
        )
        .map_err(|e| e.to_string())?;
        println!("\n{name}:\n{}", sliced.report);
        println!("output digest {}", digest(&sliced));
        if let Some(f) = &sliced.report.furniture
            && std::env::var_os("SLICER_FURNITURE_LIST").is_some()
        {
            for c in &f.choices {
                println!(
                    "  furniture {} {} {} {}",
                    c.reason, c.footprint, c.reference, c.model
                );
            }
        }
        if let Ok(text) = std::env::var("SLICER_UNDER") {
            under_report(&sliced, &doc, &config, &projection, &window, &text, &mir);
        }
        // What the interior rules skipped, by model.
        let mut by: BTreeMap<(&str, String), usize> = BTreeMap::new();
        for s in &sliced.skipped {
            if matches!(
                s.reason,
                SkipReason::AboveCut | SkipReason::Hanging | SkipReason::BelowFloor
            ) || s.reason.is_declutter()
            {
                let key = (s.reason.label(), s.model.clone().unwrap_or_default());
                *by.entry(key).or_default() += 1;
            }
        }
        for ((reason, model), n) in &by {
            println!("  {n:>3}  {reason:<24}  {model}");
        }
        let path = out.join(format!("{stem}-{name}.png"));
        picture(&sliced, &palette, &path, crop.as_deref(), None)?;
        println!("picture in {}", path.display());
        if marks && !config.open_hexes.is_empty() {
            let cover = map_slicer::walker_cover(&sliced, &config.open_hexes, [48, 80], 960);
            let occupied: BTreeSet<i32> = sliced
                .placements
                .iter()
                .filter(|p| {
                    !matches!(
                        p.role,
                        map_slicer::Role::WallStrip | map_slicer::Role::FloorPiece
                    )
                })
                .map(|p| p.hex)
                .collect();
            let dots: Vec<(i32, [u8; 4])> = cover
                .walkable
                .iter()
                .map(|&h| {
                    let colour = if cover.hidden.contains(&h) {
                        [230, 40, 40, 255]
                    } else if occupied.contains(&h) {
                        [230, 210, 40, 255]
                    } else {
                        [40, 220, 80, 255]
                    };
                    (h, colour)
                })
                .collect();
            // Who hides the hidden walkers: the placement with the most
            // pixels in each one's box, by model.
            let models: BTreeMap<String, String> = doc
                .references
                .iter()
                .map(|r| (r.form.to_string(), r.model.clone().unwrap_or_default()))
                .collect();
            let mut hiders: BTreeMap<String, usize> = BTreeMap::new();
            for &h in &cover.hidden {
                let Ok(c) = HexGrid::default().hex_center(h) else {
                    continue;
                };
                let b = [c[0] - 24, c[1] - 79, c[0] + 23, c[1]];
                let mut best: Option<(usize, &str)> = None;
                for p in sliced.placements.iter().filter(|p| p.hex > h) {
                    let a = &sliced.art[p.art];
                    let o = origin(a, p);
                    let mut n = 0;
                    for y in b[1].max(o[1])..=b[3].min(o[1] + i64::from(a.height) - 1) {
                        for x in b[0].max(o[0])..=b[2].min(o[0] + i64::from(a.width) - 1) {
                            if a.pixels[((y - o[1]) * i64::from(a.width) + x - o[0]) as usize] != 0
                            {
                                n += 1;
                            }
                        }
                    }
                    if n > 0 && best.is_none_or(|(m, _)| n > m) {
                        best = Some((n, &p.reference));
                    }
                }
                if let Some((_, r)) = best {
                    let model = models.get(r).cloned().unwrap_or_default();
                    *hiders.entry(model).or_default() += 1;
                }
            }
            let mut hiders: Vec<(usize, String)> =
                hiders.into_iter().map(|(m, n)| (n, m)).collect();
            hiders.sort_by(|a, b| b.cmp(a));
            println!("hidden walkers by the model hiding them most:");
            for (n, m) in hiders.iter().take(12) {
                println!("  {n:>4}  {m}");
            }
            let path = out.join(format!("{stem}-{name}-marks.png"));
            picture(&sliced, &palette, &path, crop.as_deref(), Some(&dots))?;
            println!("marks in {}", path.display());
        }
    }
    Ok(())
}

/// `SLICER_LIST=<text>`: one line per reference whose model or kind
/// contains `text` (empty: all): form, kind, class, placed footprint
/// (narrowest width, longest side, hexes), height span above the floor,
/// teleport, model.
fn list_references(
    mir: &Mir,
    doc: &mir::CellDoc,
    projection: &Projection,
    interior: &InteriorSlice,
    filter: &str,
) -> Result<(), String> {
    let assets = MirAssets::new(mir, 0);
    let config = SlicerConfig {
        interior: Some(interior.clone()),
        ..SlicerConfig::default()
    };
    let filter = filter.to_ascii_lowercase();
    println!("form kind class width span hexes z0 z1 teleport masked model");
    for r in &doc.references {
        let model = r.model.clone().unwrap_or_default().to_ascii_lowercase();
        let kind = r.base_kind.clone().unwrap_or_default();
        if !filter.is_empty()
            && !model.contains(&filter)
            && !kind.to_ascii_lowercase().contains(&filter)
        {
            continue;
        }
        let facts = r
            .mesh
            .as_deref()
            .and_then(|h| map_slicer::Assets::mesh(&assets, h).ok())
            .map(|m| map_slicer::MeshFacts::of(&m));
        let class = map_slicer::classify(r, facts.as_ref(), projection.tile_units(), &config);
        let fp = facts
            .and_then(|f| f.bounds)
            .filter(|_| r.transform.is_finite())
            .map(|b| map_slicer::Footprint::of(&r.transform, b));
        let dims = fp.map_or("-".to_owned(), |f| {
            format!(
                "{:.0} {:.0} {:.1} {:.0} {:.0}",
                f.width(),
                f.longest_side(),
                f.hexes(projection.tile_units()),
                f.z[0] - interior.floor_z,
                f.z[1] - interior.floor_z
            )
        });
        let masked = r
            .mesh
            .as_deref()
            .and_then(|h| map_slicer::Assets::mesh(&assets, h).ok())
            .and_then(|m| map_slicer::masked_share(&m))
            .unwrap_or(0.0);
        println!(
            "{} {kind} {:?} {dims} {} {masked:.2} {model}",
            r.form,
            class,
            r.teleport.is_some()
        );
    }
    Ok(())
}

/// Hexes whose centre lies on a navmesh triangle within the walk band
/// (`[low, high]` above the floor).
fn navmesh_hexes(
    doc: &mir::CellDoc,
    projection: &Projection,
    window: &MapWindow,
    floor_z: f32,
    walk: [f32; 2],
) -> BTreeSet<i32> {
    let grid = HexGrid::default();
    let centres: Vec<(i32, [f64; 2])> = (0..40_000)
        .filter_map(|h| {
            let c = grid.hex_center(h).ok()?;
            Some((h, [c[0] as f64, c[1] as f64]))
        })
        .collect();
    let mut out = BTreeSet::new();
    for n in &doc.navmeshes {
        for t in 0..n.triangles.len() {
            let Some(corners) = n.corners(t) else {
                continue;
            };
            if corners
                .iter()
                .any(|c| c[2] < floor_z + walk[0] || c[2] > floor_z + walk[1])
            {
                continue;
            }
            let s = corners.map(|c| {
                let anchor = [window.x_west, window.y_north, c[2]];
                let p = projection.screen(projection.tile_from_source(anchor, c));
                [f64::from(p[0]), f64::from(p[1])]
            });
            let lo = [
                s[0][0].min(s[1][0]).min(s[2][0]),
                s[0][1].min(s[1][1]).min(s[2][1]),
            ];
            let hi = [
                s[0][0].max(s[1][0]).max(s[2][0]),
                s[0][1].max(s[1][1]).max(s[2][1]),
            ];
            let side = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
                (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
            };
            for &(h, p) in &centres {
                if p[0] < lo[0] || p[0] > hi[0] || p[1] < lo[1] || p[1] > hi[1] {
                    continue;
                }
                let d = [
                    side(s[0], s[1], p),
                    side(s[1], s[2], p),
                    side(s[2], s[0], p),
                ];
                if d.iter().all(|v| *v >= 0.0) || d.iter().all(|v| *v <= 0.0) {
                    out.insert(h);
                }
            }
        }
    }
    out
}

/// `SLICER_UNDER=a,b,...`: for every reference whose model contains one of
/// the words, the hexes under its placed footprint (centre inside it or
/// within 0.15 floor squares, as the builder's footprints take them) that
/// are open and not blocked by this run, and the same once the run's
/// footprints are applied; then the walkable floor of the run.
fn under_report(
    sliced: &Sliced,
    doc: &mir::CellDoc,
    config: &SlicerConfig,
    projection: &Projection,
    window: &MapWindow,
    text: &str,
    mir: &Mir,
) {
    let words: Vec<String> = text
        .split(',')
        .map(|w| w.trim().to_ascii_lowercase())
        .collect();
    let centres = navmesh::hex_sources(projection, window);
    let mut blocked: BTreeSet<i32> = sliced.blocked_hexes.iter().copied().collect();
    for p in &sliced.placements {
        if p.blocking {
            blocked.insert(p.hex);
        }
    }
    let reach = 0.15 * f64::from(projection.tile_units());
    let under = |polygon: &[[f64; 2]]| -> Vec<i32> {
        (0..centres.len())
            .filter(|&h| {
                centres[h].is_some_and(|c| inside(polygon, c) || distance(polygon, c) <= reach)
            })
            .map(|h| h as i32)
            .collect()
    };
    let assets = MirAssets::new(mir, 0);
    let mut by_footprint: BTreeSet<i32> = BTreeSet::new();
    let models: BTreeMap<String, String> = doc
        .references
        .iter()
        .map(|r| (r.form.to_string(), r.model.clone().unwrap_or_default()))
        .collect();
    for (reference, corners) in &sliced.footprints {
        let poly: Vec<[f64; 2]> = corners
            .iter()
            .map(|c| [f64::from(c[0]), f64::from(c[1])])
            .collect();
        let hexes = under(&poly);
        let newly = hexes
            .iter()
            .filter(|h| config.open_hexes.contains(h) && !blocked.contains(h))
            .count();
        if newly >= 1 && std::env::var_os("SLICER_FOOTPRINT_LIST").is_some() {
            println!(
                "  footprint {reference} blocks {newly} free hexes: {}",
                models.get(reference).cloned().unwrap_or_default()
            );
        }
        by_footprint.extend(hexes);
    }
    let (mut total, mut free, mut free_after) = (0, 0, 0);
    for r in &doc.references {
        let model = r.model.clone().unwrap_or_default().to_ascii_lowercase();
        if !words
            .iter()
            .any(|w| !w.is_empty() && model.contains(w.as_str()))
        {
            continue;
        }
        let Some(b) = r
            .mesh
            .as_deref()
            .and_then(|h| map_slicer::Assets::mesh(&assets, h).ok())
            .and_then(|m| map_slicer::MeshFacts::of(&m).bounds)
        else {
            continue;
        };
        let poly = map_slicer::Footprint::of(&r.transform, b).polygon;
        let hexes = under(&poly);
        let open: Vec<i32> = hexes
            .iter()
            .copied()
            .filter(|h| config.open_hexes.contains(h))
            .collect();
        let f: Vec<i32> = open
            .iter()
            .copied()
            .filter(|h| !blocked.contains(h))
            .collect();
        let fa = f.iter().filter(|h| !by_footprint.contains(h)).count();
        println!(
            "  under {} {}: {} hexes, {} open, {} free, {} free with the footprints{}",
            r.form,
            model.rsplit('\\').next().unwrap_or(&model),
            hexes.len(),
            open.len(),
            f.len(),
            fa,
            if sliced.footprints.contains_key(&r.form.to_string()) {
                " (footprint given)"
            } else {
                ""
            }
        );
        total += hexes.len();
        free += f.len();
        free_after += fa;
    }
    let walkable: BTreeSet<i32> = config
        .open_hexes
        .iter()
        .copied()
        .filter(|h| !blocked.contains(h))
        .collect();
    let walkable_after: BTreeSet<i32> = walkable
        .iter()
        .copied()
        .filter(|h| !by_footprint.contains(h))
        .collect();
    println!(
        "  under all: {total} hexes, {free} free, {free_after} free with the footprints; walkable floor {} \
         (largest component {}), with the footprints {} (largest component {})",
        walkable.len(),
        largest(&walkable),
        walkable_after.len(),
        largest(&walkable_after)
    );
}

fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let side =
        |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let s: Vec<f64> = (0..n).map(|i| side(poly[i], poly[(i + 1) % n])).collect();
    s.iter().all(|v| *v >= 0.0) || s.iter().all(|v| *v <= 0.0)
}

fn distance(poly: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let len = d[0] * d[0] + d[1] * d[1];
            let t = if len > 0.0 {
                (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            ((p[0] - a[0] - t * d[0]).powi(2) + (p[1] - a[1] - t * d[1]).powi(2)).sqrt()
        })
        .fold(f64::INFINITY, f64::min)
}

/// The largest connected component of `hexes`.
fn largest(hexes: &BTreeSet<i32>) -> usize {
    let mut seen: BTreeSet<i32> = BTreeSet::new();
    let mut best = 0;
    for &start in hexes {
        if !seen.insert(start) {
            continue;
        }
        let mut stack = vec![start];
        let mut n = 0;
        while let Some(h) = stack.pop() {
            n += 1;
            for o in 0..6u8 {
                if let Ok(Some(m)) = HexGrid::neighbor(h, o)
                    && hexes.contains(&m)
                    && seen.insert(m)
                {
                    stack.push(m);
                }
            }
        }
        best = best.max(n);
    }
    best
}

/// SHA-256 over everything a run hands the map builder.
fn digest(sliced: &Sliced) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let n = |h: &mut Sha256, v: i64| h.update(v.to_le_bytes());
    for a in &sliced.art {
        n(&mut h, a.category as i64);
        n(&mut h, i64::from(a.width));
        n(&mut h, i64::from(a.height));
        n(&mut h, i64::from(a.shift[0]));
        n(&mut h, i64::from(a.shift[1]));
        h.update(&a.pixels);
        h.update(a.source.as_bytes());
    }
    for p in &sliced.placements {
        n(&mut h, p.art as i64);
        n(&mut h, i64::from(p.hex));
        n(&mut h, i64::from(p.displacement[0]));
        n(&mut h, i64::from(p.displacement[1]));
        n(&mut h, p.role as i64);
        h.update(p.reference.as_bytes());
        n(&mut h, i64::from(p.blocking));
        let run = p.run.unwrap_or((-1, -1));
        n(&mut h, i64::from(run.0));
        n(&mut h, i64::from(run.1));
    }
    for o in &sliced.overlays {
        n(&mut h, i64::from(o.origin[0]));
        n(&mut h, i64::from(o.origin[1]));
        n(&mut h, i64::from(o.width));
        n(&mut h, i64::from(o.height));
        h.update(&o.rgba);
    }
    for b in &sliced.blocked_hexes {
        n(&mut h, i64::from(*b));
    }
    for s in &sliced.skipped {
        h.update(s.reference.as_bytes());
        h.update(s.reason.label().as_bytes());
    }
    let d = h.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Engine placement of a frame's pixel (0, 0).
fn origin(a: &ArtImage, p: &Placement) -> [i64; 2] {
    let c = HexGrid::default().hex_center(p.hex).unwrap_or([0, 0]);
    [
        c[0] + i64::from(a.shift[0]) + i64::from(p.displacement[0]) - i64::from(a.width / 2),
        c[1] + i64::from(a.shift[1]) + i64::from(p.displacement[1]) - (i64::from(a.height) - 1),
    ]
}

/// The overlays, then every placement in the engine's order, cropped to
/// what was drawn.
fn picture(
    sliced: &Sliced,
    palette: &Palette,
    path: &Path,
    crop: Option<&[f64]>,
    dots: Option<&[(i32, [u8; 4])]>,
) -> Result<(), String> {
    let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    let mut grow = |x0: i64, y0: i64, w: i64, h: i64| {
        b = [
            b[0].min(x0),
            b[1].min(y0),
            b[2].max(x0 + w),
            b[3].max(y0 + h),
        ];
    };
    for o in &sliced.overlays {
        grow(
            i64::from(o.origin[0]),
            i64::from(o.origin[1]),
            i64::from(o.width),
            i64::from(o.height),
        );
    }
    for p in &sliced.placements {
        let a = &sliced.art[p.art];
        let o = origin(a, p);
        grow(o[0], o[1], i64::from(a.width), i64::from(a.height));
    }
    if b[0] >= b[2] {
        return Err("nothing drawn".into());
    }
    // `SLICER_PROBE=fx,fy`: list what is drawn at that point (fractions of
    // the whole picture, as for the crop).
    if let Ok(text) = std::env::var("SLICER_PROBE") {
        let f: Vec<f64> = text
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        if let [fx, fy] = f[..] {
            let (w, h) = ((b[2] - b[0]) as f64, (b[3] - b[1]) as f64);
            let (px, py) = (b[0] + (w * fx) as i64, b[1] + (h * fy) as i64);
            println!("probe ({px}, {py}):");
            for p in &sliced.placements {
                let a = &sliced.art[p.art];
                let o = origin(a, p);
                let (x, y) = (px - o[0], py - o[1]);
                if x >= 0
                    && y >= 0
                    && x < i64::from(a.width)
                    && y < i64::from(a.height)
                    && a.pixels[(y * i64::from(a.width) + x) as usize] != 0
                {
                    println!(
                        "  {:?} hex {} {} {} ({})",
                        p.role,
                        p.hex,
                        p.reference,
                        a.source,
                        if p.blocking { "blocks" } else { "open" }
                    );
                }
            }
        }
    }
    if let Some([fx0, fy0, fx1, fy1]) = crop.and_then(|c| <[f64; 4]>::try_from(c).ok()) {
        let (w, h) = ((b[2] - b[0]) as f64, (b[3] - b[1]) as f64);
        b = [
            b[0] + (w * fx0) as i64,
            b[1] + (h * fy0) as i64,
            b[0] + (w * fx1) as i64,
            b[1] + (h * fy1) as i64,
        ];
    }
    let (x0, y0) = (b[0] - 16, b[1] - 16);
    let (w, h) = ((b[2] - b[0] + 32) as usize, (b[3] - b[1] + 32) as usize);
    let mut rgba = [24u8, 26, 30, 255].repeat(w * h);
    let mut put = |x: i64, y: i64, c: [u8; 4]| {
        let (i, j) = (x - x0, y - y0);
        if i >= 0 && j >= 0 && (i as usize) < w && (j as usize) < h {
            let k = (j as usize * w + i as usize) * 4;
            rgba[k..k + 4].copy_from_slice(&c);
        }
    };
    for o in &sliced.overlays {
        for y in 0..o.height as i64 {
            for x in 0..o.width as i64 {
                let k = ((y * i64::from(o.width) + x) * 4) as usize;
                if o.rgba[k + 3] != 0 {
                    put(
                        i64::from(o.origin[0]) + x,
                        i64::from(o.origin[1]) + y,
                        [o.rgba[k], o.rgba[k + 1], o.rgba[k + 2], 255],
                    );
                }
            }
        }
    }
    let colours = palette.rgba8();
    for p in &sliced.placements {
        let a = &sliced.art[p.art];
        let o = origin(a, p);
        for y in 0..a.height as i64 {
            for x in 0..a.width as i64 {
                let v = a.pixels[(y * i64::from(a.width) + x) as usize];
                if v != 0 {
                    put(o[0] + x, o[1] + y, colours[usize::from(v)]);
                }
            }
        }
    }
    for &(hex, colour) in dots.unwrap_or(&[]) {
        let Ok(c) = HexGrid::default().hex_center(hex) else {
            continue;
        };
        for dy in -2..=2 {
            for dx in -3..=3 {
                put(c[0] + dx, c[1] + dy, colour);
            }
        }
    }
    // `SLICER_ZOOM=n`: every pixel n x n, to look closer.
    let zoom: usize = std::env::var("SLICER_ZOOM")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|z| (1..=8).contains(z))
        .unwrap_or(1);
    let (rgba, w, h) = if zoom == 1 {
        (rgba, w, h)
    } else {
        let mut big = vec![0u8; w * h * zoom * zoom * 4];
        for y in 0..h * zoom {
            for x in 0..w * zoom {
                let (k, j) = ((y / zoom * w + x / zoom) * 4, (y * w * zoom + x) * 4);
                big[j..j + 4].copy_from_slice(&rgba[k..k + 4]);
            }
        }
        (big, w * zoom, h * zoom)
    };
    let png = foundry_gallery::encode_png(&rgba, w as u32, h as u32).map_err(|e| e.to_string())?;
    std::fs::write(path, png).map_err(|e| format!("{}: {e}", path.display()))
}
