//! Synthetic tests: hand-built land, textures and palette.

use foundry_render::{Calibration, Projection};
use mir::{FormKey, GroundSampler, Image, LAND_GRID, LandDoc, LandLayer, LayerOpacity, MapWindow};
use quantize::PaletteMap;
use vault_formats::Palette;
use vault_formats::geometry::HexGrid;

use crate::terrain::{Geometry, Scratch, Shading, Terrain};
use crate::*;

fn palette_map() -> PaletteMap {
    let palette = Palette::parse(&quantize::fixture::synthetic_palette()).expect("palette");
    PaletteMap::new(&palette)
}

fn hash(c: char) -> String {
    c.to_string().repeat(64)
}

fn solid(rgb: [u8; 3]) -> Image {
    Image {
        width: 4,
        height: 4,
        rgba: std::iter::repeat_n([rgb[0], rgb[1], rgb[2], 255], 16)
            .flatten()
            .collect(),
    }
}

fn base(quadrant: u8, texture: &str) -> LandLayer {
    LandLayer {
        quadrant,
        layer: -1,
        land_texture: None,
        land_texture_editor_id: None,
        texture: None,
        texture_hash: Some(texture.to_owned()),
        opacity: Vec::new(),
        defaulted: None,
    }
}

fn blended(
    quadrant: u8,
    layer: i32,
    texture: &str,
    opacity: impl Fn(usize, usize) -> f32,
) -> LandLayer {
    let mut points = Vec::new();
    for row in 0..17 {
        for column in 0..17 {
            let value = opacity(row, column);
            if value > 0.0 {
                points.push(LayerOpacity {
                    position: (row * 17 + column) as u16,
                    opacity: value,
                });
            }
        }
    }
    LandLayer {
        opacity: points,
        layer,
        ..base(quadrant, texture)
    }
}

fn land(height: impl Fn(f32, f32) -> f32, cx: i32, cy: i32, layers: Vec<LandLayer>) -> LandDoc {
    let mut heights = Vec::new();
    for row in 0..LAND_GRID {
        for column in 0..LAND_GRID {
            let x = cx as f32 * 4096.0 + column as f32 * 128.0;
            let y = cy as f32 * 4096.0 + row as f32 * 128.0;
            heights.push(height(x, y));
        }
    }
    LandDoc {
        form: FormKey::new("test.esm", 1),
        heights,
        normals: None,
        colors: None,
        layers,
    }
}

fn all_bases(texture: &str) -> Vec<LandLayer> {
    (0..4).map(|q| base(q, texture)).collect()
}

/// A world of the cells `-2..=1` around the origin, which the map centred
/// on the origin sees whole.
struct World {
    land: MemoryLand,
    sampler: GroundSampler,
}

impl World {
    fn new(make: impl Fn(i32, i32) -> Option<LandDoc>) -> Self {
        let mut land = MemoryLand::default();
        let mut sampler = GroundSampler::default();
        for cy in -2..=1 {
            for cx in -2..=1 {
                if let Some(doc) = make(cx, cy) {
                    sampler.insert(cx, cy, &doc);
                    land.cells.insert((cx, cy), doc);
                }
            }
        }
        World { land, sampler }
    }

    fn texture(mut self, key: &str, image: Image) -> Self {
        self.land.textures.insert(key.to_owned(), image);
        self
    }
}

fn flat_world(rgb: [u8; 3]) -> World {
    let a = hash('a');
    World::new(|cx, cy| Some(land(|_, _| 0.0, cx, cy, all_bases(&a)))).texture(&a, solid(rgb))
}

/// Vertex colours that change everywhere, so that nearly every tile is
/// distinct.
fn gradient_world() -> World {
    let a = hash('a');
    World::new(|cx, cy| {
        let mut doc = land(|_, _| 0.0, cx, cy, all_bases(&a));
        let mut colors = Vec::new();
        for row in 0..LAND_GRID {
            for column in 0..LAND_GRID {
                let gx = cx * 32 + column as i32 + 64;
                let gy = cy * 32 + row as i32 + 64;
                colors.push([
                    (60 + gx * 3 / 2) as u8,
                    (60 + gy * 3 / 2) as u8,
                    (200 - (gx + gy) / 2) as u8,
                ]);
            }
        }
        doc.colors = Some(colors);
        Some(doc)
    })
    .texture(&a, solid([255, 255, 255]))
}

fn window() -> MapWindow {
    MapWindow::from_center([0.0, 0.0], 140.0)
}

fn projection() -> Projection {
    Projection::new(&Calibration::default()).expect("projection")
}

fn build(world: &World, overlays: &[Overlay], options: &TerrainOptions) -> Ground {
    build_ground_from(
        &world.land,
        &window(),
        &world.sampler,
        &projection(),
        &Calibration::default(),
        &palette_map(),
        overlays,
        options,
    )
    .expect("ground")
}

fn options() -> TerrainOptions {
    TerrainOptions {
        jobs: 4,
        ..TerrainOptions::default()
    }
}

/// The pieces `build_ground_from` evaluates with, for direct sampling.
struct Probe {
    land: Terrain,
    geometry: Geometry,
    shading: Shading,
}

impl Probe {
    fn new(world: &World, options: &TerrainOptions) -> Self {
        let geometry = Geometry::new(&projection(), window().anchor()).expect("geometry");
        let shading = Shading::new(&Calibration::default(), &geometry, options.relief);
        let land = Terrain::load(
            &world.land,
            &geometry,
            options,
            f64::from(options.texture_repeat_units),
            None,
        )
        .expect("terrain");
        Probe {
            land,
            geometry,
            shading,
        }
    }

    fn at(&self, world: &World, x: f64, y: f64) -> [f32; 3] {
        let mut scratch = Scratch::default();
        self.land
            .ground(
                &mut scratch,
                &world.sampler,
                &self.geometry,
                &self.shading,
                x,
                y,
            )
            .rgb
    }
}

fn brightness(rgb: [f32; 3]) -> f32 {
    rgb.iter().sum()
}

