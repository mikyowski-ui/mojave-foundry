//! The walk-through GIF encoder, checked by decoding its output with a
//! minimal GIF89a reader and LZW decoder written here.

use mojave_convert::sequence::gif::{self, TRANSPARENT};
use mojave_convert::shots::Frame;

/// A GIF's variable-length LZW decoded (least significant bit first).
fn unlzw(data: &[u8], min: u8) -> Result<Vec<u8>, String> {
    let clear = 1usize << min;
    let end = clear + 1;
    let mut size = u32::from(min) + 1;
    let fresh = || -> Vec<Vec<u8>> {
        let mut t: Vec<Vec<u8>> = (0..clear).map(|i| vec![i as u8]).collect();
        t.push(Vec::new());
        t.push(Vec::new());
        t
    };
    let mut table = fresh();
    let mut prev: Option<Vec<u8>> = None;
    let mut out = Vec::new();
    let mut bit = 0usize;
    let mut saw_end = false;
    while bit + size as usize <= data.len() * 8 {
        let mut code = 0usize;
        for i in 0..size as usize {
            let b = bit + i;
            if data[b / 8] >> (b % 8) & 1 == 1 {
                code |= 1 << i;
            }
        }
        bit += size as usize;
        if code == clear {
            table = fresh();
            size = u32::from(min) + 1;
            prev = None;
            continue;
        }
        if code == end {
            saw_end = true;
            break;
        }
        let entry = if code < table.len() {
            table[code].clone()
        } else if code == table.len() {
            let p = prev.as_ref().ok_or("a new code with no string before it")?;
            let mut e = p.clone();
            e.push(p[0]);
            e
        } else {
            return Err(format!("code {code} past the table ({})", table.len()));
        };
        if let Some(p) = &prev {
            let mut e = p.clone();
            e.push(entry[0]);
            table.push(e);
        }
        out.extend_from_slice(&entry);
        prev = Some(entry);
        if table.len() == 1 << size && size < 12 {
            size += 1;
        }
    }
    if !saw_end {
        return Err("no end code".to_owned());
    }
    Ok(out)
}

struct Decoded {
    width: usize,
    height: usize,
    loops: Option<u16>,
    /// Each composited frame (RGB) and its delay.
    frames: Vec<(Vec<u8>, u16)>,
    rects: Vec<[usize; 4]>,
}

fn u16_at(d: &[u8], at: usize) -> usize {
    usize::from(d[at]) | usize::from(d[at + 1]) << 8
}

fn sub_blocks(d: &[u8], mut at: usize) -> (Vec<u8>, usize) {
    let mut data = Vec::new();
    while d[at] != 0 {
        let n = usize::from(d[at]);
        data.extend_from_slice(&d[at + 1..at + 1 + n]);
        at += 1 + n;
    }
    (data, at + 1)
}

fn decode(d: &[u8]) -> Decoded {
    assert_eq!(&d[..6], b"GIF89a");
    let (width, height) = (u16_at(d, 6), u16_at(d, 8));
    let flags = d[10];
    assert!(flags & 0x80 != 0, "a global table");
    let table_len = 3 * (2usize << (flags & 7));
    let palette = &d[13..13 + table_len];
    let mut at = 13 + table_len;
    let mut canvas = vec![0u8; width * height * 3];
    let mut out = Decoded {
        width,
        height,
        loops: None,
        frames: Vec::new(),
        rects: Vec::new(),
    };
    let (mut transparent, mut delay) = (None, 0u16);
    loop {
        match d[at] {
            0x3B => break,
            0x21 => {
                let label = d[at + 1];
                if label == 0xF9 {
                    assert_eq!(d[at + 2], 4);
                    let packed = d[at + 3];
                    assert_eq!(packed >> 2 & 7, 1, "disposal: leave in place");
                    delay = u16_at(d, at + 4) as u16;
                    transparent = (packed & 1 == 1).then_some(d[at + 6]);
                } else if label == 0xFF {
                    assert_eq!(&d[at + 3..at + 14], b"NETSCAPE2.0");
                    out.loops = Some(u16_at(d, at + 16) as u16);
                }
                let (_, next) = sub_blocks(d, at + 2);
                at = next;
            }
            0x2C => {
                let [x, y, w, h] = [1, 3, 5, 7].map(|o| u16_at(d, at + o));
                assert_eq!(d[at + 9], 0, "no local table, not interlaced");
                let min = d[at + 10];
                let (data, next) = sub_blocks(d, at + 11);
                at = next;
                let indices = unlzw(&data, min).expect("valid LZW");
                assert_eq!(indices.len(), w * h, "one index per pixel");
                for (i, &k) in indices.iter().enumerate() {
                    if Some(k) == transparent {
                        continue;
                    }
                    let o = ((y + i / w) * width + x + i % w) * 3;
                    canvas[o..o + 3].copy_from_slice(&palette[3 * k as usize..3 * k as usize + 3]);
                }
                out.frames.push((canvas.clone(), delay));
                out.rects.push([x, y, w, h]);
            }
            other => panic!("unexpected block {other:#x} at {at}"),
        }
    }
    out
}

fn frame(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Frame {
    let mut rgb = Vec::new();
    for y in 0..height {
        for x in 0..width {
            rgb.extend_from_slice(&f(x, y));
        }
    }
    Frame { width, height, rgb }
}

/// A small deterministic noise source.
fn noise(seed: u32) -> impl FnMut() -> u32 {
    let mut s = seed.wrapping_mul(2_654_435_761).max(1);
    move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    }
}

