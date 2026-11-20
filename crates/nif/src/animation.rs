//! `.kf` animations: one `NiControllerSequence` turned into plain key
//! tracks.
//!
//! # Times
//!
//! Every time is in seconds from the sequence's start time, so a track
//! runs from 0 to [`Animation::duration`] (`stop_time - start_time`).
//!
//! # Keys
//!
//! A [`Track`] holds plain keys sorted by time: rotations are interpolated
//! by [`crate::math::slerp`] on the shorter arc, translations and scales
//! linearly. Two keys may share a time; the value then jumps there. Before
//! the first key a channel holds the first value, after the last key the
//! last value. The stored key types become plain keys like this:
//!
//! - **linear** keys are copied;
//! - **constant** keys become steps (each key repeated at the next key's
//!   time, just before the jump);
//! - **quadratic** (Hermite) and **TBC** (Kochanek–Bartels, with the usual
//!   adjustment for unequal key spacing) translation, scale and Euler angle
//!   keys are evaluated on their curve and sampled at [`SAMPLE_RATE`] per
//!   second within each key interval, the keys themselves included;
//! - quaternion keys of the **quadratic** and **TBC** types store no usable
//!   tangents for a plain track and are interpolated by slerp between the
//!   stored keys (their curve between keys is not reproduced);
//! - **Euler** rotation keys (three angle key groups, radians) are sampled
//!   at every key time of any of the three groups and turned into
//!   quaternions by [`euler_xyz_to_quat`];
//! - **B-spline** tracks (`NiBSplineCompTransformInterpolator` and the
//!   uncompressed `NiBSplineTransformInterpolator`) are open uniform cubic
//!   B-splines over the interpolator's start and stop time; their control
//!   points are decompressed (`offset + half_range · value / 32767`) and the
//!   curve is sampled at [`SAMPLE_RATE`] per second, both ends included;
//!   rotations are scaled back to unit length;
//! - a channel without keys takes the interpolator's own transform value as
//!   one key at time 0, unless that value is marked as not set; then the
//!   channel is empty and the bone keeps its skeleton value.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::anim_blocks::{
    CYCLE_CLAMP, KEY_CONSTANT, KEY_QUADRATIC, KEY_TBC, Key, KeyGroup,
    NiBSplineTransformInterpolator, NiControllerSequence, NiTransformData, NiTransformInterpolator,
    QuatTransform, no_handle,
};
use crate::blocks::Block;
use crate::error::NifError;
use crate::math::{normalize_quat, quat_axis_angle, quat_mul, slerp};
use crate::{BlockEntry, NifFile};

/// Samples per second for curves that are not plain keys (B-splines,
/// quadratic and TBC keys). 30 per second is the rate the source
/// animations are authored at.
pub const SAMPLE_RATE: f32 = 30.0;

/// Keys of one bone.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Track {
    /// The animated node's name (the controlled block's node name).
    pub bone: String,
    /// `(time, quaternion [w, x, y, z])`, sorted by time.
    pub rotations: Vec<(f32, [f32; 4])>,
    /// `(time, translation)` relative to the parent, sorted by time.
    pub translations: Vec<(f32, [f32; 3])>,
    /// `(time, uniform scale)`, sorted by time.
    pub scales: Vec<(f32, f32)>,
}

/// The keys around `time`: `(index of the key at or before, fraction to
/// the next key)`. `None` when there are no keys.
fn locate<T>(keys: &[(f32, T)], time: f32) -> Option<(usize, f32)> {
    let first = keys.first()?;
    if time <= first.0 || keys.len() == 1 {
        return Some((0, 0.0));
    }
    // Last key with key time <= time.
    let i = keys.partition_point(|k| k.0 <= time).saturating_sub(1);
    if i + 1 >= keys.len() {
        return Some((keys.len() - 1, 0.0));
    }
    let (t0, t1) = (keys[i].0, keys[i + 1].0);
    let span = t1 - t0;
    let f = if span > 0.0 { (time - t0) / span } else { 0.0 };
    Some((i, f.clamp(0.0, 1.0)))
}

