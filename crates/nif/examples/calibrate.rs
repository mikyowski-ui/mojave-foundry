//! Real-data calibration probe. Reads every `.nif` of the archive named by
//! the `MOJAVE_MESHES_BSA` environment variable and prints structural
//! tallies only: counts, flag values, block type names. It writes nothing.
//!
//! ```text
//! MOJAVE_MESHES_BSA=/path/to/meshes.bsa cargo run -p nif --example calibrate
//! ```
//!
//! What it measures:
//!
//! - **rotation convention**, from skinned shapes. In a file stored in its
//!   bind pose, `world(bone) ∘ skin_offset(bone)` is the same transform for
//!   every bone of a skin. That holds only when rotation matrices are read
//!   and composed the right way round, so it tells rows from columns
//!   without any outside reference;
//! - **vector flags**: which values occur, and whether the UV sets and
//!   tangent frames decoded under them look like UVs and tangent frames;
//! - why meshes come out **empty**, and what the **untextured** primitives
//!   have for properties.

use std::collections::BTreeMap;
use std::path::Path;

use fnv_bsa::BsaArchive;
use nif::{Block, ExtractOptions, GeometryData, NifFile, Transform};

#[derive(Default)]
struct Tally {
    counts: BTreeMap<String, u64>,
    firsts: BTreeMap<String, String>,
}

impl Tally {
    fn add(&mut self, key: impl Into<String>, file: &str) {
        let key = key.into();
        *self.counts.entry(key.clone()).or_default() += 1;
        self.firsts.entry(key).or_insert_with(|| file.to_owned());
    }

    fn print(&self, title: &str, limit: usize) {
        println!("\n{title}:");
        let mut ranked: Vec<_> = self.counts.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (key, count) in ranked.iter().take(limit) {
            println!("  {count:>8}  {key}   (first: {})", self.firsts[*key]);
        }
        if ranked.len() > limit {
            println!("  ... {} more classes", ranked.len() - limit);
        }
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let b = self.data.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let b = self.data.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn vec3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }
    /// Rotation, translation, scale: the order a skin transform is stored in.
    fn transform(&mut self) -> Option<Transform> {
        Some(Transform {
            rotation: [self.vec3()?, self.vec3()?, self.vec3()?],
            translation: self.vec3()?,
            scale: self.f32()?,
        })
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.pos = self.pos.checked_add(n)?;
        (self.pos <= self.data.len()).then_some(())
    }
}

/// The raw bytes and the type name of a block.
fn raw<'a>(data: &'a [u8], file: &'a NifFile, index: u32) -> Option<(&'a [u8], &'a str)> {
    let e = file.blocks.get(index as usize)?;
    let bytes = data.get(e.offset..e.offset + e.size as usize)?;
    Some((bytes, e.type_name.as_str()))
}

fn geom_of(block: &Block) -> Option<&GeometryData> {
    match block {
        Block::TriShapeData(d) => Some(&d.geom),
        Block::TriStripsData(d) => Some(&d.geom),
        _ => None,
    }
}

fn transposed(t: &Transform) -> Transform {
    let r = t.rotation;
    Transform {
        rotation: [
            [r[0][0], r[1][0], r[2][0]],
            [r[0][1], r[1][1], r[2][1]],
            [r[0][2], r[1][2], r[2][2]],
        ],
        ..*t
    }
}

fn local(av: &nif::AvObject) -> Transform {
    Transform {
        rotation: av.rotation,
        translation: av.translation,
        scale: av.scale,
    }
}

fn asymmetric(t: &Transform) -> bool {
    let r = &t.rotation;
    (r[0][1] - r[1][0]).abs() > 1e-3
        || (r[0][2] - r[2][0]).abs() > 1e-3
        || (r[1][2] - r[2][1]).abs() > 1e-3
}

/// Largest translation difference and largest rotation entry difference.
fn difference(a: &Transform, b: &Transform) -> (f32, f32) {
    let largest = |a: &[f32], b: &[f32]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max)
    };
    (
        largest(&a.translation, &b.translation),
        largest(a.rotation.as_flattened(), b.rotation.as_flattened()),
    )
}

