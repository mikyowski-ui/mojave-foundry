//! Skin weights and bind transforms, and linear blend skinning on the CPU.
//!
//! # Spaces
//!
//! - **Mesh space** is the space [`crate::extract_mesh`] writes every
//!   primitive in: the file's root space, X east, Y north, Z up, game
//!   units. A skinned primitive's positions and normals are its stored
//!   vertices placed by the shape's own node chain, exactly as for a static
//!   shape: the pose the file was saved in, which for the character meshes
//!   is the bind pose.
//! - **Bone space** of bone `i` is the bone node's own frame.
//! - [`SkinBone::bind_inverse`] maps a mesh-space point to bone space as
//!   the skin was bound: it is the skin data's skin-to-bone transform
//!   composed after the inverse of the shape's world transform.
//! - A [`crate::Pose`] gives each skeleton bone's transform from bone space
//!   to the skeleton's root space. Skinning maps a vertex to
//!   `Σ wᵢ · world(boneᵢ) · bind_inverseᵢ · v`, in the skeleton's root space.
//!   When the skeleton's rest pose is the bind pose and its root space is
//!   the mesh file's root space (true of the character files), skinning
//!   with the rest pose gives the stored positions back.

use serde::{Deserialize, Serialize};

use crate::blocks::Block;
use crate::error::NifError;
use crate::math::{
    IDENTITY34, Matrix34, apply_point, apply_vector, inverse, mul, transform_matrix,
};
use crate::mesh::{Primitive, StaticMesh, Transform};
use crate::pose::Pose;
use crate::skeleton::Skeleton;
use crate::skin_blocks::{NiSkinData, NiSkinPartition};
use crate::{BlockEntry, NifFile};

/// Influences kept per vertex.
pub const MAX_INFLUENCES: usize = 4;

/// One bone a skin is bound to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkinBone {
    /// The bone node's name; skinning finds the skeleton bone by it.
    pub name: String,
    /// Mesh space to this bone's space in the bind pose (see the module
    /// documentation), three rows of four.
    pub bind_inverse: Matrix34,
}

/// How a primitive's vertices follow bones.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Skin {
    /// The bones, in the skin instance's order.
    pub bones: Vec<SkinBone>,
    /// One row per vertex: up to four `(bone index, weight)` pairs, the
    /// heaviest first, weights summing to 1. Unused slots are `(0, 0.0)`.
    pub weights: Vec<[(u16, f32); MAX_INFLUENCES]>,
}

impl Skin {
    /// Whether the skin fits a primitive of `vertices` vertices: one row
    /// per vertex, every used bone index in range, every weight finite and
    /// not negative, and every row summing to 1 within `1e-4`.
    pub fn is_well_formed(&self, vertices: usize) -> bool {
        self.weights.len() == vertices
            && self.weights.iter().all(|row| {
                let mut sum = 0.0f32;
                for &(bone, weight) in row {
                    if !weight.is_finite() || weight < 0.0 {
                        return false;
                    }
                    if weight > 0.0 && usize::from(bone) >= self.bones.len() {
                        return false;
                    }
                    sum += weight;
                }
                (sum - 1.0).abs() <= 1e-4
            })
            && self
                .bones
                .iter()
                .all(|b| b.bind_inverse.iter().flatten().all(|v| v.is_finite()))
    }
}

fn scene_error(index: u32, entry: &BlockEntry, reason: impl Into<String>) -> NifError {
    NifError::Scene {
        index,
        type_name: entry.type_name.clone(),
        offset: entry.offset,
        reason: reason.into(),
    }
}

/// Collects influences, sorts them heaviest first (lower bone index first
/// on equal weight), keeps four and scales them to sum 1. A vertex with no
/// usable influence is bound to `fallback` alone.
fn finish_rows(rows: Vec<Vec<(u16, f32)>>, fallback: u16) -> Vec<[(u16, f32); MAX_INFLUENCES]> {
    rows.into_iter()
        .map(|mut influences| {
            influences.retain(|&(_, w)| w.is_finite() && w > 0.0);
            // Merge repeats of a bone.
            influences.sort_by_key(|&(bone, _)| bone);
            let mut merged: Vec<(u16, f32)> = Vec::with_capacity(influences.len());
            for (bone, weight) in influences {
                match merged.last_mut() {
                    Some(last) if last.0 == bone => last.1 += weight,
                    _ => merged.push((bone, weight)),
                }
            }
            merged.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            merged.truncate(MAX_INFLUENCES);
            let sum: f32 = merged.iter().map(|&(_, w)| w).sum();
            let mut row = [(0u16, 0.0f32); MAX_INFLUENCES];
            if merged.is_empty() || !sum.is_finite() || sum <= 0.0 {
                row[0] = (fallback, 1.0);
                return row;
            }
            for (slot, (bone, weight)) in row.iter_mut().zip(merged) {
                *slot = (bone, weight / sum);
            }
            // Make the row sum to 1 exactly up to rounding of one value.
            let rest: f32 = row[1..].iter().map(|&(_, w)| w).sum();
            row[0].1 = 1.0 - rest;
            row
        })
        .collect()
}

