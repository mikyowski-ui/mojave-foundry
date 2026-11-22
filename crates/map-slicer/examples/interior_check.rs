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
//!
//! Writes `<cell>-interior.png` (interior mode) and `<cell>-plain.png` (the
//! same references sliced as an exterior with no land, as before interior
//! mode), each the floor overlays and then every placement in the engine's
//! order, and prints both reports. Nothing is written outside the output
//! directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use foundry_render::{Calibration, CpuRenderer, Projection};
use map_slicer::{
    ArtImage, Budget, InteriorSlice, MirAssets, Placement, SkipReason, Sliced, SlicerConfig,
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
    for (name, config) in [
        (
            "interior",
            SlicerConfig {
                interior: Some(interior.clone()),
                ..SlicerConfig::default()
            },
        ),
        ("plain", SlicerConfig::default()),
    ] {
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
            ) {
                let key = (s.reason.label(), s.model.clone().unwrap_or_default());
                *by.entry(key).or_default() += 1;
            }
        }
        for ((reason, model), n) in &by {
            println!("  {n:>3}  {reason:<24}  {model}");
        }
        let path = out.join(format!("{stem}-{name}.png"));
        picture(&sliced, &palette, &path, crop.as_deref())?;
        println!("picture in {}", path.display());
    }
    Ok(())
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
    let png = foundry_gallery::encode_png(&rgba, w as u32, h as u32).map_err(|e| e.to_string())?;
    std::fs::write(path, png).map_err(|e| format!("{}: {e}", path.display()))
}
