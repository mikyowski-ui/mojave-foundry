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
    /// Only the strips beside a narrow walkway are cut (to the narrow
    /// kerb), every other strip is left whole: the cutaway outdoors when
    /// only [`Cutaway::narrow_kerb_height`] asks for it
    /// (`Cutaway::exteriors` off).
    pub narrow_only: bool,
}

/// What the cutaway did to one reference.
#[derive(Debug, Clone, Default)]
pub(crate) struct Trimmed {
    pub considered: usize,
    pub cut: usize,
    pub pixels_removed: u64,
    /// Strips left without a pixel: their hexes still block.
    pub emptied: Vec<i32>,
    /// The hexes of every strip cut.
    pub cut_hexes: Vec<i32>,
    /// Of the strips cut, those cut to the narrow-walkway kerb
    /// ([`Cutaway::narrow_kerb_height`]).
    pub narrow: usize,
    pub covered_before: BTreeSet<i32>,
    pub covered_after: BTreeSet<i32>,
}

/// Hexes that are not walkable a band measurement steps over before the
/// band starts: the wall's own footprint.
const WALL_SKIP: u32 = 2;

/// Strips of the same wall within this many hex steps give the wall's line
/// at a strip ([`band_behind`]).
const WALL_LINE_REACH: i32 = 4;

/// `sqrt(3) / 2`: a hex row's height in hex steps.
const ROW: f64 = 0.866_025_403_784_438_6;

/// Map screen space to the regular hex lattice: every hex step is one
/// unit long (`(32, 0)` and `(16, 12)` on screen).
fn lattice(p: [i64; 2]) -> [f64; 2] {
    [p[0] as f64 / 32.0, p[1] as f64 * ROW / 12.0]
}

fn screen_of_lattice(l: [f64; 2]) -> [f64; 2] {
    [l[0] * 32.0, l[1] * 12.0 / ROW]
}

/// The unit normal, pointing away from the camera (up on screen), of the
/// line through the hex centres `points` (lattice space): the principal
/// axis of their spread, turned a quarter. `None` for fewer than two
/// distinct points.
fn behind_normal(points: &[[f64; 2]]) -> Option<[f64; 2]> {
    let n = points.len() as f64;
    if points.len() < 2 {
        return None;
    }
    let mean = points
        .iter()
        .fold([0.0, 0.0], |m, p| [m[0] + p[0] / n, m[1] + p[1] / n]);
    let (mut a, mut b, mut c) = (0.0, 0.0, 0.0);
    for p in points {
        let d = [p[0] - mean[0], p[1] - mean[1]];
        a += d[0] * d[0];
        b += d[0] * d[1];
        c += d[1] * d[1];
    }
    if a + c < 1.0e-9 {
        return None;
    }
    // The larger eigenvalue's eigenvector of [[a, b], [b, c]].
    let half = (a - c) / 2.0;
    let lambda = (a + c) / 2.0 + (half * half + b * b).sqrt();
    let axis = if a >= c {
        [lambda - c, b]
    } else {
        [b, lambda - a]
    };
    let len = (axis[0] * axis[0] + axis[1] * axis[1]).sqrt();
    if len < 1.0e-12 {
        return None;
    }
    let mut normal = [-axis[1] / len, axis[0] / len];
    if normal[1] > 0.0 || (normal[1] == 0.0 && normal[0] < 0.0) {
        normal = [-normal[0], -normal[1]];
    }
    Some(normal)
}

/// Counts the band along a sequence of hexes leaving the strip: past at
/// most [`WALL_SKIP`] hexes outside `walkers`, the walkable hexes up to the
/// next one outside them, at most `cap + 1`. `None` when no walkable hex
/// comes within the skip.
fn count_band(hexes: impl Iterator<Item = i32>, walkers: &BTreeSet<i32>, cap: u32) -> Option<u32> {
    let (mut skipped, mut run) = (0u32, 0u32);
    for h in hexes {
        if walkers.contains(&h) {
            run += 1;
            if run > cap {
                break;
            }
        } else if run == 0 {
            skipped += 1;
            if skipped > WALL_SKIP {
                break;
            }
        } else {
            break;
        }
    }
    (run > 0).then_some(run)
}

