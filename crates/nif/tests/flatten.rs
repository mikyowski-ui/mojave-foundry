//! Scene flattening: transforms, strips, visibility, materials, collision
//! and hostile graphs.

use nif::fixture::{
    ALPHA_FLAGS_BLEND, ALPHA_FLAGS_BLEND_AND_TEST, ALPHA_FLAGS_TEST, ChildDesc, CollisionDesc,
    NifBuilder, NodeDesc, ShapeDesc, Topology, quad_shape,
};
use nif::{
    AlphaMode, AvObject, Block, BsSegmentedTriShape, BsShaderNoLighting, BsShaderPpLighting,
    BsShaderTextureSet, CollisionInfo, ExtractOptions, FLAG_HIDDEN, GeometryData, Material,
    NiAlphaProperty, NiGeometry, NiMaterialProperty, NiNode, NiSourceTexture, NiTexturingProperty,
    NiTriShapeData, NifError, ObjectNet, ShaderInfo, StaticMesh, Transform, TriSegment,
    expand_strips, extract_mesh, extract_mesh_with, normalize_texture_path, parse,
};

const QUARTER_TURN: [[f32; 3]; 3] = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];

fn triangle(name: &str) -> ShapeDesc {
    ShapeDesc::new(
        name,
        vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        Topology::Triangles(vec![[0, 1, 2]]),
    )
}

fn flatten(scene: &NodeDesc) -> StaticMesh {
    extract_mesh(&parse(&scene.to_nif()).unwrap()).unwrap()
}

/// The normal of a triangle from its winding (not normalised).
fn winding_normal(positions: &[[f32; 3]], t: &[u32]) -> [f32; 3] {
    let [a, b, c] = [
        positions[t[0] as usize],
        positions[t[1] as usize],
        positions[t[2] as usize],
    ];
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

fn named(name: &str) -> AvObject {
    AvObject {
        net: ObjectNet {
            name: name.to_owned(),
            ..ObjectNet::default()
        },
        ..AvObject::default()
    }
}

fn node_block(name: &str, children: &[u32]) -> Block {
    NiNode {
        av: named(name),
        children: children.iter().map(|&c| Some(c)).collect(),
        ..NiNode::default()
    }
    .into()
}

fn shape_block(name: &str, data: u32) -> Block {
    Block::tri_shape(NiGeometry {
        av: named(name),
        data: Some(data),
        active_material: -1,
        ..NiGeometry::default()
    })
}

fn triangle_data(vertices: Vec<[f32; 3]>, triangles: Vec<[u16; 3]>) -> NiTriShapeData {
    NiTriShapeData {
        geom: GeometryData {
            num_vertices: vertices.len() as u16,
            vertices: Some(vertices),
            ..GeometryData::default()
        },
        num_triangles: triangles.len() as u16,
        num_triangle_points: triangles.len() as u32 * 3,
        triangles: Some(triangles),
        match_groups: Vec::new(),
    }
}

fn unit_triangle_data() -> Block {
    triangle_data(
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        vec![[0, 1, 2]],
    )
    .into()
}

#[test]
fn node_transforms_are_composed_and_baked() {
    let mut shape = triangle("Tri").with_transform(Transform::from_translation([1.0, 0.0, 0.0]));
    shape.normals = Some(vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    let scene = NodeDesc::root("Root")
        .with_transform(Transform::from_translation([100.0, 0.0, 0.0]))
        .with_node(
            NodeDesc::new("NiNode", "Child")
                .with_transform(Transform {
                    rotation: QUARTER_TURN,
                    translation: [10.0, 20.0, 30.0],
                    scale: 2.0,
                })
                .with_shape(shape),
        );
    let mesh = flatten(&scene);
    assert_eq!(mesh.primitives.len(), 1);
    let primitive = &mesh.primitives[0];
    // (1,0,0): shape +1 in x -> (2,0,0); quarter turn -> (0,2,0); scale 2
    // -> (0,4,0); child translation -> (10,24,30); root -> (110,24,30).
    assert_eq!(
        primitive.positions,
        [
            [110.0, 24.0, 30.0],
            [108.0, 22.0, 30.0],
            [110.0, 22.0, 32.0]
        ]
    );
    // Normals turn with the rotation and ignore scale and translation.
    assert_eq!(
        primitive.normals.as_deref().unwrap(),
        [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]
    );
    assert_eq!(primitive.indices, [0, 1, 2]);
}

#[test]
fn rotation_rows_act_on_column_vectors() {
    let turn = Transform {
        rotation: Transform::rotation_z(std::f32::consts::FRAC_PI_2),
        ..Transform::IDENTITY
    };
    let [x, y, z] = turn.apply([1.0, 0.0, 0.0]);
    assert!(x.abs() < 1e-6 && (y - 1.0).abs() < 1e-6 && z == 0.0);

    let parent = Transform {
        rotation: QUARTER_TURN,
        translation: [5.0, 0.0, 0.0],
        scale: 3.0,
    };
    let child = Transform {
        rotation: QUARTER_TURN,
        translation: [1.0, 2.0, 3.0],
        scale: 0.5,
    };
    let p = [7.0, -2.0, 4.0];
    assert_eq!(parent.then(&child).apply(p), parent.apply(child.apply(p)));
}

#[test]
fn strips_are_expanded_and_degenerates_dropped() {
    // Two triangles joined by a repeated vertex: positions 1 and 2 of the
    // strip are degenerate and position 3 is odd, so it is flipped.
    assert_eq!(
        expand_strips(&[vec![0, 1, 2, 2, 3, 4]]),
        [[0, 1, 2], [2, 4, 3]]
    );
    assert_eq!(
        expand_strips(&[vec![0, 1, 2, 3], vec![4, 5], vec![], vec![6, 7, 8]]),
        [[0, 1, 2], [1, 3, 2], [6, 7, 8]]
    );

    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 2.0, 0.0],
        [1.0, 2.0, 0.0],
    ];
    let strip = ShapeDesc::new(
        "Strip",
        positions,
        Topology::Strips(vec![vec![0, 1, 2, 3, 3, 3, 4, 5], vec![0, 1, 1]]),
    );
    let mesh = flatten(&NodeDesc::root("Root").with_shape(strip));
    let primitive = &mesh.primitives[0];
    // Strip positions 2, 3 and 4 repeat a vertex; position 5 is odd.
    assert_eq!(primitive.indices, [0, 1, 2, 1, 3, 2, 3, 5, 4]);
    // Winding is consistent: every triangle of this flat strip faces +Z.
    for t in primitive.indices.chunks(3) {
        assert!(winding_normal(&primitive.positions, t)[2] > 0.0, "{t:?}");
    }
}

