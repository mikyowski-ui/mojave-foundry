mod common;

use common::*;
use foundry_gallery::{
    Canvas, GalleryError, Rect, decode_png, draw_text, encode_png, frm_frame_rgba, indexed_to_rgba,
    sprite_metrics, text_width,
};
use vault_formats::{FrmArchive, Palette};

// ---- font on a canvas ----

#[test]
fn text_clips_at_every_edge() {
    let mut buffer = vec![0u8; 10 * 10 * 4];
    let mut canvas = Canvas::new(&mut buffer, 10, 10).unwrap();
    let white = [255, 255, 255, 255];
    // 'H' row 0 is columns 0 and 4: at x = -3 only column 4 (x = 1) shows.
    let drawn = draw_text(&mut canvas, -3, 0, "H", white, 1);
    assert_eq!(drawn, 5);
    assert_eq!(canvas.pixel(1, 0), Some(white));
    assert_eq!(canvas.pixel(0, 0), Some([0, 0, 0, 0]));
    // Far off every edge, and huge scales: nothing drawn, no panic.
    for (x, y) in [
        (-1000, 0),
        (1000, 0),
        (0, -1000),
        (0, 1000),
        (i32::MIN, i32::MAX),
    ] {
        draw_text(&mut canvas, x, y, "WWWW", [1, 2, 3, 255], 1);
    }
    draw_text(&mut canvas, 8, 8, "~~~~~~~~~~", [9, 9, 9, 255], 1000);
    draw_text(&mut canvas, 0, 0, "x", [9, 9, 9, 255], 0);
    assert!(
        buffer
            .as_chunks::<4>()
            .0
            .iter()
            .all(|px| *px != [1, 2, 3, 255])
    );
    // Bottom-right clipping: an 'H' at (8, 8) leaves ink at (8, 8) and (8, 9).
    let mut buffer = vec![0u8; 10 * 10 * 4];
    let mut canvas = Canvas::new(&mut buffer, 10, 10).unwrap();
    draw_text(&mut canvas, 8, 8, "H", white, 1);
    assert_eq!(canvas.pixel(8, 8), Some(white));
    assert_eq!(canvas.pixel(8, 9), Some(white));
    assert_eq!(canvas.pixel(9, 8), Some([0, 0, 0, 0]));
}

#[test]
fn scaled_text_uses_square_blocks() {
    let mut buffer = vec![0u8; 20 * 20 * 4];
    let mut canvas = Canvas::new(&mut buffer, 20, 20).unwrap();
    let ink = [255, 0, 0, 255];
    // '|' is column 2 on every row: at scale 2 it fills x 4..6, y 0..14.
    draw_text(&mut canvas, 0, 0, "|", ink, 2);
    for y in 0..20 {
        for x in 0..20 {
            let expected = if (4..6).contains(&x) && y < 14 {
                ink
            } else {
                [0; 4]
            };
            assert_eq!(canvas.pixel(x, y), Some(expected), "({x},{y})");
        }
    }
    assert_eq!(text_width("||", 2), 22);
}

#[test]
fn canvas_rejects_bad_lengths() {
    let mut buffer = vec![0u8; 15];
    assert!(matches!(
        Canvas::new(&mut buffer, 2, 2),
        Err(GalleryError::BufferLength {
            expected: 16,
            actual: 15,
            ..
        })
    ));
}

// ---- PNG ----

/// Chunk types of a PNG stream, in order.
fn chunk_types(png: &[u8]) -> Vec<String> {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let mut out = Vec::new();
    let mut at = 8;
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        out.push(String::from_utf8_lossy(&png[at + 4..at + 8]).into_owned());
        at += 12 + len;
    }
    out
}

#[test]
fn png_round_trip_is_exact_and_deterministic() {
    let mut image = gradient(37, 23);
    for (i, px) in image.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        px[3] = (i * 11) as u8;
    }
    let first = encode_png(&image, 37, 23).unwrap();
    let second = encode_png(&image, 37, 23).unwrap();
    assert_eq!(first, second);
    let chunks = chunk_types(&first);
    assert_eq!(chunks.first().map(String::as_str), Some("IHDR"));
    assert_eq!(chunks.last().map(String::as_str), Some("IEND"));
    assert!(
        chunks
            .iter()
            .all(|c| c == "IHDR" || c == "IDAT" || c == "IEND"),
        "{chunks:?}"
    );
    let decoded = decode_png(&first).unwrap();
    assert_eq!((decoded.width, decoded.height), (37, 23));
    assert_eq!(decoded.rgba, image);

    // Through a file, as callers do.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sheet.png");
    std::fs::write(&path, &first).unwrap();
    assert_eq!(
        decode_png(&std::fs::read(&path).unwrap()).unwrap().rgba,
        image
    );
}

