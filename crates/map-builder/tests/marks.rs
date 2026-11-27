//! Link cues (`map_builder::marks`): a door link and a stair link get
//! distinct, in-palette ground cues on their own hexes; off (and a cap of
//! nothing) is byte-identical; the cap is honoured; walkability and objects
//! never change; two builds are identical.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use common::saloon::{DOOR, SALOON};
use common::scenes::full_base;
use common::*;
use map_builder::marks::{self, Cue, CueKind, LinkCues};
use map_builder::storeys::{self, SplitOptions};
use map_builder::{
    ArtCategory, BuiltLayer, ElevationInput, GroundTiles, LayerInput, LayerOptions, LinkKind,
    MapInput, Placement, Role, build_layer,
};
use mir::{CellDoc, CellLocation, CellRef, FormKey, GroundSampler, NavTriangleDoc, NavmeshDoc};
use vault_formats::map::MapFile;
use vault_formats::{ByteSource, GameProfile, HexGrid, LayeredVfs, LocalPrototypeCatalog};
use vault_formats::{MemoryDir, Vfs};

/// A grey ramp: index `i` is `(i, i, i)`.
fn cues() -> LinkCues {
    LinkCues::new((1..=255u8).map(|i| (i, [i, i, i])).collect())
}

fn mounted(layer: &emit::ModLayer, base: &BTreeMap<String, Vec<u8>>) -> LayeredVfs {
    let tree = |files: Vec<(String, Vec<u8>)>| {
        MemoryDir::new(
            files
                .into_iter()
                .map(|(path, bytes)| (path, ByteSource::new(bytes).unwrap())),
        )
        .unwrap()
    };
    let mut files: Vec<(String, Vec<u8>)> = layer
        .files()
        .iter()
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    files.push((
        "vault-mod.toml".into(),
        layer.manifest_text().unwrap().into_bytes(),
    ));
    LayeredVfs::new(
        Arc::new(tree(base.clone().into_iter().collect())),
        vec![vault_formats::ModLayer::from_tree("test", tree(files))],
    )
}

/// Every map of the layer parsed back, by number.
fn parsed(built: &BuiltLayer, base: &BTreeMap<String, Vec<u8>>) -> BTreeMap<u32, MapFile> {
    let vfs = mounted(&built.layer, base);
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    built
        .layer
        .manifest()
        .maps
        .iter()
        .map(|m| {
            let bytes = vfs.read(&format!("maps/{}", m.file)).unwrap();
            let map = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).unwrap();
            (m.number, map)
        })
        .collect()
}

/// The squares whose floor differs between two builds of the same layer,
/// by map number and elevation.
fn changed_squares(
    on: &BTreeMap<u32, MapFile>,
    off: &BTreeMap<u32, MapFile>,
) -> BTreeMap<(u32, usize), BTreeSet<i32>> {
    let mut out = BTreeMap::new();
    for (number, map) in on {
        for e in 0..3 {
            let (Some(a), Some(b)) = (&map.tiles[e], &off[number].tiles[e]) else {
                assert_eq!(map.tiles[e].is_some(), off[number].tiles[e].is_some());
                continue;
            };
            let changed: BTreeSet<i32> = a
                .iter()
                .zip(b)
                .enumerate()
                .filter(|(_, (x, y))| x.word != y.word)
                .map(|(q, _)| q as i32)
                .collect();
            if !changed.is_empty() {
                out.insert((*number, e), changed);
            }
        }
    }
    out
}