impl Track {
    /// The rotation at `time`, or `None` without rotation keys.
    pub fn rotation_at(&self, time: f32) -> Option<[f32; 4]> {
        let (i, f) = locate(&self.rotations, time)?;
        let a = self.rotations[i].1;
        Some(match self.rotations.get(i + 1) {
            Some(&(_, b)) if f > 0.0 => slerp(a, b, f),
            _ => normalize_quat(a),
        })
    }

    /// The translation at `time`, or `None` without translation keys.
    pub fn translation_at(&self, time: f32) -> Option<[f32; 3]> {
        let (i, f) = locate(&self.translations, time)?;
        let a = self.translations[i].1;
        Some(match self.translations.get(i + 1) {
            Some(&(_, b)) if f > 0.0 => std::array::from_fn(|k| a[k] + (b[k] - a[k]) * f),
            _ => a,
        })
    }

    /// The scale at `time`, or `None` without scale keys.
    pub fn scale_at(&self, time: f32) -> Option<f32> {
        let (i, f) = locate(&self.scales, time)?;
        let a = self.scales[i].1;
        Some(match self.scales.get(i + 1) {
            Some(&(_, b)) if f > 0.0 => a + (b - a) * f,
            _ => a,
        })
    }

    /// Whether the track has no key at all.
    pub fn is_empty(&self) -> bool {
        self.rotations.is_empty() && self.translations.is_empty() && self.scales.is_empty()
    }
}

/// One animation: a `NiControllerSequence` as plain key tracks.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Animation {
    /// The sequence name.
    pub name: String,
    /// `stop_time - start_time`, in seconds.
    pub duration: f32,
    /// Whether the sequence repeats (cycle type loop or reverse); a clamped
    /// sequence holds its last frame.
    pub looping: bool,
    /// One track per animated bone, in controlled block order; a bone
    /// appears at most once.
    pub tracks: Vec<Track>,
    /// `(time, text)` from the sequence's text keys, in stored order.
    pub text_keys: Vec<(f32, String)>,
    /// The accumulation root's node name: the bone whose horizontal motion
    /// is the character's travel (see [`crate::Pose::travel`]). Empty when
    /// the sequence names none.
    pub accum_root: String,
}

impl Animation {
    /// The track of `bone`: exact name first, then ignoring ASCII case.
    pub fn track(&self, bone: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.bone == bone).or_else(|| {
            self.tracks
                .iter()
                .find(|t| t.bone.eq_ignore_ascii_case(bone))
        })
    }
}

/// What [`extract_animation_with_report`] could not turn into tracks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnimationReport {
    /// Interpolator type to the number of controlled blocks that use it and
    /// were left out (float, boolean and point interpolators, ...).
    pub unhandled: BTreeMap<String, u32>,
    /// Controlled blocks without an interpolator.
    pub without_interpolator: u32,
    /// Controlled blocks that animate a bone an earlier block already
    /// animates; the first one wins.
    pub duplicates: u32,
}

fn anim_error(index: u32, entry: &BlockEntry, reason: impl Into<String>) -> NifError {
    NifError::Scene {
        index,
        type_name: entry.type_name.clone(),
        offset: entry.offset,
        reason: reason.into(),
    }
}

/// Values a key group can hold.
trait Value: Copy + Default {
    fn add(self, other: Self) -> Self;
    fn sub(self, other: Self) -> Self;
    fn scale(self, s: f32) -> Self;
}

impl Value for f32 {
    fn add(self, other: Self) -> Self {
        self + other
    }
    fn sub(self, other: Self) -> Self {
        self - other
    }
    fn scale(self, s: f32) -> Self {
        self * s
    }
}

