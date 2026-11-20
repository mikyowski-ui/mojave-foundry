//! Synthetic inputs: a window over invented cells, a flat or sloped
//! terrain, navmesh triangles, a small tile library, art and placements.

#![allow(dead_code)]

use std::collections::BTreeMap;

use foundry_render::{Calibration, Projection};
use map_builder::walk::Frame;
use map_builder::{
    ArtCategory, ArtImage, BuildInput, BuildOptions, CritterArt, CritterPlacement, GroundTiles,
    Placement, Role,
};
use mir::{
    CellDoc, CellLocation, FormKey, GroundSampler, LAND_GRID, LAND_SPACING, LandDoc, MapWindow,
    NavTriangleDoc, NavmeshDoc, Reference,
};

pub const PLUGIN: &str = "synthetic.esm";
pub const CENTRE: [f32; 2] = [2048.0, 2048.0];

pub fn projection() -> Projection {
    Projection::new(&Calibration::default()).unwrap()
}

pub fn window(projection: &Projection) -> MapWindow {
    MapWindow::from_center(CENTRE, projection.tile_units())
}

pub fn frame(window: &MapWindow, projection: &Projection) -> Frame {
    Frame::new(window, projection).unwrap()
}

/// The source point of a hex centre.
pub fn hex_point(frame: &Frame, hex: i32) -> [f32; 2] {
    let p = frame.source_of_tile(frame.hex_centre_tile(hex).unwrap());
    [p[0] as f32, p[1] as f32]
}

fn land(cx: i32, cy: i32, height: &impl Fn(f32, f32) -> f32) -> LandDoc {
    let mut heights = Vec::with_capacity(LAND_GRID * LAND_GRID);
    for row in 0..LAND_GRID {
        for column in 0..LAND_GRID {
            let x = cx as f32 * 4096.0 + column as f32 * LAND_SPACING;
            let y = cy as f32 * 4096.0 + row as f32 * LAND_SPACING;
            heights.push(height(x, y));
        }
    }
    LandDoc {
        form: FormKey::new(PLUGIN, 0x100 + ((cx + 8) * 16 + cy + 8) as u32),
        heights,
        normals: None,
        colors: None,
        layers: Vec::new(),
    }
}

/// Terrain under every cell the window touches.
pub fn sampler(window: &MapWindow, height: impl Fn(f32, f32) -> f32) -> GroundSampler {
    let mut sampler = GroundSampler::default();
    let (xs, ys) = window.cells(1);
    for y in ys {
        for x in xs.clone() {
            sampler.insert(x, y, &land(x, y, &height));
        }
    }
    sampler
}

pub fn flat(_: f32, _: f32) -> f32 {
    100.0
}

pub fn exterior_cell(x: i32, y: i32) -> CellDoc {
    CellDoc {
        schema_version: mir::SCHEMA_VERSION,
        form: FormKey::new(PLUGIN, 0x1000 + ((x + 8) * 16 + y + 8) as u32),
        editor_id: Some(format!("SynthCell{x}{y}")),
        name: None,
        location: CellLocation::Exterior {
            worldspace: "synthworld".into(),
            worldspace_form: FormKey::new(PLUGIN, 0x800),
            x,
            y,
        },
        flags: 0,
        water_height: None,
        land: None,
        references: Vec::new(),
        navmeshes: Vec::new(),
    }
}

