//! Landmarks: meshes too large to slice, kept upright in pieces that the
//! engine draws whenever they show and that sort against every walker.

use std::collections::{BTreeMap, BTreeSet};

use super::slicing::{composed, expected, expected_screen};
use super::*;
use crate::engine_view::{VIEW, view_origin, visited_hexes};
use crate::landmark::{Limits, hexes_in_rect};
use crate::{ArtCategory, Class, ClassKind, MeshFacts, Role, SkipReason, classify, hex, strips};

/// The tower (48 hexes of floor) counts as a landmark above 40.
fn landmark_config() -> SlicerConfig {
    SlicerConfig {
        max_sliced_hexes: 40.0,
        ..SlicerConfig::default()
    }
}

fn tower(id: u32, ground: &GroundSampler, xy: [f32; 2], yaw: f32) -> Reference {
    reference(
        id,
        "STAT",
        "meshes\\test\\casinotower.nif",
        "tower",
        ground,
        xy,
        0.0,
        yaw,
    )
}

/// A power plant: a tall north block with a chimney and two wings around a
/// courtyard open to the south. About 1,000 pixels each way on screen, so
/// it is larger than the engine's view in both directions.
fn plant_mesh() -> StaticMesh {
    let boxes = [
        ([-1000.0, 250.0, 0.0], [1000.0, 750.0, 630.0]),
        ([-1000.0, -750.0, 0.0], [-500.0, 250.0, 420.0]),
        ([500.0, -750.0, 0.0], [1000.0, 250.0, 420.0]),
        ([-70.0, 400.0, 630.0], [70.0, 540.0, 1400.0]),
    ];
    let faces: Vec<[[f32; 3]; 4]> = boxes
        .iter()
        .flat_map(|(min, max)| nif::fixture::box_faces(*min, *max))
        .collect();
    let faces: Vec<&[[f32; 3]]> = faces.iter().map(|f| f.as_slice()).collect();
    mesh(
        &nif::fixture::NodeDesc::root("Plant")
            .with_shape(nif::fixture::ShapeDesc::from_faces("Plant:0", &faces, WALL))
            .to_nif(),
    )
}

fn plant_assets() -> TestAssets {
    TestAssets::standard().with("plant", plant_mesh())
}

fn plant(id: u32, ground: &GroundSampler, xy: [f32; 2], yaw: f32) -> Reference {
    reference(
        id,
        "STAT",
        "meshes\\test\\powerplant.nif",
        "plant",
        ground,
        xy,
        0.0,
        yaw,
    )
}

fn placements_of<'a>(sliced: &'a Sliced, r: &Reference) -> Vec<&'a Placement> {
    sliced
        .placements
        .iter()
        .filter(|p| p.reference == r.form.to_string())
        .collect()
}

/// A piece as drawn: its anchor hex and its opaque pixels' positions.
struct Drawn {
    hex: i32,
    rect: [i64; 4],
    pixels: Vec<[i64; 2]>,
}

fn drawn_pieces(sliced: &Sliced, r: &Reference) -> Vec<Drawn> {
    placements_of(sliced, r)
        .into_iter()
        .map(|p| {
            let a = &sliced.art[p.art];
            let [left, top] = frame_origin(a, p);
            let mut pixels = Vec::new();
            for y in 0..i64::from(a.height) {
                for x in 0..i64::from(a.width) {
                    if a.pixels[(y * i64::from(a.width) + x) as usize] != 0 {
                        pixels.push([left + x, top + y]);
                    }
                }
            }
            Drawn {
                hex: p.hex,
                rect: [
                    left,
                    top,
                    left + i64::from(a.width) - 1,
                    top + i64::from(a.height) - 1,
                ],
                pixels,
            }
        })
        .collect()
}

