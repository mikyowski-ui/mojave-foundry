//! Planning (`DESIGN-levels.md` (b)): what is decided per map and per
//! elevation before any file is written. Phase A ([`plan_map`]) reads the
//! inputs: walkability and door grids. Phase B (`links`) resolves exits,
//! door links and stairs between the plans. Phase C ([`reconnect`],
//! [`place_critters`]) joins islands, chooses the entrance and places the
//! critters, per elevation.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use vault_formats::HexGrid;

use crate::cover::{Cover, Sight};
use crate::critters::{self, Spot};
use crate::exits::{DoorExit, DoorGrid, DoorKind};
use crate::layer::{TILE_HEIGHT, TILE_WIDTH};
use crate::walk::{self, Frame, HEXES, Verdict, WalkStats};
use crate::{
    BLANK_SQUARE, BuildError, BuildOptions, CRITTER_BASE_FROM, ElevationInput, EntranceReport,
    InteractiveReport, InteriorBuild, InteriorDoor, IslandReport, LOOT_BASE_FROM, LayerInput,
    LinkKind, MapInput, PassageReport, Placement, Role, SkippedCritter, SkippedTeleport,
    base_file_paths, islands, passages,
};

/// Items never block (the engine ignores item art for movement anyway).
pub(crate) fn effective_blocking(role: Role, blocking: bool) -> bool {
    blocking && !matches!(role, Role::Item | Role::FloorPiece)
}

/// Whether the engine stops a walker at this object: blocking, and wall
/// or scenery art (item and misc art never block movement).
pub(crate) fn engine_blocks(role: Role, blocking: bool) -> bool {
    blocking && matches!(role, Role::WallStrip | Role::Scenery | Role::Door)
}

pub(crate) fn role_art(role: Role) -> crate::ArtCategory {
    match role {
        Role::WallStrip => crate::ArtCategory::Walls,
        Role::Scenery | Role::Door | Role::FloorPiece => crate::ArtCategory::Scenery,
        Role::Container | Role::Item => crate::ArtCategory::Items,
    }
}

/// How doors are treated in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DoorMode {
    /// No door is converted ([`crate::LayerOptions::isolated`]).
    Isolated,
    /// Exterior maps make stub maps; interior maps list their doors.
    Stubs,
    /// Doors into maps of the layer are linked; the rest as `Stubs`.
    Linked,
}

/// A link planned from one elevation to a map of the layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedLink {
    pub kind: LinkKind,
    pub from: String,
    pub door_hex: Option<i32>,
    pub grid: Vec<i32>,
    pub steps: i32,
    /// Map index, elevation, arrival hex, facing.
    pub to: (usize, u8, i32, u8),
}

/// One elevation's plan.
pub(crate) struct ElevationPlan<'a> {
    pub input: &'a ElevationInput<'a>,
    /// The map's options with this elevation's band.
    pub options: BuildOptions,
    pub placements: Cow<'a, [Placement]>,
    pub engine_blocked: Vec<bool>,
    pub open: Vec<bool>,
    pub placement_blocks: Vec<bool>,
    pub verdicts: Vec<Verdict>,
    pub stats: WalkStats,
    pub walkable: Vec<bool>,
    pub door_hexes: BTreeSet<i32>,
    pub cover: Cover<'a>,
    /// Door grids found in phase A (stub and link doors).
    pub doors: Vec<DoorGrid>,
    /// Stairs grids found in phase B, by stair index.
    pub stair_grids: Vec<(usize, Vec<i32>, i32)>,
    pub door_exits: Vec<DoorExit>,
    pub links: Vec<PlannedLink>,
    /// Arrival hexes other links lead to here.
    pub arrivals: Vec<(i32, String)>,
    pub skipped: Vec<SkippedTeleport>,
    pub passages: Vec<PassageReport>,
    pub islands_left: Vec<IslandReport>,
    /// Components closed as unreached ([`crate::LayerOptions::close_unreached`]):
    /// first hex, size, what they held.
    pub closed: Vec<(i32, usize, Vec<String>)>,
    /// Unreached components kept because a critter would stand on them
    /// ([`crate::reach::Perch`]).
    pub perches: Vec<crate::reach::Perch>,
    /// With closing on: the critters (indices into the map's critters)
    /// that would have been placed on this elevation before anything was
    /// closed; `None` when closing is off.
    pub placed_before_closing: Option<Vec<usize>>,
    /// Items and containers moved off link hexes ([`crate::clearance`]).
    pub moved_off_links: usize,
    pub opened: BTreeSet<i32>,
    pub border: Vec<i32>,
    pub entrance: Option<EntranceReport>,
    pub spots: Vec<Spot>,
    pub skipped_critters: Vec<SkippedCritter>,
    /// The furniture footprints that apply here, and the hexes they alone
    /// blocked.
    pub footprints: crate::furniture::ElevationFootprints,
    pub footprint_blocked: usize,
}

