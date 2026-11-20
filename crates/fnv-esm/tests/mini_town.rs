//! A synthetic "mini town" built with the fixture builder and read back the
//! way an extractor would: one pass over the file, grouping what it finds
//! by worldspace and cell. Everything here is invented.

use std::collections::BTreeMap;

use fnv_esm::fixture::{CellSpec, PluginBuilder, WorldSpec, solid_colors, up_normals};
use fnv_esm::*;

const TXST_DIRT: u32 = 0x0100_0100;
const TXST_GRAVEL: u32 = 0x0100_0101;
const LTEX_DIRT: u32 = 0x0100_0110;
const LTEX_GRAVEL: u32 = 0x0100_0111;
const STAT_SHACK: u32 = 0x0100_0200;
const DOOR_SHACK: u32 = 0x0100_0201;
const CONT_CRATE: u32 = 0x0100_0202;
const MISC_BOTTLE: u32 = 0x0100_0203;
const NPC_KEEPER: u32 = 0x0100_0300;
const SCPT_DOOR: u32 = 0x0100_0400;
const QUST_TOWN: u32 = 0x0100_0500;
const DIAL_HELLO: u32 = 0x0100_0600;
const INFO_HELLO: u32 = 0x0100_0601;
const WRLD_TOWN: u32 = 0x0100_1000;
const CELL_INSIDE: u32 = 0x0100_2000;
const DOOR_OUTSIDE_REF: u32 = 0x0100_1105;
const DOOR_INSIDE_REF: u32 = 0x0100_2001;

/// A slope rising one step (8 units) per row northward and per column
/// eastward, so that neighbouring cells meet along their shared edges.
fn slope(cell_x: i32, cell_y: i32) -> Vec<f32> {
    (0..LAND_VERTICES)
        .map(|i| {
            let row = (i / LAND_GRID) as i32 + cell_y * 32;
            let column = (i % LAND_GRID) as i32 + cell_x * 32;
            512.0 + 8.0 * (row + column) as f32
        })
        .collect()
}

fn exterior_cell(index: u32, x: i32, y: i32) -> CellSpec {
    let cell_id = WRLD_TOWN + 0x100 + index * 0x10;
    let origin = [x as f32 * 4096.0, y as f32 * 4096.0];
    let mut land = Land::new(cell_id + 1).with_heights(&slope(x, y));
    land.normals = Some(up_normals());
    land.colors = Some(solid_colors([128, 120, 110]));
    for quadrant in 0..4 {
        land.base_layers.push(BaseLayer {
            texture: FormId(LTEX_DIRT),
            quadrant,
            layer: -1,
        });
    }
    land.alpha_layers.push(AlphaLayer {
        texture: FormId(LTEX_GRAVEL),
        quadrant: 0,
        layer: 0,
        opacity: (0..17)
            .map(|column| AlphaPoint {
                position: column,
                opacity: f32::from(column) / 16.0,
            })
            .collect(),
    });
    let navmesh = Navmesh::new(
        cell_id + 2,
        cell_id,
        vec![
            [origin[0], origin[1], 512.0],
            [origin[0] + 4096.0, origin[1], 512.0],
            [origin[0], origin[1] + 4096.0, 512.0],
            [origin[0] + 4096.0, origin[1] + 4096.0, 512.0],
        ],
        vec![
            NavTriangle {
                vertices: [0, 1, 2],
                edges: [-1, 1, -1],
                flags: 0,
            },
            NavTriangle {
                vertices: [1, 3, 2],
                edges: [-1, -1, 0],
                flags: 0,
            },
        ],
    );
    let mut cell = Cell::exterior(cell_id, x, y);
    cell.common.editor_id = Some(format!("TownCell{index}"));
    CellSpec::new(cell)
        .with_land(land)
        .with_navmesh(navmesh)
        .with_temporary(
            PlacedRef::new(
                PlacedKind::Object,
                cell_id + 3,
                STAT_SHACK,
                [origin[0] + 2048.0, origin[1] + 2048.0, 1024.0],
                [0.0, 0.0, std::f32::consts::FRAC_PI_2 * index as f32],
            )
            .with_scale(1.0 + 0.25 * index as f32),
        )
}

