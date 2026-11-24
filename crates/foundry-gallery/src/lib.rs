//! PNG contact sheets of rendered sprites with labels and metrics.
//!
//! [`contact_sheet`] puts RGBA8 sprites (contracts section 5) on a labelled
//! grid; [`contact_sheets`] splits a long list into pages. A tile with a
//! `compare` image is drawn beside it on a common baseline, so scale and
//! palette differences against original art show at a glance;
//! [`frm_frame_rgba`] turns an original FRM frame into such an image.
//! [`film_strips`] lays out one row of frames per animation, each frame
//! placed by its anchor, the action frame marked.
//! Labels use the crate's own 5 x 7 bitmap font ([`font`]). Output is
//! deterministic: one input always gives the same pixels and PNG bytes.

mod canvas;
pub mod font;
mod image;
mod metrics;
mod png_io;
mod reference;
mod sheet;
mod strip;

pub use canvas::Canvas;
pub use font::{draw_text, text_width, truncate_to_width};
pub use image::{Image, MAX_DIMENSION, MAX_PIXELS, Rect};
pub use metrics::{OPAQUE_ALPHA, SpriteMetrics, sprite_metrics};
pub use png_io::{decode_png, encode_png};
pub use reference::{frm_frame_rgba, indexed_to_rgba};
pub use sheet::{
    Anchor, Background, CellLayout, Columns, MAX_SCALE, MAX_TEXT_SCALE, NEW_CAPTION, REF_CAPTION,
    SHOWN_KEY, Sheet, SheetOptions, Tile, Zoom, contact_sheet, contact_sheets,
};
pub use strip::{Strip, StripFrame, StripOptions, film_strips, film_strips_png};

/// Every failure of this crate.
#[derive(Debug, thiserror::Error)]
pub enum GalleryError {
    #[error("{what} buffer is {actual} bytes; {width}x{height} needs {expected}")]
    BufferLength {
        what: &'static str,
        width: u32,
        height: u32,
        expected: u64,
        actual: usize,
    },
    #[error("a {width}x{height} image exceeds the size limit")]
    TooLarge { width: u64, height: u64 },
    #[error("a {width}x{height} image has no pixels")]
    EmptyImage { width: u32, height: u32 },
    #[error("invalid sheet option: {0}")]
    InvalidOption(&'static str),
    #[error("a contact sheet needs at least one tile")]
    NoTiles,
    #[error("PNG encoding failed: {0}")]
    PngEncode(String),
    #[error("PNG decoding failed: {0}")]
    PngDecode(String),
}
