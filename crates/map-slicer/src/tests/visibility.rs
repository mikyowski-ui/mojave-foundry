//! Landmark pieces against the engine's drawing rule (`engine_view`).

use crate::engine_view::{VIEW, view_origin, visited_hexes};
use crate::hex;
use crate::landmark::{Limits, hexes_in_rect};

/// Cameras spread over the grid, both parities, odd and even rows.
const CAMERAS: [i32; 10] = [
    20_100, 20_101, 20_300, 20_301, 15_042, 15_043, 25_555, 25_556, 8_080, 31_337,
];

/// The largest margins beyond each view edge (left, top, right, bottom)
/// inside which every hex centre is visited, each with the others at 0.
fn measured_margins(camera: i32) -> [i64; 4] {
    let visited = visited_hexes(camera);
    let v = view_origin(camera).unwrap();
    let [w, h] = VIEW;
    let unvisited: Vec<[i64; 2]> =
        hexes_in_rect([v[0] - 400, v[1] - 320, v[0] + w + 400, v[1] + h + 320])
            .into_iter()
            .filter(|x| !visited.contains(x))
            .filter_map(hex::center)
            .collect();
    let inside_y = |c: &[i64; 2]| (v[1]..v[1] + h).contains(&c[1]);
    let inside_x = |c: &[i64; 2]| (v[0]..v[0] + w).contains(&c[0]);
    // Per side: the distance beyond that edge of each unvisited centre
    // level with the view.
    let mut least = [400i64; 4];
    for c in &unvisited {
        let beyond = [
            (inside_y(c) && c[0] < v[0]).then(|| v[0] - c[0]),
            (inside_x(c) && c[1] < v[1]).then(|| v[1] - c[1]),
            (inside_y(c) && c[0] >= v[0] + w).then(|| c[0] - (v[0] + w - 1)),
            (inside_x(c) && c[1] >= v[1] + h).then(|| c[1] - (v[1] + h - 1)),
        ];
        for (side, d) in beyond.into_iter().enumerate() {
            if let Some(d) = d {
                least[side] = least[side].min(d);
            }
        }
    }
    least.map(|d| d - 1)
}

#[test]
fn the_default_reach_is_inside_the_engines_visited_window() {
    let limits = Limits::of(&crate::SlicerConfig::default());
    let [left, top, right, bottom] = limits.reach;
    let mut least = [i64::MAX; 4];
    let spread = (0..40_000).step_by(1_999).chain(CAMERAS);
    for camera in spread {
        let (row, column) = (camera / 200, camera % 200);
        if !(12..188).contains(&row) || !(12..188).contains(&column) {
            continue;
        }
        let m = measured_margins(camera);
        for side in 0..4 {
            least[side] = least[side].min(m[side]);
        }
        // Every centre in the configured rectangle, corners included.
        let visited = visited_hexes(camera);
        let v = view_origin(camera).unwrap();
        let rect = [
            v[0] - left,
            v[1] - top,
            v[0] + VIEW[0] - 1 + right,
            v[1] + VIEW[1] - 1 + bottom,
        ];
        for x in hexes_in_rect(rect) {
            assert!(visited.contains(&x), "camera {camera}: hex {x} not visited");
        }
    }
    eprintln!("measured margins left, top, right, bottom: {least:?}");
    assert!(least[0] >= left && least[1] >= top && least[2] >= right && least[3] >= bottom);
}

#[test]
fn hexes_in_a_rectangle_are_exactly_those_centred_in_it() {
    for rect in [
        [100, -40, 400, 200],
        [-50, 300, 30, 340],
        [2000, 900, 2600, 1300],
    ] {
        let fast = hexes_in_rect(rect);
        let mut slow = Vec::new();
        for x in 0..40_000 {
            let c = hex::center(x).unwrap();
            if (rect[0]..=rect[2]).contains(&c[0]) && (rect[1]..=rect[3]).contains(&c[1]) {
                slow.push(x);
            }
        }
        assert_eq!(fast, slow, "{rect:?}");
    }
}
