//! Sprites and strips land where they were rendered.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::hex::Plane;
use crate::render::{Canvas, render_canonical};
use crate::slice::{RenderKey, standing};
use crate::{ArtCategory, Role, SkipReason, estimate_with, strips};

/// The canonical render of a reference, its indexed pixels and where it
/// stands: ground point and offset to the drawn origin.
pub(super) fn expected(
    assets: &TestAssets,
    r: &Reference,
    ground: &GroundSampler,
    config: &SlicerConfig,
) -> (Canvas, Vec<u8>, [i64; 2], [i32; 2]) {
    let key = RenderKey::of(r, config).unwrap();
    let mesh = assets.mesh(&key.mesh).unwrap();
    let canvas = render_canonical(
        &mut renderer(),
        &projection(),
        &mesh,
        key.matrix(config),
        assets.textures(),
        config.render_tile,
        config.max_canvas_pixels,
    )
    .unwrap()
    .unwrap();
    let indexed = quantize::quantize(
        &canvas.rgba,
        canvas.width,
        canvas.height,
        &palette_map(),
        quantize::Dither::None,
    )
    .unwrap();
    let (g, offset, _) = standing(&projection(), &window(), ground, r).unwrap();
    (canvas, indexed, g, offset)
}

/// The expected image of one reference on a screen big enough for it.
pub(super) fn expected_screen(
    canvas: &Canvas,
    indexed: &[u8],
    g: [i64; 2],
    offset: [i32; 2],
) -> Screen {
    let left = g[0] + i64::from(offset[0]) + i64::from(canvas.origin[0]);
    let top = g[1] + i64::from(offset[1]) + i64::from(canvas.origin[1]);
    let mut screen = Screen::new(
        [left - 64, top - 64],
        canvas.width as usize + 128,
        canvas.height as usize + 128,
    );
    let w = canvas.width as usize;
    for (i, &v) in indexed.iter().enumerate() {
        if v != 0 {
            screen.put(left + (i % w) as i64, top + (i / w) as i64, v);
        }
    }
    screen
}

pub(super) fn composed(sliced: &Sliced, reference: &Reference, like: &Screen) -> Screen {
    let mut screen = Screen::new(like.origin, like.width, like.height);
    let mine: Vec<&Placement> = sliced
        .placements
        .iter()
        .filter(|p| p.reference == reference.form.to_string())
        .collect();
    screen.draw(&sliced.art, &mine);
    screen
}

pub(super) fn house(id: u32, ground: &GroundSampler, xy: [f32; 2]) -> Reference {
    reference(
        id,
        "STAT",
        "meshes\\test\\house.nif",
        "house",
        ground,
        xy,
        0.0,
        0.0,
    )
}

pub(super) fn crate_ref(id: u32, ground: &GroundSampler, xy: [f32; 2], lift: f32) -> Reference {
    reference(
        id,
        "STAT",
        "meshes\\test\\crate.nif",
        "crate",
        ground,
        xy,
        lift,
        0.0,
    )
}

#[test]
fn a_one_hex_prop_lands_on_its_rendered_pixels() {
    let assets = TestAssets::standard();
    let ground = sloped();
    let config = SlicerConfig::default();
    let r = crate_ref(1, &ground, [123.4, -57.8], 30.0);
    let sliced = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    assert_eq!(sliced.art.len(), 1);
    assert_eq!(sliced.placements.len(), 1);
    let p = &sliced.placements[0];
    assert_eq!(p.role, Role::Scenery);
    assert_eq!(sliced.art[0].category, ArtCategory::Scenery);
    let (canvas, indexed, g, offset) = expected(&assets, &r, &ground, &config);
    // The hex is the one under the projected ground point.
    assert_eq!(Some(p.hex), crate::hex::pick([g[0] as f64, g[1] as f64]));
    let want = expected_screen(&canvas, &indexed, g, offset);
    let got = composed(&sliced, &r, &want);
    assert!(want.pixels.iter().any(|v| *v != 0));
    assert!(want.pixels == got.pixels, "the sprite moved");
}

