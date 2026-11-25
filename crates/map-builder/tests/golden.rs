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
    ("town", 0x402c95f006c12ebc, 0xc980e5eb7a501bc9),
    ("town-base", 0x595090e43aaea43e, 0x7288485670a06dfe),
    ("town-standalone", 0x873ec107721e02ef, 0xaec2e3c2ac9d2612),
    ("town-numbered", 0x1609907bd8919e3f, 0xa004353677849ee5),
    ("critter-town", 0x595090e43aaea43e, 0x7288485670a06dfe),
    ("door-scene", 0x92101346821a50c3, 0x47ae7b3b9830b7e9),
    ("doors", 0x9fea1cd23449b105, 0xb32b1035072169f2),
    ("doors-base", 0x70f3e819a253b1c0, 0x22d96af569242e8e),
    ("doors-standalone", 0xed2d0daf54dac6d7, 0xd6b50a57d06faff2),
    ("populated", 0x1f8f5973f5d68272, 0xa80baf81a5d5e013),
    ("populated-packet", 0xa0352fc7e5a597f0, 0x7272837490fb8a5d),
    ("cut-town", 0xa8fb86c44e7bccdf, 0xf156751106fbd596),
    ("cut-town-off", 0x3a22299086deac99, 0xa0759ee7c83a2364),
    ("cut-town-rings", 0x320f76d8f1e8bad4, 0x95bd4a4a8b5044c1),
    ("connected", 0xb3448c1435e02344, 0xd781f373497094bb),
    ("saloon", 0xca17d4ce4d8e8243, 0x06e942d3b85b952e),
    ("saloon-clamped", 0x631db0ae2f35752c, 0xe77bbff5ea258b5d),
    ("saloon-interactive", 0xca17d4ce4d8e8243, 0x272656b058e3d33d),
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
    ("town", 0x8a41fe78eda93240, 0x445d17987aedd483),
    ("town-base", 0x304b6a6ef4ee2bda, 0xed784380452730b4),
    ("town-standalone", 0x480b8d6e3af8eb2b, 0xe238b8dcdef292c8),
    ("town-numbered", 0x02a0b77905576b83, 0xbd4b5b29d876d5cb),
    ("critter-town", 0x304b6a6ef4ee2bda, 0xed784380452730b4),
    ("door-scene", 0x75ceb7924cf9990f, 0x219e1936cb924cf0),
    ("doors", 0x036df4bc0425b4b5, 0x55e9cafd9d1ae7d2),
    ("doors-base", 0xc9558611bd72fe28, 0x5b39eefb65513cae),
    ("doors-standalone", 0xcc5972f85efe5837, 0x53003d6d57dd7cb6),
    ("populated", 0x8903fcdcfed8473a, 0x3b8383caf8c7d2e4),
    ("populated-packet", 0x0b10d07ffc8907d8, 0x82a05c3737cd4fda),
    ("cut-town", 0x3c7f049bc9def297, 0x0e24a5efbb1484b9),
    ("cut-town-off", 0x41e7c01b36b650ab, 0x3e791edb7c410dd1),
    ("cut-town-rings", 0x528cf97ea7b14ed8, 0x2193e6648534ab6e),
    ("connected", 0x36b3bc62a75371ec, 0x24bbf7fad926a4d1),
    ("saloon", 0xca17d4ce4d8e8243, 0xb872e0d86e1e4013),
    ("saloon-clamped", 0x631db0ae2f35752c, 0xbf30113185efeb1c),
    ("saloon-interactive", 0xca17d4ce4d8e8243, 0x2bfff174f5daedec),
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