impl ElevationPlan<'_> {
    /// Every exit grid hex of the elevation: stub exits and links.
    pub(crate) fn grid_hexes(&self) -> BTreeSet<i32> {
        self.door_exits
            .iter()
            .flat_map(|e| e.grid_hexes.iter().copied())
            .chain(self.links.iter().flat_map(|l| l.grid.iter().copied()))
            .collect()
    }

    /// Return hexes of the stub exits and arrival hexes of links.
    pub(crate) fn arrival_hexes(&self) -> BTreeSet<i32> {
        self.door_exits
            .iter()
            .map(|e| e.return_hex)
            .chain(self.arrivals.iter().map(|(hex, _)| *hex))
            .collect()
    }

    /// Every candidate grid hex found so far (door grids of any kind and
    /// stairs grids): what no arrival may land on.
    pub(crate) fn candidate_grids(&self) -> BTreeSet<i32> {
        self.doors
            .iter()
            .flat_map(|d| d.grid.iter().copied())
            .chain(self.stair_grids.iter().flat_map(|s| s.1.iter().copied()))
            .collect()
    }
}

/// Where a critter stands relative to the floor levels.
pub(crate) enum CritterLevel {
    Floor,
    /// Its reference is not in the cells.
    Unknown,
    Other(f32),
}

/// One map's plan.
pub(crate) struct MapPlan<'a> {
    pub input: &'a MapInput<'a>,
    pub stem: String,
    pub number: u32,
    pub frame: Frame,
    pub elevations: Vec<ElevationPlan<'a>>,
    pub entrance_elevation: usize,
    pub interactive: InteractiveReport,
    pub notes: Vec<String>,
    /// Doors listed, not converted (isolated builds, interior maps without
    /// links).
    pub listed_doors: Vec<InteriorDoor>,
    /// Per critter: the elevation whose pass places or skips it, its
    /// search radius or the reason it is skipped, and its level.
    pub critter_elevation: Vec<usize>,
    pub critter_radius: Vec<Result<i32, String>>,
    pub levels: Vec<CritterLevel>,
    pub window_centre: [f64; 2],
    /// An interior entrance elevation's target and whether it was given.
    pub entrance_near: Option<([f64; 2], bool)>,
    /// An exterior entrance elevation's target
    /// ([`BuildOptions::entrance_near`]), when given and finite.
    pub exterior_entrance: Option<[f64; 2]>,
    /// The level surfaces of an exterior map of levels.
    pub level_geometry: Option<crate::levels::LevelGeometry>,
}

impl MapPlan<'_> {
    pub(crate) fn entrance_hex(&self) -> Option<i32> {
        self.elevations[self.entrance_elevation]
            .entrance
            .map(|e| e.hex)
    }
}

/// Prefix an input error with the map (and elevation) when the layer has
/// more than one of either, so that single-map messages are unchanged.
pub(crate) fn locate(error: BuildError, map: &str, elevation: Option<usize>) -> BuildError {
    match error {
        BuildError::Input(message) => BuildError::Input(match elevation {
            Some(e) => format!("map {map}, elevation {e}: {message}"),
            None => format!("map {map}: {message}"),
        }),
        other => other,
    }
}

/// The base files check of the crate notes.
pub(crate) fn validate_base(layer: &LayerInput) -> Result<(), BuildError> {
    if layer.base_files.is_empty() {
        return Ok(());
    }
    let paths = base_file_paths();
    let mut needed: Vec<&str> = paths[..CRITTER_BASE_FROM].to_vec();
    let figures = layer.options.catalogue.as_ref().is_some_and(|c| {
        c.items
            .values()
            .any(|item| item.figure.iter().any(Option::is_some))
    });
    if layer.maps.iter().any(|m| !m.critters.is_empty()) || figures {
        needed.extend(
            paths[CRITTER_BASE_FROM..LOOT_BASE_FROM]
                .iter()
                .filter(|path| {
                    **path != critters::AI_TXT || layer.options.critter_ai_packet.is_none()
                }),
        );
    }
    if layer.options.catalogue.is_some() {
        needed.extend(&paths[LOOT_BASE_FROM..]);
    }
    let missing: Vec<&str> = needed
        .into_iter()
        .filter(|path| !layer.base_files.contains_key(*path))
        .collect();
    if !missing.is_empty() {
        return Err(BuildError::Input(format!(
            "base files are supplied but {} missing: {}; a layer list built without its base \
             file would replace the install's list with a shorter one",
            if missing.len() == 1 {
                "this one is"
            } else {
                "these are"
            },
            missing.join(", ")
        )));
    }
    Ok(())
}

