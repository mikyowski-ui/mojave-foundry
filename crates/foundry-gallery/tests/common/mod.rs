//! Procedural images and helpers shared by the integration tests.
#![allow(dead_code)]

use foundry_gallery::{Background, Columns, SheetOptions, Tile};

pub const SHEET: [u8; 4] = [10, 10, 10, 255];
pub const CELL: [u8; 4] = [20, 20, 20, 255];
pub const BORDER: [u8; 4] = [200, 0, 200, 255];
pub const BACK: [u8; 4] = [0, 0, 90, 255];
pub const LABEL: [u8; 4] = [255, 255, 0, 255];
pub const METRIC: [u8; 4] = [0, 255, 255, 255];

/// A solid image.
pub fn solid(width: u32, height: u32, colour: [u8; 4]) -> Vec<u8> {
    colour.repeat(width as usize * height as usize)
}

/// Every pixel a distinct opaque colour derived from its position.
pub fn gradient(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..height {
        for x in 0..width {
            out.extend_from_slice(&[
                (x * 17 + 3) as u8,
                (y * 29 + 5) as u8,
                ((x + y) * 7) as u8,
                255,
            ]);
        }
    }
    out
}

pub fn px(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let at = (y * width + x) as usize * 4;
    [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
}

/// Options with distinctive colours and a small, hand-computable layout:
/// border 1, padding 2, gutter 3, min content width 10, text scale 1.
pub fn small_options() -> SheetOptions {
    SheetOptions {
        columns: Columns::Fixed(2),
        scale: 1,
        max_cell: [512, 512],
        min_cell_width: 10,
        background: Background::Solid(BACK),
        padding: 2,
        gutter: 3,
        border: Some(BORDER),
        sheet_colour: SHEET,
        cell_colour: CELL,
        label_colour: LABEL,
        metric_colour: METRIC,
        text_scale: 1,
        title: None,
        ..SheetOptions::default()
    }
}

pub fn tile<'a>(rgba: &'a [u8], width: u32, height: u32) -> Tile<'a> {
    Tile {
        label: "",
        metrics: &[],
        rgba,
        width,
        height,
        compare: None,
    }
}
