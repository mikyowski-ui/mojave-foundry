//! Explain why hexes of an exterior window are walkable: per hex, the
//! verdict with nothing blocked, whether its cell has a navmesh, and every
//! navmesh triangle whose plan holds the hex centre, with its height above
//! the terrain there.
//!
//! ```text
//! cargo run --release -p map-builder --example roofs -- \
//!     <mir dir> <worldspace> <x_west> <y_north> <hex,hex,...>
//! ```
//!
//! With `ROOFS_SUMMARY=1` it also counts, over the whole window, the
//! walkable hexes by how they are walkable (navmesh at ground height,
//! navmesh high above the terrain within the "too high" limit, slope only).
//! Counts, heights and hex numbers only.

use std::collections::BTreeSet;
use std::path::Path;

use map_builder::BuildOptions;
use map_builder::walk::{self, Frame, HEXES, Verdict};

fn inside(p: [f64; 2], t: [[f64; 2]; 3]) -> bool {
    let c = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let d = [c(t[0], t[1], p), c(t[1], t[2], p), c(t[2], t[0], p)];
    let neg = d.iter().any(|v| *v < 0.0);
    let pos = d.iter().any(|v| *v > 0.0);
    !(neg && pos)
}

fn plane_z(p: [f64; 2], c: &[[f32; 3]; 3]) -> f64 {
    let a = c.map(|v| [f64::from(v[0]), f64::from(v[1]), f64::from(v[2])]);
    let u = [a[1][0] - a[0][0], a[1][1] - a[0][1], a[1][2] - a[0][2]];
    let v = [a[2][0] - a[0][0], a[2][1] - a[0][1], a[2][2] - a[0][2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    if n[2].abs() < 1e-9 {
        return (a[0][2] + a[1][2] + a[2][2]) / 3.0;
    }
    a[0][2] - (n[0] * (p[0] - a[0][0]) + n[1] * (p[1] - a[0][1])) / n[2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 6 {
        eprintln!("usage: roofs <mir> <worldspace> <x_west> <y_north> <hex,hex,...>");
        std::process::exit(2);
    }
    let mir = mir::Mir::open(Path::new(&args[1])).expect("mir");
    let worldspace = args[2].as_str();
    let window = mir::MapWindow {
        x_west: args[3].parse().unwrap(),
        y_north: args[4].parse().unwrap(),
        size: 14_000.0,
    };
    let hexes: Vec<i32> = args[5]
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
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
    let options = BuildOptions::default();
    let (verdicts, stats) = walk::verdicts(&frame, &sampler, &cells, &[], &options);
    println!("{stats:?}");
    let nav_cells: BTreeSet<(i32, i32)> = cells
        .iter()
        .filter(|c| c.navmeshes.iter().any(|n| !n.triangles.is_empty()))
        .filter_map(|c| match &c.location {
            mir::CellLocation::Exterior { x, y, .. } => Some((*x, *y)),
            _ => None,
        })
        .collect();
    println!("cells with a navmesh: {nav_cells:?}");
    let triangles: Vec<(String, usize, [[f32; 3]; 3])> = cells
        .iter()
        .flat_map(|c| c.navmeshes.iter())
        .flat_map(|n| {
            (0..n.triangles.len())
                .filter_map(move |i| n.corners(i).map(|c| (n.form.to_string(), i, c)))
        })
        .collect();
    let explain = |hex: i32| -> Vec<(String, usize, f64, f64)> {
        let Some(c) = frame.hex_centre_tile(hex) else {
            return Vec::new();
        };
        let [x, y] = frame.source_of_tile(c);
        let ground = sampler
            .height(x as f32, y as f32)
            .map_or(f64::NAN, f64::from);
        triangles
            .iter()
            .filter(|(_, _, t)| inside([x, y], t.map(|v| [f64::from(v[0]), f64::from(v[1])])))
            .map(|(f, i, t)| (f.clone(), *i, plane_z([x, y], t), ground))
            .collect()
    };
    for &hex in &hexes {
        let Some(c) = frame.hex_centre_tile(hex) else {
            println!("hex {hex}: off the grid");
            continue;
        };
        let [x, y] = frame.source_of_tile(c);
        let cell = mir::cell_of(&[x as f32, y as f32, 0.0]);
        let ground = sampler.height(x as f32, y as f32);
        let slope = sampler.slope(x as f32, y as f32);
        println!(
            "hex {hex}: source ({x:.0}, {y:.0}) cell {cell:?} navmesh cell {} ground {ground:?} \
             slope {slope:?} verdict {:?}",
            nav_cells.contains(&cell),
            verdicts[hex as usize]
        );
        for (form, index, z, g) in explain(hex) {
            println!(
                "    triangle {form}#{index}: z {z:.0} ({:+.0} above terrain)",
                z - g
            );
        }
    }
    if let Ok(form) = std::env::var("ROOFS_REF") {
        let reference = cells
            .iter()
            .flat_map(|c| c.references.iter())
            .find(|r| r.form.to_string() == form)
            .expect("reference in the window's cells");
        let mesh = mir
            .mesh(reference.mesh.as_deref().expect("mesh"))
            .expect("mesh");
        let m = reference.transform.matrix();
        let origin_ground = sampler.height(m[0][3], m[1][3]).unwrap_or(m[2][3]);
        let mut under = BTreeSet::new();
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for p in &mesh.primitives {
            let placed: Vec<[f64; 3]> = p
                .positions
                .iter()
                .map(|v| {
                    [0, 1, 2].map(|r| {
                        f64::from(m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2] + m[r][3])
                    })
                })
                .collect();
            for q in &placed {
                for k in 0..3 {
                    lo[k] = lo[k].min(q[k]);
                    hi[k] = hi[k].max(q[k]);
                }
            }
            for t in p.indices.as_chunks::<3>().0 {
                let c = t.map(|i| placed[i as usize]);
                if !c.iter().all(|q| q[2] - f64::from(origin_ground) > 120.0) {
                    continue;
                }
                let tri = c.map(|q| frame.tile_of_source(q[0], q[1]));
                for hex in 0..HEXES as i32 {
                    if let Some(h) = frame.hex_centre_tile(hex)
                        && (h[0] - tri[0][0]).abs() < 40.0
                        && inside(h, tri)
                    {
                        under.insert(hex);
                    }
                }
            }
        }
        let walkable: Vec<i32> = under
            .iter()
            .copied()
            .filter(|h| verdicts[*h as usize] == Verdict::Walkable)
            .collect();
        println!(
            "{form}: placed box x {:.0}..{:.0} y {:.0}..{:.0} z {:.0}..{:.0} (ground at origin \
             {origin_ground:.0}); {} hexes under its parts higher than 120, {} of them walkable",
            lo[0],
            hi[0],
            lo[1],
            hi[1],
            lo[2],
            hi[2],
            under.len(),
            walkable.len()
        );
        println!(
            "walkable under it (first 300): {:?}",
            &walkable[..walkable.len().min(300)]
        );
    }
    if std::env::var("ROOFS_SUMMARY").is_ok() {
        let mut slope_only = 0;
        let mut low = 0;
        let mut high = 0;
        let mut tolerance_only = 0;
        let mut high_hexes = Vec::new();
        for hex in 0..HEXES as i32 {
            if verdicts[hex as usize] != Verdict::Walkable {
                continue;
            }
            let Some(c) = frame.hex_centre_tile(hex) else {
                continue;
            };
            let [x, y] = frame.source_of_tile(c);
            if !nav_cells.contains(&mir::cell_of(&[x as f32, y as f32, 0.0])) {
                slope_only += 1;
                continue;
            }
            let tris = explain(hex);
            if tris.is_empty() {
                tolerance_only += 1;
            } else if tris.iter().any(|(_, _, z, g)| (z - g).abs() <= 60.0) {
                low += 1;
            } else {
                high += 1;
                high_hexes.push(hex);
            }
        }
        println!(
            "walkable: slope only {slope_only}, navmesh near the terrain {low}, navmesh only well \
             above or below it {high}, by the tolerance alone {tolerance_only}"
        );
        println!(
            "high hexes (first 200): {:?}",
            &high_hexes[..high_hexes.len().min(200)]
        );
    }
}