/// One elevation's inputs.
pub(crate) fn validate_elevation(input: &ElevationInput) -> Result<(), BuildError> {
    let ground = &input.ground;
    if ground.squares.len() != 10_000 {
        return Err(BuildError::Input(format!(
            "the ground has {} squares; a map has 10000",
            ground.squares.len()
        )));
    }
    for (index, tile) in ground.tiles.iter().enumerate() {
        if tile.len() != TILE_WIDTH * TILE_HEIGHT {
            return Err(BuildError::Input(format!(
                "library tile {index} has {} bytes; a tile is 80 × 36",
                tile.len()
            )));
        }
    }
    if let Some((square, tile)) = ground
        .squares
        .iter()
        .enumerate()
        .find(|(_, tile)| **tile != BLANK_SQUARE && usize::from(**tile) >= ground.tiles.len())
    {
        return Err(BuildError::Input(format!(
            "square {square} names library tile {tile}; the library has {}",
            ground.tiles.len()
        )));
    }
    for (index, image) in input.art.iter().enumerate() {
        let area = (image.width as usize).checked_mul(image.height as usize);
        if area != Some(image.pixels.len()) {
            return Err(BuildError::Input(format!(
                "art {index} ({}) has {} pixel bytes for {}×{}",
                image.source,
                image.pixels.len(),
                image.width,
                image.height
            )));
        }
    }
    for (index, placement) in input.placements.iter().enumerate() {
        let Some(image) = input.art.get(placement.art) else {
            return Err(BuildError::Input(format!(
                "placement {index} names art {}; there are {}",
                placement.art,
                input.art.len()
            )));
        };
        if !(0..HEXES as i32).contains(&placement.hex) {
            return Err(BuildError::Input(format!(
                "placement {index} is on hex {}, outside 0 to 39999",
                placement.hex
            )));
        }
        if image.category != role_art(placement.role) {
            return Err(BuildError::Input(format!(
                "placement {index} is a {:?} with {:?} art; it needs {:?} art",
                placement.role,
                image.category,
                role_art(placement.role)
            )));
        }
    }
    if let Some(hex) = input
        .blocked_hexes
        .iter()
        .find(|hex| !(0..HEXES as i32).contains(*hex))
    {
        return Err(BuildError::Input(format!(
            "blocked hex {hex} is outside 0 to 39999"
        )));
    }
    Ok(())
}

fn validate_interior(interior: &InteriorBuild) -> Result<(), BuildError> {
    let finite_point = |p: [f32; 2]| p.iter().all(|v| v.is_finite());
    if !interior.floor_z.is_finite()
        || !(interior.below.is_finite() && interior.below >= 0.0)
        || !(interior.above.is_finite() && interior.above >= 0.0)
    {
        return Err(BuildError::Input(format!(
            "interior floor z {} with band -{} / +{}: the floor must be finite and the band \
             finite and not negative",
            interior.floor_z, interior.below, interior.above
        )));
    }
    if interior.entrance_near.is_some_and(|p| !finite_point(p)) {
        return Err(BuildError::Input(format!(
            "interior entrance point {:?} is not finite",
            interior.entrance_near
        )));
    }
    if interior.clamp_radius < 0 {
        return Err(BuildError::Input(format!(
            "interior clamp radius {} is negative",
            interior.clamp_radius
        )));
    }
    Ok(())
}

/// One map's inputs, in the order the single-map build checked them.
pub(crate) fn validate_map(
    map: &MapInput,
    layer: &LayerInput,
    multi: bool,
) -> Result<(), BuildError> {
    let count = map.elevations.len();
    if !(1..=3).contains(&count) {
        return Err(locate(
            BuildError::Input(format!("{count} elevations; a map has 1 to 3")),
            map.map_name,
            None,
        ));
    }
    if map
        .entrance_elevation
        .is_some_and(|e| usize::from(e) >= count)
    {
        return Err(locate(
            BuildError::Input(format!(
                "entrance elevation {:?} is not one of its {count}",
                map.entrance_elevation
            )),
            map.map_name,
            None,
        ));
    }
    if map.options.interior.is_some() {
        return Err(locate(
            BuildError::Input(
                "BuildOptions::interior is set; in a layer each ElevationInput carries its band"
                    .into(),
            ),
            map.map_name,
            None,
        ));
    }
    if map.options.level.is_some() {
        return Err(locate(
            BuildError::Input(
                "BuildOptions::level is set; in a layer each ElevationInput carries its level"
                    .into(),
            ),
            map.map_name,
            None,
        ));
    }
    if let Some(e) = map
        .elevations
        .iter()
        .position(|e| e.interior.is_some() && e.level.is_some())
    {
        return Err(locate(
            BuildError::Input(format!(
                "elevation {e} has both an interior band and an exterior level"
            )),
            map.map_name,
            None,
        ));
    }
    let o = &map.options;
    let l = &layer.options;
    if o.standalone != l.standalone
        || o.write_maps_txt != l.write_maps_txt
        || o.critter_ai_packet != l.critter_ai_packet
        || o.critter_team != l.critter_team
    {
        return Err(locate(
            BuildError::Input(
                "its BuildOptions disagree with LayerOptions on standalone, write_maps_txt, \
                 critter_ai_packet or critter_team"
                    .into(),
            ),
            map.map_name,
            None,
        ));
    }
    if o.catalogue.is_some() && o.catalogue != l.catalogue {
        return Err(locate(
            BuildError::Input(
                "its BuildOptions::catalogue differs from LayerOptions::catalogue; give the \
                 layer's catalogue, or none"
                    .into(),
            ),
            map.map_name,
            None,
        ));
    }
    for (e, elevation) in map.elevations.iter().enumerate() {
        validate_elevation(elevation).map_err(|error| {
            if multi {
                locate(error, map.map_name, Some(e))
            } else {
                error
            }
        })?;
    }
    let wrap = |error: BuildError| {
        if multi {
            locate(error, map.map_name, None)
        } else {
            error
        }
    };
    for (index, critter) in map.critters.iter().enumerate() {
        if critter.art >= layer.critter_art.len() {
            return Err(wrap(BuildError::Input(format!(
                "critter {index} ({}) names critter art {}; there are {}",
                critter.reference,
                critter.art,
                layer.critter_art.len()
            ))));
        }
    }
    if !map.critters.is_empty() {
        if l.critter_team <= 0 {
            return Err(wrap(BuildError::Input(format!(
                "critter team {} is not allowed: team 0 is the player's and teams are not \
                 negative",
                l.critter_team
            ))));
        }
        if l.critter_ai_packet.is_some_and(|packet| packet < 0) {
            return Err(wrap(BuildError::Input(format!(
                "critter AI packet {:?} is negative",
                l.critter_ai_packet
            ))));
        }
    }
    for (e, elevation) in map.elevations.iter().enumerate() {
        if let Some(interior) = &elevation.interior {
            validate_interior(interior).map_err(|error| {
                if multi {
                    locate(error, map.map_name, Some(e))
                } else {
                    error
                }
            })?;
        }
    }
    Ok(())
}

