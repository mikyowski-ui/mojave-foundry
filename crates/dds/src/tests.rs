//! Tests on synthetic textures only. Nothing here reads a game file.

use crate::fixture::{
    DDPF_ALPHA, DDPF_ALPHAPIXELS, DDPF_LUMINANCE, DDPF_RGB, DdsFile, PixelFormat,
    UncompressedLayout, encode_alpha_block, encode_bc1, encode_bc2, encode_bc3, encode_bc5,
    encode_color_block, encode_uncompressed, pack_565,
};
use crate::{DdsError, DdsFormat, MAX_DIMENSION, decode, info};

fn fourcc_file(code: &[u8; 4], width: u32, height: u32, data: &[u8]) -> Vec<u8> {
    DdsFile::new(width, height, PixelFormat::FourCc(*code), data.to_vec()).to_bytes()
}

fn masks_file(
    flags: u32,
    bit_count: u32,
    masks: [u32; 4],
    width: u32,
    height: u32,
    data: &[u8],
) -> Vec<u8> {
    let pixel_format = PixelFormat::Masks {
        flags,
        bit_count,
        r_mask: masks[0],
        g_mask: masks[1],
        b_mask: masks[2],
        a_mask: masks[3],
    };
    DdsFile::new(width, height, pixel_format, data.to_vec()).to_bytes()
}

fn pixels(bytes: &[u8]) -> Vec<[u8; 4]> {
    decode(bytes)
        .unwrap()
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect()
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// Index bytes of a colour block: rows `0 1 2 3`, `3 2 1 0`, `0 0 0 0`,
/// `2 2 2 2`, two bits per texel from the low end.
const INDEX_ROWS: [u8; 4] = [0xE4, 0x1B, 0x00, 0xAA];

fn by_index_rows(palette: [[u8; 4]; 4]) -> Vec<[u8; 4]> {
    [0, 1, 2, 3, 3, 2, 1, 0, 0, 0, 0, 0, 2, 2, 2, 2]
        .iter()
        .map(|&i| palette[i])
        .collect()
}

/// Three-bit indices `0..=7` twice, packed from the low end.
const ALPHA_INDEX_BYTES: [u8; 6] = [0x88, 0xC6, 0xFA, 0x88, 0xC6, 0xFA];

fn gradient(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .flat_map(|(x, y)| f(x, y))
        .collect()
}

fn max_channel_error(a: &[u8], b: &[u8], channel: usize) -> u8 {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0.iter())
        .map(|(p, q)| p[channel].abs_diff(q[channel]))
        .max()
        .unwrap()
}

// --------------------------------------------------------- pinned blocks

#[test]
fn bc1_four_colour_block() {
    // c0 = pure red (0xF800) > c1 = pure blue (0x001F).
    let mut block = vec![0x00, 0xF8, 0x1F, 0x00];
    block.extend_from_slice(&INDEX_ROWS);
    let file = fourcc_file(b"DXT1", 4, 4, &block);
    assert_eq!(
        pixels(&file),
        by_index_rows([
            [255, 0, 0, 255],
            [0, 0, 255, 255],
            [170, 0, 85, 255],
            [85, 0, 170, 255],
        ])
    );
}

#[test]
fn bc1_three_colour_block_with_transparency() {
    // c0 = pure blue (0x001F) < c1 = pure red (0xF800).
    let mut block = vec![0x1F, 0x00, 0x00, 0xF8];
    block.extend_from_slice(&INDEX_ROWS);
    let file = fourcc_file(b"DXT1", 4, 4, &block);
    assert_eq!(
        pixels(&file),
        by_index_rows([
            [0, 0, 255, 255],
            [255, 0, 0, 255],
            [127, 0, 127, 255],
            [0, 0, 0, 0],
        ])
    );
}

#[test]
fn bc1_equal_endpoints_use_the_three_colour_mode() {
    let mut block = vec![0xFF, 0xFF, 0xFF, 0xFF];
    block.extend_from_slice(&INDEX_ROWS);
    let white = [255, 255, 255, 255];
    assert_eq!(
        pixels(&fourcc_file(b"DXT1", 4, 4, &block)),
        by_index_rows([white, white, white, [0, 0, 0, 0]])
    );
}

#[test]
fn bc1_endpoints_widen_by_bit_replication() {
    // 0x8410 is r 16/31, g 32/63, b 16/31; 0x0841 is r 1, g 2, b 1.
    let mut block = vec![0x10, 0x84, 0x41, 0x08];
    block.extend_from_slice(&INDEX_ROWS);
    assert_eq!(
        pixels(&fourcc_file(b"DXT1", 4, 4, &block)),
        by_index_rows([
            [132, 130, 132, 255],
            [8, 8, 8, 255],
            [90, 89, 90, 255],
            [49, 48, 49, 255],
        ])
    );
}

