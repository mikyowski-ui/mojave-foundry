//! Map screen space, the hex lattice and ground-plane geometry.
//!
//! Map screen space is `HexGrid::default()` (contracts section 9): origin
//! zero, the same space `Projection::screen` maps tile space into. Picking
//! always goes through `HexGrid::pick_hex`; nothing here re-derives it.
//!
//! **Engine draw order.** The engine draws the objects of one elevation in
//! ascending hex number (`column + 200 · row`), then list order (vault-engine
//! SPEC-022 section 3.4, SPEC-014 section 3.44). Ascending hex number is
//! row-major: every hex of row `r` before any hex of row `r + 1`, and within
//! a row by ascending column. On screen a row runs from upper right to lower
//! left, and the next row lies one step `(16, 12)` lower right; for two
//! ground hexes whose screen positions overlap horizontally, the one nearer
//! the viewer always has the larger hex number. Strips owned by the ground
//! hex their pixels stand over therefore draw back to front.

use foundry_render::Projection;
use vault_formats::geometry::HexGrid;

/// Map screen space.
pub(crate) const GRID: HexGrid = HexGrid { origin: [0, 0] };

/// Hex columns and rows of a map.
pub(crate) const HEX_SIDE: i32 = 200;

/// The hex under a map screen point, or `None` off the map.
pub(crate) fn pick(point: [f64; 2]) -> Option<i32> {
    let x = point[0].floor();
    let y = point[1].floor();
    if !(x.is_finite() && y.is_finite()) || x.abs() > 1.0e9 || y.abs() > 1.0e9 {
        return None;
    }
    GRID.pick_hex([x as i32, y as i32])
}

/// The screen centre of a hex: where the engine anchors an object.
pub(crate) fn center(hex: i32) -> Option<[i64; 2]> {
    GRID.hex_center(hex).ok()
}

/// Row and column of a hex.
pub(crate) fn row_column(hex: i32) -> (i32, i32) {
    (hex / HEX_SIDE, hex % HEX_SIDE)
}

/// The six neighbours, `None` where the map ends.
pub(crate) fn neighbours(hex: i32) -> [Option<i32>; 6] {
    std::array::from_fn(|o| HexGrid::neighbor(hex, o as u8).ok().flatten())
}

/// Hex distance on the lattice; large for invalid input.
pub(crate) fn distance(a: i32, b: i32) -> i32 {
    HexGrid::lattice_distance(a, b).unwrap_or(i32::MAX)
}

/// Ground-plane conversions between tile space `(s, t)`, map screen space
/// and source space, read from one projection and window anchor.
#[derive(Debug, Clone)]
pub(crate) struct Plane {
    /// Inverse of the ground screen matrix (columns: the `s` and `t`
    /// screen vectors).
    inverse: [[f64; 2]; 2],
    /// Tile `(s, t)` = `to_tile · [x, y] + tile_offset`.
    to_tile: [[f64; 2]; 2],
    tile_offset: [f64; 2],
    /// Source `(x, y)` = `to_source · [s, t] + source_offset`.
    to_source: [[f64; 2]; 2],
    source_offset: [f64; 2],
}

fn invert2(m: [[f64; 2]; 2]) -> Option<[[f64; 2]; 2]> {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    (det.is_finite() && det.abs() > 1.0e-12).then(|| {
        [
            [m[1][1] / det, -m[0][1] / det],
            [-m[1][0] / det, m[0][0] / det],
        ]
    })
}

