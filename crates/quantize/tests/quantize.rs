//! Behaviour of the public API on the synthetic palette.

use quantize::{
    BAYER, Dither, OutlineOptions, PaletteMap, QuantizeError, Reserved, fixture, oklab_from_rgb8,
    outline, outline_with, quantize, quantize_at, stats, to_rgba,
};
use std::sync::OnceLock;
use vault_formats::Palette;
use vault_formats::palette::{CYCLE_RANGES, RED_PULSE_INDEX};

fn palette() -> &'static Palette {
    static PALETTE: OnceLock<Palette> = OnceLock::new();
    PALETTE.get_or_init(|| Palette::parse(&fixture::synthetic_palette()).unwrap())
}

fn map() -> &'static PaletteMap {
    static MAP: OnceLock<PaletteMap> = OnceLock::new();
    MAP.get_or_init(|| PaletteMap::new(palette()))
}

/// SplitMix64, a small seeded generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn rgb(&mut self) -> [u8; 3] {
        let v = self.next();
        [v as u8, (v >> 8) as u8, (v >> 16) as u8]
    }
}

fn distance2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    distance2(a, b).sqrt()
}

/// Linear search over the allowed set, ties to the lowest index.
fn brute_force(map: &PaletteMap, lab: [f64; 3]) -> u8 {
    let mut best = (f64::INFINITY, 0u8);
    for &index in map.allowed() {
        let d = distance2(lab, map.oklab(index));
        if d < best.0 {
            best = (d, index);
        }
    }
    best.1
}

fn solid(rgb: [u8; 3], width: u32, height: u32) -> Vec<u8> {
    (0..width * height)
        .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
        .collect()
}

