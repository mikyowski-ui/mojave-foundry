//! The oblique parallel projection of `docs/contracts.md` section 4.
//!
//! The 2 × 3 screen matrix is read from `HexGrid::square_corner`
//! differences, never written out by hand. Depth is the coordinate along the
//! matrix's null space, measured in metres (tile and storey axes scaled to
//! metres first, then projected onto the unit view direction). Together the
//! screen rows and the depth row form an invertible 3 × 3 map, so
//! [`Projection::unproject`] is exact up to float rounding.

use vault_formats::geometry::HexGrid;

use crate::{Calibration, RenderError};

/// Square index used to read the screen vectors: any interior square works,
/// because the grid is affine.
const PROBE_SQUARE: i32 = 5050;

/// Tile space to screen and depth, and source space to tile space.
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    /// Rows `screen_x`, `screen_y`, `depth`, each acting on `(s, t, e)`.
    forward: [[f64; 3]; 3],
    /// Inverse of `forward`.
    inverse: [[f64; 3]; 3],
    /// The three screen vectors, the columns of the screen matrix.
    vectors: [[f64; 2]; 3],
    /// Null-space direction in tile space, reduced to small integers.
    view: [f64; 3],
    tile_units: f64,
    storey_units: f64,
    storey_metres: f64,
    tile_metres: f64,
    yaw: u8,
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn det3(m: &[[f64; 3]; 3]) -> f64 {
    dot(m[0], cross(m[1], m[2]))
}

/// Cofactor matrix: `cof(m) = det(m) · m^{-T}`.
pub(crate) fn cofactor(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    // Rows of the cofactor matrix are cross products of the rows.
    [cross(m[1], m[2]), cross(m[2], m[0]), cross(m[0], m[1])]
}

fn invert(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = det3(m);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let cof = cofactor(m);
    // inverse = cof^T / det
    let mut out = [[0.0; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, value) in row.iter_mut().enumerate() {
            *value = cof[c][r] / det;
        }
    }
    Some(out)
}

impl Projection {
    /// The projection for a calibration, read from the default grid.
    pub fn new(calibration: &Calibration) -> Result<Self, RenderError> {
        Self::with_grid(calibration, HexGrid::default())
    }

    /// The projection for a calibration, read from `grid`. The grid's
    /// origin cancels out; only corner differences are used.
    pub fn with_grid(calibration: &Calibration, grid: HexGrid) -> Result<Self, RenderError> {
        calibration.validate()?;
        let corner = |square: i32, roof: bool| {
            grid.square_corner(square, roof)
                .ok_or_else(|| RenderError::Projection(format!("square {square} out of range")))
        };
        let base = corner(PROBE_SQUARE, false)?;
        let diff = |p: [i64; 2]| [(p[0] - base[0]) as f64, (p[1] - base[1]) as f64];
        // s = 99 - square % 100: one step in +s is one square index lower;
        // t = square / 100: one step in +t is 100 indices higher; one storey
        // up is the roof corner of the same square.
        let vectors = [
            diff(corner(PROBE_SQUARE - 1, false)?),
            diff(corner(PROBE_SQUARE + 100, false)?),
            diff(corner(PROBE_SQUARE, true)?),
        ];
        let row_x = [vectors[0][0], vectors[1][0], vectors[2][0]];
        let row_y = [vectors[0][1], vectors[1][1], vectors[2][1]];
        let mut view = cross(row_x, row_y);
        // Reduce to the smallest integer direction and point it downwards:
        // farther from a viewer above the map.
        let scale = [view[0], view[1], view[2]]
            .iter()
            .map(|v| v.abs())
            .filter(|v| *v > 0.0)
            .fold(f64::INFINITY, f64::min);
        if !scale.is_finite() {
            return Err(RenderError::Projection(
                "screen vectors are degenerate".into(),
            ));
        }
        let gcd = gcd3(view.map(|v| v.round() as i64));
        let divisor = if gcd > 0 { gcd as f64 } else { scale };
        let sign = if view[2] > 0.0 { -1.0 } else { 1.0 };
        view = view.map(|v| sign * v / divisor);

        let storey_pixels = vectors[2][1].abs();
        let tile_metres = f64::from(calibration.tile_metres);
        let storey_metres = storey_pixels / f64::from(calibration.pixels_per_metre_vertical);
        let units = f64::from(calibration.units_per_metre);
        let metric = [tile_metres, tile_metres, storey_metres];
        let view_metric = [0, 1, 2].map(|i| view[i] * metric[i]);
        let length = dot(view_metric, view_metric).sqrt();
        let depth_row = [0, 1, 2].map(|i| metric[i] * view_metric[i] / length);
        let forward = [row_x, row_y, depth_row];
        let inverse = invert(&forward)
            .ok_or_else(|| RenderError::Projection("projection is not invertible".into()))?;
        Ok(Projection {
            forward,
            inverse,
            vectors,
            view,
            tile_units: tile_metres * units,
            storey_units: storey_metres * units,
            storey_metres,
            tile_metres,
            yaw: calibration.yaw_quarter_turns.rem_euclid(4) as u8,
        })
    }

