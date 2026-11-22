//! The bounded tile library: exact deduplication, then reduction to the cap.
//!
//! **Exact.** Identical tiles (byte for byte) collapse to one; the library
//! keeps them in order of first use.
//!
//! **Phase classes.** Squares whose tiles see the land textures at the same
//! offset (the texture repeat is a whole number of squares, see
//! `TerrainOptions::snap_repeat_to_tiles`) and the dither at the same phase
//! form a class. Reduction replaces a tile only by a tile of its own class,
//! so a substitute shows the same texture detail at the same place and the
//! seams between neighbours are tint steps, not texture breaks.
//!
//! **Features**, only when more distinct tiles remain than the cap. Each
//! distinct tile becomes a feature vector (its thumbnail): the mean Oklab
//! colour (of the palette display colours) of each 8 × 6 block of the tile
//! box, over the pixels of the parallelogram, each block weighted by the
//! square root of its share of those pixels, so that squared feature
//! distance is the pixel-weighted mean squared difference of block means.
//! Tiles are weighted by how many squares use them.
//!
//! **K-means** ([`LibraryMethod::KMeans`], the default). The cap is shared
//! among the classes in proportion to their distinct tiles. Clustering is
//! weighted k-means per class: k-means++ seeding from a fixed-seed
//! generator, then a fixed number of Lloyd iterations. Each cluster is
//! represented by its medoid in feature space (a real tile, so pixels stay
//! crisp). Every distinct tile then takes its [`CANDIDATES`] nearest medoids
//! of its class in feature space as candidates, by full-resolution error.
//!
//! **Greedy merge** ([`LibraryMethod::GreedyMerge`]). Every distinct tile
//! starts in the library, serving itself. Each tile knows its
//! [`MERGE_NEIGHBOURS`] nearest tiles of its class in feature space and
//! what showing each would cost: the full-resolution error plus
//! [`MERGE_THUMBNAIL_WEIGHT`] times the thumbnail distance. The library tile
//! whose removal adds least cost (each tile it serves moving to its
//! cheapest remaining library tile, weighted by squares) is removed, until
//! the cap is met. Removal costs only grow as the library shrinks, so a
//! lazily recomputed queue finds the smallest. A tile whose near tiles are
//! all gone falls back to the nearest remaining tiles of its class (where
//! costs may, rarely, fall a little: the merge is then near greedy). The last tile of a
//! class is never removed. The candidates are then the tile's cheapest
//! remaining near tiles and its assigned one, by full-resolution error.
//!
//! [`LibraryMethod::KMeans`]: crate::LibraryMethod::KMeans
//! [`LibraryMethod::GreedyMerge`]: crate::LibraryMethod::GreedyMerge
//!
//! **Error.** The substitution error of a square is the mean Oklab distance
//! between the display colours of its own pixels and its library tile's
//! pixels, over the parallelogram.
//!
//! Every reduction runs in a fixed order; parallel work only computes
//! independent per-item values, so the result never depends on `jobs`.

use quantize::PaletteMap;

use crate::parallel;
use crate::{LibraryMethod, TILE_WIDTH};

/// Block size of the feature thumbnail.
const BLOCK_W: usize = 8;
const BLOCK_H: usize = 6;
/// Medoids each tile compares at full resolution.
pub(crate) const CANDIDATES: usize = 8;
/// Tiles of its class each tile compares at full resolution in the greedy
/// merge, nearest in feature space.
const MERGE_NEIGHBOURS: usize = 32;
/// Weight, in the greedy merge's cost, of the thumbnail distance (block
/// means) next to the full-resolution error. The per-pixel error spreads a
/// small, local difference (a sliver of road, the edge of a rock) thin and
/// counts dither noise as error; the thumbnail distance keeps the first
/// and ignores the second.
const MERGE_THUMBNAIL_WEIGHT: f64 = 1.0;
/// Passes of the seam-aware substitution.
const SEAM_PASSES: usize = 4;
/// Square-index offsets of the four neighbours, in side order; side `i`'s
/// opposite is `i ^ 1`.
pub(crate) const SIDES: [i32; 4] = [1, -1, 100, -100];

/// The pixels of a tile box that belong to its square, and the ones along
/// each side (touching the neighbour at [`SIDES`]).
#[derive(Debug, Clone)]
pub(crate) struct Shape {
    pub mask: Vec<usize>,
    pub edges: [Vec<usize>; 4],
}