#[test]
fn bc2_block_has_explicit_alpha_and_always_four_colours() {
    // Alpha nibbles 0..=15 in texel order, low nibble first.
    let mut block = vec![0x10, 0x32, 0x54, 0x76, 0x98, 0xBA, 0xDC, 0xFE];
    // c0 < c1, which in BC1 would select the three-colour mode.
    block.extend_from_slice(&[0x1F, 0x00, 0x00, 0xF8]);
    block.extend_from_slice(&INDEX_ROWS);
    let colours = by_index_rows([
        [0, 0, 255, 255],
        [255, 0, 0, 255],
        [85, 0, 170, 255],
        [170, 0, 85, 255],
    ]);
    let expected: Vec<[u8; 4]> = colours
        .iter()
        .enumerate()
        .map(|(i, c)| [c[0], c[1], c[2], i as u8 * 17])
        .collect();
    assert_eq!(pixels(&fourcc_file(b"DXT3", 4, 4, &block)), expected);
}

fn bc3_block(a0: u8, a1: u8) -> Vec<u8> {
    let mut block = vec![a0, a1];
    block.extend_from_slice(&ALPHA_INDEX_BYTES);
    // White endpoints, c0 == c1: every colour is white in BC3.
    block.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    block.extend_from_slice(&INDEX_ROWS);
    block
}

fn alphas(file: &[u8]) -> Vec<u8> {
    let decoded = pixels(file);
    assert!(decoded.iter().all(|p| p[..3] == [255, 255, 255]));
    decoded.iter().map(|p| p[3]).collect()
}

#[test]
fn bc3_alpha_eight_value_mode() {
    let twice = |v: [u8; 8]| [v, v].concat();
    assert_eq!(
        alphas(&fourcc_file(b"DXT5", 4, 4, &bc3_block(255, 0))),
        twice([255, 0, 218, 182, 145, 109, 72, 36])
    );
    assert_eq!(
        alphas(&fourcc_file(b"DXT5", 4, 4, &bc3_block(200, 10))),
        twice([200, 10, 172, 145, 118, 91, 64, 37])
    );
}

#[test]
fn bc3_alpha_six_value_mode() {
    let twice = |v: [u8; 8]| [v, v].concat();
    assert_eq!(
        alphas(&fourcc_file(b"DXT5", 4, 4, &bc3_block(0, 255))),
        twice([0, 255, 51, 102, 153, 204, 0, 255])
    );
    assert_eq!(
        alphas(&fourcc_file(b"DXT5", 4, 4, &bc3_block(10, 200))),
        twice([10, 200, 48, 86, 124, 162, 0, 255])
    );
    // Equal endpoints are the six-value mode too.
    assert_eq!(
        alphas(&fourcc_file(b"DXT5", 4, 4, &bc3_block(77, 77))),
        twice([77, 77, 77, 77, 77, 77, 0, 255])
    );
}

#[test]
fn bc5_block_rebuilds_blue_from_red_and_green() {
    // Red: endpoints 255 and 0 with indices 0..=7 twice. Green: 128
    // everywhere (equal endpoints, index 0).
    let mut block = vec![255, 0];
    block.extend_from_slice(&ALPHA_INDEX_BYTES);
    block.extend_from_slice(&[128, 128, 0, 0, 0, 0, 0, 0]);
    let red = [255, 0, 218, 182, 145, 109, 72, 36];
    let blue = [128, 128, 217, 243, 254, 254, 242, 216];
    let expected: Vec<[u8; 4]> = (0..16)
        .map(|i| [red[i % 8], 128, blue[i % 8], 255])
        .collect();
    for code in [b"ATI2", b"BC5U"] {
        let file = fourcc_file(code, 4, 4, &block);
        assert_eq!(info(&file).unwrap().format, DdsFormat::Bc5);
        assert_eq!(pixels(&file), expected);
    }

    // The first block is red and the second green, not the reverse.
    let mut block = vec![40, 40, 0, 0, 0, 0, 0, 0];
    block.extend_from_slice(&[90, 90, 0, 0, 0, 0, 0, 0]);
    let decoded = pixels(&fourcc_file(b"ATI2", 4, 4, &block));
    assert_eq!(decoded[0][0], 40);
    assert_eq!(decoded[0][1], 90);
}

#[test]
fn normal_blue_values() {
    use crate::bc::normal_blue;
    assert_eq!(normal_blue(128, 128), 255);
    assert_eq!(normal_blue(127, 127), 255);
    assert_eq!(normal_blue(255, 128), 128);
    assert_eq!(normal_blue(0, 0), 128);
    assert_eq!(normal_blue(255, 255), 128);
    assert_eq!(normal_blue(200, 60), 208);
}

// ------------------------------------------------------ block geometry