/// Which maps each cell builds: `cell form -> maps`, an interior map
/// owning its interior cells and an exterior map its exterior cells.
#[derive(Debug, Default)]
pub(crate) struct CellIndex {
    pub cells: BTreeMap<String, Vec<usize>>,
}

impl CellIndex {
    pub(crate) fn new(maps: &[MapInput]) -> Self {
        let mut cells: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, map) in maps.iter().enumerate() {
            let interior = map.elevations.iter().any(|e| e.interior.is_some());
            for cell in map.cells.iter().filter(|c| c.is_interior() == interior) {
                let owners = cells.entry(cell.form.to_string()).or_default();
                if !owners.contains(&index) {
                    owners.push(index);
                }
            }
        }
        Self { cells }
    }
}

/// Every teleport door of `cells` whose cell is interior (`interior`) or
/// exterior, sorted by form key, with the hex of its door placement on the
/// first elevation that places it.
fn listed_doors(
    cells: &[mir::CellDoc],
    interior: bool,
    placements: &[&[Placement]],
) -> Vec<InteriorDoor> {
    let mut doors: BTreeMap<String, InteriorDoor> = BTreeMap::new();
    for cell in cells.iter().filter(|cell| cell.is_interior() == interior) {
        for reference in &cell.references {
            let Some(teleport) = &reference.teleport else {
                continue;
            };
            let form = reference.form.to_string();
            doors.entry(form.clone()).or_insert_with(|| InteriorDoor {
                hex: placements.iter().find_map(|list| {
                    list.iter()
                        .find(|p| p.role == Role::Door && p.reference == form)
                        .map(|p| p.hex)
                }),
                door: form,
                destination: teleport.cell.as_ref().map(|c| c.form.to_string()),
                destination_editor_id: teleport.cell.as_ref().and_then(|c| c.editor_id.clone()),
            });
        }
    }
    doors.into_values().collect()
}