/// Mean Oklab colour of a tile along each side.
fn edge_means(shape: &Shape, labs: &[[f32; 3]; 256], tile: &[u8]) -> [[f32; 3]; 4] {
    shape.edges.clone().map(|side| {
        let mut sum = [0.0f32; 3];
        for &p in &side {
            let lab = labs[usize::from(tile[p])];
            for k in 0..3 {
                sum[k] += lab[k];
            }
        }
        sum.map(|v| v / side.len().max(1) as f32)
    })
}

/// The neighbour of square `q` across side `side`, if on the map.
fn neighbour(q: usize, side: usize) -> Option<usize> {
    let column = q % 100;
    let ok = match side {
        0 => column != 99,
        1 => column != 0,
        2 => q + 100 < 10_000,
        _ => q >= 100,
    };
    ok.then(|| (q as i64 + i64::from(SIDES[side])) as usize)
        .filter(|&n| n < 10_000)
}
/// Clusters larger than this take the member nearest the centroid instead
/// of the exact medoid.
const EXACT_MEDOID_LIMIT: usize = 1024;
/// Seed of the k-means++ generator.
const SEED: u64 = 0x6d61_702d_7465_7272;

/// What [`build`] made.
#[derive(Debug, Clone)]
pub(crate) struct Library {
    pub tiles: Vec<Vec<u8>>,
    pub squares: Vec<u16>,
    pub distinct: usize,
    pub clusters: usize,
    pub substituted: usize,
    pub mean_error: f64,
    pub worst_error: f64,
}

/// SplitMix64: a small, portable, fixed-seed generator.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn dist2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// Oklab distance between the display colours of every index pair.
fn distance_table(map: &PaletteMap) -> Vec<f32> {
    let mut table = vec![0.0f32; 256 * 256];
    for a in 0..256usize {
        let la = map.oklab(a as u8);
        for b in 0..256usize {
            let lb = map.oklab(b as u8);
            let d: f64 = (0..3).map(|k| (la[k] - lb[k]) * (la[k] - lb[k])).sum();
            table[a * 256 + b] = d.sqrt() as f32;
        }
    }
    table
}

/// Mean distance over the mask pixels.
fn tile_error(table: &[f32], mask: &[usize], a: &[u8], b: &[u8]) -> f64 {
    if mask.is_empty() {
        return 0.0;
    }
    let sum: f64 = mask
        .iter()
        .map(|&p| f64::from(table[usize::from(a[p]) * 256 + usize::from(b[p])]))
        .sum();
    sum / mask.len() as f64
}

/// Feature extraction for one tile.
struct Features {
    /// `(block, pixel)` for every mask pixel.
    blocks: Vec<(usize, usize)>,
    /// Pixels per non-empty block, and its weight.
    weights: Vec<f32>,
    counts: Vec<usize>,
    labs: [[f32; 3]; 256],
}

impl Features {
    fn new(mask: &[usize], map: &PaletteMap) -> Self {
        let width = TILE_WIDTH as usize;
        let columns = width.div_ceil(BLOCK_W);
        let mut raw: Vec<(usize, usize)> = mask
            .iter()
            .map(|&p| ((p / width / BLOCK_H) * columns + (p % width) / BLOCK_W, p))
            .collect();
        // Renumber the non-empty blocks densely, in block order.
        let mut ids: Vec<usize> = raw.iter().map(|(b, _)| *b).collect();
        ids.sort_unstable();
        ids.dedup();
        for entry in &mut raw {
            entry.0 = ids.binary_search(&entry.0).unwrap_or(0);
        }
        let mut counts = vec![0usize; ids.len()];
        for (b, _) in &raw {
            counts[*b] += 1;
        }
        let total = mask.len().max(1) as f32;
        let weights = counts.iter().map(|&c| (c as f32 / total).sqrt()).collect();
        let labs = std::array::from_fn(|i| map.oklab(i as u8).map(|v| v as f32));
        Features {
            blocks: raw,
            weights,
            counts,
            labs,
        }
    }

    fn dimension(&self) -> usize {
        self.counts.len() * 3
    }

