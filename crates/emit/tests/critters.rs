//! Critters (contract section 10): animations, the critter art list, the
//! prototype, the placed record and a layer holding all of them, every one
//! read back through the pinned `vault-formats`.

use emit::critter::travel_offsets;
use emit::map::empty_map;
use emit::proto::{
    self, Material, ObjectFlags, ProtoCommon, message_entries, message_file_path, stat,
};
use emit::{
    CritterFrames, CritterList, CritterPlacement, CritterSpec, EmitError, FormatWriters, FrmSpec,
    HEX_STEPS, LocalWriters, ModLayer, MsgFile, ProtoList, RenderedFrame, critter_fid, critter_frm,
    critter_object, critter_prototype, still_frm,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use vault_formats::map::{MapFile, ObjectState};
use vault_formats::pro::{BasicFlags, Category, PrototypeData, PrototypeKind, PrototypeLookup};
use vault_formats::vfs::{ByteSource, LocalDirProvider, MemoryDir, MemoryMount};
use vault_formats::{
    ArtCatalog, Fid, FrmArchive, GameProfile, HexGrid, LayeredVfs, LocalPrototypeCatalog, LstKind,
    LstTable, MsgCatalog, Prototype, Vfs,
};

// ---------------------------------------------------------------------------
// Synthetic frames

/// A `width × height` image with a one-pixel transparent margin and a
/// pattern that encodes the seed, rendered with its bottom centre on the
/// ground point, raised by `lift`.
fn rendered(width: u32, height: u32, lift: i32, seed: u8, travel: [f32; 2]) -> RenderedFrame {
    let mut pixels = vec![0_u8; (width * height) as usize];
    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            pixels[(y * width + x) as usize] = 1 + ((x * 7 + y * 3 + u32::from(seed)) % 200) as u8;
        }
    }
    RenderedFrame {
        origin: [-(width as i32) / 2, -(height as i32) + 1 - lift],
        width,
        height,
        pixels,
        travel,
    }
}

/// `count` frames per direction of different sizes and heights, each
/// travelling `per_frame` hex steps along its facing (with a little
/// perpendicular sway that must be ignored).
fn animation(count: usize, per_frame: impl Fn(usize) -> f32) -> CritterFrames {
    let directions = std::array::from_fn(|d| {
        let step = HEX_STEPS[d];
        (0..count)
            .map(|f| {
                let along = per_frame(f);
                let sway = if f % 2 == 0 { 0.3 } else { -0.3 };
                // The perpendicular of (x, y) is (-y, x), normalized.
                let length = ((step[0] * step[0] + step[1] * step[1]) as f32).sqrt();
                let travel = [
                    step[0] as f32 * along - step[1] as f32 / length * sway,
                    step[1] as f32 * along + step[0] as f32 / length * sway,
                ];
                rendered(
                    9 + (f as u32 % 3) * 2 + d as u32,
                    30 + (f as u32 % 4),
                    (f % 2) as i32,
                    (d * 16 + f) as u8,
                    travel,
                )
            })
            .collect()
    });
    CritterFrames { fps: 8, directions }
}

/// Every opaque pixel the engine draws for direction `d`, frame `f`, in
/// screen coordinates relative to the ground point, ignoring the
/// accumulated displacement (the walk is rendered in place).
fn drawn(archive: &FrmArchive, d: usize, f: usize) -> BTreeMap<(i64, i64), u8> {
    let frame = archive.frame(d, f).expect("frame present");
    let rect = frame.world_rect([0, 0], archive.shifts[d], [0, 0]);
    let mut out = BTreeMap::new();
    for y in 0..i64::from(frame.height) {
        for x in 0..i64::from(frame.width) {
            let pixel = frame.pixels()[(y * i64::from(frame.width) + x) as usize];
            if pixel != 0 {
                out.insert((rect.left + x, rect.top + y), pixel);
            }
        }
    }
    out
}

