//! Where a walker can go on a written map, by the engine's movement rule,
//! and routes cut into hops short enough for its capped route search.
//!
//! `probe walk` prints this; `shots` walks the player along it.

use std::collections::VecDeque;
use std::path::Path;

use vault_formats::HexGrid;

/// Hexes on one elevation.
pub const HEXES: usize = 40_000;

/// An exit grid: its hex, and the map and hex it leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Exit {
    pub hex: i32,
    pub to_map: i32,
    pub to_hex: i32,
    /// The destination elevation.
    pub to_elevation: i32,
}

/// One elevation of one map (elevation 0 unless loaded with
/// [`WalkMap::load_elevation`]) as a walker sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkMap {
    /// `maps/<file>`.
    pub path: String,
    /// Objects on elevation 0.
    pub objects: usize,
    /// Objects that stop movement.
    pub blockers: usize,
    /// Per hex: something there stops movement.
    pub blocked: Vec<bool>,
    /// Exit grids, sorted.
    pub exits: Vec<Exit>,
    /// Where a walk starts: the map's entry hex on its entry elevation;
    /// on another elevation the first arrival hex of an exit grid leading
    /// there (stairs), or 0 when nothing leads there.
    pub entry: i32,
    /// The elevation loaded, and the map's entry elevation.
    pub elevation: usize,
    pub entry_elevation: usize,
    /// Arrival hexes on this elevation of exit grids leading here from
    /// this map's other elevations, sorted.
    pub arrivals: Vec<i32>,
}

/// No parent yet (not reached).
const UNSEEN: i32 = -2;
/// The start of a flood.
const ROOT: i32 = -1;

fn in_grid(hex: i32) -> bool {
    (0..HEXES as i32).contains(&hex)
}

fn neighbours(hex: i32) -> impl Iterator<Item = i32> {
    (0..6u8).filter_map(move |o| HexGrid::neighbor(hex, o).ok().flatten())
}

/// The engine never steps off an outer-border hex.
fn border(hex: i32) -> bool {
    HexGrid::coordinates(hex)
        .map(|[c, r]| c == 0 || r == 0 || c == 199 || r == 199)
        .unwrap_or(true)
}

/// A breadth-first flood from `start` over hexes that are neither blocked
/// nor in `avoid`: for every hex, the hex it was reached from (`ROOT` for
/// the start, `UNSEEN` when not reached). Border hexes are reached but
/// never left.
pub fn flood(blocked: &[bool], avoid: &[bool], start: i32) -> Vec<i32> {
    let mut parent = vec![UNSEEN; HEXES];
    let free = |hex: i32| {
        let i = hex as usize;
        !blocked.get(i).copied().unwrap_or(true) && !avoid.get(i).copied().unwrap_or(false)
    };
    if !in_grid(start) || blocked.get(start as usize).copied().unwrap_or(true) {
        return parent;
    }
    parent[start as usize] = ROOT;
    let mut queue = VecDeque::from([start]);
    while let Some(hex) = queue.pop_front() {
        if border(hex) {
            continue;
        }
        for next in neighbours(hex) {
            if parent[next as usize] == UNSEEN && free(next) {
                parent[next as usize] = hex;
                queue.push_back(next);
            }
        }
    }
    parent
}

/// Whether a flood reached `hex`.
pub fn reached(parent: &[i32], hex: i32) -> bool {
    in_grid(hex) && parent[hex as usize] != UNSEEN
}

/// The path from the flood's start to `hex`, both included; empty when
/// `hex` was not reached.
pub fn path_to(parent: &[i32], hex: i32) -> Vec<i32> {
    if !reached(parent, hex) {
        return Vec::new();
    }
    let mut path = vec![hex];
    while let Some(&last) = path.last() {
        let before = parent[last as usize];
        if before < 0 {
            break;
        }
        path.push(before);
    }
    path.reverse();
    path
}

/// Every `hop`-th hex of a path after its start, and always its end.
pub fn waypoints(path: &[i32], hop: usize) -> Vec<i32> {
    let mut points: Vec<i32> = path.iter().copied().skip(1).step_by(hop.max(1)).collect();
    if let Some(&end) = path.last()
        && path.len() > 1
        && points.last() != Some(&end)
    {
        points.push(end);
    }
    points
}