fn weights_from_data(data: &NiSkinData, vertices: usize) -> Option<Vec<Vec<(u16, f32)>>> {
    let mut rows = vec![Vec::new(); vertices];
    for (bone, entry) in data.bones.iter().enumerate() {
        for &(vertex, weight) in &entry.weights {
            rows.get_mut(usize::from(vertex))?
                .push((bone as u16, weight));
        }
    }
    Some(rows)
}

/// Weights from the partitions: a vertex that appears in several takes its
/// first appearance. `None` when an index is out of range.
fn weights_from_partition(
    partition: &NiSkinPartition,
    vertices: usize,
    bones: usize,
) -> Option<Vec<Vec<(u16, f32)>>> {
    let mut rows: Vec<Option<Vec<(u16, f32)>>> = vec![None; vertices];
    for part in &partition.partitions {
        let per = usize::from(part.weights_per_vertex);
        let (Some(weights), Some(indices)) = (&part.vertex_weights, &part.bone_indices) else {
            continue;
        };
        for j in 0..usize::from(part.num_vertices) {
            let vertex = match &part.vertex_map {
                Some(map) => usize::from(*map.get(j)?),
                None => j,
            };
            let slot = rows.get_mut(vertex)?;
            if slot.is_some() {
                continue;
            }
            let mut row = Vec::with_capacity(per);
            for k in 0..per {
                let weight = *weights.get(j * per + k)?;
                let local = usize::from(*indices.get(j * per + k)?);
                if weight == 0.0 {
                    continue;
                }
                let bone = *part.bones.get(local)?;
                if usize::from(bone) >= bones {
                    return None;
                }
                row.push((bone, weight));
            }
            *slot = Some(row);
        }
    }
    Some(rows.into_iter().map(Option::unwrap_or_default).collect())
}

/// Builds the skin of a shape whose skin instance is `instance`, whose
/// world transform is `world` and which has `vertices` vertices.
pub(crate) fn build_skin(
    file: &NifFile,
    instance: u32,
    world: &Transform,
    vertices: usize,
) -> Result<Skin, NifError> {
    let Some(entry) = file.blocks.get(instance as usize) else {
        return Err(NifError::Scene {
            index: instance,
            type_name: String::new(),
            offset: 0,
            reason: "the skin instance does not exist".to_owned(),
        });
    };
    let Block::SkinInstance(skin) = &entry.block else {
        return Err(scene_error(
            instance,
            entry,
            "the shape's skin instance is not a NiSkinInstance or BSDismemberSkinInstance",
        ));
    };
    if skin.bones.len() > usize::from(u16::MAX) {
        return Err(scene_error(
            instance,
            entry,
            "the skin has more than 65535 bones",
        ));
    }
    let Some(Block::SkinData(data)) = file.block(skin.data).map(|e| &e.block) else {
        return Err(scene_error(
            instance,
            entry,
            "the skin instance has no NiSkinData",
        ));
    };
    if data.bones.len() != skin.bones.len() {
        return Err(scene_error(
            instance,
            entry,
            format!(
                "the skin instance names {} bones but its skin data has {}",
                skin.bones.len(),
                data.bones.len()
            ),
        ));
    }
    let Some(shape_inverse) = inverse(&transform_matrix(world)) else {
        return Err(scene_error(
            instance,
            entry,
            "the skinned shape's world transform cannot be inverted",
        ));
    };
    let mut bones = Vec::with_capacity(skin.bones.len());
    for (i, (&bone, bone_data)) in skin.bones.iter().zip(&data.bones).enumerate() {
        let name = match file.block(bone).map(|e| &e.block) {
            Some(Block::Node(node)) => node.av.net.name.clone(),
            _ => {
                return Err(scene_error(
                    instance,
                    entry,
                    format!("skin bone {i} is not a node"),
                ));
            }
        };
        let bind_inverse = mul(&transform_matrix(&bone_data.skin_to_bone), &shape_inverse);
        if !bind_inverse.iter().flatten().all(|v| v.is_finite()) {
            return Err(scene_error(
                instance,
                entry,
                format!("skin bone {i} has a non-finite bind transform"),
            ));
        }
        bones.push(SkinBone { name, bind_inverse });
    }

    let partition = match file.block(skin.partition).map(|e| &e.block) {
        Some(Block::SkinPartition(p)) => Some(p.as_ref()),
        _ => None,
    };
    let rows = if data.has_vertex_weights {
        weights_from_data(data, vertices)
    } else if let Some(partition) = partition {
        weights_from_partition(partition, vertices, bones.len())
    } else {
        return Err(scene_error(
            instance,
            entry,
            "the skin stores no vertex weights and has no skin partition",
        ));
    };
    let Some(rows) = rows else {
        return Err(scene_error(
            instance,
            entry,
            "a skin weight names a vertex or bone that does not exist",
        ));
    };
    if bones.is_empty() {
        return Err(scene_error(instance, entry, "the skin has no bones"));
    }
    Ok(Skin {
        bones,
        weights: finish_rows(rows, 0),
    })
}