#[test]
fn png_decodes_other_colour_types_to_rgba() {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 3, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
            .unwrap();
    }
    let image = decode_png(&bytes).unwrap();
    assert_eq!(image.rgba, [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255]);

    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0x12, 0x34, 0xAB, 0xCD]).unwrap();
    }
    let image = decode_png(&bytes).unwrap();
    assert_eq!(image.rgba, [0x12, 0x12, 0x12, 255, 0xAB, 0xAB, 0xAB, 255]);
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[test]
fn png_bad_input_is_an_error() {
    let image = solid(2, 2, [1, 2, 3, 4]);
    assert!(matches!(
        encode_png(&image[..15], 2, 2),
        Err(GalleryError::BufferLength { .. })
    ));
    assert!(matches!(
        encode_png(&[], 0, 5),
        Err(GalleryError::EmptyImage { .. })
    ));
    assert!(matches!(
        decode_png(b"not a png"),
        Err(GalleryError::PngDecode(_))
    ));
    let good = encode_png(&image, 2, 2).unwrap();
    assert!(decode_png(&good[..good.len() - 20]).is_err());

    // A valid header claiming 100000 x 100000 is refused before allocation.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = b"IHDR".to_vec();
    ihdr.extend_from_slice(&100_000u32.to_be_bytes());
    ihdr.extend_from_slice(&100_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(&ihdr);
    png.extend_from_slice(&crc32(&ihdr).to_be_bytes());
    // A token IDAT, so the header is read in full; its data never is.
    let idat = b"IDAT\x78\x9c\x03\x00\x00\x00\x00\x01";
    png.extend_from_slice(&8u32.to_be_bytes());
    png.extend_from_slice(idat);
    png.extend_from_slice(&crc32(idat).to_be_bytes());
    let iend = b"IEND";
    png.extend_from_slice(&0u32.to_be_bytes());
    png.extend_from_slice(iend);
    png.extend_from_slice(&crc32(iend).to_be_bytes());
    let err = decode_png(&png).unwrap_err();
    assert!(matches!(err, GalleryError::TooLarge { .. }), "{err:?}");
}

// ---- reference art ----

/// A synthetic palette: index i is (i % 64, (i * 3) % 64, 63 - i % 64),
/// except index 7, whose out-of-range triple leaves it unmapped.
fn synthetic_palette() -> Palette {
    let mut bytes = vec![0u8; 0x8300];
    for i in 0..256usize {
        let triple = [(i % 64) as u8, ((i * 3) % 64) as u8, (63 - i % 64) as u8];
        bytes[i * 3..i * 3 + 3].copy_from_slice(&triple);
    }
    bytes[21..24].copy_from_slice(&[64, 64, 64]);
    Palette::parse(&bytes).unwrap()
}

/// A one-direction FRM: big-endian header (version, fps, action frame,
/// frame count, six x shifts, six y shifts, six offsets all 0 so every
/// direction shares one sequence, frame-area size), then each frame
/// (width, height, pixel count, x and y displacement, pixels).
fn synthetic_frm(frames: &[(u16, u16, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&4i32.to_be_bytes());
    out.extend_from_slice(&10i16.to_be_bytes());
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&(frames.len() as i16).to_be_bytes());
    out.extend_from_slice(&[0u8; 24]);
    out.extend_from_slice(&[0u8; 24]);
    let area: usize = frames.iter().map(|(_, _, p)| 12 + p.len()).sum();
    out.extend_from_slice(&(area as i32).to_be_bytes());
    assert_eq!(out.len(), 62);
    for (width, height, pixels) in frames {
        out.extend_from_slice(&(*width as i16).to_be_bytes());
        out.extend_from_slice(&(*height as i16).to_be_bytes());
        out.extend_from_slice(&(pixels.len() as i32).to_be_bytes());
        out.extend_from_slice(&1i16.to_be_bytes());
        out.extend_from_slice(&(-2i16).to_be_bytes());
        out.extend_from_slice(pixels);
    }
    out
}

#[test]
fn reference_frames_convert_through_the_palette() {
    let palette = synthetic_palette();
    let first: &[u8] = &[0, 1, 2, 63, 0, 7];
    let second: &[u8] = &[5, 5];
    let frm = FrmArchive::parse(&synthetic_frm(&[(3, 2, first), (1, 2, second)])).unwrap();
    let (rgba, width, height) = frm_frame_rgba(&frm, 0, 0, &palette).unwrap();
    assert_eq!((width, height), (3, 2));
    let expected: Vec<u8> = [
        [0, 0, 0, 0],       // index 0: transparent
        [4, 12, 248, 255],  // (1, 3, 62) x 4
        [8, 24, 244, 255],  // (2, 6, 61) x 4
        [252, 244, 0, 255], // (63, 189 % 64 = 61, 0) x 4
        [0, 0, 0, 0],
        [0, 0, 0, 255], // unmapped: opaque black
    ]
    .concat();
    assert_eq!(rgba, expected);

    // Every direction shares the one stored sequence.
    let (rgba, width, height) = frm_frame_rgba(&frm, 5, 1, &palette).unwrap();
    assert_eq!((width, height), (1, 2));
    assert_eq!(rgba, [20, 60, 232, 255, 20, 60, 232, 255]);
    assert!(frm_frame_rgba(&frm, 0, 2, &palette).is_none());
    assert!(frm_frame_rgba(&frm, 6, 0, &palette).is_none());

    // The helper output drops straight into a comparison tile.
    let new = solid(3, 2, [9, 9, 9, 255]);
    let (reference, rw, rh) = frm_frame_rgba(&frm, 0, 0, &palette).unwrap();
    let tiles = [foundry_gallery::Tile {
        label: "frm",
        compare: Some((&reference, rw, rh)),
        ..tile(&new, 3, 2)
    }];
    let sheet = foundry_gallery::contact_sheet(&tiles, &small_options()).unwrap();
    let c = sheet.cells[0].compare.unwrap();
    assert_eq!(sheet.pixel(c.x + 1, c.y), Some([4, 12, 248, 255]));
    assert_eq!(sheet.pixel(c.x, c.y), Some(BACK));
}

#[test]
fn indexed_conversion_checks_length() {
    let palette = synthetic_palette();
    assert!(matches!(
        indexed_to_rgba(&[1, 2, 3], 2, 2, &palette),
        Err(GalleryError::BufferLength {
            expected: 4,
            actual: 3,
            ..
        })
    ));
    assert_eq!(
        indexed_to_rgba(&[], 0, 0, &palette).unwrap(),
        Vec::<u8>::new()
    );
}

// ---- metrics ----

#[test]
fn sprite_metrics_count_opaque_pixels() {
    let mut image = solid(5, 4, [0, 0, 0, 0]);
    let mut set = |x: usize, y: usize, colour: [u8; 4]| {
        let at = (y * 5 + x) * 4;
        image[at..at + 4].copy_from_slice(&colour);
    };
    set(1, 1, [10, 20, 30, 255]);
    set(3, 2, [10, 20, 30, 200]); // same RGB
    set(2, 1, [1, 1, 1, 128]); // threshold counts
    set(4, 3, [9, 9, 9, 127]); // below threshold
    let metrics = sprite_metrics(&image, 5, 4).unwrap();
    assert_eq!(metrics.opaque_pixels, 3);
    assert_eq!(
        metrics.bounds,
        Some(Rect {
            x: 1,
            y: 1,
            width: 3,
            height: 2
        })
    );
    assert_eq!(metrics.distinct_colours, 2);
    let lines = metrics.lines();
    let keys: Vec<&str> = lines.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["size", "opaque", "bbox", "colours"]);
    assert_eq!(lines[0].1, "5x4");
    assert_eq!(lines[2].1, "1,1 3x2");

    let empty = sprite_metrics(&solid(2, 2, [0; 4]), 2, 2).unwrap();
    assert_eq!(empty.bounds, None);
    assert_eq!(empty.lines()[2].1, "none");
    assert!(sprite_metrics(&[0; 7], 1, 2).is_err());
}
