//! The map and critter outline, [`quantize::rim`], on the synthetic palette.

use quantize::{
    OutlineOptions, PaletteMap, QuantizeError, RimOptions, fixture, outline_with, quantize, rim,
};
use vault_formats::Palette;

fn map() -> PaletteMap {
    PaletteMap::new(&Palette::parse(&fixture::synthetic_palette()).unwrap())
}

/// A 7 x 5 image: a 3 x 3 square of one mid colour in the middle.
fn square(map: &PaletteMap) -> (Vec<u8>, u32, u32) {
    let colour = quantize(&[150, 110, 70, 255], 1, 1, map, quantize::Dither::None).unwrap()[0];
    let (w, h) = (7u32, 5u32);
    let mut img = vec![0u8; (w * h) as usize];
    for y in 1..4 {
        for x in 2..5 {
            img[y * w as usize + x] = colour;
        }
    }
    (img, w, h)
}

#[test]
fn an_inner_one_pixel_rim_is_the_gallery_outline() {
    let map = map();
    let (img, w, h) = square(&map);
    for factor in [0.0, 0.3, 0.5, 1.0] {
        let mut expected = img.clone();
        outline_with(&mut expected, w, h, &map, &OutlineOptions { factor }).unwrap();
        let got = rim(&img, w, h, &map, &RimOptions::inner(factor)).unwrap();
        assert_eq!(got.pixels, expected, "factor {factor}");
        assert_eq!((got.width, got.height, got.offset), (w, h, [0, 0]));
    }
}

#[test]
fn an_outer_rim_grows_the_image_and_keeps_the_sprite() {
    let map = map();
    let (img, w, h) = square(&map);
    let got = rim(&img, w, h, &map, &RimOptions::outer(0.4)).unwrap();
    assert_eq!(
        (got.width, got.height, got.offset),
        (w + 2, h + 2, [-1, -1])
    );
    let ow = got.width as usize;
    // Every opaque original pixel is unchanged, one pixel in.
    for y in 0..h as usize {
        for x in 0..w as usize {
            let v = img[y * w as usize + x];
            if v != 0 {
                assert_eq!(got.pixels[(y + 1) * ow + x + 1], v);
            }
        }
    }
    // The ring around the square is opaque, darker and allowed; corners of
    // the ring (diagonal only) stay clear at width 1.
    let colour = img[w as usize + 2];
    // Original coordinates: above, left, right and below the square.
    let ring = [
        (2, 0),
        (3, 0),
        (4, 0),
        (1, 1),
        (1, 3),
        (5, 2),
        (2, 4),
        (4, 4),
    ];
    for (x, y) in ring {
        let v = got.pixels[(y + 1) * ow + x + 1];
        assert_ne!(v, 0, "({x}, {y})");
        assert!(map.is_allowed(v));
        assert!(map.oklab(v)[0] < map.oklab(colour)[0]);
    }
    assert_eq!(got.pixels[ow + 2], 0, "diagonal corner");
    // Pixels two away stay transparent.
    assert_eq!(got.pixels[ow * 2 + 1], 0);
}

#[test]
fn a_highlight_rim_lightens_and_a_wide_rim_reaches_further() {
    let map = map();
    let (img, w, h) = square(&map);
    let colour = img[w as usize + 2];
    let lit = rim(&img, w, h, &map, &RimOptions::inner(1.8)).unwrap();
    let edge = lit.pixels[w as usize + 2];
    assert!(map.oklab(edge)[0] > map.oklab(colour)[0]);
    // The centre of a 3 x 3 square is interior at width 1, rim at width 2.
    let centre = 2 * w as usize + 3;
    assert_eq!(lit.pixels[centre], colour);
    let wide = rim(
        &img,
        w,
        h,
        &map,
        &RimOptions {
            width: 2,
            ..RimOptions::inner(0.4)
        },
    )
    .unwrap();
    assert_ne!(wide.pixels[centre], colour);
    let outer = rim(
        &img,
        w,
        h,
        &map,
        &RimOptions {
            width: 2,
            ..RimOptions::outer(0.4)
        },
    )
    .unwrap();
    assert_eq!(
        (outer.width, outer.height, outer.offset),
        (w + 4, h + 4, [-2, -2])
    );
}

#[test]
fn rim_is_deterministic_and_validates() {
    let map = map();
    let (img, w, h) = square(&map);
    let a = rim(&img, w, h, &map, &RimOptions::outer(0.5)).unwrap();
    let b = rim(&img, w, h, &map, &RimOptions::outer(0.5)).unwrap();
    assert_eq!(a, b);
    for bad in [
        RimOptions::inner(-0.1),
        RimOptions::inner(5.0),
        RimOptions::inner(f32::NAN),
        RimOptions {
            width: 0,
            ..RimOptions::default()
        },
        RimOptions {
            width: 3,
            ..RimOptions::default()
        },
    ] {
        assert!(matches!(
            rim(&img, w, h, &map, &bad),
            Err(QuantizeError::InvalidRim(_))
        ));
    }
    assert!(matches!(
        rim(&img, w + 1, h, &map, &RimOptions::default()),
        Err(QuantizeError::BufferLength { .. })
    ));
    // Every index as a lone pixel never becomes reserved.
    let all: Vec<u8> = (1..=255u8).collect();
    let inner = rim(&all, 255, 1, &map, &RimOptions::inner(0.6)).unwrap();
    assert!(inner.pixels.iter().all(|v| map.is_allowed(*v)));
    // An outer rim writes only the ring: the rows above and below.
    let outer = rim(&all, 255, 1, &map, &RimOptions::outer(0.6)).unwrap();
    let ow = outer.width as usize;
    for row in [0, 2] {
        for &v in &outer.pixels[row * ow + 1..row * ow + 256] {
            assert!(map.is_allowed(v), "{v}");
        }
    }
    assert_eq!(&outer.pixels[ow + 1..ow + 256], all.as_slice());
}