impl Value for [f32; 3] {
    fn add(self, o: Self) -> Self {
        [self[0] + o[0], self[1] + o[1], self[2] + o[2]]
    }
    fn sub(self, o: Self) -> Self {
        [self[0] - o[0], self[1] - o[1], self[2] - o[2]]
    }
    fn scale(self, s: f32) -> Self {
        self.map(|v| v * s)
    }
}

fn hermite<T: Value>(p0: T, m0: T, p1: T, m1: T, s: f32) -> T {
    let s2 = s * s;
    let s3 = s2 * s;
    p0.scale(2.0 * s3 - 3.0 * s2 + 1.0)
        .add(m0.scale(s3 - 2.0 * s2 + s))
        .add(p1.scale(-2.0 * s3 + 3.0 * s2))
        .add(m1.scale(s3 - s2))
}

/// Kochanek–Bartels tangents of key `i`: (incoming, outgoing), per
/// segment, adjusted for unequal key spacing.
fn tbc_tangents<T: Value>(keys: &[Key<T>], i: usize) -> (T, T) {
    let p = keys[i].value;
    let next = keys.get(i + 1);
    let prev = i.checked_sub(1).map(|j| &keys[j]);
    let (d_prev, dt_prev, d_next, dt_next) = match (prev, next) {
        (Some(a), Some(b)) => (
            p.sub(a.value),
            keys[i].time - a.time,
            b.value.sub(p),
            b.time - keys[i].time,
        ),
        (Some(a), None) => {
            let d = p.sub(a.value);
            let dt = keys[i].time - a.time;
            (d, dt, d, dt)
        }
        (None, Some(b)) => {
            let d = b.value.sub(p);
            let dt = b.time - keys[i].time;
            (d, dt, d, dt)
        }
        (None, None) => return (T::default(), T::default()),
    };
    let [t, c, b] = keys[i].tbc;
    let incoming = d_prev
        .scale((1.0 - t) * (1.0 + c) * (1.0 + b) / 2.0)
        .add(d_next.scale((1.0 - t) * (1.0 - c) * (1.0 - b) / 2.0));
    let outgoing = d_prev
        .scale((1.0 - t) * (1.0 - c) * (1.0 + b) / 2.0)
        .add(d_next.scale((1.0 - t) * (1.0 + c) * (1.0 - b) / 2.0));
    let total = dt_prev + dt_next;
    if total > 0.0 && total.is_finite() {
        (
            incoming.scale(2.0 * dt_prev / total),
            outgoing.scale(2.0 * dt_next / total),
        )
    } else {
        (incoming, outgoing)
    }
}

/// Number of samples a curve segment of `span` seconds is cut into.
fn steps(span: f32) -> usize {
    if span.is_finite() && span > 0.0 {
        // Bounded: a span is at most the stored float range over 30 Hz,
        // but a hostile file must not make this loop unbounded.
        ((span * SAMPLE_RATE).ceil() as usize).clamp(1, 1 << 16)
    } else {
        1
    }
}

/// The most keys [`extract_animation`] creates by sampling curves, over the
/// whole animation. A real animation needs a few thousand; a file that asks
/// for more is an error rather than an unbounded allocation.
pub const MAX_SAMPLED_KEYS: usize = 1 << 22;

/// What is left of [`MAX_SAMPLED_KEYS`].
struct Budget(usize);

impl Budget {
    fn take(&mut self, keys: usize) -> Result<(), NifError> {
        match self.0.checked_sub(keys) {
            Some(left) => {
                self.0 = left;
                Ok(())
            }
            None => Err(NifError::Animation {
                reason: format!(
                    "sampling the curves at {SAMPLE_RATE} per second would create more than \
                     {MAX_SAMPLED_KEYS} keys"
                ),
            }),
        }
    }
}

