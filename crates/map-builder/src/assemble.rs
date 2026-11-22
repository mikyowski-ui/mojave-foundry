//! [`build`]: from the input to a checked mod layer.

use std::collections::{BTreeMap, BTreeSet};

use emit::map::{CritterPlacement as CritterRecord, MAP_NAME_BYTES, empty_map, plain_object};
use emit::proto::{self, DoorSpec, ItemStats, Material, ObjectFlags, ProtoCommon};
use emit::{
    ArtList, CRITTER_LIST_PATH, CritterList, CritterSpec, EmitError, FID_INDEX_CAP, FormatWriters,
    FrmDirection, FrmFrameSpec, FrmSpec, LocalWriters, ModLayer, MsgFile, ProtoList,
    critter_object, critter_prototype,
};
use vault_formats::fallout2::Diagnostics;
use vault_formats::fallout2::maps::{MAX_MAP, MapsTable};
use vault_formats::fid::ART_DIRECTORIES;
use vault_formats::map::{MAX_OBJECTS, MapObject, ObjectDetail, ObjectState, TilePair};
use vault_formats::pro::{BasicFlags, Category, LockFlags};
use vault_formats::{Fid, Prototype};

use vault_formats::HexGrid;

use crate::cover::{Cover, Sight};
use crate::critters::{self, AI_TXT};
use crate::exits::{self, DoorExit, STUB_ENTRY, STUB_EXIT_RADIUS, STUB_FLOOR_RADIUS};
use crate::islands;
use crate::names::{ArtNames, map_stem, message_text, stub_stem};
use crate::passages;
use crate::walk::{self, Frame, HEXES};
use crate::{
    ArtCategory, ArtListReport, BLANK_SQUARE, BuildError, BuildInput, BuildReport, Built,
    CRITTER_BASE_FROM, ComponentReport, CritterComponent, CritterPrototypeReport, CritterSetReport,
    EntranceReport, ExitComponents, ExitReport, InteriorBuild, InteriorDoor, InteriorReport,
    LANGUAGE, PlacedCritter, Role, SkippedTeleport, base_file_paths,
};

use crate::budget::{
    ART_CRITTERS, ART_ITEMS, ART_MISC, ART_SCENERY, ART_TILES, ART_WALLS, BUILD_LISTS,
};
/// A floor tile's size.
pub(crate) const TILE_WIDTH: usize = 80;
pub(crate) const TILE_HEIGHT: usize = 36;
/// Tile art indexes 0 and 1 are reserved: 1 is the blank tile, and a roof
/// of index 1 is not drawn (SPEC-003 §2.8).
const RESERVED_TILES: usize = emit::RESERVED_TILE_ENTRIES;
/// The exit-grid prototype every exit uses (variant 0).
pub(crate) const EXIT_GRID_PID: u32 = 0x0500_0010;
pub(crate) const MAPS_TXT: &str = "data/maps.txt";

fn art_category(category: ArtCategory) -> u8 {
    match category {
        ArtCategory::Walls => ART_WALLS,
        ArtCategory::Scenery => ART_SCENERY,
        ArtCategory::Items => ART_ITEMS,
    }
}

fn role_art(role: Role) -> ArtCategory {
    match role {
        Role::WallStrip => ArtCategory::Walls,
        Role::Scenery | Role::Door | Role::FloorPiece => ArtCategory::Scenery,
        Role::Container | Role::Item => ArtCategory::Items,
    }
}

fn role_category(role: Role) -> Category {
    match role {
        Role::WallStrip => Category::Wall,
        Role::Scenery | Role::Door | Role::FloorPiece => Category::Scenery,
        Role::Container | Role::Item => Category::Item,
    }
}

/// Items never block (the engine ignores item art for movement anyway).
fn effective_blocking(role: Role, blocking: bool) -> bool {
    blocking && !matches!(role, Role::Item | Role::FloorPiece)
}

/// Whether the engine stops a walker at this object: blocking, and wall
/// or scenery art (item and misc art never block movement).
fn engine_blocks(role: Role, blocking: bool) -> bool {
    blocking && matches!(role, Role::WallStrip | Role::Scenery | Role::Door)
}

fn object_flags(role: Role, blocking: bool) -> ObjectFlags {
    match role {
        Role::FloorPiece => ObjectFlags::FLOOR,
        Role::Item => ObjectFlags {
            blocking: false,
            light_through: true,
            shoot_through: true,
            flat: true,
            no_highlight: false,
            never_cut: false,
        },
        _ if blocking => ObjectFlags::SOLID,
        _ => ObjectFlags {
            blocking: false,
            light_through: true,
            shoot_through: true,
            flat: false,
            no_highlight: false,
            never_cut: false,
        },
    }
}

/// The blocker: stops movement, lets light and shots through, and cannot
/// be highlighted.
const BLOCKER_FLAGS: ObjectFlags = ObjectFlags {
    blocking: true,
    light_through: true,
    shoot_through: true,
    flat: false,
    no_highlight: true,
    never_cut: false,
};

fn material(role: Role) -> Material {
    match role {
        Role::Door | Role::Container => Material::Wood,
        _ => Material::Stone,
    }
}

fn directory(category: u8) -> &'static str {
    ART_DIRECTORIES[usize::from(category)]
}

/// A one-direction, one-frame FRM.
fn frm(
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    shift: [i16; 2],
) -> Result<Vec<u8>, BuildError> {
    let dimension = |v: usize| {
        u16::try_from(v)
            .ok()
            .filter(|v| *v <= emit::frm::FRM_MAX_FIELD)
            .ok_or_else(|| {
                BuildError::Input(format!("an image dimension of {v} exceeds the format"))
            })
    };
    let spec = FrmSpec {
        fps: 0,
        action_frame: 0,
        directions: vec![FrmDirection {
            shift,
            frames: vec![FrmFrameSpec {
                width: dimension(width)?,
                height: dimension(height)?,
                displacement: [0, 0],
                pixels,
            }],
        }],
    };
    LocalWriters
        .frm(&spec)
        .map_err(|error| BuildError::emit("FRM", error))
}

/// The lists the layer extends, with what it changed.
struct Lists {
    art: BTreeMap<u8, ArtList>,
    /// Entries each art list started with (the base list, or the reserved
    /// entries of a standalone one).
    art_base: BTreeMap<u8, usize>,
    /// Entries of each base art list (0 without one).
    art_base_entries: BTreeMap<u8, usize>,
    protos: BTreeMap<Category, ProtoList>,
    messages: BTreeMap<Category, MsgFile>,
    touched_art: BTreeSet<u8>,
    touched_protos: BTreeSet<Category>,
    touched_messages: BTreeSet<Category>,
}

fn base_error(file: &str, error: impl std::fmt::Display) -> BuildError {
    BuildError::Base {
        file: file.into(),
        message: error.to_string(),
    }
}