/// The owner hex of every drawn pixel of a reference, from its render
/// (independently of the cut): screen position to the hex its surface
/// stands over.
fn owner_map(
    assets: &TestAssets,
    r: &Reference,
    ground: &GroundSampler,
    config: &SlicerConfig,
) -> BTreeMap<[i64; 2], i32> {
    let (canvas, indexed, g, offset) = expected(assets, r, ground, config);
    let owners = strips::owners(&canvas, g, &projection());
    let w = canvas.width as usize;
    let mut map = BTreeMap::new();
    for (i, &o) in owners.iter().enumerate() {
        if o < 0 || indexed[i] == 0 {
            continue;
        }
        map.insert(
            [
                g[0] + i64::from(offset[0]) + i64::from(canvas.origin[0]) + (i % w) as i64,
                g[1] + i64::from(offset[1]) + i64::from(canvas.origin[1]) + (i / w) as i64,
            ],
            o,
        );
    }
    map
}

/// Hexes no walker may stand on: blocked, or holding a blocking placement.
fn unwalkable(sliced: &Sliced) -> BTreeSet<i32> {
    sliced
        .blocked_hexes
        .iter()
        .copied()
        .chain(
            sliced
                .placements
                .iter()
                .filter(|p| p.blocking)
                .map(|p| p.hex),
        )
        .collect()
}

#[test]
fn a_landmark_stays_upright_costs_few_entries_and_recomposes() {
    let assets = TestAssets::standard();
    let ground = flat();
    let config = landmark_config();
    let facts = MeshFacts::of(&assets.mesh("tower").unwrap());
    for yaw in [0.0, 0.5] {
        let r = tower(1, &ground, [140.5, -210.25], yaw);
        assert_eq!(classify(&r, Some(&facts), 140.0, &config), Class::Landmark);
        let sliced = run(
            &assets,
            std::slice::from_ref(&r),
            &ground,
            &big_budget(),
            &config,
        )
        .unwrap();
        let mine = placements_of(&sliced, &r);
        assert!(mine.len() >= 2, "taller than one piece");
        assert!(mine.len() <= config.landmark_max_pieces);
        assert!(
            mine.iter()
                .all(|p| p.role == Role::Scenery && p.run.is_none())
        );
        assert!(
            sliced
                .art
                .iter()
                .all(|a| a.category == ArtCategory::Scenery)
        );
        assert!(sliced.overlays.is_empty(), "not painted on the ground");
        let report = &sliced.report;
        assert_eq!(report.classes[&ClassKind::Landmark].references, 1);
        assert_eq!(report.landmarks.references, 1);
        assert_eq!(report.landmarks.pieces, mine.len());
        assert!(report.landmarks.tiles >= 2 && report.landmarks.walkers > 0);
        assert_eq!(report.category(ArtCategory::Walls).used, 0);
        assert!(report.to_string().contains("pieces (scenery list)"));

        // The pieces recompose to the render, pixel for pixel.
        let (canvas, indexed, g, offset) = expected(&assets, &r, &ground, &config);
        let want = expected_screen(&canvas, &indexed, g, offset);
        let got = composed(&sliced, &r, &want);
        assert!(want.pixels.iter().any(|v| *v != 0));
        assert!(want.pixels == got.pixels, "yaw {yaw}: the pieces moved");

        // Each piece is anchored within reach of its own far edges.
        let limits = Limits::of(&config);
        for piece in drawn_pieces(&sliced, &r) {
            let w = limits.window(piece.rect);
            let c = hex::center(piece.hex).unwrap();
            assert!((w[0]..=w[2]).contains(&c[0]) && (w[1]..=w[3]).contains(&c[1]));
        }
    }
}

