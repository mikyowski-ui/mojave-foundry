//! Link cues: navigation affordances in the floor art
//! ([`crate::BuildOptions::mark_links`], off by default).
//!
//! Exit grids are engine objects, drawn by the engine its own way (and
//! often not at all). A player looking at a busy floor cannot tell where a
//! doorway or a stair begins. With link cues on, every link hex gets a
//! small, in-palette mark painted into the floor tile under it:
//!
//! - **Doorways** ([`CueKind::Door`]): the exit-grid hexes of every door
//!   link and of every stub door, and the hexes those doors' travellers
//!   arrive on here. The hexes of one kind form a region; its border, an
//!   inset frame about two pixels wide at the sides and one at the top and
//!   bottom, like the engine's own hex cursor, is blended towards a warm
//!   amber ([`LinkCues::strength`] of the way, 0.45 by default), so that it
//!   reads as the floor's own mark and never as the cursor.
//! - **Stairs** ([`CueKind::Stairs`]): the exit-grid hexes of every stair
//!   link, and their arrival hexes here. Treads: stripes along the hex rows,
//!   about 1.6 pixels thick every 4 pixels across them (as steps or a
//!   grating read from above), are blended towards a cool light grey. No
//!   frame, so the two never read alike.
//!
//! Arrival hexes take their kind's mark at 60 % of its strength. A pixel's
//! new colour is the blend of its palette colour and the cue colour, taken
//! back to the nearest allowed palette entry (squared RGB distance, the
//! lowest index on a tie; never index 0, a colour-cycling entry or the
//! pulsing red), so the result stays in the 256-colour palette and never
//! animates. Transparent pixels and pixels of another square are left
//! alone.
//!
//! **Budget.** A marked square is a new floor tile unless its result equals
//! a tile the elevation's library already holds or another marked square's
//! (identical results share one tile). Hexes are taken in priority order
//! (door grids, stair grids, door arrivals, stair arrivals; by hex number
//! within each): a hex whose squares would take the map past
//! [`LinkCues::max_tiles`] new tiles, or the layer's tile list past its
//! 4,096 entries, is left unmarked and counted as skipped for budget. The
//! square under a stub door is never repainted (the stub map's floor is
//! that square's tile). Blank squares are never painted.
//!
//! Nothing else changes: walkability, blocking, objects and their order
//! are the same with cues on or off, and with cues off the layer is
//! byte-identical. Each elevation with link hexes reports a note "link
//! cues: N hexes marked, M tiles added, K skipped for budget".

use std::collections::{BTreeMap, BTreeSet};

use vault_formats::HexGrid;
use vault_formats::palette::{CYCLE_RANGES, RED_PULSE_INDEX, TRANSPARENT_INDEX};

use crate::budget::ART_TILES;
use crate::layer::{LayerBuilder, TILE_HEIGHT, TILE_WIDTH};
use crate::plan::MapPlan;
use crate::{BLANK_SQUARE, ElevationInput, GroundTiles, LinkKind};

/// The default of [`LinkCues::max_tiles`].
pub const DEFAULT_MAX_TILES: usize = 512;

/// The default of [`LinkCues::strength`]: how far a doorway's frame is
/// blended towards its colour.
pub const DEFAULT_STRENGTH: f64 = 0.45;

/// The link-cue settings ([`crate::BuildOptions::mark_links`]).
#[derive(Debug, Clone, PartialEq)]
pub struct LinkCues {
    /// The palette entries a cue may produce: `(index, display colour)`,
    /// the colour as `vault_formats::Palette::rgba8` gives it. Entries that
    /// are never drawn as they are (index 0, the colour-cycling ranges, the
    /// pulsing red) are left out by [`LinkCues::new`].
    pub palette: Vec<(u8, [u8; 3])>,
    /// New floor tiles one map may add (default [`DEFAULT_MAX_TILES`]).
    pub max_tiles: usize,
    /// How far a doorway's frame is blended towards its warm colour, 0 to 1
    /// (default [`DEFAULT_STRENGTH`]); stair treads keep their own.
    pub strength: f64,
}

impl LinkCues {
    /// Cues over these palette entries, with the default tile cap.
    pub fn new(palette: Vec<(u8, [u8; 3])>) -> Self {
        LinkCues {
            palette: palette.into_iter().filter(|(i, _)| usable(*i)).collect(),
            max_tiles: DEFAULT_MAX_TILES,
            strength: DEFAULT_STRENGTH,
        }
    }