#[test]
fn degenerate_triangles_in_a_triangle_list_are_dropped() {
    let shape = ShapeDesc::new(
        "Tri",
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        Topology::Triangles(vec![[0, 0, 1], [0, 1, 2], [2, 1, 2]]),
    );
    let mesh = flatten(&NodeDesc::root("Root").with_shape(shape));
    assert_eq!(mesh.primitives[0].indices, [0, 1, 2]);

    // A shape left with no triangle at all gives no primitive.
    let empty = ShapeDesc::new(
        "Empty",
        vec![[0.0; 3], [1.0, 0.0, 0.0]],
        Topology::Strips(vec![vec![0, 1, 1, 0]]),
    );
    assert!(
        flatten(&NodeDesc::root("Root").with_shape(empty))
            .primitives
            .is_empty()
    );
}

#[test]
fn mirroring_transforms_reverse_the_winding() {
    let mut shape = quad_shape("Quad", [2.0, 2.0], "");
    shape.tangents = false;
    for transform in [
        Transform {
            scale: -1.0,
            ..Transform::IDENTITY
        },
        Transform {
            rotation: [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]],
            ..Transform::IDENTITY
        },
        Transform::IDENTITY,
    ] {
        let scene = NodeDesc::root("Root").with_node(
            NodeDesc::new("NiNode", "Flip")
                .with_transform(transform)
                .with_shape(shape.clone()),
        );
        let mesh = flatten(&scene);
        let primitive = &mesh.primitives[0];
        let normals = primitive.normals.as_ref().unwrap();
        for t in primitive.indices.chunks(3) {
            let face = winding_normal(&primitive.positions, t);
            let stored = normals[t[0] as usize];
            let dot = face[0] * stored[0] + face[1] * stored[1] + face[2] * stored[2];
            assert!(dot > 0.0, "winding and normal disagree under {transform:?}");
        }
    }
}