#[test]
fn sprites_share_art_wherever_they_stand_in_a_hex_and_at_any_height() {
    let assets = TestAssets::standard();
    let ground = sloped();
    let refs = [
        crate_ref(1, &ground, [0.0, 0.0], 0.0),
        crate_ref(2, &ground, [31.0, 17.5], 0.0),
        crate_ref(3, &ground, [-405.25, 911.0], 105.0),
    ];
    let sliced = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert_eq!(sliced.art.len(), 1);
    assert_eq!(sliced.placements.len(), 3);
    let by_ref: BTreeMap<&str, &Placement> = sliced
        .placements
        .iter()
        .map(|p| (p.reference.as_str(), p))
        .collect();
    let lifted = by_ref[refs[2].form.to_string().as_str()];
    let (_, offset, _) = standing(&projection(), &window(), &ground, &refs[2]).unwrap();
    assert_eq!(offset, [0, -48], "half a storey up is 48 pixels");
    // The lift is part of the displacement: the sprite's own offset from
    // the hex centre stays within a hex.
    assert!(lifted.displacement[1] <= -48 + 16 && lifted.displacement[1] >= -48 - 16);
}

/// Two hex columns in source units: `(48, -12)` on screen. (A single
/// column, `(32, 0)` or `(16, -12)` depending on its parity, changes how
/// the hex rows pair up, so interior runs and tie-breaks may differ.)
const HEX_STEP: [f32; 2] = [140.0, 0.0];
/// One row: `(16, 12)` on screen.
const ROW_STEP: [f32; 2] = [0.0, -70.0];