/// World transform of every node and shape under both conventions:
/// `[rows acting on column vectors, the transpose]`.
fn world_transforms(file: &NifFile) -> BTreeMap<u32, [Transform; 2]> {
    let mut out = BTreeMap::new();
    let mut stack: Vec<(u32, [Transform; 2], u32)> = file
        .roots
        .iter()
        .flatten()
        .map(|&r| (r, [Transform::IDENTITY; 2], 0))
        .collect();
    while let Some((index, parent, depth)) = stack.pop() {
        if depth > 256 || out.contains_key(&index) {
            continue;
        }
        let Some(entry) = file.blocks.get(index as usize) else {
            continue;
        };
        let av = match &entry.block {
            Block::Node(node) => &node.av,
            block => match block.as_shape() {
                Some(shape) => &shape.av,
                None => continue,
            },
        };
        let l = local(av);
        let world = [parent[0].then(&l), parent[1].then(&transposed(&l))];
        out.insert(index, world);
        if let Block::Node(node) = &entry.block {
            for &child in node.children.iter().flatten() {
                stack.push((child, world, depth + 1));
            }
        }
    }
    out
}

struct Skin {
    bones: Vec<u32>,
    overall: Transform,
    offsets: Vec<Transform>,
}

/// Reads a skin instance and its skin data from raw block bytes. `None`
/// when the bytes do not fit the layout exactly.
fn read_skin(data: &[u8], file: &NifFile, instance: u32) -> Option<Skin> {
    let (bytes, name) = raw(data, file, instance)?;
    if name != "NiSkinInstance" && name != "BSDismemberSkinInstance" {
        return None;
    }
    let mut c = Cursor {
        data: bytes,
        pos: 0,
    };
    let skin_data = c.u32()?;
    let _partition = c.u32()?;
    let _skeleton_root = c.u32()?;
    let count = c.u32()? as usize;
    if count > bytes.len() / 4 {
        return None;
    }
    let mut bones = Vec::with_capacity(count);
    for _ in 0..count {
        bones.push(c.u32()?);
    }
    if name == "BSDismemberSkinInstance" {
        let partitions = c.u32()? as usize;
        c.skip(partitions.checked_mul(4)?)?;
    }
    if c.pos != bytes.len() {
        return None;
    }

    let (bytes, name) = raw(data, file, skin_data)?;
    if name != "NiSkinData" {
        return None;
    }
    let mut c = Cursor {
        data: bytes,
        pos: 0,
    };
    let overall = c.transform()?;
    let n = c.u32()? as usize;
    let has_weights = c.u8()?;
    if n != count || has_weights > 1 {
        return None;
    }
    let mut offsets = Vec::with_capacity(n);
    for _ in 0..n {
        offsets.push(c.transform()?);
        c.skip(16)?; // bounding sphere
        let vertices = usize::from(c.u16()?);
        if has_weights == 1 {
            c.skip(vertices * 6)?;
        }
    }
    (c.pos == bytes.len()).then_some(Skin {
        bones,
        overall,
        offsets,
    })
}