/// Number of vertices of a skin whose stored influences were all zero or
/// missing, so that skin extraction bound them to bone 0. For calibration.
pub fn unweighted_vertices(file: &NifFile, instance: u32, vertices: usize) -> Option<usize> {
    let Block::SkinInstance(skin) = &file.blocks.get(instance as usize)?.block else {
        return None;
    };
    let Some(Block::SkinData(data)) = file.block(skin.data).map(|e| &e.block) else {
        return None;
    };
    let rows = if data.has_vertex_weights {
        weights_from_data(data, vertices)?
    } else {
        let Some(Block::SkinPartition(p)) = file.block(skin.partition).map(|e| &e.block) else {
            return None;
        };
        weights_from_partition(p, vertices, skin.bones.len())?
    };
    Some(
        rows.iter()
            .filter(|row| !row.iter().any(|&(_, w)| w.is_finite() && w > 0.0))
            .count(),
    )
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len.is_finite() && len > 1e-20 {
        v.map(|c| c / len)
    } else {
        [0.0; 3]
    }
}

fn skin_primitive(
    primitive: &Primitive,
    skin: &Skin,
    skeleton: &Skeleton,
    pose: &Pose,
) -> Primitive {
    // Each skin bone's matrix from mesh space to posed root space. A skin
    // bone the skeleton lacks stays where it was bound.
    let matrices: Vec<Matrix34> = skin
        .bones
        .iter()
        .map(|bone| {
            skeleton
                .find(&bone.name)
                .and_then(|i| pose.world.get(i as usize))
                .map_or(IDENTITY34, |world| mul(world, &bone.bind_inverse))
        })
        .collect();
    let blend = |row: &[(u16, f32); MAX_INFLUENCES]| -> Matrix34 {
        let mut m = [[0.0f32; 4]; 3];
        let mut used = false;
        for &(bone, weight) in row {
            if weight <= 0.0 {
                continue;
            }
            let Some(b) = matrices.get(usize::from(bone)) else {
                continue;
            };
            used = true;
            for (out, src) in m.iter_mut().flatten().zip(b.iter().flatten()) {
                *out += src * weight;
            }
        }
        if used { m } else { IDENTITY34 }
    };
    let blended: Vec<Matrix34> = (0..primitive.positions.len())
        .map(|i| skin.weights.get(i).map_or(IDENTITY34, blend))
        .collect();
    let positions = primitive
        .positions
        .iter()
        .zip(&blended)
        .map(|(&p, m)| apply_point(m, p))
        .collect();
    let normals = primitive.normals.as_ref().map(|normals| {
        normals
            .iter()
            .zip(&blended)
            .map(|(&n, m)| normalize(apply_vector(m, n)))
            .collect()
    });
    Primitive {
        positions,
        normals,
        skin: None,
        skinned: false,
        ..primitive.clone()
    }
}

/// Poses a mesh: linear blend skinning of positions and normals on the
/// CPU. Each skinned primitive's vertices go to
/// `Σ wᵢ · pose.world[boneᵢ] · bind_inverseᵢ · v` (see the module
/// documentation), normals through the same blended matrix and then back
/// to unit length. Bones are found in the skeleton by name
/// ([`Skeleton::find`]); a skin bone the skeleton lacks keeps its bind
/// placement. The result has no skin and `skinned == false`; primitives
/// without a skin pass through unchanged.
pub fn skin(mesh: &StaticMesh, skeleton: &Skeleton, pose: &Pose) -> StaticMesh {
    StaticMesh {
        primitives: mesh
            .primitives
            .iter()
            .map(|p| match &p.skin {
                Some(skin) => skin_primitive(p, skin, skeleton, pose),
                None => p.clone(),
            })
            .collect(),
        ..mesh.clone()
    }
}
