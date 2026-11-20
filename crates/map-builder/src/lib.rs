//! Walkability, interactive objects, exits and MAP v20 assembly.
//!
//! [`build_map`] turns one map window's ground tiles, sliced art,
//! placements and cells into a Fallout 2 mod layer (`docs/contracts.md`
//! section 9.3):
//!
//! 1. **Walkability** ([`walk`]): a hex is walkable when its centre's
//!    ground point is on (or within a tolerance of) a navmesh triangle of
//!    an exterior cell, the terrain slope there is at most the limit, it is
//!    not in the caller's blocked list and no blocking placement stands on
//!    it. A hex in a cell without a navmesh falls back to the slope test.
//! 2. **Blockers**: the engine blocks movement through objects with
//!    scenery or wall art only, so every non-walkable hex that touches a
//!    walkable hex (or a door) and holds no such object gets an invisible
//!    blocker: a sealing border, equivalent for movement to blocking every
//!    non-walkable hex.
//! 3. **Art and prototypes**: FRMs, appended to full copies of the base
//!    art lists, one prototype per distinct (art, role, blocking).
//! 4. **Exits** ([`exits`]): a door whose reference teleports into an
//!    interior gets exit grids on the walkable hexes next to it, leading to
//!    a stub map of its own, whose exit ring leads back to that door.
//! 5. **Critters** ([`critters`]): each character on the free walkable hex
//!    nearest its source position, facing the hex direction nearest its
//!    yaw; its sprite set appended to `art/critters/critters.lst`, one
//!    prototype per distinct character in `proto/critters` with its name in
//!    `pro_crit.msg`.
//! 6. **Maps and layer**: the exterior and stub maps (version 20), the
//!    lists, prototypes, messages, `data/maps.txt` (when the base one is
//!    supplied) and `vault-mod.toml`, checked by [`verify_layer`] before
//!    they are returned.

mod assemble;
pub mod critters;
mod error;
pub mod exits;
mod names;
mod verify;
pub mod walk;

use std::collections::BTreeMap;

pub use error::BuildError;
pub use verify::{VerifyReport, read_layer, verify_layer};

/// The language folder of the prototype message files this crate reads
/// and writes.
pub const LANGUAGE: &str = "english";

/// Every base-game VFS path [`base_file_paths`] names.
const BASE_FILE_PATHS: [&str; 17] = [
    "art/items/items.lst",
    "art/scenery/scenery.lst",
    "art/walls/walls.lst",
    "art/tiles/tiles.lst",
    "art/misc/misc.lst",
    "proto/items/items.lst",
    "proto/scenery/scenery.lst",
    "proto/walls/walls.lst",
    "proto/misc/misc.lst",
    "text/english/game/pro_item.msg",
    "text/english/game/pro_scen.msg",
    "text/english/game/pro_wall.msg",
    "data/maps.txt",
    // The critter files, from here on.
    "art/critters/critters.lst",
    "proto/critters/critters.lst",
    "text/english/game/pro_crit.msg",
    "data/ai.txt",
];

/// Where the critter files start in [`BASE_FILE_PATHS`].
pub(crate) const CRITTER_BASE_FROM: usize = 13;

/// The base-game VFS paths the caller should read from the Fallout 2
/// install into [`BuildInput::base_files`]: art lists, prototype lists,
/// prototype message files and the map table, then the critter files
/// (`art/critters/critters.lst`, `proto/critters/critters.lst`,
/// `pro_crit.msg` and `data/ai.txt`). Each list that is supplied is
/// extended (a layer file replaces the base file whole); each one that is
/// missing starts empty.
///
/// When any base file is supplied, the first thirteen must all be. The
/// critter files are needed only by a build that places critters: then
/// the three critter lists must be supplied too, and `data/ai.txt` as
/// well unless [`BuildOptions::critter_ai_packet`] names the packet. A
/// build without critters never reads them, so its output does not depend
/// on them. `data/ai.txt` is read, never shipped.
pub fn base_file_paths() -> &'static [&'static str] {
    &BASE_FILE_PATHS
}

/// The art list an [`ArtImage`] goes to (contracts 9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArtCategory {
    Walls,
    Scenery,
    Items,
}

/// What a placement becomes (contracts 9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// A wall prototype (wall art).
    WallStrip,
    /// Generic scenery (scenery art).
    Scenery,
    /// A scenery door (scenery art); a teleport on its reference makes an
    /// exit.
    Door,
    /// An item container (item art).
    Container,
    /// A miscellaneous item (item art).
    Item,
    /// A walking surface above the terrain, such as a porch floor (scenery
    /// art): flat, never blocking, never cut by the see-through circle.
    FloorPiece,
}

impl Role {
    /// The word used in generated prototype names.
    pub fn label(self) -> &'static str {
        match self {
            Self::WallStrip => "Wall",
            Self::Scenery => "Scenery",
            Self::Door => "Door",
            Self::Container => "Container",
            Self::Item => "Item",
            Self::FloorPiece => "Floor",
        }
    }
}

