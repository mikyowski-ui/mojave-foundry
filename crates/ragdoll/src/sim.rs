//! The simulation: build the rapier world, step it, sample poses.

use nif::math::{Matrix34, mul};
use nif::{Pose, Skeleton};
use rapier3d::math::{Pose as Rigid, Rotation, Vector};
use rapier3d::prelude::GenericJointBuilder;
use rapier3d::prelude::{
    ColliderBuilder, Group, ImpulseJointHandle, InteractionGroups, InteractionTestMode,
    JointAxesMask, JointAxis, MotorModel, PhysicsWorld, RigidBodyBuilder, RigidBodyHandle,
};

use crate::RagdollError;
use crate::geom::{matrix, scaled};
use crate::impulse::{Impulse, Push};
use crate::rig::{Rig, RigShape};
use crate::spec::{JointKind, RagdollSpec};

/// The most speed any body may reach, m/s, before the run counts as
/// exploded.
const MAX_SPEED: f32 = 25.0;
/// The most angular speed, rad/s.
const MAX_ANGULAR_SPEED: f32 = 80.0;
/// How far, in metres, any body may get from where the root started.
const MAX_REACH: f32 = 6.0;
/// How far apart, as a fraction of the figure height, a joint's two
/// anchors may drift.
const MAX_JOINT_GAP: f32 = 0.08;
/// Retries with safer parameters after an explosion.
const ATTEMPTS: u32 = 3;
/// The ground plate's half extents, metres.
const GROUND_HALF: [f32; 3] = [50.0, 50.0, 0.5];

/// The sampled fall.
#[derive(Debug, Clone, PartialEq)]
pub struct RagdollClip {
    /// One pose per sample, `poses[k]` at `k / sample_rate` seconds;
    /// `poses[0]` is the start pose itself. Every bone's matrix is in the
    /// skeleton's root space and source units, like [`nif::pose()`]'s, and
    /// the body moves within that space (it is not held in place);
    /// `travel` is zero. See [`RagdollClip::in_place`].
    pub poses: Vec<Pose>,
    /// Samples a second.
    pub sample_rate: f32,
    /// Seconds from the start until every body came to rest (the start of
    /// the at-rest window), or the clip's end when it was forced.
    pub settle_time: f32,
    /// Whether the last pose is at rest: `false` only when even the forced
    /// settling (see `forced`) ran out of steps.
    pub settled: bool,
    /// The body had not come to rest by the clip's duration; it was brought
    /// to rest under heavy damping past the duration and the last pose
    /// replaced by that rest pose.
    pub forced: bool,
    /// Per pose, the pelvis (root body) displacement from its start along
    /// the figure's `[right, forward]` axes, source units (the root's X and
    /// Y for the default figure frame).
    pub pelvis_travel: Vec<[f32; 2]>,
    /// Where the pelvis ends relative to where it started: the last entry
    /// of `pelvis_travel`. Lying on the back after a fall backward, this
    /// is negative along forward.
    pub root_travel: [f32; 2],
    /// Simulation steps taken (forced settling included).
    pub steps: u32,
    /// 0 for the first try, more when an explosion made it retry with
    /// safer parameters.
    pub attempt: u32,
    /// Rigid bodies simulated.
    pub bodies: usize,
    /// The ground plane's height along up, source units: the spec's
    /// [`RagdollSpec::ground_height`], or lower when the start pose's
    /// shapes dipped below it.
    pub ground: f32,
}

impl RagdollClip {
    /// The last pose: the corpse.
    pub fn corpse(&self) -> &Pose {
        self.poses.last().expect("a clip always has its start pose")
    }

    /// The poses held in place horizontally: each pose shifted so the
    /// pelvis keeps its start position across the ground, with that shift
    /// returned as the pose's `travel` (the [`nif::Pose`] convention for
    /// root motion).
    pub fn in_place(&self, spec: &RagdollSpec) -> Vec<Pose> {
        let Some(figure) = crate::rig::Figure::new(spec) else {
            return self.poses.clone();
        };
        self.poses
            .iter()
            .zip(&self.pelvis_travel)
            .map(|(pose, t)| {
                let shift = figure.right * t[0] + figure.forward * t[1];
                let world = pose
                    .world
                    .iter()
                    .map(|m| {
                        let mut m = *m;
                        for (r, row) in m.iter_mut().enumerate() {
                            row[3] -= shift[r];
                        }
                        m
                    })
                    .collect();
                Pose { world, travel: *t }
            })
            .collect()
    }
}