    fn of(&self, tile: &[u8]) -> Vec<f32> {
        let mut sums = vec![0.0f32; self.dimension()];
        for &(b, p) in &self.blocks {
            let lab = self.labs[usize::from(tile[p])];
            for k in 0..3 {
                sums[b * 3 + k] += lab[k];
            }
        }
        for (b, (&count, &weight)) in self.counts.iter().zip(&self.weights).enumerate() {
            let scale = weight / count.max(1) as f32;
            for k in 0..3 {
                sums[b * 3 + k] *= scale;
            }
        }
        sums
    }
}

/// The `n` nearest centres to `point`, nearest first, ties to the lowest
/// index.
fn nearest(point: &[f32], centres: &[Vec<f32>], n: usize) -> Vec<usize> {
    nearest_by(centres.len(), n, |i| dist2(point, &centres[i]))
}

/// The `n` items of `0..count` with the smallest `distance`, smallest
/// first, ties to the lowest index.
fn nearest_by(count: usize, n: usize, distance: impl Fn(usize) -> f32) -> Vec<usize> {
    let mut best: Vec<(f32, usize)> = Vec::with_capacity(n + 1);
    for i in 0..count {
        let d = distance(i);
        if best.len() < n || d < best[best.len() - 1].0 {
            let at = best.partition_point(|(bd, _)| *bd <= d);
            best.insert(at, (d, i));
            best.truncate(n);
        }
    }
    best.into_iter().map(|(_, i)| i).collect()
}

/// Shares `cap` among classes of `sizes` tiles: every non-empty class gets
/// at least one, the rest goes in proportion to size (largest remainder,
/// ties to the lower class), and no class gets more than it has. Requires
/// `cap` at least the number of non-empty classes.
fn allocate(sizes: &[usize], cap: usize) -> Vec<usize> {
    let mut out: Vec<usize> = sizes.iter().map(|&n| usize::from(n > 0)).collect();
    let used: usize = out.iter().sum();
    let spare = cap.saturating_sub(used);
    let extra: usize = sizes.iter().map(|&n| n.saturating_sub(1)).sum();
    if extra == 0 {
        return out;
    }
    let mut fractions = Vec::new();
    for (c, &n) in sizes.iter().enumerate() {
        let share = (spare as u128 * n.saturating_sub(1) as u128) / extra as u128;
        let rem = (spare as u128 * n.saturating_sub(1) as u128) % extra as u128;
        out[c] += (share as usize).min(n.saturating_sub(1));
        fractions.push((rem, c));
    }
    fractions.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut left = cap.saturating_sub(out.iter().sum());
    while left > 0 {
        let mut gave = false;
        for &(_, c) in &fractions {
            if left > 0 && out[c] < sizes[c] {
                out[c] += 1;
                left -= 1;
                gave = true;
            }
        }
        if !gave {
            break;
        }
    }
    out
}

