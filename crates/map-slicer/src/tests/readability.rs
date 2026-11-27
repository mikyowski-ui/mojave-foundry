//! The readability dials of the cutaway: walls beside narrow walkways
//! (`Cutaway::narrow_kerb_height`).

use std::collections::BTreeSet;

use super::*;
use crate::cutaway::band_behind;
use crate::hex::{self, Plane};
use crate::{Cutaway, DeclutterProfile};

/// Half the plaza's side; the wall stands at `y = -HALF`.
const HALF: f32 = 560.0;

/// Every hex `steps` steps from `start` in direction `o`.
fn line(start: i32, o: usize, steps: i32) -> Vec<i32> {
    let mut out = vec![start];
    let mut at = start;
    for _ in 0..steps {
        at = hex::neighbours(at)[o].expect("on the map");
        out.push(at);
    }
    out
}

/// The hexes 1 to `depth` steps from `wall` on the side of directions
/// `behind`.
fn band(wall: &[i32], behind: [usize; 2], depth: i32) -> BTreeSet<i32> {
    let mut out = BTreeSet::new();
    for &w in wall {
        for o in behind {
            let mut at = w;
            for _ in 0..depth {
                at = hex::neighbours(at)[o].expect("on the map");
                out.insert(at);
            }
        }
    }
    for w in wall {
        out.remove(w);
    }
    out
}

#[test]
fn the_band_behind_a_wall_is_measured_across_it_on_every_axis() {
    let start = 100 * 200 + 100;
    // A wall along each lattice axis (E, NE, NW), the walkers behind it on
    // the side away from the camera (up on screen).
    for (axis, behind) in [(1, [0, 5]), (0, [5, 4]), (5, [0, 1])] {
        let wall = line(start, axis, 12);
        let middle = wall[6];
        let corridor = band(&wall, behind, 2);
        let width = band_behind(middle, &wall, &corridor, 3);
        assert!(
            width.is_some_and(|w| (1..=3).contains(&w)),
            "axis {axis}: corridor two hexes deep measured {width:?}"
        );
        let plaza = band(&wall, behind, 8);
        let width = band_behind(middle, &wall, &plaza, 3);
        assert_eq!(width, Some(4), "axis {axis}: plaza measured {width:?}");
        // Nothing walkable behind: no band.
        assert_eq!(band_behind(middle, &wall, &BTreeSet::new(), 3), None);
        // A lone strip measures along the up-screen directions.
        let lone = band_behind(middle, &[middle], &corridor, 3);
        assert!(lone.is_some_and(|w| w <= 3), "axis {axis}: lone {lone:?}");
    }
}

fn wall_assets() -> TestAssets {
    TestAssets::new().with(
        "wall",
        mesh(&nif::fixture::box_mesh([2.0 * HALF, 16.0, 200.0], WALL)),
    )
}

fn wall() -> Vec<Reference> {
    vec![reference(
        1,
        "STAT",
        "meshes\\test\\wall.nif",
        "wall",
        &flat(),
        [0.0, -HALF],
        0.0,
        0.0,
    )]
}

fn plane() -> Plane {
    Plane::new(&projection(), window().x_west, window().y_north).unwrap()
}

/// The hexes north of the wall and clear of its ends, `depth` steps or
/// fewer from the hexes it blocks; `None` for the whole plaza.
fn walkers(blocked: &BTreeSet<i32>, depth: Option<i32>) -> BTreeSet<i32> {
    let plane = plane();
    (0..40_000)
        .filter(|h| !blocked.contains(h))
        .filter(|&h| {
            plane.hex_tile(h).is_some_and(|t| {
                let [x, y] = plane.source_of_tile(t);
                x.abs() <= f64::from(HALF) - 80.0
                    && y > f64::from(-HALF)
                    && y <= f64::from(HALF) - 80.0
            })
        })
        .filter(|&h| depth.is_none_or(|d| blocked.iter().any(|&b| hex::distance(h, b) <= d)))
        .collect()
}