    /// Cues over a Fallout 2 palette: every mapped entry a cue may use.
    pub fn from_palette(palette: &vault_formats::Palette) -> Self {
        let rgba = palette.rgba8();
        LinkCues::new(
            (0..=255u8)
                .filter(|&i| palette.is_mapped(i))
                .map(|i| {
                    let c = rgba[usize::from(i)];
                    (i, [c[0], c[1], c[2]])
                })
                .collect(),
        )
    }
}

/// Whether a cue may produce palette index `index`.
pub fn usable(index: u8) -> bool {
    index != TRANSPARENT_INDEX
        && usize::from(index) != RED_PULSE_INDEX
        && !CYCLE_RANGES
            .iter()
            .any(|r| (r.first..=r.last).contains(&index))
}

/// What a link hex leads through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CueKind {
    Door,
    Stairs,
}

impl From<LinkKind> for CueKind {
    fn from(kind: LinkKind) -> Self {
        match kind {
            LinkKind::Door => CueKind::Door,
            LinkKind::Stairs => CueKind::Stairs,
        }
    }
}

/// One hex to mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cue {
    pub hex: i32,
    pub kind: CueKind,
    /// An arrival hex (a lighter mark), not an exit grid.
    pub arrival: bool,
}

/// What [`mark_ground`] made of one elevation's ground.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedGround {
    /// The library, then the new tiles in order of their first square.
    pub tiles: Vec<Vec<u8>>,
    /// The 10,000 squares, the marked ones pointing at their tile.
    pub squares: Vec<u16>,
    /// Hexes marked.
    pub marked: usize,
    /// New tiles (`tiles.len()` less the library's).
    pub added: usize,
    /// Hexes left unmarked for the budget.
    pub skipped: usize,
}

impl MarkedGround {
    /// Whether any square changed.
    pub fn changed(&self, ground: &GroundTiles<'_>) -> bool {
        self.squares.as_slice() != ground.squares
    }
}

const GRID: HexGrid = HexGrid { origin: [0, 0] };

/// A cue's look: the colour blended towards and how far.
#[derive(Clone, Copy)]
struct Style {
    target: [u8; 3],
    alpha: f64,
}

/// A doorway's colour: a warm amber, distinct from the engine's cursor
/// colours; its alpha is [`LinkCues::strength`].
const DOOR_STYLE: Style = Style {
    target: [255, 204, 120],
    alpha: DEFAULT_STRENGTH,
};
const STAIRS_STYLE: Style = Style {
    target: [208, 222, 238],
    alpha: 0.5,
};
/// Arrival hexes take their kind's mark at this share of its strength.
const ARRIVAL_SHARE: f64 = 0.6;

/// Index to index, one table per (kind, arrival).
struct Tables {
    tables: BTreeMap<(CueKind, bool), [u8; 256]>,
}

impl Tables {
    fn new(cues: &LinkCues) -> Self {
        let colour: BTreeMap<u8, [u8; 3]> = cues.palette.iter().copied().collect();
        let nearest = |c: [f64; 3]| -> Option<u8> {
            let mut best: Option<(f64, u8)> = None;
            for &(i, p) in &cues.palette {
                let d: f64 = (0..3).map(|k| (f64::from(p[k]) - c[k]).powi(2)).sum();
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, i));
                }
            }
            best.map(|(_, i)| i)
        };
        let mut tables = BTreeMap::new();
        for kind in [CueKind::Door, CueKind::Stairs] {
            for arrival in [false, true] {
                let style = match kind {
                    CueKind::Door => Style {
                        alpha: cues.strength.clamp(0.0, 1.0),
                        ..DOOR_STYLE
                    },
                    CueKind::Stairs => STAIRS_STYLE,
                };
                let alpha = style.alpha * if arrival { ARRIVAL_SHARE } else { 1.0 };
                let table: [u8; 256] = std::array::from_fn(|i| {
                    let i = i as u8;
                    let Some(c) = colour.get(&i) else {
                        return i;
                    };
                    let blend: [f64; 3] = std::array::from_fn(|k| {
                        let (from, to) = (f64::from(c[k]), f64::from(style.target[k]));
                        from + (to - from) * alpha
                    });
                    nearest(blend).unwrap_or(i)
                });
                tables.insert((kind, arrival), table);
            }
        }
        Tables { tables }
    }

    fn get(&self, cue: (CueKind, bool)) -> &[u8; 256] {
        &self.tables[&cue]
    }
}

