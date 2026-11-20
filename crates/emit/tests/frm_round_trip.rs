//! FRM writer round trips through `FrmArchive::parse`, and the placement
//! helpers checked against `FrmFrame::world_rect`.

use emit::frm::{FRM_HEADER_BYTES, FRM_VERSION, write_frm};
use emit::{
    EmitError, FormatWriters, FrmDirection, FrmFrameSpec, FrmSpec, LocalWriters, crop_to_content,
    placed_frame, sprite_shift,
};
use vault_formats::{FrmArchive, HexGrid};

/// A synthetic frame whose pixels encode their position and a seed.
fn frame(width: u16, height: u16, displacement: [i16; 2], seed: u8) -> FrmFrameSpec {
    let pixels = (0..usize::from(width) * usize::from(height))
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect();
    FrmFrameSpec {
        width,
        height,
        displacement,
        pixels,
    }
}

fn round_trip(spec: &FrmSpec) -> FrmArchive {
    let bytes = LocalWriters.frm(spec).expect("writes");
    let archive = FrmArchive::parse(&bytes).expect("parses");
    assert_eq!(archive.version, FRM_VERSION);
    assert_eq!(archive.fps, spec.fps as i16);
    assert_eq!(archive.action_frame, spec.action_frame as i16);
    let frames = spec.directions[0].frames.len();
    assert_eq!(archive.frame_count(), frames);
    assert_eq!(
        archive.frame_area_len as usize,
        bytes.len() - FRM_HEADER_BYTES
    );
    assert_eq!(
        archive.stored_frame_area_len() as usize,
        bytes.len() - FRM_HEADER_BYTES
    );
    for d in 0..6 {
        let direction = if spec.directions.len() == 1 {
            &spec.directions[0]
        } else {
            &spec.directions[d]
        };
        assert_eq!(archive.shifts[d], direction.shift, "direction {d} shift");
        for (f, expected) in direction.frames.iter().enumerate() {
            let got = archive.frame(d, f).expect("frame present");
            assert_eq!(got.width, expected.width, "d{d} f{f} width");
            assert_eq!(got.height, expected.height, "d{d} f{f} height");
            assert_eq!(
                got.displacement, expected.displacement,
                "d{d} f{f} displacement"
            );
            assert_eq!(got.pixels(), expected.pixels.as_slice(), "d{d} f{f} pixels");
        }
        assert!(archive.frame(d, frames).is_none());
    }
    archive
}

#[test]
fn one_direction_shares_one_block_across_all_six_slots() {
    let spec = FrmSpec {
        fps: 0,
        action_frame: 0,
        directions: vec![FrmDirection {
            shift: [-7, 23],
            frames: vec![frame(5, 3, [0, 0], 1)],
        }],
    };
    let archive = round_trip(&spec);
    assert_eq!(archive.offsets, [0; 6]);
    assert_eq!(archive.stored_sequences(), 1);
    assert_eq!(archive.effective_fps(), 10);
    // 62-byte header plus one 12-byte record and 15 pixels.
    assert_eq!(write_frm(&spec).unwrap().len(), 62 + 12 + 15);
}

#[test]
fn six_directions_pack_in_order_with_negative_values() {
    let directions = (0..6)
        .map(|d| FrmDirection {
            shift: [-(d as i16) * 3, d as i16 * 5 - 9],
            frames: (0..3)
                .map(|f| {
                    frame(
                        4 + d as u16,
                        2 + f as u16,
                        [-(f as i16), f as i16 - 2],
                        (d * 10 + f) as u8,
                    )
                })
                .collect(),
        })
        .collect();
    let spec = FrmSpec {
        fps: 12,
        action_frame: 2,
        directions,
    };
    let archive = round_trip(&spec);
    assert_eq!(archive.stored_sequences(), 6);
    assert_eq!(archive.offsets[0], 0);
    for d in 1..6 {
        assert!(archive.offsets[d] > archive.offsets[d - 1]);
    }
    assert_eq!(archive.frame_interval_ms(), Some(83));
    assert_eq!(archive.accumulated_displacement(0, 2), Some([-3, -3]));
}

