//! Contact-sheet layout and rendering.
//!
//! Every cell of a sheet (and of every page of a multi-page set) has the
//! same size: wide enough for the widest tile after scaling, tall enough
//! for the tallest tile plus the label and the largest number of metric
//! lines. Cell geometry, top to bottom:
//!
//! ```text
//! border (1 px when `border` is set)
//! padding
//! image area   (image_height: sprite, or both comparison images + captions)
//! padding
//! label line   (line_height(text_scale))
//! metric lines (one line_height each)
//! padding
//! border
//! ```
//!
//! Cells sit `gutter` pixels apart and `gutter` pixels from the sheet
//! edges; an optional title line (plus one gutter) sits above the grid.

use crate::GalleryError;
use crate::canvas::Canvas;
use crate::font::{draw_text, line_height, text_width, truncate_to_width};
use crate::image::{Rect, check_allocation, check_rgba};
use crate::png_io::encode_png;

/// Largest integer upscale factor.
pub const MAX_SCALE: u32 = 16;
/// Largest text scale factor.
pub const MAX_TEXT_SCALE: u32 = 8;
/// Caption of the new image in a comparison cell.
pub const NEW_CAPTION: &str = "new";
/// Caption of the reference image in a comparison cell.
pub const REF_CAPTION: &str = "ref";
/// Metric key added to a tile shown at less than `SheetOptions::scale`.
pub const SHOWN_KEY: &str = "shown";

/// One sprite, its caption and its metric lines.
#[derive(Debug, Clone, Copy)]
pub struct Tile<'a> {
    pub label: &'a str,
    /// Drawn as `key: value` lines under the label.
    pub metrics: &'a [(String, String)],
    /// RGBA8, `width * height * 4` bytes.
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// A reference image (RGBA8, width, height) drawn to the right.
    pub compare: Option<(&'a [u8], u32, u32)>,
}

/// How many columns the grid has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Columns {
    /// Exactly this many (fewer when there are fewer tiles). Must be > 0.
    Fixed(u32),
    /// As many as fit in this sheet width (at least one).
    Auto { sheet_width: u32 },
}

/// What is drawn behind a sprite's transparent pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    /// Squares of `size` pixels, `colours[0]` at the sprite's top-left.
    Checker {
        size: u32,
        colours: [[u8; 4]; 2],
    },
    Solid([u8; 4]),
}

/// Where a sprite sits in the image area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Centred on both axes.
    Centre,
    /// Centred horizontally, bottom row on the image area's bottom row:
    /// right for sprites that stand on a ground line.
    Bottom,
}

/// The factor a tile is drawn at (nearest-neighbour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    /// Each source pixel becomes a k x k block.
    Up(u32),
    /// Every d-th source pixel in each axis is kept.
    Down(u32),
}

impl Zoom {
    /// Drawn length of `n` source pixels.
    pub fn apply(self, n: u32) -> u32 {
        match self {
            Zoom::Up(k) => n.saturating_mul(k),
            Zoom::Down(d) => n.div_ceil(d.max(1)),
        }
    }

    /// Source coordinate of drawn coordinate `i`.
    fn source(self, i: u32) -> u32 {
        match self {
            Zoom::Up(k) => i / k.max(1),
            Zoom::Down(d) => i.saturating_mul(d),
        }
    }

    fn describe(self) -> String {
        match self {
            Zoom::Up(k) => format!("x{k} (max cell)"),
            Zoom::Down(d) => format!("1/{d} (max cell)"),
        }
    }
}

/// Layout and colours of a contact sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetOptions {
    pub columns: Columns,
    /// Integer nearest-neighbour upscale, 1 to [`MAX_SCALE`].
    pub scale: u32,
    /// Largest drawn sprite area (width, height) per cell, after `scale`.
    /// A larger tile is drawn at a smaller factor ([`Zoom`]) and gets a
    /// [`SHOWN_KEY`] metric line saying so. Both must be > 0.
    pub max_cell: [u32; 2],
    /// Smallest width of a cell's content area, so labels stay readable.
    pub min_cell_width: u32,
    pub background: Background,
    pub anchor: Anchor,
    /// Space inside each cell, between border and content, and between
    /// the image area and the label.
    pub padding: u32,
    /// Space between cells and around the grid.
    pub gutter: u32,
    /// A one-pixel cell border in this colour, or none.
    pub border: Option<[u8; 4]>,
    pub sheet_colour: [u8; 4],
    pub cell_colour: [u8; 4],
    pub label_colour: [u8; 4],
    pub metric_colour: [u8; 4],
    /// Font scale, 1 to [`MAX_TEXT_SCALE`].
    pub text_scale: u32,
    /// A title line above the grid.
    pub title: Option<String>,
    /// Page height limit for [`contact_sheets`]; ignored by
    /// [`contact_sheet`]. Must be > 0.
    pub max_height: u32,
}