/// The walkable band behind the strip on hex `strip` (the `declutter`
/// module, rule 4, narrow walkways), counted to at most `cap + 1` hexes.
///
/// With other strips of the same wall near it (`wall`, hexes within
/// [`WALL_LINE_REACH`] steps), across the wall: from the strip's centre
/// along the normal of their line pointing away from the camera, every
/// hex the line enters. A lone strip measures along both up-screen hex
/// directions and takes the narrower. `None` when no walkable hex lies
/// just behind the strip.
pub(crate) fn band_behind(
    strip: i32,
    wall: &[i32],
    walkers: &BTreeSet<i32>,
    cap: u32,
) -> Option<u32> {
    let centre = hex::center(strip)?;
    let mut points: Vec<[f64; 2]> = vec![lattice(centre)];
    for &h in wall {
        if h != strip
            && hex::distance(h, strip) <= WALL_LINE_REACH
            && let Some(c) = hex::center(h)
        {
            points.push(lattice(c));
        }
    }
    if let Some(normal) = behind_normal(&points) {
        let start = lattice(centre);
        let reach = f64::from(WALL_SKIP + cap + 2);
        let mut seen = BTreeSet::from([strip]);
        let mut along = Vec::new();
        let mut t = 0.0;
        while t <= reach {
            let p = screen_of_lattice([start[0] + normal[0] * t, start[1] + normal[1] * t]);
            match hex::pick(p) {
                Some(h) => {
                    if seen.insert(h) {
                        along.push(h);
                    }
                }
                None => break,
            }
            t += 0.125;
        }
        return count_band(along.into_iter(), walkers, cap);
    }
    // A lone strip: the up-screen hex directions.
    let mut best: Option<u32> = None;
    for (o, first) in hex::neighbours(strip).into_iter().enumerate() {
        if !first
            .and_then(hex::center)
            .is_some_and(|c| c[1] < centre[1])
        {
            continue;
        }
        let steps = std::iter::successors(first, |&h| hex::neighbours(h)[o]);
        if let Some(run) = count_band(steps, walkers, cap) {
            best = Some(best.map_or(run, |b| b.min(run)));
        }
    }
    best
}

/// The height above the foot of the surface each canvas pixel shows
/// (`lift` is the origin's height above the foot).
fn height_of(canvas: &Canvas, projection: &Projection, lift: f32, i: usize) -> Option<f64> {
    let w = canvas.width as usize;
    let d = canvas.depth[i];
    if !d.is_finite() {
        return None;
    }
    let q = [
        canvas.origin[0] as f32 + (i % w) as f32 + 0.5,
        canvas.origin[1] as f32 + (i / w) as f32 + 0.5,
    ];
    let local = projection.unproject(q, d);
    Some(f64::from(local[2]) * f64::from(projection.storey_units()) + f64::from(lift))
}

/// Column by column, from the bottom up: the first pixel higher than
/// `limit` and everything drawn above it. Returns (kept, cleared).
fn crop_columns(
    canvas: &Canvas,
    pixels: &[usize],
    projection: &Projection,
    lift: f32,
    limit: f64,
) -> (Vec<usize>, Vec<usize>) {
    let w = canvas.width as usize;
    let mut columns: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &i in pixels {
        columns.entry(i % w).or_default().push(i);
    }
    let (mut kept, mut cleared) = (Vec::new(), Vec::new());
    for (_, mut column) in columns {
        column.sort_unstable_by(|a, b| b.cmp(a));
        let cut_at = column
            .iter()
            .position(|&i| height_of(canvas, projection, lift, i).is_some_and(|e| e > limit))
            .unwrap_or(column.len());
        cleared.extend_from_slice(&column[cut_at..]);
        kept.extend_from_slice(&column[..cut_at]);
    }
    kept.sort_unstable();
    cleared.sort_unstable();
    (kept, cleared)
}

