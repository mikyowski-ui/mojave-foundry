//! Skeletons, skins, animations, posing and skinning, on the synthetic
//! character fixtures.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use nif::fixture::{
    BODY_VERTICES, KfDesc, KfTrack, NifBuilder, SKIN_BONES, SKINNED_SHAPE_OFFSET, WALK_DURATION,
    WALK_STRIDE, body_weights, compress_control_points, linear_rotations, linear_translations,
    skeleton_bones, skeleton_nif, skeleton_world, skinned_body, skinned_body_with, unset_transform,
    walk_desc, walk_kf,
};
use nif::math::{apply_point, apply_vector, quat_axis_angle, quat_to_rotation, slerp};
use nif::{
    Animation, AvObject, BSplineCompression, Block, CYCLE_CLAMP, KEY_CONSTANT, KEY_QUADRATIC,
    KEY_TBC, KEY_XYZ_ROTATION, Key, KeyGroup, NO_HANDLE, NiBSplineData,
    NiBSplineTransformInterpolator, NiNode, NiTransformData, NifError, ObjectNet, Pose,
    SAMPLE_RATE, Skeleton, StaticMesh, Track, euler_xyz_to_quat, extract_animation_with_report,
    extract_mesh, extract_skeleton, parse, parse_animation, pose, skin,
};

fn close(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

fn close3(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
    (0..3).all(|k| close(a[k], b[k], tolerance))
}

/// The angle in degrees between two rotations.
fn angle(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    2.0 * dot.abs().min(1.0).acos().to_degrees()
}

fn skeleton() -> Skeleton {
    extract_skeleton(&parse(&skeleton_nif()).unwrap()).unwrap()
}

fn body() -> StaticMesh {
    extract_mesh(&parse(&skinned_body()).unwrap()).unwrap()
}

fn mesh_space(v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| v[k] + SKINNED_SHAPE_OFFSET[k])
}

fn animation_with(tracks: Vec<Track>) -> Animation {
    Animation {
        name: "Test".to_owned(),
        duration: 1.0,
        looping: true,
        tracks,
        text_keys: Vec::new(),
        accum_root: String::new(),
    }
}

// ---------------------------------------------------------------- skeleton

#[test]
fn the_skeleton_lists_every_node_parents_first() {
    let skeleton = skeleton();
    let expected = skeleton_bones();
    assert_eq!(skeleton.bones.len(), expected.len());
    for (bone, (name, parent, local)) in skeleton.bones.iter().zip(&expected) {
        assert_eq!(bone.name, *name);
        assert_eq!(bone.parent, parent.map(|p| p as u32));
        assert_eq!(bone.local, (*local).into());
    }
    assert!(skeleton.is_well_formed());
    assert_eq!(skeleton.depth(), 5);
    assert_eq!(skeleton.find("Bip01 Spine"), Some(3));
    assert_eq!(
        skeleton.find("bip01 spine"),
        Some(3),
        "case is ignored second"
    );
    assert_eq!(skeleton.find("Bip01 Tail"), None);

    // World matrices compose child first, parent second.
    let world = skeleton.world_matrices();
    for (m, t) in world.iter().zip(skeleton_world()) {
        assert!(close3(
            apply_point(m, [1.0, 2.0, 3.0]),
            t.apply([1.0, 2.0, 3.0]),
            1e-5
        ));
    }
}

#[test]
fn a_mesh_file_has_the_same_bone_tree() {
    // Real body parts carry copies of their bones; the tree comes out the
    // same, shapes left out.
    let from_mesh = extract_skeleton(&parse(&skinned_body()).unwrap()).unwrap();
    assert_eq!(from_mesh, skeleton());
}