impl Plane {
    /// The plane of a window anchored at `(x_west, y_north)`.
    pub fn new(projection: &Projection, x_west: f32, y_north: f32) -> Option<Self> {
        let vectors = projection.screen_vectors();
        let screen = [
            [f64::from(vectors[0][0]), f64::from(vectors[1][0])],
            [f64::from(vectors[0][1]), f64::from(vectors[1][1])],
        ];
        let inverse = invert2(screen)?;
        let anchor = [x_west, y_north, 0.0];
        let at = |x: f32, y: f32| projection.tile_from_source(anchor, [x, y, 0.0]);
        // The map from source to tile is affine; read it from three points
        // (unit steps, so the window anchor's magnitude costs no precision
        // in the differences beyond f32 rounding of the anchor itself).
        let origin = at(x_west, y_north);
        let east = at(x_west + 1000.0, y_north);
        let north = at(x_west, y_north + 1000.0);
        let to_tile = [
            [
                f64::from(east[0] - origin[0]) / 1000.0,
                f64::from(north[0] - origin[0]) / 1000.0,
            ],
            [
                f64::from(east[1] - origin[1]) / 1000.0,
                f64::from(north[1] - origin[1]) / 1000.0,
            ],
        ];
        let (xw, yn) = (f64::from(x_west), f64::from(y_north));
        let tile_offset = [
            f64::from(origin[0]) - to_tile[0][0] * xw - to_tile[0][1] * yn,
            f64::from(origin[1]) - to_tile[1][0] * xw - to_tile[1][1] * yn,
        ];
        let to_source = invert2(to_tile)?;
        let source_offset = [
            -(to_source[0][0] * tile_offset[0] + to_source[0][1] * tile_offset[1]),
            -(to_source[1][0] * tile_offset[0] + to_source[1][1] * tile_offset[1]),
        ];
        Some(Plane {
            inverse,
            to_tile,
            tile_offset,
            to_source,
            source_offset,
        })
    }

    /// Map screen point, read on the ground, to tile space.
    pub fn tile_of_screen(&self, p: [f64; 2]) -> [f64; 2] {
        [
            self.inverse[0][0] * p[0] + self.inverse[0][1] * p[1],
            self.inverse[1][0] * p[0] + self.inverse[1][1] * p[1],
        ]
    }

    /// Source `(x, y)` to ground tile point.
    pub fn tile_of_source(&self, xy: [f64; 2]) -> [f64; 2] {
        [
            self.to_tile[0][0] * xy[0] + self.to_tile[0][1] * xy[1] + self.tile_offset[0],
            self.to_tile[1][0] * xy[0] + self.to_tile[1][1] * xy[1] + self.tile_offset[1],
        ]
    }

    /// Ground tile point to source `(x, y)`.
    pub fn source_of_tile(&self, st: [f64; 2]) -> [f64; 2] {
        [
            self.to_source[0][0] * st[0] + self.to_source[0][1] * st[1] + self.source_offset[0],
            self.to_source[1][0] * st[0] + self.to_source[1][1] * st[1] + self.source_offset[1],
        ]
    }

    /// The tile point of a hex centre.
    pub fn hex_tile(&self, hex: i32) -> Option<[f64; 2]> {
        let c = center(hex)?;
        Some(self.tile_of_screen([c[0] as f64, c[1] as f64]))
    }