/// Weighted k-means over `points` (indices into `all`), returning the
/// medoids as indices into `all`, ascending.
fn cluster(
    all: &[Vec<f32>],
    points: &[usize],
    weights: &[f64],
    k: usize,
    iterations: u32,
    jobs: usize,
) -> Vec<usize> {
    if points.is_empty() || k == 0 {
        return Vec::new();
    }
    if points.len() <= k {
        return points.to_vec();
    }
    let point = |i: usize| &all[points[i]];
    let weight = |i: usize| weights[points[i]];
    let n = points.len();

    // k-means++ seeding.
    let mut rng = SplitMix(SEED);
    let pick = |rng: &mut SplitMix, score: &dyn Fn(usize) -> f64| -> Option<usize> {
        let total: f64 = (0..n).map(score).sum();
        if !(total > 0.0 && total.is_finite()) {
            return None;
        }
        let target = rng.unit() * total;
        let mut acc = 0.0;
        for i in 0..n {
            acc += score(i);
            if acc > target {
                return Some(i);
            }
        }
        (0..n).rev().find(|&i| score(i) > 0.0)
    };
    let first = pick(&mut rng, &weight).unwrap_or(0);
    let mut seeds = vec![first];
    let mut nearest2: Vec<f32> = parallel::map(n, jobs, |i| dist2(point(i), point(first)));
    while seeds.len() < k {
        let Some(next) = pick(&mut rng, &|i| weight(i) * f64::from(nearest2[i])) else {
            break;
        };
        seeds.push(next);
        nearest2 = parallel::map(n, jobs, |i| nearest2[i].min(dist2(point(i), point(next))));
    }
    let mut centres: Vec<Vec<f32>> = seeds.iter().map(|&s| point(s).clone()).collect();

    // Lloyd iterations; the last assignment is against the final centres.
    let dimension = all[0].len();
    let mut assignment: Vec<usize> = Vec::new();
    for round in 0..=iterations {
        assignment = parallel::map(n, jobs, |i| {
            nearest(point(i), &centres, 1).first().copied().unwrap_or(0)
        });
        if round == iterations {
            break;
        }
        let mut sums = vec![vec![0.0f64; dimension]; centres.len()];
        let mut mass = vec![0.0f64; centres.len()];
        for (i, &c) in assignment.iter().enumerate() {
            mass[c] += weight(i);
            for (s, &v) in sums[c].iter_mut().zip(point(i)) {
                *s += f64::from(v) * weight(i);
            }
        }
        for (c, centre) in centres.iter_mut().enumerate() {
            if mass[c] > 0.0 {
                for (v, s) in centre.iter_mut().zip(&sums[c]) {
                    *v = (s / mass[c]) as f32;
                }
            }
        }
    }

    // Medoids.
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); centres.len()];
    for (i, &c) in assignment.iter().enumerate() {
        members[c].push(i);
    }
    let medoid_of: Vec<Option<usize>> = parallel::map(centres.len(), jobs, |c| {
        let list = &members[c];
        let best = |costs: Vec<(f64, usize)>| {
            costs
                .into_iter()
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                .map(|(_, i)| i)
        };
        if list.len() > EXACT_MEDOID_LIMIT {
            return best(
                list.iter()
                    .map(|&i| (f64::from(dist2(point(i), &centres[c])), i))
                    .collect(),
            );
        }
        best(
            list.iter()
                .map(|&i| {
                    let cost = list
                        .iter()
                        .map(|&j| weight(j) * f64::from(dist2(point(i), point(j)).sqrt()))
                        .sum();
                    (cost, i)
                })
                .collect(),
        )
    });
    let mut medoids: Vec<usize> = medoid_of.into_iter().flatten().map(|i| points[i]).collect();
    medoids.sort_unstable();
    medoids.dedup();
    medoids
}

/// A cost with a total order, for the merge queue.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cost(f64);

impl Eq for Cost {}