#[test]
fn blocks_are_laid_out_row_major_from_the_top_left() {
    // 8 x 8: four blocks, each a flat colour (index 0 everywhere).
    let flat = |c: u16| {
        let mut block = c.to_le_bytes().to_vec();
        block.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        block
    };
    let data = [flat(0xF800), flat(0x07E0), flat(0x001F), flat(0xFFFF)].concat();
    let image = decode(&fourcc_file(b"DXT1", 8, 8, &data)).unwrap();
    let at = |x: usize, y: usize| {
        let i = (y * 8 + x) * 4;
        [
            image.rgba[i],
            image.rgba[i + 1],
            image.rgba[i + 2],
            image.rgba[i + 3],
        ]
    };
    assert_eq!(at(0, 0), [255, 0, 0, 255]);
    assert_eq!(at(3, 3), [255, 0, 0, 255]);
    assert_eq!(at(4, 0), [0, 255, 0, 255]);
    assert_eq!(at(7, 3), [0, 255, 0, 255]);
    assert_eq!(at(0, 4), [0, 0, 255, 255]);
    assert_eq!(at(3, 7), [0, 0, 255, 255]);
    assert_eq!(at(4, 4), [255, 255, 255, 255]);
    assert_eq!(at(7, 7), [255, 255, 255, 255]);
}

#[test]
fn dimensions_that_are_not_multiples_of_four() {
    // 5 x 3 needs two blocks; only the top-left texels are kept.
    let mut left = vec![0x00, 0xF8, 0x1F, 0x00];
    left.extend_from_slice(&INDEX_ROWS);
    let mut right = vec![0x00, 0xF8, 0x1F, 0x00];
    right.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
    let file = fourcc_file(b"DXT1", 5, 3, &[left, right].concat());
    let image = decode(&file).unwrap();
    assert_eq!((image.width, image.height), (5, 3));
    assert_eq!(image.rgba.len(), 5 * 3 * 4);
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    let third = [170, 0, 85, 255];
    let two_thirds = [85, 0, 170, 255];
    assert_eq!(
        pixels(&file),
        vec![
            red, blue, third, two_thirds, blue, // row 0
            two_thirds, third, blue, red, red, // row 1
            red, red, red, red, blue, // row 2
        ]
    );

    // One block short is a truncation, not a short read.
    let short = fourcc_file(b"DXT1", 5, 3, &[0u8; 8]);
    assert_eq!(
        decode(&short).unwrap_err(),
        DdsError::Truncated {
            offset: 128,
            what: "top mip level",
            needed: 16,
            available: 8,
        }
    );
}

#[test]
fn encoders_round_trip_odd_sizes() {
    for (width, height) in [(1, 1), (2, 3), (5, 3), (4, 4), (7, 9), (13, 6), (16, 16)] {
        // A flat colour that 565 holds exactly, with alpha 0 or 255.
        let colour = |x: u32, y: u32| {
            if (x / 4 + y / 4).is_multiple_of(2) {
                [255, 0, 0, 255]
            } else {
                [0, 130, 255, 255]
            }
        };
        let source = gradient(width, height, colour);
        for file in [
            encode_bc1(width, height, &source),
            encode_bc2(width, height, &source),
            encode_bc3(width, height, &source),
        ] {
            let image = decode(&file).unwrap();
            assert_eq!((image.width, image.height), (width, height));
            assert_eq!(image.rgba, source, "{width} x {height}");
        }
        let two_channel = decode(&encode_bc5(width, height, &source)).unwrap();
        for (out, src) in two_channel
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(source.as_chunks::<4>().0.iter())
        {
            assert_eq!(out[..2], src[..2]);
            assert_eq!(out[2], crate::bc::normal_blue(src[0], src[1]));
            assert_eq!(out[3], 255);
        }
    }
}

// ------------------------------------------------------------- encoders

#[test]
fn bc1_encoder_writes_punch_through_alpha() {
    let source = gradient(8, 8, |x, y| {
        if (x + y) % 2 == 0 {
            [255, 255, 255, 255]
        } else {
            [9, 9, 9, 0]
        }
    });
    let image = decode(&encode_bc1(8, 8, &source)).unwrap();
    for (out, src) in image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(source.as_chunks::<4>().0.iter())
    {
        if src[3] == 0 {
            assert_eq!(*out, [0, 0, 0, 0]);
        } else {
            assert_eq!(*out, [255, 255, 255, 255]);
        }
    }

    // A fully transparent block still decodes, to transparent black.
    let clear = vec![0u8; 4 * 4 * 4];
    assert_eq!(decode(&encode_bc1(4, 4, &clear)).unwrap().rgba, clear);
}

