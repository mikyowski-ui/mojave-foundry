//! Placing the map window for the engine's view border
//! (`fit_view_border`, `crate::readability`).
//!
//! **The border** (vault-engine, `vault_core::ui::cursor::inside_border`,
//! mirrored here as `map_builder::inside_view_border`): a hex can be a
//! view centre only when its mirrored column (`199 - column`) lies in 44 to
//! 154 and its row in 45 to 154, on every map whatever it holds. The engine
//! refuses an entry hex outside it (the load fails and leaves the empty
//! map), never centres the view there (`set_view_center` keeps the last
//! centre, and the scroll keys and the camera's reach stop at it), but
//! objects outside it load, and the player and critters walk there as
//! anywhere: such ground is only seen from a centre inside, up to half a
//! screen away.
//!
//! **The fit.** The walkable extent is the bounding box of every navmesh
//! vertex of the window's cells and their neighbours (every elevation),
//! turned by the map's yaw, in source space. Today the window is centred on
//! the slice's centre. With the key on, the window is moved so that the
//! extent's centre is the border's centre (the bounding box of the border's
//! hex centres); when the extent is wider than the border on an axis it is
//! still centred and the overflow per side is reported. The turn stays
//! about the slice's centre: the window is placed after turning.
//!
//! **Spacing.** At `units_per_hex = u` the border is about 110 hexes, `110
//! u` units, on a side. [`ViewFit::smallest_units_per_hex`] is the
//! smallest `u` at which the extent fits, at the map's turn and at the best
//! turn in 5° steps.

use mir::{CellDoc, MapWindow};

/// What the fit found and did (`## View border`).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewFit {
    /// The window's centre before and after (turned source space).
    pub centre_before: [f32; 2],
    pub centre_after: [f32; 2],
    /// The walkable extent: `[west, south]`, `[east, north]` (turned
    /// source), and how many navmesh vertices it holds.
    pub extent: [[f64; 2]; 2],
    pub vertices: usize,
    /// The border's extent, in the window's source space before the move.
    pub border: [[f64; 2]; 2],
    /// Whether the extent fits inside the border on both axes.
    pub fits: bool,
    /// Hexes of the extent past the border after the move: west, east,
    /// north, south (0 where it fits).
    pub overflow: [f64; 4],
    /// The smallest units per hex at which the extent fits: at the map's
    /// turn, and the best turn (`world_yaw_deg`, 0 to 355 in 5° steps) with
    /// its value.
    pub smallest_units_per_hex: f64,
    pub best_yaw: (f64, f64),
    /// The map's turn and its units per hex, for the lines.
    pub world_yaw_deg: f64,
    pub units_per_hex: f64,
}

/// The bounding box of the border's hex centres in source space, for a
/// window placed as `window` under `projection`.
pub fn border_box(
    window: &MapWindow,
    projection: &foundry_render::Projection,
) -> Option<[[f64; 2]; 2]> {
    let frame = map_builder::walk::Frame::new(window, projection).ok()?;
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for hex in (0..40_000).filter(|h| map_builder::inside_view_border(*h)) {
        let Some(tile) = frame.hex_centre_tile(hex) else {
            continue;
        };
        let p = frame.source_of_tile(tile);
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo[0] <= hi[0]).then_some([lo, hi])
}

/// Every navmesh vertex of `cells` (turned already), in source space.
pub fn navmesh_points(cells: &[CellDoc]) -> Vec<[f64; 2]> {
    cells
        .iter()
        .flat_map(|c| &c.navmeshes)
        .flat_map(|n| &n.vertices)
        .filter(|v| v[0].is_finite() && v[1].is_finite())
        .map(|v| [f64::from(v[0]), f64::from(v[1])])
        .collect()
}

fn bounds(points: impl Iterator<Item = [f64; 2]>) -> Option<[[f64; 2]; 2]> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in points {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo[0] <= hi[0]).then_some([lo, hi])
}

