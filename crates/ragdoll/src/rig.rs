//! A [`RagdollSpec`] resolved against one skeleton: bodies, shapes and
//! joint frames in the skeleton's units, fixed in its rest pose.

use nif::Skeleton;
use nif::math::Matrix34;
use rapier3d::math::{Pose, Rotation, Vector};

use crate::RagdollError;
use crate::geom::{column, frame, origin, perpendicular, rigid, unit};
use crate::spec::{Axis, BodySpec, JointKind, MAX_BODIES, RagdollSpec, Segment, ShapeSpec};

/// The figure's own axes, unit vectors in root space.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Figure {
    pub right: Vector,
    pub forward: Vector,
    pub up: Vector,
}

impl Figure {
    pub fn new(spec: &RagdollSpec) -> Option<Figure> {
        let up = unit(Vector::from_array(spec.up))?;
        let forward = perpendicular(Vector::from_array(spec.forward), up)?;
        Some(Figure {
            right: forward.cross(up),
            forward,
            up,
        })
    }

    /// A figure-frame vector `[right, forward, up]` in root space.
    pub fn to_root(self, v: [f32; 3]) -> Vector {
        self.right * v[0] + self.forward * v[1] + self.up * v[2]
    }

    fn axis(&self, axis: Axis, bone: &Matrix34) -> Vector {
        match axis {
            Axis::BoneX => column(bone, 0),
            Axis::BoneY => column(bone, 1),
            Axis::BoneZ => column(bone, 2),
            Axis::Up => self.up,
            Axis::Down => -self.up,
            Axis::Forward => self.forward,
            Axis::Backward => -self.forward,
            Axis::Right => self.right,
            Axis::Left => -self.right,
        }
    }
}

/// A collision shape in its body's frame, source units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RigShape {
    Capsule { a: Vector, b: Vector, radius: f32 },
    Cuboid { center: Pose, half: Vector },
    Ball { center: Vector, radius: f32 },
}

impl RigShape {
    /// The lowest point of the shape along `up` when its body sits at
    /// `body` (source units).
    pub fn lowest(&self, body: &Pose, up: Vector) -> f32 {
        match *self {
            RigShape::Capsule { a, b, radius } => {
                let ha = body.transform_point(a).dot(up);
                let hb = body.transform_point(b).dot(up);
                ha.min(hb) - radius
            }
            RigShape::Ball { center, radius } => body.transform_point(center).dot(up) - radius,
            RigShape::Cuboid { center, half } => {
                let p = *body * center;
                let local_up = p.rotation.inverse() * up;
                p.translation.dot(up) - local_up.abs().dot(half)
            }
        }
    }
}

/// One rigid body.
#[derive(Debug, Clone)]
pub(crate) struct RigBody {
    pub key: String,
    /// The skeleton bone it moves.
    pub bone: usize,
    pub shape: RigShape,
    /// Kilograms.
    pub mass: f32,
    /// The segment in the body's frame, source units (for push points).
    pub segment: (Vector, Vector),
}

/// A joint between two bodies, frames in each body's own frame.
#[derive(Debug, Clone)]
pub(crate) struct RigJoint {
    pub parent: usize,
    pub child: usize,
    pub frame1: Pose,
    pub frame2: Pose,
    /// Limits in radians, mirrored for the side the joint is on.
    pub kind: JointKind,
}

/// The resolved ragdoll.
#[derive(Debug, Clone)]
pub(crate) struct Rig {
    pub bodies: Vec<RigBody>,
    pub joints: Vec<RigJoint>,
    /// For every skeleton bone, the body it follows rigidly; `None` for a
    /// skeleton root that is not a body (it stays put).
    pub owner: Vec<Option<usize>>,
    /// The figure height, source units.
    pub height: f32,
    pub figure: Figure,
    /// Pairs of bodies (lower index first) that may never collide.
    pub excluded: Vec<(usize, usize)>,
}

fn to_radians(kind: JointKind, mirror: bool) -> JointKind {
    let r = |v: [f32; 2]| [v[0].to_radians(), v[1].to_radians()];
    let m = |v: [f32; 2]| if mirror { [-v[1], -v[0]] } else { v };
    match kind {
        JointKind::Ball { flex, side, twist } => JointKind::Ball {
            flex: r(flex),
            side: r(m(side)),
            twist: r(m(twist)),
        },
        JointKind::Hinge { flex } => JointKind::Hinge { flex: r(flex) },
        JointKind::Fixed => JointKind::Fixed,
    }
}

