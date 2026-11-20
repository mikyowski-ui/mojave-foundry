//! Block structures and their hand-written parsers, for file version
//! 20.2.0.7, user version 11, stream version 34.
//!
//! Each structure lists its fields in file order. Names stored through the
//! string table are resolved to `String` (empty when the file stores "none").

use crate::anim_blocks::{
    NiBSplineBasisData, NiBSplineData, NiBSplineTransformInterpolator, NiControllerSequence,
    NiTextKeyExtraData, NiTransformData, NiTransformInterpolator,
};
use crate::error::NifError;
use crate::reader::Reader;
use crate::skin_blocks::{NiSkinData, NiSkinInstance, NiSkinPartition};
use crate::{anim_blocks, skin_blocks};

/// A reference to another block: its index, or `None` for the stored `-1`.
pub type Ref = Option<u32>;

/// Bit 0 of an object's flags: the object is hidden (application culled).
pub const FLAG_HIDDEN: u32 = 0x0000_0001;

/// The fields every named object starts with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjectNet {
    /// Object name.
    pub name: String,
    /// Extra data blocks attached to the object.
    pub extra_data: Vec<Ref>,
    /// Controller (animation) block.
    pub controller: Ref,
}

/// The fields every scene-graph object (node or shape) starts with.
#[derive(Debug, Clone, PartialEq)]
pub struct AvObject {
    /// Name, extra data, controller.
    pub net: ObjectNet,
    /// Flags, 32 bits wide at this stream version. Bit 0 is hidden.
    pub flags: u32,
    /// Translation relative to the parent.
    pub translation: [f32; 3],
    /// Rotation relative to the parent, in file order: `rotation[r]` is row
    /// `r`, and the row dotted with a column vector gives component `r` of
    /// the result. A child's transform applies first, its parent's second.
    /// This reading was checked on real skinned meshes, where it is the one
    /// under which every bone of a skin agrees on the bind transform.
    pub rotation: [[f32; 3]; 3],
    /// Uniform scale relative to the parent.
    pub scale: f32,
    /// Property blocks (shader, alpha, material, stencil, ...).
    pub properties: Vec<Ref>,
    /// Collision object block.
    pub collision_object: Ref,
}

impl Default for AvObject {
    fn default() -> Self {
        AvObject {
            net: ObjectNet::default(),
            flags: 0x0000_000E,
            translation: [0.0; 3],
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 1.0,
            properties: Vec::new(),
            collision_object: None,
        }
    }
}

impl AvObject {
    /// Whether the hidden flag is set.
    pub fn is_hidden(&self) -> bool {
        self.flags & FLAG_HIDDEN != 0
    }
}

/// `NiNode` and every node type that starts with the same layout.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiNode {
    /// Common scene-graph fields.
    pub av: AvObject,
    /// Child nodes and shapes.
    pub children: Vec<Ref>,
    /// Dynamic effects (lights).
    pub effects: Vec<Ref>,
    /// Bytes after the `NiNode` layout, for node subclasses that add fields
    /// (`NiSwitchNode`, `BSOrderedNode`, ...). Empty for `NiNode` and
    /// `BSFadeNode`. Kept raw: bounded by the block's declared size.
    pub extra: Vec<u8>,
}

/// `NiTriShape` and `NiTriStrips`: a shape that points at its geometry data.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiGeometry {
    /// Common scene-graph fields.
    pub av: AvObject,
    /// The geometry data block.
    pub data: Ref,
    /// Skin instance; none on static meshes.
    pub skin_instance: Ref,
    /// Material names, one per material.
    pub material_names: Vec<String>,
    /// Material extra data, one per material.
    pub material_extra_data: Vec<i32>,
    /// Index of the active material.
    pub active_material: i32,
    /// The "material needs update" byte.
    pub dirty: bool,
}

/// One segment of a `BSSegmentedTriShape`: a run of the shape's triangles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TriSegment {
    /// Segment flags.
    pub flags: u8,
    /// Index of the segment's first triangle point.
    pub start: u32,
    /// Number of triangles in the segment.
    pub triangles: u32,
}

/// `BSSegmentedTriShape`: a `NiTriShape` followed by a segment list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BsSegmentedTriShape {
    /// The `NiTriShape` layout.
    pub shape: NiGeometry,
    /// The segments.
    pub segments: Vec<TriSegment>,
}

/// Bits 12 to 15 of the vector flags: tangents and bitangents are stored.
pub const VECTOR_FLAGS_TANGENTS: u16 = 0xF000;
/// Bit 0 of the vector flags: one UV set is stored.
pub const VECTOR_FLAGS_UV: u16 = 0x0001;