/// A key group as plain keys, times shifted by `-start`.
fn plain_keys<T: Value>(
    group: &KeyGroup<T>,
    start: f32,
    budget: &mut Budget,
) -> Result<Vec<(f32, T)>, NifError> {
    let keys = &group.keys;
    let mut out = Vec::with_capacity(keys.len());
    match group.interpolation {
        KEY_CONSTANT => {
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push((key.time - start, keys[i - 1].value));
                }
                out.push((key.time - start, key.value));
            }
        }
        KEY_QUADRATIC | KEY_TBC if keys.len() > 1 => {
            let total = keys.windows(2).fold(1usize, |n, w| {
                n.saturating_add(steps(w[1].time - w[0].time))
            });
            budget.take(total)?;
            let tangents: Vec<(T, T)> = if group.interpolation == KEY_TBC {
                (0..keys.len()).map(|i| tbc_tangents(keys, i)).collect()
            } else {
                keys.iter().map(|k| (k.backward, k.forward)).collect()
            };
            for i in 0..keys.len() - 1 {
                let (a, b) = (&keys[i], &keys[i + 1]);
                let n = steps(b.time - a.time);
                for k in 0..n {
                    let s = k as f32 / n as f32;
                    let value = hermite(a.value, tangents[i].1, b.value, tangents[i + 1].0, s);
                    out.push((a.time + (b.time - a.time) * s - start, value));
                }
            }
            let last = &keys[keys.len() - 1];
            out.push((last.time - start, last.value));
        }
        _ => out.extend(keys.iter().map(|k| (k.time - start, k.value))),
    }
    Ok(out)
}

fn sort_keys<T>(keys: &mut [(f32, T)]) {
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
}

/// The quaternion of Euler angles about X, Y and Z (radians): the rotation
/// about X applies first, then Y, then Z, `R = Rz · Ry · Rx`, each a
/// counter-clockwise turn looking down its axis.
pub fn euler_xyz_to_quat(x: f32, y: f32, z: f32) -> [f32; 4] {
    let qx = quat_axis_angle([1.0, 0.0, 0.0], x);
    let qy = quat_axis_angle([0.0, 1.0, 0.0], y);
    let qz = quat_axis_angle([0.0, 0.0, 1.0], z);
    normalize_quat(quat_mul(qz, quat_mul(qy, qx)))
}

fn sample_linear(keys: &[(f32, f32)], time: f32) -> f32 {
    let Some((i, f)) = locate(keys, time) else {
        return 0.0;
    };
    match keys.get(i + 1) {
        Some(&(_, b)) if f > 0.0 => keys[i].1 + (b - keys[i].1) * f,
        _ => keys[i].1,
    }
}

fn rotation_keys(
    data: &NiTransformData,
    start: f32,
    budget: &mut Budget,
) -> Result<Vec<(f32, [f32; 4])>, NifError> {
    if let Some(groups) = &data.xyz_rotations {
        let mut channels: Vec<Vec<(f32, f32)>> = Vec::with_capacity(3);
        for g in groups.iter() {
            let mut keys = plain_keys(g, start, budget)?;
            sort_keys(&mut keys);
            channels.push(keys);
        }
        let mut times: Vec<f32> = channels.iter().flatten().map(|k| k.0).collect();
        times.sort_by(f32::total_cmp);
        times.dedup();
        return Ok(times
            .into_iter()
            .map(|t| {
                let [x, y, z] = [0, 1, 2].map(|axis| sample_linear(&channels[axis], t));
                (t, euler_xyz_to_quat(x, y, z))
            })
            .collect());
    }
    let keys = &data.quaternion_keys;
    let mut out = Vec::with_capacity(keys.len());
    for (i, key) in keys.iter().enumerate() {
        if data.rotation_type == KEY_CONSTANT && i > 0 {
            out.push((key.time - start, normalize_quat(keys[i - 1].value)));
        }
        out.push((key.time - start, normalize_quat(key.value)));
    }
    Ok(out)
}

/// Flips quaternions so that each lies on the same side as the one
/// before: the same rotations, and slerp between neighbours takes the
/// short way.
fn continuous(keys: &mut [(f32, [f32; 4])]) {
    for i in 1..keys.len() {
        let prev = keys[i - 1].1;
        let q = &mut keys[i].1;
        if prev.iter().zip(q.iter()).map(|(a, b)| a * b).sum::<f32>() < 0.0 {
            *q = q.map(|v| -v);
        }
    }
}

