//! Golden digests: every synthetic single-map scene the test suite builds,
//! recorded before the layer refactor (`DESIGN-levels.md`, step 0). A
//! single-map build must stay byte for byte what it was, and so must its
//! report: both digests are pinned.

mod common;

use common::saloon::{DOOR_AT, interior, saloon};
use common::scenes::{add_door, cut_town, door_scene, doors_scene, full_base, populated, scene};
use common::*;
use map_builder::{Built, build_map};

/// FNV-1a over every layer path, file and the manifest text (the digest of
/// `tests/critters.rs`).
fn digest(layer: &emit::ModLayer) -> u64 {
    fnv(layer
        .files()
        .iter()
        .flat_map(|(path, bytes)| [path.as_bytes().to_vec(), bytes.clone()])
        .chain(std::iter::once(layer.manifest_text().unwrap().into_bytes())))
}

fn fnv(parts: impl IntoIterator<Item = Vec<u8>>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for part in parts {
        for byte in part {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The layer's digest and the digest of the report's `Debug` text.
fn digests(built: &Built) -> (u64, u64) {
    (
        digest(&built.layer),
        fnv([format!("{:?}", built.report).into_bytes()]),
    )
}

/// Every scene, by name, with the options that kept the old bytes (islands
/// neither bridged nor closed: both are on by default now) when `old`.
fn scenes(old: bool) -> Vec<(&'static str, Scene)> {
    let mut out = scene_list();
    if old {
        for (_, scene) in &mut out {
            scene.options.close_unreached = false;
            scene.options.coverage_bridges = false;
        }
    }
    out
}

fn scene_list() -> Vec<(&'static str, Scene)> {
    let mut out = Vec::new();
    out.push(("town", town()));
    let mut s = town();
    s.base = base_files(10);
    out.push(("town-base", s));
    let mut s = town();
    s.base = base_files(10);
    s.options.standalone = true;
    out.push(("town-standalone", s));
    let mut s = town();
    s.number = Some(200);
    s.base = base_files(10);
    s.options.write_maps_txt = false;
    out.push(("town-numbered", s));
    out.push(("critter-town", scene()));
    out.push(("door-scene", door_scene()));
    out.push(("doors", doors_scene()));
    let mut s = doors_scene();
    s.base = base_files(10);
    out.push(("doors-base", s));
    let mut s = doors_scene();
    s.base = base_files(10);
    s.options.standalone = true;
    out.push(("doors-standalone", s));
    out.push(("populated", populated()));
    let mut s = populated();
    s.options.critter_ai_packet = Some(2);
    s.options.critter_team = 12;
    out.push(("populated-packet", s));
    out.push(("cut-town", cut_town().0));
    let mut s = cut_town().0;
    s.options.reconnect = false;
    out.push(("cut-town-off", s));
    let mut s = cut_town().0;
    s.options.passage_rings = 1;
    out.push(("cut-town-rings", s));
    // The connected town of `tests/islands.rs`.
    let mut s = town();
    let frame = s.frame();
    add_door(&mut s, hex_at(&frame, 48.0, 48.0), 0xb01, 0xd01, None);
    add_door(
        &mut s,
        hex_at(&frame, 56.0, 48.0),
        0xb02,
        0xd01,
        Some(hex_point(&frame, hex_at(&frame, 56.0, 45.0))),
    );
    s.base = full_base();
    s.critter_art = vec![sprite_set("townsman", 1)];
    s.critters = vec![critter(
        0,
        "synthetic.esm:00c001",
        "Ann",
        hex_point(&frame, hex_at(&frame, 51.0, 53.0)),
        0.0,
    )];
    out.push(("connected", s));
    out.push(("saloon", saloon(interior(false, None))));
    out.push((
        "saloon-clamped",
        saloon(interior(true, Some([DOOR_AT[0], DOOR_AT[1]]))),
    ));
    let mut s = saloon(interior(false, None));
    s.options.interactive = [common::saloon::form(common::saloon::DOOR)].into();
    out.push(("saloon-interactive", s));
    out
}

/// Recorded on 2026-11-23 before any refactor: (scene, layer, report).
const GOLDEN: &[(&str, u64, u64)] = &[
    ("town", 0x402c95f006c12ebc, 0x725d34eec8b9f4e1),
    ("town-base", 0x595090e43aaea43e, 0xb7b012c089921d38),
    ("town-standalone", 0x873ec107721e02ef, 0xa8a767e675331554),
    ("town-numbered", 0x1609907bd8919e3f, 0x00043ba616e5c155),
    ("critter-town", 0x595090e43aaea43e, 0xb7b012c089921d38),
    ("door-scene", 0x92101346821a50c3, 0xf2206ef961007d01),
    ("doors", 0x9fea1cd23449b105, 0xde3e057046550334),
    ("doors-base", 0x70f3e819a253b1c0, 0x87349f408b3109c8),
    ("doors-standalone", 0xed2d0daf54dac6d7, 0xa62d22416c813534),
    ("populated", 0x1f8f5973f5d68272, 0x0b90f1e23d424f33),
    ("populated-packet", 0xa0352fc7e5a597f0, 0xc7fa2829b30b3ded),
    ("cut-town", 0xa8fb86c44e7bccdf, 0xee838ddb37790f90),
    ("cut-town-off", 0x3a22299086deac99, 0x86ffb050e67c9906),
    ("cut-town-rings", 0x320f76d8f1e8bad4, 0xea7dc3f86f2cd8d9),
    ("connected", 0xb3448c1435e02344, 0x134a6128d439669b),
    ("saloon", 0xca17d4ce4d8e8243, 0xb12b8b1e088722e8),
    ("saloon-clamped", 0x631db0ae2f35752c, 0x64b7772a18e8d0ed),
    ("saloon-interactive", 0xca17d4ce4d8e8243, 0xa15d3746c4247acd),
];

#[test]
fn every_single_map_scene_is_byte_identical() {
    let mut seen = Vec::new();
    for (name, scene) in scenes(true) {
        let built = build_map(&scene.input()).unwrap();
        let (layer, report) = digests(&built);
        // Twice: determinism.
        assert_eq!(
            digests(&build_map(&scene.input()).unwrap()),
            (layer, report),
            "{name}"
        );
        seen.push((name, layer, report));
    }
    for (name, layer, report) in &seen {
        println!("    (\"{name}\", {layer:#018x}, {report:#018x}),");
    }
    assert_eq!(seen.len(), GOLDEN.len(), "one golden entry per scene");
    for ((name, layer, report), (g_name, g_layer, g_report)) in seen.iter().zip(GOLDEN) {
        assert_eq!(name, g_name);
        assert_eq!((layer, report), (g_layer, g_report), "scene {name}");
    }
}

#[test]
fn the_town_digests_from_before_critters_hold() {
    let mut s = town();
    s.options.close_unreached = false;
    s.options.coverage_bridges = false;
    assert_eq!(
        digest(&build_map(&s.input()).unwrap().layer),
        0x402c_95f0_06c1_2ebc
    );
    s.base = base_files(10);
    assert_eq!(
        digest(&build_map(&s.input()).unwrap().layer),
        0x5950_90e4_3aae_a43e
    );
}

/// The same scenes with the defaults (islands closed and bridged), recorded
/// on 2026-11-24 when both became the default: only the scenes holding an
/// island that no walk from the entrance reaches differ from `GOLDEN`.
const GOLDEN_DEFAULT: &[(&str, u64, u64)] = &[
    ("town", 0x8a41fe78eda93240, 0x926cd431a99a03a3),
    ("town-base", 0x304b6a6ef4ee2bda, 0xe4e4c050123cac16),
    ("town-standalone", 0x480b8d6e3af8eb2b, 0xdfb34bf75fb5bdc2),
    ("town-numbered", 0x02a0b77905576b83, 0x623b8a2dedd7022b),
    ("critter-town", 0x304b6a6ef4ee2bda, 0xe4e4c050123cac16),
    ("door-scene", 0x75ceb7924cf9990f, 0x422546be133275ca),
    ("doors", 0x036df4bc0425b4b5, 0x008fdafd7221a214),
    ("doors-base", 0xc9558611bd72fe28, 0x8aa4d0dffa09bb68),
    ("doors-standalone", 0xcc5972f85efe5837, 0xfaa1291d0f2521b0),
    ("populated", 0x8903fcdcfed8473a, 0x3c98986cb6406386),
    ("populated-packet", 0x0b10d07ffc8907d8, 0x1493d0829f926bfc),
    ("cut-town", 0x3c7f049bc9def297, 0x1bbbcea202411191),
    ("cut-town-off", 0x41e7c01b36b650ab, 0x49f46f09e7d55d29),
    ("cut-town-rings", 0x528cf97ea7b14ed8, 0xd0c6b1f3cb201628),
    ("connected", 0x36b3bc62a75371ec, 0x2a8225964f541229),
    ("saloon", 0xca17d4ce4d8e8243, 0xa8fe2677be716f33),
    ("saloon-clamped", 0x631db0ae2f35752c, 0x9681d38e9d4098be),
    ("saloon-interactive", 0xca17d4ce4d8e8243, 0xd960e891966f054e),
];

#[test]
fn every_single_map_scene_with_the_defaults_is_pinned_and_deterministic() {
    let mut seen = Vec::new();
    for (name, scene) in scenes(false) {
        let built = build_map(&scene.input()).unwrap();
        let (layer, report) = digests(&built);
        assert_eq!(
            digests(&build_map(&scene.input()).unwrap()),
            (layer, report),
            "{name}"
        );
        seen.push((name, layer, report));
    }
    for (name, layer, report) in &seen {
        println!("    (\"{name}\", {layer:#018x}, {report:#018x}),");
    }
    assert_eq!(seen.len(), GOLDEN_DEFAULT.len(), "one entry per scene");
    for ((name, layer, report), (g_name, g_layer, g_report)) in seen.iter().zip(GOLDEN_DEFAULT) {
        assert_eq!(name, g_name);
        assert_eq!((layer, report), (g_layer, g_report), "scene {name}");
    }
}