#[test]
fn block_encoders_stay_close_on_gradients() {
    // A grey ramp: colours of a block lie on the line between its extremes.
    let source = gradient(16, 16, |x, y| {
        let v = (8 * x + 4 * y) as u8;
        [v, v, v, (255 - 9 * x - 7 * y) as u8]
    });
    let bc1 = decode(&encode_bc1(
        16,
        16,
        &gradient(16, 16, |x, y| {
            let v = (8 * x + 4 * y) as u8;
            [v, v, v, 255]
        }),
    ))
    .unwrap();
    for channel in 0..3 {
        assert!(max_channel_error(&bc1.rgba, &source, channel) <= 12);
    }
    assert!(bc1.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));

    let bc2 = decode(&encode_bc2(16, 16, &source)).unwrap();
    let bc3 = decode(&encode_bc3(16, 16, &source)).unwrap();
    for channel in 0..3 {
        assert!(max_channel_error(&bc2.rgba, &source, channel) <= 12);
        assert!(max_channel_error(&bc3.rgba, &source, channel) <= 12);
    }
    // Sixteen levels 17 apart; eight levels across a span of at most 48.
    assert!(max_channel_error(&bc2.rgba, &source, 3) <= 8);
    assert!(max_channel_error(&bc3.rgba, &source, 3) <= 4);

    let bc5 = decode(&encode_bc5(16, 16, &source)).unwrap();
    assert!(max_channel_error(&bc5.rgba, &source, 0) <= 3);
    assert!(max_channel_error(&bc5.rgba, &source, 1) <= 3);
}

#[test]
fn block_encoders_match_the_decoder_bit_layout() {
    // Texel 0 is 0, texel 15 is 255, the rest 255: index bits must land
    // where the decoder reads them.
    let mut values = [255u8; 16];
    values[0] = 0;
    let block = encode_alpha_block(&values);
    assert_eq!(block, [255, 0, 0x01, 0, 0, 0, 0, 0]);

    let mut texels = [[255u8, 255, 255, 255]; 16];
    texels[5] = [0, 0, 0, 255];
    let block = encode_color_block(&texels, false);
    assert_eq!(block, [0xFF, 0xFF, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00]);
    assert_eq!(pack_565([255, 130, 8]), 0xFC01);
}

// --------------------------------------------------------- uncompressed

#[test]
fn uncompressed_layouts_round_trip() {
    let source = gradient(7, 5, |x, y| {
        [
            (x * 36) as u8,
            (y * 60) as u8,
            (x * 20 + y * 17) as u8,
            (255 - x * 30) as u8,
        ]
    });
    let opaque: Vec<u8> = source
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    let decoded = |layout| {
        decode(&encode_uncompressed(7, 5, &source, layout))
            .unwrap()
            .rgba
    };

    assert_eq!(decoded(UncompressedLayout::Bgra8), source);
    assert_eq!(decoded(UncompressedLayout::Rgba8), source);
    assert_eq!(decoded(UncompressedLayout::Bgrx8), opaque);
    assert_eq!(decoded(UncompressedLayout::Bgr8), opaque);

    let grey: Vec<u8> = source
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[0], p[0], 255])
        .collect();
    assert_eq!(decoded(UncompressedLayout::L8), grey);
    let grey_alpha: Vec<u8> = source
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[0], p[0], p[3]])
        .collect();
    assert_eq!(decoded(UncompressedLayout::A8L8), grey_alpha);

    let packed = decoded(UncompressedLayout::Rgb565);
    assert!(max_channel_error(&packed, &opaque, 0) <= 4);
    assert!(max_channel_error(&packed, &opaque, 1) <= 2);
    assert!(max_channel_error(&packed, &opaque, 2) <= 4);
    assert!(packed.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
}

#[test]
fn uncompressed_formats_report_their_masks() {
    let file = encode_uncompressed(1, 1, &[1, 2, 3, 4], UncompressedLayout::Bgra8);
    let header = info(&file).unwrap();
    assert_eq!((header.width, header.height, header.mip_count), (1, 1, 1));
    assert!(!header.cubemap);
    assert_eq!(
        header.format,
        DdsFormat::Uncompressed {
            bit_count: 32,
            r_mask: 0x00FF_0000,
            g_mask: 0x0000_FF00,
            b_mask: 0x0000_00FF,
            a_mask: 0xFF00_0000,
            luminance: false,
        }
    );
    let file = encode_uncompressed(1, 1, &[1, 2, 3, 4], UncompressedLayout::A8L8);
    assert_eq!(
        info(&file).unwrap().format,
        DdsFormat::Uncompressed {
            bit_count: 16,
            r_mask: 0x00FF,
            g_mask: 0,
            b_mask: 0,
            a_mask: 0xFF00,
            luminance: true,
        }
    );
}

