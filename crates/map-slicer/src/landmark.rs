//! Landmark pieces: a mesh too large to slice into strips, kept upright in
//! as few scenery pieces as the engine's drawing rules allow.
//!
//! Three engine rules decide the cut (vault-engine, `vault-render`
//! `map/objects.rs`):
//!
//! - **What is drawn.** A refresh draws an object only when its hex is one
//!   of the hexes it visits (`draw_sequence` over `visited_hexes`): a fixed
//!   lattice over the 640 × 380 map view widened by 320 pixels each side and
//!   240 above and below. The object's pixels play no part (the `enhanced`
//!   canvas adds an overscan by sprite extent; the 640 × 480 game view does
//!   not). So a piece is drawn whenever one of its pixels is in view only if
//!   its anchor hex lies within reach of its far edges
//!   (`SlicerConfig::landmark_reach`): a piece is at most
//!   `landmark_piece_max` across, and its anchor is a hex whose centre lies
//!   inside the window that rule leaves.
//! - **Order.** Flat objects first, then ascending hex number, then list
//!   order. A walker on hex `H` is drawn after a piece anchored at `a` when
//!   `a <= H` (a walker is inserted after the scenery).
//! - **What is in front.** As for strips, a pixel owned by hex `o` (its
//!   surface stands over `o`) belongs in front of a walker on `H` when
//!   `o > H`, behind otherwise.
//!
//! For each piece and each walkable hex whose walker box
//! (`SlicerConfig::walker_box`) overlaps it, the box holds `F` pixels that
//! belong in front and `B` behind. With anchor `a` the walker is *drawn
//! over* `F` pixels when `a <= H`, and *hidden* by `B` pixels when `a > H`.
//! The anchor is the lowest-numbered hex of the window above every walker
//! with `F` over the tolerance: nobody is drawn over a wall, and as few as
//! possible are hidden. A piece with no such hex is cut in half
//! (visibility); a piece that hides a walker is cut by owner hex so that
//! the pixels behind the walker leave it (sorting); both until
//! `landmark_max_pieces`. What is left is resolved for the walker: a hex
//! whose walker would still be drawn over pixels in front of it is
//! blocked, and a walker still hidden is counted.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use foundry_render::Projection;

use crate::hex;
use crate::render::Canvas;
use crate::strips::{self, Strip};

/// The engine and walker limits, in map screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// Left, top, right, bottom: how far beyond a view edge a hex centre is
    /// still visited.
    pub reach: [i64; 4],
    /// Largest drawn extent of a piece before sorting cuts.
    pub piece_max: [i64; 2],
    pub max_pieces: usize,
    /// Width and height of a walker's box, bottom centre on its hex centre.
    pub walker: [i64; 2],
    pub tolerance: usize,
}

impl Limits {
    pub fn of(config: &crate::SlicerConfig) -> Self {
        Limits {
            reach: config.landmark_reach.map(i64::from),
            piece_max: config.landmark_piece_max.map(i64::from),
            max_pieces: config.landmark_max_pieces.max(1),
            walker: config.walker_box.map(i64::from),
            tolerance: config.landmark_sort_tolerance,
        }
    }

    /// The rectangle `[x0, y0, x1, y1]` (inclusive) an anchor's hex centre
    /// must lie in for a piece drawn over `rect` (inclusive): visited from
    /// every view that shows one of its pixels.
    pub fn window(&self, rect: [i64; 4]) -> [i64; 4] {
        let [left, top, right, bottom] = self.reach;
        [
            rect[2] - left,
            rect[3] - top,
            rect[0] + right,
            rect[1] + bottom,
        ]
    }

    /// The box of a walker standing on a hex centred at `c`, inclusive.
    pub fn walker_box(&self, c: [i64; 2]) -> [i64; 4] {
        let [w, h] = self.walker;
        let left = c[0] - w / 2;
        [left, c[1] - h + 1, left + w - 1, c[1]]
    }
}

/// What a landmark reference becomes.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Pieces {
    /// One per anchor hex, sorted by hex.
    pub strips: Vec<Strip>,
    /// Opaque pixels whose ground point is off the map.
    pub dropped: u64,
    /// Hexes a walker may not stand on: under the mesh, owning a pixel of a
    /// surface above `step_height`, or sealed (below); open hexes never.
    pub blocked: BTreeSet<i32>,
    pub stats: PieceStats,
}

