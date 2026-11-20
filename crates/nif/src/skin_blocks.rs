//! Skin blocks: `NiSkinInstance`, `BSDismemberSkinInstance`, `NiSkinData`
//! and `NiSkinPartition`, for file version 20.2.0.7, user version 11,
//! stream version 34.

use crate::blocks::Ref;
use crate::error::NifError;
use crate::mesh::Transform;
use crate::reader::Reader;

/// One body part entry of a `BSDismemberSkinInstance`: one per partition
/// of the skin partition block, in the same order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BodyPart {
    /// Part flags.
    pub flags: u16,
    /// Body part (dismemberment slot) number.
    pub body_part: u16,
}

/// `NiSkinInstance` and `BSDismemberSkinInstance`: binds a shape to bones.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiSkinInstance {
    /// The `NiSkinData` block.
    pub data: Ref,
    /// The `NiSkinPartition` block.
    pub partition: Ref,
    /// The skeleton root node.
    pub skeleton_root: Ref,
    /// The bone nodes. Bone `i` of the skin data and bone index `i` of the
    /// partitions mean this node.
    pub bones: Vec<Ref>,
    /// The body part list; only a `BSDismemberSkinInstance` has one.
    pub body_parts: Vec<BodyPart>,
}

/// One bone of a `NiSkinData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkinDataBone {
    /// Maps the shape's own (skin) space to the bone's space in the bind
    /// pose: `p_bone = skin_to_bone.apply(p_skin)`.
    pub skin_to_bone: Transform,
    /// Bounding sphere centre in bone space.
    pub bound_center: [f32; 3],
    /// Bounding sphere radius.
    pub bound_radius: f32,
    /// Number of vertices the bone influences, as stored.
    pub num_vertices: u16,
    /// `(vertex index, weight)` pairs; empty when the skin data stores no
    /// weights.
    pub weights: Vec<(u16, f32)>,
}

/// `NiSkinData`: bind transforms and, usually, per-vertex weights.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiSkinData {
    /// The overall skin transform, as stored. Not used for posing.
    pub skin_transform: Transform,
    /// Whether the bones carry vertex weights.
    pub has_vertex_weights: bool,
    /// One entry per bone of the skin instance.
    pub bones: Vec<SkinDataBone>,
}

/// One partition of a `NiSkinPartition`: a subset of the shape's vertices
/// and triangles with at most a few bones each.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkinPartition {
    /// Vertex count of the partition.
    pub num_vertices: u16,
    /// Triangle count of the partition.
    pub num_triangles: u16,
    /// Bones of the partition, as indices into the skin instance's bones.
    pub bones: Vec<u16>,
    /// Weights (and bone indices) per vertex.
    pub weights_per_vertex: u16,
    /// Partition vertex to shape vertex, when stored.
    pub vertex_map: Option<Vec<u16>>,
    /// `num_vertices × weights_per_vertex` weights, vertex by vertex.
    pub vertex_weights: Option<Vec<f32>>,
    /// Strip lengths.
    pub strip_lengths: Vec<u16>,
    /// Strips, when faces are stored and the partition has strips.
    pub strips: Vec<Vec<u16>>,
    /// Triangles, when faces are stored and the partition has no strips.
    pub triangles: Option<Vec<[u16; 3]>>,
    /// `num_vertices × weights_per_vertex` indices into [`Self::bones`].
    pub bone_indices: Option<Vec<u8>>,
}

/// `NiSkinPartition`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiSkinPartition {
    /// The partitions.
    pub partitions: Vec<SkinPartition>,
}

/// Rotation (nine floats, rows), translation, scale: the order a skin
/// transform is stored in.
pub(crate) fn skin_transform(
    r: &mut Reader<'_>,
    what: &'static str,
) -> Result<Transform, NifError> {
    Ok(Transform {
        rotation: r.matrix33(what)?,
        translation: r.vec3(what)?,
        scale: r.f32(what)?,
    })
}