fn pick_hex(p: [i64; 2]) -> Option<i32> {
    let x = i32::try_from(p[0]).ok()?;
    let y = i32::try_from(p[1]).ok()?;
    GRID.pick_hex([x, y])
}

fn pick_square(p: [i64; 2]) -> Option<i32> {
    let x = i32::try_from(p[0]).ok()?;
    let y = i32::try_from(p[1]).ok()?;
    GRID.pick_square([x, y], false)
}

/// The squares holding any pixel of `hex`.
pub fn squares_of_hex(hex: i32) -> BTreeSet<i32> {
    let mut out = BTreeSet::new();
    let Ok(c) = GRID.hex_center(hex) else {
        return out;
    };
    for y in c[1] - 9..=c[1] + 9 {
        for x in c[0] - 17..=c[0] + 17 {
            if pick_hex([x, y]) == Some(hex)
                && let Some(q) = pick_square([x, y])
            {
                out.insert(q);
            }
        }
    }
    out
}

/// Whether screen pixel `p` lies on a stair tread (module notes): lines
/// `3 x + 4 y = c` run along the hex rows, 5 units of `c` to a pixel across.
pub fn stair_tread(p: [i64; 2]) -> bool {
    (3 * p[0] + 4 * p[1]).rem_euclid(20) < 8
}

/// Whether the mark of a hex covers screen pixel `p` (module notes).
fn paints(mark: (CueKind, bool), p: [i64; 2], marks: &BTreeMap<i32, (CueKind, bool)>) -> bool {
    match mark.0 {
        CueKind::Door => [[-2, 0], [2, 0], [0, -1], [0, 1]].iter().any(|d| {
            pick_hex([p[0] + d[0], p[1] + d[1]]).and_then(|h| marks.get(&h)) != Some(&mark)
        }),
        CueKind::Stairs => stair_tread(p),
    }
}

/// Square `q`'s tile `tile` with every mark in `marks` painted on it.
fn paint(q: i32, tile: &[u8], marks: &BTreeMap<i32, (CueKind, bool)>, tables: &Tables) -> Vec<u8> {
    let mut out = tile.to_vec();
    let Some(corner) = GRID.square_corner(q, false) else {
        return out;
    };
    for y in 0..TILE_HEIGHT {
        for x in 0..TILE_WIDTH {
            let i = y * TILE_WIDTH + x;
            let v = out[i];
            if v == TRANSPARENT_INDEX {
                continue;
            }
            let p = [corner[0] + x as i64, corner[1] + y as i64];
            if pick_square(p) != Some(q) {
                continue;
            }
            let Some(mark) = pick_hex(p).and_then(|h| marks.get(&h)) else {
                continue;
            };
            if paints(*mark, p, marks) {
                out[i] = tables.get(*mark)[usize::from(v)];
            }
        }
    }
    out
}

