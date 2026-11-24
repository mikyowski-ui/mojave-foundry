//! Critter animation codes, suffixes and requirements, the animation set,
//! and non-looping motion (falls), read back through the pinned
//! `vault-formats` FRM parser and art catalog. All frames are synthetic.

use emit::animation::{
    Anim, AnimationSet, AttackMode, Need, WeaponCode, combat_requirements, unarmed_requirements,
    weapon_requirements,
};
use emit::critter::{action_frame_at, action_frame_for_contact, free_offsets};
use emit::{
    CritterFrames, CritterList, FormatWriters, FrmOptions, FrmSpec, LocalWriters, Motion,
    RenderedFrame, critter_frm, critter_frm_with,
};
use std::collections::BTreeMap;
use vault_formats::FrmArchive;
use vault_formats::fid::critter_suffix;

fn rendered(width: u32, height: u32, seed: u8, travel: [f32; 2]) -> RenderedFrame {
    let mut pixels = vec![0_u8; (width * height) as usize];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            pixels[(y * width + x) as usize] = 1 + ((x * 5 + y * 11 + u32::from(seed)) % 200) as u8;
        }
    }
    RenderedFrame {
        origin: [-(width as i32) / 2, -(height as i32) + 1],
        width,
        height,
        pixels,
        travel,
    }
}

/// A fall: `count` frames per direction, the figure getting wider and
/// lower, sliding `slide` pixels a frame (different per direction).
fn fall(count: usize, slide: [f32; 2]) -> CritterFrames {
    CritterFrames {
        fps: 10,
        directions: std::array::from_fn(|d| {
            (0..count)
                .map(|f| {
                    let travel = if f == 0 {
                        [0.0, 0.0]
                    } else {
                        [
                            slide[0] * (d as f32 + 1.0) / 3.0,
                            slide[1] * (1.0 - d as f32 * 0.3),
                        ]
                    };
                    rendered(
                        10 + 4 * f as u32,
                        40 - 5 * f as u32,
                        (d * 10 + f) as u8,
                        travel,
                    )
                })
                .collect()
        }),
    }
}

fn parse(spec: &FrmSpec) -> FrmArchive {
    FrmArchive::parse(&LocalWriters.frm(spec).unwrap()).unwrap()
}