/// Every opaque pixel of a rendered frame, relative to the ground point.
fn source(frame: &RenderedFrame) -> BTreeMap<(i64, i64), u8> {
    let mut out = BTreeMap::new();
    for y in 0..frame.height {
        for x in 0..frame.width {
            let pixel = frame.pixels[(y * frame.width + x) as usize];
            if pixel != 0 {
                out.insert(
                    (
                        i64::from(frame.origin[0]) + i64::from(x),
                        i64::from(frame.origin[1]) + i64::from(y),
                    ),
                    pixel,
                );
            }
        }
    }
    out
}

fn parse(spec: &FrmSpec) -> FrmArchive {
    FrmArchive::parse(&LocalWriters.frm(spec).expect("writes")).expect("parses")
}

// ---------------------------------------------------------------------------
// The walk rule, modelled as SPEC-007 §3.11.1 states it

/// Whether `displacement` has reached `step` on either axis.
fn crosses(displacement: [i64; 2], step: [i32; 2]) -> bool {
    (0..2).any(|axis| match step[axis].signum() {
        1 => displacement[axis] >= i64::from(step[axis]),
        -1 => displacement[axis] <= i64::from(step[axis]),
        _ => false,
    })
}

struct Walked {
    hexes: u64,
    /// The largest distance of the displacement from the facing's axis.
    worst_off_axis: f64,
    /// The most frame steps between two crossings.
    longest_gap: u64,
}

/// Walk `cycles` loops of a direction's offsets in a straight line: add
/// each frame's offset, move to the next hex when the displacement reaches
/// the step on either axis, carry the remainder.
fn walk(direction: usize, offsets: &[[i16; 2]], cycles: u64) -> Walked {
    let step = HEX_STEPS[direction];
    let length = f64::from(step[0] * step[0] + step[1] * step[1]).sqrt();
    let mut displacement = [0_i64; 2];
    let mut walked = Walked {
        hexes: 0,
        worst_off_axis: 0.0,
        longest_gap: 0,
    };
    let mut since = 0;
    for _ in 0..cycles {
        for offset in offsets {
            displacement[0] += i64::from(offset[0]);
            displacement[1] += i64::from(offset[1]);
            since += 1;
            if crosses(displacement, step) {
                displacement[0] -= i64::from(step[0]);
                displacement[1] -= i64::from(step[1]);
                walked.hexes += 1;
                walked.longest_gap = walked.longest_gap.max(since);
                since = 0;
            }
            let off_axis = (displacement[0] as f64 * f64::from(step[1])
                - displacement[1] as f64 * f64::from(step[0]))
            .abs()
                / length;
            walked.worst_off_axis = walked.worst_off_axis.max(off_axis);
        }
    }
    walked
}

// ---------------------------------------------------------------------------
// Animations

#[test]
fn hex_steps_are_the_engines_neighbour_steps() {
    let grid = HexGrid::default();
    for hex in [20100, 20101, 12345, 12346] {
        let center = grid.hex_center(hex).unwrap();
        for (d, step) in HEX_STEPS.iter().enumerate() {
            let next = HexGrid::neighbor(hex, d as u8).unwrap().unwrap();
            let other = grid.hex_center(next).unwrap();
            assert_eq!(
                [other[0] - center[0], other[1] - center[1]],
                [i64::from(step[0]), i64::from(step[1])],
                "hex {hex} direction {d}"
            );
        }
    }
}

#[test]
fn a_critter_frm_parses_back_with_six_directions_placed_as_rendered() {
    let frames = animation(8, |f| 0.11 + 0.03 * (f % 3) as f32);
    let spec = critter_frm(&frames, 3).unwrap();
    let archive = parse(&spec);
    assert_eq!(archive.frame_count(), 8);
    assert_eq!(archive.stored_sequences(), 6);
    assert_eq!((archive.fps, archive.action_frame), (8, 3));
    for d in 0..6 {
        let travel: Vec<[f32; 2]> = frames.directions[d].iter().map(|f| f.travel).collect();
        let offsets = travel_offsets(d, &travel).unwrap();
        let first = archive.frame(d, 0).unwrap();
        for (f, rendered) in frames.directions[d].iter().enumerate() {
            let frame = archive.frame(d, f).unwrap();
            assert_eq!(frame.displacement, offsets[f], "d{d} f{f}");
            // One size per direction: the union of the cropped frames.
            assert_eq!((frame.width, frame.height), (first.width, first.height));
            assert_eq!(drawn(&archive, d, f), source(rendered), "d{d} f{f}");
        }
        // The cycle's offsets sum to a whole multiple of the step's unit.
        let sum = archive.accumulated_displacement(d, 7).unwrap();
        let step = HEX_STEPS[d];
        assert_eq!(sum[0] * step[1], sum[1] * step[0], "d{d} sum {sum:?}");
        assert!(sum[0].signum() == step[0].signum() && sum[1].signum() == step[1].signum());
    }
}

