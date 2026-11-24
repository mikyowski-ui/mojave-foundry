//! Determinism, quality and robustness tests, on synthetic skeletons only.

use nif::math::Matrix34;
use rapier3d::math::{Pose as Rigid, Vector};

use super::*;
use crate::geom::origin;
use crate::rig::Rig;

const DURATION: f32 = 5.0;
const FPS: f32 = 10.0;

fn fall(preset: Preset, seed: u64) -> (nif::Skeleton, RagdollClip) {
    fall_with(preset, seed, &RagdollSpec::humanoid())
}

fn fall_with(preset: Preset, seed: u64, spec: &RagdollSpec) -> (nif::Skeleton, RagdollClip) {
    let skeleton = fixture::humanoid();
    let start = fixture::stand(&skeleton);
    let mut impulse = Impulse::preset(preset);
    // Survey knobs (unset in normal runs).
    if let Some(k) = std::env::var("RD_SPEED_SCALE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        for p in &mut impulse.pushes {
            p.speed *= k;
        }
    }
    if let Some(t) = std::env::var("RD_TONE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        impulse.tone = t;
    }
    let clip = simulate(&skeleton, &start, spec, &impulse, seed, DURATION, FPS)
        .unwrap_or_else(|e| panic!("{preset:?} seed {seed}: {e}"));
    (skeleton, clip)
}

/// Every float of a clip as raw bits, plus its counters.
fn bits(clip: &RagdollClip) -> Vec<u32> {
    let mut out = vec![
        clip.poses.len() as u32,
        clip.sample_rate.to_bits(),
        clip.settle_time.to_bits(),
        u32::from(clip.settled),
        u32::from(clip.forced),
        clip.steps,
        clip.attempt,
        clip.bodies as u32,
        clip.ground.to_bits(),
    ];
    for pose in &clip.poses {
        out.extend(pose.world.iter().flatten().flatten().map(|v| v.to_bits()));
        out.extend(pose.travel.iter().map(|v| v.to_bits()));
    }
    out.extend(clip.pelvis_travel.iter().flatten().map(|v| v.to_bits()));
    out.extend(clip.root_travel.iter().map(|v| v.to_bits()));
    out
}

