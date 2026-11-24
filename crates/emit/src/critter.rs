//! Critter animations: from rendered frames to one six-direction FRM.
//!
//! The engine draws a critter frame with its bottom-centre pixel at the hex
//! centre plus the direction shift plus the displacement it has
//! accumulated from the frames shown so far (SPEC-001 §3.5). A walk moves
//! the critter only through those per-frame displacements: it crosses into
//! the next hex once the accumulated displacement reaches the facing's hex
//! step on either axis, keeps the remainder, and drops the last remainder
//! when the walk ends (SPEC-007 §3.11.1). [`critter_frm`] turns frames
//! rendered in place into that representation:
//!
//! - **Placement.** Every frame is cropped to its opaque pixels, then
//!   padded with transparent index 0 to the union rectangle of its
//!   direction's cropped frames. So every frame of one direction has the
//!   same size, and the one direction shift places each of them where it
//!   was rendered relative to the character's ground point. A direction
//!   whose frames are all transparent stores 0 × 0 frames and shift (0, 0).
//! - **Travel.** Only the component of a frame's `travel` along the
//!   facing's hex step counts; the perpendicular part is dropped. The
//!   cycle's total along the step is rounded to a whole number of the
//!   step's smallest integer vector ((4, ∓3) on the diagonals, (±1, 0) east
//!   and west), at least one when the total is positive, so the offsets of a
//!   cycle sum exactly along the step and repeated cycles never drift off
//!   its axis. The running total is then rounded per axis at every frame
//!   (half away from zero), and each frame's offset is the difference from
//!   the previous frame's: whole pixels, remainder carried, and the last
//!   frame lands exactly on the cycle total. Zero total travel (a stand)
//!   gives zero offsets; a negative total (walking backwards) is refused.

use crate::EmitError;
use crate::frm::{
    FRM_MAX_FIELD, FrmDirection, FrmFrameSpec, FrmSpec, crop_to_content, sprite_shift,
};
use vault_formats::MAX_ASSET_BYTES;

/// The screen step to the neighbouring hex for each direction, NE, E, SE,
/// SW, W, NW (SPEC-007 §3.11.1); also `HexGrid::hex_center` of
/// `HexGrid::neighbor` minus that of the hex.
pub const HEX_STEPS: [[i32; 2]; 6] = [
    [16, -12],
    [32, 0],
    [16, 12],
    [-16, 12],
    [-32, 0],
    [-16, -12],
];

/// One rendered frame of one direction.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedFrame {
    /// Screen position of pixel (0, 0) relative to the character's ground
    /// point in this frame (the walk is rendered in place).
    pub origin: [i32; 2],
    pub width: u32,
    pub height: u32,
    /// Palette indexes, row-major, top row first, 0 transparent.
    pub pixels: Vec<u8>,
    /// Screen pixels the ground point moves from the previous frame to this
    /// one (for frame 0, from the cycle's last frame). The engine adds a
    /// frame's offset when it enters the frame, so this is the distance
    /// walked "in" this frame. Zero for a stand.
    pub travel: [f32; 2],
}

/// One animation: the same number of frames in each of the six
/// directions, NE first.
#[derive(Debug, Clone, PartialEq)]
pub struct CritterFrames {
    /// Stored frames per second; 0 means the engine's default of 10.
    pub fps: u16,
    pub directions: [Vec<RenderedFrame>; 6],
}