#[test]
fn travel_is_spread_into_whole_pixels_with_the_remainder_carried() {
    // An eighth of a step per frame: north-east (2, -1.5) per frame.
    let offsets = travel_offsets(0, &[[2.0, -1.5]; 8]).unwrap();
    assert_eq!(
        offsets,
        [
            [2, -2],
            [2, -1],
            [2, -2],
            [2, -1],
            [2, -2],
            [2, -1],
            [2, -2],
            [2, -1]
        ]
    );
    // East: no vertical offset ever, whatever the vertical travel.
    let offsets = travel_offsets(1, &[[4.2, 3.0], [3.9, -3.0], [4.0, 0.5]]).unwrap();
    assert!(offsets.iter().all(|offset| offset[1] == 0));
    assert_eq!(offsets.iter().map(|o| i32::from(o[0])).sum::<i32>(), 12);
    // A stand travels nothing.
    assert_eq!(travel_offsets(2, &[[0.0, 0.0]; 5]).unwrap(), [[0, 0]; 5]);
    // A positive total below one unit still finishes a walk.
    let offsets = travel_offsets(3, &[[-0.1, 0.075]; 4]).unwrap();
    let sum = offsets
        .iter()
        .fold([0, 0], |s, o| [s[0] + o[0], s[1] + o[1]]);
    assert_eq!(sum, [-4, 3]);
    // Backward and non-finite travel are refused.
    assert!(travel_offsets(4, &[[3.0, 0.0]]).is_err());
    assert!(travel_offsets(0, &[[f32::NAN, 0.0]]).is_err());
    assert!(travel_offsets(6, &[[0.0, 0.0]]).is_err());
}

/// Travel in hex steps for each frame number.
type PerFrame = Box<dyn Fn(usize) -> f32>;

#[test]
fn every_direction_walks_hex_after_hex_without_drift() {
    // Travel per frame in hex steps: whole-pixel, fractional, uneven,
    // and slow cycles that need several loops per hex.
    let cases: [(&str, usize, PerFrame); 6] = [
        ("an eighth", 8, Box::new(|_| 0.125)),
        ("fractional", 8, Box::new(|_| 0.1371)),
        (
            "uneven",
            10,
            Box::new(|f| 0.05 + 0.04 * ((f * 7) % 5) as f32),
        ),
        ("fast", 6, Box::new(|f| 0.31 + 0.017 * f as f32)),
        ("slow", 12, Box::new(|_| 0.0093)),
        ("one third", 3, Box::new(|_| 1.0 / 3.0)),
    ];
    let cycles = 400;
    for (name, count, per_frame) in &cases {
        let frames = animation(*count, per_frame);
        let archive = parse(&critter_frm(&frames, 0).unwrap());
        for d in 0..6 {
            let offsets: Vec<[i16; 2]> = (0..*count)
                .map(|f| archive.frame(d, f).unwrap().displacement)
                .collect();
            let walked = walk(d, &offsets, cycles);
            // The cycle's exact travel along the step, in hex steps.
            let intended: f64 = (0..*count).map(|f| f64::from(per_frame(f))).sum();
            let units = if d == 1 || d == 4 { 32.0 } else { 4.0 };
            let quantized = (intended * units).round().max(1.0) / units;
            let expected = quantized * cycles as f64;
            assert!(
                (walked.hexes as f64 - expected).abs() <= 1.0,
                "{name} d{d}: {} hexes, expected {expected}",
                walked.hexes
            );
            assert!(
                walked.worst_off_axis <= 1.0,
                "{name} d{d}: drifted {} px off the axis",
                walked.worst_off_axis
            );
            // No stall: a hex is finished within its share of a cycle plus one cycle.
            let frames_per_hex = (*count as f64 / quantized).ceil() as u64 + *count as u64;
            assert!(
                walked.longest_gap <= frames_per_hex,
                "{name} d{d}: {} frames between hexes",
                walked.longest_gap
            );
        }
    }
}