/// Tallies of one landmark's cut (summed in [`crate::LandmarkStats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PieceStats {
    /// Pieces of the visibility cut alone, before any split.
    pub tiles: usize,
    /// Halvings because no anchor was both visible and late enough.
    pub visibility_splits: usize,
    /// Cuts by owner hex because a piece hid a walker.
    pub sorting_splits: usize,
    /// Walkable hexes whose walker box overlaps the landmark.
    pub walkers: usize,
    /// Walkers still hidden by a piece that stands behind them.
    pub hidden: usize,
    /// Hexes blocked because a piece would still be drawn under a walker
    /// there while standing in front of it.
    pub sealed: usize,
    /// Pieces left without an anchor late enough for every walker (their
    /// walkers are sealed).
    pub unresolved: usize,
}

/// One landmark reference's render and placement.
pub(crate) struct Input<'a> {
    pub canvas: &'a Canvas,
    pub indexed: &'a [u8],
    /// The ground point of the reference's origin, map screen space.
    pub ground: [i64; 2],
    /// The placement displacement: its origin's height on screen.
    pub offset: [i32; 2],
    /// Its origin's height above the terrain, source units.
    pub lift: f32,
    pub projection: &'a Projection,
    /// Hexes under a triangle of the mesh above `step_height`.
    pub under: &'a BTreeSet<i32>,
    /// Hexes the source lets an actor stand on: never blocked.
    pub open: &'a BTreeSet<i32>,
    /// When known, the hexes the map can make walkable at all; others are
    /// no walker's concern. Open hexes are walkers either way.
    pub walkable: Option<&'a BTreeSet<i32>>,
    pub step_height: f32,
}

/// Every hex whose centre lies in `rect` (inclusive), sorted.
pub(crate) fn hexes_in_rect(rect: [i64; 4]) -> Vec<i32> {
    let mut out = Vec::new();
    if rect[0] > rect[2] || rect[1] > rect[3] {
        return out;
    }
    // A centre is about (24 h + 16 r + 16, -6 h + 12 r + 8): row r is
    // (x + 4 y) / 64 up to a few, h follows from x.
    let row_of = |x: i64, y: i64| (x + 4 * y).div_euclid(64);
    let corners = [
        row_of(rect[0], rect[1]),
        row_of(rect[2], rect[1]),
        row_of(rect[0], rect[3]),
        row_of(rect[2], rect[3]),
    ];
    let side = i64::from(hex::HEX_SIDE);
    let r0 = (corners.iter().min().copied().unwrap_or(0) - 2).max(0);
    let r1 = (corners.iter().max().copied().unwrap_or(0) + 2).min(side - 1);
    for r in r0..=r1 {
        let h0 = ((rect[0] - 16 * r - 40).div_euclid(24) - 1).max(0);
        let h1 = ((rect[2] - 16 * r + 8).div_euclid(24) + 1).min(side - 1);
        for h in h0..=h1 {
            let x = (side - 1 - h) + side * r;
            let Ok(x) = i32::try_from(x) else { continue };
            if let Some(c) = hex::center(x)
                && (rect[0]..=rect[2]).contains(&c[0])
                && (rect[1]..=rect[3]).contains(&c[1])
            {
                out.push(x);
            }
        }
    }
    out.sort_unstable();
    out
}

/// One piece while cutting.
struct Piece {
    /// Canvas pixel indices.
    pixels: Vec<u32>,
    /// Drawn extent, inclusive.
    rect: [i64; 4],
    /// `false` once a split could not divide it.
    splittable: bool,
    eval: Option<Eval>,
}

#[derive(Debug, Clone, Default)]
struct Eval {
    anchor: Option<i32>,
    /// Walkers hidden at that anchor, sorted.
    hidden: Vec<i32>,
}

/// The walkers of a piece at one anchor.
#[derive(Debug, Default)]
struct Walkers {
    /// Every walkable hex whose box overlaps the piece.
    all: Vec<i32>,
    /// Those with more than the tolerance in front of them, sorted.
    fronted: Vec<i32>,
    /// Those with more than the tolerance behind them, sorted.
    backed: Vec<i32>,
}