#[test]
fn hidden_nodes_and_shapes_are_left_out() {
    let mut hidden_shape = triangle("HiddenShape");
    hidden_shape.hidden = true;
    let mut hidden_node = NodeDesc::new("NiNode", "HiddenNode").with_shape(triangle("UnderHidden"));
    hidden_node.hidden = true;
    let scene = NodeDesc::root("Root")
        .with_shape(triangle("Visible"))
        .with_shape(hidden_shape)
        .with_node(hidden_node)
        .with_node(NodeDesc::new("NiNode", "Shown").with_shape(triangle("UnderShown")));
    let file = parse(&scene.to_nif()).unwrap();

    let names = |mesh: &StaticMesh| -> Vec<String> {
        mesh.primitives.iter().map(|p| p.name.clone()).collect()
    };
    assert_eq!(
        names(&extract_mesh(&file).unwrap()),
        ["Visible", "UnderShown"]
    );
    let all = extract_mesh_with(
        &file,
        ExtractOptions {
            include_hidden: true,
            ..ExtractOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        names(&all),
        ["Visible", "HiddenShape", "UnderHidden", "UnderShown"]
    );
}

#[test]
fn editor_markers_are_left_out_by_name() {
    let scene = NodeDesc::root("Root")
        .with_shape(triangle("EditorMarker"))
        .with_shape(triangle("editormarker:0"))
        .with_node(NodeDesc::new("NiNode", "EditorMarkerNode").with_shape(triangle("Inside")))
        .with_shape(triangle("Editor"))
        .with_shape(triangle("MyEditorMarker"));
    let file = parse(&scene.to_nif()).unwrap();
    let names: Vec<String> = extract_mesh(&file)
        .unwrap()
        .primitives
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Editor", "MyEditorMarker"]);
    let all = extract_mesh_with(
        &file,
        ExtractOptions {
            include_editor_markers: true,
            ..ExtractOptions::default()
        },
    )
    .unwrap();
    assert_eq!(all.primitives.len(), 5);
}

#[test]
fn switch_and_lod_nodes_show_one_child() {
    let children = |node: NodeDesc| {
        node.with_shape(triangle("Zero"))
            .with_shape(triangle("One"))
            .with_shape(triangle("Two"))
    };
    let names = |node: NodeDesc| -> Vec<String> {
        flatten(&NodeDesc::root("Root").with_node(node))
            .primitives
            .into_iter()
            .map(|p| p.name)
            .collect()
    };

    let mut switch = children(NodeDesc::new("NiSwitchNode", "Switch"));
    switch.extra = vec![0, 0, 1, 0, 0, 0]; // flags, then index 1
    assert_eq!(names(switch.clone()), ["One"]);
    switch.extra = vec![0, 0, 9, 0, 0, 0]; // an index with no child: all
    assert_eq!(names(switch.clone()), ["Zero", "One", "Two"]);
    switch.extra = Vec::new();
    assert_eq!(names(switch), ["Zero", "One", "Two"]);

    let mut lod = children(NodeDesc::new("NiLODNode", "Lod"));
    lod.extra = vec![0; 10];
    assert_eq!(names(lod), ["Zero"]);

    let ordered = children(NodeDesc::new("BSOrderedNode", "Ordered"));
    assert_eq!(names(ordered), ["Zero", "One", "Two"]);
}

#[test]
fn materials_come_from_the_shape_properties() {
    let mut shape = triangle("Tri").with_alpha(ALPHA_FLAGS_BLEND_AND_TEST, 128);
    shape.textures = vec![
        "Textures/Synthetic\\Wall.DDS".to_owned(),
        String::new(),
        " textures\\synthetic\\wall_g.dds\0".to_owned(),
        String::new(),
        String::new(),
        String::new(),
    ];
    shape.two_sided = true;
    shape.material = Some(NiMaterialProperty {
        alpha: 0.5,
        emissive: [0.25, 0.5, 1.0],
        emissive_multiple: 2.0,
        ..NiMaterialProperty::default()
    });
    let mesh = flatten(&NodeDesc::root("Root").with_shape(shape));
    assert_eq!(
        mesh.primitives[0].material,
        Material {
            // Lowercase, backslashes, slot order; the unused normal slot
            // keeps its place, the unused trailing slots are dropped.
            textures: vec![
                "textures\\synthetic\\wall.dds".to_owned(),
                String::new(),
                "textures\\synthetic\\wall_g.dds".to_owned(),
            ],
            alpha: AlphaMode::Mask {
                cutoff: 128.0 / 255.0
            },
            two_sided: true,
            shader: Some(ShaderInfo {
                unlit: false,
                shader_type: 1,
                flags: 0x8200_0000,
                flags2: 1,
            }),
            alpha_flags: Some(ALPHA_FLAGS_BLEND_AND_TEST),
            opacity: 0.5,
            emissive: [0.25, 0.5, 1.0],
            emissive_multiple: 2.0,
        }
    );
    assert_eq!(
        mesh.texture_paths(),
        [
            "textures\\synthetic\\wall.dds",
            "textures\\synthetic\\wall_g.dds"
        ]
    );

    // No properties at all: the default material.
    let plain = flatten(&NodeDesc::root("Root").with_shape(triangle("Plain")));
    assert_eq!(plain.primitives[0].material, Material::default());
}

#[test]
fn alpha_mode_follows_the_alpha_property() {
    let mode = |alpha: Option<(u16, u8)>| {
        let mut shape = triangle("Tri");
        shape.alpha = alpha;
        flatten(&NodeDesc::root("Root").with_shape(shape)).primitives[0]
            .material
            .alpha
    };
    assert_eq!(mode(None), AlphaMode::Opaque);
    assert_eq!(mode(Some((0, 200))), AlphaMode::Opaque);
    assert_eq!(mode(Some((ALPHA_FLAGS_BLEND, 200))), AlphaMode::Blend);
    assert_eq!(
        mode(Some((ALPHA_FLAGS_TEST, 0))),
        AlphaMode::Mask { cutoff: 0.0 }
    );
    assert_eq!(
        mode(Some((ALPHA_FLAGS_TEST, 255))),
        AlphaMode::Mask { cutoff: 1.0 }
    );
    // Test wins over blend when both are enabled.
    assert_eq!(
        mode(Some((ALPHA_FLAGS_BLEND_AND_TEST, 51))),
        AlphaMode::Mask { cutoff: 0.2 }
    );
}

#[test]
fn properties_are_inherited_from_ancestor_nodes() {
    let mut builder = NifBuilder::new();
    let root = builder.reserve();
    let inherits = builder.reserve();
    let overrides = builder.reserve();
    let data = builder.push("NiTriShapeData", &unit_triangle_data());
    let node_alpha = builder.push(
        "NiAlphaProperty",
        &NiAlphaProperty {
            flags: ALPHA_FLAGS_BLEND,
            ..NiAlphaProperty::default()
        }
        .into(),
    );
    let shape_alpha = builder.push(
        "NiAlphaProperty",
        &NiAlphaProperty {
            flags: ALPHA_FLAGS_TEST,
            threshold: 255,
            ..NiAlphaProperty::default()
        }
        .into(),
    );
    let unlit = builder.push(
        "BSShaderNoLightingProperty",
        &BsShaderNoLighting {
            file_name: "Textures\\Synthetic\\Sign.dds".to_owned(),
            extra: vec![0; 16],
            ..BsShaderNoLighting::default()
        }
        .into(),
    );
    builder.set(
        root,
        "BSFadeNode",
        &NiNode {
            av: AvObject {
                properties: vec![Some(node_alpha), Some(unlit)],
                ..named("Root")
            },
            children: vec![Some(inherits), Some(overrides)],
            ..NiNode::default()
        }
        .into(),
    );
    builder.set(inherits, "NiTriShape", &shape_block("Inherits", data));
    builder.set(
        overrides,
        "NiTriShape",
        &Block::tri_shape(NiGeometry {
            av: AvObject {
                properties: vec![Some(shape_alpha)],
                ..named("Overrides")
            },
            data: Some(data),
            ..NiGeometry::default()
        }),
    );
    builder.add_root(Some(root));

    let file = parse(&builder.build()).unwrap();
    let mesh = extract_mesh(&file).unwrap();
    assert_eq!(mesh.primitives.len(), 2, "shared geometry data serves both");
    let inherited = &mesh.primitives[0].material;
    assert_eq!(inherited.alpha, AlphaMode::Blend);
    assert_eq!(inherited.textures, ["textures\\synthetic\\sign.dds"]);
    assert!(inherited.shader.unwrap().unlit);
    let overridden = &mesh.primitives[1].material;
    assert_eq!(overridden.alpha, AlphaMode::Mask { cutoff: 1.0 });
    assert_eq!(overridden.textures, inherited.textures);
    assert_eq!(file.texture_paths(), ["textures\\synthetic\\sign.dds"]);
}

#[test]
fn texture_paths_lists_every_reference_once() {
    let scene = NodeDesc::root("Root")
        .with_shape(quad_shape("A", [1.0, 1.0], "Textures\\Synthetic\\B.dds"))
        .with_shape(quad_shape("B", [1.0, 1.0], "textures/synthetic/a.dds"))
        .with_shape(quad_shape("C", [1.0, 1.0], "textures\\synthetic\\b.DDS"));
    let bytes = scene.to_nif();
    assert_eq!(
        nif::texture_paths(&bytes).unwrap(),
        [
            "textures\\synthetic\\a.dds",
            "textures\\synthetic\\a_n.dds",
            "textures\\synthetic\\b.dds",
            "textures\\synthetic\\b_n.dds",
        ]
    );
    assert_eq!(normalize_texture_path("A/B\\C.DDS"), "a\\b\\c.dds");
}

#[test]
fn collision_is_reported_as_metadata() {
    let scene = NodeDesc::root("Root")
        .with_collision(CollisionDesc {
            flags: 0x81,
            layer: 1,
            motion_system: 7,
        })
        .with_node(NodeDesc::new("NiNode", "Part").with_collision(CollisionDesc::STATIC))
        .with_shape(triangle("Tri"));
    assert_eq!(
        flatten(&scene).collision,
        Some(CollisionInfo {
            kind: "bhkCollisionObject".to_owned(),
            target: "Root".to_owned(),
            flags: 0x81,
            body: "bhkRigidBody".to_owned(),
            layer: Some(1),
            motion_system: Some(7),
            count: 2,
        })
    );
    assert_eq!(
        flatten(&NodeDesc::root("Root").with_shape(triangle("Tri"))).collision,
        None
    );
}

#[test]
fn primitives_are_in_file_order_whatever_the_child_order() {
    let mut builder = NifBuilder::new();
    let root = builder.reserve();
    let data = builder.push("NiTriShapeData", &unit_triangle_data());
    let first = builder.push("NiTriShape", &shape_block("FirstInFile", data));
    let second = builder.push("NiTriShape", &shape_block("SecondInFile", data));
    builder.set(root, "BSFadeNode", &node_block("Root", &[second, first]));
    builder.add_root(Some(root));
    let mesh = extract_mesh(&parse(&builder.build()).unwrap()).unwrap();
    let names: Vec<&str> = mesh.primitives.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["FirstInFile", "SecondInFile"]);
}