/// Simulates a death.
///
/// `start` is the pose at the moment of death (one matrix per skeleton
/// bone; the last frame of a hit animation, or the stand pose), `impulse`
/// the kill push, `seed` its perturbation. The clip lasts at most
/// `duration` seconds, sampled `sample_rate` times a second, and ends
/// early, on a sample, once every body has been at rest for
/// [`RagdollSpec::rest_time`].
///
/// The same arguments give a bit-identical clip on every run, thread and
/// process (see the crate documentation for what holds across machines).
pub fn simulate(
    skeleton: &Skeleton,
    start: &Pose,
    spec: &RagdollSpec,
    impulse: &Impulse,
    seed: u64,
    duration: f32,
    sample_rate: f32,
) -> Result<RagdollClip, RagdollError> {
    spec.validate()?;
    impulse.validate()?;
    if !(duration.is_finite() && duration > 0.0 && duration <= 60.0) {
        return Err(RagdollError::InvalidInput(
            "duration must be within (0, 60] seconds".into(),
        ));
    }
    if !(1.0..=240.0).contains(&sample_rate) {
        return Err(RagdollError::InvalidInput(
            "sample_rate must be within [1, 240] a second".into(),
        ));
    }
    if start.world.len() != skeleton.bones.len() {
        return Err(RagdollError::InvalidInput(format!(
            "the start pose has {} matrices for {} bones",
            start.world.len(),
            skeleton.bones.len()
        )));
    }
    if start.world.iter().any(|m| !crate::geom::finite_matrix(m)) {
        return Err(RagdollError::InvalidInput(
            "the start pose is not finite".into(),
        ));
    }
    let rig = Rig::build(skeleton, spec)?;
    let pushes = impulse.perturbed(seed);
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        let params = Params::for_attempt(spec, attempt);
        match run(
            &rig,
            start,
            spec,
            &params,
            &pushes,
            impulse.tone,
            duration,
            sample_rate,
        ) {
            Ok(mut clip) => {
                clip.attempt = attempt;
                return Ok(clip);
            }
            Err(Failure::Exploded(reason)) => last = reason,
            Err(Failure::Error(e)) => return Err(e),
        }
    }
    Err(RagdollError::Exploded {
        attempts: ATTEMPTS,
        reason: last,
    })
}

/// The tunables one attempt runs with.
struct Params {
    substeps: u32,
    linear_damping: f32,
    angular_damping: f32,
    self_collision: bool,
    push_scale: f32,
}

impl Params {
    /// Attempt 0 is the spec as given; each retry doubles the substeps and
    /// damping and softens the push; the last also turns off
    /// self-collision.
    fn for_attempt(spec: &RagdollSpec, attempt: u32) -> Params {
        let k = 1u32 << attempt; // 1, 2, 4
        Params {
            substeps: (spec.substeps * k).min(64),
            linear_damping: spec.linear_damping * k as f32,
            angular_damping: spec.angular_damping * k as f32,
            self_collision: spec.self_collision && attempt + 1 < ATTEMPTS,
            push_scale: 1.0 / (1.0 + 0.5 * attempt as f32),
        }
    }
}

enum Failure {
    Exploded(String),
    Error(RagdollError),
}

impl From<RagdollError> for Failure {
    fn from(e: RagdollError) -> Self {
        Failure::Error(e)
    }
}

/// The world of one attempt.
struct World {
    world: PhysicsWorld,
    bodies: Vec<RigidBodyHandle>,
    joints: Vec<ImpulseJointHandle>,
    /// The muscle-tone joints, one per joint, until the tone is over.
    tone: Vec<ImpulseJointHandle>,
    /// The tone share last set.
    tone_share: Option<f32>,
    /// Body frames at the start pose, source units.
    start_frames: Vec<Rigid>,
    /// The ground plane's height along up, source units.
    ground: f32,
    /// Metres per source unit.
    m: f32,
}