#[test]
fn the_ground_plane_follows_the_projection() {
    let p = projection();
    let w = window();
    let g = Geometry::new(&p, w.anchor()).expect("geometry");
    for (s, t) in [(0.0, 0.0), (50.0, 50.0), (12.5, 87.25), (100.0, 0.0)] {
        let screen = p.screen([s, t, 0.0]);
        let [x, y] = g.source([f64::from(screen[0]), f64::from(screen[1])]);
        let [ex, ey] = w.source(s, t);
        assert!((x - f64::from(ex)).abs() < 0.05 && (y - f64::from(ey)).abs() < 0.05);
    }
    // One screen pixel covers about 12.8 source units² at 140 units a tile.
    assert!((g.pixel_area() - 140.0 * 140.0 / 1536.0).abs() < 1e-6);
}

#[test]
fn tiles_cover_an_interior_region_exactly_once() {
    let world = flat_world([150, 120, 80]);
    let ground = build(&world, &[], &options());
    assert_eq!(ground.squares.len(), SQUARES);
    let (origin, width, height) = Ground::bounds();
    let grid = HexGrid::default();
    let mut count = vec![0u8; (width * height) as usize];
    let mut owner = vec![-1i32; (width * height) as usize];
    for (q, &entry) in ground.squares.iter().enumerate() {
        let tile = &ground.tiles[usize::from(entry)];
        let corner = grid.square_corner(q as i32, false).expect("corner");
        for py in 0..TILE_HEIGHT as i64 {
            for px in 0..TILE_WIDTH as i64 {
                if tile[(py * i64::from(TILE_WIDTH) + px) as usize] == 0 {
                    continue;
                }
                let x = corner[0] + px - i64::from(origin[0]);
                let y = corner[1] + py - i64::from(origin[1]);
                let at = (y * i64::from(width) + x) as usize;
                count[at] += 1;
                owner[at] = q as i32;
            }
        }
    }
    // A region well inside the map's parallelogram.
    for y in 200..1000 {
        for x in 3000..5000 {
            let at = ((y - origin[1]) as i64 * i64::from(width) + (x - origin[0]) as i64) as usize;
            assert_eq!(count[at], 1, "pixel ({x}, {y})");
            assert_eq!(Some(owner[at]), grid.pick_square([x, y], false));
        }
    }
    // Every tile has exactly the parallelogram opaque.
    let mask = square_mask(grid, 5050);
    // Each side of the parallelogram touches its neighbour along a run of
    // pixels: the long sides (to s ± 1) about 48 wide, the short (t ± 1) 32.
    let shape = square_shape(grid, 5050);
    assert_eq!(shape.mask, mask);
    for side in &shape.edges {
        assert!(side.len() >= 16, "{}", side.len());
    }
    assert_eq!(mask.len(), 1536);
    for tile in &ground.tiles {
        assert_eq!(tile.iter().filter(|&&i| i != 0).count(), mask.len());
    }
    // The composition agrees with the hand count.
    let (pixels, w, h) = ground.compose();
    assert_eq!((w, h), (width, height));
    for (p, c) in pixels.iter().zip(&count) {
        assert_eq!(*p != 0, *c > 0);
    }
}

#[test]
fn flat_uniform_ground_needs_one_tile_per_dither_phase() {
    let world = flat_world([150, 120, 80]);
    let plain = build(
        &world,
        &[],
        &TerrainOptions {
            dither_strength: 0.0,
            ..options()
        },
    );
    assert_eq!(plain.tiles.len(), 1);
    assert_eq!(plain.stats.exact_duplicates, SQUARES - 1);
    assert_eq!(plain.stats.fallback_pixels, 0);
    assert_eq!(plain.stats.clusters, 0);

    let dithered = build(&world, &[], &options());
    assert!(dithered.tiles.len() <= 2, "{}", dithered.tiles.len());
    // The dither phase depends on the parity of s alone.
    for q in 0..SQUARES {
        assert_eq!(dithered.squares[q], dithered.squares[q % 2]);
    }
    assert_eq!(
        dithered.stats.distinct_tiles + dithered.stats.exact_duplicates,
        SQUARES
    );
}

#[test]
fn layer_blending_is_continuous_across_a_quadrant_seam_and_a_cell_edge() {
    let (red, green) = (hash('r'), hash('g'));
    // Green over red, its opacity ramping with the global column of the
    // two-cell strip: continuous across quadrants and cells.
    let world = World::new(|cx, cy| {
        if cy != 0 || !(0..=1).contains(&cx) {
            return None;
        }
        let mut layers = all_bases(&red);
        for q in 0..4u8 {
            let east = u32::from(q % 2);
            layers.push(blended(q, 0, &green, |_, column| {
                let global = cx as u32 * 32 + east * 16 + column as u32;
                global as f32 / 64.0
            }));
        }
        Some(land(|_, _| 0.0, cx, cy, layers))
    })
    .texture(&red, solid([200, 0, 0]))
    .texture(&green, solid([0, 200, 0]));
    let probe = Probe::new(&world, &options());
    for seam in [2048.0, 4096.0] {
        let west = probe.at(&world, seam - 0.01, 1000.0);
        let east = probe.at(&world, seam + 0.01, 1000.0);
        for k in 0..3 {
            assert!(
                (west[k] - east[k]).abs() < 0.5,
                "{seam}: {west:?} vs {east:?}"
            );
        }
    }
    // A quarter of the way: 16 of 64 columns.
    let c = probe.at(&world, 2048.0, 3000.0);
    assert!(
        (c[0] - 150.0).abs() < 1.0 && (c[1] - 50.0).abs() < 1.0,
        "{c:?}"
    );
    // Across the north-south quadrant seam too.
    let south = probe.at(&world, 1500.0, 2047.99);
    let north = probe.at(&world, 1500.0, 2048.01);
    assert!((brightness(south) - brightness(north)).abs() < 0.5);
}

#[test]
fn layers_blend_in_ascending_order() {
    let (red, green, blue) = (hash('r'), hash('g'), hash('b'));
    // Layer 3 (blue, full) listed before layer 1 (green, full): blue wins.
    let world = World::new(|_, _| {
        let mut layers = all_bases(&red);
        layers.push(blended(0, 3, &blue, |_, _| 1.0));
        layers.push(blended(0, 1, &green, |_, _| 1.0));
        Some(land(|_, _| 0.0, 0, 0, layers))
    })
    .texture(&red, solid([200, 0, 0]))
    .texture(&green, solid([0, 200, 0]))
    .texture(&blue, solid([0, 0, 200]));
    let probe = Probe::new(&world, &options());
    let c = probe.at(&world, 500.0, 500.0);
    assert!(c[2] > 199.0 && c[0] < 1.0 && c[1] < 1.0, "{c:?}");
}