#[test]
fn whole_hex_copies_share_their_strips_and_differ_only_in_hex() {
    let assets = TestAssets::standard();
    let ground = flat();
    let config = SlicerConfig::default();
    let a = house(1, &ground, [0.0, 0.0]);
    let b = house(
        2,
        &ground,
        [
            3.0 * HEX_STEP[0] + 2.0 * ROW_STEP[0],
            3.0 * HEX_STEP[1] + 2.0 * ROW_STEP[1],
        ],
    );
    let one = run(
        &assets,
        std::slice::from_ref(&a),
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    let two = run(
        &assets,
        &[a.clone(), b.clone()],
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    assert!(one.art.len() > 4, "the house is cut into strips");
    assert_eq!(two.art.len(), one.art.len(), "the copy adds no art");
    assert_eq!(two.placements.len(), 2 * one.placements.len());
    let shift = [3 * 48 + 2 * 16, 3 * -12 + 2 * 12];
    let of = |reference: &Reference| -> BTreeSet<(usize, [i64; 2], [i32; 2])> {
        two.placements
            .iter()
            .filter(|p| p.reference == reference.form.to_string())
            .map(|p| {
                let c = crate::hex::center(p.hex).unwrap();
                (p.art, c, p.displacement)
            })
            .collect()
    };
    let moved: BTreeSet<_> = of(&a)
        .into_iter()
        .map(|(art, c, d)| (art, [c[0] + shift[0], c[1] + shift[1]], d))
        .collect();
    assert_eq!(moved, of(&b));

    // Without merging, pixel ownership alone is invariant under a single
    // column step too.
    let plain = SlicerConfig {
        min_strip_pixels: 0,
        merge_interior_runs: false,
        max_perimeter_run: 1,
        ..SlicerConfig::default()
    };
    let c = house(3, &ground, [70.0, -35.0]);
    let one = run(
        &assets,
        std::slice::from_ref(&a),
        &ground,
        &big_budget(),
        &plain,
    )
    .unwrap();
    let two = run(&assets, &[a.clone(), c], &ground, &big_budget(), &plain).unwrap();
    assert_eq!(two.art.len(), one.art.len());
}

#[test]
fn recomposed_strips_reproduce_the_render_exactly() {
    let assets = TestAssets::standard();
    for (ground, yaw) in [(flat(), 0.0), (sloped(), 0.7), (flat(), 2.0)] {
        for config in [
            SlicerConfig::default(),
            SlicerConfig {
                min_strip_pixels: 0,
                merge_interior_runs: false,
                max_perimeter_run: 1,
                ..SlicerConfig::default()
            },
        ] {
            let r = reference(
                1,
                "STAT",
                "meshes\\test\\house.nif",
                "house",
                &ground,
                [210.5, -333.3],
                12.0,
                yaw,
            );
            let sliced = run(
                &assets,
                std::slice::from_ref(&r),
                &ground,
                &big_budget(),
                &config,
            )
            .unwrap();
            assert!(sliced.placements.iter().all(|p| p.role == Role::WallStrip));
            assert!(sliced.art.iter().all(|a| a.category == ArtCategory::Walls));
            let (canvas, indexed, g, offset) = expected(&assets, &r, &ground, &config);
            let want = expected_screen(&canvas, &indexed, g, offset);
            let got = composed(&sliced, &r, &want);
            let differing = want
                .pixels
                .iter()
                .zip(&got.pixels)
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(differing, 0, "yaw {yaw}: recomposition differs");
            assert_eq!(sliced.report.strips.dropped_pixels, 0);
        }
    }
}

#[test]
fn every_strip_pixel_stands_over_its_own_hex() {
    let assets = TestAssets::standard();
    let ground = flat();
    let config = SlicerConfig {
        min_strip_pixels: 0,
        merge_interior_runs: false,
        max_perimeter_run: 1,
        ..SlicerConfig::default()
    };
    let r = reference(
        1,
        "STAT",
        "meshes\\test\\house.nif",
        "house",
        &ground,
        [0.0, 0.0],
        0.0,
        0.4,
    );
    let sliced = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    let (canvas, _, g, offset) = expected(&assets, &r, &ground, &config);
    let owners = strips::owners(&canvas, g, &projection());
    let left = g[0] + i64::from(offset[0]) + i64::from(canvas.origin[0]);
    let top = g[1] + i64::from(offset[1]) + i64::from(canvas.origin[1]);
    let mut checked = 0;
    for p in &sliced.placements {
        let a = &sliced.art[p.art];
        let [x0, y0] = frame_origin(a, p);
        for y in 0..a.height as i64 {
            for x in 0..a.width as i64 {
                if a.pixels[(y * a.width as i64 + x) as usize] == 0 {
                    continue;
                }
                let (u, v) = (x0 + x - left, y0 + y - top);
                let index = (v * canvas.width as i64 + u) as usize;
                assert_eq!(owners[index], p.hex);
                checked += 1;
            }
        }
    }
    assert_eq!(checked, canvas.rgba.chunks(4).filter(|p| p[3] != 0).count());
}

/// Screen x-interval, hexes covered and their ground depths, per strip.
struct Drawn {
    hexes: (i32, i32),
    x: (i64, i64),
    depth: (f32, f32),
}

fn drawn(sliced: &Sliced, plane: &Plane, projection: &Projection) -> Vec<Drawn> {
    sliced
        .placements
        .iter()
        .map(|p| {
            let a = &sliced.art[p.art];
            let [x0, _] = frame_origin(a, p);
            let hexes = p.run.unwrap_or((p.hex, p.hex));
            let depths: Vec<f32> = (hexes.0..=hexes.1)
                .map(|h| {
                    let st = plane.hex_tile(h).unwrap();
                    projection.depth([st[0] as f32, st[1] as f32, 0.0])
                })
                .collect();
            Drawn {
                hexes,
                x: (x0, x0 + i64::from(a.width)),
                depth: (
                    depths.iter().copied().fold(f32::MAX, f32::min),
                    depths.iter().copied().fold(f32::MIN, f32::max),
                ),
            }
        })
        .collect()
}

#[test]
fn nearer_strips_are_drawn_later() {
    let assets = TestAssets::standard();
    let ground = flat();
    let projection = projection();
    let plane = Plane::new(&projection, window().x_west, window().y_north).unwrap();
    for (yaw, config) in [0.0f32, 0.5, 1.2, 2.6].into_iter().flat_map(|yaw| {
        [
            (
                yaw,
                SlicerConfig {
                    max_perimeter_run: 1,
                    ..SlicerConfig::default()
                },
            ),
            (yaw, SlicerConfig::default()),
        ]
    }) {
        let r = reference(
            1,
            "STAT",
            "meshes\\test\\house.nif",
            "house",
            &ground,
            [0.0, 0.0],
            0.0,
            yaw,
        );
        let sliced = run(
            &assets,
            std::slice::from_ref(&r),
            &ground,
            &big_budget(),
            &config,
        )
        .unwrap();
        // The engine's order: ascending hex number.
        assert!(sliced.placements.windows(2).all(|w| w[0].hex <= w[1].hex));
        let strips = drawn(&sliced, &plane, &projection);
        let mut pairs = 0;
        for a in &strips {
            for b in &strips {
                let overlap = a.x.0 < b.x.1 && b.x.0 < a.x.1;
                // `a` wholly nearer than `b`: every hex of `a` is drawn
                // after every hex of `b`.
                if overlap && a.depth.1 < b.depth.0 - 1e-4 {
                    assert!(
                        a.hexes.0 > b.hexes.1,
                        "yaw {yaw}: {:?} is nearer than {:?} but drawn first",
                        a.hexes,
                        b.hexes
                    );
                    pairs += 1;
                }
            }
        }
        assert!(pairs > 10, "yaw {yaw}: only {pairs} overlapping pairs");
    }
}

#[test]
fn tiny_strips_merge_into_a_neighbour() {
    let assets = TestAssets::standard();
    let ground = flat();
    let r = reference(
        1,
        "STAT",
        "meshes\\test\\house.nif",
        "house",
        &ground,
        [0.0, 0.0],
        0.0,
        0.3,
    );
    let loose = SlicerConfig {
        min_strip_pixels: 0,
        merge_interior_runs: false,
        max_perimeter_run: 1,
        ..SlicerConfig::default()
    };
    let strict = SlicerConfig {
        min_strip_pixels: 300,
        ..loose.clone()
    };
    let a = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &loose,
    )
    .unwrap();
    let b = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &strict,
    )
    .unwrap();
    assert!(b.placements.len() < a.placements.len());
    assert!(b.report.strips.merged_tiny > 0);
    for p in &b.placements {
        let opaque = b.art[p.art].pixels.iter().filter(|v| **v != 0).count();
        assert!(opaque >= 300, "a strip of {opaque} pixels survived");
    }
    // Merging moves pixels between strips, never on screen.
    let (canvas, indexed, g, offset) = expected(&assets, &r, &ground, &strict);
    let want = expected_screen(&canvas, &indexed, g, offset);
    assert!(composed(&b, &r, &want).pixels == want.pixels);
}