/// The vertex data shared by `NiTriShapeData` and `NiTriStripsData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GeometryData {
    /// Group id.
    pub group_id: i32,
    /// Vertex count; every present array has this many entries.
    pub num_vertices: u16,
    /// Keep flags.
    pub keep_flags: u8,
    /// Compress flags.
    pub compress_flags: u8,
    /// Vertex positions, when stored.
    pub vertices: Option<Vec<[f32; 3]>>,
    /// Bit 0: a UV set is stored. Bits 12 to 15: tangents and bitangents are
    /// stored (only together with normals).
    pub vector_flags: u16,
    /// Vertex normals, when stored.
    pub normals: Option<Vec<[f32; 3]>>,
    /// Tangents; present when normals are and the tangent flag is set.
    pub tangents: Option<Vec<[f32; 3]>>,
    /// Bitangents; present exactly when `tangents` is.
    pub bitangents: Option<Vec<[f32; 3]>>,
    /// Bounding sphere centre.
    pub center: [f32; 3],
    /// Bounding sphere radius.
    pub radius: f32,
    /// Vertex colours, RGBA floats, when stored.
    pub colors: Option<Vec<[f32; 4]>>,
    /// UV sets (zero or one at this stream version).
    pub uv_sets: Vec<Vec<[f32; 2]>>,
    /// Consistency flags.
    pub consistency_flags: u16,
    /// Additional geometry data block.
    pub additional_data: Ref,
}

/// `NiTriShapeData`: indexed triangles.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiTriShapeData {
    /// Vertex data.
    pub geom: GeometryData,
    /// Triangle count.
    pub num_triangles: u16,
    /// Stored as three times the triangle count.
    pub num_triangle_points: u32,
    /// The triangles, when stored.
    pub triangles: Option<Vec<[u16; 3]>>,
    /// Match groups: lists of vertex indices that share a position.
    pub match_groups: Vec<Vec<u16>>,
}

/// `NiTriStripsData`: triangle strips.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiTriStripsData {
    /// Vertex data.
    pub geom: GeometryData,
    /// Triangle count the strips expand to.
    pub num_triangles: u16,
    /// Length of each strip.
    pub strip_lengths: Vec<u16>,
    /// The strips' vertex indices, when stored; one list per strip.
    pub strips: Option<Vec<Vec<u16>>>,
}

/// The fields shared by the lighting shader properties.
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderProperty {
    /// Name, extra data, controller.
    pub net: ObjectNet,
    /// Property flags.
    pub flags: u16,
    /// Shader type.
    pub shader_type: u32,
    /// First shader flag word.
    pub shader_flags: u32,
    /// Second shader flag word.
    pub shader_flags2: u32,
    /// Environment map scale.
    pub env_map_scale: f32,
    /// Texture clamp mode.
    pub texture_clamp_mode: u32,
}

impl Default for ShaderProperty {
    fn default() -> Self {
        ShaderProperty {
            net: ObjectNet::default(),
            flags: 1,
            shader_type: 1,
            shader_flags: 0x8200_0000,
            shader_flags2: 1,
            env_map_scale: 1.0,
            texture_clamp_mode: 3,
        }
    }
}

/// `BSShaderPPLightingProperty` (and `Lighting30ShaderProperty`, which has
/// the same layout).
#[derive(Debug, Clone, PartialEq)]
pub struct BsShaderPpLighting {
    /// Common shader fields.
    pub shader: ShaderProperty,
    /// The `BSShaderTextureSet` block.
    pub texture_set: Ref,
    /// Refraction strength.
    pub refraction_strength: f32,
    /// Refraction fire period.
    pub refraction_fire_period: i32,
    /// Parallax maximum passes.
    pub parallax_max_passes: f32,
    /// Parallax scale.
    pub parallax_scale: f32,
}

impl Default for BsShaderPpLighting {
    fn default() -> Self {
        BsShaderPpLighting {
            shader: ShaderProperty::default(),
            texture_set: None,
            refraction_strength: 0.0,
            refraction_fire_period: 0,
            parallax_max_passes: 4.0,
            parallax_scale: 1.0,
        }
    }
}

/// `BSShaderNoLightingProperty`: an unlit, single-texture shader.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BsShaderNoLighting {
    /// Common shader fields.
    pub shader: ShaderProperty,
    /// The texture path, stored inline.
    pub file_name: String,
    /// Bytes after the file name (four falloff floats), kept raw.
    pub extra: Vec<u8>,
}