struct Cutter<'a> {
    limits: Limits,
    width: usize,
    /// Map screen position of canvas pixel (0, 0) as drawn.
    base: [i64; 2],
    owner: Vec<i32>,
    piece_of: Vec<u32>,
    blocked: &'a BTreeSet<i32>,
    walkable: Option<&'a BTreeSet<i32>>,
    open: &'a BTreeSet<i32>,
}

impl Cutter<'_> {
    /// Whether a walker may stand on `hex`.
    fn walker(&self, hex: i32) -> bool {
        !self.blocked.contains(&hex)
            && (self.open.contains(&hex) || self.walkable.is_none_or(|w| w.contains(&hex)))
    }

    fn at(&self, index: u32) -> [i64; 2] {
        let i = index as usize;
        [
            self.base[0] + (i % self.width) as i64,
            self.base[1] + (i / self.width) as i64,
        ]
    }

    fn rect(&self, pixels: &[u32]) -> [i64; 4] {
        let mut r = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
        for &p in pixels {
            let [x, y] = self.at(p);
            r = [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)];
        }
        r
    }

    /// The walkers of piece `id` drawn over `rect`.
    fn walkers(&self, id: u32, rect: [i64; 4]) -> Walkers {
        let [w, h] = self.limits.walker;
        let centres = [
            rect[0] - (w - 1) + w / 2,
            rect[1],
            rect[2] + w / 2,
            rect[3] + h - 1,
        ];
        let mut out = Walkers::default();
        let height = self.owner.len() / self.width.max(1);
        for hx in hexes_in_rect(centres) {
            if !self.walker(hx) {
                continue;
            }
            let Some(c) = hex::center(hx) else { continue };
            let b = self.limits.walker_box(c);
            let (x0, x1) = (b[0].max(rect[0]), b[2].min(rect[2]));
            let (y0, y1) = (b[1].max(rect[1]), b[3].min(rect[3]));
            let (mut front, mut back, mut any) = (0usize, 0usize, false);
            for y in y0..=y1 {
                let j = y - self.base[1];
                if j < 0 || j as usize >= height {
                    continue;
                }
                for x in x0..=x1 {
                    let i = x - self.base[0];
                    if i < 0 || i as usize >= self.width {
                        continue;
                    }
                    let index = j as usize * self.width + i as usize;
                    if self.piece_of[index] != id {
                        continue;
                    }
                    any = true;
                    if self.owner[index] > hx {
                        front += 1;
                    } else {
                        back += 1;
                    }
                }
            }
            if !any {
                continue;
            }
            out.all.push(hx);
            if front > self.limits.tolerance {
                out.fronted.push(hx);
            }
            if back > self.limits.tolerance {
                out.backed.push(hx);
            }
        }
        out
    }

    /// The anchor of a piece: the lowest hex of its window above every
    /// walker with pixels in front, or `None`.
    fn evaluate(&self, id: u32, rect: [i64; 4]) -> Eval {
        let walkers = self.walkers(id, rect);
        let late = walkers.fronted.last().copied();
        let anchor = hexes_in_rect(self.limits.window(rect))
            .into_iter()
            .find(|&a| late.is_none_or(|d| a > d));
        let hidden = match anchor {
            Some(a) => walkers.backed.iter().copied().filter(|&h| h < a).collect(),
            None => Vec::new(),
        };
        Eval { anchor, hidden }
    }
}