#[test]
fn uncompressed_pixels_from_hand_built_bytes() {
    let bgr = [0x00FF_0000, 0x0000_FF00, 0x0000_00FF];
    let with_alpha = |m: [u32; 3], a: u32| [m[0], m[1], m[2], a];

    // 32-bit BGRA, two pixels in one row.
    let file = masks_file(
        DDPF_RGB | DDPF_ALPHAPIXELS,
        32,
        with_alpha(bgr, 0xFF00_0000),
        2,
        1,
        &[1, 2, 3, 4, 5, 6, 7, 8],
    );
    assert_eq!(pixels(&file), [[3, 2, 1, 4], [7, 6, 5, 8]]);

    // The same masks without the alpha flag: the fourth byte is padding.
    let file = masks_file(
        DDPF_RGB,
        32,
        with_alpha(bgr, 0xFF00_0000),
        1,
        1,
        &[1, 2, 3, 4],
    );
    assert_eq!(pixels(&file), [[3, 2, 1, 255]]);

    // 32-bit RGBA.
    let rgba = [0x0000_00FF, 0x0000_FF00, 0x00FF_0000, 0xFF00_0000];
    let file = masks_file(DDPF_RGB | DDPF_ALPHAPIXELS, 32, rgba, 1, 1, &[1, 2, 3, 4]);
    assert_eq!(pixels(&file), [[1, 2, 3, 4]]);

    // 24-bit BGR, two rows of one pixel: the first row is the top row.
    let file = masks_file(DDPF_RGB, 24, with_alpha(bgr, 0), 1, 2, &[1, 2, 3, 9, 8, 7]);
    assert_eq!(pixels(&file), [[3, 2, 1, 255], [7, 8, 9, 255]]);

    // 16-bit 565: 0x8410, 0x0841, 0xFFFF, 0xF800.
    let file = masks_file(
        DDPF_RGB,
        16,
        [0xF800, 0x07E0, 0x001F, 0],
        4,
        1,
        &[0x10, 0x84, 0x41, 0x08, 0xFF, 0xFF, 0x00, 0xF8],
    );
    assert_eq!(
        pixels(&file),
        [
            [132, 130, 132, 255],
            [8, 8, 8, 255],
            [255, 255, 255, 255],
            [255, 0, 0, 255],
        ]
    );

    // 16-bit 1555: the single alpha bit widens to 0 or 255.
    let file = masks_file(
        DDPF_RGB | DDPF_ALPHAPIXELS,
        16,
        [0x7C00, 0x03E0, 0x001F, 0x8000],
        2,
        1,
        &[0x00, 0xFC, 0x1F, 0x00],
    );
    assert_eq!(pixels(&file), [[255, 0, 0, 255], [0, 0, 255, 0]]);

    // 16-bit 4444: 0xF12C is alpha 15, red 1, green 2, blue 12.
    let file = masks_file(
        DDPF_RGB | DDPF_ALPHAPIXELS,
        16,
        [0x0F00, 0x00F0, 0x000F, 0xF000],
        1,
        1,
        &[0x2C, 0xF1],
    );
    assert_eq!(pixels(&file), [[17, 34, 204, 255]]);

    // 8-bit luminance, and 16-bit luminance with alpha in the high byte.
    let file = masks_file(DDPF_LUMINANCE, 8, [0xFF, 0, 0, 0], 2, 1, &[77, 200]);
    assert_eq!(pixels(&file), [[77, 77, 77, 255], [200, 200, 200, 255]]);
    let file = masks_file(
        DDPF_LUMINANCE | DDPF_ALPHAPIXELS,
        16,
        [0x00FF, 0, 0, 0xFF00],
        1,
        1,
        &[77, 9],
    );
    assert_eq!(pixels(&file), [[77, 77, 77, 9]]);

    // 16-bit luminance keeps its top eight bits.
    let file = masks_file(DDPF_LUMINANCE, 16, [0xFFFF, 0, 0, 0], 1, 1, &[0x34, 0x12]);
    assert_eq!(pixels(&file), [[0x12, 0x12, 0x12, 255]]);

    // Alpha only: colour reads as zero.
    let file = masks_file(DDPF_ALPHA, 8, [0, 0, 0, 0xFF], 1, 1, &[9]);
    assert_eq!(pixels(&file), [[0, 0, 0, 9]]);
}

#[test]
fn missing_format_flags_are_inferred_from_the_masks() {
    let file = masks_file(
        0,
        32,
        [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000],
        1,
        1,
        &[1, 2, 3, 4],
    );
    assert_eq!(pixels(&file), [[3, 2, 1, 4]]);
    let file = masks_file(0, 8, [0xFF, 0, 0, 0], 1, 1, &[5]);
    assert_eq!(pixels(&file), [[5, 5, 5, 255]]);
    let file = masks_file(0, 8, [0, 0, 0, 0xFF], 1, 1, &[5]);
    assert_eq!(pixels(&file), [[0, 0, 0, 5]]);
}

// ------------------------------------------------- header level features

