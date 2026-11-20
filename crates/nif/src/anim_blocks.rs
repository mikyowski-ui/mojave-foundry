//! Animation blocks of `.kf` files (and of embedded controllers), for file
//! version 20.2.0.7, user version 11, stream version 34:
//! `NiControllerSequence`, the transform interpolators and their key data,
//! the B-spline interpolators and their control points, and
//! `NiTextKeyExtraData`.

use crate::blocks::Ref;
use crate::error::NifError;
use crate::reader::Reader;

/// Key interpolation 1: linear.
pub const KEY_LINEAR: u32 = 1;
/// Key interpolation 2: quadratic (Hermite, with a forward and a backward
/// tangent per key; quaternion keys store no tangents).
pub const KEY_QUADRATIC: u32 = 2;
/// Key interpolation 3: tension, continuity, bias (Kochanek–Bartels).
pub const KEY_TBC: u32 = 3;
/// Key interpolation 4: rotation as three Euler angle key groups.
pub const KEY_XYZ_ROTATION: u32 = 4;
/// Key interpolation 5: constant (a step at each key).
pub const KEY_CONSTANT: u32 = 5;

/// One key of a float or vector key group.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Key<T> {
    /// Time in seconds, in the sequence's time line.
    pub time: f32,
    /// Value.
    pub value: T,
    /// Quadratic keys: tangent leaving the key, per segment (not per
    /// second). Zero for other key types.
    pub forward: T,
    /// Quadratic keys: tangent arriving at the key, per segment. Zero for
    /// other key types.
    pub backward: T,
    /// TBC keys: tension, continuity, bias. Zero for other key types.
    pub tbc: [f32; 3],
}

/// A key group: an interpolation type and its keys.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeyGroup<T> {
    /// One of the `KEY_*` constants; 0 when the group has no keys.
    pub interpolation: u32,
    /// The keys, as stored.
    pub keys: Vec<Key<T>>,
}

/// One quaternion rotation key.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct QuatKey {
    /// Time in seconds.
    pub time: f32,
    /// Rotation, `w, x, y, z`.
    pub value: [f32; 4],
    /// TBC keys: tension, continuity, bias. Zero otherwise.
    pub tbc: [f32; 3],
}

/// `NiTransformData` (also `NiKeyframeData`): rotation, translation and
/// scale keys of one transform track.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiTransformData {
    /// Interpolation of the rotation keys; 0 when there are none.
    pub rotation_type: u32,
    /// Quaternion keys, unless `rotation_type` is [`KEY_XYZ_ROTATION`].
    pub quaternion_keys: Vec<QuatKey>,
    /// Euler angle keys about X, Y and Z in radians, when `rotation_type`
    /// is [`KEY_XYZ_ROTATION`].
    pub xyz_rotations: Option<Box<[KeyGroup<f32>; 3]>>,
    /// Translation keys.
    pub translations: KeyGroup<[f32; 3]>,
    /// Uniform scale keys.
    pub scales: KeyGroup<f32>,
}

/// A transform as interpolators store it. A component whose values are all
/// at or beyond ±3.0e38 means "not set" ([`QuatTransform::is_set`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuatTransform {
    /// Translation.
    pub translation: [f32; 3],
    /// Rotation, `w, x, y, z`.
    pub rotation: [f32; 4],
    /// Uniform scale.
    pub scale: f32,
}

impl Default for QuatTransform {
    fn default() -> Self {
        QuatTransform {
            translation: [0.0; 3],
            rotation: [1.0, 0.0, 0.0, 0.0],
            scale: 1.0,
        }
    }
}

/// The magnitude at and beyond which a stored transform component means
/// "not set". The format writes `-FLT_MAX` there.
pub const UNSET_THRESHOLD: f32 = 3.0e38;

impl QuatTransform {
    /// Whether every value of a component is finite and below
    /// [`UNSET_THRESHOLD`] in magnitude.
    pub fn is_set(values: &[f32]) -> bool {
        values
            .iter()
            .all(|v| v.is_finite() && v.abs() < UNSET_THRESHOLD)
    }
}

/// `NiTransformInterpolator` and `BSRotAccumTransfInterpolator`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiTransformInterpolator {
    /// The value used for a channel that has no keys.
    pub transform: QuatTransform,
    /// The `NiTransformData` block.
    pub data: Ref,
}

