//! Synthetic NIF files, built in memory.
//!
//! Tests in this crate and in later crates get their meshes from here; no
//! test reads game data. [`NifBuilder`] writes version 20.2.0.7 / 11 / 34
//! files with a correct block type table, block sizes and string table.
//!
//! There are two levels:
//!
//! - **blocks**: [`NifBuilder::push`] encodes one [`Block`] structure, the
//!   same structures the parser produces, so a test can compare what it
//!   wrote with what was parsed, or wire up a malformed graph by hand;
//! - **scenes**: [`NodeDesc`] and [`ShapeDesc`] describe a tree of nodes and
//!   shapes, and [`NifBuilder::add_scene`] turns it into blocks. The
//!   helpers [`box_mesh`], [`quad`] and [`building`] are scenes of that
//!   kind;
//! - **characters**: [`skeleton_nif`] (six bones laid out like the human
//!   skeleton), [`skinned_body`] (a shape skinned to three of them) and
//!   [`walk_kf`] (a one-second walk with plain-key and B-spline tracks);
//!   [`KfDesc`] writes other `.kf` files.

use crate::anim_blocks::{
    BSplineCompression, CYCLE_LOOP, ControlledBlock, KEY_LINEAR, KEY_QUADRATIC, KEY_TBC,
    KEY_XYZ_ROTATION, Key, KeyGroup, NO_HANDLE, NiBSplineBasisData, NiBSplineData,
    NiBSplineTransformInterpolator, NiControllerSequence, NiTextKeyExtraData, NiTransformData,
    NiTransformInterpolator, QuatKey, QuatTransform,
};
use crate::blocks::{
    AvObject, Block, BsShaderPpLighting, BsShaderTextureSet, BsxFlags, CollisionObject,
    FLAG_HIDDEN, GeometryData, NiAlphaProperty, NiGeometry, NiMaterialProperty, NiNode,
    NiStencilProperty, NiTriShapeData, NiTriStripsData, ObjectNet, RIGID_BODY_LAYER_OFFSET,
    RIGID_BODY_MOTION_OFFSET, Ref, ShaderProperty, VECTOR_FLAGS_UV,
};
use crate::header::{HEADER_LINE, STREAM_VERSION, USER_VERSION, VERSION};
use crate::mesh::Transform;
use crate::skin_blocks::{
    BodyPart, NiSkinData, NiSkinInstance, NiSkinPartition, SkinDataBone, SkinPartition,
};

/// Size in bytes of the synthetic rigid body block the scene builder writes.
pub const RIGID_BODY_SIZE: usize = 236;
/// The stencil flags the scene builder writes for a two-sided shape: draw
/// mode 3 (both faces).
pub const STENCIL_FLAGS_TWO_SIDED: u16 = 0x4D80;
/// Alpha flags with blending and testing both enabled (source alpha,
/// inverse source alpha, test "greater").
pub const ALPHA_FLAGS_BLEND_AND_TEST: u16 = 0x12ED;
/// Alpha flags with only testing enabled.
pub const ALPHA_FLAGS_TEST: u16 = 0x12EC;
/// Alpha flags with only blending enabled.
pub const ALPHA_FLAGS_BLEND: u16 = 0x10ED;
/// The tangent bit the scene builder sets in a geometry's vector flags.
pub const VECTOR_FLAG_TANGENT_BIT: u16 = 0x1000;

/// Little-endian byte sink.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn floats(&mut self, values: &[f32]) {
        for &v in values {
            self.f32(v);
        }
    }
    fn u16s(&mut self, values: &[u16]) {
        for &v in values {
            self.u16(v);
        }
    }
    fn len32(&mut self, len: usize) {
        self.u32(u32::try_from(len).unwrap_or(u32::MAX));
    }
    fn reference(&mut self, r: Ref) {
        match r {
            Some(index) => self.u32(index),
            None => self.i32(-1),
        }
    }
    fn refs(&mut self, refs: &[Ref]) {
        self.len32(refs.len());
        for &r in refs {
            self.reference(r);
        }
    }
    fn sized_string(&mut self, s: &str) {
        let bytes = latin1_bytes(s);
        self.len32(bytes.len());
        self.0.extend_from_slice(&bytes);
    }
    fn short_string(&mut self, s: &str) {
        let mut bytes = latin1_bytes(s);
        bytes.truncate(254);
        bytes.push(0);
        self.u8(bytes.len() as u8);
        self.0.extend_from_slice(&bytes);
    }
}

fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| u8::try_from(c as u32).unwrap_or(b'?'))
        .collect()
}

struct RawBlock {
    type_name: String,
    payload: Vec<u8>,
    declared_size: Option<u32>,
}

/// Writes synthetic NIF files.
pub struct NifBuilder {
    export_strings: [String; 3],
    strings: Vec<String>,
    blocks: Vec<RawBlock>,
    roots: Vec<Ref>,
    groups: Vec<u32>,
    stream_version: u32,
}

impl Default for NifBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl NifBuilder {
    /// An empty file: no blocks, no roots.
    pub fn new() -> Self {
        NifBuilder {
            export_strings: [
                "mojave-foundry".to_owned(),
                "synthetic".to_owned(),
                "fixture".to_owned(),
            ],
            strings: Vec::new(),
            blocks: Vec::new(),
            roots: Vec::new(),
            groups: Vec::new(),
            stream_version: STREAM_VERSION,
        }
    }

    /// Sets the stream version the header states, for files of the older
    /// animation stream versions. Blocks pushed afterwards are encoded for
    /// it (only `NiControllerSequence` differs).
    pub fn stream_version(&mut self, stream_version: u32) -> &mut Self {
        self.stream_version = stream_version;
        self
    }

    /// Sets the three export strings of the header.
    pub fn export_strings(&mut self, author: &str, process: &str, export: &str) -> &mut Self {
        self.export_strings = [author.to_owned(), process.to_owned(), export.to_owned()];
        self
    }

    /// Sets the header's group list.
    pub fn groups(&mut self, groups: &[u32]) -> &mut Self {
        self.groups = groups.to_vec();
        self
    }

    /// Number of blocks so far; also the index the next block will get.
    pub fn len(&self) -> u32 {
        self.blocks.len() as u32
    }

