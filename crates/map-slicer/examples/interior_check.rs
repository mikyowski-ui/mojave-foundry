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
        let open = navmesh_hexes(&doc, &projection, &window, floor_z, walk);
        println!("{} navmesh hexes in the floor band", open.len());
        let base = SlicerConfig {
            interior: Some(interior.clone()),
            open_hexes: open,
            ..SlicerConfig::default()
        };
        let mut profile = DeclutterProfile::readable();
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
