mod common;

use common::*;
use foundry_gallery::{
    Anchor, Background, Columns, GalleryError, Rect, SHOWN_KEY, SheetOptions, Tile, Zoom,
    contact_sheet, contact_sheets,
};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn three_tiles() -> [Vec<u8>; 3] {
    [solid(4, 6, RED), solid(8, 3, GREEN), solid(5, 5, BLUE)]
}

// Hand-computed for `small_options` and tiles 4x6, 8x3, 5x5 with no metrics:
// content width max(10, 8) = 10, image area 6 high, one text line of 9.
// cell = 2 border + 4 padding + 10 = 16 wide, 6 + 6 + 2 + 9 = 23 high.
// sheet = 3 + 2 * (16 + 3) = 41 wide, 3 + 2 * (23 + 3) = 55 high.
#[test]
fn grid_positions_match_hand_computed_coordinates() {
    let [a, b, c] = three_tiles();
    let tiles = [tile(&a, 4, 6), tile(&b, 8, 3), tile(&c, 5, 5)];
    let sheet = contact_sheet(&tiles, &small_options()).unwrap();
    assert_eq!((sheet.width, sheet.height), (41, 55));
    assert_eq!(sheet.rgba.len(), 41 * 55 * 4);
    let cells: Vec<Rect> = sheet.cells.iter().map(|cell| cell.cell).collect();
    let rect = |x, y, width, height| Rect {
        x,
        y,
        width,
        height,
    };
    assert_eq!(
        cells,
        [rect(3, 3, 16, 23), rect(22, 3, 16, 23), rect(3, 29, 16, 23)]
    );
    // Bottom anchoring: sprites end on the image area's last row (y = 11).
    assert_eq!(sheet.cells[0].sprite, rect(9, 6, 4, 6));
    assert_eq!(sheet.cells[1].sprite, rect(26, 9, 8, 3));
    assert_eq!(sheet.cells[2].sprite, rect(8, 33, 5, 5));
    for cell in &sheet.cells {
        assert_eq!(cell.zoom, Zoom::Up(1));
        assert_eq!(cell.compare, None);
    }

    assert_eq!(sheet.pixel(9, 6), Some(RED));
    assert_eq!(sheet.pixel(12, 11), Some(RED));
    assert_eq!(sheet.pixel(8, 6), Some(CELL));
    assert_eq!(sheet.pixel(26, 9), Some(GREEN));
    assert_eq!(sheet.pixel(33, 11), Some(GREEN));
    assert_eq!(sheet.pixel(26, 8), Some(CELL));
    assert_eq!(sheet.pixel(8, 33), Some(BLUE));
    // Borders, gutters, and the empty fourth slot.
    assert_eq!(sheet.pixel(3, 3), Some(BORDER));
    assert_eq!(sheet.pixel(18, 25), Some(BORDER));
    assert_eq!(sheet.pixel(4, 4), Some(CELL));
    assert_eq!(sheet.pixel(20, 10), Some(SHEET));
    assert_eq!(sheet.pixel(0, 0), Some(SHEET));
    assert_eq!(sheet.pixel(30, 40), Some(SHEET));
    // Every pixel is opaque.
    assert!(sheet.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255));
}

#[test]
fn centre_anchoring_centres_vertically() {
    let [a, b, c] = three_tiles();
    let tiles = [tile(&a, 4, 6), tile(&b, 8, 3), tile(&c, 5, 5)];
    let options = SheetOptions {
        anchor: Anchor::Centre,
        ..small_options()
    };
    let sheet = contact_sheet(&tiles, &options).unwrap();
    assert_eq!(sheet.cells[0].sprite.y, 6);
    assert_eq!(sheet.cells[1].sprite.y, 6 + (6 - 3) / 2);
    assert_eq!(sheet.cells[2].sprite.y, 32);
}