#[test]
fn interior_runs_cut_the_roof_cost_and_block_their_hexes() {
    let assets = TestAssets::standard();
    let ground = flat();
    let r = house(1, &ground, [0.0, 0.0]);
    let per_hex = SlicerConfig {
        merge_interior_runs: false,
        ..SlicerConfig::default()
    };
    let a = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &per_hex,
    )
    .unwrap();
    let b = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(b.placements.len() < a.placements.len());
    assert!(b.report.strips.runs > 0);
    let runs: Vec<&Placement> = b.placements.iter().filter(|p| p.run.is_some()).collect();
    for p in &runs {
        let (first, last) = p.run.unwrap();
        assert!(first <= p.hex && p.hex <= last);
        assert_eq!(first / 200, last / 200, "a run stays in one row");
        for h in first..=last {
            if h != p.hex {
                assert!(b.blocked_hexes.binary_search(&h).is_ok());
            }
        }
    }
}

#[test]
fn open_hexes_stay_unblocked_and_out_of_runs() {
    let assets = TestAssets::standard();
    let ground = flat();
    let r = house(1, &ground, [0.0, 0.0]);
    let closed = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    // Open the middle of the longest run: a porch under the roof.
    let (first, last) = closed
        .placements
        .iter()
        .filter_map(|p| p.run)
        .max_by_key(|(first, last)| last - first)
        .expect("the house has an interior run");
    assert!(last - first >= 2, "a run with a middle");
    let porch = (first + last) / 2;
    let open = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &SlicerConfig {
            open_hexes: [porch].into(),
            ..SlicerConfig::default()
        },
    )
    .unwrap();
    assert!(open.blocked_hexes.binary_search(&porch).is_err());
    for p in &open.placements {
        if let Some((a, b)) = p.run {
            assert!(!(a..=b).contains(&porch), "no run crosses the open hex");
        }
        if p.hex == porch {
            assert!(!p.blocking, "a strip on the open hex does not block");
        }
    }
    // The porch keeps its pixels, in a strip of its own.
    assert!(open.placements.iter().any(|p| p.hex == porch));
    assert!(open.placements.len() > closed.placements.len());
}