/// The six floats that decompress a `NiBSplineCompTransformInterpolator`'s
/// control points: `value = offset + half_range · short / 32767`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BSplineCompression {
    /// Translation offset.
    pub translation_offset: f32,
    /// Translation half range.
    pub translation_half_range: f32,
    /// Rotation offset.
    pub rotation_offset: f32,
    /// Rotation half range.
    pub rotation_half_range: f32,
    /// Scale offset.
    pub scale_offset: f32,
    /// Scale half range.
    pub scale_half_range: f32,
}

/// The handle value that means "this channel has no control points". The
/// files store it in the 32-bit handle field (checked on the real
/// animations); [`no_handle`] also accepts `0xFFFFFFFF`.
pub const NO_HANDLE: u32 = 0xFFFF;

/// Whether a B-spline handle means "no control points".
pub fn no_handle(handle: u32) -> bool {
    handle == NO_HANDLE || handle == u32::MAX
}

/// `NiBSplineTransformInterpolator` and
/// `NiBSplineCompTransformInterpolator`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiBSplineTransformInterpolator {
    /// Time of the first control point.
    pub start_time: f32,
    /// Time of the last control point.
    pub stop_time: f32,
    /// The `NiBSplineData` block.
    pub spline_data: Ref,
    /// The `NiBSplineBasisData` block.
    pub basis_data: Ref,
    /// The value used for a channel without control points.
    pub transform: QuatTransform,
    /// Index of the first translation value in the control points
    /// (three per control point), or [`NO_HANDLE`] (see [`no_handle`]).
    pub translation_handle: u32,
    /// Index of the first rotation value (four per control point, `w x y
    /// z`), or [`NO_HANDLE`].
    pub rotation_handle: u32,
    /// Index of the first scale value (one per control point), or
    /// [`NO_HANDLE`].
    pub scale_handle: u32,
    /// Present for the compressed type: the control points are the
    /// spline data's 16-bit values; otherwise they are its floats.
    pub compression: Option<BSplineCompression>,
}

/// `NiBSplineData`: control points for any number of interpolators.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiBSplineData {
    /// Uncompressed control point values.
    pub float_control_points: Vec<f32>,
    /// Compressed control point values.
    pub compact_control_points: Vec<i16>,
}

/// `NiBSplineBasisData`: the number of control points of an open uniform
/// cubic B-spline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NiBSplineBasisData {
    /// Control points per channel.
    pub num_control_points: u32,
}

/// One controlled block of a `NiControllerSequence`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ControlledBlock {
    /// The interpolator block.
    pub interpolator: Ref,
    /// The controller block (none in a `.kf`).
    pub controller: Ref,
    /// Blend priority.
    pub priority: u8,
    /// Name of the node the block animates.
    pub node_name: String,
    /// Property type, for property controllers.
    pub property_type: String,
    /// Controller type, for example `NiTransformController`.
    pub controller_type: String,
    /// Controller id.
    pub controller_id: String,
    /// Interpolator id.
    pub interpolator_id: String,
}

/// Cycle type 0: the sequence loops.
pub const CYCLE_LOOP: u32 = 0;
/// Cycle type 1: the sequence plays forward and back.
pub const CYCLE_REVERSE: u32 = 1;
/// Cycle type 2: the sequence plays once and holds its last frame.
pub const CYCLE_CLAMP: u32 = 2;

/// `NiControllerSequence`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiControllerSequence {
    /// Sequence name.
    pub name: String,
    /// What the sequence animates.
    pub controlled_blocks: Vec<ControlledBlock>,
    /// Array grow-by value.
    pub array_grow_by: u32,
    /// Blend weight.
    pub weight: f32,
    /// The `NiTextKeyExtraData` block.
    pub text_keys: Ref,
    /// One of the `CYCLE_*` constants.
    pub cycle_type: u32,
    /// Playback rate.
    pub frequency: f32,
    /// First time of the sequence, in seconds.
    pub start_time: f32,
    /// Last time of the sequence, in seconds.
    pub stop_time: f32,
    /// The controller manager (none in a `.kf`).
    pub manager: Ref,
    /// Name of the accumulation root node.
    pub accum_root_name: String,
    /// Animation note array blocks.
    pub anim_note_arrays: Vec<Ref>,
}

