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
//!   `TERRAIN_RELIEF` (1), `TERRAIN_VERTEX_FLOOR` (0), `TERRAIN_DITHER` (default strength),
//!   `TERRAIN_SNAP` (`true`: snap the repeat to whole squares),
//!   `TERRAIN_SEAM` (1, the seam weight);
//!   `TERRAIN_LOD` (0, the texture LOD bias; the map stage uses -1),
//!   `TERRAIN_ITERATIONS` (8, Lloyd iterations), `TERRAIN_METHOD`
//!   (`kmeans` or `greedy`, the library method; default `kmeans`),
//!   `TERRAIN_MISSING_RGB` (`r,g,b`: draw missing textures in this flat
//!   tone instead of from their neighbours, to see the holes);
//! - `TERRAIN_SYNTH_OVERLAYS` (0): this many synthetic stand-ins for baked
//!   overlays (road pieces and rocks; the example cannot run the slicer),
//!   composited over the ground; the analysis reports the squares under
//!   them apart;
//! - `TERRAIN_ANALYSE` (`false`): also build the ground with no cap, and
//!   report where the substitution error sits (by texture detail, by colour
//!   step to the neighbours) and write `error.png`, a quarter-size heat map
//!   of the per-square error (black is exact, full red is 0.08 or more);
//! - `TERRAIN_CROPS`: `name:x:y` list of 800 × 600 crops at map screen
//!   positions (default three across the map);
//! - `TERRAIN_COMPARE` (`other.png`, or `other.png:a.png:b.png`): compare
//!   this run's `ground.png` with another run's, pixel by pixel, and report
//!   the pixels and squares that differ. With `a.png` and `b.png` (two runs
//!   that differ only in where they may differ, such as two
//!   `TERRAIN_MISSING_RGB` tones), also report how many of the differing
//!   pixels and squares fall outside the pixels where those two differ;
//! - `TERRAIN_OUT` (default `out/terrain-check` under the workspace), which
//!   must sit under the workspace's `out/`.
//!
//! Writes `ground.png` (the whole floor), `overview.png` (a quarter-size
//! box-filtered view), `crop-*.png` (full-size details) and `stats.txt`,
//! which ends with a 64-bit FNV-1a digest of the library and the square
//! list, to compare runs byte for byte.

use std::path::{Path, PathBuf};

use foundry_render::{Calibration, Projection};
use map_terrain::{
    Ground, LibraryMethod, Overlay, SQUARES, TILE_HEIGHT, TILE_WIDTH, TerrainOptions, build_ground,
};
use mir::{GroundSampler, MapWindow, Mir};
use quantize::PaletteMap;
use vault_formats::geometry::HexGrid;
use vault_formats::{Dat2Archive, Palette, Vfs};

/// FNV-1a over the library tiles and the square entries.
fn digest(ground: &Ground) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for b in (ground.tiles.len() as u64).to_le_bytes() {
        eat(b);
    }
    for tile in &ground.tiles {
        tile.iter().copied().for_each(&mut eat);
    }
    for entry in &ground.squares {
        entry.to_le_bytes().into_iter().for_each(&mut eat);
    }
    h
}

/// Quarter-size box filter over opaque pixels.
fn quarter(rgba: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
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
    (overview, ow, oh)
}

fn lab_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
}