#[test]
fn equal_adjacent_directions_share_a_block_but_keep_their_shifts() {
    let a = vec![frame(3, 3, [1, -1], 7), frame(2, 2, [0, 0], 8)];
    let b = vec![frame(4, 1, [0, 0], 9), frame(1, 4, [0, 0], 10)];
    let directions = [&a, &a, &b, &b, &b, &a]
        .iter()
        .enumerate()
        .map(|(d, frames)| FrmDirection {
            shift: [d as i16, -(d as i16)],
            frames: (*frames).clone(),
        })
        .collect();
    let spec = FrmSpec {
        fps: 8,
        action_frame: 0,
        directions,
    };
    let archive = round_trip(&spec);
    assert_eq!(archive.stored_sequences(), 3);
    assert_eq!(archive.offsets[0], archive.offsets[1]);
    assert_eq!(archive.offsets[2], archive.offsets[4]);
    assert_ne!(archive.offsets[4], archive.offsets[5]);
}

#[test]
fn zero_size_frames_are_legal() {
    let spec = FrmSpec {
        fps: 1,
        action_frame: 0,
        directions: vec![FrmDirection {
            shift: [0, 0],
            frames: vec![
                frame(0, 0, [0, 0], 0),
                frame(0, 5, [2, 0], 0),
                frame(5, 0, [0, 2], 0),
                frame(1, 1, [0, 0], 3),
            ],
        }],
    };
    round_trip(&spec);
}

#[test]
fn large_and_maximal_frames() {
    let big = FrmSpec {
        fps: i16::MAX as u16,
        action_frame: i16::MAX as u16,
        directions: vec![FrmDirection {
            shift: [i16::MIN, i16::MAX],
            frames: vec![frame(1000, 800, [i16::MIN, i16::MAX], 5)],
        }],
    };
    round_trip(&big);
    let wide = FrmSpec {
        fps: 10,
        action_frame: 0,
        directions: vec![FrmDirection {
            shift: [0, 0],
            frames: vec![
                frame(i16::MAX as u16, 1, [0, 0], 2),
                frame(1, i16::MAX as u16, [0, 0], 3),
            ],
        }],
    };
    round_trip(&wide);
}

#[test]
fn the_writer_refuses_what_the_format_cannot_carry() {
    let ok = |frames: Vec<FrmFrameSpec>| FrmDirection {
        shift: [0, 0],
        frames,
    };
    let spec = |directions: Vec<FrmDirection>| FrmSpec {
        fps: 10,
        action_frame: 0,
        directions,
    };
    let cases = [
        spec(vec![]),
        spec(vec![ok(vec![frame(1, 1, [0, 0], 0)]); 2]),
        spec(vec![ok(vec![])]),
        spec(
            (0..6)
                .map(|d| ok(vec![frame(1, 1, [0, 0], 0); 1 + usize::from(d == 3)]))
                .collect(),
        ),
        spec(vec![ok(vec![FrmFrameSpec {
            width: 2,
            height: 2,
            displacement: [0, 0],
            pixels: vec![1; 3],
        }])]),
        spec(vec![ok(vec![FrmFrameSpec {
            width: 0x8000,
            height: 0,
            displacement: [0, 0],
            pixels: vec![],
        }])]),
        FrmSpec {
            fps: 0x8000,
            ..spec(vec![ok(vec![frame(1, 1, [0, 0], 0)])])
        },
        FrmSpec {
            action_frame: 0xffff,
            ..spec(vec![ok(vec![frame(1, 1, [0, 0], 0)])])
        },
    ];
    for (index, case) in cases.iter().enumerate() {
        assert!(
            matches!(write_frm(case), Err(EmitError::Frm(_))),
            "case {index} should be refused"
        );
    }
}