impl Lists {
    fn load(base: &BTreeMap<String, Vec<u8>>, standalone: bool) -> Result<Self, BuildError> {
        let mut art = BTreeMap::new();
        let mut art_base = BTreeMap::new();
        let mut art_base_entries = BTreeMap::new();
        for category in BUILD_LISTS {
            let (list, entries) = crate::budget::start_list(category, base, standalone)?;
            art_base.insert(category, list.len());
            art_base_entries.insert(category, entries);
            art.insert(category, list);
        }
        let mut protos = BTreeMap::new();
        let mut messages = BTreeMap::new();
        for category in [
            Category::Item,
            Category::Scenery,
            Category::Wall,
            Category::Misc,
        ] {
            let path = proto::prototype_list_path(category);
            let list = match base.get(&path) {
                Some(bytes) => {
                    ProtoList::from_base(category, bytes).map_err(|e| base_error(&path, e))?
                }
                None => ProtoList::new(category),
            };
            protos.insert(category, list);
            if category != Category::Misc {
                let path = proto::message_file_path(category, LANGUAGE);
                let file = match base.get(&path) {
                    Some(bytes) => MsgFile::from_base(bytes).map_err(|e| base_error(&path, e))?,
                    None => MsgFile::new(),
                };
                messages.insert(category, file);
            }
        }
        Ok(Self {
            art,
            art_base,
            art_base_entries,
            protos,
            messages,
            touched_art: BTreeSet::new(),
            touched_protos: BTreeSet::new(),
            touched_messages: BTreeSet::new(),
        })
    }

    fn art_list(&mut self, category: u8) -> &mut ArtList {
        self.art
            .get_mut(&category)
            .expect("every art list is loaded")
    }

    /// Load the critter prototype list and `pro_crit.msg` (base or empty).
    fn load_critters(&mut self, base: &BTreeMap<String, Vec<u8>>) -> Result<(), BuildError> {
        let path = proto::prototype_list_path(Category::Critter);
        let list = match base.get(&path) {
            Some(bytes) => {
                ProtoList::from_base(Category::Critter, bytes).map_err(|e| base_error(&path, e))?
            }
            None => ProtoList::new(Category::Critter),
        };
        self.protos.insert(Category::Critter, list);
        let path = proto::message_file_path(Category::Critter, LANGUAGE);
        let file = match base.get(&path) {
            Some(bytes) => MsgFile::from_base(bytes).map_err(|e| base_error(&path, e))?,
            None => MsgFile::new(),
        };
        self.messages.insert(Category::Critter, file);
        Ok(())
    }

    /// Write `bytes` as the next free name of `category` and append it.
    fn add_art(
        &mut self,
        layer: &mut ModLayer,
        category: u8,
        names: &mut ArtNames,
        bytes: Vec<u8>,
    ) -> Result<Fid, BuildError> {
        let list = self.art_list(category);
        let name = names.next(list)?;
        let index = list
            .push(&name)
            .map_err(|error| BuildError::emit(list.list_path(), error))?;
        let fid = list.fid(index);
        let path = format!("art/{}/{name}", directory(category));
        layer
            .insert(&path, bytes)
            .map_err(|error| BuildError::emit(path.clone(), error))?;
        self.touched_art.insert(category);
        Ok(fid)
    }

    /// Append a prototype of `category` built by `make`, with its name and
    /// description messages.
    fn add_prototype(
        &mut self,
        layer: &mut ModLayer,
        category: Category,
        common: impl FnOnce(u32, i32) -> ProtoCommon,
        make: impl FnOnce(ProtoCommon) -> Result<Prototype, EmitError>,
        name: &str,
        description: &str,
    ) -> Result<Prototype, BuildError> {
        self.add_prototype_file(layer, category, common, make, name, description)
            .map(|(prototype, _)| prototype)
    }

    /// [`Lists::add_prototype`], also returning the prototype's path.
    fn add_prototype_file(
        &mut self,
        layer: &mut ModLayer,
        category: Category,
        common: impl FnOnce(u32, i32) -> ProtoCommon,
        make: impl FnOnce(ProtoCommon) -> Result<Prototype, EmitError>,
        name: &str,
        description: &str,
    ) -> Result<(Prototype, String), BuildError> {
        let list = self
            .protos
            .get_mut(&category)
            .expect("every prototype list is loaded");
        let list_path = list.list_path();
        let (pid, file) = list
            .push_next()
            .map_err(|error| BuildError::emit(list_path.clone(), error))?;
        let base = proto::default_message_base(pid & proto::PID_INDEX_CAP).ok_or_else(|| {
            BuildError::Input(format!(
                "{list_path}: no message number for PID {pid:#010x}"
            ))
        })?;
        let path = proto::prototype_path(category, &file);
        let prototype = make(common(pid, base)).map_err(|e| BuildError::emit(path.clone(), e))?;
        let bytes = LocalWriters
            .pro(&prototype)
            .map_err(|e| BuildError::emit(path.clone(), e))?;
        layer
            .insert(&path, bytes)
            .map_err(|e| BuildError::emit(path.clone(), e))?;
        let message_path = proto::message_file_path(category, LANGUAGE);
        let messages = self
            .messages
            .get_mut(&category)
            .ok_or_else(|| BuildError::Input(format!("no message file for {category:?}")))?;
        let entries = proto::message_entries(base, name.as_bytes(), description.as_bytes())
            .map_err(|e| BuildError::emit(message_path.clone(), e))?;
        for entry in entries {
            if messages.contains(entry.id) {
                return Err(base_error(
                    &message_path,
                    format!(
                        "message {} already exists; PID {pid:#010x} needs it",
                        entry.id
                    ),
                ));
            }
            messages
                .push(&entry)
                .map_err(|e| BuildError::emit(message_path.clone(), e))?;
        }
        self.touched_protos.insert(category);
        self.touched_messages.insert(category);
        Ok((prototype, path))
    }

    /// Append a prototype file to `category`'s list with no message.
    fn add_raw_prototype(
        &mut self,
        layer: &mut ModLayer,
        category: Category,
        make: impl FnOnce(u32) -> Result<Prototype, EmitError>,
    ) -> Result<Prototype, BuildError> {
        let list = self
            .protos
            .get_mut(&category)
            .expect("every prototype list is loaded");
        let list_path = list.list_path();
        let (pid, file) = list
            .push_next()
            .map_err(|error| BuildError::emit(list_path, error))?;
        let path = proto::prototype_path(category, &file);
        let prototype = make(pid).map_err(|e| BuildError::emit(path.clone(), e))?;
        let bytes = LocalWriters
            .pro(&prototype)
            .map_err(|e| BuildError::emit(path.clone(), e))?;
        layer
            .insert(&path, bytes)
            .map_err(|e| BuildError::emit(path, e))?;
        self.touched_protos.insert(category);
        Ok(prototype)
    }

    /// Put every changed list and message file in the layer.
    fn finish(&self, layer: &mut ModLayer) -> Result<(), BuildError> {
        for category in &self.touched_art {
            let list = &self.art[category];
            let path = list.list_path();
            layer
                .insert(&path, list.to_bytes())
                .map_err(|e| BuildError::emit(path, e))?;
        }
        for category in &self.touched_protos {
            let list = &self.protos[category];
            let path = list.list_path();
            layer
                .insert(&path, list.to_bytes())
                .map_err(|e| BuildError::emit(path, e))?;
        }
        for category in &self.touched_messages {
            let path = proto::message_file_path(*category, LANGUAGE);
            layer
                .insert(&path, self.messages[category].to_bytes())
                .map_err(|e| BuildError::emit(path, e))?;
        }
        Ok(())
    }
}

