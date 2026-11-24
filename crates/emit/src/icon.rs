//! Inventory icons (`art/inven`, art category 7) and the item art pair.
//!
//! An item has two pictures: its ground art in `art/items` (the
//! prototype's FID) and its inventory art in `art/inven` (the item
//! prototype's inventory FID, SPEC-003 §2.3). Both are one-direction,
//! one-frame FRMs named verbatim by their list entry (SPEC-007 §2.1).
//!
//! **Size.** The specifications give no maximum size for inventory art
//! and no scaling algorithm; they give where it is drawn (SPEC-014 §3.23,
//! §3.34; SPEC-013 §3.16, §3.18):
//!
//! - the inventory, loot and barter lists scale it into 56 × 40 (59 × 40
//!   in barter);
//! - the armour and hand slots and the examine picture scale it into
//!   90 × 61 (an item in both hands into 180 × 61);
//! - the interface bar's item box draws it **unscaled**, centred in
//!   188 × 67;
//! - a dragged item is the cursor at its own size, hotspot at its centre.
//!
//! So this crate's default box is [`INVENTORY_ICON_MAX`], 90 × 61: the
//! largest slot that scales, so no slot enlarges it and the bar shows it
//! whole. A larger picture is reduced to fit, keeping its aspect ratio; a
//! smaller one is kept at its size. The ratio 56 × 40 ≈ 1.4 is the list's;
//! icons near that shape lose least in the lists.

use crate::EmitError;
use crate::art::ArtList;
use crate::frm::{FrmDirection, FrmFrameSpec, FrmSpec, crop_to_content};
use vault_formats::Fid;

/// `art/items`.
pub const ITEM_CATEGORY: u8 = 0;
/// `art/inven`.
pub const INVENTORY_CATEGORY: u8 = 7;
/// This crate's default icon box: the 90 × 61 hand and armour slots.
pub const INVENTORY_ICON_MAX: [u32; 2] = [90, 61];
/// The inventory list's slot, for previews.
pub const INVENTORY_LIST_SLOT: [u32; 2] = [56, 40];

/// The size `width × height` takes in `max` keeping its aspect ratio
/// (never enlarged, at least 1 × 1 for a non-empty image).
pub fn fit_size(width: u32, height: u32, max: [u32; 2]) -> [u32; 2] {
    if width == 0 || height == 0 {
        return [0, 0];
    }
    if width <= max[0] && height <= max[1] {
        return [width, height];
    }
    let scale = (f64::from(max[0]) / f64::from(width)).min(f64::from(max[1]) / f64::from(height));
    [
        ((f64::from(width) * scale).round() as u32).clamp(1, max[0]),
        ((f64::from(height) * scale).round() as u32).clamp(1, max[1]),
    ]
}

fn one_frame(width: u32, height: u32, pixels: Vec<u8>) -> FrmSpec {
    FrmSpec {
        fps: 0,
        action_frame: 0,
        directions: vec![FrmDirection {
            shift: [0, 0],
            frames: vec![FrmFrameSpec {
                width: width as u16,
                height: height as u16,
                displacement: [0, 0],
                pixels,
            }],
        }],
    }
}

fn check_max(max: [u32; 2]) -> Result<(), EmitError> {
    if max[0] == 0 || max[1] == 0 || max[0] > 4096 || max[1] > 4096 {
        return Err(EmitError::Image(format!(
            "icon box {}×{} is outside 1 to 4096",
            max[0], max[1]
        )));
    }
    Ok(())
}

/// An inventory FRM from an indexed picture (index 0 transparent): cropped
/// to its opaque pixels, reduced by nearest-neighbour sampling to fit
/// `max` (see the module notes), one direction, one frame, shift (0, 0),
/// FPS 0. Refused: a buffer that does not match its size, a fully
/// transparent picture, and a box outside 1 to 4,096.
pub fn inventory_frm_indexed(
    pixels: &[u8],
    width: u32,
    height: u32,
    max: [u32; 2],
) -> Result<FrmSpec, EmitError> {
    check_max(max)?;
    let (image, _) = crop_to_content(pixels, width, height)?;
    if image.width == 0 {
        return Err(EmitError::Image(
            "an inventory icon needs an opaque pixel".into(),
        ));
    }
    let [w, h] = fit_size(image.width, image.height, max);
    if [w, h] == [image.width, image.height] {
        return Ok(one_frame(w, h, image.pixels));
    }
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let sy = ((u64::from(y) * 2 + 1) * u64::from(image.height) / (u64::from(h) * 2)) as usize;
        for x in 0..w {
            let sx =
                ((u64::from(x) * 2 + 1) * u64::from(image.width) / (u64::from(w) * 2)) as usize;
            out.push(image.pixels[sy * image.width as usize + sx]);
        }
    }
    // Sampling can drop every opaque pixel of a thin picture; crop again.
    let (sampled, _) = crop_to_content(&out, w, h)?;
    if sampled.width == 0 {
        return Err(EmitError::Image(
            "the icon has no opaque pixel left at this size".into(),
        ));
    }
    Ok(one_frame(sampled.width, sampled.height, sampled.pixels))
}