/// The engine draws pixel (0, 0) of a frame placed with `sprite_shift`
/// exactly at anchor + origin, for odd and even sizes and any origin.
#[test]
fn sprite_shift_matches_world_rect() {
    let grid = HexGrid::default();
    let center = grid.hex_center(20100).expect("in range");
    let anchor = [center[0] as i32, center[1] as i32];
    for (width, height) in [(1, 1), (2, 2), (7, 4), (8, 9), (63, 120), (64, 1), (0, 0)] {
        for origin in [[0, 0], [-32, -100], [-3, -1], [5, 7], [-100, 40]] {
            let shift = sprite_shift(origin, width, height).expect("fits");
            let spec = FrmSpec {
                fps: 0,
                action_frame: 0,
                directions: vec![FrmDirection {
                    shift,
                    frames: vec![frame(width as u16, height as u16, [0, 0], 0)],
                }],
            };
            let archive = FrmArchive::parse(&write_frm(&spec).unwrap()).unwrap();
            for d in 0..6 {
                let rect =
                    archive
                        .frame(d, 0)
                        .unwrap()
                        .world_rect(anchor, archive.shifts[d], [0, 0]);
                assert_eq!(
                    rect.left,
                    i64::from(anchor[0] + origin[0]),
                    "{width}x{height} {origin:?}"
                );
                assert_eq!(
                    rect.top,
                    i64::from(anchor[1] + origin[1]),
                    "{width}x{height} {origin:?}"
                );
                assert_eq!(rect.right - rect.left + 1, i64::from(width));
                assert_eq!(rect.bottom - rect.top + 1, i64::from(height));
            }
        }
    }
    assert!(sprite_shift([i16::MAX as i32, 0], 2, 1).is_err());
    assert!(sprite_shift([0, i16::MIN as i32], 1, 0).is_err());
    assert_eq!(sprite_shift([-10, -20], 5, 4).unwrap(), [-8, -17]);
}

#[test]
fn crop_removes_transparent_margins() {
    // 6 × 5 with opaque pixels in columns 1..=3 and rows 2..=3.
    let mut image = vec![0u8; 30];
    for (x, y, value) in [(1, 2, 9), (3, 3, 8), (2, 2, 7)] {
        image[y * 6 + x] = value;
    }
    let (cropped, offset) = crop_to_content(&image, 6, 5).unwrap();
    assert_eq!(offset, [1, 2]);
    assert_eq!((cropped.width, cropped.height), (3, 2));
    assert_eq!(cropped.pixels, vec![9, 7, 0, 0, 0, 8]);
    let (empty, offset) = crop_to_content(&[0; 12], 4, 3).unwrap();
    assert_eq!((empty.width, empty.height, offset), (0, 0, [0, 0]));
    let (whole, offset) = crop_to_content(&[1; 4], 2, 2).unwrap();
    assert_eq!((whole.width, whole.height, offset), (2, 2, [0, 0]));
    assert!(matches!(
        crop_to_content(&[0; 5], 2, 2),
        Err(EmitError::Image(_))
    ));
}

/// A cropped, placed frame covers exactly the opaque pixels of the
/// original render at their original screen positions.
#[test]
fn placed_frame_keeps_the_render_position() {
    let (width, height) = (9u32, 7u32);
    let mut image = vec![0u8; 63];
    for (x, y) in [(2, 1), (6, 5), (4, 3)] {
        image[y * 9 + x] = 5;
    }
    let origin = [-40, -60];
    let placed = placed_frame(&image, width, height, origin).unwrap();
    let anchor = [300, 200];
    let spec = FrmSpec {
        fps: 0,
        action_frame: 0,
        directions: vec![FrmDirection {
            shift: placed.shift,
            frames: vec![placed.frame.clone()],
        }],
    };
    let archive = FrmArchive::parse(&write_frm(&spec).unwrap()).unwrap();
    let frame = archive.frame(0, 0).unwrap();
    let rect = frame.world_rect(anchor, archive.shifts[0], [0, 0]);
    assert_eq!(rect.left, i64::from(anchor[0] + origin[0] + 2));
    assert_eq!(rect.top, i64::from(anchor[1] + origin[1] + 1));
    assert_eq!(rect.right, i64::from(anchor[0] + origin[0] + 6));
    assert_eq!(rect.bottom, i64::from(anchor[1] + origin[1] + 5));
    let _ = (width, height);
}
