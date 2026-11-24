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
//!    art lists, one prototype per distinct (art, role, blocking). With
//!    [`BuildOptions::standalone`] the walls and tiles lists start from
//!    only their reserved base entries instead ([`art_budget`] gives the
//!    free entries of either mode without a build).
//! 4. **Exits** ([`exits`]): a door whose reference teleports into an
//!    interior gets exit grids on the walkable hexes next to it (or, with
//!    none, on the nearest walkable doorstep on its outer side), leading to
//!    a stub map of its own, whose exit ring leads back to that door.
//!
//!    **Islands** ([`islands`]): the connected components of the walkable
//!    hexes by the engine's movement rule. An island that holds an exit, a
//!    return hex or a critter is joined to the largest component through
//!    the shortest passage of openable hexes ([`BuildOptions::reconnect`];
//!    see "Passages" below). The **entrance** is then the hex of the
//!    largest component nearest the window centre that no art hides
//!    ([`cover`]; see "Entrance" below).
//! 5. **Critters** ([`critters`]): each character on the free walkable hex
//!    nearest its source position, facing the hex direction nearest its
//!    yaw; its sprite set appended to `art/critters/critters.lst`, one
//!    prototype per distinct character in `proto/critters` with its name in
//!    `pro_crit.msg`.
//! 6. **Maps and layer**: the exterior and stub maps (version 20), the
//!    lists, prototypes, messages, `data/maps.txt` (when the base one is
//!    supplied) and `vault-mod.toml`, checked by [`verify_layer`] before
//!    they are returned. The written exterior map's objects are read back
//!    by the engine's movement rule into [`BuildReport::components`].
//!
//! # Passages
//!
//! With [`BuildOptions::reconnect`] (on by default), every island that
//! holds an exit none of whose grids is in the largest component, a
//! return hex, or a critter (where it would stand) is joined to the
//! largest component, islands taken largest first, the joined ones
//! counting as part of it from then on. The passage is the shortest path
//! of **openable** hexes from it to the island
//! ([`islands::shortest_passage`]): fewest hexes, then fewest off the
//! navmesh, then lowest hexes; at most
//! [`BuildOptions::passage_max_length`] hexes, widened by
//! [`BuildOptions::passage_rings`] rings of openable hexes. Opening a hex
//! makes it walkable: the sealing border is laid around the result, so
//! the passage is sealed like any other ground. A hex is openable when it
//! is not walkable, not on the map's outer border, its terrain is under
//! the slope limit (on the navmesh: [`islands::OpenCost::Standable`]; off
//! it: [`islands::OpenCost::OffNavmesh`]), and none of these is true:
//!
//! - it holds a wall strip, a door, or any placement that blocks;
//! - it is next to a door (exit grids keep the door's doorstep);
//! - wall art stands on it ([`cover::Cover::stands_on`]): the foot of a
//!   wall strip of its row (a merged run's hexes), or a pixel on its
//!   ground point of the landmark band whose rows hold its row (a
//!   landmark's blocked core; ground a band further south merely hides
//!   stays openable);
//! - it is in [`BuildOptions::protected_hexes`].
//!
//! So what a passage crosses is invisible sealing blockers (ground off the
//! navmesh) and blocked hexes with no wall art standing on them (bounding
//! footprints, baked relief). An island with no passage within the limit
//! is left alone and reported with the reason
//! ([`BuildReport::islands_left`]). Every passage opened is reported
//! ([`BuildReport::passages`]). Without an island to join, nothing
//! changes, and the layer is byte for byte what it was without the option.
//!
//! # Entrance
//!
//! The entrance is a walkable hex of the largest component that is not on
//! the outer border, holds no exit grid and is no return hex. Among those,
//! by [`cover::Sight`] of a figure [`BuildOptions::entrance_box`] pixels
//! large standing there: first one no placement's art overlaps on screen
//! (`Clear`), else one only art drawn behind it overlaps (`Behind`), else
//! any (`Covered`); within a tier, the one nearest the window centre, ties
//! to the lower hex. Only when the largest component offers no such hex,
//! the walkable hex nearest the centre that holds no exit grid (the rule
//! before components existed). [`BuildReport::entrance`] says which.
//!
//! # Readability
//!
//! [`BuildReport::readability`] measures the written map for a player:
//! floor hexes (walkable before blockers), walkable hexes, how a figure
//! standing on each walkable hex is drawn against the art
//! ([`cover::Sight`]), the open-floor ratio (clear walkable hexes over
//! floor hexes) and the share of walkable hexes within one step of a
//! blocking placement. [`BuildReport::readability_summary`] prints them.
//!
//! **Blocking is never art height.** What blocks movement comes from
//! [`BuildInput::blocked_hexes`], [`Placement::blocking`] and the role
//! alone: a wall strip trimmed to a low kerb (a foreground cutaway) blocks
//! its hex, is never opened by a passage and needs no sealing blocker,
//! exactly as a full-height one does. Art height matters only to
//! [`cover`]: what a figure is hidden by, and so the entrance and these
//! metrics.
//!
//! **Interactive references** ([`BuildOptions::interactive`]) are never
//! given the floor piece treatment; [`BuildReport::interactive`] counts
//! them.
//!
//! # Interior demo
//!
//! With [`BuildOptions::interior`] the build is one interior cell (its
//! [`mir::CellDoc`] with `location: Interior`, in [`BuildInput::cells`])
//! made into a standalone walkable map, the window laid over the cell's own
//! coordinates. Everything else is as above, except:
//!
//! - **Walkability** ([`walk::interior_verdicts`]): a hex is walkable when
//!   a navmesh triangle of an interior cell covers its centre (within
//!   [`BuildOptions::navmesh_tolerance`]), every corner of that triangle in
//!   the band [`InteriorBuild::band`], and it is not blocked. No terrain is
//!   read: [`BuildInput::sampler`] may be empty, and its contents never
//!   matter. Other exterior-only rules (slope, "too high", per-grid-cell
//!   navmesh presence) do not apply; passages open only hexes the band's
//!   navmesh covers.
//! - **Doors**: no exits and no stub maps. Every teleport door of the cells
//!   is reported ([`InteriorReport::doors`], [`BuildReport::skipped_teleports`])
//!   as not converted; a door placement is still drawn.
//! - **Entrance**: the rule above, nearest [`InteriorBuild::entrance_near`]
//!   instead of the window centre (by default the band's navmesh centre).
//! - **Critters**: each one's height is its reference's in the cells. One
//!   outside the band is skipped as on another level, or with
//!   [`InteriorBuild::clamp_levels`] placed within
//!   [`InteriorBuild::clamp_radius`] steps.
//! - **Ground**: [`BLANK_SQUARE`] squares, and squares of an all-transparent
//!   library tile, get the blank tile.
//! - The layer holds the one map; it is checked to hold no exit grid.
//!
//! # Layers
//!
//! [`build_layer`] builds several maps into one layer ([`LayerInput`]),
//! each of one to three elevations ([`MapInput::elevations`]). What maps
//! share is built once: the art, prototype and message lists, one FRM per
//! distinct image across the layer (a tile an earlier elevation wrote
//! shares its FRM too), one invisible blocker, one exit-grid marker, the
//! manifest and `data/maps.txt`, whose start point is each map's entrance
//! elevation. The art budget is the layer's: every list is checked once
//! against the sum (a [`BuildError::FidBudget`] of a layer of several maps
//! names each map's share). Maps are numbered in order (a given number is
//! honoured), stub maps after every real map; two maps may not share a
//! stem, nor a stub stem (a stem's first four letters) another map's.
//!
//! Each elevation is planned as the single-map build plans its one map:
//! walkability (its own band, [`ElevationInput::interior`]), passages,
//! the sealing border, readability, and its objects in its own list (ids
//! run over elevation 0, 1, 2). The entrance is on
//! [`MapInput::entrance_elevation`]. A critter goes to the elevation whose
//! band holds its reference's height in the cells (outside every band:
//! the nearest, as "on another level" unless that band clamps; unknown:
//! the entrance elevation; an exterior map: elevation 0).
//!
//! # Door links
//!
//! With [`LayerOptions::link_doors`], a teleport door whose destination
//! cell a map of the layer is built from gets exit grids (found as for a
//! stub exit) leading to that map: the elevation the partner door is
//! placed on (else the band holding the arrival point's height), onto a
//! walkable arrival hex 2 to [`exits::RETURN_RADIUS`] steps from the
//! partner door and on no exit grid, nearest the source's arrival point.
//! The partner door links back by the same rule, so every link has a
//! return unless one side finds no arrival hex (reported, the other side
//! linked one way). A door leading outside the build keeps a stub map on
//! an exterior map and is reported on an interior map.
//! [`StairLink`]s (from [`storeys::layout`]) are paired exit grids between
//! two landings, on one map or two: the engine changes elevation only
//! through exit grids. Every exit grid is then followed on the written
//! maps by [`verify_links`]. With [`LayerOptions::isolated`] nothing is
//! linked and the layer is checked to hold exactly its maps and no exit
//! grid.
//!
//! # Loot
//!
//! With a [`loot::Catalogue`] ([`BuildOptions::catalogue`] for
//! [`build_map`], [`LayerOptions::catalogue`] for [`build_layer`]) items
//! get real prototypes (one per definition, PIDs in sorted key order),
//! inventory icons and names; placed items, containers with contents and
//! locks, locked and keyed doors, and armed and armoured critters are
//! written through one `emit::ProtoSet`; scenery, doors and containers
//! get their real look-at names. [`verify_loot`] checks the result. The
//! [`loot`] module notes give the rules. Without a catalogue nothing of
//! this runs and the layer is byte for byte what it was.
//!
//! # Furniture footprints
//!
//! A placement blocks only its anchor hex, and the slicer's strips over
//! its open hexes block nothing, so a wide sprite or a sliced counter can
//! be walked into ([`furniture`] notes). [`BuildOptions::footprints`]
//! blocks every hex a reference's placed base covers; without it the
//! builder reports such furniture ([`FurnitureReport::suspects`]).

