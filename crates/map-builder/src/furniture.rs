//! Furniture footprints, and upright art a walker can stand inside.
//!
//! **Why furniture can be walkable.** Only three things block a hex in
//! this crate: the caller's [`crate::ElevationInput::blocked_hexes`], a
//! blocking placement on its **anchor** hex, and the sealing border around
//! what that leaves walkable. Nothing else is known of an object's base:
//!
//! - a single sprite ([`crate::Role::Scenery`], [`crate::Role::Container`])
//!   blocks the one hex it is anchored on, however wide its base is (the
//!   slicer makes a sprite of anything up to two hexes and 150 units long,
//!   and puts nothing of its footprint in `blocked_hexes`);
//! - a sliced object's wall strips block only near its bounding footprint
//!   and never on a hex in the slicer's `open_hexes`. An interior run, and a
//!   standalone exterior run, open **every** standable hex (the navmesh
//!   within [`crate::BuildOptions::navmesh_tolerance`] of a hex centre),
//!   so wherever the source navmesh runs under or close to a table or a
//!   counter, its strips are placed `blocking: false` and the hex stays
//!   walkable here, since the same navmesh covers it.
//!
//! So a pool table or a bar counter can be walked into. The fix belongs to
//! whoever knows the footprint: the caller passes the placed bounds of
//! furniture in [`crate::BuildOptions::footprints`], and [`footprint_hexes`]
//! blocks every hex its base covers. A build without footprints is
//! unchanged.
//!
//! **Suspects** ([`FurnitureReport::suspects`]) make the problem visible on
//! any build: wall strips that do not block although wall art stands on
//! their hex ([`crate::cover::Cover::stands_on`]: the strip's foot is on
//! the ground there, unlike an eave) and the hex is walkable on the
//! written map; and blocking single sprites at least [`WIDE_SPRITE`] pixels
//! wide (a hex is 32) whose reference has no footprint, which block only
//! their anchor hex.

use std::collections::{BTreeMap, BTreeSet};

use crate::cover::Cover;
use crate::walk::Frame;
use crate::{ArtImage, Placement, Role};

/// How far outside a footprint polygon (floor squares, tile space) a hex
/// centre still counts as covered: about a third of a hex, so a table edge
/// that runs close to a hex centre blocks it.
pub const FOOTPRINT_REACH: f64 = 0.15;
/// A blocking sprite at least this wide (pixels) is reported as a suspect
/// when no footprint is given for it.
pub const WIDE_SPRITE: u32 = 48;
/// At most this many hexes of a suspect are listed.
const LISTED_HEXES: usize = 12;

/// What a suspect is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SuspectKind {
    /// Wall strips of the reference that do not block stand on walkable
    /// hexes.
    OpenStrips,
    /// A blocking sprite wider than a hex and a half blocks only its
    /// anchor hex.
    WideSprite,
}

/// One reference whose art a walker may stand inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FurnitureSuspect {
    pub reference: String,
    pub elevation: u8,
    pub kind: SuspectKind,
    /// The art's source label (model path).
    pub source: String,
    /// The hexes concerned (at most twelve listed), and how many in all.
    pub hexes: Vec<i32>,
    pub count: usize,
}

/// Footprints used and suspects found, per map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FurnitureReport {
    /// References with a footprint given ([`crate::BuildOptions::footprints`]).
    pub footprints_given: usize,
    /// Of those, references with an upright placement on the map.
    pub footprints_used: usize,
    /// Hexes the footprints blocked that nothing else blocked.
    pub blocked_hexes: usize,
    /// Footprints that cover no hex of the map.
    pub empty_footprints: Vec<String>,
    pub suspects: Vec<FurnitureSuspect>,
}

impl FurnitureReport {
    /// Plain-language lines for the map report; empty when there is
    /// nothing to say.
    pub fn summary(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.footprints_given > 0 {
            lines.push(format!(
                "furniture footprints: {} given, {} on the map, {} hexes blocked by them alone{}",
                self.footprints_given,
                self.footprints_used,
                self.blocked_hexes,
                if self.empty_footprints.is_empty() {
                    String::new()
                } else {
                    format!(", {} covering no hex", self.empty_footprints.len())
                }
            ));
        }
        for s in &self.suspects {
            let hexes: Vec<String> = s.hexes.iter().map(i32::to_string).collect();
            let what = match s.kind {
                SuspectKind::OpenStrips => format!(
                    "{} non-blocking wall strips stand on walkable hexes",
                    s.count
                ),
                SuspectKind::WideSprite => {
                    "a wide blocking sprite blocks only its anchor hex".to_owned()
                }
            };
            lines.push(format!(
                "walkable furniture? {} ({}), elevation {}: {what} ({}); pass its footprint in \
                 BuildOptions::footprints",
                s.reference,
                s.source,
                s.elevation,
                hexes.join(", ")
            ));
        }
        lines
    }
}

/// Whether `p` is inside the polygon `poly` (even-odd rule).
fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if p[0] < x {
                odd = !odd;
            }
        }
    }
    odd
}