impl Rig {
    /// Resolves `spec` against `skeleton` in its rest pose.
    pub fn build(skeleton: &Skeleton, spec: &RagdollSpec) -> Result<Rig, RagdollError> {
        if skeleton.bones.is_empty() || !skeleton.is_well_formed() {
            return Err(RagdollError::InvalidInput(
                "the skeleton is empty or not ordered parents first".into(),
            ));
        }
        let figure = Figure::new(spec)
            .ok_or_else(|| RagdollError::InvalidSpec("up and forward are degenerate".into()))?;
        let rest = skeleton.world_matrices();
        if rest.iter().any(|m| !crate::geom::finite_matrix(m)) {
            return Err(RagdollError::InvalidInput(
                "the skeleton's rest pose is not finite".into(),
            ));
        }
        let find = |names: &[String]| -> Option<usize> {
            names
                .iter()
                .find_map(|n| skeleton.find(n))
                .map(|i| i as usize)
        };

        // Which spec bodies exist, and their bones.
        let mut bone_of: Vec<Option<usize>> = Vec::with_capacity(spec.bodies.len());
        let mut missing = Vec::new();
        for body in &spec.bodies {
            let bone = find(&body.bones)
                // One bone moves one body; a later body naming a taken bone
                // is treated as missing.
                .filter(|b| !bone_of.contains(&Some(*b)));
            if bone.is_none() && !body.optional {
                missing.push(body.key.clone());
            }
            bone_of.push(bone);
        }
        if !missing.is_empty() {
            return Err(RagdollError::MissingBones { bodies: missing });
        }
        if bone_of[0].is_none() {
            return Err(RagdollError::MissingBones {
                bodies: vec![spec.bodies[0].key.clone()],
            });
        }

        // The figure height.
        let heights: Vec<f32> = rest.iter().map(|m| origin(m).dot(figure.up)).collect();
        let lowest = heights.iter().copied().fold(f32::INFINITY, f32::min);
        let highest = heights.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let head = spec
            .bodies
            .iter()
            .zip(&bone_of)
            .find(|(b, _)| b.key == "head")
            .and_then(|(_, bone)| *bone);
        let height = match (spec.figure_height, head) {
            (Some(h), _) => h,
            (None, Some(head)) if heights[head] - lowest > 1e-3 => (heights[head] - lowest) / 0.87,
            _ => highest - lowest,
        };
        if !(height.is_finite() && height > 1e-3) {
            return Err(RagdollError::NotRiggable(
                "the figure has no height in its rest pose".into(),
            ));
        }

        // Bodies in spec order, parents resolved to the nearest existing
        // ancestor body.
        let mut index_of: Vec<Option<usize>> = vec![None; spec.bodies.len()];
        let mut bodies: Vec<RigBody> = Vec::new();
        let mut joints: Vec<RigJoint> = Vec::new();
        let mut frames: Vec<Pose> = Vec::new(); // rest body frames, root space
        let mut total_share = 0.0f32;
        for (s, body) in spec.bodies.iter().enumerate() {
            let Some(bone) = bone_of[s] else { continue };
            if bodies.len() == MAX_BODIES {
                break;
            }
            let parent = if s == 0 {
                None
            } else {
                let mut key = body.parent.as_deref();
                let mut found = None;
                while let Some(k) = key {
                    let Some(p) = spec.bodies.iter().position(|b| b.key == k) else {
                        break;
                    };
                    if let Some(i) = index_of[p] {
                        found = Some(i);
                        break;
                    }
                    key = spec.bodies[p].parent.as_deref();
                }
                Some(found.unwrap_or(0))
            };
            let m = &rest[bone];
            let body_frame = rigid(m).ok_or_else(|| {
                RagdollError::NotRiggable(format!(
                    "bone `{}` has a degenerate rest transform",
                    skeleton.bones[bone].name
                ))
            })?;
            let (a, b) = segment(body, bone, &rest, &figure, height, &find);
            let inv = body_frame.inverse();
            let shape = shape(body.shape, a, b, height, &figure, &inv);
            let index = bodies.len();
            if let Some(parent) = parent {
                let dir = unit(b - a).unwrap_or(figure.up);
                let flex = figure.axis(body.joint.flex_toward, m);
                let rotation = frame(dir, flex)
                    .or_else(|| frame(dir, figure.forward))
                    .or_else(|| frame(dir, figure.up))
                    .unwrap_or(Rotation::IDENTITY);
                let joint_world = Pose::from_parts(origin(m), rotation);
                joints.push(RigJoint {
                    parent,
                    child: index,
                    frame1: frames[parent].inverse() * joint_world,
                    frame2: inv * joint_world,
                    kind: to_radians(body.joint.kind, body.joint.mirror),
                });
            }
            total_share += body.mass;
            bodies.push(RigBody {
                key: body.key.clone(),
                bone,
                shape,
                mass: body.mass,
                segment: (inv.transform_point(a), inv.transform_point(b)),
            });
            frames.push(body_frame);
            index_of[s] = Some(index);
        }
        if bodies.len() < 2 {
            return Err(RagdollError::NotRiggable(
                "fewer than two bodies resolved".into(),
            ));
        }
        for body in &mut bodies {
            body.mass = spec.total_mass * body.mass / total_share;
        }

        // Every bone follows its own body, else its nearest body
        // ancestor's, else the root body's; a skeleton root that is not a
        // body stays where it is (the figure's placement on the ground).
        let mut near: Vec<Option<usize>> = vec![None; skeleton.bones.len()];
        let mut owner: Vec<Option<usize>> = vec![None; skeleton.bones.len()];
        for (i, bone) in skeleton.bones.iter().enumerate() {
            near[i] = bodies
                .iter()
                .position(|b| b.bone == i)
                .or_else(|| bone.parent.and_then(|p| near[p as usize]));
            owner[i] = match (near[i], bone.parent) {
                (Some(k), _) => Some(k),
                (None, Some(_)) => Some(0),
                (None, None) => None,
            };
        }

        // Jointed pairs never collide.
        let mut excluded: Vec<(usize, usize)> = joints
            .iter()
            .map(|j| (j.parent.min(j.child), j.parent.max(j.child)))
            .collect();
        excluded.sort_unstable();
        excluded.dedup();
        Ok(Rig {
            bodies,
            joints,
            owner,
            height,
            figure,
            excluded,
        })
    }