/// What the door rule makes of one door sprite.
#[derive(Debug, Clone, Default)]
pub(crate) struct DoorCut {
    /// Walkers the whole door hides; none means it stays whole.
    pub before: Vec<i32>,
    /// Canvas pixels cleared (empty: whole).
    pub cleared: Vec<usize>,
    /// Walkers the stub still hides.
    pub after: Vec<i32>,
    /// The stub grew to keep `min_pixels` (a teleport door).
    pub grown: bool,
}

/// The door rule (`declutter` module, rule 4) for a door sprite rendered
/// on `canvas` and anchored at `hex`: when it hides a walker, every column
/// loses its pixels from the first one higher than `stub` above the foot
/// upwards. With `min_pixels`, the stub grows by half until at least that
/// many pixels stay, or the door is whole again.
pub(crate) fn trim_door(
    canvas: &Canvas,
    hex: i32,
    ground: [i64; 2],
    projection: &Projection,
    trim: &Trim<'_>,
    stub: f32,
    min_pixels: Option<usize>,
) -> DoorCut {
    let w = canvas.width as usize;
    let mut out = DoorCut::default();
    if w == 0 {
        return out;
    }
    let pixels: Vec<usize> = (0..w * canvas.height as usize)
        .filter(|&i| canvas.opaque(i))
        .collect();
    let base = [
        ground[0] + i64::from(canvas.origin[0]) + i64::from(trim.offset[0]),
        ground[1] + i64::from(canvas.origin[1]) + i64::from(trim.offset[1]),
    ];
    let at = |i: usize| [base[0] + (i % w) as i64, base[1] + (i / w) as i64];
    out.before = hidden(&pixels, hex, &at, trim, false);
    if out.before.len() <= trim.settings.max_walkers {
        return out;
    }
    let mut limit = f64::from(stub);
    let tallest = pixels
        .iter()
        .filter_map(|&i| height_of(canvas, projection, trim.lift, i))
        .fold(f64::MIN, f64::max);
    loop {
        let (kept, cleared) = crop_columns(canvas, &pixels, projection, trim.lift, limit);
        let short = min_pixels.is_some_and(|m| kept.len() < m);
        if short && limit < tallest {
            limit *= 1.5;
            out.grown = true;
            continue;
        }
        if short || cleared.is_empty() {
            // Whole again: nothing cut.
            out.after = out.before.clone();
            return out;
        }
        out.after = hidden(&kept, hex, &at, trim, false);
        out.cleared = cleared;
        return out;
    }
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
    // Beside a narrow walkway, the lower kerb: the cut strips with a narrow
    // band behind them, and the cut strips with no band at all (a wall's
    // end past the walkway) within [`WALL_LINE_REACH`] (or `spread`) steps
    // of one, so the cut line stays level along the wall.
    let mut narrow: BTreeSet<i32> = BTreeSet::new();
    if trim.settings.narrow_kerb_height.is_some() {
        let wall: Vec<i32> = by_strip.keys().copied().collect();
        let cap = trim.settings.narrow_max_width;
        let mut bandless = Vec::new();
        for &strip in &cut {
            match band_behind(strip, &wall, trim.walkers, cap) {
                Some(w) if w <= cap => {
                    narrow.insert(strip);
                }
                Some(_) => {}
                None => bandless.push(strip),
            }
        }
        let spread = trim.settings.spread.max(WALL_LINE_REACH);
        let follow: Vec<i32> = bandless
            .into_iter()
            .filter(|&s| narrow.iter().any(|&n| hex::distance(s, n) <= spread))
            .collect();
        narrow.extend(follow);
    }
    // Outdoors on the narrow rule alone: the other strips stay whole.
    if trim.narrow_only {
        cut.retain(|s| narrow.contains(s));
    }
    for (strip, pixels) in by_strip {
        let before = hides.remove(&strip).unwrap_or_default();
        if !cut.contains(&strip) {
            out.covered_after.extend(before);
            continue;
        }
        let kerb = match trim.settings.narrow_kerb_height {
            Some(low) if narrow.contains(&strip) => {
                out.narrow += 1;
                f64::from(low)
            }
            _ => kerb,
        };
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
        out.cut_hexes.push(strip);
        if kept.is_empty() {
            out.emptied.push(strip);
        }
        out.covered_after
            .extend(hidden(&kept, strip, &at, trim, false));
    }
    out
}