/// `BSShaderTextureSet`: texture paths by slot (diffuse, normal, glow,
/// parallax, environment, environment mask).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BsShaderTextureSet {
    /// Paths as stored; an unused slot is an empty string.
    pub textures: Vec<String>,
}

/// `NiAlphaProperty`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiAlphaProperty {
    /// Name, extra data, controller.
    pub net: ObjectNet,
    /// Bit 0: blending enabled. Bits 1 to 4: source blend. Bits 5 to 8:
    /// destination blend. Bit 9: alpha test enabled. Bits 10 to 12: test
    /// function. Bit 13: no sorting.
    pub flags: u16,
    /// Alpha test threshold, 0 to 255.
    pub threshold: u8,
}

impl NiAlphaProperty {
    /// Flag bit 0.
    pub const BLEND_ENABLE: u16 = 0x0001;
    /// Flag bit 9.
    pub const TEST_ENABLE: u16 = 0x0200;

    /// Whether alpha blending is enabled.
    pub fn blend_enabled(&self) -> bool {
        self.flags & Self::BLEND_ENABLE != 0
    }

    /// Whether alpha testing is enabled.
    pub fn test_enabled(&self) -> bool {
        self.flags & Self::TEST_ENABLE != 0
    }
}

/// `NiMaterialProperty`. At this stream version the ambient and diffuse
/// colours are not stored.
#[derive(Debug, Clone, PartialEq)]
pub struct NiMaterialProperty {
    /// Name, extra data, controller.
    pub net: ObjectNet,
    /// Specular colour.
    pub specular: [f32; 3],
    /// Emissive colour.
    pub emissive: [f32; 3],
    /// Glossiness.
    pub glossiness: f32,
    /// Opacity, 1 is opaque.
    pub alpha: f32,
    /// Emissive multiplier.
    pub emissive_multiple: f32,
}

impl Default for NiMaterialProperty {
    fn default() -> Self {
        NiMaterialProperty {
            net: ObjectNet::default(),
            specular: [0.0; 3],
            emissive: [0.0; 3],
            glossiness: 10.0,
            alpha: 1.0,
            emissive_multiple: 1.0,
        }
    }
}

/// `NiStencilProperty`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiStencilProperty {
    /// Name, extra data, controller.
    pub net: ObjectNet,
    /// Bit 0: stencil enabled. Bits 1 to 9: fail, depth-fail and pass
    /// actions. Bits 10 and 11: draw mode. Bits 12 to 14: test function.
    pub flags: u16,
    /// Stencil reference value.
    pub stencil_ref: u32,
    /// Stencil mask.
    pub stencil_mask: u32,
}

impl NiStencilProperty {
    /// Draw mode 3: both faces are drawn.
    pub const DRAW_BOTH: u16 = 3;

    /// Bits 10 and 11 of the flags.
    pub fn draw_mode(&self) -> u16 {
        (self.flags >> 10) & 3
    }

    /// Whether both faces are drawn (draw mode 3).
    pub fn two_sided(&self) -> bool {
        self.draw_mode() == Self::DRAW_BOTH
    }
}

/// `NiTexturingProperty`, read as far as its base (diffuse) texture slot.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiTexturingProperty {
    /// Name, extra data, controller.
    pub net: ObjectNet,
    /// Property flags.
    pub flags: u16,
    /// Number of texture slots the block describes.
    pub texture_count: u32,
    /// Whether the base texture slot is used.
    pub has_base_texture: bool,
    /// The base slot's `NiSourceTexture`; read only when the slot is used.
    pub base_source: Ref,
    /// Everything after that: the rest of the base slot and the other
    /// slots, kept raw.
    pub extra: Vec<u8>,
}

/// `NiSourceTexture`: a texture file referenced by a texturing property.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NiSourceTexture {
    /// Name, extra data, controller.
    pub net: ObjectNet,
    /// 1 when the texture is an external file, 0 when it is embedded.
    pub use_external: u8,
    /// The texture's file name.
    pub file_name: String,
    /// An unused link (external) or the pixel data block (embedded).
    pub link: Ref,
    /// Pixel layout.
    pub pixel_layout: u32,
    /// Mipmap use.
    pub use_mipmaps: u32,
    /// Alpha format.
    pub alpha_format: u32,
    /// "Is static" byte.
    pub is_static: u8,
    /// "Direct render" byte.
    pub direct_render: u8,
    /// "Persist render data" byte.
    pub persist_render_data: u8,
}