mod assemble;
mod budget;
pub mod cover;
pub mod critters;
mod error;
pub mod exits;
pub mod furniture;
pub mod islands;
mod layer;
mod links;
pub mod loot;
mod loot_build;
mod names;
mod passages;
mod plan;
mod readability;
pub mod storeys;
mod verify;
pub mod walk;

use std::collections::{BTreeMap, BTreeSet};

pub use budget::{ArtListBudget, art_budget};
pub use error::BuildError;
pub use furniture::{FurnitureReport, FurnitureSuspect, SuspectKind};
pub use loot::{
    Catalogue, CritterLoot, GroundArt, Held, ItemDef, ItemIcon, ItemKind, LootReport, LootSkip,
    MapLoot, PlacedLoot,
};
pub use verify::{
    LinkCheck, LootCheck, VerifyReport, read_layer, verify_layer, verify_links, verify_loot,
};
pub use walk::{dominant_floor_z, navmesh_centre};

/// The language folder of the prototype message files this crate reads
/// and writes.
pub const LANGUAGE: &str = "english";

/// Every base-game VFS path [`base_file_paths`] names.
const BASE_FILE_PATHS: [&str; 18] = [
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
    // The loot files, from here on.
    "art/inven/inven.lst",
];

/// Where the critter files start in [`BASE_FILE_PATHS`].
pub(crate) const CRITTER_BASE_FROM: usize = 13;
/// Where the loot files (read only with a [`loot::Catalogue`]) start.
pub(crate) const LOOT_BASE_FROM: usize = 17;