/// A navmesh of the given source-space triangles at height `z`.
pub fn navmesh(form: u32, triangles: &[[[f32; 2]; 3]], z: f32) -> NavmeshDoc {
    let mut vertices = Vec::new();
    let mut docs = Vec::new();
    for triangle in triangles {
        let base = vertices.len() as u32;
        for corner in triangle {
            vertices.push([corner[0], corner[1], z]);
        }
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

/// Two triangles covering the source rectangle.
pub fn rectangle(x0: f32, y0: f32, x1: f32, y1: f32) -> [[[f32; 2]; 3]; 2] {
    [
        [[x0, y0], [x1, y0], [x1, y1]],
        [[x0, y0], [x1, y1], [x0, y1]],
    ]
}

/// A two-tile library: each tile a different palette index.
pub fn library() -> Vec<Vec<u8>> {
    vec![vec![10; 80 * 36], vec![20; 80 * 36]]
}

pub fn squares() -> Vec<u16> {
    (0..10_000).map(|q| (q % 2) as u16).collect()
}

pub fn image(category: ArtCategory, width: u32, height: u32, value: u8, source: &str) -> ArtImage {
    ArtImage {
        category,
        width,
        height,
        pixels: vec![value; (width * height) as usize],
        shift: [0, -(height as i16) / 2],
        source: source.into(),
    }
}

pub fn placement(art: usize, hex: i32, role: Role, blocking: bool, reference: &str) -> Placement {
    Placement {
        art,
        hex,
        displacement: [0, 0],
        role,
        reference: reference.into(),
        blocking,
    }
}

/// Everything a `BuildInput` borrows.
pub struct Scene {
    pub projection: Projection,
    pub window: MapWindow,
    pub sampler: GroundSampler,
    pub tiles: Vec<Vec<u8>>,
    pub squares: Vec<u16>,
    pub art: Vec<ArtImage>,
    pub placements: Vec<Placement>,
    pub blocked: Vec<i32>,
    pub cells: Vec<CellDoc>,
    pub base: BTreeMap<String, Vec<u8>>,
    pub critter_art: Vec<CritterArt>,
    pub critters: Vec<CritterPlacement>,
    pub options: BuildOptions,
    pub name: String,
    pub number: Option<u32>,
}

impl Scene {
    pub fn frame(&self) -> Frame {
        frame(&self.window, &self.projection)
    }

    pub fn input(&self) -> BuildInput<'_> {
        BuildInput {
            map_name: &self.name,
            map_number: self.number,
            layer_id: "synthetic-map",
            layer_name: "Synthetic map",
            window: self.window,
            sampler: &self.sampler,
            projection: &self.projection,
            ground: GroundTiles {
                tiles: &self.tiles,
                squares: &self.squares,
            },
            art: &self.art,
            placements: &self.placements,
            blocked_hexes: &self.blocked,
            cells: &self.cells,
            base_files: &self.base,
            critter_art: &self.critter_art,
            critters: &self.critters,
            options: self.options.clone(),
        }
    }
}

/// The hex at window tile `(s, t)`.
pub fn hex_at(frame: &Frame, s: f64, t: f64) -> i32 {
    frame.hex_of_tile([s, t]).unwrap()
}

/// A scene over flat ground with one navmesh square in the middle of the
/// window (tile space 40 to 60 on both axes), a wall, scenery, a blocking
/// container, an item and a door without a teleport.
pub fn town() -> Scene {
    let projection = projection();
    let window = window(&projection);
    let frame = frame(&window, &projection);
    let sampler = sampler(&window, flat);
    let corner = |s: f64, t: f64| {
        let p = frame.source_of_tile([s, t]);
        [p[0] as f32, p[1] as f32]
    };
    let [x0, y1] = corner(40.0, 40.0);
    let [x1, y0] = corner(60.0, 60.0);
    let mut cells = Vec::new();
    let (xs, ys) = window.cells(0);
    for y in ys {
        for x in xs.clone() {
            let mut cell = exterior_cell(x, y);
            // Every other cell has a navmesh too (one sliver in its
            // corner), so slope alone decides nothing.
            let (ox, oy) = (x as f32 * 4096.0, y as f32 * 4096.0);
            cell.navmeshes.push(navmesh(
                0x3000 + cells.len() as u32,
                &[[
                    [ox + 1.0, oy + 1.0],
                    [ox + 3.0, oy + 1.0],
                    [ox + 1.0, oy + 3.0],
                ]],
                100.0,
            ));
            cells.push(cell);
        }
    }
    // The navmesh in the cell holding the window centre; the rectangle is
    // inside that one cell.
    let centre_cell = mir::cell_of(&[CENTRE[0], CENTRE[1], 0.0]);
    let cell = cells
        .iter_mut()
        .find(
            |c| matches!(c.location, CellLocation::Exterior { x, y, .. } if (x, y) == centre_cell),
        )
        .unwrap();
    // Keep the navmesh inside the centre cell, which spans 0 to 4096.
    let clamp = |v: f32| v.clamp(1.0, 4095.0);
    let tris = rectangle(clamp(x0), clamp(y0), clamp(x1), clamp(y1));
    cell.navmeshes.push(navmesh(0x2000, &tris, 100.0));
    let art = vec![
        image(ArtCategory::Walls, 32, 60, 30, "wall strip"),
        image(ArtCategory::Scenery, 20, 30, 40, "rock"),
        image(ArtCategory::Items, 16, 16, 50, "crate"),
        image(ArtCategory::Items, 6, 6, 60, "can"),
        image(ArtCategory::Scenery, 24, 50, 70, "door"),
        // A repeat of the wall: it shares one FRM.
        image(ArtCategory::Walls, 32, 60, 30, "wall strip"),
    ];
    let placements = vec![
        placement(
            0,
            hex_at(&frame, 50.0, 45.0),
            Role::WallStrip,
            true,
            "synthetic.esm:000a01",
        ),
        placement(
            5,
            hex_at(&frame, 50.5, 45.0),
            Role::WallStrip,
            true,
            "synthetic.esm:000a02",
        ),
        placement(
            1,
            hex_at(&frame, 45.0, 50.0),
            Role::Scenery,
            true,
            "synthetic.esm:000a03",
        ),
        placement(
            2,
            hex_at(&frame, 55.0, 55.0),
            Role::Container,
            true,
            "synthetic.esm:000a04",
        ),
        placement(
            3,
            hex_at(&frame, 52.0, 52.0),
            Role::Item,
            false,
            "synthetic.esm:000a05",
        ),
        placement(
            4,
            hex_at(&frame, 47.0, 55.0),
            Role::Door,
            true,
            "synthetic.esm:000a06",
        ),
    ];
    let blocked = vec![hex_at(&frame, 44.0, 44.0)];
    Scene {
        projection,
        window,
        sampler,
        tiles: library(),
        squares: squares(),
        art,
        placements,
        blocked,
        cells,
        base: BTreeMap::new(),
        critter_art: Vec::new(),
        critters: Vec::new(),
        options: BuildOptions::default(),
        name: "synth".into(),
        number: None,
    }
}

/// A complete synthetic base: every file of `base_file_paths`, short.
pub fn base_files(scenery_entries: usize) -> BTreeMap<String, Vec<u8>> {
    let list = |stem: &str, count: usize| -> Vec<u8> {
        (0..count)
            .map(|i| format!("{stem}{i:04}.frm\n"))
            .collect::<String>()
            .into_bytes()
    };
    let protos = |count: usize| -> Vec<u8> {
        (1..=count)
            .map(|i| format!("{i:08}.pro\n"))
            .collect::<String>()
            .into_bytes()
    };
    let mut base = BTreeMap::new();
    base.insert("art/items/items.lst".into(), list("bit", 3));
    base.insert(
        "art/scenery/scenery.lst".into(),
        list("bsc", scenery_entries),
    );
    base.insert("art/walls/walls.lst".into(), list("bwa", 4));
    base.insert("art/tiles/tiles.lst".into(), list("bti", 5));
    base.insert("art/misc/misc.lst".into(), list("bmi", 40));
    base.insert("proto/items/items.lst".into(), protos(2));
    base.insert("proto/scenery/scenery.lst".into(), protos(3));
    base.insert("proto/walls/walls.lst".into(), protos(1));
    base.insert("proto/misc/misc.lst".into(), protos(20));
    for stem in ["pro_item", "pro_scen", "pro_wall"] {
        base.insert(
            format!("text/english/game/{stem}.msg"),
            b"{100}{}{Base name}\n{101}{}{Base description}\n".to_vec(),
        );
    }
    base.insert(
        "data/maps.txt".into(),
        b"; synthetic map table\n[Map 000]\nmap_name=basea\n\n[Map 150]\nmap_name=baseb\nlookup_name=Base B\n"
            .to_vec(),
    );
    base
}

/// A teleport door reference into `interior` (form `interior_form`),
/// whose destination door is `destination`.
pub fn teleport_door(
    form: u32,
    position: [f32; 3],
    destination: u32,
    interior_form: u32,
    interior: Option<&str>,
    exterior: Option<(i32, i32)>,
) -> Reference {
    let location = match exterior {
        Some((x, y)) => CellLocation::Exterior {
            worldspace: "synthworld".into(),
            worldspace_form: FormKey::new(PLUGIN, 0x800),
            x,
            y,
        },
        None => CellLocation::Interior,
    };
    Reference {
        form: FormKey::new(PLUGIN, form),
        editor_id: None,
        kind: mir::RefKind::Object,
        group: mir::RefGroup::Persistent,
        base: FormKey::new(PLUGIN, 0x50),
        base_kind: Some("DOOR".into()),
        base_editor_id: None,
        model: None,
        mesh: None,
        transform: mir::Transform {
            position,
            ..mir::Transform::default()
        },
        flags: mir::RefFlags::default(),
        teleport: Some(mir::TeleportDoc {
            door: FormKey::new(PLUGIN, destination),
            cell: Some(mir::CellRef {
                form: FormKey::new(PLUGIN, interior_form),
                editor_id: interior.map(str::to_owned),
                location,
            }),
            position: [0.0, 0.0, 0.0],
            rotation: [0.0; 3],
        }),
        enable_parent: None,
        lock: None,
        count: None,
        linked_ref: None,
        owner: None,
    }
}

/// The critter files of a synthetic base: two critter art rows, two
/// critter prototypes with their messages, and a small AI table whose
/// most peaceful packet (by `map_builder::critters::choose_ai_packet`) is 3.
pub fn critter_base_files() -> BTreeMap<String, Vec<u8>> {
    let mut base = BTreeMap::new();
    base.insert(
        "art/critters/critters.lst".into(),
        b"hmbase,0\nhfbase,1\n".to_vec(),
    );
    base.insert(
        "proto/critters/critters.lst".into(),
        b"00000001.pro\n00000002.pro\n".to_vec(),
    );
    base.insert(
        "text/english/game/pro_crit.msg".into(),
        b"{100}{}{Base critter}\n{101}{}{A base description.}\n".to_vec(),
    );
    let packet = |name: &str, number: i32, min_hp: i32| {
        let mut text = format!("[{name}]\npacket_num={number}\nmin_hp={min_hp}\n");
        for key in vault_formats::ai::KEYS
            .iter()
            .chain(vault_formats::ai::END_KEYS.iter())
        {
            if !["packet_num", "min_hp"].contains(key) {
                text.push_str(&format!("{key}=0\n"));
            }
        }
        text
    };
    base.insert(
        "data/ai.txt".into(),
        [
            packet("Brave", 1, 0),
            packet("Timid", 3, 40),
            packet("Middling", 2, 10),
        ]
        .concat()
        .into_bytes(),
    );
    base
}

/// One synthetic animation: `count` frames per direction, every direction
/// a different pattern, travelling `per_frame` hex steps per frame.
pub fn critter_frm_bytes(count: usize, per_frame: f32, seed: u8) -> Vec<u8> {
    use emit::FormatWriters;
    let directions = std::array::from_fn(|d| {
        let step = emit::HEX_STEPS[d];
        (0..count)
            .map(|f| {
                let (width, height) = (9 + d as u32, 30 + (f as u32 % 3));
                let mut pixels = vec![0u8; (width * height) as usize];
                for y in 1..height - 1 {
                    for x in 1..width - 1 {
                        pixels[(y * width + x) as usize] = 1
                            + ((x * 7 + y * 3 + u32::from(seed) + d as u32 * 11 + f as u32) % 200)
                                as u8;
                    }
                }
                emit::RenderedFrame {
                    origin: [-(width as i32) / 2, -(height as i32) + 1],
                    width,
                    height,
                    pixels,
                    travel: [step[0] as f32 * per_frame, step[1] as f32 * per_frame],
                }
            })
            .collect()
    });
    let spec = emit::critter_frm(&emit::CritterFrames { fps: 8, directions }, 0).unwrap();
    emit::LocalWriters.frm(&spec).unwrap()
}

/// A sprite set with a stand, a walk and a hit placeholder.
pub fn sprite_set(label: &str, seed: u8) -> CritterArt {
    let mut frms = BTreeMap::new();
    frms.insert("aa".to_owned(), critter_frm_bytes(4, 0.0, seed));
    frms.insert("ab".to_owned(), critter_frm_bytes(8, 0.125, seed));
    frms.insert("ao".to_owned(), critter_frm_bytes(1, 0.0, seed));
    CritterArt {
        label: label.into(),
        frms,
    }
}

/// A character at source position `position` facing `yaw`.
pub fn critter(
    art: usize,
    reference: &str,
    name: &str,
    position: [f32; 2],
    yaw: f32,
) -> CritterPlacement {
    CritterPlacement {
        art,
        reference: reference.into(),
        name: name.into(),
        description: format!("{name}, a resident."),
        position,
        yaw,
        hit_points: 25,
        female: false,
    }
}
