//! FRM sprites: the writer, and the helpers that turn a rendered image into
//! a frame the engine draws where it was rendered.
//!
//! The layout is the one `vault_formats::frm::FrmArchive::parse` reads
//! (SPEC-001 §2.2, §2.3 and §3.3): a 62-byte big-endian header, then the
//! direction blocks packed in direction order, each holding the header's
//! frame count of `12 + width × height` byte records.

use crate::EmitError;
use vault_formats::MAX_ASSET_BYTES;

/// The version word this writer stores. The parser keeps it without
/// interpreting it; 4 is the value SPEC-001's fixtures use.
pub const FRM_VERSION: u32 = 4;
/// The fixed header length; direction offsets count from here.
pub const FRM_HEADER_BYTES: usize = 62;
/// The largest width, height, frame count, FPS or action frame: the file
/// stores them as signed 16-bit numbers and the parser rejects negatives.
pub const FRM_MAX_FIELD: u16 = i16::MAX as u16;

/// A whole sprite: one direction (scenery, walls, items) or six (critters
/// and other directional art), each with the same number of frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrmSpec {
    /// Stored frames per second; 0 means the engine's default of 10.
    pub fps: u16,
    /// The action-frame timing value, stored as is.
    pub action_frame: u16,
    /// One or six directions, in direction order (0 is north-east).
    pub directions: Vec<FrmDirection>,
}

/// One direction's shift and frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrmDirection {
    /// Added to the hex anchor before the frame's bottom-centre is placed.
    pub shift: [i16; 2],
    pub frames: Vec<FrmFrameSpec>,
}

/// One frame: indexed pixels, row-major, top row first, index 0 transparent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrmFrameSpec {
    pub width: u16,
    pub height: u16,
    /// The per-frame displacement increment the engine accumulates while
    /// animating (SPEC-001 §3.5); 0, 0 for a still image.
    pub displacement: [i16; 2],
    /// Exactly `width × height` palette indexes.
    pub pixels: Vec<u8>,
}