/// Phase A for one map: walkability and door grids per elevation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_map<'a>(
    input: &'a MapInput<'a>,
    stem: String,
    number: u32,
    mode: DoorMode,
    index: &CellIndex,
    references: &BTreeMap<String, mir::Reference>,
    single: bool,
) -> Result<MapPlan<'a>, BuildError> {
    let frame = Frame::new(&input.window, input.projection)?;
    let interior_map = input.elevations.iter().any(|e| e.interior.is_some());
    let multi_elevation = input.elevations.len() > 1;
    let mut notes = Vec::new();
    let mut elevations = Vec::new();
    let interactive_keys = &input.options.interactive;
    let mut interactive = InteractiveReport {
        references: interactive_keys.len(),
        ..InteractiveReport::default()
    };
    let mut matched: BTreeSet<&str> = BTreeSet::new();
    for (e, elevation) in input.elevations.iter().enumerate() {
        let mut options = input.options.clone();
        options.interior = elevation.interior.clone();
        options.level = elevation.level.clone();
        let (placements, report) =
            crate::readability::protect(elevation.placements, interactive_keys);
        interactive.placements += report.placements;
        interactive.kept_upright += report.kept_upright;
        for p in elevation.placements {
            if let Some(key) = interactive_keys.get(&p.reference) {
                matched.insert(key);
            }
        }
        let mut blocked = vec![false; HEXES];
        let mut engine_blocked = vec![false; HEXES];
        let mut open = vec![false; HEXES];
        for hex in elevation.blocked_hexes {
            blocked[*hex as usize] = true;
        }
        let mut placement_blocks = vec![false; HEXES];
        for placement in placements.iter() {
            let hex = placement.hex as usize;
            if effective_blocking(placement.role, placement.blocking) {
                placement_blocks[hex] = true;
                blocked[hex] = true;
            }
            if engine_blocks(placement.role, placement.blocking) {
                engine_blocked[hex] = true;
            }
            if placement.role == Role::Door {
                open[hex] = true;
            }
        }
        // Furniture footprints (crate notes): every hex of the base blocks,
        // and no passage opens it.
        let footprints =
            crate::furniture::elevation_footprints(&frame, &input.options.footprints, &placements);
        let mut footprint_blocked = 0;
        for hex in &footprints.hexes {
            if !blocked[*hex as usize] {
                footprint_blocked += 1;
                blocked[*hex as usize] = true;
            }
        }
        if !footprints.hexes.is_empty() {
            options.protected_hexes.extend(footprints.hexes.iter());
        }
        let (mut verdicts, mut stats) =
            walk::verdicts(&frame, elevation.sampler, input.cells, &blocked, &options);
        if options.coverage_bridges {
            let added = walk::bridge_coverage(
                &frame,
                elevation.sampler,
                input.cells,
                &blocked,
                &options,
                &mut verdicts,
                walk::BRIDGE_MAX_LENGTH,
            );
            stats.walkable += added.len();
            if !added.is_empty() {
                notes.push(format!(
                    "{}{} hexes bridge what the coverage rule cut apart: {added:?}",
                    if multi_elevation && e > 0 {
                        format!("elevation {e}: ")
                    } else {
                        String::new()
                    },
                    added.len()
                ));
            }
        }
        let walkable: Vec<bool> = verdicts
            .iter()
            .map(|v| *v == walk::Verdict::Walkable)
            .collect();
        if stats.walkable == 0 {
            return Err(if single {
                BuildError::NoWalkableHex
            } else {
                BuildError::Input(format!(
                    "map {}, elevation {e}: no hex is walkable",
                    input.map_name
                ))
            });
        }
        let prefix = if multi_elevation && e > 0 {
            format!("elevation {e}: ")
        } else {
            String::new()
        };
        if let Some(interior) = &elevation.interior {
            let [low, high] = interior.band();
            notes.push(format!(
                "{prefix}interior demo: walkable ground is the navmesh from z {low:.0} to \
                 {high:.0} ({} of {} triangles; {} on other levels ignored)",
                stats.triangles_rasterized, stats.triangles, stats.triangles_off_level
            ));
        } else if let Some(level) = &elevation.level {
            notes.push(format!(
                "{prefix}exterior level {}: walkable ground is its {} of {} navmesh triangles{}",
                level.level,
                stats.triangles_rasterized,
                stats.triangles,
                if level.level == 0 && stats.slope_only > 0 {
                    format!(", and {} hexes judged by slope alone", stats.slope_only)
                } else {
                    String::new()
                }
            ));
        } else if stats.triangles == 0 {
            notes.push(format!(
                "{prefix}no exterior cell of the window has a navmesh: walkability is the slope \
                 test alone"
            ));
        } else if stats.slope_only > 0 {
            notes.push(format!(
                "{prefix}{} hexes lie in cells without a navmesh and were judged by slope alone",
                stats.slope_only
            ));
        }
        // Doors.
        let convert = match mode {
            DoorMode::Isolated => false,
            DoorMode::Stubs => !interior_map,
            DoorMode::Linked => true,
        };
        let (doors, skipped, unknown_doors) = if convert {
            let back = |teleport: &mir::TeleportDoc, cell: &mir::CellRef| {
                if mode == DoorMode::Linked {
                    references
                        .get(&teleport.door.to_string())
                        .and_then(|door| door.teleport.as_ref())
                        .map(|t| t.position)
                        .or_else(|| crate::exits::interior_back(input.cells, teleport, cell))
                } else {
                    crate::exits::interior_back(input.cells, teleport, cell)
                }
            };
            crate::exits::door_grids(
                &frame,
                &placements,
                input.cells,
                &verdicts,
                options.door_reach,
                |_, cell| classify(mode, interior_map, index, cell),
                back,
            )
        } else {
            (Vec::new(), Vec::new(), 0)
        };
        if unknown_doors > 0 {
            notes.push(format!(
                "{prefix}{unknown_doors} door placements name references the cells do not hold; \
                 they are plain doors"
            ));
        }
        let door_hexes: BTreeSet<i32> = placements
            .iter()
            .filter(|placement| placement.role == Role::Door)
            .map(|placement| placement.hex)
            .collect();
        let cover = Cover::new(elevation.art, &placements);
        elevations.push(ElevationPlan {
            input: elevation,
            options,
            placements,
            engine_blocked,
            open,
            placement_blocks,
            verdicts,
            stats,
            walkable,
            door_hexes,
            cover,
            doors,
            stair_grids: Vec::new(),
            door_exits: Vec::new(),
            links: Vec::new(),
            arrivals: Vec::new(),
            skipped,
            passages: Vec::new(),
            islands_left: Vec::new(),
            closed: Vec::new(),
            perches: Vec::new(),
            placed_before_closing: None,
            moved_off_links: 0,
            opened: BTreeSet::new(),
            border: Vec::new(),
            entrance: None,
            spots: Vec::new(),
            skipped_critters: Vec::new(),
            footprints,
            footprint_blocked,
        });
    }
    interactive.matched = matched.len();
    // Doors listed instead of converted.
    let listing = match mode {
        DoorMode::Isolated => true,
        DoorMode::Stubs => interior_map,
        DoorMode::Linked => false,
    };
    let mut listed = Vec::new();
    if listing {
        let lists: Vec<&[Placement]> = input.elevations.iter().map(|e| e.placements).collect();
        listed = listed_doors(input.cells, interior_map, &lists);
        let why = match (mode, interior_map) {
            (DoorMode::Isolated, true) => "not converted on an interior demo",
            (DoorMode::Isolated, false) => "not converted on an isolated layer",
            _ => "not converted: door links are off (LayerOptions::link_doors)",
        };
        let skipped: Vec<SkippedTeleport> = listed
            .iter()
            .map(|door| SkippedTeleport {
                door: door.door.clone(),
                reason: format!(
                    "{why}{}",
                    match (&door.destination_editor_id, &door.destination) {
                        (Some(id), _) => format!(" (it leads to {id})"),
                        (None, Some(form)) => format!(" (it leads to {form})"),
                        (None, None) => String::new(),
                    }
                ),
            })
            .collect();
        elevations[0].skipped = skipped;
    }
    let entrance_elevation = usize::from(input.entrance_elevation.unwrap_or_else(|| {
        input
            .elevations
            .iter()
            .position(|e| {
                e.interior
                    .as_ref()
                    .is_some_and(|i| i.entrance_near.is_some())
            })
            .unwrap_or(0) as u8
    }));
    let window = &input.window;
    let window_centre = [
        f64::from(window.x_west) + f64::from(window.size) / 2.0,
        f64::from(window.y_north) - f64::from(window.size) / 2.0,
    ];
    let entrance_near = input.elevations[entrance_elevation]
        .interior
        .as_ref()
        .map(|interior| match interior.entrance_near {
            Some(p) => ([f64::from(p[0]), f64::from(p[1])], true),
            None => (
                walk::navmesh_centre(input.cells, interior)
                    .map_or(window_centre, |p| [f64::from(p[0]), f64::from(p[1])]),
                false,
            ),
        });
    let exterior_entrance = input
        .options
        .entrance_near
        .filter(|p| p.iter().all(|v| v.is_finite()))
        .filter(|_| input.elevations[entrance_elevation].interior.is_none())
        .map(|p| [f64::from(p[0]), f64::from(p[1])]);
    let level_geometry = crate::levels::LevelGeometry::new(
        input.cells,
        &input
            .elevations
            .iter()
            .map(|e| e.level.as_ref())
            .collect::<Vec<_>>(),
    );
    let (critter_elevation, critter_radius, levels) =
        assign_critters(input, entrance_elevation, level_geometry.as_ref());
    Ok(MapPlan {
        input,
        stem,
        number,
        frame,
        elevations,
        entrance_elevation,
        interactive,
        notes,
        listed_doors: listed,
        critter_elevation,
        critter_radius,
        levels,
        window_centre,
        entrance_near,
        exterior_entrance,
        level_geometry,
    })
}