/// Every camera hex from which `pieces` shows a pixel in the view, with
/// the pieces it shows.
fn views_showing(pieces: &[Drawn]) -> Vec<(i32, Vec<usize>)> {
    let mut all = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for p in pieces {
        all = [
            all[0].min(p.rect[0]),
            all[1].min(p.rect[1]),
            all[2].max(p.rect[2]),
            all[3].max(p.rect[3]),
        ];
    }
    let mut out = Vec::new();
    for camera in 0..40_000 {
        let v = view_origin(camera).unwrap();
        let view = [v[0], v[1], v[0] + VIEW[0] - 1, v[1] + VIEW[1] - 1];
        if all[0] > view[2] || all[2] < view[0] || all[1] > view[3] || all[3] < view[1] {
            continue;
        }
        let shown: Vec<usize> = pieces
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.pixels.iter().any(|q| {
                    (view[0]..=view[2]).contains(&q[0]) && (view[1]..=view[3]).contains(&q[1])
                })
            })
            .map(|(i, _)| i)
            .collect();
        if !shown.is_empty() {
            out.push((camera, shown));
        }
    }
    out
}

#[test]
fn every_piece_with_a_pixel_in_the_engines_view_is_drawn() {
    let assets = plant_assets();
    let ground = flat();
    let config = landmark_config();
    for (xy, yaw) in [([0.0, 0.0], 0.0), ([350.0, -490.0], 0.4)] {
        let r = plant(1, &ground, xy, yaw);
        let sliced = run(
            &assets,
            std::slice::from_ref(&r),
            &ground,
            &big_budget(),
            &config,
        )
        .unwrap();
        let pieces = drawn_pieces(&sliced, &r);
        let all_w = pieces.iter().map(|p| p.rect[2]).max().unwrap()
            - pieces.iter().map(|p| p.rect[0]).min().unwrap();
        let all_h = pieces.iter().map(|p| p.rect[3]).max().unwrap()
            - pieces.iter().map(|p| p.rect[1]).min().unwrap();
        assert!(
            all_w > VIEW[0] && all_h > VIEW[1],
            "larger than the view: {all_w} x {all_h}"
        );
        assert!(pieces.len() > 1);
        let views = views_showing(&pieces);
        assert!(views.len() > 1_000);
        for (camera, shown) in &views {
            let visited = visited_hexes(*camera);
            for &i in shown {
                assert!(
                    visited.contains(&pieces[i].hex),
                    "camera {camera}: piece at hex {} shows but is not drawn",
                    pieces[i].hex
                );
            }
        }

        // The rule bites: the whole landmark as one piece anchored at the
        // lowest hex owning a pixel of it (the first pass's band anchor)
        // shows from views that never draw it.
        let owners = owner_map(&assets, &r, &ground, &config);
        let lowest = owners.values().copied().min().unwrap();
        let pixels: Vec<[i64; 2]> = owners.keys().copied().collect();
        let whole = Drawn {
            hex: lowest,
            rect: [
                pixels.iter().map(|q| q[0]).min().unwrap(),
                pixels.iter().map(|q| q[1]).min().unwrap(),
                pixels.iter().map(|q| q[0]).max().unwrap(),
                pixels.iter().map(|q| q[1]).max().unwrap(),
            ],
            pixels,
        };
        let missed = views_showing(std::slice::from_ref(&whole))
            .iter()
            .filter(|(camera, _)| !visited_hexes(*camera).contains(&lowest))
            .count();
        assert!(missed > 100, "{missed}");
    }
}