#[test]
fn a_cyclic_node_graph_is_an_error_and_a_shared_node_appears_once() {
    let node = |name: &str, children: Vec<Option<u32>>| -> Block {
        NiNode {
            av: AvObject {
                net: ObjectNet {
                    name: name.to_owned(),
                    ..ObjectNet::default()
                },
                ..AvObject::default()
            },
            children,
            ..NiNode::default()
        }
        .into()
    };
    let mut builder = NifBuilder::new();
    builder.push("NiNode", &node("A", vec![Some(1)]));
    builder.push("NiNode", &node("B", vec![Some(0)]));
    builder.add_root(Some(0));
    let error = extract_skeleton(&parse(&builder.build()).unwrap()).unwrap_err();
    assert!(matches!(error, NifError::Scene { index: 0, .. }), "{error}");

    let mut builder = NifBuilder::new();
    builder.push("NiNode", &node("Root", vec![Some(1), Some(2)]));
    builder.push("NiNode", &node("Left", vec![Some(3)]));
    builder.push("NiNode", &node("Right", vec![Some(3), None]));
    builder.push("NiNode", &node("Shared", vec![]));
    builder.add_root(Some(0));
    let skeleton = extract_skeleton(&parse(&builder.build()).unwrap()).unwrap();
    let names: Vec<&str> = skeleton.bones.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["Root", "Left", "Shared", "Right"]);
    assert_eq!(skeleton.bones[2].parent, Some(1));
}

// ---------------------------------------------------------------- skin

#[test]
fn the_skin_is_parsed_with_weights_and_bind_transforms() {
    let mesh = body();
    assert_eq!(mesh.primitives.len(), 1);
    let body = &mesh.primitives[0];
    assert!(body.skinned);
    let skin = body.skin.as_ref().unwrap();
    let names: Vec<&str> = skin.bones.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, SKIN_BONES);
    assert!(skin.is_well_formed(body.positions.len()));
    assert!(body.is_well_formed());

    // Rows: heaviest first, unused slots (0, 0).
    assert_eq!(skin.weights[0], [(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]);
    assert_eq!(skin.weights[2], [(0, 0.5), (1, 0.5), (0, 0.0), (0, 0.0)]);
    assert_eq!(skin.weights[6], [(2, 0.75), (0, 0.25), (0, 0.0), (0, 0.0)]);
    for (row, expected) in skin.weights.iter().zip(body_weights()) {
        let sum: f32 = row.iter().map(|w| w.1).sum();
        assert!(close(sum, 1.0, 1e-6));
        assert_eq!(row.iter().filter(|w| w.1 > 0.0).count(), expected.len());
    }

    // The bind inverse maps a mesh-space point to the bone's own space:
    // each bone's origin goes to zero.
    let world = skeleton_world();
    let all: Vec<&str> = skeleton_bones().iter().map(|b| b.0).collect();
    for bone in &skin.bones {
        let origin = world[all.iter().position(|n| *n == bone.name).unwrap()].translation;
        assert!(close3(
            apply_point(&bone.bind_inverse, origin),
            [0.0; 3],
            1e-5
        ));
    }
    // The arm bone turns parent +Z into its own +Y.
    let arm = &skin.bones[2].bind_inverse;
    assert!(close3(
        apply_vector(arm, [0.0, 0.0, 1.0]),
        [0.0, 1.0, 0.0],
        1e-6
    ));
}

#[test]
fn partition_weights_are_used_when_the_skin_data_has_none() {
    let from_data = body();
    let from_partition =
        extract_mesh(&parse(&skinned_body_with(&body_weights(), true)).unwrap()).unwrap();
    assert_eq!(
        from_partition.primitives[0].skin,
        from_data.primitives[0].skin
    );
}

#[test]
fn weights_are_merged_sorted_cut_to_four_and_normalised() {
    let mut weights = body_weights();
    // Unnormalised, a repeated bone, and five influences.
    weights[0] = vec![(0, 2.0), (1, 2.0), (0, 4.0)];
    weights[1] = vec![(0, 0.1), (1, 0.2), (2, 0.3), (0, 0.15), (1, 0.05)];
    weights[4] = vec![(1, 0.0)];
    let mesh = extract_mesh(&parse(&skinned_body_with(&weights, false)).unwrap()).unwrap();
    let skin = mesh.primitives[0].skin.as_ref().unwrap();
    assert_eq!(skin.weights[0][0].0, 0);
    assert!(close(skin.weights[0][0].1, 0.75, 1e-6));
    assert!(close(skin.weights[0][1].1, 0.25, 1e-6));
    // Bone 0: 0.25, bone 2: 0.3, bone 1: 0.25 → bone 2 first, then 0 and 1
    // (equal weights, lower index first).
    let row = skin.weights[1];
    assert_eq!([row[0].0, row[1].0, row[2].0], [2, 0, 1]);
    assert!(close(row.iter().map(|w| w.1).sum(), 1.0, 1e-6));
    // A vertex with no usable weight is bound to bone 0 alone.
    assert_eq!(skin.weights[4], [(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]);
}

#[test]
fn a_weight_past_the_vertex_count_is_an_error() {
    let mut weights = body_weights();
    weights.extend(vec![vec![(0, 1.0)]; 3]);
    let error = extract_mesh(&parse(&skinned_body_with(&weights, false)).unwrap()).unwrap_err();
    assert!(matches!(error, NifError::Scene { .. }), "{error}");
}

#[test]
fn truncated_or_corrupted_character_files_never_panic() {
    for bytes in [skinned_body(), skeleton_nif(), walk_kf()] {
        for len in 0..bytes.len() {
            let cut = &bytes[..len];
            if let Ok(file) = parse(cut) {
                let _ = extract_mesh(&file);
                let _ = extract_skeleton(&file);
            }
            let _ = parse_animation(cut);
        }
        // Flip every byte in turn.
        for i in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[i] ^= 0xA5;
            if let Ok(file) = parse(&bad) {
                let _ = extract_mesh(&file).map(|m| nif::glb::write(&m));
                let _ = extract_skeleton(&file);
            }
            let _ = parse_animation(&bad);
        }
    }
}