/// The fit (module notes) of the walkable `points` (turned source) for a
/// window placed as `window`: the moved window's centre and the record.
/// `None` when there is no navmesh.
#[allow(clippy::too_many_arguments)]
pub fn fit(
    window: &MapWindow,
    projection: &foundry_render::Projection,
    points: &[[f64; 2]],
    turn_centre: [f64; 2],
    world_yaw_deg: f64,
    units_per_hex: f64,
) -> Option<ViewFit> {
    let extent = bounds(points.iter().copied())?;
    let border = border_box(window, projection)?;
    let size = |b: &[[f64; 2]; 2]| [b[1][0] - b[0][0], b[1][1] - b[0][1]];
    let centre = |b: &[[f64; 2]; 2]| [(b[0][0] + b[1][0]) / 2.0, (b[0][1] + b[1][1]) / 2.0];
    let (e, b) = (size(&extent), size(&border));
    let shift = {
        let (ec, bc) = (centre(&extent), centre(&border));
        [ec[0] - bc[0], ec[1] - bc[1]]
    };
    let before = [
        window.x_west + window.size * 0.5,
        window.y_north - window.size * 0.5,
    ];
    let after = [
        (f64::from(before[0]) + shift[0]) as f32,
        (f64::from(before[1]) + shift[1]) as f32,
    ];
    let hex_units = units_per_hex;
    let over = |extent: f64, border: f64| ((extent - border) / 2.0 / hex_units).max(0.0);
    let overflow = [
        over(e[0], b[0]),
        over(e[0], b[0]),
        over(e[1], b[1]),
        over(e[1], b[1]),
    ];
    // The border in hexes of `units_per_hex` units: its size scales with u.
    let per_unit = [b[0] / units_per_hex, b[1] / units_per_hex];
    let smallest = |ext: [f64; 2]| (ext[0] / per_unit[0]).max(ext[1] / per_unit[1]);
    let at_turn = smallest(e);
    let mut best = (world_yaw_deg.rem_euclid(360.0), at_turn);
    for step in 1..72 {
        let delta = f64::from(step) * 5.0;
        let (sin, cos) = (-delta.to_radians()).sin_cos();
        let turned = bounds(points.iter().map(|p| {
            let d = [p[0] - turn_centre[0], p[1] - turn_centre[1]];
            [d[0] * cos - d[1] * sin, d[0] * sin + d[1] * cos]
        }));
        if let Some(t) = turned {
            let u = smallest(size(&t));
            if u < best.1 - 1e-9 {
                best = ((world_yaw_deg + delta).rem_euclid(360.0), u);
            }
        }
    }
    Some(ViewFit {
        centre_before: before,
        centre_after: after,
        extent,
        vertices: points.len(),
        border,
        fits: e[0] <= b[0] && e[1] <= b[1],
        overflow,
        smallest_units_per_hex: at_turn,
        best_yaw: best,
        world_yaw_deg,
        units_per_hex,
    })
}