#[test]
fn info_reports_format_size_and_mips() {
    let source = gradient(8, 4, |x, y| [(x * 30) as u8, (y * 60) as u8, 0, 255]);
    for (file, format) in [
        (encode_bc1(8, 4, &source), DdsFormat::Bc1),
        (encode_bc2(8, 4, &source), DdsFormat::Bc2),
        (encode_bc3(8, 4, &source), DdsFormat::Bc3),
        (encode_bc5(8, 4, &source), DdsFormat::Bc5),
    ] {
        let header = info(&file).unwrap();
        assert_eq!((header.width, header.height), (8, 4));
        assert_eq!(header.mip_count, 1);
        assert_eq!(header.format, format);
        assert!(!header.cubemap);
    }
}

#[test]
fn only_the_top_mip_level_is_decoded() {
    // 4 x 4 red, then 2 x 2 and 1 x 1 levels in blue.
    let red = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
    let blue = [0x1F, 0x00, 0x1F, 0x00, 0, 0, 0, 0];
    let mut file = DdsFile::new(
        4,
        4,
        PixelFormat::FourCc(*b"DXT1"),
        [red, blue, blue].concat(),
    );
    file.mip_count = 3;
    let bytes = file.to_bytes();
    assert_eq!(info(&bytes).unwrap().mip_count, 3);
    assert!(pixels(&bytes).iter().all(|&p| p == [255, 0, 0, 255]));
}

#[test]
fn cube_map_decodes_its_first_face() {
    let face = |c: u8| [c, c, c, 255];
    let data: Vec<u8> = (0..6u8).flat_map(|i| face(10 * (i + 1))).collect();
    let mut file = DdsFile::new(1, 1, UncompressedLayout::Rgba8.pixel_format(), data);
    file.cubemap = true;
    let bytes = file.to_bytes();
    assert!(info(&bytes).unwrap().cubemap);
    assert_eq!(pixels(&bytes), [[10, 10, 10, 255]]);

    // The DX10 header marks a cube with its own flag.
    let mut file = DdsFile::new(
        1,
        1,
        PixelFormat::Dx10 { dxgi_format: 28 },
        vec![1, 2, 3, 4],
    );
    file.cubemap = true;
    let mut bytes = file.to_bytes();
    put_u32(&mut bytes, 112, 0);
    assert!(info(&bytes).unwrap().cubemap);
}

#[test]
fn dx10_header_maps_dxgi_formats() {
    let dx10 = |format: u32, width: u32, height: u32, data: &[u8]| {
        DdsFile::new(
            width,
            height,
            PixelFormat::Dx10 {
                dxgi_format: format,
            },
            data.to_vec(),
        )
        .to_bytes()
    };

    // RGBA8 and its sRGB twin decode to the same stored bytes.
    for format in [28, 29] {
        assert_eq!(pixels(&dx10(format, 1, 1, &[1, 2, 3, 4])), [[1, 2, 3, 4]]);
    }
    for format in [87, 91] {
        assert_eq!(pixels(&dx10(format, 1, 1, &[1, 2, 3, 4])), [[3, 2, 1, 4]]);
    }
    for format in [88, 93] {
        assert_eq!(pixels(&dx10(format, 1, 1, &[1, 2, 3, 4])), [[3, 2, 1, 255]]);
    }
    assert_eq!(
        pixels(&dx10(85, 1, 1, &[0x10, 0x84])),
        [[132, 130, 132, 255]]
    );
    assert_eq!(pixels(&dx10(86, 1, 1, &[0x00, 0xFC])), [[255, 0, 0, 255]]);
    assert_eq!(
        pixels(&dx10(115, 1, 1, &[0x2C, 0xF1])),
        [[17, 34, 204, 255]]
    );
    assert_eq!(pixels(&dx10(61, 1, 1, &[7])), [[7, 0, 0, 255]]);
    assert_eq!(pixels(&dx10(49, 1, 1, &[7, 8])), [[7, 8, 0, 255]]);
    assert_eq!(pixels(&dx10(65, 1, 1, &[7])), [[0, 0, 0, 7]]);

    let mut bc1 = vec![0x00, 0xF8, 0x1F, 0x00];
    bc1.extend_from_slice(&INDEX_ROWS);
    for format in [71, 72] {
        let file = dx10(format, 4, 4, &bc1);
        assert_eq!(info(&file).unwrap().format, DdsFormat::Bc1);
        assert_eq!(pixels(&file), pixels(&fourcc_file(b"DXT1", 4, 4, &bc1)));
    }
    let bc3 = bc3_block(200, 10);
    for (format, expected, code) in [
        (74, DdsFormat::Bc2, b"DXT3"),
        (75, DdsFormat::Bc2, b"DXT3"),
        (77, DdsFormat::Bc3, b"DXT5"),
        (78, DdsFormat::Bc3, b"DXT5"),
        (83, DdsFormat::Bc5, b"ATI2"),
    ] {
        let file = dx10(format, 4, 4, &bc3);
        assert_eq!(info(&file).unwrap().format, expected);
        assert_eq!(pixels(&file), pixels(&fourcc_file(code, 4, 4, &bc3)));
    }

    // Formats outside the table are named in the error.
    for format in [0, 2, 10, 80, 84, 95, 98, u32::MAX] {
        let err = decode(&dx10(format, 4, 4, &[0; 64])).unwrap_err();
        assert_eq!(
            err,
            DdsError::Unsupported {
                offset: 128,
                what: format!("DXGI format {format}"),
            }
        );
    }

    // A 3D resource is refused.
    let mut volume = dx10(28, 1, 1, &[1, 2, 3, 4]);
    put_u32(&mut volume, 132, 4);
    assert!(matches!(
        decode(&volume).unwrap_err(),
        DdsError::Unsupported { offset: 132, .. }
    ));

    // The extension header itself can be cut short.
    let whole = dx10(28, 1, 1, &[1, 2, 3, 4]);
    assert_eq!(
        info(&whole[..140]).unwrap_err(),
        DdsError::Truncated {
            offset: 128,
            what: "DX10 header",
            needed: 20,
            available: 12,
        }
    );
    assert_eq!(
        decode(&whole[..150]).unwrap_err(),
        DdsError::Truncated {
            offset: 148,
            what: "top mip level",
            needed: 4,
            available: 2,
        }
    );
}

