//! Exterior levels (`map_builder::levels`, crate notes "Exterior levels")
//! on a synthetic walled town: a crater floor on the terrain, a ring
//! catwalk 300 units over it with two doors into interiors, a ramp from
//! the floor up to the catwalk, a gate in the north wall, and the open
//! ground outside the wall (cells without a navmesh, judged by slope) that
//! is larger than the town. Also the exterior entrance target.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use common::scenes::full_base;
use common::*;
use map_builder::levels::{LevelOptions, RegionKind, split_levels};
use map_builder::walk::{self, Frame, Verdict};
use map_builder::{
    ArtCategory, BuildOptions, BuiltLayer, ElevationInput, ExteriorLevel, LayerInput, LayerOptions,
    LinkKind, MapInput, Placement, Role, build_layer,
};
use mir::{CellDoc, CellLocation, FormKey, GroundSampler, NavTriangleDoc, NavmeshDoc, Reference};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::{ByteSource, GameProfile, LayeredVfs, LocalPrototypeCatalog, MemoryDir, Vfs};

const FLOOR_Z: f32 = 100.0;
const DECK_Z: f32 = 400.0;
const RING_DOORS: [u32; 2] = [0xd101, 0xd102];
const GATE: u32 = 0xd100;
/// Where the gate lets a traveller in: on the floor, inside the wall.
const GATE_ARRIVAL: [f32; 2] = [2000.0, 2560.0];

/// A navmesh of 3D triangles (corners by position: shared edges are found
/// by position).
fn navmesh3(form: u32, triangles: &[[[f32; 3]; 3]]) -> NavmeshDoc {
    let mut vertices = Vec::new();
    let mut docs = Vec::new();
    for t in triangles {
        let base = vertices.len() as u32;
        vertices.extend_from_slice(t);
        docs.push(NavTriangleDoc {
            vertices: [base, base + 1, base + 2],
            links: [-1; 3],
            flags: 0,
        });
    }
    NavmeshDoc {
        form: FormKey::new(PLUGIN, form),
        vertices,
        triangles: docs,
        cover_triangles: Vec::new(),
        door_links: Vec::new(),
        external_links: Vec::new(),
    }
}

/// The rectangle `[x0, x1] × [y0, y1]` as two triangles, its height
/// `z(x, y)`.
fn cell_quad(
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    z: impl Fn(f32, f32) -> f32,
) -> Vec<[[f32; 3]; 3]> {
    let p = |x: f32, y: f32| [x, y, z(x, y)];
    vec![
        [p(x0, y0), p(x1, y0), p(x1, y1)],
        [p(x0, y0), p(x1, y1), p(x0, y1)],
    ]
}

/// Grid lines shared by the floor, the ring and the ramp, so that their
/// edges meet corner to corner.
const XS: [f32; 8] = [
    1200.0, 1300.0, 1600.0, 1900.0, 2100.0, 2400.0, 2700.0, 2800.0,
];
const YS: [f32; 7] = [1200.0, 1300.0, 1600.0, 1900.0, 2200.0, 2500.0, 2600.0];

/// The ramp's cell: x 1900 to 2100, y 1600 (top, the catwalk) to 1900
/// (foot, the floor).
fn on_ramp(x0: f32, y0: f32) -> bool {
    x0 == 1900.0 && y0 == 1600.0
}

/// Whether grid cell `(x0, y0)` is part of the ring catwalk: x 1300 to
/// 2700, y 1300 to 2500, less the hole x 1600 to 2400, y 1600 to 2200.
fn on_ring(x0: f32, x1: f32, y0: f32, y1: f32) -> bool {
    let inside = x0 >= 1300.0 && x1 <= 2700.0 && y0 >= 1300.0 && y1 <= 2500.0;
    let hole = x0 >= 1600.0 && x1 <= 2400.0 && y0 >= 1600.0 && y1 <= 2200.0;
    inside && !hole
}

