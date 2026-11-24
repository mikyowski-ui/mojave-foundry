//! Readability metrics, low (kerb-height) blocking strips, and the
//! interactive references' protection.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use common::*;
use map_builder::cover::{Cover, Sight};
use map_builder::exits::hexes_around;
use map_builder::islands::engine_blocked;
use map_builder::walk::{HEXES, Verdict, neighbours, verdicts};
use map_builder::{ArtCategory, Built, Role, build_map};
use vault_formats::map::MapFile;
use vault_formats::pro::BasicFlags;
use vault_formats::{ByteSource, Fid, GameProfile, LayeredVfs, LocalPrototypeCatalog, MemoryDir};

fn mounted(layer: &emit::ModLayer, base: &BTreeMap<String, Vec<u8>>) -> LayeredVfs {
    let tree = |files: Vec<(String, Vec<u8>)>| {
        MemoryDir::new(
            files
                .into_iter()
                .map(|(path, bytes)| (path, ByteSource::new(bytes).unwrap())),
        )
        .unwrap()
    };
    let mut files: Vec<(String, Vec<u8>)> = layer
        .files()
        .iter()
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    files.push((
        "vault-mod.toml".into(),
        layer.manifest_text().unwrap().into_bytes(),
    ));
    LayeredVfs::new(
        Arc::new(tree(base.clone().into_iter().collect())),
        vec![vault_formats::ModLayer::from_tree("test", tree(files))],
    )
}

fn parse_map(built: &Built, base: &BTreeMap<String, Vec<u8>>) -> MapFile {
    let vfs = mounted(&built.layer, base);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    use vault_formats::Vfs;
    MapFile::parse_for(
        &vfs.read("maps/synth.map").unwrap(),
        &catalog,
        GameProfile::Fallout2,
    )
    .unwrap()
}

#[test]
fn the_metrics_match_an_independent_count() {
    let scene = town();
    let built = build_map(&scene.input()).unwrap();
    let r = built.report.readability;

    // Floor: walkable with nothing blocked.
    let frame = scene.frame();
    let (free, _) = verdicts(&frame, &scene.sampler, &scene.cells, &[], &scene.options);
    let floor = free.iter().filter(|v| **v == Verdict::Walkable).count();
    assert_eq!(r.floor_hexes, floor);
    let walkable: Vec<i32> = (0..HEXES as i32)
        .filter(|h| built.walkable[*h as usize])
        .collect();
    assert_eq!(r.walkable_hexes, walkable.len());
    assert!(
        r.walkable_hexes < r.floor_hexes,
        "the placements block some"
    );

    // Sight, by the public cover index.
    let cover = Cover::new(&scene.art, &scene.placements);
    let mut sights = [0usize; 3];
    for hex in &walkable {
        sights[cover.sight(*hex, scene.options.entrance_box) as usize] += 1;
    }
    assert_eq!([r.clear, r.behind, r.covered], sights);
    assert_eq!(r.clear + r.behind + r.covered, r.walkable_hexes);
    assert!(r.covered > 0 && r.clear > 0);
    assert_eq!(r.open_floor_ratio, r.clear as f64 / r.floor_hexes as f64);

    // Near a blocking placement (the item does not block).
    let blocking: BTreeSet<i32> = scene
        .placements
        .iter()
        .filter(|p| p.blocking && p.role != Role::Item)
        .map(|p| p.hex)
        .collect();
    let near = walkable
        .iter()
        .filter(|h| neighbours(**h).any(|n| blocking.contains(&n)))
        .count();
    assert_eq!(r.near_blocking, near);
    assert!(near > 0);
    assert_eq!(r.near_blocking_share, near as f64 / walkable.len() as f64);

    let lines = built.report.readability_summary();
    assert_eq!(lines.len(), 4, "no interactive line without keys");
    assert!(lines[2].contains(&format!("{:.3}", r.open_floor_ratio)));
    // Deterministic.
    assert_eq!(build_map(&scene.input()).unwrap().report.readability, r);
}

/// The town with a closed ring of blocking wall strips `height` pixels tall
/// two steps around `centre`, and a critter standing inside it.
fn ringed(height: u32, centre: i32) -> (Scene, Vec<i32>) {
    let mut scene = town();
    let art = scene.art.len();
    scene
        .art
        .push(image(ArtCategory::Walls, 32, height, 33, "kerb"));
    let ring = hexes_around(centre, 2, true);
    for (i, hex) in ring.iter().enumerate() {
        scene.placements.push(placement(
            art,
            *hex,
            Role::WallStrip,
            true,
            &format!("synthetic.esm:00b{i:03}"),
        ));
    }
    let frame = scene.frame();
    scene.critter_art = vec![sprite_set("resident", 1)];
    scene.critters = vec![critter(
        0,
        "synthetic.esm:00c001",
        "Resident",
        hex_point(&frame, centre),
        0.0,
    )];
    (scene, ring)
}

