//! Conversions between the `nif` matrix conventions and rigid poses.
//!
//! Only `+ - * /` and `sqrt` are used here (no transcendental functions),
//! so the results are IEEE-exact and do not depend on the platform's libm.

use nif::math::Matrix34;
use rapier3d::math::{Mat3, Pose, Rotation, Vector};

/// The origin (translation column) of a bone matrix.
pub(crate) fn origin(m: &Matrix34) -> Vector {
    Vector::new(m[0][3], m[1][3], m[2][3])
}

/// Column `c` of the 3 × 3 block: where the bone's local axis `c` points.
pub(crate) fn column(m: &Matrix34, c: usize) -> Vector {
    Vector::new(m[0][c], m[1][c], m[2][c])
}

/// Whether every entry is finite.
pub(crate) fn finite_matrix(m: &Matrix34) -> bool {
    m.iter().flatten().all(|v| v.is_finite())
}

/// The unit vector along `v`, or `None` when it is (nearly) zero or not
/// finite.
pub(crate) fn unit(v: Vector) -> Option<Vector> {
    let len = v.length();
    (len.is_finite() && len > 1e-6).then(|| v / len)
}

/// `v` with its component along the unit vector `axis` removed, normalized.
pub(crate) fn perpendicular(v: Vector, axis: Vector) -> Option<Vector> {
    unit(v - axis * v.dot(axis))
}

/// A rotation whose columns are the orthonormal frame `(x, y, x × y)`;
/// `y` is made perpendicular to `x` first.
pub(crate) fn frame(x: Vector, y: Vector) -> Option<Rotation> {
    let x = unit(x)?;
    let y = perpendicular(y, x)?;
    let z = x.cross(y);
    let q = Rotation::from_mat3(&Mat3::from_cols(x, y, z)).normalize();
    q.is_finite().then_some(q)
}

/// The rigid part of a bone matrix: its origin, and its rotation with the
/// scale and any shear removed (Gram–Schmidt on the X then Y axes).
/// `None` for a degenerate or non-finite matrix.
pub(crate) fn rigid(m: &Matrix34) -> Option<Pose> {
    if !finite_matrix(m) {
        return None;
    }
    let rotation = frame(column(m, 0), column(m, 1))?;
    Some(Pose::from_parts(origin(m), rotation))
}

/// The matrix of a rigid pose, in the `nif` row convention.
pub(crate) fn matrix(p: &Pose) -> Matrix34 {
    let r = Mat3::from_quat(p.rotation);
    let t = p.translation;
    let mut m = [[0.0f32; 4]; 3];
    for (row, out) in m.iter_mut().enumerate() {
        *out = [r.x_axis[row], r.y_axis[row], r.z_axis[row], t[row]];
    }
    m
}

/// `p` with its translation multiplied by `s` (a change of length unit).
pub(crate) fn scaled(p: &Pose, s: f32) -> Pose {
    Pose::from_parts(p.translation * s, p.rotation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigid_round_trips_through_matrix() {
        let q = Rotation::from_mat3(&Mat3::from_cols(
            Vector::new(0.0, 1.0, 0.0),
            Vector::new(-1.0, 0.0, 0.0),
            Vector::new(0.0, 0.0, 1.0),
        ));
        let p = Pose::from_parts(Vector::new(1.0, 2.0, 3.0), q);
        let back = rigid(&matrix(&p)).unwrap();
        assert!((back.translation - p.translation).length() < 1e-6);
        assert!(back.rotation.dot(p.rotation).abs() > 0.999_999);
    }

    #[test]
    fn rigid_removes_scale() {
        let mut m = matrix(&Pose::IDENTITY);
        for row in &mut m {
            for v in &mut row[..3] {
                *v *= 2.5;
            }
        }
        let p = rigid(&m).unwrap();
        assert!(p.rotation.dot(Rotation::IDENTITY).abs() > 0.999_999);
        assert!(rigid(&[[0.0; 4]; 3]).is_none());
        assert!(rigid(&[[f32::NAN; 4]; 3]).is_none());
    }
}