/// Where the substitution error of `ground` sits, against `exact` (the
/// same ground with no cap). Returns the report and the heat map.
fn analyse(
    ground: &Ground,
    exact: &Ground,
    map: &PaletteMap,
    overlaid: &[bool],
) -> (String, Vec<u8>, u32, u32) {
    let own = |q: usize| &exact.tiles[usize::from(exact.squares[q])];
    let used = |q: usize| &ground.tiles[usize::from(ground.squares[q])];
    let mut error = vec![0.0f64; SQUARES];
    let mut detail = vec![0.0f64; SQUARES];
    let mut means = vec![[0.0f64; 3]; SQUARES];
    let mut used_means = vec![[0.0f64; 3]; SQUARES];
    for q in 0..SQUARES {
        let (a, b) = (own(q), used(q));
        let mut sum = 0.0;
        let mut n = 0usize;
        let mut mean = [0.0f64; 3];
        let mut used_mean = [0.0f64; 3];
        for (&pa, &pb) in a.iter().zip(b.iter()) {
            if pa == 0 {
                continue;
            }
            sum += lab_distance(map.oklab(pa), map.oklab(pb));
            let (la, lb) = (map.oklab(pa), map.oklab(pb));
            for k in 0..3 {
                mean[k] += la[k];
                used_mean[k] += lb[k];
            }
            n += 1;
        }
        let n = n.max(1) as f64;
        error[q] = sum / n;
        means[q] = mean.map(|v| v / n);
        used_means[q] = used_mean.map(|v| v / n);
        detail[q] = a
            .iter()
            .filter(|&&p| p != 0)
            .map(|&p| lab_distance(map.oklab(p), means[q]))
            .sum::<f64>()
            / n;
    }
    // The largest step in mean colour to a neighbour: blend edges and
    // borders between grounds.
    let step: Vec<f64> = (0..SQUARES)
        .map(|q| {
            let column = q % 100;
            let mut best = 0.0f64;
            for (ok, n) in [
                (column != 99, q as i64 + 1),
                (column != 0, q as i64 - 1),
                (true, q as i64 + 100),
                (true, q as i64 - 100),
            ] {
                if ok && (0..SQUARES as i64).contains(&n) {
                    best = best.max(lab_distance(means[q], means[n as usize]));
                }
            }
            best
        })
        .collect();
    let total: f64 = error.iter().sum();
    let mut text = String::new();
    let mut sorted = error.clone();
    sorted.sort_by(f64::total_cmp);
    let pct = |p: f64| sorted[((SQUARES - 1) as f64 * p) as usize];
    text.push_str(&format!(
        "error percentiles: p50 {:.4} p90 {:.4} p99 {:.4} max {:.4}; squares over 0.03 {}, over 0.05 {}\n",
        pct(0.5),
        pct(0.9),
        pct(0.99),
        pct(1.0),
        error.iter().filter(|&&e| e > 0.03).count(),
        error.iter().filter(|&&e| e > 0.05).count(),
    ));
    // Patchiness: how far the colour step between neighbouring squares
    // (tile means) moved from the step between their own tiles.
    let mut seams = Vec::new();
    for q in 0..SQUARES {
        for n in [q + 1, q + 100] {
            if n >= SQUARES || (n == q + 1 && q % 100 == 99) {
                continue;
            }
            let now: Vec<f64> = (0..3)
                .map(|k| used_means[q][k] - used_means[n][k])
                .collect();
            let was: Vec<f64> = (0..3).map(|k| means[q][k] - means[n][k]).collect();
            seams.push(lab_distance(
                [now[0], now[1], now[2]],
                [was[0], was[1], was[2]],
            ));
        }
    }
    let seam_mean = seams.iter().sum::<f64>() / seams.len().max(1) as f64;
    seams.sort_by(f64::total_cmp);
    text.push_str(&format!(
        "seam step error: mean {:.4} p99 {:.4} max {:.4}\n",
        seam_mean,
        seams[(seams.len() - 1) * 99 / 100],
        seams[seams.len() - 1],
    ));
    let marked = overlaid.iter().filter(|&&o| o).count();
    if marked > 0 {
        for (name, flag) in [("under overlays", true), ("elsewhere", false)] {
            let set: Vec<usize> = (0..SQUARES).filter(|&q| overlaid[q] == flag).collect();
            let sum: f64 = set.iter().map(|&q| error[q]).sum();
            let worst = set.iter().map(|&q| error[q]).fold(0.0, f64::max);
            let substituted = set
                .iter()
                .filter(|&&q| exact.tiles[usize::from(exact.squares[q])] != *used(q))
                .count();
            text.push_str(&format!(
                "squares {name}: {}, substituted {}, mean error {:.4}, worst {:.4}\n",
                set.len(),
                substituted,
                sum / set.len().max(1) as f64,
                worst,
            ));
        }
    }
    for (name, key) in [("detail", &detail), ("neighbour step", &step)] {
        let mut order: Vec<usize> = (0..SQUARES).collect();
        order.sort_by(|&a, &b| key[a].total_cmp(&key[b]).then(a.cmp(&b)));
        text.push_str(&format!("by {name} (quartiles, low to high):\n"));
        for part in order.chunks(SQUARES / 4) {
            let sum: f64 = part.iter().map(|&q| error[q]).sum();
            let substituted = part
                .iter()
                .filter(|&&q| exact.tiles[usize::from(exact.squares[q])] != *used(q))
                .count();
            text.push_str(&format!(
                "  {name} {:.4}..{:.4}: mean error {:.4}, share of error {:.1} %, substituted {}\n",
                key[part[0]],
                key[part[part.len() - 1]],
                sum / part.len() as f64,
                100.0 * sum / total.max(1e-12),
                substituted,
            ));
        }
    }
    // Heat map.
    let (origin, width, height) = Ground::bounds();
    let grid = HexGrid::default();
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for (q, &e) in error.iter().enumerate() {
        let Some(corner) = grid.square_corner(q as i32, false) else {
            continue;
        };
        let red = (e / 0.08 * 255.0).clamp(0.0, 255.0) as u8;
        let tile = own(q);
        for py in 0..TILE_HEIGHT as i64 {
            for px in 0..TILE_WIDTH as i64 {
                if tile[(py * i64::from(TILE_WIDTH) + px) as usize] == 0 {
                    continue;
                }
                let x = corner[0] + px - i64::from(origin[0]);
                let y = corner[1] + py - i64::from(origin[1]);
                if x < 0 || y < 0 || x >= i64::from(width) || y >= i64::from(height) {
                    continue;
                }
                let at = ((y * i64::from(width) + x) * 4) as usize;
                rgba[at] = red;
                rgba[at + 3] = 255;
            }
        }
    }
    let (heat, hw, hh) = quarter(&rgba, width, height);
    (text, heat, hw, hh)
}