    /// Whether no block has been added.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Returns the string table index of `s`, adding it when new. The empty
    /// string is "no string" (`0xFFFFFFFF`).
    pub fn intern(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return u32::MAX;
        }
        if let Some(index) = self.strings.iter().position(|existing| existing == s) {
            return index as u32;
        }
        self.strings.push(s.to_owned());
        (self.strings.len() - 1) as u32
    }

    /// Reserves a block index to fill in later with [`Self::set`], so that
    /// a parent can come before the children it refers to. Until then the
    /// slot is an empty block of the unknown type `Unset`.
    pub fn reserve(&mut self) -> u32 {
        self.push_raw("Unset", Vec::new())
    }

    /// Appends a block with the given payload bytes and returns its index.
    /// With a type name the parser does not know, this is how a test adds
    /// an unknown block.
    pub fn push_raw(&mut self, type_name: &str, payload: Vec<u8>) -> u32 {
        self.blocks.push(RawBlock {
            type_name: type_name.to_owned(),
            payload,
            declared_size: None,
        });
        (self.blocks.len() - 1) as u32
    }

    /// Replaces a block's type and payload. Does nothing for an index that
    /// was never handed out.
    pub fn set_raw(&mut self, index: u32, type_name: &str, payload: Vec<u8>) {
        if let Some(block) = self.blocks.get_mut(index as usize) {
            block.type_name = type_name.to_owned();
            block.payload = payload;
        }
    }

    /// Encodes `block` and appends it under `type_name`.
    pub fn push(&mut self, type_name: &str, block: &Block) -> u32 {
        let payload = self.encode(block);
        self.push_raw(type_name, payload)
    }

    /// Encodes `block` into a reserved slot.
    pub fn set(&mut self, index: u32, type_name: &str, block: &Block) {
        let payload = self.encode(block);
        self.set_raw(index, type_name, payload);
    }

    /// Makes the header declare `size` bytes for a block whatever its
    /// payload length, to build files with a wrong block size. The payload
    /// itself is written unchanged.
    pub fn declare_size(&mut self, index: u32, size: u32) {
        if let Some(block) = self.blocks.get_mut(index as usize) {
            block.declared_size = Some(size);
        }
    }

    /// The payload bytes of a block, for tests that corrupt them.
    pub fn payload_mut(&mut self, index: u32) -> Option<&mut Vec<u8>> {
        self.blocks.get_mut(index as usize).map(|b| &mut b.payload)
    }

    /// Adds a footer root.
    pub fn add_root(&mut self, root: Ref) -> &mut Self {
        self.roots.push(root);
        self
    }

    /// Encodes one block structure as its payload bytes, interning names.
    /// [`Block::Skipped`] and [`Block::RigidBody`] have no encoding and
    /// give an empty payload; use [`Self::push_raw`] or [`rigid_body_payload`].
    pub fn encode(&mut self, block: &Block) -> Vec<u8> {
        let mut o = Out::default();
        match block {
            Block::Node(node) => {
                self.av_object(&mut o, &node.av);
                o.refs(&node.children);
                o.refs(&node.effects);
                o.0.extend_from_slice(&node.extra);
            }
            Block::TriShape(geom) | Block::TriStrips(geom) => self.geometry(&mut o, geom),
            Block::SegmentedTriShape(segmented) => {
                self.geometry(&mut o, &segmented.shape);
                o.len32(segmented.segments.len());
                for segment in &segmented.segments {
                    o.u8(segment.flags);
                    o.u32(segment.start);
                    o.u32(segment.triangles);
                }
            }
            Block::TexturingProperty(texturing) => {
                self.object_net(&mut o, &texturing.net);
                o.u16(texturing.flags);
                o.u32(texturing.texture_count);
                o.u8(u8::from(texturing.has_base_texture));
                if texturing.has_base_texture {
                    o.reference(texturing.base_source);
                }
                o.0.extend_from_slice(&texturing.extra);
            }
            Block::SourceTexture(texture) => {
                self.object_net(&mut o, &texture.net);
                o.u8(texture.use_external);
                let name = self.intern(&texture.file_name);
                o.u32(name);
                o.reference(texture.link);
                o.u32(texture.pixel_layout);
                o.u32(texture.use_mipmaps);
                o.u32(texture.alpha_format);
                o.u8(texture.is_static);
                o.u8(texture.direct_render);
                o.u8(texture.persist_render_data);
            }
            Block::TriShapeData(data) => {
                geometry_data(&mut o, &data.geom);
                o.u16(data.num_triangles);
                o.u32(data.num_triangle_points);
                o.u8(u8::from(data.triangles.is_some()));
                for triangle in data.triangles.iter().flatten() {
                    o.u16s(triangle);
                }
                o.u16(data.match_groups.len() as u16);
                for group in &data.match_groups {
                    o.u16(group.len() as u16);
                    o.u16s(group);
                }
            }
            Block::TriStripsData(data) => {
                geometry_data(&mut o, &data.geom);
                o.u16(data.num_triangles);
                o.u16(data.strip_lengths.len() as u16);
                o.u16s(&data.strip_lengths);
                o.u8(u8::from(data.strips.is_some()));
                for strip in data.strips.iter().flatten() {
                    o.u16s(strip);
                }
            }
            Block::ShaderPpLighting(shader) => {
                self.shader_property(&mut o, &shader.shader);
                o.reference(shader.texture_set);
                o.f32(shader.refraction_strength);
                o.i32(shader.refraction_fire_period);
                o.f32(shader.parallax_max_passes);
                o.f32(shader.parallax_scale);
            }
            Block::ShaderNoLighting(shader) => {
                self.shader_property(&mut o, &shader.shader);
                o.sized_string(&shader.file_name);
                o.0.extend_from_slice(&shader.extra);
            }
            Block::ShaderTextureSet(set) => {
                o.len32(set.textures.len());
                for texture in &set.textures {
                    o.sized_string(texture);
                }
            }
            Block::AlphaProperty(alpha) => {
                self.object_net(&mut o, &alpha.net);
                o.u16(alpha.flags);
                o.u8(alpha.threshold);
            }
            Block::MaterialProperty(material) => {
                self.object_net(&mut o, &material.net);
                o.floats(&material.specular);
                o.floats(&material.emissive);
                o.f32(material.glossiness);
                o.f32(material.alpha);
                o.f32(material.emissive_multiple);
            }
            Block::StencilProperty(stencil) => {
                self.object_net(&mut o, &stencil.net);
                o.u16(stencil.flags);
                o.u32(stencil.stencil_ref);
                o.u32(stencil.stencil_mask);
            }
            Block::BsxFlags(bsx) => {
                let name = self.intern(&bsx.name);
                o.u32(name);
                o.u32(bsx.flags);
            }
            Block::CollisionObject(object) => {
                o.reference(object.target);
                o.u16(object.flags);
                o.reference(object.body);
                o.0.extend_from_slice(&object.extra);
            }
            Block::RigidBody(_) | Block::Skipped => {}
            Block::SkinInstance(skin) => {
                o.reference(skin.data);
                o.reference(skin.partition);
                o.reference(skin.skeleton_root);
                o.refs(&skin.bones);
                // A dismember instance is told apart by its type name, which
                // the caller chooses; the body part list is written when it
                // is not empty.
                if !skin.body_parts.is_empty() {
                    o.len32(skin.body_parts.len());
                    for part in &skin.body_parts {
                        o.u16(part.flags);
                        o.u16(part.body_part);
                    }
                }
            }
            Block::SkinData(data) => {
                skin_transform(&mut o, &data.skin_transform);
                o.len32(data.bones.len());
                o.u8(u8::from(data.has_vertex_weights));
                for bone in &data.bones {
                    skin_transform(&mut o, &bone.skin_to_bone);
                    o.floats(&bone.bound_center);
                    o.f32(bone.bound_radius);
                    o.u16(bone.num_vertices);
                    if data.has_vertex_weights {
                        for &(vertex, weight) in &bone.weights {
                            o.u16(vertex);
                            o.f32(weight);
                        }
                    }
                }
            }
            Block::SkinPartition(partition) => {
                o.len32(partition.partitions.len());
                for p in &partition.partitions {
                    o.u16(p.num_vertices);
                    o.u16(p.num_triangles);
                    o.u16(p.bones.len() as u16);
                    o.u16(p.strip_lengths.len() as u16);
                    o.u16(p.weights_per_vertex);
                    o.u16s(&p.bones);
                    o.u8(u8::from(p.vertex_map.is_some()));
                    o.u16s(p.vertex_map.as_deref().unwrap_or_default());
                    o.u8(u8::from(p.vertex_weights.is_some()));
                    o.floats(p.vertex_weights.as_deref().unwrap_or_default());
                    o.u16s(&p.strip_lengths);
                    let has_faces = !p.strips.is_empty() || p.triangles.is_some();
                    o.u8(u8::from(has_faces));
                    for strip in &p.strips {
                        o.u16s(strip);
                    }
                    for triangle in p.triangles.iter().flatten() {
                        o.u16s(triangle);
                    }
                    o.u8(u8::from(p.bone_indices.is_some()));
                    o.0.extend_from_slice(p.bone_indices.as_deref().unwrap_or_default());
                }
            }
            Block::ControllerSequence(sequence) => {
                let name = self.intern(&sequence.name);
                o.u32(name);
                o.len32(sequence.controlled_blocks.len());
                o.u32(sequence.array_grow_by);
                for block in &sequence.controlled_blocks {
                    o.reference(block.interpolator);
                    o.reference(block.controller);
                    o.u8(block.priority);
                    for text in [
                        &block.node_name,
                        &block.property_type,
                        &block.controller_type,
                        &block.controller_id,
                        &block.interpolator_id,
                    ] {
                        let index = self.intern(text);
                        o.u32(index);
                    }
                }
                o.f32(sequence.weight);
                o.reference(sequence.text_keys);
                o.u32(sequence.cycle_type);
                o.f32(sequence.frequency);
                o.f32(sequence.start_time);
                o.f32(sequence.stop_time);
                o.reference(sequence.manager);
                let accum = self.intern(&sequence.accum_root_name);
                o.u32(accum);
                match self.stream_version {
                    24..=28 => o.reference(sequence.anim_note_arrays.first().copied().flatten()),
                    29.. => {
                        o.u16(sequence.anim_note_arrays.len() as u16);
                        for &note in &sequence.anim_note_arrays {
                            o.reference(note);
                        }
                    }
                    _ => {}
                }
            }
            Block::TransformInterpolator(interpolator) => {
                quat_transform(&mut o, &interpolator.transform);
                o.reference(interpolator.data);
            }
            Block::TransformData(data) => {
                let count = if data.rotation_type == KEY_XYZ_ROTATION {
                    1
                } else {
                    data.quaternion_keys.len()
                };
                o.len32(count);
                if count != 0 {
                    o.u32(data.rotation_type);
                }
                if let Some(groups) = &data.xyz_rotations {
                    for group in groups.iter() {
                        key_group(&mut o, group, |o, v| o.f32(*v));
                    }
                } else {
                    for key in &data.quaternion_keys {
                        o.f32(key.time);
                        o.floats(&key.value);
                        if data.rotation_type == KEY_TBC {
                            o.floats(&key.tbc);
                        }
                    }
                }
                key_group(&mut o, &data.translations, |o, v| o.floats(v));
                key_group(&mut o, &data.scales, |o, v| o.f32(*v));
            }
            Block::BSplineTransformInterpolator(spline) => {
                o.f32(spline.start_time);
                o.f32(spline.stop_time);
                o.reference(spline.spline_data);
                o.reference(spline.basis_data);
                quat_transform(&mut o, &spline.transform);
                o.u32(spline.translation_handle);
                o.u32(spline.rotation_handle);
                o.u32(spline.scale_handle);
                if let Some(c) = &spline.compression {
                    o.floats(&[
                        c.translation_offset,
                        c.translation_half_range,
                        c.rotation_offset,
                        c.rotation_half_range,
                        c.scale_offset,
                        c.scale_half_range,
                    ]);
                }
            }
            Block::BSplineData(data) => {
                o.len32(data.float_control_points.len());
                o.floats(&data.float_control_points);
                o.len32(data.compact_control_points.len());
                for &v in &data.compact_control_points {
                    o.u16(v as u16);
                }
            }
            Block::BSplineBasisData(basis) => o.u32(basis.num_control_points),
            Block::TextKeyExtraData(text) => {
                let name = self.intern(&text.name);
                o.u32(name);
                o.len32(text.keys.len());
                for (time, value) in &text.keys {
                    o.f32(*time);
                    let index = self.intern(value);
                    o.u32(index);
                }
            }
        }
        o.0
    }

    fn geometry(&mut self, o: &mut Out, geom: &NiGeometry) {
        self.av_object(o, &geom.av);
        o.reference(geom.data);
        o.reference(geom.skin_instance);
        o.len32(geom.material_names.len());
        for name in &geom.material_names {
            let index = self.intern(name);
            o.u32(index);
        }
        for &extra in &geom.material_extra_data {
            o.i32(extra);
        }
        o.i32(geom.active_material);
        o.u8(u8::from(geom.dirty));
    }

    fn object_net(&mut self, o: &mut Out, net: &ObjectNet) {
        let name = self.intern(&net.name);
        o.u32(name);
        o.refs(&net.extra_data);
        o.reference(net.controller);
    }

    fn av_object(&mut self, o: &mut Out, av: &AvObject) {
        self.object_net(o, &av.net);
        o.u32(av.flags);
        o.floats(&av.translation);
        o.floats(av.rotation.as_flattened());
        o.f32(av.scale);
        o.refs(&av.properties);
        o.reference(av.collision_object);
    }

    fn shader_property(&mut self, o: &mut Out, shader: &ShaderProperty) {
        self.object_net(o, &shader.net);
        o.u16(shader.flags);
        o.u32(shader.shader_type);
        o.u32(shader.shader_flags);
        o.u32(shader.shader_flags2);
        o.f32(shader.env_map_scale);
        o.u32(shader.texture_clamp_mode);
    }

    /// Adds a described scene and makes its top node a footer root.
    /// Returns the top node's block index.
    pub fn add_scene(&mut self, root: &NodeDesc) -> u32 {
        let index = self.add_node(root);
        self.add_root(Some(index));
        index
    }

    fn add_node(&mut self, desc: &NodeDesc) -> u32 {
        let index = self.reserve();
        let mut av = AvObject {
            net: ObjectNet {
                name: desc.name.clone(),
                ..ObjectNet::default()
            },
            flags: if desc.hidden {
                0x0E | FLAG_HIDDEN
            } else {
                0x0E
            },
            translation: desc.transform.translation,
            rotation: desc.transform.rotation,
            scale: desc.transform.scale,
            ..AvObject::default()
        };
        if let Some(flags) = desc.bsx_flags {
            let bsx = BsxFlags {
                name: "BSX".to_owned(),
                flags,
            };
            av.net
                .extra_data
                .push(Some(self.push("BSXFlags", &bsx.into())));
        }
        if let Some(collision) = &desc.collision {
            let object = self.reserve();
            let body = self.reserve();
            // The body's shape is a block the parser has no layout for.
            let shape = self.push_raw("bhkBoxShape", vec![0; 20]);
            self.set_raw(
                body,
                "bhkRigidBody",
                rigid_body_payload(Some(shape), collision.layer, collision.motion_system),
            );
            let block = CollisionObject {
                target: Some(index),
                flags: collision.flags,
                body: Some(body),
                extra: Vec::new(),
            };
            self.set(object, "bhkCollisionObject", &block.into());
            av.collision_object = Some(object);
        }
        let mut children = Vec::with_capacity(desc.children.len());
        for child in &desc.children {
            children.push(Some(match child {
                ChildDesc::Node(node) => self.add_node(node),
                ChildDesc::Shape(shape) => self.add_shape(shape),
                ChildDesc::Unknown { type_name, payload } => {
                    self.push_raw(type_name, payload.clone())
                }
            }));
        }
        let node = NiNode {
            av,
            children,
            effects: Vec::new(),
            extra: desc.extra.clone(),
        };
        self.set(index, &desc.type_name, &node.into());
        index
    }

    fn add_shape(&mut self, desc: &ShapeDesc) -> u32 {
        let index = self.reserve();
        let mut properties = Vec::new();
        if !desc.textures.is_empty() {
            let shader = self.reserve();
            let set = BsShaderTextureSet {
                textures: desc.textures.clone(),
            };
            let set = self.push("BSShaderTextureSet", &set.into());
            let block = BsShaderPpLighting {
                texture_set: Some(set),
                ..BsShaderPpLighting::default()
            };
            self.set(shader, "BSShaderPPLightingProperty", &block.into());
            properties.push(Some(shader));
        }
        if let Some((flags, threshold)) = desc.alpha {
            let block = NiAlphaProperty {
                net: ObjectNet::default(),
                flags,
                threshold,
            };
            properties.push(Some(self.push("NiAlphaProperty", &block.into())));
        }
        if let Some(material) = &desc.material {
            properties.push(Some(
                self.push("NiMaterialProperty", &material.clone().into()),
            ));
        }
        if desc.two_sided {
            let block = NiStencilProperty {
                net: ObjectNet::default(),
                flags: STENCIL_FLAGS_TWO_SIDED,
                stencil_ref: 0,
                stencil_mask: 0xFFFF_FFFF,
            };
            properties.push(Some(self.push("NiStencilProperty", &block.into())));
        }

        let geom = desc.geometry_data();
        let (shape_type, data) = match &desc.topology {
            Topology::Triangles(triangles) => (
                "NiTriShape",
                self.push(
                    "NiTriShapeData",
                    &NiTriShapeData {
                        geom,
                        num_triangles: triangles.len() as u16,
                        num_triangle_points: (triangles.len() * 3) as u32,
                        triangles: Some(triangles.clone()),
                        match_groups: Vec::new(),
                    }
                    .into(),
                ),
            ),
            Topology::Strips(strips) => (
                "NiTriStrips",
                self.push(
                    "NiTriStripsData",
                    &NiTriStripsData {
                        geom,
                        num_triangles: strips
                            .iter()
                            .map(|s| s.len().saturating_sub(2))
                            .sum::<usize>() as u16,
                        strip_lengths: strips.iter().map(|s| s.len() as u16).collect(),
                        strips: Some(strips.clone()),
                    }
                    .into(),
                ),
            ),
        };
        let geometry = NiGeometry {
            av: AvObject {
                net: ObjectNet {
                    name: desc.name.clone(),
                    ..ObjectNet::default()
                },
                flags: if desc.hidden {
                    0x0E | FLAG_HIDDEN
                } else {
                    0x0E
                },
                translation: desc.transform.translation,
                rotation: desc.transform.rotation,
                scale: desc.transform.scale,
                properties,
                collision_object: None,
            },
            data: Some(data),
            skin_instance: None,
            material_names: Vec::new(),
            material_extra_data: Vec::new(),
            active_material: -1,
            dirty: false,
        };
        let block = match desc.topology {
            Topology::Triangles(_) => Block::tri_shape(geometry),
            Topology::Strips(_) => Block::tri_strips(geometry),
        };
        self.set(index, shape_type, &block);
        index
    }

    /// Writes the file: header, blocks, footer.
    pub fn build(&self) -> Vec<u8> {
        let mut o = Out::default();
        o.0.extend_from_slice(HEADER_LINE.as_bytes());
        o.u8(b'\n');
        o.u32(VERSION);
        o.u8(1); // little endian
        o.u32(USER_VERSION);
        o.len32(self.blocks.len());
        o.u32(self.stream_version);
        for s in &self.export_strings {
            o.short_string(s);
        }

        let mut types: Vec<&str> = Vec::new();
        let mut type_index = Vec::with_capacity(self.blocks.len());
        for block in &self.blocks {
            let position = match types.iter().position(|t| *t == block.type_name) {
                Some(position) => position,
                None => {
                    types.push(&block.type_name);
                    types.len() - 1
                }
            };
            type_index.push(position as u16);
        }
        o.u16(types.len() as u16);
        for name in &types {
            o.sized_string(name);
        }
        o.u16s(&type_index);
        for block in &self.blocks {
            o.u32(block.declared_size.unwrap_or(block.payload.len() as u32));
        }

        o.len32(self.strings.len());
        o.len32(self.strings.iter().map(|s| s.len()).max().unwrap_or(0));
        for s in &self.strings {
            o.sized_string(s);
        }
        o.len32(self.groups.len());
        for &group in &self.groups {
            o.u32(group);
        }

        for block in &self.blocks {
            o.0.extend_from_slice(&block.payload);
        }
        o.refs(&self.roots);
        o.0
    }
}

