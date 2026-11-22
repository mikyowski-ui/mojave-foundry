//! A model of which objects the engine draws in its 640 × 380 map view,
//! for checks only: nothing in the slicer's output depends on it.
//!
//! vault-engine (`vault-render`, `map/objects.rs`: `hex_under`, `lattice`,
//! `visited_hexes`, `draw_sequence`) draws an object only when its hex is
//! one the refresh visits: from the hex under the view point
//! `(-320, -240)`, a lattice of 40 columns by 71 rows of points 32 pixels
//! apart, rows 12 pixels apart and staggered by 16, chosen by the parity of
//! the camera hex. The object's pixels play no part. The `enhanced` canvas
//! adds an overscan by sprite extent; the Vanilla 640 × 480 view does not.
//! This is a port of that rule, so tests and the `slicer_check` example can
//! confirm that a landmark piece is drawn whenever one of its pixels shows.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use vault_formats::geometry::HexGrid;

/// The map view under the 640 × 480 interface, pixels.
pub const VIEW: [i64; 2] = [640, 380];

/// The hex under a view point by the engine's picking rule, without the
/// grid bounds (a port of the engine's `hex_under`).
fn hex_under(grid: HexGrid, [x, y]: [i64; 2]) -> i64 {
    let y = y - grid.origin[1];
    let k = y.div_euclid(12);
    let v = x - grid.origin[0] - 16 * k;
    let (b, r) = (v.div_euclid(64), v.rem_euclid(64));
    let mut column = 2 * b + i64::from(r >= 32);
    let mut row = b + k;
    let (lx, ly) = (r % 32, y - 12 * k);
    if ly < 4 {
        if lx < 16 - 4 * ly {
            row -= 1;
        } else if lx > 16 + 4 * ly {
            column += 1;
            if column.rem_euclid(2) == 1 {
                row -= 1;
            }
        }
    }
    199 - column + 200 * row
}

/// The visiting lattice for each parity of the camera hex: hex offsets
/// from the corner hex, with their lattice row and column.
fn lattice() -> &'static [Vec<(i64, u8, u8)>; 2] {
    static LATTICE: OnceLock<[Vec<(i64, u8, u8)>; 2]> = OnceLock::new();
    LATTICE.get_or_init(|| {
        std::array::from_fn(|parity| {
            let grid =
                HexGrid::centered(20_100 + parity as i32, 640, 380).expect("hex on the grid");
            let corner = hex_under(grid, [-320, -240]);
            let [x0, y0] = grid
                .hex_center(corner as i32)
                .expect("corner hex on the grid");
            let step = if x0 > -320 { -16 } else { 16 };
            let mut points = Vec::with_capacity(41 * 72);
            for row in 0..72_i64 {
                let first = match row {
                    0 => x0,
                    _ if row % 2 == 1 => x0 + step,
                    _ => x0 - step,
                };
                for column in 0..41_i64 {
                    let hex = hex_under(grid, [first + 32 * column, y0 + 12 * row]);
                    points.push((hex - corner, row as u8, column as u8));
                }
            }
            points
        })
    })
}

/// Map screen position of view pixel `(0, 0)` with the camera on `camera`.
pub fn view_origin(camera: i32) -> Option<[i64; 2]> {
    let grid = HexGrid::centered(camera, VIEW[0] as u32, VIEW[1] as u32).ok()?;
    Some([-grid.origin[0], -grid.origin[1]])
}

/// The hexes a refresh visits with the camera on `camera`: the objects on
/// them are drawn, all others are not.
pub fn visited_hexes(camera: i32) -> BTreeSet<i32> {
    let Ok(grid) = HexGrid::centered(camera, VIEW[0] as u32, VIEW[1] as u32) else {
        return BTreeSet::new();
    };
    let (width, height) = (VIEW[0], VIEW[1]);
    let center = hex_under(
        grid,
        [
            (width - 32).div_euclid(2) + 16,
            (height - 16).div_euclid(2) + 8,
        ],
    );
    let base = hex_under(grid, [-320, -240]);
    let (rows, columns) = ((height + 480) / 12, (width + 640) / 32);
    lattice()[center.rem_euclid(2) as usize]
        .iter()
        .filter(|(_, row, column)| i64::from(*row) < rows && i64::from(*column) < columns)
        .filter_map(|(offset, ..)| i32::try_from(base + offset).ok())
        .filter(|hex| (0..40_000).contains(hex))
        .collect()
}
