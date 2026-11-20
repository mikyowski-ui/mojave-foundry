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
    assert_eq!((s.textures_loaded, s.textures_missing), (1, 1));
    assert_eq!((s.cells_with_land, s.cells_without_land), (15, 1));
    // The missing tone defaults to the mean of the textures found: here the
    // one texture's own colour.
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
    ];
    for o in bad {
        assert!(
            matches!(o.validate(), Err(TerrainError::InvalidOption { .. })),
            "{o:?}"
        );
    }
    assert!(options().validate().is_ok());
}