impl ViewFit {
    /// The report's lines (`## View border`).
    pub fn lines(&self) -> Vec<String> {
        let e = self.extent;
        let b = self.border;
        let mut lines = vec![
            format!(
                "walkable extent (navmesh of every elevation, {} vertices): {:.0} by {:.0} units ({:.0} by {:.0} hexes); the border holds {:.0} by {:.0} units ({:.0} by {:.0} hexes)",
                self.vertices,
                e[1][0] - e[0][0],
                e[1][1] - e[0][1],
                (e[1][0] - e[0][0]) / self.units_per_hex,
                (e[1][1] - e[0][1]) / self.units_per_hex,
                b[1][0] - b[0][0],
                b[1][1] - b[0][1],
                (b[1][0] - b[0][0]) / self.units_per_hex,
                (b[1][1] - b[0][1]) / self.units_per_hex
            ),
            format!(
                "window centre moved from ({:.0}, {:.0}) to ({:.0}, {:.0}) (turned source space): the extent centred in the border",
                self.centre_before[0],
                self.centre_before[1],
                self.centre_after[0],
                self.centre_after[1]
            ),
        ];
        if self.fits {
            lines.push("the extent fits inside the border".to_owned());
        } else {
            lines.push(format!(
                "the extent does not fit: centred, it overflows the border by {:.1} hexes west, {:.1} east, {:.1} north, {:.1} south",
                self.overflow[0], self.overflow[1], self.overflow[2], self.overflow[3]
            ));
        }
        lines.push(format!(
            "smallest units_per_hex at which the extent fits: {:.1} at world_yaw_deg {}; best turn {}° needs {:.1}",
            self.smallest_units_per_hex, self.world_yaw_deg, self.best_yaw.0, self.best_yaw.1
        ));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection() -> foundry_render::Projection {
        foundry_render::Projection::new(&foundry_render::Calibration::default()).unwrap()
    }

    #[test]
    fn the_border_is_about_110_hexes_square_inside_the_window() {
        let window = MapWindow::from_center([0.0, 0.0], 140.0);
        let b = border_box(&window, &projection()).unwrap();
        let (w, h) = (b[1][0] - b[0][0], b[1][1] - b[0][1]);
        // 70 units a hex.
        assert!((w / 70.0 - 110.0).abs() < 2.0, "{w}");
        assert!((h / 70.0 - 109.0).abs() < 2.0, "{h}");
        assert!(b[0][0] > window.x_west as f64 && b[1][1] < window.y_north as f64);
    }

    #[test]
    fn an_extent_off_centre_is_moved_into_the_border() {
        let window = MapWindow::from_center([0.0, 0.0], 140.0);
        let projection = projection();
        let border = border_box(&window, &projection).unwrap();
        // A town 3,000 units square in the window's north-east corner.
        let points: Vec<[f64; 2]> = vec![[3_000.0, 3_000.0], [6_000.0, 6_000.0]];
        let fit = fit(&window, &projection, &points, [0.0, 0.0], 0.0, 70.0).unwrap();
        assert!(fit.fits);
        assert_eq!(fit.overflow, [0.0; 4]);
        let moved = MapWindow::from_center(fit.centre_after, 140.0);
        let after = border_box(&moved, &projection).unwrap();
        let centre = |b: [[f64; 2]; 2]| [(b[0][0] + b[1][0]) / 2.0, (b[0][1] + b[1][1]) / 2.0];
        let c = centre(after);
        assert!(
            (c[0] - 4_500.0).abs() < 1.0 && (c[1] - 4_500.0).abs() < 1.0,
            "{c:?}"
        );
        assert!(border[1][0] < 6_000.0, "it did not fit before");
        // 3,000 units over 110 hexes: about 27 units a hex.
        assert!((fit.smallest_units_per_hex - 3_000.0 / 109.0).abs() < 1.0);
        // Too wide: centred, the overflow reported on both sides.
        let wide = vec![[-6_000.0, 0.0], [6_000.0, 100.0]];
        let fit = super::fit(&window, &projection, &wide, [0.0, 0.0], 0.0, 70.0).unwrap();
        assert!(!fit.fits);
        assert!(fit.overflow[0] > 10.0 && fit.overflow[0] == fit.overflow[1]);
        assert_eq!(fit.overflow[2], 0.0);
        // A long east-west strip fits a square border best laid along its
        // diagonal: turned by 45 degrees.
        let strip: Vec<[f64; 2]> = (0..=10).map(|i| [f64::from(i) * 400.0, 2_000.0]).collect();
        let fit = super::fit(&window, &projection, &strip, [2_000.0, 2_000.0], 0.0, 70.0).unwrap();
        assert!(fit.best_yaw.1 < fit.smallest_units_per_hex * 0.8, "{fit:?}");
        assert!(
            [45.0, 135.0, 225.0, 315.0].contains(&fit.best_yaw.0),
            "{fit:?}"
        );
    }
}
