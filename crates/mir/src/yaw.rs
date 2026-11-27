//! A turn of the source world about the vertical axis: the readability dial
//! `world_yaw_deg` of the map stage.
//!
//! [`WorldYaw`] is the one transform every consumer of source positions and
//! orientations in the map pipeline goes through. It turns a point about a
//! fixed centre (the map window's), keeps heights, and adds the turn to a
//! reference's yaw. Positive angles are **clockwise seen from above**, the
//! sense of a placed reference's own rotation (`docs/contracts.md` section
//! 1): at 90° east turns south, so [`WorldYaw::apply_rotation`] is the
//! stored `rz` plus the turn.
//!
//! Quarter turns (0°, 90°, 180°, 270°, any multiple of 90° given exactly)
//! use exact sines and cosines (0 and ±1): a point's offset from the centre
//! is swapped and negated in `f64`, then rounded once to `f32`, so nothing
//! drifts. Any other angle goes through `sin_cos` in `f64`. A turn of 0° is
//! the identity: every method returns its input unchanged, bit for bit.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use crate::model::{CellDoc, Reference, TeleportDoc, Transform};

/// A clockwise turn by [`WorldYaw::degrees`] about the vertical axis through
/// [`WorldYaw::centre`] (module notes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldYaw {
    centre: [f64; 2],
    /// In `[0, 360)`.
    degrees: f64,
    cos: f64,
    sin: f64,
    /// `Some(k)` for a turn of exactly `90·k` degrees.
    quarter: Option<u8>,
}

impl Default for WorldYaw {
    fn default() -> Self {
        Self::identity()
    }
}

impl WorldYaw {
    /// No turn.
    pub fn identity() -> Self {
        Self {
            centre: [0.0; 2],
            degrees: 0.0,
            cos: 1.0,
            sin: 0.0,
            quarter: Some(0),
        }
    }

    /// A clockwise turn of `degrees` (any finite value, taken modulo 360)
    /// about `centre`. A non-finite angle is no turn.
    pub fn new(centre: [f32; 2], degrees: f64) -> Self {
        let degrees = if degrees.is_finite() {
            let d = degrees.rem_euclid(360.0);
            // `rem_euclid` of a tiny negative angle can round to 360.
            if d >= 360.0 { 0.0 } else { d }
        } else {
            0.0
        };
        let quarter = [0.0, 90.0, 180.0, 270.0]
            .iter()
            .position(|q| *q == degrees)
            .map(|k| k as u8);
        let (sin, cos) = match quarter {
            Some(0) => (0.0, 1.0),
            Some(1) => (1.0, 0.0),
            Some(2) => (0.0, -1.0),
            Some(_) => (-1.0, 0.0),
            None => degrees.to_radians().sin_cos(),
        };
        Self {
            centre: centre.map(f64::from),
            degrees,
            cos,
            sin,
            quarter,
        }
    }

    /// Whether this is no turn at all.
    pub fn is_identity(&self) -> bool {
        self.quarter == Some(0)
    }

    /// The turn, clockwise seen from above, in `[0, 360)`.
    pub fn degrees(&self) -> f64 {
        self.degrees
    }

    /// The number of quarter turns, when the angle is exactly one.
    pub fn quarter_turns(&self) -> Option<u8> {
        self.quarter
    }

    /// The point the world turns about (source `x`, `y`).
    pub fn centre(&self) -> [f64; 2] {
        self.centre
    }

    /// The turn in radians, clockwise: exact multiples of `π/2` for quarter
    /// turns.
    pub fn radians(&self) -> f64 {
        match self.quarter {
            Some(k) => f64::from(k) * FRAC_PI_2,
            None => self.degrees.to_radians(),
        }
    }

    /// The opposite turn about the same centre.
    pub fn inverse(&self) -> Self {
        if self.is_identity() {
            return *self;
        }
        let degrees = 360.0 - self.degrees;
        Self {
            centre: self.centre,
            degrees: if degrees >= 360.0 { 0.0 } else { degrees },
            cos: self.cos,
            sin: -self.sin,
            quarter: self.quarter.map(|k| (4 - k) % 4),
        }
    }

    /// A horizontal offset turned, in `f64`.
    pub fn apply_offset(&self, d: [f64; 2]) -> [f64; 2] {
        [
            self.cos * d[0] + self.sin * d[1],
            -self.sin * d[0] + self.cos * d[1],
        ]
    }

    /// A ground point turned about the centre, in `f64`.
    pub fn apply_xy64(&self, x: f64, y: f64) -> [f64; 2] {
        if self.is_identity() {
            return [x, y];
        }
        let [dx, dy] = self.apply_offset([x - self.centre[0], y - self.centre[1]]);
        [self.centre[0] + dx, self.centre[1] + dy]
    }

    /// A ground point turned about the centre.
    pub fn apply_xy(&self, x: f32, y: f32) -> [f32; 2] {
        if self.is_identity() {
            return [x, y];
        }
        self.apply_xy64(f64::from(x), f64::from(y))
            .map(|v| v as f32)
    }

    /// A point turned about the vertical axis through the centre; its height
    /// is kept.
    pub fn apply(&self, point: [f32; 3]) -> [f32; 3] {
        let [x, y] = self.apply_xy(point[0], point[1]);
        [x, y, point[2]]
    }

    /// A direction (or a normal) turned; its vertical part is kept.
    pub fn apply_direction(&self, d: [f32; 3]) -> [f32; 3] {
        if self.is_identity() {
            return d;
        }
        let [x, y] = self.apply_offset([f64::from(d[0]), f64::from(d[1])]);
        [x as f32, y as f32, d[2]]
    }