    /// Each body's frame in a pose (source units), from its bone's matrix.
    pub fn body_frames(&self, world: &[Matrix34]) -> Option<Vec<Pose>> {
        self.bodies.iter().map(|b| rigid(&world[b.bone])).collect()
    }
}

/// A body's segment `(a, b)` in root space, rest pose.
fn segment(
    body: &BodySpec,
    bone: usize,
    rest: &[Matrix34],
    figure: &Figure,
    height: f32,
    find: &dyn Fn(&[String]) -> Option<usize>,
) -> (Vector, Vector) {
    let m = &rest[bone];
    let o = origin(m);
    let min_len = 0.02 * height;
    let along = |fallback: &crate::spec::Fallback| {
        let dir = unit(figure.axis(fallback.direction, m)).unwrap_or(figure.up);
        (o, o + dir * (fallback.length * height))
    };
    match &body.segment {
        Segment::ToBone { bones, fallback } => match find(bones) {
            Some(end) if (origin(&rest[end]) - o).length() >= min_len => (o, origin(&rest[end])),
            _ => along(fallback),
        },
        Segment::Between { a, b, fallback } => match (find(a), find(b)) {
            (Some(a), Some(b)) if (origin(&rest[a]) - origin(&rest[b])).length() >= min_len => {
                (origin(&rest[a]), origin(&rest[b]))
            }
            _ => along(fallback),
        },
        Segment::Along(fallback) => along(fallback),
    }
}

/// A body's shape in its own frame (`inv` maps root space to it).
fn shape(
    spec: ShapeSpec,
    a: Vector,
    b: Vector,
    height: f32,
    figure: &Figure,
    inv: &Pose,
) -> RigShape {
    match spec {
        ShapeSpec::Capsule { radius } => RigShape::Capsule {
            a: inv.transform_point(a),
            b: inv.transform_point(b),
            radius: radius * height,
        },
        ShapeSpec::Ball { radius } => RigShape::Ball {
            center: inv.transform_point((a + b) * 0.5),
            radius: radius * height,
        },
        ShapeSpec::Box {
            half_width,
            half_depth,
        } => {
            let dir = unit(b - a).unwrap_or(figure.up);
            let rotation = frame(dir, figure.right)
                .or_else(|| frame(dir, figure.forward))
                .unwrap_or(Rotation::IDENTITY);
            RigShape::Cuboid {
                center: *inv * Pose::from_parts((a + b) * 0.5, rotation),
                half: Vector::new(
                    ((b - a).length() * 0.5).max(0.01 * height),
                    half_width * height,
                    half_depth * height,
                ),
            }
        }
    }
}