fn rotation_check(data: &[u8], file: &NifFile, path: &str, tally: &mut Tally, bind: &mut Tally) {
    let worlds = world_transforms(file);
    for (index, entry) in file.blocks.iter().enumerate() {
        let Some(shape) = entry.block.as_shape() else {
            continue;
        };
        let Some(instance) = shape.skin_instance else {
            continue;
        };
        let Some(skin) = read_skin(data, file, instance) else {
            tally.add("skin: layout not as expected", path);
            continue;
        };
        // skin-to-world per bone, under each convention.
        let mut per_convention: [Vec<Transform>; 2] = [Vec::new(), Vec::new()];
        let mut informative = false;
        let mut complete = true;
        for (bone, offset) in skin.bones.iter().zip(&skin.offsets) {
            let Some(world) = worlds.get(bone) else {
                complete = false;
                break;
            };
            informative |= asymmetric(offset) || asymmetric(&world[0]);
            per_convention[0].push(world[0].then(offset));
            per_convention[1].push(world[1].then(&transposed(offset)));
        }
        if !complete || skin.bones.len() < 2 {
            tally.add(
                "skin: a bone is outside the scene graph, or fewer than two bones",
                path,
            );
            continue;
        }
        if !informative {
            tally.add(
                "skin: no asymmetric rotation, conventions cannot differ",
                path,
            );
            continue;
        }
        let consistent = |list: &[Transform]| {
            list.iter().all(|m| {
                let (t, r) = difference(m, &list[0]);
                t < 0.05 && r < 2e-3
            })
        };
        let key = format!(
            "skin: bind transform constant across bones under rows: {} | under columns: {}",
            consistent(&per_convention[0]),
            consistent(&per_convention[1])
        );
        tally.add(key, path);

        // How the shape's own node chain relates to the bind transform.
        if consistent(&per_convention[0])
            && let Some(shape_world) = worlds.get(&(index as u32))
        {
            let (t, r) = difference(&per_convention[0][0], &shape_world[0]);
            let (ot, or) = difference(&skin.overall, &Transform::IDENTITY);
            bind.add(
                format!(
                    "skinned shape: node chain equals bind transform: {} | overall skin transform is identity: {}",
                    t < 0.05 && r < 2e-3,
                    ot < 0.05 && or < 2e-3
                ),
                path,
            );
        }
    }
}

#[derive(Default)]
struct VectorStats {
    uv_sets: u64,
    uv_sets_not_finite: u64,
    uv_values: u64,
    uv_values_within_64: u64,
    uv_values_within_unit: u64,
    frames: u64,
    frames_zero: u64,
    frames_unit: u64,
    frames_orthogonal: u64,
    frames_right_angle: u64,
}