/// One sprite (contracts 9.2): indexed pixels, row-major, index 0
/// transparent, placed with its bottom-centre at the hex centre plus
/// `shift`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtImage {
    pub category: ArtCategory,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub shift: [i16; 2],
    /// A short label of what it was cut from; it names the prototype and
    /// the budget diagnostics.
    pub source: String,
}

/// One object on the map (contracts 9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Index into [`BuildInput::art`].
    pub art: usize,
    pub hex: i32,
    /// The object's own screen displacement, added to the hex centre and
    /// the FRM shift.
    pub displacement: [i32; 2],
    pub role: Role,
    /// The placed reference's form key (`"plugin.esm:0abcde"`), used to
    /// find a door's teleport.
    pub reference: String,
    /// Whether it blocks movement.
    pub blocking: bool,
}

/// The ground of contracts 9.1: the tile library and each square's tile.
#[derive(Debug, Clone, Copy)]
pub struct GroundTiles<'a> {
    /// 80 × 36 indexed images, index 0 outside the square's parallelogram.
    pub tiles: &'a [Vec<u8>],
    /// 10,000 library indices, numbered as `HexGrid` numbers squares.
    pub squares: &'a [u16],
}

/// Tuning, all calibration items for the first real-data run.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildOptions {
    /// The steepest walkable terrain, rise over run.
    pub slope_limit: f32,
    /// How far (source units) outside a navmesh triangle a hex centre may
    /// lie and still count as covered.
    pub navmesh_tolerance: f32,
    /// A navmesh triangle whose every corner stands more than this many
    /// source units above the terrain (a roof, an upper floor) is ignored.
    pub navmesh_max_height: f32,
    /// Ship a full `data/maps.txt` (the base table plus one record per new
    /// map) when the base table is supplied. Under Fallout 2 the engine
    /// resolves a transition's map number through that table only.
    pub write_maps_txt: bool,
    /// The capacity (size units) of converted containers.
    pub container_capacity: i32,
    /// The AI packet every placed critter gets. `None` chooses one from
    /// the base `data/ai.txt` by [`critters::choose_ai_packet`]'s rule, or
    /// [`critters::FALLBACK_AI_PACKET`] (with a note) when no base table is
    /// supplied. A packet named here that a supplied base table lacks is
    /// an input error, since the engine would quietly use another.
    pub critter_ai_packet: Option<i32>,
    /// The team every placed critter shares; default
    /// [`critters::DEFAULT_CRITTER_TEAM`]. Must not be 0, the player's.
    pub critter_team: i32,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            slope_limit: 0.7,
            navmesh_tolerance: 24.0,
            navmesh_max_height: 160.0,
            write_maps_txt: true,
            container_capacity: 100,
            critter_ai_packet: None,
            critter_team: critters::DEFAULT_CRITTER_TEAM,
        }
    }
}

/// One sprite set: the animation files of one appearance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CritterArt {
    /// A short label of what it shows; used in diagnostics only.
    pub label: String,
    /// FRM bytes by two-letter animation suffix (`"aa"`, `"ab"`, `"ao"`,
    /// ...). `aa` (stand) and `ab` (walk) are required; every file must be
    /// a six-direction FRM.
    pub frms: BTreeMap<String, Vec<u8>>,
}

/// One character to place.
#[derive(Debug, Clone, PartialEq)]
pub struct CritterPlacement {
    /// Index into [`BuildInput::critter_art`].
    pub art: usize,
    /// The placed reference's form key.
    pub reference: String,
    pub name: String,
    pub description: String,
    /// Source position (x east, y north).
    pub position: [f32; 2],
    /// The reference's rotation about Z, radians, by the convention of
    /// contracts section 1 (see [`walk::Frame::orientation_of_yaw`]).
    pub yaw: f32,
    pub hit_points: i32,
    pub female: bool,
}

/// Everything one map is built from. The lead fills it from
/// `map-terrain` (`ground`), `map-slicer` (`art`, `placements`,
/// `blocked_hexes`) and `mir` (`cells`, `sampler`).
pub struct BuildInput<'a> {
    /// The map's file stem: 1 to 8 ASCII letters, digits, `_` or `-`
    /// (written lowercase as `maps/<stem>.map`, `<STEM>.MAP` in the
    /// header).
    pub map_name: &'a str,
    /// The exterior map's number; `None` takes the first allowed one (66,
    /// or the base `maps.txt` record count when that is larger). Stub maps
    /// take the numbers after it.
    pub map_number: Option<u32>,
    /// The manifest's `id` (`[a-z0-9_-]`, 1 to 32 bytes) and `name`.
    pub layer_id: &'a str,
    pub layer_name: &'a str,
    pub window: mir::MapWindow,
    pub sampler: &'a mir::GroundSampler,
    pub projection: &'a foundry_render::Projection,
    pub ground: GroundTiles<'a>,
    pub art: &'a [ArtImage],
    pub placements: &'a [Placement],
    /// Hexes the slicer found covered by baked relief (rocks, cliffs).
    pub blocked_hexes: &'a [i32],
    /// The exterior cells the window touches (navmeshes) and the interior
    /// cells their doors lead to (return positions). Order does not matter.
    pub cells: &'a [mir::CellDoc],
    /// Base-game files by VFS path (lowercase, `/`): those of
    /// [`base_file_paths`] that the install has. Empty for a synthetic
    /// build.
    pub base_files: &'a BTreeMap<String, Vec<u8>>,
    /// The sprite sets [`BuildInput::critters`] index; empty for none.
    pub critter_art: &'a [CritterArt],
    /// The characters to place; empty for none.
    pub critters: &'a [CritterPlacement],
    pub options: BuildOptions,
}