/// Checks the sorting promise on a run, independently of the cut: no
/// walker is drawn over pixels that stand in front of it, and the hidden
/// and walker tallies are what the report says.
fn check_sorting(
    assets: &TestAssets,
    r: &Reference,
    ground: &GroundSampler,
    config: &SlicerConfig,
    sliced: &Sliced,
) {
    let limits = Limits::of(config);
    let owners = owner_map(assets, r, ground, config);
    let pieces = drawn_pieces(sliced, r);
    let mut all = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for p in &pieces {
        all = [
            all[0].min(p.rect[0]),
            all[1].min(p.rect[1]),
            all[2].max(p.rect[2]),
            all[3].max(p.rect[3]),
        ];
    }
    // Dense map: piece index + 1 and owner of each drawn pixel.
    let width = (all[2] - all[0] + 1) as usize;
    let mut at = vec![(0usize, 0i32); width * (all[3] - all[1] + 1) as usize];
    for (i, p) in pieces.iter().enumerate() {
        for q in &p.pixels {
            let owner = owners[q];
            at[(q[1] - all[1]) as usize * width + (q[0] - all[0]) as usize] = (i + 1, owner);
        }
    }
    let blocked = unwalkable(sliced);
    let walker = |h: i32| {
        !blocked.contains(&h)
            && (config.open_hexes.contains(&h)
                || config
                    .landmark_walkers
                    .as_ref()
                    .is_none_or(|w| w.contains(&h)))
    };
    let [bw, bh] = limits.walker;
    let mut walkers = BTreeSet::new();
    let mut hidden = BTreeSet::new();
    let centres = [all[0] - bw, all[1], all[2] + bw, all[3] + bh];
    for h in hexes_in_rect(centres) {
        if !walker(h) {
            continue;
        }
        let b = limits.walker_box(hex::center(h).unwrap());
        let mut counts: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        for y in b[1].max(all[1])..=b[3].min(all[3]) {
            for x in b[0].max(all[0])..=b[2].min(all[2]) {
                let (piece, owner) = at[(y - all[1]) as usize * width + (x - all[0]) as usize];
                if piece == 0 {
                    continue;
                }
                let c = counts.entry(piece - 1).or_default();
                if owner > h {
                    c.0 += 1;
                } else {
                    c.1 += 1;
                }
            }
        }
        if counts.is_empty() {
            continue;
        }
        walkers.insert(h);
        for (piece, (front, back)) in counts {
            let anchor = pieces[piece].hex;
            assert!(
                !(anchor <= h && front > limits.tolerance),
                "a walker on {h} is drawn over {front} pixels in front of it (piece at {anchor})"
            );
            if anchor > h && back > limits.tolerance {
                hidden.insert(h);
            }
        }
    }
    let stats = &sliced.report.landmarks;
    assert_eq!(walkers.len(), stats.walkers);
    assert_eq!(hidden.len(), stats.hidden_walkers);
}

#[test]
fn no_walker_is_drawn_over_a_piece_that_stands_in_front_of_it() {
    let assets = plant_assets();
    let ground = flat();
    for yaw in [0.0, 0.4] {
        let r = plant(1, &ground, [0.0, 0.0], yaw);
        let config = landmark_config();
        let sliced = run(
            &assets,
            std::slice::from_ref(&r),
            &ground,
            &big_budget(),
            &config,
        )
        .unwrap();
        check_sorting(&assets, &r, &ground, &config, &sliced);
        let stats = &sliced.report.landmarks;
        assert!(stats.walkers > 100, "{stats:?}");
        // With pieces to spare nobody is hidden wrongly or sealed.
        assert_eq!(
            (
                stats.hidden_walkers,
                stats.sealed_hexes,
                stats.unresolved_pieces
            ),
            (0, 0, 0),
            "{stats:?}"
        );
        assert!(
            stats.sorting_splits + stats.visibility_splits > 0,
            "{stats:?}"
        );
        assert!(sliced.report.landmarks.pieces < config.landmark_max_pieces);

        // Capped at the visibility cut, the promise still holds: the
        // walkers that would be drawn over are sealed instead.
        let capped = SlicerConfig {
            landmark_max_pieces: stats.tiles,
            ..landmark_config()
        };
        let sliced = run(
            &assets,
            std::slice::from_ref(&r),
            &ground,
            &big_budget(),
            &capped,
        )
        .unwrap();
        check_sorting(&assets, &r, &ground, &capped, &sliced);
        let stats = &sliced.report.landmarks;
        assert_eq!(stats.sorting_splits + stats.visibility_splits, 0);
        assert!(
            stats.hidden_walkers + stats.sealed_hexes > 0,
            "the cap costs something: {stats:?}"
        );
    }
}