/// The distance from `p` to the polygon's edges.
fn distance(poly: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = poly.len();
    let mut best = f64::INFINITY;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = d[0] * d[0] + d[1] * d[1];
        let t = if len > 0.0 {
            (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = [a[0] + t * d[0], a[1] + t * d[1]];
        best = best.min(((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt());
    }
    best
}

/// The hexes a footprint (source-space corners) covers: every hex whose
/// centre lies inside the polygon or within [`FOOTPRINT_REACH`] floor
/// squares of it. Sorted. A polygon of fewer than three finite corners
/// covers nothing.
pub fn footprint_hexes(frame: &Frame, corners: &[[f32; 2]]) -> Vec<i32> {
    if corners.len() < 3 || corners.iter().flatten().any(|v| !v.is_finite()) {
        return Vec::new();
    }
    let poly: Vec<[f64; 2]> = corners
        .iter()
        .map(|c| frame.tile_of_source(f64::from(c[0]), f64::from(c[1])))
        .collect();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &poly {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let reach = FOOTPRINT_REACH + 0.5;
    // Hexes are about half a floor square apart: sample at a quarter.
    let mut candidates = BTreeSet::new();
    let step = 0.25;
    let mut s = lo[0] - reach;
    while s <= hi[0] + reach {
        let mut t = lo[1] - reach;
        while t <= hi[1] + reach {
            if let Some(hex) = frame.hex_of_tile([s, t]) {
                candidates.insert(hex);
            }
            t += step;
        }
        s += step;
        // A footprint larger than the map is cut by it anyway.
        if s - lo[0] > 400.0 {
            break;
        }
    }
    candidates
        .into_iter()
        .filter(|hex| {
            frame
                .hex_centre_tile(*hex)
                .is_some_and(|c| inside(&poly, c) || distance(&poly, c) <= FOOTPRINT_REACH)
        })
        .collect()
}

/// Whether a placement of this role is upright furniture a footprint
/// applies to.
pub(crate) fn takes_footprint(role: Role) -> bool {
    matches!(role, Role::WallStrip | Role::Scenery | Role::Container)
}

/// Per elevation: the hexes the given footprints block on it, the
/// references used, and the footprints that cover no hex.
pub(crate) struct ElevationFootprints {
    pub hexes: BTreeSet<i32>,
    pub used: BTreeSet<String>,
    pub empty: BTreeSet<String>,
}

/// The footprints that apply to `placements` (doors' own hexes excepted).
pub(crate) fn elevation_footprints(
    frame: &Frame,
    footprints: &BTreeMap<String, Vec<[f32; 2]>>,
    placements: &[Placement],
) -> ElevationFootprints {
    let mut out = ElevationFootprints {
        hexes: BTreeSet::new(),
        used: BTreeSet::new(),
        empty: BTreeSet::new(),
    };
    if footprints.is_empty() {
        return out;
    }
    let upright: BTreeSet<&str> = placements
        .iter()
        .filter(|p| takes_footprint(p.role))
        .map(|p| p.reference.as_str())
        .collect();
    let doors: BTreeSet<i32> = placements
        .iter()
        .filter(|p| p.role == Role::Door)
        .map(|p| p.hex)
        .collect();
    for (reference, corners) in footprints {
        if !upright.contains(reference.as_str()) {
            continue;
        }
        out.used.insert(reference.clone());
        let hexes = footprint_hexes(frame, corners);
        if hexes.is_empty() {
            out.empty.insert(reference.clone());
        }
        out.hexes
            .extend(hexes.into_iter().filter(|h| !doors.contains(h)));
    }
    out
}

/// The suspects of one elevation (module notes).
pub(crate) fn suspects(
    elevation: u8,
    art: &[ArtImage],
    placements: &[Placement],
    walkable: &[bool],
    cover: &Cover,
    footprints: &BTreeMap<String, Vec<[f32; 2]>>,
) -> Vec<FurnitureSuspect> {
    let mut open: BTreeMap<&str, (BTreeSet<i32>, usize)> = BTreeMap::new();
    let mut sprites: BTreeMap<&str, Vec<&Placement>> = BTreeMap::new();
    for p in placements {
        match p.role {
            Role::WallStrip
                if !p.blocking
                    && walkable.get(p.hex as usize).copied().unwrap_or(false)
                    && cover.stands_on(p.hex) =>
            {
                let entry = open.entry(&p.reference).or_insert((BTreeSet::new(), p.art));
                entry.0.insert(p.hex);
            }
            Role::Scenery | Role::Container if p.blocking => {
                sprites.entry(&p.reference).or_default().push(p);
            }
            _ => {}
        }
    }
    let source = |index: usize| art.get(index).map_or(String::new(), |a| a.source.clone());
    let mut out = Vec::new();
    for (reference, (hexes, first)) in open {
        out.push(FurnitureSuspect {
            reference: reference.to_owned(),
            elevation,
            kind: SuspectKind::OpenStrips,
            source: source(first),
            count: hexes.len(),
            hexes: hexes.into_iter().take(LISTED_HEXES).collect(),
        });
    }
    for (reference, list) in sprites {
        if list.len() != 1 || footprints.contains_key(reference) {
            continue;
        }
        let p = list[0];
        if art.get(p.art).is_some_and(|a| a.width >= WIDE_SPRITE) {
            out.push(FurnitureSuspect {
                reference: reference.to_owned(),
                elevation,
                kind: SuspectKind::WideSprite,
                source: source(p.art),
                hexes: vec![p.hex],
                count: 1,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_tests() {
        let square = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        assert!(inside(&square, [1.0, 1.0]));
        assert!(!inside(&square, [3.0, 1.0]));
        assert!((distance(&square, [3.0, 1.0]) - 1.0).abs() < 1e-9);
        assert!(distance(&square, [1.0, 1.0]) <= 1.0 + 1e-9);
    }
}