impl Default for SheetOptions {
    fn default() -> Self {
        Self {
            columns: Columns::Auto { sheet_width: 1600 },
            scale: 1,
            max_cell: [512, 512],
            min_cell_width: 96,
            background: Background::Checker {
                size: 8,
                colours: [[104, 104, 112, 255], [136, 136, 144, 255]],
            },
            anchor: Anchor::Bottom,
            padding: 4,
            gutter: 6,
            border: Some([96, 96, 104, 255]),
            sheet_colour: [24, 24, 28, 255],
            cell_colour: [40, 40, 46, 255],
            label_colour: [236, 236, 236, 255],
            metric_colour: [168, 176, 168, 255],
            text_scale: 1,
            title: None,
            max_height: 4096,
        }
    }
}

/// Where one tile was drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellLayout {
    /// Index of the tile in the slice passed in.
    pub tile: usize,
    /// The cell, border included.
    pub cell: Rect,
    /// The drawn sprite (the new image in a comparison cell).
    pub sprite: Rect,
    /// The drawn reference image of a comparison cell.
    pub compare: Option<Rect>,
    pub zoom: Zoom,
}

/// A rendered contact sheet: an opaque RGBA8 image and its cell layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub cells: Vec<CellLayout>,
}

impl Sheet {
    /// [`encode_png`] of this sheet.
    pub fn to_png(&self) -> Result<Vec<u8>, GalleryError> {
        encode_png(&self.rgba, self.width, self.height)
    }

    /// The sheet as a canvas, for drawing extra marks.
    pub fn canvas(&mut self) -> Result<Canvas<'_>, GalleryError> {
        Canvas::new(&mut self.rgba, self.width, self.height)
    }

    /// The pixel at (`x`, `y`), or `None` outside the sheet.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        let mut out = [0; 4];
        out.copy_from_slice(&self.rgba[at..at + 4]);
        Some(out)
    }
}

