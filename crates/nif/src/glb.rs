//! The mesh interchange format of `docs/contracts.md` section 3: glTF 2.0
//! binary, written and read here and nowhere else.
//!
//! Layout of a written file:
//!
//! - one scene, one root node (`nodes[0]`), one mesh; primitive `i` of the
//!   mesh is `StaticMesh::primitives[i]` and uses `materials[i]`;
//! - positions and normals are converted from source space `(x, y, z)` to
//!   glTF's Y-up `(x, z, -y)`; units stay game units, and `asset.extras`
//!   says so;
//! - each attribute and each index list has its own buffer view, aligned to
//!   four bytes, in the single binary chunk;
//! - what glTF has no field for travels in `extras.mojave`: the primitive's
//!   name and skinned flag on the primitive, texture paths and shader fields on the
//!   material, collision metadata and the skipped block counts on
//!   `nodes[0]`.
//!
//! When any primitive has a [`Skin`], the file also has one glTF skin:
//!
//! - `nodes[0]` gets `"skin":0` and one child, `nodes[1]` ("skeleton"),
//!   whose children are the joint nodes `nodes[2..]`, each named after its
//!   bone and placed (`matrix`) at its bind transform, so that the file
//!   shows the mesh as stored;
//! - the joints are every primitive's bones; two primitives share a joint
//!   when the bone's name and bind matrix are bit for bit the same;
//! - `inverseBindMatrices` holds each joint's [`SkinBone::bind_inverse`]
//!   converted to glTF axes (`B · A⁻¹`, `A` the axis change above);
//! - each primitive has `JOINTS_0` (`u16` × 4) and `WEIGHTS_0` (`f32` × 4);
//!   `extras.mojave.skin_bones` lists the joint of each of its own bones,
//!   in order, which is how [`read`] recovers its bone list and indices;
//! - a primitive without a skin in such a file is bound to an extra joint,
//!   [`RIGID_JOINT`], with weight 1 and has no `skin_bones`; the reader
//!   gives it no skin.
//!
//! A mesh without any skin is written exactly as before skins existed.
//!
//! The JSON is assembled by hand with a fixed key order, so the bytes for
//! one mesh are always the same.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Deserialize;

use crate::error::NifError;
use crate::math::Matrix34;
use crate::mesh::{AlphaMode, CollisionInfo, Material, Primitive, ShaderInfo, StaticMesh};
use crate::skin::{Skin, SkinBone};

const MAGIC: u32 = 0x4654_6C67; // "glTF"
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"
const HEADER_LEN: usize = 12;
const CHUNK_HEADER_LEN: usize = 8;

const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const UNSIGNED_SHORT: u32 = 5123;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const MODE_TRIANGLES: u32 = 4;

/// The `asset.generator` string of every written file.
pub const GENERATOR: &str = "mojave-foundry nif";

/// Source space `(x, y, z)` to glTF space `(x, z, -y)`.
pub fn to_gltf_axes(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

/// glTF space back to source space: `(x, y, z)` to `(x, -z, y)`.
pub fn from_gltf_axes(v: [f32; 3]) -> [f32; 3] {
    [v[0], -v[2], v[1]]
}

// ---------------------------------------------------------------- writing

fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Shortest decimal text that reads back as the same `f32`. JSON has no
/// infinities or NaN; those are written as 0.
fn push_json_f32(out: &mut String, v: f32) {
    if v.is_finite() {
        let _ = write!(out, "{v}");
    } else {
        out.push('0');
    }
}

fn push_json_f32_list(out: &mut String, values: &[f32]) {
    out.push('[');
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_json_f32(out, *v);
    }
    out.push(']');
}

fn push_json_opt_u8(out: &mut String, v: Option<u8>) {
    match v {
        Some(v) => {
            let _ = write!(out, "{v}");
        }
        None => out.push_str("null"),
    }
}

fn join(items: &[String]) -> String {
    items.join(",")
}

/// The binary chunk under construction, with its views and accessors.
#[derive(Default)]
struct Layout {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl Layout {
    /// Appends a float attribute and returns its accessor index.
    fn add_floats(
        &mut self,
        values: &[f32],
        count: usize,
        kind: &str,
        bounds: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        let offset = self.align();
        for v in values {
            self.bin.extend_from_slice(&v.to_le_bytes());
        }
        self.finish(offset, count, FLOAT, Some(ARRAY_BUFFER), kind, bounds)
    }

    /// Appends inverse bind matrices (`MAT4`, no buffer view target) and
    /// returns the accessor index.
    fn add_matrices(&mut self, values: &[f32], count: usize) -> usize {
        let offset = self.align();
        for v in values {
            self.bin.extend_from_slice(&v.to_le_bytes());
        }
        self.finish(offset, count, FLOAT, None, "MAT4", None)
    }

    /// Appends joint indices (`VEC4` of `u16`) and returns the accessor
    /// index.
    fn add_joints(&mut self, values: &[[u16; 4]]) -> usize {
        let offset = self.align();
        for v in values.iter().flatten() {
            self.bin.extend_from_slice(&v.to_le_bytes());
        }
        self.finish(
            offset,
            values.len(),
            UNSIGNED_SHORT,
            Some(ARRAY_BUFFER),
            "VEC4",
            None,
        )
    }

