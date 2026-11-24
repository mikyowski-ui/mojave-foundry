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

/// Every scene, by name.
fn scenes() -> Vec<(&'static str, Scene)> {
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
    ("town", 0x402c95f006c12ebc, 0x4217c51d43b24176),
    ("town-base", 0x595090e43aaea43e, 0x7a0be1510972b5e9),
    ("town-standalone", 0x873ec107721e02ef, 0x20b6946d37725cb9),
    ("town-numbered", 0x1609907bd8919e3f, 0xda3c777091fb01dc),
    ("critter-town", 0x595090e43aaea43e, 0x7a0be1510972b5e9),
    ("door-scene", 0x92101346821a50c3, 0xb3121fe6082e1c94),
    ("doors", 0x9fea1cd23449b105, 0x0732e8ef2db0c7a7),
    ("doors-base", 0x70f3e819a253b1c0, 0xebd894ede57f47b5),
    ("doors-standalone", 0xed2d0daf54dac6d7, 0x1cbb9be7f38a10b9),
    ("populated", 0x1f8f5973f5d68272, 0xe4999c2fa5c6bf22),
    ("populated-packet", 0xa0352fc7e5a597f0, 0xfd8e9ef06c4ae22c),
    ("cut-town", 0xa8fb86c44e7bccdf, 0xcb977f756a4f7c3d),
    ("cut-town-off", 0x3a22299086deac99, 0xbc4fed3e7bcd3cd0),
    ("cut-town-rings", 0x320f76d8f1e8bad4, 0x0d9f649f7d334b6e),
    ("connected", 0xb3448c1435e02344, 0xc88d2b4454f8684a),
    ("saloon", 0xca17d4ce4d8e8243, 0xc86f159d14248939),
    ("saloon-clamped", 0x631db0ae2f35752c, 0x3ee4fbcc1dafa276),
    ("saloon-interactive", 0xca17d4ce4d8e8243, 0x3102a289ef18a72a),
];

#[test]
fn every_single_map_scene_is_byte_identical() {
    let mut seen = Vec::new();
    for (name, scene) in scenes() {
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
