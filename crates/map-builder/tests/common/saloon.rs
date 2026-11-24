//! The saloon interior of `tests/interior.rs`, shared with the golden
//! digests (`tests/golden.rs`).

use map_builder::{ArtCategory, BLANK_SQUARE, BuildOptions, InteriorBuild, Role};
use mir::{CellDoc, CellLocation, FormKey, GroundSampler, MapWindow, Reference};

use super::scenes::full_base;
use super::*;

pub const FLOOR_Z: f32 = 1000.0;
pub const BALCONY_Z: f32 = 1300.0;
pub const SALOON: u32 = 0x7000;
pub const DOOR: u32 = 0x7101;
pub const RESIDENTS: [(u32, [f32; 3]); 3] = [
    (0x7201, [-200.0, 0.0, 1000.0]),
    (0x7202, [200.0, 100.0, 1010.0]),
    // On the balcony.
    (0x7203, [700.0, 0.0, 1300.0]),
];
/// Where the door stands: the floor's south wall.
pub const DOOR_AT: [f32; 3] = [0.0, -290.0, 1000.0];

pub fn form(id: u32) -> String {
    FormKey::new(PLUGIN, id).to_string()
}

pub fn resident(id: u32, position: [f32; 3]) -> Reference {
    let mut reference = teleport_door(id, position, 0, 0, None, None);
    reference.kind = mir::RefKind::Npc;
    reference.base_kind = Some("NPC_".into());
    reference.teleport = None;
    reference
}

/// The saloon: floor `[-400, 400] × [-300, 300]` at z 1000, balcony
/// `[500, 900] × [-300, 300]` at z 1300, a door out to an exterior cell,
/// and the residents.
pub fn saloon_cell() -> CellDoc {
    let mut references = vec![teleport_door(
        DOOR,
        DOOR_AT,
        0x7901,
        0x7900,
        None,
        Some((0, 0)),
    )];
    for (id, position) in RESIDENTS {
        references.push(resident(id, position));
    }
    references.sort_by(|a, b| a.form.cmp(&b.form));
    CellDoc {
        schema_version: mir::SCHEMA_VERSION,
        form: FormKey::new(PLUGIN, SALOON),
        editor_id: Some("SynthSaloon".into()),
        name: Some("Synthetic Saloon".into()),
        location: CellLocation::Interior,
        flags: 1,
        water_height: None,
        land: None,
        references,
        navmeshes: vec![
            navmesh(0x7301, &rectangle(-400.0, -300.0, 400.0, 300.0), FLOOR_Z),
            navmesh(0x7302, &rectangle(500.0, -300.0, 900.0, 300.0), BALCONY_Z),
        ],
    }
}

pub fn interior(clamp_levels: bool, entrance_near: Option<[f32; 2]>) -> BuildOptions {
    BuildOptions {
        interior: Some(InteriorBuild {
            entrance_near,
            clamp_levels,
            ..InteriorBuild::new(FLOOR_Z)
        }),
        ..BuildOptions::default()
    }
}

/// The saloon scene over an empty ground sampler; half the squares blank,
/// and the library's second tile all transparent.
pub fn saloon(options: BuildOptions) -> Scene {
    let projection = projection();
    let window = MapWindow::from_center([0.0, 0.0], projection.tile_units());
    let frame = frame(&window, &projection);
    let door_hex = frame
        .hex_of_source(f64::from(DOOR_AT[0]), f64::from(DOOR_AT[1]))
        .unwrap();
    let critters = RESIDENTS
        .iter()
        .enumerate()
        .map(|(i, (id, p))| critter(0, &form(*id), &format!("Resident {i}"), [p[0], p[1]], 0.0))
        .collect();
    Scene {
        projection,
        window,
        sampler: GroundSampler::default(),
        tiles: vec![vec![10; 80 * 36], vec![0; 80 * 36]],
        squares: (0..10_000)
            .map(|q| match q % 3 {
                0 => 0,
                1 => 1,
                _ => BLANK_SQUARE,
            })
            .collect(),
        art: vec![
            image(ArtCategory::Scenery, 24, 50, 70, "saloon door"),
            image(ArtCategory::Scenery, 20, 30, 40, "bar stool"),
        ],
        placements: vec![
            placement(0, door_hex, Role::Door, true, &form(DOOR)),
            placement(
                1,
                frame.hex_of_source(-100.0, 150.0).unwrap(),
                Role::Scenery,
                true,
                "synthetic.esm:007401",
            ),
        ],
        blocked: Vec::new(),
        cells: vec![saloon_cell()],
        base: full_base(),
        critter_art: vec![sprite_set("resident", 3)],
        critters,
        options,
        name: "saloon".into(),
        number: None,
    }
}