#[test]
fn roots_may_be_absent_or_a_bare_shape() {
    let mut builder = NifBuilder::new();
    let data = builder.push("NiTriShapeData", &unit_triangle_data());
    let shape = builder.push("NiTriShape", &shape_block("Lone", data));
    let file = parse(&builder.build()).unwrap();
    assert!(extract_mesh(&file).unwrap().primitives.is_empty());

    builder
        .add_root(None)
        .add_root(Some(shape))
        .add_root(Some(data));
    let mesh = extract_mesh(&parse(&builder.build()).unwrap()).unwrap();
    assert_eq!(mesh.primitives.len(), 1);
    assert_eq!(mesh.primitives[0].name, "Lone");
}

#[test]
fn a_cyclic_graph_is_an_error_not_a_hang() {
    // A -> B -> A.
    let mut builder = NifBuilder::new();
    let a = builder.reserve();
    let b = builder.reserve();
    builder.set(a, "BSFadeNode", &node_block("A", &[b]));
    builder.set(b, "NiNode", &node_block("B", &[a]));
    builder.add_root(Some(a));
    let file = parse(&builder.build()).unwrap();
    match extract_mesh(&file).unwrap_err() {
        NifError::Scene {
            index,
            type_name,
            offset,
            reason,
        } => {
            assert_eq!(index, a);
            assert_eq!(type_name, "BSFadeNode");
            assert_eq!(offset, file.blocks[a as usize].offset);
            assert!(reason.contains("cyclic"), "{reason}");
        }
        other => panic!("unexpected error: {other}"),
    }

    // A node that is its own child.
    let mut builder = NifBuilder::new();
    let own = builder.reserve();
    builder.set(own, "NiNode", &node_block("Self", &[own]));
    builder.add_root(Some(own));
    assert!(matches!(
        extract_mesh(&parse(&builder.build()).unwrap()),
        Err(NifError::Scene { .. })
    ));

    // A cycle under a hidden node is never walked.
    let mut builder = NifBuilder::new();
    let top = builder.reserve();
    let hidden = builder.reserve();
    builder.set(top, "NiNode", &node_block("Top", &[hidden]));
    builder.set(
        hidden,
        "NiNode",
        &NiNode {
            av: AvObject {
                flags: FLAG_HIDDEN,
                ..named("Hidden")
            },
            children: vec![Some(top)],
            ..NiNode::default()
        }
        .into(),
    );
    builder.add_root(Some(top));
    assert!(extract_mesh(&parse(&builder.build()).unwrap()).is_ok());
}