fn mini_town() -> Vec<u8> {
    let mut builder = PluginBuilder::new();
    builder
        .master_file(true)
        .author("mini town")
        .description("A synthetic town of four cells and one room.")
        .master("Base.esm");

    builder
        .add(TextureSet::new(
            TXST_DIRT,
            "TownDirtSet",
            "landscape\\town\\dirt.dds",
        ))
        .add(TextureSet::new(
            TXST_GRAVEL,
            "TownGravelSet",
            "landscape\\town\\gravel.dds",
        ))
        .add(LandTexture::new(LTEX_DIRT, "TownDirt", TXST_DIRT))
        .add(LandTexture::new(LTEX_GRAVEL, "TownGravel", TXST_GRAVEL))
        .add(Static::new(STAT_SHACK, "TownShack", "town\\shack.nif"))
        .add(Door {
            common: Common::new(DOOR_SHACK)
                .with_editor_id("TownShackDoor")
                .with_name("Shack Door")
                .with_model("town\\shackdoor.nif")
                .with_script(SCPT_DOOR),
            ..Door::default()
        })
        .add(Container::new(
            CONT_CRATE,
            "TownCrate",
            "Crate",
            "town\\crate.nif",
            vec![ContainerItem::new(MISC_BOTTLE, 4)],
        ))
        .add(MiscItem::new(
            MISC_BOTTLE,
            "TownBottle",
            "Empty Bottle",
            "town\\bottle.nif",
            1,
            0.25,
        ))
        .add(Npc {
            inventory: vec![ContainerItem::new(MISC_BOTTLE, 1)],
            ..Npc::new(NPC_KEEPER, "TownKeeper", "Old Renn")
        })
        .add(Script::new(
            SCPT_DOOR,
            "TownShackDoorScript",
            "scn TownShackDoorScript\r\nbegin OnActivate\r\n\tActivate\r\nend\r\n",
        ))
        .add(Quest {
            stages: vec![QuestStage {
                index: 10,
                entries: vec![QuestLogEntry::new("Old Renn asked me to look around town.")],
            }],
            ..Quest::new(QUST_TOWN, "TownQuest", "A Look Around")
        });

    // Four exterior cells around the origin; the shack door is in the
    // north-east one and leads to the interior.
    let mut world = WorldSpec::new(World::new(WRLD_TOWN, "TownWorld"));
    for (index, (x, y)) in [(-1, -1), (0, -1), (-1, 0), (0, 0)].into_iter().enumerate() {
        let mut cell = exterior_cell(index as u32, x, y);
        if (x, y) == (0, 0) {
            cell = cell
                .with_persistent(
                    PlacedRef::new(
                        PlacedKind::Object,
                        DOOR_OUTSIDE_REF,
                        DOOR_SHACK,
                        [2048.0, 1900.0, 1024.0],
                        [0.0, 0.0, 0.0],
                    )
                    .with_teleport(
                        DOOR_INSIDE_REF,
                        [0.0, 64.0, 0.0],
                        [0.0, 0.0, std::f32::consts::PI],
                    )
                    .persistent(),
                )
                .with_temporary(
                    PlacedRef::new(
                        PlacedKind::Object,
                        0x0100_1106,
                        CONT_CRATE,
                        [2200.0, 2100.0, 1024.0],
                        [0.0, 0.0, 0.3],
                    )
                    .with_enable_parent(DOOR_OUTSIDE_REF, true)
                    .initially_disabled(),
                );
        }
        world = world.with_cell(cell);
    }
    builder.world(world);

    builder.interior(
        CellSpec::new(Cell::interior(CELL_INSIDE, "TownShackInterior"))
            .with_persistent(
                PlacedRef::new(
                    PlacedKind::Object,
                    DOOR_INSIDE_REF,
                    DOOR_SHACK,
                    [0.0, 0.0, 0.0],
                    [0.0, 0.0, std::f32::consts::PI],
                )
                .with_teleport(DOOR_OUTSIDE_REF, [2048.0, 1800.0, 1024.0], [0.0; 3])
                .persistent(),
            )
            .with_persistent(
                PlacedRef::new(
                    PlacedKind::Npc,
                    0x0100_2002,
                    NPC_KEEPER,
                    [64.0, 128.0, 0.0],
                    [0.0, 0.0, 1.0],
                )
                .persistent(),
            ),
    );

    builder.topic(
        Topic::new(DIAL_HELLO, "TownHello", "Hello there.", QUST_TOWN),
        vec![Info {
            conditions: vec![
                Condition::float(72, Comparison::Equal, 1.0).with_params(NPC_KEEPER, 0),
            ],
            ..Info::new(
                INFO_HELLO,
                QUST_TOWN,
                vec![Response::new(1, "Not much to see, but have a look around.")],
            )
        }],
    );
    builder.build()
}

/// What an extractor collects in one pass.
#[derive(Default)]
struct Extract {
    texture_sets: BTreeMap<FormId, TextureSet>,
    land_textures: BTreeMap<FormId, LandTexture>,
    cells: BTreeMap<FormId, Cell>,
    cell_world: BTreeMap<FormId, FormId>,
    land: BTreeMap<FormId, Land>,
    navmeshes: BTreeMap<FormId, Navmesh>,
    refs: Vec<(FormId, CellChildKind, PlacedRef)>,
    infos: Vec<(FormId, Info)>,
    scripts: Vec<Script>,
    others: BTreeMap<String, usize>,
}

