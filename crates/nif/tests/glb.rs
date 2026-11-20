//! The `.glb` writer and reader of `docs/contracts.md` section 3.

use std::collections::BTreeMap;

use nif::fixture::building;
use nif::{
    AlphaMode, CollisionInfo, Material, NifError, Primitive, ShaderInfo, StaticMesh, extract_mesh,
    glb, nif_to_glb, parse,
};
use serde_json::Value;

const FLOAT: u64 = 5126;
const UNSIGNED_INT: u64 = 5125;
const ARRAY_BUFFER: u64 = 34962;
const ELEMENT_ARRAY_BUFFER: u64 = 34963;

fn primitive(name: &str, normals: bool, uvs: bool, colors: bool) -> Primitive {
    Primitive {
        name: name.to_owned(),
        positions: vec![
            [1.0, 2.0, 3.0],
            [4.5, -2.25, 0.125],
            [-7.0, 8.0, 9.5],
            [0.1, 0.2, 0.3],
        ],
        normals: normals.then(|| {
            vec![
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.6, 0.0, 0.8],
            ]
        }),
        uvs: uvs.then(|| vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.25, 0.75]]),
        colors: colors.then(|| {
            vec![
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0, 0.5],
                [0.0, 0.0, 1.0, 0.25],
                [0.2, 0.4, 0.6, 0.8],
            ]
        }),
        indices: vec![0, 1, 2, 0, 2, 3],
        material: Material::default(),
        skinned: false,
        skin: None,
    }
}

fn collision() -> CollisionInfo {
    CollisionInfo {
        kind: "bhkCollisionObject".to_owned(),
        target: "Scene Root".to_owned(),
        flags: 0x81,
        body: "bhkRigidBody".to_owned(),
        layer: Some(1),
        motion_system: None,
        count: 3,
    }
}

