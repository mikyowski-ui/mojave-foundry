//! What a ragdoll is made of: which bones become rigid bodies, their
//! shapes, masses and joints, and the world they fall in.
//!
//! Every length in a [`RagdollSpec`] that describes the figure is a
//! fraction of the figure's height, so one spec fits any size of skeleton;
//! every physical quantity is in SI units (metres, kilograms, seconds),
//! and [`RagdollSpec::units_per_meter`] converts the skeleton's own units.

use nif::Skeleton;

use crate::RagdollError;

/// Source units in one metre by default: a figure 128 units tall (the
/// human skeletons of the source game) behaves like a 1.8 m person, so one
/// unit is about 1.4 cm.
pub const DEFAULT_UNITS_PER_METER: f32 = 128.0 / 1.8;

/// The most rigid bodies a ragdoll may have. Each body owns one collision
/// group bit, and one bit is the ground's.
pub const MAX_BODIES: usize = 31;

/// A direction, either fixed to a bone or to the figure, always taken in
/// the skeleton's rest pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// The bone's own local X axis (along the bone in the `Bip01`
    /// convention).
    BoneX,
    /// The bone's own local Y axis.
    BoneY,
    /// The bone's own local Z axis.
    BoneZ,
    /// The figure's up ([`RagdollSpec::up`]).
    Up,
    /// The figure's down.
    Down,
    /// The way the figure faces ([`RagdollSpec::forward`]).
    Forward,
    /// The figure's back.
    Backward,
    /// The figure's right hand side: forward × up.
    Right,
    /// The figure's left hand side.
    Left,
}

/// Where a body's segment ends when the bone that should end it is
/// missing or too close: `length` (a fraction of the figure height) along
/// `direction`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fallback {
    /// The direction from the body's bone origin.
    pub direction: Axis,
    /// The segment length, a fraction of the figure height.
    pub length: f32,
}

/// The segment a body's collision shape is built around, in the rest pose.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    /// From the body's bone origin to the origin of the first of these
    /// bones that exists (and is not too close).
    ToBone {
        /// Candidate end bones, first found wins.
        bones: Vec<String>,
        /// Used when none of them exists.
        fallback: Fallback,
    },
    /// From the origin of the first found of `a` to the first found of `b`
    /// (the pelvis: across the two hip joints).
    Between {
        /// Candidates for one end.
        a: Vec<String>,
        /// Candidates for the other end.
        b: Vec<String>,
        /// Used, from the body's bone origin, when either end is missing.
        fallback: Fallback,
    },
    /// Always the fallback.
    Along(Fallback),
}

/// A body's collision shape. Sizes are fractions of the figure height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeSpec {
    /// A capsule along the segment.
    Capsule {
        /// Radius.
        radius: f32,
    },
    /// A box along the segment, as long as the segment, `half_width` to
    /// either side along the figure's right axis and `half_depth` front
    /// and back.
    Box {
        /// Half extent across the figure.
        half_width: f32,
        /// Half extent front to back.
        half_depth: f32,
    },
    /// A ball at the middle of the segment.
    Ball {
        /// Radius.
        radius: f32,
    },
}

/// How a body hangs from its parent body. Angles are in degrees.
///
/// Each joint has a frame at the child bone's origin, fixed in the rest
/// pose: X along the child's segment (the twist axis), Y the direction
/// [`JointSpec::flex_toward`] made perpendicular to X, Z = X × Y.
/// *Flex* is the rotation about Z (positive moves the child towards Y),
/// *side* the rotation about Y (positive moves the child towards −Z) and
/// *twist* the rotation about X. All are measured from the rest pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JointKind {
    /// Three rotational degrees of freedom, each limited.
    Ball {
        /// Flex range `[min, max]`.
        flex: [f32; 2],
        /// Side-bend range `[min, max]`.
        side: [f32; 2],
        /// Twist range `[min, max]`.
        twist: [f32; 2],
    },
    /// Flex only (elbows, knees): side and twist are locked.
    Hinge {
        /// Flex range `[min, max]`; one-sided ranges keep knees and
        /// elbows from bending backwards.
        flex: [f32; 2],
    },
    /// No freedom at all: the child is welded to its parent.
    Fixed,
}

/// A joint: its kind and frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointSpec {
    /// Degrees of freedom and limits, written for the figure's right side.
    pub kind: JointKind,
    /// The direction the child's far end moves on positive flex.
    pub flex_toward: Axis,
    /// A left-side joint: the side and twist ranges are mirrored (negated
    /// and swapped), so one set of numbers serves both sides.
    pub mirror: bool,
}