fn build_world(
    rig: &Rig,
    start: &Pose,
    spec: &RagdollSpec,
    params: &Params,
) -> Result<World, RagdollError> {
    let m = 1.0 / spec.units_per_meter;
    let up = rig.figure.up;
    let start_frames = rig.body_frames(&start.world).ok_or_else(|| {
        RagdollError::InvalidInput("a body bone has a degenerate start transform".into())
    })?;

    let mut world = PhysicsWorld::new();
    world.gravity = up * -spec.gravity;
    let ip = &mut world.integration_parameters;
    ip.dt = spec.timestep;
    ip.num_solver_iterations = params.substeps as usize;
    ip.num_internal_pgs_iterations = spec.pgs_iterations as usize;
    // Bodies here are 2 to 30 cm across; rapier's tolerances are relative
    // to `length_unit` metres, and 1 suits that.
    ip.length_unit = 1.0;

    // The ground: a thick fixed plate whose top is the ground plane, or
    // the start pose's lowest shape point when that dips below it (feet
    // sunk a little into the floor would otherwise be shot out of it).
    let ground = ground_for(rig, &start_frames, spec);
    let ground_group = 1u32 << 31;
    let ground_rotation = Rotation::from_rotation_arc(Vector::Z, up);
    let ground_center = up * (ground * m - GROUND_HALF[2]);
    world.colliders.insert(
        ColliderBuilder::cuboid(GROUND_HALF[0], GROUND_HALF[1], GROUND_HALF[2])
            .position(Rigid::from_parts(ground_center, ground_rotation))
            .friction(spec.friction)
            .restitution(spec.restitution)
            .collision_groups(InteractionGroups::new(
                Group::from_bits_retain(ground_group),
                Group::ALL,
                InteractionTestMode::And,
            )),
    );

    // Pairs that already touch in the start pose never collide: pushing
    // them apart would throw the body around.
    let mut excluded = rig.excluded.clone();
    if params.self_collision {
        let shapes: Vec<_> = rig.bodies.iter().map(|b| shared(&b.shape, m)).collect();
        for i in 0..rig.bodies.len() {
            for j in i + 1..rig.bodies.len() {
                let pi = scaled(&start_frames[i], m);
                let pj = scaled(&start_frames[j], m);
                let d = rapier3d::parry::query::distance(
                    &pi,
                    shapes[i].0.as_ref(),
                    &pj,
                    shapes[j].0.as_ref(),
                )
                .map(|d| d.distance)
                .unwrap_or(0.0);
                if d < 0.005 {
                    excluded.push((i, j));
                }
            }
        }
        excluded.sort_unstable();
        excluded.dedup();
    }

    let all_bodies: u32 = (1u32 << rig.bodies.len()) - 1;
    let mut bodies = Vec::with_capacity(rig.bodies.len());
    for (i, body) in rig.bodies.iter().enumerate() {
        let handle = world.bodies.insert(
            RigidBodyBuilder::dynamic()
                .pose(scaled(&start_frames[i], m))
                .linear_damping(params.linear_damping)
                .angular_damping(params.angular_damping)
                .can_sleep(false),
        );
        let filter = if params.self_collision {
            let mut f = all_bodies & !(1u32 << i);
            for &(a, b) in &excluded {
                if a == i {
                    f &= !(1u32 << b);
                } else if b == i {
                    f &= !(1u32 << a);
                }
            }
            f | ground_group
        } else {
            ground_group
        };
        let (shape, position) = shared(&body.shape, m);
        world.colliders.insert_with_parent(
            ColliderBuilder::new(shape)
                .position(position)
                .mass(body.mass)
                .friction(spec.friction)
                .restitution(spec.restitution)
                .collision_groups(InteractionGroups::new(
                    Group::from_bits_retain(1u32 << i),
                    Group::from_bits_retain(filter),
                    InteractionTestMode::And,
                )),
            handle,
            &mut world.bodies,
        );
        bodies.push(handle);
    }

    let mut joints = Vec::with_capacity(rig.joints.len());
    for joint in &rig.joints {
        let mut builder = GenericJointBuilder::new(JointAxesMask::LIN_AXES)
            .local_frame1(scaled(&joint.frame1, m))
            .local_frame2(scaled(&joint.frame2, m))
            .contacts_enabled(false);
        let free: &[(JointAxis, [f32; 2])] = match joint.kind {
            JointKind::Ball { flex, side, twist } => &[
                (JointAxis::AngX, twist),
                (JointAxis::AngY, side),
                (JointAxis::AngZ, flex),
            ],
            JointKind::Hinge { flex } => {
                builder = builder.locked_axes(
                    JointAxesMask::LIN_AXES | JointAxesMask::ANG_X | JointAxesMask::ANG_Y,
                );
                &[(JointAxis::AngZ, flex)]
            }
            JointKind::Fixed => {
                builder = builder.locked_axes(JointAxesMask::LOCKED_FIXED_AXES);
                &[]
            }
        };
        for &(axis, limits) in free {
            builder = builder.limits(axis, limits);
            if spec.joint_friction > 0.0 {
                // A velocity motor driving the axis to rest, capped: dry
                // friction in the joint.
                builder = builder
                    .motor_model(axis, MotorModel::ForceBased)
                    .motor_velocity(axis, 0.0, 1.0e3)
                    .motor_max_force(axis, spec.joint_friction);
            }
        }
        joints.push(world.impulse_joints.insert(
            bodies[joint.parent],
            bodies[joint.child],
            builder.build(),
            true,
        ));
    }

    // Muscle tone: a second joint per pair, framed so that the start pose
    // is its zero, with position motors on all three angles. Its stiffness
    // fades in `World::fade_tone`.
    let mut tone = Vec::new();
    if spec.tone_stiffness > 0.0 || spec.tone_damping > 0.0 {
        for joint in &rig.joints {
            let relative = start_frames[joint.parent].inverse() * start_frames[joint.child];
            let mut builder = GenericJointBuilder::new(JointAxesMask::empty())
                .local_frame1(scaled(&relative, m))
                .local_frame2(Rigid::IDENTITY)
                .contacts_enabled(false);
            for axis in [JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ] {
                builder = builder
                    .motor_model(axis, MotorModel::AccelerationBased)
                    .motor_position(axis, 0.0, spec.tone_stiffness, spec.tone_damping);
            }
            tone.push(world.impulse_joints.insert(
                bodies[joint.parent],
                bodies[joint.child],
                builder.build(),
                true,
            ));
        }
    }

    Ok(World {
        world,
        bodies,
        joints,
        tone,
        tone_share: None,
        start_frames,
        ground,
        m,
    })
}

