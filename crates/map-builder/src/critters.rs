//! Non-player characters: where they stand, which way they face, their
//! sprite set names, AI packet and team (contracts section 10).
//!
//! **Placement.** Critters are placed in input order. Each goes on the
//! walkable hex whose centre is nearest its source position on the ground,
//! among the hexes at most [`CRITTER_RADIUS`] steps from the hex under
//! that position, that holds no exit grid, no door, no earlier critter,
//! and is neither the map entrance nor any door's return hex (critter art
//! blocks movement in the engine, so those must stay clear). Ties go to
//! the lower hex. A critter with no such hex, a position off the grid, a
//! non-finite position or yaw, or hit points below 1 is skipped and
//! reported, never an error. Critters do not change walkability or the
//! sealing border: they can move, scenery cannot.
//!
//! **Facing.** [`crate::walk::Frame::orientation_of_yaw`] and
//! [`crate::walk::Frame::yaw_of_orientation`], both derived from the
//! projection.
//!
//! **Names.** Sprite set `n` (counting the sets placed critters use, in
//! ascending [`crate::BuildInput::critter_art`] index) is stem `mj` plus
//! `n` in four lowercase base-36 digits (contract 10.4), skipping a stem
//! the base list already holds; its files are
//! `art/critters/<stem><suffix>.frm`.
//!
//! **AI packet.** [`choose_ai_packet`]. **Team.** [`DEFAULT_CRITTER_TEAM`].

use std::collections::{BTreeMap, BTreeSet};

use emit::CritterList;
use vault_formats::ai::Packets;
use vault_formats::{FrmArchive, GameProfile, HexGrid};

use crate::exits::hexes_around;
use crate::walk::Frame;
use crate::{BuildError, CritterArt, CritterPlacement, SkippedCritter};

/// How far, in hex steps from the hex under its source position, a
/// critter may be moved to find a free walkable hex.
pub const CRITTER_RADIUS: i32 = 8;

/// The default team of every placed critter. Team 0 is the player's; the
/// base game's own critters use small team numbers, so a shared team well
/// above them keeps converted townsfolk out of base factions' fights while
/// an attack on one still brings in the rest (contract 10.4: one shared
/// team that is not the player's).
pub const DEFAULT_CRITTER_TEAM: i32 = 99;

/// The packet used when no base `data/ai.txt` is supplied (a synthetic
/// build). The engine falls back to the table's first packet for a number
/// it lacks, so any number loads.
pub const FALLBACK_AI_PACKET: i32 = 0;

/// The base AI table's path.
pub const AI_TXT: &str = "data/ai.txt";

/// The prefix of every sprite set stem (contract 10.4).
pub const STEM_PREFIX: &str = "mj";

/// The animations every sprite set must have: stand and walk. Without the
/// stand the critter is invisible; without the walk it vanishes while it
/// walks.
pub const REQUIRED_SUFFIXES: [&str; 2] = ["aa", "ab"];

/// Stem number `n`: [`STEM_PREFIX`] and four lowercase base-36 digits.
/// `None` from 36⁴ on.
pub fn critter_stem(number: u32) -> Option<String> {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if number >= 36u32.pow(4) {
        return None;
    }
    let mut digits = [b'0'; 4];
    let mut rest = number;
    for digit in digits.iter_mut().rev() {
        *digit = DIGITS[(rest % 36) as usize];
        rest /= 36;
    }
    Some(format!(
        "{STEM_PREFIX}{}",
        std::str::from_utf8(&digits).unwrap_or("0000")
    ))
}

/// The next stem from `*next` on that `list` does not hold.
pub(crate) fn next_stem(list: &CritterList, next: &mut u32) -> Result<String, BuildError> {
    loop {
        let stem = critter_stem(*next).ok_or_else(|| {
            BuildError::Input(format!("no unused {STEM_PREFIX}NNNN critter stem is left"))
        })?;
        *next += 1;
        if list.find(&stem).is_none() {
            return Ok(stem);
        }
    }
}