fn fnv(words: &[u32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for w in words {
        for b in w.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

// ------------------------------------------------------------ determinism

#[test]
fn repeated_runs_are_bit_identical() {
    for preset in Preset::ALL {
        let (_, a) = fall(preset, 7);
        let (_, b) = fall(preset, 7);
        assert_eq!(bits(&a), bits(&b), "{preset:?}");
    }
}

#[test]
fn runs_on_other_threads_are_bit_identical() {
    let here = bits(&fall(Preset::FallBackward, 11).1);
    let there: Vec<Vec<u32>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..2)
            .map(|_| s.spawn(|| bits(&fall(Preset::FallBackward, 11).1)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for t in there {
        assert_eq!(t, here);
    }
}

#[test]
fn different_seeds_give_different_falls() {
    let a = fall(Preset::FallBackward, 1).1;
    let b = fall(Preset::FallBackward, 2).1;
    assert_ne!(bits(&a), bits(&b));
    // Not just different in the last bits: the corpses differ visibly.
    let moved = a
        .corpse()
        .world
        .iter()
        .zip(&b.corpse().world)
        .map(|(p, q)| (origin(p) - origin(q)).length())
        .fold(0.0f32, f32::max);
    assert!(moved > 2.0, "corpses only {moved} units apart");
}

/// A golden digest of one clip. It pins the result across processes,
/// builds (dev and release) and, when run elsewhere, machines. It changes
/// when this crate's parameters or the rapier / parry / glam versions
/// change: then update it on purpose, after looking at the falls with the
/// preview example.
#[test]
fn golden_digest() {
    let clip = fall(Preset::FallBackward, 1).1;
    let digest = fnv(&bits(&clip));
    eprintln!("golden digest {digest:#018x}");
    assert_eq!(
        digest, GOLDEN,
        "the simulation changed; see the test's documentation"
    );
}

const GOLDEN: u64 = 0xc447_9a96_f4c3_53eb;

// ---------------------------------------------------------------- quality

/// Each body frame of a pose.
fn frames(rig: &Rig, pose: &nif::Pose) -> Vec<Rigid> {
    rig.body_frames(&pose.world).expect("posed bones are rigid")
}

/// The joint angles `[twist, side, flex]` (radians) of a rig joint in a
/// pose, measured like rapier does: from the relative rotation of the two
/// joint frames, `2·atan2(q_axis, q_w)`.
fn joint_angles(joint: &crate::rig::RigJoint, f: &[Rigid]) -> [f32; 3] {
    let a = f[joint.parent] * joint.frame1;
    let b = f[joint.child] * joint.frame2;
    let mut q = a.rotation.inverse() * b.rotation;
    if q.w < 0.0 {
        q = -q;
    }
    [q.x, q.y, q.z].map(|v| 2.0 * v.atan2(q.w))
}

fn check_quality(preset: Preset, seed: u64) {
    let (skeleton, clip) = fall(preset, seed);
    let spec = RagdollSpec::humanoid();
    let rig = Rig::build(&skeleton, &spec).unwrap();
    let up = rig.figure.up;
    let what = format!("{preset:?} seed {seed}");

    // Settles within the duration, by itself.
    assert!(
        clip.settled && !clip.forced,
        "{what}: did not settle ({:.2} s)",
        clip.settle_time
    );
    assert!(clip.settle_time < DURATION, "{what}");

    // Lying down at the end: no bone higher than 30 % of the figure's
    // height (a bent knee or a hip propped on folded legs stays below),
    // the torso on average below 20 % (a standing figure has it near 70 %).
    let corpse = clip.corpse();
    let highest = corpse
        .world
        .iter()
        .map(|m| origin(m).dot(up))
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(
        highest < 0.3 * rig.height,
        "{what}: a bone stays {highest:.1} high"
    );
    let torso: Vec<f32> = ["pelvis", "abdomen", "chest", "head"]
        .iter()
        .filter_map(|k| rig.bodies.iter().find(|b| b.key == *k))
        .map(|b| origin(&corpse.world[b.bone]).dot(up))
        .collect();
    let torso = torso.iter().sum::<f32>() / torso.len() as f32;
    assert!(torso < 0.2 * rig.height, "{what}: torso at {torso:.1}");

    let start = &clip.poses[0];
    let all = measure(&skeleton, &rig, start, &clip.poses, clip.ground);
    let last = measure(
        &skeleton,
        &rig,
        start,
        std::slice::from_ref(corpse),
        clip.ground,
    );
    eprintln!(
        "{what}: {} frames, settled {:.2} s, top {highest:.1} u, torso {torso:.1} u, travel [{:.0}, {:.0}] | worst over the fall: {all} | corpse: {last}",
        clip.poses.len(),
        clip.settle_time,
        clip.root_travel[0],
        clip.root_travel[1],
    );
    // No shape sinks into the ground by more than 2 % of the height
    // (rapier's contact slop and speculative margin are a few mm).
    assert!(all.ground < 0.02 * rig.height, "{what}: sinks {all}");
    // Over the whole fall: bones stretch by under 2.5 % of the height
    // (joints drift a little on hard landings), hinges (knees, elbows,
    // ankles) stay within 10° of their ranges, ball joints within 40°.
    // Ball joints are limited per axis, which rapier enforces only
    // approximately once two axes are bent far at once (a hip flexed and
    // adducted under a body lying on it, an arm flung over the head); the
    // excess is transient, on impact.
    assert!(
        all.stretch < 0.025 * rig.height,
        "{what}: stretch over the fall: {all}"
    );
    assert!(
        all.hinge < 10f32.to_radians(),
        "{what}: hinge over the fall: {all}"
    );
    assert!(
        all.ball < 40f32.to_radians(),
        "{what}: ball over the fall: {all}"
    );
    // The corpse, the still that stays on screen, is tighter.
    assert!(
        last.stretch < 0.01 * rig.height,
        "{what}: corpse stretch: {last}"
    );
    assert!(
        last.hinge < 5f32.to_radians(),
        "{what}: corpse hinge: {last}"
    );
    assert!(
        last.ball < 15f32.to_radians(),
        "{what}: corpse ball: {last}"
    );
}

/// The worst ground penetration (units), bone stretch (units) and joint
/// limit excess (radians) over some poses.
struct Measure {
    ground: f32,
    stretch: f32,
    hinge: f32,
    hinge_at: String,
    ball: f32,
    ball_at: String,
}

impl std::fmt::Display for Measure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ground {:.2} u, stretch {:.2} u, hinge +{:.1}° ({}), ball +{:.1}° ({})",
            self.ground,
            self.stretch,
            self.hinge.to_degrees(),
            self.hinge_at,
            self.ball.to_degrees(),
            self.ball_at
        )
    }
}

fn measure(
    skeleton: &nif::Skeleton,
    rig: &Rig,
    start: &nif::Pose,
    poses: &[nif::Pose],
    ground: f32,
) -> Measure {
    let up = rig.figure.up;
    let mut m = Measure {
        ground: 0.0,
        stretch: 0.0,
        hinge: 0.0,
        hinge_at: String::new(),
        ball: 0.0,
        ball_at: String::new(),
    };
    for pose in poses {
        let f = frames(rig, pose);
        // No collision shape sinks into the ground.
        for (body, frame) in rig.bodies.iter().zip(&f) {
            m.ground = m.ground.max(ground - body.shape.lowest(frame, up));
        }
        // Limbs keep their length: every bone's distance to its parent
        // (but not to the scene root, which stays put by design).
        for (i, bone) in skeleton.bones.iter().enumerate() {
            let Some(p) = bone.parent else { continue };
            let p = p as usize;
            if skeleton.bones[p].parent.is_none() {
                continue;
            }
            let now = (origin(&pose.world[i]) - origin(&pose.world[p])).length();
            let then = (origin(&start.world[i]) - origin(&start.world[p])).length();
            m.stretch = m.stretch.max((now - then).abs());
        }
        // Joint limits hold; hinges stay hinges.
        for joint in &rig.joints {
            let [twist, side, flex] = joint_angles(joint, &f);
            let over = |v: f32, r: [f32; 2]| (r[0] - v).max(v - r[1]).max(0.0);
            let key = &rig.bodies[joint.child].key;
            match joint.kind {
                JointKind::Ball {
                    flex: fl,
                    side: sd,
                    twist: tw,
                } => {
                    for (axis, excess) in [
                        ("flex", over(flex, fl)),
                        ("side", over(side, sd)),
                        ("twist", over(twist, tw)),
                    ] {
                        if excess > m.ball {
                            m.ball = excess;
                            m.ball_at = format!("{key} {axis}");
                        }
                    }
                }
                JointKind::Hinge { flex: fl } => {
                    let excess = over(flex, fl).max(twist.abs()).max(side.abs());
                    if excess > m.hinge {
                        m.hinge = excess;
                        m.hinge_at = key.clone();
                    }
                }
                JointKind::Fixed => {}
            }
        }
    }
    m
}

#[test]
fn every_preset_falls_cleanly() {
    for preset in Preset::ALL {
        for seed in 1..=6 {
            check_quality(preset, seed);
        }
    }
}

#[test]
fn falls_go_the_way_they_are_pushed() {
    let skeleton = fixture::humanoid();
    let head = skeleton.find("Bip01 Head").unwrap() as usize;
    let forward = |clip: &RagdollClip| origin(&clip.corpse().world[head]).y;
    for seed in 1..=4 {
        let back = fall(Preset::FallBackward, seed).1;
        assert!(
            forward(&back) < -30.0,
            "seed {seed}: head ends at {}",
            forward(&back)
        );
        let front = fall(Preset::FallForward, seed).1;
        assert!(
            forward(&front) > 30.0,
            "seed {seed}: head ends at {}",
            forward(&front)
        );
        let thrown = fall(Preset::ThrownBack, seed).1;
        assert!(
            thrown.root_travel[1] < -60.0,
            "seed {seed}: {:?}",
            thrown.root_travel
        );
    }
}

#[test]
fn knees_never_bend_backwards() {
    // Covered by the limit check, but stated on its own: the knee hinge's
    // flex never goes below its stop.
    let (skeleton, clip) = fall(Preset::FallForward, 5);
    let rig = Rig::build(&skeleton, &RagdollSpec::humanoid()).unwrap();
    for key in ["calf_l", "calf_r", "forearm_l", "forearm_r"] {
        let child = rig.bodies.iter().position(|b| b.key == key).unwrap();
        let joint = rig.joints.iter().find(|j| j.child == child).unwrap();
        let JointKind::Hinge { flex } = joint.kind else {
            panic!("{key} is not a hinge")
        };
        for pose in &clip.poses {
            let angle = joint_angles(joint, &frames(&rig, pose))[2];
            assert!(
                angle > flex[0] - 5f32.to_radians(),
                "{key}: {:.1}°",
                angle.to_degrees()
            );
        }
    }
}

#[test]
fn hinge_flex_bends_the_natural_way() {
    // A knee bent by hand (the calf turned so the foot goes back) reads as
    // positive flex; the elbow (the hand comes forward) too.
    let skeleton = fixture::humanoid();
    let rig = Rig::build(&skeleton, &RagdollSpec::humanoid()).unwrap();
    let rest = nif::Pose::rest(&skeleton);
    let f = frames(&rig, &rest);
    for (key, towards) in [("calf_r", -Vector::Y), ("forearm_l", Vector::Y)] {
        let child = rig.bodies.iter().position(|b| b.key == key).unwrap();
        let joint = rig.joints.iter().find(|j| j.child == child).unwrap();
        assert!(joint_angles(joint, &f).iter().all(|a| a.abs() < 1e-4));
        // Turn the child 30° about its joint so its far end moves `towards`.
        let anchor = (f[joint.parent] * joint.frame1).translation;
        let (a, b) = rig.bodies[child].segment;
        let dir = f[child].transform_vector(b - a).normalize();
        let axis = dir.cross(towards).normalize();
        let turn = rapier3d::math::Rotation::from_axis_angle(axis, 30f32.to_radians());
        let mut bent = f.clone();
        bent[child] = Rigid::from_parts(
            anchor + turn * (f[child].translation - anchor),
            turn * f[child].rotation,
        );
        let flex = joint_angles(joint, &bent)[2].to_degrees();
        assert!((flex - 30.0).abs() < 0.5, "{key}: flex {flex}");
    }
}

#[test]
fn in_place_holds_the_pelvis_still() {
    let (skeleton, clip) = fall(Preset::ThrownBack, 1);
    let spec = RagdollSpec::humanoid();
    let pelvis = skeleton.find("Bip01 Pelvis").unwrap() as usize;
    let held = clip.in_place(&spec);
    let start = origin(&clip.poses[0].world[pelvis]);
    for (pose, t) in held.iter().zip(&clip.pelvis_travel) {
        let p = origin(&pose.world[pelvis]);
        assert!((p.x - start.x).abs() < 1e-3 && (p.y - start.y).abs() < 1e-3);
        assert_eq!(&pose.travel, t);
    }
    assert!(clip.root_travel[1] < -60.0);
}

#[test]
fn clip_starts_on_the_start_pose_and_keeps_the_frame_rate() {
    let skeleton = fixture::humanoid();
    let start = nif::Pose::rest(&skeleton);
    let clip = simulate(
        &skeleton,
        &start,
        &RagdollSpec::humanoid(),
        &Impulse::preset(Preset::Crumple),
        1,
        DURATION,
        15.0,
    )
    .unwrap();
    assert_eq!(clip.poses[0], start);
    assert_eq!(clip.sample_rate, 15.0);
    assert!((clip.poses.len() - 1) as f32 / 15.0 <= DURATION + 1e-3);
    assert_eq!(clip.pelvis_travel.len(), clip.poses.len());
    assert_eq!(clip.bodies, 14);
}

// ------------------------------------------------------------- robustness

fn try_fall(skeleton: &nif::Skeleton, spec: &RagdollSpec) -> Result<RagdollClip, RagdollError> {
    simulate(
        skeleton,
        &nif::Pose::rest(skeleton),
        spec,
        &Impulse::preset(Preset::FallBackward),
        1,
        DURATION,
        FPS,
    )
}

#[test]
fn missing_optional_bones_are_skipped() {
    let skeleton = fixture::humanoid_with(false);
    let clip = try_fall(&skeleton, &RagdollSpec::humanoid()).unwrap();
    assert_eq!(clip.bodies, 12); // no feet
    assert!(clip.settled);
    // Without Spine2 the chest falls back to Spine1; arms and head still
    // hang from a torso body.
    let mut skeleton = fixture::humanoid();
    for bone in &mut skeleton.bones {
        if bone.name == "Bip01 Spine2" {
            bone.name = "Bip01 Chest Renamed".into();
        }
    }
    let spec = RagdollSpec::humanoid();
    let rig = Rig::build(&skeleton, &spec).unwrap();
    let abdomen = rig.bodies.iter().position(|b| b.key == "abdomen").unwrap();
    for key in ["head", "upper_arm_l", "upper_arm_r"] {
        let k = rig.bodies.iter().position(|b| b.key == key).unwrap();
        let parent = rig.joints.iter().find(|j| j.child == k).unwrap().parent;
        assert!(
            parent == abdomen || rig.bodies[parent].key == "chest",
            "{key} hangs from {}",
            rig.bodies[parent].key
        );
    }
    assert!(try_fall(&skeleton, &spec).unwrap().settled);
}

#[test]
fn a_non_humanoid_is_refused_or_rigged_generically() {
    let creature = fixture::quadruped();
    match try_fall(&creature, &RagdollSpec::humanoid()) {
        // Its `hips` passes for a pelvis; the legs are missing.
        Err(RagdollError::MissingBones { bodies }) => {
            assert!(bodies.contains(&"thigh_l".to_owned()), "{bodies:?}");
        }
        other => panic!("expected MissingBones, got {other:?}"),
    }
    let spec = RagdollSpec::generic(&creature, 8).unwrap();
    assert!(
        spec.bodies.len() <= 8 && spec.bodies.len() >= 4,
        "{}",
        spec.bodies.len()
    );
    let clip = try_fall(&creature, &spec).unwrap();
    assert!(clip.settled, "generic creature did not settle");
    let again = try_fall(&creature, &spec).unwrap();
    assert_eq!(bits(&clip), bits(&again));
    // The humanoid also rigs generically, capped.
    let spec = RagdollSpec::generic(&fixture::humanoid(), 40).unwrap();
    assert!(spec.bodies.len() <= MAX_BODIES);
    try_fall(&fixture::humanoid(), &spec).unwrap();
}

#[test]
fn zero_length_bones_fall_back_to_fixed_lengths() {
    // Every calf sits on its thigh's origin: the thighs have no length.
    let mut skeleton = fixture::humanoid();
    for name in ["Bip01 L Calf", "Bip01 R Calf"] {
        let i = skeleton.find(name).unwrap() as usize;
        skeleton.bones[i].local.translation = [0.0; 3];
    }
    let clip = try_fall(&skeleton, &RagdollSpec::humanoid()).unwrap();
    assert!(
        clip.poses
            .iter()
            .all(|p| p.world.iter().flatten().flatten().all(|v| v.is_finite()))
    );
}

#[test]
fn bad_inputs_are_errors() {
    let skeleton = fixture::humanoid();
    let spec = RagdollSpec::humanoid();
    let impulse = Impulse::preset(Preset::FallBackward);
    let rest = nif::Pose::rest(&skeleton);
    let run = |pose: &nif::Pose, spec: &RagdollSpec, impulse: &Impulse, d: f32, fps: f32| {
        simulate(&skeleton, pose, spec, impulse, 1, d, fps)
    };
    let mut nan = rest.clone();
    nan.world[5][0][3] = f32::NAN;
    assert!(matches!(
        run(&nan, &spec, &impulse, 3.0, 10.0),
        Err(RagdollError::InvalidInput(_))
    ));
    let mut short = rest.clone();
    short.world.pop();
    assert!(matches!(
        run(&short, &spec, &impulse, 3.0, 10.0),
        Err(RagdollError::InvalidInput(_))
    ));
    assert!(matches!(
        run(&rest, &spec, &impulse, 0.0, 10.0),
        Err(RagdollError::InvalidInput(_))
    ));
    assert!(matches!(
        run(&rest, &spec, &impulse, f32::INFINITY, 10.0),
        Err(RagdollError::InvalidInput(_))
    ));
    assert!(matches!(
        run(&rest, &spec, &impulse, 3.0, 0.0),
        Err(RagdollError::InvalidInput(_))
    ));
    let mut wild = impulse.clone();
    wild.pushes[0].direction[0] = f32::NAN;
    assert!(matches!(
        run(&rest, &spec, &wild, 3.0, 10.0),
        Err(RagdollError::InvalidInput(_))
    ));
    for broken in [
        RagdollSpec {
            timestep: 0.0,
            ..spec.clone()
        },
        RagdollSpec {
            substeps: 0,
            ..spec.clone()
        },
        RagdollSpec {
            units_per_meter: -1.0,
            ..spec.clone()
        },
        RagdollSpec {
            up: [0.0, 1.0, 0.0],
            ..spec.clone()
        },
        RagdollSpec {
            total_mass: f32::NAN,
            ..spec.clone()
        },
        RagdollSpec {
            bodies: Vec::new(),
            ..spec.clone()
        },
    ] {
        assert!(matches!(
            run(&rest, &broken, &impulse, 3.0, 10.0),
            Err(RagdollError::InvalidSpec(_))
        ));
    }
    let mut no_pelvis = skeleton.clone();
    no_pelvis.bones[3].name = "Bip01 Hip Thing".into();
    assert!(matches!(
        simulate(&no_pelvis, &rest, &spec, &impulse, 1, 3.0, 10.0),
        Err(RagdollError::MissingBones { .. })
    ));
}

#[test]
fn the_step_cap_holds() {
    let skeleton = fixture::humanoid();
    let spec = RagdollSpec {
        max_steps: 12,
        ..RagdollSpec::humanoid()
    };
    let clip = try_fall(&skeleton, &spec).unwrap();
    assert!(clip.steps <= 12);
    assert!(!clip.settled);
    assert!(clip.poses.len() >= 2);
}

#[test]
fn a_violent_push_retries_or_fails_cleanly() {
    let skeleton = fixture::humanoid();
    let mut impulse = Impulse::preset(Preset::ThrownBack);
    for p in &mut impulse.pushes {
        p.speed = 45.0;
        p.duration = 0.0;
    }
    let result = simulate(
        &skeleton,
        &nif::Pose::rest(&skeleton),
        &RagdollSpec::humanoid(),
        &impulse,
        1,
        DURATION,
        FPS,
    );
    match result {
        Ok(clip) => {
            assert!(
                clip.attempt > 0,
                "a 45 m/s push should not pass the first try"
            );
            assert!(
                clip.poses.iter().all(|p| p
                    .world
                    .iter()
                    .flatten()
                    .flatten()
                    .all(|v| v.is_finite()))
            );
        }
        Err(RagdollError::Exploded { attempts, .. }) => assert_eq!(attempts, 3),
        Err(other) => panic!("unexpected {other}"),
    }
}

#[test]
fn bones_that_are_not_bodies_follow_rigidly() {
    let (skeleton, clip) = fall(Preset::FallBackward, 1);
    let start = &clip.poses[0];
    let corpse = clip.corpse();
    // A finger keeps its offset from the hand; a twist bone from its arm.
    for (bone, parent) in [
        ("Bip01 L Finger21", "Bip01 L Hand"),
        ("Bip01 RUpArmTwist", "Bip01 R UpperArm"),
    ] {
        let (b, p) = (
            skeleton.find(bone).unwrap() as usize,
            skeleton.find(parent).unwrap() as usize,
        );
        let local = |pose: &nif::Pose| -> Matrix34 {
            nif::math::mul(&nif::math::inverse(&pose.world[p]).unwrap(), &pose.world[b])
        };
        let (a, z) = (local(start), local(corpse));
        for r in 0..3 {
            for c in 0..4 {
                assert!((a[r][c] - z[r][c]).abs() < 1e-3, "{bone}: {a:?} vs {z:?}");
            }
        }
    }
}

/// Not a check: prints how the presets land across seeds. Run with
/// `cargo test -p ragdoll --release -- --ignored --nocapture survey`.
#[test]
#[ignore]
fn survey() {
    let skeleton = fixture::humanoid();
    let head = skeleton.find("Bip01 Head").unwrap() as usize;
    let chest = skeleton.find("Bip01 Spine2").unwrap() as usize;
    let env = |k: &str, d: f32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let base = RagdollSpec::humanoid();
    let spec = RagdollSpec {
        friction: env("RD_FRICTION", base.friction),
        joint_friction: env("RD_JOINT_FRICTION", base.joint_friction),
        angular_damping: env("RD_ANGULAR_DAMPING", base.angular_damping),
        linear_damping: env("RD_LINEAR_DAMPING", base.linear_damping),
        tone_stiffness: env("RD_TONE_STIFFNESS", base.tone_stiffness),
        residual_tone: env("RD_RESIDUAL", base.residual_tone),
        ..base
    };
    for preset in Preset::ALL {
        let mut line = format!("{:>14}:", preset.name());
        let began = std::time::Instant::now();
        for seed in 1..=12 {
            let clip = fall_with(preset, seed, &spec).1;
            let c = clip.corpse();
            // The chest's rest forward (+Y), now: its up component.
            let facing: f32 = (0..3)
                .map(|k| c.world[chest][2][k] * clip.poses[0].world[chest][1][k])
                .sum();
            let (top_i, top) = c
                .world
                .iter()
                .enumerate()
                .map(|(i, m)| (i, m[2][3]))
                .fold((0, f32::MIN), |a, b| if b.1 > a.1 { b } else { a });
            let top_name = skeleton.bones[top_i]
                .name
                .trim_start_matches("Bip01 ")
                .to_owned();
            line += &format!(
                " [{:.1}s y{:+.0} z{:.0} {} {}]",
                clip.settle_time,
                origin(&c.world[head]).y,
                top,
                top_name,
                if facing > 0.5 {
                    "up"
                } else if facing < -0.5 {
                    "dn"
                } else {
                    "sd"
                }
            );
        }
        eprintln!(
            "{line}  ({:.1} ms each)",
            began.elapsed().as_secs_f64() * 1e3 / 12.0
        );
    }
}