impl PartialOrd for Cost {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cost {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// Greedy merging (see [`LibraryMethod::GreedyMerge`]): returns, for every
/// distinct tile, its candidates (library tile, error), best error first.
///
/// `members` are the classes (indices into `points`), each non-empty class
/// keeps at least one tile, so `cap` must be at least the number of
/// non-empty classes to be met; `error_of(d, c)` is the full-resolution
/// error of showing tile `c` in place of tile `d`.
///
/// [`LibraryMethod::GreedyMerge`]: crate::LibraryMethod::GreedyMerge
fn greedy_merge(
    points: &[Vec<f32>],
    weights: &[f64],
    members: &[Vec<usize>],
    cap: usize,
    error_of: &(dyn Fn(usize, usize) -> f64 + Sync),
    jobs: usize,
) -> Vec<Vec<(usize, f64)>> {
    let n = points.len();
    let mut class = vec![0usize; n];
    for (c, list) in members.iter().enumerate() {
        for &d in list {
            class[d] = c;
        }
    }
    let by_error = |a: &(usize, f64), b: &(usize, f64)| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0));
    // (tile, merge cost, error) of showing tile `c` for tile `d`.
    let entry = |d: usize, c: usize| -> (usize, f64, f64) {
        if c == d {
            return (c, 0.0, 0.0);
        }
        let error = error_of(d, c);
        let thumbnail = f64::from(dist2(&points[d], &points[c])).sqrt();
        (c, error + MERGE_THUMBNAIL_WEIGHT * thumbnail, error)
    };
    let by_cost =
        |a: &(usize, f64, f64), b: &(usize, f64, f64)| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0));
    // Each tile's nearest tiles of its class in feature space (itself
    // included, at cost 0), cheapest first.
    let neighbours: Vec<Vec<(usize, f64, f64)>> = parallel::map(n, jobs, |d| {
        let list = &members[class[d]];
        let near = nearest_by(list.len(), MERGE_NEIGHBOURS, |i| {
            if list[i] == d {
                -1.0
            } else {
                dist2(&points[d], &points[list[i]])
            }
        });
        let mut found: Vec<(usize, f64, f64)> =
            near.into_iter().map(|i| entry(d, list[i])).collect();
        found.sort_by(by_cost);
        found
    });

    let mut in_library = vec![true; n];
    let mut assigned: Vec<(usize, f64, f64)> = (0..n).map(|d| (d, 0.0, 0.0)).collect();
    let mut users: Vec<Vec<usize>> = (0..n).map(|d| vec![d]).collect();

    // The cheapest library tile for `q` other than `without`.
    let alternative = |in_library: &[bool], q: usize, without: usize| {
        if let Some(&found) = neighbours[q]
            .iter()
            .find(|(c, _, _)| *c != without && in_library[*c])
        {
            return Some(found);
        }
        // Every near tile is gone: the nearest of the rest in feature
        // space, by merge cost.
        let rest: Vec<usize> = members[class[q]]
            .iter()
            .copied()
            .filter(|&c| c != without && in_library[c])
            .collect();
        nearest_by(rest.len(), CANDIDATES, |i| {
            dist2(&points[q], &points[rest[i]])
        })
        .into_iter()
        .map(|i| entry(q, rest[i]))
        .min_by(by_cost)
    };
    // What removing `t` from the library adds to the total merge cost.
    let removal_cost =
        |in_library: &[bool], users: &[Vec<usize>], assigned: &[(usize, f64, f64)], t: usize| {
            let mut sum = 0.0f64;
            for &q in &users[t] {
                match alternative(in_library, q, t) {
                    Some((_, cost, _)) => sum += weights[q] * (cost - assigned[q].1),
                    None => return f64::INFINITY,
                }
            }
            sum
        };

    let mut queue: std::collections::BinaryHeap<std::cmp::Reverse<(Cost, usize)>> = (0..n)
        .map(|t| {
            let cost = removal_cost(&in_library, &users, &assigned, t);
            std::cmp::Reverse((Cost(cost), t))
        })
        .collect();
    let mut remaining = n;
    // Costs only grow as the library shrinks, so a recomputed cost that is
    // still the smallest is the true smallest (lazy greedy).
    while remaining > cap {
        let Some(std::cmp::Reverse((_, t))) = queue.pop() else {
            break;
        };
        if !in_library[t] {
            continue;
        }
        let now = removal_cost(&in_library, &users, &assigned, t);
        if let Some(std::cmp::Reverse(next)) = queue.peek()
            && (Cost(now), t) > *next
        {
            queue.push(std::cmp::Reverse((Cost(now), t)));
            continue;
        }
        if !now.is_finite() {
            break;
        }
        in_library[t] = false;
        remaining -= 1;
        for q in std::mem::take(&mut users[t]) {
            if let Some(found) = alternative(&in_library, q, t) {
                assigned[q] = found;
                users[found.0].push(q);
            }
        }
    }

    (0..n)
        .map(|d| {
            let mut found: Vec<(usize, f64)> = neighbours[d]
                .iter()
                .filter(|(c, _, _)| in_library[*c])
                .take(CANDIDATES)
                .map(|&(c, _, error)| (c, error))
                .collect();
            let (c, _, error) = assigned[d];
            if !found.iter().any(|&(f, _)| f == c) {
                found.push((c, error));
            }
            found.sort_by(by_error);
            found
        })
        .collect()
}