#[test]
fn lzw_round_trips_runs_noise_and_full_tables() {
    let mut next = noise(7);
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        vec![5],
        vec![0; 10_000],
        // The KwKwK case: a code used the moment it is made.
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab".to_vec(),
        (0..=255u8).cycle().take(5_000).collect(),
        // Noise fills the 4,096-entry table many times over.
        (0..200_000).map(|_| (next() % 256) as u8).collect(),
        (0..100_000).map(|_| (next() % 3) as u8).collect(),
    ];
    for case in cases {
        let packed = gif::lzw(&case);
        assert_eq!(
            unlzw(&packed, 8).expect("decodes"),
            case,
            "{} bytes",
            case.len()
        );
    }
}

#[test]
fn an_animation_decodes_back_to_its_frames() {
    let (w, h) = (61u32, 37u32);
    // A still room with a figure walking across it, and a clock in the
    // corner that changes every frame.
    let frames: Vec<Frame> = (0..12u32)
        .map(|t| {
            frame(w, h, |x, y| {
                if (3 + 4 * t..9 + 4 * t).contains(&x) && (10..24).contains(&y) {
                    [200, 30, 30]
                } else if x < 3 && y < 3 {
                    [(t * 20) as u8, 0, 255]
                } else {
                    [((x * 7 + y * 3) % 20) as u8, 60, ((y % 5) * 10) as u8]
                }
            })
        })
        .collect();
    let refs: Vec<&Frame> = frames.iter().collect();
    let delays: Vec<u16> = (0..12).map(|i| 6 + i as u16).collect();
    let (bytes, stats) = gif::encode(&refs, &delays, 0).expect("encodes");
    assert_eq!(stats.bytes, bytes.len());
    assert_eq!(stats.frames_in, 12);
    assert_eq!(stats.frames_written, 12);
    assert_eq!(stats.merged, 0, "{} colours", stats.colours);
    let decoded = decode(&bytes);
    assert_eq!((decoded.width, decoded.height), (61, 37));
    assert_eq!(decoded.loops, Some(0));
    assert_eq!(decoded.frames.len(), 12);
    for (i, ((rgb, delay), want)) in decoded.frames.iter().zip(&frames).enumerate() {
        assert_eq!(rgb, &want.rgb, "frame {i}");
        assert_eq!(*delay, delays[i]);
    }
    // After the first frame only the changed rectangle is written: the
    // figure's old and new place and the clock.
    assert_eq!(decoded.rects[0], [0, 0, 61, 37]);
    for rect in &decoded.rects[1..] {
        assert_eq!(rect[1], 0, "the clock is in the top row");
        assert_eq!(rect[3], 24, "down to the figure's feet");
        assert!(rect[2] < 61, "narrower than the frame: {rect:?}");
    }
    assert!(stats.rect_pixels < 12 * 61 * 37 / 2);
}

#[test]
fn identical_frames_are_merged_and_their_time_kept() {
    let a = frame(8, 4, |x, _| [x as u8 * 30, 0, 0]);
    let b = frame(8, 4, |x, y| {
        if (x, y) == (2, 1) {
            [0, 255, 0]
        } else {
            [x as u8 * 30, 0, 0]
        }
    });
    let refs = [&a, &a, &a, &b, &b, &a];
    let (bytes, stats) = gif::encode(&refs, &[10, 10, 10, 5, 5, 200], 3).expect("encodes");
    assert_eq!(stats.frames_written, 3);
    let decoded = decode(&bytes);
    assert_eq!(decoded.loops, Some(3));
    let delays: Vec<u16> = decoded.frames.iter().map(|f| f.1).collect();
    assert_eq!(delays, [30, 10, 200]);
    assert_eq!(decoded.frames[1].0, b.rgb);
    assert_eq!(decoded.frames[2].0, a.rgb);
    // A one-pixel change is a one-pixel frame.
    assert_eq!(decoded.rects[1], [2, 1, 1, 1]);
    assert_eq!(decoded.rects[2], [2, 1, 1, 1]);
}

#[test]
fn more_than_255_colours_map_to_the_nearest_kept_one() {
    // 255 greys, each twice, and 45 rare colours one step from a grey.
    let f = frame(300, 2, |x, y| match (x, y) {
        (0..255, _) => [x as u8; 3],
        (_, 0) => {
            let g = (x - 255 + 100) as u8;
            [g, g, g + 1]
        }
        _ => [0, 0, 0],
    });
    let (bytes, stats) = gif::encode(&[&f], &[1], 0).expect("encodes");
    assert!(stats.colours > 255);
    assert_eq!(stats.merged, stats.colours - 255);
    let decoded = decode(&bytes);
    let rgb = &decoded.frames[0].0;
    let worst = rgb
        .iter()
        .zip(&f.rgb)
        .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
        .max()
        .unwrap();
    assert_eq!(worst, 1);
    // Index 255 is never a colour: it is left for transparency.
    assert_eq!(TRANSPARENT, 255);
}

#[test]
fn bad_input_is_an_error() {
    let a = frame(4, 4, |_, _| [1, 2, 3]);
    let b = frame(5, 4, |_, _| [1, 2, 3]);
    assert!(gif::encode(&[], &[], 0).is_err());
    assert!(gif::encode(&[&a, &b], &[1, 1], 0).is_err());
    assert!(gif::encode(&[&a], &[1, 1], 0).is_err());
    assert_eq!(gif::changed_rect(&[1, 2, 3, 4], &[1, 2, 3, 4], 2), None);
    assert_eq!(
        gif::changed_rect(&[1, 2, 3, 4, 5, 6], &[1, 9, 3, 4, 5, 0], 2),
        Some([1, 0, 2, 3])
    );
}
