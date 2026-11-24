//! Posing a skeleton with an animation.

use crate::animation::Animation;
use crate::math::{Matrix34, apply_vector, inverse, quat_to_rotation, transform_matrix};
use crate::mesh::Transform;
use crate::skeleton::Skeleton;

/// A skeleton posed at one time.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pose {
    /// One matrix per skeleton bone, same order: the bone's space to the
    /// skeleton's root space ([`crate::math`] conventions).
    pub world: Vec<Matrix34>,
    /// The accumulation root's horizontal translation, X and Y in root
    /// space, from time 0 to the posed time, whole loops included. The
    /// posed accumulation root itself is held at its time-0 horizontal
    /// position, so a walk is rendered in place and moved by `travel`.
    pub travel: [f32; 2],
}

impl Pose {
    /// The skeleton's own rest pose.
    pub fn rest(skeleton: &Skeleton) -> Pose {
        Pose {
            world: skeleton.world_matrices(),
            travel: [0.0; 2],
        }
    }
}

/// The animation time `time` maps to, and the number of whole loops before
/// it. A looping animation wraps; any other is clamped to its duration.
fn local_time(animation: &Animation, time: f32) -> (f32, f32) {
    let d = animation.duration;
    if !time.is_finite() || !(d.is_finite() && d > 0.0) {
        return (0.0, 0.0);
    }
    if animation.looping {
        let loops = (time / d).floor();
        ((time - loops * d).clamp(0.0, d), loops)
    } else {
        (time.clamp(0.0, d), 0.0)
    }
}

/// Poses `skeleton` with `animation` at `time` seconds.
///
/// Each bone's local transform is the skeleton's, with each channel
/// (rotation, translation, scale) that the bone's track has keys for
/// replaced by the track's value at that time. Tracks are matched to bones
/// by name ([`Skeleton::find`]); a bone no track names keeps its skeleton
/// transform, and a track naming no bone is ignored. Rotations are
/// interpolated on the shorter arc.
///
/// Time wraps for a looping animation and is clamped otherwise.
///
/// The bone named [`Animation::accum_root`] (`Bip01` in every human
/// animation) is the character's placement: the skeleton stores it lifted
/// to hip height, while the animations key it at the ground and lift its
/// child (`Bip01 NonAccum`) instead. So its channels that the animation
/// does not key are the identity rather than the skeleton's values, and
/// its horizontal motion is taken out and returned as [`Pose::travel`].
pub fn pose(skeleton: &Skeleton, animation: &Animation, time: f32) -> Pose {
    compose(skeleton, &local_pose(skeleton, animation, time))
}

/// A skeleton posed at one time, as each bone's local transform (its
/// parent's space), before the bones are chained: what [`blend`] mixes.
/// [`compose`] turns it into the [`Pose`] that [`pose()`] gives.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LocalPose {
    /// One transform per skeleton bone, same order.
    pub locals: Vec<Transform>,
    /// As [`Pose::travel`].
    pub travel: [f32; 2],
}

/// The local transforms [`pose()`] chains: the same rules, the
/// accumulation root already held in place.
pub fn local_pose(skeleton: &Skeleton, animation: &Animation, time: f32) -> LocalPose {
    let (t, loops) = local_time(animation, time);
    let accum = (!animation.accum_root.is_empty())
        .then(|| skeleton.find(&animation.accum_root))
        .flatten()
        .map(|i| i as usize);
    let mut locals: Vec<Transform> = Vec::with_capacity(skeleton.bones.len());
    let mut tracks = vec![None; skeleton.bones.len()];
    for (i, bone) in skeleton.bones.iter().enumerate() {
        // The accumulation root is where the animation puts the character;
        // a channel the animation does not key is the identity there, not
        // the skeleton's rest value.
        let mut local: Transform = if Some(i) == accum {
            Transform::IDENTITY
        } else {
            bone.local.into()
        };
        if let Some(track) = animation.track(&bone.name) {
            tracks[i] = Some(track);
            if let Some(q) = track.rotation_at(t) {
                local.rotation = quat_to_rotation(q);
            }
            if let Some(v) = track.translation_at(t) {
                local.translation = v;
            }
            if let Some(s) = track.scale_at(t) {
                local.scale = s;
            }
        }
        locals.push(local);
    }

    let mut travel = [0.0f32; 2];
    let mut world: Vec<Matrix34> = Vec::with_capacity(skeleton.bones.len());
    for (i, bone) in skeleton.bones.iter().enumerate() {
        let parent = bone.parent.and_then(|p| world.get(p as usize)).copied();
        if Some(i) == accum
            && let Some(track) = tracks[i]
            && let (Some(now), Some(zero), Some(end)) = (
                track.translation_at(t),
                track.translation_at(0.0),
                track.translation_at(animation.duration),
            )
        {
            let parent_m = parent.unwrap_or(crate::math::IDENTITY34);
            let delta: [f32; 3] =
                std::array::from_fn(|k| now[k] - zero[k] + loops * (end[k] - zero[k]));
            let moved = apply_vector(&parent_m, delta);
            travel = [moved[0], moved[1]];
            // Hold the bone at its time-0 horizontal position within this
            // loop.
            let within = apply_vector(&parent_m, std::array::from_fn(|k| now[k] - zero[k]));
            if let Some(back) = inverse(&parent_m) {
                let undo = apply_vector(&back, [within[0], within[1], 0.0]);
                for (k, u) in undo.iter().enumerate() {
                    locals[i].translation[k] -= u;
                }
            }
        }
        let local = transform_matrix(&locals[i]);
        world.push(match parent {
            Some(p) => crate::math::mul(&p, &local),
            None => local,
        });
    }
    LocalPose { locals, travel }
}