/// The AI packet for peaceful townsfolk, chosen from a Fallout 2
/// `data/ai.txt` (parsed by `vault_formats::ai::Packets::parse_for`).
///
/// Rule: among the packets the engine can resolve by number (a
/// non-negative `packet_num`, and for a number several sections share,
/// the first in the table's sorted order, as `Packets::resolve` picks),
/// take the one with the **highest `min_hp`** (the engine makes a critter
/// flee whenever its hit points are below it, so the highest value fights
/// least), then the **lowest `chance`** (fewest combat taunts), then the
/// lowest packet number. It depends only on the table's bytes.
pub fn choose_ai_packet(ai_txt: &[u8]) -> Result<i32, BuildError> {
    let packets =
        Packets::parse_for(ai_txt, GameProfile::Fallout2).map_err(|e| BuildError::Base {
            file: AI_TXT.into(),
            message: e.to_string(),
        })?;
    let mut seen = BTreeSet::new();
    let mut best: Option<(i32, i32, i32)> = None;
    for packet in &packets.packets {
        if packet.packet_num < 0 || !seen.insert(packet.packet_num) {
            continue;
        }
        let candidate = (packet.min_hp, packet.chance, packet.packet_num);
        let better = best.is_none_or(|(min_hp, chance, number)| {
            (candidate.0, -candidate.1, -candidate.2) > (min_hp, -chance, -number)
        });
        if better {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, number)| number)
        .ok_or_else(|| BuildError::Base {
            file: AI_TXT.into(),
            message: "no packet has a non-negative number".into(),
        })
}

/// Whether a supplied `data/ai.txt` resolves `number` to a packet of that
/// number.
pub(crate) fn has_ai_packet(ai_txt: &[u8], number: i32) -> Result<bool, BuildError> {
    let packets =
        Packets::parse_for(ai_txt, GameProfile::Fallout2).map_err(|e| BuildError::Base {
            file: AI_TXT.into(),
            message: e.to_string(),
        })?;
    Ok(packets.resolve(number).1)
}

/// A sprite set's files by lowercase suffix, checked: every suffix two
/// ASCII letters (no two equal without case), [`REQUIRED_SUFFIXES`]
/// present, and every file an FRM whose six directions all resolve to the
/// same number of frames (a block shared between directions counts; the
/// format cannot tell one direction from six equal ones).
pub(crate) fn checked_art(
    index: usize,
    art: &CritterArt,
) -> Result<BTreeMap<String, Vec<u8>>, BuildError> {
    let bad = |message: String| {
        Err(BuildError::Input(format!(
            "critter art {index} ({}): {message}",
            art.label
        )))
    };
    let mut files = BTreeMap::new();
    for (suffix, bytes) in &art.frms {
        if suffix.len() != 2 || !suffix.bytes().all(|b| b.is_ascii_alphabetic()) {
            return bad(format!("suffix {suffix:?} is not two ASCII letters"));
        }
        let key = suffix.to_ascii_lowercase();
        if files.contains_key(&key) {
            return bad(format!("suffix {key:?} appears twice"));
        }
        let archive = match FrmArchive::parse(bytes) {
            Ok(archive) => archive,
            Err(error) => return bad(format!("{key}: {error}")),
        };
        if let Some(direction) = (0..6)
            .find(|d| (0..archive.frame_count()).any(|frame| archive.frame(*d, frame).is_none()))
        {
            return bad(format!("{key}: direction {direction} lacks frames"));
        }
        files.insert(key, bytes.clone());
    }
    for required in REQUIRED_SUFFIXES {
        if !files.contains_key(required) {
            return bad(format!(
                "has no {required:?} animation; a critter needs stand (aa) and walk (ab)"
            ));
        }
    }
    Ok(files)
}

/// Where one critter goes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Spot {
    pub index: usize,
    pub hex: i32,
    pub orientation: u8,
    pub steps: i32,
    pub moved: f64,
}