/// The base-game VFS paths the caller should read from the Fallout 2
/// install into [`BuildInput::base_files`]: art lists, prototype lists,
/// prototype message files and the map table, then the critter files
/// (`art/critters/critters.lst`, `proto/critters/critters.lst`,
/// `pro_crit.msg` and `data/ai.txt`). Each list that is supplied is
/// extended (a layer file replaces the base file whole); each one that is
/// missing starts empty. Under [`BuildOptions::standalone`] the walls and
/// tiles lists keep only their reserved base entries instead.
///
/// When any base file is supplied, the first thirteen must all be. The
/// critter files are needed only by a build that places critters: then
/// the three critter lists must be supplied too, and `data/ai.txt` as
/// well unless [`BuildOptions::critter_ai_packet`] names the packet. A
/// build without critters never reads them, so its output does not depend
/// on them. `data/ai.txt` is read, never shipped.
///
/// The last one, `art/inven/inven.lst` (inventory icons), is read only by
/// a build given a [`loot::Catalogue`]; such a build needs it whenever any
/// base file is supplied, and needs the critter files too when an armour
/// names a critter sprite set as its figure. A build without a catalogue
/// never reads it.
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
    /// On an interior build ([`BuildOptions::interior`]) a library tile
    /// whose pixels are all 0 is not written: its squares get the blank
    /// tile, as [`BLANK_SQUARE`] does.
    pub tiles: &'a [Vec<u8>],
    /// 10,000 library indices, numbered as `HexGrid` numbers squares;
    /// [`BLANK_SQUARE`] for the blank tile.
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
    /// Standalone mode, for demonstration layers (owner decision,
    /// 2026-11-22): the layer's `art/walls/walls.lst` and
    /// `art/tiles/tiles.lst` do not start from the base lists but from
    /// their reserved entries only (none for walls; floor tiles 0 and 1,
    /// the blank tile, copied from the base), so nearly all 4,096 entries
    /// of each are the layer's. Stock maps show the wrong walls and floors
    /// while such a layer is mounted. Every other list, the prototype
    /// lists and the message files are extended as usual. Default `false`.
    pub standalone: bool,
    /// Join islands that hold an exit, a return hex or a critter to the
    /// largest component through the shortest passage of openable hexes
    /// (crate notes, "Passages"). Default `true`; a map with no such island
    /// is unchanged by it.
    pub reconnect: bool,
    /// The longest passage opened, in hexes along it. Default
    /// [`PASSAGE_MAX_LENGTH`].
    pub passage_max_length: usize,
    /// Rings of openable hexes added around a passage's path: its width is
    /// `1 + 2 × rings` hexes where the barrier allows. Default 0, a path one
    /// hex wide (the engine walks it like any other).
    pub passage_rings: u32,
    /// Hexes never opened, besides those the crate finds itself: what the
    /// caller knows to be wall (a merged strip run's covered hexes, a
    /// landmark's blocked core). Default empty.
    pub protected_hexes: Vec<i32>,
    /// How far out (hex steps) a door with no walkable neighbour looks for
    /// a doorstep on its outer side (see [`exits`]); below 2 such a door is
    /// skipped. Default [`exits::DOOR_REACH`].
    pub door_reach: i32,
    /// The figure the entrance must not hide, `[width, height]` in pixels,
    /// standing with its bottom centre on the hex centre (see [`cover`]).
    /// Default [`ENTRANCE_BOX`].
    pub entrance_box: [u32; 2],
    /// Build an interior cell as a standalone demo map instead of an
    /// exterior window (crate notes, "Interior demo"). Default `None`: the
    /// exterior build, unchanged.
    pub interior: Option<InteriorBuild>,
    /// Reference form keys ([`Placement::reference`]) the caller marks as
    /// interactive or protected (doors, containers, terminals, anything a
    /// player must see and click). Their placements never get the floor
    /// piece treatment (flat, non-blocking, never cut): a
    /// [`Role::FloorPiece`] of one is built as upright [`Role::Scenery`]
    /// that blocks as [`Placement::blocking`] says. Every other placement
    /// of theirs is built as given. [`BuildReport::interactive`] counts
    /// them. Default empty: nothing changes.
    pub interactive: BTreeSet<String>,
    /// Items, loot and fighters ([`loot`]): item prototypes, placed items,
    /// filled and locked containers, locked doors, armed critters, and real
    /// names for scenery, doors and containers. Default `None`: nothing
    /// changes. In [`build_layer`] the catalogue is
    /// [`LayerOptions::catalogue`]; a map's own must be `None` or equal.
    pub catalogue: Option<std::sync::Arc<loot::Catalogue>>,
    /// Placed bounding footprints of furniture, by reference form key: the
    /// corners `[x east, y north]` (source units) of the reference's base
    /// bounds as placed, in order around the polygon (crate notes,
    /// "Furniture footprints"). Every hex whose centre lies inside (or
    /// within [`furniture::FOOTPRINT_REACH`] floor squares of) the polygon
    /// is blocked on each elevation where the reference has an upright
    /// placement (a wall strip, scenery or a container; doors, items and
    /// floor pieces never), and a passage never opens it. Default empty:
    /// nothing changes.
    pub footprints: BTreeMap<String, Vec<[f32; 2]>>,
}