/// Renders every tile onto one sheet. Fails on bad buffers or options,
/// on an empty tile list, and when the sheet would exceed
/// [`crate::MAX_DIMENSION`] or [`crate::MAX_PIXELS`].
pub fn contact_sheet(tiles: &[Tile<'_>], options: &SheetOptions) -> Result<Sheet, GalleryError> {
    if tiles.is_empty() {
        return Err(GalleryError::NoTiles);
    }
    let plan = Plan::new(tiles, options)?;
    plan.render(tiles, 0..tiles.len(), options.title.as_deref())
}

/// Renders the tiles onto as many pages as needed to keep each page at
/// most `options.max_height` tall (a single row taller than that still
/// gets its own page). All pages share one cell size and column count.
/// When there is more than one page, each title gains the page's tile
/// range, `"<title> 1-12 of 30"` (or `"Tiles 1-12 of 30"` untitled).
/// No tiles give no pages.
pub fn contact_sheets(
    tiles: &[Tile<'_>],
    options: &SheetOptions,
) -> Result<Vec<Sheet>, GalleryError> {
    if tiles.is_empty() {
        return Ok(Vec::new());
    }
    if options.max_height == 0 {
        return Err(GalleryError::InvalidOption("max_height must be > 0"));
    }
    let plan = Plan::new(tiles, options)?;
    let total_rows = tiles.len().div_ceil(plan.columns as usize) as u64;
    let rows_fitting = |title: bool| -> u64 {
        let title_h = if title { plan.title_height() } else { 0 };
        // A page of r rows is gutter + title_h + r * (cell_h + gutter) tall.
        let available =
            u64::from(options.max_height).saturating_sub(u64::from(plan.gutter) + title_h);
        (available / (plan.cell_h + u64::from(plan.gutter))).max(1)
    };
    if rows_fitting(options.title.is_some()) >= total_rows {
        return Ok(vec![plan.render(
            tiles,
            0..tiles.len(),
            options.title.as_deref(),
        )?]);
    }
    let rows_per_page = rows_fitting(true);
    let per_page = (rows_per_page * u64::from(plan.columns)) as usize;
    let mut pages = Vec::new();
    let mut start = 0;
    while start < tiles.len() {
        let end = (start + per_page).min(tiles.len());
        let range = format!("{}-{} of {}", start + 1, end, tiles.len());
        let title = match &options.title {
            Some(title) => format!("{title} {range}"),
            None => format!("Tiles {range}"),
        };
        pages.push(plan.render(tiles, start..end, Some(&title))?);
        start = end;
    }
    Ok(pages)
}

/// Per-tile drawn sizes.
#[derive(Debug, Clone, Copy)]
struct TilePlan {
    zoom: Zoom,
    /// Drawn sprite size.
    sprite: [u32; 2],
    /// Drawn reference size.
    compare: Option<[u32; 2]>,
    /// Metric lines, the zoom note included.
    lines: u64,
}

/// Geometry shared by every cell and page.
#[derive(Debug)]
struct Plan<'o> {
    options: &'o SheetOptions,
    tiles: Vec<TilePlan>,
    columns: u32,
    border: u32,
    padding: u32,
    gutter: u32,
    text_scale: u32,
    /// Content (image area and text) width.
    inner_w: u64,
    image_h: u64,
    text_lines: u64,
    cell_w: u64,
    cell_h: u64,
}

impl<'o> Plan<'o> {
    fn new(tiles: &[Tile<'_>], options: &'o SheetOptions) -> Result<Self, GalleryError> {
        validate(options)?;
        let text_scale = options.text_scale;
        let caption_w =
            u64::from(text_width(NEW_CAPTION, text_scale).max(text_width(REF_CAPTION, text_scale)));
        let gap = compare_gap(options);
        let mut plans = Vec::with_capacity(tiles.len());
        let mut inner_w = u64::from(options.min_cell_width);
        let mut image_h = 0u64;
        let mut max_lines = 0u64;
        for tile in tiles {
            check_rgba("tile", tile.rgba, tile.width, tile.height)?;
            if let Some((rgba, width, height)) = tile.compare {
                check_rgba("comparison image", rgba, width, height)?;
            }
            let compare = tile.compare.map(|(_, w, h)| [w, h]);
            let zoom = choose_zoom([tile.width, tile.height], compare, options, gap);
            let sprite = [zoom.apply(tile.width), zoom.apply(tile.height)];
            let compare = compare.map(|[w, h]| [zoom.apply(w), zoom.apply(h)]);
            let (content_w, content_h) = match compare {
                None => (u64::from(sprite[0]), u64::from(sprite[1])),
                Some(reference) => (
                    u64::from(sprite[0]).max(caption_w)
                        + u64::from(gap)
                        + u64::from(reference[0]).max(caption_w),
                    u64::from(sprite[1].max(reference[1])) + u64::from(line_height(text_scale)),
                ),
            };
            inner_w = inner_w.max(content_w);
            image_h = image_h.max(content_h);
            let lines = tile.metrics.len() as u64 + u64::from(zoom != Zoom::Up(options.scale));
            max_lines = max_lines.max(lines);
            plans.push(TilePlan {
                zoom,
                sprite,
                compare,
                lines,
            });
        }
        let border = u32::from(options.border.is_some());
        let frame = 2 * u64::from(border) + 2 * u64::from(options.padding);
        let text_lines = 1 + max_lines;
        let cell_w = frame + inner_w;
        let cell_h = frame
            + image_h
            + u64::from(options.padding)
            + text_lines.saturating_mul(u64::from(line_height(text_scale)));
        check_allocation(cell_w, cell_h)?;
        let gutter = u64::from(options.gutter);
        let columns = match options.columns {
            Columns::Fixed(columns) => u64::from(columns),
            Columns::Auto { sheet_width } => {
                (u64::from(sheet_width).saturating_sub(gutter) / (cell_w + gutter)).max(1)
            }
        }
        .min(tiles.len() as u64)
        .max(1);
        Ok(Self {
            options,
            tiles: plans,
            columns: u32::try_from(columns).unwrap_or(u32::MAX),
            border,
            padding: options.padding,
            gutter: options.gutter,
            text_scale,
            inner_w,
            image_h,
            text_lines,
            cell_w,
            cell_h,
        })
    }

    fn title_height(&self) -> u64 {
        u64::from(line_height(self.text_scale)) + u64::from(self.gutter)
    }

    fn render(
        &self,
        tiles: &[Tile<'_>],
        range: std::ops::Range<usize>,
        title: Option<&str>,
    ) -> Result<Sheet, GalleryError> {
        let options = self.options;
        let count = range.len() as u64;
        let columns = u64::from(self.columns);
        let rows = count.div_ceil(columns);
        let gutter = u64::from(self.gutter);
        let title_h = if title.is_some() {
            self.title_height()
        } else {
            0
        };
        let width = gutter + columns * (self.cell_w + gutter);
        let height = gutter + title_h + rows * (self.cell_h + gutter);
        check_allocation(width, height)?;
        let (width, height) = (width as u32, height as u32);
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        let mut canvas = Canvas::new(&mut rgba, width, height)?;
        canvas.fill_rect(0, 0, width, height, options.sheet_colour);
        if let Some(title) = title {
            let max = (u64::from(width).saturating_sub(2 * gutter)) as u32;
            let text = truncate_to_width(title, max, self.text_scale);
            draw_text(
                &mut canvas,
                self.gutter as i32,
                self.gutter as i32,
                &text,
                options.label_colour,
                self.text_scale,
            );
        }
        let top = gutter + title_h;
        let mut cells = Vec::with_capacity(range.len());
        for (slot, index) in range.enumerate() {
            let slot = slot as u64;
            let x = gutter + (slot % columns) * (self.cell_w + gutter);
            let y = top + (slot / columns) * (self.cell_h + gutter);
            // The sheet fits in MAX_DIMENSION, so every coordinate fits in i32.
            cells.push(self.draw_cell(&mut canvas, &tiles[index], index, x as u32, y as u32));
        }
        Ok(Sheet {
            width,
            height,
            rgba,
            cells,
        })
    }

    fn draw_cell(
        &self,
        canvas: &mut Canvas<'_>,
        tile: &Tile<'_>,
        index: usize,
        x: u32,
        y: u32,
    ) -> CellLayout {
        let options = self.options;
        let plan = self.tiles[index];
        let cell = Rect {
            x,
            y,
            width: self.cell_w as u32,
            height: self.cell_h as u32,
        };
        canvas.fill_rect(
            x as i32,
            y as i32,
            cell.width,
            cell.height,
            options.cell_colour,
        );
        if let Some(colour) = options.border {
            canvas.outline_rect(x as i32, y as i32, cell.width, cell.height, colour);
        }
        let inset = self.border + self.padding;
        let ix = x + inset;
        let iy = y + inset;
        let inner_w = self.inner_w as u32;
        let image_h = self.image_h as u32;
        let ts = self.text_scale;

        let (sprite, compare) = match (tile.compare, plan.compare) {
            (Some((ref_rgba, ref_w, ref_h)), Some(ref_size)) => {
                let caption_w = text_width(NEW_CAPTION, ts).max(text_width(REF_CAPTION, ts));
                let gap = compare_gap(options);
                let col1 = plan.sprite[0].max(caption_w);
                let col2 = ref_size[0].max(caption_w);
                let left = ix + (inner_w - (col1 + gap + col2)) / 2;
                let baseline = iy + image_h - line_height(ts);
                let sprite = Rect {
                    x: left + (col1 - plan.sprite[0]) / 2,
                    y: baseline - plan.sprite[1],
                    width: plan.sprite[0],
                    height: plan.sprite[1],
                };
                let reference = Rect {
                    x: left + col1 + gap + (col2 - ref_size[0]) / 2,
                    y: baseline - ref_size[1],
                    width: ref_size[0],
                    height: ref_size[1],
                };
                self.draw_sprite(
                    canvas,
                    tile.rgba,
                    tile.width,
                    tile.height,
                    sprite,
                    plan.zoom,
                );
                self.draw_sprite(canvas, ref_rgba, ref_w, ref_h, reference, plan.zoom);
                for (caption, column_x, column_w) in [
                    (NEW_CAPTION, left, col1),
                    (REF_CAPTION, left + col1 + gap, col2),
                ] {
                    let cx = column_x + (column_w - text_width(caption, ts)) / 2;
                    draw_text(
                        canvas,
                        cx as i32,
                        (baseline + ts) as i32,
                        caption,
                        options.metric_colour,
                        ts,
                    );
                }
                (sprite, Some(reference))
            }
            _ => {
                let [w, h] = plan.sprite;
                let sy = match options.anchor {
                    Anchor::Centre => iy + (image_h - h) / 2,
                    Anchor::Bottom => iy + image_h - h,
                };
                let sprite = Rect {
                    x: ix + (inner_w - w) / 2,
                    y: sy,
                    width: w,
                    height: h,
                };
                self.draw_sprite(
                    canvas,
                    tile.rgba,
                    tile.width,
                    tile.height,
                    sprite,
                    plan.zoom,
                );
                (sprite, None)
            }
        };

        let line = line_height(ts);
        let mut text_y = iy + image_h + self.padding;
        let label = truncate_to_width(tile.label, inner_w, ts);
        draw_text(
            canvas,
            ix as i32,
            text_y as i32,
            &label,
            options.label_colour,
            ts,
        );
        let note = (plan.zoom != Zoom::Up(options.scale))
            .then(|| (SHOWN_KEY.to_owned(), plan.zoom.describe()));
        for (key, value) in tile.metrics.iter().chain(note.as_ref()) {
            text_y += line;
            let text = truncate_to_width(&format!("{key}: {value}"), inner_w, ts);
            draw_text(
                canvas,
                ix as i32,
                text_y as i32,
                &text,
                options.metric_colour,
                ts,
            );
        }
        debug_assert!(plan.lines < self.text_lines);
        CellLayout {
            tile: index,
            cell,
            sprite,
            compare,
            zoom: plan.zoom,
        }
    }

    /// Background, then the image composited over it at `zoom`.
    fn draw_sprite(
        &self,
        canvas: &mut Canvas<'_>,
        rgba: &[u8],
        width: u32,
        height: u32,
        rect: Rect,
        zoom: Zoom,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let (x, y) = (rect.x as i32, rect.y as i32);
        match self.options.background {
            Background::Checker { size, colours } => {
                canvas.checker_rect(x, y, rect.width, rect.height, size, colours);
            }
            Background::Solid(colour) => canvas.fill_rect(x, y, rect.width, rect.height, colour),
        }
        let stride = width as usize * 4;
        for dy in 0..rect.height {
            let sy = zoom.source(dy).min(height.saturating_sub(1)) as usize;
            for dx in 0..rect.width {
                let sx = zoom.source(dx).min(width.saturating_sub(1)) as usize;
                let at = sy * stride + sx * 4;
                let Some(px) = rgba.get(at..at + 4) else {
                    continue;
                };
                canvas.blend_pixel(
                    i64::from(rect.x + dx),
                    i64::from(rect.y + dy),
                    [px[0], px[1], px[2], px[3]],
                );
            }
        }
    }
}

fn compare_gap(options: &SheetOptions) -> u32 {
    options.padding.max(4)
}

fn validate(options: &SheetOptions) -> Result<(), GalleryError> {
    if !(1..=MAX_SCALE).contains(&options.scale) {
        return Err(GalleryError::InvalidOption("scale must be 1 to 16"));
    }
    if !(1..=MAX_TEXT_SCALE).contains(&options.text_scale) {
        return Err(GalleryError::InvalidOption("text_scale must be 1 to 8"));
    }
    if options.max_cell[0] == 0 || options.max_cell[1] == 0 {
        return Err(GalleryError::InvalidOption("max_cell must be > 0"));
    }
    if options.columns == Columns::Fixed(0) {
        return Err(GalleryError::InvalidOption("columns must be > 0"));
    }
    if let Background::Checker { size: 0, .. } = options.background {
        return Err(GalleryError::InvalidOption("checker size must be > 0"));
    }
    // Keep every derived dimension far from overflow.
    let limit = u64::from(crate::MAX_DIMENSION);
    let too_big = u64::from(options.min_cell_width) > limit
        || u64::from(options.padding) > limit
        || u64::from(options.gutter) > limit;
    if too_big {
        return Err(GalleryError::InvalidOption(
            "min_cell_width, padding and gutter must be at most MAX_DIMENSION",
        ));
    }
    Ok(())
}

/// The largest factor up to `options.scale` at which the drawn sprite (or
/// sprite, gap and reference side by side) fits `max_cell`; failing that,
/// the smallest integer reduction that fits, or the strongest reduction.
fn choose_zoom(
    sprite: [u32; 2],
    compare: Option<[u32; 2]>,
    options: &SheetOptions,
    gap: u32,
) -> Zoom {
    let fits = |zoom: Zoom| -> bool {
        let w = u64::from(zoom.apply(sprite[0]));
        let h = u64::from(zoom.apply(sprite[1]));
        let (w, h) = match compare {
            None => (w, h),
            Some([cw, ch]) => (
                w + u64::from(gap) + u64::from(zoom.apply(cw)),
                h.max(u64::from(zoom.apply(ch))),
            ),
        };
        w <= u64::from(options.max_cell[0]) && h <= u64::from(options.max_cell[1])
    };
    for k in (1..=options.scale).rev() {
        if fits(Zoom::Up(k)) {
            return Zoom::Up(k);
        }
    }
    let largest = [sprite, compare.unwrap_or([0, 0])]
        .iter()
        .flat_map(|size| size.iter().copied())
        .max()
        .unwrap_or(1)
        .max(2);
    // Feasible reductions are monotone, so a binary search finds the
    // smallest one that fits.
    if !fits(Zoom::Down(largest)) {
        return Zoom::Down(largest);
    }
    let (mut lo, mut hi) = (2u32, largest);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if fits(Zoom::Down(mid)) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Zoom::Down(lo)
}
