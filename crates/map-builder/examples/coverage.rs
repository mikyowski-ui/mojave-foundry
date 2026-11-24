//! Compare the navmesh coverage rules (`walk::NavmeshCoverage`) on a
//! written interior map.
//!
//! ```text
//! cargo run --release -p map-builder --example coverage -- \
//!     <mir dir> <interior editor id> <floor z> <centre x,y> <layer dir> <map file>
//! ```
//!
//! For each rule: the hexes the floor band's navmesh covers, how many the
//! default tolerance rule has that this one drops, and, with every object
//! of the written map that stops movement as a blocker, the walkable hexes
//! in the entrance's component and the teleport doors whose doorstep (a
//! walkable neighbour) is in it. Counts and hex numbers only.

use std::collections::BTreeSet;
use std::path::Path;

use map_builder::islands::{self, components};
use map_builder::walk::{self, Frame, HEXES, NavmeshCoverage, Verdict, neighbours};
use map_builder::{BuildOptions, InteriorBuild, read_layer};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::{ByteSource, GameProfile, LocalPrototypeCatalog, MemoryDir};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 7 {
        eprintln!("usage: coverage <mir> <interior> <floor z> <centre x,y> <layer> <map>");
        std::process::exit(2);
    }
    let mir = mir::Mir::open(Path::new(&args[1])).expect("mir");
    let cell = mir.interior(&args[2]).expect("mir").expect("interior");
    let floor_z: f32 = args[3].parse().unwrap();
    let centre: Vec<f32> = args[4].split(',').map(|v| v.parse().unwrap()).collect();
    let projection =
        foundry_render::Projection::new(&foundry_render::Calibration::default()).unwrap();
    let window = mir::MapWindow::from_center([centre[0], centre[1]], projection.tile_units());
    let frame = Frame::new(&window, &projection).unwrap();
    let layer = read_layer(Path::new(&args[5])).expect("layer");
    let files = layer.files();
    let tree = MemoryDir::new(
        files
            .iter()
            .map(|(p, b)| (p.clone(), ByteSource::new(b.clone()).unwrap())),
    )
    .unwrap();
    let catalog = LocalPrototypeCatalog::for_game(&tree, GameProfile::Fallout2);
    let map = MapFile::parse_for(
        &files[&format!("maps/{}", args[6])],
        &catalog,
        GameProfile::Fallout2,
    )
    .expect("map");
    let objects = &map.objects.elevations[0];
    let blocked = islands::engine_blocked(objects);
    let doors: Vec<i32> = objects
        .iter()
        .filter(|o| {
            matches!(
                o.state,
                ObjectState::Other {
                    detail: ObjectDetail::Door(_),
                    ..
                }
            )
        })
        .filter_map(|o| o.hex)
        .collect();
    let entry = map.header.entry_hex;
    let cells = [cell];
    let rule = |coverage: NavmeshCoverage| {
        let options = BuildOptions {
            interior: Some(InteriorBuild::new(floor_z)),
            navmesh_coverage: coverage,
            ..BuildOptions::default()
        };
        let sampler = mir::GroundSampler::default();
        let (verdicts, _) = walk::verdicts(&frame, &sampler, &cells, &[], &options);
        verdicts
            .iter()
            .map(|v| *v == Verdict::Walkable)
            .collect::<Vec<bool>>()
    };
    let base = rule(NavmeshCoverage::Tolerance);
    println!(
        "{} doors on the map, entry hex {entry}; {} hexes stop movement",
        doors.len(),
        blocked.iter().filter(|b| **b).count()
    );
    for (name, coverage) in [
        ("tolerance 24 (default)", NavmeshCoverage::Tolerance),
        ("centre inside", NavmeshCoverage::Centre),
        ("share 0.25", NavmeshCoverage::Share(0.25)),
        ("share 0.5", NavmeshCoverage::Share(0.5)),
        ("share 0.75", NavmeshCoverage::Share(0.75)),
    ] {
        let covered = rule(coverage);
        let count = covered.iter().filter(|c| **c).count();
        let dropped: Vec<i32> = (0..HEXES as i32)
            .filter(|h| base[*h as usize] && !covered[*h as usize])
            .collect();
        let free = components(|h| covered[h as usize]);
        let walkable: Vec<bool> = (0..HEXES).map(|h| covered[h] && !blocked[h]).collect();
        let comps = components(|h| walkable[h as usize]);
        let at = walk::nearest_walkable(
            &frame,
            &walkable,
            frame.hex_centre_tile(entry).unwrap(),
            &BTreeSet::new(),
        );
        let main = at.and_then(|h| comps.of(h));
        let in_main = |h: i32| main.is_some() && comps.of(h) == main;
        let total = walkable.iter().filter(|w| **w).count();
        let reached = (0..HEXES as i32).filter(|h| in_main(*h)).count();
        let doors_in = doors
            .iter()
            .filter(|d| neighbours(**d).any(in_main))
            .count();
        println!(
            "{name}: {count} hexes covered, {} of the default's dropped; navmesh alone {} \
             components (largest {}); with the map's blockers {total} walkable, {reached} in the \
             entrance's component ({} components), doors with a doorstep there {doors_in} of {}",
            dropped.len(),
            free.len(),
            free.sizes().first().copied().unwrap_or(0),
            comps.len(),
            doors.len()
        );
        let lost: Vec<i32> = doors
            .iter()
            .copied()
            .filter(|d| !neighbours(*d).any(in_main))
            .collect();
        if !lost.is_empty() {
            println!("  doors without a doorstep in the entrance's component: {lost:?}");
        }
        if std::env::var("COVERAGE_HEXES").is_ok() {
            println!("  dropped: {dropped:?}");
        }
    }
}