    /// Appends an index list and returns its accessor index.
    fn add_indices(&mut self, values: &[u32]) -> usize {
        let offset = self.align();
        for v in values {
            self.bin.extend_from_slice(&v.to_le_bytes());
        }
        let count = values.len();
        self.finish(
            offset,
            count,
            UNSIGNED_INT,
            Some(ELEMENT_ARRAY_BUFFER),
            "SCALAR",
            None,
        )
    }

    /// Pads the binary chunk to a four-byte boundary and returns the offset
    /// the next buffer view starts at.
    fn align(&mut self) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.bin.len()
    }

    /// Records the buffer view and the accessor for the bytes appended
    /// since `offset`: each array has a view and an accessor of its own.
    fn finish(
        &mut self,
        offset: usize,
        count: usize,
        component: u32,
        target: Option<u32>,
        kind: &str,
        bounds: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        let length = self.bin.len() - offset;
        let view = self.views.len();
        let mut json = format!("{{\"buffer\":0,\"byteOffset\":{offset},\"byteLength\":{length}");
        if let Some(target) = target {
            let _ = write!(json, ",\"target\":{target}");
        }
        json.push('}');
        self.views.push(json);
        let mut accessor = format!(
            "{{\"bufferView\":{view},\"byteOffset\":0,\"componentType\":{component},\
             \"count\":{count},\"type\":\"{kind}\""
        );
        if let Some((min, max)) = bounds {
            accessor.push_str(",\"min\":");
            push_json_f32_list(&mut accessor, &min);
            accessor.push_str(",\"max\":");
            push_json_f32_list(&mut accessor, &max);
        }
        accessor.push('}');
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }
}

/// Component-wise minimum and maximum, as `POSITION` accessors must state
/// them. Written with plain comparisons and with negative zero turned into
/// zero, so that the JSON text does not depend on how a platform orders the
/// two zeros.
fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in points {
        for ((lo, hi), &v) in min.iter_mut().zip(max.iter_mut()).zip(p) {
            if v.is_finite() {
                if v < *lo {
                    *lo = v;
                }
                if v > *hi {
                    *hi = v;
                }
            }
        }
    }
    for (lo, hi) in min.iter_mut().zip(max.iter_mut()) {
        if *lo > *hi {
            // No finite value on this axis.
            *lo = 0.0;
            *hi = 0.0;
        }
        *lo += 0.0;
        *hi += 0.0;
    }
    (min, max)
}

/// Bind inverse (source axes) to glTF axes: `B · A⁻¹`, where `A` maps
/// `(x, y, z)` to `(x, z, -y)`. Done by moving columns, so that it is exact
/// and [`bind_from_gltf`] undoes it bit for bit.
fn bind_to_gltf(b: &Matrix34) -> Matrix34 {
    b.map(|row| [row[0], row[2], -row[1], row[3]])
}

/// Inverse of [`bind_to_gltf`]: `G · A`.
fn bind_from_gltf(g: &Matrix34) -> Matrix34 {
    g.map(|row| [row[0], -row[2], row[1], row[3]])
}

/// A 3 × 4 matrix as glTF's column-major sixteen floats.
fn column_major(m: &Matrix34) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for c in 0..4 {
        for r in 0..3 {
            out[c * 4 + r] = m[r][c];
        }
    }
    out[15] = 1.0;
    out
}

/// Name of the joint that primitives without a skin are bound to in a file
/// whose mesh has skinned primitives (glTF wants every primitive of a
/// skinned mesh to have joints). The reader drops it again.
pub const RIGID_JOINT: &str = "mojave:rigid";

/// The joints of the file's one skin: every primitive's bones, a bone of
/// two primitives shared when its name and bind matrix are the same.
#[derive(Default)]
struct Joints {
    names: Vec<String>,
    /// Inverse bind matrices in glTF axes.
    inverse_binds: Vec<Matrix34>,
    /// Per written primitive: the joint of each of its bones.
    maps: Vec<Option<Vec<u16>>>,
    /// The joint for primitives without a skin, when needed.
    rigid: Option<u16>,
}

impl Joints {
    fn build(primitives: &[&Primitive]) -> Joints {
        let mut joints = Joints::default();
        let mut index: BTreeMap<(String, [u32; 12]), Vec<u16>> = BTreeMap::new();
        for primitive in primitives {
            let Some(skin) = &primitive.skin else {
                joints.maps.push(None);
                continue;
            };
            if joints.names.len() + skin.bones.len() >= usize::from(u16::MAX) {
                // More joints than a 16-bit index can name: written unskinned.
                joints.maps.push(None);
                continue;
            }
            let mut map = Vec::with_capacity(skin.bones.len());
            // The k-th bone of this primitive with a given name and matrix
            // takes the k-th joint made for them, so that no joint serves
            // two bones of one primitive.
            let mut seen: BTreeMap<(String, [u32; 12]), usize> = BTreeMap::new();
            for bone in &skin.bones {
                let matrix = bind_to_gltf(&bone.bind_inverse);
                let bits: [u32; 12] = std::array::from_fn(|i| matrix[i / 4][i % 4].to_bits());
                let key = (bone.name.clone(), bits);
                let occurrence = seen.entry(key.clone()).or_insert(0);
                let k = *occurrence;
                *occurrence += 1;
                let candidates = index.entry(key).or_default();
                let joint = match candidates.get(k) {
                    Some(&j) => j,
                    None => {
                        let j = joints.names.len() as u16;
                        joints.names.push(bone.name.clone());
                        joints.inverse_binds.push(matrix);
                        candidates.push(j);
                        j
                    }
                };
                map.push(joint);
            }
            joints.maps.push(Some(map));
        }
        let any_skin = joints.maps.iter().any(Option::is_some);
        if any_skin && joints.maps.iter().any(Option::is_none) {
            joints.rigid = Some(joints.names.len() as u16);
            joints.names.push(RIGID_JOINT.to_owned());
            joints.inverse_binds.push(crate::math::IDENTITY34);
        }
        joints
    }

    fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Joint indices and weights, one row of four per vertex.
type Influences = (Vec<[u16; 4]>, Vec<[f32; 4]>);

fn primitive_json(
    layout: &mut Layout,
    index: usize,
    primitive: &Primitive,
    joint_map: Option<&[u16]>,
    rigid: Option<u16>,
) -> String {
    let count = primitive.positions.len();
    let positions: Vec<[f32; 3]> = primitive
        .positions
        .iter()
        .map(|&p| to_gltf_axes(p))
        .collect();
    let position = layout.add_floats(
        positions.as_flattened(),
        count,
        "VEC3",
        Some(bounds(&positions)),
    );
    let mut attributes = format!("\"POSITION\":{position}");
    if let Some(normals) = &primitive.normals {
        let normals: Vec<[f32; 3]> = normals.iter().map(|&n| to_gltf_axes(n)).collect();
        let accessor = layout.add_floats(normals.as_flattened(), count, "VEC3", None);
        let _ = write!(attributes, ",\"NORMAL\":{accessor}");
    }
    if let Some(uvs) = &primitive.uvs {
        let accessor = layout.add_floats(uvs.as_flattened(), count, "VEC2", None);
        let _ = write!(attributes, ",\"TEXCOORD_0\":{accessor}");
    }
    if let Some(colors) = &primitive.colors {
        let accessor = layout.add_floats(colors.as_flattened(), count, "VEC4", None);
        let _ = write!(attributes, ",\"COLOR_0\":{accessor}");
    }
    let skin = primitive.skin.as_ref().zip(joint_map);
    let influences: Option<Influences> = match (skin, rigid) {
        (Some((skin, map)), _) => Some(
            skin.weights
                .iter()
                .map(|row| {
                    let joint = |k: usize| map.get(usize::from(row[k].0)).copied().unwrap_or(0);
                    (
                        [joint(0), joint(1), joint(2), joint(3)],
                        [row[0].1, row[1].1, row[2].1, row[3].1],
                    )
                })
                .unzip(),
        ),
        (None, Some(rigid)) => Some((vec![[rigid; 4]; count], vec![[1.0, 0.0, 0.0, 0.0]; count])),
        (None, None) => None,
    };
    if let Some((joints, weights)) = influences {
        let accessor = layout.add_joints(&joints);
        let _ = write!(attributes, ",\"JOINTS_0\":{accessor}");
        let accessor = layout.add_floats(weights.as_flattened(), count, "VEC4", None);
        let _ = write!(attributes, ",\"WEIGHTS_0\":{accessor}");
    }
    let indices = layout.add_indices(&primitive.indices);
    let mut json = format!(
        "{{\"attributes\":{{{attributes}}},\"indices\":{indices},\"material\":{index},\
         \"mode\":{MODE_TRIANGLES},\"extras\":{{\"mojave\":{{\"name\":"
    );
    push_json_string(&mut json, &primitive.name);
    let _ = write!(json, ",\"skinned\":{}", primitive.skinned);
    if let Some((_, map)) = skin {
        json.push_str(",\"skin_bones\":[");
        for (i, joint) in map.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            let _ = write!(json, "{joint}");
        }
        json.push(']');
    }
    json.push_str("}}}");
    json
}

fn material_json(material: &Material) -> String {
    let mut json =
        String::from("{\"pbrMetallicRoughness\":{\"metallicFactor\":0,\"roughnessFactor\":1}");
    match material.alpha {
        AlphaMode::Opaque => json.push_str(",\"alphaMode\":\"OPAQUE\""),
        AlphaMode::Mask { cutoff } => {
            json.push_str(",\"alphaMode\":\"MASK\",\"alphaCutoff\":");
            push_json_f32(&mut json, cutoff);
        }
        AlphaMode::Blend => json.push_str(",\"alphaMode\":\"BLEND\""),
    }
    let _ = write!(json, ",\"doubleSided\":{}", material.two_sided);
    json.push_str(",\"extras\":{\"mojave\":{\"textures\":[");
    for (i, texture) in material.textures.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        push_json_string(&mut json, texture);
    }
    json.push_str("],\"shader\":");
    match material.shader {
        Some(shader) => {
            let _ = write!(
                json,
                "{{\"unlit\":{},\"type\":{},\"flags\":{},\"flags2\":{}}}",
                shader.unlit, shader.shader_type, shader.flags, shader.flags2
            );
        }
        None => json.push_str("null"),
    }
    json.push_str(",\"alpha_flags\":");
    match material.alpha_flags {
        Some(flags) => {
            let _ = write!(json, "{flags}");
        }
        None => json.push_str("null"),
    }
    json.push_str(",\"opacity\":");
    push_json_f32(&mut json, material.opacity);
    json.push_str(",\"emissive\":");
    push_json_f32_list(&mut json, &material.emissive);
    json.push_str(",\"emissive_multiple\":");
    push_json_f32(&mut json, material.emissive_multiple);
    json.push_str("}}}");
    json
}