fn static_channels(track: &mut Track, transform: &QuatTransform) {
    if track.rotations.is_empty() && QuatTransform::is_set(&transform.rotation) {
        track
            .rotations
            .push((0.0, normalize_quat(transform.rotation)));
    }
    if track.translations.is_empty() && QuatTransform::is_set(&transform.translation) {
        track.translations.push((0.0, transform.translation));
    }
    if track.scales.is_empty() && QuatTransform::is_set(&[transform.scale]) {
        track.scales.push((0.0, transform.scale));
    }
}

fn keyed_track(
    file: &NifFile,
    interpolator: &NiTransformInterpolator,
    bone: &str,
    start: f32,
    budget: &mut Budget,
) -> Result<Track, NifError> {
    let mut track = Track {
        bone: bone.to_owned(),
        ..Track::default()
    };
    if let Some(Block::TransformData(data)) = file.block(interpolator.data).map(|e| &e.block) {
        track.rotations = rotation_keys(data, start, budget)?;
        track.translations = plain_keys(&data.translations, start, budget)?;
        track.scales = plain_keys(&data.scales, start, budget)?;
    }
    static_channels(&mut track, &interpolator.transform);
    Ok(track)
}

/// Open uniform B-spline of `degree` over `points` control points of
/// `dim` values each, evaluated at parameter `u ∈ [0, n - degree]`.
fn de_boor(points: &[f32], dim: usize, degree: usize, u: f32, out: &mut [f32]) {
    let n = points.len() / dim;
    let knot = |j: usize| -> f32 {
        if j <= degree {
            0.0
        } else if j >= n {
            (n - degree) as f32
        } else {
            (j - degree) as f32
        }
    };
    // Knot span: degree <= span < n.
    let span = ((u.floor().max(0.0) as usize) + degree).min(n - 1);
    let mut d: Vec<f32> = Vec::with_capacity((degree + 1) * dim);
    for j in 0..=degree {
        let index = span + j - degree;
        d.extend_from_slice(&points[index * dim..index * dim + dim]);
    }
    for r in 1..=degree {
        for j in (r..=degree).rev() {
            let i = span + j - degree;
            let left = knot(i);
            let right = knot(i + degree + 1 - r);
            let alpha = if right > left {
                (u - left) / (right - left)
            } else {
                0.0
            };
            for c in 0..dim {
                d[j * dim + c] = (1.0 - alpha) * d[(j - 1) * dim + c] + alpha * d[j * dim + c];
            }
        }
    }
    out.copy_from_slice(&d[degree * dim..degree * dim + dim]);
}