fn town_navmesh() -> Vec<NavmeshDoc> {
    let mut floor = Vec::new();
    let mut ring = Vec::new();
    let mut ramp = Vec::new();
    for xi in 0..XS.len() - 1 {
        for yi in 0..YS.len() - 1 {
            let (x0, x1, y0, y1) = (XS[xi], XS[xi + 1], YS[yi], YS[yi + 1]);
            if on_ramp(x0, y0) {
                // From the deck at y 1600 down to the floor at y 1900:
                // slope 1, a connector.
                ramp.extend(cell_quad(x0, x1, y0, y1, |_, y| {
                    DECK_Z - (y - 1600.0) / 300.0 * (DECK_Z - FLOOR_Z)
                }));
                continue;
            }
            // The floor everywhere else, under the ring too.
            floor.extend(cell_quad(x0, x1, y0, y1, |_, _| FLOOR_Z));
            if on_ring(x0, x1, y0, y1) {
                ring.extend(cell_quad(x0, x1, y0, y1, |_, _| DECK_Z));
            }
        }
    }
    vec![
        navmesh3(0x7000, &floor),
        navmesh3(0x7001, &ring),
        navmesh3(0x7002, &ramp),
    ]
}

fn reference(form: u32, position: [f32; 3], kind: &str) -> Reference {
    let mut r = teleport_door(form, position, 0, 0, None, None);
    r.teleport = None;
    r.base_kind = Some(kind.into());
    r.kind = mir::RefKind::Npc;
    r
}

/// The town's one cell (0, 0) with its navmesh, doors and residents, and
/// the cells around it without a navmesh.
fn cells(window: &mir::MapWindow) -> Vec<CellDoc> {
    let mut out = Vec::new();
    let (xs, ys) = window.cells(0);
    for y in ys {
        for x in xs.clone() {
            out.push(exterior_cell(x, y));
        }
    }
    let cell = out
        .iter_mut()
        .find(|c| matches!(c.location, CellLocation::Exterior { x: 0, y: 0, .. }))
        .unwrap();
    cell.navmeshes = town_navmesh();
    // Two doors on the catwalk into interiors outside the build.
    cell.references.push(teleport_door(
        RING_DOORS[0],
        [1450.0, 2450.0, DECK_Z],
        0xe101,
        0xf101,
        Some("RingHouseWest"),
        None,
    ));
    cell.references.push(teleport_door(
        RING_DOORS[1],
        [2550.0, 2450.0, DECK_Z],
        0xe102,
        0xf102,
        Some("RingHouseEast"),
        None,
    ));
    // The gate leads to another worldspace's exterior.
    cell.references.push(teleport_door(
        GATE,
        [2000.0, 2650.0, FLOOR_Z],
        0xe100,
        0xf100,
        Some("OutsideCell"),
        Some((5, 5)),
    ));
    cell.references
        .push(reference(0xc001, [2000.0, 2000.0, FLOOR_Z], "NPC_"));
    cell.references
        .push(reference(0xc002, [2550.0, 1950.0, DECK_Z], "NPC_"));
    out
}

struct Town {
    projection: foundry_render::Projection,
    window: mir::MapWindow,
    sampler: GroundSampler,
    tiles: Vec<Vec<u8>>,
    squares: Vec<u16>,
    art: Vec<map_builder::ArtImage>,
    /// Per level.
    placements: [Vec<Placement>; 2],
    cells: Vec<CellDoc>,
    critters: Vec<map_builder::CritterPlacement>,
    critter_art: Vec<map_builder::CritterArt>,
    base: BTreeMap<String, Vec<u8>>,
}

impl Town {
    fn frame(&self) -> Frame {
        frame(&self.window, &self.projection)
    }
}

