//! Slices one map window of a real MIR directory and writes debug images,
//! to check classification, strips and the art budget on real data.
//!
//! ```text
//! SLICER_MIR=out/<slice>/mir SLICER_F2=/path/to/fallout2 \
//!     cargo run --release -p map-slicer --example slicer_check
//! ```
//!
//! - `SLICER_MIR`: the MIR directory (required).
//! - `SLICER_F2`: the Fallout 2 directory holding `master.dat` (required;
//!   only its palette is read).
//! - `SLICER_WORLDSPACE`: worldspace editor id (required).
//! - `SLICER_CENTER`: `x,y` of the window centre in source units (required).
//! - `SLICER_BUDGET`: `walls,scenery,items` free entries (default
//!   `2400,2200,3500`).
//! - `SLICER_OUT`: output directory, which must sit under the workspace's
//!   ignored `out/` (default `out/slicer-check`).
//! - `SLICER_LADDER`: when set, slice a second time against the budget
//!   itself, so the budget ladder runs if a list overflows.
//! - `SLICER_LANDMARK`: `width,height,max_pieces,tolerance` landmark
//!   limits to try instead of the defaults.
//! - `SLICER_PROBE`: `hex,hex,...` hexes to report (blocked, anchored).
//! - `SLICER_MODELS`: when set, a file name in the output directory: every
//!   reference of the extraction classified (by manifest bounds), one
//!   `model, class, count` line per pair.
//!
//! The output digest is a SHA-256 over everything the map builder receives,
//! for comparing two runs byte for byte.
//!
//! Prints the classification and budget report; writes PNGs of the largest
//! sliced object recomposed and spread apart, and a half-scale preview of
//! the whole window. Nothing is written outside the output directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use foundry_render::{Calibration, CpuRenderer, Projection};
use map_slicer::{
    ArtImage, Budget, Class, ClassKind, MeshFacts, MirAssets, Placement, Role, Sliced,
    SlicerConfig, classify,
};
use mir::{GroundSampler, MapWindow, Mir, Reference};
use quantize::PaletteMap;
use vault_formats::geometry::HexGrid;
use vault_formats::{Dat2Archive, Palette, Vfs};

fn main() {
    if let Err(message) = run() {
        eprintln!("slicer_check: {message}");
        std::process::exit(1);
    }
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("set {name}"))
}

fn numbers(text: &str) -> Result<Vec<f64>, String> {
    text.split(',')
        .map(|v| {
            v.trim()
                .parse::<f64>()
                .map_err(|e| format!("{text:?}: {e}"))
        })
        .collect()
}