/// The default of [`InteriorBuild::below`]: source units under the floor
/// that still count as the floor level.
pub const INTERIOR_BAND_BELOW: f32 = 40.0;
/// The default of [`InteriorBuild::above`]: source units over the floor
/// that still count as the floor level (a step, a low stage; not a
/// balcony or an upper storey).
pub const INTERIOR_BAND_ABOVE: f32 = 100.0;
/// The default of [`InteriorBuild::clamp_radius`]: how far (hex steps) a
/// critter from another level is moved under [`InteriorBuild::clamp_levels`].
pub const INTERIOR_CLAMP_RADIUS: i32 = 20;

/// The settings of an interior demo build ([`BuildOptions::interior`]).
///
/// The cell's own navmesh decides walkability: only triangles every corner
/// of which lies in the band `[floor_z - below, floor_z + above]` count;
/// no terrain is read. [`walk::dominant_floor_z`] suggests `floor_z`.
#[derive(Debug, Clone, PartialEq)]
pub struct InteriorBuild {
    /// The floor level, source units (the cell's own coordinates).
    pub floor_z: f32,
    /// The band around it, source units; both at least 0. Defaults
    /// [`INTERIOR_BAND_BELOW`] and [`INTERIOR_BAND_ABOVE`].
    pub below: f32,
    pub above: f32,
    /// The source point `(x, y)` the entrance should be near, such as a
    /// door's position; `None` takes the centre of the floor level's
    /// navmesh ([`walk::navmesh_centre`]).
    pub entrance_near: Option<[f32; 2]>,
    /// Place critters standing outside the band anyway, on the nearest free
    /// walkable hex within [`InteriorBuild::clamp_radius`] steps. Default
    /// `false`: they are skipped as on another level.
    pub clamp_levels: bool,
    /// Default [`INTERIOR_CLAMP_RADIUS`].
    pub clamp_radius: i32,
}

impl InteriorBuild {
    /// The defaults around a floor at `floor_z`.
    pub fn new(floor_z: f32) -> Self {
        Self {
            floor_z,
            below: INTERIOR_BAND_BELOW,
            above: INTERIOR_BAND_ABOVE,
            entrance_near: None,
            clamp_levels: false,
            clamp_radius: INTERIOR_CLAMP_RADIUS,
        }
    }

    /// The band's absolute bounds `[low, high]`.
    pub fn band(&self) -> [f32; 2] {
        [self.floor_z - self.below, self.floor_z + self.above]
    }

    /// Whether a source height lies in the band (bounds included).
    pub fn in_band(&self, z: f32) -> bool {
        let [low, high] = self.band();
        z >= low && z <= high
    }
}

impl Default for InteriorBuild {
    fn default() -> Self {
        Self::new(0.0)
    }
}

/// A [`GroundTiles::squares`] entry that names no library tile: the square
/// gets the blank floor tile (art index 1, drawn black; roof 1, not drawn).
/// Accepted in every build; ground outside an interior's room is the
/// typical use.
pub const BLANK_SQUARE: u16 = u16::MAX;

/// The default of [`BuildOptions::passage_max_length`].
pub const PASSAGE_MAX_LENGTH: usize = 6;

/// The default of [`BuildOptions::entrance_box`]: about a Fallout 2 man,
/// one hex wide.
pub const ENTRANCE_BOX: [u32; 2] = [32, 72];

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
            standalone: false,
            reconnect: true,
            passage_max_length: PASSAGE_MAX_LENGTH,
            passage_rings: 0,
            protected_hexes: Vec::new(),
            door_reach: exits::DOOR_REACH,
            entrance_box: ENTRANCE_BOX,
            interior: None,
            interactive: BTreeSet::new(),
            catalogue: None,
            footprints: BTreeMap::new(),
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
    /// 40,000 entries, by hex; passages opened included.
    pub walkable: Vec<bool>,
    pub report: BuildReport,
    /// What the catalogue ([`BuildOptions::catalogue`]) made; `None`
    /// without one.
    pub loot: Option<LootReport>,
    /// Furniture footprints used, and upright art that does not block
    /// where a walker can stand ([`FurnitureReport::summary`]).
    pub furniture: FurnitureReport,
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
    /// Hex steps from the door to its grids: 1 next to it, more for a door
    /// with no walkable neighbour (a doorstep further out, see [`exits`]).
    pub grid_steps: i32,
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

/// One art list after a build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArtListReport {
    /// The layer's list does not start from the base list (standalone
    /// mode; walls and tiles only).
    pub replaced: bool,
    /// Entries of the base game's list (0 when none was supplied).
    pub base_entries: usize,
    /// Entries the layer's list started with: the whole base list, or the
    /// reserved entries of a replaced one.
    pub kept: usize,
    /// Entries this build added.
    pub added: usize,
    /// Entries a FID could still address before and after this build.
    pub free_before: usize,
    pub free_after: usize,
    /// Whether the layer ships this list.
    pub shipped: bool,
}