fn extract(bytes: &[u8]) -> Extract {
    let plugin = Plugin::parse(bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut out = Extract::default();
    while let Some(record) = walker.next_record().expect("walk") {
        let context = walker.context();
        match record.decode().expect("decode") {
            Record::TextureSet(set) => {
                out.texture_sets.insert(set.common.form_id(), set);
            }
            Record::LandTexture(texture) => {
                out.land_textures.insert(texture.common.form_id(), texture);
            }
            Record::Cell(cell) => {
                if let Some(world) = context.worldspace() {
                    out.cell_world.insert(cell.common.form_id(), world);
                }
                out.cells.insert(cell.common.form_id(), cell);
            }
            Record::Land(land) => {
                out.land
                    .insert(context.cell().expect("land is in a cell"), land);
            }
            Record::Navmesh(navmesh) => {
                out.navmeshes
                    .insert(context.cell().expect("navmesh is in a cell"), navmesh);
            }
            Record::Placed(placed) => out.refs.push((
                context.cell().expect("reference is in a cell"),
                context.cell_child_kind().expect("in a children group"),
                placed,
            )),
            Record::Info(info) => out
                .infos
                .push((context.topic().expect("info is under a topic"), info)),
            Record::Script(script) => out.scripts.push(script),
            other => *out.others.entry(other.sig().to_string()).or_default() += 1,
        }
    }
    out
}

#[test]
fn the_mini_town_reads_back_whole() {
    let bytes = mini_town();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    assert_eq!(plugin.header().masters, ["Base.esm"]);
    let masters = plugin.master_list("MiniTown.esm");
    let town = extract(&bytes);

    // Four exterior cells in the worldspace, one interior outside it.
    assert_eq!(town.cells.len(), 5);
    assert_eq!(town.cell_world.len(), 4);
    assert!(town.cell_world.values().all(|w| *w == FormId(WRLD_TOWN)));
    let interior = &town.cells[&FormId(CELL_INSIDE)];
    assert!(interior.is_interior());
    assert_eq!(interior.grid, None);
    let mut grids: Vec<(i32, i32)> = town
        .cells
        .values()
        .filter_map(|c| c.grid.map(|g| (g.x, g.y)))
        .collect();
    grids.sort_unstable();
    assert_eq!(grids, [(-1, -1), (-1, 0), (0, -1), (0, 0)]);

    // Every exterior cell has land and a navmesh, keyed by the cell the
    // walker reported, which is the cell the navmesh names.
    assert_eq!(town.land.len(), 4);
    assert_eq!(town.navmeshes.len(), 4);
    for (cell, navmesh) in &town.navmeshes {
        assert_eq!(navmesh.cell, Some(*cell));
        assert!(navmesh.triangle_corners(1).is_some());
        assert!(navmesh.triangle_corners(2).is_none());
    }

    // Terrain is continuous across cell borders: the east edge of a cell is
    // the west edge of its eastern neighbour, the north edge of a cell the
    // south edge of its northern neighbour.
    let land_at = |x: i32, y: i32| {
        let cell = town
            .cells
            .values()
            .find(|c| c.grid.is_some_and(|g| (g.x, g.y) == (x, y)))
            .expect("cell at grid position");
        &town.land[&cell.common.form_id()]
    };
    for row in 0..LAND_GRID {
        assert_eq!(land_at(-1, 0).height(row, 32), land_at(0, 0).height(row, 0));
    }
    for column in 0..LAND_GRID {
        assert_eq!(
            land_at(0, -1).height(32, column),
            land_at(0, 0).height(0, column)
        );
    }
    // Row 0 is the south edge and column 0 the west edge: heights rise to
    // the north and to the east.
    let origin_land = land_at(0, 0);
    assert_eq!(origin_land.height(0, 0), Some(512.0));
    assert_eq!(origin_land.height(1, 0), Some(520.0));
    assert_eq!(origin_land.height(0, 2), Some(528.0));
    assert_eq!(origin_land.height(32, 32), Some(512.0 + 8.0 * 64.0));

    // Land layers resolve to texture paths through LTEX and TXST.
    let path_of = |layer_texture: FormId| {
        let land_texture = &town.land_textures[&layer_texture];
        let set = &town.texture_sets[&land_texture.texture_set.expect("TNAM")];
        set.diffuse().expect("TX00").to_owned()
    };
    assert_eq!(origin_land.base_layers.len(), 4);
    assert_eq!(
        path_of(origin_land.base_layers[0].texture),
        "landscape\\town\\dirt.dds"
    );
    let alpha = &origin_land.alpha_layers[0];
    assert_eq!(path_of(alpha.texture), "landscape\\town\\gravel.dds");
    assert_eq!(alpha.opacity.len(), 17);
    assert_eq!(alpha.opacity[8].opacity, 0.5);

    // References: transform, scale, teleport, enable parent, flags, and the
    // children group each was found in.
    assert_eq!(town.refs.len(), 4 + 2 + 2);
    let by_id = |id: u32| {
        town.refs
            .iter()
            .find(|(_, _, r)| r.common.form_id() == FormId(id))
            .expect("reference")
    };
    let (cell, kind, door) = by_id(DOOR_OUTSIDE_REF);
    assert_eq!(town.cells[cell].grid.map(|g| (g.x, g.y)), Some((0, 0)));
    assert_eq!(*kind, CellChildKind::Persistent);
    assert!(door.common.flags().is_persistent());
    assert_eq!(door.base, Some(FormId(DOOR_SHACK)));
    let teleport = door.teleport.expect("teleport");
    assert_eq!(teleport.door, FormId(DOOR_INSIDE_REF));
    // The door pair points at each other across the two cells.
    let (inside_cell, _, inside_door) = by_id(DOOR_INSIDE_REF);
    assert_eq!(*inside_cell, FormId(CELL_INSIDE));
    assert_eq!(
        inside_door.teleport.map(|t| t.door),
        Some(FormId(DOOR_OUTSIDE_REF))
    );

    let (_, kind, crate_ref) = by_id(0x0100_1106);
    assert_eq!(*kind, CellChildKind::Temporary);
    assert!(crate_ref.common.flags().is_initially_disabled());
    let parent = crate_ref.enable_parent.expect("enable parent");
    assert_eq!(parent.reference, FormId(DOOR_OUTSIDE_REF));
    assert!(parent.is_opposite());

    let shack = by_id(WRLD_TOWN + 0x100 + 0x30 + 3).2.clone();
    assert_eq!(shack.scale, 1.75);
    let placement = shack.placement.expect("placement");
    assert_eq!(placement.position, [2048.0, 2048.0, 1024.0]);
    assert_eq!(placement.rotation[2], std::f32::consts::FRAC_PI_2 * 3.0);
    assert_eq!(by_id(WRLD_TOWN + 0x100 + 3).2.scale, 1.0);

    let (_, _, keeper) = by_id(0x0100_2002);
    assert_eq!(keeper.kind, PlacedKind::Npc);
    assert_eq!(keeper.base, Some(FormId(NPC_KEEPER)));

    // Dialogue, script source and the rest.
    assert_eq!(town.infos.len(), 1);
    let (topic, info) = &town.infos[0];
    assert_eq!(*topic, FormId(DIAL_HELLO));
    assert_eq!(
        info.responses[0].text.as_deref(),
        Some("Not much to see, but have a look around.")
    );
    assert_eq!(info.conditions[0].function_index(), 72);
    assert_eq!(info.conditions[0].param1, NPC_KEEPER);
    assert_eq!(town.scripts.len(), 1);
    assert!(
        town.scripts[0]
            .body
            .source
            .as_deref()
            .is_some_and(|s| s.contains("begin OnActivate"))
    );
    let others: Vec<(&str, usize)> = town.others.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_eq!(
        others,
        [
            ("CONT", 1),
            ("DIAL", 1),
            ("DOOR", 1),
            ("MISC", 1),
            ("NPC_", 1),
            ("QUST", 1),
            ("STAT", 1),
            ("WRLD", 1)
        ]
    );

    // Every form id in this plugin belongs to the plugin itself: index 1,
    // one past its single master.
    let resolved = masters.resolve(door.common.form_id()).expect("own form");
    assert_eq!(resolved.to_string(), "minitown.esm:001105");
    assert_eq!(
        masters.resolve(FormId(0x0000_0007)).map(|r| r.plugin),
        Ok("base.esm")
    );
}

#[test]
fn an_extractor_can_take_only_the_groups_it_wants() {
    let bytes = mini_town();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut models = Vec::new();
    for item in plugin.top_level() {
        let TopItem::Group(group) = item.expect("top level") else {
            panic!("a valid plugin has only groups at the top level");
        };
        let wanted = matches!(
            group.header.kind(),
            GroupKind::Top(sig) if sig == sigs::STAT || sig == sigs::DOOR || sig == sigs::CONT
        );
        if !wanted {
            continue;
        }
        let mut walker = group.walk();
        while let Some(record) = walker.next_record().expect("walk") {
            let decoded = record.decode().expect("decode");
            let common = decoded.common().expect("typed");
            models.push(common.model.clone().expect("model"));
        }
    }
    assert_eq!(
        models,
        ["town\\shack.nif", "town\\shackdoor.nif", "town\\crate.nif"]
    );
}