/// An art image's identity for sharing one FRM.
type ArtKey = (u8, u32, u32, [i16; 2], Vec<u8>);

fn validate(input: &BuildInput) -> Result<(), BuildError> {
    if !input.base_files.is_empty() {
        let paths = base_file_paths();
        let mut needed: Vec<&str> = paths[..CRITTER_BASE_FROM].to_vec();
        if !input.critters.is_empty() {
            needed.extend(
                paths[CRITTER_BASE_FROM..]
                    .iter()
                    .filter(|path| **path != AI_TXT || input.options.critter_ai_packet.is_none()),
            );
        }
        let missing: Vec<&str> = needed
            .into_iter()
            .filter(|path| !input.base_files.contains_key(*path))
            .collect();
        if !missing.is_empty() {
            return Err(BuildError::Input(format!(
                "base files are supplied but {} missing: {}; a layer list built without its \
                 base file would replace the install's list with a shorter one",
                if missing.len() == 1 {
                    "this one is"
                } else {
                    "these are"
                },
                missing.join(", ")
            )));
        }
    }
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
    for (index, critter) in input.critters.iter().enumerate() {
        if critter.art >= input.critter_art.len() {
            return Err(BuildError::Input(format!(
                "critter {index} ({}) names critter art {}; there are {}",
                critter.reference,
                critter.art,
                input.critter_art.len()
            )));
        }
    }
    if !input.critters.is_empty() {
        let options = &input.options;
        if options.critter_team <= 0 {
            return Err(BuildError::Input(format!(
                "critter team {} is not allowed: team 0 is the player's and teams are not \
                 negative",
                options.critter_team
            )));
        }
        if options.critter_ai_packet.is_some_and(|packet| packet < 0) {
            return Err(BuildError::Input(format!(
                "critter AI packet {:?} is negative",
                options.critter_ai_packet
            )));
        }
    }
    if let Some(interior) = &input.options.interior {
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
    }
    Ok(())
}

/// Every teleport door of the interior cells, sorted by form key, with the
/// hex of its door placement when there is one.
fn interior_doors(input: &BuildInput) -> Vec<InteriorDoor> {
    let mut doors: BTreeMap<String, InteriorDoor> = BTreeMap::new();
    for cell in input.cells.iter().filter(|cell| cell.is_interior()) {
        for reference in &cell.references {
            let Some(teleport) = &reference.teleport else {
                continue;
            };
            let form = reference.form.to_string();
            doors.entry(form.clone()).or_insert_with(|| InteriorDoor {
                hex: input
                    .placements
                    .iter()
                    .find(|p| p.role == Role::Door && p.reference == form)
                    .map(|p| p.hex),
                door: form,
                destination: teleport.cell.as_ref().map(|c| c.form.to_string()),
                destination_editor_id: teleport.cell.as_ref().and_then(|c| c.editor_id.clone()),
            });
        }
    }
    doors.into_values().collect()
}

/// Where each critter stands relative to the interior's floor level.
enum CritterLevel {
    Floor,
    /// Its reference is not in the cells.
    Unknown,
    Other(f32),
}

fn critter_levels(input: &BuildInput, interior: &InteriorBuild) -> Vec<CritterLevel> {
    let heights: BTreeMap<String, f32> = input
        .cells
        .iter()
        .filter(|cell| cell.is_interior())
        .flat_map(|cell| cell.references.iter())
        .map(|r| (r.form.to_string(), r.transform.position[2]))
        .collect();
    input
        .critters
        .iter()
        .map(|critter| match heights.get(&critter.reference) {
            None => CritterLevel::Unknown,
            Some(z) if interior.in_band(*z) => CritterLevel::Floor,
            Some(z) => CritterLevel::Other(*z),
        })
        .collect()
}

/// A critter's search radius by its level, or why it is skipped.
fn critter_radius(
    levels: &[CritterLevel],
    interior: Option<&InteriorBuild>,
    index: usize,
) -> Result<i32, String> {
    let Some(interior) = interior else {
        return Ok(critters::CRITTER_RADIUS);
    };
    match levels.get(index) {
        Some(CritterLevel::Other(_)) if interior.clamp_levels => Ok(interior.clamp_radius),
        Some(CritterLevel::Other(z)) => {
            let [low, high] = interior.band();
            Err(format!(
                "on another level: it stands at z {z:.0}, the floor level is z {low:.0} to \
                 {high:.0} (InteriorBuild::clamp_levels places it anyway)"
            ))
        }
        _ => Ok(critters::CRITTER_RADIUS),
    }
}

/// The AI packet for this build's critters, and a note when it fell back.
fn critter_packet(input: &BuildInput) -> Result<(i32, Option<String>), BuildError> {
    let ai_txt = input.base_files.get(AI_TXT);
    match (input.options.critter_ai_packet, ai_txt) {
        (Some(packet), Some(table)) => {
            if critters::has_ai_packet(table, packet)? {
                Ok((packet, None))
            } else {
                Err(BuildError::Input(format!(
                    "critter AI packet {packet} is not in the base {AI_TXT}; the engine would \
                     use the table's first packet instead"
                )))
            }
        }
        (Some(packet), None) => Ok((packet, None)),
        (None, Some(table)) => Ok((critters::choose_ai_packet(table)?, None)),
        (None, None) => Ok((
            critters::FALLBACK_AI_PACKET,
            Some(format!(
                "no base {AI_TXT}: critters use AI packet {}",
                critters::FALLBACK_AI_PACKET
            )),
        )),
    }
}

/// Prototype message text for a critter's description: printable ASCII as
/// for names, or the name when it is blank.
fn critter_description(description: &str, name: &str) -> String {
    if description.trim().is_empty() {
        name.to_owned()
    } else {
        message_text(description, 512)
    }
}

/// The largest contributors among `labels`, at most ten.
fn largest(labels: impl IntoIterator<Item = String>) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for label in labels {
        *counts.entry(label).or_default() += 1;
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.truncate(10);
    out
}

/// The blank floor tile's art index (drawn black).
const BLANK_TILE: u16 = 1;

/// A square's floor art index: its library tile's, or the blank tile for
/// [`BLANK_SQUARE`].
fn floor_of(tile_index: &[u16], library: u16) -> u16 {
    if library == BLANK_SQUARE {
        BLANK_TILE
    } else {
        tile_index[usize::from(library)]
    }
}

fn tile_word(floor: u16) -> TilePair {
    // Roof index 1: not drawn.
    let word = 0x0001_0000 | u32::from(floor & 0x0fff);
    TilePair { stored: word, word }
}

fn exit_object(
    id: u32,
    fid: Fid,
    hex: i32,
    flags: BasicFlags,
    map: u32,
    to: i32,
    facing: u8,
) -> MapObject {
    let mut object = plain_object(id, EXIT_GRID_PID, fid, hex, 0, flags);
    object.state = ObjectState::Other {
        word: 0,
        detail: ObjectDetail::ExitGrid {
            map: map as i32,
            hex: to,
            elevation: 0,
            orientation: i32::from(facing),
        },
    };
    object
}

