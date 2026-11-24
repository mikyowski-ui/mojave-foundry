//! A synthetic humanoid skeleton for tests and the preview example.
//!
//! The proportions are generic adult anatomy in round numbers, about 128
//! units tall, standing on Z = 0 and facing +Y, with bone names in the
//! public `Bip01` biped convention. Nothing here comes from game data.

use nif::math::{Matrix34, inverse, mul};
use nif::{Bone, BoneTransform, Skeleton};

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|c| c / l)
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The world matrix of a bone at `origin` whose X axis points along `x`,
/// with Y as close to the figure's forward (+Y) as possible.
fn world(origin: [f32; 3], x: [f32; 3]) -> Matrix34 {
    let x = norm(x);
    let reference = if x[1].abs() > 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let z = norm(cross(x, reference));
    let y = cross(z, x);
    let mut m = [[0.0f32; 4]; 3];
    for r in 0..3 {
        m[r] = [x[r], y[r], z[r], origin[r]];
    }
    m
}

/// A humanoid of about 128 units, with twist, finger and helper bones. `with_hands_and_feet: false` leaves
/// out hands, fingers, feet and toes, to test missing bones.
pub fn humanoid_with(with_hands_and_feet: bool) -> Skeleton {
    const UP: [f32; 3] = [0.0, 0.0, 1.0];
    const DOWN: [f32; 3] = [0.0, 0.0, -1.0];
    const FWD: [f32; 3] = [0.0, 1.0, 0.0];
    let mut b = Builder::default();
    b.add("Scene Root", None, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    b.add("Bip01", Some("Scene Root"), [0.0, 0.0, 66.0], FWD);
    b.add("Bip01 NonAccum", Some("Bip01"), [0.0, 0.0, 66.0], FWD);
    b.add("Bip01 Pelvis", Some("Bip01 NonAccum"), [0.0, 0.0, 66.0], UP);
    b.add("Bip01 Spine", Some("Bip01 NonAccum"), [0.0, 1.0, 72.0], UP);
    b.add("Bip01 Spine1", Some("Bip01 Spine"), [0.0, 1.0, 80.0], UP);
    b.add("Bip01 Spine2", Some("Bip01 Spine1"), [0.0, 1.0, 89.0], UP);
    b.add("Bip01 Neck", Some("Bip01 Spine2"), [0.0, 0.0, 105.0], UP);
    b.add("Bip01 Neck1", Some("Bip01 Neck"), [0.0, 0.0, 107.0], UP);
    b.add("Bip01 Head", Some("Bip01 Neck1"), [0.0, 0.5, 111.0], UP);
    // A helper at the head's own origin: a zero-length bone.
    b.add("HeadHelper", Some("Bip01 Head"), [0.0, 0.5, 111.0], UP);
    for (s, side) in [(-1.0f32, "L"), (1.0, "R")] {
        let n = |part: &str| format!("Bip01 {side} {part}");
        let arm = norm([s * 0.7, 0.0, -0.714]);
        let at = |base: [f32; 3], d: [f32; 3], len: f32| -> [f32; 3] {
            [
                base[0] + d[0] * len,
                base[1] + d[1] * len,
                base[2] + d[2] * len,
            ]
        };
        let shoulder = [s * 14.0, 0.0, 102.0];
        let elbow = at(shoulder, arm, 22.0);
        let wrist = at(elbow, arm, 20.0);
        let knuckle = at(wrist, arm, 6.0);
        b.add(
            &n("Clavicle"),
            Some("Bip01 Neck"),
            [s * 3.0, 0.0, 104.0],
            [s, 0.0, -0.15],
        );
        b.add(&n("UpperArm"), Some(&n("Clavicle")), shoulder, arm);
        b.add(
            &format!("Bip01 {side}UpArmTwist"),
            Some(&n("UpperArm")),
            shoulder,
            arm,
        );
        b.add(&n("Forearm"), Some(&n("UpperArm")), elbow, arm);
        b.add(&n("ForeTwist"), Some(&n("Forearm")), elbow, arm);
        if with_hands_and_feet {
            b.add(&n("Hand"), Some(&n("Forearm")), wrist, arm);
            b.add(&n("Finger2"), Some(&n("Hand")), knuckle, arm);
            b.add(
                &n("Finger21"),
                Some(&n("Finger2")),
                at(knuckle, arm, 3.0),
                arm,
            );
        }
        b.add(
            &n("Thigh"),
            Some("Bip01 Pelvis"),
            [s * 7.0, 0.0, 66.0],
            DOWN,
        );
        b.add(&n("Calf"), Some(&n("Thigh")), [s * 7.0, 1.0, 36.0], DOWN);
        if with_hands_and_feet {
            b.add(&n("Foot"), Some(&n("Calf")), [s * 7.0, 0.0, 7.0], DOWN);
            b.add(&n("Toe0"), Some(&n("Foot")), [s * 7.0, 11.0, 1.0], FWD);
        }
    }
    b.skeleton
}

/// Builds a skeleton from world origins and X directions.
#[derive(Default)]
struct Builder {
    skeleton: Skeleton,
    worlds: Vec<Matrix34>,
}

impl Builder {
    fn add(&mut self, name: &str, parent: Option<&str>, origin: [f32; 3], x: [f32; 3]) {
        let w = world(origin, x);
        let parent = parent.map(|p| {
            self.skeleton
                .bones
                .iter()
                .position(|b| b.name == p)
                .expect("parent added first")
        });
        let local = match parent {
            Some(p) => mul(&inverse(&self.worlds[p]).expect("orthonormal"), &w),
            None => w,
        };
        self.skeleton.bones.push(Bone {
            name: name.to_owned(),
            parent: parent.map(|p| p as u32),
            local: BoneTransform {
                rotation: std::array::from_fn(|r| [local[r][0], local[r][1], local[r][2]]),
                translation: [local[0][3], local[1][3], local[2][3]],
                scale: 1.0,
            },
        });
        self.worlds.push(w);
    }
}

/// A standing pose of [`humanoid`] (or [`humanoid_with`]): the rest pose
/// with the arms hanging at the sides instead of the rest pose's A-pose,
/// as a stand animation holds them.
pub fn stand(skeleton: &Skeleton) -> nif::Pose {
    let mut pose = nif::Pose::rest(skeleton);
    for (side, s) in [("L", -1.0f32), ("R", 1.0)] {
        let Some(arm) = skeleton.find(&format!("Bip01 {side} UpperArm")) else {
            continue;
        };
        let arm = arm as usize;
        let m = pose.world[arm];
        let shoulder = [m[0][3], m[1][3], m[2][3]];
        let from = norm([m[0][0], m[1][0], m[2][0]]);
        let to = norm([s * 0.17, 0.05, -1.0]);
        let turn = rotation_between(from, to);
        // Turn the arm and everything below it about the shoulder.
        for (i, world) in pose.world.iter_mut().enumerate() {
            let mut b = Some(i as u32);
            let mut under = false;
            while let Some(k) = b {
                if k as usize == arm {
                    under = true;
                    break;
                }
                b = skeleton.bones[k as usize].parent;
            }
            if !under {
                continue;
            }
            let w = *world;
            let mut out = [[0.0f32; 4]; 3];
            for r in 0..3 {
                for c in 0..3 {
                    out[r][c] = (0..3).map(|k| turn[r][k] * w[k][c]).sum();
                }
                let rel: [f32; 3] = std::array::from_fn(|k| w[k][3] - shoulder[k]);
                out[r][3] = shoulder[r] + (0..3).map(|k| turn[r][k] * rel[k]).sum::<f32>();
            }
            *world = out;
        }
    }
    pose
}

/// The rotation (rows) taking unit `a` to unit `b` (Rodrigues, without
/// trigonometry): `I + [v]× + [v]×² / (1 + a·b)`, `v = a × b`.
fn rotation_between(a: [f32; 3], b: [f32; 3]) -> [[f32; 3]; 3] {
    let v = cross(a, b);
    let c = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let k = [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]];
    let mut r = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let k2: f32 = (0..3).map(|n| k[i][n] * k[n][j]).sum();
            r[i][j] = f32::from(u8::from(i == j)) + k[i][j] + k2 / (1.0 + c);
        }
    }
    r
}

