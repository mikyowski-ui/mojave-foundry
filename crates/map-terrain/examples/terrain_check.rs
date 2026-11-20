//! Builds one map's ground from a real MIR directory and writes the
//! composed floor for inspection.
//!
//! ```text
//! TERRAIN_MIR=out/goodsprings/mir TERRAIN_F2=/path/to/fallout2 \
//!     cargo run --release -p map-terrain --example terrain_check
//! ```
//!
//! - `TERRAIN_MIR`: the MIR directory (required);
//! - `TERRAIN_F2`: the Fallout 2 install holding `master.dat` (required);
//! - `TERRAIN_WORLDSPACE` (default `WastelandNV`);
//! - `TERRAIN_CENTER` as `x,y` source units (default `-71680,2048`);
//! - `TERRAIN_CAP` (2000), `TERRAIN_REPEAT` (512), `TERRAIN_JOBS` (4),
//!   `TERRAIN_RELIEF` (1), `TERRAIN_DITHER` (default strength),
//!   `TERRAIN_SNAP` (`true`: snap the repeat to whole squares),
//!   `TERRAIN_SEAM` (1, the seam weight);
//! - `TERRAIN_CROPS`: `name:x:y` list of 800 × 600 crops at map screen
//!   positions (default three across the map);
//! - `TERRAIN_OUT` (default `out/terrain-check` under the workspace), which
//!   must sit under the workspace's `out/`.
//!
//! Writes `ground.png` (the whole floor), `overview.png` (a quarter-size
//! box-filtered view), `crop-*.png` (full-size details) and `stats.txt`.

use std::path::{Path, PathBuf};

use foundry_render::{Calibration, Projection};
use map_terrain::{TerrainOptions, build_ground};
use mir::{GroundSampler, MapWindow, Mir};
use quantize::PaletteMap;
use vault_formats::{Dat2Archive, Palette, Vfs};

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn parse<T: std::str::FromStr>(key: &str, default: T) -> Result<T, String> {
    match env(key) {
        Some(v) => v.parse().map_err(|_| format!("{key}: cannot parse {v:?}")),
        None => Ok(default),
    }
}

