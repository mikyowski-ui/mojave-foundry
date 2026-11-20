//! Header, block framing and block layouts, on files the fixture builder
//! writes.

use std::collections::BTreeMap;

use nif::fixture::{CollisionDesc, NifBuilder, NodeDesc, box_shape, building, quad_shape};
use nif::{
    AvObject, Block, BsSegmentedTriShape, BsShaderNoLighting, BsShaderPpLighting,
    BsShaderTextureSet, BsxFlags, CollisionObject, GeometryData, HEADER_LINE, Location,
    NiAlphaProperty, NiGeometry, NiMaterialProperty, NiNode, NiSourceTexture, NiStencilProperty,
    NiTexturingProperty, NiTriShapeData, NiTriStripsData, NifError, ObjectNet, RigidBodyPeek,
    ShaderProperty, Transform, TriSegment, extract_mesh, parse,
};

/// Offset of the block count in every file the builder writes: the header
/// line and its newline, the version, the endian byte and the user version.
const NUM_BLOCKS_OFFSET: usize = HEADER_LINE.len() + 1 + 4 + 1 + 4;

fn net(name: &str) -> ObjectNet {
    ObjectNet {
        name: name.to_owned(),
        extra_data: Vec::new(),
        controller: None,
    }
}

fn full_geometry_data() -> GeometryData {
    GeometryData {
        group_id: 7,
        num_vertices: 3,
        keep_flags: 1,
        compress_flags: 2,
        vertices: Some(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.5], [0.0, 2.0, -1.0]]),
        vector_flags: 0x1001,
        normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
        tangents: Some(vec![[1.0, 0.0, 0.0]; 3]),
        bitangents: Some(vec![[0.0, 1.0, 0.0]; 3]),
        center: [0.5, 1.0, -0.25],
        radius: 1.25,
        colors: Some(vec![
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 0.5],
            [0.0, 0.0, 1.0, 0.25],
        ]),
        uv_sets: vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]],
        consistency_flags: 0x4000,
        additional_data: None,
    }
}

fn bare_geometry_data() -> GeometryData {
    GeometryData {
        num_vertices: 4,
        vertices: Some(vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ]),
        radius: 0.75,
        ..GeometryData::default()
    }
}