// ------------------------------------------------------- malformed input

#[test]
fn rejects_short_and_foreign_files() {
    assert!(matches!(
        info(&[]).unwrap_err(),
        DdsError::Truncated {
            offset: 0,
            needed: 4,
            available: 0,
            ..
        }
    ));
    assert_eq!(
        info(b"PNG\x0d and more").unwrap_err(),
        DdsError::BadMagic { found: *b"PNG\x0d" }
    );
    assert!(matches!(
        info(b"DDS ").unwrap_err(),
        DdsError::Truncated {
            needed: 128,
            available: 4,
            ..
        }
    ));

    let good = encode_bc1(4, 4, &[255u8; 64]);
    let mut bad = good.clone();
    put_u32(&mut bad, 4, 24);
    assert!(matches!(
        info(&bad).unwrap_err(),
        DdsError::BadHeader {
            offset: 4,
            value: 24,
            ..
        }
    ));
    let mut bad = good.clone();
    put_u32(&mut bad, 76, 0);
    assert!(matches!(
        info(&bad).unwrap_err(),
        DdsError::BadHeader { offset: 76, .. }
    ));
}

#[test]
fn rejects_zero_and_oversized_dimensions() {
    let good = encode_bc1(4, 4, &[255u8; 64]);
    for (width, height) in [
        (0, 4),
        (4, 0),
        (0, 0),
        (MAX_DIMENSION + 1, 4),
        (4, MAX_DIMENSION + 1),
        (u32::MAX, u32::MAX),
    ] {
        let mut bad = good.clone();
        put_u32(&mut bad, 12, height);
        put_u32(&mut bad, 16, width);
        assert_eq!(
            decode(&bad).unwrap_err(),
            DdsError::BadDimensions {
                width,
                height,
                limit: MAX_DIMENSION,
            }
        );
    }

    // The largest allowed size with a small payload fails on the length
    // check, before any output is allocated.
    let mut bad = good.clone();
    put_u32(&mut bad, 12, MAX_DIMENSION);
    put_u32(&mut bad, 16, MAX_DIMENSION);
    assert_eq!(
        decode(&bad).unwrap_err(),
        DdsError::Truncated {
            offset: 128,
            what: "top mip level",
            needed: 4096 * 4096 * 8,
            available: 8,
        }
    );
    assert_eq!(info(&bad).unwrap().width, MAX_DIMENSION);

    // A long, thin image at the limit decodes.
    let row = vec![7u8; MAX_DIMENSION as usize];
    let file = masks_file(DDPF_LUMINANCE, 8, [0xFF, 0, 0, 0], MAX_DIMENSION, 1, &row);
    let image = decode(&file).unwrap();
    assert_eq!(image.rgba.len(), MAX_DIMENSION as usize * 4);
    assert!(
        image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [7, 7, 7, 255])
    );
}

