//! Cutting one rendered object into per-hex strips.
//!
//! Every opaque pixel is unprojected through the depth buffer to its
//! surface point, dropped to the ground (`e = 0`), projected again and
//! picked: that hex owns the pixel. Then, in order:
//!
//! 1. **Off the map.** A pixel whose ground point picks no hex is dropped
//!    and counted. Nothing of the object is drawn there anyway: the engine
//!    has no hex to anchor it to.
//! 2. **Tiny strips.** A hex owning fewer than `min_strip_pixels` pixels
//!    gives them to the nearest hex (lattice distance, then lowest number)
//!    that owns at least that many, so a speck never costs an art entry.
//!    When no hex of the object reaches the threshold the largest one keeps
//!    everything.
//! 3. **Runs.** Consecutive hexes of one hex row merge into one strip
//!    anchored at the middle hex of the run, when every one of them is
//!    blocked by the object (so nothing can stand inside the run). The
//!    engine draws in ascending hex number, which is row-major, so every
//!    hex outside the run draws before or after the whole run exactly as it
//!    did before every one of its hexes: the merge changes no occlusion
//!    against anything standing outside it. A hex whose six neighbours all
//!    own pixels of the object is interior (under a roof); interior runs
//!    merge whole. Perimeter runs merge in pieces of at most
//!    `max_perimeter_run` hexes, so walls stay narrow pieces. The run's
//!    other hexes are returned so the caller can block them.
//!
//! Pixels whose ground point lies outside the object's footprint (eaves,
//! a leaning pole) stay with the hex they stand over; the caller decides
//! whether such a strip blocks.

use std::collections::BTreeMap;

use emit::PlacedFrame;
use foundry_render::Projection;

use crate::hex;
use crate::render::Canvas;

/// One strip, before deduplication.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Strip {
    /// The owning hex: where the engine anchors the strip.
    pub hex: i32,
    /// For a merged interior run, its first and last hex (same row).
    pub run: Option<(i32, i32)>,
    pub frame: PlacedFrame,
    pub pixels: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Cut {
    /// Sorted by hex.
    pub strips: Vec<Strip>,
    /// Opaque pixels whose ground point is off the map.
    pub dropped: u64,
    /// Hexes whose pixels went to a neighbour as too few.
    pub merged_tiny: usize,
    /// Interior hexes folded into a run's strip.
    pub folded: usize,
    /// Hexes of merged runs other than the anchor hex.
    pub run_hexes: Vec<i32>,
}

/// The hex each opaque pixel stands over, `-1` for transparent pixels and
/// pixels off the map. `ground` is the object origin's ground point in map
/// screen space; the canvas is canonical (origin at screen `(0, 0)` on the
/// ground).
pub(crate) fn owners(canvas: &Canvas, ground: [i64; 2], projection: &Projection) -> Vec<i32> {
    let w = canvas.width as usize;
    let mut out = vec![-1i32; w * canvas.height as usize];
    for (index, owner) in out.iter_mut().enumerate() {
        if !canvas.opaque(index) {
            continue;
        }
        let q = [
            f64::from(canvas.origin[0]) + (index % w) as f64 + 0.5,
            f64::from(canvas.origin[1]) + (index / w) as f64 + 0.5,
        ];
        let d = canvas.depth[index];
        let foot = if d.is_finite() {
            let local = projection.unproject([q[0] as f32, q[1] as f32], d);
            let g = projection.screen([local[0], local[1], 0.0]);
            [f64::from(g[0]), f64::from(g[1])]
        } else {
            q
        };
        if let Some(h) = hex::pick([foot[0] + ground[0] as f64, foot[1] + ground[1] as f64]) {
            *owner = h;
        }
    }
    out
}

/// How [`cut`] merges.
pub(crate) struct CutOptions<'a> {
    pub min_pixels: usize,
    pub merge_interior: bool,
    /// Longest merged run of perimeter hexes; 1 or less merges none.
    pub max_perimeter_run: usize,
    /// Whether a hex is blocked by the object; only blocked hexes merge.
    pub blocks: &'a dyn Fn(i32) -> bool,
}

/// What a hex is to the run merge.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Interior,
    Perimeter,
    /// Not blocked: something may stand there, so it never merges.
    Loose,
}

/// Strips of a canonical canvas whose origin's ground point is `ground`.
pub(crate) fn cut(
    canvas: &Canvas,
    indexed: &[u8],
    ground: [i64; 2],
    projection: &Projection,
    options: &CutOptions<'_>,
) -> Result<Cut, String> {
    cut_with(canvas, indexed, ground, projection, options, &mut |_| {})
}