/// One block of every type the parser reads field by field, with values
/// that differ from the defaults, wired so that every reference is valid.
fn every_block() -> Vec<(&'static str, Block)> {
    let shader = ShaderProperty {
        net: ObjectNet {
            name: "Shader".to_owned(),
            extra_data: vec![Some(11)],
            controller: Some(13),
        },
        flags: 3,
        shader_type: 29,
        shader_flags: 0x8240_0301,
        shader_flags2: 0x0000_8021,
        env_map_scale: 0.75,
        texture_clamp_mode: 2,
    };
    vec![
        (
            "BSFadeNode",
            NiNode {
                av: AvObject {
                    net: ObjectNet {
                        name: "Root".to_owned(),
                        extra_data: vec![Some(11)],
                        controller: None,
                    },
                    flags: 0x0008_000E,
                    translation: [1.0, 2.0, 3.0],
                    rotation: Transform::rotation_z(0.5),
                    scale: 1.5,
                    properties: vec![Some(7), None],
                    collision_object: Some(12),
                },
                children: vec![Some(1), None, Some(2), Some(14)],
                effects: vec![Some(13)],
                extra: Vec::new(),
            }
            .into(),
        ),
        (
            "NiTriShape",
            Block::tri_shape(NiGeometry {
                av: AvObject {
                    net: net("Shape"),
                    properties: vec![Some(5), Some(7), Some(8), Some(9)],
                    ..AvObject::default()
                },
                data: Some(3),
                skin_instance: None,
                material_names: vec!["MatA".to_owned(), "MatB".to_owned()],
                material_extra_data: vec![7, -1],
                active_material: 1,
                dirty: true,
            }),
        ),
        (
            "NiTriStrips",
            Block::tri_strips(NiGeometry {
                av: AvObject {
                    net: net("Strips"),
                    flags: 0x0F,
                    translation: [-4.0, 0.25, 9.0],
                    ..AvObject::default()
                },
                data: Some(4),
                skin_instance: Some(13),
                material_names: Vec::new(),
                material_extra_data: Vec::new(),
                active_material: -1,
                dirty: false,
            }),
        ),
        (
            "NiTriShapeData",
            NiTriShapeData {
                geom: full_geometry_data(),
                num_triangles: 1,
                num_triangle_points: 3,
                triangles: Some(vec![[0, 1, 2]]),
                match_groups: vec![vec![0, 1], vec![2]],
            }
            .into(),
        ),
        (
            "NiTriStripsData",
            NiTriStripsData {
                geom: bare_geometry_data(),
                num_triangles: 2,
                strip_lengths: vec![4, 3],
                strips: Some(vec![vec![0, 1, 2, 3], vec![3, 2, 1]]),
            }
            .into(),
        ),
        (
            "BSShaderPPLightingProperty",
            BsShaderPpLighting {
                shader: shader.clone(),
                texture_set: Some(6),
                refraction_strength: 0.125,
                refraction_fire_period: 42,
                parallax_max_passes: 8.0,
                parallax_scale: 0.5,
            }
            .into(),
        ),
        (
            "BSShaderTextureSet",
            BsShaderTextureSet {
                textures: vec![
                    "Textures\\Synthetic\\Wall.dds".to_owned(),
                    "textures/synthetic/wall_n.dds".to_owned(),
                    String::new(),
                    String::new(),
                    "textures\\synthetic\\cube_e.dds".to_owned(),
                    String::new(),
                ],
            }
            .into(),
        ),
        (
            "NiAlphaProperty",
            NiAlphaProperty {
                net: net("Alpha"),
                flags: 0x12ED,
                threshold: 128,
            }
            .into(),
        ),
        (
            "NiMaterialProperty",
            NiMaterialProperty {
                net: net("Material"),
                specular: [0.1, 0.2, 0.3],
                emissive: [0.4, 0.5, 0.6],
                glossiness: 25.0,
                alpha: 0.75,
                emissive_multiple: 2.0,
            }
            .into(),
        ),
        (
            "NiStencilProperty",
            NiStencilProperty {
                net: net("Stencil"),
                flags: 0x4D80,
                stencil_ref: 5,
                stencil_mask: 0xFFFF_FF00,
            }
            .into(),
        ),
        (
            "BSShaderNoLightingProperty",
            BsShaderNoLighting {
                shader: shader.clone(),
                file_name: "textures\\synthetic\\sign.dds".to_owned(),
                extra: (0u8..16).collect(),
            }
            .into(),
        ),
        (
            "BSXFlags",
            BsxFlags {
                name: "BSX".to_owned(),
                flags: 0x22,
            }
            .into(),
        ),
        (
            "bhkCollisionObject",
            CollisionObject {
                target: Some(0),
                flags: 0x0081,
                body: Some(13),
                extra: Vec::new(),
            }
            .into(),
        ),
        ("NiSomethingUnknown", Block::Skipped),
        (
            "NiSwitchNode",
            NiNode {
                av: AvObject {
                    net: net("Switch"),
                    ..AvObject::default()
                },
                children: vec![None, None],
                effects: Vec::new(),
                extra: vec![0x02, 0x00, 0x01, 0x00, 0x00, 0x00],
            }
            .into(),
        ),
        (
            "bhkBlendCollisionObject",
            CollisionObject {
                target: Some(14),
                flags: 9,
                body: None,
                extra: vec![0, 0, 128, 63, 0, 0, 128, 63],
            }
            .into(),
        ),
        (
            "Lighting30ShaderProperty",
            BsShaderPpLighting {
                shader,
                texture_set: None,
                ..BsShaderPpLighting::default()
            }
            .into(),
        ),
        (
            "BSSegmentedTriShape",
            BsSegmentedTriShape {
                shape: NiGeometry {
                    av: AvObject {
                        net: net("Segmented"),
                        properties: vec![Some(18)],
                        ..AvObject::default()
                    },
                    data: Some(3),
                    skin_instance: None,
                    material_names: Vec::new(),
                    material_extra_data: Vec::new(),
                    active_material: -1,
                    dirty: true,
                },
                segments: vec![
                    TriSegment {
                        flags: 0,
                        start: 0,
                        triangles: 1,
                    },
                    TriSegment {
                        flags: 3,
                        start: 3,
                        triangles: 0,
                    },
                ],
            }
            .into(),
        ),
        (
            "NiTexturingProperty",
            NiTexturingProperty {
                net: net("Texturing"),
                flags: 4,
                texture_count: 9,
                has_base_texture: true,
                base_source: Some(19),
                // The rest of the base slot (flags, no transform), eight
                // unused slots and an empty shader texture list.
                extra: vec![0x02, 0x31, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            }
            .into(),
        ),
        (
            "NiSourceTexture",
            NiSourceTexture {
                net: net("Source"),
                use_external: 1,
                file_name: "Textures\\Synthetic\\Old.dds".to_owned(),
                link: None,
                pixel_layout: 6,
                use_mipmaps: 1,
                alpha_format: 3,
                is_static: 1,
                direct_render: 1,
                persist_render_data: 0,
            }
            .into(),
        ),
    ]
}

fn build_every_block() -> (NifBuilder, Vec<(&'static str, Block)>) {
    let blocks = every_block();
    let mut builder = NifBuilder::new();
    for (type_name, block) in &blocks {
        if block.is_skipped() {
            builder.push_raw(type_name, vec![0xAB; 23]);
        } else {
            builder.push(type_name, block);
        }
    }
    builder.add_root(Some(0));
    (builder, blocks)
}

#[test]
fn header_fields_are_read() {
    let mut builder = NifBuilder::new();
    builder
        .export_strings("an author", "a process script", "an export script")
        .groups(&[3, 1]);
    builder.add_scene(&NodeDesc::root("Scene Root").with_shape(quad_shape(
        "Quad",
        [2.0, 2.0],
        "textures\\a.dds",
    )));
    let bytes = builder.build();
    let file = parse(&bytes).unwrap();
    let header = &file.header;

    assert_eq!(header.header_line, HEADER_LINE);
    assert_eq!(header.version, 0x1402_0007);
    assert_eq!(header.endian, 1);
    assert_eq!(header.user_version, 11);
    assert_eq!(header.stream_version, 34);
    assert_eq!(
        header.export_strings,
        ["an author", "a process script", "an export script"]
    );
    assert_eq!(
        header.block_types,
        [
            "BSFadeNode",
            "NiTriShape",
            "BSShaderPPLightingProperty",
            "BSShaderTextureSet",
            "NiTriShapeData"
        ]
    );
    assert_eq!(header.block_type_index, [0, 1, 2, 3, 4]);
    assert_eq!(header.num_blocks(), 5);
    let mut strings = header.strings.clone();
    strings.sort();
    assert_eq!(strings, ["Quad", "Scene Root"]);
    assert_eq!(header.max_string_length, 10);
    assert_eq!(header.groups, [3, 1]);
    assert_eq!(file.roots, [Some(0)]);

    // Blocks are contiguous, sized as the header says, and the footer (a
    // count and one root) is all that follows them.
    let mut offset = file.blocks[0].offset;
    for (entry, &size) in file.blocks.iter().zip(&header.block_sizes) {
        assert_eq!(entry.offset, offset);
        assert_eq!(entry.size, size);
        offset += size as usize;
    }
    assert_eq!(offset + 8, bytes.len());
}

#[test]
fn every_parsed_block_type_round_trips_field_by_field() {
    let (builder, blocks) = build_every_block();
    let file = parse(&builder.build()).unwrap();
    assert_eq!(file.blocks.len(), blocks.len());
    for (index, ((type_name, expected), entry)) in blocks.iter().zip(&file.blocks).enumerate() {
        assert_eq!(entry.type_name, *type_name, "block {index}");
        assert_eq!(&entry.block, expected, "block {index} ({type_name})");
    }
    // Spot checks on the layouts with a known byte size.
    let size_of = |name: &str| {
        file.blocks
            .iter()
            .find(|e| e.type_name == name)
            .map(|e| e.size)
            .unwrap()
    };
    assert_eq!(size_of("NiAlphaProperty"), 15);
    assert_eq!(size_of("NiMaterialProperty"), 48);
    assert_eq!(size_of("NiStencilProperty"), 22);
    assert_eq!(size_of("BSXFlags"), 8);
    assert_eq!(size_of("bhkCollisionObject"), 10);
    // 54 bytes with no extra data; this one has a single extra data entry.
    assert_eq!(size_of("Lighting30ShaderProperty"), 58);
    assert_eq!(size_of("NiSomethingUnknown"), 23);
    assert_eq!(size_of("NiSourceTexture"), 36);
    assert_eq!(size_of("NiTexturingProperty"), 38);
    // A triangle shape with one property is 97 bytes; then a segment count
    // and nine bytes per segment.
    assert_eq!(size_of("BSSegmentedTriShape"), 97 + 4 + 2 * 9);
}

#[test]
fn unknown_blocks_are_skipped_by_size_and_counted() {
    let scene = NodeDesc::root("Root")
        .with_collision(CollisionDesc::STATIC)
        .with_unknown("NiPointLight", vec![0x5A; 37])
        .with_shape(box_shape("First", [2.0, 2.0, 2.0], "textures\\a.dds"))
        .with_unknown("BSFurnitureMarker", vec![1, 2, 3])
        .with_unknown("BSFurnitureMarker", Vec::new())
        .with_shape(quad_shape("Second", [1.0, 1.0], "textures\\b.dds"));
    let file = parse(&scene.to_nif()).unwrap();

    let expected: BTreeMap<String, u32> = [
        ("BSFurnitureMarker", 2),
        ("NiPointLight", 1),
        ("bhkBoxShape", 1),
        ("bhkRigidBody", 1),
    ]
    .into_iter()
    .map(|(name, count)| (name.to_owned(), count))
    .collect();
    assert_eq!(file.skipped_blocks(), expected);
    assert!(
        file.blocks
            .iter()
            .filter(|e| e.type_name == "BSFurnitureMarker")
            .all(|e| e.block == Block::Skipped)
    );

    // The shapes on either side of the unknown blocks are intact.
    let mesh = extract_mesh(&file).unwrap();
    assert_eq!(mesh.skipped_blocks, expected);
    let names: Vec<&str> = mesh.primitives.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["First", "Second"]);
    assert_eq!(mesh.primitives[0].positions.len(), 24);
    assert_eq!(mesh.primitives[1].positions.len(), 4);
}

#[test]
fn rigid_body_is_peeked_and_still_counted_as_skipped() {
    let scene = NodeDesc::root("Root").with_collision(CollisionDesc {
        flags: 0x41,
        layer: 9,
        motion_system: 6,
    });
    let file = parse(&scene.to_nif()).unwrap();
    let body = file
        .blocks
        .iter()
        .find(|e| e.type_name == "bhkRigidBody")
        .unwrap();
    assert_eq!(
        body.block,
        Block::RigidBody(RigidBodyPeek {
            layer: Some(9),
            motion_system: Some(6),
        })
    );
    assert!(body.block.is_skipped());

    // A rigid body too short to hold the fields gives no values, no error.
    let mut builder = NifBuilder::new();
    builder.push_raw("bhkRigidBodyT", vec![0; 5]);
    let file = parse(&builder.build()).unwrap();
    assert_eq!(
        file.blocks[0].block,
        Block::RigidBody(RigidBodyPeek {
            layer: Some(0),
            motion_system: None,
        })
    );
}

#[test]
fn a_block_longer_than_its_layout_is_a_size_mismatch() {
    let (mut builder, blocks) = build_every_block();
    let index = blocks
        .iter()
        .position(|(name, _)| *name == "NiAlphaProperty")
        .unwrap() as u32;
    let offset = parse(&builder.build()).unwrap().blocks[index as usize].offset;

    builder
        .payload_mut(index)
        .unwrap()
        .extend_from_slice(&[0; 4]);
    let error = parse(&builder.build()).unwrap_err();
    assert_eq!(
        error,
        NifError::BlockSizeMismatch {
            index,
            type_name: "NiAlphaProperty".to_owned(),
            offset,
            declared: 19,
            consumed: 15,
        }
    );
    let message = error.to_string();
    assert!(message.contains("NiAlphaProperty"), "{message}");
    assert!(message.contains(&format!("{offset:#x}")), "{message}");
    assert!(message.contains("declared size is 19"), "{message}");
    assert!(message.contains("consumed 15"), "{message}");
    assert!(message.contains("leaving 4 unread"), "{message}");
    assert!(
        message.contains(&format!("{:#x}", offset + 15)),
        "{message}"
    );
}

#[test]
fn a_block_shorter_than_its_layout_names_the_block() {
    let (mut builder, blocks) = build_every_block();
    let index = blocks
        .iter()
        .position(|(name, _)| *name == "NiMaterialProperty")
        .unwrap() as u32;
    let offset = parse(&builder.build()).unwrap().blocks[index as usize].offset;

    // The header claims 40 bytes for a 48-byte layout.
    builder.declare_size(index, 40);
    match parse(&builder.build()).unwrap_err() {
        NifError::Truncated {
            location,
            offset: at,
            needed,
            remaining,
            ..
        } => {
            assert_eq!(
                location,
                Location::Block {
                    index,
                    type_name: "NiMaterialProperty".to_owned()
                }
            );
            // The field that does not fit starts where the declared size ends.
            assert_eq!(at, offset + 40);
            assert_eq!(needed, 4);
            assert_eq!(remaining, 0);
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn exact_node_types_are_size_checked_and_subclasses_keep_their_tail() {
    let node = NiNode {
        av: AvObject {
            net: net("N"),
            ..AvObject::default()
        },
        ..NiNode::default()
    };
    for type_name in ["NiNode", "BSFadeNode"] {
        let mut builder = NifBuilder::new();
        let index = builder.push(type_name, &node.clone().into());
        builder.payload_mut(index).unwrap().push(0);
        assert!(matches!(
            parse(&builder.build()),
            Err(NifError::BlockSizeMismatch { .. })
        ));
    }
    for type_name in nif::NODE_PREFIX_TYPES {
        let mut builder = NifBuilder::new();
        let index = builder.push(type_name, &node.clone().into());
        builder
            .payload_mut(index)
            .unwrap()
            .extend_from_slice(&[1, 2, 3]);
        let file = parse(&builder.build()).unwrap();
        let Block::Node(parsed) = &file.blocks[0].block else {
            panic!("{type_name} was not parsed as a node");
        };
        assert_eq!(parsed.av, node.av);
        assert_eq!(parsed.extra, [1, 2, 3]);
    }
}

#[test]
fn other_versions_are_rejected_with_a_clear_error() {
    let good = building([8.0, 6.0, 4.0], "textures\\w.dds", "textures\\r.dds");
    let version_at = HEADER_LINE.len() + 1;

    let mut older = good.clone();
    older[version_at..version_at + 4].copy_from_slice(&0x1400_0005u32.to_le_bytes());
    let error = parse(&older).unwrap_err();
    assert!(
        matches!(&error, NifError::UnsupportedVersion { version, .. } if version == "20.0.0.5"),
        "{error}"
    );
    assert!(error.to_string().contains("only 20.2.0.7 / 11 / 34"));

    // Right file version, a later game's user and stream versions.
    let mut later = good.clone();
    later[version_at + 5..version_at + 9].copy_from_slice(&12u32.to_le_bytes());
    later[version_at + 13..version_at + 17].copy_from_slice(&83u32.to_le_bytes());
    assert_eq!(
        parse(&later).unwrap_err(),
        NifError::UnsupportedVersion {
            offset: version_at + 5,
            version: "20.2.0.7".to_owned(),
            user_version: 12,
            stream_version: 83,
        }
    );

    let mut big_endian = good.clone();
    big_endian[version_at + 4] = 0;
    assert!(matches!(
        parse(&big_endian),
        Err(NifError::BadValue {
            location: Location::Header,
            value: 0,
            ..
        })
    ));

    let ancient = b"NetImmerse File Format, Version 4.0.0.2\n\x02\x00\x00\x04";
    assert!(matches!(
        parse(ancient),
        Err(NifError::UnsupportedVersion { version, .. }) if version == "4.0.0.2"
    ));

    assert!(matches!(parse(b""), Err(NifError::NotNif { .. })));
    assert!(matches!(
        parse(b"DDS \x7c\x00\x00\x00 not a mesh at all"),
        Err(NifError::NotNif { .. })
    ));
    assert!(matches!(
        parse(b"Gamebryo File Format, Version 20.2.0.7"),
        Err(NifError::NotNif { .. })
    ));
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    let bytes = building([8.0, 6.0, 4.0], "textures\\w.dds", "textures\\r.dds");
    assert!(parse(&bytes).is_ok());
    for len in 0..bytes.len() {
        assert!(
            parse(&bytes[..len]).is_err(),
            "prefix of {len} bytes parsed"
        );
    }
}

#[test]
fn absurd_counts_are_rejected_before_any_allocation() {
    let bytes = building([8.0, 6.0, 4.0], "textures\\w.dds", "textures\\r.dds");
    let file = parse(&bytes).unwrap();

    // Block count in the header.
    let mut bad = bytes.clone();
    bad[NUM_BLOCKS_OFFSET..NUM_BLOCKS_OFFSET + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        parse(&bad),
        Err(NifError::CountTooLarge {
            location: Location::Header,
            offset: NUM_BLOCKS_OFFSET,
            what: "block",
            count: 0xFFFF_FFFF,
            ..
        })
    ));

    // Vertex count of a geometry data block (after the four-byte group id).
    let (data_index, data) = file
        .blocks
        .iter()
        .enumerate()
        .find(|(_, e)| e.type_name == "NiTriShapeData")
        .unwrap();
    let mut bad = bytes.clone();
    bad[data.offset + 4..data.offset + 6].copy_from_slice(&u16::MAX.to_le_bytes());
    match parse(&bad).unwrap_err() {
        NifError::CountTooLarge {
            location,
            what,
            count,
            ..
        } => {
            assert_eq!(
                location,
                Location::Block {
                    index: data_index as u32,
                    type_name: "NiTriShapeData".to_owned()
                }
            );
            assert_eq!(what, "vertices");
            assert_eq!(count, 65535);
        }
        other => panic!("unexpected error: {other}"),
    }

    // Extra data count of the root node (after the four-byte name).
    let root = &file.blocks[0];
    let mut bad = bytes.clone();
    bad[root.offset + 4..root.offset + 8].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
    assert!(matches!(
        parse(&bad),
        Err(NifError::CountTooLarge {
            what: "extra data list",
            ..
        })
    ));

    // Footer root count.
    let mut bad = bytes.clone();
    let footer = bytes.len() - 8;
    bad[footer..footer + 4].copy_from_slice(&0x4000_0000u32.to_le_bytes());
    assert!(matches!(
        parse(&bad),
        Err(NifError::CountTooLarge {
            location: Location::Footer,
            ..
        })
    ));
}

#[test]
fn bad_references_strings_and_booleans_are_rejected() {
    let bytes = building([8.0, 6.0, 4.0], "textures\\w.dds", "textures\\r.dds");
    let file = parse(&bytes).unwrap();
    let num_blocks = file.blocks.len() as u32;

    // The footer's root reference points past the last block.
    let mut bad = bytes.clone();
    let root_ref = bytes.len() - 4;
    bad[root_ref..].copy_from_slice(&num_blocks.to_le_bytes());
    assert_eq!(
        parse(&bad).unwrap_err(),
        NifError::BadValue {
            location: Location::Footer,
            offset: root_ref,
            what: "root list",
            value: i64::from(num_blocks),
        }
    );
    // -2 is not "none".
    bad[root_ref..].copy_from_slice(&(-2i32).to_le_bytes());
    assert!(matches!(
        parse(&bad),
        Err(NifError::BadValue { value: -2, .. })
    ));

    // The root node's name index points past the string table.
    let root = &file.blocks[0];
    let mut bad = bytes.clone();
    bad[root.offset..root.offset + 4].copy_from_slice(&1000u32.to_le_bytes());
    assert!(matches!(
        parse(&bad),
        Err(NifError::BadValue {
            what: "name",
            value: 1000,
            ..
        })
    ));

    // "Has vertices" is a boolean; 2 means the parser is misaligned.
    let data = file
        .blocks
        .iter()
        .find(|e| e.type_name == "NiTriShapeData")
        .unwrap();
    let mut bad = bytes.clone();
    bad[data.offset + 8] = 2;
    assert!(matches!(
        parse(&bad),
        Err(NifError::BadValue {
            what: "has vertices",
            value: 2,
            ..
        })
    ));
}

#[test]
fn a_block_type_index_past_the_type_table_is_rejected() {
    let mut builder = NifBuilder::new();
    builder.push_raw("OnlyType", vec![0; 4]);
    let mut bytes = builder.build();
    // The type index follows the type table: a u16 count, then one sized
    // string. It is the two bytes before the four-byte block size.
    let type_index_at = bytes
        .windows(8)
        .position(|w| w == b"OnlyType")
        .map(|p| p + 8)
        .unwrap();
    bytes[type_index_at] = 5;
    assert!(matches!(
        parse(&bytes),
        Err(NifError::BadValue {
            location: Location::Header,
            what: "block type index",
            value: 5,
            ..
        })
    ));
}

/// A small deterministic generator, so that the corruption test needs no
/// dependency and always tries the same inputs.
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
    let (builder, _) = build_every_block();
    let sources = [
        builder.build(),
        building([8.0, 6.0, 4.0], "textures\\w.dds", "textures\\r.dds"),
    ];
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for source in &sources {
        for _ in 0..4000 {
            let mut bytes = source.clone();
            for _ in 0..1 + rng.next_u64() % 4 {
                let at = (rng.next_u64() % bytes.len() as u64) as usize;
                bytes[at] = match rng.next_u64() % 4 {
                    0 => 0x00,
                    1 => 0xFF,
                    2 => 0x7F,
                    _ => rng.next_u64() as u8,
                };
            }
            // Whatever happens, it must return.
            if let Ok(file) = parse(&bytes) {
                if let Ok(mesh) = extract_mesh(&file) {
                    let glb = nif::glb::write(&mesh);
                    assert!(nif::glb::read(&glb).is_ok());
                }
                let _ = file.texture_paths();
            }
        }
    }
}
