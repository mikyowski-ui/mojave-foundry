//! Islands: the connected components of a map's passable graph by the
//! engine's movement rule, and the passages that join them.
//!
//! **Movement rule** (read in `vault-engine`'s route search and
//! movement-blocker query, the same rule `mojave-convert probe walk`
//! uses):
//!
//! - An object stops movement when its art is critter, scenery or wall art
//!   (FID categories 1 to 3) and it is neither hidden nor non-blocking.
//!   Item and misc art (exit grids) never stop it.
//! - A blocker flagged multi-hex also stops the six neighbours of its hex.
//!   The engine finds those neighbours with its border-stopping step, which
//!   returns an outer-border hex itself, so a multi-hex object never blocks
//!   a border hex it does not stand on.
//! - A walker steps from a hex to any of its six neighbours that nothing
//!   blocks, except from a hex of the outer border (column or row 0 or
//!   199): the engine's step refuses to leave one, inward too. A border hex
//!   is entered but never left.
//!
//! **Components.** Steps between two inner hexes are symmetric (blocking
//! depends on the hex entered only), so the inner passable hexes fall into
//! ordinary connected components. A passable border hex is a dead end: it
//! joins the largest component among the inner passable hexes next to it
//! (a walker of that component can step onto it), or is a component of its
//! own when it has none. Components are numbered largest first, then by
//! lowest hex.
//!
//! **Passages** ([`shortest_passage`]): the fewest hexes to open between a
//! reached set and a target set, through hexes a cost grid accepts, the
//! hexes off the navmesh counted second, then the lowest hex numbers.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use vault_formats::map::MapObject;
use vault_formats::pro::BasicFlags;
use vault_formats::{Fid, HexGrid};

use crate::walk::{HEXES, neighbours};

/// No component.
const NONE: u32 = u32::MAX;

/// Whether `hex` is on the outer border (or off the grid): the engine
/// never steps off it.
pub fn on_border(hex: i32) -> bool {
    HexGrid::coordinates(hex)
        .map(|[c, r]| c == 0 || r == 0 || c == 199 || r == 199)
        .unwrap_or(true)
}

/// Whether an object with these flags and art stops a walker.
pub fn stops_movement(flags: BasicFlags, art_id: u32) -> bool {
    flags.0 & (BasicFlags::HIDDEN.0 | BasicFlags::NONBLOCKING.0) == 0
        && matches!(Fid(art_id).category(), 1..=3)
}

/// Per hex, whether the engine's movement-blocker query finds a blocker
/// among `objects` (one elevation): an object on the hex that stops
/// movement, or a multi-hex one on a neighbour of an inner hex.
pub fn engine_blocked(objects: &[MapObject]) -> Vec<bool> {
    let mut on = vec![false; HEXES];
    let mut multihex = vec![false; HEXES];
    for object in objects {
        let Some(hex) = object.hex.filter(|h| (0..HEXES as i32).contains(h)) else {
            continue;
        };
        if !stops_movement(object.flags, object.art_id) {
            continue;
        }
        on[hex as usize] = true;
        if object.flags.contains(BasicFlags::MULTIHEX) {
            multihex[hex as usize] = true;
        }
    }
    (0..HEXES as i32)
        .map(|hex| {
            on[hex as usize] || (!on_border(hex) && neighbours(hex).any(|n| multihex[n as usize]))
        })
        .collect()
}

/// The connected components of a passable grid (see the module notes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Components {
    label: Vec<u32>,
    sizes: Vec<usize>,
    first: Vec<i32>,
}