/// Marks `cues` into one elevation's ground (module notes). `keep` names
/// squares never repainted; at most `room` new tiles are added.
pub fn mark_ground(
    ground: GroundTiles<'_>,
    cues: &[Cue],
    keep: &BTreeSet<i32>,
    options: &LinkCues,
    room: usize,
) -> MarkedGround {
    let tables = Tables::new(options);
    let library: BTreeMap<&[u8], u16> = ground
        .tiles
        .iter()
        .enumerate()
        .rev()
        .map(|(i, t)| (t.as_slice(), i as u16))
        .collect();
    // Priority order, one cue per hex.
    let mut order: Vec<Cue> = cues.to_vec();
    order.sort_by_key(|c| (c.arrival, c.kind, c.hex));
    let mut seen = BTreeSet::new();
    order.retain(|c| seen.insert(c.hex));

    let original = |q: i32| -> Option<&[u8]> {
        let index = *ground.squares.get(usize::try_from(q).ok()?)?;
        if index == BLANK_SQUARE {
            return None;
        }
        ground.tiles.get(usize::from(index)).map(Vec::as_slice)
    };
    let mut marks: BTreeMap<i32, (CueKind, bool)> = BTreeMap::new();
    // Squares painted to a tile the library does not hold, and the count
    // of squares on each such tile.
    let mut painted: BTreeMap<i32, Vec<u8>> = BTreeMap::new();
    let mut fresh: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    let (mut marked, mut skipped) = (0usize, 0usize);
    for cue in order {
        let squares: Vec<i32> = squares_of_hex(cue.hex)
            .into_iter()
            .filter(|q| !keep.contains(q) && original(*q).is_some())
            .collect();
        if squares.is_empty() {
            continue;
        }
        marks.insert(cue.hex, (cue.kind, cue.arrival));
        let mut undo: Vec<(i32, Option<Vec<u8>>)> = Vec::new();
        let mut visible = false;
        for &q in &squares {
            let Some(tile) = original(q) else { continue };
            let result = paint(q, tile, &marks, &tables);
            visible |= result.as_slice() != tile;
            let before = painted.remove(&q);
            if let Some(old) = &before
                && let Some(n) = fresh.get_mut(old)
            {
                *n -= 1;
                if *n == 0 {
                    fresh.remove(old);
                }
            }
            undo.push((q, before));
            if !library.contains_key(result.as_slice()) {
                *fresh.entry(result.clone()).or_insert(0) += 1;
                painted.insert(q, result);
            }
        }
        if fresh.len() <= room.min(options.max_tiles) {
            // A hex whose mark falls on no pixel (a sliver of a square,
            // inside a frame) is kept for its neighbours' frames but not
            // counted.
            marked += usize::from(visible);
            continue;
        }
        // Over the budget: back to where it was.
        marks.remove(&cue.hex);
        skipped += 1;
        for (q, before) in undo.into_iter().rev() {
            if let Some(now) = painted.remove(&q)
                && let Some(n) = fresh.get_mut(&now)
            {
                *n -= 1;
                if *n == 0 {
                    fresh.remove(&now);
                }
            }
            if let Some(old) = before {
                *fresh.entry(old.clone()).or_insert(0) += 1;
                painted.insert(q, old);
            }
        }
    }
    // Repaint every square a kept mark touches: the library hits too.
    let mut tiles = ground.tiles.to_vec();
    let mut squares = ground.squares.to_vec();
    let mut added: BTreeMap<Vec<u8>, u16> = BTreeMap::new();
    let touched: BTreeSet<i32> = marks
        .keys()
        .flat_map(|&h| squares_of_hex(h))
        .filter(|q| !keep.contains(q))
        .collect();
    for q in touched {
        let Some(tile) = original(q) else { continue };
        let result = paint(q, tile, &marks, &tables);
        let index = match library.get(result.as_slice()) {
            Some(&i) => i,
            None => match added.get(&result) {
                Some(&i) => i,
                None => {
                    let i = tiles.len() as u16;
                    tiles.push(result.clone());
                    added.insert(result, i);
                    i
                }
            },
        };
        squares[q as usize] = index;
    }
    MarkedGround {
        tiles,
        squares,
        marked,
        added: added.len(),
        skipped,
    }
}