/// Waypoints along a shortest walkable path, at most `hop` steps apart,
/// placed so that the engine's own route between two of them cannot cross
/// a hex of `avoid` (exit grids). The engine picks its own shortest route
/// between waypoints, not necessarily this path's; a segment of `n` steps
/// from `a` to `b` is accepted only when every avoided hex `e` has
/// `distance(a, e) + distance(e, b) > n`, which no hex on a route of `n`
/// steps or fewer can have. Near an exit grid the waypoints close up to
/// every step. The path's own end is never avoided.
pub fn safe_waypoints(path: &[i32], hop: usize, avoid: &[i32]) -> Vec<i32> {
    let distance = |a: i32, b: i32| HexGrid::lattice_distance(a, b).unwrap_or(i32::MAX / 4);
    let end = path.last().copied();
    let avoid: Vec<i32> = avoid.iter().copied().filter(|e| Some(*e) != end).collect();
    let clear = |from: usize, to: usize| {
        let (a, b) = (path[from], path[to]);
        let steps = (to - from) as i32;
        avoid
            .iter()
            .all(|e| distance(a, *e).saturating_add(distance(*e, b)) > steps)
    };
    let mut points = Vec::new();
    let mut at = 0usize;
    while at + 1 < path.len() {
        let far = (at + hop.max(1)).min(path.len() - 1);
        let next = (at + 2..=far)
            .rev()
            .find(|to| clear(at, *to))
            .unwrap_or(at + 1);
        points.push(path[next]);
        at = next;
    }
    points
}

/// A walk from the entry to (or toward) a requested hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub requested: i32,
    /// Where the walk ends: the requested hex when it is reachable, else
    /// the reachable hex nearest it.
    pub destination: i32,
    /// From the entry to the destination, both included.
    pub path: Vec<i32>,
}

impl Route {
    pub fn steps(&self) -> usize {
        self.path.len().saturating_sub(1)
    }
}

impl WalkMap {
    /// Reads `maps/<map>` of a written mod layer from disk, by the
    /// engine's movement rule: an object stops movement when its art is
    /// critter, scenery or wall art and it is neither hidden nor
    /// non-blocking; a multi-hex object also stops its six neighbours.
    pub fn load(layer_dir: &Path, map: &str) -> Result<Self, String> {
        Self::load_elevation(layer_dir, map, 0)
    }

    /// [`WalkMap::load`] of elevation `elevation` (0 to 2).
    pub fn load_elevation(layer_dir: &Path, map: &str, elevation: usize) -> Result<Self, String> {
        use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
        use vault_formats::pro::BasicFlags;
        use vault_formats::pro_catalog::LocalPrototypeCatalog;
        use vault_formats::vfs::{ByteSource, MemoryDir, MemoryMount};
        use vault_formats::{Fid, GameProfile};

        let layer = map_builder::read_layer(layer_dir).map_err(|e| e.to_string())?;
        let mut entries = Vec::new();
        for (path, bytes) in layer.files() {
            if path == "vault-mod.toml" || path == "README.txt" {
                continue;
            }
            let source = ByteSource::new(bytes.clone()).map_err(|e| format!("{path}: {e}"))?;
            entries.push((path.clone(), source));
        }
        let tree = MemoryDir::new(entries).map_err(|e| e.to_string())?;
        let provider = MemoryMount {
            profile: GameProfile::Fallout2,
            loose: Some(tree),
            ..MemoryMount::default()
        }
        .mount()
        .map_err(|e| e.to_string())?;
        let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);
        let path = format!("maps/{map}");
        let bytes = layer
            .files()
            .get(&path)
            .ok_or_else(|| format!("{path} is not in the layer"))?;
        let parsed = MapFile::parse_for(bytes, &catalog, GameProfile::Fallout2)
            .map_err(|e| format!("{path}: {e}"))?;