/// A porch: a plank slab 40 units thick, a roof plate 260 units up over
/// all of it, and a post at each end of the front edge.
fn porch_mesh() -> StaticMesh {
    let part = |size: [f32; 3], by: [f32; 3], texture: &str| {
        let mut part = mesh(&nif::fixture::box_mesh(size, texture));
        for primitive in &mut part.primitives {
            for p in &mut primitive.positions {
                *p = [p[0] + by[0], p[1] + by[1], p[2] + by[2]];
            }
        }
        part.primitives
    };
    let mut primitives = part([600.0, 420.0, 40.0], [0.0, 0.0, 0.0], ROOF);
    primitives.extend(part([600.0, 420.0, 12.0], [0.0, 0.0, 260.0], WALL));
    primitives.extend(part([24.0, 24.0, 260.0], [-280.0, -190.0, 0.0], WALL));
    primitives.extend(part([24.0, 24.0, 260.0], [280.0, -190.0, 0.0], WALL));
    StaticMesh {
        primitives,
        collision: None,
        skipped_blocks: BTreeMap::new(),
    }
}

#[test]
fn walking_surfaces_by_an_open_hex_become_floor_pieces() {
    let assets = TestAssets::new().with("porch", porch_mesh());
    let ground = flat();
    let r = reference(
        1,
        "STAT",
        "meshes\\test\\porch.nif",
        "porch",
        &ground,
        [35.0, -70.0],
        0.0,
        0.0,
    );
    let closed = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(closed.placements.iter().all(|p| p.role == Role::WallStrip));
    assert_eq!(closed.report.strips.floor_pieces, 0);

    // Open every hex the porch stands over, as a navmesh on its planks
    // would.
    let open_hexes: BTreeSet<i32> = closed
        .placements
        .iter()
        .flat_map(|p| match p.run {
            Some((first, last)) => (first..=last).collect::<Vec<_>>(),
            None => vec![p.hex],
        })
        .collect();
    let config = SlicerConfig {
        open_hexes,
        ..SlicerConfig::default()
    };
    let open = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &config,
    )
    .unwrap();
    let floors: Vec<&Placement> = open
        .placements
        .iter()
        .filter(|p| p.role == Role::FloorPiece)
        .collect();
    let strips: Vec<&Placement> = open
        .placements
        .iter()
        .filter(|p| p.role == Role::WallStrip)
        .collect();
    assert!(!floors.is_empty() && !strips.is_empty());
    assert_eq!(open.report.strips.floor_pieces, floors.len());
    assert_eq!(floors.len() + strips.len(), open.placements.len());
    for p in &floors {
        assert!(!p.blocking && p.run.is_none());
        assert_eq!(open.art[p.art].category, ArtCategory::Scenery);
    }
    assert!(
        strips
            .iter()
            .all(|p| open.art[p.art].category == ArtCategory::Walls)
    );

    // Drawn the engine's way (flat pieces first, then the strips in hex
    // order), the porch looks as it was rendered, but for edge pixels
    // where a post and the planks share a pixel.
    let (canvas, indexed, g, offset) = expected(&assets, &r, &ground, &config);
    let want = expected_screen(&canvas, &indexed, g, offset);
    let mut got = Screen::new(want.origin, want.width, want.height);
    got.draw(&open.art, &floors);
    let floor_only = got.pixels.clone();
    got.draw(&open.art, &strips);
    let opaque = want.pixels.iter().filter(|v| **v != 0).count();
    let differing = want
        .pixels
        .iter()
        .zip(&got.pixels)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing * 50 < opaque,
        "{differing} of {opaque} pixels differ from the render"
    );

    // The floor pieces hold planks the strips never had: the ones under
    // the roof plate, which the nearest-surface render hides.
    let mut strips_only = Screen::new(want.origin, want.width, want.height);
    strips_only.draw(&open.art, &strips);
    let hidden = floor_only
        .iter()
        .zip(&strips_only.pixels)
        .filter(|(floor, strip)| **floor != 0 && **strip != 0)
        .count();
    assert!(hidden > 200, "{hidden} floor pixels lie under strip pixels");
    // And the planks left the strips: where a floor piece shows in the
    // finished picture, the strips are clear.
    let shown = floor_only
        .iter()
        .zip(&strips_only.pixels)
        .filter(|(floor, strip)| **floor != 0 && **strip == 0)
        .count();
    assert!(shown > 1000, "{shown} floor pixels show through");
}