/// The full synthetic humanoid ([`humanoid_with`] with hands and feet).
pub fn humanoid() -> Skeleton {
    humanoid_with(true)
}

/// A non-humanoid: a four-legged chain creature with no `Bip01` names.
pub fn quadruped() -> Skeleton {
    let mut b = Builder::default();
    b.add("root", None, [0.0, 0.0, 40.0], [0.0, 1.0, 0.0]);
    b.add("hips", Some("root"), [0.0, -20.0, 40.0], [0.0, 1.0, 0.0]);
    b.add("chest", Some("hips"), [0.0, 20.0, 42.0], [0.0, 1.0, 0.0]);
    b.add("skull", Some("chest"), [0.0, 38.0, 48.0], [0.0, 1.0, 0.0]);
    b.add("snout", Some("skull"), [0.0, 50.0, 46.0], [0.0, 1.0, 0.0]);
    for (x, y, end) in [
        (-8.0, 20.0, "fl"),
        (8.0, 20.0, "fr"),
        (-8.0, -20.0, "bl"),
        (8.0, -20.0, "br"),
    ] {
        let parent = if y > 0.0 { "chest" } else { "hips" };
        b.add(
            &format!("leg_{end}"),
            Some(parent),
            [x, y, 40.0],
            [0.0, 0.0, -1.0],
        );
        b.add(
            &format!("knee_{end}"),
            Some(&format!("leg_{end}")),
            [x, y, 20.0],
            [0.0, 0.0, -1.0],
        );
        b.add(
            &format!("paw_{end}"),
            Some(&format!("knee_{end}")),
            [x, y, 2.0],
            [0.0, 1.0, 0.0],
        );
    }
    b.skeleton
}