fn town_scene() -> Town {
    let projection = projection();
    let window = window(&projection);
    let frame = frame(&window, &projection);
    let door_at = |x: f64, y: f64| frame.hex_of_source(x, y).unwrap();
    let key = |form: u32| FormKey::new(PLUGIN, form).to_string();
    Town {
        sampler: sampler(&window, flat),
        tiles: library(),
        squares: squares(),
        art: vec![image(ArtCategory::Scenery, 24, 50, 70, "door")],
        placements: [
            vec![placement(
                0,
                door_at(2000.0, 2650.0),
                Role::Door,
                true,
                &key(GATE),
            )],
            vec![
                placement(
                    0,
                    door_at(1450.0, 2450.0),
                    Role::Door,
                    true,
                    &key(RING_DOORS[0]),
                ),
                placement(
                    0,
                    door_at(2550.0, 2450.0),
                    Role::Door,
                    true,
                    &key(RING_DOORS[1]),
                ),
            ],
        ],
        cells: cells(&window),
        critters: vec![
            critter(0, &key(0xc001), "Floor walker", [2000.0, 2000.0], 0.0),
            critter(0, &key(0xc002), "Deck walker", [2550.0, 1950.0], 0.0),
        ],
        critter_art: vec![sprite_set("resident", 3)],
        base: full_base(),
        projection,
        window,
    }
}

fn elevation<'a>(t: &'a Town, level: usize, exterior: Option<ExteriorLevel>) -> ElevationInput<'a> {
    ElevationInput {
        sampler: &t.sampler,
        ground: map_builder::GroundTiles {
            tiles: &t.tiles,
            squares: &t.squares,
        },
        art: &t.art,
        placements: &t.placements[level],
        blocked_hexes: &[],
        interior: None,
        level: exterior,
    }
}

fn parsed(built: &BuiltLayer, base: &BTreeMap<String, Vec<u8>>) -> BTreeMap<u32, MapFile> {
    let tree = |files: Vec<(String, Vec<u8>)>| {
        MemoryDir::new(
            files
                .into_iter()
                .map(|(path, bytes)| (path, ByteSource::new(bytes).unwrap())),
        )
        .unwrap()
    };
    let mut files: Vec<(String, Vec<u8>)> = built
        .layer
        .files()
        .iter()
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    files.push((
        "vault-mod.toml".into(),
        built.layer.manifest_text().unwrap().into_bytes(),
    ));
    let vfs = LayeredVfs::new(
        Arc::new(tree(base.clone().into_iter().collect())),
        vec![vault_formats::ModLayer::from_tree("test", tree(files))],
    );
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    built
        .layer
        .manifest()
        .maps
        .iter()
        .map(|m| {
            let bytes = vfs.read(&format!("maps/{}", m.file)).unwrap();
            (
                m.number,
                MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2).unwrap(),
            )
        })
        .collect()
}

/// Exit grids of one elevation: hex -> (map, hex, elevation).
fn grids(map: &MapFile, elevation: usize) -> BTreeMap<i32, (i32, i32, i32)> {
    map.objects.elevations[elevation]
        .iter()
        .filter_map(|object| match object.state {
            ObjectState::Other {
                detail:
                    ObjectDetail::ExitGrid {
                        map,
                        hex,
                        elevation,
                        ..
                    },
                ..
            } => Some((object.hex.unwrap(), (map, hex, elevation))),
            _ => None,
        })
        .collect()
}

fn inside_town(frame: &Frame, hex: i32) -> bool {
    let p = hex_point(frame, hex);
    (1200.0..=2800.0).contains(&p[0]) && (1200.0..=2600.0).contains(&p[1])
}