#[test]
fn a_graph_that_fans_out_exponentially_is_stopped() {
    // Forty levels, each node listing the next one twice: 2^40 paths.
    let mut builder = NifBuilder::new();
    let levels: Vec<u32> = (0..40).map(|_| builder.reserve()).collect();
    let data = builder.push("NiTriShapeData", &unit_triangle_data());
    let shape = builder.push("NiTriShape", &shape_block("Leaf", data));
    for (i, &level) in levels.iter().enumerate() {
        let next = levels.get(i + 1).copied().unwrap_or(shape);
        builder.set(level, "NiNode", &node_block("Level", &[next, next]));
    }
    builder.add_root(Some(levels[0]));
    let error = extract_mesh(&parse(&builder.build()).unwrap()).unwrap_err();
    assert!(matches!(error, NifError::Scene { .. }), "{error}");

    // Modest sharing is fine: one shape under two parents appears twice.
    let mut builder = NifBuilder::new();
    let root = builder.reserve();
    let left = builder.reserve();
    let right = builder.reserve();
    let data = builder.push("NiTriShapeData", &unit_triangle_data());
    let shape = builder.push("NiTriShape", &shape_block("Shared", data));
    builder.set(root, "NiNode", &node_block("Root", &[left, right]));
    builder.set(left, "NiNode", &node_block("Left", &[shape]));
    builder.set(right, "NiNode", &node_block("Right", &[shape]));
    builder.add_root(Some(root));
    let mesh = extract_mesh(&parse(&builder.build()).unwrap()).unwrap();
    assert_eq!(mesh.primitives.len(), 2);
}

#[test]
fn a_very_deep_chain_does_not_overflow_the_stack() {
    const DEPTH: u32 = 50_000;
    let mut builder = NifBuilder::new();
    for i in 0..DEPTH {
        let mut node = NiNode {
            av: AvObject {
                translation: [1.0, 0.0, 0.0],
                ..AvObject::default()
            },
            children: vec![Some(i + 1)],
            ..NiNode::default()
        };
        if i == 0 {
            node.av.net.name = "Top".to_owned();
        }
        builder.push("NiNode", &node.into());
    }
    let shape = builder.push("NiTriShape", &shape_block("Bottom", DEPTH + 1));
    assert_eq!(shape, DEPTH);
    builder.push("NiTriShapeData", &unit_triangle_data());
    builder.add_root(Some(0));
    let mesh = extract_mesh(&parse(&builder.build()).unwrap()).unwrap();
    assert_eq!(mesh.primitives[0].positions[0], [DEPTH as f32, 0.0, 0.0]);
}

