//! Measure islands on a layer the builder wrote, with this crate's rules.
//!
//! ```text
//! cargo run --release -p map-builder --example islands -- \
//!     <layer dir> <map file> <mir dir> <worldspace> <x_west> <y_north> \
//!     [max length] [door hex,door hex,...]
//! ```
//!
//! The layer is read with `read_layer`; walkability is recomputed from the
//! MIR cells (navmesh, slope) with every object that stops movement as a
//! blocker, since a written map does not say which hexes the slicer
//! blocked. Placements are rebuilt from the objects and their FRMs, each
//! its own reference, so every scenery sprite at least
//! `cover::LANDMARK_WIDTH` wide is a landmark band of its own (the build
//! groups bands by reference). With `ISLANDS_STRIPS=1` a wall strip's hex
//! counts as openable when no wall art stands on it (the build never
//! opens one). Prints counts and hex numbers only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use map_builder::cover::{Cover, Sight};
use map_builder::islands::{self, OpenCost, components, on_border, shortest_passage, widen};
use map_builder::walk::{self, Frame, HEXES, Verdict, neighbours};
use map_builder::{ArtCategory, ArtImage, BuildOptions, Placement, Role, read_layer};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::pro::BasicFlags;
use vault_formats::{
    ArtCatalog, ByteSource, Fid, FrmArchive, GameProfile, HexGrid, LocalPrototypeCatalog, MemoryDir,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 7 {
        eprintln!(
            "usage: islands <layer> <map> <mir> <worldspace> <x_west> <y_north> [max] [doors]"
        );
        std::process::exit(2);
    }
    let limit: usize = args
        .get(7)
        .map_or(map_builder::PASSAGE_MAX_LENGTH, |a| a.parse().unwrap());
    let doors: Vec<i32> = args
        .get(8)
        .map(|a| {
            a.split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().unwrap())
                .collect()
        })
        .unwrap_or_default();
    let layer = read_layer(Path::new(&args[1])).expect("layer");
    let files = layer.files();
    let tree = MemoryDir::new(
        files
            .iter()
            .map(|(p, b)| (p.clone(), ByteSource::new(b.clone()).unwrap())),
    )
    .unwrap();
    let catalog = LocalPrototypeCatalog::for_game(&tree, GameProfile::Fallout2);
    let art_catalog = ArtCatalog::load(&tree).expect("art lists");
    let parse = |path: &str| {
        MapFile::parse_for(&files[path], &catalog, GameProfile::Fallout2).expect("map")
    };
    let map_path = format!("maps/{}", args[2]);
    let map = parse(&map_path);
    let objects = &map.objects.elevations[0];
    let entry = map.header.entry_hex;

    // Walkability from the MIR.
    let mir = mir::Mir::open(Path::new(&args[3])).expect("mir");
    let worldspace = args[4].as_str();
    let window = mir::MapWindow {
        x_west: args[5].parse().unwrap(),
        y_north: args[6].parse().unwrap(),
        size: 14_000.0,
    };
    let projection =
        foundry_render::Projection::new(&foundry_render::Calibration::default()).unwrap();
    let frame = Frame::new(&window, &projection).unwrap();
    let sampler = mir::GroundSampler::load(&mir, worldspace, &window).expect("sampler");
    let (xs, ys) = window.cells(1);
    let mut cells = Vec::new();
    for y in ys {
        for x in xs.clone() {
            if let Some(cell) = mir.cell(worldspace, x, y).unwrap() {
                cells.push(cell);
            }
        }
    }
    let options = BuildOptions::default();
    let blocked = islands::engine_blocked(objects);
    let (verdicts, _) = walk::verdicts(&frame, &sampler, &cells, &blocked, &options);
    let walkable: Vec<bool> = verdicts.iter().map(|v| *v == Verdict::Walkable).collect();
    let (free, _) = walk::verdicts(&frame, &sampler, &cells, &[], &options);

    // Art and placements back from the objects.
    let mut art: Vec<ArtImage> = Vec::new();
    let mut art_index: BTreeMap<u32, usize> = BTreeMap::new();
    let mut placements = Vec::new();
    let mut protected = vec![false; HEXES];
    let mut strip_hex = vec![false; HEXES];
    // ISLANDS_STRIPS=1: a wall strip's hex may open when its art does not
    // stand on it; otherwise every wall strip's hex is closed.
    let strips_open = std::env::var("ISLANDS_STRIPS").is_ok_and(|v| v == "1");
    let mut critters = Vec::new();
    let mut grids: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for object in objects {
        let Some(hex) = object.hex else { continue };
        let fid = Fid(object.art_id);
        if let ObjectState::Critter(_) = object.state {
            critters.push(hex);
            protected[hex as usize] = true;
            continue;
        }
        if let ObjectState::Other {
            detail: ObjectDetail::ExitGrid { map: to, .. },
            ..
        } = object.state
        {
            grids.entry(to).or_default().push(hex);
            continue;
        }
        if !matches!(fid.category(), 2 | 3) {
            continue;
        }
        let index = *art_index.entry(object.art_id).or_insert_with(|| {
            let image = art_catalog
                .path(Fid(object.art_id))
                .ok()
                .and_then(|path| files.get(&path))
                .and_then(|bytes| FrmArchive::parse(bytes).ok())
                .and_then(|frm| {
                    let f = frm.frame(0, 0)?;
                    Some(ArtImage {
                        category: if fid.category() == 3 {
                            ArtCategory::Walls
                        } else {
                            ArtCategory::Scenery
                        },
                        width: u32::from(f.width),
                        height: u32::from(f.height),
                        pixels: f.pixels().to_vec(),
                        shift: frm.shifts[0],
                        source: String::new(),
                    })
                })
                .unwrap_or(ArtImage {
                    category: ArtCategory::Scenery,
                    width: 0,
                    height: 0,
                    pixels: Vec::new(),
                    shift: [0, 0],
                    source: String::new(),
                });
            art.push(image);
            art.len() - 1
        });
        let door = matches!(
            object.state,
            ObjectState::Other {
                detail: ObjectDetail::Door(_),
                ..
            }
        );
        let flat = object.flags.contains(BasicFlags::FLAT);
        let invisible = art[index].pixels.iter().all(|p| *p == 0);
        let role = if fid.category() == 3 {
            Role::WallStrip
        } else if door {
            Role::Door
        } else if flat {
            Role::FloorPiece
        } else {
            Role::Scenery
        };
        let stops = islands::stops_movement(object.flags, object.art_id);
        if role == Role::WallStrip {
            strip_hex[hex as usize] = true;
        } else if door || (stops && !invisible) {
            protected[hex as usize] = true;
        }
        if door {
            for n in neighbours(hex) {
                protected[n as usize] = true;
            }
        }
        placements.push(Placement {
            art: index,
            hex,
            displacement: object.displacement,
            role,
            reference: format!("object {}", object.id),
            blocking: stops,
        });
    }
    // Return hexes: where the stub maps' grids lead.
    let mut returns = BTreeSet::new();
    for path in files
        .keys()
        .filter(|p| p.starts_with("maps/") && **p != map_path)
    {
        for object in &parse(path).objects.elevations[0] {
            if let ObjectState::Other {
                detail: ObjectDetail::ExitGrid { hex, .. },
                ..
            } = object.state
            {
                returns.insert(hex);
            }
        }
    }
    let cover = Cover::new(&art, &placements);
    let mut standing = 0;
    let strips = strip_hex.iter().filter(|s| **s).count();
    for hex in 0..HEXES as i32 {
        if strip_hex[hex as usize] {
            let stands = cover.stands_on(hex);
            standing += usize::from(stands);
            if !strips_open || stands {
                protected[hex as usize] = true;
            }
        }
    }
    println!(
        "wall strip hexes {strips}, with wall art standing on them {standing}; strips open: {strips_open}"
    );

    let comps = components(|h| walkable[h as usize]);
    let walkable_count = walkable.iter().filter(|w| **w).count();
    println!(
        "{}: {} objects, {walkable_count} walkable (recomputed), {} components holding walkable hexes",
        map_path,
        objects.len(),
        comps.len()
    );
    println!("largest: {:?}", &comps.sizes()[..comps.len().min(8)]);
    println!(
        "entry {entry}: component {:?}, sight {:?}",
        comps.of(entry),
        cover.sight(entry, map_builder::ENTRANCE_BOX)
    );

    // Targets and their islands.
    let mut holdings: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut add = |hexes: &[i32], what: String| {
        let ids: Vec<usize> = hexes.iter().filter_map(|h| comps.around(*h)).collect();
        if ids.contains(&0) {
            return;
        }
        if let Some(id) = ids.into_iter().min() {
            holdings.entry(id).or_default().push(what);
        } else {
            println!("  {what}: in no walkable component");
        }
    };
    for (to, hexes) in &grids {
        add(hexes, format!("exit to map {to}"));
    }
    for hex in &returns {
        add(&[*hex], format!("return hex {hex}"));
    }
    for hex in &critters {
        add(&[*hex], format!("critter on {hex}"));
    }
    let unreachable_exits = grids
        .values()
        .filter(|hexes| !hexes.iter().any(|h| comps.of(*h) == Some(0)))
        .count();
    println!(
        "exits {} ({} outside the largest component), returns {}, critters {}",
        grids.len(),
        unreachable_exits,
        returns.len(),
        critters.len()
    );

    // Openable hexes, as `BuildOptions::reconnect` sees them.
    let mut cost: Vec<Option<OpenCost>> = (0..HEXES as i32)
        .map(|hex| {
            let h = hex as usize;
            if walkable[h] || protected[h] || on_border(hex) {
                return None;
            }
            let c = match free[h] {
                Verdict::Walkable => OpenCost::Standable,
                Verdict::OffNavmesh => {
                    let [x, y] = frame.source_of_tile(frame.hex_centre_tile(hex)?);
                    if sampler.slope(x as f32, y as f32)? > options.slope_limit {
                        return None;
                    }
                    OpenCost::OffNavmesh
                }
                _ => return None,
            };
            (!cover.stands_on(hex)).then_some(c)
        })
        .collect();
    let mut open = walkable.clone();
    let cost0 = cost.clone();
    // Why each closed hex is closed, for the breakdown of a barrier.
    let why = |hex: i32| -> &'static str {
        let h = hex as usize;
        if on_border(hex) {
            "border"
        } else if critters.contains(&hex) {
            "critter"
        } else if strip_hex[h] && cover.stands_on(hex) {
            "strip, wall art standing"
        } else if strip_hex[h] && !strips_open {
            "strip"
        } else if protected[h]
            && placements
                .iter()
                .any(|p| p.hex == hex && p.role == Role::Door)
        {
            "door"
        } else if protected[h]
            && neighbours(hex).any(|n| {
                placements
                    .iter()
                    .any(|p| p.hex == n && p.role == Role::Door)
            })
            && !strip_hex[h]
            && !blocked[h]
        {
            "door's doorstep"
        } else if protected[h] {
            "blocking scenery or doorstep"
        } else if !matches!(free[h], Verdict::Walkable | Verdict::OffNavmesh) {
            "steep or no ground"
        } else if cover.stands_on(hex) {
            "landmark art standing"
        } else if cost0[h].is_none() {
            "steep (off navmesh)"
        } else {
            "openable"
        }
    };
    let anything: Vec<Option<OpenCost>> = (0..HEXES as i32)
        .map(|h| (!walkable[h as usize] && !on_border(h)).then_some(OpenCost::Standable))
        .collect();
    let main_hex = comps.first_hex(0).unwrap();
    let mut current = components(|h| open[h as usize]);
    let mut joined = 0;
    for (id, holds) in &holdings {
        let island_hex = comps.first_hex(*id).unwrap();
        let (Some(m), Some(i)) = (current.of(main_hex), current.of(island_hex)) else {
            println!("  island {id} (hex {island_hex}): not in the open grid");
            continue;
        };
        if m == i {
            println!("  island {id}: joined by an earlier passage");
            continue;
        }
        let reached = current.mask(m);
        let target = current.mask(i);
        let needed = shortest_passage(&reached, &target, &cost, None);
        let needed_len = needed.as_ref().map(|p| p.hexes.len());
        let off = needed.as_ref().map(|p| p.off_navmesh);
        let crossed = |p: &islands::Passage| {
            let strips = p.hexes.iter().filter(|h| strip_hex[**h as usize]).count();
            let unblocked = p.hexes.iter().filter(|h| !blocked[**h as usize]).count();
            (strips, unblocked)
        };
        print!(
            "  island {id} ({} hexes, first {island_hex}) holds {}: shortest passage {:?} (off navmesh {:?}, (strip hexes, unblocked hexes) {:?})",
            comps.sizes()[*id],
            holds.join(", "),
            needed_len,
            off,
            needed.as_ref().map(crossed)
        );
        match needed {
            Some(p) if p.hexes.len() <= limit => {
                for hex in widen(&p.hexes, 0, &reached, &cost) {
                    open[hex as usize] = true;
                    cost[hex as usize] = None;
                }
                joined += 1;
                current = components(|h| open[h as usize]);
                println!(" -> opened");
            }
            _ => {
                println!(" -> left");
                if let Some(p) = shortest_passage(&reached, &target, &anything, None) {
                    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
                    for hex in &p.hexes {
                        *counts.entry(why(*hex)).or_default() += 1;
                    }
                    println!("    thinnest barrier {} hexes: {counts:?}", p.hexes.len());
                    if p.hexes.len() <= 3 {
                        for hex in &p.hexes {
                            let [cx, cy] = HexGrid::default().hex_center(*hex).unwrap();
                            print!(
                                "    hex {hex} (row {}): {}, object-blocked {}, free {:?};",
                                hex / 200,
                                why(*hex),
                                blocked[*hex as usize],
                                free[*hex as usize]
                            );
                            for pl in &placements {
                                let a = &art[pl.art];
                                if pl.role != Role::Scenery || a.width < 96 {
                                    continue;
                                }
                                let c = HexGrid::default().hex_center(pl.hex).unwrap();
                                let left =
                                    c[0] + i64::from(a.shift[0]) + i64::from(pl.displacement[0])
                                        - i64::from(a.width / 2);
                                let top =
                                    c[1] + i64::from(a.shift[1]) + i64::from(pl.displacement[1])
                                        - i64::from(a.height)
                                        + 1;
                                let (x, y) = (cx - left, cy - top);
                                if x >= 0
                                    && y >= 0
                                    && x < i64::from(a.width)
                                    && y < i64::from(a.height)
                                    && a.pixels[(y * i64::from(a.width) + x) as usize] != 0
                                {
                                    print!(
                                        " band at {} (row {}, {}x{}, blocking {})",
                                        pl.hex,
                                        pl.hex / 200,
                                        a.width,
                                        a.height,
                                        pl.blocking
                                    );
                                }
                            }
                            println!();
                        }
                    }
                }
            }
        }
    }
    let main_now = current.of(main_hex);
    let reach_exits = grids
        .values()
        .filter(|hexes| {
            hexes
                .iter()
                .any(|h| current.around(*h).is_some() && current.around(*h) == main_now)
        })
        .count();
    let reach_returns = returns
        .iter()
        .filter(|h| current.of(**h).is_some() && current.of(**h) == main_now)
        .count();
    println!(
        "with passages up to {limit}: {joined} opened; exits reachable {reach_exits}/{}, returns {reach_returns}/{}, largest {}",
        grids.len(),
        returns.len(),
        main_now.map_or(0, |m| current.sizes()[m])
    );

    // The entrance the new rule picks on the joined grid.
    let centre = frame.tile_of_source(
        f64::from(window.x_west) + f64::from(window.size) / 2.0,
        f64::from(window.y_north) - f64::from(window.size) / 2.0,
    );
    let nearest = walk::nearest_walkable(&frame, &open, centre, &BTreeSet::new()).unwrap();
    let main_now = current.of(main_hex);
    let mut candidates: Vec<(f64, i32)> = (0..HEXES as i32)
        .filter(|h| main_now.is_some() && current.of(*h) == main_now && !on_border(*h))
        .filter(|h| !returns.contains(h) && !grids.values().any(|g| g.contains(h)))
        .map(|h| {
            let c = frame.hex_centre_tile(h).unwrap();
            ((c[0] - centre[0]).powi(2) + (c[1] - centre[1]).powi(2), h)
        })
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut pick = None;
    let mut visible = None;
    let mut tiers: BTreeMap<String, usize> = BTreeMap::new();
    for (_, hex) in &candidates {
        let sight = cover.sight(*hex, map_builder::ENTRANCE_BOX);
        *tiers.entry(format!("{sight:?}")).or_default() += 1;
        if sight == Sight::Clear && pick.is_none() {
            pick = Some(*hex);
        }
        if sight != Sight::Covered && visible.is_none() {
            visible = Some(*hex);
        }
    }
    println!(
        "nearest walkable to the centre {nearest} (component {:?}, sight {:?}); new entrance {:?}, {:?} steps from it; main hexes by sight {tiers:?}",
        current.of(nearest),
        cover.sight(nearest, map_builder::ENTRANCE_BOX),
        pick,
        pick.map(|p| HexGrid::lattice_distance(nearest, p).unwrap())
    );
    println!(
        "not drawn over (clear or behind): {:?}, {:?} steps",
        visible,
        visible.map(|p| HexGrid::lattice_distance(nearest, p).unwrap())
    );

    // Doors with no walkable neighbour: how far their doorstep is.
    for door in doors {
        let ring = (2..=map_builder::exits::DOOR_REACH).find(|r| {
            map_builder::exits::hexes_around(door, *r, true)
                .iter()
                .any(|h| walkable[*h as usize])
        });
        let comp = ring.and_then(|r| {
            map_builder::exits::hexes_around(door, r, true)
                .into_iter()
                .filter(|h| walkable[*h as usize])
                .find_map(|h| current.of(h))
        });
        println!(
            "door {door}: nearest walkable ring {ring:?}, its component {comp:?} (main {main_now:?})"
        );
    }
}