fn tiles_listed(built: &BuiltLayer) -> usize {
    String::from_utf8(built.layer.files()["art/tiles/tiles.lst"].clone())
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// Every exit grid hex and arrival hex of each built map, by number and
/// elevation.
fn link_hexes(built: &BuiltLayer) -> BTreeMap<(u32, usize), BTreeSet<i32>> {
    let mut out: BTreeMap<(u32, usize), BTreeSet<i32>> = BTreeMap::new();
    for map in &built.maps {
        for link in &map.links {
            out.entry((map.number, usize::from(link.elevation)))
                .or_default()
                .extend(link.grid_hexes.iter().copied());
            out.entry((link.to_number, usize::from(link.to_elevation)))
                .or_default()
                .insert(link.to_hex);
        }
    }
    out
}

// The town and the saloon, linked by a door both ways.

fn elevation(scene: &Scene) -> ElevationInput<'_> {
    ElevationInput {
        sampler: &scene.sampler,
        ground: GroundTiles {
            tiles: &scene.tiles,
            squares: &scene.squares,
        },
        art: &scene.art,
        placements: &scene.placements,
        blocked_hexes: &scene.blocked,
        interior: scene.options.interior.clone(),
        level: None,
    }
}

fn map_of(scene: &Scene, mark: Option<LinkCues>) -> MapInput<'_> {
    let mut options = scene.options.clone();
    options.interior = None;
    options.mark_links = mark;
    MapInput {
        map_name: &scene.name,
        map_number: scene.number,
        window: scene.window,
        projection: &scene.projection,
        elevations: vec![elevation(scene)],
        entrance_elevation: None,
        cells: &scene.cells,
        critters: &scene.critters,
        options,
    }
}

const TOWN_DOOR: u32 = 0xe01;

fn town_and_saloon() -> (Scene, Scene) {
    let mut town = town();
    let frame = town.frame();
    let door_hex = hex_at(&frame, 48.0, 48.0);
    let [x, y] = hex_point(&frame, door_hex);
    let centre = town
        .cells
        .iter()
        .position(|c| matches!(c.location, CellLocation::Exterior { x: 0, y: 0, .. }))
        .unwrap();
    let mut door = teleport_door(
        TOWN_DOOR,
        [x, y, 100.0],
        DOOR,
        SALOON,
        Some("SynthSaloon"),
        None,
    );
    door.teleport.as_mut().unwrap().position = [0.0, -200.0, 1000.0];
    town.cells[centre].references.push(door);
    town.cells[centre]
        .references
        .sort_by(|a, b| a.form.cmp(&b.form));
    town.placements.push(placement(
        4,
        door_hex,
        Role::Door,
        true,
        &FormKey::new(PLUGIN, TOWN_DOOR).to_string(),
    ));
    town.base = full_base();
    let outside = town.cells[centre].form.clone();
    let back = hex_point(&frame, HexGrid::step(door_hex, 2, 3).unwrap());

    let mut saloon = common::saloon::saloon(common::saloon::interior(false, None));
    let cell = &mut saloon.cells[0];
    let door = cell
        .references
        .iter_mut()
        .find(|r| r.form == FormKey::new(PLUGIN, DOOR))
        .unwrap();
    let teleport = door.teleport.as_mut().unwrap();
    teleport.door = FormKey::new(PLUGIN, TOWN_DOOR);
    teleport.cell = Some(CellRef {
        form: outside,
        editor_id: Some("SynthCell00".into()),
        location: CellLocation::Exterior {
            worldspace: "synthworld".into(),
            worldspace_form: FormKey::new(PLUGIN, 0x800),
            x: 0,
            y: 0,
        },
    });
    teleport.position = [back[0], back[1], 100.0];
    (town, saloon)
}

fn layer<'a>(
    maps: Vec<MapInput<'a>>,
    base: &'a BTreeMap<String, Vec<u8>>,
    art: &'a [map_builder::CritterArt],
) -> LayerInput<'a> {
    LayerInput {
        layer_id: "synthetic-layer",
        layer_name: "Synthetic layer",
        base_files: base,
        critter_art: art,
        maps,
        stairs: Vec::new(),
        options: LayerOptions::default(),
    }
}

fn door_layer(
    town: &Scene,
    saloon: &Scene,
    base: &BTreeMap<String, Vec<u8>>,
    mark: Option<LinkCues>,
) -> BuiltLayer {
    build_layer(&layer(
        vec![map_of(town, mark.clone()), map_of(saloon, mark)],
        base,
        &saloon.critter_art,
    ))
    .unwrap()
}