/// The exterior map's objects read by the engine's movement rule: its
/// components that hold a walkable hex, and where the entrance, exits,
/// returns and placed critters (`report.critters`) are in them.
fn component_report(
    objects: &[MapObject],
    walkable: &[bool],
    entry: i32,
    door_exits: &[DoorExit],
    report: &BuildReport,
) -> ComponentReport {
    let blocked = islands::engine_blocked(objects);
    let comps =
        islands::components(|h| !blocked[h as usize]).retain_holding(|h| walkable[h as usize]);
    let leaked = (0..HEXES as i32)
        .filter(|h| comps.of(*h).is_some() && !walkable[*h as usize])
        .count();
    let mut exits: Vec<ExitComponents> = door_exits
        .iter()
        .map(|exit| ExitComponents {
            door: exit.door.clone(),
            door_hex: exit.door_hex,
            grids: exit
                .grid_hexes
                .iter()
                .map(|hex| (*hex, comps.of(*hex)))
                .collect(),
            return_hex: exit.return_hex,
            return_component: comps.of(exit.return_hex),
        })
        .collect();
    exits.sort_by(|a, b| a.door.cmp(&b.door).then(a.door_hex.cmp(&b.door_hex)));
    ComponentReport {
        sizes: comps.sizes().to_vec(),
        first_hexes: (0..comps.len())
            .filter_map(|id| comps.first_hex(id))
            .collect(),
        main: comps.of(entry),
        exits,
        critters: report
            .critters
            .iter()
            .map(|critter| CritterComponent {
                reference: critter.reference.clone(),
                hex: critter.hex,
                component: comps.around(critter.hex),
            })
            .collect(),
        leaked,
    }
}

/// The walkability pass's own grid, before any passage.
fn walkable_before(verdicts: &[walk::Verdict]) -> Vec<bool> {
    verdicts
        .iter()
        .map(|v| *v == walk::Verdict::Walkable)
        .collect()
}

/// The entrance by the crate notes' rule: in the largest component, off
/// the border and `avoid`, clearest [`Sight`] first, nearest `centre`
/// within a tier; else the walkable hex nearest `centre` off `grids`.
fn entrance(
    frame: &Frame,
    walkable: &[bool],
    centre: [f64; 2],
    grids: &BTreeSet<i32>,
    avoid: &BTreeSet<i32>,
    cover: &Cover,
    figure: [u32; 2],
) -> Option<EntranceReport> {
    let nearest = walk::nearest_walkable(frame, walkable, centre, grids)?;
    let comps = islands::components(|h| walkable[h as usize]);
    let distance = |hex: i32| {
        frame.hex_centre_tile(hex).map_or(f64::INFINITY, |c| {
            (c[0] - centre[0]).powi(2) + (c[1] - centre[1]).powi(2)
        })
    };
    let mut candidates: Vec<(f64, i32)> = comps
        .members(0)
        .filter(|hex| !islands::on_border(*hex) && !avoid.contains(hex))
        .map(|hex| (distance(hex), hex))
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let steps = |hex: i32| HexGrid::lattice_distance(nearest, hex).unwrap_or(0);
    let mut fallback: [Option<i32>; 2] = [None, None];
    for (_, hex) in candidates {
        match cover.sight(hex, figure) {
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
        sight: cover.sight(nearest, figure),
        in_main: comps.of(nearest) == Some(0),
        steps_from_centre: 0,
    })
}

fn header_name(stem: &str) -> Result<String, BuildError> {
    let name = format!("{}.MAP", stem.to_ascii_uppercase());
    if name.len() > MAP_NAME_BYTES {
        return Err(BuildError::Input(format!("map name {name} is too long")));
    }
    Ok(name)
}