#[test]
fn the_ring_catwalk_is_level_one_and_the_ramp_its_transition() {
    let t = town_scene();
    let frame = t.frame();
    let split = split_levels(&t.cells, &frame, &t.sampler, &LevelOptions::default());
    assert!(split.neighbours);
    assert_eq!(split.level_count(), 2, "{:#?}", split.summary());
    assert_eq!(split.needed_levels, 2);
    assert!(split.unplaced.is_empty());
    // The floor (with the ramp next to it, level 0: nothing lies under the
    // ramp) and the ring (level 1, over the floor).
    let ring: Vec<_> = split
        .regions
        .iter()
        .filter(|r| r.level == Some(1))
        .collect();
    assert_eq!(ring.len(), 1, "{:#?}", split.regions);
    assert_eq!(ring[0].kind, RegionKind::Floor);
    assert_eq!(ring[0].triangles, 2 * 14);
    assert_eq!(ring[0].z_range, [DECK_Z, DECK_Z]);
    let ramp = split
        .regions
        .iter()
        .find(|r| r.kind == RegionKind::Connector)
        .unwrap();
    assert_eq!(ramp.level, Some(0));
    // One transition, at the ramp's head, each end on its own side.
    assert_eq!(split.links.len(), 1, "{:#?}", split.links);
    let link = &split.links[0];
    assert_eq!(link.levels, [0, 1]);
    assert!(
        link.points[0][1] > 1600.0 && link.points[1][1] < 1600.0,
        "{link:?}"
    );
    assert!((link.points[0][0] - 2000.0).abs() < 40.0, "{link:?}");
    // Points find their levels: the floor under the ring is level 0, the
    // deck over it level 1; a door standing in its wall finds the nearest
    // surface.
    assert_eq!(split.level_of_point([2000.0, 1450.0, FLOOR_Z]), Some(0));
    assert_eq!(split.level_of_point([2000.0, 1450.0, DECK_Z]), Some(1));
    assert_eq!(split.level_of_point([2000.0, 2650.0, FLOOR_Z]), Some(0));
    assert_eq!(
        split.reference_level([1450.0, 2450.0, DECK_Z + 10.0]),
        Some(1)
    );
    assert_eq!(
        split.reference_level([1450.0, 2450.0, FLOOR_Z + 10.0]),
        Some(0)
    );
    assert_eq!(split.surface_height(1, 1450.0, 2450.0), Some(DECK_Z));
    assert_eq!(split.surface_height(1, 2000.0, 2000.0), None);
    // Today's one level drops the ring as too high: its hexes are floor.
    let one = walk::verdicts(&frame, &t.sampler, &t.cells, &[], &BuildOptions::default()).1;
    assert_eq!(one.triangles_too_high, 28);
    // Per level: the ring's hexes are walkable on level 1 only above the
    // floor, and nothing outside the ring is.
    let options = |level: u8| BuildOptions {
        level: Some(split.exterior_level(level)),
        ..BuildOptions::default()
    };
    let (upper, stats) = walk::verdicts(&frame, &t.sampler, &t.cells, &[], &options(1));
    assert_eq!(stats.triangles_rasterized, 28);
    assert_eq!(stats.slope_only, 0);
    let deck = frame.hex_of_source(1450.0, 2000.0).unwrap();
    let hole = frame.hex_of_source(2000.0, 2000.0).unwrap();
    let outside = frame.hex_of_source(6000.0, 6000.0).unwrap();
    assert_eq!(upper[deck as usize], Verdict::Walkable);
    assert_eq!(upper[hole as usize], Verdict::OffNavmesh);
    assert_eq!(upper[outside as usize], Verdict::NoGround);
    let (ground, _) = walk::verdicts(&frame, &t.sampler, &t.cells, &[], &options(0));
    assert_eq!(ground[deck as usize], Verdict::Walkable);
    assert_eq!(ground[hole as usize], Verdict::Walkable);
    assert_eq!(ground[outside as usize], Verdict::Walkable, "slope alone");
    // The standable hexes of each level (the slicer's open hexes).
    let standable = walk::standable_hexes(&frame, &t.sampler, &t.cells, &options(1));
    assert!(standable.contains(&deck) && !standable.contains(&hole));
}

fn town_layer<'a>(t: &'a Town, entrance: Option<[f32; 2]>) -> LayerInput<'a> {
    let frame = t.frame();
    let split = split_levels(&t.cells, &frame, &t.sampler, &LevelOptions::default());
    let map = MapInput {
        map_name: "crater",
        map_number: None,
        window: t.window,
        projection: &t.projection,
        elevations: vec![
            elevation(t, 0, Some(split.exterior_level(0))),
            elevation(t, 1, Some(split.exterior_level(1))),
        ],
        entrance_elevation: Some(0),
        cells: &t.cells,
        critters: &t.critters,
        options: BuildOptions {
            entrance_near: entrance,
            ..BuildOptions::default()
        },
    };
    LayerInput {
        layer_id: "synthetic-town",
        layer_name: "Synthetic town",
        base_files: &t.base,
        critter_art: &t.critter_art,
        maps: vec![map],
        stairs: split.stair_links(0),
        options: LayerOptions::default(),
    }
}

