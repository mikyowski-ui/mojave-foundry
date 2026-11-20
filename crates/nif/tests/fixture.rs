//! The synthetic mesh helpers later crates build their test scenes from.

use nif::fixture::{
    CollisionDesc, NifBuilder, NodeDesc, box_mesh, box_shape, building, building_scene, quad,
    quad_shape, texture_slots,
};
use nif::{Block, Primitive, StaticMesh, Transform, extract_mesh, glb, parse};

fn flatten(bytes: &[u8]) -> StaticMesh {
    extract_mesh(&parse(bytes).unwrap()).unwrap()
}

fn extent(primitive: &Primitive) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in &primitive.positions {
        for (axis, &v) in p.iter().enumerate() {
            min[axis] = min[axis].min(v);
            max[axis] = max[axis].max(v);
        }
    }
    (min, max)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(u: [f32; 3], v: [f32; 3]) -> [f32; 3] {
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

/// For every triangle: the winding agrees with the stored normals, and the
/// face looks away from `inside`.
fn assert_faces_outward(primitive: &Primitive, inside: [f32; 3]) {
    let normals = primitive.normals.as_ref().expect("normals");
    for t in primitive.indices.chunks(3) {
        let [a, b, c] = [
            primitive.positions[t[0] as usize],
            primitive.positions[t[1] as usize],
            primitive.positions[t[2] as usize],
        ];
        let face = cross(sub(b, a), sub(c, a));
        assert!(dot(face, face) > 0.0, "triangle {t:?} has no area");
        for &i in t {
            let normal = normals[i as usize];
            assert!((dot(normal, normal) - 1.0).abs() < 1e-5, "unit normal");
            assert!(
                dot(face, normal) > 0.0,
                "winding of {t:?} against its normal"
            );
        }
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        assert!(dot(face, sub(centroid, inside)) > 0.0, "{t:?} faces inward");
    }
}

#[test]
fn box_mesh_is_a_closed_textured_box_on_the_ground() {
    let bytes = box_mesh([200.0, 100.0, 50.0], "textures\\synthetic\\crate.dds");
    let file = parse(&bytes).unwrap();
    assert_eq!(file.header.block_types[0], "BSFadeNode");
    let mesh = extract_mesh(&file).unwrap();

    assert_eq!(mesh.primitives.len(), 1);
    let primitive = &mesh.primitives[0];
    assert_eq!(primitive.name, "Box:0");
    assert_eq!(primitive.positions.len(), 24);
    assert_eq!(primitive.indices.len(), 36);
    assert_eq!(
        extent(primitive),
        ([-100.0, -50.0, 0.0], [100.0, 50.0, 50.0])
    );
    assert_faces_outward(primitive, [0.0, 0.0, 25.0]);
    let uvs = primitive.uvs.as_ref().unwrap();
    assert!(uvs.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
    assert!(primitive.colors.is_none());
    assert_eq!(
        primitive.material.textures,
        [
            "textures\\synthetic\\crate.dds",
            "textures\\synthetic\\crate_n.dds"
        ]
    );

    let collision = mesh.collision.as_ref().unwrap();
    assert_eq!(collision.target, "Box");
    assert_eq!(
        (collision.layer, collision.motion_system),
        (Some(1), Some(7))
    );
    assert_eq!(mesh.skipped_blocks.len(), 2);
    assert_eq!(mesh.skipped_blocks["bhkRigidBody"], 1);
    assert_eq!(mesh.skipped_blocks["bhkBoxShape"], 1);

    // The file stores tangents, as real static meshes do; the parser reads
    // past them.
    let data = file
        .blocks
        .iter()
        .find_map(|e| match &e.block {
            Block::TriShapeData(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(data.geom.vector_flags, 0x1001);
    assert_eq!(data.geom.tangents.as_ref().map(Vec::len), Some(24));
    assert_eq!(data.geom.bitangents.as_ref().map(Vec::len), Some(24));
}

#[test]
fn quad_is_one_upward_rectangle() {
    let mesh = flatten(&quad([128.0, 64.0], "textures\\synthetic\\ground.dds"));
    assert_eq!(mesh.primitives.len(), 1);
    let primitive = &mesh.primitives[0];
    assert_eq!(primitive.name, "Quad:0");
    assert_eq!(primitive.positions.len(), 4);
    assert_eq!(primitive.indices, [0, 1, 2, 0, 2, 3]);
    assert_eq!(extent(primitive), ([-64.0, -32.0, 0.0], [64.0, 32.0, 0.0]));
    assert!(
        primitive
            .normals
            .as_ref()
            .unwrap()
            .iter()
            .all(|n| *n == [0.0, 0.0, 1.0])
    );
    assert_eq!(
        primitive.uvs.as_deref().unwrap(),
        [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]
    );
    assert!(mesh.collision.is_none());
    assert!(mesh.skipped_blocks.is_empty());

    // Without a texture there is no shader property at all.
    let bare = flatten(&quad([1.0, 1.0], ""));
    assert!(bare.primitives[0].material.textures.is_empty());
    assert!(bare.primitives[0].material.shader.is_none());
}

#[test]
fn building_has_walls_and_a_roof_as_separate_shapes() {
    let (width, depth, height) = (800.0, 600.0, 300.0);
    let bytes = building(
        [width, depth, height],
        "textures\\synthetic\\adobe.dds",
        "textures\\synthetic\\tin.dds",
    );
    let file = parse(&bytes).unwrap();
    assert_eq!(file.bsx_flags(), Some(2));
    // A triangle shape and a strip shape, under two kinds of node.
    for type_name in [
        "BSFadeNode",
        "NiNode",
        "NiTriShape",
        "NiTriShapeData",
        "NiTriStrips",
        "NiTriStripsData",
        "NiStencilProperty",
        "bhkCollisionObject",
    ] {
        assert!(
            file.header.block_types.iter().any(|t| t == type_name),
            "no {type_name} block"
        );
    }

    let mesh = extract_mesh(&file).unwrap();
    let names: Vec<&str> = mesh.primitives.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Walls", "Roof"]);
    let ridge = depth / 4.0;

    let walls = &mesh.primitives[0];
    assert_eq!(walls.positions.len(), 4 * 4 + 2 * 3);
    assert_eq!(walls.indices.len(), (4 * 2 + 2) * 3);
    assert_eq!(
        extent(walls),
        (
            [-width / 2.0, -depth / 2.0, 0.0],
            [width / 2.0, depth / 2.0, height + ridge]
        )
    );
    assert_faces_outward(walls, [0.0, 0.0, height / 2.0]);
    assert_eq!(walls.material.textures[0], "textures\\synthetic\\adobe.dds");
    assert!(!walls.material.two_sided);

    // The roof's node lifts it to the top of the walls.
    let roof = &mesh.primitives[1];
    assert_eq!(roof.positions.len(), 8);
    assert_eq!(roof.indices.len(), 4 * 3);
    assert_eq!(
        extent(roof),
        (
            [-width / 2.0, -depth / 2.0, height],
            [width / 2.0, depth / 2.0, height + ridge]
        )
    );
    assert_faces_outward(roof, [0.0, 0.0, height / 2.0]);
    assert_eq!(roof.material.textures[0], "textures\\synthetic\\tin.dds");
    assert!(roof.material.two_sided);

    assert_eq!(mesh.collision.as_ref().unwrap().target, "Building");
    assert_eq!(
        file.texture_paths(),
        [
            "textures\\synthetic\\adobe.dds",
            "textures\\synthetic\\adobe_n.dds",
            "textures\\synthetic\\tin.dds",
            "textures\\synthetic\\tin_n.dds",
        ]
    );
    assert_eq!(glb::read(&glb::write(&mesh)).unwrap(), mesh);
}

#[test]
fn strips_and_triangles_describe_the_same_box() {
    let triangles = box_shape("Box", [4.0, 6.0, 8.0], "textures\\a.dds");
    let strips = triangles.clone().into_strips();
    let as_triangles = flatten(&NodeDesc::root("Root").with_shape(triangles).to_nif());
    let as_strips = flatten(&NodeDesc::root("Root").with_shape(strips).to_nif());
    let (a, b) = (&as_triangles.primitives[0], &as_strips.primitives[0]);
    assert_eq!(a.positions, b.positions);
    assert_eq!(a.normals, b.normals);
    assert_eq!(a.indices.len(), b.indices.len());
    assert_faces_outward(a, [0.0, 0.0, 4.0]);
    assert_faces_outward(b, [0.0, 0.0, 4.0]);
}

#[test]
fn scenes_compose_into_a_small_town() {
    // Three placed buildings and a ground quad under one root, the way a
    // later crate would assemble a test town.
    let mut town = NodeDesc::root("Town").with_shape(quad_shape(
        "Ground",
        [4096.0, 4096.0],
        "textures\\synthetic\\dirt.dds",
    ));
    let lots = [[-1000.0, 500.0], [0.0, 0.0], [1200.0, -800.0]];
    for (i, lot) in lots.iter().enumerate() {
        let mut house = building_scene(
            [400.0, 300.0, 250.0],
            "textures\\synthetic\\adobe.dds",
            "textures\\synthetic\\tin.dds",
        );
        house.type_name = "NiNode".to_owned();
        house.name = format!("House{i}");
        house.transform = Transform {
            rotation: Transform::rotation_z(i as f32 * std::f32::consts::FRAC_PI_2),
            translation: [lot[0], lot[1], 0.0],
            scale: 1.0,
        };
        town = town.with_node(house);
    }
    let mut builder = NifBuilder::new();
    let root = builder.add_scene(&town);
    assert_eq!(root, 0);
    let mesh = flatten(&builder.build());

    assert_eq!(mesh.primitives.len(), 1 + 3 * 2);
    assert_eq!(mesh.collision.as_ref().unwrap().count, 3);
    for (i, lot) in lots.iter().enumerate() {
        let walls = &mesh.primitives[1 + i * 2];
        assert_eq!(walls.name, "Walls");
        let (min, max) = extent(walls);
        let centre = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
        assert!((centre[0] - lot[0]).abs() < 0.01 && (centre[1] - lot[1]).abs() < 0.01);
        assert_faces_outward(walls, [lot[0], lot[1], 125.0]);
    }
    assert_eq!(
        mesh.texture_paths(),
        [
            "textures\\synthetic\\adobe.dds",
            "textures\\synthetic\\adobe_n.dds",
            "textures\\synthetic\\dirt.dds",
            "textures\\synthetic\\dirt_n.dds",
            "textures\\synthetic\\tin.dds",
            "textures\\synthetic\\tin_n.dds",
        ]
    );
}

#[test]
fn texture_slots_fill_the_six_slot_set() {
    assert_eq!(
        texture_slots("textures\\a\\b.dds"),
        ["textures\\a\\b.dds", "textures\\a\\b_n.dds", "", "", "", ""]
    );
    assert_eq!(texture_slots("plain")[1], "plain_n");
    assert!(texture_slots("").is_empty());
}

#[test]
fn collision_descriptions_reach_the_metadata() {
    let scene = NodeDesc::root("Root")
        .with_collision(CollisionDesc {
            flags: 7,
            layer: 3,
            motion_system: 4,
        })
        .with_shape(quad_shape("Q", [1.0, 1.0], ""));
    let collision = flatten(&scene.to_nif()).collision.unwrap();
    assert_eq!(
        (collision.flags, collision.layer, collision.motion_system),
        (7, Some(3), Some(4))
    );
}