#[test]
fn bad_geometry_is_an_error_that_names_the_data_block() {
    let scene_with = |data: NiTriShapeData| {
        let mut builder = NifBuilder::new();
        let root = builder.reserve();
        let shape = builder.push("NiTriShape", &shape_block("Shape", 2));
        builder.push("NiTriShapeData", &data.into());
        builder.set(root, "BSFadeNode", &node_block("Root", &[shape]));
        builder.add_root(Some(root));
        parse(&builder.build()).unwrap()
    };
    let corner = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

    let file = scene_with(triangle_data(corner.clone(), vec![[0, 1, 3]]));
    match extract_mesh(&file).unwrap_err() {
        NifError::Scene {
            index,
            type_name,
            reason,
            ..
        } => {
            assert_eq!((index, type_name.as_str()), (2, "NiTriShapeData"));
            assert!(reason.contains("triangle index 3"), "{reason}");
        }
        other => panic!("unexpected error: {other}"),
    }

    let mut not_finite = corner.clone();
    not_finite[1][2] = f32::NAN;
    let file = scene_with(triangle_data(not_finite, vec![[0, 1, 2]]));
    match extract_mesh(&file).unwrap_err() {
        NifError::Scene { reason, .. } => assert!(reason.contains("vertex 1"), "{reason}"),
        other => panic!("unexpected error: {other}"),
    }

    // Non-finite normals, UVs and colours are replaced, not fatal.
    let mut data = triangle_data(corner, vec![[0, 1, 2]]);
    data.geom.normals = Some(vec![[f32::NAN, 0.0, 1.0]; 3]);
    data.geom.colors = Some(vec![[1.0, f32::INFINITY, 0.0, 1.0]; 3]);
    data.geom.vector_flags = 1;
    data.geom.uv_sets = vec![vec![[f32::NEG_INFINITY, 0.5]; 3]];
    let mesh = extract_mesh(&scene_with(data)).unwrap();
    let primitive = &mesh.primitives[0];
    // A normal with any non-finite component is no normal at all.
    assert_eq!(primitive.normals.as_ref().unwrap()[0], [0.0, 0.0, 0.0]);
    assert_eq!(primitive.colors.as_ref().unwrap()[0], [1.0, 1.0, 0.0, 1.0]);
    assert_eq!(primitive.uvs.as_ref().unwrap()[0], [0.0, 0.5]);
}

#[test]
fn shapes_without_usable_data_give_no_primitive() {
    let mut builder = NifBuilder::new();
    let root = builder.reserve();
    let no_data = builder.push(
        "NiTriShape",
        &Block::tri_shape(NiGeometry {
            av: named("NoData"),
            ..NiGeometry::default()
        }),
    );
    let unknown = builder.push_raw("NiAdditionalGeometryData", vec![0; 12]);
    let wrong_data = builder.push("NiTriShape", &shape_block("WrongData", unknown));
    let mut no_vertices = triangle_data(Vec::new(), Vec::new());
    no_vertices.geom.vertices = None;
    let empty = builder.push("NiTriShapeData", &no_vertices.into());
    let no_vertex_shape = builder.push("NiTriShape", &shape_block("NoVertices", empty));
    builder.set(
        root,
        "BSFadeNode",
        &node_block("Root", &[no_data, wrong_data, no_vertex_shape, unknown]),
    );
    builder.add_root(Some(root));
    let mesh = extract_mesh(&parse(&builder.build()).unwrap()).unwrap();
    assert!(mesh.primitives.is_empty());
    assert_eq!(mesh.skipped_blocks["NiAdditionalGeometryData"], 1);
}

#[test]
fn child_descriptions_cover_unknown_blocks() {
    // The `ChildDesc` enum is public so that later crates can build scenes
    // directly.
    let mut root = NodeDesc::root("Root");
    root.children.push(ChildDesc::Unknown {
        type_name: "NiCamera".to_owned(),
        payload: vec![0; 9],
    });
    root.children.push(ChildDesc::Shape(triangle("Tri")));
    let mesh = flatten(&root);
    assert_eq!(mesh.primitives.len(), 1);
    assert_eq!(mesh.skipped_blocks["NiCamera"], 1);
}