/// What a door into `cell` becomes on a map of this kind.
fn classify(
    mode: DoorMode,
    interior_map: bool,
    index: &CellIndex,
    cell: &mir::CellRef,
) -> Result<DoorKind, String> {
    let inside = cell.location == mir::CellLocation::Interior;
    if mode == DoorMode::Linked && index.cells.contains_key(&cell.form.to_string()) {
        return Ok(DoorKind::Link);
    }
    match (inside, interior_map) {
        (true, false) => Ok(DoorKind::Stub),
        (false, false) => Err(format!(
            "a teleport to another exterior position ({}); not converted in this pass",
            cell.form
        )),
        (_, true) => Err(format!(
            "it leads outside the build ({}{}); not converted on an interior map",
            cell.form,
            cell.editor_id
                .as_ref()
                .map_or(String::new(), |id| format!(", {id}"))
        )),
    }
}

/// Per critter: the elevation whose pass places (or skips) it, its search
/// radius or why it is skipped, and its level (crate notes, "Layers").
fn assign_critters(
    input: &MapInput,
    entrance_elevation: usize,
    geometry: Option<&crate::levels::LevelGeometry>,
) -> (Vec<usize>, Vec<Result<i32, String>>, Vec<CritterLevel>) {
    if let Some(geometry) = geometry {
        // An exterior map of levels: the elevation whose surface the
        // reference stands on.
        let positions: BTreeMap<String, [f32; 3]> = input
            .cells
            .iter()
            .filter(|cell| !cell.is_interior())
            .flat_map(|cell| cell.references.iter())
            .map(|r| (r.form.to_string(), r.transform.position))
            .collect();
        let mut elevation_of = Vec::new();
        let mut radius = Vec::new();
        let mut levels = Vec::new();
        for critter in input.critters {
            let at = positions
                .get(&critter.reference)
                .and_then(|p| geometry.elevation_of(*p));
            elevation_of.push(at.unwrap_or(entrance_elevation));
            radius.push(Ok(critters::CRITTER_RADIUS));
            levels.push(if at.is_some() {
                CritterLevel::Floor
            } else {
                CritterLevel::Unknown
            });
        }
        return (elevation_of, radius, levels);
    }
    let bands: Vec<(usize, &InteriorBuild)> = input
        .elevations
        .iter()
        .enumerate()
        .filter_map(|(e, elevation)| elevation.interior.as_ref().map(|i| (e, i)))
        .collect();
    let mut elevation_of = Vec::new();
    let mut radius = Vec::new();
    let mut levels = Vec::new();
    if bands.is_empty() {
        for _ in input.critters {
            elevation_of.push(0);
            radius.push(Ok(critters::CRITTER_RADIUS));
        }
        return (elevation_of, radius, levels);
    }
    let heights: BTreeMap<String, f32> = input
        .cells
        .iter()
        .filter(|cell| cell.is_interior())
        .flat_map(|cell| cell.references.iter())
        .map(|r| (r.form.to_string(), r.transform.position[2]))
        .collect();
    for critter in input.critters {
        match heights.get(&critter.reference) {
            None => {
                elevation_of.push(entrance_elevation);
                radius.push(Ok(critters::CRITTER_RADIUS));
                levels.push(CritterLevel::Unknown);
            }
            Some(z) => {
                if let Some((e, _)) = bands.iter().find(|(_, band)| band.in_band(*z)) {
                    elevation_of.push(*e);
                    radius.push(Ok(critters::CRITTER_RADIUS));
                    levels.push(CritterLevel::Floor);
                    continue;
                }
                let distance = |band: &InteriorBuild| {
                    let [low, high] = band.band();
                    if *z < low { low - z } else { z - high }
                };
                let (e, band) = bands
                    .iter()
                    .min_by(|a, b| distance(a.1).total_cmp(&distance(b.1)).then(a.0.cmp(&b.0)))
                    .copied()
                    .expect("a band");
                elevation_of.push(e);
                levels.push(CritterLevel::Other(*z));
                if band.clamp_levels {
                    radius.push(Ok(band.clamp_radius));
                } else {
                    let [low, high] = band.band();
                    radius.push(Err(if bands.len() == 1 {
                        format!(
                            "on another level: it stands at z {z:.0}, the floor level is z \
                             {low:.0} to {high:.0} (InteriorBuild::clamp_levels places it anyway)"
                        )
                    } else {
                        format!(
                            "on another level: it stands at z {z:.0}, outside every floor level; \
                             the nearest is elevation {e}, z {low:.0} to {high:.0} \
                             (InteriorBuild::clamp_levels places it anyway)"
                        )
                    }));
                }
            }
        }
    }
    (elevation_of, radius, levels)
}