#[test]
fn a_kerb_high_blocking_strip_blocks_exactly_like_a_tall_one() {
    let frame = town().frame();
    let centre = hex_at(&frame, 56.0, 46.0);
    let (kerb, ring) = ringed(12, centre);
    let (tall, _) = ringed(120, centre);
    let low = build_map(&kerb.input()).unwrap();
    let high = build_map(&tall.input()).unwrap();

    // The same walkable grid, the same blockers, the same islands.
    assert_eq!(low.walkable, high.walkable);
    assert_eq!(low.report.blockers, high.report.blockers);
    assert_eq!(low.report.components.sizes, high.report.components.sizes);
    for hex in &ring {
        assert!(
            !low.walkable[*hex as usize],
            "a kerb strip blocks hex {hex}"
        );
    }
    assert!(low.walkable[centre as usize]);

    // A passage would have to cross a kerb strip: it is never opened, and
    // the critter's island is left, in both builds.
    assert!(low.report.passages.is_empty());
    assert_eq!(low.report.islands_left.len(), 1);
    assert_eq!(low.report.islands_left, high.report.islands_left);
    assert!(
        low.report.islands_left[0]
            .reason
            .starts_with("no passage through openable hexes"),
        "{}",
        low.report.islands_left[0].reason
    );

    // On the written map each kerb is wall art the engine stops at, with
    // no sealing blocker of its own.
    let map = parse_map(&low, &kerb.base);
    let objects = &map.objects.elevations[0];
    let stops = engine_blocked(objects);
    let ring_set: BTreeSet<i32> = ring.iter().copied().collect();
    for hex in &ring {
        assert!(stops[*hex as usize]);
        let here: Vec<_> = objects.iter().filter(|o| o.hex == Some(*hex)).collect();
        assert_eq!(here.len(), 1, "one object, the strip, on hex {hex}");
        assert_eq!(Fid(here[0].art_id).category(), 3, "wall art");
        assert!(!here[0].flags.contains(BasicFlags::NONBLOCKING));
    }
    // Inside the ring only the critter's hex blocks; the pocket is closed.
    let pocket: Vec<i32> = hexes_around(centre, 1, false);
    for hex in &pocket {
        assert!(neighbours(*hex).all(|n| pocket.contains(&n) || ring_set.contains(&n)));
    }

    // Art height does matter to sight: the kerb hides less.
    let (l, h) = (low.report.readability, high.report.readability);
    assert_eq!(l.floor_hexes, h.floor_hexes);
    assert_eq!(l.walkable_hexes, h.walkable_hexes);
    assert_eq!(l.near_blocking, h.near_blocking);
    assert!(l.covered < h.covered, "{} vs {}", l.covered, h.covered);
    assert!(l.open_floor_ratio > h.open_floor_ratio);
}

#[test]
fn a_kerb_in_front_of_a_walker_does_not_cover_it() {
    // The sight rule itself: a 12-pixel strip one row south (drawn later)
    // of a figure's hex overlaps only its feet, which a tall one covers.
    let frame = town().frame();
    let at = hex_at(&frame, 50.0, 50.0);
    let front = at + 200;
    for (height, expected) in [(12, Sight::Clear), (120, Sight::Covered)] {
        let mut art = image(ArtCategory::Walls, 32, height, 30, "strip");
        art.shift = [0, 0];
        let art = vec![art];
        let cover = Cover::new(
            &art,
            &[placement(
                0,
                front,
                Role::WallStrip,
                true,
                "synthetic.esm:00d001",
            )],
        );
        assert_eq!(cover.sight(at, [32, 72]), expected, "{height} pixels");
    }
}

#[test]
fn an_interactive_reference_never_gets_the_floor_piece_treatment() {
    let mut scene = town();
    let frame = scene.frame();
    let hex = hex_at(&frame, 50.0, 50.0);
    let key = "synthetic.esm:00e001";
    // Scenery art 1, placed as a walking surface of a reference the caller
    // marks interactive (a desk top, say), plus an upright part of it.
    scene
        .placements
        .push(placement(1, hex, Role::FloorPiece, true, key));
    scene.placements.push(placement(
        1,
        hex_at(&frame, 51.0, 50.0),
        Role::Scenery,
        true,
        key,
    ));
    let floor_id = scene.placements.len() as u32 - 1;
    let plain = build_map(&scene.input()).unwrap();
    assert!(
        plain.walkable[hex as usize],
        "as a floor piece it blocks nothing"
    );

    scene.options.interactive = [key.to_owned(), "synthetic.esm:0fffff".to_owned()].into();
    let built = build_map(&scene.input()).unwrap();
    let i = built.report.interactive;
    assert_eq!(
        (i.references, i.matched, i.placements, i.kept_upright),
        (2, 1, 2, 1)
    );
    assert!(!built.walkable[hex as usize], "upright, it blocks as given");
    let map = parse_map(&built, &scene.base);
    let object = map.objects.elevations[0]
        .iter()
        .find(|o| o.id == floor_id)
        .unwrap();
    assert_eq!(object.hex, Some(hex));
    for flag in [
        BasicFlags::FLAT,
        BasicFlags::NONBLOCKING,
        BasicFlags::NO_TRANSLUCENCY,
    ] {
        assert!(!object.flags.contains(flag), "{:#x} is clear", flag.0);
    }
    assert_eq!(Fid(object.art_id).category(), 2, "still scenery art");
    let lines = built.report.readability_summary();
    assert!(
        lines
            .last()
            .unwrap()
            .contains("1 floor pieces kept upright")
    );
}

#[test]
fn interactive_keys_that_change_nothing_leave_the_layer_alone() {
    let mut scene = town();
    let before = build_map(&scene.input()).unwrap();
    // The door and the container: upright already, so nothing to withhold.
    scene.options.interactive = [
        scene.placements[5].reference.clone(),
        scene.placements[3].reference.clone(),
    ]
    .into();
    let after = build_map(&scene.input()).unwrap();
    assert_eq!(before.layer.files(), after.layer.files());
    assert_eq!(
        before.layer.manifest_text().unwrap(),
        after.layer.manifest_text().unwrap()
    );
    assert_eq!(before.walkable, after.walkable);
    let i = after.report.interactive;
    assert_eq!(
        (i.references, i.matched, i.placements, i.kept_upright),
        (2, 2, 2, 0)
    );
    assert_eq!(before.report.interactive.references, 0);
    let mut report = after.report.clone();
    report.interactive = before.report.interactive;
    assert_eq!(report, before.report, "only the count differs");
}