/// A mesh that uses every field: all eight attribute combinations, every
/// alpha mode, awkward strings and awkward floats.
fn rich_mesh() -> StaticMesh {
    let mut primitives = Vec::new();
    for bits in 0..8 {
        let mut p = primitive(
            &format!("Shape:{bits}"),
            bits & 1 != 0,
            bits & 2 != 0,
            bits & 4 != 0,
        );
        p.material = match bits % 4 {
            0 => Material::default(),
            1 => Material {
                textures: vec![
                    "textures\\synthetic\\wall.dds".to_owned(),
                    String::new(),
                    "textures\\synthetic\\wall_g.dds".to_owned(),
                ],
                alpha: AlphaMode::Mask { cutoff: 0.2 },
                two_sided: true,
                shader: Some(ShaderInfo {
                    unlit: false,
                    shader_type: 1,
                    flags: 0x8240_0301,
                    flags2: u32::MAX,
                }),
                alpha_flags: Some(0x12ED),
                opacity: 1.0 / 3.0,
                emissive: [0.1, 1e-30, 3.5],
                emissive_multiple: 12.5,
            },
            2 => Material {
                textures: vec!["textures\\synthetic\\sign.dds".to_owned()],
                alpha: AlphaMode::Blend,
                shader: Some(ShaderInfo {
                    unlit: true,
                    shader_type: 0,
                    flags: 0,
                    flags2: 0,
                }),
                alpha_flags: Some(0),
                opacity: f32::MIN_POSITIVE,
                emissive: [f32::MAX, -0.0, 1.0e-45],
                emissive_multiple: -2.0,
                ..Material::default()
            },
            _ => Material {
                alpha: AlphaMode::Mask { cutoff: 1.0 },
                ..Material::default()
            },
        };
        primitives.push(p);
    }
    primitives[6].skinned = true;
    primitives[3].name = "quote \" backslash \\ newline \n tab \t bell \u{7} é \u{6f22}".to_owned();
    primitives[5].positions[0] = [f32::MAX, f32::MIN, f32::MIN_POSITIVE];
    primitives[5].positions[1] = [-0.0, 1.0e-45, 16_777_217.0];
    StaticMesh {
        primitives,
        collision: Some(collision()),
        skipped_blocks: [("bhkRigidBody", 1), ("NiPointLight", 4), ("a \"b\"", 2)]
            .into_iter()
            .map(|(name, count)| (name.to_owned(), count))
            .collect(),
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// Splits a `.glb` into its JSON text and binary chunk, checking the
/// container on the way.
fn split(bytes: &[u8]) -> (String, Vec<u8>) {
    assert_eq!(&bytes[0..4], b"glTF");
    assert_eq!(u32_at(bytes, 4), 2, "container version");
    assert_eq!(u32_at(bytes, 8) as usize, bytes.len(), "total length");
    assert_eq!(bytes.len() % 4, 0);

    let json_len = u32_at(bytes, 12) as usize;
    assert_eq!(u32_at(bytes, 16), 0x4E4F_534A, "first chunk is JSON");
    assert_eq!(json_len % 4, 0, "JSON chunk is padded to four bytes");
    let json = &bytes[20..20 + json_len];
    let text = std::str::from_utf8(json).unwrap();
    assert!(
        text.len() - text.trim_end_matches(' ').len() < 4,
        "at most three bytes of padding"
    );
    assert!(
        text.trim_end_matches(' ').ends_with('}'),
        "padding is spaces"
    );

    let mut offset = 20 + json_len;
    let mut bin = Vec::new();
    if offset < bytes.len() {
        let bin_len = u32_at(bytes, offset) as usize;
        assert_eq!(
            u32_at(bytes, offset + 4),
            0x004E_4942,
            "second chunk is BIN"
        );
        assert_eq!(bin_len % 4, 0, "BIN chunk is padded to four bytes");
        bin = bytes[offset + 8..offset + 8 + bin_len].to_vec();
        offset += 8 + bin_len;
    }
    assert_eq!(offset, bytes.len(), "nothing after the last chunk");
    (text.to_owned(), bin)
}

fn assemble(json: &str, bin: Option<&[u8]>) -> Vec<u8> {
    let mut json = json.as_bytes().to_vec();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    if let Some(bin) = bin {
        let mut bin = bin.to_vec();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
    }
    let total = out.len() as u32;
    out[8..12].copy_from_slice(&total.to_le_bytes());
    out
}

fn read_error(bytes: &[u8]) -> String {
    match glb::read(bytes) {
        Err(NifError::Glb { reason, .. }) => reason,
        other => panic!("expected a glb error, got {other:?}"),
    }
}

#[test]
fn round_trip_with_and_without_each_optional_attribute() {
    for bits in 0..8 {
        let mesh = StaticMesh {
            primitives: vec![primitive(
                "Only",
                bits & 1 != 0,
                bits & 2 != 0,
                bits & 4 != 0,
            )],
            ..StaticMesh::default()
        };
        assert_eq!(glb::read(&glb::write(&mesh)).unwrap(), mesh, "bits {bits}");
    }
}

#[test]
fn round_trip_of_every_field() {
    let mesh = rich_mesh();
    let back = glb::read(&glb::write(&mesh)).unwrap();
    assert_eq!(back, mesh);
    // `==` on floats does not tell the two zeros apart; the bits must match
    // too.
    let bits = |m: &StaticMesh| -> Vec<u32> {
        m.primitives
            .iter()
            .flat_map(|p| p.positions.iter().flatten().map(|v| v.to_bits()))
            .collect()
    };
    assert_eq!(bits(&back), bits(&mesh));
}

#[test]
fn round_trip_of_an_empty_mesh() {
    let empty = StaticMesh::default();
    let bytes = glb::write(&empty);
    let (json, bin) = split(&bytes);
    assert!(bin.is_empty());
    let json: Value = serde_json::from_str(&json).unwrap();
    assert!(json.get("meshes").is_none() && json.get("buffers").is_none());
    assert_eq!(glb::read(&bytes).unwrap(), empty);

    let metadata_only = StaticMesh {
        primitives: Vec::new(),
        collision: Some(collision()),
        skipped_blocks: BTreeMap::from([("NiCamera".to_owned(), 2)]),
    };
    assert_eq!(
        glb::read(&glb::write(&metadata_only)).unwrap(),
        metadata_only
    );
}

#[test]
fn round_trip_of_an_extracted_building() {
    let bytes = building([800.0, 600.0, 300.0], "textures\\w.dds", "textures\\r.dds");
    let mesh = extract_mesh(&parse(&bytes).unwrap()).unwrap();
    assert_eq!(mesh.primitives.len(), 2);
    let glb_bytes = nif_to_glb(&bytes).unwrap();
    assert_eq!(glb_bytes, glb::write(&mesh));
    assert_eq!(glb::read(&glb_bytes).unwrap(), mesh);
}

#[test]
fn output_is_deterministic() {
    let mesh = rich_mesh();
    assert_eq!(glb::write(&mesh), glb::write(&mesh.clone()));
    let bytes = building([800.0, 600.0, 300.0], "textures\\w.dds", "textures\\r.dds");
    assert_eq!(nif_to_glb(&bytes).unwrap(), nif_to_glb(&bytes).unwrap());
}

#[test]
fn the_json_layout_is_pinned() {
    let mesh = StaticMesh {
        primitives: vec![Primitive {
            name: "Tri".to_owned(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Primitive::default()
        }],
        ..StaticMesh::default()
    };
    let (json, bin) = split(&glb::write(&mesh));
    let expected = concat!(
        r#"{"asset":{"version":"2.0","generator":"mojave-foundry nif","#,
        r#""extras":{"mojave":{"units":"game","source_up":"z"}}},"#,
        r#""scene":0,"scenes":[{"nodes":[0]}],"#,
        r#""nodes":[{"name":"mesh","mesh":0,"extras":{"mojave":{"skipped_blocks":{}}}}],"#,
        r#""meshes":[{"name":"mesh","primitives":[{"attributes":{"POSITION":0},"indices":1,"#,
        r#""material":0,"mode":4,"extras":{"mojave":{"name":"Tri","skinned":false}}}]}],"#,
        r#""materials":[{"pbrMetallicRoughness":{"metallicFactor":0,"roughnessFactor":1},"#,
        r#""alphaMode":"OPAQUE","doubleSided":false,"extras":{"mojave":{"textures":[],"#,
        r#""shader":null,"alpha_flags":null,"opacity":1,"emissive":[0,0,0],"#,
        r#""emissive_multiple":1}}}],"#,
        r#""accessors":[{"bufferView":0,"byteOffset":0,"componentType":5126,"count":3,"#,
        r#""type":"VEC3","min":[0,0,-2],"max":[1,0,0]},"#,
        r#"{"bufferView":1,"byteOffset":0,"componentType":5125,"count":3,"type":"SCALAR"}],"#,
        r#""bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36,"target":34962},"#,
        r#"{"buffer":0,"byteOffset":36,"byteLength":12,"target":34963}],"#,
        r#""buffers":[{"byteLength":48}]}"#,
    );
    assert_eq!(json.trim_end_matches(' '), expected);

    // Source (x, y, z) is stored as (x, z, -y): the third vertex, two units
    // north, is at glTF z = -2.
    let floats: Vec<f32> = bin[..36]
        .chunks(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    assert_eq!(floats, [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -2.0]);
    assert_eq!(glb::to_gltf_axes([1.0, 2.0, 3.0]), [1.0, 3.0, -2.0]);
    assert_eq!(glb::from_gltf_axes([1.0, 3.0, -2.0]), [1.0, 2.0, 3.0]);
}

fn index(value: &Value) -> usize {
    value.as_u64().expect("an index") as usize
}

fn component_count(kind: &str) -> usize {
    match kind {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        other => panic!("unexpected accessor type {other}"),
    }
}

/// Checks every accessor, buffer view, alignment and bound of a written
/// file by hand, from the JSON chunk and the binary chunk alone.
fn validate_structure(bytes: &[u8], mesh: &StaticMesh) {
    let (text, bin) = split(bytes);
    let json: Value = serde_json::from_str(&text).unwrap();

    // Asset and the contract's unit marker.
    assert_eq!(json["asset"]["version"], "2.0");
    assert_eq!(json["asset"]["extras"]["mojave"]["units"], "game");
    assert_eq!(json["asset"]["extras"]["mojave"]["source_up"], "z");

    // One scene, one root node, one mesh.
    assert_eq!(json["scene"], 0);
    let scenes = json["scenes"].as_array().unwrap();
    assert_eq!(scenes.len(), 1);
    assert_eq!(scenes[0]["nodes"], serde_json::json!([0]));
    let nodes = json["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["mesh"], 0);
    assert!(nodes[0].get("matrix").is_none() && nodes[0].get("translation").is_none());
    let meshes = json["meshes"].as_array().unwrap();
    assert_eq!(meshes.len(), 1);

    // One buffer: the binary chunk, at most three bytes of padding longer.
    let buffers = json["buffers"].as_array().unwrap();
    assert_eq!(buffers.len(), 1);
    assert!(buffers[0].get("uri").is_none());
    let buffer_len = index(&buffers[0]["byteLength"]);
    assert!(buffer_len >= 1 && buffer_len <= bin.len() && bin.len() - buffer_len < 4);
    assert!(
        bin[buffer_len..].iter().all(|&b| b == 0),
        "BIN padding is zeros"
    );

    // Buffer views: aligned, inside the buffer, in order, not overlapping.
    let views = json["bufferViews"].as_array().unwrap();
    let mut end_of_previous = 0;
    for (i, view) in views.iter().enumerate() {
        assert_eq!(view["buffer"], 0, "view {i}");
        let offset = index(&view["byteOffset"]);
        let length = index(&view["byteLength"]);
        assert_eq!(offset % 4, 0, "view {i} alignment");
        assert!(length >= 1, "view {i} is empty");
        assert!(
            offset >= end_of_previous,
            "view {i} overlaps the previous one"
        );
        assert!(offset + length <= buffer_len, "view {i} leaves the buffer");
        assert!(view.get("byteStride").is_none());
        let target = view["target"].as_u64().unwrap();
        assert!(target == ARRAY_BUFFER || target == ELEMENT_ARRAY_BUFFER);
        end_of_previous = offset + length;
    }

    // Accessors: each owns exactly one view and fits in it.
    let accessors = json["accessors"].as_array().unwrap();
    assert_eq!(accessors.len(), views.len());
    let mut uses = vec![0usize; accessors.len()];
    let accessor_bytes = |i: usize, component: u64, kind: &str, target: u64| -> (Vec<u8>, usize) {
        let accessor = &accessors[i];
        assert_eq!(accessor["componentType"], component, "accessor {i}");
        assert_eq!(accessor["type"], kind, "accessor {i}");
        assert!(accessor.get("sparse").is_none() && accessor.get("normalized").is_none());
        let count = index(&accessor["count"]);
        assert!(count >= 1, "accessor {i} is empty");
        let view = &views[index(&accessor["bufferView"])];
        assert_eq!(view["target"], target, "accessor {i} view target");
        let offset = index(&accessor["byteOffset"]);
        assert_eq!(offset % 4, 0);
        let size = count * component_count(kind) * 4;
        assert_eq!(
            offset + size,
            index(&view["byteLength"]),
            "accessor {i} fills its view exactly"
        );
        let start = index(&view["byteOffset"]) + offset;
        (bin[start..start + size].to_vec(), count)
    };
    let floats = |bytes: &[u8]| -> Vec<f32> {
        bytes
            .chunks(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect()
    };

    let primitives = meshes[0]["primitives"].as_array().unwrap();
    let materials = json["materials"].as_array().unwrap();
    assert_eq!(primitives.len(), mesh.primitives.len());
    assert_eq!(materials.len(), mesh.primitives.len());
    for (number, (primitive, source)) in primitives.iter().zip(&mesh.primitives).enumerate() {
        assert_eq!(
            primitive["mode"], 4,
            "primitive {number} is a triangle list"
        );
        assert_eq!(primitive["extras"]["mojave"]["name"], source.name.as_str());
        let attributes = primitive["attributes"].as_object().unwrap();
        let mut expected_attributes = vec!["POSITION"];
        if source.normals.is_some() {
            expected_attributes.push("NORMAL");
        }
        if source.uvs.is_some() {
            expected_attributes.push("TEXCOORD_0");
        }
        if source.colors.is_some() {
            expected_attributes.push("COLOR_0");
        }
        let mut present: Vec<&str> = attributes.keys().map(String::as_str).collect();
        present.sort();
        expected_attributes.sort();
        assert_eq!(present, expected_attributes, "primitive {number}");

        // POSITION: axis conversion, and min/max that match the data.
        let position = index(&attributes["POSITION"]);
        uses[position] += 1;
        let (data, vertex_count) = accessor_bytes(position, FLOAT, "VEC3", ARRAY_BUFFER);
        assert_eq!(vertex_count, source.positions.len());
        let stored = floats(&data);
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for (vertex, source) in stored.chunks(3).zip(&source.positions) {
            assert_eq!(vertex, [source[0], source[2], -source[1]]);
            for (axis, &v) in vertex.iter().enumerate() {
                min[axis] = min[axis].min(v);
                max[axis] = max[axis].max(v);
            }
        }
        for axis in 0..3 {
            let declared_min = accessors[position]["min"][axis].as_f64().unwrap() as f32;
            let declared_max = accessors[position]["max"][axis].as_f64().unwrap() as f32;
            assert_eq!(declared_min, min[axis], "primitive {number} min[{axis}]");
            assert_eq!(declared_max, max[axis], "primitive {number} max[{axis}]");
        }
        assert_eq!(accessors[position]["min"].as_array().unwrap().len(), 3);
        assert_eq!(accessors[position]["max"].as_array().unwrap().len(), 3);

        if let Some(normals) = &source.normals {
            let accessor = index(&attributes["NORMAL"]);
            uses[accessor] += 1;
            let (data, count) = accessor_bytes(accessor, FLOAT, "VEC3", ARRAY_BUFFER);
            assert_eq!(count, vertex_count);
            for (stored, source) in floats(&data).chunks(3).zip(normals) {
                assert_eq!(stored, [source[0], source[2], -source[1]]);
            }
        }
        if let Some(uvs) = &source.uvs {
            let accessor = index(&attributes["TEXCOORD_0"]);
            uses[accessor] += 1;
            let (data, count) = accessor_bytes(accessor, FLOAT, "VEC2", ARRAY_BUFFER);
            assert_eq!(count, vertex_count);
            assert_eq!(floats(&data), uvs.as_flattened());
        }
        if let Some(colors) = &source.colors {
            let accessor = index(&attributes["COLOR_0"]);
            uses[accessor] += 1;
            let (data, count) = accessor_bytes(accessor, FLOAT, "VEC4", ARRAY_BUFFER);
            assert_eq!(count, vertex_count);
            assert_eq!(floats(&data), colors.as_flattened());
        }

        // Indices: u32, a whole number of triangles, all in range.
        let indices = index(&primitive["indices"]);
        uses[indices] += 1;
        let (data, count) = accessor_bytes(indices, UNSIGNED_INT, "SCALAR", ELEMENT_ARRAY_BUFFER);
        assert_eq!(count % 3, 0);
        let stored: Vec<u32> = data
            .chunks(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(stored, source.indices);
        assert!(stored.iter().all(|&i| (i as usize) < vertex_count));

        // Material: no embedded texture, the paths in extras instead.
        let material = &materials[index(&primitive["material"])];
        assert!(
            material["pbrMetallicRoughness"]
                .get("baseColorTexture")
                .is_none()
        );
        let textures: Vec<&str> = material["extras"]["mojave"]["textures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(textures, source.material.textures);
        assert_eq!(material["doubleSided"], source.material.two_sided);
        match source.material.alpha {
            AlphaMode::Opaque => assert_eq!(material["alphaMode"], "OPAQUE"),
            AlphaMode::Blend => assert_eq!(material["alphaMode"], "BLEND"),
            AlphaMode::Mask { cutoff } => {
                assert_eq!(material["alphaMode"], "MASK");
                assert_eq!(material["alphaCutoff"].as_f64().unwrap() as f32, cutoff);
            }
        }
        if !matches!(source.material.alpha, AlphaMode::Mask { .. }) {
            assert!(material.get("alphaCutoff").is_none());
        }
    }
    assert!(uses.iter().all(|&n| n == 1), "every accessor is used once");
    assert!(json.get("images").is_none() && json.get("textures").is_none());

    // Collision is metadata on the root node.
    let node_extras = &nodes[0]["extras"]["mojave"];
    match &mesh.collision {
        Some(collision) => {
            assert_eq!(node_extras["collision"]["kind"], collision.kind.as_str());
            assert_eq!(node_extras["collision"]["flags"], collision.flags);
        }
        None => assert!(node_extras.get("collision").is_none()),
    }
}

#[test]
fn written_files_are_structurally_valid_gltf() {
    let rich = rich_mesh();
    validate_structure(&glb::write(&rich), &rich);

    let bytes = building([800.0, 600.0, 300.0], "textures\\w.dds", "textures\\r.dds");
    let extracted = extract_mesh(&parse(&bytes).unwrap()).unwrap();
    validate_structure(&glb::write(&extracted), &extracted);

    // Several primitives of one triangle each.
    let mut single = primitive("Single", true, true, true);
    single.positions.truncate(3);
    single.normals.as_mut().unwrap().truncate(3);
    single.uvs.as_mut().unwrap().truncate(3);
    single.colors.as_mut().unwrap().truncate(3);
    single.indices = vec![0, 1, 2];
    let mesh = StaticMesh {
        primitives: vec![single; 3],
        ..StaticMesh::default()
    };
    validate_structure(&glb::write(&mesh), &mesh);
}

#[test]
fn ill_formed_primitives_are_left_out_by_the_writer() {
    let good = primitive("Good", true, false, false);
    let mut no_vertices = good.clone();
    no_vertices.positions.clear();
    let mut no_triangles = good.clone();
    no_triangles.indices.clear();
    let mut index_out_of_range = good.clone();
    index_out_of_range.indices[4] = 4;
    let mut ragged = good.clone();
    ragged.indices.pop();
    let mut short_normals = good.clone();
    short_normals.normals.as_mut().unwrap().pop();
    let bad = [
        no_vertices,
        no_triangles,
        index_out_of_range,
        ragged,
        short_normals,
    ];
    assert!(bad.iter().all(|p| !p.is_well_formed()) && good.is_well_formed());

    let mut primitives = bad.to_vec();
    primitives.insert(2, good.clone());
    let mesh = StaticMesh {
        primitives,
        ..StaticMesh::default()
    };
    let bytes = glb::write(&mesh);
    let kept = StaticMesh {
        primitives: vec![good],
        ..StaticMesh::default()
    };
    validate_structure(&bytes, &kept);
    assert_eq!(glb::read(&bytes).unwrap(), kept);

    let only_bad = StaticMesh {
        primitives: bad.to_vec(),
        ..StaticMesh::default()
    };
    assert_eq!(
        glb::read(&glb::write(&only_bad)).unwrap(),
        StaticMesh::default()
    );
}

#[test]
fn non_finite_values_never_reach_the_json() {
    let mut p = primitive("NaN", false, false, false);
    p.positions[2] = [f32::NAN, f32::INFINITY, 1.0];
    p.material.alpha = AlphaMode::Mask { cutoff: f32::NAN };
    p.material.opacity = f32::INFINITY;
    p.material.emissive = [f32::NEG_INFINITY, 0.5, f32::NAN];
    let mesh = StaticMesh {
        primitives: vec![p],
        ..StaticMesh::default()
    };
    let bytes = glb::write(&mesh);
    let (json, _) = split(&bytes);
    let json: Value = serde_json::from_str(&json).expect("the JSON stays valid");
    assert_eq!(json["materials"][0]["alphaCutoff"], 0);
    assert_eq!(json["materials"][0]["extras"]["mojave"]["opacity"], 0);
    assert!(glb::read(&bytes).is_ok());
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    for bytes in [glb::write(&rich_mesh()), glb::write(&mixed_mesh())] {
        assert!(glb::read(&bytes).is_ok());
        for len in 0..bytes.len() {
            assert!(
                glb::read(&bytes[..len]).is_err(),
                "prefix of {len} bytes read"
            );
        }
    }
}

#[test]
fn a_bad_container_is_rejected() {
    let good = glb::write(&rich_mesh());
    let patched = |offset: usize, value: u32| {
        let mut bytes = good.clone();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        bytes
    };
    assert!(read_error(&patched(0, 0x4654_6C68)).contains("magic"));
    assert!(read_error(&patched(4, 1)).contains("version 1"));
    assert!(read_error(&patched(8, good.len() as u32 + 1)).contains("bytes"));
    assert!(read_error(&patched(8, 11)).contains("bytes"));
    assert!(read_error(&patched(12, u32::MAX)).contains("does not fit"));
    assert!(read_error(&patched(16, 0x004E_4942)).contains("chunk 0"));
    let bin_header = 20 + u32_at(&good, 12) as usize;
    assert!(read_error(&patched(bin_header, u32::MAX - 3)).contains("does not fit"));
    assert!(read_error(&patched(bin_header + 4, 0x4E4F_534A)).contains("chunk 1"));

    assert!(read_error(b"").contains("truncated"));
    assert!(read_error(&assemble("{ not json", None)).contains("invalid glTF JSON"));
    assert!(read_error(&assemble("\u{feff}{}", None)).contains("invalid glTF JSON"));
    assert!(read_error(&assemble("{}", None)).contains("invalid glTF JSON"));
    let mut not_utf8 = assemble("{\"asset\":{\"version\":\"2.0\"}}", None);
    not_utf8[24] = 0xFF;
    assert!(read_error(&not_utf8).contains("invalid glTF JSON"));
    // A deeply nested document hits the JSON parser's depth limit; it does
    // not recurse until the stack runs out.
    let deep = format!("{}{}", "[".repeat(200_000), "]".repeat(200_000));
    assert!(read_error(&assemble(&deep, None)).contains("invalid glTF JSON"));
    // Someone else's glTF: valid, but without the marker in asset.extras.
    let foreign = "{\"asset\":{\"version\":\"2.0\"},\"nodes\":[{}]}";
    assert!(read_error(&assemble(foreign, None)).contains("was not written by"));
}

#[test]
fn a_bad_document_is_rejected() {
    let mesh = StaticMesh {
        primitives: vec![primitive("Only", true, true, true)],
        ..StaticMesh::default()
    };
    let (json, bin) = split(&glb::write(&mesh));
    assert_eq!(glb::read(&assemble(&json, Some(&bin))).unwrap(), mesh);

    let edited = |from: &str, to: &str| {
        assert!(
            json.contains(from),
            "the test's pattern {from} is not in the JSON"
        );
        assemble(&json.replacen(from, to, 1), Some(&bin))
    };
    // A count that runs past the buffer view.
    let error = read_error(&edited(
        "\"count\":4,\"type\":\"VEC3\"",
        "\"count\":5,\"type\":\"VEC3\"",
    ));
    assert!(error.contains("outside buffer view"), "{error}");
    // A count so large that the byte size overflows.
    let error = read_error(&edited(
        "\"count\":4,\"type\":\"VEC3\"",
        "\"count\":18446744073709551615,\"type\":\"VEC3\"",
    ));
    assert!(error.contains("outside buffer view"), "{error}");
    // A buffer view that runs past the binary chunk.
    let error = read_error(&edited(
        "\"byteLength\":48,\"target\"",
        "\"byteLength\":4800,\"target\"",
    ));
    assert!(error.contains("outside the"), "{error}");
    let error = read_error(&edited(
        "\"byteOffset\":0,\"byteLength\":48",
        "\"byteOffset\":18446744073709551615,\"byteLength\":48",
    ));
    assert!(error.contains("outside the"), "{error}");
    // Wrong component type, wrong element type.
    let error = read_error(&edited("\"componentType\":5126", "\"componentType\":5123"));
    assert!(error.contains("expected VEC3 of 5126"), "{error}");
    let error = read_error(&edited("\"type\":\"VEC2\"", "\"type\":\"VEC3\""));
    assert!(error.contains("expected VEC2"), "{error}");
    // References to things that do not exist.
    assert!(read_error(&edited("\"POSITION\":0", "\"POSITION\":77")).contains("accessor 77"));
    assert!(read_error(&edited("\"material\":0", "\"material\":3")).contains("material 3"));
    assert!(read_error(&edited("\"mesh\":0", "\"mesh\":2")).contains("mesh 2"));
    assert!(
        read_error(&edited("\"bufferView\":1", "\"bufferView\":40")).contains("buffer view 40")
    );
    assert!(read_error(&edited("\"buffer\":0", "\"buffer\":1")).contains("buffer 1"));
    // Missing pieces.
    assert!(read_error(&edited("\"POSITION\":0,", "")).contains("no POSITION"));
    assert!(read_error(&edited("\"mode\":4", "\"mode\":1")).contains("not a triangle list"));
    assert!(
        read_error(&edited("\"alphaMode\":\"OPAQUE\"", "\"alphaMode\":\"FOG\"")).contains("FOG")
    );
    assert!(
        read_error(&edited("\"units\":\"game\"", "\"units\":\"metres\""))
            .contains("was not written by")
    );
    assert!(
        read_error(&edited("\"version\":\"2.0\"", "\"version\":\"1.0\"")).contains("not supported")
    );

    // An index past the vertex count, changed in the binary chunk.
    let mut bad_bin = bin.clone();
    let last = bad_bin.len() - 4;
    bad_bin[last..].copy_from_slice(&4u32.to_le_bytes());
    let error = read_error(&assemble(&json, Some(&bad_bin)));
    assert!(
        error.contains("index 4 is past the vertex count 4"),
        "{error}"
    );

    // A file whose binary chunk is missing altogether.
    assert!(read_error(&assemble(&json, None)).contains("outside the 0-byte binary chunk"));
}

#[test]
fn mismatched_attribute_counts_are_rejected() {
    let mut first = primitive("First", true, false, false);
    first.positions.truncate(3);
    first.normals.as_mut().unwrap().truncate(3);
    first.indices = vec![0, 1, 2];
    let second = primitive("Second", false, false, false);
    let mesh = StaticMesh {
        primitives: vec![first, second],
        ..StaticMesh::default()
    };
    let (json, bin) = split(&glb::write(&mesh));
    // Accessors: 0 position (3), 1 normal (3), 2 indices, 3 position (4),
    // 4 indices. Give the first primitive the second one's positions.
    assert!(json.contains("\"POSITION\":0,\"NORMAL\":1"));
    let swapped = json.replacen(
        "\"POSITION\":0,\"NORMAL\":1",
        "\"POSITION\":3,\"NORMAL\":1",
        1,
    );
    let error = read_error(&assemble(&swapped, Some(&bin)));
    assert!(
        error.contains("NORMAL has 3 elements, POSITION has 4"),
        "{error}"
    );
}

struct XorShift(u64);

impl XorShift {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn corrupted_files_never_panic() {
    for source in [glb::write(&rich_mesh()), glb::write(&mixed_mesh())] {
        corrupt(&source);
    }
}

fn corrupt(source: &[u8]) {
    let mut rng = XorShift(0xD1B5_4A32_D192_ED03);
    for _ in 0..6000 {
        let mut bytes = source.to_vec();
        for _ in 0..1 + rng.next_u64() % 3 {
            let at = (rng.next_u64() % bytes.len() as u64) as usize;
            bytes[at] = match rng.next_u64() % 4 {
                0 => b'0' + (rng.next_u64() % 10) as u8,
                1 => 0xFF,
                2 => 0x00,
                _ => rng.next_u64() as u8,
            };
        }
        let _ = glb::read(&bytes);
    }
}

/// FNV-1a, 64 bits: enough to pin a byte stream in a test without a
/// hashing dependency.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// A downstream cache keys on the bytes of meshes without a skin. These
/// values were recorded before skins were added to the format; they must
/// never change.
#[test]
fn unskinned_output_bytes_are_pinned() {
    use nif::fixture::{box_mesh, quad};
    let building = building([800.0, 600.0, 300.0], "textures\\w.dds", "textures\\r.dds");
    let cases: [(&str, Vec<u8>); 5] = [
        (
            "box",
            nif_to_glb(&box_mesh([64.0, 32.0, 16.0], "textures\\b.dds")).unwrap(),
        ),
        (
            "quad",
            nif_to_glb(&quad([128.0, 128.0], "textures\\q.dds")).unwrap(),
        ),
        ("building", nif_to_glb(&building).unwrap()),
        ("rich", glb::write(&rich_mesh())),
        ("empty", glb::write(&StaticMesh::default())),
    ];
    let got: Vec<(&str, usize, u64)> = cases
        .iter()
        .map(|(name, bytes)| (*name, bytes.len(), fnv1a(bytes)))
        .collect();
    let expected: [(&str, usize, u64); 5] = [
        ("box", 2496, 1_234_196_817_884_676_395),
        ("quad", 1568, 2_878_458_479_582_297_299),
        ("building", 3780, 12_972_242_838_424_524_615),
        ("rich", 9860, 5_756_406_687_080_735_413),
        ("empty", 236, 14_227_830_426_048_942_768),
    ];
    assert_eq!(got, expected);
}

// ---------------------------------------------------------------- skins

fn skinned_mesh() -> StaticMesh {
    extract_mesh(&parse(&nif::fixture::skinned_body()).unwrap()).unwrap()
}

/// The skinned body twice (same bones, same bind) and an unskinned box.
fn mixed_mesh() -> StaticMesh {
    let body = skinned_mesh();
    let boxed = extract_mesh(&parse(&nif::fixture::box_mesh([4.0; 3], "textures\\b.dds")).unwrap())
        .unwrap();
    let mut second = body.primitives[0].clone();
    second.name = "Body:1".to_owned();
    StaticMesh {
        primitives: vec![
            body.primitives[0].clone(),
            boxed.primitives[0].clone(),
            second,
        ],
        ..body
    }
}

#[test]
fn skins_round_trip() {
    for mesh in [skinned_mesh(), mixed_mesh()] {
        let bytes = glb::write(&mesh);
        assert_eq!(glb::read(&bytes).unwrap(), mesh);
        assert_eq!(bytes, glb::write(&mesh.clone()), "deterministic");
    }
    // A bone list of one primitive in another order, and a different bind
    // for a shared name, still come back exactly.
    let mut mesh = mixed_mesh();
    let skin = mesh.primitives[2].skin.as_mut().unwrap();
    skin.bones.swap(0, 1);
    for row in &mut skin.weights {
        for slot in row.iter_mut() {
            slot.0 = match slot.0 {
                0 => 1,
                1 => 0,
                other => other,
            };
        }
    }
    skin.bones[2].bind_inverse[0][3] += 0.5;
    assert_eq!(glb::read(&glb::write(&mesh)).unwrap(), mesh);
}

/// Checks the skin parts of a written file by hand: nodes, skin, inverse
/// bind matrices, joint and weight attributes.
fn validate_skin(bytes: &[u8], mesh: &StaticMesh) -> usize {
    let (text, bin) = split(bytes);
    let json: Value = serde_json::from_str(&text).unwrap();
    let nodes = json["nodes"].as_array().unwrap();
    let skins = json["skins"].as_array().unwrap();
    assert_eq!(skins.len(), 1);
    let joints: Vec<usize> = skins[0]["joints"]
        .as_array()
        .unwrap()
        .iter()
        .map(index)
        .collect();
    // One scene root; the mesh node holds the skin; the joints hang under
    // a common skeleton node below it.
    assert_eq!(json["scenes"][0]["nodes"], serde_json::json!([0]));
    assert_eq!(nodes[0]["mesh"], 0);
    assert_eq!(nodes[0]["skin"], 0);
    assert_eq!(nodes[0]["children"], serde_json::json!([1]));
    assert_eq!(skins[0]["skeleton"], 1);
    let children: Vec<usize> = nodes[1]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(index)
        .collect();
    assert_eq!(children, joints);
    assert_eq!(nodes.len(), 2 + joints.len());
    assert_eq!(joints, (2..2 + joints.len()).collect::<Vec<_>>());

    let accessors = json["accessors"].as_array().unwrap();
    let views = json["bufferViews"].as_array().unwrap();
    let data = |i: usize, component: u64, kind: &str, size: usize| -> Vec<u8> {
        let accessor = &accessors[i];
        assert_eq!(accessor["componentType"], component);
        assert_eq!(accessor["type"], kind);
        let view = &views[index(&accessor["bufferView"])];
        let count = index(&accessor["count"]);
        let start = index(&view["byteOffset"]) + index(&accessor["byteOffset"]);
        assert_eq!(start % 4, 0);
        assert_eq!(count * size, index(&view["byteLength"]));
        bin[start..start + count * size].to_vec()
    };
    let floats = |b: &[u8]| -> Vec<f32> {
        b.chunks(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    };

    // Inverse bind matrices: one per joint, a view without a target, and
    // each the inverse of its joint node's matrix.
    let ibm_accessor = index(&skins[0]["inverseBindMatrices"]);
    let ibm_view = &views[index(&accessors[ibm_accessor]["bufferView"])];
    assert!(ibm_view.get("target").is_none());
    let ibm = floats(&data(ibm_accessor, FLOAT, "MAT4", 64));
    assert_eq!(ibm.len(), 16 * joints.len());
    for (j, &node) in joints.iter().enumerate() {
        let m = &ibm[16 * j..16 * j + 16];
        assert_eq!([m[3], m[7], m[11], m[15]], [0.0, 0.0, 0.0, 1.0]);
        assert!(nodes[node]["name"].is_string());
        let n: Vec<f32> = nodes[node]["matrix"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        // n · m = identity (column-major).
        for r in 0..4 {
            for c in 0..4 {
                let v: f32 = (0..4).map(|k| n[k * 4 + r] * m[c * 4 + k]).sum();
                let expected = if r == c { 1.0 } else { 0.0 };
                assert!((v - expected).abs() < 1e-4, "joint {j} [{r}][{c}] = {v}");
            }
        }
    }

    // Every primitive of the skinned mesh has joints and weights.
    let primitives = json["meshes"][0]["primitives"].as_array().unwrap();
    for (number, (primitive, source)) in primitives.iter().zip(&mesh.primitives).enumerate() {
        let attributes = &primitive["attributes"];
        let joint_data = data(index(&attributes["JOINTS_0"]), 5123, "VEC4", 8);
        let weight_data = floats(&data(index(&attributes["WEIGHTS_0"]), FLOAT, "VEC4", 16));
        let joint_view = &views[index(&accessors[index(&attributes["JOINTS_0"])]["bufferView"])];
        assert_eq!(joint_view["target"], ARRAY_BUFFER);
        let joint_values: Vec<u16> = joint_data
            .chunks(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(joint_values.len(), 4 * source.positions.len());
        for (v, (js, ws)) in joint_values
            .chunks(4)
            .zip(weight_data.chunks(4))
            .enumerate()
        {
            let sum: f32 = ws.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "primitive {number} vertex {v}");
            assert!(ws.iter().all(|&w| w >= 0.0));
            let mut used: Vec<u16> = js
                .iter()
                .zip(ws)
                .filter(|(_, w)| **w > 0.0)
                .map(|(j, _)| *j)
                .collect();
            assert!(used.iter().all(|&j| usize::from(j) < joints.len()));
            let before = used.len();
            used.sort_unstable();
            used.dedup();
            assert_eq!(used.len(), before, "a joint repeats with weight");
        }
        let skin_bones = &primitive["extras"]["mojave"]["skin_bones"];
        assert_eq!(skin_bones.is_array(), source.skin.is_some());
    }
    joints.len()
}

#[test]
fn skinned_files_are_structurally_valid_gltf() {
    let body = skinned_mesh();
    assert_eq!(validate_skin(&glb::write(&body), &body), 3);
    // Two primitives with the same bones share joints; the unskinned box
    // gets the rigid joint, which the reader drops again.
    let mixed = mixed_mesh();
    let bytes = glb::write(&mixed);
    assert_eq!(validate_skin(&bytes, &mixed), 4);
    let (text, _) = split(&bytes);
    let json: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["nodes"][5]["name"], glb::RIGID_JOINT);
    let back = glb::read(&bytes).unwrap();
    assert!(back.primitives[1].skin.is_none() && !back.primitives[1].skinned);
}

type JsonEdit = Box<dyn Fn(&mut Value)>;

/// Rewrites the JSON chunk of a written file.
fn edit_json(bytes: &[u8], edit: impl Fn(&mut Value)) -> Vec<u8> {
    let (text, bin) = split(bytes);
    let mut json: Value = serde_json::from_str(&text).unwrap();
    edit(&mut json);
    assemble(&serde_json::to_string(&json).unwrap(), Some(&bin))
}

#[test]
fn a_bad_skin_is_rejected() {
    let bytes = glb::write(&skinned_mesh());
    let cases: Vec<(JsonEdit, &str)> = vec![
        (
            Box::new(|j| {
                j["meshes"][0]["primitives"][0]["extras"]["mojave"]["skin_bones"] =
                    serde_json::json!([0, 1, 7])
            }),
            "does not exist",
        ),
        (
            Box::new(|j| {
                j["meshes"][0]["primitives"][0]["extras"]["mojave"]["skin_bones"] =
                    serde_json::json!([0, 1, 1])
            }),
            "listed twice",
        ),
        (
            Box::new(|j| {
                j["meshes"][0]["primitives"][0]["extras"]["mojave"]["skin_bones"] =
                    serde_json::json!([0, 1])
            }),
            "not one of its skin bones",
        ),
        (
            Box::new(|j| {
                j["meshes"][0]["primitives"][0]["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("WEIGHTS_0");
            }),
            "needs JOINTS_0 and WEIGHTS_0",
        ),
        (
            Box::new(|j| j["nodes"][0]["skin"] = serde_json::json!(3)),
            "skin 3 does not exist",
        ),
        (
            Box::new(|j| j["skins"][0]["joints"] = serde_json::json!([2, 3])),
            "inverse bind matrices",
        ),
        (
            Box::new(|j| j["skins"][0]["joints"] = serde_json::json!([2, 3, 99])),
            "joint node 99",
        ),
    ];
    for (edit, expected) in cases {
        let reason = read_error(&edit_json(&bytes, edit));
        assert!(reason.contains(expected), "{reason}");
    }
}