/// An owned overlay image: origin, width, height, RGBA.
type OwnedOverlay = ([i32; 2], u32, u32, Vec<u8>);

/// Deterministic stand-ins for baked overlays: alternately a road piece
/// (a grained band 28 px wide along a square diagonal) and a rock (a
/// shaded disc), each centred on a pseudo-random square of the map's
/// middle.
fn synthetic_overlays(count: usize) -> Vec<OwnedOverlay> {
    let grid = HexGrid::default();
    let mut state = 0x2545_f491_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut out = Vec::new();
    for i in 0..count {
        let q = (10 + next() % 80) * 100 + 10 + next() % 80;
        let Some(corner) = grid.square_corner(q as i32, false) else {
            continue;
        };
        let centre = [corner[0] as i32 + 40, corner[1] as i32 + 18];
        if i % 2 == 0 {
            let (w, h) = (480u32, 160u32);
            // Along the s axis on screen: (48, -12) per square, or t.
            let dir = if next() % 2 == 0 {
                [48.0f32, -12.0]
            } else {
                [32.0, 24.0]
            };
            let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
            let normal = [-dir[1] / len, dir[0] / len];
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            for y in 0..h {
                for x in 0..w {
                    let dx = x as f32 - w as f32 / 2.0;
                    let dy = y as f32 - h as f32 / 2.0;
                    let across = (dx * normal[0] + dy * normal[1]).abs();
                    if across > 14.0 {
                        continue;
                    }
                    let grain = (next() % 31) as i32 - 15;
                    let edge = if across > 11.0 { -30 } else { 0 };
                    let at = ((y * w + x) * 4) as usize;
                    for (k, base) in [176i32, 152, 112].into_iter().enumerate() {
                        rgba[at + k] = (base + grain + edge).clamp(0, 255) as u8;
                    }
                    rgba[at + 3] = 255;
                }
            }
            out.push((
                [centre[0] - w as i32 / 2, centre[1] - h as i32 / 2],
                w,
                h,
                rgba,
            ));
        } else {
            let r = 20 + (next() % 41) as i32;
            let side = (2 * r + 1) as u32;
            let mut rgba = vec![0u8; (side * side * 4) as usize];
            for y in 0..side as i32 {
                for x in 0..side as i32 {
                    let (dx, dy) = (x - r, y - r);
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    // Lit from the upper left.
                    let light = 140 - (dx + dy) * 50 / r + (next() % 21) as i32 - 10;
                    let at = ((y as u32 * side + x as u32) * 4) as usize;
                    for k in 0..3 {
                        rgba[at + k] = (light - 6 * k as i32).clamp(0, 255) as u8;
                    }
                    rgba[at + 3] = 255;
                }
            }
            out.push(([centre[0] - r, centre[1] - r], side, side, rgba));
        }
    }
    out
}