/// Every opaque pixel drawn for direction `d`, frame `f`, relative to the
/// hex anchor, with the engine's accumulated displacement after a forward
/// pass from frame 0 (SPEC-001 §3.5).
fn drawn(archive: &FrmArchive, d: usize, f: usize) -> BTreeMap<(i64, i64), u8> {
    let frame = archive.frame(d, f).unwrap();
    let displacement = archive.accumulated_displacement(d, f).unwrap();
    let rect = frame.world_rect([0, 0], archive.shifts[d], displacement);
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

/// The source pixels of frame `f`, moved by the rounded total travel.
fn expected(frames: &CritterFrames, d: usize, f: usize) -> BTreeMap<(i64, i64), u8> {
    let mut total = [0.0_f64; 2];
    for frame in &frames.directions[d][..=f] {
        total[0] += f64::from(frame.travel[0]);
        total[1] += f64::from(frame.travel[1]);
    }
    let moved = total.map(|t| t.round() as i64);
    let frame = &frames.directions[d][f];
    let mut out = BTreeMap::new();
    for y in 0..frame.height {
        for x in 0..frame.width {
            let pixel = frame.pixels[(y * frame.width + x) as usize];
            if pixel != 0 {
                out.insert(
                    (
                        i64::from(frame.origin[0]) + i64::from(x) + moved[0],
                        i64::from(frame.origin[1]) + i64::from(y) + moved[1],
                    ),
                    pixel,
                );
            }
        }
    }
    out
}

#[test]
fn codes_and_suffixes_follow_the_codec() {
    for code in 0..=64 {
        let anim = Anim::from_code(code).unwrap();
        assert_eq!(anim.code(), code);
        for weapon in WeaponCode::ALL {
            let codec = critter_suffix(code, weapon.code());
            assert_eq!(anim.suffix(weapon).ok(), codec.ok(), "{anim:?} {weapon:?}");
        }
    }
    assert_eq!(Anim::from_code(65), None);
    assert_eq!(Anim::all().count(), 65);
    let w = WeaponCode::Unarmed;
    let table = [
        (Anim::Stand, "aa"),
        (Anim::Walk, "ab"),
        (Anim::ClimbLadder, "ae"),
        (Anim::GroundReach, "ak"),
        (Anim::MiddleReach, "al"),
        (Anim::Dodge, "an"),
        (Anim::HitFront, "ao"),
        (Anim::HitBack, "ap"),
        (Anim::Punch, "aq"),
        (Anim::Kick, "ar"),
        (Anim::Throw, "as"),
        (Anim::Run, "at"),
        (Anim::FallBack, "ba"),
        (Anim::FallFront, "bb"),
        (Anim::BloodyFallBack, "bo"),
        (Anim::BloodyFallFront, "bp"),
        (Anim::GetUpFront, "ch"),
        (Anim::GetUpBack, "cj"),
        (Anim::DeadFallBack, "ra"),
        (Anim::DeadFallFront, "rb"),
        (Anim::DeadBloodyFallFront, "rp"),
        (Anim::CalledShot, "na"),
    ];
    for (anim, suffix) in table {
        assert_eq!(anim.suffix_str(w).unwrap(), suffix, "{anim:?}");
    }
    let pistol = WeaponCode::Pistol;
    assert_eq!(pistol.letter(), Some(b'h'));
    for (anim, suffix) in [
        (Anim::Stand, "ha"),
        (Anim::Walk, "hb"),
        (Anim::Dodge, "he"),
        (Anim::TakeOut, "hc"),
        (Anim::PutAway, "hd"),
        (Anim::Point, "hh"),
        (Anim::Unpoint, "hi"),
        (Anim::SingleFire, "hj"),
        (Anim::Burst, "hk"),
        (Anim::ContinuousFire, "hl"),
        (Anim::Run, "at"),
        (Anim::HitFront, "ao"),
        (Anim::Throw, "as"),
    ] {
        assert_eq!(anim.suffix_str(pistol).unwrap(), suffix, "{anim:?}");
    }
    assert_eq!(Anim::Throw.suffix_str(WeaponCode::Knife).unwrap(), "dm");
    assert_eq!(Anim::Throw.suffix_str(WeaponCode::Spear).unwrap(), "gm");
    assert!(Anim::SingleFire.suffix(w).is_err());
    assert_eq!(Anim::FallBack.corpse(), Some(Anim::DeadFallBack));
    assert_eq!(
        Anim::BloodyFallFront.corpse(),
        Some(Anim::DeadBloodyFallFront)
    );
    assert_eq!(Anim::Stand.corpse(), None);
    assert!(Anim::FallFront.may_use_direction_files());
    assert!(!Anim::FireDance.may_use_direction_files());
    assert!(Anim::Electrify.aliased() && Anim::CalledShot.aliased());
    assert!(!Anim::FallBack.aliased());
    assert!(Anim::DeadFallBack.ends_prone() && !Anim::GetUpBack.ends_prone());
    assert_eq!(AttackMode::Burst.animation(), Some(Anim::Burst));
    assert_eq!(WeaponCode::from_code(10), Some(WeaponCode::Launcher));
    assert_eq!(WeaponCode::from_code(11), None);
}

#[test]
fn requirements_name_one_file_each() {
    let unarmed = unarmed_requirements();
    let required: Vec<String> = unarmed
        .iter()
        .filter(|r| r.need == Need::Required)
        .map(|r| r.suffix().unwrap())
        .collect();
    assert_eq!(
        required,
        [
            "aa", "ab", "an", "ao", "aq", "ba", "bb", "cj", "ch", "ra", "rb"
        ]
    );
    let pistol = weapon_requirements(WeaponCode::Pistol, &[AttackMode::Single, AttackMode::None]);
    let suffixes: Vec<(String, Need)> = pistol
        .iter()
        .map(|r| (r.suffix().unwrap(), r.need))
        .collect();
    for wanted in ["ha", "hb", "he", "hc", "hj", "hh", "hi"] {
        assert!(
            suffixes.contains(&(wanted.into(), Need::Required)),
            "{wanted} in {suffixes:?}"
        );
    }
    assert!(suffixes.contains(&("hd".into(), Need::Recommended)));
    let all = combat_requirements(&[
        (WeaponCode::Pistol, [AttackMode::Single, AttackMode::None]),
        (WeaponCode::Spear, [AttackMode::Thrust, AttackMode::Throw]),
        (WeaponCode::Unarmed, [AttackMode::Throw, AttackMode::None]),
        (WeaponCode::Pistol, [AttackMode::Single, AttackMode::Burst]),
    ]);
    let names: Vec<String> = all.iter().map(|r| r.suffix().unwrap()).collect();
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "one entry per file: {names:?}");
    for wanted in ["gm", "gf", "ga", "as", "hk", "aa"] {
        assert!(names.contains(&wanted.to_string()), "{wanted}");
    }
    // The throw is required (a weapon is usable only with its art).
    let throw = all
        .iter()
        .find(|r| r.anim == Anim::Throw && r.weapon == WeaponCode::Unarmed);
    assert_eq!(throw.unwrap().need, Need::Required);
}