#[test]
fn the_footprint_blocks_and_open_hexes_stay_open() {
    let assets = plant_assets();
    let ground = flat();
    let r = plant(1, &ground, [0.0, 0.0], 0.0);
    let config = landmark_config();
    let sliced = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    let blocked = unwalkable(&sliced);
    // Every hex over which a surface stands well above a step owns a
    // blocked hex: no walker under a roof, inside a wall or on a ledge.
    let owners = owner_map(&assets, &r, &ground, &config);
    let mut high = BTreeSet::new();
    for (q, &o) in &owners {
        let c = hex::center(o).unwrap();
        if c[1] - q[1] > 24 {
            high.insert(o);
        }
    }
    assert!(high.len() > 200);
    let open: Vec<i32> = high
        .iter()
        .copied()
        .filter(|h| !blocked.contains(h))
        .collect();
    assert!(open.is_empty(), "{} high hexes open: {open:?}", open.len());
    // The courtyard is walkable.
    let courtyard = hex::pick({
        let c = hex::center(*high.iter().next().unwrap()).unwrap();
        [c[0] as f64, c[1] as f64]
    });
    assert!(courtyard.is_some());

    // An open hex inside the footprint stays open, and no piece blocks it.
    let inside = *high.iter().nth(high.len() / 2).unwrap();
    let config = SlicerConfig {
        open_hexes: BTreeSet::from([inside]),
        ..landmark_config()
    };
    let opened = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    assert!(!unwalkable(&opened).contains(&inside));
    check_sorting(&assets, &r, &ground, &config, &opened);
}

#[test]
fn known_walkable_hexes_spare_pieces() {
    let assets = plant_assets();
    let ground = flat();
    let r = plant(1, &ground, [0.0, 0.0], 0.0);
    let all = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &landmark_config(),
    )
    .unwrap();
    // A map whose builder can make nothing walkable near the plant: no
    // walker, no sorting cut.
    let config = SlicerConfig {
        landmark_walkers: Some(BTreeSet::new()),
        ..landmark_config()
    };
    let none = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    let (a, n) = (&all.report.landmarks, &none.report.landmarks);
    assert_eq!(n.walkers, 0);
    assert_eq!((n.sorting_splits, n.visibility_splits), (0, 0));
    assert_eq!(n.pieces, n.tiles.min(n.pieces));
    assert!(n.pieces < a.pieces, "{n:?} against {a:?}");
    check_sorting(&assets, &r, &ground, &config, &none);
}