    /// The screen vectors of one step in `s`, `t` and `e`.
    pub fn screen_vectors(&self) -> [[f32; 2]; 3] {
        self.vectors.map(|v| v.map(|c| c as f32))
    }

    /// The view direction in tile space `(s, t, e)`, as small integers:
    /// `(2, -3, -1)` for the Fallout 2 grid. Larger depth is farther along
    /// it.
    pub fn view_direction(&self) -> [f32; 3] {
        self.view.map(|v| v as f32)
    }

    /// Source units per floor square.
    pub fn tile_units(&self) -> f32 {
        self.tile_units as f32
    }

    /// Source units per storey.
    pub fn storey_units(&self) -> f32 {
        self.storey_units as f32
    }

    /// Metres per storey.
    pub fn storey_metres(&self) -> f32 {
        self.storey_metres as f32
    }

    /// Source point to tile space `(s, t, e)` for a window anchored at
    /// `anchor = (x_west, y_north, ground_z)`, with the yaw applied.
    pub fn tile_from_source(&self, anchor: [f32; 3], p: [f32; 3]) -> [f32; 3] {
        let (linear, offset) = self.source_to_tile(anchor);
        let p = p.map(f64::from);
        [0, 1, 2].map(|r| (dot(linear[r], p) + offset[r]) as f32)
    }

    /// Tile space to screen pixels, with the projected anchor at `(0, 0)`.
    pub fn screen(&self, ste: [f32; 3]) -> [f32; 2] {
        let p = ste.map(f64::from);
        [
            dot(self.forward[0], p) as f32,
            dot(self.forward[1], p) as f32,
        ]
    }

    /// Tile space to depth in metres along the view direction; larger is
    /// farther.
    pub fn depth(&self, ste: [f32; 3]) -> f32 {
        dot(self.forward[2], ste.map(f64::from)) as f32
    }

    /// The tile-space point with this screen position and depth: the exact
    /// inverse of [`Projection::screen`] and [`Projection::depth`].
    pub fn unproject(&self, screen: [f32; 2], depth: f32) -> [f32; 3] {
        let v = [f64::from(screen[0]), f64::from(screen[1]), f64::from(depth)];
        [0, 1, 2].map(|r| dot(self.inverse[r], v) as f32)
    }