#[test]
fn a_stand_carries_zero_offsets_and_a_still_holds_frame_zero() {
    let stand = critter_frm(&animation(4, |_| 0.0), 0).unwrap();
    let archive = parse(&stand);
    for d in 0..6 {
        for f in 0..4 {
            assert_eq!(archive.frame(d, f).unwrap().displacement, [0, 0]);
        }
    }
    let walk = critter_frm(&animation(6, |_| 0.2), 5).unwrap();
    let still = still_frm(&walk).unwrap();
    let parsed = parse(&still);
    assert_eq!(parsed.frame_count(), 1);
    assert_eq!((parsed.action_frame, parsed.fps), (0, 8));
    for d in 0..6 {
        let frame = parsed.frame(d, 0).unwrap();
        assert_eq!(frame.displacement, [0, 0]);
        assert_eq!(parsed.shifts[d], walk.directions[d].shift);
        assert_eq!(
            frame.pixels(),
            walk.directions[d].frames[0].pixels.as_slice()
        );
    }
    assert!(
        still_frm(&FrmSpec {
            fps: 0,
            action_frame: 0,
            directions: vec![],
        })
        .is_err()
    );
}

#[test]
fn transparent_directions_and_bad_input() {
    let mut frames = animation(2, |_| 0.25);
    for frame in &mut frames.directions[2] {
        frame.pixels.fill(0);
    }
    let archive = parse(&critter_frm(&frames, 1).unwrap());
    let frame = archive.frame(2, 1).unwrap();
    assert_eq!(
        (frame.width, frame.height, archive.shifts[2]),
        (0, 0, [0, 0])
    );
    assert_ne!(archive.frame(2, 0).unwrap().displacement, [0, 0]);

    let refused = |frames: &CritterFrames, action: u16| {
        assert!(
            matches!(
                critter_frm(frames, action),
                Err(EmitError::Frm(_) | EmitError::Image(_))
            ),
            "should be refused"
        );
    };
    let good = animation(3, |_| 0.1);
    refused(&good, 3);
    let mut uneven = good.clone();
    uneven.directions[4].pop();
    refused(&uneven, 0);
    let mut empty = good.clone();
    empty.directions = Default::default();
    refused(&empty, 0);
    let mut short = good.clone();
    short.directions[1][0].pixels.pop();
    refused(&short, 0);
    let mut backward = good.clone();
    for frame in &mut backward.directions[5] {
        frame.travel = [16.0, 12.0];
    }
    refused(&backward, 0);
    let mut fast = good;
    fast.fps = 0x8000;
    refused(&fast, 0);
}

// ---------------------------------------------------------------------------
// The critter art list