/// One rigid body of the ragdoll.
#[derive(Debug, Clone, PartialEq)]
pub struct BodySpec {
    /// The body's name, for parents and impulses (`"pelvis"`, `"chest"`).
    pub key: String,
    /// Candidate bone names, first found wins ([`Skeleton::find`]: exact,
    /// then ignoring ASCII case).
    pub bones: Vec<String>,
    /// The key of the parent body; `None` only for the root (the first
    /// body). When the parent is missing from a skeleton the body hangs
    /// from the nearest ancestor body that exists.
    pub parent: Option<String>,
    /// The segment the shape is built around.
    pub segment: Segment,
    /// The collision shape.
    pub shape: ShapeSpec,
    /// Share of the total mass (relative: the shares of the bodies that
    /// exist are normalized).
    pub mass: f32,
    /// The joint to the parent; ignored for the root.
    pub joint: JointSpec,
    /// Whether the body may be missing. A missing required body is an
    /// error ([`RagdollError::MissingBones`]).
    pub optional: bool,
}

/// Everything that decides a ragdoll and its world.
#[derive(Debug, Clone, PartialEq)]
pub struct RagdollSpec {
    /// The bodies; the first is the root and every parent comes before its
    /// children. At most [`MAX_BODIES`] are used.
    pub bodies: Vec<BodySpec>,
    /// Source units per metre ([`DEFAULT_UNITS_PER_METER`]).
    pub units_per_meter: f32,
    /// The figure height in source units; `None` estimates it from the rest
    /// pose (the head bone's height above the lowest bone, over 0.87, or
    /// the vertical extent of all bones when there is no `head` body).
    pub figure_height: Option<f32>,
    /// The figure's up, in the skeleton's root space.
    pub up: [f32; 3],
    /// The way the figure faces, in the skeleton's root space.
    pub forward: [f32; 3],
    /// The ground plane's height along `up`, in source units.
    pub ground_height: f32,
    /// Total mass, kilograms.
    pub total_mass: f32,
    /// Gravity, m/s², along −up.
    pub gravity: f32,
    /// Friction coefficient of every collider (ground included).
    pub friction: f32,
    /// Restitution of every collider (ground included).
    pub restitution: f32,
    /// Linear damping of every body, 1/s.
    pub linear_damping: f32,
    /// Angular damping of every body, 1/s.
    pub angular_damping: f32,
    /// The most torque, N·m, that joint friction applies about each free
    /// axis of each joint; 0 makes joints frictionless.
    pub joint_friction: f32,
    /// Muscle tone while it lasts ([`crate::Impulse::tone`]): the angular
    /// stiffness holding each joint at its start-pose angles, 1/s² (an
    /// acceleration per radian, so independent of mass), at full tone.
    pub tone_stiffness: f32,
    /// The tone's angular damping, 1/s, at full tone.
    pub tone_damping: f32,
    /// The share of the tone's damping (not its stiffness) that stays once
    /// [`crate::Impulse::tone`] has run out, in `[0, 1]`: joint damping that
    /// keeps limbs from whipping and flailing after the body lands, without
    /// pulling them anywhere. 0 goes fully limp.
    pub residual_tone: f32,
    /// Whether bodies collide with each other (except jointed pairs and
    /// pairs that already touch in the start pose).
    pub self_collision: bool,
    /// The fixed simulation timestep, seconds.
    pub timestep: f32,
    /// Solver substeps per timestep (rapier's `num_solver_iterations`).
    pub substeps: u32,
    /// Gauss–Seidel iterations per substep.
    pub pgs_iterations: u32,
    /// A body is at rest below this speed, m/s.
    pub rest_linear_speed: f32,
    /// A body is at rest below this angular speed, rad/s.
    pub rest_angular_speed: f32,
    /// How long every body must stay at rest before the clip ends, seconds.
    pub rest_time: f32,
    /// How long the simulation may run past the clip's duration, heavily
    /// damped, to bring a body that has not settled to rest, seconds.
    pub max_settle_time: f32,
    /// A hard cap on simulation steps, whatever the durations.
    pub max_steps: u32,
}

impl Default for RagdollSpec {
    fn default() -> Self {
        Self::humanoid()
    }
}