fn write_png(path: &Path, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
    let bytes = foundry_gallery::encode_png(rgba, width, height).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn run() -> Result<(), String> {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workspace = workspace.canonicalize().map_err(|e| e.to_string())?;
    let mir_dir = PathBuf::from(env("TERRAIN_MIR").ok_or("set TERRAIN_MIR")?);
    let f2 = PathBuf::from(env("TERRAIN_F2").ok_or("set TERRAIN_F2")?);
    let out = env("TERRAIN_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("out/terrain-check"));
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let out = out.canonicalize().map_err(|e| e.to_string())?;
    if !out.starts_with(workspace.join("out")) {
        return Err("TERRAIN_OUT must sit under the workspace's out/ directory".into());
    }
    let worldspace = env("TERRAIN_WORLDSPACE").unwrap_or_else(|| "WastelandNV".into());
    let center: Vec<f32> = env("TERRAIN_CENTER")
        .unwrap_or_else(|| "-71680,2048".into())
        .split(',')
        .map(|v| {
            v.trim()
                .parse()
                .map_err(|_| format!("TERRAIN_CENTER: {v:?}"))
        })
        .collect::<Result<_, _>>()?;
    let [cx, cy] = center[..] else {
        return Err("TERRAIN_CENTER needs x,y".into());
    };
    let options = TerrainOptions {
        library_cap: parse("TERRAIN_CAP", 2000)?,
        texture_repeat_units: parse("TERRAIN_REPEAT", 512.0)?,
        jobs: parse("TERRAIN_JOBS", 4)?,
        snap_repeat_to_tiles: parse("TERRAIN_SNAP", true)?,
        seam_weight: parse("TERRAIN_SEAM", 1.0)?,
        relief: parse("TERRAIN_RELIEF", 1.0)?,
        dither_strength: parse("TERRAIN_DITHER", quantize::Dither::DEFAULT_STRENGTH)?,
        ..TerrainOptions::default()
    };

    let dat = Dat2Archive::open(f2.join("master.dat")).map_err(|e| format!("master.dat: {e}"))?;
    let palette = Palette::parse(&dat.read("color.pal").map_err(|e| e.to_string())?)
        .map_err(|e| format!("color.pal: {e}"))?;
    let map = PaletteMap::new(&palette);
    let mir = Mir::open(&mir_dir).map_err(|e| e.to_string())?;
    let calibration = Calibration::default();
    let projection = Projection::new(&calibration).map_err(|e| e.to_string())?;
    let window = MapWindow::from_center([cx, cy], calibration.tile_units());
    let sampler = GroundSampler::load(&mir, &worldspace, &window).map_err(|e| e.to_string())?;

    let started = std::time::Instant::now();
    let ground = build_ground(
        &mir,
        &worldspace,
        &window,
        &sampler,
        &projection,
        &calibration,
        &map,
        &[],
        &options,
    )
    .map_err(|e| e.to_string())?;
    let total = started.elapsed();

    let (indexed, width, height) = ground.compose();
    let rgba = quantize::to_rgba(&indexed, &palette);
    write_png(&out.join("ground.png"), &rgba, width, height)?;
    // Quarter-size overview, box-filtered over opaque pixels.
    let (ow, oh) = (width / 4, height / 4);
    let mut overview = vec![0u8; (ow * oh * 4) as usize];
    for y in 0..oh {
        for x in 0..ow {
            let mut sum = [0u32; 4];
            for dy in 0..4 {
                for dx in 0..4 {
                    let at = (((y * 4 + dy) * width + x * 4 + dx) * 4) as usize;
                    if rgba[at + 3] != 0 {
                        for k in 0..3 {
                            sum[k] += u32::from(rgba[at + k]);
                        }
                        sum[3] += 1;
                    }
                }
            }
            let at = ((y * ow + x) * 4) as usize;
            for k in 0..3 {
                if let Some(mean) = sum[k].checked_div(sum[3]) {
                    overview[at + k] = mean as u8;
                    overview[at + 3] = 255;
                }
            }
        }
    }
    write_png(&out.join("overview.png"), &overview, ow, oh)?;
    let crops = env("TERRAIN_CROPS")
        .unwrap_or_else(|| "centre:3584:500,west:1184:200,east:5984:900".into());
    for spec in crops.split(',') {
        let parts: Vec<&str> = spec.split(':').collect();
        let [name, x, y] = parts[..] else {
            return Err(format!("TERRAIN_CROPS: {spec:?} is not name:x:y"));
        };
        let at = |v: &str| {
            v.parse::<i32>()
                .map_err(|_| format!("TERRAIN_CROPS: {v:?}"))
        };
        let crop = ground
            .compose_rect([at(x)?, at(y)?], 800, 600)
            .map_err(|e| e.to_string())?;
        let rgba = quantize::to_rgba(&crop, &palette);
        write_png(&out.join(format!("crop-{name}.png")), &rgba, 800, 600)?;
    }

    let s = &ground.stats;
    let text = format!(
        "options: cap {} repeat {} (used {}, phase period {:?}) relief {} dither {} seam {} jobs {}\n\
         tiles before {} distinct {} exact duplicates {} clusters {} after {}\n\
         substituted squares {} mean error {:.4} worst error {:.4}\n\
         fallback pixels {} missing-texture pixels {} missing layers {}\n\
         textures loaded {} missing {} cells with land {} without {}\n\
         load {:.2?} evaluate {:.2?} deduplicate {:.2?} total {:.2?}\n",
        options.library_cap,
        options.texture_repeat_units,
        s.texture_repeat_used,
        s.phase_period,
        options.relief,
        options.dither_strength,
        options.seam_weight,
        options.jobs,
        s.tiles_before,
        s.distinct_tiles,
        s.exact_duplicates,
        s.clusters,
        s.tiles_after,
        s.substituted_squares,
        s.mean_substitution_error,
        s.worst_substitution_error,
        s.fallback_pixels,
        s.missing_texture_pixels,
        s.missing_texture_layers,
        s.textures_loaded,
        s.textures_missing,
        s.cells_with_land,
        s.cells_without_land,
        s.load_time,
        s.evaluate_time,
        s.deduplicate_time,
        total,
    );
    std::fs::write(out.join("stats.txt"), &text).map_err(|e| e.to_string())?;
    print!("{text}");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("terrain_check: {error}");
        std::process::exit(1);
    }
}