/// How the entrance was chosen (crate notes, "Entrance").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EntranceReport {
    pub hex: i32,
    /// How a figure standing there is drawn against the art.
    pub sight: cover::Sight,
    /// Whether it is in the largest component (`false` only for the last
    /// fallback).
    pub in_main: bool,
    /// Hex steps from the walkable hex nearest the window centre (the
    /// entrance before components existed).
    pub steps_from_centre: i32,
}

/// Which component each thing a walker needs to reach is in, on the
/// written exterior map, numbered as [`ComponentReport::sizes`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExitComponents {
    pub door: String,
    pub door_hex: i32,
    /// Each exit grid's hex and component.
    pub grids: Vec<(i32, Option<usize>)>,
    pub return_hex: i32,
    pub return_component: Option<usize>,
}

/// A critter's component: the one it moves in from its hex.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CritterComponent {
    pub reference: String,
    pub hex: i32,
    pub component: Option<usize>,
}

/// The written exterior map's connected components by the engine's
/// movement rule ([`islands`]): every object that stops movement, critters
/// included, blocks its hex. Only components that hold a walkable hex are
/// listed (the rest is ground behind the sealing border).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentReport {
    /// Sizes in hexes, largest first; a component's number is its index.
    pub sizes: Vec<usize>,
    /// The lowest hex of each, in the same order.
    pub first_hexes: Vec<i32>,
    /// The entrance's component.
    pub main: Option<usize>,
    /// One per exit, in [`BuildReport::exits`] order.
    pub exits: Vec<ExitComponents>,
    /// One per placed critter, in [`BuildReport::critters`] order.
    pub critters: Vec<CritterComponent>,
    /// Hexes in a listed component that are not walkable: where the
    /// sealing border leaks (0 when it is sound).
    pub leaked: usize,
}

impl ComponentReport {
    /// Exits none of whose grids is in the main component.
    pub fn unreachable_exits(&self) -> usize {
        self.exits
            .iter()
            .filter(|e| !e.grids.iter().any(|(_, c)| c.is_some() && *c == self.main))
            .count()
    }

    /// Return hexes outside the main component.
    pub fn stranded_returns(&self) -> usize {
        self.exits
            .iter()
            .filter(|e| e.return_component.is_none() || e.return_component != self.main)
            .count()
    }
}

/// One passage opened between the largest component and an island.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PassageReport {
    /// The walkable hex it starts beside, and the island hex it ends
    /// beside.
    pub from: i32,
    pub to: i32,
    /// Hexes along the path, and of them off the navmesh.
    pub length: usize,
    pub off_navmesh: usize,
    /// Every hex opened (the path and its rings), sorted.
    pub opened: Vec<i32>,
    /// The island's size and lowest hex before it was joined.
    pub island_size: usize,
    pub island_hex: i32,
    /// What the island held: `exit <door>`, `return of <door>`,
    /// `critter <reference>`.
    pub holds: Vec<String>,
}

/// An island that holds something a walker needs and was not joined.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IslandReport {
    pub island_hex: i32,
    pub island_size: usize,
    pub holds: Vec<String>,
    pub reason: String,
}

/// How readable the written map is for a player (crate notes,
/// "Readability"). Deterministic counts over the 40,000 hexes; nothing in
/// the layer depends on them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReadabilityReport {
    /// Hexes walkable before blockers: the walkability pass's walkable
    /// hexes and those it rejected only because something blocks them.
    pub floor_hexes: usize,
    /// Walkable hexes on the written map (passages included).
    pub walkable_hexes: usize,
    /// Walkable hexes by how a figure of [`BuildOptions::entrance_box`]
    /// standing there is drawn against the placements' art
    /// ([`cover::Cover::sight`]): no art over it, only art drawn behind it,
    /// art drawn over it.
    pub clear: usize,
    pub behind: usize,
    pub covered: usize,
    /// `clear / floor_hexes`: the share of the floor a figure can stand on
    /// in plain view (0 without floor).
    pub open_floor_ratio: f64,
    /// Walkable hexes within one step of a hex holding a blocking placement
    /// (sealing blockers, critters and baked relief do not count).
    pub near_blocking: usize,
    /// `near_blocking / walkable_hexes` (0 without walkable hexes).
    pub near_blocking_share: f64,
}

/// What [`BuildOptions::interactive`] matched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractiveReport {
    /// Reference keys given.
    pub references: usize,
    /// Of those, keys at least one placement names.
    pub matched: usize,
    /// Placements of those references.
    pub placements: usize,
    /// Of those, floor pieces built upright instead.
    pub kept_upright: usize,
}