#[test]
fn automatic_columns_follow_the_target_width() {
    let images: Vec<Vec<u8>> = (0..5).map(|_| solid(4, 6, RED)).collect();
    let tiles: Vec<Tile> = images.iter().map(|image| tile(image, 4, 6)).collect();
    let columns = |sheet_width, count: usize| {
        let options = SheetOptions {
            columns: Columns::Auto { sheet_width },
            ..small_options()
        };
        let sheet = contact_sheet(&tiles[..count], &options).unwrap();
        // Cells are 16 wide with a gutter of 3: width = 3 + n * 19.
        let n = (sheet.width - 3) / 19;
        assert!(sheet.width <= sheet_width.max(22));
        n
    };
    assert_eq!(columns(41, 5), 2);
    assert_eq!(columns(40, 5), 1);
    assert_eq!(columns(60, 5), 3);
    assert_eq!(columns(1000, 5), 5);
    assert_eq!(columns(1000, 2), 2);
    // Narrower than one cell still gives one column.
    assert_eq!(columns(5, 5), 1);
}

#[test]
fn integer_upscaling_is_exact() {
    let image = gradient(3, 2);
    let tiles = [tile(&image, 3, 2)];
    let options = SheetOptions {
        scale: 3,
        ..small_options()
    };
    let sheet = contact_sheet(&tiles, &options).unwrap();
    let rect = sheet.cells[0].sprite;
    assert_eq!((rect.width, rect.height), (9, 6));
    assert_eq!(sheet.cells[0].zoom, Zoom::Up(3));
    for dy in 0..6 {
        for dx in 0..9 {
            let expected = px(&image, 3, dx / 3, dy / 3);
            assert_eq!(
                sheet.pixel(rect.x + dx, rect.y + dy),
                Some(expected),
                "({dx},{dy})"
            );
        }
    }
}

#[test]
fn oversized_tiles_are_reduced_and_noted() {
    let image = gradient(40, 10);
    let metrics = [("a".to_owned(), "1".to_owned())];
    let tiles = [Tile {
        metrics: &metrics,
        ..tile(&image, 40, 10)
    }];
    let options = SheetOptions {
        max_cell: [16, 16],
        ..small_options()
    };
    let sheet = contact_sheet(&tiles, &options).unwrap();
    let cell = sheet.cells[0];
    // 40 / 2 = 20 > 16; ceil(40 / 3) = 14 fits.
    assert_eq!(cell.zoom, Zoom::Down(3));
    assert_eq!((cell.sprite.width, cell.sprite.height), (14, 4));
    for dy in 0..4 {
        for dx in 0..14 {
            let expected = px(&image, 40, dx * 3, dy * 3);
            assert_eq!(
                sheet.pixel(cell.sprite.x + dx, cell.sprite.y + dy),
                Some(expected)
            );
        }
    }
    // The note adds a metric line: label + 1 metric + note = 3 lines.
    assert_eq!(cell.cell.height, 2 + 4 + 4 + 2 + 3 * 9);
    assert_eq!(SHOWN_KEY, "shown");

    // An upscale that does not fit falls back to the largest one that does.
    let small = gradient(5, 5);
    let tiles = [tile(&small, 5, 5)];
    let options = SheetOptions {
        scale: 4,
        max_cell: [12, 12],
        ..small_options()
    };
    let sheet = contact_sheet(&tiles, &options).unwrap();
    assert_eq!(sheet.cells[0].zoom, Zoom::Up(2));
    assert_eq!(sheet.cells[0].sprite.width, 10);
}