#[test]
fn vertex_colour_and_relief_shade_in_the_expected_direction() {
    let a = hash('a');
    let make = |height: fn(f32, f32) -> f32, tint: Option<[u8; 3]>| {
        World::new(move |cx, cy| {
            let mut doc = land(height, cx, cy, all_bases(&hash('a')));
            doc.colors = tint.map(|t| vec![t; LAND_GRID * LAND_GRID]);
            Some(doc)
        })
        .texture(&a, solid([160, 160, 160]))
    };
    let flat = make(|_, _| 0.0, None);
    let tinted = make(|_, _| 0.0, Some([128, 255, 255]));
    // The key light comes from the upper left of the screen: map west
    // (−s) and north (−t). Ground rising to the east faces west.
    let facing = make(|x, _| 0.5 * x, None);
    let away = make(|x, _| -0.5 * x, None);
    let base = probe_at(&flat, &options());
    assert!(
        (base[0] - 160.0).abs() < 0.01,
        "flat ground keeps its colour: {base:?}"
    );
    let t = probe_at(&tinted, &options());
    assert!((t[0] - 160.0 * 128.0 / 255.0).abs() < 0.5 && (t[1] - 160.0).abs() < 0.01);
    let lit = probe_at(&facing, &options());
    let dark = probe_at(&away, &options());
    assert!(
        brightness(lit) > brightness(base) + 10.0,
        "{lit:?} vs {base:?}"
    );
    assert!(
        brightness(dark) < brightness(base) - 10.0,
        "{dark:?} vs {base:?}"
    );
    // Relief 0 is flat shading; more relief departs further.
    let none = probe_at(
        &facing,
        &TerrainOptions {
            relief: 0.0,
            ..options()
        },
    );
    assert_eq!(none, base);
    let strong = probe_at(
        &facing,
        &TerrainOptions {
            relief: 2.0,
            ..options()
        },
    );
    assert!(brightness(strong) > brightness(lit));
}

fn probe_at(world: &World, options: &TerrainOptions) -> [f32; 3] {
    Probe::new(world, options).at(world, 300.0, 700.0)
}

#[test]
fn overlays_composite_over_their_own_pixels() {
    let world = flat_world([150, 120, 80]);
    let map = palette_map();
    let plain = TerrainOptions {
        dither_strength: 0.0,
        ..options()
    };
    let blue: Vec<u8> = std::iter::repeat_n([0u8, 0, 255, 255], 40 * 20)
        .flatten()
        .collect();
    let clear: Vec<u8> = std::iter::repeat_n([255u8, 0, 0, 0], 40 * 20)
        .flatten()
        .collect();
    let overlays = [
        Overlay {
            origin: [3900, 600],
            width: 40,
            height: 20,
            rgba: &blue,
        },
        Overlay {
            origin: [3000, 300],
            width: 40,
            height: 20,
            rgba: &clear,
        },
    ];
    let ground = build(&world, &overlays, &plain);
    let reference = build(&world, &[], &plain);
    let floor = reference.tiles[0]
        .iter()
        .copied()
        .find(|&i| i != 0)
        .expect("an opaque pixel");
    let blue_index = map.nearest([0, 0, 255]);
    assert_ne!(floor, blue_index);
    let pixels = ground.compose_rect([3880, 580], 80, 60).expect("compose");
    for y in 0..60 {
        for x in 0..80 {
            let inside = (20..60).contains(&x) && (20..40).contains(&y);
            let expected = if inside { blue_index } else { floor };
            assert_eq!(pixels[y * 80 + x], expected, "({x}, {y})");
        }
    }
    // A fully transparent overlay changes nothing.
    let clear_area = ground.compose_rect([3000, 300], 40, 20).expect("compose");
    assert!(clear_area.iter().all(|&i| i == floor));
    // A buffer of the wrong length is an error.
    let short = [Overlay {
        origin: [0, 0],
        width: 2,
        height: 2,
        rgba: &blue[..15],
    }];
    let error = build_ground_from(
        &world.land,
        &window(),
        &world.sampler,
        &projection(),
        &Calibration::default(),
        &map,
        &short,
        &plain,
    );
    assert!(matches!(error, Err(TerrainError::Overlay { index: 0, .. })));
}

#[test]
fn the_library_cap_is_honoured_and_the_stats_add_up() {
    let world = gradient_world();
    let ground = build(
        &world,
        &[],
        &TerrainOptions {
            library_cap: 64,
            ..options()
        },
    );
    let s = &ground.stats;
    assert!(s.distinct_tiles > 64, "{s:?}");
    assert!(ground.tiles.len() <= 64);
    assert_eq!(s.tiles_after, ground.tiles.len());
    assert_eq!(s.clusters, ground.tiles.len());
    assert_eq!(s.tiles_before, SQUARES);
    assert_eq!(s.distinct_tiles + s.exact_duplicates, SQUARES);
    assert!(
        ground
            .squares
            .iter()
            .all(|&e| usize::from(e) < ground.tiles.len())
    );
    assert!(s.substituted_squares > 0 && s.substituted_squares <= SQUARES);
    assert!(s.mean_substitution_error > 0.0);
    assert!(s.mean_substitution_error <= s.worst_substitution_error);
    // Every library tile is used, and in order of first use.
    let mut first = vec![usize::MAX; ground.tiles.len()];
    for (q, &e) in ground.squares.iter().enumerate() {
        first[usize::from(e)] = first[usize::from(e)].min(q);
    }
    assert!(first.windows(2).all(|w| w[0] < w[1]));
    // A larger cap never does worse.
    let roomy = build(
        &world,
        &[],
        &TerrainOptions {
            library_cap: 400,
            ..options()
        },
    );
    assert!(roomy.stats.mean_substitution_error < s.mean_substitution_error);
    // No room at all is an error.
    {
        let error = build_ground_from(
            &world.land,
            &window(),
            &world.sampler,
            &projection(),
            &Calibration::default(),
            &palette_map(),
            &[],
            &TerrainOptions {
                library_cap: 0,
                ..options()
            },
        );
        assert!(matches!(error, Err(TerrainError::LibraryCap(0))));
    }
}