#[test]
fn copies_share_pieces_and_open_hexes_stay_open() {
    let assets = TestAssets::standard();
    let ground = flat();
    let a = tower(1, &ground, [0.0, 0.0], 0.0);
    // Three hexes east and two rows south: a whole-hex translation.
    let b = tower(2, &ground, [3.0 * 140.0, 2.0 * -70.0], 0.0);
    let one = run(
        &assets,
        std::slice::from_ref(&a),
        &ground,
        &big_budget(),
        &landmark_config(),
    )
    .unwrap();
    let two = run(
        &assets,
        &[a.clone(), b],
        &ground,
        &big_budget(),
        &landmark_config(),
    )
    .unwrap();
    assert_eq!(one.art.len(), two.art.len(), "the copy adds no art");

    let open = one.blocked_hexes[one.blocked_hexes.len() / 2];
    let config = SlicerConfig {
        open_hexes: BTreeSet::from([open]),
        ..landmark_config()
    };
    let opened = run(
        &assets,
        std::slice::from_ref(&a),
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    assert!(!opened.blocked_hexes.contains(&open));
    assert!(
        !opened
            .placements
            .iter()
            .any(|p| p.hex == open && p.blocking)
    );
}

#[test]
fn a_tall_landmark_renders_in_windows_and_one_too_large_falls_back_to_relief() {
    let assets = TestAssets::standard();
    let ground = sloped();
    let r = tower(1, &ground, [-300.0, 450.0], 0.3);
    // Small renderer windows: the tower is rendered in many pieces.
    let windows = SlicerConfig {
        render_tile: 96,
        ..landmark_config()
    };
    let sliced = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &windows,
    )
    .unwrap();
    let (canvas, indexed, g, offset) = expected(&assets, &r, &ground, &landmark_config());
    let want = expected_screen(&canvas, &indexed, g, offset);
    assert!(canvas.height > 4 * windows.render_tile);
    assert!(composed(&sliced, &r, &want).pixels == want.pixels);

    // Over the canvas limit: reported, baked as relief, not fatal.
    let tiny = SlicerConfig {
        max_canvas_pixels: 10_000,
        ..landmark_config()
    };
    let sliced = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &tiny,
    )
    .unwrap();
    assert!(placements_of(&sliced, &r).is_empty());
    assert!(sliced.skipped.is_empty());
    assert!(!sliced.overlays.is_empty() && !sliced.blocked_hexes.is_empty());
    let report = &sliced.report;
    assert_eq!(report.classes[&ClassKind::Landmark].references, 0);
    assert_eq!(report.classes[&ClassKind::GroundRelief].references, 1);
    assert_eq!(report.landmarks.fallbacks.len(), 1);
    assert_eq!(
        report.landmarks.fallbacks[0].reason,
        SkipReason::TooLarge.to_string()
    );
    assert!(report.to_string().contains("relief instead"));
}

#[test]
fn the_estimate_counts_landmark_pieces() {
    let assets = plant_assets();
    let ground = flat();
    let refs = [
        tower(1, &ground, [0.0, 0.0], 0.0),
        plant(2, &ground, [2800.0, 1400.0], 0.0),
    ];
    let config = landmark_config();
    let real = run(&assets, &refs, &ground, &big_budget(), &config)
        .unwrap()
        .report;
    let guess = crate::estimate_with(
        &assets,
        &refs,
        &window(),
        &ground,
        &projection(),
        &big_budget(),
        &config,
    )
    .unwrap();
    for kind in ClassKind::ALL {
        assert_eq!(
            guess.classes[&kind].references,
            real.classes[&kind].references
        );
    }
    // The estimate counts the visibility cut: about the real tiles.
    let (tiles, g) = (
        real.landmarks.tiles,
        guess.category(ArtCategory::Scenery).used,
    );
    assert!(
        g >= tiles / 2 && g <= 2 * tiles,
        "estimate {g} vs {tiles} tiles"
    );
    assert!(g <= real.category(ArtCategory::Scenery).used);
}

#[test]
fn trigger_volumes_are_skipped_in_a_run_and_listed_by_their_base() {
    let assets = TestAssets::standard();
    let ground = flat();
    let mut volume = reference(
        1,
        "ACTI",
        "meshes\\test\\triggerbox.nif",
        "slab",
        &ground,
        [0.0, 0.0],
        0.0,
        0.0,
    );
    volume.trigger_volume = true;
    // A primitive whose base has no model at all: listed by the base.
    let mut bare = volume.clone();
    bare.form = mir::FormKey::new("test.esm", 2);
    bare.model = None;
    bare.mesh = None;
    bare.base_editor_id = Some("TrigTest".into());
    let sliced = run(
        &assets,
        &[volume, bare],
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(sliced.placements.is_empty() && sliced.overlays.is_empty());
    assert!(
        sliced
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::TriggerVolume)
    );
    assert_eq!(sliced.report.skips["trigger volume"], 2);
    let models: Vec<Option<&str>> = sliced.skipped.iter().map(|s| s.model.as_deref()).collect();
    assert_eq!(
        models,
        [Some("meshes\\test\\triggerbox.nif"), Some("base TrigTest")]
    );
}