    /// Linear part and offset of source → tile space.
    pub(crate) fn source_to_tile(&self, anchor: [f32; 3]) -> ([[f64; 3]; 3], [f64; 3]) {
        // Counter-clockwise quarter turns about Z of the source offset.
        let (c, s) = match self.yaw {
            0 => (1.0, 0.0),
            1 => (0.0, 1.0),
            2 => (-1.0, 0.0),
            _ => (0.0, -1.0),
        };
        let yaw = [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]];
        // s = x' / tile, t = -y' / tile, e = z / storey.
        let axes = [
            1.0 / self.tile_units,
            -1.0 / self.tile_units,
            1.0 / self.storey_units,
        ];
        let linear = [0, 1, 2].map(|r| yaw[r].map(|v| v * axes[r]));
        let a = anchor.map(f64::from);
        let offset = [0, 1, 2].map(|r| -dot(linear[r], a));
        (linear, offset)
    }

    /// Rows `screen_x`, `screen_y`, `depth` acting on tile space.
    pub(crate) fn forward(&self) -> &[[f64; 3]; 3] {
        &self.forward
    }

    /// Per-axis metres of tile space: tile, tile, storey.
    pub(crate) fn metric(&self) -> [f64; 3] {
        [self.tile_metres, self.tile_metres, self.storey_metres]
    }

    /// Sign of `(row_x × row_y) · view`: relates screen winding to facing.
    pub(crate) fn winding_sign(&self) -> f64 {
        let k = dot(cross(self.forward[0], self.forward[1]), self.view);
        if k < 0.0 { -1.0 } else { 1.0 }
    }
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn gcd3(v: [i64; 3]) -> i64 {
    gcd(gcd(v[0], v[1]), v[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection() -> Projection {
        Projection::new(&Calibration::default()).expect("default projection")
    }

    #[test]
    fn screen_vectors_come_from_the_grid() {
        let p = projection();
        assert_eq!(
            p.screen_vectors(),
            [[48.0, -12.0], [32.0, 24.0], [0.0, -96.0]]
        );
        assert_eq!(p.view_direction(), [2.0, -3.0, -1.0]);
        assert_eq!(p.tile_units(), 140.0);
        assert_eq!(p.storey_units(), 210.0);
        assert_eq!(p.storey_metres(), 3.0);
    }

    #[test]
    fn grid_origin_does_not_matter() {
        let shifted = HexGrid {
            origin: [1234, -567],
        };
        let p = Projection::with_grid(&Calibration::default(), shifted).expect("projection");
        assert_eq!(p, projection());
    }

    #[test]
    fn one_storey_up_moves_96_pixels() {
        let p = projection();
        let a = p.screen([3.25, 7.5, 0.5]);
        let b = p.screen([3.25, 7.5, 1.5]);
        assert_eq!([b[0] - a[0], b[1] - a[1]], [0.0, -96.0]);
        // Through source space: one storey of source units up.
        let anchor = [1000.0, 2000.0, 50.0];
        let ground = p.tile_from_source(anchor, [1100.0, 1900.0, 50.0]);
        let up = p.tile_from_source(anchor, [1100.0, 1900.0, 260.0]);
        let (sa, sb) = (p.screen(ground), p.screen(up));
        assert!((sb[0] - sa[0]).abs() < 1e-4 && (sb[1] - sa[1] + 96.0).abs() < 1e-4);
    }

    #[test]
    fn tile_from_source_follows_the_contract() {
        let p = projection();
        let anchor = [4096.0, 8192.0, 10.0];
        // East one tile, south two tiles, up one storey.
        let ste = p.tile_from_source(anchor, [4096.0 + 140.0, 8192.0 - 280.0, 220.0]);
        assert!((ste[0] - 1.0).abs() < 1e-6 && (ste[1] - 2.0).abs() < 1e-6);
        assert!((ste[2] - 1.0).abs() < 1e-6);
        assert_eq!(p.screen([0.0, 0.0, 0.0]), [0.0, 0.0]);
    }

    #[test]
    fn yaw_turns_the_window() {
        let calibration = Calibration {
            yaw_quarter_turns: 1,
            ..Calibration::default()
        };
        let p = Projection::new(&calibration).expect("projection");
        // One tile east turns into one tile north: t = -1.
        let ste = p.tile_from_source([0.0; 3], [140.0, 0.0, 0.0]);
        assert!(ste[0].abs() < 1e-6 && (ste[1] + 1.0).abs() < 1e-6);
        let minus_three = Calibration {
            yaw_quarter_turns: -3,
            ..Calibration::default()
        };
        assert_eq!(Projection::new(&minus_three).expect("projection"), p);
    }

    #[test]
    fn view_direction_is_the_null_space() {
        let p = projection();
        let d = p.view_direction();
        assert_eq!(p.screen(d), [0.0, 0.0]);
        // Moving along d goes farther; the depth step is |d| in metres.
        let step = p.depth(d) - p.depth([0.0; 3]);
        let metres = ((2.0f32 * 2.0).powi(2) + (3.0f32 * 2.0).powi(2) + 3.0f32.powi(2)).sqrt();
        assert!((step - metres).abs() < 1e-4, "{step} vs {metres}");
        // A raised point is nearer.
        assert!(p.depth([0.0, 0.0, 1.0]) < p.depth([0.0; 3]));
    }

    #[test]
    fn unproject_inverts_screen_and_depth() {
        let p = projection();
        for ste in [
            [0.0, 0.0, 0.0],
            [12.5, -3.25, 0.75],
            [99.0, 99.0, 4.0],
            [-7.0, 40.0, -2.5],
        ] {
            let back = p.unproject(p.screen(ste), p.depth(ste));
            for (a, b) in back.iter().zip(&ste) {
                assert!((a - b).abs() < 1e-4, "{back:?} vs {ste:?}");
            }
        }
    }
}