impl Components {
    /// How many components there are.
    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }

    /// Sizes in hexes, largest first; a component's number is its index.
    pub fn sizes(&self) -> &[usize] {
        &self.sizes
    }

    /// The lowest hex of component `id`.
    pub fn first_hex(&self, id: usize) -> Option<i32> {
        self.first.get(id).copied()
    }

    /// The component `hex` is in; `None` for a hex that is not passable.
    pub fn of(&self, hex: i32) -> Option<usize> {
        let label = *self.label.get(usize::try_from(hex).ok()?)?;
        (label != NONE).then_some(label as usize)
    }

    /// The component a walker standing on `hex` moves in: the hex's own,
    /// or for an occupied hex (a critter's, which the engine never counts
    /// against the critter itself) the lowest-numbered, so largest,
    /// component among the neighbours it can step to. `None` when it can
    /// step nowhere, a border hex included.
    pub fn around(&self, hex: i32) -> Option<usize> {
        self.of(hex).or_else(|| {
            if on_border(hex) {
                return None;
            }
            neighbours(hex).filter_map(|n| self.of(n)).min()
        })
    }

    /// The hexes of component `id`, ascending.
    pub fn members(&self, id: usize) -> impl Iterator<Item = i32> + '_ {
        let id = u32::try_from(id).unwrap_or(NONE);
        (0..HEXES as i32).filter(move |hex| id != NONE && self.label[*hex as usize] == id)
    }

    /// Per hex, whether it is in component `id`.
    pub fn mask(&self, id: usize) -> Vec<bool> {
        let id = u32::try_from(id).unwrap_or(NONE);
        self.label.iter().map(|l| id != NONE && *l == id).collect()
    }

    /// Only the components that hold a hex `keep` accepts, renumbered in
    /// the same order.
    pub fn retain_holding(&self, keep: impl Fn(i32) -> bool) -> Components {
        let mut holds = vec![false; self.len()];
        for (hex, l) in self.label.iter().enumerate() {
            if *l != NONE && !holds[*l as usize] && keep(hex as i32) {
                holds[*l as usize] = true;
            }
        }
        let mut map = vec![NONE; self.len()];
        let mut sizes = Vec::new();
        let mut first = Vec::new();
        for id in 0..self.len() {
            if holds[id] {
                map[id] = sizes.len() as u32;
                sizes.push(self.sizes[id]);
                first.push(self.first[id]);
            }
        }
        Components {
            label: self
                .label
                .iter()
                .map(|l| if *l == NONE { NONE } else { map[*l as usize] })
                .collect(),
            sizes,
            first,
        }
    }
}

/// The components of the hexes `passable` accepts, by the engine's
/// movement rule (module notes).
pub fn components(passable: impl Fn(i32) -> bool) -> Components {
    let pass: Vec<bool> = (0..HEXES as i32).map(&passable).collect();
    let mut label = vec![NONE; HEXES];
    let mut sizes: Vec<usize> = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    for start in 0..HEXES as i32 {
        if !pass[start as usize] || on_border(start) || label[start as usize] != NONE {
            continue;
        }
        let id = sizes.len() as u32;
        label[start as usize] = id;
        let mut size = 1;
        queue.push_back(start);
        while let Some(hex) = queue.pop_front() {
            for next in neighbours(hex) {
                let n = next as usize;
                if pass[n] && !on_border(next) && label[n] == NONE {
                    label[n] = id;
                    size += 1;
                    queue.push_back(next);
                }
            }
        }
        sizes.push(size);
    }
    // Border hexes: dead ends of the largest inner component beside them.
    let inner = sizes.clone();
    let mut attach = Vec::new();
    for hex in 0..HEXES as i32 {
        if !pass[hex as usize] || !on_border(hex) {
            continue;
        }
        let best = neighbours(hex)
            .filter(|n| !on_border(*n))
            .filter_map(|n| {
                let l = label[n as usize];
                (l != NONE).then_some(l)
            })
            .max_by_key(|l| (inner[*l as usize], Reverse(*l)));
        attach.push((hex, best));
    }
    for (hex, best) in attach {
        let id = match best {
            Some(id) => id,
            None => {
                sizes.push(0);
                (sizes.len() - 1) as u32
            }
        };
        label[hex as usize] = id;
        sizes[id as usize] += 1;
    }
    // Renumber: largest first, then lowest hex.
    let mut first = vec![i32::MAX; sizes.len()];
    for (hex, l) in label.iter().enumerate() {
        if *l != NONE && first[*l as usize] == i32::MAX {
            first[*l as usize] = hex as i32;
        }
    }
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&id| (Reverse(sizes[id]), first[id]));
    let mut renumber = vec![NONE; sizes.len()];
    for (new, old) in order.iter().enumerate() {
        renumber[*old] = new as u32;
    }
    for l in &mut label {
        if *l != NONE {
            *l = renumber[*l as usize];
        }
    }
    Components {
        label,
        sizes: order.iter().map(|id| sizes[*id]).collect(),
        first: order.iter().map(|id| first[*id]).collect(),
    }
}

