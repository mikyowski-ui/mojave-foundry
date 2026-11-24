//! The hexes a map stage would open: hex centres within a tolerance of a
//! navmesh triangle, as `map_builder::walk::standable_hexes` rasterises
//! them (an approximation for the examples: the slicer must not depend on
//! the builder).

use std::collections::BTreeSet;

use foundry_render::Projection;
use mir::{CellDoc, GroundSampler, MapWindow};
use vault_formats::geometry::HexGrid;

/// Source `(x, y)` of every hex centre on the ground plane, by hex.
pub fn hex_sources(projection: &Projection, window: &MapWindow) -> Vec<Option<[f64; 2]>> {
    let anchor = [window.x_west, window.y_north, 0.0];
    let at = |x: f32, y: f32| {
        let s = projection.screen(projection.tile_from_source(anchor, [x, y, 0.0]));
        [f64::from(s[0]), f64::from(s[1])]
    };
    // Screen = o + x * a + y * b; invert the 2 x 2 map.
    let (x0, y0) = (window.x_west, window.y_north);
    let o = at(x0, y0);
    let a = {
        let p = at(x0 + 1000.0, y0);
        [(p[0] - o[0]) / 1000.0, (p[1] - o[1]) / 1000.0]
    };
    let b = {
        let p = at(x0, y0 + 1000.0);
        [(p[0] - o[0]) / 1000.0, (p[1] - o[1]) / 1000.0]
    };
    let det = a[0] * b[1] - a[1] * b[0];
    let grid = HexGrid::default();
    (0..40_000)
        .map(|h| {
            let c = grid.hex_center(h).ok()?;
            let d = [c[0] as f64 - o[0], c[1] as f64 - o[1]];
            let x = (d[0] * b[1] - d[1] * b[0]) / det;
            let y = (a[0] * d[1] - a[1] * d[0]) / det;
            Some([f64::from(x0) + x, f64::from(y0) + y])
        })
        .collect()
}

fn distance_to_triangle(p: [f64; 2], t: [[f64; 2]; 3]) -> f64 {
    let side =
        |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let d = [side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0])];
    if d.iter().all(|v| *v >= 0.0) || d.iter().all(|v| *v <= 0.0) {
        return 0.0;
    }
    (0..3)
        .map(|i| {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            let ab = [b[0] - a[0], b[1] - a[1]];
            let len = ab[0] * ab[0] + ab[1] * ab[1];
            let s = if len > 0.0 {
                (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = [a[0] + s * ab[0], a[1] + s * ab[1]];
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
        })
        .fold(f64::INFINITY, f64::min)
}

/// Hexes whose centre lies within `tolerance` units of a navmesh triangle
/// whose corners all lie in `band` above `floor` (interiors), or, with
/// `floor` `None`, not all more than `band[1]` above the terrain
/// (exteriors).
pub fn open_hexes(
    docs: &[CellDoc],
    centres: &[Option<[f64; 2]>],
    sampler: &GroundSampler,
    floor: Option<f32>,
    band: [f32; 2],
    tolerance: f64,
) -> BTreeSet<i32> {
    const BIN: f64 = 128.0;
    let mut bins: std::collections::BTreeMap<(i64, i64), Vec<usize>> = Default::default();
    for (h, p) in centres.iter().enumerate() {
        if let Some(p) = p {
            let key = ((p[0] / BIN).floor() as i64, (p[1] / BIN).floor() as i64);
            bins.entry(key).or_default().push(h);
        }
    }
    let mut out = BTreeSet::new();
    for doc in docs {
        for n in &doc.navmeshes {
            for t in 0..n.triangles.len() {
                let Some(c) = n.corners(t) else { continue };
                let keep = match floor {
                    Some(z) => c.iter().all(|v| v[2] >= z + band[0] && v[2] <= z + band[1]),
                    None => !c.iter().all(|v| {
                        sampler
                            .height(v[0], v[1])
                            .is_some_and(|g| v[2] - g > band[1])
                    }),
                };
                if !keep {
                    continue;
                }
                let tri = c.map(|v| [f64::from(v[0]), f64::from(v[1])]);
                let lo =
                    [0, 1].map(|k| tri.iter().map(|p| p[k]).fold(f64::MAX, f64::min) - tolerance);
                let hi =
                    [0, 1].map(|k| tri.iter().map(|p| p[k]).fold(f64::MIN, f64::max) + tolerance);
                let b0 = [(lo[0] / BIN).floor() as i64, (lo[1] / BIN).floor() as i64];
                let b1 = [(hi[0] / BIN).floor() as i64, (hi[1] / BIN).floor() as i64];
                for bx in b0[0]..=b1[0] {
                    for by in b0[1]..=b1[1] {
                        for &h in bins.get(&(bx, by)).map(Vec::as_slice).unwrap_or(&[]) {
                            let Some(p) = centres[h] else { continue };
                            if distance_to_triangle(p, tri) <= tolerance {
                                out.insert(h as i32);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}