// A tower of two storeys, linked by its stairs.

fn quad(x0: f32, x1: f32, z0: f32, z1: f32) -> Vec<[[f32; 3]; 3]> {
    let (y0, y1) = (-300.0, 300.0);
    vec![
        [[x0, y0, z0], [x1, y0, z1], [x1, y1, z1]],
        [[x0, y0, z0], [x1, y1, z1], [x0, y1, z0]],
    ]
}

fn tower_cell(floors: usize) -> CellDoc {
    tower_cell_sized(floors, 400.0)
}

/// A tower whose upper floors are `upper` units either side of their centre
/// east-west (the ground floor 400).
fn tower_cell_sized(floors: usize, upper: f32) -> CellDoc {
    let mut t = Vec::new();
    for f in 0..floors {
        let z = 300.0 * f as f32;
        let x = 1100.0 * f as f32;
        let half = if f == 0 { 400.0 } else { upper };
        t.extend(quad(x - half, x + half, z, z));
        if f + 1 < floors {
            t.extend(quad(x + 400.0, x + 1100.0 - upper, z, z + 300.0));
        }
    }
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for tri in &t {
        let base = vertices.len() as u32;
        vertices.extend_from_slice(tri);
        triangles.push(NavTriangleDoc {
            vertices: [base, base + 1, base + 2],
            links: [-1; 3],
            flags: 0,
        });
    }
    CellDoc {
        schema_version: mir::SCHEMA_VERSION,
        form: FormKey::new(PLUGIN, 0x8000),
        editor_id: Some("SynthTower".into()),
        name: None,
        location: CellLocation::Interior,
        flags: 1,
        water_height: None,
        land: None,
        references: Vec::new(),
        navmeshes: vec![NavmeshDoc {
            form: FormKey::new(PLUGIN, 0x8001),
            vertices,
            triangles,
            cover_triangles: Vec::new(),
            door_links: Vec::new(),
            external_links: Vec::new(),
        }],
    }
}

struct Tower {
    projection: foundry_render::Projection,
    window: mir::MapWindow,
    sampler: GroundSampler,
    tiles: Vec<Vec<u8>>,
    squares: Vec<u16>,
    art: Vec<map_builder::ArtImage>,
    placements: Vec<Vec<Placement>>,
    cells: Vec<CellDoc>,
    split: storeys::StoreySplit,
    layout: storeys::StoreyLayout,
    base: BTreeMap<String, Vec<u8>>,
}

fn tower() -> Tower {
    tower_of(tower_cell(2))
}

fn tower_of(cell: CellDoc) -> Tower {
    let projection = projection();
    let window = mir::MapWindow::from_center([1100.0, 0.0], projection.tile_units());
    let frame = frame(&window, &projection);
    let cells = vec![cell];
    let split = storeys::split_storeys(&cells, &SplitOptions::default());
    let layout = storeys::layout(&split, "tower").unwrap();
    let placements = (0..2)
        .map(|f| {
            vec![placement(
                0,
                frame
                    .hex_of_source(1100.0 * f as f64 - 200.0, -150.0)
                    .unwrap(),
                Role::Scenery,
                true,
                &format!("synthetic.esm:0083{f:02x}"),
            )]
        })
        .collect();
    Tower {
        projection,
        window,
        sampler: GroundSampler::default(),
        tiles: library(),
        squares: squares(),
        art: vec![image(ArtCategory::Scenery, 20, 30, 40, "stool")],
        placements,
        cells,
        split,
        layout,
        base: full_base(),
    }
}

fn tower_layer(t: &Tower, mark: Option<LinkCues>) -> BuiltLayer {
    tower_layer_pruned(t, mark, false)
}