/// How costly a hex is to open, for [`shortest_passage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OpenCost {
    /// The source's navmesh (or, in a cell without one, the slope test)
    /// lets an actor stand there; only a blocker keeps it closed.
    Standable,
    /// Off the navmesh, under the slope limit.
    OffNavmesh,
}

/// A passage between two parts of a map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passage {
    /// The reached hex it starts beside and the target hex it ends beside.
    pub from: i32,
    pub to: i32,
    /// The hexes to open, from the reached side to the target side.
    pub hexes: Vec<i32>,
    /// How many of them are off the navmesh.
    pub off_navmesh: usize,
}

/// The passage with the fewest opened hexes from a hex of `reached` to a
/// hex of `target`, through hexes `cost` accepts (`None`: never opened),
/// at most `limit` hexes long when a limit is given. Among equally short
/// ones it opens the fewest hexes off the navmesh, then the lowest hex
/// numbers along the way. Passages run between inner hexes only: border
/// hexes are neither opened nor reached from. `None` when there is none.
pub fn shortest_passage(
    reached: &[bool],
    target: &[bool],
    cost: &[Option<OpenCost>],
    limit: Option<usize>,
) -> Option<Passage> {
    let is = |grid: &[bool], hex: i32| grid.get(hex as usize).copied().unwrap_or(false);
    let open = |hex: i32| {
        !on_border(hex)
            && !is(reached, hex)
            && !is(target, hex)
            && cost.get(hex as usize).copied().flatten().is_some()
    };
    let off = |hex: i32| usize::from(cost[hex as usize] == Some(OpenCost::OffNavmesh));
    // (length, off-navmesh count) per hex, and where it was entered from.
    let mut best: Vec<Option<(usize, usize)>> = vec![None; HEXES];
    let mut parent = vec![-1i32; HEXES];
    let mut heap = BinaryHeap::new();
    for source in 0..HEXES as i32 {
        if !is(reached, source) || on_border(source) {
            continue;
        }
        for next in neighbours(source) {
            if !open(next) {
                continue;
            }
            let key = (1, off(next));
            if best[next as usize].is_none_or(|b| key < b) {
                best[next as usize] = Some(key);
                parent[next as usize] = source;
                heap.push(Reverse((key.0, key.1, next)));
            }
        }
    }
    while let Some(Reverse((length, offs, hex))) = heap.pop() {
        if best[hex as usize] != Some((length, offs)) {
            continue;
        }
        if limit.is_some_and(|limit| length > limit) {
            break;
        }
        let mut ends: Vec<i32> = neighbours(hex)
            .filter(|n| is(target, *n) && !on_border(*n))
            .collect();
        ends.sort_unstable();
        if let Some(&to) = ends.first() {
            let mut hexes = vec![hex];
            let mut at = hex;
            loop {
                let before = parent[at as usize];
                if is(reached, before) {
                    hexes.reverse();
                    return Some(Passage {
                        from: before,
                        to,
                        off_navmesh: hexes.iter().map(|h| off(*h)).sum(),
                        hexes,
                    });
                }
                hexes.push(before);
                at = before;
            }
        }
        for next in neighbours(hex) {
            if !open(next) {
                continue;
            }
            let key = (length + 1, offs + off(next));
            if best[next as usize].is_none_or(|b| key < b) {
                best[next as usize] = Some(key);
                parent[next as usize] = hex;
                heap.push(Reverse((key.0, key.1, next)));
            }
        }
    }
    None
}