#[test]
fn output_is_deterministic_and_independent_of_jobs() {
    let world = gradient_world();
    let run = |jobs| {
        build(
            &world,
            &[],
            &TerrainOptions {
                library_cap: 50,
                jobs,
                ..options()
            },
        )
    };
    let one = run(1);
    let three = run(3);
    let again = run(3);
    assert_eq!(one.tiles, three.tiles);
    assert_eq!(one.squares, three.squares);
    assert_eq!(three.tiles, again.tiles);
    assert_eq!(three.squares, again.squares);
    assert_eq!(
        one.stats.mean_substitution_error,
        three.stats.mean_substitution_error
    );
}

/// FNV-1a over the library and the square entries.
fn digest(ground: &Ground) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for b in (ground.tiles.len() as u64).to_le_bytes() {
        eat(b);
    }
    for tile in &ground.tiles {
        tile.iter().copied().for_each(&mut eat);
    }
    for entry in &ground.squares {
        entry.to_le_bytes().into_iter().for_each(&mut eat);
    }
    h
}

/// A gradient of vertex colours over a texture with detail at every
/// phase: a 16 × 16 pattern of deterministic grain, so that the sixteen
/// phase classes really differ.
fn textured_gradient_world() -> World {
    let a = hash('a');
    let mut state = 0x1234_5678u32;
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for _ in 0..16 * 16 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let grain = 120 + (state >> 27) as u8 * 3;
        rgba.extend_from_slice(&[grain, grain, grain, 255]);
    }
    let image = Image {
        width: 16,
        height: 16,
        rgba,
    };
    let base = gradient_world();
    World {
        land: MemoryLand {
            textures: [(a, image)].into_iter().collect(),
            ..base.land
        },
        sampler: base.sampler,
    }
}

#[test]
fn todays_default_caps_give_the_output_they_gave_before() {
    // Pinned on the library of before the large-cap work (k-means, the
    // method callers get by default), at the cap today's map stage passes
    // (977 free entries) and at the default cap.
    let worlds = [
        ("gradient", gradient_world()),
        ("textured", textured_gradient_world()),
    ];
    let mut seen = Vec::new();
    for (name, world) in &worlds {
        for cap in [977, 2000] {
            let ground = build(
                world,
                &[],
                &TerrainOptions {
                    library_cap: cap,
                    ..options()
                },
            );
            seen.push((
                *name,
                cap,
                ground.stats.distinct_tiles,
                ground.tiles.len(),
                format!("{:016x}", digest(&ground)),
            ));
        }
    }
    let expected: Vec<(&str, usize, usize, usize, String)> = [
        ("gradient", 977, 7600, 965, "0274ada47bdb5066"),
        ("gradient", 2000, 7600, 1984, "55cfa9325b2345a5"),
        ("textured", 977, 9758, 962, "24c546d9a80b70c1"),
        ("textured", 2000, 9758, 1975, "bc6fc9e778a7825f"),
    ]
    .into_iter()
    .map(|(n, c, d, t, h)| (n, c, d, t, h.to_owned()))
    .collect();
    assert_eq!(seen, expected);
}

/// The per-square substitution error of `ground` against `exact` (the same
/// ground with no cap), as `GroundStats` measures it.
fn square_errors(ground: &Ground, exact: &Ground) -> Vec<f64> {
    let map = palette_map();
    (0..SQUARES)
        .map(|q| {
            let own = &exact.tiles[usize::from(exact.squares[q])];
            let used = &ground.tiles[usize::from(ground.squares[q])];
            let mut sum = 0.0;
            let mut n = 0;
            for (&a, &b) in own.iter().zip(used) {
                if a != 0 {
                    let (la, lb) = (map.oklab(a), map.oklab(b));
                    sum += (0..3).map(|k| (la[k] - lb[k]).powi(2)).sum::<f64>().sqrt();
                    n += 1;
                }
            }
            sum / f64::from(n.max(1))
        })
        .collect()
}

fn greedy(cap: usize, jobs: usize) -> TerrainOptions {
    TerrainOptions {
        library_cap: cap,
        library_method: LibraryMethod::GreedyMerge,
        jobs,
        ..options()
    }
}

#[test]
fn the_greedy_merge_at_a_large_cap_is_deterministic_and_honours_the_cap() {
    let world = textured_gradient_world();
    let one = build(&world, &[], &greedy(4090, 1));
    let four = build(&world, &[], &greedy(4090, 4));
    assert_eq!(one.tiles, four.tiles);
    assert_eq!(one.squares, four.squares);
    assert_eq!(
        one.stats.mean_substitution_error,
        four.stats.mean_substitution_error
    );
    assert_eq!(
        one.stats.worst_substitution_error,
        four.stats.worst_substitution_error
    );
    assert_eq!(
        one.stats.substituted_squares,
        four.stats.substituted_squares
    );
    let again = build(&world, &[], &greedy(4090, 3));
    assert_eq!(digest(&again), digest(&four));

    let s = &four.stats;
    assert!(s.distinct_tiles > 4090, "{s:?}");
    assert!(four.tiles.len() <= 4090);
    // The merge fills the cap; only the seam pass may leave a few unused.
    assert!(four.tiles.len() > 4000, "{}", four.tiles.len());
    assert_eq!(s.clusters, four.tiles.len());
    assert_eq!(s.tiles_after, four.tiles.len());
    assert!(s.substituted_squares >= s.distinct_tiles - 4090);
    assert!(s.mean_substitution_error > 0.0);
    assert!(s.mean_substitution_error <= s.worst_substitution_error);
    let mut first = vec![usize::MAX; four.tiles.len()];
    for (q, &e) in four.squares.iter().enumerate() {
        first[usize::from(e)] = first[usize::from(e)].min(q);
    }
    assert!(first.windows(2).all(|w| w[0] < w[1]));
    // The stats agree with an independent count.
    let exact = build(
        &world,
        &[],
        &TerrainOptions {
            library_cap: SQUARES,
            ..options()
        },
    );
    let errors = square_errors(&four, &exact);
    let mean = errors.iter().sum::<f64>() / SQUARES as f64;
    assert!(
        (mean - s.mean_substitution_error).abs() < 1e-6,
        "{mean} vs {s:?}"
    );
    // A substitute always comes from the square's own phase class: squares
    // four apart along s and t see the texture at the same offset.
    let [ps, pt] = s.phase_period;
    let class = |q: usize| ((99 - q % 100) % ps, (q / 100) % pt);
    let mut owner_class = vec![None; four.tiles.len()];
    for q in 0..SQUARES {
        let slot = &mut owner_class[usize::from(four.squares[q])];
        assert!(slot.is_none_or(|c| c == class(q)), "square {q}");
        *slot = Some(class(q));
    }
}