fn run() -> Result<(), String> {
    let mir_dir = PathBuf::from(env("SLICER_MIR")?);
    let f2 = PathBuf::from(env("SLICER_F2")?);
    let worldspace = env("SLICER_WORLDSPACE")?.to_ascii_lowercase();
    let centre = numbers(&env("SLICER_CENTER")?)?;
    if centre.len() != 2 {
        return Err("SLICER_CENTER is x,y".into());
    }
    let budget = numbers(&std::env::var("SLICER_BUDGET").unwrap_or("2400,2200,3500".into()))?;
    if budget.len() != 3 {
        return Err("SLICER_BUDGET is walls,scenery,items".into());
    }
    let budget = Budget {
        walls: budget[0] as usize,
        scenery: budget[1] as usize,
        items: budget[2] as usize,
    };
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|e| format!("workspace: {e}"))?;
    let out_root = workspace.join("out");
    let out = std::env::var_os("SLICER_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| out_root.join("slicer-check"));
    std::fs::create_dir_all(&out).map_err(|e| format!("create output directory: {e}"))?;
    let out = out.canonicalize().map_err(|e| format!("output: {e}"))?;
    if !out.starts_with(&out_root) {
        return Err("SLICER_OUT must sit under the workspace's out/ directory".into());
    }

    let dat = Dat2Archive::open(f2.join("master.dat")).map_err(|e| format!("master.dat: {e}"))?;
    let palette = Palette::parse(&dat.read("color.pal").map_err(|e| e.to_string())?)
        .map_err(|e| format!("color.pal: {e}"))?;
    let map = PaletteMap::try_new(&palette).map_err(|e| e.to_string())?;

    let calibration = Calibration::default();
    let projection = Projection::new(&calibration).map_err(|e| e.to_string())?;
    let mir = Mir::open(&mir_dir).map_err(|e| e.to_string())?;
    let window = MapWindow::from_center(
        [centre[0] as f32, centre[1] as f32],
        calibration.tile_units(),
    );
    let sampler = GroundSampler::load(&mir, &worldspace, &window).map_err(|e| e.to_string())?;
    let mut references: Vec<Reference> = Vec::new();
    let (xs, ys) = window.cells(1);
    for y in ys {
        for x in xs.clone() {
            if let Some(doc) = mir.cell(&worldspace, x, y).map_err(|e| e.to_string())? {
                references.extend(doc.references.into_iter().filter(|r| {
                    let p = r.transform.position;
                    window.contains(p[0], p[1])
                }));
            }
        }
    }
    println!("{} references in the window", references.len());
    let mut config = SlicerConfig::default();
    if let Ok(text) = std::env::var("SLICER_LANDMARK") {
        // Landmark limits to try: piece width, height, max pieces, tolerance.
        let v = numbers(&text)?;
        if v.len() != 4 {
            return Err("SLICER_LANDMARK is width,height,max_pieces,tolerance".into());
        }
        config.landmark_piece_max = [v[0] as u32, v[1] as u32];
        config.landmark_max_pieces = v[2] as usize;
        config.landmark_sort_tolerance = v[3] as usize;
    }

    if let Some(name) = std::env::var_os("SLICER_MODELS") {
        // Every reference of every cell, classified by manifest bounds: to
        // compare classification rules over a whole extraction.
        let mut all = Vec::new();
        for key in mir.cells() {
            if let Some(doc) = mir
                .cell(&key.worldspace, key.x, key.y)
                .map_err(|e| e.to_string())?
            {
                all.extend(doc.references);
            }
        }
        let path = out.join(name);
        write_models(&mir, &all, &config, calibration.tile_units(), &path)?;
        println!(
            "{} references classified into {}",
            all.len(),
            path.display()
        );
    }
    print_classes(&mir, &references, &config, calibration.tile_units());
    if std::env::var_os("SLICER_RATIOS").is_some() {
        print_ratios(&mir, &references, &config, calibration.tile_units());
    }

    let started = Instant::now();
    let guess = map_slicer::estimate(
        &mir,
        &references,
        &window,
        &sampler,
        &projection,
        &budget,
        &config,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "\nestimate in {:.1} s\n{guess}\n",
        started.elapsed().as_secs_f64()
    );

    // Slice against an unlimited budget so the images exist either way,
    // then judge the real one.
    let unlimited = Budget {
        walls: usize::MAX / 4,
        scenery: usize::MAX / 4,
        items: usize::MAX / 4,
    };
    let started = Instant::now();
    let mut renderer = CpuRenderer::new(&calibration).map_err(|e| e.to_string())?;
    let assets = MirAssets::new(&mir, config.texture_cache_bytes);
    let sliced = map_slicer::slice_with(
        &assets,
        &references,
        &window,
        &sampler,
        &projection,
        &mut renderer,
        &map,
        &unlimited,
        &config,
    )
    .map_err(|e| e.to_string())?;
    println!("slice in {:.1} s", started.elapsed().as_secs_f64());
    println!("output digest {}", digest(&sliced));
    let mut report = sliced.report.clone();
    for c in &mut report.categories {
        c.free = budget.free(c.category);
    }
    println!("{report}\n");
    match report.check() {
        Ok(()) => println!("FITS the budget {budget:?}"),
        Err(e) => println!("DOES NOT FIT: {e}"),
    }
    let failed: Vec<_> = sliced
        .skipped
        .iter()
        .filter(|s| {
            matches!(
                s.reason,
                map_slicer::SkipReason::Failed(_) | map_slicer::SkipReason::TooLarge
            )
        })
        .collect();
    for s in failed.iter().take(10) {
        println!("  failed: {:?} {}", s.model, s.reason);
    }

    if std::env::var_os("SLICER_LADDER").is_some() {
        // Slice again against the real budget, so the budget ladder runs
        // when a list would overflow.
        let started = Instant::now();
        match map_slicer::slice_with(
            &assets,
            &references,
            &window,
            &sampler,
            &projection,
            &mut renderer,
            &map,
            &budget,
            &config,
        ) {
            Ok(fitted) => println!(
                "\nagainst {budget:?} in {:.1} s, output digest {}\n{}",
                started.elapsed().as_secs_f64(),
                digest(&fitted),
                fitted.report
            ),
            Err(e) => println!("\nagainst {budget:?}: {e}"),
        }
    }

    check_landmarks(&sliced);
    if let Ok(text) = std::env::var("SLICER_PROBE") {
        // Hexes to look at: blocked or not, and what is anchored there.
        for hex in numbers(&text)? {
            let hex = hex as i32;
            let here: Vec<_> = sliced.placements.iter().filter(|p| p.hex == hex).collect();
            println!(
                "probe hex {hex}: blocked {}, {} placements ({} blocking)",
                sliced.blocked_hexes.binary_search(&hex).is_ok(),
                here.len(),
                here.iter().filter(|p| p.blocking).count()
            );
        }
    }
    write_images(&sliced, &palette, &out)?;
    println!("images in {}", out.display());
    Ok(())
}