fn put_i16(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn field(value: u16, name: &str) -> Result<i16, EmitError> {
    i16::try_from(value).map_err(|_| {
        EmitError::Frm(format!(
            "{name} {value} exceeds the format's {FRM_MAX_FIELD}"
        ))
    })
}

/// Serialize `spec` as `FrmArchive::parse` reads it.
///
/// - A one-direction sprite stores one block; all six offsets are 0 and all
///   six shifts are the direction's shift, so every direction the engine
///   asks for gives the same frames and placement.
/// - A six-direction sprite stores the blocks packed in order. A direction
///   whose frames equal the previous direction's shares its block (equal
///   adjacent offsets, which the parser reads as one shared sequence) but
///   keeps its own shift.
/// - The declared frame-area length is the stored byte count after the
///   header.
pub fn write_frm(spec: &FrmSpec) -> Result<Vec<u8>, EmitError> {
    if !matches!(spec.directions.len(), 1 | 6) {
        return Err(EmitError::Frm(format!(
            "a sprite has 1 or 6 directions, not {}",
            spec.directions.len()
        )));
    }
    let fps = field(spec.fps, "FPS")?;
    let action_frame = field(spec.action_frame, "action frame")?;
    let frame_count = spec.directions[0].frames.len();
    if frame_count == 0 {
        return Err(EmitError::Frm(
            "a direction needs at least one frame".into(),
        ));
    }
    let count = i16::try_from(frame_count).map_err(|_| {
        EmitError::Frm(format!(
            "{frame_count} frames per direction exceed the format's {FRM_MAX_FIELD}"
        ))
    })?;
    for (index, direction) in spec.directions.iter().enumerate() {
        if direction.frames.len() != frame_count {
            return Err(EmitError::Frm(format!(
                "direction {index} has {} frames and direction 0 has {frame_count}; \
                 every direction stores the same count",
                direction.frames.len()
            )));
        }
        for (number, frame) in direction.frames.iter().enumerate() {
            let at = format!("direction {index} frame {number}");
            field(frame.width, &format!("{at} width"))?;
            field(frame.height, &format!("{at} height"))?;
            let area = usize::from(frame.width) * usize::from(frame.height);
            if frame.pixels.len() != area {
                return Err(EmitError::Frm(format!(
                    "{at}: {} pixel bytes for a {}×{} frame (need {area})",
                    frame.pixels.len(),
                    frame.width,
                    frame.height
                )));
            }
        }
    }
    // The six header slots and the direction each one reads.
    let slot = |d: usize| {
        if spec.directions.len() == 1 {
            &spec.directions[0]
        } else {
            &spec.directions[d]
        }
    };
    let mut data = Vec::new();
    let mut offsets = [0_usize; 6];
    for d in 0..6 {
        if d > 0 && slot(d).frames == slot(d - 1).frames {
            offsets[d] = offsets[d - 1];
            continue;
        }
        offsets[d] = data.len();
        for frame in &slot(d).frames {
            put_i16(&mut data, frame.width as i16);
            put_i16(&mut data, frame.height as i16);
            put_i32(&mut data, frame.pixels.len() as i32);
            put_i16(&mut data, frame.displacement[0]);
            put_i16(&mut data, frame.displacement[1]);
            data.extend_from_slice(&frame.pixels);
            if FRM_HEADER_BYTES + data.len() > MAX_ASSET_BYTES {
                return Err(EmitError::Frm(format!(
                    "the sprite exceeds the {MAX_ASSET_BYTES}-byte asset limit"
                )));
            }
        }
    }
    let mut out = Vec::with_capacity(FRM_HEADER_BYTES + data.len());
    out.extend_from_slice(&FRM_VERSION.to_be_bytes());
    put_i16(&mut out, fps);
    put_i16(&mut out, action_frame);
    put_i16(&mut out, count);
    for axis in 0..2 {
        for d in 0..6 {
            put_i16(&mut out, slot(d).shift[axis]);
        }
    }
    for offset in offsets {
        // Bounded by the asset limit above, so it fits a signed word.
        put_i32(&mut out, offset as i32);
    }
    put_i32(&mut out, data.len() as i32);
    debug_assert_eq!(out.len(), FRM_HEADER_BYTES);
    out.extend_from_slice(&data);
    Ok(out)
}

/// The direction shift that makes the engine draw a `width × height` frame
/// with its pixel (0, 0) at `origin`, measured from the object's anchor,
/// with no accumulated frame displacement.
///
/// The anchor is the point the engine places the object at: the
/// screen-space hex centre, `HexGrid::hex_center` (SPEC-001 §3.5). The
/// engine puts the frame's bottom-centre pixel, `(width / 2, height - 1)`
/// with the division truncated, at anchor plus shift
/// (`FrmFrame::world_rect`), so the shift is `origin + (width / 2,
/// height - 1)`. A shift outside the signed 16-bit range is an error.
pub fn sprite_shift(origin: [i32; 2], width: u32, height: u32) -> Result<[i16; 2], EmitError> {
    let x = i64::from(origin[0]) + i64::from(width / 2);
    let y = i64::from(origin[1]) + i64::from(height) - 1;
    match (i16::try_from(x), i16::try_from(y)) {
        (Ok(x), Ok(y)) => Ok([x, y]),
        _ => Err(EmitError::Frm(format!(
            "the shift ({x}, {y}) for origin {origin:?} and size {width}×{height} \
             does not fit the format's signed 16-bit fields"
        ))),
    }
}

/// An indexed image without its transparent margins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CroppedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Crop an indexed image (index 0 transparent) to the smallest rectangle
/// holding every opaque pixel. Returns the cropped image and the position
/// of its top-left pixel in the original, which is what to add to the
/// original's origin. A fully transparent image crops to 0 × 0 at (0, 0).
pub fn crop_to_content(
    indexed: &[u8],
    width: u32,
    height: u32,
) -> Result<(CroppedImage, [i32; 2]), EmitError> {
    let w = width as usize;
    let h = height as usize;
    let area = w
        .checked_mul(h)
        .ok_or_else(|| EmitError::Image(format!("{width}×{height} overflows")))?;
    if indexed.len() != area {
        return Err(EmitError::Image(format!(
            "{} bytes for a {width}×{height} indexed image (need {area})",
            indexed.len()
        )));
    }
    if i32::try_from(width).is_err() || i32::try_from(height).is_err() {
        return Err(EmitError::Image(format!(
            "{width}×{height} exceeds the signed 32-bit origin range"
        )));
    }
    let opaque_row = |y: usize| indexed[y * w..(y + 1) * w].iter().any(|p| *p != 0);
    let Some(top) = (0..h).find(|y| opaque_row(*y)) else {
        return Ok((
            CroppedImage {
                width: 0,
                height: 0,
                pixels: Vec::new(),
            },
            [0, 0],
        ));
    };
    let bottom = (0..h).rev().find(|y| opaque_row(*y)).unwrap_or(top);
    let opaque_column = |x: usize| (top..=bottom).any(|y| indexed[y * w + x] != 0);
    let left = (0..w).find(|x| opaque_column(*x)).unwrap_or(0);
    let right = (0..w).rev().find(|x| opaque_column(*x)).unwrap_or(left);
    let mut pixels = Vec::with_capacity((right - left + 1) * (bottom - top + 1));
    for y in top..=bottom {
        pixels.extend_from_slice(&indexed[y * w + left..=y * w + right]);
    }
    Ok((
        CroppedImage {
            width: (right - left + 1) as u32,
            height: (bottom - top + 1) as u32,
            pixels,
        },
        [left as i32, top as i32],
    ))
}

/// A cropped frame and the direction shift that places it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedFrame {
    pub frame: FrmFrameSpec,
    pub shift: [i16; 2],
}

/// [`crop_to_content`] then [`sprite_shift`]: a still frame (zero
/// displacement) from a rendered indexed image whose pixel (0, 0) sits at
/// `origin` from the anchor, ready for a one-direction [`FrmSpec`].
pub fn placed_frame(
    indexed: &[u8],
    width: u32,
    height: u32,
    origin: [i32; 2],
) -> Result<PlacedFrame, EmitError> {
    let (cropped, offset) = crop_to_content(indexed, width, height)?;
    let origin = [
        origin[0]
            .checked_add(offset[0])
            .ok_or_else(|| EmitError::Image("origin overflows".into()))?,
        origin[1]
            .checked_add(offset[1])
            .ok_or_else(|| EmitError::Image("origin overflows".into()))?,
    ];
    let shift = sprite_shift(origin, cropped.width, cropped.height)?;
    let dimension = |value: u32, name: &str| {
        u16::try_from(value)
            .ok()
            .filter(|value| *value <= FRM_MAX_FIELD)
            .ok_or_else(|| {
                EmitError::Frm(format!(
                    "cropped {name} {value} exceeds the format's {FRM_MAX_FIELD}"
                ))
            })
    };
    Ok(PlacedFrame {
        frame: FrmFrameSpec {
            width: dimension(cropped.width, "width")?,
            height: dimension(cropped.height, "height")?,
            displacement: [0, 0],
            pixels: cropped.pixels,
        },
        shift,
    })
}