/// [`cut`], with `trim` given the final owner of every pixel (`-1` for
/// none) before the frames are made: it may clear owners (set `-1`) to
/// crop strips, as the foreground cutaway does.
pub(crate) fn cut_with(
    canvas: &Canvas,
    indexed: &[u8],
    ground: [i64; 2],
    projection: &Projection,
    options: &CutOptions<'_>,
    trim: &mut dyn FnMut(&mut [i32]),
) -> Result<Cut, String> {
    let mut owner = owners(canvas, ground, projection);
    let mut result = Cut::default();
    let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
    for (index, &h) in owner.iter().enumerate() {
        if h >= 0 {
            *counts.entry(h).or_insert(0) += 1;
        } else if canvas.opaque(index) {
            result.dropped += 1;
        }
    }
    if counts.is_empty() {
        return Ok(result);
    }

    // Tiny strips go to the nearest strip that is large enough.
    let mut remap: BTreeMap<i32, i32> = BTreeMap::new();
    let mut large: Vec<i32> = counts
        .iter()
        .filter(|(_, n)| **n >= options.min_pixels)
        .map(|(h, _)| *h)
        .collect();
    if large.is_empty() {
        let keep = counts
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(h, _)| *h)
            .unwrap_or(-1);
        large.push(keep);
    }
    for (&h, _) in counts
        .iter()
        .filter(|(h, _)| large.binary_search(h).is_err())
    {
        let target = large
            .iter()
            .copied()
            .min_by_key(|&l| (hex::distance(h, l), l))
            .unwrap_or(h);
        remap.insert(h, target);
        result.merged_tiny += 1;
    }

    // Runs along a row merge: interior runs whole, perimeter runs in
    // pieces of at most `max_perimeter_run`, both only over blocked hexes.
    let owned: std::collections::BTreeSet<i32> = large.iter().copied().collect();
    let kind = |h: i32| {
        if !(options.blocks)(h) {
            Kind::Loose
        } else if hex::neighbours(h)
            .iter()
            .all(|n| n.is_some_and(|n| owned.contains(&n)))
        {
            Kind::Interior
        } else {
            Kind::Perimeter
        }
    };
    let kinds: Vec<Kind> = large.iter().map(|&h| kind(h)).collect();
    let mut i = 0;
    while i < large.len() {
        // A maximal run of consecutive hexes of one row and one kind.
        let (row, _) = hex::row_column(large[i]);
        let mut j = i;
        while j + 1 < large.len()
            && large[j + 1] == large[j] + 1
            && hex::row_column(large[j + 1]).0 == row
            && kinds[j + 1] == kinds[i]
        {
            j += 1;
        }
        let piece = match kinds[i] {
            Kind::Interior if options.merge_interior => j - i + 1,
            Kind::Perimeter => options.max_perimeter_run.max(1),
            _ => 1,
        };
        let mut start = i;
        while start <= j {
            let end = (start + piece - 1).min(j);
            if end > start {
                let anchor = large[start + (end - start) / 2];
                for &h in &large[start..=end] {
                    if h != anchor {
                        remap.insert(h, anchor);
                        result.run_hexes.push(h);
                        result.folded += 1;
                    }
                }
            }
            start = end + 1;
        }
        i = j + 1;
    }
    // A tiny strip remapped into a hex that then joined a run follows it.
    let snapshot = remap.clone();
    for target in remap.values_mut() {
        if let Some(next) = snapshot.get(target) {
            *target = *next;
        }
    }
    let mut runs: BTreeMap<i32, (i32, i32)> = BTreeMap::new();
    for &h in &result.run_hexes {
        if let Some(&anchor) = remap.get(&h) {
            let entry = runs.entry(anchor).or_insert((anchor, anchor));
            entry.0 = entry.0.min(h);
            entry.1 = entry.1.max(h);
        }
    }
    for h in &mut owner {
        if let Some(&target) = remap.get(h) {
            *h = target;
        }
    }
    trim(&mut owner);
    result.strips = frames(canvas, indexed, &owner, ground, &runs)?;
    result.run_hexes.sort_unstable();
    Ok(result)
}

/// One frame per owner hex: the bounding box of its pixels, placed for the
/// engine's rule at that hex. Sorted by hex.
pub(crate) fn frames(
    canvas: &Canvas,
    indexed: &[u8],
    owner: &[i32],
    ground: [i64; 2],
    runs: &BTreeMap<i32, (i32, i32)>,
) -> Result<Vec<Strip>, String> {
    let mut strips = Vec::new();
    // Bounding boxes, then the pixels of each strip.
    let w = canvas.width as usize;
    let mut boxes: BTreeMap<i32, ([usize; 4], usize)> = BTreeMap::new();
    for (index, &h) in owner.iter().enumerate() {
        if h < 0 {
            continue;
        }
        let (x, y) = (index % w, index / w);
        let entry = boxes.entry(h).or_insert(([x, y, x, y], 0));
        entry.0 = [
            entry.0[0].min(x),
            entry.0[1].min(y),
            entry.0[2].max(x),
            entry.0[3].max(y),
        ];
        entry.1 += 1;
    }
    for (h, ([x0, y0, x1, y1], pixels)) in boxes {
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut sub = vec![0u8; bw * bh];
        for y in y0..=y1 {
            for x in x0..=x1 {
                let index = y * w + x;
                if owner[index] == h {
                    sub[(y - y0) * bw + (x - x0)] = indexed[index];
                }
            }
        }
        let centre = hex::center(h).ok_or_else(|| format!("hex {h} has no centre"))?;
        let origin = [
            ground[0] + i64::from(canvas.origin[0]) + x0 as i64 - centre[0],
            ground[1] + i64::from(canvas.origin[1]) + y0 as i64 - centre[1],
        ];
        let origin = [
            i32::try_from(origin[0]).map_err(|_| "strip origin overflows".to_owned())?,
            i32::try_from(origin[1]).map_err(|_| "strip origin overflows".to_owned())?,
        ];
        let frame = emit::placed_frame(&sub, bw as u32, bh as u32, origin)
            .map_err(|e| format!("strip at hex {h}: {e}"))?;
        strips.push(Strip {
            hex: h,
            run: runs.get(&h).copied(),
            frame,
            pixels,
        });
    }
    Ok(strips)
}