fn tower_layer_pruned(t: &Tower, mark: Option<LinkCues>, prune: bool) -> BuiltLayer {
    let maps: Vec<MapInput<'_>> = t
        .layout
        .maps
        .iter()
        .map(|map| MapInput {
            map_name: &map.name,
            map_number: None,
            window: t.window,
            projection: &t.projection,
            elevations: map
                .floors
                .iter()
                .map(|f| ElevationInput {
                    sampler: &t.sampler,
                    ground: GroundTiles {
                        tiles: &t.tiles,
                        squares: &t.squares,
                    },
                    art: &t.art,
                    placements: &t.placements[*f],
                    blocked_hexes: &[],
                    interior: Some(t.split.floors[*f].interior_build(&SplitOptions::default())),
                    level: None,
                })
                .collect(),
            entrance_elevation: None,
            cells: &t.cells,
            critters: &[],
            options: map_builder::BuildOptions {
                mark_links: mark.clone(),
                ..map_builder::BuildOptions::default()
            },
        })
        .collect();
    let mut input = layer(maps, &t.base, &[]);
    input.stairs = t.layout.stair_links(0);
    input.options.prune_dead_end_stairs = prune;
    build_layer(&input).unwrap()
}

#[test]
fn a_stair_into_a_dead_end_pocket_is_dropped_only_when_asked() {
    // A roomy upper floor: kept either way, the same bytes.
    let roomy = tower();
    let kept = tower_layer_pruned(&roomy, None, true);
    assert_eq!(kept.maps[0].links.len(), 2);
    assert_eq!(kept.layer.files(), tower_layer(&roomy, None).layer.files());
    // A landing of a few hexes with nothing else on it: dropped, both ends.
    let pocket = tower_of(tower_cell_sized(2, 140.0));
    let plain = tower_layer(&pocket, None);
    assert_eq!(
        plain.maps[0].links.len(),
        2,
        "the pocket's stair is built without the option"
    );
    let pruned = tower_layer_pruned(&pocket, None, true);
    assert!(pruned.maps[0].links.is_empty());
    let skipped = &pruned.maps[0].report.skipped_teleports;
    assert!(
        skipped
            .iter()
            .filter(|s| s.reason.contains("dead end"))
            .count()
            >= 2,
        "{skipped:?}"
    );
}

/// What the changed squares of a build must lie in: the squares of every
/// link hex of their map and elevation.
fn assert_changes_on_link_hexes(
    on: &BuiltLayer,
    off: &BuiltLayer,
    base: &BTreeMap<String, Vec<u8>>,
) -> usize {
    let before = parsed(off, base);
    let changed = changed_squares(&parsed(on, base), &before);
    let links = link_hexes(on);
    let mut total = 0;
    for ((number, e), squares) in &changed {
        let allowed: BTreeSet<i32> = links
            .get(&(*number, *e))
            .into_iter()
            .flatten()
            .flat_map(|h| marks::squares_of_hex(*h))
            .collect();
        for q in squares {
            assert!(
                allowed.contains(q),
                "map {number} elevation {e}: square {q} changed off every link hex"
            );
        }
        total += squares.len();
    }
    // And every link hex is marked, unless its floor is blank.
    for ((number, e), hexes) in &links {
        let floor = before[number].tiles[*e].as_ref().unwrap();
        for h in hexes {
            let mine = marks::squares_of_hex(*h);
            let got = changed.get(&(*number, *e)).cloned().unwrap_or_default();
            let blank = mine
                .iter()
                .all(|q| floor[*q as usize].word == vault_formats::map::TilePair::BLANK.word);
            assert!(
                blank || mine.iter().any(|q| got.contains(q)),
                "map {number} elevation {e}: link hex {h} unmarked"
            );
        }
    }
    total
}

