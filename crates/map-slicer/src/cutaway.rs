//! The foreground cutaway: strips that hide walkers are cropped to a kerb
//! (the `declutter` module, rule 4).

use std::collections::{BTreeMap, BTreeSet};

use foundry_render::Projection;

use crate::declutter::Cutaway;
use crate::hex;
use crate::landmark::hexes_in_rect;
use crate::render::Canvas;

/// A strip with fewer pixels than this is a speck: it follows a cut
/// neighbour even when it stands behind someone.
const SPECK: usize = 160;

/// One sliced reference's cutaway inputs.
pub(crate) struct Trim<'a> {
    pub settings: &'a Cutaway,
    pub walkers: &'a BTreeSet<i32>,
    /// Width and height of a walker's box, bottom centre on its hex centre.
    pub walker_box: [i64; 2],
    /// The placement displacement: the origin's height on screen.
    pub offset: [i32; 2],
    /// The origin's height above the wall's foot: the lowest point of its
    /// placed bounds, but not below the floor band (the terrain outdoors).
    pub lift: f32,
}

/// What the cutaway did to one reference.
#[derive(Debug, Clone, Default)]
pub(crate) struct Trimmed {
    pub considered: usize,
    pub cut: usize,
    pub pixels_removed: u64,
    /// Strips left without a pixel: their hexes still block.
    pub emptied: Vec<i32>,
    pub covered_before: BTreeSet<i32>,
    pub covered_after: BTreeSet<i32>,
}

/// The walkers a set of canvas pixels (all owned by strip hex `strip`)
/// hides: walkable hexes below `strip` with more than `min_pixels` of them
/// in their box. With `behind`, the walkers it stands behind instead
/// (walkable hexes above `strip`), for whom it is a backdrop.
fn hidden(
    pixels: &[usize],
    strip: i32,
    at: &dyn Fn(usize) -> [i64; 2],
    trim: &Trim<'_>,
    behind: bool,
) -> Vec<i32> {
    if pixels.is_empty() {
        return Vec::new();
    }
    let mut rect = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    let points: Vec<[i64; 2]> = pixels.iter().map(|&i| at(i)).collect();
    for p in &points {
        rect = [
            rect[0].min(p[0]),
            rect[1].min(p[1]),
            rect[2].max(p[0]),
            rect[3].max(p[1]),
        ];
    }
    let [w, h] = trim.walker_box;
    let centres = [
        rect[0] - (w - 1) + w / 2,
        rect[1],
        rect[2] + w / 2,
        rect[3] + h - 1,
    ];
    let mut out = Vec::new();
    for walker in hexes_in_rect(centres) {
        let drawn_after = walker < strip;
        if walker == strip
            || drawn_after == behind
            || !trim.walkers.contains(&walker)
            || hex::distance(walker, strip) < trim.settings.min_distance
        {
            continue;
        }
        let Some(c) = hex::center(walker) else {
            continue;
        };
        let left = c[0] - w / 2;
        let b = [left, c[1] - h + 1, left + w - 1, c[1]];
        let inside = points
            .iter()
            .filter(|p| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3])
            .count();
        if inside > trim.settings.min_pixels {
            out.push(walker);
        }
    }
    out
}

/// Crops the strips (final owners, `-1` for none) of one rendered
/// reference whose origin's ground point is `ground`: a strip hiding more
/// than `max_walkers` walkers loses, in each pixel column, the lowest pixel
/// whose surface stands above the kerb and every pixel above it on screen,
/// and so does
/// a strip within `spread` hex steps of one that hides someone (not
/// chained, so a room piece's far walls stay) that hides nobody and is nobody's backdrop (or is a speck of
/// fewer than [`SPECK`] pixels), so that a wall is not left with stray
/// full-height pieces.
pub(crate) fn trim(
    canvas: &Canvas,
    owner: &mut [i32],
    ground: [i64; 2],
    projection: &Projection,
    trim: &Trim<'_>,
) -> Trimmed {
    let mut out = Trimmed::default();
    let w = canvas.width as usize;
    if w == 0 {
        return out;
    }
    let mut by_strip: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (i, &o) in owner.iter().enumerate() {
        if o >= 0 {
            by_strip.entry(o).or_default().push(i);
        }
    }
    let base = [
        ground[0] + i64::from(canvas.origin[0]) + i64::from(trim.offset[0]),
        ground[1] + i64::from(canvas.origin[1]) + i64::from(trim.offset[1]),
    ];
    let at = |i: usize| [base[0] + (i % w) as i64, base[1] + (i / w) as i64];
    let storey = f64::from(projection.storey_units());
    let kerb = f64::from(trim.settings.kerb_height);
    let lift = f64::from(trim.lift);
    let height = |i: usize| -> Option<f64> {
        let d = canvas.depth[i];
        if !d.is_finite() {
            return None;
        }
        let q = [
            canvas.origin[0] as f32 + (i % w) as f32 + 0.5,
            canvas.origin[1] as f32 + (i / w) as f32 + 0.5,
        ];
        let local = projection.unproject(q, d);
        Some(f64::from(local[2]) * storey + lift)
    };
    let mut hides: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    let mut cut: BTreeSet<i32> = BTreeSet::new();
    let mut lonely: BTreeSet<i32> = BTreeSet::new();
    for (&strip, pixels) in &by_strip {
        out.considered += 1;
        let before = hidden(pixels, strip, &at, trim, false);
        out.covered_before.extend(before.iter().copied());
        if before.len() > trim.settings.max_walkers {
            cut.insert(strip);
        } else if before.is_empty()
            && (pixels.len() < SPECK || hidden(pixels, strip, &at, trim, true).is_empty())
        {
            lonely.insert(strip);
        }
        hides.insert(strip, before);
    }
    // Along the wall: lonely strips near a strip that hides someone follow
    // it. Not chained: a room piece's far walls stay standing.
    if trim.settings.spread > 0 && !cut.is_empty() {
        let hiding: Vec<i32> = cut.iter().copied().collect();
        for l in lonely {
            if hiding
                .iter()
                .any(|&c| hex::distance(l, c) <= trim.settings.spread)
            {
                cut.insert(l);
            }
        }
    }
    for (strip, pixels) in by_strip {
        let before = hides.remove(&strip).unwrap_or_default();
        if !cut.contains(&strip) {
            out.covered_after.extend(before);
            continue;
        }
        // Column by column, from the bottom up: the first pixel above the
        // kerb and everything drawn above it leave, so the cut line is
        // clean (a lower surface seen past an edge does not stick out).
        let mut columns: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in pixels {
            columns.entry(i % w).or_default().push(i);
        }
        let mut kept = Vec::new();
        for (_, mut column) in columns {
            column.sort_unstable_by(|a, b| b.cmp(a));
            let cut_at = column
                .iter()
                .position(|&i| height(i).is_some_and(|e| e > kerb))
                .unwrap_or(column.len());
            for &i in &column[cut_at..] {
                owner[i] = -1;
                out.pixels_removed += 1;
            }
            kept.extend_from_slice(&column[..cut_at]);
        }
        kept.sort_unstable();
        out.cut += 1;
        if kept.is_empty() {
            out.emptied.push(strip);
        }
        out.covered_after
            .extend(hidden(&kept, strip, &at, trim, false));
    }
    out
}