/// Place every critter by the rule in the module notes. `reserved` holds
/// the hexes no critter may take besides those the earlier critters take:
/// exit grids, doors, the entrance and return hexes.
///
/// Each critter's search radius ([`CRITTER_RADIUS`] on an exterior build),
/// or the reason it is skipped (checked after its hit points, position and
/// rotation), is `radius` of its index.
pub(crate) fn place(
    frame: &Frame,
    critters: &[CritterPlacement],
    walkable: &[bool],
    reserved: &BTreeSet<i32>,
    radius: impl Fn(usize) -> Result<i32, String>,
) -> (Vec<Spot>, Vec<SkippedCritter>) {
    let mut taken: BTreeSet<i32> = BTreeSet::new();
    let mut spots = Vec::new();
    let mut skipped = Vec::new();
    for (index, critter) in critters.iter().enumerate() {
        let skip = |reason: String| SkippedCritter {
            reference: critter.reference.clone(),
            index,
            reason,
        };
        if critter.hit_points < 1 {
            skipped.push(skip(format!(
                "hit points {}; a critter at 0 or below is dead",
                critter.hit_points
            )));
            continue;
        }
        let [x, y] = critter.position.map(f64::from);
        if !x.is_finite() || !y.is_finite() {
            skipped.push(skip("its position is not finite".into()));
            continue;
        }
        let Some(orientation) = frame.orientation_of_yaw(critter.yaw) else {
            skipped.push(skip("its rotation is not finite".into()));
            continue;
        };
        let target = frame.tile_of_source(x, y);
        let Some(origin) = frame.hex_of_tile(target) else {
            skipped.push(skip(format!(
                "its position ({x:.1}, {y:.1}) is outside the map"
            )));
            continue;
        };
        let reach = match radius(index) {
            Ok(reach) => reach,
            Err(reason) => {
                skipped.push(skip(reason));
                continue;
            }
        };
        let mut best: Option<(f64, i32)> = None;
        for hex in hexes_around(origin, reach, false) {
            if !walkable.get(hex as usize).copied().unwrap_or(false)
                || reserved.contains(&hex)
                || taken.contains(&hex)
            {
                continue;
            }
            let Some(c) = frame.hex_centre_tile(hex) else {
                continue;
            };
            let d = (c[0] - target[0]).powi(2) + (c[1] - target[1]).powi(2);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, hex));
            }
        }
        let Some((distance, hex)) = best else {
            skipped.push(skip(format!(
                "no free walkable hex within {reach} steps of hex {origin}"
            )));
            continue;
        };
        taken.insert(hex);
        spots.push(Spot {
            index,
            hex,
            orientation,
            steps: HexGrid::lattice_distance(origin, hex).unwrap_or(0),
            moved: distance.sqrt() * frame.tile_units(),
        });
    }
    (spots, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_are_mj_and_four_base_36_digits() {
        assert_eq!(critter_stem(0).unwrap(), "mj0000");
        assert_eq!(critter_stem(35).unwrap(), "mj000z");
        assert_eq!(critter_stem(36).unwrap(), "mj0010");
        assert_eq!(critter_stem(36u32.pow(4) - 1).unwrap(), "mjzzzz");
        assert!(critter_stem(36u32.pow(4)).is_none());
        let list = CritterList::from_base(b"MJ0000,0\nmj0002,1\n").unwrap();
        let mut next = 0;
        assert_eq!(next_stem(&list, &mut next).unwrap(), "mj0001");
        assert_eq!(next_stem(&list, &mut next).unwrap(), "mj0003");
    }

    fn packet(name: &str, number: i32, min_hp: i32, chance: i32) -> String {
        let mut text = format!("[{name}]\npacket_num={number}\nmin_hp={min_hp}\nchance={chance}\n");
        for key in vault_formats::ai::KEYS
            .iter()
            .chain(vault_formats::ai::END_KEYS.iter())
        {
            if !["packet_num", "min_hp", "chance"].contains(key) {
                text.push_str(&format!("{key}=0\n"));
            }
        }
        text
    }

    #[test]
    fn the_packet_rule_prefers_fleeing_then_quiet_then_low_numbers() {
        let table = [
            packet("aaa", 1, 10, 50),
            packet("bbb", 4, 30, 20),
            packet("ccc", 2, 30, 20),
            packet("ddd", 3, 30, 40),
            packet("eee", -1, 99, 0),
        ]
        .concat();
        assert_eq!(choose_ai_packet(table.as_bytes()).unwrap(), 2);
        // A number two sections share is judged by the first in sorted
        // order, as the engine resolves it.
        let shared = [
            packet("aaa", 5, 1, 0),
            packet("zzz", 5, 90, 0),
            packet("mmm", 6, 2, 0),
        ]
        .concat();
        assert_eq!(choose_ai_packet(shared.as_bytes()).unwrap(), 6);
        assert!(has_ai_packet(shared.as_bytes(), 5).unwrap());
        assert!(!has_ai_packet(shared.as_bytes(), 7).unwrap());
        assert!(choose_ai_packet(packet("x", -2, 1, 1).as_bytes()).is_err());
        assert!(choose_ai_packet(b"[broken]\npacket_num=1\n").is_err());
    }
}