/// `BSXFlags`: an integer extra data block on the root node.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BsxFlags {
    /// Extra data name, normally `BSX`.
    pub name: String,
    /// The flag word. Bit 5 says the file contains an editor marker.
    pub flags: u32,
}

/// `bhkCollisionObject` and its siblings, as metadata only.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CollisionObject {
    /// The scene-graph object the collision belongs to.
    pub target: Ref,
    /// Collision object flags.
    pub flags: u16,
    /// The body (rigid body) block.
    pub body: Ref,
    /// Bytes after the common layout. Only `bhkBlendCollisionObject` has
    /// any; the other types are checked against the exact ten-byte layout.
    pub extra: Vec<u8>,
}

/// Two bytes peeked out of a `bhkRigidBody` / `bhkRigidBodyT`. The block is
/// otherwise skipped by size and is counted as skipped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RigidBodyPeek {
    /// Collision layer (byte 4 of the block), when the block is long enough.
    pub layer: Option<u8>,
    /// Motion system (byte 212 of the block), when the block is long enough.
    pub motion_system: Option<u8>,
}

/// Byte offset of the collision layer within a rigid body block.
pub const RIGID_BODY_LAYER_OFFSET: usize = 4;
/// Byte offset of the motion system within a rigid body block.
pub const RIGID_BODY_MOTION_OFFSET: usize = 212;

/// One parsed block. The larger structures are boxed so that a file with
/// very many blocks costs little memory per block.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// `NiNode`, `BSFadeNode` and the node subclasses in [`NODE_PREFIX_TYPES`].
    Node(Box<NiNode>),
    /// `NiTriShape`.
    TriShape(Box<NiGeometry>),
    /// `NiTriStrips`.
    TriStrips(Box<NiGeometry>),
    /// `BSSegmentedTriShape`.
    SegmentedTriShape(Box<BsSegmentedTriShape>),
    /// `NiTriShapeData`.
    TriShapeData(Box<NiTriShapeData>),
    /// `NiTriStripsData`.
    TriStripsData(Box<NiTriStripsData>),
    /// `BSShaderPPLightingProperty`, `Lighting30ShaderProperty`.
    ShaderPpLighting(Box<BsShaderPpLighting>),
    /// `BSShaderNoLightingProperty`.
    ShaderNoLighting(Box<BsShaderNoLighting>),
    /// `BSShaderTextureSet`.
    ShaderTextureSet(Box<BsShaderTextureSet>),
    /// `NiAlphaProperty`.
    AlphaProperty(Box<NiAlphaProperty>),
    /// `NiMaterialProperty`.
    MaterialProperty(Box<NiMaterialProperty>),
    /// `NiStencilProperty`.
    StencilProperty(Box<NiStencilProperty>),
    /// `NiTexturingProperty`.
    TexturingProperty(Box<NiTexturingProperty>),
    /// `NiSourceTexture`.
    SourceTexture(Box<NiSourceTexture>),
    /// `BSXFlags`.
    BsxFlags(Box<BsxFlags>),
    /// `bhkCollisionObject` and the types in [`COLLISION_OBJECT_TYPES`].
    CollisionObject(Box<CollisionObject>),
    /// `bhkRigidBody`, `bhkRigidBodyT`: skipped by size, two bytes peeked.
    RigidBody(RigidBodyPeek),
    /// `NiSkinInstance`, `BSDismemberSkinInstance`.
    SkinInstance(Box<NiSkinInstance>),
    /// `NiSkinData`.
    SkinData(Box<NiSkinData>),
    /// `NiSkinPartition`.
    SkinPartition(Box<NiSkinPartition>),
    /// `NiControllerSequence`.
    ControllerSequence(Box<NiControllerSequence>),
    /// `NiTransformInterpolator`, `BSRotAccumTransfInterpolator`.
    TransformInterpolator(Box<NiTransformInterpolator>),
    /// `NiTransformData`, `NiKeyframeData`.
    TransformData(Box<NiTransformData>),
    /// `NiBSplineTransformInterpolator`, `NiBSplineCompTransformInterpolator`.
    BSplineTransformInterpolator(Box<NiBSplineTransformInterpolator>),
    /// `NiBSplineData`.
    BSplineData(Box<NiBSplineData>),
    /// `NiBSplineBasisData`.
    BSplineBasisData(NiBSplineBasisData),
    /// `NiTextKeyExtraData`.
    TextKeyExtraData(Box<NiTextKeyExtraData>),
    /// Any other type: skipped by size.
    Skipped,
}