fn print_classes(mir: &Mir, references: &[Reference], config: &SlicerConfig, tile_units: f32) {
    let manifest = mir.manifest();
    let mut by: BTreeMap<ClassKind, BTreeMap<String, (usize, f64, f32)>> = BTreeMap::new();
    for r in references {
        let facts = r
            .mesh
            .as_ref()
            .and_then(|h| manifest.mesh_info.get(h))
            .map(|i| MeshFacts::from_bounds(i.bounds, u64::from(i.triangles)));
        let class = classify(r, facts.as_ref(), tile_units, config);
        let (hexes, height) = facts
            .and_then(|f| f.bounds)
            .map(|b| {
                let f = map_slicer::Footprint::of(&r.transform, b);
                (f.hexes(tile_units), f.height())
            })
            .unwrap_or((0.0, 0.0));
        let label = match &class {
            Class::Skip(reason) => format!(
                "{} [{}]",
                r.model.clone().unwrap_or_default(),
                reason.label()
            ),
            _ => r.model.clone().unwrap_or_default(),
        };
        let e = by
            .entry(class.kind())
            .or_default()
            .entry(label)
            .or_insert((0, 0.0, 0.0));
        e.0 += 1;
        e.1 = e.1.max(hexes);
        e.2 = e.2.max(height);
    }
    for (kind, models) in &by {
        let total: usize = models.values().map(|m| m.0).sum();
        println!(
            "\n{kind}: {total} references, {} models (by bounds only)",
            models.len()
        );
        let mut rows: Vec<_> = models.iter().collect();
        rows.sort_by(|a, b| b.1.1.total_cmp(&a.1.1).then(a.0.cmp(b.0)));
        for (model, (n, hexes, height)) in rows.iter().take(40) {
            println!("  {n:>4}  {hexes:>8.0} hexes  {height:>6.0} high  {model}");
        }
    }
}