/// The ground plane's height along up, source units: the spec's, lowered
/// to the lowest collision shape point of the start pose if need be.
pub(crate) fn ground_for(rig: &Rig, start_frames: &[Rigid], spec: &RagdollSpec) -> f32 {
    rig.bodies
        .iter()
        .zip(start_frames)
        .map(|(b, f)| b.shape.lowest(f, rig.figure.up))
        .fold(spec.ground_height, f32::min)
}

/// A rapier shape and its pose in the body, metres.
fn shared(shape: &RigShape, m: f32) -> (rapier3d::prelude::SharedShape, Rigid) {
    use rapier3d::prelude::SharedShape;
    match *shape {
        RigShape::Capsule { a, b, radius } => (
            SharedShape::capsule(a * m, b * m, radius * m),
            Rigid::IDENTITY,
        ),
        RigShape::Ball { center, radius } => (
            SharedShape::ball(radius * m),
            Rigid::from_translation(center * m),
        ),
        RigShape::Cuboid { center, half } => (
            SharedShape::cuboid(half.x * m, half.y * m, half.z * m),
            scaled(&center, m),
        ),
    }
}

impl World {
    /// Every body's frame now, source units.
    fn frames(&self) -> Vec<Rigid> {
        self.bodies
            .iter()
            .map(|h| scaled(self.world.bodies[*h].position(), 1.0 / self.m))
            .collect()
    }

    /// The skeleton posed from the bodies: every bone moves rigidly with
    /// its owner body, `world_now = frame_now · frame_start⁻¹ · world_start`.
    fn pose(&self, rig: &Rig, start: &Pose) -> Pose {
        let deltas: Vec<Matrix34> = self
            .frames()
            .iter()
            .zip(&self.start_frames)
            .map(|(now, then)| matrix(&(*now * then.inverse())))
            .collect();
        Pose {
            world: start
                .world
                .iter()
                .zip(&rig.owner)
                .map(|(m, &o)| o.map_or(*m, |o| mul(&deltas[o], m)))
                .collect(),
            travel: [0.0; 2],
        }
    }

    /// Why the state is unusable, if it is.
    fn exploded(&self, rig: &Rig, root_start: Vector) -> Option<String> {
        for (i, h) in self.bodies.iter().enumerate() {
            let b = &self.world.bodies[*h];
            let p = b.position();
            let (v, w) = (b.linvel(), b.angvel());
            if !(p.translation.is_finite()
                && p.rotation.is_finite()
                && v.is_finite()
                && w.is_finite())
            {
                return Some(format!("body `{}` is not finite", rig.bodies[i].key));
            }
            if v.length() > MAX_SPEED || w.length() > MAX_ANGULAR_SPEED {
                return Some(format!("body `{}` is too fast", rig.bodies[i].key));
            }
            if (p.translation - root_start).length() > MAX_REACH {
                return Some(format!("body `{}` flew away", rig.bodies[i].key));
            }
        }
        let max_gap = MAX_JOINT_GAP * rig.height * self.m;
        for (joint, handle) in rig.joints.iter().zip(&self.joints) {
            let data = &self.world.impulse_joints.get(*handle)?.data;
            let p1 = *self.world.bodies[self.bodies[joint.parent]].position() * data.local_frame1;
            let p2 = *self.world.bodies[self.bodies[joint.child]].position() * data.local_frame2;
            if (p1.translation - p2.translation).length() > max_gap {
                return Some(format!(
                    "joint `{}` came apart",
                    rig.bodies[joint.child].key
                ));
            }
        }
        None
    }