pub(crate) fn build(input: &BuildInput) -> Result<Built, BuildError> {
    validate(input)?;
    let stem = map_stem(input.map_name)?;
    let options = &input.options;
    let mut report = BuildReport::default();
    let mut layer = ModLayer::new(input.layer_id, input.layer_name, env!("CARGO_PKG_VERSION"))
        .map_err(|e| BuildError::emit("vault-mod.toml", e))?;
    let mut lists = Lists::load(input.base_files, options.standalone)?;

    // Map numbers.
    let maps_txt = input.base_files.get(MAPS_TXT);
    let record_count = match maps_txt {
        Some(bytes) => MapsTable::parse(bytes, &mut Diagnostics::default())
            .map_err(|e| base_error(MAPS_TXT, e))?
            .count(),
        None => 0,
    };
    let first_number = (*vault_formats::mod_manifest::MAP_NUMBERS.start())
        .max(u32::try_from(record_count).unwrap_or(u32::MAX));
    report.first_map_number = first_number;
    let exterior_number = input.map_number.unwrap_or(first_number);
    if exterior_number < first_number {
        return Err(BuildError::Input(format!(
            "map number {exterior_number} is below {first_number}, the first one a layer may \
             register here"
        )));
    }
    let ship_maps_txt = options.write_maps_txt && maps_txt.is_some();

    // Walkability.
    let frame = Frame::new(&input.window, input.projection)?;
    let mut blocked = vec![false; HEXES];
    let mut engine_blocked = vec![false; HEXES];
    let mut open = vec![false; HEXES];
    for hex in input.blocked_hexes {
        blocked[*hex as usize] = true;
    }
    for placement in input.placements {
        let hex = placement.hex as usize;
        if effective_blocking(placement.role, placement.blocking) {
            blocked[hex] = true;
        }
        if engine_blocks(placement.role, placement.blocking) {
            engine_blocked[hex] = true;
        }
        if placement.role == Role::Door {
            open[hex] = true;
        }
    }
    let (verdicts, stats) = walk::verdicts(&frame, input.sampler, input.cells, &blocked, options);
    let mut walkable: Vec<bool> = verdicts
        .iter()
        .map(|v| *v == walk::Verdict::Walkable)
        .collect();
    if stats.walkable == 0 {
        return Err(BuildError::NoWalkableHex);
    }
    let interior = options.interior.as_ref();
    if let Some(interior) = interior {
        let [low, high] = interior.band();
        report.notes.push(format!(
            "interior demo: walkable ground is the navmesh from z {low:.0} to {high:.0} \
             ({} of {} triangles; {} on other levels ignored)",
            stats.triangles_rasterized, stats.triangles, stats.triangles_off_level
        ));
    } else if stats.triangles == 0 {
        report.notes.push(
            "no exterior cell of the window has a navmesh: walkability is the slope test alone"
                .into(),
        );
    } else if stats.slope_only > 0 {
        report.notes.push(format!(
            "{} hexes lie in cells without a navmesh and were judged by slope alone",
            stats.slope_only
        ));
    }
    report.walk = stats;

    // Exits; none on an interior demo, whose doors are listed instead.
    let interior_doors = if interior.is_some() {
        interior_doors(input)
    } else {
        Vec::new()
    };
    let (door_exits, skipped, unknown_doors) = if interior.is_some() {
        let skipped = interior_doors
            .iter()
            .map(|door| SkippedTeleport {
                door: door.door.clone(),
                reason: format!(
                    "not converted on an interior demo{}",
                    match (&door.destination_editor_id, &door.destination) {
                        (Some(id), _) => format!(" (it leads to {id})"),
                        (None, Some(form)) => format!(" (it leads to {form})"),
                        (None, None) => String::new(),
                    }
                ),
            })
            .collect();
        (Vec::new(), skipped, 0)
    } else {
        exits::plan(
            &frame,
            input.placements,
            input.cells,
            &verdicts,
            options.door_reach,
        )
    };
    if unknown_doors > 0 {
        report.notes.push(format!(
            "{unknown_doors} door placements name references the cells do not hold; they are \
             plain doors"
        ));
    }
    report.skipped_teleports = skipped;
    let grid_hexes: BTreeSet<i32> = door_exits
        .iter()
        .flat_map(|exit| exit.grid_hexes.iter().copied())
        .collect();
    // One stub per door, so that leaving an interior with several doors
    // comes out at the door that was used.
    let mut stubs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, exit) in door_exits.iter().enumerate() {
        stubs.entry(exit.door.clone()).or_default().push(index);
    }

    let door_hexes: BTreeSet<i32> = input
        .placements
        .iter()
        .filter(|placement| placement.role == Role::Door)
        .map(|placement| placement.hex)
        .collect();
    let return_hexes: BTreeSet<i32> = door_exits.iter().map(|exit| exit.return_hex).collect();

    // Islands: join those holding an exit, a return hex or a critter (where
    // it would stand, the entrance aside) to the largest component.
    let cover = Cover::new(input.art, input.placements);
    let levels = match interior {
        Some(interior) => critter_levels(input, interior),
        None => Vec::new(),
    };
    let radius = |index: usize| critter_radius(&levels, interior, index);
    let provisional: Vec<(String, i32)> = if input.critters.is_empty() {
        Vec::new()
    } else {
        let mut reserved: BTreeSet<i32> = grid_hexes.clone();
        reserved.extend(&return_hexes);
        reserved.extend(&door_hexes);
        critters::place(&frame, input.critters, &walkable, &reserved, radius)
            .0
            .into_iter()
            .map(|spot| (input.critters[spot.index].reference.clone(), spot.hex))
            .collect()
    };
    let targets = passages::targets(&door_exits, &provisional);
    let (passages, islands_left) = passages::reconnect(
        &mut walkable,
        &targets,
        || {
            passages::open_costs(
                &frame,
                input.sampler,
                input.cells,
                options,
                &walkable_before(&verdicts),
                input.placements,
                |p| effective_blocking(p.role, p.blocking),
                &cover,
            )
        },
        options,
        options.reconnect,
    );
    let opened: BTreeSet<i32> = passages
        .iter()
        .flat_map(|p| p.opened.iter().copied())
        .collect();
    report.opened_hexes = opened.len();
    report.passages = passages;
    report.islands_left = islands_left;
    let border = walk::sealing_border(&walkable, &engine_blocked, &open);

    // Entrance (crate notes); an interior's is near its given point or its
    // floor's navmesh centre.
    let window = &input.window;
    let window_centre = [
        f64::from(window.x_west) + f64::from(window.size) / 2.0,
        f64::from(window.y_north) - f64::from(window.size) / 2.0,
    ];
    let entrance_near = interior.map(|interior| match interior.entrance_near {
        Some(p) => ([f64::from(p[0]), f64::from(p[1])], true),
        None => (
            walk::navmesh_centre(input.cells, interior)
                .map_or(window_centre, |p| [f64::from(p[0]), f64::from(p[1])]),
            false,
        ),
    });
    let target = entrance_near.map_or(window_centre, |(p, _)| p);
    let centre = frame.tile_of_source(target[0], target[1]);
    let mut avoid = grid_hexes.clone();
    avoid.extend(&return_hexes);
    report.entrance = entrance(
        &frame,
        &walkable,
        centre,
        &grid_hexes,
        &avoid,
        &cover,
        options.entrance_box,
    )
    .ok_or(BuildError::NoWalkableHex)?;
    let entry = report.entrance.hex;
    report.entry_hex = entry;

    // Critters: every sprite set a critter names is checked; then each
    // critter gets its hex, clear of exits, doors, the entrance and the
    // return hexes (critter art blocks movement).
    let mut critter_files: BTreeMap<usize, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    for critter in input.critters {
        if let std::collections::btree_map::Entry::Vacant(slot) = critter_files.entry(critter.art) {
            slot.insert(critters::checked_art(
                critter.art,
                &input.critter_art[critter.art],
            )?);
        }
    }
    let spots = if input.critters.is_empty() {
        Vec::new()
    } else {
        // A critter standing in a passage would close it again.
        let mut reserved: BTreeSet<i32> = grid_hexes.clone();
        reserved.insert(entry);
        reserved.extend(&return_hexes);
        reserved.extend(&door_hexes);
        reserved.extend(&opened);
        let (spots, skipped) =
            critters::place(&frame, input.critters, &walkable, &reserved, radius);
        report.skipped_critters = skipped;
        spots
    };
    let used_sets: BTreeSet<usize> = spots
        .iter()
        .map(|spot| input.critters[spot.index].art)
        .collect();
    let mut critter_list = None;
    let mut critter_ai = (0, 0);
    if !spots.is_empty() {
        let list = match input.base_files.get(CRITTER_LIST_PATH) {
            Some(bytes) => {
                CritterList::from_base(bytes).map_err(|e| base_error(CRITTER_LIST_PATH, e))?
            }
            None => CritterList::new(),
        };
        let requested = list.len() + used_sets.len();
        if requested > FID_INDEX_CAP {
            return Err(BuildError::FidBudget {
                category: "critters",
                cap: FID_INDEX_CAP,
                base: list.len(),
                requested,
                largest: largest(
                    used_sets
                        .iter()
                        .map(|art| input.critter_art[*art].label.clone()),
                ),
            });
        }
        critter_list = Some(list);
        lists.load_critters(input.base_files)?;
        let (packet, note) = critter_packet(input)?;
        report.notes.extend(note);
        critter_ai = (packet, options.critter_team);
        report.critter_ai_packet = Some(packet);
        report.critter_team = Some(options.critter_team);
    }

    // The FID budget, checked before anything is written.
    let mut unique: BTreeMap<ArtKey, usize> = BTreeMap::new();
    let mut new_labels: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    for (index, image) in input.art.iter().enumerate() {
        let category = art_category(image.category);
        let key = (
            category,
            image.width,
            image.height,
            image.shift,
            image.pixels.clone(),
        );
        if let std::collections::btree_map::Entry::Vacant(slot) = unique.entry(key) {
            slot.insert(index);
            new_labels
                .entry(category)
                .or_default()
                .push(image.source.clone());
        } else {
            report.art_duplicates += 1;
        }
    }
    let tile_pad = RESERVED_TILES.saturating_sub(lists.art_base[&ART_TILES]);
    // An interior's all-transparent library tiles are the blank tile.
    let blank_tile: Vec<bool> = input
        .ground
        .tiles
        .iter()
        .map(|tile| interior.is_some() && tile.iter().all(|p| *p == 0))
        .collect();
    let written_tiles = blank_tile.iter().filter(|blank| !**blank).count();
    new_labels
        .entry(ART_TILES)
        .or_default()
        .extend(std::iter::repeat_n(
            "ground tiles".to_owned(),
            written_tiles + tile_pad,
        ));
    new_labels
        .entry(ART_SCENERY)
        .or_default()
        .push("invisible blocker".into());
    if !door_exits.is_empty() {
        new_labels
            .entry(ART_MISC)
            .or_default()
            .push("exit grid marker".into());
    }
    for (category, labels) in &new_labels {
        let base = lists.art_base[category];
        let requested = base + labels.len();
        if requested > FID_INDEX_CAP {
            return Err(BuildError::FidBudget {
                category: directory(*category),
                cap: FID_INDEX_CAP,
                base,
                requested,
                largest: largest(labels.iter().cloned()),
            });
        }
    }

    // Floor tiles: the reserved positions of an empty list, then the
    // library in order.
    let mut tile_names = ArtNames::new("mft");
    for _ in 0..tile_pad {
        let blank = frm(
            TILE_WIDTH,
            TILE_HEIGHT,
            vec![0; TILE_WIDTH * TILE_HEIGHT],
            [0, 0],
        )?;
        lists.add_art(&mut layer, ART_TILES, &mut tile_names, blank)?;
    }
    let mut tile_index = Vec::with_capacity(input.ground.tiles.len());
    for (tile, blank) in input.ground.tiles.iter().zip(&blank_tile) {
        if *blank {
            tile_index.push(BLANK_TILE);
            continue;
        }
        let bytes = frm(TILE_WIDTH, TILE_HEIGHT, tile.clone(), [0, 0])?;
        let fid = lists.add_art(&mut layer, ART_TILES, &mut tile_names, bytes)?;
        tile_index.push(fid.base());
    }

    // Sprites, one FRM per distinct image.
    let mut names: BTreeMap<u8, ArtNames> = [
        (ART_ITEMS, ArtNames::new("mfi")),
        (ART_SCENERY, ArtNames::new("mfs")),
        (ART_WALLS, ArtNames::new("mfw")),
        (ART_MISC, ArtNames::new("mfm")),
    ]
    .into();
    let mut art_fid: Vec<Fid> = Vec::with_capacity(input.art.len());
    let mut written: BTreeMap<usize, Fid> = BTreeMap::new();
    for image in input.art {
        let category = art_category(image.category);
        let key = (
            category,
            image.width,
            image.height,
            image.shift,
            image.pixels.clone(),
        );
        let first = unique[&key];
        if let Some(fid) = written.get(&first) {
            art_fid.push(*fid);
            continue;
        }
        let (width, height, pixels) = if image.pixels.is_empty() {
            (1, 1, vec![0])
        } else {
            (
                image.width as usize,
                image.height as usize,
                image.pixels.clone(),
            )
        };
        let bytes = frm(width, height, pixels, image.shift)?;
        let namer = names.get_mut(&category).expect("a namer per sprite list");
        let fid = lists.add_art(&mut layer, category, namer, bytes)?;
        written.insert(first, fid);
        art_fid.push(fid);
    }
    let blocker_fid = {
        let bytes = frm(1, 1, vec![0], [0, 0])?;
        let namer = names.get_mut(&ART_SCENERY).expect("a scenery namer");
        lists.add_art(&mut layer, ART_SCENERY, namer, bytes)?
    };
    let exit_fid = if door_exits.is_empty() {
        None
    } else {
        let bytes = frm(1, 1, vec![0], [0, 0])?;
        let namer = names.get_mut(&ART_MISC).expect("a misc namer");
        Some(lists.add_art(&mut layer, ART_MISC, namer, bytes)?)
    };
    for category in &lists.touched_art {
        let added = lists.art[category].len() - lists.art_base[category];
        report
            .art_entries
            .insert(directory(*category).into(), added);
    }
    for category in BUILD_LISTS {
        let kept = lists.art_base[&category];
        let len = lists.art[&category].len();
        report.art_lists.insert(
            directory(category).into(),
            ArtListReport {
                replaced: crate::budget::replaced(category, options.standalone),
                base_entries: lists.art_base_entries[&category],
                kept,
                added: len - kept,
                free_before: FID_INDEX_CAP - kept,
                free_after: FID_INDEX_CAP - len,
                shipped: lists.touched_art.contains(&category),
            },
        );
    }

    // Prototypes: one per distinct (art, role, blocking).
    let mut prototypes: BTreeMap<(usize, Role, bool), (u32, BasicFlags)> = BTreeMap::new();
    for placement in input.placements {
        let blocking = effective_blocking(placement.role, placement.blocking);
        let key = (placement.art, placement.role, blocking);
        if prototypes.contains_key(&key) {
            continue;
        }
        let role = placement.role;
        let fid = art_fid[placement.art];
        let flags = object_flags(role, blocking);
        let source = message_text(&input.art[placement.art].source, 48);
        let name = format!("{} ({source})", role.label());
        let description = format!("Converted from {source}.");
        let common = |pid, message_base| ProtoCommon {
            pid,
            fid,
            message_base,
            flags,
            material: material(role),
        };
        let capacity = options.container_capacity;
        let prototype = lists.add_prototype(
            &mut layer,
            role_category(role),
            common,
            |common| match role {
                Role::WallStrip => proto::wall(common),
                Role::Scenery | Role::FloorPiece => proto::scenery(common),
                Role::Door => proto::door(common, DoorSpec::default()),
                Role::Container => proto::container(common, ItemStats::default(), capacity, false),
                Role::Item => proto::misc_item(common, ItemStats::default()),
            },
            &name,
            &description,
        )?;
        prototypes.insert(key, (prototype.pid, prototype.flags));
    }
    let blocker = lists.add_prototype(
        &mut layer,
        Category::Scenery,
        |pid, message_base| ProtoCommon {
            pid,
            fid: blocker_fid,
            message_base,
            flags: BLOCKER_FLAGS,
            material: Material::Stone,
        },
        proto::scenery,
        "Impassable ground",
        "You cannot go that way.",
    )?;
    let exit_flags = ObjectFlags::DECAL.bits();
    if let Some(exit_fid) = exit_fid {
        // Exit grids are misc PIDs 16 to 23 by number: a base list that
        // already reaches position 16 keeps its own; a shorter one is
        // padded with placeholder misc prototypes up to it.
        let misc_len = lists.protos[&Category::Misc].len();
        if misc_len < 16 {
            let grid = proto::exit_grid(0, exit_fid, proto::NO_MESSAGE)
                .map_err(|e| BuildError::emit("proto/misc", e))?;
            for _ in misc_len..15 {
                let template = grid.clone();
                lists.add_raw_prototype(&mut layer, Category::Misc, |pid| {
                    Ok(Prototype { pid, ..template })
                })?;
            }
            let placed = lists.add_raw_prototype(&mut layer, Category::Misc, |_| Ok(grid))?;
            if placed.pid != EXIT_GRID_PID {
                return Err(BuildError::Input(format!(
                    "the exit-grid prototype landed at PID {:#010x}",
                    placed.pid
                )));
            }
        }
    }

    // Critter sprite sets: one list row each, in ascending art index.
    let mut set_art: BTreeMap<usize, (String, Fid)> = BTreeMap::new();
    if let Some(list) = critter_list.as_mut() {
        let mut next_stem = 0;
        for art in &used_sets {
            let stem = critters::next_stem(list, &mut next_stem)?;
            let row = list
                .append(&stem)
                .map_err(|e| BuildError::emit(CRITTER_LIST_PATH, e))?;
            let mut files = Vec::new();
            for (suffix, bytes) in &critter_files[art] {
                let path = format!("art/critters/{stem}{suffix}.frm");
                layer
                    .insert(&path, bytes.clone())
                    .map_err(|e| BuildError::emit(path.clone(), e))?;
                files.push(path);
            }
            // The stand and walk FIDs must name the files just written.
            let mut stand = None;
            for animation in [0, 1] {
                let resolved = list
                    .art(row, animation, 0)
                    .map_err(|e| BuildError::emit(CRITTER_LIST_PATH, e))?;
                if !files.contains(&resolved.path) {
                    return Err(BuildError::Input(format!(
                        "critter row {row} animation {animation} resolves to {}, which the \
                         set does not hold",
                        resolved.path
                    )));
                }
                stand.get_or_insert(resolved.fid);
            }
            let stand = stand.ok_or_else(|| BuildError::Input("no stand FID".into()))?;
            set_art.insert(*art, (stem.clone(), stand));
            report.critter_sets.push(CritterSetReport {
                art: *art,
                label: input.critter_art[*art].label.clone(),
                stem,
                row,
                files,
            });
        }
        layer
            .insert(CRITTER_LIST_PATH, list.to_bytes())
            .map_err(|e| BuildError::emit(CRITTER_LIST_PATH, e))?;
        report
            .art_entries
            .insert(directory(ART_CRITTERS).into(), used_sets.len());
        let kept = list.len() - used_sets.len();
        report.art_lists.insert(
            directory(ART_CRITTERS).into(),
            ArtListReport {
                replaced: false,
                base_entries: kept,
                kept,
                added: used_sets.len(),
                free_before: FID_INDEX_CAP - kept,
                free_after: list.remaining(),
                shipped: true,
            },
        );
    }

    // Critter prototypes: one per distinct character, in placement order.
    type CritterKey = (usize, String, String, i32, bool);
    let mut critter_protos: BTreeMap<CritterKey, Prototype> = BTreeMap::new();
    let mut spot_keys: Vec<CritterKey> = Vec::with_capacity(spots.len());
    for spot in &spots {
        let critter = &input.critters[spot.index];
        let name = message_text(&critter.name, 128);
        let description = critter_description(&critter.description, &name);
        let key = (
            critter.art,
            name.clone(),
            description.clone(),
            critter.hit_points,
            critter.female,
        );
        spot_keys.push(key.clone());
        if critter_protos.contains_key(&key) {
            continue;
        }
        let (stem, stand) = set_art[&critter.art].clone();
        let mut spec = CritterSpec::human(critter.hit_points, critter_ai.0, critter_ai.1);
        spec.female = critter.female;
        let (prototype, file) = lists.add_prototype_file(
            &mut layer,
            Category::Critter,
            |pid, message_base| ProtoCommon {
                pid,
                fid: stand,
                message_base,
                flags: ObjectFlags::SOLID,
                material: Material::default(),
            },
            |common| critter_prototype(common, &spec),
            &name,
            &description,
        )?;
        report.critter_prototypes.push(CritterPrototypeReport {
            pid: prototype.pid,
            file,
            stem,
            name,
            description,
            hit_points: critter.hit_points,
            female: critter.female,
        });
        critter_protos.insert(key, prototype);
    }
    // The exterior map's objects: placements, blockers, exit grids.
    let mut objects: Vec<MapObject> = Vec::new();
    let mut next_id = 1u32;
    let mut take_id = || {
        let id = next_id;
        next_id += 1;
        id
    };
    for placement in input.placements {
        let blocking = effective_blocking(placement.role, placement.blocking);
        let (pid, flags) = prototypes[&(placement.art, placement.role, blocking)];
        let mut object = plain_object(
            take_id(),
            pid,
            art_fid[placement.art],
            placement.hex,
            0,
            flags,
        );
        object.displacement = placement.displacement;
        object.state = ObjectState::Other {
            word: 0,
            detail: match placement.role {
                Role::Door => ObjectDetail::Door(LockFlags(0)),
                Role::Item => ObjectDetail::Charges(0),
                _ => ObjectDetail::Plain,
            },
        };
        objects.push(object);
    }
    for hex in &border {
        objects.push(plain_object(
            take_id(),
            blocker.pid,
            blocker_fid,
            *hex,
            0,
            blocker.flags,
        ));
    }
    report.blockers = border.len();

    // Stub numbering: doors sorted by form key.
    let mut stub_numbers: BTreeMap<String, (String, u32)> = BTreeMap::new();
    for (index, door) in stubs.keys().enumerate() {
        let number = exterior_number
            .checked_add(1 + index as u32)
            .ok_or_else(|| BuildError::Input("map numbers overflow".into()))?;
        stub_numbers.insert(door.clone(), (stub_stem(&stem, index)?, number));
    }
    let mut placed_grids = BTreeSet::new();
    if let Some(exit_fid) = exit_fid {
        for exit in &door_exits {
            let (_, number) = &stub_numbers[&exit.door];
            for hex in &exit.grid_hexes {
                if placed_grids.insert(*hex) {
                    objects.push(exit_object(
                        take_id(),
                        exit_fid,
                        *hex,
                        exit_flags,
                        *number,
                        STUB_ENTRY,
                        0,
                    ));
                }
            }
        }
    }
    for (spot, key) in spots.iter().zip(&spot_keys) {
        let critter = &input.critters[spot.index];
        let prototype = &critter_protos[key];
        let record =
            CritterRecord::from_prototype(prototype, take_id(), spot.hex, 0, spot.orientation)
                .map_err(|e| BuildError::emit(format!("critter {}", critter.reference), e))?;
        objects.push(
            critter_object(&record)
                .map_err(|e| BuildError::emit(format!("critter {}", critter.reference), e))?,
        );
        report.critters.push(PlacedCritter {
            reference: critter.reference.clone(),
            index: spot.index,
            hex: spot.hex,
            orientation: spot.orientation,
            steps: spot.steps,
            moved: spot.moved,
            pid: prototype.pid,
            stem: set_art[&critter.art].0.clone(),
        });
    }
    if objects.len() > MAX_OBJECTS {
        return Err(BuildError::Input(format!(
            "the map needs {} objects; the format holds {MAX_OBJECTS}",
            objects.len()
        )));
    }
    report.objects = objects.len();
    report.components = component_report(&objects, &walkable, entry, &door_exits, &report);
    if report.components.leaked > 0 {
        report.notes.push(format!(
            "{} hexes that are not walkable can be walked onto from walkable ones: the sealing \
             border leaks",
            report.components.leaked
        ));
    }

    // The exterior map.
    let map_file = format!("{stem}.map");
    let mut map = empty_map(&header_name(&stem)?, exterior_number as i32, entry)
        .map_err(|e| BuildError::emit(map_file.clone(), e))?;
    map.tiles[0] = Some(
        input
            .ground
            .squares
            .iter()
            .map(|library| tile_word(floor_of(&tile_index, *library)))
            .collect(),
    );
    map.objects.total = objects.len();
    map.objects.elevations[0] = objects;
    let mut maps: Vec<(String, u32, i32)> = vec![(map_file.clone(), exterior_number, entry)];
    let bytes = LocalWriters
        .map(&map)
        .map_err(|e| BuildError::emit(map_file.clone(), e))?;
    layer
        .insert(&format!("maps/{map_file}"), bytes)
        .map_err(|e| BuildError::emit(map_file.clone(), e))?;
    report.map_file = format!("maps/{map_file}");
    report.map_number = exterior_number;

    // Stub maps.
    for (door, doors) in &stubs {
        let (stub, number) = &stub_numbers[door];
        let first: &DoorExit = &door_exits[doors[0]];
        let exit_fid =
            exit_fid.ok_or_else(|| BuildError::Input("an exit without exit-grid art".into()))?;
        let floor = Frame::square_of_hex(first.door_hex)
            .and_then(|square| input.ground.squares.get(square as usize))
            .map_or(1, |library| floor_of(&tile_index, *library));
        let mut tiles = vec![TilePair::BLANK; 10_000];
        for hex in exits::hexes_around(STUB_ENTRY, STUB_FLOOR_RADIUS, false) {
            if let Some(square) = Frame::square_of_hex(hex) {
                tiles[square as usize] = tile_word(floor);
            }
        }
        let mut objects = Vec::new();
        for (id, hex) in exits::hexes_around(STUB_ENTRY, STUB_EXIT_RADIUS, true)
            .into_iter()
            .enumerate()
        {
            objects.push(exit_object(
                id as u32 + 1,
                exit_fid,
                hex,
                exit_flags,
                exterior_number,
                first.return_hex,
                first.return_orientation,
            ));
        }
        let file = format!("{stub}.map");
        let mut map = empty_map(&header_name(stub)?, *number as i32, STUB_ENTRY)
            .map_err(|e| BuildError::emit(file.clone(), e))?;
        map.tiles[0] = Some(tiles);
        map.objects.total = objects.len();
        map.objects.elevations[0] = objects;
        let bytes = LocalWriters
            .map(&map)
            .map_err(|e| BuildError::emit(file.clone(), e))?;
        layer
            .insert(&format!("maps/{file}"), bytes)
            .map_err(|e| BuildError::emit(file.clone(), e))?;
        maps.push((file.clone(), *number, STUB_ENTRY));
        for index in doors {
            let exit = &door_exits[*index];
            report.exits.push(ExitReport {
                door: exit.door.clone(),
                door_hex: exit.door_hex,
                interior: exit.interior.clone(),
                interior_editor_id: exit.interior_editor_id.clone(),
                stub_file: file.clone(),
                stub_number: *number,
                grid_hexes: exit.grid_hexes.clone(),
                grid_steps: exit.grid_steps,
                return_hex: exit.return_hex,
                return_orientation: exit.return_orientation,
                return_from_teleport: exit.return_from_teleport,
            });
        }
    }
    report
        .exits
        .sort_by(|a, b| a.door.cmp(&b.door).then(a.door_hex.cmp(&b.door_hex)));

    // Registration, and the map table Fallout 2 transitions read.
    for (file, number, _) in &maps {
        if ship_maps_txt && *number > MAX_MAP {
            return Err(BuildError::Input(format!(
                "map {file} would be number {number}; data/maps.txt holds numbers up to {MAX_MAP}"
            )));
        }
        layer
            .register_map(file, *number)
            .map_err(|e| BuildError::emit("vault-mod.toml", e))?;
    }
    if let (true, Some(base)) = (ship_maps_txt, maps_txt) {
        let mut text = base.clone();
        if !text.is_empty() && !text.ends_with(b"\n") {
            text.push(b'\n');
        }
        for (file, number, entry) in &maps {
            let stem = file.trim_end_matches(".map");
            text.extend_from_slice(
                format!(
                    "\n[Map {number:03}]\nmap_name={stem}\nrandom_start_point_0=elev:0, tile_num:{entry}\n"
                )
                .as_bytes(),
            );
        }
        layer
            .insert(MAPS_TXT, text)
            .map_err(|e| BuildError::emit(MAPS_TXT, e))?;
        report.maps_txt = true;
    } else if maps_txt.is_none() {
        report.notes.push(
            "no base data/maps.txt: maps are registered in vault-mod.toml only (under Fallout 2 \
             the engine resolves exit-grid map numbers through data/maps.txt)"
                .into(),
        );
    }
    report.maps = maps
        .iter()
        .map(|(file, number, _)| (file.clone(), *number))
        .collect();

    lists.finish(&mut layer)?;
    for category in &lists.touched_protos {
        let base = input
            .base_files
            .get(&proto::prototype_list_path(*category))
            .map_or(Ok(0), |bytes| {
                ProtoList::from_base(*category, bytes).map(|l| l.len())
            })
            .map_err(|e| base_error(&proto::prototype_list_path(*category), e))?;
        report.prototypes.insert(
            category.directory().into(),
            lists.protos[category].len() - base,
        );
    }

    report.verify = crate::verify_layer(&layer, input.base_files)?;
    if let Some(interior) = interior {
        let maps = &report.verify.maps;
        if maps.len() != 1 || maps.iter().any(|map| map.exit_grids > 0) {
            return Err(BuildError::verify(
                &report.map_file,
                format!(
                    "an interior demo is one map without exit grids; the layer holds {} maps \
                     and {} exit grids",
                    maps.len(),
                    maps.iter().map(|map| map.exit_grids).sum::<usize>()
                ),
            ));
        }
        let (near, given) = entrance_near.unwrap_or((window_centre, false));
        let mut account = InteriorReport {
            cells: input
                .cells
                .iter()
                .filter(|cell| cell.is_interior())
                .map(|cell| (cell.form.to_string(), cell.editor_id.clone()))
                .collect(),
            floor_z: interior.floor_z,
            band: interior.band(),
            entrance_near: [near[0] as f32, near[1] as f32],
            entrance_near_given: given,
            doors: interior_doors,
            ..InteriorReport::default()
        };
        let other = |index: usize| match levels.get(index) {
            Some(CritterLevel::Other(z)) => Some(*z),
            _ => None,
        };
        for critter in &report.critters {
            if let Some(z) = other(critter.index) {
                account.clamped.push((critter.reference.clone(), z));
            }
        }
        account.other_level = report
            .skipped_critters
            .iter()
            .filter(|skipped| {
                other(skipped.index).is_some() && skipped.reason.starts_with("on another level")
            })
            .cloned()
            .collect();
        account.unknown_level = input
            .critters
            .iter()
            .zip(&levels)
            .filter(|(_, level)| matches!(level, CritterLevel::Unknown))
            .map(|(critter, _)| critter.reference.clone())
            .collect();
        if !account.unknown_level.is_empty() {
            report.notes.push(format!(
                "{} critters name references the interior cells do not hold; their level is \
                 unknown and they were placed as on the floor level",
                account.unknown_level.len()
            ));
        }
        report.interior = Some(account);
    }
    Ok(Built {
        layer,
        walkable,
        report,
    })
}