#[test]
fn the_greedy_merge_has_lower_error_than_k_means() {
    let world = textured_gradient_world();
    for cap in [977, 4090] {
        let kmeans = build(
            &world,
            &[],
            &TerrainOptions {
                library_cap: cap,
                ..options()
            },
        );
        let merged = build(&world, &[], &greedy(cap, 4));
        assert!(
            merged.stats.mean_substitution_error < kmeans.stats.mean_substitution_error,
            "cap {cap}: {:?} vs {:?}",
            merged.stats,
            kmeans.stats
        );
        assert!(merged.tiles.len() <= cap);
    }
    // Below the distinct count nothing changes: no tile is substituted.
    let roomy = build(&world, &[], &greedy(SQUARES, 4));
    assert_eq!(roomy.stats.substituted_squares, 0);
    assert_eq!(roomy.tiles.len(), roomy.stats.distinct_tiles);
}

#[test]
fn the_greedy_merge_keeps_distinctive_overlay_tiles() {
    // Bright marks baked over a few squares: each makes a tile unlike any
    // other, so removing it would cost most, and the merge keeps it.
    let world = textured_gradient_world();
    let mark: Vec<u8> = (0..48 * 24)
        .flat_map(|i| {
            let v = if (i / 48 + i % 48) % 6 < 3 { 250 } else { 30 };
            [v, 255 - v, 40, 255]
        })
        .collect();
    let grid = HexGrid::default();
    let marked: Vec<i32> = (0..24).map(|i| 1717 + i * 311).collect();
    let overlays: Vec<Overlay> = marked
        .iter()
        .map(|&q| {
            let corner = grid.square_corner(q, false).expect("corner");
            Overlay {
                origin: [corner[0] as i32 + 16, corner[1] as i32 + 6],
                width: 48,
                height: 24,
                rgba: &mark,
            }
        })
        .collect();
    let exact = build(
        &world,
        &overlays,
        &TerrainOptions {
            library_cap: SQUARES,
            ..options()
        },
    );
    let mut under = vec![false; SQUARES];
    for overlay in &overlays {
        for y in 0..overlay.height as i32 {
            for x in 0..overlay.width as i32 {
                if let Some(q) =
                    grid.pick_square([overlay.origin[0] + x, overlay.origin[1] + y], false)
                {
                    under[q as usize] = true;
                }
            }
        }
    }
    let marked_error = |ground: &Ground| {
        let errors = square_errors(ground, &exact);
        let picked: Vec<f64> = (0..SQUARES)
            .filter(|&q| under[q])
            .map(|q| errors[q])
            .collect();
        picked.iter().sum::<f64>() / picked.len() as f64
    };
    let merged = build(&world, &overlays, &greedy(4090, 4));
    let kmeans = build(
        &world,
        &overlays,
        &TerrainOptions {
            library_cap: 4090,
            ..options()
        },
    );
    // The squares a mark touches, slivers at the corners of the neighbours
    // included (few pixels, but they show as a mark cut off at a tile
    // edge), lose almost nothing: an order of magnitude less than the
    // rest of the map, and below k-means.
    let (m, k) = (marked_error(&merged), marked_error(&kmeans));
    let overall = merged.stats.mean_substitution_error;
    assert!(m < overall * 0.1, "under the marks {m}, overall {overall}");
    assert!(m < k, "greedy {m} vs k-means {k}");
    for &q in &marked {
        let own = &exact.tiles[usize::from(exact.squares[q as usize])];
        assert_eq!(
            &merged.tiles[usize::from(merged.squares[q as usize])],
            own,
            "{q}"
        );
    }
}

#[test]
fn missing_land_and_textures_are_counted_not_fatal() {
    let (a, lost) = (hash('a'), hash('f'));
    let world = World::new(|cx, cy| match (cx, cy) {
        (-2, -2) => None,
        (0, 0) => {
            let mut layers = all_bases(&a);
            layers.push(blended(0, 0, &lost, |_, _| 1.0));
            Some(land(|_, _| 0.0, cx, cy, layers))
        }
        // A quadrant with no base layer: the default land texture.
        (1, 1) => Some(land(
            |_, _| 0.0,
            cx,
            cy,
            vec![base(0, &a), base(1, &a), base(2, &a)],
        )),
        _ => Some(land(|_, _| 0.0, cx, cy, all_bases(&a))),
    })
    .texture(&a, solid([150, 120, 80]));
    let ground = build(&world, &[], &options());
    let s = &ground.stats;
    assert!(s.fallback_pixels > 0, "{s:?}");
    assert!(s.missing_texture_pixels > 0);
    assert_eq!(s.missing_texture_layers, 2);
    // The lost layer names a file; the absent base names nothing, and
    // borrows its neighbour's texture.
    assert_eq!(s.unnamed_texture_layers, 1);
    assert_eq!(s.borrowed_base_layers, 1);
    assert_eq!(
        (s.defaulted_texture_layers, s.defaulted_texture_pixels),
        (0, 0)
    );
    assert_eq!((s.textures_loaded, s.textures_missing), (1, 1));
    assert_eq!((s.cells_with_land, s.cells_without_land), (15, 1));
    // The missing blended layer is left out: the base shows through.
    let probe = Probe::new(&world, &options());
    let c = probe.at(&world, 500.0, 500.0);
    assert!(
        (c[0] - 150.0).abs() < 0.01 && (c[2] - 80.0).abs() < 0.01,
        "{c:?}"
    );
    let grey = Probe::new(
        &world,
        &TerrainOptions {
            missing_texture_rgb: Some([128, 128, 128]),
            ..options()
        },
    );
    assert_eq!(grey.at(&world, 500.0, 500.0), [128.0; 3]);
    let flat = build(
        &world,
        &[],
        &TerrainOptions {
            missing_texture_rgb: Some([128, 128, 128]),
            ..options()
        },
    );
    assert_eq!(flat.stats.borrowed_base_layers, 0);
    assert_eq!(flat.stats.missing_texture_pixels, s.missing_texture_pixels);
    let outside = probe.at(&world, -8000.0, -8000.0);
    assert_eq!(outside, DEFAULT_FALLBACK_RGB.map(f32::from));
}