#[test]
fn the_critter_list_keeps_the_base_and_resolves_new_rows() {
    // No final LF, a CRLF line, a blank line and an aliased row.
    let base = b"base_a,0\r\nbase_b,0\n\nothr,1";
    let mut list = CritterList::from_base(base).unwrap();
    assert_eq!(
        (list.len(), list.list_path()),
        (4, "art/critters/critters.lst")
    );
    let row = list.append("mj0001").unwrap();
    let second = list.append("MJ_02").unwrap();
    assert_eq!((row, second), (4, 5));
    let bytes = list.to_bytes();
    assert!(bytes.starts_with(base));
    assert!(bytes.ends_with(b"\nmj0001,4\nMJ_02,5\n"));
    let table = LstTable::parse(&bytes, LstKind::Art).unwrap();
    assert_eq!(table.entries().len(), 6);
    assert!(table.diagnostics().is_empty());
    assert_eq!(table.art_id(4).unwrap().alias, Some(4));
    assert_eq!(table.art_id(3).unwrap().name.as_deref(), Some(&b"othr"[..]));

    let mut catalog = ArtCatalog::default();
    catalog.insert(1, table);
    for (animation, suffix) in [(0, "aa"), (1, "ab"), (14, "ao"), (20, "ba"), (48, "ra")] {
        let art = list.art(row, animation, 0).unwrap();
        assert_eq!(art.fid, Fid::new(1, row, animation, 0, 0));
        assert_eq!(art.file_name, format!("mj0001{suffix}.frm"));
        assert_eq!(art.path, format!("art/critters/mj0001{suffix}.frm"));
        assert_eq!(catalog.path(art.fid).unwrap(), art.path);
        assert_eq!(catalog.critter_alias(row), Some(i32::from(row)));
    }
    // An aliased animation stays on the new row's own art ...
    assert_eq!(list.art(row, 27, 0).unwrap().file_name, "mj0001bh.frm");
    // ... and follows the base row's alias where it points elsewhere.
    let aliased = list.art(3, 27, 0).unwrap();
    assert_eq!(aliased.file_name, "base_bbh.frm");
    assert_eq!(catalog.path(aliased.fid).unwrap(), aliased.path);
    // Armed stand: weapon code 1 is the letter d.
    assert_eq!(list.art(second, 0, 1).unwrap().file_name, "MJ_02da.frm");
    assert_eq!(list.find("mj_02"), Some(5));
}

#[test]
fn the_critter_list_refuses_bad_stems_duplicates_and_row_4096() {
    let mut list = CritterList::from_base(b"hmbase,0\n").unwrap();
    for stem in ["", "toolong12", "a-b", "a b", "a,b", "a.frm", "é"] {
        assert!(
            matches!(list.append(stem), Err(EmitError::Critter(_))),
            "{stem:?}"
        );
    }
    assert!(matches!(list.append("HMBASE"), Err(EmitError::Critter(_))));
    list.append("ok_1").unwrap();
    assert!(matches!(list.append("OK_1"), Err(EmitError::Critter(_))));
    assert_eq!(list.len(), 2);
    assert!(list.art(2, 0, 0).is_err(), "no such row");

    let base: Vec<u8> = (0..4095)
        .flat_map(|i| format!("c{i:05},0\n").into_bytes())
        .collect();
    let mut full = CritterList::from_base(&base).unwrap();
    assert_eq!(full.remaining(), 1);
    assert_eq!(full.append("last").unwrap(), 4095);
    assert!(matches!(
        full.append("over"),
        Err(EmitError::FidBudget {
            category: "critters",
            cap: 4096,
            requested: 4097
        })
    ));
    assert!(critter_fid(4096, 0, 0).is_err());
    assert!(critter_fid(1, 0, 11).is_err());
    assert!(critter_fid(1, 65, 0).is_err());
    assert!(
        critter_fid(1, 38, 0).is_err(),
        "a weapon animation needs a weapon"
    );
}

// ---------------------------------------------------------------------------
// Prototype, map record, layer

const PACKET: i32 = 7;
const TEAM: i32 = 3;

fn critter_common(pid: u32, row: u16) -> ProtoCommon {
    ProtoCommon {
        pid,
        fid: critter_fid(row, 0, 0).unwrap(),
        message_base: proto::default_message_base(pid & 0xff_ffff).unwrap(),
        flags: ObjectFlags::SOLID,
        material: Material::default(),
    }
}