/// The walkability pass's own grid, before any passage.
pub(crate) fn walkable_before(verdicts: &[Verdict]) -> Vec<bool> {
    verdicts
        .iter()
        .map(|v| *v == walk::Verdict::Walkable)
        .collect()
}

/// A marker for critters another elevation's pass handles.
const OTHER_ELEVATION: &str = "\u{0}other elevation";

/// The critters of elevation `e` placed on `walkable` clear of `reserved`.
fn place_on(
    plan: &MapPlan,
    e: usize,
    walkable: &[bool],
    reserved: &BTreeSet<i32>,
) -> (Vec<Spot>, Vec<SkippedCritter>) {
    let radius = |index: usize| {
        if plan.critter_elevation[index] != e {
            Err(OTHER_ELEVATION.to_owned())
        } else {
            plan.critter_radius[index].clone()
        }
    };
    let (spots, skipped) =
        critters::place(&plan.frame, plan.input.critters, walkable, reserved, radius);
    let skipped = skipped
        .into_iter()
        .filter(|s| plan.critter_elevation[s.index] == e)
        .collect();
    (spots, skipped)
}

/// Phase C, first half: per elevation, join the islands holding exits,
/// arrivals and critters to the largest component, lay the sealing border,
/// and choose the entrance on the entrance elevation.
pub(crate) fn reconnect(plan: &mut MapPlan) -> Result<(), BuildError> {
    // An exterior entrance target (crate notes, "Entrance"): the entrance
    // elevation's islands join the component nearest it.
    let target = plan.exterior_entrance;
    for e in 0..plan.elevations.len() {
        let hub = match target {
            Some(t) if e == plan.entrance_elevation => {
                let at = plan.frame.tile_of_source(t[0], t[1]);
                walk::nearest_walkable(
                    &plan.frame,
                    &plan.elevations[e].walkable,
                    at,
                    &plan.elevations[e].grid_hexes(),
                )
            }
            _ => None,
        };
        let provisional: Vec<(String, i32)> = if plan.input.critters.is_empty() {
            Vec::new()
        } else {
            let elevation = &plan.elevations[e];
            let mut reserved = elevation.grid_hexes();
            reserved.extend(elevation.arrival_hexes());
            reserved.extend(&elevation.door_hexes);
            reserved.extend(crate::clearance::elevation_keep_clear(elevation));
            place_on(plan, e, &elevation.walkable, &reserved)
                .0
                .into_iter()
                .map(|spot| (plan.input.critters[spot.index].reference.clone(), spot.hex))
                .collect()
        };
        let frame = &plan.frame;
        let cells = plan.input.cells;
        let elevation = &mut plan.elevations[e];
        let links: Vec<(String, Vec<i32>)> = elevation
            .links
            .iter()
            .map(|l| {
                (
                    match l.kind {
                        LinkKind::Door => format!("exit {}", l.from),
                        LinkKind::Stairs => format!("stairs {}", l.from),
                    },
                    l.grid.clone(),
                )
            })
            .collect();
        let targets = passages::targets_with(
            &elevation.door_exits,
            &links,
            &elevation.arrivals,
            &provisional,
        );
        let mut walkable = std::mem::take(&mut elevation.walkable);
        let (passages, islands_left) = {
            let elevation = &*elevation;
            passages::reconnect(
                &mut walkable,
                &targets,
                || {
                    passages::open_costs(
                        frame,
                        elevation.input.sampler,
                        cells,
                        &elevation.options,
                        &walkable_before(&elevation.verdicts),
                        &elevation.placements,
                        |p| effective_blocking(p.role, p.blocking),
                        &elevation.cover,
                    )
                },
                &elevation.options,
                elevation.options.reconnect,
                hub,
            )
        };
        elevation.walkable = walkable;
        elevation.opened = passages
            .iter()
            .flat_map(|p| p.opened.iter().copied())
            .collect();
        elevation.passages = passages;
        elevation.islands_left = islands_left;
        elevation.border = walk::sealing_border(
            &elevation.walkable,
            &elevation.engine_blocked,
            &elevation.open,
        );
    }
    // The entrance (crate notes).
    let e = plan.entrance_elevation;
    let target = plan
        .entrance_near
        .map(|(p, _)| p)
        .or(target)
        .unwrap_or(plan.window_centre);
    let centre = plan.frame.tile_of_source(target[0], target[1]);
    let elevation = &plan.elevations[e];
    let grids = elevation.grid_hexes();
    let mut avoid = grids.clone();
    avoid.extend(elevation.arrival_hexes());
    let entrance = entrance(
        &plan.frame,
        &elevation.walkable,
        centre,
        &grids,
        &avoid,
        &elevation.cover,
        elevation.options.entrance_box,
        elevation.options.entrance_sight,
        plan.exterior_entrance.is_some(),
    )
    .ok_or(BuildError::NoWalkableHex)?;
    plan.elevations[e].entrance = Some(entrance);
    Ok(())
}