/// Counts and decisions of one build.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuildReport {
    /// `maps/<stem>.map` and its number.
    pub map_file: String,
    pub map_number: u32,
    pub entry_hex: i32,
    /// How the entrance was chosen.
    pub entrance: EntranceReport,
    /// The written exterior map's components, and where exits, returns and
    /// critters are in them.
    pub components: ComponentReport,
    /// Passages opened ([`BuildOptions::reconnect`]), in the order opened.
    pub passages: Vec<PassageReport>,
    /// Islands holding an exit, a return hex or a critter that were left
    /// as they were, with the reason.
    pub islands_left: Vec<IslandReport>,
    pub walk: walk::WalkStats,
    /// Hexes passages made walkable (not counted in `walk`, which is the
    /// walkability pass before them).
    pub opened_hexes: usize,
    /// Invisible blockers placed on the sealing border.
    pub blockers: usize,
    /// Objects on the exterior map: placements, blockers and exit grids.
    pub objects: usize,
    /// New art list entries, by art directory.
    pub art_entries: BTreeMap<String, usize>,
    /// Every art list the build loaded, by art directory: `items`,
    /// `scenery`, `walls`, `tiles`, `misc`, and `critters` when critters
    /// were placed.
    pub art_lists: BTreeMap<String, ArtListReport>,
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
    /// The interior demo's own account; `None` on an exterior build.
    pub interior: Option<InteriorReport>,
    /// How readable the written map is ([`BuildReport::readability_summary`]).
    pub readability: ReadabilityReport,
    /// What [`BuildOptions::interactive`] matched.
    pub interactive: InteractiveReport,
}

/// A teleport door of an interior demo, which leads nowhere on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteriorDoor {
    /// The door reference's form key.
    pub door: String,
    /// The hex of its door placement, when the slicer handed one over (it
    /// is drawn and opens, but leads nowhere).
    pub hex: Option<i32>,
    /// The destination cell's form key and editor id, when known.
    pub destination: Option<String>,
    pub destination_editor_id: Option<String>,
}

/// What an interior demo build did (crate notes, "Interior demo").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InteriorReport {
    /// The interior cells read: form key and editor id.
    pub cells: Vec<(String, Option<String>)>,
    pub floor_z: f32,
    /// The band's absolute bounds.
    pub band: [f32; 2],
    /// The source point the entrance was chosen near, and whether the
    /// caller gave it (otherwise the floor level's navmesh centre, or the
    /// window centre when that has none).
    pub entrance_near: [f32; 2],
    pub entrance_near_given: bool,
    /// Every teleport door of the cells, sorted by form key; each is also
    /// in [`BuildReport::skipped_teleports`].
    pub doors: Vec<InteriorDoor>,
    /// Critters skipped as standing on another level, also in
    /// [`BuildReport::skipped_critters`].
    pub other_level: Vec<SkippedCritter>,
    /// Critters from another level placed anyway
    /// ([`InteriorBuild::clamp_levels`]): reference and source height.
    pub clamped: Vec<(String, f32)>,
    /// Critters whose reference the cells do not hold, so whose height is
    /// unknown; they are placed as on the floor level.
    pub unknown_level: Vec<String>,
}

impl BuildReport {
    /// Plain-language lines on the map's readability
    /// ([`BuildReport::readability`]) and, when any were given, the
    /// interactive references ([`BuildReport::interactive`]).
    pub fn readability_summary(&self) -> Vec<String> {
        let r = &self.readability;
        let percent = |part: usize, whole: usize| {
            if whole == 0 {
                0.0
            } else {
                100.0 * part as f64 / whole as f64
            }
        };
        let mut lines = vec![
            format!(
                "floor: {} hexes walkable before blockers, {} walkable on the map",
                r.floor_hexes, r.walkable_hexes
            ),
            format!(
                "a standing figure on walkable hexes: {} clear ({:.1} %), {} behind art ({:.1} \
                 %), {} covered ({:.1} %)",
                r.clear,
                percent(r.clear, r.walkable_hexes),
                r.behind,
                percent(r.behind, r.walkable_hexes),
                r.covered,
                percent(r.covered, r.walkable_hexes)
            ),
            format!(
                "open-floor ratio {:.3} (clear walkable hexes / floor hexes)",
                r.open_floor_ratio
            ),
            format!(
                "{} walkable hexes ({:.1} %) are within one step of a blocking object",
                r.near_blocking,
                100.0 * r.near_blocking_share
            ),
        ];
        let i = &self.interactive;
        if i.references > 0 {
            lines.push(format!(
                "interactive references: {} given, {} on the map with {} placements; {} floor \
                 pieces kept upright",
                i.references, i.matched, i.placements, i.kept_upright
            ));
        }
        lines
    }

    /// Plain-language lines on an interior demo: the map, the doors not
    /// converted, and every critter placed or skipped with the reason.
    /// Empty on an exterior build.
    pub fn interior_summary(&self) -> Vec<String> {
        let Some(interior) = &self.interior else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        let cells: Vec<String> = interior
            .cells
            .iter()
            .map(|(form, id)| match id {
                Some(id) => format!("{id} ({form})"),
                None => form.clone(),
            })
            .collect();
        lines.push(format!(
            "interior demo: {} (map {}) from {}; one map, no exits, no stub maps",
            self.map_file,
            self.map_number,
            if cells.is_empty() {
                "no interior cell".to_owned()
            } else {
                cells.join(", ")
            }
        ));
        lines.push(format!(
            "floor level z {:.0}: navmesh from z {:.0} to {:.0}; {} walkable hexes, {} triangles \
             on other levels ignored",
            interior.floor_z,
            interior.band[0],
            interior.band[1],
            self.walk.walkable,
            self.walk.triangles_off_level
        ));
        lines.push(format!(
            "entrance: hex {} ({:?}), near ({:.0}, {:.0}){}",
            self.entrance.hex,
            self.entrance.sight,
            interior.entrance_near[0],
            interior.entrance_near[1],
            if interior.entrance_near_given {
                ""
            } else {
                ", the navmesh centre"
            }
        ));
        for door in &interior.doors {
            let to = match (&door.destination_editor_id, &door.destination) {
                (Some(id), _) => format!(" to {id}"),
                (None, Some(form)) => format!(" to {form}"),
                (None, None) => String::new(),
            };
            let drawn = match door.hex {
                Some(hex) => format!("drawn on hex {hex}"),
                None => "not drawn".to_owned(),
            };
            lines.push(format!(
                "door {}{to}: not converted on an interior demo ({drawn})",
                door.door
            ));
        }
        for critter in &self.critters {
            let clamped = interior
                .clamped
                .iter()
                .find(|(reference, _)| *reference == critter.reference)
                .map_or(String::new(), |(_, z)| {
                    format!(", from another level (z {z:.0})")
                });
            lines.push(format!(
                "placed {} on hex {} ({} steps from its position{clamped})",
                critter.reference, critter.hex, critter.steps
            ));
        }
        for critter in &self.skipped_critters {
            lines.push(format!("skipped {}: {}", critter.reference, critter.reason));
        }
        lines
    }
}