fn spline_track(
    file: &NifFile,
    index: u32,
    entry: &BlockEntry,
    spline: &NiBSplineTransformInterpolator,
    bone: &str,
    start: f32,
    budget: &mut Budget,
) -> Result<Track, NifError> {
    let mut track = Track {
        bone: bone.to_owned(),
        ..Track::default()
    };
    let data = match file.block(spline.spline_data).map(|e| &e.block) {
        Some(Block::BSplineData(data)) => Some(data.as_ref()),
        _ => None,
    };
    let basis = match file.block(spline.basis_data).map(|e| &e.block) {
        Some(Block::BSplineBasisData(basis)) => Some(basis.num_control_points as usize),
        _ => None,
    };
    let handles = [
        spline.translation_handle,
        spline.rotation_handle,
        spline.scale_handle,
    ];
    if let (Some(data), Some(n)) = (data, basis)
        && n > 0
        && handles.iter().any(|&h| !no_handle(h))
    {
        let (t0, t1) = (spline.start_time, spline.stop_time);
        if !(t0.is_finite() && t1.is_finite() && t1 >= t0) {
            return Err(anim_error(
                index,
                entry,
                "the spline's start and stop times are unusable",
            ));
        }
        let count = steps(t1 - t0);
        let channels = handles.iter().filter(|&&h| !no_handle(h)).count();
        budget.take((count + 1).saturating_mul(channels))?;
        let times: Vec<f32> = (0..=count)
            .map(|k| {
                if k == count {
                    t1
                } else {
                    t0 + (t1 - t0) * k as f32 / count as f32
                }
            })
            .collect();
        let degree = 3.min(n - 1);
        let points =
            |handle: u32, dim: usize, offset: f32, half: f32| -> Result<Vec<f32>, NifError> {
                let first = handle as usize;
                let len = n.checked_mul(dim).and_then(|l| l.checked_add(first));
                let bad = || {
                    anim_error(
                        index,
                        entry,
                        "a spline handle points past the control points",
                    )
                };
                match spline.compression {
                    Some(_) => {
                        let values = data
                            .compact_control_points
                            .get(first..len.ok_or_else(bad)?)
                            .ok_or_else(bad)?;
                        Ok(values
                            .iter()
                            .map(|&v| offset + half * (f32::from(v) / 32767.0))
                            .collect())
                    }
                    None => Ok(data
                        .float_control_points
                        .get(first..len.ok_or_else(bad)?)
                        .ok_or_else(bad)?
                        .to_vec()),
                }
            };
        let c = spline.compression.unwrap_or_default();
        let u_of = |t: f32| -> f32 {
            if t1 > t0 {
                ((t - t0) / (t1 - t0) * (n - degree) as f32).clamp(0.0, (n - degree) as f32)
            } else {
                0.0
            }
        };
        if !no_handle(spline.translation_handle) {
            let p = points(
                spline.translation_handle,
                3,
                c.translation_offset,
                c.translation_half_range,
            )?;
            for &t in &times {
                let mut v = [0.0f32; 3];
                de_boor(&p, 3, degree, u_of(t), &mut v);
                track.translations.push((t - start, v));
            }
        }
        if !no_handle(spline.rotation_handle) {
            let p = points(
                spline.rotation_handle,
                4,
                c.rotation_offset,
                c.rotation_half_range,
            )?;
            for &t in &times {
                let mut v = [0.0f32; 4];
                de_boor(&p, 4, degree, u_of(t), &mut v);
                track.rotations.push((t - start, normalize_quat(v)));
            }
        }
        if !no_handle(spline.scale_handle) {
            let p = points(spline.scale_handle, 1, c.scale_offset, c.scale_half_range)?;
            for &t in &times {
                let mut v = [0.0f32; 1];
                de_boor(&p, 1, degree, u_of(t), &mut v);
                track.scales.push((t - start, v[0]));
            }
        }
    }
    static_channels(&mut track, &spline.transform);
    Ok(track)
}

fn check_finite(track: &Track, index: u32, entry: &BlockEntry) -> Result<(), NifError> {
    let finite = track
        .rotations
        .iter()
        .all(|(t, q)| t.is_finite() && q.iter().all(|v| v.is_finite()))
        && track
            .translations
            .iter()
            .all(|(t, v)| t.is_finite() && v.iter().all(|c| c.is_finite()))
        && track
            .scales
            .iter()
            .all(|(t, s)| t.is_finite() && s.is_finite());
    if finite {
        Ok(())
    } else {
        Err(anim_error(
            index,
            entry,
            "a key time or value is not finite",
        ))
    }
}

fn sequence_of(file: &NifFile) -> Option<(u32, &BlockEntry, &NiControllerSequence)> {
    let is_sequence = |i: u32| match file.blocks.get(i as usize) {
        Some(entry) => match &entry.block {
            Block::ControllerSequence(s) => Some((i, entry, s.as_ref())),
            _ => None,
        },
        None => None,
    };
    file.roots
        .iter()
        .flatten()
        .find_map(|&r| is_sequence(r))
        .or_else(|| (0..file.blocks.len() as u32).find_map(is_sequence))
}