impl Block {
    /// Whether the block was skipped by size instead of parsed field by
    /// field. Such blocks are counted in `StaticMesh::skipped_blocks`.
    pub fn is_skipped(&self) -> bool {
        matches!(self, Block::Skipped | Block::RigidBody(_))
    }

    /// The shape layout of a `NiTriShape`, `NiTriStrips` or
    /// `BSSegmentedTriShape` block.
    pub fn as_shape(&self) -> Option<&NiGeometry> {
        match self {
            Block::TriShape(geometry) | Block::TriStrips(geometry) => Some(geometry),
            Block::SegmentedTriShape(segmented) => Some(&segmented.shape),
            _ => None,
        }
    }

    /// A `NiTriShape` block.
    pub fn tri_shape(geometry: NiGeometry) -> Self {
        Block::TriShape(Box::new(geometry))
    }

    /// A `NiTriStrips` block.
    pub fn tri_strips(geometry: NiGeometry) -> Self {
        Block::TriStrips(Box::new(geometry))
    }
}

macro_rules! block_from {
    ($($structure:ty => $variant:ident),* $(,)?) => {
        $(impl From<$structure> for Block {
            fn from(value: $structure) -> Self {
                Block::$variant(Box::new(value))
            }
        })*
    };
}

block_from! {
    NiNode => Node,
    BsSegmentedTriShape => SegmentedTriShape,
    NiTexturingProperty => TexturingProperty,
    NiSourceTexture => SourceTexture,
    NiTriShapeData => TriShapeData,
    NiTriStripsData => TriStripsData,
    BsShaderPpLighting => ShaderPpLighting,
    BsShaderNoLighting => ShaderNoLighting,
    BsShaderTextureSet => ShaderTextureSet,
    NiAlphaProperty => AlphaProperty,
    NiMaterialProperty => MaterialProperty,
    NiStencilProperty => StencilProperty,
    BsxFlags => BsxFlags,
    CollisionObject => CollisionObject,
    NiSkinInstance => SkinInstance,
    NiSkinData => SkinData,
    NiSkinPartition => SkinPartition,
    NiControllerSequence => ControllerSequence,
    NiTransformInterpolator => TransformInterpolator,
    NiTransformData => TransformData,
    NiBSplineTransformInterpolator => BSplineTransformInterpolator,
    NiBSplineData => BSplineData,
    NiTextKeyExtraData => TextKeyExtraData,
}

impl From<NiBSplineBasisData> for Block {
    fn from(value: NiBSplineBasisData) -> Self {
        Block::BSplineBasisData(value)
    }
}

/// Block types that animate: the mesh extraction never reads them, so a
/// [`crate::StaticMesh`] counts them with the skipped blocks.
pub const ANIMATION_TYPES: &[&str] = &[
    "BSRotAccumTransfInterpolator",
    "NiBSplineBasisData",
    "NiBSplineCompTransformInterpolator",
    "NiBSplineData",
    "NiBSplineTransformInterpolator",
    "NiControllerSequence",
    "NiKeyframeData",
    "NiTextKeyExtraData",
    "NiTransformData",
    "NiTransformInterpolator",
];

/// Skin block types. A [`crate::StaticMesh`] counts those that no emitted
/// primitive's skin uses with the skipped blocks.
pub const SKIN_TYPES: &[&str] = &[
    "BSDismemberSkinInstance",
    "NiSkinData",
    "NiSkinInstance",
    "NiSkinPartition",
];

/// Node types parsed with the exact `NiNode` layout and a strict size check.
pub const NODE_EXACT_TYPES: &[&str] = &["NiNode", "BSFadeNode"];

/// Node types that start with the `NiNode` layout and add fields after it.
/// The common part is parsed; the rest is kept raw in [`NiNode::extra`].
pub const NODE_PREFIX_TYPES: &[&str] = &[
    "BSBlastNode",
    "BSDamageStage",
    "BSDebrisNode",
    "BSLeafAnimNode",
    "BSMasterParticleSystem",
    "BSMultiBoundNode",
    "BSOrderedNode",
    "BSTreeNode",
    "BSValueNode",
    "NiBillboardNode",
    "NiLODNode",
    "NiSwitchNode",
];

/// Collision object types parsed as [`CollisionObject`].
pub const COLLISION_OBJECT_TYPES: &[&str] = &[
    "bhkBlendCollisionObject",
    "bhkCollisionObject",
    "bhkPCollisionObject",
    "bhkSPCollisionObject",
];

/// Rigid body types peeked as [`RigidBodyPeek`].
pub const RIGID_BODY_TYPES: &[&str] = &["bhkRigidBody", "bhkRigidBodyT"];