#[test]
fn the_prototype_is_a_416_byte_fallout_2_critter() {
    let pid = proto::pid(Category::Critter, 3).unwrap();
    let prototype = critter_prototype(
        critter_common(pid, 4),
        &CritterSpec::human(30, PACKET, TEAM),
    )
    .unwrap();
    let bytes = LocalWriters.pro(&prototype).unwrap();
    assert_eq!(bytes.len(), 416);
    assert_eq!(&bytes[..4], &pid.to_be_bytes());
    let parsed = Prototype::parse_for(&bytes, GameProfile::Fallout2).unwrap();
    assert_eq!(parsed, prototype);
    assert!(parsed.diagnostics.is_empty());
    assert_eq!(parsed.script, Some(-1));
    assert!(parsed.permits_talk());
    let PrototypeData::Critter(critter) = &parsed.data else {
        panic!("a critter")
    };
    assert_eq!(&critter.base_stats[..7], &[5; 7]);
    assert_eq!(critter.base_stats[stat::MAX_HIT_POINTS], 30);
    assert_eq!(critter.base_stats[stat::MAX_ACTION_POINTS], 7);
    assert_eq!(critter.base_stats[stat::CARRY_WEIGHT], 150);
    assert_eq!(critter.base_stats[stat::AGE], 25);
    assert_eq!(critter.base_stats[stat::GENDER], 0);
    assert_eq!((critter.ai, critter.team, critter.body), (PACKET, TEAM, 0));
    assert_eq!((critter.kill_category, critter.damage_type), (0, Some(0)));
    assert_eq!(critter.bonus_stats, [0; 35]);
    assert_eq!(critter.head_art, u32::MAX);
    assert!(!parsed.flags.contains(BasicFlags::NONBLOCKING));

    let mut woman = CritterSpec::human(25, 1, 1);
    woman.female = true;
    let PrototypeData::Critter(critter) = critter_prototype(critter_common(pid, 4), &woman)
        .unwrap()
        .data
    else {
        panic!("a critter")
    };
    assert_eq!((critter.kill_category, critter.base_stats[34]), (1, 1));

    // Refused figures and mismatched identifiers.
    for spec in [
        CritterSpec::human(0, 1, 1),
        CritterSpec::human(10, -1, 1),
        CritterSpec {
            special: [5, 5, 5, 5, 5, 5, 11],
            ..CritterSpec::human(10, 1, 1)
        },
        CritterSpec {
            age: 70,
            ..CritterSpec::human(10, 1, 1)
        },
        CritterSpec {
            body: 3,
            ..CritterSpec::human(10, 1, 1)
        },
    ] {
        assert!(critter_prototype(critter_common(pid, 4), &spec).is_err());
    }
    let mut scenery_art = critter_common(pid, 4);
    scenery_art.fid = Fid::new(2, 4, 0, 0, 0);
    assert!(critter_prototype(scenery_art, &CritterSpec::human(10, 1, 1)).is_err());
    let mut scenery_pid = critter_common(pid, 4);
    scenery_pid.pid = 0x0200_0003;
    assert!(critter_prototype(scenery_pid, &CritterSpec::human(10, 1, 1)).is_err());
}

/// The files a layer holding one critter ships, from synthetic base files.
struct Critter {
    files: Vec<(String, Vec<u8>)>,
    prototype: Prototype,
    stand: Fid,
    frames: BTreeMap<String, FrmSpec>,
    map: Vec<u8>,
}

const BASE_ART: &[u8] = b"hmbase,0\nhfbase,1\n";
const BASE_PROTO: &[u8] = b"00000001.pro\n00000002.pro\n";
const BASE_MSG: &[u8] = b"{100}{}{Base critter}\n{101}{}{A base description.}\n";