fn norm(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// Ball joint ranges for the figure's right side, degrees.
const fn ball(flex: [f32; 2], side: [f32; 2], twist: [f32; 2]) -> JointKind {
    JointKind::Ball { flex, side, twist }
}

impl RagdollSpec {
    /// The default humanoid: pelvis, abdomen, chest, head, upper arms,
    /// forearms (hands included), thighs, calves and feet (14 bodies),
    /// keyed to `Bip01`
    /// bone names with common fallbacks. Pelvis, thighs and calves are
    /// required; the rest may be missing, and a body whose parent is missing
    /// hangs from the nearest ancestor body that exists.
    pub fn humanoid() -> Self {
        let fb = |direction, length| Fallback { direction, length };
        let joint = |kind, flex_toward, mirror| JointSpec {
            kind,
            flex_toward,
            mirror,
        };
        let fixed = joint(JointKind::Fixed, Axis::Forward, false);
        let mut bodies = vec![
            BodySpec {
                key: "pelvis".into(),
                bones: names(&["Bip01 Pelvis", "Bip01 RigPelvis", "Pelvis", "Hips"]),
                parent: None,
                segment: Segment::Between {
                    a: names(&["Bip01 L Thigh", "L Thigh", "LeftUpLeg"]),
                    b: names(&["Bip01 R Thigh", "R Thigh", "RightUpLeg"]),
                    fallback: fb(Axis::Right, 0.1),
                },
                shape: ShapeSpec::Capsule { radius: 0.06 },
                mass: 0.15,
                joint: fixed,
                optional: false,
            },
            BodySpec {
                key: "abdomen".into(),
                bones: names(&["Bip01 Spine", "Bip01 Spine1", "Spine"]),
                parent: Some("pelvis".into()),
                segment: Segment::ToBone {
                    bones: names(&["Bip01 Spine2", "Bip01 Spine1", "Bip01 Neck", "Spine2"]),
                    fallback: fb(Axis::Up, 0.13),
                },
                shape: ShapeSpec::Box {
                    half_width: 0.08,
                    half_depth: 0.055,
                },
                mass: 0.14,
                joint: joint(
                    ball([-20.0, 35.0], [-20.0, 20.0], [-20.0, 20.0]),
                    Axis::Forward,
                    false,
                ),
                optional: true,
            },
            BodySpec {
                key: "chest".into(),
                bones: names(&["Bip01 Spine2", "Bip01 Spine1", "Spine2", "Chest"]),
                parent: Some("abdomen".into()),
                segment: Segment::ToBone {
                    bones: names(&["Bip01 Neck", "Bip01 Neck1", "Bip01 Head", "Neck"]),
                    fallback: fb(Axis::Up, 0.13),
                },
                shape: ShapeSpec::Box {
                    half_width: 0.09,
                    half_depth: 0.06,
                },
                mass: 0.20,
                joint: joint(
                    ball([-15.0, 30.0], [-15.0, 15.0], [-20.0, 20.0]),
                    Axis::Forward,
                    false,
                ),
                optional: true,
            },
            BodySpec {
                key: "head".into(),
                bones: names(&["Bip01 Head", "Head"]),
                parent: Some("chest".into()),
                segment: Segment::Along(fb(Axis::Up, 0.06)),
                shape: ShapeSpec::Capsule { radius: 0.06 },
                mass: 0.08,
                joint: joint(
                    ball([-40.0, 50.0], [-30.0, 30.0], [-50.0, 50.0]),
                    Axis::Forward,
                    false,
                ),
                optional: true,
            },
        ];
        for (side, mirror) in [("R", false), ("L", true)] {
            let n = |part: &str| format!("Bip01 {side} {part}");
            let plain = |part: &str| format!("{side} {part}");
            let key = |part: &str| format!("{part}_{}", side.to_ascii_lowercase());
            bodies.extend([
                BodySpec {
                    key: key("upper_arm"),
                    bones: vec![n("UpperArm"), plain("UpperArm")],
                    parent: Some("chest".into()),
                    segment: Segment::ToBone {
                        bones: vec![n("Forearm"), plain("Forearm")],
                        fallback: fb(Axis::BoneX, 0.17),
                    },
                    shape: ShapeSpec::Capsule { radius: 0.035 },
                    mass: 0.028,
                    joint: joint(
                        ball([-50.0, 130.0], [-90.0, 75.0], [-80.0, 80.0]),
                        Axis::Forward,
                        mirror,
                    ),
                    optional: true,
                },
                BodySpec {
                    key: key("forearm"),
                    bones: vec![n("Forearm"), plain("Forearm")],
                    parent: Some(key("upper_arm")),
                    // The hand rides on the forearm: the capsule runs on
                    // to the knuckles. A separate hand body is light enough
                    // to be jammed through its limits under the torso.
                    segment: Segment::ToBone {
                        bones: vec![n("Finger2"), n("Finger1"), n("Hand"), plain("Hand")],
                        fallback: fb(Axis::BoneX, 0.19),
                    },
                    shape: ShapeSpec::Capsule { radius: 0.03 },
                    mass: 0.022,
                    joint: joint(
                        JointKind::Hinge {
                            flex: [-5.0, 145.0],
                        },
                        Axis::Forward,
                        mirror,
                    ),
                    optional: true,
                },
                BodySpec {
                    key: key("thigh"),
                    bones: vec![n("Thigh"), plain("Thigh")],
                    parent: Some("pelvis".into()),
                    segment: Segment::ToBone {
                        bones: vec![n("Calf"), plain("Calf")],
                        fallback: fb(Axis::Down, 0.24),
                    },
                    shape: ShapeSpec::Capsule { radius: 0.05 },
                    mass: 0.10,
                    joint: joint(
                        ball([-20.0, 110.0], [-45.0, 30.0], [-40.0, 40.0]),
                        Axis::Forward,
                        mirror,
                    ),
                    optional: false,
                },
                BodySpec {
                    key: key("calf"),
                    bones: vec![n("Calf"), plain("Calf")],
                    parent: Some(key("thigh")),
                    segment: Segment::ToBone {
                        bones: vec![n("Foot"), plain("Foot")],
                        fallback: fb(Axis::Down, 0.22),
                    },
                    shape: ShapeSpec::Capsule { radius: 0.04 },
                    mass: 0.0465,
                    joint: joint(
                        JointKind::Hinge {
                            flex: [-2.0, 145.0],
                        },
                        Axis::Backward,
                        mirror,
                    ),
                    optional: false,
                },
                BodySpec {
                    key: key("foot"),
                    bones: vec![n("Foot"), plain("Foot")],
                    parent: Some(key("calf")),
                    segment: Segment::ToBone {
                        bones: vec![n("Toe0"), plain("Toe0"), n("Toe")],
                        fallback: fb(Axis::Forward, 0.1),
                    },
                    shape: ShapeSpec::Box {
                        half_width: 0.025,
                        half_depth: 0.02,
                    },
                    mass: 0.0145,
                    // The ankle as a hinge: a light foot pinned under a
                    // leg slips through narrow ball-joint ranges.
                    joint: joint(
                        JointKind::Hinge {
                            flex: [-45.0, 25.0],
                        },
                        Axis::Up,
                        mirror,
                    ),
                    optional: true,
                },
            ]);
        }
        RagdollSpec {
            bodies,
            units_per_meter: DEFAULT_UNITS_PER_METER,
            figure_height: None,
            up: [0.0, 0.0, 1.0],
            forward: [0.0, 1.0, 0.0],
            ground_height: 0.0,
            total_mass: 80.0,
            gravity: 9.81,
            friction: 0.8,
            restitution: 0.0,
            linear_damping: 0.1,
            angular_damping: 0.8,
            joint_friction: 1.5,
            tone_stiffness: 150.0,
            tone_damping: 15.0,
            residual_tone: 0.2,
            self_collision: true,
            timestep: 1.0 / 60.0,
            substeps: 16,
            pgs_iterations: 1,
            rest_linear_speed: 0.1,
            rest_angular_speed: 0.5,
            rest_time: 0.3,
            max_settle_time: 3.0,
            max_steps: 20_000,
        }
    }

    /// A generic ragdoll built from a skeleton's own bone tree, for
    /// skeletons the humanoid map does not fit (creatures).
    ///
    /// A bone becomes a body when one of its children lies at least 4 % of
    /// the skeleton's extent away (that child ends its segment). Bodies are
    /// taken in bone order, parents first, up to `max_bodies` (capped at
    /// [`MAX_BODIES`]); each hangs from its nearest ancestor body, or from
    /// the first body. Every joint is a ball joint with moderate limits,
    /// every shape a capsule with a radius of 15 % of its segment, masses
    /// follow segment lengths.
    pub fn generic(skeleton: &Skeleton, max_bodies: usize) -> Result<Self, RagdollError> {
        let world = skeleton.world_matrices();
        if world.is_empty() || !skeleton.is_well_formed() {
            return Err(RagdollError::InvalidInput(
                "the skeleton is empty or not ordered parents first".into(),
            ));
        }
        let origins: Vec<[f32; 3]> = world.iter().map(|m| [m[0][3], m[1][3], m[2][3]]).collect();
        if origins.iter().flatten().any(|v| !v.is_finite()) {
            return Err(RagdollError::InvalidInput(
                "the skeleton's rest pose is not finite".into(),
            ));
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for o in &origins {
            for k in 0..3 {
                lo[k] = lo[k].min(o[k]);
                hi[k] = hi[k].max(o[k]);
            }
        }
        let extent = norm([hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]);
        if extent.is_nan() || extent <= 1e-3 {
            return Err(RagdollError::InvalidInput(
                "the skeleton has no extent".into(),
            ));
        }
        let height = (hi[2] - lo[2]).max(extent * 0.25);
        let distance = |a: usize, b: usize| {
            let (p, q) = (origins[a], origins[b]);
            norm([p[0] - q[0], p[1] - q[1], p[2] - q[2]])
        };
        let cap = max_bodies.clamp(1, MAX_BODIES);
        let mut chosen: Vec<(usize, usize)> = Vec::new(); // (bone, end bone)
        for (i, bone) in skeleton.bones.iter().enumerate() {
            if chosen.len() == cap {
                break;
            }
            // A name that does not find this very bone cannot key it.
            if skeleton.find(&bone.name) != Some(i as u32) {
                continue;
            }
            let end = skeleton
                .bones
                .iter()
                .enumerate()
                .skip(i + 1)
                .find(|(c, b)| b.parent == Some(i as u32) && distance(i, *c) >= 0.04 * extent)
                .map(|(c, _)| c);
            if let Some(end) = end
                && skeleton.find(&skeleton.bones[end].name) == Some(end as u32)
            {
                chosen.push((i, end));
            }
        }
        if chosen.len() < 2 {
            return Err(RagdollError::NotRiggable(format!(
                "only {} bone(s) long enough for a body",
                chosen.len()
            )));
        }
        let ancestor_body = |bone: usize| -> Option<usize> {
            let mut p = skeleton.bones[bone].parent;
            while let Some(b) = p {
                if let Some(k) = chosen.iter().position(|&(c, _)| c == b as usize) {
                    return Some(k);
                }
                p = skeleton.bones[b as usize].parent;
            }
            None
        };
        let key = |k: usize| format!("body{k:02}");
        let total: f32 = chosen.iter().map(|&(b, e)| distance(b, e)).sum();
        let bodies = chosen
            .iter()
            .enumerate()
            .map(|(k, &(bone, end))| {
                let length = distance(bone, end);
                BodySpec {
                    key: key(k),
                    bones: vec![skeleton.bones[bone].name.clone()],
                    parent: (k > 0).then(|| key(ancestor_body(bone).unwrap_or(0))),
                    segment: Segment::ToBone {
                        bones: vec![skeleton.bones[end].name.clone()],
                        fallback: Fallback {
                            direction: Axis::BoneX,
                            length: length / height,
                        },
                    },
                    shape: ShapeSpec::Capsule {
                        radius: (0.15 * length / height).clamp(0.01, 0.06),
                    },
                    mass: length / total,
                    joint: JointSpec {
                        kind: ball([-35.0, 35.0], [-25.0, 25.0], [-20.0, 20.0]),
                        flex_toward: Axis::Forward,
                        mirror: false,
                    },
                    optional: false,
                }
            })
            .collect();
        Ok(RagdollSpec {
            bodies,
            figure_height: Some(height),
            ..Self::humanoid()
        })
    }

    /// Checks every number for sense. [`crate::simulate`] calls it.
    pub fn validate(&self) -> Result<(), RagdollError> {
        let bad = |what: &str| Err(RagdollError::InvalidSpec(what.to_owned()));
        let positive = |v: f32| v.is_finite() && v > 0.0;
        let non_negative = |v: f32| v.is_finite() && v >= 0.0;
        if self.bodies.is_empty() {
            return bad("no bodies");
        }
        if self.bodies[0].parent.is_some() {
            return bad("the first body is the root and has no parent");
        }
        for (i, body) in self.bodies.iter().enumerate() {
            if body.bones.is_empty() {
                return bad(&format!("body `{}` names no bone", body.key));
            }
            if self.bodies[..i].iter().any(|b| b.key == body.key) {
                return bad(&format!("body key `{}` is used twice", body.key));
            }
            if i > 0 {
                match &body.parent {
                    None => return bad(&format!("body `{}` has no parent", body.key)),
                    Some(p) if !self.bodies[..i].iter().any(|b| &b.key == p) => {
                        return bad(&format!(
                            "body `{}`: parent `{p}` is not an earlier body",
                            body.key
                        ));
                    }
                    Some(_) => {}
                }
            }
            if !positive(body.mass) {
                return bad(&format!("body `{}`: mass share must be positive", body.key));
            }
            let fallback = match &body.segment {
                Segment::ToBone { fallback, .. }
                | Segment::Between { fallback, .. }
                | Segment::Along(fallback) => fallback,
            };
            if !positive(fallback.length) || fallback.length > 2.0 {
                return bad(&format!(
                    "body `{}`: fallback length out of range",
                    body.key
                ));
            }
            let sizes_ok = match body.shape {
                ShapeSpec::Capsule { radius } | ShapeSpec::Ball { radius } => {
                    positive(radius) && radius <= 1.0
                }
                ShapeSpec::Box {
                    half_width,
                    half_depth,
                } => {
                    positive(half_width)
                        && positive(half_depth)
                        && half_width.max(half_depth) <= 1.0
                }
            };
            if !sizes_ok {
                return bad(&format!("body `{}`: shape size out of range", body.key));
            }
            let range_ok = |r: [f32; 2]| r[0].is_finite() && r[1].is_finite() && r[0] <= r[1];
            let limits_ok = match body.joint.kind {
                JointKind::Ball { flex, side, twist } => [flex, side, twist]
                    .into_iter()
                    .all(|r| range_ok(r) && r[0] >= -179.0 && r[1] <= 179.0),
                JointKind::Hinge { flex } => {
                    range_ok(flex) && flex[0] >= -179.0 && flex[1] <= 179.0
                }
                JointKind::Fixed => true,
            };
            if !limits_ok {
                return bad(&format!(
                    "body `{}`: joint ranges must be ordered and within ±179°",
                    body.key
                ));
            }
        }
        let unit_ok = |v: [f32; 3]| {
            let l = norm(v);
            l.is_finite() && l > 1e-6
        };
        if !unit_ok(self.up) || !unit_ok(self.forward) {
            return bad("up and forward must be non-zero");
        }
        let (u, f) = (self.up, self.forward);
        let cross = [
            u[1] * f[2] - u[2] * f[1],
            u[2] * f[0] - u[0] * f[2],
            u[0] * f[1] - u[1] * f[0],
        ];
        if !unit_ok(cross) {
            return bad("up and forward must not be parallel");
        }
        if !positive(self.units_per_meter) {
            return bad("units_per_meter must be positive");
        }
        if let Some(h) = self.figure_height
            && !positive(h)
        {
            return bad("figure_height must be positive");
        }
        if !self.ground_height.is_finite() {
            return bad("ground_height must be finite");
        }
        if !positive(self.total_mass) || !non_negative(self.gravity) {
            return bad("total_mass must be positive and gravity not negative");
        }
        if !non_negative(self.friction)
            || !non_negative(self.restitution)
            || self.restitution > 1.0
            || !non_negative(self.linear_damping)
            || !non_negative(self.angular_damping)
            || !non_negative(self.joint_friction)
            || !non_negative(self.tone_stiffness)
            || !non_negative(self.tone_damping)
            || !(0.0..=1.0).contains(&self.residual_tone)
        {
            return bad(
                "friction, restitution (≤ 1), damping, joint friction and tone must be ≥ 0, residual tone ≤ 1",
            );
        }
        if !(self.timestep.is_finite() && (1.0 / 2000.0..=0.1).contains(&self.timestep)) {
            return bad("timestep must be within 0.5 ms to 100 ms");
        }
        if !(1..=64).contains(&self.substeps) || !(1..=16).contains(&self.pgs_iterations) {
            return bad("substeps must be 1 to 64 and pgs_iterations 1 to 16");
        }
        if !positive(self.rest_linear_speed)
            || !positive(self.rest_angular_speed)
            || !non_negative(self.rest_time)
            || !non_negative(self.max_settle_time)
        {
            return bad("rest thresholds must be positive and times not negative");
        }
        if self.max_steps == 0 || self.max_steps > 1_000_000 {
            return bad("max_steps must be 1 to 1,000,000");
        }
        Ok(())
    }
}