#[test]
fn background_shows_behind_transparent_pixels() {
    let mut image = solid(4, 4, RED);
    // Clear pixel (1, 0) and make (2, 0) half transparent.
    image[4..8].copy_from_slice(&[0, 0, 0, 0]);
    image[8..12].copy_from_slice(&[255, 0, 0, 128]);
    let tiles = [tile(&image, 4, 4)];
    let sheet = contact_sheet(&tiles, &small_options()).unwrap();
    let r = sheet.cells[0].sprite;
    assert_eq!(sheet.pixel(r.x, r.y), Some(RED));
    assert_eq!(sheet.pixel(r.x + 1, r.y), Some(BACK));
    // 255 * 128/255 over (0, 0, 90): (128, 0, 45).
    assert_eq!(sheet.pixel(r.x + 2, r.y), Some([128, 0, 45, 255]));

    let light = [200, 200, 200, 255];
    let dark = [100, 100, 100, 255];
    let clear = solid(8, 4, [0, 0, 0, 0]);
    let tiles = [tile(&clear, 8, 4)];
    let options = SheetOptions {
        background: Background::Checker {
            size: 2,
            colours: [light, dark],
        },
        ..small_options()
    };
    let sheet = contact_sheet(&tiles, &options).unwrap();
    let r = sheet.cells[0].sprite;
    assert_eq!(sheet.pixel(r.x, r.y), Some(light));
    assert_eq!(sheet.pixel(r.x + 1, r.y + 1), Some(light));
    assert_eq!(sheet.pixel(r.x + 2, r.y), Some(dark));
    assert_eq!(sheet.pixel(r.x, r.y + 2), Some(dark));
    assert_eq!(sheet.pixel(r.x + 2, r.y + 2), Some(light));
    // The background covers the sprite rectangle only.
    assert_eq!(sheet.pixel(r.x - 1, r.y), Some(CELL));
}

#[test]
fn comparison_cells_share_a_baseline() {
    let new = solid(4, 6, RED);
    let reference = solid(6, 3, BLUE);
    let tiles = [Tile {
        label: "pair",
        compare: Some((&reference, 6, 3)),
        ..tile(&new, 4, 6)
    }];
    let options = SheetOptions {
        scale: 2,
        min_cell_width: 0,
        ..small_options()
    };
    let sheet = contact_sheet(&tiles, &options).unwrap();
    let cell = sheet.cells[0];
    let s = cell.sprite;
    let c = cell.compare.expect("comparison rectangle");
    assert_eq!((s.width, s.height), (8, 12));
    assert_eq!((c.width, c.height), (12, 6));
    assert_eq!(s.y + s.height, c.y + c.height, "common baseline");
    assert!(c.x >= s.x + s.width + 4, "side by side with a gap");
    assert_eq!(sheet.pixel(s.x, s.y), Some(RED));
    assert_eq!(sheet.pixel(c.x + 11, c.y + 5), Some(BLUE));
    // Captions sit in the line under the baseline, one per image.
    let baseline = s.y + s.height;
    let caption_ink = |x0: u32, x1: u32| {
        (baseline..baseline + 9)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| sheet.pixel(x, y) == Some(METRIC))
            .count()
    };
    let middle = (s.x + s.width + c.x) / 2;
    assert!(caption_ink(cell.cell.x, middle) > 0, "new caption");
    assert!(
        caption_ink(middle, cell.cell.x + cell.cell.width) > 0,
        "ref caption"
    );
    // Content is two 17-px caption columns at least: 17 + 4 + 17 = 38.
    assert!(cell.cell.width >= 2 + 4 + 38);
}

#[test]
fn labels_and_metrics_stay_inside_the_cell() {
    let image = solid(4, 4, RED);
    let metrics = [("a key".to_owned(), "a very long metric value".to_owned())];
    let tiles = [
        Tile {
            label: "a label far wider than the cell",
            metrics: &metrics,
            ..tile(&image, 4, 4)
        },
        tile(&image, 4, 4),
    ];
    let sheet = contact_sheet(&tiles, &small_options()).unwrap();
    let cell = sheet.cells[0].cell;
    let mut label_ink = 0;
    let mut metric_ink = 0;
    for y in 0..sheet.height {
        for x in 0..sheet.width {
            let colour = sheet.pixel(x, y).unwrap();
            if colour == LABEL || colour == METRIC {
                // Inside the content area: border 1 + padding 2.
                assert!(x >= cell.x + 3 && x < cell.x + cell.width - 3, "x {x}");
                assert!(y >= cell.y + 3 && y < cell.y + cell.height - 3, "y {y}");
            }
            label_ink += usize::from(colour == LABEL);
            metric_ink += usize::from(colour == METRIC);
        }
    }
    assert!(label_ink > 0 && metric_ink > 0);
    // Two text lines: cell height 6 + 4 + 2 + 2 * 9.
    assert_eq!(cell.height, 6 + 4 + 2 + 18);
}