#[test]
fn a_sub_rectangle_is_a_crop_of_the_whole() {
    let world = gradient_world();
    let ground = build(&world, &[], &options());
    let (whole, width, _) = ground.compose();
    let (origin, _, _) = Ground::bounds();
    let (x0, y0, w, h) = (2500, -100, 300, 200);
    let part = ground.compose_rect([x0, y0], w, h).expect("compose");
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let at = ((y0 + y - origin[1]) * width as i32 + (x0 + x - origin[0])) as usize;
            assert_eq!(part[(y * w as i32 + x) as usize], whole[at]);
        }
    }
    // Outside the map: nothing.
    let away = ground
        .compose_rect([-5000, -5000], 10, 10)
        .expect("compose");
    assert!(away.iter().all(|&i| i == 0));
    assert!(matches!(
        ground.compose_rect([0, 0], 1 << 14, 1 << 14),
        Err(TerrainError::ComposeTooLarge { .. })
    ));
}

#[test]
fn options_are_validated() {
    let bad = [
        TerrainOptions {
            dither_strength: -1.0,
            ..options()
        },
        TerrainOptions {
            relief: f32::NAN,
            ..options()
        },
        TerrainOptions {
            supersample: 0,
            ..options()
        },
        TerrainOptions {
            texture_repeat_units: 0.0,
            ..options()
        },
        TerrainOptions {
            cluster_iterations: 1000,
            ..options()
        },
        TerrainOptions {
            vertex_color_floor: 1.5,
            ..options()
        },
        TerrainOptions {
            vertex_color_floor: f32::NAN,
            ..options()
        },
    ];
    for o in bad {
        assert!(
            matches!(o.validate(), Err(TerrainError::InvalidOption { .. })),
            "{o:?}"
        );
    }
    assert!(options().validate().is_ok());
}

/// A texture of a hue with a little grain, so that tiles differ.
fn grained(rgb: [u8; 3], seed: u32) -> Image {
    let mut state = seed;
    let mut rgba = Vec::with_capacity(8 * 8 * 4);
    for _ in 0..64 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let grain = (state >> 28) as u8;
        rgba.extend_from_slice(&[rgb[0] + grain, rgb[1] + grain, rgb[2] + grain, 255]);
    }
    Image {
        width: 8,
        height: 8,
        rgba,
    }
}

fn defaulted(mut layer: LandLayer, reason: mir::LandDefault) -> LandLayer {
    layer.defaulted = Some(reason);
    layer
}

#[test]
fn defaulted_layers_draw_their_texture_and_are_counted_apart() {
    // Cell (0, 0) has the default texture as the base of quadrant 1 and as
    // a blended layer over quadrant 0, exactly as an extract writes them.
    let (a, d) = (hash('a'), hash('d'));
    let make = |mark: bool| {
        World::new(|cx, cy| {
            if (cx, cy) != (0, 0) {
                return Some(land(|_, _| 0.0, cx, cy, all_bases(&a)));
            }
            let mut layers = vec![base(0, &a), base(1, &d), base(2, &a), base(3, &a)];
            layers.push(blended(0, 0, &d, |row, _| row as f32 / 16.0));
            if mark {
                layers[1] = defaulted(layers[1].clone(), mir::LandDefault::NoBaseLayer);
                layers[4] = defaulted(layers[4].clone(), mir::LandDefault::NullTexture);
            }
            Some(land(|_, _| 0.0, cx, cy, layers))
        })
        .texture(&a, grained([150, 110, 70], 1))
        .texture(&d, grained([90, 80, 60], 2))
    };
    let marked = build(&make(true), &[], &options());
    let plain = build(&make(false), &[], &options());
    // Drawn exactly as if the record had named the texture itself.
    assert_eq!(marked.tiles, plain.tiles);
    assert_eq!(marked.squares, plain.squares);
    let s = &marked.stats;
    assert_eq!(s.defaulted_texture_layers, 2);
    assert!(s.defaulted_texture_pixels > 0, "{s:?}");
    assert_eq!((s.missing_texture_layers, s.missing_texture_pixels), (0, 0));
    assert_eq!(plain.stats.defaulted_texture_pixels, 0);
    // Quadrant 1 of cell (0, 0) is the default texture's colour.
    let world = make(true);
    let c = Probe::new(&world, &options()).at(&world, 3000.0, 1000.0);
    assert!(
        (c[0] - 97.5).abs() < 8.0 && (c[2] - 67.5).abs() < 8.0,
        "{c:?}"
    );
}

#[test]
fn a_hole_in_the_ground_borrows_the_nearest_base_and_is_still_counted() {
    // Cell (0, 0) has no layer at all, as an old document of a record
    // without base layers; everything else is red, and one far cell has a
    // green blended layer, so the mean of the textures is not red.
    let (r, g) = (hash('r'), hash('g'));
    let world = World::new(|cx, cy| match (cx, cy) {
        (0, 0) => Some(land(|_, _| 0.0, cx, cy, Vec::new())),
        (-2, -2) => {
            let mut layers = all_bases(&r);
            layers.push(blended(0, 0, &g, |_, _| 1.0));
            Some(land(|_, _| 0.0, cx, cy, layers))
        }
        _ => Some(land(|_, _| 0.0, cx, cy, all_bases(&r))),
    })
    .texture(&r, solid([200, 40, 40]))
    .texture(&g, solid([40, 200, 40]));
    let ground = build(&world, &[], &options());
    let s = &ground.stats;
    assert_eq!(s.missing_texture_layers, 4);
    assert_eq!(s.unnamed_texture_layers, 4);
    assert_eq!(s.borrowed_base_layers, 4);
    assert!(s.missing_texture_pixels > 0);
    let probe = Probe::new(&world, &options());
    for (x, y) in [
        (500.0, 500.0),
        (3500.0, 500.0),
        (500.0, 3500.0),
        (3500.0, 3500.0),
    ] {
        assert_eq!(probe.at(&world, x, y), [200.0, 40.0, 40.0], "at {x}, {y}");
    }
    // The flat tone, when asked for, shows the hole.
    let flat = Probe::new(
        &world,
        &TerrainOptions {
            missing_texture_rgb: Some([0, 0, 255]),
            ..options()
        },
    );
    assert_eq!(flat.at(&world, 500.0, 500.0), [0.0, 0.0, 255.0]);
    // With no texture anywhere to borrow, the base takes the mean colour.
    let bare = World::new(|cx, cy| Some(land(|_, _| 0.0, cx, cy, Vec::new())));
    let probe = Probe::new(&bare, &options());
    assert_eq!(probe.at(&bare, 500.0, 500.0), [128.0; 3]);
}