/// The rotation convention, settled on real meshes (see
/// `examples/calibrate.rs`): the nine floats are stored row by row, and a
/// row dotted with a column vector gives one component of the result. This
/// test pins it at the byte level, without going through the builder's own
/// idea of a matrix.
#[test]
fn the_rotation_convention_is_pinned_at_the_byte_level() {
    let scene =
        NodeDesc::root("Root").with_node(NodeDesc::new("NiNode", "Turned").with_shape(
            triangle("Tri").with_transform(Transform::from_translation([10.0, 0.0, 0.0])),
        ));
    let mut bytes = scene.to_nif();
    let file = parse(&bytes).unwrap();
    let node = file
        .blocks
        .iter()
        .find(|e| matches!(&e.block, Block::Node(n) if n.av.net.name == "Turned"))
        .unwrap();
    // Name, extra data count, controller, flags and translation come first.
    let rotation_at = node.offset + 4 + 4 + 4 + 4 + 12;
    // In file order: only the second and fourth floats are non-zero, plus
    // the last. The second float, -1, is row 0, column 1.
    let stored: [f32; 9] = [0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    for (i, value) in stored.iter().enumerate() {
        bytes[rotation_at + i * 4..rotation_at + i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }

    let file = parse(&bytes).unwrap();
    let Some(Block::Node(turned)) = file
        .blocks
        .iter()
        .map(|e| &e.block)
        .find(|b| matches!(b, Block::Node(n) if n.av.net.name == "Turned"))
    else {
        panic!("the node is gone");
    };
    assert_eq!(turned.av.rotation[0], [0.0, -1.0, 0.0]);

    // x' = 0·x - 1·y, y' = 1·x: a quarter turn that takes +X to +Y. The
    // child's translation (10, 0, 0) is turned by its parent to (0, 10, 0),
    // so the parent applies after the child. The transposed reading would
    // give (1, -10, 0), (0, -11, 0) and (0, -10, 1) instead.
    let mesh = extract_mesh(&file).unwrap();
    assert_eq!(
        mesh.primitives[0].positions,
        [[0.0, 11.0, 0.0], [-1.0, 10.0, 0.0], [0.0, 10.0, 1.0]]
    );
}

#[test]
fn segmented_shapes_are_extracted_like_triangle_shapes() {
    let mut builder = NifBuilder::new();
    let root = builder.reserve();
    let data = builder.push("NiTriShapeData", &unit_triangle_data());
    let shape = builder.push(
        "BSSegmentedTriShape",
        &BsSegmentedTriShape {
            shape: NiGeometry {
                av: AvObject {
                    translation: [0.0, 0.0, 5.0],
                    ..named("Land")
                },
                data: Some(data),
                active_material: -1,
                ..NiGeometry::default()
            },
            segments: vec![TriSegment {
                flags: 0,
                start: 0,
                triangles: 1,
            }],
        }
        .into(),
    );
    builder.set(root, "BSFadeNode", &node_block("Root", &[shape]));
    builder.add_root(Some(root));
    let file = parse(&builder.build()).unwrap();
    let mesh = extract_mesh(&file).unwrap();
    assert!(mesh.skipped_blocks.is_empty());
    assert_eq!(mesh.primitives.len(), 1);
    assert_eq!(mesh.primitives[0].name, "Land");
    assert_eq!(mesh.primitives[0].positions[1], [1.0, 0.0, 5.0]);

    // The segment list is part of the size check.
    builder
        .payload_mut(shape)
        .unwrap()
        .extend_from_slice(&[0; 9]);
    assert!(matches!(
        parse(&builder.build()),
        Err(NifError::BlockSizeMismatch { .. })
    ));
}

/// A file with one shape whose properties are the given blocks; block 0 is
/// the root, block 1 the shape, block 2 the data, the properties follow.
fn shape_with_properties(properties: &[(&str, Block)]) -> nif::NifFile {
    let mut builder = NifBuilder::new();
    let root = builder.reserve();
    let shape = builder.reserve();
    let data = builder.push("NiTriShapeData", &unit_triangle_data());
    let mut refs = Vec::new();
    for (type_name, block) in properties {
        refs.push(Some(builder.push(type_name, block)));
    }
    // Only the first `attached` properties hang on the shape; a texture set
    // or a source texture is reached through the property that names it.
    let attached: Vec<_> = refs
        .iter()
        .zip(properties)
        .filter(|(_, (name, _))| *name != "BSShaderTextureSet" && *name != "NiSourceTexture")
        .map(|(r, _)| *r)
        .collect();
    builder.set(root, "BSFadeNode", &node_block("Root", &[shape]));
    builder.set(
        shape,
        "NiTriShape",
        &Block::tri_shape(NiGeometry {
            av: AvObject {
                properties: attached,
                ..named("Shape")
            },
            data: Some(data),
            active_material: -1,
            ..NiGeometry::default()
        }),
    );
    builder.add_root(Some(root));
    parse(&builder.build()).unwrap()
}

fn texturing(base_source: Option<u32>) -> Block {
    NiTexturingProperty {
        flags: 4,
        texture_count: 9,
        has_base_texture: base_source.is_some(),
        base_source,
        extra: vec![0; 15],
        ..NiTexturingProperty::default()
    }
    .into()
}

fn source_texture(file_name: &str) -> Block {
    NiSourceTexture {
        use_external: 1,
        file_name: file_name.to_owned(),
        ..NiSourceTexture::default()
    }
    .into()
}

#[test]
fn a_texturing_property_supplies_the_diffuse_texture_when_no_shader_does() {
    let textures = |file: &nif::NifFile| {
        extract_mesh(file).unwrap().primitives[0]
            .material
            .textures
            .clone()
    };

    // Old-style material: a texturing property and its source texture.
    let old_style = shape_with_properties(&[
        ("NiTexturingProperty", texturing(Some(4))),
        (
            "NiSourceTexture",
            source_texture("Textures/Synthetic\\Old.DDS"),
        ),
    ]);
    assert_eq!(textures(&old_style), ["textures\\synthetic\\old.dds"]);
    assert!(
        extract_mesh(&old_style).unwrap().primitives[0]
            .material
            .shader
            .is_none()
    );
    assert_eq!(old_style.texture_paths(), ["textures\\synthetic\\old.dds"]);
    assert!(old_style.skipped_blocks().is_empty());

    // An unlit shader with an empty file name falls back to it as well.
    let unlit_empty = shape_with_properties(&[
        (
            "BSShaderNoLightingProperty",
            BsShaderNoLighting {
                extra: vec![0; 16],
                ..BsShaderNoLighting::default()
            }
            .into(),
        ),
        ("NiTexturingProperty", texturing(Some(5))),
        (
            "NiSourceTexture",
            source_texture("textures\\synthetic\\glow.dds"),
        ),
    ]);
    assert_eq!(textures(&unlit_empty), ["textures\\synthetic\\glow.dds"]);

    // An unlit shader's own file name is the diffuse slot and wins.
    let unlit_named = shape_with_properties(&[
        (
            "BSShaderNoLightingProperty",
            BsShaderNoLighting {
                file_name: "Textures\\Synthetic\\Sign.dds".to_owned(),
                extra: vec![0; 16],
                ..BsShaderNoLighting::default()
            }
            .into(),
        ),
        ("NiTexturingProperty", texturing(Some(5))),
        (
            "NiSourceTexture",
            source_texture("textures\\synthetic\\glow.dds"),
        ),
    ]);
    assert_eq!(textures(&unlit_named), ["textures\\synthetic\\sign.dds"]);

    // A shader texture set wins over the texturing property.
    let both = shape_with_properties(&[
        (
            "BSShaderPPLightingProperty",
            BsShaderPpLighting {
                texture_set: Some(4),
                ..BsShaderPpLighting::default()
            }
            .into(),
        ),
        (
            "BSShaderTextureSet",
            BsShaderTextureSet {
                textures: vec!["textures\\synthetic\\new.dds".to_owned(), String::new()],
            }
            .into(),
        ),
        ("NiTexturingProperty", texturing(Some(6))),
        (
            "NiSourceTexture",
            source_texture("textures\\synthetic\\old.dds"),
        ),
    ]);
    assert_eq!(textures(&both), ["textures\\synthetic\\new.dds"]);

    // No base slot, or a source without a name: no texture.
    let no_base = shape_with_properties(&[("NiTexturingProperty", texturing(None))]);
    assert!(textures(&no_base).is_empty());
    let unnamed = shape_with_properties(&[
        ("NiTexturingProperty", texturing(Some(4))),
        ("NiSourceTexture", source_texture("")),
    ]);
    assert!(textures(&unnamed).is_empty());
}

#[test]
fn skinned_shapes_are_emitted_in_their_bind_pose_and_flagged() {
    let mesh = extract_mesh(&parse(&nif::fixture::skinned_body()).unwrap()).unwrap();
    assert_eq!(mesh.primitives.len(), 1);
    let body = &mesh.primitives[0];
    // Stored vertices, placed by the shape's own transform; no bone applied.
    assert!(body.skinned && body.skin.is_some());
    let offset = nif::fixture::SKINNED_SHAPE_OFFSET;
    let expected: Vec<[f32; 3]> = nif::fixture::BODY_VERTICES
        .iter()
        .map(|v| [v[0] + offset[0], v[1] + offset[1], v[2] + offset[2]])
        .collect();
    assert_eq!(body.positions, expected);
    assert_eq!(nif::glb::read(&nif::glb::write(&mesh)).unwrap(), mesh);

    // A shape whose skin instance is not a skin block is an error, not a
    // silently unskinned shape.
    let mut builder = NifBuilder::new();
    let root = builder.reserve();
    let data = builder.push("NiTriShapeData", &unit_triangle_data());
    let not_a_skin = builder.push_raw("NiUnknownSkin", vec![0; 16]);
    let skinned = builder.push(
        "NiTriShape",
        &Block::tri_shape(NiGeometry {
            av: named("Skinned"),
            data: Some(data),
            skin_instance: Some(not_a_skin),
            active_material: -1,
            ..NiGeometry::default()
        }),
    );
    builder.set(root, "BSFadeNode", &node_block("Root", &[skinned]));
    builder.add_root(Some(root));
    let error = extract_mesh(&parse(&builder.build()).unwrap()).unwrap_err();
    assert!(matches!(error, NifError::Scene { .. }), "{error}");
}