/// `NiTextKeyExtraData`: named times (`start`, `end`, sound cues, ...).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiTextKeyExtraData {
    /// Extra data name.
    pub name: String,
    /// `(time, text)` in file order.
    pub keys: Vec<(f32, String)>,
}

fn key_size(interpolation: u32, value_size: usize) -> Option<usize> {
    match interpolation {
        KEY_LINEAR | KEY_CONSTANT => Some(4 + value_size),
        KEY_QUADRATIC => Some(4 + 3 * value_size),
        KEY_TBC => Some(4 + value_size + 12),
        _ => None,
    }
}

fn key_group<'a, T: Copy + Default>(
    r: &mut Reader<'a>,
    what: &'static str,
    value_size: usize,
    read: fn(&mut Reader<'a>, &'static str) -> Result<T, NifError>,
) -> Result<KeyGroup<T>, NifError> {
    let count_at = r.offset();
    let count = r.u32(what)?;
    if count == 0 {
        return Ok(KeyGroup::default());
    }
    let at = r.offset();
    let interpolation = r.u32("key interpolation")?;
    let Some(size) = key_size(interpolation, value_size) else {
        return Err(r.bad_value(at, "key interpolation", i64::from(interpolation)));
    };
    let count = r.check_count(u64::from(count), size, what, count_at)?;
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        let mut key = Key {
            time: r.f32(what)?,
            value: read(r, what)?,
            ..Key::default()
        };
        match interpolation {
            KEY_QUADRATIC => {
                key.forward = read(r, what)?;
                key.backward = read(r, what)?;
            }
            KEY_TBC => key.tbc = r.vec3(what)?,
            _ => {}
        }
        keys.push(key);
    }
    Ok(KeyGroup {
        interpolation,
        keys,
    })
}

pub(crate) fn transform_data(r: &mut Reader<'_>) -> Result<NiTransformData, NifError> {
    let count_at = r.offset();
    let count = r.u32("rotation keys")?;
    let mut data = NiTransformData::default();
    if count != 0 {
        let at = r.offset();
        data.rotation_type = r.u32("rotation key type")?;
        match data.rotation_type {
            KEY_XYZ_ROTATION => {
                data.xyz_rotations = Some(Box::new([
                    key_group(r, "X rotation keys", 4, Reader::f32)?,
                    key_group(r, "Y rotation keys", 4, Reader::f32)?,
                    key_group(r, "Z rotation keys", 4, Reader::f32)?,
                ]));
            }
            KEY_LINEAR | KEY_QUADRATIC | KEY_CONSTANT | KEY_TBC => {
                let tbc = data.rotation_type == KEY_TBC;
                let size = if tbc { 32 } else { 20 };
                let count = r.check_count(u64::from(count), size, "rotation keys", count_at)?;
                data.quaternion_keys.reserve(count);
                for _ in 0..count {
                    data.quaternion_keys.push(QuatKey {
                        time: r.f32("rotation key")?,
                        value: r.vec4("rotation key")?,
                        tbc: if tbc {
                            r.vec3("rotation key")?
                        } else {
                            [0.0; 3]
                        },
                    });
                }
            }
            other => return Err(r.bad_value(at, "rotation key type", i64::from(other))),
        }
    }
    data.translations = key_group(r, "translation keys", 12, Reader::vec3)?;
    data.scales = key_group(r, "scale keys", 4, Reader::f32)?;
    Ok(data)
}

fn quat_transform(r: &mut Reader<'_>) -> Result<QuatTransform, NifError> {
    Ok(QuatTransform {
        translation: r.vec3("interpolator translation")?,
        rotation: r.vec4("interpolator rotation")?,
        scale: r.f32("interpolator scale")?,
    })
}

pub(crate) fn transform_interpolator(
    r: &mut Reader<'_>,
) -> Result<NiTransformInterpolator, NifError> {
    Ok(NiTransformInterpolator {
        transform: quat_transform(r)?,
        data: r.reference("transform data")?,
    })
}