fn node_json(mesh: &StaticMesh, has_mesh: bool, skinned: bool) -> String {
    let mut json = String::from("{\"name\":\"mesh\"");
    if has_mesh {
        json.push_str(",\"mesh\":0");
    }
    if skinned {
        json.push_str(",\"skin\":0,\"children\":[1]");
    }
    json.push_str(",\"extras\":{\"mojave\":{");
    if let Some(collision) = &mesh.collision {
        json.push_str("\"collision\":{\"kind\":");
        push_json_string(&mut json, &collision.kind);
        json.push_str(",\"target\":");
        push_json_string(&mut json, &collision.target);
        let _ = write!(json, ",\"flags\":{},\"body\":", collision.flags);
        push_json_string(&mut json, &collision.body);
        json.push_str(",\"layer\":");
        push_json_opt_u8(&mut json, collision.layer);
        json.push_str(",\"motion_system\":");
        push_json_opt_u8(&mut json, collision.motion_system);
        let _ = write!(json, ",\"count\":{}}},", collision.count);
    }
    json.push_str("\"skipped_blocks\":{");
    for (i, (name, count)) in mesh.skipped_blocks.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        push_json_string(&mut json, name);
        let _ = write!(json, ":{count}");
    }
    json.push_str("}}}}");
    json
}