/// Walkability, objects and everything but the floor tiles are the same.
fn assert_only_floors_differ(on: &BuiltLayer, off: &BuiltLayer, base: &BTreeMap<String, Vec<u8>>) {
    assert_eq!(on.maps.len(), off.maps.len());
    for (a, b) in on.maps.iter().zip(&off.maps) {
        assert_eq!(a.walkable, b.walkable);
        assert_eq!(a.objects, b.objects);
        assert_eq!(a.links, b.links);
        assert_eq!(a.critter_elevations, b.critter_elevations);
    }
    let (pa, pb) = (parsed(on, base), parsed(off, base));
    for (number, map) in &pa {
        assert_eq!(
            format!("{:?}", map.objects),
            format!("{:?}", pb[number].objects),
            "map {number}"
        );
        assert_eq!(
            format!("{:?}", map.header),
            format!("{:?}", pb[number].header),
            "map {number}"
        );
    }
    for (path, bytes) in off.layer.files() {
        if path.starts_with("maps/") && path.ends_with(".map") || path.starts_with("art/tiles/") {
            continue;
        }
        assert_eq!(on.layer.files().get(path), Some(bytes), "{path}");
    }
}

fn notes(built: &BuiltLayer) -> Vec<String> {
    built
        .maps
        .iter()
        .flat_map(|m| m.report.notes.iter())
        .filter(|n| n.contains("link cues"))
        .cloned()
        .collect()
}

#[test]
fn a_door_link_is_marked_on_its_hexes_and_nothing_else_changes() {
    let (town, saloon) = town_and_saloon();
    let base = full_base();
    let off = door_layer(&town, &saloon, &base, None);
    let on = door_layer(&town, &saloon, &base, Some(cues()));
    assert_eq!(on.maps[0].links[0].kind, LinkKind::Door);
    assert!(notes(&off).is_empty());
    let changed = assert_changes_on_link_hexes(&on, &off, &base);
    assert!(changed > 0);
    assert_only_floors_differ(&on, &off, &base);
    let lines = notes(&on);
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert!(line.ends_with("0 skipped for budget"), "{line}");
        assert!(!line.contains(" 0 tiles added"), "{line}");
    }
    assert!(tiles_listed(&on) > tiles_listed(&off));
    // Two builds: identical.
    let again = door_layer(&town, &saloon, &base, Some(cues()));
    assert_eq!(again.layer.files(), on.layer.files());
    assert_eq!(notes(&again), lines);
}

#[test]
fn a_stair_link_is_marked_on_its_hexes() {
    let t = tower();
    let off = tower_layer(&t, None);
    let on = tower_layer(&t, Some(cues()));
    let map = &on.maps[0];
    assert_eq!(map.links.len(), 2, "{:?}", map.report.skipped_teleports);
    assert!(map.links.iter().all(|l| l.kind == LinkKind::Stairs));
    assert!(assert_changes_on_link_hexes(&on, &off, &t.base) > 0);
    assert_only_floors_differ(&on, &off, &t.base);
    let lines = notes(&on);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[1].starts_with("elevation 1: link cues: "),
        "{lines:?}"
    );
    assert_eq!(
        tower_layer(&t, Some(cues())).layer.files(),
        on.layer.files()
    );
}

#[test]
fn the_cap_is_honoured_and_a_cap_of_nothing_is_off() {
    let (town, saloon) = town_and_saloon();
    let base = full_base();
    let off = door_layer(&town, &saloon, &base, None);
    let full = door_layer(&town, &saloon, &base, Some(cues()));
    let none = door_layer(
        &town,
        &saloon,
        &base,
        Some(LinkCues {
            max_tiles: 0,
            ..cues()
        }),
    );
    // Nothing fits: every byte as with the cues off.
    assert_eq!(none.layer.files(), off.layer.files());
    for line in notes(&none) {
        assert!(line.contains(": 0 hexes marked, 0 tiles added, "), "{line}");
        assert!(!line.ends_with(" 0 skipped for budget"), "{line}");
    }
    // One tile per map: at most one more tile each, the rest skipped.
    let one = door_layer(
        &town,
        &saloon,
        &base,
        Some(LinkCues {
            max_tiles: 1,
            ..cues()
        }),
    );
    assert!(tiles_listed(&one) <= tiles_listed(&off) + 2);
    assert!(tiles_listed(&one) < tiles_listed(&full));
    for line in notes(&one) {
        assert!(
            line.contains(" 1 tiles added") || line.contains(" 0 tiles added"),
            "{line}"
        );
        assert!(!line.ends_with(" 0 skipped for budget"), "{line}");
    }
    assert_only_floors_differ(&one, &off, &base);
}