/// One line per distinct (model, class): the class by manifest bounds and
/// how many references have it.
fn write_models(
    mir: &Mir,
    references: &[Reference],
    config: &SlicerConfig,
    tile_units: f32,
    path: &Path,
) -> Result<(), String> {
    let manifest = mir.manifest();
    let mut rows: BTreeMap<(String, String), usize> = BTreeMap::new();
    for r in references {
        let facts = r
            .mesh
            .as_ref()
            .and_then(|h| manifest.mesh_info.get(h))
            .map(|i| MeshFacts::from_bounds(i.bounds, u64::from(i.triangles)));
        let class = match classify(r, facts.as_ref(), tile_units, config) {
            Class::Skip(reason) => format!("skipped ({})", reason.label()),
            other => other.kind().name().to_owned(),
        };
        let model = r.model.clone().unwrap_or_default().to_ascii_lowercase();
        *rows.entry((model, class)).or_insert(0) += 1;
    }
    let text: String = rows
        .iter()
        .map(|((model, class), n)| format!("{model}\t{class}\t{n}\n"))
        .collect();
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Triangle area over footprint area of the sliced references: the sparse
/// test's input.
fn print_ratios(mir: &Mir, references: &[Reference], config: &SlicerConfig, tile_units: f32) {
    let mut rows = BTreeMap::new();
    for r in references {
        let Some(hash) = &r.mesh else { continue };
        let Ok(mesh) = mir.mesh(hash) else { continue };
        let facts = MeshFacts::of(&mesh);
        if classify(r, Some(&facts), tile_units, config) != Class::Sliced {
            continue;
        }
        let Some(b) = facts.bounds else { continue };
        let f = map_slicer::Footprint::of(&r.transform, b);
        let s = f64::from(r.transform.scale);
        let ratio = facts.surface_area.unwrap_or(0.0) * s * s / f.area.max(1.0);
        rows.insert(
            r.model.clone().unwrap_or_default(),
            (ratio, f.hexes(tile_units)),
        );
    }
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_by(|a, b| a.1.0.total_cmp(&b.1.0));
    println!("\narea ratios of sliced models, lowest first:");
    for (model, (ratio, hexes)) in rows.iter().take(25) {
        println!("  {ratio:>8.3}  {hexes:>7.0} hexes  {model}");
    }
}

/// SHA-256 over everything a run hands the map builder (art, placements,
/// overlays, blocked hexes, skips), to compare two runs byte for byte.
fn digest(sliced: &Sliced) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let n = |h: &mut Sha256, v: i64| h.update(v.to_le_bytes());
    n(&mut h, sliced.art.len() as i64);
    for a in &sliced.art {
        n(&mut h, a.category as i64);
        n(&mut h, i64::from(a.width));
        n(&mut h, i64::from(a.height));
        n(&mut h, i64::from(a.shift[0]));
        n(&mut h, i64::from(a.shift[1]));
        h.update(&a.pixels);
        h.update(a.source.as_bytes());
    }
    n(&mut h, sliced.placements.len() as i64);
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
    n(&mut h, sliced.overlays.len() as i64);
    for o in &sliced.overlays {
        n(&mut h, i64::from(o.origin[0]));
        n(&mut h, i64::from(o.origin[1]));
        n(&mut h, i64::from(o.width));
        n(&mut h, i64::from(o.height));
        h.update(&o.rgba);
    }
    n(&mut h, sliced.blocked_hexes.len() as i64);
    for b in &sliced.blocked_hexes {
        n(&mut h, i64::from(*b));
    }
    n(&mut h, sliced.skipped.len() as i64);
    for s in &sliced.skipped {
        h.update(s.reference.as_bytes());
        h.update(s.reason.to_string().as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Every landmark piece against the engine's drawing rule
/// (`map_slicer::engine_view`): for every camera hex from which some piece
/// shows a pixel in the 640 x 380 view, each such piece's hex must be
/// visited. Prints the tally.
fn check_landmarks(sliced: &Sliced) {
    use map_slicer::engine_view::{VIEW, view_origin, visited_hexes};
    let landmark_models: std::collections::BTreeSet<&str> = sliced
        .report
        .contributors
        .iter()
        .filter(|c| c.class == ClassKind::Landmark)
        .map(|c| c.model.as_str())
        .collect();
    let pieces: Vec<(i32, [i64; 4])> = sliced
        .placements
        .iter()
        .filter(|p| p.role == Role::Scenery)
        .filter(|p| landmark_models.contains(sliced.art[p.art].source.as_str()))
        .map(|p| {
            let a = &sliced.art[p.art];
            let o = origin(a, p);
            (
                p.hex,
                [
                    o[0],
                    o[1],
                    o[0] + i64::from(a.width) - 1,
                    o[1] + i64::from(a.height) - 1,
                ],
            )
        })
        .collect();
    if pieces.is_empty() {
        return;
    }
    let (mut cameras, mut shown, mut missing) = (0usize, 0usize, 0usize);
    for camera in 0..40_000 {
        let Some(v) = view_origin(camera) else {
            continue;
        };
        let view = [v[0], v[1], v[0] + VIEW[0] - 1, v[1] + VIEW[1] - 1];
        let in_view: Vec<&(i32, [i64; 4])> = pieces
            .iter()
            .filter(|(_, r)| {
                r[0] <= view[2] && r[2] >= view[0] && r[1] <= view[3] && r[3] >= view[1]
            })
            .collect();
        if in_view.is_empty() {
            continue;
        }
        cameras += 1;
        let visited = visited_hexes(camera);
        for (hex, _) in in_view {
            shown += 1;
            if !visited.contains(hex) {
                missing += 1;
            }
        }
    }
    println!(
        "landmark visibility: {} pieces, {cameras} camera hexes showing one, {shown} piece views, {missing} not drawn",
        pieces.len()
    );
}

/// Engine placement of a frame's pixel (0, 0).
fn origin(a: &ArtImage, p: &Placement) -> [i64; 2] {
    let c = HexGrid::default().hex_center(p.hex).unwrap_or([0, 0]);
    [
        c[0] + i64::from(a.shift[0]) + i64::from(p.displacement[0]) - i64::from(a.width / 2),
        c[1] + i64::from(a.shift[1]) + i64::from(p.displacement[1]) - (i64::from(a.height) - 1),
    ]
}

struct Picture {
    origin: [i64; 2],
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Picture {
    fn new(origin: [i64; 2], width: usize, height: usize, background: [u8; 4]) -> Self {
        Picture {
            origin,
            width,
            height,
            rgba: background.repeat(width * height),
        }
    }

    fn put(&mut self, x: i64, y: i64, c: [u8; 4]) {
        let (i, j) = (x - self.origin[0], y - self.origin[1]);
        if i >= 0 && j >= 0 && (i as usize) < self.width && (j as usize) < self.height {
            let k = (j as usize * self.width + i as usize) * 4;
            self.rgba[k..k + 4].copy_from_slice(&c);
        }
    }

    fn art(&mut self, a: &ArtImage, at: [i64; 2], colours: &[[u8; 4]; 256], scale: i64) {
        for y in 0..a.height as i64 {
            for x in 0..a.width as i64 {
                let v = a.pixels[(y * a.width as i64 + x) as usize];
                if v != 0 {
                    self.put(
                        (at[0] + x) / scale,
                        (at[1] + y) / scale,
                        colours[usize::from(v)],
                    );
                }
            }
        }
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let png = foundry_gallery::encode_png(&self.rgba, self.width as u32, self.height as u32)
            .map_err(|e| e.to_string())?;
        std::fs::write(path, png).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn write_images(sliced: &Sliced, palette: &Palette, out: &Path) -> Result<(), String> {
    let colours = palette.rgba8();
    // The sliced references with the most strips.
    let mut strips: BTreeMap<&str, Vec<&Placement>> = BTreeMap::new();
    for p in sliced
        .placements
        .iter()
        .filter(|p| p.role == Role::WallStrip)
    {
        strips.entry(p.reference.as_str()).or_default().push(p);
    }
    let mut ranked: Vec<(&str, Vec<&Placement>)> = strips.into_iter().collect();
    ranked.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    for (rank, (reference, placements)) in ranked.iter().take(3).enumerate() {
        let source = &sliced.art[placements[0].art].source;
        let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
        let mut widths = Vec::new();
        for p in placements {
            let a = &sliced.art[p.art];
            let o = origin(a, p);
            b = [
                b[0].min(o[0]),
                b[1].min(o[1]),
                b[2].max(o[0] + i64::from(a.width)),
                b[3].max(o[1] + i64::from(a.height)),
            ];
            widths.push((a.width, a.height, p.run.is_some()));
        }
        let singles: Vec<_> = widths.iter().filter(|w| !w.2).collect();
        let mean_w =
            singles.iter().map(|w| f64::from(w.0)).sum::<f64>() / singles.len().max(1) as f64;
        let mean_h =
            singles.iter().map(|w| f64::from(w.1)).sum::<f64>() / singles.len().max(1) as f64;
        println!(
            "object {rank}: {reference} ({source}): {} strips ({} runs), single strips mean {mean_w:.0} x {mean_h:.0} px",
            placements.len(),
            widths.len() - singles.len()
        );
        let (w, h) = ((b[2] - b[0]) as usize + 16, (b[3] - b[1]) as usize + 16);
        let mut picture = Picture::new([b[0] - 8, b[1] - 8], w, h, [40, 44, 52, 255]);
        for p in placements {
            let a = &sliced.art[p.art];
            picture.art(a, origin(a, p), &colours, 1);
        }
        picture.save(&out.join(format!("object{rank}-recomposed.png")))?;

        // Spread: every strip pushed away from the object's centre in
        // proportion to its hex's offset, so the cut lines show.
        let centre = [(b[0] + b[2]) / 2, (b[1] + b[3]) / 2];
        let spread = |o: [i64; 2], p: &Placement| {
            let c = HexGrid::default().hex_center(p.hex).unwrap_or([0, 0]);
            [
                o[0] + (c[0] - centre[0]) * 3 / 5,
                o[1] + (c[1] - centre[1]) * 3 / 5,
            ]
        };
        let mut sb = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
        for p in placements {
            let a = &sliced.art[p.art];
            let o = spread(origin(a, p), p);
            sb = [
                sb[0].min(o[0]),
                sb[1].min(o[1]),
                sb[2].max(o[0] + i64::from(a.width)),
                sb[3].max(o[1] + i64::from(a.height)),
            ];
        }
        let (w, h) = ((sb[2] - sb[0]) as usize + 16, (sb[3] - sb[1]) as usize + 16);
        let mut picture = Picture::new([sb[0] - 8, sb[1] - 8], w, h, [40, 44, 52, 255]);
        for p in placements {
            let a = &sliced.art[p.art];
            picture.art(a, spread(origin(a, p), p), &colours, 1);
        }
        picture.save(&out.join(format!("object{rank}-spread.png")))?;
    }

    // The whole window at half scale: overlays, then every placement in
    // the engine's order.
    let scale = 2i64;
    let (x0, y0, x1, y1) = (-16i64, -1300i64, 8000i64, 2450i64);
    let mut picture = Picture::new(
        [x0 / scale, y0 / scale],
        ((x1 - x0) / scale) as usize,
        ((y1 - y0) / scale) as usize,
        [90, 80, 64, 255],
    );
    for o in &sliced.overlays {
        for y in 0..o.height as i64 {
            for x in 0..o.width as i64 {
                let k = ((y * i64::from(o.width) + x) * 4) as usize;
                if o.rgba[k + 3] != 0 {
                    let c = [o.rgba[k], o.rgba[k + 1], o.rgba[k + 2], 255];
                    picture.put(
                        (i64::from(o.origin[0]) + x) / scale,
                        (i64::from(o.origin[1]) + y) / scale,
                        c,
                    );
                }
            }
        }
    }
    for p in &sliced.placements {
        let a = &sliced.art[p.art];
        picture.art(a, origin(a, p), &colours, scale);
    }
    picture.save(&out.join("window-half.png"))?;
    Ok(())
}