fn vector_check(file: &NifFile, path: &str, flags: &mut Tally, stats: &mut VectorStats) {
    let len = |v: &[f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let dot = |a: &[f32; 3], b: &[f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    for g in file.blocks.iter().filter_map(|e| geom_of(&e.block)) {
        flags.add(
            format!(
                "vector_flags {:#06x} | normals {} | tangents {} | uv sets {}",
                g.vector_flags,
                g.normals.is_some(),
                g.tangents.is_some(),
                g.uv_sets.len()
            ),
            path,
        );
        for set in &g.uv_sets {
            stats.uv_sets += 1;
            if !set.iter().flatten().all(|v| v.is_finite()) {
                stats.uv_sets_not_finite += 1;
            }
            for v in set.iter().flatten() {
                stats.uv_values += 1;
                stats.uv_values_within_64 += u64::from(v.abs() <= 64.0);
                stats.uv_values_within_unit += u64::from((-0.01..=1.01).contains(v));
            }
        }
        if let (Some(n), Some(t), Some(b)) = (&g.normals, &g.tangents, &g.bitangents) {
            for ((n, t), b) in n.iter().zip(t).zip(b) {
                stats.frames += 1;
                if len(t) < 1e-6 && len(b) < 1e-6 {
                    stats.frames_zero += 1;
                    continue;
                }
                let unit = (len(t) - 1.0).abs() < 0.05 && (len(b) - 1.0).abs() < 0.05;
                stats.frames_unit += u64::from(unit);
                stats.frames_orthogonal +=
                    u64::from(dot(n, t).abs() < 0.1 && dot(n, b).abs() < 0.1);
                stats.frames_right_angle += u64::from(unit && dot(t, b).abs() < 0.3);
            }
        }
    }
}

fn classify_empty(file: &NifFile, path: &str, tally: &mut Tally) {
    let shapes: Vec<_> = file
        .blocks
        .iter()
        .filter_map(|e| e.block.as_shape())
        .collect();
    if shapes.is_empty() {
        let has = |needle: &str| file.blocks.iter().any(|e| e.type_name.contains(needle));
        let key = if has("Particle") || has("PSys") {
            "no shape blocks: particle systems"
        } else if has("bhk") {
            "no shape blocks: nodes and collision only"
        } else if file.blocks.iter().any(|e| e.block.is_skipped()) {
            "no shape blocks: nodes and other skipped types"
        } else {
            "no shape blocks: nodes only"
        };
        tally.add(key, path);
        if key.ends_with("other skipped types") {
            for name in file.skipped_blocks().keys() {
                tally.add(format!("    ...of which have a {name}"), path);
            }
        }
        return;
    }
    let with = |include_hidden, include_editor_markers| {
        nif::extract_mesh_with(
            file,
            ExtractOptions {
                include_hidden,
                include_editor_markers,
            },
        )
        .map(|m| m.primitives.len())
        .unwrap_or(0)
    };
    if with(true, false) > 0 {
        tally.add("shapes are all hidden (or under a hidden node)", path);
    } else if with(false, true) > 0 {
        tally.add("shapes are all editor markers", path);
    } else if with(true, true) > 0 {
        tally.add("shapes are all hidden editor markers", path);
    } else {
        let mut reasons: Vec<String> = shapes
            .iter()
            .map(|shape| match file.block(shape.data) {
                None => "no data block".to_owned(),
                Some(e) => match geom_of(&e.block) {
                    None => format!("data block is {}", e.type_name),
                    Some(g) if g.vertices.as_ref().is_none_or(|v| v.is_empty()) => format!(
                        "{} without vertices (additional data: {})",
                        e.type_name,
                        g.additional_data.is_some()
                    ),
                    Some(_) => format!("{} without usable triangles", e.type_name),
                },
            })
            .collect();
        reasons.sort();
        reasons.dedup();
        tally.add(format!("shapes unusable: {}", reasons.join(" + ")), path);
    }
}

fn main() {
    let Some(path) = std::env::var_os("MOJAVE_MESHES_BSA") else {
        eprintln!("set MOJAVE_MESHES_BSA to the meshes archive");
        std::process::exit(2);
    };
    let archive = BsaArchive::open(Path::new(&path)).expect("open archive");
    let mut errors = Tally::default();
    let mut rotation = Tally::default();
    let mut bind = Tally::default();
    let mut flags = Tally::default();
    let mut vectors = VectorStats::default();
    let mut empty = Tally::default();
    let mut untextured = Tally::default();
    let mut texture_source = Tally::default();
    let mut misc = Tally::default();
    let mut shape_data = Tally::default();
    let (mut segmented_primitives, mut segmented_untextured) = (0u64, 0u64);
    let (mut files, mut parsed, mut extracted, mut empty_count) = (0u64, 0u64, 0u64, 0u64);
    let (mut primitives, mut skinned, mut no_texture, mut all_skinned) = (0u64, 0u64, 0u64, 0u64);
    for entry in archive
        .entries()
        .iter()
        .filter(|e| e.path.ends_with(".nif"))
    {
        files += 1;
        let path = entry.path.as_str();
        let Ok(data) = archive.read(entry) else {
            errors.add("archive read error", path);
            continue;
        };
        let file = match nif::parse(&data) {
            Ok(file) => file,
            Err(error) => {
                let text: String = error
                    .to_string()
                    .chars()
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .collect();
                errors.add(format!("parse: {text}"), path);
                continue;
            }
        };
        parsed += 1;
        rotation_check(&data, &file, path, &mut rotation, &mut bind);
        for e in &file.blocks {
            match &e.block {
                Block::Node(node)
                    if !node.extra.is_empty()
                        || nif::NODE_PREFIX_TYPES.contains(&e.type_name.as_str()) =>
                {
                    let detail = if e.type_name == "NiSwitchNode" || e.type_name == "NiLODNode" {
                        let index = node
                            .extra
                            .get(2..6)
                            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                        format!(
                            " | index in range: {:?}",
                            index.map(|i| (i as usize) < node.children.len().max(1))
                        )
                    } else {
                        String::new()
                    };
                    misc.add(
                        format!("{} tail {} bytes{detail}", e.type_name, node.extra.len()),
                        path,
                    );
                }
                Block::RigidBody(peek) => {
                    misc.add(
                        format!(
                            "{} size {} | layer {:?} | motion system {:?}",
                            e.type_name, e.size, peek.layer, peek.motion_system
                        ),
                        path,
                    );
                }
                Block::StencilProperty(stencil) => {
                    misc.add(
                        format!(
                            "stencil flags {:#06x} draw mode {}",
                            stencil.flags,
                            stencil.draw_mode()
                        ),
                        path,
                    );
                }
                Block::CollisionObject(object) => {
                    misc.add(
                        format!("{} tail {} bytes", e.type_name, object.extra.len()),
                        path,
                    );
                }
                Block::SegmentedTriShape(segmented) => {
                    misc.add(
                        format!(
                            "BSSegmentedTriShape with {} segments",
                            segmented.segments.len()
                        ),
                        path,
                    );
                }
                _ => {}
            }
        }
        vector_check(&file, path, &mut flags, &mut vectors);
        let mesh = match nif::extract_mesh(&file) {
            Ok(mesh) => mesh,
            Err(error) => {
                let text: String = error
                    .to_string()
                    .chars()
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .collect();
                errors.add(format!("extract: {text}"), path);
                continue;
            }
        };
        extracted += 1;
        if mesh.primitives.is_empty() {
            empty_count += 1;
            classify_empty(&file, path, &mut empty);
        } else if mesh.primitives.iter().all(|p| p.skinned) {
            all_skinned += 1;
        }
        for p in &mesh.primitives {
            primitives += 1;
            skinned += u64::from(p.skinned);
            let source = match (&p.material.shader, p.material.textures.is_empty()) {
                (_, true) => "none",
                (Some(s), false) if s.unlit => "unlit shader file name",
                (Some(_), false) => "shader texture set",
                (None, false) => "texturing property base texture",
            };
            texture_source.add(source, path);
            if p.material.textures.is_empty() {
                no_texture += 1;
                let key = match &p.material.shader {
                    None => "untextured primitive: no parsed shader property".to_owned(),
                    Some(s) => format!(
                        "untextured primitive: shader unlit {} type {} without texture names",
                        s.unlit, s.shader_type
                    ),
                };
                untextured.add(key, path);
            }
        }
        if file
            .blocks
            .iter()
            .any(|e| matches!(e.block, Block::SegmentedTriShape(_)))
        {
            // Segmented shapes are the only shapes in the files that have them.
            let only_segmented = file
                .blocks
                .iter()
                .filter(|e| e.block.as_shape().is_some())
                .all(|e| matches!(e.block, Block::SegmentedTriShape(_)));
            if only_segmented {
                segmented_primitives += mesh.primitives.len() as u64;
                segmented_untextured += mesh
                    .primitives
                    .iter()
                    .filter(|p| p.material.textures.is_empty())
                    .count() as u64;
            } else {
                misc.add("file mixes segmented and plain shapes", path);
            }
        }
        // What the shapes without any texture have for properties.
        for shape in file.blocks.iter().filter_map(|e| e.block.as_shape()) {
            let mut kinds: Vec<&str> = shape
                .av
                .properties
                .iter()
                .filter_map(|&p| file.block(p))
                .map(|e| e.type_name.as_str())
                .collect();
            kinds.sort();
            let textured = shape.av.properties.iter().any(|&p| {
                matches!(
                    file.block(p).map(|e| &e.block),
                    Some(Block::ShaderPpLighting(_) | Block::ShaderNoLighting(_))
                ) || matches!(
                    file.block(p).map(|e| &e.block),
                    Some(Block::TexturingProperty(t)) if t.base_source.is_some()
                )
            });
            if !textured {
                untextured.add(format!("shape properties: [{}]", kinds.join(", ")), path);
            }
            let state = match file.block(shape.data) {
                None => "no data block".to_owned(),
                Some(e) => match (&e.block, geom_of(&e.block)) {
                    (_, None) => format!("data block is {}", e.type_name),
                    (block, Some(g)) => {
                        let triangles = match block {
                            Block::TriShapeData(d) => {
                                d.triangles.as_ref().is_some_and(|t| !t.is_empty())
                            }
                            Block::TriStripsData(d) => {
                                d.strips.as_ref().is_some_and(|s| !s.is_empty())
                            }
                            _ => false,
                        };
                        format!(
                            "vertices stored {} | triangles stored {} | additional geometry data {}",
                            g.vertices.as_ref().is_some_and(|v| !v.is_empty()),
                            triangles,
                            g.additional_data.is_some()
                        )
                    }
                },
            };
            shape_data.add(state, path);
            let segmented = file
                .blocks
                .iter()
                .any(|e| matches!(&e.block, Block::SegmentedTriShape(s) if std::ptr::eq(&s.shape, shape)));
            if segmented {
                misc.add(
                    format!("segmented shape with a textured property of its own: {textured}"),
                    path,
                );
            }
            for &p in &shape.av.properties {
                if let Some(Block::ShaderNoLighting(shader)) = file.block(p).map(|e| &e.block) {
                    let colors = file
                        .block(shape.data)
                        .and_then(|e| geom_of(&e.block))
                        .is_some_and(|g| g.colors.is_some());
                    untextured.add(
                        format!(
                            "unlit shader: file name empty {} | tail {} bytes | flags {:#010x} {:#010x} | vertex colours {} | shape properties [{}]",
                            shader.file_name.is_empty(),
                            shader.extra.len(),
                            shader.shader.shader_flags,
                            shader.shader.shader_flags2,
                            colors,
                            kinds.join(", ")
                        ),
                        path,
                    );
                }
                if let Some(Block::ShaderPpLighting(shader)) = file.block(p).map(|e| &e.block) {
                    let state = match file.block(shader.texture_set).map(|e| &e.block) {
                        None => "no texture set".to_owned(),
                        Some(Block::ShaderTextureSet(set)) => {
                            if set.textures.iter().all(String::is_empty) {
                                format!("texture set of {} empty names", set.textures.len())
                            } else {
                                continue;
                            }
                        }
                        Some(_) => "texture set reference is another block type".to_owned(),
                    };
                    untextured.add(
                        format!("lit shader type {}: {state}", shader.shader.shader_type),
                        path,
                    );
                }
            }
        }
    }
    println!(
        "files: {files}  parsed: {parsed}  extracted: {extracted}  empty meshes: {empty_count}"
    );
    println!(
        "primitives: {primitives}  skinned: {skinned}  without texture: {no_texture}  \
         files whose primitives are all skinned: {all_skinned}"
    );
    println!(
        "primitives from files whose shapes are all segmented: {segmented_primitives}  \
         of which without texture: {segmented_untextured}"
    );
    errors.print("errors", 30);
    misc.print("node tails, rigid bodies, stencils, collision objects", 120);
    rotation.print("rotation convention, from skins", 10);
    bind.print("skinned shapes and their bind transform", 10);
    flags.print("vector flags", 40);
    println!(
        "\nuv sets: {}  with a non-finite value: {}",
        vectors.uv_sets, vectors.uv_sets_not_finite
    );
    println!(
        "uv values: {}  within |64|: {}  within [0, 1]: {}",
        vectors.uv_values, vectors.uv_values_within_64, vectors.uv_values_within_unit
    );
    println!(
        "tangent frames: {}  zero: {}  unit length: {}  orthogonal to normal: {}  \
         tangent at a right angle to bitangent: {}",
        vectors.frames,
        vectors.frames_zero,
        vectors.frames_unit,
        vectors.frames_orthogonal,
        vectors.frames_right_angle
    );
    shape_data.print("shapes by what their data block stores", 12);
    texture_source.print("texture source per primitive", 10);
    empty.print("empty meshes", 70);
    untextured.print("shapes with no textured property of their own", 45);
}