/// Builds the library for the tiles of every square, in square order.
///
/// `mask` lists the pixels of the parallelogram; `cap` is at least 1.
/// `class_of[q]` is square `q`'s phase class (texture repeat and dither
/// phase): when reducing, a tile is only ever replaced by a tile of its
/// own class, so a substitute shows the same texture detail at the same
/// place and neighbouring tiles still meet. When `cap` is smaller than the
/// number of classes, the classes are ignored. `method` picks the
/// reduction (see the module documentation).
///
/// **Seams.** Each square then chooses among its candidates (library tiles
/// of its class, at most [`CANDIDATES`] or one more) by its own error plus
/// `seam_weight` times, summed over its neighbours, how far the colour step
/// across the shared edge (mean Oklab along each side) moves from the step
/// between the original tiles. Squares are visited in index order for
/// [`SEAM_PASSES`] passes, each seeing the choices made so far.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    tiles: Vec<Vec<u8>>,
    class_of: &[usize],
    shape: &Shape,
    cap: usize,
    method: LibraryMethod,
    iterations: u32,
    seam_weight: f32,
    map: &PaletteMap,
    jobs: usize,
) -> Library {
    let mask = &shape.mask;
    // Exact duplicates.
    let mut order: Vec<usize> = (0..tiles.len()).collect();
    order.sort_by(|&a, &b| tiles[a].cmp(&tiles[b]).then(a.cmp(&b)));
    let mut group_of = vec![0usize; tiles.len()];
    let mut representatives: Vec<usize> = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        if k == 0 || tiles[order[k - 1]] != tiles[i] {
            representatives.push(i);
        }
        group_of[i] = representatives.len() - 1;
    }
    // Distinct tiles in order of first use.
    let mut by_first: Vec<usize> = (0..representatives.len()).collect();
    by_first.sort_by_key(|&g| representatives[g]);
    let mut rank = vec![0usize; representatives.len()];
    for (r, &g) in by_first.iter().enumerate() {
        rank[g] = r;
    }
    let distinct_of: Vec<usize> = group_of.iter().map(|&g| rank[g]).collect();
    let distinct: Vec<usize> = by_first.iter().map(|&g| representatives[g]).collect();
    let mut counts = vec![0usize; distinct.len()];
    for &d in &distinct_of {
        counts[d] += 1;
    }

    if distinct.len() <= cap {
        let squares = distinct_of.iter().map(|&d| d as u16).collect();
        let library = distinct.iter().map(|&i| tiles[i].clone()).collect();
        return Library {
            tiles: library,
            squares,
            distinct: distinct.len(),
            clusters: 0,
            substituted: 0,
            mean_error: 0.0,
            worst_error: 0.0,
        };
    }

    let features = Features::new(mask, map);
    let points: Vec<Vec<f32>> =
        parallel::map(distinct.len(), jobs, |d| features.of(&tiles[distinct[d]]));
    let weights: Vec<f64> = counts.iter().map(|&c| c as f64).collect();

    // Classes: a distinct tile belongs to the class of its first square.
    let classes = class_of.iter().copied().max().map_or(1, |m| m + 1);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); classes];
    for (d, &first) in distinct.iter().enumerate() {
        members[class_of.get(first).copied().unwrap_or(0)].push(d);
    }
    let non_empty = members.iter().filter(|m| !m.is_empty()).count();
    if cap < non_empty {
        members = vec![(0..distinct.len()).collect()];
    }

    let table = distance_table(map);
    // For every distinct tile: its candidates (library tile, as an index
    // into `distinct`; error), best own error first.
    let candidates = match method {
        LibraryMethod::KMeans => kmeans_candidates(
            &tiles, &distinct, &points, &weights, &members, cap, iterations, &table, mask, jobs,
        ),
        LibraryMethod::GreedyMerge => {
            let error_of = |d: usize, c: usize| {
                tile_error(&table, mask, &tiles[distinct[d]], &tiles[distinct[c]])
            };
            greedy_merge(&points, &weights, &members, cap, &error_of, jobs)
        }
    };
    seam_choice_and_order(
        &tiles,
        &distinct,
        &distinct_of,
        &candidates,
        shape,
        seam_weight,
        map,
        jobs,
    )
}

/// [`LibraryMethod::KMeans`]: the cap is shared among the classes in
/// proportion to their distinct tiles, each class is clustered, and every
/// tile takes its [`CANDIDATES`] nearest medoids as candidates.
#[allow(clippy::too_many_arguments)]
fn kmeans_candidates(
    tiles: &[Vec<u8>],
    distinct: &[usize],
    points: &[Vec<f32>],
    weights: &[f64],
    members: &[Vec<usize>],
    cap: usize,
    iterations: u32,
    table: &[f32],
    mask: &[usize],
    jobs: usize,
) -> Vec<Vec<(usize, f64)>> {
    let sizes: Vec<usize> = members.iter().map(Vec::len).collect();
    let budget = allocate(&sizes, cap);
    let mut candidates: Vec<Vec<(usize, f64)>> = vec![Vec::new(); distinct.len()];
    for (list, &k) in members.iter().zip(&budget) {
        let local = cluster(points, list, weights, k, iterations, jobs);
        let local_points: Vec<Vec<f32>> = local.iter().map(|&m| points[m].clone()).collect();
        let picked: Vec<Vec<(usize, f64)>> = parallel::map(list.len(), jobs, |i| {
            let d = list[i];
            let own = &tiles[distinct[d]];
            let mut found: Vec<(usize, f64)> = nearest(&points[d], &local_points, CANDIDATES)
                .into_iter()
                .map(|m| {
                    let error = if local[m] == d {
                        0.0
                    } else {
                        tile_error(table, mask, own, &tiles[distinct[local[m]]])
                    };
                    (local[m], error)
                })
                .collect();
            found.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            found
        });
        for (&d, found) in list.iter().zip(picked) {
            candidates[d] = found;
        }
    }
    candidates
}

