//! Deterministic PNG encoding and bounded decoding.

use std::io::Cursor;

use crate::GalleryError;
use crate::image::{Image, check_allocation, check_rgba};

/// Encodes an RGBA8 image as an 8-bit RGBA PNG.
///
/// The settings are fixed (balanced deflate, adaptive row filters) and no
/// ancillary chunks are written, so one input always gives the same bytes.
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, GalleryError> {
    if width == 0 || height == 0 {
        return Err(GalleryError::EmptyImage { width, height });
    }
    check_rgba("PNG input", rgba, width, height)?;
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        encoder.set_filter(png::Filter::Adaptive);
        let mut writer = encoder
            .write_header()
            .map_err(|error| GalleryError::PngEncode(error.to_string()))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| GalleryError::PngEncode(error.to_string()))?;
        writer
            .finish()
            .map_err(|error| GalleryError::PngEncode(error.to_string()))?;
    }
    Ok(out)
}

/// Decodes a PNG of any colour type and bit depth into RGBA8.
///
/// Images over [`crate::MAX_DIMENSION`] on a side or [`crate::MAX_PIXELS`]
/// in all are refused before their pixels are allocated. Animated PNGs give
/// their first frame.
pub fn decode_png(bytes: &[u8]) -> Result<Image, GalleryError> {
    let decode = |error: png::DecodingError| GalleryError::PngDecode(error.to_string());
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(decode)?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    check_allocation(u64::from(width), u64::from(height))?;
    let size = reader.output_buffer_size().ok_or(GalleryError::TooLarge {
        width: u64::from(width),
        height: u64::from(height),
    })?;
    let mut buffer = vec![0; size];
    let frame = reader.next_frame(&mut buffer).map_err(decode)?;
    if frame.bit_depth != png::BitDepth::Eight {
        return Err(GalleryError::PngDecode(format!(
            "unexpected output bit depth {:?}",
            frame.bit_depth
        )));
    }
    let pixels = width as usize * height as usize;
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => {
            return Err(GalleryError::PngDecode(format!(
                "unexpected output colour type {other:?}"
            )));
        }
    };
    let line = frame.line_size;
    let row_bytes = width as usize * channels;
    if line < row_bytes || buffer.len() < line * height as usize {
        return Err(GalleryError::PngDecode("decoded rows are short".to_owned()));
    }
    let mut rgba = Vec::with_capacity(pixels * 4);
    for row in buffer.chunks_exact(line).take(height as usize) {
        for px in row[..row_bytes].chunks_exact(channels) {
            let out = match *px {
                [g] => [g, g, g, 255],
                [g, a] => [g, g, g, a],
                [r, g, b] => [r, g, b, 255],
                [r, g, b, a] => [r, g, b, a],
                _ => [0, 0, 0, 0],
            };
            rgba.extend_from_slice(&out);
        }
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}