fn config(walkers: &BTreeSet<i32>, narrow: Option<f32>) -> SlicerConfig {
    SlicerConfig {
        landmark_walkers: Some(walkers.clone()),
        declutter: Some(DeclutterProfile {
            cutaway: Some(Cutaway {
                exteriors: true,
                narrow_kerb_height: narrow,
                ..Cutaway::default()
            }),
            ..DeclutterProfile::default()
        }),
        ..SlicerConfig::default()
    }
}

fn slice(config: &SlicerConfig) -> Sliced {
    run(&wall_assets(), &wall(), &flat(), &big_budget(), config).expect("slices")
}

/// The topmost opaque screen row of every placement.
fn top_row(sliced: &Sliced) -> i64 {
    let mut top = i64::MAX;
    for p in &sliced.placements {
        let a = &sliced.art[p.art];
        let o = frame_origin(a, p);
        for y in 0..a.height as usize {
            if (0..a.width as usize).any(|x| a.pixels[y * a.width as usize + x] != 0) {
                top = top.min(o[1] + y as i64);
                break;
            }
        }
    }
    top
}

/// Hexes the wall blocks: blocking placements and blocked hexes.
fn blocks(sliced: &Sliced) -> BTreeSet<i32> {
    sliced
        .placements
        .iter()
        .filter(|p| p.blocking)
        .map(|p| p.hex)
        .chain(sliced.blocked_hexes.iter().copied())
        .collect()
}

#[test]
fn a_corridor_two_hexes_wide_gets_the_narrow_kerb_and_a_plaza_does_not() {
    let plain = slice(&SlicerConfig::default());
    let footprint = blocks(&plain);
    assert!(!footprint.is_empty());
    let corridor = walkers(&footprint, Some(2));
    let plaza = walkers(&footprint, None);
    assert!(corridor.len() > 20 && plaza.len() > 4 * corridor.len());

    // The corridor: the narrow kerb cuts the wall lower than the kerb.
    let kerb = slice(&config(&corridor, None));
    let narrow = slice(&config(&corridor, Some(16.0)));
    let (kerb_top, narrow_top) = (top_row(&kerb), top_row(&narrow));
    // Every strip of the wall is lower, its end past the walkway too.
    for (a, b) in kerb.placements.iter().zip(&narrow.placements) {
        assert_eq!(a.hex, b.hex);
        assert!(
            narrow.art[b.art].height + 4 < kerb.art[a.art].height,
            "strip {}: {} vs {}",
            a.hex,
            narrow.art[b.art].height,
            kerb.art[a.art].height
        );
    }
    assert!(
        kerb_top > top_row(&plain) + 20,
        "the kerb cut: {kerb_top} vs {}",
        top_row(&plain)
    );
    // 35 units down to 16: about 9 pixels lower.
    assert!(
        narrow_top > kerb_top + 4,
        "narrow {narrow_top} vs kerb {kerb_top}"
    );
    // The same strips are cut, and still block every hex they blocked.
    let (k, n) = (
        &kerb.report.declutter.as_ref().unwrap().cutaway,
        &narrow.report.declutter.as_ref().unwrap().cutaway,
    );
    assert!(k.strips_cut > 0, "{k:?}");
    assert_eq!(n.strips_cut, k.strips_cut);
    assert_eq!(n.strips_considered, k.strips_considered);
    assert!(n.pixels_removed > k.pixels_removed, "{n:?} vs {k:?}");
    assert_eq!(blocks(&narrow), blocks(&kerb));
    assert_eq!(blocks(&narrow), footprint);
    let hexes = |s: &Sliced| -> Vec<(i32, bool)> {
        s.placements.iter().map(|p| (p.hex, p.blocking)).collect()
    };
    assert_eq!(hexes(&narrow), hexes(&kerb));

    // The plaza: the band behind the wall is wide, nothing changes.
    let wide = slice(&config(&plaza, None));
    let wide_narrow = slice(&config(&plaza, Some(16.0)));
    assert!(wide.report.declutter.as_ref().unwrap().cutaway.strips_cut > 0);
    assert_eq!(wide_narrow, wide);
}