pub(crate) fn skin_instance(
    r: &mut Reader<'_>,
    dismember: bool,
) -> Result<NiSkinInstance, NifError> {
    let data = r.reference("skin data")?;
    let partition = r.reference("skin partition")?;
    let skeleton_root = r.reference("skeleton root")?;
    let bones = r.ref_array("bone list")?;
    let mut body_parts = Vec::new();
    if dismember {
        let count = r.count_u32(4, "body part list")?;
        body_parts.reserve(count);
        for _ in 0..count {
            body_parts.push(BodyPart {
                flags: r.u16("body part flags")?,
                body_part: r.u16("body part")?,
            });
        }
    }
    Ok(NiSkinInstance {
        data,
        partition,
        skeleton_root,
        bones,
        body_parts,
    })
}

/// Size of a stored bone without weights: transform, sphere, count.
const SKIN_BONE_SIZE: usize = 52 + 16 + 2;

pub(crate) fn skin_data(r: &mut Reader<'_>) -> Result<NiSkinData, NifError> {
    let overall = skin_transform(r, "skin transform")?;
    let at = r.offset();
    let num_bones = r.u32("skin bone count")?;
    let has_vertex_weights = r.bool("has vertex weights")?;
    let num_bones = r.check_count(u64::from(num_bones), SKIN_BONE_SIZE, "skin bone", at)?;
    let mut bones = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        let skin_to_bone = skin_transform(r, "bone skin transform")?;
        let bound_center = r.vec3("bone bounding sphere")?;
        let bound_radius = r.f32("bone bounding sphere")?;
        let num_vertices = r.u16("bone vertex count")?;
        let weights = if has_vertex_weights {
            r.array(
                usize::from(num_vertices),
                6,
                "bone vertex weight",
                |r, what| Ok((r.u16(what)?, r.f32(what)?)),
            )?
        } else {
            Vec::new()
        };
        bones.push(SkinDataBone {
            skin_to_bone,
            bound_center,
            bound_radius,
            num_vertices,
            weights,
        });
    }
    Ok(NiSkinData {
        skin_transform: overall,
        has_vertex_weights,
        bones,
    })
}

/// Bytes of a partition before its arrays: five counts and three flags.
const PARTITION_MIN_SIZE: usize = 10 + 3;

pub(crate) fn skin_partition(r: &mut Reader<'_>) -> Result<NiSkinPartition, NifError> {
    let count = r.count_u32(PARTITION_MIN_SIZE, "skin partition")?;
    let mut partitions = Vec::with_capacity(count);
    for _ in 0..count {
        let num_vertices = r.u16("partition vertex count")?;
        let num_triangles = r.u16("partition triangle count")?;
        let num_bones = r.u16("partition bone count")?;
        let num_strips = r.u16("partition strip count")?;
        let weights_per_vertex = r.u16("partition weights per vertex")?;
        let n = usize::from(num_vertices);
        let per_vertex = n * usize::from(weights_per_vertex);
        let bones = r.u16_array(usize::from(num_bones), "partition bones")?;
        let vertex_map = if r.bool("partition has vertex map")? {
            Some(r.u16_array(n, "partition vertex map")?)
        } else {
            None
        };
        let vertex_weights = if r.bool("partition has vertex weights")? {
            Some(r.array(per_vertex, 4, "partition vertex weights", Reader::f32)?)
        } else {
            None
        };
        let strip_lengths = r.u16_array(usize::from(num_strips), "partition strip lengths")?;
        let has_faces = r.bool("partition has faces")?;
        let mut strips = Vec::new();
        let mut triangles = None;
        if has_faces {
            if num_strips != 0 {
                strips.reserve(strip_lengths.len());
                for &len in &strip_lengths {
                    strips.push(r.u16_array(usize::from(len), "partition strip")?);
                }
            } else {
                triangles = Some(r.array(
                    usize::from(num_triangles),
                    6,
                    "partition triangles",
                    |r, what| Ok([r.u16(what)?, r.u16(what)?, r.u16(what)?]),
                )?);
            }
        }
        let bone_indices = if r.bool("partition has bone indices")? {
            Some(r.array(per_vertex, 1, "partition bone indices", Reader::u8)?)
        } else {
            None
        };
        partitions.push(SkinPartition {
            num_vertices,
            num_triangles,
            bones,
            weights_per_vertex,
            vertex_map,
            vertex_weights,
            strip_lengths,
            strips,
            triangles,
            bone_indices,
        });
    }
    Ok(NiSkinPartition { partitions })
}