/// Turns the file's controller sequence into an [`Animation`]: the first
/// root that is a `NiControllerSequence`, or else the first such block.
pub fn extract_animation(file: &NifFile) -> Result<Animation, NifError> {
    extract_animation_with_report(file).map(|(animation, _)| animation)
}

/// [`extract_animation`], and what it left out.
pub fn extract_animation_with_report(
    file: &NifFile,
) -> Result<(Animation, AnimationReport), NifError> {
    let Some((index, entry, sequence)) = sequence_of(file) else {
        return Err(NifError::Animation {
            reason: "the file has no NiControllerSequence".to_owned(),
        });
    };
    let start = sequence.start_time;
    let duration = sequence.stop_time - start;
    if !start.is_finite() || !duration.is_finite() || duration < 0.0 {
        return Err(anim_error(
            index,
            entry,
            "the sequence's start and stop times are unusable",
        ));
    }
    let mut report = AnimationReport::default();
    let mut tracks: Vec<Track> = Vec::new();
    let mut animated = std::collections::BTreeSet::new();
    let mut budget = Budget(MAX_SAMPLED_KEYS);
    for block in &sequence.controlled_blocks {
        let Some(interpolator) = block.interpolator else {
            report.without_interpolator += 1;
            continue;
        };
        let Some(source) = file.blocks.get(interpolator as usize) else {
            report.without_interpolator += 1;
            continue;
        };
        let transform = matches!(
            source.block,
            Block::TransformInterpolator(_) | Block::BSplineTransformInterpolator(_)
        );
        if !transform {
            *report
                .unhandled
                .entry(source.type_name.clone())
                .or_insert(0) += 1;
            continue;
        }
        if !animated.insert(block.node_name.as_str()) {
            report.duplicates += 1;
            continue;
        }
        let track = match &source.block {
            Block::TransformInterpolator(i) => {
                keyed_track(file, i, &block.node_name, start, &mut budget)?
            }
            Block::BSplineTransformInterpolator(s) => spline_track(
                file,
                interpolator,
                source,
                s,
                &block.node_name,
                start,
                &mut budget,
            )?,
            _ => continue,
        };
        check_finite(&track, interpolator, source)?;
        let mut track = track;
        sort_keys(&mut track.rotations);
        sort_keys(&mut track.translations);
        sort_keys(&mut track.scales);
        continuous(&mut track.rotations);
        if !track.is_empty() {
            tracks.push(track);
        }
    }
    let mut text_keys = Vec::new();
    if let Some(Block::TextKeyExtraData(text)) = file.block(sequence.text_keys).map(|e| &e.block) {
        for (time, value) in &text.keys {
            if !time.is_finite() {
                return Err(anim_error(index, entry, "a text key time is not finite"));
            }
            text_keys.push((time - start, value.clone()));
        }
    }
    Ok((
        Animation {
            name: sequence.name.clone(),
            duration,
            looping: sequence.cycle_type != CYCLE_CLAMP,
            tracks,
            text_keys,
            accum_root: sequence.accum_root_name.clone(),
        },
        report,
    ))
}

/// Parses a `.kf` file and returns its animation; see the module
/// documentation for how keys are turned into plain tracks. Besides stream
/// version 34 this accepts the older animation stream versions in
/// [`crate::ANIMATION_STREAM_VERSIONS`] (same file and user version); every
/// block is still checked against its declared size.
pub fn parse_animation(bytes: &[u8]) -> Result<Animation, NifError> {
    extract_animation(&parse_animation_file(bytes)?)
}

/// Parses a `.kf` file into blocks as [`parse_animation`] does, accepting
/// the older animation stream versions.
pub fn parse_animation_file(bytes: &[u8]) -> Result<NifFile, NifError> {
    crate::parse_with(bytes, crate::header::ANIMATION_STREAM_VERSIONS)
}
