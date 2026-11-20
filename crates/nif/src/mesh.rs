//! Scene flattening: from parsed blocks to one static mesh in source space.

use std::collections::BTreeMap;

use crate::blocks::{ANIMATION_TYPES, Block, NiGeometry, NiNode, Ref, SKIN_TYPES};
use crate::error::NifError;
use crate::skin::{Skin, build_skin};
use crate::{BlockEntry, NifFile};

/// A flattened mesh: every visible shape with its node transforms baked in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StaticMesh {
    /// One primitive per visible shape, in file (block index) order.
    pub primitives: Vec<Primitive>,
    /// Collision metadata, when the file has a collision object.
    pub collision: Option<CollisionInfo>,
    /// Block type name to the number of blocks of that type whose content
    /// does not reach the mesh: those skipped by size, every animation
    /// block, and skin blocks no primitive's skin uses.
    pub skipped_blocks: BTreeMap<String, u32>,
}

impl StaticMesh {
    /// Every non-empty texture path the primitives use, sorted, no repeats.
    pub fn texture_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .primitives
            .iter()
            .flat_map(|p| p.material.textures.iter())
            .filter(|t| !t.is_empty())
            .cloned()
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }
}

/// One shape: vertex data in source space (X east, Y north, Z up, game
/// units) and a triangle list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Primitive {
    /// The shape's name.
    pub name: String,
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals, one per position.
    pub normals: Option<Vec<[f32; 3]>>,
    /// Texture coordinates, one per position; `(0, 0)` is the image's top
    /// left corner.
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Vertex colours, RGBA, one per position.
    pub colors: Option<Vec<[f32; 4]>>,
    /// Triangle list, counter-clockwise front faces.
    pub indices: Vec<u32>,
    /// Material.
    pub material: Material,
    /// True exactly when [`Self::skin`] is `Some`. The positions and
    /// normals of a skinned primitive are in mesh space like those of any
    /// other: the stored vertices placed by the shape's own node chain,
    /// which is the bind pose (see [`mod@crate::skin`] for the spaces).
    pub skinned: bool,
    /// Bone weights and bind transforms, for a shape with a skin instance.
    pub skin: Option<Skin>,
}

impl Primitive {
    /// Whether the primitive can be written as a glTF primitive: at least
    /// one vertex and one triangle, every index in range, every optional
    /// attribute as long as `positions`.
    pub fn is_well_formed(&self) -> bool {
        let count = self.positions.len();
        count > 0
            && !self.indices.is_empty()
            && self.indices.len().is_multiple_of(3)
            && self.indices.iter().all(|&i| (i as usize) < count)
            && self.normals.as_ref().is_none_or(|v| v.len() == count)
            && self.uvs.as_ref().is_none_or(|v| v.len() == count)
            && self.colors.as_ref().is_none_or(|v| v.len() == count)
            && self.skin.as_ref().is_none_or(|s| s.is_well_formed(count))
    }
}

/// How a primitive's alpha channel is used.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AlphaMode {
    /// Alpha is ignored.
    #[default]
    Opaque,
    /// Alpha test: texels below `cutoff` (0 to 1) are discarded.
    Mask {
        /// The alpha property's threshold divided by 255.
        cutoff: f32,
    },
    /// Alpha blending.
    Blend,
}

/// The shader fields worth keeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShaderInfo {
    /// `true` for `BSShaderNoLightingProperty`.
    pub unlit: bool,
    /// Shader type as stored.
    pub shader_type: u32,
    /// First shader flag word as stored.
    pub flags: u32,
    /// Second shader flag word as stored.
    pub flags2: u32,
}

/// What a primitive is drawn with.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// Texture paths: lowercase, `\` separators, slot order (diffuse,
    /// normal, glow, parallax, environment, environment mask). An unused
    /// slot before a used one is an empty string; trailing unused slots are
    /// dropped.
    pub textures: Vec<String>,
    /// Alpha mode from the alpha property.
    pub alpha: AlphaMode,
    /// Both faces are drawn (stencil property draw mode 3).
    pub two_sided: bool,
    /// Shader type and flags, when the shape has a parsed shader property.
    pub shader: Option<ShaderInfo>,
    /// The alpha property's raw flags (blend functions live here).
    pub alpha_flags: Option<u16>,
    /// Material property alpha; 1 when there is no material property.
    pub opacity: f32,
    /// Material property emissive colour; black when there is none.
    pub emissive: [f32; 3],
    /// Material property emissive multiplier; 1 when there is none.
    pub emissive_multiple: f32,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            textures: Vec::new(),
            alpha: AlphaMode::Opaque,
            two_sided: false,
            shader: None,
            alpha_flags: None,
            opacity: 1.0,
            emissive: [0.0; 3],
            emissive_multiple: 1.0,
        }
    }
}