/// Build the layer for one map window (see the crate notes). A thin
/// adapter over [`build_layer`]: one map, one elevation, door links off,
/// and [`LayerOptions::isolated`] exactly when [`BuildOptions::interior`]
/// is set. Its output is byte for byte what it was before layers existed
/// (`tests/golden.rs`).
pub fn build_map(input: &BuildInput) -> Result<Built, BuildError> {
    assemble::build(input)
}

/// One elevation of a [`MapInput`] (crate notes, "Layers"): what the
/// terrain and the slicer produced for one storey. Every elevation of a
/// map shares its window, projection and so one [`walk::Frame`]: the hex
/// under a source point `(x, y)` is the same on each.
pub struct ElevationInput<'a> {
    /// The ground heights; indoors `mir::GroundSampler::flat(floor)` or an
    /// empty sampler (an interior elevation never reads it).
    pub sampler: &'a mir::GroundSampler,
    /// This elevation's own ground tiles (its own library).
    pub ground: GroundTiles<'a>,
    /// This elevation's own slicer output; [`Placement::art`] indexes it.
    pub art: &'a [ArtImage],
    pub placements: &'a [Placement],
    pub blocked_hexes: &'a [i32],
    /// The storey's floor and band: walkability is the interior rule
    /// ([`walk::interior_verdicts`]) over this band. `None`: the exterior
    /// rules (navmesh, terrain slope).
    pub interior: Option<InteriorBuild>,
}

/// One map of a layer: one to three elevations sharing a window.
pub struct MapInput<'a> {
    /// The map's file stem, as [`BuildInput::map_name`].
    pub map_name: &'a str,
    /// The map's number; `None` takes the next free one from the first a
    /// layer may use, in map order. Stub maps are numbered after every
    /// real map of the layer.
    pub map_number: Option<u32>,
    pub window: mir::MapWindow,
    pub projection: &'a foundry_render::Projection,
    /// Elevation `e` is `elevations[e]`; one to three.
    pub elevations: Vec<ElevationInput<'a>>,
    /// The elevation the map's entrance is on; `None`: the first elevation
    /// whose [`InteriorBuild::entrance_near`] is given, else 0.
    pub entrance_elevation: Option<u8>,
    /// The cells the map is built from (exterior cells of the window, or
    /// the interior cell), plus the interior cells its exterior doors lead
    /// to (return positions). With [`LayerOptions::link_doors`] a door
    /// leads to the map built from its destination cell: an exterior map
    /// is built from its exterior cells, an interior map (an elevation
    /// with [`ElevationInput::interior`]) from its interior cells.
    pub cells: &'a [mir::CellDoc],
    /// The characters; each goes to the elevation whose band holds its
    /// reference's height in `cells` (crate notes, "Layers").
    pub critters: &'a [CritterPlacement],
    /// Per-map tuning. The layer-wide fields (`standalone`,
    /// `write_maps_txt`, `critter_ai_packet`, `critter_team`) must agree
    /// with [`LayerOptions`], and `interior` must be `None` (each
    /// elevation carries its own band).
    pub options: BuildOptions,
}

/// Layer-wide settings of [`build_layer`].
#[derive(Debug, Clone, PartialEq)]
pub struct LayerOptions {
    /// As [`BuildOptions::standalone`].
    pub standalone: bool,
    /// As [`BuildOptions::write_maps_txt`].
    pub write_maps_txt: bool,
    /// As [`BuildOptions::critter_ai_packet`].
    pub critter_ai_packet: Option<i32>,
    /// As [`BuildOptions::critter_team`].
    pub critter_team: i32,
    /// The layer holds exactly the given maps and no exit grid: no door is
    /// converted (each is reported), no stub map is made, and stairs are
    /// refused. Today's interior demo; `build_map` sets it for
    /// [`BuildOptions::interior`].
    pub isolated: bool,
    /// Turn every teleport door whose destination cell a map of the layer
    /// is built from into exit grids to that map (crate notes, "Door
    /// links"). Off: exterior maps make stub maps as [`build_map`] does,
    /// interior maps report their doors.
    pub link_doors: bool,
    /// The layer's items, loot and fighters ([`loot`]), as
    /// [`BuildOptions::catalogue`]: one catalogue for every map (item
    /// definitions are layer-wide; references are keyed by form key, which
    /// no two maps share). Default `None`: nothing changes.
    pub catalogue: Option<std::sync::Arc<loot::Catalogue>>,
}