#[test]
fn an_animation_set_fills_placeholders_and_resolves_through_the_list() {
    let stand = critter_frm(&fall(1, [0.0, 0.0]), 0).unwrap();
    let mut set = AnimationSet::new();
    assert!(
        set.fill_placeholders(&unarmed_requirements(), Need::Required)
            .is_err()
    );
    set.insert(Anim::Stand, WeaponCode::Unarmed, stand.clone())
        .unwrap();
    set.insert(Anim::Stand, WeaponCode::Pistol, stand.clone())
        .unwrap();
    // A suffix that ignores the weapon is one file.
    set.insert(Anim::HitFront, WeaponCode::Pistol, stand.clone())
        .unwrap();
    assert!(set.contains(Anim::HitFront, WeaponCode::Unarmed));
    assert!(
        set.insert(Anim::SingleFire, WeaponCode::Unarmed, stand.clone())
            .is_err()
    );
    let mut one = stand.clone();
    one.directions.truncate(1);
    assert!(set.insert(Anim::Walk, WeaponCode::Unarmed, one).is_err());

    let requirements =
        combat_requirements(&[(WeaponCode::Pistol, [AttackMode::Single, AttackMode::None])]);
    let filled = set
        .fill_placeholders(&requirements, Need::Recommended)
        .unwrap();
    assert!(!filled.is_empty());
    assert!(set.missing(&requirements, Need::Recommended).is_empty());
    assert!(!set.missing(&requirements, Need::Optional).is_empty());
    let still = set.get(Anim::Dodge, WeaponCode::Pistol).unwrap();
    assert_eq!(still.directions[0].frames.len(), 1);

    let mut list = CritterList::from_base(b"hmbase,0\n").unwrap();
    let row = list.append("mjtest").unwrap();
    let files = set.files("mjtest").unwrap();
    for (anim, weapon, frm) in set.iter() {
        let art = list.art(row, anim.code(), weapon.code()).unwrap();
        assert_eq!(files.get(&art.file_name), Some(&frm), "{}", art.file_name);
        parse(frm);
    }
    assert_eq!(files.len(), set.len());
    assert!(files.contains_key("mjtesthj.frm"));
    assert!(files.contains_key("mjtestra.frm"));
}

#[test]
fn free_and_baked_falls_draw_the_figure_where_it_was_rendered() {
    let frames = fall(6, [5.4, 1.7]);
    let free = critter_frm_with(
        &frames,
        &FrmOptions {
            action_frame: 2,
            motion: Motion::Free,
        },
    )
    .unwrap();
    let baked = critter_frm_with(
        &frames,
        &FrmOptions {
            action_frame: 2,
            motion: Motion::Baked,
        },
    )
    .unwrap();
    let (free, baked) = (parse(&free), parse(&baked));
    assert_eq!((free.action_frame, baked.action_frame), (2, 2));
    for d in 0..6 {
        let mut total = [0_i32; 2];
        for f in 0..6 {
            assert_eq!(drawn(&free, d, f), expected(&frames, d, f), "free {d} {f}");
            assert_eq!(
                drawn(&baked, d, f),
                expected(&frames, d, f),
                "baked {d} {f}"
            );
            assert_eq!(baked.frame(d, f).unwrap().displacement, [0, 0]);
            let offset = free.frame(d, f).unwrap().displacement;
            total = [
                total[0] + i32::from(offset[0]),
                total[1] + i32::from(offset[1]),
            ];
        }
        // The figure ends away from its hex by the rounded total travel,
        // which need not be a hex step.
        let sum = frames.directions[d].iter().fold([0.0_f64; 2], |s, f| {
            [s[0] + f64::from(f.travel[0]), s[1] + f64::from(f.travel[1])]
        });
        assert_eq!(total, sum.map(|v| v.round() as i32));
        assert_ne!(total, [0, 0]);
    }
    // Cycle is what critter_frm does.
    let cycle = critter_frm_with(&fall(4, [0.0, 0.0]), &FrmOptions::default()).unwrap();
    assert_eq!(cycle, critter_frm(&fall(4, [0.0, 0.0]), 0).unwrap());
}

#[test]
fn free_offsets_carry_the_remainder() {
    let offsets = free_offsets(&[[0.4, -0.4], [0.4, -0.4], [0.4, -0.4], [0.4, -0.4]]).unwrap();
    assert_eq!(offsets, [[0, 0], [1, -1], [0, 0], [1, -1]]);
    assert!(free_offsets(&[[f32::NAN, 0.0]]).is_err());
    assert!(free_offsets(&[[40_000.0, 0.0]]).is_err());
}

#[test]
fn the_action_frame_is_the_contact_frame_plus_one() {
    // SPEC-007 §3.10.1: a delay of A starts on the step showing frame A - 1.
    assert_eq!(action_frame_for_contact(2, 11).unwrap(), 3);
    assert_eq!(action_frame_for_contact(0, 5).unwrap(), 1);
    assert_eq!(action_frame_for_contact(4, 5).unwrap(), 4);
    assert_eq!(action_frame_for_contact(0, 1).unwrap(), 0);
    assert!(action_frame_for_contact(5, 5).is_err());
    assert_eq!(action_frame_at(0.3, 10, 12).unwrap(), 4);
    assert_eq!(action_frame_at(0.3, 0, 12).unwrap(), 4);
    assert_eq!(action_frame_at(5.0, 10, 12).unwrap(), 11);
    assert!(action_frame_at(-1.0, 10, 12).is_err());
    let frames = fall(5, [0.0, 0.0]);
    let action = action_frame_for_contact(3, 5).unwrap();
    let spec = critter_frm(&frames, action).unwrap();
    assert_eq!(parse(&spec).action_frame, 4);
    assert!(critter_frm(&frames, 5).is_err());
}