/// The cutaway outdoors without `exteriors`: only the narrow kerb.
fn outdoor_config(walkers: &BTreeSet<i32>, narrow: Option<f32>) -> SlicerConfig {
    let mut config = config(walkers, narrow);
    if let Some(c) = config.declutter.as_mut().and_then(|d| d.cutaway.as_mut()) {
        c.exteriors = false;
    }
    config
}

#[test]
fn outdoors_the_narrow_kerb_alone_cuts_a_catwalk_and_leaves_a_plaza() {
    let plain = slice(&SlicerConfig::default());
    let footprint = blocks(&plain);
    let catwalk = walkers(&footprint, Some(2));
    let plaza = walkers(&footprint, None);
    // Without `exteriors` and without the narrow kerb: nothing is cut.
    let off = slice(&outdoor_config(&catwalk, None));
    let off_stats = &off.report.declutter.as_ref().unwrap().cutaway;
    assert_eq!(off_stats.strips_considered, 0, "{off_stats:?}");
    assert_eq!(off_stats.strips_cut, 0);
    // The catwalk: every strip beside it is cut to the narrow kerb, as the
    // cutaway with `exteriors` cuts it.
    let outdoors = slice(&outdoor_config(&catwalk, Some(16.0)));
    let everywhere = slice(&config(&catwalk, Some(16.0)));
    let stats = &outdoors.report.declutter.as_ref().unwrap().cutaway;
    assert!(stats.strips_cut > 0, "{stats:?}");
    assert_eq!(stats.strips_narrow, stats.strips_cut, "{stats:?}");
    assert_eq!(outdoors.placements, everywhere.placements);
    assert_eq!(outdoors.art, everywhere.art);
    assert!(top_row(&outdoors) > top_row(&off) + 20);
    assert_eq!(blocks(&outdoors), footprint);
    // The plaza: a wide band behind every strip, so none is cut; the art
    // is the uncut wall's.
    let wide = slice(&outdoor_config(&plaza, Some(16.0)));
    let wide_stats = &wide.report.declutter.as_ref().unwrap().cutaway;
    assert!(wide_stats.strips_considered > 0, "{wide_stats:?}");
    assert_eq!(wide_stats.strips_cut, 0, "{wide_stats:?}");
    assert_eq!(wide_stats.strips_narrow, 0);
    let uncut = slice(&outdoor_config(&plaza, None));
    assert_eq!(wide.placements, uncut.placements);
    assert_eq!(wide.art, uncut.art);
    // Deterministic.
    assert_eq!(slice(&outdoor_config(&catwalk, Some(16.0))), outdoors);
}

#[test]
fn the_narrow_kerb_off_is_byte_identical() {
    assert_eq!(Cutaway::default().narrow_kerb_height, None);
    assert_eq!(Cutaway::default().narrow_max_width, 3);
    assert_eq!(
        DeclutterProfile::readable()
            .cutaway
            .and_then(|c| c.narrow_kerb_height),
        None
    );
    let plain = slice(&SlicerConfig::default());
    let corridor = walkers(&blocks(&plain), Some(2));
    // Off with any width is the run without the rule.
    let off = slice(&config(&corridor, None));
    let mut wider = config(&corridor, None);
    if let Some(c) = wider.declutter.as_mut().and_then(|d| d.cutaway.as_mut()) {
        c.narrow_max_width = 9;
    }
    assert_eq!(slice(&wider), off);
    // And deterministic with it on.
    let on = config(&corridor, Some(16.0));
    assert_eq!(slice(&on), slice(&on));
}

#[test]
fn a_narrow_kerb_above_the_kerb_is_refused() {
    let plain = slice(&SlicerConfig::default());
    let corridor = walkers(&blocks(&plain), Some(2));
    for (narrow, width) in [(40.0, 3), (f32::NAN, 3), (-1.0, 3), (16.0, 0)] {
        let mut bad = config(&corridor, Some(narrow));
        if let Some(c) = bad.declutter.as_mut().and_then(|d| d.cutaway.as_mut()) {
            c.narrow_max_width = width;
        }
        let refused = run(&wall_assets(), &wall(), &flat(), &big_budget(), &bad);
        assert!(refused.is_err(), "{narrow} {width}");
    }
}