#[test]
fn title_line_sits_above_the_grid() {
    let image = solid(4, 4, RED);
    let tiles = [tile(&image, 4, 4)];
    let options = SheetOptions {
        title: Some("Title".to_owned()),
        ..small_options()
    };
    let sheet = contact_sheet(&tiles, &options).unwrap();
    // Title: 9 px line + 3 px gutter above the cells.
    assert_eq!(sheet.cells[0].cell.y, 3 + 9 + 3);
    let ink = (3..12)
        .flat_map(|y| (0..sheet.width).map(move |x| (x, y)))
        .filter(|&(x, y)| sheet.pixel(x, y) == Some(LABEL))
        .count();
    assert!(ink > 0);
}

#[test]
fn multi_page_sheets_split_by_height() {
    let images: Vec<Vec<u8>> = (0..7).map(|i| solid(4, 6, [i * 30, 0, 0, 255])).collect();
    let tiles: Vec<Tile> = images.iter().map(|image| tile(image, 4, 6)).collect();
    // Cells are 23 high. A titled page of two rows: 3 + 12 + 2 * 26 = 67.
    let options = SheetOptions {
        max_height: 67,
        ..small_options()
    };
    let pages = contact_sheets(&tiles, &options).unwrap();
    assert_eq!(pages.len(), 2);
    let indices: Vec<Vec<usize>> = pages
        .iter()
        .map(|page| page.cells.iter().map(|cell| cell.tile).collect())
        .collect();
    assert_eq!(indices, [vec![0, 1, 2, 3], vec![4, 5, 6]]);
    for page in &pages {
        assert!(page.height <= 67);
        assert_eq!(page.width, pages[0].width);
        assert_eq!(page.cells[0].cell.y, 3 + 9 + 3, "range title line");
    }
    assert_eq!(pages[0].height, 67);
    assert_eq!(pages[1].pixel(9, 18), Some([120, 0, 0, 255]));

    // Everything on one page: identical to contact_sheet.
    let options = SheetOptions {
        max_height: 10_000,
        ..small_options()
    };
    let pages = contact_sheets(&tiles, &options).unwrap();
    assert_eq!(pages, [contact_sheet(&tiles, &options).unwrap()]);
    // A page too short for one row still holds one row.
    let options = SheetOptions {
        max_height: 5,
        ..small_options()
    };
    assert_eq!(contact_sheets(&tiles, &options).unwrap().len(), 4);
    assert!(contact_sheets(&[], &options).unwrap().is_empty());
}

#[test]
fn rendering_is_deterministic() {
    let a = gradient(13, 7);
    let b = solid(5, 9, [9, 8, 7, 100]);
    let metrics = [("k".to_owned(), "v".to_owned())];
    let tiles = [
        Tile {
            label: "first",
            metrics: &metrics,
            compare: Some((&b, 5, 9)),
            ..tile(&a, 13, 7)
        },
        tile(&b, 5, 9),
    ];
    let options = SheetOptions {
        title: Some("det".to_owned()),
        scale: 2,
        ..SheetOptions::default()
    };
    let first = contact_sheet(&tiles, &options).unwrap();
    let second = contact_sheet(&tiles, &options).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.to_png().unwrap(), second.to_png().unwrap());
}