fn push_chunk(out: &mut Vec<u8>, kind: u32, data: &[u8], pad: u8) {
    let padded = data.len().div_ceil(4) * 4;
    out.extend_from_slice(&u32::try_from(padded).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(data);
    out.resize(out.len() + (padded - data.len()), pad);
}

/// Writes a mesh as glTF 2.0 binary.
///
/// A primitive that is not [`Primitive::is_well_formed`] cannot be a glTF
/// primitive and is left out; [`crate::extract_mesh`] never produces one.
/// The bytes depend on the mesh alone.
pub fn write(mesh: &StaticMesh) -> Vec<u8> {
    let mut layout = Layout::default();
    let mut primitives = Vec::new();
    let mut materials = Vec::new();
    let written: Vec<&Primitive> = mesh
        .primitives
        .iter()
        .filter(|p| p.is_well_formed())
        .collect();
    let joints = Joints::build(&written);
    for (primitive, map) in written.iter().zip(&joints.maps) {
        primitives.push(primitive_json(
            &mut layout,
            primitives.len(),
            primitive,
            map.as_deref(),
            joints.rigid,
        ));
        materials.push(material_json(&primitive.material));
    }
    let has_mesh = !primitives.is_empty();
    let skinned = has_mesh && !joints.is_empty();

    let mut nodes = node_json(mesh, has_mesh, skinned);
    let mut skins = String::new();
    if skinned {
        let first = 2usize;
        let _ = write!(nodes, ",{{\"name\":\"skeleton\",\"children\":[");
        for i in 0..joints.names.len() {
            if i > 0 {
                nodes.push(',');
            }
            let _ = write!(nodes, "{}", first + i);
        }
        nodes.push_str("]}");
        for (name, inverse_bind) in joints.names.iter().zip(&joints.inverse_binds) {
            nodes.push_str(",{\"name\":");
            push_json_string(&mut nodes, name);
            // The joint's rest transform is its bind transform, so that the
            // file shows the mesh as stored.
            if let Some(bind) = crate::math::inverse(inverse_bind) {
                nodes.push_str(",\"matrix\":");
                push_json_f32_list(&mut nodes, &column_major(&bind));
            }
            nodes.push('}');
        }
        let flat: Vec<f32> = joints.inverse_binds.iter().flat_map(column_major).collect();
        let accessor = layout.add_matrices(&flat, joints.names.len());
        let _ = write!(
            skins,
            ",\"skins\":[{{\"inverseBindMatrices\":{accessor},\"skeleton\":1,\"joints\":["
        );
        for i in 0..joints.names.len() {
            if i > 0 {
                skins.push(',');
            }
            let _ = write!(skins, "{}", first + i);
        }
        skins.push_str("]}]");
    }

    let mut json = format!(
        "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"{GENERATOR}\",\"extras\":{{\"mojave\":\
         {{\"units\":\"game\",\"source_up\":\"z\"}}}}}},\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\
         \"nodes\":[{nodes}]{skins}"
    );
    if has_mesh {
        let _ = write!(
            json,
            ",\"meshes\":[{{\"name\":\"mesh\",\"primitives\":[{}]}}],\"materials\":[{}],\
             \"accessors\":[{}],\"bufferViews\":[{}],\"buffers\":[{{\"byteLength\":{}}}]",
            join(&primitives),
            join(&materials),
            join(&layout.accessors),
            join(&layout.views),
            layout.bin.len()
        );
    }
    json.push('}');

    let mut out = Vec::with_capacity(HEADER_LEN + json.len() + layout.bin.len() + 24);
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // total length, patched below
    push_chunk(&mut out, CHUNK_JSON, json.as_bytes(), b' ');
    if has_mesh {
        push_chunk(&mut out, CHUNK_BIN, &layout.bin, 0);
    }
    let total = u32::try_from(out.len()).unwrap_or(u32::MAX);
    out[8..12].copy_from_slice(&total.to_le_bytes());
    out
}

// ---------------------------------------------------------------- reading

#[derive(Deserialize)]
struct Root {
    asset: Asset,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    meshes: Vec<Mesh>,
    #[serde(default)]
    materials: Vec<MaterialJson>,
    #[serde(default)]
    accessors: Vec<Accessor>,
    #[serde(default, rename = "bufferViews")]
    buffer_views: Vec<BufferView>,
    #[serde(default)]
    skins: Vec<SkinJson>,
}

#[derive(Deserialize)]
struct SkinJson {
    #[serde(rename = "inverseBindMatrices")]
    inverse_bind_matrices: Option<usize>,
    joints: Vec<usize>,
}

#[derive(Deserialize)]
struct Asset {
    version: String,
    extras: Option<AssetExtras>,
}

#[derive(Deserialize)]
struct AssetExtras {
    mojave: AssetMojave,
}

#[derive(Deserialize)]
struct AssetMojave {
    units: String,
    source_up: String,
}

#[derive(Deserialize)]
struct Node {
    name: Option<String>,
    mesh: Option<usize>,
    skin: Option<usize>,
    extras: Option<NodeExtras>,
}

#[derive(Deserialize)]
struct NodeExtras {
    mojave: NodeMojave,
}

#[derive(Deserialize)]
struct NodeMojave {
    collision: Option<CollisionJson>,
    #[serde(default)]
    skipped_blocks: BTreeMap<String, u32>,
}

#[derive(Deserialize)]
struct CollisionJson {
    kind: String,
    target: String,
    flags: u16,
    body: String,
    layer: Option<u8>,
    motion_system: Option<u8>,
    count: u32,
}

#[derive(Deserialize)]
struct Mesh {
    primitives: Vec<PrimitiveJson>,
}

#[derive(Deserialize)]
struct PrimitiveJson {
    attributes: BTreeMap<String, usize>,
    indices: Option<usize>,
    material: Option<usize>,
    mode: Option<u32>,
    extras: Option<PrimitiveExtras>,
}

#[derive(Deserialize)]
struct PrimitiveExtras {
    mojave: PrimitiveMojave,
}

#[derive(Deserialize)]
struct PrimitiveMojave {
    name: String,
    #[serde(default)]
    skinned: bool,
    skin_bones: Option<Vec<usize>>,
}

#[derive(Deserialize)]
struct MaterialJson {
    #[serde(rename = "alphaMode")]
    alpha_mode: Option<String>,
    #[serde(rename = "alphaCutoff")]
    alpha_cutoff: Option<f32>,
    #[serde(default, rename = "doubleSided")]
    double_sided: bool,
    extras: Option<MaterialExtras>,
}

#[derive(Deserialize)]
struct MaterialExtras {
    mojave: MaterialMojave,
}

#[derive(Deserialize)]
struct MaterialMojave {
    #[serde(default)]
    textures: Vec<String>,
    shader: Option<ShaderJson>,
    alpha_flags: Option<u16>,
    opacity: f32,
    emissive: [f32; 3],
    emissive_multiple: f32,
}

#[derive(Deserialize)]
struct ShaderJson {
    unlit: bool,
    #[serde(rename = "type")]
    shader_type: u32,
    flags: u32,
    flags2: u32,
}

#[derive(Deserialize)]
struct Accessor {
    #[serde(rename = "bufferView")]
    buffer_view: Option<usize>,
    #[serde(default, rename = "byteOffset")]
    byte_offset: usize,
    #[serde(rename = "componentType")]
    component_type: u32,
    count: usize,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct BufferView {
    buffer: usize,
    #[serde(default, rename = "byteOffset")]
    byte_offset: usize,
    #[serde(rename = "byteLength")]
    byte_length: usize,
    #[serde(rename = "byteStride")]
    byte_stride: Option<usize>,
}

fn glb_error(offset: usize, reason: impl Into<String>) -> NifError {
    NifError::Glb {
        offset,
        reason: reason.into(),
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, NifError> {
    match bytes.get(offset..).and_then(|b| b.first_chunk::<4>()) {
        Some(b) => Ok(u32::from_le_bytes(*b)),
        None => Err(glb_error(offset, "truncated: a 32-bit field does not fit")),
    }
}

/// The JSON document, the binary chunk and the binary chunk's file offset.
struct Chunks<'a> {
    json: &'a [u8],
    json_offset: usize,
    bin: &'a [u8],
    bin_offset: usize,
}

fn split_chunks(bytes: &[u8]) -> Result<Chunks<'_>, NifError> {
    if u32_at(bytes, 0)? != MAGIC {
        return Err(glb_error(0, "not a glb file: the magic is not \"glTF\""));
    }
    let version = u32_at(bytes, 4)?;
    if version != 2 {
        return Err(glb_error(4, format!("unsupported glb version {version}")));
    }
    let total = u32_at(bytes, 8)? as usize;
    if total < HEADER_LEN || total > bytes.len() {
        return Err(glb_error(
            8,
            format!(
                "the header says the file is {total} bytes, but {} were given",
                bytes.len()
            ),
        ));
    }
    let bytes = &bytes[..total];

    let mut offset = HEADER_LEN;
    let mut chunks: [Option<(usize, &[u8])>; 2] = [None, None];
    for (slot, expected) in [CHUNK_JSON, CHUNK_BIN].into_iter().enumerate() {
        if offset == bytes.len() {
            break;
        }
        let length = u32_at(bytes, offset)? as usize;
        let kind = u32_at(bytes, offset + 4)?;
        let start = offset + CHUNK_HEADER_LEN;
        let Some(data) = bytes.get(start..).and_then(|b| b.get(..length)) else {
            return Err(glb_error(
                offset,
                format!("a chunk of {length} bytes does not fit in the file"),
            ));
        };
        if kind != expected {
            return Err(glb_error(
                offset + 4,
                format!("chunk {slot} has type {kind:#010x}, expected {expected:#010x}"),
            ));
        }
        chunks[slot] = Some((start, data));
        offset = start + length;
    }
    let Some((json_offset, json)) = chunks[0] else {
        return Err(glb_error(HEADER_LEN, "the JSON chunk is missing"));
    };
    let (bin_offset, bin) = chunks[1].unwrap_or((bytes.len(), &[]));
    Ok(Chunks {
        json,
        json_offset,
        bin,
        bin_offset,
    })
}

struct Document<'a> {
    root: Root,
    chunks: Chunks<'a>,
    /// The skin's joints: name and inverse bind matrix in source axes.
    joints: Vec<(String, Matrix34)>,
}

impl Document<'_> {
    fn json_error(&self, reason: impl Into<String>) -> NifError {
        glb_error(self.chunks.json_offset, reason)
    }

    /// The bytes of accessor `index`, after checking its component type,
    /// element type and every bound; also returns the element count.
    fn accessor_bytes(
        &self,
        index: usize,
        component: u32,
        kind: &str,
        element_size: usize,
    ) -> Result<(&[u8], usize), NifError> {
        let Some(accessor) = self.root.accessors.get(index) else {
            return Err(self.json_error(format!("accessor {index} does not exist")));
        };
        if accessor.component_type != component || accessor.kind != kind {
            return Err(self.json_error(format!(
                "accessor {index} is {} of component type {}, expected {kind} of {component}",
                accessor.kind, accessor.component_type
            )));
        }
        let Some(view_index) = accessor.buffer_view else {
            return Err(self.json_error(format!("accessor {index} has no buffer view")));
        };
        let Some(view) = self.root.buffer_views.get(view_index) else {
            return Err(self.json_error(format!("buffer view {view_index} does not exist")));
        };
        if view.buffer != 0 {
            return Err(self.json_error(format!(
                "buffer view {view_index} uses buffer {}, only the binary chunk is supported",
                view.buffer
            )));
        }
        if view
            .byte_stride
            .is_some_and(|stride| stride != element_size)
        {
            return Err(self.json_error(format!(
                "buffer view {view_index} is interleaved, which is not supported"
            )));
        }
        let Some(view_bytes) = self
            .chunks
            .bin
            .get(view.byte_offset..)
            .and_then(|b| b.get(..view.byte_length))
        else {
            return Err(glb_error(
                self.chunks.bin_offset,
                format!(
                    "buffer view {view_index} ({} bytes at {}) is outside the {}-byte binary chunk",
                    view.byte_length,
                    view.byte_offset,
                    self.chunks.bin.len()
                ),
            ));
        };
        let needed = accessor.count.checked_mul(element_size);
        let data = needed.and_then(|needed| {
            view_bytes
                .get(accessor.byte_offset..)
                .and_then(|b| b.get(..needed))
        });
        match data {
            Some(data) => Ok((data, accessor.count)),
            None => Err(glb_error(
                self.chunks.bin_offset.saturating_add(view.byte_offset),
                format!(
                    "accessor {index} ({} elements of {element_size} bytes) is outside buffer \
                     view {view_index}",
                    accessor.count
                ),
            )),
        }
    }

    fn floats<const N: usize>(&self, index: usize, kind: &str) -> Result<Vec<[f32; N]>, NifError> {
        let (data, count) = self.accessor_bytes(index, FLOAT, kind, N * 4)?;
        let mut values = data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b));
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            // `data` holds exactly `count * N` floats.
            out.push(std::array::from_fn(|_| values.next().unwrap_or(0.0)));
        }
        Ok(out)
    }

    fn joint_indices(&self, index: usize) -> Result<Vec<[u16; 4]>, NifError> {
        let (data, _) = self.accessor_bytes(index, UNSIGNED_SHORT, "VEC4", 8)?;
        Ok(data
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| std::array::from_fn(|k| u16::from_le_bytes([b[2 * k], b[2 * k + 1]])))
            .collect())
    }

    /// The joints of skin `index`.
    fn skin_joints(&self, index: usize) -> Result<Vec<(String, Matrix34)>, NifError> {
        let Some(skin) = self.root.skins.get(index) else {
            return Err(self.json_error(format!("skin {index} does not exist")));
        };
        let Some(accessor) = skin.inverse_bind_matrices else {
            return Err(self.json_error("the skin has no inverse bind matrices"));
        };
        let matrices = self.floats::<16>(accessor, "MAT4")?;
        if matrices.len() != skin.joints.len() {
            return Err(self.json_error(format!(
                "the skin has {} joints but {} inverse bind matrices",
                skin.joints.len(),
                matrices.len()
            )));
        }
        let mut joints = Vec::with_capacity(skin.joints.len());
        for (&node, m) in skin.joints.iter().zip(&matrices) {
            let Some(node) = self.root.nodes.get(node) else {
                return Err(self.json_error(format!("joint node {node} does not exist")));
            };
            let g: Matrix34 = std::array::from_fn(|r| std::array::from_fn(|c| m[c * 4 + r]));
            joints.push((node.name.clone().unwrap_or_default(), bind_from_gltf(&g)));
        }
        Ok(joints)
    }

    fn primitive_skin(
        &self,
        number: usize,
        json: &PrimitiveJson,
        skin_bones: &[usize],
        count: usize,
    ) -> Result<Skin, NifError> {
        let error = |reason: String| self.json_error(format!("primitive {number}: {reason}"));
        let mut bones = Vec::with_capacity(skin_bones.len());
        let mut local_of: BTreeMap<usize, u16> = BTreeMap::new();
        for (local, &joint) in skin_bones.iter().enumerate() {
            let Some((name, bind_inverse)) = self.joints.get(joint) else {
                return Err(error(format!(
                    "skin bone {local} names joint {joint}, which does not exist"
                )));
            };
            let Ok(local16) = u16::try_from(local) else {
                return Err(error("more than 65536 skin bones".to_owned()));
            };
            if local_of.insert(joint, local16).is_some() {
                return Err(error(format!("joint {joint} is listed twice")));
            }
            bones.push(SkinBone {
                name: name.clone(),
                bind_inverse: *bind_inverse,
            });
        }
        let (Some(&joints), Some(&weights)) = (
            json.attributes.get("JOINTS_0"),
            json.attributes.get("WEIGHTS_0"),
        ) else {
            return Err(error(
                "a skinned primitive needs JOINTS_0 and WEIGHTS_0".to_owned(),
            ));
        };
        let joints = self.joint_indices(joints)?;
        let weights = self.floats::<4>(weights, "VEC4")?;
        if joints.len() != count || weights.len() != count {
            return Err(error(format!(
                "JOINTS_0 has {} and WEIGHTS_0 {} elements, POSITION has {count}",
                joints.len(),
                weights.len()
            )));
        }
        let mut rows = Vec::with_capacity(count);
        for (j, w) in joints.iter().zip(&weights) {
            let mut row = [(0u16, 0.0f32); 4];
            for k in 0..4 {
                let Some(&local) = local_of.get(&usize::from(j[k])) else {
                    return Err(error(format!(
                        "joint {} is not one of its skin bones",
                        j[k]
                    )));
                };
                row[k] = (local, w[k]);
            }
            rows.push(row);
        }
        Ok(Skin {
            bones,
            weights: rows,
        })
    }

    fn indices(&self, index: usize) -> Result<Vec<u32>, NifError> {
        let (data, _) = self.accessor_bytes(index, UNSIGNED_INT, "SCALAR", 4)?;
        Ok(data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect())
    }

    fn material(&self, index: usize) -> Result<Material, NifError> {
        let Some(json) = self.root.materials.get(index) else {
            return Err(self.json_error(format!("material {index} does not exist")));
        };
        let Some(extras) = &json.extras else {
            return Err(self.json_error(format!("material {index} has no extras.mojave")));
        };
        let alpha = match json.alpha_mode.as_deref() {
            None | Some("OPAQUE") => AlphaMode::Opaque,
            Some("MASK") => AlphaMode::Mask {
                cutoff: json.alpha_cutoff.unwrap_or(0.5),
            },
            Some("BLEND") => AlphaMode::Blend,
            Some(other) => {
                return Err(self.json_error(format!("material {index} has alphaMode \"{other}\"")));
            }
        };
        let mojave = &extras.mojave;
        Ok(Material {
            textures: mojave.textures.clone(),
            alpha,
            two_sided: json.double_sided,
            shader: mojave.shader.as_ref().map(|s| ShaderInfo {
                unlit: s.unlit,
                shader_type: s.shader_type,
                flags: s.flags,
                flags2: s.flags2,
            }),
            alpha_flags: mojave.alpha_flags,
            opacity: mojave.opacity,
            emissive: mojave.emissive,
            emissive_multiple: mojave.emissive_multiple,
        })
    }

    fn primitive(&self, number: usize, json: &PrimitiveJson) -> Result<Primitive, NifError> {
        if json.mode.is_some_and(|mode| mode != MODE_TRIANGLES) {
            return Err(self.json_error(format!("primitive {number} is not a triangle list")));
        }
        let Some(&position) = json.attributes.get("POSITION") else {
            return Err(self.json_error(format!("primitive {number} has no POSITION")));
        };
        let positions: Vec<[f32; 3]> = self
            .floats::<3>(position, "VEC3")?
            .into_iter()
            .map(from_gltf_axes)
            .collect();
        let count = positions.len();
        let check = |name: &str, len: usize| {
            if len == count {
                Ok(())
            } else {
                Err(self.json_error(format!(
                    "primitive {number}: {name} has {len} elements, POSITION has {count}"
                )))
            }
        };
        let normals = match json.attributes.get("NORMAL") {
            Some(&accessor) => {
                let normals = self.floats::<3>(accessor, "VEC3")?;
                check("NORMAL", normals.len())?;
                Some(normals.into_iter().map(from_gltf_axes).collect())
            }
            None => None,
        };
        let uvs = match json.attributes.get("TEXCOORD_0") {
            Some(&accessor) => {
                let uvs = self.floats::<2>(accessor, "VEC2")?;
                check("TEXCOORD_0", uvs.len())?;
                Some(uvs)
            }
            None => None,
        };
        let colors = match json.attributes.get("COLOR_0") {
            Some(&accessor) => {
                let colors = self.floats::<4>(accessor, "VEC4")?;
                check("COLOR_0", colors.len())?;
                Some(colors)
            }
            None => None,
        };
        let Some(indices) = json.indices else {
            return Err(self.json_error(format!("primitive {number} has no indices")));
        };
        let indices = self.indices(indices)?;
        if !indices.len().is_multiple_of(3) {
            return Err(self.json_error(format!(
                "primitive {number} has {} indices, not a multiple of three",
                indices.len()
            )));
        }
        if let Some(bad) = indices.iter().find(|&&i| i as usize >= count) {
            return Err(self.json_error(format!(
                "primitive {number}: index {bad} is past the vertex count {count}"
            )));
        }
        let Some(material) = json.material else {
            return Err(self.json_error(format!("primitive {number} has no material")));
        };
        let Some(extras) = &json.extras else {
            return Err(self.json_error(format!("primitive {number} has no extras.mojave")));
        };
        let skin = match &extras.mojave.skin_bones {
            Some(skin_bones) => Some(self.primitive_skin(number, json, skin_bones, count)?),
            None => None,
        };
        Ok(Primitive {
            name: extras.mojave.name.clone(),
            positions,
            normals,
            uvs,
            colors,
            indices,
            material: self.material(material)?,
            skinned: extras.mojave.skinned,
            skin,
        })
    }
}