/// Chains local transforms into a [`Pose`]: each bone's matrix is its
/// parent's times its own. A pose with fewer transforms than bones keeps
/// the skeleton's own for the rest.
pub fn compose(skeleton: &Skeleton, local: &LocalPose) -> Pose {
    let mut world: Vec<Matrix34> = Vec::with_capacity(skeleton.bones.len());
    for (i, bone) in skeleton.bones.iter().enumerate() {
        let parent = bone.parent.and_then(|p| world.get(p as usize)).copied();
        let matrix = match local.locals.get(i) {
            Some(t) => transform_matrix(t),
            None => transform_matrix(&bone.local.into()),
        };
        world.push(match parent {
            Some(p) => crate::math::mul(&p, &matrix),
            None => matrix,
        });
    }
    Pose {
        world,
        travel: local.travel,
    }
}

/// `a` mixed towards `b` by `weight` (0 is `a`, 1 is `b`), bone by bone:
/// rotations on the shorter arc, translations, scales and travel
/// linearly. Bones past the shorter pose keep `a`'s transform.
pub fn blend(a: &LocalPose, b: &LocalPose, weight: f32) -> LocalPose {
    let w = if weight.is_finite() {
        weight.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let locals = a
        .locals
        .iter()
        .enumerate()
        .map(|(i, x)| match b.locals.get(i) {
            Some(y) => blend_transform(x, y, w),
            None => *x,
        })
        .collect();
    LocalPose {
        locals,
        travel: [
            a.travel[0] + (b.travel[0] - a.travel[0]) * w,
            a.travel[1] + (b.travel[1] - a.travel[1]) * w,
        ],
    }
}

/// `base` with the bones `mask` marks taken from `overlay`: an upper body
/// from one animation over the legs of another. The travel is `base`'s.
pub fn layer(base: &LocalPose, overlay: &LocalPose, mask: &[bool]) -> LocalPose {
    let locals = base
        .locals
        .iter()
        .enumerate()
        .map(|(i, x)| {
            if mask.get(i).copied().unwrap_or(false) {
                overlay.locals.get(i).copied().unwrap_or(*x)
            } else {
                *x
            }
        })
        .collect();
    LocalPose {
        locals,
        travel: base.travel,
    }
}

/// Every bone at or below the bone named `name`, as a mask for [`layer`];
/// all `false` when the skeleton has no such bone. A parent precedes its
/// children, so one pass suffices.
pub fn subtree_mask(skeleton: &Skeleton, name: &str) -> Vec<bool> {
    let top = skeleton.find(name).map(|i| i as usize);
    let mut mask = vec![false; skeleton.bones.len()];
    for (i, bone) in skeleton.bones.iter().enumerate() {
        mask[i] = Some(i) == top
            || bone
                .parent
                .is_some_and(|p| mask.get(p as usize).copied().unwrap_or(false));
    }
    mask
}

fn blend_transform(a: &Transform, b: &Transform, w: f32) -> Transform {
    let qa = crate::math::rotation_to_quat(&a.rotation);
    let qb = crate::math::rotation_to_quat(&b.rotation);
    Transform {
        rotation: quat_to_rotation(crate::math::slerp(qa, qb, w)),
        translation: std::array::from_fn(|k| {
            a.translation[k] + (b.translation[k] - a.translation[k]) * w
        }),
        scale: a.scale + (b.scale - a.scale) * w,
    }
}