fn geometry_data(o: &mut Out, g: &GeometryData) {
    o.i32(g.group_id);
    o.u16(g.num_vertices);
    o.u8(g.keep_flags);
    o.u8(g.compress_flags);
    o.u8(u8::from(g.vertices.is_some()));
    for v in g.vertices.iter().flatten() {
        o.floats(v);
    }
    o.u16(g.vector_flags);
    o.u8(u8::from(g.normals.is_some()));
    for list in [&g.normals, &g.tangents, &g.bitangents] {
        for v in list.iter().flatten() {
            o.floats(v);
        }
    }
    o.floats(&g.center);
    o.f32(g.radius);
    o.u8(u8::from(g.colors.is_some()));
    for c in g.colors.iter().flatten() {
        o.floats(c);
    }
    for set in &g.uv_sets {
        for uv in set {
            o.floats(uv);
        }
    }
    o.u16(g.consistency_flags);
    o.reference(g.additional_data);
}

fn skin_transform(o: &mut Out, t: &Transform) {
    o.floats(t.rotation.as_flattened());
    o.floats(&t.translation);
    o.f32(t.scale);
}

fn quat_transform(o: &mut Out, t: &QuatTransform) {
    o.floats(&t.translation);
    o.floats(&t.rotation);
    o.f32(t.scale);
}