/// Phase C, second half: every critter on its elevation, clear of exits,
/// doors, the entrance, return and arrival hexes and passages (critter art
/// blocks movement).
pub(crate) fn place_critters(plan: &mut MapPlan) {
    if plan.input.critters.is_empty() {
        return;
    }
    for e in 0..plan.elevations.len() {
        let elevation = &plan.elevations[e];
        let reserved = critter_reserved(plan, e);
        let (spots, skipped) = place_on(plan, e, &elevation.walkable, &reserved);
        plan.elevations[e].spots = spots;
        plan.elevations[e].skipped_critters = skipped;
    }
}

/// The hexes no critter of elevation `e` may take: exit grids, doors, the
/// entrance, arrivals, passages and the keep-clear set of links.
fn critter_reserved(plan: &MapPlan, e: usize) -> BTreeSet<i32> {
    let elevation = &plan.elevations[e];
    let mut reserved = elevation.grid_hexes();
    if e == plan.entrance_elevation {
        reserved.extend(plan.entrance_hex());
    }
    reserved.extend(elevation.arrival_hexes());
    reserved.extend(&elevation.door_hexes);
    reserved.extend(&elevation.opened);
    reserved.extend(crate::clearance::elevation_keep_clear(elevation));
    reserved
}

/// Where the critters of elevation `e` would stand on its walkable grid as
/// it is now ([`place_critters`]'s rule, nothing recorded): what closing
/// the unreached asks before it closes anything.
pub(crate) fn would_place(plan: &MapPlan, e: usize) -> Vec<Spot> {
    if plan.input.critters.is_empty() {
        return Vec::new();
    }
    let reserved = critter_reserved(plan, e);
    place_on(plan, e, &plan.elevations[e].walkable, &reserved).0
}

/// The entrance by the crate notes' rule: in the largest component (with
/// `nearest_component`, the component of the walkable hex nearest
/// `centre`), off the border and `avoid`, clearest [`Sight`] by `rule`
/// first, nearest `centre` within a tier; else the walkable hex nearest
/// `centre` off `grids`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn entrance(
    frame: &Frame,
    walkable: &[bool],
    centre: [f64; 2],
    grids: &BTreeSet<i32>,
    avoid: &BTreeSet<i32>,
    cover: &Cover,
    figure: [u32; 2],
    rule: crate::cover::SightRule,
    nearest_component: bool,
) -> Option<EntranceReport> {
    let nearest = walk::nearest_walkable(frame, walkable, centre, grids)?;
    let comps = islands::components(|h| walkable[h as usize]);
    let sought = if nearest_component {
        comps.of(nearest).unwrap_or(0)
    } else {
        0
    };
    let distance = |hex: i32| {
        frame.hex_centre_tile(hex).map_or(f64::INFINITY, |c| {
            (c[0] - centre[0]).powi(2) + (c[1] - centre[1]).powi(2)
        })
    };
    let mut candidates: Vec<(f64, i32)> = comps
        .members(sought)
        .filter(|hex| !islands::on_border(*hex) && !avoid.contains(hex))
        .map(|hex| (distance(hex), hex))
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let steps = |hex: i32| HexGrid::lattice_distance(nearest, hex).unwrap_or(0);
    let mut fallback: [Option<i32>; 2] = [None, None];
    for (_, hex) in candidates {
        match cover.sight_by(hex, figure, rule) {
            Sight::Clear => {
                return Some(EntranceReport {
                    hex,
                    sight: Sight::Clear,
                    in_main: true,
                    steps_from_centre: steps(hex),
                });
            }
            Sight::Behind => {
                fallback[0].get_or_insert(hex);
            }
            Sight::Covered => {
                fallback[1].get_or_insert(hex);
            }
        }
    }
    for (slot, sight) in [(fallback[0], Sight::Behind), (fallback[1], Sight::Covered)] {
        if let Some(hex) = slot {
            return Some(EntranceReport {
                hex,
                sight,
                in_main: true,
                steps_from_centre: steps(hex),
            });
        }
    }
    Some(EntranceReport {
        hex: nearest,
        sight: cover.sight_by(nearest, figure, rule),
        in_main: comps.of(nearest) == Some(sought),
        steps_from_centre: 0,
    })
}