    /// A reference's yaw (rotation about Z, radians, clockwise) plus the
    /// turn, wrapped to `(-π, π]`. Unchanged, unwrapped, for no turn.
    pub fn apply_yaw(&self, rz: f32) -> f32 {
        if self.is_identity() || !rz.is_finite() {
            return rz;
        }
        let mut a = (f64::from(rz) + self.radians() + PI).rem_euclid(TAU) - PI;
        if a <= -PI {
            a += TAU;
        }
        a as f32
    }

    /// A reference's rotation `(rx, ry, rz)` with the turn added to `rz`:
    /// exact, because `Transform::matrix` applies `Rz` last (outermost).
    pub fn apply_rotation(&self, rotation: [f32; 3]) -> [f32; 3] {
        [rotation[0], rotation[1], self.apply_yaw(rotation[2])]
    }

    /// A placed transform turned: its position and its rotation.
    pub fn apply_transform(&self, transform: &Transform) -> Transform {
        if self.is_identity() {
            return *transform;
        }
        Transform {
            position: self.apply(transform.position),
            rotation: self.apply_rotation(transform.rotation),
            scale: transform.scale,
        }
    }

    /// Turns a reference's transform. Its teleport is left alone: a
    /// teleport's arrival lies in the space of the map it leads to, which
    /// [`WorldYaw::turn_teleport`] turns with that map's yaw. A primitive's
    /// bounds are local and follow the rotation.
    pub fn turn_reference(&self, reference: &mut Reference) {
        if !self.is_identity() {
            reference.transform = self.apply_transform(&reference.transform);
        }
    }

    /// Turns a teleport's arrival point and rotation (for a teleport that
    /// leads into the map this turn belongs to).
    pub fn turn_teleport(&self, teleport: &mut TeleportDoc) {
        if self.is_identity() {
            return;
        }
        teleport.position = self.apply(teleport.position);
        teleport.rotation = self.apply_rotation(teleport.rotation);
    }

    /// Turns a cell document: every reference (not its teleport) and every
    /// navmesh vertex. The land grid is not resampled: it is read through
    /// the inverse turn ([`crate::GroundSampler::turned`], and the terrain
    /// bake does the same).
    pub fn turn_cell(&self, cell: &mut CellDoc) {
        if self.is_identity() {
            return;
        }
        for reference in &mut cell.references {
            self.turn_reference(reference);
        }
        for navmesh in &mut cell.navmeshes {
            for vertex in &mut navmesh.vertices {
                *vertex = self.apply(*vertex);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quarter_turn_is_exact_and_clockwise() {
        let yaw = WorldYaw::new([1000.0, -2000.0], 90.0);
        assert_eq!(yaw.quarter_turns(), Some(1));
        // East of the centre goes south of it.
        assert_eq!(yaw.apply([1100.0, -2000.0, 7.0]), [1000.0, -2100.0, 7.0]);
        // North goes east.
        assert_eq!(yaw.apply_xy(1000.0, -1950.0), [1050.0, -2000.0]);
        assert_eq!(yaw.apply_direction([0.0, 1.0, 0.5]), [1.0, 0.0, 0.5]);
        for k in 0..4 {
            let turn = WorldYaw::new([1000.0, -2000.0], 90.0 * f64::from(k));
            let p = [1234.5, -1876.25, 3.0];
            let mut q = p;
            for _ in 0..4 {
                q = turn.apply(q);
            }
            assert_eq!(q, p, "four turns of {} degrees", 90 * k);
            assert_eq!(turn.inverse().apply(turn.apply(p)), p);
        }
    }

    #[test]
    fn angles_wrap_and_zero_is_the_identity() {
        assert!(WorldYaw::new([5.0, 5.0], 0.0).is_identity());
        assert!(WorldYaw::new([5.0, 5.0], 360.0).is_identity());
        assert!(WorldYaw::new([5.0, 5.0], f64::NAN).is_identity());
        assert_eq!(WorldYaw::new([0.0; 2], -90.0).quarter_turns(), Some(3));
        assert_eq!(WorldYaw::new([0.0; 2], 450.0).quarter_turns(), Some(1));
        assert_eq!(WorldYaw::new([0.0; 2], 45.0).quarter_turns(), None);
        let identity = WorldYaw::identity();
        // Bit for bit, even out of range.
        assert_eq!(identity.apply_yaw(7.5).to_bits(), 7.5f32.to_bits());
        assert_eq!(identity.apply([1.1, 2.2, 3.3]), [1.1, 2.2, 3.3]);
    }

    #[test]
    fn the_yaw_follows_the_turn_of_the_points() {
        // A reference turned by the world faces where its turned points lie.
        for degrees in [90.0, 180.0, 270.0, 45.0, 123.0] {
            let yaw = WorldYaw::new([0.0, 0.0], degrees);
            let transform = Transform {
                position: [300.0, -40.0, 12.0],
                rotation: [0.0, 0.0, 0.7],
                scale: 1.5,
            };
            let turned = yaw.apply_transform(&transform);
            let local = [10.0, 20.0, 5.0];
            let a = yaw.apply(transform.apply(local));
            let b = turned.apply(local);
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() < 1e-3, "{degrees}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn inverse_undoes_any_turn() {
        let yaw = WorldYaw::new([-71_680.0, 2_048.0], 37.5);
        let p = [-70_000.0, 3_000.0, 9.0];
        let back = yaw.inverse().apply(yaw.apply(p));
        assert!((back[0] - p[0]).abs() < 0.01 && (back[1] - p[1]).abs() < 0.01);
        let a = yaw.apply_yaw(3.0);
        assert!(a > -std::f32::consts::PI && a <= std::f32::consts::PI);
        let round = yaw.inverse().apply_yaw(a);
        assert!((round - 3.0).abs() < 1e-5);
    }
}