#[test]
fn a_missing_blended_layer_lets_the_layers_below_show_through() {
    // Over a red base: a green layer, and a lost layer above it at full
    // opacity in quadrant 0 only.
    let (r, g, lost) = (hash('r'), hash('g'), hash('x'));
    let world = World::new(|cx, cy| {
        let mut layers = all_bases(&r);
        layers.push(blended(0, 0, &g, |_, _| 0.5));
        layers.push(blended(0, 1, &lost, |_, _| 1.0));
        Some(land(|_, _| 0.0, cx, cy, layers))
    })
    .texture(&r, solid([200, 40, 40]))
    .texture(&g, solid([40, 200, 40]));
    let probe = Probe::new(&world, &options());
    assert_eq!(probe.at(&world, 500.0, 500.0), [120.0, 120.0, 40.0]);
    let ground = build(&world, &[], &options());
    let s = &ground.stats;
    assert_eq!(
        (s.missing_texture_layers, s.unnamed_texture_layers),
        (16, 0)
    );
    assert_eq!(s.textures_missing, 1);
    assert!(s.missing_texture_pixels > 0);
}

/// No land anywhere, as in an interior cell, on flat ground.
fn landless_world() -> World {
    World {
        land: MemoryLand::default(),
        sampler: GroundSampler::flat(3456.0),
    }
}

fn rgba_block(rgba: [u8; 4], width: u32, height: u32) -> Vec<u8> {
    std::iter::repeat_n(rgba, (width * height) as usize)
        .flatten()
        .collect()
}

/// The squares any pixel of a screen rectangle belongs to.
fn squares_under(origin: [i32; 2], width: i32, height: i32) -> std::collections::BTreeSet<i32> {
    let grid = HexGrid::default();
    let mut out = std::collections::BTreeSet::new();
    for y in origin[1]..origin[1] + height {
        for x in origin[0]..origin[0] + width {
            if let Some(q) = grid.pick_square([x, y], false) {
                out.insert(q);
            }
        }
    }
    out
}

#[test]
fn no_land_and_one_overlay_composites_over_the_fallback() {
    let world = landless_world();
    let map = palette_map();
    let plain = TerrainOptions {
        dither_strength: 0.0,
        ..options()
    };
    let blue = rgba_block([0, 0, 255, 255], 40, 20);
    let overlays = [Overlay {
        origin: [3900, 600],
        width: 40,
        height: 20,
        rgba: &blue,
    }];
    let ground = build(&world, &overlays, &plain);
    let s = &ground.stats;
    assert_eq!((s.cells_with_land, s.textures_loaded), (0, 0));
    assert_eq!(s.fallback_pixels, SQUARES as u64 * 1536);
    assert_eq!(s.blank_squares, 0);
    assert!(ground.blank_squares.is_empty());
    assert_eq!(ground.blank_tile(), None);
    let fallback = map.nearest(DEFAULT_FALLBACK_RGB);
    let blue_index = map.nearest([0, 0, 255]);
    let pixels = ground.compose_rect([3880, 580], 80, 60).expect("compose");
    for y in 0..60 {
        for x in 0..80 {
            let inside = (20..60).contains(&x) && (20..40).contains(&y);
            let expected = if inside { blue_index } else { fallback };
            assert_eq!(pixels[y * 80 + x], expected, "({x}, {y})");
        }
    }
}

#[test]
fn blank_ground_leaves_uncovered_squares_blank() {
    let world = landless_world();
    let map = palette_map();
    let blank = TerrainOptions {
        dither_strength: 0.0,
        blank_without_land: true,
        ..options()
    };
    let blue = rgba_block([0, 0, 255, 255], 40, 20);
    let grey = rgba_block([200, 200, 200, 128], 30, 10);
    let clear = rgba_block([255, 0, 0, 0], 40, 20);
    let overlays = [
        Overlay {
            origin: [3900, 600],
            width: 40,
            height: 20,
            rgba: &blue,
        },
        Overlay {
            origin: [4300, 700],
            width: 30,
            height: 10,
            rgba: &grey,
        },
        Overlay {
            origin: [3000, 300],
            width: 40,
            height: 20,
            rgba: &clear,
        },
    ];
    let ground = build(&world, &overlays, &blank);
    // Opaque over black, half-transparent over black, nothing.
    let blue_index = map.nearest([0, 0, 255]);
    let pixels = ground.compose_rect([3880, 580], 80, 60).expect("compose");
    for y in 0..60 {
        for x in 0..80 {
            let inside = (20..60).contains(&x) && (20..40).contains(&y);
            let expected = if inside { blue_index } else { 0 };
            assert_eq!(pixels[y * 80 + x], expected, "({x}, {y})");
        }
    }
    let half = ground.compose_rect([4300, 700], 30, 10).expect("compose");
    assert!(half.iter().all(|&i| i == map.nearest([100, 100, 100])));
    let clear_area = ground.compose_rect([3000, 300], 40, 20).expect("compose");
    assert!(clear_area.iter().all(|&i| i == 0));
    // Exactly the squares under a visible overlay are drawn.
    let mut drawn = squares_under([3900, 600], 40, 20);
    drawn.extend(squares_under([4300, 700], 30, 10));
    assert_eq!(ground.blank_squares.len(), SQUARES);
    assert_eq!(ground.stats.blank_squares, SQUARES - drawn.len());
    let zero = ground.blank_tile().expect("a blank tile");
    assert!(ground.tiles[usize::from(zero)].iter().all(|&i| i == 0));
    for q in 0..SQUARES {
        assert_eq!(ground.is_blank(q), !drawn.contains(&(q as i32)), "{q}");
        assert_eq!(ground.squares[q] == zero, ground.is_blank(q), "{q}");
    }
    assert!(!ground.is_blank(SQUARES));
}

