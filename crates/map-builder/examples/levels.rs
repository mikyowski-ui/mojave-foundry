//! Measure the exterior level split (`map_builder::levels`) on a MIR.
//!
//! ```text
//! cargo run --release -p map-builder --example levels -- \
//!     <mir dir> <worldspace> <x_west> <y_north> [entrance x,y]
//! ```
//!
//! Prints the split's summary, its largest regions, the level of every
//! teleport door and actor reference, and per level the hexes walkable
//! with nothing blocked and their components (the ground the walker could
//! use on each elevation before the slicer blocks anything); with an
//! entrance point, the share of each level's walkable hexes in the
//! component of the hex nearest it or reached from it through the
//! transitions. Counts, heights and hex numbers only.

use std::collections::BTreeSet;
use std::path::Path;

use map_builder::BuildOptions;
use map_builder::islands::components;
use map_builder::levels::{LevelOptions, split_levels};
use map_builder::walk::{self, Frame, Verdict};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: levels <mir> <worldspace> <x_west> <y_north> [entrance x,y]");
        std::process::exit(2);
    }
    let mir = mir::Mir::open(Path::new(&args[1])).expect("mir");
    let worldspace = args[2].as_str();
    let window = mir::MapWindow {
        x_west: args[3].parse().unwrap(),
        y_north: args[4].parse().unwrap(),
        size: 14_000.0,
    };
    let entrance: Option<[f32; 2]> = args.get(5).map(|a| {
        let v: Vec<f32> = a.split(',').map(|s| s.parse().unwrap()).collect();
        [v[0], v[1]]
    });
    let projection =
        foundry_render::Projection::new(&foundry_render::Calibration::default()).unwrap();
    let frame = Frame::new(&window, &projection).unwrap();
    let sampler = mir::GroundSampler::load(&mir, worldspace, &window).expect("sampler");
    let (xs, ys) = window.cells(1);
    let mut cells = Vec::new();
    for y in ys {
        for x in xs.clone() {
            if let Some(cell) = mir.cell(worldspace, x, y).unwrap() {
                cells.push(cell);
            }
        }
    }
    let mut options = LevelOptions::default();
    if let Ok(v) = std::env::var("LEVELS_CLEARANCE") {
        options.clearance = v.parse().unwrap();
    }
    if let Ok(v) = std::env::var("LEVELS_MIN_CONFLICT") {
        options.min_conflict_hexes = v.parse().unwrap();
    }
    let split = split_levels(&cells, &frame, &sampler, &options);
    for line in split.summary() {
        println!("{line}");
    }
    let mut order: Vec<usize> = (0..split.regions.len()).collect();
    order.sort_by(|a, b| split.regions[*b].area.total_cmp(&split.regions[*a].area));
    println!("largest regions:");
    for r in order.iter().take(25) {
        let g = &split.regions[*r];
        println!(
            "  region {r:>3}: {:?} level {:?} chain {} {} tris {:>8.0} sq {:>4} hexes z {:.0}..{:.0} \
             (mean {:.0}, {:+.0} above terrain) x {:.0}..{:.0} y {:.0}..{:.0}{}",
            g.kind,
            g.level,
            g.chain,
            g.triangles,
            g.area,
            g.hexes,
            g.z_range[0],
            g.z_range[1],
            g.mean_z,
            g.above_terrain.unwrap_or(f32::NAN),
            g.min[0],
            g.max[0],
            g.min[1],
            g.max[1],
            if g.split { " (split)" } else { "" }
        );
    }
    if std::env::var("LEVELS_CONFLICTS").is_ok() {
        for (lower, upper, hexes) in &split.conflict_pairs {
            let (a, b) = (&split.regions[*lower], &split.regions[*upper]);
            println!(
                "  conflict: region {lower} (level {:?}, z {:.0}) under region {upper} (level {:?}, \
                 z {:.0}) on {hexes} hexes",
                a.level, a.mean_z, b.level, b.mean_z
            );
        }
    }
    println!("doors:");
    for cell in &cells {
        for r in &cell.references {
            if let Some(t) = &r.teleport {
                let p = r.transform.position;
                println!(
                    "  {} at ({:.0}, {:.0}, {:.0}) to {}: level of point {:?}, reference level {:?}, \
                     hex {:?}",
                    r.form,
                    p[0],
                    p[1],
                    p[2],
                    t.cell
                        .as_ref()
                        .and_then(|c| c.editor_id.clone())
                        .unwrap_or_else(|| "?".into()),
                    split.level_of_point(p),
                    split.reference_level(p),
                    frame.hex_of_source(f64::from(p[0]), f64::from(p[1]))
                );
            }
        }
    }
    let mut actors = [0usize; 4];
    for cell in &cells {
        for r in &cell.references {
            if matches!(r.base_kind.as_deref(), Some("NPC_") | Some("CREA")) {
                let level = split.level_of_point(r.transform.position);
                actors[level.map_or(3, usize::from)] += 1;
            }
        }
    }
    println!(
        "actors by level: 0: {}, 1: {}, 2: {}, none: {}",
        actors[0], actors[1], actors[2], actors[3]
    );
    // Walkable hexes per level, nothing blocked.
    let mut walkables: Vec<Vec<bool>> = Vec::new();
    let mut one_level = BuildOptions::default();
    let (flat, _) = walk::verdicts(&frame, &sampler, &cells, &[], &one_level);
    let flat_walk = flat.iter().filter(|v| **v == Verdict::Walkable).count();
    println!("one level (today's rule), nothing blocked: {flat_walk} walkable hexes");
    for level in 0..split.level_count() {
        one_level.level = Some(split.exterior_level(level));
        let (verdicts, stats) = walk::verdicts(&frame, &sampler, &cells, &[], &one_level);
        let walkable: Vec<bool> = verdicts.iter().map(|v| *v == Verdict::Walkable).collect();
        let comps = components(|h| walkable[h as usize]);
        println!(
            "level {level}: {} walkable ({} covered by its triangles, {} by slope alone), {} \
             components, largest {:?}",
            stats.walkable,
            stats.covered,
            stats.slope_only,
            comps.len(),
            &comps.sizes()[..comps.len().min(6)]
        );
        walkables.push(walkable);
    }
    let Some(entrance) = entrance else {
        return;
    };
    // Reach through the transitions: a level's component is reached when
    // a link end on it lies in a reached component of the other level.
    let comps: Vec<_> = walkables
        .iter()
        .map(|w| components(|h| w[h as usize]))
        .collect();
    let near = |level: usize, p: [f32; 2]| {
        let at = frame.tile_of_source(f64::from(p[0]), f64::from(p[1]));
        walk::nearest_walkable(&frame, &walkables[level], at, &BTreeSet::new())
    };
    let mut reached: BTreeSet<(usize, usize)> = BTreeSet::new();
    let start_level = 0;
    if let Some(c) = near(start_level, entrance).and_then(|hex| comps[start_level].of(hex)) {
        reached.insert((start_level, c));
    }
    loop {
        let before = reached.len();
        for link in &split.links {
            let ends: Vec<(usize, Option<usize>)> = (0..2)
                .map(|k| {
                    let level = usize::from(link.levels[k]);
                    let c = near(level, [link.points[k][0], link.points[k][1]])
                        .and_then(|h| comps[level].of(h));
                    (level, c)
                })
                .collect();
            for k in 0..2 {
                if let ((la, Some(ca)), (lb, Some(cb))) = (ends[k], ends[1 - k])
                    && reached.contains(&(la, ca))
                {
                    reached.insert((lb, cb));
                }
            }
        }
        if reached.len() == before {
            break;
        }
    }
    let mut total = 0;
    let mut got = 0;
    for (level, c) in comps.iter().enumerate() {
        let size: usize = c.sizes().iter().sum();
        let mine: usize = reached
            .iter()
            .filter(|(l, _)| *l == level)
            .map(|(_, id)| c.sizes()[*id])
            .sum();
        total += size;
        got += mine;
        println!(
            "level {level}: {mine} of {size} walkable hexes reached from the entrance through the \
             transitions"
        );
    }
    println!(
        "all levels: {got} of {total} ({:.1} %)",
        100.0 * got as f64 / total.max(1) as f64
    );
}
