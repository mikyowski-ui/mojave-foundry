//! The bounded tile library: exact deduplication, then vector quantization.
//!
//! **Exact.** Identical tiles (byte for byte) collapse to one; the library
//! keeps them in order of first use.
//!
//! **Phase classes.** Squares whose tiles see the land textures at the same
//! offset (the texture repeat is a whole number of squares, see
//! `TerrainOptions::snap_repeat_to_tiles`) and the dither at the same phase
//! form a class. Quantization replaces a tile only by a tile of its own
//! class, so a substitute shows the same texture detail at the same place
//! and the seams between neighbours are tint steps, not texture breaks. The
//! cap is shared among the classes in proportion to their distinct tiles.
//!
//! **Vector quantization**, only when more distinct tiles remain than the
//! cap. Each distinct tile becomes a feature vector: the mean Oklab colour
//! (of the palette display colours) of each 8 × 6 block of the tile box,
//! over the pixels of the parallelogram, each block weighted by the square
//! root of its share of those pixels, so that squared feature distance is
//! the pixel-weighted mean squared difference of block means. Tiles are
//! weighted by how many squares use them. Clustering is weighted k-means:
//! k-means++ seeding from a fixed-seed generator, then a fixed number of
//! Lloyd iterations. Each cluster is represented by its medoid in feature
//! space (a real tile, so pixels stay crisp). Every distinct tile then
//! picks, among its [`CANDIDATES`] nearest medoids of its class in feature
//! space, the one with the smallest full-resolution error.
//!
//! **Error.** The substitution error of a square is the mean Oklab distance
//! between the display colours of its own pixels and its library tile's
//! pixels, over the parallelogram.
//!
//! Every reduction runs in a fixed order; parallel work only computes
//! independent per-item values, so the result never depends on `jobs`.

use quantize::PaletteMap;

use crate::TILE_WIDTH;
use crate::parallel;

/// Block size of the feature thumbnail.
const BLOCK_W: usize = 8;
const BLOCK_H: usize = 6;
/// Medoids each tile compares at full resolution.
pub(crate) const CANDIDATES: usize = 8;
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
    let mut best: Vec<(f32, usize)> = Vec::with_capacity(n + 1);
    for (i, c) in centres.iter().enumerate() {
        let d = dist2(point, c);
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

/// Builds the library for the tiles of every square, in square order.
///
/// `mask` lists the pixels of the parallelogram; `cap` is at least 1.
/// `class_of[q]` is square `q`'s phase class (texture repeat and dither
/// phase): when quantizing, a tile is only ever replaced by a tile of its
/// own class, so a substitute shows the same texture detail at the same
/// place and neighbouring tiles still meet. When `cap` is smaller than the
/// number of classes, the classes are ignored.
///
/// **Seams.** Each square then chooses among its candidates (its tile's
/// [`CANDIDATES`] nearest medoids of the class) by its own error plus
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
    let sizes: Vec<usize> = members.iter().map(Vec::len).collect();
    let budget = allocate(&sizes, cap);

    let table = distance_table(map);
    // For every distinct tile: its candidates (medoid, as an index into
    // `distinct`; error), best own error first.
    let mut candidates: Vec<Vec<(usize, f64)>> = vec![Vec::new(); distinct.len()];
    for (list, &k) in members.iter().zip(&budget) {
        let local = cluster(&points, list, &weights, k, iterations, jobs);
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
                        tile_error(&table, mask, own, &tiles[distinct[local[m]]])
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

    // Seam-aware choice per square.
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
}