/// Collision metadata. No collision geometry is extracted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CollisionInfo {
    /// Block type of the collision object, for example `bhkCollisionObject`.
    pub kind: String,
    /// Name of the node the collision object targets.
    pub target: String,
    /// The collision object's flags.
    pub flags: u16,
    /// Block type of the body, empty when there is none.
    pub body: String,
    /// Collision layer peeked from the rigid body.
    pub layer: Option<u8>,
    /// Motion system peeked from the rigid body.
    pub motion_system: Option<u8>,
    /// How many collision objects the file has; the fields above describe
    /// the first in file order.
    pub count: u32,
}

/// Translation, rotation and uniform scale: `p' = R · p · scale + t`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Rows of the rotation matrix.
    pub rotation: [[f32; 3]; 3],
    /// Translation.
    pub translation: [f32; 3],
    /// Uniform scale.
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    /// The transform that changes nothing.
    pub const IDENTITY: Transform = Transform {
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        translation: [0.0; 3],
        scale: 1.0,
    };

    /// A pure translation.
    pub fn from_translation(translation: [f32; 3]) -> Self {
        Transform {
            translation,
            ..Self::IDENTITY
        }
    }

    /// Rotation rows for a counter-clockwise turn of `radians` about +Z
    /// (looking down from above, +X turns towards +Y).
    pub fn rotation_z(radians: f32) -> [[f32; 3]; 3] {
        let (s, c) = radians.sin_cos();
        [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
    }

    /// Rotates a vector, without scale or translation.
    pub fn rotate(&self, v: [f32; 3]) -> [f32; 3] {
        let r = &self.rotation;
        [
            r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
            r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
            r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
        ]
    }

    /// Transforms a point.
    pub fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let r = self.rotate(p);
        [
            r[0] * self.scale + self.translation[0],
            r[1] * self.scale + self.translation[1],
            r[2] * self.scale + self.translation[2],
        ]
    }

    /// Transforms a normal: rotation only, negated under a negative scale.
    pub fn apply_normal(&self, n: [f32; 3]) -> [f32; 3] {
        let r = self.rotate(n);
        if self.scale < 0.0 {
            [-r[0], -r[1], -r[2]]
        } else {
            r
        }
    }

    /// The transform that applies `child` first and `self` second: a
    /// parent's world transform composed with a child's local one.
    pub fn then(&self, child: &Transform) -> Transform {
        let a = &self.rotation;
        let b = &child.rotation;
        let mut rotation = [[0.0f32; 3]; 3];
        for (r, row) in rotation.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
            }
        }
        Transform {
            rotation,
            translation: self.apply(child.translation),
            scale: self.scale * child.scale,
        }
    }

    /// Whether the transform turns a right-handed frame into a left-handed
    /// one, so that triangle winding must be reversed.
    pub fn is_mirrored(&self) -> bool {
        let r = &self.rotation;
        let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
            - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
            + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
        det * self.scale < 0.0
    }

    /// The transform that undoes this one: `R⁻¹ = Rᵀ` (the rotation is
    /// taken to be orthonormal), scale `1 / s`, translation
    /// `-Rᵀ · t / s`. A zero scale gives non-finite values.
    pub fn inverse(&self) -> Transform {
        let r = &self.rotation;
        let rotation = [
            [r[0][0], r[1][0], r[2][0]],
            [r[0][1], r[1][1], r[2][1]],
            [r[0][2], r[1][2], r[2][2]],
        ];
        let scale = 1.0 / self.scale;
        let back = Transform {
            rotation,
            translation: [0.0; 3],
            scale,
        };
        let t = back.apply(self.translation);
        Transform {
            translation: [-t[0], -t[1], -t[2]],
            ..back
        }
    }

    /// Whether every component is finite.
    pub fn is_finite(&self) -> bool {
        self.rotation.iter().flatten().all(|v| v.is_finite())
            && self.translation.iter().all(|v| v.is_finite())
            && self.scale.is_finite()
    }
}