#[test]
fn a_wall_along_a_row_is_cut_into_short_runs() {
    let assets = TestAssets::standard();
    let ground = flat();
    // East-west: along a hex row.
    let r = reference(
        1,
        "STAT",
        "meshes\\test\\fence.nif",
        "fence",
        &ground,
        [0.0, 0.0],
        0.0,
        0.0,
    );
    let single = SlicerConfig {
        max_perimeter_run: 1,
        ..SlicerConfig::default()
    };
    let a = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &single,
    )
    .unwrap();
    let b = run(
        &assets,
        std::slice::from_ref(&r),
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(a.placements.iter().all(|p| p.run.is_none()));
    assert!(b.placements.len() * 2 <= a.placements.len() + 1);
    for p in &b.placements {
        if let Some((first, last)) = p.run {
            assert!(last - first < 3 && first / 200 == last / 200);
        }
        assert!(p.blocking);
    }
    let (canvas, indexed, g, offset) = expected(&assets, &r, &ground, &SlicerConfig::default());
    let want = expected_screen(&canvas, &indexed, g, offset);
    assert!(composed(&b, &r, &want).pixels == want.pixels);
}

#[test]
fn a_tight_budget_names_the_list_and_its_largest_contributors() {
    let assets = TestAssets::standard();
    let ground = flat();
    let refs = [
        house(1, &ground, [0.0, 0.0]),
        crate_ref(2, &ground, [700.0, 0.0], 0.0),
    ];
    let budget = Budget {
        walls: 5,
        scenery: 10,
        items: 10,
    };
    let no_ladder = SlicerConfig {
        ladder: crate::BudgetLadder {
            enabled: false,
            ..crate::BudgetLadder::default()
        },
        ..SlicerConfig::default()
    };
    let error = run(&assets, &refs, &ground, &budget, &no_ladder).unwrap_err();
    let SliceError::FidBudget {
        category,
        cap,
        demand,
        top,
        report,
    } = &error
    else {
        panic!("expected a budget error, got {error}");
    };
    assert_eq!(*category, ArtCategory::Walls);
    assert_eq!(*cap, 5);
    assert!(*demand > 5);
    assert_eq!(top[0].model, "meshes\\test\\house.nif");
    assert_eq!(top[0].entries, *demand);
    let text = error.to_string();
    assert!(
        text.contains("walls") && text.contains("5 free") && text.contains(&demand.to_string())
    );
    assert!(text.contains("meshes\\test\\house.nif"));
    assert_eq!(report.category(ArtCategory::Scenery).used, 1);
    assert!(report.to_string().contains("OVER"));
}

