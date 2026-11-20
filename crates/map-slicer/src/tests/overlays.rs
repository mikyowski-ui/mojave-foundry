//! Flat and relief overlays land at the right map screen pixels.

use std::collections::BTreeSet;

use super::*;
use crate::BakedOverlay;
use crate::hex::Plane;

/// Opaque bounding box of a set of overlays, and a lookup.
fn opaque_box(overlays: &[BakedOverlay]) -> [i64; 4] {
    let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for o in overlays {
        for y in 0..o.height as usize {
            for x in 0..o.width as usize {
                if o.rgba[(y * o.width as usize + x) * 4 + 3] != 0 {
                    let (px, py) = (
                        i64::from(o.origin[0]) + x as i64,
                        i64::from(o.origin[1]) + y as i64,
                    );
                    b = [b[0].min(px), b[1].min(py), b[2].max(px), b[3].max(py)];
                }
            }
        }
    }
    b
}

fn opaque_at(overlays: &[BakedOverlay], p: [i64; 2]) -> bool {
    overlays.iter().any(|o| {
        let (x, y) = (p[0] - i64::from(o.origin[0]), p[1] - i64::from(o.origin[1]));
        x >= 0
            && y >= 0
            && (x as u32) < o.width
            && (y as u32) < o.height
            && o.rgba[(y as usize * o.width as usize + x as usize) * 4 + 3] != 0
    })
}

/// Screen bounding box of source points.
fn projected(points: &[[f32; 3]], ground_z: f32) -> [f64; 4] {
    let p = projection();
    let anchor = [window().x_west, window().y_north, ground_z];
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for q in points {
        let s = p.screen(p.tile_from_source(anchor, *q));
        b = [
            b[0].min(f64::from(s[0])),
            b[1].min(f64::from(s[1])),
            b[2].max(f64::from(s[0])),
            b[3].max(f64::from(s[1])),
        ];
    }
    b
}

fn close(a: [i64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(&b).all(|(a, b)| (*a as f64 - b).abs() <= 2.0)
}

#[test]
fn a_flat_mesh_lands_flat_on_the_ground() {
    let assets = TestAssets::standard();
    let ground = flat();
    // Lifted 20 units: flattening puts it on the ground anyway.
    let r = reference(
        1,
        "STAT",
        "meshes\\test\\strip.nif",
        "road",
        &ground,
        [350.0, -420.0],
        20.0,
        0.0,
    );
    let sliced = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(sliced.placements.is_empty() && sliced.art.is_empty());
    assert!(!sliced.overlays.is_empty());
    assert_eq!(
        sliced.report.classes[&crate::ClassKind::GroundFlat].references,
        1
    );
    let [x, y, _] = r.transform.position;
    let corners: Vec<[f32; 3]> = [
        [-280.0, -140.0],
        [280.0, -140.0],
        [280.0, 140.0],
        [-280.0, 140.0],
    ]
    .iter()
    .map(|c| [x + c[0], y + c[1], 100.0])
    .collect();
    let want = projected(&corners, 100.0);
    let got = opaque_box(&sliced.overlays);
    assert!(close(got, want), "overlay {got:?} vs projected {want:?}");
    let centre = projected(&[[x, y, 100.0]], 100.0);
    assert!(opaque_at(
        &sliced.overlays,
        [centre[0] as i64, centre[1] as i64]
    ));
    assert!(sliced.blocked_hexes.is_empty());
}

#[test]
fn relief_lands_in_relief_and_blocks_its_footprint() {
    let assets = TestAssets::standard();
    let ground = flat();
    let r = reference(
        1,
        "STAT",
        "meshes\\test\\rocks.nif",
        "slab",
        &ground,
        [-700.0, 560.0],
        0.0,
        0.0,
    );
    let sliced = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(sliced.placements.is_empty());
    let [x, y, _] = r.transform.position;
    let mut corners = Vec::new();
    for dx in [-350.0, 350.0] {
        for dy in [-350.0, 350.0] {
            for z in [100.0, 300.0] {
                corners.push([x + dx, y + dy, z]);
            }
        }
    }
    let want = projected(&corners, 100.0);
    let got = opaque_box(&sliced.overlays);
    assert!(close(got, want), "overlay {got:?} vs projected {want:?}");

    let plane = Plane::new(&projection(), window().x_west, window().y_north).unwrap();
    let square: Vec<[f64; 2]> = [
        [-350.0, -350.0],
        [350.0, -350.0],
        [350.0, 350.0],
        [-350.0, 350.0],
    ]
    .iter()
    .map(|c| plane.tile_of_source([f64::from(x) + c[0], f64::from(y) + c[1]]))
    .collect();
    let want: BTreeSet<i32> = plane.hexes_in(&square).into_iter().collect();
    let got: BTreeSet<i32> = sliced.blocked_hexes.iter().copied().collect();
    assert_eq!(want.len(), 100);
    assert_eq!(got, want);
}

#[test]
fn a_huge_overlay_comes_back_in_tiles() {
    let assets = TestAssets::standard();
    let ground = flat();
    let r = reference(
        1,
        "STAT",
        "meshes\\test\\plateau.nif",
        "huge",
        &ground,
        [0.0, 0.0],
        0.0,
        0.0,
    );
    let config = SlicerConfig {
        render_tile: 512,
        ..SlicerConfig::default()
    };
    let sliced = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    assert!(sliced.overlays.len() > 4);
    assert!(
        sliced
            .overlays
            .iter()
            .all(|o| o.width <= 512 && o.height <= 512)
    );
    assert!(sliced.report.blocked_hexes > 4000);
}