/// A plain ground: every square the same opaque tile.
fn plain_ground() -> (Vec<Vec<u8>>, Vec<u16>) {
    (vec![vec![100; 80 * 36]], vec![0; 10_000])
}

/// The screen pixels of `q`'s tile that differ between two tiles.
fn changed_pixels(q: i32, before: &[u8], after: &[u8]) -> Vec<([i64; 2], u8)> {
    let corner = HexGrid::default().square_corner(q, false).unwrap();
    (0..80 * 36)
        .filter(|&i| before[i] != after[i])
        .map(|i| {
            (
                [corner[0] + (i % 80) as i64, corner[1] + (i / 80) as i64],
                after[i],
            )
        })
        .collect()
}

#[test]
fn doors_get_a_frame_and_stairs_get_stripes_in_the_palette() {
    let (tiles, squares) = plain_ground();
    let ground = GroundTiles {
        tiles: &tiles,
        squares: &squares,
    };
    let grid = HexGrid::default();
    let door = 100 * 200 + 80;
    let stairs = 100 * 200 + 120;
    let marked = marks::mark_ground(
        ground,
        &[
            Cue {
                hex: door,
                kind: CueKind::Door,
                arrival: false,
            },
            Cue {
                hex: stairs,
                kind: CueKind::Stairs,
                arrival: false,
            },
        ],
        &BTreeSet::new(),
        &cues(),
        100,
    );
    assert_eq!((marked.marked, marked.skipped), (2, 0));
    assert!(marked.added >= 2 && marked.tiles.len() == 1 + marked.added);
    let pick = |p: [i64; 2]| grid.pick_hex([p[0] as i32, p[1] as i32]);
    let mut colours: BTreeMap<CueKind, BTreeSet<u8>> = BTreeMap::new();
    for q in 0..10_000 {
        let index = usize::from(marked.squares[q]);
        if index == 0 {
            continue;
        }
        for (p, colour) in changed_pixels(q as i32, &tiles[0], &marked.tiles[index]) {
            let hex = pick(p).unwrap();
            if hex == door {
                // On the frame: a pixel a step away lies outside the hex.
                assert!(
                    [[-2, 0], [2, 0], [0, -1], [0, 1]]
                        .iter()
                        .any(|d| pick([p[0] + d[0], p[1] + d[1]]) != Some(door)),
                    "door pixel {p:?} inside the frame"
                );
                colours.entry(CueKind::Door).or_default().insert(colour);
            } else {
                assert_eq!(hex, stairs, "pixel {p:?} outside both hexes");
                assert!(marks::stair_tread(p), "{p:?} off a tread");
                colours.entry(CueKind::Stairs).or_default().insert(colour);
            }
        }
    }
    // Both drawn, lighter than the floor, in their own colours.
    let (d, s) = (&colours[&CueKind::Door], &colours[&CueKind::Stairs]);
    assert!(d.iter().chain(s).all(|c| *c > 100 && marks::usable(*c)));
    assert!(d.is_disjoint(s), "{d:?} {s:?}");
    // The frame runs all round the hex; the stripes cross it.
    let changed_door = (0..10_000)
        .filter(|&q| marked.squares[q] != 0)
        .flat_map(|q| {
            changed_pixels(
                q as i32,
                &tiles[0],
                &marked.tiles[usize::from(marked.squares[q])],
            )
        })
        .filter(|(p, _)| pick(*p) == Some(door))
        .count();
    assert!(changed_door >= 30, "{changed_door}");
}

