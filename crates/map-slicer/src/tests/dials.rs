//! The readability dials the slicer applies: `wall_z_scale` (walls and
//! building shells drawn lower, about their anchor) and `item_scale` (an
//! item drawn narrower, so that it keeps its size on a spread-out map).
//! At their defaults nothing changes.

use super::*;
use crate::Role;

/// The drawn extent of every placement on a screen: `[left, top, right,
/// bottom]` of its opaque pixels.
fn extent(sliced: &Sliced) -> [i64; 4] {
    let mut out = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for p in &sliced.placements {
        let a = &sliced.art[p.art];
        let [left, top] = frame_origin(a, p);
        for y in 0..a.height as usize {
            for x in 0..a.width as usize {
                if a.pixels[y * a.width as usize + x] != 0 {
                    let (px, py) = (left + x as i64, top + y as i64);
                    out = [
                        out[0].min(px),
                        out[1].min(py),
                        out[2].max(px),
                        out[3].max(py),
                    ];
                }
            }
        }
    }
    out
}

#[test]
fn walls_are_drawn_lower_about_their_feet() {
    let assets = TestAssets::standard();
    let ground = flat();
    // A box-shaped wall: its topmost drawn point is its far top edge at
    // every scale, so the drop is linear in the scale (a pitched roof's
    // topmost point can change).
    let r = reference(
        1,
        "STAT",
        "meshes\\test\\fence.nif",
        "fence",
        &ground,
        [0.0, 0.0],
        0.0,
        0.0,
    );
    let refs = std::slice::from_ref(&r);
    let plain = run(
        &assets,
        refs,
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    // Every dial at its default is the default configuration.
    let same = SlicerConfig {
        wall_z_scale: 1.0,
        item_scale: 1.0,
        ..SlicerConfig::default()
    };
    let again = run(&assets, refs, &ground, &big_budget(), &same).unwrap();
    assert_eq!(again.art, plain.art);
    assert_eq!(again.placements, plain.placements);

    let half = SlicerConfig {
        wall_z_scale: 0.5,
        ..SlicerConfig::default()
    };
    let low = run(&assets, refs, &ground, &big_budget(), &half).unwrap();
    assert!(low.placements.iter().all(|p| p.role == Role::WallStrip));
    let quarter = SlicerConfig {
        wall_z_scale: 0.25,
        ..SlicerConfig::default()
    };
    let lower = run(&assets, refs, &ground, &big_budget(), &quarter).unwrap();
    let [l0, t0, r0, b0] = extent(&plain);
    let [l1, t1, r1, b1] = extent(&low);
    let [_, t2, _, b2] = extent(&lower);
    // The feet stay where they were, as do the sides; the top comes down
    // in proportion to the scale (linear about the anchor): at a quarter
    // it drops one and a half times what it drops at a half.
    assert!(
        (b1 - b0).abs() <= 1 && (b2 - b0).abs() <= 1,
        "bottom {b0} -> {b1}, {b2}"
    );
    assert!((l1 - l0).abs() <= 1 && (r1 - r0).abs() <= 1);
    let (half_drop, quarter_drop) = (t1 - t0, t2 - t0);
    // Half of the fence's 120 units: 60 units, about 27 pixels.
    assert!(
        (25..=30).contains(&half_drop),
        "the top came down {half_drop} pixels"
    );
    assert!(
        (quarter_drop * 2 - half_drop * 3).abs() <= 3,
        "drops {half_drop} at a half, {quarter_drop} at a quarter"
    );

    // A door or a sprite is never compressed: the same crate as a prop.
    let prop = crate::tests::slicing::crate_ref(2, &ground, [500.0, 0.0], 0.0);
    let one = std::slice::from_ref(&prop);
    let a = run(
        &assets,
        one,
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    let b = run(&assets, one, &ground, &big_budget(), &half).unwrap();
    assert_eq!(a.art, b.art);
}

#[test]
fn items_keep_their_size_on_a_spread_out_map() {
    let assets = TestAssets::standard();
    let ground = flat();
    let can = reference(
        3,
        "MISC",
        "meshes\\test\\crate.nif",
        "crate",
        &ground,
        [0.0, 0.0],
        0.0,
        0.0,
    );
    let refs = std::slice::from_ref(&can);
    let plain = run(
        &assets,
        refs,
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    let narrow = SlicerConfig {
        item_scale: 0.5,
        ..SlicerConfig::default()
    };
    let small = run(&assets, refs, &ground, &big_budget(), &narrow).unwrap();
    assert_eq!(small.placements[0].role, Role::Item);
    let [l0, t0, r0, b0] = extent(&plain);
    let [l1, t1, r1, b1] = extent(&small);
    // Half as wide; its height is not scaled, only its footprint's depth,
    // so the top and the front corner move in by a few pixels.
    let (w0, w1) = (r0 - l0 + 1, r1 - l1 + 1);
    assert!((w1 * 2 - w0).abs() <= 3, "width {w0} -> {w1}");
    assert!(b1 <= b0 && b0 - b1 <= 8, "bottom {b0} -> {b1}");
    assert!(t1 >= t0 && t1 - t0 <= 8, "top {t0} -> {t1}");
}
