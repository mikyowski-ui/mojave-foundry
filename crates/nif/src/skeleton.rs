//! The bone tree of a NIF file.

use serde::{Deserialize, Serialize};

use crate::NifFile;
use crate::blocks::Block;
use crate::error::NifError;
use crate::math::{Matrix34, mul, transform_matrix};
use crate::mesh::Transform;

/// A node's transform relative to its parent: `p' = R · p · scale + t`,
/// with `rotation[r]` the rows (see [`crate::Transform`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoneTransform {
    /// Rows of the rotation matrix.
    pub rotation: [[f32; 3]; 3],
    /// Translation, in the parent's units.
    pub translation: [f32; 3],
    /// Uniform scale.
    pub scale: f32,
}

impl Default for BoneTransform {
    fn default() -> Self {
        Transform::IDENTITY.into()
    }
}

impl From<Transform> for BoneTransform {
    fn from(t: Transform) -> Self {
        BoneTransform {
            rotation: t.rotation,
            translation: t.translation,
            scale: t.scale,
        }
    }
}

impl From<BoneTransform> for Transform {
    fn from(t: BoneTransform) -> Self {
        Transform {
            rotation: t.rotation,
            translation: t.translation,
            scale: t.scale,
        }
    }
}

impl BoneTransform {
    /// The transform as a [`Matrix34`].
    pub fn matrix(&self) -> Matrix34 {
        transform_matrix(&(*self).into())
    }
}

/// One node of the tree.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Bone {
    /// Node name as stored.
    pub name: String,
    /// Index of the parent bone; `None` for a root. Always smaller than the
    /// bone's own index.
    pub parent: Option<u32>,
    /// Transform relative to the parent (to the file's root space for a
    /// root).
    pub local: BoneTransform,
}

/// The node tree of a file: every node reachable from the footer roots,
/// shapes left out, in depth-first order with children in stored order, so
/// that a parent always precedes its children.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Skeleton {
    /// The bones; bone 0 is the first root node.
    pub bones: Vec<Bone>,
}

impl Skeleton {
    /// The index of the bone called `name`: an exact match, or else the
    /// first bone whose name matches ignoring ASCII case.
    pub fn find(&self, name: &str) -> Option<u32> {
        self.bones
            .iter()
            .position(|b| b.name == name)
            .or_else(|| {
                self.bones
                    .iter()
                    .position(|b| b.name.eq_ignore_ascii_case(name))
            })
            .map(|i| i as u32)
    }

    /// Each bone's transform to the root space, composed from the local
    /// transforms; `locals[i]` replaces bone `i`'s own local transform.
    pub(crate) fn compose(&self, locals: &[Matrix34]) -> Vec<Matrix34> {
        let mut world: Vec<Matrix34> = Vec::with_capacity(self.bones.len());
        for (i, bone) in self.bones.iter().enumerate() {
            let local = locals[i];
            let m = match bone.parent.and_then(|p| world.get(p as usize)) {
                Some(parent) => mul(parent, &local),
                None => local,
            };
            world.push(m);
        }
        world
    }

    /// Each bone's transform to the root space in the skeleton's own rest
    /// pose.
    pub fn world_matrices(&self) -> Vec<Matrix34> {
        let locals: Vec<Matrix34> = self.bones.iter().map(|b| b.local.matrix()).collect();
        self.compose(&locals)
    }

    /// The number of bones on the longest root-to-leaf path (a lone root
    /// has depth 1, an empty skeleton 0).
    pub fn depth(&self) -> usize {
        let mut depths: Vec<usize> = Vec::with_capacity(self.bones.len());
        for bone in &self.bones {
            let d = bone
                .parent
                .and_then(|p| depths.get(p as usize))
                .map_or(1, |d| d + 1);
            depths.push(d);
        }
        depths.into_iter().max().unwrap_or(0)
    }

    /// Whether every parent index points at an earlier bone.
    pub fn is_well_formed(&self) -> bool {
        self.bones
            .iter()
            .enumerate()
            .all(|(i, b)| b.parent.is_none_or(|p| (p as usize) < i))
    }
}

/// Extracts the node tree of a parsed file (see [`Skeleton`]). Hidden
/// nodes are kept: a bone is a bone whether or not it is drawn. A node
/// reachable along two paths appears once, under the first parent in
/// depth-first order; a node that is its own ancestor is an error.
pub fn extract_skeleton(file: &NifFile) -> Result<Skeleton, NifError> {
    let count = file.blocks.len();
    let mut seen = vec![false; count];
    let mut on_path = vec![false; count];
    let mut bones = Vec::new();
    // (block, parent bone, next child position)
    let mut stack: Vec<(u32, Option<u32>, usize)> = Vec::new();

    for &root in file.roots.iter().flatten() {
        let mut pending = Some((root, None));
        loop {
            if let Some((index, parent)) = pending.take() {
                let Some(entry) = file.blocks.get(index as usize) else {
                    continue;
                };
                if let Block::Node(node) = &entry.block {
                    if on_path[index as usize] {
                        return Err(NifError::Scene {
                            index,
                            type_name: entry.type_name.clone(),
                            offset: entry.offset,
                            reason: "the node is its own ancestor (cyclic scene graph)".to_owned(),
                        });
                    }
                    if !seen[index as usize] {
                        seen[index as usize] = true;
                        on_path[index as usize] = true;
                        let av = &node.av;
                        bones.push(Bone {
                            name: av.net.name.clone(),
                            parent,
                            local: BoneTransform {
                                rotation: av.rotation,
                                translation: av.translation,
                                scale: av.scale,
                            },
                        });
                        stack.push((index, Some((bones.len() - 1) as u32), 0));
                    }
                }
            }
            let Some(top) = stack.last_mut() else { break };
            let (node_index, bone, position) = *top;
            top.2 += 1;
            let child = match file.blocks.get(node_index as usize).map(|e| &e.block) {
                Some(Block::Node(node)) if position < node.children.len() => {
                    Some(node.children[position])
                }
                _ => None,
            };
            match child {
                Some(Some(child)) => pending = Some((child, bone)),
                Some(None) => {}
                None => {
                    stack.pop();
                    on_path[node_index as usize] = false;
                }
            }
        }
    }
    Ok(Skeleton { bones })
}