#[test]
fn arrivals_are_lighter_and_cues_never_animate_or_touch_kept_squares() {
    let (tiles, squares) = plain_ground();
    let ground = GroundTiles {
        tiles: &tiles,
        squares: &squares,
    };
    let hex = 100 * 200 + 80;
    let grid_cue = Cue {
        hex,
        kind: CueKind::Door,
        arrival: false,
    };
    let arrival = Cue {
        arrival: true,
        ..grid_cue
    };
    let brightest = |cue: Cue| {
        let m = marks::mark_ground(ground, &[cue], &BTreeSet::new(), &cues(), 100);
        m.tiles[1..].iter().flatten().copied().max().unwrap()
    };
    assert!(brightest(arrival) < brightest(grid_cue));
    // A palette whose nearest lighter entries are colour-cycling ones:
    // never produced.
    let cycling = LinkCues::new((1..=255u8).map(|i| (i, [i, i, i])).collect());
    assert!(cycling.palette.iter().all(|(i, _)| marks::usable(*i)));
    assert!(!marks::usable(0) && !marks::usable(229) && !marks::usable(254));
    // A kept square is never repainted; a hex only in kept squares is not
    // counted at all.
    let keep: BTreeSet<i32> = marks::squares_of_hex(hex);
    let m = marks::mark_ground(ground, &[grid_cue], &keep, &cues(), 100);
    assert_eq!((m.marked, m.added, m.skipped), (0, 0, 0));
    assert_eq!(m.squares, squares);
    // Blank squares are never painted.
    let blank = vec![map_builder::BLANK_SQUARE; 10_000];
    let m = marks::mark_ground(
        GroundTiles {
            tiles: &tiles,
            squares: &blank,
        },
        &[grid_cue],
        &BTreeSet::new(),
        &cues(),
        100,
    );
    assert_eq!(m.squares, blank);
    // Room for nothing: skipped, unchanged.
    let m = marks::mark_ground(ground, &[grid_cue], &BTreeSet::new(), &cues(), 0);
    assert_eq!((m.marked, m.added, m.skipped), (0, 0, 1));
    assert!(!m.changed(&ground));
}

// Lower-level context (`map_builder::context`): the upper storey of the
// tower shows the storey below, dimmed, in its blank floor near its own
// walkable hexes; nothing but floor tiles changes.

/// The tower with its own ground per storey.
fn tower_layer_with(t: &Tower, grounds: &[(Vec<Vec<u8>>, Vec<u16>)]) -> BuiltLayer {
    let maps: Vec<MapInput<'_>> = t
        .layout
        .maps
        .iter()
        .map(|map| MapInput {
            map_name: &map.name,
            map_number: None,
            window: t.window,
            projection: &t.projection,
            elevations: map
                .floors
                .iter()
                .enumerate()
                .map(|(e, f)| ElevationInput {
                    sampler: &t.sampler,
                    ground: GroundTiles {
                        tiles: &grounds[e].0,
                        squares: &grounds[e].1,
                    },
                    art: &t.art,
                    placements: &t.placements[*f],
                    blocked_hexes: &[],
                    interior: Some(t.split.floors[*f].interior_build(&SplitOptions::default())),
                    level: None,
                })
                .collect(),
            entrance_elevation: None,
            cells: &t.cells,
            critters: &[],
            options: map_builder::BuildOptions::default(),
        })
        .collect();
    let mut input = layer(maps, &t.base, &[]);
    input.stairs = t.layout.stair_links(0);
    build_layer(&input).unwrap()
}