#[test]
fn rejects_unsupported_pixel_formats() {
    let unsupported = |file: &[u8]| match decode(file).unwrap_err() {
        DdsError::Unsupported { offset, what } => (offset, what),
        other => panic!("unexpected error: {other}"),
    };

    let (offset, what) = unsupported(&fourcc_file(b"ATI1", 4, 4, &[0; 8]));
    assert_eq!(offset, 84);
    assert!(what.contains("ATI1"));
    // Premultiplied variants are refused, since the output is straight alpha.
    for code in [b"DXT2", b"DXT4"] {
        let (offset, what) = unsupported(&fourcc_file(code, 4, 4, &[0; 16]));
        assert_eq!(offset, 84);
        assert!(what.contains("premultiplied"));
    }
    // A numeric FourCC (a legacy float format).
    let (_, what) = unsupported(&fourcc_file(&113u32.to_le_bytes(), 4, 4, &[0; 128]));
    assert!(what.contains("0x00000071"));

    // Pixel sizes that are not whole bytes of 1 to 4.
    for bits in [0, 4, 12, 48, 64] {
        let (offset, _) = unsupported(&masks_file(
            DDPF_RGB,
            bits,
            [0xF, 0xF0, 0, 0],
            1,
            1,
            &[0; 8],
        ));
        assert_eq!(offset, 88);
    }
    // No masks at all.
    let (offset, _) = unsupported(&masks_file(DDPF_RGB, 32, [0; 4], 1, 1, &[0; 4]));
    assert_eq!(offset, 80);
    // Signed bump map.
    let (offset, _) = unsupported(&masks_file(
        0x8_0000,
        16,
        [0xFF, 0xFF00, 0, 0],
        1,
        1,
        &[0; 2],
    ));
    assert_eq!(offset, 80);

    // A mask outside the pixel, and a mask with a hole in it.
    assert_eq!(
        decode(&masks_file(
            DDPF_RGB,
            16,
            [0x1_F800, 0x07E0, 0x1F, 0],
            1,
            1,
            &[0; 2]
        ))
        .unwrap_err(),
        DdsError::BadHeader {
            offset: 92,
            what: "channel mask",
            value: 0x1_F800,
        }
    );
    assert_eq!(
        decode(&masks_file(
            DDPF_RGB,
            32,
            [0xFF, 0xF0F0_0000, 0, 0],
            1,
            1,
            &[0; 4]
        ))
        .unwrap_err(),
        DdsError::BadHeader {
            offset: 96,
            what: "channel mask",
            value: 0xF0F0_0000,
        }
    );

    // A volume texture.
    let mut volume = encode_uncompressed(1, 1, &[1, 2, 3, 4], UncompressedLayout::Bgra8);
    put_u32(&mut volume, 24, 4);
    put_u32(&mut volume, 112, 0x20_0000);
    let (offset, what) = unsupported(&volume);
    assert_eq!((offset, what.as_str()), (112, "volume texture"));
}

fn sample_files() -> Vec<Vec<u8>> {
    let source = gradient(6, 5, |x, y| {
        [
            (x * 40) as u8,
            (y * 50) as u8,
            (x * y * 8) as u8,
            (x * 50) as u8,
        ]
    });
    let mut files = vec![
        encode_bc1(6, 5, &source),
        encode_bc2(6, 5, &source),
        encode_bc3(6, 5, &source),
        encode_bc5(6, 5, &source),
    ];
    for layout in [
        UncompressedLayout::Bgra8,
        UncompressedLayout::Rgba8,
        UncompressedLayout::Bgrx8,
        UncompressedLayout::Bgr8,
        UncompressedLayout::Rgb565,
        UncompressedLayout::L8,
        UncompressedLayout::A8L8,
    ] {
        files.push(encode_uncompressed(6, 5, &source, layout));
    }
    let bc3 = encode_bc3(6, 5, &source);
    files.push(
        DdsFile::new(
            6,
            5,
            PixelFormat::Dx10 { dxgi_format: 77 },
            bc3[128..].to_vec(),
        )
        .to_bytes(),
    );
    files
}

#[test]
fn every_truncation_is_an_error() {
    for file in sample_files() {
        let whole = decode(&file).unwrap();
        assert_eq!(whole.rgba.len(), 6 * 5 * 4);
        for len in 0..file.len() {
            assert!(
                matches!(
                    decode(&file[..len]),
                    Err(DdsError::Truncated { .. } | DdsError::BadMagic { .. })
                ),
                "decoded {len} of {} bytes",
                file.len()
            );
        }
        // Trailing bytes (lower mip levels, other faces) are ignored.
        let mut longer = file.clone();
        longer.extend_from_slice(&[0xAB; 37]);
        assert_eq!(decode(&longer).unwrap(), whole);
    }
}

#[test]
fn random_corruption_never_panics() {
    let mut state = 0x2545_F491u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for file in sample_files() {
        for round in 0..2000 {
            let mut bytes = file.clone();
            // Most rounds damage the headers; the rest damage anything.
            let span = if round % 4 == 0 { bytes.len() } else { 128 };
            for _ in 0..1 + next() % 4 {
                let at = next() as usize % span;
                bytes[at] = next() as u8;
            }
            let _ = info(&bytes);
            if let Ok(image) = decode(&bytes) {
                assert_eq!(
                    image.rgba.len(),
                    image.width as usize * image.height as usize * 4
                );
            }
        }
    }
}