/// The smallest integer vector along `step` and how many of it make the
/// step.
fn step_unit(step: [i32; 2]) -> ([i32; 2], i32) {
    let (mut a, mut b) = (step[0].abs(), step[1].abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    ([step[0] / a, step[1] / a], a)
}

/// The per-frame offsets of one direction's cycle, from each frame's
/// travel, by the rule in the module notes.
pub fn travel_offsets(direction: usize, travel: &[[f32; 2]]) -> Result<Vec<[i16; 2]>, EmitError> {
    let step = *HEX_STEPS
        .get(direction)
        .ok_or_else(|| EmitError::Frm(format!("direction {direction} is outside 0 to 5")))?;
    let (unit, units) = step_unit(step);
    let length2 = f64::from(step[0] * step[0] + step[1] * step[1]);
    // Running totals in hex steps.
    let mut running = Vec::with_capacity(travel.len());
    let mut total = 0.0_f64;
    for (frame, moved) in travel.iter().enumerate() {
        if !moved.iter().all(|value| value.is_finite()) {
            return Err(EmitError::Frm(format!(
                "direction {direction} frame {frame}: travel {moved:?} is not finite"
            )));
        }
        total += (f64::from(moved[0]) * f64::from(step[0])
            + f64::from(moved[1]) * f64::from(step[1]))
            / length2;
        running.push(total);
    }
    if total < 0.0 {
        return Err(EmitError::Frm(format!(
            "direction {direction}: the cycle travels {total:.4} hex steps, away from its facing"
        )));
    }
    if total == 0.0 {
        return Ok(vec![[0, 0]; travel.len()]);
    }
    let whole = (total * f64::from(units)).round().max(1.0);
    let target = [f64::from(unit[0]) * whole, f64::from(unit[1]) * whole];
    let mut previous = [0_i64; 2];
    let mut offsets = Vec::with_capacity(travel.len());
    for (frame, sum) in running.iter().enumerate() {
        let position: [i64; 2] = if frame + 1 == running.len() {
            [target[0] as i64, target[1] as i64]
        } else {
            let fraction = sum / total;
            [
                (fraction * target[0]).round() as i64,
                (fraction * target[1]).round() as i64,
            ]
        };
        let mut offset = [0_i16; 2];
        for axis in 0..2 {
            let delta = position[axis] - previous[axis];
            offset[axis] = i16::try_from(delta).map_err(|_| {
                EmitError::Frm(format!(
                    "direction {direction} frame {frame}: offset {delta} does not fit the \
                     format's signed 16-bit field"
                ))
            })?;
        }
        previous = position;
        offsets.push(offset);
    }
    Ok(offsets)
}

/// One direction's frames padded to their union rectangle, its shift, and
/// each frame's offset.
fn place_direction(
    direction: usize,
    frames: &[RenderedFrame],
    offsets: &[[i16; 2]],
) -> Result<FrmDirection, EmitError> {
    // Cropped frames and their rectangles in ground-point coordinates.
    let mut cropped = Vec::with_capacity(frames.len());
    let mut union: Option<[i64; 4]> = None; // left, top, right, bottom (inclusive)
    for (index, frame) in frames.iter().enumerate() {
        let (image, offset) =
            crop_to_content(&frame.pixels, frame.width, frame.height).map_err(|error| {
                EmitError::Image(format!("direction {direction} frame {index}: {error}"))
            })?;
        let left = i64::from(frame.origin[0]) + i64::from(offset[0]);
        let top = i64::from(frame.origin[1]) + i64::from(offset[1]);
        if image.width > 0 {
            let right = left + i64::from(image.width) - 1;
            let bottom = top + i64::from(image.height) - 1;
            union = Some(match union {
                None => [left, top, right, bottom],
                Some([l, t, r, b]) => [l.min(left), t.min(top), r.max(right), b.max(bottom)],
            });
        }
        cropped.push((image, left, top));
    }
    let Some([left, top, right, bottom]) = union else {
        return Ok(FrmDirection {
            shift: [0, 0],
            frames: offsets
                .iter()
                .map(|displacement| FrmFrameSpec {
                    width: 0,
                    height: 0,
                    displacement: *displacement,
                    pixels: Vec::new(),
                })
                .collect(),
        });
    };
    let width = right - left + 1;
    let height = bottom - top + 1;
    let max = i64::from(FRM_MAX_FIELD);
    if width > max || height > max {
        return Err(EmitError::Frm(format!(
            "direction {direction}: its frames span {width}×{height} pixels; a frame holds at \
             most {FRM_MAX_FIELD} on each side"
        )));
    }
    let area = (width * height) as usize;
    if area.saturating_mul(frames.len()) > MAX_ASSET_BYTES {
        return Err(EmitError::Frm(format!(
            "direction {direction}: {} frames of {width}×{height} exceed the \
             {MAX_ASSET_BYTES}-byte asset limit",
            frames.len()
        )));
    }
    let origin = [
        i32::try_from(left).map_err(|_| EmitError::Frm("frame origin overflows".into()))?,
        i32::try_from(top).map_err(|_| EmitError::Frm("frame origin overflows".into()))?,
    ];
    let shift = sprite_shift(origin, width as u32, height as u32)
        .map_err(|error| EmitError::Frm(format!("direction {direction}: {error}")))?;
    let stride = width as usize;
    let frames = cropped
        .into_iter()
        .zip(offsets)
        .map(|((image, x, y), displacement)| {
            let mut pixels = vec![0_u8; area];
            let (dx, dy) = ((x - left) as usize, (y - top) as usize);
            let w = image.width as usize;
            for row in 0..image.height as usize {
                let at = (dy + row) * stride + dx;
                pixels[at..at + w].copy_from_slice(&image.pixels[row * w..(row + 1) * w]);
            }
            FrmFrameSpec {
                width: width as u16,
                height: height as u16,
                displacement: *displacement,
                pixels,
            }
        })
        .collect();
    Ok(FrmDirection { shift, frames })
}

/// Six directions of rendered frames as one critter [`FrmSpec`]; see the
/// module notes for the placement and travel rules.
///
/// Refused: a direction with no frames or with another frame count than
/// direction 0, more than 32,767 frames, an FPS above 32,767, an
/// `action_frame` not below the frame count, a pixel buffer that does not
/// match its size, non-finite or backward travel, and anything whose
/// size, shift or offsets the format cannot store.
pub fn critter_frm(frames: &CritterFrames, action_frame: u16) -> Result<FrmSpec, EmitError> {
    critter_frm_with(
        frames,
        &FrmOptions {
            action_frame,
            motion: Motion::Cycle,
        },
    )
}

/// How a critter animation's `travel` becomes per-frame offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Motion {
    /// A walk or run cycle, or a stand ([`travel_offsets`]): travel along
    /// the facing's hex step only, the cycle's total rounded to whole hex
    /// units so repeated cycles land on hexes.
    #[default]
    Cycle,
    /// A non-looping animation (a fall, a knockback, a death) whose figure
    /// ends away from where it started: the offsets follow `travel` in
    /// both axes exactly, each frame rounded with the remainder carried
    /// ([`free_offsets`]), so after frame `k` the engine's accumulated
    /// displacement is the rounded total travel through `k` (SPEC-001
    /// §3.5). The critter keeps that displacement while it shows the
    /// animation's last frame; the format has no way to say where it
    /// should return, so the next animation (a get-up) must carry
    /// offsets that take it back, or use [`Motion::Baked`].
    Free,
    /// A non-looping animation drawn with zero offsets: each frame's
    /// pixels are moved by its rounded total travel instead, so the figure
    /// is drawn in the same place as with [`Motion::Free`] but the
    /// object's displacement never changes. This is the robust choice for
    /// falls and get-ups: it depends on no displacement left by an
    /// earlier animation. It costs a larger frame rectangle.
    Baked,
}