/// Floor tile list entries the layer can still take: the cap less the
/// list's kept entries, its padding and every distinct library tile of the
/// plans (counted as the layer's own budget check counts them).
fn tile_room(plans: &[MapPlan<'_>], builder: &LayerBuilder) -> usize {
    let mut seen: BTreeSet<&[u8]> = BTreeSet::new();
    let mut in_use = 0usize;
    for plan in plans {
        for elevation in &plan.elevations {
            let mut group = Vec::new();
            for tile in elevation.input.ground.tiles {
                let blank = elevation.input.interior.is_some() && tile.iter().all(|p| *p == 0);
                if blank {
                    continue;
                }
                if !seen.contains(tile.as_slice()) {
                    in_use += 1;
                }
                group.push(tile.as_slice());
            }
            seen.extend(group);
        }
    }
    let base = builder.lists.art_base.get(&ART_TILES).copied().unwrap_or(0);
    emit::FID_INDEX_CAP.saturating_sub(base + builder.tile_pad() + in_use)
}

/// The new ground of each elevation that link cues changed, by map and
/// elevation ([`draw`]).
pub(crate) struct Drawn(Vec<Vec<Option<NewGround>>>);

/// One elevation's new tile library and squares.
type NewGround = (Vec<Vec<u8>>, Vec<u16>);

/// Marks every map whose options ask for link cues (module notes) and adds
/// each elevation's note to its map's notes. Nothing for the others.
pub(crate) fn draw(plans: &mut [MapPlan<'_>], builder: &LayerBuilder) -> Drawn {
    let mut out: Vec<Vec<Option<NewGround>>> = plans
        .iter()
        .map(|p| vec![None; p.elevations.len()])
        .collect();
    if !plans.iter().any(|p| p.input.options.mark_links.is_some()) {
        return Drawn(out);
    }
    let mut room = tile_room(plans, builder);
    // What leads to each arrival hex: (map, elevation, hex).
    let mut arrival_kind: BTreeMap<(usize, usize, i32), CueKind> = BTreeMap::new();
    for plan in plans.iter() {
        for elevation in &plan.elevations {
            for link in &elevation.links {
                arrival_kind
                    .entry((link.to.0, usize::from(link.to.1), link.to.2))
                    .or_insert(link.kind.into());
            }
        }
    }
    for (m, plan) in plans.iter_mut().enumerate() {
        let Some(options) = plan.input.options.mark_links.clone() else {
            continue;
        };
        let mut left = options.max_tiles;
        let several = plan.elevations.len() > 1;
        for (e, elevation) in plan.elevations.iter().enumerate() {
            let mut cues = Vec::new();
            let mut keep = BTreeSet::new();
            for link in &elevation.links {
                for &hex in &link.grid {
                    cues.push(Cue {
                        hex,
                        kind: link.kind.into(),
                        arrival: false,
                    });
                }
            }
            for exit in &elevation.door_exits {
                for &hex in &exit.grid_hexes {
                    cues.push(Cue {
                        hex,
                        kind: CueKind::Door,
                        arrival: false,
                    });
                }
                cues.push(Cue {
                    hex: exit.return_hex,
                    kind: CueKind::Door,
                    arrival: true,
                });
                keep.extend(crate::walk::Frame::square_of_hex(exit.door_hex));
            }
            for (hex, _) in &elevation.arrivals {
                cues.push(Cue {
                    hex: *hex,
                    kind: arrival_kind
                        .get(&(m, e, *hex))
                        .copied()
                        .unwrap_or(CueKind::Door),
                    arrival: true,
                });
            }
            if cues.is_empty() {
                continue;
            }
            let ground = elevation.input.ground;
            let marked = mark_ground(
                ground,
                &cues,
                &keep,
                &LinkCues {
                    max_tiles: left,
                    ..options.clone()
                },
                room,
            );
            left = left.saturating_sub(marked.added);
            room = room.saturating_sub(marked.added);
            plan.notes.push(format!(
                "{}link cues: {} hexes marked, {} tiles added, {} skipped for budget",
                if several && e > 0 {
                    format!("elevation {e}: ")
                } else {
                    String::new()
                },
                marked.marked,
                marked.added,
                marked.skipped
            ));
            if marked.changed(&ground) {
                out[m][e] = Some((marked.tiles, marked.squares));
            }
        }
    }
    Drawn(out)
}

/// The elevation inputs [`draw`] changed: each the plan's own with the new
/// ground.
pub(crate) fn inputs<'c>(
    plans: &[MapPlan<'c>],
    drawn: &'c Drawn,
) -> Vec<Vec<Option<ElevationInput<'c>>>> {
    plans
        .iter()
        .zip(&drawn.0)
        .map(|(plan, per)| {
            plan.elevations
                .iter()
                .zip(per)
                .map(|(elevation, ground)| {
                    ground.as_ref().map(|(tiles, squares)| {
                        let input = elevation.input;
                        ElevationInput {
                            sampler: input.sampler,
                            ground: GroundTiles { tiles, squares },
                            art: input.art,
                            placements: input.placements,
                            blocked_hexes: input.blocked_hexes,
                            interior: input.interior.clone(),
                            level: input.level.clone(),
                        }
                    })
                })
                .collect()
        })
        .collect()
}

/// Points each changed elevation's plan at its new input ([`inputs`]).
pub(crate) fn apply<'c>(plans: &mut [MapPlan<'c>], inputs: &'c [Vec<Option<ElevationInput<'c>>>]) {
    for (plan, per) in plans.iter_mut().zip(inputs) {
        for (elevation, input) in plan.elevations.iter_mut().zip(per) {
            if let Some(input) = input {
                elevation.input = input;
            }
        }
    }
}