/// The seam-aware choice per square among its tile's candidates, then the
/// library in order of first use.
#[allow(clippy::too_many_arguments)]
fn seam_choice_and_order(
    tiles: &[Vec<u8>],
    distinct: &[usize],
    distinct_of: &[usize],
    candidates: &[Vec<(usize, f64)>],
    shape: &Shape,
    seam_weight: f32,
    map: &PaletteMap,
    jobs: usize,
) -> Library {
    let labs: [[f32; 3]; 256] = std::array::from_fn(|i| map.oklab(i as u8).map(|v| v as f32));
    let edges: Vec<[[f32; 3]; 4]> = parallel::map(distinct.len(), jobs, |d| {
        edge_means(shape, &labs, &tiles[distinct[d]])
    });
    let mut chosen: Vec<usize> = distinct_of
        .iter()
        .map(|&d| candidates[d].first().map_or(d, |c| c.0))
        .collect();
    let squares_n = distinct_of.len();
    if seam_weight > 0.0 {
        let step = |a: &[f32; 3], b: &[f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        for _ in 0..SEAM_PASSES {
            for q in 0..squares_n {
                let list = &candidates[distinct_of[q]];
                if list.len() < 2 {
                    continue;
                }
                let original = distinct_of[q];
                let mut best = (f64::INFINITY, chosen[q]);
                for &(c, error) in list {
                    let mut seam = 0.0f64;
                    for side in 0..4 {
                        let Some(n) = neighbour(q, side).filter(|&n| n < squares_n) else {
                            continue;
                        };
                        let opposite = side ^ 1;
                        let now = step(&edges[c][side], &edges[chosen[n]][opposite]);
                        let was = step(&edges[original][side], &edges[distinct_of[n]][opposite]);
                        let d: f32 = (0..3).map(|k| (now[k] - was[k]).powi(2)).sum();
                        seam += f64::from(d.sqrt());
                    }
                    let cost = error + f64::from(seam_weight) * seam;
                    if cost < best.0 {
                        best = (cost, c);
                    }
                }
                chosen[q] = best.1;
            }
        }
    }

    // Library order is order of first use.
    let mut slot = vec![usize::MAX; distinct.len()];
    let mut order = Vec::new();
    let mut sum = 0.0;
    let mut worst = 0.0f64;
    let mut substituted = 0;
    let squares = (0..squares_n)
        .map(|q| {
            let d = distinct_of[q];
            let m = chosen[q];
            let error = candidates[d].iter().find(|c| c.0 == m).map_or(0.0, |c| c.1);
            if m != d {
                substituted += 1;
            }
            sum += error;
            worst = worst.max(error);
            if slot[m] == usize::MAX {
                slot[m] = order.len();
                order.push(m);
            }
            slot[m] as u16
        })
        .collect();
    Library {
        tiles: order.iter().map(|&m| tiles[distinct[m]].clone()).collect(),
        squares,
        distinct: distinct.len(),
        clusters: order.len(),
        substituted,
        mean_error: sum / distinct_of.len().max(1) as f64,
        worst_error: worst,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_fixed() {
        let mut a = SplitMix(SEED);
        let mut b = SplitMix(SEED);
        for _ in 0..10 {
            let x = a.unit();
            assert!((0.0..1.0).contains(&x));
            assert_eq!(x, b.unit());
        }
    }

    #[test]
    fn the_cap_is_shared_in_proportion() {
        assert_eq!(allocate(&[10, 10], 4), vec![2, 2]);
        assert_eq!(allocate(&[30, 10, 0], 9), vec![6, 3, 0]);
        assert_eq!(allocate(&[1, 100], 10), vec![1, 9]);
        assert_eq!(allocate(&[2, 2], 100), vec![2, 2]);
        let shares = allocate(&[7, 5, 3, 1], 11);
        assert_eq!(shares.iter().sum::<usize>(), 11);
        assert!(
            shares
                .iter()
                .zip([7, 5, 3, 1])
                .all(|(&a, n)| a >= 1 && a <= n)
        );
    }

    #[test]
    fn nearest_orders_by_distance_then_index() {
        let centres = vec![vec![0.0], vec![2.0], vec![1.0], vec![1.0]];
        assert_eq!(nearest(&[1.1], &centres, 3), vec![2, 3, 1]);
        assert_eq!(nearest(&[-5.0], &centres, 1), vec![0]);
    }

    /// Greedy merge over 1-D "tiles" whose error is their distance.
    fn merge_1d(values: &[f32], classes: &[usize], cap: usize) -> Vec<Vec<(usize, f64)>> {
        let points: Vec<Vec<f32>> = values.iter().map(|&v| vec![v]).collect();
        let weights = vec![1.0; values.len()];
        let count = classes.iter().copied().max().map_or(1, |m| m + 1);
        let mut members = vec![Vec::new(); count];
        for (d, &c) in classes.iter().enumerate() {
            members[c].push(d);
        }
        let error_of = |d: usize, c: usize| f64::from((values[d] - values[c]).abs());
        greedy_merge(&points, &weights, &members, cap, &error_of, 1)
    }

    #[test]
    fn greedy_merge_keeps_the_distinctive_tile() {
        let values = [0.0, 0.1, 0.2, 10.0, 10.1, 50.0];
        let candidates = merge_1d(&values, &[0; 6], 3);
        let library: std::collections::BTreeSet<usize> =
            candidates.iter().map(|c| c[0].0).collect();
        assert_eq!(library.len(), 3, "{candidates:?}");
        // The lone tile stays; each tight group keeps one of its own.
        assert!(library.contains(&5));
        assert_eq!(library.iter().filter(|&&t| t < 3).count(), 1);
        assert_eq!(library.iter().filter(|&&t| (3..5).contains(&t)).count(), 1);
        // A library tile serves itself, and every candidate is a library
        // tile, best error first.
        for (d, list) in candidates.iter().enumerate() {
            if library.contains(&d) {
                assert_eq!(list[0], (d, 0.0));
            }
            assert!(list.iter().all(|(c, _)| library.contains(c)));
            assert!(list.windows(2).all(|w| w[0].1 <= w[1].1));
        }
    }

    #[test]
    fn greedy_merge_stays_inside_classes_and_spends_the_cap_where_it_saves_most() {
        // Class 0 is tight, class 1 spread out: the cap goes to class 1.
        let values = [0.0, 0.01, 0.02, 0.03, 0.0, 5.0, 10.0, 15.0];
        let classes = [0, 0, 0, 0, 1, 1, 1, 1];
        let candidates = merge_1d(&values, &classes, 5);
        let library: std::collections::BTreeSet<usize> =
            candidates.iter().map(|c| c[0].0).collect();
        assert_eq!(library.len(), 5);
        assert_eq!(library.iter().filter(|&&t| t < 4).count(), 1);
        assert!((4..8).all(|t| library.contains(&t)));
        for (d, list) in candidates.iter().enumerate() {
            assert!(list.iter().all(|(c, _)| classes[*c] == classes[d]));
        }
        // At a cap of one per class, every class still keeps one.
        let tight = merge_1d(&values, &classes, 2);
        let library: std::collections::BTreeSet<usize> = tight.iter().map(|c| c[0].0).collect();
        assert_eq!(library.len(), 2);
        assert_eq!(library.iter().filter(|&&t| t < 4).count(), 1);
    }

    #[test]
    fn greedy_merge_falls_back_beyond_its_neighbours() {
        // More tiles than MERGE_NEIGHBOURS in one class and a cap of one:
        // most tiles' near neighbours are all gone, and they still find
        // the survivor.
        let values: Vec<f32> = (0..MERGE_NEIGHBOURS * 3).map(|i| i as f32).collect();
        let candidates = merge_1d(&values, &vec![0; values.len()], 1);
        let survivor = candidates[0][0].0;
        assert!(
            candidates
                .iter()
                .all(|c| c.len() == 1 && c[0].0 == survivor)
        );
    }
}