#[test]
fn bad_buffers_and_options_are_rejected() {
    let image = solid(4, 4, RED);
    let short = &image[..60];
    let err = contact_sheet(&[tile(short, 4, 4)], &small_options()).unwrap_err();
    assert!(matches!(
        err,
        GalleryError::BufferLength {
            actual: 60,
            expected: 64,
            ..
        }
    ));
    let bad_compare = Tile {
        compare: Some((short, 4, 4)),
        ..tile(&image, 4, 4)
    };
    assert!(matches!(
        contact_sheet(&[bad_compare], &small_options()),
        Err(GalleryError::BufferLength { .. })
    ));
    assert!(matches!(
        contact_sheet(&[], &small_options()),
        Err(GalleryError::NoTiles)
    ));

    let good = [tile(&image, 4, 4)];
    let invalid = |options: SheetOptions| {
        matches!(
            contact_sheet(&good, &options),
            Err(GalleryError::InvalidOption(_))
        )
    };
    assert!(invalid(SheetOptions {
        scale: 0,
        ..small_options()
    }));
    assert!(invalid(SheetOptions {
        scale: 17,
        ..small_options()
    }));
    assert!(invalid(SheetOptions {
        text_scale: 0,
        ..small_options()
    }));
    assert!(invalid(SheetOptions {
        text_scale: 9,
        ..small_options()
    }));
    assert!(invalid(SheetOptions {
        max_cell: [0, 5],
        ..small_options()
    }));
    assert!(invalid(SheetOptions {
        columns: Columns::Fixed(0),
        ..small_options()
    }));
    assert!(invalid(SheetOptions {
        gutter: u32::MAX,
        ..small_options()
    }));
    assert!(invalid(SheetOptions {
        background: Background::Checker {
            size: 0,
            colours: [RED; 2]
        },
        ..small_options()
    }));
    assert!(matches!(
        contact_sheets(
            &good,
            &SheetOptions {
                max_height: 0,
                ..small_options()
            }
        ),
        Err(GalleryError::InvalidOption(_))
    ));
}

#[test]
fn absurd_sheet_sizes_are_refused() {
    // One very wide tile with the reduction limit lifted.
    let wide = solid(20_000, 1, RED);
    let options = SheetOptions {
        max_cell: [u32::MAX, u32::MAX],
        ..small_options()
    };
    assert!(matches!(
        contact_sheet(&[tile(&wide, 20_000, 1)], &options),
        Err(GalleryError::TooLarge { .. })
    ));
    // With the default limit it is reduced instead.
    let sheet = contact_sheet(&[tile(&wide, 20_000, 1)], &small_options()).unwrap();
    assert!(matches!(sheet.cells[0].zoom, Zoom::Down(_)));
    assert!(sheet.cells[0].sprite.width <= 512);

    // An empty buffer claiming a huge width.
    assert!(matches!(
        contact_sheet(&[tile(&[], 4_000_000_000, 0)], &options),
        Err(GalleryError::TooLarge { .. })
    ));

    // Too many rows for one sheet, fine as pages.
    let dot = solid(1, 1, RED);
    let tiles: Vec<Tile> = (0..2000).map(|_| tile(&dot, 1, 1)).collect();
    let column = SheetOptions {
        columns: Columns::Fixed(1),
        ..small_options()
    };
    assert!(matches!(
        contact_sheet(&tiles, &column),
        Err(GalleryError::TooLarge { .. })
    ));
    let pages = contact_sheets(&tiles, &column).unwrap();
    assert!(pages.len() > 1);
    assert_eq!(
        pages.iter().map(|page| page.cells.len()).sum::<usize>(),
        2000
    );
}

#[test]
fn zero_sized_tiles_draw_nothing() {
    let image = solid(4, 4, RED);
    let tiles = [tile(&[], 0, 0), tile(&image, 4, 4)];
    let sheet = contact_sheet(&tiles, &small_options()).unwrap();
    assert_eq!(sheet.cells[0].sprite.width, 0);
    assert_eq!(sheet.cells[1].sprite.width, 4);
}