#[test]
fn context_fills_the_upper_storeys_blank_floor_and_nothing_else_changes() {
    use map_builder::context::{self, Composite, ContextLook};
    let t = tower();
    // Storey 0: the whole floor drawn. Storey 1: only the squares under its
    // own walkable hexes; the rest blank, as an upper storey is built.
    let probe = tower_layer(&t, None);
    let upper: &[bool] = &probe.maps[0].walkable[1];
    let own: BTreeSet<i32> = (0..40_000)
        .filter(|h| upper[*h as usize])
        .filter_map(map_builder::walk::Frame::square_of_hex)
        .collect();
    assert!(!own.is_empty());
    let lower = (t.tiles.clone(), t.squares.clone());
    let blank_upper: Vec<u16> = (0..10_000)
        .map(|q| {
            if own.contains(&q) {
                0
            } else {
                map_builder::BLANK_SQUARE
            }
        })
        .collect();
    let off = tower_layer_with(&t, &[lower.clone(), (t.tiles.clone(), blank_upper.clone())]);

    // The picture of storey 0: its floor (indices 10 and 20), painted over
    // the map's screen space, a grey palette.
    let (origin, width, height) = ([-200, -4000], 8400u32, 8000u32);
    let mut pixels = vec![0u8; (width * height) as usize];
    let grid = HexGrid::default();
    for q in 0..10_000i32 {
        let corner = grid.square_corner(q, false).unwrap();
        let tile = &lower.0[usize::from(lower.1[q as usize])];
        for (i, &value) in tile.iter().enumerate().take(80 * 36) {
            let (x, y) = (corner[0] + (i % 80) as i64, corner[1] + (i / 80) as i64);
            if grid.pick_square([x as i32, y as i32], false) != Some(q) {
                continue;
            }
            let (cx, cy) = (x - i64::from(origin[0]), y - i64::from(origin[1]));
            if (0..i64::from(width)).contains(&cx) && (0..i64::from(height)).contains(&cy) {
                pixels[(cy * i64::from(width) + cx) as usize] = value;
            }
        }
    }
    let palette: [[u8; 3]; 256] = std::array::from_fn(|i| [i as u8; 3]);
    let near = context::reach_squares(upper, 6);
    let painted = context::paint(
        &Composite {
            pixels: &pixels,
            origin,
            width,
            height,
            palette: &palette,
        },
        &near,
        &ContextLook {
            dim: 0.35,
            desaturate: 0.25,
            shift: 96,
        },
    );
    // Quantized on the grey ramp: every context square a tile, appended.
    let mut tiles = t.tiles.clone();
    let mut squares = blank_upper.clone();
    for c in painted.iter().filter(|c| !own.contains(&c.square)) {
        let tile: Vec<u8> = c
            .rgba
            .chunks(4)
            .map(|p| if p[3] == 0 { 0 } else { p[0].max(1) })
            .collect();
        let index = match tiles.iter().position(|x| *x == tile) {
            Some(i) => i,
            None => {
                tiles.push(tile);
                tiles.len() - 1
            }
        };
        squares[c.square as usize] = index as u16;
    }
    let on = tower_layer_with(&t, &[lower.clone(), (tiles.clone(), squares.clone())]);

    // The blank squares near the storey's walkable hexes are drawn now.
    let filled: Vec<i32> = (0..10_000)
        .filter(|&q| {
            blank_upper[q as usize] == map_builder::BLANK_SQUARE
                && squares[q as usize] != map_builder::BLANK_SQUARE
        })
        .collect();
    assert!(
        filled.len() > own.len(),
        "{} vs {}",
        filled.len(),
        own.len()
    );
    assert!(filled.iter().all(|q| near.contains(q)));
    let (pon, poff) = (parsed(&on, &t.base), parsed(&off, &t.base));
    for (number, map) in &pon {
        let a = map.tiles[1].as_ref().unwrap();
        let b = poff[number].tiles[1].as_ref().unwrap();
        let blank = vault_formats::map::TilePair::BLANK.word;
        let drawn =
            |v: &[vault_formats::map::TilePair]| v.iter().filter(|p| p.word != blank).count();
        assert!(
            drawn(a) > drawn(b),
            "map {number}: {} vs {}",
            drawn(a),
            drawn(b)
        );
        assert_eq!(map.tiles[0], poff[number].tiles[0]);
    }
    // Walkable hexes, blockers, objects and links: as without it.
    assert_only_floors_differ(&on, &off, &t.base);
    let check = |b: &BuiltLayer| map_builder::verify_links(&b.layer, &t.base).unwrap();
    assert_eq!(check(&on), check(&off));
    assert!(tiles_listed(&on) <= 4096);
    assert!(tiles_listed(&on) > tiles_listed(&off));
    // Two builds: identical.
    let again = tower_layer_with(&t, &[lower, (tiles, squares)]);
    assert_eq!(again.layer.files(), on.layer.files());
}