    /// Sets the muscle tone to `share` of full (1 at the start, 0 once
    /// [`Impulse::tone`] has run out): the stiffness fades to nothing, the
    /// damping to [`RagdollSpec::residual_tone`] of full. With neither left
    /// the tone joints are removed for good.
    fn fade_tone(&mut self, spec: &RagdollSpec, share: f32) {
        // Once the tone has run out the motors stay as they are: touching a
        // joint makes rapier rebuild its solver layout.
        if self.tone_share == Some(share) {
            return;
        }
        self.tone_share = Some(share);
        let stiffness = spec.tone_stiffness * share;
        let damping = spec.tone_damping * share.max(spec.residual_tone);
        if stiffness <= 0.0 && damping <= 0.0 {
            for handle in self.tone.drain(..) {
                self.world.impulse_joints.remove(handle, true);
            }
            return;
        }
        for handle in &self.tone {
            if let Some(joint) = self.world.impulse_joints.get_mut(*handle, true) {
                for axis in [JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ] {
                    joint.data.set_motor_position(axis, 0.0, stiffness, damping);
                }
            }
        }
    }

    /// Every body's pose now, metres.
    fn positions(&self) -> Vec<Rigid> {
        self.bodies
            .iter()
            .map(|h| *self.world.bodies[*h].position())
            .collect()
    }

    /// Whether every body moved slower than the rest speeds on average
    /// since `then` (`elapsed` seconds ago). Averages over a window, unlike
    /// instantaneous velocities, ignore the solver's tiny contact jitter
    /// while still catching a slow creep.
    fn calm_since(&self, then: &[Rigid], elapsed: f32, spec: &RagdollSpec) -> bool {
        self.positions().iter().zip(then).all(|(now, before)| {
            let moved = (now.translation - before.translation).length();
            // Twice the vector part of the relative rotation: the angle,
            // to first order, with no transcendental function.
            let q = before.rotation.inverse() * now.rotation;
            let turned = 2.0 * Vector::new(q.x, q.y, q.z).length();
            moved < spec.rest_linear_speed * elapsed && turned < spec.rest_angular_speed * elapsed
        })
    }