pub(crate) fn bspline_transform_interpolator(
    r: &mut Reader<'_>,
    compressed: bool,
) -> Result<NiBSplineTransformInterpolator, NifError> {
    let start_time = r.f32("spline start time")?;
    let stop_time = r.f32("spline stop time")?;
    let spline_data = r.reference("spline data")?;
    let basis_data = r.reference("spline basis data")?;
    let transform = quat_transform(r)?;
    let translation_handle = r.u32("translation handle")?;
    let rotation_handle = r.u32("rotation handle")?;
    let scale_handle = r.u32("scale handle")?;
    let compression = if compressed {
        Some(BSplineCompression {
            translation_offset: r.f32("translation offset")?,
            translation_half_range: r.f32("translation half range")?,
            rotation_offset: r.f32("rotation offset")?,
            rotation_half_range: r.f32("rotation half range")?,
            scale_offset: r.f32("scale offset")?,
            scale_half_range: r.f32("scale half range")?,
        })
    } else {
        None
    };
    Ok(NiBSplineTransformInterpolator {
        start_time,
        stop_time,
        spline_data,
        basis_data,
        transform,
        translation_handle,
        rotation_handle,
        scale_handle,
        compression,
    })
}

pub(crate) fn bspline_data(r: &mut Reader<'_>) -> Result<NiBSplineData, NifError> {
    let count = r.count_u32(4, "float control points")?;
    let float_control_points = r.array(count, 4, "float control points", Reader::f32)?;
    let count = r.count_u32(2, "compact control points")?;
    let compact_control_points = r.array(count, 2, "compact control points", |r, what| {
        Ok(r.u16(what)? as i16)
    })?;
    Ok(NiBSplineData {
        float_control_points,
        compact_control_points,
    })
}

pub(crate) fn bspline_basis_data(r: &mut Reader<'_>) -> Result<NiBSplineBasisData, NifError> {
    Ok(NiBSplineBasisData {
        num_control_points: r.u32("control point count")?,
    })
}

/// Bytes of one controlled block: two references, a priority byte and five
/// string indices.
const CONTROLLED_BLOCK_SIZE: usize = 4 + 4 + 1 + 5 * 4;

pub(crate) fn controller_sequence(r: &mut Reader<'_>) -> Result<NiControllerSequence, NifError> {
    let name = r.string("sequence name")?;
    let count_at = r.offset();
    let count = r.u32("controlled blocks")?;
    let array_grow_by = r.u32("array grow by")?;
    let count = r.check_count(
        u64::from(count),
        CONTROLLED_BLOCK_SIZE,
        "controlled blocks",
        count_at,
    )?;
    let mut controlled_blocks = Vec::with_capacity(count);
    for _ in 0..count {
        controlled_blocks.push(ControlledBlock {
            interpolator: r.reference("controlled interpolator")?,
            controller: r.reference("controlled controller")?,
            priority: r.u8("controlled block priority")?,
            node_name: r.string("controlled node name")?,
            property_type: r.string("controlled property type")?,
            controller_type: r.string("controlled controller type")?,
            controller_id: r.string("controlled controller id")?,
            interpolator_id: r.string("controlled interpolator id")?,
        });
    }
    let weight = r.f32("sequence weight")?;
    let text_keys = r.reference("text keys")?;
    let cycle_type = r.u32("cycle type")?;
    let frequency = r.f32("frequency")?;
    let start_time = r.f32("start time")?;
    let stop_time = r.f32("stop time")?;
    let manager = r.reference("controller manager")?;
    let accum_root_name = r.string("accumulation root name")?;
    // Stream versions 24 to 28 store one note reference, later ones a
    // counted list, earlier ones nothing.
    let mut anim_note_arrays = Vec::new();
    match r.stream_version() {
        24..=28 => anim_note_arrays.push(r.reference("animation notes")?),
        29.. => {
            let notes = r.count_u16(4, "animation note arrays")?;
            anim_note_arrays.reserve(notes);
            for _ in 0..notes {
                anim_note_arrays.push(r.reference("animation note array")?);
            }
        }
        _ => {}
    }
    Ok(NiControllerSequence {
        name,
        controlled_blocks,
        array_grow_by,
        weight,
        text_keys,
        cycle_type,
        frequency,
        start_time,
        stop_time,
        manager,
        accum_root_name,
        anim_note_arrays,
    })
}

pub(crate) fn text_key_extra_data(r: &mut Reader<'_>) -> Result<NiTextKeyExtraData, NifError> {
    let name = r.string("extra data name")?;
    let count = r.count_u32(8, "text keys")?;
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        keys.push((r.f32("text key time")?, r.string("text key")?));
    }
    Ok(NiTextKeyExtraData { name, keys })
}