fn one_critter() -> Critter {
    let mut art = CritterList::from_base(BASE_ART).unwrap();
    let row = art.append("mj0001").unwrap();
    let mut protos = ProtoList::from_base(Category::Critter, BASE_PROTO).unwrap();
    let (pid, file) = protos.push_next().unwrap();
    assert_eq!((pid, file.as_str()), (0x0100_0003, "00000003.pro"));
    let prototype = critter_prototype(
        critter_common(pid, row),
        &CritterSpec::human(30, PACKET, TEAM),
    )
    .unwrap();
    let mut messages = MsgFile::from_base(BASE_MSG).unwrap();
    for entry in message_entries(prototype.message_base, b"Townsperson", b"A resident.").unwrap() {
        messages.push(&entry).unwrap();
    }

    let stand = critter_frm(&animation(4, |_| 0.0), 0).unwrap();
    let walk = critter_frm(&animation(8, |_| 0.125), 0).unwrap();
    let placeholder = still_frm(&stand).unwrap();
    let mut frames = BTreeMap::new();
    for (animation, spec) in [(0, &stand), (1, &walk)]
        .into_iter()
        .chain([14, 20, 21, 36, 37, 48, 49, 16, 17].map(|code| (code, &placeholder)))
    {
        frames.insert(art.art(row, animation, 0).unwrap().path, spec.clone());
    }

    let mut map = empty_map("TESTMAP.MAP", 200, 20100).unwrap();
    let placement = CritterPlacement::from_prototype(&prototype, 1, 20102, 0, 2).unwrap();
    map.objects.elevations[0] = vec![critter_object(&placement).unwrap()];
    map.objects.total = 1;
    let map = LocalWriters.map(&map).unwrap();

    let mut files = vec![
        (art.list_path().to_owned(), art.to_bytes()),
        (protos.list_path(), protos.to_bytes()),
        (
            proto::prototype_path(Category::Critter, &file),
            LocalWriters.pro(&prototype).unwrap(),
        ),
        (
            message_file_path(Category::Critter, "english"),
            messages.to_bytes(),
        ),
        ("maps/testmap.map".to_owned(), map.clone()),
    ];
    for (path, spec) in &frames {
        files.push((path.clone(), LocalWriters.frm(spec).unwrap()));
    }
    Critter {
        files,
        prototype,
        stand: art.art(row, 0, 0).unwrap().fid,
        frames,
        map,
    }
}

fn memory(files: Vec<(String, Vec<u8>)>) -> vault_formats::vfs::MemoryProvider {
    let tree = MemoryDir::new(
        files
            .into_iter()
            .map(|(path, bytes)| (path, ByteSource::new(bytes).unwrap())),
    )
    .unwrap();
    MemoryMount {
        profile: GameProfile::Fallout2,
        loose: Some(tree),
        ..MemoryMount::default()
    }
    .mount()
    .unwrap()
}

#[test]
fn the_prototype_resolves_at_its_list_position() {
    let critter = one_critter();
    let provider = memory(critter.files.clone());
    let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);
    let info = catalog.lookup(critter.prototype.pid).unwrap();
    assert_eq!(info.kind, PrototypeKind::Critter);
    // The base rows name files this corpus lacks.
    assert!(catalog.lookup(0x0100_0001).is_err());
}

#[test]
fn a_map_with_a_critter_reads_back_and_rewrites_the_same_bytes() {
    let critter = one_critter();
    let provider = memory(critter.files.clone());
    let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);
    let parsed = MapFile::parse_for(&critter.map, &catalog, GameProfile::Fallout2).unwrap();
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    assert_eq!(LocalWriters.map(&parsed).unwrap(), critter.map);
    let objects = &parsed.objects.elevations[0];
    assert_eq!(objects.len(), 1);
    let object = &objects[0];
    assert_eq!(
        (object.id, object.hex, object.orientation, object.frame),
        (1, Some(20102), 2, 0)
    );
    assert_eq!(
        (object.pid, object.art_id),
        (critter.prototype.pid, critter.stand.0)
    );
    assert_eq!((object.script_id, object.inventory.len()), (-1, 0));
    assert_eq!(object.flags, critter.prototype.flags);
    let ObjectState::Critter(state) = &object.state else {
        panic!("a critter record")
    };
    assert_eq!((state.ai, state.team, state.hp), (PACKET, TEAM, 30));
    assert_eq!((state.attacker, state.results.0, state.damage), (-1, 0, 0));
    // A critter record is 128 bytes: a second one adds exactly that.
    let mut two = parsed.clone();
    let mut other = object.clone();
    other.id = 2;
    other.hex = Some(20300);
    two.objects.elevations[0].push(other);
    two.objects.total = 2;
    assert_eq!(
        LocalWriters.map(&two).unwrap().len() - critter.map.len(),
        128
    );
}