fn key_group<T>(o: &mut Out, group: &KeyGroup<T>, value: impl Fn(&mut Out, &T)) {
    o.len32(group.keys.len());
    if group.keys.is_empty() {
        return;
    }
    o.u32(group.interpolation);
    for key in &group.keys {
        o.f32(key.time);
        value(o, &key.value);
        match group.interpolation {
            KEY_QUADRATIC => {
                value(o, &key.forward);
                value(o, &key.backward);
            }
            KEY_TBC => o.floats(&key.tbc),
            _ => {}
        }
    }
}

/// A synthetic rigid body payload of [`RIGID_BODY_SIZE`] bytes: zeros, with
/// the shape reference first, the collision layer at
/// [`RIGID_BODY_LAYER_OFFSET`] and the motion system at
/// [`RIGID_BODY_MOTION_OFFSET`].
pub fn rigid_body_payload(shape: Ref, layer: u8, motion_system: u8) -> Vec<u8> {
    let mut payload = vec![0u8; RIGID_BODY_SIZE];
    let shape = shape.map_or(-1i32, |s| s as i32);
    payload[..4].copy_from_slice(&shape.to_le_bytes());
    payload[RIGID_BODY_LAYER_OFFSET] = layer;
    payload[RIGID_BODY_MOTION_OFFSET] = motion_system;
    payload
}

/// Collision metadata for a described node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollisionDesc {
    /// Collision object flags.
    pub flags: u16,
    /// Collision layer written into the rigid body.
    pub layer: u8,
    /// Motion system written into the rigid body.
    pub motion_system: u8,
}

impl CollisionDesc {
    /// A static, fixed body: layer 1, motion system 7.
    pub const STATIC: CollisionDesc = CollisionDesc {
        flags: 1,
        layer: 1,
        motion_system: 7,
    };
}

/// A node of a described scene.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeDesc {
    /// Block type: `NiNode`, `BSFadeNode`, or a node subclass.
    pub type_name: String,
    /// Node name.
    pub name: String,
    /// Transform relative to the parent.
    pub transform: Transform,
    /// Sets the hidden flag.
    pub hidden: bool,
    /// Attaches a `BSXFlags` extra data block with this value.
    pub bsx_flags: Option<u32>,
    /// Attaches a collision object, a rigid body and a shape block.
    pub collision: Option<CollisionDesc>,
    /// Children, in order.
    pub children: Vec<ChildDesc>,
    /// Bytes appended after the `NiNode` layout, for node subclasses.
    pub extra: Vec<u8>,
}

impl NodeDesc {
    /// A visible node with an identity transform and no children.
    pub fn new(type_name: &str, name: &str) -> Self {
        NodeDesc {
            type_name: type_name.to_owned(),
            name: name.to_owned(),
            transform: Transform::IDENTITY,
            hidden: false,
            bsx_flags: None,
            collision: None,
            children: Vec::new(),
            extra: Vec::new(),
        }
    }

    /// A `BSFadeNode`, the usual root of a static mesh.
    pub fn root(name: &str) -> Self {
        Self::new("BSFadeNode", name)
    }

    /// Sets the transform.
    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// Appends a child node.
    pub fn with_node(mut self, node: NodeDesc) -> Self {
        self.children.push(ChildDesc::Node(node));
        self
    }

    /// Appends a child shape.
    pub fn with_shape(mut self, shape: ShapeDesc) -> Self {
        self.children.push(ChildDesc::Shape(shape));
        self
    }

    /// Appends a child block of a type the parser does not know.
    pub fn with_unknown(mut self, type_name: &str, payload: Vec<u8>) -> Self {
        self.children.push(ChildDesc::Unknown {
            type_name: type_name.to_owned(),
            payload,
        });
        self
    }

    /// Attaches collision metadata.
    pub fn with_collision(mut self, collision: CollisionDesc) -> Self {
        self.collision = Some(collision);
        self
    }

    /// Writes a file with this node as its only root.
    pub fn to_nif(&self) -> Vec<u8> {
        let mut builder = NifBuilder::new();
        builder.add_scene(self);
        builder.build()
    }
}

/// A child of a described node.
#[derive(Debug, Clone, PartialEq)]
pub enum ChildDesc {
    /// A child node.
    Node(NodeDesc),
    /// A shape.
    Shape(ShapeDesc),
    /// A block of some other type, written as the given bytes.
    Unknown {
        /// Block type name.
        type_name: String,
        /// Payload bytes.
        payload: Vec<u8>,
    },
}

/// How a described shape stores its triangles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Topology {
    /// A triangle list: written as `NiTriShape` + `NiTriShapeData`.
    Triangles(Vec<[u16; 3]>),
    /// Triangle strips: written as `NiTriStrips` + `NiTriStripsData`.
    Strips(Vec<Vec<u16>>),
}