fn object_net(r: &mut Reader<'_>) -> Result<ObjectNet, NifError> {
    Ok(ObjectNet {
        name: r.string("name")?,
        extra_data: r.ref_array("extra data list")?,
        controller: r.reference("controller")?,
    })
}

fn av_object(r: &mut Reader<'_>) -> Result<AvObject, NifError> {
    Ok(AvObject {
        net: object_net(r)?,
        flags: r.u32("object flags")?,
        translation: r.vec3("translation")?,
        rotation: r.matrix33("rotation")?,
        scale: r.f32("scale")?,
        properties: r.ref_array("property list")?,
        collision_object: r.reference("collision object")?,
    })
}

fn node(r: &mut Reader<'_>, exact: bool) -> Result<NiNode, NifError> {
    Ok(NiNode {
        av: av_object(r)?,
        children: r.ref_array("child list")?,
        effects: r.ref_array("effect list")?,
        extra: if exact { Vec::new() } else { r.rest() },
    })
}

fn geometry(r: &mut Reader<'_>) -> Result<NiGeometry, NifError> {
    let av = av_object(r)?;
    let data = r.reference("geometry data")?;
    let skin_instance = r.reference("skin instance")?;
    // Each material is a four-byte name and a four-byte extra data value.
    let num_materials = r.count_u32(8, "material list")?;
    let mut material_names = Vec::with_capacity(num_materials);
    for _ in 0..num_materials {
        material_names.push(r.string("material name")?);
    }
    let material_extra_data = r.array(num_materials, 4, "material extra data", Reader::i32)?;
    Ok(NiGeometry {
        av,
        data,
        skin_instance,
        material_names,
        material_extra_data,
        active_material: r.i32("active material")?,
        dirty: r.bool("material update flag")?,
    })
}

fn segmented_tri_shape(r: &mut Reader<'_>) -> Result<BsSegmentedTriShape, NifError> {
    let shape = geometry(r)?;
    let count = r.count_u32(9, "segment list")?;
    let mut segments = Vec::with_capacity(count);
    for _ in 0..count {
        segments.push(TriSegment {
            flags: r.u8("segment flags")?,
            start: r.u32("segment start")?,
            triangles: r.u32("segment triangle count")?,
        });
    }
    Ok(BsSegmentedTriShape { shape, segments })
}

fn texturing_property(r: &mut Reader<'_>) -> Result<NiTexturingProperty, NifError> {
    let net = object_net(r)?;
    let flags = r.u16("texturing flags")?;
    let texture_count = r.u32("texture count")?;
    let has_base_texture = r.bool("has base texture")?;
    let base_source = if has_base_texture {
        r.reference("base texture source")?
    } else {
        None
    };
    Ok(NiTexturingProperty {
        net,
        flags,
        texture_count,
        has_base_texture,
        base_source,
        extra: r.rest(),
    })
}

fn source_texture(r: &mut Reader<'_>) -> Result<NiSourceTexture, NifError> {
    Ok(NiSourceTexture {
        net: object_net(r)?,
        use_external: r.u8("use external")?,
        file_name: r.string("texture file name")?,
        link: r.reference("texture link")?,
        pixel_layout: r.u32("pixel layout")?,
        use_mipmaps: r.u32("mipmap use")?,
        alpha_format: r.u32("alpha format")?,
        is_static: r.u8("is static")?,
        direct_render: r.u8("direct render")?,
        persist_render_data: r.u8("persist render data")?,
    })
}

fn geometry_data(r: &mut Reader<'_>) -> Result<GeometryData, NifError> {
    let group_id = r.i32("group id")?;
    let num_vertices = r.u16("vertex count")?;
    let n = usize::from(num_vertices);
    let keep_flags = r.u8("keep flags")?;
    let compress_flags = r.u8("compress flags")?;
    let vertices = if r.bool("has vertices")? {
        Some(r.array(n, 12, "vertices", Reader::vec3)?)
    } else {
        None
    };
    let vector_flags = r.u16("vector flags")?;
    let has_normals = r.bool("has normals")?;
    let normals = if has_normals {
        Some(r.array(n, 12, "normals", Reader::vec3)?)
    } else {
        None
    };
    let (tangents, bitangents) = if has_normals && vector_flags & VECTOR_FLAGS_TANGENTS != 0 {
        (
            Some(r.array(n, 12, "tangents", Reader::vec3)?),
            Some(r.array(n, 12, "bitangents", Reader::vec3)?),
        )
    } else {
        (None, None)
    };
    let center = r.vec3("bounding sphere centre")?;
    let radius = r.f32("bounding sphere radius")?;
    let colors = if r.bool("has vertex colours")? {
        Some(r.array(n, 16, "vertex colours", Reader::vec4)?)
    } else {
        None
    };
    let mut uv_sets = Vec::new();
    if vector_flags & VECTOR_FLAGS_UV != 0 {
        uv_sets.push(r.array(n, 8, "UV set", Reader::vec2)?);
    }
    Ok(GeometryData {
        group_id,
        num_vertices,
        keep_flags,
        compress_flags,
        vertices,
        vector_flags,
        normals,
        tangents,
        bitangents,
        center,
        radius,
        colors,
        uv_sets,
        consistency_flags: r.u16("consistency flags")?,
        additional_data: r.reference("additional geometry data")?,
    })
}