#[test]
fn the_town_is_one_map_of_two_elevations_linked_by_the_ramp() {
    let t = town_scene();
    let frame = t.frame();
    let built = build_layer(&town_layer(&t, Some(GATE_ARRIVAL))).unwrap();
    let map = &built.maps[0];
    assert_eq!(map.elevations.len(), 2);
    // The entrance is inside the wall, by the gate, not on the open ground
    // outside (the largest component).
    assert!(
        inside_town(&frame, map.report.entry_hex),
        "{}",
        map.report.entry_hex
    );
    // The ramp's head: grids both ways, to the map's own number.
    let stairs: Vec<_> = map
        .links
        .iter()
        .filter(|l| l.kind == LinkKind::Stairs)
        .collect();
    assert_eq!(stairs.len(), 2, "{:?}", map.report.skipped_teleports);
    for link in &stairs {
        assert_eq!(link.to_number, map.number);
        assert_ne!(link.elevation, link.to_elevation);
        assert!(link.returns);
        assert!(map.walkable[usize::from(link.to_elevation)][link.to_hex as usize]);
    }
    // The catwalk doors become exits from elevation 1, their stub maps
    // leading back to elevation 1.
    assert_eq!(
        map.report.exits.len(),
        2,
        "{:?}",
        map.report.skipped_teleports
    );
    let files = parsed(&built, &t.base);
    let file = &files[&map.number];
    let upper = grids(file, 1);
    for exit in &map.report.exits {
        for hex in &exit.grid_hexes {
            assert_eq!(upper[hex].0 as u32, exit.stub_number);
            assert!(map.walkable[1][*hex as usize]);
        }
        let stub = &files[&exit.stub_number];
        for (to_map, to_hex, to_e) in grids(stub, 0).values() {
            assert_eq!(
                (*to_map as u32, *to_hex, *to_e),
                (map.number, exit.return_hex, 1)
            );
        }
        assert!(map.walkable[1][exit.return_hex as usize]);
    }
    // The gate leads to another exterior: reported, not converted.
    let gate = FormKey::new(PLUGIN, GATE).to_string();
    assert!(
        map.report
            .skipped_teleports
            .iter()
            .any(|s| s.door == gate && s.reason.contains("another exterior")),
        "{:?}",
        map.report.skipped_teleports
    );
    // Residents on the elevation of the surface they stand on.
    assert_eq!(map.critter_elevations, vec![0, 1]);
    assert_eq!(map.elevations[1].critters.len(), 1);
    // Every elevation written, ids unique across them; the entrance on 0.
    assert!(file.tiles[0].is_some() && file.tiles[1].is_some());
    assert_eq!(file.header.entry_elevation, 0);
    let ids: Vec<u32> = file
        .objects
        .elevations
        .iter()
        .flatten()
        .map(|o| o.id)
        .collect();
    assert_eq!(
        ids.len(),
        ids.iter().copied().collect::<BTreeSet<_>>().len()
    );
    // From the entrance a walker reaches the ramp head on elevation 0, and
    // from the arrival there every exit grid on elevation 1.
    let main = &map.elevations[0].components;
    let down = stairs.iter().find(|l| l.elevation == 0).unwrap();
    assert!(
        down.grid_hexes
            .iter()
            .any(|h| main.main.is_some() && main_component(&built, 0, *h) == main.main)
    );
    let up = &map.elevations[1];
    for exit in &map.report.exits {
        assert!(
            exit.grid_hexes
                .iter()
                .any(|h| main_component(&built, 1, *h) == up.components.main)
        );
    }
    assert!(built.links.one_way.is_empty());
    for line in map
        .report
        .notes
        .iter()
        .filter(|n| n.contains("exterior level"))
    {
        assert!(line.contains("navmesh triangles"), "{line}");
    }
}