/// Squares with an opaque overlay pixel on them.
fn overlaid_squares(overlays: &[OwnedOverlay]) -> Vec<bool> {
    let grid = HexGrid::default();
    let mut out = vec![false; SQUARES];
    for (origin, w, h, rgba) in overlays {
        for y in 0..*h as i32 {
            for x in 0..*w as i32 {
                if rgba[((y * *w as i32 + x) * 4 + 3) as usize] == 0 {
                    continue;
                }
                if let Some(q) = grid.pick_square([origin[0] + x, origin[1] + y], false) {
                    out[q as usize] = true;
                }
            }
        }
    }
    out
}

fn read_png(path: &str, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let image = foundry_gallery::decode_png(&bytes).map_err(|e| format!("{path}: {e}"))?;
    if (image.width, image.height) != (width, height) {
        return Err(format!(
            "{path}: {} x {}, not {width} x {height}",
            image.width, image.height
        ));
    }
    Ok(image.rgba)
}

/// See `TERRAIN_COMPARE` in the module documentation.
fn compare(rgba: &[u8], width: u32, height: u32, spec: &str) -> Result<String, String> {
    let paths: Vec<&str> = spec.split(':').collect();
    let (other, allowed) = match paths[..] {
        [other] => (read_png(other, width, height)?, None),
        [other, a, b] => (
            read_png(other, width, height)?,
            Some((read_png(a, width, height)?, read_png(b, width, height)?)),
        ),
        _ => {
            return Err(format!(
                "TERRAIN_COMPARE: {spec:?} is not other.png[:a.png:b.png]"
            ));
        }
    };
    let (origin, _, _) = Ground::bounds();
    let grid = HexGrid::default();
    let square_of = |i: usize| {
        let x = origin[0] + (i % width as usize) as i32;
        let y = origin[1] + (i / width as usize) as i32;
        grid.pick_square([x, y], false)
    };
    fn pixel(buf: &[u8], i: usize) -> &[u8] {
        &buf[i * 4..i * 4 + 4]
    }
    let mut differ = 0u64;
    let mut outside = 0u64;
    let mut allowed_pixels = 0u64;
    let mut squares = std::collections::BTreeSet::new();
    let mut allowed_squares = std::collections::BTreeSet::new();
    for i in 0..(width as usize * height as usize) {
        let may = allowed
            .as_ref()
            .is_some_and(|(a, b)| pixel(a, i) != pixel(b, i));
        if may {
            allowed_pixels += 1;
            allowed_squares.extend(square_of(i));
        }
        if pixel(rgba, i) != pixel(&other, i) {
            differ += 1;
            squares.extend(square_of(i));
            if allowed.is_some() && !may {
                outside += 1;
            }
        }
    }
    let mut text = format!(
        "compare: {differ} pixels differ, in {} squares\n",
        squares.len()
    );
    if allowed.is_some() {
        text.push_str(&format!(
            "compare: {allowed_pixels} pixels in {} squares may differ; {outside} differing pixels \
             and {} differing squares fall outside them\n",
            allowed_squares.len(),
            squares.difference(&allowed_squares).count(),
        ));
    }
    Ok(text)
}

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
        vertex_color_floor: parse("TERRAIN_VERTEX_FLOOR", 0.0)?,
        dither_strength: parse("TERRAIN_DITHER", quantize::Dither::DEFAULT_STRENGTH)?,
        texture_lod_bias: parse("TERRAIN_LOD", 0.0)?,
        cluster_iterations: parse("TERRAIN_ITERATIONS", 8)?,
        missing_texture_rgb: match env("TERRAIN_MISSING_RGB") {
            None => None,
            Some(text) => {
                let parts: Vec<u8> = text
                    .split(',')
                    .map(|v| v.trim().parse::<u8>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| format!("TERRAIN_MISSING_RGB: {text:?} is not r,g,b"))?;
                let [r, g, b] = parts[..] else {
                    return Err(format!("TERRAIN_MISSING_RGB: {text:?} is not r,g,b"));
                };
                Some([r, g, b])
            }
        },
        library_method: match env("TERRAIN_METHOD").as_deref() {
            None | Some("kmeans") => LibraryMethod::KMeans,
            Some("greedy") => LibraryMethod::GreedyMerge,
            Some(other) => {
                return Err(format!("TERRAIN_METHOD: {other:?} is not kmeans or greedy"));
            }
        },
        ..TerrainOptions::default()
    };
    let analyse_errors: bool = parse("TERRAIN_ANALYSE", false)?;
    let owned = synthetic_overlays(parse("TERRAIN_SYNTH_OVERLAYS", 0)?);
    let overlays: Vec<Overlay> = owned
        .iter()
        .map(|(origin, width, height, rgba)| Overlay {
            origin: *origin,
            width: *width,
            height: *height,
            rgba,
        })
        .collect();

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
        &overlays,
        &options,
    )
    .map_err(|e| e.to_string())?;
    let total = started.elapsed();

    let (indexed, width, height) = ground.compose();
    let rgba = quantize::to_rgba(&indexed, &palette);
    write_png(&out.join("ground.png"), &rgba, width, height)?;
    let comparison = match env("TERRAIN_COMPARE") {
        Some(spec) => compare(&rgba, width, height, &spec)?,
        None => String::new(),
    };
    let (overview, ow, oh) = quarter(&rgba, width, height);
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
    let mut text = format!(
        "options: cap {} method {:?} repeat {} (used {}, phase period {:?}) relief {} dither {} seam {} jobs {}\n\
         tiles before {} distinct {} exact duplicates {} clusters {} after {}\n\
         substituted squares {} mean error {:.4} worst error {:.4}\n\
         fallback pixels {} missing-texture pixels {} missing layers {} (naming no file {}, bases borrowed {})\n\
         defaulted-texture pixels {} defaulted layers {}\n\
         textures loaded {} missing {} cells with land {} without {}\n\
         load {:.2?} evaluate {:.2?} deduplicate {:.2?} total {:.2?}\n",
        options.library_cap,
        options.library_method,
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
        s.unnamed_texture_layers,
        s.borrowed_base_layers,
        s.defaulted_texture_pixels,
        s.defaulted_texture_layers,
        s.textures_loaded,
        s.textures_missing,
        s.cells_with_land,
        s.cells_without_land,
        s.load_time,
        s.evaluate_time,
        s.deduplicate_time,
        total,
    );
    if analyse_errors {
        let exact = build_ground(
            &mir,
            &worldspace,
            &window,
            &sampler,
            &projection,
            &calibration,
            &map,
            &overlays,
            &TerrainOptions {
                library_cap: SQUARES,
                ..options.clone()
            },
        )
        .map_err(|e| e.to_string())?;
        let (report, heat, hw, hh) = analyse(&ground, &exact, &map, &overlaid_squares(&owned));
        text.push_str(&report);
        write_png(&out.join("error.png"), &heat, hw, hh)?;
    }
    text.push_str(&comparison);
    text.push_str(&format!("digest {:016x}\n", digest(&ground)));
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