fn tri_shape_data(r: &mut Reader<'_>) -> Result<NiTriShapeData, NifError> {
    let geom = geometry_data(r)?;
    let num_triangles = r.u16("triangle count")?;
    let num_triangle_points = r.u32("triangle point count")?;
    let triangles = if r.bool("has triangles")? {
        Some(
            r.array(usize::from(num_triangles), 6, "triangles", |r, what| {
                Ok([r.u16(what)?, r.u16(what)?, r.u16(what)?])
            })?,
        )
    } else {
        None
    };
    let num_groups = r.count_u16(2, "match groups")?;
    let mut match_groups = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        let len = r.count_u16(2, "match group")?;
        match_groups.push(r.u16_array(len, "match group")?);
    }
    Ok(NiTriShapeData {
        geom,
        num_triangles,
        num_triangle_points,
        triangles,
        match_groups,
    })
}

fn tri_strips_data(r: &mut Reader<'_>) -> Result<NiTriStripsData, NifError> {
    let geom = geometry_data(r)?;
    let num_triangles = r.u16("triangle count")?;
    let num_strips = r.count_u16(2, "strip lengths")?;
    let strip_lengths = r.u16_array(num_strips, "strip lengths")?;
    let strips = if r.bool("has strip points")? {
        let mut strips = Vec::with_capacity(strip_lengths.len());
        for &len in &strip_lengths {
            strips.push(r.u16_array(usize::from(len), "strip points")?);
        }
        Some(strips)
    } else {
        None
    };
    Ok(NiTriStripsData {
        geom,
        num_triangles,
        strip_lengths,
        strips,
    })
}

fn shader_property(r: &mut Reader<'_>) -> Result<ShaderProperty, NifError> {
    Ok(ShaderProperty {
        net: object_net(r)?,
        flags: r.u16("property flags")?,
        shader_type: r.u32("shader type")?,
        shader_flags: r.u32("shader flags")?,
        shader_flags2: r.u32("shader flags 2")?,
        env_map_scale: r.f32("environment map scale")?,
        texture_clamp_mode: r.u32("texture clamp mode")?,
    })
}

fn shader_pp_lighting(r: &mut Reader<'_>) -> Result<BsShaderPpLighting, NifError> {
    Ok(BsShaderPpLighting {
        shader: shader_property(r)?,
        texture_set: r.reference("texture set")?,
        refraction_strength: r.f32("refraction strength")?,
        refraction_fire_period: r.i32("refraction fire period")?,
        parallax_max_passes: r.f32("parallax max passes")?,
        parallax_scale: r.f32("parallax scale")?,
    })
}

fn shader_no_lighting(r: &mut Reader<'_>) -> Result<BsShaderNoLighting, NifError> {
    Ok(BsShaderNoLighting {
        shader: shader_property(r)?,
        file_name: r.sized_string("texture file name")?,
        extra: r.rest(),
    })
}

fn texture_set(r: &mut Reader<'_>) -> Result<BsShaderTextureSet, NifError> {
    let count = r.count_u32(4, "texture list")?;
    let mut textures = Vec::with_capacity(count);
    for _ in 0..count {
        textures.push(r.sized_string("texture path")?);
    }
    Ok(BsShaderTextureSet { textures })
}

fn alpha_property(r: &mut Reader<'_>) -> Result<NiAlphaProperty, NifError> {
    Ok(NiAlphaProperty {
        net: object_net(r)?,
        flags: r.u16("alpha flags")?,
        threshold: r.u8("alpha threshold")?,
    })
}

fn material_property(r: &mut Reader<'_>) -> Result<NiMaterialProperty, NifError> {
    Ok(NiMaterialProperty {
        net: object_net(r)?,
        specular: r.vec3("specular colour")?,
        emissive: r.vec3("emissive colour")?,
        glossiness: r.f32("glossiness")?,
        alpha: r.f32("material alpha")?,
        emissive_multiple: r.f32("emissive multiplier")?,
    })
}