    /// Applies a share of a push: `velocity` is the share of the push's
    /// whole-figure velocity change (m/s), `mass` the figure's mass.
    fn apply(&mut self, rig: &Rig, push: &Push, velocity: Vector, mass: f32) {
        if push.bodies.iter().any(|k| k == Push::EVERY_BODY) {
            // Topple: every body gets a velocity change growing with its
            // height above the ground (twice the push's at mid-height), the
            // start of a rotation about the feet as one piece.
            let half = 0.5 * rig.height * self.m;
            for h in &self.bodies {
                let body = &mut self.world.bodies[*h];
                let height =
                    (body.center_of_mass().dot(rig.figure.up) - self.ground * self.m).max(0.0);
                let impulse = velocity * (body.mass() * height / half);
                body.apply_impulse(impulse, true);
            }
            return;
        }
        // The first candidate present, in the push's own order.
        let k = push
            .bodies
            .iter()
            .find_map(|key| rig.bodies.iter().position(|b| &b.key == key))
            .unwrap_or(0);
        let (a, b) = rig.bodies[k].segment;
        let along = push.along.clamp(0.0, 1.0);
        let local = (a + (b - a) * along) * self.m;
        let body = &mut self.world.bodies[self.bodies[k]];
        let point = body.position().transform_point(local);
        body.apply_impulse_at_point(velocity * mass, point, true);
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    rig: &Rig,
    start: &Pose,
    spec: &RagdollSpec,
    params: &Params,
    pushes: &[Push],
    tone: f32,
    duration: f32,
    sample_rate: f32,
) -> Result<RagdollClip, Failure> {
    let mut w = build_world(rig, start, spec, params)?;
    let dt = spec.timestep;
    let root_start = w.world.bodies[w.bodies[0]].translation();
    let sampled_steps = (duration / dt).ceil() as u64;
    let settle_steps = (spec.max_settle_time / dt).ceil() as u64;
    let cap = (sampled_steps + settle_steps).min(u64::from(spec.max_steps));

    let travel_of = |w: &World| -> [f32; 2] {
        let now = w.world.bodies[w.bodies[0]].translation();
        let d = (now - root_start) / w.m;
        [d.dot(rig.figure.right), d.dot(rig.figure.forward)]
    };

    let mut poses = vec![start.clone()];
    let mut pelvis_travel = vec![[0.0f32; 2]];
    let mut next_sample: u64 = 1;
    let mut rest_since: Option<f32> = None;
    let window_steps = ((0.1 / dt).round() as u64).max(1);
    let mut window: (u64, Vec<Rigid>) = (0, w.positions());
    let mut settled = false;
    let mut forced = false;
    let mut steps: u64 = 0;

    while steps < cap {
        let t0 = steps as f32 * dt;
        let t1 = (steps + 1) as f32 * dt;
        for push in pushes {
            let end = push.start + push.duration;
            // The share of the push that falls in [t0, t1).
            let share = if push.duration <= 0.0 {
                if push.start >= t0 && push.start < t1 {
                    1.0
                } else {
                    0.0
                }
            } else {
                let lo = t0.max(push.start);
                let hi = t1.min(end);
                if hi > lo {
                    (hi - lo) / push.duration
                } else {
                    0.0
                }
            };
            if share > 0.0 {
                let v =
                    rig.figure.to_root(push.direction) * (push.speed * params.push_scale * share);
                w.apply(rig, push, v, spec.total_mass);
            }
        }
        if !w.tone.is_empty() {
            let fading = if tone > 0.0 {
                (1.0 - t0 / tone).max(0.0)
            } else {
                0.0
            };
            w.fade_tone(spec, fading);
        }
        w.world.step();
        steps += 1;
        if let Some(reason) = w.exploded(rig, root_start) {
            return Err(Failure::Exploded(reason));
        }

        let t = steps as f32 * dt;
        // Rest is judged over windows of about 0.1 s.
        if steps.is_multiple_of(window_steps) {
            let calm = w.calm_since(&window.1, (steps - window.0) as f32 * dt, spec);
            if calm {
                rest_since.get_or_insert(window.0 as f32 * dt);
            } else {
                rest_since = None;
            }
            window = (steps, w.positions());
        }
        let past_duration = steps >= sampled_steps;
        // Samples whose time this step reached, up to the duration.
        let eps = dt * 1e-3;
        loop {
            let time = next_sample as f32 / sample_rate;
            if time > t + eps || time > duration + eps {
                break;
            }
            poses.push(w.pose(rig, start));
            pelvis_travel.push(travel_of(&w));
            next_sample += 1;
        }
        let rested = rest_since.is_some_and(|s| t - s >= spec.rest_time - eps);
        let last_sample_time = (next_sample - 1) as f32 / sample_rate;
        if rested && !past_duration && rest_since.is_some_and(|s| last_sample_time >= s) {
            settled = true;
            break;
        }
        if past_duration {
            if rested {
                settled = true;
                break;
            }
            if !forced {
                forced = true;
                // Past the duration: damp hard so the body comes to rest
                // where it is.
                for h in &w.bodies {
                    let b = &mut w.world.bodies[*h];
                    b.set_linear_damping(20.0);
                    b.set_angular_damping(20.0);
                }
            }
        }
    }
    if forced {
        // The last sample becomes the rest pose.
        let last = poses.len() - 1;
        if last > 0 {
            poses[last] = w.pose(rig, start);
            pelvis_travel[last] = travel_of(&w);
        } else {
            poses.push(w.pose(rig, start));
            pelvis_travel.push(travel_of(&w));
        }
    }
    let settle_time = match (forced, rest_since) {
        (false, Some(s)) => s,
        _ => (poses.len() - 1) as f32 / sample_rate,
    };
    if poses
        .iter()
        .any(|p| p.world.iter().any(|m| !crate::geom::finite_matrix(m)))
    {
        return Err(Failure::Exploded("a sampled pose is not finite".into()));
    }
    let root_travel = *pelvis_travel.last().expect("at least the start");
    Ok(RagdollClip {
        poses,
        sample_rate,
        settle_time,
        settled,
        forced,
        pelvis_travel,
        root_travel,
        steps: steps as u32,
        attempt: 0,
        bodies: rig.bodies.len(),
        ground: w.ground,
    })
}
