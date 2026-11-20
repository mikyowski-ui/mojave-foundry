//! Walkability from navmeshes, the slope limit and blocked hexes.

mod common;

use common::*;
use map_builder::BuildOptions;
use map_builder::walk::{HEXES, walkability};

fn grid(
    scene_cells: &[mir::CellDoc],
    sampler: &mir::GroundSampler,
    frame: &map_builder::walk::Frame,
    blocked: &[bool],
    options: &BuildOptions,
) -> (Vec<bool>, map_builder::walk::WalkStats) {
    walkability(frame, sampler, scene_cells, blocked, options)
}

#[test]
fn a_navmesh_decides_inside_outside_and_the_edge_tolerance() {
    let projection = projection();
    let window = window(&projection);
    let frame = frame(&window, &projection);
    let sampler = sampler(&window, flat);
    let hex = hex_at(&frame, 50.0, 50.0);
    let [hx, hy] = hex_point(&frame, hex);
    // A triangle whose west edge passes 10 units east of the hex centre.
    let edge = hx + 10.0;
    let triangle = [[edge, hy - 500.0], [edge + 800.0, hy], [edge, hy + 500.0]];
    let mut cell = exterior_cell(0, 0);
    cell.navmeshes.push(navmesh(0x2000, &[triangle], 100.0));
    let cells = vec![cell];
    let blocked = vec![false; HEXES];

    let strict = BuildOptions {
        navmesh_tolerance: 0.0,
        ..BuildOptions::default()
    };
    let (walkable, stats) = grid(&cells, &sampler, &frame, &blocked, &strict);
    assert!(!walkable[hex as usize], "10 units outside, no tolerance");
    let inside = frame
        .hex_of_source(f64::from(edge) + 300.0, f64::from(hy))
        .unwrap();
    assert!(walkable[inside as usize]);
    let far = frame
        .hex_of_source(f64::from(hx) - 400.0, f64::from(hy))
        .unwrap();
    assert!(!walkable[far as usize]);
    assert_eq!(stats.triangles, 1);
    assert_eq!(stats.triangles_rasterized, 1);
    assert!(stats.rejected_navmesh > 0);

    let loose = BuildOptions {
        navmesh_tolerance: 24.0,
        ..BuildOptions::default()
    };
    let (walkable, _) = grid(&cells, &sampler, &frame, &blocked, &loose);
    assert!(
        walkable[hex as usize],
        "10 units outside, 24 units of tolerance"
    );
    assert!(!walkable[far as usize]);

    // Cells without a navmesh fall back to slope alone: flat, so walkable.
    let elsewhere = frame.hex_of_source(-2000.0, 5000.0).unwrap();
    assert!(walkable[elsewhere as usize]);
}

#[test]
fn standable_hexes_are_the_navmesh_alone() {
    let projection = projection();
    let window = window(&projection);
    let frame = frame(&window, &projection);
    let sampler = sampler(&window, flat);
    let mut cell = exterior_cell(0, 0);
    cell.navmeshes.push(navmesh(
        0x2000,
        &rectangle(1000.0, 1000.0, 3000.0, 3000.0),
        100.0,
    ));
    let cells = vec![cell];
    let standable =
        map_builder::walk::standable_hexes(&frame, &sampler, &cells, &BuildOptions::default());
    assert!(standable.is_sorted());
    let inside = frame.hex_of_source(2000.0, 2000.0).unwrap();
    assert!(standable.binary_search(&inside).is_ok());
    // Off the navmesh in its own cell, and in a cell that has none (which
    // the slope test alone would call walkable): neither is vouched for.
    let off = frame.hex_of_source(3800.0, 3800.0).unwrap();
    assert!(standable.binary_search(&off).is_err());
    let elsewhere = frame.hex_of_source(-2000.0, 5000.0).unwrap();
    assert!(standable.binary_search(&elsewhere).is_err());

    let (verdicts, _) = map_builder::walk::verdicts(
        &frame,
        &sampler,
        &cells,
        &vec![false; HEXES],
        &BuildOptions::default(),
    );
    use map_builder::walk::Verdict;
    assert_eq!(verdicts[inside as usize], Verdict::Walkable);
    assert_eq!(verdicts[off as usize], Verdict::OffNavmesh);
    assert_eq!(verdicts[elsewhere as usize], Verdict::Walkable);
}

#[test]
fn a_triangle_high_above_the_terrain_is_ignored() {
    let projection = projection();
    let window = window(&projection);
    let frame = frame(&window, &projection);
    let sampler = sampler(&window, flat);
    let mut cell = exterior_cell(0, 0);
    cell.navmeshes.push(navmesh(
        0x2000,
        &rectangle(1000.0, 1000.0, 3000.0, 3000.0),
        100.0 + 400.0,
    ));
    let (walkable, stats) = grid(
        &[cell],
        &sampler,
        &frame,
        &vec![false; HEXES],
        &BuildOptions::default(),
    );
    assert_eq!(stats.triangles_too_high, 2);
    let inside = frame.hex_of_source(2000.0, 2000.0).unwrap();
    assert!(!walkable[inside as usize]);
}

#[test]
fn the_slope_limit_and_blocked_hexes() {
    let projection = projection();
    let window = window(&projection);
    let frame = frame(&window, &projection);
    // Flat west of x = 2048, rising 1 per unit east of it.
    let sampler = sampler(&window, |x, _| if x < 2048.0 { 0.0 } else { x - 2048.0 });
    let cells = vec![exterior_cell(0, 0)];
    let flat_hex = frame.hex_of_source(1000.0, 2000.0).unwrap();
    let steep_hex = frame.hex_of_source(3000.0, 2000.0).unwrap();
    let mut blocked = vec![false; HEXES];
    let blocked_hex = frame.hex_of_source(1000.0, 1000.0).unwrap();
    blocked[blocked_hex as usize] = true;
    let (walkable, stats) = grid(&cells, &sampler, &frame, &blocked, &BuildOptions::default());
    assert!(walkable[flat_hex as usize]);
    assert!(!walkable[steep_hex as usize], "slope 1.0 is over 0.7");
    assert!(!walkable[blocked_hex as usize]);
    assert!(stats.rejected_slope > 0 && stats.rejected_blocked == 1);
    assert!(stats.slope_only > 0);

    let steep = BuildOptions {
        slope_limit: 1.5,
        ..BuildOptions::default()
    };
    let (walkable, _) = grid(&cells, &sampler, &frame, &blocked, &steep);
    assert!(walkable[steep_hex as usize]);

    // No terrain at all, no navmesh: nothing to stand on.
    let empty = mir::GroundSampler::default();
    let (walkable, stats) = grid(&cells, &empty, &frame, &blocked, &BuildOptions::default());
    assert!(walkable.iter().all(|w| !w));
    assert_eq!(stats.rejected_no_ground, HEXES - stats.rejected_navmesh);
}