        if elevation > 2 {
            return Err(format!(
                "elevation {elevation}: a map has elevations 0 to 2"
            ));
        }
        // The map's own number, for grids between its elevations.
        let own_number = map_builder::read_layer(layer_dir).ok().and_then(|l| {
            l.manifest()
                .maps
                .iter()
                .find(|m| m.file == map)
                .map(|m| m.number as i32)
        });
        let mut arrivals = Vec::new();
        for (e, list) in parsed.objects.elevations.iter().enumerate() {
            if e == elevation {
                continue;
            }
            for object in list {
                if let ObjectState::Other {
                    detail:
                        ObjectDetail::ExitGrid {
                            map: to,
                            hex: at,
                            elevation: to_e,
                            ..
                        },
                    ..
                } = &object.state
                    && Some(*to) == own_number
                    && *to_e as usize == elevation
                {
                    arrivals.push(*at);
                }
            }
        }
        arrivals.sort_unstable();
        arrivals.dedup();
        let mut blocked = vec![false; HEXES];
        let mut exits = Vec::new();
        let mut blockers = 0usize;
        for object in &parsed.objects.elevations[elevation] {
            let Some(hex) = object.hex.filter(|h| in_grid(*h)) else {
                continue;
            };
            if let ObjectState::Other {
                detail:
                    ObjectDetail::ExitGrid {
                        map: to,
                        hex: at,
                        elevation: to_e,
                        ..
                    },
                ..
            } = &object.state
            {
                exits.push(Exit {
                    hex,
                    to_map: *to,
                    to_hex: *at,
                    to_elevation: *to_e,
                });
            }
            let stops = object.flags.0 & (BasicFlags::HIDDEN.0 | BasicFlags::NONBLOCKING.0) == 0
                && matches!(Fid(object.art_id).category(), 1..=3);
            if !stops {
                continue;
            }
            blockers += 1;
            blocked[hex as usize] = true;
            if object.flags.contains(BasicFlags::MULTIHEX) {
                for next in neighbours(hex) {
                    blocked[next as usize] = true;
                }
            }
        }
        exits.sort_unstable();
        let entry_elevation = parsed.header.entry_elevation.clamp(0, 2) as usize;
        let entry = if elevation == entry_elevation {
            parsed.header.entry_hex
        } else {
            arrivals.first().copied().unwrap_or(0)
        };
        Ok(Self {
            path,
            objects: parsed.objects.elevations[elevation].len(),
            blockers,
            blocked,
            exits,
            entry,
            elevation,
            entry_elevation,
            arrivals,
        })
    }

    /// Per hex: an exit grid is there.
    pub fn exit_mask(&self) -> Vec<bool> {
        let mut on = vec![false; HEXES];
        for exit in &self.exits {
            on[exit.hex as usize] = true;
        }
        on
    }

    /// The flood from the entry, crossing exit grids freely (`probe walk`).
    pub fn flood_from_entry(&self) -> Vec<i32> {
        flood(&self.blocked, &[], self.entry)
    }

    /// A route from the entry to `requested` that never steps on an exit
    /// grid (stepping on one loads another map), unless the exit grid is
    /// the requested hex itself. When `requested` cannot be reached, the
    /// route ends at the reachable hex nearest it (fewest hex steps, then
    /// lowest hex number). `None` when nothing but the entry is reachable
    /// and the entry is not the request, or the entry is blocked.
    pub fn route_to(&self, requested: i32) -> Option<Route> {
        self.route_between(self.entry, requested)
    }

    /// [`WalkMap::route_to`] from `from` instead of the entry: a walk
    /// that continues from where an earlier one ended. `None` when `from`
    /// is blocked or off the grid.
    pub fn route_between(&self, from: i32, requested: i32) -> Option<Route> {
        let mut avoid = self.exit_mask();
        if in_grid(requested) {
            avoid[requested as usize] = false;
        }
        if in_grid(from) {
            avoid[from as usize] = false;
        }
        let parent = flood(&self.blocked, &avoid, from);
        if !reached(&parent, from) {
            return None;
        }
        let destination = if reached(&parent, requested) {
            requested
        } else {
            // A requested hex off the grid is measured from its clamp.
            let target = if in_grid(requested) { requested } else { from };
            (0..HEXES as i32)
                .filter(|h| reached(&parent, *h) && !avoid[*h as usize])
                .min_by_key(|h| {
                    (
                        HexGrid::lattice_distance(*h, target).unwrap_or(i32::MAX),
                        *h,
                    )
                })?
        };
        Some(Route {
            requested,
            destination,
            path: path_to(&parent, destination),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(c: i32, r: i32) -> i32 {
        c + 200 * r
    }

    fn open_map(entry: i32) -> WalkMap {
        WalkMap {
            path: "maps/test.map".to_owned(),
            objects: 0,
            blockers: 0,
            blocked: vec![false; HEXES],
            exits: Vec::new(),
            entry,
            elevation: 0,
            entry_elevation: 0,
            arrivals: Vec::new(),
        }
    }

    #[test]
    fn an_open_route_is_as_short_as_the_hex_distance() {
        let map = open_map(hex(100, 100));
        let route = map.route_to(hex(130, 100)).expect("a route");
        assert_eq!(route.destination, hex(130, 100));
        assert_eq!(route.steps(), 30);
        assert_eq!(route.path[0], map.entry);
        for pair in route.path.windows(2) {
            assert_eq!(HexGrid::lattice_distance(pair[0], pair[1]), Ok(1));
        }
        let points = waypoints(&route.path, 15);
        assert_eq!(points, vec![route.path[1], route.path[16], hex(130, 100)]);
        // The end is never repeated.
        let points = waypoints(&route.path, 10);
        assert_eq!(
            points,
            vec![route.path[1], route.path[11], route.path[21], hex(130, 100)]
        );
        let points = waypoints(&route.path, 1);
        assert_eq!(points.len(), 30);
        assert_eq!(points.last(), Some(&hex(130, 100)));
        assert!(waypoints(&[map.entry], 15).is_empty());
    }

    #[test]
    fn safe_waypoints_keep_the_engine_off_exit_grids() {
        // Along row 100, with an exit grid just beside the row at column
        // 120: a straight hop past it could step on it.
        let path: Vec<i32> = (100..=140).map(|c| hex(c, 100)).collect();
        let exit = hex(120, 99);
        let points = safe_waypoints(&path, 15, &[exit]);
        assert_eq!(points.last(), Some(&hex(140, 100)));
        let mut from = path[0];
        for &to in &points {
            let steps = path.iter().position(|h| *h == to).unwrap()
                - path.iter().position(|h| *h == from).unwrap();
            assert!(steps <= 15);
            let via = HexGrid::lattice_distance(from, exit).unwrap()
                + HexGrid::lattice_distance(exit, to).unwrap();
            assert!(via > steps as i32, "{from} -> {to} could cross the exit");
            from = to;
        }
        assert!(points.len() > 3, "closer near the exit: {points:?}");
        // Without exits: every 15 steps, then the end.
        assert_eq!(
            safe_waypoints(&path, 15, &[]),
            vec![hex(115, 100), hex(130, 100), hex(140, 100)]
        );
        // An exit grid that is the end itself is walked onto.
        assert_eq!(
            safe_waypoints(&path, 50, &[hex(140, 100)]),
            vec![hex(140, 100)]
        );
        assert!(safe_waypoints(&path[..1], 15, &[]).is_empty());
    }

    #[test]
    fn a_route_goes_around_a_wall_and_stops_short_of_a_closed_room() {
        let mut map = open_map(hex(100, 100));
        // A wall across column 110, rows 50 to 150.
        for r in 50..=150 {
            map.blocked[hex(110, r) as usize] = true;
        }
        let route = map.route_to(hex(120, 100)).expect("a route");
        assert_eq!(route.destination, hex(120, 100));
        assert!(route.steps() > 20, "it goes around: {}", route.steps());
        assert!(route.path.iter().all(|h| !map.blocked[*h as usize]));

        // A sealed ring around (150, 100): the nearest reachable hex.
        let target = hex(150, 100);
        for o in 0..6u8 {
            let n = HexGrid::neighbor(target, o).unwrap().unwrap();
            map.blocked[n as usize] = true;
        }
        let route = map.route_to(target).expect("a route");
        assert_ne!(route.destination, target);
        assert_eq!(HexGrid::lattice_distance(route.destination, target), Ok(2));
        assert!(!map.blocked[route.destination as usize]);
    }

    #[test]
    fn a_route_keeps_off_exit_grids_unless_one_is_the_target() {
        let mut map = open_map(hex(100, 100));
        // A line of exit grids across the way, with no gap.
        for r in 0..200 {
            map.exits.push(Exit {
                hex: hex(105, r),
                to_map: 152,
                to_hex: 1,
                to_elevation: 0,
            });
        }
        let route = map.route_to(hex(110, 100)).expect("a route");
        let exits = map.exit_mask();
        assert!(route.path.iter().all(|h| !exits[*h as usize]));
        assert!(
            route.destination % 200 < 105,
            "stays this side: {}",
            route.destination
        );
        // Asked for, an exit grid is a destination like any other.
        let route = map.route_to(hex(105, 100)).expect("a route");
        assert_eq!(route.destination, hex(105, 100));
        assert_eq!(route.steps(), 5);
    }

    #[test]
    fn a_route_continues_from_where_the_last_one_ended() {
        let mut map = open_map(hex(100, 100));
        for r in 50..=150 {
            map.blocked[hex(110, r) as usize] = true;
        }
        let first = map.route_to(hex(105, 100)).expect("a route");
        let second = map
            .route_between(first.destination, hex(120, 100))
            .expect("a route");
        assert_eq!(second.path[0], hex(105, 100));
        assert_eq!(second.destination, hex(120, 100));
        assert!(second.path.iter().all(|h| !map.blocked[*h as usize]));
        // Into a blocked hex: the nearest reachable one, measured from it.
        map.blocked[hex(130, 100) as usize] = true;
        let third = map
            .route_between(second.destination, hex(130, 100))
            .expect("a route");
        assert_eq!(
            HexGrid::lattice_distance(third.destination, hex(130, 100)),
            Ok(1)
        );
        assert_eq!(map.route_between(hex(110, 100), hex(120, 100)), None);
    }

    #[test]
    fn a_blocked_entry_has_no_route() {
        let mut map = open_map(hex(100, 100));
        map.blocked[map.entry as usize] = true;
        assert_eq!(map.route_to(hex(101, 100)), None);
        assert!(!reached(&map.flood_from_entry(), hex(101, 100)));
    }

    #[test]
    fn the_flood_never_leaves_the_border() {
        let map = open_map(hex(1, 100));
        let parent = map.flood_from_entry();
        assert!(reached(&parent, hex(0, 100)));
        // Every hex reached from a border hex would have it as parent.
        assert!(parent.iter().filter(|p| **p >= 0).all(|p| !border(*p)));
    }
}