// ---------------------------------------------------------------- skinning

#[test]
fn skinning_with_the_rest_pose_gives_the_stored_vertices_back() {
    let mesh = body();
    let skeleton = skeleton();
    let posed = skin(&mesh, &skeleton, &Pose::rest(&skeleton));
    let p = &posed.primitives[0];
    assert!(!p.skinned && p.skin.is_none());
    for (a, b) in p.positions.iter().zip(&mesh.primitives[0].positions) {
        assert!(close3(*a, *b, 1e-4), "{a:?} {b:?}");
    }
    for (i, v) in BODY_VERTICES.iter().enumerate() {
        assert!(close3(p.positions[i], mesh_space(*v), 1e-4));
    }
    assert_eq!(p.normals, mesh.primitives[0].normals);
    assert_eq!(p.indices, mesh.primitives[0].indices);
}

#[test]
fn skinning_follows_the_posed_bones() {
    let mesh = body();
    let skeleton = skeleton();
    // A quarter turn of the spine about +Z: everything above it turns.
    let spine = Track {
        bone: "Bip01 Spine".to_owned(),
        rotations: vec![(0.0, quat_axis_angle([0.0, 0.0, 1.0], FRAC_PI_2))],
        ..Track::default()
    };
    let posed = skin(
        &mesh,
        &skeleton,
        &pose(&skeleton, &animation_with(vec![spine]), 0.0),
    );
    let p = &posed.primitives[0];
    // The spine bone's origin is at (0, 0, 60): (x, y) turns to (-y, x).
    let turn = |v: [f32; 3]| -> [f32; 3] {
        let m = mesh_space(v);
        [-m[1], m[0], m[2]]
    };
    for i in [0, 1, 4, 5] {
        assert!(
            close3(p.positions[i], turn(BODY_VERTICES[i]), 1e-4),
            "vertex {i}"
        );
    }
    // Normals turn with the bones and stay unit length.
    let normals = p.normals.as_ref().unwrap();
    assert!(close3(normals[0], [1.0, 0.0, 0.0], 1e-5));
    // Arm vertex 7 follows the arm, a child of the spine.
    assert!(close3(p.positions[7], turn(BODY_VERTICES[7]), 1e-4));

    // Blending: vertex 2 is half spine, half head. Turning only the head
    // moves it half way.
    let head = Track {
        bone: "Bip01 Head".to_owned(),
        translations: vec![(0.0, [0.0, 10.0, 30.0])],
        ..Track::default()
    };
    let posed = skin(
        &mesh,
        &skeleton,
        &pose(&skeleton, &animation_with(vec![head]), 0.0),
    );
    let q = &posed.primitives[0].positions;
    assert!(close3(
        q[2],
        std::array::from_fn(|k| mesh_space(BODY_VERTICES[2])[k] + [0.0, 5.0, 0.0][k]),
        1e-4
    ));
    assert!(close3(
        q[4],
        std::array::from_fn(|k| mesh_space(BODY_VERTICES[4])[k] + [0.0, 10.0, 0.0][k]),
        1e-4
    ));
    assert!(close3(q[0], mesh_space(BODY_VERTICES[0]), 1e-4));
}