/// What [`critter_frm_with`] takes beyond the frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrmOptions {
    /// The FRM header's action-frame value (SPEC-001 §3.4): a delay in
    /// frame steps, below the frame count. [`action_frame_for_contact`]
    /// gives it for the frame on which a hit lands.
    pub action_frame: u16,
    pub motion: Motion,
}

/// The action-frame value for an attack, reach or dodge whose contact is
/// shown on zero-based frame `contact` of `count`.
///
/// The value is a start delay: an entry delayed by `A` starts on the
/// frame step that shows frame `A - 1` (SPEC-007 §3.10.1), so a contact on
/// frame `f` is `A = f + 1`, and 0 means at once. [`critter_frm`] keeps
/// the value below the frame count, so a contact on the last frame is
/// moved one frame earlier. Corpus examples have `A` from 1 to 7 on 5 to
/// 17 frames (SPEC-007 §3.10.2).
pub fn action_frame_for_contact(contact: usize, count: usize) -> Result<u16, EmitError> {
    if count == 0 || contact >= count {
        return Err(EmitError::Frm(format!(
            "contact frame {contact} is not one of {count} frames"
        )));
    }
    let value = (contact + 1).min(count - 1);
    u16::try_from(value)
        .ok()
        .filter(|value| *value <= FRM_MAX_FIELD)
        .ok_or_else(|| EmitError::Frm(format!("action frame {value} exceeds {FRM_MAX_FIELD}")))
}

/// The action-frame value for a contact at `seconds` into an animation
/// played at `fps` (stored 0 means 10): frame `round(seconds × fps)`,
/// then [`action_frame_for_contact`].
pub fn action_frame_at(seconds: f32, fps: u16, count: usize) -> Result<u16, EmitError> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(EmitError::Frm(format!(
            "contact time {seconds} is not a time"
        )));
    }
    let fps = if fps == 0 { 10 } else { fps };
    let frame = (f64::from(seconds) * f64::from(fps)).round();
    let frame = if frame >= count as f64 {
        count.saturating_sub(1)
    } else {
        frame as usize
    };
    action_frame_for_contact(frame, count)
}

/// Per-frame offsets that follow `travel` exactly ([`Motion::Free`]): the
/// running total of each axis rounded half away from zero at every frame,
/// each offset the difference from the previous frame's. Refused:
/// non-finite travel, and an offset outside the signed 16-bit field.
pub fn free_offsets(travel: &[[f32; 2]]) -> Result<Vec<[i16; 2]>, EmitError> {
    let totals = running_totals(travel)?;
    let mut previous = [0_i64; 2];
    let mut offsets = Vec::with_capacity(travel.len());
    for (frame, total) in totals.iter().enumerate() {
        let mut offset = [0_i16; 2];
        for axis in 0..2 {
            let delta = total[axis] - previous[axis];
            offset[axis] = i16::try_from(delta).map_err(|_| {
                EmitError::Frm(format!(
                    "frame {frame}: offset {delta} does not fit the format's signed 16-bit field"
                ))
            })?;
        }
        previous = *total;
        offsets.push(offset);
    }
    Ok(offsets)
}