/// The component of `hex` on elevation `e` of the first map, by the
/// written map's objects.
fn main_component(built: &BuiltLayer, e: usize, hex: i32) -> Option<usize> {
    let map = &built.maps[0];
    let comps = &map.elevations[e].components;
    // Components are listed by size with their lowest hex; recompute from
    // the walkable grid (grids do not stop movement).
    let walkable = &map.walkable[e];
    let all = map_builder::islands::components(|h| walkable[h as usize]);
    let id = all.of(hex)?;
    let first = all.first_hex(id)?;
    comps.first_hexes.iter().position(|f| *f == first)
}

#[test]
fn without_a_target_the_entrance_is_outside_the_wall() {
    let t = town_scene();
    let frame = t.frame();
    let built = build_layer(&town_layer(&t, None)).unwrap();
    let report = &built.maps[0].report;
    assert!(!inside_town(&frame, report.entry_hex));
    assert!(report.entrance.in_main);
    // With the target, in the component nearest it.
    let built = build_layer(&town_layer(&t, Some(GATE_ARRIVAL))).unwrap();
    let report = &built.maps[0].report;
    assert!(inside_town(&frame, report.entry_hex));
    assert!(report.entrance.in_main, "in the component sought");
    // Deterministic.
    let again = build_layer(&town_layer(&t, Some(GATE_ARRIVAL))).unwrap();
    assert_eq!(again.layer.files(), built.layer.files());
}

#[test]
fn a_one_map_exterior_takes_the_entrance_target_too() {
    // The town of the other tests as one flat level through `build_map`:
    // the entrance target alone moves the entrance into the wall.
    let t = town_scene();
    let frame = t.frame();
    let input = |entrance: Option<[f32; 2]>| map_builder::BuildInput {
        map_name: "crater",
        map_number: None,
        layer_id: "synthetic-town",
        layer_name: "Synthetic town",
        window: t.window,
        sampler: &t.sampler,
        projection: &t.projection,
        ground: map_builder::GroundTiles {
            tiles: &t.tiles,
            squares: &t.squares,
        },
        art: &t.art,
        placements: &t.placements[0],
        blocked_hexes: &[],
        cells: &t.cells,
        base_files: &t.base,
        critter_art: &[],
        critters: &[],
        options: BuildOptions {
            entrance_near: entrance,
            // Several components, the entrance in a smaller one: this test is
            // about where the entrance goes, not about closing the rest.
            close_unreached: false,
            ..BuildOptions::default()
        },
    };
    let plain = map_builder::build_map(&input(None)).unwrap();
    assert!(!inside_town(&frame, plain.report.entry_hex));
    let aimed = map_builder::build_map(&input(Some(GATE_ARRIVAL))).unwrap();
    assert!(inside_town(&frame, aimed.report.entry_hex));
    // The entrance's component is not the largest one.
    assert_ne!(aimed.report.components.main, Some(0));
    // A non-finite target is ignored.
    let ignored = map_builder::build_map(&input(Some([f32::NAN, 0.0]))).unwrap();
    assert_eq!(ignored.report.entry_hex, plain.report.entry_hex);
}

#[test]
fn a_map_level_option_is_refused_in_a_layer() {
    let t = town_scene();
    let frame = t.frame();
    let split = split_levels(&t.cells, &frame, &t.sampler, &LevelOptions::default());
    let mut input = town_layer(&t, None);
    input.maps[0].options.level = Some(split.exterior_level(0));
    let error = build_layer(&input).unwrap_err().to_string();
    assert!(error.contains("BuildOptions::level"), "{error}");
    let mut input = town_layer(&t, None);
    input.maps[0].elevations[1].interior = Some(map_builder::InteriorBuild::new(DECK_Z));
    let error = build_layer(&input).unwrap_err().to_string();
    assert!(
        error.contains("both an interior band and an exterior level"),
        "{error}"
    );
}