/// What [`build_map`] returns.
#[derive(Debug)]
pub struct Built {
    pub layer: emit::ModLayer,
    /// 40,000 entries, by hex.
    pub walkable: Vec<bool>,
    pub report: BuildReport,
}

/// One door that became an exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitReport {
    /// The door's reference form key.
    pub door: String,
    pub door_hex: i32,
    /// The interior cell's form key and editor id.
    pub interior: String,
    pub interior_editor_id: Option<String>,
    /// The stub map's file (`<stem>.map`) and number.
    pub stub_file: String,
    pub stub_number: u32,
    /// The exterior hexes holding exit grids into the stub.
    pub grid_hexes: Vec<i32>,
    /// Where the stub's exit puts the player back on the exterior map.
    pub return_hex: i32,
    pub return_orientation: u8,
    /// Whether the return hex came from the interior door's own teleport
    /// (otherwise: the nearest walkable hex two or more steps out).
    pub return_from_teleport: bool,
}

/// A door teleport this pass did not turn into an exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedTeleport {
    pub door: String,
    pub reason: String,
}

/// One critter on the exterior map.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedCritter {
    /// The reference's form key.
    pub reference: String,
    /// Index into [`BuildInput::critters`].
    pub index: usize,
    pub hex: i32,
    /// 0 to 5: NE, E, SE, SW, W, NW.
    pub orientation: u8,
    /// Hex steps from the hex under the source position to `hex`.
    pub steps: i32,
    /// Source units from the source position to the centre of `hex` on
    /// the ground.
    pub moved: f64,
    /// The critter prototype's PID and the stem of its sprite set.
    pub pid: u32,
    pub stem: String,
}

/// A critter that is not on the map, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedCritter {
    pub reference: String,
    /// Index into [`BuildInput::critters`].
    pub index: usize,
    pub reason: String,
}

/// One sprite set added to `art/critters`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CritterSetReport {
    /// Index into [`BuildInput::critter_art`], and its label.
    pub art: usize,
    pub label: String,
    /// `mj` and four base-36 digits.
    pub stem: String,
    /// Its row in `art/critters/critters.lst`.
    pub row: u16,
    /// Its files, `art/critters/<stem><suffix>.frm`, sorted.
    pub files: Vec<String>,
}

/// One critter prototype added to `proto/critters`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CritterPrototypeReport {
    pub pid: u32,
    /// `proto/critters/<file>`.
    pub file: String,
    /// The `art/critters` stem it shows.
    pub stem: String,
    /// Its name and description as written to `pro_crit.msg`.
    pub name: String,
    pub description: String,
    pub hit_points: i32,
    pub female: bool,
}

/// Counts and decisions of one build.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuildReport {
    /// `maps/<stem>.map` and its number.
    pub map_file: String,
    pub map_number: u32,
    pub entry_hex: i32,
    pub walk: walk::WalkStats,
    /// Invisible blockers placed on the sealing border.
    pub blockers: usize,
    /// Objects on the exterior map: placements, blockers and exit grids.
    pub objects: usize,
    /// New art list entries, by art directory.
    pub art_entries: BTreeMap<String, usize>,
    /// Art images that repeated an earlier image and share its FRM.
    pub art_duplicates: usize,
    /// New prototypes, by prototype directory.
    pub prototypes: BTreeMap<String, usize>,
    pub exits: Vec<ExitReport>,
    pub skipped_teleports: Vec<SkippedTeleport>,
    /// Every map of the layer: file and number, exterior first.
    pub maps: Vec<(String, u32)>,
    /// The first map number this layer could use (66 or the base table's
    /// record count).
    pub first_map_number: u32,
    /// Whether a full `data/maps.txt` was shipped.
    pub maps_txt: bool,
    /// Critters on the exterior map, in input order.
    pub critters: Vec<PlacedCritter>,
    /// Critters left off the map, in input order.
    pub skipped_critters: Vec<SkippedCritter>,
    /// Sprite sets added, in stem order (their list rows).
    pub critter_sets: Vec<CritterSetReport>,
    /// Critter prototypes added, in PID order (their list rows).
    pub critter_prototypes: Vec<CritterPrototypeReport>,
    /// The AI packet and team the critters got; `None` when none was
    /// placed.
    pub critter_ai_packet: Option<i32>,
    pub critter_team: Option<i32>,
    /// Plain-language notes on fallbacks taken.
    pub notes: Vec<String>,
    /// What the self-check found.
    pub verify: VerifyReport,
}

/// Build the layer for one map window (see the crate notes).
pub fn build_map(input: &BuildInput) -> Result<Built, BuildError> {
    assemble::build(input)
}