/// Choices for [`extract_mesh_with`]. The default honours the hidden flag
/// and leaves editor markers out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExtractOptions {
    /// Keep nodes and shapes whose hidden flag is set.
    pub include_hidden: bool,
    /// Keep nodes and shapes whose name starts with `EditorMarker`.
    pub include_editor_markers: bool,
}

/// The name prefix (compared without case) that marks editor-only geometry.
pub const EDITOR_MARKER_PREFIX: &str = "EditorMarker";

/// Lowercases a texture path (ASCII), turns `/` into `\` and trims
/// surrounding whitespace and NULs.
pub fn normalize_texture_path(raw: &str) -> String {
    raw.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0')
        .chars()
        .map(|c| match c {
            '/' => '\\',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// Expands triangle strips to a triangle list. Triangle `i` of a strip is
/// `(v[i], v[i+1], v[i+2])`, with the last two swapped when `i` is odd so
/// that winding stays consistent. Triangles that repeat a vertex are
/// dropped; they still count for the odd/even alternation.
pub fn expand_strips(strips: &[Vec<u16>]) -> Vec<[u16; 3]> {
    let mut out = Vec::new();
    for strip in strips {
        for (i, w) in strip.windows(3).enumerate() {
            let (a, b, c) = (w[0], w[1], w[2]);
            if a == b || b == c || a == c {
                continue;
            }
            out.push(if i % 2 == 0 { [a, b, c] } else { [a, c, b] });
        }
    }
    out
}

fn local_transform(av: &crate::blocks::AvObject) -> Transform {
    Transform {
        rotation: av.rotation,
        translation: av.translation,
        scale: av.scale,
    }
}

fn is_editor_marker(name: &str) -> bool {
    name.get(..EDITOR_MARKER_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(EDITOR_MARKER_PREFIX))
}

fn finite_or<const N: usize>(v: [f32; N], fallback: f32) -> [f32; N] {
    v.map(|x| if x.is_finite() { x } else { fallback })
}

fn scene_error(index: u32, entry: &BlockEntry, reason: String) -> NifError {
    NifError::Scene {
        index,
        type_name: entry.type_name.clone(),
        offset: entry.offset,
        reason,
    }
}

/// The property of each kind that applies to an object: its own, or else
/// the nearest ancestor's.
#[derive(Debug, Clone, Copy, Default)]
struct PropState {
    shader: Option<u32>,
    alpha: Option<u32>,
    material: Option<u32>,
    stencil: Option<u32>,
    texturing: Option<u32>,
}

struct Frame {
    node: u32,
    next: usize,
    end: usize,
    world: Transform,
    props: PropState,
}

struct Walker<'a> {
    file: &'a NifFile,
    options: ExtractOptions,
    stack: Vec<Frame>,
    on_path: Vec<bool>,
    visits: usize,
    budget: usize,
    out: Vec<(u32, Primitive)>,
    used_skin_blocks: Vec<u32>,
}

impl<'a> Walker<'a> {
    fn entry(&self, reference: Ref) -> Option<&'a BlockEntry> {
        self.file.blocks.get(reference? as usize)
    }

    fn skip(&self, av: &crate::blocks::AvObject) -> bool {
        (av.is_hidden() && !self.options.include_hidden)
            || (is_editor_marker(&av.net.name) && !self.options.include_editor_markers)
    }

    fn merge(&self, inherited: PropState, properties: &[Ref]) -> PropState {
        let mut own = PropState::default();
        for &reference in properties {
            let Some(index) = reference else { continue };
            let Some(entry) = self.entry(reference) else {
                continue;
            };
            let slot = match &entry.block {
                Block::ShaderPpLighting(_) | Block::ShaderNoLighting(_) => &mut own.shader,
                Block::AlphaProperty(_) => &mut own.alpha,
                Block::MaterialProperty(_) => &mut own.material,
                Block::StencilProperty(_) => &mut own.stencil,
                Block::TexturingProperty(_) => &mut own.texturing,
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some(index);
            }
        }
        PropState {
            shader: own.shader.or(inherited.shader),
            alpha: own.alpha.or(inherited.alpha),
            material: own.material.or(inherited.material),
            stencil: own.stencil.or(inherited.stencil),
            texturing: own.texturing.or(inherited.texturing),
        }
    }

    /// Visits one child reference: pushes a node, or emits a shape.
    fn enter(&mut self, index: u32, parent: &Transform, props: PropState) -> Result<(), NifError> {
        let Some(entry) = self.entry(Some(index)) else {
            return Ok(());
        };
        match &entry.block {
            Block::Node(node) => {
                self.count_visit(index, entry)?;
                if self.skip(&node.av) {
                    return Ok(());
                }
                if self.on_path[index as usize] {
                    return Err(scene_error(
                        index,
                        entry,
                        "the node is its own ancestor (cyclic scene graph)".to_owned(),
                    ));
                }
                let world = parent.then(&local_transform(&node.av));
                let props = self.merge(props, &node.av.properties);
                let (next, end) = child_range(&entry.type_name, node);
                self.on_path[index as usize] = true;
                self.stack.push(Frame {
                    node: index,
                    next,
                    end,
                    world,
                    props,
                });
                Ok(())
            }
            block => {
                let Some(geom) = block.as_shape() else {
                    return Ok(());
                };
                self.count_visit(index, entry)?;
                if self.skip(&geom.av) {
                    return Ok(());
                }
                self.emit(index, entry, geom, parent, props)
            }
        }
    }

    /// A scene graph is a tree, so each block is normally visited once. A
    /// block may legitimately be shared a few times; a file built to be
    /// visited exponentially often is stopped here.
    fn count_visit(&mut self, index: u32, entry: &BlockEntry) -> Result<(), NifError> {
        self.visits += 1;
        if self.visits > self.budget {
            return Err(scene_error(
                index,
                entry,
                format!(
                    "the scene graph shares nodes so heavily that walking it takes more than \
                     {} visits",
                    self.budget
                ),
            ));
        }
        Ok(())
    }

    fn walk(&mut self, root: u32) -> Result<(), NifError> {
        self.enter(root, &Transform::IDENTITY, PropState::default())?;
        while let Some(frame) = self.stack.last_mut() {
            if frame.next >= frame.end {
                let node = frame.node;
                self.stack.pop();
                self.on_path[node as usize] = false;
                continue;
            }
            let position = frame.next;
            frame.next += 1;
            let (node, world, props) = (frame.node, frame.world, frame.props);
            let child = match self.entry(Some(node)).map(|e| &e.block) {
                Some(Block::Node(n)) => n.children.get(position).copied().flatten(),
                _ => None,
            };
            if let Some(child) = child {
                self.enter(child, &world, props)?;
            }
        }
        Ok(())
    }

    fn emit(
        &mut self,
        index: u32,
        entry: &BlockEntry,
        geom: &NiGeometry,
        parent: &Transform,
        inherited: PropState,
    ) -> Result<(), NifError> {
        let world = parent.then(&local_transform(&geom.av));
        if !world.is_finite() {
            return Err(scene_error(
                index,
                entry,
                "the shape's world transform is not finite".to_owned(),
            ));
        }
        let Some(data_index) = geom.data else {
            return Ok(());
        };
        let Some(data_entry) = self.entry(geom.data) else {
            return Ok(());
        };
        let (data, mut triangles) = match &data_entry.block {
            Block::TriShapeData(d) => (&d.geom, d.triangles.clone().unwrap_or_default()),
            Block::TriStripsData(d) => (
                &d.geom,
                expand_strips(d.strips.as_deref().unwrap_or_default()),
            ),
            _ => return Ok(()),
        };
        let Some(vertices) = data.vertices.as_deref().filter(|v| !v.is_empty()) else {
            return Ok(());
        };
        let count = vertices.len();
        triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
        if let Some(bad) = triangles
            .iter()
            .flatten()
            .find(|&&i| usize::from(i) >= count)
        {
            return Err(scene_error(
                data_index,
                data_entry,
                format!("triangle index {bad} is past the vertex count {count}"),
            ));
        }
        if triangles.is_empty() {
            return Ok(());
        }

        let mut positions = Vec::with_capacity(count);
        for (i, &v) in vertices.iter().enumerate() {
            let p = world.apply(v);
            if !p.iter().all(|c| c.is_finite()) {
                return Err(scene_error(
                    data_index,
                    data_entry,
                    format!("vertex {i} has a non-finite position"),
                ));
            }
            positions.push(p);
        }
        // A normal with a non-finite component carries no direction; it
        // becomes the zero vector.
        let normals = data.normals.as_ref().filter(|n| n.len() == count).map(|n| {
            n.iter()
                .map(|&n| {
                    let n = world.apply_normal(n);
                    if n.iter().all(|c| c.is_finite()) {
                        n
                    } else {
                        [0.0; 3]
                    }
                })
                .collect()
        });
        let uvs = data
            .uv_sets
            .first()
            .filter(|uv| uv.len() == count)
            .map(|uv| uv.iter().map(|&uv| finite_or(uv, 0.0)).collect());
        let colors = data
            .colors
            .as_ref()
            .filter(|c| c.len() == count)
            .map(|c| c.iter().map(|&c| finite_or(c, 1.0)).collect());

        let mirrored = world.is_mirrored();
        let mut indices = Vec::with_capacity(triangles.len() * 3);
        for t in &triangles {
            let [a, b, c] = t.map(u32::from);
            let triangle = if mirrored { [a, c, b] } else { [a, b, c] };
            indices.extend_from_slice(&triangle);
        }

        let skin = match geom.skin_instance {
            Some(instance) => {
                self.used_skin_blocks.push(instance);
                Some(build_skin(self.file, instance, &world, count)?)
            }
            None => None,
        };
        let material = self.material(self.merge(inherited, &geom.av.properties));
        self.out.push((
            index,
            Primitive {
                name: geom.av.net.name.clone(),
                positions,
                normals,
                uvs,
                colors,
                indices,
                material,
                skinned: skin.is_some(),
                skin,
            },
        ));
        Ok(())
    }

    fn material(&self, props: PropState) -> Material {
        let mut material = Material::default();
        match self.entry(props.shader).map(|e| &e.block) {
            Some(Block::ShaderPpLighting(shader)) => {
                material.shader = Some(ShaderInfo {
                    unlit: false,
                    shader_type: shader.shader.shader_type,
                    flags: shader.shader.shader_flags,
                    flags2: shader.shader.shader_flags2,
                });
                if let Some(Block::ShaderTextureSet(set)) =
                    self.entry(shader.texture_set).map(|e| &e.block)
                {
                    material.textures = set
                        .textures
                        .iter()
                        .map(|t| normalize_texture_path(t))
                        .collect();
                }
            }
            Some(Block::ShaderNoLighting(shader)) => {
                material.shader = Some(ShaderInfo {
                    unlit: true,
                    shader_type: shader.shader.shader_type,
                    flags: shader.shader.shader_flags,
                    flags2: shader.shader.shader_flags2,
                });
                material.textures = vec![normalize_texture_path(&shader.file_name)];
            }
            _ => {}
        }
        while material.textures.last().is_some_and(|t| t.is_empty()) {
            material.textures.pop();
        }
        // Older-style materials name their texture through a texturing
        // property; its base texture is the diffuse slot when no shader
        // property supplied one.
        if material.textures.is_empty()
            && let Some(Block::TexturingProperty(texturing)) =
                self.entry(props.texturing).map(|e| &e.block)
            && let Some(Block::SourceTexture(source)) =
                self.entry(texturing.base_source).map(|e| &e.block)
        {
            let path = normalize_texture_path(&source.file_name);
            if !path.is_empty() {
                material.textures.push(path);
            }
        }
        if let Some(Block::AlphaProperty(alpha)) = self.entry(props.alpha).map(|e| &e.block) {
            material.alpha_flags = Some(alpha.flags);
            material.alpha = if alpha.test_enabled() {
                AlphaMode::Mask {
                    cutoff: f32::from(alpha.threshold) / 255.0,
                }
            } else if alpha.blend_enabled() {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            };
        }
        if let Some(Block::MaterialProperty(m)) = self.entry(props.material).map(|e| &e.block) {
            material.opacity = if m.alpha.is_finite() { m.alpha } else { 1.0 };
            material.emissive = finite_or(m.emissive, 0.0);
            material.emissive_multiple = if m.emissive_multiple.is_finite() {
                m.emissive_multiple
            } else {
                1.0
            };
        }
        if let Some(Block::StencilProperty(s)) = self.entry(props.stencil).map(|e| &e.block) {
            material.two_sided = s.two_sided();
        }
        material
    }
}

/// Which children of a node are walked. A plain node walks all of them. A
/// level-of-detail node walks only its first child (the nearest level); a
/// switch node walks only the child its stored index selects.
fn child_range(type_name: &str, node: &NiNode) -> (usize, usize) {
    let all = (0, node.children.len());
    match type_name {
        "NiLODNode" if !node.children.is_empty() => (0, 1),
        "NiSwitchNode" => {
            // After the node layout: a `u16` of flags, then the `u32` index.
            let Some(bytes) = node.extra.get(2..6) else {
                return all;
            };
            let index = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
            if index < node.children.len() {
                (index, index + 1)
            } else {
                all
            }
        }
        _ => all,
    }
}

fn collision_info(file: &NifFile) -> Option<CollisionInfo> {
    let mut found: Option<CollisionInfo> = None;
    let mut count = 0u32;
    for entry in &file.blocks {
        let Block::CollisionObject(object) = &entry.block else {
            continue;
        };
        count = count.saturating_add(1);
        if found.is_some() {
            continue;
        }
        let target = match file.block(object.target).map(|e| &e.block) {
            Some(Block::Node(node)) => node.av.net.name.clone(),
            Some(block) => block
                .as_shape()
                .map(|geom| geom.av.net.name.clone())
                .unwrap_or_default(),
            None => String::new(),
        };
        let body_entry = file.block(object.body);
        let (layer, motion_system) = match body_entry.map(|e| &e.block) {
            Some(Block::RigidBody(peek)) => (peek.layer, peek.motion_system),
            _ => (None, None),
        };
        found = Some(CollisionInfo {
            kind: entry.type_name.clone(),
            target,
            flags: object.flags,
            body: body_entry.map(|e| e.type_name.clone()).unwrap_or_default(),
            layer,
            motion_system,
            count: 0,
        });
    }
    found.map(|info| CollisionInfo { count, ..info })
}

/// Flattens a parsed file with the default [`ExtractOptions`].
pub fn extract_mesh(file: &NifFile) -> Result<StaticMesh, NifError> {
    extract_mesh_with(file, ExtractOptions::default())
}

/// Flattens a parsed file: walks the scene graph from the footer roots,
/// composes node transforms, bakes them into the vertex data and expands
/// strips.
pub fn extract_mesh_with(file: &NifFile, options: ExtractOptions) -> Result<StaticMesh, NifError> {
    let mut walker = Walker {
        file,
        options,
        stack: Vec::new(),
        on_path: vec![false; file.blocks.len()],
        visits: 0,
        budget: file.blocks.len().saturating_mul(4).saturating_add(64),
        out: Vec::new(),
        used_skin_blocks: Vec::new(),
    };
    for root in file.roots.iter().flatten() {
        walker.walk(*root)?;
    }
    let mut shapes = walker.out;
    // File order, whatever order the graph was walked in.
    shapes.sort_by_key(|(index, _)| *index);
    Ok(StaticMesh {
        primitives: shapes.into_iter().map(|(_, p)| p).collect(),
        collision: collision_info(file),
        skipped_blocks: unused_blocks(file, &walker.used_skin_blocks),
    })
}

/// The block counts a [`StaticMesh`] reports as skipped: blocks skipped by
/// size, animation blocks (a mesh never reads them), and skin blocks that
/// no emitted primitive's skin was built from. A file without a skinned
/// primitive therefore reports exactly what it reported before skins and
/// animations were parsed.
fn unused_blocks(file: &NifFile, skin_instances: &[u32]) -> BTreeMap<String, u32> {
    let mut used = vec![false; file.blocks.len()];
    for &instance in skin_instances {
        let mark = |used: &mut Vec<bool>, r: Ref| {
            if let Some(slot) = r.and_then(|i| used.get_mut(i as usize)) {
                *slot = true;
            }
        };
        mark(&mut used, Some(instance));
        if let Some(Block::SkinInstance(skin)) = file.block(Some(instance)).map(|e| &e.block) {
            mark(&mut used, skin.data);
            mark(&mut used, skin.partition);
        }
    }
    let mut counts = BTreeMap::new();
    for (i, entry) in file.blocks.iter().enumerate() {
        let name = entry.type_name.as_str();
        let unused = entry.block.is_skipped()
            || ANIMATION_TYPES.contains(&name)
            || (SKIN_TYPES.contains(&name) && !used[i]);
        if unused {
            *counts.entry(entry.type_name.clone()).or_insert(0u32) += 1;
        }
    }
    counts
}