/// Each frame's rounded total travel through it.
fn running_totals(travel: &[[f32; 2]]) -> Result<Vec<[i64; 2]>, EmitError> {
    let mut sum = [0.0_f64; 2];
    travel
        .iter()
        .enumerate()
        .map(|(frame, moved)| {
            if !moved.iter().all(|value| value.is_finite()) {
                return Err(EmitError::Frm(format!(
                    "frame {frame}: travel {moved:?} is not finite"
                )));
            }
            sum[0] += f64::from(moved[0]);
            sum[1] += f64::from(moved[1]);
            let rounded = sum.map(|value| value.round() as i64);
            if rounded
                .iter()
                .any(|value| value.abs() > i64::from(i32::MAX) / 2)
            {
                return Err(EmitError::Frm(format!("frame {frame}: travel overflows")));
            }
            Ok(rounded)
        })
        .collect()
}

/// [`critter_frm`] with a choice of [`Motion`]. Refused as
/// [`critter_frm`].
pub fn critter_frm_with(
    frames: &CritterFrames,
    options: &FrmOptions,
) -> Result<FrmSpec, EmitError> {
    let action_frame = options.action_frame;
    let count = frames.directions[0].len();
    if count == 0 {
        return Err(EmitError::Frm(
            "a critter animation needs at least one frame per direction".into(),
        ));
    }
    for (direction, list) in frames.directions.iter().enumerate() {
        if list.len() != count {
            return Err(EmitError::Frm(format!(
                "direction {direction} has {} frames and direction 0 has {count}; every \
                 direction has the same count",
                list.len()
            )));
        }
    }
    if count > usize::from(FRM_MAX_FIELD) || frames.fps > FRM_MAX_FIELD {
        return Err(EmitError::Frm(format!(
            "{count} frames at {} fps; the format stores at most {FRM_MAX_FIELD} of each",
            frames.fps
        )));
    }
    if usize::from(action_frame) >= count {
        return Err(EmitError::Frm(format!(
            "action frame {action_frame} is not one of the {count} frames"
        )));
    }
    let mut directions = Vec::with_capacity(6);
    for (direction, list) in frames.directions.iter().enumerate() {
        let travel: Vec<[f32; 2]> = list.iter().map(|frame| frame.travel).collect();
        let placed = match options.motion {
            Motion::Cycle => {
                let offsets = travel_offsets(direction, &travel)?;
                place_direction(direction, list, &offsets)?
            }
            Motion::Free => {
                let offsets = free_offsets(&travel).map_err(|error| match error {
                    EmitError::Frm(message) => {
                        EmitError::Frm(format!("direction {direction}: {message}"))
                    }
                    other => other,
                })?;
                place_direction(direction, list, &offsets)?
            }
            Motion::Baked => {
                let totals = running_totals(&travel)?;
                let moved = list
                    .iter()
                    .zip(&totals)
                    .map(|(frame, total)| {
                        let shift = |origin: i32, by: i64| {
                            i32::try_from(i64::from(origin) + by).map_err(|_| {
                                EmitError::Frm(format!("direction {direction}: origin overflows"))
                            })
                        };
                        Ok(RenderedFrame {
                            origin: [
                                shift(frame.origin[0], total[0])?,
                                shift(frame.origin[1], total[1])?,
                            ],
                            travel: [0.0, 0.0],
                            ..frame.clone()
                        })
                    })
                    .collect::<Result<Vec<_>, EmitError>>()?;
                place_direction(direction, &moved, &vec![[0, 0]; moved.len()])?
            }
        };
        directions.push(placed);
    }
    Ok(FrmSpec {
        fps: frames.fps,
        action_frame,
        directions,
    })
}

/// A still animation from the first frame of each direction of `source`:
/// one frame per direction, same shifts and FPS, zero offsets, action
/// frame 0. This is the placeholder for the animations a critter needs
/// before real art exists (hit, falls, getting up, corpses, attacks): the
/// critter shows that pose on its hex and the animation ends at once.
pub fn still_frm(source: &FrmSpec) -> Result<FrmSpec, EmitError> {
    if !matches!(source.directions.len(), 1 | 6) {
        return Err(EmitError::Frm(format!(
            "a sprite has 1 or 6 directions, not {}",
            source.directions.len()
        )));
    }
    let directions = source
        .directions
        .iter()
        .enumerate()
        .map(|(index, direction)| {
            let first = direction
                .frames
                .first()
                .ok_or_else(|| EmitError::Frm(format!("direction {index} has no frame to hold")))?;
            Ok(FrmDirection {
                shift: direction.shift,
                frames: vec![FrmFrameSpec {
                    displacement: [0, 0],
                    ..first.clone()
                }],
            })
        })
        .collect::<Result<_, EmitError>>()?;
    Ok(FrmSpec {
        fps: source.fps,
        action_frame: 0,
        directions,
    })
}
