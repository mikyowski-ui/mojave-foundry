//! Small matrix and quaternion helpers for posing and skinning.
//!
//! Conventions, the same as [`crate::Transform`]:
//!
//! - a [`Matrix34`] is three rows of four: `p'[r] = m[r][0]·x + m[r][1]·y +
//!   m[r][2]·z + m[r][3]`; the left 3 × 3 block is rotation times scale,
//!   the last column the translation;
//! - `mul(a, b)` applies `b` first, then `a`;
//! - a quaternion is `[w, x, y, z]`. [`quat_to_rotation`] gives the rows of
//!   the rotation it stands for under the row convention above; for a unit
//!   quaternion about +Z by `θ` that is a counter-clockwise turn of `θ`
//!   seen from above (+X turns towards +Y).

use crate::mesh::Transform;

/// An affine transform: three rows of four. See the module documentation.
pub type Matrix34 = [[f32; 4]; 3];

/// The matrix that changes nothing.
pub const IDENTITY34: Matrix34 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// The matrix of a transform: `p' = R · p · scale + t`.
pub fn transform_matrix(t: &Transform) -> Matrix34 {
    let r = &t.rotation;
    let s = t.scale;
    let mut m = [[0.0f32; 4]; 3];
    for (row, out) in m.iter_mut().enumerate() {
        *out = [
            r[row][0] * s,
            r[row][1] * s,
            r[row][2] * s,
            t.translation[row],
        ];
    }
    m
}

/// `a · b`: applies `b` first, then `a`.
pub fn mul(a: &Matrix34, b: &Matrix34) -> Matrix34 {
    let mut out = [[0.0f32; 4]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let mut v = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
            if c == 3 {
                v += a[r][3];
            }
            *cell = v;
        }
    }
    out
}

/// Transforms a point.
pub fn apply_point(m: &Matrix34, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
}

/// Transforms a direction: the 3 × 3 block only.
pub fn apply_vector(m: &Matrix34, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

/// The inverse, or `None` when the 3 × 3 block is singular or the result
/// is not finite.
pub fn inverse(m: &Matrix34) -> Option<Matrix34> {
    let a = |r: usize, c: usize| f64::from(m[r][c]);
    let c00 = a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1);
    let c01 = a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2);
    let c02 = a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0);
    let det = a(0, 0) * c00 + a(0, 1) * c01 + a(0, 2) * c02;
    if !det.is_finite() || det.abs() < 1e-30 {
        return None;
    }
    let inv = 1.0 / det;
    let r = [
        [
            c00 * inv,
            (a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2)) * inv,
            (a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1)) * inv,
        ],
        [
            c01 * inv,
            (a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0)) * inv,
            (a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2)) * inv,
        ],
        [
            c02 * inv,
            (a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1)) * inv,
            (a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0)) * inv,
        ],
    ];
    let t = [a(0, 3), a(1, 3), a(2, 3)];
    let mut out = [[0.0f32; 4]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = r[row][col] as f32;
        }
        out[row][3] = (-(r[row][0] * t[0] + r[row][1] * t[1] + r[row][2] * t[2])) as f32;
    }
    out.iter().flatten().all(|v| v.is_finite()).then_some(out)
}

/// The quaternion scaled to unit length; the identity when it has no
/// length or is not finite.
pub fn normalize_quat(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if len.is_finite() && len > 1e-12 {
        q.map(|v| v / len)
    } else {
        [1.0, 0.0, 0.0, 0.0]
    }
}

/// Rows of the rotation a unit quaternion `[w, x, y, z]` stands for.
pub fn quat_to_rotation(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [w, x, y, z] = normalize_quat(q);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// The unit quaternion of a rotation matrix (rows), with `w ≥ 0`.
pub fn rotation_to_quat(m: &[[f32; 3]; 3]) -> [f32; 4] {
    let trace = m[0][0] + m[1][1] + m[2][2];
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            0.25 * s,
            (m[2][1] - m[1][2]) / s,
            (m[0][2] - m[2][0]) / s,
            (m[1][0] - m[0][1]) / s,
        ]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [
            (m[2][1] - m[1][2]) / s,
            0.25 * s,
            (m[0][1] + m[1][0]) / s,
            (m[0][2] + m[2][0]) / s,
        ]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [
            (m[0][2] - m[2][0]) / s,
            (m[0][1] + m[1][0]) / s,
            0.25 * s,
            (m[1][2] + m[2][1]) / s,
        ]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [
            (m[1][0] - m[0][1]) / s,
            (m[0][2] + m[2][0]) / s,
            (m[1][2] + m[2][1]) / s,
            0.25 * s,
        ]
    };
    let q = normalize_quat(q);
    if q[0] < 0.0 { q.map(|v| -v) } else { q }
}

/// The Hamilton product `a · b`: the rotation `b` first, then `a`.
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [aw, ax, ay, az] = a;
    let [bw, bx, by, bz] = b;
    [
        aw * bw - ax * bx - ay * by - az * bz,
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
    ]
}

/// A rotation of `radians` about a unit axis.
pub fn quat_axis_angle(axis: [f32; 3], radians: f32) -> [f32; 4] {
    let (s, c) = (radians / 2.0).sin_cos();
    [c, axis[0] * s, axis[1] * s, axis[2] * s]
}

/// Spherical interpolation on the shorter arc: `t = 0` gives `a`, `t = 1`
/// gives `b` (or `-b`, the same rotation). The result has unit length.
pub fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let a = normalize_quat(a);
    let mut b = normalize_quat(b);
    let mut dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    if dot < 0.0 {
        b = b.map(|v| -v);
        dot = -dot;
    }
    if dot > 0.9995 {
        return normalize_quat(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
    }
    let theta = dot.min(1.0).acos();
    let sin = theta.sin();
    let wa = ((1.0 - t) * theta).sin() / sin;
    let wb = (t * theta).sin() / sin;
    normalize_quat(std::array::from_fn(|i| a[i] * wa + b[i] * wb))
}