/// An inventory FRM from an RGBA8 picture: cropped to pixels with alpha
/// 128 or more, reduced to fit `max` by averaging the opaque source
/// pixels each output pixel covers (an output pixel is opaque when at
/// least half its area is), then each opaque colour mapped to a palette
/// index by `quantize`, which must not return the transparent index 0.
/// Otherwise as [`inventory_frm_indexed`].
pub fn inventory_frm_rgba(
    rgba: &[u8],
    width: u32,
    height: u32,
    max: [u32; 2],
    quantize: &dyn Fn([u8; 3]) -> u8,
) -> Result<FrmSpec, EmitError> {
    check_max(max)?;
    let area = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| EmitError::Image(format!("{width}×{height} overflows")))?;
    if rgba.len() != area * 4 {
        return Err(EmitError::Image(format!(
            "{} bytes for a {width}×{height} RGBA image (need {})",
            rgba.len(),
            area * 4
        )));
    }
    // Crop through an opacity mask.
    let mask: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| u8::from(p[3] >= 128))
        .collect();
    let (cropped, [left, top]) = crop_to_content(&mask, width, height)?;
    if cropped.width == 0 {
        return Err(EmitError::Image(
            "an inventory icon needs an opaque pixel".into(),
        ));
    }
    let (cw, ch) = (cropped.width as usize, cropped.height as usize);
    let (left, top) = (left as usize, top as usize);
    let source = |x: usize, y: usize| {
        let at = ((top + y) * width as usize + left + x) * 4;
        &rgba[at..at + 4]
    };
    let [w, h] = fit_size(cropped.width, cropped.height, max);
    let (w, h) = (w as usize, h as usize);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let (y0, y1) = (y * ch / h, ((y + 1) * ch).div_ceil(h).max(y * ch / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * cw / w, ((x + 1) * cw).div_ceil(w).max(x * cw / w + 1));
            let (mut sum, mut opaque, mut total) = ([0_u64; 3], 0_u64, 0_u64);
            for sy in y0..y1.min(ch) {
                for sx in x0..x1.min(cw) {
                    total += 1;
                    let p = source(sx, sy);
                    if p[3] >= 128 {
                        opaque += 1;
                        for c in 0..3 {
                            sum[c] += u64::from(p[c]);
                        }
                    }
                }
            }
            if opaque * 2 < total || opaque == 0 {
                out.push(0);
                continue;
            }
            let colour = sum.map(|s| ((s + opaque / 2) / opaque) as u8);
            let index = quantize(colour);
            if index == 0 {
                return Err(EmitError::Image(format!(
                    "the palette mapping gave the transparent index 0 for {colour:?}"
                )));
            }
            out.push(index);
        }
    }
    let (sampled, _) = crop_to_content(&out, w as u32, h as u32)?;
    if sampled.width == 0 {
        return Err(EmitError::Image(
            "the icon has no opaque pixel left at this size".into(),
        ));
    }
    Ok(one_frame(sampled.width, sampled.height, sampled.pixels))
}

/// An item's two art entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemArt {
    /// `art/items` entry: the prototype's FID.
    pub ground: Fid,
    /// `art/inven` entry: [`crate::proto::ItemStats::inventory_art`].
    pub inventory: Fid,
}

/// Add (or find) an item's ground art in `items` and its inventory art in
/// `inven`, both list file names (1 to 12 ASCII bytes with the extension,
/// as `ArtList::push` accepts). Refused: lists of the wrong categories,
/// and what `ArtList::push` refuses (including the 4,096-entry budget).
pub fn push_item_art(
    items: &mut ArtList,
    inven: &mut ArtList,
    ground: &str,
    inventory: &str,
) -> Result<ItemArt, EmitError> {
    if items.category() != ITEM_CATEGORY || inven.category() != INVENTORY_CATEGORY {
        return Err(EmitError::Path {
            path: format!("{} and {}", items.list_path(), inven.list_path()),
            message: "item art goes in art/items and art/inven".into(),
        });
    }
    items.reserve(usize::from(items.find(ground).is_none()))?;
    inven.reserve(usize::from(inven.find(inventory).is_none()))?;
    let ground = items.push(ground)?;
    let inventory = inven.push(inventory)?;
    Ok(ItemArt {
        ground: items.fid(ground),
        inventory: inven.fid(inventory),
    })
}