fn stencil_property(r: &mut Reader<'_>) -> Result<NiStencilProperty, NifError> {
    Ok(NiStencilProperty {
        net: object_net(r)?,
        flags: r.u16("stencil flags")?,
        stencil_ref: r.u32("stencil reference")?,
        stencil_mask: r.u32("stencil mask")?,
    })
}

fn bsx_flags(r: &mut Reader<'_>) -> Result<BsxFlags, NifError> {
    Ok(BsxFlags {
        name: r.string("extra data name")?,
        flags: r.u32("BSX flags")?,
    })
}

fn collision_object(r: &mut Reader<'_>, exact: bool) -> Result<CollisionObject, NifError> {
    Ok(CollisionObject {
        target: r.reference("collision target")?,
        flags: r.u16("collision flags")?,
        body: r.reference("collision body")?,
        extra: if exact { Vec::new() } else { r.rest() },
    })
}

fn rigid_body_peek(r: &mut Reader<'_>) -> RigidBodyPeek {
    let bytes = r.rest();
    RigidBodyPeek {
        layer: bytes.get(RIGID_BODY_LAYER_OFFSET).copied(),
        motion_system: bytes.get(RIGID_BODY_MOTION_OFFSET).copied(),
    }
}

/// Parses one block from a reader limited to the block's declared size.
/// The caller compares the bytes consumed with the declared size for every
/// block that is not [`Block::is_skipped`].
pub(crate) fn parse_block(type_name: &str, r: &mut Reader<'_>) -> Result<Block, NifError> {
    Ok(match type_name {
        "NiTriShape" => Block::tri_shape(geometry(r)?),
        "NiTriStrips" => Block::tri_strips(geometry(r)?),
        "BSSegmentedTriShape" => segmented_tri_shape(r)?.into(),
        "NiTexturingProperty" => texturing_property(r)?.into(),
        "NiSourceTexture" => source_texture(r)?.into(),
        "NiTriShapeData" => tri_shape_data(r)?.into(),
        "NiTriStripsData" => tri_strips_data(r)?.into(),
        "BSShaderPPLightingProperty" | "Lighting30ShaderProperty" => shader_pp_lighting(r)?.into(),
        "BSShaderNoLightingProperty" => shader_no_lighting(r)?.into(),
        "BSShaderTextureSet" => texture_set(r)?.into(),
        "NiAlphaProperty" => alpha_property(r)?.into(),
        "NiMaterialProperty" => material_property(r)?.into(),
        "NiStencilProperty" => stencil_property(r)?.into(),
        "BSXFlags" => bsx_flags(r)?.into(),
        "NiSkinInstance" => skin_blocks::skin_instance(r, false)?.into(),
        "BSDismemberSkinInstance" => skin_blocks::skin_instance(r, true)?.into(),
        "NiSkinData" => skin_blocks::skin_data(r)?.into(),
        "NiSkinPartition" => skin_blocks::skin_partition(r)?.into(),
        "NiControllerSequence" => anim_blocks::controller_sequence(r)?.into(),
        "NiTransformInterpolator" | "BSRotAccumTransfInterpolator" => {
            anim_blocks::transform_interpolator(r)?.into()
        }
        "NiTransformData" | "NiKeyframeData" => anim_blocks::transform_data(r)?.into(),
        "NiBSplineTransformInterpolator" => {
            anim_blocks::bspline_transform_interpolator(r, false)?.into()
        }
        "NiBSplineCompTransformInterpolator" => {
            anim_blocks::bspline_transform_interpolator(r, true)?.into()
        }
        "NiBSplineData" => anim_blocks::bspline_data(r)?.into(),
        "NiBSplineBasisData" => anim_blocks::bspline_basis_data(r)?.into(),
        "NiTextKeyExtraData" => anim_blocks::text_key_extra_data(r)?.into(),
        "bhkBlendCollisionObject" => collision_object(r, false)?.into(),
        name if NODE_EXACT_TYPES.contains(&name) => node(r, true)?.into(),
        name if NODE_PREFIX_TYPES.contains(&name) => node(r, false)?.into(),
        name if COLLISION_OBJECT_TYPES.contains(&name) => collision_object(r, true)?.into(),
        name if RIGID_BODY_TYPES.contains(&name) => Block::RigidBody(rigid_body_peek(r)),
        _ => Block::Skipped,
    })
}