#[test]
fn a_skin_bone_missing_from_the_skeleton_keeps_its_bind_placement() {
    let mesh = body();
    let mut skeleton = skeleton();
    skeleton.bones[4].name = "Somebody Else".to_owned(); // the head
    let head_up = Track {
        bone: "Bip01 Spine".to_owned(),
        translations: vec![(0.0, [0.0, 0.0, 20.0])],
        ..Track::default()
    };
    let posed = skin(
        &mesh,
        &skeleton,
        &pose(&skeleton, &animation_with(vec![head_up]), 0.0),
    );
    let p = &posed.primitives[0].positions;
    // Head-only vertices stay; spine-only vertices move up by 10.
    assert!(close3(p[4], mesh_space(BODY_VERTICES[4]), 1e-4));
    assert!(close(p[0][2], mesh_space(BODY_VERTICES[0])[2] + 10.0, 1e-4));
}

#[test]
fn unskinned_primitives_pass_through_skinning_unchanged() {
    let static_mesh =
        extract_mesh(&parse(&nif::fixture::box_mesh([10.0; 3], "t.dds")).unwrap()).unwrap();
    let skeleton = skeleton();
    let posed = skin(&static_mesh, &skeleton, &Pose::rest(&skeleton));
    assert_eq!(posed, static_mesh);
}

// ---------------------------------------------------------------- animation

#[test]
fn the_walk_parses_into_plain_tracks() {
    let (animation, report) =
        extract_animation_with_report(&nif::parse_animation_file(&walk_kf()).unwrap()).unwrap();
    assert_eq!(animation.name, "Walk");
    assert_eq!(animation.duration, WALK_DURATION);
    assert!(animation.looping);
    assert_eq!(animation.accum_root, "Bip01");
    assert_eq!(
        animation.text_keys,
        [(0.0, "start".to_owned()), (WALK_DURATION, "end".to_owned())]
    );
    let bones: Vec<&str> = animation.tracks.iter().map(|t| t.bone.as_str()).collect();
    assert_eq!(
        bones,
        ["Bip01", "Bip01 NonAccum", "Bip01 Spine", "Bip01 L UpperArm"]
    );
    assert_eq!(report.unhandled.get("NiFloatInterpolator"), Some(&1));

    let root = animation.track("Bip01").unwrap();
    assert_eq!(
        root.translations,
        [(0.0, [0.0; 3]), (WALK_DURATION, [0.0, WALK_STRIDE, 0.0])]
    );
    // Channels without keys take the interpolator's transform, unless it is
    // marked as not set.
    assert_eq!(root.rotations, [(0.0, [1.0, 0.0, 0.0, 0.0])]);
    assert_eq!(root.scales, [(0.0, 1.0)]);
    let hips = animation.track("Bip01 NonAccum").unwrap();
    assert_eq!(hips.translations, [(0.0, [0.0, 0.0, 50.0])]);

    // The B-spline track: sampled at 30 per second, both ends included.
    let spine = animation.track("Bip01 Spine").unwrap();
    assert!(spine.translations.is_empty() && spine.scales.is_empty());
    let samples = (WALK_DURATION * SAMPLE_RATE) as usize + 1;
    assert_eq!(spine.rotations.len(), samples);
    assert_eq!(spine.rotations[0].0, 0.0);
    assert_eq!(spine.rotations[samples - 1].0, WALK_DURATION);
    let z = |degrees: f32| quat_axis_angle([0.0, 0.0, 1.0], degrees.to_radians());
    assert!(angle(spine.rotations[0].1, z(0.0)) < 0.05);
    assert!(angle(spine.rotations[samples - 1].1, z(90.0)) < 0.05);
    // The control polygon is symmetric, so the middle is at 45 degrees.
    assert!(angle(spine.rotation_at(0.5).unwrap(), z(45.0)) < 0.05);
    for (_, q) in &spine.rotations {
        assert!(close(q.iter().map(|v| v * v).sum::<f32>(), 1.0, 1e-5));
    }

    let arm = animation.track("Bip01 L UpperArm").unwrap();
    assert_eq!(arm.rotations.len(), 3);
    assert!(
        angle(
            arm.rotation_at(0.25).unwrap(),
            quat_axis_angle([1.0, 0.0, 0.0], FRAC_PI_4)
        ) < 0.01
    );

    assert_eq!(parse_animation(&walk_kf()).unwrap(), animation);
}