/// `hexes` widened by `rings` rings of the hexes `cost` accepts that are
/// not already `reached` (inner hexes only), sorted.
pub fn widen(hexes: &[i32], rings: u32, reached: &[bool], cost: &[Option<OpenCost>]) -> Vec<i32> {
    let mut all: std::collections::BTreeSet<i32> = hexes.iter().copied().collect();
    let mut edge: Vec<i32> = hexes.to_vec();
    for _ in 0..rings {
        let mut next_edge = Vec::new();
        for hex in &edge {
            for next in neighbours(*hex) {
                if !on_border(next)
                    && !reached.get(next as usize).copied().unwrap_or(false)
                    && cost.get(next as usize).copied().flatten().is_some()
                    && all.insert(next)
                {
                    next_edge.push(next);
                }
            }
        }
        edge = next_edge;
    }
    all.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(c: i32, r: i32) -> i32 {
        c + 200 * r
    }

    #[test]
    fn two_rooms_and_a_border_dead_end() {
        // A wall down column 100 splits the open grid in two; the part of
        // columns 0 to 99 is larger than that of 101 to 199.
        let wall = |h: i32| h % 200 == 100;
        let comps = components(|h| !wall(h));
        // Two rooms, and two corners whose every neighbour is a border
        // hex: dead ends nothing reaches, a component each.
        assert_eq!(comps.len(), 4);
        assert!(comps.sizes()[0] > comps.sizes()[1]);
        assert_eq!(&comps.sizes()[2..], [1, 1]);
        assert!(comps.of(199).is_some() && comps.of(199 * 200).is_some());
        assert_eq!(comps.of(hex(50, 50)), Some(0));
        assert_eq!(comps.of(hex(150, 50)), Some(1));
        assert_eq!(comps.of(hex(100, 50)), None);
        // Border hexes join the component beside them.
        assert_eq!(comps.of(hex(0, 50)), Some(0));
        assert_eq!(comps.of(hex(199, 50)), Some(1));
        assert_eq!(comps.sizes().iter().sum::<usize>(), 40_000 - 200);
        // A blocked hex between rooms: its walker moves in the larger.
        assert_eq!(comps.around(hex(100, 50)), Some(0));
    }

    #[test]
    fn a_border_hex_never_links_two_components() {
        // Only row 0 is open, plus two inner pockets touching it.
        let open = |h: i32| h / 200 == 0 || h == hex(10, 1) || h == hex(12, 1);
        let comps = components(open);
        let a = comps.of(hex(10, 1)).unwrap();
        let b = comps.of(hex(12, 1)).unwrap();
        assert_ne!(a, b, "a border row joins nothing");
        // Border hexes with no inner neighbour are components of their own.
        assert!(comps.of(hex(100, 0)).is_some());
        assert!(comps.len() > 2);
    }

    #[test]
    fn multihex_blocks_inner_neighbours_only() {
        let mut object = emit::map::plain_object(
            1,
            0x0200_0001,
            Fid::new(2, 0, 0, 0, 0),
            hex(1, 1),
            0,
            BasicFlags(BasicFlags::MULTIHEX.0),
        );
        let blocked = engine_blocked(&[object.clone()]);
        assert!(blocked[hex(1, 1) as usize]);
        assert!(blocked[hex(2, 1) as usize], "an inner neighbour");
        assert!(!blocked[hex(1, 0) as usize], "a border neighbour");
        // Misc art (an exit grid) and non-blocking objects stop nothing.
        object.art_id = Fid::new(5, 0, 0, 0, 0).0;
        assert!(!engine_blocked(&[object.clone()])[hex(1, 1) as usize]);
        object.art_id = Fid::new(3, 0, 0, 0, 0).0;
        object.flags = BasicFlags(BasicFlags::NONBLOCKING.0);
        assert!(!engine_blocked(&[object])[hex(1, 1) as usize]);
    }

    #[test]
    fn the_shortest_passage_prefers_the_navmesh() {
        // West part reached, east part the target, a barrier of columns
        // 100 and 101 between them; the navmesh covers row 60 only.
        let mut reached = vec![false; HEXES];
        let mut target = vec![false; HEXES];
        let mut cost = vec![None; HEXES];
        for h in 0..HEXES as i32 {
            match h % 200 {
                0..100 => reached[h as usize] = true,
                100 | 101 => {
                    cost[h as usize] = Some(if h / 200 == 60 {
                        OpenCost::Standable
                    } else {
                        OpenCost::OffNavmesh
                    })
                }
                _ => target[h as usize] = true,
            }
        }
        let passage = shortest_passage(&reached, &target, &cost, Some(4)).unwrap();
        assert_eq!(passage.hexes.len(), 2);
        assert_eq!(passage.off_navmesh, 0);
        assert!(passage.hexes.iter().all(|h| h / 200 == 60));
        assert!(reached[passage.from as usize] && target[passage.to as usize]);
        assert!(shortest_passage(&reached, &target, &cost, Some(1)).is_none());
        // Widened by a ring: still only barrier hexes.
        let wide = widen(&passage.hexes, 1, &reached, &cost);
        assert!(wide.len() > 2);
        assert!(wide.iter().all(|h| cost[*h as usize].is_some()));
    }
}