/// Cuts one landmark reference into pieces: see the module documentation.
pub(crate) fn cut(input: &Input<'_>, limits: &Limits) -> Result<Pieces, String> {
    let canvas = input.canvas;
    let width = canvas.width as usize;
    let owner = strips::owners(canvas, input.ground, input.projection);
    let mut result = Pieces::default();

    // Which hexes a walker may not stand on: under the mesh, or under a
    // visible surface above a step.
    let storey = f64::from(input.projection.storey_units());
    let mut blocked: BTreeSet<i32> = input.under.clone();
    let mut pixels: Vec<u32> = Vec::new();
    for (index, &h) in owner.iter().enumerate() {
        if h < 0 {
            if canvas.opaque(index) {
                result.dropped += 1;
            }
            continue;
        }
        pixels.push(index as u32);
        let d = canvas.depth[index];
        if d.is_finite() {
            let q = [
                f64::from(canvas.origin[0]) + (index % width) as f64 + 0.5,
                f64::from(canvas.origin[1]) + (index / width) as f64 + 0.5,
            ];
            let e = input.projection.unproject([q[0] as f32, q[1] as f32], d)[2];
            if f64::from(e) * storey + f64::from(input.lift) > f64::from(input.step_height) {
                blocked.insert(h);
            }
        }
    }
    blocked.retain(|h| !input.open.contains(h));
    if pixels.is_empty() {
        result.blocked = blocked;
        return Ok(result);
    }

    let base = [
        input.ground[0] + i64::from(input.offset[0]) + i64::from(canvas.origin[0]),
        input.ground[1] + i64::from(input.offset[1]) + i64::from(canvas.origin[1]),
    ];
    let mut cutter = Cutter {
        limits: *limits,
        width,
        base,
        owner,
        piece_of: vec![u32::MAX; width * canvas.height as usize],
        blocked: &blocked,
        walkable: input.walkable,
        open: input.open,
    };

    // The visibility cut: even columns of the drawn extent no wider than
    // `piece_max`, each in even slabs of its own height.
    let all = cutter.rect(&pixels);
    let [max_w, max_h] = limits.piece_max.map(|v| v.max(1));
    // `n` even parts of at most `max` over `lo..=hi`, and their step.
    let span = |lo: i64, hi: i64, max: i64| {
        let len = (hi - lo + 1).max(1);
        let n = (len + max - 1) / max;
        (n, (len + n - 1) / n)
    };
    let (columns, column_w) = span(all[0], all[2], max_w);
    let mut by_column: Vec<Vec<u32>> = vec![Vec::new(); columns as usize];
    for &p in &pixels {
        let c = ((cutter.at(p)[0] - all[0]) / column_w).min(columns - 1);
        by_column[c as usize].push(p);
    }
    let mut pieces: Vec<Piece> = Vec::new();
    for column in by_column.into_iter().filter(|c| !c.is_empty()) {
        let r = cutter.rect(&column);
        let (slabs, slab_h) = span(r[1], r[3], max_h);
        let mut by_slab: Vec<Vec<u32>> = vec![Vec::new(); slabs as usize];
        for &p in &column {
            let s = ((cutter.at(p)[1] - r[1]) / slab_h).min(slabs - 1);
            by_slab[s as usize].push(p);
        }
        for slab in by_slab.into_iter().filter(|s| !s.is_empty()) {
            let rect = cutter.rect(&slab);
            pieces.push(Piece {
                pixels: slab,
                rect,
                splittable: true,
                eval: None,
            });
        }
    }
    for (id, piece) in pieces.iter().enumerate() {
        for &p in &piece.pixels {
            cutter.piece_of[p as usize] = id as u32;
        }
    }
    result.stats.tiles = pieces.len();

    // Refine: the worst piece first (no anchor, then most walkers hidden,
    // then most pixels, then first made), until none is wrong or the cap.
    loop {
        for (id, piece) in pieces.iter_mut().enumerate() {
            if piece.eval.is_none() {
                piece.eval = Some(cutter.evaluate(id as u32, piece.rect));
            }
        }
        let worst = pieces
            .iter()
            .enumerate()
            .filter(|(_, p)| p.splittable)
            .filter_map(|(id, p)| {
                let e = p.eval.as_ref()?;
                (e.anchor.is_none() || !e.hidden.is_empty()).then_some((
                    e.anchor.is_none(),
                    e.hidden.len(),
                    p.pixels.len(),
                    Reverse(id),
                ))
            })
            .max();
        let Some((_, _, _, Reverse(id))) = worst else {
            break;
        };
        if pieces.len() >= limits.max_pieces {
            break;
        }
        let eval = pieces[id].eval.clone().unwrap_or_default();
        let piece = &pieces[id];
        // Sorting: the pixels at or behind the last hidden walker leave.
        let by_owner = eval.hidden.last().and_then(|&t| {
            let (behind, front): (Vec<u32>, Vec<u32>) = piece
                .pixels
                .iter()
                .partition(|&&p| cutter.owner[p as usize] <= t);
            (!behind.is_empty() && !front.is_empty()).then_some((behind, front))
        });
        let parts = match by_owner {
            Some(parts) => {
                result.stats.sorting_splits += 1;
                Some(parts)
            }
            None => {
                let r = piece.rect;
                let (w, h) = (r[2] - r[0] + 1, r[3] - r[1] + 1);
                let split = if h >= 2 && 4 * h >= w {
                    let mid = r[1] + h / 2;
                    Some(
                        piece
                            .pixels
                            .iter()
                            .partition::<Vec<u32>, _>(|&&p| cutter.at(p)[1] < mid),
                    )
                } else if w >= 2 {
                    let mid = r[0] + w / 2;
                    Some(
                        piece
                            .pixels
                            .iter()
                            .partition::<Vec<u32>, _>(|&&p| cutter.at(p)[0] < mid),
                    )
                } else {
                    None
                };
                if split.is_some() {
                    if eval.anchor.is_none() {
                        result.stats.visibility_splits += 1;
                    } else {
                        result.stats.sorting_splits += 1;
                    }
                }
                split.filter(|(a, b)| !a.is_empty() && !b.is_empty())
            }
        };
        let Some((first, second)) = parts else {
            pieces[id].splittable = false;
            continue;
        };
        let new_id = pieces.len() as u32;
        for &p in &second {
            cutter.piece_of[p as usize] = new_id;
        }
        let (r1, r2) = (cutter.rect(&first), cutter.rect(&second));
        pieces[id] = Piece {
            pixels: first,
            rect: r1,
            splittable: true,
            eval: None,
        };
        pieces.push(Piece {
            pixels: second,
            rect: r2,
            splittable: true,
            eval: None,
        });
    }

    // Final anchors. A piece still without one takes its window's highest
    // hex (drawn after the most walkers); a walker drawn over pixels in
    // front of it is sealed, one still hidden is counted.
    let mut sealed: BTreeSet<i32> = BTreeSet::new();
    let mut hidden: BTreeSet<i32> = BTreeSet::new();
    let mut walkers: BTreeSet<i32> = BTreeSet::new();
    let mut anchors: Vec<i32> = Vec::with_capacity(pieces.len());
    for (id, piece) in pieces.iter().enumerate() {
        let eval = piece.eval.clone().unwrap_or_default();
        let window = limits.window(piece.rect);
        let anchor = match eval.anchor {
            Some(a) => a,
            None => {
                result.stats.unresolved += 1;
                match hexes_in_rect(window).last() {
                    Some(&a) => a,
                    None => nearest_hex(window)
                        .ok_or_else(|| format!("no hex near piece {:?}", piece.rect))?,
                }
            }
        };
        let w = cutter.walkers(id as u32, piece.rect);
        walkers.extend(w.all.iter().copied());
        sealed.extend(w.fronted.iter().copied().filter(|&h| h >= anchor));
        hidden.extend(w.backed.iter().copied().filter(|&h| h < anchor));
        anchors.push(anchor);
    }
    sealed.retain(|h| !input.open.contains(h));
    hidden.retain(|h| !sealed.contains(h));
    walkers.retain(|h| !sealed.contains(h));
    result.stats.walkers = walkers.len();
    result.stats.hidden = hidden.len();
    result.stats.sealed = sealed.len();

    let mut placed = vec![-1i32; cutter.owner.len()];
    for (piece, &anchor) in pieces.iter().zip(&anchors) {
        for &p in &piece.pixels {
            placed[p as usize] = anchor;
        }
    }
    result.strips = strips::frames(
        canvas,
        input.indexed,
        &placed,
        input.ground,
        &BTreeMap::new(),
    )?;
    blocked.extend(sealed);
    result.blocked = blocked;
    Ok(result)
}

/// The hex under the centre of `rect`, if any.
fn nearest_hex(rect: [i64; 4]) -> Option<i32> {
    hex::pick([
        (rect[0] + rect[2]) as f64 / 2.0,
        (rect[1] + rect[3]) as f64 / 2.0,
    ])
}

/// Scenery entries a landmark of this drawn size is cut into by the
/// visibility cut alone (before sorting cuts): an estimate.
pub(crate) fn estimate(size: [f64; 2], limits: &Limits) -> usize {
    let [w, h] = limits.piece_max.map(|v| v.max(1) as f64);
    let n = (size[0] / w).ceil().max(1.0) * (size[1] / h).ceil().max(1.0);
    (n as usize).clamp(1, limits.max_pieces)
}