/// A shape of a described scene. At most 65535 vertices.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeDesc {
    /// Shape name.
    pub name: String,
    /// Transform relative to the parent.
    pub transform: Transform,
    /// Sets the hidden flag.
    pub hidden: bool,
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals.
    pub normals: Option<Vec<[f32; 3]>>,
    /// Texture coordinates.
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Vertex colours.
    pub colors: Option<Vec<[f32; 4]>>,
    /// Also write tangents and bitangents (only with normals). Their values
    /// are placeholders; the parser reads past them.
    pub tangents: bool,
    /// Triangles or strips.
    pub topology: Topology,
    /// Texture set slots; empty for no shader property.
    pub textures: Vec<String>,
    /// Alpha property flags and threshold.
    pub alpha: Option<(u16, u8)>,
    /// Material property.
    pub material: Option<NiMaterialProperty>,
    /// Attaches a stencil property that draws both faces.
    pub two_sided: bool,
}

impl ShapeDesc {
    /// A shape with positions and triangles only.
    pub fn new(name: &str, positions: Vec<[f32; 3]>, topology: Topology) -> Self {
        ShapeDesc {
            name: name.to_owned(),
            transform: Transform::IDENTITY,
            hidden: false,
            positions,
            normals: None,
            uvs: None,
            colors: None,
            tangents: false,
            topology,
            textures: Vec::new(),
            alpha: None,
            material: None,
            two_sided: false,
        }
    }

    /// A shape made of flat faces, each given by its corners in
    /// counter-clockwise order seen from outside: four corners for a
    /// rectangle, three for a triangle. Every face gets its own vertices, a
    /// face normal and UVs (first corner bottom left). With a non-empty
    /// `texture`, the shape gets a shader property whose diffuse slot is
    /// that path and whose normal slot is the same path with `_n` before
    /// the extension.
    pub fn from_faces(name: &str, faces: &[&[[f32; 3]]], texture: &str) -> Self {
        const UVS: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut triangles = Vec::new();
        for face in faces {
            if face.len() < 3 {
                continue;
            }
            let base = positions.len() as u16;
            let normal = face_normal(face[0], face[1], face[2]);
            for (i, &corner) in face.iter().enumerate() {
                positions.push(corner);
                normals.push(normal);
                uvs.push(UVS[i % 4]);
            }
            for i in 1..face.len() as u16 - 1 {
                triangles.push([base, base + i, base + i + 1]);
            }
        }
        let mut shape = ShapeDesc::new(name, positions, Topology::Triangles(triangles));
        shape.normals = Some(normals);
        shape.uvs = Some(uvs);
        shape.tangents = true;
        shape.textures = texture_slots(texture);
        shape
    }

    /// Rewrites a triangle list as strips, one strip per run of two
    /// triangles that form a quad `[a, b, c], [a, c, d]`, and one strip per
    /// remaining triangle. The shape is then written as `NiTriStrips`.
    pub fn into_strips(mut self) -> Self {
        if let Topology::Triangles(triangles) = &self.topology {
            let mut strips = Vec::new();
            let mut i = 0;
            while i < triangles.len() {
                let [a, b, c] = triangles[i];
                match triangles.get(i + 1) {
                    Some(&[a2, c2, d]) if a2 == a && c2 == c => {
                        strips.push(vec![a, b, d, c]);
                        i += 2;
                    }
                    _ => {
                        strips.push(vec![a, b, c]);
                        i += 1;
                    }
                }
            }
            self.topology = Topology::Strips(strips);
        }
        self
    }

    /// Sets the transform.
    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// Sets the same colour on every vertex.
    pub fn with_color(mut self, rgba: [f32; 4]) -> Self {
        self.colors = Some(vec![rgba; self.positions.len()]);
        self
    }

    /// Sets the alpha property.
    pub fn with_alpha(mut self, flags: u16, threshold: u8) -> Self {
        self.alpha = Some((flags, threshold));
        self
    }