/// Reads a `.glb` written by [`write()`] and returns the mesh in source
/// space. This is not a general glTF importer: it accepts the layout
/// described at the top of this module and reports anything else as
/// [`NifError::Glb`].
pub fn read(bytes: &[u8]) -> Result<StaticMesh, NifError> {
    let chunks = split_chunks(bytes)?;
    let root: Root = serde_json::from_slice(chunks.json)
        .map_err(|e| glb_error(chunks.json_offset, format!("invalid glTF JSON: {e}")))?;
    let mut document = Document {
        root,
        chunks,
        joints: Vec::new(),
    };
    if let Some(skin) = document.root.nodes.first().and_then(|n| n.skin) {
        document.joints = document.skin_joints(skin)?;
    }
    let root = &document.root;

    if root.asset.version != "2.0" {
        return Err(document.json_error(format!(
            "glTF version \"{}\" is not supported",
            root.asset.version
        )));
    }
    let marked = root
        .asset
        .extras
        .as_ref()
        .is_some_and(|e| e.mojave.units == "game" && e.mojave.source_up == "z");
    if !marked {
        return Err(document.json_error(
            "asset.extras.mojave does not say units \"game\" and source_up \"z\"; this file \
             was not written by nif::glb::write",
        ));
    }
    let Some(node) = root.nodes.first() else {
        return Err(document.json_error("the file has no nodes"));
    };

    let mut primitives = Vec::new();
    if let Some(mesh_index) = node.mesh {
        let Some(mesh) = root.meshes.get(mesh_index) else {
            return Err(document.json_error(format!("mesh {mesh_index} does not exist")));
        };
        for (number, primitive) in mesh.primitives.iter().enumerate() {
            primitives.push(document.primitive(number, primitive)?);
        }
    }

    let (collision, skipped_blocks) = match &node.extras {
        Some(extras) => (
            extras.mojave.collision.as_ref().map(|c| CollisionInfo {
                kind: c.kind.clone(),
                target: c.target.clone(),
                flags: c.flags,
                body: c.body.clone(),
                layer: c.layer,
                motion_system: c.motion_system,
                count: c.count,
            }),
            extras.mojave.skipped_blocks.clone(),
        ),
        None => (None, BTreeMap::new()),
    };
    Ok(StaticMesh {
        primitives,
        collision,
        skipped_blocks,
    })
}