fn random_image(seed: u64, width: u32, height: u32) -> Vec<u8> {
    let mut rng = Rng(seed);
    (0..width * height)
        .flat_map(|_| {
            let v = rng.next();
            [v as u8, (v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8]
        })
        .collect()
}

#[test]
fn the_fixture_is_a_valid_palette_with_the_described_layout() {
    let bytes = fixture::synthetic_palette();
    assert_eq!(bytes.len(), 0x8300);
    let palette = palette();
    for index in fixture::UNMAPPED {
        assert!(!palette.is_mapped(index));
    }
    for index in (1..=255u8).filter(|i| !fixture::UNMAPPED.contains(i)) {
        assert!(palette.is_mapped(index), "{index}");
    }
    // The reserved colours appear nowhere among the allowed entries.
    let map = map();
    for &reserved in map.reserved() {
        if palette.is_mapped(reserved) && reserved != 0 {
            let colour = map.colour(reserved);
            assert!(
                map.allowed().iter().all(|&i| map.colour(i) != colour),
                "reserved {reserved} duplicates an allowed colour"
            );
        }
    }
    assert_eq!(
        map.colour(fixture::DUPLICATE_WHITE),
        map.colour(*fixture::GREYS.end())
    );
}

#[test]
fn the_allowed_set_excludes_exactly_the_reserved_indices() {
    let map = map();
    let mut expected = vec![0u8];
    for range in CYCLE_RANGES {
        expected.extend(range.first..=range.last);
    }
    expected.push(RED_PULSE_INDEX as u8);
    expected.extend(fixture::UNMAPPED);
    expected.sort_unstable();
    expected.dedup();
    assert_eq!(map.reserved(), expected.as_slice());
    assert_eq!(map.allowed().len() + map.reserved().len(), 256);
    assert!(map.allowed().windows(2).all(|w| w[0] < w[1]));
    assert_eq!(map.reservation(0), Some(Reserved::Transparent));
    assert_eq!(map.reservation(230), Some(Reserved::ColourCycle));
    assert_eq!(map.reservation(254), Some(Reserved::RedPulse));
    assert_eq!(map.reservation(226), Some(Reserved::Unmapped));
    assert_eq!(map.reservation(1), None);
}

#[test]
fn an_empty_allowed_set_is_an_error() {
    let mut bytes = fixture::synthetic_palette();
    bytes[..768].fill(64);
    let palette = Palette::parse(&bytes).unwrap();
    assert_eq!(
        PaletteMap::try_new(&palette).unwrap_err(),
        QuantizeError::NoAllowedIndex
    );
}

#[test]
fn reserved_colours_are_never_chosen_even_as_exact_matches() {
    let map = map();
    let reserved: Vec<u8> = map
        .reserved()
        .iter()
        .copied()
        .filter(|&i| palette().is_mapped(i) && i != 0)
        .collect();
    assert!(reserved.len() >= 26);
    let mut rgba = Vec::new();
    for &index in &reserved {
        let colour = map.colour(index);
        // The reserved entry really is the nearest palette colour overall.
        let all_nearest = (1..=255u8)
            .filter(|&i| palette().is_mapped(i))
            .min_by(|&a, &b| {
                let ta = distance(oklab_from_rgb8(colour), map.oklab(a));
                let tb = distance(oklab_from_rgb8(colour), map.oklab(b));
                ta.total_cmp(&tb)
            })
            .unwrap();
        assert_eq!(all_nearest, index);
        assert!(map.is_allowed(map.nearest(colour)));
        assert!(map.is_allowed(map.nearest_exact(colour)));
        rgba.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
    }
    let width = reserved.len() as u32;
    for dither in [
        Dither::None,
        Dither::ordered(),
        Dither::Ordered { strength: 1.0 },
    ] {
        let out = quantize(&rgba, width, 1, map, dither).unwrap();
        assert!(out.iter().all(|&i| map.is_allowed(i)), "{dither:?}");
    }
}

#[test]
fn opaque_random_pixels_only_get_allowed_indices_and_transparent_ones_get_zero() {
    let map = map();
    let rgba = random_image(7, 64, 64);
    for dither in [
        Dither::None,
        Dither::ordered(),
        Dither::Ordered { strength: 0.5 },
    ] {
        let out = quantize(&rgba, 64, 64, map, dither).unwrap();
        for (px, &index) in rgba.as_chunks::<4>().0.iter().zip(&out) {
            if px[3] < 128 {
                assert_eq!(index, 0);
            } else {
                assert!(map.is_allowed(index), "{index}");
            }
        }
    }
}

#[test]
fn alpha_threshold_is_128() {
    let map = map();
    let rgba = [200, 100, 50, 127, 200, 100, 50, 128];
    let out = quantize(&rgba, 2, 1, map, Dither::None).unwrap();
    assert_eq!(out[0], 0);
    assert_ne!(out[1], 0);
}

#[test]
fn exact_palette_colours_map_to_themselves() {
    let map = map();
    for &index in map.allowed() {
        let colour = map.colour(index);
        let first = *map
            .allowed()
            .iter()
            .find(|&&i| map.colour(i) == colour)
            .unwrap();
        assert_eq!(map.nearest(colour), first, "index {index}");
        assert_eq!(map.nearest_exact(colour), first);
    }
    // The duplicate white loses the tie to the lower index.
    assert_eq!(map.nearest([252, 252, 252]), *fixture::GREYS.end());
}

#[test]
fn the_table_matches_a_brute_force_search() {
    let map = map();
    let mut rng = Rng(0x5eed);
    for _ in 0..20_000 {
        let rgb = rng.rgb();
        // The table answers for the colour rounded to the palette's 6-bit grid.
        let grid = rgb.map(|c| (((u16::from(c) + 2) / 4).min(63) * 4) as u8);
        assert_eq!(
            map.nearest(rgb),
            brute_force(map, oklab_from_rgb8(grid)),
            "{rgb:?}"
        );
        // The exact search is exact.
        let lab = oklab_from_rgb8(rgb);
        assert_eq!(map.nearest_exact(rgb), brute_force(map, lab), "{rgb:?}");
        // Rounding to the 6-bit grid costs little: the excess is largest
        // near black, where two 8-bit levels are a large step in Oklab.
        let table = distance(lab, map.oklab(map.nearest(rgb)));
        let best = distance(lab, map.oklab(brute_force(map, lab)));
        assert!(table - best < 0.02, "{rgb:?}: {table} vs {best}");
    }
}

#[test]
fn the_bayer_matrix_is_the_standard_one() {
    assert_eq!(BAYER[0], [0, 32, 8, 40, 2, 34, 10, 42]);
    assert_eq!(BAYER[1], [48, 16, 56, 24, 50, 18, 58, 26]);
    let mut all: Vec<u8> = BAYER.iter().flatten().copied().collect();
    all.sort_unstable();
    assert_eq!(all, (0..64).collect::<Vec<u8>>());
}

#[test]
fn ordered_dither_mixes_two_neighbouring_greys() {
    let map = map();
    // Halfway between greys 16 and 17 (6-bit levels 31 and 33): 8-bit 128.
    let (low, high) = (16u8, 17u8);
    assert_eq!(map.colour(low), [124; 3]);
    assert_eq!(map.colour(high), [132; 3]);
    let rgba = solid([128; 3], 16, 16);
    let plain = quantize(&rgba, 16, 16, map, Dither::None).unwrap();
    assert_eq!(
        plain
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1
    );
    let dithered = quantize(&rgba, 16, 16, map, Dither::ordered()).unwrap();
    let lows = dithered.iter().filter(|&&i| i == low).count();
    let highs = dithered.iter().filter(|&&i| i == high).count();
    assert_eq!(lows + highs, 256, "only the two neighbours are used");
    assert!(lows > 64 && highs > 64, "{lows} low, {highs} high");
}

#[test]
fn zero_strength_equals_no_dither() {
    let map = map();
    let rgba = random_image(11, 40, 30);
    assert_eq!(
        quantize(&rgba, 40, 30, map, Dither::Ordered { strength: 0.0 }).unwrap(),
        quantize(&rgba, 40, 30, map, Dither::None).unwrap()
    );
}

#[test]
fn invalid_strength_is_rejected() {
    let map = map();
    let rgba = solid([1, 2, 3], 1, 1);
    for strength in [-0.1, f32::NAN, f32::INFINITY] {
        assert!(matches!(
            quantize(&rgba, 1, 1, map, Dither::Ordered { strength }),
            Err(QuantizeError::InvalidStrength(_))
        ));
    }
}

#[test]
fn adjacent_tiles_dither_like_one_image() {
    let map = map();
    let (width, height) = (24u32, 13u32);
    let big = random_image(3, 2 * width, 2 * height);
    let origin = [-37, 5];
    let whole = quantize_at(&big, 2 * width, 2 * height, map, Dither::ordered(), origin).unwrap();
    let tile = |tx: u32, ty: u32| -> Vec<u8> {
        let mut rgba = Vec::new();
        for y in 0..height {
            let row = ((ty * height + y) * 2 * width + tx * width) as usize * 4;
            rgba.extend_from_slice(&big[row..row + width as usize * 4]);
        }
        let at = [
            origin[0] + (tx * width) as i32,
            origin[1] + (ty * height) as i32,
        ];
        quantize_at(&rgba, width, height, map, Dither::ordered(), at).unwrap()
    };
    for ty in 0..2 {
        for tx in 0..2 {
            let part = tile(tx, ty);
            for y in 0..height {
                let row = ((ty * height + y) * 2 * width + tx * width) as usize;
                assert_eq!(
                    &part[(y * width) as usize..((y + 1) * width) as usize],
                    &whole[row..row + width as usize],
                    "tile ({tx}, {ty}) row {y}"
                );
            }
        }
    }
    // A shifted origin changes the pattern; one shifted by 8 does not.
    let base = quantize_at(&big, 2 * width, 2 * height, map, Dither::ordered(), [0, 0]).unwrap();
    let period = quantize_at(
        &big,
        2 * width,
        2 * height,
        map,
        Dither::ordered(),
        [8, -16],
    )
    .unwrap();
    assert_eq!(base, period);
}

#[test]
fn bad_buffer_lengths_are_rejected() {
    let map = map();
    let rgba = vec![0u8; 4 * 6];
    assert_eq!(
        quantize(&rgba, 2, 4, map, Dither::None).unwrap_err(),
        QuantizeError::BufferLength {
            width: 2,
            height: 4,
            expected: 32,
            actual: 24
        }
    );
    assert!(quantize(&rgba[..23], 2, 3, map, Dither::None).is_err());
    assert!(quantize(&rgba, u32::MAX, u32::MAX, map, Dither::None).is_err());
    let mut indexed = vec![1u8; 5];
    assert!(outline(&mut indexed, 2, 3, map).is_err());
    assert!(stats(&rgba, &indexed, 2, 3, map).is_err());
    assert_eq!(
        quantize(&[], 0, 7, map, Dither::None).unwrap(),
        Vec::<u8>::new()
    );
    assert!(outline(&mut [], 0, 0, map).is_ok());
}

/// A filled disc of one colour on a transparent background.
fn disc(map: &PaletteMap, colour: [u8; 3], size: u32) -> Vec<u8> {
    let index = map.nearest(colour);
    let centre = (size as f64 - 1.0) / 2.0;
    let radius = size as f64 / 2.0 - 2.0;
    (0..size * size)
        .map(|i| {
            let (x, y) = (f64::from(i % size), f64::from(i / size));
            if (x - centre).powi(2) + (y - centre).powi(2) <= radius * radius {
                index
            } else {
                0
            }
        })
        .collect()
}

#[test]
fn outline_darkens_the_edge_and_leaves_the_rest() {
    let map = map();
    let size = 21u32;
    let before = disc(map, [180, 120, 60], size);
    let mut after = before.clone();
    outline(&mut after, size, size, map).unwrap();
    let w = size as usize;
    let mut edges = 0;
    for (at, (&b, &a)) in before.iter().zip(&after).enumerate() {
        assert_eq!(b == 0, a == 0, "silhouette changed at {at}");
        if b == 0 {
            continue;
        }
        assert!(map.is_allowed(a));
        let (x, y) = (at % w, at / w);
        let edge = x == 0
            || y == 0
            || x == w - 1
            || y == w - 1
            || before[at - 1] == 0
            || before[at + 1] == 0
            || before[at - w] == 0
            || before[at + w] == 0;
        if edge {
            edges += 1;
            assert!(map.oklab(a)[0] < map.oklab(b)[0], "edge at {at} not darker");
        } else {
            assert_eq!(a, b, "interior changed at {at}");
        }
    }
    assert!(edges > 20);
}

#[test]
fn outline_handles_thin_shapes_and_the_border() {
    let map = map();
    let colour = map.nearest([150, 150, 150]);
    // A one-pixel-wide plus sign touching the image border.
    let (w, h) = (5u32, 5u32);
    let mut img = vec![0u8; 25];
    for i in 0..5 {
        img[2 * 5 + i] = colour;
        img[i * 5 + 2] = colour;
    }
    let before = img.clone();
    outline(&mut img, w, h, map).unwrap();
    for (at, (&b, &a)) in before.iter().zip(&img).enumerate() {
        assert_eq!(b == 0, a == 0);
        if at == 12 {
            // The centre has four opaque neighbours: interior.
            assert_eq!(a, b);
        } else if b != 0 {
            assert!(map.oklab(a)[0] < map.oklab(b)[0], "pixel {at}");
        }
    }
    // A fully opaque image: only the border ring is an edge.
    let mut full = vec![colour; 9];
    outline(&mut full, 3, 3, map).unwrap();
    assert_eq!(full[4], colour);
    assert!(full.iter().enumerate().all(|(i, &v)| i == 4 || v != colour));
    // A single pixel.
    let mut one = [colour];
    outline(&mut one, 1, 1, map).unwrap();
    assert_ne!(one[0], colour);
}

#[test]
fn outline_never_brightens_or_produces_reserved_indices() {
    let map = map();
    // Every index, including reserved and unmapped ones, as a lone pixel.
    let mut img: Vec<u8> = (1..=255u8).collect();
    let before = img.clone();
    outline_with(&mut img, 255, 1, map, &OutlineOptions { factor: 0.7 }).unwrap();
    for (&b, &a) in before.iter().zip(&img) {
        assert!(map.is_allowed(a), "{b} became reserved {a}");
        if map.is_allowed(b) {
            assert!(map.oklab(a)[0] <= map.oklab(b)[0]);
        }
    }
    // Factor 1 changes no allowed pixel; out-of-range factors are errors.
    let mut same: Vec<u8> = map.allowed().to_vec();
    let width = same.len() as u32;
    outline_with(&mut same, width, 1, map, &OutlineOptions { factor: 1.0 }).unwrap();
    assert_eq!(same, map.allowed());
    for factor in [-0.5, 1.5, f32::NAN] {
        assert!(matches!(
            outline_with(&mut same, width, 1, map, &OutlineOptions { factor }),
            Err(QuantizeError::InvalidFactor(_))
        ));
    }
}

#[test]
fn to_rgba_round_trips() {
    let map = map();
    // Every allowed index whose colour is not a duplicate, plus transparency.
    let mut indexed = vec![0u8];
    for &i in map.allowed() {
        if map.nearest(map.colour(i)) == i {
            indexed.push(i);
        }
    }
    let rgba = to_rgba(&indexed, palette());
    assert_eq!(rgba.len(), indexed.len() * 4);
    assert_eq!(&rgba[..4], &[0, 0, 0, 0]);
    let width = indexed.len() as u32;
    assert_eq!(
        quantize(&rgba, width, 1, map, Dither::None).unwrap(),
        indexed
    );
    for (px, &i) in rgba.as_chunks::<4>().0.iter().zip(&indexed).skip(1) {
        let [r, g, b, a] = palette().rgba8()[usize::from(i)];
        assert_eq!(px, &[r, g, b, a]);
        assert_eq!(px[3], 255);
    }
}

#[test]
fn stats_report_error_and_histogram() {
    let map = map();
    let mut rgba = solid([124, 124, 124], 4, 1);
    rgba[7] = 0; // pixel 1 transparent
    rgba[8] = 128; // pixel 2 between two greys
    rgba[9] = 128;
    rgba[10] = 128;
    let indexed = quantize(&rgba, 4, 1, map, Dither::None).unwrap();
    let s = stats(&rgba, &indexed, 4, 1, map).unwrap();
    assert_eq!(s.opaque_pixels, 3);
    assert_eq!(s.transparency_mismatches, 0);
    assert_eq!(s.histogram.iter().sum::<u64>(), 4);
    assert_eq!(s.histogram[0], 1);
    assert!(s.max_error > 0.0 && s.max_error < 0.02);
    assert!((s.mean_error - s.max_error / 3.0).abs() < 1e-12);
    assert_eq!(s.used_indices().first(), Some(&0));
    let mut wrong = indexed.clone();
    wrong[1] = 5;
    assert_eq!(
        stats(&rgba, &wrong, 4, 1, map)
            .unwrap()
            .transparency_mismatches,
        1
    );
}

#[test]
fn everything_is_deterministic() {
    let a = PaletteMap::new(palette());
    let b = PaletteMap::new(&Palette::parse(&fixture::synthetic_palette()).unwrap());
    let mut rng = Rng(99);
    for _ in 0..5000 {
        let rgb = rng.rgb();
        assert_eq!(a.nearest(rgb), b.nearest(rgb));
    }
    assert_eq!(fixture::synthetic_palette(), fixture::synthetic_palette());
    let rgba = random_image(42, 33, 17);
    for dither in [Dither::None, Dither::ordered()] {
        let first = quantize(&rgba, 33, 17, &a, dither).unwrap();
        let second = quantize(&rgba, 33, 17, &b, dither).unwrap();
        assert_eq!(first, second);
        let mut o1 = first.clone();
        let mut o2 = second;
        outline(&mut o1, 33, 17, &a).unwrap();
        outline(&mut o2, 33, 17, &b).unwrap();
        assert_eq!(o1, o2);
    }
}