#[test]
fn times_are_measured_from_the_sequence_start() {
    let mut desc = walk_desc();
    desc.start_time = 2.0;
    desc.stop_time = 3.0;
    desc.text_keys = vec![(2.5, "hit".to_owned())];
    desc.tracks.truncate(1);
    if let KfTrack::Keyed {
        data: Some(data), ..
    } = &mut desc.tracks[0]
    {
        data.translations = linear_translations(&[(2.0, [0.0; 3]), (3.0, [1.0, 0.0, 0.0])]);
    }
    desc.cycle_type = CYCLE_CLAMP;
    let animation = parse_animation(&desc.to_kf()).unwrap();
    assert_eq!(animation.duration, 1.0);
    assert!(!animation.looping);
    assert_eq!(animation.text_keys, [(0.5, "hit".to_owned())]);
    assert_eq!(animation.tracks[0].translations[1], (1.0, [1.0, 0.0, 0.0]));
}

fn one_track(data: NiTransformData) -> Track {
    let kf = KfDesc::new("Keys", 1.0).with_track(KfTrack::Keyed {
        bone: "Bone".to_owned(),
        transform: unset_transform(),
        data: Some(data),
    });
    parse_animation(&kf.to_kf()).unwrap().tracks.remove(0)
}

fn float_group(interpolation: u32, keys: &[(f32, f32, f32, f32)]) -> KeyGroup<f32> {
    KeyGroup {
        interpolation,
        keys: keys
            .iter()
            .map(|&(time, value, forward, backward)| Key {
                time,
                value,
                forward,
                backward,
                tbc: [0.0; 3],
            })
            .collect(),
    }
}

#[test]
fn constant_keys_become_steps() {
    let track = one_track(NiTransformData {
        scales: float_group(KEY_CONSTANT, &[(0.0, 1.0, 0.0, 0.0), (0.5, 2.0, 0.0, 0.0)]),
        ..NiTransformData::default()
    });
    assert_eq!(track.scales, [(0.0, 1.0), (0.5, 1.0), (0.5, 2.0)]);
    assert_eq!(track.scale_at(0.49), Some(1.0));
    assert_eq!(track.scale_at(0.5), Some(2.0));
    assert_eq!(track.scale_at(0.9), Some(2.0));

    let mut rotations = linear_rotations(&[
        (0.0, [1.0, 0.0, 0.0, 0.0]),
        (0.5, quat_axis_angle([0.0, 0.0, 1.0], 1.0)),
    ]);
    rotations.rotation_type = KEY_CONSTANT;
    let track = one_track(rotations);
    assert_eq!(track.rotations.len(), 3);
    assert_eq!(track.rotation_at(0.25), Some([1.0, 0.0, 0.0, 0.0]));
}

#[test]
fn quadratic_keys_follow_their_hermite_curve() {
    // From 0 to 1 over one second, tangents 3 (out) and 0 (in): the value at
    // s = 0.5 is 0.125·3 + 0.5 = 0.875.
    let track = one_track(NiTransformData {
        scales: float_group(KEY_QUADRATIC, &[(0.0, 0.0, 3.0, 0.0), (1.0, 1.0, 0.0, 0.0)]),
        ..NiTransformData::default()
    });
    assert_eq!(track.scales.len(), SAMPLE_RATE as usize + 1);
    assert!(close(track.scale_at(0.5).unwrap(), 0.875, 1e-5));
    assert_eq!(track.scales.last(), Some(&(1.0, 1.0)));
}

#[test]
fn tbc_keys_with_zero_parameters_are_catmull_rom() {
    // Evenly spaced keys on a line: Catmull–Rom reproduces the line.
    let mut group = float_group(
        KEY_TBC,
        &[
            (0.0, 0.0, 0.0, 0.0),
            (1.0, 1.0, 0.0, 0.0),
            (2.0, 2.0, 0.0, 0.0),
        ],
    );
    group.keys.iter_mut().for_each(|k| k.tbc = [0.0; 3]);
    let track = one_track(NiTransformData {
        scales: group,
        ..NiTransformData::default()
    });
    for t in [0.25f32, 0.5, 1.0, 1.5, 1.9] {
        assert!(close(track.scale_at(t).unwrap(), t, 1e-4), "t = {t}");
    }
}