impl Default for LayerOptions {
    fn default() -> Self {
        Self {
            standalone: false,
            write_maps_txt: true,
            critter_ai_packet: None,
            critter_team: critters::DEFAULT_CRITTER_TEAM,
            isolated: false,
            link_doors: true,
            catalogue: None,
        }
    }
}

/// One end of a [`StairLink`]: a map of the layer (index into
/// [`LayerInput::maps`]), its elevation, and the landing point in that
/// map's source coordinates (where the stair or ramp meets the floor).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Landing {
    pub map: usize,
    pub elevation: u8,
    pub point: [f32; 3],
}

/// A stair, ramp or ladder between two floors: paired exit grids, each
/// end's grids on the walkable hexes at its landing leading to a walkable
/// arrival hex near the other landing ([`storeys::StoreyLayout::stair_links`]
/// makes them from a [`storeys::StoreySplit`]).
#[derive(Debug, Clone, PartialEq)]
pub struct StairLink {
    /// A short label for reports, such as `stairs 0-1`.
    pub label: String,
    pub ends: [Landing; 2],
}

/// Everything one layer is built from.
pub struct LayerInput<'a> {
    pub layer_id: &'a str,
    pub layer_name: &'a str,
    pub base_files: &'a BTreeMap<String, Vec<u8>>,
    /// The sprite sets every map's critters index.
    pub critter_art: &'a [CritterArt],
    /// One or more maps, numbered and written in this order.
    pub maps: Vec<MapInput<'a>>,
    pub stairs: Vec<StairLink>,
    pub options: LayerOptions,
}

/// What a link of exit grids is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LinkKind {
    /// A teleport door into a map of the layer.
    Door,
    /// A [`StairLink`] end.
    Stairs,
}

/// One door or stair end whose exit grids lead to a map of the layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkReport {
    pub kind: LinkKind,
    /// The door's form key, or the stair's label.
    pub from: String,
    /// The elevation its grids are on, the door's hex (doors only), and
    /// the grid hexes.
    pub elevation: u8,
    pub door_hex: Option<i32>,
    pub grid_hexes: Vec<i32>,
    /// Hex steps from the door (or landing) to the grids.
    pub grid_steps: i32,
    /// Where the grids lead: map file and number, elevation, arrival hex
    /// and facing.
    pub to_file: String,
    pub to_number: u32,
    pub to_elevation: u8,
    pub to_hex: i32,
    pub to_orientation: u8,
    /// Whether some link of the destination elevation leads back to this
    /// map and elevation.
    pub returns: bool,
}

/// One elevation of a built map.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ElevationReport {
    pub elevation: u8,
    /// The band walked (interior elevations).
    pub band: Option<[f32; 2]>,
    pub walk: walk::WalkStats,
    /// The entrance, on the entrance elevation only.
    pub entrance: Option<EntranceReport>,
    /// Components of the written elevation; `main` is the entrance's (or,
    /// off the entrance elevation, the first arrival hex's, else the
    /// largest).
    pub components: ComponentReport,
    pub passages: Vec<PassageReport>,
    pub islands_left: Vec<IslandReport>,
    pub opened_hexes: usize,
    pub blockers: usize,
    pub objects: usize,
    pub readability: ReadabilityReport,
    /// Arrival hexes other links lead to here, with what leads there and
    /// the arrival's component on the written elevation.
    pub arrivals: Vec<(String, i32, Option<usize>)>,
    /// References of the critters placed here.
    pub critters: Vec<String>,
}

/// One map of a built layer.
#[derive(Debug)]
pub struct BuiltMap {
    /// `maps/<stem>.map` and its number.
    pub file: String,
    pub number: u32,
    pub entrance_elevation: u8,
    /// Per elevation, 40,000 entries by hex; passages included.
    pub walkable: Vec<Vec<bool>>,
    pub elevations: Vec<ElevationReport>,
    /// Links whose grids are on this map, by elevation then door order.
    pub links: Vec<LinkReport>,
    /// The elevation of each of `report.critters`.
    pub critter_elevations: Vec<u8>,
    /// The map's report. On a map of several elevations its `walk`,
    /// `entrance`, `components`, `passages`, `islands_left`,
    /// `opened_hexes`, `blockers` and `readability` describe the entrance
    /// elevation (`elevations` has every one); `objects`, `critters`,
    /// `exits` and `skipped_teleports` cover the whole map; the layer-wide
    /// fields (`maps`, `art_*`, `prototypes`, `critter_sets`,
    /// `critter_prototypes`, `maps_txt`, `first_map_number`, `verify`)
    /// describe the layer.
    pub report: BuildReport,
    /// Furniture footprints used on this map, and upright art that does
    /// not block where a walker can stand.
    pub furniture: FurnitureReport,
}

/// What [`build_layer`] returns.
#[derive(Debug)]
pub struct BuiltLayer {
    pub layer: emit::ModLayer,
    pub maps: Vec<BuiltMap>,
    /// Stub maps (file, number) for doors leading outside the build.
    pub stubs: Vec<(String, u32)>,
    pub verify: VerifyReport,
    /// The exit-grid check over the written maps ([`verify_links`]).
    pub links: LinkCheck,
    /// What the catalogue ([`LayerOptions::catalogue`]) made, with its
    /// check ([`verify_loot`]); `None` without one.
    pub loot: Option<LootReport>,
}

/// Build one layer of several maps, each of one to three elevations, with
/// doors and stairs linked (crate notes, "Layers").
pub fn build_layer(input: &LayerInput) -> Result<BuiltLayer, BuildError> {
    assemble::build_layer(input)
}