    /// Every hex whose centre lies inside a convex polygon given in tile
    /// space (counter-clockwise or clockwise), sorted.
    pub fn hexes_in(&self, polygon: &[[f64; 2]]) -> Vec<i32> {
        let mut out = Vec::new();
        if polygon.len() < 3 {
            return out;
        }
        let (mut s0, mut t0, mut s1, mut t1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in polygon {
            s0 = s0.min(p[0]);
            t0 = t0.min(p[1]);
            s1 = s1.max(p[0]);
            t1 = t1.max(p[1]);
        }
        if !(s0.is_finite() && t0.is_finite() && s1.is_finite() && t1.is_finite()) {
            return out;
        }
        // Hex (h = 199 - column, row) is centred near (h / 2, row / 2 + 1/2).
        let clamp = |v: f64| v.clamp(-1.0, f64::from(HEX_SIDE)) as i32;
        let h_range = clamp((2.0 * s0).floor() - 2.0)..=clamp((2.0 * s1).ceil() + 2.0);
        let r_range = clamp((2.0 * t0).floor() - 3.0)..=clamp((2.0 * t1).ceil() + 2.0);
        let sign = orientation(polygon);
        for row in r_range {
            if !(0..HEX_SIDE).contains(&row) {
                continue;
            }
            for h in h_range.clone() {
                if !(0..HEX_SIDE).contains(&h) {
                    continue;
                }
                let hex = (HEX_SIDE - 1 - h) + HEX_SIDE * row;
                if let Some(c) = self.hex_tile(hex)
                    && inside_convex(polygon, sign, c)
                {
                    out.push(hex);
                }
            }
        }
        out.sort_unstable();
        out
    }
}

/// +1 for counter-clockwise, -1 for clockwise.
fn orientation(polygon: &[[f64; 2]]) -> f64 {
    if signed_area(polygon) >= 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// Shoelace area, positive for counter-clockwise.
pub(crate) fn signed_area(polygon: &[[f64; 2]]) -> f64 {
    let n = polygon.len();
    let mut area = 0.0;
    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        area += a[0] * b[1] - a[1] * b[0];
    }
    area * 0.5
}

fn inside_convex(polygon: &[[f64; 2]], sign: f64, p: [f64; 2]) -> bool {
    let n = polygon.len();
    (0..n).all(|i| {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        cross * sign >= 0.0
    })
}

/// Convex hull (monotone chain), counter-clockwise, without repeats.
pub(crate) fn convex_hull(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = points
        .iter()
        .copied()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// The map's ground area in screen space, `[x0, y0, x1, y1)`: the bounding
/// box of every floor tile and every hex.
pub(crate) fn map_screen_bounds() -> [i64; 4] {
    let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for square in [0, 99, 9900, 9999] {
        if let Some(c) = GRID.square_corner(square, false) {
            b = [
                b[0].min(c[0]),
                b[1].min(c[1]),
                b[2].max(c[0] + 80),
                b[3].max(c[1] + 36),
            ];
        }
    }
    for hex in [
        0,
        HEX_SIDE - 1,
        HEX_SIDE * (HEX_SIDE - 1),
        HEX_SIDE * HEX_SIDE - 1,
    ] {
        if let Some(c) = center(hex) {
            b = [
                b[0].min(c[0] - 16),
                b[1].min(c[1] - 8),
                b[2].max(c[0] + 17),
                b[3].max(c[1] + 9),
            ];
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use foundry_render::Calibration;

    fn plane() -> Plane {
        let projection = Projection::new(&Calibration::default()).unwrap();
        Plane::new(&projection, -7000.0, 7000.0).unwrap()
    }

    #[test]
    fn the_plane_round_trips() {
        let p = plane();
        let st = p.tile_of_source([-7000.0 + 140.0 * 3.5, 7000.0 - 140.0 * 2.0]);
        assert!((st[0] - 3.5).abs() < 1e-4 && (st[1] - 2.0).abs() < 1e-4);
        let back = p.source_of_tile(st);
        assert!((back[0] + 6510.0).abs() < 1e-2 && (back[1] - 6720.0).abs() < 1e-2);
        let st = p.tile_of_screen([80.0, 12.0]);
        assert!((st[0] - 1.0).abs() < 1e-9 && (st[1] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn hex_centres_pick_their_own_hex() {
        for hex in [0, 1, 199, 200, 201, 20_100, 39_999, 12_345] {
            let c = center(hex).unwrap();
            assert_eq!(pick([c[0] as f64, c[1] as f64]), Some(hex));
        }
    }

    #[test]
    fn a_square_holds_four_hexes() {
        let p = plane();
        let square = [[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 20.0]];
        let hexes = p.hexes_in(&square);
        assert_eq!(hexes.len(), 400);
        assert!(hexes.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn the_hull_is_counter_clockwise() {
        let hull = convex_hull(&[[0.0, 0.0], [1.0, 0.0], [0.5, 0.2], [1.0, 1.0], [0.0, 1.0]]);
        assert_eq!(hull.len(), 4);
        assert!(signed_area(&hull) > 0.99);
    }

    #[test]
    fn the_map_bounds_hold_every_hex_centre() {
        let b = map_screen_bounds();
        for hex in [0, 199, 39_800, 39_999] {
            let c = center(hex).unwrap();
            assert!(c[0] >= b[0] && c[0] < b[2] && c[1] >= b[1] && c[1] < b[3]);
        }
    }
}