#[test]
fn euler_keys_rotate_about_x_then_y_then_z() {
    let groups = [
        float_group(KEY_QUADRATIC, &[(0.0, FRAC_PI_2, 0.0, 0.0)]),
        float_group(0, &[]),
        float_group(KEY_QUADRATIC, &[(0.0, FRAC_PI_2, 0.0, 0.0)]),
    ];
    let track = one_track(NiTransformData {
        rotation_type: KEY_XYZ_ROTATION,
        xyz_rotations: Some(Box::new(groups)),
        ..NiTransformData::default()
    });
    let q = track.rotation_at(0.0).unwrap();
    assert!(angle(q, euler_xyz_to_quat(FRAC_PI_2, 0.0, FRAC_PI_2)) < 1e-3);
    // X first: +Y goes to +Z, then Z turns +X to +Y and leaves +Z.
    let r = quat_to_rotation(q);
    let apply = |v: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|i| r[i][0] * v[0] + r[i][1] * v[1] + r[i][2] * v[2])
    };
    assert!(close3(apply([0.0, 1.0, 0.0]), [0.0, 0.0, 1.0], 1e-6));
    assert!(close3(apply([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0], 1e-6));
}

#[test]
fn an_unknown_key_type_is_a_parse_error() {
    let bytes = KfDesc::new("Bad", 1.0)
        .with_track(KfTrack::Keyed {
            bone: "Bone".to_owned(),
            transform: unset_transform(),
            data: Some(NiTransformData {
                scales: float_group(9, &[(0.0, 1.0, 0.0, 0.0)]),
                ..NiTransformData::default()
            }),
        })
        .to_kf();
    let error = parse_animation(&bytes).unwrap_err();
    assert!(
        matches!(
            error,
            NifError::BadValue {
                what: "key interpolation",
                value: 9,
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn spline_handles_past_the_control_points_are_an_error() {
    let mut desc = walk_desc();
    if let KfTrack::Spline { interpolator, .. } = &mut desc.tracks[2] {
        interpolator.rotation_handle = 4;
    }
    let error = parse_animation(&desc.to_kf()).unwrap_err();
    assert!(matches!(error, NifError::Scene { .. }), "{error}");
}

#[test]
fn an_uncompressed_spline_is_sampled_from_its_floats() {
    // Translation along X from 0 to 30 over four control points.
    let points = [
        0.0f32, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0, 0.0, 0.0, 30.0, 0.0, 0.0,
    ];
    let track = KfTrack::Spline {
        bone: "Bone".to_owned(),
        interpolator: NiBSplineTransformInterpolator {
            start_time: 0.0,
            stop_time: 2.0,
            transform: unset_transform(),
            translation_handle: 0,
            rotation_handle: NO_HANDLE,
            scale_handle: u32::MAX,
            ..NiBSplineTransformInterpolator::default()
        },
        data: NiBSplineData {
            float_control_points: points.to_vec(),
            compact_control_points: Vec::new(),
        },
        control_points: 4,
    };
    let animation = parse_animation(&KfDesc::new("Spline", 2.0).with_track(track).to_kf()).unwrap();
    let t = &animation.tracks[0];
    assert!(t.rotations.is_empty() && t.scales.is_empty());
    assert_eq!(t.translations.len(), 61);
    assert_eq!(t.translations[0].1, [0.0; 3]);
    assert!(close3(t.translations[60].1, [30.0, 0.0, 0.0], 1e-5));
    // Four points of a cubic: a Bézier curve with evenly spaced points is
    // the straight line at uniform speed.
    assert!(close3(
        t.translation_at(0.5).unwrap(),
        [7.5, 0.0, 0.0],
        1e-4
    ));
    assert!(close3(
        t.translation_at(1.0).unwrap(),
        [15.0, 0.0, 0.0],
        1e-4
    ));
}

#[test]
fn compression_is_offset_plus_half_range() {
    let values = [0.0f32, 0.25, 1.0, -1.0];
    let packed = compress_control_points(&values, 0.0, 1.0);
    assert_eq!(packed, [0, 8192, 32767, -32767]);
    let c = BSplineCompression {
        rotation_offset: 0.0,
        rotation_half_range: 1.0,
        ..BSplineCompression::default()
    };
    for (&v, &p) in values.iter().zip(&packed) {
        let back = c.rotation_offset + c.rotation_half_range * (f32::from(p) / 32767.0);
        assert!(close(back, v, 1e-4));
    }
}

#[test]
fn a_file_without_a_sequence_is_an_animation_error() {
    let error = parse_animation(&skeleton_nif()).unwrap_err();
    assert!(matches!(error, NifError::Animation { .. }), "{error}");
    assert!(parse_animation(b"not a nif").is_err());
}

// ---------------------------------------------------------------- posing

#[test]
fn an_animation_without_tracks_gives_the_rest_pose() {
    let skeleton = skeleton();
    let posed = pose(&skeleton, &animation_with(Vec::new()), 0.3);
    assert_eq!(posed, Pose::rest(&skeleton));
}

#[test]
fn the_walk_travels_and_is_held_in_place() {
    let skeleton = skeleton();
    let walk = parse_animation(&walk_kf()).unwrap();
    let bip = skeleton.find("Bip01").unwrap() as usize;
    let hips = skeleton.find("Bip01 NonAccum").unwrap() as usize;
    let spine = skeleton.find("Bip01 Spine").unwrap() as usize;
    for (time, travel) in [
        (0.0, 0.0),
        (0.25, 25.0),
        (0.5, 50.0),
        (1.0, 100.0),
        (1.5, 150.0),
        (2.25, 225.0),
    ] {
        let p = pose(&skeleton, &walk, time);
        assert!(close(p.travel[0], 0.0, 1e-4), "t = {time}");
        assert!(
            close(p.travel[1], travel, 1e-3),
            "t = {time}: {:?}",
            p.travel
        );
        // The accumulation root stays at the origin: the skeleton's lift
        // to hip height is not used, the animation's hips lift instead.
        assert!(close3(apply_point(&p.world[bip], [0.0; 3]), [0.0; 3], 1e-4));
        assert!(close3(
            apply_point(&p.world[hips], [0.0; 3]),
            [0.0, 0.0, 50.0],
            1e-4
        ));
        assert!(close3(
            apply_point(&p.world[spine], [0.0; 3]),
            [0.0, 0.0, 60.0],
            1e-4
        ));
    }
    // A clamped copy holds its last frame and travels no further.
    let mut clamped = walk.clone();
    clamped.looping = false;
    assert!(close(pose(&skeleton, &clamped, 5.0).travel[1], 100.0, 1e-3));
    assert_eq!(
        pose(&skeleton, &clamped, -1.0),
        pose(&skeleton, &clamped, 0.0)
    );

    // The spline turns the spine; at half time by 45 degrees.
    let p = pose(&skeleton, &walk, 0.5);
    let x = apply_vector(&p.world[spine], [1.0, 0.0, 0.0]);
    assert!(close3(x, [FRAC_PI_4.cos(), FRAC_PI_4.sin(), 0.0], 1e-3));
}

#[test]
fn rotations_take_the_shorter_arc() {
    let skeleton = skeleton();
    // The second key is the negated quaternion of a quarter turn: the same
    // rotation. Half way must be an eighth turn, not three eighths.
    let quarter = quat_axis_angle([0.0, 0.0, 1.0], FRAC_PI_2).map(|v| -v);
    let track = Track {
        bone: "Bip01 Spine".to_owned(),
        rotations: vec![(0.0, [1.0, 0.0, 0.0, 0.0]), (1.0, quarter)],
        ..Track::default()
    };
    let q = track.rotation_at(0.5).unwrap();
    assert!(angle(q, quat_axis_angle([0.0, 0.0, 1.0], FRAC_PI_4)) < 1e-3);
    assert!(angle(slerp([1.0, 0.0, 0.0, 0.0], quarter, 0.5), q) < 1e-4);
    let p = pose(&skeleton, &animation_with(vec![track]), 0.5);
    let x = apply_vector(&p.world[3], [1.0, 0.0, 0.0]);
    assert!(close3(x, [FRAC_PI_4.cos(), FRAC_PI_4.sin(), 0.0], 1e-4));
}

#[test]
fn posing_and_skinning_are_deterministic() {
    let skeleton = skeleton();
    let walk = parse_animation(&walk_kf()).unwrap();
    let mesh = body();
    let a = skin(&mesh, &skeleton, &pose(&skeleton, &walk, 0.37));
    let b = skin(
        &mesh.clone(),
        &skeleton.clone(),
        &pose(&skeleton, &walk, 0.37),
    );
    assert_eq!(nif::glb::write(&a), nif::glb::write(&b));
}

#[test]
fn documents_serialise_without_loss() {
    let skeleton = skeleton();
    let walk = parse_animation(&walk_kf()).unwrap();
    let back: Skeleton = serde_json::from_str(&serde_json::to_string(&skeleton).unwrap()).unwrap();
    assert_eq!(back, skeleton);
    let back: Animation = serde_json::from_str(&serde_json::to_string(&walk).unwrap()).unwrap();
    assert_eq!(back, walk);
}

#[test]
fn older_animation_stream_versions_parse_as_animations_only() {
    let current = parse_animation(&walk_kf()).unwrap();
    for stream in [21, 25, 27, 33] {
        let mut desc = walk_desc();
        desc.stream_version = stream;
        let bytes = desc.to_kf();
        assert_eq!(parse_animation(&bytes).unwrap(), current, "stream {stream}");
        let error = parse(&bytes).unwrap_err();
        assert!(
            matches!(error, NifError::UnsupportedVersion { stream_version, .. } if stream_version == stream)
        );
    }
    let mut desc = walk_desc();
    desc.stream_version = 13;
    assert!(parse_animation(&desc.to_kf()).is_err());
}

#[test]
fn new_blocks_read_back_as_written() {
    // Re-encode every parsed block and parse again: the fixture encoder and
    // the parser agree on every layout.
    for bytes in [
        skinned_body(),
        walk_kf(),
        skinned_body_with(&body_weights(), true),
    ] {
        let file = parse(&bytes).unwrap();
        let mut builder = NifBuilder::new();
        for entry in &file.blocks {
            match &entry.block {
                Block::Skipped => {
                    let raw = bytes[entry.offset..entry.offset + entry.size as usize].to_vec();
                    builder.push_raw(&entry.type_name, raw);
                }
                block => {
                    builder.push(&entry.type_name, block);
                }
            }
        }
        for &root in &file.roots {
            builder.add_root(root);
        }
        let again = parse(&builder.build()).unwrap();
        let blocks =
            |f: &nif::NifFile| -> Vec<Block> { f.blocks.iter().map(|e| e.block.clone()).collect() };
        assert_eq!(blocks(&again), blocks(&file));
    }
}

#[test]
fn a_mesh_counts_animation_and_unused_skin_blocks_as_skipped() {
    // The skinned body uses its skin blocks: none is counted.
    let body = body();
    assert!(
        nif::SKIN_TYPES
            .iter()
            .all(|t| !body.skipped_blocks.contains_key(*t)),
        "{:?}",
        body.skipped_blocks
    );
    // A static mesh carrying animation blocks reports them, as it did
    // before they were parsed.
    let mut builder = NifBuilder::new();
    builder.add_scene(
        &nif::fixture::NodeDesc::root("Spinner")
            .with_shape(nif::fixture::box_shape("Box", [1.0; 3], "")),
    );
    builder.push(
        "NiTransformData",
        &NiTransformData {
            scales: float_group(KEY_CONSTANT, &[(0.0, 1.0, 0.0, 0.0)]),
            ..NiTransformData::default()
        }
        .into(),
    );
    let mesh = extract_mesh(&parse(&builder.build()).unwrap()).unwrap();
    assert_eq!(mesh.skipped_blocks.get("NiTransformData"), Some(&1));
    assert!(
        mesh.primitives
            .iter()
            .all(|p| p.skin.is_none() && !p.skinned)
    );
}

#[test]
fn sampling_is_bounded() {
    // 80 quadratic keys a day apart would sample to millions of keys.
    let keys: Vec<(f32, f32, f32, f32)> = (0..80)
        .map(|i| (i as f32 * 86_400.0, 1.0, 0.0, 0.0))
        .collect();
    let bytes = KfDesc::new("Long", 80.0 * 86_400.0)
        .with_track(KfTrack::Keyed {
            bone: "Bone".to_owned(),
            transform: unset_transform(),
            data: Some(NiTransformData {
                scales: float_group(KEY_QUADRATIC, &keys),
                ..NiTransformData::default()
            }),
        })
        .to_kf();
    assert!(bytes.len() < 4096);
    let error = parse_animation(&bytes).unwrap_err();
    assert!(matches!(error, NifError::Animation { .. }), "{error}");
}