#[test]
fn the_critter_record_refuses_what_the_engine_cannot_use() {
    let critter = one_critter();
    let good = CritterPlacement::from_prototype(&critter.prototype, 9, 20102, 0, 5).unwrap();
    assert!(critter_object(&good).is_ok());
    let cases = [
        CritterPlacement {
            hit_points: 0,
            ..good
        },
        CritterPlacement {
            orientation: 6,
            ..good
        },
        CritterPlacement {
            hex: 40_000,
            ..good
        },
        CritterPlacement {
            elevation: 3,
            ..good
        },
        CritterPlacement {
            fid: critter_fid(4, 1, 0).unwrap(),
            ..good
        },
        CritterPlacement {
            pid: 0x0100_0000,
            ..good
        },
        CritterPlacement {
            pid: 0x0200_0001,
            ..good
        },
        CritterPlacement {
            flags: BasicFlags(good.flags.0 | BasicFlags::SPECIAL.0),
            ..good
        },
    ];
    for (index, case) in cases.iter().enumerate() {
        assert!(
            matches!(critter_object(case), Err(EmitError::Critter(_))),
            "case {index}"
        );
    }
    let wall = proto::wall(ProtoCommon {
        pid: 0x0300_0001,
        fid: Fid::new(3, 1, 0, 0, 0),
        message_base: 100,
        flags: ObjectFlags::SOLID,
        material: Material::Stone,
    })
    .unwrap();
    assert!(CritterPlacement::from_prototype(&wall, 1, 1, 0, 0).is_err());
}

#[test]
fn a_layer_holding_the_critter_mounts_and_resolves() {
    let critter = one_critter();
    let mut layer = ModLayer::new("critter-test", "Synthetic critter layer", "0.1.0").unwrap();
    for (path, bytes) in &critter.files {
        layer.insert(path, bytes.clone()).unwrap();
    }
    layer.register_map("testmap.map", 200).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("mods").join("critter");
    layer.write(&dir).unwrap();

    let base = memory(vec![
        ("art/critters/critters.lst".into(), BASE_ART.to_vec()),
        ("proto/critters/critters.lst".into(), BASE_PROTO.to_vec()),
        ("text/english/game/pro_crit.msg".into(), BASE_MSG.to_vec()),
    ]);
    let mounted = vault_formats::vfs::ModLayer::new(
        "mods/critter",
        Some(LocalDirProvider::mount(&dir).unwrap()),
        None,
        None,
    );
    let vfs = LayeredVfs::new(Arc::new(base), vec![mounted]);

    // The art: the layer's list replaces the base list whole, and every
    // animation the engine asks for is a six-direction FRM.
    let list = vfs.read("art/critters/critters.lst").unwrap();
    assert!(list.starts_with(BASE_ART));
    let art = ArtCatalog::load(&vfs).unwrap();
    for (animation, suffix) in [(0, "aa"), (1, "ab"), (14, "ao"), (20, "ba"), (49, "rb")] {
        let fid = Fid::new(1, critter.stand.base(), animation, 0, 0);
        let path = art.path(fid).unwrap();
        assert_eq!(path, format!("art/critters/mj0001{suffix}.frm"));
        let archive = FrmArchive::parse(&vfs.read(&path).unwrap()).unwrap();
        let spec = &critter.frames[&path];
        assert_eq!(archive.frame_count(), spec.directions[0].frames.len());
        for d in 0..6 {
            assert_eq!(archive.shifts[d], spec.directions[d].shift);
        }
    }
    // The prototype, its name and description.
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    assert_eq!(
        catalog.lookup(critter.prototype.pid).unwrap().kind,
        PrototypeKind::Critter
    );
    let messages =
        MsgCatalog::parse_strict(&vfs.read("text/english/game/pro_crit.msg").unwrap()).unwrap();
    let base = critter.prototype.message_base;
    assert_eq!(messages.get(base).unwrap().text, b"Townsperson");
    assert_eq!(messages.get(base + 1).unwrap().text, b"A resident.");
    assert_eq!(messages.get(100).unwrap().text, b"Base critter");
    // The map, through the mounted catalog.
    let map = MapFile::parse_for(
        &vfs.read("maps/testmap.map").unwrap(),
        &catalog,
        GameProfile::Fallout2,
    )
    .unwrap();
    assert!(map.diagnostics.is_empty());
    assert_eq!(map.objects.elevations[0][0].pid, critter.prototype.pid);
    let registry = vault_formats::mod_manifest::ModRegistry::from_vfs(&vfs).unwrap();
    assert_eq!(registry.map_number(b"testmap.map"), Some(200));
}
