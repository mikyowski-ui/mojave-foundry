//! Readability metrics ([`crate::ReadabilityReport`]) and the interactive
//! references' protection ([`crate::BuildOptions::interactive`]).
//!
//! Both are cheap: one pass over the hexes, one [`Cover::sight`] query per
//! walkable hex (the entrance search makes as many), and one pass over the
//! placements. Nothing here changes the layer: the metrics only read it,
//! and the protection changes a placement only when its reference is
//! listed.

use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::cover::{Cover, Sight, SightRule};
use crate::walk::{HEXES, Verdict, neighbours};
use crate::{InteractiveReport, Placement, ReadabilityReport, Role};

/// The placements as the build treats them: a placement whose reference is
/// in `interactive` never gets the floor piece treatment (flat,
/// non-blocking, never cut by the see-through circle): a [`Role::FloorPiece`]
/// of it is built as upright [`Role::Scenery`] that blocks as the slicer
/// said. Every other placement is as given. Borrowed, so unchanged, when
/// no placement needs it.
pub(crate) fn protect<'a>(
    placements: &'a [Placement],
    interactive: &BTreeSet<String>,
) -> (Cow<'a, [Placement]>, InteractiveReport) {
    let mut report = InteractiveReport {
        references: interactive.len(),
        ..InteractiveReport::default()
    };
    if interactive.is_empty() {
        return (Cow::Borrowed(placements), report);
    }
    let mut matched: BTreeSet<&str> = BTreeSet::new();
    let mut out: Option<Vec<Placement>> = None;
    for (index, placement) in placements.iter().enumerate() {
        if !interactive.contains(&placement.reference) {
            continue;
        }
        matched.insert(&placement.reference);
        report.placements += 1;
        if placement.role == Role::FloorPiece {
            report.kept_upright += 1;
            let list = out.get_or_insert_with(|| placements.to_vec());
            list[index].role = Role::Scenery;
        }
    }
    report.matched = matched.len();
    match out {
        Some(list) => (Cow::Owned(list), report),
        None => (Cow::Borrowed(placements), report),
    }
}

/// The readability of a built map (see [`ReadabilityReport`]).
///
/// - `verdicts`: the walkability pass (floor hexes are its `Walkable` and
///   `Blocked` ones);
/// - `walkable`: the final grid, passages included;
/// - `blocking`: per hex, whether a blocking placement stands on it;
/// - `figure`: the figure [`Cover::sight`] tests.
pub(crate) fn measure(
    verdicts: &[Verdict],
    walkable: &[bool],
    blocking: &[bool],
    cover: &Cover,
    figure: [u32; 2],
    rule: SightRule,
) -> ReadabilityReport {
    let mut report = ReadabilityReport {
        floor_hexes: verdicts
            .iter()
            .filter(|v| matches!(v, Verdict::Walkable | Verdict::Blocked))
            .count(),
        ..ReadabilityReport::default()
    };
    let blocks = |hex: i32| blocking.get(hex as usize).copied().unwrap_or(false);
    for hex in 0..HEXES as i32 {
        if !walkable[hex as usize] {
            continue;
        }
        report.walkable_hexes += 1;
        match cover.sight_by(hex, figure, rule) {
            Sight::Clear => report.clear += 1,
            Sight::Behind => report.behind += 1,
            Sight::Covered => report.covered += 1,
        }
        if blocks(hex) || neighbours(hex).any(blocks) {
            report.near_blocking += 1;
        }
    }
    report.open_floor_ratio = ratio(report.clear, report.floor_hexes);
    report.near_blocking_share = ratio(report.near_blocking, report.walkable_hexes);
    report
}

/// `part / whole`, 0 for an empty whole.
fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}