#[test]
fn the_estimate_agrees_in_order_of_magnitude() {
    let assets = TestAssets::standard();
    let ground = flat();
    let refs = [
        house(1, &ground, [0.0, 0.0]),
        house(2, &ground, [1500.0, 300.0]),
        reference(
            3,
            "STAT",
            "meshes\\test\\fence.nif",
            "fence",
            &ground,
            [-900.0, 700.0],
            0.0,
            0.4,
        ),
        crate_ref(4, &ground, [700.0, 0.0], 0.0),
        reference(
            5,
            "STAT",
            "meshes\\test\\strip.nif",
            "road",
            &ground,
            [0.0, -900.0],
            0.0,
            0.0,
        ),
    ];
    let config = SlicerConfig::default();
    let real = run(&assets, &refs, &ground, &big_budget(), &config)
        .unwrap()
        .report;
    let guess = estimate_with(
        &assets,
        &refs,
        &window(),
        &ground,
        &projection(),
        &big_budget(),
        &config,
    )
    .unwrap();
    assert!(guess.estimated && !real.estimated);
    let (r, g) = (
        real.category(ArtCategory::Walls).used as f64,
        guess.category(ArtCategory::Walls).used as f64,
    );
    assert!(g / r > 0.25 && g / r < 4.0, "estimate {g} vs real {r}");
    assert_eq!(
        guess.category(ArtCategory::Scenery).used,
        real.category(ArtCategory::Scenery).used
    );
    for kind in crate::ClassKind::ALL {
        assert_eq!(
            guess.classes[&kind].references,
            real.classes[&kind].references
        );
    }
}

#[test]
fn missing_meshes_are_skipped_with_a_reason() {
    let assets = TestAssets::standard();
    let ground = flat();
    let mut gone = crate_ref(1, &ground, [0.0, 0.0], 0.0);
    gone.mesh = Some("nothing".to_owned());
    let mut none = crate_ref(2, &ground, [100.0, 0.0], 0.0);
    none.mesh = None;
    let fine = crate_ref(3, &ground, [200.0, 0.0], 0.0);
    let sliced = run(
        &assets,
        &[gone, none, fine],
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert_eq!(sliced.placements.len(), 1);
    assert_eq!(sliced.skipped.len(), 2);
    assert!(
        matches!(&sliced.skipped[0].reason, SkipReason::Unreadable(e) if e.contains("nothing"))
    );
    assert_eq!(sliced.skipped[1].reason, SkipReason::NoMesh);
    assert_eq!(sliced.report.skips["unreadable mesh"], 1);
}

#[test]
fn objects_off_the_map_are_skipped_or_clipped() {
    let assets = TestAssets::standard();
    let ground = flat();
    // Far outside the window.
    let far = crate_ref(1, &ground, [9000.0, 0.0], 0.0);
    // Straddling the west edge: some strips land, the rest is dropped.
    let edge = house(2, &ground, [window().x_west + 100.0, 0.0]);
    let sliced = run(
        &assets,
        &[far, edge.clone()],
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert_eq!(sliced.skipped.len(), 1);
    assert_eq!(sliced.skipped[0].reason, SkipReason::OutsideMap);
    assert!(
        sliced
            .placements
            .iter()
            .all(|p| p.reference == edge.form.to_string())
    );
    assert!(!sliced.placements.is_empty());
    assert!(sliced.report.strips.dropped_pixels > 0);
}

#[test]
fn enable_parents_are_followed() {
    let assets = TestAssets::standard();
    let ground = flat();
    let parent = crate_ref(1, &ground, [0.0, 0.0], 0.0);
    let mut child = crate_ref(2, &ground, [300.0, 0.0], 0.0);
    child.enable_parent = Some(mir::EnableParentDoc {
        reference: parent.form.clone(),
        opposite: true,
    });
    let sliced = run(
        &assets,
        &[parent, child],
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert_eq!(sliced.placements.len(), 1);
    assert_eq!(sliced.skipped[0].reason, SkipReason::DisabledByParent);
}

#[test]
fn runs_are_deterministic() {
    let assets = TestAssets::standard();
    let ground = sloped();
    let refs = [
        house(1, &ground, [0.0, 0.0]),
        crate_ref(2, &ground, [700.0, 10.0], 5.0),
        reference(
            3,
            "STAT",
            "meshes\\test\\strip.nif",
            "road",
            &ground,
            [0.0, -900.0],
            0.0,
            0.2,
        ),
        reference(
            4,
            "STAT",
            "meshes\\test\\rocks.nif",
            "slab",
            &ground,
            [-1500.0, 0.0],
            0.0,
            0.0,
        ),
    ];
    let mut reversed = refs.to_vec();
    reversed.reverse();
    let a = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    let b = run(
        &assets,
        &reversed,
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert_eq!(a, b);
}