#[test]
fn blank_ground_changes_nothing_where_there_is_land() {
    let world = gradient_world();
    let cap = TerrainOptions {
        library_cap: 977,
        ..options()
    };
    let before = build(&world, &[], &cap);
    let after = build(
        &world,
        &[],
        &TerrainOptions {
            blank_without_land: true,
            ..cap.clone()
        },
    );
    assert_eq!(digest(&after), digest(&before));
    assert_eq!(after.blank_squares, vec![false; SQUARES]);
    assert_eq!(after.stats.blank_squares, 0);
    assert_eq!(after.blank_tile(), None);
}

#[test]
fn the_blank_tile_survives_reduction_and_is_never_a_substitute() {
    // Land in the eastern half only, many distinct tiles, a tight cap.
    let base = gradient_world();
    let land = MemoryLand {
        cells: base
            .land
            .cells
            .into_iter()
            .filter(|((cx, _), _)| *cx >= 0)
            .collect(),
        textures: base.land.textures,
    };
    let mut sampler = GroundSampler::flat(0.0);
    for ((cx, cy), doc) in &land.cells {
        sampler.insert(*cx, *cy, doc);
    }
    let world = World { land, sampler };
    for method in [LibraryMethod::KMeans, LibraryMethod::GreedyMerge] {
        let ground = build(
            &world,
            &[],
            &TerrainOptions {
                library_cap: 120,
                library_method: method,
                blank_without_land: true,
                ..options()
            },
        );
        assert!(ground.tiles.len() <= 120, "{}", ground.tiles.len());
        let blank = ground.stats.blank_squares;
        assert!(blank > 1000 && blank < SQUARES - 1000, "{blank}");
        let zero = ground.blank_tile().expect("a blank tile");
        for q in 0..SQUARES {
            assert_eq!(
                ground.squares[q] == zero,
                ground.is_blank(q),
                "{method:?} {q}"
            );
        }
        let zeros = ground
            .tiles
            .iter()
            .filter(|t| t.iter().all(|&i| i == 0))
            .count();
        assert_eq!(zeros, 1);
    }
}

#[test]
fn each_interior_storey_builds_its_ground_from_its_own_overlays() {
    // A map of several elevations (map-builder layers) runs the terrain
    // once per storey, over no land, with that storey's floor bake: each
    // ground draws exactly its own overlays, and a storey's ground does
    // not depend on the others or on the order they are built in.
    let world = landless_world();
    let blank = TerrainOptions {
        dither_strength: 0.0,
        blank_without_land: true,
        ..options()
    };
    let lower = rgba_block([0, 0, 255, 255], 40, 20);
    let upper = rgba_block([0, 255, 0, 255], 30, 30);
    let storey = |origin: [i32; 2], width: u32, height: u32, rgba: &[u8]| {
        let overlays = [Overlay {
            origin,
            width,
            height,
            rgba,
        }];
        build(&world, &overlays, &blank)
    };
    let first = storey([3900, 600], 40, 20, &lower);
    let second = storey([4300, 700], 30, 30, &upper);
    let second_alone = storey([4300, 700], 30, 30, &upper);
    assert_eq!(digest(&second), digest(&second_alone));
    for (ground, origin, width, height) in [
        (&first, [3900, 600], 40, 20),
        (&second, [4300, 700], 30, 30),
    ] {
        let drawn = squares_under(origin, width, height);
        assert_eq!(ground.stats.blank_squares, SQUARES - drawn.len());
        for q in 0..SQUARES {
            assert_eq!(ground.is_blank(q), !drawn.contains(&(q as i32)), "{q}");
        }
    }
    // Where one storey has floor the other is blank (drawn black).
    let other = first.compose_rect([4300, 700], 30, 30).expect("compose");
    assert!(other.iter().all(|&i| i == 0));
}

#[test]
fn no_tone_and_the_identity_tone_change_no_byte() {
    let world = gradient_world();
    let cap = TerrainOptions {
        library_cap: 977,
        ..options()
    };
    let before = build(&world, &[], &cap);
    let identity = build(
        &world,
        &[],
        &TerrainOptions {
            tone: Some(foundry_render::Tone::IDENTITY),
            ..cap.clone()
        },
    );
    assert_eq!(digest(&identity), digest(&before));
    assert_eq!(identity.squares, before.squares);
}

#[test]
fn the_tone_darkens_the_land_and_leaves_opaque_overlays_alone() {
    let world = gradient_world();
    let map = palette_map();
    let plain = TerrainOptions {
        dither_strength: 0.0,
        library_cap: SQUARES,
        ..options()
    };
    let dark = TerrainOptions {
        tone: Some(foundry_render::Tone {
            gain: 0.5,
            ..foundry_render::Tone::IDENTITY
        }),
        ..plain.clone()
    };
    let blue = rgba_block([0, 0, 255, 255], 40, 20);
    let overlays = [Overlay {
        origin: [3900, 600],
        width: 40,
        height: 20,
        rgba: &blue,
    }];
    let before = build(&world, &overlays, &plain);
    let after = build(&world, &overlays, &dark);
    let a = before.compose_rect([2000, 400], 200, 100).expect("compose");
    let b = after.compose_rect([2000, 400], 200, 100).expect("compose");
    assert_ne!(a, b, "the land is toned");
    let palette = Palette::parse(&quantize::fixture::synthetic_palette())
        .expect("palette")
        .rgba8();
    let brightness = |pixels: &[u8]| -> u64 {
        pixels
            .iter()
            .map(|&i| {
                palette[usize::from(i)][..3]
                    .iter()
                    .map(|c| u64::from(*c))
                    .sum::<u64>()
            })
            .sum()
    };
    assert!(brightness(&b) < brightness(&a), "half the gain is darker");
    // The opaque overlay is not toned (the slicer tones its own).
    let over_a = before.compose_rect([3900, 600], 40, 20).expect("compose");
    let over_b = after.compose_rect([3900, 600], 40, 20).expect("compose");
    assert_eq!(over_a, over_b);
    assert!(over_b.iter().all(|&i| i == map.nearest([0, 0, 255])));
    // An invalid curve is an option error.
    let bad = TerrainOptions {
        tone: Some(foundry_render::Tone {
            gain: 9.0,
            ..foundry_render::Tone::IDENTITY
        }),
        ..plain
    };
    assert!(matches!(
        bad.validate(),
        Err(TerrainError::InvalidOption { key: "tone", .. })
    ));
}