    fn geometry_data(&self) -> GeometryData {
        let count = self.positions.len();
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for ((lo, hi), &v) in min.iter_mut().zip(max.iter_mut()).zip(p) {
                *lo = lo.min(v);
                *hi = hi.max(v);
            }
        }
        let center = if count == 0 {
            [0.0; 3]
        } else {
            [
                (min[0] + max[0]) / 2.0,
                (min[1] + max[1]) / 2.0,
                (min[2] + max[2]) / 2.0,
            ]
        };
        let radius = self
            .positions
            .iter()
            .map(|p| {
                let d = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .fold(0.0f32, f32::max);
        let tangents = self.tangents && self.normals.is_some();
        let mut vector_flags = 0;
        if self.uvs.is_some() {
            vector_flags |= VECTOR_FLAGS_UV;
        }
        if tangents {
            vector_flags |= VECTOR_FLAG_TANGENT_BIT;
        }
        GeometryData {
            group_id: 0,
            num_vertices: count as u16,
            keep_flags: 0,
            compress_flags: 0,
            vertices: Some(self.positions.clone()),
            vector_flags,
            normals: self.normals.clone(),
            tangents: tangents.then(|| vec![[1.0, 0.0, 0.0]; count]),
            bitangents: tangents.then(|| vec![[0.0, 1.0, 0.0]; count]),
            center,
            radius,
            colors: self.colors.clone(),
            uv_sets: self.uvs.iter().cloned().collect(),
            consistency_flags: 0,
            additional_data: None,
        }
    }
}

fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if length > 0.0 {
        [n[0] / length, n[1] / length, n[2] / length]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// The six texture set slots for a diffuse texture path: the path, the same
/// path with `_n` before its extension, and four empty slots. Empty input
/// gives no slots.
pub fn texture_slots(diffuse: &str) -> Vec<String> {
    if diffuse.is_empty() {
        return Vec::new();
    }
    let normal = match diffuse.rfind('.') {
        Some(dot) => format!("{}_n{}", &diffuse[..dot], &diffuse[dot..]),
        None => format!("{diffuse}_n"),
    };
    let mut slots = vec![diffuse.to_owned(), normal];
    slots.resize(6, String::new());
    slots
}

/// The five visible faces of an axis-aligned box plus its underside: six
/// rectangles with outward counter-clockwise winding, in the order east,
/// west, north, south, top, bottom.
pub fn box_faces(min: [f32; 3], max: [f32; 3]) -> [[[f32; 3]; 4]; 6] {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    [
        [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
        [[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]],
        [[x1, y1, z0], [x0, y1, z0], [x0, y1, z1], [x1, y1, z1]],
        [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        [[x0, y1, z0], [x1, y1, z0], [x1, y0, z0], [x0, y0, z0]],
    ]
}

/// A box shape: `size` is its extent along X (east), Y (north) and Z (up).
/// It is centred on the origin in X and Y and stands on the ground plane,
/// `z` from 0 to `size[2]`. 24 vertices, 12 triangles.
pub fn box_shape(name: &str, size: [f32; 3], texture: &str) -> ShapeDesc {
    let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
    let faces = box_faces([-hx, -hy, 0.0], [hx, hy, size[2]]);
    let faces: Vec<&[[f32; 3]]> = faces.iter().map(|f| f.as_slice()).collect();
    ShapeDesc::from_faces(name, &faces, texture)
}

/// A horizontal rectangle on the ground plane facing up: `size` is its
/// extent along X and Y, centred on the origin, at `z = 0`.
pub fn quad_shape(name: &str, size: [f32; 2], texture: &str) -> ShapeDesc {
    let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
    let face = [
        [-hx, -hy, 0.0],
        [hx, -hy, 0.0],
        [hx, hy, 0.0],
        [-hx, hy, 0.0],
    ];
    ShapeDesc::from_faces(name, &[&face], texture)
}

/// A complete file holding one [`box_shape`] under a `BSFadeNode` root with
/// static collision metadata.
pub fn box_mesh(size: [f32; 3], texture: &str) -> Vec<u8> {
    NodeDesc::root("Box")
        .with_collision(CollisionDesc::STATIC)
        .with_shape(box_shape("Box:0", size, texture))
        .to_nif()
}

/// A complete file holding one [`quad_shape`] under a `BSFadeNode` root.
pub fn quad(size: [f32; 2], texture: &str) -> Vec<u8> {
    NodeDesc::root("Quad")
        .with_shape(quad_shape("Quad:0", size, texture))
        .to_nif()
}

/// The scene behind [`building`]: a `BSFadeNode` root with BSX flags and
/// static collision, a `Walls` triangle shape (four walls from the ground
/// to `size[2]` plus the two gable ends) and, under a child node moved up
/// to the top of the walls, a two-sided `Roof` strip shape (two slopes
/// meeting at a ridge along X, a quarter of the depth above the walls).
/// `size` is the extent along X, Y and Z of the walls; the building is
/// centred on the origin in X and Y and stands on the ground plane.
pub fn building_scene(size: [f32; 3], wall_texture: &str, roof_texture: &str) -> NodeDesc {
    let (hx, hy, height) = (size[0] / 2.0, size[1] / 2.0, size[2]);
    let ridge = size[1] / 4.0;
    let sides = box_faces([-hx, -hy, 0.0], [hx, hy, height]);
    let east_gable = [
        [hx, -hy, height],
        [hx, hy, height],
        [hx, 0.0, height + ridge],
    ];
    let west_gable = [
        [-hx, hy, height],
        [-hx, -hy, height],
        [-hx, 0.0, height + ridge],
    ];
    let mut wall_faces: Vec<&[[f32; 3]]> = sides[..4].iter().map(|f| f.as_slice()).collect();
    wall_faces.push(&east_gable);
    wall_faces.push(&west_gable);
    let walls = ShapeDesc::from_faces("Walls", &wall_faces, wall_texture);

    // Roof coordinates are relative to the roof node at the top of the walls.
    let south_slope = [
        [-hx, -hy, 0.0],
        [hx, -hy, 0.0],
        [hx, 0.0, ridge],
        [-hx, 0.0, ridge],
    ];
    let north_slope = [
        [hx, hy, 0.0],
        [-hx, hy, 0.0],
        [-hx, 0.0, ridge],
        [hx, 0.0, ridge],
    ];
    let mut roof =
        ShapeDesc::from_faces("Roof", &[&south_slope, &north_slope], roof_texture).into_strips();
    roof.two_sided = true;

    let mut root = NodeDesc::root("Building")
        .with_collision(CollisionDesc::STATIC)
        .with_shape(walls)
        .with_node(
            NodeDesc::new("NiNode", "RoofNode")
                .with_transform(Transform::from_translation([0.0, 0.0, height]))
                .with_shape(roof),
        );
    root.bsx_flags = Some(0x02); // has collision
    root
}

/// A complete multi-part building file; see [`building_scene`].
pub fn building(size: [f32; 3], wall_texture: &str, roof_texture: &str) -> Vec<u8> {
    building_scene(size, wall_texture, roof_texture).to_nif()
}

// ------------------------------------------------------------ characters
//
// A six-node skeleton laid out like the real human one (a scene root, an
// accumulation root `Bip01` lifted to hip height, `Bip01 NonAccum` under
// it, a spine, a head and one arm), a skinned shape bound to it, and a
// `.kf` that walks it. Every value is chosen here; none comes from a game
// file.

/// The fixture skeleton's bones: name, parent index, local transform. The
/// order is the order [`crate::extract_skeleton`] returns them in.
pub fn skeleton_bones() -> Vec<(&'static str, Option<usize>, Transform)> {
    let lifted = |t: [f32; 3]| Transform::from_translation(t);
    vec![
        ("Scene Root", None, Transform::IDENTITY),
        ("Bip01", Some(0), lifted([0.0, 0.0, 50.0])),
        ("Bip01 NonAccum", Some(1), Transform::IDENTITY),
        ("Bip01 Spine", Some(2), lifted([0.0, 0.0, 10.0])),
        ("Bip01 Head", Some(3), lifted([0.0, 0.0, 30.0])),
        (
            "Bip01 L UpperArm",
            Some(3),
            Transform {
                // The arm's own +X points along the parent's +X, its +Y
                // along the parent's +Z: a quarter turn about X.
                rotation: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]],
                translation: [10.0, 0.0, 15.0],
                scale: 1.0,
            },
        ),
    ]
}

/// Bones of [`skeleton_bones`] the skinned shape is bound to, in the skin
/// instance's order.
pub const SKIN_BONES: [&str; 3] = ["Bip01 Spine", "Bip01 Head", "Bip01 L UpperArm"];

/// The translation of the skinned shape's own node relative to the root,
/// so that the bind transform involves the shape's placement too.
pub const SKINNED_SHAPE_OFFSET: [f32; 3] = [0.0, 0.0, 1.0];

/// The skinned shape's vertices in its own space (add
/// [`SKINNED_SHAPE_OFFSET`] for mesh space): a strip up the spine and
/// head (vertices 0 to 5) and a triangle along the arm (6 to 8).
pub const BODY_VERTICES: [[f32; 3]; 9] = [
    [-5.0, 0.0, 59.0],
    [5.0, 0.0, 59.0],
    [-5.0, 0.0, 79.0],
    [5.0, 0.0, 79.0],
    [-5.0, 0.0, 94.0],
    [5.0, 0.0, 94.0],
    [10.0, 0.0, 74.0],
    [30.0, 0.0, 74.0],
    [30.0, 0.0, 78.0],
];

/// The skinned shape's triangles.
pub const BODY_TRIANGLES: [[u16; 3]; 5] = [[0, 1, 3], [0, 3, 2], [2, 3, 5], [2, 5, 4], [6, 7, 8]];

/// The default weights of [`skinned_body`]: per vertex, `(index into
/// SKIN_BONES, weight)`. Vertices 2 and 3 are shared half and half between
/// spine and head; vertex 6 is a quarter spine, three quarters arm.
pub fn body_weights() -> Vec<Vec<(usize, f32)>> {
    vec![
        vec![(0, 1.0)],
        vec![(0, 1.0)],
        vec![(0, 0.5), (1, 0.5)],
        vec![(0, 0.5), (1, 0.5)],
        vec![(1, 1.0)],
        vec![(1, 1.0)],
        vec![(0, 0.25), (2, 0.75)],
        vec![(2, 1.0)],
        vec![(2, 1.0)],
    ]
}

/// World (root-space) transforms of [`skeleton_bones`] in their rest pose.
pub fn skeleton_world() -> Vec<Transform> {
    let bones = skeleton_bones();
    let mut world: Vec<Transform> = Vec::with_capacity(bones.len());
    for (_, parent, local) in &bones {
        let w = match parent {
            Some(p) => world[*p].then(local),
            None => *local,
        };
        world.push(w);
    }
    world
}

/// Adds the skeleton's nodes; the root gets `extra_children` after its
/// bone child. Returns the block index of each bone, in bone order.
fn add_skeleton_nodes(builder: &mut NifBuilder, extra_children: &[Ref]) -> Vec<u32> {
    let bones = skeleton_bones();
    let indices: Vec<u32> = bones.iter().map(|_| builder.reserve()).collect();
    for (i, (name, _, transform)) in bones.iter().enumerate() {
        let mut children: Vec<Ref> = bones
            .iter()
            .enumerate()
            .filter(|(_, (_, parent, _))| *parent == Some(i))
            .map(|(j, _)| Some(indices[j]))
            .collect();
        if i == 0 {
            children.extend_from_slice(extra_children);
        }
        let node = NiNode {
            av: AvObject {
                net: ObjectNet {
                    name: (*name).to_owned(),
                    ..ObjectNet::default()
                },
                translation: transform.translation,
                rotation: transform.rotation,
                scale: transform.scale,
                ..AvObject::default()
            },
            children,
            effects: Vec::new(),
            extra: Vec::new(),
        };
        let type_name = if i == 0 { "BSFadeNode" } else { "NiNode" };
        builder.set(indices[i], type_name, &node.into());
    }
    builder.add_root(Some(indices[0]));
    indices
}

/// A skeleton file: the nodes of [`skeleton_bones`] and nothing else.
pub fn skeleton_nif() -> Vec<u8> {
    let mut builder = NifBuilder::new();
    add_skeleton_nodes(&mut builder, &[]);
    builder.build()
}

/// [`skinned_body_with`] with [`body_weights`], weights stored in the skin
/// data (and in a partition, as the real files do).
pub fn skinned_body() -> Vec<u8> {
    skinned_body_with(&body_weights(), false)
}

/// A mesh file as the real body parts are laid out: the skeleton's bone
/// nodes, and under the root a `NiTriShape` "Body" (at
/// [`SKINNED_SHAPE_OFFSET`], with normals and UVs) whose
/// `BSDismemberSkinInstance` binds it to [`SKIN_BONES`] in the rest pose of
/// [`skeleton_bones`]. `weights` has one list per vertex of
/// [`BODY_VERTICES`] (more or fewer lists are written as given, for
/// malformed files). With `partition_only` the skin data stores no weights
/// and the `NiSkinPartition` is their only source; otherwise both hold
/// them. The partition keeps at most four influences per vertex.
pub fn skinned_body_with(weights: &[Vec<(usize, f32)>], partition_only: bool) -> Vec<u8> {
    let mut builder = NifBuilder::new();
    let shape = builder.reserve();
    let bones = add_skeleton_nodes(&mut builder, &[Some(shape)]);
    let names: Vec<&str> = skeleton_bones().iter().map(|b| b.0).collect();
    let skin_bones: Vec<u32> = SKIN_BONES
        .iter()
        .map(|name| bones[names.iter().position(|n| n == name).unwrap_or(0)])
        .collect();

    let count = BODY_VERTICES.len();
    let shape_desc = ShapeDesc {
        normals: Some(vec![[0.0, -1.0, 0.0]; count]),
        uvs: Some(
            BODY_VERTICES
                .iter()
                .map(|v| [(v[0] + 5.0) / 35.0, 1.0 - (v[2] - 59.0) / 35.0])
                .collect(),
        ),
        ..ShapeDesc::new(
            "Body",
            BODY_VERTICES.to_vec(),
            Topology::Triangles(BODY_TRIANGLES.to_vec()),
        )
    };
    let data = builder.push(
        "NiTriShapeData",
        &NiTriShapeData {
            geom: shape_desc.geometry_data(),
            num_triangles: BODY_TRIANGLES.len() as u16,
            num_triangle_points: (BODY_TRIANGLES.len() * 3) as u32,
            triangles: Some(BODY_TRIANGLES.to_vec()),
            match_groups: Vec::new(),
        }
        .into(),
    );

    // Skin space is the shape's own space; skin-to-bone is the inverse of
    // the bone's world transform composed after the shape's.
    let world = skeleton_world();
    let shape_world = Transform::from_translation(SKINNED_SHAPE_OFFSET);
    let mut data_bones = Vec::with_capacity(SKIN_BONES.len());
    for (b, name) in SKIN_BONES.iter().enumerate() {
        let bone_world = world[names.iter().position(|n| n == name).unwrap_or(0)];
        let mut influences = Vec::new();
        for (vertex, list) in weights.iter().enumerate() {
            for &(bone, weight) in list {
                if bone == b {
                    influences.push((vertex as u16, weight));
                }
            }
        }
        data_bones.push(SkinDataBone {
            skin_to_bone: bone_world.inverse().then(&shape_world),
            bound_center: [0.0; 3],
            bound_radius: 20.0,
            num_vertices: if partition_only {
                0
            } else {
                influences.len() as u16
            },
            weights: if partition_only {
                Vec::new()
            } else {
                influences
            },
        });
    }
    let skin_data = builder.push(
        "NiSkinData",
        &NiSkinData {
            skin_transform: Transform::IDENTITY,
            has_vertex_weights: !partition_only,
            bones: data_bones,
        }
        .into(),
    );

    // One partition holding everything, four influences per vertex.
    let per = 4usize;
    let mut vertex_weights = Vec::with_capacity(weights.len() * per);
    let mut bone_indices = Vec::with_capacity(weights.len() * per);
    for list in weights {
        let mut sorted = list.clone();
        sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
        sorted.resize(per, (0, 0.0));
        for (bone, weight) in sorted.into_iter().take(per) {
            vertex_weights.push(weight);
            bone_indices.push(bone as u8);
        }
    }
    let partition = builder.push(
        "NiSkinPartition",
        &NiSkinPartition {
            partitions: vec![SkinPartition {
                num_vertices: weights.len() as u16,
                num_triangles: BODY_TRIANGLES.len() as u16,
                bones: (0..SKIN_BONES.len() as u16).collect(),
                weights_per_vertex: per as u16,
                vertex_map: Some((0..weights.len() as u16).collect()),
                vertex_weights: Some(vertex_weights),
                strip_lengths: Vec::new(),
                strips: Vec::new(),
                triangles: Some(BODY_TRIANGLES.to_vec()),
                bone_indices: Some(bone_indices),
            }],
        }
        .into(),
    );
    let instance = builder.push(
        "BSDismemberSkinInstance",
        &NiSkinInstance {
            data: Some(skin_data),
            partition: Some(partition),
            skeleton_root: Some(bones[0]),
            bones: skin_bones.iter().map(|&b| Some(b)).collect(),
            body_parts: vec![BodyPart {
                flags: 1,
                body_part: 0,
            }],
        }
        .into(),
    );
    let geometry = NiGeometry {
        av: AvObject {
            net: ObjectNet {
                name: "Body".to_owned(),
                ..ObjectNet::default()
            },
            translation: SKINNED_SHAPE_OFFSET,
            ..AvObject::default()
        },
        data: Some(data),
        skin_instance: Some(instance),
        material_names: Vec::new(),
        material_extra_data: Vec::new(),
        active_material: -1,
        dirty: false,
    };
    builder.set(shape, "NiTriShape", &Block::tri_shape(geometry));
    builder.build()
}

/// One controlled block of a [`KfDesc`].
#[derive(Debug, Clone, PartialEq)]
pub enum KfTrack {
    /// A `NiTransformInterpolator`, with a `NiTransformData` when `data`
    /// is given.
    Keyed {
        /// Animated node name.
        bone: String,
        /// The interpolator's own transform (channels without keys).
        transform: QuatTransform,
        /// The keys.
        data: Option<NiTransformData>,
    },
    /// A `NiBSplineCompTransformInterpolator` (or the uncompressed type
    /// when `interpolator.compression` is `None`) with its own spline data
    /// and basis blocks. The interpolator's block references are filled in.
    Spline {
        /// Animated node name.
        bone: String,
        /// Times, transform, handles and compression.
        interpolator: NiBSplineTransformInterpolator,
        /// Control points.
        data: NiBSplineData,
        /// Control points per channel.
        control_points: u32,
    },
    /// A block of any other type, written as given, for example a
    /// `NiFloatInterpolator`.
    Raw {
        /// Animated node name.
        bone: String,
        /// Block type name.
        type_name: String,
        /// Payload bytes.
        payload: Vec<u8>,
    },
}

/// A `.kf` file: one `NiControllerSequence` (the only root) and its
/// interpolators.
#[derive(Debug, Clone, PartialEq)]
pub struct KfDesc {
    /// Sequence name.
    pub name: String,
    /// Start time, seconds.
    pub start_time: f32,
    /// Stop time, seconds.
    pub stop_time: f32,
    /// One of the `CYCLE_*` constants.
    pub cycle_type: u32,
    /// Accumulation root node name.
    pub accum_root: String,
    /// Text keys, written as a `NiTextKeyExtraData` when not empty.
    pub text_keys: Vec<(f32, String)>,
    /// The controlled blocks.
    pub tracks: Vec<KfTrack>,
    /// Stream version of the header (34, or an older animation stream).
    pub stream_version: u32,
}

impl KfDesc {
    /// An empty looping sequence from 0 to `stop_time` with accumulation
    /// root `Bip01`.
    pub fn new(name: &str, stop_time: f32) -> Self {
        KfDesc {
            name: name.to_owned(),
            start_time: 0.0,
            stop_time,
            cycle_type: CYCLE_LOOP,
            accum_root: "Bip01".to_owned(),
            text_keys: Vec::new(),
            tracks: Vec::new(),
            stream_version: STREAM_VERSION,
        }
    }

    /// Appends a track.
    pub fn with_track(mut self, track: KfTrack) -> Self {
        self.tracks.push(track);
        self
    }

    /// Writes the file.
    pub fn to_kf(&self) -> Vec<u8> {
        let mut builder = NifBuilder::new();
        builder.stream_version(self.stream_version);
        let sequence = builder.reserve();
        let text_keys = (!self.text_keys.is_empty()).then(|| {
            builder.push(
                "NiTextKeyExtraData",
                &NiTextKeyExtraData {
                    name: String::new(),
                    keys: self.text_keys.clone(),
                }
                .into(),
            )
        });
        let mut controlled = Vec::with_capacity(self.tracks.len());
        for track in &self.tracks {
            let (bone, interpolator, controller_type) = match track {
                KfTrack::Keyed {
                    bone,
                    transform,
                    data,
                } => {
                    let interpolator = builder.reserve();
                    let data = data
                        .as_ref()
                        .map(|d| builder.push("NiTransformData", &d.clone().into()));
                    builder.set(
                        interpolator,
                        "NiTransformInterpolator",
                        &NiTransformInterpolator {
                            transform: *transform,
                            data,
                        }
                        .into(),
                    );
                    (bone, interpolator, "NiTransformController")
                }
                KfTrack::Spline {
                    bone,
                    interpolator,
                    data,
                    control_points,
                } => {
                    let index = builder.reserve();
                    let data = builder.push("NiBSplineData", &data.clone().into());
                    let basis = builder.push(
                        "NiBSplineBasisData",
                        &NiBSplineBasisData {
                            num_control_points: *control_points,
                        }
                        .into(),
                    );
                    let type_name = if interpolator.compression.is_some() {
                        "NiBSplineCompTransformInterpolator"
                    } else {
                        "NiBSplineTransformInterpolator"
                    };
                    let block = NiBSplineTransformInterpolator {
                        spline_data: Some(data),
                        basis_data: Some(basis),
                        ..interpolator.clone()
                    };
                    builder.set(index, type_name, &block.into());
                    (bone, index, "NiTransformController")
                }
                KfTrack::Raw {
                    bone,
                    type_name,
                    payload,
                } => (
                    bone,
                    builder.push_raw(type_name, payload.clone()),
                    "NiGeomMorpherController",
                ),
            };
            controlled.push(ControlledBlock {
                interpolator: Some(interpolator),
                controller: None,
                priority: 0,
                node_name: bone.clone(),
                property_type: String::new(),
                controller_type: controller_type.to_owned(),
                controller_id: String::new(),
                interpolator_id: String::new(),
            });
        }
        let block = NiControllerSequence {
            name: self.name.clone(),
            controlled_blocks: controlled,
            array_grow_by: 1,
            weight: 1.0,
            text_keys,
            cycle_type: self.cycle_type,
            frequency: 1.0,
            start_time: self.start_time,
            stop_time: self.stop_time,
            manager: None,
            accum_root_name: self.accum_root.clone(),
            anim_note_arrays: Vec::new(),
        };
        builder.set(sequence, "NiControllerSequence", &block.into());
        builder.add_root(Some(sequence));
        builder.build()
    }
}

/// Compresses spline control point values as the compressed interpolator
/// stores them: `round((value - offset) / half_range · 32767)`, clamped.
pub fn compress_control_points(values: &[f32], offset: f32, half_range: f32) -> Vec<i16> {
    values
        .iter()
        .map(|&v| {
            let q = if half_range != 0.0 {
                ((v - offset) / half_range * 32767.0).round()
            } else {
                0.0
            };
            q.clamp(-32767.0, 32767.0) as i16
        })
        .collect()
}

/// The not-set marker of interpolator transforms: every component
/// `-3.4028235e38`.
pub fn unset_transform() -> QuatTransform {
    QuatTransform {
        translation: [-f32::MAX; 3],
        rotation: [-f32::MAX; 4],
        scale: -f32::MAX,
    }
}

/// Linear translation keys.
pub fn linear_translations(keys: &[(f32, [f32; 3])]) -> KeyGroup<[f32; 3]> {
    KeyGroup {
        interpolation: KEY_LINEAR,
        keys: keys
            .iter()
            .map(|&(time, value)| Key {
                time,
                value,
                ..Key::default()
            })
            .collect(),
    }
}

/// Linear quaternion keys (`w, x, y, z`).
pub fn linear_rotations(keys: &[(f32, [f32; 4])]) -> NiTransformData {
    NiTransformData {
        rotation_type: KEY_LINEAR,
        quaternion_keys: keys
            .iter()
            .map(|&(time, value)| QuatKey {
                time,
                value,
                tbc: [0.0; 3],
            })
            .collect(),
        ..NiTransformData::default()
    }
}

/// Number of control points of [`walk_kf`]'s spline track.
pub const WALK_SPLINE_POINTS: u32 = 5;
/// Distance `Bip01` moves along +Y over one cycle of [`walk_kf`].
pub const WALK_STRIDE: f32 = 100.0;
/// Duration of [`walk_kf`] in seconds.
pub const WALK_DURATION: f32 = 1.0;

/// A small looping walk for the [`skeleton_bones`] skeleton, one second
/// long, accumulation root `Bip01`, text keys `start` at 0 and `end` at 1:
///
/// - `Bip01`: linear translation keys from the origin to
///   `(0, WALK_STRIDE, 0)`, rotation and scale from the interpolator's
///   transform (identity, 1);
/// - `Bip01 NonAccum`: no keys; the interpolator's transform lifts it to
///   `(0, 0, 50)` (as the real animations lift the hips);
/// - `Bip01 Spine`: a compressed B-spline rotation about +Z from 0 to 90°
///   over [`WALK_SPLINE_POINTS`] control points; its translation and scale
///   are not set, so the skeleton's stay;
/// - `Bip01 L UpperArm`: linear quaternion keys, identity at 0 and 1 and a
///   quarter turn about +X at 0.5;
/// - `Bip01 Head`: a `NiFloatInterpolator` (not a transform), which
///   [`crate::parse_animation`] leaves out.
pub fn walk_kf() -> Vec<u8> {
    walk_desc().to_kf()
}

/// The description behind [`walk_kf`].
pub fn walk_desc() -> KfDesc {
    let identity = QuatTransform::default();
    let root = KfTrack::Keyed {
        bone: "Bip01".to_owned(),
        transform: QuatTransform {
            translation: [-f32::MAX; 3],
            ..identity
        },
        data: Some(NiTransformData {
            translations: linear_translations(&[
                (0.0, [0.0, 0.0, 0.0]),
                (WALK_DURATION, [0.0, WALK_STRIDE, 0.0]),
            ]),
            ..NiTransformData::default()
        }),
    };
    let hips = KfTrack::Keyed {
        bone: "Bip01 NonAccum".to_owned(),
        transform: QuatTransform {
            translation: [0.0, 0.0, 50.0],
            ..identity
        },
        data: None,
    };
    let n = WALK_SPLINE_POINTS as usize;
    let mut quats = Vec::with_capacity(n * 4);
    for k in 0..n {
        let angle = std::f32::consts::FRAC_PI_2 * k as f32 / (n - 1) as f32;
        let (s, c) = (angle / 2.0).sin_cos();
        quats.extend_from_slice(&[c, 0.0, 0.0, s]);
    }
    let spine = KfTrack::Spline {
        bone: "Bip01 Spine".to_owned(),
        interpolator: NiBSplineTransformInterpolator {
            start_time: 0.0,
            stop_time: WALK_DURATION,
            spline_data: None,
            basis_data: None,
            transform: unset_transform(),
            translation_handle: NO_HANDLE,
            rotation_handle: 0,
            scale_handle: NO_HANDLE,
            compression: Some(BSplineCompression {
                translation_offset: f32::MAX,
                translation_half_range: f32::MAX,
                rotation_offset: 0.5,
                rotation_half_range: 0.5,
                scale_offset: f32::MAX,
                scale_half_range: f32::MAX,
            }),
        },
        data: NiBSplineData {
            float_control_points: Vec::new(),
            compact_control_points: compress_control_points(&quats, 0.5, 0.5),
        },
        control_points: WALK_SPLINE_POINTS,
    };
    let quarter = std::f32::consts::FRAC_PI_4;
    let arm = KfTrack::Keyed {
        bone: "Bip01 L UpperArm".to_owned(),
        transform: unset_transform(),
        data: Some(linear_rotations(&[
            (0.0, [1.0, 0.0, 0.0, 0.0]),
            (0.5, [quarter.cos(), quarter.sin(), 0.0, 0.0]),
            (1.0, [1.0, 0.0, 0.0, 0.0]),
        ])),
    };
    let head = KfTrack::Raw {
        bone: "Bip01 Head".to_owned(),
        type_name: "NiFloatInterpolator".to_owned(),
        payload: {
            let mut p = 0.5f32.to_le_bytes().to_vec();
            p.extend_from_slice(&(-1i32).to_le_bytes());
            p
        },
    };
    KfDesc {
        text_keys: vec![(0.0, "start".to_owned()), (WALK_DURATION, "end".to_owned())],
        ..KfDesc::new("Walk", WALK_DURATION)
    }
    .with_track(root)
    .with_track(hips)
    .with_track(spine)
    .with_track(arm)
    .with_track(head)
}
